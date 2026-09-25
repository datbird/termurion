//! The bell in the window: the beep, the tab's bell icon, the flash, and the offer to
//! silence a bell that rings too often.

use std::time::Instant;

use super::*;
use crate::bell::{Answer, BellPrefs};

const OFFER_BAR: &str = "annoying-bell";
const SUPPRESS_OFFER: &str = "NoSyncSuppressAnnyoingBellOffer";
const SILENCE_AUTOMATICALLY: &str = "NoSyncSilenceAnnoyingBellAutomatically";

impl App {
    fn bell_prefs(&self, pane: PaneId) -> BellPrefs {
        let p = self.panes.get(&pane).and_then(|p| self.store.get(&p.profile));
        let b = |k: &str| p.is_some_and(|p| p.bool(k));
        BellPrefs {
            silence: b("Silence Bell"),
            icon: b("Visual Bell"),
            flash: b("Flashing Bell"),
            offer: !self.globals.bool(SUPPRESS_OFFER),
            auto_silence: self.globals.bool(SILENCE_AUTOMATICALLY),
        }
    }

    /// Whether the user is looking at this pane (Tandem in front, its tab shown, focused).
    pub(super) fn looking_at(&self, pane: PaneId) -> bool {
        self.focused && self.tabs.get(self.active).is_some_and(|t| t.focused == pane)
    }

    /// A bell from a program or a Ring Bell trigger.
    pub(super) fn ring_bell(&mut self, pane: PaneId) {
        let prefs = self.bell_prefs(pane);
        let now = Instant::now();
        let ring = self.bells.entry(pane).or_default().ring(now, prefs);
        if let Some(e) = ring.effects {
            if e.beep {
                unsafe {
                    windows_sys::Win32::System::Diagnostics::Debug::MessageBeep(windows_sys::Win32::UI::WindowsAndMessaging::MB_OK);
                }
            }
            if e.icon && !self.looking_at(pane) {
                self.bell_icons.insert(pane);
            }
            if e.flash {
                self.flashes.insert(pane, now);
            }
            self.redraw();
        }
        if ring.offer {
            let text = if prefs.silence { "The bell is ringing a lot. Want to suppress all output until things calm down?" } else { "The bell is ringing a lot. Silence it?" };
            self.ask(pane, OFFER_BAR, text, &["_Silence Bell Temporarily", "Suppress _All Output", "Don't Offer Again", "Silence Automatically"], false);
        }
    }

    /// The offer's answer. False for other bars.
    pub(super) fn bell_bar(&mut self, pane: PaneId, id: &str, button: Option<&str>) -> bool {
        if id != OFFER_BAR {
            return false;
        }
        let answer = match button {
            Some("_Silence Bell Temporarily") => Answer::SilenceTemporarily,
            Some("Suppress _All Output") => Answer::SuppressOutput,
            Some("Don't Offer Again") => Answer::Never,
            Some("Silence Automatically") => Answer::Automatically,
            _ => Answer::Dismissed,
        };
        match answer {
            Answer::Never => {
                self.globals.set(SUPPRESS_OFFER, toml::Value::Boolean(true));
                self.save_globals();
            }
            Answer::Automatically => {
                self.globals.set(SILENCE_AUTOMATICALLY, toml::Value::Boolean(true));
                self.save_globals();
            }
            Answer::SuppressOutput => {
                if let Some(p) = self.panes.get_mut(&pane) {
                    p.suppress_output = true;
                }
            }
            _ => {}
        }
        self.bells.entry(pane).or_default().answer(Instant::now(), answer);
        true
    }

    /// A pane the user looks at loses its bell icon.
    pub(super) fn tick_bell(&mut self) {
        if let Some(id) = self.tabs.get(self.active).map(|t| t.focused).filter(|_| self.focused) {
            if self.bell_icons.remove(&id) {
                self.redraw();
            }
        }
    }
}
