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

/// Whether the OS plays animations (off for reduced motion).
pub fn animations_on() -> bool {
    imp::animations_on()
}
