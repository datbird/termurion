//! The hotkey window's native window calls: cloaking, the fade, the front window. A window handle
//! is the HWND on Windows. Unix has no such handle (`window::hwnd_of` is None) and no system-wide
//! hotkey (`keys::set_hotkeys` is Unsupported), so the app never reaches these with one.

#[cfg(unix)]
use super::unix::hotkey as imp;
#[cfg(windows)]
use super::windows::hotkey as imp;

pub use imp::{cloak, foreground, is_window, order_front, ours, set_alpha};
