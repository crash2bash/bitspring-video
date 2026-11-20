//! EDID parsing: only the parts that say what a display can show.
//!
//! The base block gives identity and the panel's primaries. HDR lives in the CTA-861
//! extension: the HDR Static Metadata block (which transfer functions the display accepts and
//! the luminance range it wants content mapped to), the Colorimetry block (BT.2020 signalling),
//! and vendor blocks for Dolby Vision and HDR10+.

/// CIE 1931 xy chromaticity.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Xy {
    pub x: f32,
    pub y: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Chromaticities {
    pub red: Xy,
    pub green: Xy,
    pub blue: Xy,
    pub white: Xy,
}

/// CTA-861.3 HDR Static Metadata Data Block.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct HdrStatic {
    /// SMPTE ST 2084 (PQ): HDR10, HDR10+ and Dolby Vision's base signal.
    pub pq: bool,
    /// Hybrid Log-Gamma.
    pub hlg: bool,
    /// Peak luminance the display wants content mapped to, cd/m². Optional in the block.
    pub max_nits: Option<f32>,
    /// Luminance it can hold over a full white frame, cd/m².
    pub max_frame_avg_nits: Option<f32>,
    /// Black level, cd/m².
    pub min_nits: Option<f32>,
}

/// Dolby Vision Vendor-Specific Video Data Block (a TV that takes Dolby Vision over HDMI).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DolbyVision {
    pub version: u8,
    /// The display also accepts player-led ("low latency") Dolby Vision.
    pub low_latency: bool,
    /// Luminance range from the Dolby block. Layout follows edid-decode; treated as a hint.
    pub max_nits: Option<f32>,
    pub min_nits: Option<f32>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Edid {
    /// Three-letter PNP id, e.g. "AUS".
    pub manufacturer: String,
    pub vendor_id: u16,
    pub product_id: u16,
    pub serial: u32,
    pub name: Option<String>,
    pub year: u16,
    /// Bits per colour the panel input takes, when the base block says.
    pub bit_depth: Option<u8>,
    pub gamma: Option<f32>,
    pub chromaticities: Chromaticities,
    pub hdr: Option<HdrStatic>,
    /// The display accepts BT.2020 RGB signalling.
    pub bt2020_rgb: bool,
    pub dolby_vision: Option<DolbyVision>,
    pub hdr10_plus: bool,
}

#[derive(Debug, PartialEq)]
pub enum EdidError {
    TooShort,
    BadHeader,
}

impl std::fmt::Display for EdidError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EdidError::TooShort => write!(f, "EDID is shorter than one 128-byte block"),
            EdidError::BadHeader => write!(f, "EDID header is not 00 FF FF FF FF FF FF 00"),
        }
    }
}

impl std::error::Error for EdidError {}

const HEADER: [u8; 8] = [0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00];

impl Edid {
    pub fn parse(data: &[u8]) -> Result<Edid, EdidError> {
        if data.len() < 128 {
            return Err(EdidError::TooShort);
        }
        if data[..8] != HEADER {
            return Err(EdidError::BadHeader);
        }
        let base = &data[..128];

        // Manufacturer: three 5-bit letters, big-endian, 1 = 'A'.
        let m = u16::from_be_bytes([base[8], base[9]]);
        let letter = |v: u16| (b'A' + (v as u8 & 0x1F).saturating_sub(1)) as char;
        let manufacturer: String = [letter(m >> 10), letter(m >> 5), letter(m)]
            .iter()
            .collect();

        let bit_depth = if base[20] & 0x80 != 0 {
            match (base[20] >> 4) & 0x07 {
                1 => Some(6),
                2 => Some(8),
                3 => Some(10),
                4 => Some(12),
                5 => Some(14),
                6 => Some(16),
                _ => None,
            }
        } else {
            None
        };
        let gamma = (base[23] != 0xFF).then(|| (base[23] as f32 + 100.0) / 100.0);

        // Ten-bit chromaticities: high eight bits in bytes 27..=34, low two packed in 25 and 26.
        let coord = |hi: u8, lo: u8| (((hi as u16) << 2) | (lo as u16 & 0x03)) as f32 / 1024.0;
        let (l0, l1) = (base[25], base[26]);
        let chromaticities = Chromaticities {
            red: Xy {
                x: coord(base[27], l0 >> 6),
                y: coord(base[28], l0 >> 4),
            },
            green: Xy {
                x: coord(base[29], l0 >> 2),
                y: coord(base[30], l0),
            },
            blue: Xy {
                x: coord(base[31], l1 >> 6),
                y: coord(base[32], l1 >> 4),
            },
            white: Xy {
                x: coord(base[33], l1 >> 2),
                y: coord(base[34], l1),
            },
        };

        let mut name = None;
        for off in [54, 72, 90, 108] {
            let d = &base[off..off + 18];
            if d[0] == 0 && d[1] == 0 && d[3] == 0xFC {
                let text: String = d[5..18]
                    .iter()
                    .take_while(|&&b| b != 0x0A && b != 0)
                    .map(|&b| b as char)
                    .collect();
                let text = text.trim().to_string();
                if !text.is_empty() {
                    name = Some(text);
                }
            }
        }

        let mut edid = Edid {
            manufacturer,
            vendor_id: m,
            product_id: u16::from_le_bytes([base[10], base[11]]),
            serial: u32::from_le_bytes([base[12], base[13], base[14], base[15]]),
            name,
            year: 1990 + base[17] as u16,
            bit_depth,
            gamma,
            chromaticities,
            hdr: None,
            bt2020_rgb: false,
            dolby_vision: None,
            hdr10_plus: false,
        };

        // Walk every extension that is physically present. The count in byte 126 can be lower
        // than the real one (HDMI's EEODB override), so the data length is the authority.
        for block in data[128..].as_chunks::<128>().0 {
            if block[0] == 0x02 {
                edid.parse_cta(block);
            }
        }
        Ok(edid)
    }

    fn parse_cta(&mut self, block: &[u8]) {
        let dtd_start = block[2] as usize;
        if !(4..=127).contains(&dtd_start) {
            return;
        }
        let mut i = 4;
        while i < dtd_start {
            let tag = block[i] >> 5;
            let len = (block[i] & 0x1F) as usize;
            let end = i + 1 + len;
            if end > dtd_start {
                break;
            }
            let body = &block[i + 1..end];
            if tag == 7 && !body.is_empty() {
                let payload = &body[1..];
                match body[0] {
                    0x01 => self.parse_vendor_video(payload),
                    0x05 if !payload.is_empty() => self.bt2020_rgb = payload[0] & 0x80 != 0,
                    0x06 => self.hdr = parse_hdr_static(payload),
                    _ => {}
                }
            }
            i = end;
        }
    }

    fn parse_vendor_video(&mut self, payload: &[u8]) {
        if payload.len() < 4 {
            return;
        }
        // IEEE OUI, least significant byte first.
        let oui = u32::from_le_bytes([payload[0], payload[1], payload[2], 0]);
        let x = &payload[3..];
        match oui {
            0x00D046 => self.dolby_vision = parse_dolby(x),
            0x90848B => self.hdr10_plus = true,
            _ => {}
        }
    }
}

fn parse_hdr_static(p: &[u8]) -> Option<HdrStatic> {
    if p.len() < 2 {
        return None;
    }
    // Coded luminance: 0 means "not given".
    let coded = |idx: usize| p.get(idx).copied().filter(|&v| v != 0);
    let max_nits = coded(2).map(|cv| 50.0 * 2f32.powf(cv as f32 / 32.0));
    let max_frame_avg_nits = coded(3).map(|cv| 50.0 * 2f32.powf(cv as f32 / 32.0));
    let min_nits = match (max_nits, coded(4)) {
        (Some(max), Some(cv)) => Some(max * (cv as f32 / 255.0).powi(2) / 100.0),
        _ => None,
    };
    Some(HdrStatic {
        pq: p[0] & 0x04 != 0,
        hlg: p[0] & 0x08 != 0,
        max_nits,
        max_frame_avg_nits,
        min_nits,
    })
}

/// SMPTE ST 2084 EOTF: a normalised PQ code value to cd/m².
pub fn pq_to_nits(e: f32) -> f32 {
    const M1: f32 = 2610.0 / 16384.0;
    const M2: f32 = 2523.0 / 4096.0 * 128.0;
    const C1: f32 = 3424.0 / 4096.0;
    const C2: f32 = 2413.0 / 4096.0 * 32.0;
    const C3: f32 = 2392.0 / 4096.0 * 32.0;
    let p = e.clamp(0.0, 1.0).powf(1.0 / M2);
    10000.0 * ((p - C1).max(0.0) / (C2 - C3 * p)).powf(1.0 / M1)
}

fn parse_dolby(x: &[u8]) -> Option<DolbyVision> {
    let first = *x.first()?;
    let version = first >> 5;
    let pq12 = |v: u16| pq_to_nits(v as f32 / 4095.0);
    match version {
        0 if x.len() >= 16 => Some(DolbyVision {
            version,
            low_latency: false,
            min_nits: Some(pq12(((x[14] as u16) << 4) | (x[13] as u16 >> 4))),
            max_nits: Some(pq12(((x[15] as u16) << 4) | (x[13] as u16 & 0x0F))),
        }),
        1 if x.len() >= 4 => {
            let min = (x[2] >> 1) as f32 / 127.0;
            Some(DolbyVision {
                version,
                low_latency: x[3] & 0x01 != 0,
                min_nits: Some(min * min),
                max_nits: Some(100.0 + (x[1] >> 1) as f32 * 50.0),
            })
        }
        2 if x.len() >= 3 => Some(DolbyVision {
            version,
            // Interface field: 0 and 1 are low-latency only, 2 and 3 add the standard interface.
            low_latency: true,
            min_nits: Some(pq12(20 * (x[1] as u16 >> 3))),
            max_nits: Some(pq12(2055 + 65 * (x[2] as u16 >> 3))),
        }),
        _ => Some(DolbyVision {
            version,
            low_latency: false,
            min_nits: None,
            max_nits: None,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Vec<u8> {
        let hex = include_str!("../tests/fixtures/asus-pg32ucdp.edid.hex");
        let digits: Vec<u8> = hex.bytes().filter(|b| b.is_ascii_hexdigit()).collect();
        digits
            .as_chunks::<2>()
            .0
            .iter()
            .map(|p| u8::from_str_radix(std::str::from_utf8(p).unwrap(), 16).unwrap())
            .collect()
    }

    #[test]
    fn identity_of_a_real_monitor() {
        let e = Edid::parse(&fixture()).unwrap();
        assert_eq!(e.manufacturer, "AUS");
        assert_eq!(e.product_id, 13044);
        assert_eq!(e.name.as_deref(), Some("PG32UCDP"));
        assert_eq!(e.year, 2025);
        // Byte 20 is 0x80: digital input, bit depth left undefined.
        assert_eq!(e.bit_depth, None);
        assert_eq!(e.gamma, Some(2.2));
    }

    #[test]
    fn hdr_block_of_a_real_monitor() {
        let e = Edid::parse(&fixture()).unwrap();
        let hdr = e.hdr.expect("the monitor advertises HDR");
        assert!(hdr.pq);
        assert!(!hdr.hlg);
        // Coded 0x97, 0x4D, 0x01.
        assert!((hdr.max_nits.unwrap() - 1316.5).abs() < 1.0);
        assert!((hdr.max_frame_avg_nits.unwrap() - 265.0).abs() < 1.0);
        assert!(hdr.min_nits.unwrap() < 0.001);
        assert!(e.bt2020_rgb);
        assert_eq!(e.dolby_vision, None);
        assert!(!e.hdr10_plus);
    }

    #[test]
    fn primaries_are_wide_gamut() {
        let c = Edid::parse(&fixture()).unwrap().chromaticities;
        // An OLED panel: red and green sit well outside BT.709 (0.64/0.33, 0.30/0.60).
        assert!(c.red.x > 0.66, "red x = {}", c.red.x);
        assert!(c.green.y > 0.62, "green y = {}", c.green.y);
        assert!((c.white.x - 0.3127).abs() < 0.005);
        assert!((c.white.y - 0.3290).abs() < 0.005);
    }

    #[test]
    fn rejects_garbage() {
        assert_eq!(Edid::parse(&[0; 64]), Err(EdidError::TooShort));
        assert_eq!(Edid::parse(&[0x11; 128]), Err(EdidError::BadHeader));
    }

    #[test]
    fn pq_reference_points() {
        assert!(pq_to_nits(0.0) < 1e-6);
        assert!((pq_to_nits(1.0) - 10000.0).abs() < 1.0);
        // 10-bit code 520 is the well-known ~100 cd/m² point.
        assert!((pq_to_nits(520.0 / 1023.0) - 100.0).abs() < 1.5);
    }

    #[test]
    fn dolby_vision_v1_block() {
        // Version 1, DM 3.x; max = 100 + 18 * 50 = 1000 cd/m²; low latency.
        let dv = parse_dolby(&[0b0010_0100, 18 << 1, 0x00, 0x01]).unwrap();
        assert_eq!(dv.version, 1);
        assert!(dv.low_latency);
        assert_eq!(dv.max_nits, Some(1000.0));
    }
}
