//! Edit Session: the Settings window open for one session follows the active session
//! (`PseudoTerminal.m:5047-5049`), and Copy Current Settings to Selected Profile.

use super::*;

impl App {
    /// Points the open Settings window at the profile `pane` uses (Settings alone without a
    /// session): the profile selected, the tmux state, and the pane Edit Session is open for.
    pub(super) fn point_edit_session(&mut self, pane: Option<PaneId>) {
        let guid = pane.and_then(|id| self.panes.get(&id)).map(|p| self.store.shared_guid(&p.profile));
        let tmux = pane.is_some_and(|id| self.panes.get(&id).is_some_and(|p| p.tmux_io().is_some()));
        let Some(s) = self.settings.as_mut() else { return };
        s.edit_session = pane.filter(|_| guid.is_some());
        if let Some(g) = guid {
            s.select(&g);
            // iTerm2's `tmux:self.isTmuxClient` (PTYSession.m:12333-12338).
            s.set_tmux_session(tmux);
        }
    }

    /// Edit Session follows the newly active session unless Pin Edit Session is on (R3). Called
    /// from `refresh_route`, which every focus, tab, window and profile change passes through.
    pub(super) fn follow_edit_session(&mut self) {
        let now = self.focused_id();
        let editing = self.settings.as_ref().and_then(|s| s.edit_session);
        let pin = self.globals.bool("PinEditSession");
        let seen = std::mem::replace(&mut self.edit_follow_seen, now);
        if let Some(pane) = crate::settings_ui::follow_target(seen, now, pin, editing) {
            self.point_edit_session(Some(pane));
        }
    }

    /// Replace the destination profile with the session's settings, after the question
    /// (`copyToProfile:`): the source is the profile the session uses (its own copy when it has
    /// one), the destination a saved profile.
    pub(super) fn copy_to_profile(&mut self, pane: PaneId, destination: &str) {
        let Some(source) = self.panes.get(&pane).and_then(|p| self.store.get(&p.profile)).cloned() else { return };
        let Some(at) = self.store.profiles.iter().position(|p| p.guid() == destination) else { return };
        let copy = crate::settings_ui::copied_profile(&source, &self.store.profiles[at]);
        self.store.profiles[at] = copy;
        self.store.merge_sessions();
        self.refresh_panes();
        self.save_due = Some(Instant::now() + SAVE_DELAY);
        self.menus_dirty = true;
        self.redraw();
    }
}
