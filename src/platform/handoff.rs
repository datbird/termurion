//! The default-terminal handoff: Windows lets console hosts hand a console to Termurion through a COM
//! class (`handoff_win`, a Windows-only module). Unix has no such hand-off, so no `Handoff` can exist
//! there and `can_serve` says no.

use super::pty::{Notify, Pty, PtyApi};

#[cfg(unix)]
use super::unix::handoff as imp;
#[cfg(windows)]
use super::windows::handoff as imp;

pub use imp::{Handoff, Server};

/// Whether this build can register the hand-off class (Windows: the pseudoconsole library exports the pack call).
pub fn can_serve(api: &PtyApi) -> bool {
    imp::can_serve(api)
}

/// Registers the hand-off class. `send` gets every vetted hand-off.
pub fn register(send: Box<dyn Fn(Handoff) + Send + Sync>, api: &PtyApi) -> Result<Server, String> {
    imp::register(send, api)
}

/// Raised by the one process teardown: calls still arriving are refused from here on.
pub fn begin_closing() {
    imp::begin_closing()
}

/// The pty of a handed-off console, resized later by the caller, with the title the console came with
/// (empty when it came without a safe one). Termurion owns the console's handles from here.
pub fn into_pty(api: &PtyApi, h: Handoff, notify: Notify) -> Result<(Pty, String), String> {
    imp::into_pty(api, h, notify)
}
