//! The screen locking on Linux, as logind and the screen saver report it: the `Lock` signal of
//! this login session (system bus) and `ActiveChanged(true)` of org.freedesktop.ScreenSaver or
//! GNOME's org.gnome.ScreenSaver (session bus). Listener threads only set a flag and wake the
//! loop; the app locks the password manager on its next pass.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use zbus::blocking::{Connection, MessageIterator};
use zbus::zvariant::OwnedObjectPath;
use zbus::{MatchRule, message::Type};

use crate::platform::pty::Notify;

const LOGIND: &str = "org.freedesktop.login1";
const LOGIND_PATH: &str = "/org/freedesktop/login1";
const SAVERS: [&str; 2] = ["org.freedesktop.ScreenSaver", "org.gnome.ScreenSaver"];
/// A failed setup is tried again no sooner than this.
const RETRY: Duration = Duration::from_secs(30);

static LOCKED: AtomicBool = AtomicBool::new(false);
static WAKE: OnceLock<Notify> = OnceLock::new();
/// 1 while the listeners run (or wait out a failed setup).
static WATCHING: AtomicUsize = AtomicUsize::new(0);
/// Bumped to retire the listeners of an earlier setup.
static EPOCH: AtomicU64 = AtomicU64::new(0);
static STARTER: Mutex<()> = Mutex::new(());

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
    let epoch = EPOCH.load(Ordering::Acquire);
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

/// Where the signals come from: the two buses and the login session's id.
struct Buses {
    system: Box<dyn Fn() -> zbus::Result<Connection> + Send>,
    session: Box<dyn Fn() -> zbus::Result<Connection> + Send>,
    session_id: Option<String>,
}

impl Buses {
    fn real() -> Buses {
        Buses { system: Box::new(Connection::system), session: Box::new(Connection::session), session_id: std::env::var("XDG_SESSION_ID").ok() }
    }
}

/// Connects both buses and listens on one thread each. With no bus at all, waits and lets the app ask again.
fn run(buses: &Buses, epoch: u64) {
    let mut listening = 0;
    if let Ok(sys) = (buses.system)()
        && let Some(path) = session_path(&sys, buses.session_id.as_deref())
    {
        listening += listen(&sys, rule(&format!("{LOGIND}.Session"), "Lock", Some(&path)), epoch, false);
    }
    if let Ok(ses) = (buses.session)() {
        for iface in SAVERS {
            listening += listen(&ses, rule(iface, "ActiveChanged", None), epoch, true);
        }
    }
    if listening == 0 {
        std::thread::sleep(RETRY);
        if epoch == EPOCH.load(Ordering::Acquire) {
            WATCHING.store(0, Ordering::Release);
        }
    }
}

fn rule(iface: &str, member: &str, path: Option<&str>) -> Option<MatchRule<'static>> {
    let b = MatchRule::builder().msg_type(Type::Signal).interface(iface.to_string()).ok()?.member(member.to_string()).ok()?;
    let b = match path {
        Some(p) => b.path(p.to_string()).ok()?,
        None => b,
    };
    Some(b.build())
}

/// Listens for one signal on its own thread; true when the subscription is in place.
fn listen(conn: &Connection, rule: Option<MatchRule<'static>>, epoch: u64, wants_true: bool) -> usize {
    let Some(rule) = rule else { return 0 };
    let Ok(iter) = MessageIterator::for_match_rule(rule, conn, None) else { return 0 };
    let started = std::thread::Builder::new().name("screen-lock-signal".into()).spawn(move || {
        for msg in iter {
            let Ok(msg) = msg else { break };
            if wants_true && !matches!(msg.body().deserialize::<bool>(), Ok(true)) {
                continue;
            }
            on_lock(epoch);
        }
        // The bus went away: let the app set up again.
        if epoch == EPOCH.load(Ordering::Acquire) {
            WATCHING.store(0, Ordering::Release);
        }
    });
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

    /// The listeners of one test; retired when it ends. Nothing here touches the environment.
    static TURN: Mutex<()> = Mutex::new(());
    const SESSION: &str = "/org/freedesktop/login1/session/_42";

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

    /// A private bus standing in for both the system and the session bus, with a fake logind on it.
    struct Bus {
        child: Child,
        conn: Connection,
        addr: String,
    }

    impl Drop for Bus {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
            reset();
        }
    }

    /// A clean slate: listeners of an earlier bus are retired.
    fn reset() {
        EPOCH.fetch_add(1, Ordering::AcqRel);
        WATCHING.store(0, Ordering::Release);
        LOCKED.store(false, Ordering::Release);
    }

    fn bus() -> Bus {
        reset();
        let mut child = Command::new("dbus-daemon").args(["--session", "--nofork", "--print-address=1"]).stdout(Stdio::piped()).spawn().expect("dbus-daemon");
        let mut addr = String::new();
        BufReader::new(child.stdout.take().unwrap()).read_line(&mut addr).unwrap();
        let addr = addr.trim().to_string();
        let conn = zbus::blocking::connection::Builder::address(addr.as_str()).unwrap().name(LOGIND).unwrap().serve_at(LOGIND_PATH, Manager).unwrap().build().unwrap();
        Bus { child, conn, addr }
    }

    fn buses_at(addr: &str) -> Buses {
        let (a, b) = (addr.to_string(), addr.to_string());
        let at = |a: String| move || zbus::blocking::connection::Builder::address(a.as_str())?.build();
        Buses { system: Box::new(at(a)), session: Box::new(at(b)), session_id: Some("42".into()) }
    }

    /// Starts the listeners the way `watch` does, on the test's buses.
    fn start(b: &Bus) {
        WATCHING.store(1, Ordering::Release);
        let (buses, epoch) = (buses_at(&b.addr), EPOCH.load(Ordering::Acquire));
        std::thread::spawn(move || run(&buses, epoch));
    }

    /// Sends `f` until the app sees a lock (the listener may still be subscribing), then lets the
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

    fn emit(b: &Bus, path: &str, iface: &str, member: &str, active: Option<bool>) {
        match active {
            Some(a) => b.conn.emit_signal(None::<&str>, path, iface, member, &(a,)).unwrap(),
            None => b.conn.emit_signal(None::<&str>, path, iface, member, &()).unwrap(),
        }
    }

    #[test]
    fn a_logind_lock_is_taken_once() {
        let _g = TURN.lock().unwrap_or_else(|e| e.into_inner());
        let b = bus();
        start(&b);
        until_taken("the logind Lock", || emit(&b, SESSION, "org.freedesktop.login1.Session", "Lock", None));
        assert!(!take(), "a lock is taken once");
        // Another session's Lock is not ours.
        emit(&b, "/org/freedesktop/login1/session/_7", "org.freedesktop.login1.Session", "Lock", None);
        std::thread::sleep(Duration::from_millis(300));
        assert!(!take());
    }

    #[test]
    fn a_screensaver_activation_locks() {
        let _g = TURN.lock().unwrap_or_else(|e| e.into_inner());
        let b = bus();
        start(&b);
        // Waking the screen (false) does nothing; the activation (true) locks.
        until_taken("ActiveChanged(true)", || {
            emit(&b, "/org/freedesktop/ScreenSaver", "org.freedesktop.ScreenSaver", "ActiveChanged", Some(false));
            assert!(!take(), "ActiveChanged(false) is not a lock");
            emit(&b, "/org/freedesktop/ScreenSaver", "org.freedesktop.ScreenSaver", "ActiveChanged", Some(true));
        });
        assert!(!take());
    }

    #[test]
    fn gnomes_screensaver_locks_too() {
        let _g = TURN.lock().unwrap_or_else(|e| e.into_inner());
        let b = bus();
        start(&b);
        until_taken("GNOME's ActiveChanged(true)", || emit(&b, "/org/gnome/ScreenSaver", "org.gnome.ScreenSaver", "ActiveChanged", Some(true)));
    }

    #[test]
    fn no_bus_means_no_lock_and_a_retry_later() {
        let _g = TURN.lock().unwrap_or_else(|e| e.into_inner());
        reset();
        let dead = || Err(zbus::Error::Failure("no bus".into()));
        let buses = Buses { system: Box::new(dead), session: Box::new(dead), session_id: None };
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
