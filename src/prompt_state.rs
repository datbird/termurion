//! iTerm2's `PromptStateMachine` (milestone 9l): follows OSC 133 A, B, C and D, a send from
//! the composer, and what the shell echoes while a command is entered, and says when the Auto
//! Composer should appear, go away, or take text the shell already had. Also the Composer's
//! clear sequence (the bytes that empty the shell's command line before the composer takes
//! it over), the probe that erases the control characters a shell echoes back for it, and
//! the backspace bytes that probe writes.
//!
//! iTerm2 sources (`~/gitrepos/iterm2-ref/sources`): `PSM` `VT100/PromptStateMachine.swift`,
//! `MS` `VT100Screen/VT100ScreenMutableState.m` (the prompt capture and the delegate), `PS`
//! `PTYSession/PTYSession.m` (the clear sequence, its probe and `backspaceData`).

use std::time::{Duration, Instant};

use crate::keyaction::{self, Binding};
use crate::styled_copy;

/// The machine's states (`PSM:30-43`). A cursor is `(absolute line, column)`.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum State {
    Disabled,
    #[default]
    Ground,
    ReceivingPrompt,
    /// The composer is open.
    Entering { prompt: Vec<String> },
    /// The composer is open and has taken what the shell echoed so far.
    Accruing { so_far: String, prompt: Vec<String>, cursor: (i64, usize) },
    EchoingBack,
    Executing,
}

/// What the app must do for the Auto Composer (the delegate calls, `MS:8409-8445`).
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    Reveal { rows: Vec<styled_copy::Row>, text: Vec<String> },
    Dismiss,
    Append(String),
}

/// Accrued type-ahead stops growing past this many UTF-16 units (`PSM:410`).
pub const MAX_ACCRUED: usize = 4096;
/// A prompt taller than this is cut (`MS:4757`).
pub const MAX_PROMPT_LINES: i64 = 4;

/// The machine. `enabled` is the global Auto Composer setting (`autoComposerEnabled`): off,
/// a reveal only sets `eligible` and dismisses and appends are not delivered (`MS:8409-8445`),
/// and echoed text is not followed at all (`VT100ScreenMutableState+MRR.m:26`). `eligible`
/// is iTerm2's `_promptStateAllowsAutoComposer`, set by every reveal (`PS:15647-15652`).
#[derive(Clone, Debug, Default)]
pub struct PromptState {
    pub state: State,
    pub enabled: bool,
    pub eligible: bool,
    /// The captured prompt's look, for `again`.
    rows: Vec<styled_copy::Row>,
}

impl PromptState {
    /// `revealComposer` through the delegate (`MS:8409-8420`).
    fn reveal(&mut self, rows: Vec<styled_copy::Row>, text: Vec<String>) -> Vec<Event> {
        self.eligible = true;
        if self.enabled { vec![Event::Reveal { rows, text }] } else { Vec::new() }
    }

    fn dismiss(&self) -> Vec<Event> {
        if self.enabled { vec![Event::Dismiss] } else { Vec::new() }
    }

    /// `setAllowed:` (`PSM:189-204`): the soft alternate screen turns the machine off, and
    /// leaving it puts the machine back at ground whatever it was.
    pub fn set_allowed(&mut self, on: bool) -> Vec<Event> {
        if on {
            self.state = State::Ground;
            return Vec::new();
        }
        self.state = State::Disabled;
        self.dismiss()
    }

    /// OSC 133;A (`PSM:291-305`). Accruing goes on without a dismiss (kept).
    pub fn a(&mut self) -> Vec<Event> {
        match self.state {
            State::Disabled | State::ReceivingPrompt => Vec::new(),
            State::Entering { .. } => {
                self.state = State::ReceivingPrompt;
                self.dismiss()
            }
            State::Ground | State::EchoingBack | State::Executing | State::Accruing { .. } => {
                self.state = State::ReceivingPrompt;
                Vec::new()
            }
        }
    }

    /// OSC 133;B (`PSM:308-327`); `captured` follows with the prompt. Out of order, the
    /// machine goes to ground (dismissing only from Entering, as iTerm2 does).
    pub fn b(&mut self) -> Vec<Event> {
        match self.state {
            State::Disabled | State::ReceivingPrompt => Vec::new(),
            State::Entering { .. } => {
                self.state = State::Ground;
                self.dismiss()
            }
            State::Ground | State::EchoingBack | State::Executing | State::Accruing { .. } => {
                self.state = State::Ground;
                Vec::new()
            }
        }
    }

    /// `didCapturePrompt:` (`PSM:330-343`): reveals from any state but Disabled (outside
    /// ReceivingPrompt it is a trigger that found the prompt).
    pub fn captured(&mut self, rows: Vec<styled_copy::Row>, text: Vec<String>) -> Vec<Event> {
        if self.state == State::Disabled {
            return Vec::new();
        }
        self.rows = rows.clone();
        self.state = State::Entering { prompt: text.clone() };
        self.reveal(rows, text)
    }

    /// OSC 133;C (`PSM:355-370`).
    pub fn c(&mut self) -> Vec<Event> {
        match self.state {
            State::Disabled => Vec::new(),
            State::Ground | State::ReceivingPrompt | State::Executing => {
                self.state = State::Ground;
                Vec::new()
            }
            State::Entering { .. } => {
                self.state = State::Ground;
                self.dismiss()
            }
            State::EchoingBack | State::Accruing { .. } => {
                self.state = State::Executing;
                self.dismiss()
            }
        }
    }

    /// OSC 133;D (`PSM:373-383`).
    pub fn d(&mut self) -> Vec<Event> {
        match self.state {
            State::Disabled => Vec::new(),
            State::Entering { .. } => {
                self.state = State::Ground;
                self.dismiss()
            }
            _ => {
                self.state = State::Ground;
                Vec::new()
            }
        }
    }

    /// The composer is about to send its command (`PSM:229-244`).
    pub fn will_send(&mut self) {
        if matches!(self.state, State::Entering { .. }) {
            self.state = State::EchoingBack;
        }
    }

    /// Text the shell echoed while a command is entered, with the cursor after it
    /// (`PSM:386-433`). The first part is trimmed of leading spaces and tabs and an empty one
    /// is ignored; a later part on the cursor's last row at or left of its column is
    /// rejected; a part that would take the text past `MAX_ACCRUED` UTF-16 units is dropped.
    /// Not followed while the setting is off (`VT100ScreenMutableState+MRR.m:26`).
    pub fn output(&mut self, part: &str, cursor: (i64, usize)) -> Vec<Event> {
        if !self.enabled {
            return Vec::new();
        }
        let (so_far, prompt, prev, part) = match &self.state {
            State::Entering { prompt } => {
                if part.is_empty() {
                    return Vec::new();
                }
                (String::new(), prompt.clone(), (-1, 0), part.trim_start_matches(horizontal_space))
            }
            State::Accruing { so_far, prompt, cursor } => (so_far.clone(), prompt.clone(), *cursor, part),
            _ => return Vec::new(),
        };
        if cursor.0 == prev.0 && cursor.1 <= prev.1 {
            return Vec::new();
        }
        if so_far.encode_utf16().count() + part.encode_utf16().count() > MAX_ACCRUED {
            return Vec::new();
        }
        self.state = State::Accruing { so_far: so_far + part, prompt, cursor };
        if part.is_empty() { Vec::new() } else { vec![Event::Append(part.to_string())] }
    }

    /// `revealOrDismissComposerAgain` (`PSM:249-266`), for a change of the setting.
    pub fn again(&self) -> Vec<Event> {
        if !self.enabled {
            return Vec::new();
        }
        match &self.state {
            State::Entering { prompt } | State::Accruing { prompt, .. } => vec![Event::Reveal { rows: self.rows.clone(), text: prompt.clone() }],
            _ => vec![Event::Dismiss],
        }
    }

    /// The Auto Composer setting changed (`PS:18940-18943` then `MS:599-602`): the machine
    /// follows it and, turned on, reveals or dismisses again at once. A delivered dismiss
    /// clears the eligibility (`screenDismissComposer`, `PS:15663`).
    pub fn set_enabled(&mut self, on: bool) -> Vec<Event> {
        self.enabled = on;
        let events = self.again();
        if events.contains(&Event::Dismiss) {
            self.eligible = false;
        }
        events
    }

    /// `isEnteringCommand`.
    pub fn entering(&self) -> bool {
        matches!(self.state, State::Entering { .. } | State::Accruing { .. })
    }
}

/// `CharacterSet.whitespaces`: spaces and tabs, not line ends.
fn horizontal_space(c: char) -> bool {
    c == '\t' || (c.is_whitespace() && !matches!(c, '\n' | '\r' | '\u{0b}' | '\u{0c}' | '\u{85}' | '\u{2028}' | '\u{2029}'))
}

/// The prompt's range from its start to the command start, cut as iTerm2 cuts it
/// (`MS:4756-4760`): a prompt over `MAX_PROMPT_LINES` lines gets both lines set to absolute
/// line 4, not its last four lines (kept).
pub fn prompt_range(start: (i64, usize), end: (i64, usize)) -> ((i64, usize), (i64, usize)) {
    if end.0 - start.0 > MAX_PROMPT_LINES {
        return ((MAX_PROMPT_LINES, start.1), (MAX_PROMPT_LINES, end.1));
    }
    (start, end)
}

/// The key that holds the clear sequence (`ADV:675`), its default, and how long the probe
/// waits for the echo (`PS:8703`).
pub const CLEAR_KEY: &str = "ComposerClearSequence";
pub const DEFAULT_CLEAR: &str = "0x15 0x0b";
pub const PROBE: Duration = Duration::from_millis(100);

/// The control bytes the probe counts: 1 to 31 (`PS:8675-8677`).
fn control(b: u8) -> bool {
    (1..=31).contains(&b)
}

/// The expectation's pattern (`PS:8668-8688`): each control byte becomes an optional group
/// holding its caret form, escaped (`^U` for 0x15). The pattern matches the empty string, so
/// any output line in time fires it (kept).
pub fn caret_pattern(bytes: &[u8]) -> String {
    let mut out = String::new();
    for &b in bytes.iter().filter(|b| control(**b)) {
        out.push('(');
        for c in ['^', (b + 0x40) as char] {
            if "\\*?+[](){}^$|./".contains(c) {
                out.push('\\');
            }
            out.push(c);
        }
        out.push_str(")?");
    }
    out
}

/// Waits for the shell to echo the clear sequence's control characters (`PS:8690-8735`):
/// output before the deadline owes one backspace per group, matched or not
/// (`PTYTriggerEvaluator.m:173-177`); a user write first cancels the probe (`PS:4154-4158`,
/// the app drops it).
#[derive(Clone, Debug)]
pub struct ClearProbe {
    deadline: Instant,
    groups: usize,
}

impl ClearProbe {
    /// None when the sequence has no control bytes (nothing to erase).
    pub fn start(bytes: &[u8], now: Instant) -> Option<ClearProbe> {
        let groups = bytes.iter().filter(|b| control(**b)).count();
        (groups > 0).then(|| ClearProbe { deadline: now + PROBE, groups })
    }

    /// Output arrived: the backspaces owed, once, and only before the deadline.
    pub fn output(&mut self, now: Instant) -> Option<usize> {
        if self.expired(now) {
            return None;
        }
        Some(std::mem::take(&mut self.groups))
    }

    /// Past the deadline, or already fired: the app drops it.
    pub fn expired(&self, now: Instant) -> bool {
        self.groups == 0 || now > self.deadline
    }
}

/// The profile's Backspace mapping as bytes (`PS:12893-12922`): no mapping is DEL; Send Hex
/// Codes, Send Text (as written), Send Text with vim Special Chars (either kind), Send Escape
/// Sequence, Send ^H Backspace and Send ^? Backspace as mapped; any other action nothing.
pub fn backspace_bytes(b: Option<&Binding>) -> Vec<u8> {
    let Some(b) = b else { return vec![0x7f] };
    match b.action {
        11 => keyaction::hex_bytes(&b.text),
        12 => b.text.as_bytes().to_vec(),
        38 | 76 => keyaction::expand(&b.text, keyaction::Escaping::Vim).into_bytes(),
        10 => [b"\x1b".as_slice(), b.text.as_bytes()].concat(),
        16 => vec![0x08],
        17 => vec![0x7f],
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn on() -> PromptState {
        PromptState { enabled: true, ..Default::default() }
    }

    fn entering(p: &mut PromptState) {
        p.state = State::Ground;
        p.a();
        p.captured(Vec::new(), vec!["$ ".into()]);
    }

    #[test]
    fn prompt_states_follow_iterm2() {
        let prompt = vec!["$ ".to_string()];
        let mut p = on();
        assert_eq!(p.state, State::Ground);
        assert!(p.a().is_empty());
        assert_eq!(p.state, State::ReceivingPrompt);
        // B waits for the capture.
        assert!(p.b().is_empty());
        assert_eq!(p.state, State::ReceivingPrompt);
        assert_eq!(p.captured(Vec::new(), prompt.clone()), vec![Event::Reveal { rows: Vec::new(), text: prompt.clone() }]);
        assert_eq!(p.state, State::Entering { prompt: prompt.clone() });
        assert!(p.entering());
        // The first part is trimmed of leading whitespace.
        assert_eq!(p.output("  ls", (5, 2)), vec![Event::Append("ls".into())]);
        assert_eq!(p.state, State::Accruing { so_far: "ls".into(), prompt: prompt.clone(), cursor: (5, 2) });
        assert!(p.entering());
        // Same row, at or left of the last cursor: rejected.
        assert!(p.output("x", (5, 2)).is_empty());
        assert!(p.output("x", (5, 1)).is_empty());
        assert_eq!(p.state, State::Accruing { so_far: "ls".into(), prompt: prompt.clone(), cursor: (5, 2) });
        // To the right, or on another row: appended, not trimmed.
        assert_eq!(p.output(" -l", (5, 5)), vec![Event::Append(" -l".into())]);
        assert_eq!(p.output("a", (6, 0)), vec![Event::Append("a".into())]);
        assert_eq!(p.state, State::Accruing { so_far: "ls -la".into(), prompt: prompt.clone(), cursor: (6, 0) });
        // Past 4096 UTF-16 units: dropped, the state unchanged.
        let big = "\u{e9}".repeat(MAX_ACCRUED - 6);
        assert_eq!(p.output(&big, (7, 0)), vec![Event::Append(big.clone())]);
        assert!(p.output("z", (8, 0)).is_empty());
        let State::Accruing { so_far, cursor, .. } = &p.state else { panic!() };
        assert_eq!((so_far.encode_utf16().count(), *cursor), (MAX_ACCRUED, (7, 0)));
        // An emoji is two UTF-16 units.
        let mut q = on();
        entering(&mut q);
        q.output(&"a".repeat(MAX_ACCRUED - 1), (1, 1));
        assert!(q.output("\u{1f600}", (2, 0)).is_empty());
        assert_eq!(q.output("b", (2, 0)), vec![Event::Append("b".into())]);
        // An empty first part does nothing (focus reports pass through).
        let mut q = on();
        entering(&mut q);
        assert!(q.output("", (1, 1)).is_empty());
        assert_eq!(q.state, State::Entering { prompt: prompt.clone() });
        // A part of only spaces moves to accruing with nothing to append.
        assert!(q.output("   ", (1, 3)).is_empty());
        assert!(matches!(&q.state, State::Accruing { so_far, .. } if so_far.is_empty()));

        // C: from Entering, Dismiss and Ground; from EchoingBack and Accruing, Dismiss and
        // Executing; from Ground, ReceivingPrompt and Executing, Ground.
        let mut q = on();
        entering(&mut q);
        assert_eq!(q.c(), vec![Event::Dismiss]);
        assert_eq!(q.state, State::Ground);
        entering(&mut q);
        q.will_send();
        assert_eq!(q.state, State::EchoingBack);
        assert!(!q.entering());
        assert_eq!(q.c(), vec![Event::Dismiss]);
        assert_eq!(q.state, State::Executing);
        assert!(q.c().is_empty());
        assert_eq!(q.state, State::Ground);
        entering(&mut q);
        q.output("ls", (1, 4));
        assert_eq!(q.c(), vec![Event::Dismiss]);
        assert_eq!(q.state, State::Executing);
        // will_send only acts while Entering.
        q.will_send();
        assert_eq!(q.state, State::Executing);

        // D: Ground from everything, dismissing from Entering.
        assert!(q.d().is_empty());
        assert_eq!(q.state, State::Ground);
        entering(&mut q);
        assert_eq!(q.d(), vec![Event::Dismiss]);
        assert_eq!(q.state, State::Ground);
        entering(&mut q);
        q.output("ls", (1, 4));
        assert!(q.d().is_empty());
        assert_eq!(q.state, State::Ground);

        // B while Entering: Dismiss and Ground; while Accruing: Ground only.
        entering(&mut q);
        assert_eq!(q.b(), vec![Event::Dismiss]);
        assert_eq!(q.state, State::Ground);
        entering(&mut q);
        q.output("ls", (1, 4));
        assert!(q.b().is_empty());
        assert_eq!(q.state, State::Ground);
        // A while Entering dismisses; while Accruing it does not (kept).
        entering(&mut q);
        assert_eq!(q.a(), vec![Event::Dismiss]);
        assert_eq!(q.state, State::ReceivingPrompt);
        entering(&mut q);
        q.output("ls", (1, 4));
        assert!(q.a().is_empty());
        assert_eq!(q.state, State::ReceivingPrompt);
        // A capture outside ReceivingPrompt (a trigger) still reveals.
        q.state = State::Executing;
        assert_eq!(q.captured(Vec::new(), prompt.clone()).len(), 1);
        assert_eq!(q.state, State::Entering { prompt: prompt.clone() });

        // set_allowed(false): Disabled with Dismiss; nothing moves it but set_allowed(true).
        assert_eq!(q.set_allowed(false), vec![Event::Dismiss]);
        assert_eq!(q.state, State::Disabled);
        for ev in [q.a(), q.b(), q.captured(Vec::new(), prompt.clone()), q.c(), q.d(), q.output("x", (9, 9))] {
            assert!(ev.is_empty());
            assert_eq!(q.state, State::Disabled);
        }
        assert!(q.set_allowed(true).is_empty());
        assert_eq!(q.state, State::Ground);

        // With the setting off: a reveal is not delivered, only marks the session eligible;
        // Dismiss and Append are not delivered and echoed text is not followed.
        let mut off = PromptState::default();
        off.a();
        assert!(!off.eligible);
        assert!(off.captured(Vec::new(), prompt.clone()).is_empty());
        assert!(off.eligible);
        assert_eq!(off.state, State::Entering { prompt: prompt.clone() });
        assert!(off.output("ls", (1, 4)).is_empty());
        assert_eq!(off.state, State::Entering { prompt: prompt.clone() });
        assert!(off.again().is_empty());
        assert!(off.c().is_empty());
        assert_eq!(off.state, State::Ground);
        assert!(off.set_allowed(false).is_empty());
        assert_eq!(off.state, State::Disabled);

        // again(): reveals while Entering or Accruing (with the captured look), dismisses
        // otherwise.
        let mut q = on();
        let rows = vec![styled_copy::Row { runs: Vec::new(), hard_end: true }];
        q.a();
        q.captured(rows.clone(), prompt.clone());
        assert_eq!(q.again(), vec![Event::Reveal { rows: rows.clone(), text: prompt.clone() }]);
        q.output("ls", (1, 4));
        assert_eq!(q.again(), vec![Event::Reveal { rows: rows.clone(), text: prompt.clone() }]);
        q.c();
        assert_eq!(q.again(), vec![Event::Dismiss]);
        q.state = State::Ground;
        assert_eq!(q.again(), vec![Event::Dismiss]);

        // The four-line cap sets both lines to absolute line 4 (kept).
        assert_eq!(prompt_range((10, 0), (16, 3)), ((4, 0), (4, 3)));
        assert_eq!(prompt_range((10, 0), (14, 3)), ((10, 0), (14, 3)));
        assert_eq!(prompt_range((10, 2), (10, 5)), ((10, 2), (10, 5)));
    }

    #[test]
    fn clear_probe_follows_iterm2() {
        assert_eq!(keyaction::hex_bytes(DEFAULT_CLEAR), vec![0x15, 0x0b]);
        assert_eq!(CLEAR_KEY, "ComposerClearSequence");
        assert_eq!(PROBE, Duration::from_millis(100));
        assert_eq!(caret_pattern(&[0x15, 0x0b]), "(\\^U)?(\\^K)?");
        // 0 and bytes over 31 add no group; the letter is escaped like the caret.
        assert_eq!(caret_pattern(&[0, b'a', 0x7f, 0x03]), "(\\^C)?");
        assert_eq!(caret_pattern(&[0x1b, 0x1c]), "(\\^\\[)?(\\^\\\\)?");
        assert_eq!(caret_pattern(&[]), "");

        let t = Instant::now();
        let mut p = ClearProbe::start(&[0x15, 0x0b], t).unwrap();
        assert!(!p.expired(t + Duration::from_millis(50)));
        assert_eq!(p.output(t + Duration::from_millis(50)), Some(2));
        assert_eq!(p.output(t + Duration::from_millis(60)), None);
        let mut p = ClearProbe::start(&[0x15, 0x0b], t).unwrap();
        assert!(p.expired(t + Duration::from_millis(150)));
        assert_eq!(p.output(t + Duration::from_millis(150)), None);
        // One group per control byte, not per distinct byte.
        let mut p = ClearProbe::start(&[0x15, 0x15, b'x'], t).unwrap();
        assert_eq!(p.output(t), Some(2));
        assert!(ClearProbe::start(&[b'a', 0, 0x80], t).is_none());
        assert!(ClearProbe::start(&[], t).is_none());

        assert_eq!(backspace_bytes(None), vec![0x7f]);
        assert_eq!(backspace_bytes(Some(&Binding::new(11, "0x08 0x7f"))), vec![0x08, 0x7f]);
        // Send Text is sent as written (no escaping, `PS:12903-12904`).
        assert_eq!(backspace_bytes(Some(&Binding::new(12, "ab\\n"))), b"ab\\n".to_vec());
        assert_eq!(backspace_bytes(Some(&Binding::new(38, "x\\n"))), b"x\n".to_vec());
        assert_eq!(backspace_bytes(Some(&Binding::new(76, "\\<C-h>"))), vec![8]);
        assert_eq!(backspace_bytes(Some(&Binding::new(10, "[3~"))), b"\x1b[3~".to_vec());
        assert_eq!(backspace_bytes(Some(&Binding::new(16, ""))), vec![8]);
        assert_eq!(backspace_bytes(Some(&Binding::new(17, ""))), vec![0x7f]);
        for other in [0, 5, 13, 25, 63, 64] {
            assert!(backspace_bytes(Some(&Binding::new(other, "x"))).is_empty(), "action {other}");
        }
    }

    #[test]
    fn turning_auto_composer_on_follows_iterm2() {
        // Off, a prompt only makes the session eligible; the command then runs (C) without a
        // dismiss being delivered, so `eligible` is still set.
        let mut p = PromptState::default();
        entering(&mut p);
        assert!(p.eligible);
        p.will_send();
        assert!(p.c().is_empty());
        assert_eq!(p.state, State::Executing);
        assert!(p.eligible);
        // Turned on mid-command: `revealOrDismissComposerAgain` dismisses at once
        // (`MS:600-602`), which clears the eligibility (`PS:15663`), so Show Composer is no
        // longer refused.
        assert_eq!(p.set_enabled(true), vec![Event::Dismiss]);
        assert!(p.enabled);
        assert!(!p.eligible);
        // Turned on while entering: reveals with the captured prompt.
        let mut q = PromptState::default();
        entering(&mut q);
        assert_eq!(q.set_enabled(true), vec![Event::Reveal { rows: Vec::new(), text: vec!["$ ".into()] }]);
        assert!(q.eligible);
        // Turned off: nothing is delivered (a shown Auto Composer stays up, kept).
        assert!(q.set_enabled(false).is_empty());
        assert!(q.eligible);
    }
}
