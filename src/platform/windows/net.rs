//! Loopback sockets for the Python API on Windows: an exclusive bind (`SO_EXCLUSIVEADDRUSE`), the
//! IPv6 check, and a readiness poll that works where a blocking read cannot be woken.

use std::net::{SocketAddr, TcpListener, TcpStream};

/// iTerm2's listen backlog (`S:159-169`).
const BACKLOG: i32 = 5;

/// Error codes the API server's tests need, in this OS's numbers.
pub mod codes {
    use windows_sys::Win32::Networking::WinSock::{WSAEACCES, WSAEADDRINUSE, WSAEAFNOSUPPORT};
    pub const AF_UNSUPPORTED: i32 = WSAEAFNOSUPPORT;
    pub const ADDR_IN_USE: i32 = WSAEADDRINUSE;
    pub const ACCESS_DENIED: i32 = WSAEACCES;
}

/// A V6 bind that failed because the machine has no usable IPv6 loopback.
pub fn v6_unavailable(e: &std::io::Error) -> bool {
    use windows_sys::Win32::Networking::WinSock::{WSAEADDRNOTAVAIL, WSAEAFNOSUPPORT, WSAEPFNOSUPPORT, WSAEPROTONOSUPPORT};
    e.raw_os_error().is_some_and(|c| [WSAEAFNOSUPPORT, WSAEADDRNOTAVAIL, WSAEPFNOSUPPORT, WSAEPROTONOSUPPORT].contains(&c))
}

/// Waits up to `wait_ms` ms for `stream` to have something to read (data, an end or an
/// error: the read tells which) or, with `write`, room to write. Answers (read, write).
pub fn ready(stream: &TcpStream, write: bool, wait_ms: i32) -> std::io::Result<(bool, bool)> {
    use std::os::windows::io::AsRawSocket;
    use windows_sys::Win32::Networking::WinSock::{POLLERR, POLLHUP, POLLNVAL, POLLRDNORM, POLLWRNORM, WSAPOLLFD, WSAPoll};
    let events = if write { POLLRDNORM | POLLWRNORM } else { POLLRDNORM };
    let mut fd = WSAPOLLFD { fd: stream.as_raw_socket() as _, events, revents: 0 };
    match unsafe { WSAPoll(&mut fd, 1, wait_ms) } {
        n if n < 0 => Err(wsa_error()),
        0 => Ok((false, false)),
        _ => Ok((fd.revents & (POLLRDNORM | POLLHUP | POLLERR | POLLNVAL) != 0, fd.revents & POLLWRNORM != 0)),
    }
}

/// Calls `WSAStartup` once for the process (never undone; std does the same lazily).
fn winsock() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| unsafe {
        let mut data: windows_sys::Win32::Networking::WinSock::WSADATA = std::mem::zeroed();
        windows_sys::Win32::Networking::WinSock::WSAStartup(0x202, &mut data);
    });
}

fn wsa_error() -> std::io::Error {
    std::io::Error::from_raw_os_error(unsafe { windows_sys::Win32::Networking::WinSock::WSAGetLastError() })
}

/// Closes a socket unless released.
struct SocketGuard(windows_sys::Win32::Networking::WinSock::SOCKET);

impl Drop for SocketGuard {
    fn drop(&mut self) {
        unsafe { windows_sys::Win32::Networking::WinSock::closesocket(self.0) };
    }
}

/// A listener that no other socket can share (`SO_EXCLUSIVEADDRUSE`, the analog of iTerm2's
/// `socket.lock`), not inherited by children, V6 sockets V6 only. Refuses any address that is
/// not loopback.
pub fn bind_exclusive(addr: SocketAddr) -> std::io::Result<TcpListener> {
    use std::os::windows::io::FromRawSocket;
    use windows_sys::Win32::Networking::WinSock::{
        AF_INET, AF_INET6, INVALID_SOCKET, IPPROTO_IPV6, IPPROTO_TCP, IPV6_V6ONLY, SO_EXCLUSIVEADDRUSE, SOCK_STREAM, SOCKADDR, SOCKADDR_IN, SOCKADDR_IN6,
        SOL_SOCKET, WSA_FLAG_NO_HANDLE_INHERIT, WSA_FLAG_OVERLAPPED, WSASocketW, bind, listen, setsockopt,
    };
    if !addr.ip().is_loopback() {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("the Python API listens on loopback only, not {addr}")));
    }
    winsock();
    let on: i32 = 1;
    let set = |s, level, name| -> std::io::Result<()> {
        let rc = unsafe { setsockopt(s, level, name, &on as *const i32 as *const u8, std::mem::size_of::<i32>() as i32) };
        if rc != 0 { Err(wsa_error()) } else { Ok(()) }
    };
    unsafe {
        let family = if addr.is_ipv4() { AF_INET } else { AF_INET6 };
        let s = WSASocketW(family as i32, SOCK_STREAM, IPPROTO_TCP, std::ptr::null(), 0, WSA_FLAG_OVERLAPPED | WSA_FLAG_NO_HANDLE_INHERIT);
        if s == INVALID_SOCKET {
            return Err(wsa_error());
        }
        let guard = SocketGuard(s);
        set(s, SOL_SOCKET, SO_EXCLUSIVEADDRUSE)?;
        let rc = match addr {
            SocketAddr::V4(a) => {
                let mut sa: SOCKADDR_IN = std::mem::zeroed();
                sa.sin_family = AF_INET;
                sa.sin_port = a.port().to_be();
                sa.sin_addr.S_un.S_addr = u32::from_ne_bytes(a.ip().octets());
                bind(s, &sa as *const SOCKADDR_IN as *const SOCKADDR, std::mem::size_of::<SOCKADDR_IN>() as i32)
            }
            SocketAddr::V6(a) => {
                set(s, IPPROTO_IPV6, IPV6_V6ONLY)?;
                let mut sa: SOCKADDR_IN6 = std::mem::zeroed();
                sa.sin6_family = AF_INET6;
                sa.sin6_port = a.port().to_be();
                sa.sin6_addr.u.Byte = a.ip().octets();
                bind(s, &sa as *const SOCKADDR_IN6 as *const SOCKADDR, std::mem::size_of::<SOCKADDR_IN6>() as i32)
            }
        };
        if rc != 0 || listen(s, BACKLOG) != 0 {
            return Err(wsa_error());
        }
        std::mem::forget(guard);
        Ok(TcpListener::from_raw_socket(s as _))
    }
}
