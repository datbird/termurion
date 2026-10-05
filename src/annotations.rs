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
    /// The id in the row's note slot: the row's first note, shared by later notes there.
    pub row_key: u32,
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
        if self.term().mode().contains(alacritty_terminal::term::TermMode::ALT_SCREEN)
            || line < -(grid.history_size() as i32)
            || line >= grid.screen_lines() as i32
        {
            return 0;
        }
        self.bump_ticks();
        let existing = self.term().primary_grid()[Line(line)].note();
        self.notes.next += 1;
        let id = self.notes.next;
        let row_key = if existing != 0 && self.notes.by_id.values().any(|n| n.row_key == existing) {
            existing
        } else {
            self.term_mut().primary_grid_mut()[Line(line)].set_note(id);
            id
        };
        let note = Note { text: text.to_string(), start_col, more_rows: (end_line - line).max(0) as usize, end_col, hidden, row_key };
        self.notes.by_id.insert(id, note);
        id
    }

    /// Notes on the main screen, top first, with their grid line.
    pub fn notes_in_order(&self) -> Vec<(i32, u32, &Note)> {
        let grid = self.term().primary_grid();
        let top = -(grid.history_size() as i32);
        (top..grid.screen_lines() as i32)
            .filter(|l| grid[Line(*l)].note() != 0)
            .flat_map(|l| {
                let key = grid[Line(l)].note();
                self.notes.by_id.iter().filter(move |(_, n)| n.row_key == key).map(move |(id, n)| (l, *id, n))
            })
            .collect()
    }

    pub fn remove_note(&mut self, id: u32) {
        let Some(note) = self.notes.by_id.remove(&id) else { return };
        self.bump_ticks();
        let key = note.row_key;
        // The row keeps its slot while other notes there share it.
        if self.notes.by_id.values().any(|n| n.row_key == key) {
            return;
        }
        let grid = self.term_mut().primary_grid_mut();
        let top = -(grid.history_size() as i32);
        for l in top..grid.screen_lines() as i32 {
            if grid[Line(l)].note() == key {
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

    #[test]
    fn a_row_keeps_several_notes() {
        let mut s = Screen::new(20, 4);
        s.feed(b"one two three");
        let a = s.add_note(0, 0, 0, 2, "first", false);
        let b = s.add_note(0, 4, 0, 6, "second", false);
        let texts: Vec<&str> = s.notes_in_order().iter().map(|(_, _, n)| n.text.as_str()).collect();
        assert_eq!(texts, vec!["first", "second"]);
        s.remove_note(a);
        assert_eq!(s.notes_in_order().len(), 1);
        s.remove_note(b);
        assert!(s.notes_in_order().is_empty());
    }

    #[test]
    fn a_huge_annotation_length_stops_at_the_screen() {
        let mut s = Screen::new(20, 4);
        s.feed(b"x\x1b]1337;AddAnnotation=160000000000|big\x07");
        s.feed(b"\x1b]1337;AddAnnotation=18446744073709551615|max\x07");
        for (_, _, n) in s.notes_in_order() {
            assert!(n.more_rows < 4, "{n:?}");
        }
    }
}
