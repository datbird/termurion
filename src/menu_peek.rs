//! Getting the menu bar back in full screen. With "Hide menu bar in full screen" on, the bar is
//! off the window, and a person whose only full screen chord needs a key they cannot press would
//! be stuck. iTerm2 on macOS drops the menu bar down when the pointer reaches the top edge; on
//! Windows pressing and releasing Alt alone does the same, and so does the pointer at the top edge.
//! This is only the decision; the app attaches and removes the bar (`sync_menu_hidden`).

/// How the bar came back, so the right thing sends it away again.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
enum Cause {
    #[default]
    None,
    /// The pointer reached the top edge: the bar goes when the pointer is back in the window.
    Pointer,
    /// Alt was pressed and released alone: the bar stays until Alt is tapped again.
    Key,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct MenuPeek {
    cause: Cause,
    /// Alt went down and nothing else happened since: its release is a tap.
    alt_armed: bool,
}

/// How near the top edge the pointer must be, in pixels, to bring the bar back.
const EDGE: f32 = 0.5;

impl MenuPeek {
    /// The bar is brought back for now.
    pub fn shown(&self) -> bool {
        self.cause != Cause::None
    }

    /// The bar goes: a menu command ran, the window lost the focus, another window came up.
    pub fn clear(&mut self) {
        *self = MenuPeek::default();
    }

    /// A key went down or up. `is_alt` says it is the Alt key; `pressed` and `repeat` its state.
    /// Returns true when the bar's state changed (Alt was tapped).
    pub fn key(&mut self, is_alt: bool, pressed: bool, repeat: bool) -> bool {
        if !is_alt {
            // Alt+Tab, Alt+F4, an Alt chord: not a tap.
            if pressed {
                self.alt_armed = false;
            }
            return false;
        }
        if pressed {
            // Only the first press arms it: Alt held down repeats.
            if !repeat {
                self.alt_armed = true;
            }
            return false;
        }
        if !std::mem::take(&mut self.alt_armed) {
            return false;
        }
        self.cause = if self.shown() { Cause::None } else { Cause::Key };
        true
    }

    /// A mouse press while Alt is down is an Alt-click, not a tap.
    pub fn press(&mut self) {
        self.alt_armed = false;
    }

    /// The pointer moved inside the window (client coordinates in pixels). `can_peek` says the bar
    /// is hidden in full screen now and no drag is going. Returns true when the bar's state changed.
    pub fn pointer(&mut self, y: f32, can_peek: bool) -> bool {
        match self.cause {
            Cause::None if can_peek && y <= EDGE => {
                self.cause = Cause::Pointer;
                true
            }
            // A move in the client area below the bar's edge: the pointer left the bar (the bar is
            // not client area, so no move is reported while the pointer is over it).
            Cause::Pointer if y > EDGE => {
                self.cause = Cause::None;
                true
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alt_pressed_and_released_alone_shows_the_bar_and_again_hides_it() {
        let mut p = MenuPeek::default();
        assert!(!p.shown());
        assert!(!p.key(true, true, false), "the press changes nothing yet");
        assert!(p.key(true, false, false), "the release is the tap");
        assert!(p.shown());
        p.key(true, true, false);
        assert!(p.key(true, false, false));
        assert!(!p.shown(), "a second tap sends it away");
    }

    #[test]
    fn alt_with_another_key_is_not_a_tap() {
        let mut p = MenuPeek::default();
        p.key(true, true, false);
        p.key(false, true, false); // Alt+Tab, Alt+F4, Alt+Enter
        assert!(!p.key(true, false, false));
        assert!(!p.shown());
        // A click with Alt held is not a tap either.
        p.key(true, true, false);
        p.press();
        assert!(!p.key(true, false, false));
        assert!(!p.shown());
        // A release with no press seen (focus came back with Alt held) is nothing.
        assert!(!p.key(true, false, false));
    }

    #[test]
    fn a_key_repeat_of_alt_changes_nothing() {
        let mut p = MenuPeek::default();
        p.key(true, true, false);
        assert!(!p.key(true, true, true));
        assert!(p.key(true, false, false));
        assert!(p.shown());
    }

    #[test]
    fn the_top_edge_shows_the_bar_and_leaving_it_hides_it() {
        let mut p = MenuPeek::default();
        assert!(!p.pointer(30.0, true), "away from the edge");
        assert!(!p.pointer(0.0, false), "a drag or a visible bar: the edge does nothing");
        assert!(p.pointer(0.0, true));
        assert!(p.shown());
        assert!(!p.pointer(0.0, true), "already shown");
        assert!(p.pointer(12.0, false), "the pointer came down out of the bar");
        assert!(!p.shown());
    }

    #[test]
    fn a_bar_brought_by_alt_stays_until_alt_or_a_command_or_focus_loss() {
        let mut p = MenuPeek::default();
        p.key(true, true, false);
        p.key(true, false, false);
        assert!(!p.pointer(40.0, true), "moving the pointer does not hide a bar Alt brought");
        assert!(p.shown());
        p.clear();
        assert!(!p.shown());
    }
}
