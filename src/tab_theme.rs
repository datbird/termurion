//! The Theme setting (Appearance > General, `TabStyleWithAutomaticOption`): which tab style a
//! window draws, and the fixed style (Light, Dark or a High Contrast one) it resolves to.
//! 13e extends this module (the forced appearance, the palettes, Minimal and Compact).

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

/// Whether Windows is in a high contrast theme (`SPI_GETHIGHCONTRAST`, flag `HCF_HIGHCONTRASTON`).
#[cfg(windows)]
pub fn system_high_contrast() -> bool {
    use windows_sys::Win32::UI::Accessibility::{HCF_HIGHCONTRASTON, HIGHCONTRASTW};
    use windows_sys::Win32::UI::WindowsAndMessaging::{SPI_GETHIGHCONTRAST, SystemParametersInfoW};
    let mut hc = HIGHCONTRASTW { cbSize: std::mem::size_of::<HIGHCONTRASTW>() as u32, dwFlags: 0, lpszDefaultScheme: std::ptr::null_mut() };
    // SAFETY: `hc` is a valid HIGHCONTRASTW with its size set, as the call requires.
    let ok = unsafe { SystemParametersInfoW(SPI_GETHIGHCONTRAST, hc.cbSize, &mut hc as *mut _ as *mut _, 0) };
    ok != 0 && hc.dwFlags & HCF_HIGHCONTRASTON != 0
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
}
