//! Display detection on the Mac: NSScreen for the live HDR state, the I/O Registry for the EDID.

use std::ffi::{CStr, c_char, c_void};
use std::ptr;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{MainThreadMarker, class, msg_send};
use objc2_foundation::{NSArray, NSDictionary, NSNumber, NSString};

use crate::{
    Display, Edid, Error, HdrStatus, LuminanceHints, OsReport, is_wide_gamut, resolve_luminance,
};

#[link(name = "AppKit", kind = "framework")]
unsafe extern "C" {}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGDisplayVendorNumber(display: u32) -> u32;
    fn CGDisplayModelNumber(display: u32) -> u32;
    fn CGDisplayIsBuiltin(display: u32) -> u32;
    fn CGDisplayCopyDisplayMode(display: u32) -> *mut c_void;
    fn CGDisplayModeGetPixelWidth(mode: *mut c_void) -> usize;
    fn CGDisplayModeGetPixelHeight(mode: *mut c_void) -> usize;
    fn CGDisplayModeGetRefreshRate(mode: *mut c_void) -> f64;
    fn CGDisplayModeRelease(mode: *mut c_void);
}

type CfRef = *const c_void;

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(cf: CfRef);
    fn CFGetTypeID(cf: CfRef) -> usize;
    fn CFDataGetTypeID() -> usize;
    fn CFDataGetLength(data: CfRef) -> isize;
    fn CFDataGetBytePtr(data: CfRef) -> *const u8;
    fn CFStringGetTypeID() -> usize;
    fn CFStringCreateWithCString(alloc: CfRef, s: *const c_char, encoding: u32) -> CfRef;
    fn CFStringGetCString(s: CfRef, buf: *mut c_char, len: isize, encoding: u32) -> u8;
}

#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    fn IOServiceMatching(name: *const c_char) -> CfRef;
    fn IOServiceGetMatchingServices(main_port: u32, matching: CfRef, existing: *mut u32) -> i32;
    fn IOIteratorNext(iterator: u32) -> u32;
    fn IOObjectRelease(object: u32) -> i32;
    fn IORegistryEntryCreateCFProperty(entry: u32, key: CfRef, alloc: CfRef, options: u32)
    -> CfRef;
}

const UTF8: u32 = 0x0800_0100;
/// NSDisplayGamutP3.
const GAMUT_P3: isize = 2;

/// An EDID found in the I/O Registry, with the port it arrived on.
struct RegistryEdid {
    edid: Edid,
    connection: Option<String>,
}

pub fn displays() -> Result<Vec<Display>, Error> {
    if MainThreadMarker::new().is_none() {
        return Err(Error::NotMainThread);
    }
    let registry = registry_edids();
    let mut out = Vec::new();

    let screens: Retained<NSArray<AnyObject>> = unsafe { msg_send![class!(NSScreen), screens] };
    for i in 0..screens.count() {
        let screen = screens.objectAtIndex(i);
        let Some(id) = screen_number(&screen) else {
            continue;
        };

        let name: Retained<NSString> = unsafe { msg_send![&*screen, localizedName] };
        let edr_now: f64 =
            unsafe { msg_send![&*screen, maximumExtendedDynamicRangeColorComponentValue] };
        let edr_potential: f64 = unsafe {
            msg_send![
                &*screen,
                maximumPotentialExtendedDynamicRangeColorComponentValue
            ]
        };
        let edr_reference: f64 = unsafe {
            msg_send![
                &*screen,
                maximumReferenceExtendedDynamicRangeColorComponentValue
            ]
        };
        let p3: bool = unsafe { msg_send![&*screen, canRepresentDisplayGamut: GAMUT_P3] };
        let max_fps: isize = unsafe { msg_send![&*screen, maximumFramesPerSecond] };
        let color_profile = color_profile_name(&screen);

        let (mut width, mut height, mut refresh_hz) = (0u32, 0u32, 0.0f64);
        unsafe {
            let mode = CGDisplayCopyDisplayMode(id);
            if !mode.is_null() {
                width = CGDisplayModeGetPixelWidth(mode) as u32;
                height = CGDisplayModeGetPixelHeight(mode) as u32;
                refresh_hz = CGDisplayModeGetRefreshRate(mode);
                CGDisplayModeRelease(mode);
            }
        }
        if refresh_hz <= 0.0 {
            // Built-in panels with a variable refresh rate report 0 for the mode.
            refresh_hz = max_fps as f64;
        }

        let (vendor, model) = unsafe { (CGDisplayVendorNumber(id), CGDisplayModelNumber(id)) };
        let found = registry
            .iter()
            .find(|r| r.edid.vendor_id as u32 == vendor && r.edid.product_id as u32 == model);
        let edid = found.map(|r| r.edid.clone());
        let connection = found.and_then(|r| r.connection.clone());

        // The system offers headroom above SDR white only while it is sending HDR.
        let hdr_active = edr_potential > 1.0;
        let hdr_capable = hdr_active || edid.as_ref().and_then(|e| e.hdr).is_some_and(|h| h.pq);
        let hdr = match (hdr_capable, hdr_active) {
            (_, true) => HdrStatus::Active,
            (true, false) => HdrStatus::Disabled,
            (false, false) => HdrStatus::Unsupported,
        };
        let luminance = resolve_luminance(
            edid.as_ref(),
            LuminanceHints {
                headroom: Some(edr_potential as f32),
                ..Default::default()
            },
        );

        out.push(Display {
            id: id as u64,
            name: name.to_string(),
            builtin: unsafe { CGDisplayIsBuiltin(id) } != 0,
            connection,
            width,
            height,
            refresh_hz,
            wide_gamut: p3 || edid.as_ref().is_some_and(is_wide_gamut),
            hdr,
            luminance,
            edid,
            os: OsReport::MacOs {
                edr_now: edr_now as f32,
                edr_potential: edr_potential as f32,
                edr_reference: edr_reference as f32,
                color_profile,
            },
        });
    }
    Ok(out)
}

fn screen_number(screen: &AnyObject) -> Option<u32> {
    let desc: Retained<NSDictionary<NSString, AnyObject>> =
        unsafe { msg_send![screen, deviceDescription] };
    let key = NSString::from_str("NSScreenNumber");
    let number: Option<Retained<NSNumber>> = unsafe { msg_send![&*desc, objectForKey: &*key] };
    number.map(|n| unsafe { msg_send![&*n, unsignedIntValue] })
}

fn color_profile_name(screen: &AnyObject) -> String {
    let space: Option<Retained<AnyObject>> = unsafe { msg_send![screen, colorSpace] };
    let name: Option<Retained<NSString>> =
        space.and_then(|s| unsafe { msg_send![&*s, localizedName] });
    name.map(|n| n.to_string()).unwrap_or_default()
}

/// Every EDID the I/O Registry holds. Apple Silicon keeps it on the display transport node;
/// Intel Macs on the IODisplay node.
fn registry_edids() -> Vec<RegistryEdid> {
    let mut found = Vec::new();
    for (class, key) in [
        (c"IOPortTransportStateDisplayPort", c"EDID"),
        (c"IODisplay", c"IODisplayEDID"),
    ] {
        for_each_service(class, |service| {
            let Some(bytes) = data_property(service, key) else {
                return;
            };
            let Ok(edid) = Edid::parse(&bytes) else {
                return;
            };
            let connection = string_property(service, c"ParentPortTypeDescription")
                .or_else(|| string_property(service, c"TransportTypeDescription"));
            found.push(RegistryEdid { edid, connection });
        });
        if !found.is_empty() {
            break;
        }
    }
    found
}

fn for_each_service(class: &CStr, mut f: impl FnMut(u32)) {
    unsafe {
        let matching = IOServiceMatching(class.as_ptr());
        if matching.is_null() {
            return;
        }
        let mut iterator = 0u32;
        // Consumes the matching dictionary.
        if IOServiceGetMatchingServices(0, matching, &mut iterator) != 0 {
            return;
        }
        loop {
            let service = IOIteratorNext(iterator);
            if service == 0 {
                break;
            }
            f(service);
            IOObjectRelease(service);
        }
        IOObjectRelease(iterator);
    }
}

fn property(service: u32, key: &CStr) -> Option<CfRef> {
    unsafe {
        let cf_key = CFStringCreateWithCString(ptr::null(), key.as_ptr(), UTF8);
        if cf_key.is_null() {
            return None;
        }
        let value = IORegistryEntryCreateCFProperty(service, cf_key, ptr::null(), 0);
        CFRelease(cf_key);
        (!value.is_null()).then_some(value)
    }
}

fn data_property(service: u32, key: &CStr) -> Option<Vec<u8>> {
    let value = property(service, key)?;
    unsafe {
        let bytes = (CFGetTypeID(value) == CFDataGetTypeID()).then(|| {
            let len = CFDataGetLength(value).max(0) as usize;
            std::slice::from_raw_parts(CFDataGetBytePtr(value), len).to_vec()
        });
        CFRelease(value);
        bytes
    }
}

fn string_property(service: u32, key: &CStr) -> Option<String> {
    let value = property(service, key)?;
    unsafe {
        let mut text = None;
        if CFGetTypeID(value) == CFStringGetTypeID() {
            let mut buf = [0 as c_char; 128];
            if CFStringGetCString(value, buf.as_mut_ptr(), buf.len() as isize, UTF8) != 0 {
                text = Some(CStr::from_ptr(buf.as_ptr()).to_string_lossy().into_owned());
            }
        }
        CFRelease(value);
        text
    }
}
