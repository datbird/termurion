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

/// Unix seconds as local time (the OS's time zone rules, so summer time is right).
pub fn local(secs: u64) -> Parts {
    match crate::platform::clock::local_at(secs) {
        Some((year, month, day, weekday, hour, minute, second)) => Parts { year, month, day, weekday, hour, minute, second },
        None => Parts::default(),
    }
}

/// A time of day, "HH:MM:SS", in local time.
pub fn clock_time(t: std::time::SystemTime) -> String {
    let secs = t.duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    hms(&local(secs))
}

/// A local time as the tuple the platform formatting calls read.
fn tuple(p: &Parts) -> crate::platform::clock::Local {
    (p.year, p.month, p.day, p.weekday, p.hour, p.minute, p.second)
}

/// The locale's short date of a local date.
pub fn short_date(p: &Parts) -> String {
    crate::platform::clock::short_date(tuple(p))
}

/// The locale's time without seconds of a local time.
pub fn short_time(p: &Parts) -> String {
    crate::platform::clock::short_time(tuple(p))
}

/// The locale's time with seconds of a local time.
pub fn long_time(p: &Parts) -> String {
    crate::platform::clock::long_time(tuple(p))
}

/// "MMM d, yyyy" with English month names, as Command Info's date is in iTerm2.
pub fn month_day_year(p: &Parts) -> String {
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let month = MONTHS.get((p.month as usize).wrapping_sub(1)).copied().unwrap_or("?");
    format!("{month} {}, {}", p.day, p.year)
}

/// A signed distance from the baseline as the margin shows it with relative timestamps
/// (`highResolutionCompactRelativeTimeStringFromSeconds:`, `NSDateFormatterExtras.m:298-398`):
/// "Baseline" at zero, "+1.250s" under 10 seconds, whole seconds under a minute, then minutes,
/// hours, and two of year, month, week, day, hour, minute, second from a day on.
pub fn relative(seconds: f64) -> String {
    let interval = seconds.abs();
    if interval == 0.0 || seconds.is_nan() {
        return "Baseline".to_string();
    }
    let sign = if seconds < 0.0 { "-" } else { "+" };
    if interval < 10.0 {
        return format!("{sign}{interval:.3}s");
    }
    if interval < 60.0 {
        return format!("{sign}{}s", interval as i64);
    }
    let whole = interval.min(i64::MAX as f64) as i64;
    if interval < 3600.0 {
        let (m, s) = (whole / 60, whole % 60);
        return if s == 0 { format!("{sign}{m}m") } else { format!("{sign}{m}m{s}s") };
    }
    if interval < 86_400.0 {
        let (h, m, s) = (whole / 3600, (whole % 3600) / 60, whole % 60);
        let mut out = format!("{sign}{h}h");
        if m > 0 {
            out.push_str(&format!("{m}m"));
        }
        if s > 0 {
            out.push_str(&format!("{s}s"));
        }
        return out;
    }
    const YEAR: i64 = 31_536_000;
    const MONTH: i64 = 2_592_000;
    const WEEK: i64 = 604_800;
    const DAY: i64 = 86_400;
    let vals = [whole / YEAR, (whole % YEAR) / MONTH, (whole % MONTH) / WEEK, (whole % WEEK) / DAY, (whole % DAY) / 3600, (whole % 3600) / 60, whole % 60];
    let units = ["y", "mo", "w", "d", "h", "m", "s"];
    let mut out = String::new();
    let mut parts = 0;
    for (v, u) in vals.iter().zip(units) {
        if *v > 0 && parts < 2 {
            out.push_str(&format!("{v}{u}"));
            parts += 1;
        }
    }
    if out.is_empty() {
        out.push_str("0s");
    }
    format!("{sign}{out}")
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

    /// `NSDateFormatterExtras.m:298-398`.
    #[test]
    fn relative_timestamps_follow_iterm2() {
        assert_eq!(relative(0.0), "Baseline");
        assert_eq!(relative(1.25), "+1.250s");
        assert_eq!(relative(-9.9996), "-10.000s");
        assert_eq!(relative(10.0), "+10s");
        assert_eq!(relative(-59.9), "-59s");
        assert_eq!(relative(60.0), "+1m");
        assert_eq!(relative(303.0), "+5m3s");
        assert_eq!(relative(3600.0), "+1h");
        assert_eq!(relative(7500.0), "+2h5m");
        assert_eq!(relative(3661.0), "+1h1m1s");
        assert_eq!(relative(86_400.0 * 3.0 + 4.0 * 3600.0), "+3d4h");
        assert_eq!(relative(86_400.0), "+1d");
        assert_eq!(relative(31_536_000.0 + 2.0 * 2_592_000.0), "+1y2mo");
        // iTerm2 reduces the week by the month, not the year, so a year and three days reads as a year and a week.
        assert_eq!(relative(-(31_536_000.0 + 86_400.0 * 3.0 + 5.0)), "-1y1w");
    }

    #[test]
    fn block_width_is_the_widest_label_plus_a_cell() {
        let labels = vec![(0, "14:02:09".to_string()), (1, "Sat 14:02:09".to_string())];
        assert_eq!(block_width(10.0, &labels), 130.0);
        assert_eq!(block_width(10.0, &[]), 0.0);
    }
}
