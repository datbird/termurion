//! The caption strip of Minimal and Compact windows. Windows subclasses the window to take the title
//! bar away and to answer hit tests for the strip; Unix has no such subclass, so the window keeps
//! the frame the window manager draws and these leave it alone.

use crate::caption::{Event, Layout};
use crate::layout::Rect;

#[cfg(unix)]
use super::unix::caption as imp;
#[cfg(windows)]
use super::windows::caption as imp;

/// Takes the title bar away from a window; `wake` carries the strip's pointer events back to the app.
pub fn install(hwnd: isize, wake: Box<dyn Fn(isize, Event)>) {
    imp::install(hwnd, wake)
}

/// The window leaves Compact: the subclass goes.
pub fn remove(hwnd: isize) {
    imp::remove(hwnd)
}

/// Whether the window has the caption subclass.
pub fn installed(hwnd: isize) -> bool {
    imp::installed(hwnd)
}

/// The strip for the window's next hit tests (None layout: no strip now).
pub fn publish(hwnd: isize, layout: Option<Layout>, over: Vec<Rect>, cells: Vec<Rect>) {
    imp::publish(hwnd, layout, over, cells)
}

/// Runs `change` (which alters the frame) and keeps the client area the size it was.
pub fn keep_client(hwnd: isize, change: impl FnOnce()) {
    imp::keep_client(hwnd, change)
}

/// The resize border thickness along the top, in pixels.
pub fn frame_thickness(hwnd: isize) -> f32 {
    imp::frame_thickness(hwnd)
}

/// `Window::request_inner_size` for a terminal window (Windows corrects for the title bar the
/// subclass removes).
pub fn request_inner_size(window: &winit::window::Window, size: impl Into<winit::dpi::Size>) -> Option<winit::dpi::PhysicalSize<u32>> {
    imp::request_inner_size(window, size)
}

/// The client size for a DPI change, when the caption subclass decides it.
pub fn dpi_size(hwnd: isize) -> Option<winit::dpi::PhysicalSize<u32>> {
    imp::dpi_size(hwnd)
}
