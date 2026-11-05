//! The hotkey window's native calls on X11: the active window, activating one, the desktop it
//! shows on, opacity, and mapping it in and out. A handle is the X window id (`window::view_of`).
//! The system-wide key grab itself is `unix::global_keys`. On a Wayland session none of this has a
//! handle to act on, so `available` says no and the calls do nothing.

use std::sync::{Arc, Mutex};

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
}

static CONN: Mutex<Option<Arc<Xc>>> = Mutex::new(None);

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
        conn,
        root,
    })
}

/// Runs `f` on the shared connection, made on first use. A failing call drops the connection so the
/// next one starts clean; None when X cannot be reached or the call failed.
fn with_x<T>(f: impl FnOnce(&Xc) -> Result<T, x11rb::errors::ReplyError>) -> Option<T> {
    let xc = {
        let mut g = CONN.lock().unwrap_or_else(|e| e.into_inner());
        match g.as_ref() {
            Some(x) => x.clone(),
            None => {
                let x = Arc::new(connect().ok()?);
                *g = Some(x.clone());
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
            x.conn.unmap_window(w)?;
        } else {
            x.conn.map_window(w)?;
        }
        x.conn.flush()?;
        Ok(())
    });
}

/// Opacity for a compositing window manager; 1.0 removes the property. False when it was not set.
pub fn set_alpha(h: isize, alpha: f32) -> bool {
    let Some(w) = win(h) else { return false };
    with_x(|x| {
        if alpha >= 1.0 {
            x.conn.delete_property(w, x.opacity)?;
        } else {
            let v = (alpha.clamp(0.0, 1.0) as f64 * u32::MAX as f64) as u32;
            x.conn.change_property32(PropMode::REPLACE, w, x.opacity, AtomEnum::CARDINAL, &[v])?;
        }
        x.conn.flush()?;
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
        x.conn.send_event(false, x.root, EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY, ev)?;
        x.conn.flush()?;
        Ok(())
    })
    .is_some()
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

    const F11: u32 = 0xffc8;

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
            rx.recv().unwrap().unwrap_or_else(|e| panic!("these tests need Xvfb, openbox and setxkbmap: {e}"))
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
            let mut ob = Command::new("openbox");
            ob.env("DISPLAY", format!(":{n}")).env("HOME", &home).env("XDG_CONFIG_HOME", &home).arg("--sm-disable").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
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
                    // The server turns Ctrl+Alt+Fn into a VT switch and eats the press; this chord must reach a grab.
                    let st = Command::new("setxkbmap").env("DISPLAY", format!(":{n}")).args(["-option", "srvrkeys:none"]).status().map_err(|e| format!("setxkbmap: {e}"))?;
                    return if st.success() { Ok(format!(":{n}")) } else { Err("setxkbmap failed".into()) };
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

    fn chord(scan: u32) -> Chord {
        Chord { key: ChordKey::Scan(scan), mods: MOD_CONTROL | MOD_ALT }
    }

    fn until(what: &str, mut f: impl FnMut() -> bool) {
        let t = Instant::now();
        while !f() {
            assert!(t.elapsed() < Duration::from_secs(4), "timed out waiting for {what}");
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
        let r = global_keys::set_hotkeys(vec![(7101, chord(0x58))], vec![]).unwrap();
        assert_eq!(r, vec![(7101, Ok(()))]);
        key("ctrl+alt+F12");
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
        let code = keycode(&conn, F11);
        conn.grab_key(false, root, ModMask::CONTROL | ModMask::M1, code, GrabMode::ASYNC, GrabMode::ASYNC).unwrap().check().unwrap();
        let r = global_keys::set_hotkeys(vec![(7102, chord(0x57))], vec![]).unwrap();
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
        until("the second window", || property(&conn, root, b"_NET_ACTIVE_WINDOW").is_some());
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
        let half = property(&conn, w, b"_NET_WM_WINDOW_OPACITY").expect("the property");
        assert!((half as f64 / u32::MAX as f64 - 0.5).abs() < 0.01);
        assert!(set_alpha(w as isize, 1.0));
        assert_eq!(property(&conn, w, b"_NET_WM_WINDOW_OPACITY"), None);
    }

    #[test]
    fn no_x_server_says_so() {
        let _x = xenv();
        // SAFETY: under the lock, restored by `Xenv`.
        unsafe { std::env::set_var("DISPLAY", ":199") };
        let why = global_keys::session_refusal().expect("a refusal");
        assert!(why.contains("X"), "{why}");
        assert!(available().is_err());
        assert!(global_keys::set_hotkeys(vec![(7104, chord(0x58))], vec![]).is_err());
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
}
