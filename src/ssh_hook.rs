//! SSH integration's entry (iTerm2's DCS 2000p hook): finds `ESC P 2000 p` in a pane's output
//! (across reads) and hands everything after it to the conductor parser until it unhooks.
//! Bytes before the start and after the unhook go to the screen.

use crate::conductor_parser::{Parser, Token};

pub const START: &[u8] = b"\x1bP2000p";

/// The hook's name in iTerm2's DCS parser.
pub const HOOK_DESCRIPTION: &str = "[SSH CONDUCTOR]";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Out {
    /// Ordinary output for the pane's screen.
    Screen(Vec<u8>),
    /// A conductor token and its depth (0 before the framer).
    Ssh(Token, i32),
}

/// A fresh parser id (iTerm2 gives each DCS 2000p hook a new UUID).
pub fn new_dcs_id() -> String {
    uuid::Uuid::new_v4().hyphenated().to_string().to_uppercase()
}

#[derive(Default)]
pub struct Hook {
    /// The end of the last read that may begin `ESC P 2000 p`.
    held: Vec<u8>,
    parser: Option<Parser>,
    /// Where parser ids come from (None: `new_dcs_id`).
    ids: Option<fn() -> String>,
}

impl Hook {
    /// A hook whose parsers take their ids from `next` (tests).
    pub fn with_ids(next: fn() -> String) -> Hook {
        Hook { ids: Some(next), ..Default::default() }
    }

    pub fn active(&self) -> bool {
        self.parser.is_some()
    }

    /// Leaves the conductor without an `unhook` (iTerm2's forced unhook): what follows is
    /// output again.
    pub fn unhook(&mut self) {
        self.parser = None;
    }

    pub fn feed(&mut self, bytes: &[u8]) -> Vec<Out> {
        let mut data = std::mem::take(&mut self.held);
        data.extend_from_slice(bytes);
        let mut out = Vec::new();
        loop {
            if let Some(parser) = &mut self.parser {
                let fed = parser.feed(&data);
                out.extend(fed.tokens.into_iter().map(|t| Out::Ssh(t, 0)));
                match fed.rest {
                    None => break,
                    Some(rest) => {
                        self.parser = None;
                        data = rest;
                    }
                }
                continue;
            }
            match data.windows(START.len()).position(|w| w == START) {
                Some(k) => {
                    screen(&mut out, &data[..k]);
                    let id = self.ids.map_or_else(new_dcs_id, |next| next());
                    self.parser = Some(Parser::new(&id));
                    data.drain(..k + START.len());
                }
                None => {
                    // A tail that may begin the start sequence waits for the next read.
                    let keep = (1..START.len()).rev().find(|n| data.ends_with(&START[..*n])).unwrap_or(0);
                    screen(&mut out, &data[..data.len() - keep]);
                    self.held = data[data.len() - keep..].to_vec();
                    break;
                }
            }
        }
        out
    }
}

/// Adds screen bytes, joined to screen bytes just before them.
fn screen(out: &mut Vec<Out>, bytes: &[u8]) {
    if bytes.is_empty() {
        return;
    }
    if let Some(Out::Screen(last)) = out.last_mut() {
        last.extend_from_slice(bytes);
    } else {
        out.push(Out::Screen(bytes.to_vec()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conductor_parser::{parse_end, parse_init, Init};

    fn hook() -> Hook {
        Hook::with_ids(|| "D".into())
    }

    fn s(b: &[u8]) -> Out {
        Out::Screen(b.to_vec())
    }

    fn ssh(t: Token) -> Out {
        Out::Ssh(t, 0)
    }

    fn begin(id: &str) -> Out {
        ssh(Token::Begin(id.into()))
    }

    fn line(text: &str) -> Out {
        ssh(Token::Line(text.into()))
    }

    fn end(id: &str, status: u8, kind: &str) -> Out {
        ssh(Token::End { id: id.into(), status, kind: kind.into() })
    }

    fn init(token: &str, unique_id: &str, bool_args: &str, sshargs: &str) -> Out {
        ssh(Token::Init(Init {
            token: token.into(),
            unique_id: unique_id.into(),
            bool_args: bool_args.into(),
            sshargs: sshargs.into(),
            dcs_id: "D".into(),
        }))
    }

    const INIT: &[u8] = b"\x1bP2000pnone 1 YQ== - Yg==\n";

    /// A hook past its init line, in ground.
    fn hooked() -> Hook {
        let mut h = hook();
        assert_eq!(h.feed(INIT), vec![init("none", "1", "a", "b")]);
        h
    }

    /// A hook inside the reply to command 1.
    fn in_body() -> Hook {
        let mut h = hooked();
        assert_eq!(h.feed(b"\x1b]135;:begin 1\x1b\\"), vec![begin("1")]);
        h
    }

    #[test]
    fn hook_finds_the_start_follows_iterm2() {
        let mut h = hook();
        let out = h.feed(b"hi\x1bP2000pnone 12345 YmE= - c3NoIGFyZ3M=\n");
        assert_eq!(out, vec![s(b"hi"), init("none", "12345", "ba", "ssh args")]);
        assert!(h.active());
        assert_eq!(hook().feed(b"x\x1bP1000py"), vec![s(b"x\x1bP1000py")], "tmux's DCS is output here");
        assert_eq!(hook().feed(b"\x1bP2001p"), vec![s(b"\x1bP2001p")], "another DCS is output");
        assert_eq!(HOOK_DESCRIPTION, "[SSH CONDUCTOR]");
        let id = new_dcs_id();
        assert_eq!((id.len(), id.to_uppercase()), (36, id.clone()));
        assert_ne!(new_dcs_id(), new_dcs_id());
    }

    #[test]
    fn hook_survives_chunk_splits() {
        let mut h = hook();
        assert_eq!(h.feed(b"ab\x1bP20"), vec![s(b"ab")], "a possible start waits");
        assert_eq!(h.feed(b"00pnone 1 YQ== - Yg==\n"), vec![init("none", "1", "a", "b")]);
        let mut h = hook();
        assert_eq!(h.feed(b"\x1bP2"), vec![]);
        assert_eq!(h.feed(b"x"), vec![s(b"\x1bP2x")], "a false start goes to the screen whole");
        let mut h = hook();
        assert_eq!(h.feed(b"\x1bP2000pnone 1 YQ=="), vec![]);
        assert_eq!(h.feed(b" - Yg=="), vec![], "the init line waits for its newline");
        assert_eq!(h.feed(b"\n"), vec![init("none", "1", "a", "b")]);
        assert_eq!(h.feed(b"\x1b]135;:begin 1\x1b"), vec![]);
        assert_eq!(h.feed(b"\\"), vec![begin("1")]);
        assert_eq!(h.feed(b"\x1b]135;:first\nsec"), vec![]);
        assert_eq!(h.feed(b"ond\x1b"), vec![]);
        assert_eq!(h.feed(b"\\"), vec![line("first\nsecond")]);
        assert_eq!(h.feed(b"\x1b]135;:end 1 0 r\x1b\\\x1b]135;:unh"), vec![end("1", 0, "r")]);
        assert_eq!(h.feed(b"ook\x1b\\\x1bP20"), vec![ssh(Token::Unhook)]);
        assert_eq!(h.feed(b"0"), vec![]);
        assert_eq!(h.feed(b"1"), vec![s(b"\x1bP2001")], "after the unhook the finder runs again");
    }

    #[test]
    fn init_rules_follow_iterm2() {
        let got = parse_init("t u YmE= x y - c3M=", "D").unwrap();
        assert_eq!(got, Init { token: "t".into(), unique_id: "u".into(), bool_args: "ba".into(), sshargs: "ss".into(), dcs_id: "D".into() });
        assert_eq!(parse_init("t u -", "D"), None, "fewer than 5 words with the id");
        assert_eq!(parse_init("t u YmE= c3M=", "D"), None, "no separator");
        assert_eq!(parse_init("t u Y*E= - c3M=", "D"), None, "bad boolean args");
        assert_eq!(parse_init("t u YmE= - c3M", "D"), None, "bad ssh args");
        assert_eq!(parse_init("t u YmE= -", "D").unwrap().sshargs, "", "the id counts as a word");
        assert_eq!(parse_init("none 1 YmE= - c3M=", "D").unwrap().token, "none");
        let mut h = hook();
        assert_eq!(h.feed(b"\x1bP2000p\xff\xfe - Yg==\nls"), vec![s(b"\nls")], "a non-UTF-8 line is dropped");
        assert!(!h.active());
        let mut h = hook();
        assert_eq!(h.feed(b"\x1bP2000pbad\nls\r\n"), vec![s(b"\nls\r\n")], "a rejected line unhooks");
        assert!(!h.active());
    }

    #[test]
    fn osc_reader_follows_iterm2() {
        assert_eq!(hooked().feed(b"\x1b]135;:begin 7\x1b\\"), vec![begin("7")]);
        assert_eq!(hooked().feed(b"\x1b]135;junk:begin 7\x1b\\"), vec![begin("7")], "text before the colon is ignored");
        let mut h = hooked();
        assert_eq!(h.feed(b"\x1b]135;begin 7\x1b\\"), vec![], "no colon: an empty payload unhooks silently");
        assert!(!h.active());
        assert_eq!(hooked().feed(b"\x1b]135;:begin 7\x07\x1b\\"), vec![begin("7\u{7}")], "BEL does not end an OSC");
        assert_eq!(hooked().feed(b"\x1b]135;:beg\x1b]135;:begin 9\x1b\\"), vec![begin("9")], "an interrupted OSC restarts");
        let mut h = hooked();
        assert_eq!(h.feed(b"\n\x1b[31mjunk\x1b]7;:x\x1b\\\x1b]135;:begin 1\x1b\\"), vec![begin("1")], "ground drops garbage");
        assert!(h.active());
    }

    #[test]
    fn body_follows_iterm2() {
        let mut h = in_body();
        assert_eq!(h.feed(b"\x1b]135;:a\nb\x1b\\"), vec![line("a\nb")]);
        assert_eq!(h.feed(b"\x1b]135;:end 2 0 r\x1b\\"), vec![line("end 2 0 r")], "another id is a line");
        assert_eq!(h.feed(b"\x1b]134;:end 1 0 r\x1b\\"), vec![end("1", 0, "r")]);
        assert_eq!(h.feed(b"\x1b]135;:begin 2\x1b\\"), vec![begin("2")], "ground again");
        let mut h = in_body();
        assert_eq!(h.feed(b"oops"), vec![s(b"oops")]);
        assert!(!h.active());
        let mut h = in_body();
        assert_eq!(h.feed(b"\x1b]7;:x\x1b\\"), vec![s(b"\x1b]7;:x\x1b\\")], "another OSC is output");
        assert!(!h.active());
    }

    #[test]
    fn end_rules_follow_iterm2() {
        assert_eq!(parse_end("1 0 r"), Some(("1".into(), 0, "r".into())));
        assert_eq!(parse_end("1 r 0"), Some(("1".into(), 0, "0".into())), "the python bootstrap's swapped fields");
        assert_eq!(parse_end("1 07 r"), Some(("1".into(), 7, "r".into())));
        assert_eq!(parse_end("1 7x r"), Some(("1".into(), 7, "r".into())));
        assert_eq!(parse_end("1 256 r"), None);
        assert_eq!(parse_end("1 99999999999999999999999 r"), None);
        assert_eq!(parse_end("1 0"), None);
        let mut h = in_body();
        assert_eq!(h.feed(b"\x1b]135;:end 1 256 r\x1b\\"), vec![]);
        assert_eq!(h.feed(b"\x1b]135;:begin 2\x1b\\"), vec![begin("2")], "a dropped end still returns to ground");
    }

    #[test]
    fn unhook_follows_iterm2() {
        let mut h = hooked();
        assert_eq!(h.feed(b"\x1b]135;:unhook\x1b\\$ "), vec![ssh(Token::Unhook), s(b"$ ")]);
        assert!(!h.active());
        let mut h = hooked();
        assert_eq!(h.feed(b"\x1b]135;:what\x1b\\x"), vec![s(b"x")], "an unknown word unhooks silently");
        assert!(!h.active());
        let mut h = hooked();
        let out = h.feed(b"\x1b]135;:unhook\x1b\\x\x1bP2000pnone 1 YQ== - Yg==\n");
        assert_eq!(out, vec![ssh(Token::Unhook), s(b"x"), init("none", "1", "a", "b")]);
        assert!(h.active());
        let mut h = in_body();
        h.unhook();
        assert!(!h.active());
        assert_eq!(h.feed(b"\x1b]135;:end 1 0 r\x1b\\$ "), vec![s(b"\x1b]135;:end 1 0 r\x1b\\$ ")]);
    }

    #[test]
    fn framer_ground_follows_iterm2() {
        assert_eq!(hooked().feed(b"\x1b]134;:begin 5\x1b\\"), vec![begin("5")]);
        let mut h = hooked();
        assert_eq!(h.feed(b"\x1b]134;:%ping\x1b\\"), vec![]);
        assert!(h.active(), "an unknown notification is ignored");
        let mut h = hooked();
        assert_eq!(h.feed(b"\x1b]134;:nonsense\x1b\\z"), vec![s(b"z")]);
        assert!(!h.active());
    }

    #[test]
    fn payloads_decode_as_utf8() {
        assert_eq!(in_body().feed(b"\x1b]135;:caf\xc3\xa9\x1b\\"), vec![line("caf\u{e9}")]);
        assert_eq!(in_body().feed(b"\x1b]135;:caf\xff\x1b\\"), vec![line("caf\u{fffd}")]);
        // A character split across reads is whole.
        let mut h = in_body();
        assert_eq!(h.feed(b"\x1b]135;:caf\xc3"), vec![]);
        assert_eq!(h.feed(b"\xa9\x1b\\"), vec![line("caf\u{e9}")]);
    }

    /// Feeds a recording in chunks, joining screen output across reads.
    fn run(bytes: &[u8], size: usize) -> Vec<Out> {
        let mut h = hook();
        let mut out = Vec::new();
        for chunk in bytes.chunks(size) {
            for o in h.feed(chunk) {
                match o {
                    Out::Screen(b) => screen(&mut out, &b),
                    o => out.push(o),
                }
            }
        }
        out
    }

    fn tokens(out: &[Out]) -> Vec<&Token> {
        out.iter().filter_map(|o| if let Out::Ssh(t, _) = o { Some(t) } else { None }).collect()
    }

    fn lines<'a>(out: &'a [Out]) -> Vec<&'a str> {
        tokens(out).into_iter().filter_map(|t| if let Token::Line(l) = t { Some(l.as_str()) } else { None }).collect()
    }

    #[test]
    fn recordings_parse() {
        let mut all = Vec::new();
        for name in ["level1-nopython", "python36", "python312", "quirks"] {
            let bytes = std::fs::read(format!("tests/fixtures/ssh/{name}.bin")).unwrap();
            let whole = run(&bytes, bytes.len());
            for size in [1, 2, 7, 64] {
                assert_eq!(run(&bytes, size), whole, "{name} in chunks of {size}");
            }
            all.push(whole);
        }
        // level1-nopython: getshell, setenv, write, cd, pythonversion, exec_login_shell.
        let out = &all[0];
        let t = tokens(out);
        assert_eq!(
            t[0],
            &Token::Init(Init {
                token: "none".into(),
                unique_id: "12345".into(),
                bool_args: "46AaCfGgKkMNnqsTtVvXxYy".into(),
                sshargs: "example.com ".into(),
                dcs_id: "D".into(),
            })
        );
        // Six commands, each a begin and an end with the same id.
        let mut open = None;
        let mut pairs = 0;
        for token in &t[1..] {
            match token {
                Token::Begin(id) => open = Some(id),
                Token::End { id, status: 0, kind } if Some(id) == open && kind == "r" => pairs += 1,
                _ => {}
            }
        }
        assert_eq!(pairs, 6);
        assert_eq!(t.iter().filter(|t| matches!(t, Token::Begin(_))).count(), 6);
        assert!(matches!(t[1], Token::Begin(_)));
        // With no getent, id, python or perl, conductor.sh calls its undefined using_passwd
        // and the shell's complaint lands inside getshell's reply, before the shell.
        let getshell: Vec<&str> = lines(out)[0].split('\n').collect();
        assert!(getshell[0].ends_with("using_passwd: not found"), "{getshell:?}");
        assert_eq!(getshell[1..], ["/bin/sh", "/home/user", ""]);
        assert!(matches!(t[2], Token::Line(_)) && matches!(t[3], Token::End { .. }));
        assert_eq!(t[t.len() - 1], &Token::Unhook);
        assert!(matches!(out.last(), Some(Out::Screen(b)) if String::from_utf8_lossy(b).contains("hi\r\n")));
        assert!(lines(&all[1]).iter().any(|l| l.starts_with("Python 3.6.9")));
        let t = tokens(&all[2]);
        assert!(lines(&all[2]).iter().any(|l| l.starts_with("Python 3.12.1")));
        assert!(matches!(t[t.len() - 2..], [Token::Begin(_), Token::End { .. }]), "ends with the quit pair");
        let t = tokens(&all[3]);
        assert!(lines(&all[3]).contains(&"bad command nosuch"));
        let ends: Vec<u8> = t.iter().filter_map(|t| if let Token::End { status, .. } = t { Some(*status) } else { None }).collect();
        assert_eq!(ends, [0, 0, 0, 0], "every status is 0 (iTerm2 bug B1)");
    }
}
