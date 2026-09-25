//! Finds the OSC sequences alacritty does not handle (133 prompt marks, 7 location,
//! 1337 key=value) in the pty output, and where each one ends, so the screen can act
//! on it at the exact cursor position. The bytes still go to alacritty unchanged.

/// The longest payload kept. Longer ones (an inline image, say) are not ours and are dropped.
const LIMIT: usize = 64 * 1024;

/// OSC 1337 files (inline images, downloads) may be this large.
const FILE_LIMIT: usize = 512 << 20;

/// iTerm2 caps any other OSC at 1 MiB, cutting it there (OSC 52 keeps what fits).
const OSC_CAP: usize = 1 << 20;

fn limit_for(buf: &[u8]) -> usize {
    if [b"1337;File=".as_slice(), b"1337;MultipartFile=", b"1337;FilePart=", b"1337;Copy="].iter().any(|p| buf.starts_with(p)) {
        FILE_LIMIT
    } else if buf.starts_with(b"52;") {
        OSC_CAP
    } else {
        LIMIT
    }
}

/// Payloads cut at their cap instead of dropped.
fn cut_at_cap(buf: &[u8]) -> bool {
    buf.starts_with(b"52;")
}

/// OSC payload prefixes Tandem acts on.
const WANTED: [&[u8]; 4] = [b"133;", b"7;", b"1337;", b"52;"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromptKind {
    Initial,
    Secondary,
    Continuation,
    Right,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Osc {
    /// 133;A, N (fresh line), or P (no fresh line).
    Prompt { kind: PromptKind, aid: Option<String>, fresh_line: bool },
    /// 133;B: the prompt ended; the command starts at the cursor.
    CommandStart,
    /// 133;C: the command was entered; output starts.
    OutputStart,
    /// 133;D: the command finished.
    CommandEnd { code: Option<i32>, aid: Option<String> },
    /// OSC 7: a file:// URL with the user, host, and directory.
    Location(String),
    /// OSC 1337 key=value.
    Kv(String, String),
    /// A sixel image: the DCS body after `q`.
    Sixel(Vec<u8>),
    /// A kitty graphics command: the APC body after `G`.
    Kitty(Vec<u8>),
    /// OSC 52: the payload after `52;`.
    Clipboard(Vec<u8>),
    /// A report request alacritty does not answer.
    Query(crate::reports::Query),
    /// `DCS = 1 s` (true) or `DCS = 2 s`: synchronized output begins or ends.
    Sync(bool),
}

#[derive(Default)]
enum State {
    #[default]
    Ground,
    Esc,
    Osc { buf: Vec<u8>, keep: bool, esc: bool, cut: bool },
    /// `ESC P`: a DCS string (sixel images are kept).
    Dcs { buf: Vec<u8>, esc: bool },
    /// `ESC _`: an APC string (kitty graphics commands are kept).
    Apc { buf: Vec<u8>, esc: bool },
    /// `ESC [`: a control sequence (a few queries are caught). `long` marks one too long to
    /// be a query.
    Csi { buf: Vec<u8>, long: bool },
}

/// A control sequence longer than this is no query Tandem answers.
const CSI_LIMIT: usize = 32;

/// The query a control sequence asks, from its parameter bytes and final byte.
fn csi_query(params: &[u8], fin: u8) -> Option<Osc> {
    use crate::reports::Query;
    Some(Osc::Query(match (params, fin) {
        (b">" | b">0", b'q') => Query::XtVersion,
        (b"1337", b'n') => Query::ITerm2Version,
        (b"?996", b'n') => Query::ColorScheme,
        (b"?2031", b'h') => Query::ColorSchemeReports(true),
        (b"?2031", b'l') => Query::ColorSchemeReports(false),
        _ => return None,
    }))
}

/// The event a finished DCS string stands for.
fn dcs_event(buf: Vec<u8>) -> Option<Osc> {
    use crate::reports::Query;
    match buf.as_slice() {
        b"=1s" => return Some(Osc::Sync(true)),
        b"=2s" => return Some(Osc::Sync(false)),
        _ => {}
    }
    if let Some(pt) = buf.strip_prefix(b"$q") {
        return Some(Osc::Query(Query::Decrqss(pt.to_vec())));
    }
    if let Some(pt) = buf.strip_prefix(b"+q") {
        return Some(Osc::Query(Query::XtGetTcap(pt.to_vec())));
    }
    sixel_body(&buf).map(Osc::Sixel)
}

/// A DCS string kept up to this size (a large sixel image).
const DCS_LIMIT: usize = 64 << 20;

/// A DCS body that is a sixel image: the bytes after its parameters and `q`.
fn sixel_body(buf: &[u8]) -> Option<Vec<u8>> {
    let start = buf.iter().position(|b| !(b.is_ascii_digit() || *b == b';'))?;
    (buf[start] == b'q').then(|| buf[start + 1..].to_vec())
}

#[derive(Default)]
pub struct Scanner {
    state: State,
}

fn could_be_wanted(buf: &[u8]) -> bool {
    WANTED.iter().any(|w| buf.starts_with(w) || w.starts_with(buf))
}

impl Scanner {
    /// The events in `bytes`, each with the offset just past its terminator.
    pub fn scan(&mut self, bytes: &[u8]) -> Vec<(usize, Osc)> {
        self.scan_spans(bytes).into_iter().map(|(_, end, osc)| (end, osc)).collect()
    }

    /// The events in `bytes`, each with where its sequence began and the offset just past
    /// its terminator.
    pub fn scan_spans(&mut self, bytes: &[u8]) -> Vec<(usize, usize, Osc)> {
        let mut out = Vec::new();
        // Where the escape sequence being read began (0 when it began in an earlier read).
        let mut began = 0;
        let mut i = 0;
        while i < bytes.len() {
            let b = bytes[i];
            match &mut self.state {
                State::Ground => {
                    if b == 0x1b {
                        began = i;
                        self.state = State::Esc;
                    }
                }
                State::Esc => {
                    self.state = match b {
                        b']' => State::Osc { buf: Vec::new(), keep: true, esc: false, cut: false },
                        b'P' => State::Dcs { buf: Vec::new(), esc: false },
                        b'_' => State::Apc { buf: Vec::new(), esc: false },
                        b'[' => State::Csi { buf: Vec::new(), long: false },
                        0x1b => State::Esc,
                        _ => State::Ground,
                    };
                }
                State::Apc { buf, esc } => {
                    if *esc {
                        if b == b'\\' {
                            let done = std::mem::take(buf);
                            self.state = State::Ground;
                            if let Some(body) = done.strip_prefix(b"G") {
                                out.push((began, i + 1, Osc::Kitty(body.to_vec())));
                            }
                        } else {
                            began = i.saturating_sub(1);
                            self.state = State::Esc;
                            continue;
                        }
                    } else {
                        match b {
                            0x1b => *esc = true,
                            0x18 | 0x1a => self.state = State::Ground,
                            _ => {
                                let rest = &bytes[i..];
                                let n = rest.iter().position(|c| matches!(c, 0x1b | 0x18 | 0x1a)).unwrap_or(rest.len());
                                if buf.len() + n <= DCS_LIMIT {
                                    buf.extend_from_slice(&rest[..n]);
                                } else {
                                    buf.clear();
                                    buf.push(b'x');
                                }
                                i += n;
                                continue;
                            }
                        }
                    }
                }
                State::Csi { buf, long } => match b {
                    0x1b => {
                        began = i;
                        self.state = State::Esc;
                    }
                    0x18 | 0x1a => self.state = State::Ground,
                    0x20..=0x3f => {
                        if buf.len() < CSI_LIMIT {
                            buf.push(b);
                        } else {
                            *long = true;
                        }
                    }
                    0x40..=0x7e => {
                        let ev = if *long { None } else { csi_query(buf, b) };
                        self.state = State::Ground;
                        if let Some(ev) = ev {
                            out.push((began, i + 1, ev));
                        }
                    }
                    // Other control bytes act inside a control sequence without ending it.
                    _ => {}
                },
                State::Dcs { buf, esc } => {
                    if *esc {
                        if b == b'\\' {
                            let done = std::mem::take(buf);
                            self.state = State::Ground;
                            if let Some(ev) = dcs_event(done) {
                                out.push((began, i + 1, ev));
                            }
                        } else {
                            began = i.saturating_sub(1);
                            self.state = State::Esc;
                            continue;
                        }
                    } else {
                        match b {
                            0x1b => *esc = true,
                            0x18 | 0x1a => self.state = State::Ground,
                            _ => {
                                let rest = &bytes[i..];
                                let n = rest.iter().position(|c| matches!(c, 0x1b | 0x18 | 0x1a)).unwrap_or(rest.len());
                                if buf.len() + n <= DCS_LIMIT {
                                    buf.extend_from_slice(&rest[..n]);
                                } else {
                                    buf.clear();
                                    buf.push(b'x');
                                }
                                i += n;
                                continue;
                            }
                        }
                    }
                }
                State::Osc { buf, keep, esc, cut } => {
                    if *esc {
                        if b == b'\\' {
                            let done = std::mem::take(buf);
                            let keep = *keep;
                            self.state = State::Ground;
                            if keep {
                                if let Some(ev) = finish(&done) {
                                    out.push((began, i + 1, ev));
                                }
                            }
                        } else {
                            // ESC and something else: the OSC is cut off, and a new escape starts.
                            began = i.saturating_sub(1);
                            self.state = State::Esc;
                            continue;
                        }
                    } else {
                        match b {
                            0x07 => {
                                let done = std::mem::take(buf);
                                let keep = *keep;
                                self.state = State::Ground;
                                if keep {
                                    if let Some(ev) = finish(&done) {
                                        out.push((began, i + 1, ev));
                                    }
                                }
                            }
                            0x1b => *esc = true,
                            0x18 | 0x1a => self.state = State::Ground,
                            _ => {
                                if *keep && (buf.len() >= 24 || *cut) {
                                    // The prefix is known: take the whole run up to the next
                                    // control byte at once (a large inline image streams by).
                                    let rest = &bytes[i..];
                                    let n = rest.iter().position(|c| matches!(c, 0x07 | 0x1b | 0x18 | 0x1a)).unwrap_or(rest.len());
                                    if !*cut {
                                        buf.extend_from_slice(&rest[..n]);
                                        let limit = limit_for(buf);
                                        if buf.len() > limit {
                                            if cut_at_cap(buf) {
                                                buf.truncate(limit);
                                                *cut = true;
                                            } else {
                                                *keep = false;
                                                buf.clear();
                                            }
                                        }
                                    }
                                    i += n;
                                    continue;
                                }
                                if *keep {
                                    buf.push(b);
                                    if buf.len() > limit_for(buf) || !could_be_wanted(buf) {
                                        *keep = false;
                                        buf.clear();
                                    }
                                }
                            }
                        }
                    }
                }
            }
            i += 1;
        }
        out
    }
}

fn finish(buf: &[u8]) -> Option<Osc> {
    if !WANTED.iter().any(|w| buf.starts_with(w)) {
        return None;
    }
    parse(&String::from_utf8_lossy(buf))
}

fn arg<'a>(args: &[&'a str], key: &str) -> Option<&'a str> {
    args.iter().find_map(|a| a.strip_prefix(key).and_then(|r| r.strip_prefix('=')))
}

/// Reads one OSC payload (the part between `ESC ]` and the terminator).
pub fn parse(payload: &str) -> Option<Osc> {
    let (head, rest) = payload.split_once(';')?;
    match head {
        "133" => {
            let args: Vec<&str> = rest.split(';').collect();
            let letter = args[0];
            if letter.chars().count() != 1 {
                return None;
            }
            let aid = arg(&args, "aid").map(str::to_string);
            Some(match letter {
                "A" | "N" | "P" => {
                    let kind = match arg(&args, "k") {
                        None | Some("") | Some("i") => PromptKind::Initial,
                        Some("s") => PromptKind::Secondary,
                        Some("c") => PromptKind::Continuation,
                        Some("r") => PromptKind::Right,
                        Some(_) => PromptKind::Unknown,
                    };
                    Osc::Prompt { kind, aid, fresh_line: letter != "P" }
                }
                "B" => Osc::CommandStart,
                "C" => Osc::OutputStart,
                "D" => {
                    // A bare D does nothing; otherwise the first integer argument is the code.
                    if args.len() < 2 {
                        return None;
                    }
                    let code = args[1..].iter().filter(|a| !a.contains('=')).find_map(|a| a.trim().parse().ok()).unwrap_or(0);
                    Osc::CommandEnd { code: Some(code), aid }
                }
                _ => return None,
            })
        }
        "7" => Some(Osc::Location(rest.to_string())),
        "52" => Some(Osc::Clipboard(rest.as_bytes().to_vec())),
        "1337" => {
            let (k, v) = rest.split_once('=').unwrap_or((rest, ""));
            Some(Osc::Kv(k.to_string(), v.to_string()))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_133_7_and_1337() {
        let mut s = Scanner::default();
        let ev = s.scan(b"a\x1b]133;A;aid=1\x07b\x1b]7;file://h/x\x1b\\c\x1b]1337;CurrentDir=/tmp\x07");
        assert_eq!(ev.len(), 3);
        assert_eq!(ev[0].0, 15);
        assert!(matches!(&ev[0].1, Osc::Prompt { kind: PromptKind::Initial, aid: Some(a), fresh_line: true } if a == "1"));
        assert!(matches!(&ev[1].1, Osc::Location(u) if u == "file://h/x"));
        assert!(matches!(&ev[2].1, Osc::Kv(k, v) if k == "CurrentDir" && v == "/tmp"));
    }

    #[test]
    fn an_osc_split_across_reads_is_one_event() {
        let mut s = Scanner::default();
        assert!(s.scan(b"\x1b]13").is_empty());
        let ev = s.scan(b"3;A\x07rest");
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].0, 4);
        // A terminator split between ESC and backslash.
        let mut s = Scanner::default();
        assert!(s.scan(b"\x1b]133;B\x1b").is_empty());
        assert_eq!(s.scan(b"\\").len(), 1);
    }

    #[test]
    fn other_oscs_are_skipped() {
        let mut s = Scanner::default();
        assert!(s.scan(b"\x1b]0;title\x07\x1b]8;;http://x\x1b\\").is_empty());
        assert!(s.scan(b"\x1b]1337x\x07").is_empty());
    }

    #[test]
    fn a_huge_payload_is_dropped() {
        let mut s = Scanner::default();
        let mut big = b"\x1b]1337;SetUserVar=".to_vec();
        big.extend(std::iter::repeat_n(b'A', 70_000));
        big.push(7);
        big.extend(b"\x1b]133;C\x07");
        let ev = s.scan(&big);
        assert_eq!(ev.len(), 1);
        assert!(matches!(ev[0].1, Osc::OutputStart));
        // Files (inline images) may be far larger, and are kept.
        let mut file = b"\x1b]1337;File=".to_vec();
        file.extend(std::iter::repeat_n(b'A', 70_000));
        file.push(7);
        assert!(matches!(&s.scan(&file)[0].1, Osc::Kv(k, _) if k == "File"));
    }

    #[test]
    fn command_end_takes_the_first_integer() {
        assert!(matches!(parse("133;D;2;aid=4"), Some(Osc::CommandEnd { code: Some(2), aid: Some(a) }) if a == "4"));
        assert!(matches!(parse("133;D;aid=4"), Some(Osc::CommandEnd { code: Some(0), .. })));
        assert!(parse("133;D").is_none());
        assert!(matches!(parse("133;A;k=s"), Some(Osc::Prompt { kind: PromptKind::Secondary, .. })));
        assert!(matches!(parse("133;P"), Some(Osc::Prompt { fresh_line: false, .. })));
        assert!(matches!(parse("133;B"), Some(Osc::CommandStart)));
        assert!(matches!(parse("1337;SetMark"), Some(Osc::Kv(k, v)) if k == "SetMark" && v.is_empty()));
        assert!(matches!(parse("1337;SetUserVar=a=Yg=="), Some(Osc::Kv(k, v)) if k == "SetUserVar" && v == "a=Yg=="));
    }
}
