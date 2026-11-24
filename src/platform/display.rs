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

/// The primary monitor's height in pixels (it sits at the virtual screen's origin). None when this
/// OS cannot tell without a window (Unix), so a caller skips what needs it instead of guessing.
pub fn primary_height() -> Option<f64> {
    imp::primary_height()
}

/// The smallest outer size Windows lets a person drag a titled window to, in pixels (the title bar's
/// buttons need that much). None where the OS gives no such floor (Unix).
pub fn min_track_size() -> Option<(f64, f64)> {
    imp::min_track_size()
}

/// Each screen's visible frame in points, top left origin (macOS: the menu bar and Dock left out).
/// None elsewhere, and off the main thread.
pub fn visible_screens() -> Option<Vec<(f64, f64, f64, f64)>> {
    #[cfg(unix)]
    {
        imp::visible_screens()
    }
    #[cfg(not(unix))]
    {
        None
    }
}

/// The X11 desktop's work area (`_NET_WORKAREA`) in pixels, panels and docks left out. None on any
/// other system, under Wayland, or when the window manager publishes none.
pub fn desktop_work_area() -> Option<(f64, f64, f64, f64)> {
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        crate::platform::linux::workarea::work_area()
    }
    #[cfg(not(all(unix, not(target_os = "macos"))))]
    {
        None
    }
}

/// Each monitor's work area (without the taskbar) as x, y, width, height in virtual-screen pixels.
/// Empty where the OS gives no monitor list without a window (Unix: winit lists monitors per window).
pub fn work_areas() -> Vec<(f64, f64, f64, f64)> {
    imp::work_areas()
}
