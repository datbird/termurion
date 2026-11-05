//! Whether the screen locked since the last look, for the password manager (it locks with the screen).

#[cfg(target_os = "macos")]
use super::macos::screen_lock as imp;
#[cfg(target_os = "linux")]
use super::linux::screen_lock as imp;
#[cfg(windows)]
use super::windows::screen_lock as imp;

pub use imp::{take, watch, watched};
