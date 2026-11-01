//! Desktop notifications and the resident tray icon. Windows uses a notification area balloon;
//! Unix has no backend yet and every show reports Unsupported.

#[cfg(unix)]
use super::unix::notify as imp;
#[cfg(windows)]
use super::windows::notify as imp;

pub use imp::{clear_if_shown, on_click, on_taskbar_created, on_tray, remove, set_resident, show, show_with, shown_count, window_handle};

/// Whether the balloon shown when `shown_count()` was `at` is still the one on screen, now that
/// it is `now`: nothing was shown since (whoever showed it).
pub fn still_shown(at: u64, now: u64) -> bool {
    at == now
}
