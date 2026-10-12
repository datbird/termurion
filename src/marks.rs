//! Marks and shell integration: prompt marks from OSC 133, manual and named marks, the
//! command text, exit code, and times of each command, and the location and variables
//! the shell reports (OSC 7, OSC 1337). A mark lives on its row (the alacritty patch);
//! the records here hold what iTerm2's VT100ScreenMark holds.

use std::collections::BTreeMap;
use std::time::SystemTime;

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Point};
use alacritty_terminal::term::TermMode;
use alacritty_terminal::vte::ansi::Handler;

use crate::osc::{Osc, PromptKind};
use crate::screen::{Request, Screen};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Made by a shell prompt (OSC 133;A, or OSC 7 from a shell without a machine id).
    Prompt,
    /// Made by Set Mark, Set Named Mark, or OSC 1337;SetMark.
    Manual,
}

/// Most rows a prompt may span for its paths to be looked at (`MS:4786-4789`).
const MAX_PROMPT_ROWS: i64 = 5;
/// How many rows above a hovered or clicked cell the Command Info hit test reads for a prompt
/// mark: a command line spans a few rows, so a cell farther below one is not on a command.
const HIT_WALK_ROWS: i32 = 64;

/// The nearest marked row at or above `line` (down to `top`), reading at most `cap` rows.
fn nearest_mark(line: i32, top: i32, cap: i32, mark_at: impl Fn(i32) -> u32) -> Option<(u32, i32)> {
    let mut at = line;
    for _ in 0..cap {
        let id = mark_at(at);
        if id != 0 {
            return Some((id, at));
        }
        if at <= top {
            return None;
        }
        at -= 1;
    }
    None
}
/// Most path candidates kept per prompt (B4).
const MAX_PATH_CANDIDATES: usize = 4;
/// Most path marks kept per pane.
const MAX_PATH_MARKS: usize = 64;

/// A place in the text, relative to a marked row: whole lines after the mark's line, then
/// cells into that line. Reflow moves rows but keeps lines and cells, so this survives a
/// resize.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Anchor {
    pub mark: u32,
    pub lines: u32,
    pub offset: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Mark {
    pub id: u32,
    pub kind: Kind,
    /// The command's text (empty for a prompt with no command yet, and for manual marks).
    pub command: String,
    pub code: Option<i32>,
    pub start: Option<SystemTime>,
    pub end: Option<SystemTime>,
    pub aid: Option<String>,
    pub name: Option<String>,
    /// Where the command text is: from the end of the prompt to where output began.
    pub command_range: Option<(Anchor, Anchor)>,
    /// Capture Output trigger hits on this command (the Captured Output tool's source).
    pub captured: Vec<crate::captured::Captured>,
    /// Bumped by OSC 1337 ClearCapturedOutput; a Clear against a newer generation than the
    /// tool's own view shows nothing until it catches up (`captured::visible`).
    pub clear_count: u32,
    /// Where the prompt ends (133;B), for the API's prompt range.
    pub prompt_end: Option<Anchor>,
    /// Where the command's output begins, set at 133;C for a command with text (iTerm2's
    /// `outputStart`, `MS:3359-3389`).
    pub output_start: Option<Anchor>,
    /// The directory at the prompt's end.
    pub directory: Option<String>,
    /// The host and user shell integration reported at the prompt's end (Command Info's URL).
    pub host: Option<String>,
    pub user: Option<String>,
    /// Drawn as a line across the pane instead of a bar in the margin; decided when the
    /// prompt mark is made (`MS:3578-3641`) and never changed after.
    pub line_style: bool,
}

impl Mark {
    pub fn new(id: u32, kind: Kind) -> Mark {
        Mark { id, kind, command: String::new(), code: None, start: None, end: None, aid: None, name: None, command_range: None, captured: Vec::new(), clear_count: 0, prompt_end: None, output_start: None, directory: None, host: None, user: None, line_style: false }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarkColor {
    /// Blue: success, running, or a manual mark.
    Success,
    /// Yellow: stopped by a signal (codes 128 to 160).
    Other,
    /// Red: failed.
    Error,
}

/// iTerm2's mark color by exit code (iTermTextDrawingHelper).
pub fn color(m: &Mark, yellow_for_signals: bool) -> MarkColor {
    color_for_code(m.code, yellow_for_signals)
}

/// The same by exit code alone (P5: the gutter bar and the scrollbar tick share it).
pub fn color_for_code(code: Option<i32>, yellow_for_signals: bool) -> MarkColor {
    match code {
        None | Some(0) => MarkColor::Success,
        Some(128..=160) if yellow_for_signals => MarkColor::Other,
        Some(_) => MarkColor::Error,
    }
}

/// One command, for the command history (the Command History tool comes in milestone 9).
#[derive(Clone, Debug, PartialEq)]
pub struct CommandRecord {
    pub command: String,
    pub cwd: Option<String>,
    pub host: Option<String>,
    pub start: SystemTime,
    pub end: Option<SystemTime>,
    pub code: Option<i32>,
}

/// What the shell reports about itself.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Shell {
    pub cwd: Option<String>,
    pub host: Option<String>,
    pub user: Option<String>,
    /// Whether the reported host was this machine, decided when it was reported and kept
    /// (iTerm2's frozen locality, `VT100ScreenMutableState.m:4083-4103`). Only the host stack
    /// reads it; it never feeds an open or run decision (14d-G1).
    pub host_local: Option<bool>,
    /// The OSC 7 `machineID` token of the report that named `host` (bounded, untrusted).
    pub machine_id: Option<String>,
    pub shell: Option<String>,
    pub version: Option<u32>,
    /// SetUserVar values, by name (iTerm2's `user.<name>` variables).
    pub vars: BTreeMap<String, String>,
    /// The foreground job and its ancestors (lowercased, deepest first), from the window.
    pub job: Option<crate::proctree::Job>,
    pub job_ancestors: Vec<String>,
    /// The shell's own name, kept after the shell exits: set once the window has seen the
    /// session's process tree (Job Name shows U+2026 from then on, never nothing).
    pub job_root: Option<String>,
    pub history: Vec<CommandRecord>,
}

#[derive(Default)]
pub struct Marks {
    pub records: BTreeMap<u32, Mark>,
    next_id: u32,
    /// The prompt being typed at, or whose command runs.
    current: Option<u32>,
    /// The last mark with a command.
    pub(crate) last_command: Option<u32>,
    /// Between 133;B and 133;C.
    in_command: bool,
    /// After 133;C until the command ends or the next prompt starts: a command runs and its
    /// output started at `output_start`.
    running: bool,
    command_start: Option<Anchor>,
    output_start: Option<Anchor>,
    last_output: Option<(Anchor, Anchor)>,
    /// A mark was made (a prompt, or SetMark); the app may alert on it.
    made: bool,
    /// Alert on Next Mark is set for this session.
    pub alert_next: bool,
    /// Prompts seen so far (a paste that waits for prompts counts them).
    prompts: usize,
    /// Marks made since the app last looked (`screenDidAddMark`: the command queue sends one
    /// command per mark).
    marks_added: u32,
    /// iTerm2's `currentPromptRange` in absolute lines: set to the prompt start at A, to the
    /// captured range at B, and to zeros at D (`MS:3734,4772,4850`).
    prompt_range: ((i64, usize), (i64, usize)),
    /// The prompt start (set with the range at A) and the command start (at B) as anchors,
    /// which survive reflow: the range is derived from them again when rows move.
    prompt_anchors: (Option<Anchor>, Option<Anchor>),
    /// Path-shaped text in the newest prompt, waiting for the app to ask the disk (B4): text
    /// only, nothing here probes anything.
    path_candidates: Vec<crate::pathnav::PathCandidate>,
    /// The prompt mark whose paths were recorded (one record per mark).
    path_for: Option<u32>,
    /// Folders in prompts a click opens the navigator on, anchored so reflow keeps them.
    pub path_marks: Vec<crate::pathnav::PathMark>,
}

/// The one escape loop: %XX to bytes. Strict stops at a `%` without two hex digits after it;
/// lenient keeps such a `%` as it is.
fn decode_escapes(s: &str, strict: bool) -> Option<Vec<u8>> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = |c: u8| (c as char).to_digit(16);
            if let (Some(h), Some(l)) = (b.get(i + 1).and_then(|c| hex(*c)), b.get(i + 2).and_then(|c| hex(*c))) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
            if strict {
                return None;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    Some(out)
}

/// Decodes %XX escapes (bytes, then UTF-8; Latin-1 when that fails, as iTerm2 does).
pub(crate) fn percent_decode(s: &str) -> String {
    let out = decode_escapes(s, false).unwrap_or_default();
    String::from_utf8(out.clone()).unwrap_or_else(|_| out.iter().map(|&c| c as char).collect())
}

/// Foundation's `removingPercentEncoding`: None when a `%` is not followed by two hex digits or
/// the decoded bytes are not UTF-8.
pub(crate) fn percent_decode_strict(s: &str) -> Option<String> {
    String::from_utf8(decode_escapes(s, true)?).ok()
}

/// A file:// URL as an OSC 7 report.
struct Location {
    user: Option<String>,
    host: Option<String>,
    path: String,
    /// The `machineID` token, kept only when short and plain (B9: else treated as no token).
    machine: Option<String>,
    /// Whether the report carried a `machineID` at all (it is then an iTerm2 script's report).
    has_machine_id: bool,
}

fn parse_location(url: &str) -> Option<Location> {
    let rest = url.strip_prefix("file://")?;
    let (rest, query) = rest.split_once('?').unwrap_or((rest, ""));
    let slash = rest.find('/')?;
    let (authority, path) = rest.split_at(slash);
    let (user, host) = match authority.rsplit_once('@') {
        Some((u, h)) => (Some(crate::inert_files::display_text(&percent_decode(u), crate::host_identity::MAX_HOST, false)), h),
        None => (None, authority),
    };
    // A host from a program: controls, bidi marks and length are cut (it is shown in the bar).
    let host = crate::host_identity::clean_host(host);
    let token = query.split('&').find_map(|q| q.strip_prefix("machineID="));
    let machine = token.and_then(crate::host_identity::clean_token);
    Some(Location { user, host, path: percent_decode(path), machine, has_machine_id: token.is_some() })
}

/// The next mark line after `from` (or before it, going back), wrapping around.
pub fn next_mark(lines: &[i32], from: Option<i32>, forward: bool) -> Option<i32> {
    let first = *lines.first()?;
    let last = *lines.last()?;
    Some(match (from, forward) {
        (None, true) => first,
        (None, false) => last,
        (Some(f), true) => lines.iter().copied().find(|l| *l > f).unwrap_or(first),
        (Some(f), false) => lines.iter().rev().copied().find(|l| *l < f).unwrap_or(last),
    })
}

/// Where Clear to Last Mark starts: after the last mark above the cursor that is not the
/// current prompt's, or at that mark when it is directly above the cursor. `i32::MIN`
/// (everything) when there is none.
pub fn clear_start(marks: &[i32], prompt_line: Option<i32>, cursor: i32) -> i32 {
    match marks.iter().copied().filter(|l| *l < cursor && Some(*l) != prompt_line).max() {
        Some(l) if l == cursor - 1 => l,
        Some(l) => l + 1,
        None => i32::MIN,
    }
}

/// Whether a reported host is a host change that may reset modes: a previous host existed,
/// the name differs (exactly, as iTerm2's `isEqualToString:`), and the two are not both this
/// machine (`local` names, compared ignoring case as Windows names are; P1: the same list the
/// host stack's locality is decided with).
pub fn host_changed(previous: Option<&str>, new: &str, local: &[String]) -> bool {
    previous.is_some_and(|p| p != new && !(is_local_name(p, local) && is_local_name(new, local)))
}

use crate::host_identity::is_local_name;

/// What a host change does with a mode left on, from its saved answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostStep {
    Off,
    Offer,
    Nothing,
}

/// True turns the mode off, unset offers, false leaves it (`PS:17953-17962`).
pub fn host_step(saved: Option<bool>) -> HostStep {
    match saved {
        Some(true) => HostStep::Off,
        None => HostStep::Offer,
        Some(false) => HostStep::Nothing,
    }
}

/// A host change's settings, pushed to each screen before its output is read: the saved
/// answers (`NoSyncTurnOff...OnHostChange`) and whether the pane may report the mouse
/// (iTerm2's "ever allowed").
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HostPolicy {
    pub focus: Option<bool>,
    pub mouse: Option<bool>,
    pub paste: Option<bool>,
    pub mouse_allowed: bool,
    /// The names this computer answers to (P1: the one answer for the offers and the host stack).
    pub local_names: std::sync::Arc<[String]>,
    /// What an OSC 7 machine token from this machine holds (`host_identity::our_value`); None
    /// when the identity cannot be read (R3: the token is then Unknown, the names decide).
    pub ours: Option<std::sync::Arc<str>>,
}

/// The modes a host change offers to turn off.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LeftOn {
    pub mouse: bool,
    pub focus: bool,
    pub paste: bool,
}

/// iTerm2's Session > Terminal State toggles: label, escape to turn on, escape to turn
/// off, and the mode that shows the check mark. A host change turns modes off with the
/// same sequences.
pub const TOGGLES: [(&str, &str, &str, TermMode); 6] = [
    ("Alternate Screen", "\x1b[?1049h", "\x1b[?1049l", TermMode::ALT_SCREEN),
    ("Focus Reporting", "\x1b[?1004h", "\x1b[?1004l", TermMode::FOCUS_IN_OUT),
    ("Mouse Reporting", "\x1b[?1000h", "\x1b[?1000l\x1b[?1002l\x1b[?1003l", TermMode::MOUSE_MODE),
    ("Paste Bracketing", "\x1b[?2004h", "\x1b[?2004l", TermMode::BRACKETED_PASTE),
    ("Application Cursor", "\x1b[?1h", "\x1b[?1l", TermMode::APP_CURSOR),
    ("Application Keypad", "\x1b=", "\x1b>", TermMode::APP_KEYPAD),
];

/// A Terminal State toggle's "off" sequence (empty for an unknown label).
pub fn toggle_off(label: &str) -> &'static [u8] {
    TOGGLES.iter().find(|t| t.0 == label).map_or(&[], |t| t.2.as_bytes())
}

impl Marks {
    /// The program ended: no command is typed or running any more (13q).
    pub(crate) fn program_ended(&mut self) {
        self.in_command = false;
        self.running = false;
        // An ended or restarted pane's old prompt must not offer a cd into the new shell (S5).
        self.path_candidates.clear();
        self.path_marks.clear();
        self.path_for = None;
    }
}

impl Screen {
    /// The candidates recorded at the newest prompt's end, once (B4).
    pub fn take_path_candidates(&mut self) -> Vec<crate::pathnav::PathCandidate> {
        std::mem::take(&mut self.marks.path_candidates)
    }

    /// Whether `take_path_candidates` has anything.
    pub fn has_path_candidates(&self) -> bool {
        !self.marks.path_candidates.is_empty()
    }

    /// Records the path-shaped text of the prompt that just ended (133;B), text only: at most
    /// five rows (`MS:4786-4789`), at most `MAX_PATH_CANDIDATES` paths, one record per prompt
    /// mark. Nothing here asks a disk or a drive (B4); the app does, off the UI thread.
    fn record_path_candidates(&mut self, start: (i64, usize), end: (i64, usize)) {
        let Some(mark) = self.marks.current else { return };
        if self.marks.path_for == Some(mark) {
            return;
        }
        self.marks.path_for = Some(mark);
        self.marks.path_candidates.clear();
        let g = self.term.primary_grid();
        let (off, cols) = (g.scrolled_off() as i64, g.columns());
        let ((sl, sc), (el, ec)) = (start, end);
        if el < sl || el - sl + 1 > MAX_PROMPT_ROWS {
            return;
        }
        let (top, bottom) = (-(g.history_size() as i64), g.screen_lines() as i64 - 1);
        let mut found = Vec::new();
        'rows: for abs in sl..=el {
            let line = abs - off;
            if line < top || line > bottom {
                continue;
            }
            let row = &g[Line(line as i32)];
            let first = if abs == sl { sc } else { 0 };
            let last = if abs == el { ec.min(cols) } else { cols };
            let chars: Vec<char> = (0..cols)
                .map(|c| {
                    let cell = &row[Column(c)];
                    let spacer = cell.flags.intersects(alacritty_terminal::term::cell::Flags::WIDE_CHAR_SPACER | alacritty_terminal::term::cell::Flags::LEADING_WIDE_CHAR_SPACER);
                    if c < first || c >= last || spacer { ' ' } else { cell.c }
                })
                .collect();
            for cand in crate::pathnav::candidates(&chars) {
                if found.len() >= MAX_PATH_CANDIDATES {
                    break 'rows;
                }
                if let Some(anchor) = self.anchor_at(mark, Point::new(Line(line as i32), Column(cand.start))) {
                    found.push(crate::pathnav::PathCandidate { anchor, start: cand.start, end: cand.end, text: cand.text });
                }
            }
        }
        self.marks.path_candidates = found;
    }

    /// The folders the app found among the candidates. Newest last, at most `MAX_PATH_MARKS`.
    pub fn add_path_marks(&mut self, found: Vec<crate::pathnav::PathMark>) {
        let marks = &mut self.marks;
        marks.path_marks.extend(found.into_iter().filter(|m| marks.records.contains_key(&m.row_anchor.mark)));
        let extra = marks.path_marks.len().saturating_sub(MAX_PATH_MARKS);
        marks.path_marks.drain(..extra);
    }

    /// Drops the path marks whose prompt mark left the buffer.
    fn prune_path_marks(&mut self) {
        let records = &self.marks.records;
        self.marks.path_marks.retain(|m| records.contains_key(&m.row_anchor.mark));
    }

    /// The path marks, newest last.
    pub fn path_marks(&self) -> &[crate::pathnav::PathMark] {
        &self.marks.path_marks
    }

    /// The path mark under a cell (its index and a copy).
    pub fn path_mark_at(&self, p: Point) -> Option<(usize, crate::pathnav::PathMark)> {
        self.marks.path_marks.iter().enumerate().find_map(|(i, m)| {
            let at = self.resolve(m.row_anchor)?;
            (at.line == p.line && p.column.0 >= at.column.0 && p.column.0 < at.column.0 + (m.end - m.start)).then(|| (i, m.clone()))
        })
    }
}

impl Screen {
    /// The probe rules the prompt path sniff reads (`App::path_probe`).
    pub fn set_path_probe(&mut self, p: crate::netpath::Probe) {
        self.path_probe = p;
    }

    /// Clears from grid line `line` to the end (iTerm2 clearFromAbsoluteLineToEnd). The
    /// cursor's row (the prompt) goes to the top of the screen, and the rows kept above move
    /// into history, so the screen matches ConPTY after its own clear (which keeps only the
    /// cursor row). The current prompt's mark moves onto that row when its own row went.
    pub fn clear_from(&mut self, line: i32) {
        self.bump_ticks();
        let grid = self.term.grid();
        let top = -(grid.history_size() as i32);
        let line = line.max(top);
        let cursor = grid.cursor.point;
        let mut saved = grid[cursor.line].clone();
        // Rows go and move: no selection maps onto them.
        self.clear_selection();
        let cur = self.marks.current;
        // A prompt mark above the cursor row (a two-line prompt) moves down to it.
        if let Some(cur) = cur.filter(|_| saved.mark() == 0) {
            if let Some(ml) = self.mark_line(cur).filter(|ml| *ml >= line && *ml < cursor.line.0) {
                let skipped = self.anchor_at(cur, Point::new(cursor.line, Column(0))).map_or(0, |a| a.lines);
                let m = &mut self.marks;
                for a in [&mut m.command_start, &mut m.output_start, &mut m.prompt_anchors.0, &mut m.prompt_anchors.1].into_iter().flatten() {
                    a.lines = a.lines.saturating_sub(skipped);
                }
                if let Some(r) = m.records.get_mut(&cur) {
                    for a in [&mut r.prompt_end, &mut r.output_start].into_iter().flatten() {
                        a.lines = a.lines.saturating_sub(skipped);
                    }
                }
                let _ = ml;
                saved.set_mark(cur);
            }
        }
        self.term.grid_mut().cut_from(Line(line));
        if line > 0 {
            self.term.scroll_up(line as usize);
        }
        let grid = self.term.grid_mut();
        grid[Line(0)] = saved;
        grid.cursor.point = Point::new(Line(0), cursor.column);
        self.prune_marks();
        self.composer_rows_moved();
    }

    /// At the shell prompt, typing a command (iTerm2's currentCommand); None without shell
    /// integration.
    pub fn at_prompt(&self) -> Option<bool> {
        (self.marks.prompts > 0).then_some(self.marks.in_command)
    }

    /// The command being typed at the prompt (shell integration), if any.
    pub fn typed_command(&self) -> Option<String> {
        if !self.marks.in_command {
            return None;
        }
        let (a, b) = self.last_command_range()?;
        Some(self.term.bounds_to_string(a, b))
    }

    /// The last `max` rows (scrollback and screen) as characters, and the cursor's row in
    /// them and column, for Autocomplete.
    pub fn char_rows(&self, max: usize) -> (Vec<Vec<char>>, usize, usize) {
        use alacritty_terminal::grid::Dimensions;
        let grid = self.term.grid();
        let top = -(grid.history_size() as i32);
        let bottom = grid.screen_lines() as i32 - 1;
        let first = (bottom - max as i32 + 1).max(top);
        let cols = grid.columns();
        let rows: Vec<Vec<char>> = (first..=bottom)
            .map(|l| {
                let row = &grid[Line(l)];
                let mut v: Vec<char> = (0..cols).map(|c| row[Column(c)].c).collect();
                while v.last() == Some(&' ') {
                    v.pop();
                }
                v
            })
            .collect();
        let cursor = grid.cursor.point;
        ((rows), (cursor.line.0 - first) as usize, cursor.column.0)
    }

    /// Prompts seen so far.
    pub fn prompts_seen(&self) -> usize {
        self.marks.prompts
    }

    /// The mark line of the prompt being typed at, if any.
    pub fn current_prompt_line(&self) -> Option<i32> {
        self.mark_line(self.marks.current?)
    }

    /// The mark of the last command, for the Capture Output trigger.
    pub fn last_command_mark(&self) -> Option<&Mark> {
        self.marks.records.get(&self.marks.last_command?)
    }

    fn cursor_point(&self) -> Point {
        self.term.grid().cursor.point
    }

    pub(crate) fn mark_line(&self, id: u32) -> Option<i32> {
        self.row_marks().into_iter().find(|(_, m)| *m == id).map(|(l, _)| l)
    }

    /// The row continues on the next one (a soft wrap).
    fn wrapped(&self, line: i32) -> bool {
        let grid = self.term.primary_grid();
        let cols = grid.columns();
        line < grid.screen_lines() as i32 && grid[Line(line)][Column(cols - 1)].flags.contains(alacritty_terminal::term::cell::Flags::WRAPLINE)
    }

    fn anchor_at(&self, mark: u32, p: Point) -> Option<Anchor> {
        let m = self.mark_line(mark)?;
        if p.line.0 < m {
            return None;
        }
        let cols = self.term.primary_grid().columns();
        let (mut lines, mut start) = (0, m);
        for r in m..p.line.0 {
            if !self.wrapped(r) {
                lines += 1;
                start = r + 1;
            }
        }
        Some(Anchor { mark, lines, offset: (p.line.0 - start) as usize * cols + p.column.0 })
    }

    fn anchor_here(&self, mark: u32) -> Option<Anchor> {
        self.anchor_at(mark, self.cursor_point())
    }

    pub fn resolve(&self, a: Anchor) -> Option<Point> {
        self.resolve_from(self.mark_line(a.mark)?, a)
    }

    /// `resolve` when the anchor's mark is known to be on grid line `base`.
    fn resolve_from(&self, base: i32, a: Anchor) -> Option<Point> {
        let grid = self.term.primary_grid();
        let (cols, bottom) = (grid.columns(), grid.screen_lines() as i32);
        let mut r = base;
        for _ in 0..a.lines {
            while self.wrapped(r) {
                r += 1;
            }
            r += 1;
            if r >= bottom + 1 {
                return None;
            }
        }
        Some(Point::new(Line(r + (a.offset / cols) as i32), Column(a.offset % cols)))
    }

    /// The cell before `p` (range ends are inclusive).
    fn before(&self, p: Point) -> Point {
        if p.column.0 > 0 { Point::new(p.line, Column(p.column.0 - 1)) } else { Point::new(p.line - 1, Column(self.term.columns() - 1)) }
    }

    pub(crate) fn new_mark(&mut self, line: i32, kind: Kind) -> u32 {
        self.bump_ticks();
        self.marks.next_id = self.marks.next_id.wrapping_add(1).max(1);
        let id = self.marks.next_id;
        match kind {
            Kind::Prompt => self.set_row_mark(line, id),
            Kind::Manual => self.set_row_manual(line, id),
        }
        // iTerm2 decides the style when the prompt mark is made (`MS:3578-3641`).
        let line_style = kind == Kind::Prompt && self.line_style_marks;
        self.marks.records.insert(id, Mark { line_style, ..Mark::new(id, kind) });
        self.marks.made = true;
        self.marks.marks_added = self.marks.marks_added.saturating_add(1);
        id
    }

    /// A manual mark (Set Mark, Set Named Mark) on a grid line.
    pub fn set_manual_mark(&mut self, line: i32, name: Option<String>) -> u32 {
        let id = self.new_mark(line, Kind::Manual);
        self.marks.records.get_mut(&id).unwrap().name = name;
        id
    }

    /// Renames mark `id` (`PTYSession.m:16793-16821`, `renameMark:to:`): None on a mark that is
    /// not a prompt's removes it, as a named mark without its name has no reason to stay; a
    /// prompt mark only loses its name. False for a mark that is gone.
    pub fn rename_mark(&mut self, id: u32, name: Option<String>) -> bool {
        // A mark whose row is gone has already been removed.
        let Some(kind) = self.marks.records.get(&id).map(|m| m.kind).filter(|_| self.mark_line(id).is_some()) else { return false };
        if name.is_none() && kind != Kind::Prompt {
            return self.remove_mark(id);
        }
        if let Some(m) = self.marks.records.get_mut(&id) {
            m.name = name;
        }
        true
    }

    /// Removes mark `id`, its record and its row's slot (`VT100ScreenMutableState.m:3517-3529`,
    /// `removeNamedMark:`, whatever the mark's kind). False for an unknown id.
    pub fn remove_mark(&mut self, id: u32) -> bool {
        self.bump_ticks();
        if self.marks.records.remove(&id).is_none() {
            return false;
        }
        if let Some(line) = self.mark_line(id) {
            let row = &mut self.term.primary_grid_mut()[Line(line)];
            if row.mark() == id {
                row.set_mark(0);
            }
            if row.manual() == id {
                row.set_manual(0);
            }
        }
        true
    }

    /// True once after a prompt mark or a SetMark was made (for mark alerts).
    pub fn take_new_mark(&mut self) -> bool {
        std::mem::take(&mut self.marks.made)
    }

    /// The marks made since the last call (prompts and SetMark), for the command queue.
    pub fn take_marks_added(&mut self) -> u32 {
        std::mem::take(&mut self.marks.marks_added)
    }

    /// The cursor as `(absolute line, column)`: absolute lines count every row that
    /// scrolled off the top (`promptStateMachineCursorAbsCoord`, `MS:8404-8407`).
    fn abs_cursor(&self) -> (i64, usize) {
        // The main screen's, also while a full-screen app has the alternate one.
        let g = self.term.primary_grid();
        (g.scrolled_off() as i64 + g.cursor.point.line.0 as i64, g.cursor.point.column.0)
    }

    /// The captured prompt's first row on the main screen (`lastPromptMark.promptRange.start.y`
    /// less the scrollback, `PS:24212-24217`), 0 once it scrolled into history; None with no
    /// prompt range (none yet, or zeroed at D).
    pub fn prompt_start_row(&self) -> Option<usize> {
        if self.marks.prompt_range == Default::default() {
            return None;
        }
        let g = self.term.primary_grid();
        let row = self.marks.prompt_range.0 .0 - g.scrolled_off() as i64;
        Some(row.clamp(0, (g.screen_lines() as i64 - 1).max(0)) as usize)
    }

    /// The main screen's cursor row.
    pub fn primary_cursor_row(&self) -> usize {
        self.term.primary_grid().cursor.point.line.0.max(0) as usize
    }

    /// The captured prompt (`MS:4768-4770`): styled rows from the prompt start to the command
    /// start, the rows blank after trimming dropped, and their text. Rows no longer in the
    /// buffer are left out.
    pub fn prompt_rows(&self) -> (Vec<crate::styled_copy::Row>, Vec<String>) {
        // The rows are the main screen's; the styled copy reads the screen shown.
        if self.alt_screen() {
            return (Vec::new(), Vec::new());
        }
        let g = self.term.primary_grid();
        let off = g.scrolled_off() as i64;
        let (top, bottom) = (-(g.history_size() as i64), g.screen_lines() as i64 - 1);
        let ((sl, sc), (el, ec)) = self.marks.prompt_range;
        let (sl, el) = (sl - off, el - off);
        if el < top || sl > bottom || sl > el {
            return (Vec::new(), Vec::new());
        }
        // A start above the buffer begins at the first line kept.
        let start = if sl < top { (top as i32, 0) } else { (sl as i32, sc) };
        let end = if el > bottom { (bottom as i32, g.columns()) } else { (el as i32, ec) };
        let rows = crate::styled_copy::runs_of_range(self, start, end);
        rows.into_iter()
            .map(|r| {
                let text: String = r.runs.iter().map(|run| run.text.as_str()).collect();
                (r, text)
            })
            .filter(|(_, t)| !t.trim_end().is_empty())
            .unzip()
    }

    /// What the shell echoed between two absolute cursors: the cells' characters, wide
    /// spacers skipped, a row that ends in a line break trimmed of its trailing blanks. None
    /// once it passes `MAX_ACCRUED` characters: such a part is over the cap and dropped
    /// anyway. Also returns the cells read.
    fn echo_between(&self, from: (i64, usize), to: (i64, usize)) -> (Option<String>, u64) {
        use crate::prompt_state::MAX_ACCRUED;
        let g = self.term.primary_grid();
        let (off, cols) = (g.scrolled_off() as i64, g.columns());
        let top = -(g.history_size() as i64);
        // A start that left the buffer begins at its first line.
        let (fl, fc) = if from.0 - off < top { (top, 0) } else { (from.0 - off, from.1) };
        let tl = to.0 - off;
        let (mut out, mut chars, mut cells) = (String::new(), 0usize, 0u64);
        for line in fl..=tl {
            let line = line as i32;
            let first = if line as i64 == fl { fc } else { 0 };
            let last = if line as i64 == tl { to.1 } else { cols };
            let row = &g[Line(line)];
            let mut text = String::new();
            for c in first..last.min(cols) {
                cells += 1;
                let cell = &row[Column(c)];
                if cell.flags.intersects(alacritty_terminal::term::cell::Flags::WIDE_CHAR_SPACER | alacritty_terminal::term::cell::Flags::LEADING_WIDE_CHAR_SPACER) {
                    continue;
                }
                text.push(cell.c);
                if let Some(extra) = cell.zerowidth() {
                    text.extend(extra);
                }
            }
            if line as i64 != tl && !self.wrapped(line) {
                text.truncate(text.trim_end_matches(' ').len());
            }
            chars += text.chars().count();
            out.push_str(&text);
            if chars > MAX_ACCRUED {
                return (None, cells);
            }
        }
        (Some(out), cells)
    }

    /// Rows moved without scrolling (a resize's reflow, a splice, a clear): absolute lines
    /// kept from before no longer name the same rows. The accrue checkpoint goes to the cursor
    /// (as the triggers do) and the prompt range is derived again from its anchors (zeros,
    /// as after D, when they no longer resolve).
    pub(crate) fn composer_rows_moved(&mut self) {
        if self.composer_from.is_some() {
            self.composer_from = Some(self.abs_cursor());
        }
        let off = self.term.primary_grid().scrolled_off() as i64;
        let abs = |p: Point| (off + p.line.0 as i64, p.column.0);
        let (a, b) = self.marks.prompt_anchors;
        let start = a.and_then(|a| self.resolve(a)).map(abs);
        let end = b.and_then(|b| self.resolve(b)).map(abs);
        self.marks.prompt_range = match (start, b, end) {
            (Some(s), None, _) => (s, s),
            (Some(s), Some(_), Some(e)) => crate::prompt_state::prompt_range(s, e),
            _ => Default::default(),
        };
    }

    /// The accrue check (`PSM:386-433` applied to the grid, milestone 9l Decision): while a
    /// command is entered, the echo from the last checked cursor to the cursor now is the
    /// part. A cursor that moved back (a backspace) only moves the checkpoint. Each read
    /// scans only what is new, never past `MAX_ACCRUED` characters, and nothing once the text is at
    /// `MAX_ACCRUED` (no part can fit). Runs before each shell-integration event and after
    /// each read.
    pub(crate) fn composer_accrue(&mut self) {
        // iTerm2 disallows the composer on the alternate screen: its drawing is never echo.
        if !self.prompt_state.enabled || !self.prompt_state.entering() || !self.marks.in_command || self.alt_screen() {
            return;
        }
        if let crate::prompt_state::State::Accruing { so_far, .. } = &self.prompt_state.state {
            if so_far.encode_utf16().count() >= crate::prompt_state::MAX_ACCRUED {
                return;
            }
        }
        let Some(from) = self.composer_from else { return };
        let cursor = self.abs_cursor();
        self.composer_from = Some(cursor);
        if cursor <= from {
            return;
        }
        let (part, cells) = self.echo_between(from, cursor);
        self.composer_scanned += cells;
        if let Some(part) = part.filter(|p| !p.is_empty()) {
            let events = self.prompt_state.output(&part, cursor);
            self.composer_events.extend(events);
        }
    }

    /// The code of the command before the newest one, for the alert sound.
    pub fn previous_code(&self) -> Option<i32> {
        self.marks.records.values().rev().filter(|m| m.kind == Kind::Prompt).nth(1).and_then(|m| m.code)
    }

    /// A mark's exit code: None when there is no such mark, Some(None) when it has no code
    /// (iTerm2's `hasCode` false: a manual mark or a command still running).
    pub fn mark_code(&self, id: u32) -> Option<Option<i32>> {
        self.marks.records.get(&id).map(|m| m.code)
    }

    /// The marks on rows in view, for the margin indicators: (row on screen, mark). None
    /// while a full-screen app has the alternate screen. Reads only the rows in view.
    pub fn visible_marks(&self) -> Vec<(usize, Mark)> {
        if self.term.mode().contains(alacritty_terminal::term::TermMode::ALT_SCREEN) {
            return Vec::new();
        }
        let grid = self.term.grid();
        let offset = grid.display_offset() as i32;
        (0..grid.screen_lines())
            .flat_map(|row| {
                let r = &grid[Line(row as i32 - offset)];
                [(row, r.mark()), (row, r.manual())]
            })
            .filter_map(|(row, id)| Some((row, self.marks.records.get(&id)?.clone())).filter(|_| id != 0))
            .collect()
    }

    /// The prompt and command ranges of the prompt marks in view, as (kind, start, end) in cells
    /// from the top of the view: kind 0 the prompt (the mark's row start to `prompt_end`), 1 the
    /// command (`command_range`). Tandem records no excluded subranges. Debug overlay only.
    pub fn prompt_range_spans(&self) -> Vec<(u8, (i32, usize), (i32, usize))> {
        let offset = self.term.grid().display_offset() as i32;
        let view = |p: Point| (p.line.0 + offset, p.column.0);
        let mut out = Vec::new();
        for (row, m) in self.visible_marks().into_iter().filter(|(_, m)| m.kind == Kind::Prompt) {
            let base = row as i32 - offset;
            let at = |a: Anchor| self.resolve_from(base, a).map(view);
            if let Some(end) = m.prompt_end.and_then(at) {
                out.push((0, (row as i32, 0), end));
            }
            if let Some((a, b)) = m.command_range
                && let (Some(a), Some(b)) = (at(a), at(b))
            {
                out.push((1, a, b));
            }
        }
        out
    }

    /// The named marks still in the buffer, top first: id and name. Reads the rows, not the
    /// records, so a mark whose row was cleared or trimmed from the scrollback is gone at once
    /// (the Named Marks tool keys its list on this).
    pub fn live_named_marks(&self) -> Vec<(u32, Option<String>)> {
        self.row_marks().into_iter().filter_map(|(_, id)| self.marks.records.get(&id).filter(|m| m.name.is_some()).map(|m| (id, m.name.clone()))).collect()
    }

    /// Drops the records whose row is gone (after a clear).
    pub fn prune_marks(&mut self) {
        let live: std::collections::HashSet<u32> = self.row_marks().into_iter().map(|(_, id)| id).collect();
        self.marks.records.retain(|id, _| live.contains(id));
        self.prune_path_marks();
    }

    /// The prompt marks in the buffer as the offscreen command line's rules read them, oldest
    /// first. One pass over the rows, kept until the grid, a mark or the screen changes
    /// (`tick_digest`), so a frame never scans the buffer or prunes records (P11).
    pub fn offscreen_facts(&mut self) -> Vec<crate::offscreen::PromptFacts> {
        let digest = self.tick_digest();
        if let Some((d, facts)) = &self.offscreen_cache {
            if *d == digest {
                return facts.clone();
            }
        }
        let facts = self.compute_offscreen_facts();
        self.offscreen_cache = Some((digest, facts.clone()));
        facts
    }

    fn compute_offscreen_facts(&self) -> Vec<crate::offscreen::PromptFacts> {
        let lines: std::collections::HashMap<u32, i32> = self.row_marks().into_iter().map(|(l, id)| (id, l)).collect();
        let mut prompts: Vec<(i32, &Mark)> = lines.iter().filter_map(|(id, l)| Some((*l, self.marks.records.get(id)?))).filter(|(_, m)| m.kind == Kind::Prompt).collect();
        prompts.sort_by_key(|(l, m)| (*l, m.id));
        let grid = self.term.primary_grid();
        let cursor_line = grid.cursor.point.line.0;
        let blank = |line: i32| {
            let row = &grid[Line(line)];
            (0..grid.columns()).all(|c| row[Column(c)].c.is_whitespace() || row[Column(c)].c == '\0')
        };
        let at = |m: &Mark, a: Anchor| self.resolve_from(*lines.get(&m.id)?, a);
        let mut out = Vec::new();
        for (i, (line, m)) in prompts.iter().enumerate() {
            let start = m.command_range.and_then(|(a, _)| at(m, a)).map(|p| p.line.0);
            let end = m.command_range.and_then(|(_, b)| at(m, b)).map(|p| {
                // The range ends where the output begins, at the start of the next line.
                if p.column.0 == 0 && Some(p.line.0) > start { p.line.0 - 1 } else { p.line.0 }
            });
            // `rangeOfOutputForCommandMark:` (`VSS:1045-1080`): to the next prompt's line (one
            // less for a line-style mark), or for the newest to the cursor's.
            let first = m.output_start.and_then(|a| at(m, a)).map_or(end.unwrap_or(*line) + 1, |p| p.line.0);
            let last = match prompts.get(i + 1) {
                Some((next, n)) => (*next - i32::from(n.line_style)).max(first),
                None => (cursor_line + 1).max(first),
            };
            out.push(crate::offscreen::PromptFacts {
                prompt_line: *line,
                command_line: start,
                command_end_line: end,
                has_code: m.code.is_some(),
                running: m.output_start.is_some() && m.code.is_none(),
                ended: m.end.is_some(),
                output_rows: last - first + 1,
                command_empty: start.is_none_or(|l| l >= grid.screen_lines() as i32 || l < -(grid.history_size() as i32) || blank(l)),
            });
        }
        out
    }

    /// Every mark still in the buffer, top first. Records whose row is gone are dropped.
    pub fn marks_in_order(&mut self) -> Vec<(i32, Mark)> {
        let rows = self.row_marks();
        let live: std::collections::HashSet<u32> = rows.iter().map(|(_, id)| *id).collect();
        self.marks.records.retain(|id, _| live.contains(id));
        self.prune_path_marks();
        rows.into_iter().filter_map(|(l, id)| Some((l, self.marks.records.get(&id)?.clone()))).collect()
    }

    /// Whether a CUP could be the first draw of a running command: between its prompt's end and
    /// its output, or after the output started (the scanner holds CUPs only then).
    pub(crate) fn command_pending(&self) -> bool {
        self.marks.in_command || self.marks.running
    }

    /// Whether a command's output has started and the command has not ended.
    pub(crate) fn command_running(&self) -> bool {
        self.marks.running
    }

    /// The row (negative in the scrollback) where the running command's output starts.
    fn running_output_row(&self) -> Option<i32> {
        if !self.marks.running {
            return None;
        }
        Some(self.resolve(self.marks.output_start?)?.line.0)
    }

    /// iTerm2's `saveScreenToScrollbackIfCursorMovedAboveCommandOutput` (`MS+TD:480-521`), run
    /// after a CUP: a program that repaints from the top without the alternate screen would
    /// overwrite what the person still sees, so while the running command has printed nothing
    /// and the cursor went above where its output starts, the screen scrolls into history
    /// first, as CSI 2 J does.
    pub(crate) fn cursor_moved_above_output(&mut self) {
        if !self.rules.save_above_prompt
            || self.term.mode().contains(alacritty_terminal::term::TermMode::ALT_SCREEN)
            || self.term.printed != self.printed_at_output
        {
            return;
        }
        let Some(start) = self.running_output_row() else { return };
        if self.term.grid().cursor.point.line.0 >= start {
            return;
        }
        self.bump_ticks();
        self.term.scroll_screen_into_history();
    }

    /// The output of the last finished command (Select Output of Last Command), or of the
    /// running one.
    pub fn last_output_range(&self) -> Option<(Point, Point)> {
        if let Some((a, b)) = self.marks.last_output {
            return Some((self.resolve(a)?, self.before(self.resolve(b)?)));
        }
        let start = self.resolve(self.marks.output_start?)?;
        Some((start, self.before(self.cursor_point())))
    }

    /// The prompt mark whose command line holds `point`, and that line's range (from the end of
    /// the prompt to where output began, `UAF:275-286`). A mark with no command text is no hit,
    /// nor is anything while a full-screen program shows the alternate screen (P7).
    pub fn command_hit(&self, point: Point) -> Option<(u32, (Point, Point))> {
        if self.alt_screen() {
            return None;
        }
        // The command a point can belong to is the nearest prompt mark at or above its row, so a
        // hover never reads the whole buffer for marks.
        let grid = self.term.primary_grid();
        let top = -(grid.history_size() as i32);
        let line = point.line.0;
        if line < top || line >= grid.screen_lines() as i32 {
            return None;
        }
        let (id, line) = nearest_mark(line, top, HIT_WALK_ROWS, |l| grid[Line(l)].mark())?;
        let m = self.marks.records.get(&id).filter(|m| m.kind == Kind::Prompt && !m.command.trim().is_empty())?;
        let (a, b) = m.command_range?;
        let (start, end) = (self.resolve_from(line, a)?, self.resolve_from(line, b)?);
        let end = self.before(end);
        (point >= start && point <= end).then_some((id, (start, end)))
    }

    /// Prompt command `id`'s record, while its row is in the buffer.
    pub fn command_mark(&self, id: u32) -> Option<&Mark> {
        self.row_marks().into_iter().any(|(_, m)| m == id).then(|| self.marks.records.get(&id)).flatten()
    }

    /// Prompt command `id`'s record without looking for its row (the open window's tick: no scan of
    /// the buffer per frame).
    pub fn command_record(&self, id: u32) -> Option<&Mark> {
        self.marks.records.get(&id).filter(|m| m.kind == Kind::Prompt)
    }

    /// Whether `id` is the command whose output is being printed now.
    pub fn command_is_running(&self, id: u32) -> bool {
        self.marks.running && self.marks.last_command == Some(id)
    }

    /// The rows of command `id`'s output: from where it began to the row before the next
    /// prompt mark, or for the newest command to the cursor (`rangeOfOutputForCommandMark:`,
    /// `VSS:1045-1080`). None for a command with no output start, or under the alternate screen
    /// (the marks are the main screen's).
    pub fn output_range_of(&self, id: u32) -> Option<(Point, Point)> {
        if self.alt_screen() {
            return None;
        }
        let rows = self.row_marks();
        let mine = rows.iter().find(|(_, m)| *m == id)?.0;
        let m = self.marks.records.get(&id).filter(|m| m.kind == Kind::Prompt)?;
        let first = self.resolve_from(mine, m.output_start?)?;
        let next = rows.iter().filter(|(l, other)| *l > mine && self.marks.records.get(other).is_some_and(|r| r.kind == Kind::Prompt)).map(|(l, _)| *l).min();
        let last = match next {
            Some(l) => Point::new(Line(l - 1), Column(self.term.columns() - 1)),
            None => self.before(self.cursor_point()),
        };
        (last >= first).then_some((first, last))
    }

    /// Command `id`'s output as text: each row without its trailing blanks, no trailing blank
    /// rows (what Command Info counts and copies).
    pub fn output_text(&self, id: u32) -> Option<String> {
        let (a, b) = self.output_range_of(id)?;
        let text = self.term.bounds_to_string(a, b);
        let lines: Vec<&str> = text.split('\n').map(|l| l.trim_end_matches([' ', '\0', '\r'])).collect();
        let end = lines.iter().rposition(|l| !l.is_empty()).map_or(0, |i| i + 1);
        Some(lines[..end].join("\n"))
    }

    /// The fold sitting on the row where command `id`'s output begins: the row `fold` puts its
    /// summary on and the only one the Unfold button looks at (iTerm2 reads the button's line).
    pub fn command_fold(&self, id: u32) -> Option<u32> {
        let (first, _) = self.output_range_of(id)?;
        self.fold_at(first.line.0)
    }

    /// The grid lines of command `id`'s region: its prompt's line through the line before the next
    /// prompt mark, else the last screen line (`PTYSession.m` command region). None when the mark
    /// is gone or is not a prompt.
    pub fn command_grid_lines(&self, id: u32) -> Option<(i32, i32)> {
        let rows = self.row_marks();
        let first = rows.iter().find(|(_, m)| *m == id)?.0;
        self.marks.records.get(&id).filter(|m| m.kind == Kind::Prompt)?;
        let next = rows.iter().filter(|(l, other)| *l > first && self.marks.records.get(other).is_some_and(|r| r.kind == Kind::Prompt)).map(|(l, _)| *l).min();
        let last = next.map_or(self.term.primary_grid().screen_lines() as i32 - 1, |l| l - 1);
        Some((first, last.max(first)))
    }

    /// Command `id`'s region in buffer rows (row 0 the oldest scrollback row), inclusive: its
    /// prompt's row through the row before the next prompt mark, else the last row (13s Task 9,
    /// P15). One rule with `command_grid_lines`, which 13r's buttons read in grid lines. `&mut`
    /// is the frozen contract (S6, S9: the region may be cached off `tick_digest`).
    pub fn command_region(&mut self, id: u32) -> Option<(usize, usize)> {
        let digest = self.tick_digest();
        if let Some((d, m, r)) = self.region_cache {
            if d == digest && m == id {
                return r;
            }
        }
        let region = self.command_grid_lines(id).map(|(first, last)| {
            let history = self.term.primary_grid().history_size() as i32;
            ((first + history).max(0) as usize, (last + history).max(0) as usize)
        });
        self.region_cache = Some((digest, id, region));
        region
    }

    /// The prompt mark of the command a click on grid `line` is in (`commandMarkAtOrBeforeLine:`):
    /// the nearest prompt mark on that line or above it. A manual mark is no command.
    pub fn prompt_at_or_before(&self, line: i32) -> Option<u32> {
        self.row_marks().into_iter().filter(|(l, id)| *l <= line && self.marks.records.get(id).is_some_and(|r| r.kind == Kind::Prompt)).max_by_key(|(l, _)| *l).map(|(_, id)| id)
    }

    /// The command being typed, else the last command's text (Select Current Command).
    pub fn last_command_range(&self) -> Option<(Point, Point)> {
        if self.marks.in_command {
            let start = self.resolve(self.marks.command_start?)?;
            return Some((start, self.before(self.cursor_point())));
        }
        let m = self.marks.records.get(&self.marks.last_command?)?;
        let (a, b) = m.command_range?;
        Some((self.resolve(a)?, self.before(self.resolve(b)?)))
    }

    /// Prompt mark `id` (the newest one when None) as the API reads it
    /// (`getPromptResponseForMark:`, `PS:22532-22574`), in (x, absolute line) points: the
    /// prompt from its row's start to its end (empty before 133;B), the command (empty at the
    /// prompt's end from 133;B, `MS:4827-4845`, to the cursor once text is typed, then the
    /// command's own range), where output began, the cursor of the screen shown, the command's
    /// text (else the one being typed, as `currentCommand`; None with neither), the directory
    /// at the prompt's end (else the shell's) and the code.
    /// None when no such prompt mark is in the buffer.
    pub fn prompt_facts(&self, id: Option<u32>) -> Option<crate::api_buffer::PromptFacts> {
        let mut prompts = self.row_marks().into_iter().filter(|(_, m)| self.marks.records.get(m).is_some_and(|r| r.kind == Kind::Prompt));
        let (line, id) = match id {
            Some(id) => prompts.find(|(_, m)| *m == id)?,
            None => prompts.last()?,
        };
        let m = self.marks.records.get(&id)?;
        // The API's numbers: lost lines plus history, not the running count triggers use.
        let off = crate::api_buffer::above(self.term.primary_grid());
        let abs = |p: Point| (p.column.0 as i32, off + p.line.0 as i64);
        let at = |a: Anchor| self.resolve(a).map(abs);
        let start = (0, off + line as i64);
        let end = m.prompt_end.and_then(at);
        let text = if m.command.is_empty() { self.typed_command() } else { Some(m.command.clone()) };
        let cursor = abs(self.cursor_point());
        // While this prompt's command is typed, its range runs to the cursor once it holds
        // text (`commandRangeDidChange`, `MS:3765-3790`; `VT100ScreenState.m:994-1004`).
        let typed = self.marks.current == Some(id) && self.marks.in_command && text.as_deref().is_some_and(|t| !t.trim().is_empty());
        let command = match m.command_range.filter(|_| !m.command.is_empty()) {
            Some((a, b)) => at(a).zip(at(b)),
            None if typed => self.marks.command_start.and_then(at).or(end).map(|s| (s, cursor)),
            None => end.map(|e| (e, e)),
        };
        Some(crate::api_buffer::PromptFacts {
            id,
            prompt: Some((start, end.unwrap_or(start))),
            command,
            output_start: m.output_start.and_then(at),
            cursor,
            text,
            directory: m.directory.clone().or_else(|| self.shell.cwd.clone()),
            code: m.code,
        })
    }

    /// The prompt marks still in the buffer, oldest first.
    pub fn prompt_ids(&mut self) -> Vec<u32> {
        self.marks_in_order().into_iter().filter(|(_, m)| m.kind == Kind::Prompt).map(|(_, m)| m.id).collect()
    }

    pub(crate) fn prompt_start(&mut self, kind: PromptKind, aid: Option<String>, fresh_line: bool) {
        if !matches!(kind, PromptKind::Initial | PromptKind::Unknown) {
            return;
        }
        self.marks.running = false;
        // A new prompt ends whatever the last program said it was doing (OSC 21337).
        self.clear_tab_status();
        if fresh_line && self.place_prompt_first_column && !self.pty_backed && self.cursor_point().column.0 > 0 {
            self.feed_raw(b"\r\n");
        }
        let line = self.cursor_point().line.0;
        // A prompt redrawn in place (PSReadLine, zsh) keeps its mark.
        if let Some(cur) = self.marks.current {
            let row = self.term.grid()[Line(line)].mark();
            if row == cur && self.marks.records.get(&cur).is_some_and(|m| m.command.is_empty()) {
                return;
            }
        }
        let here = self.marks.current.and_then(|c| self.anchor_here(c));
        if let (Some(start), Some(end)) = (self.marks.output_start.take(), here) {
            self.marks.last_output = Some((start, end));
        }
        if let Some(prev) = self.marks.last_command.and_then(|id| self.marks.records.get_mut(&id)) {
            prev.end.get_or_insert_with(SystemTime::now);
        }
        let id = self.new_mark(line, Kind::Prompt);
        self.marks.records.get_mut(&id).unwrap().aid = aid;
        self.marks.current = Some(id);
        self.marks.in_command = false;
        self.marks.prompts += 1;
        self.fire_event(crate::trigger_events::EventKind::PromptDetected, &Default::default(), &[]);
    }

    /// Sets the progress bar's state; the request and the event follow a change of whether it
    /// shows (`PTYSession.m:24278-24279`).
    pub(crate) fn set_progress(&mut self, next: crate::progress::Progress) {
        let (was, shown) = (self.progress.visible(), next.visible());
        self.progress = next;
        if shown != was {
            self.requests.push(crate::screen::Request::Progress(shown));
            let word = if shown { "appeared" } else { "disappeared" };
            self.fire_event(crate::trigger_events::EventKind::ProgressBarChanged, &crate::trigger_events::EventFacts { appeared: shown, ..Default::default() }, &[word.to_string()]);
        }
    }

    /// Acts on one shell-integration OSC at the current cursor position. A full-screen app
    /// on the alternate screen makes no marks; its cursor is not on the main screen.
    pub(crate) fn apply(&mut self, osc: Osc) {
        // A print job drops sequences; a prompt mark or a command end ends it unprinted (B8).
        if self.print_capture.is_some() {
            if !matches!(osc, Osc::Prompt { .. } | Osc::CommandEnd { .. }) {
                return;
            }
            self.print_capture = None;
        }
        use crate::trigger_events::{EventFacts, EventKind};
        let before = (self.shell.cwd.clone(), self.shell.host.clone(), self.shell.user.clone());
        let was_local = self.shell.host_local;
        self.apply_osc(osc);
        // Directory, host, and user changes are events for triggers.
        let after = (self.shell.cwd.clone(), self.shell.host.clone(), self.shell.user.clone());
        // Another account (the first one too) reaches the app (`VT100ScreenMutableState.m:4123-4137`),
        // with the modes a program on the previous host left on and which may be turned off
        // (`PS:17929-17940`).
        if after.1.is_some() && (before.1 != after.1 || before.2 != after.2) {
            let new = after.1.as_deref().unwrap_or_default();
            // Locality is decided when a host is reported and kept: a report of the same host
            // (a user-only change) copies the previous answer (`VT100ScreenMutableState.m:4083-4103`).
            let local = if before.1 == after.1 {
                was_local.unwrap_or(false)
            } else {
                crate::host_identity::is_local_session(Some(new), self.shell.machine_id.as_deref(), &self.host_policy.local_names, self.host_policy.ours.as_deref())
            };
            self.shell.host_local = Some(local);
            let moved = before.1 != after.1 && host_changed(before.1.as_deref(), new, &self.host_policy.local_names);
            let left_on = if moved { self.reset_modes_on_host_change() } else { LeftOn::default() };
            self.requests.push(Request::HostChanged(left_on));
            if let (true, Some(prev)) = (before.1 != after.1, before.1.clone()) {
                use crate::host_state::Remote;
                let previous = Remote { user: before.2.clone(), host: prev, local: was_local.unwrap_or(false) };
                // The mode and title as they are at the report (the bytes before it are fed, the
                // rest of the chunk not yet), so a remote's same-chunk writes cannot be saved.
                let (mode, title) = (self.keyboard().mode, self.title().map(crate::host_state::capped_title));
                self.requests.push(Request::HostMoved { previous, current: Remote { user: after.2.clone(), host: new.to_string(), local }, mode, title });
            }
        }
        for (kind, old, new) in [(EventKind::DirectoryChanged, before.0, after.0), (EventKind::HostChanged, before.1, after.1), (EventKind::UserChanged, before.2, after.2)] {
            if let Some(v) = new.filter(|n| Some(n) != old.as_ref()) {
                self.fire_event(kind, &EventFacts { value: v.clone(), ..Default::default() }, &[v]);
            }
        }
    }

    /// `maybeResetTerminalStateOnHostChange` (`PS:17987-18008`), here in the output where the
    /// host was reported, as iTerm2 stops reading until it is done: the modes as the previous
    /// host left them are turned off, offered, or left alone, so the new host's own modes
    /// that follow in the same read are never touched. Gives the offers, for the app.
    fn reset_modes_on_host_change(&mut self) -> LeftOn {
        let policy = self.host_policy.clone();
        let mode = *self.term().mode();
        let fish = self.shell.shell.as_deref() == Some("fish");
        // Mouse reporting counts only where the pane may report the mouse; fish handles
        // paste bracketing itself. iTerm2's order: mouse, focus, paste.
        let modes = [
            (policy.mouse_allowed && self.mouse_modes().tracking != crate::mouse::Tracking::Off, policy.mouse, toggle_off("Mouse Reporting")),
            (mode.contains(TermMode::FOCUS_IN_OUT), policy.focus, toggle_off("Focus Reporting")),
            (!fish && mode.contains(TermMode::BRACKETED_PASTE), policy.paste, toggle_off("Paste Bracketing")),
        ];
        let mut offers = [false; 3];
        for (i, (on, saved, off)) in modes.into_iter().enumerate() {
            if !on {
                continue;
            }
            match host_step(saved) {
                HostStep::Off => {
                    let reply = self.feed_raw(off);
                    self.replies.extend(reply);
                }
                HostStep::Offer => offers[i] = true,
                HostStep::Nothing => {}
            }
        }
        // iTerm2 also turns DEC 2048 (resize reports) off outright here, without a question.
        if self.resize_reports() {
            self.set_resize_reports(false);
        }
        let [mouse, focus, paste] = offers;
        LeftOn { mouse, focus, paste }
    }

    fn apply_osc(&mut self, osc: Osc) {
        // An initial prompt mark clears alternate scroll even from the alternate screen: "avoid
        // leaving it on when ssh dies" (`VT100Terminal.m:4944`).
        if matches!(osc, Osc::Prompt { kind: PromptKind::Initial, .. }) {
            self.clear_alternate_scroll();
        }
        let alt = self.term.mode().contains(alacritty_terminal::term::TermMode::ALT_SCREEN);
        if alt && matches!(osc, Osc::Prompt { .. } | Osc::CommandStart | Osc::OutputStart | Osc::CommandEnd { .. })
            || alt && matches!(&osc, Osc::Kv(k, _) if k == "SetMark")
        {
            return;
        }
        match osc {
            Osc::Query(crate::reports::Query::ColorSchemeReports(on)) => self.set_color_scheme_reports(on),
            Osc::Query(q) => {
                let answer = crate::reports::answer(self, &q);
                self.replies.extend(answer);
            }
            Osc::Sync(_) | Osc::Held(_) => {}
            Osc::TabStatus(payload) => {
                let pairs = crate::tabstatus::parse(&payload);
                self.apply_tab_status(&pairs);
            }
            // iTerm2 sets the shape when the name is known and puts the default back otherwise.
            Osc::Pointer(name) => self.pointer = crate::pointer_shape::from_x11(&name),
            Osc::Clipboard(payload) => match crate::clipboard_osc::osc52(&payload) {
                Some(crate::clipboard_osc::Osc52::Write(t)) => self.requests.push(crate::screen::Request::ClipboardWrite(t)),
                Some(crate::clipboard_osc::Osc52::Read(pc)) => self.requests.push(crate::screen::Request::ClipboardRead(pc)),
                None => {}
            },
            Osc::Prompt { kind, aid, fresh_line } => {
                // Any A reaches the machine (`PSM:291-305`); only the kinds that make a mark
                // move the prompt range, which stays with the marks.
                let counted = matches!(kind, PromptKind::Initial | PromptKind::Unknown);
                let events = self.prompt_state.a();
                self.composer_events.extend(events);
                let redraw = self.marks.in_command;
                self.prompt_start(kind, aid, fresh_line);
                // A prompt redrawn at its own start while a command is typed keeps the range
                // (`MS:3719-3727`).
                let at = self.abs_cursor();
                let fresh = counted && !(redraw && self.marks.prompt_range.0 == at);
                // iTerm2 resets the graphic rendition when a prompt starts, only for the kinds
                // that reach `promptDidStartAt` (Initial, Unknown) and not on that redraw
                // (`MS+TD:2555-2563`, `MS:3719-3749`).
                if fresh && self.rules.reset_sgr_on_prompt {
                    use alacritty_terminal::vte::ansi::{Attr, Handler};
                    self.term.terminal_attribute(Attr::Reset);
                }
                if fresh {
                    self.marks.prompt_range = (at, at);
                    self.marks.prompt_anchors = (self.marks.current.and_then(|c| self.anchor_here(c)), None);
                }
            }
            Osc::CommandStart => {
                let events = self.prompt_state.b();
                self.composer_events.extend(events);
                self.marks.in_command = true;
                self.marks.command_start = self.marks.current.and_then(|c| self.anchor_here(c));
                let (end, cwd) = (self.marks.command_start, self.shell.cwd.clone());
                let (host, user) = (self.shell.host.clone(), self.shell.user.clone());
                if let Some(m) = self.marks.current.and_then(|c| self.marks.records.get_mut(&c)) {
                    m.prompt_end = end;
                    m.directory = cwd;
                    m.host = host;
                    m.user = user;
                }
                self.composer_from = Some(self.abs_cursor());
                // The prompt is captured unless its range did not change (`MS:4749-4777`).
                // The prompt's real extent, before the range is cut for a long prompt.
                let raw = (self.marks.prompt_range.0, self.abs_cursor());
                let range = crate::prompt_state::prompt_range(raw.0, raw.1);
                if range != self.marks.prompt_range {
                    self.marks.prompt_range = range;
                    self.marks.prompt_anchors.1 = self.marks.command_start;
                    let (rows, text) = self.prompt_rows();
                    let events = self.prompt_state.captured(rows, text);
                    self.composer_events.extend(events);
                }
                self.record_path_candidates(raw.0, raw.1);
                // The API's PROMPT, at the prompt's end (`PS:16709-16726`).
                if self.api_watch {
                    self.api_events.push(crate::api_notify::ApiEvent::Prompt(self.marks.current));
                }
            }
            Osc::OutputStart => {
                let events = self.prompt_state.c();
                self.composer_events.extend(events);
                if !std::mem::take(&mut self.marks.in_command) {
                    return;
                }
                let Some(cur) = self.marks.current else { return };
                let end = self.anchor_here(cur);
                let start = self.marks.command_start.take();
                let text = match (start.and_then(|a| self.resolve(a)), end) {
                    (Some(a), Some(_)) => self.term.bounds_to_string(a, self.before(self.cursor_point())),
                    _ => String::new(),
                };
                let text = text.trim().to_string();
                // Output starts at the next line when the cursor is past column 0.
                let p = self.cursor_point();
                let start_at = if p.column.0 > 0 { Point::new(p.line + 1, Column(0)) } else { Point::new(p.line, Column(0)) };
                self.marks.output_start = self.anchor_at(cur, start_at);
                self.marks.running = true;
                // Nothing this command prints has been seen yet (`appendedTextSinceCommandExecuted`).
                self.printed_at_output = self.term.printed;
                let now = SystemTime::now();
                let output_start = self.marks.output_start;
                if let Some(m) = self.marks.records.get_mut(&cur) {
                    m.command = text.clone();
                    m.start = Some(now);
                    m.command_range = start.zip(end);
                    // iTerm2 marks output only for a command with text (`MS:3359-3389`).
                    if !text.is_empty() {
                        m.output_start = output_start;
                    }
                }
                self.marks.last_command = Some(cur);
                // The API's COMMAND_START, only for a command that started (`PS:18645-18656`).
                if self.api_watch {
                    let command = (!text.is_empty()).then(|| text.clone());
                    self.api_events.push(crate::api_notify::ApiEvent::CommandStart { command, mark: Some(cur) });
                }
                let (cwd, host) = (self.shell.cwd.clone(), self.shell.host.clone());
                self.shell.history.push(CommandRecord { command: text, cwd, host, start: now, end: None, code: None });
            }
            Osc::CommandEnd { code, aid } => {
                let events = self.prompt_state.d();
                self.composer_events.extend(events);
                self.marks.running = false;
                self.marks.prompt_range = Default::default();
                self.marks.prompt_anchors = (None, None);
                if std::mem::take(&mut self.marks.in_command) {
                    // Ended without starting (Ctrl+C at the prompt): the mark goes away.
                    if let Some(cur) = self.marks.current.take() {
                        if let Some(line) = self.mark_line(cur) {
                            self.set_row_mark(line, 0);
                        }
                        self.marks.records.remove(&cur);
                    }
                    return;
                }
                let target = aid
                    .and_then(|a| self.marks.records.values().find(|m| m.aid.as_deref() == Some(a.as_str())).map(|m| m.id))
                    .or(self.marks.last_command);
                let now = SystemTime::now();
                if let Some(m) = target.and_then(|id| self.marks.records.get_mut(&id)) {
                    m.code = code;
                    m.end = Some(now);
                }
                if let Some(r) = self.shell.history.last_mut().filter(|r| r.end.is_none()) {
                    r.code = code;
                    r.end = Some(now);
                }
                let c = code.unwrap_or(0);
                // The API's COMMAND_END, 0 when no code came (`PS:18696-18722`).
                if self.api_watch {
                    self.api_events.push(crate::api_notify::ApiEvent::CommandEnd { code: c, mark: target });
                }
                if self.chat_watch || self.codecierge_watch {
                    self.requests.push(Request::CommandEnded { code: c, mark: target });
                }
                self.fire_event(crate::trigger_events::EventKind::CommandFinished, &crate::trigger_events::EventFacts { exit_code: Some(c), ..Default::default() }, &[c.to_string()]);
            }
            Osc::Notify(text) => {
                self.requests.push(crate::screen::Request::Notify { title: None, message: text.clone() });
                self.fire_event(crate::trigger_events::EventKind::NotificationPosted, &crate::trigger_events::EventFacts { value: text.clone(), ..Default::default() }, &[text]);
            }
            Osc::Progress(body) => {
                // 9;4 reads as iTerm2 does (`TDEL:1607-1660`): a bare 9;4 stops, a bad or
                // out-of-range field is ignored. The event fires when the bar appears or goes.
                if let Some(next) = crate::progress::parse(body.as_deref(), self.progress) {
                    self.set_progress(next);
                }
            }
            Osc::Sixel(data) => self.sixel(&data),
            Osc::Kitty(body) => self.kitty(&body),
            // Only a trusted screen answers or registers (B7); a batch answers 64 messages at
            // most. Replies are counted, not chunks: a message of many `m=1` chunks must reach
            // its last one, or the first and last parts would join with the middle skipped.
            Osc::KittyDnd(body) => {
                if self.trusted && self.dnd_answered < crate::screen::DND_ANSWERS_PER_BATCH {
                    let replies = self.dnd.inbound(&String::from_utf8_lossy(&body));
                    if !replies.is_empty() {
                        self.dnd_answered += 1;
                    }
                    for r in replies {
                        self.replies.extend(r.into_bytes());
                    }
                }
            }
            Osc::Location(url) => {
                let Some(Location { user, host, path, machine: token, has_machine_id: machine }) = parse_location(&url) else { return };
                // Accept OSC 7 off: the sequence is ignored, after a warning when it carries a
                // machine id and no proprietary location code was seen (`MS:4555-4565`,
                // `MS:4636-4655`). One pending request at most, so a flood queues nothing more.
                if !self.rules.accept_osc7 {
                    let pending = self.requests.iter().any(|r| matches!(r, crate::screen::Request::Osc7Disabled));
                    if machine && !self.saw_location_code && !pending {
                        self.requests.push(crate::screen::Request::Osc7Disabled);
                    }
                    return;
                }
                if host.is_some() && (user.is_some() || host != self.shell.host) {
                    self.shell.user = user;
                }
                // The token belongs to the report that carried it: a report with a host replaces it.
                if host.is_some() {
                    self.shell.machine_id = token;
                }
                self.shell.host = host.or(self.shell.host.take());
                if !path.is_empty() {
                    self.shell.cwd = Some(path);
                }
                // A shell without iTerm2's scripts may send only OSC 7; treat it as a prompt too.
                if !machine {
                    self.prompt_start(PromptKind::Initial, None, false);
                }
            }
            Osc::Kv(key, value) => match key.as_str() {
                "CurrentDir" if !value.is_empty() => {
                    self.saw_location_code = true;
                    self.shell.cwd = Some(value)
                }
                "RemoteHost" => {
                    self.saw_location_code = true;
                    let clean: String = value.chars().filter(|c| !c.is_control()).collect();
                    // No token comes with this report: an earlier host's must not decide for it.
                    self.shell.machine_id = None;
                    match clean.rsplit_once('@') {
                        Some((u, h)) => {
                            self.shell.user = Some(crate::inert_files::display_text(u, crate::host_identity::MAX_HOST, false));
                            self.shell.host = crate::host_identity::clean_host(h);
                        }
                        None => self.shell.host = crate::host_identity::clean_host(&clean),
                    }
                }
                "SetUserVar" => {
                    use base64::Engine;
                    let Some((name, b64)) = value.split_once('=') else {
                        self.shell.vars.remove(&value);
                        return;
                    };
                    if name.contains('.') {
                        return;
                    }
                    if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(b64) {
                        self.set_user_var(name, String::from_utf8_lossy(&bytes).into_owned());
                    }
                }
                "ShellIntegrationVersion" => {
                    let mut parts = value.split(';');
                    self.shell.version = parts.next().and_then(crate::shell_version::version_value);
                    let shell = crate::shell_version::reported_shell(parts).map(str::to_string);
                    if let Some(sh) = &shell {
                        self.shell.shell = Some(sh.clone());
                    }
                    // The app decides whether the scripts are out of date, with this report's
                    // shell (`VT100ScreenMutableState+TerminalDelegate.m:2814-2852`).
                    self.requests.push(Request::ShellIntegrationReported { version: self.shell.version, shell });
                }
                "Notification" => {
                    use base64::Engine;
                    let args = crate::blocks::parse(&value);
                    let d = |k: &str| crate::blocks::get(&args, k).and_then(|v| base64::engine::general_purpose::STANDARD.decode(v.trim()).ok()).map(|b| String::from_utf8_lossy(&b).into_owned()).filter(|v| !v.is_empty());
                    let message = match (d("subtitle"), d("message")) {
                        (Some(sub), Some(m)) => format!("{sub}\n{m}"),
                        (sub, m) => sub.or(m).unwrap_or_default(),
                    };
                    if !message.is_empty() || d("title").is_some() {
                        self.requests.push(crate::screen::Request::Notify { title: d("title"), message: message.clone() });
                        self.fire_event(crate::trigger_events::EventKind::NotificationPosted, &crate::trigger_events::EventFacts { value: message.clone(), ..Default::default() }, &[message]);
                    }
                }
                "SetBadgeFormat" => {
                    use base64::Engine;
                    if let Ok(b) = base64::engine::general_purpose::STANDARD.decode(value.trim()) {
                        let t = String::from_utf8_lossy(&b).into_owned();
                        let r = if crate::interp::has_calls(&t) { crate::screen::Request::BadgeRefused } else { crate::screen::Request::SetBadge(t) };
                        self.requests.push(r);
                    }
                }
                // Custom=id=<sender>:<payload> (trusted, and only with its colon,
                // `VT:4492-4508`): the Custom Escape Sequence event and the API's notification.
                "Custom" if self.trusted => {
                    if let Some((head, payload)) = value.split_once(':') {
                        let args = crate::blocks::parse(head);
                        let sender = crate::blocks::get(&args, "id").map(str::to_string);
                        let id = sender.clone().unwrap_or_default();
                        let facts = crate::trigger_events::EventFacts { name: id.clone(), ..Default::default() };
                        self.fire_event(crate::trigger_events::EventKind::CustomEscapeSequence, &facts, &[id, payload.to_string()]);
                        if self.api_watch {
                            self.api_events.push(crate::api_notify::ApiEvent::Custom { id: sender, payload: payload.to_string() });
                        }
                    }
                }
                "SetMark" => {
                    let line = self.cursor_point().line.0;
                    self.new_mark(line, Kind::Manual);
                }
                // Only a command with captures counts a clear (`terminalClearCapturedOutput`);
                // the tool refreshes either way, with no warning (`screenClearCapturedOutput`).
                "ClearCapturedOutput" => {
                    if let Some(id) = self.marks.last_command {
                        if let Some(m) = self.marks.records.get_mut(&id).filter(|m| !m.captured.is_empty()) {
                            m.clear_count += 1;
                        }
                    }
                    self.requests.push(crate::screen::Request::Trigger(crate::trigger_eval::TriggerRequest::CapturedCleared));
                }
                "File" | "MultipartFile" | "FilePart" | "FileEnd" => self.file_osc(&key, &value),
                // The clipboard keys need trust (Disable Potentially Insecure Escape Sequences off).
                "Copy" if self.trusted => {
                    if let Some(t) = self.copy.take(&value, self.limits.transfer) {
                        self.requests.push(crate::screen::Request::ClipboardWrite(t));
                    }
                }
                // Without clipboard access the text would be thrown away: keep nothing.
                "CopyToClipboard" if self.trusted && self.clipboard_allowed => self.capture = Some(Vec::new()),
                "CursorShape" => {
                    if let Some(shape) = crate::osc1337::cursor_shape(&value) {
                        self.term.set_cursor_shape(shape);
                    }
                }
                "ClearScrollback" => self.clear(false),
                "HighlightCursorLine" => {
                    let on = value.is_empty() || crate::osc1337::parse_bool(&value);
                    self.requests.push(Request::CursorGuide(on));
                }
                "RequestAttention" => self.requests.push(Request::Attention(crate::osc1337::attention(&value))),
                "StealFocus" if self.trusted => self.requests.push(Request::StealFocus),
                "Disinter" => self.requests.push(Request::Disinter),
                "ReportCellSize" => self.requests.push(Request::ReportCellSize),
                "ReportWindowFrame" => self.requests.push(Request::ReportWindowFrame),
                "ReportScreenFrames" if self.trusted => self.requests.push(Request::ReportScreenFrames),
                "SetWindowFrame" => {
                    let n: Vec<f64> = value.split(';').filter_map(|v| v.trim().parse().ok()).collect();
                    if let [x, y, w, h] = n[..] {
                        if value.split(';').count() == 4 {
                            self.requests.push(Request::SetWindowFrame(x, y, w, h));
                        }
                    }
                }
                "UnicodeVersion" => {
                    self.unicode.apply(&value);
                    self.push_width_rule();
                }
                "SetProfile" if self.trusted => self.requests.push(Request::SetProfile(value)),
                "SetProfileProperty" if self.trusted => {
                    let pairs: Vec<(String, String)> =
                        value.split(';').filter_map(|p| p.split_once('=')).map(|(k, v)| (k.to_string(), v.to_string())).collect();
                    if !pairs.is_empty() {
                        self.requests.push(Request::SetProfileProperty(pairs));
                    }
                }
                "SetBackgroundImageFile" if self.trusted => self.requests.push(Request::SetBackgroundImageFile(value)),
                "OpenURL" if self.trusted => self.requests.push(Request::OpenUrl(value)),
                "ReportVariable" if self.trusted => self.requests.push(Request::ReportVariable(value)),
                "RequestUpload" if self.trusted => self.requests.push(Request::RequestUpload(value)),
                "SetColors" => {
                    for (name, v) in crate::osc1337::set_colors(&value) {
                        match (name.as_str(), crate::osc1337::color(&v)) {
                            ("preset", _) => {
                                // A loop of the sequence leaves one load pending, with at most one
                                // queued color per name behind it.
                                self.queue_preset(v);
                            }
                            ("tab", _) if v == "default" => self.queue_tab_color(None),
                            ("tab", Some(crate::osc1337::Color::Rgb(c))) => self.queue_tab_color(Some(c)),
                            ("tab", Some(crate::osc1337::Color::Index(i))) => {
                                let c = self.palette.indexed(i);
                                self.queue_tab_color(Some(c));
                            }
                            ("badge", Some(crate::osc1337::Color::Rgb(c))) => self.queue_badge_color(c),
                            ("badge", Some(crate::osc1337::Color::Index(i))) => {
                                let c = self.palette.indexed(i);
                                self.queue_badge_color(c);
                            }
                            // iTerm2 pauses the stream for a preset: colors after a pending one apply
                            // after it, in order (the window's handler).
                            (_, Some(c)) if self.pending.preset_pending() => self.queue_color_after_preset(&name, c),
                            (_, Some(c)) => {
                                if self.apply_color(&name, c) {
                                    self.color_sets.retain(|(n, _)| *n != name);
                                    self.color_sets.push((name, c));
                                }
                            }
                            (_, None) => {}
                        }
                    }
                }
                "Capabilities" if self.trusted => {
                    let f = crate::osc1337::session_features(self.clipboard_allowed, self.unicode.version, self.profile_unicode, self.ambiguous_wide());
                    let reply = format!("\x1b]1337;Capabilities={}\x07", crate::osc1337::capabilities(f));
                    self.replies.extend(reply.into_bytes());
                }
                "ExecFailed" => self.exec_failed = true,
                "Env" if self.trusted => {
                    if let crate::osc1337::EnvUpdate::Set(v) = crate::osc1337::parse_env(&value) {
                        self.env = v;
                    }
                }
                // Not trust-gated (`T:4524-4527`).
                "it2ssh" => {
                    if let Some(flags) = crate::ssh_handshake::parse_it2ssh(&value) {
                        self.requests.push(Request::SshWillBegin);
                        self.ssh_flags = Some(flags);
                    }
                }
                "SendConductor" => match crate::ssh_handshake::send_conductor(&value, self.ssh_flags.take()) {
                    crate::ssh_handshake::SendConductor::Ignore => {}
                    crate::ssh_handshake::SendConductor::Banner(text) => self.append_banner(text),
                    crate::ssh_handshake::SendConductor::Send(h) => self.requests.push(Request::SshSendConductor(h)),
                },
                // Trusted and non-empty only (`T:4528-4531`); the flags go (`TD:4033`).
                "EndSSH" if self.trusted && !value.is_empty() => {
                    self.ssh_flags = None;
                    self.requests.push(Request::SshEnd(value));
                }
                "SetKeyLabel" => self.key_labels.set(&value),
                "PushKeyLabels" => self.key_labels.push(&value),
                "PopKeyLabels" => self.key_labels.pop(&value),
                "UpdateBlock" => {
                    let args = crate::blocks::parse(&value);
                    match (crate::blocks::get(&args, "id"), crate::blocks::get(&args, "action")) {
                        (Some(id), Some("fold")) => {
                            self.fold_block(id);
                        }
                        (Some(id), Some("unfold")) => {
                            self.unfold_block(id);
                        }
                        _ => {}
                    }
                }
                "Button" => match crate::blocks::button_args(&crate::blocks::parse(&value)) {
                    crate::blocks::ButtonArgs::Add(kind) => self.add_button(kind),
                    crate::blocks::ButtonArgs::Invalidate => self.buttons.invalidate_custom(),
                    crate::blocks::ButtonArgs::Ignore => {}
                },
                "Block" => {
                    let args = crate::blocks::parse(&value);
                    let alt = self.term().mode().contains(alacritty_terminal::term::TermMode::ALT_SCREEN);
                    match (crate::blocks::get(&args, "id"), crate::blocks::get(&args, "attr")) {
                        (Some(id), Some("start")) if !alt => {
                            let n = self.blocks.start(id, crate::blocks::get(&args, "type"));
                            self.term.block = n;
                        }
                        (Some(id), Some("end")) => {
                            // A block that never started is not ended.
                            if let Some(n) = self.blocks.by_id.get(id).copied() {
                                self.term.block = self.blocks.end(id);
                                // render=1: the block's rows, through the cursor's, show rendered.
                                if crate::blocks::get(&args, "render") == Some("1") && !alt {
                                    if let Some((first, last)) = self.block_rows(id) {
                                        let last = last.max(self.term.grid().cursor.point.line.0);
                                        let source = crate::porthole::store::Source { hint: self.blocks.types.get(&n).cloned(), filename: None, wide: self.porthole_wide_default };
                                        self.render_range_estimated(first, last, source);
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
                "EndCopy" if self.trusted => {
                    self.capture_hold_clear();
                    if let Some(c) = self.capture.take() {
                        self.requests.push(crate::screen::Request::ClipboardWrite(String::from_utf8_lossy(&c).into_owned()));
                    }
                }
                "AddAnnotation" | "AddHiddenAnnotation" | "AddNote" | "AddHiddenNote" => {
                    let Some((text, len, at)) = crate::annotations::parse_osc(&value) else { return };
                    let cols = self.term().columns();
                    let (line, col) = match at {
                        Some((x, y)) => (y as i32, x.min(cols - 1)),
                        None => {
                            let p = self.cursor_point();
                            (p.line.0, p.column.0)
                        }
                    };
                    // No longer than the cells left on the screen from there.
                    let lines = self.term().screen_lines() as i32;
                    let line = line.clamp(0, lines - 1);
                    let left = (lines - line) as usize * cols - col;
                    let last = col + len.unwrap_or(cols - col).clamp(1, left) - 1;
                    let end_line = line + (last / cols) as i32;
                    self.add_note(line, col, end_line, last % cols, &text, key.starts_with("AddHidden"));
                }
                _ => {}
            },
        }
    }
}

#[cfg(test)]
mod tests {
    /// Final wave item 6: the hit test reads a bounded number of rows however long the scrollback.
    #[test]
    fn the_command_hit_walk_is_bounded() {
        let reads = std::cell::Cell::new(0);
        let none = |_: i32| {
            reads.set(reads.get() + 1);
            0
        };
        assert_eq!(nearest_mark(0, -1_000_000, HIT_WALK_ROWS, none), None);
        assert_eq!(reads.get(), HIT_WALK_ROWS as usize, "no marks in a million rows: 64 reads, not a million");
        assert_eq!(nearest_mark(10, -100, HIT_WALK_ROWS, |l| if l == 4 { 7 } else { 0 }), Some((7, 4)), "a mark a few rows up is found");
        assert_eq!(nearest_mark(0, -100, HIT_WALK_ROWS, |l| if l == -100 { 7 } else { 0 }), None, "a mark farther than the cap is not a hit");
        assert_eq!(nearest_mark(0, -3, HIT_WALK_ROWS, |_| 0), None, "the top of the buffer ends the walk");
    }

    use super::*;
    use crate::screen::Screen;

    fn prompt(s: &mut Screen, cmd: &str, code: i32) {
        s.feed(b"\x1b]133;A\x07$ \x1b]133;B\x07");
        s.feed(cmd.as_bytes());
        s.feed(b"\r\n\x1b]133;C\x07output\r\n");
        s.feed(format!("\x1b]133;D;{code}\x07").as_bytes());
    }

    #[test]
    fn a_command_becomes_a_mark_with_its_text_and_code() {
        let mut s = Screen::new(40, 10);
        prompt(&mut s, "ls -l", 2);
        let marks = s.marks_in_order();
        assert_eq!(marks.len(), 1);
        assert_eq!(marks[0].1.command, "ls -l");
        assert_eq!(marks[0].1.code, Some(2));
        assert!(marks[0].1.start.is_some() && marks[0].1.end.is_some());
        assert_eq!(color(&marks[0].1, true), MarkColor::Error);
        assert_eq!(s.shell.history[0].command, "ls -l");
        assert_eq!(s.shell.history[0].code, Some(2));
    }

    #[test]
    fn osc_preset_follows_iterm2() {
        let mut s = Screen::new(40, 10);
        let before = s.palette.clone();
        s.feed(b"\x1b]1337;SetColors=preset=Solarized Dark\x07");
        assert_eq!(s.take_requests(), vec![Request::ColorPreset("Solarized Dark".into())]);
        assert!(s.palette == before, "the request, not the parser, changes colors");
        // A program looping the sequence leaves one load pending.
        s.feed(b"\x1b]1337;SetColors=preset=A\x07\x1b]1337;SetColors=preset=B\x07");
        assert_eq!(s.take_requests(), vec![Request::ColorPreset("B".into())]);
    }

    fn built_in(name: &str) -> crate::color_presets::Preset {
        crate::color_presets::built_in().iter().find(|(n, _)| n == name).unwrap().1.clone()
    }

    /// What the window's request handler does with the color requests, in order.
    fn run_color_requests(s: &mut Screen, preset: &str) {
        let reqs = s.take_requests();
        run_color_requests_from(s, reqs, preset);
    }

    fn run_color_requests_from(s: &mut Screen, reqs: Vec<Request>, preset: &str) {
        for r in reqs {
            match r {
                Request::ColorPreset(_) => {
                    s.apply_escape_preset(&built_in(preset));
                }
                Request::NamedColor(n, c) => {
                    s.set_named_color(&n, c);
                }
                _ => {}
            }
        }
    }

    /// TWEAKS 4a: `SetColors=badge=` is a request for the session's badge color (it lives in the
    /// profile, not the palette), in stream order with a preset, bounded, and bad values ignored.
    #[test]
    fn set_colors_badge_is_a_request() {
        use alacritty_terminal::vte::ansi::Rgb;
        let orange = Rgb { r: 255, g: 128, b: 0 };
        let mut s = Screen::new(40, 10);
        s.feed(b"\x1b]1337;SetColors=badge=ff8000\x07");
        assert_eq!(s.take_requests(), vec![Request::BadgeColor(orange)]);
        s.feed(b"\x1b]1337;SetColors=badge=i:4\x07");
        let want = s.palette.ansi[4];
        assert_eq!(s.take_requests(), vec![Request::BadgeColor(want)]);
        for bad in ["badge=zzz", "badge=", "badge=i:999"] {
            s.feed(format!("\x1b]1337;SetColors={bad}\x07").as_bytes());
            assert!(s.take_requests().is_empty(), "{bad}");
        }
        // Before a preset the preset's own badge color wins; after it the program's does.
        s.feed(b"\x1b]1337;SetColors=badge=ff8000\x07\x1b]1337;SetColors=preset=Tango Dark\x07\x1b]1337;SetColors=badge=102030\x07");
        let reqs = s.take_requests();
        let at = |f: &dyn Fn(&Request) -> bool| reqs.iter().position(f).unwrap();
        assert!(at(&|r| *r == Request::BadgeColor(orange)) < at(&|r| matches!(r, Request::ColorPreset(_))));
        assert!(at(&|r| matches!(r, Request::ColorPreset(_))) < at(&|r| *r == Request::BadgeColor(Rgb { r: 0x10, g: 0x20, b: 0x30 })));
        // A flood keeps one request on each side.
        for i in 0..5000u32 {
            s.feed(format!("\x1b]1337;SetColors=badge={:06x}\x07\x1b]1337;SetColors=preset=Tango Dark\x07", i).as_bytes());
        }
        assert!(s.take_requests().iter().filter(|r| matches!(r, Request::BadgeColor(_))).count() <= 2);
    }

    /// iTerm2 pauses the stream for a preset, so SetColors after it (in the same payload or the
    /// next one) apply after it and are never overwritten by it.
    #[test]
    fn osc_preset_keeps_stream_order() {
        let red = alacritty_terminal::vte::ansi::Rgb { r: 255, g: 0, b: 0 };
        for feed in [&b"\x1b]1337;SetColors=preset=Tango Dark,fg=ff0000\x07"[..], &b"\x1b]1337;SetColors=preset=Tango Dark\x07\x1b]1337;SetColors=fg=ff0000\x07"[..]] {
            let mut s = Screen::new(40, 10);
            s.feed(feed);
            run_color_requests(&mut s, "Tango Dark");
            assert_eq!(s.palette.fg, red);
            assert_eq!(s.palette.bg, crate::color_presets::swatch(&built_in("Tango Dark"), true).1, "the preset's other colors still loaded");
        }
        // A color set BEFORE the preset is the preset's to overwrite.
        let mut s = Screen::new(40, 10);
        s.feed(b"\x1b]1337;SetColors=fg=ff0000\x07\x1b]1337;SetColors=preset=Tango Dark\x07");
        run_color_requests(&mut s, "Tango Dark");
        assert_eq!(s.palette.fg, crate::color_presets::swatch(&built_in("Tango Dark"), true).0);
    }

    /// A program looping `preset=X,fg=...` (or alternating a preset with other colors) queues a
    /// bounded number of requests, and the order that matters still holds.
    #[test]
    fn osc_preset_flood_is_bounded() {
        let mut s = Screen::new(40, 10);
        let line = b"\x1b]1337;SetColors=preset=Tango Dark,fg=ff0000,bg=00ff00\x07\x1b]1337;SetColors=bold=0000ff\x07";
        for _ in 0..10_000 {
            s.feed(line);
        }
        let reqs = s.take_requests();
        let presets = reqs.iter().filter(|r| matches!(r, Request::ColorPreset(_))).count();
        let named = reqs.iter().filter(|r| matches!(r, Request::NamedColor(..))).count();
        assert_eq!(presets, 1, "one preset load is pending");
        assert!(named <= 25, "at most one queued color per name, got {named}");
        run_color_requests_from(&mut s, reqs, "Tango Dark");
        let (red, green, blue) = (alacritty_terminal::vte::ansi::Rgb { r: 255, g: 0, b: 0 }, alacritty_terminal::vte::ansi::Rgb { r: 0, g: 255, b: 0 }, alacritty_terminal::vte::ansi::Rgb { r: 0, g: 0, b: 255 });
        assert_eq!((s.palette.fg, s.palette.bg, s.palette.bold), (red, green, blue), "the colors after the last preset apply on top of it");
        // The queue empties on take, and a color with no preset pending applies at once.
        s.feed(b"\x1b]1337;SetColors=fg=010203\x07");
        assert!(s.take_requests().is_empty());
        assert_eq!(s.palette.fg, alacritty_terminal::vte::ansi::Rgb { r: 1, g: 2, b: 3 });
        // Unknown names behind a pending preset queue nothing.
        s.feed(b"\x1b]1337;SetColors=preset=A\x07");
        for i in 0..1000 {
            s.feed(format!("\x1b]1337;SetColors=junk{i}=ff0000\x07").as_bytes());
        }
        assert_eq!(s.take_requests().len(), 1);
    }

    /// A program looping `preset=X`, then many tab and color changes in the same frame (the tab
    /// requests used to make every color scan the tail): the pending state is fixed size, so the
    /// work stays linear in the sequences fed, and the order that matters still holds.
    #[test]
    fn osc_preset_pending_state_is_fixed_size() {
        crate::screen::PENDING_WORK.with(|w| w.set(0));
        let mut s = Screen::new(40, 10);
        s.feed(b"\x1b]1337;SetColors=preset=Tango Dark\x07");
        let n = 5_000;
        for i in 0..n {
            s.feed(format!("\x1b]1337;SetColors=tab={:02x}0000,fg={:02x}0000\x07", i % 256, i % 256).as_bytes());
        }
        s.feed(b"\x1b]1337;SetColors=tab=00ff00,fg=0000ff\x07");
        let work = crate::screen::PENDING_WORK.with(|w| w.get());
        assert!(work <= 10 * n, "work {work} for {n} sequences is not linear");
        let reqs = s.take_requests();
        assert!(reqs.len() <= 27, "preset + 25 colors + tab at most, got {}", reqs.len());
        assert_eq!(reqs.iter().filter(|r| matches!(r, Request::ColorPreset(_))).count(), 1);
        let last_tab = reqs.iter().rev().find_map(|r| if let Request::TabColor(c) = r { Some(*c) } else { None });
        assert_eq!(last_tab, Some(Some(alacritty_terminal::vte::ansi::Rgb { r: 0, g: 255, b: 0 })), "the latest tab wins");
        run_color_requests_from(&mut s, reqs, "Tango Dark");
        assert_eq!(s.palette.fg, alacritty_terminal::vte::ansi::Rgb { r: 0, g: 0, b: 255 });
    }

    #[test]
    fn osc_preset_sets_named_colors_and_ends_with_the_program() {
        let mut s = Screen::new(40, 10);
        let profile = s.palette.clone();
        let tango = built_in("Tango Dark");
        let named = crate::color_presets::named_colors(&tango, true);
        s.set_named_colors(&named);
        let (fg, _) = crate::color_presets::swatch(&tango, true);
        assert_eq!(s.palette.fg, fg);
        assert!(s.palette != profile);
        // A profile change keeps what the program set; the program ending drops it.
        s.set_palette(profile.clone());
        assert_eq!(s.palette.fg, fg);
        s.program_ended();
        s.set_palette(profile.clone());
        assert!(s.palette == profile, "the profile's colors are back");
    }

    #[test]
    fn osc_preset_skips_unnameable_keys() {
        // No built-in preset has a match color (an optional key), so the most one names is 24; a
        // preset lacking a color (Solarized Dark has no link color) names fewer.
        let most = crate::color_presets::built_in().iter().map(|(_, p)| crate::color_presets::named_colors(p, true).len()).max().unwrap();
        assert_eq!(most, 24);
        let names: Vec<&str> = crate::color_presets::named_colors(&built_in("Solarized Dark"), true).into_iter().map(|(n, _)| n).collect();
        assert!(names.len() < 24 && !names.contains(&"link"), "{names:?}");
        for n in ["fg", "bg", "bold", "selbg", "selfg", "curbg", "curfg", "black", "white", "br_black", "br_white"] {
            assert!(names.contains(&n), "{n}");
        }
        assert!(!names.iter().any(|n| n.contains("underline") || n.contains("guide") || n.contains("badge") || *n == "tab"));
    }

    #[test]
    fn a_repeated_prompt_start_is_one_mark() {
        let mut s = Screen::new(40, 10);
        s.feed(b"\x1b]133;A\x07$ \x1b]133;B\x07");
        s.feed(b"\r\x1b[K\x1b]133;A\x07$ \x1b]133;B\x07l");
        assert_eq!(s.marks_in_order().len(), 1);
    }

    #[test]
    fn an_aborted_command_removes_its_mark() {
        let mut s = Screen::new(40, 10);
        s.feed(b"\x1b]133;A\x07$ \x1b]133;B\x07abc^C\r\n\x1b]133;D;130\x07");
        assert!(s.marks_in_order().is_empty());
    }

    #[test]
    fn a_mark_that_leaves_history_is_forgotten() {
        let mut s = Screen::with_scrollback(20, 3, 5);
        prompt(&mut s, "x", 0);
        for i in 0..20 {
            s.feed(format!("line{i}\r\n").as_bytes());
        }
        assert!(s.marks_in_order().is_empty());
        assert!(s.marks.records.is_empty());
    }

    #[test]
    fn output_of_the_last_command_is_its_range() {
        let mut s = Screen::new(40, 10);
        prompt(&mut s, "echo hi", 0);
        s.feed(b"\x1b]133;A\x07$ ");
        let (a, b) = s.last_output_range().unwrap();
        assert_eq!(s.term().bounds_to_string(a, b).trim_end(), "output");
        let (a, b) = s.last_command_range().unwrap();
        assert_eq!(s.term().bounds_to_string(a, b).trim_end(), "echo hi");
    }

    fn drain(s: &mut Screen) -> Vec<(String, usize, usize)> {
        s.take_path_candidates().into_iter().map(|c| (c.text, c.start, c.end)).collect()
    }

    #[test]
    fn prompt_paths_are_recorded_as_text() {
        let mut s = Screen::new(80, 10);
        s.feed(b"\x1b]133;A\x07PS C:\\Users\\rob> \x1b]133;B\x07");
        assert!(s.has_path_candidates());
        let got = s.take_path_candidates();
        assert_eq!(got.len(), 1);
        assert_eq!((got[0].text.as_str(), got[0].start, got[0].end), ("C:\\Users\\rob", 3, 15));
        assert_eq!(s.resolve(got[0].anchor), Some(Point::new(Line(0), Column(3))), "the anchor names the first cell");
        assert!(drain(&mut s).is_empty(), "taken once");
        assert!(!s.has_path_candidates());
        // A redrawn prompt (a second 133;B on the same mark) records nothing new.
        s.feed(b"\x1b]133;B\x07");
        assert!(drain(&mut s).is_empty());
        // The next prompt records its own.
        s.feed(b"\r\n\x1b]133;A\x07rob@box:~/src$ \x1b]133;B\x07");
        assert_eq!(drain(&mut s), [("~/src".to_string(), 8, 13)]);
    }

    #[test]
    fn a_prompt_of_six_rows_records_none() {
        let mut s = Screen::new(40, 12);
        s.feed(b"\x1b]133;A\x07/a\r\n/b\r\n/c\r\n/d\r\n/e\r\n/f> \x1b]133;B\x07");
        assert!(drain(&mut s).is_empty(), "MS:4786-4789: a long prompt is not looked at");
        let mut five = Screen::new(40, 12);
        five.feed(b"\x1b]133;A\x07/a\r\n/b\r\n/c\r\n/d\r\n/e> \x1b]133;B\x07");
        assert_eq!(drain(&mut five).len(), 4, "five rows are looked at, four paths kept");
    }

    #[test]
    fn sniff_never_probes_on_the_parse_path() {
        struct Panics;
        impl crate::netpath::Drives for Panics {
            fn dos_device(&self, _: char) -> Option<String> {
                panic!("the parse path asked a drive");
            }
            fn drive_type(&self, _: char) -> u32 {
                panic!("the parse path asked a drive");
            }
        }
        let mut s = Screen::new(80, 10);
        s.set_path_probe(crate::netpath::Probe { drives: std::sync::Arc::new(Panics), ..crate::netpath::Probe::default() });
        for i in 0..10_000 {
            s.feed(b"\x1b]133;A\x07PS /a /b /c /d /e> \x1b]133;B\x07\r\n");
            if i % 997 == 0 {
                assert!(s.take_path_candidates().len() <= 4);
            }
        }
        let last = s.take_path_candidates();
        assert_eq!(last.len(), 4, "at most four per prompt, and only the newest prompt waits");
        assert!(s.marks.path_marks.is_empty(), "nothing became a mark: the app resolves them");
    }

    #[test]
    fn path_marks_move_with_rows_and_end_with_the_program() {
        let mut s = Screen::new(80, 5);
        s.feed(b"\x1b]133;A\x07PS C:\\Users\\rob> \x1b]133;B\x07");
        let c = s.take_path_candidates().remove(0);
        s.add_path_marks(vec![crate::pathnav::PathMark { row_anchor: c.anchor, start: c.start, end: c.end, path: c.text.clone() }]);
        let at = |line: i32, col: usize| Point::new(Line(line), Column(col));
        assert_eq!(s.path_mark_at(at(0, 3)).map(|(i, m)| (i, m.path)), Some((0, "C:\\Users\\rob".to_string())));
        assert_eq!(s.path_mark_at(at(0, 14)).map(|(i, _)| i), Some(0));
        for p in [at(0, 2), at(0, 15), at(1, 3)] {
            assert_eq!(s.path_mark_at(p), None, "{p:?}");
        }
        // Rows scrolling up keep the mark: the anchor follows its prompt mark.
        for _ in 0..5 {
            s.feed(b"\r\n");
        }
        assert_eq!(s.path_mark_at(Point::new(Line(-1), Column(4))).map(|(i, _)| i), Some(0));
        assert_eq!(s.path_marks().len(), 1);
        s.program_ended();
        assert!(s.path_marks().is_empty(), "S5: an ended program's old prompt offers no cd");
        assert!(!s.has_path_candidates());
    }

    #[test]
    fn command_hit_follows_iterm2() {
        let mut s = Screen::new(40, 10);
        s.feed(b"\x1b]133;A\x07PS> \x1b]133;B\x07dir\r\n\x1b]133;C\x07x\r\n\x1b]133;D;0\x07");
        s.feed(b"\x1b]133;A\x07PS> \x1b]133;B\x07");
        let at = |line: i32, col: usize| Point::new(Line(line), Column(col));
        let id = s.marks_in_order().iter().find(|(_, m)| m.command == "dir").map(|(_, m)| m.id).unwrap();
        // Each cell of "dir" hits the mark with the command's range.
        for col in 4..7 {
            let (hit, (a, b)) = s.command_hit(at(0, col)).unwrap_or_else(|| panic!("col {col}"));
            assert_eq!(hit, id);
            assert_eq!(a, at(0, 4));
            assert!(b >= at(0, 6), "{b:?}");
        }
        // The prompt text, the output and the new prompt are not the command.
        for p in [at(0, 0), at(0, 3), at(1, 0), at(2, 1), at(2, 5)] {
            assert_eq!(s.command_hit(p), None, "{p:?}");
        }
        // A prompt with an empty command (Enter at once) is no hit.
        let mut e = Screen::new(40, 10);
        e.feed(b"\x1b]133;A\x07PS> \x1b]133;B\x07\r\n\x1b]133;C\x07\x1b]133;D;0\x07\x1b]133;A\x07PS> \x1b]133;B\x07");
        for p in [at(0, 3), at(0, 4), at(0, 6), at(1, 0)] {
            assert_eq!(e.command_hit(p), None, "{p:?}");
        }
        // The next command is its own mark: each point belongs to the prompt above it.
        s.feed(b"ls\r\n\x1b]133;C\x07out\r\n\x1b]133;D;0\x07");
        let second = s.command_hit(at(2, 5)).map(|(m, _)| m).unwrap();
        assert_ne!(second, id);
        assert_eq!(s.command_hit(at(0, 5)).map(|(m, _)| m), Some(id));
        assert_eq!(s.command_hit(at(3, 0)), None);
        // Not under a full-screen program (P7).
        s.feed(b"\x1b[?1049h");
        assert_eq!(s.command_hit(at(0, 5)), None);
    }

    #[test]
    fn a_commands_output_is_its_rows() {
        let mut s = Screen::new(40, 10);
        s.feed(b"\x1b]133;A\x07$ \x1b]133;B\x07one\r\n\x1b]133;C\x07l1\r\nl2  \r\n\r\n\x1b]133;D;0\x07");
        s.feed(b"\x1b]133;A\x07$ \x1b]133;B\x07two\r\n\x1b]133;C\x07partial");
        let ids: Vec<u32> = s.marks_in_order().iter().map(|(_, m)| m.id).collect();
        assert_eq!(ids.len(), 2);
        // The finished command ends at the row before the next prompt; blanks are trimmed.
        assert_eq!(s.output_text(ids[0]).as_deref(), Some("l1\nl2"));
        assert!(!s.command_is_running(ids[0]));
        // The newest runs to the cursor.
        assert_eq!(s.output_text(ids[1]).as_deref(), Some("partial"));
        assert!(s.command_is_running(ids[1]));
        assert_eq!(s.command_mark(ids[1]).map(|m| m.command.as_str()), Some("two"));
        assert_eq!(s.command_mark(999).map(|m| m.id), None);
        // A command that printed nothing has no output start.
        let mut e = Screen::new(40, 10);
        e.feed(b"\x1b]133;A\x07$ \x1b]133;B\x07\r\n\x1b]133;C\x07\x1b]133;D;0\x07");
        let id = e.marks_in_order().first().map(|(_, m)| m.id);
        assert!(id.is_none_or(|id| e.output_text(id).is_none()));
        s.feed(b"\x1b[?1049h");
        assert_eq!(s.output_text(ids[0]), None);
    }

    #[test]
    fn command_grid_lines_run_to_the_next_prompt() {
        let mut s = Screen::new(40, 12);
        s.feed(b"\x1b]133;A\x07$ \x1b]133;B\x07one\r\n\x1b]133;C\x07l1\r\nl2\r\n\x1b]133;D;0\x07");
        s.feed(b"\x1b]133;A\x07$ \x1b]133;B\x07two\r\n\x1b]133;C\x07x\r\n\x1b]133;D;0\x07");
        let ids: Vec<(i32, u32)> = s.marks_in_order().iter().map(|(l, m)| (*l, m.id)).collect();
        assert_eq!(ids.len(), 2);
        assert_eq!(s.command_grid_lines(ids[0].1), Some((ids[0].0, ids[1].0 - 1)));
        assert_eq!(s.command_grid_lines(ids[1].1), Some((ids[1].0, 11)));
        assert_eq!(s.command_grid_lines(999), None);
    }

    /// Merge seam (S6, Review Focus 8): buffer rows; a manual mark between prompts does not end
    /// a region; the last runs to the last row.
    #[test]
    fn command_region_follows_iterm2() {
        let mut s = Screen::new(40, 12);
        s.feed(b"\r\n\r\n");
        s.feed(b"\x1b]133;A\x07$ \x1b]133;B\x07one\r\n\x1b]133;C\x07o1\r\n\x1b]1337;SetMark\x07o2\r\no3\r\n\x1b]133;D;0\x07");
        s.feed(b"\x1b]133;A\x07$ \x1b]133;B\x07two\r\n\x1b]133;C\x07o\r\no\r\n\x1b]133;D;0\x07");
        s.feed(b"\x1b]133;A\x07$ \x1b]133;B\x07three");
        let prompts: Vec<(i32, u32)> = s.marks_in_order().iter().filter(|(_, m)| m.kind == Kind::Prompt).map(|(l, m)| (*l, m.id)).collect();
        assert_eq!(prompts.iter().map(|(l, _)| *l).collect::<Vec<_>>(), [2, 6, 9]);
        assert_eq!(s.command_region(prompts[0].1), Some((2, 5)));
        assert_eq!(s.command_region(prompts[1].1), Some((6, 8)));
        assert_eq!(s.command_region(prompts[2].1), Some((9, 11)));
        assert_eq!(s.command_region(999), None);
        // A manual mark is no region.
        let manual = s.marks_in_order().iter().find(|(_, m)| m.kind != Kind::Prompt).map(|(_, m)| m.id).unwrap();
        assert_eq!(s.command_region(manual), None);
        // Scrolled into history: buffer rows keep counting from the oldest row.
        s.feed(b"\r\n".repeat(5).as_slice());
        let h = { use alacritty_terminal::grid::Dimensions; s.term().grid().history_size() };
        assert!(h > 0);
        assert_eq!(s.command_region(prompts[0].1), Some((2, 5)));
    }

    /// P15: the region is asked once per grid change, not scanned per frame, and follows new output.
    #[test]
    fn the_region_is_cached_per_grid_generation() {
        let mut s = Screen::new(40, 12);
        s.feed(b"\x1b]133;A\x07$ \x1b]133;B\x07one");
        let id = s.marks_in_order().iter().find(|(_, m)| m.kind == Kind::Prompt).map(|(_, m)| m.id).unwrap();
        assert_eq!(s.command_region(id), Some((0, 11)));
        assert!(s.region_cache.is_some_and(|(_, m, r)| m == id && r == Some((0, 11))));
        // A second prompt ends the first region: the digest moved, so the answer did too.
        s.feed(b"\r\n\x1b]133;A\x07$ \x1b]133;B\x07two");
        assert_eq!(s.command_region(id), Some((0, 0)));
    }

    #[test]
    fn a_commands_output_folds_inside_its_region() {
        let mut s = Screen::new(40, 12);
        s.feed(b"\x1b]133;A\x07$ \x1b]133;B\x07one\r\n\x1b]133;C\x07l1\r\nl2\r\nl3\r\n\x1b]133;D;0\x07");
        s.feed(b"\x1b]133;A\x07$ \x1b]133;B\x07two");
        let id = s.marks_in_order().iter().map(|(_, m)| m.id).next().unwrap();
        let (a, b) = s.output_range_of(id).unwrap();
        assert!(s.fold(a.line.0, b.line.0).is_some());
        let (first, last) = s.command_grid_lines(id).unwrap();
        let fold = (first..=last).find_map(|l| s.fold_at(l));
        assert!(fold.is_some(), "the Unfold button finds the fold in the region");
        assert!(s.unfold(fold.unwrap()));
        assert!((first..=s.command_grid_lines(id).unwrap().1).all(|l| s.fold_at(l).is_none()));
    }

    /// Final wave minor 12: a fold elsewhere in the region is not the command's own.
    #[test]
    fn unfold_looks_only_at_the_row_the_fold_summary_is_on() {
        let mut s = Screen::new(40, 12);
        s.feed(b"\x1b]133;A\x07$ \x1b]133;B\x07one\r\n\x1b]133;C\x07l1\r\nl2\r\nl3\r\nl4\r\n\x1b]133;D;0\x07");
        s.feed(b"\x1b]133;A\x07$ \x1b]133;B\x07two");
        let id = s.marks_in_order().iter().map(|(_, m)| m.id).next().unwrap();
        let (a, b) = s.output_range_of(id).unwrap();
        assert_eq!(s.command_fold(id), None, "nothing folded");
        assert!(s.fold(a.line.0 + 2, b.line.0).is_some(), "the tail of the output");
        assert_eq!(s.command_fold(id), None, "a fold below the output's first row is not asked about");
        let mut t = Screen::new(40, 12);
        t.feed(b"\x1b]133;A\x07$ \x1b]133;B\x07one\r\n\x1b]133;C\x07l1\r\nl2\r\nl3\r\n\x1b]133;D;0\x07");
        t.feed(b"\x1b]133;A\x07$ \x1b]133;B\x07two");
        let id = t.marks_in_order().iter().map(|(_, m)| m.id).next().unwrap();
        let (a, b) = t.output_range_of(id).unwrap();
        let fold = t.fold(a.line.0, b.line.0).unwrap();
        assert_eq!(t.command_fold(id), Some(fold), "the fold on the output's first row is found");
    }

    #[test]
    fn a_command_mark_keeps_its_host_and_user() {
        let mut s = Screen::new(40, 10);
        s.feed(b"\x1b]1337;RemoteHost=rob@box\x07\x1b]133;A\x07$ \x1b]133;B\x07ls\r\n\x1b]133;C\x07\x1b]133;D;0\x07");
        let m = s.marks_in_order().pop().unwrap().1;
        assert_eq!((m.user.as_deref(), m.host.as_deref()), (Some("rob"), Some("box")));
        let mut t = Screen::new(40, 10);
        prompt(&mut t, "ls", 0);
        let m = t.marks_in_order().pop().unwrap().1;
        assert_eq!((m.user, m.host), (None, None));
    }

    /// 13w Task 4 (R3): the machine id token is kept (bounded), not only noticed.
    #[test]
    fn location_keeps_the_machine_token() {
        let loc = parse_location("file://u@h/C:/x?machineID=1:ab").unwrap();
        assert_eq!(loc.machine.as_deref(), Some("1:ab"));
        assert!(loc.has_machine_id);
        assert_eq!((loc.user.as_deref(), loc.host.as_deref(), loc.path.as_str()), (Some("u"), Some("h"), "/C:/x"));
        let none = parse_location("file://u@h/C:/x").unwrap();
        assert_eq!(none.machine, None);
        assert!(!none.has_machine_id);
        assert_eq!(parse_location("file://h/x?a=1&machineID=0:").unwrap().machine.as_deref(), Some("0:"));
        assert_eq!(parse_location("file://al%E2%80%AEice%00@h/x").unwrap().user.as_deref(), Some("alice"), "the user is cleaned like the host");
    }

    #[test]
    fn location_refuses_an_overlong_or_odd_token() {
        let long = format!("file://h/x?machineID=1:{}", "a".repeat(200));
        let loc = parse_location(&long).unwrap();
        assert_eq!(loc.machine, None, "over 130 characters is no token");
        assert!(loc.has_machine_id, "but the sequence still carries a machine id");
        for odd in ["1:a b", "1:a\u{202e}b", "1:%41", "1:a;b"] {
            let loc = parse_location(&format!("file://h/x?machineID={odd}")).unwrap();
            assert_eq!(loc.machine, None, "{odd}");
        }
        assert_eq!(parse_location(&format!("file://h/x?machineID=1:{}", "a".repeat(128))).unwrap().machine.map(|m| m.len()), Some(130));
        assert_eq!(parse_location("file://bo\u{202e}x/x").unwrap().host.as_deref(), Some("box"), "bidi marks are cut from a host");
    }

    /// The token decides locality when it is known, the host name when it is not; the locality
    /// is decided at the report and kept; a later report replaces the token.
    #[test]
    fn the_machine_token_decides_locality() {
        use crate::host_identity::hmac_sha256_hex;
        let mut s = Screen::new(40, 10);
        s.host_policy.local_names = vec!["pc".to_string()].into();
        s.host_policy.ours = Some("ab".into());
        s.feed(b"\x1b]7;file://elsewhere/tmp?machineID=1:AB\x07");
        assert_eq!(s.shell.host_local, Some(true), "our token wins over a foreign name");
        assert_eq!(s.shell.machine_id.as_deref(), Some("1:AB"));
        s.feed(b"\x1b]7;file://pc/tmp?machineID=1:\x07");
        assert_eq!(s.shell.host_local, Some(false), "1: alone is remote even under a local name");
        s.feed(b"\x1b]7;file://pc2/tmp?machineID=0:\x07\x1b]7;file://pc/tmp?machineID=0:\x07");
        assert_eq!(s.shell.host_local, Some(true), "0: falls back to the name");
        // RemoteHost carries no token: the old one must not decide for the new host.
        s.feed(b"\x1b]7;file://far/tmp?machineID=1:ab\x07\x1b]1337;RemoteHost=u@pc\x07");
        assert_eq!(s.shell.machine_id, None);
        assert_eq!(s.shell.host_local, Some(true));
        s.host_policy.ours = None;
        s.feed(b"\x1b]7;file://far2/tmp?machineID=1:ab\x07");
        assert_eq!(s.shell.host_local, Some(false), "no identity of ours: names only");
        let _ = hmac_sha256_hex(b"k", b"m");
    }

    /// B9: the offers use the same names as the host stack, and a fake replaces them.
    #[test]
    fn host_changed_uses_the_same_names() {
        let fake = crate::host_identity::local_names("fake.host", "PC", "pc.corp");
        assert!(host_changed(Some("PC"), "fake.host", &fake), "PC is no longer this machine");
        assert!(!host_changed(Some("fake.host"), "FAKE.HOST", &fake));
    }

    #[test]
    fn location_reports_set_the_directory_host_and_user() {
        let mut s = Screen::new(40, 10);
        s.feed(b"\x1b]7;file://rob@box/home/rob/my%20dir?machineID=1:\x07");
        assert_eq!((s.shell.user.as_deref(), s.shell.host.as_deref(), s.shell.cwd.as_deref()), (Some("rob"), Some("box"), Some("/home/rob/my dir")));
        assert!(s.marks_in_order().is_empty());
        s.feed(b"\x1b]1337;RemoteHost=a@b@c\x07\x1b]1337;SetUserVar=gitBranch=bWFpbg==\x07");
        assert_eq!((s.shell.user.as_deref(), s.shell.host.as_deref()), (Some("a@b"), Some("c")));
        assert_eq!(s.shell.vars["gitBranch"], "main");
        s.feed(b"\x1b]1337;CurrentDir=C:\\Users\\rob\x07\x1b]1337;ShellIntegrationVersion=1;shell=pwsh\x07");
        assert_eq!(s.shell.cwd.as_deref(), Some("C:\\Users\\rob"));
        assert_eq!((s.shell.shell.as_deref(), s.shell.version), (Some("pwsh"), Some(1)));
    }

    /// Each version report reaches the app with the shell it names, never an earlier
    /// report's (`VT100ScreenMutableState+TerminalDelegate.m:2814-2852`).
    #[test]
    fn version_reports_reach_the_app() {
        let reported = |s: &mut Screen| s.take_requests().into_iter().filter(|r| matches!(r, Request::ShellIntegrationReported { .. })).collect::<Vec<_>>();
        let r = |version, shell: Option<&str>| Request::ShellIntegrationReported { version, shell: shell.map(str::to_string) };
        let mut s = Screen::new(40, 10);
        s.feed(b"\x1b]1337;ShellIntegrationVersion=4;shell=bash\x07");
        assert_eq!(reported(&mut s), [r(Some(4), Some("bash"))]);
        s.feed(b"\x1b]1337;ShellIntegrationVersion=3\x07\x1b]1337;ShellIntegrationVersion=x;shell=zsh\x07");
        assert_eq!(reported(&mut s), [r(Some(3), None), r(None, Some("zsh"))]);
        assert_eq!(s.shell.shell.as_deref(), Some("zsh"));
        s.feed(b"\x1b[?1049h\x1b]1337;ShellIntegrationVersion=5;shell=fish\x07");
        assert_eq!(reported(&mut s), [r(Some(5), Some("fish"))], "reported on the alternate screen too");
        s.feed(b"\x1b]1337;ShellIntegrationVersion=22beta;shell=sh;shell=bash\x07");
        assert_eq!(reported(&mut s), [r(Some(22), Some("bash"))], "integerValue's leading digits; the last shell");
    }

    #[test]
    fn osc7_without_a_machine_id_is_also_a_prompt() {
        let mut s = Screen::new(40, 10);
        s.feed(b"\x1b]7;file://box/tmp\x07$ ");
        assert_eq!(s.marks_in_order().len(), 1);
    }

    #[test]
    fn prompts_start_in_the_first_column() {
        let mut s = Screen::new(40, 10);
        s.feed(b"no newline\x1b]133;A\x07$ ");
        let line = s.marks_in_order()[0].0;
        assert_eq!((line, s.line_text(line).as_str()), (1, "$"));
        // Under ConPTY the terminal cannot add a line ConPTY never sees; the mark goes on the
        // cursor's row, and the shell script starts its prompt on a fresh line itself.
        let mut pty = Screen::new(40, 10);
        pty.pty_backed = true;
        pty.feed(b"no newline\x1b]133;A\x07$ ");
        assert_eq!(pty.marks_in_order()[0].0, 0);
        let mut off = Screen::new(40, 10);
        off.place_prompt_first_column = false;
        off.feed(b"no newline\x1b]133;A\x07$ ");
        assert_eq!(off.marks_in_order()[0].0, 0);
    }

    #[test]
    fn line_style_marks_follow_iterm2() {
        let mut s = Screen::new(40, 10);
        s.line_style_marks = true;
        s.feed(b"\x1b]133;A\x07$ \r\n");
        // A later change does not restyle a mark that exists (iTerm2 decides at creation).
        s.line_style_marks = false;
        s.feed(b"\x1b]133;A\x07$ \r\n");
        s.feed(b"\x1b]1337;SetMark\x07");
        let marks = s.marks_in_order();
        assert_eq!(marks.len(), 3);
        assert!(marks[0].1.line_style, "made while the setting was on");
        assert!(!marks[1].1.line_style, "made while it was off");
        assert!(!marks[2].1.line_style, "a manual mark is never line-style");
    }

    #[test]
    fn set_mark_and_new_mark_notices() {
        let mut s = Screen::new(40, 10);
        s.feed(b"x\x1b]1337;SetMark\x07");
        let marks = s.marks_in_order();
        assert_eq!(marks.len(), 1);
        assert_eq!(marks[0].1.kind, Kind::Manual);
        assert!(s.take_new_mark());
        assert!(!s.take_new_mark());
        let id = s.set_manual_mark(0, Some("here".into()));
        assert_eq!(s.marks.records[&id].name.as_deref(), Some("here"));
    }

    #[test]
    fn clear_captured_output_bumps_the_mark_and_asks_the_tool_to_refresh() {
        let mut s = Screen::new(40, 10);
        prompt(&mut s, "cmd", 0);
        let id = s.last_command_mark().unwrap().id;
        assert_eq!(s.marks.records[&id].clear_count, 0);
        // Nothing captured yet: no count (`terminalClearCapturedOutput`), but a refresh.
        s.feed(b"\x1b]1337;ClearCapturedOutput\x07");
        assert_eq!(s.marks.records[&id].clear_count, 0);
        let refresh = |r: &crate::screen::Request| matches!(r, crate::screen::Request::Trigger(crate::trigger_eval::TriggerRequest::CapturedCleared));
        assert!(s.take_requests().iter().any(refresh));
        let c = crate::captured::Captured { line: "x".into(), values: vec![], command: None, state: false, abs_line: 0, mark: id };
        s.marks.records.get_mut(&id).unwrap().captured.push(c);
        s.feed(b"\x1b]1337;ClearCapturedOutput\x07");
        assert_eq!(s.marks.records[&id].clear_count, 1);
        let reqs = s.take_requests();
        assert!(reqs.iter().any(refresh), "{reqs:?}");
        // No command mark: nothing to bump, but the tool still gets asked to refresh.
        let mut t = Screen::new(40, 10);
        t.feed(b"\x1b]1337;ClearCapturedOutput\x07");
        assert!(t.take_requests().iter().any(refresh));
    }

    #[test]
    fn percent_decoding_is_safe() {
        assert_eq!(percent_decode("a%20b%C3%A9"), "a b\u{e9}");
        assert_eq!(percent_decode("%zz%%é%"), "%zz%%é%");
    }

    #[test]
    fn next_mark_wraps_both_ways() {
        let lines = [-40, -10, 3];
        assert_eq!(next_mark(&lines, None, true), Some(-40));
        assert_eq!(next_mark(&lines, None, false), Some(3));
        assert_eq!(next_mark(&lines, Some(-10), true), Some(3));
        assert_eq!(next_mark(&lines, Some(3), true), Some(-40));
        assert_eq!(next_mark(&lines, Some(-40), false), Some(3));
        assert_eq!(next_mark(&[], None, true), None);
    }

    #[test]
    fn clear_to_last_mark_picks_the_line_after() {
        // Marks at -5 and 2; the current prompt's mark is on 8, the cursor on 8.
        assert_eq!(clear_start(&[-5, 2, 8], Some(8), 8), 3);
        // A mark directly above the cursor goes too.
        assert_eq!(clear_start(&[-5, 7, 8], Some(8), 8), 7);
        // No mark: everything from the top.
        assert_eq!(clear_start(&[8], Some(8), 8), i32::MIN);
    }

    #[test]
    fn clearing_from_a_line_keeps_the_prompt_line() {
        let mut s = Screen::new(20, 6);
        s.feed(b"keep\r\nold1\r\nold2\r\n$ typed");
        s.clear_from(1);
        assert_eq!(s.all_text(), "keep\n$ typed\n");
        // The prompt goes to the top of the screen (where ConPTY's clear puts it); the kept
        // rows move into history.
        let c = s.term().grid().cursor.point;
        assert_eq!((c.line.0, c.column.0), (0, 7));
        assert_eq!(s.line_text(-1), "keep");
    }

    #[test]
    fn clearing_into_history_truncates_it() {
        let mut s = Screen::new(20, 3);
        for i in 0..8 {
            s.feed(format!("l{i}\r\n").as_bytes());
        }
        s.feed(b"$ ");
        s.clear_from(-3);
        let t = s.all_text();
        assert!(t.starts_with("l0\nl1\nl2\n$"), "{t:?}");
        assert!(!t.contains("l3"), "{t:?}");
    }

    #[test]
    fn an_alternate_screen_app_keeps_the_marks() {
        let mut s = Screen::new(40, 10);
        prompt(&mut s, "vim", 0);
        s.feed(b"\x1b[?1049hfull screen app\x1b[H\x1b[2J");
        assert_eq!(s.marks_in_order().len(), 1);
        assert!(s.visible_marks().is_empty(), "no bars over an alternate-screen app");
        s.feed(b"\x1b[?1049l");
        assert_eq!(s.marks_in_order().len(), 1);
        assert_eq!(s.marks.records.len(), 1);
    }

    #[test]
    fn a_manual_mark_shares_a_row_with_a_prompt_mark() {
        let mut s = Screen::new(40, 10);
        prompt(&mut s, "ls", 1);
        let line = s.marks_in_order()[0].0;
        s.set_manual_mark(line, Some("here".into()));
        let marks = s.marks_in_order();
        assert_eq!(marks.len(), 2, "{marks:?}");
        assert!(marks.iter().any(|(_, m)| m.command == "ls" && m.code == Some(1)));
        assert!(marks.iter().any(|(_, m)| m.name.as_deref() == Some("here")));
    }

    #[test]
    fn ranges_survive_a_resize() {
        let mut s = Screen::new(20, 12);
        s.feed(b"\x1b]133;A\x07$ \x1b]133;B\x07echo abcdefghijklmnopqrstuvwxyz\r\n\x1b]133;C\x07");
        s.feed(b"0123456789ABCDEFGHIJKLMNOPQRSTUV\r\ntail\r\n\x1b]133;D;0\x07\x1b]133;A\x07$ ");
        let text = |s: &Screen| {
            let (a, b) = s.last_output_range().unwrap();
            s.term().bounds_to_string(a, b).trim_end().to_string()
        };
        let cmd = |s: &Screen| {
            let (a, b) = s.last_command_range().unwrap();
            s.term().bounds_to_string(a, b).trim_end().to_string()
        };
        let (out, command) = (text(&s), cmd(&s));
        assert_eq!(command, "echo abcdefghijklmnopqrstuvwxyz");
        for cols in [13, 31, 20] {
            s.resize(cols, 12);
            assert_eq!(text(&s), out, "at {cols} columns");
            assert_eq!(cmd(&s), command, "at {cols} columns");
        }
    }

    #[test]
    fn a_two_line_prompt_keeps_its_mark_after_a_clear() {
        let mut s = Screen::new(40, 10);
        prompt(&mut s, "ls", 0);
        s.feed(b"\x1b]133;A\x07info line\r\n$ \x1b]133;B\x07ty");
        let first = s.marks_in_order()[0].0;
        s.clear_from(first + 1);
        let marks = s.marks_in_order();
        let (line, m) = marks.last().unwrap();
        assert_eq!((*line, m.command.as_str()), (0, ""));
        s.feed(b"\r\n\x1b]133;C\x07");
        assert_eq!(s.marks_in_order().last().unwrap().1.command, "ty");
    }

    #[test]
    fn named_marks_rename_and_remove_like_iterm2() {
        let mut s = Screen::new(40, 10);
        prompt(&mut s, "ls", 0);
        let (pline, pid) = s.marks_in_order().first().map(|(l, m)| (*l, m.id)).unwrap();
        let a = s.set_manual_mark(pline + 2, Some("a".into()));
        let c = s.set_manual_mark(pline + 3, Some("c".into()));
        // Renamed: the same row and the same id.
        assert!(s.rename_mark(a, Some("b".into())));
        let marks = s.marks_in_order();
        assert_eq!(marks.len(), 3);
        assert!(marks.iter().any(|(l, m)| *l == pline + 2 && m.id == a && m.name.as_deref() == Some("b")));
        // No name on a manual mark removes it.
        assert!(s.rename_mark(a, None));
        assert!(!s.marks.records.contains_key(&a));
        assert!(!s.row_marks().iter().any(|(_, id)| *id == a));
        // A prompt mark only loses its name.
        assert!(s.rename_mark(pid, Some("p".into())));
        assert_eq!(s.marks.records[&pid].name.as_deref(), Some("p"));
        assert!(s.rename_mark(pid, None));
        assert_eq!(s.marks.records[&pid].name, None);
        assert!(s.marks_in_order().iter().any(|(_, m)| m.id == pid));
        // Remove drops the record and the row's slot, whatever the kind.
        assert!(s.remove_mark(c));
        assert!(!s.marks.records.contains_key(&c));
        assert!(!s.row_marks().iter().any(|(_, id)| *id == c));
        assert!(s.remove_mark(pid));
        assert!(s.marks_in_order().is_empty());
        // Unknown ids.
        assert!(!s.rename_mark(999, Some("x".into())));
        assert!(!s.rename_mark(c, Some("x".into())));
        assert!(!s.remove_mark(999));
        assert!(!s.remove_mark(c));
    }

    #[test]
    fn cleared_and_trimmed_named_marks_are_gone() {
        // Clear Buffer (to the last mark) and Clear Scrollback drop the records too.
        let mut s = Screen::with_scrollback(20, 3, 5);
        s.set_manual_mark(0, Some("a".into()));
        for i in 0..4 {
            s.feed(format!("line{i}\r\n").as_bytes());
        }
        assert_eq!(s.live_named_marks().len(), 1);
        s.clear(true);
        assert!(s.live_named_marks().is_empty());
        assert!(s.marks.records.is_empty());
        s.feed(b"x\r\ny\r\nz");
        let b = s.set_manual_mark(1, Some("b".into()));
        s.clear_from(0);
        assert!(!s.marks.records.contains_key(&b));
        assert!(s.live_named_marks().is_empty());
        // Trimmed from the scrollback: gone from the live list before any prune.
        let c = s.set_manual_mark(0, Some("c".into()));
        assert_eq!(s.live_named_marks(), vec![(c, Some("c".into()))]);
        for i in 0..20 {
            s.feed(format!("line{i}\r\n").as_bytes());
        }
        assert!(s.marks.records.contains_key(&c), "no prune has run yet");
        assert!(s.live_named_marks().is_empty());
    }

    #[test]
    fn colors_follow_the_exit_code() {
        let m = |code| Mark { code, ..Mark::new(1, Kind::Prompt) };
        assert_eq!(color(&m(None), true), MarkColor::Success);
        assert_eq!(color(&m(Some(0)), true), MarkColor::Success);
        assert_eq!(color(&m(Some(130)), true), MarkColor::Other);
        assert_eq!(color(&m(Some(1)), true), MarkColor::Error);
    }
    #[test]
    fn composer_events_follow_osc_133() {
        use crate::prompt_state::{Event, State};
        let texts = |ev: &[Event]| -> Vec<Vec<String>> { ev.iter().filter_map(|e| if let Event::Reveal { text, .. } = e { Some(text.clone()) } else { None }).collect() };
        let mut s = Screen::new(40, 10);
        s.prompt_state.enabled = true;
        s.feed(b"\x1b]133;A\x07$ \x1b]133;B\x07");
        let ev = s.take_composer_events();
        assert_eq!(texts(&ev), vec![vec!["$ ".to_string()]]);
        assert_eq!(ev.len(), 1);
        let Event::Reveal { rows, .. } = &ev[0] else { panic!() };
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].runs.iter().map(|r| r.text.as_str()).collect::<String>(), "$ ");
        assert_eq!(s.prompt_rows().1, vec!["$ ".to_string()]);
        assert_eq!(s.take_marks_added(), 1);
        assert_eq!(s.take_marks_added(), 0);
        // What the shell echoes after B is appended, the first part trimmed.
        s.feed(b" l");
        assert_eq!(s.take_composer_events(), vec![Event::Append("l".into())]);
        s.feed(b"s");
        assert_eq!(s.take_composer_events(), vec![Event::Append("s".into())]);
        // A backspace moves the cursor left: nothing new; a retype at or left of the last
        // accepted column is rejected; past it, appended.
        s.feed(b"\x08 \x08");
        assert!(s.take_composer_events().is_empty());
        s.feed(b"s");
        assert!(s.take_composer_events().is_empty());
        s.feed(b" -l");
        assert_eq!(s.take_composer_events(), vec![Event::Append(" -l".into())]);
        assert!(matches!(&s.prompt_state.state, State::Accruing { so_far, .. } if so_far == "ls -l"));
        // C then D: Dismiss once (at C), then Ground.
        s.feed(b"\r\n\x1b]133;C\x07out\r\n\x1b]133;D;0\x07");
        assert_eq!(s.take_composer_events(), vec![Event::Dismiss]);
        assert_eq!(s.prompt_state.state, State::Ground);
        // Echo in the same read as B, and a send: EchoingBack, C dismisses, Executing.
        s.feed(b"\x1b]133;A\x07$ \x1b]133;B\x07");
        s.take_composer_events();
        s.prompt_state.will_send();
        s.feed(b"echo hi\r\n");
        assert!(s.take_composer_events().is_empty(), "no accrual while echoing back");
        s.feed(b"\x1b]133;C\x07hi\r\n");
        assert_eq!(s.take_composer_events(), vec![Event::Dismiss]);
        assert_eq!(s.prompt_state.state, State::Executing);
        s.feed(b"\x1b]133;D;0\x07\x1b]133;A\x07$ \x1b]133;B\x07ls");
        assert_eq!(texts(&s.take_composer_events()).len(), 1);
        assert!(matches!(&s.prompt_state.state, State::Accruing { so_far, .. } if so_far == "ls"));
        // Each prompt and each SetMark counts.
        assert_eq!(s.take_marks_added(), 2);
        s.feed(b"\x1b]1337;SetMark\x07");
        assert_eq!(s.take_marks_added(), 1);
        // A two-line prompt keeps both rows, dropping the blank one.
        s.feed(b"\r\n\x1b]133;C\x07\x1b]133;D;0\x07\x1b]133;A\x07info\r\n\r\n> \x1b]133;B\x07");
        assert_eq!(texts(&s.take_composer_events()), vec![vec!["info".to_string(), "> ".to_string()]]);
        // The alternate screen: Disabled with Dismiss; leaving it: Ground.
        s.feed(b"\x1b[?1049h");
        assert_eq!(s.take_composer_events(), vec![Event::Dismiss]);
        assert_eq!(s.prompt_state.state, State::Disabled);
        s.feed(b"\x1b]133;A\x07$ \x1b]133;B\x07");
        assert!(s.take_composer_events().is_empty());
        s.feed(b"\x1b[?1049l");
        assert!(s.take_composer_events().is_empty());
        assert_eq!(s.prompt_state.state, State::Ground);

        // With Auto Composer off: no events, the session only becomes eligible.
        let mut s = Screen::new(40, 10);
        s.feed(b"\x1b]133;A\x07$ \x1b]133;B\x07ls");
        assert!(s.take_composer_events().is_empty());
        assert!(s.prompt_state.eligible);
        assert!(s.prompt_state.entering());
    }
    #[test]
    fn accruing_through_long_output_does_bounded_work() {
        use crate::prompt_state::{Event, State, MAX_ACCRUED};
        // A shell that sends A and B but C late stays Accruing through the output.
        let mut s = Screen::with_scrollback(40, 10, 1000);
        s.prompt_state.enabled = true;
        s.feed(b"\x1b]133;A\x07$ \x1b]133;B\x07ls");
        s.take_composer_events();
        let before = s.composer_scanned;
        for i in 0..300 {
            s.feed(format!("\r\noutput line {i:04}").as_bytes());
        }
        // Each read scans only what is new: about one row or two per read, not every row
        // since B (which would be about 300 * 150 rows of 40 cells).
        let scanned = s.composer_scanned - before;
        assert!(scanned <= 300 * 2 * 40, "scanned {scanned} cells");
        // At the cap, nothing more is read at all.
        let State::Accruing { so_far, .. } = &s.prompt_state.state else { panic!("{:?}", s.prompt_state.state) };
        let mut s2 = Screen::with_scrollback(40, 10, 1000);
        s2.prompt_state.enabled = true;
        s2.feed(b"\x1b]133;A\x07$ \x1b]133;B\x07");
        s2.feed("x".repeat(MAX_ACCRUED).as_bytes());
        assert!(matches!(&s2.prompt_state.state, State::Accruing { so_far, .. } if so_far.len() == MAX_ACCRUED));
        let at_cap = s2.composer_scanned;
        s2.feed(b"\r\nmore output\r\nand more");
        assert_eq!(s2.composer_scanned, at_cap);
        assert!(!so_far.is_empty());
        // Blank cells do not count toward the bound: echo after many blank rows still accrues.
        let mut s4 = Screen::with_scrollback(40, 10, 1000);
        s4.prompt_state.enabled = true;
        s4.feed(b"\x1b]133;A\x07$ \x1b]133;B\x07ls");
        s4.take_composer_events();
        let blank = format!("\x1b[{}B", 9);
        let mut burst = String::new();
        for _ in 0..600 {
            burst.push_str("\r\n");
        }
        s4.feed(format!("{burst}{blank}ok").as_bytes());
        assert_eq!(s4.take_composer_events(), vec![Event::Append("ok".into())]);
        // A part larger than the cap is dropped without reading past the bound.
        let mut s3 = Screen::with_scrollback(40, 10, 1000);
        s3.prompt_state.enabled = true;
        s3.feed(b"\x1b]133;A\x07$ \x1b]133;B\x07ls");
        s3.take_composer_events();
        let before = s3.composer_scanned;
        s3.feed("y".repeat(MAX_ACCRUED * 3).as_bytes());
        assert!(s3.take_composer_events().is_empty());
        assert!(s3.composer_scanned - before <= MAX_ACCRUED as u64 + 2 * 40);
        s3.feed(b"z");
        assert_eq!(s3.take_composer_events(), vec![Event::Append("z".into())]);
    }

    #[test]
    fn any_prompt_kind_reaches_the_machine() {
        use crate::prompt_state::{Event, State};
        let mut s = Screen::new(40, 10);
        s.prompt_state.enabled = true;
        s.feed(b"\x1b]133;A\x07$ \x1b]133;B\x07");
        s.take_composer_events();
        let marks = s.marks_in_order().len();
        // A continuation prompt (k=s) dismisses as PSM:291 does for any A, makes no mark, and
        // keeps the prompt range (so B does not re-capture: the range is unchanged).
        s.feed(b"\r\n\x1b]133;A;k=s\x07> ");
        assert_eq!(s.take_composer_events(), vec![Event::Dismiss]);
        assert_eq!(s.prompt_state.state, State::ReceivingPrompt);
        assert_eq!(s.marks_in_order().len(), marks);
        assert_eq!(s.prompt_rows().1, vec!["$ ".to_string()]);
    }
    #[test]
    fn accruing_and_the_prompt_survive_a_resize() {
        use crate::prompt_state::Event;
        for (cols, rows) in [(40, 20), (20, 10), (60, 6)] {
            let mut s = Screen::with_scrollback(40, 10, 1000);
            s.prompt_state.enabled = true;
            for i in 0..30 {
                s.feed(format!("old output line {i:02} that is long enough to wrap\r\n").as_bytes());
            }
            s.feed(b"\x1b]133;A\x07$ \x1b]133;B\x07ls");
            s.take_composer_events();
            s.resize(cols, rows);
            s.feed(b"x");
            assert_eq!(s.take_composer_events(), vec![Event::Append("x".into())], "{cols}x{rows}");
            assert_eq!(s.prompt_rows().1, vec!["$ ".to_string()], "{cols}x{rows}");
            // A repeated B at the same place after the resize is still a duplicate: the
            // cursor goes back to the command start first.
            s.feed(b"\x08\x08\x08\x1b]133;B\x07");
            assert!(!s.take_composer_events().iter().any(|e| matches!(e, Event::Reveal { .. })), "{cols}x{rows}");
        }
    }
    #[test]
    fn the_alternate_screen_never_feeds_the_composer() {
        use crate::prompt_state::{Event, State};
        let mut s = Screen::with_scrollback(40, 10, 1000);
        s.prompt_state.enabled = true;
        for i in 0..30 {
            s.feed(format!("old output line {i:02}\r\n").as_bytes());
        }
        s.feed(b"\x1b]133;A\x07$ \x1b]133;B\x07ls");
        s.take_composer_events();
        let accruing = s.prompt_state.state.clone();
        // A full-screen program started while typing (Ctrl+X Ctrl+E, fzf): Dismiss only, and
        // its drawing is never accrued.
        s.feed(b"\x1b[?1049h\x1b[H\x1b[2Jhello fullscreen\r\n~\r\n~");
        assert_eq!(s.take_composer_events(), vec![Event::Dismiss]);
        // Even with the machine put back in Accruing (the guard, not the state, stops it).
        s.prompt_state.state = accruing.clone();
        s.feed(b"more drawing");
        assert!(s.take_composer_events().is_empty());
        s.feed(b"\x1b[?1049l");
        // Leaving goes to Ground (PSM setAllowed); the next B would start a fresh checkpoint.
        assert_eq!(s.prompt_state.state, State::Ground);
        assert!(s.take_composer_events().is_empty());
        // The checkpoint is on the main screen's cursor: back in Accruing, one char is
        // exactly one Append, with no full-screen text and nothing from the buffer top.
        s.prompt_state.state = accruing;
        s.feed(b"y");
        assert_eq!(s.take_composer_events(), vec![Event::Append("y".into())]);
    }

    #[test]
    fn a_resize_on_the_alternate_screen_keeps_the_prompt() {
        let mut s = Screen::with_scrollback(40, 10, 1000);
        s.prompt_state.enabled = true;
        for i in 0..30 {
            s.feed(format!("old output line {i:02} that is long enough to wrap\r\n").as_bytes());
        }
        s.feed(b"\x1b]133;A\x07$ \x1b]133;B\x07ls");
        s.feed(b"\x1b[?1049h\x1b[H\x1b[2Jfull screen");
        s.resize(30, 14);
        s.feed(b"\x1b[?1049l");
        assert_eq!(s.prompt_rows().1, vec!["$ ".to_string()]);
    }

    #[test]
    fn offscreen_facts_follow_the_prompt_marks() {
        let mut s = Screen::with_scrollback(20, 5, 1000);
        s.feed(b"\x1b]133;A\x07$ \x1b]133;B\x07ls\r\n\x1b]133;C\x07");
        for i in 0..10 {
            s.feed(format!("o{i}\r\n").as_bytes());
        }
        s.feed(b"\x1b]133;D;0\x07\x1b]133;A\x07$ \x1b]133;B\x07");
        let f = s.offscreen_facts();
        assert_eq!(f.len(), 2, "{f:?}");
        assert_eq!((f[0].prompt_line, f[0].command_line, f[0].command_end_line), (-7, Some(-7), Some(-7)));
        assert_eq!((f[0].has_code, f[0].running, f[0].command_empty, f[0].output_rows), (true, false, false, 11));
        assert_eq!((f[1].prompt_line, f[1].has_code), (4, false));
        assert!(f[1].command_empty);
        // Cached until the grid changes.
        assert_eq!(s.offscreen_facts(), f);
        s.feed(b"x\r\n");
        assert_ne!(s.offscreen_facts(), f);
    }

    #[test]
    fn prompt_facts_follow_the_marks() {
        let mut s = Screen::new(20, 5);
        s.feed(b"\x1b]133;A\x07$ \x1b]133;B\x07ls\r\n\x1b]133;C\x07out\r\n\x1b]133;D;2\x07");
        let ids = s.prompt_ids();
        assert_eq!(ids.len(), 1);
        let first = ids[0];
        let f = s.prompt_facts(None).unwrap();
        assert_eq!(f.id, first);
        assert_eq!(f.prompt, Some(((0, 0), (2, 0))));
        assert_eq!(f.command.map(|c| c.0), Some((2, 0)));
        assert_eq!(f.output_start, Some((0, 1)));
        assert_eq!(f.text.as_deref(), Some("ls"));
        assert_eq!(f.code, Some(2));
        s.feed(b"\x1b]133;A\x07$ \x1b]133;B\x07");
        let g = s.prompt_facts(None).unwrap();
        // At B the command range is empty at the prompt's end, then runs to the cursor.
        assert_eq!(g.command, Some(((2, 2), (2, 2))));
        s.feed(b"  ");
        assert_eq!(s.prompt_facts(None).unwrap().command, Some(((2, 2), (2, 2))));
        s.feed(b"\x08\x08ls");
        let typing = s.prompt_facts(None).unwrap();
        assert_eq!(typing.command, Some(((2, 2), (4, 2))));
        assert_eq!(typing.text.as_deref(), Some("ls"));
        s.feed(b"\x08\x08");
        assert_ne!(g.id, first);
        assert_eq!((g.code, g.output_start), (None, None));
        assert_eq!(s.prompt_ids(), vec![first, g.id]);
        assert_eq!(s.prompt_facts(Some(first)), Some(crate::api_buffer::PromptFacts { cursor: g.cursor, ..f }));
    }

    #[test]
    fn an_empty_command_has_no_output_start() {
        let mut s = Screen::new(20, 5);
        s.feed(b"\x1b]133;A\x07$ \x1b]133;B\x07\r\n\x1b]133;C\x07");
        let f = s.prompt_facts(None).unwrap();
        assert_eq!((f.output_start, f.code), (None, None));
        // No command and none being typed: no text.
        assert_eq!(f.text, None);
        assert_eq!(f.command, Some(((2, 0), (2, 0))));
    }

    #[test]
    fn api_events_follow_the_prompt() {
        use crate::api_notify::ApiEvent;
        use crate::trigger_events::EventKind;
        let mut s = Screen::new(40, 10);
        s.api_watch = true;
        s.feed(b"\x1b]133;A\x07$ \x1b]133;B\x07ls\r\n\x1b]133;C\x07\x1b]133;D;1\x07\x1b]1337;Custom=id=me:hi\x07");
        let m = s.marks.last_command.expect("a command mark");
        assert_eq!(
            std::mem::take(&mut s.api_events),
            vec![
                ApiEvent::Prompt(Some(m)),
                ApiEvent::CommandStart { command: Some("ls".into()), mark: Some(m) },
                ApiEvent::CommandEnd { code: 1, mark: Some(m) },
                ApiEvent::Custom { id: Some("me".into()), payload: "hi".into() },
            ]
        );
        // Without its colon a custom sequence is nothing: no API event and no Custom Escape
        // Sequence trigger event (`VT:4492-4508`).
        let mut extra = toml::Table::new();
        extra.insert("eventParams".into(), toml::Value::Table(toml::Table::new()));
        let trigger = crate::triggers::Trigger { regex: String::new(), action: "AlertTrigger".into(), parameter: "custom".into(), instant: false, enabled: true, name: String::new(), job: String::new(), match_type: EventKind::CustomEscapeSequence.match_type(), extra };
        s.set_triggers(vec![trigger], crate::trigger_eval::TriggerPrefs::default());
        let alerts = |s: &mut Screen| s.take_requests().into_iter().filter(|r| matches!(r, Request::Trigger(crate::trigger_eval::TriggerRequest::Alert { .. }))).count();
        s.feed(b"\x1b]1337;Custom=id=me\x07");
        assert!(s.api_events.is_empty());
        assert_eq!(alerts(&mut s), 0);
        s.feed(b"\x1b]1337;Custom=id=me:\x07");
        assert_eq!(std::mem::take(&mut s.api_events), vec![ApiEvent::Custom { id: Some("me".into()), payload: String::new() }]);
        assert_eq!(alerts(&mut s), 1, "with its colon the trigger event fires");
        // No `id` parameter: the sender identity is unset (`PS:18276`).
        s.feed(b"\x1b]1337;Custom=x=y:hi\x07");
        assert_eq!(std::mem::take(&mut s.api_events), vec![ApiEvent::Custom { id: None, payload: "hi".into() }]);
        s.take_requests();
        // Unwatched, nothing is queued.
        s.api_watch = false;
        s.feed(b"\x1b]133;A\x07$ \x1b]133;B\x07ls\r\n\x1b]133;C\x07\x1b]133;D;0\x07\x1b]1337;Custom=id=me:hi\x07");
        assert!(s.api_events.is_empty());
    }

    #[test]
    fn a_finished_command_is_queued_only_while_a_chat_watches() {
        let mut s = Screen::new(40, 10);
        let ended = |s: &mut Screen| s.take_requests().into_iter().filter_map(|r| if let Request::CommandEnded { code, mark } = r { Some((code, mark)) } else { None }).collect::<Vec<_>>();
        s.feed(b"\x1b]133;A\x07$ \x1b]133;B\x07ls\r\n\x1b]133;C\x07\x1b]133;D;2\x07");
        assert!(ended(&mut s).is_empty(), "unwatched: nothing");
        s.chat_watch = true;
        s.feed(b"\x1b]133;A\x07$ \x1b]133;B\x07pwd\r\n\x1b]133;C\x07\x1b]133;D;5\x07");
        let m = s.marks.last_command;
        assert_eq!(ended(&mut s), vec![(5, m)]);
        // A command that never started (Ctrl+C at the prompt) ended nothing.
        s.feed(b"\x1b]133;A\x07$ \x1b]133;B\x07\x1b]133;D\x07");
        assert!(ended(&mut s).is_empty());
        s.chat_watch = false;
        s.feed(b"\x1b]133;A\x07$ \x1b]133;B\x07id\r\n\x1b]133;C\x07\x1b]133;D;0\x07");
        assert!(ended(&mut s).is_empty(), "watching stopped");
    }

    #[test]
    fn a_finished_command_is_queued_while_codecierge_watches_too() {
        let mut s = Screen::new(40, 10);
        let ended = |s: &mut Screen| s.take_requests().into_iter().filter_map(|r| if let Request::CommandEnded { code, mark } = r { Some((code, mark)) } else { None }).collect::<Vec<_>>();
        s.codecierge_watch = true;
        s.feed(b"\x1b]133;A\x07$ \x1b]133;B\x07ls\r\n\x1b]133;C\x07\x1b]133;D;3\x07");
        let m = s.marks.last_command;
        assert_eq!(ended(&mut s), vec![(3, m)], "one event kind for both watchers");
        s.codecierge_watch = false;
        s.feed(b"\x1b]133;A\x07$ \x1b]133;B\x07id\r\n\x1b]133;C\x07\x1b]133;D;0\x07");
        assert!(ended(&mut s).is_empty());
    }

    /// Host change offers: only when a previous host existed, the name changed and the two
    /// are not both this machine (`PS:17929-17940`); the fish flag rides along.
    /// A host change turns modes off with Terminal State's own sequences (one table).
    #[test]
    fn host_change_uses_the_terminal_state_toggles() {
        assert_eq!(toggle_off("Mouse Reporting"), b"\x1b[?1000l\x1b[?1002l\x1b[?1003l");
        assert_eq!(toggle_off("Focus Reporting"), b"\x1b[?1004l");
        assert_eq!(toggle_off("Paste Bracketing"), b"\x1b[?2004l");
        assert_eq!(toggle_off("No Such Toggle"), b"");
    }

    #[test]
    fn host_change_follows_iterm2() {
        let pc = ["PC".to_string()];
        assert!(!host_changed(None, "b", &pc), "no previous host");
        assert!(!host_changed(Some("a"), "a", &pc));
        assert!(host_changed(Some("a"), "A", &pc), "iTerm2 compares names exactly (isEqualToString:)");
        assert!(host_changed(Some("a"), "b", &pc));
        assert!(!host_changed(Some("pc"), "PC", &pc), "both this machine");
        assert!(host_changed(Some("PC"), "b", &pc));
        let changes = |s: &mut Screen| s.take_requests().into_iter().filter(|r| matches!(r, Request::HostChanged(l) if *l != LeftOn::default())).collect::<Vec<_>>();
        let offer = |mouse, focus, paste| vec![Request::HostChanged(LeftOn { mouse, focus, paste })];
        let mut s = Screen::new(40, 10);
        s.host_policy.mouse_allowed = true;
        s.feed(b"\x1b[?1000h\x1b[?1004h\x1b[?2004h\x1b]1337;RemoteHost=u@a\x07");
        assert!(changes(&mut s).is_empty(), "the first host is not a change");
        s.feed(b"\x1b]1337;RemoteHost=u@a\x07\x1b]1337;RemoteHost=v@a\x07");
        assert!(changes(&mut s).is_empty(), "same host, another user");
        s.feed(b"\x1b]1337;RemoteHost=u@b\x07");
        assert_eq!(changes(&mut s), offer(true, true, true), "unset answers offer each mode left on");
        // Mouse reporting counts only where the pane may report the mouse; nothing left on,
        // nothing offered.
        s.host_policy.mouse_allowed = false;
        s.feed(b"\x1b]1337;RemoteHost=u@c\x07");
        assert_eq!(changes(&mut s), offer(false, true, true));
        s.feed(b"\x1b[?1000l\x1b[?1004l\x1b[?2004l\x1b]1337;RemoteHost=u@d\x07");
        assert!(changes(&mut s).is_empty());
        // fish handles paste bracketing itself.
        let mut f = Screen::new(40, 10);
        f.feed(b"\x1b]1337;ShellIntegrationVersion=5;shell=fish\x07\x1b]1337;RemoteHost=u@a\x07\x1b[?2004h\x1b]1337;RemoteHost=u@b\x07");
        assert!(changes(&mut f).is_empty());
    }

    /// Fix r1: the key mode and title the stack saves are the ones in effect AT the host report,
    /// so a remote that writes a mode and a title in the same chunk, after its first report,
    /// cannot choose what the local shell gets back.
    #[test]
    fn the_saved_mode_and_title_are_the_ones_at_the_report() {
        use crate::keyproto::KeyMode;
        static NAMES: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
        let names: &'static [String] = NAMES.get_or_init(|| vec!["pc".to_string()]);
        let saved = |s: &mut Screen| {
            s.take_requests().into_iter().find_map(|r| match r {
                Request::HostMoved { mode, title, .. } => Some((mode, title)),
                _ => None,
            })
        };
        let mut s = Screen::new(40, 10);
        s.host_policy.local_names = names.into();
        s.feed(b"\x1b]2;local\x07\x1b]1337;RemoteHost=me@pc\x07");
        s.take_requests();
        s.feed(b"\x1b]1337;RemoteHost=u@box\x07\x1b[>4;2m\x1b]2;evil\x07");
        assert_eq!(s.keyboard().mode, KeyMode::Mok2, "the remote's mode is on after the chunk");
        assert_eq!(saved(&mut s), Some((KeyMode::Standard, Some("local".to_string()))), "the mode and title at the report");
        // A mode set before the report in the same chunk is the one in effect at the report.
        s.feed(b"\x1b[>4;0m\x1b]2;box\x07\x1b[>4;1m\x1b]1337;RemoteHost=me@pc\x07\x1b[>4;2m\x1b]2;after\x07");
        assert_eq!(saved(&mut s), Some((KeyMode::Mok1, Some("box".to_string()))));
    }

    /// A host change reaches the host stack (m14e Task 4): locality is decided when the host is
    /// reported and kept, a user-only report copies it, OSC 7 reports a host too.
    #[test]
    fn host_changes_are_requested() {
        use crate::host_state::Remote;
        static NAMES: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
        let names: &'static [String] = NAMES.get_or_init(|| vec!["pc".to_string()]);
        let moved = |s: &mut Screen| s.take_requests().into_iter().filter(|r| matches!(r, Request::HostMoved { .. })).collect::<Vec<_>>();
        let rem = |u: &str, h: &str, local| Remote { user: Some(u.into()), host: h.into(), local };
        let mut s = Screen::new(40, 10);
        s.host_policy.local_names = names.into();
        s.feed(b"\x1b]1337;RemoteHost=me@pc\x07");
        assert!(moved(&mut s).is_empty(), "the first report");
        s.feed(b"\x1b]1337;RemoteHost=u@box\x07");
        assert_eq!(moved(&mut s), vec![Request::HostMoved { previous: rem("me", "pc", true), current: rem("u", "box", false), mode: crate::keyproto::KeyMode::Standard, title: None }]);
        // A user-only change queues none and copies the locality, even if the answer changed.
        static BOTH: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
        s.host_policy.local_names = BOTH.get_or_init(|| vec!["pc".to_string(), "box".to_string()]).as_slice().into();
        s.feed(b"\x1b]1337;RemoteHost=root@box\x07");
        assert!(moved(&mut s).is_empty(), "user-only");
        assert_eq!(s.shell.host_local, Some(false), "copied, not recomputed");
        // OSC 7 reports a host too; locality is as the seam answers when it is reported.
        s.feed(b"\x1b]7;file://pc/tmp\x07");
        assert_eq!(moved(&mut s), vec![Request::HostMoved { previous: rem("root", "box", false), current: Remote { user: None, host: "pc".into(), local: true }, mode: crate::keyproto::KeyMode::Standard, title: None }]);
        // Once reported, a later change of the answer changes nothing.
        s.host_policy.local_names = Default::default();
        s.feed(b"\x1b]1337;CurrentDir=/x\x07");
        assert_eq!(s.shell.host_local, Some(true));
        s.feed(b"\x1b]1337;RemoteHost=root@pc\x07");
        assert!(moved(&mut s).is_empty());
        assert_eq!(s.shell.host_local, Some(true));
    }

    /// Every host report that changes the account (user or host name), the first included,
    /// reaches the app, which removes a waiting outdated scripts notice (`PS:17909`,
    /// `VT100ScreenMutableState.m:4123-4137`); offers come only with a host change.
    #[test]
    fn every_host_change_reaches_the_app() {
        let changes = |s: &mut Screen| s.take_requests().into_iter().filter(|r| matches!(r, Request::HostChanged(_))).collect::<Vec<_>>();
        let none = vec![Request::HostChanged(LeftOn::default())];
        let mut s = Screen::new(40, 10);
        s.feed(b"\x1b]1337;RemoteHost=u@a\x07");
        assert_eq!(changes(&mut s), none, "the first host");
        s.feed(b"\x1b]1337;RemoteHost=u@a\x07\x1b]1337;CurrentDir=/tmp\x07");
        assert!(changes(&mut s).is_empty(), "the same account");
        s.feed(b"\x1b]1337;RemoteHost=v@a\x07");
        assert_eq!(changes(&mut s), none, "another user");
        s.feed(b"\x1b]7;file://w@b/tmp\x07");
        assert_eq!(changes(&mut s), none, "OSC 7");
    }

    /// The modes are judged where the host was reported, as iTerm2 stops reading there
    /// (`VT100ScreenMutableState.m:4133`): the new shell turning paste bracketing on in the
    /// same read is its own, never offered and never turned off.
    #[test]
    fn host_change_judges_modes_where_reported() {
        let paste_on = |s: &Screen| s.term().mode().contains(alacritty_terminal::term::TermMode::BRACKETED_PASTE);
        let changes = |s: &mut Screen| s.take_requests().into_iter().filter(|r| matches!(r, Request::HostChanged(l) if *l != LeftOn::default())).collect::<Vec<_>>();
        for saved in [None, Some(true), Some(false)] {
            let mut s = Screen::new(40, 10);
            s.host_policy.paste = saved;
            s.feed(b"\x1b]1337;RemoteHost=u@a\x07\x1b]1337;RemoteHost=u@b\x07\x1b[?2004h");
            assert!(changes(&mut s).is_empty(), "{saved:?}");
            assert!(paste_on(&s), "{saved:?}");
        }
        // Left on by the previous host: Always turns it off before the new shell's own
        // output, which then turns it on again; Never leaves it; unset offers.
        let mut s = Screen::new(40, 10);
        s.host_policy.paste = Some(true);
        s.feed(b"\x1b]1337;RemoteHost=u@a\x07\x1b[?2004h");
        s.feed(b"\x1b]1337;RemoteHost=u@b\x07");
        assert!(!paste_on(&s), "Always turns it off");
        s.feed(b"\x1b[?2004h\x1b]1337;RemoteHost=u@c\x07\x1b[?2004h");
        assert!(changes(&mut s).is_empty());
        assert!(paste_on(&s), "the new shell's own paste bracketing stays");
        let mut s = Screen::new(40, 10);
        s.host_policy.paste = Some(false);
        s.feed(b"\x1b]1337;RemoteHost=u@a\x07\x1b[?2004h\x1b]1337;RemoteHost=u@b\x07");
        assert!(changes(&mut s).is_empty());
        assert!(paste_on(&s), "Never leaves it");
    }

    /// 13o Task 2: a prompt mark resets the colors (iTerm2's Reset colors at shell prompt).
    #[test]
    fn prompt_resets_sgr() {
        use alacritty_terminal::index::{Column, Line};
        use alacritty_terminal::vte::ansi::{Color, NamedColor};
        let fg = |s: &Screen| s.term().grid()[Line(0)][Column(0)].fg;
        let mut s = Screen::new(40, 4);
        s.feed(b"\x1b[31m\x1b]133;A\x07p");
        assert_eq!(fg(&s), Color::Named(NamedColor::Foreground));
        let mut s = Screen::new(40, 4);
        s.set_rules(crate::emu::Rules { reset_sgr_on_prompt: false, ..Default::default() });
        s.feed(b"\x1b[31m\x1b]133;A\x07p");
        assert_eq!(fg(&s), Color::Named(NamedColor::Red));
        // Only prompts that start a prompt reset: a right, secondary or continuation prompt
        // keeps the color, and so does a prompt redrawn in place while the command is typed.
        for kind in ["r", "s", "c"] {
            let mut s = Screen::new(40, 4);
            s.feed(format!("\x1b[31m\x1b]133;A;k={kind}\x07p").as_bytes());
            assert_eq!(fg(&s), Color::Named(NamedColor::Red), "k={kind}");
        }
        let mut s = Screen::new(40, 4);
        s.feed(b"\x1b]133;A\x07\x1b]133;B\x07");
        s.feed(b"\x1b[1;1H\x1b[31m\x1b]133;A\x07p");
        assert_eq!(fg(&s), Color::Named(NamedColor::Red), "a prompt redrawn at its own start");
        let mut s = Screen::new(40, 4);
        s.feed(b"\x1b[31m\x1b]133;A;k=i\x07p");
        assert_eq!(fg(&s), Color::Named(NamedColor::Foreground));
    }

    #[test]
    fn mark_color_follows_the_key() {
        for yellow in [true, false] {
            assert_eq!(color_for_code(None, yellow), MarkColor::Success);
            assert_eq!(color_for_code(Some(0), yellow), MarkColor::Success);
            assert_eq!(color_for_code(Some(127), yellow), MarkColor::Error);
            assert_eq!(color_for_code(Some(161), yellow), MarkColor::Error);
        }
        for code in [128, 143, 160] {
            assert_eq!(color_for_code(Some(code), true), MarkColor::Other, "{code}");
            assert_eq!(color_for_code(Some(code), false), MarkColor::Error, "{code}");
        }
        let mut m = Mark::new(0, Kind::Prompt);
        m.code = Some(143);
        assert_eq!((color(&m, true), color(&m, false)), (MarkColor::Other, MarkColor::Error));
        assert_eq!(crate::scrollbar::series_of_mark(Some(143), true), crate::scrollbar::Series::Other);
        assert_eq!(crate::scrollbar::series_of_mark(Some(143), false), crate::scrollbar::Series::Error);
    }
}
