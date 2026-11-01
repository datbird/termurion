//! Edge-drag snapping to the cell grid. Windows answers `WM_SIZING` in a subclass; on Unix the
//! window manager runs the drag and winit gives no hook to change its size mid-drag, so these
//! leave the drag alone.

use crate::chrome::Snap;

#[cfg(unix)]
use super::unix::sizing as imp;
#[cfg(windows)]
use super::windows::sizing as imp;

/// What a window's last frame published for its next drag: its grid, its window style and
/// `DisableWindowSizeSnap`, from which the subclass decides the snapped axes per drag.
#[derive(Clone, Copy)]
pub struct Grid {
    pub snap: Snap,
    pub style: i64,
    pub disabled: bool,
}

/// Subclasses a new terminal window so edge drags snap.
pub fn install(hwnd: isize) {
    imp::install(hwnd)
}

/// The window's grid for the next drag, or None when it keeps the size the user drags to.
pub fn publish(hwnd: isize, snap: Option<Grid>) {
    imp::publish(hwnd, snap)
}
