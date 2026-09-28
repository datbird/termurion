//! tandem-host: runs every shell in its own ConPTY and keeps it alive when Tandem is not
//! attached, so a crashed or force-quit Tandem can reattach (iTerm2's job servers). It serves
//! one Tandem at a time over the per-user pipe, keeps each session's last output in a ring,
//! and exits once no Tandem is attached and no session is left.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use crate::conpty::Api;
use crate::host_proto::{Decoder, Live, MIN_VERSION, Msg, Ring, VERSION, encode};
use crate::pty::{Pty, Read};
use crate::ssh_transport::Transport;
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
    /// Its console's password prompt samples, debounced, while the attached Tandem asks.
    prompt: crate::pwm_prompt::Debounce,
}

struct Client {
    conn: u64,
    out: mpsc::Sender<Vec<u8>>,
    /// The protocol version agreed at the handshake.
    version: u32,
}

struct Host {
    sessions: HashMap<u64, Session>,
    next_id: u64,
    client: Option<Client>,
    /// Tells this host run from any other: session ids are only good within it.
    instance: String,
    /// The attached Tandem asked for password prompt readings with this debounce (seconds).
    prompt_debounce: Option<f64>,
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

    /// The sessions whose consoles the prompt watch reads: live ones a Tandem shows. Never
    /// one paused for Undo Close (or otherwise not attached): nothing drains its output, so
    /// once its pipe fills OpenConsole blocks every console call on it, and an attach there
    /// would hold `pty::CONSOLE` for good (and every later close with it). Each comes with its
    /// pause flag, checked again under that lock.
    fn prompt_targets(&self) -> Vec<(u64, u32, Arc<std::sync::atomic::AtomicBool>)> {
        let mut out: Vec<_> = self
            .sessions
            .iter()
            .filter(|(_, s)| s.attached && s.exited.is_none())
            .filter_map(|(id, s)| s.pty.as_ref().filter(|p| !p.is_paused()).map(|p| (*id, s.pid, p.pause_flag())))
            .collect();
        out.sort_by_key(|t| t.0);
        out
    }

    /// Password prompt readings start over (a new request, or no Tandem to tell).
    /// Returns a "not at a prompt" for each session that was at one, so a Tandem that keeps
    /// listening never holds a stale "at a prompt".
    fn watch_prompts(&mut self, debounce: Option<f64>) -> Vec<Msg> {
        self.prompt_debounce = debounce;
        let mut off = Vec::new();
        for (id, s) in self.sessions.iter_mut() {
            if s.prompt.on {
                off.push(Msg::PasswordInput { id: *id, on: false });
            }
            s.prompt = Default::default();
        }
        off.sort_by_key(|m| match m {
            Msg::PasswordInput { id, .. } => *id,
            _ => 0,
        });
        off
    }
}

/// How often the host reads its sessions' console input modes while Tandem asks (the job
/// watcher's rate in front).
const PROMPT_EVERY: Duration = Duration::from_millis(500);

/// Reads each live session's console input mode while the attached Tandem asks, and tells it
/// when one reaches or leaves a password prompt. Only the host attaches to its own consoles:
/// it owns every close of them, and `Pty::drop` holds `pty::CONSOLE`, so it is never attached
/// while one closes (which would end the host and every shell with it).
fn watch_prompts(host: std::sync::Weak<Mutex<Host>>) {
    loop {
        std::thread::sleep(PROMPT_EVERY);
        let Some(shared) = host.upgrade() else { return };
        let (debounce, targets) = {
            let h = shared.lock().unwrap();
            let targets = match h.prompt_debounce {
                Some(_) => h.prompt_targets(),
                None => Vec::new(),
            };
            (h.prompt_debounce, targets)
        };
        drop(shared);
        let Some(debounce) = debounce else { continue };
        // Unknown (the mode could not be read, or the session was paused since) is never a
        // prompt.
        let samples: Vec<(u64, bool, Instant)> = targets.into_iter().map(|(id, pid, paused)| (id, crate::pty::console_mode_unless(pid, &paused).is_ok_and(crate::pwm_prompt::at_prompt), Instant::now())).collect();
        let Some(shared) = host.upgrade() else { return };
        let mut h = shared.lock().unwrap();
        // A request changed while these were read: they belong to the old one.
        if h.prompt_debounce != Some(debounce) {
            continue;
        }
        let mut msgs = Vec::new();
        for (id, prompt, at) in samples {
            if let Some(on) = h.sessions.get_mut(&id).and_then(|s| s.prompt.sample(prompt, at, debounce)) {
                msgs.push(Msg::PasswordInput { id, on });
            }
        }
        for m in &msgs {
            h.send(m);
        }
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
    let host: Shared = Arc::new(Mutex::new(Host { sessions: HashMap::new(), next_id: 1, client: None, instance: format!("{}-{nanos}", std::process::id()), prompt_debounce: None }));
    let watched = Arc::downgrade(&host);
    std::thread::spawn(move || watch_prompts(watched));
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
                } else if !(MIN_VERSION..=VERSION).contains(&version) {
                    Some((false, format!("the session host speaks versions {MIN_VERSION} to {VERSION}, not {version}")))
                } else {
                    None
                };
                if let Some((busy, reason)) = refuse {
                    let _ = out.send(encode(&Msg::Refused { busy, reason }));
                    break 'read;
                }
                h.client = Some(Client { conn, out: out.clone(), version });
                // A new Tandem starts with nothing at a prompt: nothing to tell it.
                h.watch_prompts(None);
                let welcome = Msg::Welcome { version, host_pid: std::process::id(), instance: h.instance.clone(), sessions: h.live() };
                h.send(&welcome);
                welcomed = true;
            }
        }
        let mut h = host.lock().unwrap();
        if h.client.as_ref().is_some_and(|c| c.conn == conn) {
            h.client = None;
            // No Tandem to tell.
            h.watch_prompts(None);
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

/// Starts a session on transport `t` and answers `Launched` or `LaunchFailed`.
#[allow(clippy::too_many_arguments)]
fn launch(host: &Shared, api: &Api, wake: &mpsc::Sender<u64>, t: Transport, tag: u64, cmdline: String, cwd: String, env: Vec<String>, cols: u16, rows: u16) {
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
    match crate::pty::spawn_transport(api, t, &cmdline, dir.as_deref(), &env, cols.max(1), rows.max(1), notify) {
        Ok(pty) => {
            let pid = pty.pid();
            let mut h = host.lock().unwrap();
            h.sessions.insert(id, Session { pty: Some(pty), ring: Ring::new(RING), pid, cmdline, cwd, cols, rows, exited: None, attached: true, pending: 0, prompt: Default::default() });
            h.send(&Msg::Launched { tag, id, pid });
            drop(h);
            // Output that came before the session was in the table.
            pump(host, id);
        }
        Err(error) => host.lock().unwrap().send(&Msg::LaunchFailed { tag, error }),
    }
}

fn handle(host: &Shared, m: Msg, api: &Api, wake: &mpsc::Sender<u64>) {
    match m {
        Msg::Launch { tag, cmdline, cwd, env, cols, rows } => launch(host, api, wake, Transport::Conpty, tag, cmdline, cwd, env, cols, rows),
        // Only a version 4 Tandem sends it (`host_client::launch_transport`).
        Msg::LaunchWith { tag, cmdline, cwd, env, cols, rows, transport } => match crate::ssh_transport::from_wire(transport) {
            Some(t) => launch(host, api, wake, t, tag, cmdline, cwd, env, cols, rows),
            None => host.lock().unwrap().send(&Msg::LaunchFailed { tag, error: format!("unknown transport {transport}") }),
        },
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
        // A pause waits out a prompt read in flight, so none runs on a paused console.
        Msg::Pause { id, paused } => with_pty(host, id, |p| {
            if paused {
                p.pause_after_reads();
            } else {
                p.set_paused(false);
            }
            Ok(())
        }),
        // Send Signal to a hosted pane: the host attaches, on its own thread (never while a
        // close runs, never to a paused session); a failure goes to the log.
        Msg::Signal { id, ctrl_break } => {
            let target = host.lock().unwrap().sessions.get(&id).filter(|s| s.exited.is_none()).and_then(|s| s.pty.as_ref().map(|p| (s.pid, p.pause_flag())));
            if let Some((pid, paused)) = target {
                std::thread::spawn(move || {
                    if let Ok(Err(e)) = crate::jobwatch::console_event(pid, ctrl_break, paused).join() {
                        log(&format!("session {id}: signal: {e}"));
                    }
                });
            }
        }
        Msg::Forget { id } | Msg::Kill { id } => {
            if let Some(p) = host.lock().unwrap().sessions.remove(&id).and_then(|s| s.pty) {
                drop_later(p);
            }
        }
        // Only a version 2 Tandem asks (a version 1 one never sends it).
        Msg::WatchPrompts { debounce_ms } => {
            let mut h = host.lock().unwrap();
            if h.client.as_ref().is_some_and(|c| c.version >= 2) {
                for m in h.watch_prompts(debounce_ms.map(|ms| ms as f64 / 1000.0)) {
                    h.send(&m);
                }
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
            self.launch_with(cmdline, vec![])
        }

        pub(crate) fn launch_with(&self, cmdline: &str, env: Vec<String>) -> u64 {
            self.send(&Msg::Launch { tag: 1, cmdline: cmdline.into(), cwd: String::new(), env, cols: 80, rows: 24 });
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

    /// Version 3: Launch's `env` reaches the session's shell.
    #[test]
    fn launch_env_reaches_the_session() {
        let (name, _host) = start_host(Duration::from_secs(1));
        let c = Conn::open(&name);
        c.hello();
        let id = c.launch_with("cmd.exe /d /c echo [%ITERM_PROFILE%^]", vec!["ITERM_PROFILE=Hosted".into()]);
        c.output_until(id, "[Hosted]");
    }

    /// Version 4: LaunchWith runs the session on the transport it names.
    #[test]
    fn launch_with_runs_the_transport() {
        let (name, _host) = start_host(Duration::from_secs(1));
        let c = Conn::open(&name);
        c.hello();
        let launch = |tag: u64, transport: u8| {
            c.send(&Msg::LaunchWith { tag, cmdline: "cmd.exe /d /c echo hosted-pipes".into(), cwd: String::new(), env: vec![], cols: 80, rows: 24, transport });
            c.wait(|m| match m {
                Msg::Launched { tag: t, id, .. } if *t == tag => Some(Ok(*id)),
                Msg::LaunchFailed { tag: t, error } if *t == tag => Some(Err(error.clone())),
                _ => None,
            })
        };
        let id = launch(1, 2).expect("the pipes transport starts");
        c.output_until(id, "hosted-pipes");
        assert_eq!(launch(2, 9), Err("unknown transport 9".to_string()));
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
    fn a_watch_reset_turns_an_on_session_off() {
        let session = |on: bool| {
            let mut prompt = crate::pwm_prompt::Debounce::default();
            if on {
                prompt.sample(true, Instant::now(), 0.0);
            }
            Session { pty: None, ring: Ring::new(16), pid: 1, cmdline: String::new(), cwd: String::new(), cols: 80, rows: 24, exited: None, attached: true, pending: 0, prompt }
        };
        let mut h = Host { sessions: HashMap::new(), next_id: 3, client: None, instance: "t".into(), prompt_debounce: Some(0.2) };
        h.sessions.insert(1, session(true));
        h.sessions.insert(2, session(false));
        let told = h.watch_prompts(Some(0.5));
        assert_eq!(told, vec![Msg::PasswordInput { id: 1, on: false }], "only the session that was on is told it is off");
        assert!(h.sessions.values().all(|s| !s.prompt.on));
        assert_eq!(h.prompt_debounce, Some(0.5));
        assert!(h.watch_prompts(None).is_empty(), "nothing was on any more");
    }

    #[test]
    fn the_prompt_watch_skips_paused_and_unattached_sessions() {
        let api = api();
        let session = |attached: bool| {
            let pty = crate::pty::spawn(&api, "cmd.exe /d /k", 80, 24).unwrap();
            let pid = pty.pid();
            Session { pty: Some(pty), ring: Ring::new(16), pid, cmdline: String::new(), cwd: String::new(), cols: 80, rows: 24, exited: None, attached, pending: 0, prompt: Default::default() }
        };
        let mut h = Host { sessions: HashMap::new(), next_id: 4, client: None, instance: "t".into(), prompt_debounce: Some(0.2) };
        h.sessions.insert(1, session(true));
        h.sessions.insert(2, session(true));
        h.sessions.insert(3, session(false));
        let ids = |h: &Host| h.prompt_targets().into_iter().map(|t| t.0).collect::<Vec<_>>();
        assert_eq!(ids(&h), vec![1, 2], "a session no Tandem shows is not read");
        h.sessions[&2].pty.as_ref().unwrap().pause_after_reads();
        assert_eq!(ids(&h), vec![1], "a session paused for Undo Close is not read");
        // One paused after it was picked is refused without an attach.
        let targets = h.prompt_targets();
        h.sessions[&1].pty.as_ref().unwrap().pause_after_reads();
        let (_, pid, paused) = &targets[0];
        assert_eq!(crate::pty::console_mode_unless(*pid, paused), Err("the console is paused".to_string()));
        h.sessions[&2].pty.as_ref().unwrap().set_paused(false);
        assert_eq!(ids(&h), vec![2], "an undone close is read again");
        for s in h.sessions.drain().map(|(_, s)| s) {
            drop(s.pty);
        }
    }

    /// The child half of `a_signal_goes_through_the_host`: detached (no console, as
    /// tandem-host runs), so the host can attach to its session's console.
    #[test]
    #[ignore]
    fn signal_child() {
        if std::env::var_os("TANDEM_SIGNAL_CHILD").is_none() {
            return;
        }
        // This process ignores Ctrl+C (a console-subsystem test process is not asked by the
        // handler the host installs; tandem-host is a windows-subsystem one). Its sessions
        // inherit that, so the session's program takes Ctrl+C back first.
        unsafe { windows_sys::Win32::System::Console::SetConsoleCtrlHandler(None, 1) };
        let (name, _host) = start_host(Duration::from_secs(1));
        let c = Conn::open(&name);
        c.hello();
        let script = "$k = Add-Type -Name C -Namespace T -PassThru -MemberDefinition '[DllImport(\"kernel32.dll\")] public static extern bool SetConsoleCtrlHandler(IntPtr h, bool add);'; [void]$k::SetConsoleCtrlHandler([IntPtr]::Zero, $false); 'ready'; Start-Sleep 60";
        let id = c.launch(&format!("powershell.exe -NoProfile -EncodedCommand {}", crate::pty::tests::encoded_command(script)));
        c.output_until(id, "ready");
        std::thread::sleep(Duration::from_millis(500));
        c.send(&Msg::Signal { id, ctrl_break: false });
        // Within the 10 s wait, long before the minute is up.
        c.wait(|m| matches!(m, Msg::Exited { id: i, .. } if *i == id).then_some(()));
        c.send(&Msg::KillAll);
    }

    #[test]
    fn a_signal_goes_through_the_host() {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "host::tests::signal_child", "--ignored", "--test-threads=1"])
            .env("TANDEM_SIGNAL_CHILD", "1")
            .creation_flags(DETACHED_PROCESS)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let mut out = child.stdout.take().unwrap();
        let reading = std::thread::spawn(move || {
            let mut s = String::new();
            let _ = std::io::Read::read_to_string(&mut out, &mut s);
            s
        });
        let end = Instant::now() + Duration::from_secs(60);
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break Some(status);
            }
            if Instant::now() >= end {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        let got = reading.join().unwrap();
        assert!(status.is_some_and(|s| s.success()), "the host sent Ctrl+C to its session within 60 s: {status:?}\n{got}");
    }

    #[test]
    fn versions_one_to_three_are_spoken() {
        let (name, _host) = start_host(Duration::from_secs(1));
        // An older Tandem: version 1, and nothing it does not know ever comes back.
        let old = Conn::open(&name);
        old.send(&Msg::Hello { version: 1 });
        assert_eq!(old.wait(|m| match m {
            Msg::Welcome { version, .. } => Some(*version),
            _ => None,
        }), 1);
        let id = old.launch("cmd.exe /d /c echo old-tandem");
        old.output_until(id, "old-tandem");
        old.close();
        std::thread::sleep(Duration::from_millis(300));
        // A version 2 Tandem asks for prompt readings; the stream stays whole.
        let new = Conn::open(&name);
        new.send(&Msg::Hello { version: 2 });
        assert_eq!(new.wait(|m| match m {
            Msg::Welcome { version, .. } => Some(*version),
            _ => None,
        }), 2);
        new.send(&Msg::WatchPrompts { debounce_ms: Some(200) });
        let id = new.launch("cmd.exe /d /c echo new-tandem");
        new.output_until(id, "new-tandem");
        new.send(&Msg::WatchPrompts { debounce_ms: None });
        new.close();
        std::thread::sleep(Duration::from_millis(300));
        let three = Conn::open(&name);
        three.send(&Msg::Hello { version: 3 });
        assert_eq!(three.wait(|m| match m {
            Msg::Welcome { version, .. } => Some(*version),
            _ => None,
        }), 3);
        three.close();
        std::thread::sleep(Duration::from_millis(300));
        let future = Conn::open(&name);
        future.send(&Msg::Hello { version: crate::host_proto::VERSION + 1 });
        assert!(future.wait(|m| match m {
            Msg::Refused { busy, .. } => Some(!*busy),
            _ => None,
        }), "a version it does not speak is refused, not busy");
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
