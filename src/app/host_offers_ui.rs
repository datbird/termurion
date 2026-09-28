//! iTerm2's offers to turn modes off after a host change (`PS:17929-18176`, `NC:528-567`,
//! `PSA:228-242`): when shell integration reports another host, mouse reporting, focus
//! reporting and paste bracketing a program on the previous host left on are turned off,
//! offered in a pane bar, or left alone, as each optional bool says.
//!
//! Carve-out: these offers are announcements, not `iTermWarning`s. Always and Never write a
//! plain optional bool, never `warnings::Spec` or the Suppressed Alerts catalog, so they are
//! not listed in Tandem > Suppressed Alerts (as in iTerm2); the Advanced pane's Unspecified
//! asks again. The Capture Output announcements are the same kind of exception.

use super::*;
use crate::marks::{HostPolicy, LeftOn};

/// A mode an offer turns off.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Offer {
    Focus,
    Mouse,
    Paste,
}

const OFFERS: [Offer; 3] = [Offer::Focus, Offer::Mouse, Offer::Paste];

/// Paste bracketing's Help button (`NC:560`).
pub(super) const PASTE_HELP_URL: &str = "https://iterm2.com/paste_bracketing";

/// The optional bool that answers an offer for good (`PS:245-246`, `NC:33`).
pub(super) fn offer_key(o: Offer) -> &'static str {
    match o {
        Offer::Focus => "NoSyncTurnOffFocusReportingOnHostChange",
        Offer::Mouse => "NoSyncTurnOffMouseReportingOnHostChange",
        Offer::Paste => "NoSyncTurnOffBracketedPasteOnHostChange",
    }
}

/// iTerm2's announcement identifier, the bar's id (`PS:250-251`, `NC:28`).
pub(super) fn bar_id(o: Offer) -> &'static str {
    match o {
        Offer::Focus => "TurnOffFocusReportingOnHostChange",
        Offer::Mouse => "TurnOffMouseReportingOnHostChange",
        Offer::Paste => "TurnOffBracketedPasteOnHostChange",
    }
}

pub(super) fn offer_text(o: Offer) -> &'static str {
    match o {
        Offer::Focus => "Looks like focus reporting was left on when an ssh session ended unexpectedly or an app misbehaved. Turn it off?",
        Offer::Mouse => "Looks like mouse reporting was left on when an ssh session ended unexpectedly or an app misbehaved. Turn it off?",
        Offer::Paste => "Looks like paste bracketing was left on when an ssh session ended unexpectedly or an app misbehaved. Turn it off?",
    }
}

pub(super) fn offer_buttons(o: Offer) -> &'static [&'static str] {
    match o {
        Offer::Focus | Offer::Mouse => &["_Yes", "Always", "Never"],
        Offer::Paste => &["_Yes", "Always", "Never", "Help"],
    }
}

/// An answer: the value to store (None stores nothing) and whether to turn the mode off now.
/// Every offer answers alike; Help and a close do neither (`PS:18090-18108`, `NC:538-563`).
pub(super) fn answer(_offer: Offer, button: Option<&str>) -> (Option<bool>, bool) {
    match button {
        Some("_Yes") => (None, true),
        Some("Always") => (Some(true), true),
        Some("Never") => (Some(false), false),
        _ => (None, false),
    }
}

/// The Terminal State toggle an offer turns off.
fn toggle_label(o: Offer) -> &'static str {
    match o {
        Offer::Focus => "Focus Reporting",
        Offer::Mouse => "Mouse Reporting",
        Offer::Paste => "Paste Bracketing",
    }
}

impl App {
    /// The pane's screen found modes the previous host left on at a host change and turned
    /// off those whose answer is saved (`marks::Screen::reset_modes_on_host_change`); the rest
    /// are offered here, in iTerm2's order.
    pub(super) fn host_changed(&mut self, pane: PaneId, left_on: LeftOn) {
        let asked = [(left_on.mouse, Offer::Mouse), (left_on.focus, Offer::Focus), (left_on.paste, Offer::Paste)];
        for (_, o) in asked.into_iter().filter(|(on, _)| *on) {
            // A waiting offer with the same id is replaced, so one waits per mode.
            self.ask(pane, bar_id(o), offer_text(o), offer_buttons(o), true);
        }
    }

    /// The saved answers and mouse permission each screen decides a host change with, pushed
    /// before output is read.
    pub(super) fn sync_host_policy(&mut self) {
        let saved = |o| self.globals.optional_bool(offer_key(o));
        let (focus, mouse, paste) = (saved(Offer::Focus), saved(Offer::Mouse), saved(Offer::Paste));
        let allowed: Vec<(PaneId, bool)> = self.panes.keys().map(|id| (*id, self.mouse_allowed(*id))).collect();
        for (id, mouse_allowed) in allowed {
            if let Some(p) = self.panes.get_mut(&id) {
                p.own_mut().host_policy = HostPolicy { focus, mouse, paste, mouse_allowed };
            }
        }
    }

    /// An offer's answer. False for other bars.
    pub(super) fn host_offer_bar(&mut self, pane: PaneId, id: &str, button: Option<&str>) -> bool {
        let Some(o) = OFFERS.into_iter().find(|o| bar_id(*o) == id) else { return false };
        let (store, off) = answer(o, button);
        if let Some(v) = store {
            self.globals.set(offer_key(o), toml::Value::Boolean(v));
            self.save_globals();
        }
        if off {
            self.turn_off(pane, o);
        }
        if o == Offer::Paste && button == Some("Help") {
            if let Err(e) = super::semantic_ui::shell_open(PASTE_HELP_URL, None, false) {
                self.banners.set("action", format!("Could not open {PASTE_HELP_URL}: {e}"));
            }
        }
        true
    }

    /// Turns a mode off with its Terminal State toggle's "off" sequence.
    pub(super) fn turn_off(&mut self, pane: PaneId, o: Offer) {
        let off = crate::marks::toggle_off(toggle_label(o));
        if let Some(p) = self.panes.get_mut(&pane) {
            p.own_mut().feed(off);
            self.redraw();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The offers' texts, buttons, keys and answers (`PS:17953-18176`, `NC:528-567`).
    #[test]
    fn offers_follow_iterm2() {
        use Offer::*;
        use crate::marks::{HostStep, host_step};
        assert_eq!(host_step(Some(true)), HostStep::Off);
        assert_eq!(host_step(None), HostStep::Offer);
        assert_eq!(host_step(Some(false)), HostStep::Nothing);
        let left_on = |what: &str| format!("Looks like {what} was left on when an ssh session ended unexpectedly or an app misbehaved. Turn it off?");
        assert_eq!(offer_text(Focus), left_on("focus reporting"));
        assert_eq!(offer_text(Mouse), left_on("mouse reporting"));
        assert_eq!(offer_text(Paste), left_on("paste bracketing"));
        assert_eq!(offer_buttons(Focus), ["_Yes", "Always", "Never"]);
        assert_eq!(offer_buttons(Mouse), ["_Yes", "Always", "Never"]);
        assert_eq!(offer_buttons(Paste), ["_Yes", "Always", "Never", "Help"]);
        assert_eq!(offer_key(Focus), "NoSyncTurnOffFocusReportingOnHostChange");
        assert_eq!(offer_key(Mouse), "NoSyncTurnOffMouseReportingOnHostChange");
        assert_eq!(offer_key(Paste), "NoSyncTurnOffBracketedPasteOnHostChange");
        assert_eq!(bar_id(Focus), "TurnOffFocusReportingOnHostChange");
        assert_eq!(bar_id(Mouse), "TurnOffMouseReportingOnHostChange");
        assert_eq!(bar_id(Paste), "TurnOffBracketedPasteOnHostChange");
        for o in [Focus, Mouse, Paste] {
            assert_eq!(answer(o, Some("_Yes")), (None, true));
            assert_eq!(answer(o, Some("Always")), (Some(true), true));
            assert_eq!(answer(o, Some("Never")), (Some(false), false));
            assert_eq!(answer(o, None), (None, false), "closed");
        }
        assert_eq!(answer(Paste, Some("Help")), (None, false));
        assert_eq!(PASTE_HELP_URL, "https://iterm2.com/paste_bracketing");
    }
}
