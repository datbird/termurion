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

/// Loads `url` with the system's fetch tool, run with `args` (the caller's own, `remote_prefs::curl_args`):
/// the body, or the exit code and what the tool wrote. Unix reports Unsupported (P1 picks a client).
pub fn fetch_text(url: &str, args: &[String]) -> Fetched {
    imp::fetch_text(url, args)
}

/// The system's HTTP transport.
pub fn system() -> Arc<dyn Transport> {
    imp::system()
}
