//! Keys under the focused pane's key reporting protocol: presses, repeats, releases, and
//! modifier keys (kitty's flags, modifyOtherKeys, CSI u, raw key reporting).

use winit::keyboard::{Key as WKey, NamedKey};

use super::*;
use crate::keyproto::{self, KeyMode, Kind, Modifier};

impl App {
    /// The focused pane's key reporting.
    pub(super) fn keyboard(&self) -> keyproto::Keyboard {
        self.tabs.get(self.active).and_then(|t| self.panes.get(&t.focused)).map(|p| p.own().keyboard()).unwrap_or_default()
    }

    fn proto_event<'a>(&self, ev: &'a KeyEvent, key: &'a Key, base: Option<&'a Key>, kind: Kind) -> keyproto::KeyEvent<'a> {
        let text = ev.text.as_deref().filter(|t| !t.chars().any(char::is_control));
        let mods = Mods { cmd: false, ..self.mods };
        let base = match base {
            Some(Key::Text(b)) => b.chars().next(),
            _ => None,
        };
        // Windows reports AltGr as Ctrl+Alt.
        let altgr = keyproto::is_altgr(mods, text, base);
        keyproto::KeyEvent { key, base, text, mods, kind, altgr, vk: crate::winkeys::vk_of(ev.physical_key) }
    }

    /// A press's bytes under the pane's protocol. None: the standard bytes.
    pub(super) fn protocol_bytes(&self, ev: &KeyEvent, key: &Key, base: Option<&Key>) -> Option<Vec<u8>> {
        let kb = self.keyboard();
        if kb == keyproto::Keyboard::default() {
            return None;
        }
        let kind = if ev.repeat { Kind::Repeat } else { Kind::Press };
        keyproto::encode(kb, &self.proto_event(ev, key, base, kind))
    }

    /// Key releases and modifier keys, when the protocol reports them. True when handled.
    pub(super) fn protocol_key_event(&mut self, ev: &KeyEvent) -> bool {
        let pressed = ev.state == ElementState::Pressed;
        if let Some(which) = modifier_of(ev) {
            let kb = self.keyboard();
            let kind = if !pressed { Kind::Release } else if ev.repeat { Kind::Repeat } else { Kind::Press };
            let bytes = if kb.kitty != 0 {
                keyproto::kitty_modifier(which, Mods { cmd: false, ..self.mods }, kind, kb.kitty)
            } else if kb.mode == KeyMode::Raw && kind != Kind::Repeat {
                Some(keyproto::raw_modifiers(after(self.mods, which, pressed)))
            } else {
                None
            };
            if let (Some(b), Some(p)) = (bytes, self.focused_pane()) {
                p.input(&b);
            }
            return false;
        }
        if pressed {
            return false;
        }
        // Only keys that went to the program report their release; a Tandem shortcut's key
        // does not.
        let Some(pane) = self.sent_keys.release(ev.physical_key) else { return true };
        let Some(kb) = self.panes.get(&pane).map(|p| p.own().keyboard()) else { return true };
        if !kb.wants_releases() {
            return true;
        }
        let base = keystroke::key_of(&ev.key_without_modifiers(), None);
        let Some(key) = keystroke::key_of(&ev.logical_key, ev.text.as_deref()).or_else(|| base.clone()) else { return true };
        let bytes = keyproto::encode(kb, &self.proto_event(ev, &key, base.as_ref(), Kind::Release));
        if let (Some(b), Some(p)) = (bytes.filter(|b| !b.is_empty()), self.panes.get_mut(&pane)) {
            p.input(&b);
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
