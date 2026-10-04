//! The Cockpit in the app (Window > Cockpit): opening it, hiding it with the app, and
//! keeping its frame, split and group mode.

use super::*;
use crate::cockpit::{self, Act, Frame, GroupMode, Jump, View};
use crate::cockpit_rows::{RowKey, SessionFact, TabFact, WindowFact, World};
use crate::cockpit_ui::CockpitWindow;

/// A drag emits many moves; the globals file is written at most this often.
const SAVE_EVERY: std::time::Duration = std::time::Duration::from_secs(1);

/// The panel's state at launch: the saved group mode and list height.
pub(super) fn initial_view(g: &Globals) -> View {
    let split = g.string(cockpit::SPLIT_KEY).trim().parse::<f32>().ok().filter(|v| v.is_finite() && *v > 0.0);
    View { mode: GroupMode::from_raw(g.int(cockpit::GROUP_KEY)), list_height: split, ..Default::default() }
}

impl App {
    /// Gives the pane the stable id its saved session carried, when the table came from the
    /// launch's own restoration (`launch`) and no other live or buried pane holds that id
    /// (ruling P1: an arrangement, a file or a duplicate never adopts one).
    pub(super) fn adopt_stable_id(&mut self, pane: PaneId, session: &toml::Value, launch: bool) {
        let id = crate::stable_id::adopt(super::restore_ui::stable_id_in(session).as_deref(), launch, &|id| self.panes.iter().any(|(k, p)| *k != pane && p.stable_id == id));
        let Some(id) = id else { return };
        if let Some(p) = self.panes.get_mut(&pane) {
            p.keep_stable_id(&id);
        }
    }

    /// The pane a model's reference names: a stable id of a live or buried pane. A decimal pane
    /// id or the per-launch session reference names nothing (ruling P3).
    #[allow(dead_code)] // the orchestration dispatcher (a later task) resolves its references here
    pub(super) fn pane_by_reference(&self, reference: &str) -> Option<PaneId> {
        crate::stable_id::find(reference, self.panes.iter().map(|(k, p)| (*k, p.stable_id.as_str())))
    }

    /// A mention's current session name, else `DEFUNCT` (a session that is gone, or an id that
    /// names none; only a stable id names a session, ruling P3).
    pub(super) fn mention_label(&self, id: &str) -> String {
        match self.pane_by_reference(id) {
            Some(p) => crate::cockpit_rows::session_title(&self.session_label(p).0, p),
            None => crate::mention::DEFUNCT.to_string(),
        }
    }

    /// A chip's label: "Window N", a tab's title, a session's name, else `DEFUNCT`.
    #[allow(dead_code)] // the window labels chips from the world it is handed; the command bar's messages (Task 6) name one chip here
    pub(super) fn cockpit_chip_label(&self, chip: &crate::mention::Chip) -> String {
        crate::mention::chip_label(&self.cockpit_world(), chip)
    }

    fn session_fact(&self, pane: PaneId) -> Option<SessionFact> {
        let p = self.panes.get(&pane)?;
        let s = p.own();
        Some(SessionFact { pane, stable_id: p.stable_id.clone(), name: self.session_label(pane).0, status: s.tab_status.clone(), exited: p.exited.is_some(), alt_screen: s.alt_screen(), ancestors: s.shell.job_ancestors.clone(), armed: self.status_watches.contains_key(&pane) })
    }

    /// The Cockpit's snapshot: every window by number (current or parked, as `quickly_sources`
    /// walks them), then the buried sessions. `armed` is the bell: a session's watch or a window's Notify on Status Change.
    pub(super) fn cockpit_world(&self) -> World {
        let mut world = World::default();
        for k in self.registry.order() {
            let (Some(guid), Some(number), Some((tabs, active))) = (self.registry.guid(k), self.registry.number(k), self.window_tabs(k)) else { continue };
            let custom = if self.windows.current() == Some(k) { self.win_title.clone() } else { self.windows.parked().find(|(kk, _)| **kk == k).and_then(|(_, s)| s.win_title.clone()) };
            let title = custom.filter(|t| !t.is_empty()).or_else(|| tabs.get(active).map(|t| self.window_title(t.focused))).unwrap_or_default();
            let tabs = tabs.iter().map(|t| TabFact { id: t.id, title: t.title.clone().unwrap_or_default(), sessions: t.layout.panes().into_iter().filter_map(|p| self.session_fact(p)).collect() }).collect();
            world.windows.push(WindowFact { guid: guid.to_string(), number, title, active_tab: active, tabs, armed: self.window_armed(k) });
        }
        world.buried = self.buried.items().into_iter().filter_map(|(p, _)| self.session_fact(p)).collect();
        world
    }

    /// Window > Cockpit (`showAndFocusCommand`): opens the one window where it was last, or
    /// shows it again, and puts the caret in the command bar.
    pub(super) fn reveal_cockpit(&mut self, event_loop: &ActiveEventLoop) {
        self.cockpit_view.focus_command = true;
        if let Some(w) = self.cockpit.as_mut() {
            w.show_and_focus();
            return;
        }
        let Some(g) = self.gpu.as_ref() else { return };
        let work: Vec<cockpit::Area> = super::requests_ui::work_areas().into_iter().map(|(x, y, w, h)| cockpit::Area { x: x as f32, y: y as f32, w: w as f32, h: h as f32 }).collect();
        let frame = cockpit::place(Frame::parse(&self.globals.string(cockpit::FRAME_KEY)), &work, g.window.scale_factor() as f32);
        let dark = self.dark();
        match CockpitWindow::open(event_loop, &g.instance, &g.adapter, &g.device, &g.queue, frame, dark) {
            Ok(mut w) => {
                if let Some(hwnd) = w.hwnd() {
                    // The key hook takes Win chords only for registered windows.
                    crate::winkeys::add_cockpit_window(hwnd);
                }
                w.show_and_focus();
                self.cockpit = Some(w);
            }
            Err(e) => self.banners.set("window", format!("Could not open the window: {e}")),
        }
    }

    /// Hidden while another app is in front, back when Tandem is (`hidesOnDeactivate`); the
    /// Cockpit's own window counts as Tandem's.
    pub(super) fn cockpit_follows_app(&mut self) {
        let in_front = super::hotkey_ui::tandem_in_front();
        if let Some(w) = self.cockpit.as_mut() {
            w.set_shown(in_front);
        }
    }

    /// Events for the Cockpit window. False when `window` is not it.
    pub(super) fn cockpit_event(&mut self, event_loop: &ActiveEventLoop, window: WindowId, event: &WindowEvent) -> bool {
        let Some(w) = self.cockpit.as_mut() else { return false };
        if w.id() != window {
            return false;
        }
        match event {
            WindowEvent::CloseRequested => self.cockpit_acts(event_loop, vec![Act::Closed]),
            WindowEvent::RedrawRequested => {
                // The bell follows the first chosen row: what it is and whether it is armed.
                let kind = self.cockpit_view.selected.first().cloned();
                let armed = match &kind {
                    Some(RowKey::Session(p)) => self.status_watches.contains_key(p),
                    Some(RowKey::Window(g)) => self.registry.by_guid(g).is_some_and(|k| self.window_armed(k)),
                    _ => false,
                };
                self.cockpit_view.selection_kind = kind;
                self.cockpit_view.selection_armed = armed;
                // The rows come from the world each redraw; the filter, expansion and selection stay
                // in the view by key. A filter whose status vanished goes back to All.
                let world = self.cockpit_world();
                let patterns = self.status_patterns();
                let view = &mut self.cockpit_view;
                let (total, per) = crate::cockpit_rows::counts(view.mode, &world, &patterns, &view.search);
                if view.status_filter.as_ref().is_some_and(|f| !per.iter().any(|(k, _)| k == f)) {
                    view.status_filter = None;
                }
                let rows = crate::cockpit_rows::build(view.mode, &world, &patterns, &view.search, view.status_filter.as_deref());
                view.selected = crate::cockpit_rows::keep_selection(&view.selected, &rows);
                let items = cockpit::filter_items(total, &per);
                let Some(w) = self.cockpit.as_mut() else { return true };
                let acts = w.redraw(&mut self.cockpit_view, &rows, &items, &world);
                self.cockpit_acts(event_loop, acts);
            }
            other => {
                w.event(other);
            }
        }
        true
    }

    /// Each pass: the tip line expires on its own, and a pending save of the frame is written.
    pub(super) fn cockpit_tick(&mut self) -> Option<Instant> {
        let now = Instant::now();
        if self.cockpit_save_due.is_some_and(|t| now >= t) {
            self.cockpit_save_due = None;
            self.save_globals();
        }
        if self.cockpit_view.tip.expire(now) {
            if let Some(w) = self.cockpit.as_ref() {
                w.request_redraw();
            }
        }
        let refresh = self.cockpit.as_mut().and_then(|w| w.refresh(now));
        [self.cockpit_save_due, self.cockpit_view.tip.next_change(), refresh].into_iter().flatten().min()
    }

    /// What a redraw asked for: the window closes (the view keeps its state), or the frame or
    /// split is stored, written to the settings file at most once a second.
    pub(super) fn cockpit_acts(&mut self, event_loop: &ActiveEventLoop, acts: Vec<Act>) {
        for act in acts {
            match act {
                Act::Closed => {
                    if let Some(hwnd) = self.cockpit.as_ref().and_then(|w| w.hwnd()) {
                        crate::winkeys::remove_cockpit_window(hwnd);
                    }
                    self.cockpit = None;
                    self.cockpit_save_due = None;
                    self.save_globals();
                }
                Act::Moved(f) => self.cockpit_store(cockpit::FRAME_KEY, f.text()),
                Act::Split(h) => self.cockpit_store(cockpit::SPLIT_KEY, h.to_string()),
                // The gear opens Session Status' settings under it, with the same patterns, merge
                // and subtitle; a second press closes it.
                Act::Settings => {
                    if self.status_popover_closes() {
                        continue;
                    }
                    let Some(w) = self.cockpit.as_ref() else { continue };
                    let Some(at) = w.gear_anchor(&self.cockpit_view) else { continue };
                    let work = w.hwnd().and_then(super::windows_ui::monitor_rects_of).map(|(work, _)| work);
                    self.show_status_popover(event_loop, at, work);
                }
                Act::Group(mode) => {
                    self.globals.set(cockpit::GROUP_KEY, toml::Value::Integer(mode.raw()));
                    self.cockpit_view.mode = mode;
                    self.cockpit_save_due.get_or_insert_with(|| Instant::now() + SAVE_EVERY);
                    if let Some(w) = self.cockpit.as_ref() {
                        w.request_redraw();
                    }
                }
                // A window row arms its window, a session row its session.
                Act::Notify => match self.cockpit_view.selected.first().cloned() {
                    Some(RowKey::Window(g)) => {
                        if let Some(k) = self.registry.by_guid(&g) {
                            self.arm_window_watch(k);
                        }
                    }
                    Some(RowKey::Session(p)) => self.arm_session_watch(p),
                    _ => {}
                },
                Act::Search(text) => self.cockpit_view.search = text,
                Act::Select(keys) => self.cockpit_select(keys),
                Act::Send => self.cockpit_send(),
                Act::RemoveChip(chip) => {
                    let world = self.cockpit_world();
                    self.cockpit_view.selected = cockpit::deselect_chip(&self.cockpit_view.selected, &chip, &world);
                    self.cockpit_redraw();
                }
                // A picked item chooses its row when the list shows it; the chips stay as picked.
                Act::CommitPick(chip) => {
                    let world = self.cockpit_world();
                    if let Some(key) = cockpit::chip_row(&chip, &world) {
                        let rows = self.cockpit_rows(&world);
                        if !crate::cockpit_rows::keep_selection(std::slice::from_ref(&key), &rows).is_empty() && !self.cockpit_view.selected.contains(&key) {
                            self.cockpit_view.selected.push(key);
                        }
                    }
                    self.cockpit_redraw();
                }
                Act::Toggle(key) => {
                    if !self.cockpit_view.collapsed.remove(&key) {
                        self.cockpit_view.collapsed.insert(key);
                    }
                    self.cockpit_redraw();
                }
                Act::Jump(key) => self.cockpit_jump(cockpit::jump_for(&key)),
                Act::StatusFilter(f) => {
                    self.cockpit_view.status_filter = f;
                    self.cockpit_redraw();
                }
            }
        }
    }

    /// The rows the list shows now (mode, search and status filter applied).
    fn cockpit_rows(&self, world: &World) -> Vec<crate::cockpit_rows::Row> {
        let v = &self.cockpit_view;
        crate::cockpit_rows::build(v.mode, world, &self.status_patterns(), &v.search, v.status_filter.as_deref())
    }

    /// Chooses rows and makes their chips the command bar's targets (`CW:958-966`).
    pub(super) fn cockpit_select(&mut self, keys: Vec<RowKey>) {
        let world = self.cockpit_world();
        self.cockpit_view.chips = cockpit::chips_after_send(&keys, &world);
        self.cockpit_view.selected = keys;
        self.cockpit_redraw();
    }

    /// The panes a chip token names: `win:` every pane of that window, `tab:` every pane of
    /// that tab, else a session by stable id (ruling P3). The token has no "@".
    pub(super) fn cockpit_resolve(&self, token: &str) -> Option<Vec<PaneId>> {
        if let Some(guid) = token.strip_prefix("win:") {
            let k = self.registry.by_guid(guid)?;
            let (tabs, _) = self.window_tabs(k)?;
            return Some(tabs.iter().flat_map(|t| t.layout.panes()).collect());
        }
        if let Some(id) = token.strip_prefix("tab:") {
            let id: u64 = id.parse().ok()?;
            return self.registry.order().into_iter().find_map(|k| self.window_tabs(k).and_then(|(tabs, _)| tabs.iter().find(|t| t.id == id).map(|t| t.layout.panes())));
        }
        self.pane_by_reference(token).map(|p| vec![p])
    }

    /// The command bar's Send. The text goes, as the person typed it, only to the panes the
    /// person chose (chips or chosen rows) and only if live: a buried or exited pane is never
    /// typed into (ruling P8). It is the person's own input (`SendTextLocal`, never
    /// broadcast; ruling P17).
    pub(super) fn cockpit_send(&mut self) {
        let now = Instant::now();
        self.cockpit_view.picker = None;
        let world = self.cockpit_world();
        let rows = self.cockpit_rows(&world);
        let tokens: Vec<String> = self.cockpit_view.chips.iter().map(|c| format!("@{}", c.token())).collect();
        let selected = crate::cockpit_rows::sessions_under(&rows, &self.cockpit_view.selected);
        let shown = crate::cockpit_rows::all_sessions(&rows);
        let state = |p: PaneId| world.session(p).map_or(crate::session_state::State::Unknown, |s| crate::session_state::keyword_state(&s.status));
        let text = self.cockpit_view.text.clone();
        let targets = match cockpit::plan(&tokens, &text, &selected, &shown, &state, &|t| self.cockpit_resolve(t)) {
            Ok(None) => return,
            Err(e) => {
                self.cockpit_view.tip.show(&e.text(), true, now);
                self.cockpit_redraw();
                return;
            }
            Ok(Some(t)) => t,
        };
        let live = |p: PaneId| cockpit::typable(&world, p) && self.panes.get(&p).is_some_and(|x| x.exited.is_none()) && !self.buried.contains(p);
        let writes = cockpit::writes(&targets, &text, &live);
        if writes.is_empty() {
            self.cockpit_view.tip.show(&cockpit::SendError::NoneRunning.text(), true, now);
            self.cockpit_redraw();
            return;
        }
        let n = writes.len();
        for (pane, bytes) in writes {
            // A pane of another window is entered first, so its broadcast domain is that window's.
            self.enter_pane_window(pane);
            self.user_write(pane, &bytes, crate::broadcast::Write::SendTextLocal);
        }
        self.cockpit_view.text.clear();
        self.cockpit_view.chips = cockpit::chips_after_send(&self.cockpit_view.selected, &world);
        self.cockpit_view.tip.show(&cockpit::sent_text(n), false, now);
        self.cockpit_redraw();
    }

    fn cockpit_redraw(&self) {
        if let Some(w) = self.cockpit.as_ref() {
            w.request_redraw();
        }
    }

    /// A double-click on a row (`CW:667-743`): a session is shown (a buried one is put back
    /// first), a window shows its active tab's focused session, a tab its focused session.
    pub(super) fn cockpit_jump(&mut self, jump: Jump) {
        let pane = match jump {
            Jump::Session(p) => Some(p),
            Jump::Window(guid) => self.registry.by_guid(&guid).and_then(|k| self.window_tabs(k)).and_then(|(tabs, active)| tabs.get(active).map(|t| t.focused)),
            Jump::Tab(id) => self.registry.order().into_iter().find_map(|k| self.window_tabs(k).and_then(|(tabs, _)| tabs.iter().find(|t| t.id == id).map(|t| t.focused))),
            Jump::Nothing => None,
        };
        let Some(pane) = pane.filter(|p| self.panes.contains_key(p)) else { return };
        if self.buried.contains(pane) {
            self.disinter(pane);
        }
        self.reveal_pane(pane);
    }

    fn cockpit_store(&mut self, key: &str, text: String) {
        self.globals.set(key, toml::Value::String(text));
        if self.cockpit_save_due.is_none() {
            self.cockpit_save_due = Some(Instant::now() + SAVE_EVERY);
        }
    }
}
