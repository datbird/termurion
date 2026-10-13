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

/// The event a press, release or motion reports to a tracking program. It carries the PHYSICAL
/// modifiers, the same for all three: the shortcut key (TWEAKS 9) only changes what a click means
/// to Tandem, never what the program is told, so Ctrl+Shift+click reports Ctrl and Shift.
pub fn report_event(button: Button, action: Action, physical: Mods, col: u32, row: u32) -> Event {
    Event { button, action, mods: physical, col, row }
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
    /// The pane has the keyboard in the key window (iTerm2's `mouseHandlerViewHasFocus`).
    pub view_focused: bool,
}

/// Whether an event is reported to the program (`shouldReportMouseEvent:at:`): tracking on, Alt
/// not held, then the gate of its kind. The wheel passes the first-mouse hold; clicks, drags
/// and hover do not; nothing reaches the program through an overlay; hover is not a click or drag.
pub fn should_report(modes: Modes, mods: Mods, kind: Kind, g: Gates) -> bool {
    if !app_owns_mouse(modes, mods) {
        return false;
    }
    match kind {
        Kind::Wheel { horizontal } => g.wheel && (g.horizontal || !horizontal) && !g.ui_over,
        Kind::Click => g.clicks && !g.first_mouse_blocks && !g.ui_over && g.view_focused,
        Kind::Drag => g.clicks && !g.first_mouse_blocks && !g.ui_over,
        Kind::Hover => !g.first_mouse_blocks && !g.ui_over && g.view_focused,
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

/// What the pointer entering a Tandem window does under focus follows mouse.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Enter {
    Nothing,
    /// Make that Tandem window key (Tandem is active, so Windows allows it).
    MakeKey,
    /// Take the foreground for that window although Tandem is not active (`StealKeyFocus`).
    Steal,
}

/// `iTermFocusFollowsMouse.swift:107-165`: nothing when the setting is off or the pointer has
/// not moved since activation (`mouseLocationToRefuseFirstResponderAt`); an active app makes the
/// window key; an inactive one only with the steal switch.
pub fn ffm_on_enter(ffm: bool, steal: bool, app_active: bool, moved_since_refusal: bool) -> Enter {
    if !ffm || !moved_since_refusal {
        return Enter::Nothing;
    }
    if app_active {
        Enter::MakeKey
    } else if steal {
        Enter::Steal
    } else {
        Enter::Nothing
    }
}

/// `iTermFocusFollowsMouseController.m:40-94`: on becoming active, aggressive mode activates the
/// window under the pointer unless the pointer is where it was when Tandem resigned. Regular
/// activation only refuses the first responder at the current point (no focus change).
pub fn ffm_on_activate(ffm: bool, aggressive: bool, saved: Option<(f64, f64)>, now: (f64, f64)) -> bool {
    ffm && aggressive && saved != Some(now)
}

/// What is in front of the window the pointer entered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Front {
    /// A Tandem terminal window.
    Terminal,
    /// A Tandem window that is not a terminal: a dialog, Settings, a chat or approval window.
    OtherTermurion,
    /// Another application.
    Elsewhere,
}

/// Whether focus follows mouse may act on `enter` now. It never takes focus from a Tandem
/// dialog (modal prompt, 12f approval window), from a password prompt, or while an approval is
/// waiting; it takes it from another app only as the steal switch (`Enter::Steal`) says.
pub fn ffm_may_act(enter: Enter, front: Front, password_prompt: bool, approval_pending: bool, composing: bool) -> bool {
    // An IME composition holds focus where it is (13k wires the flag).
    if password_prompt || approval_pending || composing {
        return false;
    }
    match enter {
        Enter::Nothing => false,
        Enter::MakeKey => front == Front::Terminal,
        Enter::Steal => front != Front::OtherTermurion,
    }
}

/// Whether plain focus follows mouse may move the pane focus inside a window
/// (`iTermFocusFollowsMouse.swift:118-158`: key window and no modal window). Tandem also holds
/// it at a password prompt (ruling, final review 2) and during an IME composition (13k).
pub fn ffm_pane_may_focus(window_key: bool, approval_pending: bool, password_prompt: bool, composing: bool) -> bool {
    window_key && !approval_pending && !password_prompt && !composing
}

/// A split divider's grab zone: the divider grown by `grab` on every side.
pub fn divider_grab(d: crate::layout::Rect, grab: f32) -> crate::layout::Rect {
    crate::layout::Rect { x: d.x - grab, y: d.y - grab, w: d.w + 2.0 * grab, h: d.h + 2.0 * grab }
}

/// What a left press leaves running after it set its selection (`PTYMouseHandler.m:778-782`,
/// `:595-597`): the click that activated the window may place a selection but a drag never
/// extends it, nothing is copied at once, and Alt-click never moves the cursor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PressFollow {
    pub drag_selects: bool,
    pub copy_now: bool,
    pub alt_click: bool,
}

pub fn press_follow_up(first_mouse_down: bool, smart: bool, count: u8, alt_only: bool) -> PressFollow {
    if first_mouse_down {
        return PressFollow { drag_selects: false, copy_now: false, alt_click: false };
    }
    PressFollow { drag_selects: !smart, copy_now: smart, alt_click: count == 1 && alt_only }
}

/// `xtermMouseReporting` (`PTYSession.m:7020-7028`): allowed, and outside the alternate screen
/// not restricted to it.
pub fn reporting_on(ever_allowed: bool, alt_screen: bool, restrict: bool) -> bool {
    ever_allowed && (alt_screen || !restrict)
}

/// The kinds of Tandem overlay drawn inside a pane's rectangle or over the window (B5). A new
/// overlay adds its kind here and pushes its rectangle in `App::ui_rects`; a test checks both.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Overlay {
    InfoBar,
    Porthole,
    Pill,
    Picker,
    Indicator,
    Find,
    /// The completion list (Autocomplete, Auto Command Completion).
    Completion,
    /// The composer panel (minimal and Auto Composer).
    Composer,
    ComposerPopup,
    /// The toast along the bottom (a click through it would reach the program).
    Toast,
    Banner,
    /// The Move Pane pick's looks over the session views.
    PaneMove,
    /// The first-selection notice of Clicking on a Command (13s Task 9).
    CommandInfo,
}

impl Overlay {
    pub const ALL: [Overlay; 13] = [Overlay::InfoBar, Overlay::Porthole, Overlay::Pill, Overlay::Picker, Overlay::Indicator, Overlay::Find, Overlay::Completion, Overlay::Composer, Overlay::ComposerPopup, Overlay::Toast, Overlay::Banner, Overlay::PaneMove, Overlay::CommandInfo];
}

/// Tandem's own surfaces that never lie inside a pane's terminal rectangle, so `pane_at` never
/// finds a pane there, and why: `visible()` cuts the title bar and the status bar out, the
/// inline AI panel and the Clippings panel take the right of the pane, the tab bar and toolbelt
/// are outside the content area, and Tandem draws no scrollbar.
pub const OUTSIDE_THE_TERMINAL: [(&str, &str); 5] = [
    ("title bar", "visible() splits it off (pane_bar_split)"),
    ("status bar", "visible() and the content area exclude it"),
    ("inline AI panel", "takes the right of the pane's rectangle"),
    ("clip panel", "takes the right of the pane's rectangle"),
    ("scrollbar", "none is drawn"),
];

/// Every overlay's rectangle in one list (B5): a point inside any is Tandem's.
#[derive(Clone, Debug, Default)]
pub struct UiRects {
    rects: Vec<(Overlay, crate::layout::Rect)>,
}

impl UiRects {
    pub fn push(&mut self, kind: Overlay, rect: crate::layout::Rect) {
        self.rects.push((kind, rect));
    }

    pub fn hit(&self, x: f32, y: f32) -> bool {
        self.rects.iter().any(|(_, r)| r.contains(x, y))
    }
}

/// `PTYMouseHandler.m:595-607`: Alt-click moves the cursor only when the terminal's raw mouse
/// mode is off; a program that asked for clicks gets none as arrows, whatever the profile's
/// gates, the alternate-screen restriction or the reporting toggle say.
pub fn alt_click_may_move_cursor(raw: Modes) -> bool {
    raw.tracking == Tracking::Off
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

    /// TWEAKS 9 review I1: a press, release and motion report the physical modifiers, also when
    /// they are the shortcut key (Ctrl+Shift), so the pair never turns into Cmd and is dropped.
    #[test]
    fn reports_carry_the_physical_modifiers_for_press_release_and_motion() {
        let ctrl_shift = Mods { ctrl: true, shift: true, ..Mods::default() };
        for (action, end) in [(Action::Press, 'M'), (Action::Release, 'm'), (Action::Motion, 'M')] {
            let e = report_event(Button::Left, action, ctrl_shift, 0, 0);
            let motion = if action == Action::Motion { 32 } else { 0 };
            let want = format!("\x1b[<{};1;1{end}", 4 + 16 + motion);
            assert_eq!(encode(SGR_ANY, e), Some(want.into_bytes()), "{action:?}");
        }
    }

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

    const ALL: Gates = Gates { clicks: true, wheel: true, horizontal: true, first_mouse_blocks: false, ui_over: false, view_focused: true };
    const KINDS: [Kind; 5] = [Kind::Click, Kind::Drag, Kind::Hover, Kind::Wheel { horizontal: false }, Kind::Wheel { horizontal: true }];

    /// Final review 7: a click or hover reaches only a pane that has the keyboard
    /// (`PTYMouseHandler.m:1464-1494`); a drag or release continues a click already seen.
    #[test]
    fn clicks_and_hover_need_the_view_to_have_focus() {
        let m = Mods::default();
        let unfocused = Gates { view_focused: false, ..ALL };
        assert!(!should_report(SGR_ANY, m, Kind::Click, unfocused));
        assert!(!should_report(SGR_ANY, m, Kind::Hover, unfocused));
        assert!(should_report(SGR_ANY, m, Kind::Drag, unfocused));
        assert!(should_report(SGR_ANY, m, Kind::Wheel { horizontal: false }, unfocused));
    }

    /// Final review 2: plain focus follows mouse inside a window acts only in the key window,
    /// with no approval waiting, no password prompt and no IME composition.
    #[test]
    fn pane_focus_follows_the_mouse_only_in_the_key_window() {
        assert!(ffm_pane_may_focus(true, false, false, false));
        assert!(!ffm_pane_may_focus(false, false, false, false), "a window that is not key");
        assert!(!ffm_pane_may_focus(true, true, false, false), "an approval or dialog is waiting");
        assert!(!ffm_pane_may_focus(true, false, true, false), "a password prompt");
        assert!(!ffm_pane_may_focus(true, false, false, true), "an IME composition");
    }

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
        assert!(!should_report(SGR_ANY, m, Kind::Wheel { horizontal: false }, over), "the wheel over an overlay is Tandem's too (review fix 1)");
    }

    fn r(x: f32, y: f32) -> crate::layout::Rect {
        crate::layout::Rect { x, y, w: 10.0, h: 10.0 }
    }

    /// B5, review fix 1: every overlay kind, enumerated, is a hit when it is in the list; the
    /// kinds that sit outside the terminal rectangle are named with the reason.
    #[test]
    fn each_overlay_kind_is_a_termurion_hit() {
        assert!(Overlay::ALL.len() >= 12);
        for kind in Overlay::ALL {
            let mut ui = UiRects::default();
            ui.push(kind, r(0.0, 0.0));
            assert!(ui.hit(5.0, 5.0), "{kind:?} inside");
            assert!(!ui.hit(50.0, 5.0), "{kind:?} outside");
        }
        assert!(!UiRects::default().hit(5.0, 5.0));
        for want in ["InfoBar", "Porthole", "Pill", "Picker", "Indicator", "Find", "Completion", "Composer", "ComposerPopup", "Toast", "Banner", "PaneMove", "CommandInfo"] {
            assert!(Overlay::ALL.iter().any(|k| format!("{k:?}") == want), "{want} is an overlay kind");
        }
        for want in ["title bar", "status bar", "inline AI panel", "clip panel", "scrollbar"] {
            assert!(OUTSIDE_THE_TERMINAL.iter().any(|(n, _)| *n == want), "{want} is named as outside");
        }
    }

    /// Review fix 1: presses, releases, drags, hover and the wheel over an overlay never reach the program.
    #[test]
    fn overlays_hold_every_kind_of_event() {
        let over = Gates { ui_over: true, ..ALL };
        for k in KINDS {
            assert!(!should_report(SGR_ANY, Mods::default(), k, over), "{k:?} over an overlay");
        }
    }

    /// Review fix 2 (`PTYMouseHandler.m:595-607`): Alt-click moves the cursor only when the raw
    /// mouse mode is off, whatever the gates say.
    #[test]
    fn alt_click_follows_the_raw_mouse_mode() {
        assert!(alt_click_may_move_cursor(Modes::default()));
        assert!(!alt_click_may_move_cursor(SGR_CLICK));
        assert!(!alt_click_may_move_cursor(SGR_ANY));
        assert!(!alt_click_may_move_cursor(Modes { tracking: Tracking::Drag, sgr: false }));
    }
    /// Task 6 review, Important 1 and 2 (`PTYMouseHandler.m:778-782`, `:597`): the activating
    /// click may place a selection but never extends it, copies nothing, and never moves the
    /// cursor with Alt.
    #[test]
    fn the_activating_click_selects_nothing_and_copies_nothing() {
        // (first_mouse_down, smart, count, alt_only)
        let f = press_follow_up(true, false, 1, true);
        assert_eq!(f, PressFollow { drag_selects: false, copy_now: false, alt_click: false });
        let f = press_follow_up(true, true, 2, false);
        assert_eq!(f, PressFollow { drag_selects: false, copy_now: false, alt_click: false });
        // An ordinary click is as before.
        assert_eq!(press_follow_up(false, false, 1, true), PressFollow { drag_selects: true, copy_now: false, alt_click: true });
        assert_eq!(press_follow_up(false, true, 2, false), PressFollow { drag_selects: false, copy_now: true, alt_click: false });
        assert_eq!(press_follow_up(false, false, 2, true), PressFollow { drag_selects: true, copy_now: false, alt_click: false });
    }

    /// Fix round 2: the Move Pane pick is one more kind in the one list.
    #[test]
    fn the_pane_move_pick_is_an_overlay_kind() {
        assert!(Overlay::ALL.iter().any(|k| format!("{k:?}") == "PaneMove"));
        assert_eq!(Overlay::ALL.len(), 13, "13s Task 9 added CommandInfo");
    }

    /// Fix round 2, structural: the box a completion list is drawn in is the box it holds clicks in.
    #[test]
    fn completion_hit_rect_is_the_drawn_rect() {
        let bounds = crate::layout::Rect { x: 4.0, y: 4.0, w: 400.0, h: 200.0 };
        let rows: Vec<String> = vec!["alpha".into(), "beta".into()];
        let cell = (8.0, 16.0);
        let hit = crate::scene::completion_rect(&rows, 0, None, (20.0, 40.0), bounds, cell).unwrap().0;
        let mut frame = crate::scene::Frame::default();
        crate::scene::completion_list(&mut frame, &rows, 0, None, (20.0, 40.0), bounds, cell);
        assert!(frame.popups.iter().any(|q| q.rect == hit), "the first popup quad is the hit rect");
    }

    /// Review Focus 8 (`02:614`, `02:615`; `iTermFocusFollowsMouse.swift:107-165`,
    /// `iTermFocusFollowsMouseController.m:40-94`).
    #[test]
    fn focus_follows_mouse_follows_iterm2() {
        // ffm_on_enter(ffm, steal, app_active, moved_since_refusal)
        assert_eq!(ffm_on_enter(false, true, true, true), Enter::Nothing, "setting off");
        assert_eq!(ffm_on_enter(true, false, true, true), Enter::MakeKey, "Tandem is active");
        assert_eq!(ffm_on_enter(true, false, false, true), Enter::Nothing, "inactive, no steal");
        assert_eq!(ffm_on_enter(true, true, false, true), Enter::Steal);
        assert_eq!(ffm_on_enter(true, true, true, false), Enter::Nothing, "the pointer has not moved since activation");
        assert_eq!(ffm_on_enter(true, false, true, false), Enter::Nothing);
        // ffm_on_activate(ffm, aggressive, saved, now)
        assert!(ffm_on_activate(true, true, Some((1.0, 1.0)), (5.0, 5.0)));
        assert!(!ffm_on_activate(true, true, Some((1.0, 1.0)), (1.0, 1.0)), "same point");
        assert!(!ffm_on_activate(true, false, Some((1.0, 1.0)), (5.0, 5.0)), "regular activation only refuses the first responder");
        assert!(!ffm_on_activate(false, true, Some((1.0, 1.0)), (5.0, 5.0)));
        assert!(ffm_on_activate(true, true, None, (5.0, 5.0)), "no saved point counts as moved");
    }

    /// Focus follows mouse never takes focus from a modal prompt, a password prompt, an
    /// approval, or (without the opt-in) another app.
    #[test]
    fn focus_follows_mouse_never_steals_from_a_prompt() {
        let ok = |e, front, pw, ap| ffm_may_act(e, front, pw, ap, false);
        assert!(ok(Enter::MakeKey, Front::Terminal, false, false));
        assert!(!ok(Enter::MakeKey, Front::OtherTermurion, false, false), "a dialog, settings or chat window is in front");
        assert!(!ok(Enter::MakeKey, Front::Terminal, true, false), "a password prompt holds focus");
        assert!(!ok(Enter::MakeKey, Front::Terminal, false, true), "an approval is waiting");
        assert!(!ok(Enter::MakeKey, Front::Elsewhere, false, false), "MakeKey is only for an active Tandem");
        assert!(ok(Enter::Steal, Front::Elsewhere, false, false), "the opt-in steals from another app");
        assert!(!ok(Enter::Steal, Front::Elsewhere, true, false));
        assert!(!ok(Enter::Steal, Front::OtherTermurion, false, false));
        assert!(!ok(Enter::Nothing, Front::Terminal, false, false));
    }
    /// 13k seam: an IME composition holds focus follows mouse back whatever the enter says.
    #[test]
    fn focus_follows_mouse_waits_for_a_composition() {
        for e in [Enter::MakeKey, Enter::Steal] {
            assert!(!ffm_may_act(e, Front::Terminal, false, false, true));
        }
        assert!(ffm_may_act(Enter::MakeKey, Front::Terminal, false, false, false));
    }

    /// A split divider's grab zone: the divider grown by `grab` on every side. Every button
    /// pressed in it is Tandem's (left drags the divider; right and middle do nothing), never
    /// the program's.
    #[test]
    fn divider_grab_zone_holds_every_button() {
        let d = crate::layout::Rect { x: 100.0, y: 0.0, w: 4.0, h: 200.0 };
        let z = divider_grab(d, 3.0);
        assert_eq!((z.x, z.w), (97.0, 10.0));
        assert!(z.contains(98.0, 50.0) && z.contains(106.0, 50.0));
        assert!(!z.contains(96.0, 50.0));
    }
}
