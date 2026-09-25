//! iTerm2's annotations: a note on a run of text. The note's id sits on its first row (a
//! row slot), so the note moves with its text through scrolling and reflow.

use std::collections::BTreeMap;

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::Line;

use crate::screen::Screen;

#[derive(Clone, Debug, PartialEq)]
pub struct Note {
    pub text: String,
    /// The first annotated column on the first row.
    pub start_col: usize,
    /// Rows after the first that the note covers, and the last column on its last row.
    pub more_rows: usize,
    pub end_col: usize,
    pub hidden: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Notes {
    pub by_id: BTreeMap<u32, Note>,
    next: u32,
}

/// OSC 1337 AddAnnotation's value: "message", "length|message", or
/// "message|length|x|y". Returns the message, the length, and the screen cell.
pub fn parse_osc(value: &str) -> Option<(String, Option<usize>, Option<(usize, usize)>)> {
    if value.is_empty() {
        return None;
    }
    let parts: Vec<&str> = value.split('|').collect();
    let n = |s: &str| s.trim().parse::<usize>().ok();
    Some(match parts.as_slice() {
        [len, msg] if n(len).is_some() => (msg.to_string(), n(len), None),
        [msg, len, x, y] if n(len).is_some() && n(x).is_some() && n(y).is_some() => {
            (msg.to_string(), n(len), n(x).zip(n(y)))
        }
        _ => (value.to_string(), None, None),
    })
}

/// The next note row after (or before) `from`, wrapping at the ends.
pub fn next_note(rows: &[i32], from: i32, forward: bool) -> Option<i32> {
    let mut sorted = rows.to_vec();
    sorted.sort_unstable();
    if forward {
        sorted.iter().find(|r| **r > from).or(sorted.first()).copied()
    } else {
        sorted.iter().rev().find(|r| **r < from).or(sorted.last()).copied()
    }
}

impl Screen {
    /// Adds a note over grid lines and columns, from `(line, start_col)` to `(end_line,
    /// end_col)`, on the main screen. A row holds one note: a new one there replaces it.
    pub fn add_note(&mut self, line: i32, start_col: usize, end_line: i32, end_col: usize, text: &str, hidden: bool) -> u32 {
        let grid = self.term().primary_grid();
        if line < -(grid.history_size() as i32) || line >= grid.screen_lines() as i32 {
            return 0;
        }
        self.notes.next += 1;
        let id = self.notes.next;
        let grid = self.term_mut().primary_grid_mut();
        let old = grid[Line(line)].note();
        grid[Line(line)].set_note(id);
        self.notes.by_id.remove(&old);
        let note = Note { text: text.to_string(), start_col, more_rows: (end_line - line).max(0) as usize, end_col, hidden };
        self.notes.by_id.insert(id, note);
        id
    }

    /// Notes on the main screen, top first, with their grid line.
    pub fn notes_in_order(&self) -> Vec<(i32, u32, &Note)> {
        let grid = self.term().primary_grid();
        let top = -(grid.history_size() as i32);
        (top..grid.screen_lines() as i32)
            .filter_map(|l| {
                let id = grid[Line(l)].note();
                self.notes.by_id.get(&id).map(|n| (l, id, n))
            })
            .collect()
    }

    pub fn remove_note(&mut self, id: u32) {
        self.notes.by_id.remove(&id);
        let grid = self.term_mut().primary_grid_mut();
        let top = -(grid.history_size() as i32);
        for l in top..grid.screen_lines() as i32 {
            if grid[Line(l)].note() == id {
                grid[Line(l)].set_note(0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn annotations_follow_their_rows() {
        let mut s = Screen::with_scrollback(20, 4, 100);
        s.feed(b"a\r\nhello world\r\nc");
        let id = s.add_note(1, 6, 1, 10, "a planet", false);
        for i in 0..6 {
            s.feed(format!("\r\nout{i}").as_bytes());
        }
        let notes = s.notes_in_order();
        assert_eq!(notes.len(), 1);
        let (line, nid, note) = notes[0];
        assert_eq!((nid, note.text.as_str(), note.start_col, note.end_col), (id, "a planet", 6, 10));
        assert_eq!(s.line_text(line), "hello world");
        s.remove_note(id);
        assert!(s.notes_in_order().is_empty());
    }

    #[test]
    fn osc_add_annotation_forms() {
        assert_eq!(parse_osc("hi there"), Some(("hi there".into(), None, None)));
        assert_eq!(parse_osc("5|hi"), Some(("hi".into(), Some(5), None)));
        assert_eq!(parse_osc("hi|3|4|2"), Some(("hi".into(), Some(3), Some((4, 2)))));
        assert_eq!(parse_osc(""), None);
        let mut s = Screen::new(20, 4);
        s.feed(b"abc\x1b]1337;AddAnnotation=4|note\x07def");
        let notes = s.notes_in_order();
        assert_eq!(notes.len(), 1);
        assert_eq!((notes[0].2.start_col, notes[0].2.end_col, notes[0].2.hidden), (3, 6, false));
        s.feed(b"\r\n\x1b]1337;AddHiddenAnnotation=quiet\x07");
        assert!(s.notes_in_order().iter().any(|(_, _, n)| n.hidden && n.text == "quiet"));
    }

    #[test]
    fn next_annotation_wraps() {
        let rows = [-5, 2, 7];
        assert_eq!(next_note(&rows, 2, true), Some(7));
        assert_eq!(next_note(&rows, 7, true), Some(-5));
        assert_eq!(next_note(&rows, -5, false), Some(7));
        assert_eq!(next_note(&rows, 3, false), Some(2));
        assert_eq!(next_note(&[], 0, true), None);
    }
}
