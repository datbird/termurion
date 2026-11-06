//! Win32 handle operations.

use windows_sys::Win32::Foundation::{HWND, POINT};
use windows_sys::Win32::Graphics::Gdi::{GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow, ScreenToClient};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GA_ROOT, GWL_EXSTYLE, GetAncestor, GetCursorPos, GetForegroundWindow, GetWindowLongPtrW, GetWindowLongW, GetWindowPlacement,
    IsIconic, IsZoomed, SW_HIDE, SW_SHOW, SW_SHOWMAXIMIZED, SW_SHOWMINNOACTIVE, SW_SHOWNA, SW_SHOWNOACTIVATE, SetForegroundWindow, SetWindowLongPtrW, ShowWindow, WINDOWPLACEMENT, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW, WS_EX_TRANSPARENT, WindowFromPoint,
};
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::Window;

use super::super::window::{Flash, Show};
use crate::layout::Rect;

/// Windows' title bar falls back to a font that has the character: the title as it is.
pub fn os_title(title: &str) -> std::borrow::Cow<'_, str> {
    std::borrow::Cow::Borrowed(title)
}

pub fn hwnd_of(window: &Window) -> Option<isize> {
    match window.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(h) => Some(h.hwnd.get()),
        _ => None,
    }
}

pub fn view_of(window: &Window) -> Option<isize> {
    hwnd_of(window)
}

/// Windows has no activation tokens: the caller raises the window as always.
pub fn activate(_window: &Window, _token: Option<&str>) -> Result<bool, String> {
    Ok(false)
}

/// Nothing is kept on Windows.
pub fn forget_activation() {}

pub fn cursor_pos() -> Option<(i32, i32)> {
    let mut p = POINT { x: 0, y: 0 };
    (unsafe { GetCursorPos(&mut p) } != 0).then_some((p.x, p.y))
}

pub fn root_at(x: i32, y: i32) -> Option<isize> {
    let under = unsafe { GetAncestor(WindowFromPoint(POINT { x, y }), GA_ROOT) } as isize;
    (under != 0).then_some(under)
}

pub fn screen_to_client(hwnd: isize, x: i32, y: i32) -> Option<(i32, i32)> {
    let mut p = POINT { x, y };
    (unsafe { ScreenToClient(hwnd as HWND, &mut p) } != 0).then_some((p.x, p.y))
}

pub fn set_foreground(hwnd: isize) -> bool {
    unsafe { SetForegroundWindow(hwnd as HWND) != 0 }
}

pub fn is_foreground(hwnd: isize) -> bool {
    unsafe { GetForegroundWindow() == hwnd as HWND }
}

pub fn is_minimized(hwnd: isize) -> bool {
    unsafe { IsIconic(hwnd as HWND) != 0 }
}

pub fn is_maximized(hwnd: isize) -> bool {
    unsafe { IsZoomed(hwnd as HWND) != 0 }
}

pub fn show(hwnd: isize, how: Show) {
    let cmd = match how {
        Show::Hide => SW_HIDE,
        Show::Show => SW_SHOW,
        Show::ShowNoActivate => SW_SHOWNOACTIVATE,
        Show::ShowMaximized => SW_SHOWMAXIMIZED,
        Show::ShowMinimizedNoActivate => SW_SHOWMINNOACTIVE,
        Show::ShowAsIs => SW_SHOWNA,
    };
    unsafe { ShowWindow(hwnd as HWND, cmd) };
}

pub fn tool_style(hwnd: isize) -> bool {
    unsafe { GetWindowLongPtrW(hwnd as HWND, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW != 0 }
}

pub fn set_tool_style(hwnd: isize, want: bool) {
    unsafe {
        let style = GetWindowLongPtrW(hwnd as HWND, GWL_EXSTYLE);
        let style = if want { style | WS_EX_TOOLWINDOW as isize } else { style & !(WS_EX_TOOLWINDOW as isize) };
        SetWindowLongPtrW(hwnd as HWND, GWL_EXSTYLE, style);
    }
}

pub fn set_overlay_style(hwnd: isize) {
    unsafe { SetWindowLongPtrW(hwnd as _, GWL_EXSTYLE, GetWindowLongPtrW(hwnd as _, GWL_EXSTYLE) | (WS_EX_NOACTIVATE | WS_EX_TRANSPARENT | WS_EX_TOOLWINDOW) as isize) };
}

pub fn monitor_rects(hwnd: isize) -> Option<(Rect, Rect)> {
    let mut info: MONITORINFO = unsafe { std::mem::zeroed() };
    info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
    let m = unsafe { MonitorFromWindow(hwnd as _, MONITOR_DEFAULTTONEAREST) };
    if unsafe { GetMonitorInfoW(m, &mut info) } == 0 {
        return None;
    }
    let r = |r: windows_sys::Win32::Foundation::RECT| Rect { x: r.left as f32, y: r.top as f32, w: (r.right - r.left) as f32, h: (r.bottom - r.top) as f32 };
    Some((r(info.rcWork), r(info.rcMonitor)))
}

pub fn placement(hwnd: isize) -> Option<(u32, u32, [i32; 4], (i32, i32))> {
    let h = hwnd as HWND;
    let mut wp: WINDOWPLACEMENT = unsafe { std::mem::zeroed() };
    wp.length = std::mem::size_of::<WINDOWPLACEMENT>() as u32;
    if unsafe { GetWindowPlacement(h, &mut wp) } == 0 {
        return None;
    }
    let tool = unsafe { GetWindowLongW(h, GWL_EXSTYLE) } as u32 & WS_EX_TOOLWINDOW != 0;
    let mut shift = (0, 0);
    if !tool {
        let mut mi: MONITORINFO = unsafe { std::mem::zeroed() };
        mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if unsafe { GetMonitorInfoW(MonitorFromWindow(h, MONITOR_DEFAULTTONEAREST), &mut mi) } != 0 {
            shift = (mi.rcWork.left - mi.rcMonitor.left, mi.rcWork.top - mi.rcMonitor.top);
        }
    }
    let r = wp.rcNormalPosition;
    Some((wp.showCmd, wp.flags, [r.left, r.top, r.right, r.bottom], shift))
}

/// `FlashWindowEx`: every flash of the app goes through here.
pub fn flash(hwnd: isize, how: Flash) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{FLASHW_ALL, FLASHW_STOP, FLASHW_TIMERNOFG, FLASHW_TRAY, FLASHWINFO, FlashWindowEx};
    let (flags, count) = match how {
        Flash::Once => (FLASHW_TRAY, 1),
        Flash::UntilFront => (FLASHW_TRAY | FLASHW_TIMERNOFG, 0),
        Flash::OnceAll => (FLASHW_ALL, 1),
        Flash::Stop => (FLASHW_STOP, 0),
    };
    let info = FLASHWINFO { cbSize: std::mem::size_of::<FLASHWINFO>() as u32, hwnd: hwnd as _, dwFlags: flags, uCount: count, dwTimeout: 0 };
    unsafe { FlashWindowEx(&info) };
}

/// `IsWindowArranged` (user32 on Windows 10 and later; looked up at run time, so an older Windows says no).
pub fn is_arranged(hwnd: isize) -> bool {
    use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
    type Arranged = unsafe extern "system" fn(HWND) -> i32;
    static ARRANGED: std::sync::OnceLock<Option<Arranged>> = std::sync::OnceLock::new();
    let f = ARRANGED.get_or_init(|| {
        let user32: Vec<u16> = "user32.dll".encode_utf16().chain(Some(0)).collect();
        let p = unsafe { GetProcAddress(GetModuleHandleW(user32.as_ptr()), c"IsWindowArranged".as_ptr().cast())? };
        Some(unsafe { std::mem::transmute::<unsafe extern "system" fn() -> isize, Arranged>(p) })
    });
    f.is_some_and(|f| unsafe { f(hwnd as HWND) } != 0)
}
