//! The recorder on Unix: a table from winit's key codes. There is no keyboard hook.

use crate::menus::ChordKey;
use winit::keyboard::{Key, KeyCode as K, PhysicalKey};

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

/// The chord key of a key event: the character on macOS, the position elsewhere.
pub fn chord_key_of(ev: &winit::event::KeyEvent) -> Option<ChordKey> {
    if crate::platform::keys::CMD_IS_A_KEY {
        use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;
        chord_key_from(&ev.logical_key, &ev.key_without_modifiers(), ev.physical_key)
    } else {
        chord_key(ev.physical_key)
    }
}

/// The chord key the character names, as AppKit matches a key equivalent: one ASCII graphic
/// character (a shifted one as its unshifted key), else the same from the key without modifiers
/// (Option makes a symbol), else the key's position. A non-Latin layout or a viewer that sends
/// key code 0 still gets the right key.
pub fn chord_key_from(logical: &Key, unmodified: &Key, physical: PhysicalKey) -> Option<ChordKey> {
    let ascii = |k: &Key| {
        let Key::Character(s) = k else { return None };
        let mut chars = s.chars();
        let c = chars.next().filter(|c| c.is_ascii_graphic() && chars.next().is_none())?;
        let base = crate::menus::SHIFTED.iter().find(|(shifted, _)| *shifted == c).map_or(c, |(_, base)| *base);
        Some(ChordKey::Char(base.to_ascii_lowercase()))
    };
    ascii(logical).or_else(|| ascii(unmodified)).or_else(|| chord_key(physical))
}

/// No keyboard hook, so nothing to pause.
pub fn set_recording(_on: bool) {}

/// No keyboard hook, so no remapping to count the window in.
pub fn set_settings_window(_hwnd: isize) {}

#[cfg(test)]
mod tests {
    use super::*;
    use winit::keyboard::NamedKey;

    fn ch(s: &str) -> Key {
        Key::Character(s.into())
    }

    #[test]
    fn the_chord_key_is_the_character() {
        // Key code 0 from a VNC client, with the right character.
        assert_eq!(chord_key_from(&ch("t"), &ch("t"), PhysicalKey::Code(K::KeyA)), Some(ChordKey::Char('t')));
        assert_eq!(chord_key_from(&ch("}"), &ch("}"), PhysicalKey::Code(K::BracketRight)), Some(ChordKey::Char(']')));
        assert_eq!(chord_key_from(&ch("T"), &ch("T"), PhysicalKey::Code(K::KeyT)), Some(ChordKey::Char('t')));
        // A non-Latin layout falls back to the key's US position.
        assert_eq!(chord_key_from(&ch("\u{442}"), &ch("\u{442}"), PhysicalKey::Code(K::KeyN)), Some(ChordKey::Char('n')));
        assert_eq!(chord_key_from(&Key::Named(NamedKey::ArrowUp), &Key::Named(NamedKey::ArrowUp), PhysicalKey::Code(K::ArrowUp)), Some(ChordKey::Up));
        // AZERTY puts Q where A sits on a US board.
        assert_eq!(chord_key_from(&ch("q"), &ch("q"), PhysicalKey::Code(K::KeyA)), Some(ChordKey::Char('q')));
        // Option makes the character a symbol; the key without modifiers names the key (AZERTY A).
        assert_eq!(chord_key_from(&ch("\u{e6}"), &ch("a"), PhysicalKey::Code(K::KeyQ)), Some(ChordKey::Char('a')));
        // Both non-ASCII: the US position.
        assert_eq!(chord_key_from(&ch("\u{e6}"), &ch("\u{442}"), PhysicalKey::Code(K::KeyQ)), Some(ChordKey::Char('q')));
    }
}
