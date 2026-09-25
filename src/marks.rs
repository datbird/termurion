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
}

impl Mark {
    pub fn new(id: u32, kind: Kind) -> Mark {
        Mark { id, kind, command: String::new(), code: None, start: None, end: None, aid: None, name: None, command_range: None }
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
                for a in [&mut self.marks.command_start, &mut self.marks.output_start].into_iter().flatten() {
                    a.lines = a.lines.saturating_sub(skipped);
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
    }

    /// The mark line of the prompt being typed at, if any.
    pub fn current_prompt_line(&self) -> Option<i32> {
        self.mark_line(self.marks.current?)
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
        id
    }

    /// A manual mark (Set Mark, Set Named Mark) on a grid line.
    pub fn set_manual_mark(&mut self, line: i32, name: Option<String>) -> u32 {
        let id = self.new_mark(line, Kind::Manual);
        self.marks.records.get_mut(&id).unwrap().name = name;
        id
    }

    /// True once after a prompt mark or a SetMark was made (for mark alerts).
    pub fn take_new_mark(&mut self) -> bool {
        std::mem::take(&mut self.marks.made)
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

    pub(crate) fn prompt_start(&mut self, kind: PromptKind, aid: Option<String>, fresh_line: bool) {
        if !matches!(kind, PromptKind::Initial | PromptKind::Unknown) {
            return;
        }
        // A new prompt ends whatever the last program said it was doing (OSC 21337).
        self.tab_status = crate::tabstatus::Status::default();
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
                self.tab_status.apply(&pairs);
            }
            Osc::Clipboard(payload) => match crate::clipboard_osc::osc52(&payload) {
                Some(crate::clipboard_osc::Osc52::Write(t)) => self.requests.push(crate::screen::Request::ClipboardWrite(t)),
                Some(crate::clipboard_osc::Osc52::Read(pc)) => self.requests.push(crate::screen::Request::ClipboardRead(pc)),
                None => {}
            },
            Osc::Prompt { kind, aid, fresh_line } => self.prompt_start(kind, aid, fresh_line),
            Osc::CommandStart => {
                self.marks.in_command = true;
                self.marks.command_start = self.marks.current.and_then(|c| self.anchor_here(c));
            }
            Osc::OutputStart => {
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
                if let Some(m) = self.marks.records.get_mut(&cur) {
                    m.command = text.clone();
                    m.start = Some(now);
                    m.command_range = start.zip(end);
                }
                self.marks.last_command = Some(cur);
                let (cwd, host) = (self.shell.cwd.clone(), self.shell.host.clone());
                self.shell.history.push(CommandRecord { command: text, cwd, host, start: now, end: None, code: None });
            }
            Osc::CommandEnd { code, aid } => {
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
                // Custom=id=<sender>:<payload> (trusted): the Custom Escape Sequence event.
                "Custom" if self.trusted => {
                    let (head, payload) = value.split_once(':').unwrap_or((value.as_str(), ""));
                    let args = crate::blocks::parse(head);
                    let id = crate::blocks::get(&args, "id").unwrap_or("").to_string();
                    let facts = crate::trigger_events::EventFacts { name: id.clone(), ..Default::default() };
                    self.fire_event(crate::trigger_events::EventKind::CustomEscapeSequence, &facts, &[id, payload.to_string()]);
                }
                "SetMark" => {
                    let line = self.cursor_point().line.0;
                    self.new_mark(line, Kind::Manual);
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
                    let version = if self.unicode.version == 0 { 9 } else { self.unicode.version };
                    let f = crate::osc1337::Features { clipboard_writable: self.clipboard_allowed, ambiguous_wide: false, unicode_version: version };
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
    fn colors_follow_the_exit_code() {
        let m = |code| Mark { code, ..Mark::new(1, Kind::Prompt) };
        assert_eq!(color(&m(None)), MarkColor::Success);
        assert_eq!(color(&m(Some(0))), MarkColor::Success);
        assert_eq!(color(&m(Some(130))), MarkColor::Other);
        assert_eq!(color(&m(Some(1))), MarkColor::Error);
    }
}
