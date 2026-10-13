//! iTerm2's stuck mouse reporting detection (pure): after Termurion writes a mouse report, a
//! shell that is no longer the program (it exited, or ssh died) echoes the report's printable
//! tail back as text. The tail of the reports written in the last 100 ms is remembered and
//! looked for in the output (`PTYSession.m:14033-14120`). The offer itself is 13b's host-change
//! offer (`host_offers_ui`, `Offer::Mouse`); this module only finds the echo and says what the
//! saved answer does. Kind-neutral: 13o's DEC 2048 resize reports arm the same detector.

use std::time::{Duration, Instant};

/// How long after a report its echo still counts (`PTYSession.m:14090`).
pub const DEADLINE: Duration = Duration::from_millis(100);
/// The most pattern bytes kept.
pub const MAX_LEN: usize = 32;
/// The pattern must be at least this long to arm (iTerm2: longer than 6).
pub const MIN_LEN: usize = 7;

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
        if !self.is_armed(now) {
            self.carry.clear();
            return false;
        }
        let mut seen = std::mem::take(&mut self.carry);
        seen.extend_from_slice(bytes);
        if seen.windows(self.pattern.len()).any(|w| w == self.pattern.as_slice()) {
            self.pattern.clear();
            self.deadline = None;
            return true;
        }
        let keep = self.pattern.len() - 1;
        let from = seen.len().saturating_sub(keep);
        self.carry = seen[from..].to_vec();
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

    /// `PTYSession.m:17953-17962`; the bar itself is 13b's host offer (B2).
    #[test]
    fn stuck_offer_follows_iterm2() {
        assert_eq!(next(Some(true)), Next::TurnOff);
        assert_eq!(next(Some(false)), Next::Nothing);
        assert_eq!(next(None), Next::Offer);
    }
}
