//! iTerm2's stuck mouse reporting detection (pure): after Termurion writes a mouse report, a
//! shell that is no longer the program (it exited, or ssh died) echoes the report's printable
//! tail back as text. The tail of the reports written in the last 100 ms is remembered and
//! looked for in the output (`PTYSession.m:14033-14120`). The offer itself is 13b's host-change
//! offer (`host_offers_ui`, `Offer::Mouse`); this module only finds the echo and says what the
//! saved answer does. Kind-neutral: 13o's DEC 2048 resize reports arm the same detector.
//! Termurion adds a slow-link check beside iTerm2's (FIXES 18, `SLOW_DEADLINE`).

use std::collections::{HashSet, VecDeque};
use std::time::{Duration, Instant};

/// How long after a report its echo still counts (`PTYSession.m:14090`).
pub const DEADLINE: Duration = Duration::from_millis(100);
/// The most pattern bytes kept.
pub const MAX_LEN: usize = 32;
/// The pattern must be at least this long to arm (iTerm2: longer than 6).
pub const MIN_LEN: usize = 7;

/// Termurion's slow-link check (FIXES 18), beside iTerm2's: an echo of any reports written in
/// the last second counts, so a shell behind a slow ssh link is still caught while the mouse
/// keeps moving.
pub const SLOW_DEADLINE: Duration = Duration::from_secs(1);
/// The slow check matches this many bytes: at least two SGR reports' worth, never one alone.
pub const SLOW_LEN: usize = 16;
/// The most report bytes the slow check keeps.
pub const SLOW_MAX: usize = 2048;

/// The part of a report a terminal echoes: ESC and the two bytes after it are skipped and
/// control bytes dropped (`PTYSession.m:14060-14075`).
pub fn printable(report: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut skip = 0;
    for &b in report {
        if b == 0x1b {
            skip = 3;
        }
        if skip > 0 {
            skip -= 1;
            continue;
        }
        if b >= 32 {
            out.push(b);
        }
    }
    out
}

/// Which report a detector was armed by (iTerm2's `PTYSessionTurdType`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Turd {
    #[default]
    Mouse,
    Dec2048,
}

#[derive(Clone, Debug, Default)]
pub struct Detector {
    kind: Turd,
    pattern: Vec<u8>,
    deadline: Option<Instant>,
    /// The end of the output so far, for a match split across chunks.
    carry: Vec<u8>,
    /// The printable tail of each report written in the last `SLOW_DEADLINE`, oldest first.
    recent: VecDeque<(Instant, Vec<u8>)>,
    /// The end of the printable output so far, for the slow check.
    slow_carry: Vec<u8>,
}

impl Detector {
    /// A report was written: its printable tail joins the pattern while the last report's
    /// deadline holds, else starts it over.
    pub fn armed(&mut self, report: &[u8], now: Instant) {
        self.armed_as(report, now, Turd::Mouse);
    }

    /// `armed` for a report of this kind. One detector serves every kind, as iTerm2's one
    /// `_turdDetector`: the kind of the last report written is kept.
    pub fn armed_as(&mut self, report: &[u8], now: Instant, kind: Turd) {
        self.kind = kind;
        self.remember(printable(report), now);
        if !self.deadline.is_some_and(|d| now < d) {
            self.pattern.clear();
        }
        self.pattern.extend(printable(report));
        if self.pattern.len() > MAX_LEN {
            let cut = self.pattern.len() - MAX_LEN;
            self.pattern.drain(..cut);
        }
        self.carry.clear();
        // iTerm2 sets its expectation only for a pattern past 6 characters; a short one leaves
        // none, so the next report starts over (`PTYSession.m:14086-14099`).
        if self.pattern.len() < MIN_LEN {
            self.deadline = None;
            return;
        }
        self.deadline = Some(now + DEADLINE);
    }

    /// The kind of the last report that armed the detector.
    pub fn kind(&self) -> Turd {
        self.kind
    }

    /// Armed: the pattern is long enough and its deadline has not passed.
    pub fn is_armed(&self, now: Instant) -> bool {
        self.pattern.len() >= MIN_LEN && self.deadline.is_some_and(|d| now < d)
    }

    /// Output arrived. True when the pattern is in it (across chunks); the detector is then
    /// spent, so one occurrence is one match.
    pub fn output(&mut self, bytes: &[u8], now: Instant) -> bool {
        if self.iterm2_match(bytes, now) || self.slow_match(bytes, now) {
            self.pattern.clear();
            self.deadline = None;
            self.carry.clear();
            self.recent.clear();
            self.slow_carry.clear();
            return true;
        }
        false
    }

    /// iTerm2's check: the newest reports, echoed within `DEADLINE`.
    fn iterm2_match(&mut self, bytes: &[u8], now: Instant) -> bool {
        if !self.is_armed(now) {
            self.carry.clear();
            return false;
        }
        let mut seen = std::mem::take(&mut self.carry);
        seen.extend_from_slice(bytes);
        if seen.windows(self.pattern.len()).any(|w| w == self.pattern.as_slice()) {
            return true;
        }
        let keep = self.pattern.len() - 1;
        let from = seen.len().saturating_sub(keep);
        self.carry = seen[from..].to_vec();
        false
    }

    fn remember(&mut self, tail: Vec<u8>, now: Instant) {
        self.forget_old(now);
        self.recent.push_back((now, tail));
        let mut total: usize = self.recent.iter().map(|(_, t)| t.len()).sum();
        while total > SLOW_MAX
            && let Some((_, t)) = self.recent.pop_front()
        {
            total -= t.len();
        }
    }

    fn forget_old(&mut self, now: Instant) {
        while self.recent.front().is_some_and(|(t, _)| now.saturating_duration_since(*t) >= SLOW_DEADLINE) {
            self.recent.pop_front();
        }
    }

    /// The slow check: any `SLOW_LEN` bytes of the recent reports, in the printable output
    /// (a shell's own control bytes at a line wrap are skipped).
    fn slow_match(&mut self, bytes: &[u8], now: Instant) -> bool {
        self.forget_old(now);
        let joined: Vec<u8> = self.recent.iter().flat_map(|(_, t)| t.iter().copied()).collect();
        if joined.len() < SLOW_LEN {
            self.slow_carry.clear();
            return false;
        }
        let mut seen = std::mem::take(&mut self.slow_carry);
        seen.extend(bytes.iter().copied().filter(|b| *b >= 32 && *b != 0x7f));
        let windows: HashSet<&[u8]> = joined.windows(SLOW_LEN).collect();
        if seen.windows(SLOW_LEN).any(|w| windows.contains(w)) {
            return true;
        }
        let from = seen.len().saturating_sub(SLOW_LEN - 1);
        self.slow_carry = seen[from..].to_vec();
        false
    }

    #[cfg(test)]
    fn pattern(&self) -> &[u8] {
        &self.pattern
    }
}

/// What a match does (`PTYSession.m:17953-17962`): the saved answer true turns reporting off,
/// false does nothing, none offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Next {
    TurnOff,
    Offer,
    Nothing,
}

pub fn next(choice: Option<bool>) -> Next {
    match choice {
        Some(true) => Next::TurnOff,
        Some(false) => Next::Nothing,
        None => Next::Offer,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    /// `PTYSession.m:14044-14100`: ESC and the two bytes after it are skipped, bytes below 32
    /// dropped, the last 32 kept, the old pattern carried while its deadline holds, armed
    /// only past 6 bytes.
    #[test]
    fn pattern_follows_iterm2() {
        // iTerm2 skips ESC and the two bytes after it (`[` and `<`), so the button digit stays
        // (the brief's ";12;5M" drops it; the source wins).
        assert_eq!(printable(b"\x1b[<0;12;5M"), b"0;12;5M");
        assert_eq!(printable(b"\x1b[M #!"), b" #!");
        assert_eq!(printable(b"a\x01b"), b"ab");
        assert_eq!((DEADLINE, MAX_LEN, MIN_LEN), (ms(100), 32, 7));
        let t = Instant::now();
        let mut d = Detector::default();
        d.armed(b"\x1b[<0;1;1M", t);
        assert!(!d.is_armed(t), "six bytes do not arm");
        // iTerm2 leaves no detector after a short report (PS:14086-14099): the next report
        // starts fresh, so two one-digit SGR reports do not arm.
        d.armed(b"\x1b[<0;1;1m", t + ms(50));
        assert!(!d.is_armed(t + ms(50)));
        assert_eq!(d.pattern(), b"0;1;1m");
        // Three X10 reports (3 printable bytes each) do not arm either.
        let mut d = Detector::default();
        for i in 0..3 {
            d.armed(b"\x1b[M #!", t + ms(i));
        }
        assert!(!d.is_armed(t + ms(3)));
        assert_eq!(d.pattern(), b" #!");
        // 35 bytes of reports keep only the last 32.
        let mut d = Detector::default();
        for _ in 0..5 {
            d.armed(b"\x1b[<0;12;5M", t);
        }
        assert_eq!(d.pattern().len(), 32);
        assert!(d.pattern().ends_with(b";12;5M"));
        // A report after the deadline starts over.
        let mut d = Detector::default();
        d.armed(b"\x1b[<0;1;1M", t);
        d.armed(b"\x1b[<0;1;1m", t + ms(150));
        assert_eq!(d.pattern(), b"0;1;1m");
        assert!(!d.is_armed(t + ms(150)));
    }

    #[test]
    fn detector_matches_echoed_reports() {
        let t = Instant::now();
        let armed = || {
            let mut d = Detector::default();
            d.armed(b"\x1b[<0;12;5M", t);
            d.armed(b"\x1b[<0;12;5m", t);
            d
        };
        assert_eq!(armed().pattern(), b"0;12;5M0;12;5m");
        assert!(armed().output(b"$ 0;12;5M0;12;5m", t + ms(50)));
        assert!(!armed().output(b"$ 0;12;5M0;12;5m", t + ms(150)));
        let mut d = armed();
        assert!(!d.output(b"$ 0;12;5", t + ms(20)));
        assert!(d.output(b"M0;12;5m", t + ms(40)));
        // One occurrence is one match: the detector is spent, and the same echo again says nothing.
        assert!(!d.is_armed(t + ms(40)));
        assert!(!d.output(b"0;12;5M0;12;5m", t + ms(41)));
        // Unrelated output never matches and never arms.
        let mut d = Detector::default();
        assert!(!d.output(b"0;12;5M0;12;5m", t));
    }

    /// 13o Task 6 (R8): the resize report arms the same detector, and a match says which kind.
    #[test]
    fn dec2048_turds_follow_iterm2() {
        let t = Instant::now();
        let report = crate::emu::resize_report(24, 80, 400, 640);
        let mut d = Detector::default();
        assert_eq!(d.kind(), Turd::Mouse);
        d.armed_as(report.as_bytes(), t, Turd::Dec2048);
        assert!(d.is_armed(t), "one report is 16 printable bytes: armed");
        d.armed_as(report.as_bytes(), t + ms(50), Turd::Dec2048);
        assert!(d.is_armed(t + ms(50)));
        assert_eq!(d.kind(), Turd::Dec2048);
        assert!(d.output(b"$ 8;24;80;400;640t8;24;80;400;640t", t + ms(60)));
        assert_eq!(d.kind(), Turd::Dec2048, "the kind survives the match");
        d.armed(b"\x1b[<0;12;5M", t + ms(500));
        assert_eq!(d.kind(), Turd::Mouse, "the last armed kind wins");
        assert_eq!((next(None), next(Some(true)), next(Some(false))), (Next::Offer, Next::TurnOff, Next::Nothing));
    }

    /// FIXES 18: over a slow link (ssh through a tunnel) the echo comes back after iTerm2's
    /// 100 ms, and while the mouse keeps moving the newest 32 bytes are always ahead of it, so
    /// iTerm2's check alone missed a full screen of junk. Any 16 bytes of the reports written
    /// in the last second count.
    #[test]
    fn a_slow_echo_is_found_while_the_mouse_keeps_moving() {
        let t = Instant::now();
        let report = |i: u64| format!("\x1b[<35;{};57M", 100 + i);
        let mut d = Detector::default();
        // A report every 8 ms; the shell's echo trails 250 ms behind.
        let mut first = None;
        for i in 0..60u64 {
            d.armed(report(i).as_bytes(), t + ms(i * 8));
            if i >= 32 {
                let echoed = printable(report(i - 32).as_bytes());
                if d.output(&echoed, t + ms(i * 8 + 1)) && first.is_none() {
                    first = Some(i);
                }
            }
        }
        // The second echoed report completes 16 bytes: found at once, while reports keep coming.
        assert_eq!(first, Some(33));
    }

    #[test]
    fn a_slow_echo_needs_two_reports_and_a_recent_one() {
        let t = Instant::now();
        let mut d = Detector::default();
        d.armed(b"\x1b[<35;106;57M", t);
        d.armed(b"\x1b[<35;107;57M", t + ms(8));
        // One report's text alone (10 bytes) is too short to count.
        assert!(!d.output(b"35;107;57M", t + ms(300)));
        // The shell's own bytes at a line wrap do not hide the echo.
        assert!(d.output(b"35;106;5\r\n7M35;107;57M", t + ms(300)));
        let mut d = Detector::default();
        d.armed(b"\x1b[<35;106;57M", t);
        d.armed(b"\x1b[<35;107;57M", t + ms(8));
        assert!(!d.output(b"35;106;57M35;107;57M", t + ms(1500)), "an echo after a second does not count");
        // Split across two reads.
        let mut d = Detector::default();
        d.armed(b"\x1b[<35;106;57M", t);
        d.armed(b"\x1b[<35;107;57M", t + ms(8));
        assert!(!d.output(b"$ 35;106;57M35;", t + ms(200)));
        assert!(d.output(b"107;57M", t + ms(210)));
    }

    /// `PTYSession.m:17953-17962`; the bar itself is 13b's host offer (B2).
    #[test]
    fn stuck_offer_follows_iterm2() {
        assert_eq!(next(Some(true)), Next::TurnOff);
        assert_eq!(next(Some(false)), Next::Nothing);
        assert_eq!(next(None), Next::Offer);
    }
}
