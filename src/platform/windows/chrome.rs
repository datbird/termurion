//! A window's border and corners through DWM, the menu bar's attach state and theme, and the guard
//! that asks the app to apply the Theme again after a system setting change.

use windows_sys::Win32::Foundation::RECT;
use windows_sys::Win32::Graphics::Dwm::{DWMWA_BORDER_COLOR, DWMWA_EXTENDED_FRAME_BOUNDS, DWMWA_WINDOW_CORNER_PREFERENCE, DwmGetWindowAttribute, DwmSetWindowAttribute};
use windows_sys::Win32::UI::WindowsAndMessaging::{GetMenu, GetWindowRect};

const THEME_SUBCLASS_ID: usize = 0x7468_656D;

thread_local! {
    /// The way back to the app from `theme_guard`, which never touches `App`.
    static WAKE: std::cell::RefCell<Option<Box<dyn Fn()>>> = const { std::cell::RefCell::new(None) };
    /// A `ThemeReset` is queued and not yet handled: one `WM_SETTINGCHANGE` broadcast reaches every
    /// terminal window, and the app needs to hear of it once.
    static RESET_PENDING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// The window's subclass proc: winit handles `WM_SETTINGCHANGE` (resetting the title bar to
/// Windows' mode), then the app is asked to apply the Theme again.
unsafe extern "system" fn theme_guard(hwnd: windows_sys::Win32::Foundation::HWND, msg: u32, wparam: windows_sys::Win32::Foundation::WPARAM, lparam: windows_sys::Win32::Foundation::LPARAM, _id: usize, _data: usize) -> windows_sys::Win32::Foundation::LRESULT {
    use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass};
    use windows_sys::Win32::UI::WindowsAndMessaging::WM_NCDESTROY;
    // SAFETY: forwarding the message the system delivered, unchanged.
    let result = unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) };
    if crate::dark_menu::reapplies_menus(msg) {
        // The app mode or the high contrast state may have changed.
        crate::tab_theme::forget_high_contrast();
        if !RESET_PENDING.with(|p| p.replace(true)) {
            WAKE.with(|w| {
                if let Some(wake) = w.borrow().as_ref() {
                    wake();
                }
            });
        }
    } else if msg == WM_NCDESTROY {
        // SAFETY: removing this proc from the window being destroyed.
        unsafe { RemoveWindowSubclass(hwnd, Some(theme_guard), THEME_SUBCLASS_ID) };
    }
    result
}

/// Gives the window's frame a border color (a COLORREF, or `chrome::COLOR_DEFAULT`). Only
/// Windows 11 (build 22000 on) takes the attribute; Windows 10's refusal is ignored and the
/// window keeps the system's border.
pub fn set_border(hwnd: isize, color: u32) {
    unsafe { DwmSetWindowAttribute(hwnd as _, DWMWA_BORDER_COLOR as u32, (&color as *const u32).cast(), 4) };
}

/// Sets how DWM rounds the window's corners (`chrome::corner_preference`).
pub fn set_corners(hwnd: isize, preference: i32) {
    unsafe { DwmSetWindowAttribute(hwnd as _, DWMWA_WINDOW_CORNER_PREFERENCE as u32, (&preference as *const i32).cast(), 4) };
}

/// Takes the menu bar off the window (`SetMenu` with no menu); the window keeps its outer
/// size, so the client area grows by the bar and a Resized event lays the window out.
pub fn hide_menu(menu: &muda::Menu, hwnd: isize) -> Result<(), String> {
    unsafe { menu.hide_for_hwnd(hwnd) }.map_err(|e| e.to_string())
}

/// Whether the window has a menu bar attached right now (the system's answer, not a note kept
/// by the app).
pub fn menu_attached(hwnd: isize) -> bool {
    // SAFETY: a plain query of a window of this thread.
    !unsafe { GetMenu(hwnd as _) }.is_null()
}

/// Puts the menu bar back on the window.
pub fn show_menu(menu: &muda::Menu, hwnd: isize) -> Result<(), String> {
    unsafe { menu.show_for_hwnd(hwnd) }.map_err(|e| e.to_string())
}

/// How far the outer frame reaches past the visible one at the top and the bottom: DWM's
/// invisible resize borders (0 without DWM's bounds).
pub fn invisible_top_bottom(hwnd: isize) -> (f32, f32) {
    let (mut outer, mut visible): (RECT, RECT) = unsafe { (std::mem::zeroed(), std::mem::zeroed()) };
    if unsafe { GetWindowRect(hwnd as _, &mut outer) } == 0 {
        return (0.0, 0.0);
    }
    if unsafe { DwmGetWindowAttribute(hwnd as _, DWMWA_EXTENDED_FRAME_BOUNDS as u32, (&mut visible as *mut RECT).cast(), std::mem::size_of::<RECT>() as u32) } != 0 {
        return (0.0, 0.0);
    }
    ((visible.top - outer.top).max(0) as f32, (outer.bottom - visible.bottom).max(0) as f32)
}

/// Asks `wake` (on this thread) to run after a system setting changed, once per broadcast, for the
/// window's forced title bar: winit resets a window with no preferred theme at every
/// `WM_SETTINGCHANGE` and sends nothing unless the mode changed.
pub fn guard_theme(hwnd: isize, wake: Box<dyn Fn()>) {
    WAKE.with(|w| *w.borrow_mut() = Some(wake));
    // SAFETY: the HWND is this window's; the proc only forwards and posts an event.
    unsafe { windows_sys::Win32::UI::Shell::SetWindowSubclass(hwnd as _, Some(theme_guard), THEME_SUBCLASS_ID, 0) };
}

/// The queued theme reset was handled; the next broadcast may queue another.
pub fn theme_reset_handled() {
    RESET_PENDING.with(|p| p.set(false));
}

/// Shift is held right now.
pub fn shift_down() -> bool {
    (unsafe { windows_sys::Win32::UI::Input::KeyboardAndMouse::GetKeyState(i32::from(windows_sys::Win32::UI::Input::KeyboardAndMouse::VK_SHIFT)) }) < 0
}

/// The menu bar and its drop-down menus take the mode `dark_menu::menu_dark` decides: a fixed
/// Theme's, else Windows' app mode read from the registry (muda's `Auto` needs a window allowed
/// dark, which winit never does, so the bar is always told Dark or Light). The drop-downs follow
/// the process app mode, set here too. Err is the menu library's refusal.
pub fn apply_menu_theme(hwnd: isize, forced: Option<bool>, high_contrast: bool, bar: Option<&muda::Menu>) -> Result<(), String> {
    let dark = crate::dark_menu::menu_dark(forced, crate::dark_menu::win::system_dark(), high_contrast);
    crate::dark_menu::win::apply(hwnd, crate::dark_menu::app_mode(forced, high_contrast));
    if let Some(menu) = bar {
        let theme = if crate::dark_menu::bar_dark(dark, crate::dark_menu::win::api()) { muda::MenuTheme::Dark } else { muda::MenuTheme::Light };
        // SAFETY: the HWND is this window's, and the bar was attached to it.
        unsafe { menu.set_theme_for_hwnd(hwnd, theme) }.map_err(|e| e.to_string())?;
    }
    Ok(())
}
