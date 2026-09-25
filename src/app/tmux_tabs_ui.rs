//! tmux windows as Tandem tabs: what the controller's actions do to panes, tabs, and
//! windows (iTerm2's TmuxWindowOpener and PTYSession's tmux window handlers).

use super::*;
use crate::tmux_controller::{Action, PaneOpen};
use crate::tmux_layout::{self, Node};
use crate::tmux_windows::{Place, Window};

impl App {
    /// Applies the controller's actions for the gateway `gw`. False when tmux mode ended.
    pub(super) fn tmux_actions(&mut self, gw: PaneId, actions: Vec<Action>, event_loop: &ActiveEventLoop) -> bool {
        for a in actions {
            match a {
                Action::Open { window, node, panes, place } => self.tmux_open(gw, window, node, panes, place),
                Action::Relayout { window, node, added, removed } => self.tmux_relayout(gw, window, node, added, removed, event_loop),
                Action::Close(window) => {
                    let ids = self.tmux_window_panes(gw, window);
                    self.tmux_forget(gw, &ids);
                    for id in ids {
                        self.close_pane(id, event_loop);
                    }
                }
                Action::Rename(window, name) => {
                    let title = self.tmux_title(&name);
                    if let Some(first) = self.tmux_window_panes(gw, window).first().copied() {
                        self.with_tab_of(first, |t| t.title = Some(title));
                    }
                }
                Action::CloseAll => {
                    let ids: Vec<PaneId> = self.tmux.get(&gw).map(|t| t.panes.values().map(|(id, _)| *id).collect()).unwrap_or_default();
                    self.tmux_forget(gw, &ids);
                    for id in ids {
                        self.close_pane(id, event_loop);
                    }
                }
                Action::Notify(title, text) => {
                    if let Err(e) = crate::notify::show_with(title, text, self.globals.bool("silentUserNotifications")) {
                        self.banners.set("notify", format!("Could not show a notification: {e}"));
                    }
                }
                Action::Abort(msg) => {
                    self.tmux_abort(gw, "tmux Reported a Problem", &msg);
                    return false;
                }
                Action::Output(pane, data) => {
                    let Some(id) = self.tmux.get(&gw).and_then(|t| t.panes.get(&pane)).map(|(id, _)| *id) else { continue };
                    if let Some(p) = self.panes.get_mut(&id) {
                        if let Some(io) = p.tmux_io() {
                            io.push_output(&data);
                        }
                        // Read now: nothing else wakes the loop for it.
                        p.pump();
                    }
                }
                Action::SelectPane(window, pane) => {
                    let Some(id) = self.tmux.get(&gw).and_then(|t| t.panes.get(&pane)).map(|(id, _)| *id) else { continue };
                    let _ = window;
                    self.with_tab_of(id, |t| t.focused = id);
                }
                Action::SelectWindow(window) => {
                    if let Some(first) = self.tmux_window_panes(gw, window).first().copied() {
                        if self.enter_pane_window(first) {
                            if let Some(i) = self.tabs.iter().position(|t| t.layout.panes().contains(&first)) {
                                self.active = i;
                            }
                        }
                    }
                }
            }
        }
        self.relayout();
        true
    }

    /// The Tandem panes of a tmux window, first to last.
    fn tmux_window_panes(&self, gw: PaneId, window: u32) -> Vec<PaneId> {
        let Some(t) = self.tmux.get(&gw) else { return Vec::new() };
        let mut v: Vec<(u32, PaneId)> = t.panes.iter().filter(|(_, (_, w))| *w == window).map(|(p, (id, _))| (*p, *id)).collect();
        v.sort();
        v.into_iter().map(|(_, id)| id).collect()
    }

    fn tmux_forget(&mut self, gw: PaneId, ids: &[PaneId]) {
        if let Some(t) = self.tmux.get_mut(&gw) {
            t.panes.retain(|_, (id, _)| !ids.contains(id));
        }
    }

    /// Runs `f` on the tab that holds `pane`, in whichever window has it.
    fn with_tab_of(&mut self, pane: PaneId, f: impl FnOnce(&mut Tab)) {
        if self.enter_pane_window(pane) {
            if let Some(t) = self.tabs.iter_mut().find(|t| t.layout.panes().contains(&pane)) {
                f(t);
            }
        }
    }

    /// tmuxTitlePrefix before the window's name.
    fn tmux_title(&self, name: &str) -> String {
        format!("{}{name}", self.globals.string("tmuxTitlePrefix"))
    }

    /// A tmux pane of the gateway `gw`, rebuilt from its captures.
    fn tmux_pane(&mut self, gw: PaneId, window: u32, p: &PaneOpen) -> PaneId {
        let id = self.next_id;
        self.next_id += 1;
        let guid = self.tmux_profile_guid(gw);
        let mut pane = Pane::tmux(id, gw, p.pane, p.w.clamp(1, u16::MAX as u32) as u16, p.h.clamp(1, u16::MAX as u32) as u16);
        let profile = self.profile(Some(&guid));
        pane.restyle(Palette::from_profile(&profile, self.dark()), Look::from_profile(&profile));
        let (list, prefs) = triggers_ui::triggers_of(&profile);
        pane.screen.set_triggers(list, prefs);
        pane.profile = profile.guid();
        pane.own_mut().feed_tmux_restore(&p.restore);
        pane.feed_output(&p.pending);
        self.panes.insert(id, pane);
        if let Some(t) = self.tmux.get_mut(&gw) {
            t.panes.insert(p.pane, (id, window));
        }
        id
    }

    fn tmux_open(&mut self, gw: PaneId, window: Window, node: Node, panes: Vec<PaneOpen>, place: Place) {
        let mut ids = HashMap::new();
        for p in &panes {
            ids.insert(p.pane, self.tmux_pane(gw, window.id, p));
        }
        let Some(layout) = tmux_layout::to_layout(&node, &ids) else { return };
        let focused = ids.get(&panes.first().map_or(0, |p| p.pane)).copied().unwrap_or(0);
        let tab = Tab { id: self.next_tab_id, layout, focused, zoomed: None, title: Some(self.tmux_title(&window.name)) };
        self.next_tab_id += 1;
        let others_here = |app: &App| app.tabs.iter().flat_map(|t| t.layout.panes()).any(|id| app.panes.get(&id).and_then(|p| p.tmux_io()).is_some_and(|io| io.gateway != gw));
        let target = match place {
            Place::WithSibling(w) => self.tmux_window_panes(gw, w).into_iter().find(|id| !ids.values().any(|x| x == id)),
            Place::GatewayWindow => Some(gw),
            Place::Focused => (!others_here(self)).then(|| self.tabs.get(self.active).map(|t| t.focused)).flatten(),
            Place::Topmost => self.tmux.get(&gw).and_then(|t| t.panes.values().map(|(id, _)| *id).find(|id| !ids.values().any(|x| x == id))),
            Place::NewWindow => None,
        };
        match target {
            Some(anchor) if self.enter_pane_window(anchor) => {
                self.tabs.push(tab);
                self.active = self.tabs.len() - 1;
                self.relayout();
            }
            _ => self.pending_windows.push(windows_ui::NewWindow::Tabs(vec![tab])),
        }
    }

    fn tmux_relayout(&mut self, gw: PaneId, window: u32, node: Node, added: Vec<PaneOpen>, removed: Vec<u32>, event_loop: &ActiveEventLoop) {
        let gone: Vec<PaneId> = removed.iter().filter_map(|p| self.tmux.get(&gw).and_then(|t| t.panes.get(p)).map(|(id, _)| *id)).collect();
        for p in &added {
            self.tmux_pane(gw, window, p);
        }
        let ids: HashMap<u32, PaneId> = self.tmux.get(&gw).map(|t| t.panes.iter().filter(|(_, (_, w))| *w == window).map(|(p, (id, _))| (*p, *id)).collect()).unwrap_or_default();
        // The kept panes take tmux's new sizes.
        for (p, w, h) in tmux_layout::sizes(&node) {
            if let Some(pane) = ids.get(&p).and_then(|id| self.panes.get_mut(id)) {
                pane.set_tmux_size(w.clamp(1, u16::MAX as u32) as u16, h.clamp(1, u16::MAX as u32) as u16);
            }
        }
        if let Some(layout) = tmux_layout::to_layout(&node, &ids) {
            let anchor = ids.values().copied().find(|id| !gone.contains(id));
            if let Some(anchor) = anchor {
                self.with_tab_of(anchor, |t| {
                    if !layout.panes().contains(&t.focused) {
                        t.focused = anchor;
                    }
                    t.layout = layout;
                });
            }
        }
        self.tmux_forget(gw, &gone);
        for id in gone {
            self.close_pane(id, event_loop);
        }
    }

    /// The profile tmux panes use (Task 6 adds the dedicated "tmux" profile).
    pub(super) fn tmux_profile_guid(&self, gw: PaneId) -> String {
        self.panes.get(&gw).map(|p| p.profile.clone()).unwrap_or_default()
    }

    /// The gateway ended: its panes' sessions end (they follow "After a session ends").
    pub(super) fn tmux_end_panes(&mut self, gw: PaneId) {
        let ids: Vec<PaneId> = self.panes.iter().filter(|(_, p)| p.tmux_io().is_some_and(|io| io.gateway == gw)).map(|(id, _)| *id).collect();
        for id in ids {
            if let Some(io) = self.panes.get(&id).and_then(|p| p.tmux_io()) {
                io.end();
            }
        }
    }
}
