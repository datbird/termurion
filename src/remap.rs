//! iTerm2's Remap Modifiers, done the Windows way: while a Termurion window is in front,
//! the keyboard hook swallows a remapped modifier and injects the key it stands for.
//! These are the hook's decisions, kept apart from Win32 so they can be tested.

use crate::globals::Globals;
use crate::keyproto::Sides;
use crate::mouse::Mods;

const LCTRL: u16 = 0xA2;
const RCTRL: u16 = 0xA3;
const LALT: u16 = 0xA4;
const RALT: u16 = 0xA5;
const LWIN: u16 = 0x5B;
const RWIN: u16 = 0x5C;
const LSHIFT: u16 = 0xA0;
const RSHIFT: u16 = 0xA1;
/// Windows sends a fake Left Control with this scan code in front of AltGr.
const ALTGR_FAKE_CTRL_SCAN: u32 = 0x21D;

/// The Remap Modifiers settings, as iTerm2's tag values (iTermPreferences.h).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tags {
    pub left_control: i64,
    pub right_control: i64,
    pub left_option: i64,
    pub right_option: i64,
    pub left_command: i64,
    pub right_command: i64,
}

impl Tags {
    /// Every key acts as itself.
    pub fn identity() -> Tags {
        Tags { left_control: 11, right_control: 12, left_option: 2, right_option: 3, left_command: 7, right_command: 8 }
    }

    pub fn from_globals(g: &Globals) -> Tags {
        Tags {
            left_control: g.int("LeftControl"),
            right_control: g.int("RightControl"),
            left_option: g.int("LeftOption"),
            right_option: g.int("RightOption"),
            left_command: g.int("LeftCommand"),
            right_command: g.int("RightCommand"),
        }
    }

    fn tag_for(&self, vk: u16) -> Option<i64> {
        Some(match vk {
            LCTRL => self.left_control,
            RCTRL => self.right_control,
            LALT => self.left_option,
            RALT => self.right_option,
            LWIN => self.left_command,
            RWIN => self.right_command,
            _ => return None,
        })
    }
}

/// The keys a tag stands for. "Either" tags use the left key; None and Function (which
/// Windows cannot produce) stand for nothing.
pub fn targets(tag: i64) -> Vec<u16> {
    match tag {
        11 => vec![LCTRL],
        12 | 1 => vec![RCTRL],
        2 | 5 => vec![LALT],
        3 => vec![RALT],
        7 | 4 => vec![LWIN],
        8 => vec![RWIN],
        6 => vec![LWIN, LALT],
        _ => vec![],
    }
}

/// What Termurion itself reads when Remap modifiers globally is off (`iTermApplication.m:252-285`):
/// the physical modifiers, rewritten through the tags. Which Alt, Control and Win keys are down comes
/// from `sides`; a modifier in `physical` with no side known counts as the left key. Shift is never
/// remapped. Windows' AltGr (Right Alt with its fake Left Control) is left alone, as the hook does.
pub fn remap_in_app(tags: &Tags, physical: Mods, sides: Sides) -> (Mods, Sides) {
    if *tags == Tags::identity() {
        return (physical, sides);
    }
    let altgr = sides.ralt && sides.lctrl && !sides.rctrl;
    let mut down: Vec<u16> = Vec::new();
    for (on, vk) in [(sides.lctrl, LCTRL), (sides.rctrl, RCTRL), (sides.lalt, LALT), (sides.ralt, RALT), (sides.lwin, LWIN), (sides.rwin, RWIN)] {
        if on {
            down.push(vk);
        }
    }
    for (flag, left, right) in [(physical.ctrl, LCTRL, RCTRL), (physical.alt, LALT, RALT), (physical.cmd, LWIN, RWIN)] {
        if flag && !down.contains(&left) && !down.contains(&right) {
            down.push(left);
        }
    }
    let mut out: Vec<u16> = Vec::new();
    for vk in down {
        let targets = if vk == LCTRL && altgr { vec![vk] } else { tags.tag_for(vk).map_or_else(|| vec![vk], targets) };
        for t in targets {
            if !out.contains(&t) {
                out.push(t);
            }
        }
    }
    let has = |vk: u16| out.contains(&vk);
    let sides = Sides {
        lalt: has(LALT),
        ralt: has(RALT),
        lctrl: has(LCTRL),
        rctrl: has(RCTRL),
        lwin: has(LWIN),
        rwin: has(RWIN),
    };
    let mods = Mods { shift: physical.shift, alt: sides.lalt || sides.ralt, ctrl: sides.lctrl || sides.rctrl, cmd: sides.lwin || sides.rwin };
    (mods, sides)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    Pass,
    /// Swallow the event and inject these keys (virtual key, down) instead.
    Replace(Vec<(u16, bool)>),
}

#[derive(Default)]
pub struct Remapper {
    /// Remapped keys that are down, in press order, with the keys injected for them.
    active: Vec<(u16, Vec<u16>)>,
    /// Physical modifier keys that are down, remapped or not.
    physical: Vec<u16>,
    /// A Do Not Remap Modifiers key is down: the physical modifiers were injected.
    exempt: bool,
}

impl Remapper {
    pub fn active(&self) -> bool {
        !self.active.is_empty()
    }

    /// `global` is Remap modifiers globally: off, no new key is remapped (the hook injects nothing;
    /// Termurion reads its own keys through `remap_in_app`). A key remapped before it went off still
    /// comes up cleanly.
    pub fn on_key(&mut self, vk: u16, scan: u32, down: bool, focused: bool, tags: &Tags, global: bool) -> Verdict {
        // AltGr's fake Left Control is never ours, even while a real Left Control is remapped.
        if vk == LCTRL && scan == ALTGR_FAKE_CTRL_SCAN {
            return Verdict::Pass;
        }
        // A key that was already down (it went down while another app was in front) keeps
        // passing through; remapping its repeat would leave the original key stuck down.
        let was_down = self.physical.contains(&vk);
        if matches!(vk, LCTRL | RCTRL | LALT | RALT | LWIN | RWIN | LSHIFT | RSHIFT) {
            self.physical.retain(|k| *k != vk);
            if down {
                self.physical.push(vk);
            }
        }
        if let Some(i) = self.active.iter().position(|(k, _)| *k == vk) {
            let keys = self.active[i].1.clone();
            if down {
                // During an exempt key the physical key is down already; swallow its repeat.
                return Verdict::Replace(if self.exempt { vec![] } else { keys.into_iter().map(|k| (k, true)).collect() });
            }
            self.active.remove(i);
            if self.exempt {
                // Its physical down was injected for the exempt key; that is what comes up.
                return Verdict::Replace(vec![(vk, false)]);
            }
            return Verdict::Replace(keys.into_iter().rev().map(|k| (k, false)).collect());
        }
        if !down || !focused || !global || was_down {
            return Verdict::Pass;
        }
        let Some(tag) = tags.tag_for(vk) else { return Verdict::Pass };
        let keys = targets(tag);
        if keys == [vk] {
            return Verdict::Pass;
        }
        self.active.push((vk, keys.clone()));
        Verdict::Replace(keys.into_iter().map(|k| (k, true)).collect())
    }

    /// Key-ups for every injected key still down, and forgets them: called when the remap
    /// is turned off, so no modifier stays down in Windows.
    pub fn flush(&mut self) -> Vec<(u16, bool)> {
        self.exempt = false;
        self.active.drain(..).flat_map(|(_, t)| t.into_iter().rev().map(|k| (k, false))).collect()
    }

    /// The modifiers as the keys are physically held, before remapping.
    pub fn physical_mods(&self) -> Mods {
        let has = |a: u16, b: u16| self.physical.contains(&a) || self.physical.contains(&b);
        Mods { shift: has(LSHIFT, RSHIFT), alt: has(LALT, RALT), ctrl: has(LCTRL, RCTRL), cmd: has(LWIN, RWIN) }
    }

    /// Keys to inject for a key bound to Do Not Remap Modifiers: the remapped modifiers
    /// come up, the physical ones go down, then the key.
    pub fn exempt_down(&mut self, vk: u16) -> Vec<(u16, bool)> {
        self.exempt = true;
        let mut out: Vec<(u16, bool)> = self.active.iter().flat_map(|(_, t)| t.iter().rev().map(|k| (*k, false))).collect();
        out.extend(self.active.iter().map(|(k, _)| (*k, true)));
        out.push((vk, true));
        out
    }

    /// The reverse of `exempt_down`, for the key's release.
    pub fn exempt_up(&mut self, vk: u16) -> Vec<(u16, bool)> {
        self.exempt = false;
        let mut out = vec![(vk, false)];
        out.extend(self.active.iter().map(|(k, _)| (*k, false)));
        out.extend(self.active.iter().flat_map(|(_, t)| t.iter().map(|k| (*k, true))));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn swapped() -> Tags {
        Tags { left_control: 7, left_command: 11, ..Tags::identity() }
    }

    #[test]
    fn an_identity_map_passes_everything() {
        let mut r = Remapper::default();
        assert_eq!(r.on_key(0xA2, 0x1D, true, true, &Tags::identity(), true), Verdict::Pass);
        assert!(!r.active());
    }

    #[test]
    fn left_control_becomes_left_win() {
        let mut r = Remapper::default();
        assert_eq!(r.on_key(0xA2, 0x1D, true, true, &swapped(), true), Verdict::Replace(vec![(0x5B, true)]));
        assert!(r.active());
        // Auto-repeat of a held key repeats the target.
        assert_eq!(r.on_key(0xA2, 0x1D, true, true, &swapped(), true), Verdict::Replace(vec![(0x5B, true)]));
        assert_eq!(r.on_key(0xA2, 0x1D, false, true, &swapped(), true), Verdict::Replace(vec![(0x5B, false)]));
        assert!(!r.active());
    }

    #[test]
    fn a_remapped_key_is_released_even_after_focus_leaves() {
        let mut r = Remapper::default();
        r.on_key(0x5B, 0x5B, true, true, &swapped(), true);
        assert_eq!(r.on_key(0x5B, 0x5B, false, false, &swapped(), true), Verdict::Replace(vec![(0xA2, false)]));
    }

    #[test]
    fn nothing_is_remapped_while_another_app_is_in_front() {
        let mut r = Remapper::default();
        assert_eq!(r.on_key(0xA2, 0x1D, true, false, &swapped(), true), Verdict::Pass);
        assert_eq!(r.on_key(0xA2, 0x1D, false, false, &swapped(), true), Verdict::Pass);
    }

    #[test]
    fn altgr_fake_control_is_never_remapped() {
        let mut r = Remapper::default();
        assert_eq!(r.on_key(0xA2, 0x21D, true, true, &swapped(), true), Verdict::Pass);
    }

    #[test]
    fn none_swallows_the_key() {
        let mut r = Remapper::default();
        let t = Tags { right_option: 9, ..Tags::identity() };
        assert_eq!(r.on_key(0xA5, 0x38, true, true, &t, true), Verdict::Replace(vec![]));
        assert_eq!(r.on_key(0xA5, 0x38, false, true, &t, true), Verdict::Replace(vec![]));
    }

    #[test]
    fn win_and_alt_target_presses_both() {
        let mut r = Remapper::default();
        let t = Tags { right_control: 6, ..Tags::identity() };
        assert_eq!(r.on_key(0xA3, 0x1D, true, true, &t, true), Verdict::Replace(vec![(0x5B, true), (0xA4, true)]));
        assert_eq!(r.on_key(0xA3, 0x1D, false, true, &t, true), Verdict::Replace(vec![(0xA4, false), (0x5B, false)]));
    }

    #[test]
    fn other_keys_pass() {
        let mut r = Remapper::default();
        assert_eq!(r.on_key(0x41, 0x1E, true, true, &swapped(), true), Verdict::Pass);
    }

    #[test]
    fn a_key_held_before_focus_arrives_is_never_remapped() {
        let mut r = Remapper::default();
        assert_eq!(r.on_key(0x5B, 0x5B, true, false, &swapped(), true), Verdict::Pass);
        // Auto-repeat after Termurion came to the front.
        assert_eq!(r.on_key(0x5B, 0x5B, true, true, &swapped(), true), Verdict::Pass);
        assert_eq!(r.on_key(0x5B, 0x5B, false, true, &swapped(), true), Verdict::Pass);
    }

    #[test]
    fn releasing_the_modifier_first_during_an_exempt_key_releases_it() {
        let mut r = Remapper::default();
        r.on_key(0xA2, 0x1D, true, true, &swapped(), true);
        r.exempt_down(0x43);
        // Left Control comes up before C: the injected physical Left Control must come up.
        assert_eq!(r.on_key(0xA2, 0x1D, false, true, &swapped(), true), Verdict::Replace(vec![(0xA2, false)]));
        assert_eq!(r.exempt_up(0x43), vec![(0x43, false)]);
    }

    #[test]
    fn physical_modifiers_are_tracked() {
        let mut r = Remapper::default();
        r.on_key(0xA2, 0x1D, true, true, &swapped(), true);
        r.on_key(0xA0, 0x2A, true, true, &swapped(), true); // Left Shift, not remapped
        let m = r.physical_mods();
        assert!(m.ctrl && m.shift && !m.cmd && !m.alt);
    }

    #[test]
    fn an_exempt_key_sees_the_physical_modifiers() {
        let mut r = Remapper::default();
        r.on_key(0xA2, 0x1D, true, true, &swapped(), true); // Left Control down, injected as Left Win
        assert_eq!(r.exempt_down(0x43), vec![(0x5B, false), (0xA2, true), (0x43, true)]);
        assert_eq!(r.exempt_up(0x43), vec![(0x43, false), (0xA2, false), (0x5B, true)]);
    }

    #[test]
    fn in_app_remap_follows_iterm2() {
        let left_alt = |m: Mods| Sides { lalt: m.alt, ..Sides::default() };
        let as_ctrl = Tags { left_option: 11, ..Tags::identity() };
        // Left Alt as Control: Ctrl on, Alt off.
        let alt = Mods { alt: true, ..Mods::default() };
        let (m, s) = remap_in_app(&as_ctrl, alt, left_alt(alt));
        assert!(m.ctrl && !m.alt, "{m:?}");
        assert!(s.lctrl && !s.lalt && !s.rctrl, "{s:?}");
        // Right Alt untouched.
        let ralt = Sides { ralt: true, ..Sides::default() };
        let (m, s) = remap_in_app(&as_ctrl, alt, ralt);
        assert!(m.alt && !m.ctrl && s.ralt && !s.lctrl);
        // Identity tags: unchanged, whatever the sides.
        let (m, s) = remap_in_app(&Tags::identity(), alt, left_alt(alt));
        assert_eq!((m, s), (alt, left_alt(alt)));
        // Shift is never remapped.
        let shift_alt = Mods { shift: true, alt: true, ..Mods::default() };
        assert!(remap_in_app(&as_ctrl, shift_alt, left_alt(shift_alt)).0.shift);
        // Swapping Left Control and Left Win swaps both flags.
        let (m, s) = remap_in_app(&swapped(), Mods { ctrl: true, ..Mods::default() }, Sides { lctrl: true, ..Sides::default() });
        assert!(m.cmd && !m.ctrl && s.lwin && !s.lctrl);
        // Either-key tags use the left key; a Win and Alt target sets both.
        let t = Tags { right_control: 6, ..Tags::identity() };
        let (m, _) = remap_in_app(&t, Mods { ctrl: true, ..Mods::default() }, Sides { rctrl: true, ..Sides::default() });
        assert!(m.cmd && m.alt && !m.ctrl);
        // None swallows the modifier.
        let none = Tags { left_option: 9, ..Tags::identity() };
        assert!(!remap_in_app(&none, alt, left_alt(alt)).0.alt);
        // AltGr (Right Alt with Windows' fake Left Control) is never remapped.
        let altgr_mods = Mods { ctrl: true, alt: true, ..Mods::default() };
        let altgr = Sides { lctrl: true, ralt: true, ..Sides::default() };
        let swap_ctrl = Tags { left_control: 7, ..Tags::identity() };
        assert_eq!(remap_in_app(&swap_ctrl, altgr_mods, altgr), (altgr_mods, altgr));
    }

    #[test]
    fn a_remapper_does_nothing_when_not_global() {
        let mut r = Remapper::default();
        for (vk, scan) in [(0xA2u16, 0x1Du32), (0x5B, 0x5B), (0xA4, 0x38), (0xA5, 0x38), (0x41, 0x1E)] {
            assert_eq!(r.on_key(vk, scan, true, true, &swapped(), false), Verdict::Pass, "vk {vk:#x}");
            assert_eq!(r.on_key(vk, scan, false, true, &swapped(), false), Verdict::Pass, "vk {vk:#x} up");
        }
        assert!(!r.active());
        // With the flag on the same keys are replaced, as before.
        assert_eq!(r.on_key(0xA2, 0x1D, true, true, &swapped(), true), Verdict::Replace(vec![(0x5B, true)]));
    }

    #[test]
    fn turning_it_off_releases_what_is_held() {
        let mut r = Remapper::default();
        r.on_key(0xA2, 0x1D, true, true, &swapped(), true);
        assert_eq!(r.flush(), vec![(0x5B, false)]);
        assert!(!r.active());
        assert!(r.flush().is_empty());
        // The physical release afterwards is a plain pass-through.
        assert_eq!(r.on_key(0xA2, 0x1D, false, true, &swapped(), false), Verdict::Pass);
    }

    #[test]
    fn tags_read_the_globals() {
        let mut g = crate::globals::Globals::empty(std::path::Path::new("unused.toml"));
        g.set("LeftCommand", toml::Value::Integer(11));
        assert_eq!(Tags::from_globals(&g), Tags { left_command: 11, ..Tags::identity() });
    }
}
