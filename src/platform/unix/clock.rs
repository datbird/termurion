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

/// No separate error sound is played yet; this is the same logged notice as `beep`.
pub fn beep_error() {
    beep()
}

/// This thread's CPU time.
pub fn thread_cpu_time() -> std::time::Duration {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut ts) };
    std::time::Duration::new(ts.tv_sec as u64, ts.tv_nsec as u32)
}

type Local = (u16, u16, u16, u16, u16, u16, u16);

/// `strftime` of a local time with `fmt` (the C locale's or the process locale's own pattern).
fn strftime(p: Local, fmt: &[u8]) -> String {
    let mut t: libc::tm = unsafe { std::mem::zeroed() };
    t.tm_year = p.0 as i32 - 1900;
    t.tm_mon = p.1 as i32 - 1;
    t.tm_mday = p.2 as i32;
    t.tm_wday = p.3 as i32;
    t.tm_hour = p.4 as i32;
    t.tm_min = p.5 as i32;
    t.tm_sec = p.6 as i32;
    let mut buf = [0u8; 128];
    let n = unsafe { libc::strftime(buf.as_mut_ptr().cast(), buf.len(), fmt.as_ptr().cast(), &t) };
    String::from_utf8_lossy(&buf[..n]).into_owned()
}

/// The locale's date (`%x`).
pub fn short_date(p: Local) -> String {
    strftime(p, b"%x\0")
}

/// Hours and minutes (`%R`).
pub fn short_time(p: Local) -> String {
    strftime(p, b"%R\0")
}

/// Hours, minutes and seconds (`%T`).
pub fn long_time(p: Local) -> String {
    strftime(p, b"%T\0")
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
