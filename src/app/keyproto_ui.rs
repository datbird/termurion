//! Keys under the focused pane's key reporting protocol: presses, repeats, releases, and
//! modifier keys (kitty's flags, modifyOtherKeys, CSI u, raw key reporting).

use winit::keyboard::{Key as WKey, NamedKey};

use super::*;
use crate::keyproto::{self, KeyMode, Kind, Modifier};

impl App {
    /// Remap modifiers globally is off: Termurion remaps only the keys it receives
    /// (`iTermApplication.m:252-285`), and Windows' own chords see the physical keys.
    pub(super) fn remap_in_app_on(&self) -> bool {
        !self.globals.bool("RemapModifiersGlobally")
    }

    /// Sets the modifiers every key reader sees (`mods`, `sides`, the Alt side) from the physical
    /// ones: remapped through the tags when remapping is off globally, as they are otherwise
    /// (with the remap global, Windows already delivers remapped keys).
    pub(super) fn apply_remapped_mods(&mut self) {
        let (mods, sides) = if self.remap_in_app_on() {
            crate::remap::remap_in_app(&crate::remap::Tags::from_globals(&self.globals), self.phys_mods, self.phys_sides)
        } else {
            (self.phys_mods, self.phys_sides)
        };
        self.mods = mods;
        self.sides = sides;
        if mods.alt {
            self.alt_side = if sides.ralt { AltSide::Right } else { AltSide::Left };
        }
    }

    /// The Remap modifiers setting changed: the hook follows it, and the modifiers are read again.
    pub(super) fn remap_setting_changed(&mut self) {
        crate::winkeys::set_remap_global(!self.remap_in_app_on());
        self.apply_remapped_mods();
    }

    /// The modifiers a pointer press sees: the shortcut key's own keys held with a click are Cmd
    /// (Cmd+click opens a link).
    pub(super) fn pointer_mods(&self) -> Mods {
        let l = crate::shortcut_key::mode().logical(self.mods);
        // Cmd+Ctrl is a keyboard chord only: Ctrl+Alt held with a click is what it always was.
        if l.cmd && l.ctrl { self.mods } else { l }
    }

    /// The Cmd chord a key press makes with the shortcut key's own modifiers (Ctrl+Shift+D is
    /// Cmd+D; Alt+Shift+PgUp is Cmd+Option+Shift+[, a chord with no pair of its own), when Termurion has a command, mapping or shortcut for it. Anything else is not a
    /// shortcut and goes on to the terminal with the keys as pressed, whatever key reporting the
    /// program asked for: iTerm2's menu key equivalents come before the terminal's key handler
    /// too (`PTYTextView.m:731`), so a bound shortcut is never sent to the program.
    pub(super) fn shortcut_chord(&self, ev: &KeyEvent) -> Option<crate::menus::Chord> {
        if ev.state != ElementState::Pressed {
            return None;
        }
        let types = ev.text.as_deref().is_some_and(|t| t.chars().any(|c| !c.is_control()));
        let key = crate::winkeys::vk_to_key(crate::winkeys::vk_of(ev.physical_key))?;
        // The chords this press can be, the moved ones first: the Win key is held for none of them.
        let candidates = crate::shortcut_key::mode().press_chords(self.mods, types, &key);
        if candidates.is_empty() {
            return None;
        }
        let snapshot = self.route_snapshot.read().unwrap_or_else(|e| e.into_inner());
        // The person's own mapping for these very keys (a literal Ctrl+Shift+X) runs first, as
        // iTerm2 runs key mappings before the menu's key equivalents: it is not the Cmd chord.
        if crate::keystroke::from_key_event(ev, self.mods).is_some_and(|pressed| snapshot.literal_wins(&pressed)) {
            return None;
        }
        candidates.into_iter().find(|chord| crate::keyroute::chord_bound(&snapshot, chord))
    }

    /// Whether this press is F11 with nothing claiming it: full screen in every shortcut key mode
    /// (`shortcut_key::f11_toggles_full_screen`). A profile or global key mapping, or the Leader,
    /// for F11 comes first, as it does for every other app-level chord.
    pub(super) fn full_screen_key(&self, ev: &KeyEvent) -> bool {
        if ev.state != ElementState::Pressed || ev.repeat || self.leader_on {
            return false;
        }
        let is_f11 = matches!(ev.logical_key, WKey::Named(NamedKey::F11));
        if !is_f11 || !crate::shortcut_key::f11_toggles_full_screen(true, self.mods, false) {
            return false;
        }
        let Some(stroke) = crate::keystroke::from_key_event(ev, self.mods) else { return false };
        let claimed = self.with_route(|c| !matches!(crate::keyroute::route(c, &crate::keyroute::Press { stroke, digit: None }), crate::keyroute::Route::Terminal));
        crate::shortcut_key::f11_toggles_full_screen(true, self.mods, claimed)
    }

    /// The Shortcut key setting changed: the hook, the key path and the menu text follow it.
    pub(super) fn shortcut_key_changed(&mut self) {
        let now = crate::shortcut_key::ShortcutKey::from_globals(&self.globals);
        if now != crate::shortcut_key::mode() {
            crate::shortcut_key::set_mode(now);
            self.refresh_chord_text();
        }
    }

    /// `ev` with the text and character a remapped modifier calls for, when remapping is off and
    /// the remap changed what is held: Alt remapped as Control types Ctrl+C's byte, Control remapped
    /// as Alt types the plain letter. None when nothing differs.
    pub(super) fn remapped_key(&self, ev: &KeyEvent) -> Option<KeyEvent> {
        use winit::platform::scancode::PhysicalKeyExtScancode;
        let changed = self.mods.ctrl != self.phys_mods.ctrl || self.mods.alt != self.phys_mods.alt;
        if !self.remap_in_app_on() || !changed || ev.state != ElementState::Pressed {
            return None;
        }
        let vk = crate::winkeys::vk_of(ev.physical_key);
        let scan = ev.physical_key.to_scancode()?;
        let mut out = ev.clone();
        out.text = crate::winkeys::text_for(vk, scan, self.mods).map(Into::into);
        if matches!(out.logical_key, WKey::Character(_))
            && let Some(t) = crate::winkeys::text_for(vk, scan, Mods { ctrl: false, ..self.mods }) {
                out.logical_key = WKey::Character(t.into());
            }
        Some(out)
    }

    /// The Control and Win key mappings of `pane`'s profile (they only reach kitty's modern mapper).
    pub(super) fn bucky_of(&self, pane: PaneId) -> keyproto::Bucky {
        self.panes.get(&pane).and_then(|p| self.store.get(&p.profile)).map(keyproto::Bucky::from_profile).unwrap_or_default()
    }

    fn focused_bucky(&self) -> keyproto::Bucky {
        self.tabs.get(self.active).map(|t| self.bucky_of(t.focused)).unwrap_or_default()
    }

    /// The focused pane's key reporting.
    pub(super) fn keyboard(&self) -> keyproto::Keyboard {
        self.tabs.get(self.active).and_then(|t| self.panes.get(&t.focused)).map(|p| p.own().keyboard()).unwrap_or_default()
    }

    fn proto_event<'a>(&self, ev: &'a KeyEvent, key: &'a Key, base: Option<&'a Key>, kind: Kind, bucky: keyproto::Bucky) -> keyproto::KeyEvent<'a> {
        let text = ev.text.as_deref().filter(|t| !t.chars().any(char::is_control));
        let mods = Mods { cmd: false, ..self.mods };
        let base = match base {
            Some(Key::Text(b)) => b.chars().next(),
            _ => None,
        };
        // Windows reports AltGr as Ctrl+Alt.
        let altgr = keyproto::is_altgr(mods, text, base);
        keyproto::KeyEvent { key, base, text, mods, kind, altgr, vk: crate::winkeys::vk_of(ev.physical_key), sides: self.sides, bucky }
    }

    /// A press's bytes under the pane's protocol. None: the standard bytes.
    pub(super) fn protocol_bytes(&self, ev: &KeyEvent, key: &Key, base: Option<&Key>) -> Option<Vec<u8>> {
        let kb = self.keyboard();
        if kb == keyproto::Keyboard::default() {
            return None;
        }
        let kind = if ev.repeat { Kind::Repeat } else { Kind::Press };
        keyproto::encode(kb, &self.proto_event(ev, key, base, kind, self.focused_bucky()))
    }

    /// Key releases and modifier keys, when the protocol reports them. True when handled.
    pub(super) fn protocol_key_event(&mut self, ev: &KeyEvent) -> bool {
        let pressed = ev.state == ElementState::Pressed;
        if let Some(which) = modifier_of(ev) {
            let kb = self.keyboard();
            let kind = if !pressed { Kind::Release } else if ev.repeat { Kind::Repeat } else { Kind::Press };
            let bytes = if kb.kitty != 0 {
                keyproto::kitty_modifier(which, self.mods, self.sides, self.focused_bucky(), kind, kb.kitty)
            } else if kb.mode == KeyMode::Raw && kind != Kind::Repeat {
                Some(keyproto::raw_modifiers(after(self.mods, which, pressed)))
            } else {
                None
            };
            if let (Some(b), Some(id)) = (bytes, self.focused_id()) {
                self.user_write(id, &b, crate::broadcast::Write::Key);
            }
            return false;
        }
        if pressed {
            return false;
        }
        // Only keys that went to the program report their release; a Termurion shortcut's key
        // does not.
        let Some(pane) = self.sent_keys.release(ev.physical_key) else { return true };
        let Some(kb) = self.panes.get(&pane).map(|p| p.own().keyboard()) else { return true };
        if !kb.wants_releases() {
            return true;
        }
        let base = keystroke::key_of(&ev.key_without_modifiers(), None);
        let Some(key) = keystroke::key_of(&ev.logical_key, ev.text.as_deref()).or_else(|| base.clone()) else { return true };
        let bytes = keyproto::encode(kb, &self.proto_event(ev, &key, base.as_ref(), Kind::Release, self.bucky_of(pane)));
        if let Some(b) = bytes.filter(|b| !b.is_empty() && self.panes.contains_key(&pane)) {
            self.user_write(pane, &b, crate::broadcast::Write::KeyRelease);
        }
        true
    }
}

/// The modifier key an event is for, left or right.
fn modifier_of(ev: &KeyEvent) -> Option<Modifier> {
    let right = ev.location == KeyLocation::Right;
    Some(match (&ev.logical_key, right) {
        (WKey::Named(NamedKey::Shift), false) => Modifier::LeftShift,
        (WKey::Named(NamedKey::Shift), true) => Modifier::RightShift,
        (WKey::Named(NamedKey::Control), false) => Modifier::LeftCtrl,
        (WKey::Named(NamedKey::Control), true) => Modifier::RightCtrl,
        (WKey::Named(NamedKey::Alt), false) => Modifier::LeftAlt,
        (WKey::Named(NamedKey::Alt | NamedKey::AltGraph), _) => Modifier::RightAlt,
        (WKey::Named(NamedKey::Super | NamedKey::Meta), false) => Modifier::LeftSuper,
        (WKey::Named(NamedKey::Super | NamedKey::Meta), true) => Modifier::RightSuper,
        _ => return None,
    })
}

/// The modifiers after a modifier key goes down or up.
fn after(mut m: Mods, which: Modifier, down: bool) -> Mods {
    match which {
        Modifier::LeftShift | Modifier::RightShift => m.shift = down,
        Modifier::LeftCtrl | Modifier::RightCtrl => m.ctrl = down,
        Modifier::LeftAlt | Modifier::RightAlt => m.alt = down,
        Modifier::LeftSuper | Modifier::RightSuper => m.cmd = down,
    }
    m
}

/// Keys whose press went to a program, and which pane got it, so the release goes there.
#[derive(Default)]
pub(super) struct Held(HashMap<PhysicalKey, PaneId>);

impl Held {
    pub(super) fn press(&mut self, key: PhysicalKey, pane: PaneId) {
        self.0.insert(key, pane);
    }

    /// The pane a released key's press went to.
    pub(super) fn release(&mut self, key: PhysicalKey) -> Option<PaneId> {
        self.0.remove(&key)
    }

    /// Forgets every held key (the window lost focus, so their releases never come).
    pub(super) fn clear(&mut self) {
        self.0.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn releases_go_to_the_pressed_pane() {
        let a = PhysicalKey::Code(KeyCode::KeyA);
        let mut held = Held::default();
        held.press(a, 1);
        assert_eq!(held.release(a), Some(1));
        assert_eq!(held.release(a), None, "one release per press");
        held.press(a, 2);
        held.clear();
        assert_eq!(held.release(a), None, "focus loss forgets held keys");
    }
}
