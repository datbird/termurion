//! iTerm2's VT100ConductorParser for the conductor.sh stage (milestone 14b): the init line
//! after `ESC P 2000 p`, then OSC 134/135 replies in ground and body. Pure: bytes in, tokens
//! out; unparsed input waits inside the parser for the next read. Milestone 14c adds the
//! framer's payload rules and output states to these types.

use base64::Engine;

const ESC: u8 = 0x1b;

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
    enum S {
        Ground,
        Esc,
        Code(Vec<u8>),
        Payload(u32, Vec<u8>),
        PayloadEsc(u32, Vec<u8>),
    }
    let mut state = S::Ground;
    // Where the OSC being read began: blocked input is kept from here.
    let mut checkpoint = 0;
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        i += 1;
        state = match state {
            S::Ground if c == ESC => S::Esc,
            S::Esc if c == b']' => S::Code(Vec::new()),
            S::Code(mut code) if c.is_ascii_digit() => {
                code.push(c);
                S::Code(code)
            }
            S::Code(code) if c == b';' && number(&code).is_some() => S::Payload(number(&code).unwrap_or_default(), Vec::new()),
            S::Payload(code, payload) if c == ESC => S::PayloadEsc(code, payload),
            S::Payload(code, mut payload) => {
                payload.push(c);
                S::Payload(code, payload)
            }
            S::PayloadEsc(code, payload) if c == b'\\' => {
                // Unrecognized parameters before the first colon are ignored, and the colon.
                let text = match payload.iter().position(|b| *b == b':') {
                    Some(k) => String::from_utf8_lossy(&payload[k + 1..]).into_owned(),
                    None => String::new(),
                };
                return (Osc::Osc(code, text), i);
            }
            S::PayloadEsc(..) => {
                // ESC then anything but a backslash: another OSC began inside this one.
                if !skip_garbage {
                    return (Osc::NotOsc, checkpoint);
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
                    return (Osc::NotOsc, checkpoint);
                }
                checkpoint = i;
                S::Ground
            }
        };
    }
    (Osc::Blocked, checkpoint)
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
    /// Input not parsed yet: an unfinished init line or OSC.
    held: Vec<u8>,
}

enum Step {
    Continue,
    Unhook,
}

impl Parser {
    pub fn new(dcs_id: &str) -> Parser {
        Parser { dcs_id: dcs_id.to_string(), state: State::Initial, held: Vec::new() }
    }

    /// A parser whose init line was already read.
    pub fn hooked(dcs_id: &str) -> Parser {
        Parser { state: State::Ground, ..Parser::new(dcs_id) }
    }

    pub fn dcs_id(&self) -> &str {
        &self.dcs_id
    }

    pub fn feed(&mut self, bytes: &[u8]) -> Fed {
        let mut data = std::mem::take(&mut self.held);
        data.extend_from_slice(bytes);
        let mut fed = Fed::default();
        let mut i = 0;
        loop {
            let rest = &data[i..];
            match self.state.clone() {
                State::Initial => {
                    let Some(k) = rest.iter().position(|b| *b == b'\n') else {
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
                    let (osc, n) = next_osc(rest, true);
                    i += n;
                    let step = match osc {
                        Osc::Osc(134, payload) => self.framer_payload(&payload, &mut fed),
                        Osc::Osc(135, payload) => self.pre_framer_payload(&payload, &mut fed),
                        // Other OSCs are ignored.
                        Osc::Osc(..) => Step::Continue,
                        // Garbage skipping never gives NotOsc.
                        Osc::Blocked | Osc::NotOsc => break,
                    };
                    if let Step::Unhook = step {
                        return self.unhook(fed, &data[i..]);
                    }
                }
                State::Body(id) => {
                    if rest.is_empty() {
                        break;
                    }
                    match next_osc(rest, false) {
                        (Osc::Osc(134 | 135, payload), n) => {
                            i += n;
                            match payload.strip_prefix(&format!("end {id} ")) {
                                Some(tail) => {
                                    self.state = State::Ground;
                                    if let Some((id, status, kind)) = parse_end(&format!("{id} {tail}")) {
                                        fed.tokens.push(Token::End { id, status, kind });
                                    }
                                }
                                None => fed.tokens.push(Token::Line(payload)),
                            }
                        }
                        (Osc::Blocked, _) => break,
                        // Anything else is output: the bytes from here on.
                        _ => return self.unhook(fed, rest),
                    }
                }
            }
        }
        self.held = data[i..].to_vec();
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
        let id = payload.split(' ').nth(1).unwrap_or_default().to_string();
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
}
