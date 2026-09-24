//! Windows notifications for Post Notification (mark alerts). A notification needs a tray
//! icon on Windows, so Tandem adds one the first time it notifies and removes it on exit.

use std::sync::atomic::{AtomicBool, Ordering};

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::Shell::{NIF_ICON, NIF_INFO, NIF_TIP, NIIF_INFO, NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW, Shell_NotifyIconW};
use windows_sys::Win32::UI::WindowsAndMessaging::{IDI_APPLICATION, LoadIconW};

const ICON_ID: u32 = 0x7A4D;
static ADDED: AtomicBool = AtomicBool::new(false);

fn copy(dst: &mut [u16], s: &str) {
    let wide: Vec<u16> = s.encode_utf16().take(dst.len() - 1).collect();
    dst[..wide.len()].copy_from_slice(&wide);
    dst[wide.len()] = 0;
}

/// Shows a notification with `title` and `body`.
pub fn show(hwnd: isize, title: &str, body: &str) -> Result<(), String> {
    let mut nid: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
    nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = hwnd as HWND;
    nid.uID = ICON_ID;
    nid.uFlags = NIF_INFO | NIF_ICON | NIF_TIP;
    nid.hIcon = unsafe { LoadIconW(std::ptr::null_mut(), IDI_APPLICATION) };
    nid.dwInfoFlags = NIIF_INFO;
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

/// Removes the tray icon, if one was added.
pub fn remove(hwnd: isize) {
    if ADDED.swap(false, Ordering::SeqCst) {
        let mut nid: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
        nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        nid.hWnd = hwnd as HWND;
        nid.uID = ICON_ID;
        unsafe { Shell_NotifyIconW(NIM_DELETE, &nid) };
    }
}
