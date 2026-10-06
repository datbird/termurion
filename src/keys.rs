//! Turns key presses into the bytes a terminal app expects, with the profile's Keys
//! options (Option Key Sends, Treat Option as Alt, Application Keypad), and the
//! Windows-only shortcuts. iTerm2's Cmd chords are Win chords (menus.rs, winkeys.rs).

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

/// How Alt changes a key: iTerm2's Option Key Sends (0 Normal, 1 Meta, 2 Esc+), and the
/// value Alt adds to xterm's modifier parameter (2 as Alt, 8 as Meta).
#[derive(Clone, Copy)]
struct Enc {
    alt_sends: i64,
    alt_param: u8,
}

const XTERM: Enc = Enc { alt_sends: 2, alt_param: 2 };

/// xterm's modifier parameter: 1 + shift + 2*alt (8 with Meta semantics) + 4*ctrl.
fn mod_param(m: Mods, e: Enc) -> u8 {
    1 + m.shift as u8 + e.alt_param * m.alt as u8 + 4 * m.ctrl as u8
}

/// Applies Option Key Sends to the bytes of a key pressed with Alt.
fn with_alt(mut v: Vec<u8>, mods: Mods, e: Enc) -> Vec<u8> {
    if mods.alt {
        match e.alt_sends {
            // Normal on macOS types the Option layer's character. Windows has none (AltGr
            // text arrives as text), so Normal sends Esc+, as Windows terminals do.
            1 => {
                if let Some(b) = v.first_mut() {
                    *b |= 0x80;
                }
            }
            _ => v.insert(0, 0x1b),
        }
    }
    v
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

fn tilde(code: u8, mods: Mods, e: Enc) -> Vec<u8> {
    if has_mods(mods) {
        format!("\x1b[{code};{}~", mod_param(mods, e)).into_bytes()
    } else {
        format!("\x1b[{code}~").into_bytes()
    }
}

/// The bytes for a key press, or None if the key sends nothing.
pub fn encode(key: &Key, mods: Mods, app_cursor: bool) -> Option<Vec<u8>> {
    encode_with(key, mods, app_cursor, XTERM)
}

fn encode_with(key: &Key, mods: Mods, app_cursor: bool, e: Enc) -> Option<Vec<u8>> {
    let alt_prefix = |v: Vec<u8>| with_alt(v, mods, e);
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
                format!("\x1b[1;{}{c}", mod_param(mods, e)).into_bytes()
            } else if app_cursor {
                format!("\x1bO{c}").into_bytes()
            } else {
                format!("\x1b[{c}").into_bytes()
            }
        }
        Key::PageUp => tilde(5, mods, e),
        Key::PageDown => tilde(6, mods, e),
        Key::Insert => tilde(2, mods, e),
        Key::Delete => tilde(3, mods, e),
        Key::F(n @ 1..=4) => {
            let c = ['P', 'Q', 'R', 'S'][(*n - 1) as usize];
            if has_mods(mods) {
                format!("\x1b[1;{}{c}", mod_param(mods, e)).into_bytes()
            } else {
                format!("\x1bO{c}").into_bytes()
            }
        }
        Key::F(n @ 5..=12) => tilde([15, 17, 18, 19, 20, 21, 23, 24][(*n - 5) as usize], mods, e),
        Key::F(_) => return None,
    })
}

/// The bytes for a key press, given the key (`key`), the same key with no
/// modifiers (`base`), and the text Windows produced for the press (`text`).
/// That text already holds dead-key and AltGr results, so for character keys it
/// wins, except when Ctrl makes a control code. Windows reports AltGr as Ctrl+Alt;
/// it is AltGr when the text differs from the base key.
pub fn press_bytes(key: &Key, base: Option<&Key>, text: Option<&str>, mods: Mods, app_cursor: bool) -> Option<Vec<u8>> {
    let xterm = KeyOptions { left_option: 2, right_option: 2, option_is_alt: true, app_keypad_allowed: false, left_changeable: true, right_changeable: false };
    press_bytes_with(key, base, text, mods, AltSide::Left, app_cursor, false, false, &xterm)
}

/// Which Alt key is down; each side has its own Option Key Sends setting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AltSide {
    Left,
    Right,
}

/// The profile's Keys options that change what a key sends.
#[derive(Clone, Copy, Debug)]
pub struct KeyOptions {
    /// Option Key Sends: 0 Normal, 1 Meta (high bit), 2 Esc+. Normal acts as Esc+ on Windows.
    pub left_option: i64,
    pub right_option: i64,
    /// Treat Option as Alt: Alt counts 2 in xterm's modifier parameter; off, it counts 8 (Meta).
    pub option_is_alt: bool,
    /// Application Keypad Allowed: apps may put the keypad in application mode.
    pub app_keypad_allowed: bool,
    /// Left/Right Option Key Changeable: a program's DECSET 1036 may switch that side to Esc+.
    pub left_changeable: bool,
    pub right_changeable: bool,
}

impl KeyOptions {
    pub fn from_profile(p: &crate::profile::Profile) -> KeyOptions {
        let left = p.int("Option Key Sends");
        // A missing right-side setting follows the left one (PTYSession).
        let right = p.values.get("Right Option Key Sends").and_then(|v| v.as_integer()).unwrap_or(left);
        KeyOptions { left_option: left, right_option: right, option_is_alt: p.bool("Treat Option as Alt"), app_keypad_allowed: p.bool("Application Keypad Allowed"), left_changeable: p.bool("Left Option Key Changeable"), right_changeable: p.bool("Right Option Key Changeable") }
    }

    /// The Option Key Sends value for `side` once a program's DECSET 1036 is counted (`PS:12940-12972`).
    /// `support` is the Advanced SupportDecsetMetaSendsEscape key. A side whose own setting is
    /// Esc+ already, or any Esc+ side at all, keeps the profile's values.
    pub fn effective(&self, side: AltSide, meta_sends_escape: bool, support: bool) -> i64 {
        let (own, changeable) = match side {
            AltSide::Left => (self.left_option, self.left_changeable),
            AltSide::Right => (self.right_option, self.right_changeable),
        };
        if support && self.left_option != 2 && self.right_option != 2 && meta_sends_escape && changeable {
            2
        } else {
            own
        }
    }
}

/// The SS3 final byte for a keypad key in application keypad mode (VT100Output keypadDataForString:).
fn keypad_code(key: &Key) -> Option<char> {
    Some(match key {
        Key::Enter => 'M',
        Key::Text(s) => match s.as_str() {
            "0" => 'p', "1" => 'q', "2" => 'r', "3" => 's', "4" => 't', "5" => 'u', "6" => 'v', "7" => 'w',
            "8" => 'x', "9" => 'y', "-" => 'm', "+" => 'k', "." => 'n', "/" => 'o', "*" => 'j', "=" => 'X',
            _ => return None,
        },
        _ => return None,
    })
}

/// `press_bytes` with the profile's Keys options. `keypad` is a key on the numeric keypad,
/// and `app_keypad` is the terminal's application keypad mode (DECKPAM).
#[allow(clippy::too_many_arguments)]
pub fn press_bytes_with(
    key: &Key,
    base: Option<&Key>,
    text: Option<&str>,
    mods: Mods,
    side: AltSide,
    app_cursor: bool,
    app_keypad: bool,
    keypad: bool,
    opts: &KeyOptions,
) -> Option<Vec<u8>> {
    let alt_sends = match side {
        AltSide::Left => opts.left_option,
        AltSide::Right => opts.right_option,
    };
    let e = Enc { alt_sends, alt_param: if opts.option_is_alt { 2 } else { 8 } };
    if keypad && app_keypad && opts.app_keypad_allowed {
        if let Some(c) = keypad_code(key) {
            let m = if has_mods(mods) { mod_param(mods, e).to_string() } else { String::new() };
            return Some(format!("\x1bO{m}{c}").into_bytes());
        }
    }
    let typed = text.filter(|t| !t.is_empty() && !t.chars().any(char::is_control));
    if let (Key::Text(_), Some(t)) = (key, typed) {
        let base_char = match base {
            Some(Key::Text(b)) => b.chars().next(),
            _ => None,
        };
        let altgr = crate::keyproto::is_altgr(mods, Some(t), base_char);
        if altgr {
            return Some(t.as_bytes().to_vec());
        }
        if !mods.ctrl {
            return Some(with_alt(t.as_bytes().to_vec(), mods, e));
        }
    }
    encode_with(key, mods, app_cursor, e)
}

/// Movement Keys Scroll Outside Interactive Apps: with no modifiers and no alternate
/// screen, Page Up and Down, Home and End scroll; Up and Down scroll once the session ended.
pub fn movement_scroll(key: &Key, mods: Mods, alt_screen: bool, exited: bool) -> Option<alacritty_terminal::grid::Scroll> {
    use alacritty_terminal::grid::Scroll;
    if has_mods(mods) || mods.cmd || alt_screen {
        return None;
    }
    Some(match key {
        Key::PageUp => Scroll::PageUp,
        Key::PageDown => Scroll::PageDown,
        Key::Home => Scroll::Top,
        Key::End => Scroll::Bottom,
        Key::Up if exited => Scroll::Delta(1),
        Key::Down if exited => Scroll::Delta(-1),
        _ => return None,
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

    const OPTS: KeyOptions = KeyOptions { left_option: 2, right_option: 2, option_is_alt: true, app_keypad_allowed: false, left_changeable: true, right_changeable: false };

    fn with(key: Key, text: Option<&str>, mods: Mods, side: AltSide, app_keypad: bool, keypad: bool, o: &KeyOptions) -> Option<Vec<u8>> {
        let base = match &key {
            Key::Text(s) => Some(Key::Text(s.to_lowercase())),
            k => Some(k.clone()),
        };
        press_bytes_with(&key, base.as_ref(), text, mods, side, false, app_keypad, keypad, o)
    }

    /// 13u Task 2 (Review Focus 2): `PS:12940-12972`.
    #[test]
    fn effective_option_follows_iterm2() {
        let base = KeyOptions { left_option: 0, right_option: 0, left_changeable: true, right_changeable: false, ..OPTS };
        assert_eq!(base.effective(AltSide::Left, true, true), 2);
        assert_eq!(base.effective(AltSide::Left, true, false), 0, "support off");
        assert_eq!(base.effective(AltSide::Left, false, true), 0, "mode off");
        assert_eq!(base.effective(AltSide::Right, true, true), 0, "right not changeable");
        let right = KeyOptions { right_changeable: true, ..base };
        assert_eq!(right.effective(AltSide::Right, true, true), 2);
        let none = KeyOptions { left_changeable: false, ..base };
        assert_eq!(none.effective(AltSide::Left, true, true), 0, "left not changeable");
        let esc = KeyOptions { left_option: 2, right_option: 2, ..base };
        assert_eq!(esc.effective(AltSide::Left, true, true), 2);
        assert_eq!(esc.effective(AltSide::Right, true, true), 2);
        // Right set to Esc+: the mode never changes the left side.
        let r_esc = KeyOptions { right_option: 2, ..base };
        assert_eq!(r_esc.effective(AltSide::Left, true, true), 0);
        // Meta (1) is its own setting and is changed to Esc+ like Normal.
        let meta = KeyOptions { left_option: 1, ..base };
        assert_eq!(meta.effective(AltSide::Left, true, true), 2);
        assert_eq!(meta.effective(AltSide::Left, false, true), 1);
    }

    #[test]
    fn option_normal_acts_as_esc_plus_on_windows() {
        // Windows has no Option character layer (AltGr does that job), so Normal sends Esc+.
        let o = KeyOptions { left_option: 0, ..OPTS };
        assert_eq!(with(t("b"), Some("b"), ALT, AltSide::Left, false, false, &o), Some(b"\x1bb".to_vec()));
        assert_eq!(with(Key::Backspace, None, ALT, AltSide::Left, false, false, &o), Some(vec![0x1b, 0x7f]));
    }

    #[test]
    fn option_meta_sets_the_high_bit() {
        let o = KeyOptions { left_option: 1, ..OPTS };
        assert_eq!(with(t("b"), Some("b"), ALT, AltSide::Left, false, false, &o), Some(vec![b'b' | 0x80]));
    }

    #[test]
    fn option_esc_plus_prefixes_escape() {
        assert_eq!(with(t("b"), Some("b"), ALT, AltSide::Left, false, false, &OPTS), Some(b"\x1bb".to_vec()));
        assert_eq!(with(t("a"), None, Mods { ctrl: true, ..ALT }, AltSide::Left, false, false, &OPTS), Some(vec![0x1b, 0x01]));
    }

    #[test]
    fn each_side_uses_its_own_setting() {
        let o = KeyOptions { left_option: 1, right_option: 2, ..OPTS };
        assert_eq!(with(t("b"), Some("b"), ALT, AltSide::Right, false, false, &o), Some(b"\x1bb".to_vec()));
        assert_eq!(with(t("b"), Some("b"), ALT, AltSide::Left, false, false, &o), Some(vec![b'b' | 0x80]));
    }

    #[test]
    fn meta_style_option_changes_the_modifier_parameter() {
        let o = KeyOptions { option_is_alt: false, ..OPTS };
        assert_eq!(with(Key::F(1), None, ALT, AltSide::Left, false, false, &o), Some(b"\x1b[1;9P".to_vec()));
        assert_eq!(with(Key::F(1), None, ALT, AltSide::Left, false, false, &OPTS), Some(b"\x1b[1;3P".to_vec()));
        assert_eq!(with(Key::Up, None, Mods { shift: true, ..ALT }, AltSide::Left, false, false, &o), Some(b"\x1b[1;10A".to_vec()));
    }

    #[test]
    fn keypad_needs_permission_for_application_mode() {
        let allowed = KeyOptions { app_keypad_allowed: true, ..OPTS };
        assert_eq!(with(t("5"), Some("5"), NONE, AltSide::Left, true, true, &allowed), Some(b"\x1bOu".to_vec()));
        assert_eq!(with(t("5"), Some("5"), NONE, AltSide::Left, true, true, &OPTS), Some(b"5".to_vec()));
        assert_eq!(with(Key::Enter, None, NONE, AltSide::Left, true, true, &allowed), Some(b"\x1bOM".to_vec()));
        assert_eq!(with(t("+"), Some("+"), SHIFT, AltSide::Left, true, true, &allowed), Some(b"\x1bO2k".to_vec()));
        // Application mode off: a keypad digit is just the digit.
        assert_eq!(with(t("5"), Some("5"), NONE, AltSide::Left, false, true, &allowed), Some(b"5".to_vec()));
    }

    #[test]
    fn movement_keys_scroll_only_outside_apps() {
        use alacritty_terminal::grid::Scroll;
        assert!(matches!(movement_scroll(&Key::PageUp, NONE, false, false), Some(Scroll::PageUp)));
        assert!(matches!(movement_scroll(&Key::Home, NONE, false, false), Some(Scroll::Top)));
        assert!(movement_scroll(&Key::PageUp, NONE, true, false).is_none());
        assert!(movement_scroll(&Key::PageUp, SHIFT, false, false).is_none());
        assert!(movement_scroll(&Key::Up, NONE, false, false).is_none());
        assert!(matches!(movement_scroll(&Key::Up, NONE, false, true), Some(Scroll::Delta(1))));
    }

    #[test]
    fn options_read_the_profile() {
        let mut p = crate::profile::Profile::stock();
        p.set("Option Key Sends", toml::Value::Integer(2));
        let o = KeyOptions::from_profile(&p);
        assert_eq!((o.left_option, o.right_option, o.option_is_alt, o.app_keypad_allowed), (2, 2, true, false));
        p.set("Right Option Key Sends", toml::Value::Integer(1));
        assert_eq!(KeyOptions::from_profile(&p).right_option, 1);
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
