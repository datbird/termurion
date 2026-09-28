//! Marks and shell integration: prompt marks from OSC 133, manual and named marks, the
//! command text, exit code, and times of each command, and the location and variables
//! the shell reports (OSC 7, OSC 1337). A mark lives on its row (the alacritty patch);
//! the records here hold what iTerm2's VT100ScreenMark holds.

use std::collections::BTreeMap;
use std::time::SystemTime;

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Point};
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
}

impl Mark {
    pub fn new(id: u32, kind: Kind) -> Mark {
        Mark { id, kind, command: String::new(), code: None, start: None, end: None, aid: None, name: None, command_range: None, captured: Vec::new(), clear_count: 0, prompt_end: None, output_start: None, directory: None }
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
pub fn color(m: &Mark) -> MarkColor {
    match m.code {
        None | Some(0) => MarkColor::Success,
        Some(128..=160) => MarkColor::Other,
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
}

/// Decodes %XX escapes (bytes, then UTF-8; Latin-1 when that fails, as iTerm2 does).
fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            let hex = |c: u8| (c as char).to_digit(16);
            if let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2])) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8(out.clone()).unwrap_or_else(|_| out.iter().map(|&c| c as char).collect())
}

/// A file:// URL: (user, host, path, has a machine id).
fn parse_location(url: &str) -> Option<(Option<String>, Option<String>, String, bool)> {
    let rest = url.strip_prefix("file://")?;
    let (rest, query) = rest.split_once('?').unwrap_or((rest, ""));
    let slash = rest.find('/')?;
    let (authority, path) = rest.split_at(slash);
    let (user, host) = match authority.rsplit_once('@') {
        Some((u, h)) => (Some(percent_decode(u)), h),
        None => (None, authority),
    };
    let host = (!host.is_empty()).then(|| host.to_string());
    let machine = query.split('&').any(|q| q.starts_with("machineID="));
    Some((user, host, percent_decode(path), machine))
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
/// machine (`local`, compared ignoring case as Windows names are).
pub fn host_changed(previous: Option<&str>, new: &str, local: &str) -> bool {
    let is_local = |h: &str| !local.is_empty() && h.eq_ignore_ascii_case(local);
    previous.is_some_and(|p| p != new && !(is_local(p) && is_local(new)))
}

impl Screen {
    /// Clears from grid line `line` to the end (iTerm2 clearFromAbsoluteLineToEnd). The
    /// cursor's row (the prompt) goes to the top of the screen, and the rows kept above move
    /// into history, so the screen matches ConPTY after its own clear (which keeps only the
    /// cursor row). The current prompt's mark moves onto that row when its own row went.
    pub fn clear_from(&mut self, line: i32) {
        let grid = self.term.grid();
        let top = -(grid.history_size() as i32);
        let line = line.max(top);
        let cursor = grid.cursor.point;
        let mut saved = grid[cursor.line].clone();
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

    fn mark_line(&self, id: u32) -> Option<i32> {
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
        let grid = self.term.primary_grid();
        let (cols, bottom) = (grid.columns(), grid.screen_lines() as i32);
        let mut r = self.mark_line(a.mark)?;
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
        self.marks.next_id = self.marks.next_id.wrapping_add(1).max(1);
        let id = self.marks.next_id;
        match kind {
            Kind::Prompt => self.set_row_mark(line, id),
            Kind::Manual => self.set_row_manual(line, id),
        }
        self.marks.records.insert(id, Mark::new(id, kind));
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
    }

    /// Every mark still in the buffer, top first. Records whose row is gone are dropped.
    pub fn marks_in_order(&mut self) -> Vec<(i32, Mark)> {
        let rows = self.row_marks();
        let live: std::collections::HashSet<u32> = rows.iter().map(|(_, id)| *id).collect();
        self.marks.records.retain(|id, _| live.contains(id));
        rows.into_iter().filter_map(|(l, id)| Some((l, self.marks.records.get(&id)?.clone()))).collect()
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

    /// Acts on one shell-integration OSC at the current cursor position. A full-screen app
    /// on the alternate screen makes no marks; its cursor is not on the main screen.
    pub(crate) fn apply(&mut self, osc: Osc) {
        use crate::trigger_events::{EventFacts, EventKind};
        let before = (self.shell.cwd.clone(), self.shell.host.clone(), self.shell.user.clone());
        self.apply_osc(osc);
        // Directory, host, and user changes are events for triggers.
        let after = (self.shell.cwd.clone(), self.shell.host.clone(), self.shell.user.clone());
        // Modes a program on the previous host left on may be turned off (`PS:17929-17940`).
        if let Some(new) = after.1.as_deref().filter(|_| before.1 != after.1) {
            if host_changed(before.1.as_deref(), new, &std::env::var("COMPUTERNAME").unwrap_or_default()) {
                self.requests.push(Request::HostChanged { fish: self.shell.shell.as_deref() == Some("fish") });
            }
        }
        for (kind, old, new) in [(EventKind::DirectoryChanged, before.0, after.0), (EventKind::HostChanged, before.1, after.1), (EventKind::UserChanged, before.2, after.2)] {
            if let Some(v) = new.filter(|n| Some(n) != old.as_ref()) {
                self.fire_event(kind, &EventFacts { value: v.clone(), ..Default::default() }, &[v]);
            }
        }
    }

    fn apply_osc(&mut self, osc: Osc) {
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
            Osc::Sync(_) => {}
            Osc::TabStatus(payload) => {
                let pairs = crate::tabstatus::parse(&payload);
                self.apply_tab_status(&pairs);
            }
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
                if counted && !(redraw && self.marks.prompt_range.0 == at) {
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
                if let Some(m) = self.marks.current.and_then(|c| self.marks.records.get_mut(&c)) {
                    m.prompt_end = end;
                    m.directory = cwd;
                }
                self.composer_from = Some(self.abs_cursor());
                // The prompt is captured unless its range did not change (`MS:4749-4777`).
                let range = crate::prompt_state::prompt_range(self.marks.prompt_range.0, self.abs_cursor());
                if range != self.marks.prompt_range {
                    self.marks.prompt_range = range;
                    self.marks.prompt_anchors.1 = self.marks.command_start;
                    let (rows, text) = self.prompt_rows();
                    let events = self.prompt_state.captured(rows, text);
                    self.composer_events.extend(events);
                }
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
                self.fire_event(crate::trigger_events::EventKind::CommandFinished, &crate::trigger_events::EventFacts { exit_code: Some(c), ..Default::default() }, &[c.to_string()]);
            }
            Osc::Notify(text) => {
                self.requests.push(crate::screen::Request::Notify { title: None, message: text.clone() });
                self.fire_event(crate::trigger_events::EventKind::NotificationPosted, &crate::trigger_events::EventFacts { value: text.clone(), ..Default::default() }, &[text]);
            }
            Osc::Progress(p) => {
                // 9;4;0 removes the bar; any other state shows it.
                let shown = p.split(';').next().is_some_and(|st| st.trim() != "0");
                if shown != self.progress_shown {
                    self.progress_shown = shown;
                    self.requests.push(crate::screen::Request::Progress(shown));
                    let word = if shown { "appeared" } else { "disappeared" };
                    self.fire_event(crate::trigger_events::EventKind::ProgressBarChanged, &crate::trigger_events::EventFacts { appeared: shown, ..Default::default() }, &[word.to_string()]);
                }
            }
            Osc::Sixel(data) => self.sixel(&data),
            Osc::Kitty(body) => self.kitty(&body),
            Osc::Location(url) => {
                let Some((user, host, path, machine)) = parse_location(&url) else { return };
                if host.is_some() && (user.is_some() || host != self.shell.host) {
                    self.shell.user = user;
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
                "CurrentDir" if !value.is_empty() => self.shell.cwd = Some(value),
                "RemoteHost" => {
                    let clean: String = value.chars().filter(|c| !c.is_control()).collect();
                    match clean.rsplit_once('@') {
                        Some((u, h)) => {
                            self.shell.user = Some(u.to_string());
                            self.shell.host = Some(h.to_string());
                        }
                        None => self.shell.host = Some(clean),
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
                    self.shell.version = parts.next().and_then(|v| v.trim().parse().ok());
                    if let Some(sh) = parts.find_map(|p| p.strip_prefix("shell=")) {
                        self.shell.shell = Some(sh.to_string());
                    }
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
                "UnicodeVersion" => self.unicode.apply(&value),
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
                            ("tab", _) if v == "default" => self.requests.push(Request::TabColor(None)),
                            ("tab", Some(crate::osc1337::Color::Rgb(c))) => self.requests.push(Request::TabColor(Some(c))),
                            ("tab", Some(crate::osc1337::Color::Index(i))) => {
                                let c = self.palette.indexed(i);
                                self.requests.push(Request::TabColor(Some(c)));
                            }
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
                    let f = crate::osc1337::session_features(self.clipboard_allowed, self.unicode.version, self.profile_unicode);
                    let reply = format!("\x1b]1337;Capabilities={}\x07", crate::osc1337::capabilities(f));
                    self.replies.extend(reply.into_bytes());
                }
                "ExecFailed" => self.exec_failed = true,
                "Env" if self.trusted => {
                    use base64::Engine;
                    if let Some(b) = value.strip_prefix("report=all:").and_then(|b| base64::engine::general_purpose::STANDARD.decode(b).ok()) {
                        self.env = Some(String::from_utf8_lossy(&b).into_owned());
                    }
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
        assert_eq!(color(&marks[0].1), MarkColor::Error);
        assert_eq!(s.shell.history[0].command, "ls -l");
        assert_eq!(s.shell.history[0].code, Some(2));
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
        assert_eq!(color(&m(None)), MarkColor::Success);
        assert_eq!(color(&m(Some(0))), MarkColor::Success);
        assert_eq!(color(&m(Some(130))), MarkColor::Other);
        assert_eq!(color(&m(Some(1))), MarkColor::Error);
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

    /// Host change offers: only when a previous host existed, the name changed and the two
    /// are not both this machine (`PS:17929-17940`); the fish flag rides along.
    #[test]
    fn host_change_follows_iterm2() {
        assert!(!host_changed(None, "b", "PC"), "no previous host");
        assert!(!host_changed(Some("a"), "a", "PC"));
        assert!(host_changed(Some("a"), "A", "PC"), "iTerm2 compares names exactly (isEqualToString:)");
        assert!(host_changed(Some("a"), "b", "PC"));
        assert!(!host_changed(Some("pc"), "PC", "PC"), "both this machine");
        assert!(host_changed(Some("PC"), "b", "PC"));
        let changes = |s: &mut Screen| s.take_requests().into_iter().filter(|r| matches!(r, Request::HostChanged { .. })).collect::<Vec<_>>();
        let mut s = Screen::new(40, 10);
        s.feed(b"\x1b]1337;RemoteHost=u@a\x07");
        assert!(changes(&mut s).is_empty(), "the first host is not a change");
        s.feed(b"\x1b]1337;RemoteHost=u@a\x07\x1b]1337;RemoteHost=v@a\x07");
        assert!(changes(&mut s).is_empty(), "same host, another user");
        s.feed(b"\x1b]1337;RemoteHost=u@b\x07");
        assert_eq!(changes(&mut s), vec![Request::HostChanged { fish: false }]);
        let mut f = Screen::new(40, 10);
        f.feed(b"\x1b]1337;ShellIntegrationVersion=5;shell=fish\x07\x1b]1337;RemoteHost=u@a\x07\x1b]1337;RemoteHost=u@b\x07");
        assert_eq!(changes(&mut f), vec![Request::HostChanged { fish: true }]);
    }
}
