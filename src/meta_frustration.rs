//! iTerm2's Option key frustration detector (`iTermMetaFrustrationDetector`) and its offer
//! (`PTYSession maybeOfferToSetOptionAsEscForLeft:`): Alt and a letter, then Backspace within
//! 1.5 s, then Esc within 3 s of the Backspace, is a person who wanted Alt to send Esc+ and
//! got something else. The bar then offers to make that Alt key send Esc+.
//!
//! On Windows "Normal" already sends Esc+ (`keys::with_alt`), so the one Alt setting that
//! does not is Meta (the high bit, which is garbage in UTF-8). The offer is therefore made
//! for a side set to Meta, where iTerm2 makes it for a side set to Normal. Each pane has one
//! detector; it fires once.

use crate::keys::AltSide;

/// The bar's identifier (`PTYSession.m:23258`).
pub const BAR_ID: &str = "OfferToChangeOptionKeyToSendESC";
/// "Stop Asking" sets this for good (`PTYSession.m:23216`).
pub const NEVER_KEY: &str = "NoSyncNeverPromptToChangeOption";

pub const LEFT_TEXT: &str = "You seem frustrated. Would you like the left Alt key to send esc+keystroke?";
pub const RIGHT_TEXT: &str = "You seem frustrated. Would you like the right Alt key to send esc+keystroke?";
pub const YES: &str = "Yes";
pub const STOP_ASKING: &str = "Stop Asking";
pub const THIS_PROFILE: &str = "Change This Profile";
pub const ALL_PROFILES: &str = "Change All Profiles";

/// Backspace must follow the candidate within this many seconds (`MFD:49`).
pub const BACKSPACE_WITHIN: f64 = 1.5;
/// Esc must follow the Backspace within this many seconds (`MFD:62`).
pub const ESC_WITHIN: f64 = 3.0;

/// What a key press is to the detector.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Press {
    /// A letter key with only Alt held (`eventIsCandidate:`), on this side.
    Candidate(AltSide),
    /// Backspace, or Ctrl+H, with nothing else held (`eventIsBackspace:`).
    Backspace,
    /// Esc with nothing else held (`eventIsEsc:`).
    Esc,
    Other,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum State {
    #[default]
    Ready,
    WaitingForBackspace,
    WaitingForEsc,
    Triggered,
}

#[derive(Clone, Debug, Default)]
pub struct Detector {
    state: State,
    last_time: f64,
    last_was_left: bool,
}

impl Detector {
    /// A key the session took, `now` in seconds. The side to offer for when the person is
    /// frustrated (`didSendKeyEvent:`, `MFD:25-71`).
    pub fn key(&mut self, now: f64, press: Press) -> Option<AltSide> {
        match self.state {
            State::Triggered => {}
            State::Ready => {
                if let Press::Candidate(side) = press {
                    self.last_time = now;
                    self.last_was_left = side == AltSide::Left;
                    self.state = State::WaitingForBackspace;
                }
            }
            State::WaitingForBackspace => {
                self.state = if press == Press::Backspace && now - self.last_time < BACKSPACE_WITHIN {
                    self.last_time = now;
                    State::WaitingForEsc
                } else {
                    State::Ready
                };
            }
            State::WaitingForEsc => {
                if press == Press::Esc {
                    if now - self.last_time < ESC_WITHIN {
                        self.state = State::Triggered;
                        return Some(if self.last_was_left { AltSide::Left } else { AltSide::Right });
                    }
                    self.state = State::Ready;
                }
            }
        }
        None
    }
}

/// The question's text for a side.
pub fn text(side: AltSide) -> &'static str {
    match side {
        AltSide::Left => LEFT_TEXT,
        AltSide::Right => RIGHT_TEXT,
    }
}

/// The profile key that holds a side's setting (`KEY_OPTION_KEY_SENDS`, `KEY_RIGHT_OPTION_KEY_SENDS`).
pub fn profile_key(side: AltSide) -> &'static str {
    match side {
        AltSide::Left => "Option Key Sends",
        AltSide::Right => "Right Option Key Sends",
    }
}

/// The bar's buttons: one profile has no choice of scope (`PTYSession.m:23236-23242`).
pub fn buttons(profile_count: usize) -> Vec<&'static str> {
    if profile_count == 1 {
        vec![YES, STOP_ASKING]
    } else {
        vec![THIS_PROFILE, ALL_PROFILES, STOP_ASKING]
    }
}

/// Whether to offer for a side whose setting is `sends` (0 Normal, 1 Meta, 2 Esc+).
/// Windows' Normal and Esc+ both send Esc+, so only Meta has anything to change.
pub fn offerable(sends: i64) -> bool {
    sends == 1
}

/// What an answer does.
#[derive(Debug, PartialEq, Eq)]
pub enum Answer {
    ThisProfile,
    AllProfiles,
    StopAsking,
    Nothing,
}

/// `PTYSession.m:23262-23282`: with one profile the first button changes it. A close does nothing.
pub fn answer(profile_count: usize, button: Option<&str>) -> Answer {
    match button {
        Some(b) if b == YES && profile_count == 1 => Answer::ThisProfile,
        Some(b) if b == THIS_PROFILE => Answer::ThisProfile,
        Some(b) if b == ALL_PROFILES => Answer::AllProfiles,
        Some(b) if b == STOP_ASKING => Answer::StopAsking,
        _ => Answer::Nothing,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const L: Press = Press::Candidate(AltSide::Left);

    #[test]
    fn alt_letter_backspace_esc_fires_for_that_side() {
        let mut d = Detector::default();
        assert_eq!(d.key(0.0, L), None);
        assert_eq!(d.key(1.0, Press::Backspace), None);
        assert_eq!(d.key(3.9, Press::Esc), Some(AltSide::Left));
        // Once only.
        assert_eq!(d.key(4.0, L), None);
        assert_eq!(d.key(4.1, Press::Backspace), None);
        assert_eq!(d.key(4.2, Press::Esc), None);
        let mut r = Detector::default();
        r.key(0.0, Press::Candidate(AltSide::Right));
        r.key(0.5, Press::Backspace);
        assert_eq!(r.key(1.0, Press::Esc), Some(AltSide::Right));
    }

    #[test]
    fn the_windows_are_one_and_a_half_and_three_seconds() {
        // Backspace too late: back to ready, and the Esc then does nothing.
        let mut d = Detector::default();
        d.key(0.0, L);
        d.key(1.5, Press::Backspace);
        assert_eq!(d.key(1.6, Press::Esc), None);
        // Esc too late: back to ready.
        let mut d = Detector::default();
        d.key(0.0, L);
        d.key(1.0, Press::Backspace);
        assert_eq!(d.key(4.0, Press::Esc), None);
        // The next whole run still fires.
        d.key(5.0, L);
        d.key(5.5, Press::Backspace);
        assert_eq!(d.key(6.0, Press::Esc), Some(AltSide::Left));
    }

    #[test]
    fn another_key_between_candidate_and_backspace_resets_but_not_before_esc() {
        let mut d = Detector::default();
        d.key(0.0, L);
        d.key(0.2, Press::Other);
        assert_eq!(d.key(0.4, Press::Backspace), None);
        // Between Backspace and Esc other keys are ignored (MFD:58-70).
        let mut d = Detector::default();
        d.key(0.0, L);
        d.key(0.2, Press::Backspace);
        d.key(0.4, Press::Other);
        d.key(0.5, L);
        assert_eq!(d.key(0.6, Press::Esc), Some(AltSide::Left));
    }

    #[test]
    fn only_meta_has_anything_to_change_on_windows() {
        assert!(offerable(1));
        assert!(!offerable(0) && !offerable(2));
    }

    #[test]
    fn text_and_buttons_are_iterm2s() {
        assert_eq!(text(AltSide::Left), "You seem frustrated. Would you like the left Alt key to send esc+keystroke?");
        assert_eq!(text(AltSide::Right), "You seem frustrated. Would you like the right Alt key to send esc+keystroke?");
        assert_eq!(buttons(1), ["Yes", "Stop Asking"]);
        assert_eq!(buttons(3), ["Change This Profile", "Change All Profiles", "Stop Asking"]);
        assert_eq!(profile_key(AltSide::Left), "Option Key Sends");
        assert_eq!(profile_key(AltSide::Right), "Right Option Key Sends");
    }

    #[test]
    fn answers_follow_the_scope() {
        assert_eq!(answer(1, Some("Yes")), Answer::ThisProfile);
        assert_eq!(answer(2, Some("Change This Profile")), Answer::ThisProfile);
        assert_eq!(answer(2, Some("Change All Profiles")), Answer::AllProfiles);
        assert_eq!(answer(2, Some("Stop Asking")), Answer::StopAsking);
        assert_eq!(answer(1, Some("Stop Asking")), Answer::StopAsking);
        assert_eq!(answer(2, None), Answer::Nothing);
    }
}
