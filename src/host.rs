//! tandem-host: runs every shell in its own ConPTY and keeps it alive when Tandem is not
//! attached, so a crashed or force-quit Tandem can reattach (iTerm2's job servers). It serves
//! one Tandem at a time over the per-user pipe, keeps each session's last output in a ring,
//! and exits once no Tandem is attached and no session is left.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use crate::conpty::Api;
use crate::host_proto::{Decoder, Live, Msg, Ring, VERSION, encode};
use crate::pty::{Pty, Read};
use crate::winpipe::{Listener, Pipe};

/// Output a session keeps for a reattach.
pub const RING: usize = 4 * 1024 * 1024;

#[derive(Clone, Copy, Debug)]
pub struct Options {
    /// How long the host stays with no Tandem attached and no session.
    pub grace: Duration,
}

impl Default for Options {
    fn default() -> Options {
        Options { grace: Duration::from_secs(5) }
    }
}

struct Session {
    /// None once the session was killed (it waits only to be forgotten).
    pty: Option<Pty>,
    ring: Ring,
    pid: u32,
    cmdline: String,
    cwd: String,
    cols: u16,
    rows: u16,
    exited: Option<u32>,
    /// Its output goes to the attached Tandem.
    attached: bool,
    pending: usize,
}

struct Client {
    conn: u64,
    out: mpsc::Sender<Vec<u8>>,
}

struct Host {
    sessions: HashMap<u64, Session>,
    next_id: u64,
    client: Option<Client>,
    /// Tells this host run from any other: session ids are only good within it.
    instance: String,
}

impl Host {
    fn send(&self, m: &Msg) {
        if let Some(c) = &self.client {
            let _ = c.out.send(encode(m));
        }
    }

    fn live(&self) -> Vec<Live> {
        let mut out: Vec<Live> = self
            .sessions
            .iter()
            .map(|(id, s)| Live { id: *id, pid: s.pid, cmdline: s.cmdline.clone(), cwd: s.cwd.clone(), cols: s.cols, rows: s.rows, exited: s.exited, end: s.ring.end() })
            .collect();
        out.sort_by_key(|l| l.id);
        out
    }
}

/// Closing a pty can take about 2 s (milestone 0), so it happens on its own thread.
fn drop_later(p: Pty) {
    std::thread::spawn(move || drop(p));
}

type Shared = Arc<Mutex<Host>>;

/// Serves `pipe` until the host is idle for `opts.grace`.
pub fn run(pipe: &str, api: Api, opts: Options) -> Result<(), String> {
    let mut listener = Listener::bind(pipe)?;
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let host: Shared = Arc::new(Mutex::new(Host { sessions: HashMap::new(), next_id: 1, client: None, instance: format!("{}-{nanos}", std::process::id()) }));
    let (wake, woken) = mpsc::channel::<u64>();
    let pumped = host.clone();
    std::thread::spawn(move || {
        for id in woken {
            pump(&pumped, id);
        }
    });
    let mut conns = 0u64;
    let mut idle_since: Option<Instant> = None;
    loop {
        match listener.accept(Duration::from_millis(250)) {
            Ok(Some(p)) => {
                conns += 1;
                serve(host.clone(), Arc::new(p), conns, api.clone(), wake.clone());
            }
            Ok(None) => {}
            // An error here must not end the host (and every shell with it): log, take a
            // fresh pipe instance, and go on.
            Err(e) => {
                log(&format!("accept: {e}"));
                if let Err(e) = listener.renew() {
                    log(&format!("renew: {e}"));
                }
                std::thread::sleep(Duration::from_millis(500));
            }
        }
        let idle = {
            let h = host.lock().unwrap();
            h.client.is_none() && h.sessions.is_empty()
        };
        if !idle {
            idle_since = None;
        } else if idle_since.get_or_insert_with(Instant::now).elapsed() >= opts.grace {
            return Ok(());
        }
    }
}

/// Moves a session's new output into its ring and to the attached Tandem.
fn pump(host: &Shared, id: u64) {
    let mut h = host.lock().unwrap();
    let Some(s) = h.sessions.get_mut(&id) else { return };
    let mut msgs = Vec::new();
    if let Some(p) = &s.pty {
        while let Read::Data(chunk) = p.try_read() {
            let offset = s.ring.end();
            s.ring.push(&chunk);
            if s.attached {
                msgs.push(Msg::Output { id, offset, bytes: chunk });
            }
        }
        let pending = p.pending();
        if s.attached && pending != s.pending {
            msgs.push(Msg::Pending { id, bytes: pending as u64 });
        }
        s.pending = pending;
        if s.exited.is_none() {
            if let Some(code) = p.exit_code() {
                s.exited = Some(code);
                if s.attached {
                    msgs.push(Msg::Exited { id, code });
                }
            }
        }
    }
    for m in &msgs {
        h.send(m);
    }
}

/// One connection: a writer thread for its frames and a reader thread for its requests.
fn serve(host: Shared, pipe: Arc<Pipe>, conn: u64, api: Api, wake: mpsc::Sender<u64>) {
    let (out, frames) = mpsc::channel::<Vec<u8>>();
    let writer = pipe.clone();
    std::thread::spawn(move || {
        for f in frames {
            if writer.write_all(&f).is_err() {
                writer.shutdown();
                break;
            }
        }
    });
    std::thread::spawn(move || {
        let mut d = Decoder::default();
        let mut buf = vec![0u8; 64 * 1024];
        let mut welcomed = false;
        'read: loop {
            let n = match pipe.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            d.push(&buf[..n]);
            while let Some(m) = d.next() {
                let Ok(m) = m else { break 'read };
                if welcomed {
                    handle(&host, m, &api, &wake);
                    continue;
                }
                // The first message must be Hello; only one Tandem at a time.
                let Msg::Hello { version } = m else { break 'read };
                let mut h = host.lock().unwrap();
                let refuse = if h.client.is_some() {
                    Some((true, "another Tandem is attached to the session host".to_string()))
                } else if version != VERSION {
                    Some((false, format!("the session host speaks version {VERSION}, not {version}")))
                } else {
                    None
                };
                if let Some((busy, reason)) = refuse {
                    let _ = out.send(encode(&Msg::Refused { busy, reason }));
                    break 'read;
                }
                h.client = Some(Client { conn, out: out.clone() });
                let welcome = Msg::Welcome { version: VERSION, host_pid: std::process::id(), instance: h.instance.clone(), sessions: h.live() };
                h.send(&welcome);
                welcomed = true;
            }
        }
        let mut h = host.lock().unwrap();
        if h.client.as_ref().is_some_and(|c| c.conn == conn) {
            h.client = None;
            for s in h.sessions.values_mut() {
                s.attached = false;
                // A pane paused for Undo Close must not stay parked with no Tandem to resume
                // it: the host keeps reading into the ring.
                if let Some(p) = &s.pty {
                    p.set_paused(false);
                }
            }
        }
    });
}

fn handle(host: &Shared, m: Msg, api: &Api, wake: &mpsc::Sender<u64>) {
    match m {
        Msg::Launch { tag, cmdline, cwd, env: _, cols, rows } => {
            let id = {
                let mut h = host.lock().unwrap();
                h.next_id += 1;
                h.next_id - 1
            };
            let w = wake.clone();
            let notify: crate::pty::Notify = Arc::new(move || {
                let _ = w.send(id);
            });
            let dir = (!cwd.is_empty()).then(|| std::path::PathBuf::from(&cwd));
            match crate::pty::spawn_notify(api, &cmdline, dir.as_deref(), cols.max(1), rows.max(1), notify) {
                Ok(pty) => {
                    let pid = pty.pid();
                    let mut h = host.lock().unwrap();
                    h.sessions.insert(id, Session { pty: Some(pty), ring: Ring::new(RING), pid, cmdline, cwd, cols, rows, exited: None, attached: true, pending: 0 });
                    h.send(&Msg::Launched { tag, id, pid });
                    drop(h);
                    // Output that came before the session was in the table.
                    pump(host, id);
                }
                Err(error) => host.lock().unwrap().send(&Msg::LaunchFailed { tag, error }),
            }
        }
        Msg::Attach { id, since } => {
            let mut h = host.lock().unwrap();
            let Some(s) = h.sessions.get_mut(&id) else {
                // No such session: it reads as ended.
                h.send(&Msg::Exited { id, code: u32::MAX });
                return;
            };
            s.attached = true;
            if let Some(p) = &s.pty {
                p.set_paused(false);
            }
            let (from, dropped, bytes) = s.ring.since(since);
            let exited = s.exited;
            h.send(&Msg::Attached { id, from, dropped });
            if !bytes.is_empty() {
                h.send(&Msg::Output { id, offset: from, bytes });
            }
            if let Some(code) = exited {
                h.send(&Msg::Exited { id, code });
            }
        }
        Msg::Write { id, bytes } => with_pty(host, id, |p| p.write(&bytes)),
        Msg::Resize { id, cols, rows } => {
            let mut h = host.lock().unwrap();
            if let Some(s) = h.sessions.get_mut(&id) {
                (s.cols, s.rows) = (cols, rows);
            }
            drop(h);
            with_pty(host, id, |p| p.resize(cols, rows));
        }
        Msg::Clear { id } => with_pty(host, id, |p| p.clear()),
        Msg::Pause { id, paused } => with_pty(host, id, |p| {
            p.set_paused(paused);
            Ok(())
        }),
        Msg::Forget { id } | Msg::Kill { id } => {
            if let Some(p) = host.lock().unwrap().sessions.remove(&id).and_then(|s| s.pty) {
                drop_later(p);
            }
        }
        Msg::KillAll => {
            let all: Vec<Session> = host.lock().unwrap().sessions.drain().map(|(_, s)| s).collect();
            for p in all.into_iter().filter_map(|s| s.pty) {
                drop_later(p);
            }
        }
        _ => {}
    }
}

/// Runs `f` on a session's pty. The host has no window, so a failure goes to its log.
fn with_pty(host: &Shared, id: u64, f: impl FnOnce(&Pty) -> Result<(), String>) {
    let h = host.lock().unwrap();
    if let Some(p) = h.sessions.get(&id).and_then(|s| s.pty.as_ref()) {
        if let Err(e) = f(p) {
            log(&format!("session {id}: {e}"));
        }
    }
}

/// `host.log` beside Tandem's settings.
pub fn log_path() -> std::path::PathBuf {
    crate::globals::default_path().with_file_name("host.log")
}

/// Appends a line to the host log (the host has no window and no console).
pub fn log(line: &str) {
    use std::io::Write;
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(log_path()) {
        let _ = writeln!(f, "{stamp} [{}] {line}", std::process::id());
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::host_proto::{Decoder, Live, Msg, encode};
    use crate::winpipe::{self, Pipe};
    use std::sync::Arc;
    use std::sync::mpsc::{self, Receiver};
    use std::time::Instant;

    pub(crate) fn api() -> Api {
        crate::conpty::load(&crate::conpty::Source::Shipped(crate::conpty::dev_vendor_dir())).unwrap()
    }

    /// A host on its own pipe, in a thread.
    pub(crate) fn start_host(grace: Duration) -> (String, std::thread::JoinHandle<Result<(), String>>) {
        let name = winpipe::tests::test_name("host");
        let n2 = name.clone();
        let t = std::thread::spawn(move || run(&n2, api(), Options { grace }));
        (name, t)
    }

    pub(crate) struct Conn {
        pipe: Arc<Pipe>,
        rx: Receiver<Msg>,
    }

    impl Conn {
        pub(crate) fn open(name: &str) -> Conn {
            let end = Instant::now() + Duration::from_secs(5);
            let pipe = loop {
                if let Some(p) = winpipe::connect(name, Duration::from_secs(1)).unwrap() {
                    break Arc::new(p);
                }
                assert!(Instant::now() < end, "the host never listened");
                std::thread::sleep(Duration::from_millis(20));
            };
            let (tx, rx) = mpsc::channel();
            let p2 = pipe.clone();
            std::thread::spawn(move || {
                let mut d = Decoder::default();
                let mut buf = vec![0u8; 65536];
                while let Ok(n) = p2.read(&mut buf) {
                    if n == 0 {
                        break;
                    }
                    d.push(&buf[..n]);
                    while let Some(Ok(m)) = d.next() {
                        if tx.send(m).is_err() {
                            return;
                        }
                    }
                }
            });
            Conn { pipe, rx }
        }

        pub(crate) fn send(&self, m: &Msg) {
            self.pipe.write_all(&encode(m)).unwrap();
        }

        /// The first message `pick` accepts, within 10 s.
        pub(crate) fn wait<T>(&self, mut pick: impl FnMut(&Msg) -> Option<T>) -> T {
            let end = Instant::now() + Duration::from_secs(10);
            loop {
                let left = end.saturating_duration_since(Instant::now());
                let m = self.rx.recv_timeout(left).expect("the host said nothing in time");
                if let Some(t) = pick(&m) {
                    return t;
                }
            }
        }

        pub(crate) fn hello(&self) -> Vec<Live> {
            self.send(&Msg::Hello { version: crate::host_proto::VERSION });
            self.wait(|m| match m {
                Msg::Welcome { sessions, .. } => Some(sessions.clone()),
                _ => None,
            })
        }

        pub(crate) fn launch(&self, cmdline: &str) -> u64 {
            self.send(&Msg::Launch { tag: 1, cmdline: cmdline.into(), cwd: String::new(), env: vec![], cols: 80, rows: 24 });
            self.wait(|m| match m {
                Msg::Launched { id, .. } => Some(*id),
                Msg::LaunchFailed { error, .. } => panic!("{error}"),
                _ => None,
            })
        }

        /// Output of session `id` until it holds `needle`.
        pub(crate) fn output_until(&self, id: u64, needle: &str) -> String {
            let mut all = String::new();
            self.wait(|m| {
                if let Msg::Output { id: i, bytes, .. } = m {
                    if *i == id {
                        all.push_str(&String::from_utf8_lossy(bytes));
                    }
                }
                all.contains(needle).then_some(())
            });
            all
        }

        pub(crate) fn close(self) {
            self.pipe.shutdown();
        }
    }

    #[test]
    fn host_launches_and_echoes() {
        let (name, _host) = start_host(Duration::from_secs(1));
        let c = Conn::open(&name);
        assert!(c.hello().is_empty());
        let id = c.launch("cmd.exe /d /c echo hello-host");
        c.output_until(id, "hello-host");
        assert_eq!(c.wait(|m| matches!(m, Msg::Exited { id: i, .. } if *i == id).then_some(())), ());
    }

    #[test]
    fn output_waits_while_detached() {
        let (name, _host) = start_host(Duration::from_secs(1));
        let c = Conn::open(&name);
        c.hello();
        let id = c.launch("cmd.exe /d /c ping -n 2 127.0.0.1 >nul & echo later-text & ping -n 30 127.0.0.1 >nul");
        c.close();
        std::thread::sleep(Duration::from_secs(3));
        let c = Conn::open(&name);
        let live = c.hello();
        let s = live.iter().find(|l| l.id == id).expect("the session is still there");
        assert!(s.end > 0 && s.exited.is_none(), "{s:?}");
        c.send(&Msg::Attach { id, since: 0 });
        assert_eq!(c.wait(|m| match m {
            Msg::Attached { from, dropped, .. } => Some((*from, *dropped)),
            _ => None,
        }), (0, false));
        c.output_until(id, "later-text");
        c.send(&Msg::KillAll);
    }

    #[test]
    fn detach_resumes_a_paused_session() {
        let (name, _host) = start_host(Duration::from_secs(1));
        let c = Conn::open(&name);
        c.hello();
        let id = c.launch("cmd.exe /d /c echo before-pause & ping -n 3 127.0.0.1 >nul & echo after-pause & ping -n 30 127.0.0.1 >nul");
        c.output_until(id, "before-pause");
        c.send(&Msg::Pause { id, paused: true });
        std::thread::sleep(Duration::from_millis(200));
        c.close();
        std::thread::sleep(Duration::from_millis(3500));
        let c = Conn::open(&name);
        c.hello();
        c.send(&Msg::Attach { id, since: 0 });
        c.output_until(id, "after-pause");
        c.send(&Msg::KillAll);
    }

    #[test]
    fn second_client_is_refused() {
        let (name, _host) = start_host(Duration::from_secs(1));
        let a = Conn::open(&name);
        a.hello();
        let b = Conn::open(&name);
        b.send(&Msg::Hello { version: crate::host_proto::VERSION });
        b.wait(|m| matches!(m, Msg::Refused { .. }).then_some(()));
        let id = a.launch("cmd.exe /d /c echo still-mine");
        a.output_until(id, "still-mine");
    }

    #[test]
    fn exited_sessions_wait_for_forget() {
        let (name, host) = start_host(Duration::from_millis(500));
        let c = Conn::open(&name);
        c.hello();
        let id = c.launch("cmd.exe /d /c exit 3");
        let code = c.wait(|m| match m {
            Msg::Exited { id: i, code } if *i == id => Some(*code),
            _ => None,
        });
        assert_eq!(code, 3);
        c.close();
        std::thread::sleep(Duration::from_secs(1));
        let c = Conn::open(&name);
        let live = c.hello();
        assert_eq!(live.iter().find(|l| l.id == id).map(|l| l.exited), Some(Some(3)), "an ended session waits for Tandem");
        c.send(&Msg::Forget { id });
        std::thread::sleep(Duration::from_millis(200));
        c.close();
        // Nothing left and nobody attached: the host ends.
        let end = Instant::now() + Duration::from_secs(10);
        while !host.is_finished() {
            assert!(Instant::now() < end, "the host did not exit when idle");
            std::thread::sleep(Duration::from_millis(50));
        }
        host.join().unwrap().unwrap();
    }
}
