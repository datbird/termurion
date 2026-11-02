//! The session host's local IPC: a named pipe on Windows. Unix has no backend yet.

#[cfg(unix)]
use super::unix::ipc as imp;
#[cfg(windows)]
use super::windows::ipc as imp;

pub use imp::{ConnectError, Listener, Pipe, connect, connect_owned, elevated, host_pipe_name, own_peer, secret_pipe_name, thread_peer, url_pipe_name, user_sid};

/// A pipe or socket name no other test run shares, for the live host tests.
#[cfg(all(test, unix))]
pub(crate) use super::unix::ipc::test_name;
#[cfg(all(test, windows))]
pub(crate) use super::windows::ipc::tests::test_name;

/// The pid of the process that owns the other end of a local API connection, when its token passes the
/// same-user check; otherwise the reason it is refused. Every failure refuses, and Unix, which has no
/// check yet, refuses everything (Unsupported).
pub fn check_peer(peer: std::net::SocketAddr, local: std::net::SocketAddr) -> Result<u32, String> {
    imp::check_peer(peer, local)
}

/// Ok where the allow-all file of the automation API can be written; Unix reports Unsupported (its
/// reader denies, so a written file would never count).
pub fn available() -> Result<(), super::Unsupported> {
    imp::available()
}

/// Makes this user the owner of `file` (an elevated process's new files belong to Administrators).
/// Unix reports Unsupported, which a caller treats as a refusal.
pub fn set_file_owner_to_user(file: &std::fs::File) -> Result<(), String> {
    imp::set_file_owner_to_user(file)
}

/// Creates the new file `path` for writing, owned by this user when `set_owner` (an elevated
/// process). Fails when it exists. Unix reports Unsupported.
pub fn create_new_for_write(path: &str, set_owner: bool) -> Result<std::fs::File, String> {
    imp::create_new_for_write(path, set_owner)
}
