//! The DWM system backdrop behind a see-through window (Windows 11 22H2 acrylic). The attribute is
//! documented but build dependent: Windows 10 and Windows 11 before 22H2 refuse it, and the window then
//! simply has no blur (pre-flight P18d). Nothing here is undocumented.

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::Graphics::Dwm::{DwmExtendFrameIntoClientArea, DwmSetWindowAttribute, DWMSBT_NONE, DWMSBT_TRANSIENTWINDOW, DWMWA_SYSTEMBACKDROP_TYPE};
use windows_sys::Win32::UI::Controls::MARGINS;

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
    let m = if on { -1 } else { 0 };
    let margins = MARGINS { cxLeftWidth: m, cxRightWidth: m, cyTopHeight: m, cyBottomHeight: m };
    // SAFETY: `margins` outlives the call.
    unsafe { DwmExtendFrameIntoClientArea(hwnd, &margins) };
    hr == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_blur_refuses_a_dead_window() {
        assert!(!set_blur(0, true));
    }
}
