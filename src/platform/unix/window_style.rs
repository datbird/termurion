//! Window styling on Unix. Most of it has no Unix meaning, so those calls leave the window alone.

use winit::window::{Window, WindowAttributes};

/// The taskbar button is a Windows idea; the window manager here decides what a dock or panel shows.
pub fn skip_taskbar(attrs: WindowAttributes) -> WindowAttributes {
    attrs
}

/// There is no owner window handle on Unix; the window stays top level.
pub fn owner_window(attrs: WindowAttributes, _owner: isize) -> WindowAttributes {
    attrs
}

/// Popups are opened with `with_active(false)`; there is no extended style to set.
pub fn no_activate(_window: &Window) {}

pub fn show_no_activate(window: &Window) {
    window.set_visible(true);
}

/// The window manager draws its own corners.
pub fn round_corners(_window: &Window) {}

pub fn accent_color() -> Option<(u8, u8, u8)> {
    None
}

/// Unix has no taskbar button to toggle; the window manager decides what a dock or panel shows.
pub fn set_skip_taskbar(_window: &Window, _skip: bool) {}

/// An alpha surface needs no window flag on Unix.
pub fn no_redirection_bitmap(attrs: WindowAttributes) -> WindowAttributes {
    attrs
}
