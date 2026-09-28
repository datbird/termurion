//! Settings > Advanced, the pane's model: iTerm2's categories and the keys it keeps outside every pane.

/// iTerm2's Advanced categories, the SECTION_* values in `iTermAdvancedSettingsModel.m` order.
pub const CATEGORIES: [&str; 19] = [
    "Tabs",
    "Mouse",
    "Terminal",
    "Hotkey",
    "General",
    "Drawing",
    "Semantic History",
    "Debugging",
    "Session",
    "Windows",
    "Tmux Integration",
    "SSH Integration",
    "Warnings",
    "Pasteboard",
    "Tip of the Day",
    "Badge",
    "Web Browser",
    "Experimental Features",
    "Scripting",
];

/// The tab of keys iTerm2 keeps outside every settings pane: a dialog's remembered value, a menu
/// toggle, a warning's silence. No page draws it.
pub const STORED_TAB: &str = "Stored";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{self, Kind};

    #[test]
    fn advanced_rows_use_iterm2_categories() {
        for s in schema::global().iter().filter(|s| s.tab == "Advanced") {
            assert!(CATEGORIES.contains(&s.section.as_str()), "{} has section {}", s.key, s.section);
        }
        let expect = [
            ("defaultWideMode", "Terminal"),
            ("nativeRenderingCSSDark", "Drawing"),
            ("nativeRenderingCSSLight", "Drawing"),
            ("disableTopRightIndicators", "Drawing"),
            ("silentUserNotifications", "General"),
            ("simpleNotifications", "General"),
            ("CommandHistoryUsePower", "General"),
            ("CommandHistoryAgePower", "General"),
            ("AutocompleteMaxOptions", "General"),
            ("sessionEndMessageText", "Session"),
            ("showAutomaticProfileSwitchingBanner", "Session"),
            ("preserveFontSizeOnAutomaticProfileSwitch", "Session"),
            ("fileDropCoprocess", "Terminal"),
            ("StripZeroWidthFormatCharactersOnPaste", "Terminal"),
            ("openProfilesInNewWindow", "Tabs"),
            ("moveLeftAfterClosingTab", "Tabs"),
            ("EscapeWithQuotes", "Experimental Features"),
            // iTerm2 defines it among the experimental ones but with SECTION_GENERAL (ASM:924).
            ("GenerativeAIAllowed", "General"),
            ("clippingSeparator", "Pasteboard"),
            ("toolbeltFont", "Windows"),
            ("toolbeltFontSize", "Windows"),
        ];
        for (key, section) in expect {
            let s = schema::find_global(key).unwrap_or_else(|| panic!("{key}"));
            assert_eq!((s.tab.as_str(), s.section.as_str()), ("Advanced", section), "{key}");
        }
    }

    #[test]
    fn stored_rows_follow_iterm2() {
        let int = |i: i64| Some(toml::Value::Integer(i));
        let float = |f: f64| Some(toml::Value::Float(f));
        let bool = |b: bool| Some(toml::Value::Boolean(b));
        let text = |t: &str| Some(toml::Value::String(t.into()));
        let expect = [
            ("PasteSpecialChunkSize", "Advanced Paste", Kind::Int, int(1024)),
            ("PasteSpecialChunkDelay", "Advanced Paste", Kind::Float, float(0.01)),
            ("NumberOfSpacesPerTab", "Advanced Paste", Kind::Int, int(4)),
            ("TabTransform", "Advanced Paste", Kind::Int, int(0)),
            ("EscapeShellCharsWithBackslash", "Advanced Paste", Kind::Bool, bool(false)),
            ("ConvertUnicodePunctuation", "Advanced Paste", Kind::Bool, bool(false)),
            ("ConvertDosNewlines", "Advanced Paste", Kind::Bool, bool(true)),
            ("RemoveControlCodes", "Advanced Paste", Kind::Bool, bool(true)),
            ("BracketedPasteMode", "Advanced Paste", Kind::Bool, bool(true)),
            ("PasteSpecialUseRegexSubstitution", "Advanced Paste", Kind::Bool, bool(false)),
            ("PasteSpecialRegex", "Advanced Paste", Kind::String, text("")),
            ("PasteSpecialSubstitution", "Advanced Paste", Kind::String, text("")),
            ("PasteTabToStringTabStopSize", "Paste", Kind::Int, int(4)),
            ("RequireAuthenticationToOpenPasswordManager", "Password Manager", Kind::Bool, bool(true)),
            ("RequireAuthenticationAfterScreenLocks", "Password Manager", Kind::Bool, bool(false)),
            ("RequireAuthenticationOnEveryPasswordManagerOpen", "Password Manager", Kind::Bool, bool(false)),
            ("NoSyncTmuxHung", "Warnings", Kind::Bool, bool(false)),
            ("NoSyncAutoBurialReveal", "Warnings", Kind::Bool, bool(false)),
            ("NoSyncEnableAutomaticProfileSwitchingLogging", "Script Console", Kind::Bool, bool(false)),
            ("Use background pattern indicator", "Broadcast", Kind::Bool, bool(false)),
        ];
        for (key, section, kind, default) in &expect {
            let s = schema::find_global(key).unwrap_or_else(|| panic!("{key}"));
            assert_eq!((s.tab.as_str(), s.section.as_str(), s.kind), (STORED_TAB, *section, *kind), "{key}");
            assert_eq!(&s.default, default, "{key}");
        }
        let stored = schema::global().iter().filter(|s| s.tab == STORED_TAB).count();
        assert_eq!(stored, expect.len());
    }
}
