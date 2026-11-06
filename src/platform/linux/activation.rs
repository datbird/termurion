//! Bringing a window front with an XDG activation token on Wayland (xdg_activation_v1). A
//! compositor raises a window only for a token it handed out for a real user action, such as the
//! notification click whose token the notification server sends (Notifications spec 1.2). winit
//! 0.30 can use a token only for a new window, so the request goes out on winit's own display.

use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{wl_registry, wl_surface::WlSurface};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};
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

/// Asks the compositor to activate `window` with `token`. Ok(false) when the window is not a
/// Wayland one (X11 raises it the usual way); an error names what is missing.
pub fn activate(window: &Window, token: &str) -> Result<bool, String> {
    let (Ok(d), Ok(w)) = (window.display_handle(), window.window_handle()) else { return Ok(false) };
    let (RawDisplayHandle::Wayland(d), RawWindowHandle::Wayland(w)) = (d.as_raw(), w.as_raw()) else { return Ok(false) };
    // SAFETY: winit's display and surface stay alive while `window` is borrowed, and this
    // connection only sends requests on them; its own queue holds only the registry.
    let conn = Connection::from_backend(unsafe { wayland_backend::client::Backend::from_foreign_display(d.display.as_ptr().cast()) });
    let id = unsafe { wayland_backend::client::ObjectId::from_ptr(WlSurface::interface(), w.surface.as_ptr().cast()) }.map_err(|e| format!("The window's Wayland surface could not be used: {e}"))?;
    let surface = WlSurface::from_id(&conn, id).map_err(|e| format!("The window's Wayland surface could not be used: {e}"))?;
    let (globals, queue) = registry_queue_init::<State>(&conn).map_err(|e| format!("The Wayland compositor did not answer: {e}"))?;
    let activation: XdgActivationV1 = globals.bind(&queue.handle(), 1..=1, ()).map_err(|_| "The Wayland compositor has no window activation (xdg_activation_v1)".to_string())?;
    activation.activate(token.to_string(), &surface);
    activation.destroy();
    conn.flush().map_err(|e| format!("The Wayland compositor did not answer: {e}"))?;
    Ok(true)
}
