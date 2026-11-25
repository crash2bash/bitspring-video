//! The native interface (`macos/*.swift`) as seen from Rust.
//!
//! Rust tells it the player's state as JSON a few times a second; it answers with short text
//! commands. The state's shape is mirrored by the `Status` struct in `macos/Model.swift`: keep
//! the two in step.

use std::ffi::c_void;
#[cfg(target_os = "macos")]
use std::ffi::{CStr, CString, c_char};
use std::sync::{Mutex, OnceLock};

use bsv_display::policy::{HdrMode, Primaries, SourceTransfer};
use bsv_display::{Display, HdrStatus, LuminanceSource, OsReport};
use bsv_engine::Transfer;
use serde::Serialize;

use crate::playback::{self, Command};
use crate::prefs::Preferences;
use crate::queue::QueueState;

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn bsv_app_prepare();
    fn bsv_ui_attach(
        parent: *mut c_void,
        callback: extern "C" fn(*const c_char, *mut c_void),
        user: *mut c_void,
    ) -> *mut c_void;
    fn bsv_ui_update(ui: *mut c_void, json: *const c_char);
    fn bsv_ui_perform(ui: *mut c_void, action: *const c_char);
    fn bsv_ui_detach(ui: *mut c_void);
}

/// Get ready for files opened from Finder. Call once, before the event loop starts.
pub fn prepare() {
    // SAFETY: registers an observer; no preconditions beyond being called on the main thread.
    #[cfg(target_os = "macos")]
    unsafe {
        bsv_app_prepare()
    };
}

/// Commands the interface has sent and nobody has taken yet.
static INBOX: Mutex<Vec<String>> = Mutex::new(Vec::new());
/// Wakes the event loop so a command is acted on at once.
static WAKE: OnceLock<Box<dyn Fn() + Send + Sync>> = OnceLock::new();

#[cfg(target_os = "macos")]
extern "C" fn receive(command: *const c_char, _user: *mut c_void) {
    // SAFETY: the interface passes a NUL-terminated string valid for this call.
    let text = unsafe { CStr::from_ptr(command) }
        .to_string_lossy()
        .into_owned();
    INBOX.lock().unwrap().push(text);
    if let Some(wake) = WAKE.get() {
        wake();
    }
}

/// What the interface asked for.
pub enum Request {
    Player(Command),
    /// Change a preference: its name and new value.
    Set(String, String),
    /// The controls appeared or went away.
    ControlsVisible(bool),
    /// Play this: a video, a folder, a playlist, or several of them. A subtitle file is
    /// shown with the video that is playing.
    Open(Vec<String>),
    /// Put these at the end of the list.
    Add(Vec<String>),
    /// The file after this one in the list, or the one before.
    Next,
    Previous,
    /// Play the file at this place in the list.
    Play(usize),
    /// Take the file at this place out of the list.
    Remove(usize),
    /// Move a file of the list from one place to another.
    Move(usize, usize),
    /// Write the list to this playlist file.
    Save(String),
    ToggleFullscreen,
    ExitFullscreen,
}

/// Read one command as the interface writes it: "toggle-pause", "seek-to 12.5", "open <path>".
/// Where a command takes several paths, each is on a line of its own.
pub fn parse(text: &str) -> Option<Request> {
    let (name, argument) = text.split_once(' ').unwrap_or((text, ""));
    let number = || argument.trim().parse::<f64>().ok();
    let place = || argument.trim().parse::<usize>().ok();
    let paths = || -> Option<Vec<String>> {
        let paths: Vec<String> = argument
            .lines()
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect();
        (!paths.is_empty()).then_some(paths)
    };
    Some(match name {
        "toggle-pause" => Request::Player(Command::TogglePause),
        "toggle-mute" => Request::Player(Command::ToggleMute),
        "seek-to" => Request::Player(Command::SeekTo(number()?)),
        "seek-by" => Request::Player(Command::SeekBy(number()?)),
        "volume" => Request::Player(Command::SetVolume(number()? as f32)),
        "volume-by" => Request::Player(Command::VolumeBy(number()? as f32)),
        "mode" => Request::Player(Command::SetHdrMode(match argument.trim() {
            "passthrough" => HdrMode::Passthrough,
            "sdr" => HdrMode::Sdr,
            _ => HdrMode::Auto,
        })),
        "audio-track" => Request::Player(Command::SelectAudio(number()? as i32)),
        "subtitle-track" => Request::Player(Command::SelectSubtitle(match argument.trim() {
            "off" => None,
            _ => Some(number()? as i32),
        })),
        "controls" => Request::ControlsVisible(argument.trim() == "1"),
        "set" => {
            let (key, value) = argument.split_once(' ').unwrap_or((argument, ""));
            Request::Set(key.to_string(), value.to_string())
        }
        "toggle-fullscreen" => Request::ToggleFullscreen,
        "exit-fullscreen" => Request::ExitFullscreen,
        "open" => Request::Open(paths()?),
        "queue-add" => Request::Add(paths()?),
        "next" => Request::Next,
        "previous" => Request::Previous,
        "queue-play" => Request::Play(place()?),
        "queue-remove" => Request::Remove(place()?),
        "queue-move" => {
            let (from, to) = argument.trim().split_once(' ')?;
            Request::Move(from.parse().ok()?, to.trim().parse().ok()?)
        }
        "queue-save" if !argument.is_empty() => Request::Save(argument.to_string()),
        _ => return None,
    })
}

/// Everything the interface asked for since the last call.
pub fn take_requests() -> Vec<Request> {
    let texts = std::mem::take(&mut *INBOX.lock().unwrap());
    texts.iter().filter_map(|t| parse(t)).collect()
}

/// The controls drawn over the picture. On the Mac that is the SwiftUI interface; elsewhere
/// there is none yet, and this stands in for it: the window's keys and title do the work.
pub struct Interface {
    #[cfg(target_os = "macos")]
    ptr: *mut c_void,
    #[cfg(target_os = "macos")]
    last: String,
}

impl Interface {
    /// Put the controls into a window, above everything already in `view`.
    ///
    /// # Safety
    /// `view` must be the live content view of a window, and this must be the main thread.
    #[cfg(target_os = "macos")]
    pub unsafe fn attach(view: *mut c_void, wake: impl Fn() + Send + Sync + 'static) -> Interface {
        let _ = WAKE.set(Box::new(wake));
        // SAFETY: upheld by the caller.
        let ptr = unsafe { bsv_ui_attach(view, receive, std::ptr::null_mut()) };
        Interface {
            ptr,
            last: String::new(),
        }
    }

    /// # Safety
    /// None to uphold here: there is nothing to attach.
    #[cfg(not(target_os = "macos"))]
    pub unsafe fn attach(_view: *mut c_void, wake: impl Fn() + Send + Sync + 'static) -> Interface {
        let _ = WAKE.set(Box::new(wake));
        Interface {}
    }

    /// Show this state. Does nothing when it is what the interface already has.
    #[cfg(target_os = "macos")]
    pub fn show(&mut self, state: &State) {
        let Ok(json) = serde_json::to_string(state) else {
            return;
        };
        if json == self.last {
            return;
        }
        if let Ok(c) = CString::new(json.as_str()) {
            // SAFETY: the string outlives the call; main thread, as `attach` required.
            unsafe { bsv_ui_update(self.ptr, c.as_ptr()) };
        }
        self.last = json;
    }

    #[cfg(not(target_os = "macos"))]
    pub fn show(&mut self, _state: &State) {}

    /// Interface-only actions for scripted runs: "panel", "settings", "pin".
    #[cfg(target_os = "macos")]
    pub fn perform(&self, action: &str) {
        if let Ok(c) = CString::new(action) {
            // SAFETY: as in `show`.
            unsafe { bsv_ui_perform(self.ptr, c.as_ptr()) };
        }
    }

    #[cfg(not(target_os = "macos"))]
    pub fn perform(&self, _action: &str) {}
}

#[cfg(target_os = "macos")]
impl Drop for Interface {
    fn drop(&mut self) {
        // SAFETY: we own the interface.
        unsafe { bsv_ui_detach(self.ptr) };
    }
}

// ---- The state, as the interface reads it -----------------------------------------------------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct State {
    file: Option<String>,
    error: Option<String>,
    position: f64,
    duration: f64,
    paused: bool,
    ended: bool,
    /// Where this file was picked up from, when it was not started from its beginning.
    resumed_from: Option<f64>,
    volume: f32,
    muted: bool,
    fullscreen: bool,
    mode: &'static str,
    video: Option<VideoState>,
    signal: Option<SignalState>,
    sound: Option<SoundState>,
    audio_tracks: Vec<TrackState>,
    subtitle_tracks: Vec<TrackState>,
    audio_track: Option<i32>,
    subtitle_track: Option<i32>,
    display: DisplayState,
    settings: SettingsState,
    /// What plays after what: the folder's videos, or a playlist. `None` when nothing is open.
    queue: Option<QueueState>,
}

/// The preferences the settings panel shows.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SettingsState {
    /// The user's own figure for the display's peak, when set.
    display_peak: Option<f32>,
    /// What the display itself reports.
    reported_peak: f32,
    quality: String,
    /// "off", "faithful", "restoring".
    upscaling: String,
    /// "off", "fast", "neural"; and which of the last two this program can do at all.
    interpolation: String,
    interpolations: Vec<&'static str>,
    hardware_decoding: bool,
    dolby_vision: bool,
    subtitle_scale: f32,
    subtitle_nits: f32,
    subtitle_language: String,
    audio_language: String,
    audio_delay: f32,
    auto_next: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TrackState {
    id: i32,
    codec: String,
    profile: String,
    language: String,
    title: String,
    layout: String,
    forced: bool,
    hearing_impaired: bool,
    pictures: bool,
}

fn track_states(tracks: &[bsv_engine::Track], kind: bsv_engine::TrackKind) -> Vec<TrackState> {
    tracks
        .iter()
        .filter(|t| t.kind == kind)
        .map(|t| TrackState {
            id: t.index,
            codec: t.codec.clone(),
            profile: t.profile.clone(),
            language: t.language.clone(),
            title: t.title.clone(),
            layout: t.layout.clone(),
            forced: t.forced,
            hearing_impaired: t.hearing_impaired,
            pictures: t.bitmap,
        })
        .collect()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct VideoState {
    codec: String,
    width: u32,
    height: u32,
    bit_depth: u32,
    fps: f64,
    primaries: &'static str,
    transfer: &'static str,
    mastering_min: Option<f32>,
    mastering_max: Option<f32>,
    max_cll: Option<f32>,
    dolby_vision: Option<String>,
    /// What a Dolby Vision file is without its RPU ("HDR10", "HLG", "SDR"), when it can be
    /// played that way: the interface then offers the choice.
    base_layer: Option<&'static str>,
    enhancement_layer: bool,
    hardware: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SignalState {
    dolby_vision: bool,
    hdr10_plus: bool,
    scene_peak: f32,
    output_transfer: &'static str,
    output_primaries: &'static str,
    output_bits: u32,
    output_peak: f32,
    passthrough: bool,
    /// The picture on screen was doubled by the neural network before being fitted to the window.
    upscaled: bool,
    /// Frames in between: "off", "on", "unsuited" (not for this video), "slow" (given up).
    in_between: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SoundState {
    codec: String,
    profile: String,
    layout: String,
    language: String,
    sample_rate: u32,
    channels: u32,
    device: String,
    device_rate: u32,
    device_channels: u16,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DisplayState {
    name: String,
    hdr: &'static str,
    peak: f32,
    min: Option<f32>,
    headroom: f32,
    peak_known: bool,
}

fn primaries(p: Primaries) -> &'static str {
    match p {
        Primaries::Bt709 => "BT.709",
        Primaries::DisplayP3 => "Display P3",
        Primaries::Bt2020 => "BT.2020",
    }
}

fn mode_name(mode: HdrMode) -> &'static str {
    match mode {
        HdrMode::Auto | HdrMode::ToneMap => "auto",
        HdrMode::Passthrough => "passthrough",
        HdrMode::Sdr => "sdr",
    }
}

/// What is open, for [`State::new`].
pub struct Playing<'a> {
    pub file_name: &'a str,
    pub status: &'a playback::Status,
    /// Where the file was picked up from, when it was seen before and not finished.
    pub resumed_from: Option<f64>,
}

/// Everything [`State::new`] draws on.
pub struct View<'a> {
    pub playing: Option<Playing<'a>>,
    /// The display as the player treats it (with the user's own peak, if set).
    pub display: &'a Display,
    /// The peak the display itself reports.
    pub reported_peak: f32,
    pub fullscreen: bool,
    pub error: Option<&'a str>,
    pub volume: f32,
    pub muted: bool,
    pub mode: HdrMode,
    pub prefs: &'a Preferences,
    pub queue: Option<&'a QueueState>,
}

impl State {
    pub fn new(view: View<'_>) -> State {
        let View {
            playing,
            display,
            reported_peak,
            fullscreen,
            error,
            volume,
            muted,
            mode,
            prefs,
            queue,
        } = view;
        let settings = SettingsState {
            display_peak: prefs.display_peak,
            reported_peak,
            quality: prefs.quality.clone(),
            upscaling: prefs.upscaling.clone(),
            interpolation: prefs.interpolation.clone(),
            interpolations: [
                ("fast", bsv_engine::Interpolation::System),
                ("neural", bsv_engine::Interpolation::Neural),
            ]
            .into_iter()
            .filter(|(_, method)| method.built())
            .map(|(name, _)| name)
            .collect(),
            hardware_decoding: prefs.hardware_decoding,
            dolby_vision: prefs.dolby_vision,
            subtitle_scale: prefs.subtitle_scale,
            subtitle_nits: prefs.subtitle_nits,
            subtitle_language: prefs.subtitle_language.clone(),
            audio_language: prefs.audio_language.clone(),
            audio_delay: prefs.audio_delay_ms,
            auto_next: prefs.auto_next,
        };
        let display_state = DisplayState {
            name: display.name.clone(),
            hdr: match display.hdr {
                HdrStatus::Active => "on",
                HdrStatus::Disabled => "off",
                HdrStatus::Unsupported => "unsupported",
            },
            peak: display.luminance.peak_nits,
            min: display.luminance.min_nits,
            headroom: match display.os {
                OsReport::MacOs { edr_now, .. } => edr_now,
                _ => 1.0,
            },
            peak_known: display.luminance.source != LuminanceSource::Assumed,
        };
        let Some(Playing {
            file_name,
            status,
            resumed_from,
        }) = playing
        else {
            return State {
                file: None,
                error: error.map(str::to_string),
                position: 0.0,
                duration: 0.0,
                paused: false,
                ended: false,
                resumed_from: None,
                volume,
                muted,
                fullscreen,
                mode: mode_name(mode),
                video: None,
                signal: None,
                sound: None,
                audio_tracks: Vec::new(),
                subtitle_tracks: Vec::new(),
                audio_track: None,
                subtitle_track: None,
                display: display_state,
                settings,
                queue: None,
            };
        };

        let video = status.info.as_ref().map(|info| VideoState {
            codec: info.codec.clone(),
            width: info.width,
            height: info.height,
            bit_depth: info.bit_depth,
            fps: info.fps,
            primaries: primaries(info.primaries),
            transfer: match info.transfer {
                SourceTransfer::Sdr => "SDR",
                SourceTransfer::Pq => "PQ",
                SourceTransfer::Hlg => "HLG",
            },
            mastering_min: info.mastering_nits.map(|(min, _)| min),
            mastering_max: info.mastering_nits.map(|(_, max)| max),
            max_cll: info.max_cll,
            dolby_vision: info.dolby_vision.map(|dv| {
                if dv.base_layer_compatibility != 0 {
                    format!("{}.{}", dv.profile, dv.base_layer_compatibility)
                } else {
                    dv.profile.to_string()
                }
            }),
            base_layer: info.dolby_vision.and_then(|dv| dv.base_layer()),
            enhancement_layer: info.dolby_vision.is_some_and(|dv| dv.enhancement_layer),
            hardware: info.hardware,
        });

        // Known once a frame has reached the screen.
        let stats = &status.stats;
        let signal = match (
            stats.output_transfer,
            stats.output_primaries,
            &status.output,
        ) {
            (Some(transfer), Some(out_primaries), Some(output)) => Some(SignalState {
                dolby_vision: stats.dolby_vision,
                hdr10_plus: stats.hdr10_plus,
                scene_peak: stats.scene_nits,
                output_transfer: match transfer {
                    Transfer::Sdr => "SDR",
                    Transfer::Pq => "PQ",
                    Transfer::Hlg => "HLG",
                },
                output_primaries: primaries(out_primaries),
                output_bits: stats.output_bits,
                output_peak: stats.output_nits.1,
                passthrough: output.passthrough && transfer != Transfer::Sdr,
                upscaled: stats.upscaled,
                in_between: match status.in_between {
                    playback::InBetween::Off => "off",
                    playback::InBetween::On => "on",
                    playback::InBetween::Unsuited => "unsuited",
                    playback::InBetween::TooSlow => "slow",
                },
            }),
            _ => None,
        };

        let sound = status.sound.as_ref().map(|s| SoundState {
            codec: s.track.codec.clone(),
            profile: s.track.profile.clone(),
            layout: s.track.layout.clone(),
            language: s.track.language.clone(),
            sample_rate: s.track.sample_rate,
            channels: s.track.channels,
            device: s.device.device.clone(),
            device_rate: s.device.sample_rate,
            device_channels: s.device.channels,
        });

        State {
            file: Some(file_name.to_string()),
            error: None,
            // Whole tenths: the interface is only told when the time it shows could change.
            position: (status.position * 10.0).floor() / 10.0,
            duration: status.info.as_ref().map_or(0.0, |i| i.duration),
            paused: status.paused,
            ended: status.ended,
            resumed_from,
            volume: status.volume,
            muted: status.muted,
            fullscreen,
            mode: mode_name(status.mode),
            video,
            signal,
            sound,
            audio_tracks: track_states(&status.tracks, bsv_engine::TrackKind::Audio),
            subtitle_tracks: track_states(&status.tracks, bsv_engine::TrackKind::Subtitle),
            audio_track: status.audio_track,
            subtitle_track: status.subtitle_track,
            display: display_state,
            settings,
            queue: queue.cloned(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_with_paths_take_one_to_a_line() {
        let Some(Request::Open(paths)) = parse("open /films/a b.mkv\n/films/c.mkv") else {
            panic!("not read as an open");
        };
        assert_eq!(paths, ["/films/a b.mkv", "/films/c.mkv"]);
        assert!(parse("open ").is_none());
        assert!(matches!(parse("queue-move 3 0"), Some(Request::Move(3, 0))));
        assert!(matches!(parse("queue-play 2"), Some(Request::Play(2))));
        assert!(parse("queue-play two").is_none());
        assert!(matches!(parse("next"), Some(Request::Next)));
    }
}
