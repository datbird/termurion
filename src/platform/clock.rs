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

/// The error sound (Windows' hand icon sound).
pub fn beep_error() {
    imp::beep_error()
}

/// The CPU time this thread has used.
pub fn thread_cpu_time() -> std::time::Duration {
    imp::thread_cpu_time()
}

/// A local time as `local_at` gives it: year, month, day, weekday, hour, minute, second.
pub type Local = (u16, u16, u16, u16, u16, u16, u16);

/// The locale's short date of a local time, as the OS formats it.
pub fn short_date(p: Local) -> String {
    imp::short_date(p)
}

/// The locale's time without seconds of a local time.
pub fn short_time(p: Local) -> String {
    imp::short_time(p)
}

/// The locale's time with seconds of a local time.
pub fn long_time(p: Local) -> String {
    imp::long_time(p)
}
