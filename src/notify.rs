//! Windows notifications (a tray icon's balloon, which Windows shows as a toast). A hidden
//! top-level window owns the tray icon (a message-only window never hears the taskbar's
//! `TaskbarCreated` broadcast), so a click on the notification can reveal the session it came
//! from. A message-only reopen window lets a second start of Tandem act as the dock click.

use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, Ordering};
use std::sync::OnceLock;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Shell::{NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_INFO, NIIF_NOSOUND, NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW, Shell_NotifyIconW};
use windows_sys::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DefWindowProcW, DestroyWindow, FindWindowExW, HWND_MESSAGE, IDI_APPLICATION, LoadIconW, PostMessageW, RegisterClassW, RegisterWindowMessageW, WM_APP, WNDCLASSW, WS_EX_TOOLWINDOW, WS_POPUP};

const ICON_ID: u32 = 0x7A4D;
/// The tray icon's callback message.
const CALLBACK: u32 = WM_APP + 0x4D;
/// Shell_NotifyIcon's balloon click (NIN_BALLOONUSERCLICK).
const BALLOON_CLICK: u32 = 0x0405;
static ADDED: AtomicBool = AtomicBool::new(false);
static WINDOW: AtomicIsize = AtomicIsize::new(0);
static ON_CLICK: OnceLock<Box<dyn Fn() + Send + Sync>> = OnceLock::new();
static ON_TRAY: OnceLock<Box<dyn Fn(bool) + Send + Sync>> = OnceLock::new();
/// Whether the resident (notification area) icon is wanted: it is re-added when Explorer restarts.
static RESIDENT: AtomicBool = AtomicBool::new(false);
static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);
/// A second start's request to the running Tandem: act as the dock click.
const REOPEN: u32 = WM_APP + 0x4E;
static REOPEN_WINDOW: AtomicIsize = AtomicIsize::new(0);
const REOPEN_CLASS: &str = "TandemReopen\0";
const WM_LBUTTONUP: u32 = 0x0202;
const WM_RBUTTONUP: u32 = 0x0205;

/// A tray click from the callback's low word: Some(true) for a right click, Some(false) for a
/// left click.
fn click_of(low: u32) -> Option<bool> {
    match low {
        WM_LBUTTONUP => Some(false),
        WM_RBUTTONUP => Some(true),
        _ => None,
    }
}

unsafe extern "system" fn reopen_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if msg == REOPEN {
        if let Some(f) = ON_TRAY.get() {
            f(false);
        }
        return 0;
    }
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// While Tandem is excluded from the taskbar and Alt+Tab, a message-only window waits for a second
/// start (iTerm2's reopen, `applicationShouldHandleReopen`); it is made and dropped as the
/// mode changes (on the window thread).
pub fn set_reopen(on: bool) {
    let w = REOPEN_WINDOW.load(Ordering::SeqCst);
    if on && w == 0 {
        let class: Vec<u16> = REOPEN_CLASS.encode_utf16().collect();
        unsafe {
            let module = GetModuleHandleW(std::ptr::null());
            let wc = WNDCLASSW { lpfnWndProc: Some(reopen_proc), hInstance: module, lpszClassName: class.as_ptr(), ..std::mem::zeroed() };
            RegisterClassW(&wc);
            let hwnd = CreateWindowExW(0, class.as_ptr(), class.as_ptr(), 0, 0, 0, 0, 0, HWND_MESSAGE, std::ptr::null_mut(), module, std::ptr::null());
            if !hwnd.is_null() {
                REOPEN_WINDOW.store(hwnd as isize, Ordering::SeqCst);
            }
        }
    } else if !on && w != 0 {
        REOPEN_WINDOW.store(0, Ordering::SeqCst);
        unsafe { DestroyWindow(w as HWND) };
    }
}

/// A second start hands the dock click to a running Tandem that is excluded from the taskbar;
/// true when one took it (this process then ends).
pub fn forward_reopen() -> bool {
    let class: Vec<u16> = REOPEN_CLASS.encode_utf16().collect();
    unsafe {
        let h = FindWindowExW(HWND_MESSAGE, std::ptr::null_mut(), class.as_ptr(), std::ptr::null());
        !h.is_null() && PostMessageW(h, REOPEN, 0, 0) != 0
    }
}

fn copy(dst: &mut [u16], s: &str) {
    let wide: Vec<u16> = s.encode_utf16().take(dst.len() - 1).collect();
    dst[..wide.len()].copy_from_slice(&wide);
    dst[wide.len()] = 0;
}

/// What a click on a notification does (called on the window's thread).
pub fn on_click(f: Box<dyn Fn() + Send + Sync>) {
    let _ = ON_CLICK.set(f);
}

/// What a left or right click on the resident icon does (true for a right click).
pub fn on_tray(f: Box<dyn Fn(bool) + Send + Sync>) {
    let _ = ON_TRAY.set(f);
}

unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if msg == CALLBACK {
        let low = lparam as u32 & 0xFFFF;
        if low == BALLOON_CLICK {
            if let Some(f) = ON_CLICK.get() {
                f();
            }
        } else if let (Some(right), Some(f)) = (click_of(low), ON_TRAY.get()) {
            f(right);
        }
        return 0;
    }
    let created = TASKBAR_CREATED.load(Ordering::SeqCst);
    if created != 0 && msg == created {
        // Explorer restarted and dropped every icon: the resident one is added again.
        ADDED.store(false, Ordering::SeqCst);
        if RESIDENT.load(Ordering::SeqCst) {
            let _ = add_resident(hwnd);
        }
        return 0;
    }
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// The hidden notification window, made on first use (on the window thread, whose message loop
/// serves it).
fn window() -> Result<HWND, String> {
    let w = WINDOW.load(Ordering::SeqCst);
    if w != 0 {
        return Ok(w as HWND);
    }
    let class: Vec<u16> = "TandemNotifications\0".encode_utf16().collect();
    unsafe {
        let module = GetModuleHandleW(std::ptr::null());
        let wc = WNDCLASSW { lpfnWndProc: Some(proc), hInstance: module, lpszClassName: class.as_ptr(), ..std::mem::zeroed() };
        RegisterClassW(&wc);
        // A hidden top-level window (never shown, a tool window so no taskbar button): only a
        // top-level window receives the TaskbarCreated broadcast.
        let hwnd = CreateWindowExW(WS_EX_TOOLWINDOW, class.as_ptr(), class.as_ptr(), WS_POPUP, 0, 0, 0, 0, std::ptr::null_mut(), std::ptr::null_mut(), module, std::ptr::null());
        if hwnd.is_null() {
            return Err(format!("Windows did not make the notification window ({})", std::io::Error::last_os_error()));
        }
        WINDOW.store(hwnd as isize, Ordering::SeqCst);
        let name: Vec<u16> = "TaskbarCreated\0".encode_utf16().collect();
        TASKBAR_CREATED.store(RegisterWindowMessageW(name.as_ptr()), Ordering::SeqCst);
        Ok(hwnd)
    }
}

fn base(hwnd: HWND) -> NOTIFYICONDATAW {
    let mut nid: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
    nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = hwnd;
    nid.uID = ICON_ID;
    nid
}

fn add_resident(hwnd: HWND) -> Result<(), String> {
    let mut nid = base(hwnd);
    nid.uFlags = NIF_ICON | NIF_TIP | NIF_MESSAGE;
    nid.uCallbackMessage = CALLBACK;
    nid.hIcon = unsafe { LoadIconW(std::ptr::null_mut(), IDI_APPLICATION) };
    copy(&mut nid.szTip, crate::presence::TRAY_TIP);
    let op = if ADDED.swap(true, Ordering::SeqCst) { NIM_MODIFY } else { NIM_ADD };
    if unsafe { Shell_NotifyIconW(op, &nid) } == 0 {
        ADDED.store(false, Ordering::SeqCst);
        return Err("Windows did not add the notification area icon".into());
    }
    Ok(())
}

/// Adds or removes the resident notification area icon (the way back for a Tandem that left
/// the taskbar). Removing always deletes the icon; a later balloon adds it again.
pub fn set_resident(on: bool) -> Result<(), String> {
    RESIDENT.store(on, Ordering::SeqCst);
    if on {
        return add_resident(window()?);
    }
    remove(0);
    Ok(())
}

/// Shows a notification with `title` and `body`; `silent` plays no sound.
pub fn show_with(title: &str, body: &str, silent: bool) -> Result<(), String> {
    let hwnd = window()?;
    let mut nid: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
    nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = hwnd;
    nid.uID = ICON_ID;
    nid.uFlags = NIF_INFO | NIF_ICON | NIF_TIP | NIF_MESSAGE;
    nid.uCallbackMessage = CALLBACK;
    nid.hIcon = unsafe { LoadIconW(std::ptr::null_mut(), IDI_APPLICATION) };
    nid.dwInfoFlags = NIIF_INFO | if silent { NIIF_NOSOUND } else { 0 };
    copy(&mut nid.szTip, "Tandem");
    copy(&mut nid.szInfoTitle, title);
    copy(&mut nid.szInfo, body);
    let op = if ADDED.swap(true, Ordering::SeqCst) { NIM_MODIFY } else { NIM_ADD };
    if unsafe { Shell_NotifyIconW(op, &nid) } == 0 {
        ADDED.store(false, Ordering::SeqCst);
        return Err("Windows did not show the notification".into());
    }
    Ok(())
}

/// Shows a notification (the mark alert's older entry point; `_owner` is not needed).
pub fn show(_owner: isize, title: &str, body: &str) -> Result<(), String> {
    show_with(title, body, false)
}

/// Removes the tray icon, if one was added.
pub fn remove(_owner: isize) {
    let w = WINDOW.load(Ordering::SeqCst);
    if w != 0 && ADDED.swap(false, Ordering::SeqCst) {
        let mut nid: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
        nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        nid.hWnd = w as HWND;
        nid.uID = ICON_ID;
        unsafe { Shell_NotifyIconW(NIM_DELETE, &nid) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The legacy callback's low word: a left button release is a left click, a right button
    /// release a right click, a balloon click is not a tray click.
    #[test]
    fn tray_clicks_decode() {
        assert_eq!(click_of(0x0202), Some(false));
        assert_eq!(click_of(0x0205), Some(true));
        assert_eq!(click_of(BALLOON_CLICK), None);
        assert_eq!(click_of(0x0200), None);
    }
}
