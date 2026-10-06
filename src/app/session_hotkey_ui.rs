//! Session hotkeys in the window (iTerm2's `iTermSessionHotkeyController`): a session's own hotkey
//! brings it forward, cycling through the sessions that share the chord. A hotkey works in every
//! app, so only the person's own choice registers: one a program wrote (a script, the API, an
//! escape sequence, an arrangement, an import, a tmux server's `@hotkeys`) is stored with its
//! session, inert, until the person allows it.

use super::scripts_alerts::{self, Then};
use super::*;
use crate::hotkey::Shortcut;
use crate::session_hotkeys::{self, Origin};
use crate::warnings::Kind;

/// The session value's key.
const KEY: &str = "Session Hotkey";
/// The question's identifier; only Ignore is ever remembered.
const ASK_ID: &str = "NoSyncSessionHotkeyFromProgram";
const REGISTER: &str = "Register";
const IGNORE: &str = "Ignore";

impl App {
    /// The pane's session hotkey value (the pane's own profile copy), whoever wrote it.
    pub(super) fn session_hotkey_value(&self, pane: PaneId) -> Option<Shortcut> {
        let p = self.panes.get(&pane)?;
        self.store.get(&p.profile)?.get(KEY).and_then(|v| Shortcut::from_value(&v))
    }

    /// The person set (Some) or cleared (None) a session's hotkey from Edit Session: a session
    /// value on the pane's own copy, never the shared profile, and it registers at once.
    pub(super) fn set_session_hotkey(&mut self, pane: PaneId, s: Option<Shortcut>) {
        let Some(guid) = self.divorce(pane) else { return };
        self.store.set_session(&guid, &[(KEY.to_string(), s.as_ref().map(Shortcut::to_value))]);
        self.session_hotkeys.person_set(pane, s);
        self.after_session_hotkeys(pane);
        self.register_hotkeys();
    }

    /// A program wrote (Some) or cleared (None) a pane's session hotkey: it is stored with the
    /// session by the caller, and here it is made inert, and the person is asked.
    pub(super) fn session_hotkey_from_program(&mut self, pane: PaneId, s: Option<Shortcut>, origin: Origin) {
        let written = self.session_hotkeys.program_set(pane, s, origin);
        self.session_hotkeys_dirty = true;
        // The question this one replaces is dropped if it still waits in the queue.
        if let Some(old) = written.stale {
            self.alert_drop_queued(|t| matches!(t, Then::SessionHotkey { pane: p, shortcut, .. } if *p == pane && *shortcut == old));
        }
        // Answered by an earlier decision for the same tmux session and value: it may be on already.
        if self.session_hotkeys.live().iter().any(|(p, _)| *p == pane) {
            self.after_session_hotkeys(pane);
        }
        let Some((pane, s, origin)) = written.ask else { return };
        // A chord that could never register is not worth a question.
        let Ok(chord) = session_hotkeys::check(&s, &[]) else { return };
        if self.alert_waits(|t| matches!(t, Then::SessionHotkey { pane: p, shortcut, .. } if *p == pane && *shortcut == s)) {
            return;
        }
        // The question names the chord that would register, never the shortcut's own characters.
        let text = session_hotkeys::consent_text(&origin, &session_hotkeys::chord_label(&chord));
        let ask = scripts_alerts::ask("Register a hotkey?", &text, &[REGISTER, IGNORE], Some(ASK_ID), Kind::Permanent, Then::SessionHotkey { pane, shortcut: s });
        self.ask_scripts(ask);
    }

    /// A tmux pane opened with a hotkey in the server's `@hotkeys`: stored with the pane, inert.
    pub(super) fn tmux_pane_hotkey_arrived(&mut self, pane: PaneId, s: Shortcut, session: &str) {
        let Some(guid) = self.divorce(pane) else { return };
        self.store.set_session(&guid, &[(KEY.to_string(), Some(s.to_value()))]);
        self.session_hotkey_from_program(pane, Some(s), Origin::Tmux(session.to_string()));
    }

    /// The answer to the question.
    pub(super) fn session_hotkey_answered(&mut self, pane: PaneId, s: Shortcut, index: usize) {
        if index == 0 && self.panes.contains_key(&pane) && self.session_hotkey_value(pane).as_ref() == Some(&s) && self.session_hotkeys.allow(pane, &s) {
            self.after_session_hotkeys(pane);
            self.register_hotkeys();
        } else {
            self.session_hotkeys.ignore(pane, &s);
        }
    }

    /// A session ended: its hotkey goes (registered again without it on the next pass).
    pub(super) fn session_hotkey_pane_gone(&mut self, pane: PaneId) {
        self.session_hotkeys.pane_gone(pane);
        self.session_hotkeys_dirty = true;
        if let Some(s) = self.settings.as_mut().filter(|s| s.edit_session == Some(pane)) {
            s.edit_session = None;
            s.session_hotkey = None;
        }
    }

    /// Once per pass: registrations are rebuilt when a session ended or a value changed.
    pub(super) fn session_hotkeys_flush(&mut self) {
        if std::mem::take(&mut self.session_hotkeys_dirty) && self.hotkeys_registered {
            self.register_hotkeys();
        }
    }

    /// Settings' row shows the edited session's hotkey; a tmux pane's goes to `@hotkeys`.
    fn after_session_hotkeys(&mut self, pane: PaneId) {
        self.sync_session_hotkey_row();
        if let Some(io) = self.panes.get(&pane).and_then(|p| p.tmux_io()) {
            self.tmux_save_hotkeys(io.gateway);
        }
    }

    /// The Session Hotkey row's current value.
    pub(super) fn sync_session_hotkey_row(&mut self) {
        let Some(pane) = self.settings.as_ref().and_then(|s| s.edit_session) else { return };
        let value = self.session_hotkey_value(pane);
        if let Some(s) = self.settings.as_mut() {
            s.session_hotkey = value;
        }
    }

    /// Each tmux pane's allowed session hotkey goes to `@hotkeys`.
    pub(super) fn tmux_save_hotkeys(&mut self, gw: PaneId) {
        let allowed: Vec<(u32, Shortcut)> = self
            .session_hotkeys
            .live()
            .iter()
            .filter_map(|(id, s)| {
                let io = self.panes.get(id)?.tmux_io().filter(|io| io.gateway == gw)?;
                Some((io.pane, s.clone()))
            })
            .collect();
        if let Some(t) = self.tmux.get_mut(&gw) {
            t.ctl.save_hotkeys(&allowed, &mut t.gw);
        }
        self.tmux_flush(gw);
    }

    /// A session hotkey press (the group's index): the session after the first responder, else one
    /// active in a window that is not in front, else the first, comes forward.
    pub(super) fn session_hotkey_pressed(&mut self, index: usize) {
        let Some(group) = self.session_hotkey_ids.get(index) else { return };
        let group: Vec<PaneId> = group.iter().copied().filter(|p| self.panes.contains_key(p)).collect();
        let fg = hotkey_ui::foreground();
        let mut windows = vec![(self.hwnd() == Some(fg), self.tabs.get(self.active).map(|t| t.focused))];
        windows.extend(self.windows.parked().map(|(_, s)| (s.gpu.as_ref().and_then(|g| menu::hwnd_of(&g.window)) == Some(fg), s.active_tab().map(|t| t.focused))));
        let (first, background) = session_hotkeys::focus_state(&windows);
        if let Some(pane) = session_hotkeys::pick(&group, first, &background) {
            self.reveal_pane(pane);
        }
    }
}
