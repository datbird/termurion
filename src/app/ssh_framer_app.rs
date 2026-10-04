//! SSH integration at level 2 in the app (milestone 14c): Shell > ssh > Disconnect and its
//! menu title. The framer's other app-side work rides on `App::tick_ssh` and `Pane` effects.

use super::scripts_alerts::{ask, Then};
use super::*;
use crate::warnings::Kind;

/// The heading of the warning for a session that cannot disconnect (`PS:10487-10500`).
pub(super) const CANT_DISCONNECT: &str = "Can\u{2019}t Disconnect";

/// The Disconnect item's text and whether it is enabled, for the focused session's name
/// (`TV:155-163`): the identity's description with a conductor, else nothing to disconnect.
pub(super) fn disconnect_title(name: Option<&str>) -> (String, bool) {
    match name {
        Some(name) => (format!("Disconnect from {name}"), true),
        None => ("Disconnect".to_string(), false),
    }
}

/// The warning's text (`PS:10485-10500`): the host's name, or "remote host" when unknown.
pub(super) fn cant_disconnect_text(host: Option<&str>) -> String {
    format!("Advanced SSH features are unavailable because Python 3.7 or later was not found on {}", host.unwrap_or("remote host"))
}

/// The profile keys Duplicate Session sets on a session that is not an SSH profile
/// (`PT:2790-2792`).
fn duplicate_values(command: &str) -> Vec<(String, Option<toml::Value>)> {
    vec![("Custom Command".to_string(), Some("SSH".into())), ("Command".to_string(), Some(command.into()))]
}

/// The profile keys a split from an ssh session sets (`PS:2948-2953`), read from
/// `ssh_jumps::split_profile`'s result.
fn split_values(base: &crate::profile::Profile, command_line: &str) -> Vec<(String, Option<toml::Value>)> {
    let table = crate::ssh_jumps::split_profile(&base.values.clone().into_iter().collect(), command_line);
    ["Custom Command", "Command", crate::ssh_config::KEY].iter().map(|k| (k.to_string(), table.get(*k).cloned())).collect()
}

impl App {
    /// A session copy of `pane`'s profile with `values` on top, its Guid (None for a pane that is
    /// gone). `from_saved` starts from the saved profile the pane's profile came from, else
    /// from the pane's own profile (with what the session changed).
    fn ssh_session_copy(&mut self, pane: PaneId, from_saved: bool, values: &[(String, Option<toml::Value>)]) -> Option<String> {
        let guid = self.panes.get(&pane)?.profile.clone();
        let saved = self.store.shared_guid(&guid);
        let shared = self.store.profiles.iter().find(|p| p.guid() == saved).unwrap_or_else(|| self.store.default_profile()).clone();
        let fresh = uuid::Uuid::new_v4().to_string().to_uppercase();
        let own = self.store.sessions.iter().find(|s| s.profile.guid() == guid).cloned();
        let copy = match own {
            Some(mut c) if !from_saved => {
                c.profile.set(crate::profile::GUID, toml::Value::String(fresh.clone()));
                c
            }
            _ => crate::session_profile::divorce(&shared, fresh.clone()),
        };
        Some(self.add_session_copy(copy, &shared, values, fresh))
    }

    /// Shell > Duplicate Session (`PT:2776-2808`). With no conductor it is a new session of the
    /// same profile. With one, the new session reconnects through every hop: its profile is an
    /// SSH one (made so with the first hop's arguments when it was not) and the hops go into
    /// its pending jumps, which its hooks use up one by one.
    ///
    /// Beyond iTerm2: the first hop's text becomes a local command line and every hop's text
    /// reaches the eval of the host above, so the hops are `Conductor::reconnect_hops`: only
    /// those Tandem's own token vouched for (the first hook), and only through the one
    /// validator (ruling, final review I1). Any other chain (a hook a program printed) or a
    /// refused hop gets an ordinary new session.
    pub(super) fn duplicate_session(&mut self) {
        let Some(pane) = self.focused_id() else { return };
        let plain = |app: &mut App| {
            let g = app.focused_guid();
            app.new_tab(g.as_deref());
        };
        let sequence = self.panes.get(&pane).and_then(|p| p.conductor.as_deref()).and_then(|c| c.reconnect_hops());
        let Some(sequence) = sequence else { return plain(self) };
        let Some(guid) = self.panes.get(&pane).map(|p| p.profile.clone()) else { return };
        let is_ssh = crate::profile::is_ssh(&self.profile(Some(&guid)));
        let plan = crate::ssh_jumps::duplicate_plan(is_ssh, &sequence);
        let new_guid = match &plan.command {
            Some(command) => self.ssh_session_copy(pane, false, &duplicate_values(command)),
            None => self.profile_for_new_session(pane),
        };
        let Some(new_guid) = new_guid else { return };
        // A new tab that opens as a window cannot carry the hops.
        if self.new_tab_opens_window(Some(&new_guid)) {
            if plan.command.is_some() {
                self.forget_session_profile(&new_guid);
            }
            return plain(self);
        }
        self.start_jumps_override = Some(plan.jumps);
        self.new_tab(Some(&new_guid));
        // Nothing started (the layout is locked): the copy goes.
        if self.start_jumps_override.take().is_some() {
            self.forget_session_profile(&new_guid);
        }
    }

    /// The profile a split of `pane` starts with (`PS:2915-2953`): with a conductor, the
    /// saved profile made an SSH one to the innermost host (`Identity::command_line`); else the
    /// pane's saved profile, as before. Only an innermost conductor Tandem's own token vouched
    /// for names the host (`Conductor::split_identity`); a nested or restored one gives the
    /// saved profile.
    pub(super) fn ssh_split_profile(&mut self, pane: PaneId) -> Option<String> {
        let identity = self.panes.get(&pane).and_then(|p| p.conductor.as_deref()).and_then(|c| c.split_identity());
        match identity {
            Some(id) => {
                let saved = self.store.shared_guid(&self.panes.get(&pane)?.profile);
                let base = self.profile(Some(&saved));
                self.ssh_session_copy(pane, true, &split_values(&base, &id.command_line()))
            }
            None => self.panes.get(&pane).map(|p| self.store.shared_guid(&p.profile)),
        }
    }

    /// Shell > Split Vertically or Horizontally with Current Profile.
    pub(super) fn split_current(&mut self, axis: Axis) {
        let guid = self.focused_id().and_then(|id| self.ssh_split_profile(id));
        self.split(axis, guid.as_deref());
        // A copy no pane took (the split was refused) goes.
        if let Some(g) = guid {
            self.forget_session_profile(&g);
        }
    }

    /// Shell > ssh > Disconnect for `pane` (`PS:10480-10501`). No conductor: nothing. A conductor
    /// that does not frame cannot quit: the alert says so (Persistent, OK only; closing it is
    /// OK, there is nothing to refuse, ruling F7). Framing: `quit`, whose effects the pane
    /// applies (the banner, the unhook).
    pub(super) fn ssh_disconnect(&mut self, pane: PaneId) {
        let Some(c) = self.panes.get(&pane).and_then(|p| p.conductor.as_deref()) else { return };
        if !c.framing() {
            // 14b's hostname is a String: empty means unknown.
            let host = Some(c.identity().hostname).filter(|h| !h.is_empty());
            let text = cant_disconnect_text(host.as_deref());
            self.ask_scripts(ask(CANT_DISCONNECT, &text, &[crate::api_auth::OK], None, Kind::Persistent, Then::Nothing));
            return;
        }
        if let Some(c) = self.panes.get_mut(&pane).and_then(|p| p.conductor.as_mut()) {
            c.quit();
        }
        self.ssh_apply(pane);
    }

    /// Retitles Shell > ssh > Disconnect for the focused pane, applied only when it changed
    /// (`TV:155-163`; the last shown value is per window, `WinSlot`). Called by `tick_ssh`.
    pub(super) fn refresh_ssh_menu(&mut self) {
        let name = self.tabs.get(self.active).and_then(|t| self.panes.get(&t.focused)).and_then(|p| p.conductor.as_ref()).map(|c| c.identity().description());
        if self.ssh_menu_shown.as_ref() == Some(&name) {
            return;
        }
        let (text, enabled) = disconnect_title(name.as_deref());
        let Some(item) = self.menu_bar.as_ref().and_then(|b| b.ssh_disconnect.as_ref()) else { return };
        item.set_text(text.replace('&', "&&"));
        item.set_enabled(enabled);
        self.ssh_menu_shown = Some(name);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_and_duplicate_profiles_follow_iterm2() {
        // `PT:2790-2792`: only the command type and the command.
        let keys: Vec<String> = duplicate_values("h1").into_iter().map(|(k, _)| k).collect();
        assert_eq!(keys, ["Custom Command", "Command"]);
        // `PS:2948-2953`: the type, the command line, an empty SSH configuration.
        let mut base = crate::profile::Profile::default();
        base.set("Custom Command", "Yes".into());
        let v = split_values(&base, "-l u h");
        assert_eq!(v[0], ("Custom Command".to_string(), Some("SSH".into())));
        assert_eq!(v[1], ("Command".to_string(), Some("-l u h".into())));
        assert_eq!(v[2], ("SSH".to_string(), Some(toml::Value::Table(toml::Table::new()))));
    }

    #[test]
    fn disconnect_menu_follows_iterm2() {
        // `TV:155-163`: the title names the session and the item is enabled with one.
        assert_eq!(disconnect_title(Some("u@h")), ("Disconnect from u@h".to_string(), true));
        assert_eq!(disconnect_title(None), ("Disconnect".to_string(), false));
    }

    #[test]
    fn cant_disconnect_follows_iterm2() {
        // `PS:10487-10500` and `PS:25603-25612`.
        assert_eq!(CANT_DISCONNECT, "Can\u{2019}t Disconnect");
        assert_eq!(cant_disconnect_text(Some("example.com")), "Advanced SSH features are unavailable because Python 3.7 or later was not found on example.com");
        assert!(cant_disconnect_text(None).ends_with("on remote host"));
        assert_eq!(crate::conductor_framer::disconnected_banner("u@h"), "Disconnected from u@h");
    }
}
