//! Folds in the window: Fold Selected Lines, Fold All, and unfolding with a click.

use super::*;
use alacritty_terminal::index::Point;

impl App {
    /// The fold menu commands. Returns false for other selectors.
    pub(super) fn run_fold_command(&mut self, selector: &str) -> bool {
        let Some(id) = self.focused_id() else { return false };
        match selector {
            "foldSelection:" => self.fold_selection(id),
            "foldAll:" => self.fold_all(id),
            _ => return false,
        }
        self.redraw();
        true
    }

    /// Fold Selected Lines: folds the selected lines, or unfolds the folds among them.
    fn fold_selection(&mut self, id: PaneId) {
        let Some(p) = self.panes.get_mut(&id) else { return };
        let term = p.screen.term();
        let Some(r) = term.selection.as_ref().and_then(|s| s.to_range(term)) else {
            return self.banners.set("action", "Select the lines to fold first.");
        };
        let (a, b) = (r.start.line.0, r.end.line.0);
        let folds: Vec<u32> = (a..=b).filter_map(|l| p.screen.fold_at(l)).collect();
        if !folds.is_empty() {
            for f in folds {
                p.screen.unfold(f);
            }
        } else if p.screen.fold(a, b).is_none() {
            self.banners.set("action", "Tandem cannot fold the cursor's line, or while a full-screen app runs.");
        }
    }

    /// Fold All: everything above the cursor's line folds into one line captioned with
    /// the first and last commands in it.
    fn fold_all(&mut self, id: PaneId) {
        let Some(p) = self.panes.get_mut(&id) else { return };
        let grid = p.screen.term().grid();
        let (top, cursor, cols) = (-(grid.history_size() as i32), grid.cursor.point.line.0, grid.columns());
        let mut first = top;
        while first < cursor && p.screen.fold_at(first).is_some() {
            first += 1;
        }
        let last = cursor - 1;
        if first > last {
            return;
        }
        let commands: Vec<(String, Option<std::time::SystemTime>)> = p
            .screen
            .marks_in_order()
            .into_iter()
            .filter(|(l, m)| (first..=last).contains(l) && !m.command.is_empty())
            .map(|(_, m)| (m.command.clone(), m.start))
            .collect();
        let caption = match (commands.first(), commands.last()) {
            (Some((c1, t1)), Some((c2, _))) => {
                let time = t1.map(crate::stamps::clock_time).unwrap_or_default();
                Some(crate::fold::fold_all_caption(&time, c1, c2, (last - first + 1) as usize, cols))
            }
            _ => None,
        };
        if p.screen.fold_as(first, last, caption).is_none() {
            self.banners.set("action", "Tandem cannot fold while a full-screen app runs.");
        }
    }

    /// A plain click on a fold's summary line unfolds it. Returns true when it did.
    pub(super) fn click_unfold(&mut self, id: PaneId, point: Point) -> bool {
        let Some(p) = self.panes.get_mut(&id) else { return false };
        let Some(f) = p.screen.fold_at(point.line.0) else { return false };
        p.screen.unfold(f);
        self.redraw();
        true
    }

    /// The fold at a point, for the right-click menu.
    pub(super) fn fold_at_point(&self, id: PaneId, point: Point) -> Option<u32> {
        self.panes.get(&id).and_then(|p| p.screen.fold_at(point.line.0))
    }

    pub(super) fn unfold(&mut self, id: PaneId, fold: u32) {
        if let Some(p) = self.panes.get_mut(&id) {
            p.screen.unfold(fold);
        }
        self.redraw();
    }
}
