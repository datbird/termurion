//! The DWM system backdrop behind a see-through window (Windows 11 22H2 acrylic). The attribute is
//! documented but build dependent: Windows 10 and Windows 11 before 22H2 refuse it, and the window then
//! simply has no blur (pre-flight P18d). Nothing here is undocumented.

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::Graphics::Dwm::{DwmExtendFrameIntoClientArea, DwmSetWindowAttribute, DWMSBT_NONE, DWMSBT_TRANSIENTWINDOW, DWMWA_SYSTEMBACKDROP_TYPE};
use windows_sys::Win32::UI::Controls::MARGINS;

/// The frame extension (in pixels, -1 for the whole client area) after a backdrop request: only an
/// accepted backdrop gets the extended frame; a refusal or "off" gets none.
fn frame_margin(on: bool, accepted: bool) -> i32 {
    if on && accepted {
        -1
    } else {
        0
    }
}

/// Turns the acrylic backdrop on (with the frame extended over the whole client area, or the backdrop shows
/// nothing) or off (backdrop none, zero margins). True when DWM accepted the backdrop attribute.
pub fn set_blur(hwnd: isize, on: bool) -> bool {
    if hwnd == 0 {
        return false;
    }
    let hwnd = hwnd as HWND;
    let kind: i32 = if on { DWMSBT_TRANSIENTWINDOW } else { DWMSBT_NONE };
    // SAFETY: `kind` outlives the call and the size matches the attribute's DWORD.
    let hr = unsafe { DwmSetWindowAttribute(hwnd, DWMWA_SYSTEMBACKDROP_TYPE as u32, &kind as *const i32 as *const _, std::mem::size_of::<i32>() as u32) };
    let m = frame_margin(on, hr == 0);
    let margins = MARGINS { cxLeftWidth: m, cxRightWidth: m, cyTopHeight: m, cyBottomHeight: m };
    // SAFETY: `margins` outlives the call.
    unsafe { DwmExtendFrameIntoClientArea(hwnd, &margins) };
    hr == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The frame extends over the client area only while the backdrop is on and DWM accepted it; a
    /// refusal (Windows 10) leaves no glass frame under the see-through pixels (P18d).
    #[test]
    fn frame_extends_only_for_an_accepted_backdrop() {
        assert_eq!(frame_margin(true, true), -1);
        assert_eq!(frame_margin(true, false), 0, "refused: no extended frame");
        assert_eq!(frame_margin(false, true), 0);
        assert_eq!(frame_margin(false, false), 0);
    }

    #[test]
    fn set_blur_refuses_a_dead_window() {
        assert!(!set_blur(0, true));
    }
}
