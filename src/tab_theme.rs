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
}
