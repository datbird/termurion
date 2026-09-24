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
}
