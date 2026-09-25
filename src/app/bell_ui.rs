//! The bell in the window.

use super::*;

impl App {
    /// A bell from a program or a Ring Bell trigger.
    pub(super) fn ring_bell(&mut self, _pane: PaneId) {
        unsafe {
            windows_sys::Win32::System::Diagnostics::Debug::MessageBeep(windows_sys::Win32::UI::WindowsAndMessaging::MB_OK);
        }
    }
}
