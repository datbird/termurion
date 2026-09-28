//! iTerm2's outdated shell integration notice (`PS:19378-19472`): a version report whose
//! scripts are out of date (`shell_version::outdated`) shows a warning bar in the pane unless
//! the account (user and host) was silenced; a host change removes a waiting one (`PS:17909`).

use super::*;
use crate::infobar::Bars;
use crate::shell_version::{self as sv, Upgrade};

/// General > Software Update's "Ignore non-critical shell integration updates".
const CRITICAL_ONLY: &str = "NotifyOnlyForCriticalShellIntegrationUpdates";

/// What an answer does (`PS:19455-19469`): a close does nothing.
#[derive(Debug, PartialEq, Eq)]
enum Answer {
    Upgrade,
    Silence,
    Nothing,
}

fn answer(button: Option<&str>) -> Answer {
    match button {
        Some(b) if b == sv::BUTTONS[0] => Answer::Upgrade,
        Some(b) if b == sv::BUTTONS[1] => Answer::Silence,
        _ => Answer::Nothing,
    }
}

/// Drops a waiting notice from a pane's bars.
fn drop_notice(bars: &mut Bars) {
    bars.retain(|b| b.id != sv::BAR_ID);
}

impl App {
    /// The pane's shell integration reported its version (and the shell it names, if any).
    pub(super) fn shell_integration_reported(&mut self, pane: PaneId, version: Option<u32>, shell: Option<String>) {
        if !sv::outdated(version, shell.as_deref(), self.globals.bool(CRITICAL_ONLY)) {
            return;
        }
        let Some(key) = self.shell_silence_key(pane) else { return };
        if !self.globals.bool(&key) {
            // A warning announcement stays until answered (no dismissOnKeyDown).
            self.ask(pane, sv::BAR_ID, sv::TEXT, &sv::BUTTONS, true);
        }
    }

    /// The silence key for the pane's current account; user and host are empty when unknown.
    fn shell_silence_key(&self, pane: PaneId) -> Option<String> {
        let shell = &self.panes.get(&pane)?.own().shell;
        Some(sv::silence_key(shell.user.as_deref().unwrap_or_default(), shell.host.as_deref().unwrap_or_default()))
    }

    /// A host change: a waiting notice is about the previous account.
    pub(super) fn drop_shell_version_notice(&mut self, pane: PaneId) {
        if let Some(bars) = self.bars.get_mut(&pane) {
            drop_notice(bars);
            self.redraw();
        }
    }

    /// The notice's answer. False for other bars.
    pub(super) fn shell_version_bar(&mut self, pane: PaneId, id: &str, button: Option<&str>) -> bool {
        if id != sv::BAR_ID {
            return false;
        }
        match answer(button) {
            Answer::Upgrade => self.shell_upgrade(pane),
            Answer::Silence => {
                if let Some(key) = self.shell_silence_key(pane) {
                    // Not a schema row: kept in the globals as iTerm2 keeps it in user defaults.
                    self.globals.set(&key, toml::Value::Boolean(true));
                    self.save_globals();
                }
            }
            Answer::Nothing => {}
        }
        true
    }

    /// Upgrade (`tryToRunShellIntegrationInstallerWithPromptCheck:YES`): nothing once the
    /// session has ended; the installer at a prompt; else a warning first.
    fn shell_upgrade(&mut self, pane: PaneId) {
        let Some(p) = self.panes.get(&pane) else { return };
        let auto_shown = p.composer.auto && p.composer.shown == crate::composer::Shown::Dropdown;
        let at_prompt = crate::composer::current_command(auto_shown, &p.composer.contents(), p.own().typed_command()).is_some();
        match sv::upgrade(p.exited.is_some(), at_prompt) {
            Upgrade::Nothing => {}
            Upgrade::Install => self.install_shell_integration_in(pane),
            Upgrade::Ask => {
                let then = super::scripts_alerts::Then::InstallShellIntegration(pane);
                let ask = super::scripts_alerts::ask("", sv::NOT_AT_PROMPT, &sv::RUN_ANYWAY, None, crate::warnings::Kind::Persistent, then);
                self.ask_scripts(ask);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The buttons' meanings (`PS:19455-19469`) and the host change's removal (`PS:17909`).
    #[test]
    fn shell_version_answers_follow_iterm2() {
        assert_eq!(answer(Some("Upgrade")), Answer::Upgrade);
        assert_eq!(answer(Some("Silence Warning")), Answer::Silence);
        assert_eq!(answer(None), Answer::Nothing);
        assert_eq!(CRITICAL_ONLY, "NotifyOnlyForCriticalShellIntegrationUpdates");
        let bar = |id: &str| crate::infobar::Bar { id: id.to_string(), text: String::new(), buttons: Vec::new(), important: true };
        let mut bars = Bars::default();
        bars.push(bar(sv::BAR_ID));
        bars.push(bar("other"));
        drop_notice(&mut bars);
        assert_eq!(bars.pop().map(|b| b.id), Some("other".to_string()));
        assert!(bars.is_empty());
    }

    /// The prompt check's warning: persistent, no heading, Cancel never remembered.
    #[test]
    fn not_at_prompt_warning_follows_iterm2() {
        let then = super::super::scripts_alerts::Then::InstallShellIntegration(3);
        let a = super::super::scripts_alerts::ask("", sv::NOT_AT_PROMPT, &sv::RUN_ANYWAY, None, crate::warnings::Kind::Persistent, then.clone());
        assert_eq!((a.spec.id.as_deref(), a.spec.kind, a.spec.shown_heading()), (None, crate::warnings::Kind::Persistent, "Warning"));
        assert_eq!(a.spec.cancel, Some(1));
        assert_eq!(a.then, then);
    }
}
