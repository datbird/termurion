//! Native window handles do not exist for an app window on Unix (winit gives no HWND), so the app
//! has none to pass in. These answer "nothing" for a handle they are handed anyway.

use winit::window::Window;

use super::super::window::{Flash, Show};
use crate::layout::Rect;

pub fn hwnd_of(_window: &Window) -> Option<isize> {
    None
}

/// The NSView on macOS. On Linux a per-window token: the X window id on X11, the winit window id
/// on Wayland (which has no number a client can read). Menus and the hotkey window key on it.
pub fn view_of(window: &Window) -> Option<isize> {
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    #[cfg(not(target_os = "macos"))]
    {
        use super::global_keys::{note_session, SessionKind};
        match window.window_handle().ok()?.as_raw() {
            RawWindowHandle::Xlib(_) | RawWindowHandle::Xcb(_) => note_session(SessionKind::X11),
            RawWindowHandle::Wayland(_) => note_session(SessionKind::Wayland),
            _ => {}
        }
    }
    match window.window_handle().ok()?.as_raw() {
        #[cfg(target_os = "macos")]
        RawWindowHandle::AppKit(h) => Some(h.ns_view.as_ptr() as isize),
        #[cfg(not(target_os = "macos"))]
        RawWindowHandle::Xlib(h) => Some(h.window as isize),
        #[cfg(not(target_os = "macos"))]
        RawWindowHandle::Xcb(h) => Some(h.window.get() as isize),
        #[cfg(not(target_os = "macos"))]
        RawWindowHandle::Wayland(_) => Some(u64::from(window.id()) as isize),
        _ => None,
    }
}

/// Brings `window` front with the activation token a notification click carried (Wayland);
/// false when there is none to use or the window is not a Wayland one, and the caller raises it
/// the usual way. macOS has no tokens.
pub fn activate(window: &Window, token: Option<&str>) -> Result<bool, String> {
    #[cfg(not(target_os = "macos"))]
    if let Some(t) = token {
        return super::super::linux::activation::activate(window, t);
    }
    let _ = (window, token);
    Ok(false)
}

/// Lets go of the Wayland activation binding (macOS keeps nothing).
pub fn forget_activation() {
    #[cfg(not(target_os = "macos"))]
    super::super::linux::activation::forget();
}

/// winit has no global pointer query; callers with a window use its cursor events instead.
pub fn cursor_pos() -> Option<(i32, i32)> {
    None
}

pub fn root_at(_x: i32, _y: i32) -> Option<isize> {
    None
}

pub fn screen_to_client(_hwnd: isize, _x: i32, _y: i32) -> Option<(i32, i32)> {
    None
}

/// macOS activates the app of a negative handle (another app) or makes our window key; X11 asks
/// the window manager to activate the window (the hotkey window hands focus back this way).
pub fn set_foreground(_hwnd: isize) -> bool {
    #[cfg(target_os = "macos")]
    return crate::platform::macos::hotkey::bring_front(_hwnd);
    #[cfg(not(target_os = "macos"))]
    crate::platform::linux::hotkey::activate(_hwnd)
}

pub fn is_foreground(_hwnd: isize) -> bool {
    false
}

pub fn is_minimized(_hwnd: isize) -> bool {
    false
}

pub fn is_maximized(_hwnd: isize) -> bool {
    false
}

pub fn show(_hwnd: isize, _how: Show) {}

pub fn tool_style(_hwnd: isize) -> bool {
    false
}

pub fn set_tool_style(_hwnd: isize, _want: bool) {}

pub fn set_overlay_style(_hwnd: isize) {}

/// The work area and whole area of the window's screen in pixels, top left origin (macOS only).
pub fn monitor_rects(_view: isize) -> Option<(Rect, Rect)> {
    #[cfg(target_os = "macos")]
    return crate::platform::macos::hotkey::monitor_rects(_view);
    #[cfg(not(target_os = "macos"))]
    None
}

pub fn placement(_hwnd: isize) -> Option<(u32, u32, [i32; 4], (i32, i32))> {
    None
}

pub fn flash(_hwnd: isize, _how: Flash) {}

pub fn is_arranged(_hwnd: isize) -> bool {
    false
}
