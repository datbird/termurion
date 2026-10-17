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

/// Whether the tab bar is attached to the title bar (`PT:10142-10165`): every theme and position
/// but Compact with a Top bar.
pub fn attached(style: TabStyle, top: bool) -> bool {
    !(style == TabStyle::Compact && top)
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
            #[cfg(windows)]
            let on = read_high_contrast();
            #[cfg(not(windows))]
            let on = false;
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
/// Termurion's fixed color behind the Dark tab bar (its former active tab color).
pub const DARK_MATERIAL: Rgb = rgb(0x3a, 0x3a, 0x3a);
/// `windowBackgroundColor` in the light appearance (Light High Contrast's selected tab).
const WINDOW_BACKGROUND: Rgb = rgb(236, 236, 236);
/// The label size of the High Contrast styles in points (`LHC`, `DHC`); the others use 11.
pub const HIGH_CONTRAST_FONT_PT: f32 = 12.0;
/// The label size of the other styles in points (`YOS:557-563`).
pub const REGULAR_FONT_PT: f32 = 11.0;

/// The Minimal style's parts of a palette (`None` for the other styles).
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
    /// The bar is attached to the title bar (`AttachedToTitleBar`, `PT:10142`): false for Compact
    /// with a Top bar, which draws no top lines and a fainter bottom line. The app sets it.
    pub attached: bool,
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
    /// How many tabs the bar has (Minimal: one tab is not tinted apart from the bar).
    pub tabs: usize,
}

/// A tab bar's thickness in points (`PT:12686-12739`): Minimal in a compact window with a top bar,
/// or Minimal with a side bar, is `minimal_tall` (`CompactMinimalTabBarHeight`); else `regular`.
pub fn bar_height(style: TabStyle, compact_window: bool, position: crate::tab_position::TabPosition, regular: f32, minimal_tall: f32) -> f32 {
    use crate::tab_position::{TabPosition, vertical};
    let tall = style == TabStyle::Minimal && (vertical(position) || (compact_window && position == TabPosition::Top));
    if tall {
        minimal_tall
    } else {
        regular
    }
}

/// A side bar's narrowest width in points (`RTV:2030-2044`): wider for Minimal and Compact.
pub fn side_minimum(style: TabStyle) -> f32 {
    use crate::tab_position::{MIN_SIDE_WIDTH, MIN_SIDE_WIDTH_MINIMAL};
    if matches!(style, TabStyle::Minimal | TabStyle::Compact) {
        MIN_SIDE_WIDTH_MINIMAL
    } else {
        MIN_SIDE_WIDTH
    }
}

/// The one background the window's other panes share (`PT:1919-1924`): every pane's background but
/// `current`'s own pane's, uniqued; Some only when exactly one distinct color remains. The divider
/// hides against it at prominence 0.
pub fn single_other_background<I: PartialEq + Copy>(panes: &[(I, Rgb)], current: Option<I>) -> Option<Rgb> {
    let mut others: Vec<Rgb> = Vec::new();
    for (_, c) in panes.iter().filter(|(id, _)| Some(*id) != current) {
        if !others.contains(c) {
            others.push(*c);
        }
    }
    (others.len() == 1).then(|| others[0])
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
/// resolve to; Minimal adds its own colors (`minimal_palette`).
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
                attached: true,
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
                line_inset: (0.5, 2.0),
                dark_edge_gradient: false,
                attached: true,
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
    if style == TabStyle::Minimal {
        minimal_palette(&mut p, i);
    }
    p
}

/// The Minimal style's colors (`PSMMinimalTabStyle`), over `p`, the palette of the style it
/// resolves to: the bar is the session's background (dimmed in a background window), the selected
/// tab is the bar, an unselected tab (and the bar's room beyond the tabs) differs by the
/// difference, and there are no lines (the notch outline replaces the edge).
fn minimal_palette(p: &mut TabPalette, i: &Inputs) {
    let k = i.minimal;
    let bar = dimmed(i.session_bg, i.dimming);
    // `nonSelectedTabColor` dims the already dimmed bar color once more (`MIN:218-244`).
    let apart = dimmed(non_selected(bar, k.difference), i.dimming);
    let opaque = |c: Rgb| Fill::new(c, 1.0);
    let unselected = if i.tabs > 1 { apart } else { bar };
    let clear = Fill::new(bar, 0.0);
    p.bar = bar;
    p.selected = opaque(bar);
    p.unselected = opaque(unselected);
    p.margin = opaque(apart);
    p.text_selected = minimal_text(bar, true, i.focused, k.legibility);
    p.text_unselected = minimal_text(unselected, false, i.focused, k.legibility);
    p.between = clear;
    p.edge = clear;
    p.top = clear;
    p.top_selected = clear;
    p.dark_edge_gradient = false;
    p.dark_controls = hsp(bar) < 0.5;
    p.minimal = Some(MinimalLook { outline: minimal_outline(bar, k.outline), left_inset_is_first_tab: k.left_inset_is_first_tab, underline: k.underline, deselected_colored_alpha: k.deselected_colored_alpha });
}

/// `colorByDimmingColor:` (`MIN:245-257`): every component moved toward 0.5 by `amount`.
pub fn dimmed(c: Rgb, amount: f32) -> Rgb {
    let mix = |v: u8| ((amount * 0.5 + (1.0 - amount) * f32::from(v) / 255.0) * 255.0 + 0.5).floor().clamp(0.0, 255.0) as u8;
    rgb(mix(c.r), mix(c.g), mix(c.b))
}

/// The dimming amount of a background window (`iTermDimmingAmount`, `PT:10038-10051`): none in the
/// key window or unless Dim background windows is on, else `dim::amount` of the
/// `SplitPaneDimmingAmount` (clamped to 0 to 0.9). Dim only text leaves the bar undimmed (`PT:10045`).
pub fn dimming(dim_background_windows: bool, focused: bool, amount: f32, only_text: bool) -> f32 {
    if only_text {
        return 0.0;
    }
    crate::dim::amount(false, crate::dim::background(dim_background_windows, focused), amount)
}

/// `psm_nonSelectedColorWithDifference:` (`MIN:13-26`): every component moved by `-difference`,
/// the sign flipped when the HSP brightness would leave 0 to 1. Rounded half up.
pub fn non_selected(c: Rgb, difference: f32) -> Rgb {
    let mut delta = -f64::from(difference);
    let proposed = f64::from(hsp(c)) + delta;
    if !(0.0..=1.0).contains(&proposed) {
        delta = -delta;
    }
    let mix = |v: u8| ((f64::from(v) / 255.0 + delta) * 255.0 + 0.5 + 1e-6).floor().clamp(0.0, 255.0) as u8;
    rgb(mix(c.r), mix(c.g), mix(c.b))
}

/// The text over a background (`textColorDefaultSelected:`, the opaque path, `MIN:98-156`): a
/// grey `min(1, b + delta)` over a dark background or `max(0, b - delta)` over a light one
/// (delta 0.85 selected, 0.5 not), at alpha 0.75 in the key window, else 0.5, raised to
/// `1 / legibility` when that is above 0.
pub fn minimal_text(bg: Rgb, selected: bool, focused: bool, legibility: f32) -> Fill {
    let b = hsp(bg);
    let delta = if selected { 0.85 } else { 0.5 };
    let value = if b < 0.5 { (b + delta).min(1.0) } else { (b - delta).max(0.0) };
    let base: f32 = if focused { 0.75 } else { 0.5 };
    let alpha = if legibility > 0.0 { base.powf(1.0 / legibility) } else { base };
    Fill::new(grey(value), alpha)
}

/// The outline drawn around the selected tab (`outlineColor`, `MIN:326-344`): white over a dark
/// bar, black over a light one, at `strength`.
pub fn minimal_outline(bar: Rgb, strength: f32) -> Fill {
    Fill::new(if hsp(bar) < 0.5 { WHITE } else { BLACK }, strength)
}

/// A colored tab's fill in Minimal (`cellBackgroundColorForTabColor:`, `MIN:937-946`): the tab
/// color itself, opaque when selected, else at `deselected_alpha`, halved in a window that is
/// not key. Laid over the tab's own background.
pub fn minimal_colored_fill(tab: Rgb, selected: bool, focused: bool, deselected_alpha: f32) -> Fill {
    let alpha = if selected { 1.0 } else { deselected_alpha };
    Fill::new(tab, if focused { alpha } else { alpha * 0.5 })
}

/// The two 2-point strokes under the selected colored tab.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Underline {
    pub outer: Fill,
    pub inner: Fill,
}

/// `drawPostHocDecorationsOnSelectedCell:` (`MIN:278-325`): alpha `(key ? 0.75 : 0.5) x
/// prominence`; when the tab and the bar differ in darkness the outer stroke is white at alpha
/// and the inner black at its square root, else the outer black at the square root and the inner
/// white.
pub fn minimal_underline(tab_dark: bool, bar_dark: bool, prominence: f32, focused: bool) -> Underline {
    let alpha = (if focused { 0.75 } else { 0.5 }) * prominence;
    if tab_dark != bar_dark {
        Underline { outer: Fill::new(WHITE, alpha), inner: Fill::new(BLACK, alpha.sqrt()) }
    } else {
        Underline { outer: Fill::new(BLACK, alpha.sqrt()), inner: Fill::new(WHITE, alpha) }
    }
}

/// The perceived brightness of a color in 0 to 1 (`perceivedBrightness`, `NCI:23-40`):
/// `0.30 r + 0.59 g + 0.11 b`. Not the HSP brightness of `hsp`.
pub fn perceived(c: Rgb) -> f32 {
    let n = |v: u8| f64::from(v) / 255.0;
    (0.30 * n(c.r) + 0.59 * n(c.g) + 0.11 * n(c.b)) as f32
}

/// `colorDimmedBy:towardsGrayLevel:` (`NCI:756-778`): each component `(1 - amount) c + amount
/// gray`, rounded half up.
pub fn toward_gray(c: Rgb, amount: f32, gray: f32) -> Rgb {
    let (a, g) = (f64::from(amount), f64::from(gray));
    let mix = |v: u8| (((1.0 - a) * f64::from(v) / 255.0 + a * g) * 255.0 + 0.5 + 1e-6).floor().clamp(0.0, 255.0) as u8;
    rgb(mix(c.r), mix(c.g), mix(c.b))
}

/// The split pane divider color (`terminalWindowDecorationControlColor`, `PT:1911-1950`). Minimal:
/// `invisible` (the one background every other session has, offered by the caller only when the
/// prominence is 0 and dimming is on) keeps the divider unseen; else the decoration background
/// `decoration_bg` (the session's unprocessed background) moved by `prominence` toward white when
/// its perceived brightness is under 0.5, toward black otherwise. Every other theme: light grey
/// (2/3) for the light styles, dark grey (1/3) for the dark ones, by the `resolved` style.
pub fn divider_color(style: TabStyle, resolved: TabStyle, decoration_bg: Rgb, prominence: f32, invisible: Option<Rgb>) -> Rgb {
    if style == TabStyle::Minimal {
        if prominence == 0.0
            && let Some(c) = invisible {
                return c;
            }
        let target = if perceived(decoration_bg) < 0.5 { 1.0 } else { 0.0 };
        return toward_gray(decoration_bg, prominence, target);
    }
    match resolved {
        TabStyle::Dark | TabStyle::DarkHighContrast => rgb(85, 85, 85),
        _ => rgb(170, 170, 170),
    }
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
/// `clamp(strength, 1, 3)`; over a tab whose `brightness` (`YOS:776-797`) is above 0.5 the outer stroke is white and the inner black,
/// else the reverse.
pub fn selected_outline(brightness: f32, strength: f32, focused: bool) -> Outline {
    let alpha = strength.clamp(0.0, 1.0) * if focused { 1.0 } else { 0.6 };
    let (outer, inner) = if brightness > 0.5 { (WHITE, BLACK) } else { (BLACK, WHITE) };
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
            tabs: 2,
        }
    }

    fn fill(r: u8, g: u8, b: u8, alpha: f32) -> Fill {
        Fill { color: rgb(r, g, b), alpha }
    }

    #[test]
    fn light_palette_follows_iterm2() {
        let p = palette(TabStyle::Light, &inputs(true));
        assert_eq!(p.line_inset, (0.5, 2.0), "`insetsForTabBarDividers`, `YOS:761`");
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
        // Automatic and Compact draw as what they resolve to (Minimal has its own colors).
        for style in [TabStyle::Automatic, TabStyle::Compact] {
            let auto = palette(style, &Inputs { system_dark: true, ..inputs(true) });
            assert_eq!(auto, TabPalette { style, ..palette(TabStyle::Dark, &inputs(true)) });
            assert_eq!(auto.resolved, TabStyle::Dark);
        }
        // Minimal keeps the resolved style's own parts: its `resolved` and label size.
        for (system_dark, want, pt) in [(false, TabStyle::Light, 11.0), (true, TabStyle::Dark, 11.0)] {
            let m = palette(TabStyle::Minimal, &Inputs { system_dark, ..inputs(true) });
            assert_eq!((m.resolved, m.font_pt), (want, pt));
        }
        let hc = palette(TabStyle::Minimal, &Inputs { system_high_contrast: true, ..inputs(true) });
        assert_eq!((hc.resolved, hc.font_pt), (resolved(TabStyle::Minimal, false, true), font_pt(resolved(TabStyle::Minimal, false, true))));
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
        assert_eq!(selected_outline(hsp(rgb(240, 240, 240)), 0.5, true), Outline { width: 1.0, outer: white(0.5), inner: black(0.5) });
        assert_eq!(selected_outline(hsp(rgb(20, 20, 20)), 2.0, false), Outline { width: 2.0, outer: black(0.6), inner: white(0.6) });
        assert_eq!(selected_outline(hsp(rgb(20, 20, 20)), 0.0, true).outer.alpha, 0.0);
        // The brightness is passed in (a clear selected tab's is the back's, `YOS:776-797`), and 0.5 is not above 0.5.
        assert_eq!(selected_outline(0.6, 1.0, true).outer, white(1.0));
        assert_eq!(selected_outline(0.5, 1.0, true).outer, black(1.0));
    }

    #[test]
    fn minimal_colors_follow_iterm2() {
        let (black, white, grey) = (rgb(0, 0, 0), rgb(255, 255, 255), rgb(128, 128, 128));
        assert_eq!(non_selected(black, 0.05), rgb(13, 13, 13), "proposed brightness below 0 flips the sign");
        assert_eq!(non_selected(white, 0.05), rgb(242, 242, 242));
        assert_eq!(non_selected(grey, 0.05), rgb(115, 115, 115));
        let p = palette(TabStyle::Minimal, &inputs(true));
        assert_eq!(p.bar, black);
        assert_eq!(p.selected, fill(0, 0, 0, 1.0));
        assert_eq!(p.unselected, fill(13, 13, 13, 1.0), "two tabs: the bar moved by the difference");
        assert_eq!(p.margin, fill(13, 13, 13, 1.0), "the room beyond the tabs is the unselected color");
        assert!(p.dark_controls);
        assert_eq!(p.resolved, TabStyle::Light, "the other parts draw as Regular");
        let one = palette(TabStyle::Minimal, &Inputs { tabs: 1, ..inputs(true) });
        assert_eq!(one.unselected, fill(0, 0, 0, 1.0), "one tab: the bar color");
        assert_eq!(one.margin, fill(13, 13, 13, 1.0));
        for line in [p.between, p.edge, p.top, p.top_selected] {
            assert_eq!(line.alpha, 0.0, "Minimal draws no lines");
        }
        assert!(!p.dark_edge_gradient);
        assert_eq!(p.minimal, Some(MinimalLook { outline: fill(255, 255, 255, 0.2), left_inset_is_first_tab: false, underline: 1.0, deselected_colored_alpha: 0.5 }));
        assert_eq!(p.text_selected, fill(217, 217, 217, 0.75));
        assert_eq!(p.text_unselected, minimal_text(rgb(13, 13, 13), false, true, 1.0));
        // A light session background, a window that is not key.
        let light = palette(TabStyle::Minimal, &Inputs { session_bg: white, focused: false, ..inputs(false) });
        assert_eq!(light.bar, white);
        assert_eq!(light.text_selected, fill(38, 38, 38, 0.5));
        assert_eq!(light.minimal.unwrap().outline, fill(0, 0, 0, 0.2));
        // Other themes have no Minimal parts.
        assert_eq!(palette(TabStyle::Compact, &inputs(true)).minimal, None);
        // The text (`MIN:98-156`).
        assert_eq!(minimal_text(black, true, true, 1.0), fill(217, 217, 217, 0.75));
        assert_eq!(minimal_text(black, false, true, 1.0), fill(128, 128, 128, 0.75));
        assert_eq!(minimal_text(white, true, false, 1.0), fill(38, 38, 38, 0.5));
        assert!((minimal_text(black, true, true, 2.0).alpha - 0.866_025_4).abs() < 1e-5, "0.75^(1/2)");
        assert_eq!(minimal_text(black, true, true, 0.0).alpha, 0.75, "no adjustment at 0");
        // The outline, dimming.
        assert_eq!(minimal_outline(black, 0.2), fill(255, 255, 255, 0.2));
        assert_eq!(minimal_outline(white, 0.2), fill(0, 0, 0, 0.2));
        assert_eq!(dimmed(black, 0.4), rgb(51, 51, 51));
        assert_eq!(dimmed(white, 0.0), white);
        assert_eq!(dimming(true, false, 0.4, false), 0.4);
        assert_eq!(dimming(true, false, 0.4, true), 0.0, "Dim only text leaves the tab bar alone (PT:10045)");
        assert_eq!(dimming(true, true, 0.4, false), 0.0);
        assert_eq!(dimming(false, false, 0.4, false), 0.0);
        assert_eq!(dimming(true, false, 0.95, false), 0.9);
        assert_eq!(dimming(true, false, -1.0, false), 0.0);
        // A background window's bar is dimmed, and its unselected tabs dimmed once more (`MIN:218-244`).
        let dim = palette(TabStyle::Minimal, &Inputs { dimming: 0.4, ..inputs(false) });
        assert_eq!(dim.bar, rgb(51, 51, 51));
        assert_eq!(dim.unselected.color, dimmed(non_selected(rgb(51, 51, 51), 0.05), 0.4));
    }

    #[test]
    fn minimal_colored_follows_iterm2() {
        let red = rgb(255, 0, 0);
        assert_eq!(minimal_colored_fill(red, true, true, 0.5), fill(255, 0, 0, 1.0));
        assert_eq!(minimal_colored_fill(red, false, true, 0.5), fill(255, 0, 0, 0.5));
        assert_eq!(minimal_colored_fill(red, false, false, 0.5), fill(255, 0, 0, 0.25));
        assert_eq!(minimal_colored_fill(red, true, false, 0.5), fill(255, 0, 0, 0.5));
        let (white, black) = (rgb(255, 255, 255), rgb(0, 0, 0));
        let u = minimal_underline(false, true, 1.0, true);
        assert_eq!(u, Underline { outer: Fill { color: white, alpha: 0.75 }, inner: Fill { color: black, alpha: 0.75f32.sqrt() } });
        let u = minimal_underline(true, true, 1.0, false);
        assert_eq!(u, Underline { outer: Fill { color: black, alpha: 0.5f32.sqrt() }, inner: Fill { color: white, alpha: 0.5 } });
        assert_eq!(minimal_underline(false, true, 0.5, true).outer.alpha, 0.375, "the prominence scales the alpha");
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

    #[test]
    fn divider_color_follows_iterm2() {
        use TabStyle::*;
        let (black, white) = (rgb(0, 0, 0), rgb(255, 255, 255));
        let light = rgb(170, 170, 170);
        assert_eq!(divider_color(Automatic, Light, black, 0.15, None), light, "lightGrayColor, 2/3");
        assert_eq!(divider_color(Light, Light, black, 0.15, None), light);
        assert_eq!(divider_color(LightHighContrast, LightHighContrast, black, 0.15, None), light);
        assert_eq!(divider_color(Dark, Dark, white, 0.15, None), rgb(85, 85, 85), "darkGrayColor, 1/3");
        assert_eq!(divider_color(DarkHighContrast, DarkHighContrast, white, 0.15, None), rgb(85, 85, 85));
        assert_eq!(divider_color(Compact, Dark, white, 0.15, None), rgb(85, 85, 85), "Compact resolves like Regular");
        assert_eq!(divider_color(Minimal, Dark, black, 0.15, None), rgb(38, 38, 38), "dark background moves toward white");
        assert_eq!(divider_color(Minimal, Light, white, 0.15, None), rgb(217, 217, 217), "light background moves toward black");
        assert_eq!(divider_color(Minimal, Dark, black, 0.0, Some(rgb(20, 20, 20))), rgb(20, 20, 20), "invisible at prominence 0");
        assert_eq!(divider_color(Minimal, Dark, black, 0.0, None), black);
        assert_eq!(divider_color(Minimal, Dark, black, 0.15, Some(rgb(20, 20, 20))), rgb(38, 38, 38), "invisible only at prominence 0");
        assert!((perceived(rgb(255, 0, 0)) - 0.30).abs() < 1e-6);
        assert!((perceived(rgb(0, 255, 0)) - 0.59).abs() < 1e-6);
        assert_eq!(toward_gray(rgb(100, 100, 100), 0.0, 1.0), rgb(100, 100, 100));
    }

    #[test]
    fn bar_height_follows_iterm2() {
        use crate::tab_position::TabPosition::*;
        use TabStyle::*;
        assert_eq!(bar_height(Minimal, true, Top, 24.0, 38.0), 38.0);
        assert_eq!(bar_height(Minimal, false, Top, 24.0, 38.0), 24.0);
        assert_eq!(bar_height(Minimal, false, Left, 24.0, 38.0), 38.0, "Minimal side rows");
        assert_eq!(bar_height(Compact, true, Top, 24.0, 38.0), 24.0);
        assert_eq!(bar_height(Minimal, true, Bottom, 24.0, 38.0), 24.0);
    }

    #[test]
    fn side_minimum_follows_iterm2() {
        use TabStyle::*;
        assert_eq!((side_minimum(Minimal), side_minimum(Compact)), (114.0, 114.0));
        assert_eq!((side_minimum(Automatic), side_minimum(DarkHighContrast)), (50.0, 50.0));
    }

    #[test]
    fn single_other_background_follows_iterm2() {
        let (a, b) = (rgb(1, 1, 1), rgb(2, 2, 2));
        // The window's other panes (every tab's, the focused one left out) sharing one background.
        assert_eq!(single_other_background(&[(1, a), (2, b), (3, b)], Some(1)), Some(b));
        assert_eq!(single_other_background(&[(1, a), (2, b), (3, a)], Some(1)), None, "a second distinct background");
        assert_eq!(single_other_background(&[(1, a)], Some(1)), None, "only the focused pane");
        assert_eq!(single_other_background::<u32>(&[], None), None, "no panes");
        // The current pane's own background does not count, whatever it is.
        assert_eq!(single_other_background(&[(1, b), (2, a), (3, a)], Some(1)), Some(a));
    }
}
