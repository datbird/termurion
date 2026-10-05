//! Terminal emulation switches in the window (13o): the Advanced rules handed to every
//! pane's screen, and the OSC 7 conflict warning.

use super::*;
use crate::emu;
use crate::warnings::Kind;

use super::scripts_alerts::{self, Then};

impl App {
    /// The Advanced emulation rules as the globals now stand.
    pub(super) fn emu_rules(&self) -> emu::Rules {
        emu::Rules::from_globals(&self.globals)
    }

    /// Hands the rules and the profile's Terminal Type to a pane's own screen and any view. A
    /// tmux pane keeps the type tmux reports for it (`PS:3253-3255`).
    pub(super) fn apply_emu_rules(&mut self, pane: PaneId) {
        let rules = self.emu_rules();
        let Some(p) = self.panes.get(&pane) else { return };
        let term = if p.tmux_io().is_some() {
            None
        } else {
            Some(self.store.get(&p.profile).unwrap_or_else(|| self.store.default_profile()).terminal_type())
        };
        if let Some(p) = self.panes.get_mut(&pane) {
            p.set_emu(rules, term.as_deref());
        }
        // The profile's Answerback String (`PS:6207`); a tmux pane has none of its own.
        let answerback = self.panes.get(&pane).filter(|p| p.tmux_io().is_none()).map(|p| self.store.get(&p.profile).unwrap_or_else(|| self.store.default_profile()).string("Answerback String"));
        if let (Some(text), Some(p)) = (answerback, self.panes.get_mut(&pane)) {
            p.set_answerback(&text);
        }
    }

    /// CSI 3 J with Prevent CSI 3 J unset (`N:795-815`): the pane's bar asks once. The sequence
    /// that asked was dropped.
    pub(super) fn ask_clear_history(&mut self, pane: PaneId) {
        use crate::emu::{ALWAYS_ALLOW, ALWAYS_DENY, CLEAR_HISTORY_ID, CLEAR_HISTORY_TEXT};
        self.ask(pane, CLEAR_HISTORY_ID, CLEAR_HISTORY_TEXT, &[ALWAYS_ALLOW, ALWAYS_DENY], false);
    }

    /// The answer to that bar: "Always Allow" stores false, "Always Deny" true (iTerm2 stores
    /// the key the same way); every pane takes the new rules. A closed bar stores nothing.
    pub(super) fn clear_history_answered(&mut self, button: &str) {
        self.globals.set("PreventEscapeSequenceFromClearingHistory", toml::Value::Boolean(button.contains("Deny")));
        self.save_globals();
        let ids: Vec<PaneId> = self.panes.keys().copied().collect();
        for id in ids {
            self.apply_emu_rules(id);
        }
    }

    /// DECRQCRA with Disable DECRQCRA unset (`PS:17182-17212`): the pane's bar asks once. The
    /// sequence that asked was not answered.
    pub(super) fn ask_decrqcra(&mut self, pane: PaneId) {
        use crate::emu::{DECRQCRA_ID, DECRQCRA_TEXT};
        self.ask(pane, DECRQCRA_ID, DECRQCRA_TEXT, &["Yes", "No"], false);
    }

    /// "Yes" stores Disable DECRQCRA false, "No" true (iTerm2's `NoSyncDisableDECRQCRA`); either
    /// is kept in the file (`KEPT_WHEN_DEFAULT`), so the question is asked once. A closed bar
    /// stores nothing.
    pub(super) fn decrqcra_answered(&mut self, button: &str) {
        self.globals.set(crate::emu::DECRQCRA_ID, toml::Value::Boolean(button == "No"));
        self.save_globals();
        let ids: Vec<PaneId> = self.panes.keys().copied().collect();
        for id in ids {
            self.apply_emu_rules(id);
        }
    }

    /// OSC 7 with a machine id arrived while Accept OSC 7 is off (`PS:15534-15558`): asked once
    /// a run, however many panes send it.
    pub(super) fn osc7_disabled(&mut self) {
        // A warning queued before the key was turned back on does not spend the latch
        // (`MS:4646-4653` re-checks when it is shown).
        if self.emu_rules().accept_osc7 || !self.osc7_latch.first() {
            return;
        }
        let was_silenced = crate::warnings::is_silenced(&self.globals, emu::OSC7_WARNING_ID, super::warnings_app::unix_now());
        let ask = scripts_alerts::ask(emu::OSC7_HEADING, emu::OSC7_TEXT, &[emu::OSC7_TURN_ON, crate::api_auth::CANCEL], Some(emu::OSC7_WARNING_ID), Kind::Permanent, Then::TurnOnOsc7 { was_silenced });
        self.ask_scripts(ask);
    }

    /// "Turn On OSC 7" sets the key only when the question was not silenced: a remembered
    /// answer never flips a setting (`selection == kiTermWarningSelection0 && !wasSilenced`).
    pub(super) fn osc7_answered(&mut self, was_silenced: bool, index: usize) {
        if index == 0 && !was_silenced {
            self.globals.set("AcceptOSC7", toml::Value::Boolean(true));
            self.globals_changed(false);
        }
    }
}
