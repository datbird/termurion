//! Desktop notifications on Linux, over the session bus (org.freedesktop.Notifications). One bus
//! connection, one sender thread and one listener thread serve every notification, so a show
//! never waits on the bus: the UI thread only queues it. The listener routes ActionInvoked and
//! NotificationClosed by id; a click on the default action runs the `on_click` closure.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use zbus::blocking::Connection;
use zbus::zvariant::Value;

use super::super::notify::Click;
use super::super::window::APP_ID;
use super::super::Unsupported;
use crate::dlog;

/// The name the notification shows under.
const APP_NAME: &str = "Termurion";
const BUS_NAME: &str = "org.freedesktop.Notifications";
const BUS_PATH: &str = "/org/freedesktop/Notifications";
/// How long a call to the service may take before it is called failed.
const CALL_BOUND: Duration = Duration::from_secs(5);
/// Shows waiting for the sender thread; a full queue means the service is not answering.
const QUEUE_CAP: usize = 32;
/// Notifications whose clicks are still routed; older ones are forgotten.
const LIVE_CAP: usize = 64;

/// The number of the newest show queued; each show gets the next one, and a failed show keeps
/// its number, so a number never names two notifications.
static SHOWN: AtomicU64 = AtomicU64::new(0);
/// Shown notifications that can still be clicked, oldest first.
static LIVE: Mutex<VecDeque<Live>> = Mutex::new(VecDeque::new());
/// Why the last send failed, told once to the next caller.
static FAILURE: Mutex<Option<String>> = Mutex::new(None);
type OnClick = Arc<dyn Fn(Click) + Send + Sync>;
static CLICK: Mutex<Option<OnClick>> = Mutex::new(None);
/// The shared connection, and which one it is (the listener clears only its own).
static LINK: Mutex<Option<(u64, Connection)>> = Mutex::new(None);
static LINK_GEN: AtomicU64 = AtomicU64::new(0);
static QUEUE: OnceLock<SyncSender<Cmd>> = OnceLock::new();

/// One notification on screen: the service's id, Termurion's number for the show, and the
/// activation token the server sent for a click on it (spec 1.2, before ActionInvoked).
struct Live {
    id: u32,
    shown: u64,
    activation: Option<String>,
}

enum Cmd {
    Show { shown: u64, title: String, body: String, silent: bool },
    Close { shown: u64 },
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// What a click on a notification does; it is told which show was clicked.
pub fn on_click(f: Box<dyn Fn(Click) + Send + Sync>) {
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

/// The number of the newest show (see `SHOWN`).
pub fn shown_count() -> u64 {
    SHOWN.load(Ordering::SeqCst)
}

fn run_click(click: Click) {
    let f = lock(&CLICK).clone();
    if let Some(f) = f {
        f(click);
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

/// Who owns the service's name. Any program on the bus can emit the service's signals, so only
/// the owner is believed. It is asked once, then followed by the bus's NameOwnerChanged, so a
/// stranger's signals cost no bus call each.
#[derive(Default)]
struct Owner {
    name: Option<String>,
    /// The owner is known: an ask answered, or the bus told a move.
    known: bool,
    /// When an ask last failed (a timeout, a bus hiccup, nobody owning the name yet).
    failed_at: Option<Instant>,
}

/// How long after a failed ask the owner is asked again.
const OWNER_RETRY: Duration = Duration::from_secs(1);

impl Owner {
    /// Whether `sender` is the service itself. `ask` reads the owner while it is not known: the
    /// first time, and after a failed ask at most once per OWNER_RETRY.
    fn believes(&mut self, sender: Option<&str>, ask: &dyn Fn() -> Option<String>, now: Instant) -> bool {
        let Some(sender) = sender else { return false };
        if !self.known && self.failed_at.is_none_or(|t| now.duration_since(t) >= OWNER_RETRY) {
            self.name = ask();
            self.known = self.name.is_some();
            self.failed_at = (!self.known).then_some(now);
        }
        self.name.as_deref() == Some(sender)
    }

    /// The bus said the name moved to `new` (empty: nobody owns it).
    fn changed(&mut self, new: &str) {
        self.known = true;
        self.failed_at = None;
        self.name = (!new.is_empty()).then(|| new.to_string());
    }
}

fn ask_owner(dbus: &zbus::blocking::fdo::DBusProxy) -> Option<String> {
    zbus::names::BusName::try_from(BUS_NAME).ok().and_then(|n| dbus.get_name_owner(n).ok()).map(|o| o.to_string())
}

/// A NameOwnerChanged for the service's name, from the bus itself: the new owner.
fn owner_change(msg: &zbus::Message) -> Option<String> {
    let header = msg.header();
    if header.sender().map(|s| s.as_str()) != Some("org.freedesktop.DBus") || header.member().map(|m| m.as_str()) != Some("NameOwnerChanged") {
        return None;
    }
    let (name, _, new) = msg.body().deserialize::<(String, String, String)>().ok()?;
    (name == BUS_NAME).then_some(new)
}

/// Starts the one thread that reads the service's signals on `c`.
fn listen(c: Connection, gen_id: u64) -> zbus::Result<()> {
    let proxy = zbus::blocking::fdo::DBusProxy::new(&c)?;
    for member in ["ActionInvoked", "ActivationToken", "NotificationClosed"] {
        let rule = zbus::MatchRule::builder().msg_type(zbus::message::Type::Signal).interface(BUS_NAME)?.member(member)?.build();
        proxy.add_match_rule(rule)?;
    }
    // Before the owner is first asked, so no move is missed.
    let moved = zbus::MatchRule::builder().msg_type(zbus::message::Type::Signal).sender("org.freedesktop.DBus")?.interface("org.freedesktop.DBus")?.member("NameOwnerChanged")?.add_arg(BUS_NAME)?.build();
    proxy.add_match_rule(moved)?;
    let messages = zbus::blocking::MessageIterator::from(c);
    let spawned = std::thread::Builder::new().name("notify-listener".into()).spawn(move || {
        let mut owner = Owner::default();
        for msg in messages {
            let Ok(msg) = msg else { break };
            if let Some(new) = owner_change(&msg) {
                owner.changed(&new);
                continue;
            }
            let header = msg.header();
            if header.message_type() != zbus::message::Type::Signal || header.interface().map(|i| i.as_str()) != Some(BUS_NAME) {
                continue;
            }
            if !owner.believes(header.sender().map(|s| s.as_str()), &|| ask_owner(&proxy), Instant::now()) {
                continue;
            }
            match header.member().map(|m| m.as_str()) {
                Some("ActivationToken") => {
                    if let Ok((id, token)) = msg.body().deserialize::<(u32, String)>() {
                        if let Some(l) = lock(&LIVE).iter_mut().find(|l| l.id == id) {
                            l.activation = Some(token);
                        }
                    }
                }
                Some("ActionInvoked") => {
                    if let Ok((id, action)) = msg.body().deserialize::<(u32, String)>()
                        && action == "default"
                        && let Some(l) = forget(id)
                    {
                        run_click(Click { shown: l.shown, activation: l.activation });
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

fn remember(id: u32, shown: u64) {
    let mut live = lock(&LIVE);
    live.push_back(Live { id, shown, activation: None });
    if live.len() > LIVE_CAP {
        live.pop_front();
    }
}

/// Takes the notification out of the clickable set.
fn forget(id: u32) -> Option<Live> {
    let mut live = lock(&LIVE);
    let at = live.iter().position(|l| l.id == id)?;
    live.remove(at)
}

/// The installed desktop entry that launches Termurion, looked up once (an AppImage run has none
/// until something installs one).
fn desktop_entry() -> Option<String> {
    static ENTRY: OnceLock<Option<String>> = OnceLock::new();
    ENTRY.get_or_init(|| desktop_entry_in(&xdg_data_dirs())).clone()
}

/// XDG_DATA_HOME, then XDG_DATA_DIRS, with the spec's defaults.
fn xdg_data_dirs() -> Vec<std::path::PathBuf> {
    let var = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty());
    let mut dirs: Vec<std::path::PathBuf> = var("XDG_DATA_HOME").map(Into::into).or_else(|| var("HOME").map(|h| std::path::PathBuf::from(h).join(".local/share"))).into_iter().collect();
    let system = var("XDG_DATA_DIRS").unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    dirs.extend(std::env::split_paths(&system));
    dirs
}

/// The entry's id (its file name without `.desktop`) under `<dir>/applications`: the app id's own
/// file first, else one whose StartupWMClass is the app id (an integrator's renamed copy).
fn desktop_entry_in(dirs: &[std::path::PathBuf]) -> Option<String> {
    if dirs.iter().any(|d| d.join("applications").join(format!("{APP_ID}.desktop")).is_file()) {
        return Some(APP_ID.to_string());
    }
    let class = format!("StartupWMClass={APP_ID}");
    for d in dirs {
        let Ok(list) = std::fs::read_dir(d.join("applications")) else { continue };
        for e in list.flatten() {
            let path = e.path();
            if path.extension().is_some_and(|x| x == "desktop")
                && std::fs::read_to_string(&path).is_ok_and(|t| t.lines().any(|l| l.trim() == class))
            {
                return path.file_stem().map(|s| s.to_string_lossy().into_owned());
            }
        }
    }
    None
}

fn notify(title: &str, body: &str, silent: bool) -> Result<u32, String> {
    let (gen_id, c) = link()?;
    let mut hints: HashMap<&str, Value> = HashMap::new();
    // GNOME shows a named entry that is not installed as its raw id; without the hint it shows APP_NAME.
    if let Some(entry) = desktop_entry() {
        hints.insert("desktop-entry", Value::from(entry));
    }
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

/// Closes the service's notification for show `shown`, if it appeared and is still up.
fn close(shown: u64) {
    let Some(id) = lock(&LIVE).iter().find(|l| l.shown == shown).map(|l| l.id) else { return };
    let Ok((gen_id, c)) = link() else { return };
    if c.call_method(Some(BUS_NAME), BUS_PATH, Some(BUS_NAME), "CloseNotification", &(id,)).is_err() {
        drop_link(gen_id, c);
    }
}

/// The sender thread: shows and closes in the order they were queued.
fn work(rx: Receiver<Cmd>) {
    for cmd in rx {
        match cmd {
            Cmd::Show { shown, title, body, silent } => match notify(&title, &body, silent) {
                Ok(id) => {
                    *lock(&FAILURE) = None;
                    remember(id, shown);
                }
                Err(e) => {
                    dlog!("The notification was not shown");
                    *lock(&FAILURE) = Some(e);
                }
            },
            Cmd::Close { shown } => close(shown),
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

/// Queues a notification and returns at once. The sender thread finds out later whether it
/// appeared, so a failure is told to the next caller, named as the earlier show's.
pub fn show_with(title: &str, body: &str, silent: bool) -> Result<(), String> {
    let q = queue()?;
    let shown = SHOWN.fetch_add(1, Ordering::SeqCst) + 1;
    q.try_send(Cmd::Show { shown, title: title.to_string(), body: body.to_string(), silent }).map_err(|_| "The notification service is not answering".to_string())?;
    lock(&FAILURE).take().map_or(Ok(()), |e| Err(format!("{} {e}", super::super::notify::EARLIER_NOT_SHOWN)))
}

/// Closes the notification if it is still the one on screen (nothing was shown since `at`).
pub fn clear_if_shown(at: u64) -> bool {
    if shown_count() != at {
        return false;
    }
    queue().is_ok_and(|q| q.try_send(Cmd::Close { shown: at }).is_ok())
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
        *lock(&FAILURE) = None;
        lock(&LIVE).clear();
    }

    /// The service's id for show `shown`, once it appeared.
    fn id_of(shown: u64) -> u32 {
        wait_for("the service's id", || lock(&LIVE).iter().any(|l| l.shown == shown));
        lock(&LIVE).iter().find(|l| l.shown == shown).map(|l| l.id).unwrap()
    }

    /// Records each click the listener runs.
    fn clicks() -> Arc<Mutex<Vec<Click>>> {
        let got = Arc::new(Mutex::new(Vec::new()));
        let g = got.clone();
        on_click(Box::new(move |c| g.lock().unwrap().push(c)));
        got
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

    /// A dead service is told to the next caller as the earlier show's failure, and a failed
    /// show's number is never another notification's, so it cannot close a later one.
    #[test]
    fn a_failed_send_reaches_the_next_caller_and_its_number_names_no_later_show() {
        let _g = BUS.lock().unwrap_or_else(|e| e.into_inner());
        reset();
        let saved = std::env::var_os("DBUS_SESSION_BUS_ADDRESS");
        let dir = std::env::temp_dir().join(format!("termurion-nobus2-{}", std::process::id()));
        // SAFETY: the BUS lock keeps the other tests off the environment.
        unsafe { std::env::set_var("DBUS_SESSION_BUS_ADDRESS", format!("unix:path={}/none", dir.display())) };
        assert!(show_with("t", "b", false).is_ok(), "the first show cannot know yet");
        let failed = shown_count();
        wait_for("the failure", || lock(&FAILURE).is_some());
        // The service comes up; the next show still hears about the earlier failure, and retries.
        let b = start_bus(Duration::ZERO);
        let t = Instant::now();
        let e = show_with("t", "b", false).unwrap_err();
        assert!(t.elapsed() < Duration::from_millis(500), "took {:?}", t.elapsed());
        assert!(e.starts_with("The last notification was not shown.") && e.contains("notification service"), "{e}");
        assert!(super::super::super::notify::went_out(&Err(e.clone())), "this show was queued: m6");
        wait_for("the retry", || !b.calls.lock().unwrap().is_empty());
        let retry = shown_count();
        assert_eq!(retry, failed + 1);
        let id = id_of(retry);
        assert!(lock(&FAILURE).is_none(), "told once");
        assert!(!clear_if_shown(failed), "the failed show's number is not the retry's");
        assert!(clear_if_shown(retry));
        wait_for("the close", || b.closed.lock().unwrap().contains(&id));
        assert_eq!(b.closed.lock().unwrap().len(), 1);
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
    }

    /// The hint names an installed entry only: GNOME shows a named entry that is not there as the
    /// raw id. An AppImage integrator's copy under another file name is found by its class.
    #[test]
    fn the_desktop_entry_hint_needs_an_installed_entry() {
        let root = std::env::temp_dir().join(format!("termurion-entry-{}", std::process::id()));
        let (home, sys) = (root.join("home"), root.join("sys"));
        for d in [&home, &sys] {
            std::fs::create_dir_all(d.join("applications")).unwrap();
        }
        let dirs = [home.clone(), sys.clone()];
        assert_eq!(desktop_entry_in(&dirs), None, "nothing installed: no hint");
        std::fs::write(sys.join("applications/other.desktop"), "[Desktop Entry]\nStartupWMClass=other\n").unwrap();
        assert_eq!(desktop_entry_in(&dirs), None);
        std::fs::write(home.join("applications/appimagekit_ab12-Termurion.desktop"), format!("[Desktop Entry]\nName=Termurion\nStartupWMClass={APP_ID}\n")).unwrap();
        assert_eq!(desktop_entry_in(&dirs).as_deref(), Some("appimagekit_ab12-Termurion"));
        std::fs::write(sys.join(format!("applications/{APP_ID}.desktop")), "[Desktop Entry]\n").unwrap();
        assert_eq!(desktop_entry_in(&dirs).as_deref(), Some(APP_ID), "the app id's own file wins");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_click_runs_on_click() {
        let _g = BUS.lock().unwrap_or_else(|e| e.into_inner());
        let b = bus();
        let got = clicks();
        show_with("t", "b", false).expect("shown");
        let id = id_of(shown_count());
        // The listener thread may not have subscribed yet, so signal until it answers.
        wait_for("the click", || {
            let _ = b.conn.emit_signal(None::<&str>, BUS_PATH, BUS_NAME, "ActionInvoked", &(id, "default"));
            !got.lock().unwrap().is_empty()
        });
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(got.lock().unwrap().len(), 1, "the closure ran once");
    }

    /// A click on an older notification names that show, not the newest, and carries the
    /// activation token the server sent first (spec 1.2), so its own session comes front.
    #[test]
    fn a_click_names_its_own_show_and_its_activation_token() {
        let _g = BUS.lock().unwrap_or_else(|e| e.into_inner());
        let b = bus();
        let got = clicks();
        show_with("first", "b", false).expect("shown");
        let first = shown_count();
        show_with("second", "b", false).expect("shown");
        let (id1, _) = (id_of(first), id_of(shown_count()));
        // The listener may not have subscribed yet: send the token until it is kept, then click.
        wait_for("the token", || {
            let _ = b.conn.emit_signal(None::<&str>, BUS_PATH, BUS_NAME, "ActivationToken", &(id1, "tok-1"));
            lock(&LIVE).iter().any(|l| l.activation.is_some())
        });
        b.conn.emit_signal(None::<&str>, BUS_PATH, BUS_NAME, "ActionInvoked", &(id1, "default")).unwrap();
        wait_for("the click", || !got.lock().unwrap().is_empty());
        assert_eq!(got.lock().unwrap()[0], Click { shown: first, activation: Some("tok-1".into()) });
    }

    /// Only the notification service is believed: another program on the bus that emits a click
    /// for a live id does nothing.
    #[test]
    fn a_click_from_another_program_is_ignored() {
        let _g = BUS.lock().unwrap_or_else(|e| e.into_inner());
        let b = bus();
        let got = clicks();
        show_with("t", "b", false).expect("shown");
        let id = id_of(shown_count());
        let addr = std::env::var("DBUS_SESSION_BUS_ADDRESS").unwrap();
        let rogue = zbus::blocking::connection::Builder::address(addr.as_str()).unwrap().build().unwrap();
        for _ in 0..20 {
            rogue.emit_signal(None::<&str>, BUS_PATH, BUS_NAME, "ActionInvoked", &(id, "default")).unwrap();
            std::thread::sleep(Duration::from_millis(25));
        }
        std::thread::sleep(Duration::from_millis(300));
        assert!(got.lock().unwrap().is_empty(), "a stranger's signal ran the click");
        wait_for("the service's click", || {
            let _ = b.conn.emit_signal(None::<&str>, BUS_PATH, BUS_NAME, "ActionInvoked", &(id, "default"));
            !got.lock().unwrap().is_empty()
        });
    }

    /// m4: the service's owner is asked once, then followed by NameOwnerChanged; a stranger
    /// sending signals costs no bus call each.
    #[test]
    fn a_stranger_costs_no_bus_call_per_signal() {
        let asks = std::cell::Cell::new(0);
        let ask = || {
            asks.set(asks.get() + 1);
            Some(":1.5".to_string())
        };
        let mut o = Owner::default();
        let t = Instant::now();
        assert!(o.believes(Some(":1.5"), &ask, t));
        for _ in 0..100 {
            assert!(!o.believes(Some(":1.9"), &ask, t));
        }
        assert!(!o.believes(None, &ask, t));
        assert_eq!(asks.get(), 1, "asked once");
        o.changed(":1.9");
        assert!(o.believes(Some(":1.9"), &ask, t), "the new owner is believed");
        assert!(!o.believes(Some(":1.5"), &ask, t), "the old one is not");
        o.changed("");
        assert!(!o.believes(Some(":1.9"), &ask, t), "nobody owns the name");
        assert_eq!(asks.get(), 1);
    }

    /// n1: a failed owner ask is tried again, at most once per OWNER_RETRY, so a bus hiccup does
    /// not refuse the real service for good and a stranger still costs no call per signal.
    #[test]
    fn a_failed_owner_ask_is_tried_again_later() {
        let asks = std::cell::Cell::new(0);
        let answer = std::cell::RefCell::new(None::<String>);
        let ask = || {
            asks.set(asks.get() + 1);
            answer.borrow().clone()
        };
        let t0 = Instant::now();
        let mut o = Owner::default();
        assert!(!o.believes(Some(":1.5"), &ask, t0));
        for i in 0..100 {
            assert!(!o.believes(Some(":1.5"), &ask, t0 + Duration::from_millis(i)));
        }
        assert_eq!(asks.get(), 1, "not asked again per signal");
        *answer.borrow_mut() = Some(":1.5".into());
        assert!(o.believes(Some(":1.5"), &ask, t0 + OWNER_RETRY), "asked again a while later");
        assert_eq!(asks.get(), 2);
        assert!(!o.believes(Some(":1.9"), &ask, t0 + OWNER_RETRY * 5));
        assert_eq!(asks.get(), 2, "a known owner is not asked again");
    }

    /// m4: a service that restarts under a new connection is believed once the bus says the name
    /// moved.
    #[test]
    fn a_restarted_service_is_believed() {
        let _g = BUS.lock().unwrap_or_else(|e| e.into_inner());
        let b = bus();
        let got = clicks();
        show_with("t", "b", false).expect("shown");
        id_of(shown_count());
        let addr = std::env::var("DBUS_SESSION_BUS_ADDRESS").unwrap();
        b.conn.release_name(BUS_NAME).unwrap();
        let daemon = Daemon { calls: Arc::default(), closed: Arc::default(), senders: Arc::default(), hang: Duration::ZERO };
        let next = zbus::blocking::connection::Builder::address(addr.as_str()).unwrap().name(BUS_NAME).unwrap().serve_at(BUS_PATH, daemon).unwrap().build().unwrap();
        show_with("t", "b", false).expect("shown");
        let shown = shown_count();
        let id = id_of(shown);
        wait_for("the new owner's click", || {
            let _ = next.emit_signal(None::<&str>, BUS_PATH, BUS_NAME, "ActionInvoked", &(id, "default"));
            got.lock().unwrap().iter().any(|c| c.shown == shown)
        });
    }

    /// A failed CloseNotification drops the connection, so the next call makes a fresh one.
    #[test]
    fn a_failed_close_resets_the_connection() {
        let _g = BUS.lock().unwrap_or_else(|e| e.into_inner());
        let b = bus();
        show_with("t", "b", false).expect("shown");
        let at = shown_count();
        id_of(at);
        let before = lock(&LINK).as_ref().map(|(n, _)| *n).expect("a connection");
        // The service leaves the bus: the close finds nobody.
        b.conn.release_name(BUS_NAME).unwrap();
        assert!(clear_if_shown(at));
        wait_for("the connection to go", || lock(&LINK).as_ref().is_none_or(|(n, _)| *n != before));
    }

    #[test]
    fn clear_closes_only_the_latest() {
        let _g = BUS.lock().unwrap_or_else(|e| e.into_inner());
        let b = bus();
        show_with("t", "b", false).expect("shown");
        let at = shown_count();
        let id = id_of(at);
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
        id_of(shown_count());
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
