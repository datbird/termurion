//! The shortcut key: which Windows modifiers stand for iTerm2's Cmd (TWEAKS 9, FIXES 12 and 13).
//!
//! Every Termurion shortcut, key mapping and menu item keeps iTerm2's Cmd notation (`Chord::cmd`,
//! the COMMAND flag in a key binding). That notation is the stored identity. This is the one
//! place that says what is pressed for it, and no shortcut is pressed with the Win key: Windows
//! keeps that key for itself.
//!
//! Cmd is Ctrl+Shift (the default) or Ctrl+Alt. Cmd+Shift+X cannot be Ctrl+Shift+Shift+X, so the
//! three modifiers Ctrl, Shift and Alt are shared out as pairs: Cmd is the chosen pair, Cmd+Shift
//! is all three, Cmd+Option is the second pair, and Cmd+Ctrl is the third (the one pair left).
//! Cmd+Option+Shift, Cmd+Ctrl+Shift and the four-modifier chords have no pair left. Those chords
//! are moved to a key of their own by an explicit table (`EXTRAS`): the same keys in both modes,
//! chosen so no other default uses them. Windows Terminal's Alt+Shift+- and Alt+Shift+= split a
//! pane, so Split Horizontally and Vertically take them; the rest are Shift+Alt, Ctrl+Shift or
//! Ctrl+Alt with a function key or a page key, and Ctrl+Shift+Alt+N. A letter or digit is never
//! used with Ctrl+Alt or Shift+Alt, which carry a profile's own shortcut (Cmd+Ctrl+key).
//!
//! A profile's new-window shortcut (Cmd+Ctrl+Option+key) is Ctrl+Shift+Alt+key. That press is
//! also Cmd+Shift+key, so a key that Cmd+Shift already runs something for keeps that command, and
//! the profile opens in a tab with its Cmd+Ctrl shortcut.

use std::sync::atomic::{AtomicU8, Ordering};

use crate::menus::{key_label, Chord, ChordKey};
use crate::mouse::Mods;

/// The warning the settings page shows beside a choice, if it needs one.
pub fn warning(m: ShortcutKey) -> Option<&'static str> {
    (m == ShortcutKey::CtrlAlt).then_some("Ctrl+Alt is AltGr on many European keyboard layouts. Termurion leaves a key that types a character alone, but a shortcut on a key AltGr also uses can be hard to press. Ctrl+Shift avoids this.")
}

/// The Keys setting: 0 Ctrl+Shift, 2 Ctrl+Alt. (1 was Win; a saved 1 reads as Ctrl+Shift.)
pub const KEY: &str = "ShortcutKey";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShortcutKey {
    CtrlShift,
    CtrlAlt,
}

/// The physical Ctrl, Shift and Alt held (the Win key is not part of it).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Held {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
}

/// What the key path and the menus read. Ctrl+Shift until the app sets it from the setting.
static MODE: AtomicU8 = AtomicU8::new(0);

pub fn mode() -> ShortcutKey {
    ShortcutKey::from_value(i64::from(MODE.load(Ordering::Relaxed)))
}

pub fn set_mode(m: ShortcutKey) {
    MODE.store(m.value() as u8, Ordering::Relaxed);
}

const CS: Held = Held { ctrl: true, shift: true, alt: false };
const CA: Held = Held { ctrl: true, shift: false, alt: true };
const SA: Held = Held { ctrl: false, shift: true, alt: true };
const CSA: Held = Held { ctrl: true, shift: true, alt: true };

/// The Cmd chords no pair of Ctrl, Shift and Alt covers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Rest {
    /// Cmd+Option+Shift.
    OptionShift,
    /// Cmd+Ctrl+Shift.
    CtrlShift,
    /// Cmd+Ctrl+Option+Shift.
    All,
}

impl Rest {
    fn of(c: &Chord) -> Option<Rest> {
        match (c.shift, c.alt, c.ctrl) {
            (true, true, false) => Some(Rest::OptionShift),
            (true, false, true) => Some(Rest::CtrlShift),
            (true, true, true) => Some(Rest::All),
            _ => None,
        }
    }
}

/// One chord moved off the Win key: iTerm2's chord (`rest` plus `key`) is pressed as `held` plus `press`.
struct Extra {
    rest: Rest,
    key: ChordKey,
    held: Held,
    press: ChordKey,
}

const fn ex(rest: Rest, key: ChordKey, held: Held, press: ChordKey) -> Extra {
    Extra { rest, key, held, press }
}

use ChordKey::{Char, Enter, PageDown, PageUp, Up, Down, F};
use Rest::{All, CtrlShift as CtrlShiftRest, OptionShift};

/// The explicit remap of every chord with no free pair. The test `every_default_has_a_chord_without_win`
/// checks that each shipped command is here or has a pair, and that no two share a press.
const EXTRAS: &[Extra] = &[
    // Shell and splits.
    ex(OptionShift, Char('n'), CSA, Char('n')),
    ex(OptionShift, Char('t'), CS, F(11)),
    ex(OptionShift, Char('h'), SA, Char('-')),
    ex(OptionShift, Char('v'), SA, Char('=')),
    ex(OptionShift, Char('s'), SA, F(9)),
    // Tabs.
    ex(OptionShift, Char('['), SA, PageUp),
    ex(OptionShift, Char(']'), SA, PageDown),
    ex(CtrlShiftRest, Char('['), CSA, PageUp),
    ex(CtrlShiftRest, Char(']'), CSA, PageDown),
    // Find, selection, marks and annotations: Ctrl+Shift+F1 to F10, and New Tab with Current Profile on F11.
    ex(OptionShift, Char('f'), CS, F(1)),
    ex(OptionShift, Char('e'), CS, F(2)),
    ex(OptionShift, Char('u'), CS, F(3)),
    ex(OptionShift, Char('o'), CS, F(4)),
    ex(OptionShift, Char('a'), CS, F(5)),
    ex(OptionShift, Char('m'), CS, F(6)),
    ex(OptionShift, Down, CS, F(7)),
    ex(OptionShift, Up, CS, F(8)),
    ex(OptionShift, Char('l'), CS, F(9)),
    ex(OptionShift, Char('p'), CS, F(10)),
    // Session, broadcast, AI and windows: Shift+Alt+F1 to F9.
    ex(OptionShift, Char('i'), SA, F(1)),
    ex(All, Char('i'), SA, F(2)),
    ex(OptionShift, Char('y'), SA, F(3)),
    ex(OptionShift, Char('.'), SA, F(4)),
    ex(OptionShift, Char('r'), SA, F(5)),
    ex(OptionShift, Char('b'), SA, F(6)),
    ex(CtrlShiftRest, Char('y'), SA, F(7)),
    ex(OptionShift, Char('c'), SA, F(8)),
    // tmux: Ctrl+Alt+F1 to F5.
    ex(CtrlShiftRest, Char('d'), CA, F(1)),
    ex(CtrlShiftRest, Char('n'), CA, F(2)),
    ex(CtrlShiftRest, Char('t'), CA, F(3)),
    ex(CtrlShiftRest, Char('p'), CA, F(4)),
    ex(CtrlShiftRest, Enter, CA, F(5)),
    // Make default, and the debug build's Capture GPU Frame: Ctrl+Shift+Alt+F10 to F12.
    ex(CtrlShiftRest, Char('\\'), CSA, F(10)),
    ex(All, Char('\\'), CSA, F(11)),
    ex(All, Char('g'), CSA, F(12)),
    // Workgroup peers 1 to 9: Ctrl+Shift+Alt+F1 to F9.
    ex(OptionShift, Char('1'), CSA, F(1)),
    ex(OptionShift, Char('2'), CSA, F(2)),
    ex(OptionShift, Char('3'), CSA, F(3)),
    ex(OptionShift, Char('4'), CSA, F(4)),
    ex(OptionShift, Char('5'), CSA, F(5)),
    ex(OptionShift, Char('6'), CSA, F(6)),
    ex(OptionShift, Char('7'), CSA, F(7)),
    ex(OptionShift, Char('8'), CSA, F(8)),
    ex(OptionShift, Char('9'), CSA, F(9)),
];

/// The extra entry for a Cmd chord.
fn extra_of(c: &Chord) -> Option<&'static Extra> {
    let rest = Rest::of(c)?;
    c.cmd.then_some(())?;
    EXTRAS.iter().find(|e| e.rest == rest && e.key == c.key)
}

/// A profile's new-window shortcut (Cmd+Ctrl+Option+key, A-Z or 0-9) is pressed as Ctrl+Shift+Alt+key.
fn profile_window_key(c: &Chord) -> Option<char> {
    match c.key {
        ChordKey::Char(k) if c.cmd && c.ctrl && c.alt && !c.shift && k.is_ascii_alphanumeric() => Some(k),
        _ => None,
    }
}

impl ShortcutKey {
    /// An unknown or missing value is the default, Ctrl+Shift. A saved 1 (the retired Win key)
    /// is Ctrl+Shift too.
    pub fn from_value(v: i64) -> ShortcutKey {
        match v {
            2 => ShortcutKey::CtrlAlt,
            _ => ShortcutKey::CtrlShift,
        }
    }

    pub fn value(self) -> i64 {
        match self {
            ShortcutKey::CtrlShift => 0,
            ShortcutKey::CtrlAlt => 2,
        }
    }

    /// The setting as saved; unset is the default (an existing user who never chose gets it).
    pub fn from_globals(g: &crate::globals::Globals) -> ShortcutKey {
        ShortcutKey::from_value(g.get(KEY).and_then(|v| v.as_integer()).unwrap_or(0))
    }

    /// The pairs for Cmd, Cmd+Shift, Cmd+Option and Cmd+Ctrl. The last is the one pair of Ctrl,
    /// Shift and Alt the other three leave free.
    fn pairs(self) -> [Held; 4] {
        match self {
            ShortcutKey::CtrlShift => [CS, CSA, SA, CA],
            ShortcutKey::CtrlAlt => [CA, CSA, CS, SA],
        }
    }

    /// The physical modifiers a Cmd chord with this Shift, Option and Control needs; None when
    /// the chord has no pair (the table in `EXTRAS` moves it to another key).
    pub fn held_for(self, shift: bool, alt: bool, ctrl: bool) -> Option<Held> {
        let p = self.pairs();
        match (ctrl, shift, alt) {
            (false, false, false) => Some(p[0]),
            (false, true, false) => Some(p[1]),
            (false, false, true) => Some(p[2]),
            (true, false, false) => Some(p[3]),
            _ => None,
        }
    }

    /// The physical press of a Cmd chord: its modifiers and its key. A chord with a pair keeps
    /// its key; one without moves to its entry in `EXTRAS`; a profile's new-window shortcut is
    /// Ctrl+Shift+Alt+key. None for a chord without Cmd (it is pressed as it reads) and for a
    /// Cmd chord nothing moves (an imported binding no pair reaches).
    pub fn form(self, c: &Chord) -> Option<(Held, ChordKey)> {
        if !c.cmd {
            return None;
        }
        if let Some(e) = extra_of(c) {
            return Some((e.held, e.press.clone()));
        }
        if profile_window_key(c).is_some() {
            return Some((CSA, c.key.clone()));
        }
        self.held_for(c.shift, c.alt, c.ctrl).map(|h| (h, c.key.clone()))
    }

    /// The Cmd, Shift and Option a physical key press stands for (cmd false when it is none of
    /// the pairs, or the Win key is down: that is for Windows and for a click).
    pub fn logical(self, phys: Mods) -> Mods {
        if phys.cmd {
            return phys;
        }
        let now = Held { ctrl: phys.ctrl, shift: phys.shift, alt: phys.alt };
        match self.pairs().iter().position(|h| *h == now) {
            Some(i) => Mods { cmd: true, shift: i == 1, alt: i == 2, ctrl: i == 3 },
            None => phys,
        }
    }

    /// The Cmd (with Shift, Option and Control) a key press makes with this mode's own modifiers, or None
    /// when it is not one: the Win key is down (Windows' own), the modifiers are no pair, or
    /// the press types a character with Ctrl+Alt held, which is AltGr on a layout that has one.
    pub fn press_as_cmd(self, phys: Mods, types_a_character: bool) -> Option<Mods> {
        if phys.cmd || (phys.ctrl && phys.alt && types_a_character) {
            return None;
        }
        Some(self.logical(phys)).filter(|l| l.cmd)
    }

    /// The modifiers the settings key recorder stores for a press: the Cmd (with Shift and Option)
    /// its shortcut-key modifiers stand for, else the keys as pressed.
    pub fn recorded_mods(self, phys: Mods, types_a_character: bool) -> Mods {
        self.press_as_cmd(phys, types_a_character).unwrap_or(phys)
    }

    /// Every Cmd chord a physical press can be, the explicit table first, then the pair's, then a
    /// profile's new-window shortcut. Empty for the Win key, for modifiers that are none of these
    /// and for AltGr typing. The caller takes the first one something is bound to.
    pub fn press_chords(self, phys: Mods, types_a_character: bool, key: &ChordKey) -> Vec<Chord> {
        if phys.cmd || (phys.ctrl && phys.alt && types_a_character) {
            return Vec::new();
        }
        let held = Held { ctrl: phys.ctrl, shift: phys.shift, alt: phys.alt };
        let mut out = Vec::new();
        for e in EXTRAS.iter().filter(|e| e.held == held && e.press == *key) {
            let (shift, alt, ctrl) = match e.rest {
                Rest::OptionShift => (true, true, false),
                Rest::CtrlShift => (true, false, true),
                Rest::All => (true, true, true),
            };
            out.push(Chord { key: e.key.clone(), cmd: true, alt, shift, ctrl });
        }
        if let Some(i) = self.pairs().iter().position(|h| *h == held) {
            out.push(Chord { key: key.clone(), cmd: true, shift: i == 1, alt: i == 2, ctrl: i == 3 });
        }
        if let (true, ChordKey::Char(k)) = (held == CSA, key)
            && k.is_ascii_alphanumeric() {
                out.push(Chord { key: key.clone(), cmd: true, alt: true, shift: false, ctrl: true });
            }
        out
    }

    /// The chord the explicit table makes of a press, for the key recorder: iTerm2's chord is
    /// what is stored, as it is for a pair.
    pub fn table_chord(self, phys: Mods, types_a_character: bool, key: &ChordKey) -> Option<Chord> {
        self.press_chords(phys, types_a_character, key).into_iter().find(|c| extra_of(c).is_some())
    }

    /// The Cmd chord a key press makes in a Termurion window that has no menu of its own (the
    /// password manager, AI Chat, the Cockpit): the first chord of the press, without Ctrl (those
    /// windows have no Cmd+Ctrl command). None when the press is none, or AltGr typing.
    pub fn window_chord(self, phys: Mods, types_a_character: bool, key: ChordKey) -> Option<Chord> {
        self.press_chords(phys, types_a_character, &key).into_iter().find(|c| !c.ctrl)
    }

    /// Whether a Cmd chord has a press without the Win key in this mode.
    pub fn reachable(self, c: &Chord) -> bool {
        !c.cmd || self.form(c).is_some()
    }

    /// Whether a menu shows the chord: every chord with a press does.
    pub fn reaches(self, c: &Chord) -> bool {
        self.reachable(c)
    }

    /// How a menu shows a chord: the real keys. A chord nothing presses reads with Cmd.
    pub fn label(self, c: &Chord) -> String {
        match self.form(c) {
            Some((h, k)) => held_label(h) + &key_label(&k),
            None => crate::menus::chord_label(c).replacen("Win", "Cmd", 1),
        }
    }

    /// A text with iTerm2's Cmd spelled as Win ("Win+Shift+B", "Alt+Win+", "Broadcast (Win+B)")
    /// shown with the real keys, word by word: a word that is not a chord is left alone.
    pub fn show(self, text: &str) -> String {
        if !text.contains("Win") {
            return text.to_string();
        }
        text.split(' ')
            .map(|word| {
                let core = word.trim_start_matches('(').trim_end_matches(')');
                let (open, close) = (&word[..word.len() - word.trim_start_matches('(').len()], &word[word.trim_end_matches(')').len()..]);
                if open.len() + core.len() + close.len() != word.len() {
                    return word.to_string();
                }
                format!("{open}{}{close}", self.show_chord(core))
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// `show` for one chord or prefix of one.
    fn show_chord(self, text: &str) -> String {
        if !text.split('+').any(|t| t == "Win") {
            return text.to_string();
        }
        let tokens: Vec<&str> = text.split('+').collect();
        let has = |name: &str| tokens.contains(&name);
        let (shift, alt, ctrl) = (has("Shift"), has("Alt"), has("Ctrl"));
        let rest: Vec<&str> = tokens.iter().copied().filter(|t| !matches!(*t, "Win" | "Ctrl" | "Alt" | "Shift")).collect();
        // A whole chord: its key moves with the table.
        if let Some(key) = (rest.len() == 1).then(|| parse_key_label(rest[0])).flatten() {
            let c = Chord { key, cmd: true, alt, shift, ctrl };
            if let Some((h, k)) = self.form(&c) {
                return held_label(h) + &key_label(&k);
            }
        }
        let Some(h) = self.held_for(shift, alt, ctrl) else { return text.to_string() };
        held_label(h) + &rest.join("+")
    }
}

/// A key as a menu or a stroke label spells it ("D", "PgUp", "F5", "Return", ";") as a chord key.
pub fn parse_key_label(s: &str) -> Option<ChordKey> {
    let mut chars = s.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        return Some(ChordKey::Char(c.to_ascii_lowercase()));
    }
    Some(match s {
        "Up" => ChordKey::Up,
        "Down" => ChordKey::Down,
        "Left" => ChordKey::Left,
        "Right" => ChordKey::Right,
        "Home" => ChordKey::Home,
        "End" => ChordKey::End,
        "PgUp" | "PageUp" => ChordKey::PageUp,
        "PgDn" | "PageDown" => ChordKey::PageDown,
        "Del" | "Delete" => ChordKey::Delete,
        "Enter" | "Return" => ChordKey::Enter,
        "Tab" => ChordKey::Tab,
        "Esc" => ChordKey::Escape,
        "Backspace" => ChordKey::Backspace,
        "Space" => ChordKey::Space,
        f if f.starts_with('F') => ChordKey::F(f[1..].parse().ok().filter(|n| (1..=12).contains(n))?),
        _ => return None,
    })
}

/// The key that toggles full screen in every mode, as Windows Terminal and browsers do. iTerm2's
/// own chord (Cmd+Ctrl+F) works beside it.
pub const FULL_SCREEN_KEY: &str = "F11";

/// The View > Toggle Full Screen command.
pub fn is_full_screen_command(selector: &str) -> bool {
    selector == "toggleFullScreenMode:"
}

/// Whether a bare F11 press toggles full screen: no modifier is held, and no key mapping (the
/// profile's, the global ones or the Leader) claims the key, which keeps it for a program that
/// wants F11 through such a mapping.
pub fn f11_toggles_full_screen(is_f11: bool, mods: Mods, claimed_by_mapping: bool) -> bool {
    is_f11 && !(mods.shift || mods.alt || mods.ctrl || mods.cmd) && !claimed_by_mapping
}

/// The key that pastes in every mode, as Windows Terminal does. iTerm2's own chord (Cmd+V) works
/// beside it.
pub const PASTE_KEY: &str = "Ctrl+V";

/// Edit > Paste (not Paste Slowly, which shares the selector).
pub fn is_paste_command(id: &str) -> bool {
    id == "Edit > Paste"
}

/// Whether a Ctrl+V press pastes: Ctrl alone is held, no key mapping claims the key, and the pane
/// is not in Copy Mode (where Ctrl+V starts a box selection). A mapping for Ctrl+V keeps the key
/// for a program that wants its ^V.
pub fn ctrl_v_pastes(is_v: bool, mods: Mods, claimed_by_mapping: bool, in_copy_mode: bool) -> bool {
    is_v && mods.ctrl && !(mods.shift || mods.alt || mods.cmd) && !claimed_by_mapping && !in_copy_mode
}

/// The modifiers a window without menus (password manager, AI Chat, Cockpit) has seen, and the
/// Cmd chord a key press makes with them.
#[derive(Default)]
pub struct WindowKeys {
    mods: Mods,
}

impl WindowKeys {
    /// Feed every window event; a key press that is a shortcut-key chord comes back.
    pub fn chord(&mut self, e: &winit::event::WindowEvent) -> Option<Chord> {
        use winit::event::{ElementState, WindowEvent};
        match e {
            WindowEvent::ModifiersChanged(m) => {
                let s = m.state();
                self.mods = Mods { shift: s.shift_key(), alt: s.alt_key(), ctrl: s.control_key(), cmd: s.super_key() };
                None
            }
            WindowEvent::Focused(false) => {
                self.mods = Mods::default();
                None
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed && !event.repeat => {
                let types = event.text.as_deref().is_some_and(|t| t.chars().any(|c| !c.is_control()));
                let key = crate::platform::key_record::chord_key(event.physical_key)?;
                mode().window_chord(self.mods, types, key)
            }
            _ => None,
        }
    }
}

/// The stroke the recorder stores, built with the physical Shift: a letter shows its capital, but
/// Cmd+D (Ctrl+Shift+D) is the key `d`, as `press_from_chord` makes it, so the letter is lowered
/// when the Shift was part of the shortcut key.
pub fn recorded_stroke(stroke: crate::keystroke::Keystroke, phys: Mods, recorded: Mods) -> crate::keystroke::Keystroke {
    let lower = |c: u32| char::from_u32(c).filter(char::is_ascii_uppercase).map_or(c, |c| c.to_ascii_lowercase() as u32);
    if phys.shift && !recorded.shift && recorded.cmd {
        return crate::keystroke::Keystroke::new(lower(stroke.ch), stroke.flags, stroke.keycode, lower(stroke.modified));
    }
    stroke
}

/// "Ctrl+Shift+Alt+" for the held modifiers: all three in the order the setting names them,
/// else Ctrl, Alt, Shift as the menus always order them.
fn held_label(h: Held) -> String {
    if h.ctrl && h.shift && h.alt {
        return "Ctrl+Shift+Alt+".into();
    }
    let mut out = String::new();
    for (on, name) in [(h.ctrl, "Ctrl+"), (h.alt, "Alt+"), (h.shift, "Shift+")] {
        if on {
            out.push_str(name);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::menus::ChordKey;

    fn chord(c: char, shift: bool, alt: bool, ctrl: bool) -> Chord {
        Chord { key: ChordKey::Char(c), cmd: true, alt, shift, ctrl }
    }
    fn held(ctrl: bool, shift: bool, alt: bool) -> Held {
        Held { ctrl, shift, alt }
    }
    fn phys(ctrl: bool, shift: bool, alt: bool, win: bool) -> Mods {
        Mods { ctrl, shift, alt, cmd: win }
    }
    const MODES: [ShortcutKey; 2] = [ShortcutKey::CtrlShift, ShortcutKey::CtrlAlt];

    #[test]
    fn the_setting_defaults_to_ctrl_shift_and_offers_no_win() {
        let s = crate::schema::find_global(KEY).expect("ShortcutKey is a Keys setting");
        assert_eq!((s.tab.as_str(), s.section.as_str()), ("Keys", "Shortcut Key"));
        assert_eq!(s.default.as_ref().and_then(|v| v.as_integer()), Some(0));
        let labels: Vec<&str> = s.choices.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(labels.len(), 2, "{labels:?}");
        assert!(labels.iter().all(|l| !l.contains("Win")), "{labels:?}");
        assert_eq!(ShortcutKey::from_value(0), ShortcutKey::CtrlShift);
        assert_eq!(ShortcutKey::from_value(2), ShortcutKey::CtrlAlt);
        assert_eq!(ShortcutKey::from_value(9), ShortcutKey::CtrlShift);
        assert_eq!(ShortcutKey::from_value(-1), ShortcutKey::CtrlShift);
        for m in MODES {
            assert_eq!(ShortcutKey::from_value(m.value()), m);
        }
    }

    /// A saved Win (the retired value 1) reads as Ctrl+Shift, and loading the settings drops it.
    #[test]
    fn a_saved_win_value_becomes_ctrl_shift() {
        assert_eq!(ShortcutKey::from_value(1), ShortcutKey::CtrlShift);
        let mut g = crate::globals::Globals::empty(std::path::Path::new("unused.toml"));
        g.values.insert(KEY.into(), toml::Value::Integer(1));
        assert_eq!(ShortcutKey::from_globals(&g), ShortcutKey::CtrlShift);
        g.bound_values();
        assert!(g.raw(KEY).is_none(), "the migrated value is the default again");
        g.values.insert(KEY.into(), toml::Value::Integer(2));
        g.bound_values();
        assert_eq!(ShortcutKey::from_globals(&g), ShortcutKey::CtrlAlt, "Ctrl+Alt stays");
    }

    #[test]
    fn cmd_is_the_pair_and_cmd_shift_is_all_three() {
        let m = ShortcutKey::CtrlShift;
        assert_eq!(m.held_for(false, false, false), Some(held(true, true, false)), "Cmd+X is Ctrl+Shift+X");
        assert_eq!(m.held_for(true, false, false), Some(held(true, true, true)), "Cmd+Shift+X is Ctrl+Shift+Alt+X");
        assert_eq!(m.held_for(false, true, false), Some(held(false, true, true)), "Cmd+Option+X is Alt+Shift+X");
        assert_eq!(m.held_for(true, true, false), None, "no pair is left for Cmd+Option+Shift");
        assert_eq!(m.held_for(false, false, true), Some(held(true, false, true)), "Cmd+Ctrl is the one pair left: Ctrl+Alt");
        assert_eq!(m.held_for(true, false, true), None, "Cmd+Ctrl+Shift has no pair");
        assert_eq!(m.held_for(false, true, true), None, "Cmd+Ctrl+Option has no pair");
        assert_eq!(m.held_for(true, true, true), None, "four modifiers have no pair");
        let a = ShortcutKey::CtrlAlt;
        assert_eq!(a.held_for(false, false, false), Some(held(true, false, true)));
        assert_eq!(a.held_for(true, false, false), Some(held(true, true, true)));
        assert_eq!(a.held_for(false, true, false), Some(held(true, true, false)));
        assert_eq!(a.held_for(false, false, true), Some(held(false, true, true)), "Cmd+Ctrl is Shift+Alt in Ctrl+Alt mode");
    }

    #[test]
    fn no_two_pairs_share_a_press() {
        for mode in MODES {
            let mut seen = Vec::new();
            for (s, a, c) in [(false, false, false), (true, false, false), (false, true, false), (false, false, true)] {
                let h = mode.held_for(s, a, c).unwrap();
                assert!(!seen.contains(&h), "{mode:?} {s} {a} {c}");
                seen.push(h);
            }
        }
    }

    #[test]
    fn a_press_maps_back_to_the_cmd_it_stands_for() {
        let m = ShortcutKey::CtrlShift;
        let l = m.logical(phys(true, true, false, false));
        assert_eq!((l.cmd, l.shift, l.alt, l.ctrl), (true, false, false, false));
        let l = m.logical(phys(true, true, true, false));
        assert_eq!((l.cmd, l.shift, l.alt, l.ctrl), (true, true, false, false));
        let l = m.logical(phys(false, true, true, false));
        assert_eq!((l.cmd, l.shift, l.alt, l.ctrl), (true, false, true, false));
        let l = m.logical(phys(true, false, true, false));
        assert_eq!((l.cmd, l.shift, l.alt, l.ctrl), (true, false, false, true), "Ctrl+Alt is Cmd+Ctrl");
        for p in [phys(true, false, false, false), phys(false, true, false, false), phys(false, false, true, false)] {
            assert_eq!(m.logical(p), p, "not Cmd, untouched");
        }
        // A held Win key is Windows' own and a click's Cmd: no pair is read from it.
        let p = phys(false, false, false, true);
        assert_eq!(m.logical(p), p);
        let l = ShortcutKey::CtrlAlt.logical(phys(true, false, true, false));
        assert_eq!((l.cmd, l.shift, l.alt, l.ctrl), (true, false, false, false));
        let l = ShortcutKey::CtrlAlt.logical(phys(false, true, true, false));
        assert_eq!((l.cmd, l.shift, l.alt, l.ctrl), (true, false, false, true), "Shift+Alt is Cmd+Ctrl in Ctrl+Alt mode");
    }

    #[test]
    fn altgr_typing_is_not_a_shortcut_and_win_is_never_one() {
        let a = ShortcutKey::CtrlAlt;
        assert!(a.press_as_cmd(phys(true, false, true, false), false).is_some(), "Ctrl+Alt+X with no character is Cmd+X");
        assert!(a.press_as_cmd(phys(true, false, true, false), true).is_none(), "AltGr+Q typing @ is left alone");
        assert!(ShortcutKey::CtrlShift.press_as_cmd(phys(true, true, false, false), true).is_some(), "Ctrl+Shift types no character");
        assert!(ShortcutKey::CtrlShift.press_as_cmd(phys(true, true, true, false), true).is_none(), "Ctrl+Alt in it is AltGr too");
        for m in MODES {
            assert!(m.press_as_cmd(phys(true, true, false, true), false).is_none(), "Win held: Windows' chord");
            assert!(m.press_chords(phys(true, true, false, true), false, &ChordKey::Char('d')).is_empty());
            assert!(m.press_chords(phys(true, true, true, false), true, &ChordKey::Char('n')).is_empty(), "AltGr typing");
        }
        assert!(ShortcutKey::CtrlShift.press_as_cmd(phys(true, false, false, false), false).is_none(), "Ctrl alone is the shell's");
        assert!(ShortcutKey::CtrlShift.press_as_cmd(phys(false, false, true, false), false).is_none(), "Alt alone is Meta");
        assert!(warning(ShortcutKey::CtrlAlt).is_some_and(|w| w.contains("AltGr")));
        assert!(warning(ShortcutKey::CtrlShift).is_none());
    }

    /// TWEAKS 9 review I2: recording Ctrl+Shift+D stores Cmd+D (the key `d`, the COMMAND flag).
    #[test]
    fn the_recorder_stores_the_shortcut_key_as_cmd() {
        use crate::keystroke::{Keystroke, COMMAND};
        let m = ShortcutKey::CtrlShift;
        let p = phys(true, true, false, false);
        let r = m.recorded_mods(p, false);
        assert_eq!((r.cmd, r.shift, r.alt, r.ctrl), (true, false, false, false));
        assert!(m.recorded_mods(phys(true, true, true, false), false).shift, "Ctrl+Shift+Alt is Cmd+Shift");
        // Ctrl alone is recorded as pressed, and so is a Win press (the recorder refuses it).
        assert_eq!(m.recorded_mods(phys(false, false, false, true), false), phys(false, false, false, true));
        assert_eq!(m.recorded_mods(phys(true, false, false, false), false), phys(true, false, false, false));
        // AltGr typing is never a shortcut, so it is recorded as pressed.
        let a = phys(true, false, true, false);
        assert_eq!(ShortcutKey::CtrlAlt.recorded_mods(a, true), a);
        // The stroke: the capital D from the held Shift becomes d.
        let raw = Keystroke::new('D' as u32, crate::keystroke::from_mods(r), None, 'D' as u32);
        let s = recorded_stroke(raw, p, r);
        assert_eq!((s.ch, s.flags), ('d' as u32, COMMAND));
        let cs = ShortcutKey::CtrlShift.recorded_mods(phys(true, true, true, false), false);
        let raw = Keystroke::new('D' as u32, 0, None, 'D' as u32);
        assert_eq!(recorded_stroke(raw, phys(true, true, true, false), cs).ch, 'D' as u32, "Cmd+Shift keeps the shifted character");
        // A moved chord is recorded as iTerm2's chord: Shift+Alt+PgUp is Cmd+Option+Shift+[ (Move Tab Left).
        let c = m.table_chord(phys(false, true, true, false), false, &ChordKey::PageUp).expect("Move Tab Left");
        assert_eq!(c, Chord { key: ChordKey::Char('['), cmd: true, alt: true, shift: true, ctrl: false });
        assert!(m.table_chord(p, false, &ChordKey::Char('d')).is_none(), "a pair is not in the table");
    }

    /// TWEAKS 9 review I3: the password manager and AI Chat windows take the chosen chord.
    #[test]
    fn windows_without_menus_take_the_shortcut_key_chord() {
        let m = ShortcutKey::CtrlShift;
        let c = m.window_chord(phys(true, true, false, false), false, ChordKey::Char('b')).expect("Ctrl+Shift+B is Cmd+B");
        assert_eq!((c.cmd, c.shift, c.alt, c.ctrl, c.key), (true, false, false, false, ChordKey::Char('b')));
        let g = m.window_chord(phys(true, true, true, false), false, ChordKey::Char('g')).expect("Ctrl+Shift+Alt+G is Cmd+Shift+G");
        assert!(g.cmd && g.shift && !g.alt);
        assert!(m.window_chord(phys(true, false, false, false), false, ChordKey::Char('b')).is_none(), "Ctrl+B is the program's");
        assert!(m.window_chord(phys(false, false, false, true), false, ChordKey::Char('b')).is_none(), "Win is Windows'");
        assert!(ShortcutKey::CtrlAlt.window_chord(phys(true, false, true, false), true, ChordKey::Char('e')).is_none(), "AltGr+E types a character");
        // The Cockpit window takes its moved chord.
        for mode in MODES {
            let c = mode.window_chord(phys(false, true, true, false), false, ChordKey::F(8)).expect("Alt+Shift+F8");
            assert!(crate::platform::keys::cockpit_chord(&c), "{c:?}");
        }
    }

    #[test]
    fn menus_show_the_real_keys() {
        let d = chord('d', false, false, false);
        let sd = chord('d', true, false, false);
        assert_eq!(ShortcutKey::CtrlShift.label(&d), "Ctrl+Shift+D");
        assert_eq!(ShortcutKey::CtrlShift.label(&sd), "Ctrl+Shift+Alt+D");
        assert_eq!(ShortcutKey::CtrlShift.label(&chord('d', false, true, false)), "Alt+Shift+D");
        assert_eq!(ShortcutKey::CtrlAlt.label(&d), "Ctrl+Alt+D");
        let cc = chord('d', false, false, true);
        assert!(ShortcutKey::CtrlShift.reachable(&cc));
        assert_eq!(ShortcutKey::CtrlShift.label(&cc), "Ctrl+Alt+D");
        assert_eq!(ShortcutKey::CtrlAlt.label(&cc), "Alt+Shift+D");
        // Cmd+Option+Shift+D was Win only; it has no entry, so it is not shown, not spelled Win.
        let none = chord('d', true, true, false);
        assert!(!ShortcutKey::CtrlShift.reaches(&none) && !ShortcutKey::CtrlAlt.reaches(&none));
        assert!(!ShortcutKey::CtrlShift.label(&none).contains("Win"));
        // A moved chord shows its own keys, in both modes.
        let tab_left = Chord { key: ChordKey::Char('['), cmd: true, alt: true, shift: true, ctrl: false };
        for mode in MODES {
            assert_eq!(mode.label(&tab_left), "Alt+Shift+PgUp");
        }
        // A chord without Cmd is untouched.
        let plain = Chord { cmd: false, ..d.clone() };
        assert_eq!(ShortcutKey::CtrlShift.label(&plain), "D");
        assert!(ShortcutKey::CtrlShift.reachable(&plain));
        // Ctrl+Shift+L is Termurion's: Windows keeps Win+L, which no shortcut uses now.
        let l = chord('l', false, false, false);
        assert!(ShortcutKey::CtrlShift.reaches(&l) && ShortcutKey::CtrlAlt.reaches(&l));
        assert!(ShortcutKey::CtrlShift.reaches(&chord('l', false, false, true)), "Cmd+Ctrl+L is Ctrl+Alt+L");
    }

    #[test]
    fn texts_that_name_win_follow_the_setting() {
        let m = ShortcutKey::CtrlShift;
        assert_eq!(m.show("Win+Shift+B"), "Ctrl+Shift+Alt+B");
        assert_eq!(m.show("Win+B"), "Ctrl+Shift+B");
        assert_eq!(m.show("Win+"), "Ctrl+Shift+");
        assert_eq!(m.show("Alt+Win+"), "Alt+Shift+");
        assert_eq!(m.show("Win+click"), "Ctrl+Shift+click");
        assert_eq!(m.show("Ctrl+F1"), "Ctrl+F1");
        assert_eq!(ShortcutKey::CtrlAlt.show("Win+Y"), "Ctrl+Alt+Y");
        assert_eq!(m.show("Ctrl+Win+3"), "Ctrl+Alt+3", "Cmd+Ctrl is the free pair");
        assert_eq!(ShortcutKey::CtrlAlt.show("Ctrl+Win+3"), "Alt+Shift+3");
        // The chords the table moves read with their new keys, whatever the label spells.
        assert_eq!(m.show("Alt+Shift+Win+1"), "Ctrl+Shift+Alt+F1");
        assert_eq!(ShortcutKey::CtrlAlt.show("Alt+Shift+Win+9"), "Ctrl+Shift+Alt+F9");
        assert_eq!(m.show("Ctrl+Alt+Win+3"), "Ctrl+Shift+Alt+3", "a profile's new-window shortcut");
        assert_eq!(m.show("Ctrl+Shift+Win+]"), "Ctrl+Shift+Alt+PgDn");
        // A chord in a sentence or in brackets.
        assert_eq!(m.show("Broadcast (Win+B)"), "Broadcast (Ctrl+Shift+B)");
        assert_eq!(m.show("Win+Y or Win+Alt+V."), "Ctrl+Shift+Y or Alt+Shift+V.");
        assert_eq!(m.show("Hide Windows"), "Hide Windows");
    }

    #[test]
    fn key_labels_parse_as_chord_keys() {
        assert_eq!(parse_key_label("D"), Some(ChordKey::Char('d')));
        assert_eq!(parse_key_label(";"), Some(ChordKey::Char(';')));
        assert_eq!(parse_key_label("PgUp"), Some(ChordKey::PageUp));
        assert_eq!(parse_key_label("Return"), Some(ChordKey::Enter));
        assert_eq!(parse_key_label("F12"), Some(ChordKey::F(12)));
        assert_eq!(parse_key_label("F13"), None);
        assert_eq!(parse_key_label("click"), None);
        for k in [ChordKey::PageUp, ChordKey::PageDown, ChordKey::Up, ChordKey::Home, ChordKey::Delete, ChordKey::F(5), ChordKey::Char('x'), ChordKey::Char('-')] {
            assert_eq!(parse_key_label(&key_label(&k)), Some(k.clone()), "{k:?}");
        }
    }

    /// The wiring: the shortcut key is read before the first menu text and key, an unbound press
    /// reaches the terminal with the keys as pressed (never remapped to Cmd), and a bound one is a
    /// shortcut before any key reporting protocol sees it, as iTerm2's menu is.
    #[test]
    fn the_app_applies_the_mode_where_keys_and_menus_are_made() {
        let menu = include_str!("app/menu.rs");
        let install = menu.split("pub(super) fn install_menus(").nth(1).expect("install_menus");
        assert!(install.find("set_mode(").unwrap() < install.find("Menu::new()").unwrap(), "the mode is set before any menu text");
        let app = include_str!("app/mod.rs");
        let key = app.split("if self.protocol_key_event(ev)").nth(1).expect("the key path");
        assert!(key.find("self.shortcut_chord(ev)").unwrap() < key.find("self.bar_key(ev").unwrap(), "a shortcut before the bars, find, copy mode and the terminal");
        assert!(app.contains("self.shortcut_key_changed();"), "a changed setting reaches the menus");
        let ui = include_str!("app/keyproto_ui.rs");
        let chord = ui.split("fn shortcut_chord(").nth(1).and_then(|r| r.split("\n    }\n").next()).expect("shortcut_chord");
        assert!(chord.contains("chord_bound(&snapshot, chord)") && chord.contains("press_chords("), "only a bound chord is taken; the rest is the terminal's");
        assert!(!chord.contains("user_write"), "a shortcut writes nothing to the program");
    }

    /// TWEAKS 9 review I3: the windows read the shortcut key through `WindowKeys` and label it.
    #[test]
    fn the_pwm_chat_and_cockpit_windows_read_the_shortcut_key() {
        let pwm = include_str!("pwm_ui.rs");
        let prod = pwm.split("#[cfg(test)]").next().unwrap();
        assert!(prod.contains("keys: crate::shortcut_key::WindowKeys") && prod.contains("self.keys.chord(e)"), "pwm_ui");
        assert!(prod.contains("shortcut_key::mode().show(pwm::TITLE_BROADCAST)") && prod.contains("shortcut_key::mode().show(pwm::TOOLTIP_ENTER_USER_NAME)"), "pwm labels");
        let chat = include_str!("chat_window_ui.rs");
        assert!(chat.contains("shortcut_key::WindowKeys") && chat.contains("self.keys.chord(event)"), "chat_window_ui");
        let app = include_str!("app/chat_window_app.rs");
        assert!(app.contains("w.shortcut(event)") && app.contains("filter(crate::platform::keys::chat_chord)"), "the chat window takes its own chord");
        let cockpit = include_str!("cockpit_ui.rs");
        assert!(cockpit.contains("shortcut_key::WindowKeys") && cockpit.contains("self.keys.chord(event)"), "cockpit_ui");
        let app = include_str!("app/cockpit_app.rs");
        assert!(app.contains("w.shortcut(event)") && app.contains("keys::cockpit_chord"), "the Cockpit takes its own chord");
    }

    /// TWEAKS 9 review I4: a literal mapping on the pressed keys is checked before the Cmd chord.
    #[test]
    fn a_literal_mapping_is_asked_before_the_cmd_chord_is_taken() {
        let ui = include_str!("app/keyproto_ui.rs");
        let f = ui.split("fn shortcut_chord(").nth(1).and_then(|r| r.split("\n    }\n").next()).expect("shortcut_chord");
        assert!(f.find("literal_wins(").expect("literal_wins") < f.find("chord_bound(").expect("chord_bound"));
    }

    /// Where a default lives: chords in one scope share the keyboard, so no two may share a press.
    #[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
    enum Scope {
        /// A terminal window and everything the menus and key mappings run.
        App,
        Pwm,
        Chat,
        Cockpit,
    }

    /// Every chord Termurion ships as a default, as (scope, what it is, chord): each menu item, the
    /// extra chord for Make Text Bigger, the shipped global key mappings and the key mapping
    /// presets with Cmd in them, the number shortcuts, the workgroup peers, a profile's shortcuts,
    /// and the chords of the password manager, AI Chat and Cockpit windows.
    fn default_chords() -> Vec<(Scope, String, Chord)> {
        use crate::keystroke::{COMMAND, CONTROL, OPTION, SHIFT};
        let mut out: Vec<(Scope, String, Chord)> = Vec::new();
        for i in crate::menus::shown_items() {
            if let Some(c) = i.chord().filter(|c| c.cmd) {
                out.push((Scope::App, i.id(), c));
            }
        }
        out.push((Scope::App, "Make Text Bigger (Cmd+=)".into(), chord('=', false, false, false)));
        let globals = crate::globals::keymaps::factory_global().iter();
        let presets = crate::globals::keymaps::presets().iter().flat_map(|(_, t)| t.iter());
        for (k, _) in globals.chain(presets) {
            let Some(st) = crate::keystroke::parse(k) else { continue };
            if st.flags & COMMAND == 0 {
                continue;
            }
            let Some(key) = char::from_u32(st.ch).map(|c| ChordKey::Char(c.to_ascii_lowercase())).filter(|_| st.ch < 0xF000).or(match st.ch {
                0xF700 => Some(ChordKey::Up),
                0xF701 => Some(ChordKey::Down),
                0xF702 => Some(ChordKey::Left),
                0xF703 => Some(ChordKey::Right),
                0xF729 => Some(ChordKey::Home),
                0xF72B => Some(ChordKey::End),
                0xF72C => Some(ChordKey::PageUp),
                0xF72D => Some(ChordKey::PageDown),
                _ => None,
            }) else {
                continue;
            };
            out.push((Scope::App, format!("key mapping {k}"), Chord { key, cmd: true, alt: st.flags & OPTION != 0, shift: st.flags & SHIFT != 0, ctrl: st.flags & CONTROL != 0 }));
        }
        // The number shortcuts of the shipped tags (window, tab; pane is None), and their keys 1 to 9.
        for (name, tag) in [("Switch Window", 6), ("Switch Tab", 4), ("Switch Pane", 9)] {
            let Some(mask) = crate::keyroute::tag_mask(tag) else { continue };
            if mask & COMMAND == 0 {
                continue;
            }
            for d in '1'..='9' {
                out.push((Scope::App, format!("{name} {d}"), chord(d, false, mask & OPTION != 0, mask & CONTROL != 0)));
            }
        }
        // Workgroup peers 1 to 9, and a profile's tab shortcut for every key it may hold.
        for d in '1'..='9' {
            out.push((Scope::App, format!("Workgroup peer {d}"), chord(d, true, true, false)));
        }
        for k in ('a'..='z').chain('0'..='9') {
            out.push((Scope::App, format!("Profile shortcut {k}"), chord(k, false, false, true)));
        }
        // The password manager, AI Chat and Cockpit windows.
        for k in ['b', 'u', 'c'] {
            out.push((Scope::Pwm, format!("Password manager {k}"), chord(k, false, false, false)));
        }
        for (k, shift) in [('w', false), ('f', false), ('e', false), ('g', false), ('g', true), ('y', true)] {
            out.push((Scope::Chat, format!("AI Chat {k} {shift}"), chord(k, shift, false, false)));
        }
        out.push((Scope::Cockpit, "Window > Cockpit".into(), chord('c', true, true, false)));
        out
    }

    /// The guard (FIXES 13): in both modes every default has a press, none of them is the Win key
    /// or names it, and no two defaults of one scope share a press. Nothing stays on the Win key
    /// or in a menu only, and a press resolves back to the chord that made it.
    #[test]
    fn every_default_has_a_chord_without_win_and_no_two_collide() {
        let chords = default_chords();
        assert!(chords.len() > 200, "the enumeration found the menus: {}", chords.len());
        for mode in MODES {
            let mut seen: std::collections::HashMap<(Scope, Held, ChordKey), Chord> = std::collections::HashMap::new();
            for (scope, what, c) in &chords {
                let (h, key) = mode.form(c).unwrap_or_else(|| panic!("{mode:?} {what} {c:?} has no chord without Win"));
                assert!(mode.reachable(c) && mode.reaches(c), "{mode:?} {what}");
                // Two modifiers at least: Ctrl, Shift and Alt, never one alone, which is the shell's.
                assert!([h.ctrl, h.shift, h.alt].iter().filter(|b| **b).count() >= 2, "{mode:?} {what}: {h:?}");
                let label = mode.label(c);
                assert!(!label.contains("Win") && !label.contains("Cmd"), "{mode:?} {what}: {label}");
                match seen.get(&(*scope, h, key.clone())) {
                    Some(other) if other != c => panic!("{mode:?} {scope:?}: {what} {label} is the same press as {other:?}"),
                    _ => {
                        seen.insert((*scope, h, key.clone()), c.clone());
                    }
                }
                // Pressing it makes the chord back (the first of the press for a default; a
                // profile's new-window shortcut is the one that can lose to Cmd+Shift).
                let mods = Mods { ctrl: h.ctrl, shift: h.shift, alt: h.alt, cmd: false };
                let back = mode.press_chords(mods, false, &key);
                assert_eq!(back.first(), Some(c), "{mode:?} {what}: {back:?}");
                // The rest of the labels: the text form agrees.
                assert_eq!(mode.show(&crate::menus::chord_label(c)), label, "{what}");
            }
            // A window without menus finds its chords with the first press chord that is not Cmd+Ctrl.
            for (scope, what, c) in chords.iter().filter(|(s, _, _)| *s != Scope::App) {
                let (h, key) = mode.form(c).unwrap();
                let got = mode.window_chord(Mods { ctrl: h.ctrl, shift: h.shift, alt: h.alt, cmd: false }, false, key);
                assert_eq!(got.as_ref(), Some(c), "{mode:?} {scope:?} {what}");
            }
        }
    }

    /// No two table entries share a modifier set and a key, and none of them is the pair press of a
    /// default (the table is checked against every default of the app scope in both modes above,
    /// and here against every pair chord for every key, so a user's own Cmd chord stays reachable
    /// wherever it can).
    #[test]
    fn the_table_is_a_function_both_ways_and_stays_clear_of_the_pairs() {
        for (i, a) in EXTRAS.iter().enumerate() {
            for b in &EXTRAS[i + 1..] {
                assert!(!(a.rest == b.rest && a.key == b.key), "{:?} {:?} twice", a.rest, a.key);
                assert!(!(a.held == b.held && a.press == b.press), "{:?} {:?} pressed twice", a.held, a.press);
            }
            assert!(!matches!(a.press, ChordKey::Up | ChordKey::Down | ChordKey::Left | ChordKey::Right), "arrows are taken in every combination");
        }
        // Every default of the app scope that is a pair chord has no table press.
        let table: std::collections::HashSet<(Held, ChordKey)> = EXTRAS.iter().map(|e| (e.held, e.press.clone())).collect();
        for mode in MODES {
            for (scope, what, c) in default_chords() {
                if scope == Scope::App && extra_of(&c).is_none() && profile_window_key(&c).is_none() {
                    let (h, k) = mode.form(&c).unwrap();
                    assert!(!table.contains(&(h, k)), "{mode:?} {what} is pressed like a moved chord");
                }
            }
        }
    }

    /// The chords that needed the Win key are exactly the table's: every chord with no pair is a
    /// default that has an entry, and every entry is a default (the debug build's Capture GPU Frame
    /// aside, which is hidden in a release build).
    #[test]
    fn the_chords_with_no_free_pair_are_the_tables() {
        let chords = default_chords();
        let mut left = 0;
        for (_, what, c) in &chords {
            let no_pair = ShortcutKey::CtrlShift.held_for(c.shift, c.alt, c.ctrl).is_none();
            assert_eq!(no_pair, ShortcutKey::CtrlAlt.held_for(c.shift, c.alt, c.ctrl).is_none(), "{what}: both modes leave the same chords");
            if no_pair && profile_window_key(c).is_none() {
                assert!(extra_of(c).is_some(), "{what} has no entry");
                left += 1;
            }
        }
        assert!(left >= 34, "{left} chords were Win only");
        for e in EXTRAS {
            let (shift, alt, ctrl) = match e.rest {
                Rest::OptionShift => (true, true, false),
                Rest::CtrlShift => (true, false, true),
                Rest::All => (true, true, true),
            };
            let c = Chord { key: e.key.clone(), cmd: true, alt, shift, ctrl };
            let hidden = e.key == ChordKey::Char('g') && e.rest == Rest::All;
            assert!(hidden || chords.iter().any(|(_, _, d)| *d == c), "{c:?} is no default");
        }
    }

    /// A profile's new-window shortcut is Ctrl+Shift+Alt+key. Cmd+Shift already runs something for
    /// most letters, and that wins; the keys left are the ones a profile can really use.
    #[test]
    fn a_profiles_new_window_shortcut_has_a_press() {
        let defaults = default_chords();
        let mut free = String::new();
        for k in ('a'..='z').chain('0'..='9') {
            let c = chord(k, false, true, true);
            for mode in MODES {
                assert_eq!(mode.form(&c), Some((CSA, ChordKey::Char(k))), "{k}");
                assert_eq!(mode.label(&c), format!("Ctrl+Shift+Alt+{}", k.to_ascii_uppercase()));
            }
            let shadowed = defaults.iter().any(|(s, _, d)| *s == Scope::App && d.shift && !d.alt && !d.ctrl && d.key == ChordKey::Char(k))
                || EXTRAS.iter().any(|e| e.held == CSA && e.press == ChordKey::Char(k));
            if !shadowed {
                free.push(k);
            }
        }
        assert_eq!(free, "qrx123456789", "the keys whose Ctrl+Shift+Alt press opens a profile in a window");
    }

    #[test]
    fn the_number_shortcuts_and_profile_shortcuts_have_a_non_win_form() {
        let profile = chord('1', false, false, true);
        assert_eq!(ShortcutKey::CtrlShift.label(&profile), "Ctrl+Alt+1");
        assert_eq!(ShortcutKey::CtrlAlt.label(&profile), "Alt+Shift+1");
        // The new-window form is Cmd+Ctrl+Option.
        let window = chord('1', false, true, true);
        assert_eq!(ShortcutKey::CtrlShift.label(&window), "Ctrl+Shift+Alt+1");
        // A peer is Cmd+Option+Shift+digit.
        assert_eq!(ShortcutKey::CtrlShift.label(&chord('2', true, true, false)), "Ctrl+Shift+Alt+F2");
    }

    #[test]
    fn a_ctrl_alt_press_is_cmd_ctrl_unless_it_types_a_character() {
        let m = ShortcutKey::CtrlShift;
        let l = m.press_as_cmd(phys(true, false, true, false), false).expect("Ctrl+Alt+F is Cmd+Ctrl+F");
        assert_eq!((l.cmd, l.ctrl, l.shift, l.alt), (true, true, false, false));
        assert!(m.press_as_cmd(phys(true, false, true, false), true).is_none(), "AltGr typing is left to typing");
        // The windows without menus have no Cmd+Ctrl command.
        assert!(m.window_chord(phys(true, false, true, false), false, ChordKey::Char('f')).is_none());
    }

    #[test]
    fn ctrl_v_pastes_only_alone_and_unclaimed() {
        let ctrl = Mods { ctrl: true, ..Mods::default() };
        assert!(ctrl_v_pastes(true, ctrl, false, false));
        assert!(!ctrl_v_pastes(false, ctrl, false, false), "another key");
        assert!(!ctrl_v_pastes(true, Mods::default(), false, false), "a bare v types");
        assert!(!ctrl_v_pastes(true, Mods { shift: true, ..ctrl }, false, false), "Ctrl+Shift+V is the Cmd chord");
        assert!(!ctrl_v_pastes(true, Mods { alt: true, ..ctrl }, false, false), "Ctrl+Alt is AltGr");
        assert!(!ctrl_v_pastes(true, Mods { cmd: true, ..ctrl }, false, false));
        assert!(!ctrl_v_pastes(true, ctrl, true, false), "a key mapping for Ctrl+V wins");
        assert!(!ctrl_v_pastes(true, ctrl, false, true), "Copy Mode keeps Ctrl+V for a box");
        assert!(is_paste_command("Edit > Paste") && !is_paste_command("Edit > Paste Special > Paste Slowly"));
        assert_eq!(PASTE_KEY, "Ctrl+V");
    }

    #[test]
    fn f11_toggles_full_screen_only_bare_and_unclaimed() {
        let none = Mods::default();
        assert!(f11_toggles_full_screen(true, none, false));
        assert!(!f11_toggles_full_screen(false, none, false), "another key");
        assert!(!f11_toggles_full_screen(true, Mods { shift: true, ..none }, false), "Shift+F11 is not it");
        assert!(!f11_toggles_full_screen(true, Mods { ctrl: true, ..none }, false));
        assert!(!f11_toggles_full_screen(true, Mods { alt: true, ..none }, false));
        assert!(!f11_toggles_full_screen(true, Mods { cmd: true, ..none }, false));
        assert!(!f11_toggles_full_screen(true, none, true), "a key mapping for F11 wins");
        // F11 is bound by no shipped menu item, key mapping or preset: it is free to take.
        let f11 = Chord { key: ChordKey::F(11), cmd: false, alt: false, shift: false, ctrl: false };
        assert!(crate::menus::items_for(&f11).is_empty());
        for (k, _) in crate::globals::keymaps::factory_global().iter().chain(crate::globals::keymaps::presets().iter().flat_map(|(_, t)| t.iter())) {
            assert!(crate::keystroke::parse(k).is_none_or(|s| !(s.ch == crate::keystroke::F1 + 10 && s.flags & !crate::keystroke::NUMPAD == 0)), "{k} binds F11");
        }
        assert!(is_full_screen_command("toggleFullScreenMode:") && !is_full_screen_command("maximizePane:"));
        assert_eq!(FULL_SCREEN_KEY, "F11");
        let ui = include_str!("app/keyproto_ui.rs");
        let key = ui.split("fn full_screen_key(").nth(1).and_then(|r| r.split("\n    }\n").next()).expect("full_screen_key");
        assert!(key.contains("keyroute::route(") && key.contains("Route::Terminal"), "a key mapping, the Leader or a number shortcut claims F11 first");
        let app = include_str!("app/mod.rs");
        assert!(app.contains("self.full_screen_key(ev)") && app.contains("\"View > Toggle Full Screen\""), "the key path runs the menu command");
    }

    /// FIXES 13: the keyboard hook no longer takes Win chords for Termurion. It stays for the
    /// modifier remap and the system-wide hotkeys.
    #[test]
    fn the_hook_takes_no_win_chords_for_termurion() {
        let src = include_str!("platform/windows/keys.rs").split("#[cfg(test)]").next().unwrap();
        for gone in ["struct Gate", "enum Verdict", "win_is_cmd", "pwm_hook_takes", "chat_hook_takes", "CHORD_FOREGROUND", "UserEvent::Chord"] {
            assert!(!src.contains(gone), "platform/windows/keys.rs still has {gone}");
        }
        let app = include_str!("app/mod.rs");
        assert!(!app.contains("UserEvent::Chord"), "no chord arrives from the hook");
        // The setting offers no Win, and a recorded Win press is refused with a note.
        let ui = include_str!("settings_ui.rs");
        assert!(ui.contains("refuse_win"), "the recorder says Windows keeps the Win key");
    }
}
