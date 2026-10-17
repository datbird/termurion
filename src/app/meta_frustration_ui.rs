//! iTerm2's Option key frustration offer (`PTYSession.m:11558`, `23200-23290`): Alt and a
//! letter, Backspace, Esc in quick succession means the person wanted that Alt key to send
//! Esc+; a pane bar offers to make it so. See `crate::meta_frustration` for the Windows rule.

use super::*;
use crate::meta_frustration::{self as mf, Answer, Press};

/// Seconds on a clock that only moves forward.
fn now() -> f64 {
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_secs_f64()
}

/// What a key is to the detector (`eventIsCandidate:`, `eventIsBackspace:`, `eventIsEsc:`).
/// `letter` is a physical A to Z key; `ctrl_h` is the H key with Ctrl.
pub(super) fn classify(mods: Mods, side: AltSide, letter: bool, ctrl_h: bool, backspace: bool, esc: bool) -> Press {
    let plain = !mods.shift && !mods.alt && !mods.ctrl && !mods.cmd;
    if mods.alt && !mods.shift && !mods.ctrl && !mods.cmd && letter {
        Press::Candidate(side)
    } else if (plain && backspace) || (mods.ctrl && !mods.shift && !mods.alt && !mods.cmd && ctrl_h) {
        Press::Backspace
    } else if plain && esc {
        Press::Esc
    } else {
        Press::Other
    }
}

fn is_letter(code: winit::keyboard::KeyCode) -> bool {
    use winit::keyboard::KeyCode::*;
    matches!(
        code,
        KeyA | KeyB | KeyC | KeyD | KeyE | KeyF | KeyG | KeyH | KeyI | KeyJ | KeyK | KeyL | KeyM | KeyN | KeyO | KeyP | KeyQ | KeyR | KeyS | KeyT | KeyU | KeyV | KeyW | KeyX | KeyY | KeyZ
    )
}

impl App {
    /// A key press the focused session is about to take.
    pub(super) fn meta_frustration_key(&mut self, ev: &KeyEvent) {
        let Some(id) = self.focused_id() else { return };
        let code = match ev.physical_key {
            PhysicalKey::Code(c) => Some(c),
            _ => None,
        };
        let press = classify(
            self.mods,
            self.alt_side,
            code.is_some_and(is_letter),
            code == Some(winit::keyboard::KeyCode::KeyH),
            code == Some(winit::keyboard::KeyCode::Backspace),
            code == Some(winit::keyboard::KeyCode::Escape),
        );
        if let Some(side) = self.meta_frustration.entry(id).or_default().key(now(), press) {
            self.offer_esc_plus(id, side);
        }
    }

    /// `maybeOfferToSetOptionAsEscForLeft:`.
    fn offer_esc_plus(&mut self, pane: PaneId, side: AltSide) {
        if self.globals.bool(mf::NEVER_KEY) {
            return;
        }
        let Some(guid) = self.panes.get(&pane).map(|p| p.profile.clone()) else { return };
        // A session with its own copy of the profile: "This gets gnarly. Let's be conservative."
        if self.store.is_session(&guid) {
            return;
        }
        let Some(profile) = self.store.get(&guid) else { return };
        let opts = KeyOptions::from_profile(profile);
        let sends = match side {
            AltSide::Left => opts.left_option,
            AltSide::Right => opts.right_option,
        };
        if !mf::offerable(sends) {
            return;
        }
        self.meta_offer.insert(pane, side);
        let buttons = mf::buttons(self.store.profiles.len());
        self.ask(pane, mf::BAR_ID, mf::text(side), &buttons, true);
    }

    /// The offer's answer. False for other bars.
    pub(super) fn meta_frustration_bar(&mut self, pane: PaneId, id: &str, button: Option<&str>) -> bool {
        if id != mf::BAR_ID {
            return false;
        }
        let side = self.meta_offer.remove(&pane);
        let count = self.store.profiles.len();
        let Some(side) = side else { return true };
        let key = mf::profile_key(side);
        match mf::answer(count, button) {
            Answer::ThisProfile => {
                let guid = self.panes.get(&pane).map(|p| p.profile.clone()).unwrap_or_default();
                if let Some(p) = self.store.get_mut(&guid) {
                    p.set(key, toml::Value::Integer(2));
                }
            }
            Answer::AllProfiles => {
                for p in self.store.profiles.iter_mut() {
                    p.set(key, toml::Value::Integer(2));
                }
            }
            Answer::StopAsking => {
                self.globals.set(mf::NEVER_KEY, toml::Value::Boolean(true));
                self.save_globals();
                return true;
            }
            Answer::Nothing => return true,
        }
        self.store.merge_sessions();
        self.save_due = Some(Instant::now());
        self.refresh_panes();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mods(shift: bool, alt: bool, ctrl: bool, cmd: bool) -> Mods {
        Mods { shift, alt, ctrl, cmd }
    }

    #[test]
    fn candidates_are_alt_and_a_letter_only() {
        let l = AltSide::Left;
        assert_eq!(classify(mods(false, true, false, false), l, true, false, false, false), Press::Candidate(l));
        assert_eq!(classify(mods(false, true, false, false), AltSide::Right, true, false, false, false), Press::Candidate(AltSide::Right));
        // Shift, Ctrl or the shortcut key beside Alt, or a non-letter, is no candidate.
        assert_eq!(classify(mods(true, true, false, false), l, true, false, false, false), Press::Other);
        assert_eq!(classify(mods(false, true, true, false), l, true, false, false, false), Press::Other);
        assert_eq!(classify(mods(false, true, false, true), l, true, false, false, false), Press::Other);
        assert_eq!(classify(mods(false, true, false, false), l, false, false, false, false), Press::Other);
    }

    #[test]
    fn backspace_and_esc_need_nothing_held() {
        let l = AltSide::Left;
        assert_eq!(classify(mods(false, false, false, false), l, false, false, true, false), Press::Backspace);
        assert_eq!(classify(mods(false, false, true, false), l, true, true, false, false), Press::Backspace);
        assert_eq!(classify(mods(true, false, false, false), l, false, false, true, false), Press::Other);
        assert_eq!(classify(mods(false, false, false, false), l, false, false, false, true), Press::Esc);
        assert_eq!(classify(mods(false, true, false, false), l, false, false, false, true), Press::Other);
    }

    #[test]
    fn the_bar_is_wired_and_fed_from_the_key_path() {
        let m = include_str!("mod.rs");
        assert!(include_str!("infobar_ui.rs").contains("self.meta_frustration_bar(pane, id, button)"), "bar_answered asks the offer");
        assert!(m.contains("self.meta_frustration_key(ev);"), "a key the session takes feeds the detector");
        assert!(m.contains("self.meta_frustration.remove(&id)"), "a closed pane drops its detector");
    }
}
