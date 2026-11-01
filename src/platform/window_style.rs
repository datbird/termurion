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
