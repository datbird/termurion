//! The hotkey window's native calls on X11: the active window, activating one, the desktop it
//! shows on, opacity, and mapping it in and out. A handle is the X window id (`window::view_of`).
//! The system-wide key grab itself is `unix::global_keys`. On a Wayland session none of this has a
//! handle to act on, so `available` says no and the calls do nothing.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use x11rb::connection::Connection;
use x11rb::protocol::xproto::{AtomEnum, ClientMessageEvent, ConnectionExt, EventMask, MapState, PropMode};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;
use x11rb::CURRENT_TIME;

use crate::platform::unix::global_keys::{session_kind, SessionKind};
use crate::platform::Unsupported;

const ALL_DESKTOPS: u32 = 0xFFFF_FFFF;

struct Xc {
    conn: RustConnection,
    root: u32,
    active: u32,
    current_desktop: u32,
    wm_desktop: u32,
    opacity: u32,
    pid: u32,
    user_time: u32,
}

/// The shared connection, with the DISPLAY it was made for.
static CONN: Mutex<Option<(Option<std::ffi::OsString>, Arc<Xc>)>> = Mutex::new(None);

fn connect() -> Result<Xc, String> {
    let (conn, screen) = RustConnection::connect(None).map_err(|e| e.to_string())?;
    let root = conn.setup().roots[screen].root;
    let atom = |name: &str| -> Result<u32, String> {
        let c = conn.intern_atom(false, name.as_bytes()).map_err(|e| e.to_string())?;
        Ok(c.reply().map_err(|e| e.to_string())?.atom)
    };
    Ok(Xc {
        active: atom("_NET_ACTIVE_WINDOW")?,
        current_desktop: atom("_NET_CURRENT_DESKTOP")?,
        wm_desktop: atom("_NET_WM_DESKTOP")?,
        opacity: atom("_NET_WM_WINDOW_OPACITY")?,
        pid: atom("_NET_WM_PID")?,
        user_time: atom("_NET_WM_USER_TIME")?,
        conn,
        root,
    })
}

/// Runs `f` on the shared connection, made on first use (and again when DISPLAY changes). A failing call drops the connection so the
/// next one starts clean; None when X cannot be reached or the call failed.
fn with_x<T>(f: impl FnOnce(&Xc) -> Result<T, x11rb::errors::ReplyError>) -> Option<T> {
    let xc = {
        let mut g = CONN.lock().unwrap_or_else(|e| e.into_inner());
        let display = std::env::var_os("DISPLAY");
        match g.as_ref() {
            Some((d, x)) if *d == display => x.clone(),
            _ => {
                let x = Arc::new(connect().ok()?);
                *g = Some((display, x.clone()));
                x
            }
        }
    };
    match f(&xc) {
        Ok(v) => Some(v),
        Err(x11rb::errors::ReplyError::X11Error(_)) => None,
        Err(_) => {
            *CONN.lock().unwrap_or_else(|e| e.into_inner()) = None;
            None
        }
    }
}

fn win(h: isize) -> Option<u32> {
    u32::try_from(h).ok().filter(|w| *w != 0 && session_kind() != SessionKind::Wayland)
}

fn first_u32(x: &Xc, w: u32, prop: u32, ty: AtomEnum) -> Result<Option<u32>, x11rb::errors::ReplyError> {
    let r = x.conn.get_property(false, w, prop, ty, 0, 1)?.reply()?;
    Ok(r.value32().and_then(|mut v| v.next()))
}

/// Why the grab cannot work, or None when an X server answers. Connects fresh each time.
pub fn x_server_refusal() -> Option<String> {
    match RustConnection::connect(None) {
        Ok(_) => None,
        Err(e) if std::env::var_os("DISPLAY").is_none() => Some(format!("System-wide hotkeys need X11, and no X display is set ({e})")),
        Err(e) => Some(format!("System-wide hotkeys could not reach the X server: {e}")),
    }
}

/// Ok on X11; an error naming Wayland on a Wayland session, one naming X11 when no server answers.
pub fn available() -> Result<(), Unsupported> {
    if session_kind() == SessionKind::Wayland {
        return Err(Unsupported { feature: "The hotkey window on Wayland", plural: false });
    }
    match x_server_refusal() {
        None => Ok(()),
        Some(_) => Err(Unsupported { feature: "The hotkey window without X11", plural: false }),
    }
}

/// Hides the window by unmapping it, or shows it by mapping it.
pub fn cloak(h: isize, on: bool) {
    let Some(w) = win(h) else { return };
    with_x(|x| {
        if on {
            x.conn.unmap_window(w)?.check()?;
        } else {
            x.conn.map_window(w)?.check()?;
        }
        Ok(())
    });
}

/// Opacity for a compositing window manager; 1.0 removes the property. False when it was not set.
pub fn set_alpha(h: isize, alpha: f32) -> bool {
    let Some(w) = win(h) else { return false };
    with_x(|x| {
        if alpha >= 1.0 {
            x.conn.delete_property(w, x.opacity)?.check()?;
        } else {
            let v = (alpha.clamp(0.0, 1.0) as f64 * u32::MAX as f64) as u32;
            x.conn.change_property32(PropMode::REPLACE, w, x.opacity, AtomEnum::CARDINAL, &[v])?.check()?;
        }
        Ok(())
    })
    .is_some()
}

/// The window the window manager calls active, or 0.
pub fn foreground() -> isize {
    if session_kind() == SessionKind::Wayland {
        return 0;
    }
    with_x(|x| first_u32(x, x.root, x.active, AtomEnum::WINDOW)).flatten().unwrap_or(0) as isize
}

/// Whether the window belongs to this process (its `_NET_WM_PID`).
pub fn ours(h: isize) -> bool {
    let Some(w) = win(h) else { return false };
    with_x(|x| first_u32(x, w, x.pid, AtomEnum::CARDINAL)).flatten() == Some(std::process::id())
}

/// Whether the window still exists on the server.
pub fn is_window(h: isize) -> bool {
    let Some(w) = win(h) else { return false };
    with_x(|x| Ok(x.conn.get_window_attributes(w)?.reply().is_ok())).unwrap_or(false)
}

/// Asks the window manager to make the window active, as a pager does, so focus stealing rules
/// let it through. A minimized window is restored by the same request.
pub fn order_front(h: isize, _minimized: bool) {
    activate(h);
}

/// The request behind `order_front`; false when it could not be sent.
pub fn activate(h: isize) -> bool {
    let Some(w) = win(h) else { return false };
    with_x(|x| {
        let ev = ClientMessageEvent::new(32, w, x.active, [2, CURRENT_TIME, 0, 0, 0]);
        x.conn.send_event(false, x.root, EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY, ev)?.check()?;
        Ok(())
    })
    .is_some()
}

/// After the show: waits (bounded) for the window to be mapped, then asks the window manager to
/// activate it as a pager, with a real server timestamp that is also its `_NET_WM_USER_TIME`. winit's
/// own request can come before the map, and as an application it is only "demand attention" to
/// some window managers. The waits run on their own thread, never the UI's; one at a time, so two
/// quick shows do not take each other's timestamp.
pub fn take_focus(h: isize) {
    static ONE: Mutex<()> = Mutex::new(());
    let Some(w) = win(h) else { return };
    let _ = std::thread::Builder::new().name("hotkey-focus".into()).spawn(move || {
        let _one = ONE.lock().unwrap_or_else(|e| e.into_inner());
        focus_when_mapped(w);
    });
}

fn focus_when_mapped(w: u32) {
    with_x(|x| {
        let t = Instant::now();
        while t.elapsed() < Duration::from_millis(500) {
            if x.conn.get_window_attributes(w)?.reply()?.map_state == MapState::VIEWABLE {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        let time = server_time(x, w).unwrap_or_else(|| {
            crate::dlog!("The dropdown window's server time did not come; its focus request goes with CURRENT_TIME");
            CURRENT_TIME
        });
        x.conn.change_property32(PropMode::REPLACE, w, x.user_time, AtomEnum::CARDINAL, &[time])?;
        let ev = ClientMessageEvent::new(32, w, x.active, [2, time, 0, 0, 0]);
        x.conn.send_event(false, x.root, EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY, ev)?.check()?;
        Ok(())
    });
}

/// The server's clock now: an empty property append on the window and the time of its notify.
fn server_time(x: &Xc, w: u32) -> Option<u32> {
    use x11rb::protocol::xproto::ChangeWindowAttributesAux;
    use x11rb::protocol::Event;
    x.conn.change_window_attributes(w, &ChangeWindowAttributesAux::new().event_mask(EventMask::PROPERTY_CHANGE)).ok()?;
    x.conn.change_property32(PropMode::APPEND, w, x.user_time, AtomEnum::CARDINAL, &[]).ok()?;
    x.conn.flush().ok()?;
    let t = Instant::now();
    let mut found = None;
    while found.is_none() && t.elapsed() < Duration::from_millis(200) {
        match x.conn.poll_for_event() {
            Ok(Some(Event::PropertyNotify(e))) if e.window == w && e.atom == x.user_time => found = Some(e.time),
            Ok(Some(_)) => {}
            Ok(None) => std::thread::sleep(Duration::from_millis(2)),
            Err(_) => break,
        }
    }
    let _ = x.conn.change_window_attributes(w, &ChangeWindowAttributesAux::new().event_mask(EventMask::NO_EVENT));
    let _ = x.conn.flush();
    found
}

/// Whether the current keymap turns Ctrl+Alt+F`n` into a console switch (the server eats that press):
/// read from the keymap, where the F key's keycode carries an XF86Switch_VT_`n` keysym.
pub fn switches_console(n: u8) -> bool {
    const F1: u32 = 0xffbe;
    const VT1: u32 = 0x1008_fe01;
    if !(1..=12).contains(&n) {
        return false;
    }
    with_x(|x| {
        let s = x.conn.setup();
        let m = x.conn.get_keyboard_mapping(s.min_keycode, s.max_keycode - s.min_keycode + 1)?.reply()?;
        let per = m.keysyms_per_keycode as usize;
        let want = VT1 + (n as u32 - 1);
        Ok(m.keysyms.chunks(per).any(|k| k.contains(&(F1 + n as u32 - 1)) && k.contains(&want)))
    })
    .unwrap_or(false)
}

/// Puts the window on the desktop the person is on, unless it is on all of them.
pub fn join_active_space(h: isize) {
    let Some(w) = win(h) else { return };
    with_x(|x| {
        let Some(now) = first_u32(x, x.root, x.current_desktop, AtomEnum::CARDINAL)? else { return Ok(()) };
        if first_u32(x, w, x.wm_desktop, AtomEnum::CARDINAL)? == Some(ALL_DESKTOPS) {
            return Ok(());
        }
        // A mapped window moves by asking the manager; an unmapped one carries the property in.
        if x.conn.get_window_attributes(w)?.reply()?.map_state == MapState::VIEWABLE {
            let ev = ClientMessageEvent::new(32, w, x.wm_desktop, [now, 2, 0, 0, 0]);
            x.conn.send_event(false, x.root, EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY, ev)?;
        } else {
            x.conn.change_property32(PropMode::REPLACE, w, x.wm_desktop, AtomEnum::CARDINAL, &[now])?;
        }
        x.conn.flush()?;
        Ok(())
    });
}

/// No per-window state is kept here: the server holds it all.
pub fn forget(_h: isize) {}


#[cfg(test)]
mod tests {
    use std::process::{Command, Stdio};
    use std::sync::{mpsc, MutexGuard, OnceLock};
    use std::time::{Duration, Instant};

    use x11rb::protocol::xproto::{CreateWindowAux, GrabMode, ModMask, WindowClass};

    use super::*;
    use crate::hotkey::{Chord, ChordKey, MOD_ALT, MOD_CONTROL};
    use crate::platform::unix::global_keys;

    const KEY_K: u32 = 0x6b;

    fn kill_with_parent(cmd: &mut Command) {
        use std::os::unix::process::CommandExt;
        // SAFETY: prctl is async-signal-safe; it only arms the death signal.
        unsafe {
            cmd.pre_exec(|| {
                libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL);
                Ok(())
            });
        }
    }

    /// A private Xvfb with openbox on a free display, started once. A thread that never ends owns the
    /// children, so the kernel kills them when the test run exits.
    fn display() -> &'static str {
        static D: OnceLock<String> = OnceLock::new();
        D.get_or_init(|| {
            let (tx, rx) = mpsc::channel();
            std::thread::spawn(move || {
                let r = start_x();
                let ok = r.is_ok();
                let _ = tx.send(r);
                while ok {
                    std::thread::park();
                }
            });
            rx.recv().unwrap().unwrap_or_else(|e| panic!("these tests need Xvfb and openbox: {e}"))
        })
    }

    fn start_x() -> Result<String, String> {
        for n in 140..190 {
            let (sock, lock) = (format!("/tmp/.X11-unix/X{n}"), format!("/tmp/.X{n}-lock"));
            // A lock naming a live process is a server in use; a dead one is left from a killed run, and Xvfb clears it.
            let in_use = std::fs::read_to_string(&lock).ok().and_then(|t| t.trim().parse::<u32>().ok()).is_some_and(|pid| std::path::Path::new(&format!("/proc/{pid}")).exists());
            if in_use {
                continue;
            }
            let _ = std::fs::remove_file(&sock);
            let mut xvfb = Command::new("Xvfb");
            xvfb.args([&format!(":{n}"), "-screen", "0", "1024x768x24", "-nolisten", "tcp"]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
            kill_with_parent(&mut xvfb);
            let mut child = xvfb.spawn().map_err(|e| format!("Xvfb: {e}"))?;
            let up = Instant::now();
            while !std::path::Path::new(&sock).exists() && up.elapsed() < Duration::from_secs(5) {
                std::thread::sleep(Duration::from_millis(50));
            }
            if child.try_wait().ok().flatten().is_some() || !std::path::Path::new(&sock).exists() {
                continue;
            }
            let home = std::env::temp_dir().join(format!("termurion-openbox-{n}"));
            let _ = std::fs::create_dir_all(&home);
            // New windows are not focused by the manager, as on a desktop with focus stealing prevention.
            let rc = home.join("rc.xml");
            std::fs::write(&rc, "<?xml version=\"1.0\"?>\n<openbox_config xmlns=\"http://openbox.org/3.4/rc\"><focus><focusNew>no</focusNew></focus></openbox_config>\n").map_err(|e| e.to_string())?;
            let mut ob = Command::new("openbox");
            ob.env("DISPLAY", format!(":{n}")).env("HOME", &home).env("XDG_CONFIG_HOME", &home).arg("--sm-disable").arg("--config-file").arg(&rc).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
            kill_with_parent(&mut ob);
            let wm = ob.spawn().map_err(|e| format!("openbox: {e}"))?;
            std::mem::forget(wm);
            std::mem::forget(child);
            let (conn, screen) = RustConnection::connect(Some(&format!(":{n}"))).map_err(|e| e.to_string())?;
            let root = conn.setup().roots[screen].root;
            let check = conn.intern_atom(false, b"_NET_SUPPORTING_WM_CHECK").map_err(|e| e.to_string())?.reply().map_err(|e| e.to_string())?.atom;
            let t = Instant::now();
            loop {
                let r = conn.get_property(false, root, check, AtomEnum::WINDOW, 0, 1).map_err(|e| e.to_string())?.reply().map_err(|e| e.to_string())?;
                if r.value_len > 0 {
                    // The check property comes up before the manager serves windows: a window mapped now is never managed.
                    // The client list appears once its startup scan is done.
                    let list = conn.intern_atom(false, b"_NET_CLIENT_LIST").map_err(|e| e.to_string())?.reply().map_err(|e| e.to_string())?.atom;
                    let t = Instant::now();
                    loop {
                        let r = conn.get_property(false, root, list, AtomEnum::WINDOW, 0, 1).map_err(|e| e.to_string())?.reply().map_err(|e| e.to_string())?;
                        if r.type_ != u32::from(AtomEnum::NONE) {
                            break;
                        }
                        if t.elapsed() > Duration::from_secs(20) {
                            return Err("openbox did not finish starting".into());
                        }
                        std::thread::sleep(Duration::from_millis(50));
                    }
                    return Ok(format!(":{n}"));
                }
                if t.elapsed() > Duration::from_secs(5) {
                    return Err("openbox did not come up".into());
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }
        Err("no free display".into())
    }

    static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());
    static HITS: Mutex<Vec<u32>> = Mutex::new(Vec::new());

    /// Holds the lock on the shared server and points DISPLAY at it for the test.
    struct Xenv {
        _lock: MutexGuard<'static, ()>,
        old: Option<std::ffi::OsString>,
    }

    fn xenv() -> Xenv {
        let d = display();
        let lock = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
        let old = std::env::var_os("DISPLAY");
        // SAFETY: the lock keeps the X tests from racing each other over the variable.
        unsafe { std::env::set_var("DISPLAY", d) };
        global_keys::force_session(Some(SessionKind::X11));
        global_keys::reset_main_thread();
        global_keys::set_hotkey_sink(Box::new(|ids| HITS.lock().unwrap_or_else(|e| e.into_inner()).extend(ids)));
        HITS.lock().unwrap_or_else(|e| e.into_inner()).clear();
        Xenv { _lock: lock, old }
    }

    impl Drop for Xenv {
        fn drop(&mut self) {
            // SAFETY: as in `xenv`.
            unsafe {
                match &self.old {
                    Some(v) => std::env::set_var("DISPLAY", v),
                    None => std::env::remove_var("DISPLAY"),
                }
            }
            global_keys::force_session(None);
        }
    }

    const SPACE: u32 = 0x39;
    const K: u32 = 0x25;

    fn chord(scan: u32) -> Chord {
        Chord { key: ChordKey::Scan(scan), mods: MOD_CONTROL | MOD_ALT }
    }

    fn until(what: &str, mut f: impl FnMut() -> bool) {
        let t = Instant::now();
        while !f() {
            assert!(t.elapsed() < Duration::from_secs(10), "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(40));
        }
    }

    fn key(chord: &str) {
        let st = Command::new("xdotool").args(["key", chord]).status().expect("xdotool");
        assert!(st.success());
    }

    fn keycode(conn: &RustConnection, sym: u32) -> u8 {
        let s = conn.setup();
        let m = conn.get_keyboard_mapping(s.min_keycode, s.max_keycode - s.min_keycode + 1).unwrap().reply().unwrap();
        let per = m.keysyms_per_keycode as usize;
        m.keysyms.chunks(per).position(|c| c.contains(&sym)).map(|i| s.min_keycode + i as u8).expect("a keycode")
    }

    /// A plain top-level window that says whose it is, as winit's do.
    fn make_window(conn: &RustConnection, root: u32) -> u32 {
        let id = conn.generate_id().unwrap();
        conn.create_window(0, id, root, 10, 10, 200, 100, 0, WindowClass::INPUT_OUTPUT, 0, &CreateWindowAux::new()).unwrap();
        let pid = conn.intern_atom(false, b"_NET_WM_PID").unwrap().reply().unwrap().atom;
        conn.change_property32(PropMode::REPLACE, id, pid, AtomEnum::CARDINAL, &[std::process::id()]).unwrap();
        conn.map_window(id).unwrap();
        conn.flush().unwrap();
        id
    }

    fn property(conn: &RustConnection, w: u32, name: &[u8]) -> Option<u32> {
        let a = conn.intern_atom(false, name).unwrap().reply().unwrap().atom;
        conn.get_property(false, w, a, AtomEnum::ANY, 0, 1).unwrap().reply().unwrap().value32().and_then(|mut v| v.next())
    }

    #[test]
    fn a_registered_chord_fires() {
        let _x = xenv();
        let r = global_keys::set_hotkeys(vec![(7101, chord(SPACE))], vec![]).unwrap();
        assert_eq!(r, vec![(7101, Ok(()))]);
        key("ctrl+alt+space");
        until("the press", || HITS.lock().unwrap().contains(&7101));
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(*HITS.lock().unwrap(), vec![7101], "one press, one id");
        global_keys::set_hotkeys(vec![], vec![]).unwrap();
    }

    #[test]
    fn a_taken_chord_says_so() {
        let _x = xenv();
        let (conn, screen) = RustConnection::connect(None).unwrap();
        let root = conn.setup().roots[screen].root;
        let code = keycode(&conn, KEY_K);
        conn.grab_key(false, root, ModMask::CONTROL | ModMask::M1, code, GrabMode::ASYNC, GrabMode::ASYNC).unwrap().check().unwrap();
        let r = global_keys::set_hotkeys(vec![(7102, chord(K))], vec![]).unwrap();
        assert_eq!(r, vec![(7102, Err("Another app already uses this shortcut".to_string()))]);
        global_keys::set_hotkeys(vec![], vec![]).unwrap();
    }

    #[test]
    fn taps_are_refused_on_linux() {
        let _x = xenv();
        let r = global_keys::set_hotkeys(vec![], vec![(7103, 1)]).unwrap();
        assert_eq!(r.len(), 1);
        let why = r[0].1.as_ref().unwrap_err();
        assert!(why.contains("Double-tap") && why.contains("Linux"), "{why}");
    }

    #[test]
    fn order_front_makes_our_window_active() {
        let _x = xenv();
        let (conn, screen) = RustConnection::connect(None).unwrap();
        let root = conn.setup().roots[screen].root;
        let (a, b) = (make_window(&conn, root), make_window(&conn, root));
        until("the second window managed", || property(&conn, b, b"_NET_WM_DESKTOP").is_some());
        order_front(a as isize, false);
        until("the first window active", || property(&conn, root, b"_NET_ACTIVE_WINDOW") == Some(a));
        assert_eq!(foreground(), a as isize);
        order_front(b as isize, false);
        until("the second window active", || foreground() == b as isize);
        assert!(ours(a as isize) && ours(b as isize));
        assert!(!ours(root as isize), "the root belongs to no app of ours");
        assert!(is_window(a as isize) && !is_window(0x7fff_fff0));
    }

    #[test]
    fn a_window_joins_the_desktop_the_person_is_on() {
        let _x = xenv();
        let (conn, screen) = RustConnection::connect(None).unwrap();
        let root = conn.setup().roots[screen].root;
        let w = make_window(&conn, root);
        until("the window managed", || property(&conn, w, b"_NET_WM_DESKTOP").is_some());
        let cur = conn.intern_atom(false, b"_NET_CURRENT_DESKTOP").unwrap().reply().unwrap().atom;
        let ev = ClientMessageEvent::new(32, root, cur, [2, CURRENT_TIME, 0, 0, 0]);
        conn.send_event(false, root, EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY, ev).unwrap();
        conn.flush().unwrap();
        until("desktop 2", || property(&conn, root, b"_NET_CURRENT_DESKTOP") == Some(2));
        join_active_space(w as isize);
        until("the window on desktop 2", || property(&conn, w, b"_NET_WM_DESKTOP") == Some(2));
    }

    #[test]
    fn cloak_unmaps_and_opacity_is_set() {
        let _x = xenv();
        let (conn, screen) = RustConnection::connect(None).unwrap();
        let root = conn.setup().roots[screen].root;
        let w = make_window(&conn, root);
        let mapped = || conn.get_window_attributes(w).unwrap().reply().unwrap().map_state == MapState::VIEWABLE;
        until("the window mapped", mapped);
        cloak(w as isize, true);
        until("the window unmapped", || !mapped());
        cloak(w as isize, false);
        until("the window mapped again", mapped);
        assert!(set_alpha(w as isize, 0.5));
        let mut half = None;
        until("the opacity", || {
            half = property(&conn, w, b"_NET_WM_WINDOW_OPACITY");
            half.is_some()
        });
        assert!((half.unwrap() as f64 / u32::MAX as f64 - 0.5).abs() < 0.01);
        assert!(set_alpha(w as isize, 1.0));
        until("the opacity gone", || property(&conn, w, b"_NET_WM_WINDOW_OPACITY").is_none());
    }

    #[test]
    fn no_x_server_says_so() {
        let _x = xenv();
        // SAFETY: under the lock, restored by `Xenv`.
        unsafe { std::env::set_var("DISPLAY", ":199") };
        let why = global_keys::session_refusal().expect("a refusal");
        assert!(why.contains("X"), "{why}");
        assert!(available().is_err());
        assert!(global_keys::set_hotkeys(vec![(7104, chord(SPACE))], vec![]).is_err());
    }

    #[test]
    fn wayland_has_no_hotkey_window() {
        let _x = xenv();
        global_keys::force_session(Some(SessionKind::Wayland));
        let e = available().unwrap_err().to_string();
        assert!(e.contains("Wayland"), "{e}");
        assert_eq!(foreground(), 0);
        assert!(!ours(5) && !is_window(5));
    }

    #[test]
    fn showing_takes_focus_when_the_manager_does_not_give_it() {
        let _x = xenv();
        let (conn, screen) = RustConnection::connect(None).unwrap();
        let root = conn.setup().roots[screen].root;
        let other = make_window(&conn, root);
        until("the other window managed", || property(&conn, other, b"_NET_WM_DESKTOP").is_some());
        order_front(other as isize, false);
        until("the other window active", || foreground() == other as isize);
        let w = make_window(&conn, root);
        until("the new window managed", || property(&conn, w, b"_NET_WM_DESKTOP").is_some());
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(foreground(), other as isize, "the manager must not focus a new window in this setup");
        cloak(w as isize, true);
        cloak(w as isize, false);
        take_focus(w as isize);
        until("the shown window active", || foreground() == w as isize);
    }

    /// The map wait and the server time poll run off the caller (the UI thread): a window that
    /// never maps costs the caller nothing.
    #[test]
    fn taking_focus_never_holds_the_caller() {
        let _x = xenv();
        let (conn, screen) = RustConnection::connect(None).unwrap();
        let root = conn.setup().roots[screen].root;
        let w = make_window(&conn, root);
        cloak(w as isize, true);
        let t = Instant::now();
        take_focus(w as isize);
        assert!(t.elapsed() < Duration::from_millis(50), "take_focus held the caller for {:?}", t.elapsed());
        // Let the worker finish before the next test uses the server.
        std::thread::sleep(Duration::from_millis(800));
    }

    #[test]
    fn a_console_switch_chord_is_refused() {
        let _x = xenv();
        assert!(switches_console(12), "the stock keymap has the console keysyms");
        let r = global_keys::set_hotkeys(vec![(7105, chord(0x58))], vec![]).unwrap();
        assert_eq!(r, vec![(7105, Err("Ctrl+Alt+F12 switches to a text console on this system; pick another shortcut".to_string()))]);
        // The keymap ignores Super for this switch, so the chord with Super is refused too.
        let sup = Chord { key: ChordKey::Scan(0x58), mods: MOD_CONTROL | MOD_ALT | crate::hotkey::MOD_WIN };
        let r = global_keys::set_hotkeys(vec![(7107, sup)], vec![]).unwrap();
        assert!(matches!(&r[..], [(7107, Err(e))] if e.contains("switches to a text console")), "{r:?}");
        let plain = Chord { key: ChordKey::Scan(0x58), mods: MOD_ALT };
        let r = global_keys::set_hotkeys(vec![(7106, plain)], vec![]).unwrap();
        assert_eq!(r, vec![(7106, Ok(()))], "Alt+F12 alone is no console switch");
        global_keys::set_hotkeys(vec![], vec![]).unwrap();
    }

    #[test]
    fn an_apostrophe_chord_is_refused_in_plain_words() {
        let _x = xenv();
        let c = Chord { key: ChordKey::Char('\''), mods: MOD_CONTROL | MOD_ALT };
        let r = global_keys::set_hotkeys(vec![(7107, c)], vec![]).unwrap();
        let why = r[0].1.as_ref().unwrap_err();
        assert!(why.contains("apostrophe") && !why.contains("keycode"), "{why}");
    }

    #[test]
    fn a_dead_worker_is_found() {
        let _x = xenv();
        let live = global_hotkey::GlobalHotKeyManager::new().unwrap();
        assert!(global_keys::worker_alive(&live));
        // SAFETY: under the lock, restored by `Xenv`.
        unsafe { std::env::set_var("DISPLAY", ":199") };
        let dead = global_hotkey::GlobalHotKeyManager::new().unwrap();
        std::thread::sleep(Duration::from_millis(300));
        assert!(!global_keys::worker_alive(&dead), "a worker with no display must not pass for a live one");
    }

    #[test]
    fn hotkeys_are_set_from_the_main_thread_only() {
        let _x = xenv();
        assert!(global_keys::set_hotkeys(vec![], vec![]).is_ok());
        let from_elsewhere = std::thread::spawn(|| global_keys::set_hotkeys(vec![], vec![])).join().unwrap();
        assert!(from_elsewhere.unwrap_err().contains("main thread"));
        global_keys::set_hotkeys(vec![], vec![]).unwrap();
    }

    #[test]
    fn the_connection_follows_display() {
        let _x = xenv();
        let (conn, screen) = RustConnection::connect(None).unwrap();
        let root = conn.setup().roots[screen].root;
        let w = make_window(&conn, root);
        assert!(ours(w as isize));
        // SAFETY: under the lock, restored by `Xenv`.
        unsafe { std::env::set_var("DISPLAY", ":199") };
        assert!(!ours(w as isize), "a cached connection to the old display answered");
    }
}
