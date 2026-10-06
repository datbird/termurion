//! The toolbelt's tools in the app: keys routed to a focused tool, and each tool's rows,
//! clicks, double-clicks, buttons and menus. The pure parts are `tool_list` (the widget) and
//! `toolbelt_tools` (each tool's rows and pick rules).

use std::collections::HashSet;
use std::hash::Hash;
use std::ops::Range;
use std::rc::Rc;

use super::close_ui::confirm;
use super::statusbar_content::MenuAt;
use super::toolbelt_more_ui::{self as more, CapturedShown, JobsShown, StatusShown};
use super::toolbelt_ui::{BeltDrag, ToolState};
use super::*;
use crate::broadcast::Write;
use crate::keyaction::{self, Binding};
use crate::menus::{Chord, ChordKey};
use crate::mouse::Mods;
use crate::shell_history::Dir;
use crate::snippets::{self, Snippet};
use crate::tool_list::{self, FieldOut, Focus, ListHit, ListKey, ListLayout, ListSpec, ToolButton};
use crate::toolbelt::ToolId;
use crate::toolbelt_tools::{self as tt, ProfileOpen, SnippetPick, SnippetRowKind};
use crate::user_actions;
use winit::keyboard::{Key as WKey, NamedKey};

/// Menu ids of the tools' own menus (row menus and Profiles' popup).
pub(super) const CTX_ID: &str = "toolbelt:ctx:";
/// Command History's and Recent Directories' Help (`ToolCommandHistoryView.m:316-325`,
/// `ToolDirectoriesView.m`; the first is http, as the source has it).
const COMMAND_HELP: &str = "http://iterm2.com/shell_integration.html";
const DIRS_HELP: &str = "https://iterm2.com/shell_integration.html";
/// Profiles' popup is as wide as the strip lets it be (`ToolProfiles.m:120-131`).
const POPUP_FILL: f32 = 10_000.0;
/// Snippets' glyphs (Segoe Fluent Icons) and colors (`TS:733-737`): a snippet in blue, a folder
/// in orange, and a folder's disclosure, closed and open.
const SNIPPET_GLYPH: char = '\u{E8BD}';
const FOLDER_GLYPH: char = '\u{E8B7}';
const SNIPPET_BLUE: crate::palette::Rgb = crate::palette::rgb(94, 159, 208);
const FOLDER_CLOSED: char = '\u{E76C}';
const FOLDER_OPEN: char = '\u{E70D}';
/// A tagged snippet's row is 1.8 rows tall (`TS:836-842`).
const TAGGED_ROW: f32 = 1.8;
/// Info bar id: Snippets' search help.
const SNIPPET_HELP_BAR: &str = "toolbelt-snippet-help";
/// A row press becomes a reorder once the pointer moved this far, points.
const DRAG_SLOP: f32 = 4.0;

/// One undoable change in Snippets or Actions: the list as it was.
#[derive(Clone, Debug)]
pub(super) enum ToolUndo {
    Snippets(Vec<Snippet>),
    Actions(toml::Value),
}

/// What a tool shows now, behind its rows. Kept per window (`Belt::shown`) until its
/// `ShownKey` changes.
pub(super) enum Shown {
    Profiles(Vec<tt::ProfileRow>),
    /// The paste history's values, oldest first.
    Paste(Vec<String>),
    Commands(Vec<tt::CommandRow>),
    /// The host's directories, and the abbreviation tree of every host's.
    Dirs(Vec<Dir>, tt::DirTree),
    /// Snippets' outline as this window opened its folders, each folder row's tag path, and the
    /// snippets the search let through (indexes into the snippets file).
    Snippets { rows: Vec<tt::SnippetRow>, paths: Vec<Option<String>>, filtered: Vec<usize> },
    Actions(Vec<String>),
    /// The focused session's named marks, top first: id and name.
    Marks(Vec<(u32, String)>),
    Captured(CapturedShown),
    Status(StatusShown),
    Jobs(JobsShown),
    /// Notes and Codecierge draw with their own scene calls, not the list: no rows.
    Notes,
    Codecierge(crate::codecierge::View),
}

impl Shown {
    fn len(&self) -> usize {
        match self {
            Shown::Profiles(v) => v.len(),
            Shown::Paste(v) => v.len(),
            Shown::Commands(v) => v.len(),
            Shown::Dirs(v, _) => v.len(),
            Shown::Snippets { rows, .. } => rows.len(),
            Shown::Actions(v) => v.len(),
            Shown::Marks(v) => v.len(),
            Shown::Captured(c) => c.entries.len(),
            Shown::Status(st) => st.items.len(),
            Shown::Jobs(j) => j.rows.len(),
            Shown::Notes | Shown::Codecierge(_) => 0,
        }
    }
}

/// What a tool's `Shown` was built from: its query, the focused session and its host,
/// `App::tool_gen` (bumped when the shell history, the paste history, the profiles, the
/// snippets or the actions change), for Command History whether it is April 1 (its
/// failed-command prefix changes at midnight), and a hash of what else the tool reads (the
/// focused session's live named marks, or its profile's Snippets Filter).
#[derive(Clone, Debug, PartialEq)]
pub(super) struct ShownKey {
    query: String,
    pane: Option<PaneId>,
    host: String,
    generation: u64,
    april_first: bool,
    extra: u64,
}

/// The keys whose press a focused tool took. A release is the tool's exactly when its press
/// was, whatever has the keyboard or which modifiers are held by then: an Esc that gave the
/// keyboard back still owns its release, and the C of a Ctrl+C that went to the session still
/// sends its release after Ctrl is let go first.
#[derive(Debug)]
pub(super) struct TakenKeys<K>(HashSet<K>);

impl<K> Default for TakenKeys<K> {
    fn default() -> Self {
        TakenKeys(HashSet::new())
    }
}

impl<K: Hash + Eq> TakenKeys<K> {
    /// A press (a repeat too), taken by the tool or not.
    pub(super) fn press(&mut self, key: K, taken: bool) {
        if taken {
            self.0.insert(key);
        } else {
            self.0.remove(&key);
        }
    }

    /// A release: true when the tool took its press.
    pub(super) fn release(&mut self, key: &K) -> bool {
        self.0.remove(key)
    }

    /// The window lost focus: its releases will not come.
    pub(super) fn clear(&mut self) {
        self.0.clear();
    }
}

/// Text sizes for the tools' lists, in pixels unless named a scale.
#[derive(Clone, Copy)]
pub(super) struct ListMetrics {
    /// The toolbelt font against the terminal font.
    pub(super) text_scale: f32,
    /// A row: the font's line plus a 2 point inset above and below (`NSTableView+iTerm.m:36-89`).
    pub(super) row_h: f32,
    /// The font's line (each further line of a wrapped row).
    pub(super) line: f32,
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

pub(super) fn now_ms() -> u128 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis())
}

pub(super) fn unix(t: std::time::SystemTime) -> f64 {
    t.duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64())
}

pub(super) fn is_modifier(key: &WKey) -> bool {
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
    pub(super) fn tool_state(&mut self, tool: ToolId) -> &mut ToolState {
        self.belt.tools.entry(tool).or_default()
    }

    pub(super) fn tool_field_text(&self, tool: ToolId) -> String {
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
            // The field and Help; Send and Open in Advanced Paste at the left; Edit, Remove, Add
            // at the right (`TS:311-348`).
            ToolId::Snippets => ListSpec { field: true, top: vec![ToolButton::Help], left: vec![ToolButton::Send, ToolButton::AdvancedPaste], right: vec![ToolButton::Add, ToolButton::Remove, ToolButton::Edit], ..ListSpec::default() },
            // Apply at the left; Edit, Remove, Add at the right (`iTermToolActions.m:117-137`).
            ToolId::Actions => ListSpec { left: vec![ToolButton::Apply], right: vec![ToolButton::Add, ToolButton::Remove, ToolButton::Edit], ..ListSpec::default() },
            // Edit, Remove, Add at the right (`ToolNamedMarks.swift:104-112`).
            ToolId::NamedMarks => ListSpec { right: vec![ToolButton::Add, ToolButton::Remove, ToolButton::Edit], ..ListSpec::default() },
            // The field, Clear, then Help (`CO:170-230`).
            ToolId::CapturedOutput => ListSpec { field: true, top: vec![ToolButton::Trash, ToolButton::Help], ..ListSpec::default() },
            // Bell and Snooze at the left; Help at the right, Settings left of it (`TS:246-275`).
            ToolId::SessionStatus => ListSpec { left: vec![ToolButton::Bell, ToolButton::Snooze], right: vec![ToolButton::Help, ToolButton::Settings], ..ListSpec::default() },
            // The signal popup at the left; Send, then Inspect at the right (`JT:336-356`,
            // `viewDidLayout`).
            ToolId::Jobs => ListSpec { right: vec![ToolButton::Inspect, ToolButton::Send], popup_w: more::SIGNAL_POPUP_W, ..ListSpec::default() },
            // Their own layouts (`more::note_layout`, Codecierge's onboarding view).
            ToolId::Notes | ToolId::Codecierge => ListSpec::default(),
        }
    }

    pub(super) fn list_metrics(&self) -> ListMetrics {
        let scale = self.scale();
        let (_, ch) = self.cell();
        let (px, font_px) = self.gpu.as_ref().map_or((11.0 * 4.0 / 3.0 * scale, ch / 1.2), |g| (g.renderer.toolbelt_px(scale), g.renderer.font_px()));
        let text_scale = px / font_px.max(1.0);
        let line = ch * text_scale;
        // The tag font is 2 points smaller, 3 above the small system size (`PLV:574-582`).
        let pt = px * 3.0 / 4.0 / scale;
        let second_scale = ((pt - if pt <= 11.0 { 2.0 } else { 3.0 }) / pt).max(0.1);
        ListMetrics { text_scale, row_h: line + 2.0 * tool_list_inset(scale), line, second_h: line * second_scale, second_scale }
    }

    /// The Profiles tool's rows: every saved profile, the command only when it is not the login
    /// shell (`ProfileModel.m:281-284`).
    fn profile_shown(&self, query: &str) -> Vec<tt::ProfileRow> {
        let list = crate::profile_list::rows(&self.store.profiles);
        tt::profile_rows(&list, &self.store.default_guid, query)
    }

    /// Command History's rows for the focused session's host, filtered.
    fn command_shown(&mut self, query: &str, april_first: bool) -> Vec<tt::CommandRow> {
        let Some(pane) = self.focused_id() else { return Vec::new() };
        let host = self.history_host_pub(pane);
        let mine: HashSet<(String, u64)> = self.panes.get(&pane).map(|p| p.own().shell.history.iter().map(|r| (r.command.trim().to_string(), unix(r.start).to_bits())).collect()).unwrap_or_default();
        let uses = self.shell_history_pub().uses(&host);
        let rows = tt::command_rows(&uses, &|c: &str, t: f64| mine.contains(&(c.to_string(), t.to_bits())), april_first);
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

    /// Snippets' rows: the search over every snippet with the focused session's Snippets Filter
    /// tags added (`TS:445-451`), as a tree with this window's open folders. The source decides
    /// each row's text by the snippet's own `searchMatches` (`TS:692-696`); a snippet carries
    /// them exactly when the search engine ran, which is when the field or the filter is not
    /// empty (`iTermSnippetsModel.m:635-658`: `phraseIndexes` is never nil), so that is
    /// `searching` for every row.
    fn snippet_shown(&self, query: &str) -> Shown {
        let tags = self.snippets_filter();
        let list = &self.snippets.snippets;
        let filtered = snippets::search(list, query, &tags);
        let searching = !query.is_empty() || !tags.is_empty();
        let tree = tt::snippet_tree(list, &filtered);
        let expanded = self.belt.tools.get(&ToolId::Snippets).map(|t| t.expanded.clone()).unwrap_or_default();
        let rows = tt::snippet_rows(list, &tree, &expanded, searching);
        // A folder row's path: the folders above it at each shallower depth, then its name.
        let paths = tt::snippet_row_keys(&rows).into_iter().map(|(path, snippet)| snippet.is_none().then_some(path)).collect();
        Shown::Snippets { rows, paths, filtered }
    }

    /// The focused session's named marks in buffer order (`namedMarkSort`).
    fn mark_shown(&mut self) -> Vec<(u32, String)> {
        let Some(p) = self.focused_id().and_then(|id| self.panes.get_mut(&id)) else { return Vec::new() };
        let named: Vec<(i32, crate::marks::Mark)> = p.screen.marks_in_order().into_iter().filter(|(_, m)| m.name.is_some()).collect();
        tt::mark_rows(&named)
    }

    /// What else `tool` reads, hashed for its `ShownKey`: the focused session's named marks
    /// (Named Marks) or its Snippets Filter (Snippets).
    fn shown_extra(&self, tool: ToolId, pane: Option<PaneId>) -> u64 {
        use std::hash::Hasher;
        let mut h = std::collections::hash_map::DefaultHasher::new();
        match tool {
            // The live rows, not the records: a mark cleared or trimmed from the scrollback
            // leaves the list at once. The scan runs only while Named Marks shows.
            ToolId::NamedMarks => {
                if let Some(p) = pane.and_then(|id| self.panes.get(&id)) {
                    p.screen.live_named_marks().hash(&mut h);
                }
            }
            ToolId::Snippets => self.snippets_filter().hash(&mut h),
            ToolId::CapturedOutput => self.captured_extra(&mut h),
            ToolId::SessionStatus => self.status_extra(&mut h),
            ToolId::Jobs => self.jobs_extra(&mut h),
            _ => {}
        }
        h.finish()
    }

    /// Something a tool shows changed (the shell history, the paste history, the profiles):
    /// every window's tools rebuild their rows on their next use.
    pub(super) fn tools_changed(&mut self) {
        self.tool_gen = self.tool_gen.wrapping_add(1);
    }

    /// What `tool` shows, rebuilt only when its `ShownKey` changed.
    pub(super) fn tool_shown(&mut self, tool: ToolId) -> Rc<Shown> {
        match tool {
            ToolId::CapturedOutput => self.captured_catch_up(),
            ToolId::SessionStatus => self.prune_snoozes(),
            _ => {}
        }
        let query = self.tool_field_text(tool);
        let pane = self.focused_id();
        let per_host = matches!(tool, ToolId::CommandHistory | ToolId::RecentDirectories);
        let host = match pane {
            Some(p) if per_host => self.history_host_pub(p),
            _ => String::new(),
        };
        let april_first = tool == ToolId::CommandHistory && {
            let (_, month, day, ..) = crate::prompt_ui::local_time();
            month == 4 && day == 1
        };
        // Codecierge's view reads the session and the gate, which `shown_extra` (`&self`) cannot.
        let cc_view = (tool == ToolId::Codecierge).then(|| self.codecierge_view());
        let extra = match &cc_view {
            Some(v) => {
                use std::hash::{Hash, Hasher};
                let mut h = std::collections::hash_map::DefaultHasher::new();
                v.hash(&mut h);
                h.finish()
            }
            None => self.shown_extra(tool, pane),
        };
        let key = ShownKey { query, pane, host, generation: self.tool_gen, april_first, extra };
        if let Some((k, shown)) = self.belt.shown.get(&tool) {
            if *k == key {
                return shown.clone();
            }
        }
        let query = key.query.clone();
        let shown = Rc::new(match tool {
            ToolId::Profiles => Shown::Profiles(self.profile_shown(&query)),
            ToolId::PasteHistory => Shown::Paste(self.history().entries.iter().map(|e| e.value.clone()).collect()),
            ToolId::CommandHistory => Shown::Commands(self.command_shown(&query, april_first)),
            ToolId::RecentDirectories => {
                let dirs = self.dir_shown(&query);
                let mut tree = tt::DirTree::default();
                for p in self.shell_history_pub().all_dir_paths() {
                    tree.add(&p);
                }
                Shown::Dirs(dirs, tree)
            }
            ToolId::Snippets => self.snippet_shown(&query),
            ToolId::Actions => Shown::Actions(tt::action_rows(&user_actions::load(self.globals.get("Actions").as_ref()))),
            ToolId::NamedMarks => Shown::Marks(self.mark_shown()),
            ToolId::CapturedOutput => self.captured_shown(&query),
            ToolId::SessionStatus => self.status_shown(),
            ToolId::Jobs => self.jobs_shown(),
            ToolId::Notes => Shown::Notes,
            ToolId::Codecierge => Shown::Codecierge(cc_view.unwrap_or_else(|| crate::codecierge::View::Onboarding(crate::codecierge::CodeciergeState::Ready))),
        });
        // Jobs' selection follows its processes, not its row numbers.
        if let Shown::Jobs(new) = &*shown {
            let old = self.belt.shown.get(&ToolId::Jobs).and_then(|(_, s)| match &**s {
                Shown::Jobs(j) => Some((j.shell, j.keys())),
                _ => None,
            });
            if let Some((shell, keys)) = old {
                more::follow_jobs(&mut self.tool_state(ToolId::Jobs).selection, (shell, &keys), (new.shell, &new.keys()));
            }
        }
        self.belt.shown.insert(tool, (key, shown.clone()));
        shown
    }

    fn row_heights(shown: &Shown, m: ListMetrics) -> Vec<f32> {
        match shown {
            Shown::Profiles(rows) => rows.iter().map(|r| if r.tags.is_empty() { m.row_h } else { m.row_h + m.second_h }).collect(),
            Shown::Snippets { rows, .. } => rows.iter().map(|r| if matches!(r.kind, SnippetRowKind::Snippet(_)) && !r.tags.is_empty() { m.row_h * TAGGED_ROW } else { m.row_h }).collect(),
            // Variable heights (`CO:267-314`, `ToolStatusCellView.fittingSize`): a line more for
            // each wrapped line, and for Session Status' status line when there is one.
            Shown::Captured(c) => c.wraps.borrow().lines.iter().map(|l| m.row_h + l.len().saturating_sub(1) as f32 * m.line).collect(),
            Shown::Status(st) => st.wraps.borrow().lines.iter().map(|l| m.row_h + l.iter().enumerate().filter(|(k, t)| *k > 0 || !t.is_empty()).count() as f32 * m.line).collect(),
            other => vec![m.row_h; other.len()],
        }
    }

    /// The rows in `visible` as drawn, `width` pixels wide, from what the tool shows: only
    /// those are built and fitted (tail truncated; directories abbreviated first). The first
    /// returned row is row `visible.start` (`ListDraw::first`).
    fn tool_rows(&mut self, shown: &Shown, width: f32, visible: Range<usize>) -> Vec<scene::ListRow> {
        let scale = self.scale();
        let dark = self.dark();
        let m = self.list_metrics();
        let avail = (width - 2.0 * tool_list_inset(scale)).max(0.0);
        let range = visible.start.min(shown.len())..visible.end.min(shown.len());
        let Some(g) = self.gpu.as_mut() else { return Vec::new() };
        let renderer = std::cell::RefCell::new(&mut g.renderer);
        let width = |s: &str| renderer.borrow_mut().toolbelt_width(s, scale);
        let second_width = |s: &str| renderer.borrow_mut().text_width(s, true, m.text_scale * m.second_scale);
        let plain = |text: String, bold: bool| scene::ListRow { text: tool_list::truncate_tail(&text, avail, &width), bold, ..scene::ListRow::default() };
        let glyph_w = (m.row_h - 2.0 * tool_list_inset(scale)) / 1.2;
        let expanded = self.belt.tools.get(&ToolId::Snippets).map(|t| t.expanded.clone()).unwrap_or_default();
        let collapsed = self.belt.tools.get(&ToolId::Jobs).map(|t| t.collapsed.clone()).unwrap_or_default();
        range
            .map(|i| match shown {
                Shown::Profiles(v) => {
                    let r = &v[i];
                    scene::ListRow { second: (!r.tags.is_empty()).then(|| tool_list::truncate_tail(&r.tags, avail, &second_width)), ..plain(r.name.clone(), false) }
                }
                Shown::Paste(v) => plain(tt::paste_rows(&[v[i].as_str()]).remove(0), false),
                Shown::Commands(v) => plain(v[i].text.clone(), v[i].bold),
                Shown::Dirs(v, tree) => {
                    let d = &v[i];
                    plain(tt::dir_text(&d.path, d.starred, &tree.safe(&d.path), avail, &width), false)
                }
                Shown::Snippets { rows, paths, .. } => {
                    let r = &rows[i];
                    let folder = paths[i].as_ref().map(|p| expanded.contains(p));
                    let disclosure = folder.map(|open| if open { FOLDER_OPEN } else { FOLDER_CLOSED });
                    let icon = if folder.is_some() { (FOLDER_GLYPH, scene::CHIP_BG) } else { (SNIPPET_GLYPH, SNIPPET_BLUE) };
                    let lead = r.depth as f32 * scene::LIST_INDENT * scale + glyph_w + 4.0 * scale + if disclosure.is_some() { glyph_w + 2.0 * scale } else { 0.0 };
                    let text = tool_list::truncate_tail(&r.text, (avail - lead).max(0.0), &width);
                    let chips = if folder.is_some() { Vec::new() } else { r.tags.iter().map(|t| format!(" {t} ")).map(|t| (t.clone(), width(&t) * scene::CHIP_SCALE)).collect() };
                    scene::ListRow { text, icon: Some(icon), depth: r.depth, disclosure, chips, ..scene::ListRow::default() }
                }
                Shown::Actions(v) => plain(v[i].clone(), false),
                Shown::Marks(v) => plain(v[i].1.clone(), false),
                Shown::Captured(c) => more::captured_list_row(c.wraps.borrow().lines.get(i).map_or(&[][..], Vec::as_slice)),
                Shown::Status(st) => more::status_list_row(&st.items[i], st.wraps.borrow().lines.get(i).map_or(&[][..], Vec::as_slice), st.separator == Some(i), avail, scale, dark, &width),
                Shown::Jobs(j) => more::job_list_row(&j.rows[i], collapsed.contains(&j.rows[i].pid), avail, glyph_w, scale, &width),
                Shown::Notes | Shown::Codecierge(_) => unreachable!("Notes and Codecierge have no rows"),
            })
            .collect()
    }

    /// The content rectangle of `tool`'s wrapper, while it shows and is not collapsed.
    pub(super) fn tool_area(&self, tool: ToolId) -> Option<Rect> {
        let l = self.belt_layout()?;
        l.wrappers.iter().zip(&self.belt.split.tools).find(|(_, t)| **t == tool).and_then(|(w, _)| w.content)
    }

    /// The tool's list laid out in `area`, its scroll kept in range (and at the end for a list
    /// that changed without the keyboard), and its rows fitted where they show.
    pub(super) fn tool_layout(&mut self, tool: ToolId, area: Rect) -> (ListLayout, Vec<scene::ListRow>, usize, Rc<Shown>) {
        let scale = self.scale();
        let m = self.list_metrics();
        let spec = self.tool_spec(tool);
        let shown = self.tool_shown(tool);
        self.wrap_rows(&shown, area.w);
        let heights = Self::row_heights(&shown, m);
        let n = shown.len();
        let focused_list = self.belt.focus == Some((tool, Focus::List));
        // Session Status' selection follows the focused session, without the keyboard
        // (`updateSelectionWithoutChangingFirstResponder`).
        let status_row = match &*shown {
            Shown::Status(st) => Some(self.focused_id().and_then(|f| st.items.iter().position(|i| i.row.pane == f))),
            _ => None,
        };
        let st = self.tool_state(tool);
        if let Some(row) = status_row {
            st.selection.rows = row.into_iter().collect();
        }
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
        (l, rows, first, shown)
    }

    /// Draws each shown tool's list into its wrapper.
    pub(super) fn draw_tools(&mut self, frame: &mut Frame, cell: (f32, f32)) {
        let Some(bl) = self.belt_layout() else { return };
        let scale = self.scale();
        let dark = self.dark();
        let m = self.list_metrics();
        let areas: Vec<(ToolId, Rect)> = bl.wrappers.iter().zip(self.belt.split.tools.clone()).filter_map(|(w, t)| w.content.map(|c| (t, c))).collect();
        self.jobs_want(areas.iter().any(|(t, _)| *t == ToolId::Jobs));
        for (tool, area) in areas {
            match tool {
                ToolId::Notes => {
                    self.draw_notes(frame, area, cell);
                    continue;
                }
                ToolId::Codecierge => {
                    self.draw_codecierge(frame, area, cell);
                    continue;
                }
                _ => {}
            }
            let (l, rows, first, shown) = self.tool_layout(tool, area);
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
            let count = selected_count(&shown, &st.selection.rows);
            let mut disabled: Vec<ToolButton> = [ToolButton::Open, ToolButton::Send, ToolButton::AdvancedPaste, ToolButton::Apply, ToolButton::Remove, ToolButton::Edit, ToolButton::Inspect].into_iter().filter(|b| !button_enabled(*b, count)).collect();
            let mut lit = Vec::new();
            match &*shown {
                // Clear with nothing showing (`CO:268`).
                Shown::Captured(c) if c.entries.is_empty() => disabled.push(ToolButton::Trash),
                // Snooze needs a row; it and the bell light when on (`TS:595-625,1239-1255`).
                Shown::Status(_) => {
                    let selected = self.status_selected();
                    match selected {
                        Some(p) if self.belt.snoozed.contains_key(&p) => lit.push(ToolButton::Snooze),
                        Some(_) => {}
                        None => disabled.push(ToolButton::Snooze),
                    }
                    if self.status_bell_lit() {
                        lit.push(ToolButton::Bell);
                    }
                }
                _ => {}
            }
            let drop_y = match self.drag {
                Drag::Belt(BeltDrag::Row { tool: t, start_y }) if t == tool && (self.cursor.1 - start_y).abs() >= DRAG_SLOP * scale => drop_slot(&shown, &l.rows, self.cursor.1).map(|slot| l.rows.get(slot).map_or_else(|| l.rows.last().map_or(l.list.y, |r| r.y + r.h), |r| r.y)),
                _ => None,
            };
            let popup = match &*shown {
                Shown::Profiles(_) => Some((tt::PROFILE_OPENS[st.popup.min(tt::PROFILE_OPENS.len() - 1)].1, !none)),
                // Enabled with a selection (`JT:573-578`).
                Shown::Jobs(_) => Some((tt::SIGNALS[st.signal.min(tt::SIGNALS.len() - 1)].name(), !none)),
                _ => None,
            };
            let d = scene::ListDraw {
                layout: &l,
                rows: &rows,
                first,
                selected: &st.selection.rows,
                dark,
                field: field.as_ref().map(|(t, w)| (t.as_str(), *w)),
                focus,
                disabled: &disabled,
                popup,
                second_scale: m.second_scale,
                drop_y,
                lit: &lit,
            };
            scene::tool_list(frame, &d, cell, m.text_scale, scale);
        }
    }

    /// A press in `tool`'s content.
    pub(super) fn tool_press(&mut self, tool: ToolId, area: Rect, button: MouseButton, x: f32, y: f32, event_loop: &ActiveEventLoop) {
        match tool {
            ToolId::Notes => return self.notes_press(area, button, x, y),
            ToolId::Codecierge => return self.codecierge_press(area, button, x, y),
            _ => {}
        }
        let (l, _, _, shown) = self.tool_layout(tool, area);
        let hit = tool_list::hit(&l, x, y);
        // A parent's disclosure hides or shows its subtree, and selects nothing (NSOutlineView).
        if let (ListHit::Row(i), MouseButton::Left, Shown::Jobs(j)) = (hit, button, &*shown) {
            if let (Some(r), Some(rect)) = (j.rows.get(i).filter(|r| r.has_children), l.rows.get(i)) {
                let scale = self.scale();
                let glyph_w = (self.list_metrics().row_h - 2.0 * tool_list_inset(scale)) / 1.2;
                let lead = if r.watched { scene::lead_width(&[(ToolButton::Bell.glyph(), scene::LIST_SELECTED)], scale) } else { 0.0 };
                let left = rect.x + tool_list_inset(scale) + r.depth as f32 * scene::LIST_INDENT * scale + lead;
                if x >= left && x < left + glyph_w + 2.0 * scale {
                    self.belt_take_focus(tool, Focus::List);
                    let pid = r.pid;
                    self.toggle_job(pid, None);
                    return self.redraw();
                }
            }
        }
        // A folder's disclosure glyph opens or closes it, and selects nothing (NSOutlineView).
        if let (ListHit::Row(i), MouseButton::Left, Shown::Snippets { rows, paths, .. }) = (hit, button, &*shown) {
            if let (Some(path), Some(r), Some(rect)) = (&paths[i], rows.get(i), l.rows.get(i)) {
                let scale = self.scale();
                let glyph_w = (self.list_metrics().row_h - 2.0 * tool_list_inset(scale)) / 1.2;
                let left = rect.x + tool_list_inset(scale) + r.depth as f32 * scene::LIST_INDENT * scale;
                if x < left + glyph_w + 2.0 * scale {
                    self.belt_take_focus(tool, Focus::List);
                    self.toggle_folder(path, None);
                    return self.redraw();
                }
            }
        }
        let activated = self.tool_click(tool, hit, button, event_loop);
        // A row pressed in Snippets or Actions may be dragged to a new place.
        if button == MouseButton::Left && !activated && matches!(hit, ListHit::Row(_)) && matches!(tool, ToolId::Snippets | ToolId::Actions) {
            self.drag = Drag::Belt(BeltDrag::Row { tool, start_y: y });
        }
    }

    /// Opens (`open` Some(true)), closes or toggles one of Snippets' folders in this window.
    /// The selection follows its items to their new rows.
    fn toggle_folder(&mut self, path: &str, open: Option<bool>) {
        let keys = |shown: &Shown| match shown {
            Shown::Snippets { rows, .. } => tt::snippet_row_keys(rows),
            _ => Vec::new(),
        };
        let before = keys(&self.tool_shown(ToolId::Snippets));
        let expanded = &mut self.tool_state(ToolId::Snippets).expanded;
        let now = expanded.contains(path);
        if open.unwrap_or(!now) {
            expanded.insert(path.to_string());
        } else {
            expanded.remove(path);
        }
        // The rows are built with the open folders.
        self.belt.shown.remove(&ToolId::Snippets);
        let after = keys(&self.tool_shown(ToolId::Snippets));
        self.tool_state(ToolId::Snippets).selection.remap(&before, &after);
    }

    /// A click on part of a tool. A left click in the field or the list takes the keyboard;
    /// a double-click on a row (the Windows double-click time) activates it; a right click on a
    /// row shows that row's menu. True when a double-click activated a row.
    pub(super) fn tool_click(&mut self, tool: ToolId, hit: ListHit, button: MouseButton, event_loop: &ActiveEventLoop) -> bool {
        let Mods { ctrl, shift, .. } = self.mods;
        let mut activated = false;
        match (hit, button) {
            (ListHit::Field, MouseButton::Left) => self.belt_take_focus(tool, Focus::Field),
            (ListHit::Row(i), MouseButton::Left) => {
                self.belt_take_focus(tool, Focus::List);
                // Multiple selection (`ToolProfiles.m`, `TS:147`, `iTermToolActions.m:83`,
                // `ToolNamedMarks.swift:49`); the history lists pick one.
                let multi = matches!(tool, ToolId::Profiles | ToolId::Snippets | ToolId::Actions | ToolId::NamedMarks | ToolId::Jobs);
                // Snippets' and Actions' rows drag (`tool_press`): a press on a selected row
                // keeps the others until the release (`tool_drop`).
                let draggable = matches!(tool, ToolId::Snippets | ToolId::Actions);
                let double_ms = super::clippings_ui::double_click_ms();
                let sel = &mut self.tool_state(tool).selection;
                let double = sel.press(i, ctrl, shift, multi, draggable, now_ms(), double_ms);
                // Ctrl+double-click toggled the row out again; it still opens it (a split).
                if double && !sel.rows.contains(&i) {
                    sel.rows.push(i);
                    sel.rows.sort_unstable();
                }
                self.tool_selected(tool);
                // Named Marks have no double-click (`ToolNamedMarks.swift`).
                if double && tool != ToolId::NamedMarks {
                    self.tool_activate(tool, event_loop);
                    activated = true;
                }
            }
            (ListHit::Row(i), MouseButton::Right) => self.tool_row_menu(tool, i),
            (ListHit::Empty, MouseButton::Left) => {
                self.belt_take_focus(tool, Focus::List);
                self.tool_state(tool).selection.rows.clear();
            }
            (ListHit::Button(b), MouseButton::Left) => self.tool_button(tool, b, event_loop),
            (ListHit::Popup, MouseButton::Left) => self.tool_popup_menu(tool),
            _ => {}
        }
        // Session Status never takes the keyboard (`keyboardNavigable: false`).
        if tool == ToolId::SessionStatus {
            self.belt.focus = None;
        }
        self.redraw();
        activated
    }

    /// A selection moved: Command History scrolls the focused session to that command's mark
    /// (`ToolCommandHistoryView.m:247-257`), and Named Marks to the mark
    /// (`ToolNamedMarks.swift:183-190`, `toolbeltDidSelectNamedMark:`).
    fn tool_selected(&mut self, tool: ToolId) {
        let first = self.belt.tools.get(&tool).and_then(|t| t.selection.rows.first().copied());
        match (tool, first) {
            // The entry's line shows and the session takes the keyboard (`CO:330-349`).
            (ToolId::CapturedOutput, Some(row)) => return self.reveal_captured(row),
            // The session comes forward (`TS`, `tableViewSelectionDidChange`).
            (ToolId::SessionStatus, Some(row)) => {
                let shown = self.tool_shown(tool);
                let pane = match &*shown {
                    Shown::Status(st) => st.items.get(row).map(|i| i.row.pane),
                    _ => None,
                };
                if let Some(pane) = pane {
                    self.reveal_pane(pane);
                }
                self.belt.focus = None;
                return;
            }
            _ => {}
        }
        if tool == ToolId::NamedMarks {
            let Some(row) = self.belt.tools.get(&tool).and_then(|t| t.selection.rows.first().copied()) else { return };
            let shown = self.tool_shown(tool);
            let Shown::Marks(marks) = &*shown else { return };
            let (Some(&(mark, _)), Some(pane)) = (marks.get(row), self.focused_id()) else { return };
            let line = self.panes.get_mut(&pane).and_then(|p| p.screen.marks_in_order().into_iter().find(|(_, m)| m.id == mark).map(|(l, _)| l));
            if let Some(line) = line {
                self.show_line(pane, line, false);
                self.mark_pos.insert(pane, mark);
            }
            return;
        }
        if tool != ToolId::CommandHistory {
            return;
        }
        let Some(row) = self.belt.tools.get(&tool).and_then(|t| t.selection.rows.first().copied()) else { return };
        let shown = self.tool_shown(tool);
        let Shown::Commands(rows) = &*shown else { return };
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
        let Mods { ctrl, alt, shift, .. } = self.mods;
        let rows = self.belt.tools.get(&tool).map(|t| t.selection.rows.clone()).unwrap_or_default();
        let Some(&first) = rows.first() else { return };
        let shown = self.tool_shown(tool);
        let pane = self.focused_id();
        match &*shown {
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
            Shown::Dirs(v, _) => {
                let (Some(d), Some(pane)) = (v.get(first), pane) else { return };
                let shell = self.panes.get(&pane).and_then(|p| p.own().shell.shell.clone());
                let text = tt::dir_insert(&d.path, shell.as_deref(), alt, shift);
                self.user_write(pane, text.as_bytes(), Write::SendText);
            }
            // Option wins over Shift (`TS:372-396`).
            Shown::Snippets { .. } => return self.apply_snippets(tt::snippet_pick(alt, shift), false, event_loop),
            Shown::Actions(_) => return self.apply_actions(event_loop),
            Shown::Captured(_) => return self.run_captured(first),
            Shown::Marks(_) | Shown::Status(_) | Shown::Jobs(_) | Shown::Notes | Shown::Codecierge(_) => return,
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
        // A button that needs a selection it does not have does nothing (drawn dimmed).
        let shown = self.tool_shown(tool);
        let rows = self.belt.tools.get(&tool).map(|t| t.selection.rows.clone()).unwrap_or_default();
        if !button_enabled(b, selected_count(&shown, &rows)) {
            return;
        }
        let Mods { alt, shift, .. } = self.mods;
        match (tool, b) {
            (ToolId::Snippets, ToolButton::Send) => self.apply_snippets(tt::snippet_pick(alt, shift), false, event_loop),
            // Shift makes it the Composer (`TS:390-392`).
            (ToolId::Snippets, ToolButton::AdvancedPaste) => self.apply_snippets(if shift { SnippetPick::Composer } else { SnippetPick::AdvancedPaste }, false, event_loop),
            (ToolId::Snippets, ToolButton::Help) => {
                if let Some(pane) = self.focused_id() {
                    let text = crate::settings_shortcuts::SEARCH_HELP.replace("\n\n", " ").replace('\n', " ");
                    self.ask(pane, SNIPPET_HELP_BAR, &text, &["OK"], false);
                }
            }
            (ToolId::Snippets | ToolId::Actions | ToolId::NamedMarks, ToolButton::Remove) => self.tool_remove(tool),
            (ToolId::Snippets | ToolId::Actions | ToolId::NamedMarks, ToolButton::Add) => self.tool_edit(tool, false, event_loop),
            (ToolId::Snippets | ToolId::Actions | ToolId::NamedMarks, ToolButton::Edit) => self.tool_edit(tool, true, event_loop),
            (ToolId::Actions, ToolButton::Apply) => self.apply_actions(event_loop),
            (ToolId::Profiles, ToolButton::Open) => {
                if self.belt.tools.get(&tool).is_some_and(|t| !t.selection.rows.is_empty()) {
                    let pick = self.tool_state(tool).popup.min(tt::PROFILE_OPENS.len() - 1);
                    self.open_profiles(tt::PROFILE_OPENS[pick].0);
                }
            }
            (ToolId::PasteHistory, ToolButton::Trash) => {
                if confirm(self.hwnd(), tt::ERASE_PASTE.0, tt::ERASE_PASTE.1) {
                    self.history().clear();
                    self.tools_changed();
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
                    self.tools_changed();
                    self.tool_state(tool).selection.rows.clear();
                }
            }
            (ToolId::RecentDirectories, ToolButton::Trash) => {
                if confirm(self.hwnd(), tt::ERASE_DIRS.0, tt::ERASE_DIRS.1) {
                    self.shell_history_pub().erase_dirs();
                    self.save_shell_history();
                    self.tools_changed();
                    self.tool_state(tool).selection.rows.clear();
                }
            }
            (ToolId::CapturedOutput, ToolButton::Trash) => self.clear_captured(),
            (ToolId::CapturedOutput, ToolButton::Help) => {
                if let Some(pane) = self.focused_id() {
                    self.open_url(pane, crate::captured::HELP_URL);
                }
            }
            (ToolId::SessionStatus, ToolButton::Bell) => {
                let target = self.status_selected();
                self.arm_status_watch(target);
            }
            (ToolId::SessionStatus, ToolButton::Snooze) => {
                if let Some(pane) = self.status_selected() {
                    self.toggle_snooze(pane);
                }
            }
            (ToolId::SessionStatus, ToolButton::Help) => self.status_help(),
            (ToolId::SessionStatus, ToolButton::Settings) => self.open_status_popover(event_loop),
            (ToolId::Jobs, ToolButton::Send) => self.jobs_send(),
            (ToolId::Jobs, ToolButton::Inspect) => self.jobs_inspect(event_loop),
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
        // Jobs' signal popup, enabled with a selection (`JT:573-578`).
        if tool == ToolId::Jobs {
            if self.belt.tools.get(&tool).is_none_or(|t| t.selection.rows.is_empty()) {
                return;
            }
            let Some(area) = self.tool_area(tool) else { return };
            let (l, ..) = self.tool_layout(tool, area);
            if let Some(r) = l.popup {
                self.job_signal_menu(MenuAt { x: r.x, y: r.y + r.h, above: false });
            }
            return;
        }
        if tool != ToolId::Profiles || self.belt.tools.get(&tool).is_none_or(|t| t.selection.rows.is_empty()) {
            return;
        }
        let Some(area) = self.tool_area(tool) else { return };
        let (l, ..) = self.tool_layout(tool, area);
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
        if tool == ToolId::Jobs {
            return self.job_row_menu(row);
        }
        let shown = self.tool_shown(tool);
        // Session Status' Snooze, checked while snoozed (`TS:1187-1218`).
        if let Shown::Status(st) = &*shown {
            let Some(item) = st.items.get(row) else { return };
            let menu = muda::Menu::new();
            if let Err(e) = menu.append(&muda::CheckMenuItem::with_id(format!("{CTX_ID}snooze:{}", item.row.pane), "Snooze", true, item.row.snoozed, None)) {
                self.banners.set("menu", format!("Toolbelt menu: {e}"));
            }
            let (x, y) = self.cursor;
            return self.show_menu_at(menu, MenuAt { x, y, above: false });
        }
        let items: Vec<(String, &str)> = match &*shown {
            Shown::Paste(v) if row < v.len() => vec![(format!("{CTX_ID}paste-copy:{row}"), "Copy"), (format!("{CTX_ID}paste-advanced:{row}"), "Open in Advanced Paste")],
            Shown::Dirs(v, _) => match v.get(row) {
                Some(d) => vec![(format!("{CTX_ID}star:{}", d.path), "Toggle Star")],
                None => return,
            },
            // Keyed by the entry (pane, mark, line and place on it), not the row: captures that
            // arrive while the menu is up shift the rows.
            Shown::Captured(c) if row < c.entries.len() => match self.captured_check_key(c, row) {
                Some(key) => vec![(format!("{CTX_ID}captured-check:{key}"), "Toggle Checkmark")],
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
            "captured-check" => {
                let f: Vec<&str> = arg.split(':').collect();
                if let [pane, mark, line, nth] = f[..] {
                    if let (Ok(pane), Ok(mark), Ok(line), Ok(nth)) = (pane.parse::<PaneId>(), mark.parse::<u32>(), line.parse::<i64>(), nth.parse::<usize>()) {
                        self.toggle_captured_check(pane, mark, (line, nth));
                    }
                }
            }
            "signal" => {
                if let Ok(i) = arg.parse::<usize>() {
                    if i < tt::SIGNALS.len() {
                        self.tool_state(ToolId::Jobs).signal = i;
                    }
                }
            }
            "job-copy" | "job-pid" | "job-notify" => {
                let process = arg.split_once(':').and_then(|(p, c)| Some((p.parse::<u32>().ok()?, c.parse::<u64>().ok()?)));
                match (what, process) {
                    ("job-notify", Some((pid, created))) => self.toggle_job_watch(pid, created),
                    (_, Some((pid, created))) => self.copy_job(pid, created, what == "job-copy"),
                    _ => {}
                }
            }
            "snooze" => {
                if let Ok(pane) = arg.parse::<PaneId>() {
                    self.toggle_snooze(pane);
                }
            }
            "star" => {
                if let Some(pane) = self.focused_id() {
                    let host = self.history_host_pub(pane);
                    self.shell_history_pub().toggle_star(&host, arg);
                    self.save_shell_history();
                    self.tools_changed();
                }
            }
            _ => {}
        }
        self.redraw();
        true
    }

    /// Another key consumer in the window takes the keyboard (find typing, Copy Mode, the
    /// autocomplete list, the shortcut labels): no tool keeps it, or its keys would still go to
    /// the tool, which `key` asks first.
    /// A tool takes the keyboard: a composer that had it lets it go (and the keyboard hook
    /// stops taking the composer's Win chords).
    pub(super) fn belt_take_focus(&mut self, tool: ToolId, focus: Focus) {
        self.belt.focus = Some((tool, focus));
        self.composer_yield_keys();
    }

    pub(super) fn belt_yield_keys(&mut self) {
        self.belt.focus = None;
        self.composer_yield_keys();
    }

    /// Keys while a tool's field or list has the keyboard (`tool_list::takes_key`): nothing
    /// taken reaches the session, releases and modifier keys alone included. Chords with Ctrl,
    /// Alt or Win go on as usual, except Ctrl+V into a field, Return with its modifiers, and
    /// AltGr typing.
    pub(super) fn toolbelt_key(&mut self, ev: &KeyEvent, event_loop: &ActiveEventLoop) -> bool {
        if ev.state != ElementState::Pressed {
            return self.belt_keys.release(&ev.physical_key);
        }
        let Some((tool, focus)) = self.belt.focus else {
            self.belt_keys.press(ev.physical_key, false);
            return false;
        };
        // A tool removed, or whose wrapper collapsed, has let the keyboard go.
        if !self.belt.visible || !self.belt.split.expanded(tool) {
            self.belt.focus = None;
            self.belt_keys.press(ev.physical_key, false);
            return false;
        }
        // The Notes editor reads its own keys (`text_edit::takes`).
        if tool == ToolId::Notes {
            let taken = self.notes_key(ev);
            self.belt_keys.press(ev.physical_key, taken);
            return taken;
        }
        // So does Codecierge's goal and reply (`codecierge_key`).
        if tool == ToolId::Codecierge {
            let taken = self.codecierge_key(ev);
            self.belt_keys.press(ev.physical_key, taken);
            return taken;
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
        self.belt_keys.press(ev.physical_key, taken);
        if !taken {
            return false;
        }
        let Some(key) = key else { return true };
        match focus {
            Focus::Field => self.tool_field_key(tool, key, event_loop),
            Focus::List => self.tool_list_key(tool, key, event_loop),
        }
        self.redraw();
        true
    }

    pub(super) fn tool_field_key(&mut self, tool: ToolId, key: ListKey, event_loop: &ActiveEventLoop) {
        let Mods { alt, shift, .. } = self.mods;
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
            // Return sends the selection, else the only match (`TS:963-975`).
            FieldOut::Enter if tool == ToolId::Snippets => self.apply_snippets(tt::snippet_pick(alt, shift), true, event_loop),
            FieldOut::Enter | FieldOut::Kept => {}
            FieldOut::GiveBack => self.belt.focus = None,
            FieldOut::Up | FieldOut::Down => self.tool_step(tool, out == FieldOut::Down),
            FieldOut::ToList => self.belt_take_focus(tool, Focus::List),
        }
    }

    fn tool_list_key(&mut self, tool: ToolId, key: ListKey, event_loop: &ActiveEventLoop) {
        match key {
            ListKey::Up | ListKey::Down => self.tool_step(tool, key == ListKey::Down),
            // Space toggles Inspect (`JT:40-46`); Right opens the selected parent, Left closes
            // it (NSOutlineView).
            ListKey::Text(t) if t == " " && tool == ToolId::Jobs => self.jobs_inspect(event_loop),
            ListKey::Right | ListKey::Left if tool == ToolId::Jobs => {
                let row = self.belt.tools.get(&tool).and_then(|t| t.selection.rows.first().copied());
                if let Some((pid, _, true)) = row.and_then(|r| self.job_at(r)) {
                    self.toggle_job(pid, Some(key == ListKey::Right));
                }
            }
            ListKey::Enter if tool == ToolId::Profiles => self.tool_activate(tool, event_loop),
            ListKey::Tab if self.tool_spec(tool).field => self.belt_take_focus(tool, Focus::Field),
            ListKey::Escape => self.belt.focus = None,
            // The Mac delete key (`kVK_Delete`, Windows' Backspace), and Windows' Delete, remove
            // without a question (`TS:421-428`, `iTermToolActions.m:177-183`,
            // `ToolNamedMarks.swift:140-146`).
            ListKey::Delete | ListKey::Backspace if matches!(tool, ToolId::Snippets | ToolId::Actions | ToolId::NamedMarks) => self.tool_remove(tool),
            // A named mark's cell is edited with Return (the prompt, here); as the Edit button,
            // only with exactly one mark selected.
            ListKey::Enter if tool == ToolId::NamedMarks => self.tool_button(tool, ToolButton::Edit, event_loop),
            // Right opens the selected folder and Left closes it (NSOutlineView).
            ListKey::Right | ListKey::Left if tool == ToolId::Snippets => {
                let row = self.belt.tools.get(&tool).and_then(|t| t.selection.rows.first().copied());
                let shown = self.tool_shown(tool);
                if let (Some(row), Shown::Snippets { paths, .. }) = (row, &*shown) {
                    if let Some(Some(path)) = paths.get(row) {
                        self.toggle_folder(path, Some(key == ListKey::Right));
                    }
                }
            }
            _ => {}
        }
    }

    /// The selected snippets (indexes into the snippets file), in row order. A snippet shown
    /// under two tags counts twice, as the source's `selectedSnippets` maps each row.
    fn selected_snippets(&mut self) -> Vec<usize> {
        let rows = self.belt.tools.get(&ToolId::Snippets).map(|t| t.selection.rows.clone()).unwrap_or_default();
        let shown = self.tool_shown(ToolId::Snippets);
        let Shown::Snippets { rows: list, .. } = &*shown else { return Vec::new() };
        rows.iter()
            .filter_map(|&i| match list.get(i)?.kind {
                SnippetRowKind::Snippet(k) => Some(k),
                SnippetRowKind::Folder(_) => None,
            })
            .collect()
    }

    /// Sends, opens in Advanced Paste, or opens in the Composer, each selected snippet
    /// (`TS:560-588`); with `only_match` and nothing selected, the search's only match
    /// (Return in the field). The keyboard goes back to the session.
    fn apply_snippets(&mut self, pick: SnippetPick, only_match: bool, event_loop: &ActiveEventLoop) {
        let mut picked = self.selected_snippets();
        if picked.is_empty() && only_match {
            if let Shown::Snippets { filtered, .. } = &*self.tool_shown(ToolId::Snippets) {
                if filtered.len() == 1 {
                    picked = filtered.clone();
                }
            }
        }
        if picked.is_empty() {
            return;
        }
        let Some(pane) = self.focused_id() else { return };
        for k in picked {
            let Some(s) = self.snippets.snippets.get(k).cloned() else { continue };
            match pick {
                SnippetPick::Send => {
                    let b = Binding { action: 63, text: s.action_key(), label: "Send Snippet".into(), version: s.version, escaping: Some(s.escaping.code()), apply_mode: 0 };
                    self.run_key_action(&b, None, None, event_loop);
                }
                // As Edit > Snippets does with Option: the text on the clipboard, then
                // Advanced Paste on it.
                SnippetPick::AdvancedPaste => {
                    let text = keyaction::expand(&s.value, s.escaping);
                    if let Some(cb) = self.clipboard.as_mut() {
                        if let Err(e) = cb.set_text(text) {
                            self.banners.set("clipboard", format!("Could not copy: {e}"));
                            continue;
                        }
                    }
                    self.open_advanced_paste(event_loop, pane, None);
                }
                // `TS:581-582`: appended to the dropdown, escaped by the snippet's mode.
                SnippetPick::Composer => self.open_composer_with(pane, &s.value, s.escaping),
            }
        }
        self.belt.focus = None;
        self.redraw();
    }

    /// Runs each selected action in the current session, with its apply mode
    /// (`iTermToolActions.m:231-236`), as Edit > Actions does.
    fn apply_actions(&mut self, event_loop: &ActiveEventLoop) {
        let rows = self.belt.tools.get(&ToolId::Actions).map(|t| t.selection.rows.clone()).unwrap_or_default();
        let actions = user_actions::load(self.globals.get("Actions").as_ref());
        let picked: Vec<Binding> = rows.iter().filter_map(|&i| actions.get(i).map(|a| a.binding())).collect();
        if picked.is_empty() {
            return;
        }
        for b in picked {
            self.run_key_action(&b, None, None, event_loop);
        }
        self.belt.focus = None;
        self.redraw();
    }

    /// The Actions setting as stored now (an empty list when there is none).
    fn actions_value(&self) -> toml::Value {
        self.globals.get("Actions").unwrap_or_else(|| user_actions::store(&[]))
    }

    /// Replaces the actions and saves them; Edit > Actions and the tool follow.
    fn set_actions(&mut self, v: toml::Value) {
        self.globals.set("Actions", v);
        self.save_globals();
        self.refresh_snippet_menus();
        self.tools_changed();
    }

    /// Remove and Delete: Snippets and Actions drop the selected rows and push an undo
    /// (`TS:409-413`, `iTermToolActions.m:165-169`), Named Marks remove the marks
    /// (`ToolNamedMarks.swift:196-201`). No question.
    fn tool_remove(&mut self, tool: ToolId) {
        match tool {
            ToolId::Snippets => {
                let picked = self.selected_snippets();
                if picked.is_empty() {
                    return;
                }
                let guids: HashSet<String> = picked.iter().filter_map(|&k| self.snippets.snippets.get(k).map(|s| s.guid.clone())).collect();
                let before = self.snippets.snippets.clone();
                self.tool_state(tool).undo.push(ToolUndo::Snippets(before));
                self.snippets.snippets.retain(|s| !guids.contains(&s.guid));
                self.save_snippets();
            }
            ToolId::Actions => {
                let rows = self.belt.tools.get(&tool).map(|t| t.selection.rows.clone()).unwrap_or_default();
                let mut list = user_actions::load(self.globals.get("Actions").as_ref());
                if !rows.iter().any(|&r| r < list.len()) {
                    return;
                }
                let before = self.actions_value();
                self.tool_state(tool).undo.push(ToolUndo::Actions(before));
                let mut i = 0;
                list.retain(|_| {
                    i += 1;
                    !rows.contains(&(i - 1))
                });
                self.set_actions(user_actions::store(&list));
            }
            ToolId::NamedMarks => {
                let rows = self.belt.tools.get(&tool).map(|t| t.selection.rows.clone()).unwrap_or_default();
                let Shown::Marks(marks) = &*self.tool_shown(tool) else { return };
                let ids: Vec<u32> = rows.iter().filter_map(|&r| marks.get(r).map(|(id, _)| *id)).collect();
                let Some(p) = self.focused_id().and_then(|id| self.panes.get_mut(&id)) else { return };
                for id in ids {
                    p.screen.remove_mark(id);
                }
            }
            _ => return,
        }
        self.tool_state(tool).selection.rows.clear();
        self.redraw();
    }

    /// Add (`edit` false) and Edit: Snippets and Actions open Settings > Shortcuts with their
    /// sheet (a new one, or the selected row's); Named Marks open the Set Named Mark prompt, to
    /// add a mark or rename the selected one (`PT:15733-15752`).
    fn tool_edit(&mut self, tool: ToolId, edit: bool, event_loop: &ActiveEventLoop) {
        use crate::settings_shortcuts::Tab;
        let row = self.belt.tools.get(&tool).and_then(|t| t.selection.rows.first().copied());
        match tool {
            ToolId::Snippets => {
                let index = if edit {
                    let picked = self.selected_snippets();
                    if picked.len() != 1 {
                        return;
                    }
                    Some(picked[0])
                } else {
                    None
                };
                self.open_settings_page(event_loop, crate::settings_ui::Page::Shortcuts);
                if let Some(s) = self.settings.as_mut() {
                    s.show_shortcuts_editor(Tab::Snippets, index);
                }
            }
            ToolId::Actions => {
                let index = if edit { row.filter(|&r| r < user_actions::load(self.globals.get("Actions").as_ref()).len()) } else { None };
                if edit && index.is_none() {
                    return;
                }
                self.open_settings_page(event_loop, crate::settings_ui::Page::Shortcuts);
                if let Some(s) = self.settings.as_mut() {
                    s.show_shortcuts_editor(Tab::Actions, index);
                    // The sheet's OK pushes the actions as they are now (`settle_action_undo`).
                    self.action_edit_undo = self.current_window().map(|w| (w, self.actions_value()));
                }
            }
            ToolId::NamedMarks => {
                let Some(pane) = self.focused_id() else { return };
                if !edit {
                    return self.open_named_mark_prompt(pane, event_loop);
                }
                let Some(row) = row else { return };
                let shown = self.tool_shown(tool);
                let Shown::Marks(marks) = &*shown else { return };
                let Some((mark, _)) = marks.get(row).cloned() else { return };
                let name = self.panes.get(&pane).and_then(|p| p.screen.marks.records.get(&mark)).and_then(|m| m.name.clone()).unwrap_or_default();
                self.open_rename_mark_prompt(pane, mark, &name, event_loop);
            }
            _ => {}
        }
    }

    /// After a settings edit: an Edit Action sheet the toolbelt opened has closed; if it changed
    /// the actions, the actions it began from go on that window's Actions undo.
    pub(super) fn settle_action_undo(&mut self) {
        let Some((window, before)) = self.action_edit_undo.take() else { return };
        if self.settings.as_ref().is_some_and(|s| s.editing_action()) {
            self.action_edit_undo = Some((window, before));
            return;
        }
        if self.actions_value() == before {
            return;
        }
        let back = self.current_window();
        if self.enter(window) {
            self.tool_state(ToolId::Actions).undo.push(ToolUndo::Actions(before));
        }
        if let Some(w) = back {
            self.enter(w);
        }
    }

    /// Win+Z while Snippets or Actions has the keyboard: that tool's last change is undone and
    /// saved (Edit > Undo stays disabled in sessions). True when the chord was the tool's.
    pub(super) fn toolbelt_chord(&mut self, chord: &Chord) -> bool {
        let z = matches!(chord.key, ChordKey::Char(c) if c.eq_ignore_ascii_case(&'z'));
        if !(z && chord.cmd && !chord.alt && !chord.shift && !chord.ctrl) {
            return false;
        }
        self.toolbelt_undo()
    }

    /// Undo in the Snippets or Actions tool while it has the keyboard: its last change is undone
    /// and saved. True when a tool had the keyboard, whether or not it had a change to undo.
    pub(super) fn toolbelt_undo(&mut self) -> bool {
        let Some((tool, _)) = self.belt.focus.filter(|(t, _)| matches!(t, ToolId::Snippets | ToolId::Actions)) else { return false };
        if !self.belt.visible {
            return false;
        }
        let Some(undo) = self.tool_state(tool).undo.pop() else { return true };
        match undo {
            ToolUndo::Snippets(list) => {
                self.snippets.snippets = list;
                self.save_snippets();
            }
            ToolUndo::Actions(v) => self.set_actions(v),
        }
        self.tool_state(tool).selection.rows.clear();
        self.redraw();
        true
    }

    /// Key action 44, Undo: Edit > Undo does something only in a tool's text field; with a
    /// terminal focused the item is disabled in a session (`commands::DISABLED_IN_SESSION`), and
    /// the action beeps (`PS:12166-12168`, `PS:5148-5153`).
    pub(super) fn undo_key_action(&mut self) {
        if !self.toolbelt_undo() {
            super::find_ui::beep();
        }
    }

    /// A row drag let go: Actions move the selected actions above the drop row and push an
    /// undo (`iTermToolActions.m:360-378`); Snippets move the selected snippets to the root
    /// place, its child index taken as a model index as the source does (`TS:876-899`), with no
    /// undo. A press that did not move is a click.
    pub(super) fn tool_drop(&mut self, tool: ToolId, start_y: f32) {
        let (_, y) = self.cursor;
        let dragged = (y - start_y).abs() >= DRAG_SLOP * self.scale();
        // A press that kept a selection of several and did not drag leaves its row alone.
        self.tool_state(tool).selection.release(dragged);
        if !dragged {
            return;
        }
        let Some(area) = self.tool_area(tool) else { return };
        let (l, _, _, shown) = self.tool_layout(tool, area);
        if y < l.list.y || y > l.list.y + l.list.h {
            return;
        }
        let Some(slot) = drop_slot(&shown, &l.rows, y) else { return };
        let sel = self.belt.tools.get(&tool).map(|t| t.selection.rows.clone()).unwrap_or_default();
        match &*shown {
            Shown::Actions(_) => {
                let list = user_actions::load(self.globals.get("Actions").as_ref());
                let rows: Vec<usize> = sel.iter().copied().filter(|&r| r < list.len()).collect();
                if rows.is_empty() {
                    return;
                }
                let before = self.actions_value();
                self.tool_state(tool).undo.push(ToolUndo::Actions(before));
                self.set_actions(user_actions::store(&tt::move_actions(&list, &rows, slot)));
                // The moved rows stay selected where they landed.
                let start = slot - rows.iter().filter(|&&r| r < slot).count();
                self.tool_state(tool).selection.rows = (start..start + rows.len()).collect();
            }
            Shown::Snippets { rows, filtered, .. } => {
                let child = rows[..slot.min(rows.len())].iter().filter(|r| r.depth == 0).count();
                let guids: Vec<String> = self.selected_snippets().iter().filter_map(|&k| self.snippets.snippets.get(k).map(|s| s.guid.clone())).collect();
                if guids.is_empty() {
                    return;
                }
                let Some(list) = tt::move_snippets(&self.snippets.snippets, &guids, child, filtered.len()) else { return };
                self.snippets.snippets = list;
                self.save_snippets();
                // The moved snippets stay selected.
                let shown = self.tool_shown(tool);
                if let Shown::Snippets { rows, .. } = &*shown {
                    let keep: Vec<usize> = rows.iter().enumerate().filter(|(_, r)| matches!(r.kind, SnippetRowKind::Snippet(k) if self.snippets.snippets.get(k).is_some_and(|s| guids.contains(&s.guid)))).map(|(i, _)| i).collect();
                    self.tool_state(tool).selection.rows = keep;
                }
            }
            _ => {}
        }
        self.redraw();
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
        let (l, ..) = self.tool_layout(tool, area);
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
        match tool {
            ToolId::Notes => return self.notes_wheel(dy),
            ToolId::Codecierge => return self.codecierge_wheel(dy),
            _ => {}
        }
        let row_h = self.list_metrics().row_h;
        let (l, ..) = self.tool_layout(tool, area);
        let st = self.tool_state(tool);
        st.scroll = tool_list::clamp_scroll(st.scroll - dy * row_h, &l);
        self.redraw();
    }
}

/// A row's inset in pixels.
fn tool_list_inset(scale: f32) -> f32 {
    scene::LIST_INSET * scale
}

/// How many selected rows a tool's buttons act on: Snippets' folder rows do not count (a
/// folder item has no snippet, `TS:590-596`).
fn selected_count(shown: &Shown, rows: &[usize]) -> usize {
    match shown {
        Shown::Snippets { rows: list, .. } => rows.iter().filter(|&&i| matches!(list.get(i).map(|r| &r.kind), Some(SnippetRowKind::Snippet(_)))).count(),
        _ => rows.len(),
    }
}

/// Whether a button acts with `count` rows selected: Open, Send, Apply, Remove and Inspect need
/// one or more, Open in Advanced Paste and Edit exactly one (`ToolProfiles.m:212-215`,
/// `TS:641-647`, `iTermToolActions.m:309-314`, `ToolNamedMarks.swift:98-101`, `JT:573-578`).
fn button_enabled(b: ToolButton, count: usize) -> bool {
    match b {
        ToolButton::Open | ToolButton::Send | ToolButton::Apply | ToolButton::Remove | ToolButton::Inspect => count > 0,
        ToolButton::AdvancedPaste | ToolButton::Edit => count == 1,
        _ => true,
    }
}

/// Where a row drag at `y` drops: before row `slot` (the list's end past the last row), in
/// the half of a row the pointer is over. Actions drop above any row; Snippets only between
/// the root's rows (a drop into a folder is refused, `TS:880-882`).
fn drop_slot(shown: &Shown, rows: &[Rect], y: f32) -> Option<usize> {
    let slot = rows.iter().position(|r| y < r.y + r.h / 2.0).unwrap_or(rows.len());
    match shown {
        Shown::Actions(_) => Some(slot),
        Shown::Snippets { rows: list, .. } => (slot >= list.len() || list[slot].depth == 0).then_some(slot),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(ctrl: bool, alt: bool, shift: bool) -> Mods {
        Mods { shift, alt, ctrl, cmd: false }
    }

    /// A release belongs to the tool exactly when its press did, whatever the keyboard focus
    /// or the modifiers are by then.
    #[test]
    fn releases_follow_their_press() {
        let mut keys = TakenKeys::default();
        // Esc taken on press (it gives the keyboard back): its release is still the tool's.
        keys.press("Esc", true);
        assert!(keys.release(&"Esc"));
        // Only once.
        assert!(!keys.release(&"Esc"));
        // Ctrl+C went to the session: C's release goes there too, Ctrl let go first or not.
        keys.press("C", false);
        assert!(!keys.release(&"C"));
        // A key pressed before the tool had the keyboard is not the tool's.
        assert!(!keys.release(&"A"));
        // A press the tool no longer takes (a repeat after focus left) forgets the old one.
        keys.press("B", true);
        keys.press("B", false);
        assert!(!keys.release(&"B"));
        // Focus lost: no release will come.
        keys.press("D", true);
        keys.clear();
        assert!(!keys.release(&"D"));
    }

    /// Buttons need their selection (folders do not count), and a drag drops above a row in
    /// Actions but only between the root's rows in Snippets.
    #[test]
    fn buttons_and_drops_follow_iterm2() {
        use tt::{SnippetRow, SnippetRowKind as K};
        assert!(!button_enabled(ToolButton::Send, 0) && button_enabled(ToolButton::Send, 2));
        assert!(button_enabled(ToolButton::Remove, 3) && !button_enabled(ToolButton::Apply, 0));
        assert!(button_enabled(ToolButton::Edit, 1) && !button_enabled(ToolButton::Edit, 2) && !button_enabled(ToolButton::AdvancedPaste, 0));
        assert!(button_enabled(ToolButton::Add, 0) && button_enabled(ToolButton::Help, 0));
        assert!(!button_enabled(ToolButton::Inspect, 0) && button_enabled(ToolButton::Inspect, 1));

        let row = |depth: usize, kind: K| SnippetRow { depth, kind, text: String::new(), tags: Vec::new() };
        // A, folder f (open) with B in it, then C at the root.
        let rows = vec![row(0, K::Snippet(0)), row(0, K::Folder("f".into())), row(1, K::Snippet(1)), row(0, K::Snippet(2))];
        let shown = Shown::Snippets { rows, paths: vec![None, Some("f".into()), None, None], filtered: vec![0, 1, 2] };
        assert_eq!(selected_count(&shown, &[0, 1, 2]), 2);
        let rects: Vec<Rect> = (0..4).map(|i| Rect { x: 0.0, y: i as f32 * 10.0, w: 100.0, h: 10.0 }).collect();
        assert_eq!(drop_slot(&shown, &rects, 2.0), Some(0));
        assert_eq!(drop_slot(&shown, &rects, 8.0), Some(1));
        // Between the folder and its child, or its child's top half: into the folder, refused.
        assert_eq!(drop_slot(&shown, &rects, 16.0), None);
        assert_eq!(drop_slot(&shown, &rects, 22.0), None);
        assert_eq!(drop_slot(&shown, &rects, 27.0), Some(3));
        assert_eq!(drop_slot(&shown, &rects, 99.0), Some(4));
        let actions = Shown::Actions(vec!["a".into(), "b".into()]);
        assert_eq!(drop_slot(&actions, &rects[..2], 16.0), Some(2));
        assert_eq!(drop_slot(&actions, &rects[..2], 14.0), Some(1));
        assert_eq!(drop_slot(&Shown::Marks(Vec::new()), &rects, 1.0), None);
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
