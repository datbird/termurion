//! The local clock and the alert sound.

pub fn local_time() -> (u16, u16, u16, u16, u16, u16) {
    let now = unsafe { libc::time(std::ptr::null_mut()) };
    let mut t: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&now, &mut t) };
    ((t.tm_year + 1900) as u16, (t.tm_mon + 1) as u16, t.tm_mday as u16, t.tm_hour as u16, t.tm_min as u16, t.tm_sec as u16)
}

/// No system sound call yet (P1 replaces this). A GUI app's stderr goes nowhere, so the beep is
/// logged once through `dlog` as unsupported instead of being dropped.
pub fn beep() {
    static LOGGED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if !LOGGED.swap(true, std::sync::atomic::Ordering::Relaxed) {
        crate::dlog!("{}", super::super::Unsupported { feature: "System beep" });
    }
}
