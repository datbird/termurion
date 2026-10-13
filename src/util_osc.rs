//! The utilities' shared writer (milestone 11e): the scripts' `print_osc` / `print_st` (the
//! tmux wrapping they choose by `TERM`), macOS `base64` (one unwrapped line), `base64 -di`
//! and `fold -w N`, and the streamed `base64 < file` the file utilities share. Pure; each
//! utility builds its bytes from these.

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};

/// `print_osc` under tmux or screen: the DCS passthrough head, then an escaped `ESC ]`.
pub const TMUX_OSC: &str = "\x1bPtmux;\x1b\x1b]";

/// `print_st` under tmux or screen: BEL ends the inner OSC, `ESC \` the passthrough.
pub const TMUX_ST: &str = "\x07\x1b\\";

/// `[[ $TERM == screen* || $TERM == tmux* ]]`: case-sensitive, an unset `TERM` is neither.
pub fn wrapped(term: Option<&str>) -> bool {
    term.is_some_and(|t| t.starts_with("screen") || t.starts_with("tmux"))
}

/// `print_osc`.
pub fn osc(term: Option<&str>) -> &'static str {
    if wrapped(term) { TMUX_OSC } else { "\x1b]" }
}

/// `print_st`.
pub fn st(term: Option<&str>) -> &'static str {
    if wrapped(term) { TMUX_ST } else { "\x07" }
}

/// `print_osc; printf "<body>"; print_st`.
pub fn seq(term: Option<&str>, body: &str) -> String {
    format!("{}{body}{}", osc(term), st(term))
}

/// macOS `base64` inside `$(...)`: standard alphabet, padded, one line, the newline dropped.
pub fn b64(bytes: &[u8]) -> String {
    STANDARD.encode(bytes)
}

/// macOS `base64` piped: one line and its newline.
pub fn b64_line(bytes: &[u8]) -> String {
    let mut s = b64(bytes);
    s.push('\n');
    s
}

/// Reads until `buf` is full or the input ends (an interrupted read is tried again); the
/// count, and whether a read failed.
pub fn fill(r: &mut dyn std::io::Read, buf: &mut [u8]) -> (usize, bool) {
    let mut n = 0;
    while n < buf.len() {
        match r.read(&mut buf[n..]) {
            Ok(0) => break,
            Ok(k) => n += k,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return (n, true),
        }
    }
    (n, false)
}

/// `base64 < source` as one unwrapped line without its newline, streamed: `each` gets the
/// base64 of each `piece` bytes read (a multiple of 3, so the pieces join into the whole
/// input's text). A read error ends the data there. Returns the number of bytes read.
pub fn stream_b64(r: &mut dyn std::io::Read, piece: usize, each: &mut dyn FnMut(&str)) -> u64 {
    debug_assert!(piece > 0 && piece % 3 == 0);
    let mut buf = vec![0u8; piece];
    let mut total = 0u64;
    loop {
        let (n, failed) = fill(r, &mut buf);
        if n > 0 {
            total += n as u64;
            each(&b64(&buf[..n]));
        }
        if n < piece || failed {
            return total;
        }
    }
}

/// `base64 -di`: characters outside the alphabet (whitespace, padding) are ignored and an
/// unpadded tail is accepted; a lone last character (never whole bytes) is dropped.
pub fn unb64(text: &str) -> Vec<u8> {
    const LOOSE: GeneralPurpose = GeneralPurpose::new(
        &base64::alphabet::STANDARD,
        GeneralPurposeConfig::new().with_decode_padding_mode(DecodePaddingMode::RequireNone).with_decode_allow_trailing_bits(true),
    );
    let mut clean: String = text.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '+' || *c == '/').collect();
    if clean.len() % 4 == 1 {
        clean.pop();
    }
    LOOSE.decode(clean).unwrap_or_default()
}

/// `fold -w <width>` then bash word splitting: each line cut into pieces of `width`
/// characters, empty lines dropped.
pub fn fold(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut out = Vec::new();
    for line in text.split('\n') {
        let chars: Vec<char> = line.chars().collect();
        out.extend(chars.chunks(width).map(|c| c.iter().collect::<String>()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::osc1337::Attention;
    use crate::screen::{Request, Screen};

    const PLAIN: &str = "\x1b]1337;X\x07";
    const WRAPPED: &str = "\x1bPtmux;\x1b\x1b]1337;X\x07\x1b\\";

    #[test]
    fn wrapping_follows_the_scripts() {
        assert_eq!(seq(Some("xterm-256color"), "1337;X"), PLAIN);
        for term in ["screen.xterm-256color", "screen", "tmux-256color"] {
            assert_eq!(seq(Some(term), "1337;X"), WRAPPED, "{term}");
        }
        for term in [None, Some("Screen"), Some("tmu"), Some("")] {
            assert_eq!(seq(term, "1337;X"), PLAIN, "{term:?}");
        }
        assert_eq!(osc(Some("tmux")), TMUX_OSC);
        assert_eq!(st(Some("tmux")), TMUX_ST);
    }

    #[test]
    fn base64_and_fold_follow_the_scripts() {
        assert_eq!(b64(b"hi"), "aGk=");
        assert_eq!(b64_line(b"hello\n"), "aGVsbG8K\n");
        let long = b64(&[b'x'; 250]);
        assert_eq!(long.len(), 336);
        assert!(!long.contains('\n'));
        assert_eq!(unb64("aGk=\n"), b"hi");
        assert_eq!(unb64("a G k"), b"hi");
        let lens: Vec<usize> = fold(&"a".repeat(450), 200).iter().map(String::len).collect();
        assert_eq!(lens, [200, 200, 50]);
        assert!(fold("", 200).is_empty());
        assert_eq!(fold("ab\ncd\n", 80), ["ab", "cd"]);
    }

    #[test]
    fn wrapped_sequences_reach_termurion() {
        for term in [None, Some("tmux-256color")] {
            let mut s = Screen::new(80, 24);
            s.feed(seq(term, "1337;RequestAttention=once").as_bytes());
            assert_eq!(s.take_requests(), vec![Request::Attention(Attention::Once)], "{term:?}");
            assert_eq!(s.line_text(0).trim(), "", "{term:?}: nothing visible");
        }
    }
}
