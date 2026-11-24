//! The X11 desktop's work area (`_NET_WORKAREA`): the strip left once panels and docks have taken
//! their edges, for SetWindowFrame's clamp. Wayland has no such property, and no program places a
//! window there anyway.

use x11rb::connection::Connection;
use x11rb::protocol::xproto::{AtomEnum, ConnectionExt};
use x11rb::rust_connection::RustConnection;

/// The current desktop's work area in pixels (x, y, width, height). None without an X server or
/// when the window manager publishes none.
pub fn work_area() -> Option<(f64, f64, f64, f64)> {
    let (conn, screen) = RustConnection::connect(None).ok()?;
    let root = conn.setup().roots.get(screen)?.root;
    let atom = |name: &str| conn.intern_atom(false, name.as_bytes()).ok()?.reply().ok().map(|r| r.atom);
    let cardinals = |name: &str, count: u32| -> Option<Vec<u32>> {
        let reply = conn.get_property(false, root, atom(name)?, AtomEnum::CARDINAL, 0, count).ok()?.reply().ok()?;
        Some(reply.value32()?.collect())
    };
    // One rectangle per virtual desktop; the current one counts.
    let rects = cardinals("_NET_WORKAREA", 4 * 64)?;
    let desktop = cardinals("_NET_CURRENT_DESKTOP", 1).and_then(|d| d.first().copied()).unwrap_or(0) as usize;
    let i = desktop.min((rects.len() / 4).checked_sub(1)?);
    let r = &rects[i * 4..i * 4 + 4];
    (r[2] > 0 && r[3] > 0).then(|| (f64::from(r[0]), f64::from(r[1]), f64::from(r[2]), f64::from(r[3])))
}
