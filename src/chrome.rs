//! Window chrome rules: the border around a window (when it shows, its color), the corner
//! preference, the line under the title bar, the snap to the cell grid while resizing and
//! whether programs may resize the window, the window's size after a font size change, the
//! Zoom frame and the profile's size units, as iTerm2 decides them.

use crate::layout::Rect;
use crate::palette::Rgb;
use crate::tab_theme::TabStyle;

/// A color with its opacity, components 0 to 255 and alpha 0 to 1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: f32,
}

/// DWM's "no color of mine" value (`DWMWA_COLOR_DEFAULT`): the system draws its own border.
pub const COLOR_DEFAULT: u32 = 0xFFFF_FFFF;

/// The border color a setting names (`iTermWindowBorderColorFromSetting`, `RTV:1023-1050`):
/// "#rrggbb", or "#rrggbbaa" with its own alpha, either after "p3#" for Display P3; without an
/// alpha the border is at 0.75. The color part is read as `getHashColorRed:` reads it, so
/// "#rgb" and the 16-bit "#rrrrggggbbbb" work too, and a value without "#" is no color.
pub fn parse_border_color(s: &str) -> Option<Rgba> {
    let (p3, hex) = match (s.strip_prefix("p3#"), s.strip_prefix('#')) {
        (Some(h), _) => (true, h),
        (None, Some(h)) => (false, h),
        // iTerm2 passes the bare text on to a parser that needs the "#".
        (None, None) => return None,
    };
    let (hex, a) = if hex.len() == 8 {
        let alpha = hex.get(6..).filter(|a| a.bytes().all(|b| b.is_ascii_hexdigit()))?;
        (hex.get(..6)?, f32::from(u8::from_str_radix(alpha, 16).ok()?) / 255.0)
    } else {
        (hex, 0.75)
    };
    let (r, g, b) = hash_color(hex)?;
    let (r, g, b) = if p3 { crate::import::p3_to_srgb(r, g, b) } else { (r, g, b) };
    let byte = |c: f64| (c.clamp(0.0, 1.0) * 255.0).round() as u8;
    Some(Rgba { r: byte(r), g: byte(g), b: byte(b), a })
}

/// The components of "rgb", "rrggbb" or "rrrrggggbbbb" (after the "#"), 0 to 1.
fn hash_color(hex: &str) -> Option<(f64, f64, f64)> {
    if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let part = |i: usize, n: usize| u32::from_str_radix(&hex[i * n..(i + 1) * n], 16).ok();
    let (max, n) = match hex.len() {
        3 => (15.0, 1),
        6 => (255.0, 2),
        12 => (65535.0, 4),
        _ => return None,
    };
    Some((f64::from(part(0, n)?) / max, f64::from(part(1, n)?) / max, f64::from(part(2, n)?) / max))
}

/// The window's border color (`resolvedWindowBorderColor`, `RTV:1052-1060`): a focused window
/// uses the focused color, else the unfocused one; an unfocused window the other way round;
/// with neither, grey 0.5 at 0.75.
pub fn border_color(focused: bool, focused_setting: &str, unfocused_setting: &str) -> Rgba {
    let f = parse_border_color(focused_setting);
    let u = parse_border_color(unfocused_setting);
    let base = if focused { f.or(u) } else { u.or(f) };
    base.unwrap_or(Rgba { r: 128, g: 128, b: 128, a: 0.75 })
}

/// Whether the window shows a border (`shouldShowBorder`, `PT:13088-13099`): only with
/// `UseBorder`; a transparent pane forces it, else `BordersOnlyInLightMode` keeps it to light
/// windows. Full screen drops every edge (`PT:13101-13170`).
pub fn show_border(use_border: bool, any_transparent: bool, only_light: bool, dark: bool, full_screen: bool) -> bool {
    if !use_border || full_screen {
        return false;
    }
    any_transparent || !only_light || !dark
}

/// A color as a Win32 COLORREF (`0x00BBGGRR`). DWM draws the border opaque, so the alpha is
/// dropped.
pub fn colorref(c: Rgba) -> u32 {
    u32::from(c.b) << 16 | u32::from(c.g) << 8 | u32::from(c.r)
}

/// The window corner preference DWM is given: `DWMWCP_DONOTROUND` (1) for square corners
/// (`SquareWindowCorners`, `RTV:1067`), else `DWMWCP_DEFAULT` (0).
pub fn corner_preference(square: bool) -> i32 {
    if square { 1 } else { 0 }
}

/// Whether a resolved style is a dark one (`isDark`, `PT:13071-13085`).
pub fn is_dark_style(resolved: TabStyle) -> bool {
    matches!(resolved, TabStyle::Dark | TabStyle::DarkHighContrast)
}

/// The line under the title bar is one point tall (`kDivisionViewHeight`, `RTV:47`).
pub const DIVISION_HEIGHT: f32 = 1.0;

/// The line's height in the window's pixels: one point at the window's scale, in whole
/// pixels so the panes below it start on a pixel; none when it does not show.
pub fn division_height(visible: bool, scale: f32) -> f32 {
    if visible { (DIVISION_HEIGHT * scale).round().max(1.0) } else { 0.0 }
}

/// Whether the line under the title bar shows (`divisionViewShouldBeVisible`, `PT:1168-1181`):
/// with `EnableDivisionView`, a Theme other than Minimal, a title bar (`decorated`) that is
/// not a Compact window's (13e draws that caption), not full screen, and no tab bar on top.
pub fn division_visible(enabled: bool, style: TabStyle, decorated: bool, compact: bool, full_screen: bool, top_bar_visible: bool) -> bool {
    enabled && style != TabStyle::Minimal && decorated && !compact && !full_screen && !top_bar_visible
}

/// The line's color (`RTV:1435-1450`): a light style grey 0.70 in the focused window and 0.86
/// in the others, a dark style 0.1 and 0.07.
pub fn division_color(resolved: TabStyle, focused: bool) -> Rgb {
    let brightness = match (is_dark_style(resolved), focused) {
        (false, true) => 0.70,
        (false, false) => 0.86,
        (true, true) => 0.1,
        (true, false) => 0.07,
    };
    let v = (brightness * 255.0_f32).round() as u8;
    crate::palette::rgb(v, v, v)
}

/// What a window snaps to while the user drags an edge, in the window's pixels: the focused
/// pane's cell, the outer size less the tab's pane area (`decoration`), the padding on both
/// sides of the text (`margins`), the smallest pane area (`min`) and the axes that snap.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Snap {
    pub cell: (f32, f32),
    pub decoration: (f32, f32),
    pub margins: (f32, f32),
    pub min: (f32, f32),
    pub snap_w: bool,
    pub snap_h: bool,
}

/// The axes that snap (`windowWillResize:toSize:`, `PT:5784-5855`): none for a Maximized or
/// Compact Maximized window, a maximized or full-screen one, a size-locked one, with
/// `DisableWindowSizeSnap`, or while Ctrl alone is held; a left or right percentage window
/// keeps its height free when the height does not change, a top or bottom one its width.
#[allow(clippy::too_many_arguments)]
pub fn snap_axes(style: i64, disabled: bool, ctrl_only: bool, maximized: bool, full_screen: bool, locked: bool, width_changed: bool, height_changed: bool) -> (bool, bool) {
    use crate::frames::{BOTTOM_PERCENT, COMPACT_MAXIMIZED, LEFT_PERCENT, MAXIMIZED, RIGHT_PERCENT, TOP_PERCENT};
    if matches!(style, MAXIMIZED | COMPACT_MAXIMIZED) || maximized || full_screen || locked || disabled || ctrl_only {
        return (false, false);
    }
    let h = !(matches!(style, LEFT_PERCENT | RIGHT_PERCENT) && !height_changed);
    let w = !(matches!(style, TOP_PERCENT | BOTTOM_PERCENT) && !width_changed);
    (w, h)
}

/// The size a resize to `proposed` becomes (`PT:5856-5935`), per axis: the pane area (the
/// size less the decoration) snapped to whole cells inside the margins, at least `min`; a
/// snapped size less than half a cell from `current` keeps `current`.
pub fn snap(s: &Snap, current: (f32, f32), proposed: (f32, f32)) -> (f32, f32) {
    let axis = |on: bool, cell: f32, decoration: f32, margins: f32, min: f32, current: f32, proposed: f32| {
        let mut content = proposed - decoration;
        if on && cell > 0.0 {
            content = ((content - margins) / cell).floor().max(0.0) * cell + margins;
        }
        let size = content.max(min) + decoration;
        if on && (current - size).abs().floor() < (cell / 2.0).floor() {
            return current;
        }
        size
    };
    (
        axis(s.snap_w, s.cell.0, s.decoration.0, s.margins.0, s.min.0, current.0, proposed.0),
        axis(s.snap_h, s.cell.1, s.decoration.1, s.margins.1, s.min.1, current.1, proposed.1),
    )
}

/// A snapped size within a cell short of the screen fills the screen when the pointer asked
/// for no more than half the gap it leaves (`PT:5937-5955`); `screen` is the monitor's work
/// area along the axis.
pub fn fill_screen(snapped_axis: bool, snapped: f32, proposed: f32, cell: f32, screen: f32) -> f32 {
    if snapped_axis && snapped + cell > screen && snapped < screen && screen - proposed <= ((screen - snapped) / 2.0).ceil() {
        return screen;
    }
    snapped
}

/// The work area's size in outer-frame pixels: the outer frame (`GetWindowRect`, which
/// `WM_SIZING` carries) is larger than the `visible` frame by DWM's invisible borders, so a
/// window that visibly fills the work area measures that much more (iTerm2 compares one
/// frame with the screen's visible frame, `PT:5921-5955`).
pub fn outer_screen(work: (f32, f32), outer: (f32, f32), visible: (f32, f32)) -> (f32, f32) {
    (work.0 + (outer.0 - visible.0).max(0.0), work.1 + (outer.1 - visible.1).max(0.0))
}

/// The drag rectangle (left, top, right, bottom) at `size`: a dragged left or top edge moves
/// while the right or bottom stays; otherwise the left and top stay, as AppKit keeps the
/// origin of an axis the drag only re-snaps.
pub fn sized_rect(rect: (i32, i32, i32, i32), left: bool, top: bool, size: (i32, i32)) -> (i32, i32, i32, i32) {
    let (mut l, mut t, mut r, mut b) = rect;
    if left {
        l = r - size.0;
    } else {
        r = l + size.0;
    }
    if top {
        t = b - size.1;
    } else {
        b = t + size.1;
    }
    (l, t, r, b)
}

/// Whether the user may drag the window's edges (`PTWS:1063-1067`): not when its size is
/// locked, nor with `AllowLiveResize` off.
pub fn resizable(locked: bool, allow_live: bool) -> bool {
    !locked && allow_live
}

/// The window's outer frame after a font size change (`windowFrameForTabSize:`,
/// `PT:11770-11825`): the grid at the new `cell` plus the `chrome` around the text (the
/// decoration and the padding), no larger than the work area; the top-left stays unless the
/// frame would pass the work area's right or bottom edge.
pub fn font_fit(grid: (u16, u16), cell: (f32, f32), chrome: (f32, f32), work: Rect, outer: Rect) -> Rect {
    let w = (f32::from(grid.0) * cell.0 + chrome.0).min(work.w);
    let h = (f32::from(grid.1) * cell.1 + chrome.1).min(work.h);
    Rect { x: outer.x.min(work.x + work.w - w), y: outer.y.min(work.y + work.h - h), w, h }
}

/// Whether a program may resize or move its window (`screenShouldInitiateWindowResize`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResizePermission {
    Allowed,
    Denied,
    DeniedInactive,
}

/// `PS:15717-15726`: the profile's Disable Window Resizing denies it; Disable Window Resizing
/// by Unfocused Sessions denies it to a session that is not the focused one.
pub fn resize_permission(disable: bool, disable_unfocused: bool, focused: bool) -> ResizePermission {
    if disable {
        ResizePermission::Denied
    } else if !focused && disable_unfocused {
        ResizePermission::DeniedInactive
    } else {
        ResizePermission::Allowed
    }
}

/// Window > Zoom's frame (`windowWillUseStandardFrame:defaultFrame:`, `PT:7055-7110`): the
/// work area, or with a vertical zoom the window's own x and width over the work area's
/// height. A Zoom from a key is vertical with MaxVertically; any other Zoom is vertical with
/// MaxVertically or Shift, not both.
pub fn zoom_frame(current: Rect, work: Rect, max_vertically: bool, from_key: bool, shift: bool) -> Rect {
    let vertical = if from_key { max_vertically } else { max_vertically != shift };
    if vertical {
        Rect { x: current.x, y: work.y, w: current.w, h: work.h }
    } else {
        work
    }
}

/// What a window size field counts: cells (columns or rows) or percent of the screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Units {
    Cells,
    Percent,
}

/// The profile's Columns and Rows row (`updateWindowTypeControlFromSettings`, `PWV:477-700`):
/// each axis's units popup and which field it shows (None: no popup, the cells field alone),
/// whether the fields take input, and whether "by" stands between the two halves.
#[derive(Clone, Debug, PartialEq)]
pub struct UnitsRow {
    pub columns_units: Option<Units>,
    pub rows_units: Option<Units>,
    pub columns_enabled: bool,
    pub rows_enabled: bool,
    pub by_label: bool,
}

/// The row for a stored type and its explicitly set Width and Height Percentage: an axis is
/// cells when its percentage is unset or negative. Maximized and the full screen types hide
/// the popups and disable the fields; Accessory shows the fields alone; the left and right
/// edges drop "by"; the cells ids 8 to 11 show their cells fields.
pub fn units_rows(style: i64, width_percent: Option<f64>, height_percent: Option<f64>) -> UnitsRow {
    use crate::frames::{self, ACCESSORY, BOTTOM_CELLS, FULL_SCREEN, LEFT_CELLS, LEFT_PERCENT, LION_FULL_SCREEN, MAXIMIZED, RIGHT_CELLS, RIGHT_PERCENT, TOP_CELLS};
    let units = |p: Option<f64>| if p.is_some_and(|p| p >= 0.0) { Units::Percent } else { Units::Cells };
    let row = |c: Option<Units>, r: Option<Units>, enabled: bool, by_label: bool| UnitsRow { columns_units: c, rows_units: r, columns_enabled: enabled, rows_enabled: enabled, by_label };
    match frames::unthemed(style) {
        MAXIMIZED | FULL_SCREEN | LION_FULL_SCREEN => row(None, None, false, false),
        ACCESSORY => row(None, None, true, false),
        TOP_CELLS | BOTTOM_CELLS => row(Some(Units::Cells), Some(Units::Cells), true, true),
        LEFT_CELLS | RIGHT_CELLS => row(Some(Units::Cells), Some(Units::Cells), true, false),
        s => row(Some(units(width_percent)), Some(units(height_percent)), true, !matches!(s, LEFT_PERCENT | RIGHT_PERCENT)),
    }
}

/// What a change in the row stores (`saveWindowType`, `PWV:394-475`): the Window Type, the
/// Width Percentage and the Height Percentage. A cells axis stores -1; a negative percent
/// falls back to 25 (width) and 50 (height); an edge style stores its cells form unless either
/// popup says percent; Maximized and the full screen types store only the type.
pub fn units_save(style: i64, columns_units: Units, rows_units: Units, width_percent: f64, height_percent: f64) -> (i64, f64, f64) {
    use crate::frames::{self, ACCESSORY, FULL_SCREEN, LION_FULL_SCREEN, MAXIMIZED};
    let style = frames::profile_menu_value(style);
    if matches!(style, MAXIMIZED | FULL_SCREEN | LION_FULL_SCREEN | ACCESSORY) {
        return (style, width_percent, height_percent);
    }
    let percent = |u: Units, p: f64, fallback: f64| match u {
        Units::Cells => -1.0,
        Units::Percent if p < 0.0 => fallback,
        Units::Percent => p,
    };
    let stored = frames::store_id(style, columns_units == Units::Percent, rows_units == Units::Percent);
    (stored, percent(columns_units, width_percent, 25.0), percent(rows_units, height_percent, 50.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::palette::rgb;

    fn rgba(r: u8, g: u8, b: u8, a: f32) -> Rgba {
        Rgba { r, g, b, a }
    }

    #[test]
    fn border_follows_iterm2() {
        assert!(!show_border(false, true, true, false, false));
        assert!(show_border(true, false, true, false, false));
        assert!(!show_border(true, false, true, true, false));
        assert!(show_border(true, true, true, true, false));
        assert!(show_border(true, false, false, true, false));
        assert!(!show_border(true, false, false, false, true));
        assert_eq!(border_color(true, "", ""), rgba(128, 128, 128, 0.75));
        assert_eq!(border_color(true, "#ff0000", ""), rgba(255, 0, 0, 0.75));
        // The unfocused color falls back to the focused one, and the other way round.
        assert_eq!(border_color(false, "#ff0000", ""), rgba(255, 0, 0, 0.75));
        assert_eq!(border_color(false, "#ff0000", "#00ff0080"), rgba(0, 255, 0, 128.0 / 255.0));
        assert_eq!(border_color(true, "", "#00ff00"), rgba(0, 255, 0, 0.75));
        assert_eq!(colorref(rgba(1, 2, 3, 0.5)), 0x0003_0201);
        assert_eq!(corner_preference(true), 1);
        assert_eq!(corner_preference(false), 0);
        #[cfg(windows)]
        {
            use windows_sys::Win32::Graphics::Dwm::{DWMWCP_DEFAULT, DWMWCP_DONOTROUND};
            assert_eq!(corner_preference(true), DWMWCP_DONOTROUND);
            assert_eq!(corner_preference(false), DWMWCP_DEFAULT);
        }
        assert!(is_dark_style(TabStyle::DarkHighContrast));
        assert!(is_dark_style(TabStyle::Dark));
        assert!(!is_dark_style(TabStyle::Light));
        assert!(!is_dark_style(TabStyle::LightHighContrast));
    }

    #[test]
    fn division_line_follows_iterm2() {
        use TabStyle::*;
        assert!(division_visible(true, Automatic, true, false, false, false));
        assert!(!division_visible(false, Automatic, true, false, false, false));
        assert!(!division_visible(true, Minimal, true, false, false, false));
        // No title bar.
        assert!(!division_visible(true, Automatic, false, false, false, false));
        // A compact window's caption is 13e's.
        assert!(!division_visible(true, Automatic, true, true, false, false));
        assert!(!division_visible(true, Automatic, true, false, true, false));
        // A top tab bar shows.
        assert!(!division_visible(true, Automatic, true, false, false, true));
        assert_eq!(division_color(Light, true), rgb(179, 179, 179));
        assert_eq!(division_color(Light, false), rgb(219, 219, 219));
        assert_eq!(division_color(LightHighContrast, true), rgb(179, 179, 179));
        assert_eq!(division_color(Dark, true), rgb(26, 26, 26));
        assert_eq!(division_color(Dark, false), rgb(18, 18, 18));
        assert_eq!(DIVISION_HEIGHT, 1.0);
        // One point at the window's scale.
        assert_eq!(division_height(true, 1.0), 1.0);
        assert_eq!(division_height(true, 2.0), 2.0);
        assert_eq!(division_height(true, 1.25), 1.0);
        assert_eq!(division_height(true, 1.5), 2.0);
        assert_eq!(division_height(true, 1.75), 2.0);
        assert_eq!(division_height(false, 2.0), 0.0);
    }

    #[test]
    fn border_color_parses_like_iterm2() {
        assert_eq!(parse_border_color("#12ab34"), Some(rgba(0x12, 0xab, 0x34, 0.75)));
        // iTerm2 hands "12ab34" to a parser that needs the "#": no color.
        assert_eq!(parse_border_color("12ab34"), None);
        assert_eq!(parse_border_color("#12ab34cc"), Some(rgba(0x12, 0xab, 0x34, 204.0 / 255.0)));
        assert_eq!(parse_border_color("p3#ffffff"), Some(rgba(255, 255, 255, 0.75)));
        assert_eq!(parse_border_color("p3#ffffff80"), Some(rgba(255, 255, 255, 128.0 / 255.0)));
        assert_eq!(parse_border_color(""), None);
        assert_eq!(parse_border_color("#12ab3"), None);
        assert_eq!(parse_border_color("#12ab34zz"), None);
        // The short and the 16-bit forms `getHashColorRed:` also reads.
        assert_eq!(parse_border_color("#1a3"), Some(rgba(0x11, 0xaa, 0x33, 0.75)));
        assert_eq!(parse_border_color("#ffff00008080"), Some(rgba(255, 0, 128, 0.75)));
        assert_eq!(parse_border_color("#12ab3z"), None);
        assert_eq!(parse_border_color("#\u{e9}\u{e9}\u{e9}"), None);
        assert_eq!(parse_border_color("#abcde\u{e9}f"), None);
    }

    #[test]
    fn snap_follows_iterm2() {
        use crate::frames::{LEFT_PERCENT, MAXIMIZED, NORMAL, TOP_PERCENT};
        let s = Snap { cell: (10.0, 20.0), decoration: (16.0, 60.0), margins: (8.0, 8.0), min: (0.0, 0.0), snap_w: true, snap_h: true };
        // Width: content 827, 81 columns, 81 x 10 + 8 + 16; height: content 640, 31 rows.
        assert_eq!(snap(&s, (816.0, 668.0), (843.0, 700.0)), (834.0, 688.0));
        // The snapped width 814 is within the dead zone of 5 of 816: the width stays.
        assert_eq!(snap(&s, (816.0, 668.0), (819.0, 668.0)), (816.0, 668.0));
        let smooth_w = Snap { snap_w: false, ..s };
        assert_eq!(snap(&smooth_w, (816.0, 668.0), (843.0, 700.0)), (843.0, 688.0));
        // The minimum holds with or without snapping.
        let small = Snap { min: (28.0, 28.0), snap_w: false, ..s };
        assert_eq!(snap(&small, (816.0, 668.0), (20.0, 70.0)), (44.0, 88.0));
        assert_eq!(snap_axes(NORMAL, false, false, false, false, false, true, true), (true, true));
        // Ctrl alone held.
        assert_eq!(snap_axes(NORMAL, false, true, false, false, false, true, true), (false, false));
        assert_eq!(snap_axes(MAXIMIZED, false, false, false, false, false, true, true), (false, false));
        assert_eq!(snap_axes(crate::frames::COMPACT_MAXIMIZED, false, false, false, false, false, true, true), (false, false));
        assert_eq!(snap_axes(NORMAL, false, false, true, false, false, true, true), (false, false));
        assert_eq!(snap_axes(NORMAL, false, false, false, true, false, true, true), (false, false));
        // DisableWindowSizeSnap.
        assert_eq!(snap_axes(NORMAL, true, false, false, false, false, true, true), (false, false));
        assert_eq!(snap_axes(NORMAL, false, false, false, false, true, true, true), (false, false));
        assert_eq!(snap_axes(LEFT_PERCENT, false, false, false, false, false, true, false), (true, false));
        assert_eq!(snap_axes(TOP_PERCENT, false, false, false, false, false, false, true), (false, true));
        assert_eq!(snap_axes(NORMAL, false, false, false, false, false, false, true), (true, true));
        // A snapped size just short of the screen fills it when the pointer's margin to the
        // screen edge is at most half the snapped margin, rounded up (`PT:5937-5955`).
        assert_eq!(fill_screen(true, 1915.0, 1918.0, 10.0, 1920.0), 1920.0);
        assert_eq!(fill_screen(true, 1915.0, 1916.0, 10.0, 1920.0), 1915.0);
        assert_eq!(fill_screen(false, 1915.0, 1918.0, 10.0, 1920.0), 1915.0);
        assert_eq!(fill_screen(true, 1900.0, 1918.0, 10.0, 1920.0), 1900.0);
    }

    #[test]
    fn font_change_fit_follows_iterm2() {
        let work = Rect { x: 0.0, y: 0.0, w: 1920.0, h: 1040.0 };
        let outer = Rect { x: 100.0, y: 100.0, w: 900.0, h: 600.0 };
        // 80 x 11 + 24 by 25 x 22 + 72, the top-left kept.
        assert_eq!(font_fit((80, 25), (11.0, 22.0), (24.0, 72.0), work, outer), Rect { x: 100.0, y: 100.0, w: 904.0, h: 622.0 });
        // 2224 by 1392 is larger than the work area: its size, moved onto it.
        assert_eq!(font_fit((200, 60), (11.0, 22.0), (24.0, 72.0), work, outer), Rect { x: 0.0, y: 0.0, w: 1920.0, h: 1040.0 });
        // Height 952 from y 500 would pass the bottom: y = 1040 - 952.
        let low = Rect { x: 100.0, y: 500.0, w: 900.0, h: 600.0 };
        assert_eq!(font_fit((80, 40), (11.0, 22.0), (24.0, 72.0), work, low), Rect { x: 100.0, y: 88.0, w: 904.0, h: 952.0 });
    }

    #[test]
    fn unchanged_axis_snaps_like_iterm2() {
        use crate::frames::{LEFT_PERCENT, NORMAL, TOP_PERCENT};
        let s = Snap { cell: (10.0, 20.0), decoration: (16.0, 60.0), margins: (8.0, 8.0), min: (0.0, 0.0), snap_w: true, snap_h: true };
        // A right-edge drag of a Normal window off the grid re-snaps its height too
        // (`PT:5826-5835`, `:5913-5918`): 680 is 12 from 668, past the dead zone of 10.
        let (w, h) = snap_axes(NORMAL, false, false, false, false, false, true, false);
        assert_eq!(snap(&Snap { snap_w: w, snap_h: h, ..s }, (816.0, 680.0), (843.0, 680.0)), (834.0, 668.0));
        // Within the dead zone the height stays.
        assert_eq!(snap(&Snap { snap_w: w, snap_h: h, ..s }, (816.0, 675.0), (843.0, 675.0)), (834.0, 675.0));
        // A left percentage window keeps its height free on a width drag (`PT:5837-5850`).
        let (w, h) = snap_axes(LEFT_PERCENT, false, false, false, false, false, true, false);
        assert_eq!(snap(&Snap { snap_w: w, snap_h: h, ..s }, (816.0, 680.0), (843.0, 680.0)), (834.0, 680.0));
        let (w, h) = snap_axes(TOP_PERCENT, false, false, false, false, false, false, true);
        assert_eq!(snap(&Snap { snap_w: w, snap_h: h, ..s }, (823.0, 668.0), (823.0, 700.0)), (823.0, 688.0));
        // The dragged edges move; an axis only snapped moves its right or bottom side.
        assert_eq!(sized_rect((100, 100, 943, 780), false, false, (834, 668)), (100, 100, 934, 768));
        assert_eq!(sized_rect((109, 100, 943, 780), true, false, (834, 668)), (109, 100, 943, 768));
        assert_eq!(sized_rect((100, 100, 943, 780), true, true, (834, 668)), (109, 112, 943, 780));
    }

    #[test]
    fn fill_screen_uses_the_visible_frame() {
        // DWM's invisible borders (7 on each side and the bottom) are outside the visible
        // frame: the work area in outer pixels is that much larger.
        assert_eq!(outer_screen((1920.0, 1040.0), (934.0, 768.0), (920.0, 761.0)), (1934.0, 1047.0));
        // No invisible borders.
        assert_eq!(outer_screen((1920.0, 1040.0), (934.0, 768.0), (934.0, 768.0)), (1920.0, 1040.0));
        // An outer frame that fills it shows exactly the work area.
        assert_eq!(fill_screen(true, 1929.0, 1932.0, 10.0, 1934.0), 1934.0);
    }

    #[test]
    fn vertical_zoom_follows_iterm2() {
        let current = Rect { x: 100.0, y: 200.0, w: 800.0, h: 500.0 };
        let work = Rect { x: 0.0, y: 0.0, w: 1920.0, h: 1040.0 };
        let tall = Rect { x: 100.0, y: 0.0, w: 800.0, h: 1040.0 };
        assert_eq!(zoom_frame(current, work, true, true, false), tall);
        assert_eq!(zoom_frame(current, work, false, true, false), work);
        // A mouse Zoom with Shift inverts MaxVertically; a key Zoom ignores Shift.
        assert_eq!(zoom_frame(current, work, false, false, true), tall);
        assert_eq!(zoom_frame(current, work, true, false, true), work);
        assert_eq!(zoom_frame(current, work, false, true, true), work);
    }

    #[test]
    fn units_follow_iterm2() {
        use crate::frames::{FULL_SCREEN, LEFT_CELLS, LEFT_PERCENT, MAXIMIZED, NORMAL, TOP_PERCENT};
        use Units::{Cells, Percent};
        let normal = units_rows(NORMAL, None, None);
        assert_eq!(normal, UnitsRow { columns_units: Some(Cells), rows_units: Some(Cells), columns_enabled: true, rows_enabled: true, by_label: true });
        assert_eq!(units_rows(NORMAL, Some(50.0), None).columns_units, Some(Percent));
        assert_eq!(units_rows(NORMAL, Some(50.0), None).rows_units, Some(Cells));
        assert_eq!(units_rows(NORMAL, Some(-1.0), None).columns_units, Some(Cells), "negative is cells");
        assert_eq!(units_rows(13, None, Some(40.0)).rows_units, Some(Percent), "Compact is Normal");
        let max = units_rows(MAXIMIZED, Some(50.0), Some(50.0));
        assert_eq!(max, UnitsRow { columns_units: None, rows_units: None, columns_enabled: false, rows_enabled: false, by_label: false });
        assert_eq!(units_rows(FULL_SCREEN, None, None), max);
        assert_eq!(units_rows(crate::frames::LION_FULL_SCREEN, None, None), max);
        let left = units_rows(LEFT_PERCENT, None, None);
        assert_eq!((left.columns_units, left.rows_units, left.by_label), (Some(Cells), Some(Cells), false));
        assert!(!units_rows(LEFT_CELLS, None, None).by_label);
        assert!(units_rows(crate::frames::TOP_CELLS, None, None).by_label);
        // Accessory: the fields without the popups.
        let acc = units_rows(crate::frames::ACCESSORY, None, None);
        assert_eq!(acc, UnitsRow { columns_units: None, rows_units: None, columns_enabled: true, rows_enabled: true, by_label: false });

        assert_eq!(units_save(TOP_PERCENT, Cells, Cells, 100.0, 100.0), (9, -1.0, -1.0), "the cells form");
        assert_eq!(units_save(TOP_PERCENT, Percent, Cells, 100.0, 100.0), (2, 100.0, -1.0));
        assert_eq!(units_save(crate::frames::TOP_CELLS, Cells, Percent, 100.0, 30.0), (2, -1.0, 30.0), "a stored cells id saves as its edge");
        assert_eq!(units_save(NORMAL, Percent, Percent, -5.0, -5.0), (0, 25.0, 50.0), "iTerm2's fallbacks");
        assert_eq!(units_save(NORMAL, Cells, Cells, 40.0, 60.0), (0, -1.0, -1.0));
        assert_eq!(units_save(MAXIMIZED, Percent, Percent, 40.0, 60.0), (15, 40.0, 60.0), "only the type");
    }

    #[test]
    fn live_resize_follows_iterm2() {
        assert!(resizable(false, true));
        assert!(!resizable(true, true));
        assert!(!resizable(false, false));
    }

    #[test]
    fn resize_permission_follows_iterm2() {
        assert_eq!(resize_permission(true, false, true), ResizePermission::Denied);
        assert_eq!(resize_permission(false, true, false), ResizePermission::DeniedInactive);
        assert_eq!(resize_permission(false, true, true), ResizePermission::Allowed);
        assert_eq!(resize_permission(false, false, false), ResizePermission::Allowed);
    }
}
