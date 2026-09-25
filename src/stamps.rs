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

/// iTerm2's timestamp text for a line written `age` seconds ago at local time `p`: the time
/// of day under a day, then the weekday, then the month and day, then the year too.
pub fn label(age: u64, p: &Parts) -> String {
    const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    const DAY: u64 = 86_400;
    let month = MONTHS.get((p.month as usize).wrapping_sub(1)).copied().unwrap_or("?");
    match age {
        a if a < DAY => hms(p),
        a if a < 6 * DAY => format!("{} {}", DAYS.get(p.weekday as usize).copied().unwrap_or("?"), hms(p)),
        a if a < 180 * DAY => format!("{month} {} {}", p.day, hms(p)),
        _ => format!("{} {month} {} {}", p.year, p.day, hms(p)),
    }
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
    fn labels_scale_with_age() {
        let p = Parts { year: 2026, month: 3, day: 7, weekday: 6, hour: 14, minute: 2, second: 9 };
        let day = 86_400;
        assert_eq!(label(60, &p), "14:02:09");
        assert_eq!(label(2 * day, &p), "Sat 14:02:09");
        assert_eq!(label(30 * day, &p), "Mar 7 14:02:09");
        assert_eq!(label(400 * day, &p), "2026 Mar 7 14:02:09");
    }

    #[test]
    fn clock_times_are_hh_mm_ss() {
        assert_eq!(hms(&Parts { hour: 9, minute: 5, second: 3, ..Default::default() }), "09:05:03");
        // 2026-09-24 12:00:00 UTC is that day (or the day either side) in any time zone.
        let p = local(1_790_251_200);
        assert_eq!((p.year, p.month), (2026, 9));
        assert!((23..=25).contains(&p.day), "{p:?}");
    }
}
