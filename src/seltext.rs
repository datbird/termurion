//! The selection model's small pure parts: how a selection was made (its live mode) and which
//! side of a cell the pointer picks. Task 3 adds the text model beside these.

use alacritty_terminal::index::Side;

use crate::find::Pos;

/// How a selection was made, in the API's order (`iTermSelectionMode`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Character,
    Word,
    Line,
    Smart,
    Box,
    WholeLine,
}

/// What a `Screen` remembers about its live selection beyond alacritty's own: the mode that
/// made it, the column window it stays inside, and the word or line a double or triple click
/// began on (a drag snaps its moving end to whole words or lines around it).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct State {
    pub live_mode: Mode,
    pub window: Option<(usize, usize)>,
    pub anchor: Option<(Pos, Pos)>,
    /// The cell the last drag step looked at, so a pointer that stays in one cell costs nothing.
    pub last: Option<Pos>,
}

/// The side of a cell a pointer at `frac` (0 to 1 across the cell) picks: the right neighbor's
/// side once it is within `fraction` of the cell's right edge (`PTYTextView+ARC.m:316`).
pub fn side_for(frac: f32, fraction: f32) -> Side {
    if frac > 1.0 - fraction { Side::Right } else { Side::Left }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neighbor_fraction_follows_iterm2() {
        assert_eq!(side_for(0.60, 0.35), Side::Left);
        assert_eq!(side_for(0.66, 0.35), Side::Right);
        assert_eq!(side_for(0.49, 0.5), Side::Left);
        assert_eq!(side_for(0.51, 0.5), Side::Right);
    }

    #[test]
    fn the_api_order_of_modes() {
        assert_eq!(Mode::default(), Mode::Character);
        assert_eq!(Mode::Box as i32, 4);
        assert_eq!(Mode::WholeLine as i32, 5);
    }
}
