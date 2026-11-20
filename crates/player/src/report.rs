//! Plain-text descriptions of the display, the video and the path between them.

use bsv_display::policy::{Advice, HdrMode, Output, Primaries, Signal, SourceTransfer};
use bsv_display::{Display, HdrStatus, LuminanceSource, OsReport};
use bsv_engine::{AudioInfo, MediaInfo, RenderStats, Track, TrackKind, Transfer};

pub fn primaries(p: Primaries) -> &'static str {
    match p {
        Primaries::Bt709 => "BT.709",
        Primaries::DisplayP3 => "Display P3",
        Primaries::Bt2020 => "BT.2020",
    }
}

fn nits(v: f32) -> String {
    if v >= 10.0 {
        format!("{v:.0}")
    } else if v >= 0.1 {
        format!("{v:.2}")
    } else {
        format!("{v:.4}")
    }
}

/// "1:02:03" or "2:03".
#[cfg_attr(target_os = "macos", allow(dead_code))]
pub fn clock(seconds: f64) -> String {
    let s = seconds.max(0.0) as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

pub fn display(d: &Display) -> String {
    let mut out = String::new();
    let connection = d
        .connection
        .as_deref()
        .map(|c| format!(", {c}"))
        .unwrap_or_default();
    let builtin = if d.builtin { ", built-in" } else { "" };
    out += &format!(
        "{} ({}x{} @ {:.0} Hz{connection}{builtin})\n",
        d.name, d.width, d.height, d.refresh_hz
    );

    let hdr = match d.hdr {
        HdrStatus::Active => "on",
        HdrStatus::Disabled => "supported, switched off in the system",
        HdrStatus::Unsupported => "not supported",
    };
    out += &format!("  HDR:        {hdr}\n");
    out += &format!(
        "  Gamut:      {}\n",
        if d.wide_gamut {
            "wide (P3 or beyond)"
        } else {
            "standard (sRGB)"
        }
    );

    if d.hdr != HdrStatus::Unsupported {
        let source = match d.luminance.source {
            LuminanceSource::Os => "reported by the system",
            LuminanceSource::Edid => "from the display's EDID",
            LuminanceSource::DolbyVision => "from the display's Dolby Vision block",
            LuminanceSource::Headroom => "estimated from the system's HDR headroom",
            LuminanceSource::Assumed => "unknown, assumed",
            LuminanceSource::User => "set in the player's settings",
        };
        let mut line = format!("  Luminance:  peak {} cd/m²", nits(d.luminance.peak_nits));
        if let Some(avg) = d.luminance.max_frame_avg_nits {
            line += &format!(", full-screen {}", nits(avg));
        }
        if let Some(min) = d.luminance.min_nits {
            line += &format!(", black {}", nits(min));
        }
        out += &format!("{line} ({source})\n");
    }

    if let Some(edid) = &d.edid {
        let mut formats = Vec::new();
        if let Some(h) = edid.hdr {
            if h.pq {
                formats.push("HDR10 (PQ)".to_string());
            }
            if h.hlg {
                formats.push("HLG".to_string());
            }
        }
        if edid.hdr10_plus {
            formats.push("HDR10+".to_string());
        }
        if let Some(dv) = edid.dolby_vision {
            formats.push(format!("Dolby Vision (block v{})", dv.version));
        }
        if formats.is_empty() {
            formats.push("SDR only".to_string());
        }
        out += &format!("  Accepts:    {}\n", formats.join(", "));
        let c = edid.chromaticities;
        out += &format!(
            "  Primaries:  R {:.3}/{:.3}  G {:.3}/{:.3}  B {:.3}/{:.3}  W {:.4}/{:.4}\n",
            c.red.x, c.red.y, c.green.x, c.green.y, c.blue.x, c.blue.y, c.white.x, c.white.y
        );
    }

    match &d.os {
        OsReport::MacOs {
            edr_now,
            edr_potential,
            edr_reference,
            color_profile,
        } => {
            out += &format!(
                "  macOS:      EDR headroom {edr_now:.2} now, {edr_potential:.2} available, {edr_reference:.2} reference; profile \"{color_profile}\"\n"
            );
        }
        OsReport::Windows {
            hdr10_desktop,
            sdr_white_nits,
            bits_per_color,
        } => {
            let white = sdr_white_nits
                .map(|w| format!(", SDR white {} cd/m²", nits(w)))
                .unwrap_or_default();
            out += &format!(
                "  Windows:    desktop is {}{white}, {bits_per_color} bits per colour\n",
                if *hdr10_desktop { "HDR10" } else { "SDR" }
            );
        }
        OsReport::Unknown => {}
    }
    out
}

/// "HEVC 3840x2160 10-bit, BT.2020 PQ, mastered 0.0050-1000 cd/m², MaxCLL 1000, Dolby Vision 8.1".
pub fn media(info: &MediaInfo) -> String {
    let mut parts = vec![format!(
        "{} {}x{} {}-bit",
        info.codec.to_uppercase(),
        info.width,
        info.height,
        info.bit_depth
    )];
    if info.fps > 0.0 {
        parts.push(format!("{:.3} fps", info.fps));
    }
    let transfer = match info.transfer {
        SourceTransfer::Sdr => "SDR",
        SourceTransfer::Pq => "PQ",
        SourceTransfer::Hlg => "HLG",
    };
    parts.push(format!("{} {transfer}", primaries(info.primaries)));
    if let Some((min, max)) = info.mastering_nits {
        parts.push(format!("mastered {}-{} cd/m²", nits(min), nits(max)));
    }
    if let Some(cll) = info.max_cll {
        parts.push(format!("MaxCLL {}", nits(cll)));
    }
    if let Some(dv) = info.dolby_vision {
        let mut text = format!("Dolby Vision profile {}", dv.profile);
        if dv.base_layer_compatibility != 0 {
            text += &format!(".{}", dv.base_layer_compatibility);
        }
        if dv.enhancement_layer {
            text += " (enhancement layer not used)";
        }
        parts.push(text);
    }
    parts.join(", ")
}

pub fn advice(a: &Advice) -> String {
    match a {
        Advice::EnableHdrInSystem => {
            "This display takes HDR, but the system is sending it SDR, so the picture is tone-mapped down. \
             Turn on HDR for it (macOS: System Settings > Displays > High Dynamic Range)."
                .into()
        }
        Advice::DisplayPeakUnknown => {
            "The display does not say how bright it gets; 1000 cd/m² is assumed.".into()
        }
        Advice::Compressing { content_nits, display_nits } => format!(
            "The video goes up to {} cd/m², the display to {}: highlights are tone-mapped to fit.",
            nits(*content_nits),
            nits(*display_nits)
        ),
        Advice::HdrOnSdrDisplay => "An HDR video on an SDR display: it is tone-mapped down to SDR.".into(),
    }
}

/// One line for what is happening right now, from the file to the screen.
pub fn signal_path(
    info: &MediaInfo,
    stats: &RenderStats,
    output: &Output,
    mode: HdrMode,
) -> String {
    let mut source = match (stats.source_primaries, stats.source_transfer) {
        (Some(p), Some(t)) => {
            let t = match t {
                Transfer::Sdr => "SDR",
                Transfer::Pq => "PQ",
                Transfer::Hlg => "HLG",
            };
            format!("{} {t}", primaries(p))
        }
        _ => "untagged".to_string(),
    };
    if stats.dolby_vision {
        source += " + Dolby Vision RPU";
    }
    if stats.hdr10_plus {
        source += " + HDR10+";
    }
    if stats.source_nits.1 > 0.0 && stats.source_transfer != Some(Transfer::Sdr) {
        source += &format!(", {} cd/m²", nits(stats.source_nits.1));
    }

    let got = match (stats.output_primaries, stats.output_transfer) {
        (Some(p), Some(Transfer::Pq)) => {
            format!(
                "{} PQ {}-bit, {} cd/m²",
                primaries(p),
                stats.output_bits,
                nits(stats.output_nits.1)
            )
        }
        (Some(p), Some(Transfer::Hlg)) => format!("{} HLG {}-bit", primaries(p), stats.output_bits),
        (Some(p), _) => format!("{} SDR {}-bit", primaries(p), stats.output_bits),
        _ => "unknown surface".to_string(),
    };
    let wanted_hdr = output.signal == Signal::Hdr10;
    let got_hdr = matches!(stats.output_transfer, Some(Transfer::Pq | Transfer::Hlg));
    let how = match (wanted_hdr, got_hdr) {
        (true, true) if output.passthrough => "passthrough",
        (true, true) => "mapped in the player",
        (true, false) => "HDR surface refused by the system",
        (false, _) if info.transfer != SourceTransfer::Sdr => "tone-mapped to SDR",
        (false, _) => "as is",
    };
    let decode = if info.hardware {
        "hardware decode"
    } else {
        "software decode"
    };
    format!(
        "{source}  ->  {got} ({how}; {decode}; mode: {})",
        mode.label()
    )
}

/// "DTS (DTS-HD MA) 5.1(side) 48000 Hz [eng]".
pub fn sound_track(track: &AudioInfo) -> String {
    let mut text = track.codec.to_uppercase();
    if !track.profile.is_empty() && !track.profile.eq_ignore_ascii_case(&track.codec) {
        text += &format!(" ({})", track.profile);
    }
    text += &format!(" {} {} Hz", track.layout, track.sample_rate);
    if !track.language.is_empty() {
        text += &format!(" [{}]", track.language);
    }
    text
}

/// The sound's path: the track, the device, and what had to be changed in between.
pub fn sound_path(track: &AudioInfo, device: &bsv_audio::Format) -> String {
    let mut changes = Vec::new();
    if track.channels > device.channels as u32 {
        changes.push(format!(
            "{} channels mixed down to {}",
            track.channels, device.channels
        ));
    }
    if track.sample_rate != device.sample_rate {
        changes.push(format!("resampled to {} Hz", device.sample_rate));
    }
    let how = if changes.is_empty() {
        "as is".to_string()
    } else {
        changes.join(", ")
    };
    format!(
        "{}  ->  {}, {} ch {} Hz ({how})",
        sound_track(track),
        device.device,
        device.channels,
        device.sample_rate
    )
}

/// "sound, EAC3 5.1(side) 48000 Hz [eng]" or "subtitles, SUBRIP [rus] "Forced" (forced)".
pub fn track(track: &Track) -> String {
    let mut text = match track.kind {
        TrackKind::Audio => {
            format!(
                "sound, {} {} {} Hz",
                track.codec.to_uppercase(),
                track.layout,
                track.sample_rate
            )
        }
        TrackKind::Subtitle => format!(
            "subtitles, {}{}",
            track.codec.to_uppercase(),
            if track.bitmap { " (pictures)" } else { "" }
        ),
    };
    if !track.language.is_empty() {
        text += &format!(" [{}]", track.language);
    }
    if !track.title.is_empty() {
        text += &format!(" \"{}\"", track.title);
    }
    if track.forced {
        text += " (forced)";
    }
    if track.is_default {
        text += " (default)";
    }
    text
}
