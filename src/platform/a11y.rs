//! Screen reader support. Windows answers UI Automation (`uia_win`, a Windows-only module); Unix has
//! no accessibility backend yet, so `available` reports Unsupported.

use super::Unsupported;

#[cfg(unix)]
use super::unix::a11y as imp;
#[cfg(windows)]
use super::windows::a11y as imp;

#[cfg(windows)]
pub use imp::{Host, PaneSnapshot, Request, WindowSnapshot, disconnect_all};

/// Ok when a screen reader can attach to a window here.
pub fn available() -> Result<(), Unsupported> {
    imp::available()
}
