//! The Python API's sockets: an exclusive listener on each loopback family (never any other
//! address). The accept thread runs the peer check and the connection caps; a connection that
//! passes gets a thread (and a blocking reader thread, as iTerm2's) that reads and checks the
//! request (`api_http`), waits for the UI thread's verdict and then carries binary WebSocket
//! frames with tungstenite (framing only). No socket ever has a receive timeout. Events reach the
//! UI thread through a channel and the wake (`S:457-608`).

use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use tungstenite::protocol::Role;
use tungstenite::{Message, WebSocket};

use crate::api_http;
use crate::platform::pty::Notify;

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
    /// An upgraded connection ended, whoever ended it, or a pending one's client went before
    /// its verdict.
    Closed { conn: u64 },
    /// A connection refused before any verdict: by the peer check, an unreadable request or an
    /// upgrade check. `key` is the client's `x-iterm2-key` where iTerm2 posts it.
    Rejected { key: Option<String>, reason: String },
}

/// Events waiting for the UI thread, bounded like iTerm2's 16 in-flight read chunks
/// (`W:33-40`), so a flooding client meets TCP back pressure.
const EVENT_QUEUE: usize = 16;

/// Read chunks a connection's reader queues before it stops draining the socket (`W:40`).
const INBOUND_CHUNKS: usize = 16;

/// The reader's chunk size.
const CHUNK: usize = 16 * 1024;

/// Live connection threads (each with its reader) and, among them, those not yet authorized.
/// iTerm2 has no cap; over one, a connection gets `503` and is closed on the accept thread.
pub(crate) const MAX_CONNECTIONS: usize = 32;
pub(crate) const MAX_UNAUTHORIZED: usize = 8;

/// The reason logged when a cap refuses a connection (Termurion's own text).
pub(crate) const TOO_MANY: &str = "Too many Python API connections";

/// The largest message and frame a client may send. iTerm2 has no cap (`iTermWebSocketFrame.m:
/// 101-150` takes any 64-bit length); 16 MiB is tungstenite's default frame cap, far above any
/// request the protocol has.
pub(crate) const MAX_MESSAGE: usize = 16 * 1024 * 1024;

/// Bytes queued for one client past which it is cut off, and separately tungstenite's write
/// buffer cap: at most 32 MiB queue plus tungstenite's 32 MiB buffer per client.
pub(crate) const OUTBOUND_LIMIT: usize = 2 * MAX_MESSAGE;

/// How often an accept loop polls its listener.
const POLL: Duration = Duration::from_millis(50);

/// How long a refused same-user connection waits for the client to close after the answer, so
/// the answer is not lost to a reset. Also how long stop waits for close frames.
const LINGER: Duration = Duration::from_secs(1);

/// The least time between two logged accept errors.
const ACCEPT_LOG_EVERY: Duration = Duration::from_secs(60);

/// iTerm2's listen backlog (`S:159-169`).
const BACKLOG: i32 = 5;

pub struct Server {
    addrs: Vec<SocketAddr>,
    events: Option<Receiver<Event>>,
    shared: Arc<Shared>,
    accepts: Vec<JoinHandle<()>>,
}

/// The next connection id: unique for the life of the process, never reset when a server
/// stops and a new one starts, so no per-connection state of an old connection can ever be
/// taken for a new one's.
static NEXT_CONN: AtomicU64 = AtomicU64::new(1);

struct Shared {
    stop: AtomicBool,
    conns: Mutex<HashMap<u64, Control>>,
    threads: Mutex<Vec<JoinHandle<()>>>,
    /// Live connection threads and those among them not yet authorized, checked and raised in
    /// one step by either accept thread.
    counts: Mutex<Counts>,
    /// Connection threads ever started (tests): never lowered, it proves a refusal made none.
    spawned: AtomicUsize,
    peer: PeerCheck,
    cookie: CookieCheck,
    wake: Notify,
}

#[derive(Default)]
struct Counts {
    live: usize,
    unauthorized: usize,
    /// The highest of each ever reached (tests).
    peak: (usize, usize),
}

impl Shared {
    /// Takes a live, unauthorized place if both caps allow, in one step.
    fn reserve(&self) -> bool {
        let mut c = self.counts.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if c.live >= MAX_CONNECTIONS || c.unauthorized >= MAX_UNAUTHORIZED {
            return false;
        }
        c.live += 1;
        c.unauthorized += 1;
        c.peak = (c.peak.0.max(c.live), c.peak.1.max(c.unauthorized));
        true
    }

    /// Gives back a place; `authorized` places were already off the unauthorized count.
    fn release(&self, authorized: bool) {
        let mut c = self.counts.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        c.live -= 1;
        if !authorized {
            c.unauthorized -= 1;
        }
    }
}

/// A connection's place in the caps and the table and its reader, given back on every exit of
/// its thread, a panic included.
struct Slot {
    shared: Arc<Shared>,
    conn: u64,
    mailbox: Arc<Mailbox>,
    /// A handle on the socket, to end the reader's reads.
    stream: TcpStream,
    reader: Option<JoinHandle<()>>,
    authorized: bool,
}

impl Slot {
    /// The verdict allowed it: off the unauthorized count.
    fn authorize(&mut self) {
        if !self.authorized {
            self.authorized = true;
            self.shared.counts.lock().unwrap_or_else(std::sync::PoisonError::into_inner).unauthorized -= 1;
        }
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        self.mailbox.update(|m| m.done = true);
        let _ = self.stream.shutdown(std::net::Shutdown::Both);
        if let Some(r) = self.reader.take() {
            let _ = r.join();
        }
        self.shared.conns.lock().unwrap_or_else(std::sync::PoisonError::into_inner).remove(&self.conn);
        self.shared.release(self.authorized);
    }
}

/// The UI thread's handles on one connection.
struct Control {
    mailbox: Arc<Mailbox>,
    /// A second handle on the socket, to cut the connection off.
    stream: TcpStream,
}

/// What a connection thread waits on (its reader's chunks, the UI's frames and verdict, the
/// stop) and what its reader waits on (room in the queue, its handed-over room to write being
/// used, the end). Each side has its own condition variable and is woken only when the other
/// changed something it looks at, so two waiting sides never wake each other in a loop.
#[derive(Default)]
struct Mailbox {
    mail: Mutex<Mail>,
    to_conn: Condvar,
    to_reader: Condvar,
    /// Times a wait woke (tests).
    wakeups: AtomicUsize,
}

#[derive(Default)]
struct Mail {
    inbound: VecDeque<Vec<u8>>,
    /// The client closed its side, or the socket failed.
    eof: bool,
    outbound: VecDeque<Out>,
    /// Bytes of notifications queued (the cap's count).
    outbound_bytes: usize,
    /// Bytes of responses queued, counted apart (`refused`).
    response_bytes: usize,
    verdict: Option<bool>,
    /// The server stops (close frame, then end).
    stop: bool,
    /// Cut off at once: the outbound cap was passed.
    abort: bool,
    /// The connection thread is done; the reader ends.
    done: bool,
    /// tungstenite holds bytes the socket would not take; the reader watches for room.
    want_write: bool,
    /// The reader saw room to write.
    writable: bool,
}

enum Out {
    Frame(Vec<u8>),
    /// A response to a request: it may pass the cap (`refused`).
    Response(Vec<u8>),
    Close,
}

/// Whether a frame of `len` bytes cuts its client off, with `notes` bytes of notifications and
/// `responses` bytes of responses queued. Notifications keep the cap: past `OUTBOUND_LIMIT`
/// queued the client is not keeping up. A response is counted apart and always taken when no
/// other response waits, so one answer larger than the cap (a long styled GetBuffer) is sent
/// instead of dropping the connection (m11b ruling; the cost: that one answer's memory).
fn refused(notes: usize, responses: usize, len: usize, response: bool) -> bool {
    if response { responses > 0 && responses + len > OUTBOUND_LIMIT } else { notes + len > OUTBOUND_LIMIT }
}

impl Mail {
    /// What either side's waits look at; a change wakes the other side.
    fn view(&self) -> (usize, bool, bool, bool, bool) {
        (self.inbound.len(), self.eof, self.writable, self.want_write, self.done)
    }
}

impl Mailbox {
    /// The connection thread waits until `f` yields, `until` passes (None) or forever.
    fn wait_conn<T>(&self, until: Option<Instant>, f: impl FnMut(&mut Mail) -> Option<T>) -> Option<T> {
        self.wait(&self.to_conn, &self.to_reader, until, f)
    }

    /// The reader waits until `f` yields.
    fn wait_reader<T>(&self, f: impl FnMut(&mut Mail) -> Option<T>) -> Option<T> {
        self.wait(&self.to_reader, &self.to_conn, None, f)
    }

    /// Waits on `mine`, waking `theirs` only when `f` changed what the other side looks at.
    fn wait<T>(&self, mine: &Condvar, theirs: &Condvar, until: Option<Instant>, mut f: impl FnMut(&mut Mail) -> Option<T>) -> Option<T> {
        let mut m = self.mail.lock().unwrap();
        loop {
            let before = m.view();
            let got = f(&mut m);
            if m.view() != before {
                theirs.notify_all();
            }
            if got.is_some() {
                return got;
            }
            m = match until {
                None => mine.wait(m).unwrap(),
                Some(t) => {
                    let now = Instant::now();
                    if now >= t {
                        return None;
                    }
                    mine.wait_timeout(m, t - now).unwrap().0
                }
            };
            self.wakeups.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Changes the mail and wakes both sides.
    fn update(&self, f: impl FnOnce(&mut Mail)) {
        f(&mut self.mail.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
        self.to_conn.notify_all();
        self.to_reader.notify_all();
    }
}

/// The socket as tungstenite sees it after the upgrade: reads come from the chunks the reader
/// thread queued (`WouldBlock` when none are left), writes go straight to the socket, which is
/// non-blocking from then on (`WouldBlock` when it has no room; tungstenite keeps the bytes).
struct Pipe {
    stream: TcpStream,
    buf: Vec<u8>,
    at: usize,
}

impl Read for Pipe {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        if self.at == self.buf.len() {
            return Err(std::io::ErrorKind::WouldBlock.into());
        }
        let n = out.len().min(self.buf.len() - self.at);
        out[..n].copy_from_slice(&self.buf[self.at..self.at + n]);
        self.at += n;
        if self.at == self.buf.len() {
            self.buf.clear();
            self.at = 0;
        }
        Ok(n)
    }
}

impl Write for Pipe {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.stream.write(b)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.stream.flush()
    }
}

impl Server {
    /// Listens on 127.0.0.1 and [::1] at `port` (port 0: V4 takes an ephemeral port and V6 the
    /// same one). A missing IPv6 stack leaves V4 alone; a port in use on either is an error.
    pub fn start(port: u16, peer: PeerCheck, cookie: CookieCheck, wake: Notify) -> Result<Server, String> {
        serve_on(bind_pair(port)?, peer, cookie, wake)
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

    /// Answers a `Pending` connection: allow upgrades it, deny answers 401. The first answer
    /// counts.
    pub fn verdict(&self, conn: u64, allow: bool) {
        if let Some(m) = self.mailbox(conn) {
            m.update(|m| {
                m.verdict.get_or_insert(allow);
            });
        }
    }

    /// Sends a binary frame (a notification). A client that lets more than `OUTBOUND_LIMIT`
    /// bytes queue up is cut off (its connection ends with `Closed`).
    pub fn send(&self, conn: u64, bytes: Vec<u8>) {
        self.queue(conn, bytes, false);
    }

    /// Sends a response: like `send`, except that one response larger than the cap is still
    /// sent (`refused`).
    pub fn respond(&self, conn: u64, bytes: Vec<u8>) {
        self.queue(conn, bytes, true);
    }

    fn queue(&self, conn: u64, bytes: Vec<u8>, response: bool) {
        let conns = self.shared.conns.lock().unwrap();
        let Some(c) = conns.get(&conn) else { return };
        let mut m = c.mailbox.mail.lock().unwrap();
        if refused(m.outbound_bytes, m.response_bytes, bytes.len(), response) {
            m.abort = true;
            m.outbound.clear();
            m.outbound_bytes = 0;
            m.response_bytes = 0;
            // Unblocks a write stuck on a client that stopped reading.
            let _ = c.stream.shutdown(std::net::Shutdown::Both);
        } else if response {
            m.response_bytes += bytes.len();
            m.outbound.push_back(Out::Response(bytes));
        } else {
            m.outbound_bytes += bytes.len();
            m.outbound.push_back(Out::Frame(bytes));
        }
        c.mailbox.to_conn.notify_all();
    }

    /// Closes a connection with a close frame.
    pub fn close(&self, conn: u64) {
        if let Some(m) = self.mailbox(conn) {
            m.update(|m| m.outbound.push_back(Out::Close));
        }
    }

    fn mailbox(&self, conn: u64) -> Option<Arc<Mailbox>> {
        self.shared.conns.lock().unwrap().get(&conn).map(|c| c.mailbox.clone())
    }

    /// Stops listening and drops every connection (also on Drop).
    pub fn stop(self) {}

    /// Live connection threads and those not yet authorized.
    #[cfg(test)]
    fn counts(&self) -> (usize, usize) {
        let c = self.shared.counts.lock().unwrap();
        (c.live, c.unauthorized)
    }

    #[cfg(test)]
    fn spawned(&self) -> usize {
        self.shared.spawned.load(Ordering::SeqCst)
    }

    #[cfg(test)]
    fn peak(&self) -> (usize, usize) {
        self.shared.counts.lock().unwrap().peak
    }

    #[cfg(test)]
    fn conns_len(&self) -> usize {
        self.shared.conns.lock().unwrap().len()
    }

    /// Wake-ups of every live connection's waits so far.
    #[cfg(test)]
    fn wakeups(&self) -> usize {
        self.shared.conns.lock().unwrap().values().map(|c| c.mailbox.wakeups.load(Ordering::Relaxed)).sum()
    }
}

/// Serves already bound listeners.
fn serve_on(listeners: Vec<TcpListener>, peer: PeerCheck, cookie: CookieCheck, wake: Notify) -> Result<Server, String> {
    let mut addrs = Vec::new();
    for l in &listeners {
        l.set_nonblocking(true).map_err(|e| e.to_string())?;
        addrs.push(l.local_addr().map_err(|e| e.to_string())?);
    }
    let (tx, rx) = mpsc::sync_channel(EVENT_QUEUE);
    let shared = Arc::new(Shared {
        stop: AtomicBool::new(false),
        conns: Mutex::new(HashMap::new()),
        threads: Mutex::new(Vec::new()),
        counts: Mutex::new(Counts::default()),
        spawned: AtomicUsize::new(0),
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

impl Drop for Server {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        // Unblocks any thread waiting for room in the event queue.
        drop(self.events.take());
        // The listeners close as their loops end, so new connects are refused from here on.
        for h in self.accepts.drain(..) {
            let _ = h.join();
        }
        let mailboxes: Vec<Arc<Mailbox>> = self.shared.conns.lock().unwrap().values().map(|c| c.mailbox.clone()).collect();
        for m in mailboxes {
            m.update(|m| m.stop = true);
        }
        // Each connection sends its close frame; one stuck in a write to a client that stopped
        // reading is cut off.
        let deadline = Instant::now() + LINGER;
        while !self.shared.conns.lock().unwrap().is_empty() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
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

/// Binds both loopback families at `port`.
fn bind_pair(port: u16) -> Result<Vec<TcpListener>, String> {
    bind_pair_with(port, &mut bind_exclusive)
}

/// Binds 127.0.0.1 at `port`, then [::1] with `bind6` on the port V4 got. With port 0, a V6
/// port already taken (in use, or held exclusively or reserved: `WSAEACCES`) retries a few
/// ephemeral ports; with a fixed port it is fatal.
fn bind_pair_with(port: u16, bind6: &mut dyn FnMut(SocketAddr) -> std::io::Result<TcpListener>) -> Result<Vec<TcpListener>, String> {
    use std::io::ErrorKind::{AddrInUse, PermissionDenied};
    let attempts = if port == 0 { 8 } else { 1 };
    for _ in 0..attempts {
        let v4 = bind_exclusive(SocketAddr::from((Ipv4Addr::LOCALHOST, port))).map_err(|e| format!("127.0.0.1 port {port}: {e}"))?;
        let taken = v4.local_addr().map_err(|e| format!("127.0.0.1 port {port}: {e}"))?.port();
        match bind6(SocketAddr::from((Ipv6Addr::LOCALHOST, taken))) {
            Ok(v6) => return Ok(vec![v4, v6]),
            Err(e) if v6_unavailable(&e) => return Ok(vec![v4]),
            Err(e) if port == 0 && matches!(e.kind(), AddrInUse | PermissionDenied) => continue,
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
    let mut last_log: Option<Instant> = None;
    let mut unlogged = 0u32;
    while !shared.stop.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((stream, peer)) => admit(stream, peer, &shared, &tx),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(POLL),
            Err(e) => {
                if last_log.is_none_or(|t| t.elapsed() >= ACCEPT_LOG_EVERY) {
                    let more = if unlogged > 0 { format!(" ({unlogged} more since the last)") } else { String::new() };
                    eprintln!("termurion: the Python API could not accept a connection: {e}{more}");
                    last_log = Some(Instant::now());
                    unlogged = 0;
                } else {
                    unlogged += 1;
                }
                std::thread::sleep(POLL);
            }
        }
    }
}

/// Queues an event without ever blocking the accept thread; a full queue drops it.
fn post_now(shared: &Shared, tx: &SyncSender<Event>, e: Event) {
    if tx.try_send(e).is_ok() {
        (shared.wake)();
    }
}

/// Answers `code` on the accept thread with one non-blocking write (the socket is still in the
/// listener's non-blocking mode) and closes; no thread, no linger.
fn refuse_now(mut stream: TcpStream, code: u16) {
    let _ = stream.write(api_http::status_response(code).as_bytes());
    let _ = stream.shutdown(std::net::Shutdown::Write);
}

/// Runs the peer check and the caps on the accept thread, then starts the connection's thread.
fn admit(stream: TcpStream, peer: SocketAddr, shared: &Arc<Shared>, tx: &SyncSender<Event>) {
    let Ok(local) = stream.local_addr() else { return };
    // iTerm2 checks the peer before reading anything (`S:457-472`); here also before any thread
    // exists, so another user's process never gets one.
    let peer_pid = match (shared.peer)(peer, local) {
        Ok(pid) => pid,
        Err(reason) => {
            refuse_now(stream, 401);
            post_now(shared, tx, Event::Rejected { key: None, reason });
            return;
        }
    };
    if !shared.reserve() {
        refuse_now(stream, 503);
        post_now(shared, tx, Event::Rejected { key: None, reason: TOO_MANY.to_string() });
        return;
    }
    // An accepted socket inherits the listener's non-blocking mode.
    let _ = stream.set_nodelay(true);
    let (Ok(()), Ok(handle), Ok(slot_stream), Ok(reader_stream)) = (stream.set_nonblocking(false), stream.try_clone(), stream.try_clone(), stream.try_clone()) else {
        shared.release(false);
        return;
    };
    let conn = NEXT_CONN.fetch_add(1, Ordering::SeqCst);
    let mailbox = Arc::new(Mailbox::default());
    shared.conns.lock().unwrap().insert(conn, Control { mailbox: mailbox.clone(), stream: handle });
    // From here the slot gives everything back, whatever happens to the thread (and if the
    // thread cannot start, the closure drops it at once).
    let slot = Slot { shared: shared.clone(), conn, mailbox: mailbox.clone(), stream: slot_stream, reader: None, authorized: false };
    shared.spawned.fetch_add(1, Ordering::SeqCst);
    let (thread_shared, tx) = (shared.clone(), tx.clone());
    let spawned = std::thread::Builder::new().spawn(move || {
        let mut slot = slot;
        let reader_box = mailbox.clone();
        slot.reader = std::thread::Builder::new().spawn(move || read_into(reader_stream, &reader_box)).ok();
        if slot.reader.is_none() {
            return;
        }
        let mut c = Connection { conn, shared: thread_shared, tx, mailbox, slot };
        let last = c.serve(&stream, peer_pid);
        let Connection { slot, shared, tx, .. } = c;
        // The counts are settled before the last event is seen.
        drop(slot);
        if let Some(e) = last
            && tx.send(e).is_ok()
        {
            (shared.wake)();
        }
    });
    if let Ok(h) = spawned {
        let mut threads = shared.threads.lock().unwrap();
        threads.retain(|t| !t.is_finished());
        threads.push(h);
    }
}

/// A connection's reader, as iTerm2's (`W:231-256`): blocking reads with no timeout, queued for
/// the connection thread, stopping when `INBOUND_CHUNKS` wait so TCP flow control engages. It
/// reads only once `WSAPoll` says the socket is readable (so the read returns at once), and
/// between polls looks for the connection's end: on Windows neither `shutdown` nor the other
/// handle's close wakes a blocking read, and a poll timing out consumes and cancels nothing.
fn read_into(mut stream: TcpStream, mailbox: &Mailbox) {
    let mut buf = vec![0u8; CHUNK];
    loop {
        let want_write = mailbox.mail.lock().unwrap().want_write;
        let (read, write) = match ready(&stream, want_write) {
            Ok(r) => r,
            Err(_) => {
                mailbox.update(|m| m.eof = true);
                return;
            }
        };
        if write {
            // Hand the room over, then wait until the connection thread has used it.
            mailbox.update(|m| m.writable = true);
            let done = mailbox.wait_reader(|m| {
                if !m.want_write {
                    // Flushed meanwhile: nobody will take the room.
                    m.writable = false;
                }
                (m.done || !m.writable).then_some(m.done)
            });
            if done == Some(true) {
                return;
            }
        }
        if !read {
            if mailbox.mail.lock().unwrap().done {
                return;
            }
            continue;
        }
        let n = match stream.read(&mut buf) {
            Ok(n) => n,
            Err(e) if matches!(e.kind(), std::io::ErrorKind::Interrupted | std::io::ErrorKind::WouldBlock) => continue,
            Err(_) => 0,
        };
        if n == 0 {
            mailbox.update(|m| m.eof = true);
            return;
        }
        let chunk = buf[..n].to_vec();
        let queued = mailbox.wait_reader(|m| {
            if m.done {
                Some(false)
            } else if m.inbound.len() < INBOUND_CHUNKS {
                Some(true)
            } else {
                None
            }
        });
        if queued != Some(true) {
            return;
        }
        mailbox.update(|m| m.inbound.push_back(chunk));
    }
}

/// How long the reader's poll waits before it looks for the connection's end.
const READER_POLL: i32 = 50;

/// Waits up to `READER_POLL` ms for `stream` to have something to read (data, an end or an
/// error: the read tells which) or, with `write`, room to write. Answers (read, write).
fn ready(stream: &TcpStream, write: bool) -> std::io::Result<(bool, bool)> {
    use std::os::windows::io::AsRawSocket;
    use windows_sys::Win32::Networking::WinSock::{POLLERR, POLLHUP, POLLNVAL, POLLRDNORM, POLLWRNORM, WSAPOLLFD, WSAPoll};
    let events = if write { POLLRDNORM | POLLWRNORM } else { POLLRDNORM };
    let mut fd = WSAPOLLFD { fd: stream.as_raw_socket() as _, events, revents: 0 };
    match unsafe { WSAPoll(&mut fd, 1, READER_POLL) } {
        n if n < 0 => Err(wsa_error()),
        0 => Ok((false, false)),
        _ => Ok((fd.revents & (POLLRDNORM | POLLHUP | POLLERR | POLLNVAL) != 0, fd.revents & POLLWRNORM != 0)),
    }
}

/// How the wait for a verdict ended.
enum Waited {
    Verdict(bool),
    /// The client closed its end first.
    Gone,
}

struct Connection {
    conn: u64,
    shared: Arc<Shared>,
    tx: SyncSender<Event>,
    mailbox: Arc<Mailbox>,
    slot: Slot,
}

impl Connection {
    /// Queues an event for the UI thread and wakes it; false once the server is gone.
    fn event(&self, e: Event) -> bool {
        let ok = self.tx.send(e).is_ok();
        if ok {
            (self.shared.wake)();
        }
        ok
    }

    /// iTerm2's sequence after the peer check (`S:497-608`): the request, the checks, the
    /// verdict, then the upgrade. Returns the connection's last event.
    fn serve(&mut self, stream: &TcpStream, peer_pid: u32) -> Option<Event> {
        let read_failed = || Some(Event::Rejected { key: None, reason: api_http::READ_FAILED.to_string() });
        let Some((head, tail)) = self.read_head() else {
            self.refuse(stream, 400);
            return read_failed();
        };
        let Some(request) = api_http::parse(&head) else {
            self.refuse(stream, 400);
            return read_failed();
        };
        let cookie = self.shared.cookie.clone();
        let upgrade = match api_http::check(&request, &mut |c| cookie(c), &mut crate::api_auth::fresh_key) {
            Ok(u) => u,
            Err(refusal) => {
                self.refuse(stream, refusal.code());
                // iTerm2 posts the client's key with an upgrade refusal, not with a bad path
                // (`S:513-526`, `S:596-601`).
                let key = if api_http::url_path(&request.path) == "/" { request.headers.get("x-iterm2-key").cloned() } else { None };
                return Some(Event::Rejected { key, reason: refusal.reason().to_string() });
            }
        };
        let (ws_key, ws_version) = (upgrade.ws_key.clone(), upgrade.ws_version);
        if !self.event(Event::Pending(Pending { conn: self.conn, upgrade, peer_pid })) {
            self.refuse(stream, 401);
            return None;
        }
        // The wait also ends when the client goes: its slot is freed at once, and `Closed`
        // tells the UI to drop its question about it.
        let answer = self.mailbox.wait_conn(None, |m| match (m.verdict, m.stop, m.eof) {
            (Some(v), ..) => Some(Waited::Verdict(v)),
            (None, true, _) => Some(Waited::Verdict(false)),
            (None, false, true) => Some(Waited::Gone),
            _ => None,
        });
        match answer {
            Some(Waited::Verdict(true)) => {}
            Some(Waited::Gone) => return Some(Event::Closed { conn: self.conn }),
            _ => {
                self.refuse(stream, 401);
                return None;
            }
        }
        self.slot.authorize();
        let mut writer = stream;
        if writer.write_all(api_http::accept_response(&ws_key, ws_version).as_bytes()).is_err() {
            return Some(Event::Closed { conn: self.conn });
        }
        let Ok(write_half) = stream.try_clone() else {
            return Some(Event::Closed { conn: self.conn });
        };
        // From here no write blocks, so a client that stops reading cannot hold the thread (or
        // the stop) hostage; the reader only reads what `WSAPoll` says is there.
        if write_half.set_nonblocking(true).is_err() {
            return Some(Event::Closed { conn: self.conn });
        }
        let config = tungstenite::protocol::WebSocketConfig::default()
            .max_message_size(Some(MAX_MESSAGE))
            .max_frame_size(Some(MAX_MESSAGE))
            .max_write_buffer_size(OUTBOUND_LIMIT);
        let pipe = Pipe { stream: write_half, buf: tail, at: 0 };
        self.carry(WebSocket::from_raw_socket(pipe, Role::Server, Some(config)));
        Some(Event::Closed { conn: self.conn })
    }

    /// Collects the reader's chunks until the end of the request head, `MAX_REQUEST` bytes,
    /// `REQUEST_DEADLINE`, the client's close or the stop; returns the head and whatever
    /// followed it. Each search resumes where the last one ended.
    fn read_head(&self) -> Option<(Vec<u8>, Vec<u8>)> {
        let deadline = Instant::now() + api_http::REQUEST_DEADLINE;
        let mut buf = Vec::new();
        loop {
            let (chunks, eof, stop) = self.mailbox.wait_conn(Some(deadline), |m| {
                (!m.inbound.is_empty() || m.eof || m.stop).then(|| (std::mem::take(&mut m.inbound), m.eof, m.stop))
            })?;
            let mut chunks = chunks.into_iter();
            while let Some(chunk) = chunks.next() {
                let from = buf.len().saturating_sub(3);
                buf.extend_from_slice(&chunk);
                if let Some(end) = api_http::head_end(&buf[from..]).map(|e| from + e) {
                    if end > api_http::MAX_REQUEST {
                        return None;
                    }
                    // Bytes pipelined behind the head, in this chunk and any later one, are the
                    // WebSocket's.
                    let mut tail = buf.split_off(end);
                    chunks.for_each(|c| tail.extend_from_slice(&c));
                    return Some((buf, tail));
                }
                if buf.len() >= api_http::MAX_REQUEST {
                    return None;
                }
            }
            if eof || stop {
                return None;
            }
        }
    }

    /// Answers `code` and closes, as iTerm2's `sendResponseWithCode` does (`HC:85-120`), letting
    /// the client read the answer before the socket goes: the write side is shut and whatever the
    /// client still sends is discarded for up to `LINGER`.
    fn refuse(&self, mut stream: &TcpStream, code: u16) {
        if stream.write_all(api_http::status_response(code).as_bytes()).is_err() {
            return;
        }
        let _ = stream.shutdown(std::net::Shutdown::Write);
        self.mailbox.wait_conn(Some(Instant::now() + LINGER), |m| {
            m.inbound.clear();
            (m.eof || m.stop).then_some(())
        });
    }

    /// Carries frames until either side closes, an error, the outbound cap or the stop: the
    /// thread sleeps until the reader or the UI has something, feeds the reader's bytes to
    /// tungstenite, queues the UI's frames, flushes what the socket takes (the rest waits for the
    /// reader to see room), and posts binary frames (text frames are ignored, `S:1436-1447`;
    /// pings are answered by tungstenite).
    fn carry(&self, mut ws: WebSocket<Pipe>) {
        let would_block = |e: &tungstenite::Error| matches!(e, tungstenite::Error::Io(e) if e.kind() == std::io::ErrorKind::WouldBlock);
        let mut closing = false;
        // The write buffer's cap while a response larger than usual is in it.
        let mut raised = OUTBOUND_LIMIT;
        loop {
            let got = self.mailbox.wait_conn(None, |m| {
                let any = !m.inbound.is_empty() || !m.outbound.is_empty() || m.eof || m.stop || m.abort || (m.want_write && m.writable);
                any.then(|| {
                    m.outbound_bytes = 0;
                    m.response_bytes = 0;
                    m.writable = false;
                    (std::mem::take(&mut m.inbound), std::mem::take(&mut m.outbound), m.eof, m.stop, m.abort)
                })
            });
            let Some((chunks, outs, eof, stop, abort)) = got else { return };
            if abort {
                return;
            }
            for chunk in chunks {
                ws.get_mut().buf.extend_from_slice(&chunk);
            }
            for out in outs {
                if closing {
                    break;
                }
                let queued = match out {
                    Out::Frame(bytes) => ws.write(Message::binary(bytes)),
                    // tungstenite's buffer takes one response past its cap; the cap comes
                    // back once the buffer is empty.
                    Out::Response(bytes) => {
                        let need = OUTBOUND_LIMIT + bytes.len();
                        if need > raised {
                            raised = need;
                            ws.set_config(|c| c.max_write_buffer_size = need);
                        }
                        ws.write(Message::binary(bytes))
                    }
                    Out::Close => {
                        closing = true;
                        ws.close(None)
                    }
                };
                match queued {
                    Ok(()) => {}
                    Err(e) if would_block(&e) => {}
                    // WriteBufferFull included: the client is not keeping up.
                    Err(_) => return,
                }
            }
            loop {
                match ws.read() {
                    Ok(Message::Binary(bytes)) => {
                        if !self.event(Event::Message { conn: self.conn, bytes: bytes.to_vec() }) {
                            return;
                        }
                    }
                    Ok(Message::Close(_)) => {
                        // tungstenite has queued the close reply; send it once the socket takes it.
                        closing = true;
                        break;
                    }
                    Ok(_) => {}
                    Err(e) if would_block(&e) => break,
                    Err(_) => return,
                }
            }
            if stop && !closing {
                closing = true;
                let _ = ws.close(None);
            }
            let want_write = match ws.flush() {
                Ok(()) => {
                    if raised != OUTBOUND_LIMIT {
                        raised = OUTBOUND_LIMIT;
                        ws.set_config(|c| c.max_write_buffer_size = OUTBOUND_LIMIT);
                    }
                    false
                }
                Err(e) if would_block(&e) => true,
                Err(_) => return,
            };
            // A close waits for its frame to go out, except when the server stops.
            if (closing && !want_write) || stop || eof {
                return;
            }
            self.mailbox.update(|m| m.want_write = want_write);
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
        // Refused on the accept thread: no connection thread ever existed.
        assert_eq!(refusing.counts(), (0, 0));
        assert_eq!(refusing.spawned(), 0);
    }

    fn rejected(s: &Server) -> String {
        match next(s) {
            Event::Rejected { reason, .. } => reason,
            e => panic!("expected Rejected, got {e:?}"),
        }
    }

    fn settle(s: &Server, counts: (usize, usize)) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while s.counts() != counts {
            assert!(Instant::now() < deadline, "counts {:?}, want {counts:?}", s.counts());
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn connection_caps_refuse_the_excess() {
        let s = start_with(allow());
        let addr = s.addrs()[0];
        // Connections still before authorization are capped first.
        let idle: Vec<TcpStream> = (0..MAX_UNAUTHORIZED).map(|_| TcpStream::connect(addr).unwrap()).collect();
        settle(&s, (MAX_UNAUTHORIZED, MAX_UNAUTHORIZED));
        let mut t = TcpStream::connect(addr).unwrap();
        assert_eq!(read_all(&mut t), "HTTP/1.1 503 Service Unavailable\r\n\r\n");
        assert_eq!(rejected(&s), TOO_MANY);
        drop(idle);
        for _ in 0..MAX_UNAUTHORIZED {
            assert_eq!(rejected(&s), api_http::READ_FAILED);
        }
        settle(&s, (0, 0));

        // Then every live connection, with room left before authorization.
        let mut open = Vec::new();
        for _ in 0..MAX_CONNECTIONS - MAX_UNAUTHORIZED + 1 {
            let c = client(addr);
            let p = pending(&s);
            s.verdict(p.conn, true);
            open.push(c.join().unwrap().unwrap().0);
        }
        let idle: Vec<TcpStream> = (0..MAX_UNAUTHORIZED - 1).map(|_| TcpStream::connect(addr).unwrap()).collect();
        settle(&s, (MAX_CONNECTIONS, MAX_UNAUTHORIZED - 1));
        let mut t = TcpStream::connect(addr).unwrap();
        assert_eq!(read_all(&mut t), "HTTP/1.1 503 Service Unavailable\r\n\r\n");
        assert_eq!(rejected(&s), TOO_MANY);
        // Under the cap again, a valid client still gets through.
        drop(open.pop());
        assert!(matches!(next(&s), Event::Closed { .. }));
        let c = client(addr);
        let p = pending(&s);
        s.verdict(p.conn, true);
        c.join().unwrap().unwrap();
        drop(idle);
        drop(open);
    }

    #[test]
    fn connection_ids_are_never_reused() {
        let first = start_with(allow());
        let mut t = TcpStream::connect(first.addrs()[0]).unwrap();
        t.write_all(head("/", "python 2.24", "").as_bytes()).unwrap();
        let a = pending(&first).conn;
        drop(t);
        drop(first);
        // A new server (the API turned off and on) goes on from the last id.
        let second = start_with(allow());
        let mut t = TcpStream::connect(second.addrs()[0]).unwrap();
        t.write_all(head("/", "python 2.24", "").as_bytes()).unwrap();
        assert!(pending(&second).conn > a);
    }

    #[test]
    fn a_client_gone_before_its_verdict_frees_its_slot() {
        let s = start_with(allow());
        let mut t = TcpStream::connect(s.addrs()[0]).unwrap();
        t.write_all(head("/", "python 2.24", "").as_bytes()).unwrap();
        let p = pending(&s);
        settle(&s, (1, 1));
        drop(t);
        // The wait ends with the client, not with the verdict: the slot is free at once and
        // the UI hears the connection is gone.
        match next(&s) {
            Event::Closed { conn } => assert_eq!(conn, p.conn),
            e => panic!("{e:?}"),
        }
        settle(&s, (0, 0));
        // A late verdict for it does nothing.
        s.verdict(p.conn, true);
        quiet(&s);
    }

    #[test]
    fn pending_verdict_does_not_spin() {
        let s = start_with(allow());
        let mut t = TcpStream::connect(s.addrs()[0]).unwrap();
        t.write_all(head("/", "python 2.24", "").as_bytes()).unwrap();
        let p = pending(&s);
        // Fill the reader's queue while the verdict waits: the reader then waits for room.
        let mut flood = t.try_clone().unwrap();
        let writer = std::thread::spawn(move || {
            let junk = vec![0u8; 64 * 1024];
            while flood.write_all(&junk).is_ok() {}
        });
        std::thread::sleep(Duration::from_millis(500));
        let before = s.wakeups();
        std::thread::sleep(Duration::from_millis(200));
        let woke = s.wakeups() - before;
        assert!(woke < 10, "{woke} wake-ups in 200 ms");
        s.verdict(p.conn, false);
        drop(t);
        let _ = writer.join();
    }

    #[test]
    fn panicking_connection_frees_its_slot() {
        let cookie: CookieCheck = Arc::new(|c: &str| {
            assert!(c != "boom", "cookie check failed on purpose");
            c == "good"
        });
        let s = Server::start(0, allow(), cookie, Arc::new(|| {})).unwrap();
        let addr = s.addrs()[0];
        let mut t = TcpStream::connect(addr).unwrap();
        t.write_all(head("/", "python 2.24", "x-iterm2-cookie: boom\r\n").as_bytes()).unwrap();
        let _ = read_all(&mut t);
        settle(&s, (0, 0));
        assert_eq!(s.conns_len(), 0);
        let c = client(addr);
        let p = pending(&s);
        s.verdict(p.conn, true);
        c.join().unwrap().unwrap();
    }

    #[test]
    fn caps_hold_across_both_listeners() {
        let s = start_with(allow());
        let racers: Vec<_> = s
            .addrs()
            .into_iter()
            .map(|addr| std::thread::spawn(move || (0..3 * MAX_UNAUTHORIZED).filter_map(|_| TcpStream::connect(addr).ok()).collect::<Vec<_>>()))
            .collect();
        let held: Vec<Vec<TcpStream>> = racers.into_iter().map(|r| r.join().unwrap()).collect();
        settle(&s, (MAX_UNAUTHORIZED, MAX_UNAUTHORIZED));
        std::thread::sleep(Duration::from_millis(200));
        let (live, unauthorized) = s.peak();
        assert!(live <= MAX_CONNECTIONS && unauthorized <= MAX_UNAUTHORIZED, "peak {live}, {unauthorized}");
        drop(held);
        settle(&s, (0, 0));
    }

    #[test]
    fn v4_serves_alone_without_ipv6() {
        use windows_sys::Win32::Networking::WinSock::{WSAEACCES, WSAEADDRINUSE, WSAEAFNOSUPPORT};
        let err = |c| std::io::Error::from_raw_os_error(c);
        assert!(v6_unavailable(&err(WSAEAFNOSUPPORT)));
        assert!(!v6_unavailable(&err(WSAEADDRINUSE)));
        assert!(!v6_unavailable(&err(WSAEACCES)));

        let listeners = bind_pair_with(0, &mut |_| Err(err(WSAEAFNOSUPPORT))).unwrap();
        let s = serve_on(listeners, allow(), Arc::new(|_: &str| false), Arc::new(|| {})).unwrap();
        let a = s.addrs();
        assert_eq!(a.len(), 1);
        assert!(a[0].is_ipv4() && a[0].ip().is_loopback());
        let c = client(a[0]);
        let p = pending(&s);
        s.verdict(p.conn, true);
        c.join().unwrap().unwrap();

        // A fixed port in use (or held exclusively) on V6 is fatal; port 0 retries both.
        assert!(bind_pair_with(0, &mut |_| Err(err(WSAEADDRINUSE))).is_err());
        for code in [WSAEADDRINUSE, WSAEACCES] {
            let mut tries = 0;
            let pair = bind_pair_with(0, &mut |addr| {
                tries += 1;
                if tries == 1 { Err(err(code)) } else { bind_exclusive(addr) }
            })
            .unwrap();
            assert_eq!(pair.len(), 2);
            assert_eq!(tries, 2);
        }
    }

    #[test]
    fn frames_arrive_whole_and_in_order() {
        let s = start_with(allow());
        let c = client(s.addrs()[1]);
        let p = pending(&s);
        s.verdict(p.conn, true);
        let (mut ws, _) = c.join().unwrap().unwrap();
        let big: Vec<u8> = (0..1 << 20).map(|i| i as u8).collect();
        ws.send(Message::binary(big.clone())).unwrap();
        for i in 0..500u32 {
            ws.send(Message::binary(i.to_le_bytes().to_vec())).unwrap();
        }
        match next(&s) {
            Event::Message { bytes, .. } => assert!(bytes == big),
            e => panic!("{e:?}"),
        }
        for i in 0..500u32 {
            match next(&s) {
                Event::Message { bytes, .. } => assert_eq!(bytes, i.to_le_bytes()),
                e => panic!("{e:?}"),
            }
        }

        // Frames larger than the socket takes at once go out as the client makes room.
        let frame = |k: u8| vec![k; MAX_MESSAGE / 2];
        for k in 1..=3 {
            s.send(p.conn, frame(k));
        }
        std::thread::sleep(Duration::from_millis(300));
        ws.get_ref().set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        for k in 1..=3 {
            match ws.read().unwrap() {
                Message::Binary(b) => assert!(b[..] == frame(k)[..], "frame {k}"),
                m => panic!("{m:?}"),
            }
        }
    }

    #[test]
    fn stop_cuts_off_a_client_that_never_reads() {
        let s = start_with(allow());
        let c = client(s.addrs()[0]);
        let p = pending(&s);
        s.verdict(p.conn, true);
        let (_ws, _) = c.join().unwrap().unwrap();
        // More than any socket buffer, less than the outbound cap: the writer blocks.
        s.send(p.conn, vec![0u8; MAX_MESSAGE]);
        s.send(p.conn, vec![0u8; MAX_MESSAGE / 2]);
        std::thread::sleep(Duration::from_millis(300));
        let t = Instant::now();
        s.stop();
        assert!(t.elapsed() < Duration::from_secs(5), "stop took {:?}", t.elapsed());
    }

    #[test]
    fn one_response_may_pass_the_cap() {
        // Notifications keep the cap; a response is counted apart, one at a time.
        assert!(refused(OUTBOUND_LIMIT, 0, 1, false));
        assert!(!refused(OUTBOUND_LIMIT - 1, 0, 1, false));
        assert!(!refused(0, 0, OUTBOUND_LIMIT + 1, true), "one answer larger than the cap is sent");
        assert!(!refused(OUTBOUND_LIMIT, 0, 10, true), "queued notifications do not refuse an answer");
        assert!(!refused(0, OUTBOUND_LIMIT + 1, 10, false), "a queued answer does not refuse a notification");
        assert!(refused(0, OUTBOUND_LIMIT, 1, true), "answers waiting together share the cap");
        assert!(!refused(0, 10, 10, true));

        // A reading client gets the whole answer, and a notification after it.
        let s = start_with(allow());
        let c = client(s.addrs()[0]);
        let p = pending(&s);
        s.verdict(p.conn, true);
        let (mut ws, _) = c.join().unwrap().unwrap();
        ws.set_config(|c| {
            c.max_message_size = None;
            c.max_frame_size = None;
        });
        ws.get_ref().set_read_timeout(Some(Duration::from_secs(30))).unwrap();
        s.respond(p.conn, vec![7u8; OUTBOUND_LIMIT + 1]);
        s.send(p.conn, vec![1, 2]);
        match ws.read() {
            Ok(Message::Binary(b)) => assert_eq!((b.len(), b[0]), (OUTBOUND_LIMIT + 1, 7)),
            m => panic!("{m:?}"),
        }
        match ws.read() {
            Ok(Message::Binary(b)) => assert_eq!(&b[..], &[1, 2]),
            m => panic!("{m:?}"),
        }
        assert_eq!(s.counts(), (1, 0), "still connected");
    }

    #[test]
    fn limits_close_the_connection() {
        let s = start_with(allow());
        let addr = s.addrs()[0];
        // A message over the cap ends the connection; nothing reaches the UI.
        let c = client(addr);
        let p = pending(&s);
        s.verdict(p.conn, true);
        let (mut ws, _) = c.join().unwrap().unwrap();
        let _ = ws.send(Message::binary(vec![0u8; MAX_MESSAGE + 1]));
        match next(&s) {
            Event::Closed { conn } => assert_eq!(conn, p.conn),
            e => panic!("{e:?}"),
        }

        // A client that stops reading while more than the outbound cap is queued is cut off.
        let c = client(addr);
        let p = pending(&s);
        s.verdict(p.conn, true);
        let (_ws, _) = c.join().unwrap().unwrap();
        for _ in 0..(OUTBOUND_LIMIT / MAX_MESSAGE) + 2 {
            s.send(p.conn, vec![0u8; MAX_MESSAGE]);
        }
        match next(&s) {
            Event::Closed { conn } => assert_eq!(conn, p.conn),
            e => panic!("{e:?}"),
        }
        settle(&s, (0, 0));
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

    /// A Python with the stock `iterm2` package: `target\\api-venv` when someone prepared it,
    /// else `py -3`. Nothing is ever installed or downloaded here.
    fn python_with_iterm2() -> Option<(std::path::PathBuf, Vec<&'static str>)> {
        let venv = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target").join("api-venv").join("Scripts").join("python.exe");
        let candidates = [(venv, vec![]), (std::path::PathBuf::from("py"), vec!["-3"])];
        candidates.into_iter().find(|(exe, args)| {
            std::process::Command::new(exe)
                .args(args)
                .args(["-c", "import iterm2"])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .is_ok_and(|s| s.success())
        })
    }

    /// The stock `iterm2` package, unmodified, connects on port 1912 with a session cookie and
    /// builds its App from Termurion's answers (the fixture model through `route` and `answer`).
    /// Run with `scripts/xt test --lib stock_iterm2 -- --ignored`.
    #[test]
    #[ignore]
    fn stock_iterm2_package_connects() {
        let Some((python, args)) = python_with_iterm2() else {
            eprintln!(
                "skipped: no Python with the iterm2 package. This test never installs anything; to run it, \
                 prepare target\\api-venv yourself (py -3 -m venv target\\api-venv, then its pip install iterm2 \"websockets<14\")."
            );
            return;
        };
        let jar = Arc::new(Mutex::new(crate::api_auth::Jar::default()));
        let cookie = jar.lock().unwrap().mint(crate::api_auth::CookieUse::Session);
        let check: CookieCheck = {
            let jar = jar.clone();
            Arc::new(move |c: &str| jar.lock().unwrap().consume(c))
        };
        let s = match Server::start(api_http::PORT, Arc::new(crate::api_peer::check), check, Arc::new(|| {})) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("skipped: port {} is taken: {e}", api_http::PORT);
                return;
            }
        };
        let dir = std::env::temp_dir().join(format!("termurion-stock-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("stock_get_app.py");
        std::fs::write(&script, "import iterm2\n\nasync def main(connection):\n    print(len((await iterm2.async_get_app(connection)).windows))\n\niterm2.run_until_complete(main)\n").unwrap();
        /// The script's process, killed on every way out of the test (a failed assert included).
        struct Script(Option<std::process::Child>);
        impl Drop for Script {
            fn drop(&mut self) {
                if let Some(c) = self.0.as_mut() {
                    let _ = c.kill();
                    let _ = c.wait();
                }
            }
        }
        let child = std::process::Command::new(&python)
            .args(&args)
            .arg(&script)
            .env("ITERM2_COOKIE", &cookie)
            .env_remove("ITERM2_KEY")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let mut script_process = Script(Some(child));
        let child = script_process.0.as_mut().unwrap();
        let model = crate::api_model::fixture();
        let mut subs = crate::api_dispatch::Subscriptions::default();
        let mut rejected = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(60);
        while child.try_wait().unwrap().is_none() {
            if Instant::now() > deadline {
                let _ = child.kill();
                break;
            }
            match s.try_event() {
                Some(Event::Pending(p)) => {
                    assert!(p.upgrade.preauthorized, "the session cookie authorizes the package");
                    s.verdict(p.conn, true);
                }
                Some(Event::Message { conn, bytes }) => {
                    let Some(req) = crate::api_proto::decode(&bytes) else { continue };
                    let reply = match crate::api_dispatch::route(&mut subs, conn, req) {
                        crate::api_dispatch::Route::Reply(m) => m,
                        crate::api_dispatch::Route::App(req) => crate::api_model::answer(&req, &model).unwrap_or_else(|| crate::api_proto::malformed(&req)),
                    };
                    s.send(conn, crate::api_proto::encode(&reply));
                }
                Some(Event::Rejected { reason, .. }) => rejected.push(reason),
                Some(Event::Closed { .. }) => {}
                None => std::thread::sleep(Duration::from_millis(5)),
            }
        }
        let out = script_process.0.take().unwrap().wait_with_output().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        let (stdout, stderr) = (String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        assert_eq!(stdout.trim(), "2", "stderr: {stderr}\nrefusals: {rejected:?}");
    }
}
