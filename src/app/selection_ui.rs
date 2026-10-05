//! Word and line selection in the window: double and triple click, the live modes a drag keeps,
//! and the column window of "Selection Respects Soft Boundaries".

use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::cell::Flags;

use super::find_ui::{to_point, to_pos};
use super::*;
use crate::find::{self, Logical, Pos};
use crate::screen::Screen;
use crate::seltext::Mode;
use crate::seltools;
use crate::wordsel::{self, Rule, Segmenter};

/// The cells a triple click selects at buffer row `row`: the whole logical line when
/// `full_wrapped`, else that screen row; a column `window` keeps it to those columns of the row.
pub fn line_span(screen: &Screen, row: usize, full_wrapped: bool, window: Option<(usize, usize)>) -> (Pos, Pos) {
    let grid = screen.term().grid();
    let (top, bottom) = (-(grid.history_size() as i32), grid.screen_lines() as i32 - 1);
    let last = grid.columns().saturating_sub(1);
    let line = (row as i32 + top).clamp(top, bottom);
    let row = (line - top) as usize;
    if let Some((lo, hi)) = window {
        return (Pos { row, col: lo }, Pos { row, col: hi.min(last) });
    }
    if !full_wrapped {
        return (Pos { row, col: 0 }, Pos { row, col: last });
    }
    let (mut a, mut b) = (line, line);
    while a > top && find::wraps(screen, a - 1) {
        a -= 1;
    }
    while b < bottom && find::wraps(screen, b) {
        b += 1;
    }
    (Pos { row: (a - top) as usize, col: 0 }, Pos { row: (b - top) as usize, col: last })
}

/// The columns between the tmux or vim pane dividers around a click (`window_at` over the rows
/// near it); None where the row has no divider.
pub fn click_window(screen: &Screen, row: usize, col: usize) -> Option<(usize, usize)> {
    let grid = screen.term().grid();
    let (top, bottom) = (-(grid.history_size() as i32), grid.screen_lines() as i32 - 1);
    let line = (row as i32 + top).clamp(top, bottom);
    let (from, to) = ((line - seltools::DIVIDER_RUN as i32).max(top), (line + seltools::DIVIDER_RUN as i32).min(bottom));
    let rows: Vec<Vec<char>> = (from..=to).map(|l| (0..grid.columns()).map(|c| grid[Line(l)][Column(c)].c).collect()).collect();
    wordsel::window_at(&rows, (line - from) as usize, col, grid.columns())
}

/// `line` cut to the cells of `row` in columns `lo..=hi`.
fn restrict(line: &Logical, row: usize, lo: usize, hi: usize) -> Logical {
    let mut out = Logical { text: String::new(), cells: Vec::new() };
    for (i, (o, p)) in line.cells.iter().enumerate() {
        if p.row == row && p.col >= lo && p.col <= hi {
            let end = line.cells.get(i + 1).map_or(line.text.len(), |c| c.0);
            out.cells.push((out.text.len(), *p));
            out.text.push_str(&line.text[*o..end]);
        }
    }
    out
}

/// The cells of the word at `at` (a double click), both ends inclusive: iTerm2's word over the
/// logical line, kept inside the column `window`. Where there is no text (past a line's end)
/// it is the one cell.
pub fn word_span(screen: &Screen, at: Pos, rule: &Rule, seg: &dyn Segmenter, window: Option<(usize, usize)>) -> (Pos, Pos) {
    let (lines, i) = find::logical_lines_around(screen, at.row, 0);
    let Some(line) = lines.get(i) else { return (at, at) };
    let owned;
    let line = match window {
        Some((lo, hi)) => {
            owned = restrict(line, at.row, lo, hi);
            &owned
        }
        None => line,
    };
    let Some(offset) = line.offset_of(at) else { return (at, at) };
    let range = wordsel::word_at(line, offset, rule, seg);
    if range.is_empty() {
        return (at, at);
    }
    let (a, mut b) = (line.pos_before(range.start), line.pos_before(range.end - 1));
    // A wide character's spacer joins it.
    if screen.term().grid()[to_point(screen, b)].flags.contains(Flags::WIDE_CHAR) {
        b.col += 1;
    }
    (a, b)
}

#[cfg(windows)]
fn segmenter() -> Box<dyn Segmenter> {
    Box::new(wordsel::IcuSegmenter)
}

#[cfg(not(windows))]
fn segmenter() -> Box<dyn Segmenter> {
    Box::new(wordsel::NoSegmenter)
}

/// Makes `a..=b` the live selection of `pane`, made in `mode`, remembering what a drag snaps to.
fn make_live(pane: &mut crate::pane::Pane, a: Pos, b: Pos, mode: Mode, window: Option<(usize, usize)>) {
    let mut sel = Selection::new(SelectionType::Simple, to_point(&pane.screen, a), Side::Left);
    sel.update(to_point(&pane.screen, b), Side::Right);
    pane.set_selection(Some(sel), mode);
    pane.screen.sel.window = window;
    pane.screen.sel.anchor = Some((a, b));
}

impl App {
    /// The text of every selection in `id`'s pane through `seltext::extract` with the copy
    /// options; None with no selection. `Screen::selection_text` is the one entry (every other
    /// reader of the selected text goes through it too); this is its pane-level name.
    pub(super) fn selected_text(&mut self, id: PaneId) -> Option<String> {
        self.panes.get(&id).and_then(|p| p.screen.selection_text())
    }

    /// Copies a selection the person just made (mouse release, Smart Selection, Select All):
    /// nothing with `CopySelection` off (`TV:3940-3945`).
    pub(super) fn copy_on_selection(&mut self, id: PaneId) {
        if !self.globals.bool("CopySelection") {
            return;
        }
        self.copy_selection(id);
    }

    /// The share of a cell, from its right edge, in which the pointer picks the next cell
    /// (`FractionOfCharacterSelectingNextNeighbor`, iTerm2's 0.35).
    pub(super) fn neighbor_fraction(&self) -> f32 {
        let f = self.globals.number("FractionOfCharacterSelectingNextNeighbor").unwrap_or(0.35) as f32;
        if f.is_nan() { 0.35 } else { f.clamp(0.0, 1.0) }
    }

    fn respects_soft_boundaries(&self) -> bool {
        self.globals.bool("Selection Respects Soft Boundaries")
    }

    /// A double click: selects iTerm2's word at `point`.
    pub(super) fn select_word_at(&mut self, id: PaneId, point: Point) {
        let rule = Rule::from_globals(&self.globals);
        let respects = self.respects_soft_boundaries();
        let Some(p) = self.panes.get_mut(&id) else { return };
        let mut at = to_pos(&p.screen, point);
        // The right half of a wide character is its character.
        if at.col > 0 && p.screen.term().grid()[point].flags.contains(Flags::WIDE_CHAR_SPACER) {
            at.col -= 1;
        }
        let window = respects.then(|| click_window(&p.screen, at.row, at.col)).flatten();
        let (a, b) = word_span(&p.screen, at, &rule, &*segmenter(), window);
        make_live(p, a, b, Mode::Word, window);
    }

    /// A triple click: selects the line at `point`, the whole wrapped line or the screen row.
    pub(super) fn select_line_at(&mut self, id: PaneId, point: Point) {
        let full = self.globals.bool("TripleClickSelectsFullWrappedLines");
        let respects = self.respects_soft_boundaries();
        let Some(p) = self.panes.get_mut(&id) else { return };
        let at = to_pos(&p.screen, point);
        let window = respects.then(|| click_window(&p.screen, at.row, at.col)).flatten();
        let (a, b) = line_span(&p.screen, at.row, full, window);
        make_live(p, a, b, if full { Mode::WholeLine } else { Mode::Line }, window);
    }

    /// A drag moves the live selection's end to `point`. A word or line selection snaps the end
    /// to the word or line under it (the start of it moving back, the end moving on); while the
    /// column window holds the end stays inside it, and a pointer that leaves it drops the window
    /// for the rest of the drag (`PTYTextView.m:4007-4013`).
    pub(super) fn extend_live_to(&mut self, id: PaneId, point: Point, side: Side) {
        let rule = Rule::from_globals(&self.globals);
        let Some(p) = self.panes.get_mut(&id) else { return };
        let at = to_pos(&p.screen, point);
        if p.screen.sel.window.is_some_and(|(lo, hi)| at.col < lo || at.col > hi) {
            p.screen.sel.window = None;
        }
        let mode = p.screen.sel.live_mode;
        let Some((a0, b0)) = p.screen.sel.anchor.filter(|_| matches!(mode, Mode::Word | Mode::Line | Mode::WholeLine)) else {
            if let Some(sel) = p.screen.term_mut().selection.as_mut() {
                sel.update(point, side);
            }
            return;
        };
        if p.screen.sel.last == Some(at) {
            return;
        }
        let window = p.screen.sel.window;
        let (a1, b1) = match mode {
            Mode::Word => word_span(&p.screen, at, &rule, &*segmenter(), window),
            _ => line_span(&p.screen, at.row, mode == Mode::WholeLine, window),
        };
        let sel = if a1 < a0 {
            let mut sel = Selection::new(SelectionType::Simple, to_point(&p.screen, b0), Side::Right);
            sel.update(to_point(&p.screen, a1), Side::Left);
            sel
        } else {
            let mut sel = Selection::new(SelectionType::Simple, to_point(&p.screen, a0), Side::Left);
            sel.update(to_point(&p.screen, b1), Side::Right);
            sel
        };
        p.screen.reshape_selection(sel);
        p.screen.sel.last = Some(at);
    }

    /// A Smart Selection with Selection Respects Soft Boundaries on stays between the dividers:
    /// the window is remembered and a one-row match is clipped to it.
    pub(super) fn smart_window(&mut self, id: PaneId, point: Point) {
        if !self.respects_soft_boundaries() {
            return;
        }
        let Some((a, b)) = self.selection_pos(id) else { return };
        let Some(p) = self.panes.get_mut(&id) else { return };
        let Some(window @ (lo, hi)) = click_window(&p.screen, a.row, to_pos(&p.screen, point).col) else { return };
        let (na, nb) = (a.col.max(lo), b.col.min(hi));
        if a.row == b.row && (na, nb) != (a.col, b.col) && na <= nb {
            make_live(p, Pos { row: a.row, col: na }, Pos { row: a.row, col: nb }, Mode::Smart, Some(window));
        } else {
            p.screen.sel.window = Some(window);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_spans_follow_iterm2() {
        let mut s = Screen::new(10, 4);
        s.feed(b"abcdefghijklmno\r\nxyz");
        let at = |row, col| Pos { row, col };
        // Row 0 wraps into row 1: a full triple click spans both, from either.
        assert_eq!(line_span(&s, 0, true, None), (at(0, 0), at(1, 9)));
        assert_eq!(line_span(&s, 1, true, None), (at(0, 0), at(1, 9)));
        // One row as it appears on screen.
        assert_eq!(line_span(&s, 0, false, None), (at(0, 0), at(0, 9)));
        assert_eq!(line_span(&s, 1, false, None), (at(1, 0), at(1, 9)));
        // A window spans its columns of the clicked row.
        assert_eq!(line_span(&s, 1, true, Some((4, 6))), (at(1, 4), at(1, 6)));
        // A row that does not wrap is itself.
        assert_eq!(line_span(&s, 2, true, None), (at(2, 0), at(2, 9)));
    }
}
