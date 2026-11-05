//! The hotkey window's native window calls: cloaking, the fade, the front window. A window handle
//! is the HWND on Windows, the NSView on macOS and the X window id on X11 (`window::view_of`); on
//! macOS a negative handle from `foreground` is minus the pid of another app. On a Wayland session
//! `available` says no and the app never reaches the rest with a handle.

#[cfg(target_os = "macos")]
use super::macos::hotkey as imp;
#[cfg(all(unix, not(target_os = "macos")))]
use super::linux::hotkey as imp;
#[cfg(windows)]
use super::windows::hotkey as imp;

pub use imp::{available, cloak, foreground, is_window, forget, join_active_space, order_front, ours, set_alpha, take_focus};
