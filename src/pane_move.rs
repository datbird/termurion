//! The pick and drag model of Move Session to Split Pane, Swap With Session and title bar
//! drags: iTerm2's `SplitSelectionView.m` (half under the pointer, looks) and
//! `MovePaneController.m` (the drop). Pure; the app side is `app/pane_move_ui.rs`.

use crate::hexcolor;
use crate::layout::{Axis, Rect};
use crate::palette::{rgb, Rgb};
use crate::tmux_edits::{move_pane, swap_panes};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Half {
    None,
    West,
    East,
    North,
    South,
    Full,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Drop {
    Split { axis: Axis, before: bool },
    Swap,
    Cancel,
}

pub const ALPHA: f32 = 0.9;
pub const SOURCE_MOVE: &str = "Select a destination pane";
pub const SOURCE_SWAP: &str = "Select pane to swap with";
pub const TARGET_MOVE: &str = "Click to move source pane to this split";
pub const TARGET_SWAP: &str = "Click to swap source pane with this one";
pub const SWAP_WITH_SESSION: &str = "Swap With Session\u{2026}";

#[derive(Clone, Debug, PartialEq)]
pub struct Colors {
    pub source_fill: Rgb,
    pub source_border: Rgb,
    pub source_inner: Rgb,
    pub target_fill: Rgb,
    pub target_border: Rgb,
    pub target_inner: Rgb,
}

/// `SplitSelectionView updateAtPoint:` in top-down coordinates (x, y from the pane's top left).
/// Swap modes are always the full pane; otherwise the nearer horizontal and vertical halves
/// compete by their distance to the edge over the size, the lower score wins (horizontal on a
/// tie), nothing changes when that is the current half or the best score is 0.4 or more, and a
/// switch needs the new distance below the current half's less the hysteresis.
pub fn half_at(current: Half, swap: bool, w: f32, h: f32, x: f32, y: f32, scale: f32) -> Half {
    if swap {
        return Half::Full;
    }
    let (horiz, hs, hd) = if x < w / 2.0 { (Half::West, x / w, x) } else { (Half::East, (w - x) / w, w - x) };
    let (vert, vs, vd) = if y < h / 2.0 { (Half::North, y / h, y) } else { (Half::South, (h - y) / h, h - y) };
    let (best, score, dist) = if vs < hs { (vert, vs, vd) } else { (horiz, hs, hd) };
    if current == best || score >= 0.4 {
        return current;
    }
    let current_dist = match current {
        Half::West => x,
        Half::East => w - x,
        Half::North => y,
        Half::South => h - y,
        Half::None | Half::Full => f32::INFINITY,
    };
    let hysteresis = (0.05 * w.min(h)).max(8.0 * scale);
    if current_dist == f32::INFINITY || dist < current_dist - hysteresis {
        best
    } else {
        current
    }
}

/// The part of the pane a target highlights (`drawTargetWithMessage:`).
pub fn half_rect(r: Rect, half: Half) -> Option<Rect> {
    let (hw, hh) = (r.w / 2.0, r.h / 2.0);
    Some(match half {
        Half::None => return None,
        Half::West => Rect { w: hw, ..r },
        Half::East => Rect { x: r.x + hw, w: hw, ..r },
        Half::North => Rect { h: hh, ..r },
        Half::South => Rect { y: r.y + hh, h: hh, ..r },
        Half::Full => r,
    })
}

/// `MovePaneController`: a drop on the source does nothing; a swap needs the full pane; a move
/// splits the destination (West and East side by side, North and West first).
pub fn drop_plan(half: Half, swap: bool, same_pane: bool) -> Drop {
    if same_pane {
        return Drop::Cancel;
    }
    match (swap, half) {
        (true, Half::Full) => Drop::Swap,
        (false, Half::West) => Drop::Split { axis: Axis::Row, before: true },
        (false, Half::East) => Drop::Split { axis: Axis::Row, before: false },
        (false, Half::North) => Drop::Split { axis: Axis::Column, before: true },
        (false, Half::South) => Drop::Split { axis: Axis::Column, before: false },
        _ => Drop::Cancel,
    }
}

/// `PTYSession isCompatibleWith:`: both outside tmux, or the same tmux gateway.
pub fn compatible(src_gateway: Option<u64>, dst_gateway: Option<u64>) -> bool {
    src_gateway == dst_gateway
}

/// The tmux command for a drop, from numeric pane ids only.
pub fn tmux_command(src: u32, dst: u32, plan: Drop) -> Option<String> {
    match plan {
        Drop::Split { axis, before } => Some(move_pane(src, dst, axis == Axis::Row, before)),
        Drop::Swap => Some(swap_panes(src, dst)),
        Drop::Cancel => None,
    }
}

/// Each Advanced color through `hexcolor::parse`, else iTerm2's fallback (`SplitSelectionView.m`).
pub fn colors(get: &dyn Fn(&str) -> String) -> Colors {
    let of = |key: &str, fallback: Rgb| hexcolor::parse(&get(key)).unwrap_or(fallback);
    Colors {
        source_fill: of("SplitPaneSourceFillColor", rgb(0, 128, 0)),
        source_border: of("SplitPaneSourceBorderColor", rgb(255, 255, 255)),
        source_inner: of("SplitPaneSourceInnerBorderColor", rgb(0, 0, 0)),
        target_fill: of("SplitPaneTargetDropFillColor", rgb(128, 0, 0)),
        target_border: of("SplitPaneTargetDropBorderColor", rgb(255, 255, 255)),
        target_inner: of("SplitPaneTargetDropInnerBorderColor", rgb(0, 0, 0)),
    }
}

/// The message's scale (iTerm2 steps the font from 24 points down to 8 until the text fits):
/// the largest that fits `rect_w`, within 8/16 and 24/16 of the cell height (`_cell_h` is kept
/// so the caller states which cell the text width was measured against).
pub fn message_scale(text_w_at_1: f32, rect_w: f32, _cell_h: f32) -> f32 {
    let fit = if text_w_at_1 > 0.0 { rect_w / text_w_at_1 } else { f32::INFINITY };
    fit.clamp(8.0 / 16.0, 24.0 / 16.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::palette::rgb;

    #[test]
    fn halves_follow_iterm2() {
        let h = |cur, swap, w, hh, x, y, s| half_at(cur, swap, w, hh, x, y, s);
        assert_eq!(h(Half::None, false, 200.0, 100.0, 10.0, 50.0, 1.0), Half::West);
        assert_eq!(h(Half::West, false, 200.0, 100.0, 100.0, 5.0, 1.0), Half::North);
        assert_eq!(h(Half::North, false, 200.0, 100.0, 60.0, 45.0, 1.0), Half::North);
        for cur in [Half::None, Half::West, Half::East, Half::North, Half::South] {
            assert_eq!(h(cur, false, 200.0, 100.0, 80.0, 50.0, 1.0), cur);
        }
        assert_eq!(h(Half::None, false, 200.0, 100.0, 190.0, 50.0, 1.0), Half::East);
        assert_eq!(h(Half::None, false, 200.0, 100.0, 100.0, 95.0, 1.0), Half::South);
        for (x, y) in [(10.0, 50.0), (190.0, 50.0), (100.0, 5.0), (100.0, 95.0)] {
            assert_eq!(h(Half::None, true, 200.0, 100.0, x, y, 1.0), Half::Full);
        }
        // Hysteresis: 8 minimum on a 20 x 20 pane. From West at (9.9, y) North's distance y must
        // be below 9.9 - 8.
        assert_eq!(h(Half::West, false, 20.0, 20.0, 9.9, 3.0, 1.0), Half::West);
        assert_eq!(h(Half::West, false, 20.0, 20.0, 9.9, 1.0, 1.0), Half::North);
        // 400 x 400 uses 20: from West at (150, y), y must be below 130.
        assert_eq!(h(Half::West, false, 400.0, 400.0, 150.0, 135.0, 1.0), Half::West);
        assert_eq!(h(Half::West, false, 400.0, 400.0, 150.0, 125.0, 1.0), Half::North);
        // The minimum scales with the window scale (16 at scale 2).
        assert_eq!(h(Half::West, false, 20.0, 20.0, 9.9, 1.0, 2.0), Half::West);
    }

    #[test]
    fn drop_plan_follows_iterm2() {
        let split = |axis, before| Drop::Split { axis, before };
        assert_eq!(drop_plan(Half::West, false, false), split(Axis::Row, true));
        assert_eq!(drop_plan(Half::East, false, false), split(Axis::Row, false));
        assert_eq!(drop_plan(Half::North, false, false), split(Axis::Column, true));
        assert_eq!(drop_plan(Half::South, false, false), split(Axis::Column, false));
        assert_eq!(drop_plan(Half::Full, true, false), Drop::Swap);
        assert_eq!(drop_plan(Half::None, false, false), Drop::Cancel);
        assert_eq!(drop_plan(Half::Full, false, false), Drop::Cancel);
        assert_eq!(drop_plan(Half::None, true, false), Drop::Cancel);
        for half in [Half::None, Half::West, Half::East, Half::North, Half::South, Half::Full] {
            for swap in [false, true] {
                assert_eq!(drop_plan(half, swap, true), Drop::Cancel);
            }
        }
        let r = Rect { x: 0.0, y: 0.0, w: 200.0, h: 100.0 };
        let at = |x, y, w, h| Some(Rect { x, y, w, h });
        assert_eq!(half_rect(r, Half::West), at(0.0, 0.0, 100.0, 100.0));
        assert_eq!(half_rect(r, Half::East), at(100.0, 0.0, 100.0, 100.0));
        assert_eq!(half_rect(r, Half::North), at(0.0, 0.0, 200.0, 50.0));
        assert_eq!(half_rect(r, Half::South), at(0.0, 50.0, 200.0, 50.0));
        assert_eq!(half_rect(r, Half::Full), Some(r));
        assert_eq!(half_rect(r, Half::None), None);
        assert_eq!(tmux_command(1, 2, split(Axis::Row, true)).as_deref(), Some("move-pane -s \"%1\" -t \"%2\" -h -b"));
        assert_eq!(tmux_command(1, 2, split(Axis::Column, false)).as_deref(), Some("move-pane -s \"%1\" -t \"%2\" -v"));
        assert_eq!(tmux_command(1, 2, Drop::Swap).as_deref(), Some("swap-pane -s \"%1\" -t \"%2\""));
        assert_eq!(tmux_command(1, 2, Drop::Cancel), None);
    }

    #[test]
    fn compatibility_follows_iterm2() {
        assert!(compatible(None, None));
        assert!(compatible(Some(4), Some(4)));
        assert!(!compatible(Some(4), Some(5)));
        assert!(!compatible(None, Some(4)));
        assert!(!compatible(Some(4), None));
    }

    #[test]
    fn looks_follow_iterm2() {
        let c = colors(&|_| String::new());
        assert_eq!(
            [c.source_fill, c.source_border, c.source_inner, c.target_fill, c.target_border, c.target_inner],
            [rgb(0, 128, 0), rgb(255, 255, 255), rgb(0, 0, 0), rgb(128, 0, 0), rgb(255, 255, 255), rgb(0, 0, 0)]
        );
        let one = colors(&|k| if k == "SplitPaneTargetDropFillColor" { "#123456".to_string() } else { String::new() });
        assert_eq!(one.target_fill, rgb(0x12, 0x34, 0x56));
        assert_eq!(Colors { target_fill: c.target_fill, ..one.clone() }, c);
        let junk = colors(&|_| "junk".to_string());
        assert_eq!(junk, c);
        assert_eq!(ALPHA, 0.9);
        assert_eq!(SOURCE_MOVE, "Select a destination pane");
        assert_eq!(SOURCE_SWAP, "Select pane to swap with");
        assert_eq!(TARGET_MOVE, "Click to move source pane to this split");
        assert_eq!(TARGET_SWAP, "Click to swap source pane with this one");
        assert_eq!(SWAP_WITH_SESSION, "Swap With Session\u{2026}");
        assert_eq!(message_scale(600.0, 300.0, 16.0), 8.0 / 16.0);
        assert_eq!(message_scale(10.0, 300.0, 16.0), 24.0 / 16.0);
        let s = message_scale(400.0, 300.0, 16.0);
        assert!((s * 400.0 - 300.0).abs() < 0.01 && s >= 0.5 && s <= 1.5);
    }
}
