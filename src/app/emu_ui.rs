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
            Some(self.store.get(&p.profile).unwrap_or_else(|| self.store.default_profile()).string("Terminal Type"))
        };
        if let Some(p) = self.panes.get_mut(&pane) {
            p.set_emu(rules, term.as_deref());
        }
    }

    /// OSC 7 with a machine id arrived while Accept OSC 7 is off (`PS:15534-15558`): asked once
    /// a run, however many panes send it.
    pub(super) fn osc7_disabled(&mut self) {
        if !self.osc7_latch.first() {
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
