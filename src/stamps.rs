//! Times as iTerm2 shows them: per-line timestamps and captions.

/// A local date and time.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Parts {
    pub year: u16,
    pub month: u16,
    pub day: u16,
    /// 0 is Sunday.
    pub weekday: u16,
    pub hour: u16,
    pub minute: u16,
    pub second: u16,
}

/// "HH:MM:SS".
pub fn hms(p: &Parts) -> String {
    format!("{:02}:{:02}:{:02}", p.hour, p.minute, p.second)
}

/// Unix seconds as local time (Windows' time zone rules, so summer time is right).
#[cfg(windows)]
pub fn local(secs: u64) -> Parts {
    use windows_sys::Win32::Foundation::{FILETIME, SYSTEMTIME};
    use windows_sys::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};
    // FILETIME counts 100 ns steps from 1601.
    let ticks = (secs + 11_644_473_600) * 10_000_000;
    let ft = FILETIME { dwLowDateTime: ticks as u32, dwHighDateTime: (ticks >> 32) as u32 };
    let mut utc: SYSTEMTIME = unsafe { std::mem::zeroed() };
    let mut here: SYSTEMTIME = unsafe { std::mem::zeroed() };
    let ok = unsafe { FileTimeToSystemTime(&ft, &mut utc) != 0 && SystemTimeToTzSpecificLocalTime(std::ptr::null(), &utc, &mut here) != 0 };
    if !ok {
        return Parts::default();
    }
    Parts {
        year: here.wYear,
        month: here.wMonth,
        day: here.wDay,
        weekday: here.wDayOfWeek,
        hour: here.wHour,
        minute: here.wMinute,
        second: here.wSecond,
    }
}

/// A time of day, "HH:MM:SS", in local time.
#[cfg(windows)]
pub fn clock_time(t: std::time::SystemTime) -> String {
    let secs = t.duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    hms(&local(secs))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_times_are_hh_mm_ss() {
        assert_eq!(hms(&Parts { hour: 9, minute: 5, second: 3, ..Default::default() }), "09:05:03");
        // 2026-09-24 12:00:00 UTC is that day (or the day either side) in any time zone.
        let p = local(1_790_251_200);
        assert_eq!((p.year, p.month), (2026, 9));
        assert!((23..=25).contains(&p.day), "{p:?}");
    }
}
