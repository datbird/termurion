//! iTerm2's keystrokes: the keys of its key binding tables ("0xf700-0x260000" and the
//! other forms), how a press is matched against a table, and a Windows label for each.
//! Flags use macOS values; Cmd is Win and Option is Alt (spec 3.4).

use winit::event::KeyEvent;
use winit::keyboard::{Key as WKey, KeyCode, KeyLocation, NamedKey, PhysicalKey};

use crate::keys::Key;
use crate::mouse::Mods;

pub const SHIFT: u64 = 0x20000;
pub const CONTROL: u64 = 0x40000;
pub const OPTION: u64 = 0x80000;
pub const COMMAND: u64 = 0x100000;
pub const NUMPAD: u64 = 0x200000;
/// Set on a key pressed right after the Leader.
pub const LEADER: u64 = 0x1000000;

/// The flags a key keeps; everything else (the Function flag, caps lock) is dropped.
const KEPT: u64 = SHIFT | CONTROL | OPTION | COMMAND | NUMPAD | LEADER;

/// macOS function-key characters (NSUpArrowFunctionKey and the rest).
pub const UP: u32 = 0xF700;
pub const DOWN: u32 = 0xF701;
pub const LEFT: u32 = 0xF702;
pub const RIGHT: u32 = 0xF703;
pub const F1: u32 = 0xF704;
pub const INSERT: u32 = 0xF727;
pub const DELETE: u32 = 0xF728;
pub const HOME: u32 = 0xF729;
pub const END: u32 = 0xF72B;
pub const PAGE_UP: u32 = 0xF72C;
pub const PAGE_DOWN: u32 = 0xF72D;
pub const CLEAR: u32 = 0xF739;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Keystroke {
    /// The character with no modifiers but Shift (macOS charactersIgnoringModifiers).
    pub ch: u32,
    pub flags: u64,
    /// The macOS virtual keycode of the physical key, when known.
    pub keycode: Option<u16>,
    /// The character the press typed, with every modifier applied.
    pub modified: u32,
    /// Only the typed character is known (the ":0x..:0x.." form).
    pub char_is_modified: bool,
}

impl Keystroke {
    pub fn new(ch: u32, flags: u64, keycode: Option<u16>, modified: u32) -> Keystroke {
        let mut flags = flags & KEPT;
        // Some keyboards set the numeric pad flag on arrows and some do not; iTerm2 always sets it.
        if (UP..=RIGHT).contains(&ch) {
            flags |= NUMPAD;
        }
        Keystroke { ch, flags, keycode, modified, char_is_modified: false }
    }

    pub fn legacy(&self) -> String {
        format!("0x{:x}-0x{:x}", self.ch, self.flags)
    }

    pub fn modified_key(&self) -> String {
        let ch = if self.char_is_modified { self.ch } else { self.modified };
        format!(":0x{:x}:0x{:x}", ch, self.flags & !SHIFT)
    }

    /// The physical identity: modifiers and keycode, not the character.
    pub fn portable(&self) -> String {
        match self.keycode {
            _ if self.char_is_modified => self.modified_key(),
            None => self.legacy(),
            Some(code) => format!("*-0x{:x}-0x{code:x}", self.flags),
        }
    }

    /// The form iTerm2 writes for this keystroke.
    pub fn serialized(&self) -> String {
        match self.keycode {
            _ if self.char_is_modified => self.modified_key(),
            None => self.legacy(),
            Some(code) => format!("0x{:x}-0x{:x}-0x{code:x}", self.ch, self.flags),
        }
    }

    /// The key in `map` this press uses, in iTerm2's order (keyInBindingDictionary:).
    pub fn find<'a>(&self, map: &'a toml::Table, language_agnostic: bool) -> Option<&'a str> {
        let has = |k: &str| map.get_key_value(k).map(|(k, _)| k.as_str());
        if language_agnostic && self.keycode.is_some() {
            if let Some(k) = has(&self.modified_key()) {
                return Some(k);
            }
            let portable = self.portable();
            if let Some(k) = map.keys().find(|k| parse(k).is_some_and(|c| c.portable() == portable)) {
                return Some(k);
            }
            return has(&self.legacy());
        }
        if let Some(k) = has(&self.serialized()).or_else(|| has(&self.legacy())).or_else(|| has(&self.modified_key())) {
            return Some(k);
        }
        let legacy = self.legacy();
        map.keys().find(|k| parse(k).is_some_and(|c| c.legacy() == legacy)).map(String::as_str)
    }

    /// Same character and flags (iTerm2's isEqual:), keycode aside.
    pub fn same_key(&self, other: &Keystroke) -> bool {
        self.ch == other.ch && self.flags == other.flags
    }

    /// "Win+Shift+Left": how the key reads on Windows.
    pub fn label(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        for (flag, name) in [(COMMAND, "Win"), (CONTROL, "Ctrl"), (OPTION, "Alt"), (SHIFT, "Shift")] {
            if self.flags & flag != 0 {
                parts.push(name.into());
            }
        }
        let keypad = self.flags & NUMPAD != 0 && !(UP..=RIGHT).contains(&self.ch);
        let name = key_name(self.ch);
        parts.push(if keypad { format!("Keypad {name}") } else { name });
        // iTerm2's Cmd is spelled with the shortcut key's real keys; the stored notation is untouched.
        let text = crate::shortcut_key::mode().show(&parts.join("+"));
        if self.flags & LEADER != 0 { format!("Leader, {text}") } else { text }
    }
}

fn key_name(ch: u32) -> String {
    match ch {
        0x03 => "Enter".into(),
        0x09 | 0x19 => "Tab".into(),
        0x0D => "Return".into(),
        0x1B => "Esc".into(),
        0x20 => "Space".into(),
        0x7F => "Backspace".into(),
        UP => "Up".into(),
        DOWN => "Down".into(),
        LEFT => "Left".into(),
        RIGHT => "Right".into(),
        INSERT => "Insert".into(),
        DELETE => "Delete".into(),
        HOME => "Home".into(),
        END => "End".into(),
        PAGE_UP => "PgUp".into(),
        PAGE_DOWN => "PgDn".into(),
        CLEAR => "Clear".into(),
        c if (F1..F1 + 35).contains(&c) => format!("F{}", c - F1 + 1),
        c => char::from_u32(c).filter(|c| !c.is_control()).map_or_else(|| format!("0x{c:x}"), |c| c.to_uppercase().to_string()),
    }
}

fn hex(s: &str) -> Option<u64> {
    u64::from_str_radix(s.trim().trim_start_matches("0x").trim_start_matches("0X"), 16).ok()
}

/// Reads any stored key form: "0xf700-0x260000", "0x61-0x100000-0x0", ":0x21:0x40000".
pub fn parse(s: &str) -> Option<Keystroke> {
    if let Some(rest) = s.strip_prefix(':') {
        let (ch, flags) = rest.split_once(':')?;
        let ch = hex(ch)? as u32;
        let mut k = Keystroke::new(ch, hex(flags)?, None, ch);
        k.char_is_modified = true;
        return Some(k);
    }
    let parts: Vec<&str> = s.split('-').collect();
    let (ch, flags, keycode) = match parts[..] {
        [c, f] => (hex(c)?, hex(f)?, None),
        [c, f, k] => (hex(c)?, hex(f)?, Some(hex(k)? as u16)),
        _ => return None,
    };
    Some(Keystroke::new(ch as u32, flags, keycode, ch as u32))
}

pub fn from_mods(m: Mods) -> u64 {
    (if m.shift { SHIFT } else { 0 }) | (if m.ctrl { CONTROL } else { 0 }) | (if m.alt { OPTION } else { 0 }) | (if m.cmd { COMMAND } else { 0 })
}

/// The macOS character for a key: its text for a character key, else the function-key
/// code. `shift` turns Tab into back-tab (0x19), as macOS does.
pub fn mac_char(key: &Key, shift: bool, keypad_enter: bool) -> u32 {
    match key {
        Key::Text(s) => s.chars().next().map_or(0, |c| c as u32),
        Key::Enter if keypad_enter => 0x03,
        Key::Enter => 0x0D,
        Key::Tab if shift => 0x19,
        Key::Tab => 0x09,
        Key::Backspace => 0x7F,
        Key::Escape => 0x1B,
        Key::Up => UP,
        Key::Down => DOWN,
        Key::Left => LEFT,
        Key::Right => RIGHT,
        Key::Home => HOME,
        Key::End => END,
        Key::PageUp => PAGE_UP,
        Key::PageDown => PAGE_DOWN,
        Key::Insert => INSERT,
        Key::Delete => DELETE,
        Key::F(n) => F1 + *n as u32 - 1,
    }
}

/// The macOS virtual keycode (kVK_*) for a physical key.
pub fn mac_keycode(code: KeyCode) -> Option<u16> {
    use KeyCode::*;
    Some(match code {
        KeyA => 0x00, KeyS => 0x01, KeyD => 0x02, KeyF => 0x03, KeyH => 0x04, KeyG => 0x05, KeyZ => 0x06, KeyX => 0x07,
        KeyC => 0x08, KeyV => 0x09, IntlBackslash => 0x0A, KeyB => 0x0B, KeyQ => 0x0C, KeyW => 0x0D, KeyE => 0x0E,
        KeyR => 0x0F, KeyY => 0x10, KeyT => 0x11, Digit1 => 0x12, Digit2 => 0x13, Digit3 => 0x14, Digit4 => 0x15,
        Digit6 => 0x16, Digit5 => 0x17, Equal => 0x18, Digit9 => 0x19, Digit7 => 0x1A, Minus => 0x1B, Digit8 => 0x1C,
        Digit0 => 0x1D, BracketRight => 0x1E, KeyO => 0x1F, KeyU => 0x20, BracketLeft => 0x21, KeyI => 0x22,
        KeyP => 0x23, Enter => 0x24, KeyL => 0x25, KeyJ => 0x26, Quote => 0x27, KeyK => 0x28, Semicolon => 0x29,
        Backslash => 0x2A, Comma => 0x2B, Slash => 0x2C, KeyN => 0x2D, KeyM => 0x2E, Period => 0x2F, Tab => 0x30,
        Space => 0x31, Backquote => 0x32, Backspace => 0x33, Escape => 0x35, NumpadDecimal => 0x41,
        NumpadMultiply => 0x43, NumpadAdd => 0x45, NumLock => 0x47, NumpadDivide => 0x4B, NumpadEnter => 0x4C,
        NumpadSubtract => 0x4E, NumpadEqual => 0x51, Numpad0 => 0x52, Numpad1 => 0x53, Numpad2 => 0x54,
        Numpad3 => 0x55, Numpad4 => 0x56, Numpad5 => 0x57, Numpad6 => 0x58, Numpad7 => 0x59, Numpad8 => 0x5B,
        Numpad9 => 0x5C, F5 => 0x60, F6 => 0x61, F7 => 0x62, F3 => 0x63, F8 => 0x64, F9 => 0x65, F11 => 0x67,
        F13 => 0x69, F16 => 0x6A, F14 => 0x6B, F10 => 0x6D, F12 => 0x6F, F15 => 0x71, Insert => 0x72, Home => 0x73,
        PageUp => 0x74, Delete => 0x75, F4 => 0x76, End => 0x77, F2 => 0x78, PageDown => 0x79, F1 => 0x7A,
        ArrowLeft => 0x7B, ArrowRight => 0x7C, ArrowDown => 0x7D, ArrowUp => 0x7E, F17 => 0x40, F18 => 0x4F,
        F19 => 0x50, F20 => 0x5A,
        _ => return None,
    })
}

/// The macOS virtual keycode (kVK_Shift and the rest) for a modifier key, which
/// `mac_keycode` leaves out; None for any other key.
pub fn mac_modifier_keycode(code: KeyCode) -> Option<u16> {
    use KeyCode::*;
    Some(match code {
        SuperLeft => 0x37,
        ShiftLeft => 0x38,
        CapsLock => 0x39,
        AltLeft => 0x3A,
        ControlLeft => 0x3B,
        ShiftRight => 0x3C,
        AltRight => 0x3D,
        ControlRight => 0x3E,
        SuperRight => 0x36,
        _ => return None,
    })
}

/// The Termurion key for a winit key, with `text` for keys winit does not name.
pub fn key_of(key: &WKey, text: Option<&str>) -> Option<Key> {
    Some(match key {
        WKey::Named(n) => match n {
            NamedKey::Enter => Key::Enter,
            NamedKey::Backspace => Key::Backspace,
            NamedKey::Tab => Key::Tab,
            NamedKey::Escape => Key::Escape,
            NamedKey::ArrowUp => Key::Up,
            NamedKey::ArrowDown => Key::Down,
            NamedKey::ArrowLeft => Key::Left,
            NamedKey::ArrowRight => Key::Right,
            NamedKey::Home => Key::Home,
            NamedKey::End => Key::End,
            NamedKey::PageUp => Key::PageUp,
            NamedKey::PageDown => Key::PageDown,
            NamedKey::Insert => Key::Insert,
            NamedKey::Delete => Key::Delete,
            NamedKey::Space => Key::Text(" ".into()),
            NamedKey::F1 => Key::F(1),
            NamedKey::F2 => Key::F(2),
            NamedKey::F3 => Key::F(3),
            NamedKey::F4 => Key::F(4),
            NamedKey::F5 => Key::F(5),
            NamedKey::F6 => Key::F(6),
            NamedKey::F7 => Key::F(7),
            NamedKey::F8 => Key::F(8),
            NamedKey::F9 => Key::F(9),
            NamedKey::F10 => Key::F(10),
            NamedKey::F11 => Key::F(11),
            NamedKey::F12 => Key::F(12),
            NamedKey::F13 => Key::F(13),
            NamedKey::F14 => Key::F(14),
            NamedKey::F15 => Key::F(15),
            NamedKey::F16 => Key::F(16),
            NamedKey::F17 => Key::F(17),
            NamedKey::F18 => Key::F(18),
            NamedKey::F19 => Key::F(19),
            NamedKey::F20 => Key::F(20),
            NamedKey::F21 => Key::F(21),
            NamedKey::F22 => Key::F(22),
            NamedKey::F23 => Key::F(23),
            NamedKey::F24 => Key::F(24),
            _ => return None,
        },
        WKey::Character(s) => Key::Text(s.to_string()),
        _ => return text.map(|t| Key::Text(t.to_string())),
    })
}

/// The keystroke for a key press, as the settings recorder and the app build it:
/// the Shift-applied character, the modifiers, the numeric pad flag, and the keycode.
pub fn from_key_event(ev: &KeyEvent, mods: Mods) -> Option<Keystroke> {
    let key = key_of(&ev.logical_key, ev.text.as_deref())?;
    let keypad = ev.location == KeyLocation::Numpad;
    let code = match ev.physical_key {
        PhysicalKey::Code(c) => Some(c),
        _ => None,
    };
    let flags = from_mods(mods) | if keypad { NUMPAD } else { 0 };
    let ch = mac_char(&key, mods.shift, keypad && key == Key::Enter);
    let modified = ev.text.as_deref().and_then(|t| t.chars().next()).map_or(ch, |c| c as u32);
    Some(Keystroke::new(ch, flags, code.and_then(mac_keycode), modified))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_every_stored_form() {
        let k = parse("0xf700-0x260000").unwrap();
        assert_eq!((k.ch, k.flags, k.keycode), (0xF700, 0x260000, None));
        let k = parse("0x61-0x100000-0x0").unwrap();
        assert_eq!((k.ch, k.flags, k.keycode), (0x61, COMMAND, Some(0)));
        let k = parse(":0x21:0x40000").unwrap();
        assert!(k.char_is_modified && k.ch == 0x21 && k.flags == CONTROL);
        assert!(parse("junk").is_none() && parse("0x61").is_none());
    }

    #[test]
    fn parses_the_short_legacy_form() {
        // Natural Text Editing stores "f702-0x280000" with no 0x on the character.
        let k = parse("f702-0x280000").unwrap();
        assert_eq!((k.ch, k.flags), (0xF702, OPTION | NUMPAD));
    }

    #[test]
    fn arrows_always_carry_numpad() {
        let k = Keystroke::new(0xF702, OPTION, None, 0xF702);
        assert_eq!(k.legacy(), "0xf702-0x280000");
    }

    #[test]
    fn unknown_flags_are_dropped() {
        // The Function flag (0x800000) is not part of a key.
        assert_eq!(Keystroke::new(0x61, COMMAND | 0x800000, None, 0x61).flags, COMMAND);
    }

    #[test]
    fn serializes_like_iterm2() {
        let k = Keystroke::new(0x61, COMMAND | SHIFT, Some(0), 0x41);
        assert_eq!(k.serialized(), "0x61-0x120000-0x0");
        assert_eq!(k.legacy(), "0x61-0x120000");
        assert_eq!(k.modified_key(), ":0x41:0x100000");
        assert_eq!(Keystroke::new(0x61, COMMAND, None, 0x61).serialized(), "0x61-0x100000");
    }

    #[test]
    fn a_three_part_key_matches_a_legacy_press() {
        let mut map = toml::Table::new();
        map.insert("0x61-0x100000-0x0".into(), toml::Value::Table(toml::Table::new()));
        let press = Keystroke::new(0x61, COMMAND, None, 0x61);
        assert_eq!(press.find(&map, false), Some("0x61-0x100000-0x0"));
    }

    #[test]
    fn an_exact_key_wins_over_a_scan() {
        let mut map = toml::Table::new();
        map.insert("0x61-0x100000-0x5".into(), toml::Value::Table(toml::Table::new()));
        map.insert("0x61-0x100000".into(), toml::Value::Table(toml::Table::new()));
        let press = Keystroke::new(0x61, COMMAND, Some(0), 0x61);
        assert_eq!(press.find(&map, false), Some("0x61-0x100000"));
    }

    #[test]
    fn language_agnostic_matches_the_physical_key() {
        let mut map = toml::Table::new();
        map.insert("0x71-0x100000-0xc".into(), toml::Value::Table(toml::Table::new())); // Cmd+Q, kVK_ANSI_Q
        let azerty_a = Keystroke::new(0x61, COMMAND, Some(0x0C), 0x61); // A on the Q key
        assert_eq!(azerty_a.find(&map, true), Some("0x71-0x100000-0xc"));
        assert_eq!(azerty_a.find(&map, false), None);
    }

    #[test]
    fn labels_use_windows_names() {
        assert_eq!(parse("0xf702-0x320000").unwrap().label(), "Win+Shift+Left");
        assert_eq!(parse("0x9-0x40000").unwrap().label(), "Ctrl+Tab");
        assert_eq!(parse("0x19-0x60000").unwrap().label(), "Ctrl+Shift+Tab");
        assert_eq!(parse("0x61-0x1100000").unwrap().label(), "Leader, Win+A");
        assert_eq!(parse("0x35-0x200000").unwrap().label(), "Keypad 5");
        assert_eq!(parse("0xf704-0x80000").unwrap().label(), "Alt+F1");
    }

    #[test]
    fn shift_tab_is_backtab() {
        assert_eq!(mac_char(&crate::keys::Key::Tab, true, false), 0x19);
        assert_eq!(mac_char(&crate::keys::Key::Enter, false, true), 0x03);
        assert_eq!(mac_char(&crate::keys::Key::F(1), false, false), 0xF704);
        assert_eq!(mac_char(&crate::keys::Key::Text("A".into()), true, false), 0x41);
    }

    #[test]
    fn modifier_keycodes_follow_macos() {
        use winit::keyboard::KeyCode::*;
        for (code, want) in [(ShiftLeft, 0x38), (ShiftRight, 0x3C), (ControlLeft, 0x3B), (ControlRight, 0x3E), (AltLeft, 0x3A), (AltRight, 0x3D), (SuperLeft, 0x37), (SuperRight, 0x36), (CapsLock, 0x39)] {
            assert_eq!(mac_modifier_keycode(code), Some(want), "{code:?}");
        }
        assert_eq!(mac_modifier_keycode(KeyA), None);
    }

    #[test]
    fn mods_become_flags() {
        let m = crate::mouse::Mods { shift: true, alt: true, ctrl: false, cmd: true };
        assert_eq!(from_mods(m), SHIFT | OPTION | COMMAND);
    }
}
