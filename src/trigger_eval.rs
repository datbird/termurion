//! Runs a screen's triggers over its output: each logical line when it ends, and the line
//! being written for Instant triggers; applies grid actions and asks the window for others.

use std::time::{Duration, Instant};

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::TermMode;

use crate::screen::{Request, Screen};
use crate::triggers::{self, Trigger};

/// The profile's trigger switches.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TriggerPrefs {
    /// "Enable triggers in interactive apps".
    pub interactive: bool,
    /// "Use interpolated strings for parameters".
    pub interpolated: bool,
}

/// A trigger action the window runs.
#[derive(Clone, Debug, PartialEq)]
pub enum TriggerRequest {
    Alert { trigger: usize, text: String },
    Notify(String),
    Bell,
    Bounce { until_active: bool },
    SendText(String),
    RunCommand(String),
    SetTitle(String),
    TabStatus { status: String, dot: String, text: String },
    BufferInput(bool),
    ProfileSetting { key: String, on: bool },
    Coprocess { cmd: String, silent: bool },
    Unavailable { trigger: usize, title: String, milestone: u8 },
}

/// A trigger with its compiled pattern (or the pattern's error).
#[derive(Clone, Debug)]
pub struct Compiled {
    pub trigger: Trigger,
    re: Option<fancy_regex::Regex>,
    pub error: Option<String>,
    /// The line an Instant trigger last fired on.
    last_line: Option<i64>,
}

#[derive(Clone, Debug, Default)]
pub struct TriggerState {
    pub list: Vec<Compiled>,
    pub prefs: TriggerPrefs,
    /// The next row number to check (rows before it were checked or skipped).
    next: i64,
    /// The alternate screen was up at the last check.
    alt: bool,
    last_instant: Option<Instant>,
    /// Inside an action (Inject Data): output does not run triggers again.
    busy: bool,
    pub lines_checked: u64,
}

/// A logical line's text and, for each character, its byte offset and cell.
struct LineText {
    text: String,
    cells: Vec<(usize, i32, usize)>,
}

/// How the matches of one trigger go on after an action.
enum Flow {
    More,
    Done,
    StopAll,
}

/// iTerm2's triggerRadius: rows before and after a row a logical line may span.
const RADIUS: i32 = 3;
const INSTANT_EVERY: Duration = Duration::from_millis(500);

impl Screen {
    pub fn set_triggers(&mut self, list: Vec<Trigger>, prefs: TriggerPrefs) {
        self.triggers.list = list
            .into_iter()
            .map(|trigger| {
                let (re, error) = match fancy_regex::Regex::new(&trigger.regex) {
                    Ok(re) => (Some(re), None),
                    Err(e) => (None, Some(e.to_string())),
                };
                Compiled { trigger, re, error, last_line: None }
            })
            .collect();
        self.triggers.prefs = prefs;
    }

    /// Patterns that do not compile: (index, error).
    pub fn trigger_errors(&self) -> Vec<(usize, String)> {
        self.triggers.list.iter().enumerate().filter_map(|(i, c)| c.error.clone().map(|e| (i, e))).collect()
    }

    fn row_number(&self, line: i32) -> i64 {
        self.term.scrolled_off as i64 + line as i64
    }

    fn cursor_row(&self) -> i64 {
        self.row_number(self.term.grid().cursor.point.line.0)
    }

    /// Rows moved (a resize, a fold, a porthole): checking goes on from the cursor.
    pub(crate) fn triggers_from_cursor(&mut self) {
        self.triggers.next = self.cursor_row();
    }

    fn triggers_active(&self) -> bool {
        !self.triggers.list.is_empty() && !self.triggers.busy && (!self.term.mode().contains(TermMode::ALT_SCREEN) || self.triggers.prefs.interactive)
    }

    /// After output: each logical line that ended since the last check, once.
    pub(crate) fn run_triggers_after_feed(&mut self) {
        let cursor = self.cursor_row();
        let alt = self.term.mode().contains(TermMode::ALT_SCREEN);
        if alt != self.triggers.alt || !self.triggers_active() {
            self.triggers.alt = alt;
            if !self.triggers.busy {
                self.triggers.next = cursor;
            }
            return;
        }
        let g = self.term.grid();
        let (history, lines, cols) = (g.history_size() as i64, g.screen_lines() as i32, g.columns());
        let first = self.triggers.next.max(self.term.scrolled_off as i64 - history);
        self.triggers.next = cursor;
        for row in first..cursor {
            let line = (row - self.term.scrolled_off as i64) as i32;
            if line >= lines {
                break;
            }
            let wraps = self.term.grid()[Line(line)][Column(cols - 1)].flags.contains(Flags::WRAPLINE);
            if !wraps {
                self.evaluate_line(line, false);
            }
        }
    }

    /// Instant triggers on the line being written, at most every 0.5 s.
    pub fn check_instant_triggers(&mut self, now: Instant) {
        if !self.triggers_active() || !self.triggers.list.iter().any(|c| c.trigger.instant && c.trigger.enabled) {
            return;
        }
        if self.triggers.last_instant.is_some_and(|t| now.saturating_duration_since(t) < INSTANT_EVERY) {
            return;
        }
        self.triggers.last_instant = Some(now);
        let line = self.term.grid().cursor.point.line.0;
        self.evaluate_line(line, true);
    }

    /// The logical line ending on row `end`: up to RADIUS rows it wraps from.
    fn logical_line(&self, end: i32) -> LineText {
        let g = self.term.grid();
        let (top, cols) = (-(g.history_size() as i32), g.columns());
        let mut start = end;
        while start > top && end - start < RADIUS && g[Line(start - 1)][Column(cols - 1)].flags.contains(Flags::WRAPLINE) {
            start -= 1;
        }
        let mut text = String::new();
        let mut cells = Vec::new();
        for l in start..=end {
            let row = &g[Line(l)];
            // A row that does not wrap ends at its last written cell.
            let wraps = row[Column(cols - 1)].flags.contains(Flags::WRAPLINE);
            let used = if wraps || l < end { cols } else { (0..cols).rev().find(|c| row[Column(*c)].c != ' ' || row[Column(*c)].flags.contains(Flags::WIDE_CHAR)).map_or(0, |c| c + 1) };
            for c in 0..used {
                let cell = &row[Column(c)];
                if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                    continue;
                }
                cells.push((text.len(), l, c));
                text.push(if cell.c == '\0' { ' ' } else { cell.c });
            }
        }
        LineText { text, cells }
    }

    /// A variable for interpolated parameters.
    fn trigger_var(&self, name: &str, row: i64) -> Option<String> {
        match name {
            "session.hostname" => self.shell.host.clone(),
            "session.username" => self.shell.user.clone(),
            "session.path" => self.shell.cwd.clone(),
            "line" => Some(row.to_string()),
            n => n.strip_prefix("user.").and_then(|u| self.shell.vars.get(u).cloned()).or_else(|| self.app_vars.get(n).cloned()),
        }
    }

    fn evaluate_line(&mut self, end: i32, instant: bool) {
        let line = self.logical_line(end);
        let row = self.row_number(end);
        if !instant {
            self.triggers.lines_checked += 1;
        }
        for i in 0..self.triggers.list.len() {
            let c = &self.triggers.list[i];
            let t = &c.trigger;
            // Jobs are known from milestone 6c; event triggers come with 6b.
            if !t.enabled || c.re.is_none() || t.match_type != 0 || !t.job.is_empty() {
                continue;
            }
            let multi = triggers::action_info(&t.action).is_some_and(|a| a.multi_fire);
            if instant && (!t.instant || (c.last_line == Some(row) && !multi)) {
                continue;
            }
            if !instant && t.instant && c.last_line == Some(row) {
                continue;
            }
            let re = c.re.clone().expect("checked above");
            let param = t.parameter.clone();
            let mut fired = false;
            let mut stop = false;
            for caps in re.captures_iter(&line.text) {
                let Ok(caps) = caps else { break };
                let Some(m) = caps.get(0).filter(|m| m.start() < m.end()) else { continue };
                let groups: Vec<Option<&str>> = (0..caps.len()).map(|g| caps.get(g).map(|m| m.as_str())).collect();
                let value = if self.triggers.prefs.interpolated {
                    match crate::interp::evaluate(&param, &|n| self.trigger_var(n, row), &groups) {
                        Ok(v) => v,
                        Err(e) => {
                            self.triggers.list[i].error = Some(e);
                            break;
                        }
                    }
                } else {
                    triggers::substitute(&param, &groups)
                };
                fired = true;
                match self.trigger_action(i, &value, (m.start(), m.end()), &line) {
                    Flow::More => {}
                    Flow::Done => break,
                    Flow::StopAll => {
                        stop = true;
                        break;
                    }
                }
            }
            if fired && instant {
                self.triggers.list[i].last_line = Some(row);
            }
            if stop {
                break;
            }
        }
    }

    /// Runs trigger `i`'s action on a match (byte range in the line).
    fn trigger_action(&mut self, i: usize, value: &str, _span: (usize, usize), _line: &LineText) -> Flow {
        let class = self.triggers.list[i].trigger.action.clone();
        let class = triggers::action_info(&class).map_or(class.as_str(), |a| a.class).to_string();
        match class.as_str() {
            "iTermSetUserVariableTrigger" => {
                if let Some((name, v)) = value.split_once('\u{1}') {
                    let name = name.strip_prefix("user.").unwrap_or(name);
                    if !name.is_empty() && !name.contains('.') {
                        self.shell.vars.insert(name.to_string(), v.to_string());
                    }
                }
                Flow::Done
            }
            "StopTrigger" => Flow::StopAll,
            "AlertTrigger" => {
                self.requests.push(Request::Trigger(TriggerRequest::Alert { trigger: i, text: value.to_string() }));
                Flow::Done
            }
            _ => Flow::Done,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screen::{Request, Screen};
    use crate::triggers::Trigger;

    fn trig(regex: &str, action: &str, parameter: &str, instant: bool) -> Trigger {
        Trigger { regex: regex.into(), action: action.into(), parameter: parameter.into(), instant, enabled: true, name: String::new(), job: String::new(), match_type: 0, extra: toml::Table::new() }
    }

    fn alerts(s: &mut Screen) -> usize {
        s.take_requests().into_iter().filter(|r| matches!(r, Request::Trigger(TriggerRequest::Alert { .. }))).count()
    }

    #[test]
    fn each_line_is_checked_once() {
        let mut s = Screen::with_scrollback(40, 5, 10000);
        s.set_triggers(vec![trig(r"^line (\d+)$", "iTermSetUserVariableTrigger", "last\u{1}\\1", false)], TriggerPrefs::default());
        let text: String = (0..3000).map(|i| format!("line {i}\r\n")).collect();
        s.feed(text.as_bytes());
        assert_eq!(s.shell.vars.get("last").map(String::as_str), Some("2999"));
        assert_eq!(s.triggers.lines_checked, 3000);
        s.feed(b"line x\r\nline y\r\n");
        assert_eq!(s.triggers.lines_checked, 3002, "no line twice");
    }

    #[test]
    fn wrapped_lines_are_one_logical_line() {
        let mut s = Screen::new(10, 5);
        s.set_triggers(vec![trig("jK", "AlertTrigger", "x", false)], TriggerPrefs::default());
        s.feed(b"abcdefghijKLM\r\n");
        assert_eq!(alerts(&mut s), 1);
        assert_eq!(s.triggers.lines_checked, 1, "two rows, one line");
    }

    #[test]
    fn instant_triggers_fire_once_per_line() {
        let mut s = Screen::new(40, 5);
        s.set_triggers(vec![trig("foo", "AlertTrigger", "x", true)], TriggerPrefs::default());
        let t0 = std::time::Instant::now();
        s.feed(b"foo");
        assert_eq!(alerts(&mut s), 0, "not yet: no line ended");
        s.check_instant_triggers(t0);
        assert_eq!(alerts(&mut s), 1);
        s.check_instant_triggers(t0 + std::time::Duration::from_secs(1));
        assert_eq!(alerts(&mut s), 0, "once per line");
        s.feed(b" more\r\n");
        assert_eq!(alerts(&mut s), 0, "the line's end does not fire it again");
        s.feed(b"foo");
        s.check_instant_triggers(t0 + std::time::Duration::from_secs(2));
        assert_eq!(alerts(&mut s), 1, "a new line");
        s.check_instant_triggers(t0 + std::time::Duration::from_millis(2100));
        assert_eq!(alerts(&mut s), 0);
    }

    #[test]
    fn empty_matches_do_nothing() {
        let mut s = Screen::new(40, 5);
        s.set_triggers(vec![trig("x*", "AlertTrigger", "x", false)], TriggerPrefs::default());
        s.feed(b"abc\r\n");
        assert_eq!(alerts(&mut s), 0);
        s.set_triggers(vec![trig(".", "AlertTrigger", "x", false)], TriggerPrefs::default());
        s.feed(b"abc\r\n");
        assert_eq!(alerts(&mut s), 1, "one alert per line, not per match");
    }

    #[test]
    fn lines_gone_before_the_check() {
        let mut s = Screen::with_scrollback(40, 3, 2);
        s.set_triggers(vec![trig("line", "HighlightTrigger", "{#ff0000,}", false)], TriggerPrefs::default());
        let text: String = (0..50).map(|i| format!("line {i}\r\n")).collect();
        s.feed(text.as_bytes());
        assert!(s.triggers.lines_checked > 0 && s.triggers.lines_checked <= 5, "{}", s.triggers.lines_checked);
    }

    #[test]
    fn alt_screen_needs_the_profile_switch() {
        let mut s = Screen::new(40, 5);
        s.set_triggers(vec![trig("foo", "AlertTrigger", "x", false)], TriggerPrefs::default());
        s.feed(b"\x1b[?1049hfoo\r\n");
        assert_eq!(alerts(&mut s), 0);
        s.set_triggers(vec![trig("foo", "AlertTrigger", "x", false)], TriggerPrefs { interactive: true, interpolated: false });
        s.feed(b"foo\r\n");
        assert_eq!(alerts(&mut s), 1);
    }

    #[test]
    fn job_triggers_wait_for_6c() {
        let mut s = Screen::new(40, 5);
        let mut t = trig("foo", "AlertTrigger", "x", false);
        t.job = "vim".into();
        s.set_triggers(vec![t], TriggerPrefs::default());
        s.feed(b"foo\r\n");
        assert_eq!(alerts(&mut s), 0);
    }

    #[test]
    fn interpolated_parameters() {
        let mut s = Screen::new(40, 5);
        s.set_triggers(vec![trig("id=(\\w+)", "AlertTrigger", "got \\(matches[1]) on \\(session.name)", false)], TriggerPrefs { interactive: false, interpolated: true });
        s.app_vars.insert("session.name".into(), "Main".into());
        s.feed(b"id=abc\r\n");
        let texts: Vec<String> = s.take_requests().into_iter().filter_map(|r| match r {
            Request::Trigger(TriggerRequest::Alert { text, .. }) => Some(text),
            _ => None,
        }).collect();
        assert_eq!(texts, vec!["got abc on Main"]);
    }
}
