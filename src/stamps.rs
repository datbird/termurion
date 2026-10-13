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

/// A local time as the SYSTEMTIME the formatting calls read.
#[cfg(windows)]
fn systemtime(p: &Parts) -> windows_sys::Win32::Foundation::SYSTEMTIME {
    windows_sys::Win32::Foundation::SYSTEMTIME { wYear: p.year, wMonth: p.month, wDayOfWeek: p.weekday, wDay: p.day, wHour: p.hour, wMinute: p.minute, wSecond: p.second, wMilliseconds: 0 }
}

/// A formatting call's UTF-16 buffer as text; a call that failed (0) or gave nothing is "".
#[cfg(windows)]
fn wide_text(buf: &[u16], n: i32) -> String {
    if n > 1 {
        String::from_utf16_lossy(&buf[..n as usize - 1])
    } else {
        String::new()
    }
}

/// The locale's short date (Windows' `DATE_SHORTDATE`) of a local date.
#[cfg(windows)]
pub fn short_date(p: &Parts) -> String {
    use windows_sys::Win32::Globalization::{GetDateFormatEx, DATE_SHORTDATE};
    let (st, mut buf) = (systemtime(p), [0u16; 128]);
    let n = unsafe { GetDateFormatEx(std::ptr::null(), DATE_SHORTDATE, &st, std::ptr::null(), buf.as_mut_ptr(), buf.len() as i32, std::ptr::null()) };
    wide_text(&buf, n)
}

/// The locale's time without seconds (`TIME_NOSECONDS`) of a local time.
#[cfg(windows)]
pub fn short_time(p: &Parts) -> String {
    use windows_sys::Win32::Globalization::{GetTimeFormatEx, TIME_NOSECONDS};
    let (st, mut buf) = (systemtime(p), [0u16; 128]);
    let n = unsafe { GetTimeFormatEx(std::ptr::null(), TIME_NOSECONDS, &st, std::ptr::null(), buf.as_mut_ptr(), buf.len() as i32) };
    wide_text(&buf, n)
}

/// The locale's time with seconds (`GetTimeFormatEx` with no flags) of a local time.
#[cfg(windows)]
pub fn long_time(p: &Parts) -> String {
    use windows_sys::Win32::Globalization::GetTimeFormatEx;
    let (st, mut buf) = (systemtime(p), [0u16; 128]);
    let n = unsafe { GetTimeFormatEx(std::ptr::null(), 0, &st, std::ptr::null(), buf.as_mut_ptr(), buf.len() as i32) };
    wide_text(&buf, n)
}

/// "MMM d, yyyy" with English month names, as Command Info's date is in iTerm2.
pub fn month_day_year(p: &Parts) -> String {
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let month = MONTHS.get((p.month as usize).wrapping_sub(1)).copied().unwrap_or("?");
    format!("{month} {}, {}", p.day, p.year)
}

/// iTerm2's right extra for Adjacent timestamps, in points (`PTYSession.m`,
/// `desiredRightExtraForProfile:session:`).
pub const ADJACENT_POINTS: f32 = 100.0;

/// How a pane shows its timestamps (the "Timestamps Style" values in Termurion's schema).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// 1: over the text.
    Over,
    /// 2: over the text, only while the pointer is near the right edge.
    Hover,
    /// 3: in a column right of the text.
    Adjacent,
}

impl Mode {
    /// The stored style; a value outside 1 to 3 is iTerm2's default, Over.
    pub fn from_style(v: i64) -> Mode {
        match v {
            2 => Mode::Hover,
            3 => Mode::Adjacent,
            _ => Mode::Over,
        }
    }
}

/// The width a pane's grid gives up for its timestamps: only Adjacent takes room.
pub fn room(mode: Mode, scale: f32) -> f32 {
    if mode == Mode::Adjacent { ADJACENT_POINTS * scale } else { 0.0 }
}

/// The width of the label block: the widest label plus one cell of margin (iTerm2's
/// `frameForStringGivenWidth:`: the maximum label width plus the side margin).
pub fn block_width(cell_w: f32, labels: &[(usize, String)]) -> f32 {
    let widest = labels.iter().map(|(_, t)| t.chars().count()).max();
    widest.map_or(0.0, |n| (n as f32 + 1.0) * cell_w)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_info_dates_follow_iterm2() {
        let p = Parts { year: 2026, month: 10, day: 1, weekday: 4, hour: 15, minute: 4, second: 5 };
        assert_eq!(month_day_year(&p), "Oct 1, 2026");
        assert_eq!(month_day_year(&Parts { month: 1, day: 31, year: 1999, ..p }), "Jan 31, 1999");
        assert_eq!(month_day_year(&Parts { month: 13, ..p }), "? 1, 2026");
    }

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

    #[cfg(windows)]
    #[test]
    fn short_forms_follow_the_locale() {
        use windows_sys::Win32::Foundation::SYSTEMTIME;
        use windows_sys::Win32::Globalization::{GetDateFormatEx, GetTimeFormatEx, DATE_SHORTDATE, TIME_NOSECONDS};
        let p = Parts { year: 2026, month: 9, day: 27, weekday: 0, hour: 15, minute: 4, second: 59 };
        let st = SYSTEMTIME { wYear: 2026, wMonth: 9, wDayOfWeek: 0, wDay: 27, wHour: 15, wMinute: 4, wSecond: 59, wMilliseconds: 0 };
        let (mut date, mut time) = ([0u16; 128], [0u16; 128]);
        let (d, t) = unsafe {
            (
                GetDateFormatEx(std::ptr::null(), DATE_SHORTDATE, &st, std::ptr::null(), date.as_mut_ptr(), date.len() as i32, std::ptr::null()),
                GetTimeFormatEx(std::ptr::null(), TIME_NOSECONDS, &st, std::ptr::null(), time.as_mut_ptr(), time.len() as i32),
            )
        };
        assert!(d > 1 && t > 1);
        assert_eq!(short_date(&p), String::from_utf16_lossy(&date[..d as usize - 1]));
        assert_eq!(short_time(&p), String::from_utf16_lossy(&time[..t as usize - 1]));
        // Seconds are not shown, and a date that does not exist gives no text rather than a wrong one.
        assert_eq!(short_time(&p), short_time(&Parts { second: 0, ..p }));
        assert_eq!(short_date(&Parts { month: 13, ..p }), "");
    }

    #[test]
    fn adjacent_room_follows_iterm2() {
        assert_eq!(room(Mode::Adjacent, 1.5), 150.0);
        assert_eq!(room(Mode::Adjacent, 1.0), 100.0);
        assert_eq!(room(Mode::Over, 1.5), 0.0);
        assert_eq!(room(Mode::Hover, 1.5), 0.0);
        assert_eq!(Mode::from_style(1), Mode::Over);
        assert_eq!(Mode::from_style(2), Mode::Hover);
        assert_eq!(Mode::from_style(3), Mode::Adjacent);
        assert_eq!(Mode::from_style(0), Mode::Over);
        assert_eq!(Mode::from_style(9), Mode::Over);
    }

    #[test]
    fn block_width_is_the_widest_label_plus_a_cell() {
        let labels = vec![(0, "14:02:09".to_string()), (1, "Sat 14:02:09".to_string())];
        assert_eq!(block_width(10.0, &labels), 130.0);
        assert_eq!(block_width(10.0, &[]), 0.0);
    }
}
