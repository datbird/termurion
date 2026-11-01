//! The keyboard layer on Unix. Cmd and Super come from winit's modifiers, so there is no
//! low-level hook to install; the Windows-only hook calls are no-ops, and system-wide hotkeys report Unsupported.

use std::sync::{Arc, RwLock};

use super::super::Unsupported;
use crate::mouse::Mods;
use crate::remap::Tags;

/// Windows virtual key codes have no Unix meaning; 0 is "no key", as on Windows for a key without one.
pub fn vk_of(_key: winit::keyboard::PhysicalKey) -> u16 {
    0
}

/// The modifier remap hook is Windows-only (winit reports Cmd and Super), so there is nothing to install.
pub fn install(_hwnd: isize, _tags: Arc<RwLock<Tags>>, _exempt: Box<dyn Fn(u16, Mods) -> bool + Send + Sync>) -> Result<(), String> {
    Ok(())
}

/// Nothing was installed, so nothing is removed.
pub fn uninstall() {}

pub fn set_remap_global(_on: bool) {}

/// There is no Start menu to keep closed.
pub fn mask_start_menu() {}

/// Whether the buttons are swapped, then the left and right button states. Unix reports nothing
/// here: winit's mouse events carry the button state, so this is (false, false, false).
pub fn mouse_buttons() -> (bool, bool, bool) {
    (false, false, false)
}

pub fn remove_window(_hwnd: isize) {}

pub fn set_web_focus(_hwnd: isize) {}

pub fn set_hotkey_sink(_sink: Box<dyn Fn(Vec<u32>) + Send + Sync>) {}

pub fn set_hotkeys(_chords: Vec<(u32, crate::hotkey::Chord)>, _taps: Vec<(u32, u8)>, _min: std::time::Duration, _max: std::time::Duration) -> Result<Vec<(u32, Result<(), String>)>, String> {
    Err(Unsupported { feature: "System-wide hotkeys" }.into())
}

/// The text a key types in the keyboard layout; Unix has no virtual key layout lookup, so a key types nothing here.
pub fn text_for(_vk: u16, _scan: u32, _mods: Mods) -> Option<String> {
    None
}

/// The character a punctuation virtual key types: no layout lookup on Unix.
pub fn punctuation(_vk: u16) -> Option<char> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unix_hook_calls_are_noops() {
        let tags = Arc::new(RwLock::new(Tags::identity()));
        assert!(install(0, tags, Box::new(|_, _| false)).is_ok());
        assert_eq!(vk_of(winit::keyboard::PhysicalKey::Unidentified(winit::keyboard::NativeKeyCode::Unidentified)), 0);
    }

    #[test]
    fn unix_hotkeys_unsupported() {
        let e = set_hotkeys(Vec::new(), Vec::new(), std::time::Duration::ZERO, std::time::Duration::ZERO).unwrap_err();
        assert!(e.contains("System-wide hotkeys is not available"), "{e}");
    }
}
