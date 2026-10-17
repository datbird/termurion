//! Pure mouse decisions: click counting, the kind of selection, and the wheel's reports and arrows.

use std::time::{Duration, Instant};

pub use alacritty_terminal::selection::SelectionType;

use crate::keys::{self, Key};
use crate::mouse::{self, Action, Button, Event, Modes, Mods};

/// Windows' default double-click time.
pub const MULTI_CLICK: Duration = Duration::from_millis(500);

#[derive(Default)]
pub struct Clicks {
    last: Option<(Instant, u32, u32)>,
    count: u8,
}

impl Clicks {
    /// Counts a press: 1 to 4 for a single to a quadruple click on one cell.
    pub fn press(&mut self, now: Instant, col: u32, row: u32) -> u8 {
        self.count = match self.last {
            Some((t, c, r)) if now.duration_since(t) <= MULTI_CLICK && (c, r) == (col, row) && self.count < 4 => {
                self.count + 1
            }
            _ => 1,
        };
        self.last = Some((now, col, row));
        self.count
    }
}

/// What a left press selects.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pick {
    Type(SelectionType),
    /// Smart Selection's best rule match, else a word.
    Smart,
    /// iTerm2's word (`iTermWordExtractor`), not alacritty's separator list.
    Word,
    /// A line: a whole wrapped line or one screen row by the setting (the app decides).
    Line,
}

/// iTerm2's clicks: double selects a word, triple a line, quadruple a Smart Selection.
/// Double-click performs smart selection swaps the double and quadruple clicks.
pub fn click_pick(clicks: u8, mods: Mods, double_is_smart: bool) -> Pick {
    if mods.ctrl && mods.alt {
        return Pick::Type(SelectionType::Block);
    }
    match (clicks, double_is_smart) {
        (2, true) | (4, false) => Pick::Smart,
        (2, false) | (4, true) => Pick::Word,
        (3, _) => Pick::Line,
        _ => Pick::Type(SelectionType::Simple),
    }
}

/// `lines` wheel reports (positive is up), one encoded report repeated `|lines|` times; empty
/// when the program does not own the mouse (12f's scroll_wheel: N reports for N notches).
pub fn wheel_reports(modes: Modes, lines: i32, col: u32, row: u32) -> Vec<u8> {
    wheel_reports_with(modes, Mods::default(), lines, col, row)
}

/// As `wheel_reports`, with the modifiers held (the person's wheel).
pub fn wheel_reports_with(modes: Modes, mods: Mods, lines: i32, col: u32, row: u32) -> Vec<u8> {
    if lines == 0 || !mouse::app_owns_mouse(modes, mods) {
        return Vec::new();
    }
    let button = if lines > 0 { Button::WheelUp } else { Button::WheelDown };
    let one = mouse::encode(modes, Event { button, action: Action::Press, mods, col, row }).unwrap_or_default();
    one.repeat(lines.unsigned_abs() as usize)
}

/// A horizontal wheel's reports: `steps` of them (the whole part, either sign) as WheelLeft
/// when `left`, else WheelRight; nothing when `ReportHorizontalScrollEvents` is off, so the
/// caller makes no (empty) write.
pub fn horizontal_wheel_reports(modes: Modes, mods: Mods, steps: i32, left: bool, enabled: bool, col: u32, row: u32) -> Vec<u8> {
    if !enabled || steps == 0 {
        return Vec::new();
    }
    let button = if left { Button::WheelLeft } else { Button::WheelRight };
    mouse::encode(modes, Event { button, action: Action::Press, mods, col, row }).unwrap_or_default().repeat(steps.unsigned_abs() as usize)
}

/// The arrows the alternate-screen wheel sends, `count` times (`PTYTextView.m:8358-8380`):
/// vertical sends Up or Down, horizontal sends Left for a positive delta and Right otherwise,
/// all with the event's modifiers as the xterm parameter.
pub fn alt_arrows(up_or_left: bool, vertical: bool, mods: Mods, app_cursor: bool, count: usize) -> Vec<u8> {
    let key = match (vertical, up_or_left) {
        (true, true) => Key::Up,
        (true, false) => Key::Down,
        (false, true) => Key::Left,
        (false, false) => Key::Right,
    };
    keys::encode(&key, mods, app_cursor).unwrap_or_default().repeat(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mouse::Tracking;

    const NONE: Mods = Mods { shift: false, alt: false, ctrl: false, cmd: false };

    #[test]
    fn clicks_count_up_to_four() {
        let mut c = Clicks::default();
        let t = Instant::now();
        let ms = Duration::from_millis;
        assert_eq!(c.press(t, 1, 1), 1);
        assert_eq!(c.press(t + ms(100), 1, 1), 2);
        assert_eq!(c.press(t + ms(200), 1, 1), 3);
        assert_eq!(c.press(t + ms(300), 1, 1), 4);
        assert_eq!(c.press(t + ms(350), 1, 1), 1);
        assert_eq!(c.press(t + ms(400), 2, 1), 1);
        assert_eq!(c.press(t + ms(1500), 2, 1), 1);
    }

    #[test]
    fn quad_click_is_smart_selection() {
        assert_eq!(click_pick(4, NONE, false), Pick::Smart);
        assert_eq!(click_pick(2, NONE, false), Pick::Word);
        assert_eq!(click_pick(3, NONE, false), Pick::Line);
        assert_eq!(click_pick(1, NONE, false), Pick::Type(SelectionType::Simple));
    }

    #[test]
    fn double_click_setting_swaps_word_and_smart() {
        assert_eq!(click_pick(2, NONE, true), Pick::Smart);
        assert_eq!(click_pick(4, NONE, true), Pick::Word);
        let block = Mods { ctrl: true, alt: true, shift: false, cmd: false };
        assert_eq!(click_pick(2, block, true), Pick::Type(SelectionType::Block));
    }

    #[test]
    fn triple_click_rows_follow_iterm2() {
        assert_eq!(click_pick(3, NONE, false), Pick::Line);
        assert_eq!(click_pick(3, NONE, true), Pick::Line);
        assert_eq!(click_pick(2, NONE, false), Pick::Word);
        assert_eq!(click_pick(2, NONE, true), Pick::Smart);
        assert_eq!(click_pick(4, NONE, true), Pick::Word);
        assert_eq!(click_pick(1, NONE, false), Pick::Type(SelectionType::Simple));
    }

    #[test]
    fn orch_scroll_keeps_n_reports() {
        // 12f's scroll_wheel: N reports for N notches, nothing for a program that reports nothing.
        let modes = Modes { tracking: Tracking::Click, sgr: true };
        assert_eq!(wheel_reports(modes, 3, 2, 1), b"\x1b[<64;3;2M".repeat(3));
        assert_eq!(wheel_reports(modes, -1, 0, 0), b"\x1b[<65;1;1M".to_vec());
        assert!(wheel_reports(Modes::default(), 3, 2, 1).is_empty());
        assert!(wheel_reports(modes, 0, 2, 1).is_empty());
    }

    #[test]
    fn wheel_in_the_alternate_screen_sends_arrows_only_when_asked() {
        // R3: the arrows themselves (route_wheel decides when; wheel::alternate says No by default).
        assert_eq!(alt_arrows(true, true, NONE, false, 2), b"\x1b[A\x1b[A".to_vec());
        assert_eq!(alt_arrows(false, true, NONE, true, 1), b"\x1bOB".to_vec());
        // A positive horizontal delta sends Left (PTYTextView.m:8363-8375).
        assert_eq!(alt_arrows(true, false, NONE, false, 1), b"\x1b[D".to_vec());
        assert_eq!(alt_arrows(false, false, NONE, false, 1), b"\x1b[C".to_vec());
        assert!(alt_arrows(true, true, NONE, false, 0).is_empty());
    }

    #[test]
    fn arrows_carry_modifiers() {
        assert_eq!(alt_arrows(true, true, Mods { shift: true, ..NONE }, true, 1), b"\x1b[1;2A".to_vec());
        assert_eq!(alt_arrows(true, true, Mods { ctrl: true, ..NONE }, false, 1), b"\x1b[1;5A".to_vec());
    }

    #[test]
    fn horizontal_wheel_reports_need_the_setting() {
        let modes = Modes { tracking: Tracking::Click, sgr: true };
        let m = Mods::default();
        assert!(horizontal_wheel_reports(modes, m, 2, true, false, 2, 1).is_empty(), "setting off: no bytes at all");
        assert!(horizontal_wheel_reports(modes, m, 0, true, true, 2, 1).is_empty());
        assert_eq!(horizontal_wheel_reports(modes, m, 2, true, true, 2, 1), b"\x1b[<66;3;2M\x1b[<66;3;2M".to_vec());
        assert_eq!(horizontal_wheel_reports(modes, m, 1, false, true, 2, 1), b"\x1b[<67;3;2M".to_vec());
    }
}
