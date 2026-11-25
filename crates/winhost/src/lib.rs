//! The player as a library for the Windows application (`windows/BitSpringVideo`, WinUI 3).
//!
//! The application owns the window and draws the controls; this library plays into a
//! swapchain that the application shows in one of its panels. The two talk the same way the
//! Mac's interface and the player do: the player's state goes out as JSON, commands come in
//! as short lines of text ("toggle-pause", "seek-to 12.5", "open C:\film.mkv").
//!
//! Every function here is called from the application's interface thread.
#![cfg(windows)]

use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bitspring_video::chapters;
use bitspring_video::history::{Memory, ResumeNote};
use bitspring_video::playback::{self, Command, Options, Player};
use bitspring_video::prefs::Preferences;
use bitspring_video::queue::{Arrived, Queue};
use bitspring_video::report;
use bitspring_video::sidecars;
use bitspring_video::steps;
use bitspring_video::ui::{self, Playing, Request, State, View};
use bsv_display::policy::HdrMode;
use bsv_display::{Display, HdrStatus, Luminance, LuminanceSource, OsReport};
use bsv_engine::LogLevel;

/// Height the transport bar and its margins take at the bottom of the window, in the
/// application's units (see `MainWindow.xaml`): subtitles stay above it while it shows.
const TRANSPORT_AND_MARGINS: f32 = 56.0 + 22.0 + 14.0;
const DISPLAY_POLL: Duration = Duration::from_millis(1000);

/// A COM object held by one counted reference.
struct ComRef(usize);

impl ComRef {
    /// Take a new reference to `object`.
    ///
    /// # Safety
    /// `object` must be a live COM interface pointer.
    unsafe fn adopt(object: usize) -> ComRef {
        // SAFETY: every COM interface starts with IUnknown's QueryInterface, AddRef, Release.
        unsafe {
            let table = *(object as *const *const [usize; 3]);
            let add_ref: unsafe extern "system" fn(*mut c_void) -> u32 =
                std::mem::transmute((*table)[1]);
            add_ref(object as *mut c_void);
        }
        ComRef(object)
    }

    /// Give the reference away: the receiver must release it.
    fn into_raw(self) -> *mut c_void {
        let object = self.0;
        std::mem::forget(self);
        object as *mut c_void
    }
}

impl Drop for ComRef {
    fn drop(&mut self) {
        // SAFETY: we hold a reference taken in `adopt`.
        unsafe {
            let table = *(self.0 as *const *const [usize; 3]);
            let release: unsafe extern "system" fn(*mut c_void) -> u32 =
                std::mem::transmute((*table)[2]);
            release(self.0 as *mut c_void);
        }
    }
}

// SAFETY: a swapchain's reference count may be touched from any thread.
unsafe impl Send for ComRef {}

/// What the application should do with its video panel next.
enum PanelChange {
    Show(ComRef),
    Clear,
}

struct Session {
    player: Player,
    file_name: String,
    logged_media: String,
    logged_signal: String,
    /// Shown for a while when the file was picked up where it was left.
    resume_note: Option<ResumeNote>,
}

pub struct Host {
    session: Option<Session>,
    /// Where each file was stopped, and the tracks chosen for it.
    memory: Memory,
    /// What plays after what: the folder of the file that was opened, or a playlist.
    queue: Queue,
    /// Subtitle files opened together with a video: shown once the video starts.
    subtitles_to_add: Vec<String>,
    /// Sound files opened along with a video, to be played with it.
    sounds_to_add: Vec<String>,
    /// The display as the system reports it, and which one the window is on.
    display: Display,
    monitor: u64,
    last_display_poll: Instant,
    error: Option<String>,
    saved: Preferences,
    prefs: Preferences,
    preferences_path: PathBuf,
    volume: f32,
    muted: bool,
    /// How fast files play: kept from one to the next until the player is closed.
    speed: f64,
    /// False to play silently, for trying things out.
    sound: bool,
    mode: HdrMode,
    /// The video panel's size in pixels, and how many pixels one of its units is.
    size: (u32, u32),
    scale: f32,
    controls_visible: bool,
    fullscreen: bool,
    /// Filled by the render thread when a session's swapchain exists.
    new_swapchain: Arc<Mutex<Option<ComRef>>>,
    clear_panel: bool,
    state: CString,
}

fn display_on(monitor: u64) -> Display {
    let all = bsv_display::displays().unwrap_or_default();
    all.iter()
        .find(|d| d.id == monitor)
        .cloned()
        .or_else(|| all.into_iter().next())
        .unwrap_or(Display {
            id: monitor,
            name: "Display".into(),
            builtin: false,
            connection: None,
            width: 0,
            height: 0,
            refresh_hz: 60.0,
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
        })
}

impl Host {
    fn new(preferences_path: PathBuf, sound: bool) -> Host {
        let saved = Preferences::load(&preferences_path);
        let display = display_on(0);
        bsv_engine::log_line(&format!(
            "Display: {}",
            report::display(&display).trim_end()
        ));
        Host {
            session: None,
            memory: Memory::beside(&preferences_path),
            queue: Queue::default(),
            subtitles_to_add: Vec::new(),
            sounds_to_add: Vec::new(),
            display,
            monitor: 0,
            last_display_poll: Instant::now(),
            error: None,
            volume: saved.volume,
            muted: false,
            speed: 1.0,
            sound,
            mode: saved.mode(),
            prefs: saved.clone(),
            saved,
            preferences_path,
            size: (1280, 720),
            scale: 1.0,
            controls_visible: true,
            fullscreen: false,
            new_swapchain: Arc::new(Mutex::new(None)),
            clear_panel: false,
            state: CString::default(),
        }
    }

    /// The display as the player should treat it: with the user's own peak, when one is set.
    fn shown_display(&self) -> Display {
        let mut display = self.display.clone();
        if let Some(peak) = self.prefs.display_peak {
            display.luminance.peak_nits = peak;
            display.luminance.source = LuminanceSource::User;
        }
        display
    }

    fn subtitle_lift(&self) -> f32 {
        if self.controls_visible {
            TRANSPORT_AND_MARGINS * self.scale
        } else {
            0.0
        }
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

    fn save(&self) {
        if let Err(e) = self.saved.save(&self.preferences_path) {
            bsv_engine::log_line(&format!("[warn] cannot save settings: {e}"));
        }
    }

    fn close(&mut self) {
        // Dropping the player stops its threads and destroys its swapchain.
        self.session = None;
        self.memory.closed();
        *self.new_swapchain.lock().unwrap() = None;
        self.clear_panel = true;
    }

    /// Something was opened: a video is played and its folder becomes the list around it; a
    /// folder, a playlist or several files become the list, played once it has been read.
    /// Subtitle and sound files go with the video: the one opened with them, or the one playing.
    fn open(&mut self, paths: Vec<String>) {
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
            self.play(&file.to_string_lossy());
        }
    }

    /// Play the file at this place in the list.
    fn go(&mut self, index: Option<usize>) {
        if let Some(file) = index.and_then(|index| self.queue.go(index)) {
            self.play(&file.to_string_lossy());
        }
    }

    /// To the next chapter or the previous one. `files`: past the last chapter (or the first),
    /// on to the next file (or the previous), as a track key does.
    fn chapter(&mut self, step: i32, files: bool) {
        let Some(status) = self.session.as_ref().map(|s| s.player.status()) else {
            return;
        };
        match chapters::skip(&status.chapters, status.position, step) {
            chapters::Skip::Seek(at) => self.send(Command::SeekTo(at)),
            chapters::Skip::File if files => {
                let index = if step > 0 {
                    self.queue.next()
                } else {
                    self.queue.previous()
                };
                self.go(index);
            }
            chapters::Skip::File => {}
        }
    }

    fn play(&mut self, path: &str) {
        self.close();
        self.error = None;
        let file_name = Path::new(path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string());
        bsv_engine::log_line(&format!("File:    {path}"));
        let recalled = self.memory.opened(path);
        // A folder opened whole, or a playlist, is picked up on this file the next time.
        if let Some(list) = self.queue.remembered_as() {
            self.memory.list_at(list, Path::new(path));
        }
        if let Some(from) = recalled.start {
            bsv_engine::log_line(&format!("Resume:  from {}", report::clock(from)));
        }
        let options = Options {
            path: path.to_string(),
            hardware: self.prefs.hardware_decoding,
            mode: self.mode,
            dolby_vision: self.prefs.dolby_vision,
            quality: self.prefs.render_quality(),
            upscaler: self.prefs.upscaler(),
            shader_cache: Some(Preferences::shader_cache(&self.preferences_path)),
            interpolation: self.prefs.interpolation(),
            renderer_options: String::new(),
            start: recalled.start.unwrap_or(0.0),
            sound: self.sound,
            audio_delay: self.prefs.audio_delay_ms as f64 / 1000.0,
            speed: self.speed,
            subtitle_delay: recalled.subtitle_delay,
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
        let slot = self.new_swapchain.clone();
        let player =
            Player::start_composed(self.size, self.shown_display(), options, move |swapchain| {
                // SAFETY: the renderer has just made this swapchain and holds it for the session.
                *slot.lock().unwrap() = Some(unsafe { ComRef::adopt(swapchain) });
            });
        player.send(Command::Scale(self.scale));
        for file in self.subtitles_to_add.drain(..) {
            player.send(Command::AddSubtitles(file));
        }
        for file in self.sounds_to_add.drain(..) {
            player.send(Command::AddSound(file));
        }
        self.clear_panel = false;
        self.session = Some(Session {
            player,
            file_name,
            logged_media: String::new(),
            logged_signal: String::new(),
            resume_note: recalled.start.map(ResumeNote::new),
        });
    }

    fn set_preference(&mut self, key: &str, value: &str) {
        self.saved.set(key, value);
        if !self.prefs.set(key, value) {
            return;
        }
        self.save();
        match key {
            "displayPeak" => self.send(Command::Display(Box::new(self.shown_display()))),
            "quality" => self.send(Command::SetQuality(self.prefs.render_quality())),
            "upscaling" => self.send(Command::SetUpscaler(self.prefs.upscaler())),
            "interpolation" => {
                self.send(Command::SetInterpolation(self.prefs.interpolation()));
            }
            "dolbyVision" => self.send(Command::SetDolbyVision(self.prefs.dolby_vision)),
            "subtitleScale" | "subtitleNits" => self.send(Command::SubtitleLook {
                scale: self.prefs.subtitle_scale,
                nits: self.prefs.subtitle_nits,
            }),
            "audioDelay" => self.send(Command::SetAudioDelay(
                self.prefs.audio_delay_ms as f64 / 1000.0,
            )),
            _ => {}
        }
    }

    fn command(&mut self, text: &str) {
        match ui::parse(text) {
            Some(Request::Player(Command::SetHdrMode(mode))) => {
                self.mode = mode;
                self.saved.set_mode(mode);
                self.prefs.set_mode(mode);
                self.save();
                self.send(Command::SetHdrMode(mode));
            }
            Some(Request::Player(command)) => self.send(command),
            Some(Request::Set(key, value)) => self.set_preference(&key, &value),
            Some(Request::Open(paths)) => self.open(paths),
            Some(Request::Add(paths)) => self
                .queue
                .add(paths.into_iter().map(PathBuf::from).collect()),
            Some(Request::Next) => self.go(self.queue.next()),
            Some(Request::Previous) => self.go(self.queue.previous()),
            Some(Request::Chapter(step)) => self.chapter(step, false),
            Some(Request::SpeedBy(step)) => {
                let speed = steps::speed(self.speed, step, playback::MIN_SPEED, playback::MAX_SPEED);
                self.send(Command::SetSpeed(speed));
            }
            Some(Request::SubtitleDelayBy(step)) => {
                if let Some(now) = self.session.as_ref().map(|s| s.player.status().subtitle_delay) {
                    let delay = steps::subtitle_delay(now, step, playback::MAX_SUBTITLE_DELAY);
                    self.send(Command::SetSubtitleDelay(delay));
                }
            }
            Some(Request::Skip(step)) => self.chapter(step, true),
            Some(Request::Play(index)) => self.go(Some(index)),
            Some(Request::Remove(index)) => self.queue.remove(index),
            Some(Request::Move(from, to)) => self.queue.move_item(from, to),
            Some(Request::Save(file)) => {
                if let Err(e) = self.queue.save(Path::new(&file)) {
                    bsv_engine::log_line(&format!("[warn] cannot save the playlist: {e}"));
                }
            }
            Some(Request::ControlsVisible(visible)) => {
                self.controls_visible = visible;
                self.send(Command::SubtitleLift(self.subtitle_lift()));
            }
            // The application owns the window: it never asks the library for these.
            Some(Request::ToggleFullscreen | Request::ExitFullscreen) | None => {}
        }
    }

    fn poll_display(&mut self) {
        if self.last_display_poll.elapsed() < DISPLAY_POLL {
            return;
        }
        self.last_display_poll = Instant::now();
        let display = display_on(self.monitor);
        if display != self.display {
            self.display = display;
            let shown = self.shown_display();
            bsv_engine::log_line(&format!("Display: {}", report::display(&shown).trim_end()));
            self.send(Command::Display(Box::new(shown)));
        }
    }

    /// Bring everything up to date and describe it for the interface.
    fn refresh(&mut self) {
        self.poll_display();
        // A folder or a playlist that was opened has been read.
        match self.queue.poll(|list| self.memory.list_left_at(list)) {
            Arrived::Play(file) => self.play(&file.to_string_lossy()),
            Arrived::Failed(e) => {
                bsv_engine::log_line(&format!("[error] {e}"));
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
            bsv_engine::log_line("Next:    the file has ended, going on to the one after it");
            self.go(Some(next));
        }
        let queue = self.session.is_some().then(|| self.queue.state().clone());
        let shown = self.shown_display();
        let mut failed = None;
        let mut json = None;
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
                let media = report::media(info);
                if media != session.logged_media {
                    bsv_engine::log_line(&format!("Video:   {media}"));
                    session.logged_media = media;
                }
                if let Some(output) = &status.output
                    && status.stats.output_transfer.is_some()
                {
                    let signal = report::signal_path(info, &status.stats, output, status.mode);
                    if signal != session.logged_signal {
                        bsv_engine::log_line(&format!("Signal:  {signal}"));
                        session.logged_signal = signal;
                    }
                }
            }
            let state = State::new(View {
                playing: Some(Playing {
                    file_name: &session.file_name,
                    status: &status,
                    resumed_from: session
                        .resume_note
                        .as_mut()
                        .and_then(|note| note.showing(status.stats.output_transfer.is_some())),
                }),
                display: &shown,
                reported_peak: self.display.luminance.peak_nits,
                fullscreen: self.fullscreen,
                error: None,
                volume: status.volume,
                muted: status.muted,
                mode: status.mode,
                prefs: &self.prefs,
                queue: queue.as_ref(),
            });
            json = serde_json::to_string(&state).ok();
        }
        if let Some(error) = failed {
            bsv_engine::log_line(&format!("[error] {error}"));
            self.close();
            self.error = Some(error);
        }
        if self.session.is_none() {
            let state = State::new(View {
                playing: None,
                display: &shown,
                reported_peak: self.display.luminance.peak_nits,
                fullscreen: self.fullscreen,
                error: self.error.as_deref(),
                volume: self.volume,
                muted: self.muted,
                mode: self.mode,
                prefs: &self.prefs,
                queue: None,
            });
            json = serde_json::to_string(&state).ok();
        }
        self.state = json.and_then(|j| CString::new(j).ok()).unwrap_or_default();
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        self.session = None;
        self.memory.closed();
        if (self.saved.volume - self.volume).abs() > 0.001 {
            self.saved.volume = self.volume;
            self.save();
        }
    }
}

fn text(pointer: *const c_char) -> Option<String> {
    if pointer.is_null() {
        return None;
    }
    // SAFETY: the application passes NUL-terminated UTF-8 valid for the call.
    Some(
        unsafe { CStr::from_ptr(pointer) }
            .to_string_lossy()
            .into_owned(),
    )
}

/// Create the player. `settings`: where preferences are kept, or NULL for the usual place.
/// `log`: a file to write what the player does into, or NULL for none. `sound`: 0 to play
/// silently.
///
/// # Safety
/// Both strings, when given, must be NUL-terminated UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn bsv_host_create(
    settings: *const c_char,
    log: *const c_char,
    sound: c_int,
) -> *mut Host {
    if let Some(log) = text(log) {
        let _ = bsv_engine::log_to_file_up_to(Path::new(&log), LogLevel::Info);
    }
    let path = text(settings)
        .map(PathBuf::from)
        .unwrap_or_else(Preferences::default_path);
    Box::into_raw(Box::new(Host::new(path, sound != 0)))
}

/// # Safety
/// `host` must come from `bsv_host_create` and not be used afterwards.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn bsv_host_destroy(host: *mut Host) {
    if !host.is_null() {
        // SAFETY: upheld by the caller.
        drop(unsafe { Box::from_raw(host) });
    }
}

/// The video panel's size in pixels, and how many pixels one of its units is.
///
/// # Safety
/// `host` must be live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn bsv_host_resize(host: *mut Host, width: c_int, height: c_int, scale: f32) {
    // SAFETY: upheld by the caller.
    let host = unsafe { &mut *host };
    let size = (width.max(8) as u32, height.max(8) as u32);
    if size != host.size {
        host.size = size;
        host.send(Command::Resize(size.0, size.1));
    }
    if scale > 0.0 && (scale - host.scale).abs() > 0.001 {
        host.scale = scale;
        host.send(Command::Scale(scale));
        host.send(Command::SubtitleLift(host.subtitle_lift()));
    }
}

/// Which display the window is on (its HMONITOR), and whether the window is full screen.
///
/// # Safety
/// `host` must be live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn bsv_host_set_window(host: *mut Host, monitor: isize, fullscreen: c_int) {
    // SAFETY: upheld by the caller.
    let host = unsafe { &mut *host };
    host.fullscreen = fullscreen != 0;
    if monitor as u64 != host.monitor {
        host.monitor = monitor as u64;
        host.last_display_poll = Instant::now() - DISPLAY_POLL;
    }
}

/// One command, as the interface writes them.
///
/// # Safety
/// `host` must be live; `command` NUL-terminated UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn bsv_host_command(host: *mut Host, command: *const c_char) {
    // SAFETY: upheld by the caller.
    let host = unsafe { &mut *host };
    if let Some(command) = text(command) {
        host.command(&command);
    }
}

/// The player's state as JSON. The string is valid until the next call on this host.
///
/// # Safety
/// `host` must be live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn bsv_host_state(host: *mut Host) -> *const c_char {
    // SAFETY: upheld by the caller.
    let host = unsafe { &mut *host };
    host.refresh();
    host.state.as_ptr()
}

/// What to do with the video panel: 0 nothing, 1 show `*swapchain` (an IDXGISwapChain1* with
/// a reference the caller must release once the panel has adopted it), 2 show nothing.
///
/// # Safety
/// `host` must be live; `swapchain` a valid out-pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn bsv_host_panel(host: *mut Host, swapchain: *mut *mut c_void) -> c_int {
    // SAFETY: upheld by the caller.
    let host = unsafe { &mut *host };
    let change = match host.new_swapchain.lock().unwrap().take() {
        Some(new) => Some(PanelChange::Show(new)),
        None if host.clear_panel => Some(PanelChange::Clear),
        None => None,
    };
    host.clear_panel = false;
    match change {
        Some(PanelChange::Show(new)) => {
            // SAFETY: upheld by the caller.
            unsafe { *swapchain = new.into_raw() };
            1
        }
        Some(PanelChange::Clear) => 2,
        None => 0,
    }
}
