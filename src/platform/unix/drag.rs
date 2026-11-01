//! No drag source on Unix yet.

use super::super::Unsupported;

pub fn drag_text(_text: &str) -> Result<bool, Unsupported> {
    super::super::unsupported("Dragging text out of the window")
}
