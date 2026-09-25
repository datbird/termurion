//! Window styles and their frames: a port of iTerm2's `FrameCanonicalizer` for Windows
//! coordinates (y grows down). Sizes are pixels.


pub use crate::layout::Rect;

pub const NORMAL: i64 = 0;
pub const FULL_SCREEN: i64 = 1;
pub const TOP_PERCENT: i64 = 2;
pub const BOTTOM_PERCENT: i64 = 5;
pub const LEFT_PERCENT: i64 = 6;
pub const RIGHT_PERCENT: i64 = 7;
pub const BOTTOM_CELLS: i64 = 8;
pub const TOP_CELLS: i64 = 9;
pub const LEFT_CELLS: i64 = 10;
pub const RIGHT_CELLS: i64 = 11;
pub const NO_TITLE_BAR: i64 = 12;
pub const MAXIMIZED: i64 = 15;
pub const CENTERED: i64 = 17;
pub const CENTERED_NO_TITLE_BAR: i64 = 19;

/// The Window > Window Style menu's names, in its order.
pub const MENU: [(i64, &str); 14] = [
    (NORMAL, "Normal"),
    (FULL_SCREEN, "Full Screen"),
    (MAXIMIZED, "Maximized"),
    (NO_TITLE_BAR, "No Title Bar"),
    (CENTERED, "Centered"),
    (CENTERED_NO_TITLE_BAR, "Centered, No Title Bar"),
    (TOP_PERCENT, "Full-Width Top of Screen"),
    (BOTTOM_PERCENT, "Full-Width Bottom of Screen"),
    (LEFT_PERCENT, "Full-Height Left of Screen"),
    (RIGHT_PERCENT, "Full-Height Right of Screen"),
    (TOP_CELLS, "Top of Screen"),
    (BOTTOM_CELLS, "Bottom of Screen"),
    (LEFT_CELLS, "Left of Screen"),
    (RIGHT_CELLS, "Right of Screen"),
];

pub fn name(style: i64) -> &'static str {
    MENU.iter().find(|(id, _)| *id == unthemed(style)).map_or("Normal", |(_, n)| n)
}

/// What a frame is computed from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Input {
    pub style: i64,
    /// Width and Height Percentage; negative means "use the columns or rows".
    pub percent: (f64, f64),
    pub cells: (u16, u16),
    pub cell: (f32, f32),
    /// The space around the cells (padding, tab bar, borders).
    pub chrome: (f32, f32),
    /// The window's frame now (Normal and No Title Bar keep it).
    pub initial: Rect,
    /// The monitor's work area (without the taskbar) and its whole area.
    pub work: Rect,
    pub screen: Rect,
}

/// A style's frame and how the window shows it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub rect: Rect,
    pub decorations: bool,
    pub maximized: bool,
    pub fullscreen: bool,
}

/// The Compact theme ids (13, 16, 18) are Normal, Maximized, and Centered.
pub fn unthemed(style: i64) -> i64 {
    match style {
        13 => NORMAL,
        16 => MAXIMIZED,
        18 => CENTERED,
        s => s,
    }
}

/// The id the profile stores for an edge style: the cells form when neither units popup
/// says "% of screen" (iTerm2's ProfilesWindowPreferencesViewController).
pub fn store_id(style: i64, width_percent: bool, height_percent: bool) -> i64 {
    if width_percent || height_percent {
        return style;
    }
    match style {
        TOP_PERCENT => TOP_CELLS,
        BOTTOM_PERCENT => BOTTOM_CELLS,
        LEFT_PERCENT => LEFT_CELLS,
        RIGHT_PERCENT => RIGHT_CELLS,
        s => s,
    }
}

fn preferred(i: &Input) -> (f32, f32) {
    let w = (i.cells.0 as f32 * i.cell.0 + i.chrome.0).min(i.work.w);
    let h = (i.cells.1 as f32 * i.cell.1 + i.chrome.1).min(i.work.h);
    (w, h)
}

fn percent_size(i: &Input) -> (f32, f32) {
    let p = preferred(i);
    let w = if i.percent.0 < 0.0 { p.0 } else { (i.work.w as f64 * i.percent.0 / 100.0).round() as f32 };
    let h = if i.percent.1 < 0.0 { p.1 } else { (i.work.h as f64 * i.percent.1 / 100.0).round() as f32 };
    (w, h)
}

/// The frame for the style on the given monitor.
pub fn frame(i: &Input) -> Frame {
    let s = unthemed(i.style);
    let work = i.work;
    let center_x = |w: f32| work.x + (work.w - w) / 2.0;
    let center_y = |h: f32| work.y + (work.h - h) / 2.0;
    let plain = |rect: Rect, decorations: bool| Frame { rect, decorations, maximized: false, fullscreen: false };
    let pct = percent_size(i);
    let pref = preferred(i);
    match s {
        FULL_SCREEN => Frame { rect: i.screen, decorations: false, maximized: false, fullscreen: true },
        MAXIMIZED => Frame { rect: work, decorations: true, maximized: true, fullscreen: false },
        CENTERED | CENTERED_NO_TITLE_BAR => plain(Rect { x: center_x(pref.0), y: center_y(pref.1), w: pref.0, h: pref.1 }, s == CENTERED),
        TOP_PERCENT | BOTTOM_PERCENT => {
            let (w, h) = pct;
            let y = if s == TOP_PERCENT { work.y } else { work.y + work.h - h };
            plain(Rect { x: center_x(w), y, w, h }, false)
        }
        TOP_CELLS | BOTTOM_CELLS => {
            let (w, h) = (i.initial.w.min(work.w), pref.1);
            let y = if s == TOP_CELLS { work.y } else { work.y + work.h - h };
            plain(Rect { x: center_x(w), y, w, h }, false)
        }
        LEFT_PERCENT | RIGHT_PERCENT => {
            let (w, h) = pct;
            let x = if s == LEFT_PERCENT { work.x } else { work.x + work.w - w };
            plain(Rect { x, y: center_y(h), w, h }, false)
        }
        LEFT_CELLS | RIGHT_CELLS => {
            let (w, h) = (pref.0, i.initial.h.min(work.h));
            let x = if s == LEFT_CELLS { work.x } else { work.x + work.w - w };
            plain(Rect { x, y: center_y(h), w, h }, false)
        }
        NO_TITLE_BAR => plain(i.initial, false),
        _ => plain(i.initial, true),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(style: i64) -> Input {
        Input {
            style,
            percent: (100.0, 50.0),
            cells: (80, 25),
            cell: (10.0, 20.0),
            chrome: (20.0, 40.0),
            initial: Rect { x: 100.0, y: 100.0, w: 600.0, h: 400.0 },
            // A monitor left of the primary one, with a taskbar at the bottom.
            work: Rect { x: -1920.0, y: 0.0, w: 1920.0, h: 1040.0 },
            screen: Rect { x: -1920.0, y: 0.0, w: 1920.0, h: 1080.0 },
        }
    }

    #[test]
    fn edge_frames_follow_the_work_area() {
        let top = frame(&input(TOP_PERCENT));
        assert_eq!(top.rect, Rect { x: -1920.0, y: 0.0, w: 1920.0, h: 520.0 }, "full width, half the height, at the top");
        assert!(!top.decorations);
        let bottom = frame(&input(BOTTOM_PERCENT));
        assert_eq!(bottom.rect, Rect { x: -1920.0, y: 520.0, w: 1920.0, h: 520.0 }, "above the taskbar");
        let left = frame(&Input { percent: (25.0, 100.0), ..input(LEFT_PERCENT) });
        assert_eq!(left.rect, Rect { x: -1920.0, y: 0.0, w: 480.0, h: 1040.0 });
        let right = frame(&Input { percent: (25.0, 100.0), ..input(RIGHT_PERCENT) });
        assert_eq!(right.rect, Rect { x: -480.0, y: 0.0, w: 480.0, h: 1040.0 });
    }

    #[test]
    fn percent_and_cells() {
        let cells = frame(&input(TOP_CELLS));
        assert_eq!(cells.rect, Rect { x: -1920.0 + (1920.0 - 600.0) / 2.0, y: 0.0, w: 600.0, h: 540.0 }, "rows x cell + chrome, the width kept and centered");
        let neg = frame(&Input { percent: (-1.0, 50.0), ..input(TOP_PERCENT) });
        assert_eq!(neg.rect.w, 820.0, "a negative percent uses the columns");
        let huge = frame(&Input { cells: (1000, 1000), ..input(TOP_CELLS) });
        assert_eq!(huge.rect.h, 1040.0, "never taller than the work area");
    }

    #[test]
    fn centered_and_maximized() {
        let c = frame(&input(CENTERED));
        assert_eq!(c.rect, Rect { x: -1920.0 + (1920.0 - 820.0) / 2.0, y: (1040.0 - 540.0) / 2.0, w: 820.0, h: 540.0 });
        assert!(c.decorations);
        assert!(!frame(&input(CENTERED_NO_TITLE_BAR)).decorations);
        let m = frame(&input(MAXIMIZED));
        assert!(m.maximized && m.rect == input(0).work);
        let f = frame(&input(FULL_SCREEN));
        assert!(f.fullscreen && f.rect == input(0).screen);
        let n = frame(&input(NORMAL));
        assert!(n.decorations && n.rect == input(0).initial && !n.maximized);
        assert!(!frame(&input(NO_TITLE_BAR)).decorations);
    }

    #[test]
    fn theme_ids_map_back() {
        assert_eq!((unthemed(13), unthemed(16), unthemed(18), unthemed(7)), (NORMAL, MAXIMIZED, CENTERED, RIGHT_PERCENT));
        assert_eq!(frame(&input(13)).rect, input(0).initial, "Compact is Normal");
        assert_eq!(store_id(TOP_PERCENT, false, false), TOP_CELLS, "neither units popup says % of screen: the cells id");
        assert_eq!(store_id(TOP_PERCENT, true, false), TOP_PERCENT);
        assert_eq!(store_id(RIGHT_PERCENT, false, false), RIGHT_CELLS);
        assert_eq!(store_id(CENTERED, false, false), CENTERED);
        assert_eq!(name(TOP_CELLS), "Top of Screen");
        assert_eq!(name(TOP_PERCENT), "Full-Width Top of Screen");
    }
}
