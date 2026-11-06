//! The keyboard layer on Unix. Cmd and Super come from winit's modifiers, so there is no
//! low-level hook to install; the Windows-only hook calls are no-ops. System-wide hotkeys are
//! `global_keys`: Carbon on macOS, an X11 key grab on Linux (a Wayland session says it has none).

use std::sync::{Arc, RwLock};

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

pub fn set_hotkey_sink(sink: Box<dyn Fn(Vec<u32>) + Send + Sync>) {
    super::global_keys::set_hotkey_sink(sink)
}

/// Carbon hot keys on macOS, an X11 grab on Linux; the double-tap delays have no use on either (taps are refused).
pub fn set_hotkeys(chords: Vec<(u32, crate::hotkey::Chord)>, taps: Vec<(u32, u8)>, _min: std::time::Duration, _max: std::time::Duration) -> Result<Vec<(u32, Result<(), String>)>, String> {
    super::global_keys::set_hotkeys(chords, taps)
}

/// Why this session can have no system-wide hotkey at all (Wayland), for Settings to say; None
/// where one can be tried. It looks only at the session kind, so it is cheap to ask each frame.
pub fn session_hotkey_refusal() -> Option<String> {
    #[cfg(not(target_os = "macos"))]
    if super::global_keys::session_kind() == super::global_keys::SessionKind::Wayland {
        return super::global_keys::session_refusal();
    }
    None
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

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn hotkeys_on_wayland_name_wayland() {
        use crate::hotkey::{Chord, ChordKey, MOD_ALT};
        use crate::platform::unix::global_keys::{force_session, SessionKind};
        force_session(Some(SessionKind::Wayland));
        let chord = Chord { key: ChordKey::Scan(0x58), mods: MOD_ALT };
        let e = set_hotkeys(vec![(1, chord)], Vec::new(), std::time::Duration::ZERO, std::time::Duration::ZERO).unwrap_err();
        assert!(e.contains("Wayland"), "{e}");
        force_session(None);
    }
}
