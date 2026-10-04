//! Virtual desktops in the window (13g): the shell manager, and putting a restored window back
//! on the desktop it was saved on.

use super::*;
use crate::arrangement::Value;

impl App {
    /// The shell's virtual desktop manager, made on first use. None (logged once) when the
    /// session has no desktop shell, which turns every desktop feature off quietly (P7).
    pub(super) fn desktops(&mut self) -> Option<&crate::desktops::win::Manager> {
        if !self.desktops_tried {
            self.desktops_tried = true;
            self.desktops = crate::desktops::win::Manager::new();
            if self.desktops.is_none() {
                eprintln!("virtual desktop manager unavailable; desktop features are off");
            }
        }
        self.desktops.as_ref()
    }

    /// Restore windows to same desktops: a window restored at launch moves to the desktop its
    /// saved table names, when the setting is on and that desktop still exists. A failure is a
    /// log line only.
    pub(super) fn restore_desktop(&mut self, saved_window: &Value) {
        if !self.globals.bool(crate::desktops::RESTORE_KEY) {
            return;
        }
        let Some(saved) = crate::desktops::saved(saved_window) else { return };
        let Some(hwnd) = self.hwnd() else { return };
        let Some(m) = self.desktops() else { return };
        let Some(target) = crate::desktops::restore_target(Some(saved), &m.list()) else { return };
        if let Err(e) = m.move_window(hwnd, target) {
            eprintln!("could not move the window to its saved desktop: {e}");
        }
    }
}
