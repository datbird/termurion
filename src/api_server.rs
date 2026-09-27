//! The Python API's sockets: an exclusive listener on each loopback family (never any other
//! address), a thread per connection that runs the peer check, reads and checks the request
//! (`api_http`), waits for the UI thread's verdict and then carries binary WebSocket frames with
//! tungstenite (framing only). Events reach the UI thread through a channel and the wake
//! (`S:457-608`).

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use tungstenite::protocol::Role;
use tungstenite::{Message, WebSocket};

use crate::api_http;
use crate::pty::Notify;

/// Maps a connection (peer, local) to the peer's pid, or refuses it with a reason (Task 3).
pub type PeerCheck = Arc<dyn Fn(SocketAddr, SocketAddr) -> Result<u32, String> + Send + Sync>;

/// Consumes a cookie, answering whether it was known (Task 4's jar).
pub type CookieCheck = Arc<dyn Fn(&str) -> bool + Send + Sync>;

/// A connection whose request passed every check, waiting for `Server::verdict`.
#[derive(Clone, Debug)]
pub struct Pending {
    pub conn: u64,
    pub upgrade: api_http::Upgrade,
    pub peer_pid: u32,
}

#[derive(Debug)]
pub enum Event {
    Pending(Pending),
    /// A binary frame's payload.
    Message { conn: u64, bytes: Vec<u8> },
    /// An upgraded connection ended, whoever ended it.
    Closed { conn: u64 },
    /// A connection refused before any verdict: by the peer check, an unreadable request or an
    /// upgrade check. `key` is the client's `x-iterm2-key` where iTerm2 posts it.
    Rejected { key: Option<String>, reason: String },
}

/// iTerm2 queues at most 16 read chunks per connection (`W:65-66`); Tandem bounds the events
/// waiting for the UI thread the same way, so a flooding client meets TCP back pressure.
const EVENT_QUEUE: usize = 16;

/// How often an accept loop polls its listener and a waiting connection its verdict.
const POLL: Duration = Duration::from_millis(50);

/// How long an upgraded connection's read waits before it looks for frames to send.
const FRAME_POLL: Duration = Duration::from_millis(10);

/// How long a refused connection waits for the client to close after the answer, so the answer
/// is not lost to a reset.
const LINGER: Duration = Duration::from_secs(1);

/// iTerm2's listen backlog (`S:159-169`).
const BACKLOG: i32 = 5;

pub struct Server {
    addrs: Vec<SocketAddr>,
    events: Option<Receiver<Event>>,
    shared: Arc<Shared>,
    accepts: Vec<JoinHandle<()>>,
}

struct Shared {
    stop: AtomicBool,
    next_conn: AtomicU64,
    conns: Mutex<HashMap<u64, Control>>,
    threads: Mutex<Vec<JoinHandle<()>>>,
    peer: PeerCheck,
    cookie: CookieCheck,
    wake: Notify,
}

/// The UI thread's handles on one connection.
struct Control {
    verdict: Option<Sender<bool>>,
    out: Sender<Out>,
    /// A second handle on the socket, to unblock the thread when the server stops.
    stream: TcpStream,
}

enum Out {
    Frame(Vec<u8>),
    Close,
}

impl Server {
    /// Listens on 127.0.0.1 and [::1] at `port` (port 0: V4 takes an ephemeral port and V6 the
    /// same one). A missing IPv6 stack leaves V4 alone; a port in use on either is an error.
    pub fn start(port: u16, peer: PeerCheck, cookie: CookieCheck, wake: Notify) -> Result<Server, String> {
        let listeners = bind_pair(port)?;
        let mut addrs = Vec::new();
        for l in &listeners {
            l.set_nonblocking(true).map_err(|e| format!("port {port}: {e}"))?;
            addrs.push(l.local_addr().map_err(|e| format!("port {port}: {e}"))?);
        }
        let (tx, rx) = mpsc::sync_channel(EVENT_QUEUE);
        let shared = Arc::new(Shared {
            stop: AtomicBool::new(false),
            next_conn: AtomicU64::new(1),
            conns: Mutex::new(HashMap::new()),
            threads: Mutex::new(Vec::new()),
            peer,
            cookie,
            wake,
        });
        let accepts = listeners
            .into_iter()
            .map(|l| {
                let (shared, tx) = (shared.clone(), tx.clone());
                std::thread::spawn(move || accept_loop(l, shared, tx))
            })
            .collect();
        Ok(Server { addrs, events: Some(rx), shared, accepts })
    }

    pub fn addrs(&self) -> Vec<SocketAddr> {
        self.addrs.clone()
    }

    pub fn try_event(&self) -> Option<Event> {
        self.events.as_ref()?.try_recv().ok()
    }

    /// Blocks for the next event; None once stopped.
    pub fn wait_event(&self) -> Option<Event> {
        self.events.as_ref()?.recv().ok()
    }

    /// Answers a `Pending` connection: allow upgrades it, deny answers 401.
    pub fn verdict(&self, conn: u64, allow: bool) {
        let tx = self.shared.conns.lock().unwrap().get_mut(&conn).and_then(|c| c.verdict.take());
        if let Some(tx) = tx {
            let _ = tx.send(allow);
        }
    }

    /// Sends a binary frame.
    pub fn send(&self, conn: u64, bytes: Vec<u8>) {
        self.post(conn, Out::Frame(bytes));
    }

    /// Closes a connection with a close frame.
    pub fn close(&self, conn: u64) {
        self.post(conn, Out::Close);
    }

    fn post(&self, conn: u64, out: Out) {
        if let Some(c) = self.shared.conns.lock().unwrap().get(&conn) {
            let _ = c.out.send(out);
        }
    }

    /// Stops listening and drops every connection (also on Drop).
    pub fn stop(self) {}
}

impl Drop for Server {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        // Unblocks any thread waiting for room in the event queue.
        drop(self.events.take());
        // The listeners close as their loops end, so new connects are refused from here on.
        for h in self.accepts.drain(..) {
            let _ = h.join();
        }
        // Each connection sees the flag within a poll and sends its close frame; one stuck in a
        // write to a client that stopped reading is cut off.
        let deadline = Instant::now() + LINGER;
        while !self.shared.conns.lock().unwrap().is_empty() && Instant::now() < deadline {
            std::thread::sleep(FRAME_POLL);
        }
        for c in self.shared.conns.lock().unwrap().values() {
            let _ = c.stream.shutdown(std::net::Shutdown::Both);
        }
        let threads = std::mem::take(&mut *self.shared.threads.lock().unwrap());
        for h in threads {
            let _ = h.join();
        }
    }
}

/// Binds both loopback families at `port`, retrying a few ephemeral ports when V6 finds the one
/// V4 took already in use.
fn bind_pair(port: u16) -> Result<Vec<TcpListener>, String> {
    let attempts = if port == 0 { 8 } else { 1 };
    for _ in 0..attempts {
        let v4 = bind_exclusive(SocketAddr::from((Ipv4Addr::LOCALHOST, port))).map_err(|e| format!("127.0.0.1 port {port}: {e}"))?;
        let taken = v4.local_addr().map_err(|e| format!("127.0.0.1 port {port}: {e}"))?.port();
        match bind_exclusive(SocketAddr::from((Ipv6Addr::LOCALHOST, taken))) {
            Ok(v6) => return Ok(vec![v4, v6]),
            Err(e) if v6_unavailable(&e) => return Ok(vec![v4]),
            Err(e) if port == 0 && e.kind() == std::io::ErrorKind::AddrInUse => continue,
            Err(e) => return Err(format!("[::1] port {taken}: {e}")),
        }
    }
    Err("no ephemeral port was free on both 127.0.0.1 and [::1]".to_string())
}

/// A V6 bind that failed because the machine has no usable IPv6 loopback.
fn v6_unavailable(e: &std::io::Error) -> bool {
    use windows_sys::Win32::Networking::WinSock::{WSAEADDRNOTAVAIL, WSAEAFNOSUPPORT, WSAEPFNOSUPPORT, WSAEPROTONOSUPPORT};
    e.raw_os_error().is_some_and(|c| [WSAEAFNOSUPPORT, WSAEADDRNOTAVAIL, WSAEPFNOSUPPORT, WSAEPROTONOSUPPORT].contains(&c))
}

fn accept_loop(listener: TcpListener, shared: Arc<Shared>, tx: SyncSender<Event>) {
    while !shared.stop.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((stream, peer)) => admit(stream, peer, &shared, &tx),
            Err(_) => std::thread::sleep(POLL),
        }
    }
}

/// Registers an accepted connection and starts its thread.
fn admit(stream: TcpStream, peer: SocketAddr, shared: &Arc<Shared>, tx: &SyncSender<Event>) {
    // An accepted socket inherits the listener's non-blocking mode.
    if stream.set_nonblocking(false).is_err() {
        return;
    }
    let _ = stream.set_nodelay(true);
    let (Ok(local), Ok(handle)) = (stream.local_addr(), stream.try_clone()) else {
        return;
    };
    let conn = shared.next_conn.fetch_add(1, Ordering::SeqCst);
    let (verdict_tx, verdict_rx) = mpsc::channel();
    let (out_tx, out_rx) = mpsc::channel();
    shared.conns.lock().unwrap().insert(conn, Control { verdict: Some(verdict_tx), out: out_tx, stream: handle });
    let (thread_shared, tx) = (shared.clone(), tx.clone());
    let h = std::thread::spawn(move || {
        let c = Connection { conn, shared: thread_shared, tx };
        c.serve(stream, peer, local, verdict_rx, out_rx);
        c.shared.conns.lock().unwrap().remove(&conn);
    });
    let mut threads = shared.threads.lock().unwrap();
    threads.retain(|t| !t.is_finished());
    threads.push(h);
}

struct Connection {
    conn: u64,
    shared: Arc<Shared>,
    tx: SyncSender<Event>,
}

impl Connection {
    fn stopping(&self) -> bool {
        self.shared.stop.load(Ordering::SeqCst)
    }

    /// Queues an event for the UI thread and wakes it; false once the server is gone.
    fn event(&self, e: Event) -> bool {
        let ok = self.tx.send(e).is_ok();
        if ok {
            (self.shared.wake)();
        }
        ok
    }

    /// iTerm2's sequence (`S:457-608`): the peer check before reading, the request, the checks,
    /// the verdict, then the upgrade.
    fn serve(&self, mut stream: TcpStream, peer: SocketAddr, local: SocketAddr, verdict: Receiver<bool>, out: Receiver<Out>) {
        let peer_pid = match (self.shared.peer)(peer, local) {
            Ok(pid) => pid,
            Err(reason) => {
                refuse(&mut stream, 401);
                self.event(Event::Rejected { key: None, reason });
                return;
            }
        };
        let Some((head, tail)) = self.read_head(&mut stream) else {
            refuse(&mut stream, 400);
            self.event(Event::Rejected { key: None, reason: api_http::READ_FAILED.to_string() });
            return;
        };
        let Some(request) = api_http::parse(&head) else {
            refuse(&mut stream, 400);
            self.event(Event::Rejected { key: None, reason: api_http::READ_FAILED.to_string() });
            return;
        };
        let cookie = self.shared.cookie.clone();
        let upgrade = match api_http::check(&request, &mut |c| cookie(c), &mut crate::api_auth::fresh_key) {
            Ok(u) => u,
            Err(refusal) => {
                refuse(&mut stream, refusal.code());
                // iTerm2 posts the client's key with an upgrade refusal, not with a bad path
                // (`S:513-526`, `S:596-601`).
                let key = if api_http::url_path(&request.path) == "/" { request.headers.get("x-iterm2-key").cloned() } else { None };
                self.event(Event::Rejected { key, reason: refusal.reason().to_string() });
                return;
            }
        };
        let (ws_key, ws_version) = (upgrade.ws_key.clone(), upgrade.ws_version);
        if !self.event(Event::Pending(Pending { conn: self.conn, upgrade, peer_pid })) {
            refuse(&mut stream, 401);
            return;
        }
        let allowed = loop {
            match verdict.recv_timeout(POLL) {
                Ok(allow) => break allow,
                Err(RecvTimeoutError::Timeout) if !self.stopping() => {}
                Err(_) => break false,
            }
        };
        if !allowed {
            refuse(&mut stream, 401);
            return;
        }
        if stream.write_all(api_http::accept_response(&ws_key, ws_version).as_bytes()).is_err() || stream.set_read_timeout(Some(FRAME_POLL)).is_err() {
            self.event(Event::Closed { conn: self.conn });
            return;
        }
        let ws = WebSocket::from_partially_read(stream, tail, Role::Server, None);
        self.carry(ws, out);
        self.event(Event::Closed { conn: self.conn });
    }

    /// Reads until the end of the request head, `MAX_REQUEST` bytes or `REQUEST_DEADLINE`;
    /// returns the head and whatever followed it.
    fn read_head(&self, stream: &mut TcpStream) -> Option<(Vec<u8>, Vec<u8>)> {
        let deadline = Instant::now() + api_http::REQUEST_DEADLINE;
        stream.set_read_timeout(Some(POLL)).ok()?;
        let mut buf = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            if let Some(end) = api_http::head_end(&buf) {
                let tail = buf.split_off(end);
                return Some((buf, tail));
            }
            if buf.len() >= api_http::MAX_REQUEST || Instant::now() >= deadline || self.stopping() {
                return None;
            }
            let room = chunk.len().min(api_http::MAX_REQUEST - buf.len());
            match stream.read(&mut chunk[..room]) {
                Ok(0) => return None,
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
                Err(e) if retry(&e) => {}
                Err(_) => return None,
            }
        }
    }

    /// Carries frames until either side closes, an error, or the server stops: queued frames
    /// go out between reads; binary frames come in as events, text frames are ignored
    /// (`S:1436-1447`), pings are answered by tungstenite on the next flush.
    fn carry(&self, mut ws: WebSocket<TcpStream>, out: Receiver<Out>) {
        loop {
            if self.stopping() {
                let _ = ws.close(None);
                let _ = ws.flush();
                return;
            }
            loop {
                match out.try_recv() {
                    Ok(Out::Frame(bytes)) => {
                        if ws.send(Message::binary(bytes)).is_err() {
                            return;
                        }
                    }
                    Ok(Out::Close) => {
                        let _ = ws.close(None);
                        let _ = ws.flush();
                        return;
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => return,
                }
            }
            match ws.read() {
                Ok(Message::Binary(bytes)) => {
                    if !self.event(Event::Message { conn: self.conn, bytes: bytes.to_vec() }) {
                        return;
                    }
                }
                Ok(Message::Close(_)) => {
                    // tungstenite has queued the close reply; send it.
                    let _ = ws.flush();
                    return;
                }
                Ok(_) => {}
                Err(tungstenite::Error::Io(e)) if retry(&e) => {}
                Err(_) => return,
            }
        }
    }
}

/// A read that only timed out.
fn retry(e: &std::io::Error) -> bool {
    matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut | std::io::ErrorKind::Interrupted)
}

/// Answers `code` and closes, as iTerm2's `sendResponseWithCode` does (`HC:85-120`), letting the
/// client read the answer before the socket goes.
fn refuse(stream: &mut TcpStream, code: u16) {
    if stream.write_all(api_http::status_response(code).as_bytes()).is_err() {
        return;
    }
    let _ = stream.shutdown(std::net::Shutdown::Write);
    let _ = stream.set_read_timeout(Some(POLL));
    let deadline = Instant::now() + LINGER;
    let mut sink = [0u8; 4096];
    while Instant::now() < deadline {
        match stream.read(&mut sink) {
            Ok(0) => return,
            Ok(_) => {}
            Err(e) if retry(&e) => {}
            Err(_) => return,
        }
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
fn bind_exclusive(addr: SocketAddr) -> std::io::Result<TcpListener> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use tungstenite::client::IntoClientRequest;

    fn allow() -> PeerCheck {
        Arc::new(|_, _| Ok(4242))
    }

    fn start_with(peer: PeerCheck) -> Server {
        Server::start(0, peer, Arc::new(|c: &str| c == "good"), Arc::new(|| {})).unwrap()
    }

    fn next(s: &Server) -> Event {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(e) = s.try_event() {
                return e;
            }
            assert!(Instant::now() < deadline, "no event");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn quiet(s: &Server) {
        std::thread::sleep(Duration::from_millis(200));
        assert!(s.try_event().is_none());
    }

    fn head(path: &str, library: &str, extra: &str) -> String {
        format!(
            "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:1912\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
             Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\
             Sec-WebSocket-Protocol: api.iterm2.com\r\norigin: ws://localhost/\r\n\
             x-iterm2-library-version: {library}\r\nx-iterm2-disable-auth-ui: true\r\n{extra}\r\n"
        )
    }

    /// Everything the server writes until it closes (a reset ends the read too).
    fn read_all(t: &mut TcpStream) -> String {
        t.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        let mut out = Vec::new();
        let mut buf = [0u8; 1024];
        while let Ok(n) = t.read(&mut buf) {
            if n == 0 {
                break;
            }
            out.extend_from_slice(&buf[..n]);
        }
        String::from_utf8_lossy(&out).into_owned()
    }

    fn raw(addr: SocketAddr, request: &str) -> String {
        let mut t = TcpStream::connect(addr).unwrap();
        t.write_all(request.as_bytes()).unwrap();
        read_all(&mut t)
    }

    fn client(addr: SocketAddr) -> JoinHandle<Result<(WebSocket<TcpStream>, tungstenite::handshake::client::Response), String>> {
        let mut req = format!("ws://{addr}/").into_client_request().unwrap();
        let h = req.headers_mut();
        h.insert("Sec-WebSocket-Protocol", "api.iterm2.com".parse().unwrap());
        h.insert("origin", "ws://localhost/".parse().unwrap());
        h.insert("x-iterm2-library-version", "python 2.24".parse().unwrap());
        h.insert("x-iterm2-disable-auth-ui", "true".parse().unwrap());
        h.insert("x-iterm2-advisory-name", "demo.py".parse().unwrap());
        let stream = TcpStream::connect(addr).unwrap();
        std::thread::spawn(move || tungstenite::client::client(req, stream).map_err(|e| e.to_string()))
    }

    fn pending(s: &Server) -> Pending {
        match next(s) {
            Event::Pending(p) => p,
            e => panic!("expected Pending, got {e:?}"),
        }
    }

    #[test]
    fn server_listens_on_loopback_only() {
        let s = start_with(allow());
        let a = s.addrs();
        assert_eq!(a.len(), 2, "{a:?}");
        assert!(a.iter().all(|x| x.ip().is_loopback()), "{a:?}");
        assert!(a[0].is_ipv4() && a[1].is_ipv6(), "{a:?}");
        assert_eq!(a[0].port(), a[1].port());
        assert_ne!(a[0].port(), 0);
        let port = a[0].port();

        let e = Server::start(port, allow(), Arc::new(|_: &str| false), Arc::new(|| {})).err().expect("a second start on the port");
        assert!(e.contains(&port.to_string()), "{e}");
        assert!(TcpListener::bind(a[0]).is_err());
        assert!(TcpListener::bind(a[1]).is_err());

        let plain = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        assert!(bind_exclusive(plain.local_addr().unwrap()).is_err());
        let plain6 = TcpListener::bind((Ipv6Addr::LOCALHOST, 0)).unwrap();
        assert!(bind_exclusive(plain6.local_addr().unwrap()).is_err());
        // Loopback only, even if asked otherwise.
        assert!(bind_exclusive(SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0))).is_err());
        assert!(bind_exclusive(SocketAddr::from((Ipv6Addr::UNSPECIFIED, 0))).is_err());
        s.stop();
    }

    #[test]
    fn server_upgrades_and_carries_frames() {
        let s = start_with(allow());
        for addr in s.addrs() {
            let c = client(addr);
            let p = pending(&s);
            assert_eq!(p.peer_pid, 4242);
            assert_eq!(p.upgrade.advisory_name.as_deref(), Some("demo.py"));
            assert!(!p.upgrade.preauthorized);
            s.verdict(p.conn, true);
            let (mut ws, resp) = c.join().unwrap().unwrap();
            assert_eq!(resp.headers()["x-iterm2-protocol-version"], "1.19");
            assert_eq!(resp.headers()["sec-websocket-protocol"], "api.iterm2.com");

            ws.send(Message::text("ignored")).unwrap();
            ws.send(Message::binary(vec![1u8, 2, 3])).unwrap();
            match next(&s) {
                Event::Message { conn, bytes } => {
                    assert_eq!(conn, p.conn);
                    assert_eq!(bytes, [1, 2, 3]);
                }
                e => panic!("{e:?}"),
            }

            s.send(p.conn, vec![9, 8]);
            ws.get_ref().set_read_timeout(Some(Duration::from_secs(10))).unwrap();
            match ws.read().unwrap() {
                Message::Binary(b) => assert_eq!(&b[..], &[9, 8]),
                m => panic!("{m:?}"),
            }

            ws.close(None).unwrap();
            while ws.read().is_ok() {}
            match next(&s) {
                Event::Closed { conn } => assert_eq!(conn, p.conn),
                e => panic!("{e:?}"),
            }
        }
        quiet(&s);
    }

    #[test]
    fn server_refusals_follow_iterm2() {
        let s = start_with(allow());
        let addr = s.addrs()[0];

        let out = raw(addr, &head("/x", "python 2.24", ""));
        assert!(out.starts_with("HTTP/1.1 400 Bad Request\r\n"), "{out}");
        match next(&s) {
            Event::Rejected { key, reason } => {
                assert_eq!(key, None);
                assert_eq!(reason, "Bad request. URL path was /x, but should be /");
            }
            e => panic!("{e:?}"),
        }

        let out = raw(addr, &head("/", "python 0.1", "x-iterm2-key: K\r\n"));
        assert_eq!(out, "HTTP/1.1 406 Unacceptable (upgrade Python library)\r\n\r\n");
        match next(&s) {
            Event::Rejected { key, reason } => {
                assert_eq!(key.as_deref(), Some("K"));
                assert!(reason.starts_with("Library version too old."), "{reason}");
            }
            e => panic!("{e:?}"),
        }

        let mut t = TcpStream::connect(addr).unwrap();
        t.write_all(head("/", "python 2.24", "x-iterm2-cookie: good\r\n").as_bytes()).unwrap();
        let p = pending(&s);
        assert!(p.upgrade.preauthorized);
        s.verdict(p.conn, false);
        assert_eq!(read_all(&mut t), "HTTP/1.1 401 Unauthorized\r\n\r\n");

        for garbage in ["garbage\r\n\r\n", "GET / HTTP/1.1\r\nno colon\r\n\r\n"] {
            let out = raw(addr, garbage);
            assert!(out.starts_with("HTTP/1.1 400 Bad Request\r\n"), "{out}");
            match next(&s) {
                Event::Rejected { key: None, reason } => assert_eq!(reason, api_http::READ_FAILED),
                e => panic!("{e:?}"),
            }
        }
        drop(TcpStream::connect(addr).unwrap());
        match next(&s) {
            Event::Rejected { key: None, reason } => assert_eq!(reason, api_http::READ_FAILED),
            e => panic!("{e:?}"),
        }
        quiet(&s);

        // A peer refused before any request is read.
        let refusing = start_with(Arc::new(|_, _| Err("nope".to_string())));
        let mut t = TcpStream::connect(refusing.addrs()[1]).unwrap();
        assert_eq!(read_all(&mut t), "HTTP/1.1 401 Unauthorized\r\n\r\n");
        match refusing.wait_event() {
            Some(Event::Rejected { key: None, reason }) => assert_eq!(reason, "nope"),
            e => panic!("{e:?}"),
        }
    }

    #[test]
    fn stop_drops_connections() {
        let s = start_with(allow());
        let addr = s.addrs()[0];
        let c = client(addr);
        let p = pending(&s);
        s.verdict(p.conn, true);
        let (mut ws, _) = c.join().unwrap().unwrap();
        ws.get_ref().set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        s.stop();
        match ws.read() {
            Ok(Message::Close(_)) | Err(tungstenite::Error::ConnectionClosed) | Err(tungstenite::Error::AlreadyClosed) => {}
            Err(tungstenite::Error::Io(e)) => assert!(!matches!(e.kind(), std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock), "{e}"),
            other => panic!("{other:?}"),
        }
        assert!(TcpStream::connect_timeout(&addr, Duration::from_secs(5)).is_err());
    }
}
