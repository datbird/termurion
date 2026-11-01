//! Window chrome that is the OS's own: the border color, the corner rounding, the menu bar's
//! attach state and theme, and the Theme guard. DWM and the menu bar's native handle are Windows
//! things; on Unix the window manager draws the frame and the menu is not attached to a handle yet.

#[cfg(unix)]
use super::unix::chrome as imp;
#[cfg(windows)]
use super::windows::chrome as imp;

/// Gives the window's frame a border color (a COLORREF); a no-op where the OS draws the border.
pub fn set_border(hwnd: isize, color: u32) {
    imp::set_border(hwnd, color)
}

/// Sets how the window's corners are rounded (the DWM corner preference).
pub fn set_corners(hwnd: isize, preference: i32) {
    imp::set_corners(hwnd, preference)
}

/// How far the outer frame reaches past the visible one at the top and the bottom.
pub fn invisible_top_bottom(hwnd: isize) -> (f32, f32) {
    imp::invisible_top_bottom(hwnd)
}

/// Takes the menu bar off the window.
pub fn hide_menu(menu: &muda::Menu, hwnd: isize) -> Result<(), String> {
    imp::hide_menu(menu, hwnd)
}

/// Puts the menu bar back on the window.
pub fn show_menu(menu: &muda::Menu, hwnd: isize) -> Result<(), String> {
    imp::show_menu(menu, hwnd)
}

/// Whether the window has a menu bar attached right now.
pub fn menu_attached(hwnd: isize) -> bool {
    imp::menu_attached(hwnd)
}

/// The menu bar and drop-down menus take the dark or light mode. Err is the menu library's refusal.
pub fn apply_menu_theme(hwnd: isize, forced: Option<bool>, high_contrast: bool, bar: Option<&muda::Menu>) -> Result<(), String> {
    imp::apply_menu_theme(hwnd, forced, high_contrast, bar)
}

/// Asks `wake` to run after a system setting change, so the forced title bar is applied again.
pub fn guard_theme(hwnd: isize, wake: Box<dyn Fn()>) {
    imp::guard_theme(hwnd, wake)
}

/// The queued theme reset was handled.
pub fn theme_reset_handled() {
    imp::theme_reset_handled()
}

/// Shift is held right now (Unix: not readable, so false).
pub fn shift_down() -> bool {
    imp::shift_down()
}
