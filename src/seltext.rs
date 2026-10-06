//! The selection model's small pure parts: how a selection was made (its live mode) and which
//! side of a cell the pointer picks, and the selection text model: sub-selections, iTerm2's
//! range enumeration (overlaps cancel) and the text extraction with its copy options.

use alacritty_terminal::index::Side;

use crate::find::Pos;

/// How a selection was made, in the API's order (`iTermSelectionMode`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Character,
    Word,
    Line,
    Smart,
    Box,
    WholeLine,
}

/// What a `Screen` remembers about its live selection beyond alacritty's own: the mode that
/// made it, the column window it stays inside, and the word or line a double or triple click
/// began on (a drag snaps its moving end to whole words or lines around it).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct State {
    pub live_mode: Mode,
    pub window: Option<(usize, usize)>,
    pub anchor: Option<(Pos, Pos)>,
    /// The cell the last drag step looked at, so a pointer that stays in one cell costs nothing.
    pub last: Option<Pos>,
    /// Selections added beside the live one, as numbered lines (`dropped + row`) so a full
    /// scrollback dropping its oldest line never shifts them (B2).
    pub extras: Vec<Sub>,
    /// What the buffer's rows depended on when `extras` were made; a change drops them.
    pub epoch: Epoch,
}

/// The facts that decide whether numbered extras still name the same text: the size, the
/// resets, the history splices taken and the screen shown.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Epoch {
    pub columns: usize,
    pub lines: usize,
    pub resets: u64,
    pub history_clears: u64,
    pub splices: u64,
    pub alt: bool,
}

/// One selection: `end` inclusive. A Box sub is a rectangle, every other mode a run, clipped
/// to `window` on each row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sub {
    pub start: Pos,
    pub end: Pos,
    pub mode: Mode,
    pub window: Option<(usize, usize)>,
    pub connected: bool,
}

/// The cells of a buffer as the text model reads them (rows from the oldest line).
pub trait Cells {
    fn width(&self) -> usize;
    fn rows(&self) -> usize;
    /// Whether `row` wraps into the next.
    fn wraps(&self, row: usize) -> bool;
    /// Appends the text of the cell (an empty cell is a space); false for a wide character's
    /// spacer, which holds none.
    fn push_cell(&self, row: usize, col: usize, out: &mut String) -> bool;
}

/// What a copy does with whitespace (`TrimWhitespaceOnCopy`, `CopyLastNewline`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Options {
    pub trim: bool,
    pub last_newline: bool,
}

impl Options {
    pub fn from_globals(g: &crate::globals::Globals) -> Options {
        Options { trim: g.bool("TrimWhitespaceOnCopy"), last_newline: g.bool("CopyLastNewline") }
    }
}

/// Whether a copy of `text` goes to the clipboard (`DisallowCopyEmptyString`).
pub fn copy_decision(text: &str, disallow_empty: bool) -> bool {
    !(text.is_empty() && disallow_empty)
}

/// `r` (alacritty's range, grid lines) as a sub; `history` is the rows above the screen.
pub fn from_range(r: &alacritty_terminal::selection::SelectionRange, history: usize, mode: Mode, window: Option<(usize, usize)>) -> Sub {
    let pos = |p: alacritty_terminal::index::Point| Pos { row: (p.line.0 + history as i32).max(0) as usize, col: p.column.0 };
    Sub { start: pos(r.start), end: pos(r.end), mode: if r.is_block { Mode::Box } else { mode }, window, connected: false }
}

/// The most sub-selections Select Matches and SetSelection make; the rest are dropped (P4).
pub const MAX_SUBS: usize = 10_000;

/// The columns `sub` covers on `row` of a `width`-column buffer, `(first, last)` inclusive;
/// None when it covers none there. A box is its columns on every row, a run is partial on its
/// first and last row, and a window clips every row.
pub fn row_span(sub: &Sub, row: usize, width: usize) -> Option<(usize, usize)> {
    if width == 0 || row < sub.start.row || row > sub.end.row {
        return None;
    }
    let last = width - 1;
    let (mut from, mut to) = if sub.mode == Mode::Box {
        (sub.start.col.min(sub.end.col), sub.start.col.max(sub.end.col))
    } else {
        (if row == sub.start.row { sub.start.col } else { 0 }, if row == sub.end.row { sub.end.col } else { last })
    };
    if let Some((lo, hi)) = sub.window {
        from = from.max(lo);
        to = to.min(hi);
    }
    let to = to.min(last);
    (from <= to).then_some((from, to))
}

/// A visual (bidi) selection's runs, each `(first, last)` inclusive as (buffer row, column), as
/// character subs: a run that starts at column 0 continues the previous one when that ended
/// at the last column of the line above and merges into it; a run followed by another on its
/// own line is `connected`. The API's `from_runs` and `Screen::subs` share this rule.
pub fn merge_runs(runs: &[((i64, i32), (i64, i32))], width: i32) -> Vec<((i64, i32), (i64, i32), bool)> {
    let mut merged: Vec<((i64, i32), (i64, i32))> = Vec::new();
    for &(a, b) in runs {
        match merged.last_mut() {
            Some(prev) if a.1 == 0 && a.0 == prev.1.0 + 1 && prev.1.1 == width - 1 => prev.1 = b,
            _ => merged.push((a, b)),
        }
    }
    (0..merged.len()).map(|i| (merged[i].0, merged[i].1, merged.get(i + 1).is_some_and(|n| n.0.0 == merged[i].1.0))).collect()
}

/// The cells `sub` covers as `(first, last)` linear indexes (`row * width + col`), one
/// interval per row for a box or a window and one interval for a plain run.
fn intervals(sub: &Sub, width: usize) -> Vec<(usize, usize)> {
    let at = |p: Pos| p.row * width + p.col.min(width.saturating_sub(1));
    if sub.mode != Mode::Box && sub.window.is_none() {
        return vec![(at(sub.start), at(sub.end))];
    }
    let mut out = Vec::new();
    for row in sub.start.row..=sub.end.row {
        let (mut from, mut to) = if sub.mode == Mode::Box {
            (sub.start.col.min(sub.end.col), sub.start.col.max(sub.end.col))
        } else {
            (if row == sub.start.row { sub.start.col } else { 0 }, if row == sub.end.row { sub.end.col } else { width.saturating_sub(1) })
        };
        if let Some((lo, hi)) = sub.window {
            from = from.max(lo);
            to = to.min(hi);
        }
        if from <= to {
            out.push((row * width + from, row * width + to));
        }
    }
    out
}

/// iTerm2's `enumerateSelectedAbsoluteRanges` (`iTermSelection.m:1370-1459`): one sub passes
/// through; several become the cells covered an odd number of times (an overlap cancels), as
/// sorted runs, each flagged to end its line unless it is the last or ends at a connected sub.
/// A sweep over the interval bounds, so a selection per match costs O(n log n).
pub fn ranges(subs: &[Sub], width: usize) -> Vec<(Pos, Pos, bool)> {
    if subs.len() == 1 {
        return vec![(subs[0].start, subs[0].end, false)];
    }
    if width == 0 {
        return Vec::new();
    }
    let mut events: Vec<usize> = Vec::new();
    let mut connectors = std::collections::HashSet::new();
    for sub in subs {
        let parts = intervals(sub, width);
        if sub.connected {
            if let Some(last) = parts.last() {
                connectors.insert(last.1);
            }
        }
        for (a, b) in parts {
            events.push(a);
            events.push(b + 1);
        }
    }
    events.sort_unstable();
    // A position appearing an odd number of times flips the cover; runs are between flips.
    let mut flips: Vec<usize> = Vec::new();
    let mut i = 0;
    while i < events.len() {
        let mut j = i;
        while j < events.len() && events[j] == events[i] {
            j += 1;
        }
        if (j - i) % 2 == 1 {
            flips.push(events[i]);
        }
        i = j;
    }
    let pos = |i: usize| Pos { row: i / width, col: i % width };
    let mut runs: Vec<(usize, usize)> = flips.chunks_exact(2).map(|c| (c[0], c[1] - 1)).collect();
    // Cells joined end to end are one run.
    runs.dedup_by(|b, a| {
        if a.1 + 1 == b.0 {
            a.1 = b.1;
            true
        } else {
            false
        }
    });
    let n = runs.len();
    runs.into_iter().enumerate().map(|(k, (a, b))| (pos(a), pos(b), k + 1 < n && !connectors.contains(&b))).collect()
}

/// Removes trailing spaces and tabs from `out`, never before `floor`.
fn trim_tail(out: &mut String, floor: usize) {
    let keep = out.trim_end_matches([' ', '\t']).len().max(floor);
    out.truncate(keep);
}

/// One sub's text (`iTermTextExtractor.m:1490-1545`): a newline ends a row that ends hard,
/// except after the sub's last row unless `last_newline`; with `trim` the spaces and tabs
/// before each newline and at the end go.
fn extract_sub(cells: &dyn Cells, sub: &Sub, opts: Options, end_trim: bool, out: &mut String) {
    let width = cells.width();
    if width == 0 || cells.rows() == 0 {
        return;
    }
    let base = out.len();
    let last_row = sub.end.row.min(cells.rows() - 1);
    for row in sub.start.row.min(last_row)..=last_row {
        let (mut from, mut to) = if sub.mode == Mode::Box {
            (sub.start.col.min(sub.end.col), sub.start.col.max(sub.end.col))
        } else {
            (if row == sub.start.row { sub.start.col } else { 0 }, if row == sub.end.row { sub.end.col } else { width - 1 })
        };
        // A window narrower than the row (a soft boundary, such as a tmux divider) ends its line
        // only where it ends in a blank; else the line goes on in the next row (`TE:2004-2101`).
        let windowed = sub.window.is_some_and(|(lo, hi)| lo > 0 || hi < width - 1);
        let edge = sub.window.map_or(width - 1, |(_, hi)| hi.min(width - 1));
        let reaches_end = sub.mode != Mode::Box && to >= edge;
        if let Some((lo, hi)) = sub.window {
            from = from.max(lo);
            to = to.min(hi);
        }
        to = to.min(width - 1);
        let mut col = from;
        let mut first = true;
        while col <= to {
            let held = cells.push_cell(row, col, out);
            // The character a selected trailing spacer belongs to.
            if !held && first && col > 0 {
                cells.push_cell(row, col - 1, out);
            }
            first = false;
            col += 1;
        }
        let last = row == last_row;
        let ends_row = || {
            if !windowed {
                return !cells.wraps(row);
            }
            let mut tail = String::new();
            cells.push_cell(row, to, &mut tail);
            // A wide character's trailing spacer pushes nothing: the character before it is the tail.
            if tail.is_empty() && to > 0 {
                cells.push_cell(row, to - 1, &mut tail);
            }
            tail.chars().all(char::is_whitespace)
        };
        let newline = if sub.mode == Mode::Box { !last || opts.last_newline } else { reaches_end && (!last || opts.last_newline) && ends_row() };
        if newline || (last && opts.trim && end_trim) {
            if opts.trim {
                trim_tail(out, base);
            }
            if newline {
                out.push('\n');
            }
        }
    }
}

/// The text of `subs` over `cells`: each sub's text, a newline after each unless it already
/// ends with one or is the last (`SX:20-31`).
pub fn extract(cells: &dyn Cells, subs: &[Sub], opts: Options) -> String {
    let mut out = String::new();
    if subs.len() == 1 {
        extract_sub(cells, &subs[0], opts, true, &mut out);
        return out;
    }
    let runs = ranges(subs, cells.width());
    let n = runs.len();
    for (k, (start, end, eol)) in runs.into_iter().enumerate() {
        let run = Sub { start, end, mode: Mode::Character, window: None, connected: false };
        // A run that goes on into the next (a connector) keeps its trailing spaces.
        extract_sub(cells, &run, opts, eol || k + 1 == n, &mut out);
        if eol && !out.ends_with('\n') {
            out.push('\n');
        }
    }
    out
}

/// The side of a cell a pointer at `frac` (0 to 1 across the cell) picks: the right neighbor's
/// side once it is within `fraction` of the cell's right edge (`PTYTextView+ARC.m:316`).
pub fn side_for(frac: f32, fraction: f32) -> Side {
    if frac > 1.0 - fraction { Side::Right } else { Side::Left }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neighbor_fraction_follows_iterm2() {
        assert_eq!(side_for(0.60, 0.35), Side::Left);
        assert_eq!(side_for(0.66, 0.35), Side::Right);
        assert_eq!(side_for(0.49, 0.5), Side::Left);
        assert_eq!(side_for(0.51, 0.5), Side::Right);
    }

    #[test]
    fn the_api_order_of_modes() {
        assert_eq!(Mode::default(), Mode::Character);
        assert_eq!(Mode::Box as i32, 4);
        assert_eq!(Mode::WholeLine as i32, 5);
    }

    /// Rows of text with a wrap flag each; "\u{0}" stands for a wide character's spacer.
    struct FakeCells {
        rows: Vec<(Vec<char>, bool)>,
        width: usize,
    }

    impl FakeCells {
        fn new(width: usize, rows: &[(&str, bool)]) -> FakeCells {
            FakeCells { rows: rows.iter().map(|(t, w)| (t.chars().collect(), *w)).collect(), width }
        }
    }

    impl Cells for FakeCells {
        fn width(&self) -> usize {
            self.width
        }
        fn rows(&self) -> usize {
            self.rows.len()
        }
        fn wraps(&self, row: usize) -> bool {
            self.rows[row].1
        }
        fn push_cell(&self, row: usize, col: usize, out: &mut String) -> bool {
            match self.rows[row].0.get(col).copied().unwrap_or(' ') {
                '\0' => false,
                c => {
                    out.push(c);
                    true
                }
            }
        }
    }

    /// Final review minor 9: a window whose last cell is a wide character's trailing spacer does
    /// not end its row (the wide character is text, so the line goes on).
    #[test]
    fn a_window_ending_in_a_wide_character_does_not_end_the_row() {
        let cells = FakeCells::new(4, &[("a\u{4e2d}\0b", false), ("cdef", false)]);
        let mut win = sub((0, 0), (1, 3), Mode::Character);
        win.window = Some((1, 2));
        assert_eq!(extract(&cells, &[win], TRIM), "\u{4e2d}de");
        // A blank at the window's end still ends it.
        let blank = FakeCells::new(4, &[("ab  ", false), ("cdef", false)]);
        let mut win = sub((0, 0), (1, 3), Mode::Character);
        win.window = Some((1, 2));
        assert_eq!(extract(&blank, &[win], TRIM), "b\nde");
    }

    fn sub(a: (usize, usize), b: (usize, usize), mode: Mode) -> Sub {
        Sub { start: Pos { row: a.0, col: a.1 }, end: Pos { row: b.0, col: b.1 }, mode, window: None, connected: false }
    }

    const TRIM: Options = Options { trim: true, last_newline: false };

    #[test]
    fn extract_follows_iterm2() {
        let cells = FakeCells::new(4, &[("ab  ", false), ("cd  ", false)]);
        let run = sub((0, 0), (1, 1), Mode::Character);
        assert_eq!(extract(&cells, &[run.clone()], TRIM), "ab\ncd");
        assert_eq!(extract(&cells, &[run.clone()], Options { trim: false, ..TRIM }), "ab  \ncd");
        // A row wrapped into the next joins without a newline.
        let wrapped = FakeCells::new(4, &[("abcd", true), ("ef  ", false)]);
        assert_eq!(extract(&wrapped, &[sub((0, 0), (1, 1), Mode::Character)], TRIM), "abcdef");
        // The last row ending at its last column: no newline unless CopyLastNewline.
        let to_end = sub((0, 0), (1, 3), Mode::Character);
        assert_eq!(extract(&cells, &[to_end.clone()], TRIM), "ab\ncd");
        assert_eq!(extract(&cells, &[to_end], Options { last_newline: true, ..TRIM }), "ab\ncd\n");
        // R3: a Line sub over one row used to end with a newline (alacritty added one).
        assert_eq!(extract(&cells, &[sub((0, 0), (0, 3), Mode::Line)], TRIM), "ab");
        assert_eq!(extract(&cells, &[sub((0, 0), (0, 3), Mode::Line)], Options { last_newline: true, ..TRIM }), "ab\n");
        // A Box takes the same columns of every row.
        assert_eq!(extract(&cells, &[sub((0, 0), (1, 0), Mode::Box)], TRIM), "a\nc");
        // A window clips each row.
        let mut windowed = sub((0, 0), (1, 1), Mode::Character);
        windowed.window = Some((1, 2));
        assert_eq!(extract(&cells, &[windowed], TRIM), "b\nd");
        // iTerm2's soft-boundary rule (`TE:2004-2101`): a window row with no blank at its end
        // continues on the next row (a line wrapped at a divider), one with a blank ends its line.
        let divided = FakeCells::new(6, &[("abcdXX", false), ("efg XX", false), ("ij  XX", false)]);
        let mut win = sub((0, 0), (2, 3), Mode::Character);
        win.window = Some((0, 3));
        assert_eq!(extract(&divided, &[win.clone()], TRIM), "abcdefg\nij");
        assert_eq!(extract(&divided, &[win], Options { last_newline: true, ..TRIM }), "abcdefg\nij\n");
        // A wide character's spacer adds nothing, and selecting only the spacer takes its character.
        let wide = FakeCells::new(4, &[("a\u{4e2d}\0b", false)]);
        assert_eq!(extract(&wide, &[sub((0, 0), (0, 3), Mode::Character)], TRIM), "a\u{4e2d}b");
        assert_eq!(extract(&wide, &[sub((0, 2), (0, 2), Mode::Character)], TRIM), "\u{4e2d}");
    }

    #[test]
    fn extract_trims_only_line_ends() {
        let cells = FakeCells::new(6, &[("a b   ", false), ("c  \t ", false)]);
        assert_eq!(extract(&cells, &[sub((0, 0), (1, 5), Mode::Character)], TRIM), "a b\nc");
        // Two selections: each ends a line, trimmed on its own.
        let two = [sub((0, 0), (0, 2), Mode::Character), sub((1, 0), (1, 0), Mode::Character)];
        assert_eq!(extract(&cells, &two, TRIM), "a b\nc");
    }

    #[test]
    fn ranges_follow_iterm2() {
        let one = sub((0, 0), (0, 4), Mode::Character);
        assert_eq!(ranges(&[one.clone()], 10), vec![(one.start, one.end, false)]);
        // The overlap cancels.
        let p = |r, c| Pos { row: r, col: c };
        let got = ranges(&[sub((0, 0), (0, 4), Mode::Character), sub((0, 2), (0, 7), Mode::Character)], 10);
        assert_eq!(got.iter().map(|r| (r.0, r.1)).collect::<Vec<_>>(), vec![(p(0, 0), p(0, 1)), (p(0, 5), p(0, 7))]);
        // Sorted, the first ending its line and the last not.
        let got = ranges(&[sub((2, 0), (2, 3), Mode::Character), sub((0, 0), (0, 3), Mode::Character)], 10);
        assert_eq!(got, vec![(p(0, 0), p(0, 3), true), (p(2, 0), p(2, 3), false)]);
        // A connected first sub's run is not flagged.
        let mut first = sub((0, 0), (0, 3), Mode::Character);
        first.connected = true;
        let got = ranges(&[first, sub((2, 0), (2, 3), Mode::Character)], 10);
        assert!(!got[0].2);
        // A run that crosses a row boundary stays one run.
        let got = ranges(&[sub((0, 8), (1, 1), Mode::Character), sub((5, 0), (5, 0), Mode::Character)], 10);
        assert_eq!(got[0], (p(0, 8), p(1, 1), true));
    }

    #[test]
    fn overlapping_selections_cancel_in_the_text() {
        let cells = FakeCells::new(10, &[("abcdefghij", false)]);
        let two = [sub((0, 0), (0, 4), Mode::Character), sub((0, 2), (0, 7), Mode::Character)];
        assert_eq!(extract(&cells, &two, TRIM), "ab\nfgh");
    }

    #[test]
    fn copy_options_follow_iterm2() {
        let mut g = crate::globals::Globals::empty(std::path::Path::new("unused.toml"));
        assert_eq!(Options::from_globals(&g), Options { trim: true, last_newline: false });
        g.set("TrimWhitespaceOnCopy", toml::Value::Boolean(false));
        g.set("CopyLastNewline", toml::Value::Boolean(true));
        assert_eq!(Options::from_globals(&g), Options { trim: false, last_newline: true });
    }

    #[test]
    fn copy_follows_the_keys() {
        assert!(!copy_decision("", true));
        assert!(copy_decision("", false));
        assert!(copy_decision("a", true));
    }

    #[test]
    fn a_connected_run_keeps_its_spaces() {
        let cells = FakeCells::new(10, &[("ab c      ", false)]);
        let mut first = sub((0, 0), (0, 2), Mode::Character);
        first.connected = true;
        assert_eq!(extract(&cells, &[first, sub((0, 3), (0, 3), Mode::Character)], TRIM), "ab c");
    }
}
