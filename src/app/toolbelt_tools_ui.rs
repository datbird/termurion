//! The toolbelt's tools in the app: keys routed to a focused tool, and each tool's rows,
//! clicks, double-clicks, buttons and menus. The pure parts are `tool_list` (the widget) and
//! `toolbelt_tools` (each tool's rows and pick rules).

use std::collections::HashSet;
use std::ops::Range;

use super::close_ui::confirm;
use super::statusbar_content::MenuAt;
use super::toolbelt_ui::ToolState;
use super::*;
use crate::broadcast::Write;
use crate::mouse::Mods;
use crate::shell_history::Dir;
use crate::snippets::Snippet;
use crate::tool_list::{self, FieldOut, Focus, ListHit, ListKey, ListLayout, ListSpec, ToolButton};
use crate::toolbelt::ToolId;
use crate::toolbelt_tools::{self as tt, ProfileOpen};
use winit::keyboard::{Key as WKey, NamedKey};

/// Menu ids of the tools' own menus (row menus and Profiles' popup).
pub(super) const CTX_ID: &str = "toolbelt:ctx:";
/// Command History's and Recent Directories' Help (`ToolCommandHistoryView.m:316-325`,
/// `ToolDirectoriesView.m`; the first is http, as the source has it).
const COMMAND_HELP: &str = "http://iterm2.com/shell_integration.html";
const DIRS_HELP: &str = "https://iterm2.com/shell_integration.html";
/// Profiles' popup is as wide as the strip lets it be (`ToolProfiles.m:120-131`).
const POPUP_FILL: f32 = 10_000.0;

/// One undoable change in Snippets or Actions: the list as it was.
// Pushed and popped by Snippets and Actions (the next 9j task).
#[allow(dead_code)]
#[derive(Clone, Debug)]
pub(super) enum ToolUndo {
    Snippets(Vec<Snippet>),
    Actions(toml::Value),
}

/// What a tool shows now, behind its rows.
enum Shown {
    Profiles(Vec<tt::ProfileRow>),
    /// The paste history's values, oldest first.
    Paste(Vec<String>),
    Commands(Vec<tt::CommandRow>),
    Dirs(Vec<Dir>),
    Nothing,
}

impl Shown {
    fn len(&self) -> usize {
        match self {
            Shown::Profiles(v) => v.len(),
            Shown::Paste(v) => v.len(),
            Shown::Commands(v) => v.len(),
            Shown::Dirs(v) => v.len(),
            Shown::Nothing => 0,
        }
    }
}

/// Text sizes for the tools' lists, in pixels unless named a scale.
#[derive(Clone, Copy)]
struct ListMetrics {
    /// The toolbelt font against the terminal font.
    text_scale: f32,
    /// A row: the font's line plus a 2 point inset above and below (`NSTableView+iTerm.m:36-89`).
    row_h: f32,
    /// Profiles' tag line.
    second_h: f32,
    second_scale: f32,
}

/// The field's search rules: whether Esc clears the text first (NSSearchField's cancel does)
/// and whether Up and Down move the list (`iTermSearchField.m:40-66`: not in Profiles).
fn field_rules(tool: ToolId) -> (bool, bool) {
    (true, !matches!(tool, ToolId::Profiles))
}

/// A list that scrolls to its end when it changes, unless it has the keyboard
/// (`ToolPasteHistory.m`, `ToolCommandHistoryView.m:103`, `ToolDirectoriesView.m`).
fn sticks_to_end(tool: ToolId) -> bool {
    matches!(tool, ToolId::PasteHistory | ToolId::CommandHistory | ToolId::RecentDirectories)
}

fn now_ms() -> u128 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis())
}

fn unix(t: std::time::SystemTime) -> f64 {
    t.duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64())
}

fn is_modifier(key: &WKey) -> bool {
    matches!(key, WKey::Named(NamedKey::Shift | NamedKey::Control | NamedKey::Alt | NamedKey::Super | NamedKey::Meta | NamedKey::AltGraph))
}

/// The end of `text` that fits `max_w` (a field shows where the caret is).
fn fit_end(text: &str, max_w: f32, width: &dyn Fn(&str) -> f32) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut start = 0;
    while start < chars.len() && width(&chars[start..].iter().collect::<String>()) > max_w {
        start += 1;
    }
    chars[start..].iter().collect()
}

/// A key as a focused tool's field or list reads it (`find_ui::route_key`'s mapping).
/// Return keeps its modifiers (Profiles and Snippets read them); every other Ctrl or Alt chord
/// is None, except Ctrl+V (`Paste`) and AltGr typing, which arrives as Ctrl and Alt together.
pub(super) fn list_key(key: &WKey, text: Option<&str>, mods: Mods) -> Option<ListKey> {
    let chord = mods.ctrl || mods.alt;
    let printable = text.filter(|t| !t.is_empty() && !t.chars().any(char::is_control));
    let named = match key {
        WKey::Named(NamedKey::Enter) => return Some(ListKey::Enter),
        WKey::Named(NamedKey::Escape) => Some(ListKey::Escape),
        WKey::Named(NamedKey::Backspace) => Some(ListKey::Backspace),
        WKey::Named(NamedKey::Delete) => Some(ListKey::Delete),
        WKey::Named(NamedKey::ArrowUp) => Some(ListKey::Up),
        WKey::Named(NamedKey::ArrowDown) => Some(ListKey::Down),
        WKey::Named(NamedKey::ArrowLeft) => Some(ListKey::Left),
        WKey::Named(NamedKey::ArrowRight) => Some(ListKey::Right),
        WKey::Named(NamedKey::Tab) => Some(ListKey::Tab),
        _ => None,
    };
    if let Some(k) = named {
        return (!chord).then_some(k);
    }
    match key {
        WKey::Character(c) if mods.ctrl && !mods.alt && c.eq_ignore_ascii_case("v") => Some(ListKey::Paste),
        _ if mods.ctrl == mods.alt => printable.map(|t| ListKey::Text(t.to_string())),
        _ => None,
    }
}

impl App {
    fn tool_state(&mut self, tool: ToolId) -> &mut ToolState {
        self.belt.tools.entry(tool).or_default()
    }

    fn tool_field_text(&self, tool: ToolId) -> String {
        self.belt.tools.get(&tool).map(|t| t.field.text.clone()).unwrap_or_default()
    }

    /// What a tool draws: its field, its buttons and its popup.
    pub(super) fn tool_spec(&self, tool: ToolId) -> ListSpec {
        match tool {
            // The popup, then Open at its right (`ToolProfiles.m:51-89,120-131`).
            ToolId::Profiles => ListSpec { field: true, right: vec![ToolButton::Open], popup_w: POPUP_FILL, ..ListSpec::default() },
            // Clear at the bottom right, no field, no help (`ToolPasteHistory.m:160-165`).
            ToolId::PasteHistory => ListSpec { right: vec![ToolButton::Trash], ..ListSpec::default() },
            // The field, Clear, then Help (`ToolCommandHistoryView.m:128-155`).
            ToolId::CommandHistory | ToolId::RecentDirectories => ListSpec { field: true, top: vec![ToolButton::Trash, ToolButton::Help], ..ListSpec::default() },
            ToolId::Snippets | ToolId::Actions | ToolId::NamedMarks => ListSpec::default(),
        }
    }

    fn list_metrics(&self) -> ListMetrics {
        let scale = self.scale();
        let (_, ch) = self.cell();
        let (px, font_px) = self.gpu.as_ref().map_or((11.0 * 4.0 / 3.0 * scale, ch / 1.2), |g| (g.renderer.toolbelt_px(scale), g.renderer.font_px()));
        let text_scale = px / font_px.max(1.0);
        let line = ch * text_scale;
        // The tag font is 2 points smaller, 3 above the small system size (`PLV:574-582`).
        let pt = px * 3.0 / 4.0 / scale;
        let second_scale = ((pt - if pt <= 11.0 { 2.0 } else { 3.0 }) / pt).max(0.1);
        ListMetrics { text_scale, row_h: line + 2.0 * tool_list_inset(scale), second_h: line * second_scale, second_scale }
    }

    /// The Profiles tool's rows: every saved profile, the command only when it is not the login
    /// shell (`ProfileModel.m:281-284`).
    fn profile_shown(&self, query: &str) -> Vec<tt::ProfileRow> {
        let list: Vec<(String, String, Vec<String>, String)> = self
            .store
            .profiles
            .iter()
            .map(|p| {
                let tags = p.get("Tags").and_then(|v| v.as_array().map(|a| a.iter().filter_map(|t| t.as_str().map(str::to_string)).collect::<Vec<_>>())).unwrap_or_default();
                let command = if p.string("Custom Command") == "No" { String::new() } else { p.string("Command") };
                (p.guid(), p.name(), tags, command)
            })
            .collect();
        tt::profile_rows(&list, &self.store.default_guid, query)
    }

    /// Command History's rows for the focused session's host, filtered.
    fn command_shown(&mut self, query: &str) -> Vec<tt::CommandRow> {
        let Some(pane) = self.focused_id() else { return Vec::new() };
        let host = self.history_host_pub(pane);
        let mine: HashSet<(String, u64)> = self.panes.get(&pane).map(|p| p.own().shell.history.iter().map(|r| (r.command.trim().to_string(), unix(r.start).to_bits())).collect()).unwrap_or_default();
        let uses = self.shell_history_pub().uses(&host);
        let (_, month, day, ..) = crate::prompt_ui::local_time();
        let rows = tt::command_rows(&uses, &|c: &str, t: f64| mine.contains(&(c.to_string(), t.to_bits())), month == 4 && day == 1);
        let keep = tt::command_filter(&rows, query);
        keep.into_iter().map(|i| rows[i].clone()).collect()
    }

    /// Recent Directories' rows for the focused session's host, best last, filtered.
    fn dir_shown(&mut self, query: &str) -> Vec<Dir> {
        let Some(pane) = self.focused_id() else { return Vec::new() };
        let host = self.history_host_pub(pane);
        let dirs = tt::dir_rows(&self.shell_history_pub().dirs(&host));
        tt::dir_filter(&dirs, query).into_iter().map(|i| dirs[i].clone()).collect()
    }

    fn tool_shown(&mut self, tool: ToolId) -> Shown {
        let query = self.tool_field_text(tool);
        match tool {
            ToolId::Profiles => Shown::Profiles(self.profile_shown(&query)),
            ToolId::PasteHistory => Shown::Paste(self.history().entries.iter().map(|e| e.value.clone()).collect()),
            ToolId::CommandHistory => Shown::Commands(self.command_shown(&query)),
            ToolId::RecentDirectories => Shown::Dirs(self.dir_shown(&query)),
            ToolId::Snippets | ToolId::Actions | ToolId::NamedMarks => Shown::Nothing,
        }
    }

    fn row_heights(shown: &Shown, m: ListMetrics) -> Vec<f32> {
        match shown {
            Shown::Profiles(rows) => rows.iter().map(|r| if r.tags.is_empty() { m.row_h } else { m.row_h + m.second_h }).collect(),
            other => vec![m.row_h; other.len()],
        }
    }

    /// A tool's rows as drawn, `width` pixels wide, from what it shows. Only rows in `visible`
    /// are measured and fitted (tail truncated; directories abbreviated first); the rest keep
    /// their whole text and are never drawn (a long Command History would otherwise be
    /// measured every frame).
    fn tool_rows(&mut self, shown: &Shown, width: f32, visible: Range<usize>) -> Vec<scene::ListRow> {
        let scale = self.scale();
        let m = self.list_metrics();
        let avail = (width - 2.0 * tool_list_inset(scale)).max(0.0);
        let plain = |text: String, bold: bool| scene::ListRow { text, second: None, bold, icon: None, depth: 0 };
        let mut rows: Vec<scene::ListRow> = match shown {
            Shown::Profiles(v) => v.iter().map(|r| scene::ListRow { second: (!r.tags.is_empty()).then(|| r.tags.clone()), ..plain(r.name.clone(), false) }).collect(),
            Shown::Paste(v) => tt::paste_rows(&v.iter().map(String::as_str).collect::<Vec<_>>()).into_iter().map(|t| plain(t, false)).collect(),
            Shown::Commands(v) => v.iter().map(|r| plain(r.text.clone(), r.bold)).collect(),
            Shown::Dirs(v) => v.iter().map(|d| plain(d.path.clone(), false)).collect(),
            Shown::Nothing => Vec::new(),
        };
        let tree = match shown {
            Shown::Dirs(_) => {
                let mut tree = tt::DirTree::default();
                for p in self.shell_history_pub().all_dir_paths() {
                    tree.add(&p);
                }
                Some(tree)
            }
            _ => None,
        };
        let Some(g) = self.gpu.as_mut() else { return rows };
        let renderer = std::cell::RefCell::new(&mut g.renderer);
        let width = |s: &str| renderer.borrow_mut().toolbelt_width(s, scale);
        let second_width = |s: &str| renderer.borrow_mut().text_width(s, true, m.text_scale * m.second_scale);
        for i in visible.start.min(rows.len())..visible.end.min(rows.len()) {
            if let (Shown::Dirs(v), Some(tree)) = (shown, tree.as_ref()) {
                let d = &v[i];
                rows[i].text = tt::dir_text(&d.path, d.starred, &tree.safe(&d.path), avail, &width);
            }
            let r = &mut rows[i];
            r.text = tool_list::truncate_tail(&r.text, avail, &width);
            if let Some(second) = r.second.take() {
                r.second = Some(tool_list::truncate_tail(&second, avail, &second_width));
            }
        }
        rows
    }

    /// The content rectangle of `tool`'s wrapper, while it shows and is not collapsed.
    fn tool_area(&self, tool: ToolId) -> Option<Rect> {
        let l = self.belt_layout()?;
        l.wrappers.iter().zip(&self.belt.split.tools).find(|(_, t)| **t == tool).and_then(|(w, _)| w.content)
    }

    /// The tool's list laid out in `area`, its scroll kept in range (and at the end for a list
    /// that changed without the keyboard), and its rows fitted where they show.
    fn tool_layout(&mut self, tool: ToolId, area: Rect) -> (ListLayout, Vec<scene::ListRow>, Shown) {
        let scale = self.scale();
        let m = self.list_metrics();
        let spec = self.tool_spec(tool);
        let shown = self.tool_shown(tool);
        let heights = Self::row_heights(&shown, m);
        let n = shown.len();
        let focused_list = self.belt.focus == Some((tool, Focus::List));
        let st = self.tool_state(tool);
        st.selection.retain(n);
        if sticks_to_end(tool) && st.seen != n && !focused_list {
            st.scroll = f32::MAX;
        }
        st.seen = n;
        let mut scroll = st.scroll;
        let mut l = tool_list::layout(area, &spec, &heights, 0.0, scale);
        scroll = tool_list::clamp_scroll(scroll, &l);
        if scroll != 0.0 {
            l = tool_list::layout(area, &spec, &heights, scroll, scale);
        }
        self.tool_state(tool).scroll = scroll;
        let list = l.list;
        let first = l.rows.iter().position(|r| r.y + r.h > list.y).unwrap_or(n);
        let last = l.rows.iter().rposition(|r| r.y < list.y + list.h).map_or(first, |i| i + 1);
        let rows = self.tool_rows(&shown, list.w, first..last.max(first));
        (l, rows, shown)
    }

    /// Draws each shown tool's list into its wrapper.
    pub(super) fn draw_tools(&mut self, frame: &mut Frame, cell: (f32, f32)) {
        let Some(bl) = self.belt_layout() else { return };
        let scale = self.scale();
        let dark = self.dark();
        let m = self.list_metrics();
        let areas: Vec<(ToolId, Rect)> = bl.wrappers.iter().zip(self.belt.split.tools.clone()).filter_map(|(w, t)| w.content.map(|c| (t, c))).collect();
        for (tool, area) in areas {
            let (l, rows, shown) = self.tool_layout(tool, area);
            let st = self.belt.tools.get(&tool).cloned().unwrap_or_default();
            let focus = self.belt.focus.filter(|(t, _)| *t == tool).map(|(_, f)| f);
            let field = match (l.field, self.gpu.as_mut()) {
                (Some(r), Some(g)) => {
                    let renderer = std::cell::RefCell::new(&mut g.renderer);
                    let w = |s: &str| renderer.borrow_mut().toolbelt_width(s, scale);
                    let shown_text = fit_end(&st.field.text, (r.w - 8.0 * scale).max(0.0), &w);
                    let width = w(&shown_text);
                    Some((shown_text, width))
                }
                _ => None,
            };
            let none = st.selection.rows.is_empty();
            let disabled: Vec<ToolButton> = match tool {
                ToolId::Profiles if none => vec![ToolButton::Open],
                _ => Vec::new(),
            };
            let popup = matches!(shown, Shown::Profiles(_)).then(|| (tt::PROFILE_OPENS[st.popup.min(tt::PROFILE_OPENS.len() - 1)].1, !none));
            let d = scene::ListDraw {
                layout: &l,
                rows: &rows,
                selected: &st.selection.rows,
                dark,
                field: field.as_ref().map(|(t, w)| (t.as_str(), *w)),
                focus,
                disabled: &disabled,
                popup,
                second_scale: m.second_scale,
            };
            scene::tool_list(frame, &d, cell, m.text_scale, scale);
        }
    }

    /// A press in `tool`'s content.
    pub(super) fn tool_press(&mut self, tool: ToolId, area: Rect, button: MouseButton, x: f32, y: f32, event_loop: &ActiveEventLoop) {
        let (l, _, _) = self.tool_layout(tool, area);
        let hit = tool_list::hit(&l, x, y);
        self.tool_click(tool, hit, button, event_loop);
    }

    /// A click on part of a tool. A left click in the field or the list takes the keyboard;
    /// a double-click on a row (the Windows double-click time) activates it; a right click on a
    /// row shows that row's menu.
    pub(super) fn tool_click(&mut self, tool: ToolId, hit: ListHit, button: MouseButton, event_loop: &ActiveEventLoop) {
        let Mods { ctrl, shift, .. } = self.mods;
        match (hit, button) {
            (ListHit::Field, MouseButton::Left) => self.belt.focus = Some((tool, Focus::Field)),
            (ListHit::Row(i), MouseButton::Left) => {
                self.belt.focus = Some((tool, Focus::List));
                let multi = tool == ToolId::Profiles;
                let double_ms = super::clippings_ui::double_click_ms();
                let sel = &mut self.tool_state(tool).selection;
                let double = sel.click(i, ctrl, shift, multi, now_ms(), double_ms);
                // Ctrl+double-click toggled the row out again; it still opens it (a split).
                if double && !sel.rows.contains(&i) {
                    sel.rows.push(i);
                    sel.rows.sort_unstable();
                }
                self.tool_selected(tool);
                if double {
                    self.tool_activate(tool, event_loop);
                }
            }
            (ListHit::Row(i), MouseButton::Right) => self.tool_row_menu(tool, i),
            (ListHit::Empty, MouseButton::Left) => {
                self.belt.focus = Some((tool, Focus::List));
                self.tool_state(tool).selection.rows.clear();
            }
            (ListHit::Button(b), MouseButton::Left) => self.tool_button(tool, b, event_loop),
            (ListHit::Popup, MouseButton::Left) => self.tool_popup_menu(tool),
            _ => {}
        }
        self.redraw();
    }

    /// A selection moved: Command History scrolls the focused session to that command's mark
    /// (`ToolCommandHistoryView.m:247-257`).
    fn tool_selected(&mut self, tool: ToolId) {
        if tool != ToolId::CommandHistory {
            return;
        }
        let Some(row) = self.belt.tools.get(&tool).and_then(|t| t.selection.rows.first().copied()) else { return };
        let Shown::Commands(rows) = self.tool_shown(tool) else { return };
        let (Some(r), Some(pane)) = (rows.get(row), self.focused_id()) else { return };
        let line = self.panes.get_mut(&pane).and_then(|p| p.screen.marks_in_order().into_iter().find(|(_, m)| m.command.trim() == r.command && m.start.map(unix) == Some(r.time)).map(|(l, _)| l));
        if let Some(line) = line {
            self.show_line(pane, line, false);
        }
    }

    /// A double-click, or Return: Profiles opens every selected profile by the modifiers
    /// (`ToolProfiles.m:190-202`); Paste History pastes the entry (`ToolPasteHistory.m:260-275`);
    /// Command History and Recent Directories type theirs (`ToolCommandHistoryView.m:294-314`,
    /// `ToolDirectoriesView.m:261-280`). The keyboard goes back to the session.
    pub(super) fn tool_activate(&mut self, tool: ToolId, event_loop: &ActiveEventLoop) {
        let _ = event_loop;
        let Mods { ctrl, alt, shift, .. } = self.mods;
        let rows = self.belt.tools.get(&tool).map(|t| t.selection.rows.clone()).unwrap_or_default();
        let Some(&first) = rows.first() else { return };
        let shown = self.tool_shown(tool);
        let pane = self.focused_id();
        match shown {
            Shown::Profiles(_) => self.open_profiles(tt::profile_open(ctrl, alt, shift)),
            Shown::Paste(values) => {
                let (Some(v), Some(pane)) = (values.get(first), pane) else { return };
                if let Some(cb) = &mut self.clipboard {
                    let _ = cb.set_text(v.clone());
                }
                self.user_paste(pane, v, Write::Paste);
            }
            Shown::Commands(v) => {
                let (Some(r), Some(pane)) = (v.get(first), pane) else { return };
                if let Some(text) = tt::command_insert(&r.command, r.dir.as_deref(), alt, shift) {
                    self.user_write(pane, text.as_bytes(), Write::SendText);
                }
            }
            Shown::Dirs(v) => {
                let (Some(d), Some(pane)) = (v.get(first), pane) else { return };
                let shell = self.panes.get(&pane).and_then(|p| p.own().shell.shell.clone());
                let text = tt::dir_insert(&d.path, shell.as_deref(), alt, shift);
                self.user_write(pane, text.as_bytes(), Write::SendText);
            }
            Shown::Nothing => return,
        }
        self.belt.focus = None;
        self.redraw();
    }

    /// Opens every selected profile as `how` says (`ToolProfiles.m:120-188`).
    fn open_profiles(&mut self, how: ProfileOpen) {
        let rows = self.belt.tools.get(&ToolId::Profiles).map(|t| t.selection.rows.clone()).unwrap_or_default();
        let query = self.tool_field_text(ToolId::Profiles);
        let shown = self.profile_shown(&query);
        let guids: Vec<String> = rows.iter().filter_map(|&i| shown.get(i).map(|r| r.guid.clone())).collect();
        for guid in guids {
            match how {
                ProfileOpen::Tab => self.new_tab(Some(&guid)),
                ProfileOpen::Window => self.pending_windows.push(windows_ui::NewWindow::Profile(Some(guid))),
                ProfileOpen::HorizontalSplit => self.split(Axis::Column, Some(&guid)),
                ProfileOpen::VerticalSplit => self.split(Axis::Row, Some(&guid)),
            }
        }
        self.belt.focus = None;
        self.redraw();
    }

    fn tool_button(&mut self, tool: ToolId, b: ToolButton, event_loop: &ActiveEventLoop) {
        let _ = event_loop;
        match (tool, b) {
            (ToolId::Profiles, ToolButton::Open) => {
                if self.belt.tools.get(&tool).is_some_and(|t| !t.selection.rows.is_empty()) {
                    let pick = self.tool_state(tool).popup.min(tt::PROFILE_OPENS.len() - 1);
                    self.open_profiles(tt::PROFILE_OPENS[pick].0);
                }
            }
            (ToolId::PasteHistory, ToolButton::Trash) => {
                if confirm(self.hwnd(), tt::ERASE_PASTE.0, tt::ERASE_PASTE.1) {
                    self.history().clear();
                    let path = super::history_ui::history_path();
                    if let Err(e) = std::fs::remove_file(&path) {
                        if e.kind() != std::io::ErrorKind::NotFound {
                            self.banners.set("history", format!("Could not delete the paste history: {e}"));
                        }
                    }
                    self.tool_state(tool).selection.rows.clear();
                }
            }
            (ToolId::CommandHistory, ToolButton::Trash) => {
                if confirm(self.hwnd(), tt::ERASE_COMMANDS.0, tt::ERASE_COMMANDS.1) {
                    self.shell_history_pub().erase_commands();
                    self.save_shell_history();
                    self.tool_state(tool).selection.rows.clear();
                }
            }
            (ToolId::RecentDirectories, ToolButton::Trash) => {
                if confirm(self.hwnd(), tt::ERASE_DIRS.0, tt::ERASE_DIRS.1) {
                    self.shell_history_pub().erase_dirs();
                    self.save_shell_history();
                    self.tool_state(tool).selection.rows.clear();
                }
            }
            (ToolId::CommandHistory | ToolId::RecentDirectories, ToolButton::Help) => {
                let url = if tool == ToolId::CommandHistory { COMMAND_HELP } else { DIRS_HELP };
                if let Some(pane) = self.focused_id() {
                    self.open_url(pane, url);
                }
            }
            _ => {}
        }
        self.redraw();
    }

    /// Profiles' popup, from its rectangle: the four ways to open, the last pick checked.
    fn tool_popup_menu(&mut self, tool: ToolId) {
        if tool != ToolId::Profiles || self.belt.tools.get(&tool).is_none_or(|t| t.selection.rows.is_empty()) {
            return;
        }
        let Some(area) = self.tool_area(tool) else { return };
        let (l, _, _) = self.tool_layout(tool, area);
        let Some(r) = l.popup else { return };
        let pick = self.tool_state(tool).popup;
        let menu = muda::Menu::new();
        for (i, (_, label)) in tt::PROFILE_OPENS.iter().enumerate() {
            if let Err(e) = menu.append(&muda::CheckMenuItem::with_id(format!("{CTX_ID}open:{i}"), *label, true, i == pick, None)) {
                self.banners.set("menu", format!("Profiles menu: {e}"));
            }
        }
        self.show_menu_at(menu, MenuAt { x: r.x, y: r.y + r.h, above: false });
    }

    /// A row's menu: Paste History's Copy and Open in Advanced Paste
    /// (`ToolPasteHistory.m:96-144`), Recent Directories' Toggle Star, on the clicked row.
    fn tool_row_menu(&mut self, tool: ToolId, row: usize) {
        let items: Vec<(String, &str)> = match self.tool_shown(tool) {
            Shown::Paste(v) if row < v.len() => vec![(format!("{CTX_ID}paste-copy:{row}"), "Copy"), (format!("{CTX_ID}paste-advanced:{row}"), "Open in Advanced Paste")],
            Shown::Dirs(v) => match v.get(row) {
                Some(d) => vec![(format!("{CTX_ID}star:{}", d.path), "Toggle Star")],
                None => return,
            },
            _ => return,
        };
        let menu = muda::Menu::new();
        for (id, label) in items {
            if let Err(e) = menu.append(&muda::MenuItem::with_id(id, label, true, None)) {
                self.banners.set("menu", format!("Toolbelt menu: {e}"));
            }
        }
        let (x, y) = self.cursor;
        self.show_menu_at(menu, MenuAt { x, y, above: false });
    }

    /// A pick in a tool's own menu (`CTX_ID` stripped). False when it is not one.
    pub(super) fn tool_menu_picked(&mut self, id: &str, event_loop: &ActiveEventLoop) -> bool {
        let Some(rest) = id.strip_prefix(CTX_ID) else { return false };
        let (what, arg) = rest.split_once(':').unwrap_or((rest, ""));
        match what {
            // Choosing opens at once, and Open repeats it (the popup's items carry the
            // actions, `ToolProfiles.m:65-80`).
            "open" => {
                if let Ok(i) = arg.parse::<usize>() {
                    if i < tt::PROFILE_OPENS.len() {
                        self.tool_state(ToolId::Profiles).popup = i;
                        self.open_profiles(tt::PROFILE_OPENS[i].0);
                    }
                }
            }
            "paste-copy" | "paste-advanced" => {
                let value = arg.parse::<usize>().ok().and_then(|i| self.history().entries.get(i).map(|e| e.value.clone()));
                let Some(v) = value else { return true };
                if let Some(cb) = &mut self.clipboard {
                    let _ = cb.set_text(v.clone());
                }
                if what == "paste-copy" {
                    self.remember_paste(&v);
                } else if let Some(pane) = self.focused_id() {
                    self.open_advanced_paste(event_loop, pane, None);
                }
            }
            "star" => {
                if let Some(pane) = self.focused_id() {
                    let host = self.history_host_pub(pane);
                    self.shell_history_pub().toggle_star(&host, arg);
                    self.save_shell_history();
                }
            }
            _ => {}
        }
        self.redraw();
        true
    }

    /// Keys while a tool's field or list has the keyboard (`tool_list::takes_key`): nothing
    /// taken reaches the session, releases and modifier keys alone included. Chords with Ctrl,
    /// Alt or Win go on as usual, except Ctrl+V into a field, Return with its modifiers, and
    /// AltGr typing.
    pub(super) fn toolbelt_key(&mut self, ev: &KeyEvent, event_loop: &ActiveEventLoop) -> bool {
        let Some((tool, focus)) = self.belt.focus else { return false };
        if !self.belt.visible || !self.belt.split.tools.contains(&tool) {
            self.belt.focus = None;
            return false;
        }
        let mods = self.mods;
        let key = list_key(&ev.logical_key, ev.text.as_deref(), mods);
        let taken = if is_modifier(&ev.logical_key) {
            true
        } else {
            match &key {
                Some(ListKey::Enter | ListKey::Text(_)) => !mods.cmd,
                Some(k) => tool_list::takes_key(Some(focus), k, mods.ctrl, mods.alt, mods.cmd),
                None => !(mods.ctrl || mods.alt || mods.cmd),
            }
        };
        if !taken {
            return false;
        }
        if ev.state != ElementState::Pressed {
            return true;
        }
        let Some(key) = key else { return true };
        match focus {
            Focus::Field => self.tool_field_key(tool, key, event_loop),
            Focus::List => self.tool_list_key(tool, key, event_loop),
        }
        self.redraw();
        true
    }

    fn tool_field_key(&mut self, tool: ToolId, key: ListKey, event_loop: &ActiveEventLoop) {
        let (esc_clears, arrows) = field_rules(tool);
        let out = if key == ListKey::Paste {
            let text = self.clipboard.as_mut().and_then(|cb| cb.get_text().ok()).unwrap_or_default();
            let line = text.replace(['\r', '\n'], " ");
            if line.is_empty() {
                FieldOut::Kept
            } else {
                self.tool_state(tool).field.text.push_str(&line);
                FieldOut::Changed
            }
        } else {
            self.tool_state(tool).field.key(&key, esc_clears, arrows)
        };
        match out {
            FieldOut::Changed => {
                let n = self.tool_shown(tool).len();
                let st = self.tool_state(tool);
                st.selection.retain(n);
                // Each change selects the first row when none is (`PLV:1003-1008`).
                if tool == ToolId::Profiles {
                    st.selection.first_if_none(n);
                    st.scroll = 0.0;
                }
            }
            // Return opens when one row shows or one is selected (`PLV:982-990`).
            FieldOut::Enter if tool == ToolId::Profiles => {
                let n = self.tool_shown(tool).len();
                let st = self.tool_state(tool);
                if n == 1 || !st.selection.rows.is_empty() {
                    st.selection.first_if_none(n);
                    self.tool_activate(tool, event_loop);
                }
            }
            FieldOut::Enter | FieldOut::Kept => {}
            FieldOut::GiveBack => self.belt.focus = None,
            FieldOut::Up | FieldOut::Down => self.tool_step(tool, out == FieldOut::Down),
            FieldOut::ToList => self.belt.focus = Some((tool, Focus::List)),
        }
    }

    fn tool_list_key(&mut self, tool: ToolId, key: ListKey, event_loop: &ActiveEventLoop) {
        match key {
            ListKey::Up | ListKey::Down => self.tool_step(tool, key == ListKey::Down),
            ListKey::Enter if tool == ToolId::Profiles => self.tool_activate(tool, event_loop),
            ListKey::Tab if self.tool_spec(tool).field => self.belt.focus = Some((tool, Focus::Field)),
            ListKey::Escape => self.belt.focus = None,
            // Delete belongs to Snippets, Actions and Named Marks (the next 9j task).
            _ => {}
        }
    }

    /// Up or Down moves the selection one row and shows it.
    fn tool_step(&mut self, tool: ToolId, down: bool) {
        let n = self.tool_shown(tool).len();
        self.tool_state(tool).selection.step(down, n);
        self.tool_selected(tool);
        self.tool_reveal(tool);
    }

    /// Scrolls the tool's list so its first selected row shows.
    fn tool_reveal(&mut self, tool: ToolId) {
        let Some(area) = self.tool_area(tool) else { return };
        let (l, _, _) = self.tool_layout(tool, area);
        let Some(row) = self.belt.tools.get(&tool).and_then(|t| t.selection.rows.first().copied()) else { return };
        let Some(r) = l.rows.get(row) else { return };
        let st = self.tool_state(tool);
        if r.y < l.list.y {
            st.scroll -= l.list.y - r.y;
        } else if r.y + r.h > l.list.y + l.list.h {
            st.scroll += r.y + r.h - (l.list.y + l.list.h);
        }
        st.scroll = tool_list::clamp_scroll(st.scroll, &l);
    }

    /// The wheel over a tool's list scrolls it, `dy` lines.
    pub(super) fn tool_wheel(&mut self, dy: f32) {
        let (x, y) = self.cursor;
        let Some(bl) = self.belt_layout() else { return };
        let Some((tool, area)) = bl.wrappers.iter().zip(self.belt.split.tools.clone()).find_map(|(w, t)| w.content.filter(|c| c.contains(x, y)).map(|c| (t, c))) else { return };
        let row_h = self.list_metrics().row_h;
        let (l, _, _) = self.tool_layout(tool, area);
        let st = self.tool_state(tool);
        st.scroll = tool_list::clamp_scroll(st.scroll - dy * row_h, &l);
        self.redraw();
    }
}

/// A row's inset in pixels.
fn tool_list_inset(scale: f32) -> f32 {
    scene::LIST_INSET * scale
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(ctrl: bool, alt: bool, shift: bool) -> Mods {
        Mods { shift, alt, ctrl, cmd: false }
    }

    #[test]
    fn list_keys_map_like_the_find_bar() {
        let none = m(false, false, false);
        let named = |k: NamedKey| list_key(&WKey::Named(k), None, none);
        assert_eq!(named(NamedKey::Enter), Some(ListKey::Enter));
        assert_eq!(named(NamedKey::Escape), Some(ListKey::Escape));
        assert_eq!(named(NamedKey::Backspace), Some(ListKey::Backspace));
        assert_eq!(named(NamedKey::Delete), Some(ListKey::Delete));
        assert_eq!(named(NamedKey::ArrowUp), Some(ListKey::Up));
        assert_eq!(named(NamedKey::ArrowDown), Some(ListKey::Down));
        assert_eq!(named(NamedKey::ArrowLeft), Some(ListKey::Left));
        assert_eq!(named(NamedKey::ArrowRight), Some(ListKey::Right));
        assert_eq!(named(NamedKey::Tab), Some(ListKey::Tab));
        // Shift keeps a key what it is.
        assert_eq!(list_key(&WKey::Named(NamedKey::Tab), None, m(false, false, true)), Some(ListKey::Tab));

        // Printable text, Space included.
        assert_eq!(list_key(&WKey::Character("a".into()), Some("a"), none), Some(ListKey::Text("a".into())));
        assert_eq!(list_key(&WKey::Character("A".into()), Some("A"), m(false, false, true)), Some(ListKey::Text("A".into())));
        assert_eq!(list_key(&WKey::Named(NamedKey::Space), Some(" "), none), Some(ListKey::Text(" ".into())));

        // Ctrl+V pastes.
        assert_eq!(list_key(&WKey::Character("v".into()), Some("\u{16}"), m(true, false, false)), Some(ListKey::Paste));

        // Ctrl+Tab and other Ctrl or Alt chords are not the tool's.
        assert_eq!(list_key(&WKey::Named(NamedKey::Tab), None, m(true, false, false)), None);
        assert_eq!(list_key(&WKey::Character("c".into()), Some("\u{3}"), m(true, false, false)), None);
        assert_eq!(list_key(&WKey::Character("x".into()), Some("x"), m(false, true, false)), None);
        assert_eq!(list_key(&WKey::Named(NamedKey::ArrowUp), None, m(true, false, false)), None);
        assert_eq!(list_key(&WKey::Named(NamedKey::Backspace), None, m(false, true, false)), None);

        // AltGr typing (Ctrl and Alt together) still types.
        assert_eq!(list_key(&WKey::Character("@".into()), Some("@"), m(true, true, false)), Some(ListKey::Text("@".into())));

        // Return keeps its modifiers: Profiles and Snippets read them.
        assert_eq!(list_key(&WKey::Named(NamedKey::Enter), None, m(true, false, false)), Some(ListKey::Enter));
        assert_eq!(list_key(&WKey::Named(NamedKey::Enter), None, m(false, true, false)), Some(ListKey::Enter));

        // Keys no tool reads.
        assert_eq!(named(NamedKey::F1), None);
        assert_eq!(named(NamedKey::Shift), None);
    }
}
