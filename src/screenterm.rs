//! TERM=screen support (iTerm2's translateFromScreenTerminal): in a session whose terminal
//! type is screen-like, SGR italics (3, 23) become reverse video (7, 27) and DECSET 5
//! (reverse screen) is ignored. A tmux pane's type is tmux's `default-terminal`.

/// iTerm2's isScreenLike: "screen" anywhere, or "tmux" with
/// convertItalicsToReverseVideoForTmuxBugwardsCompatible.
pub fn screen_like(term: &str, bugwards: bool) -> bool {
    term.contains("screen") || (bugwards && term.contains("tmux"))
}

/// Rewrites CSI sequences as they pass; a sequence split across reads waits for its end.
#[derive(Clone, Debug, Default)]
pub struct Filter {
    /// convertItalicsToReverseVideoForTmux.
    pub italics: bool,
    held: Vec<u8>,
}

impl Filter {
    pub fn new(italics: bool) -> Filter {
        Filter { italics, held: Vec::new() }
    }

    /// The pane's contents are replaced (a tmux unpause): a held partial sequence is dropped.
    pub fn reset(&mut self) {
        self.held.clear();
    }

    pub fn feed(&mut self, bytes: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(bytes.len());
        for &b in bytes {
            if self.held.is_empty() {
                if b == 0x1b {
                    self.held.push(b);
                } else {
                    out.push(b);
                }
                continue;
            }
            self.held.push(b);
            if self.held.len() == 2 {
                if b != b'[' {
                    out.append(&mut self.held);
                }
                continue;
            }
            if (0x40..=0x7e).contains(&b) {
                let seq = std::mem::take(&mut self.held);
                out.extend(self.rewrite(seq));
            } else if !(0x20..=0x3f).contains(&b) || self.held.len() > MAX_CSI {
                // Not a CSI after all (or too long to be one): it passes as it came.
                out.append(&mut self.held);
            }
        }
        out
    }

    /// A whole `ESC [ ... final` sequence.
    fn rewrite(&self, seq: Vec<u8>) -> Vec<u8> {
        let body = &seq[2..seq.len() - 1];
        let last = seq[seq.len() - 1];
        let Ok(text) = std::str::from_utf8(body) else { return seq };
        if last == b'm' && self.italics && text.bytes().all(|c| c.is_ascii_digit() || c == b';' || c == b':') {
            let params: Vec<&str> = text
                .split(';')
                .map(|p| match p.parse::<u32>() {
                    Ok(3) if !p.contains(':') => "7",
                    Ok(23) if !p.contains(':') => "27",
                    _ => p,
                })
                .collect();
            return format!("\x1b[{}m", params.join(";")).into_bytes();
        }
        if (last == b'h' || last == b'l') && text.starts_with('?') && text[1..].bytes().all(|c| c.is_ascii_digit() || c == b';') {
            let params: Vec<&str> = text[1..].split(';').filter(|p| p.parse::<u32>() != Ok(5)).collect();
            if params.is_empty() {
                return Vec::new();
            }
            return format!("\x1b[?{}{}", params.join(";"), last as char).into_bytes();
        }
        seq
    }
}

/// Longer CSI sequences are not rewritten.
const MAX_CSI: usize = 256;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screen_terms_are_found() {
        assert!(screen_like("screen-256color", false));
        assert!(screen_like("screen", false));
        assert!(!screen_like("tmux-256color", false));
        assert!(screen_like("tmux-256color", true));
        assert!(!screen_like("xterm-256color", true));
    }

    #[test]
    fn italics_become_reverse_video() {
        let mut f = Filter::new(true);
        assert_eq!(f.feed(b"a\x1b[3;23;1mb"), b"a\x1b[7;27;1mb");
        assert_eq!(f.feed(b"x\x1b[3"), b"x");
        assert_eq!(f.feed(b";23m"), b"\x1b[7;27m");
        assert_eq!(f.feed(b"\x1b[3:1m\x1b[33m\x1b[m"), b"\x1b[3:1m\x1b[33m\x1b[m", "sub-parameters and other numbers stay");
        assert_eq!(f.feed(b"\x1b[?5h\x1b[?5l"), b"", "reverse screen is ignored");
        assert_eq!(f.feed(b"\x1b[?5;1h"), b"\x1b[?1h");
        assert_eq!(f.feed(b"\x1b[5h\x1b]0;t\x07\x1bM"), b"\x1b[5h\x1b]0;t\x07\x1bM", "other bytes pass");
        assert_eq!(f.feed(b"\x1b[3"), b"");
        f.reset();
        assert_eq!(f.feed(b"hello"), b"hello", "a reset drops the half sequence");
        let mut off = Filter::new(false);
        assert_eq!(off.feed(b"\x1b[3m\x1b[?5h"), b"\x1b[3m");
        assert_eq!(off.feed(b"\x1b"), b"");
        assert_eq!(off.feed(b"[3m"), b"\x1b[3m");
    }
}
