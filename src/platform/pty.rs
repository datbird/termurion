//! A child process on a pseudo terminal: ConPTY on Windows. Unix has no backend yet.

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

/// The pseudo terminal's loaded library: ConPTY's functions on Windows, nothing on Unix.
#[cfg(windows)]
pub type PtyApi = super::windows::conpty::Api;
#[cfg(unix)]
pub use super::unix::pty::PtyApi;

pub use crate::ssh_transport::Transport;
pub use imp::{Pty, console_mode_unless, spawn, spawn_notify, spawn_transport};

// Windows-only pieces (the loader, handoff, console attach) are reached through these re-exports.
#[cfg(windows)]
pub use super::windows::conpty;
#[cfg(windows)]
pub use super::windows::pty::{console_mode, from_handoff, spawn_flags};
#[cfg(windows)]
pub(crate) use super::windows::pty::{attached_unless, send_ctrl_event_attached};

/// The api the live tests spawn real shells with: the repo's shipped ConPTY on Windows.
/// Unix has no backend, so those tests fail there at the spawn, not at compile time.
#[cfg(test)]
pub fn test_api() -> PtyApi {
    imp::test_api()
}
