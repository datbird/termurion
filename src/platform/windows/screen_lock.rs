//! `WTS_SESSION_LOCK` through `WTSRegisterSessionNotification` on a terminal window (iTerm2's
//! `com.apple.screenIsLocked`), seen by a window subclass and handled on the next pass.
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::RemoteDesktop::{WTSRegisterSessionNotification, WTSUnRegisterSessionNotification, NOTIFY_FOR_THIS_SESSION};
use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::{WM_NCDESTROY, WM_WTSSESSION_CHANGE};

use crate::platform::pty::Notify;

const SUBCLASS_ID: usize = 0x7077_6D6C;
const WTS_SESSION_LOCK: usize = 7;

static LOCKED: AtomicBool = AtomicBool::new(false);
static WAKE: OnceLock<Notify> = OnceLock::new();
static WINDOWS: Mutex<Vec<isize>> = Mutex::new(Vec::new());

/// Watches `hwnd` for the screen locking, once per window.
pub fn watch(hwnd: isize, wake: Notify) {
    let _ = WAKE.set(wake);
    let Ok(mut w) = WINDOWS.lock() else { return };
    if w.contains(&hwnd) {
        return;
    }
    let ok = unsafe { WTSRegisterSessionNotification(hwnd as HWND, NOTIFY_FOR_THIS_SESSION) } != 0;
    if ok && unsafe { SetWindowSubclass(hwnd as HWND, Some(proc), SUBCLASS_ID, 0) } != 0 {
        w.push(hwnd);
    }
}

/// How many windows are watched.
pub fn watched() -> usize {
    WINDOWS.lock().map_or(0, |w| w.len())
}

/// Whether the screen locked since the last call.
pub fn take() -> bool {
    LOCKED.swap(false, Ordering::AcqRel)
}

unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM, _id: usize, _data: usize) -> LRESULT {
    if msg == WM_WTSSESSION_CHANGE && wparam == WTS_SESSION_LOCK {
        LOCKED.store(true, Ordering::Release);
        if let Some(w) = WAKE.get() {
            w();
        }
    } else if msg == WM_NCDESTROY {
        unsafe {
            WTSUnRegisterSessionNotification(hwnd);
            RemoveWindowSubclass(hwnd, Some(proc), SUBCLASS_ID);
        }
        if let Ok(mut w) = WINDOWS.lock() {
            w.retain(|h| *h != hwnd as isize);
        }
    }
    unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
}
