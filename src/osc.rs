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

use alacritty_terminal::vte::{self, Params, Perform, State as VState};

/// A control sequence as the parser finished it: the first value of each parameter group (what
/// alacritty reads), the marker and intermediate bytes in order, the parser's ignore flag, and the
/// final byte.
struct Csi {
    vals: Vec<u16>,
    inters: Vec<u8>,
    ignore: bool,
    fin: u8,
}

/// A DCS string's header as the parser hooked it.
struct Hook {
    vals: Vec<u16>,
    inters: Vec<u8>,
    ignore: bool,
    action: u8,
}

/// What one byte did in the parser. The scanner runs its own `vte::Parser` (the very state machine
/// that runs the screen) over the same bytes and records only what it needs, so it can never
/// disagree with it about where a sequence starts or ends (milestone 13o, B1).
#[derive(Default)]
struct Tap {
    csi: Option<Csi>,
    hook: Option<Hook>,
    unhook: bool,
    osc_end: bool,
    enq: bool,
}

fn firsts(params: &Params) -> Vec<u16> {
    params.iter().map(|g| g.first().copied().unwrap_or(0)).collect()
}

impl Perform for Tap {
    fn execute(&mut self, byte: u8) {
        if byte == 0x05 {
            self.enq = true;
        }
    }

    fn csi_dispatch(&mut self, params: &Params, intermediates: &[u8], ignore: bool, action: char) {
        self.csi = Some(Csi { vals: firsts(params), inters: intermediates.to_vec(), ignore, fin: action as u8 });
    }

    fn hook(&mut self, params: &Params, intermediates: &[u8], ignore: bool, action: char) {
        self.hook = Some(Hook { vals: firsts(params), inters: intermediates.to_vec(), ignore, action: action as u8 });
    }

    fn unhook(&mut self) {
        self.unhook = true;
    }

    fn osc_dispatch(&mut self, _params: &[&[u8]], _bell_terminated: bool) {
        self.osc_end = true;
    }
}

impl Csi {
    /// The held sequence this control sequence is, if any, read exactly as alacritty reads it.
    fn held(&self, cup_armed: bool) -> Option<Osc> {
        use crate::emu::Held;
        if self.ignore {
            return None;
        }
        let vals = &self.vals[..];
        let first = vals.first().copied().unwrap_or(0);
        let held = match (self.inters.as_slice(), self.fin) {
            ([], b'b') => Held::Rep(first),
            ([], b'J') if first == 3 => Held::ClearHistory,
            ([b'?'], b'h' | b'l') if vals.iter().any(|v| matches!(v, 47 | 1047 | 1048 | 1049)) => {
                Held::AltScreen { params: vals.to_vec(), set: self.fin == b'h' }
            }
            ([], b't') if matches!(first, 20 | 21) => Held::TitleReport(first as u8),
            ([], b'i') => Held::Print(first),
            ([], b'H' | b'f') if cup_armed => Held::Cup { params: vals.to_vec(), fin: self.fin },
            ([b'*'], b'y') => Held::Decrqcra(vals.to_vec()),
            _ => return None,
        };
        Some(Osc::Held(held))
    }

    /// The query this control sequence asks, if Tandem answers it.
    fn query(&self) -> Option<Osc> {
        use crate::reports::Query;
        if self.ignore {
            return None;
        }
        Some(Osc::Query(match (self.inters.as_slice(), self.vals.as_slice(), self.fin) {
            ([b'>'], [0], b'q') => Query::XtVersion,
            ([], [1337], b'n') => Query::ITerm2Version,
            ([b'?'], [996], b'n') => Query::ColorScheme,
            ([b'?'], [2031], b'h') => Query::ColorSchemeReports(true),
            ([b'?'], [2031], b'l') => Query::ColorSchemeReports(false),
            _ => return None,
        }))
    }
}

/// The event a finished DCS string stands for.
fn dcs_event(hook: &Hook, body: Vec<u8>) -> Option<Osc> {
    use crate::reports::Query;
    if hook.ignore {
        return None;
    }
    match (hook.inters.as_slice(), hook.vals.as_slice(), hook.action) {
        ([b'='], [1], b's') if body.is_empty() => Some(Osc::Sync(true)),
        ([b'='], [2], b's') if body.is_empty() => Some(Osc::Sync(false)),
        ([b'$'], _, b'q') => Some(Osc::Query(Query::Decrqss(body))),
        ([b'+'], _, b'q') => Some(Osc::Query(Query::XtGetTcap(body))),
        ([], _, b'q') => Some(Osc::Sixel(body)),
        _ => None,
    }
}

/// A DCS string kept up to this size (a large sixel image).
const DCS_LIMIT: usize = 64 << 20;

/// A DECRQSS (`DCS $ q`) or XTGETTCAP (`DCS + q`) request is a few names: past this it is
/// dropped, not buffered (13o pre-flight P9).
const QUERY_LIMIT: usize = 4096;

/// How much of a DCS string is kept: a query's few names, or an image.
fn dcs_limit(hook: &Hook) -> usize {
    match (hook.inters.as_slice(), hook.action) {
        ([b'$' | b'+'], b'q') => QUERY_LIMIT,
        _ => DCS_LIMIT,
    }
}

/// The payload of an OSC string being read.
struct OscBuf {
    buf: Vec<u8>,
    keep: bool,
    cut: bool,
    len: usize,
    swallow: bool,
}

/// The string the parser is inside, if any.
#[derive(Default)]
enum Str {
    #[default]
    None,
    Osc(OscBuf),
    /// `ESC X` (SOS) or `ESC ^` (PM): a string to ST that ignores everything, ENQ included.
    Sos,
    /// `ESC P`: a DCS string (sixel images are kept).
    Dcs { buf: Vec<u8>, hook: Hook, over: bool },
    /// `ESC _`: an APC string (kitty graphics commands are kept).
    Apc { buf: Vec<u8> },
}

/// An event that waits for the `\` of its string terminator.
struct Deferred {
    ev: Osc,
    began: usize,
}

#[derive(Default)]
pub struct Scanner {
    parser: vte::Parser,
    tap: Tap,
    string: Str,
    /// Where the string being read began in the current read (0 when in an earlier one).
    sbegan: usize,
    deferred: Option<Deferred>,
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
        let mut i = 0;
        while i < bytes.len() {
            let b = bytes[i];
            if let Some(d) = self.deferred.take() {
                if b == b'\\' {
                    out.push((d.began, i + 1, d.ev));
                }
            }
            let before = self.parser.state();
            // Runs the parser would only buffer or print are taken at once; the parser sees the
            // byte that ends them, so it never holds a string's text.
            let stop: &[u8] = match before {
                VState::Ground => {
                    let n = bytes[i..].iter().take_while(|c| (0x20..=0x7e).contains(*c)).count();
                    if n > 0 {
                        self.parser.advance(&mut self.tap, &bytes[i..i + n]);
                        i += n;
                        continue;
                    }
                    &[]
                }
                VState::OscString => &[0x07, 0x18, 0x1a, 0x1b],
                VState::DcsPassthrough => &[0x18, 0x1a, 0x1b, 0x9c],
                VState::SosPmApcString => &[0x18, 0x1a, 0x1b],
                _ => &[],
            };
            if !stop.is_empty() {
                let n = bytes[i..].iter().position(|c| stop.contains(c)).unwrap_or(bytes.len() - i);
                if n > 0 {
                    self.string_run(i, &bytes[i..i + n], &mut out);
                    i += n;
                    continue;
                }
            }
            if b == 0x1b {
                began = i;
            }
            self.parser.advance(&mut self.tap, &bytes[i..=i]);
            let after = self.parser.state();
            self.react(before, after, b, i, began, &mut out);
            i += 1;
        }
        self.pending = (self.parser.state() != VState::Ground).then_some(began);
        self.sbegan = 0;
        if let Some(d) = &mut self.deferred {
            d.began = 0;
        }
        out
    }

    /// What the byte at `i` did in the parser.
    fn react(&mut self, before: VState, after: VState, b: u8, i: usize, began: usize, out: &mut Vec<(usize, usize, Osc)>) {
        let tap = std::mem::take(&mut self.tap);
        if let Some(csi) = &tap.csi {
            if let Some(ev) = csi.held(self.cup_armed).or_else(|| csi.query()) {
                out.push((began, i + 1, ev));
            }
        }
        if tap.enq && before == VState::Ground {
            out.push((i, i + 1, Osc::Held(crate::emu::Held::Enq)));
        }
        if before == VState::OscString && after != VState::OscString {
            self.end_osc(b, i, out);
        }
        if tap.unhook {
            self.end_dcs(b, i, out);
        }
        if before == VState::SosPmApcString && after != VState::SosPmApcString {
            self.end_apc(b);
        }
        if after == VState::OscString && before != VState::OscString {
            self.string = Str::Osc(OscBuf { buf: Vec::new(), keep: true, cut: false, len: 0, swallow: false });
            self.sbegan = began;
        }
        if let Some(hook) = tap.hook {
            self.string = Str::Dcs { buf: Vec::new(), hook, over: false };
            self.sbegan = began;
        }
        if after == VState::SosPmApcString && before != VState::SosPmApcString {
            self.string = if b == b'_' { Str::Apc { buf: Vec::new() } } else { Str::Sos };
            self.sbegan = began;
        }
    }

    /// The parser ended an OSC string at the byte `b`.
    fn end_osc(&mut self, b: u8, i: usize, out: &mut Vec<(usize, usize, Osc)>) {
        let Str::Osc(o) = std::mem::take(&mut self.string) else { return };
        if o.swallow {
            // Cancelled at the cap: the rest was dropped up to its terminator, and a BEL with it.
            if b == 0x07 {
                out.push((i, i + 1, Osc::Held(crate::emu::Held::Drop)));
            }
            return;
        }
        if !o.keep {
            return;
        }
        let Some(ev) = finish(&o.buf) else { return };
        match b {
            0x07 => out.push((self.sbegan, i + 1, ev)),
            0x1b => self.deferred = Some(Deferred { ev, began: self.sbegan }),
            _ => {}
        }
    }

    /// The parser ended a DCS string at the byte `b`.
    fn end_dcs(&mut self, b: u8, i: usize, out: &mut Vec<(usize, usize, Osc)>) {
        let Str::Dcs { buf, hook, over } = std::mem::take(&mut self.string) else { return };
        // A query past the cap is answered as an invalid one (an error reply), not dropped.
        let buf = match (over, dcs_limit(&hook) == QUERY_LIMIT) {
            (true, true) => b"zz".to_vec(),
            (true, false) => return,
            _ => buf,
        };
        let Some(ev) = dcs_event(&hook, buf) else { return };
        match b {
            0x9c => out.push((self.sbegan, i + 1, ev)),
            0x1b => self.deferred = Some(Deferred { ev, began: self.sbegan }),
            _ => {}
        }
    }

    /// The parser ended an SOS, PM or APC string at the byte `b`.
    fn end_apc(&mut self, b: u8) {
        let Str::Apc { buf } = std::mem::take(&mut self.string) else {
            self.string = Str::None;
            return;
        };
        if b == 0x1b {
            if let Some(body) = buf.strip_prefix(b"G") {
                self.deferred = Some(Deferred { ev: Osc::Kitty(body.to_vec()), began: self.sbegan });
            }
        }
    }

    /// Bytes inside the string the parser is in, up to (not including) its terminator.
    fn string_run(&mut self, base: usize, run: &[u8], out: &mut Vec<(usize, usize, Osc)>) {
        match &mut self.string {
            Str::Osc(o) => {
                if o.swallow {
                    out.push((base, base + run.len(), Osc::Held(crate::emu::Held::Drop)));
                    return;
                }
                let mut p = 0;
                while p < run.len() {
                    if !o.keep {
                        // Not ours: skipped, but counted, so a string past the cap is cut (vte
                        // would buffer all of it).
                        let n = run.len() - p;
                        if o.len + n > OSC_CAP {
                            let k = base + p + OSC_CAP.saturating_sub(o.len);
                            out.push((k, k + 1, Osc::Held(crate::emu::Held::CancelString)));
                            o.swallow = true;
                            if k + 1 < base + run.len() {
                                out.push((k + 1, base + run.len(), Osc::Held(crate::emu::Held::Drop)));
                            }
                        } else {
                            o.len += n;
                        }
                        return;
                    }
                    if o.buf.len() >= 24 || o.cut {
                        // The prefix is known: take the whole run at once (a large inline image
                        // streams by).
                        o.len += run.len() - p;
                        if !o.cut {
                            o.buf.extend_from_slice(&run[p..]);
                            let limit = limit_for(&o.buf);
                            if o.buf.len() > limit {
                                if cut_at_cap(&o.buf) {
                                    o.buf.truncate(limit);
                                    o.cut = true;
                                } else {
                                    o.keep = false;
                                    o.buf.clear();
                                }
                            }
                        }
                        return;
                    }
                    o.len += 1;
                    o.buf.push(run[p]);
                    if o.buf.len() > limit_for(&o.buf) || !could_be_wanted(&o.buf) {
                        o.keep = false;
                        o.buf.clear();
                    }
                    p += 1;
                }
            }
            Str::Dcs { buf, over, hook } => {
                if *over {
                    return;
                }
                let limit = dcs_limit(hook);
                // The parser passes on every byte up to DEL; DEL and bytes of 0x80 or more are
                // ignored.
                let n = run.iter().filter(|c| **c <= 0x7e).count();
                if buf.len() + n <= limit {
                    buf.extend(run.iter().copied().filter(|c| *c <= 0x7e));
                } else {
                    buf.clear();
                    *over = true;
                }
            }
            Str::Apc { buf } => {
                if buf.len() + run.len() <= DCS_LIMIT {
                    buf.extend_from_slice(run);
                } else {
                    buf.clear();
                    buf.push(b'x');
                }
            }
            Str::Sos | Str::None => {}
        }
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

    /// 13o Task 6 (P9): DECRQSS and XTGETTCAP requests are cut at 4 KiB; an image is not.
    #[test]
    fn dcs_queries_are_capped() {
        use crate::reports::Query;
        let events = |head: &[u8], n: usize| {
            let mut bytes = head.to_vec();
            bytes.extend(std::iter::repeat_n(b'm', n));
            bytes.extend_from_slice(b"\x1b\\");
            let mut s = Scanner::default();
            s.scan_spans(&bytes).into_iter().map(|(_, _, o)| o).collect::<Vec<_>>()
        };
        assert!(matches!(events(b"\x1bP$q", 4096).as_slice(), [Osc::Query(Query::Decrqss(b))] if b.len() == 4096));
        assert!(matches!(events(b"\x1bP$q", 4097).as_slice(), [Osc::Query(Query::Decrqss(b))] if b == b"zz"), "past the cap it is an invalid query");
        assert!(matches!(events(b"\x1bP+q", 4096).as_slice(), [Osc::Query(Query::XtGetTcap(_))]));
        assert!(matches!(events(b"\x1bP+q", 100_000).as_slice(), [Osc::Query(Query::XtGetTcap(b))] if b == b"zz"));
        assert!(matches!(events(b"\x1bPq", 100_000).as_slice(), [Osc::Sixel(b)] if b.len() == 100_000), "an image keeps its size");
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

    /// 13o Task 2 fix (B1): the scanner reads the stream as vte does, so every spelling vte takes
    /// as a held sequence is found, with its span from the ESC.
    #[test]
    fn held_sequences_are_found_in_every_vte_spelling() {
        use crate::emu::Held;
        let one = |text: &[u8]| {
            let mut s = Scanner::default();
            let ev = s.scan_spans(text);
            let held: Vec<_> = ev.iter().filter(|e| matches!(e.2, Osc::Held(_))).collect();
            assert_eq!(held.len(), 1, "{text:?}: {ev:?}");
            (held[0].0, held[0].1, held[0].2.clone())
        };
        for form in [&b"\x1b\x00[3J"[..], b"\x1b\x07[3J", b"\x1b\r[3J", b"\x1b\x7f[3J", b"\x1b\xc3[3J", b"\x1b\x00\x7f\xff[3J", b"\x1b[\x003J", b"\x1b[3\x7fJ"] {
            let mut text = b"ab".to_vec();
            text.extend(form);
            let (start, end, held) = one(&text);
            assert_eq!((start, end, held), (2, text.len(), Osc::Held(Held::ClearHistory)), "{form:?}");
        }
        let (_, _, held) = one(b"\x1b\x00[?1049h");
        assert_eq!(held, Osc::Held(Held::AltScreen { params: vec![1049], set: true }));
        let (_, _, held) = one(b"\x1b\x7f[5b");
        assert_eq!(held, Osc::Held(Held::Rep(5)));
        // ESC inside a sequence starts a new one.
        let (start, _, held) = one(b"\x1b[1\x1b\x00[3J");
        assert_eq!((start, held), (3, Osc::Held(Held::ClearHistory)));
        // Queries too.
        let mut s = Scanner::default();
        assert_eq!(s.scan(b"\x1b\x00[>q").len(), 1);
    }
}
