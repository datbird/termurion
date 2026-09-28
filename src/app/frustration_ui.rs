//! iTerm2's mouse reporting frustration offer (`NC:497-526`, `PS:23993-24011`): a drag a
//! program took as mouse reports, then Copy, means the person could not make a selection;
//! a pane bar offers to turn mouse reporting off. The detector is on each `Pane`.

use super::*;
use crate::mouse_frustration::{self as mf, Detector};

/// "Stop Asking" sets this for good (`ASM:782`).
const NEVER_ASK: &str = "NoSyncNeverAskAboutMouseReportingFrustration";

/// What an answer does: turn mouse reporting off (and whether in the profile too), or
/// stop asking. A close does nothing (`NC:510-525`).
#[derive(Debug, PartialEq, Eq)]
enum Answer {
    Off { permanently: bool },
    StopAsking,
    Nothing,
}

fn answer(button: Option<&str>) -> Answer {
    match button {
        Some(b) if b == mf::BUTTONS[0] => Answer::Off { permanently: false },
        Some(b) if b == mf::BUTTONS[1] => Answer::Off { permanently: true },
        Some(b) if b == mf::BUTTONS[2] => Answer::StopAsking,
        _ => Answer::Nothing,
    }
}

impl App {
    /// Feeds a pane's detector.
    pub(super) fn frustration(&mut self, pane: PaneId, feed: impl FnOnce(&mut Detector)) {
        if let Some(p) = self.panes.get_mut(&pane) {
            feed(&mut p.frustration);
        }
    }

    /// Copy ran for `pane`: after a reported drag it offers to turn mouse reporting off,
    /// unless the person stopped the offer.
    pub(super) fn frustration_copy(&mut self, pane: PaneId) {
        let Some(p) = self.panes.get_mut(&pane) else { return };
        if p.frustration.copy() && !self.globals.bool(NEVER_ASK) {
            self.ask(pane, mf::BAR_ID, mf::TEXT, &mf::BUTTONS, true);
        }
    }

    /// The offer's answer. False for other bars.
    pub(super) fn frustration_bar(&mut self, pane: PaneId, id: &str, button: Option<&str>) -> bool {
        if id != mf::BAR_ID {
            return false;
        }
        match answer(button) {
            Answer::Off { permanently } => self.disable_mouse_reporting(pane, permanently),
            Answer::StopAsking => {
                self.globals.set(NEVER_ASK, toml::Value::Boolean(true));
                self.save_globals();
            }
            Answer::Nothing => {}
        }
        true
    }

    /// Turns the program's mouse reporting off (`naggingControllerDisableMouseReportingPermanently:`).
    /// Permanently also sets the profile's "Mouse Reporting" off: the pane's own copy when it
    /// has one, else the saved profile. The profile then decides for the pane, as after any
    /// profile change, so a Toggle Mouse Reporting choice on it ends.
    fn disable_mouse_reporting(&mut self, pane: PaneId, permanently: bool) {
        if permanently {
            let Some(guid) = self.panes.get(&pane).map(|p| p.profile.clone()) else { return };
            let value = toml::Value::Boolean(false);
            if self.store.sessions.iter().any(|s| s.profile.guid() == guid) {
                self.set_session_values(pane, &[("Mouse Reporting".to_string(), Some(value))]);
            } else if let Some(p) = self.store.get_mut(&guid) {
                p.set("Mouse Reporting", value);
                self.save_due = Some(Instant::now());
                self.refresh_panes();
            }
            self.mouse_override.remove(&pane);
        }
        self.turn_off(pane, super::host_offers_ui::Offer::Mouse);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The buttons' meanings (`NC:510-525`).
    #[test]
    fn frustration_answers_follow_iterm2() {
        assert_eq!(answer(Some("_Temporarily")), Answer::Off { permanently: false });
        assert_eq!(answer(Some("Permanently")), Answer::Off { permanently: true });
        assert_eq!(answer(Some("Stop Asking")), Answer::StopAsking);
        assert_eq!(answer(None), Answer::Nothing);
        assert_eq!(NEVER_ASK, "NoSyncNeverAskAboutMouseReportingFrustration");
    }
}
