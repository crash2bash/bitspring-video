//! What the connected display can do, and how to drive it.
//!
//! Two sources are combined. The EDID is the display's own claim: which transfer functions it
//! accepts and the luminance range it wants content mapped to. The operating system adds what
//! is true right now: whether HDR output is switched on and how much headroom there is.

pub mod edid;
pub mod policy;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

pub use edid::Edid;

/// Whether HDR can reach this display right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HdrStatus {
    /// The display does not take an HDR signal.
    Unsupported,
    /// The display takes HDR, but the operating system is sending it SDR.
    Disabled,
    /// The operating system is sending HDR.
    Active,
}

/// Where a luminance figure came from, most to least trustworthy for its platform.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LuminanceSource {
    /// Reported by the operating system (on Windows this includes the user's HDR calibration).
    Os,
    /// The display's EDID HDR Static Metadata block.
    Edid,
    /// The display's Dolby Vision block.
    DolbyVision,
    /// Derived from the operating system's HDR headroom, assuming 100 cd/m² reference white.
    Headroom,
    /// Nothing known: a conservative default.
    Assumed,
    /// Set by the user, who knows the display better than it describes itself.
    User,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Luminance {
    pub peak_nits: f32,
    pub min_nits: Option<f32>,
    pub max_frame_avg_nits: Option<f32>,
    pub source: LuminanceSource,
}

/// Platform-specific numbers, kept for display to the user.
#[derive(Clone, Debug, PartialEq)]
pub enum OsReport {
    MacOs {
        /// Extended dynamic range headroom in use right now (1.0 = SDR only).
        edr_now: f32,
        /// Headroom the display could give in its current mode.
        edr_potential: f32,
        /// Headroom in a reference mode, 0 when the display has none.
        edr_reference: f32,
        color_profile: String,
    },
    Windows {
        /// The desktop colour space is HDR10 (BT.2020 PQ).
        hdr10_desktop: bool,
        sdr_white_nits: Option<f32>,
        bits_per_color: u32,
    },
    Unknown,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Display {
    /// CGDirectDisplayID on the Mac, HMONITOR on Windows.
    pub id: u64,
    pub name: String,
    pub builtin: bool,
    /// "HDMI", "DisplayPort", when known.
    pub connection: Option<String>,
    pub width: u32,
    pub height: u32,
    pub refresh_hz: f64,
    /// The panel covers clearly more than BT.709.
    pub wide_gamut: bool,
    pub hdr: HdrStatus,
    pub luminance: Luminance,
    pub edid: Option<Edid>,
    pub os: OsReport,
}

#[derive(Debug)]
pub enum Error {
    /// The Mac only answers screen queries on the main thread.
    NotMainThread,
    Os(String),
    Unsupported,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::NotMainThread => write!(f, "displays can only be queried from the main thread"),
            Error::Os(e) => write!(f, "{e}"),
            Error::Unsupported => write!(f, "display detection is not available on this system"),
        }
    }
}

impl std::error::Error for Error {}

/// Every connected display, the main one first.
pub fn displays() -> Result<Vec<Display>, Error> {
    #[cfg(target_os = "macos")]
    return macos::displays();
    #[cfg(windows)]
    return windows::displays();
    #[cfg(not(any(target_os = "macos", windows)))]
    Err(Error::Unsupported)
}

/// The display with this id (see [`Display::id`]).
pub fn display(id: u64) -> Result<Option<Display>, Error> {
    Ok(displays()?.into_iter().find(|d| d.id == id))
}

/// Inputs to [`resolve_luminance`], each optional.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct LuminanceHints {
    pub os_peak: Option<f32>,
    pub os_min: Option<f32>,
    pub os_frame_avg: Option<f32>,
    /// HDR headroom as a multiple of SDR white.
    pub headroom: Option<f32>,
}

/// Pick the luminance range to map content to, from the best source available.
pub(crate) fn resolve_luminance(edid: Option<&Edid>, hints: LuminanceHints) -> Luminance {
    let hdr = edid.and_then(|e| e.hdr);
    let dolby = edid.and_then(|e| e.dolby_vision);
    let edid_min = hdr.and_then(|h| h.min_nits);
    let edid_avg = hdr.and_then(|h| h.max_frame_avg_nits);

    if let Some(peak) = hints.os_peak.filter(|&p| p > 0.0) {
        return Luminance {
            peak_nits: peak,
            min_nits: hints.os_min.or(edid_min),
            max_frame_avg_nits: hints.os_frame_avg.or(edid_avg),
            source: LuminanceSource::Os,
        };
    }
    if let Some(peak) = hdr.and_then(|h| h.max_nits) {
        return Luminance {
            peak_nits: peak,
            min_nits: edid_min,
            max_frame_avg_nits: edid_avg,
            source: LuminanceSource::Edid,
        };
    }
    if let Some(peak) = dolby.and_then(|d| d.max_nits) {
        return Luminance {
            peak_nits: peak,
            min_nits: dolby.and_then(|d| d.min_nits),
            max_frame_avg_nits: edid_avg,
            source: LuminanceSource::DolbyVision,
        };
    }
    if let Some(headroom) = hints.headroom.filter(|&h| h > 1.0) {
        return Luminance {
            peak_nits: headroom * 100.0,
            min_nits: edid_min,
            max_frame_avg_nits: edid_avg,
            source: LuminanceSource::Headroom,
        };
    }
    Luminance {
        // HDR10's own fallback mastering peak: most content is graded at or below it.
        peak_nits: 1000.0,
        min_nits: edid_min,
        max_frame_avg_nits: edid_avg,
        source: LuminanceSource::Assumed,
    }
}

/// A panel whose red or green primary sits clearly outside BT.709.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn is_wide_gamut(edid: &Edid) -> bool {
    let c = edid.chromaticities;
    c.red.x > 0.66 || c.green.y > 0.63 || c.green.x < 0.28
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn os_figures_win_over_everything() {
        let l = resolve_luminance(
            None,
            LuminanceHints {
                os_peak: Some(1499.0),
                os_min: Some(0.01),
                headroom: Some(4.0),
                ..Default::default()
            },
        );
        assert_eq!(l.source, LuminanceSource::Os);
        assert_eq!(l.peak_nits, 1499.0);
        assert_eq!(l.min_nits, Some(0.01));
    }

    #[test]
    fn headroom_when_no_edid_luminance() {
        // A MacBook XDR panel: no HDR block in its EDID, sixteen times SDR white available.
        let l = resolve_luminance(
            None,
            LuminanceHints {
                headroom: Some(16.0),
                ..Default::default()
            },
        );
        assert_eq!(l.source, LuminanceSource::Headroom);
        assert_eq!(l.peak_nits, 1600.0);
    }

    #[test]
    fn nothing_known_is_flagged() {
        let l = resolve_luminance(None, LuminanceHints::default());
        assert_eq!(l.source, LuminanceSource::Assumed);
        assert_eq!(l.peak_nits, 1000.0);
    }
}
