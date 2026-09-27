//! The utilities' dispatch (milestone 11e): the names `tandem-utils.exe` answers to, the
//! choice of utility from its own file name or its first argument, the `Io` context every
//! utility takes (streams, the console as a `Tty`, environment, working folder, a random
//! source and a command runner), the console input modes the scripts' `stty` calls stand
//! for, and the UTF-16 key decoding the console reader uses. Pure; `util_console` is the
//! real console.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::time::Duration;

/// iTerm2's bundled utilities that Tandem ports, sorted (it2ssh is milestone 14; the Swift
/// `it2` is not ported).
pub const NAMES: [&str; 16] = [
    "imgcat",
    "imgls",
    "it2api",
    "it2attention",
    "it2cat",
    "it2check",
    "it2copy",
    "it2dl",
    "it2getvar",
    "it2git",
    "it2profile",
    "it2setcolor",
    "it2setkeylabel",
    "it2tip",
    "it2ul",
    "it2universion",
];

/// The multi-call program's own file stem.
pub const EXE_STEM: &str = "tandem-utils";

/// The utility a program path names: its file stem (after the last `\` or `/`, ".exe"
/// removed), compared without case.
pub fn name_of(argv0: &str) -> Option<&'static str> {
    let file = argv0.rsplit(['\\', '/']).next().unwrap_or(argv0);
    let lower = file.to_ascii_lowercase();
    let stem = lower.strip_suffix(".exe").unwrap_or(&lower);
    NAMES.iter().copied().find(|n| *n == stem)
}

/// The utility to run and its arguments: the one the program's own name names, else (run
/// as `tandem-utils`) the one its first argument names. Err is the usage text.
pub fn pick(args: &[String]) -> Result<(&'static str, Vec<String>), String> {
    if let Some(name) = args.first().and_then(|a| name_of(a)) {
        return Ok((name, args[1..].to_vec()));
    }
    match args.get(1).and_then(|a| name_of(a)) {
        Some(name) => Ok((name, args[2..].to_vec())),
        None => Err(multi_usage()),
    }
}

/// What `tandem-utils` run by itself prints.
pub fn multi_usage() -> String {
    format!("Usage: {EXE_STEM} <utility> [arguments...]\nUtilities: {}\n", NAMES.join(", "))
}

/// The console input mode a script's `stty` stands for: `Raw` is `stty -echo -icanon raw`
/// (it2check), `Cbreak` is `stty -echo` plus bash's character-mode `read -d` (it2getvar,
/// it2profile, it2ul), which keeps signals.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Raw,
    Cbreak,
}

const ENABLE_PROCESSED_INPUT: u32 = 0x1;
const ENABLE_LINE_INPUT: u32 = 0x2;
const ENABLE_ECHO_INPUT: u32 = 0x4;
const ENABLE_VIRTUAL_TERMINAL_INPUT: u32 = 0x200;

/// The console input mode for `m`, from the mode the console had: VT input on (replies
/// arrive as typed), line input and echo off, processed input (^C as a signal) kept only in
/// `Cbreak`; every other bit kept.
pub fn input_mode(m: Mode, current: u32) -> u32 {
    let off = match m {
        Mode::Raw => ENABLE_LINE_INPUT | ENABLE_ECHO_INPUT | ENABLE_PROCESSED_INPUT,
        Mode::Cbreak => ENABLE_LINE_INPUT | ENABLE_ECHO_INPUT,
    };
    (current | ENABLE_VIRTUAL_TERMINAL_INPUT) & !off
}

/// `/dev/tty`: the console the utility talks to directly.
pub trait Tty {
    fn write(&mut self, bytes: &[u8]);
    /// The next byte typed (or replied), waiting at most `wait` (None: without end). None
    /// when the wait ran out or there is no console.
    fn read(&mut self, wait: Option<Duration>) -> Option<u8>;
    fn set_mode(&mut self, m: Mode);
    /// Drops input not read yet.
    fn discard_input(&mut self);
    /// Puts back every mode `set_mode` changed.
    fn restore(&mut self);
    fn is_console(&self) -> bool;
}

/// A program a utility runs where its script runs one (curl, tar, git, python).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cmd {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub stdin: Option<Vec<u8>>,
    /// Standard streams shared with the utility; the output is not captured.
    pub inherit: bool,
}

/// A finished program: its exit code and captured standard output.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Output {
    pub code: i32,
    pub stdout: Vec<u8>,
}

/// Everything a utility touches outside itself.
pub struct Io<'a> {
    pub out: &'a mut dyn Write,
    pub err: &'a mut dyn Write,
    pub input: &'a mut dyn Read,
    /// `[ -t 0 ]` and `[ -t 1 ]`.
    pub stdin_tty: bool,
    pub stdout_tty: bool,
    pub tty: &'a mut dyn Tty,
    pub env: &'a dyn Fn(&str) -> Option<String>,
    pub cwd: PathBuf,
    /// `$RANDOM`: 0 to 32767.
    pub random: &'a mut dyn FnMut() -> u32,
    pub run: &'a mut dyn FnMut(&Cmd) -> Result<Output, String>,
    /// `builtin command -V <name>` succeeds (the scripts' `check_dependency`).
    pub have: &'a dyn Fn(&str) -> bool,
}

/// `$TERM`.
pub fn term(io: &Io) -> Option<String> {
    (io.env)("TERM")
}

/// Runs utility `name` with `args`; returns its exit code. Each utility's task adds its arm
/// here; a name not built yet prints the usage. A utility returns its exit code and never
/// calls `std::process::exit`: the console's restore (`Drop`, then `real_main`'s restore of
/// every saved mode) runs only when it returns.
pub fn run(name: &str, args: &[String], io: &mut Io) -> i32 {
    use crate::util_files;
    match name {
        "imgcat" => util_files::imgcat(args, io),
        "it2cat" => util_files::it2cat(args, io),
        "it2dl" => util_files::it2dl(args, io),
        _ => {
            let _ = io.err.write_all(multi_usage().as_bytes());
            2
        }
    }
}

/// What a console focus-in record reads as while a reply is awaited: ConPTY's input parser
/// takes a reply's `ESC [ I` (as in `ESC [ ITERM2 ...`) for a focus report.
pub const FOCUS_IN: &[u8] = b"\x1b[I";

/// How Tandem's `CSI 1337 n` answer starts once ConPTY has swallowed its `ESC [ I`.
pub const SWALLOWED_REPLY: &[u8] = b"TERM2 ";

/// The shipped ConPTY asks the terminal for focus reports in its own start-up handshake and
/// takes every `ESC [ I` in its input for one, so `ESC [ ITERM2 3.7.0 n` reaches the program
/// as `TERM2 3.7.0 n` (measured, `conpty_carries_replies`). After `CSI 1337 n`, a reply
/// starting "TERM2 " gets its `ESC [ I` back; anything else is unchanged. it2check's reader
/// applies this on Windows; the version bytes are still compared by the script's rule.
pub fn restore_reply_prefix(reply: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(reply.len() + FOCUS_IN.len());
    if reply.starts_with(SWALLOWED_REPLY) {
        out.extend_from_slice(FOCUS_IN);
    }
    out.extend_from_slice(reply);
    out
}

/// Console key characters (UTF-16 units) to UTF-8: a surrogate pair is joined, a lone
/// surrogate is U+FFFD.
#[derive(Debug, Default)]
pub struct KeyText {
    high: Option<u16>,
}

impl KeyText {
    pub fn push(&mut self, unit: u16, out: &mut Vec<u8>) {
        let mut put = |c: char| {
            let mut b = [0u8; 4];
            out.extend_from_slice(c.encode_utf8(&mut b).as_bytes());
        };
        match unit {
            0xD800..=0xDBFF => {
                if self.high.replace(unit).is_some() {
                    put('\u{FFFD}');
                }
            }
            0xDC00..=0xDFFF => match self.high.take() {
                Some(h) => {
                    let c = 0x10000 + ((u32::from(h) - 0xD800) << 10) + (u32::from(unit) - 0xDC00);
                    put(char::from_u32(c).unwrap_or('\u{FFFD}'));
                }
                None => put('\u{FFFD}'),
            },
            _ => {
                if self.high.take().is_some() {
                    put('\u{FFFD}');
                }
                put(char::from_u32(u32::from(unit)).unwrap_or('\u{FFFD}'));
            }
        }
    }

    /// A high surrogate still waiting for its pair (input ended) becomes U+FFFD.
    pub fn flush(&mut self, out: &mut Vec<u8>) {
        if self.high.take().is_some() {
            out.extend_from_slice("\u{FFFD}".as_bytes());
        }
    }
}

/// A console for tests: records what is written and which modes were set, and replays
/// `replies` (None: a wait that ran out).
#[cfg(test)]
#[derive(Debug, Default)]
pub(crate) struct FakeTty {
    pub written: Vec<u8>,
    pub replies: std::collections::VecDeque<Option<u8>>,
    pub modes: Vec<Mode>,
    pub discarded: bool,
    pub restored: bool,
    pub console: bool,
}

#[cfg(test)]
impl Tty for FakeTty {
    fn write(&mut self, bytes: &[u8]) {
        self.written.extend_from_slice(bytes);
    }
    fn read(&mut self, _wait: Option<Duration>) -> Option<u8> {
        self.replies.pop_front().flatten()
    }
    fn set_mode(&mut self, m: Mode) {
        self.modes.push(m);
    }
    fn discard_input(&mut self) {
        self.discarded = true;
    }
    fn restore(&mut self) {
        self.restored = true;
    }
    fn is_console(&self) -> bool {
        self.console
    }
}

/// A utility run in memory: its streams, environment, console, the programs it ran (each
/// answered from `answers`, else an error), the commands `have` reports missing, and the
/// `$RANDOM` values it gets.
#[cfg(test)]
#[derive(Default)]
pub(crate) struct Harness {
    pub out: Vec<u8>,
    pub err: Vec<u8>,
    pub input: Vec<u8>,
    pub stdin_tty: bool,
    pub stdout_tty: bool,
    pub env: std::collections::BTreeMap<String, String>,
    pub cwd: PathBuf,
    pub tty: FakeTty,
    pub runs: Vec<Cmd>,
    pub answers: std::collections::VecDeque<Result<Output, String>>,
    pub missing: Vec<String>,
    pub randoms: std::collections::VecDeque<u32>,
}

#[cfg(test)]
impl Harness {
    pub fn run(&mut self, name: &str, args: &[&str]) -> i32 {
        let Harness { out, err, input, stdin_tty, stdout_tty, env, cwd, tty, runs, answers, missing, randoms } = self;
        let mut input: &[u8] = input;
        let env_fn = |k: &str| env.get(k).cloned();
        let have = |p: &str| !missing.iter().any(|m| m == p);
        let mut random = || randoms.pop_front().expect("Harness: no $RANDOM value left");
        let mut run_fn = |c: &Cmd| {
            runs.push(c.clone());
            answers.pop_front().unwrap_or_else(|| Err(format!("Harness: no answer for {}", c.program)))
        };
        let mut io = Io {
            out,
            err,
            input: &mut input,
            stdin_tty: *stdin_tty,
            stdout_tty: *stdout_tty,
            tty,
            env: &env_fn,
            cwd: cwd.clone(),
            random: &mut random,
            run: &mut run_fn,
            have: &have,
        };
        let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
        run(name, &args, &mut io)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn names_follow_the_bundle() {
        let mut sorted = NAMES;
        sorted.sort();
        assert_eq!(sorted, NAMES);
        assert_eq!(NAMES.len(), 16);
        assert!(!NAMES.contains(&"it2ssh") && !NAMES.contains(&"it2"));
        assert_eq!(name_of("C:\\t\\utilities\\IMGCAT.EXE"), Some("imgcat"));
        assert_eq!(name_of("it2ul"), Some("it2ul"));
        assert_eq!(name_of("tandem-utils.exe"), None);
        assert_eq!(pick(&strings(&["C:\\t\\tandem-utils.exe", "it2check", "2.0"])), Ok(("it2check", strings(&["2.0"]))));
        assert_eq!(pick(&strings(&["imgcat.exe", "a.png"])), Ok(("imgcat", strings(&["a.png"]))));
        assert_eq!(pick(&strings(&["tandem-utils.exe"])), Err(multi_usage()));
        assert_eq!(pick(&strings(&["tandem-utils.exe", "it2ssh"])), Err(multi_usage()));
        assert!(multi_usage().starts_with("Usage: tandem-utils <utility> [arguments...]\nUtilities: imgcat, imgls, "));
        assert!(multi_usage().ends_with(", it2universion\n"));
    }

    #[test]
    fn unbuilt_utilities_print_the_usage() {
        let mut h = Harness::default();
        assert_eq!(h.run("no-such", &[]), 2);
        assert_eq!(String::from_utf8(h.err).unwrap(), multi_usage());
        assert!(h.out.is_empty());
    }

    #[test]
    fn input_modes_follow_stty() {
        assert_eq!(input_mode(Mode::Raw, 0x1F7), 0x3F0);
        assert_eq!(input_mode(Mode::Cbreak, 0x1F7), 0x3F1);
        for current in [0, 0x1F7, 0x200, 0x3FF, 0x7] {
            for m in [Mode::Raw, Mode::Cbreak] {
                assert_ne!(input_mode(m, current) & 0x200, 0, "{m:?} {current:#x}");
            }
        }
    }

    #[test]
    fn console_keys_become_bytes() {
        let mut k = KeyText::default();
        let mut out = Vec::new();
        k.push(0x41, &mut out);
        assert_eq!(out, b"A");
        out.clear();
        k.push(0xD83D, &mut out);
        assert!(out.is_empty(), "nothing after the first unit");
        k.push(0xDE00, &mut out);
        assert_eq!(out, "\u{1F600}".as_bytes());
        assert_eq!(out.len(), 4);
        out.clear();
        k.push(0xDE00, &mut out);
        assert_eq!(out, [0xEF, 0xBF, 0xBD]);
        out.clear();
        k.push(0xE9, &mut out);
        assert_eq!(out, [0xC3, 0xA9]);
        out.clear();
        k.push(0xD83D, &mut out);
        k.flush(&mut out);
        assert_eq!(out, [0xEF, 0xBF, 0xBD], "a trailing lone high surrogate");
        out.clear();
        k.flush(&mut out);
        assert!(out.is_empty(), "nothing left to flush");
    }

    #[test]
    fn swallowed_reply_prefix_is_restored() {
        assert_eq!(restore_reply_prefix(b"TERM2 3.7.0n"), b"\x1b[ITERM2 3.7.0n");
        assert_eq!(restore_reply_prefix(b"TERM2 3.7.0n\x1b[0n"), b"\x1b[ITERM2 3.7.0n\x1b[0n");
        for kept in [&b"\x1b[ITERM2 3.7.0n"[..], b"\x1b[0n", b"TERM2", b"", b"ITERM2 3.7.0n"] {
            assert_eq!(restore_reply_prefix(kept), kept, "{:?}", String::from_utf8_lossy(kept));
        }
    }
}
