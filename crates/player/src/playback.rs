//! Playback: three threads behind the window.
//!
//! - the reading thread takes packets from the file, decodes sound straight into the output
//!   device's buffer and passes video packets on;
//! - the video thread decodes pictures;
//! - the render thread shows the picture that belongs to the current time.
//!
//! Time comes from the sound device (see `bsv_audio`), so the picture follows the sound. A file
//! without sound, or a machine without an output device, falls back to the system clock.

use std::collections::VecDeque;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{
    Receiver, Sender, SyncSender, TryRecvError, TrySendError, channel, sync_channel,
};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use bsv_audio::AudioOutput;
use bsv_display::Display;
use bsv_display::policy::{HdrMode, Output, plan};

use crate::language;
use bsv_engine::{
    AudioDecoder, AudioInfo, Decoded, Frame, Gpu, MediaInfo, Packet, Pull, Quality, Read,
    RenderStats, RenderStatus, Source, SubtitleDecoder, SubtitleEvent, SubtitleKind, Track,
    TrackKind, VideoDecoder,
};

/// Decoded frames waiting for the renderer. A 4K 10-bit frame is about 25 MB.
const FRAMES_AHEAD: usize = 8;
/// Compressed video kept ahead of the decoder, in seconds of content. It also bounds how far
/// apart a file may store a moment's picture and its sound before one of them runs dry.
const VIDEO_PACKETS_AHEAD: f64 = 1.5;
/// Longest stretch of silence inserted when a sound track starts later than the picture.
const MAX_LEADING_SILENCE: f64 = 10.0;

pub struct Options {
    pub path: String,
    pub hardware: bool,
    pub mode: HdrMode,
    pub quality: Quality,
    /// Extra libplacebo options, "key=value,key=value".
    pub renderer_options: String,
    /// Where to begin, in seconds.
    pub start: f64,
    pub sound: bool,
    /// Seconds to hold the picture back against the sound: positive when the sound arrives
    /// late (a TV or receiver that delays it).
    pub audio_delay: f64,
    /// 0.0 to 1.0.
    pub volume: f32,
    pub muted: bool,
    /// Pixels at the bottom of the window that subtitles start out clear of.
    pub subtitle_lift: f32,
    /// Subtitle text size, 1.0 as styled; and how bright subtitle white is, in cd/m².
    pub subtitle_scale: f32,
    pub subtitle_nits: f32,
    /// Languages to choose tracks by, as ISO 639 codes. Empty: sound as the file prefers,
    /// subtitles only when forced.
    pub audio_language: String,
    pub subtitle_language: String,
}

pub enum Command {
    Quit,
    Resize(u32, u32),
    TogglePause,
    SeekBy(f64),
    SeekTo(f64),
    /// The window is on this display now, or the display's state changed.
    Display(Box<Display>),
    CycleHdrMode,
    VolumeBy(f32),
    SetVolume(f32),
    ToggleMute,
    SetHdrMode(HdrMode),
    /// Play another sound track, by its stream number.
    SelectAudio(i32),
    /// Show this subtitle track, or none.
    SelectSubtitle(Option<i32>),
    /// Keep subtitles clear of this many pixels at the bottom (the controls are showing there).
    SubtitleLift(f32),
    /// Subtitle text size (1.0 as styled) and white level in cd/m².
    SubtitleLook {
        scale: f32,
        nits: f32,
    },
    SetQuality(Quality),
    /// Seconds to hold the picture back against the sound.
    SetAudioDelay(f64),
}

/// The sound track and the device it plays on.
#[derive(Clone, PartialEq)]
pub struct Sound {
    pub track: AudioInfo,
    pub device: bsv_audio::Format,
}

#[derive(Clone, Default)]
pub struct Status {
    pub position: f64,
    pub paused: bool,
    pub ended: bool,
    pub info: Option<MediaInfo>,
    pub stats: RenderStats,
    pub mode: HdrMode,
    pub output: Option<Output>,
    pub sound: Option<Sound>,
    /// Every sound and subtitle track in the file.
    pub tracks: Vec<Track>,
    pub audio_track: Option<i32>,
    pub subtitle_track: Option<i32>,
    /// 0.0 to 1.0.
    pub volume: f32,
    pub muted: bool,
    /// Gaps in the sound since the start: the device wanted samples that were not ready.
    pub underruns: u64,
    /// Set when playback cannot go on; the thread has stopped.
    pub error: Option<String>,
}

pub struct Player {
    commands: Sender<Command>,
    status: Arc<Mutex<Status>>,
    thread: Option<JoinHandle<()>>,
}

struct LayerPtr(*mut c_void);
// SAFETY: the layer is only used by the render thread; `Player::start`'s caller keeps it alive.
unsafe impl Send for LayerPtr {}

impl Player {
    /// Start playing into a `CAMetalLayer` (the Mac) or a window, by its handle (Windows).
    ///
    /// # Safety
    /// `layer` must stay alive until the returned `Player` is dropped.
    pub unsafe fn start(
        layer: *mut c_void,
        size: (u32, u32),
        display: Display,
        options: Options,
    ) -> Player {
        let (commands, rx) = channel();
        let status = Arc::new(Mutex::new(Status {
            mode: options.mode,
            volume: options.volume,
            muted: options.muted,
            ..Status::default()
        }));
        let shared = status.clone();
        let layer = LayerPtr(layer);
        let thread = thread::Builder::new()
            .name("render".into())
            .spawn(move || {
                let layer = layer;
                if let Err(e) = render_loop(layer.0, size, display, options, rx, &shared) {
                    shared.lock().unwrap().error = Some(e.to_string());
                }
            })
            .expect("spawn the render thread");
        Player {
            commands,
            status,
            thread: Some(thread),
        }
    }

    pub fn send(&self, command: Command) {
        let _ = self.commands.send(command);
    }

    pub fn status(&self) -> Status {
        self.status.lock().unwrap().clone()
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.send(Command::Quit);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

// ---- Time -----------------------------------------------------------------------------------

/// A clock that can be stopped and set: the time reference when there is no sound.
struct SystemClock {
    base: f64,
    running_since: Option<Instant>,
}

impl SystemClock {
    fn stopped_at(position: f64) -> SystemClock {
        SystemClock {
            base: position,
            running_since: None,
        }
    }

    fn position(&self) -> f64 {
        self.base
            + self
                .running_since
                .map_or(0.0, |t| t.elapsed().as_secs_f64())
    }

    fn stop(&mut self) {
        self.base = self.position();
        self.running_since = None;
    }

    fn run(&mut self) {
        if self.running_since.is_none() {
            self.running_since = Some(Instant::now());
        }
    }
}

/// Playback time: the sound device's when there is sound, the system's otherwise.
struct Timeline {
    sound: Option<(bsv_audio::Clock, bsv_audio::Controls)>,
    system: SystemClock,
    /// Time stands at `floor`: paused, or waiting for the first picture after a seek.
    held: bool,
    /// Time never runs behind this: the seek target, or the moment of the last pause.
    floor: f64,
    audio_delay: f64,
}

impl Timeline {
    fn position(&self) -> f64 {
        if self.held {
            return self.floor;
        }
        match &self.sound {
            // No reading yet means the sound for this position is still on its way.
            Some((clock, _)) => clock
                .position()
                .map_or(self.floor, |p| (p - self.audio_delay).max(self.floor)),
            None => self.system.position(),
        }
    }

    fn run(&mut self) {
        self.held = false;
        if let Some((_, controls)) = &self.sound {
            controls.set_playing(true);
        }
        self.system.run();
    }

    fn hold(&mut self) {
        self.floor = self.position();
        self.held = true;
        if let Some((_, controls)) = &self.sound {
            controls.set_playing(false);
        }
        self.system.stop();
    }

    fn jump(&mut self, to: f64) {
        self.hold();
        self.floor = to;
        self.system = SystemClock::stopped_at(to);
    }
}

// ---- Between the threads --------------------------------------------------------------------

/// State every thread reads: which seek is current, and whether to stop.
struct Shared {
    generation: AtomicU64,
    quit: AtomicBool,
}

impl Shared {
    /// Work started for `generation` is no longer wanted.
    fn stale(&self, generation: u64) -> bool {
        self.quit.load(Ordering::Relaxed) || self.generation.load(Ordering::Relaxed) != generation
    }
}

enum Item {
    Packet(Packet),
    /// A seek happened: forget what is in flight, then drop frames that end before `to`.
    Flush {
        generation: u64,
        to: f64,
    },
    End,
    Failed(String),
}

/// Video packets on their way to the decoder, bounded by how much playing time they hold.
struct PacketQueue {
    state: Mutex<(VecDeque<Item>, f64)>,
    changed: Condvar,
}

impl PacketQueue {
    fn new() -> PacketQueue {
        PacketQueue {
            state: Mutex::new((VecDeque::new(), 0.0)),
            changed: Condvar::new(),
        }
    }

    /// Add a packet, waiting while the queue is full. Returns `false` (and drops the packet)
    /// when a seek or quit made it stale in the meantime.
    fn push(&self, packet: Packet, generation: u64, shared: &Shared) -> bool {
        let mut state = self.state.lock().unwrap();
        while state.1 >= VIDEO_PACKETS_AHEAD {
            if shared.stale(generation) {
                return false;
            }
            state = self
                .changed
                .wait_timeout(state, Duration::from_millis(20))
                .unwrap()
                .0;
        }
        state.1 += packet.duration;
        state.0.push_back(Item::Packet(packet));
        self.changed.notify_all();
        true
    }

    fn push_control(&self, item: Item) {
        self.state.lock().unwrap().0.push_back(item);
        self.changed.notify_all();
    }

    fn pop(&self, timeout: Duration) -> Option<Item> {
        let mut state = self.state.lock().unwrap();
        if state.0.is_empty() {
            state = self.changed.wait_timeout(state, timeout).unwrap().0;
        }
        let item = state.0.pop_front()?;
        if let Item::Packet(p) = &item {
            state.1 = (state.1 - p.duration).max(0.0);
        }
        self.changed.notify_all();
        Some(item)
    }

    fn clear(&self) {
        let mut state = self.state.lock().unwrap();
        state.0.clear();
        state.1 = 0.0;
        self.changed.notify_all();
    }
}

enum ReaderCommand {
    /// Go to `to`. Optionally switch to another sound track or subtitle track on the way: a
    /// track change is a seek to where playback already is, so everything restarts in step.
    Seek {
        to: f64,
        generation: u64,
        audio: Option<i32>,
        subtitle: Option<Option<i32>>,
    },
    /// The subtitle files found next to the video, in the order of their track numbers.
    Sidecars(Vec<String>),
}

/// News from the reading thread that is not a packet.
enum FromReader {
    /// A subtitle track was opened (what it is made of), or subtitles were turned off.
    Subtitles {
        generation: u64,
        kind: Option<SubtitleKind>,
    },
    Subtitle {
        generation: u64,
        event: SubtitleEvent,
    },
    /// The sound track now playing.
    Sound(AudioInfo),
}

/// What the reading thread reports once it has tried to open the sound device.
struct SoundSetup {
    clock: bsv_audio::Clock,
    controls: bsv_audio::Controls,
    sound: Sound,
}

/// Messages from the video thread to the renderer.
enum FromVideo {
    Frame {
        generation: u64,
        frame: Frame,
    },
    End {
        generation: u64,
    },
    /// Sent once real decoding has started: HDR metadata from the bitstream, and whether the
    /// hardware took the stream, are known only then.
    Info(MediaInfo),
    Failed(String),
}

// ---- The reading thread ---------------------------------------------------------------------

struct SoundPath {
    output: AudioOutput,
    decoder: AudioDecoder,
    /// The time base has been declared for the current position.
    based: bool,
}

impl SoundPath {
    fn open(source: &Source, track: &AudioInfo) -> Result<SoundPath, String> {
        let output = AudioOutput::open(track.sample_rate).map_err(|e| e.to_string())?;
        let format = output.format();
        let decoder = AudioDecoder::open(source, format.sample_rate, format.channels)
            .map_err(|e| e.to_string())?;
        Ok(SoundPath {
            output,
            decoder,
            based: false,
        })
    }

    /// Move everything the decoder has ready into the device's buffer. `start` is where
    /// playback begins (zero, or the last seek target): sound before it is cut away.
    fn pump(&mut self, start: f64, generation: u64, shared: &Shared) -> Result<(), String> {
        let rate = self.output.format().sample_rate as f64;
        let channels = self.output.format().channels as usize;
        loop {
            let samples = match self.decoder.receive().map_err(|e| e.to_string())? {
                Decoded::Some(s) => s,
                Decoded::NeedsPacket | Decoded::Finished => return Ok(()),
            };
            let mut data = samples.data;
            if !self.based {
                let frames = data.len() / channels;
                let end = samples.pts + frames as f64 / rate;
                if end <= start {
                    continue; // entirely before the starting point
                }
                if samples.pts < start {
                    let skip = (((start - samples.pts) * rate).round() as usize).min(frames);
                    data = &data[skip * channels..];
                } else if samples.pts > start + 0.005 {
                    // The track begins after the picture: fill the gap so both stay on one timeline.
                    let gap = (samples.pts - start).min(MAX_LEADING_SILENCE);
                    let silence = vec![0.0f32; (gap * rate) as usize * channels];
                    self.output.set_base(start);
                    self.based = true;
                    if !write_all(&mut self.output, &silence, generation, shared) {
                        return Ok(());
                    }
                }
                if !self.based {
                    self.output.set_base(start.max(samples.pts));
                    self.based = true;
                }
            }
            if !write_all(&mut self.output, data, generation, shared) {
                return Ok(());
            }
        }
    }
}

/// Queue samples, waiting for room. Returns `false` if a seek or quit interrupted it.
fn write_all(output: &mut AudioOutput, mut data: &[f32], generation: u64, shared: &Shared) -> bool {
    while !data.is_empty() {
        let taken = output.write(data);
        data = &data[taken..];
        if taken == 0 {
            if shared.stale(generation) {
                return false;
            }
            thread::sleep(Duration::from_millis(2));
        }
    }
    true
}

/// Track numbers from here up stand for subtitle files next to the video, not streams inside it.
const SIDECAR_BASE: i32 = 1000;

/// Subtitle files that belong to this video: same folder, same name, another ending
/// ("film.srt", "film.ru.ass"). Returned with the language the name suggests.
fn find_sidecars(video: &str) -> Vec<(String, String)> {
    let path = std::path::Path::new(video);
    let (Some(folder), Some(stem)) = (path.parent(), path.file_stem().and_then(|s| s.to_str()))
    else {
        return Vec::new();
    };
    let folder = if folder.as_os_str().is_empty() {
        std::path::Path::new(".")
    } else {
        folder
    };
    let Ok(entries) = std::fs::read_dir(folder) else {
        return Vec::new();
    };
    let mut found: Vec<(String, String)> = entries
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name().to_str()?.to_string();
            let rest = name.strip_prefix(stem)?.strip_prefix('.')?;
            let (middle, extension) = rest.rsplit_once('.').unwrap_or(("", rest));
            let known =
                ["srt", "ass", "ssa", "vtt"].contains(&extension.to_ascii_lowercase().as_str());
            // "film.ru.srt", "film.eng.forced.srt": the first short word is taken as the language.
            let language = middle
                .split('.')
                .find(|part| {
                    (2..=3).contains(&part.len()) && part.chars().all(|c| c.is_ascii_alphabetic())
                })
                .unwrap_or("")
                .to_ascii_lowercase();
            known.then(|| (e.path().to_string_lossy().into_owned(), language))
        })
        .collect();
    found.sort();
    found
}

/// Read a whole subtitle file and send everything in it. These files are small, so this is
/// simply done again after each seek.
fn send_sidecar(path: &str, generation: u64, news: &Sender<FromReader>) {
    let opened = Source::open_sidecar(path)
        .and_then(|source| SubtitleDecoder::open(&source).map(|d| (source, d)));
    let (mut source, mut decoder) = match opened {
        Ok(pair) => pair,
        Err(e) => {
            eprintln!("[warn] subtitles cannot be shown: {e}");
            let _ = news.send(FromReader::Subtitles {
                generation,
                kind: None,
            });
            return;
        }
    };
    let _ = news.send(FromReader::Subtitles {
        generation,
        kind: Some(decoder.kind()),
    });
    while let Ok(Read::Subtitle(packet)) = source.read() {
        if let Some(event) = decoder.decode(packet) {
            let _ = news.send(FromReader::Subtitle { generation, event });
        }
    }
}

/// Open a decoder for the subtitle track the source has selected, and say what it is.
fn open_subtitles(
    source: &Source,
    generation: u64,
    news: &Sender<FromReader>,
) -> Option<SubtitleDecoder> {
    let decoder = match source.subtitle_index() {
        Some(_) => SubtitleDecoder::open(source)
            .map_err(|e| eprintln!("[warn] subtitles cannot be shown: {e}"))
            .ok(),
        None => None,
    };
    let _ = news.send(FromReader::Subtitles {
        generation,
        kind: decoder.as_ref().map(SubtitleDecoder::kind),
    });
    decoder
}

/// What the reading thread is given to work with, besides the file itself.
struct Reading {
    sound_wanted: bool,
    /// Answered once: whether a sound device could be opened.
    setup: Sender<Option<SoundSetup>>,
    commands: Receiver<ReaderCommand>,
    video: Arc<PacketQueue>,
    news: Sender<FromReader>,
    shared: Arc<Shared>,
}

fn read_loop(mut source: Source, reading: Reading) {
    let Reading {
        sound_wanted,
        setup,
        commands,
        video,
        news,
        shared,
    } = reading;
    // Subtitle files next to the video, in the order of their track numbers (they are found
    // a moment after playback starts), and the one of them being shown.
    let mut sidecars: Vec<String> = Vec::new();
    let mut sidecar: Option<String> = None;
    // The output device is opened here because its stream must stay on the thread that made it.
    let mut sound = None;
    if let Some(track) = source.audio_info().filter(|_| sound_wanted) {
        match SoundPath::open(&source, &track) {
            Ok(path) => {
                let _ = setup.send(Some(SoundSetup {
                    clock: path.output.clock(),
                    controls: path.output.controls(),
                    sound: Sound {
                        track,
                        device: path.output.format().clone(),
                    },
                }));
                sound = Some(path);
            }
            Err(e) => eprintln!("[warn] playing without sound: {e}"),
        }
    }
    if sound.is_none() {
        let _ = setup.send(None);
    }

    let mut generation = 0u64;
    let mut start = 0.0f64;
    let mut idle = false; // at the end of the file, or after an error: wait for a seek
    let mut subtitles = open_subtitles(&source, generation, &news);

    loop {
        if shared.quit.load(Ordering::Relaxed) {
            return;
        }
        let command = if idle {
            match commands.recv_timeout(Duration::from_millis(50)) {
                Ok(c) => Some(c),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
            }
        } else {
            match commands.try_recv() {
                Ok(c) => Some(c),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => return,
            }
        };
        if let Some(ReaderCommand::Sidecars(found)) = command {
            sidecars = found;
            continue;
        }
        if let Some(ReaderCommand::Seek {
            to,
            generation: g,
            audio,
            subtitle,
        }) = command
        {
            generation = g;
            start = to;
            idle = false;
            if let Some(index) = audio
                && let Some(path) = &mut sound
            {
                let format = path.output.format().clone();
                let reopened = source.select_audio(index).and_then(|()| {
                    AudioDecoder::open(&source, format.sample_rate, format.channels)
                });
                match reopened {
                    Ok(decoder) => {
                        path.decoder = decoder;
                        if let Some(track) = source.audio_info() {
                            let _ = news.send(FromReader::Sound(track));
                        }
                    }
                    Err(e) => eprintln!("[warn] cannot switch the sound track: {e}"),
                }
            }
            if let Some(choice) = subtitle {
                // A file next to the video, or a stream inside it.
                sidecar = choice
                    .filter(|&i| i >= SIDECAR_BASE)
                    .and_then(|i| sidecars.get((i - SIDECAR_BASE) as usize).cloned());
                let inside = if sidecar.is_some() { None } else { choice };
                if let Err(e) = source.select_subtitle(inside) {
                    eprintln!("[warn] {e}");
                }
                subtitles = None;
                if sidecar.is_none() {
                    subtitles = open_subtitles(&source, generation, &news);
                }
            }
            if let Some(path) = &sidecar {
                send_sidecar(path, generation, &news);
            }
            let moved = source.seek(to);
            // Sound is silenced first: once the video thread sees the flush, the device's clock
            // no longer reports the old position.
            if let Some(path) = &mut sound {
                path.decoder.flush();
                path.output.flush();
                path.based = false;
            }
            video.clear();
            video.push_control(Item::Flush { generation, to });
            if let Err(e) = moved {
                video.push_control(Item::Failed(e.to_string()));
                idle = true;
            }
            continue;
        }

        let failed = match source.read() {
            Ok(Read::Video(packet)) => {
                video.push(packet, generation, &shared);
                None
            }
            Ok(Read::Audio(packet)) => match &mut sound {
                Some(path) => path
                    .decoder
                    .send(Some(packet))
                    .map_err(|e| e.to_string())
                    .and_then(|()| path.pump(start, generation, &shared))
                    .err(),
                None => None,
            },
            Ok(Read::Subtitle(packet)) => {
                if let Some(event) = subtitles.as_mut().and_then(|d| d.decode(packet)) {
                    let _ = news.send(FromReader::Subtitle { generation, event });
                }
                None
            }
            Ok(Read::End) => {
                video.push_control(Item::End);
                if let Some(path) = &mut sound {
                    let _ = path.decoder.send(None);
                    let _ = path.pump(start, generation, &shared);
                    path.output.set_ended();
                }
                idle = true;
                None
            }
            Err(e) => Some(e.to_string()),
        };
        if let Some(e) = failed {
            video.push_control(Item::Failed(e));
            idle = true;
        }
    }
}

// ---- The video thread -----------------------------------------------------------------------

fn video_loop(
    mut decoder: VideoDecoder,
    packets: Arc<PacketQueue>,
    out: SyncSender<FromVideo>,
    shared: Arc<Shared>,
) {
    let mut generation = 0u64;
    let mut skip_before = 0.0f64;
    let mut info_sent = false;

    // Hand a message over, giving up if a seek or quit makes it stale while the queue is full.
    // Returns `false` only when the renderer is gone.
    let deliver = |mut message: FromVideo, generation: u64| -> bool {
        loop {
            match out.try_send(message) {
                Ok(()) => return true,
                Err(TrySendError::Disconnected(_)) => return false,
                Err(TrySendError::Full(back)) => message = back,
            }
            if shared.stale(generation) {
                return true;
            }
            thread::sleep(Duration::from_millis(2));
        }
    };

    loop {
        if shared.quit.load(Ordering::Relaxed) {
            return;
        }
        let Some(item) = packets.pop(Duration::from_millis(50)) else {
            continue;
        };
        let sent = match item {
            Item::Flush { generation: g, to } => {
                decoder.flush(to);
                generation = g;
                skip_before = to;
                continue;
            }
            Item::Failed(e) => {
                deliver(FromVideo::Failed(e), generation);
                continue;
            }
            Item::End => decoder.send(None),
            Item::Packet(packet) => decoder.send(Some(packet)),
        };
        if let Err(e) = sent {
            deliver(FromVideo::Failed(e.to_string()), generation);
            continue;
        }
        loop {
            match decoder.receive() {
                Ok(Decoded::Some(frame)) => {
                    if !info_sent {
                        info_sent = true;
                        deliver(FromVideo::Info(decoder.info()), generation);
                    }
                    // After a seek the decoder starts at a keyframe before the target.
                    if frame.pts + frame.duration <= skip_before {
                        continue;
                    }
                    if !deliver(FromVideo::Frame { generation, frame }, generation) {
                        return;
                    }
                }
                Ok(Decoded::NeedsPacket) => break,
                Ok(Decoded::Finished) => {
                    deliver(FromVideo::End { generation }, generation);
                    break;
                }
                Err(e) => {
                    deliver(FromVideo::Failed(e.to_string()), generation);
                    break;
                }
            }
        }
    }
}

// ---- The render thread ----------------------------------------------------------------------

/// Loudness as heard is far from linear in amplitude: a squared control feels even.
fn gain_for(volume: f32, muted: bool) -> f32 {
    if muted { 0.0 } else { volume * volume }
}

fn render_loop(
    layer: *mut c_void,
    size: (u32, u32),
    mut display: Display,
    options: Options,
    commands: Receiver<Command>,
    status: &Mutex<Status>,
) -> Result<(), bsv_engine::Error> {
    // SAFETY: `Player::start`'s caller keeps the layer (or window) alive until this thread is joined.
    #[cfg(target_os = "macos")]
    let mut gpu = unsafe { Gpu::for_metal_layer(layer) }?;
    #[cfg(windows)]
    let mut gpu = unsafe { Gpu::for_window(layer) }?;
    gpu.set_options(options.quality, &options.renderer_options)?;
    gpu.resize(size.0, size.1);

    let mut source = Source::open(&options.path)?;
    if !source.has_video() {
        return Err(bsv_engine::Error("the file has no video".into()));
    }
    let decoder = VideoDecoder::open(&source, options.hardware)?;
    let mut info = decoder.info();
    let duration = info.duration;

    let mut tracks = source.tracks();
    for (name, data) in source.fonts() {
        gpu.add_font(&name, &data);
    }
    // Subtitle files next to the video are looked for on the side: listing a folder can wait
    // on the system asking the user for access to it, and the film should not wait with it.
    let (found_tx, found_sidecars) = channel();
    {
        let path = options.path.clone();
        thread::Builder::new()
            .name("subtitle files".into())
            .spawn(move || {
                let _ = found_tx.send(find_sidecars(&path));
            })
            .expect("spawn the subtitle file search");
    }
    // Sound in the language asked for, when the file has it; otherwise as the file prefers.
    if let Some(track) = tracks.iter().find(|t| {
        t.kind == TrackKind::Audio && language::same(&t.language, &options.audio_language)
    }) {
        let _ = source.select_audio(track.index);
    }
    // Subtitles on from the start, in order of preference: a full track in the language asked
    // for; forced subtitles (signs, a line in another language), which are part of the film.
    // Anything else waits to be asked for. A subtitle file next to the video can still take
    // over when the search for one comes back.
    let subtitle = |wanted: &dyn Fn(&Track) -> bool| {
        tracks
            .iter()
            .find(|t| t.kind == TrackKind::Subtitle && wanted(t))
            .map(|t| t.index)
    };
    let in_language = |t: &Track| language::same(&t.language, &options.subtitle_language);
    let mut subtitle_track = subtitle(&|t| in_language(t) && !t.forced && !t.hearing_impaired)
        .or_else(|| subtitle(&|t| in_language(t)))
        .or_else(|| subtitle(&|t| t.forced));
    let chosen_by_language =
        subtitle_track.is_some_and(|i| tracks.iter().any(|t| t.index == i && in_language(t)));
    if subtitle_track.is_some() && source.select_subtitle(subtitle_track).is_err() {
        subtitle_track = None;
    }
    let mut audio_track = source.audio_index().filter(|_| options.sound);

    let shared = Arc::new(Shared {
        generation: AtomicU64::new(0),
        quit: AtomicBool::new(false),
    });
    let packets = Arc::new(PacketQueue::new());
    let (reader_commands, reader_rx) = channel();
    let (setup_tx, setup_rx) = channel();
    let (news_tx, news) = channel();
    let (frames_tx, frames) = sync_channel(FRAMES_AHEAD);

    let reader_thread = {
        let (packets, shared, sound) = (packets.clone(), shared.clone(), options.sound);
        thread::Builder::new()
            .name("read".into())
            .spawn(move || {
                read_loop(
                    source,
                    Reading {
                        sound_wanted: sound,
                        setup: setup_tx,
                        commands: reader_rx,
                        video: packets,
                        news: news_tx,
                        shared,
                    },
                )
            })
            .expect("spawn the reading thread")
    };
    let video_thread = {
        let (packets, shared) = (packets.clone(), shared.clone());
        thread::Builder::new()
            .name("video".into())
            .spawn(move || video_loop(decoder, packets, frames_tx, shared))
            .expect("spawn the video thread")
    };

    let setup = setup_rx.recv_timeout(Duration::from_secs(5)).ok().flatten();
    let mut sound = setup.as_ref().map(|s| s.sound.clone());
    if sound.is_none() {
        audio_track = None;
    }
    let mut timeline = Timeline {
        sound: setup.map(|s| (s.clock, s.controls)),
        system: SystemClock::stopped_at(0.0),
        held: true,
        floor: 0.0,
        audio_delay: options.audio_delay,
    };

    let mut mode = options.mode;
    let mut output = plan(&display, &info.source(), mode);
    let mut generation = 0u64;
    let mut paused = false;
    let mut ended = false;
    // After opening or seeking, time stands still until the first picture is on screen.
    let mut waiting_for_picture = true;
    let mut volume = options.volume;
    let mut muted = options.muted;
    let mut stats = RenderStats::default();
    let mut failures = 0u32;
    let mut result = Ok(());
    // Commands the loop gives itself: the starting position, a subtitle file that turned up.
    let mut pending: VecDeque<Command> = VecDeque::new();
    if options.start > 0.0 {
        pending.push_back(Command::SeekBy(options.start));
    }
    let mut sidecars_known = false;
    // Subtitles glide out of the controls' way rather than jump.
    let mut lift_wanted = options.subtitle_lift;
    let mut lift = options.subtitle_lift;
    gpu.set_subtitle_lift(lift);
    gpu.set_subtitle_look(options.subtitle_scale, options.subtitle_nits);

    'playing: loop {
        loop {
            let command = match pending.pop_front() {
                Some(command) => Ok(command),
                None => commands.try_recv(),
            };
            // Where to go next, with the track changes to make on the way.
            let mut jump: Option<(f64, Option<i32>, Option<Option<i32>>)> = None;
            match command {
                Ok(Command::Quit) | Err(TryRecvError::Disconnected) => break 'playing,
                Ok(Command::Resize(w, h)) => gpu.resize(w, h),
                Ok(Command::TogglePause) => {
                    paused = !paused;
                    if paused {
                        timeline.hold();
                    } else if !waiting_for_picture && !ended {
                        timeline.run();
                    }
                }
                Ok(seek @ (Command::SeekBy(_) | Command::SeekTo(_))) => {
                    let limit = if duration > 0.0 {
                        (duration - 0.5).max(0.0)
                    } else {
                        f64::MAX
                    };
                    let wanted = match seek {
                        Command::SeekBy(delta) => timeline.position() + delta,
                        Command::SeekTo(position) => position,
                        _ => unreachable!(),
                    };
                    jump = Some((wanted.clamp(0.0, limit), None, None));
                }
                Ok(Command::SelectAudio(index)) => {
                    let known = tracks
                        .iter()
                        .any(|t| t.kind == TrackKind::Audio && t.index == index);
                    if known && sound.is_some() && audio_track != Some(index) {
                        audio_track = Some(index);
                        jump = Some((timeline.position(), Some(index), None));
                    }
                }
                Ok(Command::SelectSubtitle(choice)) => {
                    let known = choice.is_none_or(|i| {
                        tracks
                            .iter()
                            .any(|t| t.kind == TrackKind::Subtitle && t.index == i)
                    });
                    if known && subtitle_track != choice {
                        subtitle_track = choice;
                        // The old track's lines go at once; the new track's arrive after the jump.
                        gpu.begin_subtitles(None, (info.width, info.height));
                        jump = Some((timeline.position(), None, Some(choice)));
                    }
                }
                Ok(Command::Display(d)) => {
                    display = *d;
                    output = plan(&display, &info.source(), mode);
                }
                Ok(Command::CycleHdrMode) => {
                    mode = mode.next();
                    output = plan(&display, &info.source(), mode);
                }
                Ok(Command::VolumeBy(step)) => {
                    volume = (volume + step).clamp(0.0, 1.0);
                    muted = false;
                }
                Ok(Command::SetVolume(v)) => {
                    volume = v.clamp(0.0, 1.0);
                    muted = false;
                }
                Ok(Command::ToggleMute) => muted = !muted,
                Ok(Command::SubtitleLift(pixels)) => lift_wanted = pixels,
                Ok(Command::SubtitleLook { scale, nits }) => gpu.set_subtitle_look(scale, nits),
                Ok(Command::SetQuality(quality)) => {
                    if let Err(e) = gpu.set_options(quality, &options.renderer_options) {
                        eprintln!("[warn] {e}");
                    }
                }
                Ok(Command::SetAudioDelay(seconds)) => timeline.audio_delay = seconds,
                Ok(Command::SetHdrMode(m)) => {
                    mode = m;
                    output = plan(&display, &info.source(), mode);
                }
                Err(TryRecvError::Empty) => break,
            }
            if let Some((to, audio, subtitle)) = jump {
                generation += 1;
                shared.generation.store(generation, Ordering::Relaxed);
                let _ = reader_commands.send(ReaderCommand::Seek {
                    to,
                    generation,
                    audio,
                    subtitle,
                });
                gpu.reset();
                gpu.clear_subtitles();
                timeline.jump(to);
                waiting_for_picture = true;
                ended = false;
            }
        }
        if (lift - lift_wanted).abs() > 0.5 {
            lift += (lift_wanted - lift) * 0.18;
            gpu.set_subtitle_lift(lift);
        }
        // The search for subtitle files next to the video has come back.
        if !sidecars_known && let Ok(found) = found_sidecars.try_recv() {
            sidecars_known = true;
            for (n, (path, language)) in found.iter().enumerate() {
                let name = std::path::Path::new(path)
                    .file_name()
                    .map(|f| f.to_string_lossy().into_owned());
                tracks.push(Track {
                    index: SIDECAR_BASE + n as i32,
                    kind: TrackKind::Subtitle,
                    codec: String::new(),
                    profile: String::new(),
                    language: language.clone(),
                    title: name.unwrap_or_default(),
                    layout: String::new(),
                    channels: 0,
                    sample_rate: 0,
                    is_default: false,
                    forced: false,
                    hearing_impaired: false,
                    bitmap: false,
                });
            }
            let _ = reader_commands.send(ReaderCommand::Sidecars(
                found.iter().map(|(p, _)| p.clone()).collect(),
            ));
            // A file someone put next to the video is shown without being asked for, unless a
            // track in the preferred language is already on and this file is not in it.
            let wanted = found
                .iter()
                .position(|(_, l)| language::same(l, &options.subtitle_language))
                .or((!chosen_by_language && !found.is_empty()).then_some(0));
            if let Some(n) = wanted {
                pending.push_back(Command::SelectSubtitle(Some(SIDECAR_BASE + n as i32)));
            }
        }
        // Subtitles and track changes from the reading thread.
        while let Ok(item) = news.try_recv() {
            match item {
                FromReader::Subtitles {
                    generation: g,
                    kind,
                } if g == generation => {
                    gpu.begin_subtitles(kind.as_ref(), (info.width, info.height));
                }
                FromReader::Subtitle {
                    generation: g,
                    event,
                } if g == generation => gpu.push_subtitle(event),
                // Left over from before the last seek.
                FromReader::Subtitles { .. } | FromReader::Subtitle { .. } => {}
                FromReader::Sound(track) => {
                    if let Some(sound) = &mut sound {
                        sound.track = track;
                    }
                }
            }
        }
        if let Some((_, controls)) = &timeline.sound {
            controls.set_gain(gain_for(volume, muted));
        }

        let vsync = if display.refresh_hz > 1.0 {
            1.0 / display.refresh_hz
        } else {
            1.0 / 60.0
        };
        let mut new_info = None;
        let mut decode_error = None;
        let mut pull = || -> Pull {
            // The queue may ask several times in one render: once told to wait, it keeps waiting.
            if new_info.is_some() {
                return Pull::Pending;
            }
            loop {
                match frames.try_recv() {
                    Ok(FromVideo::Frame {
                        generation: g,
                        frame,
                    }) if g == generation => return Pull::Frame(frame),
                    Ok(FromVideo::End { generation: g }) if g == generation => return Pull::End,
                    // Left over from before the last seek.
                    Ok(FromVideo::Frame { .. } | FromVideo::End { .. }) => continue,
                    Ok(FromVideo::Info(i)) => {
                        // Stop here: the frames that follow must be shown under a plan made
                        // with this information, which happens before the next render.
                        new_info = Some(i);
                        return Pull::Pending;
                    }
                    Ok(FromVideo::Failed(e)) => {
                        decode_error = Some(e);
                        return Pull::End;
                    }
                    Err(TryRecvError::Empty) => return Pull::Pending,
                    Err(TryRecvError::Disconnected) => return Pull::End,
                }
            }
        };

        match gpu.render(timeline.position(), vsync, &output, &mut pull) {
            Ok((RenderStatus::Shown, s)) => {
                failures = 0;
                stats = s;
                if waiting_for_picture {
                    waiting_for_picture = false;
                    if !paused {
                        timeline.run();
                    }
                }
                if paused {
                    // The picture is not changing: no need to redraw at the display's full rate.
                    thread::sleep(Duration::from_millis(30));
                }
            }
            Ok((RenderStatus::Waiting, _)) => thread::sleep(Duration::from_millis(2)),
            Ok((RenderStatus::Ended, s)) => {
                if s.output_transfer.is_some() {
                    stats = s;
                }
                if !ended {
                    ended = true;
                    timeline.hold();
                }
                thread::sleep(Duration::from_millis(30));
            }
            Err(e) => {
                failures += 1;
                if failures > 30 {
                    result = Err(e);
                    break 'playing;
                }
                thread::sleep(Duration::from_millis(10));
            }
        }

        if let Some(i) = new_info {
            info = i;
            output = plan(&display, &info.source(), mode);
        }
        if let Some(e) = decode_error {
            result = Err(bsv_engine::Error(e));
            break 'playing;
        }

        let mut s = status.lock().unwrap();
        s.position = timeline.position();
        s.paused = paused;
        s.ended = ended;
        s.stats = stats;
        s.mode = mode;
        s.volume = volume;
        s.muted = muted;
        s.underruns = timeline
            .sound
            .as_ref()
            .map_or(0, |(clock, _)| clock.underruns());
        if s.info.as_ref() != Some(&info) {
            s.info = Some(info.clone());
        }
        if s.output.as_ref() != Some(&output) {
            s.output = Some(output.clone());
        }
        if s.sound != sound {
            s.sound = sound.clone();
        }
        if s.tracks.len() != tracks.len() {
            s.tracks = tracks.clone();
        }
        s.audio_track = audio_track;
        s.subtitle_track = subtitle_track;
    }

    shared.quit.store(true, Ordering::Relaxed);
    drop(frames);
    drop(reader_commands);
    let _ = reader_thread.join();
    let _ = video_thread.join();
    result
}
