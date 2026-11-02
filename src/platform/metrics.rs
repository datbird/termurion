//! The status bar's machine meters: processor, memory, network and battery readings.

#[cfg(unix)]
use super::unix::metrics as imp;
#[cfg(windows)]
use super::windows::metrics as imp;

pub use imp::{has_battery, SystemSamplers};
