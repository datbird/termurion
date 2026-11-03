//! Virtual desktops: Windows asks the shell's manager; Unix has none, which the app reads as "the
//! session has no desktop shell" and turns the desktop features off.

#[cfg(unix)]
use super::unix::desktops as imp;
#[cfg(windows)]
use super::windows::desktops as imp;

pub use imp::Manager;
