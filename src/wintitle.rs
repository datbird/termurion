//! Window title rules: which text a window's caption shows.

/// The title of a window with nothing else to show.
pub const UNTITLED: &str = "Untitled";

/// The text a window's caption shows before its number and client name (iTerm2's
/// `undecoratedWindowTitle`, `PT:13636-13644`): a custom title whose FORMAT is not empty (Edit Window Title or
/// the profile's), even when it evaluates to nothing; with the tab bar hidden and `show_window_title`
/// (ShowWindowTitleWhenTabBarInvisible) off, the session's title; else the window title the
/// session's program set, falling back to its session title. "Untitled" when the chosen one
/// is missing.
pub fn window_title(custom: Option<&str>, bar_visible: bool, show_window_title: bool, window_title: Option<&str>, session_title: Option<&str>) -> String {
    if let Some(c) = custom {
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

/// Menu ids of the Window menu's window list: the prefix, then the window's registry key.
pub const LIST_PREFIX: &str = "window-list:";

/// The words for SwitchWindowModifier's tag (`WSL:103-122`): None for no modifier (9); Win
/// (4, Cmd), Alt (5, Option), Alt+Win (6, Cmd+Option) and Ctrl (1) in macOS's glyph order
/// (control, option, command); an unknown tag has no words.
pub fn modifiers(tag: i64) -> Option<&'static str> {
    match tag {
        9 => None,
        4 => Some("Win+"),
        5 => Some("Alt+"),
        6 => Some("Alt+Win+"),
        1 => Some("Ctrl+"),
        _ => Some(""),
    }
}

/// The window number label (`WSL stringForOrdinal:`): the switch chord for windows 1 to 9,
/// the bare number from 10 on or with no switch modifier.
pub fn shortcut_label(tag: i64, number: usize) -> String {
    let ordinal = number + 1;
    match modifiers(tag) {
        Some(mods) if ordinal < 10 => format!("{mods}{ordinal}"),
        _ => ordinal.to_string(),
    }
}

/// The "N. " a window's title starts with (`PT:3105-3122`): with Show window number on, when
/// the label cannot show it (UseShortcutAccessoryViewController off, or no title bar).
pub fn number_prefix(show_number: bool, accessory: bool, titled: bool, number: usize) -> Option<String> {
    (show_number && !(accessory && titled)).then(|| format!("{}. ", number + 1))
}

/// The window has an OS title bar to carry the number label: decorated, not full screen, and not
/// a Compact or Minimal window (whose own caption draws it, `compact_caption`).
pub fn has_title_bar(decorated: bool, full_screen: bool, compact: bool) -> bool {
    decorated && !full_screen && !compact
}

/// The OS caption: the title with its "N. " prefix, or, where iTerm2 shows the label in a
/// title bar accessory, the title and the label after four spaces (R11).
pub fn caption(base: &str, show_number: bool, accessory: bool, titled: bool, tag: i64, number: usize) -> String {
    if !show_number {
        return base.to_string();
    }
    match number_prefix(show_number, accessory, titled, number) {
        Some(prefix) => format!("{prefix}{base}"),
        None => format!("{base}    {}", shortcut_label(tag, number)),
    }
}

/// What a Compact or Minimal window's own caption draws (R11): its title, and the label
/// right-aligned when the accessory shows one; the "N. " prefix instead with the accessory
/// off.
pub fn compact_caption(base: &str, show_number: bool, accessory: bool, tag: i64, number: usize) -> (String, Option<String>) {
    match (show_number, accessory) {
        (true, true) => (base.to_string(), Some(shortcut_label(tag, number))),
        (true, false) => (format!("{}. {base}", number + 1), None),
        _ => (base.to_string(), None),
    }
}

/// A window's Window menu entry (`PT titleForWindowMenu`, `:3069-3084`): with
/// IncludeShortcutInWindowsMenu and a switch modifier, `title` (the title without the number)
/// and the chord for 1 to 9; from 10 on `text` (the window's own title, its "N. " too) and
/// the number; `text` alone otherwise.
pub fn menu_entry(text: &str, title: &str, include_shortcut: bool, tag: i64, number: usize) -> String {
    let Some(mods) = modifiers(tag).filter(|_| include_shortcut) else { return text.to_string() };
    let ordinal = number + 1;
    if ordinal >= 10 { format!("{text} \u{2014} {ordinal}") } else { format!("{title} \u{2014} {mods}{ordinal}") }
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
        // A custom title wins (PT:13636-13644), even one that evaluated to nothing (the caller
        // passes None for an empty format).
        assert_eq!(window_title(Some("build"), true, true, Some("vim"), Some("zsh")), "build");
        assert_eq!(window_title(Some(""), true, true, Some("vim"), Some("zsh")), "");
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
    fn number_label_follows_iterm2() {
        // SwitchWindowModifier's tags (WSL:103-122): control, option, command in that order.
        assert_eq!(modifiers(6), Some("Alt+Win+"));
        assert_eq!(modifiers(9), None);
        assert_eq!(modifiers(1), Some("Ctrl+"));
        assert_eq!(modifiers(4), Some("Win+"));
        assert_eq!(modifiers(5), Some("Alt+"));
        assert_eq!(modifiers(3), Some(""));
        // The label (WSL stringForOrdinal:): the chord for 1 to 9, the bare number from 10
        // on or with no modifier.
        assert_eq!(shortcut_label(6, 0), "Alt+Win+1");
        assert_eq!(shortcut_label(6, 8), "Alt+Win+9");
        assert_eq!(shortcut_label(6, 9), "10");
        assert_eq!(shortcut_label(9, 0), "1");
        // "N. " only when the label cannot show (PT:3105-3122).
        assert_eq!(number_prefix(true, true, true, 0), None);
        assert_eq!(number_prefix(true, false, true, 0).as_deref(), Some("1. "));
        assert_eq!(number_prefix(true, true, false, 2).as_deref(), Some("3. "));
        assert_eq!(number_prefix(false, false, false, 0), None);
        // R11: the label ends the caption after four spaces.
        assert_eq!(caption("vim", true, true, true, 6, 0), "vim    Alt+Win+1");
        assert_eq!(caption("vim", true, true, true, 6, 9), "vim    10");
        assert_eq!(caption("vim", true, true, true, 9, 0), "vim    1");
        assert_eq!(caption("vim", true, true, false, 6, 0), "1. vim");
        // A compact window has no OS title bar: its taskbar caption keeps the "N. " prefix.
        assert!(!has_title_bar(true, false, true) && has_title_bar(true, false, false) && !has_title_bar(true, true, false) && !has_title_bar(false, false, false));
        assert_eq!(caption("vim", true, true, has_title_bar(true, false, true), 6, 0), "1. vim");
        assert_eq!(caption("vim", true, false, true, 6, 0), "1. vim");
        assert_eq!(caption("vim", false, true, true, 6, 0), "vim");
    }

    #[test]
    fn compact_caption_follows_r11() {
        assert_eq!(compact_caption("vim", true, true, 6, 0), ("vim".to_string(), Some("Alt+Win+1".to_string())));
        assert_eq!(compact_caption("vim", true, true, 6, 9), ("vim".to_string(), Some("10".to_string())));
        assert_eq!(compact_caption("vim", true, false, 6, 0), ("1. vim".to_string(), None));
        assert_eq!(compact_caption("vim", false, true, 6, 0), ("vim".to_string(), None));
    }

    #[test]
    fn window_menu_entry_follows_iterm2() {
        // PT:3069-3084: the title without the number and the chord for 1 to 9.
        assert_eq!(menu_entry("vim", "vim", true, 6, 0), "vim \u{2014} Alt+Win+1");
        assert_eq!(menu_entry("1. vim", "vim", true, 6, 0), "vim \u{2014} Alt+Win+1");
        // From 10 on, and with no modifier or the key off, the window's own title (its "N. "
        // too, when the caption has one).
        assert_eq!(menu_entry("vim", "vim", true, 6, 9), "vim \u{2014} 10");
        assert_eq!(menu_entry("10. vim", "vim", true, 6, 9), "10. vim \u{2014} 10");
        assert_eq!(menu_entry("vim", "vim", true, 9, 0), "vim");
        assert_eq!(menu_entry("1. vim", "vim", true, 9, 0), "1. vim");
        assert_eq!(menu_entry("vim", "vim", false, 6, 0), "vim");
        assert!(LIST_PREFIX.ends_with(':'));
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
