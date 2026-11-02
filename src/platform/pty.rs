//! A child process on a pseudo terminal: ConPTY on Windows, a Unix pty elsewhere.

use std::sync::Arc;

#[cfg(unix)]
use super::unix::pty as imp;
#[cfg(windows)]
use super::windows::pty as imp;

/// Called from background threads when a pty has new output or its child exits.
pub type Notify = Arc<dyn Fn() + Send + Sync>;

/// The result of a non-blocking read.
pub enum Read {
    Data(Vec<u8>),
    Empty,
    /// The output pipe closed. ConPTY closes it only after the pseudoconsole closes,
    /// so a child exit shows up in `exit_code`, not here.
    Closed,
}

/// The pseudo terminal's loaded library: ConPTY's functions on Windows, nothing to load on Unix.
#[cfg(windows)]
pub type PtyApi = super::windows::conpty::Api;
#[cfg(unix)]
pub use super::unix::pty::PtyApi;

pub use crate::ssh_transport::Transport;
pub use imp::{Pty, console_mode_unless, default_shell, inherits_term, spawn, spawn_notify, spawn_transport};

// Windows-only pieces (the loader, handoff, console attach) are reached through these re-exports.
#[cfg(windows)]
pub use super::windows::conpty;
#[cfg(windows)]
pub use super::windows::pty::{console_mode, from_handoff, spawn_flags};
#[cfg(windows)]
pub(crate) use super::windows::pty::{attached_unless, send_ctrl_event_attached};

/// Loads the pseudo terminal library the app spawns shells with. Windows takes the ConPTY shipped
/// beside the exe, else the copy built into Windows (the note says so for a banner); Unix has none to
/// load.
pub fn load_for_app(exe_dir: &std::path::Path) -> Result<(PtyApi, Option<String>), String> {
    imp::load_for_app(exe_dir)
}

/// The loaded library has its pack (the default-terminal rule); never on Unix.
pub fn api_has_pack(api: &PtyApi) -> bool {
    imp::api_has_pack(api)
}

/// The default-terminal start rule: the shipped ConPTY has its pack (Windows only).
pub fn shipped_has_pack(exe_dir: &std::path::Path) -> bool {
    imp::shipped_has_pack(exe_dir)
}

/// The api the live tests spawn real shells with: the repo's shipped ConPTY on Windows.
#[cfg(test)]
pub fn test_api() -> PtyApi {
    imp::test_api()
}

/// Reads until `needle` appears, taking chunks from `next` (which waits up to the time it is
/// given). Ok with everything read on a hit, Err with everything read on timeout. Both backends'
/// `Pty::read_until` call it.
pub(crate) fn read_until_with(mut next: impl FnMut(std::time::Duration) -> Option<Vec<u8>>, needle: &[u8], wait: std::time::Duration) -> Result<Vec<u8>, Vec<u8>> {
    let end = std::time::Instant::now() + wait;
    let mut all = Vec::new();
    loop {
        if all.windows(needle.len()).any(|w| w == needle) {
            return Ok(all);
        }
        let left = end.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() {
            return Err(all);
        }
        if let Some(chunk) = next(left.min(std::time::Duration::from_millis(50))) {
            all.extend(chunk);
        }
    }
}
