//! The dimming model (iTerm2's `SessionView` dimming amount, `PseudoTerminal`'s inactive-split
//! predicate and `iTermColorMap`'s dimmed colors): which panes dim, by how much, and the color
//! math for Dim Only Text. Pure.

use crate::broadcast::{Broadcast, PaneId, View};
use crate::palette::Rgb;
use crate::tab_theme::toward_gray;

/// How a pane is dimmed: the amount (0 to 0.9) and whether only text and colored cells dim.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Dim {
    pub amount: f32,
    pub only_text: bool,
}

impl Dim {
    /// Not dimmed.
    pub const NONE: Dim = Dim { amount: 0.0, only_text: false };
    /// The fixed fade of the paths that pass no highlights (previews, screenshots).
    pub const LEGACY: Dim = Dim { amount: 0.4, only_text: false };
}

/// The middle gray every dimmed color averages toward (`iTermColorMap.m:299-306`).
pub const GRAY: f32 = 0.5;

/// The default `perceived` brightness difference under which a background is "the default"
/// for Dim Only Text (`iTermColorMap.m:529-560`).
const DEFAULT_BG_EPSILON: f32 = 0.01;

/// Whether a session dims as an inactive split (`PT:12402-12422`): Dim inactive split panes is
/// on, it is not its tab's active session, and input is not broadcast to it and to the current
/// session.
pub fn inactive_split(dim_inactive: bool, is_active: bool, receives: bool, current_receives: bool) -> bool {
    dim_inactive && !is_active && !(receives && current_receives)
}

/// Whether every session of the window dims as a background window (`SV:1497-1508`).
pub fn background(dim_background_windows: bool, window_focused: bool) -> bool {
    dim_background_windows && !window_focused
}

/// The dimming amount (`SV:1449-1469`): 0, `base` or 1.5 times `base` for none, one or both
/// reasons, clamped to 0 to 0.9.
pub fn amount(inactive_split: bool, background: bool, base: f32) -> f32 {
    let scale = match (inactive_split, background) {
        (false, false) => return 0.0,
        (true, true) => 1.5,
        _ => 1.0,
    };
    (base * scale).clamp(0.0, 0.9)
}

/// The two broadcast inputs of `inactive_split`, both with the tab's focused pane as the sender
/// (`broadcastInputToSession:fromSessionWithGUID:currentSession.guid`): whether `id` receives
/// from it, and whether the focused pane receives from itself.
pub fn broadcast_inputs(b: &Broadcast, v: &View, id: PaneId, focused: PaneId) -> (bool, bool) {
    (b.should_broadcast_to(v, id, Some(focused)), b.should_broadcast_to(v, focused, Some(focused)))
}

/// A text color under `d` (`iTermColorMap.m:343-360`): with Dim Only Text, averaged toward the
/// default background's brightness; otherwise untouched (the gray quad dims it).
pub fn text_color(c: Rgb, d: Dim, bg_brightness: f32) -> Rgb {
    if d.only_text && d.amount > 0.0 { toward_gray(c, d.amount, bg_brightness) } else { c }
}

/// A cell background under `d` (`iTermColorMap.m:529-560`): with Dim Only Text, a color more
/// than 0.01 from `default_bg` in any channel is averaged toward the default background's
/// brightness and the default background itself stays; otherwise untouched.
pub fn bg_color(c: Rgb, default_bg: Rgb, d: Dim, bg_brightness: f32) -> Rgb {
    if !d.only_text || d.amount <= 0.0 {
        return c;
    }
    let far = |a: u8, b: u8| (f32::from(a) - f32::from(b)).abs() / 255.0 > DEFAULT_BG_EPSILON;
    if far(c.r, default_bg.r) || far(c.g, default_bg.g) || far(c.b, default_bg.b) {
        toward_gray(c, d.amount, bg_brightness)
    } else {
        c
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::broadcast::{Broadcast, Mode, TabView, View};
    use crate::palette::rgb;
    use crate::tab_theme::toward_gray;
    use std::collections::BTreeSet;

    fn near(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-6
    }

    #[test]
    fn dim_amounts_follow_iterm2() {
        assert!(near(amount(true, false, 0.4), 0.4));
        assert!(near(amount(true, true, 0.4), 0.6));
        assert!(near(amount(false, true, 0.4), 0.4));
        assert_eq!(amount(false, false, 0.4), 0.0);
        assert!(near(amount(true, true, 0.6), 0.9), "capped");
        assert!(near(amount(true, false, 1.5), 0.9));
        assert!(inactive_split(true, false, false, false));
        assert!(!inactive_split(true, true, false, false));
        assert!(!inactive_split(true, true, true, true));
        assert!(!inactive_split(false, false, false, false));
        assert!(!inactive_split(true, false, true, true), "broadcasting to it and to the current session");
        assert!(inactive_split(true, false, true, false));
        assert!(inactive_split(true, false, false, true));
        assert!(!background(false, false));
        assert!(background(true, false));
        assert!(!background(true, true));
        assert_eq!(Dim::NONE.amount, 0.0);
        assert_eq!(Dim::LEGACY.amount, 0.4);
        assert!(!Dim::LEGACY.only_text);
    }

    #[test]
    fn dim_colors_follow_iterm2() {
        assert_eq!(toward_gray(rgb(255, 255, 255), 0.4, GRAY), rgb(204, 204, 204));
        assert_eq!(toward_gray(rgb(0, 0, 0), 0.4, GRAY), rgb(51, 51, 51));
        let d = Dim { amount: 0.4, only_text: false };
        assert_eq!(text_color(rgb(255, 255, 255), d, 0.0), rgb(255, 255, 255), "the gray quad does the averaging");
        assert_eq!(bg_color(rgb(255, 0, 0), rgb(0, 0, 0), d, 0.0), rgb(255, 0, 0));
    }

    #[test]
    fn only_text_follows_iterm2() {
        let d = Dim { amount: 0.4, only_text: true };
        assert_eq!(text_color(rgb(255, 255, 255), d, 0.0), rgb(153, 153, 153));
        assert_eq!(bg_color(rgb(255, 0, 0), rgb(0, 0, 0), d, 0.0), rgb(153, 0, 0));
        assert_eq!(bg_color(rgb(1, 1, 1), rgb(0, 0, 0), d, 0.0), rgb(1, 1, 1), "within 0.01 of the default is the default");
        assert_eq!(bg_color(rgb(0, 0, 0), rgb(0, 0, 0), d, 0.0), rgb(0, 0, 0));
        let none = Dim { amount: 0.0, only_text: true };
        assert_eq!(text_color(rgb(255, 255, 255), none, 0.0), rgb(255, 255, 255));
        assert_eq!(bg_color(rgb(255, 0, 0), rgb(0, 0, 0), none, 0.0), rgb(255, 0, 0));
    }

    #[test]
    fn broadcast_inputs_use_the_focused_sender() {
        // Pane 2 receives only from pane 3 (the source), not from the focused pane 1: it dims.
        let v = View { tabs: vec![TabView { broadcasting: false, panes: vec![1, 2, 3] }], current_tab: 0, current: 1, exited: BTreeSet::new() };
        let b = Broadcast { raw: Mode::Custom, domains: vec![BTreeSet::from([1, 2, 3])], sources: BTreeSet::from([3]) };
        assert!(b.receives(&v, 2), "from any sender it does");
        let (receives, current) = broadcast_inputs(&b, &v, 2, 1);
        assert!(!receives && !current);
        assert!(inactive_split(true, false, receives, current));
        // Without sources, pane 2 and the focused pane both receive: pane 2 stays lit.
        let b = Broadcast { raw: Mode::Custom, domains: vec![BTreeSet::from([1, 2, 3])], sources: BTreeSet::new() };
        let (receives, current) = broadcast_inputs(&b, &v, 2, 1);
        assert!(receives && current);
        assert!(!inactive_split(true, false, receives, current));
    }
}
