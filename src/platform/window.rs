//! Raw native-handle window operations: the pointer, the window under a point, show and hide, the
//! tool-window style, monitors. A handle is the HWND on Windows. On Unix an app window has no such
//! handle (`hwnd_of` is None), so the app never reaches the rest with one; the Unix versions answer
//! None or false for a handle they are given anyway.

use winit::window::Window;

use crate::layout::Rect;

#[cfg(unix)]
use super::unix::window as imp;
#[cfg(windows)]
use super::windows::window as imp;

/// How `show` changes a window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Show {
    Hide,
    Show,
    ShowNoActivate,
    ShowMaximized,
    ShowMinimizedNoActivate,
    /// Shown, with no activation, in whatever state it last had (`SW_SHOWNA`).
    ShowAsIs,
}

/// A window's native handle, when the OS gives it one.
pub fn hwnd_of(window: &Window) -> Option<isize> {
    imp::hwnd_of(window)
}

/// The pointer's screen point.
pub fn cursor_pos() -> Option<(i32, i32)> {
    imp::cursor_pos()
}

/// The top-level window under a screen point.
pub fn root_at(x: i32, y: i32) -> Option<isize> {
    imp::root_at(x, y)
}

/// A screen point in a window's client pixels.
pub fn screen_to_client(hwnd: isize, x: i32, y: i32) -> Option<(i32, i32)> {
    imp::screen_to_client(hwnd, x, y)
}

/// Asks the OS to put a window in front. False when it refused or cannot.
pub fn set_foreground(hwnd: isize) -> bool {
    imp::set_foreground(hwnd)
}

/// Whether a window is the foreground one.
pub fn is_foreground(hwnd: isize) -> bool {
    imp::is_foreground(hwnd)
}

pub fn is_minimized(hwnd: isize) -> bool {
    imp::is_minimized(hwnd)
}

pub fn is_maximized(hwnd: isize) -> bool {
    imp::is_maximized(hwnd)
}

/// Shows or hides a window.
pub fn show(hwnd: isize, how: Show) {
    imp::show(hwnd, how)
}

/// Whether a window has the tool-window style (no taskbar button, no Alt+Tab entry).
pub fn tool_style(hwnd: isize) -> bool {
    imp::tool_style(hwnd)
}

/// Sets or clears the tool-window style.
pub fn set_tool_style(hwnd: isize, want: bool) {
    imp::set_tool_style(hwnd, want)
}

/// Makes a popup click-through, never active, and out of the taskbar.
pub fn set_overlay_style(hwnd: isize) {
    imp::set_overlay_style(hwnd)
}

/// The work area and the whole of the monitor a window is on, in pixels.
pub fn monitor_rects(hwnd: isize) -> Option<(Rect, Rect)> {
    imp::monitor_rects(hwnd)
}

/// A window's restore placement: (show command, flags, normal rect as left/top/right/bottom, shift of
/// the work area from the monitor origin). The shift is zero for a tool window, whose placement is
/// in screen coordinates already.
pub fn placement(hwnd: isize) -> Option<(u32, u32, [i32; 4], (i32, i32))> {
    imp::placement(hwnd)
}

/// Flashes the window's taskbar button: until the window is in front, or once.
pub fn flash_taskbar(hwnd: isize, until_front: bool) {
    imp::flash_taskbar(hwnd, until_front)
}
