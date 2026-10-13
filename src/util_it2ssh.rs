//! it2ssh (milestone 14a): iTerm2's `OtherResources/Utilities/it2ssh` over 11e's `util_cli::Io`.
//! It asks ssh for its usage to learn the options without a value, sorts the arguments into
//! ssh's options and the host, sends the three handshake OSCs (Env, it2ssh, SendConductor)
//! from the remote side through ssh's remote command, which then reads the conductor Termurion
//! types, and after ssh ends prints EndSSH and waits for the blank line that answers it.

use std::io::Write;

use crate::util_cli::{self, Cmd, Io, Mode};
use crate::util_osc::b64;

/// The ssh run when `SSH` is unset or empty (iTerm2 `/usr/bin/ssh`, `IS:43`): found on PATH.
pub const DEFAULT_SSH: &str = "ssh";

/// The remote `sh -c` script: it evaluates its `$0`, the sanitized command (`IS:41`).
pub const EVAL_CMD: &str = "'eval \"$(echo \"$0\")\"'";

/// The origin token when no Termurion answers (iTerm2 off macOS, `IS:55`).
pub const NO_TOKEN: &str = "none";

/// The environment variable naming the transport a pane was started with.
pub const TRANSPORT_VAR: &str = "TANDEM_SSH_TRANSPORT";

pub const NEEDS_PROFILE: &str =
    "it2ssh: SSH integration needs a session from a Termurion SSH profile (Settings > Profiles > General > Command type: SSH); running plain ssh.";

/// bash 5.2's `printf "%q " "$@"` in a UTF-8 locale (`lib/sh/shquote.c`): each word quoted
/// and followed by one space.
pub fn requote(args: &[String]) -> String {
    let mut out = String::new();
    for a in args {
        quote_word(a, &mut out);
        out.push(' ');
    }
    out
}

/// One word as `%q` writes it: `''` when empty, `$'...'` when it holds a control character,
/// else backslashes before the shell's special characters.
fn quote_word(w: &str, out: &mut String) {
    if w.is_empty() {
        out.push_str("''");
        return;
    }
    if w.chars().any(char::is_control) {
        out.push_str("$'");
        for c in w.chars() {
            match c {
                '\u{7}' => out.push_str("\\a"),
                '\u{8}' => out.push_str("\\b"),
                '\u{1b}' => out.push_str("\\E"),
                '\u{c}' => out.push_str("\\f"),
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                '\t' => out.push_str("\\t"),
                '\u{b}' => out.push_str("\\v"),
                '\\' => out.push_str("\\\\"),
                '\'' => out.push_str("\\'"),
                c if c.is_control() => {
                    let mut b = [0u8; 4];
                    for byte in c.encode_utf8(&mut b).bytes() {
                        out.push_str(&format!("\\{byte:03o}"));
                    }
                }
                c => out.push(c),
            }
        }
        out.push('\'');
        return;
    }
    let mut prev: Option<char> = None;
    for c in w.chars() {
        if needs_quote(c, prev) {
            out.push('\\');
        }
        out.push(c);
        prev = Some(c);
    }
}

/// Whether `%q` puts a backslash before `c`, a printable character after `prev` in its word
/// (None at the word's start): the shell's special characters, `~` where it would expand, `#`
/// where it would start a comment.
pub fn needs_quote(c: char, prev: Option<char>) -> bool {
    matches!(c, ' ' | '!' | '"' | '$' | '&' | '\'' | '(' | ')' | '*' | ',' | ';' | '<' | '>' | '?' | '[' | '\\' | ']' | '^' | '`' | '{' | '|' | '}')
        || (c == '~' && matches!(prev, None | Some('=') | Some(':')))
        || (c == '#' && prev.is_none())
}

/// The options without a value, from ssh's usage (`IS:77`): newlines removed, then the letters
/// after the first `[` and its dashes; a usage without `[` is kept as it is (sed's unmatched
/// line).
pub fn boolean_args(usage: &str) -> String {
    let line: String = usage.chars().filter(|&c| c != '\n').collect();
    let Some(open) = line.find('[') else { return line };
    line[open + 1..].trim_start_matches('-').chars().take_while(char::is_ascii_alphanumeric).collect()
}

/// it2ssh's arguments sorted as its loop sorts them (`IS:72-124`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Parsed {
    pub user_args: Vec<String>,
    pub host: String,
    pub command: Vec<String>,
}

/// Sorts `args` as `IS:80-124` does, with `boolean` the options that take no value: `-t` is
/// appended when not given, and a second one when `force_tty` (standard input is not a
/// console, so ssh would not allocate a remote pty for one). Err is `not_interactive`'s text.
/// Kept quirks: `--` is matched as an option (its own case is dead), and an option's whole
/// text after its dash is looked for in `boolean` as a literal substring, so `--`, `-vvv` and
/// `-tt` take the next word as their value.
pub fn parse(args: &[String], boolean: &str, force_tty: bool) -> Result<Parsed, String> {
    let mut p = Parsed { user_args: Vec::new(), host: String::new(), command: Vec::new() };
    let (mut args_allowed, mut expect_value, mut has_t) = (true, false, false);
    for a in args {
        if expect_value {
            p.user_args.push(a.clone());
            expect_value = false;
        } else if !args_allowed {
            if p.host.is_empty() {
                p.host = a.clone();
            } else {
                p.command.push(a.clone());
            }
        } else {
            match a.as_str() {
                "-N" | "-n" | "-f" | "-G" => return Err(not_interactive(a)),
                "-t" => {
                    has_t = true;
                    p.user_args.push(a.clone());
                }
                _ if a.starts_with('-') => {
                    expect_value = !boolean.contains(&a[1..]);
                    p.user_args.push(a.clone());
                }
                _ => {
                    args_allowed = false;
                    p.host = a.clone();
                }
            }
        }
    }
    if !has_t {
        p.user_args.push("-t".to_string());
    }
    if force_tty {
        p.user_args.push("-t".to_string());
    }
    Ok(p)
}

/// iTerm2's refusal of a non-interactive option (`IS:93`, its "it2sh" kept).
pub fn not_interactive(arg: &str) -> String {
    format!("it2sh is meant for interactive use via SSH only and is not compatible with the {arg} argument.")
}

/// it2ssh's own tmux rule (`IS:11-26`): `TERM` starts with `screen` (not `tmux*`).
pub fn screen_wrapped(term: Option<&str>) -> bool {
    term.is_some_and(|t| t.starts_with("screen"))
}

/// `ESC ] body BEL`, passed through tmux when `screen_wrapped`.
pub fn osc(term: Option<&str>, body: &str) -> String {
    if screen_wrapped(term) {
        format!("\x1bPtmux;\x1b\x1b]{body}\x07\x1b\\")
    } else {
        format!("\x1b]{body}\x07")
    }
}

/// The three handshake OSCs the remote prints (`IS:134-147`).
pub fn intro(env_b64: &str, token: &str, unique_id: &str, encoded_ba: &str, ssh_args_b64: &str, term: Option<&str>) -> String {
    osc(term, &format!("1337;Env=report=all:{env_b64}"))
        + &osc(term, &format!("1337;it2ssh={token} {unique_id} {encoded_ba} {ssh_args_b64}"))
        + &osc(term, "1337;SendConductor=v=3")
}

/// The remote command (`IS:154-159`): echo off, print a space and `intro`, skip to the
/// conductor's start line, gather its base64 lines up to the ESC line, decode and run it.
pub fn sanitized(intro: &str) -> String {
    format!(
        "stty -echo -isig; printf \"%s {intro}\";s=\"\";IFS=\"\"; while read -r l;do [ \"$l\" = \"-- BEGIN CONDUCTOR --\" ]&&break; done; while read -r l; do [ \"$l\" = \"\x1b\" ]&&break; s=$(printf \"%s\\n%s\" \"$s\" \"$l\"); done; unset IFS; s=$(echo \"$s\" | {{ command -v base64 > /dev/null 2> /dev/null && command base64 -d || {{ command -v b64encode > /dev/null 2> /dev/null && command fold -w 76 | command b64decode -r; }} || echo \"echo base64 not available on remote host\"; }}); eval \"$s\""
    )
}

/// ssh's arguments (`IS:164-172`): the options, `--`, the host, then `exec sh -c` over the
/// quoted `sanitized` command.
pub fn ssh_argv(p: &Parsed, sanitized: &str) -> Vec<String> {
    let mut argv = p.user_args.clone();
    argv.extend(["--", &p.host, "exec", "sh", "-c", EVAL_CMD].map(str::to_string));
    argv.push(format!("'{sanitized}'"));
    argv
}

/// `command env`: one `NAME=value` line each.
pub fn env_listing(vars: &[(String, String)]) -> String {
    vars.iter().map(|(k, v)| format!("{k}={v}\n")).collect()
}

/// `${RANDOM}${RANDOM}`.
pub fn unique_id(a: u32, b: u32) -> String {
    format!("{a}{b}")
}

/// ssh's usage as it2ssh prints it (`IS:7`): the first `usage: ssh` of each line renamed.
pub fn usage_text(ssh_usage: &str) -> String {
    ssh_usage.split_inclusive('\n').map(|line| line.replacen("usage: ssh", "usage: it2ssh", 1)).collect()
}

/// A line that ends the wait after EndSSH (`IS:188-191`): empty, or the one byte `^C`, once
/// `read`'s default IFS has trimmed spaces and tabs from both ends.
pub fn line_ends_wait(line: &[u8]) -> bool {
    let blank = |b: &u8| matches!(b, b' ' | b'\t');
    let start = line.iter().position(|b| !blank(b)).unwrap_or(line.len());
    let end = line.iter().rposition(|b| !blank(b)).map_or(start, |i| i + 1);
    let word = &line[start..end];
    word.is_empty() || word == [0x03]
}

/// it2ssh (`IS`) over `io`: its exit code.
pub fn run(args: &[String], io: &mut Io) -> i32 {
    run_with(args, io)
}

fn say(w: &mut dyn Write, text: &str) {
    let _ = w.write_all(text.as_bytes());
    let _ = w.write_all(b"\n");
    let _ = w.flush();
}

/// `ssh` with the user's arguments and streams, its code returned (127 when it cannot start).
fn plain_ssh(ssh: &str, args: &[String], io: &mut Io) -> i32 {
    let c = Cmd { program: ssh.to_string(), args: args.to_vec(), cwd: None, stdin: None, inherit: true, quiet: false, env: Vec::new(), merge_stderr: false };
    match (io.run)(&c) {
        Ok(o) => o.code,
        Err(e) => {
            say(io.err, &format!("it2ssh: {e}"));
            127
        }
    }
}

/// ssh's usage (its output with standard error joined), lossy; Err when it cannot start.
fn ssh_usage(ssh: &str, io: &mut Io) -> Result<(i32, String), String> {
    let c = Cmd { program: ssh.to_string(), args: Vec::new(), cwd: None, stdin: None, inherit: false, quiet: false, env: Vec::new(), merge_stderr: true };
    (io.run)(&c).map(|o| (o.code, String::from_utf8_lossy(&o.stdout).into_owned()))
}

fn run_with(args: &[String], io: &mut Io) -> i32 {
    let env_ssh = (io.env)("SSH").filter(|s| !s.is_empty());
    let ssh = env_ssh.as_deref().unwrap_or(DEFAULT_SSH).to_string();
    // `IS:6-9`: plain `ssh`, not `$SSH`; under pipefail ssh's own failure is the code.
    if args.is_empty() {
        return match ssh_usage(DEFAULT_SSH, io) {
            Ok((code, text)) => {
                let _ = io.err.write_all(usage_text(&text).as_bytes());
                let _ = io.err.flush();
                if code != 0 { code } else { 1 }
            }
            Err(e) => {
                say(io.err, &format!("it2ssh: ssh: {e}"));
                127
            }
        };
    }
    let chosen = crate::ssh_transport::label(crate::ssh_transport::CHOSEN);
    if (io.env)(TRANSPORT_VAR).as_deref() != Some(chosen) {
        say(io.err, NEEDS_PROFILE);
        return plain_ssh(&ssh, args, io);
    }
    let token = (io.secret)().unwrap_or_else(|| NO_TOKEN.to_string());
    let ssh_args_b64 = b64(requote(args).as_bytes());
    let (a, b) = ((io.random)(), (io.random)());
    let uid = unique_id(a, b);
    let boolean = ssh_usage(&ssh, io).map(|(_, text)| boolean_args(&text)).unwrap_or_default();
    let parsed = match parse(args, &boolean, !io.stdin_tty) {
        Ok(p) => p,
        Err(m) => {
            say(io.out, &m);
            return 1;
        }
    };
    let term = util_cli::term(io);
    let term = term.as_deref();
    let env = b64(env_listing(&(io.env_all)()).as_bytes());
    let intro = intro(&env, &token, &uid, &b64(boolean.as_bytes()), &ssh_args_b64, term);
    let argv = ssh_argv(&parsed, &sanitized(&intro));
    let c = Cmd { program: ssh, args: argv, cwd: None, stdin: None, inherit: true, quiet: false, env: Vec::new(), merge_stderr: false };
    if let Err(e) = (io.run)(&c) {
        say(io.err, &format!("it2ssh: {e}"));
    }
    // `IS:175-193`: echo and signals off, EndSSH, then wait for Termurion's `^C` line or a blank
    // one, so a conductor typed after ssh died never reaches the local shell.
    let console = io.tty.is_console();
    if console {
        io.tty.set_mode(Mode::Raw);
    }
    let _ = io.out.write_all(osc(term, &format!("1337;EndSSH={uid}")).as_bytes());
    let _ = io.out.flush();
    let mut line = Vec::new();
    loop {
        let byte = if console {
            io.tty.read(None)
        } else {
            let mut b = [0u8; 1];
            match io.input.read(&mut b) {
                Ok(1) => Some(b[0]),
                _ => None,
            }
        };
        match byte {
            None => break,
            Some(b'\n' | b'\r') => {
                if line_ends_wait(&line) {
                    break;
                }
                line.clear();
            }
            Some(b) => line.push(b),
        }
    }
    io.tty.restore();
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ssh_transport::{CHOSEN, label};
    use crate::util_cli::{Harness, Output};

    fn strings(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    fn pairs(v: &[(&str, &str)]) -> Vec<(String, String)> {
        v.iter().map(|(k, x)| (k.to_string(), x.to_string())).collect()
    }

    const B: &str = "46AaCfGgKkMNnqsTtVvXxYy";

    /// OpenSSH for Windows 9.5p2's usage (EXTREME, 2026-09-27).
    const WIN_USAGE: &str = "usage: ssh [-46AaCfGgKkMNnqsTtVvXxYy] [-B bind_interface] [-b bind_address]\r\n           [-c cipher_spec] [-D [bind_address:]port] [-E log_file]\r\n           [-e escape_char] [-F configfile] [-I pkcs11] [-i identity_file]\r\n           [-J destination] [-L address] [-l login_name] [-m mac_spec]\r\n           [-O ctl_cmd] [-o option] [-P tag] [-p port] [-Q query_option]\r\n           [-R address] [-S ctl_path] [-W host:port] [-w local_tun[:remote_tun]]\r\n           destination [command [argument ...]]\r\n";

    fn parsed(user_args: &[&str], host: &str, command: &[&str]) -> Parsed {
        Parsed { user_args: strings(user_args), host: host.to_string(), command: strings(command) }
    }

    #[test]
    fn requote_follows_bash() {
        let words = strings(&[
            "host", "user@host", "-p", "2222", "a b", "it's", "$HOME", "~x", "a~b", "#x", "a#b", "", "a,b", "x=~", "a:~", "tab\there", "\u{e9}", "a\nb", "\u{1b}", "a=b", "%",
            "{x}", "[y]", "a^b", "a+b", "a.b-c/d_e", "\u{e9} b",
        ]);
        assert_eq!(
            requote(&words),
            "host user@host -p 2222 a\\ b it\\'s \\$HOME \\~x a~b \\#x a#b '' a\\,b x=\\~ a:\\~ $'tab\\there' \u{e9} $'a\\nb' $'\\E' a=b % \\{x\\} \\[y\\] a\\^b a+b a.b-c/d_e \u{e9}\\ b "
        );
        let words = strings(&[
            "\u{1}", "a'b\tc", "\u{7f}", "a\\b\tc", "\u{7}\u{8}\u{c}\r\u{b}", "!", "a*b", "a?b", "a|b", "a&b", "a;b", "a<b>c", "a(b)", "a\"b", "a`b", "tilde~", "=~", ":~x",
        ]);
        assert_eq!(
            requote(&words),
            "$'\\001' $'a\\'b\\tc' $'\\177' $'a\\\\b\\tc' $'\\a\\b\\f\\r\\v' \\! a\\*b a\\?b a\\|b a\\&b a\\;b a\\<b\\>c a\\(b\\) a\\\"b a\\`b tilde~ =\\~ :\\~x "
        );
        assert_eq!(requote(&[]), "");
        assert_eq!(b64(requote(&strings(&["-p", "2222", "user@host"])).as_bytes()), "LXAgMjIyMiB1c2VyQGhvc3Qg");
    }

    #[test]
    fn boolean_args_follow_iterm2() {
        assert_eq!(boolean_args(WIN_USAGE), B);
        assert_eq!(boolean_args("usage: x [--abc] y"), "abc");
        assert_eq!(boolean_args("no brackets\nhere"), "no bracketshere");
        assert_eq!(boolean_args(""), "");
        assert_eq!(b64(B.as_bytes()), "NDZBYUNmR2dLa01ObnFzVHRWdlh4WXk=");
    }

    #[test]
    fn parse_follows_iterm2() {
        let p = |a: &[&str]| parse(&strings(a), B, false);
        assert_eq!(p(&["-p", "2222", "user@host"]), Ok(parsed(&["-p", "2222", "-t"], "user@host", &[])));
        assert_eq!(p(&["-t", "-o", "ServerAliveInterval=30", "host", "ls", "-l"]), Ok(parsed(&["-t", "-o", "ServerAliveInterval=30"], "host", &["ls", "-l"])));
        assert_eq!(p(&["--", "host"]), Ok(parsed(&["--", "host", "-t"], "", &[])));
        assert_eq!(p(&["-vvv", "host"]), Ok(parsed(&["-vvv", "host", "-t"], "", &[])));
        assert_eq!(p(&["-tt", "host"]), Ok(parsed(&["-tt", "host", "-t"], "", &[])));
        assert_eq!(p(&["-v", "host"]), Ok(parsed(&["-v", "-t"], "host", &[])));
        assert_eq!(p(&["-", "h"]), Ok(parsed(&["-", "-t"], "h", &[])));
        assert_eq!(p(&["host", "-p", "22"]), Ok(parsed(&["-t"], "host", &["-p", "22"])));
        for arg in ["-N", "-n", "-f", "-G"] {
            assert_eq!(p(&[arg, "host"]), Err(format!("it2sh is meant for interactive use via SSH only and is not compatible with the {arg} argument.")));
            assert_eq!(not_interactive(arg), format!("it2sh is meant for interactive use via SSH only and is not compatible with the {arg} argument."));
        }
        assert_eq!(parse(&strings(&["host"]), B, true), Ok(parsed(&["-t", "-t"], "host", &[])));
        assert_eq!(parse(&strings(&["-t", "host"]), B, true), Ok(parsed(&["-t", "-t"], "host", &[])));
    }

    #[test]
    fn intro_and_command_follow_iterm2() {
        let i = intro("QT0xCg==", "none", "1234512345", "NDZBYUNmR2dLa01ObnFzVHRWdlh4WXk=", "LXAgMjIyMiB1c2VyQGhvc3Qg", Some("xterm-256color"));
        assert_eq!(
            i,
            "\u{1b}]1337;Env=report=all:QT0xCg==\u{7}\u{1b}]1337;it2ssh=none 1234512345 NDZBYUNmR2dLa01ObnFzVHRWdlh4WXk= LXAgMjIyMiB1c2VyQGhvc3Qg\u{7}\u{1b}]1337;SendConductor=v=3\u{7}"
        );
        let wrapped = intro("QT0xCg==", "none", "1234512345", "NDZBYUNmR2dLa01ObnFzVHRWdlh4WXk=", "LXAgMjIyMiB1c2VyQGhvc3Qg", Some("screen-256color"));
        assert_eq!(
            wrapped,
            "\x1bPtmux;\x1b\x1b]1337;Env=report=all:QT0xCg==\x07\x1b\\\x1bPtmux;\x1b\x1b]1337;it2ssh=none 1234512345 NDZBYUNmR2dLa01ObnFzVHRWdlh4WXk= LXAgMjIyMiB1c2VyQGhvc3Qg\x07\x1b\\\x1bPtmux;\x1b\x1b]1337;SendConductor=v=3\x07\x1b\\"
        );
        for term in [Some("tmux-256color"), None] {
            assert_eq!(intro("QT0xCg==", "none", "1234512345", "NDZBYUNmR2dLa01ObnFzVHRWdlh4WXk=", "LXAgMjIyMiB1c2VyQGhvc3Qg", term), i, "{term:?}");
        }
        assert!(screen_wrapped(Some("screen")));
        assert!(!screen_wrapped(Some("tmux-256color")));
        assert!(!screen_wrapped(Some("xterm")));
        assert!(!screen_wrapped(None));
        assert_eq!(osc(Some("xterm"), "1337;EndSSH=1"), "\x1b]1337;EndSSH=1\x07");
        assert_eq!(osc(Some("screen"), "1337;EndSSH=1"), "\x1bPtmux;\x1b\x1b]1337;EndSSH=1\x07\x1b\\");

        let s = sanitized(&i);
        assert_eq!(
            s,
            "stty -echo -isig; printf \"%s ".to_string()
                + &i
                + "\";s=\"\";IFS=\"\"; while read -r l;do [ \"$l\" = \"-- BEGIN CONDUCTOR --\" ]&&break; done; while read -r l; do [ \"$l\" = \"\u{1b}\" ]&&break; s=$(printf \"%s\\n%s\" \"$s\" \"$l\"); done; unset IFS; s=$(echo \"$s\" | { command -v base64 > /dev/null 2> /dev/null && command base64 -d || { command -v b64encode > /dev/null 2> /dev/null && command fold -w 76 | command b64decode -r; } || echo \"echo base64 not available on remote host\"; }); eval \"$s\""
        );
        let p = parse(&strings(&["-p", "2222", "user@host"]), B, false).unwrap();
        let mut want = strings(&["-p", "2222", "-t", "--", "user@host", "exec", "sh", "-c", EVAL_CMD]);
        want.push(format!("'{s}'"));
        assert_eq!(ssh_argv(&p, &s), want);
        assert_eq!(EVAL_CMD, "'eval \"$(echo \"$0\")\"'");
        assert_eq!(env_listing(&pairs(&[("A", "1"), ("B", "x y")])), "A=1\nB=x y\n");
        assert_eq!(unique_id(12345, 7), "123457");
        assert_eq!(usage_text("usage: ssh [-46] x\r\n           [-c c]\r\n"), "usage: it2ssh [-46] x\r\n           [-c c]\r\n");
        assert!(line_ends_wait(b""));
        assert!(line_ends_wait(b"\x03"));
        assert!(!line_ends_wait(b"x"));
        assert!(!line_ends_wait(b"\x03\x03"));
        assert!(line_ends_wait(b" \t "), "read trims IFS whitespace");
        assert!(line_ends_wait(b" \x03\t"));
        assert!(!line_ends_wait(b" x "));
    }

    /// A harness ready for a ready it2ssh run of `-p 2222 user@host`.
    fn ready_harness(env: &[(&str, &str)]) -> Harness {
        let mut h = Harness::default();
        for (k, v) in env {
            h.env.insert(k.to_string(), v.to_string());
        }
        h.randoms = [12345, 12345].into();
        h.secrets = [None].into();
        h.answers = [Ok(Output { code: 255, stdout: WIN_USAGE.as_bytes().to_vec() }), Ok(Output { code: 0, stdout: vec![] })].into();
        h.stdin_tty = true;
        h.tty.console = true;
        h.tty.replies = [Some(3), Some(10)].into();
        h
    }

    fn cmd(program: &str, args: Vec<String>, inherit: bool, merge_stderr: bool) -> Cmd {
        Cmd { program: program.to_string(), args, cwd: None, stdin: None, inherit, quiet: false, env: Vec::new(), merge_stderr }
    }

    /// The ssh argv a ready run of `args` builds.
    fn argv_for(h: &Harness, args: &[&str], token: &str, force_tty: bool) -> Vec<String> {
        let vars: Vec<(String, String)> = h.env.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        let term = h.env.get("TERM").map(String::as_str);
        let i = intro(&b64(env_listing(&vars).as_bytes()), token, "1234512345", "NDZBYUNmR2dLa01ObnFzVHRWdlh4WXk=", &b64(requote(&strings(args)).as_bytes()), term);
        ssh_argv(&parse(&strings(args), B, force_tty).unwrap(), &sanitized(&i))
    }

    #[test]
    fn it2ssh_runs_ssh_like_iterm2() {
        let chosen = label(CHOSEN);
        let args = strings(&["-p", "2222", "user@host"]);
        let base = [("SSH", r"C:\ssh\ssh.exe"), ("TERM", "xterm-256color")];
        assert_eq!(b64(env_listing(&pairs(&base)).as_bytes()), "U1NIPUM6XHNzaFxzc2guZXhlClRFUk09eHRlcm0tMjU2Y29sb3IK");

        let mut h = ready_harness(&[base[0], base[1], (TRANSPORT_VAR, chosen)]);
        assert_eq!(h.with_io(&mut |io| run_with(&args, io)), 0);
        assert_eq!(h.runs.len(), 2);
        assert_eq!(h.runs[0], cmd(r"C:\ssh\ssh.exe", vec![], false, true));
        assert_eq!(h.runs[1], cmd(r"C:\ssh\ssh.exe", argv_for(&h, &["-p", "2222", "user@host"], "none", false), true, false));
        assert!(h.runs[1].args.iter().any(|a| a.contains("it2ssh=none 1234512345 NDZBYUNmR2dLa01ObnFzVHRWdlh4WXk= LXAgMjIyMiB1c2VyQGhvc3Qg\x07")));
        assert_eq!(h.out, b"\x1b]1337;EndSSH=1234512345\x07");
        assert!(h.err.is_empty(), "{:?}", String::from_utf8_lossy(&h.err));
        assert_eq!(h.tty.modes, [Mode::Raw]);
        assert!(h.tty.restored);

        // A secret from Termurion is the token.
        let secret = "0123456789abcdef0123456789abcdef".to_string();
        let mut h = ready_harness(&[base[0], base[1], (TRANSPORT_VAR, chosen)]);
        h.secrets = [Some(secret.clone())].into();
        assert_eq!(h.with_io(&mut |io| run_with(&args, io)), 0);
        assert!(h.runs[1].args.iter().any(|a| a.contains(&format!("it2ssh={secret} "))));
        assert!(h.runs[1] == cmd(r"C:\ssh\ssh.exe", argv_for(&h, &["-p", "2222", "user@host"], &secret, false), true, false));

        // SSH unset or empty: ssh from PATH.
        for ssh in [None, Some("")] {
            let mut env = vec![base[1], (TRANSPORT_VAR, chosen)];
            if let Some(s) = ssh {
                env.push(("SSH", s));
            }
            let mut h = ready_harness(&env);
            assert_eq!(h.with_io(&mut |io| run_with(&args, io)), 0);
            assert_eq!(h.runs[0].program, "ssh");
            assert_eq!(h.runs[1].program, "ssh");
        }

        // Standard input not a console: a second -t, and the wait reads standard input.
        let mut h = ready_harness(&[base[0], base[1], (TRANSPORT_VAR, chosen)]);
        h.stdin_tty = false;
        h.tty.console = false;
        h.tty.replies.clear();
        h.input = b"x\n\nleft".to_vec();
        assert_eq!(h.with_io(&mut |io| run_with(&args, io)), 0);
        assert_eq!(h.runs[1].args, argv_for(&h, &["-p", "2222", "user@host"], "none", true));
        assert_eq!(&h.runs[1].args[..4], strings(&["-p", "2222", "-t", "-t"]).as_slice());
        assert!(h.tty.modes.is_empty());

        // The EndSSH wrap follows TERM=screen* only.
        let mut h = ready_harness(&[base[0], ("TERM", "screen"), (TRANSPORT_VAR, chosen)]);
        assert_eq!(h.with_io(&mut |io| run_with(&args, io)), 0);
        assert_eq!(h.out, b"\x1bPtmux;\x1b\x1b]1337;EndSSH=1234512345\x07\x1b\\");
        let mut h = ready_harness(&[base[0], ("TERM", "tmux-256color"), (TRANSPORT_VAR, chosen)]);
        assert_eq!(h.with_io(&mut |io| run_with(&args, io)), 0);
        assert_eq!(h.out, b"\x1b]1337;EndSSH=1234512345\x07");

        // Not a session of a Termurion SSH profile (no transport, or another one): plain ssh.
        for transport in [None, Some("conpty")] {
            let mut env = vec![base[0], base[1]];
            if let Some(t) = transport {
                env.push((TRANSPORT_VAR, t));
            }
            let mut h = ready_harness(&env);
            h.answers = [Ok(Output { code: 3, stdout: vec![] })].into();
            assert_eq!(h.with_io(&mut |io| run_with(&args, io)), 3);
            assert_eq!(String::from_utf8_lossy(&h.err), format!("{NEEDS_PROFILE}\n"));
            assert_eq!(h.runs, [cmd(r"C:\ssh\ssh.exe", args.clone(), true, false)]);
            assert!(h.out.is_empty());
            assert_eq!(h.secrets.len(), 1, "no secret fetched");
        }
    }

    #[test]
    fn it2ssh_errors_follow_iterm2() {
        let chosen = label(CHOSEN);
        // No arguments: plain ssh's usage, renamed, on standard error; ssh's code.
        let mut h = ready_harness(&[("SSH", r"C:\ssh\ssh.exe"), (TRANSPORT_VAR, chosen)]);
        assert_eq!(h.with_io(&mut |io| run_with(&[], io)), 255);
        assert_eq!(h.runs, [cmd("ssh", vec![], false, true)]);
        assert_eq!(String::from_utf8_lossy(&h.err), usage_text(WIN_USAGE));
        assert!(String::from_utf8_lossy(&h.err).starts_with("usage: it2ssh [-46"));
        let mut h = ready_harness(&[]);
        h.answers = [Ok(Output { code: 0, stdout: b"usage: ssh x\n".to_vec() })].into();
        assert_eq!(h.with_io(&mut |io| run_with(&[], io)), 1);
        let mut h = ready_harness(&[]);
        h.answers = [Err("not found".into())].into();
        assert_eq!(h.with_io(&mut |io| run_with(&[], io)), 127);
        assert_eq!(String::from_utf8_lossy(&h.err), "it2ssh: ssh: not found\n");

        // A non-interactive option: the message on standard output, 1, only the usage probe.
        let mut h = ready_harness(&[(TRANSPORT_VAR, chosen)]);
        assert_eq!(h.with_io(&mut |io| run_with(&strings(&["-N", "h"]), io)), 1);
        assert_eq!(String::from_utf8_lossy(&h.out), format!("{}\n", not_interactive("-N")));
        assert_eq!(h.runs, [cmd("ssh", vec![], false, true)]);

        // ssh cannot start: the error, then EndSSH and the wait as always.
        let mut h = ready_harness(&[(TRANSPORT_VAR, chosen)]);
        h.answers = [Ok(Output { code: 255, stdout: WIN_USAGE.as_bytes().to_vec() }), Err("not found".into())].into();
        assert_eq!(h.with_io(&mut |io| run_with(&strings(&["h"]), io)), 0);
        assert_eq!(String::from_utf8_lossy(&h.err), "it2ssh: not found\n");
        assert_eq!(h.out, b"\x1b]1337;EndSSH=1234512345\x07");
        assert!(h.tty.replies.is_empty(), "the wait read to its end");
        assert!(h.tty.restored);

        // The usage probe fails: no options without a value, every option takes one.
        let mut h = ready_harness(&[(TRANSPORT_VAR, chosen)]);
        h.answers = [Err("x".into()), Ok(Output { code: 0, stdout: vec![] })].into();
        assert_eq!(h.with_io(&mut |io| run_with(&strings(&["-v", "h"]), io)), 0);
        assert_eq!(&h.runs[1].args[..3], strings(&["-v", "h", "-t"]).as_slice());
    }
}
