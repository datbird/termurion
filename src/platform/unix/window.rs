//! Native window handles do not exist for an app window on Unix (winit gives no HWND), so the app
//! has none to pass in. These answer "nothing" for a handle they are handed anyway.

use winit::window::Window;

use super::super::window::Show;
use crate::layout::Rect;

pub fn hwnd_of(_window: &Window) -> Option<isize> {
    None
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

pub fn set_foreground(_hwnd: isize) -> bool {
    false
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

pub fn monitor_rects(_hwnd: isize) -> Option<(Rect, Rect)> {
    None
}

pub fn placement(_hwnd: isize) -> Option<(u32, u32, [i32; 4], (i32, i32))> {
    None
}
