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

impl App {
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
