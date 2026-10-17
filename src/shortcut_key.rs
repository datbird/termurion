//! The shortcut key: which Windows modifiers stand for iTerm2's Cmd (TWEAKS 9).
//!
//! Every Termurion shortcut, key mapping and menu item keeps iTerm2's Cmd notation (`Chord::cmd`,
//! the COMMAND flag in a key binding). This is the one place that says what Cmd is pressed as:
//! Ctrl+Shift (the default), Win, or Ctrl+Alt. Cmd+Shift+X cannot be Ctrl+Shift+Shift+X, so the
//! three modifiers Ctrl, Shift and Alt are shared out as pairs: Cmd is the chosen pair, Cmd+Shift
//! is all three, Cmd+Option is the second pair, and Cmd+Ctrl is the third (the one pair left).
//! Cmd+Ctrl+Shift, Cmd+Option+Shift and Cmd+Ctrl+Option have no pair left. Those chords are not
//! shown in a menu outside the Win mode (the Win key form still works where Windows passes it),
//! and the command stays in its menu.

use std::sync::atomic::{AtomicU8, Ordering};

use crate::menus::{chord_label, key_label, Chord};
use crate::mouse::Mods;

/// The warning the settings page shows beside a choice, if it needs one.
pub fn warning(m: ShortcutKey) -> Option<&'static str> {
    (m == ShortcutKey::CtrlAlt).then_some("Ctrl+Alt is AltGr on many European keyboard layouts. Termurion leaves a key that types a character alone, but a shortcut on a key AltGr also uses can be hard to press. Ctrl+Shift avoids this.")
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
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Held {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
}

/// What the hook and the key path read. Win until the app sets it from the setting, so code that
/// runs before that (and the hook's own tests) behaves as Termurion always did.
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

    /// The pairs for Cmd, Cmd+Shift, Cmd+Option and Cmd+Ctrl; None in Win mode. The last is the
    /// one pair of Ctrl, Shift and Alt the other three leave free.
    fn pairs(self) -> Option<[Held; 4]> {
        let h = |ctrl, shift, alt| Held { ctrl, shift, alt };
        match self {
            ShortcutKey::CtrlShift => Some([h(true, true, false), h(true, true, true), h(false, true, true), h(true, false, true)]),
            ShortcutKey::CtrlAlt => Some([h(true, false, true), h(true, true, true), h(true, true, false), h(false, true, true)]),
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
            (true, false, false) => Some(p[3]),
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
            Some(i) => Mods { cmd: true, shift: i == 1, alt: i == 2, ctrl: i == 3 },
            None => phys,
        }
    }

    /// The Cmd (with Shift, Option and Control) a key press makes with this mode's own modifiers, or None
    /// when it is not one: the Win key is down (that is Cmd anyway), the modifiers are no pair, or
    /// the press types a character with Ctrl+Alt held, which is AltGr on a layout that has one.
    pub fn press_as_cmd(self, phys: Mods, types_a_character: bool) -> Option<Mods> {
        if self == ShortcutKey::Win || phys.cmd || (phys.ctrl && phys.alt && types_a_character) {
            return None;
        }
        Some(self.logical(phys)).filter(|l| l.cmd)
    }

    /// The modifiers the settings key recorder stores for a press: the Cmd (with Shift and Option)
    /// its shortcut-key modifiers stand for, else the keys as pressed.
    pub fn recorded_mods(self, phys: Mods, types_a_character: bool) -> Mods {
        self.press_as_cmd(phys, types_a_character).unwrap_or(phys)
    }

    /// The Cmd chord a key press makes in a Termurion window that has no menu of its own (the
    /// password manager, AI Chat): the shortcut key's own modifiers with `key`. None in Win mode
    /// (the hook carries the Win chord), when the press is no pair or is AltGr typing, and for
    /// Cmd+Ctrl (those windows have no such command).
    pub fn window_chord(self, phys: Mods, types_a_character: bool, key: crate::menus::ChordKey) -> Option<Chord> {
        let l = self.press_as_cmd(phys, types_a_character).filter(|l| !l.ctrl)?;
        Some(Chord { key, cmd: true, alt: l.alt, shift: l.shift, ctrl: false })
    }

    /// Whether a Cmd chord is pressed without the Win key in this mode.
    pub fn reachable(self, c: &Chord) -> bool {
        !c.cmd || self == ShortcutKey::Win || self.held_for(c.shift, c.alt, c.ctrl).is_some()
    }

    /// Whether a menu shows the chord: Windows keeps Win+L, but Ctrl+Shift+L is Termurion's. In a
    /// mode other than Win a Cmd chord with no free pair (Cmd+Option+Shift) is not shown: the
    /// Win key form is not something to teach, and the command stays in its menu.
    pub fn reaches(self, c: &Chord) -> bool {
        if c.cmd && self != ShortcutKey::Win {
            return self.reachable(c);
        }
        crate::menus::reaches_termurion(c)
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

/// The modifiers a window without menus (password manager, AI Chat) has seen, and the Cmd chord a
/// key press makes with them. The hook carries the Win form; this is the shortcut key's own form.
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
                let key = crate::winkeys::vk_to_key(crate::winkeys::vk_of(event.physical_key))?;
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
        assert_eq!(m.held_for(false, false, true), Some(held(true, false, true)), "Cmd+Ctrl is the one pair left: Ctrl+Alt");
        assert_eq!(m.held_for(true, false, true), None, "Cmd+Ctrl+Shift has no pair");
        assert_eq!(m.held_for(false, true, true), None, "Cmd+Ctrl+Option has no pair");
        assert_eq!(m.held_for(true, true, true), None, "four modifiers have no pair");
        let a = ShortcutKey::CtrlAlt;
        assert_eq!(a.held_for(false, false, false), Some(held(true, false, true)));
        assert_eq!(a.held_for(true, false, false), Some(held(true, true, true)));
        assert_eq!(a.held_for(false, true, false), Some(held(true, true, false)));
        assert_eq!(a.held_for(false, false, true), Some(held(false, true, true)), "Cmd+Ctrl is Shift+Alt in Ctrl+Alt mode");
        assert_eq!(ShortcutKey::Win.held_for(false, false, false), None);
    }

    #[test]
    fn no_two_chords_share_a_press() {
        for mode in [ShortcutKey::CtrlShift, ShortcutKey::CtrlAlt] {
            let mut seen = Vec::new();
            for (s, a, c) in [(false, false, false), (true, false, false), (false, true, false), (true, true, false), (false, false, true)] {
                if let Some(h) = mode.held_for(s, a, c) {
                    assert!(!seen.contains(&h), "{mode:?} {s} {a} {c}");
                    seen.push(h);
                }
            }
            assert_eq!(seen.len(), 4, "{mode:?}: four pairs, none shared");
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
        // Ctrl alone, Shift alone, Alt alone: not Cmd, untouched.
        for p in [phys(true, false, false, false), phys(false, true, false, false), phys(false, false, true, false)] {
            assert_eq!(m.logical(p), p);
        }
        // The Win key is Cmd in every mode.
        let p = phys(false, false, false, true);
        assert_eq!(m.logical(p), p);
        assert_eq!(ShortcutKey::Win.logical(phys(true, true, false, false)), phys(true, true, false, false));
        let l = ShortcutKey::CtrlAlt.logical(phys(true, false, true, false));
        assert_eq!((l.cmd, l.shift, l.alt, l.ctrl), (true, false, false, false));
        let l = ShortcutKey::CtrlAlt.logical(phys(false, true, true, false));
        assert_eq!((l.cmd, l.shift, l.alt, l.ctrl), (true, false, false, true), "Shift+Alt is Cmd+Ctrl in Ctrl+Alt mode");
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

    /// TWEAKS 9 review I2: recording Ctrl+Shift+D stores Cmd+D (the key `d`, the COMMAND flag).
    #[test]
    fn the_recorder_stores_the_shortcut_key_as_cmd() {
        use crate::keystroke::{Keystroke, COMMAND};
        let m = ShortcutKey::CtrlShift;
        let p = phys(true, true, false, false);
        let r = m.recorded_mods(p, false);
        assert_eq!((r.cmd, r.shift, r.alt, r.ctrl), (true, false, false, false));
        assert_eq!(m.recorded_mods(phys(true, true, true, false), false).shift, true, "Ctrl+Shift+Alt is Cmd+Shift");
        // Win is Cmd as it always was; Ctrl alone and Win mode are recorded as pressed.
        assert_eq!(m.recorded_mods(phys(false, false, false, true), false), phys(false, false, false, true));
        assert_eq!(m.recorded_mods(phys(true, false, false, false), false), phys(true, false, false, false));
        assert_eq!(ShortcutKey::Win.recorded_mods(p, false), p);
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
    }

    /// TWEAKS 9 review I3: the password manager and AI Chat windows take the chosen chord.
    #[test]
    fn windows_without_menus_take_the_shortcut_key_chord() {
        use crate::menus::ChordKey;
        let m = ShortcutKey::CtrlShift;
        let c = m.window_chord(phys(true, true, false, false), false, ChordKey::Char('b')).expect("Ctrl+Shift+B is Cmd+B");
        assert_eq!((c.cmd, c.shift, c.alt, c.ctrl, c.key), (true, false, false, false, ChordKey::Char('b')));
        let g = m.window_chord(phys(true, true, true, false), false, ChordKey::Char('g')).expect("Ctrl+Shift+Alt+G is Cmd+Shift+G");
        assert!(g.cmd && g.shift && !g.alt);
        assert!(m.window_chord(phys(true, false, false, false), false, ChordKey::Char('b')).is_none(), "Ctrl+B is the program's");
        assert!(m.window_chord(phys(false, false, false, true), false, ChordKey::Char('b')).is_none(), "Win is the hook's");
        assert!(ShortcutKey::Win.window_chord(phys(true, true, false, false), false, ChordKey::Char('b')).is_none());
        assert!(ShortcutKey::CtrlAlt.window_chord(phys(true, false, true, false), true, ChordKey::Char('e')).is_none(), "AltGr+E types a character");
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
        let cc = chord('d', false, false, true);
        assert!(ShortcutKey::CtrlShift.reachable(&cc));
        assert_eq!(ShortcutKey::CtrlShift.label(&cc), "Ctrl+Alt+D");
        assert_eq!(ShortcutKey::CtrlAlt.label(&cc), "Alt+Shift+D");
        // No pair left (Cmd+Option+Shift): not shown outside Win mode, the Win spelling in it.
        let none = chord('d', true, true, false);
        assert!(!ShortcutKey::CtrlShift.reachable(&none) && !ShortcutKey::CtrlShift.reaches(&none) && !ShortcutKey::CtrlAlt.reaches(&none));
        assert_eq!(ShortcutKey::Win.label(&none), "Win+Alt+Shift+D");
        assert!(ShortcutKey::Win.reaches(&none));
        // A chord without Cmd is untouched.
        let plain = Chord { cmd: false, ..d.clone() };
        assert_eq!(ShortcutKey::CtrlShift.label(&plain), "D");
        assert!(ShortcutKey::CtrlShift.reachable(&plain));
        // Windows keeps Win+L; Ctrl+Shift+L is ours.
        let l = chord('l', false, false, false);
        assert!(ShortcutKey::CtrlShift.reaches(&l) && ShortcutKey::CtrlAlt.reaches(&l));
        assert!(!ShortcutKey::Win.reaches(&l));
        assert!(ShortcutKey::CtrlShift.reaches(&chord('l', false, false, true)), "Cmd+Ctrl+L is Ctrl+Alt+L");
        assert!(!ShortcutKey::Win.reaches(&chord('l', false, false, true)), "Win+Ctrl+L is Windows'");
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
        assert_eq!(m.show("Ctrl+Win+3"), "Ctrl+Alt+3", "Cmd+Ctrl is the free pair");
        assert_eq!(ShortcutKey::CtrlAlt.show("Ctrl+Win+3"), "Alt+Shift+3");
        assert_eq!(m.show("Ctrl+Alt+Win+3"), "Ctrl+Alt+Win+3", "no pair for Cmd+Ctrl+Option: stays on Win");
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

    /// TWEAKS 9 review I3: the windows read the shortcut key through `WindowKeys` and label it.
    #[test]
    fn the_pwm_and_chat_windows_read_the_shortcut_key() {
        let pwm = include_str!("pwm_ui.rs");
        let prod = pwm.split("#[cfg(test)]").next().unwrap();
        assert!(prod.contains("keys: crate::shortcut_key::WindowKeys") && prod.contains("self.keys.chord(e)"), "pwm_ui");
        assert!(prod.contains("shortcut_key::mode().show(pwm::TITLE_BROADCAST)") && prod.contains("shortcut_key::mode().show(pwm::TOOLTIP_ENTER_USER_NAME)"), "pwm labels");
        let chat = include_str!("chat_window_ui.rs");
        assert!(chat.contains("shortcut_key::WindowKeys") && chat.contains("self.keys.chord(event)"), "chat_window_ui");
        let app = include_str!("app/chat_window_app.rs");
        assert!(app.contains("w.shortcut(event)") && app.contains("filter(crate::winkeys::chat_chord)"), "the chat window takes its own chord");
    }

    /// TWEAKS 9 review I4: a literal mapping on the pressed keys is checked before the Cmd chord.
    #[test]
    fn a_literal_mapping_is_asked_before_the_cmd_chord_is_taken() {
        let ui = include_str!("app/keyproto_ui.rs");
        let f = ui.split("fn shortcut_chord(").nth(1).and_then(|r| r.split("\n    }\n").next()).expect("shortcut_chord");
        assert!(f.find("literal_wins(").expect("literal_wins") < f.find("chord_bound(").expect("chord_bound"));
    }

    /// Every chord Termurion ships as a default, as (what it is, chord).
    fn default_chords() -> Vec<(String, Chord)> {
        use crate::keystroke::{COMMAND, CONTROL, OPTION, SHIFT};
        let mut out: Vec<(String, Chord)> = Vec::new();
        for i in crate::menus::shown_items() {
            if let Some(c) = i.chord().filter(|c| c.cmd) {
                out.push((i.id(), c));
            }
        }
        // The extra chord for Make Text Bigger.
        out.push(("Make Text Bigger (Cmd+=)".into(), chord('=', false, false, false)));
        // The shipped global key mappings and the key mapping presets with Cmd in them.
        let globals = crate::globals::keymaps::factory_global().iter();
        let presets = crate::globals::keymaps::presets().iter().flat_map(|(_, t)| t.iter());
        for (k, _) in globals.chain(presets) {
            let Some(st) = crate::keystroke::parse(k) else { continue };
            if st.flags & COMMAND == 0 {
                continue;
            }
            let Some(key) = char::from_u32(st.ch).map(|c| ChordKey::Char(c.to_ascii_lowercase())).filter(|_| st.ch < 0xF000).or_else(|| match st.ch {
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
            out.push((format!("key mapping {k}"), Chord { key, cmd: true, alt: st.flags & OPTION != 0, shift: st.flags & SHIFT != 0, ctrl: st.flags & CONTROL != 0 }));
        }
        out
    }

    /// The Cmd chords with a modifier set no pair covers: Cmd with two or more of Ctrl, Shift and
    /// Option, except Cmd+Shift (all three) and Cmd+Option (the pair) which have a pair.
    fn has_no_pair(c: &Chord) -> bool {
        (c.ctrl && (c.shift || c.alt)) || (c.shift && c.alt)
    }

    #[test]
    fn every_default_chord_without_win_is_reachable_and_no_two_collide() {
        let chords = default_chords();
        assert!(chords.len() > 100, "the enumeration found the menus: {}", chords.len());
        for mode in [ShortcutKey::CtrlShift, ShortcutKey::CtrlAlt] {
            let mut seen: std::collections::HashMap<(Held, ChordKey), Chord> = std::collections::HashMap::new();
            for (what, c) in &chords {
                let h = mode.held_for(c.shift, c.alt, c.ctrl);
                // Reachable exactly when the modifiers are one of Cmd, Cmd+Shift, Cmd+Option, Cmd+Ctrl.
                // Cmd+Shift here is all three and Cmd+Option the pair, whatever the others hold.
                let expect_pair = !(c.ctrl && (c.shift || c.alt)) && !(c.shift && c.alt);
                assert_eq!(h.is_some(), expect_pair, "{mode:?} {what} {}", chord_label(c));
                assert_eq!(mode.reachable(c), h.is_some());
                assert_eq!(mode.reaches(c), h.is_some(), "{what}");
                let Some(h) = h else { continue };
                match seen.get(&(h, c.key.clone())) {
                    Some(other) if other != c => panic!("{mode:?}: {what} {} and {} are the same press", chord_label(c), chord_label(other)),
                    _ => {
                        seen.insert((h, c.key.clone()), c.clone());
                    }
                }
                // The label names no Win key.
                assert!(!mode.label(c).contains("Win"), "{mode:?} {what}: {}", mode.label(c));
            }
        }
    }

    /// The chords that stay without a non-Win form, by modifier set: nothing else may be in it.
    #[test]
    fn only_chords_with_no_free_pair_are_left_on_win() {
        for (what, c) in default_chords() {
            let left = ShortcutKey::CtrlShift.held_for(c.shift, c.alt, c.ctrl).is_none();
            assert_eq!(left, has_no_pair(&c), "{what}");
            assert_eq!(left, ShortcutKey::CtrlAlt.held_for(c.shift, c.alt, c.ctrl).is_none(), "{what}: both modes leave the same chords");
            if left {
                // Cmd+Option+Shift, Cmd+Ctrl+Shift, or all four.
                assert!(c.shift || c.alt, "{what}");
            }
        }
        // The command the user was stuck on, Toggle Full Screen (Cmd+Ctrl+F), and Move Divider
        // (Cmd+Ctrl+arrows) are no longer among them.
        for (what, c) in default_chords() {
            if what == "View > Toggle Full Screen" {
                assert_eq!(ShortcutKey::CtrlShift.label(&c), "Ctrl+Alt+F");
                assert_eq!(ShortcutKey::CtrlAlt.label(&c), "Alt+Shift+F");
            }
            if what.starts_with("Session > Move Divider") || what.contains("Move Divider") {
                assert!(ShortcutKey::CtrlShift.reachable(&c), "{what}");
            }
        }
    }

    #[test]
    fn the_number_shortcuts_and_profile_shortcuts_keep_a_non_win_form() {
        use crate::keyroute::tag_mask;
        use crate::keystroke::{COMMAND, OPTION};
        // Switch Tab, Switch Pane, Switch Window: Cmd, Cmd+Option (the shipped tags).
        for tag in [4, 6, 9, 10] {
            let Some(mask) = tag_mask(tag) else { continue };
            let (alt, ctrl) = (mask & OPTION != 0, mask & crate::keystroke::CONTROL != 0);
            assert!(mask & COMMAND == 0 || ShortcutKey::CtrlShift.held_for(false, alt, ctrl).is_some(), "tag {tag}");
        }
        // A profile shortcut is Ctrl+Win+<key>, Cmd+Ctrl+<key> here: Ctrl+Alt+<key>.
        let profile = Chord { key: ChordKey::Char('1'), cmd: true, alt: false, shift: false, ctrl: true };
        assert_eq!(ShortcutKey::CtrlShift.label(&profile), "Ctrl+Alt+1");
        // The alternate (a new window) is Cmd+Ctrl+Option: no pair.
        let alt_form = Chord { alt: true, ..profile };
        assert!(!ShortcutKey::CtrlShift.reachable(&alt_form));
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
}
