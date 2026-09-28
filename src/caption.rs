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
/// The window name's metrics in points (`RTV:54-70`): margins, the widest and narrowest it gets,
/// and the alpha it draws at over the bar.
pub const NAME_LEFT: f32 = 6.0;
pub const NAME_RIGHT: f32 = 14.0;
pub const NAME_MAX: f32 = 180.0;
pub const NAME_MIN: f32 = 44.0;
pub const NAME_ALPHA: f32 = 0.55;

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

/// `ShowWindowNameBesideTabs` (`AS:706`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NameMode {
    Never,
    WhenCustom,
    Always,
}

/// The mode of the stored value; anything unknown is the default, When a custom name is set.
pub fn name_mode(v: i64) -> NameMode {
    match v {
        0 => NameMode::Never,
        2 => NameMode::Always,
        _ => NameMode::WhenCustom,
    }
}

/// The name shown beside the tabs (`rootTerminalViewWindowNameBesideTabs`, `PT:1203-1233`): None in
/// full screen, in a window that is not compact, when no top bar shows, and for Never; the custom
/// name for When custom; the custom name, else the automatic title, for Always. `custom` is the
/// evaluated Edit Window Title text, Some only while its format is set. An empty name shows nothing.
pub fn name_beside_tabs(mode: NameMode, compact_window: bool, full_screen: bool, top_bar_visible: bool, custom: Option<&str>, automatic: &str) -> Option<String> {
    if full_screen || !compact_window || !top_bar_visible {
        return None;
    }
    let name = match mode {
        NameMode::Never => return None,
        NameMode::WhenCustom => custom?,
        NameMode::Always => custom.unwrap_or(automatic),
    };
    (!name.is_empty()).then(|| name.to_string())
}

/// The name's width (`natural`, its text's measured width) within the `allowance` the tabs leave,
/// in one unit, at most `NAME_MAX` (`measuredWindowNameBesideTabsTextWidth`, `RTV:647-671`): a
/// name that fits shows whole, and one that must be cut is hidden (0) when the room is under
/// `NAME_MIN` (the minimum gates truncation, not slack).
pub fn name_width(natural: f32, allowance: f32) -> f32 {
    let allowance = allowance.min(NAME_MAX);
    if natural <= 0.0 || (natural > allowance && allowance < NAME_MIN) {
        return 0.0;
    }
    natural.min(allowance)
}

/// The room the tabs leave the name (`RTV:590-620`): the tab area's width less what the tabs
/// need and the name's two margins; pixels, never negative. The tabs need every tab's minimum
/// (`min_tab_w`), or on a bar that scrolls (`scrollable_tab_w`, `ScrollableTabWidth` in pixels)
/// at most one scrollable tab, since the rest are a scroll away (`minimumUsableCellAreaWidth`,
/// `PSM:3077-3090`).
pub fn name_allowance(tabs_w: f32, tab_count: usize, min_tab_w: f32, scrollable_tab_w: Option<f32>, scale: f32) -> f32 {
    let all = tab_count as f32 * min_tab_w;
    let need = scrollable_tab_w.map_or(all, |w| all.min(w));
    (tabs_w - need - (NAME_LEFT + NAME_RIGHT) * scale).max(0.0)
}

/// What a point of the strip is, for the pointer: the one answer the hit test (`hit`), a right
/// click (`strip_right_click` asks the bar only where this is not a button) and a double click
/// (`strip_double_click`) share.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StripPart {
    /// The resize border along the top.
    Top,
    Button(Button),
    /// The menu button.
    Menu,
    /// The window name: painted over the strip, its clicks are the strip's (`RTV:358-361`).
    Name,
    /// A tab, a chip, or a button of the tab bar (`over_item`).
    Item,
    /// The strip with nothing on it.
    Empty,
}

/// The part of the strip at the client point (`x`, `y`), None outside the strip. `over_item`:
/// the point is over a tab, a chip or a button of the tab bar.
pub fn strip_part(l: &Layout, x: f32, y: f32, over_item: bool) -> Option<StripPart> {
    if !l.bar.contains(x, y) {
        return None;
    }
    if !l.maximized && y < l.resize_top {
        return Some(StripPart::Top);
    }
    if l.menu.is_some_and(|m| m.contains(x, y)) {
        return Some(StripPart::Menu);
    }
    if let Some((b, _)) = l.buttons.iter().find(|(_, r)| r.contains(x, y)) {
        return Some(StripPart::Button(*b));
    }
    Some(if l.name.is_some_and(|n| n.contains(x, y)) {
        StripPart::Name
    } else if over_item {
        StripPart::Item
    } else {
        StripPart::Empty
    })
}

/// What a double click where Windows answers caption does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StripDoubleClick {
    /// Edit Window Title (`iTermTabBarDoubleClickAtPointInWindow:`, `RTV:2213-2219`).
    EditName,
    /// The title bar's own double click (maximize or restore).
    TitleBar,
    /// Nothing: over a tab in the edge strip, where the press was the window's drag.
    Nothing,
}

/// A double click at the client point (`x`, `y`): on the window name it edits the window title;
/// over a tab (`in_cell`, only in the edge strip) nothing happens, as iTerm2 runs the title bar's
/// double click only where `clickedInCell:` is false (`iTermTabBarControlView.m:418-433`; chips
/// and the bar's buttons are no cells, `PSM:4666-4692`); elsewhere it is the title bar's.
pub fn strip_double_click(l: &Layout, x: f32, y: f32, in_cell: bool) -> StripDoubleClick {
    match strip_part(l, x, y, in_cell) {
        Some(StripPart::Name) => StripDoubleClick::EditName,
        Some(StripPart::Item) => StripDoubleClick::Nothing,
        _ => StripDoubleClick::TitleBar,
    }
}

/// `text` cut to `room` pixels with a trailing ellipsis; empty when not even the ellipsis fits.
pub fn fit_name(text: &str, room: f32, measure: &mut dyn FnMut(&str) -> f32) -> String {
    if text.is_empty() || measure(text) <= room {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    for n in (0..chars.len()).rev() {
        let cut: String = chars[..n].iter().collect::<String>() + "\u{2026}";
        if measure(&cut) <= room {
            return cut;
        }
    }
    String::new()
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
    /// The caption menu asked for: Alt or F10 (the button's corner), `true`: at the pointer.
    Menu(bool),
    /// A right click where Windows answers caption, at this client point (`strip_right_click`).
    RightClick(i32, i32),
    /// A double click on the window name (`strip_double_click`).
    EditName,
}

/// The button a release clicks: the one pressed and let go over the same button.
pub fn clicked(pressed: Option<Button>, released: Button) -> Option<Button> {
    pressed.filter(|p| *p == released)
}

/// A left button message on a window button (`WM_NCLBUTTONDOWN`, `WM_NCLBUTTONUP`,
/// `WM_NCLBUTTONDBLCLK`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NcButton {
    Down,
    Up,
    DoubleClick,
}

/// What the app is told of a button message: a double click's second press is a press, so a
/// fast double click clicks twice, as on Windows' own buttons.
pub fn button_event(m: NcButton, b: Button) -> Event {
    match m {
        NcButton::Down | NcButton::DoubleClick => Event::Down(b),
        NcButton::Up => Event::Up(b),
    }
}

/// The button under the pointer as the caption subclass tracks it; the app draws what it is
/// told and never clears it on its own, so the two cannot disagree.
#[derive(Clone, Copy, Debug, Default)]
pub struct HoverTrack {
    hover: Option<Button>,
}

impl HoverTrack {
    /// The pointer is over `now`: the event to tell the app when that is a change.
    pub fn to(&mut self, now: Option<Button>) -> Option<Event> {
        (std::mem::replace(&mut self.hover, now) != now).then_some(Event::Hover(now))
    }

    /// The left button came up away from the buttons: no hover, and a held button is let go,
    /// which the app is always told.
    pub fn release_elsewhere(&mut self) -> Event {
        self.hover = None;
        Event::Hover(None)
    }
}

/// The outer rect `(left, top, right, bottom)` of a maximized caption window on a monitor whose
/// work area is `work`: the work area overhung by the frame `frame` on every side, so the client
/// the subclass leaves (the top overhang added back, the bottom frame kept) is the work area.
pub fn maximized_outer(work: (i32, i32, i32, i32), frame: i32) -> (i32, i32, i32, i32) {
    (work.0 - frame, work.1 - frame, work.2 + frame, work.3 + frame)
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

/// Virtual key codes the key menu watches: Alt (`VK_MENU`; `VK_LMENU` and `VK_RMENU` are Alt too)
/// and F10.
pub const VK_ALT: u16 = 0x12;
pub const VK_F10: u16 = 0x79;

fn is_alt(vk: u16) -> bool {
    matches!(vk, VK_ALT | 0xA4 | 0xA5)
}

/// A message the key menu tracker sees before winit: a key press (`WM_KEYDOWN`, or with `sys`
/// `WM_SYSKEYDOWN`; `repeat` is auto-repeat, lparam bit 30), a key release, a mouse press, or
/// the keyboard focus leaving.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyMsg {
    Down { vk: u16, repeat: bool, sys: bool },
    Up { vk: u16, sys: bool },
    Pointer,
    FocusLost,
}

/// Alt pressed and released alone, and F10, in a window whose menu bar muda detached (a Minimal or
/// Compact window): winit hands `WM_SYSKEYUP` to `DefWindowProc` only while a menu bar is attached
/// (`event_loop.rs:1773-1779`), so Windows never sends `SC_KEYMENU`, and the caption subclass
/// tracks the keys itself.
#[derive(Clone, Copy, Debug, Default)]
pub struct KeyMenu {
    /// Alt is down and nothing else happened since its press.
    alt_alone: bool,
}

impl KeyMenu {
    /// Takes one message (`shift_or_ctrl`: Shift or Ctrl is held); true when it opens the menu:
    /// the release of an Alt pressed alone, or an F10 press without Alt, Shift or Ctrl. Any
    /// other key, a click (Alt+drag) or a lost focus between cancels the Alt.
    pub fn feed(&mut self, m: KeyMsg, shift_or_ctrl: bool) -> bool {
        match m {
            KeyMsg::Down { vk, repeat, sys } if is_alt(vk) => {
                if !repeat {
                    self.alt_alone = sys && !shift_or_ctrl;
                }
                false
            }
            KeyMsg::Down { vk, repeat, sys } => {
                // F10 comes as a system key; with Alt held it is Alt+F10, a chord.
                let alt_held = std::mem::replace(&mut self.alt_alone, false);
                vk == VK_F10 && sys && !repeat && !shift_or_ctrl && !alt_held
            }
            KeyMsg::Up { vk, sys } if is_alt(vk) => std::mem::replace(&mut self.alt_alone, false) && sys,
            KeyMsg::Up { .. } => false,
            KeyMsg::Pointer | KeyMsg::FocusLost => {
                self.alt_alone = false;
                false
            }
        }
    }
}

/// The menu a right click in the strip gives where Windows answers caption (the empty strip, or
/// the edge strip over the bar), from what the tab bar has at the point.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StripMenu {
    /// The tab's menu (over a tab or its close button).
    Tab(usize),
    /// The group's menu (over a group chip).
    Group,
    /// The whole menu at the pointer.
    Main,
}

/// `PSM menuForEvent:` (`PSM:3961-3985`): the cell's menu first, then the chip's; elsewhere the
/// whole menu, as iTerm2 does where its menu bar is out of reach.
pub fn strip_right_click(hit: Option<crate::tab_layout::Hit>) -> StripMenu {
    use crate::tab_layout::Hit as TabHit;
    match hit {
        Some(TabHit::Tab(i) | TabHit::Close(i)) => StripMenu::Tab(i),
        Some(TabHit::Chip(_)) => StripMenu::Group,
        _ => StripMenu::Main,
    }
}

/// The client size to hand `ScaleFactorChanged`'s `inner_size_writer` when a caption window moves
/// to a monitor of another DPI: winit's own answer (the old client, `old`, scaled from `old_dpi`
/// to `new_dpi` and rounded, `event_loop.rs:2273-2280`) less the height the subclass removes at
/// the new DPI, since winit then frames it for a style that still has its title bar.
pub fn dpi_inner(old: (u32, u32), old_dpi: u32, new_dpi: u32, removed: i32) -> (u32, u32) {
    let scale = |v: u32| (v as f64 / (old_dpi.max(1) as f64 / 96.0) * (new_dpi as f64 / 96.0)).round() as u32;
    (scale(old.0), corrected_inner(scale(old.1), removed))
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

/// Whether a converted drag moves the window (`performWindowDragWithEvent:` asks
/// `ptyWindowIsDraggable:`, `iTermWindowImpl.m:163-167`, `PT:2106-2135`): a window type that can
/// be dragged (not Centered, not docked to a screen edge) and not in full screen. Elsewhere the
/// drag still converts and moves nothing (`PSM:3788-3794`).
pub fn window_movable(style: i64, full_screen: bool) -> bool {
    crate::frames::draggable(style) && !full_screen
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

/// The answer for the client point (`x`, `y`) (`strip_part`): the top resize border first, then
/// the menu button (client) and the window buttons, then the strip (`over_tab`: the point is over
/// a tab or a button of the tab bar, which stay client). The name and the empty strip drag.
pub fn hit(l: &Layout, x: f32, y: f32, over_tab: bool) -> Hit {
    let in_drag_region = l.drag_regions.iter().any(|r| r.contains(x, y));
    match strip_part(l, x, y, over_tab) {
        // A bottom bar's edge strip lies outside the top strip.
        None if in_drag_region => Hit::Caption,
        None | Some(StripPart::Menu) => Hit::Client,
        Some(StripPart::Top) => Hit::Top,
        Some(StripPart::Button(Button::Minimize)) => Hit::Min,
        Some(StripPart::Button(Button::Maximize)) => Hit::Max,
        Some(StripPart::Button(Button::Close)) => Hit::Close,
        // The edge strip moves the window even over a tab (the bar takes no mouse downs there).
        Some(StripPart::Item) if !in_drag_region => Hit::Client,
        Some(StripPart::Item | StripPart::Name | StripPart::Empty) => Hit::Caption,
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

    /// winit sends `ScaleFactorChanged` inside `WM_DPICHANGED` before it resizes, with the old
    /// client scaled to the new DPI as the size it will set (`event_loop.rs:2273-2291`); the
    /// writer gets that less the title bar the subclass removes at the new DPI.
    #[test]
    fn caption_dpi_change_keeps_the_client() {
        assert_eq!(dpi_inner((800, 562), 96, 144, 45), (1200, 798));
        assert_eq!(dpi_inner((1200, 843), 144, 96, 31), (800, 531));
        // winit rounds the scaled size: 801 * 1.25 = 1001.25, 563 * 1.25 = 703.75.
        assert_eq!(dpi_inner((801, 563), 96, 120, 0), (1001, 704));
        assert_eq!(dpi_inner((10, 10), 96, 96, 40), (10, 1), "never below 1");
    }

    /// Alt pressed and released with nothing between, and F10, open the menu (what Windows'
    /// `DefWindowProc` does for a window with a menu bar); a chord, a click or a lost focus
    /// between the Alt press and release does not.
    #[test]
    fn caption_key_menu_tracks_alt_alone() {
        use KeyMsg::{Down, FocusLost, Pointer, Up};
        let alt_down = |repeat| Down { vk: VK_ALT, repeat, sys: true };
        let alt_up = Up { vk: VK_ALT, sys: true };
        let mut k = KeyMenu::default();
        assert!(!k.feed(alt_down(false), false));
        assert!(!k.feed(alt_down(true), false), "auto-repeat keeps it");
        assert!(k.feed(alt_up, false), "Alt alone");
        assert!(!k.feed(alt_up, false), "an Alt release with no press before it");
        // Alt+Space (the system menu) and Alt+F (a chord) keep working: no menu at the release.
        for vk in [0x20, 0x46] {
            k.feed(alt_down(false), false);
            assert!(!k.feed(Down { vk, repeat: false, sys: true }, false));
            assert!(!k.feed(Up { vk, sys: true }, false));
            assert!(!k.feed(alt_up, false), "a chord with {vk:#x}");
        }
        // Ctrl+Alt reaches the window as a plain key down.
        k.feed(alt_down(false), false);
        k.feed(Down { vk: 0x11, repeat: false, sys: false }, false);
        assert!(!k.feed(alt_up, false));
        k.feed(alt_down(false), false);
        k.feed(Pointer, false);
        assert!(!k.feed(alt_up, false), "a click between (Alt+drag)");
        k.feed(alt_down(false), false);
        k.feed(FocusLost, false);
        assert!(!k.feed(alt_up, false), "Alt+Tab");
        k.feed(alt_down(false), true);
        assert!(!k.feed(alt_up, false), "Shift+Alt switches the input language");
        // The left and right Alt keys are Alt too.
        k.feed(Down { vk: 0xA5, repeat: false, sys: true }, false);
        assert!(k.feed(Up { vk: 0xA5, sys: true }, false));
        // F10 opens it on its press, once; Shift+F10 and Ctrl+F10 do not.
        assert!(k.feed(Down { vk: VK_F10, repeat: false, sys: true }, false));
        assert!(!k.feed(Down { vk: VK_F10, repeat: true, sys: true }, false));
        assert!(!k.feed(Up { vk: VK_F10, sys: true }, false));
        assert!(!k.feed(Down { vk: VK_F10, repeat: false, sys: true }, true));
        // Alt held, then F10: Alt+F10 is a chord.
        k.feed(alt_down(false), false);
        assert!(!k.feed(Down { vk: VK_F10, repeat: false, sys: true }, false));
        assert!(!k.feed(alt_up, false));
    }

    /// A right click in the strip that Windows answers as caption (the edge strip over a tab or
    /// a chip, or the empty strip): the tab's menu over a tab or its close button, the group's
    /// over a chip, the whole menu elsewhere (`PSM menuForEvent:`, `PSM:3961-3985`).
    #[test]
    fn caption_right_click_follows_the_bar() {
        use crate::tab_layout::Hit as TabHit;
        assert_eq!(strip_right_click(Some(TabHit::Tab(2))), StripMenu::Tab(2));
        assert_eq!(strip_right_click(Some(TabHit::Close(1))), StripMenu::Tab(1));
        assert_eq!(strip_right_click(Some(TabHit::Chip(0))), StripMenu::Group);
        for h in [Some(TabHit::Bar), Some(TabHit::Add), Some(TabHit::Overflow), None] {
            assert_eq!(strip_right_click(h), StripMenu::Main, "{h:?}");
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
        // Collapsed group members are tabs (numberOfVisibleTabs, PSM:4716-4731); only overflow stops it.
        assert_eq!(visible_tab_count(3, None), 3);
        assert_eq!(visible_tab_count(5, Some(3)), 3);
        assert_eq!(visible_tab_count(0, None), 0);
        // One open tab and a collapsed group of two is three tabs, not solitary.
        assert!(!drag_to_window(false, true, false, visible_tab_count(3, None), false, true));
    }

    /// A converted drag moves the window only where `ptyWindowIsDraggable:` answers yes
    /// (`iTermWindowImpl.m:163-167`, `PT:2106-2135`): never a Centered or screen-edge window, nor
    /// one in full screen; the conversion still swallows the drag there (`PSM:3788-3794`).
    #[test]
    fn window_drag_needs_a_draggable_window() {
        use crate::frames::{ACCESSORY, CENTERED, CENTERED_NO_TITLE_BAR, COMPACT, COMPACT_CENTERED, COMPACT_MAXIMIZED, MAXIMIZED, NORMAL, NO_TITLE_BAR, TOP_PERCENT};
        for style in [NORMAL, MAXIMIZED, NO_TITLE_BAR, ACCESSORY, COMPACT, COMPACT_MAXIMIZED] {
            assert!(window_movable(style, false), "{style}");
            assert!(!window_movable(style, true), "{style} in full screen");
        }
        for style in [CENTERED, CENTERED_NO_TITLE_BAR, COMPACT_CENTERED, TOP_PERCENT] {
            assert!(!window_movable(style, false), "{style}");
        }
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
    }

    /// A button's non-client messages as told to the app: the second click of a double click is
    /// a press, so a fast double click on Maximize toggles twice as Windows' own button does.
    #[test]
    fn caption_double_click_on_a_button_is_a_press() {
        assert_eq!(button_event(NcButton::Down, Button::Maximize), Event::Down(Button::Maximize));
        assert_eq!(button_event(NcButton::DoubleClick, Button::Maximize), Event::Down(Button::Maximize));
        assert_eq!(button_event(NcButton::Up, Button::Close), Event::Up(Button::Close));
        // Down, up, down (the double click), up: two clicks.
        let mut pressed = None;
        let mut clicks = 0;
        for m in [NcButton::Down, NcButton::Up, NcButton::DoubleClick, NcButton::Up] {
            match button_event(m, Button::Maximize) {
                Event::Down(b) => pressed = Some(b),
                Event::Up(b) => clicks += usize::from(clicked(pressed.take(), b).is_some()),
                _ => {}
            }
        }
        assert_eq!(clicks, 2);
    }

    /// The subclass owns the hover: the app hears each change once, a release away from the
    /// buttons lets go of it, and the button the pointer comes back to lights again.
    #[test]
    fn caption_hover_is_told_once_per_change() {
        let mut h = HoverTrack::default();
        assert_eq!(h.to(Some(Button::Maximize)), Some(Event::Hover(Some(Button::Maximize))));
        assert_eq!(h.to(Some(Button::Maximize)), None, "no change, nothing told");
        assert_eq!(h.release_elsewhere(), Event::Hover(None));
        assert_eq!(h.to(Some(Button::Maximize)), Some(Event::Hover(Some(Button::Maximize))), "lit again after the release");
        assert_eq!(h.to(None), Some(Event::Hover(None)));
        assert_eq!(h.to(None), None);
        assert_eq!(h.release_elsewhere(), Event::Hover(None), "a release always lets go of a held button");
    }

    /// winit keeps a maximized window's client across a DPI change and frames it for a style
    /// with a title bar (`event_loop.rs:2270-2280`, `allow_resize` false), so the subclass fits it
    /// back to the monitor's work area overhung by the frame; writing a smaller size through
    /// winit instead would un-maximize it (`MAXIMIZED` unset, `SW_RESTORE`).
    #[test]
    fn caption_maximized_dpi_change_fits_the_monitor() {
        assert_eq!(maximized_outer((0, 0, 1920, 1040), 8), (-8, -8, 1928, 1048));
        assert_eq!(maximized_outer((1920, 0, 4480, 1400), 12), (1908, -12, 4492, 1412));
        // The subclass's client (the top overhang added back, the bottom frame kept) is the work area.
        let (_, t, _, b) = maximized_outer((0, 40, 1920, 1080), 8);
        assert_eq!((t + 8, b - 8), (40, 1080));
    }

    #[test]
    fn caption_glyphs_are_segoe_fluent() {
        assert_eq!((MINIMIZE_GLYPH, MAXIMIZE_GLYPH, RESTORE_GLYPH, CLOSE_GLYPH), ('\u{E921}', '\u{E922}', '\u{E923}', '\u{E8BB}'));
    }

    #[test]
    fn window_name_beside_tabs_follows_iterm2() {
        assert_eq!(name_mode(0), NameMode::Never);
        assert_eq!(name_mode(1), NameMode::WhenCustom);
        assert_eq!(name_mode(2), NameMode::Always);
        assert_eq!(name_mode(7), NameMode::WhenCustom);
        assert_eq!(name_mode(-1), NameMode::WhenCustom);
        let s = |x: &str| Some(x.to_string());
        assert_eq!(name_beside_tabs(NameMode::WhenCustom, true, false, true, Some("build"), "zsh"), s("build"));
        assert_eq!(name_beside_tabs(NameMode::WhenCustom, true, false, true, None, "zsh"), None);
        assert_eq!(name_beside_tabs(NameMode::Always, true, false, true, None, "zsh"), s("zsh"));
        assert_eq!(name_beside_tabs(NameMode::Always, true, false, true, Some("b"), "zsh"), s("b"));
        assert_eq!(name_beside_tabs(NameMode::Never, true, false, true, Some("b"), "zsh"), None);
        assert_eq!(name_beside_tabs(NameMode::Always, false, false, true, Some("b"), "zsh"), None, "a window with a title bar");
        assert_eq!(name_beside_tabs(NameMode::Always, true, true, true, Some("b"), "zsh"), None, "full screen");
        assert_eq!(name_beside_tabs(NameMode::Always, true, false, false, Some("b"), "zsh"), None, "the title already fills the bar");
        assert_eq!(name_beside_tabs(NameMode::Always, true, false, true, None, ""), None, "an empty automatic title shows nothing");
        assert_eq!((NAME_LEFT, NAME_RIGHT, NAME_MAX, NAME_MIN, NAME_ALPHA), (6.0, 14.0, 180.0, 44.0, 0.55));
        assert_eq!(name_width(60.0, 180.0), 60.0);
        assert_eq!(name_width(300.0, 180.0), 180.0);
        assert_eq!(name_width(300.0, 100.0), 100.0);
        assert_eq!(name_width(60.0, 30.0), 0.0);
        assert_eq!(name_width(0.0, 100.0), 0.0);
        assert_eq!(name_width(300.0, 44.0), 44.0);
        // The minimum gates truncation, not slack (`RTV:661-670`): a name that fits whole shows.
        assert_eq!(name_width(30.0, 40.0), 30.0);
        assert_eq!(name_width(44.0, 44.0), 44.0);
        assert_eq!(name_width(41.0, 40.0), 0.0, "cut below the minimum: hidden");
        assert_eq!(name_width(30.0, 0.0), 0.0);
    }

    #[test]
    fn window_name_allowance_follows_the_tabs() {
        // 800 wide, 3 tabs of at least 75, margins 6 + 14 at scale 1: 800 - 225 - 20.
        assert_eq!(name_allowance(800.0, 3, 75.0, None, 1.0), 555.0);
        // Scaled by 2: the margins and the tabs' minimum are pixels already for the minimum.
        assert_eq!(name_allowance(1600.0, 3, 150.0, None, 2.0), 1600.0 - 450.0 - 40.0);
        // The tabs take everything: nothing is left, never negative.
        assert_eq!(name_allowance(100.0, 3, 75.0, None, 1.0), 0.0);
        // A scrollable bar needs one scrollable tab at most, not every tab (`PSM:3085-3090`).
        assert_eq!(name_allowance(800.0, 10, 75.0, Some(120.0), 1.0), 800.0 - 120.0 - 20.0);
        assert_eq!(name_allowance(800.0, 10, 75.0, None, 1.0), 30.0, "a bar that does not scroll counts them all");
        assert_eq!(name_allowance(800.0, 1, 75.0, Some(120.0), 1.0), 800.0 - 75.0 - 20.0, "never more than all of them");
    }

    /// The name is painted over the strip and its clicks belong to the bar underneath
    /// (`RTV:358-361`): a press there drags the window like the empty strip, and only a double
    /// click on it edits the window title (`RTV:2213-2219`, `iTermTabBarControlView.m:418-433`);
    /// a double click elsewhere on the strip stays the title bar's.
    #[test]
    fn window_name_drags_and_edits_on_double_click() {
        let mut l = layout(1000.0, 38.0, 1.0, 75.0, 0.0, 60.0, 0.0, 8.0, false);
        l.menu = None;
        let n = l.name.unwrap();
        assert_eq!(strip_part(&l, n.x + 5.0, 20.0, false), Some(StripPart::Name));
        assert_eq!(hit(&l, n.x + 5.0, 20.0, false), Hit::Caption, "a press on the name drags the window");
        assert_eq!(hit(&l, n.x + 5.0, 20.0, true), Hit::Caption, "whatever the client rects say");
        assert_eq!(strip_double_click(&l, n.x + 5.0, 20.0, false), StripDoubleClick::EditName);
        assert_eq!(strip_double_click(&l, n.x + 5.0, 10.0, false), StripDoubleClick::EditName, "in the edge strip too");
        assert_eq!(strip_double_click(&l, n.x + n.w + 1.0, 20.0, false), StripDoubleClick::TitleBar);
        assert_eq!(strip_double_click(&l, 500.0, 20.0, false), StripDoubleClick::TitleBar);
        assert_eq!(strip_double_click(&l, n.x + 5.0, 3.0, false), StripDoubleClick::TitleBar, "the resize edge above it");
        let none = layout(1000.0, 38.0, 1.0, 75.0, 0.0, 0.0, 0.0, 8.0, false);
        assert_eq!(strip_double_click(&none, 10.0, 20.0, false), StripDoubleClick::TitleBar);
        // Over a tab (the edge strip) the title bar's double click does not run (`clickedInCell:`,
        // `iTermTabBarControlView.m:418-421`); the press there was the window's drag.
        assert_eq!(strip_double_click(&l, 500.0, 10.0, true), StripDoubleClick::Nothing);
        assert_eq!(strip_double_click(&l, n.x + 5.0, 10.0, true), StripDoubleClick::EditName, "the name is painted over the bar");
        // One classification for the rest of the strip.
        assert_eq!(strip_part(&l, 500.0, 20.0, true), Some(StripPart::Item));
        assert_eq!(strip_part(&l, 500.0, 20.0, false), Some(StripPart::Empty));
        assert_eq!(strip_part(&l, 990.0, 20.0, false), Some(StripPart::Button(Button::Close)));
        assert_eq!(strip_part(&l, 500.0, 3.0, true), Some(StripPart::Top));
        assert_eq!(strip_part(&l, 500.0, 100.0, true), None, "below the strip");
    }

    #[test]
    fn window_name_truncates_with_an_ellipsis() {
        let mut m = |t: &str| t.chars().count() as f32 * 10.0;
        assert_eq!(fit_name("build", 50.0, &mut m), "build");
        assert_eq!(fit_name("build-all", 50.0, &mut m), "buil\u{2026}");
        assert_eq!(fit_name("build-all", 10.0, &mut m), "\u{2026}");
        assert_eq!(fit_name("build-all", 5.0, &mut m), "");
        assert_eq!(fit_name("", 50.0, &mut m), "");
    }
}
