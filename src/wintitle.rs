//! Window title rules: which text a window's caption shows.

/// The title of a window with nothing else to show.
pub const UNTITLED: &str = "Untitled";

/// The text a window's caption shows before its number and client name (iTerm2's
/// `undecoratedWindowTitle`, `PT:13636-13644`): a non-empty custom title (Edit Window Title or
/// the profile's); with the tab bar hidden and `show_window_title`
/// (ShowWindowTitleWhenTabBarInvisible) off, the session's title; else the window title the
/// session's program set, falling back to its session title. "Untitled" when the chosen one
/// is missing.
pub fn window_title(custom: Option<&str>, bar_visible: bool, show_window_title: bool, window_title: Option<&str>, session_title: Option<&str>) -> String {
    if let Some(c) = custom.filter(|c| !c.is_empty()) {
        return c.to_string();
    }
    let chosen = if !bar_visible && !show_window_title { session_title } else { window_title };
    chosen.unwrap_or(UNTITLED).to_string()
}

/// Whether a title a program sets goes to every session of its window: when each tab may
/// not have its own window title (SeparateWindowTitlePerTab off, `PT:15578-15584`).
pub fn shares_title(separate_per_tab: bool) -> bool {
    !separate_per_tab
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_title_follows_iterm2() {
        // A custom title wins (PT:13636-13644); an empty one is no custom title.
        assert_eq!(window_title(Some("build"), true, true, Some("vim"), Some("zsh")), "build");
        assert_eq!(window_title(Some(""), true, true, Some("vim"), Some("zsh")), "vim");
        assert_eq!(window_title(None, true, true, Some("vim"), Some("zsh")), "vim");
        // The bar hidden and ShowWindowTitleWhenTabBarInvisible off: the session's title.
        assert_eq!(window_title(None, false, false, Some("vim"), Some("zsh")), "zsh");
        assert_eq!(window_title(None, false, true, Some("vim"), Some("zsh")), "vim");
        assert_eq!(window_title(None, true, false, Some("vim"), Some("zsh")), "vim");
        assert_eq!(window_title(None, false, false, Some("vim"), None), UNTITLED);
        assert_eq!(window_title(None, true, true, None, None), "Untitled");
        // SeparateWindowTitlePerTab off shares a program's title (PT:15578-15584).
        assert!(shares_title(false));
        assert!(!shares_title(true));
    }
}
