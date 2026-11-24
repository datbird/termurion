//! The X11 desktop's work area (`_NET_WORKAREA`): the strip left once panels and docks have taken
//! their edges, for SetWindowFrame's clamp. Wayland has no such property, and no program places a
//! window there anyway. One connection is kept and the answer is cached, so a program that prints
//! requests in a loop costs no connection and no round trips.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use x11rb::connection::Connection;
use x11rb::protocol::xproto::{AtomEnum, ChangeWindowAttributesAux, ConnectionExt, EventMask};
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;

/// The longest a cached work area is trusted without a change notice.
const MAX_AGE: Duration = Duration::from_secs(1);

/// Whether the cached work area must be read again: a property changed, or it has aged out.
fn needs_refresh(age: Duration, changed: bool) -> bool {
    changed || age >= MAX_AGE
}

struct Cache {
    conn: RustConnection,
    root: u32,
    display: Option<std::ffi::OsString>,
    workarea: u32,
    desktop: u32,
    at: Instant,
    value: Option<(f64, f64, f64, f64)>,
}

static CACHE: Mutex<Option<Cache>> = Mutex::new(None);

fn connect() -> Option<Cache> {
    let (conn, screen) = RustConnection::connect(None).ok()?;
    let root = conn.setup().roots.get(screen)?.root;
    let atom = |name: &str| conn.intern_atom(false, name.as_bytes()).ok()?.reply().ok().map(|r| r.atom);
    let (workarea, desktop) = (atom("_NET_WORKAREA")?, atom("_NET_CURRENT_DESKTOP")?);
    // Hear when either changes, so a panel moving shows at once rather than after the age limit.
    conn.change_window_attributes(root, &ChangeWindowAttributesAux::new().event_mask(EventMask::PROPERTY_CHANGE)).ok()?;
    conn.flush().ok()?;
    let mut c = Cache { conn, root, display: std::env::var_os("DISPLAY"), workarea, desktop, at: Instant::now(), value: None };
    c.value = read(&c);
    Some(c)
}

/// The current desktop's rectangle, read now (four round trips at most).
fn read(c: &Cache) -> Option<(f64, f64, f64, f64)> {
    let cardinals = |atom: u32, count: u32| -> Option<Vec<u32>> {
        let reply = c.conn.get_property(false, c.root, atom, AtomEnum::CARDINAL, 0, count).ok()?.reply().ok()?;
        Some(reply.value32()?.collect())
    };
    // One rectangle per virtual desktop; the current one counts.
    let rects = cardinals(c.workarea, 4 * 64)?;
    let desktop = cardinals(c.desktop, 1).and_then(|d| d.first().copied()).unwrap_or(0) as usize;
    let i = desktop.min((rects.len() / 4).checked_sub(1)?);
    let r = &rects[i * 4..i * 4 + 4];
    (r[2] > 0 && r[3] > 0).then(|| (f64::from(r[0]), f64::from(r[1]), f64::from(r[2]), f64::from(r[3])))
}

/// The current desktop's work area in pixels (x, y, width, height). None without an X server or
/// when the window manager publishes none.
pub fn work_area() -> Option<(f64, f64, f64, f64)> {
    let mut guard = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    // A connection made for another DISPLAY is made again.
    if guard.as_ref().is_some_and(|c| c.display != std::env::var_os("DISPLAY")) {
        *guard = None;
    }
    if guard.is_none() {
        *guard = connect();
    }
    let c = guard.as_mut()?;
    let mut changed = false;
    loop {
        match c.conn.poll_for_event() {
            Ok(Some(Event::PropertyNotify(p))) => changed |= p.atom == c.workarea || p.atom == c.desktop,
            Ok(Some(_)) => {}
            Ok(None) => break,
            // The server went away: forget the connection, and the next request makes a new one.
            Err(_) => {
                *guard = None;
                return None;
            }
        }
    }
    if needs_refresh(c.at.elapsed(), changed) {
        c.value = read(c);
        c.at = Instant::now();
    }
    c.value
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_work_area_is_read_again_only_when_it_changed_or_aged() {
        assert!(!needs_refresh(Duration::from_millis(10), false), "a request in a loop reuses the answer");
        assert!(!needs_refresh(Duration::from_millis(999), false));
        assert!(needs_refresh(Duration::from_secs(1), false), "at most once a second");
        assert!(needs_refresh(Duration::from_millis(10), true), "a panel that moved shows at once");
    }
}
