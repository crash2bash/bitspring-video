//! What the user has chosen, kept between runs in a small JSON file.

use std::path::{Path, PathBuf};

use bsv_display::policy::HdrMode;
use bsv_engine::Quality;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Preferences {
    /// Who fits HDR to the screen: "auto" (the player), "passthrough" (the display), "sdr".
    pub hdr_mode: String,
    /// The display's peak in cd/m² as the user set it; `None` to believe the display.
    pub display_peak: Option<f32>,
    /// "high", "balanced", "fast".
    pub quality: String,
    pub hardware_decoding: bool,
    /// For a file that is both Dolby Vision and a plain signal (HDR10 for profile 8.1): play
    /// the Dolby Vision version. `false`: play the plain one.
    pub dolby_vision: bool,
    /// Subtitle text size, 1.0 as styled.
    pub subtitle_scale: f32,
    /// How bright subtitle white is, cd/m².
    pub subtitle_nits: f32,
    /// ISO 639 code of the language to show subtitles in; empty for forced subtitles only.
    pub subtitle_language: String,
    /// ISO 639 code of the language to play sound in; empty for the file's own choice.
    pub audio_language: String,
    /// Milliseconds to hold the picture back, for a TV or receiver that delays sound.
    pub audio_delay_ms: f32,
    /// 0.0 to 1.0.
    pub volume: f32,
    /// When an episode ends, the next one in its folder starts by itself, and a playlist
    /// goes on to its next file.
    pub auto_next: bool,
}

impl Default for Preferences {
    fn default() -> Preferences {
        Preferences {
            hdr_mode: "auto".into(),
            display_peak: None,
            quality: "high".into(),
            hardware_decoding: true,
            dolby_vision: true,
            subtitle_scale: 1.0,
            subtitle_nits: 203.0,
            subtitle_language: String::new(),
            audio_language: String::new(),
            audio_delay_ms: 0.0,
            volume: 1.0,
            auto_next: true,
        }
    }
}

/// Limits of what can be set, shared with the interface's sliders (see `Settings` in
/// `macos/Overlay.swift`).
pub const DISPLAY_PEAK: (f32, f32) = (200.0, 4000.0);
pub const SUBTITLE_SCALE: (f32, f32) = (0.5, 2.0);
pub const SUBTITLE_NITS: (f32, f32) = (60.0, 400.0);
pub const AUDIO_DELAY_MS: (f32, f32) = (-200.0, 500.0);

impl Preferences {
    /// Where preferences live unless the command line says otherwise.
    pub fn default_path() -> PathBuf {
        #[cfg(target_os = "macos")]
        let base =
            std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Application Support"));
        #[cfg(windows)]
        let base = std::env::var_os("APPDATA").map(PathBuf::from);
        #[cfg(not(any(target_os = "macos", windows)))]
        let base = std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config"));
        base.unwrap_or_else(|| PathBuf::from("."))
            .join("BitSpring Video")
            .join("settings.json")
    }

    /// Read the file. A missing or damaged file gives the defaults; a missing field, its default.
    pub fn load(path: &Path) -> Preferences {
        let mut prefs: Preferences = std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        prefs.tidy();
        prefs
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(folder) = path.parent() {
            std::fs::create_dir_all(folder)?;
        }
        let text = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(path, text + "\n")
    }

    /// Bring every value inside its limits, whatever the file or the interface said.
    fn tidy(&mut self) {
        if !["auto", "passthrough", "sdr"].contains(&self.hdr_mode.as_str()) {
            self.hdr_mode = "auto".into();
        }
        if !["high", "balanced", "fast"].contains(&self.quality.as_str()) {
            self.quality = "high".into();
        }
        let within = |v: f32, (low, high): (f32, f32), fallback: f32| {
            if v.is_finite() {
                v.clamp(low, high)
            } else {
                fallback
            }
        };
        self.display_peak = self
            .display_peak
            .filter(|p| p.is_finite())
            .map(|p| p.clamp(DISPLAY_PEAK.0, DISPLAY_PEAK.1));
        self.subtitle_scale = within(self.subtitle_scale, SUBTITLE_SCALE, 1.0);
        self.subtitle_nits = within(self.subtitle_nits, SUBTITLE_NITS, 203.0);
        self.audio_delay_ms = within(self.audio_delay_ms, AUDIO_DELAY_MS, 0.0);
        self.volume = within(self.volume, (0.0, 1.0), 1.0);
    }

    /// Change one preference by its name in the file. Returns whether anything changed.
    pub fn set(&mut self, key: &str, value: &str) -> bool {
        let before = self.clone();
        let number = value.trim().parse::<f32>().ok();
        match key {
            "hdrMode" => self.hdr_mode = value.trim().to_string(),
            "displayPeak" => self.display_peak = number,
            "quality" => self.quality = value.trim().to_string(),
            "hardwareDecoding" => self.hardware_decoding = matches!(value.trim(), "1" | "true"),
            "dolbyVision" => self.dolby_vision = matches!(value.trim(), "1" | "true"),
            "subtitleScale" => self.subtitle_scale = number.unwrap_or(1.0),
            "subtitleNits" => self.subtitle_nits = number.unwrap_or(203.0),
            "subtitleLanguage" => self.subtitle_language = value.trim().to_string(),
            "audioLanguage" => self.audio_language = value.trim().to_string(),
            "audioDelay" => self.audio_delay_ms = number.unwrap_or(0.0),
            "volume" => self.volume = number.unwrap_or(1.0),
            "autoNext" => self.auto_next = matches!(value.trim(), "1" | "true"),
            _ => return false,
        }
        self.tidy();
        *self != before
    }

    pub fn mode(&self) -> HdrMode {
        match self.hdr_mode.as_str() {
            "passthrough" => HdrMode::Passthrough,
            "sdr" => HdrMode::Sdr,
            _ => HdrMode::Auto,
        }
    }

    pub fn set_mode(&mut self, mode: HdrMode) {
        self.hdr_mode = match mode {
            HdrMode::Auto | HdrMode::ToneMap => "auto",
            HdrMode::Passthrough => "passthrough",
            HdrMode::Sdr => "sdr",
        }
        .into();
    }

    pub fn render_quality(&self) -> Quality {
        match self.quality.as_str() {
            "balanced" => Quality::Balanced,
            "fast" => Quality::Fast,
            _ => Quality::High,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_are_kept_inside_their_limits() {
        let mut p = Preferences::default();
        assert!(p.set("displayPeak", "90000"));
        assert_eq!(p.display_peak, Some(4000.0));
        assert!(p.set("subtitleScale", "0.1"));
        assert_eq!(p.subtitle_scale, 0.5);
        assert!(p.set("audioDelay", "-900"));
        assert_eq!(p.audio_delay_ms, -200.0);
    }

    #[test]
    fn the_display_peak_can_go_back_to_automatic() {
        let mut p = Preferences::default();
        p.set("displayPeak", "1500");
        assert!(p.set("displayPeak", "auto"));
        assert_eq!(p.display_peak, None);
    }

    #[test]
    fn nonsense_falls_back_instead_of_sticking() {
        let mut p = Preferences::default();
        p.set("quality", "ludicrous");
        assert_eq!(p.quality, "high");
        p.set("hdrMode", "");
        assert_eq!(p.hdr_mode, "auto");
        assert!(!p.set("noSuchThing", "1"));
    }

    #[test]
    fn setting_the_same_value_is_not_a_change() {
        let mut p = Preferences::default();
        assert!(!p.set("quality", "high"));
        assert!(p.set("quality", "fast"));
    }

    #[test]
    fn a_file_from_an_older_version_fills_in_what_it_lacks() {
        let p: Preferences =
            serde_json::from_str(r#"{"quality":"fast","somethingRetired":3}"#).unwrap();
        assert_eq!(p.quality, "fast");
        assert_eq!(p.subtitle_nits, 203.0);
        assert!(p.hardware_decoding);
    }

    #[test]
    fn saved_preferences_read_back_the_same() {
        let path = std::env::temp_dir().join(format!("bsv-prefs-test-{}.json", std::process::id()));
        let mut p = Preferences::default();
        p.set("displayPeak", "1100");
        p.set("subtitleLanguage", "rus");
        p.save(&path).unwrap();
        assert_eq!(Preferences::load(&path), p);
        let _ = std::fs::remove_file(&path);
    }
}
