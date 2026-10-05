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
const WANTED: [&[u8]; 6] = [b"133;", b"7;", b"1337;", b"52;", b"21337;", b"9;"];

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
    /// OSC 21337: tab status fields.
    TabStatus(String),
    /// OSC 52: the payload after `52;`.
    Clipboard(Vec<u8>),
    /// A report request alacritty does not answer.
    Query(crate::reports::Query),
    /// `DCS = 1 s` (true) or `DCS = 2 s`: synchronized output begins or ends.
    Sync(bool),
    /// OSC 9: a notification's text.
    Notify(String),
    /// OSC 9;4: a progress bar (its state and value).
    Progress(String),
    /// A sequence a setting may drop or rewrite (milestone 13o).
    Held(crate::emu::Held),
}

#[derive(Default)]
enum State {
    #[default]
    Ground,
    Esc,
    Osc { buf: Vec<u8>, keep: bool, esc: bool, cut: bool, len: usize, swallow: bool },
    /// `ESC X` (SOS) or `ESC ^` (PM): a string to ST that ignores everything, ENQ included.
    Sos { esc: bool },
    /// `ESC P`: a DCS string (sixel images are kept).
    Dcs { buf: Vec<u8>, esc: bool },
    /// `ESC _`: an APC string (kitty graphics commands are kept).
    Apc { buf: Vec<u8>, esc: bool },
    /// `ESC [`: a control sequence (a few queries are caught). `long` marks one too long to
    /// be a query.
    Csi { buf: Vec<u8>, long: bool, num: CsiNum },
}

/// Where a control sequence's bytes are, as vte's parser sees them.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Phase {
    #[default]
    Start,
    Param,
    Inter,
    Ignore,
}

/// A control sequence's parameters read as numbers, as vte reads them: digits saturate at u16,
/// `;` and `:` separate, a subparameter after `:` does not count as a parameter of its own (only
/// the first of each group is kept) but does count toward the limit of 32, and more than 32 makes
/// vte drop the sequence.
#[derive(Clone, Default)]
struct CsiNum {
    vals: [u16; MAX_VALUES],
    n: usize,
    total: usize,
    cur: u16,
    in_sub: bool,
    marker: u8,
    inter: u8,
    inters: u8,
    phase: Phase,
}

/// vte's parameter limit.
const MAX_VALUES: usize = 32;

impl CsiNum {
    fn commit(&mut self) {
        if !self.in_sub && self.n < MAX_VALUES {
            self.vals[self.n] = self.cur;
            self.n += 1;
        }
        self.total += 1;
        self.cur = 0;
    }

    fn byte(&mut self, b: u8) {
        match b {
            b'0'..=b'9' => {
                if matches!(self.phase, Phase::Inter | Phase::Ignore) {
                    self.phase = Phase::Ignore;
                } else {
                    self.phase = Phase::Param;
                    self.cur = self.cur.saturating_mul(10).saturating_add((b - b'0') as u16);
                }
            }
            b':' | b';' => {
                if matches!(self.phase, Phase::Inter | Phase::Ignore) {
                    self.phase = Phase::Ignore;
                } else {
                    self.phase = Phase::Param;
                    self.commit();
                    self.in_sub = b == b':';
                }
            }
            0x3c..=0x3f => {
                if self.phase == Phase::Start {
                    self.marker = b;
                    self.phase = Phase::Param;
                } else {
                    self.phase = Phase::Ignore;
                }
            }
            _ => {
                if self.phase != Phase::Ignore {
                    self.phase = Phase::Inter;
                    self.inters = self.inters.saturating_add(1);
                    if self.inters == 1 {
                        self.inter = b;
                    }
                }
            }
        }
    }

    /// The held sequence the finished control sequence is, if any.
    fn held(mut self, fin: u8, cup_armed: bool) -> Option<Osc> {
        use crate::emu::Held;
        self.commit();
        if self.phase == Phase::Ignore || self.total > MAX_VALUES {
            return None;
        }
        let vals = &self.vals[..self.n];
        let first = vals.first().copied().unwrap_or(0);
        let held = match (self.marker, self.inters, fin) {
            (0, 0, b'b') => Held::Rep(first),
            (0, 0, b'J') if first == 3 => Held::ClearHistory,
            (b'?', 0, b'h' | b'l') if vals.iter().any(|v| matches!(v, 47 | 1047 | 1048 | 1049)) => {
                Held::AltScreen { params: vals.to_vec(), set: fin == b'h' }
            }
            (0, 0, b't') if matches!(first, 20 | 21) => Held::TitleReport(first as u8),
            (0, 0, b'i') => Held::Print(first),
            (0, 0, b'H' | b'f') if cup_armed => Held::Cup { params: vals.to_vec(), fin },
            (0, 1, b'y') if self.inter == b'*' => Held::Decrqcra(vals.to_vec()),
            _ => return None,
        };
        Some(Osc::Held(held))
    }
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
    /// After a scan: where an unfinished escape sequence began in those bytes (0 when it
    /// began in an earlier read), or None when the bytes ended between sequences.
    pub pending: Option<usize>,
    /// Report `CSI Pl;Pc H` and `f` as held cursor moves (armed by the screen while a command
    /// runs, milestone 13o Task 4).
    pub cup_armed: bool,
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
        let cup_armed = self.cup_armed;
        let mut i = 0;
        while i < bytes.len() {
            let b = bytes[i];
            match &mut self.state {
                State::Ground => {
                    if b == 0x1b {
                        began = i;
                        self.state = State::Esc;
                    } else if b == 0x05 {
                        out.push((i, i + 1, Osc::Held(crate::emu::Held::Enq)));
                    }
                }
                State::Esc => {
                    self.state = match b {
                        b']' => State::Osc { buf: Vec::new(), keep: true, esc: false, cut: false, len: 0, swallow: false },
                        b'X' | b'^' => State::Sos { esc: false },
                        b'P' => State::Dcs { buf: Vec::new(), esc: false },
                        b'_' => State::Apc { buf: Vec::new(), esc: false },
                        b'[' => State::Csi { buf: Vec::new(), long: false, num: CsiNum::default() },
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
                State::Csi { buf, long, num } => match b {
                    0x1b => {
                        began = i;
                        self.state = State::Esc;
                    }
                    0x18 | 0x1a => self.state = State::Ground,
                    0x20..=0x3f => {
                        num.byte(b);
                        if buf.len() < CSI_LIMIT {
                            buf.push(b);
                        } else {
                            *long = true;
                        }
                    }
                    0x40..=0x7e => {
                        let ev = num.clone().held(b, cup_armed).or_else(|| if *long { None } else { csi_query(buf, b) });
                        self.state = State::Ground;
                        if let Some(ev) = ev {
                            out.push((began, i + 1, ev));
                        }
                    }
                    // Other control bytes act inside a control sequence without ending it.
                    _ => {}
                },
                State::Sos { esc } => {
                    if *esc {
                        if b == b'\\' {
                            self.state = State::Ground;
                        } else {
                            began = i.saturating_sub(1);
                            self.state = State::Esc;
                            continue;
                        }
                    } else {
                        match b {
                            0x1b => *esc = true,
                            0x18 | 0x1a => self.state = State::Ground,
                            _ => {}
                        }
                    }
                }
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
                State::Osc { buf, keep, esc, cut, len, swallow } => {
                    if *swallow {
                        // Past the cap: the parser was sent CAN, and what is left of the string
                        // is dropped up to its terminator (an ESC goes on as it was).
                        if *esc {
                            if b == b'\\' {
                                self.state = State::Ground;
                            } else {
                                began = i.saturating_sub(1);
                                self.state = State::Esc;
                                continue;
                            }
                        } else {
                            match b {
                                0x07 => {
                                    out.push((i, i + 1, Osc::Held(crate::emu::Held::Drop)));
                                    self.state = State::Ground;
                                }
                                0x1b => *esc = true,
                                0x18 | 0x1a => self.state = State::Ground,
                                _ => {
                                    let rest = &bytes[i..];
                                    let n = rest.iter().position(|c| matches!(c, 0x07 | 0x1b | 0x18 | 0x1a)).unwrap_or(rest.len());
                                    out.push((i, i + n, Osc::Held(crate::emu::Held::Drop)));
                                    i += n;
                                    continue;
                                }
                            }
                        }
                        i += 1;
                        continue;
                    }
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
                                if !*keep {
                                    // Not ours: skipped, but counted, so a string past the cap is cut
                                    // (vte would buffer all of it).
                                    let rest = &bytes[i..];
                                    let n = rest.iter().position(|c| matches!(c, 0x07 | 0x1b | 0x18 | 0x1a)).unwrap_or(rest.len());
                                    if *len + n > OSC_CAP {
                                        let k = i + OSC_CAP.saturating_sub(*len);
                                        out.push((k, k + 1, Osc::Held(crate::emu::Held::CancelString)));
                                        *swallow = true;
                                        i = k + 1;
                                    } else {
                                        *len += n;
                                        i += n;
                                    }
                                    continue;
                                }
                                if *keep && (buf.len() >= 24 || *cut) {
                                    // The prefix is known: take the whole run up to the next
                                    // control byte at once (a large inline image streams by).
                                    let rest = &bytes[i..];
                                    let n = rest.iter().position(|c| matches!(c, 0x07 | 0x1b | 0x18 | 0x1a)).unwrap_or(rest.len());
                                    *len += n;
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
                                    *len += 1;
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
        self.pending = (!matches!(self.state, State::Ground)).then_some(began);
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
        "9" => {
            if let Some(p) = rest.strip_prefix("4;") {
                return Some(Osc::Progress(p.to_string()));
            }
            // ConEmu's subcommands (`9;<n>;...`, which Windows shells send): 9;9 reports the
            // current folder (quoted or not); the others are not notifications.
            let (n, arg) = rest.split_once(';').unwrap_or((rest, ""));
            if !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) {
                return (n == "9").then(|| {
                    let path = arg.strip_prefix('"').and_then(|a| a.strip_suffix('"')).unwrap_or(arg);
                    Osc::Kv("CurrentDir".into(), path.to_string())
                });
            }
            Some(Osc::Notify(rest.to_string()))
        }
        "52" => Some(Osc::Clipboard(rest.as_bytes().to_vec())),
        "21337" => Some(Osc::TabStatus(rest.to_string())),
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

    /// The spans of the held events in `bytes`.
    fn held_in(bytes: &[u8]) -> Vec<(usize, usize, crate::emu::Held)> {
        let mut s = Scanner::default();
        s.scan_spans(bytes).into_iter().filter_map(|(a, b, o)| if let Osc::Held(h) = o { Some((a, b, h)) } else { None }).collect()
    }

    #[test]
    fn held_sequences_are_found() {
        use crate::emu::Held;
        let parts: [(&[u8], Held); 7] = [
            (b"\x1b[5b", Held::Rep(5)),
            (b"\x1b[3J", Held::ClearHistory),
            (b"\x1b[?1;1049h", Held::AltScreen { params: vec![1, 1049], set: true }),
            (b"\x1b[21t", Held::TitleReport(21)),
            (b"\x1b[1;1;1;1;2;2*y", Held::Decrqcra(vec![1, 1, 1, 1, 2, 2])),
            (b"\x1b[5i", Held::Print(5)),
            (b"\x05", Held::Enq),
        ];
        let mut bytes = b"x".to_vec();
        let mut want = Vec::new();
        for (text, held) in &parts {
            want.push((bytes.len(), bytes.len() + text.len(), held.clone()));
            bytes.extend_from_slice(text);
        }
        assert_eq!(held_in(&bytes), want);
        // Not held: other erases and modes, other window reports, an ENQ inside a string.
        for text in [&b"\x1b[2J"[..], b"\x1b[?25h", b"\x1b[18t", b"\x1b]0;a\x05b\x07", b"\x1bPqa\x05\x1b\\", b"\x1b^a\x05b\x1b\\", b"\x1bXa\x05b\x1b\\", b"\x1b[?5i", b"\x1b[1;5H"] {
            assert_eq!(held_in(text), vec![], "{:?}", String::from_utf8_lossy(text));
        }
        // An ENQ right after a string is held again.
        assert_eq!(held_in(b"\x1b^a\x1b\\\x05"), vec![(5, 6, Held::Enq)]);
    }

    /// B1: the scanner reads numbers as vte does, so padding cannot hide a sequence.
    #[test]
    fn held_sequences_read_numbers() {
        use crate::emu::Held;
        let one = |text: &[u8]| {
            let mut v = held_in(text);
            assert_eq!(v.len(), 1, "{:?}", String::from_utf8_lossy(text));
            let (a, b, h) = v.remove(0);
            assert_eq!((a, b), (0, text.len()));
            h
        };
        assert_eq!(one(b"\x1b[000000000000000000000000000000000003J"), Held::ClearHistory);
        assert_eq!(one(b"\x1b[3:1J"), Held::ClearHistory);
        assert_eq!(one(b"\x1b[3;0;0;0;0;0;0;0;0;0;0;0;0;0;0;0;0;0;0;0;0;0;0;0;0;0;0;0;0;0;0;0J"), Held::ClearHistory);
        assert_eq!(one(b"\x1b[?0001049h"), Held::AltScreen { params: vec![1049], set: true });
        assert_eq!(one(b"\x1b[?25;47l"), Held::AltScreen { params: vec![25, 47], set: false });
        assert_eq!(one(b"\x1b[?1049:2h"), Held::AltScreen { params: vec![1049], set: true });
        assert_eq!(one(b"\x1b[b"), Held::Rep(0));
        assert_eq!(one(b"\x1b[99999999b"), Held::Rep(u16::MAX));
        assert_eq!(one(b"\x1b[i"), Held::Print(0));
        assert_eq!(one(b"\x1b[20;1t"), Held::TitleReport(20));
        // 33 values: vte ignores the sequence, so nothing is held.
        let mut long = b"\x1b[3".to_vec();
        for _ in 0..32 {
            long.extend(b";0");
        }
        long.push(b'J');
        assert_eq!(held_in(&long), vec![]);
        // Not an erase 3: another first value, a marker, a misplaced marker, an intermediate.
        for text in [&b"\x1b[;3J"[..], b"\x1b[?3J", b"\x1b[0;3J", b"\x1b[3$J", b"\x1b[3?J", b"\x1b[?5:1049h", b"\x1b[>1049h", b"\x1b[1049h", b"\x1b[?1049$h", b"\x1b[5;1b\x1b[3 b"] {
            let got = held_in(text);
            assert!(got.iter().all(|(_, _, h)| !matches!(h, Held::ClearHistory | Held::AltScreen { .. })), "{:?}", String::from_utf8_lossy(text));
        }
        // The cursor moves are held only while armed.
        assert_eq!(held_in(b"\x1b[3;4H"), vec![]);
        let mut s = Scanner::default();
        s.cup_armed = true;
        let ev = s.scan_spans(b"\x1b[3;4H\x1b[f");
        assert_eq!(ev[0].2, Osc::Held(Held::Cup { params: vec![3, 4], fin: b'H' }));
        assert_eq!(ev[1].2, Osc::Held(Held::Cup { params: vec![0], fin: b'f' }));
    }

    #[test]
    fn held_sequences_split_across_reads_start_at_zero() {
        use crate::emu::Held;
        let mut s = Scanner::default();
        assert!(s.scan_spans(b"ab\x1b[3").is_empty());
        assert_eq!(s.pending, Some(2));
        let ev = s.scan_spans(b"Jx");
        assert_eq!(ev, vec![(0, 1, Osc::Held(Held::ClearHistory))]);
    }

    #[test]
    fn a_huge_osc_is_cancelled() {
        use crate::emu::Held;
        let mut s = Scanner::default();
        let mut big = b"\x1b]0;".to_vec();
        big.extend(std::iter::repeat_n(b'A', OSC_CAP + 10));
        big.extend(b"\x07z\x1b]0;ok\x07");
        let ev = s.scan_spans(&big);
        let cancel = ev.iter().position(|e| e.2 == Osc::Held(Held::CancelString)).expect("cancelled");
        assert_eq!(ev[cancel].1 - ev[cancel].0, 1);
        assert!(ev[cancel].0 <= 4 + OSC_CAP + 1);
        // Everything after the cut up to the terminator is dropped; the next OSC is untouched.
        let dropped: usize = ev[cancel + 1..].iter().filter(|e| e.2 == Osc::Held(Held::Drop)).map(|e| e.1 - e.0).sum();
        let bel = 4 + OSC_CAP + 10;
        assert_eq!(dropped, bel - ev[cancel].1 + 1);
        assert!(s.pending.is_none());
    }
}
