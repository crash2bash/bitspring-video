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

use bitspring_video::playback::{Command, Options, Player};
use bitspring_video::prefs::Preferences;
use bitspring_video::report;
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
}

pub struct Host {
    session: Option<Session>,
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
            display,
            monitor: 0,
            last_display_poll: Instant::now(),
            error: None,
            volume: saved.volume,
            muted: false,
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

    fn send(&self, command: Command) {
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
        *self.new_swapchain.lock().unwrap() = None;
        self.clear_panel = true;
    }

    fn open(&mut self, path: &str) {
        self.close();
        self.error = None;
        let file_name = Path::new(path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string());
        bsv_engine::log_line(&format!("File:    {path}"));
        let options = Options {
            path: path.to_string(),
            hardware: self.prefs.hardware_decoding,
            mode: self.mode,
            dolby_vision: self.prefs.dolby_vision,
            quality: self.prefs.render_quality(),
            renderer_options: String::new(),
            start: 0.0,
            sound: self.sound,
            audio_delay: self.prefs.audio_delay_ms as f64 / 1000.0,
            volume: self.volume,
            muted: self.muted,
            subtitle_lift: self.subtitle_lift(),
            subtitle_scale: self.prefs.subtitle_scale,
            subtitle_nits: self.prefs.subtitle_nits,
            audio_language: self.prefs.audio_language.clone(),
            subtitle_language: self.prefs.subtitle_language.clone(),
        };
        let slot = self.new_swapchain.clone();
        let player =
            Player::start_composed(self.size, self.shown_display(), options, move |swapchain| {
                // SAFETY: the renderer has just made this swapchain and holds it for the session.
                *slot.lock().unwrap() = Some(unsafe { ComRef::adopt(swapchain) });
            });
        player.send(Command::Scale(self.scale));
        self.clear_panel = false;
        self.session = Some(Session {
            player,
            file_name,
            logged_media: String::new(),
            logged_signal: String::new(),
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
            Some(Request::Open(path)) => self.open(&path),
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
                }),
                display: &shown,
                reported_peak: self.display.luminance.peak_nits,
                fullscreen: self.fullscreen,
                error: None,
                volume: status.volume,
                muted: status.muted,
                mode: status.mode,
                prefs: &self.prefs,
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
            });
            json = serde_json::to_string(&state).ok();
        }
        self.state = json.and_then(|j| CString::new(j).ok()).unwrap_or_default();
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        self.session = None;
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
