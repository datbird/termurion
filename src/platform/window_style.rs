//! Window styling that differs per OS: taskbar button, no-activate popups, rounded corners, accent.

use winit::window::{Window, WindowAttributes};

#[cfg(unix)]
use super::unix::window_style as imp;
#[cfg(windows)]
use super::windows::window_style as imp;

/// `attrs` with no taskbar button.
pub fn skip_taskbar(attrs: WindowAttributes) -> WindowAttributes {
    imp::skip_taskbar(attrs)
}

/// `attrs` owned by the window `owner` (a native handle): it stays above its owner and goes with it.
/// Unix has no owner handle to give, so the window is left a plain top-level one.
pub fn owner_window(attrs: WindowAttributes, owner: isize) -> WindowAttributes {
    imp::owner_window(attrs, owner)
}

/// Marks a popup so a click on it does not take the focus.
pub fn no_activate(window: &Window) {
    imp::no_activate(window)
}

/// Shows a hidden window without taking the focus.
pub fn show_no_activate(window: &Window) {
    imp::show_no_activate(window)
}

/// Rounds a borderless window's corners where the OS does not do it on its own.
pub fn round_corners(window: &Window) {
    imp::round_corners(window)
}

/// The system accent color as (r, g, b), or None when the OS has none to read.
pub fn accent_color() -> Option<(u8, u8, u8)> {
    imp::accent_color()
}

/// Shows or hides the window's taskbar button after the window exists.
pub fn set_skip_taskbar(window: &Window, skip: bool) {
    imp::set_skip_taskbar(window, skip)
}

/// `attrs` for a window that gets its alpha from the compositor (no redirection bitmap on Windows).
pub fn no_redirection_bitmap(attrs: WindowAttributes) -> WindowAttributes {
    imp::no_redirection_bitmap(attrs)
}

/// `attrs` with no taskbar button when `skip` is true, else unchanged.
pub fn skip_taskbar_if(attrs: WindowAttributes, skip: bool) -> WindowAttributes {
    if skip { skip_taskbar(attrs) } else { attrs }
}
