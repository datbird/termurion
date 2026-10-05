//! The text cursor on screen: what `scene::pane_with` is told to draw for each pane.

use super::*;
use crate::cursor_look::{CursorLook, Geometry};

impl App {
    /// How the cursor of `pane` is drawn. `visible` is whether it is shown at all (the blink
    /// phase and an Auto Composer's cover). `App.cursor` is the mouse pointer, never read here.
    pub(super) fn cursor_look_for(&self, _pane: PaneId, _focused: bool, visible: bool) -> CursorLook {
        CursorLook { geometry: Geometry::from_globals(&self.globals, self.scale(), self.base_cell()), ..CursorLook::shown(visible) }
    }
}
