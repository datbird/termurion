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
        s.edit_session_profile = guid.clone().filter(|_| s.edit_session.is_some());
        if let Some(g) = guid {
            s.select(&g);
            // iTerm2's `tmux:self.isTmuxClient` (PTYSession.m:12333-12338).
            s.set_tmux_session(tmux);
        }
        // The Session Hotkey row (13u) shows the pane Edit Session now points at, so a follow or
        // a re-point moves the row with it; Settings alone has no row and no value.
        if s.edit_session.is_none() {
            s.session_hotkey = None;
            s.session_hotkey_waiting = false;
            s.session_hotkey_refused = None;
        }
        self.sync_session_hotkey_row();
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

#[cfg(test)]
mod tests {
    fn body(src: &'static str, name: &str) -> &'static str {
        let start = src.find(name).unwrap_or_else(|| panic!("{name}"));
        let rest = &src[start..];
        &rest[..rest.find("\n    }\n").expect("end of fn")]
    }

    /// S3 (13t x 13u): the Session Hotkey row shows the pane Edit Session points at. Every way
    /// the pane changes (opened from the Session menu, followed to the active session, its pane
    /// closed) moves or clears the row in the same block.
    #[test]
    fn session_hotkey_row_follows_edit_session() {
        let point = body(include_str!("edit_session_ui.rs"), "fn point_edit_session(");
        assert!(point.contains("self.sync_session_hotkey_row();"), "a re-point reads the new pane's hotkey");
        assert!(point.contains("s.session_hotkey = None;"), "Settings alone shows no session value");
        // Follow goes through point_edit_session, so it moves the row too.
        assert!(body(include_str!("edit_session_ui.rs"), "fn follow_edit_session(").contains("self.point_edit_session(Some(pane));"));
        // The Session menu's Edit Session is point_edit_session (13t's), not a second copy.
        let menu = body(include_str!("menu.rs"), "fn edit_session(&mut self");
        assert!(menu.contains("self.point_edit_session(pane);") && !menu.contains("s.edit_session ="));
        // A closed pane: both drop paths clear the field and the row through 13t's rule.
        let gone = body(include_str!("mod.rs"), "fn pane_gone(&mut self");
        assert!(gone.contains("edit_session_after_close") && gone.contains("s.session_hotkey = None;"));
        let hk = body(include_str!("session_hotkey_ui.rs"), "fn session_hotkey_pane_gone(");
        assert!(hk.contains("edit_session_after_close") && hk.contains("s.session_hotkey_waiting = false;"));
    }

    /// S5: `refresh_route` hands the hook both milestones' parts in one snapshot (13u's Respect
    /// Windows shortcuts, 13t's claimed profile keys) and then lets Edit Session follow.
    #[test]
    fn refresh_route_carries_both_milestones() {
        let route = body(include_str!("mod.rs"), "fn refresh_route(&mut self)");
        let at = |n: &str| route.find(n).unwrap_or_else(|| panic!("refresh_route sets {n}"));
        assert!(at("snapshot.respect_system =") < at("*self.route_snapshot.write()"));
        assert!(at("snapshot.profile_shortcuts = keyroute::claimed_shortcuts(") < at("*self.route_snapshot.write()"));
        assert!(at("*self.remap_tags.write()") < at("self.follow_edit_session();"));
    }
}
