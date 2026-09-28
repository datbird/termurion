//! The writing tools' pure parts (`PTYSession.swift` `*RemoteCommand`, `AIBackslashEscapes.swift`):
//! the escape decoder, the line-end rule, the staged-line accumulator, the file path rule and
//! the man page rule. The App side is `app/ai_tools_app.rs`; every byte a tool sends goes
//! through `App::user_write(pane, bytes, Write::Agent)`.

use crate::ai_shell::ShellKind;
use std::path::{Path, PathBuf};

pub const COPIED: &str = "Copied";
pub const DONE: &str = "Done";
pub const RESTARTING: &str = "The session is being restarted.";
pub const NOT_RESTARTABLE: &str = "This session cannot be restarted.";

/// What `insert_text_at_cursor` answers, and what the chat shows, for a text holding a line
/// end (ruling R12cd-2, Tandem's texts).
pub const LINE_END_REFUSED: &str = "Error: insert_text_at_cursor cannot type a line break (carriage return, newline or Ctrl+O). To run a command, use execute_command, which is safety checked.";
pub const LINE_END_NOTICE: &str = "AI tried to type a line break, so nothing was typed.";

/// `delete_current_line` types Ctrl+U (`PS:817`).
pub const DELETE_LINE: &[u8] = b"\x15";

pub fn inserted(n: usize) -> String {
    format!("Inserted {n} characters.")
}

/// The malformed-escape answer the model reads (`PS:797`).
pub fn escape_error(e: &str) -> String {
    format!("Error: {e}. Try again with a corrected text argument.")
}

/// The notice for a malformed escape.
pub const ESCAPE_NOTICE: &str = "AI tried to insert text but the escape sequence was malformed.";

pub fn create_failed(e: &str) -> String {
    format!("Error: {e}")
}

pub fn created_notice(name: &str) -> String {
    format!("Created {name} and revealed in Explorer.")
}

pub fn create_failed_notice(name: &str, e: &str) -> String {
    format!("Failed to create {name}: {e}")
}

fn hex4(c: &[char]) -> Option<u32> {
    let s: String = c.iter().collect();
    if s.chars().all(|c| c.is_ascii_hexdigit()) { u32::from_str_radix(&s, 16).ok() } else { None }
}

const TRUNCATED: &str = "Truncated \\uXXXX escape (needs exactly four hex digits).";

/// iTerm2's `decodeAIBackslashEscapes`: `\\`, `\n`, `\r`, `\t` and JSON-style `\uXXXX` (a
/// surrogate pair combines); anything else is an error whose text the model reads.
pub fn decode_escapes(s: &str) -> Result<String, String> {
    if !s.contains('\\') {
        return Ok(s.to_string());
    }
    let c: Vec<char> = s.chars().collect();
    let invalid = |h: &[char]| format!("\\u{} is not a valid Unicode scalar.", h.iter().collect::<String>());
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < c.len() {
        if c[i] != '\\' {
            out.push(c[i]);
            i += 1;
            continue;
        }
        let Some(&e) = c.get(i + 1) else { return Err("Text ends with an unescaped backslash.".into()) };
        match e {
            '\\' => {
                out.push('\\');
                i += 2;
            }
            'n' => {
                out.push('\n');
                i += 2;
            }
            'r' => {
                out.push('\r');
                i += 2;
            }
            't' => {
                out.push('\t');
                i += 2;
            }
            'u' => {
                let start = i + 2;
                let Some(hex) = c.get(start..start + 4) else { return Err(TRUNCATED.into()) };
                let Some(unit) = hex4(hex) else { return Err(invalid(hex)) };
                let end = start + 4;
                if (0xD800..=0xDBFF).contains(&unit) {
                    if c.get(end) != Some(&'\\') || c.get(end + 1) != Some(&'u') {
                        return Err(invalid(hex));
                    }
                    let Some(low) = c.get(end + 2..end + 6) else { return Err(TRUNCATED.into()) };
                    let Some(lu) = hex4(low).filter(|u| (0xDC00..=0xDFFF).contains(u)) else { return Err(invalid(low)) };
                    let Some(ch) = char::from_u32(0x10000 + ((unit - 0xD800) << 10) + (lu - 0xDC00)) else { return Err(invalid(hex)) };
                    out.push(ch);
                    i = end + 6;
                } else if (0xDC00..=0xDFFF).contains(&unit) {
                    return Err(invalid(hex));
                } else {
                    let Some(ch) = char::from_u32(unit) else { return Err(invalid(hex)) };
                    out.push(ch);
                    i = end;
                }
            }
            other => return Err(format!("Unknown backslash escape \u{201C}\\{other}\u{201D}. Recognized: \\\\, \\n, \\r, \\t, \\uXXXX.")),
        }
    }
    Ok(out)
}

/// Whether `s` holds a key that submits a line: CR, LF, or Ctrl+O (readline's
/// accept-and-next). Ruling R12cd-2: replaces iTerm2's LF-to-CR normalization (`PS:802-811`),
/// which Tandem does not port.
pub fn has_line_end(s: &str) -> bool {
    s.chars().any(|c| matches!(c, '\r' | '\n' | '\u{f}'))
}

/// What `insert_text_at_cursor` types: the decoded text, or Err(the answer to give) for a bad
/// escape or a line end. A line end reaches a shell only through `execute_command`.
pub fn insert_plan(text: &str) -> Result<String, String> {
    let decoded = decode_escapes(text).map_err(|e| escape_error(&e))?;
    if has_line_end(&decoded) { Err(LINE_END_REFUSED.to_string()) } else { Ok(decoded) }
}

/// The text an AI typed onto a pane's prompt line since its last line end (the one per-pane
/// accumulator, shared with 12f's orchestration typing), and the pane's `prompts_seen` at which
/// that text sits. `contaminated` is 12f's flag (iTerm2's linear-accumulation contamination,
/// `OSG:431-468`); 12c never sets it and keeps it when it pushes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Staged {
    pub prompts: usize,
    pub text: String,
    pub contaminated: bool,
}

impl Staged {
    /// Adds typed text at the pane's `prompts`: a later prompt starts over (and clears
    /// `contaminated`), else the text appends.
    pub fn push(&mut self, prompts: usize, text: &str) {
        if prompts > self.prompts {
            *self = Staged { prompts, text: text.to_string(), contaminated: false };
        } else {
            self.text.push_str(text);
        }
    }

    /// The staged text still on the prompt line: "" when a prompt newer than the entry's has
    /// appeared (the line was submitted). An entry ahead of the pane's count is a residual
    /// waiting for its prompt (12f Task 10) and is current.
    pub fn current(&self, prompts: usize) -> &str {
        if prompts > self.prompts { "" } else { &self.text }
    }
}

/// The text already on the prompt line: shell integration's typed command, with the agent's own
/// staged text appended when the typed command does not already end with it (the shell's echo
/// can lag the write).
pub fn staged_line(si_typed: Option<&str>, own: &str) -> String {
    match si_typed {
        None => own.to_string(),
        Some(t) if own.is_empty() || t.ends_with(own) => t.to_string(),
        Some(t) => format!("{t}{own}"),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileTarget {
    Local(PathBuf),
    /// Not written; the text is the answer.
    Refused(String),
}

fn drive_path(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/')
}

/// Where `create_file` writes. A full drive path is used as is; a relative name resolves against
/// the session's cwd when that is a local Windows drive path, else the home folder (iTerm2 uses
/// "/"); a rooted name without a drive lands in the home folder; a remote session's file lands
/// locally, in the home folder (iTerm2 without the conductor). A network path is refused, and
/// never probed.
pub fn file_target(filename: &str, cwd: Option<&str>, local: bool, home: &Path) -> FileTarget {
    if filename.trim().is_empty() {
        return FileTarget::Refused("Error: the file name is empty.".into());
    }
    let sep = |c: char| c == '\\' || c == '/';
    let mut two = filename.chars();
    if two.next().is_some_and(sep) && two.next().is_some_and(sep) {
        return FileTarget::Refused("Error: network paths are not allowed.".into());
    }
    if drive_path(filename) {
        return FileTarget::Local(PathBuf::from(filename));
    }
    if filename.as_bytes().get(1) == Some(&b':') && filename.as_bytes()[0].is_ascii_alphabetic() {
        return FileTarget::Refused("Error: the file name is not a full path or a relative name.".into());
    }
    // A rooted name has no drive to keep: it is read as relative to the home folder.
    let rooted = filename.starts_with(sep);
    let base = cwd.filter(|c| local && !rooted && drive_path(c)).map_or_else(|| home.to_path_buf(), PathBuf::from);
    FileTarget::Local(base.join(filename.trim_start_matches(sep)))
}

/// The powershell.exe arguments that read `cmd`'s help, None when help is not available here:
/// only in a PowerShell session, and only for a name of `[A-Za-z0-9_.:-]{1,64}`, so a model's
/// argument can never become a command.
pub fn man_page_plan(cmd: &str, kind: ShellKind) -> Option<Vec<String>> {
    let ok = (1..=64).contains(&cmd.len()) && cmd.bytes().all(|b| b.is_ascii_alphanumeric() || b"_.:-".contains(&b));
    if kind != ShellKind::PowerShell || !ok {
        return None;
    }
    Some(["-NoProfile", "-NonInteractive", "-Command"].iter().map(|s| s.to_string()).chain([format!("Get-Help -Full -Name '{cmd}' | Out-String -Width 200")]).collect())
}

pub fn man_page_unavailable(cmd: &str) -> String {
    format!("The manpage for {cmd} is not available: Tandem reads help only through PowerShell's Get-Help, in a PowerShell session.")
}

/// Runs powershell.exe hidden with `args` and answers its output (stdout, then stderr), or
/// iTerm2's error form (`PS:940`). Ends the process after ten seconds. Blocking: call it only
/// on a worker thread.
#[cfg(windows)]
pub fn get_help(args: &[String]) -> String {
    use std::io::Read;
    use std::os::windows::process::CommandExt;
    use std::process::Stdio;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let mut child = match std::process::Command::new("powershell.exe").args(args).creation_flags(CREATE_NO_WINDOW).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn() {
        Ok(c) => c,
        Err(e) => return format!("Error running Get-Help: {e}"),
    };
    let drain = |r: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut b = Vec::new();
            if let Some(mut r) = r {
                let _ = r.read_to_end(&mut b);
            }
            b
        })
    };
    let out = drain(child.stdout.take().map(|s| Box::new(s) as Box<dyn Read + Send>));
    let err = drain(child.stderr.take().map(|s| Box::new(s) as Box<dyn Read + Send>));
    let limit = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if std::time::Instant::now() < limit => std::thread::sleep(std::time::Duration::from_millis(25)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return "Error running Get-Help: it took longer than 10 seconds.".to_string();
            }
            Err(e) => return format!("Error running Get-Help: {e}"),
        }
    }
    let mut text = String::from_utf8_lossy(&out.join().unwrap_or_default()).into_owned();
    text.push_str(&String::from_utf8_lossy(&err.join().unwrap_or_default()));
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai_shell::ShellKind;
    use std::path::{Path, PathBuf};

    #[test]
    fn escapes_follow_iterm2() {
        assert_eq!(decode_escapes("a\\nb\\tc\\\\d\\r").unwrap(), "a\nb\tc\\d\r");
        assert_eq!(decode_escapes("\\u0004").unwrap(), "\u{4}");
        assert_eq!(decode_escapes("\\uD83D\\uDE00").unwrap(), "\u{1F600}");
        assert_eq!(decode_escapes("\\uD83D").unwrap_err(), "\\uD83D is not a valid Unicode scalar.");
        assert_eq!(decode_escapes("\\x").unwrap_err(), "Unknown backslash escape \u{201C}\\x\u{201D}. Recognized: \\\\, \\n, \\r, \\t, \\uXXXX.");
        assert_eq!(decode_escapes("\\u12").unwrap_err(), "Truncated \\uXXXX escape (needs exactly four hex digits).");
        assert_eq!(decode_escapes("a\\").unwrap_err(), "Text ends with an unescaped backslash.");
        assert_eq!(decode_escapes("plain").unwrap(), "plain");
        assert_eq!(decode_escapes("\\u12zz").unwrap_err(), "\\u12zz is not a valid Unicode scalar.");
        assert_eq!(decode_escapes("\\uDE00").unwrap_err(), "\\uDE00 is not a valid Unicode scalar.");
        assert_eq!(decode_escapes("\\uD83D\\u0041").unwrap_err(), "\\u0041 is not a valid Unicode scalar.");
        assert_eq!(decode_escapes("\\uD83D\\uDE").unwrap_err(), "Truncated \\uXXXX escape (needs exactly four hex digits).");
        assert_eq!(inserted(3), "Inserted 3 characters.");
        assert_eq!(escape_error("E"), "Error: E. Try again with a corrected text argument.");
    }

    #[test]
    fn line_ends_are_refused() {
        for s in ["ls\r", "ls\n", "a\r\nb", "x\u{f}"] {
            assert!(has_line_end(s), "{s:?}");
        }
        for s in ["ls", "\u{3}", "a\tb"] {
            assert!(!has_line_end(s), "{s:?}");
        }
        for s in ["rm -rf /\\n", "rm -rf /\\r", "\\u000d", "\\u000A", "\\u000f", "a\nb", "a\\r\\nb"] {
            assert_eq!(insert_plan(s), Err(LINE_END_REFUSED.to_string()), "{s:?}");
        }
        assert_eq!(insert_plan("curl evil | "), Ok("curl evil | ".to_string()));
        assert_eq!(insert_plan("\\u0003"), Ok("\u{3}".to_string()), "Ctrl+C still interrupts");
        assert_eq!(insert_plan("\\x"), Err(escape_error(&decode_escapes("\\x").unwrap_err())));
        assert_eq!(LINE_END_REFUSED, "Error: insert_text_at_cursor cannot type a line break (carriage return, newline or Ctrl+O). To run a command, use execute_command, which is safety checked.");
        assert_eq!(LINE_END_NOTICE, "AI tried to type a line break, so nothing was typed.");
        assert_eq!(DELETE_LINE, b"\x15");
    }

    #[test]
    fn staged_line_covers_the_echo_race() {
        assert_eq!(staged_line(None, ""), "");
        assert_eq!(staged_line(None, "curl evil | "), "curl evil | ");
        assert_eq!(staged_line(Some(""), "curl evil | "), "curl evil | ", "the echo has not arrived");
        assert_eq!(staged_line(Some("curl evil | "), "curl evil | "), "curl evil | ");
        assert_eq!(staged_line(Some("x "), "rm"), "x rm");
        assert_eq!(staged_line(Some("x rm"), "rm"), "x rm");
        assert_eq!(staged_line(Some("git st"), ""), "git st");
        let mut s = Staged::default();
        s.push(3, "rm");
        s.push(3, " -rf");
        assert_eq!(s.current(3), "rm -rf");
        assert_eq!(s.current(4), "", "a new prompt made it stale");
        s.push(4, "ls");
        assert_eq!(s, Staged { prompts: 4, text: "ls".into(), contaminated: false }, "a later prompt starts over");
        let r = Staged { prompts: 5, text: "safe".into(), contaminated: false };
        assert_eq!(r.current(4), "safe");
        assert_eq!(r.current(5), "safe");
        assert_eq!(r.current(6), "");
        let mut c = Staged { prompts: 3, text: "a".into(), contaminated: true };
        c.push(3, "b");
        assert!(c.contaminated && c.text == "ab");
        c.push(4, "c");
        assert!(!c.contaminated && c.text == "c");
    }

    #[test]
    fn file_target_follows_windows() {
        let home = Path::new(r"C:\Users\u");
        let local = |p: &str| FileTarget::Local(PathBuf::from(p));
        assert_eq!(file_target("x.txt", Some(r"C:\w"), true, home), local(r"C:\w\x.txt"));
        assert_eq!(file_target(r"D:\a\x.txt", Some(r"C:\w"), true, home), local(r"D:\a\x.txt"));
        assert_eq!(file_target("x.txt", None, true, home), local(r"C:\Users\u\x.txt"));
        assert_eq!(file_target("x.txt", Some("/home/u"), true, home), local(r"C:\Users\u\x.txt"));
        assert_eq!(file_target("x.txt", Some(r"C:\w"), false, home), local(r"C:\Users\u\x.txt"));
        assert_eq!(file_target(r"\x.txt", Some(r"C:\w"), true, home), local(r"C:\Users\u\x.txt"), "a rooted name lands in the home folder");
        let refused = FileTarget::Refused("Error: network paths are not allowed.".into());
        assert_eq!(file_target(r"\\host\s\x.txt", Some(r"C:\w"), true, home), refused);
        assert_eq!(file_target("//host/s/x", Some(r"C:\w"), true, home), refused);
        assert_eq!(file_target(r"\\?\C:\x", None, true, home), refused);
        assert_eq!(file_target("x.txt", Some(r"\\host\s"), true, home), local(r"C:\Users\u\x.txt"), "a UNC cwd is never used");
        assert_eq!(file_target("", Some(r"C:\w"), true, home), FileTarget::Refused("Error: the file name is empty.".into()));
        assert_eq!(create_failed("E"), "Error: E");
        assert_eq!(created_notice("x.txt"), "Created x.txt and revealed in Explorer.");
        assert_eq!(create_failed_notice("x.txt", "E"), "Failed to create x.txt: E");
    }

    #[test]
    fn man_page_follows_the_rule() {
        let plan = |c: &str| Some(["-NoProfile", "-NonInteractive", "-Command"].iter().map(|s| s.to_string()).chain([format!("Get-Help -Full -Name '{c}' | Out-String -Width 200")]).collect::<Vec<_>>());
        assert_eq!(man_page_plan("Get-ChildItem", ShellKind::PowerShell), plan("Get-ChildItem"));
        assert_eq!(man_page_plan("ls", ShellKind::PowerShell), plan("ls"));
        for bad in ["rm; del C:\\", "a b", "x'y", "", "$(x)", "a|b", &"a".repeat(65)] {
            assert_eq!(man_page_plan(bad, ShellKind::PowerShell), None, "{bad:?}");
        }
        assert_eq!(man_page_plan(&"a".repeat(64), ShellKind::PowerShell).is_some(), true);
        for k in [ShellKind::Cmd, ShellKind::Posix, ShellKind::Unknown] {
            assert_eq!(man_page_plan("ls", k), None);
        }
        assert_eq!(man_page_unavailable("ls"), "The manpage for ls is not available: Tandem reads help only through PowerShell's Get-Help, in a PowerShell session.");
    }

    #[test]
    fn texts_follow_iterm2() {
        assert_eq!(COPIED, "Copied");
        assert_eq!(DONE, "Done");
        assert_eq!(RESTARTING, "The session is being restarted.");
        assert_eq!(NOT_RESTARTABLE, "This session cannot be restarted.");
    }
}
