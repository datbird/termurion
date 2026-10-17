//! The screen reader's text model (13v2 Task 8): what Narrator and NVDA read through UI
//! Automation. Pure: the grid is copied into `Rows` (`Screen::access_rows`), `build` turns
//! them into UTF-16 text with the cell behind every unit, and `Tracker` decides what to
//! announce, as iTerm2's accessibility helper (`iTermTextViewAccessibilityHelper.m`) and
//! `PTYTextView` (`refreshAccessibility`) do for VoiceOver.
//!
//! Everything here is bounded: a snapshot holds at most `MAX_LINES` rows and `MAX_UNITS`
//! units, a move never loops over its count, and a pane at a password prompt exposes only
//! the prompt (`secure_rows`).

use std::collections::HashMap;
use std::time::{Duration, Instant};

pub type PaneId = crate::layout::PaneId;

/// The most rows a snapshot exposes, whatever `NumberOfLinesForAccessibility` says.
pub const MAX_LINES: usize = 10_000;
/// The most UTF-16 units a snapshot holds; older rows go first.
pub const MAX_UNITS: usize = 4_000_000;
/// An announcement reads at most this many lines (`PTYTextView.m:3270`).
pub const MAX_ANNOUNCE_LINES: i64 = 256;
/// How long output must stop before it is announced (`PTYTextView.m:3361`).
pub const SETTLE: Duration = Duration::from_millis(150);

/// `NumberOfLinesForAccessibility` as the number of rows to expose: 1 to `MAX_LINES`, and a
/// value of zero or less (P16) reads as the default 1000.
pub fn clamp_lines(n: i64) -> usize {
    if n <= 0 {
        return 1000;
    }
    n.min(MAX_LINES as i64) as usize
}

/// The grid as the model reads it: each row's cells as their characters (an empty string for a
/// wide character's spacer cell), whether the row wraps onto the next, the cursor's row and
/// column in `rows`, and the first row's absolute line (history first, from 0).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Rows {
    pub rows: Vec<(Vec<String>, bool)>,
    pub cursor: (usize, usize),
    pub first_line: i64,
}

fn units_of(cells: &[String]) -> usize {
    cells.iter().map(|c| c.encode_utf16().count()).sum::<usize>() + 1
}

impl Rows {
    /// The newest rows holding at most `max_units` units (one extra for each row's newline); a
    /// row is never split, the newest is kept even when it alone is over.
    pub fn capped(mut self, max_units: usize) -> Rows {
        let mut units = 0usize;
        let mut keep = 0usize;
        for (cells, _) in self.rows.iter().rev() {
            units += units_of(cells);
            if units > max_units && keep > 0 {
                break;
            }
            keep += 1;
        }
        let drop = self.rows.len() - keep;
        if drop > 0 {
            self.rows.drain(..drop);
            self.cursor.0 = self.cursor.0.saturating_sub(drop);
            self.first_line += drop as i64;
        }
        self
    }
}

/// A pane at a password prompt: only the prompt. `input_start` is where the prompt ended, as
/// (absolute line, column), recorded when the pane reached the prompt; the model exposes that
/// row up to that column and nothing else, so a program that echoes what is typed never exposes
/// it, even when the echo wraps onto later rows (the cursor is then on a later row). Without a
/// recorded start, or when its row is no longer in `rows`, nothing is exposed: unknown means
/// hidden. The cursor shows at the end of the prompt.
pub fn secure_rows(rows: &Rows, input_start: Option<(i64, usize)>) -> Rows {
    let Some((line, col)) = input_start else { return Rows::default() };
    let Ok(idx) = usize::try_from(line - rows.first_line) else { return Rows::default() };
    let Some((cells, _)) = rows.rows.get(idx) else { return Rows::default() };
    let cut = col.min(cells.len());
    Rows { rows: vec![(cells[..cut].to_vec(), false)], cursor: (0, cut), first_line: line }
}

/// The text a client reads: UTF-16 units, the offset after each row, and the (row, column)
/// of the cell behind each unit (every unit of a surrogate pair or a combining sequence
/// shares its cell; a row's newline is at the column its text ends).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Text {
    pub text: Vec<u16>,
    pub breaks: Vec<usize>,
    pub cells: Vec<(usize, usize)>,
}

fn private_use(c: char) -> bool {
    ('\u{e000}'..='\u{f8ff}').contains(&c)
}

fn blank(s: &str) -> bool {
    s.chars().all(|c| c == ' ' || c == '\0' || private_use(c))
}

/// `iTermTextViewAccessibilityHelper.m:246-319`: each row's characters up to its last
/// non-empty cell (a wrapped row keeps its spaces: they are text), a newline after a row that
/// does not wrap, private-use characters as nothing, then the trailing rows after the cursor's
/// row that hold nothing removed with their newlines and breaks.
pub fn build(rows: &Rows) -> Text {
    let mut keep = rows.rows.len();
    if keep == 0 {
        return Text::default();
    }
    let cursor_row = rows.cursor.0.min(keep - 1);
    while keep > cursor_row + 1 && rows.rows[keep - 1].0.iter().all(|c| blank(c)) {
        keep -= 1;
    }
    let mut t = Text::default();
    for (r, (cells, wraps)) in rows.rows[..keep].iter().enumerate() {
        let end = if *wraps { cells.len() } else { cells.iter().rposition(|c| !blank(c)).map_or(0, |i| i + 1) };
        for (c, cell) in cells[..end].iter().enumerate() {
            for ch in cell.chars().filter(|ch| !private_use(*ch)) {
                let mut buf = [0u16; 2];
                for u in ch.encode_utf16(&mut buf) {
                    t.text.push(*u);
                    t.cells.push((r, c));
                }
            }
        }
        if !*wraps {
            t.text.push(b'\n' as u16);
            t.cells.push((r, end));
        }
        t.breaks.push(t.text.len());
    }
    t
}

fn is_lead(u: u16) -> bool {
    (0xD800..0xDC00).contains(&u)
}

fn is_trail(u: u16) -> bool {
    (0xDC00..0xE000).contains(&u)
}

/// The offset of the first unit at or after (`row`, `col`) in its row; past the row's text the
/// row's end (its newline), past the last row the end of the text.
pub fn offset_of(t: &Text, row: usize, col: usize) -> usize {
    if row >= t.breaks.len() {
        return t.text.len();
    }
    let start = if row == 0 { 0 } else { t.breaks[row - 1] };
    let end = t.breaks[row];
    let slice = &t.cells[start..end];
    let i = slice.partition_point(|&(_, c)| c < col);
    if i < slice.len() {
        start + i
    } else if end > start && t.text[end - 1] == b'\n' as u16 {
        end - 1
    } else {
        end
    }
}

/// The cell behind `offset`; at or past the end column 0 of the row after the last one.
pub fn cell_of(t: &Text, offset: usize) -> (usize, usize) {
    match t.cells.get(offset) {
        Some(c) => *c,
        None => (t.breaks.len(), 0),
    }
}

/// The row holding `offset`; the last row for the end of the text.
pub fn line_of(t: &Text, offset: usize) -> usize {
    t.breaks.partition_point(|&b| b <= offset).min(t.breaks.len().saturating_sub(1))
}

/// The offsets a row covers (its newline included).
pub fn line_range(t: &Text, line: usize) -> (usize, usize) {
    let Some(&end) = t.breaks.get(line) else { return (t.text.len(), t.text.len()) };
    (if line == 0 { 0 } else { t.breaks[line - 1] }, end)
}

/// UI Automation's text units: Format maps to Word, Paragraph to Line, Page to Document.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unit {
    Character,
    Word,
    Line,
    Document,
}

fn space(u: u16) -> bool {
    char::from_u32(u as u32).is_some_and(char::is_whitespace)
}

/// Where each unit's ranges start, ascending, ending with the text's length (empty text: just 0).
/// Not for characters (see `step`).
fn starts(t: &Text, unit: Unit) -> Vec<usize> {
    let len = t.text.len();
    let mut s = Vec::new();
    match unit {
        Unit::Character | Unit::Document => s.push(0),
        Unit::Line => {
            s.push(0);
            s.extend(t.breaks.iter().take(t.breaks.len().saturating_sub(1)).copied());
        }
        // A word is a run of non-space characters and the spaces after it (a newline included);
        // a row's first word starts after the newline.
        Unit::Word => {
            for i in 0..len {
                if i == 0 || (!space(t.text[i]) && space(t.text[i - 1])) {
                    s.push(i);
                }
            }
            if s.is_empty() {
                s.push(0);
            }
        }
    }
    if len > 0 {
        s.dedup();
        if s.last() != Some(&len) {
            s.push(len);
        }
    }
    s
}

/// The next character boundary after `o` (never inside a surrogate pair).
fn next_char(t: &Text, o: usize) -> usize {
    if o >= t.text.len() {
        return t.text.len();
    }
    if is_lead(t.text[o]) && o + 1 < t.text.len() && is_trail(t.text[o + 1]) {
        o + 2
    } else {
        o + 1
    }
}

fn prev_char(t: &Text, o: usize) -> usize {
    if o == 0 {
        return 0;
    }
    let p = o.min(t.text.len()) - 1;
    if is_trail(t.text[p]) && p > 0 && is_lead(t.text[p - 1]) {
        p - 1
    } else {
        p
    }
}

/// The range of the `unit` holding `offset`.
pub fn expand(t: &Text, offset: usize, unit: Unit) -> (usize, usize) {
    let len = t.text.len();
    let offset = offset.min(len);
    match unit {
        Unit::Character => {
            let start = if offset > 0 && offset < len && is_trail(t.text[offset]) && is_lead(t.text[offset - 1]) { offset - 1 } else { offset };
            (start, next_char(t, start))
        }
        Unit::Document => (0, len),
        Unit::Line => line_range(t, line_of(t, offset)),
        Unit::Word => {
            let s = starts(t, unit);
            if len == 0 {
                return (0, 0);
            }
            let i = s.partition_point(|&x| x <= offset).saturating_sub(1).min(s.len() - 2);
            (s[i], s[i + 1])
        }
    }
}

/// Moves `offset` by `count` units (to the start of a unit; to the end of the text past the
/// last): the new offset and how many units it moved, negative backwards. Bounded by the text,
/// never by `count`.
pub fn move_by(t: &Text, offset: usize, unit: Unit, count: i32) -> (usize, i32) {
    let len = t.text.len();
    let offset = offset.min(len);
    if count == 0 || len == 0 {
        return (offset, 0);
    }
    if unit == Unit::Character {
        let (mut o, mut moved) = (offset, 0i32);
        if count > 0 {
            while moved < count && o < len {
                o = next_char(t, o);
                moved += 1;
            }
        } else {
            while moved > count && o > 0 {
                o = prev_char(t, o);
                moved -= 1;
            }
        }
        return (o, moved);
    }
    let s = starts(t, unit);
    let last = s.len() - 1;
    let here = if offset >= len { last } else { s.partition_point(|&x| x <= offset) - 1 };
    let target = (here as i64 + count as i64).clamp(0, last as i64) as usize;
    let moved = target as i64 - here as i64;
    if moved == 0 {
        return (offset, 0);
    }
    (s[target], moved as i32)
}

/// Spaces and NULs off both ends (`NSCharacterSet+iTerm.m:1745-1752`).
pub fn trimmed(s: &str) -> String {
    s.trim_matches(|c| c == ' ' || c == '\0').to_string()
}

/// The lines to read after a burst (`PTYTextView.m:3208-3228`): the line at the old cursor's
/// place is the echoed command, dropped when it is still what the cursor's line was; blank
/// lines are dropped.
pub fn announcement_lines(lines: &[String], first_abs: i64, old_cursor_abs: i64, old_cursor_line: &str) -> Vec<String> {
    let old = trimmed(old_cursor_line);
    let cmd = old_cursor_abs - first_abs;
    lines
        .iter()
        .enumerate()
        .filter_map(|(i, l)| {
            let l = trimmed(l);
            if cmd >= 0 && cmd == i as i64 && !old.is_empty() && l == old {
                return None;
            }
            (!l.is_empty()).then_some(l)
        })
        .collect()
}

fn joined(pieces: &[(usize, String)], from: usize, to: usize) -> String {
    pieces.iter().filter(|(x, _)| *x >= from && *x < to).map(|(_, s)| s.as_str()).collect()
}

fn from_col(pieces: &[(usize, String)], x: usize) -> Option<usize> {
    pieces.iter().position(|(c, _)| *c >= x)
}

fn rest(pieces: &[(usize, String)], idx: Option<usize>) -> String {
    idx.map_or_else(String::new, |i| pieces[i..].iter().map(|(_, s)| s.as_str()).collect())
}

fn blanks(c: char) -> bool {
    c == ' ' || c == '\t'
}

/// What a deletion on the cursor's line removed (`PTYTextView.swift:1079-1160`); the cursor
/// columns are 1-based as iTerm2's `cursorX`, each line a list of (column, cell text).
pub fn deletion(old_x: usize, new_x: usize, old: &[(usize, String)], new: &[(usize, String)]) -> Option<String> {
    let (old_g, new_g) = (old_x.saturating_sub(1), new_x.saturating_sub(1));
    if new_x < old_x {
        // The cursor moved left: Backspace, delete word, kill line.
        let was = joined(old, new_g, old_g);
        if !was.is_empty() && was != joined(new, new_g, old_g) {
            return Some(was);
        }
    } else if new_x == old_x {
        let old_s: String = old.iter().map(|(_, s)| s.as_str()).collect();
        let new_s: String = new.iter().map(|(_, s)| s.as_str()).collect();
        if old_s == new_s {
            return None;
        }
        let (oi, ni) = (from_col(old, new_g), from_col(new, new_g));
        let (old_after, new_after) = (rest(old, oi), rest(new, ni));
        let (ot, nt) = (old_after.trim_matches(blanks), new_after.trim_matches(blanks));
        if !ot.is_empty() && nt.is_empty() {
            return Some(ot.to_string());
        } else if !ot.is_empty() && !old_after.starts_with(&new_after) {
            // A forward delete: the character that was under the cursor.
            if let Some(i) = oi {
                let x = old[i].0;
                let ch = joined(old, x, x + 1);
                if !ch.is_empty() {
                    return Some(ch);
                }
            }
        }
    }
    None
}

/// What a client is told (`PTYTextView.m:3301-3400`): UIA's text-changed and
/// text-selection-changed events and the two announcements (`UiaRaiseNotificationEvent`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    TextChanged,
    SelectionChanged,
    Announce(String),
    AnnounceDeleted(String),
}

/// `refreshAccessibility` and its settle: fed the cursor on every refresh of a pane, it says
/// what changed. Cursor columns are 1-based, `cursor_abs` the cursor's absolute line.
#[derive(Clone, Debug, Default)]
pub struct Tracker {
    last_abs: Option<i64>,
    last_x: usize,
    last_line: Option<Vec<(usize, String)>>,
    /// The old cursor's absolute line and text where the burst began.
    pending: Option<(i64, String)>,
    deadline: Option<Instant>,
}

impl Tracker {
    pub fn refresh(&mut self, now: Instant, cursor_abs: i64, cursor_x: usize, cursor_line: &[(usize, String)]) -> Vec<Event> {
        let mut out = Vec::new();
        let old_abs = self.last_abs;
        // A deletion is announced before anything else.
        let mut deleted = false;
        if let (Some(last), Some(old)) = (&self.last_line, old_abs)
            && old == cursor_abs
                && let Some(d) = deletion(self.last_x, cursor_x, last, cursor_line) {
                    out.push(Event::AnnounceDeleted(d));
                    deleted = true;
                }
        let moved_down = !deleted && old_abs.is_some_and(|o| cursor_abs > o);
        let mut pending = self.pending.is_some();
        // Moving up while output is pending is someone reviewing history: cancel.
        if pending && old_abs.is_some_and(|o| cursor_abs < o) {
            self.pending = None;
            self.deadline = None;
            pending = false;
        }
        if moved_down
            && let Some(old) = old_abs {
                if !pending {
                    let text = self.last_line.as_ref().map(|l| l.iter().map(|(_, s)| s.as_str()).collect()).unwrap_or_default();
                    self.pending = Some((old, text));
                }
                self.deadline = Some(now + SETTLE);
                pending = true;
            }
        if !deleted && !pending {
            out.push(Event::TextChanged);
        }
        if cursor_x != self.last_x || old_abs != Some(cursor_abs) {
            if !deleted && !pending {
                out.push(Event::SelectionChanged);
            }
            self.last_x = cursor_x;
            self.last_abs = Some(cursor_abs);
        }
        self.last_line = Some(cursor_line.to_vec());
        out
    }

    /// Reads the burst once output has stopped for `SETTLE`: `lines` is an absolute line's text
    /// (None when it is gone). The announcement is followed by the refresh the helper posts
    /// after it, so a client's cached text catches up after the speech.
    pub fn settle(&mut self, now: Instant, lines: &dyn Fn(i64) -> Option<String>, cursor_abs: i64) -> Vec<Event> {
        match self.deadline {
            Some(d) if now >= d && self.pending.is_some() => {}
            _ => return Vec::new(),
        }
        let (start, old_line) = self.pending.take().unwrap_or_default();
        self.deadline = None;
        let last = cursor_abs - 1;
        let first = if last - start + 1 > MAX_ANNOUNCE_LINES { last - MAX_ANNOUNCE_LINES + 1 } else { start };
        let got: Vec<String> = (first..=last).map(|a| trimmed(&lines(a).unwrap_or_default())).collect();
        let out_lines = announcement_lines(&got, first, start, &old_line);
        let mut out = Vec::new();
        if !out_lines.is_empty() {
            out.push(Event::Announce(out_lines.join("\n")));
        }
        out.push(Event::TextChanged);
        out.push(Event::SelectionChanged);
        out
    }

    /// When the pending burst settles (None with nothing pending): the next time `settle` has work.
    pub fn due(&self) -> Option<Instant> {
        self.deadline.filter(|_| self.pending.is_some())
    }

    /// The pane is at a password prompt: forget everything (no deletion is read from the
    /// prompt, no burst is pending) and raise nothing until the state clears.
    pub fn refresh_secure(&mut self) -> Vec<Event> {
        *self = Tracker::default();
        Vec::new()
    }
}

/// The 13v2 x 13w seam (no event storm): output whose built text, cursor and first line equal
/// the last published snapshot's (`prev`: its text, cursor offset and first absolute line) changed
/// nothing a client reads. A chunk of bells, a title or any other non-cell output moves the
/// screen's tick, and without this each publish (up to one per `BUILD_GAP`) would raise a
/// TextChanged for it.
pub fn output_unchanged(prev: Option<(&Text, usize, i64)>, rows: &Rows, text: &Text) -> bool {
    prev.is_some_and(|(t, cursor, first)| first == rows.first_line && t == text && cursor == offset_of(text, rows.cursor.0, rows.cursor.1))
}

/// What the provider's snapshot of a pane depends on besides its text, so a selection, a scroll,
/// a focus change or a move republishes (`tick` is `Screen::tick_digest`, `offset` the display
/// offset, `sel` the selection's ends as (line, column)).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Signal {
    pub tick: u64,
    pub sel: Option<((i32, usize), (i32, usize))>,
    pub offset: usize,
    pub focused: bool,
    pub rect: (i32, i32, i32, i32),
    pub cell: (u32, u32),
}

/// The panes whose signal differs from the last published one (or that are new).
pub fn changed_panes(last: &HashMap<PaneId, Signal>, now: &[(PaneId, Signal)]) -> Vec<PaneId> {
    now.iter().filter(|(id, s)| last.get(id) != Some(s)).map(|(id, _)| *id).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// One row from a string: each char a cell, a "~" the spacer cell after a wide character.
    fn row(s: &str, wraps: bool) -> (Vec<String>, bool) {
        (s.chars().map(|c| if c == '~' { String::new() } else { c.to_string() }).collect(), wraps)
    }

    fn rows(spec: &[(&str, bool)], cursor: (usize, usize)) -> Rows {
        Rows { rows: spec.iter().map(|(s, w)| row(s, *w)).collect(), cursor, first_line: 0 }
    }

    fn string(t: &Text) -> String {
        String::from_utf16(&t.text).unwrap()
    }

    #[test]
    fn text_follows_iterm2() {
        let r = rows(&[("ab", false), ("c ", false), ("de", true), ("f", false), ("", false), ("  ", false)], (3, 1));
        let t = build(&r);
        assert_eq!(string(&t), "ab\nc\ndef\n", "the wrapped row adds no newline, trailing blanks after the cursor row go");
        assert_eq!(t.breaks, vec![3, 5, 7, 9]);
        // Blank rows before the cursor's row stay.
        let t = build(&rows(&[("a", false), ("", false), ("b", false)], (2, 0)));
        assert_eq!(string(&t), "a\n\nb\n");
        // Rows after the cursor with text stay.
        let t = build(&rows(&[("$", false), ("out", false)], (0, 1)));
        assert_eq!(string(&t), "$\nout\n");
        // A private-use character adds nothing.
        let t = build(&rows(&[("a\u{e000}b\u{f8ff}", false)], (0, 0)));
        assert_eq!(string(&t), "ab\n");
        // A combining sequence adds all its units, at its one cell.
        let mut r = rows(&[("xy", false)], (0, 0));
        r.rows[0].0[0] = "e\u{301}".to_string();
        let t = build(&r);
        assert_eq!(t.text.len(), 4, "e, the accent, y and the newline");
        assert_eq!(&t.cells[..3], &[(0, 0), (0, 0), (0, 1)]);
        // A wide character's spacer cell adds nothing.
        let t = build(&rows(&[("\u{3042}~x", false)], (0, 0)));
        assert_eq!(string(&t), "\u{3042}x\n");
        assert_eq!(&t.cells[..2], &[(0, 0), (0, 2)]);
        // Nothing at all.
        assert_eq!(build(&Rows::default()), Text::default());
    }

    #[test]
    fn ranges_round_trip() {
        let t = build(&rows(&[("x\u{3042}~\u{1F600}~y", false), ("zz", false)], (1, 0)));
        let units = t.text.len();
        assert_eq!(units, 1 + 1 + 2 + 1 + 1 + 2 + 1);
        for o in 0..=units {
            let inside_pair = o > 0 && (0xDC00..0xE000).contains(&t.text.get(o).copied().unwrap_or(0)) && (0xD800..0xDC00).contains(&t.text[o - 1]);
            if inside_pair {
                continue;
            }
            let (r, c) = cell_of(&t, o);
            assert_eq!(offset_of(&t, r, c), o, "offset {o} is cell ({r}, {c})");
        }
        // The two units of a pair share their cell.
        assert_eq!(cell_of(&t, 2), cell_of(&t, 3));
        // A column past the row's text is the row's end; a row past the text is the end.
        assert_eq!(offset_of(&t, 0, 50), 5);
        assert_eq!(offset_of(&t, 9, 0), units);
        for o in 0..units {
            let line = line_of(&t, o);
            let (a, b) = line_range(&t, line);
            assert!(a <= o && o < b, "offset {o} in line {line} {a}..{b}");
        }
        assert_eq!(line_range(&t, 0), (0, t.breaks[0]));
        assert_eq!(line_range(&t, 1), (t.breaks[0], t.breaks[1]));
        assert_eq!(line_of(&t, units), 1, "the end belongs to the last line");
    }

    #[test]
    fn units_follow_uia() {
        let t = build(&rows(&[("hello world", false), ("foo", false)], (1, 0)));
        let s = string(&t);
        let word = |o: usize| {
            let (a, b) = expand(&t, o, Unit::Word);
            String::from_utf16(&t.text[a..b]).unwrap()
        };
        assert_eq!(word(8), "world\n");
        assert_eq!(word(1), "hello ");
        assert_eq!(word(12), "foo\n");
        assert_eq!(expand(&t, 8, Unit::Line), (0, 12));
        assert_eq!(expand(&t, 13, Unit::Line), (12, s.len()));
        assert_eq!(expand(&t, 8, Unit::Document), (0, s.len()));
        assert_eq!(expand(&t, 2, Unit::Character), (2, 3));
        assert_eq!(expand(&t, s.len(), Unit::Character), (s.len(), s.len()));
        // The word of a text with no newline after it is exactly the word.
        let t2 = Text { text: "hello world".encode_utf16().collect(), breaks: vec![11], cells: (0..11).map(|c| (0, c)).collect() };
        assert_eq!(expand(&t2, 8, Unit::Word), (6, 11));
        // Characters never split a surrogate pair.
        let p = build(&rows(&[("a\u{1F600}~b\u{1F600}~c", false)], (0, 0)));
        assert_eq!(move_by(&p, 0, Unit::Character, 3), (4, 3));
        assert_eq!(move_by(&p, 4, Unit::Character, -1), (3, -1));
        assert_eq!(move_by(&p, 1, Unit::Character, 1), (3, 1));
        assert_eq!(expand(&p, 2, Unit::Character), (1, 3), "the middle of a pair expands to the pair");
        // Words and lines.
        assert_eq!(move_by(&t, 0, Unit::Word, 1), (6, 1));
        assert_eq!(move_by(&t, 0, Unit::Word, 10), (s.len(), 3));
        assert_eq!(move_by(&t, 0, Unit::Line, 1), (12, 1));
        assert_eq!(move_by(&t, 0, Unit::Line, -1), (0, 0), "from the first line nothing moves");
        assert_eq!(move_by(&t, 13, Unit::Line, -1), (0, -1));
        assert_eq!(move_by(&t, 5, Unit::Document, 1), (s.len(), 1));
        assert_eq!(move_by(&t, 5, Unit::Character, 0), (5, 0));
    }

    #[test]
    fn move_by_huge_count_is_bounded() {
        let big = build(&Rows { rows: (0..2000).map(|_| row("abcdefghij", false)).collect(), cursor: (1999, 0), first_line: 0 });
        for unit in [Unit::Character, Unit::Word, Unit::Line, Unit::Document] {
            let (end, moved) = move_by(&big, 0, unit, i32::MAX);
            assert_eq!(end, big.text.len(), "{unit:?}");
            assert!(moved > 0);
            let (start, moved) = move_by(&big, end, unit, i32::MIN);
            assert_eq!(start, 0, "{unit:?}");
            assert!(moved < 0);
        }
    }

    #[test]
    fn announcements_follow_iterm2() {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(announcement_lines(&s(&["$ ls", "a", "", "b"]), 10, 10, "$ ls"), s(&["a", "b"]));
        assert_eq!(announcement_lines(&s(&["$ ls", "a", "", "b"]), 10, 20, "$ ls"), s(&["$ ls", "a", "b"]), "the command is outside the lines");
        assert_eq!(announcement_lines(&s(&["$ ls", "a"]), 10, 10, "$ other"), s(&["$ ls", "a"]), "a different line is not the echo");
        assert_eq!(trimmed("\0  a b \0 "), "a b");

        let t0 = Instant::now();
        let mut t = Tracker::default();
        let line = |s: &str| vec![(0usize, s.to_string())];
        assert_eq!(t.refresh(t0, 10, 5, &line("$ ls")), vec![Event::TextChanged, Event::SelectionChanged]);
        let ev = t.refresh(t0 + Duration::from_millis(10), 13, 1, &line("$ "));
        assert!(ev.is_empty(), "pending output posts nothing yet: {ev:?}");
        let text = |abs: i64| Some(match abs {
            10 => "$ ls".to_string(),
            11 => "a".to_string(),
            12 => "b".to_string(),
            _ => unreachable!("{abs}"),
        });
        assert!(t.settle(t0 + Duration::from_millis(100), &text, 13).is_empty(), "not before SETTLE");
        let ev = t.settle(t0 + Duration::from_millis(10) + SETTLE, &text, 13);
        assert_eq!(ev, vec![Event::Announce("a\nb".into()), Event::TextChanged, Event::SelectionChanged]);
        assert!(t.settle(t0 + Duration::from_secs(5), &text, 13).is_empty(), "announced once");

        // More output keeps the one announcement from its first start.
        let mut t = Tracker::default();
        t.refresh(t0, 10, 1, &line("$ ls"));
        t.refresh(t0 + Duration::from_millis(10), 11, 1, &line(""));
        t.refresh(t0 + Duration::from_millis(40), 12, 1, &line(""));
        let text = |abs: i64| Some(["$ ls", "a", "b"][(abs - 10) as usize].to_string());
        assert!(t.settle(t0 + Duration::from_millis(160), &text, 12).is_empty(), "the deadline moved with the last output");
        let ev = t.settle(t0 + Duration::from_millis(40) + SETTLE, &text, 12);
        assert_eq!(ev[0], Event::Announce("a".into()), "lines 10 and 11 only: the cursor's own line is not read");

        // A cursor moving up while pending cancels it.
        let mut t = Tracker::default();
        t.refresh(t0, 10, 1, &line("$ ls"));
        t.refresh(t0 + Duration::from_millis(10), 13, 1, &line(""));
        let ev = t.refresh(t0 + Duration::from_millis(20), 5, 1, &line(""));
        assert!(ev.contains(&Event::TextChanged), "nothing pending now");
        assert!(t.settle(t0 + Duration::from_secs(1), &|_| Some("x".into()), 5).is_empty());

        // A huge burst announces the last 256 lines.
        let mut t = Tracker::default();
        t.refresh(t0, 0, 1, &line("$ cat"));
        t.refresh(t0 + Duration::from_millis(10), 1000, 1, &line(""));
        let asked = std::cell::Cell::new(0);
        let lines = |abs: i64| {
            asked.set(asked.get() + 1);
            Some(format!("l{abs}"))
        };
        let ev = t.settle(t0 + Duration::from_secs(1), &lines, 1000);
        assert_eq!(asked.get(), 256);
        let Event::Announce(text) = &ev[0] else { panic!("{ev:?}") };
        assert_eq!(text.lines().count(), 256);
        assert!(text.starts_with("l744\n") && text.ends_with("l999"));
        // Lines that are gone read as blank.
        let mut t = Tracker::default();
        t.refresh(t0, 3, 1, &line("$"));
        t.refresh(t0 + Duration::from_millis(1), 5, 1, &line(""));
        assert!(!t.settle(t0 + Duration::from_secs(1), &|_| None, 5).iter().any(|e| matches!(e, Event::Announce(_))));
    }

    #[test]
    fn deletions_follow_iterm2() {
        let p = |s: &str| s.chars().enumerate().map(|(i, c)| (i, c.to_string())).collect::<Vec<_>>();
        assert_eq!(deletion(6, 5, &p("$ abc"), &p("$ ab")), Some("c".to_string()));
        assert_eq!(deletion(3, 3, &p("$ abc def"), &p("$  ")), Some("abc def".to_string()));
        assert_eq!(deletion(3, 3, &p("$ abc"), &p("$ abc")), None);
        assert_eq!(deletion(3, 4, &p("$ abc"), &p("$ ab")), None, "the cursor moved right");
        assert_eq!(deletion(5, 4, &p("$ abc"), &p("$ abc")), None, "the text under the cursor did not change");
        assert_eq!(deletion(3, 3, &p("$ abc"), &p("$ bc")), Some("a".to_string()), "a forward delete names the first character");
    }

    #[test]
    fn secure_pane_exposes_only_the_prompt() {
        let r = rows(&[("old output", false), ("Password: hunter2", false)], (1, 17));
        let s = secure_rows(&r, Some((1, 10)));
        let t = build(&s);
        assert_eq!(string(&t), "Password:\n");
        assert!(!string(&t).contains("hunter2") && !string(&t).contains("old"));
        // Without a recorded start (or one that scrolled out) nothing is exposed.
        assert_eq!(secure_rows(&rows(&[("Password: ", false)], (0, 10)), None), Rows::default());
        assert_eq!(secure_rows(&rows(&[("Password: ", false)], (0, 10)), Some((9, 10))), Rows::default());
        assert_eq!(s.cursor, (0, 10));
        assert_eq!(secure_rows(&Rows::default(), None), Rows::default());
    }

    /// A password echoed (or typed before the state was seen) that wraps onto the next row: the
    /// cursor is on the later row, so a cut at the cursor's row would expose the echo.
    #[test]
    fn secure_hides_a_wrapped_echo() {
        let mut r = rows(&[("old", false), ("Password: ", true), ("hunter2", false)], (2, 7));
        r.first_line = 50;
        // The prompt began on absolute line 51 and ended at column 10.
        let s = secure_rows(&r, Some((51, 10)));
        let shown = string(&build(&s));
        assert!(!shown.contains("hunter") && !shown.contains("old"), "got {shown:?}");
        assert_eq!(shown, "Password:\n");
    }

    #[test]
    fn secure_pane_raises_no_announcement() {
        let t0 = Instant::now();
        let p = |s: &str| s.chars().enumerate().map(|(i, c)| (i, c.to_string())).collect::<Vec<_>>();
        let mut t = Tracker::default();
        t.refresh(t0, 4, 7, &p("$ abcd"));
        t.refresh(t0 + Duration::from_millis(5), 6, 1, &p(""));
        assert!(t.refresh_secure().is_empty());
        // Everything is forgotten: a Backspace right after the prompt clears says nothing, and
        // nothing is pending.
        let ev = t.refresh(t0 + Duration::from_millis(10), 4, 6, &p("$ abc"));
        assert!(!ev.iter().any(|e| matches!(e, Event::Announce(_) | Event::AnnounceDeleted(_))), "{ev:?}");
        assert!(t.settle(t0 + Duration::from_secs(5), &|_| Some("x".into()), 6).is_empty());
    }

    #[test]
    fn lines_are_capped() {
        assert_eq!(clamp_lines(10_000_000_000), MAX_LINES);
        assert_eq!(MAX_LINES, 10_000);
        assert_eq!(clamp_lines(0), 1000, "P16: zero or less reads as the default");
        assert_eq!(clamp_lines(1000), 1000);
        assert_eq!(clamp_lines(-5), 1000);
        assert_eq!(clamp_lines(1), 1);
        let r = Rows { rows: (0..5).map(|_| row("abcdef", false)).collect(), cursor: (4, 0), first_line: 100 };
        let c = r.capped(14);
        assert_eq!(c.rows.len(), 2, "older rows go first, 7 units each");
        assert_eq!((c.cursor.0, c.first_line), (1, 103));
        assert_eq!(Rows { rows: vec![], ..Default::default() }.capped(0), Rows::default());
    }

    #[test]
    fn the_tracker_reports_its_deadline() {
        let t0 = Instant::now();
        let mut t = Tracker::default();
        assert_eq!(t.due(), None);
        t.refresh(t0, 10, 1, &[]);
        t.refresh(t0, 11, 1, &[]);
        assert_eq!(t.due(), Some(t0 + SETTLE));
        t.settle(t0 + SETTLE, &|_| None, 11);
        assert_eq!(t.due(), None);
    }

    #[test]
    fn publish_follows_the_signal() {
        use std::collections::HashMap;
        let base = Signal { tick: 5, sel: None, offset: 0, focused: true, rect: (0, 0, 100, 50), cell: (8, 16) };
        let mut last = HashMap::new();
        last.insert(1u64, base.clone());
        last.insert(2u64, base.clone());
        let now = |a: Signal| vec![(1u64, a), (2u64, base.clone())];
        assert!(changed_panes(&last, &now(base.clone())).is_empty(), "an unchanged pane does not publish");
        assert_eq!(changed_panes(&last, &now(Signal { tick: 6, ..base.clone() })), vec![1]);
        assert_eq!(changed_panes(&last, &now(Signal { sel: Some(((0, 1), (0, 4))), ..base.clone() })), vec![1], "a selection alone");
        assert_eq!(changed_panes(&last, &now(Signal { offset: 3, ..base.clone() })), vec![1], "a scroll alone");
        assert_eq!(changed_panes(&last, &now(Signal { focused: false, ..base.clone() })), vec![1], "focus alone");
        assert_eq!(changed_panes(&last, &now(Signal { rect: (1, 0, 100, 50), ..base.clone() })), vec![1], "a window move");
        assert_eq!(changed_panes(&last, &[(3, base.clone())]), vec![3], "a new pane");
    }
}
