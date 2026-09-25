//! tmux windows as Tandem tabs: what the controller's actions do to panes, tabs, and
//! windows (iTerm2's TmuxWindowOpener and PTYSession's tmux window handlers).

use super::*;
use crate::tmux_controller::{Action, PaneOpen};
use crate::tmux_layout::{self, Node};
use crate::tmux_windows::{Place, Window};

pub(super) const PLANE_BAR: &str = "tmux-22-plane";
/// Close questions: "tmux-close-tab:GW:WINDOW" and "tmux-close-win:GW" (every tmux tab of
/// this window).
const CLOSE_TAB: &str = "tmux-close-tab:";
const CLOSE_WIN: &str = "tmux-close-win:";

impl App {
    /// Applies the controller's actions for the gateway `gw`. False when tmux mode ended.
    pub(super) fn tmux_actions(&mut self, gw: PaneId, actions: Vec<Action>, event_loop: &ActiveEventLoop) -> bool {
        // Output alone changes no pane or tab: no relayout for it.
        let changes = actions.iter().any(|a| !matches!(a, Action::Output(..)));
        for a in actions {
            match a {
                Action::Open { window, node, panes, place } => self.tmux_open(gw, window, node, panes, place),
                Action::Relayout { window, node, added, removed, kept, zoomed } => {
                    let first = kept.first().copied();
                    self.tmux_relayout(gw, window, node, added, removed, kept, event_loop);
                    // tmux's zoom shows as Maximize Active Pane.
                    if let (Some(z), Some(id)) = (zoomed, first.and_then(|p| self.tmux.get(&gw).and_then(|t| t.panes.get(&p)).map(|(id, _)| *id))) {
                        self.with_tab_of(id, |t| t.zoomed = if z { Some(t.focused) } else { None });
                    }
                }
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
                    // tmux's own change: no select-pane goes back.
                    if let Some(t) = self.tmux.get_mut(&gw) {
                        t.selected = Some(pane);
                    }
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
        if changes {
            self.relayout();
            self.tmux_save_affinities(gw);
        }
        true
    }

    /// Saves which tmux windows share each Tandem window (iTerm2's saveAffinities).
    pub(super) fn tmux_save_affinities(&mut self, gw: PaneId) {
        let mut groups = Vec::new();
        let mut add = |app: &App, key: u64, tabs: &[Tab]| {
            let windows: Vec<u32> = tabs.iter().filter_map(|t| app.tmux_of(t.focused)).filter(|(g, _, _)| *g == gw).map(|(_, _, w)| w).collect();
            groups.push(crate::tmux_persist::Group { windows, terminal: format!("pty-{key}"), full_screen: false });
        };
        if let Some(k) = self.windows.current() {
            add(self, k, &self.tabs);
        }
        for (k, slot) in self.windows.parked() {
            add(self, *k, &slot.tabs);
        }
        if let Some(t) = self.tmux.get_mut(&gw) {
            t.ctl.save_affinities(&groups, &mut t.gw);
        }
        self.tmux_flush(gw);
    }

    /// Close Tab on a tmux tab: iTerm2's question (a window question when it is the
    /// session's only tab here). True when the question took over the close.
    pub(super) fn tmux_close_tab(&mut self, index: usize) -> bool {
        let Some(focused) = self.tabs.get(index).map(|t| t.focused) else { return false };
        let Some((gw, _, window)) = self.tmux_of(focused) else { return false };
        let same = self.tabs.iter().filter(|t| self.tmux_of(t.focused).is_some_and(|(g, _, _)| g == gw)).count();
        if same > 1 {
            if self.globals.bool("ClosingTmuxTabKillsTmuxWindows") {
                self.tmux_close_answer(gw, &[window], "Hide", false);
                return true;
            }
            let (text, buttons) = crate::tmux_persist::close_tab_question();
            self.ask(focused, &format!("{CLOSE_TAB}{gw}:{window}"), text, &buttons, true);
            return true;
        }
        self.tmux_close_window_question(focused, gw)
    }

    /// Closing a window that shows tmux tabs of the session `gw` (or of any session when
    /// `gw` is None): iTerm2's four-way question.
    fn tmux_close_window_question(&mut self, anchor: PaneId, gw: PaneId) -> bool {
        let n = self.tabs.iter().filter(|t| self.tmux_of(t.focused).is_some_and(|(g, _, _)| g == gw)).count();
        let Some((text, buttons)) = crate::tmux_persist::close_window_question(n) else { return false };
        if self.globals.bool("ClosingTmuxWindowKillsTmuxWindows") {
            let windows = self.tmux_windows_here(gw);
            self.tmux_close_answer(gw, &windows, "Hide", true);
            return true;
        }
        self.ask(anchor, &format!("{CLOSE_WIN}{gw}"), text, &buttons, true);
        true
    }

    /// Close Window with tmux tabs: the question first. True when it took over the close.
    pub(super) fn tmux_close_window(&mut self) -> bool {
        let Some((anchor, gw)) = self.tabs.iter().find_map(|t| self.tmux_of(t.focused).map(|(g, _, _)| (t.focused, g))) else { return false };
        self.tmux_close_window_question(anchor, gw)
    }

    fn tmux_windows_here(&self, gw: PaneId) -> Vec<u32> {
        self.tabs.iter().filter_map(|t| self.tmux_of(t.focused)).filter(|(g, _, _)| *g == gw).map(|(_, _, w)| w).collect()
    }

    /// Hide, Kill, or Detach tmux Session for `windows`; `whole` closes this window's tabs
    /// right away (iTerm2 closes the window without waiting for tmux).
    fn tmux_close_answer(&mut self, gw: PaneId, windows: &[u32], answer: &str, whole: bool) {
        let mut actions = Vec::new();
        match answer {
            "Hide" => {
                for w in windows {
                    if let Some(t) = self.tmux.get_mut(&gw) {
                        actions.extend(t.ctl.hide(*w, &mut t.gw));
                    }
                }
            }
            "Kill" => {
                for w in windows {
                    if let Some(t) = self.tmux.get_mut(&gw) {
                        t.ctl.kill_window(*w, &mut t.gw);
                    }
                }
            }
            "Detach tmux Session" => self.tmux_gateway_input(gw, b"\x1b", true),
            _ => return,
        }
        self.tmux_flush(gw);
        self.tmux_pending_close = Some((gw, actions, whole));
    }

    /// Runs the close a question's answer left (needs the event loop).
    pub(super) fn tmux_finish_close(&mut self, event_loop: &ActiveEventLoop) {
        let Some((gw, actions, whole)) = self.tmux_pending_close.take() else { return };
        self.tmux_actions(gw, actions, event_loop);
        if whole {
            let panes: Vec<PaneId> = self.tabs.iter().flat_map(|t| t.layout.panes()).collect();
            for p in panes {
                self.close_pane(p, event_loop);
            }
        }
    }

    /// The close questions' answers. True when the bar was one of these.
    pub(super) fn tmux_close_bar(&mut self, id: &str, button: Option<&str>) -> bool {
        if let Some(rest) = id.strip_prefix(CLOSE_TAB) {
            let mut it = rest.split(':').map(|s| s.parse::<u64>().ok());
            if let (Some(Some(gw)), Some(Some(w))) = (it.next(), it.next()) {
                if let Some(b) = button {
                    self.tmux_close_answer(gw, &[w as u32], b, false);
                }
            }
            return true;
        }
        if let Some(rest) = id.strip_prefix(CLOSE_WIN) {
            if let (Ok(gw), Some(b)) = (rest.parse::<u64>(), button) {
                let windows = self.tmux_windows_here(gw);
                self.tmux_close_answer(gw, &windows, b, true);
            }
            return true;
        }
        false
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
            // The sibling's window is still waiting to open (every window of one attach
            // arrives in one read): the tab joins it there.
            Some(anchor) if self.pending_windows.iter().any(|w| matches!(w, windows_ui::NewWindow::Tabs(ts) if ts.iter().any(|t| t.layout.panes().contains(&anchor)))) => {
                for w in self.pending_windows.iter_mut() {
                    if let windows_ui::NewWindow::Tabs(ts) = w {
                        if ts.iter().any(|t| t.layout.panes().contains(&anchor)) {
                            ts.push(tab);
                            break;
                        }
                    }
                }
            }
            _ => self.pending_windows.push(windows_ui::NewWindow::Tabs(vec![tab])),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn tmux_relayout(&mut self, gw: PaneId, window: u32, node: Node, added: Vec<PaneOpen>, removed: Vec<u32>, kept: Vec<u32>, event_loop: &ActiveEventLoop) {
        let tandem = |app: &App, p: u32| app.tmux.get(&gw).and_then(|t| t.panes.get(&p)).map(|(id, _)| *id).filter(|id| app.panes.contains_key(id));
        let gone: Vec<PaneId> = removed.iter().filter_map(|p| tandem(self, *p)).collect();
        // The tab is found through a pane it already shows, first in layout order.
        let kept_ids: Vec<PaneId> = kept.iter().filter_map(|p| tandem(self, *p)).collect();
        for p in &added {
            self.tmux_pane(gw, window, p);
        }
        let ids: HashMap<u32, PaneId> = tmux_layout::panes(&node).into_iter().filter_map(|p| tandem(self, p).map(|id| (p, id))).collect();
        // The kept panes take tmux's new sizes.
        for (p, w, h) in tmux_layout::sizes(&node) {
            if let Some(pane) = ids.get(&p).and_then(|id| self.panes.get_mut(id)) {
                pane.set_tmux_size(w.clamp(1, u16::MAX as u32) as u16, h.clamp(1, u16::MAX as u32) as u16);
            }
        }
        if let Some(layout) = tmux_layout::to_layout(&node, &ids) {
            match kept_ids.first().copied() {
                Some(anchor) => self.with_tab_of(anchor, |t| {
                    if !layout.panes().contains(&t.focused) {
                        t.focused = anchor;
                    }
                    t.layout = layout;
                }),
                // Tandem shows none of this window's panes any more: it opens as a new tab.
                None => {
                    let focused = layout.panes()[0];
                    let tab = Tab { id: self.next_tab_id, layout, focused, zoomed: None, title: None };
                    self.next_tab_id += 1;
                    self.pending_windows.push(windows_ui::NewWindow::Tabs(vec![tab]));
                }
            }
        }
        self.tmux_forget(gw, &gone);
        for id in gone {
            self.close_pane(id, event_loop);
        }
    }

    /// After a layout of the current window: each tmux tab's size in cells (its whole area,
    /// a divider counting as a cell as in tmux) goes to its session.
    pub(super) fn tmux_report_sizes(&mut self) {
        if self.tmux.is_empty() {
            return;
        }
        let (cols, rows) = self.grid_size(self.content_area());
        let mut changed: Vec<PaneId> = Vec::new();
        for t in &self.tabs {
            let Some(io) = self.panes.get(&t.focused).and_then(|p| p.tmux_io()) else { continue };
            let (gw, pane) = (io.gateway, io.pane);
            let Some(g) = self.tmux.get_mut(&gw) else { continue };
            let Some(window) = g.panes.get(&pane).map(|(_, w)| *w) else { continue };
            if g.tab_sizes.insert(window, (cols as u32, rows as u32)) != Some((cols as u32, rows as u32)) && !changed.contains(&gw) {
                changed.push(gw);
            }
        }
        for gw in changed {
            let Some(g) = self.tmux.get_mut(&gw) else { continue };
            let sizes: Vec<(u32, (u32, u32))> = g.tab_sizes.iter().filter(|(w, _)| g.ctl.windows.contains_key(w)).map(|(w, s)| (*w, *s)).collect();
            g.ctl.tabs_resized(&sizes, &mut g.gw);
            self.tmux_flush(gw);
        }
    }

    /// A tmux pane's gateway, tmux pane id, and tmux window.
    pub(super) fn tmux_of(&self, pane: PaneId) -> Option<(PaneId, u32, u32)> {
        let io = self.panes.get(&pane)?.tmux_io()?;
        let window = self.tmux.get(&io.gateway)?.panes.get(&io.pane)?.1;
        Some((io.gateway, io.pane, window))
    }

    /// The profile's Initial Directory for tmux (Home, Reuse, or Custom).
    fn tmux_dir(&self, pane: PaneId) -> crate::tmux_edits::Dir {
        use crate::tmux_edits::Dir;
        let Some(p) = self.profile_of(pane) else { return Dir::Home };
        match p.string("Custom Directory").as_str() {
            "Yes" => Dir::Custom(p.string("Working Directory")),
            "Recycle" => Dir::Recycle,
            _ => Dir::Home,
        }
    }

    /// Split Pane in a tmux tab: tmux splits, and its layout change shows the new pane.
    pub(super) fn tmux_split(&mut self, pane: PaneId, side_by_side: bool) -> bool {
        let Some((gw, p, _)) = self.tmux_of(pane) else { return false };
        let dir = self.tmux_dir(pane);
        let Some(t) = self.tmux.get_mut(&gw) else { return false };
        let recycling = t.hs.at_least(1.9);
        t.gw.send(&crate::tmux_edits::split_window(p, side_by_side, &dir, recycling), None, Default::default());
        self.tmux_flush(gw);
        true
    }

    /// Maximize Active Pane in a tmux tab: tmux zooms.
    pub(super) fn tmux_zoom(&mut self, pane: PaneId) -> bool {
        let Some((gw, p, _)) = self.tmux_of(pane) else { return false };
        let Some(t) = self.tmux.get_mut(&gw) else { return false };
        t.ctl.zoom(p, &mut t.gw);
        self.tmux_flush(gw);
        true
    }

    pub(super) fn tmux_swap(&mut self, a: PaneId, b: PaneId) -> bool {
        let (Some((gw, pa, _)), Some((gb, pb, _))) = (self.tmux_of(a), self.tmux_of(b)) else { return false };
        if gw != gb {
            return true;
        }
        let Some(t) = self.tmux.get_mut(&gw) else { return false };
        t.ctl.swap(pa, pb, &mut t.gw);
        self.tmux_flush(gw);
        true
    }

    /// Edit Tab Title on a tmux tab renames the tmux window; %window-renamed brings it back.
    pub(super) fn tmux_rename(&mut self, pane: PaneId, name: &str) -> bool {
        let Some((gw, _, window)) = self.tmux_of(pane) else { return false };
        let Some(t) = self.tmux.get_mut(&gw) else { return false };
        t.gw.send(&crate::tmux_edits::rename_window(window, None, name), None, Default::default());
        self.tmux_flush(gw);
        true
    }

    /// Close Session on a tmux pane: tmux kills it and its layout change removes it.
    pub(super) fn tmux_kill_pane(&mut self, pane: PaneId) -> bool {
        let Some((gw, p, _)) = self.tmux_of(pane) else { return false };
        let Some(t) = self.tmux.get_mut(&gw) else { return false };
        let flags = crate::tmux_gateway::Flags { tolerate: true, laggy: true, ..Default::default() };
        t.gw.send(&crate::tmux_edits::kill_pane(p), None, flags);
        self.tmux_flush(gw);
        true
    }

    /// Tabs moved in this window: each session's windows take the tabs' order.
    pub(super) fn tmux_reorder(&mut self) {
        let mut orders: HashMap<PaneId, Vec<u32>> = HashMap::new();
        for t in &self.tabs {
            if let Some((gw, _, window)) = self.tmux_of(t.focused) {
                orders.entry(gw).or_default().push(window);
            }
        }
        for (gw, wanted) in orders {
            if wanted.len() < 2 {
                continue;
            }
            if let Some(t) = self.tmux.get_mut(&gw) {
                t.ctl.reorder(wanted, &mut t.gw);
            }
            self.tmux_flush(gw);
            self.tmux_save_affinities(gw);
        }
    }

    /// New Tab or New Window while a tmux pane is current: None when it is not one (or the
    /// person chose the default profile); Some(()) when tmux makes it or it was cancelled.
    pub(super) fn tmux_new(&mut self, window: bool, ask: bool) -> Option<()> {
        let pane = self.focused_id()?;
        let (gw, _, tmux_window) = self.tmux_of(pane)?;
        if ask {
            let legacy = self.globals.get("NoSyncNewWindowOrTabFromTmuxOpensTmux").and_then(|v| v.as_bool()).is_some();
            let key = crate::tmux_edits::new_kind_key(window, legacy);
            let answer = match self.globals.get(key).and_then(|v| v.as_bool()) {
                Some(a) => Some(a),
                None => {
                    let (heading, text, b) = crate::tmux_edits::new_kind_question(window);
                    super::broadcast_ui::yes_no_cancel(self.hwnd(), heading, &format!("{text}\n\nYes: {}. No: {}.", b[0], b[1]))
                }
            };
            match answer {
                None => return Some(()),
                Some(false) => return None,
                Some(true) => {}
            }
        }
        let dir = self.tmux_dir(pane);
        let size = self.profile_of(pane).map(|p| (p.int("Columns").max(1) as u32, p.int("Rows").max(1) as u32));
        let affinity = (!window).then_some(tmux_window);
        let t = self.tmux.get_mut(&gw)?;
        t.ctl.new_window(affinity, &dir, size, &mut t.gw);
        self.tmux_flush(gw);
        Some(())
    }

    /// A divider drag in a tmux tab starts: tmux's lines wait until it ends.
    pub(super) fn tmux_drag_start(&mut self, path: &[bool], axis: Axis) {
        let Some(tab) = self.tabs.get(self.active) else { return };
        // The first pane right of or below the divider.
        let mut node = tab.layout.tree();
        for step in path {
            node = match node {
                crate::layout::Tree::Split { a, b, .. } => if *step { *b } else { *a },
                leaf => leaf,
            };
        }
        let after = match node {
            crate::layout::Tree::Split { b, .. } => b.leaves().first().copied().copied(),
            _ => None,
        };
        let Some((gw, p, _)) = after.and_then(|id| self.tmux_of(id)) else { return };
        let pos = if axis == Axis::Row { self.cursor.0 } else { self.cursor.1 };
        if let Some(t) = self.tmux.get_mut(&gw) {
            t.gw.pause();
        }
        self.tmux_drag = Some((gw, p, axis == Axis::Row, pos));
    }

    /// The drag ended: one resize-pane for how far it moved, then tmux's held lines.
    pub(super) fn tmux_drag_end(&mut self, event_loop: &ActiveEventLoop) {
        let Some((gw, p, horizontal, start)) = self.tmux_drag.take() else { return };
        let (cw, ch) = self.cell();
        let pos = if horizontal { self.cursor.0 } else { self.cursor.1 };
        let cells = ((pos - start) / if horizontal { cw } else { ch }).round() as i32;
        let Some(t) = self.tmux.get_mut(&gw) else { return };
        t.ctl.resize_pane(p, cells, horizontal, &mut t.gw);
        let events = t.gw.resume();
        self.tmux_flush(gw);
        self.tmux_events(gw, events, event_loop);
    }

    /// A tmux pane closed in Tandem: the controller forgets it (tmux keeps it until 8c sends
    /// kill-pane), so the next layout that names it opens it again.
    pub(super) fn tmux_pane_closed(&mut self, id: PaneId) {
        let Some((gw, pane)) = self.panes.get(&id).and_then(|p| p.tmux_io()).map(|io| (io.gateway, io.pane)) else { return };
        if let Some(t) = self.tmux.get_mut(&gw) {
            t.panes.retain(|_, (x, _)| *x != id);
            t.ctl.forget_pane(pane);
        }
    }

    /// The profile tmux panes use: the "tmux" profile with TmuxUsesDedicatedProfile (made
    /// the first time), else the gateway session's.
    pub(super) fn tmux_profile_guid(&mut self, gw: PaneId) -> String {
        if self.globals.bool("TmuxUsesDedicatedProfile") {
            let (guid, made) = self.store.tmux_profile();
            if made {
                if let Err(e) = self.store.save() {
                    self.banners.set("profiles", format!("Could not save profiles: {e}"));
                }
            }
            return guid;
        }
        self.panes.get(&gw).map(|p| p.profile.clone()).unwrap_or_default()
    }

    /// Typing waiting in tmux panes goes to tmux as send-keys, one command list per pane
    /// (errors tolerated, as iTerm2).
    pub(super) fn tmux_send_input(&mut self) {
        let ready: Vec<(PaneId, PaneId, u32, Vec<u8>)> = self
            .panes
            .iter()
            .filter_map(|(id, p)| {
                let io = p.tmux_io()?;
                let bytes = io.take_input();
                (!bytes.is_empty()).then_some((*id, io.gateway, io.pane, bytes))
            })
            .collect();
        for (id, gw, pane, bytes) in ready {
            let Some(t) = self.tmux.get_mut(&gw) else { continue };
            match crate::tmux_keys::encode(&bytes, pane, t.hs.min, t.hs.max) {
                Ok(cmds) => {
                    let tolerate = crate::tmux_gateway::Flags { tolerate: true, ..Default::default() };
                    if !cmds.is_empty() {
                        t.gw.send_list(cmds.into_iter().map(|c| (c, None, tolerate)).collect(), false);
                    }
                }
                Err(c) => {
                    self.ask(id, PLANE_BAR, &format!("Because of a bug in tmux 2.2, the character \u{201c}{c}\u{201d} cannot be sent."), &["Why?"], false);
                }
            }
            self.tmux_flush(gw);
        }
    }

    /// tmux 3.2 and later under modifyOtherKeys: a modified key goes by name, so tmux
    /// encodes it for the pane (iTerm2's tmuxControlModeKeyNameForEvent). True when sent.
    pub(super) fn tmux_key_by_name(&mut self, pane: PaneId, key: &crate::keys::Key) -> bool {
        use crate::keys::Key;
        let Some(io) = self.panes.get(&pane).and_then(|p| p.tmux_io()) else { return false };
        let (gw, tpane) = (io.gateway, io.pane);
        let Some(t) = self.tmux.get(&gw) else { return false };
        if !t.hs.at_least(3.2) {
            return false;
        }
        // A broadcast keystroke keeps the byte path, which broadcasts.
        if self.broadcast.targets(&self.broadcast_view(), pane).len() > 1 {
            return false;
        }
        let kb = self.panes.get(&pane).map(|p| p.own().keyboard()).unwrap_or_default();
        if kb.kitty != 0 || !matches!(kb.mode, crate::keyproto::KeyMode::Mok1 | crate::keyproto::KeyMode::Mok2) {
            return false;
        }
        let c = match key {
            Key::Text(s) => match s.chars().next() {
                Some(c) if s.chars().count() == 1 => c,
                _ => return false,
            },
            Key::Enter => '\r',
            Key::Tab => '\t',
            Key::Escape => '\u{1b}',
            Key::Backspace => '\u{7f}',
            _ => return false,
        };
        let meta = self.profile_of(pane).is_some_and(|p| p.int("Option Key Sends") != 0);
        let Some(name) = crate::tmux_keys::key_name(c, self.mods.ctrl, self.mods.alt, self.mods.shift, meta, false) else { return false };
        let Some(t) = self.tmux.get_mut(&gw) else { return false };
        let tolerate = crate::tmux_gateway::Flags { tolerate: true, ..Default::default() };
        t.gw.send_list(vec![(crate::tmux_keys::key_name_command(tpane, &name), None, tolerate)], false);
        self.tmux_flush(gw);
        true
    }

    /// A tmux pane that takes focus (the window focused, the pane current) tells tmux with
    /// select-pane; losing window focus lets the next gain tell it again.
    pub(super) fn tmux_focus_tick(&mut self) {
        let focused = if self.focused { self.focused_id() } else { None };
        let now = focused.and_then(|id| self.panes.get(&id)).and_then(|p| p.tmux_io()).map(|io| (io.gateway, io.pane));
        for (gw, t) in self.tmux.iter_mut() {
            let mine = now.filter(|(g, _)| g == gw).map(|(_, p)| p);
            if mine.is_none() {
                t.selected = None;
                continue;
            }
            if t.selected != mine {
                t.selected = mine;
                if let Some(cmd) = mine.and_then(|p| crate::tmux_windows::select_pane_command(p, t.hs.min)) {
                    t.gw.send(&cmd, None, Default::default());
                }
            }
        }
        let gws: Vec<PaneId> = self.tmux.keys().copied().collect();
        for gw in gws {
            self.tmux_flush(gw);
        }
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
