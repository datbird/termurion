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

use super::super::window::Show;
use crate::layout::Rect;

pub fn hwnd_of(window: &Window) -> Option<isize> {
    match window.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(h) => Some(h.hwnd.get()),
        _ => None,
    }
}

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
