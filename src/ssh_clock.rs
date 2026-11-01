//! The Clock's server time (milestone 14c): the script that asks the remote for its clock,
//! its reply, and what the Clock shows for it (`CD:619-649`, `CLK:33-196`). The remote's
//! answer is hostile text: the number is range checked, the names are capped and cleaned.

use crate::statusbar_text::Civil;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// How often the Clock asks again (`CLK:75-78`).
pub const REFRESH: Duration = Duration::from_secs(15 * 60);

/// The Clock's globe: Segoe Fluent Icons U+E774 (Globe), after a space (Decision: no emoji,
/// the bar already draws Segoe Fluent glyphs).
pub const GLOBE: char = '\u{E774}';

/// The longest zone name kept.
pub const TZ_MAX: usize = 32;
/// The most of a remote's text an error message repeats.
const ECHO_MAX: usize = 120;
/// A server clock further than this from ours (a century) is not believed.
const OFFSET_MAX: f64 = 100.0 * 365.25 * 86400.0;
/// A UTC offset is within a day.
const ZONE_MAX: i32 = 24 * 3600;

/// What the remote told the Clock: how far its clock is ahead of ours, its zone's name, and
/// its UTC offset in seconds when it said (Termurion's third value, a Decision).
#[derive(Clone, Debug, PartialEq)]
pub struct Offset {
    pub offset: f64,
    pub tz: String,
    pub zone: Option<i32>,
}

/// The monotonic clock's zero: the first time anything asked.
pub(crate) fn epoch() -> Instant {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    *EPOCH.get_or_init(Instant::now)
}

/// Seconds on the monotonic clock (`NSDate.it_timeSinceBoot`): what the round trip is
/// measured with, never the wall clock.
pub fn monotonic(now: Instant) -> f64 {
    now.saturating_duration_since(epoch()).as_secs_f64()
}

/// The remote's clock script (`CD:619-630`) for a local wall clock reading; it returns three
/// lines: the difference, the zone name (iTerm2's two) and the UTC offset (Termurion's third).
pub fn script(local_now: f64) -> String {
    format!(
        "import time\nfrom datetime import datetime, timezone\n\ntz = time.tzname[time.daylight and time.localtime().tm_isdst]\ndelta = time.time() - {local_now}\nreturn f\"{{delta}}\\n{{tz}}\\n{{time.localtime().tm_gmtoff}}\""
    )
}

/// A remote's text made safe to repeat: controls, bidi and invisible characters become
/// spaces, the ends are trimmed and the length is capped.
fn tame(text: &str, cap: usize) -> String {
    crate::ssh_procs::clean(text, cap)
}

/// The reply of the script (`CD:630-647`): the offset is the difference less half the round
/// trip; a reply of another shape is invalid. Nothing here trusts the remote: a number that
/// is not finite or is a century off is invalid, a zone outside a day is left out.
pub fn parse_reply(ok: bool, output: &str, latency: f64) -> Result<Offset, String> {
    if !ok {
        return Err(format!("Failed to fetch time: {}", tame(output, ECHO_MAX)));
    }
    let invalid = || Err(format!("Invalid output: {}", tame(output, ECHO_MAX)));
    let lines: Vec<&str> = output.split('\n').collect();
    let diff = match lines.first().and_then(|l| l.trim().parse::<f64>().ok()) {
        Some(d) if lines.len() >= 2 && d.is_finite() => d,
        _ => return invalid(),
    };
    let latency = if latency.is_finite() { latency.max(0.0) } else { 0.0 };
    let offset = diff - latency / 2.0;
    if offset.abs() > OFFSET_MAX {
        return invalid();
    }
    let zone = lines.get(2).and_then(|l| l.trim().parse::<i32>().ok()).filter(|z| z.abs() <= ZONE_MAX);
    Ok(Offset { offset, tz: tame(lines[1], TZ_MAX), zone })
}

/// What the Clock draws for a reply and its two knobs (`CLK:155-196`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shown {
    /// Seconds added to the local time.
    pub shift: f64,
    /// The zone to format in, else the local one.
    pub zone: Option<i32>,
    /// Whether the globe follows the text.
    pub globe: bool,
}

/// The offset counts only when non-zero and "Show server time" is on, the zone only when
/// known and "Use server time zone" is on, and the globe shows when either was learned and
/// either knob is on.
pub fn shown(offset: f64, tz_known: bool, zone: Option<i32>, time_knob: bool, zone_knob: bool) -> Shown {
    Shown {
        shift: if offset != 0.0 && time_knob { offset } else { 0.0 },
        zone: if tz_known && zone_knob { zone } else { None },
        globe: (offset != 0.0 || tz_known) && (time_knob || zone_knob),
    }
}

/// What a pane's Clock does next.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refresh {
    Nothing,
    /// Local time (level below 2): the reply is reset.
    Local,
    /// Ask the remote.
    Fetch,
}

/// A pane's Clock: the remote's answer and when to ask again (`CLK:33-99`).
#[derive(Default)]
pub struct Clock {
    pub offset: f64,
    pub tz: Option<String>,
    pub zone: Option<i32>,
    level: i32,
    host: Option<String>,
    next: Option<Instant>,
}

impl Clock {
    fn reset(&mut self) {
        self.offset = 0.0;
        self.tz = None;
        self.zone = None;
    }

    /// Asks what to do at `now` for the pane's SSH level and host name: a change of either is
    /// due at once, then every `REFRESH`; below level 2 the Clock is local (once).
    pub fn due(&mut self, level: i32, host: Option<&str>, now: Instant) -> Refresh {
        let changed = level != self.level || host != self.host.as_deref();
        if changed {
            self.level = level;
            self.host = host.map(str::to_string);
        }
        if level < 2 {
            if changed || self.next.is_none() {
                self.next = Some(now + REFRESH);
                self.reset();
                return Refresh::Local;
            }
            return Refresh::Nothing;
        }
        if changed || self.next.is_none_or(|due| now >= due) {
            self.next = Some(now + REFRESH);
            return Refresh::Fetch;
        }
        Refresh::Nothing
    }

    /// When the next fetch is due, at level 2 only (below it there is nothing to wait for).
    pub fn next_due(&self) -> Option<Instant> {
        if self.level >= 2 { self.next } else { None }
    }

    /// Whether the Clock holds anything of a remote (a reset is still owed when it does).
    pub fn is_remote(&self) -> bool {
        self.level >= 2 || self.offset != 0.0 || self.tz.is_some()
    }

    /// A reply: an error resets; a reply that arrives after the level dropped (the remote is
    /// gone) is ignored.
    pub fn set(&mut self, reply: Result<Offset, String>) {
        match reply {
            Ok(o) if self.level >= 2 => {
                self.offset = o.offset;
                self.tz = Some(o.tz).filter(|t| !t.is_empty());
                self.zone = o.zone;
            }
            Ok(_) => {}
            Err(_) => self.reset(),
        }
    }
}

/// The civil time at `unix` seconds in the zone `zone` (a UTC offset), else in this computer's
/// zone.
#[cfg(windows)]
pub fn civil_at(unix: f64, zone: Option<i32>) -> Civil {
    let whole = unix.floor();
    let millis = ((unix - whole) * 1000.0) as u32;
    let secs = whole as i64;
    if let Some(z) = zone {
        return Civil::from_unix(secs.saturating_add(i64::from(z)), millis.min(999));
    }
    local_civil(secs, millis.min(999)).unwrap_or_else(|| Civil::from_unix(secs, millis.min(999)))
}

/// This computer's local time for a UTC instant (`SystemTimeToTzSpecificLocalTime`); None when
/// the instant is outside what Windows can hold.
#[cfg(windows)]
fn local_civil(secs: i64, millis: u32) -> Option<Civil> {
    use windows_sys::Win32::Foundation::{FILETIME, SYSTEMTIME};
    use windows_sys::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};
    // 100 ns ticks from 1601.
    let ticks = secs.checked_add(11_644_473_600)?.checked_mul(10_000_000)?;
    let ticks = u64::try_from(ticks).ok()?;
    let file = FILETIME { dwLowDateTime: ticks as u32, dwHighDateTime: (ticks >> 32) as u32 };
    let mut utc: SYSTEMTIME = unsafe { std::mem::zeroed() };
    let mut local: SYSTEMTIME = unsafe { std::mem::zeroed() };
    unsafe {
        if FileTimeToSystemTime(&file, &mut utc) == 0 || SystemTimeToTzSpecificLocalTime(std::ptr::null(), &utc, &mut local) == 0 {
            return None;
        }
    }
    Some(Civil {
        year: i32::from(local.wYear),
        month: u32::from(local.wMonth),
        day: u32::from(local.wDay),
        weekday: u32::from(local.wDayOfWeek),
        hour: u32::from(local.wHour),
        minute: u32::from(local.wMinute),
        second: u32::from(local.wSecond),
        millis,
    })
}

/// This computer's zone name, as `GetTimeZoneInformation` gives it (standard or daylight):
/// what `get_time_offset` answers with no conductor (Decision).
#[cfg(windows)]
pub fn local_zone_name() -> String {
    use windows_sys::Win32::System::Time::{GetTimeZoneInformation, TIME_ZONE_INFORMATION};
    /// `TIME_ZONE_ID_DAYLIGHT`.
    const DAYLIGHT: u32 = 2;
    let mut info: TIME_ZONE_INFORMATION = unsafe { std::mem::zeroed() };
    let id = unsafe { GetTimeZoneInformation(&mut info) };
    let name = if id == DAYLIGHT { &info.DaylightName } else { &info.StandardName };
    let end = name.iter().position(|c| *c == 0).unwrap_or(name.len());
    tame(&String::from_utf16_lossy(&name[..end]), TZ_MAX)
}

/// The name of this computer's time zone right now (`tm_zone`, as "CST").
#[cfg(unix)]
pub fn local_zone_name() -> String {
    let now = unsafe { libc::time(std::ptr::null_mut()) };
    let mut t: libc::tm = unsafe { std::mem::zeroed() };
    if unsafe { libc::localtime_r(&now, &mut t) }.is_null() || t.tm_zone.is_null() {
        return String::new();
    }
    let name = unsafe { std::ffi::CStr::from_ptr(t.tm_zone) }.to_string_lossy().into_owned();
    tame(&name, TZ_MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn time_offset_script_follows_iterm2() {
        assert_eq!(
            script(1790000000.5),
            "import time\nfrom datetime import datetime, timezone\n\ntz = time.tzname[time.daylight and time.localtime().tm_isdst]\ndelta = time.time() - 1790000000.5\nreturn f\"{delta}\\n{tz}\\n{time.localtime().tm_gmtoff}\""
        );
    }

    #[test]
    fn parse_reply_follows_iterm2() {
        assert_eq!(parse_reply(true, "12.5\nCDT\n-18000", 2.0), Ok(Offset { offset: 11.5, tz: "CDT".into(), zone: Some(-18000) }));
        assert_eq!(parse_reply(true, "12.5\nUTC", 0.0).map(|o| o.zone), Ok(None));
        assert_eq!(parse_reply(true, "x", 0.0), Err("Invalid output: x".to_string()));
        assert_eq!(parse_reply(false, "Traceback", 0.0), Err("Failed to fetch time: Traceback".to_string()));
    }

    #[test]
    fn parse_reply_distrusts_the_remote() {
        // Not a finite number, or a clock a century off: an invalid reply.
        for bad in ["nan\nUTC\n0", "inf\nUTC\n0", "-inf\nUTC\n0", "1e300\nUTC\n0", "4e9\nUTC\n0"] {
            assert!(matches!(parse_reply(true, bad, 0.0), Err(e) if e.starts_with("Invalid output: ")), "{bad}");
        }
        // The zone must be a real offset (within a day); the name is short and plain.
        assert_eq!(parse_reply(true, "0\nCDT\n999999", 0.0).map(|o| o.zone), Ok(None));
        assert_eq!(parse_reply(true, "0\nCDT\nabc", 0.0).map(|o| o.zone), Ok(None));
        let hostile = format!("0\n\u{202e}C{}T\u{1b}[31m\n0", "D".repeat(500));
        let tz = parse_reply(true, &hostile, 0.0).unwrap().tz;
        assert!(tz.chars().count() <= TZ_MAX && !tz.chars().any(|c| c.is_control() || c == '\u{202e}'), "{tz:?}");
        // An error's text is capped and free of controls and line breaks.
        let long = format!("Traceback\n\u{1b}[2J{}", "x".repeat(10_000));
        let Err(text) = parse_reply(false, &long, 0.0) else { panic!() };
        assert!(text.chars().count() < 300 && !text.chars().any(char::is_control), "{}", text.len());
        let Err(text) = parse_reply(true, &long, 0.0) else { panic!() };
        assert!(text.chars().count() < 300 && !text.chars().any(char::is_control));
        // A negative latency (a clock step) never adds time.
        assert_eq!(parse_reply(true, "10\nUTC\n0", -4.0).unwrap().offset, 10.0);
    }

    #[test]
    fn clock_shift_follows_iterm2() {
        let s = shown(0.0, false, None, true, true);
        assert_eq!((s.shift, s.zone, s.globe), (0.0, None, false));
        let s = shown(11.5, false, None, true, true);
        assert_eq!((s.shift, s.zone, s.globe), (11.5, None, true));
        let s = shown(11.5, false, None, false, true);
        assert_eq!((s.shift, s.zone, s.globe), (0.0, None, true));
        let s = shown(0.0, true, Some(-18000), false, true);
        assert_eq!((s.shift, s.zone, s.globe), (0.0, Some(-18000), true));
        let s = shown(0.0, true, Some(-18000), false, false);
        assert_eq!((s.shift, s.zone, s.globe), (0.0, None, false));
        assert_eq!(GLOBE, '\u{E774}');
    }

    #[test]
    fn clock_refresh_follows_iterm2() {
        let t0 = Instant::now();
        let mut c = Clock::default();
        assert_eq!(c.due(2, Some("h"), t0), Refresh::Fetch);
        assert_eq!(c.due(2, Some("h"), t0), Refresh::Nothing);
        assert_eq!(c.due(2, Some("h"), t0 + REFRESH - Duration::from_secs(1)), Refresh::Nothing);
        assert_eq!(c.due(2, Some("h"), t0 + REFRESH), Refresh::Fetch);
        assert_eq!(c.due(2, Some("h"), t0 + REFRESH), Refresh::Nothing);
        // A level or hostname change is due at once.
        assert_eq!(c.due(2, Some("other"), t0 + REFRESH), Refresh::Fetch);
        assert_eq!(c.due(2, None, t0 + REFRESH), Refresh::Fetch);
        // A reply sets the shift; level 1 or 0 is local time and resets it.
        c.set(Ok(Offset { offset: 11.5, tz: "CDT".into(), zone: Some(-18000) }));
        assert_eq!((c.offset, c.tz.as_deref(), c.zone), (11.5, Some("CDT"), Some(-18000)));
        assert_eq!(c.due(1, None, t0 + REFRESH), Refresh::Local);
        assert_eq!((c.offset, c.tz.clone(), c.zone), (0.0, None, None));
        assert_eq!(c.due(1, None, t0 + REFRESH), Refresh::Nothing, "already local");
        assert_eq!(c.due(0, None, t0 + REFRESH), Refresh::Local);
        assert_eq!(c.due(2, None, t0 + REFRESH), Refresh::Fetch, "back to level 2");
        // An error resets.
        c.set(Ok(Offset { offset: 3.0, tz: "UTC".into(), zone: None }));
        c.set(Err("Failed to fetch time: x".into()));
        assert_eq!((c.offset, c.tz.clone(), c.zone), (0.0, None, None));
        // A fresh clock below level 2 is local at once.
        assert_eq!(Clock::default().due(0, None, t0), Refresh::Local);
    }

    #[test]
    fn a_late_reply_after_the_level_dropped_is_ignored() {
        let t0 = Instant::now();
        let mut c = Clock::default();
        assert_eq!(c.due(2, Some("h"), t0), Refresh::Fetch);
        assert_eq!(c.due(1, Some("h"), t0), Refresh::Local);
        c.set(Ok(Offset { offset: 99.0, tz: "UTC".into(), zone: None }));
        assert_eq!((c.offset, c.tz.clone()), (0.0, None), "the remote is gone: local time stays");
    }

    #[test]
    fn the_next_refresh_is_known_only_at_level_2() {
        let t0 = Instant::now();
        let mut c = Clock::default();
        assert_eq!(c.next_due(), None);
        c.due(2, None, t0);
        assert_eq!(c.next_due(), Some(t0 + REFRESH));
        c.due(1, None, t0);
        assert_eq!(c.next_due(), None);
    }
}
