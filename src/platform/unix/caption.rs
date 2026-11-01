//! No caption subclass on Unix: the window manager owns the frame, so the frame is never changed
//! and the client keeps its size by itself.

use crate::caption::{Event, Layout};
use crate::layout::Rect;

pub fn install(_hwnd: isize, _wake: Box<dyn Fn(isize, Event)>) {}

pub fn remove(_hwnd: isize) {}

pub fn installed(_hwnd: isize) -> bool {
    false
}

pub fn publish(_hwnd: isize, _layout: Option<Layout>, _over: Vec<Rect>, _cells: Vec<Rect>) {}

pub fn keep_client(_hwnd: isize, change: impl FnOnce()) {
    change()
}

/// The window manager draws the frame; there is no resize border to overhang.
pub fn frame_thickness(_hwnd: isize) -> f32 {
    0.0
}

pub fn request_inner_size(window: &winit::window::Window, size: impl Into<winit::dpi::Size>) -> Option<winit::dpi::PhysicalSize<u32>> {
    window.request_inner_size(size)
}

pub fn dpi_size(_hwnd: isize) -> Option<winit::dpi::PhysicalSize<u32>> {
    None
}
