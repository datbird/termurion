//! tmux control mode's entry and line reader (iTerm2's DCS 1000p hook and VT100TmuxParser):
//! finds `ESC P 1000 p` in a pane's output (across reads), then splits everything after it
//! into lines until `%exit`. Bytes before the start and after the exit go to the screen.

const START: &[u8] = b"\x1bP1000p";

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

    pub fn feed(&mut self, bytes: &[u8]) -> Vec<Out> {
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
                }
                Some(k) => {
                    self.line.extend(rest[..k].iter().filter(|b| **b != b'\r'));
                    i += k + 1;
                    let line = std::mem::take(&mut self.line);
                    if let Some(o) = self.process(line) {
                        if matches!(o, Out::Exit(_)) {
                            self.control = false;
                        }
                        out.push(o);
                    }
                }
            }
        }
        if !screen.is_empty() {
            out.push(Out::Screen(screen));
        }
        out
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

#[cfg(test)]
mod tests {
    use super::*;

    fn s(b: &[u8]) -> Out {
        Out::Screen(b.to_vec())
    }

    fn l(b: &str) -> Out {
        Out::Line(b.as_bytes().to_vec())
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

    #[test]
    fn transcripts_parse() {
        let bytes = std::fs::read("tests/fixtures/tmux/wsl-3.4-shipped.bin").unwrap();
        // Any read boundary gives the same result.
        for size in [1, 7, 64, bytes.len()] {
            let mut h = Hook::default();
            let out: Vec<Out> = bytes.chunks(size).flat_map(|c| h.feed(c)).collect();
            let lines: Vec<&Out> = out.iter().filter(|o| matches!(o, Out::Line(_))).collect();
            assert!(matches!(out.iter().find(|o| !matches!(o, Out::Screen(_))), Some(Out::Enter)), "size {size}");
            assert_eq!(lines.first(), Some(&&l("%begin 1790341902 273 0")));
            assert!(lines.iter().any(|o| matches!(o, Out::Line(b) if b.len() == 300)), "the long reply line is whole");
            assert!(matches!(out.iter().rev().find(|o| !matches!(o, Out::Screen(_))), Some(Out::Exit(_))));
            let screen: Vec<u8> = out.iter().filter_map(|o| if let Out::Screen(b) = o { Some(b.clone()) } else { None }).flatten().collect();
            assert_eq!(screen, b"\x1b[1t\x1b[c\x1b[?1004h\x1b[?9001h\x1b\\".to_vec(), "size {size}");
        }
    }
}
