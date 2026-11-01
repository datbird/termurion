//! Windows-only shell extras with no Unix meaning: the Jump List, the taskbar button's badge and the
//! DLL search hardening. On Unix these are `Ok(())` no-ops (the one place a silent success is
//! allowed): there is no taskbar button or Jump List to fill, and no DLL search path to restrict.

#[cfg(unix)]
use super::unix::shell_extras as imp;
#[cfg(windows)]
use super::windows::shell_extras as imp;

/// Writes the Jump List (the Recent and Tasks categories); the names it had to leave out are returned.
/// A test build never writes the real list.
pub fn jump_list(m: &crate::jumplist::Model, exe: &std::path::Path) -> Result<Vec<Vec<String>>, String> {
    imp::jump_list(m, exe)
}

/// Clears the recent destinations Windows keeps for this app (the Jump List's Recent category).
pub fn jump_list_clear() -> Result<(), String> {
    imp::jump_list_clear()
}

/// Puts `label` (the bell count; "" removes it) on the window's taskbar button.
pub fn taskbar_progress(hwnd: isize, label: &str) -> Result<(), String> {
    imp::taskbar_progress(hwnd, label)
}

/// A window's taskbar button is told when Explorer creates it again, so the badge can be set afresh.
pub fn taskbar_watch(hwnd: isize) {
    imp::taskbar_watch(hwnd)
}

/// Every taskbar overlay off, as the process ends.
pub fn taskbar_clear_all() {
    imp::taskbar_clear_all()
}

/// Restricts the process's DLL search to System32 and the app folder. Call first in `main`.
pub fn dll_hardening() -> Result<(), String> {
    imp::dll_hardening()
}
