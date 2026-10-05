//! Prevent Sleep (13p, `SPC:106-188`): the one request is held while any live or buried session
//! whose profile has Prevent Sleep has not exited (and the PC is on power, or
//! PreventSleepOnBatteryToo). Looked at on every pass of the loop against a fingerprint (the
//! count, the power line, the key), so a pane's exit, a profile edit, an Undo Close stash or
//! restore and a tmux pane are all seen without a call site of their own (P16).

use super::*;

impl App {
    /// The sessions asking for Prevent Sleep: a stashed pane is not in `panes`, so it stops
    /// asking and Undo restores it.
    fn sleep_requesters(&self) -> usize {
        let panes: Vec<(bool, bool)> = self.panes.iter().map(|(id, p)| (self.profile_of(*id).is_some_and(|pr| pr.bool("Prevent Sleep")), p.exited.is_some())).collect();
        crate::sleep_guard::requesting(&panes)
    }

    /// Takes or drops the request when the fingerprint changed and pushes the status line to an
    /// open Settings window. Returns when the power line is due to be looked at again.
    pub(super) fn recompute_sleep(&mut self, now: Instant) -> Option<Instant> {
        let (requesting, battery_too) = (self.sleep_requesters(), self.globals.bool("PreventSleepOnBatteryToo"));
        // An unknown power line counts as on power (Decision).
        if let Some(d) = self.sleep_tracker.look(now, requesting, battery_too, || crate::power::sample().is_none_or(|s| s.on_ac)) {
            self.sleep.set(d.holding > 0);
            self.sleep_status = crate::sleep_guard::status_text(d.holding, d.requesting);
            for s in self.settings.iter_mut().chain(self.profiles_win.iter_mut()) {
                s.sleep_status = self.sleep_status.clone();
                s.request_redraw();
            }
        }
        self.sleep_tracker.due()
    }
}

#[cfg(test)]
mod tests {
    /// The request is looked at in the loop's wake pass, folded into the one `wake` array.
    #[test]
    fn sleep_is_wired_into_the_wake() {
        let m = include_str!("mod.rs");
        assert!(m.contains("let sleep_due = self.recompute_sleep(Instant::now());"));
        assert!(m.contains("flicker_due, progress_due, sleep_due]"), "folded into the one wake array");
        assert!(m.contains("sleep: Default::default(),") && m.contains("s.sleep_status = self.sleep_status.clone();"));
        let me = include_str!("sleep_ui.rs").split("#[cfg(test)]").next().unwrap();
        assert!(!me.contains("thread::spawn") && !me.contains("set_timer"), "no second timer");
        assert!(include_str!("../settings_ui.rs").contains("s.key == \"Prevent Sleep\""));
    }
}
