//! The selection tools in the window: Replace Selection, Clear to Start of Selection,
//! Selection Respects Soft Boundaries, and the selection endpoint moves.

use alacritty_terminal::index::{Line, Point};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::cell::Flags;

use super::*;
use super::find_ui::{to_point, to_pos};
use crate::find::Pos;
use crate::seltools::{self, End};

/// Menu items with a check mark that milestone 4c adds, by selector.
pub(super) const CHECKED: &[&str] = &["toggleSelectionRespectsSoftBoundaries:"];

impl App {
    /// Whether a checkable menu item shows its check mark.
    pub(super) fn is_checked(&self, selector: &str) -> bool {
        match selector {
            "toggleSelectionRespectsSoftBoundaries:" => self.globals.bool("Selection Respects Soft Boundaries"),
            _ => false,
        }
    }

    fn selection_pos(&self, id: PaneId) -> Option<(Pos, Pos)> {
        let p = self.panes.get(&id)?;
        let term = p.screen.term();
        let r = term.selection.as_ref()?.to_range(term)?;
        Some((to_pos(&p.screen, r.start), to_pos(&p.screen, r.end)))
    }

    fn set_simple_selection(&mut self, id: PaneId, a: Pos, b: Pos) {
        let Some(p) = self.panes.get_mut(&id) else { return };
        let mut sel = Selection::new(SelectionType::Simple, to_point(&p.screen, a), Side::Left);
        sel.update(to_point(&p.screen, b), Side::Right);
        p.screen.term_mut().selection = Some(sel);
    }

    /// The selection tools' menu commands. Returns false for other selectors.
    pub(super) fn run_seltools_command(&mut self, selector: &str) -> bool {
        let Some(id) = self.focused_id() else { return false };
        match selector {
            "replaceSelectionWithPrettyPrintedJSON:" => self.replace_selection(id, seltools::pretty_json),
            "replaceSelectionWithBase64Encoded:" => self.replace_selection(id, |t| Ok(seltools::base64_encode(t))),
            "replaceSelectionWithBase64Decoded:" => self.replace_selection(id, seltools::base64_decode),
            "clearToStartOfSelection:" => self.clear_to_start_of_selection(id),
            "toggleSelectionRespectsSoftBoundaries:" => {
                let on = !self.globals.bool("Selection Respects Soft Boundaries");
                self.globals.set("Selection Respects Soft Boundaries", toml::Value::Boolean(on));
                self.save_globals();
            }
            _ => return false,
        }
        self.sync_checks();
        self.redraw();
        true
    }

    /// Replace Selection: the selected whole lines are replaced in Tandem's buffer (not
    /// sent to the shell), as iTerm2 does.
    fn replace_selection(&mut self, id: PaneId, f: impl Fn(&str) -> Result<String, String>) {
        let Some((a, b)) = self.selection_pos(id) else { return self.banners.set("action", "Select whole lines first.") };
        let Some(p) = self.panes.get_mut(&id) else { return };
        if p.live.is_some() {
            return self.banners.set("action", "Replace Selection does not work in a Filter or Zoom view.");
        }
        let (pa, pb) = (to_point(&p.screen, a), to_point(&p.screen, b));
        let grid = p.screen.term().grid();
        let cols = grid.columns();
        let row = &grid[pb.line];
        let wraps = row[Column(cols - 1)].flags.contains(Flags::WRAPLINE);
        let last_end = (0..cols).rev().find(|&c| row[Column(c)].c != ' ').unwrap_or(0);
        if wraps || !seltools::whole_lines(a, b, last_end) {
            return self.banners.set("action", "Replace Selection needs whole lines, from the start of the first.");
        }
        let text = p.screen.term().bounds_to_string(pa, Point::new(pb.line, Column(cols - 1)));
        match f(text.trim_end_matches('\n')) {
            Err(e) => self.banners.set("action", e),
            Ok(new) => {
                let lines: Vec<String> = new.lines().map(str::to_string).collect();
                let rows = p.screen.text_rows(&lines);
                p.screen.splice_rows(pa.line.0, pb.line.0, rows);
            }
        }
    }

    /// Clear to Start of Selection: everything above the selection's first line goes.
    fn clear_to_start_of_selection(&mut self, id: PaneId) {
        let Some((a, _)) = self.selection_pos(id) else { return };
        let Some(p) = self.panes.get_mut(&id) else { return };
        p.end_view();
        let first = to_point(&p.screen, a).line.0;
        let top = -(p.screen.term().grid().history_size() as i32);
        if first > top {
            p.screen.splice_rows(top, first - 1, Vec::new());
        }
    }

    /// With Selection Respects Soft Boundaries on, a one-row selection made by a double
    /// click or Smart Selection stops at tmux and vim pane dividers.
    pub(super) fn clip_selection_to_dividers(&mut self, id: PaneId, click: Point) {
        if !self.globals.bool("Selection Respects Soft Boundaries") {
            return;
        }
        let Some((a, b)) = self.selection_pos(id) else { return };
        if a.row != b.row {
            return;
        }
        let Some(p) = self.panes.get(&id) else { return };
        let grid = p.screen.term().grid();
        let (top, bottom) = (-(grid.history_size() as i32), grid.screen_lines() as i32 - 1);
        let line = to_point(&p.screen, a).line.0;
        let (from, to) = ((line - seltools::DIVIDER_RUN as i32).max(top), (line + seltools::DIVIDER_RUN as i32).min(bottom));
        let rows: Vec<Vec<char>> = (from..=to).map(|l| (0..grid.columns()).map(|c| grid[Line(l)][Column(c)].c).collect()).collect();
        let dividers = seltools::divider_columns(&rows, (line - from) as usize);
        let (lo, hi) = seltools::clip_to_dividers(a.col, b.col, click.column.0, &dividers);
        if (lo, hi) != (a.col, b.col) && lo <= hi {
            self.set_simple_selection(id, Pos { row: a.row, col: lo }, Pos { row: a.row, col: hi });
        }
    }

    /// Key actions 45 to 48: one end of the selection moves `count` cells.
    pub(super) fn move_selection_end(&mut self, id: PaneId, action: i64, count: &str) {
        let n = count.trim().parse::<usize>().unwrap_or(1);
        let Some(sel) = self.selection_pos(id) else { return };
        let Some(p) = self.panes.get(&id) else { return };
        let grid = p.screen.term().grid();
        let last_row = grid.history_size() + grid.screen_lines() - 1;
        let (which, left) = match action {
            45 => (End::End, true),
            46 => (End::End, false),
            47 => (End::Start, true),
            _ => (End::Start, false),
        };
        let (a, b) = seltools::move_endpoint(sel, which, left, n, grid.columns(), last_row);
        self.set_simple_selection(id, a, b);
        self.copy_selection(id);
    }
}
