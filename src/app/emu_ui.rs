//! Terminal emulation switches in the window (13o): the Advanced rules handed to every
//! pane's screen, the profile's encoding, and the OSC 7 conflict warning.

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
        // The profile's Character Encoding (`PS:18899`); a tmux pane's output and input are
        // always UTF-8 (control mode), so it keeps none.
        let encoding = self.panes.get(&pane).filter(|p| p.tmux_io().is_none() && !p.gateway).map(|p| self.store.get(&p.profile).unwrap_or_else(|| self.store.default_profile()).int("Character Encoding"));
        if let (Some(ns), Some(p)) = (encoding, self.panes.get_mut(&pane)) {
            p.set_encoding(if ns == 0 { crate::encoding::UTF8 } else { ns });
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

    /// An escape-sequence print (`CSI 4 i` after `CSI 5 i`, or `CSI i`): the pane's print guard
    /// decides (`PG:20-75`); a second attempt within 30 s asks, and only the person's answer at
    /// the print dialog prints anything.
    pub(super) fn print_request(&mut self, pane: PaneId, job: crate::screen::PrintJob) {
        use crate::emu::Guard;
        // One question at a time per pane: while "Allow Printing?" waits (shown or queued) a
        // further attempt is dropped, and is not counted against the guard (P21).
        if self.alert_waits(|t| matches!(t, Then::PrintGuard { pane: p, .. } if *p == pane)) {
            return;
        }
        let disabled = self.profile_of(pane).is_some_and(|p| p.bool("Disable Printing"));
        let guard = self.print_guards.entry(pane).or_default().check(std::time::Instant::now(), disabled, true);
        match guard {
            Guard::Print => self.print_job(pane, &job),
            Guard::Refuse => {}
            Guard::Ask => {
                let ask = scripts_alerts::ask(emu::PRINT_HEADING, emu::PRINT_TEXT, &emu::PRINT_BUTTONS, Some(emu::PRINT_ID), Kind::Persistent, Then::PrintGuard { pane, job });
                self.ask_scripts(ask);
            }
        }
    }

    /// The answer to "Allow Printing?": Allow prints; Disable Permanently also turns the saved
    /// profile's Disable Printing on (the original of a session copy, `PG:57-62`).
    pub(super) fn print_guard_answered(&mut self, pane: PaneId, job: crate::screen::PrintJob, index: usize) {
        // A pane that closed since has no guard and prints nothing.
        let now = std::time::Instant::now();
        let Some(guard) = self.print_guards.get_mut(&pane) else { return };
        // The guard is checked again now: an Allow that waited behind a Disable prints nothing
        // (iTerm2 asks and answers synchronously, `PG:31-33`).
        let refused = guard.refused(now);
        let permanent = guard.answer(now, index);
        if permanent {
            let guid = self.panes.get(&pane).map(|p| self.store.shared_guid(&p.profile));
            if let Some(profile) = guid.and_then(|g| self.store.get_mut(&g)) {
                profile.set("Disable Printing", toml::Value::Boolean(true));
                if let Err(e) = self.store.save() {
                    self.banners.set("print", format!("Could not save the profile: {e}"));
                }
                self.refresh_panes();
            }
        }
        if index == 0 && !refused {
            self.print_job(pane, &job);
        }
    }

    /// Opens the print dialog for the job (the person cancels there or prints).
    fn print_job(&mut self, pane: PaneId, job: &crate::screen::PrintJob) {
        let Some(hwnd) = self.hwnd() else { return };
        let Some(p) = self.panes.get(&pane) else { return };
        let screen = &p.screen;
        let mut rows = match job {
            crate::screen::PrintJob::Screen => crate::print::rows(screen, crate::print::What::Screen),
            crate::screen::PrintJob::Text(t) => crate::print::text_rows(t, screen.palette.fg, screen.palette.bg),
        };
        let cols = { use alacritty_terminal::grid::Dimensions; screen.term().columns() };
        let title = screen.title().unwrap_or("Tandem").to_string();
        let font = self.profile_of(pane).map(|pr| pr.font("Normal Font")).unwrap_or(crate::profile::FontSpec { family: "Cascadia Mono".into(), size_pt: 12.0 });
        if self.globals.bool("Print In Black And White") {
            crate::print::black_and_white(&mut rows);
        }
        if let Err(e) = crate::winprint::print(hwnd as _, &mut self.print_setup, &title, &rows, cols, &font.family, font.size_pt) {
            self.banners.set("print", format!("Printing failed: {e}"));
        }
    }
}
