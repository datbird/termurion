//! Loopback sockets for the Python API server: the exclusive bind, the IPv6 check and the readiness poll.

use std::net::{SocketAddr, TcpListener, TcpStream};

#[cfg(unix)]
use super::unix::net as imp;
#[cfg(windows)]
use super::windows::net as imp;

pub use imp::codes;

/// A listener on loopback that no other socket can share. Refuses any address that is not loopback.
pub fn bind_exclusive(addr: SocketAddr) -> std::io::Result<TcpListener> {
    imp::bind_exclusive(addr)
}

/// A V6 bind that failed because the machine has no usable IPv6 loopback.
pub fn v6_unavailable(e: &std::io::Error) -> bool {
    imp::v6_unavailable(e)
}

/// Waits up to `wait_ms` ms for `stream` to have something to read (data, an end or an error: the
/// read tells which) or, with `write`, room to write. Answers (read, write).
pub fn ready(stream: &TcpStream, write: bool, wait_ms: i32) -> std::io::Result<(bool, bool)> {
    imp::ready(stream, write, wait_ms)
}

/// This computer's full DNS name; the short name when the full one is not known, "" when neither is.
pub fn local_host() -> String {
    imp::local_host()
}
