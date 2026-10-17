//! iTerm2's offers to turn modes off after a host change (`PS:17929-18176`, `NC:528-567`,
//! `PSA:228-242`): when shell integration reports another host, mouse reporting, focus
//! reporting and paste bracketing a program on the previous host left on are turned off,
//! offered in a pane bar, or left alone, as each optional bool says.
//!
//! Carve-out: these offers are announcements, not `iTermWarning`s. Always and Never write a
//! plain optional bool, never `warnings::Spec` or the Suppressed Alerts catalog, so they are
//! not listed in Termurion > Suppressed Alerts (as in iTerm2); the Advanced pane's Unspecified
//! asks again. The Capture Output announcements are the same kind of exception.

use super::*;
use crate::marks::{HostPolicy, LeftOn};

/// A mode an offer turns off.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Offer {
    Focus,
    Mouse,
    Paste,
    /// DEC mode 2048 (resize reports), offered by 13o's stuck-report detector.
    Dec2048,
}

const OFFERS: [Offer; 4] = [Offer::Focus, Offer::Mouse, Offer::Paste, Offer::Dec2048];

/// Paste bracketing's Help button (`NC:560`).
pub(super) const PASTE_HELP_URL: &str = crate::links::HELP_PASTE_BRACKETING;

/// The optional bool that answers an offer for good (`PS:245-246`, `NC:33`).
pub(super) fn offer_key(o: Offer) -> &'static str {
    match o {
        Offer::Focus => "NoSyncTurnOffFocusReportingOnHostChange",
        Offer::Mouse => "NoSyncTurnOffMouseReportingOnHostChange",
        Offer::Paste => "NoSyncTurnOffBracketedPasteOnHostChange",
        Offer::Dec2048 => "NoSyncTurnOffDEC2048OnHostChange",
    }
}

/// iTerm2's announcement identifier, the bar's id (`PS:250-251`, `NC:28`).
pub(super) fn bar_id(o: Offer) -> &'static str {
    match o {
        Offer::Focus => "TurnOffFocusReportingOnHostChange",
        Offer::Mouse => "TurnOffMouseReportingOnHostChange",
        Offer::Paste => "TurnOffBracketedPasteOnHostChange",
        Offer::Dec2048 => "TurnOffDEC2048OnHostChange",
    }
}

pub(super) fn offer_text(o: Offer) -> &'static str {
    match o {
        Offer::Focus => "Looks like focus reporting was left on when an ssh session ended unexpectedly or an app misbehaved. Turn it off?",
        Offer::Mouse => "Looks like mouse reporting was left on when an ssh session ended unexpectedly or an app misbehaved. Turn it off?",
        Offer::Paste => "Looks like paste bracketing was left on when an ssh session ended unexpectedly or an app misbehaved. Turn it off?",
        Offer::Dec2048 => "Looks like resize reporting was left on when an ssh session ended unexpectedly or an app misbehaved. Turn it off?",
    }
}

pub(super) fn offer_buttons(o: Offer) -> &'static [&'static str] {
    match o {
        Offer::Focus | Offer::Mouse | Offer::Dec2048 => &["_Yes", "Always", "Never"],
        Offer::Paste => &["_Yes", "Always", "Never", "Help"],
    }
}

/// An answer: the value to store (None stores nothing) and whether to turn the mode off now.
/// Every offer answers alike; Help and a close do neither (`PS:18090-18108`, `NC:538-563`).
pub(super) fn answer(button: Option<&str>) -> (Option<bool>, bool) {
    match button {
        Some("_Yes") => (None, true),
        Some("Always") => (Some(true), true),
        Some("Never") => (Some(false), false),
        _ => (None, false),
    }
}

/// The Terminal State toggle an offer turns off.
fn toggle_label(o: Offer) -> Option<&'static str> {
    match o {
        Offer::Focus => Some("Focus Reporting"),
        Offer::Mouse => Some("Mouse Reporting"),
        Offer::Paste => Some("Paste Bracketing"),
        // No toggle: the mode is switched off on the screen itself.
        Offer::Dec2048 => None,
    }
}

impl App {
    /// The pane's shell integration reported another account. A waiting outdated scripts
    /// notice goes (`PS:17909`). At a host change the screen found modes the previous host
    /// left on and turned off those whose answer is saved
    /// (`marks::Screen::reset_modes_on_host_change`); the rest are offered here, in iTerm2's
    /// order.
    pub(super) fn host_changed(&mut self, pane: PaneId, left_on: LeftOn) {
        self.drop_shell_version_notice(pane);
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
        // The fake name and fake boot identity are read from the settings each pass.
        crate::host_identity::set_fakes(&self.globals.string("FakeFullyQualifiedDomainName"), &self.globals.string("FakeBootSessionUUID"));
        let local_names: std::sync::Arc<[String]> = super::statusbar_content::local_names().into();
        let ours: Option<std::sync::Arc<str>> = crate::host_identity::current_ours().map(Into::into);
        let allowed: Vec<(PaneId, bool)> = self.panes.keys().map(|id| (*id, self.mouse_allowed(*id))).collect();
        for (id, mouse_allowed) in allowed {
            if let Some(p) = self.panes.get_mut(&id) {
                p.own_mut().host_policy = HostPolicy { focus, mouse, paste, mouse_allowed, local_names: local_names.clone(), ours: ours.clone() };
            }
        }
    }

    /// An offer's answer. False for other bars.
    pub(super) fn host_offer_bar(&mut self, pane: PaneId, id: &str, button: Option<&str>) -> bool {
        let Some(o) = OFFERS.into_iter().find(|o| bar_id(*o) == id) else { return false };
        let (store, off) = answer(button);
        if let Some(v) = store {
            self.globals.set(offer_key(o), toml::Value::Boolean(v));
            self.save_globals();
        }
        if off {
            self.turn_off(pane, o);
        }
        if o == Offer::Paste && button == Some("Help")
            && let Err(e) = super::semantic_ui::shell_open(PASTE_HELP_URL, false) {
                self.banners.set("action", format!("Could not open {PASTE_HELP_URL}: {e}"));
            }
        true
    }

    /// Turns a mode off with its Terminal State toggle's "off" sequence.
    pub(super) fn turn_off(&mut self, pane: PaneId, o: Offer) {
        if let Some(p) = self.panes.get_mut(&pane) {
            match toggle_label(o) {
                Some(label) => {
                    p.own_mut().feed(crate::marks::toggle_off(label));
                }
                None => p.own_mut().set_resize_reports(false),
            }
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
        assert_eq!(answer(Some("_Yes")), (None, true));
        assert_eq!(answer(Some("Always")), (Some(true), true));
        assert_eq!(answer(Some("Never")), (Some(false), false));
        assert_eq!(answer(None), (None, false), "closed");
        assert_eq!(answer(Some("Help")), (None, false));
        // 13o Task 6 (B3): DEC mode 2048's offer shares the answers; iTerm2 has no Advanced row for its key.
        assert_eq!(offer_text(Dec2048), left_on("resize reporting"));
        assert_eq!(offer_buttons(Dec2048), ["_Yes", "Always", "Never"]);
        assert_eq!(offer_key(Dec2048), "NoSyncTurnOffDEC2048OnHostChange");
        assert_eq!(bar_id(Dec2048), "TurnOffDEC2048OnHostChange");
        assert!(crate::schema::find_global("NoSyncTurnOffDEC2048OnHostChange").is_none());
        assert_eq!(PASTE_HELP_URL, "https://github.com/datbird/termurion/blob/main/docs/help/paste-bracketing.md");
    }
}
