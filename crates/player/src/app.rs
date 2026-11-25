//! The window. It owns the video layer and the native interface above it, starts a playback
//! session for each file, and carries messages between the two.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use bsv_display::policy::HdrMode;
use bsv_display::{Display, HdrStatus, Luminance, LuminanceSource, OsReport};
use bsv_engine::{Quality, Transfer};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalSize};
use winit::event::WindowEvent;
#[cfg(not(target_os = "macos"))]
use winit::event::{ElementState, MouseButton};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
#[cfg(not(target_os = "macos"))]
use winit::keyboard::{Key, ModifiersState, NamedKey};
#[cfg(target_os = "macos")]
use winit::platform::macos::{MonitorHandleExtMacOS, WindowAttributesExtMacOS};
#[cfg(windows)]
use winit::platform::windows::MonitorHandleExtWindows;
use winit::window::{Fullscreen, Theme, Window, WindowId};

use crate::history::{Memory, ResumeNote};
use crate::launch::Settings;

use crate::playback::{self, Command, Options, Picture, Player};
use crate::prefs::{self, Preferences};
use crate::queue::{Arrived, Queue};
use crate::report;
use crate::sidecars;
use crate::steps;
use crate::surface::Surface;
use crate::ui::{self, Interface, PictureChange, Playing, Request, State, View};

/// How often the display is asked again whether HDR is on (it can be switched while we play).
const DISPLAY_POLL: Duration = Duration::from_millis(1000);
/// How often the interface is told the player's state.
const INTERFACE_REFRESH: Duration = Duration::from_millis(100);

/// One file being played.
struct Session {
    player: Player,
    file_name: String,
    /// The whole path of the file.
    path: String,
    /// What was last printed, so each line appears only when it changes.
    printed_media: String,
    printed_chapters: bool,
    printed_sound: String,
    printed_path: String,
    printed_layer: String,
    printed_advice: Vec<String>,
    sized_to_video: bool,
    /// Whether the layer is currently marked as carrying HDR.
    layer_hdr: Option<bool>,
    /// Playback position against the system clock when playing began, to show drift from it.
    clock_origin: Option<(Instant, f64, f64)>,
    underruns: u64,
    /// Shown for a while when the file was picked up where it was left.
    resume_note: Option<ResumeNote>,
}

struct Running {
    // Field order is drop order: playback stops before the interface, the layer and the window go.
    session: Option<Session>,
    interface: Interface,
    surface: Surface,
    window: Window,
    display: Display,
    /// Why the last file could not be played, shown on the empty screen.
    error: Option<String>,
    /// The interface's controls are showing (subtitles move up out of their way).
    controls_visible: bool,
    /// Preferences as saved, and as in force for this run (with the command line's on top).
    saved: Preferences,
    prefs: Preferences,
    preferences_path: PathBuf,
    /// Where each file was stopped, and the tracks chosen for it.
    memory: Memory,
    /// What plays after what: the folder of the file that was opened, or a playlist.
    queue: Queue,
    /// Subtitle files opened together with a video: shown once the video starts.
    subtitles_to_add: Vec<String>,
    /// Sound files opened along with a video, to be played with it.
    sounds_to_add: Vec<String>,
    // Carried from one file to the next.
    volume: f32,
    muted: bool,
    mode: HdrMode,
    quality: Quality,
    hardware: bool,
    audio_delay: f64,
    /// How fast files play: kept from one to the next until the player is closed.
    speed: f64,
    /// How the picture is placed: kept the same way.
    picture: Picture,
    /// Files played lately, read from the history when nothing is open; forgotten when a
    /// file opens, as that changes them.
    recent: Option<Vec<crate::history::Recent>>,
    /// The sound outputs there are and the system's own, looked at again when the settings
    /// are opened.
    devices: Option<(Vec<crate::SoundDevice>, Option<String>)>,
}

/// How many files played lately the empty window lists.
const RECENT_SHOWN: usize = 8;

struct App {
    settings: Settings,
    running: Option<Running>,
    started: Instant,
    last_display_poll: Instant,
    last_clock_report: Instant,
    failure: Option<String>,
    // Without a drawn interface the window reads the keyboard and the mouse itself.
    #[cfg(not(target_os = "macos"))]
    modifiers: ModifiersState,
    #[cfg(not(target_os = "macos"))]
    last_click: Option<Instant>,
}

pub fn run(settings: Settings) -> Result<(), String> {
    ui::prepare();
    let event_loop = EventLoop::new().map_err(|e| e.to_string())?;
    let proxy = event_loop.create_proxy();
    let mut app = App {
        settings,
        running: None,
        started: Instant::now(),
        last_display_poll: Instant::now(),
        last_clock_report: Instant::now(),
        failure: None,
        #[cfg(not(target_os = "macos"))]
        modifiers: ModifiersState::empty(),
        #[cfg(not(target_os = "macos"))]
        last_click: None,
    };
    // The interface wakes the loop when it has a command, so a click is acted on at once.
    WAKE.with(|wake| *wake.borrow_mut() = Some(proxy));
    event_loop.run_app(&mut app).map_err(|e| e.to_string())?;
    if let Some(r) = &mut app.running {
        // Where the film was left.
        r.memory.closed();
        // The volume is remembered as it was left.
        if (r.saved.volume - r.volume).abs() > 0.001 {
            r.saved.volume = r.volume;
            r.save_preferences();
        }
    }
    match app.failure {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

thread_local! {
    static WAKE: std::cell::RefCell<Option<winit::event_loop::EventLoopProxy<()>>> =
        const { std::cell::RefCell::new(None) };
}

// ---- The display ------------------------------------------------------------------------------

/// Extended-range headroom the system is giving right now, 1.0 when there is none.
fn headroom_in_use(display: &Display) -> f32 {
    match display.os {
        OsReport::MacOs { edr_now, .. } => edr_now,
        _ => 1.0,
    }
}

/// The display with its moment-to-moment readings blanked, for telling real changes apart.
fn without_live_state(display: &Display) -> Display {
    let mut d = display.clone();
    if let OsReport::MacOs { edr_now, .. } = &mut d.os {
        *edr_now = 1.0;
    }
    d
}

/// The display the window is on, falling back to the main one, and to a plain SDR display
/// when the system will not describe any (a remote session, a machine without a monitor).
fn display_of(window: &Window) -> Display {
    #[cfg(target_os = "macos")]
    let id = window.current_monitor().map(|m| m.native_id() as u64);
    #[cfg(windows)]
    let id = window.current_monitor().map(|m| m.hmonitor() as u64);
    let all = bsv_display::displays().unwrap_or_default();
    all.iter()
        .find(|d| Some(d.id) == id)
        .cloned()
        .or_else(|| all.into_iter().next())
        .unwrap_or_else(|| {
            let size = window
                .current_monitor()
                .map(|m| m.size())
                .unwrap_or_default();
            let refresh = window
                .current_monitor()
                .and_then(|m| m.refresh_rate_millihertz());
            Display {
                id: id.unwrap_or(0),
                name: "Display".into(),
                builtin: false,
                connection: None,
                width: size.width,
                height: size.height,
                refresh_hz: refresh.map_or(60.0, |r| r as f64 / 1000.0),
                wide_gamut: false,
                hdr: HdrStatus::Unsupported,
                luminance: Luminance {
                    peak_nits: 203.0,
                    min_nits: None,
                    max_frame_avg_nits: None,
                    source: LuminanceSource::Assumed,
                },
                edid: None,
                os: OsReport::Unknown,
            }
        })
}

// ---- The application --------------------------------------------------------------------------

impl Running {
    /// Something was opened: by a drop, from the dialog, from Finder, on the command line.
    /// A video is played, and its folder becomes the list around it; a folder, a playlist or
    /// several files become the list, and its first file is played once it has been read.
    /// Subtitle and sound files go with the video: the one opened with them, or the one playing.
    fn open(&mut self, paths: Vec<String>, settings: &Settings, start: Option<f64>) {
        let (subtitles, others): (Vec<String>, Vec<String>) = paths
            .into_iter()
            .partition(|path| sidecars::is_subtitle_file(path));
        let (sounds, others): (Vec<String>, Vec<String>) = others
            .into_iter()
            .partition(|path| sidecars::is_sound_file(path));
        if others.is_empty() {
            if self.session.is_some() {
                for file in subtitles {
                    self.send(Command::AddSubtitles(file));
                }
                for file in sounds {
                    self.send(Command::AddSound(file));
                }
            } else if !subtitles.is_empty() {
                self.error = Some("Subtitles go with a video: open the video first.".into());
            } else if !sounds.is_empty() {
                self.error = Some("A sound file goes with a video: open the video first.".into());
            }
            return;
        }
        self.subtitles_to_add = subtitles;
        self.sounds_to_add = sounds;
        let others = others.into_iter().map(PathBuf::from).collect();
        if let Some(file) = self.queue.open(others) {
            self.play(&file.to_string_lossy(), settings, start);
        }
    }

    /// Play the file at this place in the list.
    fn go(&mut self, index: Option<usize>, settings: &Settings) {
        if let Some(file) = index.and_then(|index| self.queue.go(index)) {
            self.play(&file.to_string_lossy(), settings, None);
        }
    }

    /// To the next chapter or the previous one. `files`: past the last chapter (or the first),
    /// on to the next file (or the previous), as a track key does.
    fn chapter(&mut self, step: i32, files: bool, settings: &Settings) {
        let Some(status) = self.session.as_ref().map(|s| s.player.status()) else {
            return;
        };
        match crate::chapters::skip(&status.chapters, status.position, step) {
            crate::chapters::Skip::Seek(at) => self.send(Command::SeekTo(at)),
            crate::chapters::Skip::File if files => {
                let index = if step > 0 {
                    self.queue.next()
                } else {
                    self.queue.previous()
                };
                self.go(index, settings);
            }
            crate::chapters::Skip::File => {}
        }
    }

    /// Play `path`, replacing whatever is playing. `start` is where to begin, in seconds,
    /// when the command line says; otherwise where the file was stopped the last time.
    fn play(&mut self, path: &str, settings: &Settings, start: Option<f64>) {
        // The old session must be gone before a new renderer takes the layer.
        self.session = None;
        self.recent = None;
        self.error = None;
        // The last file's final frame is still on the layer: keep it out of sight until this
        // file's first frame replaces it.
        self.surface.set_hidden(true);

        let file_name = Path::new(path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string());
        println!("File:    {path}");
        let recalled = self.memory.opened(path);
        // A folder opened whole, or a playlist, is picked up on this file the next time.
        if let Some(list) = self.queue.remembered_as() {
            self.memory.list_at(list, Path::new(path));
        }
        let resumed_from = recalled.start.filter(|_| start.is_none());
        if let Some(from) = resumed_from {
            println!("Resume:  from {}", report::clock(from));
        }
        let size = self.window.inner_size();
        let options = Options {
            path: path.to_string(),
            hardware: self.hardware,
            mode: self.mode,
            dolby_vision: self.prefs.dolby_vision,
            quality: self.quality,
            upscaler: self.prefs.upscaler(),
            shader_cache: Some(Preferences::shader_cache(&self.preferences_path)),
            interpolation: self.prefs.interpolation(),
            renderer_options: settings.renderer_options.clone(),
            start: start.or(recalled.start).unwrap_or(0.0),
            sound: settings.sound,
            audio_delay: self.audio_delay,
            speed: self.speed,
            subtitle_delay: recalled.subtitle_delay,
            picture: self.picture,
            night: self.prefs.night_mode,
            sound_device: self.prefs.sound_output(),
            volume: self.volume,
            muted: self.muted,
            subtitle_lift: self.subtitle_lift(),
            subtitle_scale: self.prefs.subtitle_scale,
            subtitle_nits: self.prefs.subtitle_nits,
            audio_language: self.prefs.audio_language.clone(),
            subtitle_language: self.prefs.subtitle_language.clone(),
            audio_track: recalled.audio_track,
            subtitles: recalled.subtitles,
            sound_place: recalled.sound_place,
            own_sound: recalled.own_sound,
        };
        // SAFETY: `Running` drops the session before the layer, and `open` drops the previous
        // session before starting another on the same layer.
        let player = unsafe {
            Player::start(
                self.surface.pointer(),
                (size.width, size.height),
                self.shown_display(),
                options,
            )
        };
        for file in self.subtitles_to_add.drain(..) {
            player.send(Command::AddSubtitles(file));
        }
        for file in self.sounds_to_add.drain(..) {
            player.send(Command::AddSound(file));
        }
        self.window.set_title(&file_name);
        self.session = Some(Session {
            player,
            file_name,
            path: path.to_string(),
            printed_media: String::new(),
            printed_chapters: false,
            printed_sound: String::new(),
            printed_path: String::new(),
            printed_layer: String::new(),
            printed_advice: Vec::new(),
            sized_to_video: false,
            layer_hdr: None,
            clock_origin: None,
            underruns: 0,
            resume_note: resumed_from.map(ResumeNote::new),
        });
    }

    /// How far up subtitles must stay, in pixels: the height the transport bar takes at the
    /// bottom of the window while the controls are showing (see `Transport` in Overlay.swift).
    fn subtitle_lift(&self) -> f32 {
        const TRANSPORT_AND_MARGINS: f64 = 56.0 + 22.0 + 14.0;
        if self.controls_visible {
            (TRANSPORT_AND_MARGINS * self.window.scale_factor()) as f32
        } else {
            0.0
        }
    }

    /// The command that steps to the next subtitle track, ending with none; and to the next
    /// sound track, going round. (The Mac's interface works these out itself.)
    #[cfg(not(target_os = "macos"))]
    fn next_subtitles(&self) -> Option<Command> {
        use bsv_engine::TrackKind;
        let status = self.session.as_ref()?.player.status();
        let ids: Vec<i32> = status
            .tracks
            .iter()
            .filter(|t| t.kind == TrackKind::Subtitle)
            .map(|t| t.index)
            .collect();
        let next = match status
            .subtitle_track
            .and_then(|now| ids.iter().position(|&i| i == now))
        {
            Some(at) => ids.get(at + 1).copied(),
            None => ids.first().copied(),
        };
        (!ids.is_empty()).then_some(Command::SelectSubtitle(next))
    }

    #[cfg(not(target_os = "macos"))]
    fn next_sound(&self) -> Option<Command> {
        use bsv_engine::TrackKind;
        let status = self.session.as_ref()?.player.status();
        let ids: Vec<i32> = status
            .tracks
            .iter()
            .filter(|t| t.kind == TrackKind::Audio)
            .map(|t| t.index)
            .collect();
        let at = ids.iter().position(|&i| Some(i) == status.audio_track)?;
        (ids.len() > 1).then(|| Command::SelectAudio(ids[(at + 1) % ids.len()]))
    }

    fn toggle_fullscreen(&self) {
        let next = if self.window.fullscreen().is_some() {
            None
        } else {
            Some(Fullscreen::Borderless(None))
        };
        self.window.set_fullscreen(next);
    }

    fn send(&mut self, command: Command) {
        match command {
            // A track chosen by hand is remembered for this file.
            Command::SelectAudio(track) => {
                // A sound file next to the video is remembered by where it is, so that the
                // next episode's file from the same place can be put on with it.
                let place = self.session.as_ref().and_then(|session| {
                    let places = session.player.status().sound_places;
                    places.into_iter().find(|(index, _)| *index == track)
                });
                self.memory
                    .chose_audio(track, place.as_ref().map(|(_, place)| place.as_str()));
            }
            Command::SelectSubtitle(track) => self.memory.chose_subtitles(track),
            // Kept for the files after this one, until the player is closed.
            Command::SetSpeed(speed) => {
                self.speed = speed.clamp(playback::MIN_SPEED, playback::MAX_SPEED);
            }
            // Subtitles moved to meet the speech stay moved for this file.
            Command::SetSubtitleDelay(seconds) => self.memory.moved_subtitles(seconds),
            // Going anywhere else answers the note about where the file was picked up.
            Command::SeekTo(_) | Command::SeekBy(_) => {
                if let Some(session) = &mut self.session {
                    session.resume_note = None;
                }
            }
            _ => {}
        }
        if let Some(session) = &self.session {
            session.player.send(command);
        }
    }

    /// The display as the player should treat it: as reported, with the user's own peak
    /// brightness in place of the display's claim when one is set.
    fn shown_display(&self) -> Display {
        let mut display = self.display.clone();
        if let Some(peak) = self.prefs.display_peak {
            display.luminance.peak_nits = peak;
            display.luminance.source = LuminanceSource::User;
        }
        display
    }

    fn save_preferences(&self) {
        if let Err(e) = self.saved.save(&self.preferences_path) {
            eprintln!(
                "[warn] cannot save settings to {}: {e}",
                self.preferences_path.display()
            );
        }
    }

    /// Change a preference from the interface: keep it, and put it into effect at once where
    /// that is possible. Decoder and language choices apply from the next file.
    fn set_preference(&mut self, key: &str, value: &str) {
        self.saved.set(key, value);
        if !self.prefs.set(key, value) {
            return;
        }
        if key == "soundDevice" {
            let devices = &self.devices.get_or_insert_with(crate::sound_devices).0;
            self.saved.name_device(devices);
            self.prefs.name_device(devices);
        }
        self.save_preferences();
        match key {
            "displayPeak" => self.send(Command::Display(Box::new(self.shown_display()))),
            "quality" => {
                self.quality = self.prefs.render_quality();
                self.send(Command::SetQuality(self.quality));
            }
            "upscaling" => self.send(Command::SetUpscaler(self.prefs.upscaler())),
            "interpolation" => {
                self.send(Command::SetInterpolation(self.prefs.interpolation()));
            }
            "hardwareDecoding" => self.hardware = self.prefs.hardware_decoding,
            "dolbyVision" => self.send(Command::SetDolbyVision(self.prefs.dolby_vision)),
            "nightMode" => self.send(Command::SetNight(self.prefs.night_mode)),
            "soundDevice" => self.send(Command::SetSoundDevice(self.prefs.sound_output())),
            "subtitleScale" | "subtitleNits" => self.send(Command::SubtitleLook {
                scale: self.prefs.subtitle_scale,
                nits: self.prefs.subtitle_nits,
            }),
            "audioDelay" => {
                self.audio_delay = self.prefs.audio_delay_ms as f64 / 1000.0;
                self.send(Command::SetAudioDelay(self.audio_delay));
            }
            _ => {}
        }
    }

    fn handle(&mut self, request: Request, settings: &Settings) {
        match request {
            Request::Player(Command::SetHdrMode(mode)) => {
                // Chosen in the signal panel, and kept: it is a preference like the others.
                self.saved.set_mode(mode);
                self.prefs.set_mode(mode);
                self.save_preferences();
                self.send(Command::SetHdrMode(mode));
            }
            Request::Player(command) => self.send(command),
            Request::Set(key, value) => self.set_preference(&key, &value),
            Request::ControlsVisible(visible) => {
                self.controls_visible = visible;
                self.send(Command::SubtitleLift(self.subtitle_lift()));
            }
            Request::Open(paths) => self.open(paths, settings, None),
            Request::Add(paths) => self
                .queue
                .add(paths.into_iter().map(PathBuf::from).collect()),
            Request::Next => self.go(self.queue.next(), settings),
            Request::Previous => self.go(self.queue.previous(), settings),
            Request::Chapter(step) => self.chapter(step, false, settings),
            Request::Thumbnail(at) => {
                if let Some(session) = &self.session {
                    session.player.thumbnail(at);
                }
            }
            Request::SoundDevices => self.devices = None,
            Request::SaveFrame => {
                if let Some(session) = &self.session {
                    let at = session.player.status().position;
                    session
                        .player
                        .save_frame(prefs::frame_file(&session.file_name, at));
                }
            }
            Request::Picture(change) => {
                match change {
                    PictureChange::Shape(shape) => self.picture.shape = shape,
                    PictureChange::Cut(cut) => self.picture.cut = cut,
                    PictureChange::Fill(fill) => self.picture.fill = fill,
                }
                self.send(Command::SetPicture(self.picture));
            }
            Request::SpeedBy(step) => {
                let speed =
                    steps::speed(self.speed, step, playback::MIN_SPEED, playback::MAX_SPEED);
                self.send(Command::SetSpeed(speed));
            }
            Request::SubtitleDelayBy(step) => {
                if let Some(now) = self
                    .session
                    .as_ref()
                    .map(|s| s.player.status().subtitle_delay)
                {
                    let delay = steps::subtitle_delay(now, step, playback::MAX_SUBTITLE_DELAY);
                    self.send(Command::SetSubtitleDelay(delay));
                }
            }
            Request::Skip(step) => self.chapter(step, true, settings),
            Request::Play(index) => self.go(Some(index), settings),
            Request::Remove(index) => self.queue.remove(index),
            Request::Move(from, to) => self.queue.move_item(from, to),
            Request::Save(file) => {
                if let Err(e) = self.queue.save(Path::new(&file)) {
                    eprintln!("[warn] cannot save the playlist: {e}");
                }
            }
            Request::ToggleFullscreen => self.toggle_fullscreen(),
            Request::ExitFullscreen => self.window.set_fullscreen(None),
        }
    }

    /// Look at the playback state: print what changed, keep the layer and the title in step,
    /// and tell the interface.
    fn refresh(&mut self, settings: &Settings) {
        // A folder or a playlist that was opened has been read.
        match self.queue.poll(|list| self.memory.list_left_at(list)) {
            Arrived::Play(file) => self.play(&file.to_string_lossy(), settings, None),
            Arrived::Failed(e) => {
                eprintln!("bitspring-video: {e}");
                if self.session.is_none() {
                    self.error = Some(e);
                }
            }
            Arrived::Nothing => {}
        }
        // An episode has ended and the next one is there: go on to it.
        if self.prefs.auto_next
            && self
                .session
                .as_ref()
                .is_some_and(|s| s.player.status().ended)
            && let Some(next) = self.queue.after_end()
        {
            println!("Next:    the file has ended, going on to the one after it");
            self.go(Some(next), settings);
        }

        let mut failed = None;
        let shown = self.shown_display();
        let queue = self.session.is_some().then(|| self.queue.state().clone());
        if let Some(session) = &mut self.session {
            let status = session.player.status();
            if let Some(e) = &status.error {
                failed = Some(format!("{}: {e}", session.file_name));
            }
            self.volume = status.volume;
            self.muted = status.muted;
            self.mode = status.mode;
            // Once a picture is on screen the position is a real one, worth remembering.
            if status.stats.output_transfer.is_some() {
                let duration = status.info.as_ref().map_or(0.0, |i| i.duration);
                self.memory
                    .note(status.position, duration, status.ended, status.paused);
            }

            if let Some(info) = &status.info {
                // Printed again when the first decoded frame adds what the container did not say.
                let media = report::media(info);
                if media != session.printed_media {
                    println!("Video:   {media}");
                    session.printed_media = media;
                }
                // A sound file next to the video is named: its format alone does not say
                // which of several it is.
                let beside = status.audio_track.and_then(|playing| {
                    let outside = status.sound_places.iter().any(|(i, _)| *i == playing);
                    let track = status.tracks.iter().find(|t| t.index == playing);
                    track.filter(|_| outside).map(|t| t.title.clone())
                });
                let sound = match (&status.sound, beside) {
                    (Some(s), Some(file)) => {
                        format!("{}, from {file}", report::sound_path(&s.track, &s.device))
                    }
                    (Some(s), None) => report::sound_path(&s.track, &s.device),
                    (None, _) => "none (the picture runs on the system clock)".to_string(),
                };
                if !status.chapters.is_empty() && !session.printed_chapters {
                    session.printed_chapters = true;
                    let first = status.chapters.iter().take(3).map(report::chapter);
                    let more = status.chapters.len().saturating_sub(3);
                    let more = if more > 0 {
                        format!(", and {more} more")
                    } else {
                        String::new()
                    };
                    println!(
                        "Chapters: {} ({}{more})",
                        status.chapters.len(),
                        first.collect::<Vec<_>>().join(", ")
                    );
                }
                if sound != session.printed_sound {
                    println!("Sound:   {sound}");
                    session.printed_sound = sound;
                }

                if !session.sized_to_video && info.width > 0 && info.height > 0 {
                    session.sized_to_video = true;
                    if self.window.fullscreen().is_none() {
                        // Fit the window to the picture's shape, at most 70% of the screen.
                        let scale = self.window.scale_factor();
                        let max_w = self.display.width as f64 * 0.7;
                        let max_h = self.display.height as f64 * 0.7;
                        // As shown: pixels that are not square (a DVD) made so.
                        let shown_w = info.width as f64 * info.pixel_aspect;
                        let fit = (max_w / shown_w)
                            .min(max_h / info.height as f64)
                            .min(scale.max(1.0));
                        let size = PhysicalSize::new(
                            (shown_w * fit) as u32,
                            (info.height as f64 * fit) as u32,
                        );
                        let _ = self.window.request_inner_size(size);
                    }
                }

                if let Some(output) = &status.output
                    && status.stats.output_transfer.is_some()
                {
                    let path = report::signal_path(info, &status.stats, output, status.mode);
                    if path != session.printed_path {
                        println!("Signal:  {path}");
                        session.printed_path = path;
                    }
                    let hdr = matches!(
                        status.stats.output_transfer,
                        Some(Transfer::Pq | Transfer::Hlg)
                    );
                    if session.layer_hdr != Some(hdr) {
                        if session.layer_hdr.is_none() {
                            self.surface.set_hidden(false);
                        }
                        self.surface.set_hdr(hdr);
                        session.layer_hdr = Some(hdr);
                    }
                    if let Some(layer) = self.surface.describe()
                        && layer != session.printed_layer
                    {
                        println!("Layer:   {layer}");
                        session.printed_layer = layer;
                    }
                    let advice: Vec<String> = output.advice.iter().map(report::advice).collect();
                    if advice != session.printed_advice {
                        for line in &advice {
                            println!("Note:    {line}");
                        }
                        session.printed_advice = advice;
                    }
                }
            }
            if status.underruns > session.underruns {
                println!(
                    "Note:    the sound was interrupted {} time(s)",
                    status.underruns - session.underruns
                );
                session.underruns = status.underruns;
            }

            let state = State::new(View {
                playing: Some(Playing {
                    file_name: &session.file_name,
                    path: &session.path,
                    status: &status,
                    resumed_from: session
                        .resume_note
                        .as_mut()
                        .and_then(|note| note.showing(status.stats.output_transfer.is_some())),
                }),
                display: &shown,
                reported_peak: self.display.luminance.peak_nits,
                fullscreen: self.window.fullscreen().is_some(),
                error: None,
                volume: status.volume,
                muted: status.muted,
                mode: status.mode,
                prefs: &self.prefs,
                queue: queue.as_ref(),
                recent: &[],
                devices: self.devices.get_or_insert_with(crate::sound_devices),
            });
            self.interface.show(&state);

            // With no interface drawn, the title bar says where playback is.
            #[cfg(not(target_os = "macos"))]
            {
                let state = if status.ended {
                    "  (end)"
                } else if status.paused {
                    "  (paused)"
                } else {
                    ""
                };
                let length = status.info.as_ref().map_or(0.0, |i| i.duration);
                self.window.set_title(&format!(
                    "{} - {} / {}{state} - BitSpring Video",
                    session.file_name,
                    report::clock(status.position),
                    report::clock(length)
                ));
            }
        }

        if let Some(error) = failed {
            eprintln!("bitspring-video: {error}");
            self.session = None;
            self.memory.closed();
            self.surface.set_hidden(true);
            self.error = Some(error);
            self.window.set_title("BitSpring Video");
        }
        if self.session.is_none() {
            let state = State::new(View {
                playing: None,
                display: &shown,
                reported_peak: self.display.luminance.peak_nits,
                fullscreen: self.window.fullscreen().is_some(),
                error: self.error.as_deref(),
                volume: self.volume,
                muted: self.muted,
                mode: self.mode,
                prefs: &self.prefs,
                queue: None,
                recent: self
                    .recent
                    .get_or_insert_with(|| self.memory.recent(RECENT_SHOWN)),
                devices: self.devices.get_or_insert_with(crate::sound_devices),
            });
            self.interface.show(&state);
        }
    }
}

impl App {
    fn start(&mut self, event_loop: &ActiveEventLoop) -> Result<(), String> {
        let attributes = Window::default_attributes()
            .with_title("BitSpring Video")
            .with_inner_size(LogicalSize::new(1280.0, 720.0))
            .with_min_inner_size(LogicalSize::new(640.0, 360.0))
            .with_theme(Some(Theme::Dark));
        // On the Mac the picture fills the whole window and the title bar's buttons float over it.
        #[cfg(target_os = "macos")]
        let attributes = attributes
            .with_titlebar_transparent(true)
            .with_fullsize_content_view(true)
            .with_title_hidden(true);
        let window = event_loop
            .create_window(attributes)
            .map_err(|e| e.to_string())?;

        // SAFETY: `Running` drops the surface before the window.
        let surface = unsafe { Surface::new(&window) }?;
        // SAFETY: the window's own view, on the main thread; dropped before the window.
        let interface = unsafe {
            Interface::attach(surface.view(), || {
                WAKE.with(|wake| {
                    if let Some(proxy) = wake.borrow().as_ref() {
                        let _ = proxy.send_event(());
                    }
                })
            })
        };

        let display = display_of(&window);
        println!("Display: {}", report::display(&display).trim_end());

        // What is saved, then what the command line asks for on top of it for this run.
        let saved = Preferences::load(&self.settings.preferences);
        let mut prefs = saved.clone();
        if let Some(mode) = self.settings.mode {
            prefs.set_mode(mode);
        }
        if let Some(quality) = self.settings.quality {
            prefs.quality = match quality {
                Quality::High => "high",
                Quality::Balanced => "balanced",
                Quality::Fast => "fast",
            }
            .into();
        }
        if let Some(hardware) = self.settings.hardware {
            prefs.hardware_decoding = hardware;
        }
        if let Some(delay) = self.settings.audio_delay {
            prefs.audio_delay_ms = (delay * 1000.0) as f32;
        }

        let mut running = Running {
            session: None,
            interface,
            surface,
            window,
            display,
            error: None,
            controls_visible: true,
            volume: prefs.volume,
            muted: false,
            mode: prefs.mode(),
            quality: prefs.render_quality(),
            hardware: prefs.hardware_decoding,
            audio_delay: prefs.audio_delay_ms as f64 / 1000.0,
            speed: 1.0,
            picture: Picture::default(),
            recent: None,
            devices: None,
            saved,
            prefs,
            preferences_path: self.settings.preferences.clone(),
            memory: Memory::beside(&self.settings.preferences),
            queue: Queue::default(),
            subtitles_to_add: Vec::new(),
            sounds_to_add: Vec::new(),
        };
        if !self.settings.paths.is_empty() {
            running.open(
                self.settings.paths.clone(),
                &self.settings,
                self.settings.start,
            );
        }
        running.refresh(&self.settings);
        self.running = Some(running);
        Ok(())
    }

    fn handle_requests(&mut self) {
        let Some(running) = &mut self.running else {
            return;
        };
        for request in ui::take_requests() {
            running.handle(request, &self.settings);
        }
    }

    /// Run the script steps whose time has come.
    fn run_script(&mut self, event_loop: &ActiveEventLoop) {
        let now = self.started.elapsed().as_secs_f64();
        while self.settings.script.first().is_some_and(|s| s.at <= now) {
            let step = self.settings.script.remove(0);
            let Some(r) = &mut self.running else { return };
            let position = r
                .session
                .as_ref()
                .map_or(0.0, |s| s.player.status().position);
            println!(
                "Script:  {:.1} s  {}  (position {position:.3} s)",
                step.at, step.action
            );
            if let Some(session) = &mut r.session {
                session.clock_origin = None;
            }
            let number = |text: &str| text.parse::<f64>().unwrap_or(0.0);
            match step.action.as_str() {
                "pause" => r.send(Command::TogglePause),
                "mute" => r.send(Command::ToggleMute),
                "hdr" => r.send(Command::CycleHdrMode),
                "fullscreen" => r.toggle_fullscreen(),
                "quit" => event_loop.exit(),
                "panel" | "pin" | "settings" | "mini" | "on-top" => {
                    r.interface.perform(&step.action)
                }
                a if a.starts_with("set ") => {
                    let (key, value) = a[4..].trim().split_once(' ').unwrap_or((a[4..].trim(), ""));
                    r.set_preference(key, value);
                }
                a if a.starts_with("seek") => r.send(Command::SeekBy(number(&a[4..]))),
                a if a.starts_with("audio ") => {
                    r.send(Command::SelectAudio(number(a[6..].trim()) as i32))
                }
                "subs off" => r.send(Command::SelectSubtitle(None)),
                a if a.starts_with("subs ") => {
                    r.send(Command::SelectSubtitle(Some(number(a[5..].trim()) as i32)))
                }
                a if a.starts_with("volume") => {
                    r.send(Command::VolumeBy(number(&a[6..]) as f32 / 100.0))
                }
                "next" => r.go(r.queue.next(), &self.settings),
                "previous" => r.go(r.queue.previous(), &self.settings),
                "queue" => r.interface.perform("queue"),
                a if a.starts_with("play ") => {
                    r.go(Some(number(a[5..].trim()) as usize), &self.settings)
                }
                a if a.starts_with("open ") => {
                    let paths = a[5..].split('|').map(|p| p.trim().to_string()).collect();
                    r.open(paths, &self.settings, None)
                }
                a if a.starts_with("add ") => r.handle(
                    Request::Add(a[4..].split('|').map(|p| p.trim().to_string()).collect()),
                    &self.settings,
                ),
                a if a.starts_with("save ") => {
                    r.handle(Request::Save(a[5..].trim().to_string()), &self.settings)
                }
                // Anything the interface can say: "chapter-next", "skip-previous" ...
                other => match ui::parse(other) {
                    Some(request) => r.handle(request, &self.settings),
                    None => eprintln!("[warn] unknown script command \"{other}\""),
                },
            }
        }
    }

    /// Once a second: where playback is, and how it has moved against the system clock since
    /// it last started running. With sound the two are separate clocks, so a steady figure
    /// means the picture is following the sound device smoothly.
    fn report_clock(&mut self) {
        let Some(session) = self.running.as_mut().and_then(|r| r.session.as_mut()) else {
            return;
        };
        let status = session.player.status();
        if !self.settings.diagnostics || status.paused || status.ended || status.info.is_none() {
            session.clock_origin = None;
            return;
        }
        match session.clock_origin {
            // Measured afresh when the speed changes: the film's time runs at it.
            Some((_, _, speed)) if speed != status.speed => session.clock_origin = None,
            None => session.clock_origin = Some((Instant::now(), status.position, status.speed)),
            Some((since, from, speed)) => {
                let drift = (status.position - from) - since.elapsed().as_secs_f64() * speed;
                println!(
                    "Clock:   {:.3} s, {:+.1} ms against the system clock, {} pictures a second, each takes the GPU {:.1} ms, {} refreshes missed{}{}{}{}",
                    status.position,
                    drift * 1000.0,
                    status.pictures,
                    status.picture_gpu_ms,
                    status.late,
                    if status.stats.upscaled {
                        ", upscaled"
                    } else {
                        ""
                    },
                    report::in_between(status.in_between, status.in_between_ms),
                    if status.stats.subtitle_shown {
                        ", a subtitle on screen"
                    } else {
                        ""
                    },
                    if speed != 1.0 {
                        format!(", at {}x", report::trimmed(speed))
                    } else {
                        String::new()
                    }
                );
            }
        }
    }

    fn poll_display(&mut self) {
        let Some(r) = &mut self.running else { return };
        let display = display_of(&r.window);
        // The headroom in use moves with what is on screen; it is not a change of display.
        let in_use = headroom_in_use(&display);
        if (in_use - headroom_in_use(&r.display)).abs() > 0.05 {
            println!("Headroom: the system is giving {in_use:.2}x SDR white");
        }
        let changed = without_live_state(&display) != without_live_state(&r.display);
        r.display = display;
        if changed {
            let shown = r.shown_display();
            println!("Display: {}", report::display(&shown).trim_end());
            r.send(Command::Display(Box::new(shown)));
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.running.is_none()
            && let Err(e) = self.start(event_loop)
        {
            self.failure = Some(e);
            event_loop.exit();
        }
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, _event: ()) {
        self.handle_requests();
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(r) = &mut self.running else { return };
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => r.send(Command::Resize(size.width, size.height)),
            WindowEvent::Moved(_) | WindowEvent::ScaleFactorChanged { .. } => {
                // Possibly on another display now: look again right away.
                self.last_display_poll = Instant::now() - DISPLAY_POLL;
            }
            // The Mac's interface takes drops itself; elsewhere the window does.
            #[cfg(not(target_os = "macos"))]
            WindowEvent::DroppedFile(path) => {
                r.handle(
                    Request::Open(vec![path.to_string_lossy().into_owned()]),
                    &self.settings,
                );
            }
            #[cfg(not(target_os = "macos"))]
            WindowEvent::ModifiersChanged(m) => self.modifiers = m.state(),
            #[cfg(not(target_os = "macos"))]
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => {
                let now = Instant::now();
                if self
                    .last_click
                    .is_some_and(|t| now - t < Duration::from_millis(400))
                {
                    self.last_click = None;
                    r.toggle_fullscreen();
                } else {
                    self.last_click = Some(now);
                }
            }
            #[cfg(not(target_os = "macos"))]
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                let step = if self.modifiers.shift_key() {
                    60.0
                } else {
                    5.0
                };
                let request = match event.logical_key.as_ref() {
                    Key::Named(NamedKey::Space) if !event.repeat => {
                        Some(Request::Player(Command::TogglePause))
                    }
                    Key::Named(NamedKey::ArrowLeft) => {
                        Some(Request::Player(Command::SeekBy(-step)))
                    }
                    Key::Named(NamedKey::ArrowRight) => {
                        Some(Request::Player(Command::SeekBy(step)))
                    }
                    Key::Named(NamedKey::ArrowUp) => Some(Request::Player(Command::VolumeBy(0.05))),
                    Key::Named(NamedKey::ArrowDown) => {
                        Some(Request::Player(Command::VolumeBy(-0.05)))
                    }
                    Key::Named(NamedKey::PageDown) => Some(Request::Chapter(1)),
                    Key::Named(NamedKey::PageUp) => Some(Request::Chapter(-1)),
                    Key::Named(NamedKey::MediaPlayPause) => {
                        Some(Request::Player(Command::TogglePause))
                    }
                    Key::Named(NamedKey::MediaTrackNext) => Some(Request::Skip(1)),
                    Key::Named(NamedKey::MediaTrackPrevious) => Some(Request::Skip(-1)),
                    Key::Named(NamedKey::Enter) if !event.repeat => Some(Request::ToggleFullscreen),
                    Key::Named(NamedKey::Escape) => Some(Request::ExitFullscreen),
                    Key::Character(c) if !event.repeat => match c.to_lowercase().as_str() {
                        "f" => Some(Request::ToggleFullscreen),
                        "m" => Some(Request::Player(Command::ToggleMute)),
                        "h" => Some(Request::Player(Command::CycleHdrMode)),
                        "s" => r.next_subtitles().map(Request::Player),
                        "a" => r.next_sound().map(Request::Player),
                        _ => None,
                    },
                    _ => None,
                };
                if let Some(request) = request {
                    r.handle(request, &self.settings);
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.handle_requests();
        if self.last_display_poll.elapsed() >= DISPLAY_POLL {
            self.last_display_poll = Instant::now();
            self.poll_display();
        }
        if self.last_clock_report.elapsed() >= DISPLAY_POLL {
            self.last_clock_report = Instant::now();
            self.report_clock();
        }
        self.run_script(event_loop);
        if let Some(r) = &mut self.running {
            r.refresh(&self.settings);
        }
        let wait = if self.settings.script.is_empty() {
            INTERFACE_REFRESH
        } else {
            Duration::from_millis(20)
        };
        event_loop.set_control_flow(ControlFlow::WaitUntil(Instant::now() + wait));
    }
}
