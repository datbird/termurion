//! Window arrangements in the window: saving (Window > Arrangements), restoring, and the
//! Duplicate Window that rebuilds a window from its own snapshot.

use super::*;
use crate::arrangement::{self, Value, WindowFacts};
use crate::arrangements::{self, Store as Arrangements};
use crate::prompt_ui::{PromptResult, PromptWindow};

/// Lines of a session's contents kept in a file "with contents" (iTerm2's
/// maxHistoryLinesToRestore default).
const CONTENT_LINES: usize = 20_000;

impl App {
    pub(super) fn arrangements(&self) -> Arrangements {
        Arrangements::from_values(self.globals.get(arrangements::ARRANGEMENTS_KEY).as_ref(), self.globals.get(arrangements::DEFAULT_KEY).as_ref())
    }

    pub(super) fn put_arrangements(&mut self, s: &Arrangements) {
        let (all, default) = s.to_values();
        self.globals.set(arrangements::ARRANGEMENTS_KEY, all.unwrap_or_else(|| Value::Table(Default::default())));
        self.globals.set(arrangements::DEFAULT_KEY, default.unwrap_or_else(|| Value::String(String::new())));
        self.save_globals();
        self.menus_dirty = true;
        self.sync_checks();
    }

    /// One session's table.
    fn session_value(&self, id: PaneId, with_contents: bool) -> Value {
        let Some(p) = self.panes.get(&id) else { return Value::Table(Default::default()) };
        let s = p.own();
        let grid = s.term().grid();
        let profile = self.store.get(&p.profile).map(|pr| pr.values.clone().into_iter().collect()).unwrap_or_default();
        let mut v = arrangement::session(grid.columns() as u16, grid.screen_lines() as u16, profile, &id.to_string(), s.shell.cwd.as_deref());
        if with_contents {
            let text = s.all_text();
            let lines: Vec<&str> = text.lines().collect();
            let keep = lines[lines.len().saturating_sub(CONTENT_LINES)..].join("\n");
            if let Some(t) = v.as_table_mut() {
                t.insert("Tandem Contents".into(), Value::String(keep));
            }
        }
        v
    }

    /// The current window as an arrangement window table.
    pub(super) fn snapshot_window(&self, with_contents: bool) -> Value {
        let area = self.content_area();
        let tabs: Vec<Value> = self
            .tabs
            .iter()
            .enumerate()
            .map(|(i, t)| {
                let tree = t.layout.tree().map(&mut |id| self.session_value(id, with_contents));
                arrangement::tab(arrangement::node(&tree, area.w as f64, area.h as f64), i == self.active, t.title.as_deref())
            })
            .collect();
        let (pos, size) = self.gpu.as_ref().map_or(((0, 0), (0, 0)), |g| {
            let p = g.window.outer_position().unwrap_or_default();
            let s = g.window.outer_size();
            ((p.x, p.y), (s.width, s.height))
        });
        let profile = self.tabs.first().and_then(|t| self.panes.get(&t.focused)).and_then(|p| self.store.get(&p.profile)).unwrap_or_else(|| self.store.default_profile());
        let facts = WindowFacts {
            x: pos.0 as f64,
            y: pos.1 as f64,
            w: size.0 as f64,
            h: size.1 as f64,
            style: self.win_style,
            selected_tab: self.active,
            title: self.win_title.clone(),
            size_locked: self.win_locks.size,
            layout_locked: self.win_locks.layout,
            percent: (profile.float("Width Percentage"), profile.float("Height Percentage")),
            cells: (profile.int("Columns").clamp(1, 1000) as u16, profile.int("Rows").clamp(1, 1000) as u16),
        };
        arrangement::window(facts, tabs)
    }

    /// Every window, by number.
    pub(super) fn snapshot_all(&mut self, with_contents: bool) -> Vec<Value> {
        let back = self.current_window();
        let ids: Vec<WindowId> = self.registry.order().into_iter().filter_map(|k| self.window_id_of_key(k)).collect();
        let mut out = Vec::new();
        for id in ids {
            if self.enter(id) {
                out.push(self.snapshot_window(with_contents));
            }
        }
        if let Some(b) = back {
            self.enter(b);
        }
        out
    }

    /// Window > Arrangements > Save Window Arrangement (all windows) or Save Current Window
    /// as Arrangement.
    pub(super) fn open_save_arrangement(&mut self, event_loop: &ActiveEventLoop, all: bool) {
        if let Some((w, _)) = &self.arrange_prompt {
            return w.focus();
        }
        let taken: Vec<String> = self.arrangements().names().iter().map(|s| s.to_string()).collect();
        let (date, time, secs) = arrangement::date_names(crate::prompt_ui::local_time());
        let name = arrangements::unique_name(&date, &time, &secs, &taken);
        let Some(g) = self.gpu.as_ref() else { return };
        match PromptWindow::open(event_loop, &g.instance, &g.adapter, &g.device, &g.queue, "Save Window Arrangement", "Name for saved window arrangement:", &name) {
            Ok(w) => self.arrange_prompt = Some((w.with_extra("Save to File with Contents...").refuse_blank(), all)),
            Err(e) => self.banners.set("window", format!("Could not open the window: {e}")),
        }
    }

    /// Events for the save window. False when `window` is not it.
    pub(super) fn arrange_prompt_event(&mut self, window: WindowId, event: &WindowEvent) -> bool {
        let Some((w, all)) = self.arrange_prompt.as_mut() else { return false };
        if w.id() != window {
            return false;
        }
        let all = *all;
        match event {
            WindowEvent::CloseRequested => self.arrange_prompt = None,
            WindowEvent::RedrawRequested => match w.redraw() {
                PromptResult::Open => {}
                PromptResult::Cancel => self.arrange_prompt = None,
                PromptResult::Ok(name) => {
                    self.arrange_prompt = None;
                    self.save_arrangement(&arrangements::clean_name(&name), all);
                }
                PromptResult::Extra(name) => {
                    self.arrange_prompt = None;
                    self.save_arrangement_file(&arrangements::clean_name(&name), all);
                }
            },
            other => {
                w.event(other);
            }
        }
        true
    }

    fn save_arrangement(&mut self, name: &str, all: bool) {
        if name.is_empty() {
            return;
        }
        if let Some(w) = self.last_window {
            self.enter(w);
        }
        let mut store = self.arrangements();
        if store.get(name).is_some() && !yes_no(self.hwnd(), "Replace Existing Saved Window Arrangement?", "There is an existing saved window arrangement with this name. Would you like to replace it with the current arrangement?") {
            return;
        }
        let windows = if all { self.snapshot_all(false) } else { vec![self.snapshot_window(false)] };
        store.save(name, windows);
        self.put_arrangements(&store);
    }

    fn save_arrangement_file(&mut self, name: &str, all: bool) {
        if let Some(w) = self.last_window {
            self.enter(w);
        }
        let windows = if all { self.snapshot_all(true) } else { vec![self.snapshot_window(true)] };
        let Some(path) = rfd::FileDialog::new().set_title("Save Arrangement with Contents").set_file_name(format!("{name}.itermarrangement")).add_filter("Arrangement", &["itermarrangement", "iterm2arrangement"]).save_file() else { return };
        match arrangement::to_plist(&windows).and_then(|b| std::fs::write(&path, b).map_err(|e| e.to_string())) {
            Ok(()) => self.banners.clear("arrangement"),
            Err(e) => self.banners.set("arrangement", format!("Could not save {}: {e}", path.display())),
        }
        self.redraw();
    }
}

/// A Yes and No question; true for Yes.
pub(super) fn yes_no(owner: Option<isize>, title: &str, text: &str) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{IDYES, MB_ICONQUESTION, MB_YESNO, MessageBoxW};
    let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    let owner = owner.map_or(std::ptr::null_mut(), |h| h as *mut core::ffi::c_void);
    unsafe { MessageBoxW(owner, wide(text).as_ptr(), wide(title).as_ptr(), MB_YESNO | MB_ICONQUESTION) == IDYES }
}

/// Menu ids of the Restore Window Arrangement submenus' entries.
pub(super) const RESTORE_ID: &str = "arrangement-restore:";
pub(super) const RESTORE_TABS_ID: &str = "arrangement-tabs:";

impl App {
    fn profile_list(&self) -> Vec<(String, String)> {
        self.store.profiles.iter().map(|p| (p.guid(), p.name())).collect()
    }

    /// The profile of an arrangement window's first session (for the window's size and style).
    pub(super) fn first_arrangement_guid(&self, w: &Value) -> Option<String> {
        let tab = w.get("Tabs")?.as_array()?.first()?;
        let tree = arrangement::tree_of(tab.get("Root")?).ok()?;
        let first = tree.leaves().first().map(|v| (*v).clone())?;
        arrangement::resolve_profile(&first, &self.profile_list())
    }

    /// Builds one tab of an arrangement in the current window: a pane per session.
    fn build_tab(&mut self, t: &Value) -> Result<Tab, String> {
        let root = t.get("Root").ok_or("a tab in the arrangement has no panes")?;
        let tree = arrangement::tree_of(root)?;
        let profiles = self.profile_list();
        let area = self.content_area();
        let mut started: Vec<PaneId> = Vec::new();
        let mut failed = None;
        let mut missing = false;
        let ids = tree.map(&mut |session| {
            if failed.is_some() {
                return 0;
            }
            let guid = arrangement::resolve_profile(&session, &profiles);
            missing |= guid.is_none();
            self.start_dir_override = Some(arrangement::folder_for(&session));
            let id = self.start_pane(area, guid.as_deref());
            self.start_dir_override = None;
            match id {
                Some(id) => {
                    if let (Some(text), Some(p)) = (arrangement::contents(&session), self.panes.get_mut(&id)) {
                        // The saved text comes back as the session's history.
                        p.screen.feed(text.replace('\n', "\r\n").as_bytes());
                        p.screen.feed(b"\r\n");
                    }
                    started.push(id);
                    id
                }
                None => {
                    failed = Some(self.banners.text().unwrap_or_else(|| "a session could not start".to_string()));
                    0
                }
            }
        });
        if let Some(e) = failed {
            for id in started {
                self.panes.remove(&id);
            }
            return Err(e);
        }
        if missing {
            self.banners.set("arrangement", "A profile in the arrangement no longer exists; the default profile is used.");
        }
        let layout = Layout::from_tree(ids);
        let focused = layout.panes()[0];
        let tab = Tab { id: self.next_tab_id, layout, focused, zoomed: None, title: t.get("Title Override").and_then(|v| v.as_str()).map(str::to_string) };
        self.next_tab_id += 1;
        Ok(tab)
    }

    /// Fills the current (new) window from an arrangement window table: its tabs, selected
    /// tab, title, locks, style, and frame.
    pub(super) fn apply_arrangement_window(&mut self, w: &Value) -> Result<(), String> {
        let tabs = w.get("Tabs").and_then(|t| t.as_array()).cloned().unwrap_or_default();
        for t in &tabs {
            let tab = self.build_tab(t)?;
            self.tabs.push(tab);
        }
        let f = arrangement::window_facts(w);
        self.active = f.selected_tab.min(self.tabs.len().saturating_sub(1));
        self.win_title = f.title;
        self.win_locks = crate::winmodel::Locks { size: f.size_locked, layout: f.layout_locked && !f.size_locked };
        if let Some(g) = &self.gpu {
            if f.w > 0.0 && f.h > 0.0 {
                g.window.set_outer_position(winit::dpi::PhysicalPosition::new(f.x.round() as i32, f.y.round() as i32));
                let (outer, inner) = (g.window.outer_size(), g.window.inner_size());
                let (dw, dh) = (outer.width.saturating_sub(inner.width), outer.height.saturating_sub(inner.height));
                let _ = g.window.request_inner_size(winit::dpi::PhysicalSize::new((f.w as u32).saturating_sub(dw).max(1), (f.h as u32).saturating_sub(dh).max(1)));
            }
            g.window.set_resizable(!self.win_locks.size);
        }
        if f.style != crate::frames::NORMAL {
            self.apply_window_style(f.style, false);
        }
        self.relayout();
        Ok(())
    }

    /// Window > Arrangements > Restore Window Arrangement (each saved window a new window)
    /// or Restore Window Arrangement as Tabs (every saved tab in this window).
    pub(super) fn restore_arrangement(&mut self, name: &str, as_tabs: bool) {
        let Some(windows) = self.arrangements().get(name).cloned() else { return };
        if as_tabs {
            if let Some(w) = self.last_window {
                self.enter(w);
            }
            for w in &windows {
                for t in w.get("Tabs").and_then(|t| t.as_array()).cloned().unwrap_or_default() {
                    match self.build_tab(&t) {
                        Ok(tab) => {
                            self.tabs.push(tab);
                            self.active = self.tabs.len() - 1;
                        }
                        Err(e) => self.banners.set("arrangement", format!("Could not restore a tab: {e}")),
                    }
                }
            }
            self.relayout();
        } else {
            for w in windows {
                self.pending_windows.push(super::windows_ui::NewWindow::Arrangement(w));
            }
        }
        self.redraw();
    }

    /// The Restore submenus list every arrangement (the default marked).
    pub(super) fn refresh_arrangement_menus(&mut self) {
        let store = self.arrangements();
        let default = store.default_name().map(str::to_string);
        let items: Vec<(String, bool)> = store.names().iter().map(|n| (n.to_string(), Some(n.to_string()) == default)).collect();
        let Some(bar) = &mut self.menu_bar else { return };
        if bar.arrangement_items == items && !self.menus_dirty {
            return;
        }
        for (sub, prefix) in [(&bar.restore, RESTORE_ID), (&bar.restore_tabs, RESTORE_TABS_ID)] {
            let Some(sub) = sub else { continue };
            while sub.remove_at(0).is_some() {}
            for (name, is_default) in &items {
                let label = if *is_default { format!("{name} (default)") } else { name.clone() };
                let _ = sub.append(&muda::MenuItem::with_id(format!("{prefix}{name}"), label.replace('&', "&&"), true, None));
            }
        }
        bar.arrangement_items = items;
    }

    /// A pick from the Restore submenus. False for other ids.
    pub(super) fn arrangement_picked(&mut self, id: &str) -> bool {
        if let Some(name) = id.strip_prefix(RESTORE_ID) {
            self.restore_arrangement(name, false);
            return true;
        }
        if let Some(name) = id.strip_prefix(RESTORE_TABS_ID) {
            self.restore_arrangement(name, true);
            return true;
        }
        false
    }
}

impl App {
    /// Window > Arrangements > Load Arrangement from File...: each window in it opens; a
    /// downloaded file asks first.
    pub(super) fn load_arrangement_file(&mut self) {
        let Some(path) = rfd::FileDialog::new().set_title("Load Arrangement").add_filter("Arrangement", &["itermarrangement", "iterm2arrangement", "plist"]).pick_file() else { return };
        let windows = match std::fs::read(&path).map_err(|e| e.to_string()).and_then(|b| arrangement::from_plist(&b)) {
            Ok(w) => w,
            Err(e) => {
                self.banners.set("arrangement", format!("Could not load {}: {e}", path.display()));
                return self.redraw();
            }
        };
        if arrangement::has_mark_of_the_web(&path) {
            let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            let text = arrangement::trust_text(&name, &windows);
            if !yes_no(self.hwnd(), "Open Arrangement?", &text) {
                return;
            }
        }
        for w in windows {
            self.pending_windows.push(super::windows_ui::NewWindow::Arrangement(w));
        }
        self.banners.clear("arrangement");
        self.redraw();
    }
}
