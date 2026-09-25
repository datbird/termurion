//! Smart Selection and Semantic History in the window: what a click selects and what a
//! Win-click opens.

use alacritty_terminal::index::Point;

use super::*;
use super::find_ui::{reveal, to_pos};
use crate::find::{self, Match};
use crate::smartsel;

/// Smart Selection looks this many logical lines each way from the click.
const RADIUS: usize = 2;

impl App {
    /// Selects the best Smart Selection match at `point`. Returns the rule and match, or
    /// None (and selects nothing) when no rule matches there.
    pub(super) fn smart_select(&mut self, id: PaneId, point: Point, ignore_newlines: bool) -> Option<smartsel::Found> {
        let rules = self.panes.get(&id).and_then(|p| self.store.get(&p.profile)).map(smartsel::rules)?;
        let p = self.panes.get_mut(&id)?;
        let at = to_pos(&p.screen, point);
        let (lines, _) = find::logical_lines_around(&p.screen, at.row, RADIUS);
        let joined = find::Logical::join(&lines, if ignore_newlines { "" } else { "\n" });
        let click = joined.offset_of(at)?;
        let found = smartsel::select_at(&joined.text, click, &rules)?;
        let m = Match { start: joined.pos_before(found.range.start), end: joined.pos_before(found.range.end - 1) };
        reveal(p, &m);
        Some(found)
    }
}
