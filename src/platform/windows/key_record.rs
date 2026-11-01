//! The recorder's view of the keyboard hook.

use crate::menus::ChordKey;

pub fn chord_key(key: winit::keyboard::PhysicalKey) -> Option<ChordKey> {
    crate::winkeys::vk_to_key(crate::winkeys::vk_of(key))
}

pub fn set_recording(on: bool) {
    crate::winkeys::set_recording(on)
}

pub fn set_settings_window(hwnd: isize) {
    crate::winkeys::set_settings_window(hwnd)
}
