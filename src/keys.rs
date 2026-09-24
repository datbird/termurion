//! Turns key presses into the bytes a terminal app expects, and maps iTerm2's
//! Cmd shortcuts (Cmd is Ctrl+Shift, Cmd+Shift is Ctrl+Shift+Alt).

use crate::layout::Dir;
use crate::mouse::Mods;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Key {
    Text(String),
    Enter,
    Backspace,
    Tab,
    Escape,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Insert,
    Delete,
    F(u8),
}

fn has_mods(m: Mods) -> bool {
    m.shift || m.alt || m.ctrl
}

/// xterm's modifier parameter: 1 + shift + 2*alt + 4*ctrl.
fn mod_param(m: Mods) -> u8 {
    1 + m.shift as u8 + 2 * m.alt as u8 + 4 * m.ctrl as u8
}

fn ctrl_byte(c: char) -> Option<u8> {
    match c {
        'a'..='z' => Some(c as u8 - b'a' + 1),
        'A'..='Z' => Some(c as u8 - b'A' + 1),
        '@' | ' ' | '2' => Some(0),
        '[' | '3' => Some(0x1b),
        '\\' | '4' => Some(0x1c),
        ']' | '5' => Some(0x1d),
        '^' | '6' => Some(0x1e),
        '_' | '-' | '7' | '/' => Some(0x1f),
        '8' | '?' => Some(0x7f),
        _ => None,
    }
}

fn tilde(code: u8, mods: Mods) -> Vec<u8> {
    if has_mods(mods) {
        format!("\x1b[{code};{}~", mod_param(mods)).into_bytes()
    } else {
        format!("\x1b[{code}~").into_bytes()
    }
}

/// The bytes for a key press, or None if the key sends nothing.
pub fn encode(key: &Key, mods: Mods, app_cursor: bool) -> Option<Vec<u8>> {
    let alt_prefix = |mut v: Vec<u8>| {
        if mods.alt {
            v.insert(0, 0x1b);
        }
        v
    };
    Some(match key {
        Key::Text(s) if s.is_empty() => return None,
        Key::Text(s) => {
            let mut chars = s.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) if mods.ctrl => alt_prefix(vec![ctrl_byte(c)?]),
                _ => alt_prefix(s.as_bytes().to_vec()),
            }
        }
        Key::Enter => alt_prefix(b"\r".to_vec()),
        Key::Backspace => alt_prefix(vec![if mods.ctrl { 0x08 } else { 0x7f }]),
        Key::Tab if mods.shift => b"\x1b[Z".to_vec(),
        Key::Tab => alt_prefix(b"\t".to_vec()),
        Key::Escape => alt_prefix(b"\x1b".to_vec()),
        Key::Up | Key::Down | Key::Right | Key::Left | Key::Home | Key::End => {
            let c = match key {
                Key::Up => 'A',
                Key::Down => 'B',
                Key::Right => 'C',
                Key::Left => 'D',
                Key::Home => 'H',
                _ => 'F',
            };
            if has_mods(mods) {
                format!("\x1b[1;{}{c}", mod_param(mods)).into_bytes()
            } else if app_cursor {
                format!("\x1bO{c}").into_bytes()
            } else {
                format!("\x1b[{c}").into_bytes()
            }
        }
        Key::PageUp => tilde(5, mods),
        Key::PageDown => tilde(6, mods),
        Key::Insert => tilde(2, mods),
        Key::Delete => tilde(3, mods),
        Key::F(n @ 1..=4) => {
            let c = ['P', 'Q', 'R', 'S'][(*n - 1) as usize];
            if has_mods(mods) {
                format!("\x1b[1;{}{c}", mod_param(mods)).into_bytes()
            } else {
                format!("\x1bO{c}").into_bytes()
            }
        }
        Key::F(n @ 5..=12) => tilde([15, 17, 18, 19, 20, 21, 23, 24][(*n - 5) as usize], mods),
        Key::F(_) => return None,
    })
}

/// The bytes for a key press, given the key (`key`), the same key with no
/// modifiers (`base`), and the text Windows produced for the press (`text`).
/// That text already holds dead-key and AltGr results, so for character keys it
/// wins, except when Ctrl makes a control code. Windows reports AltGr as Ctrl+Alt;
/// it is AltGr when the text differs from the base key.
pub fn press_bytes(key: &Key, base: Option<&Key>, text: Option<&str>, mods: Mods, app_cursor: bool) -> Option<Vec<u8>> {
    let typed = text.filter(|t| !t.is_empty() && !t.chars().any(char::is_control));
    if let (Key::Text(_), Some(t)) = (key, typed) {
        let altgr = mods.ctrl && mods.alt && base != Some(&Key::Text(t.to_lowercase()));
        if altgr {
            return Some(t.as_bytes().to_vec());
        }
        if !mods.ctrl {
            let mut bytes = t.as_bytes().to_vec();
            if mods.alt {
                bytes.insert(0, 0x1b);
            }
            return Some(bytes);
        }
    }
    encode(key, mods, app_cursor)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    NewTab,
    ClosePane,
    SplitRight,
    SplitDown,
    NextPane,
    PrevPane,
    Focus(Dir),
    NextTab,
    PrevTab,
    SelectTab(usize),
    Copy,
    Paste,
    ScrollPageUp,
    ScrollPageDown,
    ScrollTop,
    ScrollBottom,
    OpenSettings,
    OpenProfiles,
}

/// The app shortcut for a key, if any. `key` must be the key with no modifiers
/// applied (so Ctrl+Shift+T arrives as "t" or "T").
pub fn shortcut(key: &Key, mods: Mods) -> Option<Action> {
    // iTerm2's Cmd chords are Win chords from the menu table (menus.rs). These are the
    // Windows extras: Ctrl+Shift+C and V, Ctrl+Tab, and Shift+PageUp and PageDown.
    let ctrl_shift = mods.ctrl && mods.shift && !mods.alt && !mods.cmd;
    let shift_only = mods.shift && !mods.ctrl && !mods.alt && !mods.cmd;
    let text = match key {
        Key::Text(s) => s.to_lowercase(),
        _ => String::new(),
    };
    Some(match (key, text.as_str()) {
        (_, "c") if ctrl_shift => Action::Copy,
        (_, "v") if ctrl_shift => Action::Paste,
        (Key::Tab, _) if mods.ctrl && !mods.alt && !mods.cmd => {
            if mods.shift {
                Action::PrevTab
            } else {
                Action::NextTab
            }
        }
        (Key::PageUp, _) if shift_only => Action::ScrollPageUp,
        (Key::PageDown, _) if shift_only => Action::ScrollPageDown,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONE: Mods = Mods { shift: false, alt: false, ctrl: false, cmd: false };
    const CTRL: Mods = Mods { shift: false, alt: false, ctrl: true, cmd: false };
    const ALT: Mods = Mods { shift: false, alt: true, ctrl: false, cmd: false };
    const SHIFT: Mods = Mods { shift: true, alt: false, ctrl: false, cmd: false };
    const CTRL_SHIFT: Mods = Mods { shift: true, alt: false, ctrl: true, cmd: false };

    fn t(s: &str) -> Key {
        Key::Text(s.into())
    }

    fn enc(k: Key, m: Mods) -> Vec<u8> {
        encode(&k, m, false).unwrap()
    }

    #[test]
    fn text_and_control_keys() {
        assert_eq!(enc(t("a"), NONE), b"a");
        assert_eq!(enc(t("c"), CTRL), b"\x03");
        assert_eq!(enc(t("["), CTRL), b"\x1b");
        assert_eq!(enc(t(" "), CTRL), b"\x00");
        assert_eq!(enc(t("x"), ALT), b"\x1bx");
        assert_eq!(enc(Key::Enter, NONE), b"\r");
        assert_eq!(enc(Key::Backspace, NONE), b"\x7f");
        assert_eq!(enc(Key::Backspace, CTRL), b"\x08");
        assert_eq!(enc(Key::Tab, SHIFT), b"\x1b[Z");
        assert_eq!(encode(&t(""), NONE, false), None);
    }

    #[test]
    fn arrows_follow_cursor_mode_and_modifiers() {
        assert_eq!(enc(Key::Up, NONE), b"\x1b[A");
        assert_eq!(encode(&Key::Up, NONE, true).unwrap(), b"\x1bOA");
        assert_eq!(enc(Key::Right, CTRL), b"\x1b[1;5C");
        assert_eq!(enc(Key::Home, SHIFT), b"\x1b[1;2H");
    }

    #[test]
    fn editing_and_function_keys() {
        assert_eq!(enc(Key::PageUp, NONE), b"\x1b[5~");
        assert_eq!(enc(Key::Delete, CTRL), b"\x1b[3;5~");
        assert_eq!(enc(Key::F(1), NONE), b"\x1bOP");
        assert_eq!(enc(Key::F(4), SHIFT), b"\x1b[1;2S");
        assert_eq!(enc(Key::F(5), NONE), b"\x1b[15~");
        assert_eq!(enc(Key::F(12), NONE), b"\x1b[24~");
        assert_eq!(encode(&Key::F(13), NONE, false), None);
    }

    fn press(key: &str, base: &str, text: &str, m: Mods) -> Vec<u8> {
        press_bytes(&t(key), Some(&t(base)), Some(text), m, false).unwrap()
    }

    #[test]
    fn altgr_sends_the_typed_character() {
        // German layout: AltGr+Q types @. Windows reports AltGr as Ctrl+Alt.
        let altgr = Mods { ctrl: true, alt: true, shift: false, cmd: false };
        assert_eq!(press("@", "q", "@", altgr), b"@");
        assert_eq!(press("{", "7", "{", altgr), b"{");
    }

    #[test]
    fn ctrl_alt_letter_is_still_a_control_key() {
        let ctrl_alt = Mods { ctrl: true, alt: true, shift: false, cmd: false };
        assert_eq!(press("a", "a", "a", ctrl_alt), b"\x1b\x01");
    }

    #[test]
    fn composed_text_wins_over_the_key() {
        assert_eq!(press("e", "e", "\u{e9}", NONE), "\u{e9}".as_bytes());
        assert_eq!(press("A", "a", "A", SHIFT), b"A");
    }

    #[test]
    fn alt_and_ctrl_keep_their_meaning() {
        assert_eq!(press("x", "x", "x", ALT), b"\x1bx");
        assert_eq!(press("c", "c", "c", CTRL), b"\x03");
        assert_eq!(press_bytes(&Key::Enter, None, Some("\r"), NONE, false).unwrap(), b"\r");
    }

    #[test]
    fn windows_extras() {
        assert_eq!(shortcut(&t("c"), CTRL_SHIFT), Some(Action::Copy));
        assert_eq!(shortcut(&t("v"), CTRL_SHIFT), Some(Action::Paste));
        assert_eq!(shortcut(&Key::Tab, CTRL), Some(Action::NextTab));
        assert_eq!(shortcut(&Key::PageUp, SHIFT), Some(Action::ScrollPageUp));
        assert_eq!(shortcut(&t("t"), CTRL_SHIFT), None, "Cmd chords come from the menu table now");
    }

    #[test]
    fn plain_keys_are_not_shortcuts() {
        assert_eq!(shortcut(&t("t"), CTRL), None);
        assert_eq!(shortcut(&t("c"), CTRL), None);
        assert_eq!(shortcut(&t("t"), NONE), None);
        assert_eq!(shortcut(&Key::PageUp, NONE), None);
    }
}
