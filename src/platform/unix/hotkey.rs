//! The hotkey window needs a native handle and a system-wide hotkey, and Unix has neither yet
//! (`keys::set_hotkeys` reports Unsupported, `window::hwnd_of` is None), so no caller holds a
//! handle to pass. These answer "nothing" for one handed in anyway.

/// No cloaking without a handle.
pub fn cloak(_hwnd: isize, _on: bool) {}

/// False: no opacity was set, so a fade stops at once.
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

pub fn order_front(_hwnd: isize, _minimized: bool) {}
