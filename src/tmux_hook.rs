//! tmux control mode's entry and line reader (iTerm2's DCS 1000p hook and VT100TmuxParser):
//! finds `ESC P 1000 p` in a pane's output (across reads), then splits everything after it
//! into lines until `%exit`. Bytes before the start and after the exit go to the screen.

/// The DCS that starts tmux control mode.
pub const START: &[u8] = b"\x1bP1000p";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Out {
    /// Ordinary output for the pane's screen.
    Screen(Vec<u8>),
    /// tmux control mode began.
    Enter,
    /// One protocol line (raw bytes, carriage returns dropped).
    Line(Vec<u8>),
    /// The `%exit` line: control mode is over.
    Exit(Vec<u8>),
}

#[derive(Default)]
pub struct Hook {
    control: bool,
    /// The end of the last read that may begin `ESC P 1000 p`.
    held: Vec<u8>,
    line: Vec<u8>,
    /// The `%begin` block being read: its time and number.
    block: Option<(String, String)>,
    /// Reattaching to a live connection: the first line may be half a notification.
    recovery: bool,
    /// The client behind this hook was ended by Termurion (a psmux detach): whatever it still
    /// writes is control-mode text, never output for the screen.
    sealed: bool,
}

impl Hook {
    /// A hook that joins a control-mode stream already running (session restoration).
    pub fn recovering() -> Hook {
        Hook { control: true, recovery: true, ..Default::default() }
    }

    pub fn active(&self) -> bool {
        self.control
    }

    /// Leaves control mode without an `%exit` (iTerm2's forceUnhookDCS): what follows is
    /// output again.
    pub fn unhook(&mut self) {
        *self = Hook::default();
    }

    /// Leaves control mode and drops everything after it: the client is being ended, and its
    /// last writes (`%output` lines) must never reach the screen.
    pub fn seal(&mut self) {
        *self = Hook { sealed: true, ..Default::default() };
    }

    pub fn sealed(&self) -> bool {
        self.sealed
    }

    pub fn feed(&mut self, bytes: &[u8]) -> Vec<Out> {
        if self.sealed {
            return Vec::new();
        }
        let mut data = std::mem::take(&mut self.held);
        data.extend_from_slice(bytes);
        let mut out = Vec::new();
        let mut screen = Vec::new();
        let mut i = 0;
        while i < data.len() {
            let rest = &data[i..];
            if !self.control {
                match rest.windows(START.len()).position(|w| w == START) {
                    Some(k) => {
                        screen.extend_from_slice(&rest[..k]);
                        if !screen.is_empty() {
                            out.push(Out::Screen(std::mem::take(&mut screen)));
                        }
                        out.push(Out::Enter);
                        self.control = true;
                        self.block = None;
                        i += k + START.len();
                    }
                    None => {
                        // A tail that may begin the start sequence waits for the next read.
                        let keep = (1..START.len()).rev().find(|n| *n <= rest.len() && rest.ends_with(&START[..*n])).unwrap_or(0);
                        screen.extend_from_slice(&rest[..rest.len() - keep]);
                        self.held = rest[rest.len() - keep..].to_vec();
                        i = data.len();
                    }
                }
                continue;
            }
            match rest.iter().position(|b| *b == b'\n') {
                None => {
                    self.line.extend(rest.iter().filter(|b| **b != b'\r'));
                    i = data.len();
                    // psmux ends a `detach` reply with `%exit` and no newline.
                    if split_glued_exit(&self.line).is_some() {
                        let line = std::mem::take(&mut self.line);
                        self.finish_line(line, &mut out);
                    }
                }
                Some(k) => {
                    self.line.extend(rest[..k].iter().filter(|b| **b != b'\r'));
                    i += k + 1;
                    let line = std::mem::take(&mut self.line);
                    self.finish_line(line, &mut out);
                }
            }
        }
        if !screen.is_empty() {
            out.push(Out::Screen(screen));
        }
        out
    }

    /// A whole line: psmux glues `%exit` to the end of the `%begin` line of its `detach` reply,
    /// so that line is two.
    fn finish_line(&mut self, line: Vec<u8>, out: &mut Vec<Out>) {
        let parts = match split_glued_exit(&line).map(|(head, exit)| (head.to_vec(), exit.to_vec())) {
            Some((head, exit)) => vec![head, exit],
            None => vec![line],
        };
        for line in parts {
            if let Some(o) = self.process(line) {
                if matches!(o, Out::Exit(_)) {
                    self.control = false;
                }
                out.push(o);
            }
        }
    }

    /// One line (iTerm2's processLineIntoToken): the reply block it opens or closes, and
    /// whether it is the exit.
    fn process(&mut self, line: Vec<u8>) -> Option<Out> {
        let text = String::from_utf8_lossy(&line).into_owned();
        if std::mem::take(&mut self.recovery) && !text.starts_with("%begin") && !text.starts_with("%exit") {
            return None;
        }
        let parts: Vec<&str> = text.split(' ').collect();
        if let Some((id, number)) = &self.block {
            // tmux 1.8 can print %exit inside a block: it still ends control mode.
            if text.starts_with("%exit") {
                self.block = None;
                return Some(Out::Exit(line));
            }
            if (text.starts_with("%end ") || text.starts_with("%error ")) && parts.len() >= 3 && parts[1] == id && parts[2] == number {
                self.block = None;
            }
        } else if text.starts_with("%begin") {
            if parts.len() >= 3 {
                self.block = Some((parts[1].to_string(), parts[2].to_string()));
            }
        } else if text.starts_with("%exit") {
            return Some(Out::Exit(line));
        }
        Some(Out::Line(line))
    }
}

/// A `%begin` line with `%exit` glued to its end (psmux ends control mode this way after
/// `detach`): the `%begin` part and the `%exit`. Real tmux never prints this line.
fn split_glued_exit(line: &[u8]) -> Option<(&[u8], &[u8])> {
    let head = line.strip_suffix(b"%exit")?;
    (head.starts_with(b"%begin ") && head.last().is_some_and(u8::is_ascii_digit)).then(|| (head, &line[head.len()..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(b: &[u8]) -> Out {
        Out::Screen(b.to_vec())
    }

    fn l(b: &str) -> Out {
        Out::Line(b.as_bytes().to_vec())
    }

    /// A client that Termurion ended (a psmux detach) may still write control-mode lines: they
    /// never reach the screen, whether they come whole, in pieces, or with a new DCS.
    #[test]
    fn a_sealed_hook_drops_everything() {
        let mut h = Hook::default();
        h.feed(b"\x1bP1000p%begin 1 2 0\n");
        assert!(h.active());
        h.seal();
        assert!(h.sealed() && !h.active());
        assert!(h.feed(b"%output %4 \\033[38;5;238m x\r\n").is_empty());
        assert!(h.feed(b"%output %4 par").is_empty());
        assert!(h.feed(b"tial\n\x1bP1000p%begin 1 2 0\n").is_empty());
        assert!(!h.active(), "a sealed hook does not enter control mode again");
    }

    #[test]
    fn hook_finds_the_start() {
        let mut h = Hook::default();
        assert_eq!(h.feed(b"hello\x1bP1000p%begin 1 2 0\r\n"), vec![s(b"hello"), Out::Enter, l("%begin 1 2 0")]);
        assert!(h.active());
        assert_eq!(Hook::default().feed(b"plain \x1bP1001p text"), vec![s(b"plain \x1bP1001p text")], "another DCS is output");
    }

    #[test]
    fn unhook_returns_to_the_screen() {
        let mut h = Hook::default();
        assert_eq!(h.feed(b"\x1bP1000p%begin 1 1 0\r\n%end"), vec![Out::Enter, l("%begin 1 1 0")]);
        h.unhook();
        assert!(!h.active());
        assert_eq!(h.feed(b" 1 1 0\r\n%exit\r\n\x1b\\$ "), vec![s(b" 1 1 0\r\n%exit\r\n\x1b\\$ ")], "X: tmux's lines show as text");
    }

    #[test]
    fn hook_survives_chunk_splits() {
        let mut h = Hook::default();
        assert_eq!(h.feed(b"ab\x1bP10"), vec![s(b"ab")], "a possible start waits");
        assert_eq!(h.feed(b"00p%x"), vec![Out::Enter]);
        assert_eq!(h.feed(b"yz\r"), vec![]);
        assert_eq!(h.feed(b"\n"), vec![l("%xyz")]);
        let mut h = Hook::default();
        assert_eq!(h.feed(b"\x1bP1"), vec![]);
        assert_eq!(h.feed(b"x"), vec![s(b"\x1bP1x")], "a false start goes to the screen whole");
    }

    #[test]
    fn lines_drop_carriage_returns() {
        let mut h = Hook::default();
        let out = h.feed(b"\x1bP1000p%output %1 a\\015b\r\n%exit\r\n\x1b\\after");
        assert_eq!(out, vec![Out::Enter, l("%output %1 a\\015b"), Out::Exit(b"%exit".to_vec()), s(b"\x1b\\after")]);
        assert!(!h.active());
        assert_eq!(h.feed(b"more"), vec![s(b"more")]);
    }

    #[test]
    fn exit_inside_a_reply_ends_tmux_mode() {
        // iTerm2's tmux 1.8 workaround: %exit ends control mode inside a reply block too.
        let mut h = Hook::default();
        let out = h.feed(b"\x1bP1000p%begin 5 7 1\n%exit\n%end 5 7 1\n");
        assert_eq!(out[..3], [Out::Enter, l("%begin 5 7 1"), Out::Exit(b"%exit".to_vec())]);
        // An %end for another block does not close this one.
        let mut h = Hook::default();
        h.feed(b"\x1bP1000p%begin 5 7 1\n%end 9 9 1\n");
        assert!(h.block.is_some());
        h.feed(b"%end 5 7 1\n");
        assert!(h.block.is_none());
    }

    #[test]
    fn recovery_skips_half_a_line() {
        let mut h = Hook::recovering();
        assert_eq!(h.feed(b"put %1 abc\n%output %1 d\n"), vec![l("%output %1 d")]);
        let mut h = Hook::recovering();
        assert_eq!(h.feed(b"%begin 1 1 1\n"), vec![l("%begin 1 1 1")]);
    }

    /// The raw start of a recorded session: the screen text before the DCS, the lines after it.
    /// `wsl-3.4-shipped` ends in `%exit`; psmux 3.3.8 sends none after `kill-session`.
    #[test]
    fn transcripts_parse() {
        let cases: [(&str, &str, bool); 2] = [("wsl-3.4-shipped", "%begin 1790341902 273 0", true), ("psmux-3.3.8-start", "%begin 1791960881 1 0", false)];
        for (name, first, exits) in cases {
            let bytes = std::fs::read(format!("tests/fixtures/tmux/{name}.bin")).unwrap();
            // Any read boundary gives the same result.
            for size in [1, 7, 64, bytes.len()] {
                let mut h = Hook::default();
                let out: Vec<Out> = bytes.chunks(size).flat_map(|c| h.feed(c)).collect();
                let lines: Vec<&Out> = out.iter().filter(|o| matches!(o, Out::Line(_))).collect();
                assert!(matches!(out.iter().find(|o| !matches!(o, Out::Screen(_))), Some(Out::Enter)), "{name} size {size}");
                assert_eq!(lines.first(), Some(&&l(first)), "{name}");
                assert!(lines.iter().any(|o| matches!(o, Out::Line(b) if b.len() == 300)), "{name}: the long reply line is whole");
                assert_eq!(matches!(out.iter().rev().find(|o| !matches!(o, Out::Screen(_))), Some(Out::Exit(_))), exits, "{name}");
                assert_eq!(h.active(), !exits, "{name}: without %exit the hook stays in control mode");
                let screen: Vec<u8> = out.iter().filter_map(|o| if let Out::Screen(b) = o { Some(b.clone()) } else { None }).flatten().collect();
                let tail: &[u8] = if exits { b"\x1b\\" } else { b"" };
                assert_eq!(screen, [&b"\x1b[1t\x1b[c\x1b[?1004h\x1b[?9001h"[..], tail].concat(), "{name} size {size}");
            }
        }
    }

    #[test]
    fn psmux_detach_glues_the_exit_to_the_begin_line() {
        let mut h = Hook::default();
        let out = h.feed(b"\x1bP1000p%begin 1791963131 62 1%exit\n\x1b\\tail");
        assert_eq!(out, vec![Out::Enter, l("%begin 1791963131 62 1"), Out::Exit(b"%exit".to_vec()), s(b"\x1b\\tail")]);
        assert!(!h.active(), "control mode is over");
        // psmux sends no newline after it: the exit is found in the partial line, at any read size.
        for size in [1, 5, 64] {
            let mut h = Hook::default();
            let bytes = b"\x1bP1000p%begin 1791963131 62 1%exit";
            let out: Vec<Out> = bytes.chunks(size).flat_map(|c| h.feed(c)).collect();
            assert_eq!(out, vec![Out::Enter, l("%begin 1791963131 62 1"), Out::Exit(b"%exit".to_vec())], "size {size}");
            assert!(!h.active());
            assert_eq!(h.feed(b"\n\x1b\\"), vec![s(b"\n\x1b\\")]);
        }
        // A reply line that merely ends in the word is data, and a bare %exit is as before.
        let mut h = Hook::default();
        let out = h.feed(b"\x1bP1000p%begin 1 2 1\nsay %exit\n%end 1 2 1\n%exit\n");
        assert_eq!(out, vec![Out::Enter, l("%begin 1 2 1"), l("say %exit"), l("%end 1 2 1"), Out::Exit(b"%exit".to_vec())]);
        assert_eq!(split_glued_exit(b"%begin 1 2 1%exit"), Some((&b"%begin 1 2 1"[..], &b"%exit"[..])));
        assert_eq!(split_glued_exit(b"%begin 1 2 1 %exit"), None);
        assert_eq!(split_glued_exit(b"%output %1 a1%exit"), None);
    }

    /// psmux ends its lines with a bare newline, tmux with CR LF: both give the same lines.
    #[test]
    fn psmux_lines_end_in_newline_only() {
        let bytes = std::fs::read("tests/fixtures/tmux/psmux-3.3.8-start.bin").unwrap();
        assert!(!bytes.windows(2).any(|w| w == b"\r\n"), "the recording has no CR LF");
        let mut h = Hook::default();
        let out = h.feed(&bytes);
        let session = out.iter().find_map(|o| match o {
            Out::Line(b) if b.starts_with(b"%session-changed") => Some(String::from_utf8_lossy(b).into_owned()),
            _ => None,
        });
        assert_eq!(session.as_deref(), Some("%session-changed $335 termurion-test-rec"));
    }
}
