//! Windows notifications (a tray icon's balloon, which Windows shows as a toast). A
//! message-only window owns the tray icon, so a click on the notification can reveal the
//! session it came from.

use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::OnceLock;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Shell::{NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_INFO, NIIF_NOSOUND, NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW, Shell_NotifyIconW};
use windows_sys::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DefWindowProcW, HWND_MESSAGE, IDI_APPLICATION, LoadIconW, RegisterClassW, WM_APP, WNDCLASSW};

const ICON_ID: u32 = 0x7A4D;
/// The tray icon's callback message.
const CALLBACK: u32 = WM_APP + 0x4D;
/// Shell_NotifyIcon's balloon click (NIN_BALLOONUSERCLICK).
const BALLOON_CLICK: u32 = 0x0405;
static ADDED: AtomicBool = AtomicBool::new(false);
static WINDOW: AtomicIsize = AtomicIsize::new(0);
static ON_CLICK: OnceLock<Box<dyn Fn() + Send + Sync>> = OnceLock::new();

fn copy(dst: &mut [u16], s: &str) {
    let wide: Vec<u16> = s.encode_utf16().take(dst.len() - 1).collect();
    dst[..wide.len()].copy_from_slice(&wide);
    dst[wide.len()] = 0;
}

/// What a click on a notification does (called on the window's thread).
pub fn on_click(f: Box<dyn Fn() + Send + Sync>) {
    let _ = ON_CLICK.set(f);
}

unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if msg == CALLBACK && (lparam as u32 & 0xFFFF) == BALLOON_CLICK {
        if let Some(f) = ON_CLICK.get() {
            f();
        }
        return 0;
    }
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// The message-only window, made on first use (on the window thread, whose message loop
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
        let hwnd = CreateWindowExW(0, class.as_ptr(), class.as_ptr(), 0, 0, 0, 0, 0, HWND_MESSAGE, std::ptr::null_mut(), module, std::ptr::null());
        if hwnd.is_null() {
            return Err(format!("Windows did not make the notification window ({})", std::io::Error::last_os_error()));
        }
        WINDOW.store(hwnd as isize, Ordering::SeqCst);
        Ok(hwnd)
    }
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
