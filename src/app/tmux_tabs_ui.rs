//! tmux windows as Tandem tabs: what the controller's actions do to panes, tabs, and
//! windows (iTerm2's TmuxWindowOpener and PTYSession's tmux window handlers).

use super::*;
use crate::tmux_controller::{Action, PaneOpen};
use crate::tmux_layout::{self, Node};
use crate::tmux_windows::{Place, Window};

pub(super) const PLANE_BAR: &str = "tmux-22-plane";
/// The tab close question: "tmux-close-tab:GW:WINDOW" (window questions: tmux_persist).
const CLOSE_TAB: &str = "tmux-close-tab:";
/// Pause bars: "tmux-paused:GW:PANE:1" (1: tmux paused it) and "tmux-pause-warn:GW:PANE".
const PAUSED: &str = "tmux-paused:";
const PAUSE_WARN: &str = "tmux-pause-warn:";
/// The paste buffer offer (iTerm2's NoSyncOfferToSyncTmuxClipboard) and the key that says
/// it was answered (iTerm2 checks whether TmuxSyncClipboard exists at all).
const PASTE_OFFER: &str = "NoSyncOfferToSyncTmuxClipboard";
const PASTE_ANSWERED: &str = "NoSyncTmuxSyncClipboardAnswered";
/// "Fix corrupted tab colors?" (iTerm2's permanently silenceable NoSyncResetRedTmuxTabs).
const RED_TABS: &str = "NoSyncResetRedTmuxTabs";
/// "tmux-double:GW:OTHER": this gateway attached to a session OTHER already shows.
const DOUBLE: &str = "tmux-double:";

impl App {
    /// Applies the controller's actions for the gateway `gw`. False when tmux mode ended.
    pub(super) fn tmux_actions(&mut self, gw: PaneId, actions: Vec<Action>, event_loop: &ActiveEventLoop) -> bool {
        // Output and status lines change no pane or tab: no relayout for them.
        let changes = actions.iter().any(|a| !matches!(a, Action::Output(..) | Action::StatusLine { .. } | Action::NewWindowReply { .. } | Action::Abandoned(..)));
        for a in actions {
            match a {
                Action::Open { window, node, panes, place } => self.tmux_open(gw, window, node, panes, place),
                Action::NewWindowReply { tag, window } => self.api_tmux_new_window_reply(gw, tag, window),
                // %paste-buffer-changed: mirrored with TmuxSyncClipboard, else offered once.
                Action::PasteBuffer(name) => {
                    if self.globals.bool("TmuxSyncClipboard") {
                        if let Some(t) = self.tmux.get_mut(&gw) {
                            t.ctl.fetch_buffer(&name, &mut t.gw);
                        }
                        self.tmux_flush(gw);
                    } else if crate::tmux_clients::offer_paste_sync(false, self.globals.bool(PASTE_ANSWERED), false) {
                        // In the current window's current session, tmux or not (iTerm2's).
                        if let Some(id) = self.focused_id() {
                            self.ask(id, PASTE_OFFER, "The tmux paste buffer was updated. Would you like to mirror it to the local clipboard from now on?", &["Always", "Never"], false);
                        }
                    }
                }
                Action::RedTabColors => {
                    if !self.globals.bool(RED_TABS) {
                        self.ask(gw, &format!("{RED_TABS}:{gw}"), "Fix corrupted tab colors? While attaching to tmux, some tabs were found whose color is pure red. This may be due to a bug in early versions of iTerm2 3.6.x. Would you like to reset those tabs' colors?", &["Reset", "Keep Red"], true);
                    }
                }
                Action::Clipboard(text) => {
                    if let Some(cb) = &mut self.clipboard {
                        if let Err(e) = cb.set_text(text) {
                            self.banners.set("clipboard", format!("Copy failed: {e}"));
                        }
                    }
                }
                Action::PaneInfo { pane, what, value } => {
                    let Some(id) = self.tmux.get(&gw).and_then(|t| t.panes.get(&pane)).map(|(id, _)| *id) else { continue };
                    if let Some(io) = self.panes.get(&id).and_then(|p| p.tmux_io()) {
                        let mut info = io.info.borrow_mut();
                        match what {
                            crate::tmux_controller::Info::Title => info.title = value,
                            crate::tmux_controller::Info::Job => info.job = value,
                            crate::tmux_controller::Info::Cwd => info.cwd = value,
                        }
                    }
                    self.redraw();
                }
                // tmux's set-titles string names the tab (iTerm2's tmuxWindowTitle override).
                Action::WindowTitle(window, value) => {
                    if let Some(t) = self.tmux.get_mut(&gw) {
                        t.window_titles.insert(window, value.clone());
                    }
                    let title = self.tmux_title(&value);
                    if let Some(first) = self.tmux_window_panes(gw, window).first().copied() {
                        // The tab may still wait to become a window.
                        let mut pending = self.pending_windows.iter_mut().flat_map(|w| match w {
                            windows_ui::NewWindow::Tab(t) => std::slice::from_mut(t),
                            windows_ui::NewWindow::Tabs(ts) => ts.as_mut_slice(),
                            _ => &mut [],
                        });
                        match pending.find(|t| t.layout.panes().contains(&first)) {
                            Some(t) => t.title = Some(title),
                            None => self.with_tab_of(first, |t| t.title = Some(title)),
                        }
                    }
                }
                // AutoHideTmuxClientSession: once the attach's windows have opened (they may
                // still wait to become windows), the gateway is buried.
                Action::InitialWindowsOpened => {
                    if self.globals.bool("AutoHideTmuxClientSession") {
                        if let Some(t) = self.tmux.get_mut(&gw) {
                            t.bury_pending = !t.auto_buried;
                        }
                    }
                }
                Action::Relayout { window, node, added, removed, kept, zoomed } => {
                    let first = kept.first().copied();
                    self.tmux_relayout(gw, window, node, added, removed, kept, event_loop);
                    // tmux's zoom shows as Maximize Active Pane.
                    if let (Some(z), Some(id)) = (zoomed, first.and_then(|p| self.tmux.get(&gw).and_then(|t| t.panes.get(&p)).map(|(id, _)| *id))) {
                        self.with_tab_of(id, |t| t.zoomed = if z { Some(t.focused) } else { None });
                    }
                }
                // Closed while still opening: a waiting CreateWindow gets no tab ("0").
                Action::Abandoned(window) => self.api_tmux_window_opened(gw, window, None),
                Action::Close(window) => {
                    // Closed before it opened: a waiting CreateWindow gets no tab ("0").
                    self.api_tmux_window_opened(gw, window, None);
                    if let Some(t) = self.tmux.get_mut(&gw) {
                        t.window_titles.remove(&window);
                        t.tab_overrides.remove(&window);
                    }
                    let ids = self.tmux_window_panes(gw, window);
                    // Its tab may still wait to become a window (opened in this same batch).
                    self.api_tmux_tabs_closed(gw, &ids);
                    self.tmux_forget(gw, &ids);
                    for id in ids {
                        self.close_pane(id, event_loop);
                    }
                }
                Action::Rename(window, name) => {
                    // A set-titles string names the tab instead.
                    if self.tmux.get(&gw).is_some_and(|t| t.window_titles.contains_key(&window)) {
                        continue;
                    }
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
                    let Some(p) = self.panes.get_mut(&id) else { continue };
                    let (title, vars) = (p.own().title().map(str::to_string), p.own().shell.vars.clone());
                    let osc4 = self.tmux.get(&gw).is_some_and(|t| t.ctl.answers_osc4());
                    let Some(p) = self.panes.get_mut(&id) else { continue };
                    if let Some(io) = p.tmux_io() {
                        io.answers_osc4.set(osc4);
                        io.push_output(&data);
                    }
                    // Read now: nothing else wakes the loop for it.
                    p.pump();
                    self.api_output(&[id]);
                    let Some(p) = self.panes.get_mut(&id) else { continue };
                    let title_changed = p.own().title().map(str::to_string) != title;
                    let now_vars = (p.own().shell.vars != vars).then(|| p.own().shell.vars.iter().map(|(k, v)| (format!("user.{k}"), v.clone())).collect());
                    let Some(t) = self.tmux.get_mut(&gw) else { continue };
                    // OSC 0, 1, or 2: the title and job are asked for again (before tmux 3.2).
                    if title_changed {
                        t.ctl.pane_event(pane, crate::tmux_controller::PaneEvent::Title, &mut t.gw);
                    }
                    // SetUserVar: the pane's variables go to @uservars.
                    if let Some(vars) = now_vars {
                        t.ctl.sync_user_vars(pane, &vars, &mut t.gw);
                    }
                    self.tmux_flush(gw);
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
                // iTerm2's didAttachWithHiddenWindows:tooManyWindows:.
                Action::Attached { too_many, hidden } => {
                    if too_many || (hidden && self.globals.bool("OpenTmuxDashboardIfHiddenWindows")) {
                        self.open_tmux_dashboard_on(event_loop, Some(gw));
                    }
                }
                Action::SessionGuid(guid) => {
                    let other = self.tmux.iter().find(|(g, t)| **g != gw && t.ctl.session_guid.as_deref() == Some(guid.as_str())).map(|(g, _)| *g);
                    if let Some(other) = other {
                        let (heading, text, buttons) = crate::tmux_persist::attach_warning(true);
                        self.ask(gw, &format!("{DOUBLE}{gw}:{other}"), &format!("{heading}: {text}"), &buttons, true);
                    }
                }
                Action::Paused { pane, by_tmux } => {
                    let Some(id) = self.tmux.get(&gw).and_then(|t| t.panes.get(&pane)).map(|(id, _)| *id) else { continue };
                    self.drop_bar(id, &format!("{PAUSE_WARN}{gw}:{pane}"));
                    self.ask(id, &format!("{PAUSED}{gw}:{pane}:{}", u8::from(by_tmux)), crate::tmux_pause::paused_text(by_tmux), &["Unpause", "Settings"], true);
                }
                Action::Unpaused(panes) => {
                    for p in panes {
                        let Some(id) = self.tmux.get(&gw).and_then(|t| t.panes.get(&p.pane)).map(|(id, _)| *id) else { continue };
                        self.drop_bar(id, &format!("{PAUSED}{gw}:{}:1", p.pane));
                        self.drop_bar(id, &format!("{PAUSED}{gw}:{}:0", p.pane));
                        // iTerm2's unpauseTmux dismisses the warning too (one announcement id).
                        self.drop_bar(id, &format!("{PAUSE_WARN}{gw}:{}", p.pane));
                        if let Some(pane) = self.panes.get_mut(&id) {
                            // iTerm2's resetForTmuxUnpause and clearScrollbackBuffer, then the captures.
                            pane.own_mut().feed_tmux_restore(b"\x1bc\x1b[3J");
                            pane.own_mut().feed_tmux_restore(&p.restore);
                            // The screen filter starts over and converts the pending bytes too.
                            let pending = match pane.screen_filter.as_mut() {
                                Some(f) => {
                                    f.reset();
                                    f.feed(&p.pending)
                                }
                                None => p.pending.clone(),
                            };
                            pane.feed_output(&pending);
                        }
                    }
                }
                Action::PauseWarning(pane, bar) => {
                    let Some(id) = self.tmux.get(&gw).and_then(|t| t.panes.get(&pane)).map(|(id, _)| *id) else { continue };
                    let bid = format!("{PAUSE_WARN}{gw}:{pane}");
                    match bar {
                        crate::tmux_pause::Bar::Show(text) => {
                            self.ask(id, &bid, &text, &["Pause Settings"], true);
                        }
                        crate::tmux_pause::Bar::Dismiss => self.drop_bar(id, &bid),
                    }
                }
                // The tmux status bar's text: the bar shows it (iTerm2's tmuxStatusLeft and
                // tmuxStatusRight session variables).
                Action::StatusLine { window, side, value } => {
                    let line = self.tmux_status.entry((gw, window)).or_default();
                    let slot = match side {
                        crate::tmux_controller::Side::Left => &mut line.0,
                        crate::tmux_controller::Side::Right => &mut line.1,
                    };
                    if *slot != value {
                        *slot = value;
                        self.redraw_all();
                    }
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
            self.tmux_watch_status(gw);
        }
        true
    }

    /// The tmux status bar (`PTYSession.m:10077-10091`, `PTYSession.m:6272`): each tmux
    /// window with a pane whose profile has "Status bar enabled" has its status lines
    /// watched, and no other. A window that closed, a pane that moved to another window,
    /// or a profile turned off stops them; its text goes.
    pub(super) fn tmux_watch_status(&mut self, gw: PaneId) {
        let Some(t) = self.tmux.get(&gw) else { return };
        let want: std::collections::BTreeSet<u32> = t.panes.values().filter(|(id, _)| self.shows_status_bar(*id)).map(|(_, w)| *w).collect();
        let have = t.ctl.status_windows();
        let Some(t) = self.tmux.get_mut(&gw) else { return };
        for w in have.difference(&want) {
            t.ctl.watch_status(*w, false, &mut t.gw);
        }
        for w in want.difference(&have) {
            t.ctl.watch_status(*w, true, &mut t.gw);
        }
        let watched = t.ctl.status_windows();
        self.tmux_status.retain(|(g, w), _| *g != gw || watched.contains(w));
        self.tmux_flush(gw);
    }

    /// `tmux_watch_status` for every gateway; the text of a gateway that ended goes.
    pub(super) fn tmux_watch_status_all(&mut self) {
        let gws: Vec<PaneId> = self.tmux.keys().copied().collect();
        for gw in gws {
            self.tmux_watch_status(gw);
        }
        let tmux = &self.tmux;
        self.tmux_status.retain(|(g, _), _| tmux.contains_key(g));
    }

    /// Saves which tmux windows share each Tandem window (iTerm2's saveAffinities), and
    /// the windows' state: size, origin, tab titles, and buried tabs.
    pub(super) fn tmux_save_affinities(&mut self, gw: PaneId) {
        let mut groups = Vec::new();
        let mut states = Vec::new();
        let mut geometry = Vec::new();
        let overrides = self.tmux.get(&gw).map(|t| t.tab_overrides.clone()).unwrap_or_default();
        let mut add = |app: &App, key: u64, tabs: &[Tab], gpu: Option<&Gpu>| {
            let windows: Vec<u32> = tabs.iter().filter_map(|t| app.tmux_of(t.focused)).filter(|(g, _, _)| *g == gw).map(|(_, _, w)| w).collect();
            // A minimized window keeps the place and size it had before.
            let (origin, size) = gpu.and_then(|g| window_geometry(&g.window)).or_else(|| app.tmux_last_geometry.get(&key).copied()).unwrap_or(((0, 0), None));
            geometry.push((key, (origin, size)));
            let tab_titles = windows.iter().filter_map(|w| Some((*w, overrides.get(w)?.clone()))).collect();
            states.push(crate::tmux_controller::GroupState { terminal: format!("pty-{key}"), windows: windows.clone(), origin, size, tab_titles });
            groups.push(crate::tmux_persist::Group { windows, terminal: format!("pty-{key}"), full_screen: false });
        };
        if let Some(k) = self.windows.current() {
            add(self, k, &self.tabs, self.gpu.as_ref());
        }
        for (k, slot) in self.windows.parked() {
            add(self, *k, &slot.tabs, slot.gpu.as_ref());
        }
        self.tmux_last_geometry.extend(geometry);
        if let Some(t) = self.tmux.get_mut(&gw) {
            t.ctl.save_affinities(&groups, &mut t.gw);
            t.ctl.save_window_state(&states, &mut t.gw);
        }
        self.tmux_flush(gw);
    }

    /// Every gateway's window state (a window moved, resized, or went full screen).
    pub(super) fn tmux_save_all_windows(&mut self) {
        for gw in self.tmux_gateways() {
            self.tmux_save_affinities(gw);
        }
    }

    /// Each tmux pane's tab color (a program's or the saved one, else the profile's) goes
    /// to `@tab_colors` (iTerm2 writes it whenever a profile is applied).
    pub(super) fn tmux_save_tab_colors(&mut self) {
        let dark = self.dark();
        let mut by_gw: HashMap<PaneId, Vec<(u32, Option<(u8, u8, u8)>)>> = HashMap::new();
        for (id, p) in &self.panes {
            let Some(io) = p.tmux_io() else { continue };
            let c = self.tab_colors.get(id).copied().or_else(|| self.store.get(&p.profile).and_then(|pr| pr.tab_color(dark)));
            by_gw.entry(io.gateway).or_default().push((io.pane, c.map(|c| (c.r, c.g, c.b))));
        }
        for (gw, colors) in by_gw {
            if let Some(t) = self.tmux.get_mut(&gw) {
                t.ctl.save_tab_colors(&colors, &mut t.gw);
            }
            self.tmux_flush(gw);
        }
    }

    /// Move Session to Window or to Tab on a tmux pane: tmux's break-pane moves it, and the
    /// new tmux window opens in a new Tandem window or beside its old one. True when it was
    /// a tmux pane (of a tab with other panes).
    pub(super) fn tmux_break(&mut self, pane: PaneId, to_tab: bool) -> bool {
        let Some((gw, p, window)) = self.tmux_of(pane) else { return false };
        if self.tabs.iter().find(|t| t.layout.panes().contains(&pane)).is_none_or(|t| t.layout.panes().len() < 2) {
            return false;
        }
        // A new window opens a little right of and below this one (iTerm2's +10, -10).
        let point = (!to_tab).then(|| self.gpu.as_ref().and_then(|g| window_geometry(&g.window)).map(|g| g.0)).flatten().map(|(x, y)| (x + 10, y + 10));
        if let Some(t) = self.tmux.get_mut(&gw) {
            t.ctl.break_pane(p, to_tab.then_some(window), point, &mut t.gw);
        }
        self.tmux_flush(gw);
        true
    }

    /// A tmux window that opened in a new Tandem window takes its saved size and place.
    pub(super) fn tmux_apply_geometry(&mut self) {
        let Some(tab) = self.tabs.iter().map(|t| t.id).find(|id| self.tmux_geometry.contains_key(id)) else { return };
        let Some((origin, size)) = self.tmux_geometry.remove(&tab) else { return };
        let Some(g) = self.gpu.as_ref() else { return };
        let w = &g.window;
        if let Some(crate::tmux_persist::WinSize::Size(x, y)) = size {
            // The saved size is the outer frame; the window takes an inner size.
            let scale = w.scale_factor();
            let (outer, inner) = (w.outer_size(), w.inner_size());
            let (dw, dh) = (outer.width.saturating_sub(inner.width) as f64 / scale, outer.height.saturating_sub(inner.height) as f64 / scale);
            let _ = w.request_inner_size(winit::dpi::LogicalSize::new((x as f64 - dw).max(100.0), (y as f64 - dh).max(100.0)));
        }
        if let Some(p) = origin {
            let monitors: Vec<(i32, i32, i32, i32)> = w.available_monitors().map(|m| (m.position().x, m.position().y, m.size().width as i32, m.size().height as i32)).collect();
            let outer = w.outer_size();
            let (x, y) = crate::tmux_persist::visible_origin(p, (outer.width as i32, outer.height as i32), &monitors);
            w.set_outer_position(winit::dpi::PhysicalPosition::new(x, y));
        }
        if size == Some(crate::tmux_persist::WinSize::FullScreen) {
            w.set_fullscreen(Some(winit::window::Fullscreen::Borderless(None)));
        }
    }

    /// Close Tab on a tmux tab: iTerm2's question (the window question when it is the
    /// session's only tab here; only the tab closes). True when the question took over.
    pub(super) fn tmux_close_tab(&mut self, index: usize) -> bool {
        let Some(focused) = self.tabs.get(index).map(|t| t.focused) else { return false };
        let Some((gw, _, window)) = self.tmux_of(focused) else { return false };
        let same = self.tabs.iter().filter(|t| self.tmux_of(t.focused).is_some_and(|(g, _, _)| g == gw)).count();
        if same > 1 {
            if self.globals.bool("ClosingTmuxTabKillsTmuxWindows") {
                self.tmux_close_answer(&[(gw, vec![window])], "Hide", false);
                return true;
            }
            let (text, buttons) = crate::tmux_persist::close_tab_question();
            self.ask(focused, &format!("{CLOSE_TAB}{gw}:{window}"), text, &buttons, true);
            return true;
        }
        self.tmux_close_window_question(focused, Some(gw), false)
    }

    /// iTerm2's four-way question for the tmux tabs of `gw` (every session when None) in
    /// this window; `whole`: the window closes after the answer.
    fn tmux_close_window_question(&mut self, anchor: PaneId, gw: Option<PaneId>, whole: bool) -> bool {
        let scope = self.tmux_scope(gw);
        let n: usize = scope.iter().map(|(_, ws)| ws.len()).sum();
        let Some((text, buttons)) = crate::tmux_persist::close_window_question(n) else { return false };
        if self.globals.bool("ClosingTmuxWindowKillsTmuxWindows") {
            self.tmux_close_answer(&scope, "Hide", whole);
            return true;
        }
        self.ask(anchor, &crate::tmux_persist::close_win_id(gw.unwrap_or(0), whole), text, &buttons, true);
        true
    }

    /// Close Window with tmux tabs, after Tandem's own confirmation: the question first.
    /// True when it took over the close.
    pub(super) fn tmux_close_window(&mut self) -> bool {
        let Some(anchor) = self.tabs.iter().find(|t| self.tmux_of(t.focused).is_some()).map(|t| t.focused) else { return false };
        self.tmux_close_window_question(anchor, None, true)
    }

    /// The tmux windows in this Tandem window, by session (`gw`, or every session).
    fn tmux_scope(&self, gw: Option<PaneId>) -> Vec<(PaneId, Vec<u32>)> {
        let mut out: Vec<(PaneId, Vec<u32>)> = Vec::new();
        for (g, _, w) in self.tabs.iter().filter_map(|t| self.tmux_of(t.focused)) {
            if gw.is_some_and(|x| x != g) {
                continue;
            }
            match out.iter_mut().find(|(x, _)| *x == g) {
                Some((_, ws)) => ws.push(w),
                None => out.push((g, vec![w])),
            }
        }
        out
    }

    /// Hide, Kill, or Detach tmux Session for each session's windows; `whole` closes this
    /// window afterwards (iTerm2 closes the window without waiting for tmux).
    fn tmux_close_answer(&mut self, scope: &[(PaneId, Vec<u32>)], answer: &str, whole: bool) {
        if !matches!(answer, "Hide" | "Kill" | "Detach tmux Session") {
            return;
        }
        for (i, (gw, windows)) in scope.iter().enumerate() {
            let mut actions = Vec::new();
            for w in windows {
                let Some(t) = self.tmux.get_mut(gw) else { break };
                match answer {
                    "Hide" => {
                        let (gwid, win) = (*gw, *w);
                        self.tmux_note_burial(gwid, win);
                        let Some(t) = self.tmux.get_mut(gw) else { break };
                        actions.extend(t.ctl.hide(win, &mut t.gw));
                    }
                    "Kill" => t.ctl.kill_window(*w, &mut t.gw),
                    _ => {}
                }
            }
            if answer == "Detach tmux Session" {
                self.tmux_gateway_input(*gw, b"\x1b", true);
            }
            self.tmux_flush(*gw);
            self.tmux_pending_close.push((*gw, actions, whole && i + 1 == scope.len()));
        }
    }

    /// Runs the closes the answers left (they need the event loop).
    pub(super) fn tmux_finish_close(&mut self, event_loop: &ActiveEventLoop) {
        for (gw, actions, whole) in std::mem::take(&mut self.tmux_pending_close) {
            self.tmux_actions(gw, actions, event_loop);
            if whole {
                let panes: Vec<PaneId> = self.tabs.iter().flat_map(|t| t.layout.panes()).collect();
                self.begin_undo(crate::undo_close::Kind::Window, &panes);
                for p in panes {
                    self.close_pane(p, event_loop);
                }
                self.end_undo();
            }
        }
    }

    /// A gateway ended: its "Cannot Attach" bars, here and on the other gateway, go.
    pub(super) fn tmux_drop_double_bars(&mut self, gw: PaneId) {
        let names = |id: &str| id.strip_prefix(DOUBLE).is_some_and(|r| r.split(':').any(|x| x == gw.to_string()));
        for bars in self.bars.values_mut() {
            bars.retain(|b| !names(&b.id));
        }
    }

    /// Shell > tmux > Detach (iTerm2's requestDetach): a second request asks to force it.
    pub(super) fn tmux_request_detach(&mut self) {
        let Some(gw) = self.focused_id().and_then(|id| if self.tmux.contains_key(&id) { Some(id) } else { self.tmux_of(id).map(|(g, _, _)| g) }) else { return };
        self.tmux_request_detach_of(gw);
    }

    pub(super) fn tmux_request_detach_of(&mut self, gw: PaneId) {
        let Some(t) = self.tmux.get_mut(&gw) else { return };
        if t.gw.request_detach() {
            return self.tmux_flush(gw);
        }
        if super::close_ui::confirm(self.hwnd(), "Force Detach?", "A previous detach request has not yet been honored. Force detach?") {
            if let Some(t) = self.tmux.get_mut(&gw) {
                t.gw.force_disconnect();
            }
            self.tmux_disconnected(gw, true);
        }
    }

    /// Shell > tmux > Force Detach: the gateway's X.
    pub(super) fn tmux_force_detach(&mut self) {
        let Some(gw) = self.focused_id().and_then(|id| if self.tmux.contains_key(&id) { Some(id) } else { self.tmux_of(id).map(|(g, _, _)| g) }) else { return };
        self.tmux_gateway_input(gw, b"X", false);
    }

    pub(super) fn drop_bar(&mut self, pane: PaneId, id: &str) {
        if let Some(b) = self.bars.get_mut(&pane) {
            b.retain(|x| x.id != id);
        }
    }

    /// Once a second per session in pause mode: the monitor, automatic unpauses, and the
    /// settings it follows. Returns when to look again.
    pub(super) fn tmux_pause_tick(&mut self, event_loop: &ActiveEventLoop) -> Option<Instant> {
        let now = Instant::now();
        let clock = self.started.elapsed().as_secs_f64();
        let age = self.globals.int("TmuxPauseModeAgeLimit").max(1) as f64;
        let auto = self.globals.bool("TmuxUnpauseAutomatically");
        let warn = self.globals.bool("TmuxWarnBeforePausing") && !self.globals.bool("noSyncDontWarnAboutTmuxPause");
        let mut next: Option<Instant> = None;
        let gws: Vec<PaneId> = self.tmux.keys().copied().collect();
        for gw in gws {
            let Some(t) = self.tmux.get_mut(&gw) else { continue };
            if !t.ctl.pause_mode() {
                continue;
            }
            t.ctl.prefs.unpause_automatically = auto;
            t.ctl.prefs.warn_before_pausing = warn;
            t.ctl.set_age_limit(age, &mut t.gw);
            let due = t.pause_tick.map_or(now, |p| p + std::time::Duration::from_secs(1));
            if now >= due {
                t.pause_tick = Some(now);
                let actions = t.ctl.tick(clock, &mut t.gw);
                self.tmux_flush(gw);
                self.tmux_actions(gw, actions, event_loop);
                next = [next, Some(now + std::time::Duration::from_secs(1))].into_iter().flatten().min();
            } else {
                next = [next, Some(due)].into_iter().flatten().min();
            }
            self.tmux_flush(gw);
        }
        next
    }

    /// Shell > tmux > Pause Pane: pauses the focused tmux pane, or unpauses it.
    pub(super) fn tmux_toggle_pause(&mut self) {
        let Some((gw, p, _)) = self.focused_id().and_then(|id| self.tmux_of(id)) else { return };
        let Some(t) = self.tmux.get_mut(&gw) else { return };
        if t.ctl.is_paused(p) {
            t.ctl.unpause(&[p], &mut t.gw);
        } else {
            t.ctl.pause_panes(&[p], &mut t.gw);
        }
        self.tmux_flush(gw);
    }

    /// Pause Pane's check: the focused tmux pane is paused.
    pub(super) fn tmux_pane_paused(&self) -> bool {
        self.focused_id().and_then(|id| self.tmux_of(id)).is_some_and(|(gw, p, _)| self.tmux.get(&gw).is_some_and(|t| t.ctl.is_paused(p)))
    }

    /// The pause bars' answers. True when the bar was one of these.
    pub(super) fn tmux_pause_bar(&mut self, id: &str, button: Option<&str>, event_loop: &ActiveEventLoop) -> bool {
        if let Some(rest) = id.strip_prefix(PAUSED) {
            let parts: Vec<&str> = rest.split(':').collect();
            let (Some(gw), Some(p)) = (parts.first().and_then(|s| s.parse::<u64>().ok()), parts.get(1).and_then(|s| s.parse::<u32>().ok())) else { return true };
            match button {
                Some("Unpause") => {
                    if let Some(t) = self.tmux.get_mut(&gw) {
                        t.ctl.unpause(&[p], &mut t.gw);
                    }
                    self.tmux_flush(gw);
                }
                Some("Settings") => {
                    self.open_settings_page(event_loop, crate::settings_ui::Page::General);
                    // iTerm2 shows the bar again: the pane is still paused.
                    if let Some(pane) = self.tmux.get(&gw).and_then(|t| t.panes.get(&p)).map(|(x, _)| *x) {
                        let by_tmux = parts.get(2) == Some(&"1");
                        self.ask(pane, id, crate::tmux_pause::paused_text(by_tmux), &["Unpause", "Settings"], true);
                    }
                }
                _ => {}
            }
            return true;
        }
        if let Some(gw) = id.strip_prefix(RED_TABS).and_then(|r| r.strip_prefix(':')).and_then(|g| g.parse::<PaneId>().ok()) {
            if button == Some("Reset") {
                if let Some(t) = self.tmux.get_mut(&gw) {
                    t.ctl.reset_red_tab_colors();
                }
                let red = crate::palette::rgb(255, 0, 0);
                let ids: Vec<PaneId> = self.tmux.get(&gw).map(|t| t.panes.values().map(|(id, _)| *id).collect()).unwrap_or_default();
                self.tab_colors.retain(|id, c| !(ids.contains(id) && *c == red));
                self.tmux_save_tab_colors();
            }
            if button.is_some() {
                self.globals.set(RED_TABS, toml::Value::Boolean(true));
                self.save_globals();
            }
            return true;
        }
        if id == PASTE_OFFER {
            // Always does not copy the buffer that asked (iTerm2's).
            if let Some(b) = button.filter(|b| *b == "Always" || *b == "Never") {
                self.globals.set("TmuxSyncClipboard", toml::Value::Boolean(b == "Always"));
                self.globals.set(PASTE_ANSWERED, toml::Value::Boolean(true));
                self.save_globals();
            }
            return true;
        }
        if id.starts_with(PAUSE_WARN) {
            if button == Some("Pause Settings") {
                self.open_settings_page(event_loop, crate::settings_ui::Page::General);
            }
            return true;
        }
        false
    }

    /// The close questions' answers. True when the bar was one of these.
    pub(super) fn tmux_close_bar(&mut self, id: &str, button: Option<&str>) -> bool {
        if let Some(rest) = id.strip_prefix(DOUBLE) {
            let mut it = rest.split(':').map(|s| s.parse::<u64>().ok());
            if let (Some(Some(gw)), Some(Some(other))) = (it.next(), it.next()) {
                // Either connection may have ended since the bar came up.
                if !self.tmux.contains_key(&gw) || !self.tmux.contains_key(&other) {
                    return true;
                }
                match button {
                    // The other connection goes; this one stays attached.
                    Some("Force Detach Other") => {
                        if let Some(t) = self.tmux.get_mut(&other) {
                            t.gw.force_disconnect();
                        }
                        self.tmux_disconnected(other, true);
                    }
                    answer => {
                        if answer == Some("Reveal") {
                            if let Some(first) = self.tmux.get(&other).and_then(|t| t.panes.values().map(|(id, _)| *id).min()) {
                                self.reveal_pane(first);
                            }
                        }
                        // iTerm2 detaches this connection when the other one stays.
                        if let Some(t) = self.tmux.get_mut(&gw) {
                            t.gw.detach();
                        }
                        self.tmux_flush(gw);
                        if let Some(t) = self.tmux.get_mut(&gw) {
                            t.gw.force_disconnect();
                        }
                        self.tmux_disconnected(gw, true);
                    }
                }
            }
            return true;
        }
        if let Some(rest) = id.strip_prefix(CLOSE_TAB) {
            let mut it = rest.split(':').map(|s| s.parse::<u64>().ok());
            if let (Some(Some(gw)), Some(Some(w))) = (it.next(), it.next()) {
                if let Some(b) = button {
                    self.tmux_close_answer(&[(gw, vec![w as u32])], b, false);
                }
            }
            return true;
        }
        if let Some((gw, whole)) = crate::tmux_persist::parse_close_win(id) {
            if let Some(b) = button {
                let scope = self.tmux_scope((gw != 0).then_some(gw));
                self.tmux_close_answer(&scope, b, whole);
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
        pane.screen.set_scrollback_with_status_bar(profile.bool("Scrollback With Status Bar"));
        pane.profile = profile.guid();
        pane.dvr = crate::dvr::Dvr::new(crate::pane::capacity_for(self.globals.int("IRMemory")));
        // TERM is tmux's default-terminal, else "screen" (iTerm2's termType for tmux panes).
        let term = self.tmux.get(&gw).and_then(|t| t.hs.default_terminal.clone()).unwrap_or_else(|| "screen".into());
        if self.globals.bool("translateScreenToXterm") && crate::screenterm::screen_like(&term, self.globals.bool("convertItalicsToReverseVideoForTmuxBugwardsCompatible")) {
            pane.screen_filter = Some(crate::screenterm::Filter::new(self.globals.bool("convertItalicsToReverseVideoForTmux")));
        }
        pane.own_mut().feed_tmux_restore(&p.restore);
        let pending = match pane.screen_filter.as_mut() {
            Some(f) => f.feed(&p.pending),
            None => p.pending.clone(),
        };
        pane.feed_output(&pending);
        // `@uservars`: the pane's user.* variables (iTerm2's setTmuxController).
        for (k, v) in &p.user_vars {
            if let Some(name) = k.strip_prefix("user.") {
                pane.own_mut().shell.vars.insert(name.to_string(), v.clone());
            }
        }
        self.panes.insert(id, pane);
        // The job is watched only when the profile shows an icon (iTerm2's PS:10098).
        let job = profile.int("Icon") != 0;
        let pal = Palette::from_profile(&profile, self.dark());
        if let Some(t) = self.tmux.get_mut(&gw) {
            t.panes.insert(p.pane, (id, window));
            t.ctl.watch_pane(p.pane, job, &mut t.gw);
            // tmux answers OSC 10 and 11 in the pane from these (3.4 and later).
            t.ctl.color_reports(p.pane, (pal.fg.r, pal.fg.g, pal.fg.b), (pal.bg.r, pal.bg.g, pal.bg.b), &mut t.gw);
        }
        self.tmux_flush(gw);
        id
    }

    fn tmux_open(&mut self, gw: PaneId, window: Window, node: Node, panes: Vec<PaneOpen>, place: Place) {
        let window_id = window.id;
        let mut ids = HashMap::new();
        for p in &panes {
            ids.insert(p.pane, self.tmux_pane(gw, window.id, p));
        }
        let Some(layout) = tmux_layout::to_layout(&node, &ids) else {
            // iTerm2's opener completes even so: a waiting CreateWindow gets no tab ("0").
            return self.api_tmux_window_opened(gw, window_id, None);
        };
        let focused = ids.get(&panes.first().map_or(0, |p| p.pane)).copied().unwrap_or(0);
        let tab = Tab { id: self.next_tab_id, layout, focused, zoomed: None, title: Some(self.tmux_title(&window.name)) };
        self.next_tab_id += 1;
        // What was saved about this window: the tab's title and colors, its place, and the
        // Tandem window's size and origin.
        let tmux_panes: Vec<u32> = panes.iter().map(|p| p.pane).collect();
        let restore = self.tmux.get_mut(&gw).map(|t| t.ctl.restore_of(window.id, &tmux_panes)).unwrap_or_default();
        let variable = self.tmux.get(&gw).is_some_and(|t| t.ctl.variable_sizes());
        if let Some(title) = restore.tab_title.clone() {
            if let Some(t) = self.tmux.get_mut(&gw) {
                t.tab_overrides.insert(window.id, title);
            }
        }
        for (p, c) in &restore.tab_colors {
            if let (Some((r, g, b)), Some(id)) = (c, ids.get(p)) {
                self.tab_colors.insert(*id, crate::palette::rgb(*r, *g, *b));
            }
        }
        let size = restore.settings.and_then(|s| s.size).filter(|_| self.globals.bool("rememberTmuxWindowSizes") && variable);
        let origin = restore.origin.filter(|_| !self.globals.bool("disableTmuxWindowPositionRestoration"));
        let tab_index = restore.tab_index;
        let tab_id = tab.id;
        // A window hidden from a Tandem window that is still open goes back there, even
        // when no tmux tab is left in it.
        let home = restore.terminal.as_deref().and_then(|t| t.strip_prefix("pty-")).and_then(|k| k.parse::<u64>().ok()).and_then(|k| {
            if self.windows.current() == Some(k) {
                self.tabs.first().map(|t| t.focused)
            } else {
                self.windows.parked().find(|(x, _)| **x == k).and_then(|(_, s)| s.tabs.first().map(|t| t.focused))
            }
        });
        let others_here = |app: &App| app.tabs.iter().flat_map(|t| t.layout.panes()).any(|id| app.panes.get(&id).and_then(|p| p.tmux_io()).is_some_and(|io| io.gateway != gw));
        let target = match place {
            Place::WithSibling(w) => self.tmux_window_panes(gw, w).into_iter().find(|id| !ids.values().any(|x| x == id)),
            Place::GatewayWindow => Some(gw),
            Place::Focused => (!others_here(self)).then(|| self.tabs.get(self.active).map(|t| t.focused)).flatten(),
            Place::Topmost => self.tmux.get(&gw).and_then(|t| t.panes.values().map(|(id, _)| *id).find(|id| !ids.values().any(|x| x == id))),
            Place::NewWindow => None,
        };
        let target = home.or(target);
        match target {
            Some(anchor) if self.enter_pane_window(anchor) => {
                // A window hidden from a tab goes back to its place.
                let at = tab_index.map_or(self.tabs.len(), |i| i.min(self.tabs.len()));
                self.tabs.insert(at, tab);
                self.active = at;
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
            _ => {
                if origin.is_some() || size.is_some() {
                    self.tmux_geometry.insert(tab_id, (origin, size));
                }
                self.pending_windows.push(windows_ui::NewWindow::Tabs(vec![tab]));
            }
        }
        self.tmux_save_tab_colors();
        // A Python API CreateWindow waiting for this window answers with its tab (once the
        // tab's window exists, when it opens as a new window).
        self.api_tmux_window_opened(gw, window_id, Some(tab_id));
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
        // Each tab's own area: a tmux tab's shared status bar is already out of it.
        let tabs: Vec<(PaneId, (u16, u16))> = self.tabs.iter().map(|t| (t.focused, self.grid_size(self.content_area_for(t)))).collect();
        let mut changed: Vec<PaneId> = Vec::new();
        for (focused, (cols, rows)) in tabs {
            let Some(io) = self.panes.get(&focused).and_then(|p| p.tmux_io()) else { continue };
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

    /// AutoHideTmuxClientSession: the gateway is buried once its windows open, with iTerm2's
    /// notification the first time ever (NoSyncAutoBurialReveal).
    pub(super) fn tmux_auto_bury(&mut self) {
        if !self.pending_windows.is_empty() {
            return;
        }
        let due: Vec<PaneId> = self.tmux.iter().filter(|(_, t)| t.bury_pending).map(|(g, _)| *g).collect();
        for gw in due {
            if let Some(t) = self.tmux.get_mut(&gw) {
                t.bury_pending = false;
            }
            if !self.bury_pane(gw) {
                continue;
            }
            if let Some(t) = self.tmux.get_mut(&gw) {
                t.auto_buried = true;
            }
            if self.globals.bool("NoSyncAutoBurialReveal") {
                continue;
            }
            if let Err(e) = crate::notify::show_with("Session Buried", "It can be restored by detaching from tmux, or from the Sessions > Buried Sessions menu.", self.globals.bool("silentUserNotifications")) {
                self.banners.set("notify", format!("Could not show a notification: {e}"));
            }
            self.globals.set("NoSyncAutoBurialReveal", toml::Value::Boolean(true));
            self.save_globals();
        }
    }

    /// A tmux window hidden by Tandem: which Tandem window and tab it came from.
    pub(super) fn tmux_note_burial(&mut self, gw: PaneId, window: u32) {
        let Some(index) = self.tabs.iter().position(|t| self.tmux_of(t.focused).is_some_and(|(g, _, w)| g == gw && w == window)) else { return };
        let Some(key) = self.windows.current() else { return };
        if let Some(t) = self.tmux.get_mut(&gw) {
            t.ctl.bury_window(window, &format!("pty-{key}"), index);
        }
    }

    /// A tmux tab's label when it is the session title (iTerm2's updateTabTitleForCurrentSessionName):
    /// no set-titles string, and tmux gave the pane a title. None: the tab's own title.
    pub(super) fn tmux_tab_label(&self, focused: PaneId) -> Option<String> {
        let io = self.panes.get(&focused)?.tmux_io()?;
        let t = self.tmux.get(&io.gateway)?;
        let window = t.panes.get(&io.pane)?.1;
        let user = t.tab_overrides.get(&window);
        let pane_title = io.info.borrow().title.clone();
        if user.is_none() && (t.window_titles.contains_key(&window) || pane_title.is_empty()) {
            return None;
        }
        let prefix = self.globals.string("tmuxTitlePrefix");
        Some(crate::tmux_clients::tab_label(user.map(String::as_str), None, &pane_title, &self.session_title(focused), "", &prefix))
    }

    /// Every tmux pane's colors go to tmux again (a profile or the appearance changed).
    pub(super) fn tmux_resend_colors(&mut self) {
        let dark = self.dark();
        let panes: Vec<(PaneId, u32, (u8, u8, u8), (u8, u8, u8))> = self
            .panes
            .values()
            .filter_map(|p| {
                let io = p.tmux_io()?;
                let pr = self.store.get(&p.profile)?;
                let pal = Palette::from_profile(pr, dark);
                Some((io.gateway, io.pane, (pal.fg.r, pal.fg.g, pal.fg.b), (pal.bg.r, pal.bg.g, pal.bg.b)))
            })
            .collect();
        for (gw, pane, fg, bg) in panes {
            if let Some(t) = self.tmux.get_mut(&gw) {
                t.ctl.color_reports(pane, fg, bg, &mut t.gw);
            }
            self.tmux_flush(gw);
        }
    }

    /// The gateway of the focused session: itself, or the gateway of its tmux pane.
    fn focused_tmux_gateway(&self) -> Option<PaneId> {
        let id = self.focused_id()?;
        if self.tmux.contains_key(&id) {
            return Some(id);
        }
        self.tmux_of(id).map(|(g, _, _)| g)
    }

    /// tmux's prefix bindings for the focused tmux pane (empty elsewhere).
    pub(super) fn tmux_key_map(&self) -> toml::Table {
        let Some(id) = self.focused_id() else { return toml::Table::new() };
        let Some((gw, _, _)) = self.tmux_of(id) else { return toml::Table::new() };
        self.tmux.get(&gw).map(|t| crate::tmux_prefix::key_map(&t.hs.keys)).unwrap_or_default()
    }

    /// Key action 65, Send tmux Command (iTerm2's performTmuxCommand).
    pub(super) fn send_tmux_command(&mut self, command: &str) {
        let Some(gw) = self.focused_tmux_gateway() else { return };
        if let Some(t) = self.tmux.get_mut(&gw) {
            t.gw.send(command, None, Default::default());
        }
        self.tmux_flush(gw);
    }

    /// A tmux pane's gateway, tmux pane id, and tmux window.
    pub(super) fn tmux_of(&self, pane: PaneId) -> Option<(PaneId, u32, u32)> {
        let io = self.panes.get(&pane)?.tmux_io()?;
        let window = self.tmux.get(&io.gateway)?.panes.get(&io.pane)?.1;
        Some((io.gateway, io.pane, window))
    }

    /// The profile's Initial Directory for tmux (Home, Reuse, or Custom).
    fn tmux_dir(&self, pane: PaneId) -> crate::tmux_edits::Dir {
        tmux_dir_of(self.profile_of(pane))
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
        // The tab shows the name the person gave it (iTerm2's tabTitleOverrideFormat).
        if name.is_empty() {
            t.tab_overrides.remove(&window);
        } else {
            t.tab_overrides.insert(window, name.to_string());
        }
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
            let legacy = self.globals.bool("NoSyncNewWindowOrTabFromTmuxOpensTmux");
            let specific = self.globals.bool(crate::tmux_edits::new_kind_key(window, false));
            let answer = if crate::tmux_edits::skip_new_kind_question(legacy, specific) {
                Some(true)
            } else {
                let (heading, text, b) = crate::tmux_edits::new_kind_question(window);
                super::broadcast_ui::yes_no_cancel(self.hwnd(), heading, &format!("{text}\n\nYes: {}. No: {}.", b[0], b[1]))
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
        self.tmux_drag_cancel();
        let Some(tab) = self.tabs.get(self.active) else { return };
        let before = crate::layout::pane_before(&tab.layout.tree(), path);
        let Some((gw, p, _)) = before.and_then(|id| self.tmux_of(id)) else { return };
        let Some(win) = self.windows.current() else { return };
        let pos = if axis == Axis::Row { self.cursor.0 } else { self.cursor.1 };
        if let Some(t) = self.tmux.get_mut(&gw) {
            t.gw.pause();
        }
        self.tmux_drag = Some((win, gw, p, axis == Axis::Row, pos));
    }

    /// The drag ended in its window: one resize-pane for how far it moved, then tmux's
    /// held lines.
    pub(super) fn tmux_drag_end(&mut self, event_loop: &ActiveEventLoop) {
        let Some((win, gw, p, horizontal, start)) = self.tmux_drag.take() else { return };
        if self.windows.current() != Some(win) {
            self.tmux_drag = Some((win, gw, p, horizontal, start));
            return;
        }
        let (cw, ch) = self.cell();
        let pos = if horizontal { self.cursor.0 } else { self.cursor.1 };
        let cells = ((pos - start) / if horizontal { cw } else { ch }).round() as i32;
        let Some(t) = self.tmux.get_mut(&gw) else { return };
        t.ctl.resize_pane(p, cells, horizontal, &mut t.gw);
        let events = t.gw.resume();
        self.tmux_flush(gw);
        self.tmux_events(gw, events, event_loop);
    }

    /// A drag that never got its release (focus lost, another press): tmux's lines go on
    /// and nothing is resized.
    pub(super) fn tmux_drag_cancel(&mut self) {
        let Some((_, gw, ..)) = self.tmux_drag.take() else { return };
        let Some(t) = self.tmux.get_mut(&gw) else { return };
        let events = t.gw.resume();
        self.tmux_held_events.push((gw, events));
    }

    /// A tmux pane closed in Tandem: the controller forgets it (tmux keeps it until 8c sends
    /// kill-pane), so the next layout that names it opens it again.
    pub(super) fn tmux_pane_closed(&mut self, id: PaneId) {
        let Some((gw, pane)) = self.panes.get(&id).and_then(|p| p.tmux_io()).map(|io| (io.gateway, io.pane)) else { return };
        if let Some(t) = self.tmux.get_mut(&gw) {
            t.panes.retain(|_, (x, _)| *x != id);
            t.ctl.forget_pane(pane);
            t.ctl.unwatch_pane(pane, &mut t.gw);
        }
        self.tmux_flush(gw);
        self.tmux_watch_status(gw);
    }

    /// The profile tmux panes use: the "tmux" profile with TmuxUsesDedicatedProfile (made
    /// the first time), else the gateway session's.
    pub(super) fn tmux_profile_guid(&mut self, gw: PaneId) -> String {
        if self.globals.bool("TmuxUsesDedicatedProfile") {
            let (guid, made) = self.store.tmux_profile();
            if made {
                self.tools_changed();
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
            // A key press: the working directory may have changed (iTerm2's userDidPressKey).
            t.ctl.pane_event(pane, crate::tmux_controller::PaneEvent::Key, &mut t.gw);
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

/// A window's outer origin (screen pixels) and its size for `@per_window_settings`: the
/// outer size in logical pixels, or full screen.
/// None while the window is minimized (Windows parks it off screen at a caption's size).
fn window_geometry(w: &winit::window::Window) -> Option<((i32, i32), Option<crate::tmux_persist::WinSize>)> {
    if w.is_minimized() == Some(true) {
        return None;
    }
    let origin = w.outer_position().map(|p| (p.x, p.y)).unwrap_or((0, 0));
    if w.fullscreen().is_some() {
        return Some((origin, Some(crate::tmux_persist::WinSize::FullScreen)));
    }
    let s = w.outer_size().to_logical::<f64>(w.scale_factor());
    Some((origin, Some(crate::tmux_persist::WinSize::Size(s.width.round() as i64, s.height.round() as i64))))
}

/// A profile's Initial Directory for tmux (Home, Reuse, or Custom); Home without a profile.
pub(super) fn tmux_dir_of(p: Option<&Profile>) -> crate::tmux_edits::Dir {
    use crate::tmux_edits::Dir;
    let Some(p) = p else { return Dir::Home };
    match p.string("Custom Directory").as_str() {
        "Yes" => Dir::Custom(p.string("Working Directory")),
        "Recycle" => Dir::Recycle,
        _ => Dir::Home,
    }
}
