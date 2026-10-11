//! The keyboard mode and title a host change gives back (m14e Task 4; iTerm2 `PTYSession.m`
//! `pushOrPopHostState`, `maybeOfferToRestoreIconName`, `NC:619-690`): leaving an ssh session
//! restores the key mode the previous host had, and offers (or, once told Always, just does)
//! the title restore.
//!
//! The stack is per pane (`host_stacks`, dropped with the pane in `pane_gone`). The change
//! came from output through `marks.rs`, never from the conductor, so `via_ssh_integration`
//! is false here (iTerm2 skips conductor-driven changes; those never reach this path).
//! Locality is as reported (`Remote::local`), never recomputed here (R14de-8).

use super::*;
use crate::host_state::{self as hs, Others, Remote, TitleChoice};

impl App {
    /// A reported host name changed from a known one (`Request::HostMoved`); `mode` and `title`
    /// are the pane's at the report (fix r1), not now: the rest of that output chunk may be the
    /// new host's.
    pub(super) fn host_moved(&mut self, pane: PaneId, previous: Remote, current: Remote, mode: crate::keyproto::KeyMode, title: Option<String>) {
        let enabled = self.globals.bool("RestoreKeyModeAutomaticallyOnHostChange");
        if !hs::should_track(Some(&previous), &current, previous.local && current.local, false, enabled) {
            return;
        }
        let Some(p) = self.panes.get(&pane) else { return };
        let allow_title = p.own().allow_title;
        let Some(restore) = self.host_stacks.entry(pane).or_default().push_or_pop(&current, &previous, mode, title.as_deref()) else { return };
        if let (Some(mode), Some(p)) = (restore.mode, self.panes.get_mut(&pane)) {
            p.own_mut().set_key_mode(mode);
        }
        let Some(saved) = restore.title else {
            self.redraw();
            return;
        };
        match hs::title_choice(allow_title, self.globals.optional_bool(hs::RESTORE_KEY)) {
            TitleChoice::Restore => self.restore_title(pane, saved),
            TitleChoice::Offer => {
                // A newer offer replaces this pane's waiting one (one bar per id).
                self.pending_title_restores.insert(pane, saved);
                if !self.ask(pane, hs::RESTORE_ID, hs::RESTORE_TITLES, &[hs::ONLY_THIS_TIME, hs::ALWAYS, hs::NEVER], true) {
                    self.pending_title_restores.remove(&pane);
                }
            }
            TitleChoice::Nothing => {}
        }
        self.redraw();
    }

    fn restore_title(&mut self, pane: PaneId, title: Option<String>) {
        if let Some(p) = self.panes.get_mut(&pane) {
            p.own_mut().set_title(title);
        }
    }

    /// The title offer's answer. False for other bars.
    pub(super) fn host_state_bar(&mut self, pane: PaneId, id: &str, button: Option<&str>) -> bool {
        if id != hs::RESTORE_ID {
            return false;
        }
        let answer = hs::answer(button);
        if let Some(v) = answer.remember {
            self.globals.set(hs::RESTORE_KEY, toml::Value::Boolean(v));
            self.save_globals();
        }
        let mine = self.pending_title_restores.remove(&pane);
        if let (true, Some(title)) = (answer.restore_this, mine) {
            self.restore_title(pane, title);
        }
        if answer.others != Others::Leave {
            // Always and Never also settle the other panes waiting on the offer.
            let waiting: Vec<(PaneId, Option<String>)> = self.pending_title_restores.drain().collect();
            for (other, title) in waiting {
                if let Some(b) = self.bars.get_mut(&other) {
                    b.retain(|bar| bar.id != hs::RESTORE_ID);
                }
                if answer.others == Others::Restore {
                    self.restore_title(other, title);
                }
            }
        }
        self.redraw();
        true
    }
}
