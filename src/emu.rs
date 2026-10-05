//! Terminal emulation switches (milestone 13o): the Advanced knobs a program's sequences meet,
//! read once from the globals into one Copy value. Pure.

/// The Advanced emulation knobs, iTerm2's defaults in `Default`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rules {
    pub focus_reporting: bool,
    pub accept_osc7: bool,
    pub tab_width: usize,
    pub xterm_version: u32,
    pub color_16: bool,
    pub buffer_depth: i64,
    pub clear_history: Option<bool>,
    pub save_on_clear: bool,
    pub save_above_prompt: bool,
    pub max_url: usize,
    pub rep: bool,
    pub reset_sgr_on_prompt: bool,
    pub decrqcra_off: Option<bool>,
    pub tabs_wrap: bool,
    pub zwsp_advances: bool,
    pub vs16_all: bool,
    pub vs16_primary: bool,
    pub full_width_flags: bool,
}

impl Default for Rules {
    fn default() -> Rules {
        Rules {
            focus_reporting: true,
            accept_osc7: true,
            tab_width: 8,
            xterm_version: 2500,
            color_16: true,
            buffer_depth: 40,
            clear_history: None,
            save_on_clear: true,
            save_above_prompt: true,
            max_url: 2_097_152,
            rep: true,
            reset_sgr_on_prompt: true,
            decrqcra_off: None,
            tabs_wrap: false,
            zwsp_advances: true,
            vs16_all: false,
            vs16_primary: true,
            full_width_flags: true,
        }
    }
}

impl Rules {
    /// The rules as the globals stand. A key that has no row yet reads iTerm2's default, so a
    /// row added later changes nothing until it is edited.
    pub fn from_globals(g: &crate::globals::Globals) -> Rules {
        let d = Rules::default();
        let flag = |key: &str, default: bool| g.get(key).and_then(|v| v.as_bool()).unwrap_or(default);
        let int = |key: &str, default: i64| g.get(key).and_then(|v| v.as_integer()).unwrap_or(default);
        Rules {
            focus_reporting: flag("FocusReportingEnabled", d.focus_reporting),
            accept_osc7: flag("AcceptOSC7", d.accept_osc7),
            // iTerm2's `MAX(1, width)`.
            tab_width: int("DefaultTabStopWidth", d.tab_width as i64).clamp(1, 4096) as usize,
            xterm_version: int("XtermVersion", d.xterm_version as i64).clamp(0, u32::MAX as i64) as u32,
            color_16: flag("OscColorReport16Bits", d.color_16),
            buffer_depth: int("BufferDepth", d.buffer_depth),
            clear_history: g.optional_bool("PreventEscapeSequenceFromClearingHistory"),
            save_on_clear: flag("SaveScrollBufferWhenClearing", d.save_on_clear),
            save_above_prompt: flag("SaveScrollbackWhenCursorMovesAbovePrompt", d.save_above_prompt),
            max_url: int("MaxURLLength", d.max_url as i64).max(0) as usize,
            rep: flag("SupportREPCode", d.rep),
            reset_sgr_on_prompt: flag("ResetSGROnPrompt", d.reset_sgr_on_prompt),
            decrqcra_off: g.optional_bool("NoSyncDisableDECRQCRA"),
            tabs_wrap: flag("TabsWrapAround", d.tabs_wrap),
            zwsp_advances: flag("ZeroWidthSpaceAdvancesCursor", d.zwsp_advances),
            vs16_all: flag("Vs16Supported", d.vs16_all),
            vs16_primary: flag("Vs16SupportedInPrimaryScreen", d.vs16_primary),
            full_width_flags: flag("FullWidthFlags", d.full_width_flags),
        }
    }
}

/// Identifier of the CSI 3 J question (iTerm2's `ClearScrollbackHistory`).
pub const CLEAR_HISTORY_ID: &str = "ClearScrollbackHistory";
pub const CLEAR_HISTORY_TEXT: &str = "A control sequence attempted to clear scrollback history. Allow this in the future?";
pub const ALWAYS_ALLOW: &str = "Always _Allow";
pub const ALWAYS_DENY: &str = "Always _Deny";

/// CSI 3 J's permission from `PreventEscapeSequenceFromClearingHistory`: unset asks (and the
/// sequence that asked is dropped), true denies, false allows (`PTYSession.m:19822-19836`).
pub fn clear_history_decision(prevent: Option<bool>) -> crate::prompted::Decision {
    use crate::prompted::Decision;
    match prevent {
        None => Decision::Ask,
        Some(true) => Decision::Deny,
        Some(false) => Decision::Apply,
    }
}

/// Identifier of the DECRQCRA question (iTerm2's `NoSyncDisableDECRQCRA` announcement).
pub const DECRQCRA_ID: &str = "NoSyncDisableDECRQCRA";
pub const DECRQCRA_TEXT: &str = "An app tried to read screen contents with DECRQCRA. Enable this feature?";

/// The rectangle a DECRQCRA names, `params` being `Pid;Pp;Pt;Pl;Pb;Pr` (a missing or 0 value is
/// the default): (top, left, bottom, right), 0-based and inclusive, clipped to the screen, as
/// iTerm2's `rectangleInToken` without origin mode. None for an empty or negative rectangle.
pub fn decrqcra_rect(params: &[i64], cols: usize, lines: usize) -> Option<(usize, usize, usize, usize)> {
    if cols == 0 || lines == 0 {
        return None;
    }
    // 1-based, with -1 for a default, as iTerm2's parser hands them over.
    let at = |i: usize, default: i64| match params.get(i).copied() {
        Some(v) if v > 0 => v,
        _ => default,
    };
    let (top, left) = (at(2, 1), at(3, 1));
    let (bottom, right) = (at(4, lines as i64), at(5, cols as i64));
    if bottom < top || right < left || top > lines as i64 || left > cols as i64 {
        return None;
    }
    Some((top as usize - 1, left as usize - 1, bottom.min(lines as i64) as usize - 1, right.min(cols as i64) as usize - 1))
}

/// The sum of the UTF-16 code units of the characters given (one cell's text after another).
pub fn checksum(chars: impl Iterator<Item = char>) -> i32 {
    let mut units = [0u16; 2];
    chars.fold(0i32, |sum, c| c.encode_utf16(&mut units).iter().fold(sum, |sum, u| sum.wrapping_add(i32::from(*u))))
}

/// The DECRQCRA answer, `DCS Pid ! ~ xxxx ST`: iTerm2 prints `(short)checksum` with `%04x`, so a
/// sum past 32767 sign-extends.
pub fn checksum_reply(id: i64, sum: i32) -> String {
    format!("\x1bP{id}!~{:04x}\x1b\\", sum as i16 as i32 as u32)
}

/// A title as it may go into a report: iTerm2 replaces `|`, `;`, CR, LF and ESC with spaces.
/// Tandem also replaces every other control (C0, DEL, C1, U+2028, U+2029) and cuts at
/// `host_state::MAX_TITLE`, since the report is typed into the shell's input.
pub fn safe_title(t: &str) -> String {
    t.chars()
        .take(crate::host_state::MAX_TITLE)
        .map(|c| if matches!(c, '|' | ';' | '\u{2028}' | '\u{2029}') || c.is_control() { ' ' } else { c })
        .collect()
}

/// `CSI 20 t` (icon title, `ESC ] L`) or `CSI 21 t` (window title, `ESC ] l`) answered with
/// `title`, made safe. Empty for any other kind.
pub fn title_report(kind: u8, title: &str) -> String {
    let letter = match kind {
        20 => 'L',
        21 => 'l',
        _ => return String::new(),
    };
    format!("\x1b]{letter}{}\x1b\\", safe_title(title))
}

/// DEC mode 2048's report after a resize (`VT100Output.m:1231-1238`).
pub fn resize_report(rows: u16, cols: u16, height_px: u32, width_px: u32) -> String {
    format!("\x1b[48;{rows};{cols};{height_px};{width_px}t")
}

/// A control sequence a setting may drop or rewrite before it reaches the parser. The scanner
/// reads parameters as vte does (numbers, never raw text), so padding, leading zeros and the
/// colon form cannot slip past a gate; `rebuild` writes the canonical sequence back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Held {
    /// `CSI Pn b`: repeat the previous character.
    Rep(u16),
    /// `CSI 3 J` (with any further parameters): erase the history.
    ClearHistory,
    /// `CSI ? Pm h` or `l` whose list holds 47, 1047, 1048 or 1049; the whole list, in order.
    AltScreen { params: Vec<u16>, set: bool },
    /// `CSI 20 t` or `CSI 21 t`.
    TitleReport(u8),
    /// `CSI Pi;Pg;Pt;Pl;Pb;Pr * y`.
    Decrqcra(Vec<u16>),
    /// `CSI Pn i` (`CSI i` is 0): media copy.
    Print(u16),
    /// ENQ (0x05) in the ground state.
    Enq,
    /// `CSI Pl;Pc H` or `f`, found only while the scanner is armed for cursor moves.
    Cup { params: Vec<u16>, fin: u8 },
    /// An OSC that passed the length cap: the parser is sent CAN (vte ends the string there, as
    /// iTerm2 cuts it), and the rest of the string is dropped as `Drop` runs.
    CancelString,
    /// Bytes of a cancelled string: fed to nothing.
    Drop,
}

impl Held {
    /// Whether answering or acting on this reads what the grid and title are now, so a
    /// synchronized update in progress must be drawn first.
    pub fn reads_state(&self) -> bool {
        matches!(self, Held::ClearHistory | Held::TitleReport(_) | Held::Decrqcra(_) | Held::Print(_) | Held::Cup { .. })
    }
}

fn join(params: &[u16]) -> String {
    params.iter().map(u16::to_string).collect::<Vec<_>>().join(";")
}

/// The canonical bytes of a held sequence.
pub fn rebuild(h: &Held) -> Vec<u8> {
    match h {
        Held::Rep(n) => format!("\x1b[{n}b").into_bytes(),
        Held::ClearHistory => b"\x1b[3J".to_vec(),
        Held::AltScreen { params, set } => format!("\x1b[?{}{}", join(params), if *set { 'h' } else { 'l' }).into_bytes(),
        Held::TitleReport(n) => format!("\x1b[{n}t").into_bytes(),
        Held::Decrqcra(p) => format!("\x1b[{}*y", join(p)).into_bytes(),
        Held::Print(n) => format!("\x1b[{n}i").into_bytes(),
        Held::Enq => vec![0x05],
        Held::Cup { params, fin } => format!("\x1b[{}{}", join(params), *fin as char).into_bytes(),
        Held::CancelString => vec![0x18],
        Held::Drop => Vec::new(),
    }
}

/// The bytes to feed the parser for a held event. `keep` is what survives (None: nothing). A CAN
/// always goes first: the parser may hold the start of a sequence fed in an earlier read, or the
/// partial start of a chain of escapes (`ESC ESC [ 3 J`, `ESC [ 1 ESC [ 3 J`), and CAN returns it
/// to the ground state from any state, where it is invisible (alacritty ignores it there).
pub fn held_bytes(keep: Option<Vec<u8>>) -> Vec<u8> {
    let mut out = vec![0x18];
    out.extend(keep.unwrap_or_default());
    out
}

/// Disable Smcup Rmcup (`T:913-980`): the DECSET list without 47, 1047, 1048 and 1049 (1048 "by
/// analogy to xterm's titeInhibit"); None when nothing is left.
pub fn strip_alt_screen(params: &[u16]) -> Option<Vec<u16>> {
    let kept: Vec<u16> = params.iter().copied().filter(|p| !matches!(p, 47 | 1047 | 1048 | 1049)).collect();
    (!kept.is_empty()).then_some(kept)
}

pub const PRINT_ID: &str = "NoSyncAllowPrinting";
pub const PRINT_HEADING: &str = "Allow Printing?";
pub const PRINT_TEXT: &str = "There's a lot of printing going on. Want to keep allowing it?";
pub const PRINT_BUTTONS: [&str; 3] = ["Allow", "Disable Temporarily", "Disable Permanently"];

/// What the guard says about one print.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Guard {
    Print,
    Refuse,
    Ask,
}

/// iTerm2's `iTermPrintGuard` (`PG:20-75`), one per pane.
#[derive(Clone, Debug, Default)]
pub struct PrintGuard {
    last: Option<std::time::Instant>,
    disabled_until: Option<std::time::Instant>,
    off: bool,
}

/// A second attempt within this long asks (`PG:23`).
const RECENT: std::time::Duration = std::time::Duration::from_secs(30);
/// Disable Temporarily lasts this long (`PG:55`).
const TEMPORARY: std::time::Duration = std::time::Duration::from_secs(5 * 60);

impl PrintGuard {
    /// Whether a print goes ahead. An attempt that will print counts, asked or not (`PG:21-25`).
    pub fn check(&mut self, now: std::time::Instant, profile_disabled: bool, will_print: bool) -> Guard {
        if self.off || self.disabled_until.is_some_and(|t| now < t) {
            return Guard::Refuse;
        }
        self.disabled_until = None;
        if profile_disabled {
            return Guard::Refuse;
        }
        if will_print {
            let recent = self.last.is_some_and(|last| now.saturating_duration_since(last) < RECENT);
            self.last = Some(now);
            if recent {
                return Guard::Ask;
            }
        }
        Guard::Print
    }

    /// The person's answer to the question: 0 Allow, 1 Disable Temporarily, 2 Disable
    /// Permanently. True when the profile's Disable Printing must be set.
    pub fn answer(&mut self, now: std::time::Instant, choice: usize) -> bool {
        match choice {
            1 => self.disabled_until = Some(now + TEMPORARY),
            2 => {
                self.off = true;
                return true;
            }
            _ => {}
        }
        false
    }
}

/// A capture older than this ends at the next output (B8).
pub const PRINT_CAPTURE_SECS: u64 = 30;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Cap {
    Ground,
    Esc,
    /// After ESC and an intermediate (`ESC ( B`): one final byte ends it.
    Inter,
    Csi,
    /// An OSC, DCS, SOS, PM or APC string: ends at BEL or ST.
    Str,
    StrEsc,
}

/// The text of an escape-sequence print job (`CSI 5 i` to `CSI 4 i`): printable text, CR and LF.
/// Escape sequences are dropped, never executed (Decision, P8).
#[derive(Clone, Debug)]
pub struct PrintCapture {
    buf: Vec<u8>,
    state: Cap,
    pub started: std::time::Instant,
}

impl Default for PrintCapture {
    fn default() -> Self {
        PrintCapture { buf: Vec::new(), state: Cap::Ground, started: std::time::Instant::now() }
    }
}

impl PrintCapture {
    /// Takes the bytes the program wrote. None: all taken. Some((at, esc)): the capture ends and
    /// the parser resumes from `bytes[at..]`, after an ESC first when `esc` (a RIS whose ESC came
    /// in an earlier read).
    pub fn push(&mut self, bytes: &[u8], cap: usize) -> Option<(usize, bool)> {
        for (i, &b) in bytes.iter().enumerate() {
            // CAN and SUB end any sequence; the byte itself is dropped.
            if b == 0x18 || b == 0x1a {
                self.state = Cap::Ground;
                continue;
            }
            loop {
                match self.state {
                    Cap::Ground => {
                        match b {
                            0x1b => self.state = Cap::Esc,
                            b'\r' | b'\n' | 0x20..=0x7e | 0x80..=0xff => {
                                if self.buf.len() >= cap {
                                    return Some((i, false));
                                }
                                self.buf.push(b);
                            }
                            _ => {}
                        }
                    }
                    Cap::Esc => match b {
                        b'c' => return Some(if i == 0 { (0, true) } else { (i - 1, false) }),
                        b'[' => self.state = Cap::Csi,
                        b']' | b'P' | b'X' | b'^' | b'_' => self.state = Cap::Str,
                        0x20..=0x2f => self.state = Cap::Inter,
                        0x1b => {}
                        _ => self.state = Cap::Ground,
                    },
                    Cap::Inter => match b {
                        0x20..=0x2f => {}
                        0x1b => self.state = Cap::Esc,
                        _ => self.state = Cap::Ground,
                    },
                    Cap::Csi => match b {
                        0x40..=0x7e => self.state = Cap::Ground,
                        0x1b => self.state = Cap::Esc,
                        _ => {}
                    },
                    Cap::Str => match b {
                        0x07 => self.state = Cap::Ground,
                        0x1b => self.state = Cap::StrEsc,
                        _ => {}
                    },
                    Cap::StrEsc => {
                        self.state = if b == b'\\' { Cap::Ground } else { Cap::Esc };
                        if self.state == Cap::Esc {
                            continue;
                        }
                    }
                }
                break;
            }
        }
        None
    }

    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.buf).into_owned()
    }
}

pub const OSC7_WARNING_ID: &str = "NoSyncOSC7DisabledBreaksShellIntegration";
pub const OSC7_HEADING: &str = "Shell Integration Setting Conflict";
pub const OSC7_TEXT: &str = "Tandem\u{2019}s shell integration now reports your username, hostname, and current directory using OSC 7, but the \u{201c}Accept OSC 7\u{201d} advanced setting is turned off. Shell integration will not work correctly until it is turned back on.";
pub const OSC7_TURN_ON: &str = "Turn On OSC 7";

/// iTerm2's once-per-process OSC 7 warning flag, held on the App.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Osc7Latch {
    warned: bool,
}

impl Osc7Latch {
    /// True the first time, false after.
    pub fn first(&mut self) -> bool {
        !std::mem::replace(&mut self.warned, true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::globals::Globals;
    use toml::Value;

    fn empty() -> Globals {
        Globals::empty(std::path::Path::new("emu-test.toml"))
    }

    /// 13o Task 3 (Review Focus 1): CSI 3 J's decision and its texts.
    #[test]
    fn clear_history_follows_iterm2() {
        use crate::prompted::Decision;
        assert_eq!(clear_history_decision(None), Decision::Ask);
        assert_eq!(clear_history_decision(Some(true)), Decision::Deny);
        assert_eq!(clear_history_decision(Some(false)), Decision::Apply);
        assert_eq!(CLEAR_HISTORY_ID, "ClearScrollbackHistory");
        assert_eq!(CLEAR_HISTORY_TEXT, "A control sequence attempted to clear scrollback history. Allow this in the future?");
        assert_eq!((ALWAYS_ALLOW, ALWAYS_DENY), ("Always _Allow", "Always _Deny"));
    }

    #[test]
    fn plumb_rules_follow_iterm2() {
        let d = Rules::default();
        assert!(d.focus_reporting && d.accept_osc7 && d.color_16);
        assert_eq!((d.tab_width, d.xterm_version, d.buffer_depth), (8, 2500, 40));
        assert_eq!((d.clear_history, d.decrqcra_off), (None, None));
        assert!(d.save_on_clear && d.save_above_prompt && d.rep && d.reset_sgr_on_prompt);
        assert_eq!(d.max_url, 2_097_152);
        assert!(!d.tabs_wrap && !d.vs16_all);
        assert!(d.zwsp_advances && d.vs16_primary && d.full_width_flags);
        assert_eq!(Rules::from_globals(&empty()), d);
        let mut g = empty();
        g.set("DefaultTabStopWidth", Value::Integer(0));
        assert_eq!(Rules::from_globals(&g).tab_width, 1, "MAX(1, width)");
        g.set("DefaultTabStopWidth", Value::Integer(-3));
        assert_eq!(Rules::from_globals(&g).tab_width, 1);
        g.set("DefaultTabStopWidth", Value::Integer(4));
        g.set("XtermVersion", Value::Integer(95));
        g.set("OscColorReport16Bits", Value::Boolean(false));
        g.set("FocusReportingEnabled", Value::Boolean(false));
        g.set("AcceptOSC7", Value::Boolean(false));
        g.set("BufferDepth", Value::Integer(7));
        let r = Rules::from_globals(&g);
        assert_eq!((r.tab_width, r.xterm_version, r.color_16, r.focus_reporting, r.accept_osc7, r.buffer_depth), (4, 95, false, false, false, 7));
        let mut g = empty();
        g.set("PreventEscapeSequenceFromClearingHistory", Value::Boolean(false));
        g.set("NoSyncDisableDECRQCRA", Value::Boolean(true));
        let r = Rules::from_globals(&g);
        assert_eq!((r.clear_history, r.decrqcra_off), (Some(false), Some(true)));
    }

    #[test]
    fn osc7_latch_is_once() {
        let mut a = Osc7Latch::default();
        assert!(a.first());
        assert!(!a.first());
        assert!(Osc7Latch::default().first());
    }

    /// 13o Task 6 (Review Focus 5): DECRQCRA's rectangle, checksum and reply.
    #[test]
    fn decrqcra_follows_iterm2() {
        assert_eq!(decrqcra_rect(&[1, 1, 1, 1, 2, 2], 80, 24), Some((0, 0, 1, 1)));
        assert_eq!(decrqcra_rect(&[1, 1], 80, 24), Some((0, 0, 23, 79)), "defaults are the whole screen");
        assert_eq!(decrqcra_rect(&[1, 1, 0, 0, 0, 0], 80, 24), Some((0, 0, 23, 79)), "0 is a default");
        assert_eq!(decrqcra_rect(&[1, 1, 3, 3, 2, 2], 80, 24), None);
        assert_eq!(decrqcra_rect(&[1, 1, 1, 1, 99, 200], 80, 24), Some((0, 0, 23, 79)), "clipped to the screen");
        assert_eq!(decrqcra_rect(&[1, 1, 30, 1, 40, 2], 80, 24), None, "below the screen");
        assert_eq!(checksum("ab".chars()), 195);
        assert_eq!(checksum("\u{1F600}".chars()), 0xD83D + 0xDE00, "UTF-16 units");
        assert_eq!(checksum_reply(1, 195), "\x1bP1!~00c3\x1b\\");
        assert_eq!(checksum_reply(1, 40000), "\x1bP1!~ffff9c40\x1b\\");
    }

    /// 13o Task 6 (B6): a title in a report is never able to type into the shell.
    #[test]
    fn title_reports_follow_iterm2() {
        assert_eq!(safe_title("a|b;c\rd\ne\x1bf"), "a b c d e f");
        assert_eq!(title_report(21, "t"), "\x1b]lt\x1b\\");
        assert_eq!(title_report(20, "i"), "\x1b]Li\x1b\\");
        assert_eq!(title_report(22, "t"), "");
        for c in ['\u{3}', '\u{f}', '\t', '\u{7f}', '\u{80}', '\u{9b}', '\u{9f}', '\u{2028}', '\u{2029}', '\0', '\u{18}'] {
            assert_eq!(safe_title(&format!("a{c}b")), "a b", "{c:?}");
        }
        assert_eq!(safe_title("caf\u{e9} \u{1F600}"), "caf\u{e9} \u{1F600}", "text stays");
        let long = "x".repeat(10_000);
        assert_eq!(safe_title(&long).chars().count(), crate::host_state::MAX_TITLE);
        assert_eq!(title_report(21, &long).chars().count(), crate::host_state::MAX_TITLE + 5);
    }

    /// 13o Task 6 (R8): the resize report's bytes.
    #[test]
    fn resize_report_follows_iterm2() {
        assert_eq!(resize_report(24, 80, 400, 640), "\x1b[48;24;80;400;640t");
    }

    /// 13o Task 7 (Review Focus 6): Disable Smcup Rmcup removes only 47, 1047, 1048, 1049.
    #[test]
    fn smcup_switch_follows_iterm2() {
        assert_eq!(strip_alt_screen(&[1049]), None);
        assert_eq!(strip_alt_screen(&[1, 1049, 25]), Some(vec![1, 25]));
        assert_eq!(strip_alt_screen(&[47, 1047, 1048]), None);
        assert_eq!(strip_alt_screen(&[2004]), Some(vec![2004]));
    }

    /// 13o Task 7: the print guard (`PG:20-75`).
    #[test]
    fn print_guard_follows_iterm2() {
        use std::time::{Duration, Instant};
        let t0 = Instant::now();
        let s = Duration::from_secs;
        let mut g = PrintGuard::default();
        assert_eq!(g.check(t0, false, true), Guard::Print);
        assert_eq!(g.check(t0 + s(10), false, true), Guard::Ask);
        assert!(!g.answer(t0 + s(10), 1));
        assert_eq!(g.check(t0 + s(100), false, true), Guard::Refuse);
        assert_eq!(g.check(t0 + s(10 + 299), false, true), Guard::Refuse);
        assert_eq!(g.check(t0 + s(10 + 301), false, true), Guard::Print);
        let mut g = PrintGuard::default();
        g.check(t0, false, true);
        assert_eq!(g.check(t0 + s(5), false, true), Guard::Ask);
        assert!(g.answer(t0 + s(5), 2));
        assert_eq!(g.check(t0 + s(10_000), false, true), Guard::Refuse);
        let mut g = PrintGuard::default();
        assert_eq!(g.check(t0, true, true), Guard::Refuse);
        assert_eq!(g.check(t0, false, false), Guard::Print);
        assert_eq!(g.check(t0, false, false), Guard::Print, "no attempt is counted");
        assert_eq!(g.check(t0 + s(1), false, true), Guard::Print, "first counted attempt");
        let mut g = PrintGuard::default();
        g.check(t0, false, true);
        assert!(!g.answer(t0, 0), "Allow changes nothing");
        assert_eq!(g.check(t0 + s(31), false, true), Guard::Print, "30 s apart is not recent");
    }

    /// 13o Task 7 (B8): the capture keeps text, CR and LF, drops escape sequences, is bounded and
    /// stops at RIS.
    #[test]
    fn print_capture_text_follows_iterm2() {
        let mut c = PrintCapture::default();
        assert_eq!(c.push(b"b\x1b[31mc\r\nd\x1b]0;t\x07e\x1bP1$qm\x1b\\f\x1b(Bg\x07\x00h", 100), None);
        assert_eq!(c.text(), "bc\r\ndefgh");
        // A sequence split across reads.
        let mut c = PrintCapture::default();
        assert_eq!(c.push(b"a\x1b[3", 100), None);
        assert_eq!(c.push(b"1mb\x1b", 100), None);
        assert_eq!(c.push(b"]2;x", 100), None);
        assert_eq!(c.push(b"\x1b\\c", 100), None);
        assert_eq!(c.text(), "abc");
        // RIS ends it: the parser gets the ESC and what follows.
        let mut c = PrintCapture::default();
        assert_eq!(c.push(b"ab\x1bcxy", 100), Some((2, false)));
        let mut c = PrintCapture::default();
        assert_eq!(c.push(b"ab\x1b", 100), None);
        assert_eq!(c.push(b"cxy", 100), Some((0, true)));
        // The cap ends it too.
        let mut c = PrintCapture::default();
        assert_eq!(c.push(b"abcdefgh", 4), Some((4, false)));
    }
}
