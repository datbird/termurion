//! The menu bar and pop-up menus on macOS. One bar serves the whole screen, so a focused window's
//! bar becomes the app's (`bar_focused`). Window border, corners and the theme guard have no
//! meaning here and come from the shared Unix no-ops.

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use muda::ContextMenu;
use objc2::MainThreadMarker;
use objc2_app_kit::NSView;

use crate::platform::Unsupported;

pub use super::super::unix::chrome::{Lift, guard_theme, install_menu_tips, invisible_top_bottom, set_border, set_corners, shift_down, theme_reset_handled};

/// The bar is the screen's, not a strip in the window.
pub const IN_WINDOW_BAR: bool = false;

/// AppKit draws each item's shortcut from its key equivalent.
pub const NATIVE_ACCELERATORS: bool = true;

/// A bar was handed to the app at least once.
static BAR_SET: AtomicBool = AtomicBool::new(false);

/// The menu (its NSMenu address) that is the app's main menu now.
static CURRENT: AtomicUsize = AtomicUsize::new(0);

const OFF_MAIN: &str = "AppKit menus can only be used on the main thread.";

pub fn available() -> Result<(), Unsupported> {
    Ok(())
}

/// The screen has one bar and macOS hides it in full screen itself: no meaning to do here.
pub fn hide_menu(_menu: &muda::Menu, _view: isize) -> Result<(), String> {
    Ok(())
}

/// See `hide_menu`.
pub fn show_menu(_menu: &muda::Menu, _view: isize) -> Result<(), String> {
    Ok(())
}

pub fn menu_attached(_view: isize) -> bool {
    BAR_SET.load(Ordering::Relaxed)
}

/// AppKit follows the system appearance; the bar has no theme of its own to set.
pub fn apply_menu_theme(_view: isize, _forced: Option<bool>, _high_contrast: bool, _bar: Option<&muda::Menu>) -> Result<(), String> {
    Ok(())
}

/// Makes `menu` the screen's bar, then drops the earlier one: a menu dropped while it is still the
/// app's main menu would take the bar away.
pub fn replace_menu_bar(earlier: Option<muda::Menu>, menu: &muda::Menu, _view: isize, _hide: bool) -> Result<(), String> {
    if MainThreadMarker::new().is_none() {
        return Err(OFF_MAIN.to_string());
    }
    menu.init_for_nsapp();
    CURRENT.store(menu.ns_menu() as usize, Ordering::Relaxed);
    drop(earlier);
    BAR_SET.store(true, Ordering::Relaxed);
    Ok(())
}

/// The window that came to the front hands its bar to the screen.
pub fn bar_focused(menu: &muda::Menu) {
    // Only when another window's bar is the app's: a focus gain on the same bar changes nothing.
    if MainThreadMarker::new().is_some() && CURRENT.swap(menu.ns_menu() as usize, Ordering::Relaxed) != menu.ns_menu() as usize {
        menu.init_for_nsapp();
    }
}

/// Termurion's Help menu gets AppKit's search field. Needs the bar attached first.
pub fn adopt_help_menu(sub: &muda::Submenu) {
    if MainThreadMarker::new().is_some() {
        sub.set_as_help_menu_for_nsapp();
    }
}

pub fn services_item() -> Option<muda::PredefinedMenuItem> {
    MainThreadMarker::new()?;
    Some(muda::PredefinedMenuItem::services(None))
}

pub fn claim_key_equivalents(view: isize, route: std::sync::Arc<std::sync::RwLock<crate::keyroute::Snapshot>>) -> Result<(), String> {
    super::key_claim::install(view, route)
}

pub fn forget_key_equivalents(view: isize) {
    super::key_claim::forget(view)
}

/// Pops the menu up in the window's view. False for a dismissed menu, and also when there is no
/// live view on the main thread (muda would panic on a view with no window).
pub fn popup<M: muda::ContextMenu + ?Sized>(menu: &M, view: isize, position: Option<muda::dpi::Position>) -> bool {
    if view == 0 || MainThreadMarker::new().is_none() {
        return false;
    }
    // SAFETY: the app got this pointer from its own live window this call.
    let in_window = unsafe { (*(view as *const NSView)).window().is_some() };
    if !in_window {
        return false;
    }
    // SAFETY: a live NSView of a window, on the main thread.
    unsafe { menu.show_context_menu_for_nsview(view as *const c_void, position) }
}
