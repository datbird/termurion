//! Loading PowerShell shell integration automatically (iTerm2's `ShellIntegrationInjection`,
//! `SII:81-129`, `SII:238-243`). Termurion's shells with an integration script are PowerShell
//! (`powershell`, `pwsh`): the injection appends `-NoExit -Command` with an
//! `Invoke-Expression` of the script's text, which runs after the user's `$PROFILE` (so the
//! script's guard and its capture of `prompt` work) and is not a script file, so the
//! execution policy never applies and is never changed. A command line Termurion cannot extend
//! safely is left alone (iTerm2 leaves a shell it cannot inject alone).

use std::path::{Path, PathBuf};

/// A shell the injection supports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shell {
    PowerShell,
    Pwsh,
}

/// The command line's tokens split at spaces outside double quotes, quotes removed (the
/// Windows command line rule; a backslash before a quote is refused by `balanced`).
fn tokens(cmdline: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut open = false;
    let mut any = false;
    for c in cmdline.chars() {
        match c {
            '"' => {
                open = !open;
                any = true;
            }
            c if c.is_whitespace() && !open => {
                if any {
                    out.push(std::mem::take(&mut cur));
                    any = false;
                }
            }
            c => {
                cur.push(c);
                any = true;
            }
        }
    }
    if any {
        out.push(cur);
    }
    out
}

/// Whether the quotes close and none is escaped: an odd count leaves an argument open, and a
/// `\"` would swallow the `-Command` appended after it (B6).
fn balanced(cmdline: &str) -> bool {
    cmdline.matches('"').count().is_multiple_of(2) && !cmdline.contains("\\\"")
}

/// The shell a command line starts: its first token, quotes stripped, the file name
/// lowercased without ".exe" (`supportedShellName`, `SII:238-243`).
pub fn shell_of(cmdline: &str) -> Option<Shell> {
    let first = tokens(cmdline).into_iter().next()?;
    let name = first.rsplit(['\\', '/']).next()?.to_lowercase();
    match name.strip_suffix(".exe").unwrap_or(&name) {
        "powershell" => Some(Shell::PowerShell),
        "pwsh" => Some(Shell::Pwsh),
        _ => None,
    }
}

/// The switches that run something or keep the shell open, which PowerShell accepts as any
/// unambiguous prefix (at least two characters), and its one-letter forms.
const BLOCKING: [&str; 4] = ["command", "file", "encodedcommand", "noexit"];
/// The switches that take a value, which is then not the positional script.
const VALUED: [&str; 7] = ["executionpolicy", "windowstyle", "workingdirectory", "configurationname", "version", "outputformat", "inputformat"];

fn prefix_of(name: &str, words: &[&str]) -> bool {
    name.len() >= 2 && words.iter().any(|w| w.starts_with(name))
}

/// Whether the command line already runs something (or cannot take a second command):
/// `-Command`, `-File`, `-EncodedCommand`, `-NoExit` (any unambiguous prefix PowerShell
/// accepts, and the one-letter forms `-c`, `-f`, `-e`, `-ec`), or a positional script. A
/// command Termurion cannot extend is left alone, as iTerm2 leaves a shell it cannot inject
/// (Decision).
pub fn args_block(cmdline: &str) -> bool {
    let mut args = tokens(cmdline).into_iter().skip(1).map(|a| a.to_lowercase());
    while let Some(a) = args.next() {
        let Some(name) = a.strip_prefix('-').or_else(|| a.strip_prefix('/')) else {
            // The first argument that is not a switch is a script path.
            return true;
        };
        // A lone dash reads the commands from standard input.
        if name.is_empty() || matches!(name, "c" | "f" | "e" | "ec") || prefix_of(name, &BLOCKING) {
            return true;
        }
        if prefix_of(name, &VALUED) || matches!(name, "ep" | "wd" | "ws") {
            args.next();
        }
    }
    false
}

/// `<cmdline> -NoExit -Command "Invoke-Expression ([IO.File]::ReadAllText('<script>'))"`.
/// None when the shell is not PowerShell, when `args_block`, when the quotes of the command
/// line do not balance, or when the path cannot sit in the double-quoted argument. The
/// path is a single-quoted PowerShell string: PowerShell reads `'` and U+2018 to U+201B as
/// quotes, so each is doubled (B6).
pub fn inject(cmdline: &str, script: &Path) -> Option<String> {
    shell_of(cmdline)?;
    if args_block(cmdline) || !balanced(cmdline) {
        return None;
    }
    let path = script.to_str()?;
    if path.chars().any(|c| c == '"' || c.is_control()) {
        return None;
    }
    let mut quoted = String::with_capacity(path.len() + 4);
    for c in path.chars() {
        quoted.push(c);
        if matches!(c, '\'' | '\u{2018}'..='\u{201b}') {
            quoted.push(c);
        }
    }
    Some(format!("{} -NoExit -Command \"Invoke-Expression ([IO.File]::ReadAllText('{quoted}'))\"", cmdline.trim_end()))
}

/// Where Termurion keeps the script: `<appdata>\Termurion\shell_integration\
/// termurion_shell_integration.ps1`, the install menu's path.
pub fn script_path(appdata: &Path) -> PathBuf {
    appdata.join("Termurion").join("shell_integration").join("termurion_shell_integration.ps1")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn injection_follows_iterm2() {
        let p = Path::new(r"C:\A\s.ps1");
        assert_eq!(shell_of("pwsh.exe"), Some(Shell::Pwsh));
        assert_eq!(shell_of(r#""C:\Program Files\PowerShell\7\pwsh.exe" -NoLogo"#), Some(Shell::Pwsh));
        assert_eq!(shell_of("POWERSHELL"), Some(Shell::PowerShell));
        assert_eq!(shell_of(r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe -NoProfile"), Some(Shell::PowerShell));
        for no in ["cmd.exe", "wsl.exe", "bash", "pwsh-preview.exe", "", "   "] {
            assert_eq!(shell_of(no), None, "{no:?}");
        }
        for open in ["pwsh -NoProfile", "pwsh", "pwsh -NoLogo -ExecutionPolicy Bypass", "pwsh -WorkingDirectory C:\\x -NoLogo", "pwsh -NoLogo -NoProfile -MTA"] {
            assert!(!args_block(open), "{open}");
        }
        for shut in [
            "pwsh -c dir",
            "pwsh -File x.ps1",
            "powershell -EncodedCommand AA==",
            "pwsh -NoExit",
            "pwsh -Comm dir",
            "pwsh -fi x.ps1",
            "powershell -enc AA==",
            "pwsh x.ps1",
            "pwsh -NoLogo x.ps1",
            "pwsh -NoLogo -ExecutionPolicy Bypass x.ps1",
            "pwsh -E AA==",
            "pwsh -EC AA==",
            "pwsh -COMMAND dir",
            "pwsh -",
        ] {
            assert!(args_block(shut), "{shut}");
        }
        assert_eq!(inject("cmd.exe", p), None);
        assert_eq!(inject("pwsh -c dir", p), None);
        assert_eq!(inject("pwsh x.ps1", p), None);
        assert!(inject("pwsh -NoLogo", p).is_some());
    }

    #[test]
    fn injection_command_line_is_quoted() {
        let script = Path::new(r"C:\Users\o'brien\AppData\Roaming\Termurion\shell_integration\termurion_shell_integration.ps1");
        assert_eq!(
            inject("pwsh.exe -NoLogo", script).as_deref(),
            Some(r#"pwsh.exe -NoLogo -NoExit -Command "Invoke-Expression ([IO.File]::ReadAllText('C:\Users\o''brien\AppData\Roaming\Termurion\shell_integration\termurion_shell_integration.ps1'))""#)
        );
        assert_eq!(script_path(Path::new(r"C:\A")), PathBuf::from(r"C:\A").join("Termurion").join("shell_integration").join("termurion_shell_integration.ps1"));
        // B6: PowerShell reads U+2018..U+201B as single quotes too: every one is doubled.
        let typo = Path::new("C:\\Users\\O\u{2019}Brien\\x\u{2018}\u{201a}\u{201b}.ps1");
        assert_eq!(
            inject("pwsh", typo).as_deref(),
            Some("pwsh -NoExit -Command \"Invoke-Expression ([IO.File]::ReadAllText('C:\\Users\\O\u{2019}\u{2019}Brien\\x\u{2018}\u{2018}\u{201a}\u{201a}\u{201b}\u{201b}.ps1'))\"")
        );
        // A path that cannot sit inside the double-quoted argument is refused.
        for bad in ["C:\\a\"b.ps1", "C:\\a\u{0}b.ps1", "C:\\a\nb.ps1", "C:\\a\u{7}b.ps1"] {
            assert_eq!(inject("pwsh", Path::new(bad)), None, "{bad:?}");
        }
    }

    #[test]
    fn inject_refuses_odd_quotes() {
        let p = Path::new(r"C:\A\s.ps1");
        assert!(inject(r#"pwsh -WorkingDirectory "C:\my dir""#, p).is_some());
        assert_eq!(inject(r#"pwsh -WorkingDirectory "C:\my dir"#, p), None, "an unterminated quote");
        assert_eq!(inject(r#"pwsh -WorkingDirectory "C:\dir\""#, p), None, "an escaped quote ends the argument late");
        assert_eq!(inject(r#""C:\Program Files\PowerShell\7\pwsh.exe" -NoLogo"#, p).as_deref().map(|s| s.ends_with(" -NoExit -Command \"Invoke-Expression ([IO.File]::ReadAllText('C:\\A\\s.ps1'))\"")), Some(true));
    }
}
