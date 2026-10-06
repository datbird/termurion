//! Session-local profiles in the window: a pane divorces its profile when a program
//! changes one of its settings, and remarries when it switches profile.

use super::*;

impl App {
    /// The pane's private profile copy, made now if it has none. Its Guid, or None for a
    /// pane that is gone.
    pub(super) fn divorce(&mut self, pane: PaneId) -> Option<String> {
        let guid = self.panes.get(&pane)?.profile.clone();
        let new_guid = self.store.divorce(&guid);
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

    /// Sets or removes (None) values on the pane's own copy of its profile, and redraws. It never
    /// drops a palette-owned binding (`PS:8384-8394`); the Settings color writers do
    /// (`bindings::drop_for_write`).
    pub(super) fn set_session_values(&mut self, pane: PaneId, values: &[(String, Option<toml::Value>)]) {
        let Some(guid) = self.divorce(pane) else { return };
        self.store.set_session(&guid, values);
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
