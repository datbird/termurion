//! Virtual desktops are the Windows shell's; on Unix there is no manager, which the app reads as
//! "the session has no desktop shell" and turns the desktop features off.

use crate::desktops::DesktopId;

pub struct Manager;

impl Manager {
    pub fn new() -> Option<Manager> {
        None
    }

    pub fn window_desktop(&self, _hwnd: isize) -> Option<DesktopId> {
        None
    }

    pub fn on_current(&self, _hwnd: isize) -> Option<bool> {
        None
    }

    pub fn move_window(&self, _hwnd: isize, _id: DesktopId) -> Result<(), String> {
        Err(crate::platform::Unsupported { feature: "Virtual desktops" }.to_string())
    }

    pub fn list(&self) -> Vec<DesktopId> {
        Vec::new()
    }

    pub fn current(&self) -> Option<DesktopId> {
        None
    }
}
