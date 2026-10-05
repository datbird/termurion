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

/// The delta mouse reporting uses (`PTYTextView.m:8446-8471`): under fast trackpad the lines
/// divided by the unit (the cell height or width in points), floored when negative, else
/// ceiled, so a notch of three lines reports one step; otherwise the lines as they are.
pub fn report_delta(lines: f64, unit_pt: f64, fast_trackpad: bool) -> f64 {
    if !fast_trackpad || unit_pt <= 0.0 {
        return lines;
    }
    let frac = lines / unit_pt;
    if frac < 0.0 { frac.floor() } else { frac.ceil() }
}

/// How many reports one wheel event sends (`PTYSession.m:13820-13838`): proportional gives the
/// truncated size capped at 32, else one; a single step becomes two when the tmux workaround
/// is on.
pub fn report_steps(chosen: f64, proportional: bool, double: bool) -> usize {
    let steps = if proportional { chosen.abs().min(32.0) as usize } else { 1 };
    if steps == 1 && double { 2 } else { steps }
}

/// One wheel event in lines (`dx` positive moves the content right).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Event {
    pub dx: f64,
    pub dy: f64,
    /// Windows' fractional-notch touchpad delta (no gesture phases): the trackpad path.
    pub precise: bool,
}

/// A winit delta as an `Event`: a pixel delta over the cell size, or notches times
/// `wheel_lines`; a notch count that is not whole is a touchpad's, so precise.
pub fn event_from(delta_x: f32, delta_y: f32, pixel: Option<(f64, f64)>, wheel_lines: f64, cell_pt: (f64, f64)) -> Event {
    if let Some((px, py)) = pixel {
        return Event { dx: px / cell_pt.0, dy: py / cell_pt.1, precise: true };
    }
    let fractional = |d: f32| (d * 120.0).round() as i64 % 120 != 0;
    Event { dx: delta_x as f64 * wheel_lines, dy: delta_y as f64 * wheel_lines, precise: fractional(delta_x) || fractional(delta_y) }
}

/// The Advanced settings the accumulators read (`ScrollWheelAcceleration`, `FastTrackpad`,
/// `UseModernScrollWheelAccumulator`, `SensitiveScrollWheel`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Prefs {
    pub acceleration: f64,
    pub fast_trackpad: bool,
    pub modern: bool,
    pub sensitive: bool,
}

impl Prefs {
    pub fn from_globals(g: &crate::globals::Globals) -> Prefs {
        Prefs {
            acceleration: g.number("ScrollWheelAcceleration").unwrap_or(1.0),
            fast_trackpad: g.bool("FastTrackpad"),
            modern: g.bool("UseModernScrollWheelAccumulator"),
            sensitive: g.bool("SensitiveScrollWheel"),
        }
    }
}

fn round_away(x: f64) -> f64 {
    if x < 0.0 { x.floor() } else { x.ceil() }
}

/// `iTermScrollAccumulator`: one remainder, one sensitivity.
#[derive(Clone, Debug, PartialEq)]
pub struct Accumulator {
    acc: f64,
    pub sensitivity: f64,
}

impl Accumulator {
    pub fn new(sensitivity: f64) -> Accumulator {
        Accumulator { acc: 0.0, sensitivity }
    }

    /// `takeWholePortionWithDelta:` (`iTermScrollAccumulator.m:111-148`).
    fn take_whole(&mut self, delta: f64) -> f64 {
        if self.acc.abs() >= 1.0 {
            let whole = self.acc.trunc();
            self.acc -= whole;
            whole
        } else if delta * self.acc < 0.0 {
            self.acc = 0.0;
            delta.round()
        } else {
            0.0
        }
    }

    /// The modern accumulator, `deltaForEvent:increment:` (`:48-66`, `:69-99`). `raw` is lines.
    pub fn delta(&mut self, raw: f64, precise: bool, p: &Prefs) -> f64 {
        let adjusted = if precise && p.fast_trackpad { round_away(raw) } else { raw };
        let got = if precise {
            let d = adjusted * self.sensitivity;
            self.acc += d;
            self.take_whole(d)
        } else if self.sensitivity == 1.0 {
            let r = adjusted.round();
            if r == 0.0 && adjusted != 0.0 { adjusted.signum() } else { r }
        } else {
            self.acc += adjusted * self.sensitivity;
            self.take_whole(adjusted)
        };
        let sign = if got > 0.0 { 1.0 } else { -1.0 };
        got.abs().powf(p.acceleration.max(0.0)) * sign
    }

    /// The legacy accumulator (`legacyDeltaForEvent:increment:`, `:158-181`). `raw` is what
    /// the event delivers: points when `precise`, lines otherwise.
    pub fn legacy_delta(&mut self, raw: f64, precise: bool, increment: f64, p: &Prefs) -> f64 {
        let mut d = raw * self.sensitivity;
        if precise {
            d /= increment;
        }
        if p.sensitive {
            return if d > 0.0 { d.ceil() } else if d < 0.0 { d.floor() } else { 0.0 };
        }
        self.acc += d;
        if self.acc.abs() >= 1.0 {
            let whole = self.acc.trunc();
            self.acc -= whole;
            whole
        } else {
            0.0
        }
    }

    /// Modern or legacy by the preference (`PTYScrollView.m:230-237`, `PTYTextView.m:8318-8328`).
    /// `raw` is lines; the legacy path wants points for a precise event, so it is scaled up
    /// by `increment` (the cell size) first and comes back down in `legacy_delta`.
    pub fn take(&mut self, raw: f64, precise: bool, increment: f64, p: &Prefs) -> f64 {
        if p.modern {
            self.delta(raw, precise, p)
        } else {
            self.legacy_delta(if precise { raw * increment } else { raw }, precise, increment, p)
        }
    }

    pub fn reset(&mut self) {
        self.acc = 0.0;
    }
}

/// The three accumulators a session holds (`PTYTextView.m:349-353`): the scroller's (history),
/// the text view's (reports and arrows), and the horizontal one.
#[derive(Clone, Debug, PartialEq)]
pub struct PaneWheel {
    pub scroll: Accumulator,
    pub report: Accumulator,
    pub horizontal: Accumulator,
}

impl PaneWheel {
    pub fn new(horizontal_sensitivity: f64) -> PaneWheel {
        PaneWheel { scroll: Accumulator::new(1.0), report: Accumulator::new(1.0), horizontal: Accumulator::new(horizontal_sensitivity) }
    }
}

/// Whether a horizontal report goes as "scroll left" (`PTYMouseHandler.m:1553-1570`): a
/// touchpad's delta is swapped back when natural scrolling is on, unless the Advanced setting
/// says otherwise. A mouse wheel is never swapped.
pub fn natural_swap(dx_positive: bool, precise: bool, natural_on: bool, affects: bool) -> bool {
    if precise && natural_on && !affects { !dx_positive } else { dx_positive }
}

/// The precision touchpad's "Down motion scrolls up" (natural) setting. UNVERIFIED mapping:
/// `ScrollDirection` 0 is natural, 0xFFFFFFFF the classic direction; absent is false.
#[cfg(windows)]
pub fn touchpad_natural() -> bool {
    use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
    let key: Vec<u16> = "Software\\Microsoft\\Windows\\CurrentVersion\\PrecisionTouchPad\0".encode_utf16().collect();
    let name: Vec<u16> = "ScrollDirection\0".encode_utf16().collect();
    let mut data: u32 = 0;
    let mut len: u32 = 4;
    // SAFETY: both strings are NUL-terminated; `data` and `len` outlive the call.
    let rc = unsafe { RegGetValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr(), RRF_RT_REG_DWORD, std::ptr::null_mut(), &mut data as *mut u32 as *mut _, &mut len) };
    rc == 0 && data == 0
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

    const P0: Prefs = Prefs { acceleration: 1.0, fast_trackpad: true, modern: true, sensitive: false };

    #[test]
    fn modern_accumulator_follows_iterm2() {
        let mut a = Accumulator::new(1.0);
        assert_eq!(a.delta(3.0, false, &P0), 3.0);
        assert_eq!(a.delta(0.3, false, &P0), 1.0, "a wheel moves at least one");
        assert_eq!(a.delta(-0.3, false, &P0), -1.0);
        assert_eq!(a.delta(2.5, false, &P0), 3.0, "half rounds away from zero");
        assert_eq!(a.delta(0.2, true, &P0), 1.0, "fast trackpad rounds away from zero");
        let slow = Prefs { fast_trackpad: false, ..P0 };
        let mut a = Accumulator::new(1.0);
        assert_eq!(a.delta(0.4, true, &slow), 0.0);
        assert_eq!(a.delta(0.4, true, &slow), 0.0);
        assert_eq!(a.delta(0.4, true, &slow), 1.0, "the whole part toward zero");
        assert!((a.acc - 0.2).abs() < 1e-9, "leaves 0.2, got {}", a.acc);
        // Turnaround (P11): 0.8 held, then -0.6 reverses at once and clears.
        let mut a = Accumulator::new(1.0);
        assert_eq!(a.delta(0.4, true, &slow), 0.0);
        assert_eq!(a.delta(0.4, true, &slow), 0.0);
        assert_eq!(a.delta(-0.6, true, &slow), -1.0);
        assert_eq!(a.acc, 0.0);
        // Sensitivity under 1: a wheel accumulates 0.3 a notch.
        let mut a = Accumulator::new(0.1);
        assert_eq!(a.delta(3.0, false, &P0), 0.0);
        assert_eq!(a.delta(3.0, false, &P0), 0.0);
        assert_eq!(a.delta(3.0, false, &P0), 0.0);
        assert_eq!(a.delta(3.0, false, &P0), 1.0);
    }

    #[test]
    fn legacy_accumulator_follows_iterm2() {
        let legacy = Prefs { modern: false, ..P0 };
        let mut a = Accumulator::new(1.0);
        assert_eq!(a.legacy_delta(0.4, false, 17.0, &legacy), 0.0);
        assert_eq!(a.legacy_delta(0.4, false, 17.0, &legacy), 0.0);
        assert_eq!(a.legacy_delta(0.4, false, 17.0, &legacy), 1.0);
        let sens = Prefs { sensitive: true, ..legacy };
        let mut a = Accumulator::new(1.0);
        assert_eq!(a.legacy_delta(0.1, false, 17.0, &sens), 1.0);
        assert_eq!(a.legacy_delta(-0.1, false, 17.0, &sens), -1.0);
        assert_eq!(a.legacy_delta(0.0, false, 17.0, &sens), 0.0);
        let mut a = Accumulator::new(1.0);
        assert_eq!(a.legacy_delta(34.0, true, 17.0, &legacy), 2.0, "pixels over the increment");
        // take picks the path by the preference; its raw is in lines.
        let mut a = Accumulator::new(1.0);
        assert_eq!(a.take(3.0, false, 17.0, &legacy), 3.0);
        assert_eq!(a.take(2.0, true, 17.0, &legacy), 2.0, "lines in, lines out");
        assert_eq!(a.take(3.0, false, 17.0, &P0), 3.0);
    }

    #[test]
    fn acceleration_follows_iterm2() {
        let mut a = Accumulator::new(1.0);
        let p = Prefs { acceleration: 2.0, ..P0 };
        assert_eq!(a.delta(3.0, false, &p), 9.0);
        assert_eq!(a.delta(-2.0, false, &p), -4.0);
        assert_eq!(a.delta(4.0, false, &Prefs { acceleration: 0.5, ..P0 }), 2.0);
        let neg = Prefs { acceleration: -1.0, ..P0 };
        assert_eq!(a.delta(3.0, false, &neg), 1.0, "a negative power counts as 0");
        assert_eq!(a.delta(0.0, false, &neg), -1.0, "iTerm2's sign is -1 for 0 (kept)");
    }

    #[test]
    fn horizontal_follows_iterm2() {
        let w = PaneWheel::new(0.1);
        assert_eq!(w.horizontal.sensitivity, 0.1);
        assert_eq!(w.scroll.sensitivity, 1.0);
        assert_eq!(w.report.sensitivity, 1.0);
        let mut h = w.horizontal;
        assert_eq!(h.take(3.0, false, 8.0, &P0), 0.0);
        assert_eq!(h.take(3.0, false, 8.0, &P0), 0.0);
        assert_eq!(h.take(3.0, false, 8.0, &P0), 0.0);
        assert_eq!(h.take(3.0, false, 8.0, &P0), 1.0);
        let mut h = PaneWheel::new(1.0).horizontal;
        assert_eq!(h.take(3.0, false, 8.0, &P0), 3.0);
        let mut h = PaneWheel::new(0.0).horizontal;
        for _ in 0..5 {
            assert_eq!(h.take(3.0, false, 8.0, &P0), 0.0, "0 disables");
        }
    }

    #[test]
    fn natural_scrolling_follows_iterm2() {
        assert!(!natural_swap(true, true, true, false), "iTerm2 undoes the reversal");
        assert!(natural_swap(true, true, true, true));
        assert!(natural_swap(true, false, true, false), "a wheel is never swapped");
        assert!(natural_swap(true, true, false, false));
        assert!(natural_swap(false, true, true, false), "a leftward delta is swapped to scroll left");
    }

    #[test]
    fn event_from_follows_the_delta_kind() {
        assert_eq!(event_from(0.0, 1.0, None, 3.0, (8.0, 17.0)), Event { dx: 0.0, dy: 3.0, precise: false });
        assert_eq!(event_from(0.0, 0.25, None, 3.0, (8.0, 17.0)), Event { dx: 0.0, dy: 0.75, precise: true });
        assert_eq!(event_from(0.0, 0.0, Some((0.0, 34.0)), 3.0, (8.0, 17.0)), Event { dx: 0.0, dy: 2.0, precise: true });
        assert_eq!(event_from(-1.0, 0.0, None, 3.0, (8.0, 17.0)), Event { dx: -3.0, dy: 0.0, precise: false });
    }

    #[test]
    fn report_steps_follow_iterm2() {
        // report_steps(chosen, proportional, double): PTYSession.m:13820-13838.
        assert_eq!(report_steps(3.0, true, false), 3);
        assert_eq!(report_steps(40.0, true, false), 32, "capped at 32");
        assert_eq!(report_steps(-5.0, true, false), 5);
        assert_eq!(report_steps(0.0, true, false), 0);
        assert_eq!(report_steps(3.0, false, false), 1);
        assert_eq!(report_steps(1.0, true, true), 2);
        assert_eq!(report_steps(1.0, false, true), 2);
        assert_eq!(report_steps(3.0, true, true), 3, "only a single step doubles");
        assert_eq!(report_steps(0.5, true, true), 0, "truncated before the doubling check");
    }

    #[test]
    fn a_notch_reports_one_step() {
        // report_delta(lines, unit_pt, fast_trackpad): PTYTextView.m:8446-8471.
        assert_eq!(report_delta(3.0, 17.0, true), 1.0);
        assert_eq!(report_delta(-3.0, 17.0, true), -1.0);
        assert_eq!(report_delta(0.0, 17.0, true), 0.0);
        assert_eq!(report_delta(3.0, 17.0, false), 3.0);
        // A default notch (3 lines, fast trackpad on) reports once.
        assert_eq!(report_steps(report_delta(3.0, 17.0, true), true, false), 1);
    }
}
