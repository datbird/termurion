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

/// The window title format a window's initial profile gives it (`PT:887-890`): the custom
/// text with `Use Custom Window Title`, a blank title (" ") when that text is empty.
pub fn custom_window_format(use_custom: bool, text: &str) -> Option<String> {
    use_custom.then(|| if text.is_empty() { " ".to_string() } else { text.to_string() })
}

/// The title format a new tab takes from its profile (`PT:15265-15269`): the custom text with
/// `Use Custom Tab Title`, even empty (the automatic title then shows); never for a tmux tab,
/// which keeps tmux's window name.
pub fn custom_tab_format(use_custom: bool, text: &str, tmux: bool) -> Option<String> {
    (use_custom && !tmux).then(|| text.to_string())
}

/// Where a window's scope finds its focused session.
pub const WINDOW_SESSION: &str = "currentTab.currentSession.";
/// Where a tab's scope finds its focused session.
pub const TAB_SESSION: &str = "currentSession.";

/// A name in a window's or tab's scope (`own`, from `api_vars::scope_values`), or in its
/// focused session's (`session`) under `session_prefix`, as text.
pub fn scoped_value(name: &str, own: &crate::api_vars::Values, session: &crate::api_vars::Values, session_prefix: &str) -> Option<String> {
    let value = match name.strip_prefix(session_prefix) {
        Some(rest) => session.get(rest),
        None => own.get(name),
    }?;
    Some(match value {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    })
}

/// A title format (an interpolated string) evaluated as iTerm2's title swifty strings are
/// (`PT:894-902`): an error shows "\u{1f41e} " and the error; `lax` is
/// `laxNilPolicyInInterpolatedStrings`. A format with a function call is shown as written (the
/// caption is rebuilt every frame, with no room for an asynchronous call); one without a
/// backslash is returned as is, with nothing looked up.
pub fn evaluate(format: &str, lookup: &dyn Fn(&str) -> Option<String>, lax: bool) -> String {
    if !format.contains('\\') || crate::interp::has_calls(format) {
        return format.to_string();
    }
    match crate::interp::evaluate_lax(format, lookup, &[], lax) {
        Ok(text) => text,
        Err(e) => format!("\u{1f41e} {e}"),
    }
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

    #[test]
    fn custom_titles_follow_iterm2() {
        // The window's initial profile (PT:887-890): an empty text is a blank title.
        assert_eq!(custom_window_format(true, "Build").as_deref(), Some("Build"));
        assert_eq!(custom_window_format(true, "").as_deref(), Some(" "));
        assert_eq!(custom_window_format(false, "Build"), None);
        // A new tab's profile (PT:15265-15269): never a tmux tab, which keeps tmux's name.
        assert_eq!(custom_tab_format(true, "t", false).as_deref(), Some("t"));
        assert_eq!(custom_tab_format(true, "t", true), None);
        assert_eq!(custom_tab_format(false, "t", false), None);
        // An empty override, which then shows the automatic title.
        assert_eq!(custom_tab_format(true, "", false).as_deref(), Some(""));
    }

    #[test]
    fn title_interpolation_follows_iterm2() {
        let lookup = |n: &str| match n {
            "session.name" => Some("zsh".to_string()),
            "currentTab.currentSession.path" => Some("C:\\src".to_string()),
            _ => None,
        };
        assert_eq!(evaluate("\\(currentTab.currentSession.path)", &lookup, true), "C:\\src");
        assert_eq!(evaluate("W: \\(user.x)", &lookup, true), "W: ");
        assert_eq!(evaluate("plain", &lookup, true), "plain");
        // An error shows the bug and the error (PT:894-902): strict nil, a syntax error.
        assert!(evaluate("\\(nosuch)", &lookup, false).starts_with("\u{1f41e} "));
        assert!(evaluate("\\(1 +)", &lookup, true).starts_with("\u{1f41e} "));
        // The rest of an unclosed \( is text (EP:343-407), not an error.
        assert_eq!(evaluate("\\(nosuch", &lookup, true), "\\(nosuch");
        // A function call is shown as written (Decision).
        assert_eq!(evaluate("\\(f())", &lookup, true), "\\(f())");
        // A window scope reads its focused session under currentTab.currentSession.
        let own: crate::api_vars::Values = [("number".to_string(), serde_json::json!(2))].into_iter().collect();
        let session: crate::api_vars::Values = [("path".to_string(), serde_json::json!("C:\\src"))].into_iter().collect();
        assert_eq!(scoped_value("number", &own, &session, WINDOW_SESSION).as_deref(), Some("2"));
        assert_eq!(scoped_value("currentTab.currentSession.path", &own, &session, WINDOW_SESSION).as_deref(), Some("C:\\src"));
        assert_eq!(scoped_value("currentSession.path", &own, &session, TAB_SESSION).as_deref(), Some("C:\\src"));
        assert_eq!(scoped_value("path", &own, &session, TAB_SESSION), None);
    }
}
