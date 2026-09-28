//! Window chrome rules: the border around a window (when it shows, its color), the corner
//! preference, the line under the title bar, the snap to the cell grid while resizing and
//! whether programs may resize the window, as iTerm2 decides them.

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

/// The line under the title bar is one pixel tall (`kDivisionViewHeight`, `RTV:47`).
pub const DIVISION_HEIGHT: f32 = 1.0;

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

/// Whether the user may drag the window's edges (`PTWS:1063-1067`): not when its size is
/// locked, nor with `AllowLiveResize` off.
pub fn resizable(locked: bool, allow_live: bool) -> bool {
    !locked && allow_live
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
        // A snapped size just short of the screen fills it when the pointer asked for more
        // than half the gap (`PT:5937-5955`).
        assert_eq!(fill_screen(true, 1915.0, 1918.0, 10.0, 1920.0), 1920.0);
        assert_eq!(fill_screen(true, 1915.0, 1916.0, 10.0, 1920.0), 1915.0);
        assert_eq!(fill_screen(false, 1915.0, 1918.0, 10.0, 1920.0), 1915.0);
        assert_eq!(fill_screen(true, 1900.0, 1918.0, 10.0, 1920.0), 1900.0);
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
