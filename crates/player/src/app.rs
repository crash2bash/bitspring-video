//! The window. It owns the video layer and the native interface above it, starts a playback
//! session for each file, and carries messages between the two.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use bsv_display::policy::HdrMode;
use bsv_display::{Display, LuminanceSource, OsReport};
use bsv_engine::{Quality, Transfer};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalSize};
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::platform::macos::{MonitorHandleExtMacOS, WindowAttributesExtMacOS};
use winit::window::{Fullscreen, Theme, Window, WindowId};

use crate::playback::{Command, Options, Player};
use crate::prefs::Preferences;
use crate::report;
use crate::ui::{self, Interface, Playing, Request, State, View};

/// How often the display is asked again whether HDR is on (it can be switched while we play).
const DISPLAY_POLL: Duration = Duration::from_millis(1000);
/// How often the interface is told the player's state.
const INTERFACE_REFRESH: Duration = Duration::from_millis(100);

/// What the command line set, applied to every file opened in this run.
pub struct Settings {
    pub path: Option<String>,
    /// Where preferences are kept.
    pub preferences: PathBuf,
    /// Choices made on the command line: they hold for this run and are not saved.
    pub hardware: Option<bool>,
    pub mode: Option<HdrMode>,
    pub quality: Option<Quality>,
    pub audio_delay: Option<f64>,
    pub renderer_options: String,
    pub start: f64,
    pub sound: bool,
    /// Print the clock's state once a second.
    pub diagnostics: bool,
    pub script: Vec<Step>,
}

/// One timed command of a test script: "2:pause", "5:seek+10", "9:quit".
pub struct Step {
    pub at: f64,
    pub action: String,
}

pub fn parse_script(text: &str) -> Result<Vec<Step>, String> {
    text.split(',')
        .map(|part| {
            let (at, action) = part
                .split_once(':')
                .ok_or(format!("\"{part}\" is not time:command"))?;
            let at = at
                .trim()
                .parse()
                .map_err(|_| format!("\"{at}\" is not a time in seconds"))?;
            Ok(Step {
                at,
                action: action.trim().to_string(),
            })
        })
        .collect()
}

/// One file being played.
struct Session {
    player: Player,
    file_name: String,
    /// What was last printed, so each line appears only when it changes.
    printed_media: String,
    printed_sound: String,
    printed_path: String,
    printed_layer: String,
    printed_advice: Vec<String>,
    sized_to_video: bool,
    /// Whether the layer is currently marked as carrying HDR.
    layer_hdr: Option<bool>,
    /// Playback position against the system clock when playing began, to show drift from it.
    clock_origin: Option<(Instant, f64)>,
    underruns: u64,
}

struct Running {
    // Field order is drop order: playback stops before the interface, the layer and the window go.
    session: Option<Session>,
    interface: Interface,
    layer: raw_window_metal::Layer,
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
    // Carried from one file to the next.
    volume: f32,
    muted: bool,
    mode: HdrMode,
    quality: Quality,
    hardware: bool,
    audio_delay: f64,
}

struct App {
    settings: Settings,
    running: Option<Running>,
    started: Instant,
    last_display_poll: Instant,
    last_clock_report: Instant,
    failure: Option<String>,
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
    };
    // The interface wakes the loop when it has a command, so a click is acted on at once.
    WAKE.with(|wake| *wake.borrow_mut() = Some(proxy));
    event_loop.run_app(&mut app).map_err(|e| e.to_string())?;
    // The volume is remembered as it was left.
    if let Some(r) = &mut app.running
        && (r.saved.volume - r.volume).abs() > 0.001
    {
        r.saved.volume = r.volume;
        r.save_preferences();
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

// ---- The layer --------------------------------------------------------------------------------

/// An `NSString` for a short ASCII literal.
fn ns_string(text: &std::ffi::CStr) -> objc2::rc::Retained<objc2::runtime::NSObject> {
    // SAFETY: `stringWithUTF8String:` copies a valid NUL-terminated string.
    unsafe { objc2::msg_send![objc2::class!(NSString), stringWithUTF8String: text.as_ptr()] }
}

/// Tell Core Animation whether the layer's content goes above SDR white.
///
/// MoltenVK sets the older `wantsExtendedDynamicRangeContent` when the swapchain turns HDR, but
/// current macOS only gives a layer headroom once `preferredDynamicRange` says "high". Without
/// this the picture is quietly held at SDR brightness although every other setting says HDR.
fn set_layer_dynamic_range(layer: &raw_window_metal::Layer, hdr: bool) {
    use objc2::msg_send;
    use objc2::runtime::AnyObject;
    // SAFETY: the pointer is a live CAMetalLayer; the property is set only where it exists.
    unsafe {
        let obj = layer.as_ptr().as_ptr() as *mut AnyObject;
        let responds: bool =
            msg_send![obj, respondsToSelector: objc2::sel!(setPreferredDynamicRange:)];
        if responds {
            let value = ns_string(if hdr { c"high" } else { c"standard" });
            let _: () =
                msg_send![obj, setValue: &*value, forKey: &*ns_string(c"preferredDynamicRange")];
        }
    }
}

/// Show or hide the video layer. Hidden, the window's black shows instead of whatever frame a
/// finished session left behind.
fn set_layer_hidden(layer: &raw_window_metal::Layer, hidden: bool) {
    use objc2::msg_send;
    use objc2::runtime::AnyObject;
    // SAFETY: the pointer is a live CAMetalLayer; `hidden` is an ordinary layer property.
    unsafe {
        let obj = layer.as_ptr().as_ptr() as *mut AnyObject;
        let _: () = msg_send![obj, setHidden: hidden];
    }
}

/// What Core Animation has been told about our layer: the proof that HDR really is requested.
fn layer_state(layer: &raw_window_metal::Layer) -> String {
    use objc2::msg_send;
    use objc2::runtime::AnyObject;
    use std::ffi::{CStr, c_char, c_void};

    /// Opaque, but the Objective-C runtime checks its name on every message.
    #[repr(C)]
    struct CGColorSpace([u8; 0]);
    // SAFETY: matches the encoding of `CGColorSpaceRef`.
    unsafe impl objc2::encode::RefEncode for CGColorSpace {
        const ENCODING_REF: objc2::encode::Encoding =
            objc2::encode::Encoding::Pointer(&objc2::encode::Encoding::Struct("CGColorSpace", &[]));
    }

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGColorSpaceCopyName(space: *const CGColorSpace) -> *const c_void;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFStringGetCString(s: *const c_void, buf: *mut c_char, len: isize, encoding: u32) -> u8;
        fn CFRelease(cf: *const c_void);
    }

    // SAFETY: the pointer is a live CAMetalLayer and these are plain property reads.
    unsafe {
        let obj = layer.as_ptr().as_ptr() as *mut AnyObject;
        let edr: bool = msg_send![obj, wantsExtendedDynamicRangeContent];
        let metadata: *mut AnyObject = msg_send![obj, EDRMetadata];
        let space: *const CGColorSpace = msg_send![obj, colorspace];
        let mut name = String::from("none");
        if !space.is_null() {
            let cf = CGColorSpaceCopyName(space);
            if !cf.is_null() {
                let mut buf = [0 as c_char; 128];
                if CFStringGetCString(cf, buf.as_mut_ptr(), buf.len() as isize, 0x0800_0100) != 0 {
                    name = CStr::from_ptr(buf.as_ptr()).to_string_lossy().into_owned();
                }
                CFRelease(cf);
            }
        }
        let name = name.trim_start_matches("kCGColorSpace");
        let mut text = format!(
            "colour space {name}, extended range {}",
            if edr { "requested" } else { "off" }
        );
        if !metadata.is_null() {
            // "CAEDRMetadata <0x...> display min nits: 0.000200, max nits: 1316.604370"
            let description: objc2::rc::Retained<objc2::runtime::NSObject> =
                msg_send![metadata, debugDescription];
            let c: *const c_char = msg_send![&*description, UTF8String];
            let description = CStr::from_ptr(c).to_string_lossy();
            if let Some((_, range)) = description.split_once("display ") {
                text += &format!(", HDR metadata: {range}");
            }
        }
        text
    }
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

/// The display the window is on, falling back to the main one.
fn display_of(window: &Window) -> Option<Display> {
    let all = bsv_display::displays().ok()?;
    let id = window.current_monitor().map(|m| m.native_id() as u64);
    all.iter()
        .find(|d| Some(d.id) == id)
        .cloned()
        .or_else(|| all.into_iter().next())
}

// ---- The application --------------------------------------------------------------------------

impl Running {
    /// Play `path`, replacing whatever is playing. `start` is where to begin, in seconds.
    fn open(&mut self, path: &str, settings: &Settings, start: f64) {
        // The old session must be gone before a new renderer takes the layer.
        self.session = None;
        self.error = None;
        // The last file's final frame is still on the layer: keep it out of sight until this
        // file's first frame replaces it.
        set_layer_hidden(&self.layer, true);

        let file_name = Path::new(path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string());
        println!("File:    {path}");
        let size = self.window.inner_size();
        let options = Options {
            path: path.to_string(),
            hardware: self.hardware,
            mode: self.mode,
            quality: self.quality,
            renderer_options: settings.renderer_options.clone(),
            start,
            sound: settings.sound,
            audio_delay: self.audio_delay,
            volume: self.volume,
            muted: self.muted,
            subtitle_lift: self.subtitle_lift(),
            subtitle_scale: self.prefs.subtitle_scale,
            subtitle_nits: self.prefs.subtitle_nits,
            audio_language: self.prefs.audio_language.clone(),
            subtitle_language: self.prefs.subtitle_language.clone(),
        };
        // SAFETY: `Running` drops the session before the layer, and `open` drops the previous
        // session before starting another on the same layer.
        let player = unsafe {
            Player::start(
                self.layer.as_ptr().as_ptr(),
                (size.width, size.height),
                self.shown_display(),
                options,
            )
        };
        self.window.set_title(&file_name);
        self.session = Some(Session {
            player,
            file_name,
            printed_media: String::new(),
            printed_sound: String::new(),
            printed_path: String::new(),
            printed_layer: String::new(),
            printed_advice: Vec::new(),
            sized_to_video: false,
            layer_hdr: None,
            clock_origin: None,
            underruns: 0,
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

    fn toggle_fullscreen(&self) {
        let next = if self.window.fullscreen().is_some() {
            None
        } else {
            Some(Fullscreen::Borderless(None))
        };
        self.window.set_fullscreen(next);
    }

    fn send(&self, command: Command) {
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
        self.save_preferences();
        match key {
            "displayPeak" => self.send(Command::Display(Box::new(self.shown_display()))),
            "quality" => {
                self.quality = self.prefs.render_quality();
                self.send(Command::SetQuality(self.quality));
            }
            "hardwareDecoding" => self.hardware = self.prefs.hardware_decoding,
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
            Request::Open(path) => self.open(&path, settings, 0.0),
            Request::ToggleFullscreen => self.toggle_fullscreen(),
            Request::ExitFullscreen => self.window.set_fullscreen(None),
        }
    }

    /// Look at the playback state: print what changed, keep the layer and the title in step,
    /// and tell the interface.
    fn refresh(&mut self) {
        let mut failed = None;
        let shown = self.shown_display();
        if let Some(session) = &mut self.session {
            let status = session.player.status();
            if let Some(e) = &status.error {
                failed = Some(format!("{}: {e}", session.file_name));
            }
            self.volume = status.volume;
            self.muted = status.muted;
            self.mode = status.mode;

            if let Some(info) = &status.info {
                // Printed again when the first decoded frame adds what the container did not say.
                let media = report::media(info);
                if media != session.printed_media {
                    println!("Video:   {media}");
                    session.printed_media = media;
                }
                let sound = match &status.sound {
                    Some(s) => report::sound_path(&s.track, &s.device),
                    None => "none (the picture runs on the system clock)".to_string(),
                };
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
                        let fit = (max_w / info.width as f64)
                            .min(max_h / info.height as f64)
                            .min(scale.max(1.0));
                        let size = PhysicalSize::new(
                            (info.width as f64 * fit) as u32,
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
                            set_layer_hidden(&self.layer, false);
                        }
                        set_layer_dynamic_range(&self.layer, hdr);
                        session.layer_hdr = Some(hdr);
                    }
                    let layer = layer_state(&self.layer);
                    if layer != session.printed_layer {
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
                    status: &status,
                }),
                display: &shown,
                reported_peak: self.display.luminance.peak_nits,
                fullscreen: self.window.fullscreen().is_some(),
                error: None,
                volume: status.volume,
                muted: status.muted,
                mode: status.mode,
                prefs: &self.prefs,
            });
            self.interface.show(&state);
        }

        if let Some(error) = failed {
            eprintln!("bitspring-video: {error}");
            self.session = None;
            set_layer_hidden(&self.layer, true);
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
            .with_theme(Some(Theme::Dark))
            // The picture fills the whole window; the title bar's buttons float over it.
            .with_titlebar_transparent(true)
            .with_fullsize_content_view(true)
            .with_title_hidden(true);
        let window = event_loop
            .create_window(attributes)
            .map_err(|e| e.to_string())?;

        let handle = window.window_handle().map_err(|e| e.to_string())?;
        let RawWindowHandle::AppKit(appkit) = handle.as_raw() else {
            return Err("not an AppKit window".into());
        };
        // SAFETY: the view belongs to `window`, which outlives the layer (see `Running`).
        let layer = unsafe { raw_window_metal::Layer::from_ns_view(appkit.ns_view) };
        // SAFETY: the same view, on the main thread; the interface is dropped before the window.
        let interface = unsafe {
            Interface::attach(appkit.ns_view.as_ptr(), || {
                WAKE.with(|wake| {
                    if let Some(proxy) = wake.borrow().as_ref() {
                        let _ = proxy.send_event(());
                    }
                })
            })
        };

        let display = display_of(&window).ok_or("no display found")?;
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
            layer,
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
            saved,
            prefs,
            preferences_path: self.settings.preferences.clone(),
        };
        if let Some(path) = self.settings.path.clone() {
            running.open(&path, &self.settings, self.settings.start);
        }
        running.refresh();
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
                "panel" | "pin" | "settings" => r.interface.perform(&step.action),
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
                a if a.starts_with("open ") => r.open(a[5..].trim(), &self.settings, 0.0),
                other => eprintln!("[warn] unknown script command \"{other}\""),
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
            None => session.clock_origin = Some((Instant::now(), status.position)),
            Some((since, from)) => {
                let drift = (status.position - from) - since.elapsed().as_secs_f64();
                println!(
                    "Clock:   {:.3} s, {:+.1} ms against the system clock",
                    status.position,
                    drift * 1000.0
                );
            }
        }
    }

    fn poll_display(&mut self) {
        let Some(r) = &mut self.running else { return };
        let Some(display) = display_of(&r.window) else {
            return;
        };
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
        let Some(r) = &self.running else { return };
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => r.send(Command::Resize(size.width, size.height)),
            WindowEvent::Moved(_) | WindowEvent::ScaleFactorChanged { .. } => {
                // Possibly on another display now: look again right away.
                self.last_display_poll = Instant::now() - DISPLAY_POLL;
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
            r.refresh();
        }
        let wait = if self.settings.script.is_empty() {
            INTERFACE_REFRESH
        } else {
            Duration::from_millis(20)
        };
        event_loop.set_control_flow(ControlFlow::WaitUntil(Instant::now() + wait));
    }
}
