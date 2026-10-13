//! iTerm2's mouse reporting frustration detector (`iTermMouseReportingFrustrationDetector`):
//! a drag that a program took as mouse reports, followed by Copy, means the person tried to
//! select text that mouse reporting kept from being selected. Each pane has one.

/// Where the detector is in a drag-then-Copy episode.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum State {
    #[default]
    Ground,
    MouseDown,
    MouseDownMultiple,
    Dragged,
    Primed,
    /// Mouse up after a reported multi-click press.
    PrimedMultiple,
}

#[derive(Clone, Debug, Default)]
pub struct Detector {
    pub state: State,
}

impl Detector {
    /// A left press; `clicks` is its click count (`MF:34-54`).
    pub fn mouse_down(&mut self, clicks: u8, reported: bool) {
        self.state = match self.state {
            State::Ground | State::MouseDown | State::MouseDownMultiple | State::PrimedMultiple if clicks > 0 && reported => {
                if clicks == 1 {
                    State::MouseDown
                } else {
                    State::MouseDownMultiple
                }
            }
            _ => State::Ground,
        };
    }

    /// A left drag (`MF:56-72`).
    pub fn mouse_dragged(&mut self, reported: bool) {
        self.state = match self.state {
            State::MouseDown | State::MouseDownMultiple | State::Dragged | State::PrimedMultiple if reported => State::Dragged,
            _ => State::Ground,
        };
    }

    /// A left release; `clicks` is 0 when the release no longer counts as a click (`MF:74-97`).
    pub fn mouse_up(&mut self, clicks: u8, reported: bool) {
        self.state = match self.state {
            State::MouseDownMultiple | State::PrimedMultiple if reported => State::PrimedMultiple,
            State::Dragged if clicks == 0 && reported => State::Primed,
            _ => State::Ground,
        };
    }

    /// Any key but Copy (`MF:99-110`).
    pub fn key(&mut self) {
        self.state = State::Ground;
    }

    /// Copy (iTerm2's Cmd-C). True when the person was frustrated (`MF:116-131`).
    pub fn copy(&mut self) -> bool {
        let frustrated = matches!(self.state, State::Primed | State::PrimedMultiple | State::MouseDownMultiple | State::Dragged);
        self.state = State::Ground;
        frustrated
    }

    /// A right or middle button event (`MF:112-114`).
    pub fn other_mouse(&mut self) {
        self.state = State::Ground;
    }

    /// The program copied to the clipboard itself (OSC 52): the episode ends unreported;
    /// a lone press is left as it is (`MF:133-148`).
    pub fn copied_by_sequence(&mut self) {
        if matches!(self.state, State::Primed | State::PrimedMultiple | State::MouseDownMultiple | State::Dragged) {
            self.state = State::Ground;
        }
    }
}

/// The offer (`NC:500`).
pub const TEXT: &str = "Looks like you\u{2019}re trying to copy to the pasteboard, but mouse reporting has prevented making a selection. Disable mouse reporting?";

/// The offer's buttons (`NC:505`); "_" marks Alt+T.
pub const BUTTONS: [&str; 3] = ["_Temporarily", "Permanently", "Stop Asking"];

/// iTerm2's announcement identifier, the bar's id (`NC:27`).
pub const BAR_ID: &str = "AskAboutMouseReportingFrustration";

#[cfg(test)]
mod tests {
    use super::*;

    fn at(state: State) -> Detector {
        Detector { state }
    }

    /// The transitions of `MF:34-148`.
    #[test]
    fn frustration_follows_iterm2() {
        // A reported drag, then a release that is not a click, then Copy: detected.
        let mut d = Detector::default();
        d.mouse_down(1, true);
        assert_eq!(d.state, State::MouseDown);
        d.mouse_dragged(true);
        assert_eq!(d.state, State::Dragged);
        d.mouse_up(0, true);
        assert_eq!(d.state, State::Primed);
        assert!(d.copy());
        assert_eq!(d.state, State::Ground);
        // A detection needs a new drag.
        assert!(!d.copy());

        // A click is not a drag.
        let mut d = Detector::default();
        d.mouse_down(1, true);
        d.mouse_up(1, true);
        assert_eq!(d.state, State::Ground);
        assert!(!d.copy());

        // A reported multi-click press, then Copy.
        let mut d = Detector::default();
        d.mouse_down(2, true);
        assert_eq!(d.state, State::MouseDownMultiple);
        assert!(d.copy());
        assert_eq!(d.state, State::Ground);

        // A reported multi-click press and release.
        let mut d = Detector::default();
        d.mouse_down(2, true);
        d.mouse_up(2, true);
        assert_eq!(d.state, State::PrimedMultiple);
        assert!(d.copy());
        assert!(!d.copy());

        // An unreported release after a multi-click press resets.
        let mut d = at(State::MouseDownMultiple);
        d.mouse_up(2, false);
        assert_eq!(d.state, State::Ground);

        // An unreported press stays at Ground; a press with no click count too.
        let mut d = Detector::default();
        d.mouse_down(1, false);
        assert_eq!(d.state, State::Ground);
        d.mouse_down(0, true);
        assert_eq!(d.state, State::Ground);
        // A press while Primed or Dragged starts over at Ground.
        for s in [State::Primed, State::Dragged] {
            let mut d = at(s);
            d.mouse_down(1, true);
            assert_eq!(d.state, State::Ground, "{s:?}");
        }
        // A press after PrimedMultiple starts a new episode.
        let mut d = at(State::PrimedMultiple);
        d.mouse_down(1, true);
        assert_eq!(d.state, State::MouseDown);

        // An unreported drag after the press resets; a drag from Ground or Primed does too.
        for s in [State::MouseDown, State::MouseDownMultiple, State::Dragged, State::PrimedMultiple] {
            let mut d = at(s);
            d.mouse_dragged(false);
            assert_eq!(d.state, State::Ground, "{s:?}");
        }
        for s in [State::Ground, State::Primed] {
            let mut d = at(s);
            d.mouse_dragged(true);
            assert_eq!(d.state, State::Ground, "{s:?}");
        }
        // A reported drag after a multi-click release is a drag.
        let mut d = at(State::PrimedMultiple);
        d.mouse_dragged(true);
        assert_eq!(d.state, State::Dragged);
        // A release still counted as a click after a drag, or unreported, resets.
        let mut d = at(State::Dragged);
        d.mouse_up(1, true);
        assert_eq!(d.state, State::Ground);
        let mut d = at(State::Dragged);
        d.mouse_up(0, false);
        assert_eq!(d.state, State::Ground);

        // Copy right after a drag, before the release, detects.
        let mut d = at(State::Dragged);
        assert!(d.copy());
        // Copy after a single press alone does not.
        let mut d = at(State::MouseDown);
        assert!(!d.copy());
        assert_eq!(d.state, State::Ground);

        // Any other key, another button, or a copy by control sequence resets.
        for s in [State::MouseDown, State::MouseDownMultiple, State::Dragged, State::Primed, State::PrimedMultiple] {
            let mut d = at(s);
            d.key();
            assert_eq!(d.state, State::Ground, "key {s:?}");
            let mut d = at(s);
            d.other_mouse();
            assert_eq!(d.state, State::Ground, "other {s:?}");
        }
        for s in [State::MouseDownMultiple, State::Dragged, State::Primed, State::PrimedMultiple] {
            let mut d = at(s);
            d.copied_by_sequence();
            assert_eq!(d.state, State::Ground, "sequence {s:?}");
            assert!(!d.copy(), "sequence {s:?}");
        }
        // A copy by control sequence leaves a single press alone (`MF:143-145`).
        let mut d = at(State::MouseDown);
        d.copied_by_sequence();
        assert_eq!(d.state, State::MouseDown);
    }

    /// The announcement's words (`NC:497-526`), Cmd-C read as Termurion's Copy.
    #[test]
    fn texts_follow_iterm2() {
        assert_eq!(TEXT, "Looks like you\u{2019}re trying to copy to the pasteboard, but mouse reporting has prevented making a selection. Disable mouse reporting?");
        assert_eq!(BUTTONS, ["_Temporarily", "Permanently", "Stop Asking"]);
        assert_eq!(BAR_ID, "AskAboutMouseReportingFrustration");
    }
}
