//! Desktop notifications on Linux, over the session bus (org.freedesktop.Notifications). The send
//! runs off the calling thread with a 5 s bound; a click on the default action runs the `on_click`
//! closure from the listener thread that owns the notification.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use notify_rust::{Hint, Notification, Timeout};

use super::super::Unsupported;

/// The name the notification shows under.
const APP_NAME: &str = "Termurion";
/// The desktop entry (app id) the shell matches to Termurion's launcher and icon.
const APP_ID: &str = "termurion";
const BUS_NAME: &str = "org.freedesktop.Notifications";
const BUS_PATH: &str = "/org/freedesktop/Notifications";
/// How long a send may take before it is called failed.
const SEND_BOUND: Duration = Duration::from_secs(5);

static SHOWN: AtomicU64 = AtomicU64::new(0);
/// The service's id of the last notification shown, for closing it.
static LAST_ID: Mutex<u32> = Mutex::new(0);
type Click = Arc<dyn Fn() + Send + Sync>;
static CLICK: Mutex<Option<Click>> = Mutex::new(None);

/// What a click on a notification does.
pub fn on_click(f: Box<dyn Fn() + Send + Sync>) {
    *CLICK.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::from(f));
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
    let f = CLICK.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if let Some(f) = f {
        f();
    }
}

/// Sends one notification, then waits on its default action for the life of the notification.
fn send(title: &str, body: &str, silent: bool, told: std::sync::mpsc::Sender<Result<u32, String>>) {
    let mut n = Notification::new();
    n.appname(APP_NAME).summary(title).body(body).hint(Hint::DesktopEntry(APP_ID.into())).action("default", "Show").timeout(Timeout::Default);
    if silent {
        n.hint(Hint::SuppressSound(true));
    }
    match n.show() {
        Ok(h) => {
            let _ = told.send(Ok(h.id()));
            h.wait_for_action(|a| {
                if a == "default" {
                    run_click();
                }
            });
        }
        Err(e) => {
            let _ = told.send(Err(format!("Termurion could not reach the notification service: {e}")));
        }
    }
}

pub fn show_with(title: &str, body: &str, silent: bool) -> Result<(), String> {
    let (tx, rx) = channel();
    let (t, b) = (title.to_string(), body.to_string());
    std::thread::Builder::new().name("notify".into()).spawn(move || send(&t, &b, silent, tx)).map_err(|e| format!("Notifications could not start: {e}"))?;
    match rx.recv_timeout(SEND_BOUND) {
        Ok(Ok(id)) => {
            *LAST_ID.lock().unwrap_or_else(|e| e.into_inner()) = id;
            SHOWN.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        Ok(Err(e)) => Err(e),
        Err(_) => Err("The notification service did not answer in time".to_string()),
    }
}

/// Closes the notification if it is still the one on screen (nothing was shown since `at`).
pub fn clear_if_shown(at: u64) -> bool {
    if shown_count() != at {
        return false;
    }
    let id = *LAST_ID.lock().unwrap_or_else(|e| e.into_inner());
    if id == 0 {
        return false;
    }
    std::thread::spawn(move || {
        if let Ok(c) = zbus::blocking::Connection::session() {
            let _ = c.call_method(Some(BUS_NAME), BUS_PATH, Some(BUS_NAME), "CloseNotification", &(id,));
        }
    });
    true
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
    }

    #[zbus::interface(name = "org.freedesktop.Notifications")]
    impl Daemon {
        #[allow(clippy::too_many_arguments)]
        fn notify(&self, app: String, _replaces: u32, _icon: String, title: String, body: String, actions: Vec<String>, hints: HashMap<String, zbus::zvariant::OwnedValue>, _timeout: i32) -> u32 {
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
    }

    impl Drop for Bus {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    fn bus() -> Bus {
        let mut child = Command::new("dbus-daemon").args(["--session", "--nofork", "--print-address=1"]).stdout(Stdio::piped()).spawn().expect("dbus-daemon");
        let mut addr = String::new();
        BufReader::new(child.stdout.take().unwrap()).read_line(&mut addr).unwrap();
        let addr = addr.trim().to_string();
        // SAFETY: the BUS lock keeps the other tests off the environment while this one is set.
        unsafe { std::env::set_var("DBUS_SESSION_BUS_ADDRESS", &addr) };
        let calls = Arc::new(Mutex::new(Vec::new()));
        let closed = Arc::new(Mutex::new(Vec::new()));
        let conn = zbus::blocking::connection::Builder::address(addr.as_str())
            .unwrap()
            .name(BUS_NAME)
            .unwrap()
            .serve_at(BUS_PATH, Daemon { calls: calls.clone(), closed: closed.clone() })
            .unwrap()
            .build()
            .unwrap();
        Bus { child, conn, calls, closed }
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
        let dir = std::env::temp_dir().join(format!("termurion-nobus-{}", std::process::id()));
        // SAFETY: the BUS lock keeps the other tests off the environment.
        unsafe { std::env::set_var("DBUS_SESSION_BUS_ADDRESS", format!("unix:path={}/none", dir.display())) };
        let t = Instant::now();
        let e = show_with("t", "b", false).unwrap_err();
        assert!(t.elapsed() < Duration::from_secs(6), "took {:?}", t.elapsed());
        assert!(e.contains("notification service"), "{e}");
    }

    #[test]
    fn a_notification_reaches_the_daemon() {
        let _g = BUS.lock().unwrap_or_else(|e| e.into_inner());
        let b = bus();
        let before = shown_count();
        show_with("Build done", "all green", false).expect("shown");
        assert_eq!(shown_count(), before + 1);
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
        let id = *LAST_ID.lock().unwrap();
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
        let id = *LAST_ID.lock().unwrap();
        assert!(!clear_if_shown(at - 1), "a newer show is not ours to clear");
        assert!(clear_if_shown(at));
        wait_for("the close", || b.closed.lock().unwrap().contains(&id));
    }
}
