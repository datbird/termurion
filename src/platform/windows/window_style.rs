//! Win32 window styling.

use winit::platform::windows::WindowAttributesExtWindows;
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::{Window, WindowAttributes};

fn hwnd(window: &Window) -> Option<isize> {
    match window.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(h) => Some(h.hwnd.get()),
        _ => None,
    }
}

pub fn skip_taskbar(attrs: WindowAttributes) -> WindowAttributes {
    attrs.with_skip_taskbar(true)
}

pub fn owner_window(attrs: WindowAttributes, owner: isize) -> WindowAttributes {
    attrs.with_owner_window(owner)
}

pub fn no_activate(window: &Window) {
    if let Some(h) = hwnd(window) {
        use windows_sys::Win32::UI::WindowsAndMessaging::{GWL_EXSTYLE, GetWindowLongPtrW, SetWindowLongPtrW, WS_EX_NOACTIVATE};
        unsafe { SetWindowLongPtrW(h as _, GWL_EXSTYLE, GetWindowLongPtrW(h as _, GWL_EXSTYLE) | WS_EX_NOACTIVATE as isize) };
    }
}

pub fn show_no_activate(window: &Window) {
    if let Some(h) = hwnd(window) {
        use windows_sys::Win32::UI::WindowsAndMessaging::{SW_SHOWNOACTIVATE, ShowWindow};
        unsafe { ShowWindow(h as _, SW_SHOWNOACTIVATE) };
    }
}

/// Windows 11 rounds a borderless window only when asked.
pub fn round_corners(window: &Window) {
    if let Some(h) = hwnd(window) {
        use windows_sys::Win32::Graphics::Dwm::{DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DwmSetWindowAttribute};
        let v = DWMWCP_ROUND;
        unsafe { DwmSetWindowAttribute(h as _, DWMWA_WINDOW_CORNER_PREFERENCE as u32, (&v as *const i32).cast(), 4) };
    }
}

/// The DWM colorization color.
pub fn accent_color() -> Option<(u8, u8, u8)> {
    let (mut color, mut opaque) = (0u32, 0i32);
    let ok = unsafe { windows_sys::Win32::Graphics::Dwm::DwmGetColorizationColor(&mut color, &mut opaque) } >= 0;
    ok.then(|| ((color >> 16) as u8, (color >> 8) as u8, color as u8))
}
