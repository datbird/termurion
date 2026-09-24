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
}

/// The app shortcut for a key, if any. `key` must be the key with no modifiers
/// applied (so Ctrl+Shift+T arrives as "t" or "T").
pub fn shortcut(key: &Key, mods: Mods) -> Option<Action> {
    let cmd = mods.ctrl && mods.shift && !mods.alt;
    let cmd_shift = mods.ctrl && mods.shift && mods.alt;
    let shift_only = mods.shift && !mods.ctrl && !mods.alt;
    let text = match key {
        Key::Text(s) => s.to_lowercase(),
        _ => String::new(),
    };
    Some(match (key, text.as_str()) {
        (_, "t") if cmd => Action::NewTab,
        (_, "w") if cmd => Action::ClosePane,
        (_, "d") if cmd => Action::SplitRight,
        (_, "d") if cmd_shift => Action::SplitDown,
        (_, "]") if cmd => Action::NextPane,
        (_, "[") if cmd => Action::PrevPane,
        (_, "]") if cmd_shift => Action::NextTab,
        (_, "[") if cmd_shift => Action::PrevTab,
        (_, "c") if cmd => Action::Copy,
        (_, "v") if cmd => Action::Paste,
        (_, d) if cmd && d.len() == 1 && matches!(d.as_bytes()[0], b'1'..=b'9') => {
            Action::SelectTab((d.as_bytes()[0] - b'1') as usize)
        }
        (Key::Tab, _) if mods.ctrl && !mods.alt => {
            if mods.shift {
                Action::PrevTab
            } else {
                Action::NextTab
            }
        }
        (Key::Left, _) if cmd_shift => Action::Focus(Dir::Left),
        (Key::Right, _) if cmd_shift => Action::Focus(Dir::Right),
        (Key::Up, _) if cmd_shift => Action::Focus(Dir::Up),
        (Key::Down, _) if cmd_shift => Action::Focus(Dir::Down),
        (Key::PageUp, _) if shift_only => Action::ScrollPageUp,
        (Key::PageDown, _) if shift_only => Action::ScrollPageDown,
        (Key::Home, _) if cmd => Action::ScrollTop,
        (Key::End, _) if cmd => Action::ScrollBottom,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONE: Mods = Mods { shift: false, alt: false, ctrl: false };
    const CTRL: Mods = Mods { shift: false, alt: false, ctrl: true };
    const ALT: Mods = Mods { shift: false, alt: true, ctrl: false };
    const SHIFT: Mods = Mods { shift: true, alt: false, ctrl: false };
    const CMD: Mods = Mods { shift: true, alt: false, ctrl: true };
    const CMD_SHIFT: Mods = Mods { shift: true, alt: true, ctrl: true };

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

    #[test]
    fn cmd_shortcuts() {
        assert_eq!(shortcut(&t("t"), CMD), Some(Action::NewTab));
        assert_eq!(shortcut(&t("T"), CMD), Some(Action::NewTab));
        assert_eq!(shortcut(&t("w"), CMD), Some(Action::ClosePane));
        assert_eq!(shortcut(&t("d"), CMD), Some(Action::SplitRight));
        assert_eq!(shortcut(&t("d"), CMD_SHIFT), Some(Action::SplitDown));
        assert_eq!(shortcut(&t("]"), CMD), Some(Action::NextPane));
        assert_eq!(shortcut(&t("]"), CMD_SHIFT), Some(Action::NextTab));
        assert_eq!(shortcut(&t("3"), CMD), Some(Action::SelectTab(2)));
        assert_eq!(shortcut(&t("c"), CMD), Some(Action::Copy));
        assert_eq!(shortcut(&t("v"), CMD), Some(Action::Paste));
        assert_eq!(shortcut(&Key::Left, CMD_SHIFT), Some(Action::Focus(Dir::Left)));
        assert_eq!(shortcut(&Key::Tab, CTRL), Some(Action::NextTab));
        assert_eq!(shortcut(&Key::PageUp, SHIFT), Some(Action::ScrollPageUp));
        assert_eq!(shortcut(&Key::Home, CMD), Some(Action::ScrollTop));
    }

    #[test]
    fn plain_keys_are_not_shortcuts() {
        assert_eq!(shortcut(&t("t"), CTRL), None);
        assert_eq!(shortcut(&t("c"), CTRL), None);
        assert_eq!(shortcut(&t("t"), NONE), None);
        assert_eq!(shortcut(&Key::PageUp, NONE), None);
    }
}
