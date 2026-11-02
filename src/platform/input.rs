//! System input and display preferences the UI reads: keys held, the double-click time, whether the
//! OS plays animations.

#[cfg(unix)]
use super::unix::input as imp;
#[cfg(windows)]
use super::windows::input as imp;

/// Whether Alt (Option) is held right now, read from the system rather than an event. Unix has no
/// global key state to read, so it says no; callers that have an event use its modifiers.
pub fn alt_down() -> bool {
    imp::alt_down()
}

/// The double-click time in milliseconds.
pub fn double_click_ms() -> u128 {
    imp::double_click_ms()
}

/// Starts noticing a window's first-mouse click (a click that activates it; `take`n by the mouse code).
/// Unix has no such mark, so this does nothing there.
pub fn first_mouse_watch(hwnd: isize) {
    imp::first_mouse_watch(hwnd)
}

/// Stops noticing a window (it is going away). Nothing on Unix.
pub fn first_mouse_forget(hwnd: isize) {
    imp::first_mouse_forget(hwnd)
}

/// Whether the OS plays animations (off for reduced motion).
pub fn animations_on() -> bool {
    imp::animations_on()
}
