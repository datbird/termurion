//! The menu bar stays off a Minimal or Compact window on every path that attaches one. The bar
//! flashed into view while a window was resized: a bar attached behind the app's remembered "it is
//! hidden" note (a window recreated for transparency, a rebuild) sat under the client, and each
//! resize's non-client repaint made muda draw it over the client until the next frame.

use super::{menu_attached, menu_needs_change};
use windows_sys::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DestroyWindow};

/// Source of a file without its test modules.
fn live(src: &str) -> &str {
    src.split("#[cfg(test)]").next().unwrap()
}

/// The text of `fn_name`'s body: up to the next function at the same indent.
fn body<'a>(src: &'a str, head: &str) -> &'a str {
    let rest = src.split(head).nth(1).unwrap_or_else(|| panic!("{head} is gone"));
    rest.split("\n    pub(super) fn ").next().unwrap().split("\n    fn ").next().unwrap()
}

#[test]
fn the_window_is_asked_never_a_remembered_note() {
    let chrome = live(include_str!("chrome_ui.rs"));
    let sync = body(chrome, "pub(super) fn sync_menu_hidden");
    assert!(sync.contains("menu_attached(hwnd)"), "sync reads what the window has");
    assert!(!sync.contains("self.menu_hidden"), "no remembered hidden flag decides it");
    for (name, src) in [("mod.rs", include_str!("mod.rs")), ("windows_ui.rs", include_str!("windows_ui.rs")), ("chrome_ui.rs", include_str!("chrome_ui.rs"))] {
        assert!(!src.contains("menu_hidden:") && !src.contains("self.menu_hidden") && !src.contains("s.menu_hidden"), "{name} keeps no hidden flag");
    }
}

#[test]
fn a_built_bar_is_hidden_as_it_attaches() {
    let menu = live(include_str!("menu.rs"));
    let install = body(menu, "pub(super) fn install_menus");
    assert!(install.contains("self.want_menu_hidden()"), "install_menus passes what the window wants");
    let helper = menu.split("unsafe fn replace_menu_bar").nth(1).unwrap().split("\n}\n").next().unwrap();
    assert!(helper.contains("hide_for_hwnd"), "the helper hides the new bar itself");
}

#[test]
fn a_resize_settles_the_bar() {
    let m = include_str!("mod.rs");
    let resized = m.split("WindowEvent::Resized(size) =>").nth(1).unwrap().split("WindowEvent::ScaleFactorChanged").next().unwrap();
    assert!(resized.contains("self.sync_menu_hidden()"));
}

#[test]
fn a_bar_is_changed_only_when_the_window_differs_from_what_is_wanted() {
    assert!(menu_needs_change(true, true), "attached but to be hidden");
    assert!(menu_needs_change(false, false), "detached but to be shown");
    assert!(!menu_needs_change(true, false));
    assert!(!menu_needs_change(false, true));
}

fn window() -> isize {
    let class: Vec<u16> = "STATIC\0".encode_utf16().collect();
    // SAFETY: a hidden window of the system's STATIC class on this thread, destroyed by the test.
    let w = unsafe { CreateWindowExW(0, class.as_ptr(), std::ptr::null(), 0, 0, 0, 10, 10, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null()) };
    assert!(!w.is_null());
    w as isize
}

fn bar() -> muda::Menu {
    let m = muda::Menu::new();
    let shell = muda::Submenu::new("Shell", true);
    shell.append(&muda::MenuItem::new("New Window", true, None)).unwrap();
    m.append(&shell).unwrap();
    m
}

#[test]
fn the_window_reports_its_own_bar() {
    let w = window();
    assert!(!menu_attached(w));
    let b = bar();
    unsafe { b.init_for_hwnd(w) }.unwrap();
    assert!(menu_attached(w), "a bar attached by anyone is seen");
    super::hide_menu(&b, w).unwrap();
    assert!(!menu_attached(w));
    drop(b);
    unsafe { DestroyWindow(w as _) };
}
