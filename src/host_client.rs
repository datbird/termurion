//! Termurion's side of the session host: one connection, and a `HostedPty` per session with the
//! same methods as `Pty`, so a pane runs its shell in the host or in Termurion alike.

use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use crate::host_proto::{Decoder, Live, MIN_VERSION, Msg, VERSION, encode};
use crate::pty::{Notify, Read};
use crate::ssh_transport::Transport;
use crate::winpipe::Pipe;

/// What the host said at the handshake.
#[derive(Clone, Debug)]
pub struct Welcome {
    /// The protocol version both ends speak.
    pub version: u32,
    pub instance: String,
    pub host_pid: u32,
    pub sessions: Vec<Live>,
}

/// Why Termurion could not use the host.
#[derive(Debug)]
pub enum HostError {
    /// Another Termurion is attached: this one is a second copy.
    Busy(String),
    Failed(String),
}

impl std::fmt::Display for HostError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            HostError::Busy(s) | HostError::Failed(s) => f.write_str(s),
        }
    }
}

impl From<String> for HostError {
    fn from(s: String) -> HostError {
        HostError::Failed(s)
    }
}

/// `Client::launch_transport`'s answer when the host speaks a protocol older than
/// `ssh_transport::HOST_VERSION`.
pub const HOST_TOO_OLD: &str = "the session host is too old for this transport";

/// How long a launch or an attach waits for the host's answer.
const REPLY_WAIT: Duration = Duration::from_secs(10);

/// One session's incoming side, filled by the reader thread.
#[derive(Default)]
struct Incoming {
    queue: VecDeque<Vec<u8>>,
    /// The output offset after the last byte queued.
    end: u64,
    /// The output offset after the last byte `try_read` gave out.
    taken: u64,
    exited: Option<u32>,
    host_pending: u64,
    /// The connection to the host is gone.
    lost: bool,
    notify: Option<Notify>,
    /// The host's last word on its console: at a password prompt (debounced).
    password_input: bool,
}

type Slot = Arc<Mutex<Incoming>>;

struct Inner {
    pipe: Arc<Pipe>,
    out: mpsc::Sender<Vec<u8>>,
    /// Bytes handed to the writer thread and not yet written to the pipe.
    queued: Arc<AtomicUsize>,
    sessions: Mutex<HashMap<u64, Slot>>,
    launches: Mutex<HashMap<u64, (mpsc::Sender<Result<(u64, u32), String>>, Notify)>>,
    attaches: Mutex<HashMap<u64, mpsc::Sender<(u64, bool)>>>,
    next_tag: AtomicUsize,
    lost: AtomicBool,
    /// The protocol version agreed at the handshake (version 2 reads password prompts).
    version: std::sync::atomic::AtomicU32,
    /// The last `WatchPrompts` sent: None before any.
    watching: Mutex<Option<Option<u32>>>,
}

#[derive(Clone)]
pub struct Client {
    inner: Arc<Inner>,
}

pub struct HostedPty {
    inner: Arc<Inner>,
    id: u64,
    pid: u32,
    slot: Slot,
    detached: bool,
}

/// Waits until nothing is queued for the host, at most `wait`; true when it is empty.
fn wait_drained(queued: &AtomicUsize, wait: Duration) -> bool {
    let end = std::time::Instant::now() + wait;
    while queued.load(Ordering::Relaxed) > 0 {
        if std::time::Instant::now() >= end {
            return false;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    true
}

impl Inner {
    fn send(&self, m: &Msg) -> Result<(), String> {
        if self.lost.load(Ordering::Acquire) {
            return Err("the session host went away".into());
        }
        let f = encode(m);
        self.queued.fetch_add(f.len(), Ordering::Relaxed);
        self.out.send(f).map_err(|_| "the session host went away".to_string())
    }

    fn slot(&self, id: u64) -> Option<Slot> {
        self.sessions.lock().unwrap().get(&id).cloned()
    }
}

fn wake(slot: &Incoming) {
    if let Some(n) = &slot.notify {
        n();
    }
}

/// Reads the host's messages until the connection ends, then marks every session lost.
fn read_loop(inner: Arc<Inner>, welcome: mpsc::Sender<Result<Welcome, HostError>>) {
    let mut welcome = Some(welcome);
    let mut d = Decoder::default();
    let mut buf = vec![0u8; 64 * 1024];
    'read: while let Ok(n) = inner.pipe.read(&mut buf) {
        if n == 0 {
            break;
        }
        d.push(&buf[..n]);
        while let Some(m) = d.next() {
            let Ok(m) = m else { break 'read };
            dispatch(&inner, m, &mut welcome);
        }
    }
    inner.lost.store(true, Ordering::Release);
    crate::rlog!("session host connection lost");
    inner.launches.lock().unwrap().clear();
    inner.attaches.lock().unwrap().clear();
    for slot in inner.sessions.lock().unwrap().values() {
        let mut s = slot.lock().unwrap();
        s.lost = true;
        wake(&s);
    }
    inner.pipe.shutdown();
}

fn dispatch(inner: &Inner, m: Msg, welcome: &mut Option<mpsc::Sender<Result<Welcome, HostError>>>) {
    match m {
        Msg::Welcome { version, instance, host_pid, sessions } => {
            inner.version.store(version, Ordering::Release);
            if let Some(w) = welcome.take() {
                let _ = w.send(Ok(Welcome { version, instance, host_pid, sessions }));
            }
        }
        Msg::PasswordInput { id, on } => {
            if let Some(slot) = inner.slot(id) {
                let mut s = slot.lock().unwrap();
                if s.password_input != on {
                    s.password_input = on;
                    wake(&s);
                }
            }
        }
        Msg::Refused { busy, reason } => {
            if let Some(w) = welcome.take() {
                let text = format!("the session host refused Termurion: {reason}");
                let _ = w.send(Err(if busy { HostError::Busy(text) } else { HostError::Failed(text) }));
            }
        }
        Msg::Launched { tag, id, pid } => {
            let waiting = inner.launches.lock().unwrap().remove(&tag);
            match waiting {
                Some((tx, notify)) => {
                    inner.sessions.lock().unwrap().insert(id, Arc::new(Mutex::new(Incoming { notify: Some(notify), ..Default::default() })));
                    let _ = tx.send(Ok((id, pid)));
                }
                // Nobody waits for it any more (the launch timed out): no shell runs unseen.
                None => {
                    let _ = inner.send(&Msg::Kill { id });
                }
            }
        }
        Msg::LaunchFailed { tag, error } => {
            if let Some((tx, _)) = inner.launches.lock().unwrap().remove(&tag) {
                let _ = tx.send(Err(error));
            }
        }
        Msg::Attached { id, from, dropped } => {
            if let Some(slot) = inner.slot(id) {
                let mut s = slot.lock().unwrap();
                (s.end, s.taken) = (from, from);
            }
            if let Some(tx) = inner.attaches.lock().unwrap().remove(&id) {
                let _ = tx.send((from, dropped));
            }
        }
        Msg::Output { id, offset, bytes } => {
            if let Some(slot) = inner.slot(id) {
                let mut s = slot.lock().unwrap();
                let end = offset + bytes.len() as u64;
                // Bytes already here (a replay overlapping live output) are not taken twice.
                if end > s.end {
                    let skip = s.end.saturating_sub(offset) as usize;
                    s.queue.push_back(bytes[skip..].to_vec());
                    s.end = end;
                    wake(&s);
                }
            }
        }
        Msg::Pending { id, bytes } => {
            if let Some(slot) = inner.slot(id) {
                slot.lock().unwrap().host_pending = bytes;
            }
        }
        Msg::Exited { id, code } => {
            if let Some(slot) = inner.slot(id) {
                let mut s = slot.lock().unwrap();
                s.exited = Some(code);
                wake(&s);
            }
            // An attach to a session the host no longer has ends here.
            if let Some(tx) = inner.attaches.lock().unwrap().remove(&id) {
                let _ = tx.send((0, false));
            }
        }
        _ => {}
    }
}

impl Client {
    /// Connects to the host at `pipe` and says hello. None when no host listens there. The
    /// host must run as this user.
    pub fn connect(pipe: &str, wait: Duration) -> Result<Option<(Client, Welcome)>, HostError> {
        let mut version = VERSION;
        loop {
            match Client::connect_as(pipe, wait, version) {
                // A host from an older Termurion refuses this version (and says so, not busy): try
                // the next lower one on a new connection, so a version 2 host keeps password
                // prompts and signals. Only a version 3 host gives new sessions their environment;
                // a version 1 host's password prompts read as unknown.
                Err(HostError::Failed(_)) if version > MIN_VERSION => version -= 1,
                other => return other,
            }
        }
    }

    fn connect_as(pipe: &str, wait: Duration, version: u32) -> Result<Option<(Client, Welcome)>, HostError> {
        let Some(p) = crate::winpipe::connect(pipe, wait)? else { return Ok(None) };
        p.server_is_this_user()?;
        let pipe = Arc::new(p);
        let (out, frames) = mpsc::channel::<Vec<u8>>();
        let queued = Arc::new(AtomicUsize::new(0));
        let (writer, left) = (pipe.clone(), queued.clone());
        std::thread::spawn(move || {
            for f in frames {
                let r = writer.write_all(&f);
                left.fetch_sub(f.len(), Ordering::Relaxed);
                if r.is_err() {
                    writer.shutdown();
                    break;
                }
            }
        });
        let inner = Arc::new(Inner {
            pipe,
            out,
            queued,
            sessions: Mutex::new(HashMap::new()),
            launches: Mutex::new(HashMap::new()),
            attaches: Mutex::new(HashMap::new()),
            next_tag: AtomicUsize::new(1),
            lost: AtomicBool::new(false),
            version: std::sync::atomic::AtomicU32::new(version),
            watching: Mutex::new(None),
        });
        let (tx, rx) = mpsc::channel();
        let reader = inner.clone();
        std::thread::spawn(move || read_loop(reader, tx));
        let client = Client { inner };
        client.inner.send(&Msg::Hello { version })?;
        match rx.recv_timeout(wait.max(REPLY_WAIT)) {
            Ok(Ok(w)) => {
                crate::rlog!("session host connected, protocol {}", version);
                Ok(Some((client, w)))
            }
            Ok(Err(e)) => {
                client.close();
                Err(e)
            }
            Err(_) => {
                client.close();
                Err(HostError::Failed("the session host did not answer".into()))
            }
        }
    }

    /// Starts a session in the host. `env` goes over the host's environment; a host older than
    /// version 3 ignores it (the field was always on the wire).
    pub fn launch(&self, cmdline: &str, cwd: Option<&Path>, env: &[String], cols: u16, rows: u16, notify: Notify) -> Result<HostedPty, String> {
        self.launch_transport(Transport::Conpty, cmdline, cwd, env, cols, rows, notify)
    }

    /// `launch` on transport `t`: Conpty sends `Launch`, any other `LaunchWith`, which only a
    /// host speaking `ssh_transport::HOST_VERSION` or later understands; an older one gives
    /// `Err(HOST_TOO_OLD)` and is sent nothing.
    #[allow(clippy::too_many_arguments)]
    pub fn launch_transport(&self, t: Transport, cmdline: &str, cwd: Option<&Path>, env: &[String], cols: u16, rows: u16, notify: Notify) -> Result<HostedPty, String> {
        if t != Transport::Conpty && self.inner.version.load(Ordering::Acquire) < crate::ssh_transport::HOST_VERSION {
            return Err(HOST_TOO_OLD.into());
        }
        let tag = self.inner.next_tag.fetch_add(1, Ordering::Relaxed) as u64;
        let (tx, rx) = mpsc::channel();
        self.inner.launches.lock().unwrap().insert(tag, (tx, notify));
        let cwd = cwd.map(|d| d.display().to_string()).unwrap_or_default();
        let (cmdline, env) = (cmdline.to_string(), env.to_vec());
        let m = match t {
            Transport::Conpty => Msg::Launch { tag, cmdline, cwd, env, cols, rows },
            _ => Msg::LaunchWith { tag, cmdline, cwd, env, cols, rows, transport: crate::ssh_transport::wire(t) },
        };
        if let Err(e) = self.inner.send(&m) {
            self.inner.launches.lock().unwrap().remove(&tag);
            return Err(e);
        }
        match rx.recv_timeout(REPLY_WAIT) {
            Ok(Ok((id, pid))) => {
                let slot = self.inner.slot(id).ok_or("the session host lost the new session")?;
                Ok(HostedPty { inner: self.inner.clone(), id, pid, slot, detached: false })
            }
            Ok(Err(e)) => Err(e),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                self.inner.launches.lock().unwrap().remove(&tag);
                Err("the session host did not start the shell in time".into())
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => Err("the session host went away".into()),
        }
    }

    /// Takes over a running session: its output from `since` on. Returns the pty, the offset
    /// the output starts at, and whether bytes before it were dropped.
    pub fn attach(&self, id: u64, pid: u32, since: u64, notify: Notify) -> Result<(HostedPty, u64, bool), String> {
        let slot: Slot = Arc::new(Mutex::new(Incoming { notify: Some(notify), end: since, taken: since, ..Default::default() }));
        self.inner.sessions.lock().unwrap().insert(id, slot.clone());
        let (tx, rx) = mpsc::channel();
        self.inner.attaches.lock().unwrap().insert(id, tx);
        let pty = HostedPty { inner: self.inner.clone(), id, pid, slot, detached: false };
        self.inner.send(&Msg::Attach { id, since })?;
        match rx.recv_timeout(REPLY_WAIT) {
            Ok((from, dropped)) => Ok((pty, from, dropped)),
            Err(_) => {
                // The shell keeps running in the host (a later launch adopts it).
                let mut pty = pty;
                pty.detach();
                Err("the session host did not hand over the session".into())
            }
        }
    }

    /// Asks the host to read its sessions' consoles for password prompts with this debounce
    /// (seconds), or to stop (None). Sent when it changes, and only to a version 2 host; a
    /// change clears every session's prompt state until the host reads again.
    pub fn watch_prompts(&self, debounce: Option<f64>) {
        if self.inner.version.load(Ordering::Acquire) < 2 {
            return;
        }
        let ms = debounce.map(|d| (d.max(0.0) * 1000.0).round().min(u32::MAX as f64) as u32);
        let mut last = self.inner.watching.lock().unwrap();
        if *last != Some(ms) && self.inner.send(&Msg::WatchPrompts { debounce_ms: ms }).is_ok() {
            *last = Some(ms);
            // The host starts its readings over: what it said before no longer holds (it
            // also says so for each session that was on, and an older host would not).
            for slot in self.inner.sessions.lock().unwrap().values() {
                let mut s = slot.lock().unwrap();
                if s.password_input {
                    s.password_input = false;
                    wake(&s);
                }
            }
        }
    }

    /// An ended session nobody claimed: the host drops it.
    pub fn forget(&self, id: u64) {
        let _ = self.inner.send(&Msg::Forget { id });
    }

    /// Quit: the host ends every session.
    pub fn kill_all(&self) {
        let _ = self.inner.send(&Msg::KillAll);
    }

    /// The host went away (it crashed or was ended).
    pub fn is_lost(&self) -> bool {
        self.inner.lost.load(Ordering::Acquire)
    }

    /// Leaves the host: the sessions keep running there. Waits (briefly) for queued frames.
    pub fn close(&self) {
        // A host that speaks version 5 tells a clean leave from a vanish by this (an older one
        // would break on the unknown message, so it is not sent there).
        if self.inner.version.load(Ordering::Acquire) >= 5 {
            let _ = self.inner.send(&Msg::Goodbye);
        }
        wait_drained(&self.inner.queued, Duration::from_secs(2));
        self.inner.pipe.shutdown();
    }

    /// Ends every session and waits, at most `wait`, for the request to be written to the host
    /// (logoff: the process may end as soon as the handler returns). False when it was not.
    pub fn kill_all_and_wait(&self, wait: Duration) -> bool {
        self.kill_all();
        wait_drained(&self.inner.queued, wait)
    }
}

impl HostedPty {
    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// The output offset after the last byte read: a reattach asks for output from here.
    pub fn offset(&self) -> u64 {
        self.slot.lock().unwrap().taken
    }

    /// Ends the program now: the host drops the session, which kills the process. The session
    /// reads as exited here at once, so the pane closes like any program that ended.
    pub fn terminate(&self) {
        let _ = self.inner.send(&Msg::Kill { id: self.id });
        let notify = {
            let mut s = self.slot.lock().unwrap();
            s.exited.get_or_insert(1);
            s.notify.clone()
        };
        if let Some(n) = notify {
            n();
        }
    }

    /// The session stays in the host when this goes (quit with jobs kept).
    pub fn detach(&mut self) {
        self.detached = true;
    }

    pub fn try_read(&self) -> Read {
        let mut s = self.slot.lock().unwrap();
        match s.queue.pop_front() {
            Some(d) => {
                s.taken += d.len() as u64;
                Read::Data(d)
            }
            None if s.lost => Read::Closed,
            None => Read::Empty,
        }
    }

    pub fn write(&self, bytes: &[u8]) -> Result<(), String> {
        self.inner.send(&Msg::Write { id: self.id, bytes: bytes.to_vec() })
    }

    pub fn resize(&self, cols: u16, rows: u16) -> Result<(), String> {
        self.inner.send(&Msg::Resize { id: self.id, cols, rows })
    }

    pub fn clear(&self) -> Result<(), String> {
        self.inner.send(&Msg::Clear { id: self.id })
    }

    pub fn set_paused(&self, paused: bool) {
        let _ = self.inner.send(&Msg::Pause { id: self.id, paused });
    }

    /// Send Signal's Ctrl+C (or Ctrl+Break) to this session's console, sent by the host (the
    /// console is the host's: Termurion never attaches to it). A version 1 host cannot.
    pub fn signal(&self, ctrl_break: bool) -> Result<(), String> {
        if self.inner.version.load(Ordering::Acquire) < 2 {
            return Err("the session host is too old to send console signals; quit Termurion with no sessions kept to update it".into());
        }
        self.inner.send(&Msg::Signal { id: self.id, ctrl_break })
    }

    /// Bytes on their way to the shell: queued here, plus what the host has not written.
    pub fn pending(&self) -> usize {
        self.inner.queued.load(Ordering::Relaxed) + self.slot.lock().unwrap().host_pending as usize
    }

    /// The host's session is at a password prompt (debounced); false when the host never
    /// said (an older host), or the connection is gone.
    pub fn password_input(&self) -> bool {
        let s = self.slot.lock().unwrap();
        s.password_input && !s.lost
    }

    /// The exit code; a host that went away took the shell with it (0xFFFFFFFF).
    pub fn exit_code(&self) -> Option<u32> {
        let s = self.slot.lock().unwrap();
        s.exited.or(s.lost.then_some(u32::MAX))
    }
}

impl Drop for HostedPty {
    fn drop(&mut self) {
        self.inner.sessions.lock().unwrap().remove(&self.id);
        if !self.detached {
            let _ = self.inner.send(&Msg::Kill { id: self.id });
        }
    }
}

/// The session host program beside the app; `start_host_process` starts it by this file name.
pub const HOST_EXE: &str = "termurion-host.exe";

/// Starts `termurion-host.exe` from `exe_dir`, detached from Termurion (its own process group,
/// out of Termurion's job when Windows allows), so it outlives Termurion.
pub fn start_host_process(exe_dir: &Path) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
    let exe = exe_dir.join(HOST_EXE);
    let spawn = |flags: u32| {
        std::process::Command::new(&exe)
            .creation_flags(flags)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
    };
    let base = DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP;
    // A job that forbids breakaway refuses the flag; the host then lives in Termurion's job.
    spawn(base | CREATE_BREAKAWAY_FROM_JOB).or_else(|_| spawn(base)).map(drop).map_err(|e| format!("could not start {}: {e}", exe.display()))
}

/// Connects to this user's host, starting it when none runs, waiting up to `wait`.
pub fn connect_or_start(exe_dir: &Path, wait: Duration) -> Result<(Client, Welcome), HostError> {
    let pipe = crate::winpipe::host_pipe_name()?;
    if let Some(c) = Client::connect(&pipe, Duration::from_secs(1))? {
        return Ok(c);
    }
    start_host_process(exe_dir)?;
    let end = std::time::Instant::now() + wait;
    loop {
        if let Some(c) = Client::connect(&pipe, Duration::from_secs(1))? {
            return Ok(c);
        }
        if std::time::Instant::now() >= end {
            return Err(HostError::Failed(format!("termurion-host did not start within {} s", wait.as_secs_f64())));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::tests::{Conn, start_host};
    use std::time::Instant;

    fn quiet() -> Notify {
        Arc::new(|| {})
    }

    #[test]
    fn a_changed_watch_clears_hosted_prompts() {
        let (name, _host) = start_host(Duration::from_secs(1));
        let (client, _) = connect(&name);
        let pty = client.launch("cmd.exe /d /k", None, &[], 80, 24, quiet()).unwrap();
        client.watch_prompts(Some(0.2));
        pty.slot.lock().unwrap().password_input = true;
        assert!(pty.password_input());
        client.watch_prompts(Some(0.2));
        assert!(pty.password_input(), "the same request again changes nothing");
        client.watch_prompts(Some(0.5));
        assert!(!pty.password_input(), "a changed request clears what the host said before");
        pty.slot.lock().unwrap().password_input = true;
        client.watch_prompts(None);
        assert!(!pty.password_input(), "turning detection off clears it too");
        client.kill_all();
    }

    /// A logoff waits for the kill request to be written: the queue empties while it waits, and a
    /// queue that never empties costs the wait once and reports it.
    #[test]
    fn a_wait_for_the_host_lasts_until_the_queue_is_empty() {
        let q = Arc::new(AtomicUsize::new(7));
        let l = q.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(150));
            l.store(0, Ordering::Relaxed);
        });
        assert!(super::wait_drained(&q, Duration::from_secs(5)), "the queue emptied");
        assert_eq!(q.load(Ordering::Relaxed), 0, "it waited for the write");
        let stuck = AtomicUsize::new(3);
        let t = std::time::Instant::now();
        assert!(!super::wait_drained(&stuck, Duration::from_millis(200)), "a stuck queue is reported");
        assert!(t.elapsed() >= Duration::from_millis(190), "and waited the whole time");
    }

    /// A host from an older Termurion that speaks up to `speaks`: it refuses each higher version
    /// (not busy), welcomes the client at `speaks`, and returns the first message after that.
    fn older_host(name: String, speaks: u32) -> std::thread::JoinHandle<Msg> {
        use crate::host_proto::Decoder;
        let mut listener = crate::winpipe::Listener::bind(&name).unwrap();
        std::thread::spawn(move || {
            let hello = |p: &Pipe| {
                let mut d = Decoder::default();
                let mut buf = vec![0u8; 4096];
                loop {
                    let n = p.read(&mut buf).unwrap();
                    d.push(&buf[..n]);
                    if let Some(m) = d.next() {
                        return (m.unwrap(), d, buf);
                    }
                }
            };
            for refused in (speaks + 1..=VERSION).rev() {
                let conn = listener.accept(Duration::from_secs(10)).unwrap().expect("a connection");
                let (m, _, _) = hello(&conn);
                assert_eq!(m, Msg::Hello { version: refused }, "one version lower each time");
                conn.write_all(&encode(&Msg::Refused { busy: false, reason: format!("the session host speaks versions 1 to {speaks}, not {refused}") })).unwrap();
                conn.shutdown();
            }
            let conn = listener.accept(Duration::from_secs(10)).unwrap().expect("the last connection");
            let (m, mut d, mut buf) = hello(&conn);
            assert_eq!(m, Msg::Hello { version: speaks });
            conn.write_all(&encode(&Msg::Welcome { version: speaks, host_pid: 1, instance: "old".into(), sessions: vec![] })).unwrap();
            loop {
                if let Some(m) = d.next() {
                    return m.unwrap();
                }
                let n = conn.read(&mut buf).unwrap();
                d.push(&buf[..n]);
            }
        })
    }

    /// A version 1 host refuses versions 3 and 2; the client speaks version 1 to it and never
    /// sends it a version 2 message.
    #[test]
    fn an_older_host_is_spoken_to_in_version_one() {
        let name = crate::winpipe::tests::test_name("oldhost");
        let old_host = older_host(name.clone(), 1);
        let (client, welcome) = Client::connect(&name, Duration::from_secs(5)).unwrap().expect("the old host answers");
        assert_eq!(welcome.version, 1);
        client.watch_prompts(Some(0.2));
        client.kill_all();
        assert_eq!(old_host.join().unwrap(), Msg::KillAll, "no version 2 message reached the old host");
        client.close();
    }

    /// A version 2 host (milestone 10b) refuses version 3; the client speaks version 2 to it, so
    /// password prompts still work there.
    #[test]
    fn a_version_two_host_keeps_version_two() {
        let name = crate::winpipe::tests::test_name("v2host");
        let old_host = older_host(name.clone(), 2);
        let (client, welcome) = Client::connect(&name, Duration::from_secs(5)).unwrap().expect("the version 2 host answers");
        assert_eq!(welcome.version, 2);
        client.watch_prompts(Some(0.2));
        assert_eq!(old_host.join().unwrap(), Msg::WatchPrompts { debounce_ms: Some(200) }, "a version 2 message reaches it");
        client.close();
    }

    /// A version 3 host never sees LaunchWith: a transport launch is refused in Termurion, and a
    /// ConPTY launch still goes out as Launch.
    #[test]
    fn a_version_three_host_is_too_old_for_a_transport() {
        let name = crate::winpipe::tests::test_name("v3host");
        let old_host = older_host(name.clone(), 3);
        let (client, welcome) = Client::connect(&name, Duration::from_secs(5)).unwrap().expect("the version 3 host answers");
        assert_eq!(welcome.version, 3);
        let err = client.launch_transport(Transport::Passthrough, "ssh.exe host", None, &[], 80, 24, quiet()).err();
        assert_eq!(err.as_deref(), Some(HOST_TOO_OLD));
        client.kill_all();
        assert_eq!(old_host.join().unwrap(), Msg::KillAll, "nothing reached the old host before KillAll");
        client.close();
    }

    #[test]
    fn a_transport_launch_runs_in_the_host() {
        let (name, _host) = start_host(Duration::from_secs(1));
        let (client, welcome) = connect(&name);
        assert_eq!(welcome.version, VERSION);
        let pty = client.launch_transport(Transport::Pipes, "cmd.exe /d /c echo client-pipes", None, &[], 80, 24, quiet()).unwrap();
        read_until(&pty, "client-pipes");
        client.kill_all();
    }

    fn connect(name: &str) -> (Client, Welcome) {
        let end = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(c) = Client::connect(name, Duration::from_secs(5)).unwrap() {
                return c;
            }
            assert!(Instant::now() < end, "no host");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn read_until(p: &HostedPty, needle: &str) -> String {
        let end = Instant::now() + Duration::from_secs(10);
        let mut all = String::new();
        while !all.contains(needle) {
            assert!(Instant::now() < end, "never saw {needle:?} in {all:?}");
            match p.try_read() {
                Read::Data(d) => all.push_str(&String::from_utf8_lossy(&d)),
                _ => std::thread::sleep(Duration::from_millis(10)),
            }
        }
        all
    }

    #[test]
    fn hosted_pane_runs_a_shell() {
        let (name, _host) = start_host(Duration::from_secs(1));
        let (c, w) = connect(&name);
        assert!(w.sessions.is_empty() && !w.instance.is_empty());
        let woke = Arc::new(AtomicUsize::new(0));
        let w2 = woke.clone();
        let p = c.launch("cmd.exe /d /c echo hosted-hi & ping -n 30 127.0.0.1 >nul", None, &[], 80, 24, Arc::new(move || {
            w2.fetch_add(1, Ordering::Relaxed);
        })).unwrap();
        assert!(p.pid() > 0);
        let text = read_until(&p, "hosted-hi");
        assert_eq!(p.offset(), text.len() as u64, "the offset counts what was read");
        assert!(woke.load(Ordering::Relaxed) > 0, "output wakes the window");
        p.write(b"x").unwrap();
        p.resize(100, 30).unwrap();
        assert_eq!(p.exit_code(), None);
    }

    #[test]
    fn a_busy_host_says_so() {
        let (name, _host) = start_host(Duration::from_secs(1));
        let _first = connect(&name);
        match Client::connect(&name, Duration::from_secs(5)) {
            Err(HostError::Busy(_)) => {}
            Err(e) => panic!("not busy: {e}"),
            Ok(_) => panic!("a second Termurion was let in"),
        }
    }

    #[test]
    fn an_attached_pane_sets_its_size() {
        let (name, _host) = start_host(Duration::from_secs(1));
        let (c, _) = connect(&name);
        let mut p = c.launch("cmd.exe /d /c ping -n 30 127.0.0.1 >nul", None, &[], 80, 24, quiet()).unwrap();
        let (id, pid) = (p.id(), p.pid());
        p.detach();
        drop(p);
        c.close();
        std::thread::sleep(Duration::from_millis(300));
        let (c, _) = connect(&name);
        let (pty, _, _) = c.attach(id, pid, 0, quiet()).unwrap();
        let mut pane = crate::pane::Pane::attached(1, pty, 100, 30);
        pane.detach();
        drop(pane);
        std::thread::sleep(Duration::from_millis(300));
        c.close();
        std::thread::sleep(Duration::from_millis(300));
        let raw = Conn::open(&name);
        let s = raw.hello().into_iter().find(|l| l.id == id).unwrap();
        assert_eq!((s.cols, s.rows), (100, 30), "the host's pty took the pane's size");
        raw.send(&Msg::KillAll);
    }

    #[test]
    fn hosted_exit_code_arrives() {
        let (name, _host) = start_host(Duration::from_secs(1));
        let (c, _) = connect(&name);
        let p = c.launch("cmd.exe /d /c exit 7", None, &[], 80, 24, quiet()).unwrap();
        let end = Instant::now() + Duration::from_secs(10);
        while p.exit_code().is_none() {
            assert!(Instant::now() < end, "no exit");
            let _ = p.try_read();
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(p.exit_code(), Some(7));
    }

    #[test]
    fn drop_kills_the_session() {
        let (name, _host) = start_host(Duration::from_secs(1));
        let (c, _) = connect(&name);
        let kept = c.launch("cmd.exe /d /c ping -n 30 127.0.0.1 >nul", None, &[], 80, 24, quiet()).unwrap();
        let gone = c.launch("cmd.exe /d /c ping -n 30 127.0.0.1 >nul", None, &[], 80, 24, quiet()).unwrap();
        let (kept_id, gone_id) = (kept.id(), gone.id());
        drop(gone);
        let mut detached = kept;
        detached.detach();
        drop(detached);
        std::thread::sleep(Duration::from_millis(300));
        c.close();
        std::thread::sleep(Duration::from_millis(300));
        let raw = Conn::open(&name);
        let ids: Vec<u64> = raw.hello().iter().map(|l| l.id).collect();
        assert_eq!(ids, vec![kept_id], "the dropped session ended ({gone_id}); the detached one runs on");
        raw.send(&Msg::KillAll);
    }

    #[test]
    fn reattach_continues_the_output() {
        let (name, _host) = start_host(Duration::from_secs(1));
        let (c, _) = connect(&name);
        let mut p = c.launch("cmd.exe /d /c echo first-part & ping -n 3 127.0.0.1 >nul & echo second-part & ping -n 30 127.0.0.1 >nul", None, &[], 80, 24, quiet()).unwrap();
        let first = read_until(&p, "first-part");
        let (id, at) = (p.id(), p.offset());
        p.detach();
        drop(p);
        c.close();
        std::thread::sleep(Duration::from_secs(3));
        let (c, w) = connect(&name);
        assert!(w.sessions.iter().any(|l| l.id == id));
        let (p, from, dropped) = c.attach(id, 0, at, quiet()).unwrap();
        assert_eq!((from, dropped), (at, false));
        let rest = read_until(&p, "second-part");
        assert!(!rest.contains("first-part"), "nothing read before comes again: {first:?} then {rest:?}");
        c.kill_all();
    }
}
