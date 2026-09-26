//! The 9k tools in the app: Captured Output (the Capture Output trigger's list, its warnings,
//! Clear, Toggle Checkmark and the coprocess a double-click runs) and Session Status (every
//! session's status in this window, sorted by the user's priority patterns, its bell, snooze,
//! help and settings popover), plus the pattern notifications and watches the status changes
//! drive. The pure parts are `captured` and `status_tool`; the list widget is 9j's.

use std::cell::RefCell;
use std::hash::{Hash, Hasher};
use std::time::Duration;

use super::toolbelt_tools_ui::{Shown, unix};
use super::toolbelt_ui::wrap_words;
use super::*;
use crate::captured::{self, Captured, Cleared};
use crate::palette::Rgb;
use crate::status_tool::{self, Pattern, StatusRow};
use crate::tool_list;
use crate::toolbelt::ToolId;

/// The Capture Output trigger's two warnings' bar ids (`PS:25167-25233`).
const NOT_VISIBLE_BAR: &str = "captured-not-visible";
const NO_SHELL_INTEGRATION_BAR: &str = "captured-no-shell-integration";
/// Session Status' help bar.
const STATUS_HELP_BAR: &str = "toolbelt-status-help";
/// Session Status' detail: one indent level in, at most 3 lines (`ToolStatusCellView.swift`).
pub(super) const DETAIL_INDENT: f32 = 14.0;
const DETAIL_LINES: usize = 3;
/// A press on the gear this soon after the popover closed for losing the focus to it closes
/// it, as a second click on a popover's button does.
const REOPEN_GUARD: Duration = Duration::from_millis(300);
/// The name a session without one goes by in a status alert (`StatusChange.DefaultSessionName`).
const DEFAULT_NAME: &str = "A session";

/// Each row's lines at one list width: Captured Output's label char-wrapped (first line
/// included), Session Status' status line (empty without one) then its detail lines.
#[derive(Default)]
pub(super) struct Wraps {
    width: f32,
    pub(super) lines: Vec<Vec<String>>,
}

/// What Captured Output shows: the session and command mark it follows, the entries that show
/// (cleared ones and those the search hides left out) and each one's index in the mark's list.
#[derive(Default)]
pub(super) struct CapturedShown {
    pub(super) pane: Option<PaneId>,
    pub(super) mark: u32,
    pub(super) entries: Vec<Captured>,
    pub(super) index: Vec<usize>,
    pub(super) wraps: RefCell<Wraps>,
}

/// One Session Status row and what it draws beside the text.
pub(super) struct StatusItem {
    pub(super) row: StatusRow,
    pub(super) dot: Option<Rgb>,
    pub(super) color: Option<Rgb>,
    /// The bell watches this session.
    pub(super) watched: bool,
}

pub(super) struct StatusShown {
    pub(super) items: Vec<StatusItem>,
    pub(super) separator: Option<usize>,
    pub(super) wraps: RefCell<Wraps>,
}

/// The panes the Session Status tool lists: each `(pane, in this window, buried, status text,
/// has an indicator)`, kept when it lives in this window, is not buried, and has a status
/// (text or indicator) (`TS:1050-1053`, `hasActiveStatus`).
fn rows_for(panes: &[(u64, bool, bool, Option<String>, bool)]) -> Vec<u64> {
    panes.iter().filter(|(_, here, buried, text, dot)| *here && !*buried && (text.as_deref().is_some_and(|t| !t.is_empty()) || *dot)).map(|(id, ..)| *id).collect()
}

/// `text` broken anywhere into lines no wider than `max_w` (NSLineBreakByCharWrapping); each
/// line holds at least one character, and an empty text is one empty line.
fn char_wrap(text: &str, max_w: f32, width: &dyn Fn(&str) -> f32) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for c in text.chars() {
        line.push(c);
        if line.chars().count() > 1 && width(&line) > max_w {
            line.pop();
            lines.push(std::mem::take(&mut line));
            line.push(c);
        }
    }
    if !line.is_empty() || lines.is_empty() {
        lines.push(line);
    }
    lines
}

/// A detail text word-wrapped to at most `DETAIL_LINES` lines, the last tail-truncated when
/// more would follow (`maximumNumberOfLines`, `truncatesLastVisibleLine`).
fn detail_wrap(text: &str, max_w: f32, width: &dyn Fn(&str) -> f32) -> Vec<String> {
    let text = text.replace(['\r', '\n'], " ");
    let mut lines = wrap_words(&text, max_w, width);
    if lines.len() > DETAIL_LINES {
        let rest = lines.split_off(DETAIL_LINES - 1).join(" ");
        lines.push(format!("{rest}\u{2026}"));
    }
    lines.into_iter().map(|l| tool_list::truncate_tail(&l, max_w, width)).collect()
}

/// The help markdown as one plain line for an info bar: headings, emphasis and code marks
/// dropped, list bullets kept as bullets.
fn plain_help(md: &str) -> String {
    md.lines()
        .map(|l| l.trim_start_matches('#').trim())
        .filter(|l| !l.is_empty())
        .map(|l| {
            let l = l.replace("**", "").replace('`', "");
            match l.strip_prefix("* ") {
                Some(rest) => format!("\u{2022} {rest}"),
                None => l,
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Captured Output's row: the label's first line, the rest under it in the text color.
pub(super) fn captured_list_row(lines: &[String]) -> scene::ListRow {
    scene::ListRow { text: lines.first().cloned().unwrap_or_default(), lines: lines.iter().skip(1).cloned().collect(), ..scene::ListRow::default() }
}

/// Session Status' row: the lead glyphs, the name with the shortcut right-aligned, then the
/// status line and the detail (`ToolStatusCellView.configure`).
pub(super) fn status_list_row(item: &StatusItem, lines: &[String], rule_above: bool, avail: f32, scale: f32, dark: bool, width: &dyn Fn(&str) -> f32) -> scene::ListRow {
    let lead = status_lead(item, dark);
    let lead_w = scene::lead_width(&lead, scale);
    let right = item.row.shortcut.clone().map(|s| {
        let w = width(&s);
        (s, w)
    });
    let right_room = right.as_ref().map_or(0.0, |(_, w)| w + 4.0 * scale);
    let text = tool_list::truncate_tail(&item.row.name, (avail - lead_w - right_room).max(0.0), width);
    scene::ListRow {
        text,
        right,
        lines: lines.to_vec(),
        lines_indent: DETAIL_INDENT,
        dim: item.row.snoozed,
        rule_above,
        status_color: Some(item.color.unwrap_or(scene::list_secondary(dark))),
        lead,
        ..scene::ListRow::default()
    }
}

/// Snooze (secondary), bell (accent) and the indicator dot, each only when it applies.
fn status_lead(item: &StatusItem, dark: bool) -> Vec<(char, Rgb)> {
    let mut lead = Vec::new();
    if item.row.snoozed {
        lead.push((tool_list::ToolButton::Snooze.glyph(), scene::list_secondary(dark)));
    }
    if item.watched {
        lead.push((tool_list::ToolButton::Bell.glyph(), scene::LIST_SELECTED));
    }
    if let Some(c) = item.dot {
        lead.push((scene::STATUS_DOT, c));
    }
    lead
}

impl App {
    /// The priority patterns as stored (`status_tool::load_patterns`).
    pub(super) fn status_patterns(&self) -> Vec<Pattern> {
        status_tool::load_patterns(self.globals.get(status_tool::PATTERNS_KEY).as_ref(), self.globals.get(status_tool::LEGACY_KEY).as_ref())
    }

    /// The command mark Captured Output follows: Command History's selected use while that
    /// tool is in the toolbelt, else the focused session's last command (`CO:137-148`).
    pub(super) fn captured_mark(&self) -> Option<(PaneId, u32)> {
        let pane = self.focused_id()?;
        let p = self.panes.get(&pane)?;
        if self.belt.split.tools.contains(&ToolId::CommandHistory) {
            let row = self.belt.tools.get(&ToolId::CommandHistory).and_then(|t| t.selection.rows.first().copied());
            let shown = self.belt.shown.get(&ToolId::CommandHistory).map(|(_, s)| s.clone());
            if let (Some(row), Some(shown)) = (row, shown) {
                if let Shown::Commands(rows) = &*shown {
                    if let Some(r) = rows.get(row) {
                        let used = p.screen.marks.records.values().find(|m| m.command.trim() == r.command && m.start.map(unix) == Some(r.time));
                        if let Some(m) = used {
                            return Some((pane, m.id));
                        }
                    }
                }
            }
        }
        p.screen.marks.last_command.map(|id| (pane, id))
    }

    /// Captured Output's view follows its mark: a new mark shows everything and clears the
    /// selection; an OSC 1337 ClearCapturedOutput seen for this mark hides what it had
    /// captured so far, as Clear does (`CO:137-164`).
    pub(super) fn captured_catch_up(&mut self) {
        let Some((pane, mark)) = self.captured_mark() else { return };
        let Some(count) = self.panes.get(&pane).and_then(|p| p.screen.marks.records.get(&mark)).map(|m| m.clear_count) else { return };
        let st = self.belt.tools.entry(ToolId::CapturedOutput).or_default();
        if st.cleared_pane != Some(pane) || st.cleared.mark != Some(mark) {
            st.cleared_pane = Some(pane);
            st.cleared = Cleared { mark: Some(mark), abs_line: i64::MIN, clear_count: count };
            st.selection.rows.clear();
        } else if count > st.cleared.clear_count {
            let list = self.panes.get(&pane).and_then(|p| p.screen.marks.records.get(&mark)).map(|m| m.captured.clone()).unwrap_or_default();
            if let Some(st) = self.belt.tools.get_mut(&ToolId::CapturedOutput) {
                st.cleared = captured::clear(&list, mark, count);
            }
        }
    }

    /// What Captured Output shows for `query`.
    pub(super) fn captured_shown(&self, query: &str) -> Shown {
        let Some((pane, mark)) = self.captured_mark() else { return Shown::Captured(CapturedShown::default()) };
        let Some(m) = self.panes.get(&pane).and_then(|p| p.screen.marks.records.get(&mark)) else { return Shown::Captured(CapturedShown::default()) };
        let cleared = self.belt.tools.get(&ToolId::CapturedOutput).map(|t| t.cleared).unwrap_or_default();
        let matching = captured::filter(&m.captured, query);
        let index: Vec<usize> = captured::visible(&m.captured, mark, m.clear_count, &cleared).into_iter().filter(|i| matching.contains(i)).collect();
        let entries = index.iter().map(|&i| m.captured[i].clone()).collect();
        Shown::Captured(CapturedShown { pane: Some(pane), mark, entries, index, wraps: RefCell::default() })
    }

    /// What Captured Output reads besides its query: the mark, its entries (a checkmark
    /// toggled or a line grown in place), its clear count, and the view's Clear.
    pub(super) fn captured_extra(&self, h: &mut impl Hasher) {
        let at = self.captured_mark();
        at.hash(h);
        if let Some(m) = at.and_then(|(pane, mark)| self.panes.get(&pane).and_then(|p| p.screen.marks.records.get(&mark))) {
            m.clear_count.hash(h);
            for c in &m.captured {
                (&c.line, c.state, c.abs_line).hash(h);
            }
        }
        if let Some(t) = self.belt.tools.get(&ToolId::CapturedOutput) {
            (t.cleared.mark, t.cleared.abs_line, t.cleared.clear_count, t.cleared_pane).hash(h);
        }
    }

    /// The toolbelt shows and lists Captured Output (`toolbeltIsVisibleWithCapturedOutput`:
    /// the tool list, collapsed or not).
    fn captured_tool_visible(&self) -> bool {
        self.belt.visible && self.toolbelt_tools().contains(&ToolId::CapturedOutput)
    }

    /// A Capture Output trigger fired in `pane` (`CaptureTrigger.m:53-68`): without Shell
    /// Integration it says so; else, unless the tool shows or the warning is silenced, it
    /// says the tool is not visible. The tool refreshes either way.
    pub(super) fn capture_fired(&mut self, pane: PaneId, needs_shell_integration: bool) {
        self.tools_changed();
        let no_shell_integration = needs_shell_integration && self.panes.get(&pane).is_some_and(|p| p.screen.prompts_seen() == 0);
        if no_shell_integration {
            if !self.globals.bool(captured::SUPPRESS_SI_KEY) {
                self.ask(pane, NO_SHELL_INTEGRATION_BAR, captured::NO_SHELL_INTEGRATION, &["Install", "Silence Warning"], false);
            }
        } else if !self.captured_tool_visible() && !self.globals.bool(captured::SUPPRESS_KEY) {
            self.ask(pane, NOT_VISIBLE_BAR, captured::NOT_VISIBLE, &["Show It", "Silence Warning"], false);
        }
        self.redraw();
    }

    /// Show It: the toolbelt shows and Captured Output is switched on (`PS:25078-25085`).
    pub(super) fn show_captured_tool(&mut self) {
        if !self.belt.visible {
            self.set_toolbelt_visible(true, true);
        }
        if !self.toolbelt_tools().contains(&ToolId::CapturedOutput) {
            self.toggle_toolbelt_tool(ToolId::CapturedOutput);
        }
    }

    /// The Capture Output warnings' answers. False for other bars.
    pub(super) fn captured_bar(&mut self, _pane: PaneId, id: &str, button: Option<&str>) -> bool {
        let key = match id {
            NOT_VISIBLE_BAR => captured::SUPPRESS_KEY,
            NO_SHELL_INTEGRATION_BAR => captured::SUPPRESS_SI_KEY,
            _ => return false,
        };
        match button {
            Some("Show It") => self.show_captured_tool(),
            Some("Install") => self.install_shell_integration(),
            Some("Silence Warning") => {
                self.globals.set(key, toml::Value::Boolean(true));
                self.save_globals();
            }
            _ => {}
        }
        true
    }

    /// Clear: hides what the mark has captured so far (`CO:250-265`).
    pub(super) fn clear_captured(&mut self) {
        let Some((pane, mark)) = self.captured_mark() else { return };
        let Some((list, count)) = self.panes.get(&pane).and_then(|p| p.screen.marks.records.get(&mark)).map(|m| (m.captured.clone(), m.clear_count)) else { return };
        let st = self.tool_state(ToolId::CapturedOutput);
        st.cleared_pane = Some(pane);
        st.cleared = captured::clear(&list, mark, count);
        st.selection.rows.clear();
        self.tools_changed();
    }

    /// Toggle Checkmark on shown row `row` (`CO:377-384`).
    pub(super) fn toggle_captured_check(&mut self, row: usize) {
        let shown = self.tool_shown(ToolId::CapturedOutput);
        let Shown::Captured(c) = &*shown else { return };
        let (Some(pane), Some(&i)) = (c.pane, c.index.get(row)) else { return };
        if let Some(e) = self.panes.get_mut(&pane).and_then(|p| p.screen.marks.records.get_mut(&c.mark)).and_then(|m| m.captured.get_mut(i)) {
            e.state = !e.state;
        }
        self.tools_changed();
        self.redraw();
    }

    /// A selected entry: its line shows in the session, which takes the keyboard
    /// (`toolbeltDidSelectMark:`).
    pub(super) fn reveal_captured(&mut self, row: usize) {
        let shown = self.tool_shown(ToolId::CapturedOutput);
        let Shown::Captured(c) = &*shown else { return };
        let (Some(pane), Some(e)) = (c.pane, c.entries.get(row)) else { return };
        let off = self.panes.get(&pane).map_or(0, |p| p.screen.term().grid().scrolled_off() as i64);
        let line = (e.abs_line - off).clamp(i32::MIN as i64, i32::MAX as i64) as i32;
        self.show_line(pane, line, false);
        self.belt.focus = None;
    }

    /// A double-click runs the entry's command as a coprocess, not silent, titled "Captured
    /// Output trigger", and the session takes the keyboard (`PS:25087-25101`).
    pub(super) fn run_captured(&mut self, row: usize) {
        let shown = self.tool_shown(ToolId::CapturedOutput);
        let Shown::Captured(c) = &*shown else { return };
        let (Some(pane), Some(e)) = (c.pane, c.entries.get(row)) else { return };
        if let Some(cmd) = e.command.clone() {
            self.start_coprocess_titled(pane, &cmd, false, crate::coprocess::Entry::Trigger, Some(captured::RUN_TITLE));
        }
        self.belt.focus = None;
    }

    /// Fills a Captured Output or Session Status list's wrapped lines for a list `width`
    /// pixels wide (kept until the width or the rows change).
    pub(super) fn wrap_rows(&mut self, shown: &Shown, width: f32) {
        let scale = self.scale();
        let dark = self.dark();
        let avail = (width - 2.0 * scene::LIST_INSET * scale).max(0.0);
        let renderer = self.gpu.as_mut().map(|g| RefCell::new(&mut g.renderer));
        let measure = |s: &str| match &renderer {
            Some(r) => r.borrow_mut().toolbelt_width(s, scale),
            None => s.chars().count() as f32 * 7.0 * scale,
        };
        match shown {
            Shown::Captured(c) => {
                let mut w = c.wraps.borrow_mut();
                if w.width != width || w.lines.len() != c.entries.len() {
                    w.width = width;
                    w.lines = c.entries.iter().map(|e| char_wrap(&captured::label(e), avail, &measure)).collect();
                }
            }
            Shown::Status(st) => {
                let mut w = st.wraps.borrow_mut();
                if w.width != width || w.lines.len() != st.items.len() {
                    w.width = width;
                    w.lines = st
                        .items
                        .iter()
                        .map(|it| {
                            let text_w = (avail - scene::lead_width(&status_lead(it, dark), scale)).max(0.0);
                            let mut v = vec![it.row.status.as_deref().map(|s| tool_list::truncate_tail(s, text_w, &measure)).unwrap_or_default()];
                            if let Some(d) = &it.row.detail {
                                v.extend(detail_wrap(d, (text_w - DETAIL_INDENT * scale).max(0.0), &measure));
                            }
                            if v.len() == 1 && v[0].is_empty() {
                                v.clear();
                            }
                            v
                        })
                        .collect();
                }
            }
            _ => {}
        }
    }

    /// Session Status' rows for this window, sorted (`TS:38-52`).
    pub(super) fn status_rows(&self) -> Vec<StatusRow> {
        let patterns = self.status_patterns();
        let (tab_tag, pane_tag) = (self.globals.int("SwitchTabModifier"), self.globals.int("SwitchPaneModifier"));
        let mut rows = Vec::new();
        for (ti, t) in self.tabs.iter().enumerate() {
            let panes = t.layout.panes();
            let facts: Vec<(u64, bool, bool, Option<String>, bool)> = panes
                .iter()
                .filter_map(|id| self.panes.get(id).map(|p| (*id, true, self.buried.contains(*id), p.own().tab_status.status.clone(), p.own().tab_status.indicator.is_some())))
                .collect();
            for id in rows_for(&facts) {
                let Some(p) = self.panes.get(&id) else { continue };
                let st = &p.own().tab_status;
                let generation = p.own().tab_status_gen;
                let ordinal = panes.iter().position(|x| *x == id).map_or(0, |i| i + 1);
                rows.push(StatusRow {
                    pane: id,
                    snoozed: self.belt.snoozed.get(&id) == Some(&generation),
                    priority: status_tool::priority(st.status.as_deref(), &patterns),
                    changed: generation,
                    name: self.session_label(id).0,
                    status: st.status.as_deref().filter(|s| !s.is_empty()).map(status_tool::display_status),
                    detail: st.detail.clone().filter(|d| !d.is_empty()),
                    shortcut: status_tool::shortcut_label(ti + 1, ti == self.active, ordinal, panes.len(), tab_tag, pane_tag),
                });
            }
        }
        status_tool::sort_rows(&mut rows);
        rows
    }

    /// What Session Status shows: its rows with their colors and bells.
    pub(super) fn status_shown(&self) -> Shown {
        let items: Vec<StatusItem> = self
            .status_rows()
            .into_iter()
            .map(|row| {
                let st = self.panes.get(&row.pane).map(|p| p.own().tab_status.clone()).unwrap_or_default();
                let watched = self.status_watch.as_ref().is_some_and(|(w, _)| *w == row.pane);
                StatusItem { dot: st.indicator.as_deref().and_then(crate::tabstatus::xterm_color), color: st.status_color.as_deref().and_then(crate::tabstatus::xterm_color), watched, row }
            })
            .collect();
        let rows: Vec<StatusRow> = items.iter().map(|i| i.row.clone()).collect();
        Shown::Status(StatusShown { separator: status_tool::separator_before(&rows), items, wraps: RefCell::default() })
    }

    /// What Session Status reads: every pane of this window (its status's generation and its
    /// name), the tabs and the focus (the shortcuts), the snoozes, the watches, the switch
    /// modifiers and the patterns.
    pub(super) fn status_extra(&self, h: &mut impl Hasher) {
        for t in &self.tabs {
            for id in t.layout.panes() {
                id.hash(h);
                if let Some(p) = self.panes.get(&id) {
                    p.own().tab_status_gen.hash(h);
                }
                self.session_label(id).0.hash(h);
                self.buried.contains(id).hash(h);
            }
            0u8.hash(h);
        }
        (self.active, self.focused_id(), &self.belt.snoozed, &self.status_watch, self.notify_status_armed).hash(h);
        (self.globals.int("SwitchTabModifier"), self.globals.int("SwitchPaneModifier")).hash(h);
        for p in self.status_patterns() {
            (p.pattern, p.notify).hash(h);
        }
    }

    /// A snooze lasts until its session's status changes: those that moved, or whose session
    /// is gone, are dropped.
    pub(super) fn prune_snoozes(&mut self) {
        let gens: HashMap<u64, u64> = self.belt.snoozed.keys().filter_map(|id| self.panes.get(id).map(|p| (*id, p.own().tab_status_gen))).collect();
        self.belt.snoozed.retain(|id, generation| gens.get(id) == Some(generation));
    }

    /// Snooze or un-snooze a session (`TS:1187-1218`).
    pub(super) fn toggle_snooze(&mut self, pane: PaneId) {
        let Some(generation) = self.panes.get(&pane).map(|p| p.own().tab_status_gen) else { return };
        if self.belt.snoozed.get(&pane) == Some(&generation) {
            self.belt.snoozed.remove(&pane);
        } else {
            self.belt.snoozed.insert(pane, generation);
        }
        self.tools_changed();
        self.redraw();
    }

    /// The session selected in Session Status (the focused one, when it has a row).
    pub(super) fn status_selected(&mut self) -> Option<PaneId> {
        let row = self.belt.tools.get(&ToolId::SessionStatus).and_then(|t| t.selection.rows.first().copied())?;
        let shown = self.tool_shown(ToolId::SessionStatus);
        match &*shown {
            Shown::Status(s) => s.items.get(row).map(|i| i.row.pane),
            _ => None,
        }
    }

    /// The bell: arms (or disarms) the watch on a session, or with none selected, Window >
    /// Notify on Status Change (`TS`, `toggleNotify`).
    pub(super) fn arm_status_watch(&mut self, pane: Option<PaneId>) {
        match pane {
            Some(p) => {
                if self.status_watch.as_ref().is_some_and(|(w, _)| *w == p) {
                    self.status_watch = None;
                } else {
                    let now = self.panes.get(&p).and_then(|x| x.own().tab_status.status.clone());
                    self.status_watch = Some((p, now));
                }
            }
            None => {
                self.notify_status_armed = !self.notify_status_armed;
                self.sync_checks();
            }
        }
        self.tools_changed();
        self.redraw();
    }

    /// The bell is lit: armed for what it acts on now.
    pub(super) fn status_bell_lit(&mut self) -> bool {
        match self.status_selected() {
            Some(p) => self.status_watch.as_ref().is_some_and(|(w, _)| *w == p),
            None => self.notify_status_armed,
        }
    }

    /// Help: the help text in a bar.
    pub(super) fn status_help(&mut self) {
        if let Some(pane) = self.focused_id() {
            self.ask(pane, STATUS_HELP_BAR, &plain_help(status_tool::HELP_TEXT), &["OK"], false);
        }
    }

    /// The gear: the settings popover above it, or closes the open one (`SPS`, `show`).
    pub(super) fn open_status_popover(&mut self, event_loop: &ActiveEventLoop) {
        if self.status_popover.take().is_some() || self.status_popover_closed.is_some_and(|t| t.elapsed() < REOPEN_GUARD) {
            self.status_prompt = None;
            return;
        }
        let Some(area) = self.tool_area(ToolId::SessionStatus) else { return };
        let (l, ..) = self.tool_layout(ToolId::SessionStatus, area);
        let Some(&(_, r)) = l.buttons.iter().find(|(b, _)| *b == tool_list::ToolButton::Settings) else { return };
        let (patterns, merge, subtitle, dark) = (self.status_patterns(), self.globals.bool(status_tool::MERGE_KEY), self.globals.bool(status_tool::SUBTITLE_KEY), self.dark());
        let Some(g) = self.gpu.as_ref() else { return };
        let origin = g.window.inner_position().unwrap_or_default();
        let at = (origin.x as f64 + f64::from(r.x + r.w / 2.0), origin.y as f64 + f64::from(r.y));
        match crate::status_popover_ui::StatusPopover::open(event_loop, &g.instance, &g.adapter, &g.device, &g.queue, patterns, merge, subtitle, at, dark) {
            Ok(w) => self.status_popover = Some(w),
            Err(e) => self.banners.set("window", format!("Could not open the window: {e}")),
        }
    }

    /// New patterns from the popover (or its prompt): stored, and every tab and tool follows.
    fn set_status_patterns(&mut self, patterns: &[Pattern]) {
        self.globals.set(status_tool::PATTERNS_KEY, status_tool::patterns_value(patterns));
        self.save_globals();
        self.tools_changed();
        self.redraw_all();
    }

    /// Handles an event for the settings popover or its + prompt. False for other windows.
    pub(super) fn status_popover_event(&mut self, event_loop: &ActiveEventLoop, window: winit::window::WindowId, event: &WindowEvent) -> bool {
        use crate::prompt_ui::PromptResult;
        use crate::status_popover_ui::PopoverEvent;
        if let Some(w) = self.status_prompt.as_mut().filter(|w| w.id() == window) {
            let answer = match event {
                WindowEvent::CloseRequested => Some(None),
                WindowEvent::RedrawRequested => match w.redraw() {
                    PromptResult::Open | PromptResult::Extra(_) => None,
                    PromptResult::Cancel => Some(None),
                    PromptResult::Ok(text) => Some(Some(text)),
                },
                other => {
                    w.event(other);
                    None
                }
            };
            if let Some(text) = answer {
                self.status_prompt = None;
                // An empty pattern adds nothing (`makeNew`); the new one goes at the end.
                let mut patterns = self.status_patterns();
                if let Some(t) = text.filter(|t| !t.is_empty()) {
                    patterns.push(Pattern { pattern: t, notify: false });
                    self.set_status_patterns(&patterns);
                }
                if let Some(p) = self.status_popover.as_mut() {
                    p.hold = false;
                    p.set_patterns(patterns);
                    p.focus();
                }
            }
            return true;
        }
        let Some(p) = self.status_popover.as_mut().filter(|p| p.id() == window) else { return false };
        let answer = p.event(event);
        match answer {
            PopoverEvent::None => {}
            PopoverEvent::Patterns(v) => self.set_status_patterns(&v),
            PopoverEvent::Merge(on) => {
                self.globals.set(status_tool::MERGE_KEY, toml::Value::Boolean(on));
                self.save_globals();
                self.tools_changed();
            }
            PopoverEvent::Subtitle(on) => {
                self.globals.set(status_tool::SUBTITLE_KEY, toml::Value::Boolean(on));
                self.save_globals();
                self.redraw_all();
            }
            // "New Priority Pattern" (`makeNew`); the popover stays open while it asks.
            PopoverEvent::AddPattern => {
                if let Some(w) = self.status_prompt.as_ref() {
                    w.focus();
                } else if let Some(g) = self.gpu.as_ref() {
                    let (title, info) = status_tool::NEW_PATTERN;
                    match crate::prompt_ui::PromptWindow::open(event_loop, &g.instance, &g.adapter, &g.device, &g.queue, title, info, "") {
                        Ok(w) => {
                            self.status_prompt = Some(w);
                            if let Some(p) = self.status_popover.as_mut() {
                                p.hold = true;
                            }
                        }
                        Err(e) => self.banners.set("window", format!("Could not open the window: {e}")),
                    }
                }
            }
            PopoverEvent::Close => {
                self.status_popover = None;
                self.status_prompt = None;
                self.status_popover_closed = Some(Instant::now());
            }
        }
        true
    }

    /// A status change's pattern notification and watches, from `tick_status_change`: for
    /// each `(pane, before, now)`, a notification when the new text is not empty, differs and
    /// its first matching pattern notifies (`PS:25683-25700`); the bell's session watch; and
    /// Window > Notify on Status Change's, on the first change (one alert for a session both
    /// watch, `NotifyOnStatusChangeController.flush`).
    pub(super) fn status_changes(&mut self, changes: &[(PaneId, Option<String>, Option<String>)]) {
        if !changes.is_empty() {
            let patterns = self.status_patterns();
            for (pane, from, to) in changes {
                let Some(text) = to.as_deref().filter(|t| !t.is_empty()) else { continue };
                if from.as_deref() == Some(text) || !status_tool::should_notify(Some(text), &patterns) {
                    continue;
                }
                let (title, body) = status_tool::notify_text(&self.session_label(*pane).0, text);
                match crate::notify::show_with(&title, &body, self.globals.bool("silentUserNotifications")) {
                    Ok(()) => self.last_notified = Some(*pane),
                    Err(e) => self.banners.set("notify", format!("Could not show a notification: {e}")),
                }
            }
        }
        let mut alerted = None;
        if let Some((pane, base)) = self.status_watch.clone() {
            match self.panes.get(&pane).map(|p| p.own().tab_status.status.clone()) {
                None => self.status_watch = None,
                Some(now) if now != base => {
                    self.status_watch = None;
                    self.status_alert(pane, base.as_deref(), now.as_deref());
                    alerted = Some(pane);
                    self.tools_changed();
                }
                Some(_) => {}
            }
        }
        if let (true, Some((pane, from, to))) = (self.notify_status_armed, changes.first()) {
            self.notify_status_armed = false;
            if alerted != Some(*pane) {
                self.status_alert(*pane, from.as_deref(), to.as_deref());
            }
            self.sync_checks();
            self.tools_changed();
        }
    }

    /// "Session status changed" with OK and Reveal (`presentAlert`).
    fn status_alert(&mut self, pane: PaneId, from: Option<&str>, to: Option<&str>) {
        let name = self.session_label(pane).0;
        let name = if name.is_empty() { DEFAULT_NAME.to_string() } else { name };
        let text = format!("Session status changed. {}", status_tool::watch_text(&name, from, to));
        self.ask(pane, super::alerts_ui::STATUS_BAR, &text, &["OK", "Reveal"], true);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_rows_come_from_this_window() {
        let panes = vec![
            (1, true, false, Some("work".to_string()), false),
            // Another window's pane.
            (2, false, false, Some("wait".to_string()), true),
            // Buried.
            (3, true, true, Some("idle".to_string()), true),
            // No status at all.
            (4, true, false, None, false),
            // An indicator alone is a status.
            (5, true, false, None, true),
            // An empty text is no text.
            (6, true, false, Some(String::new()), false),
        ];
        assert_eq!(rows_for(&panes), vec![1, 5]);
    }

    #[test]
    fn rows_wrap_like_iterm2() {
        let w = |s: &str| s.chars().count() as f32;
        assert_eq!(char_wrap("-abcdefg", 3.0, &w), vec!["-ab", "cde", "fg"]);
        assert_eq!(char_wrap("", 3.0, &w), vec![""]);
        assert_eq!(char_wrap("abc", 0.5, &w), vec!["a", "b", "c"], "a line holds one character at least");
        assert_eq!(detail_wrap("one two three", 20.0, &w), vec!["one two three"]);
        assert_eq!(detail_wrap("aa bb cc dd ee", 5.0, &w), vec!["aa bb", "cc dd", "ee"]);
        // A fourth line folds into the third, cut with an ellipsis.
        let four = detail_wrap("aa bb cc dd ee ff gg", 5.0, &w);
        assert_eq!(four.len(), 3);
        assert_eq!(&four[..2], &["aa bb".to_string(), "cc dd".to_string()]);
        assert!(four[2].starts_with("ee") && four[2].ends_with('\u{2026}') && four[2].chars().count() <= 5, "{four:?}");
        let help = plain_help("## Title\n\nThe **Session Status** tool.\n\n* `status=TEXT` sets it.");
        assert_eq!(help, "Title The Session Status tool. \u{2022} status=TEXT sets it.");
    }
}
