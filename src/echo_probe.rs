//! iTerm2's echo probe (pure): whether a session is really at a password prompt, found by
//! writing a space then a backspace and watching what comes back, plus the broadcast round that
//! runs the probe over several sessions at once and decides who gets the password
//! (`iTermEchoProbe.m`, `iTermBroadcastPasswordHelper.m`, `PseudoTerminal.m` broadcastPassword:).
//!
//! Everything here is plain data and functions over raw output bytes, so it is unit tested on
//! Linux; the app side (feeding `Pane::pump`'s output, scheduling the timeout, driving the
//! window) comes in a later task.

use std::time::{Duration, Instant};

use crate::pwm::Secret;

/// The probe's state machine (iTerm2's `iTermEchoProbeState`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Off,
    Waiting,
    OneAsterisk,
    BackspaceOverAsterisk,
    SpaceOverAsterisk,
    BackspaceOverSpace,
    Failed,
}

/// A simplified token from raw output bytes: just enough of VT100 to drive the probe (a
/// printable run, a backspace, cursor-back N, erase-line N, or anything else).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Token {
    Text(String),
    Backspace,
    CursorBack(u16),
    EraseLine(u16),
    Other,
}

/// Turns raw output bytes into `Token`s. A run of printable bytes within one `feed` call is one
/// `Text` token; an escape sequence split across two `feed` calls is carried over and completed
/// on the next call.
pub struct Tokenizer {
    pending: Vec<u8>,
}

impl Tokenizer {
    pub fn new() -> Tokenizer {
        Tokenizer { pending: Vec::new() }
    }

    pub fn feed(&mut self, bytes: &[u8]) -> Vec<Token> {
        let mut buf = std::mem::take(&mut self.pending);
        buf.extend_from_slice(bytes);

        let mut out = Vec::new();
        let mut text = String::new();
        let n = buf.len();
        let mut i = 0usize;
        while i < n {
            let b = buf[i];
            if b == 0x1b {
                match parse_escape(&buf[i..]) {
                    Esc::Complete(tok, len) => {
                        flush_text(&mut out, &mut text);
                        out.push(tok);
                        i += len;
                    }
                    Esc::Incomplete => {
                        flush_text(&mut out, &mut text);
                        self.pending = buf[i..].to_vec();
                        return out;
                    }
                }
            } else if b == 0x08 {
                flush_text(&mut out, &mut text);
                out.push(Token::Backspace);
                i += 1;
            } else if (0x20..0x7f).contains(&b) {
                text.push(b as char);
                i += 1;
            } else {
                flush_text(&mut out, &mut text);
                out.push(Token::Other);
                i += 1;
            }
        }
        flush_text(&mut out, &mut text);
        out
    }
}

impl Default for Tokenizer {
    fn default() -> Tokenizer {
        Tokenizer::new()
    }
}

fn flush_text(out: &mut Vec<Token>, text: &mut String) {
    if !text.is_empty() {
        out.push(Token::Text(std::mem::take(text)));
    }
}

enum Esc {
    /// A complete escape sequence: the token it means, and how many bytes (from the leading
    /// ESC) it consumed.
    Complete(Token, usize),
    /// Not enough bytes yet to tell; carry the whole thing (from the leading ESC) to the next
    /// `feed` call.
    Incomplete,
}

/// Parses one escape sequence starting at `buf[0]` (which is ESC). Only CSI final bytes `D`
/// (cursor back) and `K` (erase line) are given a typed token; anything else recognized as a
/// complete sequence is `Other`.
fn parse_escape(buf: &[u8]) -> Esc {
    if buf.len() < 2 {
        return Esc::Incomplete;
    }
    if buf[1] != b'[' {
        return Esc::Complete(Token::Other, 2);
    }
    let mut i = 2;
    while i < buf.len() {
        let b = buf[i];
        if b.is_ascii_digit() || b == b';' {
            i += 1;
            continue;
        }
        if (0x40..=0x7e).contains(&b) {
            let params = std::str::from_utf8(&buf[2..i]).unwrap_or("");
            let first: Option<u16> = params.split(';').next().and_then(|s| {
                if s.is_empty() {
                    None
                } else {
                    s.parse().ok()
                }
            });
            let tok = match b {
                b'D' => Token::CursorBack(first.unwrap_or(1)),
                b'K' => Token::EraseLine(first.unwrap_or(0)),
                _ => Token::Other,
            };
            return Esc::Complete(tok, i + 1);
        }
        // A byte that belongs to neither the parameter block nor a final byte: not a CSI
        // sequence we recognize, but complete as far as we can tell.
        return Esc::Complete(Token::Other, i + 1);
    }
    Esc::Incomplete
}

/// The state transition table (`iTermEchoProbeGetNextState`). A token that does not match the
/// one expected next fails the probe; `Off` and `Failed` are absorbing.
pub fn next(s: State, t: &Token) -> State {
    match s {
        State::Off | State::Failed => s,
        State::Waiting => match t {
            Token::Text(text) if text == "*" => State::OneAsterisk,
            _ => State::Failed,
        },
        State::OneAsterisk => match t {
            Token::Backspace => State::BackspaceOverAsterisk,
            Token::CursorBack(1) => State::BackspaceOverAsterisk,
            _ => State::Failed,
        },
        State::BackspaceOverAsterisk => match t {
            Token::Text(text) if text == " " => State::SpaceOverAsterisk,
            Token::EraseLine(0) => State::BackspaceOverSpace,
            _ => State::Failed,
        },
        State::SpaceOverAsterisk => match t {
            Token::Backspace => State::BackspaceOverSpace,
            _ => State::Failed,
        },
        State::BackspaceOverSpace => State::Failed,
    }
}

/// `iTermAdvancedSettingsModel.echoProbeDuration`'s global key and default (0.5 s).
pub const DURATION_KEY: &str = "echoProbeDuration";
pub const DEFAULT_DURATION: f64 = 0.5;

/// What a probe wants the caller to do.
#[derive(Debug, PartialEq)]
pub enum Out {
    Write(Vec<u8>),
    SendPassword,
    Failed,
}

/// One session's echo probe (`iTermEchoProbe`): the state machine over its output, plus the
/// deadline the caller schedules a wake for. Holds no password; the caller sends one on
/// `Out::SendPassword` via `password_bytes`.
pub struct Probe {
    pub state: State,
    deadline: Option<Instant>,
    tok: Tokenizer,
}

impl Probe {
    /// Starts a probe. With the probe setting off, a zero or negative duration, or no backspace
    /// bytes to send, there is nothing to wait for: send the password at once
    /// (`beginProbeWithBackspace:password:`'s early-out paths). Otherwise writes a space then
    /// the backspace and starts waiting.
    pub fn begin(probe_on: bool, duration: f64, backspace: &[u8], now: Instant) -> (Probe, Vec<Out>) {
        if !probe_on || duration <= 0.0 || backspace.is_empty() {
            let probe = Probe { state: State::Off, deadline: None, tok: Tokenizer::new() };
            return (probe, vec![Out::SendPassword]);
        }
        let deadline = now + Duration::from_secs_f64(duration);
        let probe = Probe { state: State::Waiting, deadline: Some(deadline), tok: Tokenizer::new() };
        let outs = vec![Out::Write(b" ".to_vec()), Out::Write(backspace.to_vec())];
        (probe, outs)
    }

    /// Feeds output bytes through the tokenizer and advances the state machine
    /// (`updateEchoProbeStateWithTokenCVector:`). A no-op once the probe is not active.
    pub fn output(&mut self, bytes: &[u8]) {
        if !self.active() {
            return;
        }
        for t in self.tok.feed(bytes) {
            self.state = next(self.state, &t);
            if self.state == State::Failed {
                break;
            }
        }
    }

    /// The instant the caller should wake this probe at, if it is waiting on one.
    pub fn due(&self) -> Option<Instant> {
        self.deadline
    }

    /// Fires the timeout once `now` reaches the deadline (`-[iTermEchoProbe timeout]`): sends
    /// from `Waiting` (nothing came back at all) or `BackspaceOverSpace` (the full dance
    /// completed), fails from every other state, and always ends the probe.
    pub fn tick(&mut self, now: Instant) -> Vec<Out> {
        match self.deadline {
            Some(deadline) if now >= deadline => {
                let out = match self.state {
                    State::Waiting | State::BackspaceOverSpace => Out::SendPassword,
                    _ => Out::Failed,
                };
                self.state = State::Off;
                self.deadline = None;
                vec![out]
            }
            _ => Vec::new(),
        }
    }

    /// `isActive` (`_state != Off`; `Failed` stays active until `tick` fires the timeout).
    pub fn active(&self) -> bool {
        self.state != State::Off
    }

    /// `-[iTermEchoProbe reset]`.
    pub fn reset(&mut self) {
        self.state = State::Off;
        self.deadline = None;
        self.tok = Tokenizer::new();
    }
}

/// The password to write once the probe says to send: the password, with a trailing "\n" when
/// `send_return` (`shouldSendReturnAfterPassword`).
pub fn password_bytes(p: &Secret, send_return: bool) -> Secret {
    if send_return {
        Secret::new(format!("{}\n", p.expose()))
    } else {
        p.duplicate()
    }
}

pub const SENT: &str = "Password sent.";
pub const SENT_PRESS_RETURN: &str = "Password sent (press Return).";
pub const CHECK_TEXT: &str =
    "Are you really at a password prompt? It looks like what you're typing is echoed to the screen.";

/// One broadcast round: which of the probing sessions have reported in, and how
/// (`iTermBroadcastPasswordHelper`'s `_successes`/`_failures`/`_indeterminate`). `indeterminate`
/// counts sessions that went away (`vanished`) and counts toward `done`.
pub struct Round<Id> {
    pending: Vec<Id>,
    total: usize,
    pub ok: Vec<Id>,
    pub failed: Vec<Id>,
    pub indeterminate: usize,
}

impl<Id: PartialEq> Round<Id> {
    pub fn new(probing: Vec<Id>) -> Round<Id> {
        let total = probing.len();
        Round { pending: probing, total, ok: Vec::new(), failed: Vec::new(), indeterminate: 0 }
    }

    /// Records one probing session's result (`addSuccess:`/`addFailure:`). A report for an id
    /// not in `pending` (already reported, or never probing) is ignored.
    pub fn report(&mut self, id: Id, ok: bool) {
        if let Some(pos) = self.pending.iter().position(|p| *p == id) {
            self.pending.remove(pos);
            if ok {
                self.ok.push(id);
            } else {
                self.failed.push(id);
            }
        }
    }

    /// A probing session went away before it reported (the proxy's session is gone): it counts
    /// as indeterminate (`incrementIndeterminateCount`).
    pub fn vanished(&mut self, id: Id) {
        if let Some(pos) = self.pending.iter().position(|p| *p == id) {
            self.pending.remove(pos);
            self.indeterminate += 1;
        }
    }

    /// `checkIfFinished`'s count comparison: successes, failures and indeterminate sessions
    /// together reach the number of probing sessions.
    pub fn done(&self) -> bool {
        self.ok.len() + self.failed.len() + self.indeterminate >= self.total
    }
}

/// What `broadcastPassword:`'s completion block decides once a round is done.
#[derive(Debug, PartialEq)]
pub enum Finish<Id> {
    SendTo(Vec<Id>),
    Ask { heading: &'static str, text: String, buttons: Vec<&'static str>, ok: Vec<Id> },
}

/// All sessions ok sends to all of them at once; any failure asks first, with iTerm2's heading,
/// one-name/many-name texts and buttons (`PseudoTerminal.m broadcastPassword:`). `ok` is what
/// gets sent to if the user picks "Enter Password in Sessions at Prompt" (or is sent to
/// unconditionally when there is no failure).
pub fn finish<Id: Clone>(ok: Vec<Id>, failed_names: Vec<String>) -> Finish<Id> {
    if failed_names.is_empty() {
        return Finish::SendTo(ok);
    }
    let text = if failed_names.len() == 1 {
        format!(
            "The session named \u{201c}{}\u{201d} does not appear to be at a password prompt.",
            failed_names[0]
        )
    } else {
        format!(
            "The following sessions to which input is broadcast do not appear to be at a password prompt: {}",
            oxford(&failed_names)
        )
    };
    let buttons: Vec<&'static str> = if ok.is_empty() {
        vec!["OK"]
    } else {
        vec!["Cancel", "Enter Password in Sessions at Prompt"]
    };
    Finish::Ask { heading: "Not all sessions at password prompt", text, buttons, ok }
}

/// `componentsJoinedWithOxfordComma`: no comma for two, an Oxford comma from three on.
pub fn oxford(names: &[String]) -> String {
    match names.len() {
        0 => String::new(),
        1 => names[0].clone(),
        2 => format!("{} and {}", names[0], names[1]),
        _ => {
            let (last, rest) = names.split_last().expect("checked above");
            format!("{}, and {}", rest.join(", "), last)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_follows_iterm2() {
        let t0 = Instant::now();

        let (mut probe, outs) = Probe::begin(true, 0.5, &[0x7f], t0);
        assert_eq!(outs, vec![Out::Write(b" ".to_vec()), Out::Write(vec![0x7f])]);
        assert_eq!(probe.state, State::Waiting);
        assert_eq!(probe.due(), Some(t0 + Duration::from_secs_f64(0.5)));

        assert_eq!(probe.tick(t0), Vec::new());
        assert_eq!(probe.tick(t0 + Duration::from_secs_f64(0.5)), vec![Out::SendPassword]);
        assert_eq!(probe.state, State::Off);
        assert!(!probe.active());

        // "*" then BS then " " then BS reaches BackspaceOverSpace; the timeout sends.
        let (mut probe, _) = Probe::begin(true, 0.5, &[0x7f], t0);
        probe.output(b"*");
        probe.output(&[0x08]);
        probe.output(b" ");
        probe.output(&[0x08]);
        assert_eq!(probe.state, State::BackspaceOverSpace);
        assert_eq!(probe.tick(t0 + Duration::from_secs_f64(0.5)), vec![Out::SendPassword]);

        // "*", CSI 1 D, CSI K reaches the same state the same way.
        let (mut probe, _) = Probe::begin(true, 0.5, &[0x7f], t0);
        probe.output(b"*");
        probe.output(b"\x1b[D");
        probe.output(b"\x1b[K");
        assert_eq!(probe.state, State::BackspaceOverSpace);
        assert_eq!(probe.tick(t0 + Duration::from_secs_f64(0.5)), vec![Out::SendPassword]);

        // A shell echoing the space back (no asterisk first) fails, and the timeout reports it.
        let (mut probe, _) = Probe::begin(true, 0.5, &[0x7f], t0);
        probe.output(b" ");
        assert_eq!(probe.state, State::Failed);
        assert_eq!(probe.tick(t0 + Duration::from_secs_f64(0.5)), vec![Out::Failed]);

        // Two asterisks arriving as one chunk are one Text token, so they never match "*".
        let (mut probe, _) = Probe::begin(true, 0.5, &[0x7f], t0);
        probe.output(b"**");
        assert_eq!(probe.state, State::Failed);

        // Probe off, zero duration, or no backspace bytes: send at once, no wait.
        let (probe, outs) = Probe::begin(false, 0.5, &[0x7f], t0);
        assert_eq!(outs, vec![Out::SendPassword]);
        assert!(!probe.active());

        let (probe, outs) = Probe::begin(true, 0.0, &[0x7f], t0);
        assert_eq!(outs, vec![Out::SendPassword]);
        assert!(!probe.active());

        let (probe, outs) = Probe::begin(true, 0.5, &[], t0);
        assert_eq!(outs, vec![Out::SendPassword]);
        assert!(!probe.active());

        // password_bytes appends the return key only when asked.
        let pw = Secret::new("pw".to_string());
        assert_eq!(password_bytes(&pw, true).expose(), "pw\n");
        let pw2 = Secret::new("pw".to_string());
        assert_eq!(password_bytes(&pw2, false).expose(), "pw");

        // A split escape sequence carries over between feed calls.
        let (mut probe, _) = Probe::begin(true, 0.5, &[0x7f], t0);
        probe.output(b"*");
        probe.output(b"\x1b[D");
        probe.output(b"\x1b");
        probe.output(b"[K");
        assert_eq!(probe.state, State::BackspaceOverSpace);
    }

    #[test]
    fn broadcast_round_follows_iterm2() {
        assert_eq!(finish::<i32>(vec![1, 2], vec![]), Finish::SendTo(vec![1, 2]));

        assert_eq!(
            finish(vec![1], vec!["build".to_string()]),
            Finish::Ask {
                heading: "Not all sessions at password prompt",
                text: "The session named \u{201c}build\u{201d} does not appear to be at a password prompt."
                    .to_string(),
                buttons: vec!["Cancel", "Enter Password in Sessions at Prompt"],
                ok: vec![1],
            }
        );

        assert_eq!(
            finish::<i32>(vec![], vec!["a".to_string(), "b".to_string(), "c".to_string()]),
            Finish::Ask {
                heading: "Not all sessions at password prompt",
                text: "The following sessions to which input is broadcast do not appear to be at a password prompt: a, b, and c".to_string(),
                buttons: vec!["OK"],
                ok: vec![],
            }
        );

        assert_eq!(oxford(&["a".to_string(), "b".to_string()]), "a and b");
        assert_eq!(oxford(&["a".to_string(), "b".to_string(), "c".to_string()]), "a, b, and c");
        assert_eq!(oxford(&["a".to_string()]), "a");
        assert_eq!(oxford(&[]), "");

        let mut round = Round::new(vec![1, 2, 3]);
        assert!(!round.done());
        round.report(1, true);
        assert!(!round.done());
        round.report(2, false);
        assert!(!round.done());
        round.report(3, true);
        assert!(round.done());
        assert_eq!(round.ok, vec![1, 3]);
        assert_eq!(round.failed, vec![2]);

        // `checkIfFinished` counts sessions that went away (indeterminate) as reported.
        let mut round = Round::new(vec![1, 2]);
        round.report(1, true);
        round.vanished(2);
        assert!(round.done());
        assert_eq!(round.indeterminate, 1);
        assert!(round.failed.is_empty());
        let mut round = Round::new(vec![1, 2]);
        round.report(1, false);
        round.indeterminate += 1;
        assert!(round.done(), "a directly bumped count finishes the round too");
    }
}
