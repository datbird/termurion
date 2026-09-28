//! The caption area of a Minimal or Compact window (milestone 13e): the tab bar sits in the strip
//! where the title bar was, with the window buttons at its right (Windows draws them nowhere
//! else, so Tandem does). Pure geometry: the strip, its buttons, the room the tabs get, and what
//! a point of the client answers to `WM_NCHITTEST`. Sizes are pixels.

pub use crate::layout::Rect;

/// One window button's width in points (Windows 11).
pub const BUTTON_W: f32 = 46.0;
/// `CompactTabBarStoplightButtonsWidth`'s default: the width iTerm2 reserves for its stoplights.
const REFERENCE_SETTING: f32 = 75.0;
/// The window name's margins in points (`RTV:54-70`).
const NAME_LEFT: f32 = 6.0;
const NAME_RIGHT: f32 = 14.0;

/// Segoe Fluent Icons glyphs.
pub const MINIMIZE_GLYPH: char = '\u{E921}';
pub const MAXIMIZE_GLYPH: char = '\u{E922}';
pub const RESTORE_GLYPH: char = '\u{E923}';
pub const CLOSE_GLYPH: char = '\u{E8BB}';

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Minimize,
    Maximize,
    Close,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    /// The whole strip, from the window's top to `bar_h`.
    pub bar: Rect,
    pub buttons: [(Button, Rect); 3],
    /// Where the tab bar sits: after the window name, before the shortcut label and the buttons.
    pub tabs: Rect,
    /// The window name beside the tabs (Task 8); None without one.
    pub name: Option<Rect>,
    /// The window switch label (R11), right-aligned directly left of the reserved button room;
    /// None when its width is 0.
    pub shortcut: Option<Rect>,
    /// The top strip that drags the window (Task 7 fills it in).
    pub drag_edge: f32,
    pub drag_regions: Vec<Rect>,
    /// The resize border along the top, in pixels: the frame thickness, 0 when maximized.
    pub resize_top: f32,
    pub maximized: bool,
}

/// The room reserved at the right for the buttons: their width and the setting's difference from
/// iTerm2's 75 (`CompactTabBarStoplightButtonsWidth`); a smaller setting lets the tabs overlap
/// the buttons, as iTerm2's help says.
pub fn reserved_for_buttons(setting: f32, buttons_w: f32) -> f32 {
    (buttons_w + setting - REFERENCE_SETTING).max(0.0)
}

/// What the pointer did to a window button (`WM_NCMOUSEMOVE`, `WM_NCLBUTTONDOWN`, `WM_NCLBUTTONUP`),
/// told to the app by the window's subclass.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// The pointer is over a button, or over none.
    Hover(Option<Button>),
    Down(Button),
    Up(Button),
}

/// The button a release clicks: the one pressed and let go over the same button.
pub fn clicked(pressed: Option<Button>, released: Button) -> Option<Button> {
    pressed.filter(|p| *p == released)
}

/// The button a non-client hit-test code is over.
pub fn button_of(hit: Hit) -> Option<Button> {
    match hit {
        Hit::Min => Some(Button::Minimize),
        Hit::Max => Some(Button::Maximize),
        Hit::Close => Some(Button::Close),
        _ => None,
    }
}

/// The strip of a `client_w` wide window, `bar_h` tall, at `scale`. `setting` and `extra_space` are
/// points (`CompactTabBarStoplightButtonsWidth`, `ExtraSpaceBeforeCompactTopTabBar`); `name_w`
/// and `shortcut_w` are pixels, 0 for none.
#[allow(clippy::too_many_arguments)]
pub fn layout(client_w: f32, bar_h: f32, scale: f32, setting: f32, extra_space: f32, name_w: f32, shortcut_w: f32, resize_top: f32, maximized: bool) -> Layout {
    let bw = BUTTON_W * scale;
    let first = client_w - 3.0 * bw;
    let rect = |k: f32| Rect { x: first + k * bw, y: 0.0, w: bw, h: bar_h };
    let buttons = [(Button::Minimize, rect(0.0)), (Button::Maximize, rect(1.0)), (Button::Close, rect(2.0))];
    // The setting's difference from 75 scales with the buttons.
    let reserved = reserved_for_buttons(REFERENCE_SETTING + (setting - REFERENCE_SETTING) * scale, 3.0 * bw);
    let room_end = client_w - reserved;
    let shortcut = (shortcut_w > 0.0).then(|| Rect { x: room_end - shortcut_w, y: 0.0, w: shortcut_w, h: bar_h });
    let end = shortcut.map_or(room_end, |s| s.x) - extra_space * scale;
    let name = (name_w > 0.0).then(|| Rect { x: NAME_LEFT * scale, y: 0.0, w: name_w, h: bar_h });
    let start = name.map_or(0.0, |n| n.x + n.w + NAME_RIGHT * scale);
    Layout {
        bar: Rect { x: 0.0, y: 0.0, w: client_w, h: bar_h },
        buttons,
        tabs: Rect { x: start, y: 0.0, w: (end - start).max(0.0), h: bar_h },
        name,
        shortcut,
        drag_edge: 0.0,
        drag_regions: Vec::new(),
        resize_top,
        maximized,
    }
}

/// The tab bar's rect: the strip's tab area while a top bar shows, else None.
pub fn tab_bar_rect(l: &Layout, top_bar_visible: bool) -> Option<Rect> {
    top_bar_visible.then_some(l.tabs)
}

/// What a client point answers to `WM_NCHITTEST`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    Client,
    Caption,
    /// The resize border along the top.
    Top,
    Min,
    Max,
    Close,
}

/// The answer for the client point (`x`, `y`): the top resize border first, then a button, then
/// the strip (`over_tab`: the point is over a tab or a button of the tab bar, which stay client).
pub fn hit(l: &Layout, x: f32, y: f32, over_tab: bool) -> Hit {
    if !l.bar.contains(x, y) {
        return Hit::Client;
    }
    if !l.maximized && y < l.resize_top {
        return Hit::Top;
    }
    if let Some((b, _)) = l.buttons.iter().find(|(_, r)| r.contains(x, y)) {
        return match b {
            Button::Minimize => Hit::Min,
            Button::Maximize => Hit::Max,
            Button::Close => Hit::Close,
        };
    }
    if over_tab {
        Hit::Client
    } else {
        Hit::Caption
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect { x, y, w, h }
    }

    #[test]
    fn caption_layout_follows_iterm2() {
        assert_eq!(reserved_for_buttons(75.0, 138.0), 138.0);
        assert_eq!(reserved_for_buttons(100.0, 138.0), 163.0);
        assert_eq!(reserved_for_buttons(0.0, 138.0), 63.0);
        assert_eq!(reserved_for_buttons(-10.0, 138.0), 53.0);
        assert_eq!(reserved_for_buttons(-200.0, 138.0), 0.0);
        let l = layout(1000.0, 38.0, 1.0, 75.0, 0.0, 0.0, 0.0, 8.0, false);
        assert_eq!(l.bar, r(0.0, 0.0, 1000.0, 38.0));
        assert_eq!(l.buttons, [(Button::Minimize, r(862.0, 0.0, 46.0, 38.0)), (Button::Maximize, r(908.0, 0.0, 46.0, 38.0)), (Button::Close, r(954.0, 0.0, 46.0, 38.0))]);
        assert_eq!(l.tabs, r(0.0, 0.0, 862.0, 38.0));
        assert_eq!((l.shortcut, l.name, l.resize_top, l.maximized), (None, None, 8.0, false));
        assert_eq!((l.drag_edge, l.drag_regions.len()), (0.0, 0), "Task 7 fills the drag edge");
        // The extra space is left of the buttons' room.
        assert_eq!(layout(1000.0, 38.0, 1.0, 75.0, 20.0, 0.0, 0.0, 8.0, false).tabs, r(0.0, 0.0, 842.0, 38.0));
        // The shortcut label sits right-aligned directly left of the reserved room.
        let s = layout(1000.0, 38.0, 1.0, 75.0, 0.0, 0.0, 70.0, 8.0, false);
        assert_eq!((s.shortcut, s.tabs), (Some(r(792.0, 0.0, 70.0, 38.0)), r(0.0, 0.0, 792.0, 38.0)));
        let both = layout(1000.0, 38.0, 1.0, 75.0, 20.0, 0.0, 70.0, 8.0, false);
        assert_eq!((both.shortcut, both.tabs), (Some(r(792.0, 0.0, 70.0, 38.0)), r(0.0, 0.0, 772.0, 38.0)));
        assert_eq!(tab_bar_rect(&l, true), Some(l.tabs));
        assert_eq!(tab_bar_rect(&l, false), None);
        // The window name: margin 6 before it, 14 after.
        let n = layout(1000.0, 38.0, 1.0, 75.0, 0.0, 60.0, 0.0, 8.0, false);
        assert_eq!((n.name, n.tabs), (Some(r(6.0, 0.0, 60.0, 38.0)), r(80.0, 0.0, 782.0, 38.0)));
        // Scaled: the buttons are 69 wide and the setting's difference from 75 scales too.
        let big = layout(1500.0, 57.0, 1.5, 75.0, 0.0, 0.0, 0.0, 12.0, false);
        assert_eq!(big.buttons[0], (Button::Minimize, r(1293.0, 0.0, 69.0, 57.0)));
        assert_eq!(big.tabs.w, 1293.0);
        assert_eq!(layout(1500.0, 57.0, 1.5, 100.0, 0.0, 0.0, 0.0, 12.0, false).tabs.w, 1500.0 - (207.0 + 37.5));
        // A setting smaller than the buttons lets the tabs overlap them.
        assert_eq!(layout(1000.0, 38.0, 1.0, 0.0, 0.0, 0.0, 0.0, 8.0, false).tabs.w, 1000.0 - 63.0);
    }

    #[test]
    fn caption_hit_test_follows_iterm2() {
        let l = layout(1000.0, 38.0, 1.0, 75.0, 0.0, 0.0, 0.0, 8.0, false);
        assert_eq!(hit(&l, 500.0, 3.0, false), Hit::Top);
        let max = layout(1000.0, 38.0, 1.0, 75.0, 0.0, 0.0, 0.0, 0.0, true);
        assert_eq!(hit(&max, 500.0, 3.0, false), Hit::Caption, "no resize edge when maximized");
        assert_eq!(hit(&l, 870.0, 20.0, false), Hit::Min);
        assert_eq!(hit(&l, 930.0, 20.0, false), Hit::Max);
        assert_eq!(hit(&l, 990.0, 20.0, false), Hit::Close);
        assert_eq!(hit(&l, 500.0, 20.0, true), Hit::Client, "over a tab");
        assert_eq!(hit(&l, 500.0, 20.0, false), Hit::Caption, "the empty bar");
        assert_eq!(hit(&l, 500.0, 100.0, false), Hit::Client, "below the bar");
        assert_eq!(hit(&l, 870.0, 3.0, false), Hit::Top, "the resize edge wins over a button");
    }

    #[test]
    fn caption_clicks_need_press_and_release_on_one_button() {
        assert_eq!(clicked(Some(Button::Close), Button::Close), Some(Button::Close));
        assert_eq!(clicked(Some(Button::Close), Button::Maximize), None);
        assert_eq!(clicked(None, Button::Close), None);
        assert_eq!((button_of(Hit::Min), button_of(Hit::Max), button_of(Hit::Close)), (Some(Button::Minimize), Some(Button::Maximize), Some(Button::Close)));
        assert_eq!((button_of(Hit::Caption), button_of(Hit::Top), button_of(Hit::Client)), (None, None, None));
    }

    #[test]
    fn caption_glyphs_are_segoe_fluent() {
        assert_eq!((MINIMIZE_GLYPH, MAXIMIZE_GLYPH, RESTORE_GLYPH, CLOSE_GLYPH), ('\u{E921}', '\u{E922}', '\u{E923}', '\u{E8BB}'));
    }
}
