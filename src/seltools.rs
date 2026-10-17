//! iTerm2's selection tools: Replace Selection (pretty JSON, Base64), the selection
//! endpoint moves (key actions 45 to 48), and soft boundaries (pane dividers drawn by
//! tmux or vim that selection should not cross).

use crate::find::Pos;

pub fn pretty_json(text: &str) -> Result<String, String> {
    let v: serde_json::Value = serde_json::from_str(text.trim()).map_err(|e| format!("Not JSON: {e}"))?;
    serde_json::to_string_pretty(&v).map_err(|e| e.to_string())
}

pub fn base64_encode(text: &str) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(text)
}

pub fn base64_decode(text: &str) -> Result<String, String> {
    use base64::Engine;
    let compact: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    let bytes = base64::engine::general_purpose::STANDARD.decode(compact).map_err(|e| format!("Not Base64: {e}"))?;
    String::from_utf8(bytes).map_err(|_| "The decoded value is not text.".to_string())
}

/// Replace Selection needs whole lines: the selection starts at column 0 and ends at or
/// past the last character of its last line.
pub fn whole_lines(start: Pos, end: Pos, last_line_end: usize) -> bool {
    start.col == 0 && end.col >= last_line_end
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum End {
    Start,
    End,
}

/// Moves one end of a selection `n` cells left or right, across rows; the other end stays.
pub fn move_endpoint(sel: (Pos, Pos), which: End, left: bool, n: usize, cols: usize, last_row: usize) -> (Pos, Pos) {
    let index = |p: Pos| p.row * cols + p.col;
    let pos = |i: usize| Pos { row: i / cols, col: i % cols };
    let max = last_row * cols + cols - 1;
    let (start, end) = (index(sel.0), index(sel.1));
    let moved = |i: usize| if left { i.saturating_sub(n) } else { (i + n).min(max) };
    match which {
        End::Start => (pos(moved(start).min(end)), sel.1),
        End::End => (sel.0, pos(moved(end).max(start))),
    }
}

/// iTerm2's soft boundary characters.
pub const DIVIDERS: [char; 4] = ['|', '\u{2502}', '\u{251c}', '\u{2524}'];

/// A divider must repeat in its column this many rows running.
pub const DIVIDER_RUN: usize = 8;

/// Columns of `rows[row]` that hold a pane divider: a divider character with the same
/// character in that column for DIVIDER_RUN rows running through this one.
pub fn divider_columns(rows: &[Vec<char>], row: usize) -> Vec<usize> {
    let Some(line) = rows.get(row) else { return Vec::new() };
    let same = |r: usize, col: usize, c: char| rows[r].get(col) == Some(&c);
    line.iter()
        .enumerate()
        .filter(|(_, c)| DIVIDERS.contains(c))
        .filter(|(col, c)| {
            let up = (0..=row).rev().take_while(|r| same(*r, *col, **c)).count();
            let down = (row..rows.len()).take_while(|r| same(*r, *col, **c)).count();
            // The run through this row, both ways.
            up + down > DIVIDER_RUN
        })
        .map(|(col, _)| col)
        .collect()
}

/// The part of `lo..=hi` on the click's side of any divider at `click`'s row.
pub fn clip_to_dividers(lo: usize, hi: usize, click: usize, dividers: &[usize]) -> (usize, usize) {
    let left = dividers.iter().filter(|d| **d < click).max().map_or(lo, |d| lo.max(d + 1));
    let right = dividers.iter().filter(|d| **d > click).min().map_or(hi, |d| hi.min(d - 1));
    (left, right)
}

/// A match that ends at its window's right edge, extended across the divider onto the rows
/// below (`SearchResultSoftBoundaryExtender.swift`): the text from `start` to the window's edge
/// on `start.row`, then each following row's window while the divider column still holds a
/// divider, is matched again and a match from the start that reaches a later row wins. Returns
/// its end cell and text. `rows` are the cell characters, `start.row` indexes them; the regex
/// is the caller's bounded one and an error is no match.
pub fn extend_across_divider(rows: &[Vec<char>], start: (usize, usize), window: (usize, usize), re: &fancy_regex::Regex) -> Option<((usize, usize), String)> {
    let (lo, hi) = window;
    let divider_col = hi + 1;
    let mut text = String::new();
    let mut coords: Vec<(usize, usize)> = Vec::new();
    let mut append = |row: usize, from: usize| {
        for col in from..=hi {
            let Some(&c) = rows.get(row).and_then(|r| r.get(col)) else { break };
            text.push(c);
            coords.push((row, col));
        }
    };
    append(start.0, start.1);
    let mut row = start.0 + 1;
    while row < rows.len() && rows[row].get(divider_col).is_some_and(|c| DIVIDERS.contains(c)) {
        append(row, lo);
        row += 1;
    }
    let m = re.find(&text).ok().flatten().filter(|m| m.start() == 0 && m.end() > 0)?;
    let last = text[..m.end()].chars().count() - 1;
    let end = coords[last];
    (end.0 > start.0).then(|| (end, m.as_str().to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(row: usize, col: usize) -> Pos {
        Pos { row, col }
    }

    #[test]
    fn json_is_pretty_printed() {
        assert_eq!(pretty_json(r#"{"a":1,"b":[true,null]}"#).unwrap(), "{\n  \"a\": 1,\n  \"b\": [\n    true,\n    null\n  ]\n}");
        assert!(pretty_json("{not json").is_err());
    }

    #[test]
    fn base64_round_trips() {
        assert_eq!(base64_encode("hello"), "aGVsbG8=");
        assert_eq!(base64_decode("aGVsbG8=").unwrap(), "hello");
        assert_eq!(base64_decode(" aGVs\nbG8= ").unwrap(), "hello");
        assert!(base64_decode("!!!").is_err());
        assert!(base64_decode("/w==").is_err(), "not text");
    }

    #[test]
    fn replacement_needs_whole_lines() {
        assert!(whole_lines(at(3, 0), at(5, 9), 9));
        assert!(whole_lines(at(3, 0), at(5, 79), 9));
        assert!(!whole_lines(at(3, 1), at(5, 9), 9));
        assert!(!whole_lines(at(3, 0), at(5, 4), 9));
    }

    #[test]
    fn endpoint_moves_wrap_rows() {
        let sel = (at(1, 2), at(1, 5));
        assert_eq!(move_endpoint(sel, End::End, false, 3, 10, 9), (at(1, 2), at(1, 8)));
        assert_eq!(move_endpoint(sel, End::End, false, 6, 10, 9), (at(1, 2), at(2, 1)));
        assert_eq!(move_endpoint(sel, End::Start, true, 4, 10, 9), (at(0, 8), at(1, 5)));
        assert_eq!(move_endpoint(sel, End::Start, true, 99, 10, 9), (at(0, 0), at(1, 5)));
        assert_eq!(move_endpoint(sel, End::End, false, 999, 10, 9), (at(1, 2), at(9, 9)));
        // The ends never cross: the moving end stops at the other.
        assert_eq!(move_endpoint(sel, End::Start, false, 9, 10, 9), (at(1, 5), at(1, 5)));
    }

    #[test]
    #[allow(clippy::if_same_then_else)] // each branch states its own case; merging them would hide the cases
    fn dividers_need_eight_rows() {
        let grid = |n: usize| (0..12).map(|r| format!("{} left \u{2502} right", if r < n { "" } else { "" }).chars().collect::<Vec<char>>()).collect::<Vec<_>>();
        let rows = grid(12);
        let col = rows[0].iter().position(|c| *c == '\u{2502}').unwrap();
        assert_eq!(divider_columns(&rows, 5), vec![col]);
        let mut short = rows.clone();
        for r in short.iter_mut().skip(4) {
            r[col] = ' ';
        }
        assert_eq!(divider_columns(&short, 2), Vec::<usize>::new(), "only 4 rows");
        let pipes: Vec<Vec<char>> = vec!["a | b".chars().collect()];
        assert!(divider_columns(&pipes, 0).is_empty(), "a lone pipe is text");
        assert_eq!(clip_to_dividers(0, 20, 3, &[6]), (0, 5));
        assert_eq!(clip_to_dividers(0, 20, 9, &[6]), (7, 20));
        assert_eq!(clip_to_dividers(2, 4, 3, &[]), (2, 4));
    }

    #[test]
    fn soft_boundary_urls_extend() {
        let re = fancy_regex::Regex::new(r"https?://[a-z.]+/?").unwrap();
        let rows: Vec<Vec<char>> = ["http://ex\u{2502}zz", "ample.com\u{2502}zz"].iter().map(|r| r.chars().collect()).collect();
        // The match "http://ex" ends at column 8, the window's right edge: it reaches row 1.
        assert_eq!(extend_across_divider(&rows, (0, 0), (0, 8), &re), Some(((1, 8), "http://example.com".to_string())));
        // A match that starts mid-row extends the same way.
        let rows: Vec<Vec<char>> = ["see http://ex\u{2502}", "ample.com    \u{2502}"].iter().map(|r| r.chars().collect()).collect();
        assert_eq!(extend_across_divider(&rows, (0, 4), (0, 12), &re), Some(((1, 8), "http://example.com".to_string())));
        // A match ending before the window's edge has nothing to extend: the row below holds
        // no divider in the divider column.
        let rows: Vec<Vec<char>> = ["http://a.b  \u{2502}zz", "next line   zz"].iter().map(|r| r.chars().collect()).collect();
        assert_eq!(extend_across_divider(&rows, (0, 0), (0, 11), &re), None);
        // Rows with no divider do not extend.
        let rows: Vec<Vec<char>> = ["http://ex", "ample.com"].iter().map(|r| r.chars().collect()).collect();
        assert_eq!(extend_across_divider(&rows, (0, 0), (0, 8), &re), None);
    }
}
