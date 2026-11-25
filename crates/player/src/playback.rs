//! Playback: three threads behind the window.
//!
//! - the reading thread takes packets from the file, decodes sound straight into the output
//!   device's buffer and passes video packets on;
//! - the video thread decodes pictures;
//! - the render thread shows the picture that belongs to the current time.
//!
//! Time comes from the sound device (see `bsv_audio`), so the picture follows the sound. A file
//! without sound, or a machine without an output device, falls back to the system clock.
//!
//! The sound may also come from a file next to the video (a dubbing kept beside it). The
//! reading thread then reads two files: the video's as before, without its sound, and the
//! sound file as fast as the device takes sound, both from the same place after every seek.

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
use crate::sidecars::{self, Sidecar};
use bsv_engine::{
    AudioDecoder, AudioInfo, Decoded, Frame, Gpu, MediaInfo, Packet, Pull, Quality, Read,
    RenderStats, RenderStatus, Source, SubtitleDecoder, SubtitleEvent, SubtitleKind, Track,
    TrackKind, Upscaler, VideoDecoder,
};

/// Decoded frames waiting for the renderer. A 4K 10-bit frame is about 25 MB.
const FRAMES_AHEAD: usize = 8;
/// Compressed video kept ahead of the decoder, in seconds of content. It also bounds how far
/// apart a file may store a moment's picture and its sound before one of them runs dry.
const VIDEO_PACKETS_AHEAD: f64 = 1.5;
/// Longest stretch of silence inserted when a sound track starts later than the picture.
const MAX_LEADING_SILENCE: f64 = 10.0;
/// Decoded sound the reading thread may hold beyond what the device's own buffer takes, in
/// seconds. The device takes nothing until the first picture is on screen, and a decoder may
/// want a second or more of the file before it gives that picture: the sound stored alongside
/// has to go somewhere meanwhile, or the reading stops and the picture never comes.
const SOUND_WAITING: f64 = 20.0;
/// Sound from a file next to the video kept decoded beyond the device's own buffer, in seconds.
const SOUND_AHEAD: f64 = 1.0;

pub struct Options {
    pub path: String,
    pub hardware: bool,
    pub mode: HdrMode,
    /// Apply a Dolby Vision file's RPU, or show its base layer as the plain signal it also is.
    pub dolby_vision: bool,
    pub quality: Quality,
    /// The neural network that doubles pictures smaller than the window, when one is chosen.
    pub upscaler: Option<Upscaler>,
    /// The file compiled shaders are kept in between runs, if any.
    pub shader_cache: Option<std::path::PathBuf>,
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
    /// Tracks chosen by hand the last time this file was played, by stream number; for
    /// subtitles `Some(None)` is "off". They come before the languages above.
    pub audio_track: Option<i32>,
    pub subtitles: Option<Option<i32>>,
    /// The sound file to play instead of the video's own sound, by its place against the
    /// video (see [`Sidecar::place`]): the one chosen for this file before, or for the last
    /// file played from its folder.
    pub sound_place: Option<String>,
    /// The video's own sound was chosen by hand, here or for the last file of its folder: no
    /// sound file is put on in its place for the language's sake.
    pub own_sound: bool,
}

pub enum Command {
    Quit,
    Resize(u32, u32),
    /// Pixels per unit of the application's panel (Windows, composed into an application).
    Scale(f32),
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
    SetUpscaler(Option<Upscaler>),
    /// Play a Dolby Vision file as Dolby Vision, or as the plain signal its base layer is.
    SetDolbyVision(bool),
    /// Seconds to hold the picture back against the sound.
    SetAudioDelay(f64),
    /// Show the subtitles of this file, which may be anywhere: it was opened by hand.
    AddSubtitles(String),
    /// Play the sound of this file with the picture; it too was opened by hand.
    AddSound(String),
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
    /// For each sound track that is a file next to the video, its place against the video.
    /// Empty for a file opened by hand, which the next episode has no counterpart of.
    pub sound_places: Vec<(i32, String)>,
    /// 0.0 to 1.0.
    pub volume: f32,
    pub muted: bool,
    /// Gaps in the sound since the start: the device wanted samples that were not ready.
    pub underruns: u64,
    /// What drawing a picture takes on the GPU, in milliseconds: the longest of the last second.
    pub picture_gpu_ms: f32,
    /// Refreshes of the display that came and went while a picture was still being drawn,
    /// since the start: the GPU is not keeping up.
    pub late: u64,
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

/// Where the picture is drawn.
enum Target {
    /// A `CAMetalLayer` (the Mac) or a window handle (Windows).
    Surface(LayerPtr),
    /// A swapchain of the renderer's own making, for an application that composes its own
    /// window. `adopt` is called with it (an `IDXGISwapChain1*`, as an address) once it exists.
    #[cfg(windows)]
    Composition(Box<dyn FnOnce(usize) + Send>),
}

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
        Player::spawn(Target::Surface(LayerPtr(layer)), size, display, options)
    }

    /// Start playing into a swapchain that the player makes and the application shows:
    /// `adopt` receives it (an `IDXGISwapChain1*`, as an address) from the render thread.
    #[cfg(windows)]
    pub fn start_composed(
        size: (u32, u32),
        display: Display,
        options: Options,
        adopt: impl FnOnce(usize) + Send + 'static,
    ) -> Player {
        Player::spawn(Target::Composition(Box::new(adopt)), size, display, options)
    }

    fn spawn(target: Target, size: (u32, u32), display: Display, options: Options) -> Player {
        let (commands, rx) = channel();
        let status = Arc::new(Mutex::new(Status {
            mode: options.mode,
            volume: options.volume,
            muted: options.muted,
            ..Status::default()
        }));
        let shared = status.clone();
        let thread = thread::Builder::new()
            .name("render".into())
            .spawn(move || {
                if let Err(e) = render_loop(target, size, display, options, rx, &shared) {
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

    /// A sound device was opened after playback began (the video has no sound of its own,
    /// and a sound file was put on): from now on time is the device's.
    fn adopt(&mut self, clock: bsv_audio::Clock, controls: bsv_audio::Controls) {
        controls.set_playing(!self.held);
        self.sound = Some((clock, controls));
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
    /// when a seek or quit made it stale in the meantime. `meanwhile` is called every few
    /// milliseconds of the wait: the sound must not stand still because the picture does.
    fn push(
        &self,
        packet: Packet,
        generation: u64,
        shared: &Shared,
        mut meanwhile: impl FnMut(),
    ) -> bool {
        let mut state = self.state.lock().unwrap();
        while state.1 >= VIDEO_PACKETS_AHEAD {
            if shared.stale(generation) {
                return false;
            }
            drop(state);
            meanwhile();
            state = self.state.lock().unwrap();
            if state.1 < VIDEO_PACKETS_AHEAD {
                break;
            }
            state = self
                .changed
                .wait_timeout(state, Duration::from_millis(10))
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
    /// The subtitle files there are for the video (found near it, or opened by hand), in the
    /// order of their track numbers.
    Sidecars(Vec<String>),
    /// The same for sound files: one for each of their tracks.
    Sounds(Vec<OutsideSound>),
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
    /// Which track that is, after a change of track was asked for: the one asked for, or the
    /// one before it when that could not be opened.
    Playing(Option<i32>),
    /// A sound device was opened after playback began.
    Device(SoundSetup),
    /// A font that came with a subtitle file.
    Font { name: String, data: Vec<u8> },
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
    /// Samples of silence owed to the device before anything in `waiting`: the stretch
    /// between where playback starts and where a track that begins later does.
    silence: usize,
    /// Decoded sound the device's buffer had no room for yet, oldest first.
    waiting: VecDeque<f32>,
    /// The file has no more sound: the device is told once `waiting` has gone through.
    ending: bool,
    /// The longest silence put before a track that begins after the picture, in seconds.
    most_silence: f64,
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
            silence: 0,
            waiting: VecDeque::new(),
            ending: false,
            most_silence: MAX_LEADING_SILENCE,
        })
    }

    /// Hand the device whatever it has room for, and tell it when the last of the sound is in.
    fn settle(&mut self) {
        settle(&mut self.output, &mut self.silence, &mut self.waiting);
        if self.ending && self.silence == 0 && self.waiting.is_empty() {
            self.output.set_ended();
            self.ending = false;
        }
    }

    /// Forget everything not yet played: the step before sound from a new position.
    fn flush(&mut self) {
        self.decoder.flush();
        self.output.flush();
        self.silence = 0;
        self.waiting.clear();
        self.ending = false;
        self.based = false;
    }

    /// The file has no more sound. If none came at all for where playback starts (the track
    /// is shorter than the picture), time starts there all the same and runs over silence.
    fn finish(&mut self, start: f64) {
        if !self.based {
            self.output.set_base(start);
            self.based = true;
        }
        self.ending = true;
        self.settle();
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
                    let gap = (samples.pts - start).min(self.most_silence);
                    self.silence = (gap * rate) as usize * channels;
                    self.output.set_base(start);
                    self.based = true;
                }
                if !self.based {
                    self.output.set_base(start.max(samples.pts));
                    self.based = true;
                }
            }
            if !queue(
                &mut self.output,
                &mut self.silence,
                &mut self.waiting,
                data,
                generation,
                shared,
            ) {
                return Ok(());
            }
        }
    }
}

/// Move waiting sound into the device's buffer, as much as it has room for: first the
/// silence owed before it.
fn settle(output: &mut AudioOutput, silence: &mut usize, waiting: &mut VecDeque<f32>) {
    const QUIET: [f32; 4096] = [0.0; 4096];
    while *silence > 0 {
        let taken = output.write(&QUIET[..QUIET.len().min(*silence)]);
        if taken == 0 {
            return;
        }
        *silence -= taken;
    }
    while !waiting.is_empty() {
        let taken = output.write(waiting.make_contiguous());
        if taken == 0 {
            return;
        }
        waiting.drain(..taken);
    }
}

/// Add samples behind those already waiting for the device. Waits only when more than
/// [`SOUND_WAITING`] seconds are held; returns `false` if a seek or quit interrupted that.
fn queue(
    output: &mut AudioOutput,
    silence: &mut usize,
    waiting: &mut VecDeque<f32>,
    data: &[f32],
    generation: u64,
    shared: &Shared,
) -> bool {
    let format = output.format();
    let limit = (SOUND_WAITING * format.sample_rate as f64) as usize * format.channels as usize;
    waiting.extend(data);
    loop {
        settle(output, silence, waiting);
        if waiting.len() <= limit {
            return true;
        }
        if shared.stale(generation) {
            return false;
        }
        thread::sleep(Duration::from_millis(2));
    }
}

/// Track numbers from here up stand for subtitle files next to the video, not streams inside it.
const SIDECAR_BASE: i32 = 1000;
/// And from here up, for the sound tracks of sound files next to it.
const SOUND_BASE: i32 = 2000;

/// One sound track of a file next to the video.
#[derive(Clone)]
struct OutsideSound {
    path: String,
    /// The track's stream number in that file.
    stream: i32,
    /// See [`Sidecar::place`]; empty for a file opened by hand.
    place: String,
}

/// The sound tracks in a sound file, as the tracks they are listed as: numbered from
/// `SOUND_BASE + first`. Empty when the file cannot be read or has no sound this build plays.
fn outside_tracks(file: &Sidecar, first: usize) -> Vec<(Track, OutsideSound)> {
    let source = match Source::open_sound(&file.path) {
        Ok(source) => source,
        Err(e) => {
            eprintln!("[warn] {}: {e}", file.path);
            return Vec::new();
        }
    };
    let streams: Vec<Track> = source
        .tracks()
        .into_iter()
        .filter(|t| t.kind == TrackKind::Audio)
        .collect();
    let several = streams.len() > 1;
    streams
        .into_iter()
        .enumerate()
        .map(|(n, mut track)| {
            let stream = track.index;
            track.index = SOUND_BASE + (first + n) as i32;
            // What the file says of itself comes before what its name and folders suggest.
            if track.language.is_empty() || track.language == "und" {
                track.language = file.language.clone();
            }
            // Where the file is says whose sound it is; its tracks' own names are needed
            // only to tell several of them apart.
            track.title = match (several, track.title.is_empty()) {
                (true, false) => format!("{}, {}", file.title, track.title),
                (true, true) => format!("{}, {}", file.title, n + 1),
                (false, _) => file.title.clone(),
            };
            track.is_default = false;
            let place = match (file.place.is_empty(), several) {
                (true, _) => String::new(),
                (false, true) => format!("{}#{n}", file.place),
                (false, false) => file.place.clone(),
            };
            let outside = OutsideSound {
                path: file.path.clone(),
                stream,
                place,
            };
            (track, outside)
        })
        .collect()
}

/// Which of the sound files found next to a video is put on without being asked for, if any.
/// `own` are the video's own tracks.
///
/// In order: the file chosen before, for this video or for the last one played from its
/// folder (the same dubbing of the next episode). Then none, if the video's own sound was
/// chosen by hand. Then a file in the language asked for, when the video has no track in it;
/// or any file at all, when the video has no sound.
fn sound_file_wanted(
    options: &Options,
    own: &[Track],
    found: &[(Track, OutsideSound)],
) -> Option<usize> {
    let in_language = |t: &Track| language::same(&t.language, &options.audio_language);
    let first_in_language = || found.iter().position(|(track, _)| in_language(track));
    let mut own = own.iter().filter(|t| t.kind == TrackKind::Audio).peekable();
    options
        .sound_place
        .as_deref()
        .filter(|place| !place.is_empty())
        .and_then(|place| found.iter().position(|(_, file)| file.place == place))
        .or_else(|| {
            if options.audio_track.is_some() || options.own_sound {
                None
            } else if own.peek().is_none() {
                first_in_language().or((!found.is_empty()).then_some(0))
            } else if own.any(in_language) {
                None
            } else {
                first_in_language()
            }
        })
}

/// The sound file being read in place of the video's own sound.
struct Outside {
    file: OutsideSound,
    source: Source,
    /// Its track number.
    track: i32,
    /// Nothing more to read from it, until the next seek.
    ended: bool,
}

impl Outside {
    fn open(file: &OutsideSound, track: i32) -> Result<Outside, String> {
        let mut source = Source::open_sound(&file.path).map_err(|e| e.to_string())?;
        source
            .select_audio(file.stream)
            .map_err(|e| e.to_string())?;
        Ok(Outside {
            file: file.clone(),
            source,
            track,
            ended: false,
        })
    }

    /// Go to `to`. A file that cannot be searched is read again from its beginning, and what
    /// comes before `to` is dropped on the way.
    fn seek(&mut self, to: f64) {
        self.ended = false;
        if self.source.seek(to).is_ok() {
            return;
        }
        match Outside::open(&self.file, self.track) {
            Ok(again) => self.source = again.source,
            Err(e) => {
                eprintln!("[warn] the sound file cannot be read from there: {e}");
                self.ended = true;
            }
        }
    }
}

/// Play the sound track `from` has selected: through the device that is open, or through one
/// opened now (the video had no sound of its own).
fn take_sound(
    sound: &mut Option<SoundPath>,
    from: &Source,
    news: &Sender<FromReader>,
) -> Result<(), String> {
    let track = from.audio_info().ok_or("the file has no sound")?;
    match sound {
        Some(path) => {
            let format = path.output.format().clone();
            path.decoder = AudioDecoder::open(from, format.sample_rate, format.channels)
                .map_err(|e| e.to_string())?;
        }
        None => {
            let path = SoundPath::open(from, &track)?;
            let _ = news.send(FromReader::Device(SoundSetup {
                clock: path.output.clock(),
                controls: path.output.controls(),
                sound: Sound {
                    track: track.clone(),
                    device: path.output.format().clone(),
                },
            }));
            *sound = Some(path);
        }
    }
    let _ = news.send(FromReader::Sound(track));
    Ok(())
}

/// Keep the device in sound: move on what is waiting for it, and read more of the sound
/// file next to the video when there is one. That file is read as fast as its sound is
/// played, whatever the video's packets are doing: the picture follows the sound's clock, so
/// the sound may never wait for the picture.
fn tend(
    sound: &mut Option<SoundPath>,
    outside: &mut Option<Outside>,
    start: f64,
    generation: u64,
    shared: &Shared,
) {
    let Some(path) = sound else {
        return;
    };
    path.settle();
    let Some(file) = outside.as_mut().filter(|file| !file.ended) else {
        return;
    };
    let format = path.output.format();
    let ahead = (SOUND_AHEAD * format.sample_rate as f64) as usize * format.channels as usize;
    let began = Instant::now();
    while path.silence + path.waiting.len() < ahead && !shared.stale(generation) {
        // A little at a time, so the video's packets are not kept waiting; more while the
        // sound has yet to reach the place playback starts at, since nothing moves until then.
        let enough = Duration::from_millis(if path.based { 4 } else { 30 });
        if began.elapsed() > enough {
            return;
        }
        let over = match file.source.read() {
            Ok(Read::Audio(packet)) => path
                .decoder
                .send(Some(packet))
                .map_err(|e| e.to_string())
                .and_then(|()| path.pump(start, generation, shared))
                .err(),
            Ok(Read::End) => {
                let _ = path.decoder.send(None);
                let _ = path.pump(start, generation, shared);
                Some(String::new())
            }
            Ok(_) => None,
            Err(e) => Some(e.to_string()),
        };
        if let Some(why) = over {
            if !why.is_empty() {
                eprintln!("[warn] the sound file cannot be read further: {why}");
            }
            file.ended = true;
            path.finish(start);
            return;
        }
    }
}

/// The track a subtitle file is listed as.
fn sidecar_track(number: usize, file: &Sidecar) -> Track {
    Track {
        index: SIDECAR_BASE + number as i32,
        kind: TrackKind::Subtitle,
        codec: String::new(),
        profile: String::new(),
        language: file.language.clone(),
        title: file.title.clone(),
        layout: String::new(),
        channels: 0,
        sample_rate: 0,
        is_default: false,
        forced: false,
        hearing_impaired: false,
        bitmap: false,
    }
}

/// Send the fonts that come with a subtitle file: styled subtitles are drawn with them.
fn send_fonts(subtitles: &str, video: &str, news: &Sender<FromReader>) {
    let subtitles = std::path::Path::new(subtitles);
    // Up to the video's folder for a file found below it; its own folder for any other.
    let top = std::path::Path::new(video)
        .parent()
        .filter(|folder| subtitles.starts_with(folder));
    for font in sidecars::fonts_for(subtitles, top) {
        let name = font.file_name().map(|n| n.to_string_lossy().into_owned());
        if let (Some(name), Ok(data)) = (name, std::fs::read(&font)) {
            let _ = news.send(FromReader::Font { name, data });
        }
    }
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
    /// The video's path: subtitle files below its folder bring their fonts from there.
    path: String,
}

fn read_loop(mut source: Source, reading: Reading) {
    let Reading {
        sound_wanted,
        setup,
        commands,
        video,
        news,
        shared,
        path: video_path,
    } = reading;
    // Subtitle files next to the video, in the order of their track numbers (they are found
    // a moment after playback starts), and the one of them being shown.
    let mut sidecars: Vec<String> = Vec::new();
    let mut sidecar: Option<String> = None;
    // The subtitle files whose fonts have been sent.
    let mut with_fonts: Vec<String> = Vec::new();
    // The sound tracks in files next to the video, likewise, and the one of them playing.
    let mut sounds: Vec<OutsideSound> = Vec::new();
    let mut outside: Option<Outside> = None;
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
        // Sound held back while the device was full moves on with every packet read.
        tend(&mut sound, &mut outside, start, generation, &shared);
        let command = if idle {
            // Nothing left to read of the video, but the end of the sound may still be
            // waiting, and a sound file goes on being read.
            let wait = if outside.is_some() { 10 } else { 50 };
            match commands.recv_timeout(Duration::from_millis(wait)) {
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
        if let Some(ReaderCommand::Sounds(found)) = command {
            sounds = found;
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
            if let Some(index) = audio.filter(|_| sound_wanted) {
                // A track of a file next to the video, or a stream inside it.
                let switched = if index >= SOUND_BASE {
                    sounds
                        .get((index - SOUND_BASE) as usize)
                        .ok_or_else(|| "there is no such sound file".to_string())
                        .and_then(|file| Outside::open(file, index))
                        .and_then(|file| {
                            take_sound(&mut sound, &file.source, &news)?;
                            // The video's own sound is no longer read.
                            source.no_audio();
                            outside = Some(file);
                            Ok(())
                        })
                } else if sound.is_some() {
                    let before = source.audio_index();
                    source
                        .select_audio(index)
                        .map_err(|e| e.to_string())
                        .and_then(|()| take_sound(&mut sound, &source, &news))
                        .map(|()| outside = None)
                        .inspect_err(|_| {
                            if outside.is_some() {
                                source.no_audio();
                            } else if let Some(before) = before {
                                let _ = source.select_audio(before);
                            }
                        })
                } else {
                    Err("there is no sound device".to_string())
                };
                if let Err(e) = switched {
                    eprintln!("[warn] cannot switch the sound track: {e}");
                }
                if let Some(path) = &mut sound {
                    // A track that begins late is waited for in full when it is a file of
                    // its own: nothing in the video's packets would cut the wait short.
                    path.most_silence = if outside.is_some() {
                        f64::MAX
                    } else {
                        MAX_LEADING_SILENCE
                    };
                }
                let playing = match &outside {
                    Some(file) => Some(file.track),
                    None => source.audio_index().filter(|_| sound.is_some()),
                };
                let _ = news.send(FromReader::Playing(playing));
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
            } else if sidecar.is_none() {
                // The same track as before, said again: the renderer listens only to what is
                // said since the latest seek, and the first seek of all (to where a film is
                // picked up) comes before it has heard what the track is.
                let _ = news.send(FromReader::Subtitles {
                    generation,
                    kind: subtitles.as_ref().map(SubtitleDecoder::kind),
                });
            }
            if let Some(path) = &sidecar {
                if !with_fonts.contains(path) {
                    with_fonts.push(path.clone());
                    send_fonts(path, &video_path, &news);
                }
                send_sidecar(path, generation, &news);
            }
            let moved = source.seek(to);
            if let Some(file) = &mut outside {
                file.seek(to);
            }
            // Sound is silenced first: once the video thread sees the flush, the device's clock
            // no longer reports the old position.
            if let Some(path) = &mut sound {
                path.flush();
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
                video.push(packet, generation, &shared, || {
                    tend(&mut sound, &mut outside, start, generation, &shared)
                });
                None
            }
            // The video's own sound, unless a file next to it is playing instead.
            Ok(Read::Audio(packet)) => match sound.as_mut().filter(|_| outside.is_none()) {
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
                if let Some(path) = sound.as_mut().filter(|_| outside.is_none()) {
                    let _ = path.decoder.send(None);
                    let _ = path.pump(start, generation, &shared);
                    path.finish(start);
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
    let mut on_hardware = false;

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
                    // Again if the hardware has since handed the stream over to software.
                    if !info_sent || decoder.is_hardware() != on_hardware {
                        info_sent = true;
                        on_hardware = decoder.is_hardware();
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
    target: Target,
    size: (u32, u32),
    mut display: Display,
    options: Options,
    commands: Receiver<Command>,
    status: &Mutex<Status>,
) -> Result<(), bsv_engine::Error> {
    let mut gpu = match target {
        // SAFETY: `Player::start`'s caller keeps the layer (or window) alive until this thread is joined.
        #[cfg(target_os = "macos")]
        Target::Surface(layer) => unsafe { Gpu::for_metal_layer(layer.0) }?,
        #[cfg(windows)]
        Target::Surface(window) => unsafe { Gpu::for_window(window.0) }?,
        #[cfg(windows)]
        Target::Composition(adopt) => {
            let (gpu, swapchain) = Gpu::for_composition(size.0, size.1)?;
            adopt(swapchain as usize);
            gpu
        }
    };
    // Shaders compiled before are not compiled again: the upscaler's take seconds on some
    // systems, and without this they would at the start of every file.
    if let Some(data) = options
        .shader_cache
        .as_ref()
        .and_then(|file| std::fs::read(file).ok())
    {
        gpu.recall_compiled(&data);
    }
    let compiled_before = gpu.compiled_mark();
    gpu.set_options(options.quality, &options.renderer_options)?;
    if let Err(e) = gpu.set_upscaler(options.upscaler) {
        eprintln!("[warn] {e}");
    }
    gpu.set_dolby_vision(options.dolby_vision);
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
    // Subtitle and sound files next to the video are looked for on the side: listing a
    // folder can wait on the system asking the user for access to it, and the film should not
    // wait with it. The sound files are also opened there, to learn what tracks they hold.
    let (found_tx, found_sidecars) = channel();
    let (sounds_tx, found_sounds) = channel();
    {
        let path = options.path.clone();
        let sound = options.sound;
        thread::Builder::new()
            .name("files beside".into())
            .spawn(move || {
                let _ = found_tx.send(sidecars::find(&path));
                let mut tracks: Vec<(Track, OutsideSound)> = Vec::new();
                if sound {
                    for file in sidecars::find_sound(&path) {
                        tracks.extend(outside_tracks(&file, tracks.len()));
                    }
                }
                let _ = sounds_tx.send(tracks);
            })
            .expect("spawn the search for files beside the video");
    }
    // Sound: the track chosen for this file before; else in the language asked for, when the
    // file has it; otherwise as the file prefers.
    let has =
        |kind: TrackKind, index: i32| tracks.iter().any(|t| t.kind == kind && t.index == index);
    if let Some(index) = options.audio_track.filter(|&i| has(TrackKind::Audio, i)) {
        let _ = source.select_audio(index);
    } else if let Some(track) = tracks.iter().find(|t| {
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
    let by_the_rules = || {
        subtitle(&|t| in_language(t) && !t.forced && !t.hearing_impaired)
            .or_else(|| subtitle(&|t| in_language(t)))
            .or_else(|| subtitle(&|t| t.forced))
    };
    // What was chosen for this file before comes first: a track inside it, a file next to it
    // (put on when the search for those comes back), or no subtitles at all.
    let mut subtitle_track = match options.subtitles {
        Some(None) => None,
        Some(Some(index)) if index >= SIDECAR_BASE => None,
        Some(Some(index)) if has(TrackKind::Subtitle, index) => Some(index),
        _ => by_the_rules(),
    };
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
        let path = options.path.clone();
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
                        path,
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
    // How the GPU is keeping up: see `Status::picture_gpu_ms` and `Status::late`.
    let mut gpu_peak = (Instant::now(), 0f32, 0f32);
    let mut late = 0u64;
    let mut failures = 0u32;
    // The time the last picture was drawn for.
    let mut drawn_for = f64::MIN;
    // When the sound belonging to the last picture is over.
    let mut over_at: Option<Instant> = None;
    let mut result = Ok(());
    // Commands the loop gives itself: the starting position, a subtitle file that turned up.
    let mut pending: VecDeque<Command> = VecDeque::new();
    if options.start > 0.0 {
        pending.push_back(Command::SeekBy(options.start));
    }
    let mut sidecars_known = false;
    // Every subtitle file there is a track for, and those opened by hand before the search
    // for the ones near the video came back (their tracks are numbered after the found ones).
    let mut sidecar_files: Vec<String> = Vec::new();
    let mut opened_early: Vec<String> = Vec::new();
    // The same for sound files: one entry for each of their tracks.
    let mut sounds_known = false;
    let mut sound_files: Vec<OutsideSound> = Vec::new();
    let mut sounds_opened_early: Vec<String> = Vec::new();
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
                #[cfg(windows)]
                Ok(Command::Scale(scale)) => gpu.set_scale(scale),
                #[cfg(not(windows))]
                Ok(Command::Scale(_)) => {}
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
                    // A sound file can be put on even when the video has no sound of its
                    // own, and so no device has been opened yet.
                    let possible = options.sound && (sound.is_some() || index >= SOUND_BASE);
                    if known && possible && audio_track != Some(index) {
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
                Ok(Command::SetUpscaler(upscaler)) => {
                    if let Err(e) = gpu.set_upscaler(upscaler) {
                        eprintln!("[warn] {e}");
                    }
                }
                Ok(Command::SetAudioDelay(seconds)) => timeline.audio_delay = seconds,
                Ok(Command::AddSubtitles(file)) if !sidecars_known => opened_early.push(file),
                Ok(Command::AddSubtitles(file)) => {
                    let number = match sidecar_files.iter().position(|known| *known == file) {
                        Some(number) => number,
                        None => {
                            let title = std::path::Path::new(&file)
                                .file_name()
                                .map(|n| n.to_string_lossy().into_owned())
                                .unwrap_or_default();
                            let opened = Sidecar {
                                path: file.clone(),
                                title,
                                ..Sidecar::default()
                            };
                            tracks.push(sidecar_track(sidecar_files.len(), &opened));
                            sidecar_files.push(file);
                            let _ = reader_commands
                                .send(ReaderCommand::Sidecars(sidecar_files.clone()));
                            sidecar_files.len() - 1
                        }
                    };
                    pending.push_back(Command::SelectSubtitle(Some(SIDECAR_BASE + number as i32)));
                }
                Ok(Command::AddSound(file)) if !sounds_known => sounds_opened_early.push(file),
                Ok(Command::AddSound(file)) => {
                    let first = match sound_files.iter().position(|known| known.path == file) {
                        Some(number) => Some(number),
                        None => {
                            let title = std::path::Path::new(&file)
                                .file_name()
                                .map(|n| n.to_string_lossy().into_owned())
                                .unwrap_or_default();
                            let opened = Sidecar {
                                path: file,
                                title,
                                ..Sidecar::default()
                            };
                            let number = sound_files.len();
                            let added = outside_tracks(&opened, number);
                            let any = !added.is_empty();
                            for (track, file) in added {
                                tracks.push(track);
                                sound_files.push(file);
                            }
                            let _ =
                                reader_commands.send(ReaderCommand::Sounds(sound_files.clone()));
                            any.then_some(number)
                        }
                    };
                    if let Some(number) = first {
                        pending.push_back(Command::SelectAudio(SOUND_BASE + number as i32));
                    }
                }
                Ok(Command::SetDolbyVision(apply)) => {
                    gpu.set_dolby_vision(apply);
                    // Frames already decoded were taken the other way: decode again from here.
                    if info
                        .dolby_vision
                        .is_some_and(|dv| dv.base_layer().is_some())
                    {
                        jump = Some((timeline.position(), None, None));
                    }
                }
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
                drawn_for = f64::MIN;
                over_at = None;
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
            for (n, file) in found.iter().enumerate() {
                tracks.push(sidecar_track(n, file));
                sidecar_files.push(file.path.clone());
            }
            let _ = reader_commands.send(ReaderCommand::Sidecars(sidecar_files.clone()));
            // A file someone put next to the video is shown without being asked for, unless a
            // track in the preferred language is already on and this file is not in it.
            let wanted = match options.subtitles {
                // The file chosen for this video before, if it is still there.
                Some(Some(index)) if index >= SIDECAR_BASE => {
                    Some((index - SIDECAR_BASE) as usize).filter(|&n| n < found.len())
                }
                // Any other choice made by hand stands.
                Some(_) => None,
                None => found
                    .iter()
                    .position(|f| language::same(&f.language, &options.subtitle_language))
                    .or((!chosen_by_language && !found.is_empty()).then_some(0)),
            };
            if let Some(n) = wanted {
                pending.push_back(Command::SelectSubtitle(Some(SIDECAR_BASE + n as i32)));
            }
            // What was opened by hand meanwhile comes after, and so is what ends up showing.
            pending.extend(opened_early.drain(..).map(Command::AddSubtitles));
        }
        // The search for sound files next to the video has come back.
        if !sounds_known && let Ok(found) = found_sounds.try_recv() {
            sounds_known = true;
            let wanted = sound_file_wanted(&options, &tracks, &found);
            for (track, file) in found {
                tracks.push(track);
                sound_files.push(file);
            }
            let _ = reader_commands.send(ReaderCommand::Sounds(sound_files.clone()));
            if let Some(n) = wanted {
                pending.push_back(Command::SelectAudio(SOUND_BASE + n as i32));
            }
            pending.extend(sounds_opened_early.drain(..).map(Command::AddSound));
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
                FromReader::Font { name, data } => gpu.add_font(&name, &data),
                FromReader::Sound(track) => {
                    if let Some(sound) = &mut sound {
                        sound.track = track;
                    }
                }
                FromReader::Playing(track) => audio_track = track,
                FromReader::Device(setup) => {
                    timeline.adopt(setup.clock, setup.controls);
                    sound = Some(setup.sound);
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

        // A picture drawn now is seen once those in line before it have had their refresh
        // each (measured on the Mac at 120 Hz: 20 to 30 ms, with three in line), so the one
        // drawn is the one that belongs to that moment: it then meets its sound. Never an
        // earlier time than the last one, which the queue of frames could not go back to.
        let ahead = gpu.frames_in_line() as f64 * vsync;
        let moment = (timeline.position() + ahead).max(drawn_for);
        let asked = Instant::now();
        match gpu.render(moment, vsync, &output, &mut pull) {
            Ok((RenderStatus::Shown, s)) => {
                failures = 0;
                stats = s;
                // The call returns at the display's next refresh. One that took the time of
                // two had a picture still on the GPU when its refresh came.
                if !paused && !waiting_for_picture {
                    let refreshes = asked.elapsed().as_secs_f64() / vsync;
                    if refreshes > 1.7 {
                        late += (refreshes - 0.7) as u64;
                    }
                }
                if s.new_picture {
                    gpu_peak.2 = gpu_peak.2.max(s.gpu_ms);
                }
                if gpu_peak.0.elapsed() >= Duration::from_secs(1) {
                    gpu_peak = (Instant::now(), gpu_peak.2, 0.0);
                }
                drawn_for = moment;
                over_at = None;
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
                // The time asked for runs ahead of the sound: the sound has that much left.
                let over = *over_at.get_or_insert(Instant::now() + Duration::from_secs_f64(ahead));
                if !ended && Instant::now() >= over {
                    ended = true;
                    timeline.hold();
                }
                thread::sleep(Duration::from_millis(if ended { 30 } else { 2 }));
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
        s.picture_gpu_ms = gpu_peak.1;
        s.late = late;
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
            s.sound_places = sound_files
                .iter()
                .enumerate()
                .map(|(n, file)| (SOUND_BASE + n as i32, file.place.clone()))
                .collect();
        }
        s.audio_track = audio_track;
        s.subtitle_track = subtitle_track;
    }

    if let Some(file) = &options.shader_cache
        && gpu.compiled_mark() != compiled_before
    {
        if let Some(folder) = file.parent() {
            let _ = std::fs::create_dir_all(folder);
        }
        if let Err(e) = std::fs::write(file, gpu.compiled()) {
            eprintln!(
                "[warn] cannot keep the compiled shaders in {}: {e}",
                file.display()
            );
        }
    }

    shared.quit.store(true, Ordering::Relaxed);
    drop(frames);
    drop(reader_commands);
    let _ = reader_thread.join();
    let _ = video_thread.join();
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options(language: &str) -> Options {
        Options {
            path: String::new(),
            hardware: false,
            mode: HdrMode::default(),
            dolby_vision: true,
            quality: Quality::High,
            upscaler: None,
            shader_cache: None,
            renderer_options: String::new(),
            start: 0.0,
            sound: true,
            audio_delay: 0.0,
            volume: 1.0,
            muted: false,
            subtitle_lift: 0.0,
            subtitle_scale: 1.0,
            subtitle_nits: 203.0,
            audio_language: language.into(),
            subtitle_language: String::new(),
            audio_track: None,
            subtitles: None,
            sound_place: None,
            own_sound: false,
        }
    }

    fn track(index: i32, language: &str) -> Track {
        Track {
            index,
            kind: TrackKind::Audio,
            codec: String::new(),
            profile: String::new(),
            language: language.into(),
            title: String::new(),
            layout: String::new(),
            channels: 2,
            sample_rate: 48000,
            is_default: false,
            forced: false,
            hearing_impaired: false,
            bitmap: false,
        }
    }

    fn beside(files: &[(&str, &str)]) -> Vec<(Track, OutsideSound)> {
        files
            .iter()
            .enumerate()
            .map(|(n, (place, language))| {
                let file = OutsideSound {
                    path: String::new(),
                    stream: 0,
                    place: place.to_string(),
                };
                (track(SOUND_BASE + n as i32, language), file)
            })
            .collect()
    }

    #[test]
    fn a_sound_file_is_put_on_for_the_language_or_for_want_of_any_sound() {
        let found = beside(&[("ENG Sound|.mka", "eng"), ("RUS Sound/A|.mka", "rus")]);
        let japanese = [track(1, "jpn")];
        // No language asked for: the video's own sound stays.
        assert_eq!(sound_file_wanted(&options(""), &japanese, &found), None);
        // Russian asked for, and the video has none of its own.
        assert_eq!(
            sound_file_wanted(&options("rus"), &japanese, &found),
            Some(1)
        );
        // The video has it: its own track is the one to play.
        let both = [track(1, "jpn"), track(2, "ru")];
        assert_eq!(sound_file_wanted(&options("rus"), &both, &found), None);
        // A language no file is in.
        assert_eq!(sound_file_wanted(&options("deu"), &japanese, &found), None);
        // A video without sound takes the file in the language, or else the first.
        assert_eq!(sound_file_wanted(&options("rus"), &[], &found), Some(1));
        assert_eq!(sound_file_wanted(&options(""), &[], &found), Some(0));
        assert_eq!(sound_file_wanted(&options(""), &[], &[]), None);
    }

    #[test]
    fn what_was_chosen_by_hand_comes_before_the_language() {
        let found = beside(&[("RUS Sound/A|.mka", "rus"), ("RUS Sound/B|.mka", "rus")]);
        let japanese = [track(1, "jpn")];
        // The dubbing chosen for the episode before.
        let chosen = Options {
            sound_place: Some("RUS Sound/B|.mka".into()),
            ..options("rus")
        };
        assert_eq!(sound_file_wanted(&chosen, &japanese, &found), Some(1));
        // This episode does not have it: back to the rule.
        let missing = Options {
            sound_place: Some("RUS Sound/C|.mka".into()),
            ..options("rus")
        };
        assert_eq!(sound_file_wanted(&missing, &japanese, &found), Some(0));
        // The video's own sound was chosen by hand, here or in the episode before.
        let own = Options {
            own_sound: true,
            ..options("rus")
        };
        assert_eq!(sound_file_wanted(&own, &japanese, &found), None);
        let this_track = Options {
            audio_track: Some(1),
            ..options("rus")
        };
        assert_eq!(sound_file_wanted(&this_track, &japanese, &found), None);
    }
}
