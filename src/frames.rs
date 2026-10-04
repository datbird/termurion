//! Window styles and their frames: a port of iTerm2's `FrameCanonicalizer` for Windows
//! coordinates (y grows down). Sizes are pixels.


pub use crate::layout::Rect;

pub const NORMAL: i64 = 0;
pub const FULL_SCREEN: i64 = 1;
pub const TOP_PERCENT: i64 = 2;
/// macOS's native full screen; Tandem opens it as Full Screen.
pub const LION_FULL_SCREEN: i64 = 4;
pub const BOTTOM_PERCENT: i64 = 5;
pub const LEFT_PERCENT: i64 = 6;
pub const RIGHT_PERCENT: i64 = 7;
pub const BOTTOM_CELLS: i64 = 8;
pub const TOP_CELLS: i64 = 9;
pub const LEFT_CELLS: i64 = 10;
pub const RIGHT_CELLS: i64 = 11;
pub const NO_TITLE_BAR: i64 = 12;
/// A floating panel in iTerm2 (scripting and the command line only); Tandem's is a Normal
/// window on top of the others without a taskbar button.
pub const ACCESSORY: i64 = 14;
pub const MAXIMIZED: i64 = 15;
/// The Compact and Minimal themes' Normal, Maximized and Centered.
pub const COMPACT: i64 = 13;
pub const COMPACT_MAXIMIZED: i64 = 16;
pub const COMPACT_CENTERED: i64 = 18;
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

/// The profile's Window Style popup (`XIB`, outlet `_windowStyle`), in its order: one entry
/// per edge, its units popups choosing the percent or the cells form.
pub const PROFILE_MENU: [(i64, &str); 10] = [
    (NORMAL, "Normal"),
    (FULL_SCREEN, "Full Screen"),
    (MAXIMIZED, "Maximized"),
    (NO_TITLE_BAR, "No Title Bar"),
    (CENTERED, "Centered"),
    (CENTERED_NO_TITLE_BAR, "Centered, No Title Bar"),
    (BOTTOM_PERCENT, "Bottom of Screen"),
    (TOP_PERCENT, "Top of Screen"),
    (LEFT_PERCENT, "Left of Screen"),
    (RIGHT_PERCENT, "Right of Screen"),
];

/// The profile popup's entry for a stored type (`PWV:477-520`): the Compact forms show as
/// theirs, the cells ids 8 to 11 as their edge's entry.
pub fn profile_menu_value(stored: i64) -> i64 {
    match unthemed(stored) {
        BOTTOM_CELLS => BOTTOM_PERCENT,
        TOP_CELLS => TOP_PERCENT,
        LEFT_CELLS => LEFT_PERCENT,
        RIGHT_CELLS => RIGHT_PERCENT,
        s => s,
    }
}

/// Whether a type may be sized in percent of the screen (`percentageEligible`, `WSH:44-62`):
/// the percentage edge styles, Normal, No Title Bar, Centered, Centered No Title Bar and the
/// Compact and Compact Centered forms.
pub fn percent_eligible(style: i64) -> bool {
    matches!(style, TOP_PERCENT | BOTTOM_PERCENT | LEFT_PERCENT | RIGHT_PERCENT | NORMAL | NO_TITLE_BAR | 13 | CENTERED | 18 | CENTERED_NO_TITLE_BAR)
}

/// Each axis's share of the screen for a new window, or None for its columns or rows
/// (`measurements(profile:)`, `WSH:170-215`): an eligible type's explicit, non-negative
/// Width and Height Percentage; unset, the percentage edge styles span their attached edge.
pub fn percents(style: i64, width: Option<f64>, height: Option<f64>) -> (Option<f64>, Option<f64>) {
    if !percent_eligible(style) {
        return (None, None);
    }
    let axis = |set: Option<f64>, spans: bool| match set {
        Some(p) if p < 0.0 => None,
        Some(p) => Some(p),
        None => spans.then_some(100.0),
    };
    (axis(width, matches!(style, TOP_PERCENT | BOTTOM_PERCENT)), axis(height, matches!(style, LEFT_PERCENT | RIGHT_PERCENT)))
}

/// A profile's Width and Height Percentage when explicitly set (a missing key is None, as
/// iTerm2's `valueIsExplicitlySetForKey:` and `profile[KEY_WIDTH_PERCENTAGE] as? Double` read it).
pub fn profile_percents(p: &crate::profile::Profile) -> (Option<f64>, Option<f64>) {
    let get = |k: &str| p.values.get(k).and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64)));
    (get("Width Percentage"), get("Height Percentage"))
}

/// The Width and Height Percentage an arrangement saves for a window's profile: an explicit
/// non-negative percentage, else -1 for cells, as the units popups store it (`saveWindowType`).
pub fn saved_percents(width: Option<f64>, height: Option<f64>) -> (f64, f64) {
    let axis = |p: Option<f64>| p.filter(|p| *p >= 0.0).unwrap_or(-1.0);
    (axis(width), axis(height))
}

/// The cells a percentage of the screen holds (`measurementToCells`, `WSH:223-233`).
pub fn percent_cells(percent: f64, cell: f32, screen: f32) -> u16 {
    (f64::from(screen) * percent / (100.0 * f64::from(cell.max(1.0)))).round().clamp(1.0, f64::from(u16::MAX)) as u16
}

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
        COMPACT => NORMAL,
        COMPACT_MAXIMIZED => MAXIMIZED,
        COMPACT_CENTERED => CENTERED,
        s => s,
    }
}

/// What a display change does to a window of a type (`canonicalizeWindowFrame`,
/// `PT:5144-5215`): normal, no-title-bar and accessory windows keep their frame, centered ones
/// center again keeping their size, every other type takes its canonical frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refit {
    Keep,
    Recenter,
    Fit,
}

pub fn refit(style: i64) -> Refit {
    match unthemed(style) {
        NORMAL | NO_TITLE_BAR | ACCESSORY => Refit::Keep,
        CENTERED | CENTERED_NO_TITLE_BAR => Refit::Recenter,
        _ => Refit::Fit,
    }
}

/// A frame centered in a work area, keeping its size up to the work area's.
pub fn recenter(current: Rect, work: Rect) -> Rect {
    let (w, h) = (current.w.min(work.w), current.h.min(work.h));
    Rect { x: work.x + ((work.w - w) / 2.0).floor(), y: work.y + ((work.h - h) / 2.0).floor(), w, h }
}

/// The theme's form of a type (`iTermThemedWindowType`, `ITAddressBookMgr.m:120-175`): under
/// the Compact and Minimal themes Normal, Maximized and Centered become their Compact forms,
/// and back under the others; every other type is itself.
pub fn themed(style: i64, compact_theme: bool) -> i64 {
    match (style, compact_theme) {
        (NORMAL | COMPACT, true) => COMPACT,
        (NORMAL | COMPACT, false) => NORMAL,
        (MAXIMIZED | COMPACT_MAXIMIZED, true) => COMPACT_MAXIMIZED,
        (MAXIMIZED | COMPACT_MAXIMIZED, false) => MAXIMIZED,
        (CENTERED | COMPACT_CENTERED, true) => COMPACT_CENTERED,
        (CENTERED | COMPACT_CENTERED, false) => CENTERED,
        (s, _) => s,
    }
}

/// The user may drag a window of this type (`ptyWindowIsDraggable:`, `PT:2106-2135`): not a
/// centered or edge-docked one, nor full screen.
pub fn draggable(style: i64) -> bool {
    matches!(style, NORMAL | MAXIMIZED | NO_TITLE_BAR | ACCESSORY | COMPACT | COMPACT_MAXIMIZED)
}

/// The type is one of the Compact theme's forms (13, 16, 18), whose caption area holds the tabs.
pub fn is_compact(style: i64) -> bool {
    matches!(style, COMPACT | COMPACT_MAXIMIZED | COMPACT_CENTERED)
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
        FULL_SCREEN | LION_FULL_SCREEN => Frame { rect: i.screen, decorations: false, maximized: false, fullscreen: true },
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
    fn window_types_follow_iterm2() {
        let lion = frame(&input(LION_FULL_SCREEN));
        assert!(lion.fullscreen && lion.rect == input(0).screen, "Lion Full Screen opens full screen");
        let accessory = frame(&input(ACCESSORY));
        assert!(accessory.decorations && !accessory.fullscreen && !accessory.maximized);
        assert_eq!(accessory.rect, input(0).initial, "an Accessory window has the Normal frame");
        for s in [0, 12, 13, 17, 18, 19, 2, 5, 6, 7] {
            assert!(percent_eligible(s), "{s}");
        }
        for s in [1, 4, 8, 9, 10, 11, 14, 15, 16] {
            assert!(!percent_eligible(s), "{s}");
        }
        assert_eq!((profile_menu_value(9), profile_menu_value(8), profile_menu_value(10), profile_menu_value(11), profile_menu_value(0)), (2, 5, 6, 7, 0));
        let ids: Vec<i64> = PROFILE_MENU.iter().map(|(id, _)| *id).collect();
        assert_eq!(ids, [0, 1, 15, 12, 17, 19, 5, 2, 6, 7], "the profile popup's order");
        assert_eq!(PROFILE_MENU[6], (BOTTOM_PERCENT, "Bottom of Screen"));
        assert_eq!(PROFILE_MENU[5], (CENTERED_NO_TITLE_BAR, "Centered, No Title Bar"));
    }

    #[test]
    fn window_size_follows_iterm2() {
        // An explicit percentage sizes an eligible style in whole cells of that share of the
        // screen (`WSH:170-235`): 50% of 1920 at 10 px is 96 columns.
        assert_eq!(percents(NORMAL, Some(50.0), None), (Some(50.0), None));
        assert_eq!(percents(NORMAL, Some(-1.0), Some(-1.0)), (None, None), "negative is cells");
        assert_eq!(percents(MAXIMIZED, Some(50.0), Some(50.0)), (None, None), "not eligible");
        assert_eq!(percents(TOP_CELLS, Some(50.0), None), (None, None));
        // Unset, the edge styles span their attached edge.
        assert_eq!(percents(TOP_PERCENT, None, None), (Some(100.0), None));
        assert_eq!(percents(RIGHT_PERCENT, None, None), (None, Some(100.0)));
        assert_eq!(percent_cells(50.0, 10.0, 1920.0), 96);
        assert_eq!(percent_cells(25.0, 22.0, 1040.0), 12, "260 / 22 rounds to 12");
        assert_eq!(percent_cells(100.0, 0.5, 100.0), 100, "a cell under 1 counts as 1");
        // An arrangement saves what the units popups would store: -1 for a cells axis.
        assert_eq!(saved_percents(None, None), (-1.0, -1.0), "unset is cells, not the schema's 100");
        assert_eq!(saved_percents(Some(50.0), Some(-1.0)), (50.0, -1.0));
        assert_eq!(saved_percents(Some(0.0), Some(30.0)), (0.0, 30.0));
    }

    /// `ptyWindowIsDraggable:` (`PT:2106-2135`): centered and edge-docked windows and full
    /// screen are not; Normal, Maximized, No Title Bar, Accessory and the Compact forms are.
    #[test]
    fn draggable_types_follow_iterm2() {
        for t in [NORMAL, MAXIMIZED, NO_TITLE_BAR, ACCESSORY, COMPACT, COMPACT_MAXIMIZED] {
            assert!(draggable(t), "{}", name(t));
        }
        for t in [FULL_SCREEN, LION_FULL_SCREEN, CENTERED, COMPACT_CENTERED, CENTERED_NO_TITLE_BAR, TOP_PERCENT, BOTTOM_PERCENT, LEFT_PERCENT, RIGHT_PERCENT, TOP_CELLS, BOTTOM_CELLS, LEFT_CELLS, RIGHT_CELLS] {
            assert!(!draggable(t), "{}", name(t));
        }
    }

    #[test]
    fn themed_window_types_follow_iterm2() {
        assert_eq!((themed(NORMAL, true), themed(13, true), themed(13, false)), (13, 13, 0));
        assert_eq!((themed(MAXIMIZED, true), themed(16, false)), (16, 15));
        assert_eq!((themed(CENTERED, true), themed(18, false)), (18, 17));
        assert_eq!((themed(FULL_SCREEN, true), themed(TOP_CELLS, true)), (1, 9), "only Normal, Maximized and Centered have compact forms");
        assert_eq!(unthemed(13), NORMAL, "the frame math is unchanged");
        assert_eq!((COMPACT, COMPACT_MAXIMIZED, COMPACT_CENTERED), (13, 16, 18));
        assert!(is_compact(13) && is_compact(16) && is_compact(18));
        assert!(!is_compact(0) && !is_compact(15) && !is_compact(17) && !is_compact(1));
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

    /// `PT:5144-5215` (`canonicalizeWindowFrame`): normal and no-title-bar windows keep their
    /// frame, centered ones re-center, maximized and edge windows take the canonical frame.
    #[test]
    fn refit_follows_iterm2() {
        for s in [NORMAL, NO_TITLE_BAR] {
            assert_eq!(refit(s), Refit::Keep, "{s}");
        }
        for s in [CENTERED, CENTERED_NO_TITLE_BAR] {
            assert_eq!(refit(s), Refit::Recenter, "{s}");
        }
        for s in [MAXIMIZED, TOP_PERCENT, BOTTOM_PERCENT, LEFT_PERCENT, RIGHT_PERCENT, TOP_CELLS, BOTTOM_CELLS, LEFT_CELLS, RIGHT_CELLS, FULL_SCREEN, LION_FULL_SCREEN] {
            assert_eq!(refit(s), Refit::Fit, "{s}");
        }
        assert_eq!(refit(ACCESSORY), Refit::Keep);
        // A themed id follows its base.
        assert_eq!(refit(COMPACT), Refit::Keep);
        assert_eq!(refit(COMPACT_CENTERED), Refit::Recenter);
        assert_eq!(refit(COMPACT_MAXIMIZED), Refit::Fit);
    }

    /// A centered window keeps its size and is centered in its monitor's work area (`PT:5160-5172`).
    #[test]
    fn recenter_keeps_size() {
        let work = Rect { x: -1920.0, y: 0.0, w: 1920.0, h: 1040.0 };
        assert_eq!(recenter(Rect { x: 0.0, y: 0.0, w: 600.0, h: 400.0 }, work), Rect { x: -1260.0, y: 320.0, w: 600.0, h: 400.0 });
        let work = Rect { x: 0.0, y: 0.0, w: 1920.0, h: 1040.0 };
        assert_eq!(recenter(Rect { x: 5.0, y: 5.0, w: 2000.0, h: 1200.0 }, work), work, "clamped to the work area");
    }
}
