//! The Python API's peer check: the process that owns the other end of a loopback TCP
//! connection, which must run as this user at this elevation and integrity level, unsandboxed. iTerm2 compares the peer's euid
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

pub const OTHER_INTEGRITY: &str = "Peer's integrity level is not equal to mine.";
pub const APP_CONTAINER: &str = "Peer runs in an app container.";
pub const RESTRICTED: &str = "Peer runs with a restricted token.";

/// Mandatory integrity levels: the last subauthority of a token's label SID.
pub const LOW_RID: u32 = 0x1000;
pub const MEDIUM_RID: u32 = 0x2000;
pub const HIGH_RID: u32 = 0x3000;

/// What the peer check reads from a process's token, all from one handle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Token {
    pub sid: String,
    pub elevated: bool,
    /// The integrity level's RID (`LOW_RID`, `MEDIUM_RID`, ...).
    pub integrity: u32,
    pub app_container: bool,
    pub restricted: bool,
}

/// Whether the peer's token may drive Termurion's: the same user (iTerm2's euid compare), not
/// sandboxed (an AppContainer or a restricted token), at the same elevation and the same
/// integrity level, so no less trusted process can type into Termurion's shells.
pub fn judge(theirs: &Token, mine: &Token) -> Result<(), String> {
    if theirs.sid != mine.sid {
        return Err(other_user(&theirs.sid, &mine.sid));
    }
    if theirs.app_container {
        return Err(APP_CONTAINER.to_string());
    }
    if theirs.restricted {
        return Err(RESTRICTED.to_string());
    }
    if theirs.elevated != mine.elevated {
        return Err(OTHER_ELEVATION.to_string());
    }
    if theirs.integrity != mine.integrity {
        return Err(OTHER_INTEGRITY.to_string());
    }
    Ok(())
}

pub const OTHER_SESSION: &str = "Peer runs in another Windows session.";

/// A peer's token and its Windows session (None when it could not be read).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Peer {
    pub token: Token,
    pub session: Option<u32>,
}

/// `judge`, and the same Windows session (an unreadable one is another). The one rule for every
/// local caller that may drive Termurion: the launch pipe's server and the console handoff's caller.
pub fn judge_peer(theirs: &Peer, mine: &Peer) -> Result<(), String> {
    judge(&theirs.token, &mine.token)?;
    match (theirs.session, mine.session) {
        (Some(a), Some(b)) if a == b => Ok(()),
        _ => Err(OTHER_SESSION.to_string()),
    }
}

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
/// the peer's address and whose remote end is ours. The table also holds Termurion's own row for
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

/// The pid listening on `port` among the listener table's rows (pid 0 skipped); the first found.
pub fn listener(rows: &[Row], port: u16) -> Option<u32> {
    rows.iter().find(|r| r.pid != 0 && r.local.port() == port).map(|r| r.pid)
}

/// The text naming a process, for "port in use" (Termurion's own).
pub fn process_label(name: Option<&str>, pid: u32) -> String {
    match name {
        Some(n) => format!("{n} (pid {pid})"),
        None => format!("pid {pid}"),
    }
}

// The TCP owner lookups live under platform (Windows reads the TCP table, Unix says Unsupported).
pub use crate::platform::tcp_owner::{check, listen_rows, process_name, tcp_rows};

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
            // Termurion's own row alone never names the peer.
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
    fn listener_names_the_port_owner() {
        let rows = [
            Row { local: a("127.0.0.1:80"), remote: a("0.0.0.0:0"), pid: 5 },
            Row { local: a("0.0.0.0:1912"), remote: a("0.0.0.0:0"), pid: 0 },
            Row { local: a("[::1]:1912"), remote: a("[::]:0"), pid: 9 },
        ];
        assert_eq!(listener(&rows, 1912), Some(9));
        assert_eq!(listener(&rows, 81), None);
        assert_eq!(process_label(Some("python.exe"), 9), "python.exe (pid 9)");
        assert_eq!(process_label(None, 9), "pid 9");
    }

    #[cfg(windows)]
    #[test]
    fn this_process_owns_its_listener() {
        let l = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = l.local_addr().unwrap().port();
        assert_eq!(listener(&listen_rows().unwrap(), port), Some(std::process::id()));
        assert!(process_name(std::process::id()).is_some_and(|n| n.ends_with(".exe") && !n.contains('\\')));
    }

    #[test]
    fn tokens_must_match_mine() {
        let mine = Token { sid: "S-1-5-21-1".into(), elevated: false, integrity: MEDIUM_RID, app_container: false, restricted: false };
        assert_eq!(judge(&mine.clone(), &mine), Ok(()));
        let other = Token { sid: "S-1-5-21-2".into(), ..mine.clone() };
        assert_eq!(judge(&other, &mine), Err(other_user("S-1-5-21-2", "S-1-5-21-1")));
        assert_eq!(judge(&Token { elevated: true, integrity: HIGH_RID, ..mine.clone() }, &mine), Err(OTHER_ELEVATION.to_string()));
        // A sandboxed process of the same user: Low integrity, or an AppContainer or a
        // restricted token at any level, is refused.
        assert_eq!(judge(&Token { integrity: LOW_RID, ..mine.clone() }, &mine), Err(OTHER_INTEGRITY.to_string()));
        assert_eq!(judge(&Token { integrity: LOW_RID, app_container: true, ..mine.clone() }, &mine), Err(APP_CONTAINER.to_string()));
        assert_eq!(judge(&Token { restricted: true, ..mine.clone() }, &mine), Err(RESTRICTED.to_string()));
        // A higher level than mine is refused too: the levels must be equal.
        assert_eq!(judge(&Token { integrity: HIGH_RID, ..mine.clone() }, &mine), Err(OTHER_INTEGRITY.to_string()));
        let high = Token { elevated: true, integrity: HIGH_RID, ..mine.clone() };
        assert_eq!(judge(&high.clone(), &high), Ok(()));
        assert_eq!(judge(&mine, &high), Err(OTHER_ELEVATION.to_string()));
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
        use crate::platform::windows::ipc;
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
        let mine = ipc::own_token_facts().unwrap();
        assert_eq!(ipc::process_token_facts(std::process::id()), Ok(mine.clone()));
        assert_eq!(Ok(mine.sid.clone()), ipc::user_sid());
        assert_eq!(Ok(mine.elevated), ipc::elevated());
        assert!(!mine.app_container && !mine.restricted);
        assert!(mine.integrity >= MEDIUM_RID, "the suite runs at Medium or higher");
        // A pid that is no process is refused, never passed.
        assert!(ipc::process_token_facts(0).is_err());
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
