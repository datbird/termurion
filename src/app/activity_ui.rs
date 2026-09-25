//! Output and idle per session (milestone 6b task 3 fills this in).

use super::*;

impl App {
    /// Whether any of these panes shows the new-output or the idle indicator.
    pub(super) fn activity_icons(&self, _panes: &[PaneId]) -> (bool, bool) {
        (false, false)
    }
}
