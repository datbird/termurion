//! Browser profiles and the browser pane's model (milestone 13x, Task 5): pure. A browser pane is
//! a `Pane` whose `Io` is `Io::Browser(BrowserIo)`; this file holds what decides its profile
//! type, its rows on the profile page, its start, its session name and what an arrangement may
//! keep of it (a sanitized, bounded URL: never cookies or page content).

use crate::jobtitle::BROWSER_HIDDEN;
use crate::webhost::Store;

/// `Custom Command`'s value for a browser profile (`PGP:268-290`).
pub const BROWSER_VALUE: &str = "Browser";
pub const DEFAULT_URL: &str = "iterm2-about:welcome";
pub const DEFAULT_NAME: &str = "Web Browser";
/// The fixed browser user (`BVC:182`).
pub const USER_ID: &str = "AC0E9812-7F88-478B-B361-5526082EDDB3";
/// The key of a session's table in an arrangement that holds the page's URL.
pub const URL_KEY: &str = "Browser URL";
/// The longest URL Tandem keeps or loads from a setting.
pub const MAX_URL: usize = 2048;
/// The longest session name a page's title gives.
const TITLE_CHARS: usize = 200;

/// What a browser pane shows of its page; the view itself is the app's.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BrowserIo {
    pub url: String,
    pub title: String,
    pub loading: bool,
    pub can_back: bool,
    pub can_forward: bool,
    pub favicon: Option<Vec<u8>>,
    pub dev_null: bool,
    pub zoom: f64,
}

/// What starting a pane with a profile makes.
#[derive(Clone, Debug, PartialEq)]
pub enum Start {
    Browser(String),
    Terminal,
}

pub fn is_browser(custom_command: &str) -> bool {
    custom_command == BROWSER_VALUE
}

/// The Profile type control's index: 1 for Browser, else 0 (`PGP:268-290`).
pub fn profile_type(custom_command: &str) -> i64 {
    i64::from(is_browser(custom_command))
}

/// The `Custom Command` value a pick writes and whether Icon becomes Automatic (`PGP:270-285`).
pub fn set_profile_type(v: i64) -> (&'static str, bool) {
    if v == 1 { (BROWSER_VALUE, true) } else { ("No", false) }
}

/// Whether a profile page row shows (`OptionalEnclosure.swift`): `browser` is whether the profile
/// is a browser profile that runs as one (the type and `BrowserProfiles`).
pub fn settings_visible(mode: Option<&str>, hidden_mode: bool, browser: bool, browser_profiles: bool) -> bool {
    if hidden_mode && !browser_profiles {
        return false;
    }
    match mode {
        Some("terminal") => !browser,
        Some("browser") => browser,
        _ => true,
    }
}

/// Whether a profile tab has a visible row (tabs with none are removed in browser mode).
pub fn tab_visible(tab: &str, browser: bool, browser_profiles: bool) -> bool {
    crate::schema::profile().iter().any(|s| s.tab == tab && settings_visible(s.mode.as_deref(), s.hidden_mode, browser, browser_profiles))
}

/// The session name after a title or a finished navigation (`BST:24-43`): None keeps the name.
pub fn resolved_name(page_title: Option<&str>, host: Option<&str>, profile_name: &str, settled: bool) -> Option<String> {
    let title = page_title.unwrap_or("").trim();
    if !title.is_empty() {
        return Some(title.to_string());
    }
    if !settled {
        return None;
    }
    match host.filter(|h| !h.trim().is_empty()) {
        Some(h) => Some(h.to_string()),
        None => Some(profile_name.to_string()),
    }
}

/// Whether a Title components item is hidden for a profile (`BST:46-66`): terminal-only
/// components for a browser profile, unless one is selected.
pub fn component_hidden(tag: u32, browser: bool, selected: u32) -> bool {
    if !browser || tag == 0 || tag & BROWSER_HIDDEN == 0 {
        return false;
    }
    tag & selected == 0
}

/// A page's title as a session name: another party's text, so control, bidirectional and
/// zero-width characters go and the length is bounded.
pub fn shown_title(title: &str) -> String {
    let clean: Vec<char> = crate::inert_files::clean_name(title).chars().collect();
    if clean.len() <= TITLE_CHARS { clean.into_iter().collect() } else { clean[..TITLE_CHARS].iter().chain(['\u{2026}'].iter()).collect() }
}

/// A browser profile's window in cells: its points, one point to the cell (`TWSH:170-176`).
pub fn window_cells(width_pts: i64, height_pts: i64) -> (u16, u16) {
    (width_pts.clamp(1, 1250) as u16, height_pts.clamp(1, 1250) as u16)
}

/// What a screen reader is told a browser pane is (B7): its title as a session name.
pub fn access_label(title: &str) -> String {
    format!("Browser: {}", shown_title(title))
}

fn clean_text(s: &str) -> bool {
    crate::inert_files::clean_name(s) == s
}

/// The URL a profile's Initial URL or a restored one becomes: empty is the welcome page; a text
/// with no scheme is a host (https); a scheme that is not a page's (`javascript:`, `data:`, an
/// external protocol) is the welcome page. A `file:` URL is kept: the navigation guard judges it.
fn start_url(text: &str) -> String {
    let t = text.trim();
    if t.is_empty() || t.len() > MAX_URL || !clean_text(t) {
        return DEFAULT_URL.to_string();
    }
    let scheme = crate::webhost::scheme_of(t);
    if scheme.is_empty() {
        // "host:8080" has a scheme-shaped prefix; a space means it is not a URL at all.
        if t.contains(char::is_whitespace) {
            return DEFAULT_URL.to_string();
        }
        return format!("https://{t}");
    }
    match scheme.as_str() {
        "http" | "https" | "iterm2-about" | "about" | "file" => t.to_string(),
        _ => DEFAULT_URL.to_string(),
    }
}

/// What starting a pane with a profile makes (`GW:98-111`): a browser only for a Browser profile
/// with `BrowserProfiles` on, else a terminal. `restored` is an arrangement's saved URL.
pub fn start_plan(custom_command: &str, browser_profiles: bool, initial_url: &str, restored: Option<&str>) -> Start {
    if !is_browser(custom_command) || !browser_profiles {
        return Start::Terminal;
    }
    Start::Browser(start_url(restored.unwrap_or(initial_url)))
}

/// The store a profile's views live in: Dev Null Mode has its own (InPrivate views, a folder per run).
pub fn store_of(dev_null: bool) -> Store {
    if dev_null { Store::BrowserDevNull } else { Store::Browser }
}

/// The URL an arrangement may keep, or None: http, https, `iterm2-about:` and `about:` only, at
/// most `MAX_URL`, with no control or bidirectional character and no credentials. Used on save and
/// again on load (an arrangement file is another party's).
pub fn saved_url(url: &str) -> Option<String> {
    let t = url.trim();
    if t.is_empty() || t.len() > MAX_URL || !clean_text(t) {
        return None;
    }
    match crate::webhost::scheme_of(t).as_str() {
        "iterm2-about" | "about" => Some(t.to_string()),
        "http" | "https" => {
            let (scheme, rest) = t.split_once("//")?;
            let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
            let authority = &rest[..end];
            let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
            Some(format!("{scheme}//{host}{}", &rest[end..]))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jobtitle::{JOB, PROFILE_NAME, SESSION_NAME};

    #[test]
    fn profile_type_follows_iterm2() {
        assert_eq!(profile_type("Browser"), 1);
        assert_eq!(profile_type("No"), 0);
        assert_eq!(profile_type("SSH"), 0);
        assert!(is_browser("Browser") && !is_browser("No"));
        // Setting Browser also sets Icon to Automatic; Terminal writes Login Shell (`PGP:276-281`).
        assert_eq!(set_profile_type(1), ("Browser", true));
        assert_eq!(set_profile_type(0), ("No", false));
        let s = |k: &str| crate::schema::find(k).unwrap_or_else(|| panic!("{k}"));
        assert_eq!(s("Initial URL").default.as_ref().and_then(|v| v.as_str()), Some(DEFAULT_URL));
        assert_eq!(s("Width in Points").default.as_ref().and_then(|v| v.as_integer()), Some(1000));
        assert_eq!(s("Height in Points").default.as_ref().and_then(|v| v.as_integer()), Some(800));
        assert_eq!((DEFAULT_URL, DEFAULT_NAME, USER_ID), ("iterm2-about:welcome", "Web Browser", "AC0E9812-7F88-478B-B361-5526082EDDB3"));
    }

    #[test]
    fn settings_modes_follow_iterm2() {
        assert!(!settings_visible(Some("terminal"), false, true, true), "a terminal row hides in a browser profile");
        assert!(settings_visible(Some("terminal"), false, false, true));
        assert!(!settings_visible(Some("browser"), false, false, true), "a browser row hides in a terminal profile");
        assert!(settings_visible(Some("browser"), false, true, true));
        assert!(!settings_visible(None, true, false, false), "the Profile type row needs BrowserProfiles (`OE:57-66`)");
        assert!(settings_visible(None, true, false, true));
        assert!(settings_visible(None, false, true, false) && settings_visible(None, false, false, false), "a row with no mode shows in both");
        let mode = |k: &str| crate::schema::find(k).unwrap_or_else(|| panic!("{k}")).mode.clone();
        for k in ["Initial URL", "Width in Points", "Height in Points", "Dev Null Mode", "Browser Zoom", "Instant Replay"] {
            assert_eq!(mode(k).as_deref(), Some("browser"), "{k}");
        }
        // Rows from the 27 terminal enclosures of PreferencePanel.xib.
        for k in ["Command", "Columns", "Rows", "Custom Command", "Initial Text", "Custom Directory", "Badge Text", "Scrollback Lines", "Terminal Type", "Cursor Type", "Use Bold Font", "Foreground Color", "Ansi 3 Color", "Normal Font", "Transparency", "Blur", "Background Image Location", "Automatically Log", "Send Code When Idle", "Prevent Automatic Profile Switching", "Option Key Sends", "Enable Triggers in Interactive Apps", "Session Close Undo Timeout", "Timestamps Visible", "Mouse Reporting", "Silence Bell"] {
            assert_eq!(mode(k).as_deref(), Some("terminal"), "{k}");
        }
        let hidden = crate::schema::find("Profile Type (Phony)").unwrap();
        assert!(hidden.hidden_mode && hidden.mode.is_none());
        // The rows a browser profile shares (Title components, the session name, the window style).
        for k in ["Name", "Title Components", "Window Type", "Space"] {
            assert_eq!(mode(k), None, "{k}");
        }
        assert!(crate::schema::find_global("BrowserProfiles").is_some_and(|s| s.section == "Experimental Features" && s.default.as_ref().and_then(|v| v.as_bool()) == Some(true)));
        // Tabs with nothing visible are removed in a browser profile (`ProfilePreferencesViewController.m:575-600`).
        assert!(tab_visible("General", true, true) && tab_visible("Window", true, true));
        assert!(!tab_visible("Colors", true, true) && !tab_visible("Text", true, true));
        assert!(tab_visible("Colors", false, true));
    }

    #[test]
    fn browser_titles_follow_iterm2() {
        assert_eq!(resolved_name(Some(" T "), Some("h"), "P", false).as_deref(), Some("T"));
        assert_eq!(resolved_name(Some(""), Some("h"), "P", false), None, "a transient blank keeps the name");
        assert_eq!(resolved_name(Some("  "), Some("h"), "P", false), None);
        assert_eq!(resolved_name(Some(""), Some("h"), "P", true).as_deref(), Some("h"));
        assert_eq!(resolved_name(None, None, "P", true).as_deref(), Some("P"));
        assert_eq!(resolved_name(None, Some("  "), "P", true).as_deref(), Some("P"));
        assert!(component_hidden(JOB, true, 0));
        assert!(!component_hidden(JOB, true, JOB), "a stale selection stays visible");
        assert!(!component_hidden(SESSION_NAME, true, 0));
        assert!(!component_hidden(PROFILE_NAME, true, 0));
        assert!(!component_hidden(JOB, false, 0));
        assert!(!component_hidden(0, true, 0), "tag 0 is the separator");
        for c in [crate::jobtitle::PWD, crate::jobtitle::TTY, crate::jobtitle::USER, crate::jobtitle::HOST, crate::jobtitle::COMMAND_LINE, crate::jobtitle::SIZE] {
            assert!(component_hidden(c, true, 0));
        }
        // A page's title is another party's text.
        assert_eq!(shown_title("a\u{202e}b\u{7}c"), "abc");
        assert_eq!(shown_title(&"x".repeat(1000)).chars().count(), 201);
    }

    #[test]
    fn window_size_is_points() {
        assert_eq!(window_cells(1000, 800), (1000, 800));
        assert_eq!(window_cells(0, 5000), (1, 1250));
        assert_eq!(window_cells(-5, 1250), (1, 1250));
    }

    #[test]
    fn a_browser_profile_starts_a_browser_only_with_the_setting() {
        assert_eq!(start_plan("Browser", true, "https://a.test/x", None), Start::Browser("https://a.test/x".into()));
        assert_eq!(start_plan("Browser", true, "", None), Start::Browser(DEFAULT_URL.into()), "empty uses the welcome page");
        assert_eq!(start_plan("Browser", true, "  ", None), Start::Browser(DEFAULT_URL.into()));
        assert_eq!(start_plan("Browser", false, "https://a.test/", None), Start::Terminal, "iTerm2 falls back to a terminal (`GW:98-111`)");
        assert_eq!(start_plan("No", true, "https://a.test/", None), Start::Terminal);
        // A restored URL wins over the profile's.
        assert_eq!(start_plan("Browser", true, "https://a.test/", Some("https://b.test/")), Start::Browser("https://b.test/".into()));
        // What a profile or an arrangement file names is judged before it loads.
        for bad in ["javascript:alert(1)", "data:text/html,x", "vbscript:x", "ms-msdt:id", "search-ms:query=x", "iterm2-file:/x", "iterm2-ssh:h"] {
            assert_eq!(start_plan("Browser", true, bad, None), Start::Browser(DEFAULT_URL.into()), "{bad}");
        }
        assert_eq!(start_plan("Browser", true, "example.com/a", None), Start::Browser("https://example.com/a".into()));
        assert_eq!(start_plan("Browser", true, "file:///C:/x.html", None), Start::Browser("file:///C:/x.html".into()), "the navigation guard judges a file URL");
        assert_eq!(store_of(false), crate::webhost::Store::Browser);
        assert_eq!(store_of(true), crate::webhost::Store::BrowserDevNull, "Dev Null Mode picks its own store");
    }

    #[test]
    fn a_saved_url_is_sanitized_and_bounded() {
        assert_eq!(saved_url("https://a.test/p?q=1#f").as_deref(), Some("https://a.test/p?q=1#f"));
        assert_eq!(saved_url("iterm2-about:history").as_deref(), Some("iterm2-about:history"));
        assert_eq!(saved_url("https://user:pw@a.test/x").as_deref(), Some("https://a.test/x"), "no credentials in Tandem's files");
        assert_eq!(saved_url("https://a.test/\u{1}"), None);
        assert_eq!(saved_url("https://a.test/\u{202e}x"), None);
        assert_eq!(saved_url("javascript:alert(1)"), None);
        assert_eq!(saved_url("file:///C:/x"), None, "an arrangement never names a local file");
        assert_eq!(saved_url("data:text/html,x"), None);
        assert_eq!(saved_url(&format!("https://a.test/{}", "x".repeat(MAX_URL))), None);
        assert_eq!(saved_url(""), None);
        assert_eq!(saved_url("about:blank").as_deref(), Some("about:blank"));
    }

    #[test]
    fn browser_io_is_plain_data() {
        let io = BrowserIo::default();
        assert!(io.url.is_empty() && !io.loading && !io.can_back && !io.can_forward && io.favicon.is_none() && !io.dev_null);
    }
}
