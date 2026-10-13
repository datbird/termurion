//! The SSH transport probe's pure half (milestone 14a Task 1): the fake remote that prints iTerm2's
//! protocol edge cases and reads the conductor bootstrap, the sshd it runs against, and the verdict
//! on what came back. `ssh_probe_win` runs it; the fixtures are in tests/fixtures/ssh.

use crate::ssh_transport::{ORDER, Transport, conpty_flags};

/// The probe's own sshd, inside WSL, on 127.0.0.1 only.
pub const SSHD_PORT: u16 = 22990;
/// The probe's folder inside WSL (keys, config, pid file, log, remote.sh).
pub const WSL_DIR: &str = "/tmp/termurion-ssh-probe";

/// A DCS 2000p line as conductor.sh prints it (`print_dcs`): raw, ending in a bare LF.
pub const DCS_LINE: &[u8] = b"\x1bP2000pnone 1234512345 NDZBYQ== - LXAgMjIyMiB1c2VyQGhvc3Qg\n";
/// A framed `%output` block for pane 7, as framer.py wraps a pane's output.
pub const OUT_BEGIN: &[u8] = b"\x1b]134;:%output 7 42 -1 1\x1b\\";
pub const OUT_END: &[u8] = b"\x1b]134;:%end 7\x1b\\";
/// A background block (pane 8): its bytes must arrive but must not be rendered by the terminal.
pub const SIDE_BEGIN: &[u8] = b"\x1b]134;:%output 8 43 1 1\x1b\\";
pub const SIDE_END: &[u8] = b"\x1b]134;:%end 8\x1b\\";

/// The main block's payload: every printable ASCII byte, then the edge cases a terminal in the
/// middle would rewrite (CR LF, a bare CR, tab, backspace, bell, SGR, cursor moves, erase, UTF-8,
/// invalid UTF-8, a line longer than the screen).
pub fn main_payload() -> Vec<u8> {
    let mut p: Vec<u8> = (0x20u8..=0x7e).collect();
    for part in [&b"\r\n"[..], b"\rover", b"\t|\x08|\x07|", b"\x1b[31mred\x1b[0m", b"\x1b[5;10H", b"\x1b[2K", "\u{e9}\u{20ac}\u{1f600}".as_bytes(), b"\xc3\x28\xff"] {
        p.extend(part);
    }
    p.extend([b'x'; 300]);
    p.extend(b"\r\n");
    p
}

/// The background block's payload: a title and a clear that must not reach the screen.
pub fn side_payload() -> Vec<u8> {
    b"\x1b]0;evil\x07\x1b[2Jside\r\n".to_vec()
}

/// The conductor bootstrap as iTerm2 types it (`PS:20283-20301`): `^C`, the begin line, the
/// base64 lines, an ESC line.
pub const BOOTSTRAP: &[u8] = b"\x03\n-- BEGIN CONDUCTOR --\nZWNobyBoaQptYWluIGJtOXVaUT09IE1USXpORFV4TWpNME5RPT0gVGtSYVFsbFJQVDA9IFlVYzVlbVJE\nUVQwPQ==\n\x1b\n";
/// Base64 of what it2ssh's remote loop collects from `BOOTSTRAP`: `"\n" + line1 + "\n" + line2`.
pub const BOOTSTRAP_SEEN: &str = "ClpXTm9ieUJvYVFwdFlXbHVJR0p0T1hWYVVUMDlJRTFVU1hwT1JGVjRUV3BOTUU1UlBUMGdWR3RTWVZGc2JGSlFWREE5SUZsVll6VmxiVkpFClVWUXdQUT09";
/// The hex of the lines the remote skipped before `-- BEGIN CONDUCTOR --`: exactly the `^C` byte.
pub const PRE_SEEN: &str = "03";

/// `\ooo` per byte, for a POSIX `printf` format.
pub fn octal(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("\\{b:03o}")).collect()
}

/// The fake remote, run by `sh`: it prints the DCS line, the main block and the side block, reports
/// the tty size before and after the resize, then reads the bootstrap with it2ssh's own loop
/// (`IS:128-159`; the first loop also keeps the lines it skips) and reports what it read.
/// `stty -onlcr -opost` is conductor.sh's (`C:490`, before `print_dcs`): without it the remote tty
/// itself turns every LF into CR LF, which no transport could undo.
pub fn remote_script() -> String {
    let mut main = OUT_BEGIN.to_vec();
    main.extend(main_payload());
    main.extend(OUT_END);
    let mut side = SIDE_BEGIN.to_vec();
    side.extend(side_payload());
    side.extend(SIDE_END);
    let size = "printf '\\033]134;:size %s\\033\\\\' \"$(stty size)\"";
    [
        "stty -echo -isig".to_string(),
        "stty -onlcr -opost".into(),
        format!("printf '{}'", octal(DCS_LINE)),
        format!("printf '{}'", octal(&main)),
        format!("printf '{}'", octal(&side)),
        size.into(),
        "read -r q".into(),
        size.into(),
        "esc=$(printf '\\033')".into(),
        "pre=\"\"".into(),
        "s=\"\";IFS=\"\"; while read -r l;do [ \"$l\" = \"-- BEGIN CONDUCTOR --\" ]&&break; pre=\"$pre$l\"; done; while read -r l; do [ \"$l\" = \"$esc\" ]&&break; s=$(printf \"%s\\n%s\" \"$s\" \"$l\"); done; unset IFS".into(),
        "printf '\\033]134;:pre %s\\033\\\\' \"$(printf %s \"$pre\" | od -An -tx1 | tr -d ' \\n')\"".into(),
        "printf '\\033]134;:got %s\\033\\\\' \"$(printf %s \"$s\" | base64 | tr -d '\\n')\"".into(),
        "exit 0".into(),
    ]
    .join("\n")
        + "\n"
}

/// it2ssh's remote command with its intro replaced by a marker (`IS:152-159`).
pub const ARGV_SANITIZED: &str = "stty -echo -isig; printf \"%s \x1b]134;:argv-ok\x07\"; exit 0";

/// The remote command in it2ssh's own shape (`IS:41,164-172`): `exec sh -c 'eval "$(echo "$0")"'
/// '<sanitized>'`, each word one argument of ssh.exe.
pub fn argv_args() -> Vec<String> {
    vec!["exec".into(), "sh".into(), "-c".into(), "'eval \"$(echo \"$0\")\"'".into(), format!("'{ARGV_SANITIZED}'")]
}

/// What the argv case prints when its command survived CreateProcess, ssh and the remote shell.
pub const ARGV_SEEN: &[u8] = b" \x1b]134;:argv-ok\x07";

/// Sets up and starts the probe's sshd, fed to `wsl.exe -e sh` on stdin: an sshd left by an
/// earlier run is stopped, `WSL_DIR` made (mode 700) with a host key, a client key, its
/// authorized_keys and an sshd_config for this user only, then the installed sshd started.
pub fn sshd_setup(user: &str) -> String {
    let d = WSL_DIR;
    format!(
        "set -e
{SSHD_STOP}
mkdir -m 700 {d}
ssh-keygen -q -t ed25519 -N '' -f {d}/host_key
ssh-keygen -q -t ed25519 -N '' -f {d}/client_key
cp {d}/client_key.pub {d}/authorized_keys
cat > {d}/sshd_config <<'EOF'
Port {SSHD_PORT}
ListenAddress 127.0.0.1
HostKey {d}/host_key
PidFile {d}/sshd.pid
AuthorizedKeysFile {d}/authorized_keys
UsePAM no
StrictModes no
PasswordAuthentication no
KbdInteractiveAuthentication no
PubkeyAuthentication yes
AllowUsers {user}
PrintMotd no
EOF
/usr/sbin/sshd -f {d}/sshd_config -E {d}/sshd.log
"
    )
}

/// Stops the probe's sshd from its pid file and removes `WSL_DIR`.
pub const SSHD_STOP: &str = "if [ -f /tmp/termurion-ssh-probe/sshd.pid ]; then kill \"$(cat /tmp/termurion-ssh-probe/sshd.pid)\" 2>/dev/null || true; fi; rm -rf /tmp/termurion-ssh-probe";

/// Finds the two programs the probe needs inside WSL; installs nothing.
pub const PRECHECK: &str = "command -v /usr/sbin/sshd; command -v ssh-keygen";

/// Why the probe cannot run (ruling "14a probe": nothing is installed), the first missing piece.
pub fn blocked_reason(wsl: bool, sshd: bool, keygen: bool, ssh_exe: bool, sshd_pid: bool, port_open: bool) -> Option<&'static str> {
    [
        (wsl, "wsl.exe is not available"),
        (sshd, "WSL has no /usr/sbin/sshd"),
        (keygen, "WSL has no ssh-keygen"),
        (ssh_exe, "ssh.exe is not on PATH"),
        (sshd_pid, "sshd did not start (no pid file)"),
        (port_open, "sshd is not reachable on 127.0.0.1:22990"),
    ]
    .into_iter()
    .find(|(ok, _)| !ok)
    .map(|(_, text)| text)
}

/// Quotes one argument for CreateProcess with std's rule (`library/std/src/sys/args/windows.rs`,
/// `append_arg`): quoted only when empty or holding a space or tab; a backslash run is doubled only
/// before a quote (escaped) or the closing quote. `git_wrapper::quote_arg` also quotes on a quote,
/// LF or VT, so std's `Command` and it would give ssh.exe different command lines.
pub(crate) fn quote_arg(arg: &str, out: &mut String) {
    let quote = arg.is_empty() || arg.contains([' ', '\t']);
    if quote {
        out.push('"');
    }
    let mut backslashes = 0;
    for c in arg.chars() {
        if c == '\\' {
            backslashes += 1;
        } else {
            if c == '"' {
                out.extend(std::iter::repeat_n('\\', backslashes + 1));
            }
            backslashes = 0;
        }
        out.push(c);
    }
    if quote {
        out.extend(std::iter::repeat_n('\\', backslashes));
        out.push('"');
    }
}

/// The command line for `program` and `args`, each through `quote_arg`.
pub fn cmdline(program: &str, args: &[String]) -> String {
    let mut out = String::new();
    quote_arg(program, &mut out);
    for a in args {
        out.push(' ');
        quote_arg(a, &mut out);
    }
    out
}

/// ssh.exe's arguments for the probe: no user config, the probe's key, host key checks off, one
/// `-t` with a console, two on pipes (a remote pty even though stdin is not a terminal).
pub fn ssh_args(t: Transport, key: &str, user: &str, remote: &[String]) -> Vec<String> {
    let port = SSHD_PORT.to_string();
    let mut args: Vec<String> = [
        "-F", "NUL", "-i", key, "-p", &port, "-o", "StrictHostKeyChecking=no", "-o", "UserKnownHostsFile=NUL", "-o", "IdentitiesOnly=yes", "-o", "BatchMode=yes", "-o", "LogLevel=ERROR", "-t",
    ]
    .map(String::from)
    .to_vec();
    if conpty_flags(t).is_none() {
        args.push("-t".into());
    }
    args.push(format!("{user}@127.0.0.1"));
    args.extend(remote.iter().cloned());
    args
}

/// What one transport carried: each protocol piece intact or not, and the tty sizes the remote
/// saw before and after the resize (recorded, never gating: pipes has no resize).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Verdict {
    pub dcs: bool,
    pub output: bool,
    pub side: bool,
    pub order: bool,
    pub ctrl_c: bool,
    pub size_before: Option<(u16, u16)>,
    pub size_after: Option<(u16, u16)>,
}

impl Verdict {
    pub fn passes(&self) -> bool {
        self.dcs && self.output && self.side && self.order && self.ctrl_c
    }
}

fn position(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    hay.get(from..)?.windows(needle.len()).position(|w| w == needle).map(|i| i + from)
}

/// The bytes between `begin` and the next `end`, and where `begin` starts.
fn block<'a>(raw: &'a [u8], begin: &[u8], end: &[u8]) -> Option<(usize, &'a [u8])> {
    let at = position(raw, begin, 0)?;
    let start = at + begin.len();
    let stop = position(raw, end, start)?;
    Some((at, &raw[start..stop]))
}

/// The texts of every `ESC ] 134 ; : <name> <text> ESC \` report, in order.
pub(crate) fn reports(raw: &[u8], name: &str) -> Vec<String> {
    let head = format!("\x1b]134;:{name} ");
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(at) = position(raw, head.as_bytes(), from) {
        let start = at + head.len();
        let Some(stop) = position(raw, b"\x1b\\", start) else { break };
        out.push(String::from_utf8_lossy(&raw[start..stop]).into_owned());
        from = stop;
    }
    out
}

fn size(text: &str) -> Option<(u16, u16)> {
    let mut words = text.split_whitespace().map(|w| w.parse::<u16>().ok());
    Some((words.next()??, words.next()??))
}

/// Judges one transport's raw capture.
pub fn check(raw: &[u8]) -> Verdict {
    let main = block(raw, OUT_BEGIN, OUT_END);
    let side = block(raw, SIDE_BEGIN, SIDE_END);
    let sizes = reports(raw, "size");
    let pre = reports(raw, "pre");
    let got = reports(raw, "got");
    Verdict {
        dcs: position(raw, DCS_LINE, 0).is_some(),
        output: main.is_some_and(|(_, b)| b == main_payload()),
        side: side.is_some_and(|(_, b)| b == side_payload()),
        order: matches!((main, side), (Some((m, _)), Some((s, _))) if m < s),
        ctrl_c: pre.first().is_some_and(|p| p == PRE_SEEN) && got.first().is_some_and(|g| g == BOOTSTRAP_SEEN),
        size_before: sizes.first().and_then(|s| size(s)),
        size_after: sizes.get(1).and_then(|s| size(s)),
    }
}

/// The argv case's marker arrived.
pub fn argv_ok(raw: &[u8]) -> bool {
    position(raw, ARGV_SEEN, 0).is_some()
}

/// The first transport in ruling 1's order that passed.
pub fn choose(results: &[(Transport, Verdict)]) -> Option<Transport> {
    ORDER.into_iter().find(|t| results.iter().any(|(r, v)| r == t && v.passes()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ssh_transport::Transport;

    fn exact() -> Vec<u8> {
        let mut raw = b"\x1b[?25l\x1b[H".to_vec();
        raw.extend(DCS_LINE);
        raw.extend(OUT_BEGIN);
        raw.extend(main_payload());
        raw.extend(OUT_END);
        raw.extend(SIDE_BEGIN);
        raw.extend(side_payload());
        raw.extend(SIDE_END);
        raw.extend(b"\x1b]134;:size 40 120\x1b\\");
        raw.extend(b"\x1b]134;:size 30 100\x1b\\");
        raw.extend(b"\x1b]134;:pre 03\x1b\\");
        raw.extend(got(BOOTSTRAP_SEEN));
        raw
    }

    fn got(text: &str) -> Vec<u8> {
        let mut v = b"\x1b]134;:got ".to_vec();
        v.extend(text.as_bytes());
        v.extend(b"\x1b\\");
        v
    }

    fn find(hay: &[u8], needle: &[u8]) -> usize {
        hay.windows(needle.len()).position(|w| w == needle).unwrap()
    }

    /// `raw` with the first `from` at or after `start` replaced by `to`.
    fn swap(raw: &[u8], start: usize, from: &[u8], to: &[u8]) -> Vec<u8> {
        let at = start + find(&raw[start..], from);
        [&raw[..at], to, &raw[at + from.len()..]].concat()
    }

    fn all_true(v: &Verdict) -> bool {
        v.dcs && v.output && v.side && v.order && v.ctrl_c
    }

    #[test]
    fn check_passes_an_exact_stream() {
        let v = check(&exact());
        assert!(all_true(&v), "{v:?}");
        assert_eq!(v.size_before, Some((40, 120)));
        assert_eq!(v.size_after, Some((30, 100)));
        assert!(v.passes());
    }

    #[test]
    fn check_fails_a_rewritten_stream() {
        let raw = exact();
        let main_at = find(&raw, OUT_BEGIN) + OUT_BEGIN.len();
        let v = check(&swap(&raw, main_at, b"\r\n", b"\n"));
        assert!(!v.output && v.dcs && v.side && v.order && v.ctrl_c, "{v:?}");
        assert!(!v.passes());

        let side_at = find(&raw, SIDE_BEGIN);
        let v = check(&swap(&raw, side_at, b"\x1b[2J", b""));
        assert!(!v.side && v.output && v.dcs && v.order && v.ctrl_c, "{v:?}");
        assert!(!v.passes());

        let mut dcs_crlf = DCS_LINE[..DCS_LINE.len() - 1].to_vec();
        dcs_crlf.extend(b"\r\n");
        let v = check(&swap(&raw, 0, DCS_LINE, &dcs_crlf));
        assert!(!v.dcs && v.output && v.side && v.order && v.ctrl_c, "{v:?}");
        assert!(!v.passes());

        let mut main_block = OUT_BEGIN.to_vec();
        main_block.extend(main_payload());
        main_block.extend(OUT_END);
        let mut side_block = SIDE_BEGIN.to_vec();
        side_block.extend(side_payload());
        side_block.extend(SIDE_END);
        let both = [main_block.as_slice(), side_block.as_slice()].concat();
        let swapped = [side_block.as_slice(), main_block.as_slice()].concat();
        let v = check(&swap(&raw, 0, &both, &swapped));
        assert!(!v.order && v.output && v.side && v.dcs && v.ctrl_c, "{v:?}");
        assert!(!v.passes());

        let v = check(&swap(&raw, 0, &got(BOOTSTRAP_SEEN), b""));
        assert!(!v.ctrl_c && v.output && v.side && v.dcs && v.order, "{v:?}");
        assert!(!v.passes());
        let v = check(&swap(&raw, 0, &got(BOOTSTRAP_SEEN), &got("ClpXTm9ieUJvYVFw")));
        assert!(!v.ctrl_c, "{v:?}");
        assert!(!v.passes());

        for pre in [&b"\x1b]134;:pre \x1b\\"[..], b"\x1b]134;:pre 0a\x1b\\", b"\x1b]134;:pre 5e43\x1b\\", b""] {
            let v = check(&swap(&raw, 0, b"\x1b]134;:pre 03\x1b\\", pre));
            assert!(!v.ctrl_c && v.output && v.side && v.dcs && v.order, "{:?}: {v:?}", String::from_utf8_lossy(pre));
            assert!(!v.passes());
        }
    }

    #[test]
    fn blocked_follows_the_ruling() {
        assert_eq!(blocked_reason(true, true, true, true, true, true), None);
        let texts = [
            "wsl.exe is not available",
            "WSL has no /usr/sbin/sshd",
            "WSL has no ssh-keygen",
            "ssh.exe is not on PATH",
            "sshd did not start (no pid file)",
            "sshd is not reachable on 127.0.0.1:22990",
        ];
        for (i, want) in texts.iter().enumerate() {
            let mut a = [true; 6];
            a[i] = false;
            assert_eq!(blocked_reason(a[0], a[1], a[2], a[3], a[4], a[5]), Some(*want), "argument {i}");
        }
        assert_eq!(blocked_reason(true, false, true, false, true, false), Some(texts[1]));
        assert_eq!(blocked_reason(false, false, false, false, false, false), Some(texts[0]));
        for word in ["apt", "install", "sudo"] {
            assert!(!PRECHECK.contains(word), "{word}");
        }
    }

    #[test]
    fn cmdline_quotes_like_std() {
        let args: Vec<String> = ["a b", "x\"y", "", r"c:\p\", "a b\\"].map(String::from).to_vec();
        assert_eq!(cmdline("ssh.exe", &args), r#"ssh.exe "a b" x\"y "" c:\p\ "a b\\""#);
    }

    #[test]
    fn payloads_are_the_protocol_edge_cases() {
        let mut want: Vec<u8> = (0x20u8..=0x7e).collect();
        for part in [&b"\r\n"[..], b"\rover", b"\t|\x08|\x07|", b"\x1b[31mred\x1b[0m", b"\x1b[5;10H", b"\x1b[2K", "\u{e9}\u{20ac}\u{1f600}".as_bytes(), b"\xc3\x28\xff"] {
            want.extend(part);
        }
        want.extend([b'x'; 300]);
        want.extend(b"\r\n");
        assert_eq!(main_payload(), want);
        assert_eq!(side_payload(), b"\x1b]0;evil\x07\x1b[2Jside\r\n");
    }

    #[test]
    fn remote_script_prints_the_payloads() {
        assert_eq!(octal(b"A\x1b\xff"), "\\101\\033\\377");
        let s = remote_script();
        assert_eq!(s.lines().next(), Some("stty -echo -isig"));
        for part in [
            octal(DCS_LINE),
            octal(OUT_BEGIN),
            octal(&main_payload()),
            octal(SIDE_END),
            "\"$(stty size)\"".into(),
            "-- BEGIN CONDUCTOR --".into(),
            "esc=$(printf '\\033')".into(),
            "[ \"$l\" = \"$esc\" ]&&break".into(),
            "pre=\"$pre$l\"".into(),
            "od -An -tx1".into(),
            "exit 0".into(),
        ] {
            assert!(s.contains(&part), "{part:?} in {s}");
        }
    }

    #[test]
    fn ssh_args_follow_the_probe() {
        let remote: Vec<String> = vec!["sh".into(), "/tmp/termurion-ssh-probe/remote.sh".into()];
        let want: Vec<String> = [
            "-F", "NUL", "-i", r"C:\T\k", "-p", "22990", "-o", "StrictHostKeyChecking=no", "-o", "UserKnownHostsFile=NUL", "-o", "IdentitiesOnly=yes", "-o",
            "BatchMode=yes", "-o", "LogLevel=ERROR", "-t", "-t", "rob@127.0.0.1", "sh", "/tmp/termurion-ssh-probe/remote.sh",
        ]
        .map(String::from)
        .to_vec();
        assert_eq!(ssh_args(Transport::Pipes, r"C:\T\k", "rob", &remote), want);
        for t in [Transport::Conpty, Transport::Passthrough] {
            let got = ssh_args(t, r"C:\T\k", "rob", &remote);
            assert_eq!(got.iter().filter(|a| *a == "-t").count(), 1, "{t:?}");
            let one_t: Vec<String> = want.iter().enumerate().filter(|(i, _)| *i != 16).map(|(_, a)| a.clone()).collect();
            assert_eq!(got, one_t, "{t:?}");
        }
        let setup = sshd_setup("rob");
        for part in [
            "Port 22990",
            "ListenAddress 127.0.0.1",
            "UsePAM no",
            "StrictModes no",
            "PasswordAuthentication no",
            "AllowUsers rob",
            "/usr/sbin/sshd -f /tmp/termurion-ssh-probe/sshd_config -E /tmp/termurion-ssh-probe/sshd.log",
        ] {
            assert!(setup.contains(part), "{part:?} in {setup}");
        }
    }

    #[test]
    fn choose_follows_the_ruling_order() {
        let pass = check(&exact());
        let fail = Verdict::default();
        let with = |p: bool, c: bool, q: bool| {
            let v = |ok: bool| if ok { pass.clone() } else { fail.clone() };
            vec![(Transport::Passthrough, v(p)), (Transport::Conpty, v(c)), (Transport::Pipes, v(q))]
        };
        assert_eq!(choose(&with(false, true, true)), Some(Transport::Conpty));
        assert_eq!(choose(&with(false, false, true)), Some(Transport::Pipes));
        assert_eq!(choose(&with(true, true, true)), Some(Transport::Passthrough));
        assert_eq!(choose(&with(false, false, false)), None);
    }

    /// The probe run on EXTREME (2026-09-27, shipped ConPTY 1.24.260710001, OpenSSH_for_Windows
    /// 9.5p2, sshd in WSL Ubuntu-Personal). Both ConPTY modes carry every piece but the main
    /// payload, whose invalid UTF-8 comes back as U+FFFD; pipes ends at once ("GetConsoleMode on
    /// console input handle failed with 6") and never forwards a byte on anonymous pipes. The pipes
    /// investigation's named pipes (overlapped named pipes both ways, a console with no window;
    /// test-only evidence, not a transport) carry everything byte for byte; the remote pty is 0x0
    /// and never resized (no console).
    #[test]
    fn probe_fixtures_follow_the_ruling() {
        let conpty_like = Verdict { dcs: true, output: false, side: true, order: true, ctrl_c: true, size_before: Some((40, 120)), size_after: Some((30, 100)) };
        type Fixture = (Transport, &'static [u8], &'static [u8], Verdict, bool);
        let fixtures: [Fixture; 3] = [
            (Transport::Passthrough, include_bytes!("../tests/fixtures/ssh/passthrough.bin"), include_bytes!("../tests/fixtures/ssh/argv-passthrough.bin"), conpty_like.clone(), true),
            (Transport::Conpty, include_bytes!("../tests/fixtures/ssh/conpty.bin"), include_bytes!("../tests/fixtures/ssh/argv-conpty.bin"), conpty_like.clone(), true),
            (Transport::Pipes, include_bytes!("../tests/fixtures/ssh/pipes.bin"), include_bytes!("../tests/fixtures/ssh/argv-pipes.bin"), Verdict::default(), false),
        ];
        let mut results = Vec::new();
        for (t, raw, argv, want, argv_want) in fixtures.clone() {
            assert_eq!(check(raw), want, "{t:?}");
            assert_eq!(argv_ok(argv), argv_want, "{t:?}");
            results.push((t, check(raw)));
        }
        assert_eq!(choose(&results), None, "ruling 1's three as the brief measured them");
        // Ruling 1 (Robert): passthrough, accepting U+FFFD for invalid UTF-8 (pinned below) over
        // named pipes' 0x0 remote pty with no resize.
        assert_eq!(crate::ssh_transport::CHOSEN, Transport::Passthrough);
        assert!(results[0].1.dcs && results[0].1.side && results[0].1.order && results[0].1.ctrl_c && !results[0].1.output);
        // The named pipes (evidence only): every check passes, and nothing but the remote's own
        // bytes arrives, from the first byte on.
        let named: &[u8] = include_bytes!("../tests/fixtures/ssh/named-pipes.bin");
        let named_argv: &[u8] = include_bytes!("../tests/fixtures/ssh/argv-named-pipes.bin");
        let named_verdict = check(named);
        assert_eq!(named_verdict, Verdict { dcs: true, output: true, side: true, order: true, ctrl_c: true, size_before: Some((0, 0)), size_after: Some((0, 0)) });
        assert!(named_verdict.passes());
        assert!(named.starts_with(DCS_LINE));
        assert_eq!(named_argv, ARGV_SEEN);
        // What ConPTY changed: only the invalid UTF-8, each bad byte turned into U+FFFD.
        let bad = b"\xc3\x28\xff";
        let at = find(&main_payload(), bad);
        let want = [&main_payload()[..at], "\u{fffd}(\u{fffd}".as_bytes(), &main_payload()[at + bad.len()..]].concat();
        for raw in [fixtures[0].1, fixtures[1].1] {
            let start = find(raw, OUT_BEGIN) + OUT_BEGIN.len();
            let end = start + find(&raw[start..], OUT_END);
            assert_eq!(&raw[start..end], &want[..]);
        }
        assert!(fixtures[2].1.starts_with(b"GetConsoleMode on console input handle failed with 6"));
    }

    #[test]
    fn argv_ok_finds_the_marker() {
        assert!(argv_ok(b"\x1b[H \x1b]134;:argv-ok\x07\r\n"));
        assert!(!argv_ok(b"\x1b[H \x1b]134;:argv-ok"));
    }
}
