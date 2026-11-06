//! Bringing a window front with an XDG activation token on Wayland (xdg_activation_v1). A
//! compositor raises a window only for a token it handed out for a real user action, such as the
//! notification click whose token the notification server sends (Notifications spec 1.2). winit
//! 0.30 can use a token only for a new window, so the request goes out on winit's own display,
//! through an activation global bound there once and kept.

use std::cell::RefCell;

use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{wl_registry, wl_surface::WlSurface};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle};
use wayland_protocols::xdg::activation::v1::client::xdg_activation_v1::{self, XdgActivationV1};
use winit::raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle};
use winit::window::Window;

struct State;

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(_: &mut Self, _: &wl_registry::WlRegistry, _: wl_registry::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<XdgActivationV1, ()> for State {
    fn event(_: &mut Self, _: &XdgActivationV1, _: xdg_activation_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

/// The activation global, bound on winit's display once and kept: its connection, the queue
/// that holds the registry, and the global.
struct Bound {
    conn: Connection,
    queue: EventQueue<State>,
    activation: XdgActivationV1,
}

thread_local! {
    /// Keyed by winit's display, so a click makes no registry and waits on no round trip.
    static BOUND: RefCell<Option<(usize, Bound)>> = const { RefCell::new(None) };
}

/// The value kept in `slot` for `key`, bound by `bind` when there is none for it yet. A failed
/// bind keeps nothing, so the next call tries again.
fn kept<T>(slot: &mut Option<(usize, T)>, key: usize, bind: impl FnOnce() -> Result<T, String>) -> Result<&mut T, String> {
    if slot.as_ref().is_none_or(|(k, _)| *k != key) {
        *slot = None;
        *slot = Some((key, bind()?));
    }
    Ok(&mut slot.as_mut().expect("just bound").1)
}

/// Binds xdg_activation_v1 on winit's display (one registry round trip on its own queue).
fn bind(display: *mut std::ffi::c_void) -> Result<Bound, String> {
    // SAFETY: winit's display lives as long as the app, and this connection only sends requests
    // on it; its own queue holds only the registry and the global.
    let conn = Connection::from_backend(unsafe { wayland_backend::client::Backend::from_foreign_display(display.cast()) });
    let (globals, queue) = registry_queue_init::<State>(&conn).map_err(|e| format!("The Wayland compositor did not answer: {e}"))?;
    let activation: XdgActivationV1 = globals.bind(&queue.handle(), 1..=1, ()).map_err(|_| "The Wayland compositor has no window activation (xdg_activation_v1)".to_string())?;
    Ok(Bound { conn, queue, activation })
}

/// Asks the compositor to activate `window` with `token`. Ok(false) when the window is not a
/// Wayland one (X11 raises it the usual way); an error names what is missing.
pub fn activate(window: &Window, token: &str) -> Result<bool, String> {
    let (Ok(d), Ok(w)) = (window.display_handle(), window.window_handle()) else { return Ok(false) };
    let (RawDisplayHandle::Wayland(d), RawWindowHandle::Wayland(w)) = (d.as_raw(), w.as_raw()) else { return Ok(false) };
    BOUND.with_borrow_mut(|slot| {
        let b = kept(slot, d.display.as_ptr() as usize, || bind(d.display.as_ptr()))?;
        // Registry events that came since (a global added or gone) are taken off the queue.
        let _ = b.queue.dispatch_pending(&mut State);
        // SAFETY: winit's surface stays alive while `window` is borrowed.
        let id = unsafe { wayland_backend::client::ObjectId::from_ptr(WlSurface::interface(), w.surface.as_ptr().cast()) }.map_err(|e| format!("The window's Wayland surface could not be used: {e}"))?;
        let surface = WlSurface::from_id(&b.conn, id).map_err(|e| format!("The window's Wayland surface could not be used: {e}"))?;
        b.activation.activate(token.to_string(), &surface);
        if let Err(e) = b.conn.flush() {
            // A broken link is bound again on the next click.
            *slot = None;
            return Err(format!("The Wayland compositor did not answer: {e}"));
        }
        Ok(true)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// m2: the activation global is bound once per display and kept, so a click makes no new
    /// registry and waits on no round trip; a failed bind is tried again on the next click.
    #[test]
    fn the_activation_global_is_bound_once() {
        let mut slot: Option<(usize, u32)> = None;
        let binds = std::cell::Cell::new(0);
        let bind = || {
            binds.set(binds.get() + 1);
            Ok(binds.get())
        };
        assert_eq!(kept(&mut slot, 7, bind).copied(), Ok(1));
        assert_eq!(kept(&mut slot, 7, bind).copied(), Ok(1));
        assert_eq!(binds.get(), 1, "bound once");
        assert_eq!(kept(&mut slot, 8, bind).copied(), Ok(2), "another display binds its own");
        let mut slot: Option<(usize, u32)> = None;
        assert!(kept(&mut slot, 7, || Err::<u32, _>("no global".to_string())).is_err());
        assert!(slot.is_none());
        assert_eq!(kept(&mut slot, 7, || Ok(9)).copied(), Ok(9), "tried again");
    }
}
