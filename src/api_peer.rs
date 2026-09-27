//! The Python API's peer check: the process that owns the other end of a loopback TCP
//! connection, which must run as this user at this elevation. iTerm2 compares the peer's euid
//! with its own (`S:457-472`); Windows has no euid on a socket, so the owner is found in the
//! TCP table (`GetExtendedTcpTable`) and its token is compared instead.

use std::net::{Ipv4Addr, SocketAddr};

/// One row of the TCP table: a connection end as seen from `local`, owned by `pid`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Row {
    pub local: SocketAddr,
    pub remote: SocketAddr,
    pub pid: u32,
}

pub const NO_OWNER: &str = "Could not find the process that owns the connection.";
pub const OTHER_ELEVATION: &str = "Peer's elevation is not equal to mine.";

/// iTerm2's "Peer's euid of %@ not equal to my euid of %@" with the user's SID for the euid.
pub fn other_user(theirs: &str, mine: &str) -> String {
    format!("Peer's user of {theirs} not equal to my user of {mine}")
}

/// A port from the table: network byte order in the low 16 bits.
pub fn port_from_table(dw: u32) -> u16 {
    (dw as u16).swap_bytes()
}

/// An IPv4 address from the table: the address's bytes as they lie in memory.
pub fn v4_from_table(dw: u32) -> Ipv4Addr {
    Ipv4Addr::from(dw.to_le_bytes())
}

fn same(a: SocketAddr, b: SocketAddr) -> bool {
    // Addresses and ports only: a V6 row's scope id and flow info are not the socket's.
    a.ip() == b.ip() && a.port() == b.port()
}

/// The pid owning the connection that reached `local` from `peer`: the row whose local end is
/// the peer's address and whose remote end is ours. The table also holds Tandem's own row for
/// the same connection (local = ours), which never names the peer. Rows owned by pid 0 (ends
/// in TIME_WAIT, owned by nobody) are skipped; matches with two different owners give None.
pub fn owner(rows: &[Row], peer: SocketAddr, local: SocketAddr) -> Option<u32> {
    let mut found = None;
    for r in rows.iter().filter(|r| r.pid != 0 && same(r.local, peer) && same(r.remote, local)) {
        match found {
            None => found = Some(r.pid),
            Some(pid) if pid == r.pid => {}
            Some(_) => return None,
        }
    }
    found
}

#[cfg(windows)]
mod win {
    use super::*;
    use std::net::{Ipv6Addr, SocketAddrV4, SocketAddrV6};

    use windows_sys::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, NO_ERROR};
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        GetExtendedTcpTable, MIB_TCP6ROW_OWNER_PID, MIB_TCP6TABLE_OWNER_PID, MIB_TCPROW_OWNER_PID, MIB_TCPTABLE_OWNER_PID,
        TCP_TABLE_OWNER_PID_CONNECTIONS,
    };
    use windows_sys::Win32::Networking::WinSock::{AF_INET, AF_INET6};

    use crate::winpipe;

    /// The raw table for one family, in u64 cells so the rows are aligned. The table can grow
    /// between the size call and the data call, so it retries a few times.
    fn table(family: u32) -> Result<(Vec<u64>, usize), String> {
        let mut size = 0u32;
        for _ in 0..8 {
            let mut buf = vec![0u64; (size as usize).div_ceil(8).max(1)];
            let mut len = (buf.len() * 8) as u32;
            let r = unsafe { GetExtendedTcpTable(buf.as_mut_ptr().cast(), &mut len, 0, family, TCP_TABLE_OWNER_PID_CONNECTIONS, 0) };
            match r {
                NO_ERROR => return Ok((buf, len as usize)),
                // Room for a few more rows that may appear before the next call.
                ERROR_INSUFFICIENT_BUFFER => size = len + 4096,
                e => return Err(format!("GetExtendedTcpTable failed: {}", std::io::Error::from_raw_os_error(e as i32))),
            }
        }
        Err("GetExtendedTcpTable failed: the table kept growing".into())
    }

    /// The rows of a table whose header is `dwNumEntries` followed by rows starting at `first`.
    unsafe fn rows_of<R: Copy>(buf: &[u64], len: usize, count: u32, first: *const R) -> Result<&[R], String> {
        let start = first as usize - buf.as_ptr() as usize;
        let need = start + count as usize * size_of::<R>();
        if need > len || need > buf.len() * 8 {
            return Err("GetExtendedTcpTable failed: the table is shorter than its row count".into());
        }
        Ok(unsafe { std::slice::from_raw_parts(first, count as usize) })
    }

    fn v4_rows() -> Result<Vec<Row>, String> {
        let (buf, len) = table(AF_INET as u32)?;
        let t = buf.as_ptr() as *const MIB_TCPTABLE_OWNER_PID;
        let rows: &[MIB_TCPROW_OWNER_PID] = unsafe { rows_of(&buf, len, (*t).dwNumEntries, (*t).table.as_ptr())? };
        Ok(rows
            .iter()
            .map(|r| Row {
                local: SocketAddr::V4(SocketAddrV4::new(v4_from_table(r.dwLocalAddr), port_from_table(r.dwLocalPort))),
                remote: SocketAddr::V4(SocketAddrV4::new(v4_from_table(r.dwRemoteAddr), port_from_table(r.dwRemotePort))),
                pid: r.dwOwningPid,
            })
            .collect())
    }

    fn v6_rows() -> Result<Vec<Row>, String> {
        let (buf, len) = table(AF_INET6 as u32)?;
        let t = buf.as_ptr() as *const MIB_TCP6TABLE_OWNER_PID;
        let rows: &[MIB_TCP6ROW_OWNER_PID] = unsafe { rows_of(&buf, len, (*t).dwNumEntries, (*t).table.as_ptr())? };
        Ok(rows
            .iter()
            .map(|r| Row {
                local: SocketAddr::V6(SocketAddrV6::new(Ipv6Addr::from(r.ucLocalAddr), port_from_table(r.dwLocalPort), 0, r.dwLocalScopeId)),
                remote: SocketAddr::V6(SocketAddrV6::new(Ipv6Addr::from(r.ucRemoteAddr), port_from_table(r.dwRemotePort), 0, r.dwRemoteScopeId)),
                pid: r.dwOwningPid,
            })
            .collect())
    }

    /// Every TCP connection on the machine, IPv4 then IPv6.
    pub fn tcp_rows() -> Result<Vec<Row>, String> {
        let mut rows = v4_rows()?;
        rows.extend(v6_rows()?);
        Ok(rows)
    }

    /// The pid of the process that owns the other end of the connection that reached `local`
    /// from `peer`, when it runs as this user at this elevation; otherwise the reason it is
    /// refused. Every failure refuses.
    pub fn check(peer: SocketAddr, local: SocketAddr) -> Result<u32, String> {
        // Only the peer's family: a machine without IPv6 still checks IPv4 peers.
        let rows = if peer.is_ipv4() { v4_rows()? } else { v6_rows()? };
        let pid = owner(&rows, peer, local).ok_or_else(|| NO_OWNER.to_string())?;
        let theirs = winpipe::process_sid(pid)?;
        let mine = winpipe::user_sid()?;
        if theirs != mine {
            return Err(other_user(&theirs, &mine));
        }
        if winpipe::process_elevated(pid)? != winpipe::elevated()? {
            return Err(OTHER_ELEVATION.to_string());
        }
        Ok(pid)
    }
}

#[cfg(windows)]
pub use win::{check, tcp_rows};

#[cfg(test)]
mod tests {
    use super::*;

    fn a(s: &str) -> SocketAddr {
        s.parse().unwrap()
    }

    #[test]
    fn owner_follows_the_connection() {
        for (client, server) in [("127.0.0.1:50000", "127.0.0.1:1912"), ("[::1]:50000", "[::1]:1912")] {
            let rows = [Row { local: a(client), remote: a(server), pid: 42 }, Row { local: a(server), remote: a(client), pid: 7 }];
            assert_eq!(owner(&rows, a(client), a(server)), Some(42));
            // Tandem's own row alone never names the peer.
            assert_eq!(owner(&rows[1..], a(client), a(server)), None);
            assert_eq!(owner(&[], a(client), a(server)), None);
            let other = client.replace("50000", "50001");
            assert_eq!(owner(&[Row { local: a(&other), remote: a(server), pid: 42 }], a(client), a(server)), None);
        }
        // A V6 row's scope id does not hide it.
        let scoped = SocketAddr::V6(std::net::SocketAddrV6::new(std::net::Ipv6Addr::LOCALHOST, 50000, 0, 1));
        assert_eq!(owner(&[Row { local: scoped, remote: a("[::1]:1912"), pid: 42 }], a("[::1]:50000"), a("[::1]:1912")), Some(42));
        // An end owned by nobody is no owner; two different owners are no owner.
        let (c, s) = (a("127.0.0.1:50000"), a("127.0.0.1:1912"));
        assert_eq!(owner(&[Row { local: c, remote: s, pid: 0 }], c, s), None);
        assert_eq!(owner(&[Row { local: c, remote: s, pid: 0 }, Row { local: c, remote: s, pid: 42 }], c, s), Some(42));
        assert_eq!(owner(&[Row { local: c, remote: s, pid: 42 }, Row { local: c, remote: s, pid: 43 }], c, s), None);
    }

    #[test]
    fn port_bytes_follow_windows() {
        assert_eq!(port_from_table(0x7807), 1912);
        assert_eq!(port_from_table(0xFFFF_7807), 1912);
        assert_eq!(v4_from_table(0x0100_007F), Ipv4Addr::LOCALHOST);
        assert_eq!(other_user("S-1-5-21-2", "S-1-5-21-1"), "Peer's user of S-1-5-21-2 not equal to my user of S-1-5-21-1");
        assert_eq!(NO_OWNER, "Could not find the process that owns the connection.");
        assert_eq!(OTHER_ELEVATION, "Peer's elevation is not equal to mine.");
    }

    #[cfg(windows)]
    #[test]
    fn this_process_is_its_own_peer() {
        use crate::winpipe;
        use std::net::{Ipv4Addr, Ipv6Addr, TcpListener, TcpStream};
        for ip in [std::net::IpAddr::V4(Ipv4Addr::LOCALHOST), std::net::IpAddr::V6(Ipv6Addr::LOCALHOST)] {
            let listener = TcpListener::bind(SocketAddr::new(ip, 0)).unwrap();
            let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
            let (accepted, _) = listener.accept().unwrap();
            let (peer, local) = (accepted.peer_addr().unwrap(), accepted.local_addr().unwrap());
            assert_eq!(peer, client.local_addr().unwrap());
            assert_eq!(check(peer, local), Ok(std::process::id()), "{ip}");
            let rows = tcp_rows().unwrap();
            assert_eq!(owner(&rows, peer, local), Some(std::process::id()));
            // The server's own row is there too, and is not the answer by accident.
            assert!(rows.iter().any(|r| r.local == local && r.remote == peer));
        }
        // A connection that does not exist has no owner.
        assert_eq!(check("127.0.0.1:1".parse().unwrap(), "127.0.0.1:2".parse().unwrap()), Err(NO_OWNER.to_string()));
        assert_eq!(winpipe::process_sid(std::process::id()), winpipe::user_sid());
        assert_eq!(winpipe::process_elevated(std::process::id()), winpipe::elevated());
        // A pid that is no process is refused, never passed.
        assert!(winpipe::process_sid(0).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn server_runs_the_peer_check() {
        use crate::api_server::{Event, Server};
        use std::io::Write;
        use std::sync::Arc;
        use std::time::{Duration, Instant};
        let s = Server::start(0, Arc::new(check), Arc::new(|_: &str| false), Arc::new(|| {})).unwrap();
        for addr in s.addrs() {
            let mut t = std::net::TcpStream::connect(addr).unwrap();
            let request = format!(
                "GET / HTTP/1.1\r\nHost: localhost:{}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
                 Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\
                 Sec-WebSocket-Protocol: api.iterm2.com\r\norigin: ws://localhost/\r\n\
                 x-iterm2-library-version: python 2.24\r\n\r\n",
                addr.port()
            );
            t.write_all(request.as_bytes()).unwrap();
            let deadline = Instant::now() + Duration::from_secs(10);
            let e = loop {
                if let Some(e) = s.try_event() {
                    break e;
                }
                assert!(Instant::now() < deadline, "no event");
                std::thread::sleep(Duration::from_millis(5));
            };
            match e {
                Event::Pending(p) => {
                    assert_eq!(p.peer_pid, std::process::id(), "{addr}");
                    s.verdict(p.conn, false);
                }
                e => panic!("{addr}: {e:?}"),
            }
        }
        s.stop();
    }
}
