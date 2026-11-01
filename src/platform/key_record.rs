//! What the shortcut recorder needs from the keyboard layer.

use crate::menus::ChordKey;

#[cfg(unix)]
use super::unix::key_record as imp;
#[cfg(windows)]
use super::windows::key_record as imp;

/// The chord key a physical key types, or None when it has none the table knows.
pub fn chord_key(key: winit::keyboard::PhysicalKey) -> Option<ChordKey> {
    imp::chord_key(key)
}

/// The recorder is capturing a keystroke. On Windows this pauses the system-wide hotkeys; Unix has
/// no keyboard hook to pause, so it is a no-op there.
pub fn set_recording(on: bool) {
    imp::set_recording(on)
}

/// The settings window counts as Termurion for modifier remapping. Windows only (the hook); a no-op on Unix.
pub fn set_settings_window(hwnd: isize) {
    imp::set_settings_window(hwnd)
}
