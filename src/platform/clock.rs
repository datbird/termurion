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

/// Plays a named system sound through the judge: None when it is refused or this OS has none, else
/// whether it started (false while another sound plays).
pub fn play_named(name: &str) -> Option<bool> {
    imp::play_named(name)
}

/// The OS's best date pattern (ICU's skeleton match) for `template` in the user's locale; None when
/// the OS has none, and the caller uses the template as written.
pub fn best_date_pattern(template: &str) -> Option<String> {
    imp::best_date_pattern(template)
}

/// This computer's time zone name right now, as the OS gives it ("CST", or Windows' own wording).
pub fn zone_name() -> String {
    imp::zone_name()
}

/// The local time now with its weekday and milliseconds: year, month, day, weekday (0 is Sunday), hour,
/// minute, second, millisecond.
pub fn local_now() -> (u16, u16, u16, u16, u16, u16, u16, u16) {
    imp::local_now()
}

/// `local_at` for a signed Unix time (an instant before 1970 converts where the OS can hold it).
pub fn local_at_signed(secs: i64) -> Option<Local> {
    imp::local_at_signed(secs)
}
