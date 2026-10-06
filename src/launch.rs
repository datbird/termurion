//! The command line and the message a second `tandem.exe` hands to the running one (m13v1 Task 1).
//! Both are untrusted: the message arrives on a pipe any program of the same user can open, so
//! `decode` is a strict parser of a closed set of kinds (never serde on foreign bytes) and a
//! request names no command to run.

use std::path::{Path, PathBuf};

/// What a second `tandem.exe` asks the running one to do. A closed set: none names a command, a
/// directory or an environment to run (P3); a profile or arrangement is looked up in Tandem's own
/// store by the app (P22).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    /// A bare start: iTerm2's reopen (the dock click).
    Reopen,
    Open { paths: Vec<PathBuf> },
    Url { url: String },
    NewWindow { guid: Option<String> },
    NewTab { guid: Option<String> },
    Arrangement { name: String },
    /// COM starting Tandem to take a console hand-off: handled by this process, never forwarded.
    Embedding,
}

pub const USAGE: &str = "Usage: tandem.exe [--url URL | --new-window [GUID] | --new-tab [GUID] | --arrangement NAME | PATH...]";

/// The whole message at most this long (P2); the 8 KiB URL cap stays inside `url_launch::url_ok`.
pub const MAX_MESSAGE: usize = 16 * 1024;
/// Paths one `Open` carries (P2): an Explorer selection opens the first 16.
pub const MAX_PATHS: usize = 16;
/// A path is at most this many UTF-16 units (the Windows extended limit).
pub const MAX_PATH_UNITS: usize = 32_767;
/// An arrangement name, in characters.
pub const MAX_NAME: usize = 1024;
/// A profile guid, in characters.
pub const MAX_GUID: usize = 64;

/// A request is forwarded to a running Tandem unless it is the COM start.
pub fn forwardable(r: &Request) -> bool {
    !matches!(r, Request::Embedding)
}

/// `forwardable` for a process start: the relaunch after Import All Settings (`WAIT_PID_VAR`)
/// waited for the old Tandem to end and must run, never hand its request to the one it replaced (B2).
pub fn forwardable_start(r: &Request, relaunch: bool) -> bool {
    forwardable(r) && !relaunch
}

/// The banner for requests dropped by the flood cap.
pub fn ignored_banner(n: usize) -> String {
    format!("{n} launch request{} ignored", if n == 1 { "" } else { "s" })
}

fn is_embedding(arg: &str) -> bool {
    arg.eq_ignore_ascii_case("-embedding") || arg.eq_ignore_ascii_case("/embedding")
}

fn guid_ok(g: &str) -> bool {
    (1..=MAX_GUID).contains(&g.chars().count())
        && g.starts_with(|c: char| c.is_ascii_alphanumeric() || c == '{')
        && g.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '{' | '}'))
}

fn name_ok(n: &str) -> bool {
    n.chars().count() <= MAX_NAME && crate::pathnav::safe_text(n).is_some()
}

fn path_ok(p: &str) -> bool {
    !p.is_empty() && p.encode_utf16().count() <= MAX_PATH_UNITS && !p.contains('"') && crate::pathnav::safe_text(p).is_some()
}

/// Whether a request holds only what the pipe and the command line accept.
fn valid(r: &Request) -> bool {
    match r {
        Request::Reopen => true,
        Request::Open { paths } => {
            (1..=MAX_PATHS).contains(&paths.len()) && paths.iter().all(|p| p.to_str().is_some_and(path_ok))
        }
        Request::Url { url } => crate::url_launch::url_ok(url),
        Request::NewWindow { guid } | Request::NewTab { guid } => guid.as_deref().is_none_or(guid_ok),
        Request::Arrangement { name } => name_ok(name),
        Request::Embedding => false,
    }
}

/// The request for the arguments after argv0. `--url` is exactly `url_launch::url_argument`'s one
/// rule; `-Embedding` or `/Embedding` (any case) only as the sole argument (P24); anything that
/// is not a flag is a path, joined to `cwd` when relative. Explorer's `"D:\"` reaches the argv
/// parser as `D:"`: that one shape is repaired, any other quote refuses.
pub fn parse(args: &[String], cwd: &Path) -> Result<Request, String> {
    let usage = || Err(USAGE.to_string());
    let Some(first) = args.first() else { return Ok(Request::Reopen) };
    if first == "--arrangement" {
        return match args {
            [_, name] if name_ok(name) => Ok(Request::Arrangement { name: name.clone() }),
            _ => usage(),
        };
    }
    let argv: Vec<std::ffi::OsString> = std::iter::once("tandem.exe".into()).chain(args.iter().map(std::ffi::OsString::from)).collect();
    match crate::url_launch::url_argument(&argv) {
        crate::url_launch::UrlArg::Url(url) => return Ok(Request::Url { url }),
        crate::url_launch::UrlArg::Refused(_) => return usage(),
        crate::url_launch::UrlArg::None => {}
    }
    if args.iter().any(|a| is_embedding(a)) {
        return if args.len() == 1 { Ok(Request::Embedding) } else { usage() };
    }
    if first == "--new-window" || first == "--new-tab" {
        let guid = match &args[1..] {
            [] => None,
            [g] if guid_ok(g) => Some(g.clone()),
            _ => return usage(),
        };
        return Ok(if first == "--new-window" { Request::NewWindow { guid } } else { Request::NewTab { guid } });
    }
    let mut paths = Vec::new();
    for arg in args {
        if arg.starts_with("--") {
            return usage();
        }
        let repaired = match arg.as_bytes() {
            [d, b':', b'"'] if d.is_ascii_alphabetic() => format!("{}:\\", *d as char),
            _ => arg.clone(),
        };
        if !path_ok(&repaired) {
            return usage();
        }
        let path = PathBuf::from(&repaired);
        paths.push(if path.is_absolute() { path } else { cwd.join(path) });
    }
    paths.truncate(MAX_PATHS);
    let request = Request::Open { paths };
    if valid(&request) { Ok(request) } else { usage() }
}

/// The message for `r`: a kind word, then one payload line per field, each line ending in "\n"
/// (`open-url\nURL\n` is 14e's framing). None for `Embedding` and for a request that holds
/// anything `decode` would refuse.
pub fn encode(r: &Request) -> Option<Vec<u8>> {
    if !valid(r) {
        return None;
    }
    let text = match r {
        Request::Reopen => "reopen\n".to_string(),
        Request::Url { url } => format!("open-url\n{url}\n"),
        Request::NewWindow { guid } => format!("new-window\n{}\n", guid.as_deref().unwrap_or("")),
        Request::NewTab { guid } => format!("new-tab\n{}\n", guid.as_deref().unwrap_or("")),
        Request::Arrangement { name } => format!("arrangement\n{name}\n"),
        Request::Open { paths } => {
            let mut s = format!("open\n{}\n", paths.len());
            for p in paths {
                s.push_str(p.to_str()?);
                s.push('\n');
            }
            s
        }
        Request::Embedding => return None,
    };
    (text.len() <= MAX_MESSAGE).then(|| text.into_bytes())
}

/// The one request a message is, or None: unknown kind, a missing or extra line, bad UTF-8, a
/// control, bidi or zero-width character in a line, a second message, or more than `MAX_MESSAGE`.
pub fn decode(bytes: &[u8]) -> Option<Request> {
    if bytes.len() > MAX_MESSAGE {
        return None;
    }
    let text = std::str::from_utf8(bytes).ok()?;
    let lines: Vec<&str> = text.strip_suffix('\n')?.split('\n').collect();
    let (kind, payload) = lines.split_first()?;
    let request = match (*kind, payload) {
        ("reopen", []) => Request::Reopen,
        ("open-url", [url]) => Request::Url { url: url.to_string() },
        ("new-window", [g]) => Request::NewWindow { guid: (!g.is_empty()).then(|| g.to_string()) },
        ("new-tab", [g]) => Request::NewTab { guid: (!g.is_empty()).then(|| g.to_string()) },
        ("arrangement", [name]) => Request::Arrangement { name: name.to_string() },
        ("open", [count, paths @ ..]) => {
            let n: usize = count.parse().ok().filter(|n| count.bytes().all(|b| b.is_ascii_digit()) && *n == paths.len())?;
            if n == 0 {
                return None;
            }
            Request::Open { paths: paths.iter().map(PathBuf::from).collect() }
        }
        _ => return None,
    };
    // A line with a control character is hostile whatever the kind (the payload lines never hold
    // a newline, so a control here is a tab, an escape or a NUL).
    (valid(&request) && payload.iter().all(|l| crate::pathnav::safe_text(l).is_some())).then_some(request)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    /// The Windows argv rules (CommandLineToArgvW for the cases `quote_arg` produces): a quote
    /// toggles, `2n` backslashes before a quote are `n`, `2n+1` are `n` and a literal quote.
    fn split_args(line: &str) -> Vec<String> {
        let chars: Vec<char> = line.chars().collect();
        let (mut out, mut cur, mut open, mut have, mut i) = (Vec::new(), String::new(), false, false, 0);
        while i < chars.len() {
            match chars[i] {
                '\\' => {
                    let mut n = 0;
                    while i < chars.len() && chars[i] == '\\' {
                        n += 1;
                        i += 1;
                    }
                    have = true;
                    if i < chars.len() && chars[i] == '"' {
                        cur.extend(std::iter::repeat_n('\\', n / 2));
                        if n % 2 == 1 {
                            cur.push('"');
                            i += 1;
                        }
                    } else {
                        cur.extend(std::iter::repeat_n('\\', n));
                    }
                    continue;
                }
                '"' => {
                    open = !open;
                    have = true;
                }
                ' ' if !open => {
                    if have {
                        out.push(std::mem::take(&mut cur));
                        have = false;
                    }
                }
                c => {
                    cur.push(c);
                    have = true;
                }
            }
            i += 1;
        }
        if have {
            out.push(cur);
        }
        out
    }

    fn args(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    /// Review Focus 1: the command line is a closed set of shapes.
    #[test]
    fn requests_follow_the_command_line() {
        let cwd = Path::new(r"C:\w");
        assert_eq!(parse(&[], cwd), Ok(Request::Reopen));
        assert_eq!(parse(&args(&["a.ps1"]), cwd), Ok(Request::Open { paths: vec![PathBuf::from(r"C:\w\a.ps1")] }));
        assert_eq!(parse(&args(&["D:\\x", "e.sh"]), cwd), Ok(Request::Open { paths: vec![PathBuf::from(r"D:\x"), PathBuf::from(r"C:\w\e.sh")] }));
        assert_eq!(parse(&args(&["--url", "ssh://h"]), cwd), Ok(Request::Url { url: "ssh://h".into() }));
        assert_eq!(parse(&args(&["--url"]), cwd), Err(USAGE.to_string()));
        assert_eq!(parse(&args(&["--new-window"]), cwd), Ok(Request::NewWindow { guid: None }));
        assert_eq!(parse(&args(&["--new-tab", "G"]), cwd), Ok(Request::NewTab { guid: Some("G".into()) }));
        assert_eq!(parse(&args(&["--arrangement", "Dev"]), cwd), Ok(Request::Arrangement { name: "Dev".into() }));
        for e in ["-Embedding", "/embedding", "-EMBEDDING"] {
            assert_eq!(parse(&args(&[e]), cwd), Ok(Request::Embedding), "{e}");
        }
        assert_eq!(parse(&args(&["--bogus"]), cwd), Err(USAGE.to_string()));
        assert!(!forwardable(&Request::Embedding));
        for r in [Request::Reopen, Request::Open { paths: vec![] }, Request::Url { url: "ssh://h".into() }, Request::NewWindow { guid: None }, Request::NewTab { guid: None }, Request::Arrangement { name: "a".into() }] {
            assert!(forwardable(&r), "{r:?}");
        }
        assert_eq!(USAGE, "Usage: tandem.exe [--url URL | --new-window [GUID] | --new-tab [GUID] | --arrangement NAME | PATH...]");
    }

    /// P24: `-Embedding` only alone; the Explorer drive-root quirk is repaired, any other quote refused.
    #[test]
    fn hostile_command_lines_are_refused() {
        let cwd = Path::new(r"C:\w");
        for bad in [
            &["-Embedding", "a.ps1"][..],
            &["a.ps1", "/Embedding"],
            &["--new-window", "G", "H"],
            &["--new-tab", "a b"],
            &["--new-tab", "-x"],
            &["--arrangement"],
            &["--arrangement", "a", "b"],
            &["--arrangement", "a\u{202e}b"],
            &["a\"b"],
            &["a\u{1b}b"],
            &["a\nb"],
            &["--url", "ssh://h", "x"],
            &["ssh://h"],
            &["--bogus", "a.ps1"],
        ] {
            assert_eq!(parse(&args(bad), cwd), Err(USAGE.to_string()), "{bad:?}");
        }
        // Explorer on a drive root passes `"D:\"`, which the argv parser reads as `D:"`.
        assert_eq!(parse(&args(&["D:\""]), cwd), Ok(Request::Open { paths: vec![PathBuf::from(r"D:\")] }));
        assert_eq!(parse(&args(&["DD:\""]), cwd), Err(USAGE.to_string()));
        let long = "a".repeat(32_768);
        assert!(parse(&args(&[&long]), cwd).is_err());
        let many: Vec<String> = (0..20).map(|i| format!("f{i}.sh")).collect();
        match parse(&many, cwd) {
            Ok(Request::Open { paths }) => assert_eq!(paths.len(), MAX_PATHS, "an Explorer selection opens the first 16"),
            other => panic!("{other:?}"),
        }
    }

    /// B10: a Jump List name goes through `quote_arg`, then the parser, and comes back the same.
    #[test]
    fn jump_list_args_round_trip_through_the_parser() {
        let cwd = Path::new(r"C:\w");
        for name in ["a\"b", "a\\", "a b\\", "--url", "--new-window", "", &"n".repeat(300)] {
            let mut line = String::from("--arrangement ");
            crate::ssh_probe::quote_arg(name, &mut line);
            let parsed = split_args(&line);
            assert_eq!(parse(&parsed, cwd), Ok(Request::Arrangement { name: name.to_string() }), "{name:?}");
        }
    }

    #[test]
    fn forwarding_round_trips() {
        for r in [
            Request::Reopen,
            Request::Open { paths: vec![PathBuf::from(r"C:\w\a.ps1"), PathBuf::from(r"\\?\C:\x")] },
            Request::Url { url: "ssh://u@h:22/x".into() },
            Request::NewWindow { guid: None },
            Request::NewWindow { guid: Some("ABC-123".into()) },
            Request::NewTab { guid: None },
            Request::NewTab { guid: Some("ABC-123".into()) },
            Request::Arrangement { name: "My Dev".into() },
            Request::Arrangement { name: String::new() },
        ] {
            let bytes = encode(&r).unwrap_or_else(|| panic!("{r:?}"));
            assert_eq!(decode(&bytes), Some(r.clone()), "{r:?}");
        }
        assert_eq!(encode(&Request::Reopen).as_deref(), Some(&b"reopen\n"[..]));
        assert_eq!(encode(&Request::Url { url: "ssh://h".into() }).as_deref(), Some(&b"open-url\nssh://h\n"[..]));
        assert_eq!(encode(&Request::Embedding), None, "Embedding is never forwarded");
        assert_eq!(decode(b"junk"), None);
        assert_eq!(decode(b""), None);
    }

    /// P2: a hostile message of any kind is dropped whole.
    #[test]
    fn decode_refuses_hostile_messages() {
        let long = format!("arrangement\n{}\n", "a".repeat(MAX_MESSAGE));
        let paths: String = (0..17).map(|i| format!("C:\\f{i}\n")).collect();
        for bad in [
            b"nonsense\nx\n".to_vec(),
            b"reopen\nextra\n".to_vec(),
            b"reopen\nreopen\n".to_vec(),
            b"reopen".to_vec(),
            b"open-url\nssh://h\nssh://h2\n".to_vec(),
            b"open-url\nhttp://h\n".to_vec(),
            b"arrangement\na\0b\n".to_vec(),
            "arrangement\na\u{202e}b\n".as_bytes().to_vec(),
            "arrangement\na\u{200b}b\n".as_bytes().to_vec(),
            b"arrangement\n\xff\xfe\n".to_vec(),
            b"arrangement\nx\ny\n".to_vec(),
            b"new-tab\n".to_vec(),
            b"new-tab\nbad guid\n".to_vec(),
            b"new-window\nA\nB\n".to_vec(),
            b"open\n0\n".to_vec(),
            b"open\n2\nC:\\a\n".to_vec(),
            b"open\n1\nC:\\a\nC:\\b\n".to_vec(),
            b"open\n1\nC:\\a\"b\n".to_vec(),
            b"open\nx\nC:\\a\n".to_vec(),
            format!("open\n17\n{paths}").into_bytes(),
            long.into_bytes(),
        ] {
            assert_eq!(decode(&bad), None, "{:?}", String::from_utf8_lossy(&bad[..bad.len().min(60)]));
        }
    }

    /// B2: a restart after Import All Settings never forwards; a bare start does.
    #[test]
    fn restart_after_import_never_forwards() {
        assert!(forwardable_start(&Request::Reopen, false));
        assert!(!forwardable_start(&Request::Reopen, true));
        assert!(!forwardable_start(&Request::Embedding, false));
        assert!(forwardable_start(&Request::NewTab { guid: None }, false));
    }

    #[test]
    fn the_ignored_banner_counts() {
        assert_eq!(ignored_banner(1), "1 launch request ignored");
        assert_eq!(ignored_banner(3), "3 launch requests ignored");
    }

    /// P3: no request kind starts a program from this module.
    #[test]
    fn no_request_runs_a_command() {
        let src = include_str!("launch.rs");
        let code = &src[..src.find("#[cfg(test)]").unwrap()];
        for word in [concat!("Command", "::new"), concat!("Shell", "Execute"), concat!("Create", "Process"), concat!("cmd", ".exe")] {
            assert!(!code.contains(word), "{word}");
        }
    }
}
