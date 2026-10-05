//! Encodes mouse events into the bytes a terminal app expects.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Left,
    Middle,
    Right,
    WheelUp,
    WheelDown,
    WheelLeft,
    WheelRight,
    /// Motion with no button held.
    None,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Press,
    Release,
    Motion,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mods {
    pub shift: bool,
    pub alt: bool,
    pub ctrl: bool,
    /// The Win key, standing in for Cmd.
    pub cmd: bool,
}

/// Which DEC mouse tracking mode the app turned on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tracking {
    #[default]
    Off,
    /// DEC 1000: presses and releases.
    Click,
    /// DEC 1002: also motion while a button is held.
    Drag,
    /// DEC 1003: all motion.
    Any,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modes {
    pub tracking: Tracking,
    /// DEC 1006: SGR encoding.
    pub sgr: bool,
}

/// A mouse event at a 0-based cell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Event {
    pub button: Button,
    pub action: Action,
    pub mods: Mods,
    pub col: u32,
    pub row: u32,
}

/// True when mouse events go to the app. Alt always takes the mouse back for
/// selection, like Option in iTerm2.
pub fn app_owns_mouse(modes: Modes, mods: Mods) -> bool {
    modes.tracking != Tracking::Off && !mods.alt
}

/// What kind of event is asking to be reported (`PTYMouseHandler.m:1432-1515`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Click,
    Drag,
    /// Motion with no button (DEC 1003).
    Hover,
    Wheel { horizontal: bool },
}

/// The profile and Tandem state that gate reports besides the program's own request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Gates {
    /// The profile's "Mouse Reporting allow clicks and drags".
    pub clicks: bool,
    /// The profile's "Mouse Reporting allow mouse wheel".
    pub wheel: bool,
    /// ReportHorizontalScrollEvents.
    pub horizontal: bool,
    /// The activating click holds and ReportOnFirstMouse is off.
    pub first_mouse_blocks: bool,
    /// The pointer is over one of Tandem's own overlays inside the pane (B5).
    pub ui_over: bool,
}

/// Whether an event is reported to the program (`shouldReportMouseEvent:at:`): tracking on, Alt
/// not held, then the gate of its kind. The wheel scrolls under an overlay and passes the
/// first-mouse hold; clicks, drags and hover do neither; hover is not a click or drag.
pub fn should_report(modes: Modes, mods: Mods, kind: Kind, g: Gates) -> bool {
    if !app_owns_mouse(modes, mods) {
        return false;
    }
    match kind {
        Kind::Wheel { horizontal } => g.wheel && (g.horizontal || !horizontal),
        Kind::Click | Kind::Drag => g.clicks && !g.first_mouse_blocks && !g.ui_over,
        Kind::Hover => !g.first_mouse_blocks && !g.ui_over,
    }
}

/// What a press does when it may be the click that activated the window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FirstClick {
    /// A normal click.
    Normal,
    /// The click only brought the window forward and does nothing else.
    ActivateOnly,
    /// Focus follows mouse: the click only focuses the pane.
    FocusOnly,
}

/// `mouseDownImpl:` (`PTYMouseHandler.m:165-203`): the kind of the click, and whether it still
/// counts as first mouse for reporting. Focus follows mouse comes first (an unfocused pane
/// without the Win key only takes focus); then an activating click without the Win key does
/// nothing unless AlwaysAcceptFirstMouse.
pub fn first_click(first_mouse: bool, cmd: bool, always_accept: bool, ffm: bool, pane_has_focus: bool) -> (FirstClick, bool) {
    if !pane_has_focus && !cmd && ffm {
        return (FirstClick::FocusOnly, true);
    }
    if first_mouse && !cmd && !always_accept {
        return (FirstClick::ActivateOnly, true);
    }
    (FirstClick::Normal, first_mouse)
}

/// `xtermMouseReporting` (`PTYSession.m:7020-7028`): allowed, and outside the alternate screen
/// not restricted to it.
pub fn reporting_on(ever_allowed: bool, alt_screen: bool, restrict: bool) -> bool {
    ever_allowed && (alt_screen || !restrict)
}

/// Tandem's own overlays that sit inside a pane's rectangle, by kind (B5).
#[derive(Clone, Debug, Default)]
pub struct UiRects {
    pub info_bars: Vec<crate::layout::Rect>,
    pub portholes: Vec<crate::layout::Rect>,
    pub pills: Vec<crate::layout::Rect>,
    pub pickers: Vec<crate::layout::Rect>,
    pub indicators: Vec<crate::layout::Rect>,
    pub find: Vec<crate::layout::Rect>,
}

impl UiRects {
    pub fn hit(&self, x: f32, y: f32) -> bool {
        [&self.info_bars, &self.portholes, &self.pills, &self.pickers, &self.indicators, &self.find].into_iter().flatten().any(|r| r.contains(x, y))
    }
}

/// Returns the bytes to send the app for `ev`, or None when the app did not ask for it.
pub fn encode(modes: Modes, ev: Event) -> Option<Vec<u8>> {
    let wheel = matches!(ev.button, Button::WheelUp | Button::WheelDown | Button::WheelLeft | Button::WheelRight);
    let reported = match (modes.tracking, ev.action) {
        (Tracking::Off, _) => false,
        (_, Action::Press) => ev.button != Button::None,
        (_, Action::Release) => !wheel && ev.button != Button::None,
        (Tracking::Click, Action::Motion) => false,
        (Tracking::Drag, Action::Motion) => !wheel && ev.button != Button::None,
        (Tracking::Any, Action::Motion) => !wheel,
    };
    if !reported {
        return None;
    }

    let button: u32 = match ev.button {
        Button::Left => 0,
        Button::Middle => 1,
        Button::Right => 2,
        Button::None => 3,
        Button::WheelUp => 64,
        Button::WheelDown => 65,
        Button::WheelLeft => 66,
        Button::WheelRight => 67,
    };
    let mods = 4 * ev.mods.shift as u32 + 8 * ev.mods.alt as u32 + 16 * ev.mods.ctrl as u32;
    let motion = if ev.action == Action::Motion { 32 } else { 0 };
    let (x, y) = (ev.col + 1, ev.row + 1);

    if modes.sgr {
        let end = if ev.action == Action::Release { 'm' } else { 'M' };
        return Some(format!("\x1b[<{};{x};{y}{end}", button + mods + motion).into_bytes());
    }

    // Legacy encoding: each value is one byte offset by 32, and a release does not
    // say which button. ConPTY replaces input bytes above 127 with U+FFFD, so 95 is
    // the largest position that arrives intact (measured in milestone 0).
    if x > 95 || y > 95 {
        return None;
    }
    let cb = if ev.action == Action::Release { 3 + mods } else { button + mods + motion };
    Some(vec![0x1b, b'[', b'M', (32 + cb) as u8, (32 + x) as u8, (32 + y) as u8])
}

/// Maps a pixel position to a cell, clamped to the grid so drags outside the
/// window still land on an edge cell.
pub fn cell_at(x: f64, y: f64, cell_w: f64, cell_h: f64, cols: u32, rows: u32) -> (u32, u32) {
    let col = (x / cell_w).floor().clamp(0.0, cols.saturating_sub(1) as f64) as u32;
    let row = (y / cell_h).floor().clamp(0.0, rows.saturating_sub(1) as f64) as u32;
    (col, row)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(button: Button, action: Action, col: u32, row: u32) -> Event {
        Event { button, action, mods: Mods::default(), col, row }
    }

    const SGR_CLICK: Modes = Modes { tracking: Tracking::Click, sgr: true };
    const SGR_DRAG: Modes = Modes { tracking: Tracking::Drag, sgr: true };
    const SGR_ANY: Modes = Modes { tracking: Tracking::Any, sgr: true };
    const LEGACY_CLICK: Modes = Modes { tracking: Tracking::Click, sgr: false };

    #[test]
    fn horizontal_wheel_reports_66_and_67() {
        let modes = Modes { tracking: Tracking::Click, sgr: true };
        let ev = |button| Event { button, action: Action::Press, mods: Mods::default(), col: 0, row: 0 };
        assert_eq!(encode(modes, ev(Button::WheelLeft)).unwrap(), b"\x1b[<66;1;1M");
        assert_eq!(encode(modes, ev(Button::WheelRight)).unwrap(), b"\x1b[<67;1;1M");
        assert_eq!(encode(modes, Event { action: Action::Release, ..ev(Button::WheelLeft) }), None);
    }

    #[test]
    fn off_sends_nothing() {
        assert_eq!(encode(Modes::default(), ev(Button::Left, Action::Press, 0, 0)), None);
    }

    #[test]
    fn sgr_press_and_release() {
        assert_eq!(encode(SGR_CLICK, ev(Button::Left, Action::Press, 0, 0)).unwrap(), b"\x1b[<0;1;1M");
        assert_eq!(encode(SGR_CLICK, ev(Button::Left, Action::Release, 0, 0)).unwrap(), b"\x1b[<0;1;1m");
        assert_eq!(encode(SGR_CLICK, ev(Button::Right, Action::Press, 9, 4)).unwrap(), b"\x1b[<2;10;5M");
    }

    #[test]
    fn legacy_press_and_release() {
        assert_eq!(encode(LEGACY_CLICK, ev(Button::Left, Action::Press, 4, 9)).unwrap(), vec![0x1b, b'[', b'M', 32, 37, 42]);
        assert_eq!(encode(LEGACY_CLICK, ev(Button::Left, Action::Release, 4, 9)).unwrap(), vec![0x1b, b'[', b'M', 35, 37, 42]);
    }

    #[test]
    fn click_mode_ignores_motion() {
        assert_eq!(encode(SGR_CLICK, ev(Button::Left, Action::Motion, 1, 1)), None);
    }

    #[test]
    fn drag_mode_reports_drags_only() {
        assert_eq!(encode(SGR_DRAG, ev(Button::Left, Action::Motion, 1, 1)).unwrap(), b"\x1b[<32;2;2M");
        assert_eq!(encode(SGR_DRAG, ev(Button::None, Action::Motion, 1, 1)), None);
    }

    #[test]
    fn any_mode_reports_plain_motion() {
        assert_eq!(encode(SGR_ANY, ev(Button::None, Action::Motion, 1, 1)).unwrap(), b"\x1b[<35;2;2M");
    }

    #[test]
    fn wheel_has_no_release() {
        assert_eq!(encode(SGR_CLICK, ev(Button::WheelUp, Action::Press, 0, 0)).unwrap(), b"\x1b[<64;1;1M");
        assert_eq!(encode(SGR_CLICK, ev(Button::WheelDown, Action::Press, 0, 0)).unwrap(), b"\x1b[<65;1;1M");
        assert_eq!(encode(SGR_CLICK, ev(Button::WheelUp, Action::Release, 0, 0)), None);
    }

    #[test]
    fn modifiers_add_bits() {
        let e = Event { mods: Mods { shift: true, alt: false, ctrl: true, cmd: false }, ..ev(Button::Left, Action::Press, 0, 0) };
        assert_eq!(encode(SGR_CLICK, e).unwrap(), b"\x1b[<20;1;1M");
    }

    #[test]
    fn legacy_release_keeps_modifiers() {
        let e = Event { mods: Mods { shift: true, alt: false, ctrl: false, cmd: false }, ..ev(Button::Left, Action::Release, 0, 0) };
        assert_eq!(encode(LEGACY_CLICK, e).unwrap(), vec![0x1b, b'[', b'M', 32 + 3 + 4, 33, 33]);
    }

    #[test]
    fn legacy_past_column_95_is_dropped() {
        // ConPTY turns any input byte above 127 into U+FFFD, so 95 + 32 = 127 is the last safe position.
        assert_eq!(encode(LEGACY_CLICK, ev(Button::Left, Action::Press, 95, 0)), None);
        assert_eq!(encode(LEGACY_CLICK, ev(Button::Left, Action::Press, 0, 95)), None);
        assert!(encode(LEGACY_CLICK, ev(Button::Left, Action::Press, 94, 94)).is_some());
    }

    #[test]
    fn sgr_handles_wide_screens() {
        assert_eq!(encode(SGR_CLICK, ev(Button::Left, Action::Press, 299, 0)).unwrap(), b"\x1b[<0;300;1M");
    }

    #[test]
    fn alt_takes_the_mouse_back() {
        let none = Mods::default();
        let alt = Mods { alt: true, ..none };
        assert!(!app_owns_mouse(Modes::default(), none));
        assert!(app_owns_mouse(SGR_CLICK, none));
        assert!(!app_owns_mouse(SGR_CLICK, alt));
    }

    #[test]
    fn cell_at_maps_pixels() {
        assert_eq!(cell_at(25.0, 41.0, 10.0, 20.0, 80, 24), (2, 2));
    }

    #[test]
    fn cell_at_handles_an_empty_grid() {
        // A minimized window or a squeezed pane can have 0 columns or rows.
        assert_eq!(cell_at(5.0, 5.0, 10.0, 20.0, 0, 0), (0, 0));
    }

    #[test]
    fn cell_at_clamps_outside_the_grid() {
        assert_eq!(cell_at(-50.0, -1.0, 10.0, 20.0, 80, 24), (0, 0));
        assert_eq!(cell_at(5000.0, 9000.0, 10.0, 20.0, 80, 24), (79, 23));
    }

    const ALL: Gates = Gates { clicks: true, wheel: true, horizontal: true, first_mouse_blocks: false, ui_over: false };
    const KINDS: [Kind; 5] = [Kind::Click, Kind::Drag, Kind::Hover, Kind::Wheel { horizontal: false }, Kind::Wheel { horizontal: true }];

    #[test]
    fn reporting_gates_follow_iterm2() {
        let m = Mods::default();
        for k in KINDS {
            assert!(should_report(SGR_CLICK, m, k, ALL), "{k:?} all on");
        }
        let no_clicks = Gates { clicks: false, ..ALL };
        assert!(!should_report(SGR_CLICK, m, Kind::Click, no_clicks));
        assert!(!should_report(SGR_CLICK, m, Kind::Drag, no_clicks));
        assert!(should_report(SGR_CLICK, m, Kind::Hover, no_clicks), "mouse moved is not in the clicks-and-drags list");
        assert!(should_report(SGR_CLICK, m, Kind::Wheel { horizontal: false }, no_clicks));
        let no_wheel = Gates { wheel: false, ..ALL };
        assert!(!should_report(SGR_CLICK, m, Kind::Wheel { horizontal: false }, no_wheel));
        assert!(should_report(SGR_CLICK, m, Kind::Click, no_wheel));
        let no_h = Gates { horizontal: false, ..ALL };
        assert!(!should_report(SGR_CLICK, m, Kind::Wheel { horizontal: true }, no_h));
        assert!(should_report(SGR_CLICK, m, Kind::Wheel { horizontal: false }, no_h));
        let alt = Mods { alt: true, ..m };
        for k in KINDS {
            assert!(!should_report(SGR_CLICK, alt, k, ALL), "{k:?} with Alt");
            assert!(!should_report(Modes::default(), m, k, ALL), "{k:?} tracking off");
        }
        let first = Gates { first_mouse_blocks: true, ..ALL };
        for k in [Kind::Click, Kind::Drag, Kind::Hover] {
            assert!(!should_report(SGR_ANY, m, k, first), "{k:?} while the first-mouse click holds");
        }
        assert!(should_report(SGR_CLICK, m, Kind::Wheel { horizontal: false }, first), "the wheel is excepted");
    }

    #[test]
    fn restrict_to_alternate_screen() {
        assert!(!reporting_on(true, false, true));
        assert!(reporting_on(true, true, true));
        assert!(reporting_on(true, false, false));
        assert!(!reporting_on(false, true, false));
    }

    /// `PTYMouseHandler.m:165-203`: the kind of the click and whether it still counts as first mouse.
    #[test]
    fn first_mouse_follows_iterm2() {
        use FirstClick::*;
        // (first, cmd, always_accept, ffm, pane_has_focus)
        assert_eq!(first_click(true, false, true, false, true), (Normal, true), "accepted, still first mouse");
        assert_eq!(first_click(true, false, false, false, true), (ActivateOnly, true));
        assert_eq!(first_click(true, true, false, false, true), (Normal, true), "a Win-click acts");
        assert_eq!(first_click(false, false, true, false, true), (Normal, false));
        assert_eq!(first_click(false, false, true, true, false), (FocusOnly, true), "focus follows mouse, pane not focused");
        assert_eq!(first_click(false, true, true, true, false), (Normal, false), "Win key beats focus follows mouse");
    }

    /// R3 (`02:612`): the activating click is not reported unless ReportOnFirstMouse; the wheel never is held.
    #[test]
    fn activating_click_is_not_reported() {
        let m = Mods::default();
        let blocked = Gates { first_mouse_blocks: true, ..ALL };
        for k in [Kind::Click, Kind::Drag, Kind::Hover] {
            assert!(!should_report(SGR_ANY, m, k, blocked), "{k:?}");
        }
        // ReportOnFirstMouse on: the App passes first_mouse_blocks false.
        for k in KINDS {
            assert!(should_report(SGR_ANY, m, k, ALL), "{k:?} with ReportOnFirstMouse");
        }
        assert!(should_report(SGR_ANY, m, Kind::Wheel { horizontal: false }, blocked));
        assert!(should_report(SGR_ANY, m, Kind::Wheel { horizontal: true }, blocked));
    }

    /// B5: a program never gets a click, drag or hover meant for a Tandem overlay.
    #[test]
    fn overlay_clicks_are_not_reported() {
        let m = Mods::default();
        let over = Gates { ui_over: true, ..ALL };
        for k in [Kind::Click, Kind::Drag, Kind::Hover] {
            assert!(!should_report(SGR_ANY, m, k, over), "{k:?} over an overlay");
        }
        assert!(should_report(SGR_ANY, m, Kind::Wheel { horizontal: false }, over), "the wheel scrolls under a bar as before");
    }

    fn r(x: f32, y: f32) -> crate::layout::Rect {
        crate::layout::Rect { x, y, w: 10.0, h: 10.0 }
    }

    /// B5: one test per overlay kind, each alone in the list, hit inside and missed outside.
    #[test]
    fn each_overlay_kind_is_a_tandem_hit() {
        let kinds: [(&str, UiRects); 6] = [
            ("info bar", UiRects { info_bars: vec![r(0.0, 0.0)], ..Default::default() }),
            ("porthole", UiRects { portholes: vec![r(0.0, 0.0)], ..Default::default() }),
            ("block pill", UiRects { pills: vec![r(0.0, 0.0)], ..Default::default() }),
            ("picker", UiRects { pickers: vec![r(0.0, 0.0)], ..Default::default() }),
            ("chat indicator", UiRects { indicators: vec![r(0.0, 0.0)], ..Default::default() }),
            ("find bar", UiRects { find: vec![r(0.0, 0.0)], ..Default::default() }),
        ];
        for (name, ui) in kinds {
            assert!(ui.hit(5.0, 5.0), "{name} inside");
            assert!(!ui.hit(50.0, 5.0), "{name} outside");
        }
        assert!(!UiRects::default().hit(5.0, 5.0));
    }
}
