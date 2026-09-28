//! The Theme setting (Appearance > General, `TabStyleWithAutomaticOption`): which tab style a
//! window draws, and the fixed style (Light, Dark or a High Contrast one) it resolves to.
//! 13e extends this module (the forced appearance, the palettes, Minimal and Compact).

use crate::palette::{Rgb, rgb};

/// iTerm2's tab styles, in its stored order (`iTermPreferences.h:30-36`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabStyle {
    Light,
    Dark,
    LightHighContrast,
    DarkHighContrast,
    Automatic,
    Minimal,
    Compact,
}

/// The global key the Theme popup stores.
pub const KEY: &str = "TabStyleWithAutomaticOption";

/// The style a stored value names; anything else is Automatic (iTerm2's computed default).
pub fn from_value(v: i64) -> TabStyle {
    match v {
        0 => TabStyle::Light,
        1 => TabStyle::Dark,
        2 => TabStyle::LightHighContrast,
        3 => TabStyle::DarkHighContrast,
        5 => TabStyle::Minimal,
        6 => TabStyle::Compact,
        _ => TabStyle::Automatic,
    }
}

/// The value a style is stored as.
pub fn value(s: TabStyle) -> i64 {
    match s {
        TabStyle::Light => 0,
        TabStyle::Dark => 1,
        TabStyle::LightHighContrast => 2,
        TabStyle::DarkHighContrast => 3,
        TabStyle::Automatic => 4,
        TabStyle::Minimal => 5,
        TabStyle::Compact => 6,
    }
}

/// The fixed style a window draws (`it_tabStyle:`, `NSAppearance+iTerm.m:53-90`): a fixed
/// style as stated; Automatic, Minimal and Compact follow the system's dark and high contrast
/// modes.
pub fn resolved(style: TabStyle, system_dark: bool, system_high_contrast: bool) -> TabStyle {
    match style {
        TabStyle::Light | TabStyle::Dark | TabStyle::LightHighContrast | TabStyle::DarkHighContrast => style,
        TabStyle::Automatic | TabStyle::Minimal | TabStyle::Compact => match (system_dark, system_high_contrast) {
            (false, false) => TabStyle::Light,
            (true, false) => TabStyle::Dark,
            (false, true) => TabStyle::LightHighContrast,
            (true, true) => TabStyle::DarkHighContrast,
        },
    }
}

/// The appearance a fixed style forces on the whole app (`updateAppearance`,
/// `iTermApplication.m:1276-1292`): `Some(dark)` for Light, Dark and the High Contrast styles;
/// Regular (Automatic), Minimal and Compact leave it to the system.
pub fn forced_dark(style: TabStyle) -> Option<bool> {
    match style {
        TabStyle::Light | TabStyle::LightHighContrast => Some(false),
        TabStyle::Dark | TabStyle::DarkHighContrast => Some(true),
        TabStyle::Automatic | TabStyle::Minimal | TabStyle::Compact => None,
    }
}

/// The winit theme a window is given for a forced appearance; `None` follows Windows.
pub fn window_theme(forced: Option<bool>) -> Option<winit::window::Theme> {
    forced.map(|dark| if dark { winit::window::Theme::Dark } else { winit::window::Theme::Light })
}

/// Whether the tab bar goes in the caption area with the window buttons (Minimal and Compact).
pub fn compact_theme(style: TabStyle) -> bool {
    matches!(style, TabStyle::Minimal | TabStyle::Compact)
}

/// The text of the warning on Profiles > Colors (`ProfilesColorsPreferencesViewController.m:1036-1038`).
pub const MODE_WARNING: &str = "Your current theme overrides the system light and dark mode setting, so color switching will not occur. You can change it in Settings > Appearance > General > Theme.";

/// Whether the Colors tab warns: a fixed theme, while the profile keeps separate light and dark
/// colors (`:384-397`, `:373-380`).
pub fn mode_warning(style: TabStyle, separate_colors: bool) -> bool {
    separate_colors && forced_dark(style).is_some()
}

/// Whether the mode a window follows changed since it was last applied: never on the first
/// apply (a new window has nothing to refresh).
pub fn theme_changed(applied: Option<bool>, now: bool) -> bool {
    applied.is_some_and(|prev| prev != now)
}

/// What `system_high_contrast` remembered: 0 not read, 1 off, 2 on.
static HIGH_CONTRAST: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

/// Whether Windows is in a high contrast theme, read once and again after
/// `forget_high_contrast` (every layout, hit test and frame asks).
pub fn system_high_contrast() -> bool {
    use std::sync::atomic::Ordering;
    match HIGH_CONTRAST.load(Ordering::Relaxed) {
        0 => {
            let on = read_high_contrast();
            HIGH_CONTRAST.store(if on { 2 } else { 1 }, Ordering::Relaxed);
            on
        }
        v => v == 2,
    }
}

/// Drops what `system_high_contrast` remembered; the system settings changed.
pub fn forget_high_contrast() {
    HIGH_CONTRAST.store(0, std::sync::atomic::Ordering::Relaxed);
}

/// `SPI_GETHIGHCONTRAST`, flag `HCF_HIGHCONTRASTON`.
#[cfg(windows)]
fn read_high_contrast() -> bool {
    use windows_sys::Win32::UI::Accessibility::{HCF_HIGHCONTRASTON, HIGHCONTRASTW};
    use windows_sys::Win32::UI::WindowsAndMessaging::{SPI_GETHIGHCONTRAST, SystemParametersInfoW};
    let mut hc = HIGHCONTRASTW { cbSize: std::mem::size_of::<HIGHCONTRASTW>() as u32, dwFlags: 0, lpszDefaultScheme: std::ptr::null_mut() };
    // SAFETY: `hc` is a valid HIGHCONTRASTW with its size set, as the call requires.
    let ok = unsafe { SystemParametersInfoW(SPI_GETHIGHCONTRAST, hc.cbSize, &mut hc as *mut _ as *mut _, 0) };
    ok != 0 && hc.dwFlags & HCF_HIGHCONTRASTON != 0
}

/// A color with its alpha (`NSColor` with alpha): laid over what is behind it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fill {
    pub color: Rgb,
    pub alpha: f32,
}

impl Fill {
    const fn new(color: Rgb, alpha: f32) -> Fill {
        Fill { color, alpha }
    }
}

/// The macOS title bar material behind the Light tab bar (`PSMYosemiteTabStyle tabBarColor`,
/// `YOS:1164-1169`).
pub const LIGHT_MATERIAL: Rgb = rgb(225, 225, 225);
/// Tandem's fixed color behind the Dark tab bar (its former active tab color).
pub const DARK_MATERIAL: Rgb = rgb(0x3a, 0x3a, 0x3a);
/// `windowBackgroundColor` in the light appearance (Light High Contrast's selected tab).
const WINDOW_BACKGROUND: Rgb = rgb(236, 236, 236);
/// The label size of the High Contrast styles in points (`LHC`, `DHC`); the others use 11.
pub const HIGH_CONTRAST_FONT_PT: f32 = 12.0;
/// The label size of the other styles in points (`YOS:557-563`).
pub const REGULAR_FONT_PT: f32 = 11.0;

/// The Minimal style's parts of a palette (Task 4 fills them; `None` for the other styles).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MinimalLook {
    /// The outline drawn around the selected tab.
    pub outline: Fill,
    pub left_inset_is_first_tab: bool,
    pub underline: f32,
    pub deselected_colored_alpha: f32,
}

/// The Minimal style's Advanced settings, iTerm2's defaults.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MinimalKnobs {
    /// `MinimalTabStyleBackgroundColorDifference`.
    pub difference: f32,
    /// `MinimalTabStyleOutlineStrength`.
    pub outline: f32,
    /// `MinimalTextLegibilityAdjustment`.
    pub legibility: f32,
    /// `MinimalTabStyleTreatLeftInsetAsPartOfFirstTab`.
    pub left_inset_is_first_tab: bool,
    /// `MinimalSelectedTabUnderlineProminence`.
    pub underline: f32,
    /// `MinimalDeslectedColoredTabAlpha`.
    pub deselected_colored_alpha: f32,
}

impl Default for MinimalKnobs {
    fn default() -> MinimalKnobs {
        MinimalKnobs { difference: 0.05, outline: 0.2, legibility: 1.0, left_inset_is_first_tab: false, underline: 1.0, deselected_colored_alpha: 0.5 }
    }
}

/// Every color and size a tab bar draws with.
#[derive(Clone, Debug, PartialEq)]
pub struct TabPalette {
    /// The Theme setting.
    pub style: TabStyle,
    /// The fixed style it draws as (`resolved`).
    pub resolved: TabStyle,
    /// The opaque material the fills are laid over.
    pub bar: Rgb,
    pub selected: Fill,
    pub unselected: Fill,
    /// The bar's room beyond the tabs.
    pub margin: Fill,
    pub text_selected: Fill,
    pub text_unselected: Fill,
    /// The line between two tabs.
    pub between: Fill,
    /// The line between the bar and the content.
    pub edge: Fill,
    /// The line along the top of an unselected tab (`topLineColorSelected:NO`, `YOS:571`, `DRK:39`,
    /// `LHC:40`) and of the selected one (which a top bar leaves out).
    pub top: Fill,
    pub top_selected: Fill,
    /// The room a side bar's row line leaves at the left and right (`insetsForTabBarDividers`,
    /// `YOS:760`, `DRK:99`), in points.
    pub line_inset: (f32, f32),
    /// The Dark styles draw the content-side line as a solid black one (iTerm2's one-point
    /// gradient from black to black at 0.75, Decision).
    pub dark_edge_gradient: bool,
    /// The label size in points.
    pub font_pt: f32,
    /// Light glyphs (the bar is dark).
    pub dark_controls: bool,
    pub minimal: Option<MinimalLook>,
    /// `ColoredSelectedTabOutlineStrength`.
    pub colored_outline: f32,
    /// `ColoredUnselectedTabTextProminence`.
    pub colored_text_prominence: f32,
}

/// What a palette depends on.
#[derive(Clone, Copy, Debug)]
pub struct Inputs {
    pub system_dark: bool,
    pub system_high_contrast: bool,
    /// The window is the key window of the active app.
    pub focused: bool,
    /// `LightModeInactiveTabDarkness`.
    pub light_darkness: f32,
    /// `DarkModeInactiveTabDarkness`.
    pub dark_darkness: f32,
    /// `HighVisibility`.
    pub high_visibility: bool,
    /// The focused pane's background (Minimal's bar).
    pub session_bg: Rgb,
    /// The dimming amount of a background window (Minimal).
    pub dimming: f32,
    pub minimal: MinimalKnobs,
    /// `ColoredSelectedTabOutlineStrength`.
    pub colored_outline: f32,
    /// `ColoredUnselectedTabTextProminence`.
    pub colored_text_prominence: f32,
}

/// The label size in points a resolved style draws (`font_pt` of its palette): 12 for the High
/// Contrast styles, else 11. Pure, so hot paths need no palette.
pub fn font_pt(resolved: TabStyle) -> f32 {
    match resolved {
        TabStyle::LightHighContrast | TabStyle::DarkHighContrast => HIGH_CONTRAST_FONT_PT,
        _ => REGULAR_FONT_PT,
    }
}

/// `f` laid over `base` (source-over), each component rounded half up.
pub fn over(base: Rgb, f: Fill) -> Rgb {
    let a = f.alpha.clamp(0.0, 1.0);
    let mix = |b: u8, c: u8| (f32::from(b) * (1.0 - a) + f32::from(c) * a + 0.5).floor() as u8;
    rgb(mix(base.r, f.color.r), mix(base.g, f.color.g), mix(base.b, f.color.b))
}

/// A grey of `w` in 0 to 1 as a color, rounded half up.
fn grey(w: f32) -> Rgb {
    let v = (w * 255.0 + 0.5).floor() as u8;
    rgb(v, v, v)
}

const BLACK: Rgb = rgb(0, 0, 0);
const WHITE: Rgb = rgb(255, 255, 255);

/// The palette a style draws with (`PSMYosemiteTabStyle` and its Dark, Light High Contrast and
/// Dark High Contrast subclasses). Automatic, Minimal and Compact draw as the style they
/// resolve to; Task 4 adds the Minimal parts.
pub fn palette(style: TabStyle, i: &Inputs) -> TabPalette {
    let resolved = resolved(style, i.system_dark, i.system_high_contrast);
    let focused = i.focused;
    let (light_bar, dark_bar) = (LIGHT_MATERIAL, DARK_MATERIAL);
    let fill = |c: Rgb, a: f32| Fill::new(c, a);
    let mut p = match resolved {
        TabStyle::Dark | TabStyle::DarkHighContrast => {
            let unselected = fill(BLACK, i.dark_darkness);
            TabPalette {
                style,
                resolved,
                bar: dark_bar,
                selected: fill(BLACK, 0.0),
                unselected,
                margin: unselected,
                text_selected: if focused { fill(WHITE, 1.0) } else { fill(grey(0.45), 1.0) },
                text_unselected: if focused { fill(grey(0.65), 1.0) } else { fill(grey(0.37), 1.0) },
                between: fill(WHITE, if focused { 0.18 } else { 0.15 }),
                edge: fill(BLACK, 0.3),
                top: fill(BLACK, 1.0),
                top_selected: fill(WHITE, if focused { 0.20 } else { 0.16 }),
                line_inset: (1.0, 1.0),
                dark_edge_gradient: true,
                font_pt: REGULAR_FONT_PT,
                dark_controls: true,
                minimal: None,
                colored_outline: i.colored_outline,
                colored_text_prominence: i.colored_text_prominence,
            }
        }
        _ => {
            let unselected = fill(BLACK, i.light_darkness);
            TabPalette {
                style,
                resolved,
                bar: light_bar,
                selected: fill(BLACK, 0.0),
                unselected,
                margin: unselected,
                text_selected: if focused { fill(BLACK, 1.0) } else { fill(grey(189.0 / 255.0), 1.0) },
                text_unselected: if focused { fill(BLACK, 1.0) } else { fill(grey(164.0 / 255.0), 1.0) },
                between: fill(BLACK, 0.07),
                edge: fill(rgb(230, 230, 230), 1.0),
                top: fill(BLACK, 0.0),
                top_selected: fill(BLACK, 0.0),
                line_inset: (1.0, 2.0),
                dark_edge_gradient: false,
                font_pt: REGULAR_FONT_PT,
                dark_controls: false,
                minimal: None,
                colored_outline: i.colored_outline,
                colored_text_prominence: i.colored_text_prominence,
            }
        }
    };
    match resolved {
        TabStyle::LightHighContrast => {
            let line = fill(rgb(140, 140, 140), 1.0);
            p.selected = fill(WINDOW_BACKGROUND, 1.0);
            p.unselected = fill(rgb(180, 180, 180), 1.0);
            p.margin = p.unselected;
            p.text_selected = fill(BLACK, 1.0);
            p.text_unselected = fill(BLACK, 1.0);
            // The between lines stay Light's: `verticalLineColor` (`LHC:36`) is never called, PSM
            // asks `verticalLineColorSelected:` (`YOS:738`). The bottom and unselected top lines
            // are the grey (`LHC:40-50`).
            p.edge = line;
            p.top = line;
        }
        TabStyle::DarkHighContrast => {
            // `HighVisibility` (Dark High Contrast only): a light selected tab with black text.
            p.unselected = fill(grey(0.03), 1.0);
            p.margin = p.unselected;
            if i.high_visibility {
                p.selected = fill(grey(0.80), 1.0);
                p.text_selected = fill(BLACK, 1.0);
            } else {
                p.selected = fill(grey(0.2), 1.0);
                p.text_selected = fill(WHITE, 1.0);
            }
            p.text_unselected = fill(WHITE, 1.0);
        }
        _ => {}
    }
    p.font_pt = font_pt(resolved);
    p
}

/// The perceived brightness of a color in 0 to 1 (HSP: `sqrt(0.241 r^2 + 0.691 g^2 + 0.068 b^2)`,
/// `PSMC:39-47`).
pub fn hsp(c: Rgb) -> f32 {
    let n = |v: u8| f32::from(v) / 255.0;
    (0.241 * n(c.r) * n(c.r) + 0.691 * n(c.g) * n(c.g) + 0.068 * n(c.b) * n(c.b)).sqrt()
}

/// A colored tab's fill (`YOS:611-636`): each component of the tab color mixed toward 50% grey
/// by alpha 1 (selected, key window), 0.4 (unselected, key), 0.6 (selected) or 0.3 (unselected)
/// in a window that is not key. Rounded half up.
pub fn colored_fill(tab: Rgb, selected: bool, focused: bool) -> Rgb {
    let a: f64 = match (selected, focused) {
        (true, true) => 1.0,
        (false, true) => 0.4,
        (true, false) => 0.6,
        (false, false) => 0.3,
    };
    // The epsilon keeps a value that is exactly .5 in decimal from rounding down in binary.
    let mix = |c: u8| (f64::from(c) * a + 127.5 * (1.0 - a) + 0.5 + 1e-6).floor() as u8;
    rgb(mix(tab.r), mix(tab.g), mix(tab.b))
}

/// The text over a brightness (0 to 1) in a window with colored tabs (`YOS:466-509`): black
/// above 0.5, else white; opaque when selected, else at `prominence`.
pub fn text_for_brightness(brightness: f32, selected: bool, prominence: f32) -> Fill {
    let color = if brightness > 0.5 { BLACK } else { WHITE };
    Fill { color, alpha: if selected { 1.0 } else { prominence } }
}

/// The text over an opaque fill in a window with colored tabs.
pub fn colored_text(fill: Rgb, selected: bool, prominence: f32) -> Fill {
    text_for_brightness(hsp(fill), selected, prominence)
}

/// The brightness a translucent fill has over the material (`YOS:776-797`): the fill's HSP
/// blended with the back (1 in a light bar, 0 in a dark one) by its alpha.
pub fn fill_brightness(f: Fill, dark_back: bool) -> f32 {
    let back = if dark_back { 0.0 } else { 1.0 };
    let a = f.alpha.clamp(0.0, 1.0);
    hsp(f.color) * a + back * (1.0 - a)
}

/// The two strokes around the selected tab of a window with colored tabs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Outline {
    pub width: f32,
    pub outer: Fill,
    pub inner: Fill,
}

/// `YOS:805-839`: alpha `clamp(strength, 0, 1)` (times 0.6 when the window is not key), width
/// `clamp(strength, 1, 3)`; over a bright fill the outer stroke is white and the inner black,
/// else the reverse.
pub fn selected_outline(fill: Rgb, strength: f32, focused: bool) -> Outline {
    let alpha = strength.clamp(0.0, 1.0) * if focused { 1.0 } else { 0.6 };
    let (outer, inner) = if hsp(fill) > 0.5 { (WHITE, BLACK) } else { (BLACK, WHITE) };
    Outline { width: strength.clamp(1.0, 3.0), outer: Fill { color: outer, alpha }, inner: Fill { color: inner, alpha } }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_resolves_like_iterm2() {
        use TabStyle::*;
        let order = [Light, Dark, LightHighContrast, DarkHighContrast, Automatic, Minimal, Compact];
        for (v, s) in order.iter().enumerate() {
            assert_eq!(from_value(v as i64), *s);
            assert_eq!(value(*s), v as i64);
        }
        assert_eq!(from_value(9), Automatic);
        assert_eq!(resolved(Automatic, false, false), Light);
        assert_eq!(resolved(Automatic, true, false), Dark);
        assert_eq!(resolved(Automatic, false, true), LightHighContrast);
        assert_eq!(resolved(Automatic, true, true), DarkHighContrast);
        for style in [Minimal, Compact] {
            for (dark, hc) in [(false, false), (true, false), (false, true), (true, true)] {
                assert_eq!(resolved(style, dark, hc), resolved(Automatic, dark, hc));
            }
        }
        assert_eq!(resolved(Light, true, true), Light);
        assert_eq!(resolved(DarkHighContrast, false, false), DarkHighContrast);
    }

    #[test]
    fn forced_appearance_follows_iterm2() {
        use TabStyle::*;
        assert_eq!(forced_dark(Light), Some(false));
        assert_eq!(forced_dark(LightHighContrast), Some(false));
        assert_eq!(forced_dark(Dark), Some(true));
        assert_eq!(forced_dark(DarkHighContrast), Some(true));
        for style in [Automatic, Minimal, Compact] {
            assert_eq!(forced_dark(style), None);
        }
        assert_eq!(window_theme(Some(true)), Some(winit::window::Theme::Dark));
        assert_eq!(window_theme(Some(false)), Some(winit::window::Theme::Light));
        assert_eq!(window_theme(None), None);
    }

    #[test]
    fn mode_warning_follows_iterm2() {
        use TabStyle::*;
        assert!(mode_warning(Light, true));
        assert!(mode_warning(Dark, true));
        assert!(mode_warning(LightHighContrast, true));
        assert!(!mode_warning(Automatic, true));
        assert!(!mode_warning(Minimal, true));
        assert!(!mode_warning(Compact, true));
        assert!(!mode_warning(Light, false));
        assert_eq!(MODE_WARNING, "Your current theme overrides the system light and dark mode setting, so color switching will not occur. You can change it in Settings > Appearance > General > Theme.");
        assert!(compact_theme(Minimal) && compact_theme(Compact));
        for style in [Light, Dark, LightHighContrast, DarkHighContrast, Automatic] {
            assert!(!compact_theme(style));
        }
    }

    fn inputs(focused: bool) -> Inputs {
        Inputs {
            system_dark: false,
            system_high_contrast: false,
            focused,
            light_darkness: 0.07,
            dark_darkness: 0.5,
            high_visibility: true,
            session_bg: rgb(0, 0, 0),
            dimming: 0.0,
            minimal: MinimalKnobs::default(),
            colored_outline: 0.5,
            colored_text_prominence: 0.5,
        }
    }

    fn fill(r: u8, g: u8, b: u8, alpha: f32) -> Fill {
        Fill { color: rgb(r, g, b), alpha }
    }

    #[test]
    fn light_palette_follows_iterm2() {
        let p = palette(TabStyle::Light, &inputs(true));
        assert_eq!(p.bar, LIGHT_MATERIAL);
        assert_eq!(LIGHT_MATERIAL, rgb(225, 225, 225));
        assert_eq!(p.selected.alpha, 0.0);
        assert_eq!(p.unselected, fill(0, 0, 0, 0.07));
        assert_eq!(over(p.bar, p.unselected), rgb(209, 209, 209));
        assert_eq!(p.margin, p.unselected);
        assert_eq!((p.text_selected, p.text_unselected), (fill(0, 0, 0, 1.0), fill(0, 0, 0, 1.0)));
        assert_eq!(p.between, fill(0, 0, 0, 0.07));
        assert_eq!(p.edge, fill(230, 230, 230, 1.0));
        assert_eq!(p.font_pt, 11.0);
        assert!(!p.dark_controls && !p.dark_edge_gradient);
        assert_eq!((p.style, p.resolved), (TabStyle::Light, TabStyle::Light));
        let off = palette(TabStyle::Light, &inputs(false));
        assert_eq!((off.text_selected, off.text_unselected), (fill(189, 189, 189, 1.0), fill(164, 164, 164, 1.0)));
        let more = palette(TabStyle::Light, &Inputs { light_darkness: 0.2, ..inputs(true) });
        assert_eq!(more.unselected.alpha, 0.2);
    }

    #[test]
    fn dark_palette_follows_iterm2() {
        let p = palette(TabStyle::Dark, &inputs(true));
        assert_eq!(p.bar, DARK_MATERIAL);
        assert_eq!(DARK_MATERIAL, rgb(0x3a, 0x3a, 0x3a));
        assert_eq!(p.selected.alpha, 0.0);
        assert_eq!(p.unselected, fill(0, 0, 0, 0.5));
        assert_eq!(over(p.bar, p.unselected), rgb(29, 29, 29));
        assert_eq!((p.text_selected, p.text_unselected), (fill(255, 255, 255, 1.0), fill(166, 166, 166, 1.0)));
        let off = palette(TabStyle::Dark, &inputs(false));
        assert_eq!((off.text_selected, off.text_unselected), (fill(115, 115, 115, 1.0), fill(94, 94, 94, 1.0)));
        assert_eq!(p.between, fill(255, 255, 255, 0.18));
        assert_eq!(off.between.alpha, 0.15);
        assert_eq!(p.edge, fill(0, 0, 0, 0.3));
        assert!(p.dark_edge_gradient && p.dark_controls);
        assert_eq!(p.font_pt, 11.0);
    }

    #[test]
    fn high_contrast_follows_iterm2() {
        let l = palette(TabStyle::LightHighContrast, &inputs(true));
        assert_eq!(l.selected, fill(236, 236, 236, 1.0));
        assert_eq!(l.unselected, fill(180, 180, 180, 1.0));
        // The between lines are Light's (`LHC:36` is never called); the edge and the top line are 140 grey.
        assert_eq!((l.between, l.edge, l.top, l.top_selected), (fill(0, 0, 0, 0.07), fill(140, 140, 140, 1.0), fill(140, 140, 140, 1.0), fill(0, 0, 0, 0.0)));
        assert_eq!((l.text_selected, l.text_unselected), (fill(0, 0, 0, 1.0), fill(0, 0, 0, 1.0)));
        assert_eq!(l.font_pt, 12.0);
        let d = palette(TabStyle::DarkHighContrast, &inputs(true));
        assert_eq!(d.selected, fill(204, 204, 204, 1.0));
        assert_eq!(d.unselected, fill(8, 8, 8, 1.0));
        assert_eq!((d.text_selected, d.text_unselected), (fill(0, 0, 0, 1.0), fill(255, 255, 255, 1.0)));
        assert_eq!(d.font_pt, 12.0);
        let plain = palette(TabStyle::DarkHighContrast, &Inputs { high_visibility: false, ..inputs(true) });
        assert_eq!(plain.selected, fill(51, 51, 51, 1.0));
        assert_eq!((plain.text_selected, plain.text_unselected), (fill(255, 255, 255, 1.0), fill(255, 255, 255, 1.0)));
        // Light High Contrast ignores HighVisibility.
        assert_eq!(palette(TabStyle::LightHighContrast, &Inputs { high_visibility: false, ..inputs(true) }), l);
        // Automatic, Minimal and Compact draw as what they resolve to.
        for style in [TabStyle::Automatic, TabStyle::Minimal, TabStyle::Compact] {
            let auto = palette(style, &Inputs { system_dark: true, ..inputs(true) });
            assert_eq!(auto, TabPalette { style, ..palette(TabStyle::Dark, &inputs(true)) });
            assert_eq!(auto.resolved, TabStyle::Dark);
        }
        assert_eq!(font_pt(TabStyle::LightHighContrast), 12.0);
        assert_eq!(font_pt(TabStyle::Dark), 11.0);
    }

    #[test]
    fn colored_tab_follows_iterm2() {
        let red = rgb(255, 0, 0);
        assert_eq!(colored_fill(red, true, true), rgb(255, 0, 0));
        assert_eq!(colored_fill(red, false, true), rgb(179, 77, 77));
        assert_eq!(colored_fill(red, true, false), rgb(204, 51, 51));
        assert_eq!(colored_fill(red, false, false), rgb(166, 89, 89));
        assert!((hsp(rgb(255, 255, 255)) - 1.0).abs() < 1e-4);
        assert_eq!(hsp(rgb(0, 0, 0)), 0.0);
        let black = |a: f32| Fill { color: rgb(0, 0, 0), alpha: a };
        let white = |a: f32| Fill { color: rgb(255, 255, 255), alpha: a };
        assert_eq!(colored_text(rgb(240, 240, 240), true, 0.5), black(1.0));
        assert_eq!(colored_text(rgb(20, 20, 20), true, 0.5), white(1.0));
        assert_eq!(colored_text(rgb(240, 240, 240), false, 0.5), black(0.5));
        assert_eq!(colored_text(rgb(20, 20, 20), false, 0.3), white(0.3));
        assert_eq!(selected_outline(rgb(240, 240, 240), 0.5, true), Outline { width: 1.0, outer: white(0.5), inner: black(0.5) });
        assert_eq!(selected_outline(rgb(20, 20, 20), 2.0, false), Outline { width: 2.0, outer: black(0.6), inner: white(0.6) });
        assert_eq!(selected_outline(rgb(20, 20, 20), 0.0, true).outer.alpha, 0.0);
    }

    #[test]
    fn theme_change_decision() {
        // A new window has nothing to refresh, whatever mode it takes.
        assert!(!theme_changed(None, true));
        assert!(!theme_changed(None, false));
        assert!(!theme_changed(Some(true), true));
        assert!(theme_changed(Some(true), false));
        assert!(theme_changed(Some(false), true));
    }
}
