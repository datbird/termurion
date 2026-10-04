//! The 9k tools in the app: Captured Output (the Capture Output trigger's list, its warnings,
//! Clear, Toggle Checkmark and the coprocess a double-click runs), Session Status (every
//! session's status in this window, sorted by the user's priority patterns, its bell, snooze,
//! help and settings popover), plus the pattern notifications and watches the status changes
//! drive; Jobs (the focused session's process tree, its signals, Inspect and the termination
//! watches), Notes (the global note in `notes.txt` and the session note, in a native editor)
//! and Codecierge (its onboarding, goal and suggestion views). The pure parts are `captured`,
//! `status_tool`, `toolbelt_tools`, `notes` and `text_edit`; the list widget is 9j's.

use std::cell::RefCell;
use std::collections::{BTreeSet, HashSet};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::toolbelt_tools_ui::{Shown, is_modifier, now_ms, unix};
use super::toolbelt_ui::{BeltDrag, ToolState, wrap_words};
use super::*;
use crate::captured::{self, Captured, Cleared};
use crate::mouse::Mods;
use crate::notes::{self, Mode, Wrote};
#[cfg(test)]
use crate::notes::read_note;
use crate::palette::Rgb;
use crate::proctree::Proc;
use crate::status_tool::{self, Pattern, StatusRow};
use crate::text_edit::{self, EditKey, EditOut};
use crate::tool_list::{self, Focus};
use crate::toolbelt::ToolId;
use crate::codecierge::{CodeciergeState, GetStarted, Stage, View};
use crate::toolbelt_tools::{self as tt, JobRow, SignalWay};
use crate::warnings::Preempt;
use winit::keyboard::{Key as WKey, NamedKey};

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

/// "Confirmation Needed" before sending a signal to `count` processes (`JT:425-432`): OK and
/// Cancel, permanently silenceable.
pub(super) fn signal_spec(count: usize, any_children: bool) -> crate::warnings::Spec {
    let (heading, text) = tt::signal_question(count, any_children);
    let labels = [crate::api_auth::OK, crate::api_auth::CANCEL];
    crate::warnings::Spec {
        id: Some(tt::SEND_SIGNAL_KEY.to_string()),
        kind: crate::warnings::Kind::Permanent,
        heading: heading.to_string(),
        text,
        labels: labels.iter().map(|l| l.to_string()).collect(),
        cancel: Some(1),
        never_remember: Vec::new(),
    }
}

/// What an OSC clear's empty view is keyed to: the mark's capture count and its last entry
/// (a capture appends or grows the last one; nothing else changes the list while it is empty).
fn captured_fingerprint(list: &[Captured]) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    list.len().hash(&mut h);
    if let Some(c) = list.last() {
        (&c.line, c.state, c.abs_line).hash(&mut h);
    }
    h.finish()
}

/// One step of Captured Output's view (`captured_catch_up`) for the mark it follows now, that
/// mark's clear count, its captures' fingerprint and the search.
fn view_follows(st: &mut ToolState, pane: PaneId, mark: u32, count: u32, print: u64, query: String) {
    if st.cleared_pane != Some(pane) || st.cleared.mark != Some(mark) {
        st.cleared_pane = Some(pane);
        st.cleared = Cleared { mark: Some(mark), abs_line: i64::MIN, clear_count: count };
        st.blanked = None;
        st.selection.rows.clear();
    } else if count > st.cleared.clear_count {
        st.cleared.clear_count = count;
        st.blanked = Some((print, query));
        st.selection.rows.clear();
    } else if st.blanked.as_ref().is_some_and(|b| *b != (print, query)) {
        st.blanked = None;
    }
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

/// Jobs' row (`JT:245,265-275`): the disclosure (blank for a process without children), the
/// bell of a watched process before it, the command line, and the pid right-aligned (the PID
/// column).
pub(super) fn job_list_row(r: &JobRow, collapsed: bool, avail: f32, glyph_w: f32, scale: f32, width: &dyn Fn(&str) -> f32) -> scene::ListRow {
    let pid = r.pid.to_string();
    let pid_w = width(&pid);
    let lead = if r.watched { vec![(tool_list::ToolButton::Bell.glyph(), scene::LIST_SELECTED)] } else { Vec::new() };
    let used = r.depth as f32 * scene::LIST_INDENT * scale + scene::lead_width(&lead, scale) + glyph_w + 2.0 * scale + pid_w + 4.0 * scale;
    let disclosure = if !r.has_children {
        JOB_LEAF
    } else if collapsed {
        JOB_CLOSED
    } else {
        JOB_OPEN
    };
    scene::ListRow { text: tool_list::truncate_tail(&r.text, (avail - used).max(0.0), width), depth: r.depth, disclosure: Some(disclosure), right: Some((pid, pid_w)), lead, ..scene::ListRow::default() }
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

/// The global note's file: `<appdata>\Tandem\notes.txt` (`notes::FILE_NAME`).
pub(super) fn notes_path(appdata: &Path) -> PathBuf {
    appdata.join("Tandem").join(notes::FILE_NAME)
}

/// Jobs' signal popup is as wide as the widest signal name needs (`SignalPicker sizeToFit`).
pub(super) const SIGNAL_POPUP_W: f32 = 70.0;
/// Jobs refreshes once a second while it shows (`JT:379-396`), and the termination watches
/// look as often.
const JOBS_TICK: Duration = Duration::from_secs(1);
/// How often Send Signal's console events on their way are looked at.
const SIGNAL_POLL: Duration = Duration::from_millis(50);
/// Jobs' disclosure glyphs (Segoe Fluent Icons ChevronRight and ChevronDown); a row without
/// children keeps the room blank, as NSOutlineView does.
const JOB_CLOSED: char = '\u{E76C}';
const JOB_OPEN: char = '\u{E70D}';
const JOB_LEAF: char = ' ';
/// Inspect's labels (`iTermJobInfoPopoverViewController.swift:75,81`).
const INSPECT_PID: &str = "Process ID";
const INSPECT_COMMAND: &str = "Command";
/// The banner when every process a Send picked has ended by the time it is sent.
const SIGNAL_NONE_LEFT: &str = "No signal was sent: the selected processes have already ended.";
/// Notes: the mode control (a small segmented control) and the gap under it (`TN` `relayout`,
/// `kMargin`), the text's padding inside the frame (the text container's line fragment
/// padding), in points; a tab is drawn and measured as four spaces.
const SEGMENT_H: f32 = 20.0;
const NOTES_GAP: f32 = 4.0;
const NOTE_PAD_X: f32 = 5.0;
const NOTE_PAD_Y: f32 = 2.0;
const TAB_AS: &str = "    ";
/// Codecierge's button and the gap above it, points (`CC` `layoutSubviews`), its answer bar.
const CC_BUTTON_H: f32 = 22.0;
const CC_BUTTON_PAD: f32 = 24.0;
const CC_GAP: f32 = 8.0;
/// The suggestion view's markdown inset in its panel, points.
const CC_DOC_PAD: f32 = 6.0;

/// What Jobs shows: the shell it is rooted at, its rows, and the tree they came from (each
/// process's creation time and command line).
#[derive(Default)]
pub(super) struct JobsShown {
    pub(super) shell: Option<u32>,
    pub(super) rows: Vec<JobRow>,
    pub(super) procs: Vec<Proc>,
}

impl JobsShown {
    fn proc(&self, pid: u32) -> Option<&Proc> {
        self.procs.iter().find(|p| p.pid == pid)
    }
}

impl JobsShown {
    /// Each row's process as the selection keys it: pid and creation time.
    pub(super) fn keys(&self) -> Vec<(u32, u64)> {
        self.rows.iter().map(|r| (r.pid, self.proc(r.pid).map_or(0, |p| p.created))).collect()
    }
}

/// The Jobs selection follows its processes across a rebuild of the rows (a refreshed tree, a
/// collapse; `JT:492-569`), each keyed by pid and creation time, so a row added or removed
/// above it never moves it onto another process; another shell (a new key session) starts
/// with nothing selected.
pub(super) fn follow_jobs(sel: &mut tool_list::Selection, old: (Option<u32>, &[(u32, u64)]), new: (Option<u32>, &[(u32, u64)])) {
    if old.0 != new.0 {
        *sel = tool_list::Selection::default();
    } else {
        sel.remap(old.1, new.1);
    }
}

/// Signals Jobs will send once the question is answered: to each `(pid, creation time)` of
/// the shell's tree.
pub(super) struct SignalRequest {
    shell: u32,
    pids: Vec<(u32, u64)>,
    way: SignalWay,
    any_children: bool,
}

/// The note the Notes editor holds: the global one, or a session's (None: no session, and
/// typing stores nothing).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum NoteSource {
    Global,
    Session(Option<PaneId>),
}

/// "Cannot Notify on Termination" (`iTermJobTerminationMonitor.swift:143-149`).
fn cannot_notify_text(name: &str, pid: u32) -> (&'static str, String) {
    let name = if name.is_empty() { "(unknown)" } else { name };
    ("Cannot Notify on Termination", format!("Tandem cannot watch the job {name} with process ID {pid} because it has already terminated."))
}

/// A key as the focused Notes editor reads it, with the Ctrl and Alt that `text_edit::takes`
/// judges it by: Ctrl turns the arrows, Home, End, Backspace and Delete into their word and
/// document forms (`ctrl_nav`) and letters into the edit chords (`ctrl_key`); AltGr typing
/// (Ctrl and Alt together) is plain text; Shift+Insert, Ctrl+Insert and Shift+Delete are
/// paste, copy and cut. None: not an editor key (another Ctrl chord, or a plain Insert, which
/// the editor swallows).
pub(super) fn edit_key(key: &WKey, text: Option<&str>, mods: Mods) -> Option<(EditKey, bool, bool)> {
    // The Windows clipboard keys: Shift+Insert pastes, Ctrl+Insert copies, Shift+Delete cuts.
    let only = |ctrl: bool, shift: bool| mods.ctrl == ctrl && mods.shift == shift && !mods.alt;
    match key {
        WKey::Named(NamedKey::Insert) if only(false, true) => return Some((EditKey::Paste(String::new()), false, false)),
        WKey::Named(NamedKey::Insert) if only(true, false) => return Some((EditKey::Copy, true, false)),
        WKey::Named(NamedKey::Delete) if only(false, true) => return Some((EditKey::Cut, false, false)),
        _ => {}
    }
    let named = match key {
        WKey::Named(NamedKey::Enter) => Some(EditKey::Enter),
        WKey::Named(NamedKey::Tab) => Some(EditKey::Tab),
        WKey::Named(NamedKey::Backspace) => Some(EditKey::Backspace),
        WKey::Named(NamedKey::Delete) => Some(EditKey::Delete),
        WKey::Named(NamedKey::ArrowLeft) => Some(EditKey::Left),
        WKey::Named(NamedKey::ArrowRight) => Some(EditKey::Right),
        WKey::Named(NamedKey::ArrowUp) => Some(EditKey::Up),
        WKey::Named(NamedKey::ArrowDown) => Some(EditKey::Down),
        WKey::Named(NamedKey::Home) => Some(EditKey::Home),
        WKey::Named(NamedKey::End) => Some(EditKey::End),
        WKey::Named(NamedKey::PageUp) => Some(EditKey::PageUp),
        WKey::Named(NamedKey::PageDown) => Some(EditKey::PageDown),
        _ => None,
    };
    if let Some(k) = named {
        if mods.ctrl && !mods.alt {
            // Ctrl+Enter and Ctrl+Tab keep their names, and `takes` refuses them.
            return Some((text_edit::ctrl_nav(&k).unwrap_or(k), true, false));
        }
        return Some((k, mods.ctrl, mods.alt));
    }
    if let WKey::Character(c) = key {
        if mods.ctrl && !mods.alt {
            return c.chars().next().and_then(|c| text_edit::ctrl_key(c, mods.shift)).map(|k| (k, true, false));
        }
    }
    let printable = text.filter(|t| !t.is_empty() && !t.chars().any(char::is_control))?;
    let altgr = mods.ctrl && mods.alt;
    Some((EditKey::Text(printable.to_string()), mods.ctrl && !altgr, mods.alt && !altgr))
}

/// Where the Notes tool's parts are (pixels): the mode control's two segments, the editor's
/// frame and the text's room inside it.
pub(super) struct NoteLayout {
    pub(super) segments: [Rect; 2],
    pub(super) frame: Rect,
    pub(super) text: Rect,
}

pub(super) fn note_layout(area: Rect, scale: f32) -> NoteLayout {
    let seg_h = (SEGMENT_H * scale).min(area.h);
    let half = (area.w / 2.0).floor();
    let segments = [Rect { x: area.x, y: area.y, w: half, h: seg_h }, Rect { x: area.x + half, y: area.y, w: area.w - half, h: seg_h }];
    let top = area.y + seg_h + NOTES_GAP * scale;
    let frame = Rect { x: area.x, y: top, w: area.w, h: (area.y + area.h - top).max(0.0) };
    let (px, py) = (NOTE_PAD_X * scale + 1.0, NOTE_PAD_Y * scale + 1.0);
    let text = Rect { x: frame.x + px, y: frame.y + py, w: (frame.w - 2.0 * px).max(0.0), h: (frame.h - 2.0 * py).max(0.0) };
    NoteLayout { segments, frame, text }
}

/// Codecierge's onboarding view laid out in `area` (`CC:512-524,720-735`): the label's `lines`
/// on top, the button `CC_GAP` under them, the two centered together; returns the label's top
/// and the button.
fn codecierge_layout(area: Rect, lines: usize, line_h: f32, button_label_w: f32, scale: f32) -> (f32, Rect) {
    let button_h = (CC_BUTTON_H * scale).max(line_h + 4.0 * scale);
    let w = button_label_w + CC_BUTTON_PAD * scale;
    let label_h = lines as f32 * line_h;
    let total = label_h + CC_GAP * scale + button_h;
    let top = area.y + ((area.h - total) / 2.0).max(0.0);
    (top, Rect { x: area.x + ((area.w - w) / 2.0).max(0.0), y: top + label_h + CC_GAP * scale, w: w.min(area.w), h: button_h })
}

/// Where Codecierge's goal view puts things (`CG:745-912`): the prompt's top, the field's frame
/// and the text's room inside it, the checkbox and its label's origin, and Start.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct GoalLayout {
    pub(super) top: f32,
    pub(super) frame: Rect,
    pub(super) text: Rect,
    pub(super) check: Rect,
    pub(super) label_x: f32,
    pub(super) label_y: f32,
    pub(super) start: Rect,
}

/// The goal view in `area`: the prompt's `lines` on top, Start centered at the bottom, the
/// checkbox above it and the field filling what is between.
pub(super) fn goal_layout(area: Rect, lines: usize, line_h: f32, start_label_w: f32, scale: f32) -> GoalLayout {
    let pad = 4.0 * scale;
    let gap = CC_GAP * scale;
    let button_h = (CC_BUTTON_H * scale).max(line_h + 4.0 * scale);
    let top = area.y + pad;
    let start_y = (area.y + area.h - pad - button_h).max(top);
    let w = (start_label_w + CC_BUTTON_PAD * scale).min(area.w);
    let start = Rect { x: area.x + ((area.w - w) / 2.0).max(0.0), y: start_y, w, h: button_h };
    let side = line_h.max(12.0 * scale);
    let check = Rect { x: area.x + pad, y: (start_y - gap - side).max(top), w: side, h: side };
    let field_y = top + lines as f32 * line_h + gap / 2.0;
    let frame = Rect { x: area.x + pad, y: field_y, w: (area.w - 2.0 * pad).max(0.0), h: (check.y - gap - field_y).max(0.0) };
    let (px, py) = (NOTE_PAD_X * scale + 1.0, NOTE_PAD_Y * scale + 1.0);
    let text = Rect { x: frame.x + px, y: frame.y + py, w: (frame.w - 2.0 * px).max(0.0), h: (frame.h - 2.0 * py).max(0.0) };
    GoalLayout { top, frame, text, check, label_x: check.x + side + gap / 2.0, label_y: check.y + (side - line_h) / 2.0, start }
}

/// Where the suggestion view puts things (`CG:914-1115`): the goal line's row (the spinner's
/// box, and the label's x while it shows), the transcript's panel, the reply field and the send
/// button inside its right end, and End Task at the bottom right.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct RunLayout {
    pub(super) spinner: Rect,
    pub(super) goal_x: f32,
    pub(super) room: Rect,
    pub(super) reply: Rect,
    pub(super) reply_text: Rect,
    pub(super) send: Rect,
    pub(super) end: Rect,
}

pub(super) fn run_layout(area: Rect, line_h: f32, end_label_w: f32, scale: f32) -> RunLayout {
    let pad = 4.0 * scale;
    let gap = CC_GAP * scale;
    let button_h = (CC_BUTTON_H * scale).max(line_h + 4.0 * scale);
    let w = (end_label_w + CC_BUTTON_PAD * scale).min(area.w);
    let end = Rect { x: area.x + area.w - pad - w, y: (area.y + area.h - pad - button_h).max(area.y), w, h: button_h };
    let row_h = line_h + 2.0 * (NOTE_PAD_Y * scale + 1.0);
    let reply = Rect { x: area.x + pad, y: (end.y - gap - row_h).max(area.y), w: (area.w - 2.0 * pad).max(0.0), h: row_h };
    let send = Rect { x: reply.x + reply.w - row_h, y: reply.y, w: row_h.min(reply.w), h: row_h };
    let (px, py) = (NOTE_PAD_X * scale + 1.0, NOTE_PAD_Y * scale + 1.0);
    let reply_text = Rect { x: reply.x + px, y: reply.y + py, w: (send.x - reply.x - px).max(0.0), h: (reply.h - 2.0 * py).max(0.0) };
    let spinner = Rect { x: area.x + pad, y: area.y + pad, w: line_h, h: line_h };
    let room_y = spinner.y + line_h + gap / 2.0;
    let room = Rect { x: area.x + pad, y: room_y, w: (area.w - 2.0 * pad).max(0.0), h: (reply.y - gap / 2.0 - room_y).max(0.0) };
    RunLayout { spinner, goal_x: spinner.x + line_h + pad, room, reply, reply_text, send, end }
}

/// `text` cut at the tail to fit `max_w`, with an ellipsis (the goal line, `CG:914-1115`).
fn fit_tail(text: &str, max_w: f32, width: &dyn Fn(&str) -> f32) -> String {
    if width(text) <= max_w {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    let mut n = chars.len();
    while n > 0 {
        n -= 1;
        let cut: String = chars[..n].iter().collect::<String>().trim_end().to_string() + "\u{2026}";
        if width(&cut) <= max_w {
            return cut;
        }
    }
    "\u{2026}".to_string()
}

/// What `codecierge_geometry` measured: the wrapped label or prompt (x, y, text), the button
/// (Get Started, Reveal Setting, Start or End Task) with its label and whether it is enabled,
/// and the goal or suggestion view's layout.
struct CcGeo {
    lines: Vec<(f32, f32, String)>,
    button: Rect,
    label: &'static str,
    label_w: f32,
    enabled: bool,
    goal: Option<GoalLayout>,
    run: Option<RunLayout>,
    /// The suggestion view's goal line, cut to fit.
    goal_text: String,
    check_label_w: f32,
}

/// One editor's drawn parts.
struct FieldParts {
    clip: Rect,
    lines: Vec<(f32, f32, String)>,
    selection: Vec<Rect>,
    caret: Option<Rect>,
    thumb: Option<Rect>,
}

/// Codecierge's button for a state: its label and whether it is enabled (`CC:692-709`).
fn codecierge_button(s: CodeciergeState) -> (&'static str, bool) {
    match s {
        CodeciergeState::AdminDisabled => ("Get Started", false),
        CodeciergeState::NeedsAi => ("Reveal Setting", true),
        CodeciergeState::Ready => ("Get Started", true),
    }
}

/// An OK alert with an information icon.
fn alert(owner: Option<isize>, title: &str, text: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONINFORMATION, MB_OK, MessageBoxW};
    let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    let owner = owner.map_or(std::ptr::null_mut(), |h| h as *mut core::ffi::c_void);
    unsafe { MessageBoxW(owner, wide(text).as_ptr(), wide(title).as_ptr(), MB_OK | MB_ICONINFORMATION) };
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
    /// selection. An OSC 1337 ClearCapturedOutput seen for this mark empties the list for
    /// that one update only: it stays empty until the mark's captures or the search change,
    /// then every entry shows again (`CO:137-164`, kept); Clear's cut-off line is untouched.
    pub(super) fn captured_catch_up(&mut self) {
        let Some((pane, mark)) = self.captured_mark() else { return };
        let Some((count, print)) = self.panes.get(&pane).and_then(|p| p.screen.marks.records.get(&mark)).map(|m| (m.clear_count, captured_fingerprint(&m.captured))) else { return };
        let query = self.tool_field_text(ToolId::CapturedOutput);
        view_follows(self.belt.tools.entry(ToolId::CapturedOutput).or_default(), pane, mark, count, print, query);
    }

    /// What Captured Output shows for `query`.
    pub(super) fn captured_shown(&self, query: &str) -> Shown {
        let Some((pane, mark)) = self.captured_mark() else { return Shown::Captured(CapturedShown::default()) };
        let Some(m) = self.panes.get(&pane).and_then(|p| p.screen.marks.records.get(&mark)) else { return Shown::Captured(CapturedShown::default()) };
        let cleared = self.belt.tools.get(&ToolId::CapturedOutput).map(|t| t.cleared).unwrap_or_default();
        if self.belt.tools.get(&ToolId::CapturedOutput).is_some_and(|t| t.blanked.is_some()) {
            return Shown::Captured(CapturedShown { pane: Some(pane), mark, ..CapturedShown::default() });
        }
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
            (t.cleared.mark, t.cleared.abs_line, t.cleared.clear_count, t.cleared_pane, &t.blanked).hash(h);
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
        st.blanked = None;
        st.selection.rows.clear();
        self.tools_changed();
    }

    /// The Toggle Checkmark menu's key for shown row `row`: `pane:mark:line:nth`
    /// (`captured::key_of`).
    pub(super) fn captured_check_key(&self, c: &CapturedShown, row: usize) -> Option<String> {
        let (pane, &i) = (c.pane?, c.index.get(row)?);
        let list = &self.panes.get(&pane)?.screen.marks.records.get(&c.mark)?.captured;
        let (line, nth) = captured::key_of(list, i)?;
        Some(format!("{pane}:{}:{line}:{nth}", c.mark))
    }

    /// Toggle Checkmark on the entry its menu was opened on (`CO:377-384`).
    pub(super) fn toggle_captured_check(&mut self, pane: PaneId, mark: u32, key: (i64, usize)) {
        let Some(list) = self.panes.get_mut(&pane).and_then(|p| p.screen.marks.records.get_mut(&mark)).map(|m| &mut m.captured) else { return };
        if let Some(e) = captured::find(list, key).and_then(|i| list.get_mut(i)) {
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
        // The button is in this window: the popover stays on this window's monitor.
        let work = self.monitor_rects().map(|(w, _)| w);
        let Some(g) = self.gpu.as_ref() else { return };
        let origin = g.window.inner_position().unwrap_or_default();
        let at = (origin.x as f64 + f64::from(r.x + r.w / 2.0), origin.y as f64 + f64::from(r.y));
        match crate::status_popover_ui::StatusPopover::open(event_loop, &g.instance, &g.adapter, &g.device, &g.queue, patterns, merge, subtitle, at, work, dark) {
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
                // An edit still in the popover's field lands first.
                if let Some(v) = self.status_popover.as_mut().and_then(|p| p.pending_patterns()) {
                    self.set_status_patterns(&v);
                }
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
                // A focus loss closes before the field lets go: its edit lands now.
                if let Some(v) = self.status_popover.as_mut().and_then(|p| p.pending_patterns()) {
                    self.set_status_patterns(&v);
                }
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
                    Ok(()) => {
                        self.last_notified = Some(*pane);
                        self.last_notified_script = None;
                    }
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

impl App {
    // ---- Jobs (`JT`, `ToolJobs.m`) ----

    /// The shell Jobs is rooted at: the focused session's (`ToolJobs.m:101-104`).
    fn jobs_shell(&self) -> Option<u32> {
        self.focused_id().and_then(|id| self.panes.get(&id)).and_then(|p| p.shell_pid())
    }

    /// What Jobs shows: the watcher's last tree of the focused session's shell.
    pub(super) fn jobs_shown(&self) -> Shown {
        let Some(shell) = self.jobs_shell() else { return Shown::Jobs(JobsShown::default()) };
        let procs = self.jobs.tree(shell);
        let collapsed = self.belt.tools.get(&ToolId::Jobs).map(|t| t.collapsed.clone()).unwrap_or_default();
        let watched: BTreeSet<u32> = self.job_watches.iter().filter(|(pid, created, _)| procs.iter().any(|p| p.pid == *pid && p.created == *created)).map(|(pid, ..)| *pid).collect();
        let rows = tt::job_rows(&procs, shell, &collapsed, &watched);
        Shown::Jobs(JobsShown { shell: Some(shell), rows, procs })
    }

    /// What Jobs reads besides the query: the shell, the trees, the collapsed rows, the watches.
    pub(super) fn jobs_extra(&self, h: &mut impl Hasher) {
        (self.jobs_shell(), self.jobs.tree_gen(), &self.job_watches).hash(h);
        if let Some(t) = self.belt.tools.get(&ToolId::Jobs) {
            t.collapsed.hash(h);
        }
    }

    /// This window draws Jobs (`showing`) or not: the watcher keeps the trees of the shells
    /// some window's Jobs shows, and only those.
    pub(super) fn jobs_want(&mut self, showing: bool) {
        let Some(w) = self.current_window() else { return };
        let shell = if showing { self.jobs_shell() } else { None };
        let changed = match shell {
            Some(s) => self.jobs_wanted.insert(w, s) != Some(s),
            None => self.jobs_wanted.remove(&w).is_some(),
        };
        if changed {
            self.sync_tree_wants();
        }
    }

    fn sync_tree_wants(&mut self) {
        let want: HashSet<u32> = self.jobs_wanted.values().copied().collect();
        for s in want.difference(&self.jobs_wanting) {
            self.jobs.set_tree_wanted(*s, true);
        }
        for s in self.jobs_wanting.difference(&want) {
            self.jobs.set_tree_wanted(*s, false);
        }
        self.jobs_wanting = want;
    }

    /// Jobs' tick: the trees the watcher keeps follow the windows that show Jobs, a changed
    /// tree refreshes the tool, and each watched process that ended is announced ("Job
    /// Terminated", `iTermJobTerminationMonitor.swift:123-141`). When to look again.
    pub(super) fn tick_jobs_tool(&mut self) -> Option<Instant> {
        let live: HashSet<WindowId> = self.registry.order().into_iter().filter_map(|k| self.window_id_of_key(k)).collect();
        let before = self.jobs_wanted.len();
        self.jobs_wanted.retain(|w, _| live.contains(w));
        if self.jobs_wanted.len() != before {
            self.sync_tree_wants();
        }
        let generation = self.jobs.tree_gen();
        if generation != self.jobs_tree_seen {
            self.jobs_tree_seen = generation;
            self.tools_changed();
            self.redraw();
            self.redraw_all();
        }
        let now = Instant::now();
        if self.job_watches.is_empty() {
            self.jobs_watch_at = None;
        } else if self.jobs_watch_at.is_none_or(|t| now >= t) {
            self.jobs_watch_at = Some(now + JOBS_TICK);
            let (ended, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut self.job_watches).into_iter().partition(|(pid, created, _)| !crate::jobwatch::process_alive(*pid, *created));
            self.job_watches = kept;
            if !ended.is_empty() {
                self.tools_changed();
                self.redraw();
                let jobs: Vec<(String, u32)> = ended.into_iter().map(|(pid, _, name)| (name, pid)).collect();
                let (title, text) = tt::job_terminated_text(&jobs);
                alert(self.hwnd(), title, &text);
            }
        }
        let polling = (!self.jobs_wanting.is_empty()).then_some(now + JOBS_TICK);
        [polling, self.jobs_watch_at, self.collect_signals(now)].into_iter().flatten().min()
    }

    /// Send Signal's console events that finished: a failure shows in the banner. When to look
    /// again while some are still on their way.
    fn collect_signals(&mut self, now: Instant) -> Option<Instant> {
        let (done, waiting): (Vec<_>, Vec<_>) = std::mem::take(&mut self.signal_waits).into_iter().partition(|h| h.is_finished());
        self.signal_waits = waiting;
        for h in done {
            if let Err(e) = h.join().unwrap_or_else(|_| Err("the signal thread stopped".into())) {
                self.banners.set("jobs-signal", format!("Could not send the signal: {e}"));
                self.redraw();
            }
        }
        (!self.signal_waits.is_empty()).then(|| now + SIGNAL_POLL)
    }

    /// The selected rows' pids, each with its creation time, and whether one has children.
    fn jobs_selected(&mut self) -> (Option<u32>, Vec<(u32, u64)>, bool) {
        // Rebuilt first: the selection follows the rows it keys (`follow_jobs`).
        let shown = self.tool_shown(ToolId::Jobs);
        let rows = self.belt.tools.get(&ToolId::Jobs).map(|t| t.selection.rows.clone()).unwrap_or_default();
        let Shown::Jobs(j) = &*shown else { return (None, Vec::new(), false) };
        let picked: Vec<&JobRow> = rows.iter().filter_map(|&i| j.rows.get(i)).collect();
        let pids = picked.iter().filter_map(|r| j.proc(r.pid).map(|p| (p.pid, p.created))).collect();
        (j.shell, pids, picked.iter().any(|r| r.has_children))
    }

    /// Send: the selected processes get the popup's signal, after "Confirmation Needed"
    /// unless it was silenced (`JT:398-455`).
    pub(super) fn jobs_send(&mut self) {
        let (shell, pids, any_children) = self.jobs_selected();
        let (Some(shell), false) = (shell, pids.is_empty()) else { return };
        let index = self.belt.tools.get(&ToolId::Jobs).map_or(0, |t| t.signal).min(tt::SIGNALS.len() - 1);
        let req = SignalRequest { shell, pids, way: tt::signal_way(tt::SIGNALS[index]), any_children };
        if let Some((Some(w), ..)) = &self.jobs_confirm {
            return w.focus();
        }
        if self.jobs_confirm.is_some() {
            return;
        }
        let remembered = match self.warning_preempt(&signal_spec(req.pids.len(), any_children)) {
            // Only OK may be remembered.
            Preempt::Answer(_) => return self.jobs_signal(req),
            Preempt::Show => None,
            Preempt::ShowRemembered { saved_label } => Some(saved_label),
        };
        // The question opens on the next turn of the loop (it needs the event loop).
        self.jobs_confirm = Some((None, req, remembered));
    }

    /// Opens the pending "Confirmation Needed" question.
    pub(super) fn open_jobs_confirm(&mut self, event_loop: &ActiveEventLoop) {
        let Some((None, req, remembered)) = &self.jobs_confirm else { return };
        if self.gpu.is_none() {
            return;
        }
        let spec = signal_spec(req.pids.len(), req.any_children);
        match self.open_warning_window(event_loop, &spec, remembered.as_deref()) {
            Ok(w) => {
                if let Some((slot, ..)) = self.jobs_confirm.as_mut() {
                    *slot = Some(w);
                }
            }
            Err(e) => {
                self.jobs_confirm = None;
                self.banners.set("window", format!("Could not open the window: {e}"));
            }
        }
    }

    /// Sends a request's signal to each of its processes that still runs (the same pid and
    /// creation time) under the shell: never a process outside the pane's tree, never a reused
    /// pid. The watcher's tree is the ancestry; when it was dropped (Jobs hidden while the
    /// question was up), the processes under the shell are looked up now. A console event goes
    /// once: it reaches the whole console. When none is left, a banner says so.
    fn jobs_signal(&mut self, req: SignalRequest) {
        self.banners.clear("jobs-signal");
        let mut tree = self.jobs.tree(req.shell);
        if tree.is_empty() {
            match crate::jobwatch::descendants_now(req.shell) {
                Ok(t) => tree = t,
                Err(e) => self.banners.set("jobs-signal", format!("Could not read the running programs: {e}")),
            }
        }
        let (mut sent, mut failed, mut consoled) = (false, false, false);
        for (pid, created) in req.pids {
            let under_shell = pid == req.shell || tree.iter().any(|p| p.pid == pid && p.created == created);
            if !under_shell || !crate::jobwatch::process_alive(pid, created) {
                continue;
            }
            let console = matches!(req.way, SignalWay::CtrlC | SignalWay::CtrlBreak);
            if console && consoled {
                continue;
            }
            // A hosted pane's console belongs to tandem-host, which sends the event: Tandem
            // never attaches to it. A pane closed for Undo Close still has its console.
            let pane = self.panes.values().chain(self.undo.iter().flat_map(|u| u.panes())).find(|p| p.shell_pid() == Some(req.shell));
            let hosted = if console { pane.and_then(|p| p.hosted_signal(req.way == SignalWay::CtrlBreak)) } else { None };
            // A local console paused for an inline file's question is never attached to.
            let stop = pane.and_then(|p| p.console_pause_flag()).unwrap_or_default();
            let result = match hosted {
                Some(r) => r,
                None => crate::jobwatch::signal_process(pid, Some(created), req.way, stop).map(|s| {
                    if let crate::jobwatch::Sent::Later(h) = s {
                        self.signal_waits.push(h);
                    }
                }),
            };
            match result {
                Ok(()) => {
                    sent = true;
                    consoled |= console;
                }
                Err(e) => {
                    failed = true;
                    self.banners.set("jobs-signal", format!("Could not send the signal: {e}"));
                }
            }
        }
        if !sent && !failed {
            self.banners.set("jobs-signal", SIGNAL_NONE_LEFT);
        }
        self.tools_changed();
        self.redraw();
    }

    /// Inspect (and Space in the list): a popover with the selected process's id and command
    /// line above the button, or closes the open one (`JT` `jobTreeOutlineViewToggleInfoPopover:`).
    pub(super) fn jobs_inspect(&mut self, event_loop: &ActiveEventLoop) {
        if self.jobs_popover.take().is_some() || self.jobs_popover_closed.is_some_and(|t| t.elapsed() < REOPEN_GUARD) {
            return;
        }
        let shown = self.tool_shown(ToolId::Jobs);
        let row = self.belt.tools.get(&ToolId::Jobs).and_then(|t| t.selection.rows.first().copied());
        let Shown::Jobs(j) = &*shown else { return };
        let Some(p) = row.and_then(|r| j.rows.get(r)).and_then(|r| j.proc(r.pid)) else { return };
        let command = p.cmdline.clone().filter(|c| !c.is_empty()).unwrap_or_else(|| p.name.clone());
        let lines = vec![(INSPECT_PID.to_string(), p.pid.to_string()), (INSPECT_COMMAND.to_string(), command)];
        let Some(area) = self.tool_area(ToolId::Jobs) else { return };
        let (l, ..) = self.tool_layout(ToolId::Jobs, area);
        let Some(&(_, r)) = l.buttons.iter().find(|(b, _)| *b == tool_list::ToolButton::Inspect) else { return };
        let dark = self.dark();
        let work = self.monitor_rects().map(|(w, _)| w);
        let Some(g) = self.gpu.as_ref() else { return };
        let origin = g.window.inner_position().unwrap_or_default();
        let at = (origin.x as f64 + f64::from(r.x + r.w / 2.0), origin.y as f64 + f64::from(r.y));
        match crate::status_popover_ui::InfoPopover::open(event_loop, &g.instance, &g.adapter, &g.device, &g.queue, lines, at, work, dark) {
            Ok(w) => self.jobs_popover = Some(w),
            Err(e) => self.banners.set("window", format!("Could not open the window: {e}")),
        }
    }

    /// Events for Inspect's popover and the signal question. False for other windows.
    pub(super) fn jobs_window_event(&mut self, window: WindowId, event: &WindowEvent) -> bool {
        use crate::prompt_ui::ConfirmResult;
        if let Some(p) = self.jobs_popover.as_mut().filter(|p| p.id() == window) {
            if !p.event(event) {
                self.jobs_popover = None;
                self.jobs_popover_closed = Some(Instant::now());
            }
            return true;
        }
        let Some((Some(w), ..)) = self.jobs_confirm.as_mut().filter(|(w, ..)| w.as_ref().is_some_and(|w| w.id() == window)) else { return false };
        let answer = match event {
            WindowEvent::CloseRequested => w.close_answer(),
            WindowEvent::RedrawRequested => w.redraw(),
            other => {
                w.event(other);
                ConfirmResult::Open
            }
        };
        let ConfirmResult::Action { index, suppress } = answer else { return true };
        let Some((_, req, remembered)) = self.jobs_confirm.take() else { return true };
        let spec = signal_spec(req.pids.len(), req.any_children);
        if self.warning_window_answered(&spec, remembered.is_some(), index, suppress) {
            // Forgotten: the question again, without the remembered mode.
            self.jobs_confirm = Some((None, req, None));
        } else if index == 0 {
            self.jobs_signal(req);
        }
        true
    }

    /// A row's disclosure: a parent's subtree hides or shows; the selection follows its rows
    /// (the rebuild remaps it, `follow_jobs`).
    pub(super) fn toggle_job(&mut self, pid: u32, open: Option<bool>) {
        self.tool_shown(ToolId::Jobs);
        let st = self.tool_state(ToolId::Jobs);
        let closed = st.collapsed.contains(&pid);
        if open.unwrap_or(closed) {
            st.collapsed.remove(&pid);
        } else {
            st.collapsed.insert(pid);
        }
        self.tool_shown(ToolId::Jobs);
        self.redraw();
    }

    /// The process of Jobs' row `row` (pid and creation time), and whether it has children.
    pub(super) fn job_at(&mut self, row: usize) -> Option<(u32, u64, bool)> {
        let shown = self.tool_shown(ToolId::Jobs);
        let Shown::Jobs(j) = &*shown else { return None };
        j.rows.get(row).map(|r| (r.pid, j.proc(r.pid).map_or(0, |p| p.created), r.has_children))
    }

    /// A row's menu (`iTermJobTreeViewController.xib:133-154`): Copy Command, Copy Process ID,
    /// and Notify on Termination, checked while watched.
    pub(super) fn job_row_menu(&mut self, row: usize) {
        let Some((pid, created, _)) = self.job_at(row) else { return };
        let watched = self.job_watches.iter().any(|(p, c, _)| *p == pid && *c == created);
        let menu = muda::Menu::new();
        let ctx = super::toolbelt_tools_ui::CTX_ID;
        let items: [&dyn muda::IsMenuItem; 3] = [
            &muda::MenuItem::with_id(format!("{ctx}job-copy:{pid}:{created}"), "Copy Command", true, None),
            &muda::MenuItem::with_id(format!("{ctx}job-pid:{pid}:{created}"), "Copy Process ID", true, None),
            &muda::CheckMenuItem::with_id(format!("{ctx}job-notify:{pid}:{created}"), "Notify on Termination", true, watched, None),
        ];
        if let Err(e) = menu.append_items(&items) {
            self.banners.set("menu", format!("Toolbelt menu: {e}"));
        }
        let (x, y) = self.cursor;
        self.show_menu_at(menu, super::statusbar_content::MenuAt { x, y, above: false });
    }

    /// The signal popup's menu: the six signals, the chosen one checked.
    pub(super) fn job_signal_menu(&mut self, at: super::statusbar_content::MenuAt) {
        let pick = self.belt.tools.get(&ToolId::Jobs).map_or(0, |t| t.signal);
        let menu = muda::Menu::new();
        let ctx = super::toolbelt_tools_ui::CTX_ID;
        for (i, s) in tt::SIGNALS.iter().enumerate() {
            if let Err(e) = menu.append(&muda::CheckMenuItem::with_id(format!("{ctx}signal:{i}"), s.name(), true, i == pick, None)) {
                self.banners.set("menu", format!("Jobs menu: {e}"));
            }
        }
        self.show_menu_at(menu, at);
    }

    /// Copy Command and Copy Process ID: the process's command line (else its name), or its id.
    pub(super) fn copy_job(&mut self, pid: u32, created: u64, command: bool) {
        let shown = self.tool_shown(ToolId::Jobs);
        let Shown::Jobs(j) = &*shown else { return };
        let Some(p) = j.proc(pid).filter(|p| p.created == created) else { return };
        let text = if command { p.cmdline.clone().filter(|c| !c.is_empty()).unwrap_or_else(|| p.name.clone()) } else { pid.to_string() };
        if let Some(cb) = self.clipboard.as_mut() {
            if let Err(e) = cb.set_text(text) {
                self.banners.set("clipboard", format!("Could not copy: {e}"));
            }
        }
    }

    /// Notify on Termination: watches the process (its pid and creation time), or stops; one
    /// already gone says so ("Cannot Notify on Termination").
    pub(super) fn toggle_job_watch(&mut self, pid: u32, created: u64) {
        if let Some(i) = self.job_watches.iter().position(|(p, c, _)| *p == pid && *c == created) {
            self.job_watches.remove(i);
        } else {
            let shown = self.tool_shown(ToolId::Jobs);
            let Shown::Jobs(j) = &*shown else { return };
            let found = j.proc(pid).filter(|p| p.created == created).map(|p| (p.created, crate::proctree::strip_exe(&p.name).to_string()));
            match found {
                Some((created, name)) if crate::jobwatch::process_alive(pid, created) => self.job_watches.push((pid, created, name)),
                other => {
                    let name = other.map(|(_, n)| n).unwrap_or_default();
                    let (title, text) = cannot_notify_text(&name, pid);
                    alert(self.hwnd(), title, &text);
                }
            }
        }
        self.tools_changed();
        self.redraw();
    }

    // ---- Notes (`TN`) ----

    /// The note the editor should hold now, and its text: the global note (read from its file
    /// on first use), or the focused session's.
    fn note_wanted(&mut self) -> (NoteSource, String) {
        match self.belt.notes_mode {
            Mode::Global => {
                if let Some(e) = self.global_note.ensure_loaded() {
                    self.banners.set("notes", e);
                }
                (NoteSource::Global, self.global_note.text.clone())
            }
            Mode::Session => {
                let pane = self.focused_id();
                let text = pane.and_then(|id| self.panes.get(&id)).map(|p| p.session_note.clone()).unwrap_or_default();
                (NoteSource::Session(pane), text)
            }
        }
    }

    /// The editor follows its note: another note (a mode switch, or a new key session)
    /// loads with a fresh undo (`TN` `modeChanged:`, `loadSessionNote`); the same note keeps
    /// the caret, the selection and the undo unless its text changed elsewhere.
    pub(super) fn notes_sync(&mut self) {
        let (source, text) = self.note_wanted();
        if self.belt.note_source != Some(source) {
            self.belt.note_source = Some(source);
            self.belt.note_edit.load(&text);
            self.belt.note_scroll = 0.0;
        } else {
            self.belt.note_edit.set_text(&text);
        }
    }

    /// Runs `f` with the editor and its width measure: the toolbelt font, a tab as four spaces.
    fn with_note_measure<R>(&mut self, f: impl FnOnce(&mut super::toolbelt_ui::Belt, &dyn Fn(&str) -> f32, u64) -> R) -> R {
        let scale = self.scale();
        let renderer = self.gpu.as_mut().map(|g| RefCell::new(&mut g.renderer));
        // The measure's own key, for the editor's wrap cache (`text_edit::WrapCache`).
        let font = match &renderer {
            Some(r) => r.borrow().toolbelt_font_key(scale),
            None => 1 << 63 | u64::from(scale.to_bits()),
        };
        let measure = |s: &str| {
            let s = s.replace('\t', TAB_AS);
            match &renderer {
                Some(r) => r.borrow_mut().toolbelt_width(&s, scale),
                None => s.chars().count() as f32 * 7.0 * scale,
            }
        };
        f(&mut self.belt, &measure, font)
    }

    /// The editor's text changed: the note it holds takes it (the global note is saved and
    /// every window's editor follows; a session's note, and its open Session Note window, follow;
    /// without a session nothing is stored).
    fn notes_edited(&mut self) {
        let text = self.belt.note_edit.text.clone();
        match self.belt.note_source {
            Some(NoteSource::Global) => match self.global_note.edited(text, Instant::now()) {
                Ok(Wrote::Written) => self.banners.clear("notes"),
                Ok(_) => {}
                Err(e) => self.banners.set("notes", e),
            },
            Some(NoteSource::Session(Some(pane))) => {
                if let Some(p) = self.panes.get_mut(&pane) {
                    p.session_note = text.clone();
                }
                if let Some((w, p, None)) = self.note_win.as_mut() {
                    if *p == pane {
                        w.set_text(&text);
                    }
                }
            }
            _ => {}
        }
        self.tools_changed();
        self.redraw_all();
    }

    /// A Paste key filled with the clipboard's text; any other key as it is. Shared by the
    /// Notes tool and the inline chat's input.
    pub(super) fn with_clipboard_text(&mut self, key: EditKey) -> EditKey {
        match key {
            EditKey::Paste(_) => EditKey::Paste(self.clipboard.as_mut().and_then(|cb| cb.get_text().ok()).unwrap_or_default()),
            k => k,
        }
    }

    /// Puts an editor's Copy or Cut on the clipboard; a failure shows as a banner.
    pub(super) fn set_clipboard_text(&mut self, text: String) {
        if let Some(cb) = self.clipboard.as_mut() {
            if let Err(e) = cb.set_text(text) {
                self.banners.set("clipboard", format!("Could not copy: {e}"));
            }
        }
    }

    /// Keys while the Notes editor has the keyboard (`text_edit::takes`): typing, Enter, Tab,
    /// the edit keys and their Ctrl chords are the editor's; Esc gives the keyboard back;
    /// other chords pass. True when taken.
    pub(super) fn notes_key(&mut self, ev: &KeyEvent) -> bool {
        let mods = self.mods;
        if is_modifier(&ev.logical_key) {
            return true;
        }
        if ev.logical_key == WKey::Named(NamedKey::Escape) && !(mods.ctrl || mods.alt || mods.cmd) {
            self.belt.focus = None;
            self.redraw();
            return true;
        }
        let Some((key, ctrl, alt)) = edit_key(&ev.logical_key, ev.text.as_deref(), mods) else { return !(mods.ctrl || mods.alt || mods.cmd) };
        if !text_edit::takes(ctrl, alt, mods.cmd, &key) {
            return false;
        }
        let key = self.with_clipboard_text(key);
        let Some(area) = self.tool_area(ToolId::Notes) else { return true };
        self.notes_sync();
        let l = note_layout(area, self.scale());
        let line_h = self.list_metrics().line.max(1.0);
        let cut = key == EditKey::Cut;
        let (out, caret_line) = self.with_note_measure(|belt, measure, font| {
            let lines = belt.note_wrap.wrap(&belt.note_edit.text, l.text.w, font, measure);
            belt.note_edit.page_lines = ((l.text.h / line_h).floor() as usize).max(1);
            let out = belt.note_edit.key(key, mods.shift, &lines, measure);
            let lines = belt.note_wrap.wrap(&belt.note_edit.text, l.text.w, font, measure);
            (out, belt.note_edit.caret_line(&lines, measure).0)
        });
        match out {
            EditOut::Changed => self.notes_edited(),
            EditOut::Copied(text) => {
                self.set_clipboard_text(text);
                // A Cut changed the text as well.
                if cut {
                    self.notes_edited();
                }
            }
            EditOut::Moved | EditOut::Kept => {}
        }
        // The caret stays in view.
        let top = caret_line as f32 * line_h;
        let scroll = &mut self.belt.note_scroll;
        if top < *scroll {
            *scroll = top;
        } else if top + line_h > *scroll + l.text.h {
            *scroll = top + line_h - l.text.h;
        }
        self.redraw();
        true
    }

    /// The byte index under a point of the editor, and whether it is a soft-wrapped line's end.
    fn note_index_at(&mut self, l: &NoteLayout, x: f32, y: f32) -> (usize, bool) {
        let line_h = self.list_metrics().line.max(1.0);
        let scroll = self.belt.note_scroll;
        self.with_note_measure(|belt, measure, font| {
            let text = &belt.note_edit.text;
            let lines = belt.note_wrap.wrap(text, l.text.w, font, measure);
            let line = (((y - l.text.y + scroll) / line_h).floor().max(0.0) as usize).min(lines.len().saturating_sub(1));
            let at = text_edit::index_at(text, &lines, line, x - l.text.x, measure);
            (at, text_edit::ends_line(&lines, line, at))
        })
    }

    /// A press in Notes: a segment switches the mode; the editor takes the keyboard and places
    /// the caret (Shift extends, a double-click selects a word), and a drag selects.
    pub(super) fn notes_press(&mut self, area: Rect, button: MouseButton, x: f32, y: f32) {
        if button != MouseButton::Left {
            return;
        }
        let l = note_layout(area, self.scale());
        if let Some(i) = l.segments.iter().position(|r| r.contains(x, y)) {
            self.belt.notes_mode = notes::MODES[i].0;
            self.notes_sync();
            self.tools_changed();
            return self.redraw();
        }
        if !l.frame.contains(x, y) {
            return;
        }
        self.belt_take_focus(ToolId::Notes, Focus::Field);
        self.notes_sync();
        let (at, end) = self.note_index_at(&l, x, y);
        let shift = self.mods.shift;
        self.belt.note_edit.click(at, end, shift, now_ms(), super::clippings_ui::double_click_ms());
        self.drag = Drag::Belt(BeltDrag::Note);
        self.redraw();
    }

    /// A selection drag in the editor.
    pub(super) fn notes_drag(&mut self, x: f32, y: f32) {
        let Some(area) = self.tool_area(ToolId::Notes) else { return };
        let l = note_layout(area, self.scale());
        let (at, end) = self.note_index_at(&l, x, y);
        self.belt.note_edit.drag_to(at, end);
    }

    /// The wheel over the editor scrolls it, `dy` lines.
    pub(super) fn notes_wheel(&mut self, dy: f32) {
        self.belt.note_scroll = (self.belt.note_scroll - dy * self.list_metrics().line).max(0.0);
        self.redraw();
    }

    /// Draws Notes into `area`: the mode control, then the editor with its text, selection,
    /// caret and scroller.
    pub(super) fn draw_notes(&mut self, frame: &mut Frame, area: Rect, cell: (f32, f32)) {
        self.notes_sync();
        let scale = self.scale();
        let dark = self.dark();
        let m = self.list_metrics();
        let line_h = m.line.max(1.0);
        let l = note_layout(area, scale);
        let focused = self.belt.focus.is_some_and(|(t, _)| t == ToolId::Notes);
        let selected = notes::MODES.iter().position(|(mode, _)| *mode == self.belt.notes_mode).unwrap_or(0);
        let clip = Rect { x: l.frame.x + 1.0, y: l.frame.y + 1.0, w: (l.frame.w - 2.0).max(0.0), h: (l.frame.h - 2.0).max(0.0) };
        let Some(g) = self.gpu.as_mut() else { return };
        let renderer = RefCell::new(&mut g.renderer);
        let measure = |s: &str| renderer.borrow_mut().toolbelt_width(&s.replace('\t', TAB_AS), scale);
        let font = renderer.borrow().toolbelt_font_key(scale);
        let label_w = |s: &str| renderer.borrow_mut().text_width(s, true, m.text_scale);
        let belt = &mut self.belt;
        let edit = &mut belt.note_edit;
        let lines = belt.note_wrap.wrap(&edit.text, l.text.w, font, &measure);
        edit.page_lines = ((l.text.h / line_h).floor() as usize).max(1);
        let content = lines.len() as f32 * line_h;
        belt.note_scroll = belt.note_scroll.clamp(0.0, (content - l.text.h).max(0.0));
        let scroll = belt.note_scroll;
        let y_of = |i: usize| l.text.y + i as f32 * line_h - scroll;
        let text = &edit.text;
        let shown: Vec<(f32, f32, String)> = lines
            .iter()
            .enumerate()
            .filter(|(i, _)| y_of(*i) + line_h > clip.y && y_of(*i) < clip.y + clip.h)
            .map(|(i, v)| (l.text.x, y_of(i), text[v.start..v.end].replace('\t', TAB_AS)))
            .collect();
        let selection: Vec<Rect> = text_edit::selection_spans(edit.selection(), text, &lines, 0..lines.len(), &measure).into_iter().map(|(i, x, w)| Rect { x: l.text.x + x, y: y_of(i), w, h: line_h }).collect();
        // Trailing spaces hang past the edge: the caret stays inside the text's room.
        let caret = focused.then(|| {
            let (line, x) = edit.caret_line(&lines, &measure);
            Rect { x: l.text.x + x.min((l.text.w - 1.0).max(0.0)), y: y_of(line), w: 1.0, h: line_h }
        });
        let thumb = (content > l.text.h && l.text.h > 0.0).then(|| {
            let h = (clip.h * l.text.h / content).max(12.0 * scale).min(clip.h);
            let y = clip.y + (clip.h - h) * scroll / (content - l.text.h);
            Rect { x: clip.x + clip.w - 4.0 * scale, y, w: 3.0 * scale, h }
        });
        let segments: Vec<(Rect, &str, f32)> = notes::MODES.iter().zip(l.segments).map(|((_, label), r)| (r, *label, label_w(label))).collect();
        let d = scene::NoteDraw { segments: &segments, selected, frame: l.frame, clip, lines: &shown, selection: &selection, caret, focused, thumb, dark, label_scale: m.text_scale };
        scene::note_editor(frame, &d, cell, m.text_scale, scale);
    }

    /// Writes the note now if a change is waiting (Tandem quits).
    pub(super) fn flush_notes(&mut self) {
        if let Err(e) = self.global_note.flush() {
            self.banners.set("notes", e);
        }
    }

    /// The global note's debounced writes and retries. When the next is due.
    pub(super) fn tick_notes(&mut self) -> Option<Instant> {
        match self.global_note.tick(Instant::now()) {
            Ok(Wrote::Written) => self.banners.clear("notes"),
            Ok(Wrote::Adopted) => {
                // The file was readable again and holds the note: every editor shows it.
                self.tools_changed();
                self.redraw();
                self.redraw_all();
            }
            Ok(_) => {}
            Err(e) => self.banners.set("notes", e),
        }
        self.global_note.due()
    }

    // ---- Codecierge (`CG`) ----

    /// The gate for the default model and whether it has function calling, from the models found
    /// so far (no probe: this runs every frame while the tool shows, P2).
    pub(super) fn codecierge_gate(&self) -> (bool, bool) {
        let found = self.ai_discovery.all();
        let Some(m) = crate::ai_model::configured(&self.globals, crate::ai_catalog::bundled(), &found) else { return (false, false) };
        (crate::chat_gate::gate_open(&self.globals, &crate::chat_gate::endpoint_of(&m)), crate::ai_model::has(&m, crate::ai_catalog::Feature::FunctionCalling))
    }

    /// The focused pane's session guid.
    fn codecierge_guid(&self) -> Option<String> {
        self.focused_id().and_then(|id| self.panes.get(&id)).map(|p| p.session_guid.clone())
    }

    /// The focused pane's Codecierge session, created on first use (`CG:554-567`): its first
    /// stage is goal setting only when the whole gate is open and a key is stored (`CG:412-418`).
    pub(super) fn codecierge_session(&mut self) -> Option<&mut crate::codecierge::Session> {
        let guid = self.codecierge_guid()?;
        if !self.codecierge.contains_key(&guid) {
            let (allowed, _) = self.codecierge_gate();
            let has_key = self.ai_has_key();
            self.codecierge.insert(guid.clone(), crate::codecierge::Session::new(&guid, allowed, has_key));
        }
        self.codecierge.get_mut(&guid)
    }

    /// What the tool shows now. A gate that turned off shows onboarding at once; the full stop
    /// (`CG:476-480`, ruling R12e-3) is `codecierge_gate_tick`'s, once per loop pass.
    pub(super) fn codecierge_view(&mut self) -> View {
        let (admin, ai) = (self.globals.bool(crate::ai_settings::ADMIN), self.globals.bool(crate::ai_settings::ENABLE_AI));
        let (open, calling) = self.codecierge_gate();
        match self.codecierge_session() {
            Some(s) => {
                if !open && s.stage != Stage::Onboarding {
                    let mut shown = s.clone();
                    shown.stage = Stage::Onboarding;
                    return crate::codecierge::view(&shown, admin, ai, calling);
                }
                crate::codecierge::view(s, admin, ai, calling)
            }
            None => View::Onboarding(crate::codecierge::onboarding(admin, ai)),
        }
    }

    /// Get Started (ruling R12e-1): the gate for the model's own endpoint runs first (its alert
    /// explains a refusal and the session stays), then a key is asked for only when none is
    /// stored.
    fn codecierge_get_started(&mut self, guid: &str) {
        let Some(model) = self.ai_model_now() else {
            self.ai_check_endpoint(None, false);
            return;
        };
        let endpoint = crate::chat_gate::endpoint_of(&model);
        let open = self.ai_check_endpoint(Some(&endpoint), false);
        match crate::codecierge::get_started(open, self.ai_has_key()) {
            GetStarted::Stay => {}
            GetStarted::AskKey => {
                let found = self.ai_found();
                let vendor = crate::ai_models::effective_vendor(&self.globals, crate::ai_catalog::bundled(), &found);
                self.ask_ai_key(vendor, Some(&endpoint), super::ai_app::KeyThen::Codecierge(guid.to_string()));
            }
            GetStarted::Goal => self.codecierge_goal(guid),
        }
    }

    /// The session goes to its goal view.
    pub(super) fn codecierge_goal(&mut self, guid: &str) {
        if let Some(s) = self.codecierge.get_mut(guid) {
            s.stage = Stage::GoalSetting;
        }
        self.tools_changed();
        self.redraw_all();
    }

    /// End Task (`CG`): the run, the pending call and the queue stop (ruling R12e-3), and the
    /// goal view shows with the goal kept.
    fn codecierge_end(&mut self, guid: &str) {
        self.codecierge_stop(guid, Stage::GoalSetting);
        if let Some(s) = self.codecierge.get_mut(guid) {
            s.transcript.clear();
            s.reply.clear();
        }
        self.belt.cc_guid = None;
        self.tools_changed();
        self.redraw_all();
    }

    /// A reply (Enter or the send button): the "You" section is appended and the field clears
    /// (`CG:595-602`), then the text is queued for the model.
    fn codecierge_press_send(&mut self, guid: &str) {
        let Some(s) = self.codecierge.get_mut(guid) else { return };
        let text = s.reply.trim().to_string();
        if text.is_empty() {
            return;
        }
        s.transcript.push_str(&crate::codecierge::you(&text));
        s.reply.clear();
        self.belt.cc_reply.load("");
        self.belt.cc_scroll = 0.0;
        self.tools_changed();
        self.redraw_all();
        self.codecierge_send(guid, text);
    }

    /// The Start button: the Privacy Warning unless it was accepted, then the start.
    fn codecierge_press_start(&mut self, guid: &str) {
        if self.globals.bool(crate::codecierge::PRIVACY_KEY) {
            return self.codecierge_start(guid);
        }
        self.ask_ai(crate::ai_gate::Alert {
            heading: crate::codecierge::PRIVACY_HEADING.to_string(),
            text: crate::codecierge::PRIVACY_TEXT.to_string(),
            buttons: vec!["OK", "Cancel"],
            silence: None,
            then: crate::ai_gate::Then::CodeciergePrivacy(guid.to_string()),
            gate: false,
            close: None,
        });
    }

    /// The editors hold the focused session's goal and reply; another session's text loads with
    /// a fresh undo, the same session's keeps the caret unless its text changed elsewhere.
    fn codecierge_sync(&mut self) {
        let Some(s) = self.codecierge_session().map(|s| (s.guid.clone(), s.goal.clone(), s.reply.clone())) else { return };
        let (guid, goal, reply) = s;
        if self.belt.cc_guid.as_deref() != Some(guid.as_str()) {
            self.belt.cc_guid = Some(guid);
            self.belt.cc_goal.load(&goal);
            self.belt.cc_reply.load(&reply);
            self.belt.cc_goal_scroll = 0.0;
            self.belt.cc_reply_scroll = 0.0;
        } else {
            self.belt.cc_goal.set_text(&goal);
            self.belt.cc_reply.set_text(&reply);
        }
    }

    /// The text the editors hold goes back to the session.
    fn codecierge_edited(&mut self) {
        let (goal, reply) = (self.belt.cc_goal.text.clone(), self.belt.cc_reply.text.clone());
        if let Some(s) = self.codecierge_session() {
            s.goal = goal;
            s.reply = reply;
        }
        self.tools_changed();
        self.redraw_all();
    }

    /// Whether the focused session is in its suggestion view (the reply editor), else the goal.
    fn codecierge_running(&mut self) -> bool {
        self.codecierge_session().is_some_and(|s| s.stage == Stage::Running)
    }

    /// The laid-out parts of Codecierge in `area` for `view`.
    fn codecierge_geometry(&mut self, area: Rect, view: &View) -> CcGeo {
        let scale = self.scale();
        let m = self.list_metrics();
        let dead = Rect { x: 0.0, y: 0.0, w: 0.0, h: 0.0 };
        let (label, enabled) = match view {
            View::Onboarding(s) => codecierge_button(*s),
            View::Goal { start_enabled, .. } => (crate::codecierge::START, *start_enabled),
            View::Running { .. } => (crate::codecierge::END_TASK, true),
        };
        let mut geo = CcGeo { lines: Vec::new(), button: dead, label, label_w: 0.0, enabled, goal: None, run: None, goal_text: String::new(), check_label_w: 0.0 };
        let Some(g) = self.gpu.as_mut() else { return geo };
        let renderer = RefCell::new(&mut g.renderer);
        let width = |s: &str| renderer.borrow_mut().text_width(s, true, m.text_scale);
        let inner = Rect { x: area.x + 4.0 * scale, w: (area.w - 8.0 * scale).max(0.0), ..area };
        geo.label_w = width(label);
        let centered = |lines: Vec<String>, top: f32| -> Vec<(f32, f32, String)> {
            lines
                .into_iter()
                .enumerate()
                .map(|(i, t)| {
                    let w = width(&t);
                    (inner.x + ((inner.w - w) / 2.0).max(0.0), top + i as f32 * m.line, t)
                })
                .collect()
        };
        match view {
            View::Onboarding(state) => {
                let wrapped = wrap_words(crate::codecierge::codecierge_text(*state), inner.w, &width);
                let (top, button) = codecierge_layout(area, wrapped.len(), m.line, geo.label_w, scale);
                geo.lines = centered(wrapped, top);
                geo.button = button;
            }
            View::Goal { .. } => {
                let wrapped = wrap_words(crate::codecierge::GOAL_PROMPT, inner.w, &width);
                let l = goal_layout(area, wrapped.len(), m.line, geo.label_w, scale);
                geo.lines = wrapped.into_iter().enumerate().map(|(i, t)| (inner.x, l.top + i as f32 * m.line, t)).collect();
                geo.button = l.start;
                geo.check_label_w = width(crate::codecierge::AUTO_LABEL);
                geo.goal = Some(l);
            }
            View::Running { goal, busy, .. } => {
                let l = run_layout(area, m.line, geo.label_w, scale);
                let x = if *busy { l.goal_x } else { l.spinner.x };
                geo.goal_text = fit_tail(goal, (area.x + area.w - 4.0 * scale - x).max(0.0), &width);
                geo.button = l.end;
                geo.run = Some(l);
            }
        }
        geo
    }

    /// The transcript laid out for a room, kept until the text, the width or the theme changes.
    /// The content's height.
    fn codecierge_doc(&mut self, transcript: &str, room: Rect) -> f32 {
        let scale = self.scale();
        let m = self.list_metrics();
        let dark = self.dark();
        let width = (room.w - 2.0 * CC_DOC_PAD * scale).max(1.0);
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (transcript, width.to_bits(), dark, m.text_scale.to_bits(), self.cell().1.to_bits()).hash(&mut h);
        let key = h.finish();
        if self.belt.cc_doc.as_ref().is_some_and(|(k, _)| *k == key) {
            return self.belt.cc_doc.as_ref().map_or(0.0, |(_, l)| l.height);
        }
        let (text, back) = scene::codecierge_colors(dark);
        let palette = crate::palette::Palette { fg: text, bg: back, ..crate::palette::Palette::default() };
        let doc = crate::porthole::render::render(transcript, &crate::porthole::detect::Language::Markdown, &palette, dark);
        let cell_h = self.cell().1 * m.text_scale;
        let Some(g) = self.gpu.as_mut() else { return 0.0 };
        let r = RefCell::new(&mut g.renderer);
        let measure = |t: &str, f: crate::porthole::render::Face, s: f32| r.borrow_mut().text_width(t, f == crate::porthole::render::Face::Prose, s * m.text_scale);
        let layout = crate::porthole::render::layout(&doc, width, cell_h, &measure, false);
        let height = layout.height;
        self.belt.cc_doc = Some((key, layout));
        height
    }

    /// Where the transcript's text starts (top left) for a room and its content height, with
    /// `cc_scroll` (distance from the end) applied and kept in range.
    fn codecierge_origin(&mut self, room: Rect, content: f32) -> (f32, f32) {
        let scale = self.scale();
        let pad = CC_DOC_PAD * scale;
        let inner_h = (room.h - 2.0 * pad).max(0.0);
        let max = (content - inner_h).max(0.0);
        self.belt.cc_scroll = self.belt.cc_scroll.clamp(0.0, max);
        (room.x + pad, room.y + pad - (max - self.belt.cc_scroll))
    }

    pub(super) fn draw_codecierge(&mut self, frame: &mut Frame, area: Rect, cell: (f32, f32)) {
        self.codecierge_sync();
        let view = match &*self.tool_shown(ToolId::Codecierge) {
            Shown::Codecierge(v) => v.clone(),
            _ => self.codecierge_view(),
        };
        let geo = self.codecierge_geometry(area, &view);
        let scale = self.scale();
        let dark = self.dark();
        let m = self.list_metrics();
        let line_h = m.line.max(1.0);
        let focused = self.belt.focus.is_some_and(|(t, _)| t == ToolId::Codecierge);
        let mut d = scene::CodeciergeDraw { lines: &geo.lines, clip: area, button: geo.button, label: geo.label, label_w: geo.label_w, enabled: geo.enabled, dark, goal: None, run: None };
        match &view {
            View::Onboarding(_) => scene::codecierge(frame, &d, cell, m.text_scale, scale),
            View::Goal { auto_box, auto_enabled, .. } => {
                let Some(l) = &geo.goal else { return scene::codecierge(frame, &d, cell, m.text_scale, scale) };
                let parts = self.codecierge_field_parts(false, l.frame, l.text, focused);
                let check_label = (l.label_x, l.label_y, crate::codecierge::AUTO_LABEL);
                let placeholder = self.belt.cc_goal.text.is_empty().then_some((l.text.x, l.text.y, crate::codecierge::GOAL_PLACEHOLDER));
                let edit = scene::NoteDraw { segments: &[], selected: 0, frame: l.frame, clip: parts.clip, lines: &parts.lines, selection: &parts.selection, caret: parts.caret, focused, thumb: parts.thumb, dark, label_scale: m.text_scale };
                d.goal = Some(scene::GoalDraw { field: scene::FieldDraw { edit, placeholder }, check: l.check, checked: *auto_box, check_enabled: *auto_enabled, check_label });
                scene::codecierge(frame, &d, cell, m.text_scale, scale);
            }
            View::Running { transcript, busy, reply, .. } => {
                let Some(l) = &geo.run else { return scene::codecierge(frame, &d, cell, m.text_scale, scale) };
                let content = self.codecierge_doc(transcript, l.room);
                let (ox, oy) = self.codecierge_origin(l.room, content);
                let room = Rect { x: l.room.x + 1.0, y: l.room.y + 1.0, w: (l.room.w - 2.0).max(0.0), h: (l.room.h - 2.0).max(0.0) };
                let parts = self.codecierge_field_parts(true, l.reply, l.reply_text, focused);
                let placeholder = self.belt.cc_reply.text.is_empty().then_some((l.reply_text.x, l.reply_text.y, crate::codecierge::RESPONSE_PLACEHOLDER));
                let edit = scene::NoteDraw { segments: &[], selected: 0, frame: l.reply, clip: parts.clip, lines: &parts.lines, selection: &parts.selection, caret: parts.caret, focused, thumb: parts.thumb, dark, label_scale: m.text_scale };
                let lines: &[crate::porthole::render::VisualLine] = match &self.belt.cc_doc {
                    Some((_, layout)) => layout.visible(room.y - oy, room.y + room.h - oy),
                    None => &[],
                };
                let thumb = (content > room.h).then(|| {
                    let max = content - room.h;
                    let h = (room.h * room.h / content).max(12.0 * scale).min(room.h);
                    Rect { x: room.x + room.w - 4.0 * scale, y: room.y + (room.h - h) * (1.0 - self.belt.cc_scroll / max), w: 3.0 * scale, h }
                });
                let spinner = busy.then_some((l.spinner, ((now_ms() / 100) % 8) as u32));
                d.run = Some(scene::RunDraw {
                    goal: (if *busy { l.goal_x } else { l.spinner.x }, l.spinner.y, &geo.goal_text),
                    spinner,
                    lines,
                    origin: (ox, oy),
                    room,
                    band: (room.x, room.w),
                    busy: *busy,
                    thumb,
                    reply: scene::FieldDraw { edit, placeholder },
                    send: l.send,
                    send_enabled: !reply.trim().is_empty(),
                });
                scene::codecierge(frame, &d, cell, m.text_scale, scale);
            }
        }
        let _ = line_h;
    }

    /// One editor's drawn parts (the Notes editor's rules): its lines, selection, caret and
    /// scroller in `text`, inside `frame`.
    fn codecierge_field_parts(&mut self, reply: bool, frame: Rect, text: Rect, focused: bool) -> FieldParts {
        let scale = self.scale();
        let line_h = self.list_metrics().line.max(1.0);
        let clip = Rect { x: frame.x + 1.0, y: frame.y + 1.0, w: (frame.w - 2.0).max(0.0), h: (frame.h - 2.0).max(0.0) };
        let Some(g) = self.gpu.as_mut() else { return FieldParts { clip, lines: Vec::new(), selection: Vec::new(), caret: None, thumb: None } };
        let renderer = RefCell::new(&mut g.renderer);
        let measure = |s: &str| renderer.borrow_mut().toolbelt_width(&s.replace('\t', TAB_AS), scale);
        let font = renderer.borrow().toolbelt_font_key(scale);
        let belt = &mut self.belt;
        let (edit, wrap, scroll) = if reply { (&mut belt.cc_reply, &mut belt.cc_reply_wrap, &mut belt.cc_reply_scroll) } else { (&mut belt.cc_goal, &mut belt.cc_goal_wrap, &mut belt.cc_goal_scroll) };
        let lines = wrap.wrap(&edit.text, text.w, font, &measure);
        edit.page_lines = ((text.h / line_h).floor() as usize).max(1);
        let content = lines.len() as f32 * line_h;
        *scroll = scroll.clamp(0.0, (content - text.h).max(0.0));
        let top = *scroll;
        let y_of = |i: usize| text.y + i as f32 * line_h - top;
        let src = &edit.text;
        let shown: Vec<(f32, f32, String)> = lines
            .iter()
            .enumerate()
            .filter(|(i, _)| y_of(*i) + line_h > clip.y && y_of(*i) < clip.y + clip.h)
            .map(|(i, v)| (text.x, y_of(i), src[v.start..v.end].replace('\t', TAB_AS)))
            .collect();
        let selection: Vec<Rect> = text_edit::selection_spans(edit.selection(), src, &lines, 0..lines.len(), &measure).into_iter().map(|(i, x, w)| Rect { x: text.x + x, y: y_of(i), w, h: line_h }).collect();
        let caret = focused.then(|| {
            let (line, x) = edit.caret_line(&lines, &measure);
            Rect { x: text.x + x.min((text.w - 1.0).max(0.0)), y: y_of(line), w: 1.0, h: line_h }
        });
        let thumb = (content > text.h && text.h > 0.0).then(|| {
            let h = (clip.h * text.h / content).max(12.0 * scale).min(clip.h);
            let y = clip.y + (clip.h - h) * top / (content - text.h);
            Rect { x: clip.x + clip.w - 4.0 * scale, y, w: 3.0 * scale, h }
        });
        FieldParts { clip, lines: shown, selection, caret, thumb }
    }

    /// A press in Codecierge: onboarding's button, the goal view's field, checkbox and Start, the
    /// suggestion view's code blocks, reply field, send and End Task.
    pub(super) fn codecierge_press(&mut self, area: Rect, button: MouseButton, x: f32, y: f32) {
        if button != MouseButton::Left {
            return;
        }
        let view = match &*self.tool_shown(ToolId::Codecierge) {
            Shown::Codecierge(v) => v.clone(),
            _ => self.codecierge_view(),
        };
        let Some(guid) = self.codecierge_session().map(|s| s.guid.clone()) else {
            // No session pane: only Reveal Setting works.
            if let View::Onboarding(CodeciergeState::NeedsAi) = view {
                let geo = self.codecierge_geometry(area, &view);
                if geo.button.contains(x, y) {
                    self.ai_reveal = Some(crate::settings_ai::SubTab::General);
                }
            }
            return;
        };
        let geo = self.codecierge_geometry(area, &view);
        match &view {
            View::Onboarding(state) => {
                if !geo.enabled || !geo.button.contains(x, y) {
                    return;
                }
                match state {
                    CodeciergeState::NeedsAi => self.ai_reveal = Some(crate::settings_ai::SubTab::General),
                    CodeciergeState::Ready => self.codecierge_get_started(&guid),
                    CodeciergeState::AdminDisabled => {}
                }
            }
            View::Goal { auto_box, auto_enabled, start_enabled, .. } => {
                let Some(l) = &geo.goal else { return };
                let check_row = Rect { w: l.label_x + geo.check_label_w - l.check.x, ..l.check };
                if l.start.contains(x, y) {
                    if *start_enabled {
                        self.codecierge_press_start(&guid);
                    }
                } else if (*auto_enabled || *auto_box) && check_row.contains(x, y) {
                    // One Danger! per session at a time: a click while it waits asks nothing more.
                    let then = crate::ai_gate::Then::CodeciergeDanger(guid.clone());
                    let ask = self.codecierge.get_mut(&guid).is_some_and(|s| s.box_clicked()) && !self.ai_alert_pending(&then);
                    if ask {
                        self.ask_ai(crate::ai_gate::Alert {
                            heading: crate::codecierge::DANGER_HEADING.to_string(),
                            text: crate::codecierge::DANGER_TEXT.to_string(),
                            buttons: crate::codecierge::DANGER_BUTTONS.to_vec(),
                            silence: None,
                            then,
                            gate: false,
                            close: Some(crate::codecierge::DANGER_CLOSE),
                        });
                    }
                    self.tools_changed();
                } else if l.frame.contains(x, y) {
                    self.codecierge_click(false, l.text, x, y);
                }
            }
            View::Running { transcript, reply, .. } => {
                let Some(l) = &geo.run else { return };
                if l.end.contains(x, y) {
                    return self.codecierge_end(&guid);
                }
                if l.send.contains(x, y) {
                    if !reply.trim().is_empty() {
                        self.codecierge_press_send(&guid);
                    }
                } else if l.reply.contains(x, y) {
                    self.codecierge_click(true, l.reply_text, x, y);
                } else if l.room.contains(x, y) {
                    self.codecierge_copy_block(transcript, l.room, y);
                }
            }
        }
        self.redraw();
    }

    /// A click on a code block of the transcript copies it and gives the keyboard back to the
    /// session (iTerm2's `didCopy`).
    fn codecierge_copy_block(&mut self, transcript: &str, room: Rect, y: f32) {
        let content = self.codecierge_doc(transcript, room);
        let (_, oy) = self.codecierge_origin(room, content);
        let Some((_, layout)) = &self.belt.cc_doc else { return };
        let Some(at) = layout.lines.iter().position(|l| y >= oy + l.y && y < oy + l.y + l.h) else { return };
        if !layout.lines[at].code_bg {
            return;
        }
        let first = (0..=at).rev().take_while(|i| layout.lines[*i].code_bg).last().unwrap_or(at);
        let last = (at..layout.lines.len()).take_while(|i| layout.lines[*i].code_bg).last().unwrap_or(at);
        let text = (first..=last).map(|i| layout.line_text(i)).collect::<Vec<_>>().join("\n");
        self.set_clipboard_text(text);
        self.belt_yield_keys();
        self.redraw();
    }

    /// The editor takes the keyboard and places the caret (Shift extends, a double-click selects
    /// a word), and a drag selects.
    fn codecierge_click(&mut self, reply: bool, text: Rect, x: f32, y: f32) {
        self.belt_take_focus(ToolId::Codecierge, Focus::Field);
        self.codecierge_sync();
        let (at, end) = self.codecierge_index_at(reply, text, x, y);
        let shift = self.mods.shift;
        let edit = if reply { &mut self.belt.cc_reply } else { &mut self.belt.cc_goal };
        edit.click(at, end, shift, now_ms(), super::clippings_ui::double_click_ms());
        self.drag = Drag::Belt(BeltDrag::Codecierge);
    }

    fn codecierge_index_at(&mut self, reply: bool, text: Rect, x: f32, y: f32) -> (usize, bool) {
        let line_h = self.list_metrics().line.max(1.0);
        self.with_note_measure(|belt, measure, font| {
            let (edit, wrap, scroll) = if reply { (&mut belt.cc_reply, &mut belt.cc_reply_wrap, belt.cc_reply_scroll) } else { (&mut belt.cc_goal, &mut belt.cc_goal_wrap, belt.cc_goal_scroll) };
            let lines = wrap.wrap(&edit.text, text.w, font, measure);
            let line = (((y - text.y + scroll) / line_h).floor().max(0.0) as usize).min(lines.len().saturating_sub(1));
            let at = text_edit::index_at(&edit.text, &lines, line, x - text.x, measure);
            (at, text_edit::ends_line(&lines, line, at))
        })
    }

    /// A selection drag in Codecierge's editor.
    pub(super) fn codecierge_drag(&mut self, x: f32, y: f32) {
        let Some(area) = self.tool_area(ToolId::Codecierge) else { return };
        let view = self.codecierge_view();
        let geo = self.codecierge_geometry(area, &view);
        let (reply, text) = match (&geo.goal, &geo.run) {
            (Some(l), _) => (false, l.text),
            (_, Some(l)) => (true, l.reply_text),
            _ => return,
        };
        let (at, end) = self.codecierge_index_at(reply, text, x, y);
        let edit = if reply { &mut self.belt.cc_reply } else { &mut self.belt.cc_goal };
        edit.drag_to(at, end);
    }

    /// The wheel over Codecierge scrolls the transcript, `dy` lines (up shows earlier text).
    pub(super) fn codecierge_wheel(&mut self, dy: f32) {
        if self.codecierge_running() {
            self.belt.cc_scroll = (self.belt.cc_scroll + dy * self.list_metrics().line).max(0.0);
            self.redraw();
        }
    }

    /// Keys while a Codecierge editor has the keyboard (`text_edit::takes`, as the Notes
    /// editor): Shift+Enter in the goal presses Start, Enter in the reply sends, Esc gives the
    /// keyboard back; other chords pass. True when taken.
    pub(super) fn codecierge_key(&mut self, ev: &KeyEvent) -> bool {
        let mods = self.mods;
        if is_modifier(&ev.logical_key) {
            return true;
        }
        if ev.logical_key == WKey::Named(NamedKey::Escape) && !(mods.ctrl || mods.alt || mods.cmd) {
            self.belt.focus = None;
            self.redraw();
            return true;
        }
        let Some(area) = self.tool_area(ToolId::Codecierge) else { return true };
        let view = self.codecierge_view();
        let running = match &view {
            View::Goal { .. } => false,
            View::Running { .. } => true,
            View::Onboarding(_) => return false,
        };
        let Some(guid) = self.codecierge_session().map(|s| s.guid.clone()) else { return false };
        let enter = ev.logical_key == WKey::Named(NamedKey::Enter) && !(mods.ctrl || mods.alt || mods.cmd);
        if enter && !running && mods.shift {
            if let View::Goal { start_enabled: true, .. } = view {
                self.codecierge_press_start(&guid);
            }
            return true;
        }
        if enter && running && !mods.shift {
            self.codecierge_press_send(&guid);
            return true;
        }
        let Some((key, ctrl, alt)) = edit_key(&ev.logical_key, ev.text.as_deref(), mods) else { return !(mods.ctrl || mods.alt || mods.cmd) };
        if !text_edit::takes(ctrl, alt, mods.cmd, &key) {
            return false;
        }
        let key = self.with_clipboard_text(key);
        self.codecierge_sync();
        let geo = self.codecierge_geometry(area, &view);
        let text = match (&geo.goal, &geo.run) {
            (Some(l), _) => l.text,
            (_, Some(l)) => l.reply_text,
            _ => return true,
        };
        let line_h = self.list_metrics().line.max(1.0);
        let cut = key == EditKey::Cut;
        let (out, caret_line) = self.with_note_measure(|belt, measure, font| {
            let (edit, wrap, _) = if running { (&mut belt.cc_reply, &mut belt.cc_reply_wrap, ()) } else { (&mut belt.cc_goal, &mut belt.cc_goal_wrap, ()) };
            let lines = wrap.wrap(&edit.text, text.w, font, measure);
            edit.page_lines = ((text.h / line_h).floor() as usize).max(1);
            let out = edit.key(key, mods.shift, &lines, measure);
            let lines = wrap.wrap(&edit.text, text.w, font, measure);
            (out, edit.caret_line(&lines, measure).0)
        });
        match out {
            EditOut::Changed => self.codecierge_edited(),
            EditOut::Copied(t) => {
                self.set_clipboard_text(t);
                if cut {
                    self.codecierge_edited();
                }
            }
            EditOut::Moved | EditOut::Kept => {}
        }
        // The caret stays in view.
        let top = caret_line as f32 * line_h;
        let scroll = if running { &mut self.belt.cc_reply_scroll } else { &mut self.belt.cc_goal_scroll };
        if top < *scroll {
            *scroll = top;
        } else if top + line_h > *scroll + text.h {
            *scroll = top + line_h - text.h;
        }
        self.redraw();
        true
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

    /// An OSC 1337 ClearCapturedOutput empties the list only until the next capture, which
    /// brings every entry back (`CO:157-163`); a Clear's cut-off line is kept throughout.
    #[test]
    fn an_osc_clear_empties_the_list_until_the_next_capture() {
        let c = |line: &str, abs_line: i64| Captured { line: line.into(), values: vec![], command: None, state: false, abs_line, mark: 1 };
        let mut list = vec![c("a", 1), c("b", 2)];
        let mut st = ToolState::default();
        view_follows(&mut st, 7, 1, 0, captured_fingerprint(&list), String::new());
        assert!(st.blanked.is_none());
        // The Clear button hid "a".
        st.cleared = captured::clear(&list[..1], 1, 0);
        list.push(c("c", 3));
        view_follows(&mut st, 7, 1, 0, captured_fingerprint(&list), String::new());
        assert_eq!(captured::visible(&list, 1, 0, &st.cleared), vec![1, 2]);
        // The OSC clear: nothing shows, and stays so while nothing changes.
        view_follows(&mut st, 7, 1, 1, captured_fingerprint(&list), String::new());
        assert!(st.blanked.is_some());
        view_follows(&mut st, 7, 1, 1, captured_fingerprint(&list), String::new());
        assert!(st.blanked.is_some(), "no change, still empty");
        // A search change is an update too.
        view_follows(&mut st, 7, 1, 1, captured_fingerprint(&list), "x".into());
        assert!(st.blanked.is_none());
        view_follows(&mut st, 7, 1, 2, captured_fingerprint(&list), String::new());
        assert!(st.blanked.is_some());
        // The next capture: every entry since the Clear's cut shows again, old ones included.
        list.push(c("d", 4));
        view_follows(&mut st, 7, 1, 2, captured_fingerprint(&list), String::new());
        assert!(st.blanked.is_none());
        assert_eq!(st.cleared.abs_line, 1, "the Clear's line is kept");
        assert_eq!(captured::visible(&list, 1, 2, &st.cleared), vec![1, 2, 3]);
        // A line grown in place counts as a change as well.
        view_follows(&mut st, 7, 1, 3, captured_fingerprint(&list), String::new());
        list[3].line.push('!');
        view_follows(&mut st, 7, 1, 3, captured_fingerprint(&list), String::new());
        assert!(st.blanked.is_none());
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

    #[test]
    fn note_file_round_trips() {
        let base = std::path::Path::new(r"C:\Users\me\AppData\Roaming");
        assert_eq!(notes_path(base), base.join("Tandem").join("notes.txt"));
        let text = "first line\nsecond \u{00e9}\ttab\n";
        assert_eq!(read_note(text.as_bytes()), text);
        // A byte order mark (Notepad's UTF-8) is dropped; only a leading one.
        let mut bom = vec![0xEF, 0xBB, 0xBF];
        bom.extend_from_slice(b"a\xEF\xBB\xBFb");
        assert_eq!(read_note(&bom), "a\u{feff}b");
        // Invalid UTF-8 is replaced, never refused (a note always opens).
        assert_eq!(read_note(b"ok\xFFok"), "ok\u{fffd}ok");
        assert_eq!(read_note(b""), "");
    }
    /// Review Focus 6: a focused Notes editor takes typing, Enter, Tab, the edit keys and its
    /// Ctrl chords; other chords go on to the menus and the session.
    #[test]
    fn notes_keys_stay_in_the_editor() {
        let m = |ctrl: bool, alt: bool, shift: bool, cmd: bool| Mods { shift, alt, ctrl, cmd };
        let none = m(false, false, false, false);
        let taken = |k: &WKey, text: Option<&str>, mods: Mods| edit_key(k, text, mods).filter(|(key, c, a)| text_edit::takes(*c, *a, mods.cmd, key)).map(|(key, ..)| key);
        let ch = |c: &str| WKey::Character(c.into());
        let named = |n: NamedKey| WKey::Named(n);
        assert_eq!(taken(&ch("a"), Some("a"), none), Some(EditKey::Text("a".into())));
        assert_eq!(taken(&named(NamedKey::Space), Some(" "), none), Some(EditKey::Text(" ".into())));
        assert_eq!(taken(&named(NamedKey::Enter), Some("\r"), none), Some(EditKey::Enter));
        assert_eq!(taken(&named(NamedKey::Tab), Some("\t"), none), Some(EditKey::Tab));
        assert_eq!(taken(&named(NamedKey::Backspace), None, none), Some(EditKey::Backspace));
        assert_eq!(taken(&named(NamedKey::ArrowLeft), None, m(false, false, true, false)), Some(EditKey::Left));
        assert_eq!(taken(&named(NamedKey::End), None, none), Some(EditKey::End));
        // The Ctrl chords: edit keys, word and document moves.
        assert_eq!(taken(&ch("c"), Some("\u{3}"), m(true, false, false, false)), Some(EditKey::Copy));
        assert_eq!(taken(&ch("v"), Some("\u{16}"), m(true, false, false, false)), Some(EditKey::Paste(String::new())));
        assert_eq!(taken(&ch("Z"), Some("\u{1a}"), m(true, false, true, false)), Some(EditKey::Redo));
        assert_eq!(taken(&ch("y"), Some("\u{19}"), m(true, false, false, false)), Some(EditKey::Redo));
        assert_eq!(taken(&named(NamedKey::ArrowRight), None, m(true, false, false, false)), Some(EditKey::WordRight));
        assert_eq!(taken(&named(NamedKey::Backspace), None, m(true, false, false, false)), Some(EditKey::DeleteWordBack));
        assert_eq!(taken(&named(NamedKey::Home), None, m(true, false, false, false)), Some(EditKey::TextStart));
        // AltGr typing is typing.
        assert_eq!(taken(&ch("@"), Some("@"), m(true, true, false, false)), Some(EditKey::Text("@".into())));
        // Not the editor's: other Ctrl chords, Ctrl+Enter and Ctrl+Tab, Alt and Win chords.
        assert_eq!(taken(&ch("t"), Some("\u{14}"), m(true, false, false, false)), None);
        assert_eq!(taken(&named(NamedKey::Enter), None, m(true, false, false, false)), None);
        assert_eq!(taken(&named(NamedKey::Tab), None, m(true, false, false, false)), None);
        assert_eq!(taken(&ch("x"), Some("x"), m(false, true, false, false)), None);
        assert_eq!(taken(&named(NamedKey::ArrowLeft), None, m(false, true, false, false)), None);
        assert_eq!(taken(&ch("c"), Some("c"), m(false, false, false, true)), None);
        assert_eq!(taken(&named(NamedKey::F5), None, none), None);
        // The Windows clipboard keys.
        assert_eq!(taken(&named(NamedKey::Insert), None, m(false, false, true, false)), Some(EditKey::Paste(String::new())));
        assert_eq!(taken(&named(NamedKey::Insert), None, m(true, false, false, false)), Some(EditKey::Copy));
        assert_eq!(taken(&named(NamedKey::Delete), None, m(false, false, true, false)), Some(EditKey::Cut));
        // A plain Insert is no editor key (the editor swallows it, as any plain key).
        assert_eq!(edit_key(&named(NamedKey::Insert), None, none), None);
    }

    /// Review finding: the selection stays with the process the user picked when the tree
    /// refreshes (`JT:492-569`).
    #[test]
    fn jobs_selection_follows_its_processes() {
        let mut sel = tool_list::Selection::default();
        sel.click(1, false, false, true, 0, 500);
        let before = [(10, 1), (11, 2), (12, 3)];
        // A new process shows above the selected one: the selection moves down with it.
        let after = [(10, 1), (99, 9), (11, 2), (12, 3)];
        follow_jobs(&mut sel, (Some(10), &before), (Some(10), &after));
        assert_eq!(sel.rows, vec![2]);
        // One above it ends: it moves up again.
        follow_jobs(&mut sel, (Some(10), &after), (Some(10), &before));
        assert_eq!(sel.rows, vec![1]);
        // The selected pid comes back as another process (a reused pid): nothing selected.
        let reused = [(10, 1), (11, 7), (12, 3)];
        follow_jobs(&mut sel, (Some(10), &before), (Some(10), &reused));
        assert!(sel.rows.is_empty());
        // Another session's shell: nothing selected.
        sel.click(0, false, false, true, 1000, 500);
        follow_jobs(&mut sel, (Some(10), &before), (Some(20), &before));
        assert!(sel.rows.is_empty());
    }

    #[test]
    fn jobs_and_codecierge_texts_follow_iterm2() {
        assert_eq!(cannot_notify_text("vim", 42), ("Cannot Notify on Termination", "Tandem cannot watch the job vim with process ID 42 because it has already terminated.".to_string()));
        assert_eq!(cannot_notify_text("", 7).1, "Tandem cannot watch the job (unknown) with process ID 7 because it has already terminated.");
        assert_eq!(codecierge_button(CodeciergeState::AdminDisabled), ("Get Started", false));
        assert_eq!(codecierge_button(CodeciergeState::NeedsAi), ("Reveal Setting", true));
        assert_eq!(codecierge_button(CodeciergeState::Ready), ("Get Started", true));
        // The label and button, 8 points apart, centered together.
        let area = Rect { x: 0.0, y: 0.0, w: 200.0, h: 200.0 };
        let (top, b) = codecierge_layout(area, 2, 15.0, 80.0, 1.0);
        assert_eq!((b.w, b.h), (104.0, 22.0));
        assert_eq!(b.x, 48.0);
        assert_eq!(b.y, top + 30.0 + 8.0);
        assert_eq!(top, (200.0 - (30.0 + 8.0 + 22.0)) / 2.0);
        // Notes: the two segments across the top, the editor 4 points under them.
        let l = note_layout(Rect { x: 10.0, y: 20.0, w: 101.0, h: 100.0 }, 1.0);
        assert_eq!((l.segments[0].w, l.segments[1].x, l.segments[1].w), (50.0, 60.0, 51.0));
        assert_eq!((l.frame.y, l.frame.h), (44.0, 76.0));
        assert_eq!((l.text.x, l.text.y), (16.0, 47.0));
    }

    #[test]
    fn codecierge_views_fit_their_room() {
        let area = Rect { x: 10.0, y: 20.0, w: 300.0, h: 260.0 };
        let inside = |r: Rect| r.x >= area.x && r.y >= area.y && r.x + r.w <= area.x + area.w + 0.01 && r.y + r.h <= area.y + area.h + 0.01;
        let g = goal_layout(area, 3, 15.0, 40.0, 1.0);
        for r in [g.frame, g.text, g.check, g.start] {
            assert!(inside(r), "{r:?}");
        }
        // Top to bottom: the prompt, the field, the checkbox, Start.
        assert!(g.frame.y >= g.top + 3.0 * 15.0 && g.frame.y + g.frame.h <= g.check.y && g.check.y + g.check.h <= g.start.y);
        // Start is centered.
        assert!(((g.start.x + g.start.w / 2.0) - (area.x + area.w / 2.0)).abs() < 1.0);
        let r = run_layout(area, 15.0, 50.0, 1.0);
        for rect in [r.spinner, r.room, r.reply, r.reply_text, r.send, r.end] {
            assert!(inside(rect), "{rect:?}");
        }
        // The transcript, then the reply row, then End Task at the bottom right; send sits inside
        // the reply field's right end.
        assert!(r.room.y + r.room.h <= r.reply.y && r.reply.y + r.reply.h <= r.end.y);
        assert!(r.end.x + r.end.w <= area.x + area.w);
        assert!(r.send.x + r.send.w <= r.reply.x + r.reply.w + 0.01 && r.reply_text.x + r.reply_text.w <= r.send.x);
        // A tool at its minimum height (130 points) still lays out without negative sizes.
        let small = Rect { h: crate::codecierge::MIN_HEIGHT, ..area };
        let g = goal_layout(small, 3, 15.0, 40.0, 1.0);
        assert!(g.frame.h >= 0.0 && g.text.h >= 0.0);
        let r = run_layout(small, 15.0, 50.0, 1.0);
        assert!(r.room.h >= 0.0 && r.reply_text.h >= 0.0);
    }

    #[test]
    fn goal_line_is_cut_at_the_tail() {
        let w = |s: &str| s.chars().count() as f32;
        assert_eq!(fit_tail("Goal: deploy", 20.0, &w), "Goal: deploy");
        assert_eq!(fit_tail("Goal: deploy the site", 10.0, &w), "Goal: dep\u{2026}");
        assert_eq!(fit_tail("abc", 0.0, &w), "\u{2026}");
    }
}
