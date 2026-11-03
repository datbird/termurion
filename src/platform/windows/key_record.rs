//! The recorder's view of the keyboard hook.

use crate::menus::ChordKey;

pub fn chord_key(key: winit::keyboard::PhysicalKey) -> Option<ChordKey> {
    crate::platform::keys::vk_to_key(super::keys::vk_of(key))
}

pub fn chord_key_of(ev: &winit::event::KeyEvent) -> Option<ChordKey> {
    chord_key(ev.physical_key)
}

pub fn set_recording(on: bool) {
    super::keys::set_recording(on)
}

pub fn set_settings_window(hwnd: isize) {
    super::keys::set_settings_window(hwnd)
}
