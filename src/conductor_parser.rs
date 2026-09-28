//! iTerm2's VT100ConductorParser for the conductor.sh stage (milestone 14b): the init line
//! after `ESC P 2000 p`, then OSC 134/135 replies in ground and body. Pure: bytes in, tokens
//! out; unparsed input waits inside the parser for the next read. Milestone 14c adds the
//! framer's payload rules and output states to these types.

use base64::Engine;

const ESC: u8 = 0x1b;

/// The most unparsed input a parser keeps (iTerm2's kMaxDataLength, `DP:32`); past it the
/// parser unhooks and the input is output.
pub const MAX_HELD: usize = 1024 * 1024;

/// The longest command id a `begin` may carry (Tandem's: iTerm2 takes any length).
/// conductor.sh's ids (`randomnumber`) are a counter and four random numbers, about 30
/// bytes; framer.py's (`makeid`) about 25. Every reply line is matched against the id, so
/// a remote-sent id of a megabyte would make each line cost a megabyte.
pub const MAX_ID: usize = 64;

/// Why a longer id ends the conductor (`Token::Error`).
pub const ID_TOO_LONG: &str = "The remote sent a command id that is too long";

/// The init line, checked (iTerm2's parseHookSSHConductorParameter plus the base64 decode of
/// terminalDidHookSSHConductorWithParams).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Init {
    pub token: String,
    pub unique_id: String,
    pub bool_args: String,
    pub sshargs: String,
    pub dcs_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Token {
    Init(Init),
    /// A command's reply began: its id.
    Begin(String),
    /// One reply payload (it may hold newlines).
    Line(String),
    End { id: String, status: u8, kind: String },
    /// conductor.sh's `unhook`: what follows is output again.
    Unhook,
    /// A protocol error (Tandem's): the parser unhooked, and the conductor fails with this
    /// text rather than waiting for a reply that cannot come.
    Error(String),
}

/// Checks the init line; the parser's id is appended as the last word first, as iTerm2 builds
/// `<line> <dcsID>`. None where iTerm2 logs and returns.
pub fn parse_init(line: &str, dcs_id: &str) -> Option<Init> {
    let param = format!("{line} {dcs_id}");
    let mut parts: Vec<&str> = param.split(' ').collect();
    if parts.len() < 5 {
        return None;
    }
    let (token, unique_id, bool_args) = (parts[0], parts[1], parts[2]);
    // Unknown words before the separator are skipped.
    let mut i = 3 + parts[3..].iter().position(|w| *w == "-")?;
    i += 1;
    if i >= parts.len() {
        return None;
    }
    let id = parts.pop()?;
    let sshargs = parts[i..].join(" ");
    let decode = |s: &str| {
        let bytes = base64::engine::general_purpose::STANDARD.decode(s).ok()?;
        String::from_utf8(bytes).ok()
    };
    Some(Init {
        token: token.to_string(),
        unique_id: unique_id.to_string(),
        bool_args: decode(bool_args)?,
        sshargs: decode(&sshargs)?,
        dcs_id: id.to_string(),
    })
}

/// SSH_END's text `<id> <status> <type> ...` (VT100Terminal): the status is the leading
/// digits of the second word (none: 0) and over 255 drops the token.
pub fn parse_end(text: &str) -> Option<(String, u8, String)> {
    let parts: Vec<&str> = text.split(' ').collect();
    if parts.len() < 3 {
        return None;
    }
    // NSScanner's scanUnsignedLongLong: leading whitespace skipped, saturating.
    let digits = parts[1].trim_start().bytes().take_while(u8::is_ascii_digit);
    let status = digits.fold(0u64, |n, d| n.saturating_mul(10).saturating_add(u64::from(d - b'0')));
    let status = u8::try_from(status).ok()?;
    Some((parts[0].to_string(), status, parts[2].to_string()))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Osc {
    /// The code and the payload after its first `:` ("" when it has none).
    Osc(u32, String),
    NotOsc,
    /// The input ended inside a possible OSC: wait for more.
    Blocked,
}

/// Reads one `ESC ] <digits> ; <payload> ESC \` from the start of `bytes` (iTerm2's
/// parseNextOSC). With `skip_garbage` anything before an OSC is dropped; without it anything
/// else is NotOsc. The count is the bytes consumed: through the terminator for an OSC, the
/// garbage already dropped when blocked, 0 for NotOsc.
pub fn next_osc(bytes: &[u8], skip_garbage: bool) -> (Osc, usize) {
    let (osc, n, _) = scan_osc(bytes, skip_garbage, Scan::default());
    (osc, n)
}

/// Where an OSC reader stopped. Positions count from the start of the bytes it read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum ScanState {
    #[default]
    Ground,
    Esc,
    /// The code's digits start here.
    Code(usize),
    /// The code, and where the payload starts.
    Payload(u32, usize),
    PayloadEsc(u32, usize),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Scan {
    state: ScanState,
    /// The next byte to read.
    at: usize,
    /// Where the OSC being read began: blocked input is kept from here.
    checkpoint: usize,
}

impl Scan {
    /// The same point, counted from `k` bytes later (`k` is at most every position in it).
    fn shifted(self, k: usize) -> Scan {
        let state = match self.state {
            ScanState::Code(start) => ScanState::Code(start - k),
            ScanState::Payload(code, start) => ScanState::Payload(code, start - k),
            ScanState::PayloadEsc(code, start) => ScanState::PayloadEsc(code, start - k),
            s => s,
        };
        Scan { state, at: self.at - k, checkpoint: self.checkpoint - k }
    }
}

/// next_osc continued from `from` (the point a blocked read stopped at, over the same bytes
/// with more after them), so a long OSC arriving in pieces is read once. Blocked also gives
/// the point to continue from.
fn scan_osc(bytes: &[u8], skip_garbage: bool, from: Scan) -> (Osc, usize, Scan) {
    use ScanState as S;
    let Scan { mut state, at: mut i, mut checkpoint } = from;
    while i < bytes.len() {
        let c = bytes[i];
        i += 1;
        state = match state {
            S::Ground if c == ESC => S::Esc,
            S::Esc if c == b']' => S::Code(i),
            S::Code(start) if c.is_ascii_digit() => S::Code(start),
            S::Code(start) if c == b';' && number(&bytes[start..i - 1]).is_some() => {
                S::Payload(number(&bytes[start..i - 1]).unwrap_or_default(), i)
            }
            S::Payload(code, start) if c == ESC => S::PayloadEsc(code, start),
            S::Payload(code, start) => {
                // Nothing but ESC matters inside a payload.
                i += bytes[i..].iter().position(|b| *b == ESC).unwrap_or(bytes.len() - i);
                S::Payload(code, start)
            }
            S::PayloadEsc(code, start) if c == b'\\' => {
                // Unrecognized parameters before the first colon are ignored, and the colon.
                let payload = &bytes[start..i - 2];
                let text = match payload.iter().position(|b| *b == b':') {
                    Some(k) => String::from_utf8_lossy(&payload[k + 1..]).into_owned(),
                    None => String::new(),
                };
                return (Osc::Osc(code, text), i, Scan::default());
            }
            S::PayloadEsc(..) => {
                // ESC then anything but a backslash: another OSC began inside this one.
                if !skip_garbage {
                    return (Osc::NotOsc, checkpoint, Scan::default());
                }
                if c == ESC {
                    checkpoint = i - 1;
                } else {
                    // Tandem restarts at the ESC that interrupted (iTerm2 backs up one byte
                    // past it and loses the second OSC, bug B14).
                    checkpoint = i - 2;
                    i -= 1;
                }
                S::Esc
            }
            _ => {
                if !skip_garbage {
                    return (Osc::NotOsc, checkpoint, Scan::default());
                }
                checkpoint = i;
                S::Ground
            }
        };
    }
    (Osc::Blocked, checkpoint, Scan { state, at: i, checkpoint })
}

/// A blocked OSC's point to continue from; anything else starts over.
fn osc_resume(resume: Resume) -> Scan {
    match resume {
        Resume::Osc(scan) => scan,
        _ => Scan::default(),
    }
}

/// An OSC's code: digits only, none is not a code (Swift's Int of the text).
fn number(digits: &[u8]) -> Option<u32> {
    std::str::from_utf8(digits).ok()?.parse().ok()
}

/// What one read gave.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Fed {
    pub tokens: Vec<Token>,
    /// Some when the parser unhooked: the bytes from that point on, which are screen output
    /// again. The parser is finished then.
    pub rest: Option<Vec<u8>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum State {
    /// Reading the init line.
    Initial,
    Ground,
    /// Inside the reply to the command with this id.
    Body(String),
}

pub struct Parser {
    dcs_id: String,
    state: State,
    /// Input not parsed yet: an unfinished init line or OSC, at most MAX_HELD bytes.
    held: Vec<u8>,
    /// How far into `held` the last read already looked, so the next one goes on from there.
    resume: Resume,
}

/// Where the scan of `held` stopped.
#[derive(Clone, Copy, Debug, Default)]
enum Resume {
    #[default]
    Start,
    /// The init line: this many bytes hold no newline.
    Line(usize),
    Osc(Scan),
}

enum Step {
    Continue,
    Unhook,
}

impl Parser {
    pub fn new(dcs_id: &str) -> Parser {
        Parser { dcs_id: dcs_id.to_string(), state: State::Initial, held: Vec::new(), resume: Resume::Start }
    }

    /// A parser whose init line was already read.
    pub fn hooked(dcs_id: &str) -> Parser {
        Parser { state: State::Ground, ..Parser::new(dcs_id) }
    }

    pub fn dcs_id(&self) -> &str {
        &self.dcs_id
    }

    pub fn feed(&mut self, bytes: &[u8]) -> Fed {
        // Grown in place: held input is neither copied nor read again on each read.
        let mut data = std::mem::take(&mut self.held);
        data.extend_from_slice(bytes);
        // The resume point counts from the start of `data`, so only the first scan uses it.
        let mut resume = std::mem::take(&mut self.resume);
        let mut fed = Fed::default();
        let mut i = 0;
        loop {
            let rest = &data[i..];
            // Matched in place: the body's id is compared, never copied, for each reply line.
            match self.state {
                State::Initial => {
                    let from = match std::mem::take(&mut resume) {
                        Resume::Line(n) => n,
                        _ => 0,
                    };
                    let Some(k) = rest[from..].iter().position(|b| *b == b'\n').map(|k| from + k) else {
                        self.resume = Resume::Line(rest.len());
                        break;
                    };
                    // The newline is left for ground, which drops it.
                    let line = &rest[..k];
                    i += k;
                    match std::str::from_utf8(line).ok().and_then(|s| parse_init(s, &self.dcs_id)) {
                        Some(init) => {
                            fed.tokens.push(Token::Init(init));
                            self.state = State::Ground;
                        }
                        // Tandem unhooks on a rejected line too (iTerm2 stays hooked, bug B4).
                        None => return self.unhook(fed, &data[i..]),
                    }
                }
                State::Ground => {
                    if rest.is_empty() {
                        break;
                    }
                    let (osc, n, scan) = scan_osc(rest, true, osc_resume(std::mem::take(&mut resume)));
                    i += n;
                    let step = match osc {
                        Osc::Osc(134, payload) => self.framer_payload(&payload, &mut fed),
                        Osc::Osc(135, payload) => self.pre_framer_payload(&payload, &mut fed),
                        // Other OSCs are ignored.
                        Osc::Osc(..) => Step::Continue,
                        // Garbage skipping never gives NotOsc.
                        Osc::Blocked | Osc::NotOsc => {
                            self.resume = Resume::Osc(scan.shifted(n));
                            break;
                        }
                    };
                    if let Step::Unhook = step {
                        return self.unhook(fed, &data[i..]);
                    }
                }
                State::Body(ref id) => {
                    if rest.is_empty() {
                        break;
                    }
                    match scan_osc(rest, false, osc_resume(std::mem::take(&mut resume))) {
                        (Osc::Osc(134 | 135, payload), n, _) => {
                            i += n;
                            // iTerm2's `hasPrefix("end \(id) ")` (`P:436`), without building it.
                            let tail = payload.strip_prefix("end ").and_then(|t| t.strip_prefix(id.as_str())).and_then(|t| t.strip_prefix(' '));
                            match tail {
                                Some(tail) => {
                                    let end = parse_end(&format!("{id} {tail}"));
                                    self.state = State::Ground;
                                    if let Some((id, status, kind)) = end {
                                        fed.tokens.push(Token::End { id, status, kind });
                                    }
                                }
                                None => fed.tokens.push(Token::Line(payload)),
                            }
                        }
                        (Osc::Blocked, n, scan) => {
                            self.resume = Resume::Osc(scan.shifted(n));
                            break;
                        }
                        // Anything else is output: the bytes from here on.
                        _ => return self.unhook(fed, rest),
                    }
                }
            }
        }
        data.drain(..i);
        if data.len() > MAX_HELD {
            // An init line or OSC that never ends: Tandem gives up on it and shows it, where
            // iTerm2's DCS parser stops at kMaxDataLength (`DP:441-448`).
            return self.unhook(fed, &data);
        }
        self.held = data;
        fed
    }

    fn unhook(&self, mut fed: Fed, rest: &[u8]) -> Fed {
        fed.rest = Some(rest.to_vec());
        fed
    }

    /// OSC 135 in ground (parsePreFramerPayload).
    fn pre_framer_payload(&mut self, payload: &str, fed: &mut Fed) -> Step {
        if payload.starts_with("begin ") {
            return self.begin(payload, fed);
        }
        if payload == "unhook" {
            fed.tokens.push(Token::Unhook);
        }
        // Any other word unhooks silently (iTerm2 bug B5).
        Step::Unhook
    }

    /// OSC 134 in ground (parseFramerPayload); 14c adds %output and the rest before the `%`
    /// rule.
    fn framer_payload(&mut self, payload: &str, fed: &mut Fed) -> Step {
        if payload.starts_with("begin ") {
            return self.begin(payload, fed);
        }
        if payload.starts_with('%') {
            // An unrecognized notification is ignored.
            return Step::Continue;
        }
        Step::Unhook
    }

    fn begin(&mut self, payload: &str, fed: &mut Fed) -> Step {
        // "begin " always splits into at least two words.
        let id = payload.split(' ').nth(1).unwrap_or_default();
        if id.len() > MAX_ID {
            // No sane conductor sends it: a protocol error, so the conductor ends now.
            fed.tokens.push(Token::Error(ID_TOO_LONG.into()));
            return Step::Unhook;
        }
        let id = id.to_string();
        self.state = State::Body(id.clone());
        fed.tokens.push(Token::Begin(id));
        Step::Continue
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parser_split_follows_iterm2() {
        let begin = Parser::hooked("d").feed(b"\x1b]135;:begin 1\x1b\\");
        assert_eq!(begin, Fed { tokens: vec![Token::Begin("1".into())], rest: None });
        assert_eq!(Parser::hooked("d").dcs_id(), "d");
        let unhook = Parser::hooked("d").feed(b"\x1b]135;:unhook\x1b\\$ ");
        assert_eq!(unhook, Fed { tokens: vec![Token::Unhook], rest: Some(b"$ ".to_vec()) });
        let init = Parser::new("d").feed(b"a b YQ== - Yg==\n");
        assert_eq!(init.rest, None);
        assert_eq!(
            init.tokens,
            vec![Token::Init(Init { token: "a".into(), unique_id: "b".into(), bool_args: "a".into(), sshargs: "b".into(), dcs_id: "d".into() })]
        );
        assert_eq!(next_osc(b"\x1b]135;:x\x1b", false), (Osc::Blocked, 0));
        assert_eq!(next_osc(b"zz\x1b]135;:x\x1b\\", true), (Osc::Osc(135, "x".into()), 12));
        assert_eq!(next_osc(b"zz", false), (Osc::NotOsc, 0));
        // Blocked garbage skipping keeps the OSC that may be starting, not the garbage.
        assert_eq!(next_osc(b"zz\x1b]13", true), (Osc::Blocked, 2));
        assert_eq!(next_osc(b"\x1b];x\x1b\\", false), (Osc::NotOsc, 0), "an OSC needs a code");
        // An interrupted OSC blocked mid-way keeps the new one from its ESC.
        assert_eq!(next_osc(b"\x1b]135;:a\x1b]135;:b", true), (Osc::Blocked, 8));
        assert_eq!(next_osc(b"\x1b]135;:a\x1b\x1b]135;:b\x1b\\", true), (Osc::Osc(135, "b".into()), 19));
    }

    #[test]
    fn a_long_begin_id_ends_the_conductor() {
        let id = "7".repeat(MAX_ID);
        let mut p = Parser::hooked("d");
        let fed = p.feed(format!("\x1b]135;:begin {id}\x1b\\\x1b]135;:x\x1b\\\x1b]135;:end {id} 0 r\x1b\\").as_bytes());
        assert_eq!(fed.tokens, [Token::Begin(id.clone()), Token::Line("x".into()), Token::End { id, status: 0, kind: "r".into() }]);
        assert_eq!(fed.rest, None);

        // One byte more: a protocol error, and what follows is output again.
        let long = "7".repeat(MAX_ID + 1);
        let fed = Parser::hooked("d").feed(format!("\x1b]135;:begin {long}\x1b\\$ ").as_bytes());
        assert_eq!(fed.tokens, [Token::Error(ID_TOO_LONG.into())]);
        assert_eq!(fed.rest.as_deref(), Some(&b"$ "[..]));
        let fed = Parser::hooked("d").feed(format!("\x1b]134;:begin {long}\x1b\\").as_bytes());
        assert_eq!(fed.tokens, [Token::Error(ID_TOO_LONG.into())], "the framer's begin too");

        // A lookalike of the end is a line: the id must match in full, then a space.
        let mut p = Parser::hooked("d");
        let fed = p.feed(b"\x1b]135;:begin 12\x1b\\\x1b]135;:end 1 0 r\x1b\\\x1b]135;:end 123 0 r\x1b\\\x1b]135;:end 12\x1b\\");
        assert_eq!(fed.tokens[1..], [Token::Line("end 1 0 r".into()), Token::Line("end 123 0 r".into()), Token::Line("end 12".into())]);
    }

    #[test]
    fn reply_lines_are_linear_in_their_bytes() {
        // A hostile remote's stream of tiny lines under the longest id: each line is matched
        // against the id in place, so 4 MiB of lines in 64 KiB reads takes well under a second
        // (a megabyte-long id copied twice per line took minutes).
        let mut p = Parser::hooked("d");
        let id = "9".repeat(MAX_ID);
        assert_eq!(p.feed(format!("\x1b]135;:begin {id}\x1b\\").as_bytes()).tokens.len(), 1);
        let line = b"\x1b]135;:a\x1b\\";
        let read: Vec<u8> = line.iter().copied().cycle().take(line.len() * (64 * 1024 / line.len())).collect();
        let started = std::time::Instant::now();
        let mut lines = 0;
        for _ in 0..64 {
            let fed = p.feed(&read);
            assert_eq!(fed.rest, None);
            lines += fed.tokens.len();
        }
        assert_eq!(lines, 64 * (64 * 1024 / line.len()));
        assert!(started.elapsed() < std::time::Duration::from_secs(10), "took {:?}", started.elapsed());
    }
}
