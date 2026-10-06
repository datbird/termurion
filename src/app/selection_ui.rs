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

/// How a selection the person just made is copied.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CopyKind {
    Plain,
    Styled,
}

/// What copy on selection does: nothing with `CopySelection` off, else plain text or, with
/// `CopyWithStylesByDefault`, Copy with Styles (`PTYTextView.m:4089-4095`).
pub fn copy_kind(copy_on_select: bool, with_styles: bool) -> Option<CopyKind> {
    copy_on_select.then_some(if with_styles { CopyKind::Styled } else { CopyKind::Plain })
}

/// Whether a selection of `len` cells is short enough for auto search: at most 80 or the
/// pane's width, whichever is more (`PTYTextView.m:3893-3896`).
pub(super) fn short_selection(len: usize, width: usize) -> bool {
    len <= width.max(80)
}

/// Whether a key sent to the session removes a finished selection: `TypingClearsSelection` on
/// and no selection being dragged (`PTYTextView.m:7495-7500`).
pub(super) fn typing_clears(live: bool, key_on: bool) -> bool {
    key_on && !live
}

/// The query a finished selection puts on the shared find query: its text when `AutoSearch` is
/// on and it is short (`PTYTextView.m:6275-6284`).
pub(super) fn auto_search_query(on: bool, text: &str, width: usize) -> Option<String> {
    let len = text.chars().count();
    (on && len > 0 && short_selection(len, width)).then(|| text.to_string())
}

impl App {
    /// The text of every selection in `id`'s pane through `seltext::extract` with the copy
    /// options; None with no selection. `Screen::selection_text` is the one entry (every other
    /// reader of the selected text goes through it too); this is its pane-level name.
    pub(super) fn selected_text(&mut self, id: PaneId) -> Option<String> {
        self.panes.get(&id).and_then(|p| p.screen.selection_text())
    }

    /// A selection the person just made ended (`liveSelectionDidEnd`, `PTYTextView.m:6275-6284`):
    /// with `AutoSearch` on a short one becomes the shared find query, even with sync off, and the
    /// focused bar searches it quietly (B5), leaving the selection alone.
    pub(super) fn selection_ended(&mut self, id: PaneId) {
        let on = self.globals.bool("AutoSearch");
        let Some(p) = self.panes.get(&id) else { return };
        let width = p.screen.term().columns();
        let Some(text) = p.screen.selection_text() else { return };
        if let Some(q) = auto_search_query(on, &text, width) {
            self.set_find_pasteboard_always(&q, None);
        }
    }

    /// Copies a selection the person just made (mouse release, Smart Selection, Select All):
    /// nothing with `CopySelection` off (`TV:3940-3945`).
    pub(super) fn copy_on_selection(&mut self, id: PaneId) {
        match copy_kind(self.globals.bool("CopySelection"), self.globals.bool("CopyWithStylesByDefault")) {
            None => {}
            Some(CopyKind::Plain) => self.copy_selection(id),
            Some(CopyKind::Styled) => self.copy_with_styles(id),
        }
    }

    /// The selections a press keeps (`iTermSelection.m:391-400`): with `append` (Win without Alt)
    /// every selection now, to be put back beside the new one with `carry_subs`; a new
    /// non-append selection starts over, which empties the extras. The selections join at the
    /// press, never at the release (B11).
    pub(super) fn commit_live_selection(&mut self, id: PaneId, append: bool, multi: bool) -> Vec<crate::seltext::Sub> {
        self.panes.get(&id).map(|p| p.screen.carried_subs(append, multi)).unwrap_or_default()
    }

    pub(super) fn carry_subs(&mut self, id: PaneId, subs: Vec<crate::seltext::Sub>) {
        if let Some(p) = self.panes.get_mut(&id) {
            p.screen.carry_subs(subs);
        }
    }

    /// Makes `subs` the selection of `id` (the last the live one, the rest extras); true when
    /// the cap dropped some. Used by Select Matches and the API.
    pub(super) fn set_subs(&mut self, id: PaneId, subs: Vec<crate::seltext::Sub>) -> bool {
        let capped = self.panes.get_mut(&id).is_some_and(|p| p.screen.set_subs(subs));
        if capped {
            self.banners.set("select-matches", format!("Selected the first {} selections", crate::seltext::MAX_SUBS));
        }
        capped
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
    fn auto_search_follows_iterm2() {
        assert!(short_selection(80, 40));
        assert!(!short_selection(81, 40));
        assert!(short_selection(120, 132));
        assert!(!short_selection(133, 132));
        // On, a finished 5-character selection becomes the shared query; off, nothing.
        assert_eq!(auto_search_query(true, "hello", 80).as_deref(), Some("hello"));
        assert_eq!(auto_search_query(false, "hello", 80), None);
        // A 200-character selection in an 80-column pane is not short; nor is an empty one.
        assert_eq!(auto_search_query(true, &"x".repeat(200), 80), None);
        assert_eq!(auto_search_query(true, "", 80), None);
        // It goes to the shared query even with sync off (setStringValueUnconditionally).
        let mut board = String::new();
        let q = auto_search_query(true, "hello", 80).unwrap();
        assert!(super::find_ui::board_set(&mut board, false, &q, true));
        assert_eq!(board, "hello");
    }

    /// A key typed to the session removes a finished selection, not one being dragged (R3, `02:1016`).
    #[test]
    fn typing_clears_selection() {
        assert!(typing_clears(false, true));
        assert!(!typing_clears(true, true));
        assert!(!typing_clears(false, false));
        // B4: the clear sits inside the bytes branch of to_terminal, after the movement scroll and
        // before the write, so a bare modifier press (a Win for a Win-drag) keeps the selection.
        let body = include_str!("mod.rs").split("fn to_terminal(").nth(1).unwrap().split("fn mouse_press(").next().unwrap();
        let (bytes, clear, write) = (body.find("if let Some(bytes) = bytes").unwrap(), body.find("typing_clears(").unwrap(), body.find("self.user_write(").unwrap());
        let movement = body.find("movement_scroll(").unwrap();
        assert!(movement < bytes && bytes < clear && clear < write);
    }

    /// Task 5 fix r1 items 1 and 2 (`PTYTextView.m:4189-4193`, `:4210`): Copy with Styles refuses an
    /// empty copy with DisallowCopyEmptyString and saves the text to Paste History, before it writes
    /// the clipboard, like the plain copy.
    #[test]
    fn copy_with_styles_follows_the_plain_copy() {
        let body = include_str!("seltools_ui.rs").split("fn copy_with_styles(").nth(1).unwrap().split("\n    }\n").next().unwrap();
        let (text, refuse, remember, write) = (body.find("selection_text()").unwrap(), body.find("copy_decision(").unwrap_or(usize::MAX), body.find("remember_paste(").unwrap_or(usize::MAX), body.find("winclip::set(").unwrap());
        assert!(text < refuse && refuse < write, "an empty styled copy is refused before the write");
        assert!(body.contains("DisallowCopyEmptyString"));
        assert!(remember < write, "the styled copy reaches Paste History");
    }

    /// Task 4 re-review item 3: a Win-press masks the Start menu for a left press on a pane even
    /// with CommandSelection off (the Win-drag append still ends with a Win release).
    #[test]
    fn win_press_masks_the_start_menu_without_command_selection() {
        let body = include_str!("mod.rs").split("fn mouse_press(").nth(1).unwrap().split("fn mouse_release(").next().unwrap();
        let at = body.find("The Win release must not open the Start menu").unwrap();
        let guard = &body[at..];
        let mask = guard.find("mask_start_menu()").unwrap();
        let cond = guard[..mask].rfind("if mods.cmd").unwrap();
        assert!(!guard[cond..mask].contains("CommandSelection"));
        assert!(guard[..mask].contains("#[cfg(windows)]"));
    }

    #[test]
    fn copy_on_selection_uses_styles() {
        assert_eq!(copy_kind(false, false), None);
        assert_eq!(copy_kind(false, true), None);
        assert_eq!(copy_kind(true, false), Some(CopyKind::Plain));
        assert_eq!(copy_kind(true, true), Some(CopyKind::Styled));
    }

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

    /// A press as `mouse_press` makes it: the carried selections first (B11), the new live one,
    /// the carried ones back beside it.
    fn press(s: &mut Screen, row: i32, col: usize, append: bool) {
        let carried = s.carried_subs(append, false);
        s.set_selection(Some(Selection::new(SelectionType::Simple, Point::new(Line(row), Column(col)), Side::Left)), Mode::Character);
        s.carry_subs(carried);
    }

    /// Review I1: a Win second or third click resumes, replacing the previous click's word.
    #[test]
    fn win_triple_click_replaces_the_word() {
        let mut s = Screen::new(10, 4);
        s.feed(b"abc def");
        let make = |s: &mut Screen, a: usize, b: usize, mode: Mode, multi: bool| {
            let carried = s.carried_subs(true, multi);
            let mut sel = Selection::new(SelectionType::Simple, Point::new(Line(0), Column(a)), Side::Left);
            sel.update(Point::new(Line(0), Column(b)), Side::Right);
            s.set_selection(Some(sel), mode);
            s.carry_subs(carried);
        };
        make(&mut s, 0, 2, Mode::Word, false);
        make(&mut s, 0, 9, Mode::Line, true);
        assert_eq!(s.subs().len(), 1, "one line sub, not the word and the line");
        assert_eq!(s.selection_text().as_deref(), Some("abc def"));
        // An earlier separate selection survives the resume.
        let mut s = Screen::new(10, 4);
        s.feed(b"abc def\r\nxyz");
        make(&mut s, 0, 2, Mode::Character, false);
        let carried = s.carried_subs(true, false);
        let mut sel = Selection::new(SelectionType::Simple, Point::new(Line(1), Column(0)), Side::Left);
        sel.update(Point::new(Line(1), Column(2)), Side::Right);
        s.set_selection(Some(sel), Mode::Word);
        s.carry_subs(carried);
        let carried = s.carried_subs(true, true);
        assert_eq!(carried.len(), 1, "the first selection stays, the word goes");
        assert_eq!(carried[0].end.col, 2);
    }

    fn drag_to(s: &mut Screen, row: i32, col: usize) {
        s.term_mut().selection.as_mut().unwrap().update(Point::new(Line(row), Column(col)), Side::Right);
    }

    /// Review Focus 4: Win-drag adds a selection, Win+Alt does not, a plain press clears them.
    #[test]
    fn win_drag_appends() {
        let mut s = Screen::new(10, 4);
        s.feed(b"abcx\r\ndefgh");
        press(&mut s, 0, 0, false);
        drag_to(&mut s, 0, 2);
        assert_eq!(s.subs().len(), 1);
        // Win (append): the first selection stays, the drag is the second.
        press(&mut s, 1, 0, true);
        drag_to(&mut s, 1, 3);
        assert_eq!(s.subs().len(), 2);
        assert_eq!(s.selection_text().as_deref(), Some("abc\ndefg"));
        // Win+Alt is no append: a fresh selection.
        press(&mut s, 1, 0, false);
        drag_to(&mut s, 1, 3);
        assert_eq!(s.subs().len(), 1);
        assert_eq!(s.selection_text().as_deref(), Some("defg"));
        // A plain press clears every selection at once.
        press(&mut s, 0, 0, true);
        drag_to(&mut s, 0, 1);
        assert_eq!(s.subs().len(), 2);
        press(&mut s, 3, 0, false);
        assert!(s.subs().is_empty() && s.sel.extras.is_empty());
        assert_eq!(s.selection_text(), None);
    }

    /// B11: the earlier selection joins at the press, so nothing counts twice at the release.
    #[test]
    fn append_commit_does_not_double() {
        let mut s = Screen::new(10, 4);
        s.feed(b"abcx\r\ndefgh");
        press(&mut s, 0, 0, false);
        drag_to(&mut s, 0, 2);
        press(&mut s, 1, 0, true);
        assert_eq!((s.subs().len(), s.sel.extras.len()), (1, 1), "the empty live selection adds none");
        drag_to(&mut s, 1, 3);
        assert_eq!((s.subs().len(), s.sel.extras.len()), (2, 1));
        assert_eq!(s.selection_text().as_deref(), Some("abc\ndefg"), "each text once");
    }

    /// B11: a Win-click with no drag gives the last selection back as the live one.
    #[test]
    fn empty_win_click_keeps_extras() {
        let mut s = Screen::new(10, 4);
        s.feed(b"abcx\r\ndefgh");
        press(&mut s, 0, 0, false);
        drag_to(&mut s, 0, 2);
        press(&mut s, 3, 0, true);
        assert!(s.live_is_empty());
        let kept = s.extra_subs();
        assert_eq!(kept.len(), 1);
        assert!(!s.set_subs(kept));
        assert!(!s.live_is_empty() && s.sel.extras.is_empty());
        assert_eq!(s.selection_text().as_deref(), Some("abc"));
        // A plain click with no drag clears (it is no append).
        press(&mut s, 3, 0, false);
        assert!(s.live_is_empty() && s.extra_subs().is_empty());
    }

    /// Extras live and die with alacritty's selection: output that takes it takes them.
    #[test]
    fn extras_die_with_the_live_selection() {
        let mut s = Screen::new(10, 4);
        s.feed(b"abcx\r\ndefgh");
        press(&mut s, 0, 0, false);
        drag_to(&mut s, 0, 2);
        press(&mut s, 1, 0, true);
        drag_to(&mut s, 1, 3);
        assert_eq!(s.subs().len(), 2);
        // A reset clears alacritty's selection; `feed` syncs, so the extras go with it.
        s.feed(b"\x1bc");
        assert!(s.subs().is_empty());
        assert!(s.sel.extras.is_empty());
        assert_eq!(s.selection_text(), None);
    }

    /// Select Matches and the API make several: the last is the live one.
    #[test]
    fn set_subs_makes_the_last_live() {
        use crate::seltext::Sub;
        let mut s = Screen::new(10, 4);
        s.feed(b"abcx\r\ndefgh\r\nijk");
        let sub = |r1, c1, r2, c2, mode| Sub { start: Pos { row: r1, col: c1 }, end: Pos { row: r2, col: c2 }, mode, window: None, connected: false };
        let subs = vec![sub(0, 0, 0, 1, Mode::Character), sub(1, 2, 1, 3, Mode::Word), sub(2, 0, 2, 2, Mode::Character)];
        assert!(!s.set_subs(subs.clone()));
        assert_eq!(s.subs(), subs);
        assert_eq!(s.sel.live_mode, Mode::Character);
        assert_eq!(s.selection_text().as_deref(), Some("ab\nfg\nijk"));
        // Past the cap the rest is dropped.
        let many: Vec<Sub> = (0..crate::seltext::MAX_SUBS + 5).map(|_| sub(0, 0, 0, 0, Mode::Character)).collect();
        assert!(s.set_subs(many));
        assert_eq!(s.subs().len(), crate::seltext::MAX_SUBS);
        // Nothing clears.
        assert!(!s.set_subs(Vec::new()));
        assert!(s.subs().is_empty());
    }
}
