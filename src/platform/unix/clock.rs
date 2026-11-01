//! The local clock and the alert sound.

pub fn local_time() -> (u16, u16, u16, u16, u16, u16) {
    let now = unsafe { libc::time(std::ptr::null_mut()) };
    let mut t: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&now, &mut t) };
    ((t.tm_year + 1900) as u16, (t.tm_mon + 1) as u16, t.tm_mday as u16, t.tm_hour as u16, t.tm_min as u16, t.tm_sec as u16)
}

/// No system sound call yet (P1 replaces this). A GUI app's stderr goes nowhere, so the beep is
/// logged once through `dlog` as unsupported instead of being dropped.
pub fn beep() {
    static LOGGED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if !LOGGED.swap(true, std::sync::atomic::Ordering::Relaxed) {
        crate::dlog!("System beep is not available on this platform yet");
    }
}

/// Unix seconds as local time: year, month, day, weekday (0 is Sunday), hour, minute, second.
/// None when the time does not convert.
pub fn local_at(secs: u64) -> Option<(u16, u16, u16, u16, u16, u16, u16)> {
    let at = libc::time_t::try_from(secs).ok()?;
    let mut t: libc::tm = unsafe { std::mem::zeroed() };
    if unsafe { libc::localtime_r(&at, &mut t) }.is_null() {
        return None;
    }
    Some(((t.tm_year + 1900) as u16, (t.tm_mon + 1) as u16, t.tm_mday as u16, t.tm_wday as u16, t.tm_hour as u16, t.tm_min as u16, t.tm_sec as u16))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_at_is_near_the_utc_day() {
        // 2026-09-24 12:00:00 UTC is that day (or the day either side) in any time zone.
        let (y, m, d, ..) = local_at(1_790_251_200).unwrap();
        assert_eq!((y, m), (2026, 9));
        assert!((23..=25).contains(&d), "{d}");
    }
}
