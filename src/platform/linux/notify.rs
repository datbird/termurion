//! Desktop notifications on Linux, over the session bus (org.freedesktop.Notifications). One bus
//! connection, one sender thread and one listener thread serve every notification, so a show
//! never waits on the bus: the UI thread only queues it. The listener routes ActionInvoked and
//! NotificationClosed by id; a click on the default action runs the `on_click` closure.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use zbus::blocking::Connection;
use zbus::zvariant::Value;

use super::super::Unsupported;
use crate::dlog;

/// The name the notification shows under.
const APP_NAME: &str = "Termurion";
/// The desktop entry (app id) the shell matches to Termurion's launcher and icon.
const APP_ID: &str = "termurion";
const BUS_NAME: &str = "org.freedesktop.Notifications";
const BUS_PATH: &str = "/org/freedesktop/Notifications";
/// How long a call to the service may take before it is called failed.
const CALL_BOUND: Duration = Duration::from_secs(5);
/// Shows waiting for the sender thread; a full queue means the service is not answering.
const QUEUE_CAP: usize = 32;
/// Notifications whose clicks are still routed; older ones are forgotten.
const LIVE_CAP: usize = 64;

static SHOWN: AtomicU64 = AtomicU64::new(0);
/// The service's id of the last notification shown, for closing it (0: none).
static LAST_ID: Mutex<u32> = Mutex::new(0);
/// Ids of shown notifications that can still be clicked.
static LIVE: Mutex<VecDeque<u32>> = Mutex::new(VecDeque::new());
/// Why the last send failed, for a person reading the log.
static FAILURE: Mutex<Option<String>> = Mutex::new(None);
type Click = Arc<dyn Fn() + Send + Sync>;
static CLICK: Mutex<Option<Click>> = Mutex::new(None);
/// The shared connection, and which one it is (the listener clears only its own).
static LINK: Mutex<Option<(u64, Connection)>> = Mutex::new(None);
static LINK_GEN: AtomicU64 = AtomicU64::new(0);
static QUEUE: OnceLock<SyncSender<Cmd>> = OnceLock::new();

enum Cmd {
    Show { title: String, body: String, silent: bool },
    Close,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// What a click on a notification does.
pub fn on_click(f: Box<dyn Fn() + Send + Sync>) {
    *lock(&CLICK) = Some(Arc::from(f));
}

pub const MENU_AT_ICON: bool = false;

/// What a click on the resident icon does. There is no icon on Linux yet.
pub fn on_tray(_f: Box<dyn Fn(bool) + Send + Sync>) {}

/// What Explorer restarting does. Linux has no such event.
pub fn on_taskbar_created(_f: Box<dyn Fn() + Send + Sync>) {}

/// No hidden notification window exists on Linux.
pub fn window_handle() -> Option<isize> {
    None
}

/// The tray comes later.
pub fn set_resident(_on: bool) -> Result<(), String> {
    Err(Unsupported { feature: "The resident icon", plural: false }.into())
}

pub fn shown_count() -> u64 {
    SHOWN.load(Ordering::SeqCst)
}

fn run_click() {
    let f = lock(&CLICK).clone();
    if let Some(f) = f {
        f();
    }
}

fn unreachable_text(e: impl std::fmt::Display) -> String {
    format!("Termurion could not reach the notification service: {e}")
}

/// The shared connection, made (with its listener) on first use.
fn link() -> Result<(u64, Connection), String> {
    let mut g = lock(&LINK);
    if let Some((n, c)) = g.as_ref() {
        return Ok((*n, c.clone()));
    }
    let c = zbus::blocking::connection::Builder::session().and_then(|b| b.method_timeout(CALL_BOUND).build()).map_err(unreachable_text)?;
    let gen_id = LINK_GEN.fetch_add(1, Ordering::SeqCst) + 1;
    listen(c.clone(), gen_id).map_err(unreachable_text)?;
    *g = Some((gen_id, c.clone()));
    Ok((gen_id, c))
}

/// Starts the one thread that reads the service's signals on `c`.
fn listen(c: Connection, gen_id: u64) -> zbus::Result<()> {
    let proxy = zbus::blocking::fdo::DBusProxy::new(&c)?;
    for member in ["ActionInvoked", "NotificationClosed"] {
        let rule = zbus::MatchRule::builder().msg_type(zbus::message::Type::Signal).interface(BUS_NAME)?.member(member)?.build();
        proxy.add_match_rule(rule)?;
    }
    let messages = zbus::blocking::MessageIterator::from(c);
    let spawned = std::thread::Builder::new().name("notify-listener".into()).spawn(move || {
        for msg in messages {
            let Ok(msg) = msg else { break };
            let header = msg.header();
            if header.interface().map(|i| i.as_str()) != Some(BUS_NAME) {
                continue;
            }
            match header.member().map(|m| m.as_str()) {
                Some("ActionInvoked") => {
                    if let Ok((id, action)) = msg.body().deserialize::<(u32, String)>()
                        && action == "default"
                        && forget(id)
                    {
                        run_click();
                    }
                }
                Some("NotificationClosed") => {
                    if let Ok((id, _)) = msg.body().deserialize::<(u32, u32)>() {
                        forget(id);
                    }
                }
                _ => {}
            }
        }
        // The bus went away: the next send connects again.
        let mut g = lock(&LINK);
        if g.as_ref().is_some_and(|(n, _)| *n == gen_id) {
            *g = None;
        }
    });
    spawned.map(|_| ()).map_err(|e| zbus::Error::Failure(e.to_string()))
}

/// A call failed or timed out: close the connection (which ends its listener) so the next send makes a fresh one.
fn drop_link(gen_id: u64, c: Connection) {
    let mut g = lock(&LINK);
    if g.as_ref().is_some_and(|(n, _)| *n == gen_id) {
        *g = None;
    }
    drop(g);
    let _ = c.graceful_shutdown();
}

fn remember(id: u32) {
    let mut live = lock(&LIVE);
    live.push_back(id);
    if live.len() > LIVE_CAP {
        live.pop_front();
    }
}

/// Drops the id from the clickable set; whether it was there.
fn forget(id: u32) -> bool {
    let mut live = lock(&LIVE);
    let before = live.len();
    live.retain(|n| *n != id);
    live.len() != before
}

fn notify(title: &str, body: &str, silent: bool) -> Result<u32, String> {
    let (gen_id, c) = link()?;
    let mut hints: HashMap<&str, Value> = HashMap::new();
    hints.insert("desktop-entry", Value::from(APP_ID));
    if silent {
        hints.insert("suppress-sound", Value::from(true));
    }
    let actions = ["default", "Show"];
    let sent = c.call_method(Some(BUS_NAME), BUS_PATH, Some(BUS_NAME), "Notify", &(APP_NAME, 0u32, "", title, body, &actions[..], hints, -1i32));
    let reply = match sent {
        Ok(r) => r,
        Err(e) => {
            drop_link(gen_id, c);
            return Err(unreachable_text(e));
        }
    };
    reply.body().deserialize::<u32>().map_err(unreachable_text)
}

/// The sender thread: shows and closes in the order they were queued.
fn work(rx: Receiver<Cmd>) {
    for cmd in rx {
        match cmd {
            Cmd::Show { title, body, silent } => match notify(&title, &body, silent) {
                Ok(id) => {
                    *lock(&FAILURE) = None;
                    *lock(&LAST_ID) = id;
                    remember(id);
                }
                Err(e) => {
                    // The show was counted when it was queued; it never appeared.
                    SHOWN.fetch_sub(1, Ordering::SeqCst);
                    *lock(&LAST_ID) = 0;
                    dlog!("The notification was not shown");
                    *lock(&FAILURE) = Some(e);
                }
            },
            Cmd::Close => {
                let id = std::mem::take(&mut *lock(&LAST_ID));
                if id != 0
                    && let Ok((_, c)) = link()
                {
                    let _ = c.call_method(Some(BUS_NAME), BUS_PATH, Some(BUS_NAME), "CloseNotification", &(id,));
                }
            }
        }
    }
}

fn queue() -> Result<&'static SyncSender<Cmd>, String> {
    if let Some(q) = QUEUE.get() {
        return Ok(q);
    }
    let (tx, rx) = sync_channel(QUEUE_CAP);
    std::thread::Builder::new().name("notify-worker".into()).spawn(move || work(rx)).map_err(|e| format!("Notifications could not start: {e}"))?;
    Ok(QUEUE.get_or_init(|| tx))
}

/// Queues a notification and returns at once; a failure to reach the service is logged by the
/// sender thread, as on the other systems.
pub fn show_with(title: &str, body: &str, silent: bool) -> Result<(), String> {
    // An earlier send failed: say so now, and still try this one.
    let earlier = lock(&FAILURE).clone();
    queue()?.try_send(Cmd::Show { title: title.to_string(), body: body.to_string(), silent }).map_err(|_| "The notification service is not answering".to_string())?;
    SHOWN.fetch_add(1, Ordering::SeqCst);
    earlier.map_or(Ok(()), Err)
}

/// Closes the notification if it is still the one on screen (nothing was shown since `at`).
pub fn clear_if_shown(at: u64) -> bool {
    if shown_count() != at {
        return false;
    }
    queue().is_ok_and(|q| q.try_send(Cmd::Close).is_ok())
}

pub fn show(_owner: isize, title: &str, body: &str) -> Result<(), String> {
    show_with(title, body, false)
}

pub fn remove(_owner: isize) {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::io::{BufRead, BufReader};
    use std::process::{Child, Command, Stdio};
    use std::sync::atomic::AtomicUsize;
    use std::time::Instant;

    /// The bus address is process wide, so the tests take turns.
    static BUS: Mutex<()> = Mutex::new(());

    /// One recorded Notify call.
    #[derive(Clone, Debug)]
    struct Call {
        app: String,
        title: String,
        body: String,
        actions: Vec<String>,
        entry: Option<String>,
    }

    struct Daemon {
        calls: Arc<Mutex<Vec<Call>>>,
        closed: Arc<Mutex<Vec<u32>>>,
        senders: Arc<Mutex<Vec<String>>>,
        hang: Duration,
    }

    #[zbus::interface(name = "org.freedesktop.Notifications")]
    impl Daemon {
        #[allow(clippy::too_many_arguments)]
        fn notify(&self, #[zbus(header)] hdr: zbus::message::Header<'_>, app: String, _replaces: u32, _icon: String, title: String, body: String, actions: Vec<String>, hints: HashMap<String, zbus::zvariant::OwnedValue>, _timeout: i32) -> u32 {
            self.senders.lock().unwrap().push(hdr.sender().map(|s| s.to_string()).unwrap_or_default());
            std::thread::sleep(self.hang);
            let entry = hints.get("desktop-entry").and_then(|v| <String>::try_from(v.clone()).ok());
            let mut c = self.calls.lock().unwrap();
            c.push(Call { app, title, body, actions, entry });
            c.len() as u32 + 40
        }
        fn close_notification(&self, id: u32) {
            self.closed.lock().unwrap().push(id);
        }
        fn get_capabilities(&self) -> Vec<String> {
            vec!["actions".into(), "body".into()]
        }
        fn get_server_information(&self) -> (String, String, String, String) {
            ("test".into(), "test".into(), "1".into(), "1.2".into())
        }
    }

    /// A private session bus with a listener that owns the notification name.
    struct Bus {
        child: Child,
        conn: zbus::blocking::Connection,
        calls: Arc<Mutex<Vec<Call>>>,
        closed: Arc<Mutex<Vec<u32>>>,
        senders: Arc<Mutex<Vec<String>>>,
        saved: Option<std::ffi::OsString>,
    }

    impl Drop for Bus {
        fn drop(&mut self) {
            restore_bus(self.saved.take());
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    fn restore_bus(saved: Option<std::ffi::OsString>) {
        // SAFETY: the BUS lock is held by the test that owns this guard.
        unsafe {
            match saved {
                Some(v) => std::env::set_var("DBUS_SESSION_BUS_ADDRESS", v),
                None => std::env::remove_var("DBUS_SESSION_BUS_ADDRESS"),
            }
        }
    }

    fn bus() -> Bus {
        bus_hanging(Duration::ZERO)
    }

    /// A clean slate: the shared connection and the last send's state belong to the last bus.
    fn reset() {
        *lock(&LINK) = None;
        *lock(&LAST_ID) = 0;
        *lock(&FAILURE) = None;
        lock(&LIVE).clear();
    }

    fn last_id() -> u32 {
        wait_for("the service's id", || *lock(&LAST_ID) != 0);
        *lock(&LAST_ID)
    }

    /// A bus whose Notify handler takes `hang` to answer.
    fn bus_hanging(hang: Duration) -> Bus {
        reset();
        start_bus(hang)
    }

    fn start_bus(hang: Duration) -> Bus {
        let saved = std::env::var_os("DBUS_SESSION_BUS_ADDRESS");
        let mut child = Command::new("dbus-daemon").args(["--session", "--nofork", "--print-address=1"]).stdout(Stdio::piped()).spawn().expect("dbus-daemon");
        let mut addr = String::new();
        BufReader::new(child.stdout.take().unwrap()).read_line(&mut addr).unwrap();
        let addr = addr.trim().to_string();
        // SAFETY: the BUS lock keeps the other tests off the environment while this one is set.
        unsafe { std::env::set_var("DBUS_SESSION_BUS_ADDRESS", &addr) };
        let calls = Arc::new(Mutex::new(Vec::new()));
        let closed = Arc::new(Mutex::new(Vec::new()));
        let senders = Arc::new(Mutex::new(Vec::new()));
        let conn = zbus::blocking::connection::Builder::address(addr.as_str())
            .unwrap()
            .name(BUS_NAME)
            .unwrap()
            .serve_at(BUS_PATH, Daemon { calls: calls.clone(), closed: closed.clone(), senders: senders.clone(), hang })
            .unwrap()
            .build()
            .unwrap();
        Bus { child, conn, calls, closed, senders, saved }
    }

    fn wait_for(what: &str, f: impl Fn() -> bool) {
        let end = Instant::now() + Duration::from_secs(10);
        while !f() {
            assert!(Instant::now() < end, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn notifications_without_a_daemon_say_so() {
        let _g = BUS.lock().unwrap_or_else(|e| e.into_inner());
        reset();
        let saved = std::env::var_os("DBUS_SESSION_BUS_ADDRESS");
        let dir = std::env::temp_dir().join(format!("termurion-nobus-{}", std::process::id()));
        // SAFETY: the BUS lock keeps the other tests off the environment.
        unsafe { std::env::set_var("DBUS_SESSION_BUS_ADDRESS", format!("unix:path={}/none", dir.display())) };
        let t = Instant::now();
        assert!(show_with("t", "b", false).is_ok(), "the failure is reported without waiting");
        assert!(t.elapsed() < Duration::from_millis(500), "took {:?}", t.elapsed());
        wait_for("the failure", || lock(&FAILURE).is_some());
        restore_bus(saved);
        let e = lock(&FAILURE).clone().unwrap();
        assert!(e.contains("notification service"), "{e}");
    }

    /// A dead service is told to the caller on the next show, and a failed show is not counted.
    #[test]
    fn a_failed_send_reaches_the_next_caller_and_is_not_counted() {
        let _g = BUS.lock().unwrap_or_else(|e| e.into_inner());
        reset();
        let saved = std::env::var_os("DBUS_SESSION_BUS_ADDRESS");
        let dir = std::env::temp_dir().join(format!("termurion-nobus2-{}", std::process::id()));
        // SAFETY: the BUS lock keeps the other tests off the environment.
        unsafe { std::env::set_var("DBUS_SESSION_BUS_ADDRESS", format!("unix:path={}/none", dir.display())) };
        let before = shown_count();
        assert!(show_with("t", "b", false).is_ok(), "the first show cannot know yet");
        wait_for("the failure", || lock(&FAILURE).is_some());
        assert_eq!(shown_count(), before, "a failed show is not counted");
        // The service comes up; the next show still hears about the earlier failure, and retries.
        let b = start_bus(Duration::ZERO);
        let t = Instant::now();
        let e = show_with("t", "b", false).unwrap_err();
        assert!(t.elapsed() < Duration::from_millis(500), "took {:?}", t.elapsed());
        assert!(e.contains("notification service"), "{e}");
        wait_for("the retry", || !b.calls.lock().unwrap().is_empty());
        wait_for("the failure to clear", || lock(&FAILURE).is_none());
        assert_eq!(shown_count(), before + 1, "only the retry counts");
        restore_bus(saved);
    }

    #[test]
    fn a_notification_reaches_the_daemon() {
        let _g = BUS.lock().unwrap_or_else(|e| e.into_inner());
        let b = bus();
        let before = shown_count();
        show_with("Build done", "all green", false).expect("shown");
        assert_eq!(shown_count(), before + 1);
        wait_for("the Notify call", || !b.calls.lock().unwrap().is_empty());
        let calls = b.calls.lock().unwrap();
        let c = calls.last().expect("a Notify call");
        assert_eq!((c.app.as_str(), c.title.as_str(), c.body.as_str()), ("Termurion", "Build done", "all green"));
        assert!(c.actions.iter().any(|a| a == "default"), "{:?}", c.actions);
        assert_eq!(c.entry.as_deref(), Some("termurion"));
    }

    #[test]
    fn a_click_runs_on_click() {
        let _g = BUS.lock().unwrap_or_else(|e| e.into_inner());
        let b = bus();
        let ran = Arc::new(AtomicUsize::new(0));
        let r = ran.clone();
        on_click(Box::new(move || {
            r.fetch_add(1, Ordering::SeqCst);
        }));
        show_with("t", "b", false).expect("shown");
        let id = last_id();
        // The listener thread may not have subscribed yet, so signal until it answers.
        wait_for("the click", || {
            let _ = b.conn.emit_signal(None::<&str>, BUS_PATH, BUS_NAME, "ActionInvoked", &(id, "default"));
            ran.load(Ordering::SeqCst) >= 1
        });
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(ran.load(Ordering::SeqCst), 1, "the closure ran once");
    }

    #[test]
    fn clear_closes_only_the_latest() {
        let _g = BUS.lock().unwrap_or_else(|e| e.into_inner());
        let b = bus();
        show_with("t", "b", false).expect("shown");
        let at = shown_count();
        let id = last_id();
        assert!(!clear_if_shown(at - 1), "a newer show is not ours to clear");
        assert!(clear_if_shown(at));
        wait_for("the close", || b.closed.lock().unwrap().contains(&id));
    }

    /// A daemon that sits on Notify must not hold the caller: the UI thread calls this.
    #[test]
    fn show_with_returns_at_once_while_the_daemon_hangs() {
        let _g = BUS.lock().unwrap_or_else(|e| e.into_inner());
        let _b = bus_hanging(Duration::from_secs(1));
        let t = Instant::now();
        let r = show_with("t", "b", false);
        assert!(t.elapsed() < Duration::from_millis(500), "show_with held the caller for {:?}", t.elapsed());
        assert!(r.is_ok(), "{r:?}");
        // Let the sender finish its slow call, so it is idle for the next test.
        last_id();
    }

    /// Many notifications share one bus connection and a fixed few threads.
    #[test]
    fn many_notifications_share_one_connection_and_few_threads() {
        let _g = BUS.lock().unwrap_or_else(|e| e.into_inner());
        let b = bus();
        for i in 0..20 {
            let _ = show_with("t", &format!("b{i}"), false);
        }
        wait_for("all twenty", || b.calls.lock().unwrap().len() == 20);
        let mut senders = b.senders.lock().unwrap().clone();
        senders.sort();
        senders.dedup();
        assert_eq!(senders.len(), 1, "connections used: {senders:?}");
        let ours = || {
            std::fs::read_dir("/proc/self/task")
                .unwrap()
                .flatten()
                .filter_map(|t| std::fs::read_to_string(t.path().join("comm")).ok())
                .filter(|c| c.starts_with("notify"))
                .collect::<Vec<_>>()
        };
        // Threads of an earlier test's bus may still be leaving.
        wait_for("at most the sender and the listener", || {
            let n = ours();
            n.len() <= 2 || { eprintln!("threads: {n:?}"); false }
        });
    }
}
