//! The 9k tools in the app: Captured Output (the Capture Output trigger's list, its warnings,
//! Clear, Toggle Checkmark and the coprocess a double-click runs), Session Status (every
//! session's status in this window, sorted by the user's priority patterns, its bell, snooze,
//! help and settings popover), plus the pattern notifications and watches the status changes
//! drive; Jobs (the focused session's process tree, its signals, Inspect and the termination
//! watches), Notes (the global note in `notes.txt` and the session note, in a native editor)
//! and Codecierge (its onboarding view until milestone 12). The pure parts are `captured`,
//! `status_tool`, `toolbelt_tools`, `notes` and `text_edit`; the list widget is 9j's.

use std::cell::RefCell;
use std::collections::{BTreeSet, HashSet};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use super::toolbelt_tools_ui::{Shown, is_modifier, now_ms, unix};
use super::toolbelt_ui::{BeltDrag, ToolState, wrap_words};
use super::*;
use crate::captured::{self, Captured, Cleared};
use crate::mouse::Mods;
use crate::notes::{self, Mode};
use crate::palette::Rgb;
use crate::proctree::Proc;
use crate::status_tool::{self, Pattern, StatusRow};
use crate::text_edit::{self, EditKey, EditOut};
use crate::tool_list::{self, Focus};
use crate::toolbelt::ToolId;
use crate::toolbelt_tools::{self as tt, CodeciergeState, JobRow, SignalWay};
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

/// The global note's text from its file's bytes: UTF-8, invalid bytes replaced, a leading
/// byte order mark dropped.
pub(super) fn read_note(bytes: &[u8]) -> String {
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    String::from_utf8_lossy(bytes).into_owned()
}

/// Jobs' signal popup is as wide as the widest signal name needs (`SignalPicker sizeToFit`).
pub(super) const SIGNAL_POPUP_W: f32 = 70.0;
/// Jobs refreshes once a second while it shows (`JT:379-396`), and the termination watches
/// look as often.
const JOBS_TICK: Duration = Duration::from_secs(1);
/// Jobs' disclosure glyphs (Segoe Fluent Icons ChevronRight and ChevronDown); a row without
/// children keeps the room blank, as NSOutlineView does.
const JOB_CLOSED: char = '\u{E76C}';
const JOB_OPEN: char = '\u{E70D}';
const JOB_LEAF: char = ' ';
/// Inspect's labels (`iTermJobInfoPopoverViewController.swift:75,81`).
const INSPECT_PID: &str = "Process ID";
const INSPECT_COMMAND: &str = "Command";
/// Notes: the mode control (a small segmented control) and the gap under it (`TN` `relayout`,
/// `kMargin`), the text's padding inside the frame (the text container's line fragment
/// padding), in points; a tab is drawn and measured as four spaces.
const SEGMENT_H: f32 = 20.0;
const NOTES_GAP: f32 = 4.0;
const NOTE_PAD_X: f32 = 5.0;
const NOTE_PAD_Y: f32 = 2.0;
const TAB_AS: &str = "    ";
/// The debounced large-note write retries while the file stays unreadable, at most this many
/// times (`TN` `kMaxLargeNoteWriteRetries`).
const NOTE_RETRIES: u32 = 60;
/// Codecierge's button and the gap above it, points (`CC` `layoutSubviews`), its answer bar.
const CC_BUTTON_H: f32 = 22.0;
const CC_BUTTON_PAD: f32 = 24.0;
const CC_GAP: f32 = 8.0;
const CODECIERGE_BAR: &str = "toolbelt-codecierge";
const CODECIERGE_LATER: &str = "AI features come in milestone 12.";
/// The admin switch (`iTermAdvancedSettingsModel.generativeAIAllowed`); `EnableAI` is off until
/// milestone 12.
const AI_ALLOWED_KEY: &str = "GenerativeAIAllowed";

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
/// (Ctrl and Alt together) is plain text. None: not an editor key (another Ctrl chord).
pub(super) fn edit_key(key: &WKey, text: Option<&str>, mods: Mods) -> Option<(EditKey, bool, bool)> {
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

/// What a write of the global note did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Wrote {
    Written,
    /// Nothing new to write, or an empty note with no file (a deleted note stays deleted).
    Nothing,
    /// The file is there but could not be read: its content is unknown, so it is left alone
    /// and the write retried later.
    Deferred,
    /// The file could be read again and holds the note now (the typing over the unread,
    /// emptied view was backed up and dropped).
    Adopted,
}

/// The file's size and modification time: a cheap fingerprint of the notes file, to tell
/// whether someone else wrote it since this note read or wrote it (`notesFileStatToken`).
fn stat_token(path: &Path) -> Option<(u64, Option<SystemTime>)> {
    std::fs::metadata(path).ok().map(|m| (m.len(), m.modified().ok()))
}

/// The global note (one for every window) and its file, kept with iTerm2's rules for never
/// losing a note (`TN:161-280,500-548`): a file that exists but cannot be read is never
/// overwritten by the empty view that failed read left, unless the file is gone by the time
/// the user typed; typing over such a view is dropped (backed up) if the file becomes readable;
/// a file changed by someone else is backed up before it is overwritten; an empty note never
/// recreates a missing file; every write goes through a temporary file and a rename.
#[derive(Default)]
pub(super) struct GlobalNote {
    path: PathBuf,
    loaded: bool,
    pub(super) text: String,
    /// A read failed: the file may hold a note the view does not (`needsReloadBeforeSave_`).
    needs_reload: bool,
    /// The view is empty because that read failed (`viewEmptiedByFailedLoad_`).
    emptied: bool,
    /// The user typed since the failed read (`editedSinceFailedReload_`).
    edited_since_failed: bool,
    /// The note holds a change not yet on disk (`globalNoteDirty_`).
    dirty: bool,
    token: Option<(u64, Option<SystemTime>)>,
    saver: notes::Saver,
    retries: u32,
    retry_at: Option<Instant>,
}

impl GlobalNote {
    pub(super) fn new(path: PathBuf) -> GlobalNote {
        GlobalNote { path, ..GlobalNote::default() }
    }

    /// Reads the file the first time the note is wanted. Some error when it exists but
    /// could not be read (the note then shows empty and the file is protected).
    pub(super) fn ensure_loaded(&mut self) -> Option<String> {
        if self.loaded {
            return None;
        }
        self.loaded = true;
        self.edited_since_failed = false;
        self.dirty = false;
        match std::fs::read(&self.path) {
            Ok(bytes) => {
                self.text = read_note(&bytes);
                self.needs_reload = false;
                self.emptied = false;
                self.token = stat_token(&self.path);
                None
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                self.text.clear();
                self.needs_reload = false;
                self.emptied = false;
                self.token = None;
                None
            }
            Err(e) => {
                self.text.clear();
                self.needs_reload = true;
                self.emptied = true;
                self.token = None;
                Some(format!("Could not read the notes in {}: {e}", self.path.display()))
            }
        }
    }

    /// The file exists (an error other than not found counts as there: never overwrite what
    /// cannot be looked at).
    fn file_exists(&self) -> bool {
        match std::fs::metadata(&self.path) {
            Ok(_) => true,
            Err(e) => e.kind() != std::io::ErrorKind::NotFound,
        }
    }

    fn clear_stale(&mut self) {
        self.needs_reload = false;
        self.emptied = false;
        self.edited_since_failed = false;
    }

    /// The user changed the note to `text` at `now`: written at once while short, else on the
    /// 2 s trailing debounce (`notes::Saver`).
    pub(super) fn edited(&mut self, text: String, now: Instant) -> Result<Wrote, String> {
        self.text = text;
        self.dirty = true;
        if self.needs_reload {
            self.edited_since_failed = true;
        }
        self.retries = 0;
        self.retry_at = None;
        if self.saver.changed(notes::length(&self.text), now) { self.write_retrying(now) } else { Ok(Wrote::Nothing) }
    }

    /// When the debounced write or a retry is due.
    pub(super) fn due(&self) -> Option<Instant> {
        [self.saver.due(), self.retry_at].into_iter().flatten().min()
    }

    /// Makes a write that is due at `now`.
    pub(super) fn tick(&mut self, now: Instant) -> Result<Wrote, String> {
        let retry = self.retry_at.is_some_and(|t| now >= t);
        if !self.saver.take_due(now) && !retry {
            return Ok(Wrote::Nothing);
        }
        self.retry_at = None;
        self.write_retrying(now)
    }

    /// Writes a pending change now (Tandem is quitting).
    pub(super) fn flush(&mut self) -> Result<Wrote, String> {
        self.saver.flush();
        self.retry_at = None;
        if self.dirty { self.write() } else { Ok(Wrote::Nothing) }
    }

    /// `write`, retried after `notes::DEBOUNCE` (at most `NOTE_RETRIES` times) while the file
    /// cannot be read.
    fn write_retrying(&mut self, now: Instant) -> Result<Wrote, String> {
        let r = self.write();
        if r == Ok(Wrote::Deferred) && self.retries < NOTE_RETRIES {
            self.retries += 1;
            self.retry_at = Some(now + notes::DEBOUNCE);
        }
        r
    }

    /// A sibling of the notes file for a recovery copy.
    fn beside(&self, name: &str) -> PathBuf {
        self.path.with_file_name(name)
    }

    fn write(&mut self) -> Result<Wrote, String> {
        let exists = self.file_exists();
        if self.needs_reload {
            if !exists {
                if self.edited_since_failed {
                    // The note went away and the user typed a new one: nothing to clobber.
                    self.clear_stale();
                } else {
                    // Unedited and gone: the stale view must not recreate it.
                    self.text.clear();
                    self.clear_stale();
                }
            } else if self.edited_since_failed && !self.emptied {
                self.clear_stale();
            } else {
                match std::fs::read(&self.path) {
                    Ok(bytes) => {
                        if self.edited_since_failed && !self.text.is_empty() {
                            let _ = std::fs::write(self.beside("notes-discarded.txt"), self.text.as_bytes());
                        }
                        self.text = read_note(&bytes);
                        self.clear_stale();
                        self.dirty = false;
                        self.token = stat_token(&self.path);
                        return Ok(Wrote::Adopted);
                    }
                    Err(_) => return Ok(Wrote::Deferred),
                }
            }
        }
        if self.text.is_empty() && !exists {
            return Ok(Wrote::Nothing);
        }
        if !self.dirty {
            return Ok(Wrote::Nothing);
        }
        if exists && (self.token.is_none() || self.token != stat_token(&self.path)) {
            // Someone else wrote it since this note read it: keep their copy.
            let _ = std::fs::copy(&self.path, self.beside("notes-backup.txt"));
        }
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("Could not save the notes: {e}"))?;
        }
        let tmp = self.beside("notes.txt.tmp");
        if let Err(e) = std::fs::write(&tmp, self.text.as_bytes()).and_then(|()| std::fs::rename(&tmp, &self.path)) {
            let _ = std::fs::remove_file(&tmp);
            return Err(format!("Could not save the notes: {e}"));
        }
        self.token = stat_token(&self.path);
        self.dirty = false;
        Ok(Wrote::Written)
    }
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
        [polling, self.jobs_watch_at].into_iter().flatten().min()
    }

    /// The selected rows' pids, each with its creation time, and whether one has children.
    fn jobs_selected(&mut self) -> (Option<u32>, Vec<(u32, u64)>, bool) {
        let rows = self.belt.tools.get(&ToolId::Jobs).map(|t| t.selection.rows.clone()).unwrap_or_default();
        let shown = self.tool_shown(ToolId::Jobs);
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
        if self.globals.bool(tt::SEND_SIGNAL_KEY) {
            self.jobs_signal(req);
        } else if self.jobs_confirm.is_none() {
            // The question opens on the next turn of the loop (it needs the event loop).
            self.jobs_confirm = Some((None, req));
        } else if let Some((Some(w), _)) = &self.jobs_confirm {
            w.focus();
        }
    }

    /// Opens the pending "Confirmation Needed" question.
    pub(super) fn open_jobs_confirm(&mut self, event_loop: &ActiveEventLoop) {
        let Some((None, req)) = &self.jobs_confirm else { return };
        let (heading, text) = tt::signal_question(req.pids.len(), req.any_children);
        let Some(g) = self.gpu.as_ref() else { return };
        match crate::prompt_ui::ConfirmWindow::open(event_loop, &g.instance, &g.adapter, &g.device, &g.queue, heading, &text) {
            Ok(w) => {
                if let Some((slot, _)) = self.jobs_confirm.as_mut() {
                    *slot = Some(w);
                }
            }
            Err(e) => {
                self.jobs_confirm = None;
                self.banners.set("window", format!("Could not open the window: {e}"));
            }
        }
    }

    /// Sends a request's signal to each of its processes that is still in the shell's tree
    /// as the tree showed it (never a process outside the pane's tree, never a reused pid).
    /// A console event goes once: it reaches the whole console.
    fn jobs_signal(&mut self, req: SignalRequest) {
        let tree = self.jobs.tree(req.shell);
        let mut consoled = false;
        for (pid, created) in req.pids {
            if !tree.iter().any(|p| p.pid == pid && p.created == created) {
                continue;
            }
            let console = matches!(req.way, SignalWay::CtrlC | SignalWay::CtrlBreak);
            if console && consoled {
                continue;
            }
            match crate::jobwatch::signal_process(pid, Some(created), req.way) {
                Ok(()) => consoled |= console,
                Err(e) => self.banners.set("jobs-signal", format!("Could not send the signal: {e}")),
            }
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
        let row = self.belt.tools.get(&ToolId::Jobs).and_then(|t| t.selection.rows.first().copied());
        let shown = self.tool_shown(ToolId::Jobs);
        let Shown::Jobs(j) = &*shown else { return };
        let Some(p) = row.and_then(|r| j.rows.get(r)).and_then(|r| j.proc(r.pid)) else { return };
        let command = p.cmdline.clone().filter(|c| !c.is_empty()).unwrap_or_else(|| p.name.clone());
        let lines = vec![(INSPECT_PID.to_string(), p.pid.to_string()), (INSPECT_COMMAND.to_string(), command)];
        let Some(area) = self.tool_area(ToolId::Jobs) else { return };
        let (l, ..) = self.tool_layout(ToolId::Jobs, area);
        let Some(&(_, r)) = l.buttons.iter().find(|(b, _)| *b == tool_list::ToolButton::Inspect) else { return };
        let dark = self.dark();
        let Some(g) = self.gpu.as_ref() else { return };
        let origin = g.window.inner_position().unwrap_or_default();
        let at = (origin.x as f64 + f64::from(r.x + r.w / 2.0), origin.y as f64 + f64::from(r.y));
        match crate::status_popover_ui::InfoPopover::open(event_loop, &g.instance, &g.adapter, &g.device, &g.queue, lines, at, dark) {
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
        let Some((Some(w), _)) = self.jobs_confirm.as_mut().filter(|(w, _)| w.as_ref().is_some_and(|w| w.id() == window)) else { return false };
        let answer = match event {
            WindowEvent::CloseRequested => ConfirmResult::Cancel,
            WindowEvent::RedrawRequested => w.redraw(),
            other => {
                w.event(other);
                ConfirmResult::Open
            }
        };
        match answer {
            ConfirmResult::Open => {}
            ConfirmResult::Cancel => self.jobs_confirm = None,
            ConfirmResult::Ok { suppress } => {
                // The checkbox is remembered only on OK (`iTermWarning.m:517-520`).
                if suppress {
                    self.globals.set(tt::SEND_SIGNAL_KEY, toml::Value::Boolean(true));
                    self.save_globals();
                }
                if let Some((_, req)) = self.jobs_confirm.take() {
                    self.jobs_signal(req);
                }
            }
        }
        true
    }

    /// A row's disclosure: a parent's subtree hides or shows; the selection follows its rows.
    pub(super) fn toggle_job(&mut self, pid: u32, open: Option<bool>) {
        let keys = |shown: &Shown| match shown {
            Shown::Jobs(j) => j.rows.iter().map(|r| r.pid).collect::<Vec<u32>>(),
            _ => Vec::new(),
        };
        let before = keys(&self.tool_shown(ToolId::Jobs));
        let st = self.tool_state(ToolId::Jobs);
        let closed = st.collapsed.contains(&pid);
        if open.unwrap_or(closed) {
            st.collapsed.remove(&pid);
        } else {
            st.collapsed.insert(pid);
        }
        let after = keys(&self.tool_shown(ToolId::Jobs));
        self.tool_state(ToolId::Jobs).selection.remap(&before, &after);
        self.redraw();
    }

    /// The pid of Jobs' row `row`, and whether it has children.
    pub(super) fn job_at(&mut self, row: usize) -> Option<(u32, bool)> {
        let shown = self.tool_shown(ToolId::Jobs);
        let Shown::Jobs(j) = &*shown else { return None };
        j.rows.get(row).map(|r| (r.pid, r.has_children))
    }

    /// A row's menu (`iTermJobTreeViewController.xib:133-154`): Copy Command, Copy Process ID,
    /// and Notify on Termination, checked while watched.
    pub(super) fn job_row_menu(&mut self, row: usize) {
        let Some((pid, _)) = self.job_at(row) else { return };
        let watched = self.job_watches.iter().any(|(p, ..)| *p == pid);
        let menu = muda::Menu::new();
        let ctx = super::toolbelt_tools_ui::CTX_ID;
        let items: [&dyn muda::IsMenuItem; 3] = [
            &muda::MenuItem::with_id(format!("{ctx}job-copy:{pid}"), "Copy Command", true, None),
            &muda::MenuItem::with_id(format!("{ctx}job-pid:{pid}"), "Copy Process ID", true, None),
            &muda::CheckMenuItem::with_id(format!("{ctx}job-notify:{pid}"), "Notify on Termination", true, watched, None),
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
    pub(super) fn copy_job(&mut self, pid: u32, command: bool) {
        let shown = self.tool_shown(ToolId::Jobs);
        let Shown::Jobs(j) = &*shown else { return };
        let Some(p) = j.proc(pid) else { return };
        let text = if command { p.cmdline.clone().filter(|c| !c.is_empty()).unwrap_or_else(|| p.name.clone()) } else { pid.to_string() };
        if let Some(cb) = self.clipboard.as_mut() {
            if let Err(e) = cb.set_text(text) {
                self.banners.set("clipboard", format!("Could not copy: {e}"));
            }
        }
    }

    /// Notify on Termination: watches the process (or stops); one already gone says so
    /// ("Cannot Notify on Termination").
    pub(super) fn toggle_job_watch(&mut self, pid: u32) {
        if let Some(i) = self.job_watches.iter().position(|(p, ..)| *p == pid) {
            self.job_watches.remove(i);
        } else {
            let shown = self.tool_shown(ToolId::Jobs);
            let Shown::Jobs(j) = &*shown else { return };
            let found = j.proc(pid).map(|p| (p.created, crate::proctree::strip_exe(&p.name).to_string()));
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
    fn with_note_measure<R>(&mut self, f: impl FnOnce(&mut super::toolbelt_ui::Belt, &dyn Fn(&str) -> f32) -> R) -> R {
        let scale = self.scale();
        let renderer = self.gpu.as_mut().map(|g| RefCell::new(&mut g.renderer));
        let measure = |s: &str| {
            let s = s.replace('\t', TAB_AS);
            match &renderer {
                Some(r) => r.borrow_mut().toolbelt_width(&s, scale),
                None => s.chars().count() as f32 * 7.0 * scale,
            }
        };
        f(&mut self.belt, &measure)
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
        let key = match key {
            EditKey::Paste(_) => EditKey::Paste(self.clipboard.as_mut().and_then(|cb| cb.get_text().ok()).unwrap_or_default()),
            k => k,
        };
        let Some(area) = self.tool_area(ToolId::Notes) else { return true };
        self.notes_sync();
        let l = note_layout(area, self.scale());
        let line_h = self.list_metrics().line.max(1.0);
        let cut = key == EditKey::Cut;
        let (out, caret_line) = self.with_note_measure(|belt, measure| {
            let lines = text_edit::wrap(&belt.note_edit.text, l.text.w, measure);
            belt.note_edit.page_lines = ((l.text.h / line_h).floor() as usize).max(1);
            let out = belt.note_edit.key(key, mods.shift, &lines, measure);
            let lines = text_edit::wrap(&belt.note_edit.text, l.text.w, measure);
            (out, belt.note_edit.caret_line(&lines, measure).0)
        });
        match out {
            EditOut::Changed => self.notes_edited(),
            EditOut::Copied(text) => {
                if let Some(cb) = self.clipboard.as_mut() {
                    if let Err(e) = cb.set_text(text) {
                        self.banners.set("clipboard", format!("Could not copy: {e}"));
                    }
                }
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
        self.with_note_measure(|belt, measure| {
            let text = &belt.note_edit.text;
            let lines = text_edit::wrap(text, l.text.w, measure);
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
        self.belt.focus = Some((ToolId::Notes, Focus::Field));
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
        let label_w = |s: &str| renderer.borrow_mut().text_width(s, true, m.text_scale);
        let belt = &mut self.belt;
        let edit = &mut belt.note_edit;
        let lines = text_edit::wrap(&edit.text, l.text.w, &measure);
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
        let mut selection = Vec::new();
        if let Some(r) = edit.selection() {
            let space = measure(" ");
            for (i, v) in lines.iter().enumerate() {
                if r.end < v.start || r.start > v.end || (r.end == v.start && v.start != v.end) {
                    continue;
                }
                let (a, b) = (r.start.max(v.start), r.end.min(v.end));
                let x0 = measure(&text[v.start..a]);
                // A selected line end shows as a space's width past the text.
                let x1 = measure(&text[v.start..b]) + if r.end > v.end && lines.get(i + 1).is_some_and(|n| n.start > v.end) { space } else { 0.0 };
                if x1 > x0 {
                    selection.push(Rect { x: l.text.x + x0, y: y_of(i), w: x1 - x0, h: line_h });
                }
            }
        }
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

    // ---- Codecierge (`CC`) ----

    /// Codecierge's state: the admin switch, and AI never enabled before milestone 12.
    pub(super) fn codecierge_now(&self) -> CodeciergeState {
        tt::codecierge_state(self.globals.bool(AI_ALLOWED_KEY), false)
    }

    /// Codecierge laid out in `area`: its label lines (x, y, text), its button, the button's
    /// label and width, and whether it is enabled.
    fn codecierge_geometry(&mut self, area: Rect) -> (Vec<(f32, f32, String)>, Rect, &'static str, f32, bool) {
        let state = match &*self.tool_shown(ToolId::Codecierge) {
            Shown::Codecierge(s) => *s,
            _ => self.codecierge_now(),
        };
        let scale = self.scale();
        let m = self.list_metrics();
        let (label, enabled) = codecierge_button(state);
        let Some(g) = self.gpu.as_mut() else { return (Vec::new(), Rect { x: 0.0, y: 0.0, w: 0.0, h: 0.0 }, label, 0.0, enabled) };
        let renderer = RefCell::new(&mut g.renderer);
        let width = |s: &str| renderer.borrow_mut().text_width(s, true, m.text_scale);
        let inner = Rect { x: area.x + 4.0 * scale, w: (area.w - 8.0 * scale).max(0.0), ..area };
        let wrapped = wrap_words(tt::codecierge_text(state), inner.w, &width);
        let label_w = width(label);
        let (top, button) = codecierge_layout(area, wrapped.len(), m.line, label_w, scale);
        let lines = wrapped
            .into_iter()
            .enumerate()
            .map(|(i, t)| {
                let w = width(&t);
                (inner.x + ((inner.w - w) / 2.0).max(0.0), top + i as f32 * m.line, t)
            })
            .collect();
        (lines, button, label, label_w, enabled)
    }

    pub(super) fn draw_codecierge(&mut self, frame: &mut Frame, area: Rect, cell: (f32, f32)) {
        let (lines, button, label, label_w, enabled) = self.codecierge_geometry(area);
        let d = scene::CodeciergeDraw { lines: &lines, clip: area, button, label, label_w, enabled, dark: self.dark() };
        scene::codecierge(frame, &d, cell, self.list_metrics().text_scale, self.scale());
    }

    /// A press on Codecierge's button: Reveal Setting answers that AI comes later (the
    /// Settings control "Enable AI" arrives with milestone 12); a disabled button does nothing.
    pub(super) fn codecierge_press(&mut self, area: Rect, button: MouseButton, x: f32, y: f32) {
        if button != MouseButton::Left {
            return;
        }
        let (_, rect, _, _, enabled) = self.codecierge_geometry(area);
        if !enabled || !rect.contains(x, y) {
            return;
        }
        if self.codecierge_now() == CodeciergeState::NeedsAi {
            if let Some(pane) = self.focused_id() {
                self.ask(pane, CODECIERGE_BAR, CODECIERGE_LATER, &["OK"], false);
            }
        }
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
    }

    /// Losing a note is the worst failure: a file that could not be read is never written
    /// over by the empty view that read left, the typing over it is backed up when the file
    /// reads again, an empty note never creates a file, a file changed elsewhere is backed up,
    /// and writes leave no temporary file.
    #[test]
    fn global_note_never_loses_the_file() {
        let dir = std::env::temp_dir().join(format!("tandem-notes-{}-{}", std::process::id(), now_ms()));
        let path = dir.join("Tandem").join(notes::FILE_NAME);
        let now = Instant::now();

        // No file: empty, and an empty note writes nothing.
        let mut g = GlobalNote::new(path.clone());
        assert_eq!(g.ensure_loaded(), None);
        assert_eq!(g.edited(String::new(), now), Ok(Wrote::Nothing));
        assert!(!path.exists());
        // A first note is written, through a temporary file that does not stay.
        assert_eq!(g.edited("hello".into(), now), Ok(Wrote::Written));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello");
        assert!(!path.with_file_name("notes.txt.tmp").exists());

        // Someone else rewrote it: their copy is kept aside before it is overwritten.
        std::thread::sleep(Duration::from_millis(20));
        std::fs::write(&path, "theirs, longer").unwrap();
        assert_eq!(g.edited("mine".into(), now), Ok(Wrote::Written));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "mine");
        assert_eq!(std::fs::read_to_string(path.with_file_name("notes-backup.txt")).unwrap(), "theirs, longer");

        // A file that exists but cannot be read (a directory stands in for a locked file).
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        let mut g = GlobalNote::new(path.clone());
        assert!(g.ensure_loaded().is_some(), "the failed read is reported");
        assert_eq!(g.text, "");
        // Typing over the emptied view never writes over the unread file; it retries.
        assert_eq!(g.edited("typed".into(), now), Ok(Wrote::Deferred));
        assert!(path.is_dir());
        assert!(g.due().is_some(), "a retry is armed");
        // The file reads again: it wins, and the typing is kept aside.
        std::fs::remove_dir(&path).unwrap();
        std::fs::write(&path, "the real note").unwrap();
        assert_eq!(g.tick(now + notes::DEBOUNCE), Ok(Wrote::Adopted));
        assert_eq!(g.text, "the real note");
        assert_eq!(std::fs::read_to_string(path.with_file_name("notes-discarded.txt")).unwrap(), "typed");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "the real note");

        // Unreadable, then gone before the user typed: the typing is a new note.
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        let mut g = GlobalNote::new(path.clone());
        assert!(g.ensure_loaded().is_some());
        std::fs::remove_dir(&path).unwrap();
        assert_eq!(g.edited("new".into(), now), Ok(Wrote::Written));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new");

        // A large note waits for the debounce, and a flush writes it.
        let big = "x".repeat(notes::LARGE);
        assert_eq!(g.edited(big.clone(), now), Ok(Wrote::Nothing));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new");
        assert_eq!(g.tick(now), Ok(Wrote::Nothing));
        assert_eq!(g.flush(), Ok(Wrote::Written));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), big);
        let _ = std::fs::remove_dir_all(&dir);
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
}
