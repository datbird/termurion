//! iTerm2's alternate-screen scroll inferrer (`iTermAltScreenMouseScrollInferrer.m`): it
//! watches the person's own wheel and arrow keys in one pane and, once, guesses that they
//! wanted the wheel to move the cursor. Only the person's input feeds it, never program output.

/// The question's bar (`iTermNaggingController.m:395-425`).
pub const ASK_ID: &str = "AskAboutAlternateMouseScroll";
pub const NEVER_KEY: &str = "NoSyncNeverAskAboutSettingAlternateMouseScroll";
pub const ASK_TEXT: &str = "Do you want the scroll wheel to move the cursor in interactive programs like this?";
pub const YES: &str = "Yes";
pub const DONT_ASK: &str = "Don\u{2019}t Ask Again";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Initial,
    ScrolledUp,
    ScrolledDown,
    Frustration,
}

/// What a state change tells the owner: ask the question (`Infer(true)`) or take it back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    None,
    Infer(bool),
}

#[derive(Debug)]
pub struct Inferrer {
    state: State,
    /// Frustration is entered once per inferrer, so a program cannot make the question return.
    have_inferred: bool,
}

impl Default for Inferrer {
    fn default() -> Inferrer {
        Inferrer { state: State::Initial, have_inferred: false }
    }
}

impl Inferrer {
    pub fn state(&self) -> State {
        self.state
    }

    fn set(&mut self, new: State) -> Change {
        if new == self.state {
            return Change::None;
        }
        let mut change = Change::None;
        if self.state == State::Frustration {
            change = Change::Infer(false);
        }
        if new == State::Frustration {
            if self.have_inferred {
                return Change::None;
            }
            self.have_inferred = true;
            change = Change::Infer(true);
        }
        self.state = new;
        change
    }

    /// A wheel event (`scrollWheel:`): up is a positive `dy`.
    pub fn scroll(&mut self, dy: f64, dx: f64) -> Change {
        if self.state == State::Frustration {
            return Change::None;
        }
        if dy > 0.0 {
            self.set(State::ScrolledUp)
        } else if dy < 0.0 {
            self.set(State::ScrolledDown)
        } else if dx != 0.0 {
            self.set(State::Initial)
        } else {
            Change::None
        }
    }

    /// A key press: `Some(true)` the up arrow, `Some(false)` the down arrow, `None` any other.
    pub fn key(&mut self, arrow: Option<bool>) -> Change {
        match self.state {
            State::Initial => Change::None,
            State::Frustration => {
                if arrow.is_some() {
                    Change::None
                } else {
                    self.set(State::Initial)
                }
            }
            State::ScrolledDown => self.set(if arrow.is_some() { State::Frustration } else { State::Initial }),
            State::ScrolledUp => self.set(if arrow == Some(true) { State::Frustration } else { State::Initial }),
        }
    }

    /// A click or drag (`nonScrollWheelEvent:`).
    pub fn non_scroll(&mut self) -> Change {
        self.set(State::Initial)
    }

    /// The pane lost the keyboard focus (`firstResponderDidChange`).
    pub fn reset(&mut self) -> Change {
        self.set(State::Initial)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inferrer_follows_iterm2() {
        let mut i = Inferrer::default();
        assert_eq!(i.state(), State::Initial);
        assert_eq!(i.scroll(1.0, 0.0), Change::None);
        assert_eq!(i.state(), State::ScrolledUp);
        assert_eq!(i.key(Some(true)), Change::Infer(true));
        assert_eq!(i.state(), State::Frustration);
        // Any scroll in Frustration changes nothing.
        assert_eq!(i.scroll(-1.0, 0.0), Change::None);
        assert_eq!(i.state(), State::Frustration);
        // Up or down arrow keeps it; another key leaves it and reports NO.
        assert_eq!(i.key(Some(false)), Change::None);
        assert_eq!(i.state(), State::Frustration);
        assert_eq!(i.key(None), Change::Infer(false));
        assert_eq!(i.state(), State::Initial);
        // Once per inferrer: a second visit stays in the prior state and reports nothing.
        i.scroll(-1.0, 0.0);
        assert_eq!(i.state(), State::ScrolledDown);
        assert_eq!(i.key(Some(false)), Change::None);
        assert_eq!(i.state(), State::ScrolledDown);
        assert_eq!(i.key(Some(true)), Change::None);
        assert_eq!(i.state(), State::ScrolledDown);
    }

    #[test]
    fn from_scrolled_down_either_arrow_reaches_frustration() {
        for arrow in [false, true] {
            let mut i = Inferrer::default();
            i.scroll(-1.0, 0.0);
            assert_eq!(i.key(Some(arrow)), Change::Infer(true));
        }
        // From ScrolledUp only the up arrow does; down or another key is Initial.
        let mut i = Inferrer::default();
        i.scroll(1.0, 0.0);
        assert_eq!(i.key(Some(false)), Change::None);
        assert_eq!(i.state(), State::Initial);
        i.scroll(1.0, 0.0);
        assert_eq!(i.key(None), Change::None);
        assert_eq!(i.state(), State::Initial);
        // Keys in Initial change nothing.
        assert_eq!(i.key(Some(true)), Change::None);
        assert_eq!(i.state(), State::Initial);
    }

    #[test]
    fn horizontal_only_and_resets() {
        let mut i = Inferrer::default();
        i.scroll(1.0, 0.0);
        assert_eq!(i.scroll(0.0, 1.0), Change::None);
        assert_eq!(i.state(), State::Initial);
        // No movement at all changes nothing.
        i.scroll(1.0, 0.0);
        i.scroll(0.0, 0.0);
        assert_eq!(i.state(), State::ScrolledUp);
        assert_eq!(i.non_scroll(), Change::None);
        assert_eq!(i.state(), State::Initial);
        // Leaving Frustration through a click or a focus change reports NO.
        for leave in [Inferrer::non_scroll, Inferrer::reset] {
            let mut i = Inferrer::default();
            i.scroll(1.0, 0.0);
            i.key(Some(true));
            assert_eq!(leave(&mut i), Change::Infer(false));
            assert_eq!(i.state(), State::Initial);
        }
    }

    #[test]
    fn inferrer_question_follows_iterm2() {
        // iTermNaggingController.m:395-425.
        assert_eq!(ASK_TEXT, "Do you want the scroll wheel to move the cursor in interactive programs like this?");
        assert_eq!([YES, DONT_ASK], ["Yes", "Don\u{2019}t Ask Again"]);
        assert_eq!(ASK_ID, "AskAboutAlternateMouseScroll");
        assert_eq!(NEVER_KEY, "NoSyncNeverAskAboutSettingAlternateMouseScroll");
    }
}
