//! Loopback sockets for the Python API on Unix. A listener bound to a loopback address with no
//! SO_REUSEPORT cannot be shared while it listens; the readiness poll is `poll(2)`.

use std::net::{SocketAddr, TcpListener, TcpStream};
use std::os::fd::AsRawFd;

/// Error codes the API server's tests need, in this OS's numbers.
pub mod codes {
    pub const AF_UNSUPPORTED: i32 = libc::EAFNOSUPPORT;
    pub const ADDR_IN_USE: i32 = libc::EADDRINUSE;
    pub const ACCESS_DENIED: i32 = libc::EACCES;
}

/// Loopback only. std binds and listens in one step, close-on-exec, and never sets SO_REUSEPORT, so
/// a port with a live listener is refused.
pub fn bind_exclusive(addr: SocketAddr) -> std::io::Result<TcpListener> {
    if !addr.ip().is_loopback() {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("the Python API listens on loopback only, not {addr}")));
    }
    TcpListener::bind(addr)
}

pub fn v6_unavailable(e: &std::io::Error) -> bool {
    e.raw_os_error().is_some_and(|c| [libc::EAFNOSUPPORT, libc::EADDRNOTAVAIL, libc::EPFNOSUPPORT, libc::EPROTONOSUPPORT].contains(&c))
}

pub fn ready(stream: &TcpStream, write: bool, wait_ms: i32) -> std::io::Result<(bool, bool)> {
    let events = if write { libc::POLLIN | libc::POLLOUT } else { libc::POLLIN };
    let mut fd = libc::pollfd { fd: stream.as_raw_fd(), events, revents: 0 };
    // SAFETY: one valid pollfd, and the count says so.
    match unsafe { libc::poll(&mut fd, 1, wait_ms) } {
        n if n < 0 => {
            let e = std::io::Error::last_os_error();
            // A signal is not a failure: nothing is ready yet.
            if e.kind() == std::io::ErrorKind::Interrupted { Ok((false, false)) } else { Err(e) }
        }
        0 => Ok((false, false)),
        _ => Ok((fd.revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0, fd.revents & libc::POLLOUT != 0)),
    }
}

pub fn local_host() -> String {
    let mut buf = [0u8; 256];
    if unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) } != 0 {
        return std::env::var("HOSTNAME").unwrap_or_default();
    }
    let n = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[..n]).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    #[test]
    fn unix_bind_is_loopback_only_and_exclusive() {
        assert!(bind_exclusive(SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0))).is_err());
        assert!(bind_exclusive(SocketAddr::from((Ipv6Addr::UNSPECIFIED, 0))).is_err());
        let first = bind_exclusive(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).unwrap();
        assert!(bind_exclusive(first.local_addr().unwrap()).is_err());
    }

    #[test]
    fn unix_ipv6_codes() {
        assert!(v6_unavailable(&std::io::Error::from_raw_os_error(codes::AF_UNSUPPORTED)));
        assert!(!v6_unavailable(&std::io::Error::from_raw_os_error(codes::ADDR_IN_USE)));
    }
}
