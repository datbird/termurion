//! Typing into a tmux pane (iTerm2's TmuxGateway sendKeys and iTermTmuxControlModeKeyName):
//! text becomes send-keys commands in three classes (literal characters, hex keys, raw
//! bytes), grouped in runs under tmux 1.8's 1024-byte line limit; modified keys on tmux 3.2
//! and later can go by name so tmux encodes them for the pane.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Class {
    Literal,
    Hex,
    Byte,
}

/// Commands longer than 1024 bytes crashed tmux 1.8.
const MAX_LITERAL: usize = 1000;
const MAX_HEX: usize = MAX_LITERAL / 8;
const MAX_BYTE: usize = MAX_LITERAL / 3;

fn class(c: u32, raw_controls: bool) -> Class {
    let literal = matches!(c, 0x2b | 0x2f | 0x29 | 0x3a | 0x2c | 0x5f) || (c < 0x80 && (c as u8).is_ascii_alphanumeric());
    if literal {
        Class::Literal
    } else if raw_controls && c <= 0x1f {
        Class::Byte
    } else {
        Class::Hex
    }
}

/// The send-keys commands for `text` to pane `%pane`, by the server's version bracket.
/// Err: tmux exactly 2.2 cannot take a character outside the Basic Multilingual Plane.
pub fn encode(text: &[u8], pane: u32, min: Option<f64>, max: Option<f64>) -> Result<Vec<String>, char> {
    let utf8 = min.is_some_and(|v| v >= 2.2);
    let points: Vec<u32> = if utf8 {
        String::from_utf8_lossy(text).chars().map(|c| c as u32).collect()
    } else {
        // tmux 2.1 and earlier: each UTF-8 byte is its own key.
        text.iter().map(|b| *b as u32).collect()
    };
    if min == Some(2.2) && max == Some(2.2) {
        if let Some(c) = points.iter().find(|c| **c > 0xffff).and_then(|c| char::from_u32(*c)) {
            return Err(c);
        }
    }
    // tmux 3.0a is the first with send-keys -H.
    let raw_controls = min.is_some_and(|v| v >= 3.01);
    let mut out = Vec::new();
    let mut run: Vec<u32> = Vec::new();
    let mut kind: Option<Class> = None;
    let flush = |run: &mut Vec<u32>, k: Class, out: &mut Vec<String>| {
        if run.is_empty() {
            return;
        }
        let cmd = match k {
            Class::Literal => format!("send -lt %{pane} {}", run.iter().filter_map(|c| char::from_u32(*c)).collect::<String>()),
            Class::Hex => format!("send -t %{pane} {}", run.iter().map(|c| if *c == 0 { "C-Space".to_string() } else { format!("0x{c:x}") }).collect::<Vec<_>>().join(" ")),
            Class::Byte => format!("send -H -t %{pane} {}", run.iter().map(|c| format!("{:02x}", c & 0xff)).collect::<Vec<_>>().join(" ")),
        };
        out.push(cmd);
        run.clear();
    };
    for c in points {
        let k = class(c, raw_controls);
        if kind.is_some_and(|old| old != k) {
            flush(&mut run, kind.unwrap(), &mut out);
        }
        kind = Some(k);
        run.push(c);
        let max = match k {
            Class::Literal => MAX_LITERAL,
            Class::Hex => MAX_HEX,
            Class::Byte => MAX_BYTE,
        };
        if run.len() == max {
            flush(&mut run, k, &mut out);
        }
    }
    if let Some(k) = kind {
        flush(&mut run, k, &mut out);
    }
    Ok(out)
}

/// A modified key's tmux name (iTerm2's iTermTmuxControlModeOtherKeyName), when the
/// modifyOtherKeys byte path would encode it; None keeps the bytes. `c` is the key's base
/// character (Enter is '\r', Backspace 0x7f); `alt_meta`: Option (Alt) acts as Meta.
pub fn key_name(c: char, ctrl: bool, alt: bool, shift: bool, alt_meta: bool, keypad: bool) -> Option<String> {
    if keypad || !(ctrl || alt || shift) {
        return None;
    }
    let base = match c {
        '\r' => "Enter".to_string(),
        '\t' => "Tab".into(),
        ' ' => "Space".into(),
        '\u{1b}' => "Escape".into(),
        '\u{7f}' => "BSpace".into(),
        c if (c as u32) < 0x20 => return None,
        c => c.to_string(),
    };
    let command_key = matches!(c, '\r' | '\t' | '\u{1b}' | '\u{7f}');
    let shift_space = c == ' ' && shift && !ctrl && !alt;
    if !ctrl && !(alt && alt_meta) && !command_key && !shift_space {
        return None;
    }
    let mut name = String::new();
    if ctrl {
        name.push_str("C-");
    }
    if alt {
        name.push_str("M-");
    }
    if shift {
        name.push_str("S-");
    }
    name.push_str(&base);
    Some(name)
}

/// `send -t %P 'NAME'`, double quotes when the name holds a single quote.
pub fn key_name_command(pane: u32, name: &str) -> String {
    if name.contains('\'') { format!("send -t %{pane} \"{name}\"") } else { format!("send -t %{pane} '{name}'") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn send_keys_follow_iterm2() {
        let text = "ab;\t\n\u{e9}".as_bytes();
        assert_eq!(encode(text, 1, Some(3.4), None).unwrap(), ["send -lt %1 ab", "send -t %1 0x3b", "send -H -t %1 09 0a", "send -t %1 0xe9"]);
        assert_eq!(encode(text, 1, Some(2.9), None).unwrap(), ["send -lt %1 ab", "send -t %1 0x3b 0x9 0xa 0xe9"], "no -H before 3.0a");
        assert_eq!(encode(text, 1, Some(3.0), None).unwrap()[1], "send -t %1 0x3b 0x9 0xa 0xe9", "3.0 lacks -H too");
        assert_eq!(encode("\u{e9}".as_bytes(), 1, Some(2.1), None).unwrap(), ["send -t %1 0xc3 0xa9"], "2.1: UTF-8 bytes as keys");
        assert_eq!(encode(b"\0", 1, Some(2.9), None).unwrap(), ["send -t %1 C-Space"]);
        assert_eq!(encode(b"+/):,_x", 1, Some(3.4), None).unwrap(), ["send -lt %1 +/):,_x"]);
        assert_eq!(encode("\u{1f600}".as_bytes(), 1, Some(2.2), Some(2.2)), Err('\u{1f600}'));
        assert_eq!(encode("\u{1f600}".as_bytes(), 1, Some(3.4), None).unwrap(), ["send -t %1 0x1f600"]);
        assert!(encode(b"", 1, Some(3.4), None).unwrap().is_empty());
    }

    #[test]
    fn long_input_splits_under_1024() {
        let lits = encode(&[b'a'; 2500], 3, Some(3.4), None).unwrap();
        assert_eq!(lits.iter().map(|c| c.len() - "send -lt %3 ".len()).collect::<Vec<_>>(), [1000, 1000, 500]);
        let hex = encode(&[b';'; 200], 3, Some(3.4), None).unwrap();
        assert_eq!(hex.len(), 2);
        let bytes = encode(&[1u8; 400], 3, Some(3.4), None).unwrap();
        assert_eq!(bytes.len(), 2);
        for c in lits.iter().chain(&hex).chain(&bytes) {
            assert!(c.len() < 1024, "{}", c.len());
        }
    }

    #[test]
    fn key_names_follow_iterm2() {
        assert_eq!(key_name('x', true, false, false, false, false).as_deref(), Some("C-x"));
        assert_eq!(key_name('x', true, true, true, false, false).as_deref(), Some("C-M-S-x"));
        assert_eq!(key_name('\r', false, false, true, false, false).as_deref(), Some("S-Enter"));
        assert_eq!(key_name('\u{7f}', false, true, false, false, false).as_deref(), Some("M-BSpace"), "a command key");
        assert_eq!(key_name(' ', false, false, true, false, false).as_deref(), Some("S-Space"));
        assert_eq!(key_name(' ', false, true, true, false, false), None, "Shift+Space only alone");
        assert_eq!(key_name('a', false, false, false, false, false), None, "unmodified keys keep the bytes");
        assert_eq!(key_name('a', false, true, false, false, false), None, "Alt that is not Meta types text");
        assert_eq!(key_name('a', false, true, false, true, false).as_deref(), Some("M-a"));
        assert_eq!(key_name('5', true, false, false, false, true), None, "keypad keys keep their encoding");
        assert_eq!(key_name('\u{1}', true, false, false, false, false), None, "a bare control code");
        assert_eq!(key_name_command(4, "C-x"), "send -t %4 'C-x'");
        assert_eq!(key_name_command(4, "C-'"), "send -t %4 \"C-'\"");
    }
}
