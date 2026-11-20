//! Decide what to send to the display for a given video.
//!
//! The rule the player follows: the picture is mapped to what the display can really show, in
//! the player, using every piece of metadata the file carries. A display reached over HDMI as
//! HDR10 never sees Dolby Vision or HDR10+ dynamic metadata, so only the player can apply it.

use crate::{Display, HdrStatus, LuminanceSource};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Primaries {
    Bt709,
    DisplayP3,
    Bt2020,
}

/// How the source encodes light.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceTransfer {
    Sdr,
    Pq,
    Hlg,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Source {
    pub primaries: Primaries,
    pub transfer: SourceTransfer,
    /// Brightest level the content claims (MaxCLL, else the mastering display's peak), cd/m².
    pub peak_nits: Option<f32>,
    pub dolby_vision: bool,
}

impl Source {
    pub fn is_hdr(&self) -> bool {
        self.transfer != SourceTransfer::Sdr
    }
}

/// The user's choice of who maps HDR to the display.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HdrMode {
    /// The player decides: currently the same as [`HdrMode::ToneMap`].
    #[default]
    Auto,
    /// The player maps to the display's range and sends a signal that fits it.
    ToneMap,
    /// The source's own metadata is sent on and the display does the mapping.
    Passthrough,
    /// Always send SDR, even to an HDR display.
    Sdr,
}

impl HdrMode {
    pub fn next(self) -> HdrMode {
        match self {
            HdrMode::Auto => HdrMode::ToneMap,
            HdrMode::ToneMap => HdrMode::Passthrough,
            HdrMode::Passthrough => HdrMode::Sdr,
            HdrMode::Sdr => HdrMode::Auto,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            HdrMode::Auto => "auto",
            HdrMode::ToneMap => "tone-map in player",
            HdrMode::Passthrough => "passthrough",
            HdrMode::Sdr => "SDR",
        }
    }
}

/// The signal sent to the display.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Signal {
    /// SDR, sRGB-encoded, in these primaries. The operating system maps it to the panel.
    Sdr(Primaries),
    /// HDR10: BT.2020 primaries, PQ.
    Hdr10,
}

/// Something the user should know or can fix.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Advice {
    /// The display takes HDR but the system has it switched off.
    EnableHdrInSystem,
    /// No luminance data for this display: its peak is a guess.
    DisplayPeakUnknown,
    /// The content is brighter than the display; highlights are being compressed.
    Compressing {
        content_nits: f32,
        display_nits: f32,
    },
    /// An HDR video on a display that cannot show HDR at all.
    HdrOnSdrDisplay,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Output {
    pub signal: Signal,
    /// Luminance range the picture is mapped to, cd/m². Unused for SDR.
    pub peak_nits: f32,
    pub min_nits: Option<f32>,
    /// Send the source's metadata unchanged and leave the mapping to the display.
    pub passthrough: bool,
    pub advice: Vec<Advice>,
}

pub fn plan(display: &Display, source: &Source, mode: HdrMode) -> Output {
    let sdr_primaries = if display.wide_gamut && source.primaries != Primaries::Bt709 {
        Primaries::DisplayP3
    } else {
        Primaries::Bt709
    };
    let sdr = |advice: Vec<Advice>| Output {
        signal: Signal::Sdr(sdr_primaries),
        peak_nits: 0.0,
        min_nits: None,
        passthrough: false,
        advice,
    };

    if !source.is_hdr() {
        // SDR stays SDR even on an HDR display: stretching it is not what was graded.
        return sdr(Vec::new());
    }
    if mode == HdrMode::Sdr {
        return sdr(Vec::new());
    }
    match display.hdr {
        HdrStatus::Unsupported => return sdr(vec![Advice::HdrOnSdrDisplay]),
        HdrStatus::Disabled => return sdr(vec![Advice::EnableHdrInSystem]),
        HdrStatus::Active => {}
    }

    let lum = display.luminance;
    let mut advice = Vec::new();
    if lum.source == LuminanceSource::Assumed {
        advice.push(Advice::DisplayPeakUnknown);
    }
    let passthrough = mode == HdrMode::Passthrough;
    if !passthrough && let Some(content) = source.peak_nits {
        // A few percent over is inside the tolerance of the metadata itself.
        if content > lum.peak_nits * 1.05 {
            advice.push(Advice::Compressing {
                content_nits: content,
                display_nits: lum.peak_nits,
            });
        }
    }
    Output {
        signal: Signal::Hdr10,
        peak_nits: lum.peak_nits,
        min_nits: lum.min_nits,
        passthrough,
        advice,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Luminance, OsReport};

    fn display(hdr: HdrStatus, wide_gamut: bool, peak: f32, source: LuminanceSource) -> Display {
        Display {
            id: 1,
            name: "Test".into(),
            builtin: false,
            connection: None,
            width: 3840,
            height: 2160,
            refresh_hz: 120.0,
            wide_gamut,
            hdr,
            luminance: Luminance {
                peak_nits: peak,
                min_nits: Some(0.0005),
                max_frame_avg_nits: None,
                source,
            },
            edid: None,
            os: OsReport::Unknown,
        }
    }

    const UHD_BLURAY: Source = Source {
        primaries: Primaries::Bt2020,
        transfer: SourceTransfer::Pq,
        peak_nits: Some(4000.0),
        dolby_vision: false,
    };
    const BLURAY: Source = Source {
        primaries: Primaries::Bt709,
        transfer: SourceTransfer::Sdr,
        peak_nits: None,
        dolby_vision: false,
    };

    #[test]
    fn hdr_on_an_active_hdr_display_is_mapped_to_its_peak() {
        let d = display(HdrStatus::Active, true, 1316.0, LuminanceSource::Edid);
        let out = plan(&d, &UHD_BLURAY, HdrMode::Auto);
        assert_eq!(out.signal, Signal::Hdr10);
        assert_eq!(out.peak_nits, 1316.0);
        assert!(!out.passthrough);
        assert_eq!(
            out.advice,
            vec![Advice::Compressing {
                content_nits: 4000.0,
                display_nits: 1316.0
            }]
        );
    }

    #[test]
    fn content_within_the_display_range_needs_no_advice() {
        let d = display(HdrStatus::Active, true, 1316.0, LuminanceSource::Edid);
        let src = Source {
            peak_nits: Some(1000.0),
            ..UHD_BLURAY
        };
        assert!(plan(&d, &src, HdrMode::Auto).advice.is_empty());
    }

    #[test]
    fn hdr_switched_off_in_the_system_falls_back_to_sdr_and_says_so() {
        let d = display(HdrStatus::Disabled, true, 1316.0, LuminanceSource::Edid);
        let out = plan(&d, &UHD_BLURAY, HdrMode::Auto);
        assert_eq!(out.signal, Signal::Sdr(Primaries::DisplayP3));
        assert_eq!(out.advice, vec![Advice::EnableHdrInSystem]);
    }

    #[test]
    fn sdr_is_never_inflated() {
        let d = display(HdrStatus::Active, true, 1316.0, LuminanceSource::Edid);
        let out = plan(&d, &BLURAY, HdrMode::Auto);
        assert_eq!(out.signal, Signal::Sdr(Primaries::Bt709));
        assert!(out.advice.is_empty());
    }

    #[test]
    fn passthrough_keeps_the_source_metadata() {
        let d = display(HdrStatus::Active, true, 600.0, LuminanceSource::Edid);
        let out = plan(&d, &UHD_BLURAY, HdrMode::Passthrough);
        assert!(out.passthrough);
        assert!(out.advice.is_empty());
    }

    #[test]
    fn an_unknown_peak_is_reported() {
        let d = display(HdrStatus::Active, false, 1000.0, LuminanceSource::Assumed);
        let src = Source {
            peak_nits: Some(1000.0),
            ..UHD_BLURAY
        };
        assert_eq!(
            plan(&d, &src, HdrMode::Auto).advice,
            vec![Advice::DisplayPeakUnknown]
        );
    }

    #[test]
    fn forced_sdr_on_a_narrow_gamut_display() {
        let d = display(HdrStatus::Active, false, 1000.0, LuminanceSource::Edid);
        assert_eq!(
            plan(&d, &UHD_BLURAY, HdrMode::Sdr).signal,
            Signal::Sdr(Primaries::Bt709)
        );
    }
}
