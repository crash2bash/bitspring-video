//! What the picture is drawn onto: a Metal layer on the Mac, the window itself on Windows.

use std::ffi::c_void;

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::Window;

#[cfg(target_os = "macos")]
pub struct Surface {
    layer: raw_window_metal::Layer,
    view: *mut c_void,
}

#[cfg(target_os = "macos")]
impl Surface {
    /// # Safety
    /// The surface must be dropped before `window`.
    pub unsafe fn new(window: &Window) -> Result<Surface, String> {
        let handle = window.window_handle().map_err(|e| e.to_string())?;
        let RawWindowHandle::AppKit(appkit) = handle.as_raw() else {
            return Err("not an AppKit window".into());
        };
        // SAFETY: the view belongs to `window`, which the caller keeps alive longer than this.
        let layer = unsafe { raw_window_metal::Layer::from_ns_view(appkit.ns_view) };
        Ok(Surface {
            layer,
            view: appkit.ns_view.as_ptr(),
        })
    }

    /// What the renderer draws into: the `CAMetalLayer`.
    pub fn pointer(&self) -> *mut c_void {
        self.layer.as_ptr().as_ptr()
    }

    /// The window's content view, which the interface is put into.
    pub fn view(&self) -> *mut c_void {
        self.view
    }

    /// Hidden, the window's black shows instead of whatever frame a finished session left behind.
    pub fn set_hidden(&self, hidden: bool) {
        set_layer_hidden(&self.layer, hidden);
    }

    /// Say whether the content goes above SDR white.
    pub fn set_hdr(&self, hdr: bool) {
        set_layer_dynamic_range(&self.layer, hdr);
    }

    /// What the system has been told about the surface, for the log.
    pub fn describe(&self) -> Option<String> {
        Some(layer_state(&self.layer))
    }
}

#[cfg(windows)]
pub struct Surface {
    hwnd: *mut c_void,
}

#[cfg(windows)]
impl Surface {
    /// # Safety
    /// The surface must be dropped before `window`.
    pub unsafe fn new(window: &Window) -> Result<Surface, String> {
        let handle = window.window_handle().map_err(|e| e.to_string())?;
        let RawWindowHandle::Win32(win32) = handle.as_raw() else {
            return Err("not a Win32 window".into());
        };
        Ok(Surface {
            hwnd: win32.hwnd.get() as *mut c_void,
        })
    }

    /// What the renderer draws into: the window, by its handle.
    pub fn pointer(&self) -> *mut c_void {
        self.hwnd
    }

    pub fn view(&self) -> *mut c_void {
        self.hwnd
    }

    // On Windows the swapchain carries the colour space and the HDR metadata itself, and it
    // goes away with the session: there is nothing to hide or to tell the system separately.
    pub fn set_hidden(&self, _hidden: bool) {}

    pub fn set_hdr(&self, _hdr: bool) {}

    pub fn describe(&self) -> Option<String> {
        None
    }
}

#[cfg(target_os = "macos")]
/// An `NSString` for a short ASCII literal.
fn ns_string(text: &std::ffi::CStr) -> objc2::rc::Retained<objc2::runtime::NSObject> {
    // SAFETY: `stringWithUTF8String:` copies a valid NUL-terminated string.
    unsafe { objc2::msg_send![objc2::class!(NSString), stringWithUTF8String: text.as_ptr()] }
}
#[cfg(target_os = "macos")]
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
#[cfg(target_os = "macos")]
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
#[cfg(target_os = "macos")]
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
