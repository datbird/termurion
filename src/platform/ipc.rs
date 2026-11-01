//! The session host's local IPC: a named pipe on Windows. Unix has no backend yet.

#[cfg(unix)]
use super::unix::ipc as imp;
#[cfg(windows)]
use super::windows::ipc as imp;

pub use imp::{ConnectError, Listener, Pipe, connect, connect_owned, host_pipe_name, own_peer, secret_pipe_name, thread_peer, url_pipe_name};

/// A pipe or socket name no other test run shares, for the live host tests.
#[cfg(all(test, unix))]
pub(crate) use super::unix::ipc::test_name;
#[cfg(all(test, windows))]
pub(crate) use super::windows::ipc::tests::test_name;
