//! A side channel's bytes as iTerm2's text pieces (milestone 14c): what
//! `VT100Terminal.m:1776-1811` keeps of a background process's output after the parser has
//! split it into tokens (`VT100StringParser.m:270-311`).

/// Where the reducer is in an escape sequence.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Mode {
    #[default]
    Ground,
    /// After ESC.
    Esc,
    /// ESC and intermediate bytes, waiting for the final one.
    Intermediate,
    /// A CSI sequence, waiting for its final byte.
    Csi,
    /// A string sequence (OSC, DCS, APC, PM, SOS), waiting for BEL or ESC backslash.
    Text,
    /// ESC inside a string sequence.
    TextEsc,
}

/// Reduces one pid's side-channel bytes to text pieces. State lives across reads: an escape
/// sequence or a UTF-8 character can be split between two `%output` blocks. Nothing is kept
/// that grows with the input: an escape that never ends holds no bytes.
#[derive(Debug, Default)]
pub struct Reducer {
    mode: Mode,
    /// The start of a UTF-8 character that the last read ended in.
    pending: Vec<u8>,
}

impl Reducer {
    /// The pieces this read completes, in order. An ASCII run (0x20 to 0x7F, with CR LF pairs)
    /// is one piece, a lone CR or LF is one, a non-ASCII run is one; tabs, other controls and
    /// escape sequences are dropped and end the piece before them (`SP:270-311`,
    /// `T:1776-1811`).
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<String> {
        let mut out = Vec::new();
        let mut ascii = String::new();
        let mut high: Vec<u8> = std::mem::take(&mut self.pending);
        let mut i = 0;
        while i < bytes.len() {
            let b = bytes[i];
            i += 1;
            if b >= 0x80 && self.mode == Mode::Ground {
                flush(&mut out, &mut ascii);
                high.push(b);
                continue;
            }
            // A run of high bytes ends at the first other byte.
            flush_high(&mut out, &mut high, false);
            match self.mode {
                Mode::Ground => match b {
                    0x20..=0x7f => ascii.push(b as char),
                    // A CR LF pair stays in the run; any other CR or LF is its own piece.
                    13 if bytes.get(i) == Some(&10) => {
                        ascii.push_str("\r\n");
                        i += 1;
                    }
                    13 | 10 => {
                        flush(&mut out, &mut ascii);
                        out.push(if b == 13 { "\r" } else { "\n" }.to_string());
                    }
                    0x1b => {
                        flush(&mut out, &mut ascii);
                        self.mode = Mode::Esc;
                    }
                    // Tabs, BEL and the other controls.
                    _ => flush(&mut out, &mut ascii),
                },
                Mode::Esc => {
                    self.mode = match b {
                        b'[' => Mode::Csi,
                        b']' | b'P' | b'X' | b'^' | b'_' => Mode::Text,
                        0x20..=0x2f => Mode::Intermediate,
                        0x1b => Mode::Esc,
                        _ => Mode::Ground,
                    }
                }
                Mode::Intermediate => {
                    self.mode = match b {
                        0x20..=0x2f => Mode::Intermediate,
                        0x1b => Mode::Esc,
                        _ => Mode::Ground,
                    }
                }
                Mode::Csi => {
                    self.mode = match b {
                        0x40..=0x7e => Mode::Ground,
                        0x1b => Mode::Esc,
                        _ => Mode::Csi,
                    }
                }
                Mode::Text => {
                    self.mode = match b {
                        0x07 => Mode::Ground,
                        0x1b => Mode::TextEsc,
                        _ => Mode::Text,
                    }
                }
                Mode::TextEsc => {
                    self.mode = match b {
                        b'\\' => Mode::Ground,
                        0x1b => Mode::TextEsc,
                        // Not a terminator: the string is cut and this is a new escape.
                        b'[' => Mode::Csi,
                        b']' => Mode::Text,
                        _ => Mode::Ground,
                    }
                }
            }
        }
        flush(&mut out, &mut ascii);
        flush_high(&mut out, &mut high, true);
        self.pending = high;
        out
    }
}

fn flush(out: &mut Vec<String>, ascii: &mut String) {
    if !ascii.is_empty() {
        out.push(std::mem::take(ascii));
    }
}

/// Turns collected high bytes into a piece. `last` is the end of the read: a character cut
/// short there stays in `high` for the next read, and at most 3 bytes do.
fn flush_high(out: &mut Vec<String>, high: &mut Vec<u8>, last: bool) {
    if high.is_empty() {
        return;
    }
    let mut keep = 0;
    if last {
        // The last lead byte within 3 bytes of the end, when its character is not complete.
        let n = high.len();
        if let Some(k) = (1..=3.min(n)).find(|k| high[n - k] >= 0xc0) {
            let lead = high[n - k];
            let need = if lead >= 0xf0 { 4 } else if lead >= 0xe0 { 3 } else { 2 };
            if k < need && high[n - k + 1..].iter().all(|c| (0x80..0xc0).contains(c)) {
                keep = k;
            }
        }
    }
    let rest = high.split_off(high.len() - keep);
    if !high.is_empty() {
        out.push(String::from_utf8_lossy(high).into_owned());
    }
    *high = rest;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn joined(pieces: &[String]) -> String {
        pieces.concat()
    }

    #[test]
    fn reduce_follows_iterm2() {
        let got = Reducer::default().feed(b"ab\r\ncd\nEOF\n\x1b[1mX\x07\ty\x1b]0;t\x1b\\z\r");
        assert_eq!(got, ["ab\r\ncd", "\n", "EOF", "\n", "X", "y", "z", "\r"]);
        // Non-ASCII runs are their own pieces; only the joined text is fixed.
        assert_eq!(joined(&Reducer::default().feed("caf\u{e9}".as_bytes())), "caf\u{e9}");
        assert_eq!(joined(&Reducer::default().feed(b"a\xffb")), "a\u{fffd}b", "an invalid byte");
        // Escapes and characters split across reads.
        let mut r = Reducer::default();
        let mut all = r.feed(b"a\x1b[");
        all.extend(r.feed(b"1mb"));
        assert_eq!(joined(&all), "ab");
        let mut r = Reducer::default();
        let mut all = r.feed(b"a\x1b]0;");
        all.extend(r.feed(b"t\x07b"));
        assert_eq!(joined(&all), "ab");
        let mut r = Reducer::default();
        let mut all = r.feed(b"a\x1b]0;t\x1b");
        all.extend(r.feed(b"\\b"));
        assert_eq!(joined(&all), "ab", "an OSC ended by ESC backslash across reads");
        let mut r = Reducer::default();
        let mut all = r.feed(b"caf\xc3");
        all.extend(r.feed(b"\xa9!"));
        assert_eq!(joined(&all), "caf\u{e9}!", "a character split across reads");
        // Every other control is dropped; a lone CR or LF is its own piece.
        assert_eq!(Reducer::default().feed(b"\r\nx\r\r\n\n"), ["\r\nx", "\r", "\r\n", "\n"]);
        assert_eq!(joined(&Reducer::default().feed(b"a\x00b\x08c\x7fd")), "abc\x7fd");
    }

    #[test]
    fn reduce_is_bounded_and_total() {
        // A hostile stream: an escape that never ends holds no text and no memory.
        let mut r = Reducer::default();
        let mut got = r.feed(b"a\x1b]0;");
        for _ in 0..64 {
            got.extend(r.feed(&vec![b'x'; 65536]));
        }
        assert_eq!(joined(&got), "a");
        // Every byte value is accepted at every split.
        let all: Vec<u8> = (0..=255u8).collect();
        for k in 0..=all.len() {
            let mut r = Reducer::default();
            r.feed(&all[..k]);
            r.feed(&all[k..]);
        }
    }
}
