//! Display changes: a subclass on each window tells the app when monitors or work areas change.

use std::sync::OnceLock;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::{SPI_SETWORKAREA, WM_DISPLAYCHANGE, WM_NCDESTROY, WM_SETTINGCHANGE};

const SUBCLASS_ID: usize = 0x7A4E;

static ON_CHANGE: OnceLock<Box<dyn Fn() + Send + Sync>> = OnceLock::new();

/// What to call (from the UI thread) when the monitors or a work area changed. Set once.
pub fn on_change(f: Box<dyn Fn() + Send + Sync>) {
    let _ = ON_CHANGE.set(f);
}

/// Whether a window message reports a display change: a resolution or monitor change, or a
/// new work area (the taskbar moved or resized).
pub fn is_display_message(msg: u32, wparam: usize) -> bool {
    msg == WM_DISPLAYCHANGE || (msg == WM_SETTINGCHANGE && wparam == SPI_SETWORKAREA as usize)
}

unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM, _id: usize, _data: usize) -> LRESULT {
    if is_display_message(msg, wparam) {
        if let Some(f) = ON_CHANGE.get() {
            f();
        }
    }
    if msg == WM_NCDESTROY {
        // SAFETY: removing this proc from the window being destroyed.
        unsafe { RemoveWindowSubclass(hwnd, Some(proc), SUBCLASS_ID) };
    }
    // SAFETY: forwarding the message the system delivered, unchanged.
    unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
}

/// Starts watching a window (its HWND) for display changes.
pub fn watch(hwnd: isize) -> bool {
    // SAFETY: the HWND is a live window of this process; the proc only forwards and calls the closure.
    unsafe { SetWindowSubclass(hwnd as _, Some(proc), SUBCLASS_ID, 0) != 0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::UI::WindowsAndMessaging::{SPI_SETWORKAREA, WM_DISPLAYCHANGE, WM_SETTINGCHANGE, WM_SIZE};

    #[test]
    fn display_messages_follow_windows() {
        assert!(is_display_message(WM_DISPLAYCHANGE, 0));
        assert!(is_display_message(WM_SETTINGCHANGE, SPI_SETWORKAREA as usize));
        assert!(!is_display_message(WM_SETTINGCHANGE, 0));
        assert!(!is_display_message(WM_SIZE, 0));
    }
}
