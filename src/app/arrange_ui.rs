//! Window arrangements in the window: saving (Window > Arrangements), restoring, and the
//! Duplicate Window that rebuilds a window from its own snapshot.

use super::*;
use crate::arrangement::{self, ProfileChoice, Value, WindowFacts};
use crate::arrangements::{self, Store as Arrangements};
use crate::prompt_ui::{PromptResult, PromptWindow};

/// Lines of a session's contents kept in a file "with contents" (iTerm2's
/// maxHistoryLinesToRestore default).
const CONTENT_LINES: usize = 20_000;

/// What a snapshot keeps of each session's text: nothing, plain text (a file saved "with
/// contents"), or styled text capped at N lines (session restoration).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Contents {
    None,
    Plain,
    Styled(usize),
    /// No text, but the host session (restoration with contents off).
    KeysOnly,
}

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
    fn session_value(&self, id: PaneId, contents: Contents) -> Value {
        let Some(p) = self.panes.get(&id) else { return Value::Table(Default::default()) };
        let s = p.own();
        let grid = s.term().grid();
        let profile = self.store.get(&p.profile).map(|pr| pr.values.clone().into_iter().collect()).unwrap_or_default();
        // OSC 7 reports "/C:/x"; the arrangement keeps a folder a shell can start in.
        let cwd = s.shell.cwd.as_deref().map(arrangement::saved_folder);
        let mut v = arrangement::session(grid.columns() as u16, grid.screen_lines() as u16, profile, &id.to_string(), cwd.as_deref());
        let saved = match contents {
            Contents::None => None,
            Contents::Plain => Some((arrangement::CONTENTS_KEY, crate::restore_state::cap_lines(&s.all_text(), CONTENT_LINES).to_string())),
            Contents::Styled(n) => {
                let (text, (row, col)) = crate::styled_copy::sgr_restorable(s);
                if let Some(t) = v.as_table_mut() {
                    let mut c = toml::Table::new();
                    c.insert("Row".into(), Value::Integer(row as i64));
                    c.insert("Column".into(), Value::Integer(col as i64));
                    t.insert(arrangement::CURSOR_KEY.into(), Value::Table(c));
                }
                Some((arrangement::STYLED_KEY, crate::restore_state::cap_lines(&text, n).to_string()))
            }
            Contents::KeysOnly => None,
        };
        if let (Some((key, text)), Some(t)) = (saved, v.as_table_mut()) {
            t.insert(key.into(), Value::String(text));
        }
        if let Some(t) = v.as_table_mut() {
            t.extend(self.clippings_table(id));
        }
        // Session restoration: the host session, so a later launch can take the shell back.
        if let (Contents::Styled(_) | Contents::KeysOnly, Some((host_id, offset)), Some(pid), Some(t)) = (contents, p.host_session(), p.shell_pid(), v.as_table_mut()) {
            t.insert(crate::restore_state::HOST_KEY.into(), super::restore_ui::host_key_value(host_id, pid, offset));
            // The shell's cookie, so it still works when the shell is taken back (`PS:7395-7396`).
            // Decision (Task 6): saved only under this gate (session restoration of a hosted
            // pane, never a saved arrangement), the write-side twin of `attach_pane` re-adding a
            // cookie only for a shell it took back; a new shell in its place gets a fresh one.
            if let Some(c) = &p.cookie {
                t.insert(crate::session_env::REUSABLE_COOKIE_KEY.into(), Value::String(c.clone()));
            }
        }
        v
    }

    /// The current window as an arrangement window table. A loaded Instant Replay
    /// recording's pane is left out (iTerm2 never restores one); a tab or window with
    /// nothing else is left out whole (None).
    pub(super) fn snapshot_window(&self, contents: Contents) -> Option<Value> {
        let area = self.content_area();
        let kept: Vec<(usize, &Tab, crate::layout::Tree<PaneId>)> = self
            .tabs
            .iter()
            .enumerate()
            .filter_map(|(i, t)| t.layout.tree().retain(&mut |id| !self.panes.get(id).is_some_and(|p| p.dvr.read_only)).map(|tree| (i, t, tree)))
            .collect();
        if kept.is_empty() {
            return None;
        }
        // The selected tab, or the first kept one when it was a recording's.
        let selected = kept.iter().position(|(i, _, _)| *i == self.active).unwrap_or(0);
        let tabs: Vec<Value> = kept
            .iter()
            .enumerate()
            .map(|(n, (_, t, tree))| {
                let active = tree.leaves().iter().position(|p| **p == t.focused);
                let tree = tree.clone().map(&mut |id| self.session_value(id, contents));
                arrangement::tab(arrangement::node_with_active(&tree, area.w as f64, area.h as f64, active), n == selected, t.title.as_deref())
            })
            .collect();
        let (x, y, w, h) = self.gpu.as_ref().map_or((0.0, 0.0, 0.0, 0.0), |g| {
            let p = g.window.outer_position().unwrap_or_default();
            let s = g.window.outer_size();
            arrangement::saved_frame((p.x, p.y), (s.width, s.height), g.window.is_minimized().unwrap_or(false))
        });
        let first = kept[0].2.leaves().first().map(|p| **p);
        let profile = first.and_then(|id| self.panes.get(&id)).and_then(|p| self.store.get(&p.profile)).unwrap_or_else(|| self.store.default_profile());
        let facts = WindowFacts {
            x,
            y,
            w,
            h,
            style: self.win_style,
            selected_tab: selected,
            title: self.win_title.clone(),
            size_locked: self.win_locks.size,
            layout_locked: self.win_locks.layout,
            percent: (profile.float("Width Percentage"), profile.float("Height Percentage")),
            cells: (profile.int("Columns").clamp(1, 1000) as u16, profile.int("Rows").clamp(1, 1000) as u16),
        };
        let mut w = arrangement::window(facts, tabs);
        if let Some(t) = w.as_table_mut() {
            crate::toolbelt::write_arrangement(t, &self.toolbelt_arranged());
        }
        if let (Some(h), Some(t)) = (&self.hotkey, w.as_table_mut()) {
            t.insert(arrangement::HOTKEY_WINDOW_KEY.into(), Value::Boolean(true));
            t.insert(arrangement::HOTKEY_GUID_KEY.into(), Value::String(h.guid.clone()));
        }
        Some(w)
    }

    /// Every window, by number.
    pub(super) fn snapshot_all(&mut self, contents: Contents) -> Vec<Value> {
        let back = self.current_window();
        let ids: Vec<WindowId> = self.registry.order().into_iter().filter_map(|k| self.window_id_of_key(k)).collect();
        let mut out = Vec::new();
        for id in ids {
            if self.enter(id) {
                out.extend(self.snapshot_window(contents));
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
        if self.arrangements().get(name).is_some() && !yes_no(self.hwnd(), "Replace Existing Saved Window Arrangement?", "There is an existing saved window arrangement with this name. Would you like to replace it with the current arrangement?") {
            return;
        }
        self.store_arrangement(name, all);
    }

    /// Saves every window, or the current one, as arrangement `name`, replacing one of that
    /// name without asking (the Python API's path, `IC:493-523`).
    pub(super) fn store_arrangement(&mut self, name: &str, all: bool) {
        let windows = if all { self.snapshot_all(Contents::None) } else { self.snapshot_window(Contents::None).into_iter().collect() };
        let mut store = self.arrangements();
        store.save(name, windows);
        self.put_arrangements(&store);
    }

    fn save_arrangement_file(&mut self, name: &str, all: bool) {
        if let Some(w) = self.last_window {
            self.enter(w);
        }
        let windows = if all { self.snapshot_all(Contents::Plain) } else { self.snapshot_window(Contents::Plain).into_iter().collect() };
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

/// The bad folder announcement's id: iTerm2 reuses the missing-profile identifier
/// (`iTermNaggingControllerArrangementProfileMissingIdentifier`, `NC:281`).
pub(super) const BAD_FOLDER_BAR: &str = "ThisProfileNoLongerExists";
pub(super) const DONT_WARN_AGAIN: &str = "Don\u{2019}t Warn Again";
pub(super) const REPAIR: &str = "Repair";
pub(super) const BAD_FOLDER_BUTTONS: [&str; 2] = [DONT_WARN_AGAIN, REPAIR];
/// Don't Warn Again's setting (`ASM:780-781`).
pub(super) const BAD_FOLDER_KEY: &str = "NoSyncSuppressBadPWDInArrangementWarning";
/// Keeps "iTerm2": the bug was iTerm2's, and such arrangements come from iTerm2.
pub(super) const BROKEN_PROFILE: &str = "This arrangement\u{2019}s profile is missing. This could be due to a bug in iTerm2 version 3.5.7, which caused profiles to be corrupted in saved arrangements.";
pub(super) const BROKEN_PROFILE_BAR: &str = "ArrangementMissingProfile";
pub(super) const ASSIGN_PROFILE: &str = "Assign Profile";
pub(super) const BROKEN_PROFILE_BUTTONS: [&str; 1] = [ASSIGN_PROFILE];
pub(super) const SELECT_PROFILE: &str = "Select a profile to use for this session. Your selection will be saved back to the arrangement.";

pub(super) fn bad_folder_text(arrangement: &str, folder: &str) -> String {
    format!("The saved arrangement \u{201c}{arrangement}\u{201d} has a bad initial directory of \u{201c}{folder}\u{201d} for this session.")
}

/// Whether a restored session's folder is announced as bad: only for a named arrangement
/// (there is one to repair), never for a UNC folder (never probed), and only when it is
/// not a folder.
pub(super) fn check_folder(named: bool, folder: &str, is_dir: &dyn Fn(&std::path::Path) -> bool) -> bool {
    named && !folder.trim().is_empty() && !crate::remote_prefs::is_unc(folder) && !is_dir(std::path::Path::new(folder))
}

/// Menu ids of the Restore Window Arrangement submenus' entries.
pub(super) const RESTORE_ID: &str = "arrangement-restore:";
pub(super) const RESTORE_TABS_ID: &str = "arrangement-tabs:";

impl App {
    fn profile_list(&self) -> Vec<(String, String)> {
        self.store.profiles.iter().map(|p| (p.guid(), p.name())).collect()
    }

    /// The profile of an arrangement window's first session (for the window's size and
    /// style): the saved one, or for the arrangement's own copy the saved profile it was
    /// copied from, else None (the default).
    pub(super) fn first_arrangement_guid(&self, w: &Value) -> Option<String> {
        let tab = w.get("Tabs")?.as_array()?.first()?;
        let tree = arrangement::tree_of(tab.get("Root")?).ok()?;
        let first = tree.leaves().first().map(|v| (*v).clone())?;
        let saved = self.profile_list();
        match arrangement::choose_profile(&first, &saved) {
            ProfileChoice::Saved(g) => Some(g),
            ProfileChoice::Embedded(t) => t.get(crate::session_profile::ORIGINAL_GUID).and_then(|v| v.as_str()).filter(|g| saved.iter().any(|(s, _)| s == g)).map(str::to_string),
            ProfileChoice::Broken | ProfileChoice::Default => None,
        }
    }

    /// A session copy holding an arrangement's own profile (iTerm2's `needDivorce`): the
    /// saved profile it was copied from, or the default, with the copy's values on top.
    /// Its Guid.
    fn arrangement_copy(&mut self, table: &toml::Table) -> String {
        let original = table.get(crate::session_profile::ORIGINAL_GUID).and_then(|v| v.as_str()).and_then(|g| self.store.profiles.iter().find(|p| p.guid() == g));
        let shared = original.unwrap_or_else(|| self.store.default_profile()).clone();
        let mut copy = crate::session_profile::divorce(&shared, uuid::Uuid::new_v4().to_string().to_uppercase());
        let values: Vec<(String, Option<Value>)> = table.iter().map(|(k, v)| (k.clone(), Some(v.clone()))).collect();
        copy.set(&values, &shared);
        let guid = copy.profile.guid();
        self.store.sessions.push(copy);
        guid
    }

    /// Builds one tab of an arrangement in the current window: a pane per session. `name`
    /// is the saved arrangement's (None for window restoration, a file, or Duplicate Window).
    fn build_tab(&mut self, t: &Value, name: Option<&str>) -> Result<Tab, String> {
        let root = t.get("Root").ok_or("a tab in the arrangement has no panes")?;
        let tree = arrangement::tree_of(root)?;
        let profiles = self.profile_list();
        let area = self.content_area();
        let mut started: Vec<PaneId> = Vec::new();
        let mut failed = None;
        let ids = tree.map(&mut |session| {
            if failed.is_some() {
                return 0;
            }
            let choice = arrangement::choose_profile(&session, &profiles);
            // A missing profile restores from the arrangement's own copy, even a broken one
            // (`PS:2019-2025`); only the saved profile is shared.
            let guid = match &choice {
                ProfileChoice::Saved(g) => Some(g.clone()),
                ProfileChoice::Embedded(copy) => Some(self.arrangement_copy(copy)),
                ProfileChoice::Broken => arrangement::profile_of(&session).map(|(_, _, copy)| copy.clone()).map(|copy| self.arrangement_copy(&copy)),
                ProfileChoice::Default => None,
            };
            let copied = guid.clone().filter(|_| !matches!(choice, ProfileChoice::Saved(_)));
            self.start_dir_override = Some(arrangement::folder_for(&session));
            let id = match super::restore_ui::attach_key(&session) {
                Some((host_id, pid, since)) => match self.attach_pane(area, guid.as_deref(), host_id, pid, since, super::restore_ui::saved_cookie(&session)) {
                    Ok(id) => Some(id),
                    Err(e) => {
                        self.banners.set("reattach", format!("A running shell could not be taken back ({e}); a new shell started in its place."));
                        self.start_pane(area, guid.as_deref())
                    }
                },
                None => self.start_pane(area, guid.as_deref()),
            };
            self.start_dir_override = None;
            match id {
                Some(id) => {
                    if let Some(p) = self.panes.get_mut(&id) {
                        if let Some(text) = arrangement::styled_contents(&session) {
                            // Session restoration: styled text, then the banner when no job came back.
                            let banner = session.get(arrangement::BANNER_KEY).and_then(|b| b.as_str());
                            let grays = crate::restore_state::banner_grays(p.screen.palette.bg);
                            p.screen.feed_restored(text, arrangement::saved_cursor(&session), banner.map(|b| (b, grays)));
                        } else if let Some(text) = arrangement::contents(&session) {
                            // The saved text comes back as the session's history, as plain text.
                            p.screen.feed_inert(&format!("{text}\n"));
                        }
                    }
                    self.restore_clippings(id, &session);
                    self.arrangement_announcements(id, &session, &choice, name);
                    started.push(id);
                    id
                }
                None => {
                    if let Some(g) = copied {
                        self.forget_session_profile(&g);
                    }
                    failed = Some(self.banners.text().unwrap_or_else(|| "a session could not start".to_string()));
                    0
                }
            }
        });
        if let Some(e) = failed {
            for id in started {
                self.clips.remove(&id);
                self.focus_order.remove(&id);
                self.bars.remove(&id);
                if let Some(p) = self.panes.remove(&id) {
                    if let Some(pid) = p.shell_pid() {
                        self.jobs.forget(pid);
                    }
                    self.forget_cookie(&p);
                    self.forget_session_profile(&p.profile);
                    self.pane_gone(id);
                }
            }
            return Err(e);
        }
        let layout = Layout::from_tree(ids);
        let panes = layout.panes();
        let focused = arrangement::active_leaf(root).and_then(|i| panes.get(i).copied()).unwrap_or(panes[0]);
        let tab = Tab { id: self.next_tab_id, layout, focused, zoomed: None, title: t.get("Title Override").and_then(|v| v.as_str()).map(str::to_string) };
        self.next_tab_id += 1;
        Ok(tab)
    }

    /// A restored pane's announcements: a broken profile offers Assign Profile
    /// (`PS:1960-1969`); a named arrangement's session remembers where it came from and
    /// offers Repair when its folder is gone (`PS:3561-3570`).
    fn arrangement_announcements(&mut self, pane: PaneId, session: &Value, choice: &ProfileChoice, name: Option<&str>) {
        let from = name.zip(arrangement::session_guid(session)).map(|(n, g)| (n.to_string(), g));
        if let Some(p) = self.panes.get_mut(&pane) {
            p.arrangement = from.clone();
        }
        if *choice == ProfileChoice::Broken {
            self.ask(pane, BROKEN_PROFILE_BAR, BROKEN_PROFILE, &BROKEN_PROFILE_BUTTONS, true);
        }
        let Some((name, _)) = from else { return };
        let folder = session.get("Working Directory").and_then(|v| v.as_str()).unwrap_or_default();
        if !self.globals.bool(BAD_FOLDER_KEY) && check_folder(true, folder, &|p| p.is_dir()) {
            self.ask(pane, BAD_FOLDER_BAR, &bad_folder_text(&name, folder), &BAD_FOLDER_BUTTONS, false);
        }
    }

    /// The arrangement announcements' answers. False for other bars.
    pub(super) fn arrangement_bar(&mut self, pane: PaneId, id: &str, button: Option<&str>, event_loop: &ActiveEventLoop) -> bool {
        if id != BAD_FOLDER_BAR && id != BROKEN_PROFILE_BAR {
            return false;
        }
        let from = self.panes.get(&pane).and_then(|p| p.arrangement.clone());
        match (id, button, from) {
            (BAD_FOLDER_BAR, Some(DONT_WARN_AGAIN), _) => {
                self.globals.set(BAD_FOLDER_KEY, Value::Boolean(true));
                self.save_globals();
            }
            // A folder is picked and written back to that session in that arrangement
            // (`PS:24041-24057`).
            (BAD_FOLDER_BAR, Some(REPAIR), Some((name, session))) => {
                if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                    let dir = dir.to_string_lossy().into_owned();
                    self.repair_arrangement(&name, &|w| arrangement::replace_folder(w, &session, &dir));
                }
            }
            (BROKEN_PROFILE_BAR, Some(ASSIGN_PROFILE), Some((name, session))) => self.open_assign_profile(event_loop, pane, name, session),
            _ => {}
        }
        true
    }

    /// Assign Profile's list of the saved profiles (`PS:24071-24101`).
    fn open_assign_profile(&mut self, event_loop: &ActiveEventLoop, pane: PaneId, name: String, session: String) {
        let tags = |p: &crate::profile::Profile| p.values.get("Tags").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|t| t.as_str()).collect::<Vec<_>>().join(", ")).unwrap_or_default();
        let rows: Vec<(String, String)> = self.store.profiles.iter().map(|p| (p.name(), tags(p))).collect();
        let guids: Vec<String> = self.store.profiles.iter().map(|p| p.guid()).collect();
        self.open_list_popup_for(event_loop, pane, SELECT_PROFILE, ["Profile Name", "Tags"], rows, guids, super::shell_history_ui::PopupFor::AssignProfile { arrangement: name, session });
    }

    /// A pick from Assign Profile: the saved profile goes into that session of the
    /// arrangement; the running pane keeps its profile (`PS:24104-24114`).
    pub(super) fn assign_arrangement_profile(&mut self, name: &str, session: &str, guid: &str) {
        let Some(profile) = self.store.profiles.iter().find(|p| p.guid() == guid) else { return };
        let table: toml::Table = profile.values.clone().into_iter().collect();
        self.repair_arrangement(name, &|w| arrangement::replace_profile(w, session, &table));
    }

    /// Rewrites a saved arrangement and saves it; nothing when it is gone.
    fn repair_arrangement(&mut self, name: &str, repair: &dyn Fn(&[Value]) -> Vec<Value>) {
        let mut store = self.arrangements();
        let Some(windows) = store.get(name) else { return };
        let repaired = repair(windows);
        store.replace(name, repaired);
        self.put_arrangements(&store);
    }

    /// Fills the current (new) window from an arrangement window table: its tabs, selected
    /// tab, title, locks, style, and frame.
    pub(super) fn apply_arrangement_window(&mut self, w: &Value, name: Option<&str>) -> Result<(), String> {
        let tabs = w.get("Tabs").and_then(|t| t.as_array()).cloned().unwrap_or_default();
        for t in &tabs {
            let tab = self.build_tab(t, name)?;
            self.tabs.push(tab);
        }
        let f = arrangement::window_facts(w);
        self.active = f.selected_tab.min(self.tabs.len().saturating_sub(1));
        self.win_title = f.title;
        self.win_locks = crate::winmodel::Locks { size: f.size_locked, layout: f.layout_locked && !f.size_locked };
        // restoreWindowsWithinScreens: a normal window moves back onto the displays of today.
        let mut frame = crate::placement::Rect { x: f.x as f32, y: f.y as f32, w: f.w as f32, h: f.h as f32 };
        if f.style == crate::frames::NORMAL && self.globals.bool("restoreWindowsWithinScreens") {
            let monitors: Vec<crate::placement::Rect> = super::requests_ui::work_areas().into_iter().map(|(x, y, w, h)| crate::placement::Rect { x: x as f32, y: y as f32, w: w as f32, h: h as f32 }).collect();
            frame = crate::placement::within_screens(frame, &crate::placement::primary_first(monitors));
        }
        if let Some(g) = &self.gpu {
            if frame.w > 0.0 && frame.h > 0.0 {
                g.window.set_outer_position(winit::dpi::PhysicalPosition::new(frame.x.round() as i32, frame.y.round() as i32));
                let (outer, inner) = (g.window.outer_size(), g.window.inner_size());
                let (dw, dh) = (outer.width.saturating_sub(inner.width), outer.height.saturating_sub(inner.height));
                let _ = g.window.request_inner_size(winit::dpi::PhysicalSize::new((frame.w as u32).saturating_sub(dw).max(1), (frame.h as u32).saturating_sub(dh).max(1)));
            }
            g.window.set_resizable(!self.win_locks.size);
        }
        if f.style != crate::frames::NORMAL {
            self.apply_window_style(f.style, false);
        }
        self.restore_toolbelt(&crate::toolbelt::read_arrangement(w));
        self.relayout();
        Ok(())
    }

    /// Window > Arrangements > Restore Window Arrangement (each saved window a new window)
    /// or Restore Window Arrangement as Tabs (every saved tab in the current window; callers
    /// enter the window first).
    pub(super) fn restore_arrangement(&mut self, name: &str, as_tabs: bool) {
        let Some(windows) = self.arrangements().get(name).cloned() else { return };
        // A hotkey window belongs to its hotkey, not to a restored arrangement.
        let windows: Vec<Value> = windows.into_iter().filter(|w| arrangement::hotkey_guid(w).is_none()).collect();
        if as_tabs {
            for w in &windows {
                for t in w.get("Tabs").and_then(|t| t.as_array()).cloned().unwrap_or_default() {
                    match self.build_tab(&t, Some(name)) {
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
                self.pending_windows.push(super::windows_ui::NewWindow::Arrangement(w, Some(name.to_string())));
            }
        }
        self.redraw();
    }

    /// Makes the window used last current (where a menu pick restores as tabs).
    pub(super) fn enter_last_window(&mut self) {
        if let Some(w) = self.last_window {
            self.enter(w);
        }
    }

    /// The Restore submenus list every arrangement (the default marked).
    pub(super) fn refresh_arrangement_menus(&mut self) {
        // The store changes only through a save or a settings edit, both of which mark the
        // menus dirty; a window's bar is filled once when it is new.
        if self.menu_bar.as_ref().is_none_or(|b| b.arrangements_filled) && !self.menus_dirty {
            return;
        }
        let store = self.arrangements();
        let default = store.default_name().map(str::to_string);
        let items: Vec<(String, bool)> = store.names().iter().map(|n| (n.to_string(), Some(n.to_string()) == default)).collect();
        let Some(bar) = &mut self.menu_bar else { return };
        bar.arrangements_filled = true;
        if bar.arrangement_items == items {
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
            self.enter_last_window();
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
            self.pending_windows.push(super::windows_ui::NewWindow::Arrangement(w, None));
        }
        self.banners.clear("arrangement");
        self.redraw();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The arrangement announcements' texts, buttons and ids (`NC:115-129`, `NC:266-298`,
    /// `PS:24071`) and when a folder is checked (`PS:3561-3570`).
    #[test]
    fn bad_folder_follows_iterm2() {
        assert_eq!(bad_folder_text("Work", r"D:\gone"), "The saved arrangement \u{201c}Work\u{201d} has a bad initial directory of \u{201c}D:\\gone\u{201d} for this session.");
        assert_eq!(BAD_FOLDER_BUTTONS, [DONT_WARN_AGAIN, REPAIR]);
        assert_eq!((DONT_WARN_AGAIN, REPAIR), ("Don\u{2019}t Warn Again", "Repair"));
        assert_eq!(BAD_FOLDER_BAR, "ThisProfileNoLongerExists", "iTerm2 reuses the missing-profile identifier");
        assert_eq!(BROKEN_PROFILE, "This arrangement\u{2019}s profile is missing. This could be due to a bug in iTerm2 version 3.5.7, which caused profiles to be corrupted in saved arrangements.");
        assert_eq!(BROKEN_PROFILE_BAR, "ArrangementMissingProfile");
        assert_eq!(BROKEN_PROFILE_BUTTONS, [ASSIGN_PROFILE]);
        assert_eq!(ASSIGN_PROFILE, "Assign Profile");
        assert_eq!(SELECT_PROFILE, "Select a profile to use for this session. Your selection will be saved back to the arrangement.");
        assert_eq!(BAD_FOLDER_KEY, "NoSyncSuppressBadPWDInArrangementWarning");
        let none = |_: &std::path::Path| false;
        let probed = std::cell::Cell::new(false);
        let watch = |_: &std::path::Path| {
            probed.set(true);
            false
        };
        assert!(check_folder(true, r"D:\gone", &none), "a named arrangement whose folder is missing");
        assert!(!check_folder(true, r"C:\here", &|_: &std::path::Path| true), "a folder that exists");
        assert!(!check_folder(false, r"D:\gone", &none), "window restoration has no arrangement to repair");
        assert!(!check_folder(true, "", &none), "no folder");
        assert!(!check_folder(true, r"\\host\share\x", &watch) && !probed.get(), "a UNC folder is never probed");
        assert!(!check_folder(true, "//host/share", &watch) && !probed.get());
    }
}
