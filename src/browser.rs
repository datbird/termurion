//! Browser profiles and the browser pane's model (milestone 13x, Task 5): pure. A browser pane is
//! a `Pane` whose `Io` is `Io::Browser(BrowserIo)`; this file holds what decides its profile
//! type, its rows on the profile page, its start, its session name and what an arrangement may
//! keep of it (a sanitized, bounded URL: never cookies or page content).

use crate::jobtitle::BROWSER_HIDDEN;
use crate::layout::Rect;
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
    /// The last navigation failed (what shows is an error page).
    pub failed: bool,
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
    display_title_in(title, TITLE_CHARS)
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


// --- The toolbar and the URL bar (Task 6) ---

/// The toolbar's height in points (an `NSStackView` at its fitting height in iTerm2; Decision).
pub const TOOLBAR_H: f32 = 32.0;
/// How long a press on Back or Forward must last to list its history (`BT:520-560`).
pub const LONG_PRESS: std::time::Duration = std::time::Duration::from_millis(500);

/// The default search engine and suggestion service (`AS:481`, `AS:482`).
pub const DEFAULT_SEARCH: &str = "https://google.com/search?q=%@";
pub const DEFAULT_SUGGEST: &str = "https://suggestqueries.google.com/complete/search?client=firefox&q=%@";

/// The toolbar's buttons (`BT:80-122`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Back,
    Forward,
    Reload,
    Stop,
    Menu,
}

/// The buttons in order with their enabled flags: Reload turns into Stop while the page loads.
pub fn buttons(io: &BrowserIo) -> Vec<(Button, bool)> {
    vec![(Button::Back, io.can_back), (Button::Forward, io.can_forward), (if io.loading { Button::Stop } else { Button::Reload }, true), (Button::Menu, true)]
}

pub fn button_label(b: Button) -> &'static str {
    match b {
        Button::Back => "Back",
        Button::Forward => "Forward",
        Button::Reload => "Reload",
        Button::Stop => "Stop",
        Button::Menu => "Menu",
    }
}

/// The Segoe Fluent Icons glyph of a button (SF Symbols in iTerm2).
pub fn button_glyph(b: Button) -> char {
    match b {
        Button::Back => '\u{E72B}',
        Button::Forward => '\u{E72A}',
        Button::Reload => '\u{E72C}',
        Button::Stop => '\u{E711}',
        Button::Menu => '\u{E700}',
    }
}

pub const GLYPH_DEV_NULL: char = '\u{E7B3}';
/// The proxy indicator (Segoe Fluent "NetworkTower"; checklist item to confirm the glyph).
pub const GLYPH_PROXY: char = '\u{E839}';
pub const GLYPH_GLOBE: char = '\u{E774}';

/// `BrowserTransitionType` (`BrowserHistory.swift:10-18`): how a visit began.
pub const TRANSITION_TYPED: i64 = 0;
pub const TRANSITION_RELOAD: i64 = 2;
pub const TRANSITION_BACK_FORWARD: i64 = 3;
pub const TRANSITION_OTHER: i64 = 6;
pub const GLYPH_LOCK: char = '\u{E72E}';
pub const GLYPH_WARNING: char = '\u{E7BA}';

fn has_scheme_prefix(t: &str, prefix: &str) -> bool {
    t.len() >= prefix.len() && t.is_char_boundary(prefix.len()) && t[..prefix.len()].eq_ignore_ascii_case(prefix)
}

/// `stringHasValidScheme` (`UH:23-30`), matched ignoring case (WebView2 lowercases the scheme).
fn has_valid_scheme(t: &str) -> bool {
    ["http://", "https://", "iterm2-about:", "iterm2-ssh:", "iterm2-file:", "about:", "file://"].iter().any(|p| has_scheme_prefix(t, p))
}

fn is_ipv4_with_port(t: &str) -> bool {
    let (addr, port) = match t.split_once(':') {
        Some((a, p)) => (a, Some(p)),
        None => (t, None),
    };
    let parts: Vec<&str> = addr.split('.').collect();
    parts.len() == 4 && parts.iter().all(|p| (1..=3).contains(&p.len()) && p.bytes().all(|b| b.is_ascii_digit())) && port.is_none_or(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
}

/// `isValidDomainOrIP` (`UH:43-79`).
fn is_valid_domain_or_ip(t: &str) -> bool {
    if t.is_empty() || t.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return false;
    }
    if is_ipv4_with_port(t) || (t.starts_with('[') && t.ends_with(']')) || t.starts_with("localhost") || t.starts_with("127.0.0.1") {
        return true;
    }
    if t.contains('.') {
        let parts: Vec<&str> = t.split('.').collect();
        return parts.len() >= 2 && parts.iter().all(|p| !p.is_empty());
    }
    // An intranet style host: one word, possibly with a port.
    let (host, port) = match t.split_once(':') {
        Some((h, p)) => (h, Some(p)),
        None => (t, None),
    };
    !host.is_empty() && host.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') && port.is_none_or(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
}

/// The URL a typed text names, or None (`normalizeURL`, `UH:8-21`): a text with a valid scheme
/// stays, a domain or an address gets https://. One line of printable characters, at most
/// `MAX_URL`; anything else is not a URL.
pub fn normalize_url(text: &str) -> Option<String> {
    let t = text.trim();
    if t.is_empty() || t.len() > MAX_URL || t.chars().any(crate::inert_files::is_hidden) {
        return None;
    }
    if has_valid_scheme(t) {
        return if t.chars().any(char::is_whitespace) { None } else { Some(t.to_string()) };
    }
    is_valid_domain_or_ip(t).then(|| format!("https://{t}"))
}

/// `stringIsStronglyURLLike` (`UH:32-41`).
pub fn strongly_url_like(text: &str) -> bool {
    let t = text.trim();
    has_valid_scheme(t) || (is_valid_domain_or_ip(t) && t.contains('.'))
}

/// Query parameter percent escaping (`urlQueryAllowed`): letters, digits and `!$&'()*+,-./:;=?@_~`
/// stay.
fn percent_query(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"!$&'()*+,-./:;=?@_~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// `template` with its `%@` replaced by `query`, escaped as a query parameter.
pub fn search_url(template: &str, query: &str) -> String {
    template.replace("%@", &percent_query(query))
}

/// The pieces of a URL `search_query_from_url` compares (`URLComponents`).
struct Parts<'a> {
    host: Option<String>,
    path: &'a str,
    query: &'a str,
}

fn parts_of(url: &str) -> Parts<'_> {
    let url = url.split('#').next().unwrap_or("");
    let (before, query) = url.split_once('?').unwrap_or((url, ""));
    let (host, path) = match before.split_once("://") {
        Some((_, rest)) => {
            let end = rest.find('/').unwrap_or(rest.len());
            let authority = &rest[..end];
            let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
            let host = if host.starts_with('[') { host.split_once(']').map_or(host, |(h, _)| h) } else { host.split(':').next().unwrap_or(host) };
            (Some(host.to_ascii_lowercase()), &rest[end..])
        }
        None => (None, before),
    };
    Parts { host, path, query }
}

fn normal_host(h: &str) -> &str {
    for p in ["www.", "m.", "mobile."] {
        if let Some(rest) = h.strip_prefix(p) {
            return rest;
        }
    }
    h
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = b.get(i + 1..i + 3).and_then(|h| std::str::from_utf8(h).ok()).and_then(|h| u8::from_str_radix(h, 16).ok());
            match hex {
                Some(v) => {
                    out.push(v);
                    i += 3;
                    continue;
                }
                // `removingPercentEncoding` fails on a bad escape and the value stays as it is.
                None => return s.to_string(),
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| s.to_string())
}

/// What a search URL of the engine searched for (`searchQueryFromURL`, `BSC:128-190`): the host
/// (without `www.`, `m.` or `mobile.`) and path must match the template's and the template's
/// `%@` parameter must be present.
pub fn search_query_from_url(url: &str, template: &str) -> Option<String> {
    let (a, t) = (parts_of(url), parts_of(template));
    let (ah, th) = (a.host?, t.host?);
    if normal_host(&ah) != normal_host(&th) || a.path != t.path {
        return None;
    }
    let name = t.query.split('&').find_map(|item| {
        let (n, v) = item.split_once('=')?;
        v.contains("%@").then_some(n)
    })?;
    a.query.split('&').find_map(|item| {
        let (n, v) = item.split_once('=')?;
        (n == name).then(|| percent_decode(v))
    })
}

/// A row of the suggestions list.
#[derive(Clone, Debug, PartialEq)]
pub struct Suggestion {
    pub url: String,
    pub text: String,
    pub detail: String,
    pub score: i64,
}

const SCORE_STRONG_SEARCH: i64 = 3_000_001;
const SCORE_STRONG_URL: i64 = 3_000_000;
const SCORE_WEAK_SEARCH: i64 = 2_999_999;
const SCORE_OPEN_SEARCH: i64 = 2_000_000;
const SCORE_WEAK_URL: i64 = 999_998;
const SCORE_BOOKMARK: i64 = 500_000;
/// The largest OpenSearch answer parsed, and how many of its entries are looked at.
const MAX_OPEN_SEARCH_BODY: usize = 256 * 1024;
const MAX_OPEN_SEARCH_SCAN: usize = 100;
/// The longest text a suggestion row shows.
const SUGGESTION_CHARS: usize = 200;

/// Whether the typed text asks the suggestion service (`BOS:30-38`): not empty, not an https URL.
pub fn wants_open_search(query: &str) -> bool {
    let t = query.trim();
    !t.is_empty() && !t.starts_with("https://")
}

/// Whether typed text may go to the suggestion service: only text that is not a URL by the one
/// parser the Enter path uses (`normalize_url`), so a `localhost:3000/x?token=y` never leaves.
pub fn suggest_allowed(query: &str) -> bool {
    wants_open_search(query) && query.trim().chars().count() <= 200 && normalize_url(query).is_none()
}

/// Whether a message's source is one of Tandem's own `iterm2-about:` pages: the scheme, compared
/// ignoring ASCII case, and nothing else (a web page that holds a local page's name in its path or
/// as a scheme prefix is not one). 13x2 calls this from `browser_js::local_page_ok`.
pub fn local_source_ok(source: &str) -> bool {
    source.split_once(':').is_some_and(|(scheme, _)| scheme.eq_ignore_ascii_case("iterm2-about"))
}

/// The service's answer to `query`, as rows (`BOS:66-104`): `[query, [suggestion, ...]]` where the
/// query is echoed. At most `max` rows scored 2000000 minus their index; an empty entry is
/// skipped (it still counts as an index).
pub fn open_search_results(body: &str, query: &str, template: &str, max: usize) -> Vec<Suggestion> {
    if body.len() > MAX_OPEN_SEARCH_BODY {
        return Vec::new();
    }
    let Ok(serde_json::Value::Array(top)) = serde_json::from_str::<serde_json::Value>(body) else { return Vec::new() };
    let (Some(serde_json::Value::String(echo)), Some(serde_json::Value::Array(list))) = (top.first(), top.get(1)) else { return Vec::new() };
    if echo != query.trim() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for (i, v) in list.iter().enumerate().take(MAX_OPEN_SEARCH_SCAN) {
        let serde_json::Value::String(s) = v else { return Vec::new() };
        if s.is_empty() {
            continue;
        }
        out.push(Suggestion { url: search_url(template, s), text: display_title_in(s, SUGGESTION_CHARS), detail: "Search Suggestion".into(), score: SCORE_OPEN_SEARCH - i as i64 });
        if out.len() == max {
            break;
        }
    }
    out
}

/// The rows for what is typed, best first (`suggestions(forQuery:)`, `BSC:44-126`): bookmarks
/// (url, title, visits), then history rows for URLs no bookmark covers, the service's rows, the
/// search row and the navigate row.
pub fn rank(query: &str, bookmarks: &[(String, Option<String>, i64)], history: &[(String, Option<String>, i64)], open_search: &[Suggestion], template: &str) -> Vec<Suggestion> {
    if query.trim().is_empty() {
        return Vec::new();
    }
    let mut rows = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for (url, title, visits) in bookmarks {
        let text = title.as_deref().filter(|t| !t.is_empty()).unwrap_or(url);
        rows.push(Suggestion { url: url.clone(), text: display_title(text), detail: "Bookmark".into(), score: SCORE_BOOKMARK + visits });
        seen.insert(url.as_str());
    }
    for (url, title, visits) in history {
        if seen.contains(url.as_str()) {
            continue;
        }
        let (text, detail) = match title.as_deref().filter(|t| !t.is_empty()) {
            Some(t) => (display_title(t), display_url(url)),
            None => (display_url(url), format!("Visited {visits} times")),
        };
        rows.push(Suggestion { url: url.clone(), text, detail, score: *visits });
    }
    if wants_open_search(query) {
        rows.extend(open_search.iter().cloned());
    }
    let (strong, actual) = match search_query_from_url(query, template) {
        Some(q) => (true, q),
        None => (false, query.to_string()),
    };
    let actual = actual.trim();
    rows.push(Suggestion { url: search_url(template, actual), text: format!("Search for \u{201c}{}\u{201d}", display_title(actual)), detail: "Web Search".into(), score: if strong { SCORE_STRONG_SEARCH } else { SCORE_WEAK_SEARCH } });
    if let Some(n) = normalize_url(query) {
        let score = if strongly_url_like(query) { SCORE_STRONG_URL } else { SCORE_WEAK_URL };
        rows.push(Suggestion { text: format!("Navigate to \u{201c}{}\u{201d}", display_url(&n)), url: n, detail: "URL".into(), score });
    }
    rows.sort_by(|a, b| b.score.cmp(&a.score));
    rows
}

/// What pressing Return in the URL bar loads: a URL or a search (`BT` delegate, `UH`): the
/// normalized text when it names a URL, else the search for it.
pub fn submitted_url(text: &str, template: &str) -> String {
    normalize_url(text).unwrap_or_else(|| search_url(template, text.trim()))
}

/// The toolbar menu: None is a separator, the flag is whether the item is enabled (`BT:306-420`).
/// Dev Null hides the bookmark and history items; Settings stays disabled until 13x8 builds the
/// page.
pub fn menu_items(bookmarked: bool, has_url: bool, dev_null: bool) -> Vec<Option<(&'static str, bool)>> {
    let mut out = Vec::new();
    if !dev_null {
        out.push(Some((if bookmarked { "Remove Bookmark" } else { "Add Bookmark" }, has_url)));
        out.push(None);
        out.push(Some(("Manage Bookmarks", true)));
        out.push(Some(("History", true)));
        out.push(None);
    }
    out.push(Some(("Settings", false)));
    // The one menu filter (13w): an item this build removes is never shown, here as in every
    // other menu copy (iTerm2's own toolbar menu has a debug-build-only item).
    out.retain(|i| i.is_none_or(|(label, _)| !crate::menus::hidden_in_menu(menu_selector(label), cfg!(debug_assertions))));
    out
}

/// The iTerm2 action of a toolbar menu item (`iTermBrowserToolbar.swift:347-412`), the name the
/// one menu filter (`menus::hidden_in_menu`) reads.
pub fn menu_selector(label: &str) -> &'static str {
    match label {
        "Add Bookmark" | "Remove Bookmark" => "bookmarkMenuItemSelected:",
        "Manage Bookmarks" => "manageBookmarksMenuItemSelected:",
        "History" => "historyMenuItemSelected:",
        "Settings" => "settingsMenuItemSelected:",
        "Debug Autofill Fields" => "debugAutofillMenuItemSelected:",
        _ => "",
    }
}

/// The lines of a Back or Forward history menu: the title, else the URL (`BT:520-560`).
pub fn history_menu(items: &[(String, String)]) -> Vec<String> {
    items.iter().map(|(title, url)| display_title(if title.trim().is_empty() { url } else { title })).collect()
}

// --- What the person sees of a page's address ---

/// Another party's text through THE display sanitizer (`inert_files::display_text`, the 13w
/// ruling), at most `limit` characters and an ellipsis past them.
fn display_title_in(t: &str, limit: usize) -> String {
    let clean: Vec<char> = crate::inert_files::display_text(t, limit.saturating_add(1), false).chars().collect();
    if clean.len() <= limit { clean.into_iter().collect() } else { clean[..limit].iter().chain(['\u{2026}'].iter()).collect() }
}

/// Another party's text (a page title, a history title, a suggestion) as one line of printable
/// characters, at most 120 of them.
pub fn display_title(t: &str) -> String {
    display_title_in(t, 120)
}

/// RFC 3492 punycode of one label (the part after `xn--`), None when it cannot be encoded.
pub fn punycode(label: &str) -> Option<String> {
    const BASE: u32 = 36;
    const TMIN: u32 = 1;
    const TMAX: u32 = 26;
    fn digit(d: u32) -> char {
        if d < 26 { (b'a' + d as u8) as char } else { (b'0' + (d - 26) as u8) as char }
    }
    fn adapt(mut delta: u32, points: u32, first: bool) -> u32 {
        delta = if first { delta / 700 } else { delta / 2 };
        delta += delta / points;
        let mut k = 0;
        while delta > ((BASE - TMIN) * TMAX) / 2 {
            delta /= BASE - TMIN;
            k += BASE;
        }
        k + (BASE - TMIN + 1) * delta / (delta + 38)
    }
    let input: Vec<u32> = label.chars().map(|c| c as u32).collect();
    let mut out: String = label.chars().filter(char::is_ascii).collect();
    let basic = out.len() as u32;
    let mut handled = basic;
    if basic > 0 {
        out.push('-');
    }
    let (mut n, mut delta, mut bias) = (128u32, 0u32, 72u32);
    while (handled as usize) < input.len() {
        let m = input.iter().copied().filter(|&c| c >= n).min()?;
        delta = delta.checked_add((m - n).checked_mul(handled + 1)?)?;
        n = m;
        for &c in &input {
            if c < n {
                delta = delta.checked_add(1)?;
            }
            if c == n {
                let mut q = delta;
                let mut k = BASE;
                loop {
                    let t = if k <= bias { TMIN } else if k >= bias + TMAX { TMAX } else { k - bias };
                    if q < t {
                        break;
                    }
                    out.push(digit(t + (q - t) % (BASE - t)));
                    q = (q - t) / (BASE - t);
                    k += BASE;
                }
                out.push(digit(q));
                bias = adapt(delta, handled + 1, handled == basic);
                delta = 0;
                handled += 1;
            }
        }
        delta += 1;
        n += 1;
    }
    Some(out)
}

/// A host with every non-ASCII label in its `xn--` form (as Chromium shows a host it will not
/// trust in Unicode; Tandem shows every IDN so, Decision). Full width dots are dots.
fn ascii_host(host: &str) -> String {
    let host: String = host.chars().map(|c| if matches!(c, '\u{3002}' | '\u{FF0E}' | '\u{FF61}') { '.' } else { c }).collect();
    host.split('.').map(|label| if label.is_ascii() { label.to_string() } else { punycode(&label.to_lowercase()).map_or_else(|| label.to_string(), |p| format!("xn--{p}")) }).collect::<Vec<_>>().join(".")
}

/// What the URL bar shows of a page's address: no control, bidirectional or zero width
/// character, the real host (punycode for a non-ASCII one), the user information in front of it
/// when the URL has some (never hidden), at most `MAX_URL` characters.
pub fn display_url(url: &str) -> String {
    let clean = crate::inert_files::display_text(url, usize::MAX, false);
    let shown = match clean.split_once("://") {
        Some((scheme, rest)) if !scheme.is_empty() && !scheme.contains('/') => {
            let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
            let (authority, tail) = rest.split_at(end);
            let (userinfo, hostport) = authority.rsplit_once('@').map_or(("", authority), |(u, h)| (u, h));
            let (host, port) = if hostport.starts_with('[') {
                hostport.split_once(']').map_or((hostport, ""), |(h, p)| (&hostport[..h.len() + 1], p))
            } else {
                hostport.rsplit_once(':').filter(|(_, p)| p.bytes().all(|b| b.is_ascii_digit())).map_or((hostport, ""), |(h, _)| (h, &hostport[h.len()..]))
            };
            let at = if authority.contains('@') { format!("{userinfo}@") } else { String::new() };
            format!("{scheme}://{at}{}{port}{tail}", ascii_host(host))
        }
        _ => clean,
    };
    let chars: Vec<char> = shown.chars().collect();
    if chars.len() <= MAX_URL { shown } else { chars[..MAX_URL].iter().chain(['\u{2026}'].iter()).collect() }
}

/// The address as it fits `cells` characters of the idle field. The end of the host always stays
/// visible (a long user name cannot push it out, `https://accounts.google.com...@evil.test/` must
/// show `evil.test`): the path is cut first, then the user information is shortened to a marked
/// prefix, and last the host is cut from its left.
pub fn fit_address(shown: &str, cells: usize) -> String {
    let chars: Vec<char> = shown.chars().collect();
    if chars.len() <= cells {
        return shown.to_string();
    }
    let cut = |text: &str, room: usize| -> String {
        let c: Vec<char> = text.chars().collect();
        if c.len() <= room { text.to_string() } else if room == 0 { String::new() } else { c[..room - 1].iter().chain(['\u{2026}'].iter()).collect() }
    };
    let Some((scheme, rest)) = shown.split_once("://").filter(|(s, _)| !s.is_empty() && !s.contains('/')) else {
        return cut(shown, cells);
    };
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(end);
    let (userinfo, hostport) = authority.rsplit_once('@').map_or((None, authority), |(u, h)| (Some(u), h));
    let prefix = format!("{scheme}://");
    let short = userinfo.map(|u| if u.chars().count() > 4 { format!("{}\u{2026}", u.chars().take(4).collect::<String>()) } else { u.to_string() });
    let marks = ["\u{2026}".to_string()];
    let users: Vec<&str> = userinfo.into_iter().chain(short.as_deref()).chain(userinfo.map(|_| marks[0].as_str())).collect();
    for u in users.iter().map(|u| Some(*u)).chain(userinfo.is_none().then_some(None)) {
        let head = format!("{prefix}{}{hostport}", u.map_or(String::new(), |u| format!("{u}@")));
        let n = head.chars().count();
        if n <= cells {
            return format!("{head}{}", cut(tail, cells - n));
        }
    }
    // The host itself does not fit: keep its end.
    let at = if userinfo.is_some() { "@" } else { "" };
    let host: Vec<char> = format!("{at}{hostport}").chars().collect();
    let room = cells.saturating_sub(1);
    ['\u{2026}'].iter().chain(host[host.len().saturating_sub(room)..].iter()).take(cells).collect()
}

/// What the lock mark beside the address shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Security {
    /// Tandem's own `iterm2-about:` page or a blank one; nothing else is Tandem's.
    Local,
    Secure,
    NotSecure,
    File,
    /// The last navigation failed: whatever shows is an error page.
    Failed,
}

/// The mark for the page that is really loaded: its URL's scheme and whether its navigation
/// failed. Never the typed text.
pub fn security(url: &str, failed: bool) -> Security {
    if failed {
        return Security::Failed;
    }
    match crate::webhost::scheme_of(url).as_str() {
        "https" => Security::Secure,
        "http" => Security::NotSecure,
        "file" => Security::File,
        // The creator's origin decides: only a blob made by an https page is secure.
        "blob" | "filesystem" => {
            let inner = url.split_once(':').map_or("", |(_, r)| r);
            if crate::webhost::scheme_of(inner) == "https" { Security::Secure } else { Security::NotSecure }
        }
        // Tandem's own pages (and the blank one, and an empty address before any load).
        "iterm2-about" => Security::Local,
        "about" if url.trim().eq_ignore_ascii_case("about:blank") => Security::Local,
        "" if url.trim().is_empty() => Security::Local,
        // data:, javascript:, other about: pages and every other scheme: not Tandem's, not secure.
        _ => Security::NotSecure,
    }
}

/// How a typed URL is loaded: web pages and Tandem's own pages directly, a `file:` URL through the
/// navigation verdict (the path judge), anything else through the external-scheme handler.
#[derive(Clone, Debug, PartialEq)]
pub enum Typed {
    Navigate(String),
    File(String),
    External(String),
}

pub fn typed_route(url: &str) -> Typed {
    let scheme = crate::webhost::scheme_of(url);
    match scheme.as_str() {
        "http" | "https" | "iterm2-about" => Typed::Navigate(url.to_string()),
        "about" if url.eq_ignore_ascii_case("about:blank") => Typed::Navigate(url.to_string()),
        "file" => Typed::File(url.to_string()),
        _ => Typed::External(url.to_string()),
    }
}

/// A search or suggestion template as it may be used (P13): http or https and holding `%@`, else
/// the default.
fn template_ok(t: &str) -> bool {
    let s = crate::webhost::scheme_of(t);
    (s == "http" || s == "https") && t.contains("%@") && t.len() <= MAX_URL && !t.chars().any(|c| c.is_control() || c.is_whitespace())
}

pub fn search_template(configured: &str) -> String {
    let t = configured.trim();
    if template_ok(t) { t.to_string() } else { DEFAULT_SEARCH.to_string() }
}

pub fn suggest_template(configured: &str) -> String {
    let t = configured.trim();
    if template_ok(t) { t.to_string() } else { DEFAULT_SUGGEST.to_string() }
}

/// The zoom in percent after Make Text Bigger (`dir` 1), Smaller (-1) or Normal Size (0, the
/// saved profile's): five points per step (`FontTable.swift:427`), kept inside what WebView2
/// takes (25 to 500; iTerm2 goes down to 5).
pub fn zoom_step(current: i64, dir: i32, saved: i64) -> i64 {
    if dir == 0 {
        return if saved > 0 { saved.clamp(25, 500) } else { 100 };
    }
    (current + 5 * i64::from(dir)).clamp(25, 500)
}

/// The most queries the OpenSearch cache keeps.
const CACHE_MAX: usize = 64;

/// What the suggestion service answered, by query, for the 50 ms race (`BOS:43-64`).
#[derive(Clone, Debug, Default)]
pub struct OpenSearchCache {
    map: std::collections::HashMap<String, Vec<Suggestion>>,
    order: Vec<String>,
}

impl OpenSearchCache {
    pub fn put(&mut self, query: &str, rows: Vec<Suggestion>) {
        if self.map.insert(query.to_string(), rows).is_none() {
            self.order.push(query.to_string());
        }
        while self.order.len() > CACHE_MAX {
            let old = self.order.remove(0);
            self.map.remove(&old);
        }
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn has(&self, query: &str) -> bool {
        self.map.contains_key(query)
    }

    pub fn get(&self, query: &str) -> Option<Vec<Suggestion>> {
        self.map.get(query).cloned()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// The rows of the longest cached prefix of `query` that begin with it, scored 10 below
    /// their place (`filterAndRescoreSuggestions`, `BOS:166-175`).
    pub fn fallback(&self, query: &str) -> Vec<Suggestion> {
        let best = self.map.keys().filter(|k| query.starts_with(k.as_str())).max_by_key(|k| k.chars().count());
        let Some(rows) = best.and_then(|k| self.map.get(k)) else { return Vec::new() };
        let q = query.to_lowercase();
        rows.iter().filter(|r| r.text.to_lowercase().starts_with(&q)).enumerate().map(|(i, r)| Suggestion { score: SCORE_OPEN_SEARCH - i as i64 - 10, ..r.clone() }).collect()
    }
}

// --- The toolbar's layout and hits ---

/// Where each part of the toolbar sits, in pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Geometry {
    pub back: Rect,
    pub forward: Rect,
    /// Reload, or Stop while loading.
    pub reload: Rect,
    pub dev_null: Option<Rect>,
    /// The proxy indicator, after the Dev Null one, while the browser proxy is on.
    pub proxy: Option<Rect>,
    /// The address field with its mark.
    pub field: Rect,
    /// The favicon (a globe until one loads) at the field's left.
    pub indicator: Rect,
    /// The lock or warning mark after it.
    pub mark: Rect,
    /// Where the address text goes.
    pub text: Rect,
    pub menu: Rect,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Hit {
    Button(Button),
    DevNull,
    Proxy,
    Indicator,
    Mark,
    Field,
}

/// The toolbar's parts across `bar`: square buttons the bar's height, the field between.
pub fn geometry(bar: Rect, scale: f32, dev_null: bool) -> Geometry {
    geometry_with(bar, scale, dev_null, false)
}

/// `geometry` with the proxy indicator (Task 8, P15) when `proxy` is set.
pub fn geometry_with(bar: Rect, scale: f32, dev_null: bool, proxy: bool) -> Geometry {
    let side = bar.h.max(1.0);
    let gap = 4.0 * scale;
    let right = bar.x + bar.w;
    let sq = |x: f32| Rect { x, y: bar.y, w: side.min((right - x).max(0.0)), h: bar.h };
    let back = sq(bar.x);
    let forward = sq(back.x + back.w);
    let reload = sq(forward.x + forward.w);
    let mut x = reload.x + reload.w;
    let dn = dev_null.then(|| {
        let r = sq(x);
        x = r.x + r.w;
        r
    });
    let px = proxy.then(|| {
        let r = sq(x);
        x = r.x + r.w;
        r
    });
    let menu_x = (right - side).max(x);
    let menu = Rect { x: menu_x, y: bar.y, w: (right - menu_x).max(0.0), h: bar.h };
    let fx = x + gap;
    let field = Rect { x: fx, y: bar.y + 4.0 * scale, w: (menu.x - gap - fx).max(0.0), h: (bar.h - 8.0 * scale).max(0.0) };
    let indicator = Rect { x: field.x, y: field.y, w: field.h.min(field.w), h: field.h };
    let mark = Rect { x: indicator.x + indicator.w, y: field.y, w: (field.h * 0.8).min((field.w - indicator.w).max(0.0)), h: field.h };
    let text = Rect { x: mark.x + mark.w, y: field.y, w: (field.w - indicator.w - mark.w - gap).max(0.0), h: field.h };
    Geometry { back, forward, reload, dev_null: dn, proxy: px, field, indicator, mark, text, menu }
}

/// The part of the toolbar under a point.
pub fn hit(g: &Geometry, x: f32, y: f32) -> Option<Hit> {
    let buttons = [(g.back, Button::Back), (g.forward, Button::Forward), (g.reload, Button::Reload), (g.menu, Button::Menu)];
    if let Some((_, b)) = buttons.iter().find(|(r, _)| r.w > 0.0 && r.contains(x, y)) {
        return Some(Hit::Button(*b));
    }
    if g.dev_null.is_some_and(|r| r.contains(x, y)) {
        return Some(Hit::DevNull);
    }
    if g.proxy.is_some_and(|r| r.contains(x, y)) {
        return Some(Hit::Proxy);
    }
    if g.indicator.w > 0.0 && g.indicator.contains(x, y) {
        return Some(Hit::Indicator);
    }
    if g.mark.w > 0.0 && g.mark.contains(x, y) {
        return Some(Hit::Mark);
    }
    (g.field.w > 0.0 && g.field.contains(x, y)).then_some(Hit::Field)
}

/// The most suggestion rows shown.
pub const MAX_SUGGESTIONS: usize = 8;

/// The rows of the suggestions list under the field, each `row_h` tall, at most `MAX_SUGGESTIONS`
/// and never below `bottom`.
pub fn suggestion_rects(g: &Geometry, row_h: f32, n: usize, bottom: f32) -> Vec<Rect> {
    let top = g.field.y + g.field.h + 2.0;
    (0..n.min(MAX_SUGGESTIONS)).map(|i| Rect { x: g.field.x, y: top + i as f32 * row_h, w: g.field.w, h: row_h }).take_while(|r| r.y + r.h <= bottom).collect()
}

/// A page's back and forward entries, kept from its `Source` events: WebView2 has no list of them
/// (Decision). Entries a page adds with `history.pushState` may be missing.
#[derive(Clone, Debug, Default)]
pub struct NavList {
    entries: Vec<(String, String)>,
    index: usize,
    pending: Option<i32>,
}

/// The most entries kept, and the most a menu lists.
const NAV_MAX: usize = 100;
const NAV_MENU: usize = 25;

impl NavList {
    /// A new committed URL: a step Tandem asked for lands on its entry, anything else is a new
    /// page after the current one.
    pub fn source(&mut self, url: &str) {
        if self.entries.is_empty() {
            self.entries.push((url.to_string(), String::new()));
            self.index = 0;
            return;
        }
        if self.entries[self.index].0 == url {
            self.pending = None;
            return;
        }
        if let Some(steps) = self.pending.take() {
            let target = self.index as i64 + i64::from(steps);
            if target >= 0 && (target as usize) < self.entries.len() && self.entries[target as usize].0 == url {
                self.index = target as usize;
                return;
            }
        }
        self.entries.truncate(self.index + 1);
        self.entries.push((url.to_string(), String::new()));
        if self.entries.len() > NAV_MAX {
            self.entries.remove(0);
        }
        self.index = self.entries.len() - 1;
    }

    /// The current page's title.
    pub fn title(&mut self, title: &str) {
        if let Some(e) = self.entries.get_mut(self.index) {
            e.1 = title.to_string();
        }
    }

    /// Tandem is about to go `steps` entries (negative is back).
    pub fn expect(&mut self, steps: i32) {
        self.pending = Some(steps);
    }

    /// The entries before the current one, nearest first, with the steps back to each.
    pub fn back_items(&self) -> Vec<(String, String, i32)> {
        (0..self.index).rev().take(NAV_MENU).map(|i| (self.entries[i].1.clone(), self.entries[i].0.clone(), i as i32 - self.index as i32)).collect()
    }

    /// The entries after the current one, nearest first, with the steps forward to each.
    pub fn forward_items(&self) -> Vec<(String, String, i32)> {
        (self.index + 1..self.entries.len()).take(NAV_MENU).map(|i| (self.entries[i].1.clone(), self.entries[i].0.clone(), (i - self.index) as i32)).collect()
    }
}

/// The pane each window had focused when its views were last placed, so a pane change is told from
/// none. Per window, and acted on only while the window is in front: a background window's redraw
/// never moves keyboard focus (Task 5 fix r1).
#[derive(Clone, Debug)]
pub struct FocusSeen<K> {
    seen: std::collections::HashMap<K, Option<crate::layout::PaneId>>,
}

impl<K> Default for FocusSeen<K> {
    fn default() -> Self {
        FocusSeen { seen: std::collections::HashMap::new() }
    }
}

impl<K: std::hash::Hash + Eq> FocusSeen<K> {
    /// `Some(previous)` when this window's focus changed and the window is in front; a window that
    /// is not in front keeps what it saw, so the change is acted on once it is.
    pub fn step(&mut self, window: K, focused: Option<crate::layout::PaneId>, in_front: bool) -> Option<Option<crate::layout::PaneId>> {
        if !in_front {
            return None;
        }
        let prev = self.seen.insert(window, focused).unwrap_or(None);
        (prev != focused).then_some(prev)
    }

    pub fn forget(&mut self, pane: crate::layout::PaneId) {
        for v in self.seen.values_mut() {
            if *v == Some(pane) {
                *v = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: &str = "https://google.com/search?q=%@";

    #[test]
    fn normalize_follows_iterm2() {
        assert_eq!(normalize_url("  https://a  ").as_deref(), Some("https://a"));
        assert_eq!(normalize_url("example.com").as_deref(), Some("https://example.com"));
        assert_eq!(normalize_url("a b"), None);
        assert_eq!(normalize_url("192.168.1.1:80").as_deref(), Some("https://192.168.1.1:80"));
        assert_eq!(normalize_url("[::1]").as_deref(), Some("https://[::1]"));
        assert_eq!(normalize_url("localhost:8080").as_deref(), Some("https://localhost:8080"));
        assert_eq!(normalize_url("intranet").as_deref(), Some("https://intranet"), "the single word host rule (`UH:72-76`)");
        assert_eq!(normalize_url("about:blank").as_deref(), Some("about:blank"));
        assert_eq!(normalize_url("iterm2-about:history").as_deref(), Some("iterm2-about:history"));
        assert!(!strongly_url_like("intranet"));
        assert!(strongly_url_like("a.b"));
        assert!(strongly_url_like("file:///c:/x"));
        assert_eq!(submitted_url("rust lang", T), search_url(T, "rust lang"));
        assert_eq!(search_url(T, "rust lang"), "https://google.com/search?q=rust%20lang");
        // Text that is not one line of printable characters never becomes a URL.
        assert_eq!(normalize_url("a.b\nc.d"), None);
        assert_eq!(normalize_url(&format!("https://a.test/{}", "x".repeat(MAX_URL))), None);
        // Typing a URL navigates it; a word with a space searches; the scheme is matched ignoring case.
        assert_eq!(submitted_url(" example.com/a ", T), "https://example.com/a");
        assert_eq!(normalize_url("HTTPS://A.TEST/").as_deref(), Some("HTTPS://A.TEST/"));
    }

    fn sg(url: &str, text: &str, detail: &str, score: i64) -> Suggestion {
        Suggestion { url: url.into(), text: text.into(), detail: detail.into(), score }
    }

    #[test]
    fn suggestion_scores_follow_iterm2() {
        let os = open_search_results(r#"["rust",["rust lang","rustc"]]"#, "rust", T, 5);
        assert_eq!(os.len(), 2);
        let bookmarks = vec![("https://u1.test/".to_string(), Some("One".to_string()), 4)];
        let history = vec![("https://u1.test/".to_string(), None, 9), ("https://u2.test/".to_string(), None, 2)];
        let rows = rank("rust", &bookmarks, &history, &os, T);
        let order: Vec<(&str, i64)> = rows.iter().map(|r| (r.text.as_str(), r.score)).collect();
        assert_eq!(
            order,
            vec![("Search for \u{201c}rust\u{201d}", 2_999_999), ("rust lang", 2_000_000), ("rustc", 1_999_999), ("Navigate to \u{201c}https://rust\u{201d}", 999_998), ("One", 500_004), ("https://u2.test/", 2)]
        );
        assert_eq!(rows[0].detail, "Web Search");
        assert_eq!(rows[0].url, "https://google.com/search?q=rust");
        assert_eq!(rows[3].detail, "URL");
        assert_eq!(rows[4].detail, "Bookmark");
        assert_eq!(rows[5].detail, "Visited 2 times");
        // A strongly URL-like query navigates first and asks no OpenSearch.
        let rows = rank("https://x.y", &[], &[], &os, T);
        assert_eq!(rows[0].text, "Navigate to \u{201c}https://x.y\u{201d}");
        assert_eq!(rows[0].score, 3_000_000);
        assert!(rows.iter().all(|r| r.detail != "Search Suggestion"), "no OpenSearch for an https query");
        assert!(!wants_open_search("https://x.y") && !wants_open_search("   ") && wants_open_search("rust"));
        // A query that is a search URL of the engine searches for what it holds, strongly.
        let rows = rank("https://www.google.com/search?q=q", &[], &[], &[], T);
        assert_eq!((rows[0].text.as_str(), rows[0].score), ("Search for \u{201c}q\u{201d}", 3_000_001));
        assert_eq!(rows[0].url, "https://google.com/search?q=q");
        // Nothing typed, nothing suggested; a history row with a title shows the title and the URL.
        assert!(rank("  ", &bookmarks, &history, &os, T).is_empty());
        let rows = rank("zz", &[], &[("https://h.test/".into(), Some("Home".into()), 1)], &[], T);
        let h = rows.iter().find(|r| r.score == 1).unwrap();
        assert_eq!((h.text.as_str(), h.detail.as_str()), ("Home", "https://h.test/"));
        let _ = sg("", "", "", 0);
    }

    #[test]
    fn search_round_trip_follows_iterm2() {
        assert_eq!(search_query_from_url("https://www.google.com/search?q=a%20b", T).as_deref(), Some("a b"));
        assert_eq!(search_query_from_url("https://google.com/other?q=a", T), None, "a different path");
        assert_eq!(search_query_from_url("https://google.com/search?q=a", "https://google.com/search?q=x"), None, "a template without the placeholder");
        assert_eq!(search_query_from_url("https://bing.com/search?q=a", T), None, "another host");
        assert_eq!(search_query_from_url("https://m.google.com/search?q=a", T).as_deref(), Some("a"));
        assert_eq!(search_query_from_url("not a url", T), None);
    }

    #[test]
    fn open_search_parse_follows_iterm2() {
        let r = open_search_results(r#"["ru",["rust","ruby",""]]"#, "ru", T, 5);
        assert_eq!(r.iter().map(|s| (s.text.as_str(), s.score, s.detail.as_str())).collect::<Vec<_>>(), vec![("rust", 2_000_000, "Search Suggestion"), ("ruby", 1_999_999, "Search Suggestion")]);
        assert_eq!(r[0].url, "https://google.com/search?q=rust");
        assert!(open_search_results(r#"["other",["rust"]]"#, "ru", T, 5).is_empty(), "the echoed query must match");
        assert!(open_search_results("not json", "ru", T, 5).is_empty());
        assert!(open_search_results(r#"["ru",["a",1]]"#, "ru", T, 5).is_empty(), "every suggestion must be text");
        let many = format!(r#"["ru",[{}]]"#, (0..9).map(|i| format!("\"r{i}\"")).collect::<Vec<_>>().join(","));
        assert_eq!(open_search_results(&many, "ru", T, 5).len(), 5);
        assert!(open_search_results(&format!(r#"["ru",["{}"]]"#, "x".repeat(300_000)), "ru", T, 5).is_empty(), "a huge body is not parsed");
    }

    #[test]
    fn toolbar_menus_follow_iterm2() {
        let m = menu_items(true, true, false);
        assert_eq!(m[0], Some(("Remove Bookmark", true)));
        assert_eq!(menu_items(false, false, false)[0], Some(("Add Bookmark", false)));
        fn names<'a>(v: &[Option<(&'a str, bool)>]) -> Vec<&'a str> {
            v.iter().flatten().map(|(n, _)| *n).collect()
        }
        assert_eq!(names(&m), vec!["Remove Bookmark", "Manage Bookmarks", "History", "Settings"]);
        assert_eq!(m[1], None);
        assert_eq!(m.last().copied().flatten(), Some(("Settings", false)), "Settings stays disabled until 13x8");
        let dev = menu_items(false, true, true);
        assert_eq!(names(&dev), vec!["Settings"], "Dev Null hides the bookmark and history items (`BT:344`)");
        assert_eq!(history_menu(&[("T".into(), "https://a/".into()), ("".into(), "https://b/".into()), ("x\u{202e}y".into(), "u".into())]), vec!["T", "https://b/", "xy"]);
        let io = BrowserIo { can_back: true, loading: true, ..Default::default() };
        assert_eq!(buttons(&io), vec![(Button::Back, true), (Button::Forward, false), (Button::Stop, true), (Button::Menu, true)]);
        assert_eq!(buttons(&BrowserIo::default())[2], (Button::Reload, true));
        assert_eq!([Button::Back, Button::Forward, Button::Reload, Button::Stop, Button::Menu].map(button_label), ["Back", "Forward", "Reload", "Stop", "Menu"]);
        assert_eq!(LONG_PRESS, std::time::Duration::from_millis(500));
    }

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

    /// The 13w merge (Important 2 ruling): every text a page or program supplies that the browser
    /// shows (titles, the URL bar, suggestions, history items, the popover's Open in name, a
    /// page title cleaned before posting) goes through `inert_files::display_text`, so the sets
    /// cannot drift; the browser's wider set was folded into `is_hidden`.
    /// The 13w merge: the toolbar menu goes through the one `hidden_in_menu` filter, and the
    /// filter knows iTerm2's debug-build-only toolbar item.
    #[test]
    fn browser_menu_follows_the_one_menu_filter() {
        assert!(crate::menus::hidden_in_menu(menu_selector("Debug Autofill Fields"), false), "debug builds only (iTermBrowserToolbar.swift:370-378)");
        assert!(!crate::menus::hidden_in_menu(menu_selector("Debug Autofill Fields"), true));
        for dev_null in [false, true] {
            for (label, _) in menu_items(false, true, dev_null).into_iter().flatten() {
                assert!(!menu_selector(label).is_empty(), "{label} has its iTerm2 action");
                assert!(!crate::menus::hidden_in_menu(menu_selector(label), false), "{label} shows in release");
            }
        }
        let live = include_str!("browser.rs").split("#[cfg(test)]\nmod tests").next().unwrap();
        let body = &live[live.find("pub fn menu_items(").unwrap()..];
        assert!(body[..body.find("\n}\n").unwrap()].contains("crate::menus::hidden_in_menu(menu_selector(label), cfg!(debug_assertions))"));
    }

    #[test]
    fn browser_display_text_uses_the_one_sanitizer() {
        // The browser's former extras are hidden everywhere now.
        for c in ['\u{ad}', '\u{600}', '\u{6dd}', '\u{70f}', '\u{206a}', '\u{fff9}', '\u{1d173}', '\u{e0041}'] {
            assert!(crate::inert_files::is_hidden(c), "{c:?}");
            assert_eq!(crate::inert_files::display_text(&format!("a{c}b"), 10, false), "ab");
        }
        assert_eq!(display_title("a\u{ad}b\u{e0041}c"), "abc");
        assert_eq!(shown_title("x\u{2029}y\u{180e}z"), "xyz");
        // Zero width in a posted title and in a registry name: removed (RED before the merge).
        assert_eq!(crate::webhost::clean_title("a\u{200b}b\u{2060}c\td"), "abc d");
        assert_eq!(crate::web_popover::open_in_title(Some("Br\u{200b}owser")), "Open in Browser");
        assert_eq!(crate::web_popover::quick_look_target("https://a.test/\u{2060}x"), None);
        // No own removal set is left in the browser's display code.
        for (f, src) in [("browser.rs", include_str!("browser.rs")), ("webhost.rs", include_str!("webhost.rs")), ("web_popover.rs", include_str!("web_popover.rs")), ("app/browser_app.rs", include_str!("app/browser_app.rs"))] {
            let live = src.split("#[cfg(test)]\nmod tests").next().unwrap();
            for own in ["fn is_invisible", "fn bidi_format", "\\u{202e}", "\\u{202E}", "\\u{200b}", "\\u{200B}"] {
                assert!(!live.contains(own), "{f}: an own display set ({own}); use inert_files::display_text");
            }
        }
        for f in [include_str!("browser.rs"), include_str!("webhost.rs"), include_str!("web_popover.rs"), include_str!("app/browser_app.rs")] {
            assert!(f.contains("inert_files::display_text("));
        }
    }

    #[test]
    fn display_url_shows_the_real_host() {
        // Punycode as Chromium shows a look-alike host; ASCII hosts are untouched.
        assert_eq!(punycode("m\u{fc}nchen").as_deref(), Some("mnchen-3ya"));
        assert_eq!(punycode("b\u{fc}cher").as_deref(), Some("bcher-kva"));
        assert_eq!(punycode("\u{43f}\u{440}\u{438}\u{43c}\u{435}\u{440}").as_deref(), Some("e1afmkfd"));
        assert_eq!(display_url("https://example.com/a?b=c#d"), "https://example.com/a?b=c#d");
        assert_eq!(display_url("https://m\u{fc}nchen.de/p"), "https://xn--mnchen-3ya.de/p");
        // A Cyrillic "a" in "apple.com" is a different host and must look like one.
        assert_eq!(display_url("https://\u{430}pple.com/"), "https://xn--pple-43d.com/");
        assert_eq!(display_url("https://EX\u{ff0e}com/"), "https://EX.com/", "a full width dot is a dot");
        // Userinfo is shown, never hidden: the person sees who the URL names.
        assert_eq!(display_url("https://paypal.com@evil.test/x"), "https://paypal.com@evil.test/x");
        assert_eq!(display_url("https://u:p@h.test:8443/"), "https://u:p@h.test:8443/");
        // Bidirectional controls, zero width characters and controls never reach the field.
        assert_eq!(display_url("https://a.test/\u{202e}txt.exe"), "https://a.test/txt.exe");
        assert_eq!(display_url("https://a\u{200b}.test/\u{7}x\u{feff}"), "https://a.test/x");
        assert_eq!(display_url("about:blank"), "about:blank");
        assert_eq!(display_url("iterm2-about:history"), "iterm2-about:history");
        assert_eq!(display_url("https://[::1]:80/"), "https://[::1]:80/");
        assert_eq!(display_url(&format!("https://a.test/{}", "x".repeat(5000))).chars().count(), MAX_URL + 1, "bounded with an ellipsis");
        assert_eq!(display_title("a\u{202e}b\u{1}c\nd"), "abcd");
        assert_eq!(display_title(&"y".repeat(500)).chars().count(), 121);
    }

    #[test]
    fn a_url_never_reaches_the_suggestion_service() {
        for url in ["localhost:3000/reset?token=abc123", "localhost/admin?key=k", "intranet:8080", "http://u:p@h.test/", "a.b/c?token=1", "10.0.0.1:80/x"] {
            assert!(normalize_url(url).is_some(), "{url} navigates on Enter");
            assert!(!suggest_allowed(url), "{url} must not be sent");
        }
        for q in ["rust lang", "how to cook", "what is 1.5 times 2"] {
            assert!(suggest_allowed(q), "{q} is a search");
        }
        assert!(!suggest_allowed(&"x".repeat(201)) && !suggest_allowed("  "));
    }

    #[test]
    fn a_long_userinfo_cannot_hide_the_host() {
        let url = "https://accounts.google.com.signin.verify.session-7781@evil.test/path/that/goes/on";
        for cells in [10usize, 11, 14, 20, 24, 30, 40, 60] {
            let fit = fit_address(url, cells);
            assert!(fit.chars().count() <= cells, "{cells}: {fit}");
            assert!(fit.contains("evil.test"), "{cells}: the host must stay visible: {fit}");
            assert!(!fit.contains("accounts.google.com"), "{cells}: userinfo is shortened: {fit}");
        }
        assert_eq!(fit_address(url, 30), "https://acco\u{2026}@evil.test/path/\u{2026}", "path first, then userinfo");
        assert_eq!(fit_address("https://a.test/x", 40), "https://a.test/x", "what fits is untouched");
        // A long host is cut from the left: the registrable end stays.
        let long = "https://very.long.subdomain.chain.example-bank.test:8443/x";
        let fit = fit_address(long, 24);
        assert!(fit.starts_with('\u{2026}') && fit.contains("bank.test") && fit.chars().count() <= 24, "{fit}");
        // Something that is not an authority URL is cut from the right.
        assert_eq!(fit_address("about:blank", 6), "about\u{2026}");
    }

    #[test]
    fn local_page_messages_need_their_source() {
        for ok in ["iterm2-about:history", "ITERM2-ABOUT:bookmarks", "Iterm2-About:welcome", "iterm2-about:"] {
            assert!(local_source_ok(ok), "{ok}");
        }
        for bad in ["https://evil.invalid/", "", "iterm2-aboutx:y", "https://a.test/iterm2-about:history", "blob:iterm2-about:history/1", " iterm2-about:history", "iterm2-about", "xiterm2-about:history", "about:blank", "file:///iterm2-about:history"] {
            assert!(!local_source_ok(bad), "{bad:?}");
        }
    }

    #[test]
    fn the_indicator_follows_the_page_state() {
        assert_eq!(security("https://a.test/", false), Security::Secure);
        assert_eq!(security("http://a.test/", false), Security::NotSecure);
        assert_eq!(security("https://a.test/", true), Security::Failed, "an error page is not secure");
        assert_eq!(security("iterm2-about:welcome", false), Security::Local);
        assert_eq!(security("about:blank", false), Security::Local);
        assert_eq!(security("file:///C:/x.html", false), Security::File);
        assert_eq!(security("", false), Security::Local);
        assert_eq!(security("HTTPS://A.TEST/", false), Security::Secure);
        // Only Tandem's own page is Tandem's own; every other scheme shows its creator's state.
        assert_eq!(security("blob:https://evil.test/1234", false), Security::Secure, "a blob has its creator's origin");
        assert_eq!(security("blob:http://evil.test/1234", false), Security::NotSecure);
        assert_eq!(security("blob:null/1234", false), Security::NotSecure);
        assert_eq!(security("filesystem:https://a.test/temporary/x", false), Security::Secure);
        assert_eq!(security("data:text/html,<b>x</b>", false), Security::NotSecure);
        assert_eq!(security("about:srcdoc", false), Security::NotSecure);
        assert_eq!(security("javascript:alert(1)", false), Security::NotSecure);
        assert_eq!(security("ms-msdt:id", false), Security::NotSecure);
        assert_eq!(security("ITERM2-ABOUT:history", false), Security::Local);
        assert_eq!(security("blob:iterm2-about:history/1", false), Security::NotSecure, "a blob never claims Tandem's page");
    }

    #[test]
    fn typed_urls_route_by_scheme() {
        assert_eq!(typed_route("https://a.test/"), Typed::Navigate("https://a.test/".into()));
        assert_eq!(typed_route("HTTP://a.test/"), Typed::Navigate("HTTP://a.test/".into()));
        assert_eq!(typed_route("iterm2-about:history"), Typed::Navigate("iterm2-about:history".into()));
        assert_eq!(typed_route("about:blank"), Typed::Navigate("about:blank".into()));
        assert_eq!(typed_route("file:///C:/a.html"), Typed::File("file:///C:/a.html".into()));
        for other in ["mailto:a@b.test", "ms-msdt:id", "search-ms:query=x", "javascript:alert(1)", "data:text/html,x", "about:flags", "iterm2-file:/x", "iterm2-ssh:h", "ftp://h/"] {
            assert_eq!(typed_route(other), Typed::External(other.into()), "{other}");
        }
    }

    #[test]
    fn search_templates_are_judged() {
        assert_eq!(search_template("https://duckduckgo.com/?q=%@"), "https://duckduckgo.com/?q=%@");
        assert_eq!(search_template("http://intranet/s?q=%@"), "http://intranet/s?q=%@");
        for bad in ["", "file://srv/share/%@", "javascript:%@", "https://nohole.test/", "ms-msdt:%@"] {
            assert_eq!(search_template(bad), DEFAULT_SEARCH, "{bad}");
        }
        assert_eq!(suggest_template("https://s.test/?q=%@"), "https://s.test/?q=%@");
        assert_eq!(suggest_template("ftp://s.test/%@"), DEFAULT_SUGGEST);
    }

    #[test]
    fn zoom_steps_follow_iterm2() {
        // `FontTable.swift:427`: five points of percent per step, never outside what WebView2 takes.
        assert_eq!(zoom_step(100, 1, 100), 105);
        assert_eq!(zoom_step(100, -1, 100), 95);
        assert_eq!(zoom_step(25, -1, 100), 25);
        assert_eq!(zoom_step(500, 1, 100), 500);
        assert_eq!(zoom_step(180, 0, 100), 100, "Normal Size is the saved profile's zoom");
        assert_eq!(zoom_step(180, 0, 0), 100, "a missing zoom is 100");
        assert_eq!(zoom_step(0, 1, 100), 25, "a zero zoom counts as the lowest");
    }

    #[test]
    fn the_open_search_cache_falls_back_to_the_longest_prefix() {
        let mut c = OpenSearchCache::default();
        c.put("ru", open_search_results(r#"["ru",["rust","ruby","python"]]"#, "ru", T, 5));
        c.put("r", open_search_results(r#"["r",["rust"]]"#, "r", T, 5));
        let f = c.fallback("rus");
        assert_eq!(f.iter().map(|s| (s.text.as_str(), s.score)).collect::<Vec<_>>(), vec![("rust", 1_999_990)], "only matches of the new query, rescored lower (`BOS:166-175`)");
        assert!(c.fallback("x").is_empty());
        for i in 0..200 {
            c.put(&format!("q{i}"), Vec::new());
        }
        assert!(c.len() <= 64, "the cache is bounded");
    }

    #[test]
    fn the_toolbar_is_laid_out_in_one_strip() {
        let bar = Rect { x: 10.0, y: 20.0, w: 800.0, h: 32.0 };
        let g = geometry(bar, 1.0, false);
        assert!(g.dev_null.is_none());
        for r in [g.back, g.forward, g.reload, g.field, g.menu] {
            assert!(r.x >= bar.x && r.x + r.w <= bar.x + bar.w + 0.01 && r.y >= bar.y && r.y + r.h <= bar.y + bar.h + 0.01, "{r:?} inside the bar");
        }
        assert!(g.back.x < g.forward.x && g.forward.x < g.reload.x && g.reload.x < g.field.x && g.field.x < g.menu.x);
        assert!(g.field.x + g.field.w <= g.menu.x, "the field ends where the menu begins");
        assert!(g.field.contains(g.indicator.x + 1.0, g.indicator.y + 1.0) && g.indicator.x + g.indicator.w <= g.mark.x + 0.01 && g.mark.x + g.mark.w <= g.text.x + 0.01);
        assert_eq!(hit(&g, g.mark.x + 2.0, g.mark.y + 2.0), Some(Hit::Mark));
        assert_eq!(hit(&g, g.back.x + 2.0, g.back.y + 2.0), Some(Hit::Button(Button::Back)));
        assert_eq!(hit(&g, g.reload.x + 2.0, g.reload.y + 2.0), Some(Hit::Button(Button::Reload)));
        assert_eq!(hit(&g, g.menu.x + 2.0, g.menu.y + 2.0), Some(Hit::Button(Button::Menu)));
        assert_eq!(hit(&g, g.text.x + 5.0, g.text.y + 5.0), Some(Hit::Field));
        assert_eq!(hit(&g, g.indicator.x + 2.0, g.indicator.y + 2.0), Some(Hit::Indicator));
        assert_eq!(hit(&g, 0.0, 0.0), None);
        let d = geometry(bar, 1.0, true);
        let dn = d.dev_null.expect("the Dev Null indicator shows");
        assert!(dn.x > d.reload.x && dn.x + dn.w <= d.field.x + 0.01);
        assert_eq!(hit(&d, dn.x + 2.0, dn.y + 2.0), Some(Hit::DevNull));
        // A narrow pane keeps the buttons and gives the field what is left, never a negative width.
        let n = geometry(Rect { x: 0.0, y: 0.0, w: 120.0, h: 32.0 }, 1.0, true);
        assert!(n.field.w >= 0.0 && n.text.w >= 0.0);
    }

    #[test]
    fn back_and_forward_lists_follow_the_sources() {
        let mut n = NavList::default();
        for u in ["https://a/", "https://b/", "https://c/"] {
            n.source(u);
        }
        n.title("C");
        assert_eq!(n.back_items(), vec![("".into(), "https://b/".into(), -1), ("".into(), "https://a/".into(), -2)]);
        assert!(n.forward_items().is_empty());
        n.expect(-2);
        n.source("https://a/");
        assert_eq!(n.forward_items().iter().map(|i| i.2).collect::<Vec<_>>(), vec![1, 2]);
        assert_eq!(n.forward_items()[1].0, "C");
        n.source("https://d/");
        assert!(n.forward_items().is_empty(), "a new page drops the forward entries");
        assert_eq!(n.back_items().len(), 1);
        for i in 0..300 {
            n.source(&format!("https://x{i}/"));
        }
        assert!(n.back_items().len() <= 25);
        let g = geometry(Rect { x: 0.0, y: 0.0, w: 400.0, h: 32.0 }, 1.0, false);
        assert!(suggestion_rects(&g, 20.0, 20, 1000.0).len() <= MAX_SUGGESTIONS);
        assert_eq!(suggestion_rects(&g, 20.0, 5, 60.0).len(), 1, "never below the pane");
    }

    #[test]
    fn focus_handover_is_per_window_and_only_in_front() {
        let mut f: FocusSeen<u32> = FocusSeen::default();
        // Window A shows a browser pane (1) and is in front: its first placement is a change.
        assert_eq!(f.step(1, Some(1), true), Some(None));
        // Window B (a terminal pane 2) is drawn in the background: nothing moves.
        assert_eq!(f.step(2, Some(2), false), None);
        // A is drawn again (a title event): its focus did not change, so the keyboard stays where it is.
        assert_eq!(f.step(1, Some(1), true), None, "a background window's redraw never moves keyboard focus");
        // B comes to the front: now its change is acted on, once.
        assert_eq!(f.step(2, Some(2), true), Some(None));
        assert_eq!(f.step(2, Some(2), true), None);
        // A change in a window that is not in front waits until it is.
        assert_eq!(f.step(1, Some(3), false), None);
        assert_eq!(f.step(1, Some(3), true), Some(Some(1)));
        f.forget(3);
        assert_eq!(f.step(1, Some(3), true), Some(None), "a closed pane is forgotten");
    }

    #[test]
    fn browser_io_is_plain_data() {
        let io = BrowserIo::default();
        assert!(io.url.is_empty() && !io.loading && !io.can_back && !io.can_forward && io.favicon.is_none() && !io.dev_null);
    }
}
