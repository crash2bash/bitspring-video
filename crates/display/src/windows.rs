//! Display detection on Windows: DXGI for the HDR state and luminance (which include the
//! user's HDR calibration), the display configuration API for the SDR white level.

use windows::Win32::Devices::Display::{
    DISPLAYCONFIG_DEVICE_INFO_GET_SDR_WHITE_LEVEL, DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
    DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_PATH_INFO, DISPLAYCONFIG_SDR_WHITE_LEVEL,
    DISPLAYCONFIG_SOURCE_DEVICE_NAME, DisplayConfigGetDeviceInfo, GetDisplayConfigBufferSizes,
    QDC_ONLY_ACTIVE_PATHS, QueryDisplayConfig,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020;
use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1, IDXGIOutput6};
use windows::core::Interface;

use crate::{Display, Error, HdrStatus, LuminanceHints, OsReport, resolve_luminance};

/// What the display configuration API knows about one active output.
struct PathInfo {
    gdi_name: Vec<u16>,
    sdr_white_nits: Option<f32>,
    refresh_hz: f64,
}

pub fn displays() -> Result<Vec<Display>, Error> {
    let paths = active_paths();
    let mut out = Vec::new();
    let os = |e: windows::core::Error| Error::Os(e.to_string());

    let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1() }.map_err(os)?;
    let mut adapter_index = 0;
    while let Ok(adapter) = unsafe { factory.EnumAdapters1(adapter_index) } {
        adapter_index += 1;
        let mut output_index = 0;
        while let Ok(output) = unsafe { adapter.EnumOutputs(output_index) } {
            output_index += 1;
            let Ok(output6) = output.cast::<IDXGIOutput6>() else {
                continue;
            };
            let Ok(desc) = (unsafe { output6.GetDesc1() }) else {
                continue;
            };

            let name_len = desc
                .DeviceName
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(desc.DeviceName.len());
            let gdi_name = &desc.DeviceName[..name_len];
            let path = paths.iter().find(|p| p.gdi_name == gdi_name);

            let hdr10_desktop = desc.ColorSpace == DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020;
            // With HDR off Windows reports an SDR range here (peak 270 or so); a display whose
            // reported peak clears SDR by a wide margin can take HDR once it is switched on.
            let hdr = if hdr10_desktop {
                HdrStatus::Active
            } else if desc.MaxLuminance > 400.0 {
                HdrStatus::Disabled
            } else {
                HdrStatus::Unsupported
            };
            let luminance = resolve_luminance(
                None,
                LuminanceHints {
                    os_peak: hdr10_desktop.then_some(desc.MaxLuminance),
                    os_min: hdr10_desktop.then_some(desc.MinLuminance),
                    os_frame_avg: hdr10_desktop.then_some(desc.MaxFullFrameLuminance),
                    headroom: None,
                },
            );
            let rect = desc.DesktopCoordinates;
            // Red and green clearly outside BT.709.
            let wide_gamut = desc.RedPrimary[0] > 0.66 || desc.GreenPrimary[1] > 0.63;

            out.push(Display {
                id: desc.Monitor.0 as usize as u64,
                name: String::from_utf16_lossy(gdi_name),
                builtin: false,
                connection: None,
                width: (rect.right - rect.left).max(0) as u32,
                height: (rect.bottom - rect.top).max(0) as u32,
                refresh_hz: path.map(|p| p.refresh_hz).unwrap_or(0.0),
                wide_gamut,
                hdr,
                luminance,
                edid: None,
                os: OsReport::Windows {
                    hdr10_desktop,
                    sdr_white_nits: path.and_then(|p| p.sdr_white_nits),
                    bits_per_color: desc.BitsPerColor,
                },
            });
        }
    }
    Ok(out)
}

fn active_paths() -> Vec<PathInfo> {
    let mut found = Vec::new();
    unsafe {
        let (mut path_count, mut mode_count) = (0u32, 0u32);
        if GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut path_count, &mut mode_count)
            .is_err()
        {
            return found;
        }
        let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); path_count as usize];
        let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); mode_count as usize];
        if QueryDisplayConfig(
            QDC_ONLY_ACTIVE_PATHS,
            &mut path_count,
            paths.as_mut_ptr(),
            &mut mode_count,
            modes.as_mut_ptr(),
            None,
        )
        .is_err()
        {
            return found;
        }
        paths.truncate(path_count as usize);

        for path in &paths {
            let mut source = DISPLAYCONFIG_SOURCE_DEVICE_NAME::default();
            source.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME;
            source.header.size = size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32;
            source.header.adapterId = path.sourceInfo.adapterId;
            source.header.id = path.sourceInfo.id;
            if DisplayConfigGetDeviceInfo(&mut source.header) != 0 {
                continue;
            }
            let name = source.viewGdiDeviceName;
            let len = name.iter().position(|&c| c == 0).unwrap_or(name.len());

            let mut white = DISPLAYCONFIG_SDR_WHITE_LEVEL::default();
            white.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_SDR_WHITE_LEVEL;
            white.header.size = size_of::<DISPLAYCONFIG_SDR_WHITE_LEVEL>() as u32;
            white.header.adapterId = path.targetInfo.adapterId;
            white.header.id = path.targetInfo.id;
            // SDRWhiteLevel is in thousandths of 80 cd/m².
            let sdr_white_nits = (DisplayConfigGetDeviceInfo(&mut white.header) == 0)
                .then(|| white.SDRWhiteLevel as f32 / 1000.0 * 80.0);

            let rate = path.targetInfo.refreshRate;
            let refresh_hz = if rate.Denominator != 0 {
                rate.Numerator as f64 / rate.Denominator as f64
            } else {
                0.0
            };

            found.push(PathInfo {
                gdi_name: name[..len].to_vec(),
                sdr_white_nits,
                refresh_hz,
            });
        }
    }
    found
}
