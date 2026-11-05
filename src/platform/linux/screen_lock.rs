//! The screen locking on Linux, as logind and the screen saver report it: the `Lock` signal of
//! this login session (system bus) and `ActiveChanged(true)` of org.freedesktop.ScreenSaver or
//! GNOME's org.gnome.ScreenSaver (session bus). Listener threads only set a flag and wake the
//! loop; the app locks the password manager on its next pass.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use zbus::blocking::{Connection, MessageIterator};
use zbus::zvariant::OwnedObjectPath;
use zbus::{MatchRule, message::Type};

use crate::platform::pty::Notify;

const LOGIND: &str = "org.freedesktop.login1";
const LOGIND_PATH: &str = "/org/freedesktop/login1";
const SAVERS: [&str; 2] = ["org.freedesktop.ScreenSaver", "org.gnome.ScreenSaver"];
/// The longest a bus connect or a logind call may take.
const BOUND: Duration = Duration::from_secs(5);
/// A failed setup is tried again no sooner than this.
const RETRY: Duration = Duration::from_secs(30);

static LOCKED: AtomicBool = AtomicBool::new(false);
static WAKE: OnceLock<Notify> = OnceLock::new();
/// 1 while the listeners run (or wait out a failed setup).
static WATCHING: AtomicUsize = AtomicUsize::new(0);
/// Bumped to retire the listeners of an earlier setup.
static EPOCH: AtomicU64 = AtomicU64::new(0);
static STARTER: Mutex<()> = Mutex::new(());
/// Listener threads running now.
static LIVE: AtomicUsize = AtomicUsize::new(0);

/// The screen locked: remember it and wake the loop.
fn on_lock(epoch: u64) {
    if epoch != EPOCH.load(Ordering::Acquire) {
        return;
    }
    LOCKED.store(true, Ordering::Release);
    if let Some(w) = WAKE.get() {
        w();
    }
}

/// Starts the listeners, once. The window handle is unused: the signals are session wide.
pub fn watch(_view: isize, wake: Notify) {
    let _ = WAKE.set(wake);
    let _g = STARTER.lock().unwrap_or_else(|e| e.into_inner());
    if WATCHING.swap(1, Ordering::AcqRel) != 0 {
        return;
    }
    // A new setup retires any listener still waiting from an earlier one.
    let epoch = EPOCH.fetch_add(1, Ordering::AcqRel) + 1;
    let spawned = std::thread::Builder::new().name("screen-lock".into()).spawn(move || run(&Buses::real(), epoch));
    if spawned.is_err() {
        WATCHING.store(0, Ordering::Release);
    }
}

/// How many setups are live: 0 or 1.
pub fn watched() -> usize {
    WATCHING.load(Ordering::Acquire)
}

/// Whether the screen locked since the last call.
pub fn take() -> bool {
    LOCKED.swap(false, Ordering::AcqRel)
}

type Connect = Arc<dyn Fn() -> zbus::Result<Connection> + Send + Sync>;

/// Where the signals come from: the two buses and the login session's id.
struct Buses {
    system: Connect,
    session: Connect,
    session_id: Option<String>,
}

impl Buses {
    fn real() -> Buses {
        let timed = |b: zbus::Result<zbus::blocking::connection::Builder<'static>>| b.and_then(|b| b.method_timeout(BOUND).build());
        Buses {
            system: Arc::new(move || timed(zbus::blocking::connection::Builder::system())),
            session: Arc::new(move || timed(zbus::blocking::connection::Builder::session())),
            session_id: std::env::var("XDG_SESSION_ID").ok(),
        }
    }
}

/// Runs `connect` on its own thread and gives up after `limit`; the thread is left to finish alone.
fn bounded(limit: Duration, connect: Connect) -> Result<Connection, String> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(connect());
    });
    match rx.recv_timeout(limit) {
        Ok(Ok(c)) => Ok(c),
        Ok(Err(e)) => Err(e.to_string()),
        Err(_) => Err("the bus did not answer in time.".to_string()),
    }
}

/// Connects both buses and listens on one thread each. If any listener ends, all of this setup's
/// connections are closed so the rest end too, and the app may set up again. With no bus at all,
/// waits and lets the app ask again.
fn run(buses: &Buses, epoch: u64) {
    let sys = bounded(BOUND, buses.system.clone());
    let ses = bounded(BOUND, buses.session.clone());
    let conns: Vec<Connection> = sys.iter().chain(ses.iter()).cloned().collect();
    let mut listening = 0;
    if let Ok(sys) = &sys
        && let Some(path) = session_path(sys, buses.session_id.as_deref())
    {
        listening += listen(sys, rule(&format!("{LOGIND}.Session"), "Lock", Some(&path), Some(LOGIND)), epoch, false, &conns);
    }
    if let Ok(ses) = &ses {
        for iface in SAVERS {
            listening += listen(ses, rule(iface, "ActiveChanged", None, Some(iface)), epoch, true, &conns);
        }
    }
    if listening == 0 {
        std::thread::sleep(RETRY);
        if epoch == EPOCH.load(Ordering::Acquire) {
            WATCHING.store(0, Ordering::Release);
        }
    }
}

fn rule(iface: &str, member: &str, path: Option<&str>, sender: Option<&str>) -> Option<MatchRule<'static>> {
    let mut b = MatchRule::builder().msg_type(Type::Signal).interface(iface.to_string()).ok()?.member(member.to_string()).ok()?;
    if let Some(p) = path {
        b = b.path(p.to_string()).ok()?;
    }
    if let Some(s) = sender {
        b = b.sender(s.to_string()).ok()?;
    }
    Some(b.build())
}

/// Listens for one signal on its own thread; 1 when the subscription is in place.
fn listen(conn: &Connection, rule: Option<MatchRule<'static>>, epoch: u64, wants_true: bool, conns: &[Connection]) -> usize {
    let Some(rule) = rule else { return 0 };
    let Ok(iter) = MessageIterator::for_match_rule(rule, conn, None) else { return 0 };
    let conns = conns.to_vec();
    LIVE.fetch_add(1, Ordering::AcqRel);
    let started = std::thread::Builder::new().name("screen-lock-signal".into()).spawn(move || {
        for msg in iter {
            if epoch != EPOCH.load(Ordering::Acquire) {
                break;
            }
            let Ok(msg) = msg else { break };
            if wants_true && !matches!(msg.body().deserialize::<bool>(), Ok(true)) {
                continue;
            }
            on_lock(epoch);
        }
        // This stream ended: close the setup's connections so its other listeners end, then let the app set up again.
        for c in conns {
            let _ = c.close();
        }
        if epoch == EPOCH.load(Ordering::Acquire) {
            WATCHING.store(0, Ordering::Release);
        }
        LIVE.fetch_sub(1, Ordering::AcqRel);
    });
    if started.is_err() {
        LIVE.fetch_sub(1, Ordering::AcqRel);
    }
    usize::from(started.is_ok())
}

/// This login session's object path: `XDG_SESSION_ID` first, else the session of this process.
fn session_path(sys: &Connection, id: Option<&str>) -> Option<String> {
    let mut by_id = None;
    if let Some(id) = id.filter(|i| !i.is_empty()) {
        by_id = sys.call_method(Some(LOGIND), LOGIND_PATH, Some("org.freedesktop.login1.Manager"), "GetSession", &(id,)).ok();
    }
    let reply = match by_id {
        Some(r) => r,
        None => sys.call_method(Some(LOGIND), LOGIND_PATH, Some("org.freedesktop.login1.Manager"), "GetSessionByPID", &(std::process::id(),)).ok()?,
    };
    let path: OwnedObjectPath = reply.body().deserialize().ok()?;
    Some(path.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};
    use std::process::{Child, Command, Stdio};
    use std::time::Instant;

    /// One test at a time: the listener counters are process wide. Nothing touches the environment.
    static TURN: Mutex<()> = Mutex::new(());
    const SESSION: &str = "/org/freedesktop/login1/session/_42";
    const SAVER_PATH: &str = "/org/freedesktop/ScreenSaver";

    struct Manager;

    #[zbus::interface(name = "org.freedesktop.login1.Manager")]
    impl Manager {
        fn get_session(&self, _id: String) -> zbus::fdo::Result<OwnedObjectPath> {
            Ok(OwnedObjectPath::try_from(SESSION).unwrap())
        }
        #[zbus(name = "GetSessionByPID")]
        fn get_session_by_pid(&self, _pid: u32) -> zbus::fdo::Result<OwnedObjectPath> {
            Ok(OwnedObjectPath::try_from(SESSION).unwrap())
        }
    }

    /// A private dbus-daemon.
    struct Daemon {
        child: Child,
        addr: String,
    }

    impl Drop for Daemon {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    fn daemon() -> Option<Daemon> {
        let mut child = match Command::new("dbus-daemon").args(["--session", "--nofork", "--print-address=1"]).stdout(Stdio::piped()).spawn() {
            Ok(c) => c,
            Err(e) => {
                println!("skipped: dbus-daemon is not available ({e})");
                return None;
            }
        };
        let mut addr = String::new();
        BufReader::new(child.stdout.take().unwrap()).read_line(&mut addr).unwrap();
        Some(Daemon { child, addr: addr.trim().to_string() })
    }

    fn at(addr: &str) -> zbus::Result<Connection> {
        zbus::blocking::connection::Builder::address(addr)?.build()
    }

    /// Fake logind (system) and a screensaver owner (session) on private daemons.
    struct Rig {
        system: Daemon,
        session: Daemon,
        logind: Connection,
        saver: Connection,
    }

    impl Drop for Rig {
        fn drop(&mut self) {
            reset();
        }
    }

    /// A clean slate: listeners of an earlier rig are retired.
    fn reset() {
        EPOCH.fetch_add(1, Ordering::AcqRel);
        WATCHING.store(0, Ordering::Release);
        LOCKED.store(false, Ordering::Release);
    }

    fn rig() -> Option<Rig> {
        reset();
        let (system, session) = (daemon()?, daemon()?);
        let logind = zbus::blocking::connection::Builder::address(system.addr.as_str()).unwrap().name(LOGIND).unwrap().serve_at(LOGIND_PATH, Manager).unwrap().build().unwrap();
        let saver = at(&session.addr).unwrap();
        for n in SAVERS {
            saver.request_name(n).unwrap();
        }
        Some(Rig { system, session, logind, saver })
    }

    fn buses(r: &Rig) -> Buses {
        let (a, b) = (r.system.addr.clone(), r.session.addr.clone());
        Buses { system: Arc::new(move || at(&a)), session: Arc::new(move || at(&b)), session_id: Some("42".into()) }
    }

    /// Starts the listeners the way `watch` does, on the rig's buses.
    fn start(r: &Rig) {
        WATCHING.store(1, Ordering::Release);
        let (b, epoch) = (buses(r), EPOCH.load(Ordering::Acquire));
        std::thread::spawn(move || run(&b, epoch));
    }

    fn wait_for(what: &str, f: impl Fn() -> bool) {
        let end = Instant::now() + Duration::from_secs(10);
        while !f() {
            assert!(Instant::now() < end, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Sends `f` until the app sees a lock (the listeners may still be subscribing), then lets the
    /// stragglers land and clears the flag.
    fn until_taken(what: &str, f: impl Fn()) {
        let end = Instant::now() + Duration::from_secs(10);
        loop {
            f();
            std::thread::sleep(Duration::from_millis(100));
            if take() {
                break;
            }
            assert!(Instant::now() < end, "timed out waiting for {what}");
        }
        std::thread::sleep(Duration::from_millis(300));
        take();
    }

    fn emit(c: &Connection, path: &str, iface: &str, member: &str, active: Option<bool>) {
        match active {
            Some(a) => c.emit_signal(None::<&str>, path, iface, member, &(a,)).unwrap(),
            None => c.emit_signal(None::<&str>, path, iface, member, &()).unwrap(),
        }
    }

    #[test]
    fn a_logind_lock_is_taken_once() {
        let _g = TURN.lock().unwrap_or_else(|e| e.into_inner());
        let Some(r) = rig() else { return };
        start(&r);
        until_taken("the logind Lock", || emit(&r.logind, SESSION, "org.freedesktop.login1.Session", "Lock", None));
        assert!(!take(), "a lock is taken once");
        // Another session's Lock is not ours.
        emit(&r.logind, "/org/freedesktop/login1/session/_7", "org.freedesktop.login1.Session", "Lock", None);
        std::thread::sleep(Duration::from_millis(300));
        assert!(!take());
    }

    /// A client that is not logind cannot lock the manager by sending the signal itself.
    #[test]
    fn a_lock_from_another_sender_is_ignored() {
        let _g = TURN.lock().unwrap_or_else(|e| e.into_inner());
        let Some(r) = rig() else { return };
        start(&r);
        // Wait until the listeners are in place using the real sender, then try the impostor.
        until_taken("the real Lock", || emit(&r.logind, SESSION, "org.freedesktop.login1.Session", "Lock", None));
        let other = at(&r.system.addr).unwrap();
        emit(&other, SESSION, "org.freedesktop.login1.Session", "Lock", None);
        let other_saver = at(&r.session.addr).unwrap();
        emit(&other_saver, SAVER_PATH, "org.freedesktop.ScreenSaver", "ActiveChanged", Some(true));
        std::thread::sleep(Duration::from_millis(400));
        assert!(!take(), "a signal from a client that does not own the name locked");
    }

    #[test]
    fn a_screensaver_activation_locks() {
        let _g = TURN.lock().unwrap_or_else(|e| e.into_inner());
        let Some(r) = rig() else { return };
        start(&r);
        // false then true: exactly the activation locks, and false alone has had time to show.
        until_taken("ActiveChanged(true)", || {
            emit(&r.saver, SAVER_PATH, "org.freedesktop.ScreenSaver", "ActiveChanged", Some(false));
            std::thread::sleep(Duration::from_millis(300));
            assert!(!take(), "ActiveChanged(false) is not a lock");
            emit(&r.saver, SAVER_PATH, "org.freedesktop.ScreenSaver", "ActiveChanged", Some(true));
        });
        assert!(!take());
    }

    #[test]
    fn gnomes_screensaver_locks_too() {
        let _g = TURN.lock().unwrap_or_else(|e| e.into_inner());
        let Some(r) = rig() else { return };
        start(&r);
        until_taken("GNOME's ActiveChanged(true)", || emit(&r.saver, "/org/gnome/ScreenSaver", "org.gnome.ScreenSaver", "ActiveChanged", Some(true)));
    }

    /// One bus going away closes the whole setup, so its other listeners do not linger.
    #[test]
    fn a_lost_bus_retires_every_listener_of_its_setup() {
        let _g = TURN.lock().unwrap_or_else(|e| e.into_inner());
        let Some(mut r) = rig() else { return };
        start(&r);
        wait_for("three listeners", || LIVE.load(Ordering::Acquire) == 3);
        let _ = r.system.child.kill();
        let _ = r.system.child.wait();
        wait_for("every listener to end", || LIVE.load(Ordering::Acquire) == 0);
        wait_for("the app may set up again", || watched() == 0);
    }

    #[test]
    fn a_connect_that_hangs_is_given_up() {
        let hang: Connect = Arc::new(|| {
            std::thread::sleep(Duration::from_secs(3));
            Err(zbus::Error::Failure("late".into()))
        });
        let t = Instant::now();
        assert!(bounded(Duration::from_millis(200), hang).is_err());
        assert!(t.elapsed() < Duration::from_secs(2), "took {:?}", t.elapsed());
    }

    #[test]
    fn no_bus_means_no_lock_and_a_retry_later() {
        let _g = TURN.lock().unwrap_or_else(|e| e.into_inner());
        reset();
        let dead: Connect = Arc::new(|| Err(zbus::Error::Failure("no bus".into())));
        let buses = Buses { system: dead.clone(), session: dead, session_id: None };
        let epoch = EPOCH.load(Ordering::Acquire);
        WATCHING.store(1, Ordering::Release);
        // Only the retry wait is long; the listeners are not started.
        let t = std::thread::spawn(move || run(&buses, epoch));
        std::thread::sleep(Duration::from_millis(300));
        assert!(!take());
        assert_eq!(watched(), 1, "setup waits out its retry delay before the app may ask again");
        reset();
        drop(t);
    }
}
