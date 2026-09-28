//! iTerm2's outdated shell integration rule (`VT100ScreenMutableState+TerminalDelegate.m:2814-2852`)
//! and its announcement (`PS:19378-19472`): a shell integration script that reports an older
//! version than the latest known for its shell, or no shell at all, is offered an upgrade
//! unless that account on that host was silenced.

/// The latest script version by shell (`iTermLatestVersionByShell.h`).
pub const LATEST: [(&str, i64); 5] = [("bash", 22), ("fish", 25), ("tcsh", 9), ("xonsh", 4), ("zsh", 19)];

/// The oldest version that is not critically out of date, read while
/// `NotifyOnlyForCriticalShellIntegrationUpdates` is on (its default).
pub const CRITICAL: [(&str, i64); 4] = [("tcsh", 2), ("bash", 5), ("zsh", 5), ("fish", 5)];

/// Whether a report of `version` (None when it did not read as a number, which iTerm2's
/// `integerValue` reads as 0) from `shell` (None when the report names none) suggests an
/// upgrade: no shell, or older than the table's version for it. A shell no table names has
/// latest 0 and never does, so Tandem's own PowerShell script ("pwsh", "powershell") is
/// never out of date.
pub fn outdated(version: Option<u32>, shell: Option<&str>, critical_only: bool) -> bool {
    let table: &[(&str, i64)] = if critical_only { &CRITICAL } else { &LATEST };
    let Some(shell) = shell else { return true };
    let latest = table.iter().find(|(s, _)| *s == shell).map_or(0, |(_, v)| *v);
    i64::from(version.unwrap_or(0)) < latest
}

/// The globals key that silences the notice for an account (`PS:19378-19381`); an unknown
/// user or host is empty.
pub fn silence_key(user: &str, host: &str) -> String {
    format!("SuppressShellIntegrationUpgradeAnnouncementForHost_{user}@{host}")
}

/// The warning announcement (`PS:19451-19453`).
pub const TEXT: &str = "This account\u{2019}s Shell Integration scripts are out of date.";
pub const BUTTONS: [&str; 2] = ["Upgrade", "Silence Warning"];
/// The announcement's identifier, the bar's id; a host change removes it (`PS:17909`).
pub const BAR_ID: &str = "kShellIntegrationOutOfDateAnnouncementIdentifier";

/// What Upgrade does (`PS:19383-19406`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Upgrade {
    /// The session has ended.
    Nothing,
    /// At a command prompt: runs the installer.
    Install,
    /// Asks first (`NOT_AT_PROMPT`, a persistent warning).
    Ask,
}

/// Upgrade in a session that has `exited` or not, and is `at_prompt` (iTerm2's
/// `currentCommand` is not nil) or not.
pub fn upgrade(exited: bool, at_prompt: bool) -> Upgrade {
    match (exited, at_prompt) {
        (true, _) => Upgrade::Nothing,
        (false, true) => Upgrade::Install,
        (false, false) => Upgrade::Ask,
    }
}

/// The warning Upgrade shows away from a prompt, with no heading (`PS:19392-19396`).
pub const NOT_AT_PROMPT: &str = "It looks like you're not at a command prompt.";
pub const RUN_ANYWAY: [&str; 2] = ["Run Installer Anyway", "Cancel"];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outdated_follows_iterm2() {
        assert!(outdated(Some(4), Some("bash"), true));
        assert!(!outdated(Some(5), Some("bash"), true));
        assert!(outdated(Some(21), Some("bash"), false));
        assert!(!outdated(Some(22), Some("bash"), false));
        assert!(!outdated(Some(1), Some("pwsh"), false), "a shell with no entry has latest 0");
        assert!(!outdated(Some(1), Some("powershell"), true), "Tandem's own script never nags");
        assert!(outdated(Some(1), None, true), "iTerm2 nags when no shell is named");
        assert!(outdated(None, None, true));
        assert!(outdated(None, None, false));
        assert!(outdated(None, Some("zsh"), true), "an unreadable version is 0 (integerValue)");
        assert!(!outdated(None, Some("pwsh"), true));
        for (shell, latest) in CRITICAL {
            assert!(outdated(Some(latest as u32 - 1), Some(shell), true), "{shell}");
            assert!(!outdated(Some(latest as u32), Some(shell), true), "{shell}");
        }
        for (shell, latest) in LATEST {
            assert!(outdated(Some(latest as u32 - 1), Some(shell), false), "{shell}");
            assert!(!outdated(Some(latest as u32), Some(shell), false), "{shell}");
        }
        assert!(!outdated(Some(1), Some("xonsh"), true), "xonsh has no critical entry");
        assert_eq!(LATEST, [("bash", 22), ("fish", 25), ("tcsh", 9), ("xonsh", 4), ("zsh", 19)]);
        assert_eq!(CRITICAL, [("tcsh", 2), ("bash", 5), ("zsh", 5), ("fish", 5)]);
        assert_eq!(silence_key("rob", "pc"), "SuppressShellIntegrationUpgradeAnnouncementForHost_rob@pc");
        assert_eq!(silence_key("", ""), "SuppressShellIntegrationUpgradeAnnouncementForHost_@");
        assert_eq!(TEXT, "This account\u{2019}s Shell Integration scripts are out of date.");
        assert_eq!(BUTTONS, ["Upgrade", "Silence Warning"]);
        assert_eq!(BAR_ID, "kShellIntegrationOutOfDateAnnouncementIdentifier");
    }

    /// Upgrade's prompt check (`tryToRunShellIntegrationInstallerWithPromptCheck:`,
    /// `PS:19383-19406`).
    #[test]
    fn upgrade_follows_iterm2() {
        assert_eq!(upgrade(true, true), Upgrade::Nothing, "the shell has exited");
        assert_eq!(upgrade(true, false), Upgrade::Nothing);
        assert_eq!(upgrade(false, true), Upgrade::Install);
        assert_eq!(upgrade(false, false), Upgrade::Ask);
        assert_eq!(NOT_AT_PROMPT, "It looks like you're not at a command prompt.");
        assert_eq!(RUN_ANYWAY, ["Run Installer Anyway", "Cancel"]);
    }
}
