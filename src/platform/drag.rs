//! Dragging selected text out of the window to another app. Windows runs OLE's drag loop; Unix has no
//! drag source yet, so it says so where the person tried.

use super::Unsupported;

#[cfg(unix)]
use super::unix::drag as imp;
#[cfg(windows)]
use super::windows::drag as imp;

/// Drags `text` out of the window; blocks until the drop ends on the UI thread. True when a target
/// took it.
pub fn drag_text(text: &str) -> Result<bool, Unsupported> {
    imp::drag_text(text)
}
