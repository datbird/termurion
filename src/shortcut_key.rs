//! The shortcut key: which Windows modifiers stand for iTerm2's Cmd (TWEAKS 9).
//!
//! Every Tandem shortcut, key mapping and menu item keeps iTerm2's Cmd notation (`Chord::cmd`,
//! the COMMAND flag in a key binding). This is the one place that says what Cmd is pressed as:
//! Ctrl+Shift (the default), Win, or Ctrl+Alt. Cmd+Shift+X cannot be Ctrl+Shift+Shift+X, so the
//! three modifiers Ctrl, Shift and Alt are shared out as three pairs: Cmd is the chosen pair,
//! Cmd+Shift is all three, Cmd+Option is the remaining pair. A Cmd chord with no such pair
//! (Cmd+Ctrl, Cmd+Option+Shift) stays on the Win key in every mode, so nothing is lost.

use std::sync::atomic::{AtomicU8, Ordering};

use crate::menus::{chord_label, key_label, Chord};
use crate::mouse::Mods;

/// The warning the settings page shows beside a choice, if it needs one.
pub fn warning(m: ShortcutKey) -> Option<&'static str> {
    (m == ShortcutKey::CtrlAlt).then_some("Ctrl+Alt is AltGr on many European keyboard layouts. Tandem leaves a key that types a character alone, but a shortcut on a key AltGr also uses can be hard to press. Ctrl+Shift avoids this.")
}

/// The Keys setting: 0 Ctrl+Shift, 1 Win, 2 Ctrl+Alt.
pub const KEY: &str = "ShortcutKey";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShortcutKey {
    CtrlShift,
    Win,
    CtrlAlt,
}

/// The physical Ctrl, Shift and Alt held (the Win key is not part of it).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Held {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
}

/// What the hook and the key path read. Win until the app sets it from the setting, so code that
/// runs before that (and the hook's own tests) behaves as Tandem always did.
static MODE: AtomicU8 = AtomicU8::new(1);

pub fn mode() -> ShortcutKey {
    ShortcutKey::from_value(i64::from(MODE.load(Ordering::Relaxed)))
}

pub fn set_mode(m: ShortcutKey) {
    MODE.store(m.value() as u8, Ordering::Relaxed);
}

impl ShortcutKey {
    /// An unknown or missing value is the default, Ctrl+Shift.
    pub fn from_value(v: i64) -> ShortcutKey {
        match v {
            1 => ShortcutKey::Win,
            2 => ShortcutKey::CtrlAlt,
            _ => ShortcutKey::CtrlShift,
        }
    }

    pub fn value(self) -> i64 {
        match self {
            ShortcutKey::CtrlShift => 0,
            ShortcutKey::Win => 1,
            ShortcutKey::CtrlAlt => 2,
        }
    }

    /// The setting as saved; unset is the default (an existing user who never chose gets it).
    pub fn from_globals(g: &crate::globals::Globals) -> ShortcutKey {
        ShortcutKey::from_value(g.get(KEY).and_then(|v| v.as_integer()).unwrap_or(0))
    }

    /// The pairs for Cmd, Cmd+Shift and Cmd+Option; None in Win mode.
    fn pairs(self) -> Option<[Held; 3]> {
        let h = |ctrl, shift, alt| Held { ctrl, shift, alt };
        match self {
            ShortcutKey::CtrlShift => Some([h(true, true, false), h(true, true, true), h(false, true, true)]),
            ShortcutKey::CtrlAlt => Some([h(true, false, true), h(true, true, true), h(true, true, false)]),
            ShortcutKey::Win => None,
        }
    }

    /// The physical modifiers a Cmd chord with this Shift, Option and Control needs; None when it
    /// stays on the Win key (this mode is Win, or the chord has no free pair).
    pub fn held_for(self, shift: bool, alt: bool, ctrl: bool) -> Option<Held> {
        let p = self.pairs()?;
        match (ctrl, shift, alt) {
            (false, false, false) => Some(p[0]),
            (false, true, false) => Some(p[1]),
            (false, false, true) => Some(p[2]),
            _ => None,
        }
    }

    /// The Cmd, Shift and Option a physical key press stands for (cmd false when it is none of
    /// the pairs, or the Win key is down: Win is Cmd in every mode).
    pub fn logical(self, phys: Mods) -> Mods {
        let Some(p) = self.pairs() else { return phys };
        if phys.cmd {
            return phys;
        }
        let now = Held { ctrl: phys.ctrl, shift: phys.shift, alt: phys.alt };
        match p.iter().position(|h| *h == now) {
            Some(i) => Mods { cmd: true, shift: i == 1, alt: i == 2, ctrl: false },
            None => phys,
        }
    }

    /// The Cmd (with Shift and Option) a key press makes with this mode's own modifiers, or None
    /// when it is not one: the Win key is down (that is Cmd anyway), the modifiers are no pair, or
    /// the press types a character with Ctrl+Alt held, which is AltGr on a layout that has one.
    pub fn press_as_cmd(self, phys: Mods, types_a_character: bool) -> Option<Mods> {
        if self == ShortcutKey::Win || phys.cmd || (phys.ctrl && phys.alt && types_a_character) {
            return None;
        }
        Some(self.logical(phys)).filter(|l| l.cmd)
    }

    /// Whether a Cmd chord is pressed without the Win key in this mode.
    pub fn reachable(self, c: &Chord) -> bool {
        !c.cmd || self == ShortcutKey::Win || self.held_for(c.shift, c.alt, c.ctrl).is_some()
    }

    /// Whether the chord reaches Tandem: Windows keeps Win+L, but Ctrl+Shift+L is Tandem's.
    pub fn reaches(self, c: &Chord) -> bool {
        if c.cmd && self != ShortcutKey::Win && self.reachable(c) {
            return true;
        }
        crate::menus::reaches_tandem(c)
    }

    /// How a menu shows a chord: the real keys.
    pub fn label(self, c: &Chord) -> String {
        match c.cmd.then(|| self.held_for(c.shift, c.alt, c.ctrl)).flatten() {
            Some(h) => held_label(h) + &key_label(&c.key),
            None => chord_label(c),
        }
    }

    /// A text with iTerm2's Cmd spelled as Win ("Win+Shift+B", "Alt+Win+") shown with the real keys.
    /// Only for a chord or a prefix of one; a sentence is left alone by the caller.
    pub fn show(self, text: &str) -> String {
        if self == ShortcutKey::Win || !text.split('+').any(|t| t == "Win") {
            return text.to_string();
        }
        let tokens: Vec<&str> = text.split('+').collect();
        let has = |name: &str| tokens.iter().any(|t| *t == name);
        let Some(h) = self.held_for(has("Shift"), has("Alt"), has("Ctrl")) else { return text.to_string() };
        let rest: Vec<&str> = tokens.into_iter().filter(|t| !matches!(*t, "Win" | "Ctrl" | "Alt" | "Shift")).collect();
        held_label(h) + &rest.join("+")
    }
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

    #[test]
    fn the_setting_defaults_to_ctrl_shift_and_unknown_values_too() {
        let s = crate::schema::find_global(KEY).expect("ShortcutKey is a Keys setting");
        assert_eq!((s.tab.as_str(), s.section.as_str()), ("Keys", "Shortcut Key"));
        assert_eq!(s.default.as_ref().and_then(|v| v.as_integer()), Some(0));
        assert_eq!(ShortcutKey::from_value(0), ShortcutKey::CtrlShift);
        assert_eq!(ShortcutKey::from_value(1), ShortcutKey::Win);
        assert_eq!(ShortcutKey::from_value(2), ShortcutKey::CtrlAlt);
        assert_eq!(ShortcutKey::from_value(9), ShortcutKey::CtrlShift);
        assert_eq!(ShortcutKey::from_value(-1), ShortcutKey::CtrlShift);
        for m in [ShortcutKey::CtrlShift, ShortcutKey::Win, ShortcutKey::CtrlAlt] {
            assert_eq!(ShortcutKey::from_value(m.value()), m);
        }
    }

    #[test]
    fn cmd_is_the_pair_and_cmd_shift_is_all_three() {
        let m = ShortcutKey::CtrlShift;
        assert_eq!(m.held_for(false, false, false), Some(held(true, true, false)), "Cmd+X is Ctrl+Shift+X");
        assert_eq!(m.held_for(true, false, false), Some(held(true, true, true)), "Cmd+Shift+X is Ctrl+Shift+Alt+X");
        assert_eq!(m.held_for(false, true, false), Some(held(false, true, true)), "Cmd+Option+X is Alt+Shift+X");
        assert_eq!(m.held_for(true, true, false), None, "no pair is left for Cmd+Option+Shift");
        assert_eq!(m.held_for(false, false, true), None, "Cmd+Ctrl stays on Win");
        let a = ShortcutKey::CtrlAlt;
        assert_eq!(a.held_for(false, false, false), Some(held(true, false, true)));
        assert_eq!(a.held_for(true, false, false), Some(held(true, true, true)));
        assert_eq!(a.held_for(false, true, false), Some(held(true, true, false)));
        assert_eq!(ShortcutKey::Win.held_for(false, false, false), None);
    }

    #[test]
    fn no_two_chords_share_a_press() {
        for mode in [ShortcutKey::CtrlShift, ShortcutKey::CtrlAlt] {
            let mut seen = Vec::new();
            for (s, a) in [(false, false), (true, false), (false, true), (true, true)] {
                if let Some(h) = mode.held_for(s, a, false) {
                    assert!(!seen.contains(&h), "{mode:?} {s} {a}");
                    seen.push(h);
                }
            }
            assert_eq!(seen.len(), 3);
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
        // Ctrl alone, Shift alone, Ctrl+Alt: not Cmd, untouched.
        for p in [phys(true, false, false, false), phys(false, true, false, false), phys(true, false, true, false)] {
            assert_eq!(m.logical(p), p);
        }
        // The Win key is Cmd in every mode.
        let p = phys(false, false, false, true);
        assert_eq!(m.logical(p), p);
        assert_eq!(ShortcutKey::Win.logical(phys(true, true, false, false)), phys(true, true, false, false));
        let l = ShortcutKey::CtrlAlt.logical(phys(true, false, true, false));
        assert_eq!((l.cmd, l.shift, l.alt, l.ctrl), (true, false, false, false));
    }

    #[test]
    fn altgr_typing_is_not_a_shortcut_and_win_stays_cmd() {
        let a = ShortcutKey::CtrlAlt;
        assert!(a.press_as_cmd(phys(true, false, true, false), false).is_some(), "Ctrl+Alt+X with no character is Cmd+X");
        assert!(a.press_as_cmd(phys(true, false, true, false), true).is_none(), "AltGr+Q typing @ is left alone");
        assert!(ShortcutKey::CtrlShift.press_as_cmd(phys(true, true, false, false), true).is_some(), "Ctrl+Shift types no character");
        assert!(ShortcutKey::CtrlShift.press_as_cmd(phys(true, true, true, false), true).is_none(), "Ctrl+Alt in it is AltGr too");
        assert!(ShortcutKey::CtrlShift.press_as_cmd(phys(true, true, false, true), false).is_none(), "Win held: the hook's chord");
        assert!(ShortcutKey::Win.press_as_cmd(phys(true, true, false, false), false).is_none());
        assert!(ShortcutKey::CtrlShift.press_as_cmd(phys(true, false, false, false), false).is_none(), "Ctrl alone is the shell's");
        assert!(ShortcutKey::CtrlShift.press_as_cmd(phys(false, false, true, false), false).is_none(), "Alt alone is Meta");
        assert!(warning(ShortcutKey::CtrlAlt).is_some_and(|w| w.contains("AltGr")));
        assert!(warning(ShortcutKey::CtrlShift).is_none() && warning(ShortcutKey::Win).is_none());
    }

    #[test]
    fn menus_show_the_real_keys() {
        let d = chord('d', false, false, false);
        let sd = chord('d', true, false, false);
        assert_eq!(ShortcutKey::CtrlShift.label(&d), "Ctrl+Shift+D");
        assert_eq!(ShortcutKey::CtrlShift.label(&sd), "Ctrl+Shift+Alt+D");
        assert_eq!(ShortcutKey::CtrlShift.label(&chord('d', false, true, false)), "Alt+Shift+D");
        assert_eq!(ShortcutKey::CtrlAlt.label(&d), "Ctrl+Alt+D");
        assert_eq!(ShortcutKey::Win.label(&sd), "Win+Shift+D");
        // Not reachable without Win: the Win spelling, and it says it is reachable only there.
        let cc = chord('d', false, false, true);
        assert!(!ShortcutKey::CtrlShift.reachable(&cc));
        assert_eq!(ShortcutKey::CtrlShift.label(&cc), "Win+Ctrl+D");
        // A chord without Cmd is untouched.
        let plain = Chord { cmd: false, ..d.clone() };
        assert_eq!(ShortcutKey::CtrlShift.label(&plain), "D");
        assert!(ShortcutKey::CtrlShift.reachable(&plain));
        // Windows keeps Win+L; Ctrl+Shift+L is ours.
        let l = chord('l', false, false, false);
        assert!(ShortcutKey::CtrlShift.reaches(&l) && ShortcutKey::CtrlAlt.reaches(&l));
        assert!(!ShortcutKey::Win.reaches(&l));
        assert!(!ShortcutKey::CtrlShift.reaches(&chord('l', false, false, true)), "Cmd+Ctrl+L is still Win+Ctrl+L");
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
        assert_eq!(ShortcutKey::Win.show("Win+Shift+B"), "Win+Shift+B");
        assert_eq!(ShortcutKey::CtrlAlt.show("Win+Y"), "Ctrl+Alt+Y");
        assert_eq!(m.show("Ctrl+Win+3"), "Ctrl+Win+3", "no pair for Cmd+Ctrl: stays on Win");
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
        assert!(app.contains("self.shortcut_key_changed();"), "a changed setting reaches the hook and the menus");
        let ui = include_str!("app/keyproto_ui.rs");
        let chord = ui.split("fn shortcut_chord(").nth(1).and_then(|r| r.split("\n    }\n").next()).expect("shortcut_chord");
        assert!(chord.contains("chord_bound(&snapshot, &chord, false)") && chord.contains(".then_some(chord)"), "only a bound chord is taken; the rest is the terminal's");
        assert!(!chord.contains("user_write"), "a shortcut writes nothing to the program");
    }
}
