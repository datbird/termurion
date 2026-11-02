//! Window chrome that is the OS's own: the border color, the corner rounding, the menu bar's
//! attach state and theme, and the Theme guard. DWM and the menu bar's native handle are Windows
//! things; on Unix the window manager draws the frame and the menu is not attached to a handle yet.

#[cfg(unix)]
use super::unix::chrome as imp;
#[cfg(windows)]
use super::windows::chrome as imp;

/// Whether the menu bar and pop-up menus exist on this OS. The app asks before it builds one and
/// shows the reason where a menu was wanted.
pub fn available() -> Result<(), super::Unsupported> {
    imp::available()
}

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

pub use imp::Lift;

/// Shows the menu as a pop-up at `position` (the window's client pixels; None: at the pointer).
/// True when an item was picked, false when it was dismissed. The handle is a live window of this
/// thread, as the app gets it from `window::hwnd_of`.
pub fn popup<M: muda::ContextMenu + ?Sized>(menu: &M, hwnd: isize, position: Option<muda::dpi::Position>) -> bool {
    imp::popup(menu, hwnd, position)
}

/// Attaches `menu` as `hwnd`'s menu bar after taking `earlier` (the window's bar so far) off it.
/// The order matters: muda's drop of a menu clears the menu of every window it was on, so an
/// earlier bar dropped after the new one is attached leaves the window with no menu bar.
///
/// A bar for a window that is to hide it (`hide`: Minimal, Compact, full screen) is taken off
/// again at once, so it is never attached while the app believes it hidden.
pub fn replace_menu_bar(earlier: Option<muda::Menu>, menu: &muda::Menu, hwnd: isize, hide: bool) -> muda::Result<()> {
    imp::replace_menu_bar(earlier, menu, hwnd, hide)
}

/// Shows a tip beside the highlighted menu item after `delay` (Windows owner-draws its menus; the toolkit
/// that draws them elsewhere has no such hook, so this does nothing there).
pub fn install_menu_tips(hwnd: isize, delay: std::time::Duration) {
    imp::install_menu_tips(hwnd, delay)
}
