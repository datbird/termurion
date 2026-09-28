//! The caption area of a Minimal or Compact window (milestone 13e): the tab bar sits in the strip
//! where the title bar was, with the window buttons at its right (Windows draws them nowhere
//! else, so Tandem does). Pure geometry: the strip, its buttons, the room the tabs get, and what
//! a point of the client answers to `WM_NCHITTEST`. Sizes are pixels.

pub use crate::layout::Rect;
use crate::tab_position::TabPosition;
use crate::tab_theme::TabStyle;

/// One window button's width in points (Windows 11).
pub const BUTTON_W: f32 = 46.0;
/// `CompactTabBarStoplightButtonsWidth`'s default: the width iTerm2 reserves for its stoplights.
const REFERENCE_SETTING: f32 = 75.0;
/// The window name's margins in points (`RTV:54-70`).
const NAME_LEFT: f32 = 6.0;
const NAME_RIGHT: f32 = 14.0;

/// The caption's menu button in points: the native menu bar cannot sit under the strip, so the
/// button pops the whole menu instead.
pub const MENU_W: f32 = 40.0;
/// Segoe Fluent Icons' "GlobalNavigationButton" (the hamburger).
pub const MENU_GLYPH: char = '\u{E700}';

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
    /// The menu button at the strip's left; the tabs start after it (None until `with_menu_button`).
    pub menu: Option<Rect>,
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
    /// The caption menu asked for: Alt or F10 (the button's corner), a right click on the strip
    /// (`true`: at the pointer).
    Menu(bool),
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

/// The strip with the menu button at its left: the name and the tabs move right of it.
pub fn with_menu_button(mut l: Layout, scale: f32) -> Layout {
    let w = MENU_W * scale;
    l.menu = Some(Rect { x: 0.0, y: 0.0, w, h: l.bar.h });
    if let Some(n) = &mut l.name {
        n.x += w;
    }
    l.tabs = Rect { x: l.tabs.x + w, w: (l.tabs.w - w).max(0.0), ..l.tabs };
    l
}

/// Whether a `WM_SYSCOMMAND` is the keyboard menu (Alt pressed and released alone, F10):
/// `SC_KEYMENU` with `lparam` 0. Alt+Space and mnemonics carry a character and are not.
pub fn is_key_menu(wparam: usize, lparam: isize) -> bool {
    wparam & 0xFFF0 == 0xF100 && lparam == 0
}

/// The height a caption subclass takes off a window winit sized for a style with its title bar:
/// a request for `height` client pixels asks winit for that less `removed`.
pub fn corrected_inner(height: u32, removed: i32) -> u32 {
    height.saturating_sub(removed.max(0) as u32).max(1)
}

/// The outer height that keeps the client height `before` after a frame change left it `after`.
pub fn kept_outer_height(outer: i32, before: i32, after: i32) -> i32 {
    outer - (after - before)
}

/// The top resize border in pixels: the frame thickness, 0 when maximized or when the window's
/// size is locked (Lock Size, or `AllowLiveResize` off), as the other edges.
pub fn resize_top(thickness: f32, maximized: bool, resizable: bool) -> f32 {
    if maximized || !resizable { 0.0 } else { thickness }
}

/// The tabs `numberOfVisibleTabs` counts (`PSM:4716-4731`): every tab up to the first in the
/// overflow menu, collapsed group members included (chips are not tabs).
pub fn visible_tab_count(total: usize, first_overflow: Option<usize>) -> usize {
    first_overflow.map_or(total, |i| i.min(total))
}

/// When the pointer's drag is judged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DragPhase {
    Press,
    Move,
}

/// Whether a drag on the tab bar moves the window: never at the press (Alt+click still selects),
/// never for a group chip (`PSM:3766-3795`), else `drag_to_window`.
#[allow(clippy::too_many_arguments)]
pub fn window_drag(phase: DragPhase, chip: bool, alt: bool, compact_theme: bool, no_title_bar: bool, visible_tabs: usize, in_strip: bool, convert_solitary: bool) -> bool {
    phase == DragPhase::Move && !chip && drag_to_window(alt, compact_theme, no_title_bar, visible_tabs, in_strip, convert_solitary)
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
        menu: None,
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
    let in_drag_region = l.drag_regions.iter().any(|r| r.contains(x, y));
    if !l.bar.contains(x, y) {
        // A bottom bar's edge strip lies outside the top strip.
        return if in_drag_region { Hit::Caption } else { Hit::Client };
    }
    if !l.maximized && y < l.resize_top {
        return Hit::Top;
    }
    if l.menu.is_some_and(|m| m.contains(x, y)) {
        return Hit::Client;
    }
    if let Some((b, _)) = l.buttons.iter().find(|(_, r)| r.contains(x, y)) {
        return match b {
            Button::Minimize => Hit::Min,
            Button::Maximize => Hit::Max,
            Button::Close => Hit::Close,
        };
    }
    // The edge strip moves the window even over a tab (the bar takes no mouse downs there).
    if over_tab && !in_drag_region {
        Hit::Client
    } else {
        Hit::Caption
    }
}

/// The size in pixels of the edge strip that drags the window (`PT:10124-10138`): the setting of
/// the Minimal or Compact theme, 0 in other themes and when the window cannot be dragged (full
/// screen or maximized). `minimal` and `compact` are `MinimalEdgeDragSize` and
/// `CompactEdgeDragSize`.
pub fn edge_strip(style: TabStyle, minimal: f32, compact: f32, draggable: bool) -> f32 {
    if !draggable {
        return 0.0;
    }
    match style {
        TabStyle::Minimal => minimal,
        TabStyle::Compact => compact,
        _ => 0.0,
    }
}

/// Whether the point (`x`, `y`) is in the bar's edge strip: the top `strip` of a top bar or the
/// bottom `strip` of a bottom bar, never a side bar's.
pub fn in_edge_strip(bar: Rect, position: TabPosition, strip: f32, x: f32, y: f32) -> bool {
    if strip <= 0.0 || x < bar.x || x >= bar.x + bar.w {
        return false;
    }
    match position {
        TabPosition::Top => y >= bar.y && y < bar.y + strip.min(bar.h),
        TabPosition::Bottom => y < bar.y + bar.h && y >= bar.y + bar.h - strip.min(bar.h),
        TabPosition::Left | TabPosition::Right => false,
    }
}

/// Whether a press that starts a drag on the tab bar drags the window instead
/// (`tabViewShouldDragWindow:event:`, `PT:10222-10256`): always with Alt (Option); else only in a
/// Minimal or Compact theme or a No Title Bar window; with several visible tabs only from the
/// edge strip, with a solitary tab when `convert_solitary` is on.
pub fn drag_to_window(alt: bool, compact_theme: bool, no_title_bar: bool, visible_tabs: usize, in_strip: bool, convert_solitary: bool) -> bool {
    if alt {
        return true;
    }
    if !compact_theme && !no_title_bar {
        return false;
    }
    if visible_tabs > 1 {
        in_strip
    } else {
        convert_solitary
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
        assert_eq!((l.drag_edge, l.drag_regions.len()), (0.0, 0), "the app fills the drag edge in");
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
    fn window_drag_follows_iterm2() {
        use crate::tab_position::TabPosition::{Bottom, Left, Right, Top};
        use crate::tab_theme::TabStyle::{Automatic, Compact, Minimal};
        assert_eq!(edge_strip(Minimal, 12.0, 10.0, true), 12.0);
        assert_eq!(edge_strip(Compact, 12.0, 10.0, true), 10.0);
        assert_eq!(edge_strip(Automatic, 12.0, 10.0, true), 0.0);
        assert_eq!(edge_strip(Minimal, 12.0, 10.0, false), 0.0);
        let top = r(0.0, 0.0, 800.0, 38.0);
        assert!(in_edge_strip(top, Top, 12.0, 100.0, 11.0));
        assert!(!in_edge_strip(top, Top, 12.0, 100.0, 12.0));
        let bottom = r(0.0, 562.0, 800.0, 38.0);
        assert!(in_edge_strip(bottom, Bottom, 12.0, 100.0, 590.0));
        assert!(!in_edge_strip(bottom, Bottom, 12.0, 100.0, 587.0));
        assert!(!in_edge_strip(r(0.0, 0.0, 120.0, 600.0), Left, 12.0, 5.0, 5.0));
        assert!(!in_edge_strip(r(680.0, 0.0, 120.0, 600.0), Right, 12.0, 790.0, 5.0));
        assert!(!in_edge_strip(top, Top, 0.0, 100.0, 0.0), "no strip, no region");
        assert!(drag_to_window(true, false, false, 3, false, true), "Alt always");
        assert!(!drag_to_window(false, false, false, 1, false, true), "a regular theme never converts");
        assert!(drag_to_window(false, false, true, 1, false, true), "a No Title Bar window converts in any theme");
        assert!(drag_to_window(false, true, false, 3, true, true), "the edge strip with several tabs");
        assert!(!drag_to_window(false, true, false, 3, false, true));
        assert!(drag_to_window(false, true, false, 1, false, true), "a solitary tab");
        assert!(!drag_to_window(false, true, false, 1, false, false), "the key off");
    }

    #[test]
    fn caption_drag_edge_answers_caption() {
        let mut l = layout(1000.0, 38.0, 1.0, 75.0, 0.0, 0.0, 0.0, 8.0, false);
        l.drag_edge = 12.0;
        l.drag_regions = vec![r(0.0, 0.0, 862.0, 12.0)];
        assert_eq!(hit(&l, 500.0, 10.0, true), Hit::Caption, "over a tab, in the strip");
        assert_eq!(hit(&l, 500.0, 20.0, true), Hit::Client, "over a tab below the strip");
        assert_eq!(hit(&l, 500.0, 3.0, true), Hit::Top, "the resize border wins");
        assert_eq!(hit(&l, 990.0, 10.0, true), Hit::Close, "a button wins");
        // A region outside the strip (a bottom bar's).
        l.drag_regions = vec![r(0.0, 588.0, 800.0, 12.0)];
        assert_eq!(hit(&l, 100.0, 590.0, true), Hit::Caption);
        assert_eq!(hit(&l, 100.0, 570.0, true), Hit::Client);
    }

    /// winit sizes a window's outer frame for a style that still has its title bar, which the
    /// caption subclass then removes: every request is short by that height.
    #[test]
    fn caption_size_correction_follows_frame() {
        assert_eq!(corrected_inner(600, 38), 562);
        assert_eq!(corrected_inner(600, 0), 600, "a window without the subclass (or in full screen)");
        assert_eq!(corrected_inner(10, 38), 1, "never below 1");
        assert_eq!(corrected_inner(600, -5), 600);
    }

    /// A window's client height under a frame change: Regular (caption, menu bar) to Minimal
    /// (neither) and back, and full screen, never loses a caption or menu bar height.
    #[test]
    fn caption_round_trips_keep_the_client() {
        struct Win {
            outer: i32,
            caption: bool,
            menu: bool,
        }
        impl Win {
            fn client(&self) -> i32 {
                self.outer - 8 - if self.caption { 31 } else { 0 } - if self.menu { 20 } else { 0 }
            }
            // The frame changes; the outer height follows `kept_outer_height`.
            fn change(&mut self, f: impl FnOnce(&mut Win)) {
                let before = self.client();
                f(self);
                self.outer = kept_outer_height(self.outer, before, self.client());
            }
        }
        let mut w = Win { outer: 600, caption: true, menu: true };
        let c = w.client();
        for _ in 0..3 {
            w.change(|w| {
                w.menu = false;
                w.caption = false;
            });
            assert_eq!(w.client(), c, "Regular to Minimal");
            assert_eq!(w.outer, 600 - 31 - 20);
            w.change(|w| {
                w.caption = true;
                w.menu = true;
            });
            assert_eq!(w.client(), c, "and back");
            assert_eq!(w.outer, 600);
        }
    }

    #[test]
    fn caption_top_edge_follows_lock_size() {
        assert_eq!(resize_top(8.0, false, true), 8.0);
        assert_eq!(resize_top(8.0, false, false), 0.0, "Lock Size or AllowLiveResize off: no top resize");
        assert_eq!(resize_top(8.0, true, true), 0.0, "maximized");
        let mut l = layout(1000.0, 38.0, 1.0, 75.0, 0.0, 0.0, 0.0, resize_top(8.0, false, false), false);
        assert_eq!(hit(&l, 500.0, 3.0, false), Hit::Caption, "never HTTOP when locked");
        l.resize_top = resize_top(8.0, false, true);
        assert_eq!(hit(&l, 500.0, 3.0, false), Hit::Top);
    }

    #[test]
    fn caption_menu_button_follows_ruling() {
        let base = layout(1000.0, 38.0, 1.0, 75.0, 0.0, 0.0, 0.0, 8.0, false);
        assert_eq!(base.menu, None);
        let l = with_menu_button(base.clone(), 1.0);
        assert_eq!(l.menu, Some(r(0.0, 0.0, MENU_W, 38.0)));
        assert_eq!(l.tabs, r(MENU_W, 0.0, 862.0 - MENU_W, 38.0), "the tabs start after the button");
        assert_eq!(hit(&l, 10.0, 20.0, false), Hit::Client, "the button is a client point");
        assert_eq!(hit(&l, 500.0, 20.0, false), Hit::Caption);
        assert_eq!(with_menu_button(base, 1.5).menu, Some(r(0.0, 0.0, MENU_W * 1.5, 38.0)));
        // The Alt/F10 key menu is a keyboard SC_KEYMENU (lparam 0), not Alt+Space or a mnemonic.
        assert!(is_key_menu(0xF100, 0));
        assert!(!is_key_menu(0xF100, i32::from(b' ') as isize), "Alt+Space is the system menu");
        assert!(!is_key_menu(0xF010, 0));
    }

    #[test]
    fn tab_drag_to_window_follows_iterm2() {
        // Alt converts on DRAG only (PSM:3790): a press never does, so Alt+click still selects.
        assert!(!window_drag(DragPhase::Press, false, true, false, false, 3, false, true));
        assert!(window_drag(DragPhase::Move, false, true, false, false, 3, false, true));
        // A group chip's drag never converts, in the strip or with Alt.
        assert!(!window_drag(DragPhase::Move, true, true, true, false, 1, true, true));
        // Collapsed group members are tabs (numberOfVisibleTabs, PSM:4716-4731); only overflow stops it.
        assert_eq!(visible_tab_count(3, None), 3);
        assert_eq!(visible_tab_count(5, Some(3)), 3);
        assert_eq!(visible_tab_count(0, None), 0);
        // One open tab and a collapsed group of two is three tabs, not solitary.
        assert!(!window_drag(DragPhase::Move, false, false, true, false, visible_tab_count(3, None), false, true));
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
