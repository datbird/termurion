//! WM_ENDSESSION on a terminal window (`iTermController.m:1746-1751`): a logoff, restart or
//! shut down ends the hosted shells before Windows ends the process. A window subclass runs
//! the closure inside the window procedure, so the work is done before the process can end.
//! HWND_MESSAGE windows get no broadcast; the watch goes on a terminal window.

use std::sync::Arc;

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::{WM_ENDSESSION, WM_NCDESTROY};

const SUBCLASS_ID: usize = 0x7464_6C67;

type OnEnd = Arc<dyn Fn() + Send + Sync>;

/// Every watched window, with the closure it runs (None once cleared).
fn windows() -> &'static Mutex<HashMap<isize, Option<OnEnd>>> {
    static W: OnceLock<Mutex<HashMap<isize, Option<OnEnd>>>> = OnceLock::new();
    W.get_or_init(Default::default)
}

/// Watches `hwnd` for the session ending, once per window; a second call replaces the closure.
pub(super) fn watch(hwnd: isize, on_end: OnEnd) {
    let Ok(mut w) = windows().lock() else { return };
    if let Some(slot) = w.get_mut(&hwnd) {
        *slot = Some(on_end);
        return;
    }
    // SAFETY: `hwnd` is a window of this thread; the proc removes itself at WM_NCDESTROY.
    if unsafe { SetWindowSubclass(hwnd as HWND, Some(proc), SUBCLASS_ID, 0) } != 0 {
        w.insert(hwnd, Some(on_end));
    }
}

/// How many windows are watched (the test reads it).
#[cfg(test)]
pub(super) fn watched() -> usize {
    windows().lock().map_or(0, |w| w.len())
}

/// Stops running the closure of `hwnd` (the watch itself stays until the window is destroyed).
pub(super) fn clear(hwnd: isize) {
    if let Ok(mut w) = windows().lock() {
        if let Some(slot) = w.get_mut(&hwnd) {
            *slot = None;
        }
    }
}

impl super::App {
    /// Keeps the logoff watch of the current window in step with the settings: with
    /// `KillSessionsOnLogout` and `runJobsInServers` on and a host running, WM_ENDSESSION ends
    /// every hosted shell (`killRestorableSessions`, `IC:1463-1472`); otherwise it does nothing.
    pub(super) fn tick_logout(&mut self) {
        let Some(hwnd) = self.hwnd() else { return };
        let (key, run) = (self.globals.bool("KillSessionsOnLogout"), self.globals.bool("runJobsInServers"));
        match &self.host {
            Some(host) if crate::session_end::kill_on_logout(key, run, true) => {
                let host = host.clone();
                watch(hwnd, Arc::new(move || host.kill_all()));
            }
            _ => clear(hwnd),
        }
    }
}

unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM, _id: usize, _data: usize) -> LRESULT {
    if msg == WM_ENDSESSION && wparam != 0 {
        // Run it here: Windows may end the process as soon as this returns. The lock is not held
        // while it runs.
        let on_end = windows().lock().ok().and_then(|w| w.get(&(hwnd as isize)).cloned().flatten());
        if let Some(f) = on_end {
            f();
        }
    } else if msg == WM_NCDESTROY {
        // SAFETY: removing the subclass this proc was installed as.
        unsafe { RemoveWindowSubclass(hwnd, Some(proc), SUBCLASS_ID) };
        if let Ok(mut w) = windows().lock() {
            w.remove(&(hwnd as isize));
        }
    }
    // SAFETY: forwarding the message the system delivered, unchanged.
    unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DestroyWindow, SendMessageW, HWND_MESSAGE, WM_ENDSESSION};

    fn message_window() -> HWND {
        let class: Vec<u16> = "STATIC\0".encode_utf16().collect();
        unsafe { CreateWindowExW(0, class.as_ptr(), class.as_ptr(), 0, 0, 0, 0, 0, HWND_MESSAGE, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null()) }
    }

    #[test]
    fn logout_watch_is_installed_once() {
        let hwnd = message_window();
        assert!(!hwnd.is_null());
        let before = watched();
        let calls = Arc::new(AtomicUsize::new(0));
        let c = calls.clone();
        let on_end: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
            c.fetch_add(1, Ordering::SeqCst);
        });
        watch(hwnd as isize, on_end.clone());
        watch(hwnd as isize, on_end);
        assert_eq!(watched(), before + 1, "a window is watched once");
        unsafe { SendMessageW(hwnd, WM_ENDSESSION, 1, 0) };
        assert_eq!(calls.load(Ordering::SeqCst), 1, "wParam nonzero ends the shells");
        unsafe { SendMessageW(hwnd, WM_ENDSESSION, 0, 0) };
        assert_eq!(calls.load(Ordering::SeqCst), 1, "wParam zero is a cancelled logoff");
        clear(hwnd as isize);
        unsafe { SendMessageW(hwnd, WM_ENDSESSION, 1, 0) };
        assert_eq!(calls.load(Ordering::SeqCst), 1, "a cleared watch does nothing");
        unsafe { DestroyWindow(hwnd) };
        assert_eq!(watched(), before, "the watch goes with the window");
    }
}
