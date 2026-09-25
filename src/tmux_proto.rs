//! tmux control mode's lines (iTerm2's TmuxGateway parsers): `%output` octal decoding and
//! every notification, with iTerm2's texts for a malformed one.

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Note {
    Begin { time: String, number: String, flags: i64 },
    End { time: String, number: String },
    Error { time: String, number: String },
    Output { pane: u32, data: Vec<u8> },
    ExtendedOutput { pane: u32, latency_ms: i64, data: Vec<u8> },
    LayoutChange { window: u32, layout: String, visible: Option<String>, zoomed: Option<bool> },
    WindowAdd(u32),
    WindowClose(u32),
    WindowRenamed { window: u32, name: String },
    UnlinkedWindowAdd,
    SessionChanged { session: u32, name: String },
    SessionRenamed { session: u32, name: String },
    SessionsChanged,
    WindowPaneChanged { window: u32, pane: u32 },
    SessionWindowChanged { session: u32, window: u32 },
    PasteBufferChanged(String),
    Pause(u32),
    Continue,
    SubscriptionChanged { name: String, args: Vec<String>, value: String },
    ClientSessionChanged(String),
    ClientDetached(String),
    Exit(Option<String>),
    /// Another `%` line (logged only).
    Other(String),
    /// A line that is not a tmux line at all.
    Text(String),
}

/// `%output` data: bytes under 0x20 are dropped (tmux adds `\r`), `\` and three octal
/// digits is one byte (carriage returns between the digits skipped), a bad escape is `?`.
pub fn decode_output(bytes: &[u8]) -> Vec<u8> {
    // iTerm2's loop over a NUL-terminated string: the end reads as 0.
    let at = |i: usize| bytes.get(i).copied().unwrap_or(0);
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while at(i) != 0 {
        let mut c = at(i);
        if c < b' ' {
            i += 1;
            continue;
        }
        if c == b'\\' {
            c = 0;
            for _ in 0..3 {
                i += 1;
                while at(i) == b'\r' {
                    i += 1;
                }
                let d = at(i);
                if !(b'0'..=b'7').contains(&d) {
                    c = b'?';
                    i -= 1;
                    break;
                }
                c = c.wrapping_mul(8).wrapping_add(d - b'0');
            }
        }
        out.push(c);
        i += 1;
    }
    out
}

fn num(s: &str) -> Option<u32> {
    (!s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())).then(|| s.parse().ok()).flatten()
}

fn malformed(expected: &str, line: &str) -> String {
    format!("Malformed command (expected {expected}): \"{line}\"")
}

/// `%output %<pane> <data>` and `%extended-output %<pane> <latency> [args] : <data>`, on raw
/// bytes (the data may not be UTF-8).
fn parse_output(line: &[u8], extended: bool) -> Result<Note, String> {
    let text = String::from_utf8_lossy(line).into_owned();
    let bad = || malformed("%num data", &text);
    let sp = line.iter().position(|b| *b == b' ').ok_or_else(bad)?;
    let rest = &line[sp + 1..];
    if rest.first() != Some(&b'%') {
        return Err(bad());
    }
    let sp2 = rest.iter().position(|b| *b == b' ').ok_or_else(bad)?;
    let pane = num(std::str::from_utf8(&rest[1..sp2]).map_err(|_| bad())?).ok_or_else(bad)?;
    let payload = &rest[sp2 + 1..];
    if !extended {
        return Ok(Note::Output { pane, data: decode_output(payload) });
    }
    let sp3 = payload.iter().position(|b| *b == b' ').ok_or_else(bad)?;
    let latency_ms = std::str::from_utf8(&payload[..sp3]).ok().and_then(|s| s.parse::<i64>().ok()).ok_or_else(bad)?;
    let after = &payload[sp3 + 1..];
    let colon = after.iter().position(|b| *b == b':').ok_or_else(bad)?;
    if after.get(colon + 1) != Some(&b' ') {
        return Err(bad());
    }
    Ok(Note::ExtendedOutput { pane, latency_ms, data: decode_output(&after[colon + 2..]) })
}

/// Parses one control-mode line.
pub fn parse(line: &[u8]) -> Result<Note, String> {
    if line.starts_with(b"%output ") {
        return parse_output(line, false);
    }
    if line.starts_with(b"%extended-output ") {
        return parse_output(line, true);
    }
    let text = String::from_utf8_lossy(line).into_owned();
    let t = text.as_str();
    let words: Vec<&str> = t.split(' ').collect();
    let id = |w: &str, sigil: char| w.strip_prefix(sigil).and_then(num);
    let after = |n: usize| words.get(n..).map(|w| w.join(" ")).unwrap_or_default();
    let head = words[0];
    Ok(match head {
        "%begin" | "%end" | "%error" if words.len() >= 3 => {
            let (time, number) = (words[1].to_string(), words[2].to_string());
            match head {
                "%begin" => Note::Begin { time, number, flags: words.get(3).and_then(|f| f.parse().ok()).unwrap_or(-1) },
                "%end" => Note::End { time, number },
                _ => Note::Error { time, number },
            }
        }
        "%layout-change" => {
            let window = words.get(1).and_then(|w| id(w, '@')).filter(|_| words.len() >= 3).ok_or_else(|| malformed("%layout-change <window> <layout> [<window_visible_layout> <window_flags>]", t))?;
            let rest: Vec<&str> = words[2..].to_vec();
            Note::LayoutChange { window, layout: rest[0].to_string(), visible: rest.get(1).map(|s| s.to_string()), zoomed: rest.get(2).map(|f| f.contains('Z')) }
        }
        "%window-add" => Note::WindowAdd((words.len() == 2).then(|| id(words[1], '@')).flatten().ok_or_else(|| malformed("%window-add id", t))?),
        "%window-close" | "%unlinked-window-close" => Note::WindowClose((words.len() == 2).then(|| id(words[1], '@')).flatten().ok_or_else(|| malformed("%window-close id", t))?),
        "%window-renamed" | "%unlinked-window-renamed" => {
            let window = words.get(1).and_then(|w| id(w, '@')).filter(|_| words.len() >= 3).ok_or_else(|| malformed("%window-renamed id new_name", t))?;
            Note::WindowRenamed { window, name: after(2).replace("\\\\", "\\") }
        }
        "%unlinked-window-add" => Note::UnlinkedWindowAdd,
        "%session-changed" | "%session-renamed" => {
            let (what, ctor): (&str, fn(u32, String) -> Note) = if head == "%session-changed" {
                ("%session-changed id name", |session, name| Note::SessionChanged { session, name })
            } else {
                ("%session-renamed id name", |session, name| Note::SessionRenamed { session, name })
            };
            let session = words.get(1).and_then(|w| id(w, '$')).filter(|_| words.len() >= 3 && !after(2).is_empty()).ok_or_else(|| malformed(what, t))?;
            ctor(session, after(2))
        }
        "%sessions-changed" if words.len() == 1 => Note::SessionsChanged,
        "%sessions-changed" => return Err(malformed("%sessions-changed", t)),
        "%window-pane-changed" => match (words.len(), words.get(1).and_then(|w| id(w, '@')), words.get(2).and_then(|w| id(w, '%'))) {
            (3, Some(window), Some(pane)) => Note::WindowPaneChanged { window, pane },
            _ => return Err(malformed("%window-pane-changed @window-id %pane-id", t)),
        },
        "%session-window-changed" => match (words.len(), words.get(1).and_then(|w| id(w, '$')), words.get(2).and_then(|w| id(w, '@'))) {
            (3, Some(session), Some(window)) => Note::SessionWindowChanged { session, window },
            _ => return Err(malformed("%session-window-changed $session-id @window-id", t)),
        },
        // Only bufferN names: the name is written back to tmux to fetch the buffer.
        "%paste-buffer-changed" => match words.get(1).filter(|w| w.strip_prefix("buffer").and_then(num).is_some()) {
            Some(b) => Note::PasteBufferChanged(b.to_string()),
            None => Note::Other(text.clone()),
        },
        "%pause" => Note::Pause((words.len() == 2).then(|| id(words[1], '%')).flatten().ok_or_else(|| malformed("%pause %wp", t))?),
        "%continue" => Note::Continue,
        "%subscription-changed" => {
            let body = t.strip_prefix("%subscription-changed ").unwrap_or("");
            let (args, value) = body.split_once(" : ").filter(|(a, _)| !a.is_empty() && !a.contains(':')).ok_or_else(|| malformed("%subscription-changed sid [...] : value", t))?;
            let args: Vec<String> = args.split(' ').map(str::to_string).collect();
            Note::SubscriptionChanged { name: args[0].clone(), args, value: value.to_string() }
        }
        "%client-session-changed" if words.len() >= 3 && words[2].starts_with('$') => Note::ClientSessionChanged(words[1].to_string()),
        "%client-detached" if words.len() == 2 => Note::ClientDetached(words[1].to_string()),
        "%exit" => Note::Exit((words.len() > 1).then(|| after(1))),
        _ if head.starts_with('%') => Note::Other(text.clone()),
        _ => Note::Text(text.clone()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn octal_decoding_follows_iterm2() {
        assert_eq!(decode_output(b"a\\015\\012b"), b"a\r\nb");
        assert_eq!(decode_output(b"\\1\r01"), b"A", "carriage returns inside an escape are skipped");
        assert_eq!(decode_output(b"x\r\ny"), b"xy", "control bytes are dropped");
        assert_eq!(decode_output(b"\\9z"), b"?9z", "a bad escape is a question mark");
        assert_eq!(decode_output(b"\\\\"), b"??", "each backslash starts a bad escape (iTerm2's loop)");
        assert_eq!(decode_output(b"\xff\xfe"), vec![0xff, 0xfe], "not UTF-8 is kept");
        assert_eq!(decode_output(b"\\3"), b"?");
    }

    #[test]
    fn notifications_parse() {
        let p = |s: &[u8]| parse(s).unwrap();
        assert_eq!(p(b"%output %12 hi\\033[0m"), Note::Output { pane: 12, data: b"hi\x1b[0m".to_vec() });
        assert_eq!(p(b"%extended-output %3 250 : ab"), Note::ExtendedOutput { pane: 3, latency_ms: 250, data: b"ab".to_vec() });
        assert_eq!(p(b"%layout-change @4 b25d,80x24,0,0,1 b25d,80x24,0,0,1 *Z"), Note::LayoutChange { window: 4, layout: "b25d,80x24,0,0,1".into(), visible: Some("b25d,80x24,0,0,1".into()), zoomed: Some(true) });
        assert_eq!(p(b"%layout-change @4 b25d,80x24,0,0,1"), Note::LayoutChange { window: 4, layout: "b25d,80x24,0,0,1".into(), visible: None, zoomed: None });
        assert_eq!(p(b"%window-add @7"), Note::WindowAdd(7));
        assert_eq!(p(b"%unlinked-window-close @7"), Note::WindowClose(7));
        assert_eq!(p(b"%window-renamed @7 a\\\\b c"), Note::WindowRenamed { window: 7, name: "a\\b c".into() });
        assert_eq!(p(b"%session-changed $1 my session"), Note::SessionChanged { session: 1, name: "my session".into() });
        assert_eq!(p(b"%sessions-changed"), Note::SessionsChanged);
        assert_eq!(p(b"%window-pane-changed @1 %5"), Note::WindowPaneChanged { window: 1, pane: 5 });
        assert_eq!(p(b"%session-window-changed $0 @2"), Note::SessionWindowChanged { session: 0, window: 2 });
        assert_eq!(p(b"%paste-buffer-changed buffer3"), Note::PasteBufferChanged("buffer3".into()));
        assert_eq!(p(b"%paste-buffer-changed evil;rm"), Note::Other("%paste-buffer-changed evil;rm".into()), "only bufferN names");
        assert_eq!(p(b"%pause %9"), Note::Pause(9));
        assert_eq!(p(b"%subscription-changed title $1 @2 0 %3 : hello : world"), Note::SubscriptionChanged { name: "title".into(), args: vec!["title".into(), "$1".into(), "@2".into(), "0".into(), "%3".into()], value: "hello : world".into() });
        assert_eq!(p(b"%begin 1790 273 0"), Note::Begin { time: "1790".into(), number: "273".into(), flags: 0 });
        assert_eq!(p(b"%begin 1790 273"), Note::Begin { time: "1790".into(), number: "273".into(), flags: -1 }, "tmux 1.8 has no flags");
        assert_eq!(p(b"%end 1790 273 1"), Note::End { time: "1790".into(), number: "273".into() });
        assert_eq!(p(b"%exit"), Note::Exit(None));
        assert_eq!(p(b"%exit server exited"), Note::Exit(Some("server exited".into())));
        assert_eq!(p(b"%continue %2"), Note::Continue);
        assert_eq!(p(b"%client-detached /dev/pts/3"), Note::ClientDetached("/dev/pts/3".into()));
        assert_eq!(p(b"%newthing x"), Note::Other("%newthing x".into()));
        assert_eq!(p(b"bash: tmux: command not found"), Note::Text("bash: tmux: command not found".into()));
    }

    #[test]
    fn malformed_lines_say_why() {
        assert_eq!(parse(b"%window-add x"), Err("Malformed command (expected %window-add id): \"%window-add x\"".into()));
        assert_eq!(parse(b"%output 12 hi"), Err("Malformed command (expected %num data): \"%output 12 hi\"".into()));
        assert_eq!(parse(b"%layout-change 4 x"), Err("Malformed command (expected %layout-change <window> <layout> [<window_visible_layout> <window_flags>]): \"%layout-change 4 x\"".into()));
        assert_eq!(parse(b"%session-changed 1 x"), Err("Malformed command (expected %session-changed id name): \"%session-changed 1 x\"".into()));
        assert_eq!(parse(b"%pause 9"), Err("Malformed command (expected %pause %wp): \"%pause 9\"".into()));
    }
}
