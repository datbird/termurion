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

#[cfg(test)]
mod tests {
    use super::*;

    fn row(v: &str) -> Row {
        Row { shown: String::new(), value: v.into() }
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
        let mut empty = List::new(Kind::Autocomplete, Query::default(), vec![]);
        assert_eq!(empty.key(Key::Enter), Reply::Close);
    }
}
