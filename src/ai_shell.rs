//! Which kind of shell a chat's session runs (ruling 9), so the AI tools pick the right
//! per-shell form. There is one detector: `ai_terminal::detected_shell` sets the session's
//! `shell` variable, and `shell_kind` only classifies that answer. iTerm2 walks no process
//! tree for this (`screenDidDetectShell`, `bestGuessAtUserShellWithPath:`, `PS:19408-19425`), so
//! a shell started inside another without shell integration reads as the outer one.

use crate::proctree::strip_exe;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShellKind {
    Cmd,
    PowerShell,
    Posix,
    Unknown,
}

/// A process or shell name's kind, any case, ".exe" stripped. `wsl` and `ssh` count as POSIX:
/// what they start is a POSIX shell. None for a name that is no shell.
pub fn classify(name: &str) -> Option<ShellKind> {
    let n = strip_exe(name.trim()).to_ascii_lowercase();
    match n.as_str() {
        "cmd" => Some(ShellKind::Cmd),
        "powershell" | "pwsh" => Some(ShellKind::PowerShell),
        "bash" | "zsh" | "sh" | "fish" | "dash" | "ksh" | "tcsh" | "csh" | "wsl" | "ssh" => Some(ShellKind::Posix),
        _ => None,
    }
}

/// The kind of the session's shell: the detected shell's when it is a known one, else POSIX
/// for a remote host (a POSIX shell over ssh), else Unknown.
pub fn shell_kind(detected: Option<&str>, local: bool) -> ShellKind {
    if let Some(k) = detected.and_then(classify) {
        return k;
    }
    if local { ShellKind::Unknown } else { ShellKind::Posix }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shells_follow_the_ruling() {
        assert_eq!(classify("cmd.exe"), Some(ShellKind::Cmd));
        assert_eq!(classify("CMD"), Some(ShellKind::Cmd));
        assert_eq!(classify("powershell"), Some(ShellKind::PowerShell));
        assert_eq!(classify("pwsh.exe"), Some(ShellKind::PowerShell));
        for n in ["bash", "zsh", "sh", "fish", "dash", "ksh", "tcsh", "csh", "wsl", "wsl.exe", "ssh", "ssh.exe"] {
            assert_eq!(classify(n), Some(ShellKind::Posix), "{n}");
        }
        assert_eq!(classify("python"), None);
        assert_eq!(shell_kind(Some("powershell"), true), ShellKind::PowerShell);
        assert_eq!(shell_kind(Some("cmd"), true), ShellKind::Cmd);
        assert_eq!(shell_kind(Some("zsh"), true), ShellKind::Posix);
        assert_eq!(shell_kind(Some("wsl"), true), ShellKind::Posix, "a pane launched as wsl.exe");
        assert_eq!(shell_kind(Some("ssh"), true), ShellKind::Posix, "a pane launched as ssh.exe");
        assert_eq!(shell_kind(Some("python"), true), ShellKind::Unknown);
        assert_eq!(shell_kind(None, false), ShellKind::Posix, "a remote host is a POSIX shell over ssh");
        assert_eq!(shell_kind(None, true), ShellKind::Unknown);
    }
}
