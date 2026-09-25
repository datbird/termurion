//! iTerm2's clipboard sequences: OSC 52 (write and read), OSC 1337 `Copy=` (one piece or
//! segments), and the legacy `CopyToClipboard` ... `EndCopy` capture, with iTerm2's policy.

use base64::Engine;

/// An OSC 52 request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Osc52 {
    /// Put this text on the clipboard.
    Write(String),
    /// Report the clipboard; the selection parameter (`Pc`) is echoed in the answer.
    Read(String),
}

/// Reads an OSC 52 payload (after `52;`): `Pc ; base64` or `Pc ; ?`.
pub fn osc52(payload: &[u8]) -> Option<Osc52> {
    let semi = payload.iter().position(|b| *b == b';')?;
    let (pc, data) = (&payload[..semi], &payload[semi + 1..]);
    if !pc.iter().all(|b| b"psc01234567".contains(b)) {
        return None;
    }
    let pc = String::from_utf8_lossy(pc).into_owned();
    if data == b"?" {
        return Some(Osc52::Read(pc));
    }
    Some(Osc52::Write(clean(&decode(data)?)))
}

/// Clipboard text from decoded bytes, as iTerm2 cleans it: cut at the first NUL, and C0
/// controls other than tab, line feed, and carriage return removed.
pub fn clean(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len());
    let kept: Vec<u8> = bytes[..end].iter().copied().filter(|b| *b >= 0x20 || matches!(b, b'\t' | b'\n' | b'\r')).collect();
    String::from_utf8_lossy(&kept).into_owned()
}

/// Base64 that may be cut short (a capped payload) or padded in pieces (segments).
fn decode(text: &[u8]) -> Option<Vec<u8>> {
    use base64::engine::general_purpose::STANDARD;
    let clean: Vec<u8> = text.iter().copied().filter(|b| !b.is_ascii_whitespace()).collect();
    if let Ok(v) = STANDARD.decode(&clean) {
        return Some(v);
    }
    // Pieces padded one by one: decode each run that ends in padding.
    let mut out = Vec::new();
    let mut rest: &[u8] = &clean;
    while !rest.is_empty() {
        let end = rest.iter().position(|b| *b == b'=').map_or(rest.len(), |p| {
            let mut e = p;
            while e < rest.len() && rest[e] == b'=' {
                e += 1;
            }
            e
        });
        let piece = &rest[..end];
        // A payload cut at a cap ends mid-group: drop the partial group.
        let whole = if piece.ends_with(b"=") { piece } else { &piece[..piece.len() / 4 * 4] };
        out.extend(STANDARD.decode(whole).ok()?);
        rest = &rest[end..];
    }
    Some(out)
}

/// Who may read the clipboard: iTerm2's three read modes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadPolicy {
    Never,
    Always,
    Ask,
}

/// The read mode from the saved choices (`NoSyncNeverAllowPaste`, `NoSyncAllowPaste`).
pub fn read_policy(never: bool, always: bool) -> ReadPolicy {
    if never {
        ReadPolicy::Never
    } else if always {
        ReadPolicy::Always
    } else {
        ReadPolicy::Ask
    }
}

/// The answer to a read: `ESC ] 52 ; Pc ; base64 ESC \`.
pub fn read_reply(pc: &str, text: &str) -> Vec<u8> {
    let data = base64::engine::general_purpose::STANDARD.encode(text);
    format!("\x1b]52;{pc};{data}\x1b\\").into_bytes()
}

/// Whether DA1 claims clipboard support (`52`): access is on, or the setting was never
/// chosen and the denied warning is not turned off (iTerm2 then asks).
pub fn advertise_52(allowed: bool, chosen: bool, warning_off: bool) -> bool {
    allowed || (!chosen && !warning_off)
}

/// OSC 1337 `Copy=`, one piece or in segments.
#[derive(Default)]
pub struct Copy {
    segments: Option<(String, Vec<u8>)>,
}

impl Copy {
    /// Takes one `Copy=` value (`[mode[;uid]]:base64`). Returns the text when a copy is
    /// complete. `cap` bounds a segmented copy's bytes.
    pub fn take(&mut self, value: &str, cap: usize) -> Option<String> {
        let (head, body) = value.split_once(':').unwrap_or((value, ""));
        let (mode, uid) = head.split_once(';').unwrap_or((head, ""));
        match mode {
            "" | "1" => {
                self.segments = None;
                Some(clean(&decode(body.as_bytes())?))
            }
            "2" => {
                self.segments = Some((uid.to_string(), body.as_bytes().to_vec()));
                None
            }
            "3" => {
                let (id, buf) = self.segments.as_mut()?;
                if id != uid || buf.len() + body.len() > cap.saturating_mul(4) / 3 + 4 {
                    self.segments = None;
                    return None;
                }
                buf.extend_from_slice(body.as_bytes());
                None
            }
            "4" => {
                let (id, mut buf) = self.segments.take()?;
                if !uid.is_empty() && id != uid {
                    return None;
                }
                buf.extend_from_slice(body.as_bytes());
                let bytes = decode(&buf)?;
                (bytes.len() <= cap).then(|| clean(&bytes))
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screen::{Request, Screen};

    fn b64(s: &str) -> String {
        base64::engine::general_purpose::STANDARD.encode(s)
    }

    #[test]
    fn osc52_writes_when_allowed() {
        assert_eq!(osc52(format!("c;{}", b64("hello")).as_bytes()), Some(Osc52::Write("hello".into())));
        assert_eq!(osc52(format!(";{}", b64("x")).as_bytes()), Some(Osc52::Write("x".into())), "empty Pc");
        assert_eq!(osc52(b"s0;?"), Some(Osc52::Read("s0".into())));
        assert_eq!(osc52(b"z;aGk="), None, "bad Pc");
        let mut s = Screen::new(20, 4);
        s.feed(format!("\x1b]52;c;{}\x07", b64("from vim")).as_bytes());
        assert_eq!(s.take_requests(), vec![Request::ClipboardWrite("from vim".into())]);
        s.feed(b"\x1b]52;p;?\x1b\\");
        assert_eq!(s.take_requests(), vec![Request::ClipboardRead("p".into())]);
    }

    #[test]
    fn osc52_strips_controls() {
        assert_eq!(clean(b"a\tb\r\nc\x1b[31md\x07e"), "a\tb\r\nc[31mde");
        assert_eq!(clean(b"keep\0gone"), "keep");
        assert_eq!(osc52(format!("c;{}", b64("x\x1by")).as_bytes()), Some(Osc52::Write("xy".into())));
    }

    #[test]
    fn osc52_caps_the_payload() {
        let mut s = Screen::new(20, 4);
        let exact = "a".repeat(3 * 1024);
        s.feed(format!("\x1b]52;c;{}\x07", b64(&exact)).as_bytes());
        assert_eq!(s.take_requests(), vec![Request::ClipboardWrite(exact)]);
        // Past 1 MiB of payload the text is cut at the cap.
        let big = "b".repeat(1 << 20);
        s.feed(format!("\x1b]52;c;{}\x07", b64(&big)).as_bytes());
        match s.take_requests().as_slice() {
            [Request::ClipboardWrite(t)] => {
                assert!(t.len() < big.len() && t.len() > (1 << 20) * 7 / 10, "cut near 1 MiB of base64: {}", t.len());
                assert!(t.bytes().all(|b| b == b'b'));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn osc52_read_follows_policy() {
        assert_eq!(read_policy(false, false), ReadPolicy::Ask);
        assert_eq!(read_policy(true, false), ReadPolicy::Never);
        assert_eq!(read_policy(false, true), ReadPolicy::Always);
        assert_eq!(read_policy(true, true), ReadPolicy::Never, "never wins");
        assert_eq!(read_reply("c", "hi"), b"\x1b]52;c;aGk=\x1b\\");
    }

    #[test]
    fn copy_monolithic_and_segmented() {
        let mut c = Copy::default();
        assert_eq!(c.take(&format!(":{}", b64("one")), 1 << 20), Some("one".into()));
        assert_eq!(c.take(&format!("1:{}", b64("two")), 1 << 20), Some("two".into()));
        assert_eq!(c.take("2;abc:", 1 << 20), None);
        assert_eq!(c.take(&format!("3;abc:{}", b64("par")), 1 << 20), None);
        assert_eq!(c.take(&format!("3;abc:{}", b64("ts")), 1 << 20), None);
        assert_eq!(c.take("4;abc:", 1 << 20), Some("parts".into()));
        // A segment for another copy aborts it.
        c.take("2;abc:", 1 << 20);
        assert_eq!(c.take(&format!("3;zzz:{}", b64("x")), 1 << 20), None);
        assert_eq!(c.take("4;abc:", 1 << 20), None, "aborted");
        // The cap stops a segmented copy from growing without end.
        c.take("2;q:", 8);
        c.take(&format!("3;q:{}", b64("0123456789")), 8);
        assert_eq!(c.take("4;q:", 8), None, "over the cap");
        // Through the screen.
        let mut s = Screen::new(20, 4);
        s.feed(format!("\x1b]1337;Copy=:{}\x07", b64("osc")).as_bytes());
        assert_eq!(s.take_requests(), vec![Request::ClipboardWrite("osc".into())]);
    }

    #[test]
    fn copy_to_clipboard_captures_raw_bytes() {
        let mut s = Screen::new(20, 4);
        s.clipboard_allowed = true;
        s.feed(b"\x1b]1337;CopyToClipboard=\x07hello ");
        s.feed(b"world\x1b]1337;EndCopy\x07after");
        assert_eq!(s.take_requests(), vec![Request::ClipboardWrite("hello world".into())]);
        let row: String = (0..20).map(|c| s.term().grid()[alacritty_terminal::index::Line(0)][alacritty_terminal::index::Column(c)].c).collect();
        assert_eq!(row.trim_end(), "hello worldafter", "the copied text still draws");
    }

    #[test]
    fn copy_to_clipboard_caps_its_capture() {
        let mut s = Screen::new(20, 4);
        s.clipboard_allowed = true;
        s.limits.copy = 10;
        s.feed(b"\x1b]1337;CopyToClipboard=\x07");
        for _ in 0..100 {
            s.feed(b"0123456789");
        }
        s.feed(b"\x1b]1337;EndCopy\x07");
        assert_eq!(s.take_requests(), vec![Request::ClipboardWrite("0123456789".into())]);
    }

    #[test]
    fn untrusted_sequences_are_refused() {
        let mut s = Screen::new(20, 4);
        s.trusted = false;
        s.feed(format!("\x1b]1337;Copy=:{}\x07", b64("no")).as_bytes());
        s.feed(b"\x1b]1337;CopyToClipboard=\x07no\x1b]1337;EndCopy\x07");
        assert_eq!(s.take_requests(), vec![]);
        s.feed(format!("\x1b]52;c;{}\x07", b64("yes")).as_bytes());
        assert_eq!(s.take_requests(), vec![Request::ClipboardWrite("yes".into())], "OSC 52 is not trust-gated");
    }

    #[test]
    fn da1_advertises_52_by_policy() {
        assert!(advertise_52(true, true, true));
        assert!(advertise_52(false, false, false), "never chosen: claims support, then asks");
        assert!(!advertise_52(false, true, false));
        assert!(!advertise_52(false, false, true));
        let mut s = Screen::new(20, 4);
        assert!(String::from_utf8(s.feed(b"\x1b[c")).unwrap().ends_with(";52c"));
        s.set_clipboard_advertised(false);
        assert_eq!(String::from_utf8(s.feed(b"\x1b[c")).unwrap(), "\x1b[?64;1;2;4;6;17;18;21;22c");
    }
    #[test]
    fn a_split_end_copy_is_not_captured() {
        let mut s = Screen::new(40, 4);
        s.clipboard_allowed = true;
        s.feed(b"\x1b]1337;CopyToClipboard=\x07text\x1b]1337;EndC");
        s.feed(b"opy\x07");
        assert_eq!(s.take_requests(), vec![Request::ClipboardWrite("text".into())]);
        // A sequence Tandem does not act on, split across reads, is part of the text.
        s.feed(b"\x1b]1337;CopyToClipboard=\x07a\x1b[3");
        s.feed(b"1mb\x1b]1337;End");
        s.feed(b"C");
        s.feed(b"opy\x07");
        assert_eq!(s.take_requests(), vec![Request::ClipboardWrite("a\x1b[31mb".into())]);
    }

    #[test]
    fn no_capture_without_clipboard_access() {
        let mut s = Screen::new(20, 4);
        s.clipboard_allowed = false;
        s.feed(b"\x1b]1337;CopyToClipboard=\x07lots of output");
        assert!(s.capture.is_none(), "nothing kept when the result would be thrown away");
        s.feed(b"\x1b]1337;EndCopy\x07");
        assert_eq!(s.take_requests(), vec![]);
    }
}
