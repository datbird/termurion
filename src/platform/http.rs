//! The HTTP client for AI chat and the Keeper adapter. Windows uses WinHTTP; Unix has no client yet
//! (P1 picks one), so every request ends at once with an Unsupported error.

use std::sync::Arc;

use crate::ai_transport::Transport;

#[cfg(unix)]
use super::unix::http as imp;
#[cfg(windows)]
use super::windows::http as imp;

/// A fetch of a URL as text: the body, or the tool's exit code and message.
pub type Fetched = Result<String, (i32, String)>;

/// Loads `url` with the system's fetch tool: the body, or the exit code and what the tool wrote. The
/// tool's arguments are built here and in the backend only (`remote_prefs::curl_args`, which allows
/// http and https and nothing else), never by a caller. Unix reports Unsupported (P1 picks a client).
pub fn fetch_text(url: &str, timeout: Option<f64>) -> Fetched {
    imp::fetch_text(url, timeout)
}

/// The system's HTTP transport.
pub fn system() -> Arc<dyn Transport> {
    imp::system()
}
