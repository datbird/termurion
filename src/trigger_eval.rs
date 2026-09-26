//! Runs a screen's triggers over its output: each logical line when it ends, and the line
//! being written for Instant triggers; applies grid actions and asks the window for others.

use std::time::{Duration, Instant};

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::{Cell, Flags, Hyperlink};
use alacritty_terminal::term::TermMode;
use alacritty_terminal::vte::ansi::{Color, NamedColor, Rgb};

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
    /// A Capture Output trigger fired; the tool should refresh (or warn: no shell integration
    /// means no command mark to store it on).
    Captured { needs_shell_integration: bool },
    /// OSC 1337 ClearCapturedOutput: the Captured Output tool refreshes, with no warning.
    CapturedCleared,
    Unavailable { trigger: usize, title: String, milestone: u8 },
    /// An interpolated parameter that cannot be evaluated (shown once).
    ParameterError { trigger: usize, text: String },
}

/// A trigger with its compiled pattern (or the pattern's error).
#[derive(Clone, Debug)]
pub struct Compiled {
    pub trigger: Trigger,
    re: Option<fancy_regex::Regex>,
    pub error: Option<String>,
    /// The line an Instant trigger last fired on.
    last_line: Option<i64>,
    /// Show Alert's Disable This Alert: off for this session, not in the profile.
    pub off_here: bool,
    /// An event trigger's timer state.
    pub events: crate::trigger_events::EventState,
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
    pub(crate) busy: bool,
    /// Events nested inside an action's own event (see `fire_event`).
    pub(crate) event_depth: u8,
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

const fn c(r: u8, g: u8, b: u8) -> Rgb {
    Rgb { r, g, b }
}

/// AppKit's named colors in iTerm2's preset order: black, dark gray, light gray, white,
/// gray, red, green, blue, cyan, yellow, magenta, orange, purple, brown.
const NAMED: [Rgb; 14] = [
    c(0, 0, 0), c(85, 85, 85), c(170, 170, 170), c(255, 255, 255), c(128, 128, 128), c(255, 0, 0), c(0, 255, 0),
    c(0, 0, 255), c(0, 255, 255), c(255, 255, 0), c(255, 0, 255), c(255, 128, 0), c(128, 0, 128), c(153, 102, 51),
];

fn hex(v: &str) -> Option<Rgb> {
    let h = v.trim().strip_prefix('#').filter(|h| h.len() == 6 && h.is_ascii())?;
    let n = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok();
    Some(c(n(0)?, n(2)?, n(4)?))
}

/// A Highlight trigger's colors: `{#fg,#bg}` (either side may be empty), an old numeric
/// preset, or white on red when empty.
pub fn highlight_colors(param: &str) -> (Option<Rgb>, Option<Rgb>) {
    let p = param.trim();
    if let Some(body) = p.strip_prefix('{').and_then(|b| b.strip_suffix('}')) {
        let (f, b) = body.split_once(',').unwrap_or((body, ""));
        return (hex(f), hex(b));
    }
    let named = |i: i64| NAMED.get(i as usize).copied();
    match p.parse::<i64>() {
        Ok(n @ 0..=7) => {
            let (f, b) = [(9, 0), (0, 9), (3, 5), (5, 3), (0, 11), (11, 0), (0, 12), (12, 0)][n as usize];
            (named(f), named(b))
        }
        Ok(n @ 1000..=1013) => (named(n - 1000), None),
        Ok(n @ 2000..=2013) => (None, named(n - 2000)),
        _ => (Some(NAMED[3]), Some(NAMED[5])),
    }
}

/// Change Style's SGR parameters applied to a cell (`;` between parameters, `:` inside one).
fn apply_sgr(cell: &mut Cell, param: &str) {
    let params: Vec<Vec<u16>> = param.split(';').map(|p| p.split(':').map(|n| n.trim().parse().unwrap_or(0)).collect()).collect();
    let mut i = 0;
    let color = |sub: &[u16], rest: &[Vec<u16>], used: &mut usize| -> Option<Color> {
        // 38:5:n, 38:2:[cs:]r:g:b, or the ; forms 38;5;n and 38;2;r;g;b.
        let flat: Vec<u16> = if sub.len() > 1 { sub[1..].to_vec() } else { rest.iter().take(4).map(|p| p[0]).collect() };
        match flat.first() {
            Some(5) => {
                *used = if sub.len() > 1 { 0 } else { 2 };
                flat.get(1).map(|n| Color::Indexed(*n as u8))
            }
            Some(2) => {
                let rgb = if sub.len() > 1 && flat.len() >= 5 { &flat[2..5] } else if flat.len() >= 4 { &flat[1..4] } else { return None };
                *used = if sub.len() > 1 { 0 } else { 4 };
                Some(Color::Spec(c(rgb[0] as u8, rgb[1] as u8, rgb[2] as u8)))
            }
            _ => None,
        }
    };
    while i < params.len() {
        let p = &params[i];
        let mut used = 0;
        match p[0] {
            0 => {
                cell.flags.remove(Flags::BOLD | Flags::DIM | Flags::ITALIC | Flags::UNDERLINE | Flags::INVERSE | Flags::HIDDEN | Flags::STRIKEOUT);
                cell.fg = Color::Named(NamedColor::Foreground);
                cell.bg = Color::Named(NamedColor::Background);
            }
            1 => cell.flags.insert(Flags::BOLD),
            2 => cell.flags.insert(Flags::DIM),
            3 => cell.flags.insert(Flags::ITALIC),
            4 => cell.flags.insert(Flags::UNDERLINE),
            7 => cell.flags.insert(Flags::INVERSE),
            8 => cell.flags.insert(Flags::HIDDEN),
            9 => cell.flags.insert(Flags::STRIKEOUT),
            22 => cell.flags.remove(Flags::BOLD | Flags::DIM),
            23 => cell.flags.remove(Flags::ITALIC),
            24 => cell.flags.remove(Flags::UNDERLINE),
            27 => cell.flags.remove(Flags::INVERSE),
            28 => cell.flags.remove(Flags::HIDDEN),
            29 => cell.flags.remove(Flags::STRIKEOUT),
            n @ 30..=37 => cell.fg = Color::Indexed((n - 30) as u8),
            n @ 90..=97 => cell.fg = Color::Indexed((n - 90 + 8) as u8),
            n @ 40..=47 => cell.bg = Color::Indexed((n - 40) as u8),
            n @ 100..=107 => cell.bg = Color::Indexed((n - 100 + 8) as u8),
            39 => cell.fg = Color::Named(NamedColor::Foreground),
            49 => cell.bg = Color::Named(NamedColor::Background),
            38 => {
                if let Some(col) = color(p, &params[i + 1..], &mut used) {
                    cell.fg = col;
                }
            }
            48 => {
                if let Some(col) = color(p, &params[i + 1..], &mut used) {
                    cell.bg = col;
                }
            }
            _ => {}
        }
        i += 1 + used;
    }
}

/// A URL with a scheme (Make Hyperlink ignores anything else).
fn url_ok(v: &str) -> bool {
    v.split_once(':').is_some_and(|(scheme, rest)| {
        scheme.len() > 1 && scheme.chars().next().is_some_and(|c| c.is_ascii_alphabetic()) && scheme.chars().all(|c| c.is_ascii_alphanumeric() || "+.-".contains(c)) && !rest.is_empty()
    })
}

/// iTerm2's triggerRadius: rows before and after a row a logical line may span.
const RADIUS: i32 = 3;
const INSTANT_EVERY: Duration = Duration::from_millis(500);

impl Screen {
    /// The profile's triggers. A trigger that is unchanged keeps its state (the line an
    /// Instant trigger fired on, Disable This Alert), so a settings refresh fires nothing.
    pub fn set_triggers(&mut self, list: Vec<Trigger>, prefs: TriggerPrefs) {
        let old = std::mem::take(&mut self.triggers.list);
        self.triggers.list = list
            .into_iter()
            .enumerate()
            .map(|(i, trigger)| {
                if let Some(prev) = old.get(i).filter(|p| p.trigger == trigger) {
                    return prev.clone();
                }
                let (re, error) = match fancy_regex::Regex::new(&trigger.regex) {
                    Ok(re) => (Some(re), None),
                    Err(e) => (None, Some(e.to_string())),
                };
                Compiled { trigger, re, error, last_line: None, off_here: false, events: Default::default() }
            })
            .collect();
        self.triggers.prefs = prefs;
    }

    /// Disable This Alert: trigger `i` stays off for this session.
    pub fn disable_trigger_here(&mut self, i: usize) {
        if let Some(c) = self.triggers.list.get_mut(i) {
            c.off_here = true;
        }
    }

    /// Patterns that do not compile: (index, error).
    pub fn trigger_errors(&self) -> Vec<(usize, String)> {
        self.triggers.list.iter().enumerate().filter_map(|(i, c)| c.error.clone().map(|e| (i, e))).collect()
    }

    fn row_number(&self, line: i32) -> i64 {
        self.term.grid().scrolled_off() as i64 + line as i64
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
    /// Saved text (an arrangement's contents) as plain history: control characters go, so no
    /// escape sequence acts; triggers are off while it goes in and never check its lines.
    pub fn feed_inert(&mut self, text: &str) {
        let text = crate::arrangement::inert_text(text).replace("\r\n", "\n").replace('\n', "\r\n");
        self.feed_quiet(text.as_bytes());
    }

    /// A tmux pane's history and screen from tmux's captures: triggers do not see them
    /// (they are not new output) and no reply goes back.
    pub fn feed_tmux_restore(&mut self, bytes: &[u8]) {
        self.feed_quiet(bytes);
    }

    /// A tmux gateway message (iTerm2's printTmuxMessage): the text as is, in the default
    /// colors, then a new line. No escape sequence in it acts.
    pub fn print_message(&mut self, text: &str) {
        self.feed_quiet(b"\x1b[39;49m");
        let text = crate::arrangement::inert_text(text).replace(['\r', '\n'], " ");
        self.feed_quiet(format!("{text}\r\n").as_bytes());
    }

    /// Session restoration's contents (SGR-safe styled text) and the optional banner
    /// (text, (background gray, text gray)): real cells, a full-width bar on its own line.
    pub fn feed_restored(&mut self, text: &str, cursor: Option<(usize, usize)>, banner: Option<(&str, (u8, u8))>) {
        let mut bytes = crate::restore_state::sgr_safe(text).replace("\r\n", "\n").replace('\n', "\r\n");
        bytes.push_str("\x1b[0m");
        // The cursor goes back where the shell left it, so the shell's own view of the
        // screen and this one agree when a reattached shell writes on.
        if let Some((row, col)) = cursor {
            bytes += &format!("\x1b[{};{}H", row + 1, col + 1);
        }
        self.feed_quiet(bytes.as_bytes());
        if let Some((line, (bg, fg))) = banner {
            let mut b = String::new();
            if self.term.grid().cursor.point.column.0 != 0 {
                b.push_str("\r\n");
            }
            // Erasing with the bar's background paints the whole line (iTerm2 does the same).
            b += &format!("\x1b[48;2;{bg};{bg};{bg}m\x1b[38;2;{fg};{fg};{fg}m\x1b[2K{}\x1b[0m\r\n", crate::arrangement::inert_text(line));
            self.feed_quiet(b.as_bytes());
        }
    }

    /// Output that is not the program's: no triggers see it, no request it makes is kept,
    /// and triggers never check its lines later.
    fn feed_quiet(&mut self, bytes: &[u8]) {
        let list = std::mem::take(&mut self.triggers.list);
        let queued = self.requests.len();
        let _ = self.feed(bytes);
        self.requests.truncate(queued);
        self.triggers.list = list;
        self.triggers.next = self.triggers.next.max(self.cursor_row());
    }

    pub(crate) fn run_triggers_after_feed(&mut self) {
        let cursor = self.cursor_row();
        let alt = self.term.mode().contains(TermMode::ALT_SCREEN);
        if alt != self.triggers.alt {
            // Another grid, other row numbers.
            self.triggers.alt = alt;
            self.triggers.next = cursor;
            return;
        }
        if !self.triggers_active() {
            if !self.triggers.busy {
                self.triggers.next = self.triggers.next.max(cursor);
            }
            return;
        }
        let g = self.term.grid();
        let (history, cols) = (g.history_size() as i64, g.columns());
        let first = self.triggers.next.max(g.scrolled_off() as i64 - history);
        // A high-water mark: a cursor moving up does not bring lines back to check.
        self.triggers.next = self.triggers.next.max(cursor);
        for row in first..cursor {
            // An action (Inject Data) may have switched screens: that grid's rows are not these.
            if self.term.mode().contains(TermMode::ALT_SCREEN) != alt {
                break;
            }
            let g = self.term.grid();
            let line = (row - g.scrolled_off() as i64) as i32;
            if line >= g.screen_lines() as i32 {
                break;
            }
            if line < -(g.history_size() as i32) {
                continue;
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
            let written = (0..cols).rev().find(|c| row[Column(*c)].c != ' ' || row[Column(*c)].flags.contains(Flags::WIDE_CHAR)).map_or(0, |c| c + 1);
            // On the cursor's row the text runs to the cursor (a prompt's trailing space).
            let cursor = g.cursor.point;
            let written = if cursor.line.0 == l { written.max(cursor.column.0.min(cols)) } else { written };
            let used = if wraps || l < end { cols } else { written };
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
    pub(crate) fn trigger_var(&self, name: &str, row: i64) -> Option<String> {
        match name {
            "session.hostname" => self.shell.host.clone(),
            "session.username" => self.shell.user.clone(),
            "session.path" => self.shell.cwd.clone(),
            "session.jobName" => self.shell.job.as_ref().map(|j| j.name.clone()),
            "session.processTitle" => self.shell.job.as_ref().map(|j| j.title.clone()),
            "session.commandLine" => self.shell.job.as_ref().map(|j| j.cmdline.clone()),
            "session.jobPid" => self.shell.job.as_ref().map(|j| j.pid.to_string()),
            "session.foregroundJobAncestors" => Some(self.shell.job_ancestors.join("\n")),
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
            if !t.enabled || c.off_here || c.re.is_none() || t.match_type != 0 || !crate::trigger_events::job_gate(&t.job, &self.shell.job_ancestors) {
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
                // iTerm2 interpolates only a parameter holding an expression.
                let value = if self.triggers.prefs.interpolated && param.contains("\\(") {
                    match crate::interp::evaluate(&param, &|n| self.trigger_var(n, row), &groups) {
                        Ok(v) => v,
                        Err(e) => {
                            if self.triggers.list[i].error.as_ref() != Some(&e) {
                                self.requests.push(Request::Trigger(TriggerRequest::ParameterError { trigger: i, text: e.clone() }));
                                self.triggers.list[i].error = Some(e);
                            }
                            break;
                        }
                    }
                } else {
                    triggers::substitute(&param, &groups)
                };
                fired = true;
                match self.trigger_action(i, &value, (m.start(), m.end()), &line, row, &groups) {
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

    /// An event trigger's action, on the cursor's line. True to stop the rest.
    pub(crate) fn run_event_action(&mut self, i: usize, value: &str, groups: &[Option<&str>]) -> bool {
        let line = self.logical_line(self.term.grid().cursor.point.line.0);
        let row = self.cursor_row();
        matches!(self.trigger_action(i, value, (0, 0), &line, row, groups), Flow::StopAll)
    }

    /// The cells of a match: (line, column), from byte offsets in the logical line.
    fn match_cells(line: &LineText, span: (usize, usize)) -> Vec<(i32, usize)> {
        line.cells.iter().filter(|(b, _, _)| *b >= span.0 && *b < span.1).map(|(_, l, c)| (*l, *c)).collect()
    }

    fn each_cell(&mut self, cells: &[(i32, usize)], f: &mut dyn FnMut(&mut Cell)) {
        let g = self.term.grid_mut();
        let (top, lines, cols) = (-(g.history_size() as i32), g.screen_lines() as i32, g.columns());
        for &(l, col) in cells {
            if l >= top && l < lines && col < cols {
                f(&mut g[Line(l)][Column(col)]);
            }
        }
    }

    /// Runs trigger `i`'s action on a match (byte range in the line). `row` is the line's
    /// absolute row number and `groups` its capture groups (event triggers pass their own
    /// captured strings; a plain regex match its `captures_iter` groups).
    fn trigger_action(&mut self, i: usize, value: &str, span: (usize, usize), line: &LineText, row: i64, groups: &[Option<&str>]) -> Flow {
        let raw = self.triggers.list[i].trigger.action.clone();
        let Some(info) = triggers::action_info(&raw) else {
            self.requests.push(Request::Trigger(TriggerRequest::Unavailable { trigger: i, title: raw, milestone: 0 }));
            return Flow::Done;
        };
        if let Some(m) = info.later {
            self.requests.push(Request::Trigger(TriggerRequest::Unavailable { trigger: i, title: info.title.to_string(), milestone: m }));
            return Flow::Done;
        }
        let cells = Self::match_cells(line, span);
        let first = cells.first().copied();
        let end_line = line.cells.last().map_or(0, |(_, l, _)| *l);
        let ask = |s: &mut Screen, r: TriggerRequest| {
            s.requests.push(Request::Trigger(r));
            Flow::Done
        };
        match info.class {
            "HighlightTrigger" => {
                let (fg, bg) = highlight_colors(value);
                self.each_cell(&cells, &mut |c| {
                    if let Some(f) = fg {
                        c.fg = Color::Spec(f);
                    }
                    if let Some(b) = bg {
                        c.bg = Color::Spec(b);
                    }
                });
                Flow::More
            }
            "iTermHighlightLineTrigger" => {
                let (fg, bg) = highlight_colors(value);
                let cols = self.term.grid().columns();
                let rows: Vec<i32> = {
                    let mut r: Vec<i32> = line.cells.iter().map(|(_, l, _)| *l).collect();
                    r.dedup();
                    if r.is_empty() {
                        r.push(end_line);
                    }
                    r.truncate(10);
                    r
                };
                let all: Vec<(i32, usize)> = rows.iter().flat_map(|l| (0..cols).map(move |c| (*l, c))).collect();
                self.each_cell(&all, &mut |c| {
                    if let Some(f) = fg {
                        c.fg = Color::Spec(f);
                    }
                    if let Some(b) = bg {
                        c.bg = Color::Spec(b);
                    }
                });
                Flow::Done
            }
            "iTermSGRTrigger" => {
                let v = value.to_string();
                self.each_cell(&cells, &mut |c| apply_sgr(c, &v));
                Flow::More
            }
            "iTermHyperlinkTrigger" => {
                if url_ok(value) {
                    let link = Hyperlink::new(None::<String>, value.to_string());
                    self.each_cell(&cells, &mut |c| c.set_hyperlink(Some(link.clone())));
                }
                Flow::More
            }
            "AnnotateTrigger" => {
                if let (Some((l0, c0)), Some((l1, c1))) = (first, cells.last().copied()) {
                    if !self.term.mode().contains(TermMode::ALT_SCREEN) && !value.is_empty() {
                        self.add_note(l0, c0, l1, c1 + 1, value, false);
                    }
                }
                Flow::Done
            }
            "MarkTrigger" => {
                let l = first.map_or(end_line, |f| f.0);
                self.set_manual_mark(l, None);
                if value.trim() == "1" {
                    // Stop Scrolling: the marked line is the last one in view.
                    let g = self.term.grid();
                    let want = (g.screen_lines() as i32 - 1 - l).clamp(0, g.history_size() as i32);
                    let delta = want - g.display_offset() as i32;
                    self.term.scroll_display(alacritty_terminal::grid::Scroll::Delta(delta));
                }
                Flow::Done
            }
            "iTermSetNamedMarkTrigger" => {
                self.set_manual_mark(first.map_or(end_line, |f| f.0), Some(value.to_string()));
                Flow::Done
            }
            "iTermFoldTrigger" => {
                let here = first.map_or(end_line, |f| f.0);
                let from = self.marks_in_order().into_iter().rev().find(|(l, m)| *l < here && m.name.as_deref() == Some(value)).map(|(l, _)| l);
                if let Some(from) = from {
                    self.fold(from, end_line);
                }
                Flow::Done
            }
            "iTermShellPromptTrigger" => {
                let l = first.map_or(end_line, |f| f.0);
                if self.term.grid()[Line(l)].mark() == 0 {
                    if l == self.term.grid().cursor.point.line.0 {
                        self.apply(crate::osc::Osc::Prompt { kind: crate::osc::PromptKind::Initial, aid: None, fresh_line: false });
                        self.apply(crate::osc::Osc::CommandStart);
                    } else {
                        self.new_mark(l, crate::marks::Kind::Prompt);
                    }
                }
                Flow::Done
            }
            "SetDirectoryTrigger" => {
                if !value.is_empty() {
                    self.apply(crate::osc::Osc::Kv("CurrentDir".into(), value.to_string()));
                }
                Flow::Done
            }
            "SetHostnameTrigger" => {
                if !value.is_empty() {
                    self.apply(crate::osc::Osc::Kv("RemoteHost".into(), value.to_string()));
                }
                Flow::Done
            }
            "iTermInjectTrigger" => {
                // The text goes through the parser once, as output; it runs no triggers.
                self.triggers.busy = true;
                self.feed(value.as_bytes());
                self.triggers.busy = false;
                self.triggers_from_cursor();
                // The line's cells may have moved: later triggers wait for the next line.
                Flow::StopAll
            }
            "iTermSetUserVariableTrigger" => {
                if let Some((name, v)) = value.split_once('\u{1}') {
                    let name = name.strip_prefix("user.").unwrap_or(name);
                    if !name.is_empty() && !name.contains('.') {
                        self.set_user_var(name, v.to_string());
                    }
                }
                Flow::Done
            }
            "StopTrigger" => Flow::StopAll,
            "AlertTrigger" => ask(self, TriggerRequest::Alert { trigger: i, text: value.to_string() }),
            "iTermUserNotificationTrigger" => ask(self, TriggerRequest::Notify(value.to_string())),
            "BellTrigger" => {
                self.bell_count += 1;
                ask(self, TriggerRequest::Bell)
            }
            "BounceTrigger" => ask(self, TriggerRequest::Bounce { until_active: value.trim() != "1" }),
            "SendTextTrigger" => ask(self, TriggerRequest::SendText(value.to_string())),
            "ScriptTrigger" => ask(self, TriggerRequest::RunCommand(value.to_string())),
            "iTermSetTitleTrigger" => ask(self, TriggerRequest::SetTitle(value.to_string())),
            "iTermSetTabStatusTrigger" => {
                let mut parts = value.splitn(3, '\u{1}');
                let (dot, text, status) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""), parts.next().unwrap_or(""));
                ask(self, TriggerRequest::TabStatus { status: status.into(), dot: dot.into(), text: text.into() })
            }
            "iTermBufferInputTrigger" => ask(self, TriggerRequest::BufferInput(value.trim() != "1")),
            "iTermSetProfileBooleanTrigger" => match value.split_once('\u{1}') {
                // Only a profile check box (never the trigger list or another value).
                Some((key, on)) if crate::schema::find(key).is_some_and(|s| s.kind == crate::schema::Kind::Bool) => ask(self, TriggerRequest::ProfileSetting { key: key.into(), on: on.trim() == "1" }),
                _ => Flow::Done,
            },
            "CoprocessTrigger" | "MuteCoprocessTrigger" => ask(self, TriggerRequest::Coprocess { cmd: value.to_string(), silent: info.class == "MuteCoprocessTrigger" }),
            "CaptureTrigger" => {
                let needs_shell_integration = self.marks.last_command.is_none();
                if let Some(id) = self.marks.last_command {
                    let command = (!value.is_empty()).then(|| value.to_string());
                    let values: Vec<String> = groups.iter().map(|g| (*g).unwrap_or("").to_string()).collect();
                    let entry = crate::captured::Captured { line: line.text.clone(), values, command, state: false, abs_line: row, mark: id };
                    if let Some(m) = self.marks.records.get_mut(&id) {
                        crate::captured::add(&mut m.captured, entry);
                    }
                }
                self.requests.push(Request::Trigger(TriggerRequest::Captured { needs_shell_integration }));
                // One fire per line (CaptureTrigger.m:65 returns NO; Trigger.m:367-372 stops
                // enumerating that trigger's matches, not the whole trigger list).
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
    fn restore_matches_the_saved_viewport() {
        use alacritty_terminal::index::{Column, Line};
        let row = |s: &Screen, l: i32| -> String { (0..20).map(|c| s.term().grid()[Line(l)][Column(c)].c).collect::<String>().trim_end().to_string() };
        let cursor = |s: &Screen| (s.term().grid().cursor.point.line.0, s.term().grid().cursor.point.column.0);
        // History above, the prompt on the last row with the cursor after "$ ".
        let mut s = Screen::with_scrollback(20, 4, 100);
        s.feed(b"one\r\ntwo\r\nthree\r\nfour\r\nfive\r\n$ ");
        let (text, at) = crate::styled_copy::sgr_restorable(&s);
        let mut r = Screen::with_scrollback(20, 4, 100);
        r.feed_restored(&text, Some(at), None);
        for l in -2..4 {
            assert_eq!(row(&r, l), row(&s, l), "line {l}");
        }
        assert_eq!(cursor(&r), (3, 2), "the cursor sits where the shell left it");
        // The cursor high on the screen, blank rows below it: they are not pushed away.
        let mut s = Screen::with_scrollback(20, 4, 100);
        s.feed(b"a\r\n");
        let (text, at) = crate::styled_copy::sgr_restorable(&s);
        let mut r = Screen::with_scrollback(20, 4, 100);
        r.feed_restored(&text, Some(at), None);
        assert_eq!((row(&r, 0), cursor(&r)), ("a".to_string(), (1, 0)));
        // A full-screen program is up: the shell's screen and history are what is saved.
        let mut s = Screen::with_scrollback(20, 4, 100);
        s.feed(b"shell\r\n\x1b[?1049hVIM");
        let (text, at) = crate::styled_copy::sgr_restorable(&s);
        assert!(text.contains("shell") && !text.contains("VIM"), "{text:?}");
        assert_eq!(at, (1, 0));
    }

    #[test]
    fn restored_contents_keep_colors_and_fire_nothing() {
        use alacritty_terminal::index::{Column, Line};
        use alacritty_terminal::vte::ansi::{Color, NamedColor};
        let mut s = Screen::new(40, 5);
        s.set_triggers(vec![trig("Reply", "AlertTrigger", "x", false)], TriggerPrefs::default());
        s.feed_restored("\x1b[0;31mReply\x1b[0m\n\x1b]0;evil\x07ok\n", None, None);
        assert_eq!(alerts(&mut s), 0);
        assert!(s.take_requests().is_empty());
        assert_ne!(s.title(), Some("evil"));
        assert_eq!(s.term().grid()[Line(0)][Column(0)].fg, Color::Named(NamedColor::Red));
        assert_eq!(s.term().grid()[Line(1)][Column(0)].c, 'o');
        s.feed(b"Reply\r\n");
        assert_eq!(alerts(&mut s), 1, "new output still fires");
    }

    #[test]
    fn banner_is_one_full_line() {
        use alacritty_terminal::index::{Column, Line};
        use alacritty_terminal::vte::ansi::{Color, Rgb};
        let mut s = Screen::new(20, 5);
        s.set_triggers(vec![trig("Restored", "AlertTrigger", "x", false)], TriggerPrefs::default());
        s.feed_restored("abc", None, Some(("Session Contents Restored on X", (38, 102))));
        let g = s.term().grid();
        let row: String = (0..20).map(|c| g[Line(1)][Column(c)].c).collect();
        assert_eq!(row.trim_end(), "Session Contents Res", "the banner starts its own line");
        assert_eq!(g[Line(0)][Column(0)].c, 'a');
        assert_eq!(g[Line(1)][Column(0)].bg, Color::Spec(Rgb { r: 38, g: 38, b: 38 }));
        assert_eq!(g[Line(1)][Column(0)].fg, Color::Spec(Rgb { r: 102, g: 102, b: 102 }));
        let mut w = Screen::new(40, 5);
        w.feed_restored("", None, Some(("B", (38, 102))));
        let g = w.term().grid();
        assert_eq!(g[Line(0)][Column(39)].bg, Color::Spec(Rgb { r: 38, g: 38, b: 38 }), "the bar spans the width");
        assert_eq!((g.cursor.point.line.0, g.cursor.point.column.0), (1, 0), "the shell starts below it");
        assert_eq!(g[Line(1)][Column(0)].bg, Color::Named(alacritty_terminal::vte::ansi::NamedColor::Background));
        assert_eq!(alerts(&mut s), 0, "triggers do not see the banner");
    }

    #[test]
    fn gateway_messages_print_literally() {
        let mut s = Screen::new(40, 5);
        s.feed(b"\x1b[31;44m");
        s.print_message("tmux \x1b]0;evil\x07on");
        assert_ne!(s.title(), Some("evil"), "no escape sequence acts");
        let g = s.term().grid();
        assert_eq!(g[Line(0)][Column(0)].c, 't');
        assert_eq!(g[Line(0)][Column(0)].fg, Color::Named(alacritty_terminal::vte::ansi::NamedColor::Foreground), "default colors");
        assert_eq!(g[Line(0)][Column(0)].bg, Color::Named(alacritty_terminal::vte::ansi::NamedColor::Background));
        assert_eq!((g.cursor.point.line.0, g.cursor.point.column.0), (1, 0), "then a new line");
    }

    #[test]
    fn restored_text_fires_nothing() {
        let mut s = Screen::new(40, 5);
        s.set_triggers(vec![trig("Reply", "AlertTrigger", "x", false)], TriggerPrefs::default());
        s.feed_inert("Reply one\nReply two\x1b]0;evil\x07\n");
        assert_eq!(alerts(&mut s), 0, "saved text is not new output");
        assert!(s.take_requests().is_empty(), "no escape sequence acts");
        assert_ne!(s.title(), Some("evil"));
        s.feed(b"\r\n");
        assert_eq!(alerts(&mut s), 0, "the restored lines are not checked later either");
        s.feed(b"Reply three\r\n");
        assert_eq!(alerts(&mut s), 1, "new output still fires");
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
        // The read goes in pieces, so every line is checked before it scrolls away, and no
        // cell action touches a row that is gone.
        assert_eq!(s.triggers.lines_checked, 50);
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
    fn job_field_gates_triggers() {
        let mut s = Screen::new(40, 5);
        let mut t = trig("foo", "AlertTrigger", "x", false);
        t.job = "Vim.exe".into();
        s.set_triggers(vec![t], TriggerPrefs::default());
        s.feed(b"foo\r\n");
        assert_eq!(alerts(&mut s), 0, "vim is not running");
        s.set_job(None, vec!["vim".into(), "cmd".into()]);
        s.feed(b"foo\r\n");
        assert_eq!(alerts(&mut s), 1, "vim is in the ancestors");
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

    use alacritty_terminal::index::{Column, Line};
    use alacritty_terminal::term::cell::Flags;
    use alacritty_terminal::vte::ansi::{Color, Rgb};

    fn one(regex: &str, action: &str, parameter: &str, text: &[u8]) -> Screen {
        let mut s = Screen::with_scrollback(40, 6, 100);
        s.set_triggers(vec![trig(regex, action, parameter, false)], TriggerPrefs::default());
        s.feed(text);
        s
    }

    fn cell(s: &Screen, line: i32, col: usize) -> alacritty_terminal::term::cell::Cell {
        s.term().grid()[Line(line)][Column(col)].clone()
    }

    fn spec(r: u8, g: u8, b: u8) -> Color {
        Color::Spec(Rgb { r, g, b })
    }

    #[test]
    fn highlight_text_colors_the_match() {
        let s = one("err", "HighlightTrigger", "{#ff0000,#0000ff}", b"an err here\r\n");
        assert_eq!((cell(&s, 0, 3).fg, cell(&s, 0, 3).bg), (spec(255, 0, 0), spec(0, 0, 255)));
        assert_eq!(cell(&s, 0, 5).fg, spec(255, 0, 0));
        assert_ne!(cell(&s, 0, 2).fg, spec(255, 0, 0), "only the match");
        assert_ne!(cell(&s, 0, 6).fg, spec(255, 0, 0));
        let s = one("err", "HighlightTrigger", "{,#00ff00}", b"err\r\n");
        assert_eq!(cell(&s, 0, 0).bg, spec(0, 255, 0));
        assert_ne!(cell(&s, 0, 0).fg, spec(0, 255, 0), "an empty side stays");
        let s = one("err", "HighlightTrigger", "2", b"err err\r\n");
        assert_eq!((cell(&s, 0, 0).fg, cell(&s, 0, 0).bg), (spec(255, 255, 255), spec(255, 0, 0)), "legacy preset: White on Red");
        assert_eq!(cell(&s, 0, 4).bg, spec(255, 0, 0), "every match");
        let s = one("err", "HighlightTrigger", "", b"err\r\n");
        assert_eq!(cell(&s, 0, 0).bg, spec(255, 0, 0), "default white on red");
        assert_eq!(highlight_colors("1009"), (Some(Rgb { r: 255, g: 255, b: 0 }), None), "Yellow Foreground");
        assert_eq!(highlight_colors("2005"), (None, Some(Rgb { r: 255, g: 0, b: 0 })), "Red Background");
    }

    #[test]
    fn highlight_line_colors_the_line() {
        let s = one("warn", "iTermHighlightLineTrigger", "{,#ff0000}", b"a warn\r\nnext\r\n");
        assert_eq!(cell(&s, 0, 0).bg, spec(255, 0, 0));
        assert_eq!(cell(&s, 0, 39).bg, spec(255, 0, 0), "the whole width");
        assert_ne!(cell(&s, 1, 0).bg, spec(255, 0, 0));
    }

    #[test]
    fn change_style_applies_sgr() {
        let s = one("hot", "iTermSGRTrigger", "1;31", b"so hot\r\n");
        let c = cell(&s, 0, 3);
        assert!(c.flags.contains(Flags::BOLD));
        assert_eq!(c.fg, Color::Indexed(1));
        let s = one("hot", "iTermSGRTrigger", "38:2:1:10:20:30;4", b"hot\r\n");
        assert_eq!(cell(&s, 0, 0).fg, spec(10, 20, 30));
        assert!(cell(&s, 0, 0).flags.contains(Flags::UNDERLINE));
        let s = one("hot", "iTermSGRTrigger", "48;5;200", b"hot\r\n");
        assert_eq!(cell(&s, 0, 0).bg, Color::Indexed(200));
    }

    #[test]
    fn hyperlink_trigger_links_cells() {
        let s = one(r"id(\d+)", "iTermHyperlinkTrigger", "https://example.com/\\1", b"see id42 now\r\n");
        assert_eq!(cell(&s, 0, 4).hyperlink().map(|h| h.uri().to_string()), Some("https://example.com/42".into()));
        assert_eq!(cell(&s, 0, 3).hyperlink(), None);
        let s = one(r"id(\d+)", "iTermHyperlinkTrigger", "not a url", b"id42\r\n");
        assert_eq!(cell(&s, 0, 0).hyperlink(), None);
    }

    #[test]
    fn annotate_and_marks() {
        let mut s = one("todo", "AnnotateTrigger", "look here", b"a todo item\r\n");
        assert_eq!(s.notes_in_order().len(), 1);
        let mut s2 = one("mark me", "MarkTrigger", "0", b"x\r\nmark me\r\n");
        assert!(s2.marks_in_order().iter().any(|(l, _)| *l == 1));
        let mut s3 = one("build", "iTermSetNamedMarkTrigger", "Build \\0", b"build\r\n");
        assert!(s3.marks_in_order().iter().any(|(_, m)| m.name.as_deref() == Some("Build build")));
        let _ = &mut s;
    }

    #[test]
    fn fold_to_named_mark() {
        let mut s = Screen::with_scrollback(40, 8, 100);
        s.set_triggers(vec![trig("^start$", "iTermSetNamedMarkTrigger", "top", false), trig("^end$", "iTermFoldTrigger", "top", false)], TriggerPrefs::default());
        s.feed(b"start\r\na\r\nb\r\nend\r\n$ ");
        assert_eq!(s.fold_count(), 1);
        let mut t = Screen::new(40, 5);
        t.set_triggers(vec![trig("^end$", "iTermFoldTrigger", "nope", false)], TriggerPrefs::default());
        t.feed(b"end\r\n");
        assert_eq!(t.fold_count(), 0, "no mark with that name");
    }

    #[test]
    fn prompt_detected_marks_once() {
        let mut s = Screen::new(40, 5);
        s.set_triggers(vec![trig(r"^\$ ", "iTermShellPromptTrigger", "", true)], TriggerPrefs::default());
        s.feed(b"out\r\n$ ");
        let t0 = std::time::Instant::now();
        s.check_instant_triggers(t0);
        s.check_instant_triggers(t0 + std::time::Duration::from_secs(1));
        let prompts = s.marks_in_order().into_iter().filter(|(_, m)| m.kind == crate::marks::Kind::Prompt).count();
        assert_eq!(prompts, 1);
    }

    #[test]
    fn report_directory_and_host() {
        let s = one(r"cd (\S+)", "SetDirectoryTrigger", "/tmp/\\1", b"cd x\r\n");
        assert_eq!(s.shell.cwd.as_deref(), Some("/tmp/x"));
        let s = one("login", "SetHostnameTrigger", "bob@box", b"login\r\n");
        assert_eq!((s.shell.user.as_deref(), s.shell.host.as_deref()), (Some("bob"), Some("box")));
    }

    #[test]
    fn injected_text_does_not_refire_forever() {
        let mut s = Screen::with_scrollback(40, 6, 100);
        s.set_triggers(vec![trig("ping", "iTermInjectTrigger", "ping\\r\\n", false)], TriggerPrefs::default());
        s.feed(b"ping\r\n");
        let rows: Vec<String> = (0..6).map(|l| s.line_text(l).trim_end().to_string()).collect();
        assert_eq!(rows.iter().filter(|r| *r == "ping").count(), 2, "{rows:?}");
        assert_eq!(s.triggers.lines_checked, 1);
    }

    #[test]
    fn user_variable_names() {
        let s = one("x", "iTermSetUserVariableTrigger", "user.a\u{1}v", b"x\r\n");
        assert_eq!(s.shell.vars.get("a").map(String::as_str), Some("v"));
        let s = one("x", "iTermSetUserVariableTrigger", "a.b\u{1}v", b"x\r\n");
        assert!(s.shell.vars.is_empty(), "a dotted name is refused");
    }

    #[test]
    fn window_requests() {
        let cases: Vec<(&str, &str, TriggerRequest)> = vec![
            ("BellTrigger", "", TriggerRequest::Bell),
            ("BounceTrigger", "1", TriggerRequest::Bounce { until_active: false }),
            ("BounceTrigger", "0", TriggerRequest::Bounce { until_active: true }),
            ("SendTextTrigger", "yes\\r", TriggerRequest::SendText("yes\r".into())),
            ("ScriptTrigger", "echo hi", TriggerRequest::RunCommand("echo hi".into())),
            ("iTermSetTitleTrigger", "T", TriggerRequest::SetTitle("T".into())),
            ("iTermSetTabStatusTrigger", "#00ff00\u{1}#ffffff\u{1}work", TriggerRequest::TabStatus { dot: "#00ff00".into(), text: "#ffffff".into(), status: "work".into() }),
            ("iTermBufferInputTrigger", "0", TriggerRequest::BufferInput(true)),
            ("iTermBufferInputTrigger", "1", TriggerRequest::BufferInput(false)),
            ("iTermSetProfileBooleanTrigger", "Silence Bell\u{1}1", TriggerRequest::ProfileSetting { key: "Silence Bell".into(), on: true }),
            ("MuteCoprocessTrigger", "cat", TriggerRequest::Coprocess { cmd: "cat".into(), silent: true }),
            ("iTermUserNotificationTrigger", "done", TriggerRequest::Notify("done".into())),
            ("GrowlTrigger", "done", TriggerRequest::Notify("done".into())),
            ("CaptureTrigger", "", TriggerRequest::Captured { needs_shell_integration: true }),
        ];
        for (action, param, want) in cases {
            let mut s = one("go", action, param, b"go\r\n");
            let got: Vec<TriggerRequest> = s.take_requests().into_iter().filter_map(|r| match r {
                Request::Trigger(t) => Some(t),
                _ => None,
            }).collect();
            assert_eq!(got, vec![want], "{action}");
        }
    }

    #[test]
    fn moving_the_cursor_up_does_not_recheck() {
        let mut s = Screen::new(40, 8);
        s.set_triggers(vec![trig("done", "AlertTrigger", "x", false)], TriggerPrefs::default());
        s.feed(b"a done\r\nb\r\nc\r\n");
        assert_eq!(alerts(&mut s), 1);
        // A progress display: up three rows, redraw, back down.
        s.feed(b"\x1b[3A\rA done");
        s.feed(b"\x1b[3B\r");
        s.feed(b"d\r\n");
        assert_eq!(alerts(&mut s), 0, "no line checked twice");
    }

    #[test]
    fn a_read_bigger_than_the_scrollback_checks_every_line() {
        let mut s = Screen::with_scrollback(40, 5, 100);
        s.set_triggers(vec![trig(r"^line (\d+)$", "iTermSetUserVariableTrigger", "last\u{1}\\1", false)], TriggerPrefs::default());
        let text: String = (0..3000).map(|i| format!("line {i}\r\n")).collect();
        s.feed(text.as_bytes());
        assert_eq!(s.triggers.lines_checked, 3000);
        assert_eq!(s.shell.vars.get("last").map(String::as_str), Some("2999"));
    }

    #[test]
    fn clear_and_alternate_visits_keep_row_numbers() {
        let mut s = Screen::with_scrollback(40, 5, 100);
        s.set_triggers(vec![trig("x", "AlertTrigger", "x", false)], TriggerPrefs::default());
        s.feed(b"a\r\nb\r\n\x1b[H\x1b[2Jx1\r\nx2\r\n");
        assert_eq!(s.triggers.lines_checked, 4, "lines after a clear in the same read are checked");
        assert_eq!(alerts(&mut s), 2);
        let alt: String = (0..20).map(|i| format!("alt {i}\r\n")).collect();
        s.feed(format!("\x1b[?1049h{alt}\x1b[?1049lx3\r\n").as_bytes());
        assert_eq!(s.triggers.lines_checked, 5, "the alternate screen's scrolling does not renumber the main screen");
        assert_eq!(alerts(&mut s), 1);
    }

    #[test]
    fn interpolation_only_with_an_expression() {
        let mut s = Screen::new(40, 5);
        s.set_triggers(vec![trig(r"id=(\w+)", "SendTextTrigger", "\\1\\r", false)], TriggerPrefs { interactive: false, interpolated: true });
        s.feed(b"id=abc\r\n");
        let got: Vec<TriggerRequest> = s.take_requests().into_iter().filter_map(|r| match r {
            Request::Trigger(t) => Some(t),
            _ => None,
        }).collect();
        assert_eq!(got, vec![TriggerRequest::SendText("abc\r".into())], "no \\( in the parameter: plain backreferences");
    }

    #[test]
    fn parameter_errors_are_reported_once() {
        let mut s = Screen::new(40, 5);
        s.set_triggers(vec![trig("x", "AlertTrigger", "\\(f(1))", false)], TriggerPrefs { interactive: false, interpolated: true });
        s.feed(b"x\r\nx\r\n");
        let errors: Vec<TriggerRequest> = s.take_requests().into_iter().filter_map(|r| match r {
            Request::Trigger(t @ TriggerRequest::ParameterError { .. }) => Some(t),
            _ => None,
        }).collect();
        assert_eq!(errors.len(), 1, "{errors:?}");
    }

    #[test]
    fn refreshing_settings_keeps_trigger_state() {
        let mut s = Screen::new(40, 5);
        let list = vec![trig("Password:", "AlertTrigger", "x", true)];
        s.set_triggers(list.clone(), TriggerPrefs::default());
        s.feed(b"Password:");
        let t0 = std::time::Instant::now();
        s.check_instant_triggers(t0);
        assert_eq!(alerts(&mut s), 1);
        s.set_triggers(list.clone(), TriggerPrefs::default());
        s.check_instant_triggers(t0 + std::time::Duration::from_secs(1));
        assert_eq!(alerts(&mut s), 0, "a settings refresh does not fire it again");
        s.disable_trigger_here(0);
        s.set_triggers(list, TriggerPrefs::default());
        assert!(s.triggers.list[0].off_here, "Disable This Alert lasts the session");
    }

    #[test]
    fn inject_that_switches_screens_stops_the_line() {
        let mut s = Screen::with_scrollback(40, 5, 100);
        s.set_triggers(vec![trig("go", "iTermInjectTrigger", "\\e[?1049h", false), trig(".", "HighlightTrigger", "", false)], TriggerPrefs::default());
        let text: String = std::iter::once("go\r\n".to_string()).chain((0..20).map(|i| format!("l{i}\r\n"))).collect();
        s.feed(text.as_bytes());
        assert!(s.term().mode().contains(alacritty_terminal::term::TermMode::ALT_SCREEN));
    }

    #[test]
    fn profile_setting_needs_a_check_box_key() {
        let mut s = one("go", "iTermSetProfileBooleanTrigger", "Triggers\u{1}1", b"go\r\n");
        assert!(s.take_requests().is_empty(), "not a check box setting");
    }

    fn trigger_requests(s: &mut Screen) -> Vec<TriggerRequest> {
        s.take_requests().into_iter().filter_map(|r| match r {
            Request::Trigger(t) => Some(t),
            _ => None,
        }).collect()
    }

    #[test]
    fn capture_follows_iterm2() {
        let mut s = Screen::new(40, 10);
        s.set_triggers(vec![trig(r"error: (\w+)", "CaptureTrigger", "run \\1", false)], TriggerPrefs::default());
        // No command mark: nothing is stored, and the request says shell integration is needed.
        s.feed(b"error: x\r\n");
        assert_eq!(trigger_requests(&mut s), vec![TriggerRequest::Captured { needs_shell_integration: true }]);
        assert!(s.last_command_mark().is_none());

        // A prompt and a command: output starts, so there is a command mark to store on. The
        // match is only part of the line, so `values[0]` (the whole match) differs from `line`
        // (the whole line).
        s.feed(b"\x1b]133;A\x07$ \x1b]133;B\x07cmd\r\n\x1b]133;C\x07");
        s.feed(b"see error: x here\r\n");
        let mark_id = s.last_command_mark().unwrap().id;
        let captured = s.last_command_mark().unwrap().captured.clone();
        assert_eq!(captured.len(), 1);
        assert_eq!(captured[0].line, "see error: x here");
        assert_eq!(captured[0].values, vec!["error: x".to_string(), "x".to_string()], "group 0 (the match), then the numbered groups");
        assert_eq!(captured[0].command.as_deref(), Some("run x"));
        assert_eq!(captured[0].mark, mark_id);
        assert!(!captured[0].state);
        assert_eq!(trigger_requests(&mut s), vec![TriggerRequest::Captured { needs_shell_integration: false }]);

        // A different line appends (the merge rule itself is `captured::add`'s job, tested there).
        s.feed(b"error: q\r\n");
        let captured = s.last_command_mark().unwrap().captured.clone();
        assert_eq!(captured.len(), 2, "a different line appends");
        assert_eq!(captured[1].line, "error: q");
        assert_eq!(captured[1].values, vec!["error: q".to_string(), "q".to_string()]);

        // Two matches on one line: only one fire (CaptureTrigger.m:65 returns NO, stopping that
        // trigger's own match enumeration, not the trigger list).
        s.feed(b"error: a error: b\r\n");
        let captured = s.last_command_mark().unwrap().captured.clone();
        assert_eq!(captured.len(), 3, "one entry, not two, for the two matches on this line");
        assert_eq!(captured[2].line, "error: a error: b");
        assert_eq!(captured[2].values, vec!["error: a".to_string(), "a".to_string()], "the first match only");
    }
}
