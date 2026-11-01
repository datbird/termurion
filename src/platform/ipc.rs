//! The session host's local IPC: a named pipe on Windows. Unix has no backend yet.

#[cfg(unix)]
use super::unix::ipc as imp;
#[cfg(windows)]
use super::windows::ipc as imp;

pub use imp::{ConnectError, Listener, Pipe, connect, connect_owned, host_pipe_name, own_peer, secret_pipe_name, thread_peer, url_pipe_name};
