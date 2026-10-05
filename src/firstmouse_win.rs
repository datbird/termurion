//! First mouse (milestone 13j): a subclass on each terminal window sees the click that
//! activates it. `WM_MOUSEACTIVATE` arrives before the button message when a click activates a
//! window; its `lParam` low word is the hit test. The flag is set for a client-area activation
//! of a window that is not the foreground window, and taken once by the press that follows
//! (`PTYMouseHandler.m:1233-1237`, `:165`). The subclass id 0x7466_6D73 ("tfms") is free; the
//! others in use are 0x7A4E (display), 0x7473_697A, 0x7468_656D, 0x7463_6170, 0x7077_6D6C and
//! 0x7462_6D75.

use std::sync::Mutex;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, HTCLIENT, WM_MOUSEACTIVATE, WM_NCDESTROY};

const SUBCLASS_ID: usize = 0x7466_6D73;

/// The watched windows and whether each has an activating click waiting.
static WINDOWS: Mutex<Vec<(isize, bool)>> = Mutex::new(Vec::new());

/// Whether a `WM_MOUSEACTIVATE` marks first mouse: the hit test (low word of `lParam`) is the
/// client area and the window is not the foreground window.
pub fn marks(hit_test: u32, foreground: isize, hwnd: isize) -> bool {
    hit_test == HTCLIENT && foreground != hwnd
}

/// Starts watching a window (its HWND), once.
pub fn watch(hwnd: isize) {
    let Ok(mut w) = WINDOWS.lock() else { return };
    if w.iter().any(|(h, _)| *h == hwnd) {
        return;
    }
    // SAFETY: the HWND is a live window of this process; the proc only records and forwards.
    if unsafe { SetWindowSubclass(hwnd as HWND, Some(proc), SUBCLASS_ID, 0) } != 0 {
        w.push((hwnd, false));
    }
}

/// True once after a client-area activation of `hwnd` while it was not in front.
pub fn take(hwnd: isize) -> bool {
    let Ok(mut w) = WINDOWS.lock() else { return false };
    w.iter_mut().find(|(h, _)| *h == hwnd).is_some_and(|(_, f)| std::mem::take(f))
}

/// Stops watching a window and removes its subclass.
pub fn forget(hwnd: isize) {
    if let Ok(mut w) = WINDOWS.lock() {
        w.retain(|(h, _)| *h != hwnd);
    }
    // SAFETY: removing this proc from a window of this process (a no-op if it is not there).
    unsafe { RemoveWindowSubclass(hwnd as HWND, Some(proc), SUBCLASS_ID) };
}

unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM, _id: usize, _data: usize) -> LRESULT {
    if msg == WM_MOUSEACTIVATE {
        // SAFETY: a plain query.
        let front = unsafe { GetForegroundWindow() } as isize;
        if marks((lparam & 0xFFFF) as u32, front, hwnd as isize) {
            if let Ok(mut w) = WINDOWS.lock() {
                if let Some(e) = w.iter_mut().find(|(h, _)| *h == hwnd as isize) {
                    e.1 = true;
                }
            }
        }
    } else if msg == WM_NCDESTROY {
        if let Ok(mut w) = WINDOWS.lock() {
            w.retain(|(h, _)| *h != hwnd as isize);
        }
        // SAFETY: removing this proc from the window being destroyed.
        unsafe { RemoveWindowSubclass(hwnd, Some(proc), SUBCLASS_ID) };
    }
    // SAFETY: forwarding the message the system delivered, unchanged (winit's answer passes the click on).
    unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DestroyWindow, SendMessageW, HTCAPTION, HTCLIENT, HWND_MESSAGE, WM_MOUSEACTIVATE};

    fn message_window() -> HWND {
        let class: Vec<u16> = "STATIC\0".encode_utf16().collect();
        // SAFETY: a hidden message-only window of the system's STATIC class, on this thread.
        let h = unsafe { CreateWindowExW(0, class.as_ptr(), std::ptr::null(), 0, 0, 0, 0, 0, HWND_MESSAGE, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null()) };
        assert!(!h.is_null());
        h
    }

    #[test]
    fn first_mouse_flag_follows_windows() {
        let h = message_window();
        watch(h as isize);
        // SAFETY: the window is this thread's; the subclass proc runs inside SendMessageW.
        unsafe { SendMessageW(h, WM_MOUSEACTIVATE, 0, HTCLIENT as isize) };
        assert!(take(h as isize), "a client-area activation while not in front");
        assert!(!take(h as isize), "true once");
        unsafe { SendMessageW(h, WM_MOUSEACTIVATE, 0, HTCAPTION as isize) };
        assert!(!take(h as isize), "a title bar click is not first mouse");
        unsafe { SendMessageW(h, WM_MOUSEACTIVATE, 0, (HTCLIENT as isize) | (0x0201 << 16)) };
        assert!(take(h as isize), "only the low word is the hit test");
        forget(h as isize);
        unsafe { SendMessageW(h, WM_MOUSEACTIVATE, 0, HTCLIENT as isize) };
        assert!(!take(h as isize), "forgotten");
        unsafe { DestroyWindow(h) };
    }

    #[test]
    fn activation_rule_is_client_area_and_not_in_front() {
        assert!(marks(HTCLIENT, 5, 6));
        assert!(!marks(HTCLIENT, 6, 6), "already the foreground window");
        assert!(!marks(HTCAPTION, 5, 6));
    }
}
