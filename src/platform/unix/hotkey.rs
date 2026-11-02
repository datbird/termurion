//! The hotkey window needs a native handle and a system-wide hotkey, and Unix has neither yet
//! (`keys::set_hotkeys` reports Unsupported, `window::hwnd_of` is None). Calls that can fail say
//! `Unsupported { feature: "Hotkey window" }`; the window handle calls below can never be reached with
//! a handle, and answer "nothing" for one handed in anyway.

use super::super::Unsupported;

const FEATURE: &str = "Hotkey window";

/// No cloaking without a handle (none exists on Unix, so never reached).
pub fn cloak(_hwnd: isize, _on: bool) {}

/// Unsupported: no opacity was set, so a fade stops at once.
pub fn set_alpha(_hwnd: isize, _alpha: f32) -> bool {
    false
}

/// 0: no foreground handle.
pub fn foreground() -> isize {
    0
}

pub fn ours(_hwnd: isize) -> bool {
    false
}

pub fn is_window(_hwnd: isize) -> bool {
    false
}

/// No handle to order (never reached on Unix).
pub fn order_front(_hwnd: isize, _minimized: bool) {}

/// The hotkey window is not available on Unix yet.
pub fn available() -> Result<(), Unsupported> {
    Err(Unsupported { feature: FEATURE })
}
