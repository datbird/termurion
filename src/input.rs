//! Pure mouse decisions: click counting, the kind of selection, and what the wheel does.

use std::time::{Duration, Instant};

pub use alacritty_terminal::selection::SelectionType;

use crate::mouse::{self, Action, Button, Event, Modes, Mods};

/// Windows' default double-click time.
pub const MULTI_CLICK: Duration = Duration::from_millis(500);

#[derive(Default)]
pub struct Clicks {
    last: Option<(Instant, u32, u32)>,
    count: u8,
}

impl Clicks {
    /// Counts a press: 1, 2 or 3 for a single, double or triple click on one cell.
    pub fn press(&mut self, now: Instant, col: u32, row: u32) -> u8 {
        self.count = match self.last {
            Some((t, c, r)) if now.duration_since(t) <= MULTI_CLICK && (c, r) == (col, row) && self.count < 3 => {
                self.count + 1
            }
            _ => 1,
        };
        self.last = Some((now, col, row));
        self.count
    }
}

/// Double-click selects a word, triple-click a line, Ctrl+Alt-drag a rectangle.
pub fn selection_type(clicks: u8, mods: Mods) -> SelectionType {
    if mods.ctrl && mods.alt {
        return SelectionType::Block;
    }
    match clicks {
        2 => SelectionType::Semantic,
        3 => SelectionType::Lines,
        _ => SelectionType::Simple,
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Wheel {
    /// Bytes for the app.
    Send(Vec<u8>),
    /// Lines to scroll the history (positive is up).
    Scroll(i32),
}

/// What `lines` of wheel movement do (positive is up). A tracking app gets wheel
/// events. In the alternate screen without tracking, the wheel sends arrow keys,
/// as iTerm2 does by default. Otherwise it scrolls the history.
pub fn wheel(modes: Modes, mods: Mods, alt_screen: bool, app_cursor: bool, lines: i32, col: u32, row: u32) -> Wheel {
    let n = lines.unsigned_abs() as usize;
    if lines != 0 && mouse::app_owns_mouse(modes, mods) {
        let button = if lines > 0 { Button::WheelUp } else { Button::WheelDown };
        let one = mouse::encode(modes, Event { button, action: Action::Press, mods, col, row }).unwrap_or_default();
        return Wheel::Send(one.repeat(n));
    }
    if lines != 0 && alt_screen {
        let arrow: &[u8] = match (lines > 0, app_cursor) {
            (true, true) => b"\x1bOA",
            (true, false) => b"\x1b[A",
            (false, true) => b"\x1bOB",
            (false, false) => b"\x1b[B",
        };
        return Wheel::Send(arrow.repeat(n));
    }
    Wheel::Scroll(lines)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mouse::Tracking;

    const NONE: Mods = Mods { shift: false, alt: false, ctrl: false, cmd: false };

    #[test]
    fn clicks_count_up_to_three() {
        let mut c = Clicks::default();
        let t = Instant::now();
        let ms = Duration::from_millis;
        assert_eq!(c.press(t, 1, 1), 1);
        assert_eq!(c.press(t + ms(100), 1, 1), 2);
        assert_eq!(c.press(t + ms(200), 1, 1), 3);
        assert_eq!(c.press(t + ms(300), 1, 1), 1);
        assert_eq!(c.press(t + ms(400), 2, 1), 1);
        assert_eq!(c.press(t + ms(1500), 2, 1), 1);
    }

    #[test]
    fn selection_kinds() {
        assert_eq!(selection_type(1, NONE), SelectionType::Simple);
        assert_eq!(selection_type(2, NONE), SelectionType::Semantic);
        assert_eq!(selection_type(3, NONE), SelectionType::Lines);
        assert_eq!(selection_type(1, Mods { ctrl: true, alt: true, shift: false, cmd: false }), SelectionType::Block);
    }

    #[test]
    fn wheel_goes_to_a_tracking_app() {
        let modes = Modes { tracking: Tracking::Click, sgr: true };
        assert_eq!(wheel(modes, NONE, false, false, 2, 2, 1), Wheel::Send(b"\x1b[<64;3;2M\x1b[<64;3;2M".to_vec()));
        assert_eq!(wheel(modes, NONE, false, false, -1, 0, 0), Wheel::Send(b"\x1b[<65;1;1M".to_vec()));
    }

    #[test]
    fn wheel_sends_arrows_in_the_alternate_screen() {
        let off = Modes::default();
        assert_eq!(wheel(off, NONE, true, false, 2, 0, 0), Wheel::Send(b"\x1b[A\x1b[A".to_vec()));
        assert_eq!(wheel(off, NONE, true, true, -1, 0, 0), Wheel::Send(b"\x1bOB".to_vec()));
    }

    #[test]
    fn wheel_scrolls_history_otherwise() {
        assert_eq!(wheel(Modes::default(), NONE, false, false, 3, 0, 0), Wheel::Scroll(3));
        let modes = Modes { tracking: Tracking::Click, sgr: true };
        let alt = Mods { alt: true, ..NONE };
        assert_eq!(wheel(modes, alt, false, false, 3, 0, 0), Wheel::Scroll(3));
    }
}
