//! Display changes and the window backdrop. Windows subclasses each window to hear monitor and
//! work-area changes and asks DWM for the acrylic backdrop; on Unix winit reports display changes
//! itself and the compositor owns blur, so these do nothing.

#[cfg(unix)]
use super::unix::display as imp;
#[cfg(windows)]
use super::windows::display as imp;

/// What to call (from the UI thread) when the monitors or a work area changed. Set once.
pub fn on_change(f: Box<dyn Fn() + Send + Sync>) {
    imp::on_change(f)
}

/// Starts watching a window (its handle) for display changes. False when no watch was installed.
pub fn watch(hwnd: isize) -> bool {
    imp::watch(hwnd)
}

/// Turns the system backdrop blur on or off. True when the system accepted it.
pub fn set_blur(hwnd: isize, on: bool) -> bool {
    imp::set_blur(hwnd, on)
}

/// Where a sheet drops from: the top of the owner window's client area, centered for a window `width`
/// wide (screen pixels). None when the OS places it (Unix) or the owner has no rect.
pub fn sheet_origin(owner: isize, width: u32) -> Option<(i32, i32)> {
    imp::sheet_origin(owner, width)
}
