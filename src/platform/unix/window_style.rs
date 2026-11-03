//! Window styling on Unix. Most of it has no Unix meaning, so those calls leave the window alone.

use winit::window::{Window, WindowAttributes};

/// The taskbar button is a Windows idea; the window manager here decides what a dock or panel shows.
pub fn skip_taskbar(attrs: WindowAttributes) -> WindowAttributes {
    attrs
}

/// macOS: the window becomes a child of the owner's view window (winit's addChildWindow), so it
/// stays in front of its terminal window. Elsewhere on Unix there is no owner handle.
#[cfg(target_os = "macos")]
pub fn owner_window(attrs: WindowAttributes, owner: isize) -> WindowAttributes {
    use winit::raw_window_handle::{AppKitWindowHandle, RawWindowHandle};
    let Some(view) = std::ptr::NonNull::new(owner as *mut std::ffi::c_void) else { return attrs };
    // SAFETY: `owner` is the live NSView of the terminal window (`view_of`), which outlives this call.
    // Close only: a minimized manager would leave its terminal window with no sign of it.
    let attrs = attrs.with_enabled_buttons(winit::window::WindowButtons::CLOSE);
    unsafe { attrs.with_parent_window(Some(RawWindowHandle::AppKit(AppKitWindowHandle::new(view)))) }
}

#[cfg(not(target_os = "macos"))]
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
