//! The local clock and the alert sound.

pub fn local_time() -> (u16, u16, u16, u16, u16, u16) {
    let now = unsafe { libc::time(std::ptr::null_mut()) };
    let mut t: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&now, &mut t) };
    ((t.tm_year + 1900) as u16, (t.tm_mon + 1) as u16, t.tm_mday as u16, t.tm_hour as u16, t.tm_min as u16, t.tm_sec as u16)
}

/// The terminal bell, since there is no system sound call without a desktop library.
pub fn beep() {
    use std::io::Write;
    let _ = std::io::stderr().write_all(b"\x07");
}
