//! The local wall clock.

#[cfg(unix)]
use super::unix::clock as imp;
#[cfg(windows)]
use super::windows::clock as imp;

/// The local time: year, month, day, hour, minute, second.
pub fn local_time() -> (u16, u16, u16, u16, u16, u16) {
    imp::local_time()
}

/// The system alert sound.
pub fn beep() {
    imp::beep()
}
