//! Window chrome on Unix: Linux in full, plus the no-ops macOS shares. The window manager draws
//! border and corners. On Linux the menu bar is drawn inside the window (`menu_overlay`); this
//! file keeps which windows have one and whether it is shown, and the pop-ups waiting to be drawn.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::platform::Unsupported;
use crate::platform::chrome::QueuedPopup;

/// The bar is a strip inside the window that the app draws (Linux), not the system's. macOS
/// shares this file only for the no-ops below and has its own bar.
pub const IN_WINDOW_BAR: bool = true;

/// Linux draws its menus itself and always can.
pub fn available() -> Result<(), Unsupported> {
    Ok(())
}

/// One window's bar: put on it, and taken off again by Hide Menu Bar.
#[derive(Clone, Copy)]
struct Bar {
    hidden: bool,
}

static BARS: Mutex<Option<HashMap<isize, Bar>>> = Mutex::new(None);

fn with_bars<R>(f: impl FnOnce(&mut HashMap<isize, Bar>) -> R) -> R {
    let mut g = BARS.lock().unwrap_or_else(|e| e.into_inner());
    f(g.get_or_insert_with(HashMap::new))
}

thread_local! {
    /// Menus are the UI thread's (muda items are not Send), so is their queue.
    static QUEUED: RefCell<HashMap<isize, QueuedPopup>> = RefCell::new(HashMap::new());
    static WAKE: RefCell<HashMap<isize, Arc<winit::window::Window>>> = RefCell::new(HashMap::new());
}

/// Takes the pop-up queued for this window, if any.
pub fn take_popup(view: isize) -> Option<QueuedPopup> {
    QUEUED.with(|q| q.borrow_mut().remove(&view))
}

/// Remembers the window so a queued pop-up can ask it for a frame.
pub fn bind_window(view: isize, window: Arc<winit::window::Window>) {
    WAKE.with(|w| w.borrow_mut().insert(view, window));
}

/// The window is gone: its bar, queue and wake handle go too.
pub fn forget_window(view: isize) {
    with_bars(|b| b.remove(&view));
    QUEUED.with(|q| q.borrow_mut().remove(&view));
    WAKE.with(|w| w.borrow_mut().remove(&view));
}

/// Linux menus are the toolkit's own and show no native shortcuts yet.
pub const NATIVE_ACCELERATORS: bool = false;

/// Linux keeps no screen-wide bar to hand over.
pub fn bar_focused(_menu: &muda::Menu) {}

/// Linux has no system Help menu to adopt.
pub fn adopt_help_menu(_sub: &muda::Submenu) {}

/// Linux has no Services menu.
pub fn services_item() -> Option<muda::PredefinedMenuItem> {
    None
}

/// Linux keys do not pass a menu first; nothing to claim.
pub fn claim_key_equivalents(_view: isize, _route: std::sync::Arc<std::sync::RwLock<crate::keyroute::Snapshot>>) -> Result<(), String> {
    Ok(())
}

/// Nothing was claimed on Linux, so nothing to forget.
pub fn forget_key_equivalents(_view: isize) {}

pub fn set_border(_hwnd: isize, _color: u32) {}

pub fn set_corners(_hwnd: isize, _preference: i32) {}

pub fn invisible_top_bottom(_hwnd: isize) -> (f32, f32) {
    (0.0, 0.0)
}

pub fn hide_menu(_menu: &muda::Menu, view: isize) -> Result<(), String> {
    with_bars(|b| b.get_mut(&view).map(|bar| bar.hidden = true)).ok_or_else(|| "this window has no menu bar".to_string())
}

pub fn show_menu(_menu: &muda::Menu, view: isize) -> Result<(), String> {
    with_bars(|b| b.get_mut(&view).map(|bar| bar.hidden = false)).ok_or_else(|| "this window has no menu bar".to_string())
}

/// The window has a bar and shows it.
pub fn menu_attached(view: isize) -> bool {
    with_bars(|b| b.get(&view).is_some_and(|bar| !bar.hidden))
}

pub fn apply_menu_theme(_hwnd: isize, _forced: Option<bool>, _high_contrast: bool, _bar: Option<&muda::Menu>) -> Result<(), String> {
    Ok(())
}

/// No subclass: winit tells the app about a theme change itself.
pub fn guard_theme(_hwnd: isize, _wake: Box<dyn Fn()>) {}

pub fn theme_reset_handled() {}

pub fn shift_down() -> bool {
    false
}

/// Menus open where the toolkit puts them; there is nothing to lift.
pub struct Lift;

impl Lift {
    pub fn new(_hwnd: isize, _x: f32, _y: f32) -> Lift {
        Lift
    }
}

/// Queues the menu for the window's next frame, where the overlay draws it (Linux). True: it was
/// queued; the pick comes back as a menu event, not here. macOS pop-ups are `platform::macos`.
pub fn popup<M: crate::platform::chrome::PopupMenu + ?Sized>(menu: &M, view: isize, position: Option<muda::dpi::Position>) -> bool {
    let at = position.map(|p| match p {
        muda::dpi::Position::Physical(p) => (p.x as f32, p.y as f32),
        muda::dpi::Position::Logical(p) => (p.x as f32, p.y as f32),
    });
    QUEUED.with(|q| q.borrow_mut().insert(view, QueuedPopup { items: menu.entries(), at }));
    if let Some(w) = WAKE.with(|w| w.borrow().get(&view).cloned()) {
        w.request_redraw();
    }
    true
}

/// The bar is kept per window and drawn by the overlay; the earlier one is dropped. A bar that is
/// to hide is hidden at once, so it never shows while the app believes it away.
pub fn replace_menu_bar(earlier: Option<muda::Menu>, _menu: &muda::Menu, view: isize, hide: bool) -> muda::Result<()> {
    drop(earlier);
    with_bars(|b| b.insert(view, Bar { hidden: hide }));
    Ok(())
}

/// A Windows-only extra: menus here are the toolkit's own.
pub fn install_menu_tips(_hwnd: isize, _delay: std::time::Duration) {}
