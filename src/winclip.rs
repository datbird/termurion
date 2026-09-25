//! Writes several clipboard formats at once (plain text, "HTML Format", "Rich Text
//! Format"), for Copy with Styles.

use windows_sys::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, RegisterClipboardFormatW, SetClipboardData};
use windows_sys::Win32::Foundation::GlobalFree;
use windows_sys::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows_sys::Win32::System::Ole::CF_UNICODETEXT;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// Puts `bytes` on the clipboard as `format`. The clipboard owns the memory once this
/// succeeds.
unsafe fn put(format: u32, bytes: &[u8]) -> Result<(), String> {
    let mem = unsafe { GlobalAlloc(GMEM_MOVEABLE, bytes.len()) };
    if mem.is_null() {
        return Err("out of memory".into());
    }
    let p = unsafe { GlobalLock(mem) } as *mut u8;
    if p.is_null() {
        unsafe { GlobalFree(mem) };
        return Err("could not lock clipboard memory".into());
    }
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len());
        GlobalUnlock(mem);
    }
    if unsafe { SetClipboardData(format, mem) }.is_null() {
        unsafe { GlobalFree(mem) };
        return Err(format!("SetClipboardData failed ({})", std::io::Error::last_os_error()));
    }
    Ok(())
}

/// Replaces the clipboard with `text` and, for programs that take styles, `html` (a full
/// "HTML Format" document) and `rtf`.
pub fn set(text: &str, html: &str, rtf: &str) -> Result<(), String> {
    // Another program may hold the clipboard for a moment.
    let mut opened = false;
    for _ in 0..10 {
        if unsafe { OpenClipboard(std::ptr::null_mut()) } != 0 {
            opened = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    if !opened {
        return Err(format!("could not open the clipboard ({})", std::io::Error::last_os_error()));
    }
    let result = (|| unsafe {
        EmptyClipboard();
        let utf16: Vec<u8> = wide(text).iter().flat_map(|u| u.to_le_bytes()).collect();
        put(CF_UNICODETEXT as u32, &utf16)?;
        let mut h = html.as_bytes().to_vec();
        h.push(0);
        put(RegisterClipboardFormatW(wide("HTML Format").as_ptr()), &h)?;
        let mut r = rtf.as_bytes().to_vec();
        r.push(0);
        put(RegisterClipboardFormatW(wide("Rich Text Format").as_ptr()), &r)
    })();
    unsafe { CloseClipboard() };
    result
}

/// The first file on the clipboard (a copied file in Explorer), if any.
pub fn first_file() -> Option<std::path::PathBuf> {
    use windows_sys::Win32::System::DataExchange::{GetClipboardData, IsClipboardFormatAvailable};
    use windows_sys::Win32::System::Ole::CF_HDROP;
    use windows_sys::Win32::UI::Shell::{DragQueryFileW, HDROP};
    if unsafe { IsClipboardFormatAvailable(CF_HDROP as u32) } == 0 {
        return None;
    }
    if unsafe { OpenClipboard(std::ptr::null_mut()) } == 0 {
        return None;
    }
    let drop = unsafe { GetClipboardData(CF_HDROP as u32) } as HDROP;
    let path = if drop.is_null() {
        None
    } else {
        let len = unsafe { DragQueryFileW(drop, 0, std::ptr::null_mut(), 0) };
        let mut buf = vec![0u16; len as usize + 1];
        let got = unsafe { DragQueryFileW(drop, 0, buf.as_mut_ptr(), buf.len() as u32) };
        (got > 0).then(|| std::path::PathBuf::from(String::from_utf16_lossy(&buf[..got as usize])))
    };
    unsafe { CloseClipboard() };
    path
}
