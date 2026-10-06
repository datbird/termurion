//! The completion list drawn at the cursor: Open Autocomplete (iTerm2's Autocomplete keys:
//! Enter inserts, Tab is "more", Shift+Tab "less") and Auto Command Completion (iTerm2's
//! CommandHistoryPopup in auto mode: Enter runs what was typed, Shift+Enter inserts the
//! rest, Alt+Enter inserts it and runs it). The terminal keeps the keyboard; keys come here
//! first while the list shows.

use crate::autocomplete::Query;

#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    Autocomplete,
    /// Commands starting with what is typed at the prompt.
    Commands { typed: String },
    /// The prompt path navigator (13r): the folders around `base`, picked with Enter.
    Paths { base: String },
}

/// One row: what is shown before the value (the prefix), and the value.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub shown: String,
    pub value: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Up,
    Down,
    Enter,
    ShiftEnter,
    AltEnter,
    Tab,
    ShiftTab,
    Esc,
    Other,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Reply {
    /// Type this and close.
    Insert(String),
    /// Type this, then Enter, and close.
    InsertAndRun(String),
    /// A navigator row was picked: its path.
    Navigate(String),
    /// Search again with the list's query (Tab, Shift+Tab).
    Search,
    Close,
    /// Close; the key goes to the terminal.
    CloseAndPass,
    /// The key goes to the terminal; the list stays.
    Pass,
    Handled,
}

#[derive(Clone, Debug, PartialEq)]
pub struct List {
    pub kind: Kind,
    pub rows: Vec<Row>,
    pub selected: usize,
    /// Autocomplete: the query, text chosen with Tab so far, and the states Shift+Tab
    /// goes back to.
    pub query: Query,
    pub more: String,
    stack: Vec<(String, Query)>,
}

impl List {
    pub fn new(kind: Kind, query: Query, rows: Vec<Row>) -> List {
        List { kind, rows, selected: 0, query, more: String::new(), stack: Vec::new() }
    }

    pub fn key(&mut self, key: Key) -> Reply {
        match key {
            Key::Up => {
                self.selected = self.selected.saturating_sub(1);
                return Reply::Handled;
            }
            Key::Down => {
                if self.selected + 1 < self.rows.len() {
                    self.selected += 1;
                }
                return Reply::Handled;
            }
            Key::Esc => return Reply::Close,
            _ => {}
        }
        let chosen = self.rows.get(self.selected).map(|r| r.value.clone());
        match &self.kind {
            Kind::Autocomplete => match key {
                Key::Enter | Key::ShiftEnter | Key::AltEnter => match chosen {
                    Some(v) => Reply::Insert(format!("{}{v}", self.more)),
                    None => Reply::Close,
                },
                Key::Tab => {
                    let Some(v) = chosen else { return Reply::Handled };
                    self.stack.push((self.more.clone(), self.query.clone()));
                    self.more.push_str(&v);
                    if self.query.whitespace_before {
                        self.query.prefix.push(' ');
                        self.query.whitespace_before = false;
                    }
                    self.query.prefix.push_str(&v);
                    self.selected = 0;
                    Reply::Search
                }
                Key::ShiftTab => match self.stack.pop() {
                    Some((more, query)) => {
                        self.more = more;
                        self.query = query;
                        self.selected = 0;
                        Reply::Search
                    }
                    None => Reply::Handled,
                },
                _ => Reply::CloseAndPass,
            },
            Kind::Paths { .. } => match (key, chosen) {
                (Key::Enter, Some(v)) if !v.is_empty() => Reply::Navigate(v),
                (Key::Enter, _) => Reply::Close,
                _ => Reply::CloseAndPass,
            },
            Kind::Commands { typed } => {
                let rest = chosen.map(|v| crate::shell_history::remainder(&v, typed));
                match (key, rest) {
                    (Key::Enter, _) => Reply::CloseAndPass,
                    (Key::ShiftEnter, Some(r)) => Reply::Insert(r),
                    (Key::AltEnter, Some(r)) => Reply::InsertAndRun(r),
                    (Key::ShiftEnter | Key::AltEnter, None) => Reply::Close,
                    _ => Reply::Pass,
                }
            }
        }
    }
}

/// Auto Command Completion's timing (iTerm2's PseudoTerminal auto command history): the
/// first showing waits 0.2 s after typing stops; while it shows it follows every change at
/// once; a closed list stays closed until the typed text changes; a text already checked
/// with no match is not looked up again.
#[derive(Clone, Debug, Default)]
pub struct Auto {
    shown_for: Option<String>,
    dismissed_for: Option<String>,
    checked: Option<String>,
    pending: Option<(String, f64)>,
}

/// What the app does now.
#[derive(Clone, Debug, PartialEq)]
pub enum AutoDo {
    Nothing,
    /// Look up and show (or update) the list for this typed text.
    Show(String),
    Hide,
    /// Check again at this time.
    WakeAt(f64),
}

/// Auto Command Completion's first delay (iTerm2's 0.2 s).
pub const AUTO_DELAY: f64 = 0.2;

impl Auto {
    /// One check: `typed` at the prompt (None: not at a prompt), `other`: another list
    /// (Autocomplete) shows.
    pub fn step(&mut self, typed: Option<&str>, now: f64, other: bool) -> AutoDo {
        let Some(t) = typed.filter(|t| !t.is_empty()) else {
            self.dismissed_for = None;
            self.checked = None;
            self.pending = None;
            return if self.shown_for.take().is_some() { AutoDo::Hide } else { AutoDo::Nothing };
        };
        if other || self.dismissed_for.as_deref() == Some(t) {
            return AutoDo::Nothing;
        }
        self.dismissed_for = None;
        if let Some(s) = &self.shown_for {
            return if s == t { AutoDo::Nothing } else { AutoDo::Show(t.to_string()) };
        }
        if self.checked.as_deref() == Some(t) {
            return AutoDo::Nothing;
        }
        match &self.pending {
            Some((p, due)) if p == t => {
                if now >= *due {
                    self.pending = None;
                    AutoDo::Show(t.to_string())
                } else {
                    AutoDo::WakeAt(*due)
                }
            }
            _ => {
                self.pending = Some((t.to_string(), now + AUTO_DELAY));
                AutoDo::WakeAt(now + AUTO_DELAY)
            }
        }
    }

    /// The list showed for `typed`, or nothing matched it.
    pub fn showed(&mut self, typed: &str, any: bool) {
        if any {
            self.shown_for = Some(typed.to_string());
            self.checked = None;
        } else {
            self.shown_for = None;
            self.checked = Some(typed.to_string());
        }
    }

    /// The list was closed (Esc, Enter, an insert) while `typed` was typed.
    pub fn dismissed(&mut self, typed: &str) {
        self.shown_for = None;
        self.dismissed_for = Some(typed.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(v: &str) -> Row {
        Row { shown: String::new(), value: v.into() }
    }

    #[test]
    fn auto_completion_timing_follows_iterm2() {
        let mut a = Auto::default();
        let wake = |d: AutoDo, t: f64| matches!(d, AutoDo::WakeAt(x) if (x - t).abs() < 1e-9);
        assert!(wake(a.step(Some("gi"), 0.0, false), 0.2), "the first showing waits");
        assert!(wake(a.step(Some("gi"), 0.1, false), 0.2));
        assert!(wake(a.step(Some("git"), 0.15, false), 0.35), "typing restarts the wait");
        assert_eq!(a.step(Some("git"), 0.36, false), AutoDo::Show("git".into()));
        a.showed("git", true);
        assert_eq!(a.step(Some("git"), 0.4, false), AutoDo::Nothing);
        assert_eq!(a.step(Some("git "), 0.45, false), AutoDo::Show("git ".into()), "while it shows it follows at once");
        a.showed("git ", true);
        a.dismissed("git ");
        assert_eq!(a.step(Some("git "), 1.0, false), AutoDo::Nothing, "closed stays closed");
        assert!(wake(a.step(Some("git s"), 1.1, false), 1.3), "until the text changes");
        assert_eq!(a.step(Some("git s"), 1.31, false), AutoDo::Show("git s".into()));
        a.showed("git s", false);
        assert_eq!(a.step(Some("git s"), 2.0, false), AutoDo::Nothing, "no match: not looked up again");
        assert_eq!(a.step(Some("git st"), 2.0, true), AutoDo::Nothing, "Autocomplete is open");
        a.showed("git st", true);
        assert_eq!(a.step(None, 3.0, false), AutoDo::Hide, "left the prompt");
        assert_eq!(a.step(None, 3.1, false), AutoDo::Nothing);
    }

    #[test]
    fn completion_keys_follow_iterm2() {
        let q = Query { prefix: "sta".into(), context: vec![], whitespace_before: false };
        let mut a = List::new(Kind::Autocomplete, q.clone(), vec![row("tus"), row("tic")]);
        assert_eq!(a.key(Key::Down), Reply::Handled);
        assert_eq!(a.selected, 1);
        assert_eq!(a.key(Key::Up), Reply::Handled);
        assert_eq!(a.key(Key::Tab), Reply::Search, "more");
        assert_eq!((a.query.prefix.as_str(), a.more.as_str()), ("status", "tus"));
        a.rows = vec![row(" -s")];
        assert_eq!(a.key(Key::Enter), Reply::Insert("tus -s".into()), "the Tab text, then the choice");
        assert_eq!(a.key(Key::ShiftTab), Reply::Search, "less");
        assert_eq!((a.query.clone(), a.more.as_str()), (q.clone(), ""));
        assert_eq!(a.key(Key::ShiftTab), Reply::Handled, "nothing to go back to");
        assert_eq!(a.key(Key::Esc), Reply::Close);
        assert_eq!(a.key(Key::Other), Reply::CloseAndPass);
        let ws = Query { prefix: "git".into(), context: vec![], whitespace_before: true };
        let mut b = List::new(Kind::Autocomplete, ws, vec![row("status")]);
        b.key(Key::Tab);
        assert_eq!((b.query.prefix.as_str(), b.query.whitespace_before), ("git status", false), "a space joins them");
        let mut c = List::new(Kind::Commands { typed: "gi".into() }, Query::default(), vec![row("git status")]);
        assert_eq!(c.key(Key::Enter), Reply::CloseAndPass, "Enter runs what was typed");
        assert_eq!(c.key(Key::ShiftEnter), Reply::Insert("t status".into()));
        assert_eq!(c.key(Key::AltEnter), Reply::InsertAndRun("t status".into()));
        assert_eq!(c.key(Key::Tab), Reply::Pass, "Tab goes to the shell");
        assert_eq!(c.key(Key::Other), Reply::Pass, "typing goes on; the list follows it");
        assert_eq!(c.key(Key::Esc), Reply::Close);
        let mut nav = List::new(Kind::Paths { base: "C:\\a".into() }, Query::default(), vec![row("C:\\"), row("C:\\a\\b")]);
        assert_eq!(nav.key(Key::Down), Reply::Handled);
        assert_eq!(nav.key(Key::Enter), Reply::Navigate("C:\\a\\b".into()));
        assert_eq!(nav.key(Key::Tab), Reply::CloseAndPass);
        assert_eq!(nav.key(Key::Esc), Reply::Close);
        let mut waiting = List::new(Kind::Paths { base: "C:\\a".into() }, Query::default(), vec![row("")]);
        assert_eq!(waiting.key(Key::Enter), Reply::Close, "the placeholder row picks nothing");
        let mut empty = List::new(Kind::Autocomplete, Query::default(), vec![]);
        assert_eq!(empty.key(Key::Enter), Reply::Close);
    }
}
