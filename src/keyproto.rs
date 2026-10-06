//! The keyboard protocols a program can ask for, as iTerm2 encodes them: xterm's
//! modifyOtherKeys (modes 1 and 2), CSI u (fixterms), the kitty keyboard protocol, and
//! iTerm2's raw key reporting.

use crate::keys::Key;
use crate::mouse::Mods;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Press,
    Repeat,
    Release,
}

/// One key event. `base` is the key's character without modifiers (from the layout);
/// `text` is what it types; `altgr` marks text typed with AltGr (Ctrl+Alt on Windows).
#[derive(Clone, Copy, Debug)]
pub struct KeyEvent<'a> {
    pub key: &'a Key,
    pub base: Option<char>,
    pub text: Option<&'a str>,
    pub mods: Mods,
    pub kind: Kind,
    pub altgr: bool,
    pub vk: u16,
    /// Which Control and Win keys are down (the modifier bits cannot say left from right).
    pub sides: Sides,
    /// The pane's Control and Win key mappings.
    pub bucky: Bucky,
}

/// Which Control and Win keys are down, left and right apart.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Sides {
    pub lctrl: bool,
    pub rctrl: bool,
    pub lwin: bool,
    pub rwin: bool,
}

/// The profile's Left/Right Control and Win (Command) key mappings: 0 Normal, 1 Hyper, 2 Meta, 3 Super.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Bucky {
    pub left_ctrl: u8,
    pub right_ctrl: u8,
    pub left_win: u8,
    pub right_win: u8,
}

impl Bucky {
    pub fn from_profile(p: &crate::profile::Profile) -> Bucky {
        let v = |k: &str| p.int(k).clamp(0, 3) as u8;
        Bucky { left_ctrl: v("Left Control Key"), right_ctrl: v("Right Control Key"), left_win: v("Left Command Key"), right_win: v("Right Command Key") }
    }
}

/// kitty's modifier bits (shift 1, alt 2, ctrl 4, super 8, hyper 16, meta 32) for a key event
/// (`iTermModernKeyMapper.swift:1717-1860`). A down Control or Win key adds its own bit, or the
/// bit of the bucky it is mapped to; Win alone adds nothing. With no side known (a synthetic
/// event) a held Control counts as plain Control.
pub fn cook(mods: Mods, sides: Sides, b: Bucky) -> u32 {
    fn side(map: u8, plain: u32) -> u32 {
        match map {
            1 => 16,
            2 => 32,
            3 => 8,
            _ => plain,
        }
    }
    let mut bits = mods.shift as u32 + 2 * mods.alt as u32;
    if sides.lctrl {
        bits |= side(b.left_ctrl, 4);
    }
    if sides.rctrl {
        bits |= side(b.right_ctrl, 4);
    }
    if mods.ctrl && !sides.lctrl && !sides.rctrl {
        bits |= 4;
    }
    if sides.lwin {
        bits |= side(b.left_win, 0);
    }
    if sides.rwin {
        bits |= side(b.right_win, 0);
    }
    bits
}

/// Which protocol reports keys, besides kitty's flags.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum KeyMode {
    #[default]
    Standard,
    Mok1,
    Mok2,
    CsiU,
    Raw,
}

/// A pane's key reporting: the mode, and kitty's flags (which win when any is set).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Keyboard {
    pub mode: KeyMode,
    pub kitty: u8,
}

impl Keyboard {
    /// Whether key releases go to the program.
    pub fn wants_releases(&self) -> bool {
        self.kitty & EVENT_TYPES != 0 || (self.kitty == 0 && self.mode == KeyMode::Raw)
    }
}

/// The bytes a key event sends under `kb`. None: the standard bytes (presses only).
pub fn encode(kb: Keyboard, ev: &KeyEvent) -> Option<Vec<u8>> {
    if kb.kitty != 0 {
        return kitty(ev, kb.kitty);
    }
    match kb.mode {
        KeyMode::Raw => Some(raw(ev)),
        KeyMode::Mok1 => modify_other_keys(ev, 1),
        KeyMode::Mok2 => modify_other_keys(ev, 2),
        KeyMode::CsiU => csi_u(ev),
        KeyMode::Standard if ev.kind == Kind::Release => Some(Vec::new()),
        KeyMode::Standard => None,
    }
}

/// Whether a key typed with Ctrl and Alt down is AltGr's character: its text is not the
/// key's own (base) character. winit's text ignores Ctrl, so Ctrl+Alt+A still says "a".
pub fn is_altgr(mods: Mods, text: Option<&str>, base: Option<char>) -> bool {
    let Some(t) = text.filter(|t| !t.is_empty() && !t.chars().any(char::is_control)) else { return false };
    mods.ctrl && mods.alt && base.map(|b| b.to_string()) != Some(t.to_lowercase())
}

/// The kitty flags.
pub const DISAMBIGUATE: u8 = 1;
pub const EVENT_TYPES: u8 = 2;
pub const ALTERNATE_KEYS: u8 = 4;
pub const ALL_AS_ESCAPES: u8 = 8;
pub const ASSOCIATED_TEXT: u8 = 16;

/// xterm's modifier parameter: 1 + Shift 1 + Alt 2 + Ctrl 4. Win is never counted.
fn param(m: Mods) -> u32 {
    1 + m.shift as u32 + 2 * m.alt as u32 + 4 * m.ctrl as u32
}

fn modified(m: Mods) -> bool {
    m.shift || m.alt || m.ctrl
}

/// The character a text key stands for: its text, else its base character.
fn text_char(ev: &KeyEvent) -> Option<char> {
    match ev.key {
        Key::Text(t) => t.chars().next().or(ev.base),
        _ => None,
    }
}

/// The key's own character, without Shift: the layout's base character when known.
fn base_char(ev: &KeyEvent) -> Option<char> {
    match ev.key {
        Key::Text(t) => ev.base.or_else(|| t.chars().next().map(|c| c.to_lowercase().next().unwrap_or(c))),
        _ => None,
    }
}

/// The code of Enter, Tab, Backspace, and Escape.
fn control_code(key: &Key) -> Option<u32> {
    match key {
        Key::Enter => Some(13),
        Key::Tab => Some(9),
        Key::Backspace => Some(127),
        Key::Escape => Some(27),
        _ => None,
    }
}

fn altgr_text(ev: &KeyEvent) -> Option<Vec<u8>> {
    if ev.kind == Kind::Release {
        return Some(Vec::new());
    }
    Some(ev.text.unwrap_or("").as_bytes().to_vec())
}

/// xterm modifyOtherKeys: `CSI 27 ; m ; code ~` for modified keys (all of them in mode 2,
/// only the ones the standard bytes cannot tell apart in mode 1). None: send the standard
/// bytes. Some(empty): send nothing (a release).
pub fn modify_other_keys(ev: &KeyEvent, level: u8) -> Option<Vec<u8>> {
    if ev.altgr {
        return altgr_text(ev);
    }
    if ev.kind == Kind::Release {
        return Some(Vec::new());
    }
    let m = ev.mods;
    if !modified(m) || level == 0 {
        return None;
    }
    let code = if let Some(c) = control_code(ev.key) {
        if matches!(ev.key, Key::Tab) && m.shift && !m.alt && !m.ctrl {
            return None;
        }
        if level == 1 && !m.ctrl && !m.shift {
            return None;
        }
        c
    } else if let Some(c) = text_char(ev) {
        if m.shift && !m.alt && !m.ctrl && c != ' ' {
            return None;
        }
        if level == 1 {
            let b = base_char(ev).unwrap_or(c);
            let has_control = b.is_ascii_alphabetic() || matches!(b, ' ' | '@' | '[' | '\\' | ']' | '^' | '_' | '?');
            if !m.ctrl || has_control {
                return None;
            }
        }
        c as u32
    } else {
        return None;
    };
    Some(format!("\x1b[27;{};{}~", param(m), code).into_bytes())
}

/// CSI u as iTerm2's "Report keys using CSI u" (fixterms / libtickit) sends it.
pub fn csi_u(ev: &KeyEvent) -> Option<Vec<u8>> {
    if ev.altgr {
        return altgr_text(ev);
    }
    if ev.kind == Kind::Release {
        return Some(Vec::new());
    }
    let m = ev.mods;
    if !modified(m) {
        return None;
    }
    if let Key::Tab = ev.key {
        if m.shift && !m.alt && !m.ctrl {
            return Some(b"\x1b[Z".to_vec());
        }
        return Some(format!("\x1b[1;{}Z", param(m)).into_bytes());
    }
    if let Some(code) = control_code(ev.key) {
        return Some(format!("\x1b[{};{}u", code, param(m)).into_bytes());
    }
    let c = text_char(ev)?;
    if m.shift && !m.alt && !m.ctrl {
        return None;
    }
    let b = base_char(ev).unwrap_or(c);
    if m.ctrl && !m.alt && !m.shift {
        if b == ' ' {
            return Some(vec![0]);
        }
        if b.is_ascii_lowercase() && !matches!(b, 'i' | 'j' | 'm') {
            return Some(vec![b as u8 - b'a' + 1]);
        }
    }
    Some(format!("\x1b[{};{}u", b as u32, param(m)).into_bytes())
}

/// How kitty writes a key: its number and final byte.
enum Form {
    /// `CSI code ... u`
    U(u32),
    /// `CSI 1 ; mods X`, or `CSI X` when there is nothing to add.
    Letter(char),
    /// `CSI n ; mods ~`
    Tilde(u32),
}

fn kitty_form(ev: &KeyEvent) -> Option<Form> {
    Some(match ev.key {
        Key::Text(_) => Form::U(base_char(ev)? as u32),
        Key::Enter => Form::U(13),
        Key::Tab => Form::U(9),
        Key::Backspace => Form::U(127),
        Key::Escape => Form::U(27),
        Key::Up => Form::Letter('A'),
        Key::Down => Form::Letter('B'),
        Key::Right => Form::Letter('C'),
        Key::Left => Form::Letter('D'),
        Key::Home => Form::Letter('H'),
        Key::End => Form::Letter('F'),
        Key::Insert => Form::Tilde(2),
        Key::Delete => Form::Tilde(3),
        Key::PageUp => Form::Tilde(5),
        Key::PageDown => Form::Tilde(6),
        Key::F(n @ 1..=4) => Form::Tilde(10 + *n as u32),
        Key::F(n @ 5..=12) => Form::Tilde([15, 17, 18, 19, 20, 21, 23, 24][*n as usize - 5]),
        Key::F(n) if *n >= 13 && *n <= 35 => Form::U(57376 + (*n as u32 - 13)),
        Key::F(_) => return None,
    })
}

/// The kitty keyboard protocol with `flags` (non-zero).
pub fn kitty(ev: &KeyEvent, flags: u8) -> Option<Vec<u8>> {
    let all = flags & ALL_AS_ESCAPES != 0;
    let events = flags & EVENT_TYPES != 0;
    let (form, m) = if ev.altgr {
        if !all {
            return altgr_text(ev);
        }
        let c = text_char(ev)?;
        (Form::U(c as u32), 0)
    } else {
        (kitty_form(ev)?, cook(ev.mods, ev.sides, ev.bucky))
    };
    let (shift, others) = (m & 1 != 0, m & !1 != 0);
    let text_key = matches!(ev.key, Key::Text(_));
    let legacy_key = text_key || matches!(ev.key, Key::Enter | Key::Tab | Key::Backspace);
    // Keys that go out as plain text (or legacy Enter, Tab, Backspace) report no release.
    let as_text = !all && legacy_key && !others && (!shift || text_key);
    if ev.kind == Kind::Release && (!events || as_text) {
        return Some(Vec::new());
    }
    if as_text {
        return None;
    }
    let event = match ev.kind {
        Kind::Press => 1,
        Kind::Repeat if events => 2,
        Kind::Repeat => 1,
        Kind::Release => 3,
    };
    let mut mods = String::new();
    if m != 0 || event != 1 {
        mods = (m + 1).to_string();
        if event != 1 {
            mods += &format!(":{event}");
        }
    }
    let mut out = String::from("\x1b[");
    match form {
        Form::U(code) => {
            out += &code.to_string();
            if flags & ALTERNATE_KEYS != 0 && shift && !ev.altgr {
                if let (Some(t), Some(b)) = (text_char(ev), base_char(ev)) {
                    if t != b {
                        out += &format!(":{}", t as u32);
                    }
                }
            }
            let text: Vec<String> = match ev.text {
                Some(t) if flags & ASSOCIATED_TEXT != 0 && all && ev.kind != Kind::Release && !others => {
                    t.chars().filter(|c| !c.is_control()).map(|c| (c as u32).to_string()).collect()
                }
                _ => Vec::new(),
            };
            if !mods.is_empty() || !text.is_empty() {
                out += &format!(";{mods}");
            }
            if !text.is_empty() {
                out += &format!(";{}", text.join(":"));
            }
            out.push('u');
        }
        Form::Letter(c) => {
            if !mods.is_empty() {
                out += &format!("1;{mods}");
            }
            out.push(c);
        }
        Form::Tilde(n) => {
            out += &n.to_string();
            if !mods.is_empty() {
                out += &format!(";{mods}");
            }
            out.push('~');
        }
    }
    Some(out.into_bytes())
}

fn raw_flags(m: Mods) -> u32 {
    1 + m.shift as u32 + 4 * m.alt as u32 + 16 * m.ctrl as u32
}

fn hex(s: &str) -> String {
    s.bytes().map(|b| format!("{b:02x}")).collect()
}

/// A modifier key, as kitty numbers it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Modifier {
    LeftShift,
    LeftCtrl,
    LeftAlt,
    LeftSuper,
    RightShift,
    RightCtrl,
    RightAlt,
    RightSuper,
}

/// A modifier key's own press or release under kitty flag 8. `mods` is the state before
/// the event; the report carries the state after it. None: nothing to send.
pub fn kitty_modifier(key: Modifier, mods: Mods, sides: Sides, bucky: Bucky, kind: Kind, flags: u8) -> Option<Vec<u8>> {
    use Modifier::*;
    if flags & ALL_AS_ESCAPES == 0 || (kind == Kind::Release && flags & EVENT_TYPES == 0) {
        return None;
    }
    let code = match key {
        LeftShift => 57441,
        LeftCtrl => 57442,
        LeftAlt => 57443,
        LeftSuper => 57444,
        RightShift => 57447,
        RightCtrl => 57448,
        RightAlt => 57449,
        RightSuper => 57450,
    };
    // The state after the event: the key's own side, then the modifier it stands for.
    let down = kind != Kind::Release;
    let (mut m, mut sd) = (mods, sides);
    match key {
        LeftShift | RightShift => m.shift = down,
        LeftAlt | RightAlt => m.alt = down,
        LeftCtrl => sd.lctrl = down,
        RightCtrl => sd.rctrl = down,
        LeftSuper => sd.lwin = down,
        RightSuper => sd.rwin = down,
    }
    if matches!(key, LeftCtrl | RightCtrl) {
        m.ctrl = sd.lctrl || sd.rctrl;
    }
    if matches!(key, LeftSuper | RightSuper) {
        m.cmd = sd.lwin || sd.rwin;
    }
    let after = cook(m, sd, bucky);
    let event = match kind {
        Kind::Press => 1,
        Kind::Repeat if flags & EVENT_TYPES != 0 => 2,
        Kind::Repeat => 1,
        Kind::Release => 3,
    };
    let text = if event == 1 && after == 0 {
        format!("\x1b[{code}u")
    } else if event == 1 {
        format!("\x1b[{code};{}u", after + 1)
    } else {
        format!("\x1b[{code};{}:{event}u", after + 1)
    };
    Some(text.into_bytes())
}

/// iTerm2's raw key reporting: `OSC 1337 ; d|u ; flags ; hex(text) ; key code ;
/// hex(base) BEL`, with Windows virtual key codes. Tandem does not tell the left and
/// right modifiers apart, so both count as the left one.
pub fn raw(ev: &KeyEvent) -> Vec<u8> {
    let dir = if ev.kind == Kind::Release { 'u' } else { 'd' };
    let mut flags = raw_flags(ev.mods);
    if ev.kind == Kind::Repeat {
        flags += 256;
    }
    let text = ev.text.unwrap_or("");
    let base = base_char(ev).map(|c| c.to_string()).unwrap_or_default();
    format!("\x1b]1337;{dir};{flags};{};{};{}\x07", hex(text), ev.vk, hex(&base)).into_bytes()
}

/// Raw key reporting's modifier change: `OSC 1337 ; f ; flags BEL`.
pub fn raw_modifiers(mods: Mods) -> Vec<u8> {
    format!("\x1b]1337;f;{}\x07", raw_flags(mods)).into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONE: Mods = Mods { shift: false, alt: false, ctrl: false, cmd: false };

    fn m(shift: bool, alt: bool, ctrl: bool) -> Mods {
        Mods { shift, alt, ctrl, cmd: false }
    }

    fn ev<'a>(key: &'a Key, base: Option<char>, text: Option<&'a str>, mods: Mods) -> KeyEvent<'a> {
        KeyEvent { key, base, text, mods, kind: Kind::Press, altgr: false, vk: 0, sides: Sides::default(), bucky: Bucky::default() }
    }

    fn s(v: Option<Vec<u8>>) -> Option<String> {
        v.map(|b| String::from_utf8(b).unwrap())
    }

    #[test]
    fn mok2_encodes_modified_keys() {
        let a = Key::Text("a".into());
        let up = Key::Up;
        assert_eq!(s(modify_other_keys(&ev(&a, Some('a'), Some("a"), m(false, false, true)), 2)).as_deref(), Some("\x1b[27;5;97~"));
        assert_eq!(s(modify_other_keys(&ev(&a, Some('a'), Some("a"), m(false, true, false)), 2)).as_deref(), Some("\x1b[27;3;97~"));
        assert_eq!(s(modify_other_keys(&ev(&Key::Text("A".into()), Some('a'), Some("A"), m(true, false, true)), 2)).as_deref(), Some("\x1b[27;6;65~"));
        assert_eq!(s(modify_other_keys(&ev(&Key::Enter, None, None, m(false, false, true)), 2)).as_deref(), Some("\x1b[27;5;13~"));
        assert_eq!(s(modify_other_keys(&ev(&Key::Text(" ".into()), Some(' '), Some(" "), m(true, false, false)), 2)).as_deref(), Some("\x1b[27;2;32~"));
        assert_eq!(modify_other_keys(&ev(&Key::Tab, None, None, m(true, false, false)), 2), None, "Shift+Tab stays CSI Z");
        assert_eq!(modify_other_keys(&ev(&a, Some('a'), Some("a"), NONE), 2), None);
        assert_eq!(modify_other_keys(&ev(&up, None, None, m(false, false, true)), 2), None);
    }

    #[test]
    fn mok1_leaves_plain_keys() {
        let a = Key::Text("a".into());
        assert_eq!(modify_other_keys(&ev(&a, Some('a'), Some("a"), m(false, false, true)), 1), None, "Ctrl+A has its control code");
        assert_eq!(modify_other_keys(&ev(&a, Some('a'), Some("a"), m(false, true, false)), 1), None, "Alt+A is Esc a");
        let one = Key::Text("1".into());
        assert_eq!(s(modify_other_keys(&ev(&one, Some('1'), Some("1"), m(false, false, true)), 1)).as_deref(), Some("\x1b[27;5;49~"));
        assert_eq!(s(modify_other_keys(&ev(&Key::Enter, None, None, m(true, false, false)), 1)).as_deref(), Some("\x1b[27;2;13~"));
    }

    #[test]
    fn csi_u_follows_fixterms() {
        let a = Key::Text("a".into());
        let c = |k: &Key, b: Option<char>, t: Option<&str>, mods: Mods| s(csi_u(&ev(k, b, t, mods)));
        assert_eq!(c(&a, Some('a'), Some("a"), NONE), None);
        assert_eq!(c(&Key::Text("A".into()), Some('a'), Some("A"), m(true, false, false)), None, "Shift+letter types");
        assert_eq!(c(&a, Some('a'), Some("a"), m(false, false, true)).as_deref(), Some("\x01"));
        assert_eq!(c(&Key::Text("i".into()), Some('i'), Some("i"), m(false, false, true)).as_deref(), Some("\x1b[105;5u"));
        assert_eq!(c(&a, Some('a'), Some("a"), m(false, true, false)).as_deref(), Some("\x1b[97;3u"));
        assert_eq!(c(&a, Some('a'), Some("a"), m(true, false, true)).as_deref(), Some("\x1b[97;6u"));
        assert_eq!(c(&Key::Enter, None, None, m(true, false, false)).as_deref(), Some("\x1b[13;2u"));
        assert_eq!(c(&Key::Escape, None, None, m(false, false, true)).as_deref(), Some("\x1b[27;5u"));
        assert_eq!(c(&Key::Tab, None, None, m(true, false, false)).as_deref(), Some("\x1b[Z"));
        assert_eq!(c(&Key::Tab, None, None, m(false, false, true)).as_deref(), Some("\x1b[1;5Z"));
        assert_eq!(c(&Key::Text(" ".into()), Some(' '), Some(" "), m(false, false, true)).as_deref(), Some("\x00"));
        assert_eq!(c(&Key::Up, None, None, m(false, false, true)), None);
    }

    fn k(key: &Key, base: Option<char>, text: Option<&str>, mods: Mods, kind: Kind, flags: u8) -> Option<String> {
        s(kitty(&KeyEvent { key, base, text, mods, kind, altgr: false, vk: 0, sides: Sides::default(), bucky: Bucky::default() }, flags))
    }

    #[test]
    fn kitty_disambiguates_escape() {
        let a = Key::Text("a".into());
        assert_eq!(k(&Key::Escape, None, None, NONE, Kind::Press, 1).as_deref(), Some("\x1b[27u"));
        assert_eq!(k(&a, Some('a'), Some("a"), NONE, Kind::Press, 1), None, "plain text stays text");
        assert_eq!(k(&a, Some('a'), Some("a"), m(false, false, true), Kind::Press, 1).as_deref(), Some("\x1b[97;5u"));
        assert_eq!(k(&a, Some('a'), Some("a"), m(false, true, false), Kind::Press, 1).as_deref(), Some("\x1b[97;3u"));
        assert_eq!(k(&Key::Enter, None, None, NONE, Kind::Press, 1), None);
        assert_eq!(k(&Key::Enter, None, None, m(false, false, true), Kind::Press, 1).as_deref(), Some("\x1b[13;5u"));
        assert_eq!(k(&Key::Up, None, None, NONE, Kind::Press, 1).as_deref(), Some("\x1b[A"));
        assert_eq!(k(&Key::Up, None, None, m(true, false, false), Kind::Press, 1).as_deref(), Some("\x1b[1;2A"));
        assert_eq!(k(&Key::F(1), None, None, NONE, Kind::Press, 1).as_deref(), Some("\x1b[11~"), "iTerm2's F1");
        assert_eq!(k(&Key::F(5), None, None, m(false, false, true), Kind::Press, 1).as_deref(), Some("\x1b[15;5~"));
        assert_eq!(k(&Key::Delete, None, None, NONE, Kind::Press, 1).as_deref(), Some("\x1b[3~"));
    }

    #[test]
    fn kitty_reports_press_repeat_release() {
        let a = Key::Text("a".into());
        let f = EVENT_TYPES | DISAMBIGUATE;
        assert_eq!(k(&a, Some('a'), Some("a"), m(false, false, true), Kind::Press, f).as_deref(), Some("\x1b[97;5u"));
        assert_eq!(k(&a, Some('a'), Some("a"), m(false, false, true), Kind::Repeat, f).as_deref(), Some("\x1b[97;5:2u"));
        assert_eq!(k(&a, Some('a'), Some("a"), m(false, false, true), Kind::Release, f).as_deref(), Some("\x1b[97;5:3u"));
        assert_eq!(k(&Key::Escape, None, None, NONE, Kind::Release, f).as_deref(), Some("\x1b[27;1:3u"));
        assert_eq!(k(&Key::Up, None, None, NONE, Kind::Release, f).as_deref(), Some("\x1b[1;1:3A"));
        assert_eq!(k(&a, Some('a'), Some("a"), NONE, Kind::Release, f).as_deref(), Some(""), "text keys send no release");
        assert_eq!(k(&Key::Escape, None, None, NONE, Kind::Release, DISAMBIGUATE).as_deref(), Some(""), "no releases without flag 2");
    }

    #[test]
    fn kitty_alternate_keys_and_text() {
        let a = Key::Text("A".into());
        let f = ALL_AS_ESCAPES | ALTERNATE_KEYS | ASSOCIATED_TEXT;
        assert_eq!(k(&a, Some('a'), Some("A"), m(true, false, false), Kind::Press, f).as_deref(), Some("\x1b[97:65;2;65u"));
        assert_eq!(k(&Key::Text("a".into()), Some('a'), Some("a"), m(false, false, true), Kind::Press, f).as_deref(), Some("\x1b[97;5u"), "no text with Ctrl");
    }

    #[test]
    fn kitty_all_keys_as_escapes() {
        let a = Key::Text("a".into());
        assert_eq!(k(&a, Some('a'), Some("a"), NONE, Kind::Press, ALL_AS_ESCAPES).as_deref(), Some("\x1b[97u"));
        assert_eq!(k(&Key::Enter, None, None, NONE, Kind::Press, ALL_AS_ESCAPES).as_deref(), Some("\x1b[13u"));
        assert_eq!(k(&a, Some('a'), Some("a"), NONE, Kind::Release, ALL_AS_ESCAPES | EVENT_TYPES).as_deref(), Some("\x1b[97;1:3u"));
    }

    #[test]
    fn altgr_types_its_character() {
        let at = Key::Text("@".into());
        let e = KeyEvent { key: &at, base: Some('q'), text: Some("@"), mods: m(false, true, true), kind: Kind::Press, altgr: true, vk: 0x51, sides: Sides::default(), bucky: Bucky::default() };
        assert_eq!(s(modify_other_keys(&e, 2)).as_deref(), Some("@"));
        assert_eq!(s(csi_u(&e)).as_deref(), Some("@"));
        assert_eq!(s(kitty(&e, DISAMBIGUATE)).as_deref(), Some("@"));
        assert_eq!(s(kitty(&e, ALL_AS_ESCAPES)).as_deref(), Some("\x1b[64u"));
    }

    #[test]
    fn raw_reports_key_down_and_up() {
        let a = Key::Text("a".into());
        let mut e = KeyEvent { key: &a, base: Some('a'), text: Some("a"), mods: m(true, false, false), kind: Kind::Press, altgr: false, vk: 0x41, sides: Sides::default(), bucky: Bucky::default() };
        assert_eq!(String::from_utf8(raw(&e)).unwrap(), "\x1b]1337;d;2;61;65;61\x07");
        e.kind = Kind::Release;
        assert!(String::from_utf8(raw(&e)).unwrap().starts_with("\x1b]1337;u;2;"));
        e.kind = Kind::Repeat;
        assert!(String::from_utf8(raw(&e)).unwrap().starts_with("\x1b]1337;d;258;"));
        assert_eq!(String::from_utf8(raw_modifiers(m(false, false, true))).unwrap(), "\x1b]1337;f;17\x07");
    }
    #[test]
    fn encode_picks_the_protocol() {
        let a = Key::Text("a".into());
        let ctrl_a = ev(&a, Some('a'), Some("a"), m(false, false, true));
        let kb = |mode, kitty| Keyboard { mode, kitty };
        assert_eq!(encode(kb(KeyMode::Standard, 0), &ctrl_a), None);
        assert_eq!(s(encode(kb(KeyMode::Mok2, 0), &ctrl_a)).as_deref(), Some("\x1b[27;5;97~"));
        assert_eq!(s(encode(kb(KeyMode::CsiU, 0), &ctrl_a)).as_deref(), Some("\x01"));
        assert_eq!(s(encode(kb(KeyMode::Mok2, DISAMBIGUATE), &ctrl_a)).as_deref(), Some("\x1b[97;5u"), "kitty wins");
        assert!(s(encode(kb(KeyMode::Raw, 0), &ctrl_a)).unwrap().starts_with("\x1b]1337;d;"));
        let mut up = ctrl_a;
        up.kind = Kind::Release;
        assert_eq!(s(encode(kb(KeyMode::Standard, 0), &up)).as_deref(), Some(""));
        assert!(!kb(KeyMode::Mok2, DISAMBIGUATE).wants_releases());
        assert!(kb(KeyMode::Standard, EVENT_TYPES).wants_releases());
        assert!(kb(KeyMode::Raw, 0).wants_releases());
    }

    #[test]
    fn kitty_reports_modifier_keys() {
        assert_eq!(s(kitty_modifier(Modifier::LeftShift, NONE, Sides::default(), Bucky::default(), Kind::Press, ALL_AS_ESCAPES)).as_deref(), Some("\x1b[57441;2u"));
        assert_eq!(s(kitty_modifier(Modifier::RightCtrl, m(false, false, true), Sides { rctrl: true, ..Sides::default() }, Bucky::default(), Kind::Release, ALL_AS_ESCAPES | EVENT_TYPES)).as_deref(), Some("\x1b[57448;1:3u"));
        assert_eq!(kitty_modifier(Modifier::LeftAlt, NONE, Sides::default(), Bucky::default(), Kind::Press, DISAMBIGUATE), None, "only with flag 8");
    }
    /// 13u Task 3 (Review Focus 3): `iTermModernKeyMapper.swift:1717-1860`.
    #[test]
    fn bucky_bits_follow_iterm2() {
        let a = Key::Text("a".into());
        let ctrl = m(false, false, true);
        let win = Mods { cmd: true, ..NONE };
        let left = Sides { lctrl: true, ..Sides::default() };
        let right = Sides { rctrl: true, ..Sides::default() };
        let lwin = Sides { lwin: true, ..Sides::default() };
        let run = |mods: Mods, sides: Sides, b: Bucky, flags: u8| {
            let mut e = ev(&a, Some('a'), Some("a"), mods);
            e.sides = sides;
            e.bucky = b;
            s(kitty(&e, flags))
        };
        let plain = Bucky::default();
        let hyper = Bucky { left_ctrl: 1, ..plain };
        assert_eq!(run(ctrl, left, plain, DISAMBIGUATE).as_deref(), Some("\x1b[97;5u"));
        assert_eq!(run(ctrl, left, hyper, DISAMBIGUATE).as_deref(), Some("\x1b[97;17u"), "left Control as Hyper");
        assert_eq!(run(ctrl, right, hyper, DISAMBIGUATE).as_deref(), Some("\x1b[97;5u"), "per side: right stays Control");
        let both = Sides { lctrl: true, rctrl: true, ..Sides::default() };
        assert_eq!(run(ctrl, both, hyper, DISAMBIGUATE).as_deref(), Some("\x1b[97;21u"), "both down: Hyper and Control");
        let sup = Bucky { left_win: 3, ..plain };
        assert_eq!(run(win, lwin, sup, DISAMBIGUATE).as_deref(), Some("\x1b[97;9u"), "Win as Super");
        assert_eq!(run(win, lwin, plain, DISAMBIGUATE), None, "Win alone is never reported");
        let meta = Bucky { left_win: 2, ..plain };
        assert_eq!(run(Mods { shift: true, ..win }, lwin, meta, DISAMBIGUATE).as_deref(), Some("\x1b[97;34u"), "Meta 32 + Shift 1, plus one");
        assert_eq!(run(win, lwin, Bucky { left_win: 2, ..plain }, DISAMBIGUATE).as_deref(), Some("\x1b[97;33u"), "Meta");
        // The modifier key itself reports its own bucky bit.
        let report = kitty_modifier(Modifier::LeftCtrl, NONE, Sides::default(), hyper, Kind::Press, ALL_AS_ESCAPES);
        assert_eq!(s(report).as_deref(), Some("\x1b[57442;17u"));
        let win_key = kitty_modifier(Modifier::LeftSuper, NONE, Sides::default(), sup, Kind::Press, ALL_AS_ESCAPES);
        assert_eq!(s(win_key).as_deref(), Some("\x1b[57444;9u"));
        let release = kitty_modifier(Modifier::LeftCtrl, ctrl, left, hyper, Kind::Release, ALL_AS_ESCAPES | EVENT_TYPES);
        assert_eq!(s(release).as_deref(), Some("\x1b[57442;1:3u"));
        // Legacy modes never see the mapping.
        let mut e = ev(&a, Some('a'), Some("a"), ctrl);
        e.sides = left;
        e.bucky = hyper;
        assert_eq!(encode(Keyboard { mode: KeyMode::Standard, kitty: 0 }, &e), None, "standard bytes stay \x01");
        assert_eq!(s(encode(Keyboard { mode: KeyMode::CsiU, kitty: 0 }, &e)).as_deref(), Some("\x01"));
        assert_eq!(s(encode(Keyboard { mode: KeyMode::Mok2, kitty: 0 }, &e)).as_deref(), Some("\x1b[27;5;97~"));
    }

    #[test]
    fn ctrl_alt_letter_is_not_altgr() {
        let both = m(false, true, true);
        assert!(!is_altgr(both, Some("a"), Some('a')), "Ctrl+Alt+A on a US layout");
        assert!(!is_altgr(both, Some("A"), Some('a')), "with Shift too");
        assert!(is_altgr(both, Some("@"), Some('q')), "AltGr+Q on a German layout");
        assert!(!is_altgr(m(false, true, false), Some("@"), Some('q')), "Alt alone");
        assert!(!is_altgr(both, None, Some('a')));
    }
}
