//! Desktop notifications and the resident tray icon. Windows uses a notification area balloon;
//! macOS uses UserNotifications from the app bundle; Linux has no backend yet and every show reports Unsupported.

#[cfg(target_os = "macos")]
use super::macos::notify as imp;
#[cfg(all(unix, not(target_os = "macos")))]
use super::unix::notify as imp;
#[cfg(windows)]
use super::windows::notify as imp;

pub use imp::{clear_if_shown, on_click, on_taskbar_created, on_tray, remove, set_resident, show_with, shown_count, window_handle};

/// Whether the tray menu opens at the icon itself (the macOS menu bar status item) rather than on
/// a window of Termurion's.
pub const MENU_AT_ICON: bool = imp::MENU_AT_ICON;

/// Whether the balloon shown when `shown_count()` was `at` is still the one on screen, now that
/// it is `now`: nothing was shown since (whoever showed it).
pub fn still_shown(at: u64, now: u64) -> bool {
    at == now
}

/// Tells the backend whether Termurion is the active app (macOS shows banners only then).
pub fn set_active(on: bool) {
    #[cfg(target_os = "macos")]
    imp::set_active(on);
    #[cfg(not(target_os = "macos"))]
    let _ = on;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The resident icon is the notification area icon on Windows and the menu bar status item on
    /// macOS, under the one rule (`resident::icon_wanted`): resident with no window, or 13g's
    /// StatusBarIcon in UI element mode. Linux has none yet.
    #[test]
    fn the_resident_icon_shows_under_the_same_rule_on_each_os() {
        use crate::resident::icon_wanted;
        assert!(icon_wanted(false, true), "resident with no window");
        assert!(icon_wanted(true, false), "13g's StatusBarIcon");
        assert!(!icon_wanted(false, false), "a window is open and no StatusBarIcon");
        assert_eq!(MENU_AT_ICON, cfg!(target_os = "macos"), "the menu opens at the menu bar item on macOS only");
        let mac = include_str!("macos/notify.rs");
        let set = mac.split("pub fn set_resident").nth(1).and_then(|r| r.split("\n}").next()).expect("set_resident");
        assert!(set.contains("status_item::set(on)"), "macOS puts the item up and takes it down");
        let rm = mac.split("pub fn remove").nth(1).and_then(|r| r.split("\n}").next()).expect("remove");
        assert!(rm.contains("status_item::set(false)"), "the quit takes it down");
    }

    /// Off the main thread the item is refused, not a crash (AppKit is main thread only).
    #[test]
    fn the_status_item_refuses_off_the_main_thread() {
        // Only here: on Windows this would really add the icon.
        if MENU_AT_ICON {
            let r = std::thread::spawn(|| set_resident(true)).join().expect("no panic");
            assert!(r.is_err());
        }
    }
}
