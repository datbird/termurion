//! OSC 9;4 progress bars (`TDEL:1607-1660`, `VSP:12-40`, `PBV:147-535`): the state machine, where
//! the bar shows (inline in a pane or in the tab's cell), its colors and its animation clock.
//! Pure, so the rules are tested here; `app::progress_ui` draws them.

use std::time::Instant;

/// What a pane's program last reported (`VT100ScreenProgress`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Progress {
    #[default]
    Stopped,
    Error,
    Indeterminate,
    Success(u8),
    ErrorAt(u8),
    Warning(u8),
}

impl Progress {
    /// The percentage a state carries (`VT100ScreenProgressPercentage`).
    pub fn percentage(self) -> Option<u8> {
        match self {
            Progress::Success(n) | Progress::ErrorAt(n) | Progress::Warning(n) => Some(n),
            Progress::Stopped | Progress::Error | Progress::Indeterminate => None,
        }
    }

    /// Whether a bar shows for it (`VT100ScreenProgressIsVisible`).
    pub fn visible(self) -> bool {
        self != Progress::Stopped
    }

    /// 0 stopped, 1 error, 2 indeterminate, 3 a percentage: the kinds of `PBV Mode`.
    fn kind(self) -> u8 {
        match self {
            Progress::Stopped => 0,
            Progress::Error => 1,
            Progress::Indeterminate => 2,
            _ => 3,
        }
    }
}

/// A paused bar with no percentage to keep shows this much (`kMinimumVisibleProgressPercentage`,
/// `TDEL:30`): the bar's width comes from the percentage, so 0 would be invisible.
pub const MIN_VISIBLE: u8 = 10;

/// `NSString isNumeric`: one or more ASCII digits.
fn numeric(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

/// `NSString intValue`: leading spaces, a sign, then leading digits; anything else reads 0.
/// Saturates, so a long run of digits never wraps.
fn int_value(s: &str) -> i64 {
    let s = s.trim_start();
    let (neg, digits) = match s.as_bytes().first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    let mut v: i64 = 0;
    for b in digits.bytes().take_while(u8::is_ascii_digit) {
        v = (v * 10 + i64::from(b - b'0')).min(i64::from(i32::MAX));
    }
    if neg { -v } else { v }
}

/// Reads what follows `9;4;` (`None` for a bare `9;4`) against the current state
/// (`TDEL:1607-1660`). `None` back leaves the state unchanged. `st` must be numeric; a third
/// field is present when it exists at all (an empty one reads 0); a percentage outside 0..=100
/// is ignored, except that a bad one on an error makes a plain error.
pub fn parse(body: Option<&str>, current: Progress) -> Option<Progress> {
    // A bare 9;4 stops (`TDEL:1655-1657`).
    let Some(body) = body else { return Some(Progress::Stopped) };
    let mut fields = body.splitn(3, ';');
    let st = fields.next().filter(|f| numeric(f))?;
    let pr = fields.next().map(int_value);
    let in_range = pr.and_then(|n| u8::try_from(n).ok()).filter(|n| *n <= 100);
    // `st` is compared as the int iTerm2 reads; a number past int range matches no case.
    match int_value(st) {
        0 => Some(Progress::Stopped),
        1 => in_range.map(Progress::Success),
        2 => Some(in_range.map_or(Progress::Error, Progress::ErrorAt)),
        3 => Some(Progress::Indeterminate),
        4 => match (in_range, pr) {
            (Some(n), _) => Some(Progress::Warning(n)),
            // Absent: keep the shown percentage when above 0, else show the minimum.
            (None, None) => Some(Progress::Warning(current.percentage().filter(|n| *n > 0).unwrap_or(MIN_VISIBLE))),
            // Present and out of range: ignored.
            (None, Some(_)) => None,
        },
        _ => None,
    }
}

/// The tab's progress: the active pane's when visible, else the first other visible one
/// (`TAB:7827-7846`).
pub fn tab_progress(active: Progress, others: &[Progress]) -> Progress {
    if active.visible() {
        return active;
    }
    others.iter().copied().find(|p| p.visible()).unwrap_or(Progress::Stopped)
}

/// Whether the pane draws its own bar (`PT:9168-9192`, `SV:2120-2184`): enabled and visible,
/// and the tab bar is not showing it (one tab, split panes, or no tab bar).
pub fn inline_shown(enabled: bool, p: Progress, tabs: usize, panes_in_tab: usize, tab_bar_visible: bool) -> bool {
    enabled && p.visible() && (tabs == 1 || panes_in_tab > 1 || !tab_bar_visible)
}

/// Whether the tab's cell draws a bar (`TBC:305-356`): more than one tab, the active pane's
/// Enable Progress Bars on, and a visible progress.
pub fn tab_shown(tabs: usize, active_enabled: bool, p: Progress) -> bool {
    tabs > 1 && active_enabled && p.visible()
}

/// A color as sRGB floats with alpha.
pub type Color = [f32; 4];

fn c(r: f32, g: f32, b: f32) -> Color {
    [r, g, b, 1.0]
}

/// One scheme's gradient (`PBV colorSchemeColors`, `PBV:147-291`); an unknown name is "default".
pub fn scheme_colors(scheme: &str, dark: bool, indeterminate: bool) -> Vec<Color> {
    match scheme {
        "rainbow" => vec![c(1.0, 0.0, 0.0), c(1.0, 0.5, 0.0), c(1.0, 1.0, 0.0), c(0.0, 1.0, 0.0), c(0.0, 0.5, 1.0), c(0.5, 0.0, 1.0), c(1.0, 0.0, 0.5)],
        "red" => vec![c(0.8, 0.0, 0.0), c(1.0, 0.2, 0.2)],
        "green" => vec![c(0.0, 0.8, 0.0), c(0.2, 1.0, 0.2)],
        "blue" => vec![c(0.0, 0.0, 0.8), c(0.2, 0.2, 1.0)],
        "yellow" => vec![c(0.8, 0.8, 0.0), c(1.0, 1.0, 0.2)],
        "purple" => vec![c(0.6, 0.0, 0.8), c(0.8, 0.2, 1.0)],
        "cyan" => vec![c(0.0, 0.8, 0.8), c(0.2, 1.0, 1.0)],
        "orange" => vec![c(1.0, 0.5, 0.0), c(1.0, 0.7, 0.2)],
        // The default indeterminate gradient fades to nothing at its ends over the bar's
        // background (`PBV:178-182`).
        _ => match (indeterminate, dark) {
            (true, true) => [0.0, 0.5, 1.0, 1.0, 0.5, 0.0].iter().map(|a| [0.0, 1.0, 0.0, *a]).collect(),
            (true, false) => [0.0, 0.3, 1.0, 0.3, 0.0].iter().map(|a| [0.0, 0.0, 1.0, *a]).collect(),
            (false, true) => vec![c(0.0, 1.0, 0.0), c(0.2, 1.0, 0.2)],
            (false, false) => vec![c(0.0, 0.0, 1.0), c(0.2, 0.2, 1.0)],
        },
    }
}

/// A determinate bar's gradient (`PBV determinateColors`): an error's red, a pause's amber,
/// else the scheme.
pub fn determinate_colors(p: Progress, scheme: &str, dark: bool) -> Vec<Color> {
    match p {
        Progress::ErrorAt(_) => vec![c(1.0, 0.0, 0.0), c(1.0, 0.2, 0.2)],
        Progress::Warning(_) if dark => vec![c(1.0, 0.5, 0.0), c(1.0, 0.7, 0.2)],
        Progress::Warning(_) => vec![c(0.8, 0.6, 0.0), c(1.0, 0.8, 0.2)],
        _ => scheme_colors(scheme, dark, false),
    }
}

/// The pulsing error bar's two ends (`PBV errorColors`): base, light.
pub fn error_colors(dark: bool) -> (Vec<Color>, Vec<Color>) {
    if dark {
        (vec![c(1.0, 0.5, 0.5), c(1.0, 0.3, 0.3), c(1.0, 0.5, 0.5)], vec![c(1.0, 0.9, 0.9), c(1.0, 0.8, 0.8), c(1.0, 0.9, 0.9)])
    } else {
        (vec![c(0.8, 0.2, 0.2), c(1.0, 0.0, 0.0), c(1.0, 0.2, 0.2)], vec![c(0.9, 0.6, 0.6), c(1.0, 0.5, 0.5), c(1.0, 0.6, 0.6)])
    }
}

/// The bar's height in points, from 1 up to the top margin (`PSV:365-371`).
pub fn bar_height(points: f64, top_margin: f64) -> f64 {
    points.min(top_margin).max(1.0)
}

/// A typed height cut to the top margin, and whether the margin cut it (the message shows).
pub fn clamp_typed(typed: f64, margin: f64) -> (f64, bool) {
    (bar_height(typed, margin), typed > margin)
}

/// Shown beside the height field when a typed value was cut to the top margin
/// (`ProfilesSession.ProgressBarHeightTooTall`).
pub const HEIGHT_TOO_TALL: &str = "The progress bar height cannot exceed the top margin\u{2019}s height. You can adjust it in **Settings > Appearance > Panes > Top & Bottom Margins**.";

/// A determinate width eases over this long (`PBV:520-527`).
const EASE_SECS: f32 = 0.25;
/// The error bar's pulse runs base to light and back over this long (0.8 s each way, autoreverse).
const PULSE_SECS: f32 = 1.6;

/// One bar's animation clock. The app updates it with a width of 1.0 (a fraction of the bar),
/// so a resize never restarts an ease; the tests give real widths.
#[derive(Clone, Debug, Default)]
pub struct Anim {
    /// The state the clock was last given.
    pub shown: Progress,
    /// When the shown error or indeterminate mode began (the animations' zero).
    pub since: Option<Instant>,
    pub width_from: f32,
    pub width_to: f32,
    /// When the width began moving from `width_from` to `width_to`; None at rest.
    pub width_at: Option<Instant>,
}

impl Anim {
    /// Follows a state and the bar's full width. A change between two percentages eases the
    /// width; any other change jumps (`PBV:359-411`, `:509-535`).
    pub fn update(&mut self, p: Progress, width: f32, now: Instant) {
        let target = p.percentage().map_or(0.0, |n| width * f32::from(n) / 100.0);
        if p == self.shown {
            // A resized bar jumps to its new width.
            if p.percentage().is_some() && target != self.width_to {
                (self.width_from, self.width_to, self.width_at) = (target, target, None);
            }
            return;
        }
        if p.kind() != self.shown.kind() {
            self.since = matches!(p.kind(), 1 | 2).then_some(now);
        }
        if self.shown.percentage().is_some() && p.percentage().is_some() {
            self.width_from = self.width(now);
            self.width_to = target;
            self.width_at = Some(now);
        } else {
            (self.width_from, self.width_to, self.width_at) = (target, target, None);
        }
        self.shown = p;
    }

    /// The determinate bar's drawn width now.
    pub fn width(&self, now: Instant) -> f32 {
        match self.width_at {
            Some(t) => {
                let k = (now.saturating_duration_since(t).as_secs_f32() / EASE_SECS).clamp(0.0, 1.0);
                self.width_from + (self.width_to - self.width_from) * ease_in_out(k)
            }
            None => self.width_to,
        }
    }

    /// The two gradient centers of an indeterminate bar `width` wide: each runs from -w/2 to
    /// 3w/2 linearly over 3 s (dark) or 6 s (light), the second half a period ahead
    /// (`PBV:459-497`).
    pub fn indeterminate_offsets(&self, width: f32, dark: bool, now: Instant) -> [f32; 2] {
        let period = if dark { 3.0 } else { 6.0 };
        let t = self.since.map_or(0.0, |s| now.saturating_duration_since(s).as_secs_f32()) / period;
        let at = |phase: f32| -width / 2.0 + 2.0 * width * phase.fract();
        [at(t), at(t + 0.5)]
    }

    /// How far an error bar is from its base colors toward its light ones, 0 to 1 to 0 over
    /// 1.6 s, each half eased (`PBV:417-437`).
    pub fn pulse(&self, now: Instant) -> f32 {
        let t = self.since.map_or(0.0, |s| now.saturating_duration_since(s).as_secs_f32()) % PULSE_SECS;
        let half = PULSE_SECS / 2.0;
        ease_in_out(if t < half { t / half } else { (PULSE_SECS - t) / half })
    }

    /// Whether frames must keep coming: always for an error or indeterminate bar, for a
    /// percentage only while its width eases.
    pub fn animating(&self, now: Instant) -> bool {
        match self.shown {
            Progress::Stopped => false,
            Progress::Error | Progress::Indeterminate => true,
            _ => self.width_at.is_some_and(|t| now.saturating_duration_since(t).as_secs_f32() < EASE_SECS),
        }
    }
}

/// CoreAnimation's ease-in-ease-out: the cubic Bezier (0.42, 0) (0.58, 1), solved for `t` on
/// x by bisection.
pub fn ease_in_out(t: f32) -> f32 {
    if t <= 0.0 {
        return 0.0;
    }
    if t >= 1.0 {
        return 1.0;
    }
    let bezier = |s: f32, p1: f32, p2: f32| {
        let u = 1.0 - s;
        3.0 * u * u * s * p1 + 3.0 * u * s * s * p2 + s * s * s
    };
    let (mut lo, mut hi) = (0.0f32, 1.0f32);
    for _ in 0..32 {
        let mid = (lo + hi) / 2.0;
        if bezier(mid, 0.42, 0.58) < t {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    bezier((lo + hi) / 2.0, 0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn near(a: f32, b: f32, tol: f32) -> bool {
        (a - b).abs() <= tol
    }

    #[test]
    fn progress_parse_follows_iterm2() {
        use Progress::*;
        let p = |body: Option<&str>| parse(body, Stopped);
        // A bare 9;4 stops (TDEL:1655-1657); a present but non-numeric state is ignored.
        assert_eq!(p(None), Some(Stopped));
        assert_eq!(p(Some("")), None);
        assert_eq!(p(Some("0")), Some(Stopped));
        assert_eq!(p(Some("1;50")), Some(Success(50)));
        assert_eq!(p(Some("1;0")), Some(Success(0)));
        assert_eq!(p(Some("1;")), Some(Success(0)));
        assert_eq!(p(Some("1")), None);
        assert_eq!(p(Some("1;101")), None);
        assert_eq!(p(Some("1;-1")), None);
        assert_eq!(p(Some("2")), Some(Error));
        assert_eq!(p(Some("2;30")), Some(ErrorAt(30)));
        assert_eq!(p(Some("2;150")), Some(Error));
        assert_eq!(p(Some("3")), Some(Indeterminate));
        assert_eq!(p(Some("3;40")), Some(Indeterminate));
        assert_eq!(p(Some("4;20")), Some(Warning(20)));
        assert_eq!(p(Some("4")), Some(Warning(10)));
        assert_eq!(p(Some("4;300")), None);
        assert_eq!(p(Some("5")), None);
        assert_eq!(p(Some("x")), None);
        assert_eq!(p(Some("1;7x")), Some(Success(7)));
        // Paused keeps the shown percentage when above 0, else 10.
        assert_eq!(parse(Some("4"), Success(40)), Some(Warning(40)));
        assert_eq!(parse(Some("4"), Warning(0)), Some(Warning(10)));
        assert_eq!(parse(Some("4"), ErrorAt(25)), Some(Warning(25)));
        assert_eq!(parse(Some("4"), Error), Some(Warning(10)));
        // Values are bounded: huge numbers and long tails never panic or wrap.
        assert_eq!(p(Some("1;99999999999999999999")), None);
        assert_eq!(p(Some("99999999999999999999")), None);
        assert_eq!(p(Some("1;5;a;b;c;d")), Some(Success(5)));
        assert!(Error.visible());
        assert!(!Stopped.visible());
        assert!(Success(0).visible());
        assert!(Indeterminate.visible());
        assert_eq!(ErrorAt(30).percentage(), Some(30));
        assert_eq!(Warning(5).percentage(), Some(5));
        assert_eq!(Error.percentage(), None);
        assert_eq!(MIN_VISIBLE, 10);
    }

    #[test]
    fn progress_placement_follows_iterm2() {
        use Progress::*;
        assert!(inline_shown(true, Success(5), 1, 1, false));
        assert!(!inline_shown(true, Success(5), 3, 1, true));
        assert!(inline_shown(true, Success(5), 3, 2, true));
        assert!(inline_shown(true, Success(5), 3, 1, false));
        assert!(!inline_shown(false, Success(5), 1, 1, false));
        assert!(!inline_shown(true, Stopped, 1, 1, false));
        assert!(!tab_shown(1, true, Success(5)));
        assert!(tab_shown(2, true, Success(5)));
        assert!(!tab_shown(2, false, Success(5)));
        assert!(!tab_shown(2, true, Stopped));
        assert_eq!(tab_progress(Stopped, &[Stopped, Error, Success(3)]), Error);
        assert_eq!(tab_progress(Success(9), &[Error]), Success(9));
        assert_eq!(tab_progress(Stopped, &[]), Stopped);
    }

    #[test]
    fn progress_colors_follow_iterm2() {
        use Progress::*;
        assert_eq!(scheme_colors("default", true, false), vec![[0.0, 1.0, 0.0, 1.0], [0.2, 1.0, 0.2, 1.0]]);
        assert_eq!(scheme_colors("default", false, false), vec![[0.0, 0.0, 1.0, 1.0], [0.2, 0.2, 1.0, 1.0]]);
        let g = |a: f32| [0.0, 1.0, 0.0, a];
        assert_eq!(scheme_colors("default", true, true), vec![g(0.0), g(0.5), g(1.0), g(1.0), g(0.5), g(0.0)]);
        let b = |a: f32| [0.0, 0.0, 1.0, a];
        assert_eq!(scheme_colors("default", false, true), vec![b(0.0), b(0.3), b(1.0), b(0.3), b(0.0)]);
        assert_eq!(
            scheme_colors("rainbow", true, false),
            vec![[1.0, 0.0, 0.0, 1.0], [1.0, 0.5, 0.0, 1.0], [1.0, 1.0, 0.0, 1.0], [0.0, 1.0, 0.0, 1.0], [0.0, 0.5, 1.0, 1.0], [0.5, 0.0, 1.0, 1.0], [1.0, 0.0, 0.5, 1.0]]
        );
        assert_eq!(scheme_colors("rainbow", false, true), scheme_colors("rainbow", true, false));
        let two = |a: [f32; 3], b: [f32; 3]| vec![[a[0], a[1], a[2], 1.0], [b[0], b[1], b[2], 1.0]];
        assert_eq!(scheme_colors("red", true, false), two([0.8, 0.0, 0.0], [1.0, 0.2, 0.2]));
        assert_eq!(scheme_colors("green", true, false), two([0.0, 0.8, 0.0], [0.2, 1.0, 0.2]));
        assert_eq!(scheme_colors("blue", false, false), two([0.0, 0.0, 0.8], [0.2, 0.2, 1.0]));
        assert_eq!(scheme_colors("yellow", true, false), two([0.8, 0.8, 0.0], [1.0, 1.0, 0.2]));
        assert_eq!(scheme_colors("purple", true, false), two([0.6, 0.0, 0.8], [0.8, 0.2, 1.0]));
        assert_eq!(scheme_colors("cyan", false, false), two([0.0, 0.8, 0.8], [0.2, 1.0, 1.0]));
        assert_eq!(scheme_colors("orange", true, false), two([1.0, 0.5, 0.0], [1.0, 0.7, 0.2]));
        assert_eq!(scheme_colors("nonsense", true, false), scheme_colors("default", true, false));
        assert_eq!(scheme_colors("", false, true), scheme_colors("default", false, true));
        assert_eq!(determinate_colors(ErrorAt(5), "rainbow", true), vec![[1.0, 0.0, 0.0, 1.0], [1.0, 0.2, 0.2, 1.0]]);
        assert_eq!(determinate_colors(Warning(5), "rainbow", true), vec![[1.0, 0.5, 0.0, 1.0], [1.0, 0.7, 0.2, 1.0]]);
        assert_eq!(determinate_colors(Warning(5), "rainbow", false), vec![[0.8, 0.6, 0.0, 1.0], [1.0, 0.8, 0.2, 1.0]]);
        assert_eq!(determinate_colors(Success(5), "red", true), scheme_colors("red", true, false));
        let (base, light) = error_colors(true);
        assert_eq!(base, vec![[1.0, 0.5, 0.5, 1.0], [1.0, 0.3, 0.3, 1.0], [1.0, 0.5, 0.5, 1.0]]);
        assert_eq!(light, vec![[1.0, 0.9, 0.9, 1.0], [1.0, 0.8, 0.8, 1.0], [1.0, 0.9, 0.9, 1.0]]);
        let (base, light) = error_colors(false);
        assert_eq!(base, vec![[0.8, 0.2, 0.2, 1.0], [1.0, 0.0, 0.0, 1.0], [1.0, 0.2, 0.2, 1.0]]);
        assert_eq!(light, vec![[0.9, 0.6, 0.6, 1.0], [1.0, 0.5, 0.5, 1.0], [1.0, 0.6, 0.6, 1.0]]);
        assert_eq!(bar_height(5.0, 2.0), 2.0);
        assert_eq!(bar_height(0.5, 2.0), 1.0);
        assert_eq!(bar_height(1.5, 4.0), 1.5);
        assert_eq!(bar_height(3.0, 0.0), 1.0);
        assert_eq!(clamp_typed(5.0, 2.0), (2.0, true));
        assert_eq!(clamp_typed(2.0, 2.0), (2.0, false));
        assert_eq!(clamp_typed(0.0, 2.0), (1.0, false));
        assert_eq!(HEIGHT_TOO_TALL, "The progress bar height cannot exceed the top margin\u{2019}s height. You can adjust it in **Settings > Appearance > Panes > Top & Bottom Margins**.");
    }

    #[test]
    fn progress_animation_follows_iterm2() {
        use Progress::*;
        assert_eq!(ease_in_out(0.0), 0.0);
        assert_eq!(ease_in_out(1.0), 1.0);
        assert!(near(ease_in_out(0.5), 0.5, 1e-3));
        assert!(near(ease_in_out(0.25), 0.129, 1e-2));
        assert!(ease_in_out(0.1) < 0.1 && ease_in_out(0.9) > 0.9);
        let t0 = Instant::now();
        // Entering determinate from Stopped jumps.
        let mut a = Anim::default();
        a.update(Success(20), 200.0, t0);
        assert_eq!(a.width(t0), 40.0);
        assert!(!a.animating(t0));
        // Success(20) to Success(60) eases over 0.25 s.
        a.update(Success(60), 200.0, t0);
        assert_eq!(a.width(t0), 40.0);
        assert!(near(a.width(t0 + Duration::from_millis(125)), 80.0, 1.0));
        assert!(near(a.width(t0 + Duration::from_millis(250)), 120.0, 1e-3));
        assert_eq!(a.width(t0 + Duration::from_secs(5)), 120.0);
        assert!(a.animating(t0 + Duration::from_millis(100)));
        assert!(!a.animating(t0 + Duration::from_millis(250)));
        // The same state at the same width changes nothing; a new width jumps.
        a.update(Success(60), 200.0, t0 + Duration::from_secs(1));
        assert!(!a.animating(t0 + Duration::from_secs(1)));
        a.update(Success(60), 100.0, t0 + Duration::from_secs(2));
        assert_eq!(a.width(t0 + Duration::from_secs(2)), 60.0);
        // A restart mid-ease continues from where the bar is drawn.
        let mut b = Anim::default();
        b.update(Success(0), 100.0, t0);
        b.update(Success(100), 100.0, t0);
        b.update(Success(0), 100.0, t0 + Duration::from_millis(125));
        assert!(near(b.width(t0 + Duration::from_millis(125)), 50.0, 1.0));
        // Indeterminate: the center runs from -w/2 to 3w/2 over 3 s (dark) or 6 s (light).
        let mut i = Anim::default();
        i.update(Indeterminate, 100.0, t0);
        let at = |s: f32| t0 + Duration::from_secs_f32(s);
        let o = i.indeterminate_offsets(100.0, true, at(1.5));
        assert!(near(o[0], 50.0, 0.01), "{o:?}");
        assert!(near(o[1], -50.0 + 200.0 * 0.0, 0.01), "{o:?}");
        let o = i.indeterminate_offsets(100.0, true, at(0.0));
        assert!(near(o[0], -50.0, 0.01) && near(o[1], 50.0, 0.01), "{o:?}");
        let o = i.indeterminate_offsets(100.0, false, at(3.0));
        assert!(near(o[0], 50.0, 0.01), "{o:?}");
        // Error pulses 0 to 1 to 0 over 1.6 s.
        let mut e = Anim::default();
        e.update(Error, 100.0, t0);
        assert_eq!(e.pulse(t0), 0.0);
        assert!(near(e.pulse(at(0.8)), 1.0, 1e-3));
        assert!(near(e.pulse(at(1.6)), 0.0, 1e-3));
        assert!(near(e.pulse(at(2.4)), 1.0, 1e-3));
        // What keeps frames coming.
        assert!(i.animating(at(100.0)));
        assert!(e.animating(at(100.0)));
        assert!(!Anim::default().animating(t0));
        let mut s = Anim::default();
        s.update(Success(5), 10.0, t0);
        s.update(Stopped, 10.0, t0);
        assert!(!s.animating(t0));
        assert_eq!(s.width(t0), 0.0);
    }

    /// The stream drives the screen's state through the real parser; the Progress request and
    /// the event follow shown changes only.
    #[test]
    fn the_stream_drives_the_screen_state() {
        use crate::screen::{Request, Screen};
        let mut s = Screen::new(40, 4);
        let progress = |s: &mut Screen| s.take_requests().into_iter().filter_map(|r| if let Request::Progress(b) = r { Some(b) } else { None }).collect::<Vec<_>>();
        s.feed(b"\x1b]9;4;1;40\x07");
        assert_eq!(s.progress, Progress::Success(40));
        assert_eq!(progress(&mut s), vec![true]);
        // A percentage change is not a shown change.
        s.feed(b"\x1b]9;4;1;60\x07");
        assert_eq!(s.progress, Progress::Success(60));
        assert!(progress(&mut s).is_empty());
        // A bad one is ignored; paused keeps the percentage.
        s.feed(b"\x1b]9;4;1;500\x07\x1b]9;4;4\x07");
        assert_eq!(s.progress, Progress::Warning(60));
        assert!(progress(&mut s).is_empty());
        // A bare 9;4 stops (R3).
        s.feed(b"\x1b]9;4\x07");
        assert_eq!(s.progress, Progress::Stopped);
        assert_eq!(progress(&mut s), vec![false]);
        s.feed(b"\x1b]9;4;3\x07\x1b]9;4;\x07");
        assert_eq!(s.progress, Progress::Indeterminate);
    }
}
