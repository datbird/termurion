//! termurion-host: runs every shell in its own ConPTY and keeps it alive when Termurion is not
//! attached, so a crashed or force-quit Termurion can reattach (iTerm2's job servers). It serves
//! one Termurion at a time over the per-user pipe, keeps each session's last output in a ring,
//! and exits once no Termurion is attached and no session is left.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use crate::platform::pty::PtyApi;
use crate::host_proto::{Decoder, FLOW_VERSION, Live, MIN_VERSION, Msg, Ring, VERSION, WINDOW, encode};
use crate::platform::pty::{Pty, Read};
use crate::ssh_transport::Transport;
use crate::platform::ipc::{Listener, Pipe};

pub use crate::host_base::{log, log_path};

/// Output a session keeps for a reattach.
pub const RING: usize = 4 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct Options {
    /// How long the host stays with no Termurion attached and no session.
    pub grace: Duration,
    /// The termurion.exe to start when the attached Termurion vanishes (`host_watch`). None
    /// (tests) turns the whole watch off.
    pub reopen: Option<std::path::PathBuf>,
}

impl Default for Options {
    fn default() -> Options {
        Options { grace: Duration::from_secs(5), reopen: None }
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
    /// Its output goes to the attached Termurion.
    attached: bool,
    pending: usize,
    /// Its console's password prompt samples, debounced, while the attached Termurion asks.
    prompt: crate::pwm_prompt::Debounce,
    /// The output offset the attached Termurion has taken up to (`Consumed`).
    acked: u64,
    /// When the host first saw the shell's exit while output was still held, and the ring's
    /// end then.
    exit_seen: Option<(Instant, u64)>,
}

/// How long `Exited` waits for held output after the shell ends: a background writer that
/// outlives the shell must not hold the end for good.
const EXIT_DRAIN_CAP: Duration = Duration::from_millis(200);

/// What can still sit between the shell and the ring when it exits: the reader's budget, one
/// read, and the pty's own buffer (well under the rest). Once the ring has grown by more than
/// this, every byte the shell wrote has been read: what comes now is a background writer's.
const EXIT_DRAIN_BYTES: u64 = (crate::pty_budget::PTY_BUDGET as u64) + (256 << 10);

/// The session's client is a full window behind: `pump` stops reading it until an ack. Only for
/// a client that acks (`flow`); a session nobody shows reads into its ring.
fn held(s: &Session, flow: bool) -> bool {
    at_window(&s.ring, s.acked, s.attached, flow)
}

fn at_window(ring: &Ring, acked: u64, attached: bool, flow: bool) -> bool {
    attached && flow && ring.end().saturating_sub(acked) >= WINDOW
}

/// `pump` joins the chunks waiting for a session into frames of at most this size, so the app
/// still takes at most 64 KiB at a time (a pty read is at most that, so any chunk fits).
pub const MERGE_CAP: usize = 64 << 10;

/// What `pump` reads a session's output from: its pty (a test's list stands in).
trait OutputSource {
    fn try_read(&self) -> Read;
    fn rearm_notify(&self);
}

impl OutputSource for Pty {
    fn try_read(&self) -> Read {
        Pty::try_read(self)
    }

    fn rearm_notify(&self) {
        Pty::rearm_notify(self)
    }
}

/// Reads `src` into `ring` until it is empty (true) or the window holds it. An attached session's
/// bytes go out as `Output` frames of at most `MERGE_CAP`, offsets contiguous.
fn drain(src: &impl OutputSource, ring: &mut Ring, acked: u64, attached: bool, flow: bool, id: u64, msgs: &mut Vec<Msg>) -> bool {
    let mut frame: Option<(u64, Vec<u8>)> = None;
    let mut drained = false;
    while !at_window(ring, acked, attached, flow) {
        let chunk = match src.try_read() {
            Read::Data(c) => c,
            Read::Closed => {
                drained = true;
                break;
            }
            // The reader wakes us again only after a rearm, and a chunk that came just before
            // it did not: read once more. A held pump never rearms (the ack's wake pumps it).
            Read::Empty => {
                src.rearm_notify();
                match src.try_read() {
                    Read::Data(c) => c,
                    _ => {
                        drained = true;
                        break;
                    }
                }
            }
        };
        let offset = ring.end();
        ring.push(&chunk);
        if !attached {
            continue;
        }
        match &mut frame {
            Some((_, f)) if f.len() + chunk.len() <= MERGE_CAP => {
                f.reserve_exact(MERGE_CAP - f.len());
                f.extend_from_slice(&chunk);
            }
            _ => {
                if let Some((offset, bytes)) = frame.replace((offset, chunk)) {
                    msgs.push(Msg::Output { id, offset, bytes });
                }
            }
        }
    }
    if let Some((offset, bytes)) = frame {
        msgs.push(Msg::Output { id, offset, bytes });
    }
    drained
}

/// `Exited` goes now: this pump read everything there was, or the cap since the exit passed and
/// `since_exit` bytes read after it prove the shell's own output is all out. Time alone is not
/// enough: an app slower than the cap would see the end before the shell's last bytes.
fn exit_due(drained: bool, seen: Instant, now: Instant, since_exit: u64) -> bool {
    drained || (now.saturating_duration_since(seen) >= EXIT_DRAIN_CAP && since_exit > EXIT_DRAIN_BYTES)
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
    /// The attached Termurion asked for password prompt readings with this debounce (seconds).
    prompt_debounce: Option<f64>,
    /// Client arrivals and departures for the main loop's `host_watch::Watch`, oldest first.
    events: Vec<(Instant, crate::host_watch::Event)>,
}

impl Host {
    fn send(&self, m: &Msg) {
        if let Some(c) = &self.client {
            let _ = c.out.send(encode(m));
        }
    }

    /// Sessions still running (an ended one waits only to be forgotten).
    fn running(&self) -> usize {
        self.sessions.values().filter(|s| s.exited.is_none()).count()
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

    /// The sessions whose consoles the prompt watch reads: live ones a Termurion shows. Never
    /// one paused for Undo Close (or otherwise not attached): nothing drains its output, so
    /// once its pipe fills OpenConsole blocks every console call on it, and an attach there
    /// would hold `pty::CONSOLE` for good (and every later close with it). Each comes with its
    /// pause flag, checked again under that lock. Never one held at the window or with a full
    /// output budget either: it is mid-flood, not at a prompt, and an attach to it lets its
    /// reader read past the budget while nothing takes the output.
    fn prompt_targets(&self) -> Vec<(u64, u32, Arc<std::sync::atomic::AtomicBool>)> {
        let flow = self.client.as_ref().is_some_and(|c| c.version >= FLOW_VERSION);
        let mut out: Vec<_> = self
            .sessions
            .iter()
            .filter(|(_, s)| s.attached && s.exited.is_none() && !held(s, flow))
            .filter_map(|(id, s)| s.pty.as_ref().filter(|p| !p.is_paused() && p.queued() < crate::pty_budget::PTY_BUDGET).map(|p| (*id, s.pid, p.pause_flag())))
            .collect();
        out.sort_by_key(|t| t.0);
        out
    }

    /// The sessions the prompt watch skips for a flood (held at the window, or a full output
    /// budget), not a pause: each reads as not at a prompt, so password input never stays on
    /// through a flood.
    fn flooding_targets(&self) -> Vec<u64> {
        let flow = self.client.as_ref().is_some_and(|c| c.version >= FLOW_VERSION);
        let mut out: Vec<u64> = self
            .sessions
            .iter()
            .filter(|(_, s)| s.attached && s.exited.is_none())
            .filter(|(_, s)| s.pty.as_ref().is_some_and(|p| !p.is_paused() && (held(s, flow) || p.queued() >= crate::pty_budget::PTY_BUDGET)))
            .map(|(id, _)| *id)
            .collect();
        out.sort();
        out
    }

    /// Password prompt readings start over (a new request, or no Termurion to tell).
    /// Returns a "not at a prompt" for each session that was at one, so a Termurion that keeps
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

/// How often the host reads its sessions' console input modes while Termurion asks (the job
/// watcher's rate in front).
const PROMPT_EVERY: Duration = Duration::from_millis(500);

/// Reads each live session's console input mode while the attached Termurion asks, and tells it
/// when one reaches or leaves a password prompt. Only the host attaches to its own consoles:
/// it owns every close of them, and `Pty::drop` holds `pty::CONSOLE`, so it is never attached
/// while one closes (which would end the host and every shell with it).
fn watch_prompts(host: std::sync::Weak<Mutex<Host>>) {
    loop {
        std::thread::sleep(PROMPT_EVERY);
        let Some(shared) = host.upgrade() else { return };
        let (debounce, targets, flooding) = {
            let h = shared.lock().unwrap();
            let (targets, flooding) = match h.prompt_debounce {
                Some(_) => (h.prompt_targets(), h.flooding_targets()),
                None => Default::default(),
            };
            (h.prompt_debounce, targets, flooding)
        };
        drop(shared);
        let Some(debounce) = debounce else { continue };
        // Unknown (the mode could not be read, or the session was paused since) is never a
        // prompt.
        let mut samples: Vec<(u64, bool, Instant)> = targets.into_iter().map(|(id, pid, paused)| (id, crate::platform::pty::console_mode_unless(pid, &paused).is_ok_and(crate::pwm_prompt::at_prompt), Instant::now())).collect();
        samples.extend(flooding.into_iter().map(|id| (id, false, Instant::now())));
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
pub fn run(pipe: &str, api: PtyApi, opts: Options) -> Result<(), String> {
    let mut listener = Listener::bind(pipe)?;
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let host: Shared = Arc::new(Mutex::new(Host { sessions: HashMap::new(), next_id: 1, client: None, instance: format!("{}-{nanos}", std::process::id()), prompt_debounce: None, events: Vec::new() }));
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
    let mut watch = crate::host_watch::Watch::default();
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
        if let Some(exe) = &opts.reopen {
            run_watch(&host, &mut watch, exe);
        }
        let (idle, ended_only) = {
            let h = host.lock().unwrap();
            let unattended = h.client.is_none() && h.running() == 0;
            (unattended && h.sessions.is_empty(), unattended && !h.sessions.is_empty())
        };
        // Ended sessions wait for Termurion to show how they ended, but not forever: with no
        // app attached for ENDED_WAIT the host leaves (it kept the installer's lock, FIXES 2).
        let wait = if ended_only { Some(ENDED_WAIT.max(opts.grace)) } else if idle { Some(opts.grace) } else { None };
        match wait {
            None => idle_since = None,
            Some(w) => {
                if idle_since.get_or_insert_with(Instant::now).elapsed() >= w {
                    return Ok(());
                }
            }
        }
    }
}

/// Feeds the client events and the clock to the watch and carries out what it decides.
/// How long ended sessions wait for an app to attach and show them, with nothing running.
const ENDED_WAIT: Duration = Duration::from_secs(3600);

fn run_watch(host: &Shared, watch: &mut crate::host_watch::Watch, exe: &std::path::Path) {
    use crate::host_watch::{Action, Event};
    let (events, live) = {
        let mut h = host.lock().unwrap();
        (std::mem::take(&mut h.events), h.running())
    };
    let mut actions = Vec::new();
    for (at, ev) in events {
        actions.extend(watch.on(at, ev));
    }
    actions.extend(watch.on(Instant::now(), Event::Tick { live }));
    for a in actions {
        match a {
            Action::Log(line) => log(&line),
            Action::Reopen => {
                if let Err(e) = reopen_app(exe) {
                    log(&format!("host: could not reopen termurion.exe: {e}"));
                }
            }
            Action::EndSessions => {
                let all: Vec<Session> = host.lock().unwrap().sessions.drain().map(|(_, s)| s).collect();
                for p in all.into_iter().filter_map(|s| s.pty) {
                    drop_later(p);
                }
            }
        }
    }
}

/// Starts Termurion again as this user at normal priority with default window flags (CreateProcessW
/// through std), in the user's profile folder, so its reattach brings the sessions back.
fn reopen_app(exe: &std::path::Path) -> Result<(), String> {
    let mut cmd = std::process::Command::new(exe);
    cmd.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    if let Some(home) = crate::platform::paths::home_dir() {
        cmd.current_dir(home);
    }
    // Under the spawn gate, so a descriptor made on another thread is never inherited (macOS).
    crate::platform::pty::spawn_gated(|| cmd.spawn()).map(drop).map_err(|e| format!("{}: {e}", exe.display()))
}

/// Moves a session's new output into its ring and to the attached Termurion.
fn pump(host: &Shared, id: u64) {
    let mut h = host.lock().unwrap();
    let flow = h.client.as_ref().is_some_and(|c| c.version >= FLOW_VERSION);
    let Some(s) = h.sessions.get_mut(&id) else { return };
    let mut msgs = Vec::new();
    if let Some(p) = &s.pty {
        let drained = drain(p, &mut s.ring, s.acked, s.attached, flow, id, &mut msgs);
        let pending = p.pending();
        if s.attached && pending != s.pending {
            msgs.push(Msg::Pending { id, bytes: pending as u64 });
        }
        s.pending = pending;
        // Exited follows the output, unless output still comes long after the shell ended.
        if s.exited.is_none()
            && let Some(code) = p.exit_code() {
                let now = Instant::now();
                let (seen, end) = *s.exit_seen.get_or_insert((now, s.ring.end()));
                if exit_due(drained, seen, now, s.ring.end() - end) {
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
fn serve(host: Shared, pipe: Arc<Pipe>, conn: u64, api: PtyApi, wake: mpsc::Sender<u64>) {
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
        // The client said goodbye (or ended every session): its pipe closing is not a vanish.
        let mut clean = false;
        'read: loop {
            let n = match pipe.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            d.push(&buf[..n]);
            while let Some(m) = d.next() {
                let Ok(m) = m else { break 'read };
                if welcomed {
                    clean |= matches!(m, Msg::Goodbye | Msg::KillAll);
                    handle(&host, m, &api, &wake);
                    continue;
                }
                // The first message must be Hello; only one Termurion at a time.
                let Msg::Hello { version } = m else { break 'read };
                let mut h = host.lock().unwrap();
                let refuse = if h.client.is_some() {
                    Some((true, "another Termurion is attached to the session host".to_string()))
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
                // A new Termurion starts with nothing at a prompt: nothing to tell it.
                h.watch_prompts(None);
                let welcome = Msg::Welcome { version, host_pid: std::process::id(), instance: h.instance.clone(), sessions: h.live() };
                h.send(&welcome);
                h.events.push((Instant::now(), crate::host_watch::Event::Connected));
                welcomed = true;
            }
        }
        let mut h = host.lock().unwrap();
        let mut unheld = Vec::new();
        if h.client.as_ref().is_some_and(|c| c.conn == conn) {
            h.client = None;
            let live = h.running();
            h.events.push((Instant::now(), crate::host_watch::Event::ClientGone { clean, live }));
            // No Termurion to tell.
            h.watch_prompts(None);
            for s in h.sessions.values_mut() {
                s.attached = false;
                // A pane paused for Undo Close must not stay parked with no Termurion to resume
                // it: the host keeps reading into the ring.
                if let Some(p) = &s.pty {
                    p.set_paused(false);
                }
            }
            // A session held at the window has a reader waiting for room: pump it into the ring.
            unheld = h.sessions.keys().copied().collect();
        }
        drop(h);
        for id in unheld {
            let _ = wake.send(id);
        }
    });
}

/// Starts a session on transport `t` and answers `Launched` or `LaunchFailed`.
#[allow(clippy::too_many_arguments)]
fn launch(host: &Shared, api: &PtyApi, wake: &mpsc::Sender<u64>, t: Transport, tag: u64, cmdline: String, cwd: String, env: Vec<String>, cols: u16, rows: u16) {
    let id = {
        let mut h = host.lock().unwrap();
        h.next_id += 1;
        h.next_id - 1
    };
    let w = wake.clone();
    let notify: crate::platform::pty::Notify = Arc::new(move || {
        let _ = w.send(id);
    });
    let dir = (!cwd.is_empty()).then(|| std::path::PathBuf::from(&cwd));
    match crate::platform::pty::spawn_transport(api, t, &cmdline, dir.as_deref(), &env, cols.max(1), rows.max(1), notify) {
        Ok(pty) => {
            // One wake per drain, not per read: `pump` rearms when it finds the output empty.
            pty.set_edge_notify(true);
            let pid = pty.pid();
            let mut h = host.lock().unwrap();
            h.sessions.insert(id, Session { pty: Some(pty), ring: Ring::new(RING), pid, cmdline, cwd, cols, rows, exited: None, attached: true, pending: 0, prompt: Default::default(), acked: 0, exit_seen: None });
            h.send(&Msg::Launched { tag, id, pid });
            drop(h);
            // The start size, so a resize line after it shows whether the shell started at its real grid.
            log(&format!("pty: start {cols}x{rows} (session {id})"));
            // Output that came before the session was in the table.
            pump(host, id);
        }
        Err(error) => host.lock().unwrap().send(&Msg::LaunchFailed { tag, error }),
    }
}

fn handle(host: &Shared, m: Msg, api: &PtyApi, wake: &mpsc::Sender<u64>) {
    match m {
        Msg::Launch { tag, cmdline, cwd, env, cols, rows } => launch(host, api, wake, Transport::Conpty, tag, cmdline, cwd, env, cols, rows),
        // Only a version 4 Termurion sends it (`host_client::launch_transport`).
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
            // The replay counts against the window: live output waits for its acks.
            s.acked = from;
            let exited = s.exited;
            h.send(&Msg::Attached { id, from, dropped });
            if !bytes.is_empty() {
                h.send(&Msg::Output { id, offset: from, bytes });
            }
            match exited {
                Some(code) => h.send(&Msg::Exited { id, code }),
                // A held session's reader waits for room and will not notify, and this attach
                // may have lowered the gap below the window: pump it.
                None => {
                    drop(h);
                    let _ = wake.send(id);
                }
            }
        }
        Msg::Write { id, bytes } => with_pty(host, id, |p| p.write(&bytes)),
        // Version 6: an attach the app gave up on. Its reader may wait for room: pump it.
        Msg::Detach { id } => {
            let mut h = host.lock().unwrap();
            if let Some(s) = h.sessions.get_mut(&id) {
                s.attached = false;
                drop(h);
                let _ = wake.send(id);
            }
        }
        // Version 6. The held session's reader waits for room and will not notify: pump it.
        Msg::Consumed { id, offset } => {
            let mut h = host.lock().unwrap();
            if let Some(s) = h.sessions.get_mut(&id) {
                s.acked = s.acked.max(offset.min(s.ring.end()));
                drop(h);
                let _ = wake.send(id);
            }
        }
        Msg::Resize { id, cols, rows } => {
            let mut h = host.lock().unwrap();
            let mut line = None;
            if let Some(s) = h.sessions.get_mut(&id) {
                line = resize_log_line((s.cols, s.rows), (cols, rows), id);
                (s.cols, s.rows) = (cols, rows);
            }
            drop(h);
            with_pty(host, id, |p| p.resize(cols, rows));
            // One line per size that reaches ConPTY, so a report of a prompt in the wrong row
            // can be traced to the resizes around it.
            if let Some(line) = line {
                log(&line);
            }
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
                    if let Ok(Err(e)) = crate::platform::proc::console_event(pid, ctrl_break, paused).join() {
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
        // Only a version 2 Termurion asks (a version 1 one never sends it).
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
    if let Some(p) = h.sessions.get(&id).and_then(|s| s.pty.as_ref())
        && let Err(e) = f(p) {
            log(&format!("session {id}: {e}"));
        }
}

/// The `host.log` line for a session's size going from `old` to `new`: None when it did not change.
fn resize_log_line(old: (u16, u16), new: (u16, u16), id: u64) -> Option<String> {
    (old != new).then(|| format!("pty: resize {}x{} (session {id})", new.0, new.1))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn a_size_change_is_logged_once() {
        assert_eq!(resize_log_line((100, 30), (120, 40), 3).as_deref(), Some("pty: resize 120x40 (session 3)"));
        assert_eq!(resize_log_line((120, 40), (120, 40), 3), None, "the same size again is not a line");
        assert_eq!(resize_log_line((120, 40), (120, 41), 3).as_deref(), Some("pty: resize 120x41 (session 3)"));
    }
    use crate::host_proto::{Decoder, Live, Msg, encode};
    use crate::platform::ipc::Pipe;
    use std::sync::Arc;
    use std::sync::mpsc::{self, Receiver};
    use std::time::Instant;

    pub(crate) fn api() -> PtyApi {
        crate::platform::pty::test_api()
    }

    /// A host on its own pipe, in a thread.
    pub(crate) fn start_host(grace: Duration) -> (String, std::thread::JoinHandle<Result<(), String>>) {
        let name = crate::platform::ipc::test_name("host");
        let n2 = name.clone();
        let t = std::thread::spawn(move || run(&n2, api(), Options { grace, reopen: None }));
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
                if let Some(p) = crate::platform::ipc::connect(name, Duration::from_secs(1)).unwrap() {
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
            self.hello_as(crate::host_proto::VERSION)
        }

        pub(crate) fn hello_as(&self, version: u32) -> Vec<Live> {
            self.send(&Msg::Hello { version });
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
                if let Msg::Output { id: i, bytes, .. } = m
                    && *i == id {
                        all.push_str(&String::from_utf8_lossy(bytes));
                    }
                all.contains(needle).then_some(())
            });
            all
        }

        pub(crate) fn close(self) {
            self.pipe.shutdown();
        }

        /// Output of session `id` until none comes for `quiet` (or `most` passes): the bytes
        /// and the offset after the last one (0 when none came).
        fn output_until_quiet(&self, id: u64, quiet: Duration, most: Duration) -> (u64, u64) {
            let (mut total, mut end) = (0u64, 0u64);
            let stop = Instant::now() + most;
            while let Ok(m) = self.rx.recv_timeout(quiet.min(stop.saturating_duration_since(Instant::now()))) {
                if let Msg::Output { id: i, offset, bytes } = m
                    && i == id {
                        total += bytes.len() as u64;
                        end = offset + bytes.len() as u64;
                    }
                if Instant::now() >= stop {
                    break;
                }
            }
            (total, end)
        }
    }

    /// A program that writes output as fast as it can, for good.
    fn flood() -> &'static str {
        crate::platform::by_os(r#"cmd.exe /d /c "for /l %i in (0,0,1) do @echo xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx""#, "/bin/sh -c yes")
    }

    fn bare_session(attached: bool) -> Session {
        Session { pty: None, ring: Ring::new(RING), pid: 1, cmdline: String::new(), cwd: String::new(), cols: 80, rows: 24, exited: None, attached, pending: 0, prompt: Default::default(), acked: 0, exit_seen: None }
    }

    #[test]
    fn the_window_holds_only_a_flow_client() {
        use crate::host_proto::WINDOW;
        let mut s = bare_session(true);
        s.ring.push(&vec![b'x'; WINDOW as usize]);
        assert!(held(&s, true), "a full window unacked holds");
        s.acked = 1;
        assert!(!held(&s, true), "one byte short of the window does not");
        s.acked = 0;
        assert!(!held(&s, false), "a client below version 6 is never held");
        s.attached = false;
        assert!(!held(&s, true), "a detached session is never held");
    }

    /// Chunks standing in for a pty's queue, counting rearms.
    #[derive(Default)]
    struct Chunks {
        queue: std::cell::RefCell<std::collections::VecDeque<Vec<u8>>>,
        rearms: std::cell::Cell<u32>,
    }

    impl Chunks {
        /// `n` chunks of `len` bytes, each byte its chunk's number.
        fn of(n: usize, len: usize) -> Chunks {
            let c = Chunks::default();
            c.queue.borrow_mut().extend((0..n).map(|i| vec![i as u8; len]));
            c
        }

        fn bytes(&self) -> Vec<u8> {
            self.queue.borrow().iter().flatten().copied().collect()
        }
    }

    impl OutputSource for Chunks {
        fn try_read(&self) -> Read {
            self.queue.borrow_mut().pop_front().map_or(Read::Empty, Read::Data)
        }

        fn rearm_notify(&self) {
            self.rearms.set(self.rearms.get() + 1);
        }
    }

    fn frames(msgs: &[Msg]) -> Vec<(u64, &[u8])> {
        msgs.iter()
            .map(|m| match m {
                Msg::Output { id: 7, offset, bytes } => (*offset, bytes.as_slice()),
                m => panic!("not an Output of 7: {m:?}"),
            })
            .collect()
    }

    #[test]
    fn waiting_chunks_go_in_one_frame() {
        let mut ring = Ring::new(RING);
        let (ten, mut msgs) = (Chunks::of(10, 4096), Vec::new());
        assert!(drain(&ten, &mut ring, 0, true, true, 7, &mut msgs), "drained");
        assert_eq!(frames(&msgs).iter().map(|(o, b)| (*o, b.len())).collect::<Vec<_>>(), vec![(0, 40960)]);
        assert_eq!(ten.rearms.get(), 1, "an empty read rearms");
        let hundred = Chunks::of(100, 4096);
        let want = hundred.bytes();
        msgs.clear();
        assert!(drain(&hundred, &mut ring, 0, true, false, 7, &mut msgs));
        let got = frames(&msgs);
        assert!(got.iter().all(|(_, b)| b.len() <= MERGE_CAP), "a frame over the cap");
        assert_eq!(got.len(), 100 * 4096 / MERGE_CAP + 1, "full frames but the last");
        let mut at = 40960;
        for (offset, bytes) in &got {
            assert_eq!(*offset, at, "offsets are contiguous");
            at += bytes.len() as u64;
        }
        assert_eq!(got.iter().flat_map(|(_, b)| b.iter().copied()).collect::<Vec<_>>(), want);
        assert_eq!(ring.end(), at);
    }

    #[test]
    fn a_held_drain_stops_at_the_window_and_never_rearms() {
        use crate::host_proto::WINDOW;
        let (src, mut ring, mut msgs) = (Chunks::of(600, 4096), Ring::new(RING), Vec::new());
        assert!(!drain(&src, &mut ring, 0, true, true, 7, &mut msgs), "held, not drained");
        assert_eq!(ring.end(), WINDOW);
        assert_eq!(src.rearms.get(), 0);
        // Nobody shows it: it reads into the ring and sends nothing.
        msgs.clear();
        assert!(drain(&src, &mut ring, 0, false, true, 7, &mut msgs));
        assert!(msgs.is_empty());
        assert_eq!(ring.end(), 600 * 4096);
    }

    #[test]
    fn exit_waits_for_the_drain_or_the_cap() {
        let seen = Instant::now();
        let past = EXIT_DRAIN_BYTES + 1;
        assert!(exit_due(true, seen, seen, 0), "drained: at once");
        assert!(!exit_due(false, seen, seen + Duration::from_millis(199), past));
        assert!(exit_due(false, seen, seen + EXIT_DRAIN_CAP, past));
        // A slow app: the shell's last bytes may still be in the pty.
        assert!(!exit_due(false, seen, seen + Duration::from_secs(60), EXIT_DRAIN_BYTES), "the cap alone never ends a held session");
    }

    #[test]
    fn a_flow_client_holds_the_host_at_the_window() {
        use crate::host_proto::{FLOW_VERSION, WINDOW};
        let (name, _host) = start_host(Duration::from_secs(1));
        let c = Conn::open(&name);
        c.hello_as(FLOW_VERSION);
        let id = c.launch(flood());
        let (total, end) = c.output_until_quiet(id, Duration::from_secs(1), Duration::from_secs(1));
        assert!(total > 0 && total <= WINDOW + 64 * 1024, "the host sent past the window: {total}");
        c.send(&Msg::Consumed { id, offset: end });
        let (more, _) = c.output_until_quiet(id, Duration::from_secs(1), Duration::from_secs(1));
        assert!(more > 0, "an ack lets more through");
        c.send(&Msg::KillAll);
    }

    #[test]
    fn a_version_five_client_is_never_held() {
        use crate::host_proto::WINDOW;
        let (name, _host) = start_host(Duration::from_secs(1));
        let c = Conn::open(&name);
        c.hello_as(5);
        let id = c.launch(flood());
        // A held client never gets past WINDOW plus a read, however long it waits; the 10 s
        // covers a loaded machine and ConPTY's slower flood.
        let (mut total, stop) = (0u64, Instant::now() + Duration::from_secs(10));
        while total <= 2 * WINDOW && Instant::now() < stop {
            total += c.output_until_quiet(id, Duration::from_secs(1), Duration::from_millis(200)).0;
        }
        assert!(total > 2 * WINDOW, "a version 5 client was held: {total}");
        c.send(&Msg::KillAll);
    }

    #[test]
    fn exited_follows_all_output_under_the_window() {
        use crate::host_proto::{ACK_EVERY, FLOW_VERSION};
        let (name, _host) = start_host(Duration::from_secs(1));
        let c = Conn::open(&name);
        c.hello_as(FLOW_VERSION);
        // Pipes on Windows: ConPTY would rewrite the bytes.
        // 100,000 lines of 58 x's and a line end, 60 bytes each.
        let line = "x".repeat(58);
        let windows = format!("cmd.exe /d /c \"(for /l %i in (1,1,100000) do @echo {line}) & exit 3\"");
        let cmdline = crate::platform::by_os(&windows, "/bin/sh -c 'head -c 6000000 /dev/zero | tr \"\\000\" x; exit 3'").to_string();
        let transport = if cfg!(windows) { 2 } else { 0 };
        c.send(&Msg::LaunchWith { tag: 1, cmdline, cwd: String::new(), env: vec![], cols: 80, rows: 24, transport });
        let id = c.wait(|m| match m {
            Msg::Launched { id, .. } => Some(*id),
            Msg::LaunchFailed { error, .. } => panic!("{error}"),
            _ => None,
        });
        // cmd writes slower than a client that acks every ACK_EVERY takes, so on Windows the
        // client acks only once the window is full (the host is held then) and waits first.
        let (step, pause) = if cfg!(windows) { (crate::host_proto::WINDOW, 100) } else { (ACK_EVERY, 20) };
        let (mut end, mut acked, mut holds) = (0u64, 0u64, 0);
        let stop = Instant::now() + Duration::from_secs(60);
        loop {
            let m = c.rx.recv_timeout(stop.saturating_duration_since(Instant::now())).expect("no Exited in 60 s");
            match m {
                Msg::Output { id: i, offset, bytes } if i == id => end = offset + bytes.len() as u64,
                Msg::Exited { id: i, code } if i == id => {
                    assert_eq!(end, 6_000_000, "Exited came before all the output");
                    assert_eq!(code, 3);
                    break;
                }
                _ => {}
            }
            if end - acked >= step {
                holds += (end - acked >= crate::host_proto::WINDOW) as u32;
                std::thread::sleep(Duration::from_millis(pause));
                c.send(&Msg::Consumed { id, offset: end });
                acked = end;
            }
        }
        if cfg!(windows) {
            assert!(holds >= 2, "the host was held only {holds} times");
        }
        c.send(&Msg::KillAll);
    }

    #[cfg(unix)] // test: a background writer on the session's pty
    #[test]
    fn a_writer_that_outlives_the_shell_still_ends() {
        use crate::host_proto::{ACK_EVERY, FLOW_VERSION};
        let (name, _host) = start_host(Duration::from_secs(1));
        let c = Conn::open(&name);
        c.hello_as(FLOW_VERSION);
        let id = c.launch("/bin/sh -c 'yes & exit 0'");
        let (mut end, mut acked) = (0u64, 0u64);
        let stop = Instant::now() + Duration::from_secs(2);
        loop {
            let m = c.rx.recv_timeout(stop.saturating_duration_since(Instant::now())).expect("no Exited within 2 s");
            match m {
                Msg::Output { id: i, offset, bytes } if i == id => end = offset + bytes.len() as u64,
                Msg::Exited { id: i, code } if i == id => {
                    assert_eq!(code, 0);
                    break;
                }
                _ => {}
            }
            if end - acked >= ACK_EVERY {
                c.send(&Msg::Consumed { id, offset: end });
                acked = end;
            }
        }
        c.send(&Msg::KillAll);
    }

    #[test]
    fn a_detached_session_reads_into_the_ring() {
        use crate::host_proto::{FLOW_VERSION, WINDOW};
        let (name, _host) = start_host(Duration::from_secs(1));
        let c = Conn::open(&name);
        c.hello_as(FLOW_VERSION);
        let id = c.launch(flood());
        c.output_until_quiet(id, Duration::from_millis(500), Duration::from_secs(2));
        c.close();
        std::thread::sleep(Duration::from_secs(1));
        let c = Conn::open(&name);
        let live = c.hello_as(FLOW_VERSION);
        let s = live.iter().find(|l| l.id == id).expect("the session is still there");
        assert!(s.end > WINDOW + crate::pty_budget::PTY_BUDGET as u64, "a session nobody shows stopped reading: {}", s.end);
        c.send(&Msg::KillAll);
    }

    #[test]
    fn a_detached_attach_reads_into_the_ring() {
        use crate::host_proto::FLOW_VERSION;
        let (name, _host) = start_host(Duration::from_secs(1));
        let c = Conn::open(&name);
        c.hello_as(FLOW_VERSION);
        let id = c.launch(flood());
        let (_, at) = c.output_until_quiet(id, Duration::from_millis(500), Duration::from_secs(5));
        assert!(at > 0);
        c.send(&Msg::Detach { id });
        std::thread::sleep(Duration::from_secs(2));
        c.send(&Msg::Attach { id, since: at });
        let (from, bytes) = c.wait(|m| match m {
            Msg::Output { id: i, offset, bytes } if *i == id => Some((*offset, bytes.len() as u64)),
            _ => None,
        });
        // Past what its reader had queued when it was held: it read again.
        assert!(from + bytes > at + crate::pty_budget::PTY_BUDGET as u64 + 64 * 1024, "a detached session stopped reading: {} after {at}", from + bytes);
        c.send(&Msg::KillAll);
    }

    /// A session held at the window and attached again near its end (the gap now below the
    /// window) reads on, with no ack: its reader will not notify, so the attach must pump it.
    #[test]
    fn a_reattach_below_the_window_unholds() {
        use crate::host_proto::FLOW_VERSION;
        let (name, _host) = start_host(Duration::from_secs(1));
        let c = Conn::open(&name);
        c.hello_as(FLOW_VERSION);
        let id = c.launch(flood());
        let (_, at) = c.output_until_quiet(id, Duration::from_millis(500), Duration::from_secs(5));
        assert!(at > 0);
        c.send(&Msg::Attach { id, since: at });
        let (more, _) = c.output_until_quiet(id, Duration::from_secs(1), Duration::from_secs(3));
        assert!(more > 0, "the attach did not unhold the session");
        c.send(&Msg::KillAll);
    }

    #[test]
    fn an_ack_past_the_end_is_clamped() {
        use crate::host_proto::{FLOW_VERSION, WINDOW};
        let (name, _host) = start_host(Duration::from_secs(1));
        let c = Conn::open(&name);
        c.hello_as(FLOW_VERSION);
        let id = c.launch(flood());
        let (_, at) = c.output_until_quiet(id, Duration::from_millis(500), Duration::from_secs(5));
        assert!(at > 0);
        c.send(&Msg::Consumed { id, offset: u64::MAX });
        let (more, end) = c.output_until_quiet(id, Duration::from_millis(500), Duration::from_secs(3));
        assert!(more > 0, "the ack let more through");
        assert!(end <= at + WINDOW + 64 * 1024, "the window no longer holds: {end} after {at}");
        c.send(&Msg::KillAll);
    }

    #[test]
    fn host_launches_and_echoes() {
        let (name, _host) = start_host(Duration::from_secs(1));
        let c = Conn::open(&name);
        assert!(c.hello().is_empty());
        let id = c.launch(crate::platform::by_os("cmd.exe /d /c echo hello-host", "/bin/sh -c 'echo hello-host'"));
        // The exit can be told before the last output is read (a pseudo console flushes late), so
        // the two are awaited together: waiting for the text first could swallow the Exited.
        let (mut all, mut exited) = (String::new(), false);
        c.wait(|m| {
            match m {
                Msg::Output { id: i, bytes, .. } if *i == id => all.push_str(&String::from_utf8_lossy(bytes)),
                Msg::Exited { id: i, .. } if *i == id => exited = true,
                _ => {}
            }
            (exited && all.contains("hello-host")).then_some(())
        });
    }

    /// Version 3: Launch's `env` reaches the session's shell.
    #[test]
    fn launch_env_reaches_the_session() {
        let (name, _host) = start_host(Duration::from_secs(1));
        let c = Conn::open(&name);
        c.hello();
        let id = c.launch_with(crate::platform::by_os("cmd.exe /d /c echo [%ITERM_PROFILE%^]", "/bin/sh -c 'echo [$ITERM_PROFILE]'"), vec!["ITERM_PROFILE=Hosted".into()]);
        c.output_until(id, "[Hosted]");
    }

    /// Version 4: LaunchWith runs the session on the transport it names.
    #[test]
    fn launch_with_runs_the_transport() {
        let (name, _host) = start_host(Duration::from_secs(1));
        let c = Conn::open(&name);
        c.hello();
        let launch = |tag: u64, transport: u8| {
            c.send(&Msg::LaunchWith { tag, cmdline: crate::platform::by_os("cmd.exe /d /c echo hosted-pipes", "/bin/sh -c 'echo hosted-pipes'").into(), cwd: String::new(), env: vec![], cols: 80, rows: 24, transport });
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
        let id = c.launch(crate::platform::by_os("cmd.exe /d /c ping -n 2 127.0.0.1 >nul & echo later-text & ping -n 30 127.0.0.1 >nul", "/bin/sh -c 'sleep 1; echo later-text; sleep 30'"));
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
        let id = c.launch(crate::platform::by_os("cmd.exe /d /c echo before-pause & ping -n 3 127.0.0.1 >nul & echo after-pause & ping -n 30 127.0.0.1 >nul", "/bin/sh -c 'echo before-pause; sleep 3; echo after-pause; sleep 30'"));
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
            Session { pty: None, ring: Ring::new(16), pid: 1, cmdline: String::new(), cwd: String::new(), cols: 80, rows: 24, exited: None, attached: true, pending: 0, prompt, acked: 0, exit_seen: None }
        };
        let mut h = Host { sessions: HashMap::new(), next_id: 3, client: None, instance: "t".into(), prompt_debounce: Some(0.2), events: Vec::new() };
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
            let pty = crate::platform::pty::spawn(&api, crate::platform::by_os("cmd.exe /d /k", "env PS1=> ENV= /bin/sh -i"), 80, 24).unwrap();
            let pid = pty.pid();
            Session { pty: Some(pty), ring: Ring::new(16), pid, cmdline: String::new(), cwd: String::new(), cols: 80, rows: 24, exited: None, attached, pending: 0, prompt: Default::default(), acked: 0, exit_seen: None }
        };
        let mut h = Host { sessions: HashMap::new(), next_id: 4, client: None, instance: "t".into(), prompt_debounce: Some(0.2), events: Vec::new() };
        h.sessions.insert(1, session(true));
        h.sessions.insert(2, session(true));
        h.sessions.insert(3, session(false));
        let ids = |h: &Host| h.prompt_targets().into_iter().map(|t| t.0).collect::<Vec<_>>();
        assert_eq!(ids(&h), vec![1, 2], "a session no Termurion shows is not read");
        h.sessions[&2].pty.as_ref().unwrap().pause_after_reads();
        assert_eq!(ids(&h), vec![1], "a session paused for Undo Close is not read");
        // One paused after it was picked is refused without an attach.
        let targets = h.prompt_targets();
        h.sessions[&1].pty.as_ref().unwrap().pause_after_reads();
        let (_, pid, paused) = &targets[0];
        assert_eq!(crate::platform::pty::console_mode_unless(*pid, paused), Err("the console is paused".to_string()));
        h.sessions[&2].pty.as_ref().unwrap().set_paused(false);
        assert_eq!(ids(&h), vec![2], "an undone close is read again");
        for s in h.sessions.drain().map(|(_, s)| s) {
            drop(s.pty);
        }
    }

    #[test]
    fn the_prompt_watch_skips_held_and_flooding_sessions() {
        let api = api();
        let session = |cmd: &str| {
            let pty = crate::platform::pty::spawn(&api, cmd, 80, 24).unwrap();
            let pid = pty.pid();
            Session { pty: Some(pty), ring: Ring::new(RING), pid, cmdline: String::new(), cwd: String::new(), cols: 80, rows: 24, exited: None, attached: true, pending: 0, prompt: Default::default(), acked: 0, exit_seen: None }
        };
        let idle = crate::platform::by_os("cmd.exe /d /k", "env PS1=> ENV= /bin/sh -i");
        let client = |version| Some(Client { conn: 1, out: mpsc::channel().0, version });
        let mut h = Host { sessions: HashMap::new(), next_id: 4, client: client(crate::host_proto::FLOW_VERSION), instance: "t".into(), prompt_debounce: Some(0.2), events: Vec::new() };
        h.sessions.insert(1, session(idle));
        let mut held = session(idle);
        held.ring.push(&vec![b'x'; crate::host_proto::WINDOW as usize]);
        h.sessions.insert(2, held);
        h.sessions.insert(3, session(flood()));
        let end = Instant::now() + Duration::from_secs(10);
        while h.sessions[&3].pty.as_ref().unwrap().queued() < crate::pty_budget::PTY_BUDGET {
            assert!(Instant::now() < end, "the flood never filled its budget");
            std::thread::sleep(Duration::from_millis(10));
        }
        let ids = |h: &Host| h.prompt_targets().into_iter().map(|t| t.0).collect::<Vec<_>>();
        assert_eq!(ids(&h), vec![1], "a held session and one with a full budget are not read");
        assert_eq!(h.flooding_targets(), vec![2, 3], "both read as not at a prompt");
        h.client = client(5);
        assert_eq!(ids(&h), vec![1, 2], "a version 5 client's session is never held");
        assert_eq!(h.flooding_targets(), vec![3]);
        h.sessions[&3].pty.as_ref().unwrap().set_paused(true);
        assert!(h.flooding_targets().is_empty(), "a paused session keeps its prompt state");
        for s in h.sessions.drain().map(|(_, s)| s) {
            drop(s.pty);
        }
    }

    /// The child half of `a_signal_goes_through_the_host`: detached (no console, as
    /// termurion-host runs), so the host can attach to its session's console.
    // Windows only: it drives a console child with Win32 console calls and PowerShell.
    #[cfg(windows)] // test: a live ConPTY host
    #[test]
    #[ignore]
    fn signal_child() {
        if std::env::var_os("TERMURION_SIGNAL_CHILD").is_none() {
            return;
        }
        // This process ignores Ctrl+C (a console-subsystem test process is not asked by the
        // handler the host installs; termurion-host is a windows-subsystem one). Its sessions
        // inherit that, so the session's program takes Ctrl+C back first.
        unsafe { windows_sys::Win32::System::Console::SetConsoleCtrlHandler(None, 1) };
        let (name, _host) = start_host(Duration::from_secs(1));
        let c = Conn::open(&name);
        c.hello();
        let script = "$k = Add-Type -Name C -Namespace T -PassThru -MemberDefinition '[DllImport(\"kernel32.dll\")] public static extern bool SetConsoleCtrlHandler(IntPtr h, bool add);'; [void]$k::SetConsoleCtrlHandler([IntPtr]::Zero, $false); 'ready'; Start-Sleep 60";
        let id = c.launch(&format!("powershell.exe -NoProfile -EncodedCommand {}", crate::platform::windows::pty::tests::encoded_command(script)));
        c.output_until(id, "ready");
        std::thread::sleep(Duration::from_millis(500));
        c.send(&Msg::Signal { id, ctrl_break: false });
        // Within the 10 s wait, long before the minute is up.
        c.wait(|m| matches!(m, Msg::Exited { id: i, .. } if *i == id).then_some(()));
        c.send(&Msg::KillAll);
    }

    #[cfg(windows)] // test: a signal through a live host
    #[test]
    fn a_signal_goes_through_the_host() {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "host::tests::signal_child", "--ignored", "--test-threads=1"])
            .env("TERMURION_SIGNAL_CHILD", "1")
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
        // An older Termurion: version 1, and nothing it does not know ever comes back.
        let old = Conn::open(&name);
        old.send(&Msg::Hello { version: 1 });
        assert_eq!(old.wait(|m| match m {
            Msg::Welcome { version, .. } => Some(*version),
            _ => None,
        }), 1);
        let id = old.launch(crate::platform::by_os("cmd.exe /d /c echo old-termurion", "/bin/sh -c 'echo old-termurion'"));
        old.output_until(id, "old-termurion");
        old.close();
        std::thread::sleep(Duration::from_millis(300));
        // A version 2 Termurion asks for prompt readings; the stream stays whole.
        let new = Conn::open(&name);
        new.send(&Msg::Hello { version: 2 });
        assert_eq!(new.wait(|m| match m {
            Msg::Welcome { version, .. } => Some(*version),
            _ => None,
        }), 2);
        new.send(&Msg::WatchPrompts { debounce_ms: Some(200) });
        let id = new.launch(crate::platform::by_os("cmd.exe /d /c echo new-termurion", "/bin/sh -c 'echo new-termurion'"));
        new.output_until(id, "new-termurion");
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
        let id = a.launch(crate::platform::by_os("cmd.exe /d /c echo still-mine", "/bin/sh -c 'echo still-mine'"));
        a.output_until(id, "still-mine");
    }

    #[test]
    fn exited_sessions_wait_for_forget() {
        let (name, host) = start_host(Duration::from_millis(500));
        let c = Conn::open(&name);
        c.hello();
        let id = c.launch(crate::platform::by_os("cmd.exe /d /c exit 3", "/bin/sh -c 'exit 3'"));
        let code = c.wait(|m| match m {
            Msg::Exited { id: i, code } if *i == id => Some(*code),
            _ => None,
        });
        assert_eq!(code, 3);
        c.close();
        std::thread::sleep(Duration::from_secs(1));
        let c = Conn::open(&name);
        let live = c.hello();
        assert_eq!(live.iter().find(|l| l.id == id).map(|l| l.exited), Some(Some(3)), "an ended session waits for Termurion");
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

    /// This process's user plus system CPU so far.
    #[cfg(unix)]
    fn cpu() -> Duration {
        let mut u: libc::rusage = unsafe { std::mem::zeroed() };
        unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut u) };
        let t = |v: libc::timeval| Duration::from_secs(v.tv_sec as u64) + Duration::from_micros(v.tv_usec as u64);
        t(u.ru_utime) + t(u.ru_stime)
    }

    /// The relay's cost, for a before and after: 256 MiB of a pty flood through the host to a
    /// client that acks as the app does. Run in release with
    /// `cargo test --release --lib host::tests::relay_bench -- --ignored --nocapture`.
    #[cfg(unix)]
    #[test]
    #[ignore]
    fn relay_bench() {
        use crate::host_proto::{ACK_EVERY, FLOW_VERSION};
        const N: u64 = 256 << 20;
        let (name, _host) = start_host(Duration::from_secs(1));
        let c = Conn::open(&name);
        c.hello_as(FLOW_VERSION);
        let (cpu0, t0) = (cpu(), Instant::now());
        let id = c.launch(&format!("/bin/sh -c 'head -c {N} /dev/zero; sleep 2'"));
        let (mut end, mut acked, mut frames) = (0u64, 0u64, 0u64);
        while end < N {
            match c.rx.recv_timeout(Duration::from_secs(10)).expect("the flood stalled") {
                Msg::Output { id: i, offset, bytes } if i == id => {
                    frames += 1;
                    end = offset + bytes.len() as u64;
                }
                _ => {}
            }
            if end - acked >= ACK_EVERY {
                c.send(&Msg::Consumed { id, offset: end });
                acked = end;
            }
        }
        let (took, used) = (t0.elapsed(), cpu() - cpu0);
        println!("relay: {:.0} MB/s, {frames} frames ({} KiB each), CPU {:.2} s = {:.0}% over {:.2} s", N as f64 / 1e6 / took.as_secs_f64(), N / frames / 1024, used.as_secs_f64(), 100.0 * used.as_secs_f64() / took.as_secs_f64(), took.as_secs_f64());
        c.send(&Msg::KillAll);
    }
}
