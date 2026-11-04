//! Window chrome that is the OS's own: the border color, the corner rounding, the menu bar's
//! attach state and theme, and the Theme guard. DWM and the menu bar's native handle are Windows
//! things; on Linux the window manager draws the frame and the menu is not attached to a handle yet.
//! On macOS the menu bar is the screen's own and is built here (`macos::chrome`).
//!
//! A "view" below is `window::view_of`: the HWND on Windows, the NSView on macOS.

#[cfg(target_os = "macos")]
use super::macos::chrome as imp;
#[cfg(all(unix, not(target_os = "macos")))]
use super::unix::chrome as imp;
#[cfg(windows)]
use super::windows::chrome as imp;

/// Whether the menu bar and pop-up menus exist on this OS. The app asks before it builds one and
/// shows the reason where a menu was wanted.
pub fn available() -> Result<(), super::Unsupported> {
    imp::available()
}

/// The OS draws each item's shortcut itself (macOS), so a label carries no tab and chord text and
/// an item gets a native key equivalent instead.
pub const NATIVE_ACCELERATORS: bool = imp::NATIVE_ACCELERATORS;

/// The bar is a strip the app draws inside the window (Linux), so the layout reserves its height.
pub const IN_WINDOW_BAR: bool = imp::IN_WINDOW_BAR;

/// What a pop-up can show: a muda menu or submenu. The native systems pop the menu up themselves; the
/// in-window bar reads its items and draws them (`entries`).
pub trait PopupMenu: muda::ContextMenu {
    fn entries(&self) -> Vec<muda::MenuItemKind>;
}

impl PopupMenu for muda::Menu {
    fn entries(&self) -> Vec<muda::MenuItemKind> {
        self.items()
    }
}

impl PopupMenu for muda::Submenu {
    fn entries(&self) -> Vec<muda::MenuItemKind> {
        self.items()
    }
}

/// A pop-up waiting for its window's next frame.
pub struct QueuedPopup {
    pub items: Vec<muda::MenuItemKind>,
    /// Window pixels; None: where the pointer is.
    pub at: Option<(f32, f32)>,
}

/// Gives the window's frame a border color (a COLORREF); a no-op where the OS draws the border.
pub fn set_border(hwnd: isize, color: u32) {
    imp::set_border(hwnd, color)
}

/// Sets how the window's corners are rounded (the DWM corner preference).
pub fn set_corners(hwnd: isize, preference: i32) {
    imp::set_corners(hwnd, preference)
}

/// How far the outer frame reaches past the visible one at the top and the bottom.
pub fn invisible_top_bottom(hwnd: isize) -> (f32, f32) {
    imp::invisible_top_bottom(hwnd)
}

/// Takes the menu bar off the window.
pub fn hide_menu(menu: &muda::Menu, hwnd: isize) -> Result<(), String> {
    imp::hide_menu(menu, hwnd)
}

/// Puts the menu bar back on the window.
pub fn show_menu(menu: &muda::Menu, hwnd: isize) -> Result<(), String> {
    imp::show_menu(menu, hwnd)
}

/// Whether the window has a menu bar attached right now.
pub fn menu_attached(hwnd: isize) -> bool {
    imp::menu_attached(hwnd)
}

/// The menu bar and drop-down menus take the dark or light mode. Err is the menu library's refusal.
pub fn apply_menu_theme(hwnd: isize, forced: Option<bool>, high_contrast: bool, bar: Option<&muda::Menu>) -> Result<(), String> {
    imp::apply_menu_theme(hwnd, forced, high_contrast, bar)
}

/// Asks `wake` to run after a system setting change, so the forced title bar is applied again.
pub fn guard_theme(hwnd: isize, wake: Box<dyn Fn()>) {
    imp::guard_theme(hwnd, wake)
}

/// The queued theme reset was handled.
pub fn theme_reset_handled() {
    imp::theme_reset_handled()
}

/// Shift is held right now (Unix: not readable, so false).
pub fn shift_down() -> bool {
    imp::shift_down()
}

pub use imp::Lift;

/// Shows the menu as a pop-up at `position` (the window's client pixels; None: at the pointer).
/// True when an item was picked, false when it was dismissed. The handle is a live window of this
/// thread, as the app gets it from `window::view_of`.
pub fn popup<M: PopupMenu + ?Sized>(menu: &M, view: isize, position: Option<muda::dpi::Position>) -> bool {
    imp::popup(menu, view, position)
}

/// Tells the in-window bar which window to ask for a frame when a pop-up is queued (Linux).
pub fn bind_window(view: isize, window: std::sync::Arc<winit::window::Window>) {
    #[cfg(all(unix, not(target_os = "macos")))]
    imp::bind_window(view, window);
    #[cfg(not(all(unix, not(target_os = "macos"))))]
    let _ = (view, window);
}

/// The pop-up the last `popup` call queued for this window, to draw in its next frame (Linux).
pub fn take_popup(view: isize) -> Option<QueuedPopup> {
    #[cfg(all(unix, not(target_os = "macos")))]
    return imp::take_popup(view);
    #[cfg(not(all(unix, not(target_os = "macos"))))]
    {
        let _ = view;
        None
    }
}

/// The window is going away: its bar state goes (Linux).
pub fn forget_window(view: isize) {
    #[cfg(all(unix, not(target_os = "macos")))]
    imp::forget_window(view);
    #[cfg(not(all(unix, not(target_os = "macos"))))]
    let _ = view;
}

/// Attaches `menu` as `hwnd`'s menu bar after taking `earlier` (the window's bar so far) off it.
/// The order matters: muda's drop of a menu clears the menu of every window it was on, so an
/// earlier bar dropped after the new one is attached leaves the window with no menu bar.
///
/// A bar for a window that is to hide it (`hide`: Minimal, Compact, full screen) is taken off
/// again at once, so it is never attached while the app believes it hidden.
/// Err is the refusal in words.
pub fn replace_menu_bar(earlier: Option<muda::Menu>, menu: &muda::Menu, view: isize, hide: bool) -> Result<(), String> {
    imp::replace_menu_bar(earlier, menu, view, hide).map_err(|e| e.to_string())
}

/// A window with this menu bar came to the front: its bar becomes the screen's. Windows and Linux
/// keep a bar on each window, so there is nothing to hand over there.
pub fn bar_focused(menu: &muda::Menu) {
    imp::bar_focused(menu)
}

/// Gives AppKit the Help submenu (its search field). Only macOS has a system Help menu.
pub fn adopt_help_menu(sub: &muda::Submenu) {
    imp::adopt_help_menu(sub)
}

/// The system's own Services submenu item, where the OS has one (macOS); none elsewhere.
pub fn services_item() -> Option<muda::PredefinedMenuItem> {
    imp::services_item()
}

/// Makes the window's view take every Command key-down before the menu does, so key mappings beat
/// menu shortcuts (macOS). Windows and Linux route keys through their own hook, so nothing to do.
/// `route` is what the key path takes (mappings, menu chords): only those Cmd keys are claimed.
pub fn claim_key_equivalents(view: isize, route: std::sync::Arc<std::sync::RwLock<crate::keyroute::Snapshot>>) -> Result<(), String> {
    imp::claim_key_equivalents(view, route)
}

/// The window's view is going away: it stops taking keys (macOS; nothing to forget elsewhere).
pub fn forget_key_equivalents(view: isize) {
    imp::forget_key_equivalents(view)
}

/// Shows a tip beside the highlighted menu item after `delay` (Windows owner-draws its menus; the toolkit
/// that draws them elsewhere has no such hook, so this does nothing there).
pub fn install_menu_tips(view: isize, delay: std::time::Duration) {
    imp::install_menu_tips(view, delay)
}
