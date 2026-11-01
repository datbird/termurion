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

/// Unix seconds as local time: year, month, day, weekday (0 is Sunday), hour, minute, second.
/// None when the time does not convert.
pub fn local_at(secs: u64) -> Option<(u16, u16, u16, u16, u16, u16, u16)> {
    imp::local_at(secs)
}
