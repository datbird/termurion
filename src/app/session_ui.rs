//! Session-local profiles in the window: a pane divorces its profile when a program
//! changes one of its settings, and remarries when it switches profile.

use super::*;

impl App {
    /// The pane's private profile copy, made now if it has none. Its Guid, or None for a
    /// pane that is gone.
    pub(super) fn divorce(&mut self, pane: PaneId) -> Option<String> {
        let guid = self.panes.get(&pane)?.profile.clone();
        if self.store.sessions.iter().any(|s| s.profile.guid() == guid) {
            return Some(guid);
        }
        let shared = self.store.get(&guid).cloned().unwrap_or_else(|| self.store.default_profile().clone());
        let copy = crate::session_profile::divorce(&shared, uuid::Uuid::new_v4().to_string().to_uppercase());
        let new_guid = copy.profile.guid();
        self.store.sessions.push(copy);
        self.panes.get_mut(&pane)?.profile = new_guid.clone();
        Some(new_guid)
    }

    /// The profile a new session based on `pane` starts with (`ProfileModel
    /// profileForCreatingNewSessionBasedOn:`): the pane's saved profile, or, when the pane has
    /// a private copy, a new copy of it with a fresh Guid. Its Guid, or None for a pane that
    /// is gone.
    pub(super) fn profile_for_new_session(&mut self, pane: PaneId) -> Option<String> {
        let guid = self.panes.get(&pane)?.profile.clone();
        let Some(copy) = self.store.sessions.iter().find(|s| s.profile.guid() == guid) else { return Some(guid) };
        let mut copy = copy.clone();
        let fresh = uuid::Uuid::new_v4().to_string().to_uppercase();
        copy.profile.set(crate::profile::GUID, toml::Value::String(fresh.clone()));
        self.store.sessions.push(copy);
        Some(fresh)
    }

    /// Sets or removes (None) values on the pane's own copy of its profile, and redraws.
    pub(super) fn set_session_values(&mut self, pane: PaneId, values: &[(String, Option<toml::Value>)]) {
        let Some(guid) = self.divorce(pane) else { return };
        let Some(i) = self.store.sessions.iter().position(|s| s.profile.guid() == guid) else { return };
        let original = self.store.sessions[i].original_guid();
        let shared = self.store.profiles.iter().find(|p| p.guid() == original).cloned().unwrap_or_else(|| self.store.default_profile().clone());
        self.store.sessions[i].set(values, &shared);
        self.refresh_panes();
        // Show Status Bar on a pane that is not focused, or in another window, changes
        // that pane's room only.
        self.relayout_changed_windows();
    }

    /// The pane takes a saved profile; any private copy (and what it overrode) goes.
    pub(super) fn remarry(&mut self, pane: PaneId, guid: &str) {
        if let Some(p) = self.panes.get_mut(&pane) {
            let old = std::mem::replace(&mut p.profile, guid.to_string());
            self.forget_session_profile(&old);
        }
        self.refresh_panes();
    }

    /// Puts `values` on a session copy, stores it under `guid` and returns the Guid. Pane-free (P8):
    /// the caller forgets it (`forget_session_profile`) when no pane takes it.
    pub(super) fn add_session_copy(&mut self, mut copy: crate::session_profile::SessionProfile, shared: &crate::profile::Profile, values: &[(String, Option<toml::Value>)], guid: String) -> String {
        copy.set(values, shared);
        self.store.sessions.push(copy);
        guid
    }

    /// Drops the private profile copy of a pane that closed.
    pub(super) fn forget_session_profile(&mut self, guid: &str) {
        if self.panes.values().any(|p| p.profile == guid) {
            return;
        }
        self.store.sessions.retain(|s| s.profile.guid() != guid);
    }
}
