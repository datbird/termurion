//! Session > Bury Session and Session > Buried Sessions (iTerm2's iTermBuriedSessions): a
//! buried session keeps running outside every tab until it is restored.

use super::*;
use crate::bury::{self, Buried, Restore};

/// Menu ids: a buried session, and a hidden tmux window ("buried-tmux:GW:WINDOW").
pub(super) const BURIED_ID: &str = "buried:";
pub(super) const BURIED_TMUX_ID: &str = "buried-tmux:";

impl App {
    /// Buries a session. The last session of the last window stays (Tandem ends when its
    /// last window closes). True when it was buried.
    pub(super) fn bury_pane(&mut self, id: PaneId) -> bool {
        // A tmux pane is not buried: its tmux window is hidden (iTerm2's bury of a tmux client).
        if let Some((gw, _, window)) = self.tmux_of(id) {
            self.tmux_note_burial(gw, window);
            let actions = self.tmux.get_mut(&gw).map(|t| t.ctl.hide(window, &mut t.gw)).unwrap_or_default();
            self.tmux_flush(gw);
            self.tmux_pending_close.push((gw, actions, false));
            self.menus_dirty = true;
            return true;
        }
        if !self.enter_pane_window(id) {
            return false;
        }
        let only = self.tabs.len() == 1 && self.tabs[0].layout.panes().len() == 1 && self.windows.parked().next().is_none() && self.pending_windows.is_empty();
        if only {
            self.banners.set("bury", "The last session cannot be buried: Tandem ends when its last window closes.");
            return false;
        }
        let Some(t) = self.tabs.iter().position(|t| t.layout.panes().contains(&id)) else { return false };
        let (Some(window), tab) = (self.windows.current(), self.tabs[t].id) else { return false };
        let name = self.session_title(id);
        let tabref = &mut self.tabs[t];
        if tabref.layout.remove(id) {
            if tabref.focused == id {
                tabref.focused = tabref.layout.panes()[0];
            }
            if tabref.zoomed == Some(id) {
                tabref.zoomed = None;
            }
        } else {
            let gone = self.tabs.remove(t);
            self.mru.forget(gone.id);
            self.api_vars.forget(crate::api_vars::Scope::Tab, &gone.id.to_string());
            self.active = crate::close_rules::active_after_close(self.active, t, self.tabs.len(), self.globals.bool("moveLeftAfterClosingTab"));
            // The neighbor selected may be hidden in a collapsed group.
            self.expand_selected_group();
        }
        self.buried.bury(Buried { pane: id, window, tab, name });
        self.menus_dirty = true;
        self.refresh_buried_menu();
        if self.tabs.is_empty() {
            self.drop_current_window();
        } else {
            self.relayout();
        }
        true
    }

    /// Restores a buried session where it was (iTerm2's restoreSession).
    pub(super) fn disinter(&mut self, id: PaneId) {
        let Some(b) = self.buried.take(id) else { return };
        self.menus_dirty = true;
        self.refresh_buried_menu();
        let window = self.window_id_of_key(b.window);
        let window_open = window.is_some_and(|w| self.enter(w));
        let tab_open = window_open && self.tabs.iter().any(|t| t.id == b.tab);
        match bury::restore_target(&b, window_open, tab_open) {
            Restore::InTab(_, tab) => {
                if let Some(i) = self.tabs.iter().position(|t| t.id == tab) {
                    let t = &mut self.tabs[i];
                    let f = t.focused;
                    t.layout.split(f, Axis::Row, id);
                    t.focused = id;
                    self.select_tab(i);
                }
                self.relayout();
            }
            Restore::NewTab(_) => {
                self.tabs.push(Tab::new(self.next_tab_id, Layout::new(id), id));
                self.next_tab_id += 1;
                self.active = self.tabs.len() - 1;
                self.relayout();
            }
            Restore::NewWindow => {
                let tab = Tab::new(self.next_tab_id, Layout::new(id), id);
                self.next_tab_id += 1;
                self.pending_windows.push(windows_ui::NewWindow::Tabs(vec![tab]));
            }
        }
    }

    /// The newest buried session, as a new tab of the current (otherwise empty) window.
    pub(super) fn disinter_here(&mut self) {
        let Some(id) = self.buried.newest() else { return };
        self.buried.take(id);
        self.menus_dirty = true;
        self.refresh_buried_menu();
        self.tabs.push(Tab::new(self.next_tab_id, Layout::new(id), id));
        self.next_tab_id += 1;
        self.active = self.tabs.len() - 1;
        self.relayout();
    }

    /// The Buried Sessions entries: the buried sessions, then the hidden tmux windows, as
    /// (menu id, label); an empty id is a separator. The menu bar and the notification area
    /// icon's menu both list these.
    pub(super) fn buried_entries(&self) -> Vec<(muda::MenuId, String)> {
        let mut items: Vec<(String, String)> = self.buried.items().into_iter().map(|(p, n)| (format!("{BURIED_ID}{p}"), n)).collect();
        let prefix = self.globals.string("tmuxTitlePrefix");
        let mut gws: Vec<PaneId> = self.tmux.keys().copied().collect();
        gws.sort_unstable();
        let mut tmux_items = Vec::new();
        for gw in gws {
            let Some(t) = self.tmux.get(&gw) else { continue };
            let names: HashMap<u32, String> = t.ctl.session_windows.values().flatten().map(|w| (w.id, w.name.clone())).collect();
            for w in t.ctl.hidden() {
                let name = names.get(w).cloned().unwrap_or_else(|| format!("@{w}"));
                tmux_items.push((format!("{BURIED_TMUX_ID}{gw}:{w}"), bury::hidden_window_title(&prefix, &t.client, &name)));
            }
        }
        if !items.is_empty() && !tmux_items.is_empty() {
            items.push((String::new(), String::new()));
        }
        items.extend(tmux_items);
        items.into_iter().map(|(id, label)| (muda::MenuId::new(id), label)).collect()
    }

    /// Session > Buried Sessions: rebuilt from `buried_entries`.
    pub(super) fn refresh_buried_menu(&mut self) {
        let items: Vec<(String, String)> = self.buried_entries().into_iter().map(|(id, label)| (id.0, label)).collect();
        let Some(bar) = &mut self.menu_bar else { return };
        if bar.buried_items == items {
            return;
        }
        if let Some(sub) = &bar.buried {
            while sub.remove_at(0).is_some() {}
            for (id, label) in &items {
                if id.is_empty() {
                    let _ = sub.append(&muda::PredefinedMenuItem::separator());
                } else {
                    let _ = sub.append(&muda::MenuItem::with_id(id.clone(), label.replace('&', "&&"), true, None));
                }
            }
            sub.set_enabled(!items.is_empty());
        }
        bar.buried_items = items;
    }

    /// A pick from Buried Sessions. False for other ids.
    pub(super) fn buried_picked(&mut self, id: &str) -> bool {
        if let Some(p) = id.strip_prefix(BURIED_ID).and_then(|p| p.parse::<u64>().ok()) {
            self.disinter(p);
            return true;
        }
        if let Some(rest) = id.strip_prefix(BURIED_TMUX_ID) {
            if let Some((gw, w)) = rest.split_once(':').and_then(|(g, w)| Some((g.parse::<u64>().ok()?, w.parse::<u32>().ok()?))) {
                if let Some(t) = self.tmux.get_mut(&gw) {
                    t.ctl.open_windows(&[w], false, &mut t.gw);
                }
                self.tmux_flush(gw);
            }
            return true;
        }
        false
    }
}
