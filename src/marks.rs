//! Marks and shell integration: prompt marks from OSC 133, manual and named marks, the
//! command text, exit code, and times of each command, and the location and variables
//! the shell reports (OSC 7, OSC 1337). A mark lives on its row (the alacritty patch);
//! the records here hold what iTerm2's VT100ScreenMark holds.

use std::collections::BTreeMap;
use std::time::SystemTime;

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Point};

use crate::osc::{Osc, PromptKind};
use crate::screen::Screen;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Made by a shell prompt (OSC 133;A, or OSC 7 from a shell without a machine id).
    Prompt,
    /// Made by Set Mark, Set Named Mark, or OSC 1337;SetMark.
    Manual,
}

/// A place on the grid, relative to a marked row, so it moves with that row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Anchor {
    pub mark: u32,
    pub dy: i32,
    pub col: usize,
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
    pub history: Vec<CommandRecord>,
}

#[derive(Default)]
pub struct Marks {
    pub records: BTreeMap<u32, Mark>,
    next_id: u32,
    /// The prompt being typed at, or whose command runs.
    current: Option<u32>,
    /// The last mark with a command.
    last_command: Option<u32>,
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

impl Screen {
    fn cursor_point(&self) -> Point {
        self.term.grid().cursor.point
    }

    fn mark_line(&self, id: u32) -> Option<i32> {
        self.row_marks().into_iter().find(|(_, m)| *m == id).map(|(l, _)| l)
    }

    fn anchor_here(&self, mark: u32) -> Option<Anchor> {
        let line = self.mark_line(mark)?;
        let p = self.cursor_point();
        Some(Anchor { mark, dy: p.line.0 - line, col: p.column.0 })
    }

    pub fn resolve(&self, a: Anchor) -> Option<Point> {
        Some(Point::new(Line(self.mark_line(a.mark)? + a.dy), Column(a.col)))
    }

    /// The cell before `p` (range ends are inclusive).
    fn before(&self, p: Point) -> Point {
        if p.column.0 > 0 { Point::new(p.line, Column(p.column.0 - 1)) } else { Point::new(p.line - 1, Column(self.term.columns() - 1)) }
    }

    fn new_mark(&mut self, line: i32, kind: Kind) -> u32 {
        self.marks.next_id = self.marks.next_id.wrapping_add(1).max(1);
        let id = self.marks.next_id;
        self.set_row_mark(line, id);
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

    fn prompt_start(&mut self, kind: PromptKind, aid: Option<String>, fresh_line: bool) {
        if !matches!(kind, PromptKind::Initial | PromptKind::Unknown) {
            return;
        }
        if fresh_line && self.place_prompt_first_column && self.cursor_point().column.0 > 0 {
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
    }

    /// Acts on one shell-integration OSC at the current cursor position.
    pub(crate) fn apply(&mut self, osc: Osc) {
        match osc {
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
                self.marks.output_start = self.mark_line(cur).map(|l| {
                    let (line, col) = if p.column.0 > 0 { (p.line.0 + 1, 0) } else { (p.line.0, 0) };
                    Anchor { mark: cur, dy: line - l, col }
                });
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
            }
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
                        self.shell.vars.insert(name.to_string(), String::from_utf8_lossy(&bytes).into_owned());
                    }
                }
                "ShellIntegrationVersion" => {
                    let mut parts = value.split(';');
                    self.shell.version = parts.next().and_then(|v| v.trim().parse().ok());
                    if let Some(sh) = parts.find_map(|p| p.strip_prefix("shell=")) {
                        self.shell.shell = Some(sh.to_string());
                    }
                }
                "SetMark" => {
                    let line = self.cursor_point().line.0;
                    self.new_mark(line, Kind::Manual);
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
    fn colors_follow_the_exit_code() {
        let m = |code| Mark { code, ..Mark::new(1, Kind::Prompt) };
        assert_eq!(color(&m(None)), MarkColor::Success);
        assert_eq!(color(&m(Some(0))), MarkColor::Success);
        assert_eq!(color(&m(Some(130))), MarkColor::Other);
        assert_eq!(color(&m(Some(1))), MarkColor::Error);
    }
}
