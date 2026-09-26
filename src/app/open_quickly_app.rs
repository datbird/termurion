//! View > Open Quickly in the app (`iTermOpenQuicklyWindowController.m`, `WC`): the sources
//! snapshot for every keystroke, the palette's events, the preview, and activation of each kind
//! of item through the paths the menus already use (`WC:367-533`). Nothing runs a program from
//! an item's text.

use super::*;
use crate::open_quickly::{self, Item, ItemKind, SessionSource, Sources};
use crate::open_quickly_ui::{self as ui, OpenQuicklyWindow, PaletteEvent, PaletteRow, PreviewWindow};

/// The most commands, directories and hosts a session offers for scoring.
const HISTORY_LIMIT: usize = 100;

/// An open palette: its window, the preview, what its rows are, and the picture of each pane
/// shown so far (dropped when the palette closes, as iTerm2 tears the preview down).
pub(super) struct Quickly {
    window: OpenQuicklyWindow,
    preview: Option<PreviewWindow>,
    items: Vec<Item>,
    /// The snapshot's snippets and actions, which `ItemKind::Snippet` and `ItemKind::Action`
    /// index.
    snippet_guids: Vec<String>,
    actions: Vec<crate::keyaction::Binding>,
    /// The monitor's work area, pixels, for placing the preview.
    work: Rect,
    images: HashMap<PaneId, (u32, u32, Vec<u8>)>,
}

/// Each item kind's Segoe Fluent Icons glyph (Decisions; no generated session logo).
fn icon(kind: &ItemKind) -> char {
    match kind {
        ItemKind::Session(_) => '\u{E756}',
        ItemKind::Window(_) => '\u{E737}',
        ItemKind::NewTab(_) => '\u{E710}',
        ItemKind::ChangeProfile(_) => '\u{E8AB}',
        ItemKind::Arrangement { .. } => '\u{E7C4}',
        ItemKind::Tip(_) => '\u{E946}',
        ItemKind::Action(_) => '\u{E945}',
        ItemKind::Snippet(_) => '\u{E8BD}',
        ItemKind::NamedMark { .. } => '\u{E8A4}',
        ItemKind::MenuItem(_) => '\u{E700}',
    }
}

fn row_of(item: &Item) -> PaletteRow {
    PaletteRow { icon: icon(&item.kind), title: item.title.clone(), title_bold: item.title_bold.clone(), detail: item.detail.clone(), detail_bold: item.detail_bold.clone() }
}

/// The last `HISTORY_LIMIT` distinct non-empty values, oldest first.
fn recent_distinct<'a>(values: impl Iterator<Item = &'a str>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for v in values.filter(|v| !v.is_empty()) {
        out.retain(|x| x != v);
        out.push(v.to_string());
    }
    let skip = out.len().saturating_sub(HISTORY_LIMIT);
    out.split_off(skip)
}

impl App {
    /// Stamps the key window's focused pane with the next focus ordinal when it just became
    /// the focused one (the palette's recency tiebreaker).
    pub(super) fn stamp_focus(&mut self) {
        if self.last_window.is_some() && self.last_window != self.current_window() {
            return;
        }
        let Some(id) = self.focused_id() else { return };
        if self.focus_order.get(&id) != Some(&self.focus_seq) {
            self.focus_seq += 1;
            self.focus_order.insert(id, self.focus_seq);
        }
    }

    /// View > Open Quickly (and its hotkey): the palette on the screen of the window in front,
    /// with the query kept from last time, or its focus when it is open.
    pub(super) fn open_quickly(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(q) = &self.open_quickly {
            return q.window.focus();
        }
        if let Some(w) = self.last_window {
            self.enter(w);
        }
        // The key window's screen (`[[NSApp keyWindow] screen]`, else the main screen): the
        // monitor of the window in front, whichever app it belongs to.
        let fg = super::hotkey_ui::foreground();
        let rects = (fg != 0).then(|| super::windows_ui::monitor_rects_of(fg)).flatten().or_else(|| self.monitor_rects());
        let Some((work, monitor)) = rects else {
            return self.banners.set("window", "Could not read the screen for Open Quickly.");
        };
        let dark = self.dark();
        let duration = self.globals.get("openQuicklyAnimationDuration").and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64))).unwrap_or(0.12) as f32;
        let Some(g) = self.gpu.as_ref() else { return };
        match OpenQuicklyWindow::open(event_loop, &g.instance, &g.adapter, &g.device, &g.queue, monitor, &self.quickly_query, dark, duration) {
            Ok(window) => {
                self.open_quickly = Some(Quickly { window, preview: None, items: Vec::new(), snippet_guids: Vec::new(), actions: Vec::new(), work, images: HashMap::new() });
                self.quickly_refresh(event_loop);
            }
            Err(e) => self.banners.set("window", format!("Could not open the window: {e}")),
        }
    }

    /// Handles an event for the palette or its preview. False when `window` is neither.
    pub(super) fn open_quickly_event(&mut self, event_loop: &ActiveEventLoop, window: WindowId, event: &WindowEvent) -> bool {
        let Some(q) = self.open_quickly.as_mut() else { return false };
        if let Some(p) = q.preview.as_mut().filter(|p| p.id() == window) {
            p.event(event);
            return true;
        }
        if q.window.id() != window {
            return false;
        }
        if !matches!(event, WindowEvent::RedrawRequested) {
            if !q.window.event(event) {
                self.close_quickly();
            }
            return true;
        }
        match q.window.redraw() {
            PaletteEvent::None => {}
            PaletteEvent::Close => self.close_quickly(),
            PaletteEvent::Query(query) => {
                self.quickly_query = query;
                self.quickly_refresh(event_loop);
            }
            PaletteEvent::Selected(row) => self.quickly_preview(row, event_loop),
            PaletteEvent::Open { row, alt } => {
                if let Some(item) = q.items.get(row).cloned() {
                    self.quickly_activate(&item, alt, event_loop);
                }
            }
        }
        true
    }

    /// Closes the palette and its preview (`close:`, `windowDidResignKey:`).
    fn close_quickly(&mut self) {
        if let Some(q) = self.open_quickly.take() {
            self.quickly_query = q.window.query().to_string();
        }
    }

    /// Runs the query again over a fresh snapshot, selects the first row, and shows its
    /// preview (`update`).
    fn quickly_refresh(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(w) = self.last_window {
            self.enter(w);
        }
        let (sources, snippet_guids, actions) = (self.quickly_sources(), self.quickly_snippets().into_iter().map(|i| self.snippets.snippets[i].guid.clone()).collect(), self.quickly_actions());
        let Some(q) = self.open_quickly.as_mut() else { return };
        let items = open_quickly::items(q.window.query(), &sources);
        q.window.set_items(items.iter().map(row_of).collect());
        q.items = items;
        q.snippet_guids = snippet_guids;
        q.actions = actions;
        match q.window.selected() {
            Some(row) => self.quickly_preview(row, event_loop),
            None => {
                if let Some(p) = self.open_quickly.as_mut().and_then(|q| q.preview.as_mut()) {
                    p.hide();
                }
            }
        }
    }

    /// The snippets the focused session's Snippets Filter lets through (their indexes).
    fn quickly_snippets(&self) -> Vec<usize> {
        crate::snippets::filtered(&self.snippets.snippets, &self.snippets_filter())
    }

    /// The Actions, as bindings.
    fn quickly_actions(&self) -> Vec<crate::keyaction::Binding> {
        crate::user_actions::load(self.globals.get("Actions").as_ref()).iter().map(|a| a.binding()).collect()
    }

    /// Everything Open Quickly searches, as it is now. `ItemKind::Snippet` indexes
    /// `quickly_snippets` and `ItemKind::Action` indexes `quickly_actions`.
    pub(super) fn quickly_sources(&self) -> Sources {
        let current = self.focused_id();
        let has_window = self.gpu.is_some() && current.is_some();
        let mut s = Sources { has_window, current_profile_type_ok: has_window, default_profile: self.store.default_profile().guid(), ..Default::default() };

        // Sessions and windows, window by window.
        let monitors = self.gpu.as_ref().map_or(1, |g| g.window.available_monitors().count());
        for k in self.registry.order() {
            let (tabs, gpu, hotkey) = if self.windows.current() == Some(k) {
                (&self.tabs, self.gpu.as_ref(), self.hotkey.as_ref())
            } else {
                match self.windows.parked().find(|(kk, _)| **kk == k) {
                    Some((_, slot)) => (&slot.tabs, slot.gpu.as_ref(), slot.hotkey.as_ref()),
                    None => continue,
                }
            };
            for t in tabs {
                for pane in t.layout.panes() {
                    if let Some(src) = self.session_source(pane, t, current) {
                        s.sessions.push(src);
                    }
                }
            }
            let Some(g) = gpu else { continue };
            let w = &g.window;
            let monitor = w.current_monitor().map(|m| m.name().unwrap_or_default().trim_start_matches("\\\\.\\").to_string());
            let hotkey_text = hotkey.and_then(|h| self.store.get(&h.guid)).and_then(|p| {
                let prefs = crate::hotkey::Prefs::from_profile(p);
                ui::hotkey_feature(prefs.primary.as_ref().map(crate::settings_hotkey::label), prefs.double_tap)
            });
            let features = ui::window_features(monitors, monitor.as_deref(), w.is_minimized().unwrap_or(false), w.fullscreen().is_some(), hotkey_text);
            s.windows.push((k, w.title(), features));
        }

        s.profiles = self.store.profiles.iter().map(|p| (p.name(), p.guid(), p.bool("Prevent Opening in a Tab"))).collect();
        let arrangements = self.arrangements();
        s.arrangements = arrangements.names().into_iter().map(str::to_string).collect();
        s.default_arrangement = arrangements.default_name().map(str::to_string);

        s.actions = self.quickly_actions().iter().map(|b| (b.label.clone(), crate::settings_keys::action_summary(b))).collect();
        let all = &self.snippets.snippets;
        s.snippets = self.quickly_snippets().into_iter().map(|i| (all[i].title.clone(), crate::snippets::ellipsize(&all[i].value, 80), all[i].display_title())).collect();

        // Named marks in every session, in screen order.
        for src in &s.sessions {
            let Some(p) = self.panes.get(&src.pane) else { continue };
            let mut rows = p.screen.row_marks();
            rows.sort_by_key(|(l, _)| *l);
            for (_, id) in rows {
                if let Some(name) = p.screen.marks.records.get(&id).and_then(|m| m.name.clone()).filter(|n| !n.is_empty()) {
                    s.named_marks.push((src.pane, id, name, Some(src.pane) == current));
                }
            }
        }

        // Every menu command Tandem runs.
        s.menu_items = crate::menus::items()
            .iter()
            .filter(|i| !i.separator && !i.submenu && !i.selector.is_empty() && crate::commands::status(i) == crate::commands::Status::Done)
            .map(|i| (i.id(), i.label().to_string(), i.path[..i.path.len().saturating_sub(1)].to_vec(), i.alternate))
            .collect();
        s
    }

    /// One session as Open Quickly scores it.
    fn session_source(&self, pane: PaneId, tab: &Tab, current: Option<PaneId>) -> Option<SessionSource> {
        let p = self.panes.get(&pane)?;
        let sh = &p.own().shell;
        let history = &sh.history[sh.history.len().saturating_sub(HISTORY_LIMIT)..];
        let cwd = sh.cwd.clone().unwrap_or_default();
        let host = sh.host.clone().unwrap_or_default();
        let dirs = recent_distinct(history.iter().filter_map(|r| r.cwd.as_deref()).chain(std::iter::once(cwd.as_str())));
        let hosts = recent_distinct(history.iter().filter_map(|r| r.host.as_deref()).chain(std::iter::once(host.as_str())));
        let profile = self.store.get(&self.store.shared_guid(&p.profile)).map(|pr| pr.name()).unwrap_or_default();
        Some(SessionSource {
            pane,
            current: Some(pane) == current,
            ordinal: self.focus_order.get(&pane).copied().unwrap_or(0),
            name: self.session_title(pane),
            // An Edit Tab Title title, or the tmux window's name (both land in the tab's title).
            tab_title: tab.title.clone(),
            badge: self.badge_text(pane),
            commands: history.iter().map(|r| r.command.clone()).filter(|c| !c.is_empty()).collect(),
            dirs,
            hosts,
            users: sh.user.iter().filter(|u| !u.is_empty()).cloned().collect(),
            profile,
            git_branch: self.git.get(&pane).and_then(|g| g.poller.state.as_ref()).map(|st| st.branch.clone()).unwrap_or_default(),
            user_vars: sh.vars.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
            pid: sh.job.as_ref().map(|j| j.pid.to_string()).unwrap_or_default(),
            cwd,
            last_command: history.last().map(|r| r.command.clone()).unwrap_or_default(),
            host,
        })
    }

    /// The session a row previews: a session's, a window's current one, a named mark's.
    fn preview_pane(&self, kind: &ItemKind) -> Option<PaneId> {
        match kind {
            ItemKind::Session(p) | ItemKind::NamedMark { pane: p, .. } => Some(*p),
            ItemKind::Window(k) => {
                if self.windows.current() == Some(*k) {
                    return self.focused_id();
                }
                self.windows.parked().find(|(kk, _)| **kk == *k).and_then(|(_, s)| s.active_tab()).map(|t| t.focused)
            }
            _ => None,
        }
    }

    /// The tab number (from 1) of the tab showing `pane`, and its window's title.
    fn pane_place(&self, pane: PaneId) -> Option<(usize, String)> {
        if let Some(i) = self.tabs.iter().position(|t| t.layout.panes().contains(&pane)) {
            return Some((i + 1, self.gpu.as_ref().map(|g| g.window.title()).unwrap_or_default()));
        }
        self.windows.parked().find_map(|(_, s)| {
            let i = s.tabs.iter().position(|t| t.layout.panes().contains(&pane))?;
            Some((i + 1, s.gpu.as_ref().map(|g| g.window.title()).unwrap_or_default()))
        })
    }

    /// Shows the preview for row `row`, or hides it for a row with no session (`refreshPreview`).
    fn quickly_preview(&mut self, row: usize, event_loop: &ActiveEventLoop) {
        let Some(kind) = self.open_quickly.as_ref().and_then(|q| q.items.get(row)).map(|i| i.kind.clone()) else { return };
        let Some(pane) = self.preview_pane(&kind).filter(|p| self.panes.contains_key(p)) else {
            if let Some(p) = self.open_quickly.as_mut().and_then(|q| q.preview.as_mut()) {
                p.hide();
            }
            return;
        };
        let title = self.session_title(pane);
        let detail = self.pane_place(pane).map(|(n, t)| ui::preview_detail(n, &t)).unwrap_or_default();
        if !self.open_quickly.as_ref().is_some_and(|q| q.images.contains_key(&pane)) {
            match self.visible_picture(pane) {
                Ok(img) => {
                    if let Some(q) = self.open_quickly.as_mut() {
                        q.images.insert(pane, img);
                    }
                }
                Err(e) => self.banners.set("open quickly", format!("Could not draw the preview: {e}")),
            }
        }
        if self.open_quickly.as_ref().is_some_and(|q| q.preview.is_none()) {
            let dark = self.dark();
            let Some(g) = self.gpu.as_ref() else { return };
            match PreviewWindow::open(event_loop, &g.instance, &g.adapter, &g.device, &g.queue, dark) {
                Ok(p) => {
                    if let Some(q) = self.open_quickly.as_mut() {
                        q.preview = Some(p);
                    }
                }
                Err(e) => return self.banners.set("window", format!("Could not open the preview: {e}")),
            }
        }
        let Some(q) = self.open_quickly.as_mut() else { return };
        let image = q.images.get(&pane).map(|(w, h, px)| (*w, *h, px.as_slice()));
        let size = image.map(|(w, h, _)| (w, h));
        let parent = q.window.target_rect();
        let work = q.work;
        // Placed in points beside the palette where it ends up (on the palette's screen, so at
        // its scale), then back to pixels.
        let k = q.window.scale() as f32;
        let Some(p) = q.preview.as_mut() else { return };
        let pt = |r: Rect| Rect { x: r.x / k, y: r.y / k, w: r.w / k, h: r.h / k };
        let r = ui::preview_rect(pt(parent), pt(work), size);
        p.set(pane, &title, &detail, image);
        p.place(Rect { x: r.x * k, y: r.y * k, w: r.w * k, h: r.h * k });
    }

    /// A picture of the lines of `pane` on screen now, drawn by its own window.
    fn visible_picture(&mut self, pane: PaneId) -> Result<(u32, u32, Vec<u8>), String> {
        use alacritty_terminal::grid::Dimensions;
        let back = self.current_window();
        if !self.enter_pane_window(pane) {
            return Err("the session has no window".into());
        }
        let out = (|| {
            let p = self.panes.get_mut(&pane).ok_or("the session is gone")?;
            let g = p.screen.term().grid();
            let first = g.history_size() - g.display_offset();
            let last = first + g.screen_lines().max(1) - 1;
            // Drawn from the pane's own screen, taken out for a moment (as Make Screenshot does).
            let screen = std::mem::replace(&mut p.screen, crate::screen::Screen::new(1, 1));
            let drawn = self.render_lines(pane, &screen, first, last);
            if let Some(p) = self.panes.get_mut(&pane) {
                p.screen = screen;
            }
            drawn
        })();
        if let Some(b) = back {
            self.enter(b);
        }
        out
    }

    /// Opens a row (`openSelectedRow`), then closes the palette; a tip instead puts its command
    /// in the field and stays open. Committing brings Tandem forward.
    pub(super) fn quickly_activate(&mut self, item: &Item, alt: bool, event_loop: &ActiveEventLoop) {
        if let ItemKind::Tip(c) = &item.kind {
            if let Some(q) = self.open_quickly.as_mut() {
                q.window.set_query(&format!("/{c} "));
            }
            self.quickly_query = format!("/{c} ");
            return self.quickly_refresh(event_loop);
        }
        let (snippet_guids, actions) = self.open_quickly.as_ref().map(|q| (q.snippet_guids.clone(), q.actions.clone())).unwrap_or_default();
        self.close_quickly();
        if let Some(w) = self.last_window {
            self.enter(w);
        }
        match &item.kind {
            ItemKind::Tip(_) => {}
            ItemKind::Session(pane) => self.reveal_pane(*pane),
            ItemKind::Window(k) => {
                let Some(id) = self.window_id_of_key(*k) else { return };
                self.enter(id);
                match self.hotkey.as_ref().map(|h| h.guid.clone()) {
                    Some(guid) => self.show_hotkey_window(&guid, super::hotkey_ui::foreground()),
                    None => self.focus_terminal_window(id),
                }
            }
            ItemKind::NewTab(guid) => {
                // The profile's hotkey window when it has a hotkey and no window yet;
                // otherwise a new tab in the current window (a window when there is none).
                let hotkey = self.store.get(guid).is_some_and(|p| crate::hotkey::Prefs::from_profile(p).active());
                if hotkey && self.hotkey_window_of(guid).is_none() {
                    self.show_hotkey_window(guid, super::hotkey_ui::foreground());
                } else if self.gpu.is_none() || self.registry.order().is_empty() {
                    self.pending_windows.push(windows_ui::NewWindow::Profile(Some(guid.clone())));
                } else {
                    self.new_tab(Some(guid));
                    self.bring_forward();
                }
            }
            ItemKind::ChangeProfile(guid) => {
                if let Some(id) = self.focused_id() {
                    self.set_profile_on(id, guid);
                    self.refresh_panes();
                    self.bring_forward();
                }
            }
            ItemKind::Arrangement { name, in_tabs } => {
                self.restore_arrangement(name, *in_tabs);
                self.bring_forward();
            }
            ItemKind::Action(i) => {
                if let Some(b) = actions.get(*i) {
                    self.run_key_action(b, None, None, event_loop);
                    self.refresh_panes();
                }
                self.bring_forward();
            }
            ItemKind::Snippet(i) => {
                if let Some(guid) = snippet_guids.get(*i) {
                    self.send_snippet(guid, alt, event_loop);
                }
                self.bring_forward();
            }
            ItemKind::NamedMark { pane, mark } => {
                self.reveal_pane(*pane);
                let line = self.panes.get_mut(pane).and_then(|p| p.screen.marks_in_order().into_iter().find(|(_, m)| m.id == *mark).map(|(l, _)| l));
                if let Some(line) = line {
                    self.show_line(*pane, line, false);
                    self.mark_pos.insert(*pane, *mark);
                }
                self.redraw();
            }
            ItemKind::MenuItem(id) => {
                // On the next turn, after the palette is gone; the command runs only if it
                // still does (`run_command` skips a command owned by a later milestone).
                self.bring_forward();
                let sent = self.proxy.lock().map_err(|e| e.to_string()).and_then(|p| p.send_event(UserEvent::Menu(id.clone())).map_err(|e| e.to_string()));
                if let Err(e) = sent {
                    self.banners.set("open quickly", format!("Could not run the menu item: {e}"));
                }
            }
        }
    }
}
