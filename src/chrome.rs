//! Window chrome rules: the border around a window (when it shows, its color), the corner
//! preference and the line under the title bar, as iTerm2 decides them.

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
}
