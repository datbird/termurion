//! The recorder on Unix: a table from winit's key codes. There is no keyboard hook.

use crate::menus::ChordKey;
use winit::keyboard::{KeyCode as K, PhysicalKey};

pub fn chord_key(key: PhysicalKey) -> Option<ChordKey> {
    let PhysicalKey::Code(code) = key else { return None };
    let letter = |c: char| Some(ChordKey::Char(c));
    match code {
        K::KeyA => letter('a'), K::KeyB => letter('b'), K::KeyC => letter('c'), K::KeyD => letter('d'),
        K::KeyE => letter('e'), K::KeyF => letter('f'), K::KeyG => letter('g'), K::KeyH => letter('h'),
        K::KeyI => letter('i'), K::KeyJ => letter('j'), K::KeyK => letter('k'), K::KeyL => letter('l'),
        K::KeyM => letter('m'), K::KeyN => letter('n'), K::KeyO => letter('o'), K::KeyP => letter('p'),
        K::KeyQ => letter('q'), K::KeyR => letter('r'), K::KeyS => letter('s'), K::KeyT => letter('t'),
        K::KeyU => letter('u'), K::KeyV => letter('v'), K::KeyW => letter('w'), K::KeyX => letter('x'),
        K::KeyY => letter('y'), K::KeyZ => letter('z'),
        K::Digit0 => letter('0'), K::Digit1 => letter('1'), K::Digit2 => letter('2'), K::Digit3 => letter('3'),
        K::Digit4 => letter('4'), K::Digit5 => letter('5'), K::Digit6 => letter('6'), K::Digit7 => letter('7'),
        K::Digit8 => letter('8'), K::Digit9 => letter('9'),
        K::ArrowUp => Some(ChordKey::Up),
        K::ArrowDown => Some(ChordKey::Down),
        K::ArrowLeft => Some(ChordKey::Left),
        K::ArrowRight => Some(ChordKey::Right),
        K::Home => Some(ChordKey::Home),
        K::End => Some(ChordKey::End),
        K::PageUp => Some(ChordKey::PageUp),
        K::PageDown => Some(ChordKey::PageDown),
        K::Delete => Some(ChordKey::Delete),
        K::Enter => Some(ChordKey::Enter),
        K::Tab => Some(ChordKey::Tab),
        K::Escape => Some(ChordKey::Escape),
        K::Backspace => Some(ChordKey::Backspace),
        K::Space => Some(ChordKey::Space),
        K::F1 => Some(ChordKey::F(1)), K::F2 => Some(ChordKey::F(2)), K::F3 => Some(ChordKey::F(3)),
        K::F4 => Some(ChordKey::F(4)), K::F5 => Some(ChordKey::F(5)), K::F6 => Some(ChordKey::F(6)),
        K::F7 => Some(ChordKey::F(7)), K::F8 => Some(ChordKey::F(8)), K::F9 => Some(ChordKey::F(9)),
        K::F10 => Some(ChordKey::F(10)), K::F11 => Some(ChordKey::F(11)), K::F12 => Some(ChordKey::F(12)),
        K::Minus => letter('-'), K::Equal => letter('='), K::BracketLeft => letter('['), K::BracketRight => letter(']'),
        K::Backslash => letter('\\'), K::Semicolon => letter(';'), K::Quote => letter('\''), K::Comma => letter(','),
        K::Period => letter('.'), K::Slash => letter('/'), K::Backquote => letter('`'),
        _ => None,
    }
}

/// No keyboard hook, so nothing to pause.
pub fn set_recording(_on: bool) {}

/// No keyboard hook, so no remapping to count the window in.
pub fn set_settings_window(_hwnd: isize) {}
