//! The hotkey window's native window calls: cloaking, the fade, the front window. A window handle
//! is the HWND on Windows and the NSView on macOS (`window::view_of`); on macOS a negative handle
//! from `foreground` is minus the pid of another app. Linux has no such handle (`window::view_of` is
//! None) and no system-wide hotkey, so the app never reaches these with one.

#[cfg(target_os = "macos")]
use super::macos::hotkey as imp;
#[cfg(all(unix, not(target_os = "macos")))]
use super::unix::hotkey as imp;
#[cfg(windows)]
use super::windows::hotkey as imp;

pub use imp::{available, cloak, foreground, is_window, forget, join_active_space, order_front, ours, set_alpha};
