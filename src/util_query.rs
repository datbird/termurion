//! it2check, it2getvar, it2profile and it2ul (milestone 11e): iTerm2's query utilities
//! (`OtherResources/Utilities/`), byte for byte. The scripts' tty and `/dev/tty` are
//! `Io.tty` (the console); pure otherwise.

use std::collections::VecDeque;
use std::io::Write;
use std::time::{Duration, Instant};

use crate::util_cli::{self, Cmd, Io, Mode, SWALLOWED_REPLY, Tty};
use crate::util_osc::{b64, seq, unb64};

fn put(w: &mut dyn Write, s: &str) {
    let _ = w.write_all(s.as_bytes());
}

/// The iTerm2 version it2check asks for when not given one (`it2check:63`).
pub const MIN_VERSION: &str = "2.9.20160304";

/// it2check's `read_dsr` (`it2check:17-27`): two bytes dropped, then bytes up to "n", as
/// `echo ${dsr}` prints them (each byte's newline lost to `$(...)`, words joined by one
/// space). Input ending ends the answer.
pub fn read_dsr(next: &mut dyn FnMut() -> Option<u8>) -> String {
    let mut dsr = Vec::new();
    for _ in 0..2 {
        if next().is_none() {
            return String::new();
        }
    }
    loop {
        match next() {
            None | Some(b'n') => break,
            Some(b'\n') => {}
            Some(b) => dsr.push(b),
        }
    }
    let text = String::from_utf8_lossy(&dsr).into_owned();
    text.split([' ', '\t', '\n']).filter(|w| !w.is_empty()).collect::<Vec<_>>().join(" ")
}

/// `sed -e 's/ .*//'`: the text before the first space.
pub fn terminal_of(s: &str) -> &str {
    s.split(' ').next().unwrap_or(s)
}

/// `sed -e 's/.* //'`: the text after the last space.
pub fn version_of(s: &str) -> &str {
    s.rsplit(' ').next().unwrap_or(s)
}

/// `test "$term" = ITERM2 -a \( "$version" \> "$MIN_VERSION" -o "$version" = "$MIN_VERSION" \)`
/// (`it2check:94`): `test` compares in byte order, so a bare "ITERM2" (its own version)
/// passes.
pub fn it2check_passes(version_string: &str, min: &str) -> bool {
    let version = version_of(version_string);
    terminal_of(version_string) == "ITERM2" && version.as_bytes() >= min.as_bytes()
}

/// The reply's bytes with ConPTY's swallowed `ESC [ I` put back (Task 1): the first bytes
/// are read only while they can still be "TERM2 ", so a terminal answering only DSR 5 is
/// never waited on for more.
fn reply_reader<'t>(tty: &'t mut dyn Tty) -> impl FnMut() -> Option<u8> + 't {
    let mut head: Option<VecDeque<u8>> = None;
    move || {
        let peeked = head.get_or_insert_with(|| {
            let mut seen = Vec::new();
            while seen.len() < SWALLOWED_REPLY.len() && SWALLOWED_REPLY.starts_with(&seen) {
                match tty.read(None) {
                    Some(b) => seen.push(b),
                    None => break,
                }
            }
            util_cli::restore_reply_prefix(&seen).into()
        });
        peeked.pop_front().or_else(|| tty.read(None))
    }
}

/// it2check (`U/it2check`): exit 1 unless stdin and stdout are the console; drains input,
/// raw mode, `CSI 1337 n` and `CSI 5 n`, then the script's reading and comparison.
pub fn it2check(args: &[String], io: &mut Io) -> i32 {
    if !io.stdin_tty || !io.stdout_tty || !io.tty.is_console() {
        return 1;
    }
    io.tty.discard_input();
    io.tty.set_mode(Mode::Raw);
    let min = if args.len() == 1 { args[0].as_str() } else { MIN_VERSION };
    io.tty.write(b"\x1b[1337n");
    io.tty.write(b"\x1b[5n");
    let passes = {
        let mut next = reply_reader(&mut *io.tty);
        let mut version_string = read_dsr(&mut next);
        if !version_string.is_empty() && version_string != "0" && version_string != "3" {
            // DSR 5's answer, thrown away.
            read_dsr(&mut next);
        } else {
            version_string.clear();
        }
        it2check_passes(&version_string, min)
    };
    io.tty.restore();
    if passes { 0 } else { 1 }
}

/// How long it2getvar and it2profile wait for the answer (`read -t 5`).
pub const VARIABLE_WAIT: Duration = Duration::from_secs(5);

/// `read -r -t <wait> -d $'\a'`: bytes up to a BEL (not kept), or what arrived before the
/// wait ran out.
pub fn read_to_bel(tty: &mut dyn Tty, wait: Duration, now: &dyn Fn() -> Instant) -> String {
    let start = now();
    let mut got = Vec::new();
    loop {
        let left = wait.saturating_sub(now().saturating_duration_since(start));
        if left.is_zero() {
            break;
        }
        match tty.read(Some(left)) {
            None | Some(0x07) => break,
            Some(b) => got.push(b),
        }
    }
    String::from_utf8_lossy(&got).into_owned()
}

/// The printed value of a ReportVariable answer (`echo "$(b64_decode <<< ${response#*=})"`):
/// None without "ReportVariable="; else the text after the first "=" decoded, its trailing
/// newlines dropped and one added.
pub fn variable_answer(response: &str) -> Option<String> {
    if !response.contains("ReportVariable=") {
        return None;
    }
    let (_, value) = response.split_once('=')?;
    let mut text = String::from_utf8_lossy(&unb64(value)).into_owned();
    while text.ends_with('\n') {
        text.pop();
    }
    text.push('\n');
    Some(text)
}

/// it2getvar's `show_help` (to stderr).
pub fn getvar_help() -> String {
    [
        "",
        "Usage: it2getvar variable_name",
        "",
        "Output value of the Tandem variable",
        "",
        "See the Variables Reference for information about built-in Tandem variables:",
        "  -> https://iterm2.com/documentation-variables.html",
        "",
    ]
    .iter()
    .map(|l| format!("{l}\n"))
    .collect()
}

/// it2profile's `show_help` (to stderr).
pub fn profile_help() -> String {
    [
        "",
        "Usage: it2profile -s profile_name",
        "       it2profile -g",
        "",
        "Change Tandem session profile on the fly",
        "",
        "Options:",
        "",
        "    -s      Set current Tandem session profile to specified profile name",
        "                empty string refers to profile marked as Default",
        "    -g      Get current Tandem session profile name",
        "    -r      Reset to the session initial profile",
        "",
    ]
    .iter()
    .map(|l| format!("{l}\n"))
    .collect()
}

/// `get_variable` / `get_profile`: echo off (cbreak), the request to the console, the
/// answer read for 5 s, echo back on, then the value or the script's error (exit 2).
fn report_variable(io: &mut Io, body: &str) -> i32 {
    io.tty.set_mode(Mode::Cbreak);
    let term = util_cli::term(io);
    io.tty.write(seq(term.as_deref(), body).as_bytes());
    let response = read_to_bel(&mut *io.tty, VARIABLE_WAIT, &Instant::now);
    io.tty.restore();
    match variable_answer(&response) {
        Some(value) => {
            put(io.out, &value);
            0
        }
        None => {
            put(io.err, "ERROR: Failed to read response from Tandem\n");
            2
        }
    }
}

/// it2getvar (`U/it2getvar:107-133`).
pub fn it2getvar(args: &[String], io: &mut Io) -> i32 {
    let Some(first) = args.first() else {
        put(io.err, &getvar_help());
        return 0;
    };
    match first.as_str() {
        "-h" | "--h" | "--help" => {
            put(io.err, &getvar_help());
            0
        }
        o if o.starts_with('-') => {
            put(io.err, &format!("ERROR: Unknown option: {o}\n{}", getvar_help()));
            1
        }
        "" => {
            put(io.err, "ERROR: Variable name can't be empty\n");
            1
        }
        name => report_variable(io, &format!("1337;ReportVariable={}", b64(name.as_bytes()))),
    }
}

/// it2profile (`U/it2profile:124-170`).
pub fn it2profile(args: &[String], io: &mut Io) -> i32 {
    let Some(first) = args.first() else {
        put(io.err, &profile_help());
        return 0;
    };
    let refuse = |io: &mut Io, error: &str| {
        put(io.err, &format!("ERROR: {error}\n{}", profile_help()));
        1
    };
    let set_profile = |io: &mut Io, name: &str| {
        let term = util_cli::term(io);
        io.tty.write(seq(term.as_deref(), &format!("1337;SetProfile={name}")).as_bytes());
        0
    };
    match first.as_str() {
        "-h" | "--h" | "--help" => {
            put(io.err, &profile_help());
            0
        }
        "-s" | "--s" | "--set" if args.len() == 2 => set_profile(io, &args[1]),
        "-s" | "--s" | "--set" => refuse(io, "-s option requires exactly 1 argument - profile name"),
        "-g" | "--g" | "--get" if args.len() == 1 => report_variable(io, "1337;ReportVariable=cHJvZmlsZU5hbWU="),
        "-g" | "--g" | "--get" => refuse(io, "-g option does not accept any argument"),
        "-r" | "--r" | "--reset" if args.len() == 1 => {
            let initial = (io.env)("ITERM_PROFILE").unwrap_or_default();
            set_profile(io, &initial)
        }
        "-r" | "--r" | "--reset" => refuse(io, "-r option does not accept any argument"),
        other => refuse(io, &format!("Unknown option: {other}")),
    }
}

/// it2ul's `send_request_for_upload` from `tar --version`'s output: its first line (`head
/// -1`, newline included) in base64, `$(...)` dropping only base64's own newline.
pub fn upload_request(term: Option<&str>, tar_version: &str) -> String {
    let first = tar_version.find('\n').map_or(tar_version, |i| &tar_version[..=i]);
    seq(term, &format!("1337;RequestUpload=format=tgz;version={}", b64(first.as_bytes())))
}

/// `read`'s line: bytes up to a CR or LF (each ends a line, as ICRNL makes a CR one); None
/// when input ends before anything was read.
pub fn read_line(next: &mut dyn FnMut() -> Option<u8>) -> Option<String> {
    let mut line = Vec::new();
    loop {
        match next() {
            Some(b'\r' | b'\n') => break,
            Some(b) => line.push(b),
            None if line.is_empty() => return None,
            None => break,
        }
    }
    Some(String::from_utf8_lossy(&line).into_owned())
}

/// `read`'s field splitting of one variable: IFS whitespace trimmed at both ends.
fn read_word(line: &str) -> &str {
    line.trim_matches([' ', '\t', '\n'])
}

/// `read_base64_stanza` (`it2ul:27-36`): lines joined until an empty one or the end of
/// input.
pub fn read_stanza(next: &mut dyn FnMut() -> Option<u8>) -> String {
    let mut data = String::new();
    while let Some(line) = read_line(next) {
        let line = read_word(&line);
        if line.is_empty() {
            break;
        }
        data.push_str(line);
    }
    data
}

/// it2ul (`U/it2ul`): echo off on a console stdin, the request on stdout, then the status
/// line and the data from stdin, extracted by `tar -x -z -C <location> -f - <flags>` with
/// tar's output on stderr; tar's code is the utility's.
pub fn it2ul(args: &[String], io: &mut Io) -> i32 {
    let location = args.first().cloned().unwrap_or_else(|| io.cwd.to_string_lossy().into_owned());
    // Tandem never lets a program open a network path for it (implementer rule).
    if !util_cli::may_open(&util_cli::join_local(&io.cwd, &location)) {
        put(io.err, &format!("Fatal error: {location}: network paths are not supported\n"));
        return 1;
    }
    // `$*` unquoted: the flags split into words.
    let flags: Vec<String> = args.iter().skip(1).flat_map(|a| a.split([' ', '\t', '\n'])).filter(|w| !w.is_empty()).map(str::to_string).collect();
    let console = io.stdin_tty;
    if console {
        io.tty.set_mode(Mode::Cbreak);
    }
    let version = (io.run)(&Cmd { program: "tar".into(), args: vec!["--version".into()], cwd: Some(io.cwd.clone()), stdin: None, inherit: false, quiet: false, env: Vec::new() });
    let version = version.map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
    let term = util_cli::term(io);
    put(io.out, &upload_request(term.as_deref(), &version));
    let _ = io.out.flush();

    let (status, data) = {
        let input = &mut *io.input;
        let tty = &mut *io.tty;
        let mut next = move || {
            if console {
                tty.read(None)
            } else {
                let mut b = [0u8];
                matches!(input.read(&mut b), Ok(1)).then_some(b[0])
            }
        };
        let status = read_line(&mut next).unwrap_or_default();
        let status = read_word(&status).to_string();
        let data = (status == "ok").then(|| read_stanza(&mut next));
        (status, data)
    };
    if console {
        io.tty.restore();
    }
    match (status.as_str(), data) {
        ("ok", Some(data)) => {
            let mut tar_args: Vec<String> = ["-x", "-z", "-C", &location, "-f", "-"].iter().map(|a| a.to_string()).collect();
            tar_args.extend(flags);
            let tar = Cmd { program: "tar".into(), args: tar_args, cwd: Some(io.cwd.clone()), stdin: Some(unb64(&data)), inherit: false, quiet: false, env: Vec::new() };
            match (io.run)(&tar) {
                Ok(out) => {
                    let _ = io.err.write_all(&out.stdout);
                    out.code
                }
                Err(e) => {
                    put(io.err, &format!("tar: {e}\n"));
                    127
                }
            }
        }
        ("abort", _) => {
            put(io.err, "Upload aborted\n");
            0
        }
        (other, _) => {
            put(io.err, &format!("Fatal error: Unknown status: {other}\n"));
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::rc::Rc;
    use std::time::{Duration, Instant};

    use super::*;
    use crate::reports::ITERM2_VERSION;
    use crate::util_cli::{Cmd, Harness, Io, Mode, Output, Tty};
    use crate::util_osc::{TMUX_OSC, TMUX_ST, b64};

    fn text(b: &[u8]) -> String {
        String::from_utf8(b.to_vec()).unwrap()
    }

    /// A console utility run: stdin and stdout ttys, the console answering `replies`.
    fn console(replies: &[u8]) -> Harness {
        let mut h = Harness { stdin_tty: true, stdout_tty: true, ..Harness::default() };
        h.tty.console = true;
        h.tty.replies = replies.iter().map(|b| Some(*b)).collect();
        h.cwd = std::env::temp_dir();
        h
    }

    fn left(h: &Harness) -> Vec<u8> {
        h.tty.replies.iter().map(|b| b.unwrap_or(0)).collect()
    }

    /// A console that logs every call, in order, into the same list as the programs run.
    struct LogTty {
        log: Rc<RefCell<Vec<String>>>,
        replies: VecDeque<u8>,
    }

    impl Tty for LogTty {
        fn write(&mut self, bytes: &[u8]) {
            self.log.borrow_mut().push(format!("write {}", String::from_utf8_lossy(bytes)));
        }
        fn read(&mut self, _wait: Option<Duration>) -> Option<u8> {
            self.replies.pop_front()
        }
        fn set_mode(&mut self, m: Mode) {
            self.log.borrow_mut().push(format!("mode {m:?}"));
        }
        fn discard_input(&mut self) {
            self.log.borrow_mut().push("discard".into());
        }
        fn restore(&mut self) {
            self.log.borrow_mut().push("restore".into());
        }
        fn is_console(&self) -> bool {
            true
        }
    }

    /// Runs `name` over a `LogTty` answering `replies`, programs answered from `answers`;
    /// returns the code, stdout, stderr and the log.
    fn logged(name: &str, args: &[&str], replies: &[u8], answers: Vec<Result<Output, String>>) -> (i32, String, String, Vec<String>) {
        let log = Rc::new(RefCell::new(Vec::new()));
        let mut tty = LogTty { log: log.clone(), replies: replies.iter().copied().collect() };
        let mut answers: VecDeque<_> = answers.into();
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let mut input: &[u8] = b"";
        let env = |_: &str| None;
        let have = |_: &str| true;
        let mut random = || 0;
        let run_log = log.clone();
        let mut run = |c: &Cmd| {
            run_log.borrow_mut().push(format!("run {} {}", c.program, c.args.join(" ")));
            answers.pop_front().unwrap_or_else(|| Err("no answer".into()))
        };
        let mut io = Io {
            out: &mut out,
            err: &mut err,
            input: &mut input,
            stdin_tty: true,
            stdout_tty: true,
            tty: &mut tty,
            env: &env,
            cwd: std::env::temp_dir(),
            random: &mut random,
            run: &mut run,
            have: &have,
        };
        let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
        let code = crate::util_cli::run(name, &args, &mut io);
        let log = log.borrow().clone();
        (code, text(&out), text(&err), log)
    }

    fn ok(stdout: &str) -> Result<Output, String> {
        Ok(Output { code: 0, stdout: stdout.as_bytes().to_vec() })
    }

    #[test]
    fn version_passes_it2check() {
        assert!(it2check_passes(&format!("ITERM2 {ITERM2_VERSION}"), MIN_VERSION));
        assert!(it2check_passes("ITERM2 3.7.0", MIN_VERSION));
        assert!(!it2check_passes("ITERM2 0.0.1", MIN_VERSION));
        assert!(it2check_passes("ITERM2 2.9.20160304", MIN_VERSION), "equal passes");
        assert!(!it2check_passes("XTERM 9", MIN_VERSION));
        assert!(it2check_passes("ITERM2", MIN_VERSION), "the version is then the word itself, above \"2\"");
        assert!(!it2check_passes("", MIN_VERSION));
        assert!(!ITERM2_VERSION.contains('n'));
        assert!(!ITERM2_VERSION.starts_with('0'));
    }

    #[test]
    fn it2check_follows_the_script() {
        let mut h = console(b"\x1b[ITERM2 3.7.0n\x1b[0n");
        assert_eq!(h.run("it2check", &[]), 0);
        assert!(h.tty.discarded);
        assert_eq!(h.tty.modes, [Mode::Raw]);
        assert_eq!(text(&h.tty.written), "\x1b[1337n\x1b[5n");
        assert!(h.tty.restored);
        assert!(h.out.is_empty() && h.err.is_empty());
        // The order: input drained before anything is written, raw mode, the two queries.
        let (code, _, _, log) = logged("it2check", &[], b"\x1b[ITERM2 3.7.0n\x1b[0n", vec![]);
        assert_eq!(code, 0);
        assert_eq!(log, ["discard", "mode Raw", "write \x1b[1337n", "write \x1b[5n", "restore"]);

        // Only DSR 5 answers: its "0" is the first answer and nothing more is read.
        let mut h = console(b"\x1b[0nXYZ");
        assert_eq!(h.run("it2check", &[]), 1);
        assert_eq!(left(&h), b"XYZ");
        let mut h = console(b"\x1b[3nXYZ");
        assert_eq!(h.run("it2check", &[]), 1);
        assert_eq!(left(&h), b"XYZ");
        let mut h = console(b"\x1b[ITERM2 0.0.1n\x1b[0n");
        assert_eq!(h.run("it2check", &[]), 1);
        let mut h = console(b"\x1b[ITERM2 3.7.0n\x1b[0n");
        assert_eq!(h.run("it2check", &["3.8"]), 1);
        let mut h = console(b"\x1b[ITERM2 3.7.0n\x1b[0n");
        assert_eq!(h.run("it2check", &["3.7.0"]), 0);
        let mut h = console(b"\x1b[ITERM2 3.7.0n\x1b[0n");
        assert_eq!(h.run("it2check", &["3.8", "x"]), 0, "two arguments keep the minimum");
        // ConPTY swallowed the reply's ESC [ I (Task 1): it is put back.
        let mut h = console(b"TERM2 3.7.0n\x1b[0n");
        assert_eq!(h.run("it2check", &[]), 0);
        assert!(left(&h).is_empty());

        for (stdin_tty, stdout_tty, is_console) in [(false, true, true), (true, false, true), (true, true, false)] {
            let mut h = console(b"\x1b[ITERM2 3.7.0n\x1b[0n");
            h.stdin_tty = stdin_tty;
            h.stdout_tty = stdout_tty;
            h.tty.console = is_console;
            assert_eq!(h.run("it2check", &[]), 1);
            assert!(h.tty.written.is_empty() && h.tty.modes.is_empty() && !h.tty.discarded);
        }

        let mut bytes = b"\x1b[ITERM2 3.7.0n".iter().copied();
        assert_eq!(read_dsr(&mut || bytes.next()), "ITERM2 3.7.0");
        assert_eq!(bytes.next(), None);
        let mut bytes = b"\x1b[0n\x1b[5n".iter().copied();
        assert_eq!(read_dsr(&mut || bytes.next()), "0");
        assert_eq!(bytes.next(), Some(0x1b), "one answer only");
        // `echo ${dsr}`: words joined by one space; input ending early ends the answer.
        let mut bytes = b"\x1b[A  B\tC\nn".iter().copied();
        assert_eq!(read_dsr(&mut || bytes.next()), "A B C");
        let mut bytes = b"\x1b[ITER".iter().copied();
        assert_eq!(read_dsr(&mut || bytes.next()), "ITER");
        assert_eq!((terminal_of("A B C"), version_of("A B C")), ("A", "C"));
        assert_eq!((terminal_of("ABC"), version_of("ABC")), ("ABC", "ABC"));
    }

    #[test]
    fn variable_queries_follow_the_script() {
        let mut h = console(b"\x1b]1337;ReportVariable=aGk=\x07");
        assert_eq!(h.run("it2getvar", &["user.x"]), 0);
        assert_eq!(text(&h.tty.written), "\x1b]1337;ReportVariable=dXNlci54\x07");
        assert_eq!(h.tty.modes, [Mode::Cbreak]);
        assert!(h.tty.restored);
        assert_eq!(text(&h.out), "hi\n");
        assert!(h.err.is_empty());

        let mut h = console(b"\x1b]1337;ReportVariable=\x07");
        assert_eq!(h.run("it2getvar", &["user.x"]), 0);
        assert_eq!(text(&h.out), "\n");

        let mut h = console(b"");
        assert_eq!(h.run("it2getvar", &["user.x"]), 2);
        assert_eq!(text(&h.err), "ERROR: Failed to read response from Tandem\n");
        assert!(h.out.is_empty());
        assert!(h.tty.restored);

        let mut h = console(format!("\x1b]1337;ReportVariable={}\x07", b64(b"a\n\n")).as_bytes());
        assert_eq!(h.run("it2getvar", &["user.x"]), 0);
        assert_eq!(text(&h.out), "a\n");

        // The 5 s wait, on a fake clock one second per look: bytes keep coming, no BEL.
        struct Endless;
        impl Tty for Endless {
            fn write(&mut self, _: &[u8]) {}
            fn read(&mut self, wait: Option<Duration>) -> Option<u8> {
                assert!(wait.is_some_and(|w| w <= VARIABLE_WAIT && !w.is_zero()), "{wait:?}");
                Some(b'x')
            }
            fn set_mode(&mut self, _: Mode) {}
            fn discard_input(&mut self) {}
            fn restore(&mut self) {}
            fn is_console(&self) -> bool {
                true
            }
        }
        let start = Instant::now();
        let ticks = std::cell::Cell::new(0u64);
        let clock = || {
            let t = ticks.get();
            ticks.set(t + 1);
            start + Duration::from_secs(t)
        };
        assert_eq!(read_to_bel(&mut Endless, VARIABLE_WAIT, &clock), "xxxx", "reads at 1 s to 4 s, stops at 5 s");
        assert_eq!(VARIABLE_WAIT, Duration::from_secs(5));
        let mut replies = console(b"ab\x07cd").tty;
        assert_eq!(read_to_bel(&mut replies, VARIABLE_WAIT, &Instant::now), "ab");
        assert_eq!(replies.replies.len(), 2, "nothing after the BEL is read");

        assert_eq!(variable_answer("\x1b]1337;ReportVariable=aGk="), Some("hi\n".to_string()));
        assert_eq!(variable_answer("\x1b]1337;Other=aGk="), None);
        assert_eq!(variable_answer(""), None);
        assert_eq!(variable_answer("\x1b]1337;ReportVariable=/w=="), Some("\u{FFFD}\n".to_string()), "lossy UTF-8");

        let mut h = console(b"");
        assert_eq!(h.run("it2profile", &["-s", "Work"]), 0);
        assert_eq!(text(&h.tty.written), "\x1b]1337;SetProfile=Work\x07");
        assert!(h.tty.modes.is_empty() && h.out.is_empty() && h.err.is_empty());

        let mut h = console(b"\x1b]1337;ReportVariable=RGVmYXVsdA==\x07");
        assert_eq!(h.run("it2profile", &["-g"]), 0);
        assert_eq!(text(&h.tty.written), "\x1b]1337;ReportVariable=cHJvZmlsZU5hbWU=\x07");
        assert_eq!(h.tty.modes, [Mode::Cbreak]);
        assert_eq!(text(&h.out), "Default\n");

        let mut h = console(b"");
        h.env.insert("ITERM_PROFILE".into(), "Default".into());
        assert_eq!(h.run("it2profile", &["-r"]), 0);
        assert_eq!(text(&h.tty.written), "\x1b]1337;SetProfile=Default\x07");
        let mut h = console(b"");
        assert_eq!(h.run("it2profile", &["--reset"]), 0);
        assert_eq!(text(&h.tty.written), "\x1b]1337;SetProfile=\x07");

        // Under tmux every write is wrapped.
        let wrap = |body: &str| format!("{TMUX_OSC}{body}{TMUX_ST}");
        let tmux = |replies: &[u8]| {
            let mut h = console(replies);
            h.env.insert("TERM".into(), "tmux-256color".into());
            h
        };
        let mut h = tmux(b"\x1b]1337;ReportVariable=aGk=\x07");
        assert_eq!(h.run("it2getvar", &["user.x"]), 0);
        assert_eq!(text(&h.tty.written), wrap("1337;ReportVariable=dXNlci54"));
        let mut h = tmux(b"");
        assert_eq!(h.run("it2profile", &["--set", "Work"]), 0);
        assert_eq!(text(&h.tty.written), wrap("1337;SetProfile=Work"));
        let mut h = tmux(b"\x1b]1337;ReportVariable=RGVmYXVsdA==\x07");
        assert_eq!(h.run("it2profile", &["--get"]), 0);
        assert_eq!(text(&h.tty.written), wrap("1337;ReportVariable=cHJvZmlsZU5hbWU="));
        let mut h = tmux(b"");
        assert_eq!(h.run("it2profile", &["-r"]), 0);
        assert_eq!(text(&h.tty.written), wrap("1337;SetProfile="));
    }

    #[test]
    fn query_texts_follow_the_scripts() {
        let help = "\nUsage: it2getvar variable_name\n\nOutput value of the Tandem variable\n\nSee the Variables Reference for information about built-in Tandem variables:\n  -> https://iterm2.com/documentation-variables.html\n\n";
        assert_eq!(getvar_help(), help);
        for (args, code, err) in [
            (&[][..], 0, help.to_string()),
            (&["-h"][..], 0, help.to_string()),
            (&["--h"][..], 0, help.to_string()),
            (&["--help", "x"][..], 0, help.to_string()),
            (&["-x"][..], 1, format!("ERROR: Unknown option: -x\n{help}")),
            (&[""][..], 1, "ERROR: Variable name can't be empty\n".to_string()),
        ] {
            let mut h = console(b"");
            assert_eq!(h.run("it2getvar", args), code, "{args:?}");
            assert_eq!(text(&h.err), err, "{args:?}");
            assert!(h.out.is_empty() && h.tty.written.is_empty(), "{args:?}");
        }

        let help = profile_help();
        assert_eq!(
            help,
            "\nUsage: it2profile -s profile_name\n       it2profile -g\n\nChange Tandem session profile on the fly\n\nOptions:\n\n    -s      Set current Tandem session profile to specified profile name\n                empty string refers to profile marked as Default\n    -g      Get current Tandem session profile name\n    -r      Reset to the session initial profile\n\n"
        );
        assert!(help.contains("Change Tandem session profile on the fly") && !help.contains("iTerm2"));
        for (args, code, err) in [
            (&[][..], 0, help.clone()),
            (&["-h"][..], 0, help.clone()),
            (&["-s"][..], 1, format!("ERROR: -s option requires exactly 1 argument - profile name\n{help}")),
            (&["-s", "a", "b"][..], 1, format!("ERROR: -s option requires exactly 1 argument - profile name\n{help}")),
            (&["-g", "x"][..], 1, format!("ERROR: -g option does not accept any argument\n{help}")),
            (&["-r", "x"][..], 1, format!("ERROR: -r option does not accept any argument\n{help}")),
            (&["-q"][..], 1, format!("ERROR: Unknown option: -q\n{help}")),
            (&["Work"][..], 1, format!("ERROR: Unknown option: Work\n{help}")),
        ] {
            let mut h = console(b"");
            assert_eq!(h.run("it2profile", args), code, "{args:?}");
            assert_eq!(text(&h.err), err, "{args:?}");
            assert!(h.out.is_empty() && h.tty.written.is_empty(), "{args:?}");
        }
    }

    #[test]
    fn it2ul_follows_the_script() {
        let version = "bsdtar 3.7.2 - libarchive 3.7.2\nmore\n";
        // `tar --version | head -1 | b64_encode`: the first line goes into base64 with its newline.
        let request = format!("\x1b]1337;RequestUpload=format=tgz;version={}\x07", b64(b"bsdtar 3.7.2 - libarchive 3.7.2\n"));
        assert_eq!(upload_request(None, version), request);
        assert_eq!(upload_request(Some("screen"), version), format!("{TMUX_OSC}1337;RequestUpload=format=tgz;version={}{TMUX_ST}", b64(b"bsdtar 3.7.2 - libarchive 3.7.2\n")));
        assert_eq!(upload_request(None, "one line"), format!("\x1b]1337;RequestUpload=format=tgz;version={}\x07", b64(b"one line")));
        assert_eq!(upload_request(None, ""), "\x1b]1337;RequestUpload=format=tgz;version=\x07");

        let cwd = std::env::temp_dir();
        let loc = cwd.to_string_lossy().into_owned();
        let mut h = console(b"ok\nQUJD\rREVG\r\r");
        h.answers = [ok(version), ok("")].into();
        assert_eq!(h.run("it2ul", &[]), 0);
        assert_eq!(text(&h.out), request);
        assert_eq!(h.tty.modes, [Mode::Cbreak]);
        assert!(h.tty.restored);
        assert!(h.tty.written.is_empty(), "the request goes to stdout");
        assert_eq!(h.runs.len(), 2);
        assert_eq!((h.runs[0].program.as_str(), h.runs[0].args.clone(), h.runs[0].stdin.clone()), ("tar", vec!["--version".to_string()], None));
        let tar = &h.runs[1];
        assert_eq!(tar.program, "tar");
        assert_eq!(tar.args, ["-x", "-z", "-C", &loc, "-f", "-"]);
        assert_eq!(tar.stdin.as_deref(), Some(&b"ABCDEF"[..]));
        assert!(!tar.inherit);
        assert!(h.err.is_empty());

        // The mode is put back before tar runs; tar's output goes to stderr and its code is the
        // utility's.
        let (code, out, err, log) = logged("it2ul", &["C:\\d", "-v"], b"ok\nQUJD\r\r", vec![ok(version), Ok(Output { code: 3, stdout: b"x f.txt\n".to_vec() })]);
        assert_eq!(code, 3);
        assert_eq!(out, request);
        assert_eq!(err, "x f.txt\n");
        assert_eq!(log, ["mode Cbreak", "run tar --version", "restore", "run tar -x -z -C C:\\d -f - -v"]);
        // `$*` unquoted: the flags are split into words.
        let (_, _, _, log) = logged("it2ul", &["C:\\d", "-v  -k"], b"ok\n\r", vec![ok(version), ok("")]);
        assert_eq!(log.last().unwrap(), "run tar -x -z -C C:\\d -f - -v -k");

        let mut h = console(b"abort\n");
        h.answers = [ok(version)].into();
        assert_eq!(h.run("it2ul", &[]), 0);
        assert_eq!(text(&h.err), "Upload aborted\n");
        assert_eq!(h.runs.len(), 1, "tar is not run");
        assert!(h.tty.restored);

        let mut h = console(b"zzz\n");
        h.answers = [ok(version)].into();
        assert_eq!(h.run("it2ul", &[]), 1);
        assert_eq!(text(&h.err), "Fatal error: Unknown status: zzz\n");
        assert!(h.tty.restored);

        // Input ending before any status is an unknown (empty) status.
        let mut h = console(b"");
        h.answers = [ok(version)].into();
        assert_eq!(h.run("it2ul", &[]), 1);
        assert_eq!(text(&h.err), "Fatal error: Unknown status: \n");

        // Not a console: the status and data come from standard input, and no mode is set.
        let mut h = console(b"");
        h.stdin_tty = false;
        h.input = b"ok\r\nQUJD\n\n".to_vec();
        h.answers = [ok(version), ok("")].into();
        assert_eq!(h.run("it2ul", &[]), 0);
        assert_eq!(h.runs[1].stdin.as_deref(), Some(&b""[..]), "CR ends the status line, so the LF is an empty line and the data empty");
        let mut h = console(b"");
        h.stdin_tty = false;
        h.input = b"ok\nQUJD\n\n".to_vec();
        h.answers = [ok(version), ok("")].into();
        assert_eq!(h.run("it2ul", &[]), 0);
        assert!(h.tty.modes.is_empty() && !h.tty.restored);
        assert_eq!(h.runs[1].stdin.as_deref(), Some(&b"ABC"[..]));

        // A network destination is never handed to tar.
        let mut h = console(b"ok\nQUJD\r\r");
        h.answers = [ok(version), ok("")].into();
        assert_eq!(h.run("it2ul", &["\\\\host\\share"]), 1);
        assert!(h.out.is_empty() && h.runs.is_empty());
        assert!(h.tty.modes.is_empty() && !h.tty.restored, "no console mode is set before the refusal");
        assert_eq!(text(&h.err), "Fatal error: \\\\host\\share: network paths are not supported\n");
        // A verbatim local destination is local; verbatim UNC and GLOBALROOT forms are not.
        let verbatim = format!("\\\\?\\{loc}");
        let mut h = console(b"ok\nQUJD\r\r");
        h.answers = [ok(version), ok("")].into();
        assert_eq!(h.run("it2ul", &[verbatim.as_str()]), 0);
        assert_eq!(h.runs.len(), 2, "tar runs");
        assert_eq!(h.runs[1].args, ["-x", "-z", "-C", verbatim.as_str(), "-f", "-"]);
        for refused in ["\\\\?\\UNC\\host\\share", "\\\\?\\GLOBALROOT\\Device\\Mup\\host\\share"] {
            let mut h = console(b"ok\nQUJD\r\r");
            h.answers = [ok(version), ok("")].into();
            assert_eq!(h.run("it2ul", &[refused]), 1, "{refused}");
            assert!(h.out.is_empty() && h.runs.is_empty() && h.tty.modes.is_empty() && !h.tty.restored, "{refused}");
            assert_eq!(text(&h.err), format!("Fatal error: {refused}: network paths are not supported\n"));
        }

        let mut bytes = b"ok\r\n".iter().copied();
        assert_eq!(read_line(&mut || bytes.next()), Some("ok".to_string()));
        assert_eq!(read_line(&mut || bytes.next()), Some(String::new()));
        assert_eq!(read_line(&mut || bytes.next()), None);
        let mut bytes = b"tail".iter().copied();
        assert_eq!(read_line(&mut || bytes.next()), Some("tail".to_string()));
        let mut bytes = b"QUJD\rREVG\n\rafter".iter().copied();
        assert_eq!(read_stanza(&mut || bytes.next()), "QUJDREVG");
        assert_eq!(bytes.collect::<Vec<u8>>(), b"after");
        let mut bytes = b"QUJD".iter().copied();
        assert_eq!(read_stanza(&mut || bytes.next()), "QUJD", "input ending ends the stanza");
    }
}
