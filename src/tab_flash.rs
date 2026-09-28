//! The tab bar's flash in full screen (milestone 13d): with Show tab bar in fullscreen off
//! and Flash tab bar on, a tab switch shows the hidden bar at once, holds it, then fades it
//! out; a held Win key keeps it up. Pure code; the app feeds it times and draws the bar at
//! `alpha`.
//!
//! Citations: `TBV` = `sources/TerminalView/iTermTabBarControlView.m`, `PT` =
//! `sources/TerminalView/PseudoTerminal.m` under `~/gitrepos/iterm2-ref`.

use std::time::{Duration, Instant};

/// Where a flash is (`TBV iTermTabBarFlashState`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Off,
    /// Shown at full alpha until the hold ends.
    Holding,
    /// The hold ended with Win held: shown until Win comes up.
    Extending,
    /// Fading to nothing.
    FadingOut,
}

/// One window's flash. `since` is when the current state's clock started: the hold's start
/// while Holding, the fade's start while FadingOut.
#[derive(Clone, Debug)]
pub struct Flash {
    state: State,
    since: Instant,
    hold: Duration,
    fade: Duration,
    win_held: bool,
}

impl Default for Flash {
    fn default() -> Flash {
        Flash::new()
    }
}

impl Flash {
    pub fn new() -> Flash {
        Flash { state: State::Off, since: Instant::now(), hold: Duration::ZERO, fade: Duration::ZERO, win_held: false }
    }

    /// `TBV setFlashing:`: `on` counts only when `eligible`
    /// (`iTermTabBarShouldFlashAutomatically`). On: from Off or FadingOut the bar shows at
    /// once and holds for `hold`; from Holding the hold starts again; Extending stays. Off:
    /// Holding and Extending fade out over the fade; FadingOut stops at once.
    pub fn set_flashing(&mut self, on: bool, eligible: bool, now: Instant, hold: Duration, fade: Duration) {
        self.hold = hold;
        self.fade = fade;
        if on && eligible {
            match self.state {
                // `fadeIn`, then a fade scheduled after the hold.
                State::Off | State::FadingOut | State::Holding => self.enter(State::Holding, now),
                State::Extending => {}
            }
        } else {
            match self.state {
                State::Off => {}
                State::Holding | State::Extending => self.enter(State::FadingOut, now),
                // `stopFlashInstantly`.
                State::FadingOut => self.enter(State::Off, now),
            }
        }
    }

    fn enter(&mut self, state: State, since: Instant) {
        self.state = state;
        self.since = since;
    }

    /// `TBV setCmdPressed:`: Win coming up while Extending fades the bar out.
    pub fn set_win_held(&mut self, held: bool, now: Instant) {
        if held == self.win_held {
            return;
        }
        self.win_held = held;
        if self.state == State::Extending && !held {
            self.enter(State::FadingOut, now);
        }
    }

    /// Moves the flash along to `now`: a hold that ended becomes Extending with Win held,
    /// else a fade from the hold's end; a fade that ended is Off.
    pub fn tick(&mut self, now: Instant) {
        if self.state == State::Holding && now >= self.since + self.hold {
            let end = self.since + self.hold;
            self.enter(if self.win_held { State::Extending } else { State::FadingOut }, end);
        }
        if self.state == State::FadingOut && now >= self.since + self.fade {
            let end = self.since + self.fade;
            self.enter(State::Off, end);
        }
    }

    /// The bar's opacity: 1 while shown, falling to 0 over the fade, 1 while Off (the bar
    /// then draws as it normally does, `TBV stopFlashInstantly`).
    pub fn alpha(&self, now: Instant) -> f32 {
        match self.state {
            State::FadingOut if !self.fade.is_zero() => {
                let gone = now.saturating_duration_since(self.since).as_secs_f32() / self.fade.as_secs_f32();
                (1.0 - gone).clamp(0.0, 1.0)
            }
            State::FadingOut => 0.0,
            State::Off | State::Holding | State::Extending => 1.0,
        }
    }

    /// A flash is under way (`TBV flashing`).
    pub fn flashing(&self) -> bool {
        self.state != State::Off
    }

    pub fn state(&self) -> State {
        self.state
    }

    /// When the flash next changes state by itself: the hold's end while Holding, the fade's
    /// end while FadingOut; None while Off or Extending (Win decides then).
    pub fn next_change(&self) -> Option<Instant> {
        match self.state {
            State::Holding => Some(self.since + self.hold),
            State::FadingOut => Some(self.since + self.fade),
            State::Off | State::Extending => None,
        }
    }
}

/// A flash may start (`PT iTermTabBarShouldFlashAutomatically`, `PT:14770-14780`): the window
/// is full screen, Show tab bar in fullscreen is off, Flash tab bar is on and Find Cursor is
/// not showing.
pub fn eligible(full_screen: bool, show_in_full_screen: bool, flash_on: bool, finding_cursor: bool) -> bool {
    full_screen && !show_in_full_screen && flash_on && !finding_cursor
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOLD: Duration = Duration::from_millis(1000);
    const FADE: Duration = Duration::from_millis(250);

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    #[test]
    fn flash_follows_iterm2() {
        let t0 = Instant::now();
        // A switch shows the bar at once and holds it.
        let mut f = Flash::new();
        assert_eq!(f.state(), State::Off);
        assert!(!f.flashing());
        f.set_flashing(true, true, t0, HOLD, FADE);
        assert_eq!(f.state(), State::Holding);
        assert!(f.flashing());
        assert!(close(f.alpha(t0), 1.0));
        assert_eq!(f.next_change(), Some(t0 + HOLD));
        f.tick(t0 + ms(500));
        assert_eq!(f.state(), State::Holding);
        assert!(close(f.alpha(t0 + ms(500)), 1.0));
        // Past the hold it fades from the hold's end.
        f.tick(t0 + ms(1125));
        assert_eq!(f.state(), State::FadingOut);
        assert!(close(f.alpha(t0 + ms(1125)), 0.5), "{}", f.alpha(t0 + ms(1125)));
        assert_eq!(f.next_change(), Some(t0 + HOLD + FADE));
        f.tick(t0 + ms(1250));
        assert_eq!(f.state(), State::Off);
        assert!(!f.flashing());
        assert_eq!(f.next_change(), None);

        // A second switch while holding starts the hold again.
        let mut f = Flash::new();
        f.set_flashing(true, true, t0, HOLD, FADE);
        f.set_flashing(true, true, t0 + ms(500), HOLD, FADE);
        f.tick(t0 + ms(1400));
        assert_eq!(f.state(), State::Holding);
        assert!(close(f.alpha(t0 + ms(1400)), 1.0));
        assert_eq!(f.next_change(), Some(t0 + ms(1500)));

        // A switch while fading out shows the bar at once again.
        let mut f = Flash::new();
        f.set_flashing(true, true, t0, HOLD, FADE);
        f.tick(t0 + ms(1100));
        assert_eq!(f.state(), State::FadingOut);
        f.set_flashing(true, true, t0 + ms(1100), HOLD, FADE);
        assert_eq!(f.state(), State::Holding);
        assert!(close(f.alpha(t0 + ms(1100)), 1.0));
        assert_eq!(f.next_change(), Some(t0 + ms(2100)));

        // Win held when the hold ends keeps the bar up; Win coming up fades it from then.
        let mut f = Flash::new();
        f.set_flashing(true, true, t0, HOLD, FADE);
        f.set_win_held(true, t0 + ms(200));
        f.tick(t0 + ms(3000));
        assert_eq!(f.state(), State::Extending);
        assert!(close(f.alpha(t0 + ms(3000)), 1.0));
        assert_eq!(f.next_change(), None);
        // A switch while extending leaves it extending.
        f.set_flashing(true, true, t0 + ms(3100), HOLD, FADE);
        assert_eq!(f.state(), State::Extending);
        f.set_win_held(false, t0 + ms(4000));
        assert_eq!(f.state(), State::FadingOut);
        assert!(close(f.alpha(t0 + ms(4125)), 0.5));
        f.tick(t0 + ms(4250));
        assert_eq!(f.state(), State::Off);

        // Win released before the hold ends: the fade comes as usual.
        let mut f = Flash::new();
        f.set_flashing(true, true, t0, HOLD, FADE);
        f.set_win_held(true, t0 + ms(100));
        f.set_win_held(false, t0 + ms(200));
        assert_eq!(f.state(), State::Holding);
        f.tick(t0 + ms(1000));
        assert_eq!(f.state(), State::FadingOut);

        // Off from Holding fades out; off from FadingOut stops at once.
        let mut f = Flash::new();
        f.set_flashing(true, true, t0, HOLD, FADE);
        f.set_flashing(false, true, t0 + ms(300), HOLD, FADE);
        assert_eq!(f.state(), State::FadingOut);
        assert!(close(f.alpha(t0 + ms(425)), 0.5));
        assert_eq!(f.next_change(), Some(t0 + ms(550)));
        f.set_flashing(false, true, t0 + ms(400), HOLD, FADE);
        assert_eq!(f.state(), State::Off);
        assert!(!f.flashing());
        assert!(close(f.alpha(t0 + ms(400)), 1.0));
        assert_eq!(f.next_change(), None);

        // Not eligible: nothing starts.
        let mut f = Flash::new();
        f.set_flashing(true, false, t0, HOLD, FADE);
        assert_eq!(f.state(), State::Off);
        assert!(!f.flashing());
        assert_eq!(f.next_change(), None);
        // iTerm2 masks `on` with eligibility, so an ineligible "on" is an "off"
        // (`TBV:229`): a hold under way fades out.
        f.set_flashing(true, true, t0, HOLD, FADE);
        f.set_flashing(true, false, t0 + ms(100), HOLD, FADE);
        assert_eq!(f.state(), State::FadingOut);

        // A zero fade ends at the hold's end.
        let mut f = Flash::new();
        f.set_flashing(true, true, t0, HOLD, Duration::ZERO);
        f.tick(t0 + HOLD);
        assert_eq!(f.state(), State::Off);
    }

    #[test]
    fn eligible_follows_iterm2() {
        assert!(eligible(true, false, true, false));
        assert!(!eligible(true, true, true, false));
        assert!(!eligible(false, false, true, false));
        assert!(!eligible(true, false, false, false));
        // Find Cursor showing.
        assert!(!eligible(true, false, true, true));
    }
}
