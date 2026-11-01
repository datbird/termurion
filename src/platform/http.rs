//! The HTTP client for AI chat and the Keeper adapter. Windows uses WinHTTP; Unix has no client yet
//! (P1 picks one), so every request ends at once with an Unsupported error.

use std::sync::Arc;

use crate::ai_transport::Transport;

#[cfg(unix)]
use super::unix::http as imp;
#[cfg(windows)]
use super::windows::http as imp;

/// The system's HTTP transport.
pub fn system() -> Arc<dyn Transport> {
    imp::system()
}
