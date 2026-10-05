//! The alternate-screen wheel: whether a wheel event in a full-screen program becomes arrow
//! keys, is swallowed, or falls through (`PTYMouseHandler.m:1583-1635`, `PTYTextView.m:8398-8408`).

/// The settings that can turn alternate scroll on, read fresh for each wheel event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AltSwitches {
    /// The Advanced setting AlternateMouseScroll.
    pub advanced: bool,
    /// The profile's "Automatically Enable Alternate Mouse Scroll".
    pub profile_auto: bool,
    /// The profile's "Restrict Alternate Mouse Scroll to Vertical".
    pub profile_vertical_only: bool,
    /// DECSET 1007, as the program set it (the profile's Allow gate is applied at the set).
    pub term_1007: bool,
}

/// What the wheel does in the alternate screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AltScroll {
    /// Fall through to reporting, then the history.
    No,
    /// As `No`, and the inferrer sees the event.
    NoAndInfer,
    /// Send arrow keys.
    Arrows { vertical_only: bool },
    /// Send nothing and do not scroll (a custom string is set; iTerm2 sends its empty string).
    Swallow,
}

/// Whether alternate scroll is on and whether it is restricted to vertical movement
/// (`PTYTextView.m:8398-8408`, `PTYSession.m:14945-14950`): the Advanced switch beats the
/// profile and clears the restriction.
pub fn alternate_enabled(s: AltSwitches, alt_screen: bool) -> (bool, bool) {
    if s.advanced {
        return (true, false);
    }
    ((alt_screen && s.profile_auto) || s.term_1007, s.profile_vertical_only)
}

/// `scrollWheelShouldSendDataForEvent:at:` after its coordinate and wheel checks, including
/// iTerm2's inverted vertical test (a vertical event with alternate scroll off falls through;
/// its log says "Horizontal", the condition tests vertical).
pub fn alternate(alt_screen: bool, writable: bool, reporting: bool, alt_held: bool, vertical: bool, s: AltSwitches, strings_set: bool) -> AltScroll {
    if !alt_screen || !writable || reporting || alt_held {
        return AltScroll::No;
    }
    let (enabled, vertical_only) = alternate_enabled(s, alt_screen);
    if vertical && !enabled {
        return AltScroll::No;
    }
    if !vertical && vertical_only {
        return AltScroll::No;
    }
    if enabled {
        return AltScroll::Arrows { vertical_only };
    }
    if strings_set {
        return AltScroll::Swallow;
    }
    AltScroll::NoAndInfer
}

/// How many arrows a scroll delta sends (`PTYMouseHandler.m:1166-1182`).
pub fn arrow_count(delta: f64) -> usize {
    delta.abs().ceil() as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    const S0: AltSwitches = AltSwitches { advanced: false, profile_auto: false, profile_vertical_only: false, term_1007: false };

    #[test]
    fn alternate_scroll_follows_iterm2() {
        // alternate(alt_screen, writable, reporting, alt_held, vertical, switches, strings_set)
        let on = AltSwitches { term_1007: true, ..S0 };
        assert_eq!(alternate(false, true, false, false, true, on, false), AltScroll::No, "main screen");
        assert_eq!(alternate(true, false, false, false, true, on, false), AltScroll::No, "not writable");
        assert_eq!(alternate(true, true, true, false, true, on, false), AltScroll::No, "reporting wins");
        assert_eq!(alternate(true, true, false, true, true, on, false), AltScroll::No, "Alt");
        assert_eq!(alternate(true, true, false, false, true, S0, false), AltScroll::No, "nothing on");
        assert_eq!(alternate(true, true, false, false, true, on, false), AltScroll::Arrows { vertical_only: false });
        let vo = AltSwitches { term_1007: true, profile_vertical_only: true, ..S0 };
        assert_eq!(alternate(true, true, false, false, true, vo, false), AltScroll::Arrows { vertical_only: true });
        assert_eq!(alternate(true, true, false, false, false, vo, false), AltScroll::No);
        let adv = AltSwitches { advanced: true, profile_vertical_only: true, ..S0 };
        assert_eq!(alternate(true, true, false, false, false, adv, false), AltScroll::Arrows { vertical_only: false });
        let auto = AltSwitches { profile_auto: true, ..S0 };
        assert_eq!(alternate(true, true, false, false, true, auto, false), AltScroll::Arrows { vertical_only: false });
        assert_eq!(alternate(true, true, false, false, false, S0, false), AltScroll::NoAndInfer);
        assert_eq!(alternate_enabled(AltSwitches { profile_auto: true, profile_vertical_only: true, ..S0 }, true), (true, true));
        assert_eq!(alternate_enabled(AltSwitches { profile_auto: true, profile_vertical_only: true, ..S0 }, false), (false, true));
        assert_eq!(arrow_count(0.0), 0);
        assert_eq!(arrow_count(0.2), 1);
        assert_eq!(arrow_count(-2.5), 3);
    }

    #[test]
    fn wheel_in_the_alternate_screen_scrolls_by_default() {
        // R3: with nothing asking for arrows, a vertical wheel in the alternate screen falls
        // through to the history (route_wheel's scroll branch).
        assert_eq!(alternate(true, true, false, false, true, S0, false), AltScroll::No);
    }

    #[test]
    fn custom_strings_follow_iterm2() {
        // 02:605, 02:606: the strings never reach the program; a horizontal wheel is swallowed.
        assert_eq!(alternate(true, true, false, false, true, S0, true), AltScroll::No);
        assert_eq!(alternate(true, true, false, false, false, S0, true), AltScroll::Swallow);
        let on = AltSwitches { term_1007: true, ..S0 };
        assert_eq!(alternate(true, true, false, false, true, on, true), AltScroll::Arrows { vertical_only: false });
        assert_eq!(alternate(true, true, false, false, false, on, true), AltScroll::Arrows { vertical_only: false });
    }
}
