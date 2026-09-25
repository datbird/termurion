//! Broadcast input in the window: the window's broadcast state as the model sees it, and
//! the one way user input reaches sessions (the sender's domain, or the sender alone).

use super::*;
use crate::broadcast::{self, Route, TabView, View, Write};

impl App {
    /// The current window as the broadcast model sees it.
    pub(super) fn broadcast_view(&self) -> View {
        let tabs = self.tabs.iter().map(|t| TabView { broadcasting: self.broadcast_tabs.contains(&t.id), panes: t.layout.panes() }).collect();
        let current = self.tabs.get(self.active).map_or(0, |t| t.focused);
        let exited = self.panes.iter().filter(|(_, p)| p.exited.is_some()).map(|(id, _)| *id).collect();
        View { tabs, current_tab: self.active, current, exited }
    }

    /// Writes the model's tab flags back.
    pub(super) fn keep_broadcast_flags(&mut self, v: &View) {
        self.broadcast_tabs = self.tabs.iter().zip(&v.tabs).filter(|(_, tv)| tv.broadcasting).map(|(t, _)| t.id).collect();
    }

    /// User input from `sender`: to every session of its broadcast domain (the same bytes,
    /// as the sender encoded them), or to the sender alone.
    pub(super) fn user_write(&mut self, sender: PaneId, bytes: &[u8], what: Write) {
        let targets = match broadcast::route(what) {
            Route::Broadcast => self.broadcast.targets(&self.broadcast_view(), sender),
            Route::Local => vec![sender],
        };
        for t in targets {
            if let Some(p) = self.panes.get_mut(&t) {
                p.input(bytes);
            }
        }
    }

    /// A paste from `sender`: bracketed by the sender's mode, then written like typing.
    pub(super) fn user_paste(&mut self, sender: PaneId, text: &str, what: Write) {
        let Some(bytes) = self.panes.get(&sender).map(|p| p.paste_payload(text)) else { return };
        self.user_write(sender, &bytes, what);
    }
}
