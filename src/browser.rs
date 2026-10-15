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
/// The longest URL Termurion keeps or loads from a setting.
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
    /// The page's background color, as reported once a load completed (the minimal tab style's tint,
    /// `PS:5838-5850`); RGB only, never page text.
    pub page_color: Option<(u8, u8, u8)>,
    /// The link target of an arrangement (`PS:334`), kept and written back unchanged: bounded and clean.
    pub target: Option<String>,
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

/// What one window's placement pass does with a browser pane (Task 10 review I1). `browser_views` is
/// app-wide but each window places only its own: a pane shown here is Shown; one of this window's panes
/// (another tab, buried, zoomed away) or a view parked in this window's handle (an Undo Close stash)
/// is Hidden; any other pane belongs to another window, whose own pass decides, so this pass reports
/// nothing for it (reporting it hidden cancelled its sign-in on every redraw of this window).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placed {
    Shown,
    Hidden,
    Elsewhere,
}

pub fn placement(shown_here: bool, in_this_window: bool, in_this_handle: bool) -> Placed {
    if shown_here {
        Placed::Shown
    } else if in_this_window || in_this_handle {
        Placed::Hidden
    } else {
        Placed::Elsewhere
    }
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

/// The URL an arrangement or restoration file keeps (final review Minor 8): `saved_url` with the
/// fragment dropped (an OAuth `#access_token=` lives there) and, of `about:` pages, only
/// `about:blank`. Used on save and again on load.
pub fn arrangement_url(url: &str) -> Option<String> {
    let url = url.split('#').next().unwrap_or_default();
    let kept = saved_url(url)?;
    if crate::webhost::scheme_of(&kept) == "about" && !kept.eq_ignore_ascii_case("about:blank") {
        return None;
    }
    Some(kept)
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
pub const TRANSITION_LINK: i64 = 1;
pub const TRANSITION_RELOAD: i64 = 2;
pub const TRANSITION_BACK_FORWARD: i64 = 3;
pub const TRANSITION_REDIRECT: i64 = 4;
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

/// Whether typed text may go to the suggestion service. Text the Enter path would navigate to
/// (`normalize_url`) stays home when it has a scheme, a dot, a port, a path, a query, a fragment,
/// `@`, an IP literal or `localhost`, so `localhost:3000/x?token=y` never leaves. A single bare
/// word ("rust") is a search for iTerm2 (`BOS:30-38` skips only empty and `https://`), so it is sent.
pub fn suggest_allowed(query: &str) -> bool {
    let t = query.trim();
    if !wants_open_search(query) || t.chars().count() > 200 {
        return false;
    }
    let urlish = t.contains(['.', ':', '/', '?', '@', '#']) || t.to_ascii_lowercase().starts_with("localhost");
    !(urlish && normalize_url(query).is_some())
}

/// Whether a message's source is one of Termurion's own `iterm2-about:` pages: the scheme, compared
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

/// What the toolbar menu knows of the reader when Reader Mode is offered (`BT:313-343`): whether each
/// mode is on (the entry then offers its exit) and whether the page has a URL (the entries' enabling).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReaderMenu {
    pub reader: bool,
    pub distraction: bool,
    pub has_url: bool,
    /// Whether "Ask AI\u{2026}" is enabled: a page with a URL that tools may read (never Dev Null, never
    /// a Termurion, local or extension page: `ai_browser::offerable`).
    pub ask_ai: bool,
}

/// The toolbar menu's "Ask AI\u{2026}" entry: its label is its id (`browser_menu_picked` reads the label).
pub const ASK_AI_ID: &str = "Ask AI\u{2026}";

/// The toolbar menu: None is a separator, the flag is whether the item is enabled (`BT:306-420`).
/// Dev Null hides the bookmark and history items; Settings stays disabled until 13x8 builds the
/// page.
pub fn menu_items(bookmarked: bool, has_url: bool, dev_null: bool, reader: Option<ReaderMenu>) -> Vec<Option<(&'static str, bool)>> {
    let mut out = Vec::new();
    // The reader block does not depend on Dev Null (`BT:313-345`). Ask AI is enabled only for a page
    // the AI tools may read (`ReaderMenu::ask_ai`).
    if let Some(r) = reader {
        out.push(Some((if r.reader { "Exit Reader Mode" } else { "Reader Mode" }, r.has_url)));
        out.push(Some((if r.distraction { "Exit Distraction Removal" } else { "Remove Distractions" }, r.has_url)));
        out.push(None);
        out.push(Some((ASK_AI_ID, r.ask_ai)));
    }
    if !dev_null {
        out.push(Some((if bookmarked { "Remove Bookmark" } else { "Add Bookmark" }, has_url)));
        out.push(None);
        out.push(Some(("Manage Bookmarks", true)));
        out.push(Some(("History", true)));
        out.push(None);
    }
    out.push(Some(("Settings", true)));
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
        "Reader Mode" | "Exit Reader Mode" => "readerModeMenuItemSelected",
        "Remove Distractions" | "Exit Distraction Removal" => "distractionRemovalMenuItemSelected",
        ASK_AI_ID => "askAIMenuItemSelected",
        "Debug Autofill Fields" => "debugAutofillMenuItemSelected:",
        _ => "",
    }
}

/// The lines of a Back or Forward history menu: the title, else the URL (`BT:520-560`), the URL
/// through the one shortener at the menu's width in cells (final residual 1).
pub fn history_menu(items: &[(String, String)], cells: usize) -> Vec<String> {
    items.iter().map(|(title, url)| if title.trim().is_empty() { fit_address(&display_url(url), cells) } else { display_title(title) }).collect()
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
/// trust in Unicode; Termurion shows every IDN so, Decision). Full width dots are dots.
fn ascii_host(host: &str) -> String {
    let host: String = host.chars().map(|c| if matches!(c, '\u{3002}' | '\u{FF0E}' | '\u{FF61}') { '.' } else { c }).collect();
    host.split('.').map(|label| if label.is_ascii() { label.to_string() } else { punycode(&label.to_lowercase()).map_or_else(|| label.to_string(), |p| format!("xn--{p}")) }).collect::<Vec<_>>().join(".")
}

/// What the URL bar shows of a page's address: no control, bidirectional or zero width
/// character, the real host (punycode for a non-ASCII one), the user information in front of it
/// when the URL has some (never hidden), at most `MAX_URL` characters.
pub fn display_url(url: &str) -> String {
    let clean = crate::inert_files::display_text(url, usize::MAX, false);
    let Some((scheme, rest)) = clean.split_once("://").filter(|(s, _)| !s.is_empty() && !s.contains('/')) else {
        return cut_end(&clean, MAX_URL);
    };
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(end);
    let (userinfo, hostport) = authority.rsplit_once('@').map_or(("", authority), |(u, h)| (u, h));
    let (host, port) = if hostport.starts_with('[') {
        hostport.split_once(']').map_or((hostport, ""), |(h, p)| (&hostport[..h.len() + 1], p))
    } else {
        hostport.rsplit_once(':').filter(|(_, p)| p.bytes().all(|b| b.is_ascii_digit())).map_or((hostport, ""), |(h, _)| (h, &hostport[h.len()..]))
    };
    // The host is what a person must see (a page can push it out by padding what comes before it):
    // the host and port first, then the user information shortened to what is left of a quarter of
    // the room, then the path takes the rest. A host too long to be one keeps its end.
    let host = ascii_host(host);
    let host = if host.chars().count() > MAX_HOST_SHOWN { format!("\u{2026}{}", tail_chars(&host, MAX_HOST_SHOWN)) } else { host };
    let at = if authority.contains('@') { format!("{}@", cut_end(userinfo, MAX_USERINFO_SHOWN)) } else { String::new() };
    // A scheme and a port of any length cannot push the host out (round 2).
    let head = format!("{}://{at}{host}{}", cut_end(scheme, MAX_SCHEME_SHOWN), cut_end(port, MAX_PORT_SHOWN));
    let room = MAX_URL.saturating_sub(head.chars().count());
    format!("{head}{}", cut_end(tail, room.max(1)))
}

/// The host (and port) `display_url` shows for `url`: punycode for a non-ASCII host, nothing hidden;
/// empty for a URL with no authority.
pub fn display_host(url: &str) -> String {
    let shown = display_url(url);
    let Some((_, rest)) = shown.split_once("://") else { return String::new() };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    authority.rsplit('@').next().unwrap_or("").to_string()
}

/// The most of a user name or password a displayed URL shows (the rest is an ellipsis).
const MAX_USERINFO_SHOWN: usize = 64;
/// The most of a scheme and of a `:port` a displayed URL shows.
const MAX_SCHEME_SHOWN: usize = 32;
const MAX_PORT_SHOWN: usize = 6;
/// The most of a host a displayed URL shows (a real host is 253 at most).
const MAX_HOST_SHOWN: usize = 253;

/// The last `n` characters of `text`.
fn tail_chars(text: &str, n: usize) -> String {
    let count = text.chars().count();
    text.chars().skip(count.saturating_sub(n)).collect()
}

/// `text` cut to `max` characters, with an ellipsis after the cut (so at most `max` + 1 characters).
fn cut_end(text: &str, max: usize) -> String {
    if text.chars().count() <= max { text.to_string() } else { text.chars().take(max).chain(['\u{2026}']).collect() }
}

/// The cells `text` takes on a line: its display width, a character the width table calls zero or
/// unknown counted as one (never under, so a fitted text never overflows).
pub fn text_cells(text: &str) -> usize {
    text.chars().map(|c| unicode_width::UnicodeWidthChar::width(c).unwrap_or(1).max(1)).sum()
}

/// `text` cut from the right to at most `cells` cells, an ellipsis after the cut.
fn cut_cells(text: &str, cells: usize) -> String {
    if text_cells(text) <= cells {
        return text.to_string();
    }
    if cells == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut used = 0;
    for c in text.chars() {
        let w = text_cells(c.encode_utf8(&mut [0; 4]));
        if used + w > cells - 1 {
            break;
        }
        out.push(c);
        used += w;
    }
    out.push('\u{2026}');
    out
}

/// A URL's path, query and fragment cut to `cells`: whole, or its leading separator and more behind an
/// ellipsis, or nothing; never an ellipsis right after the host, which would read as a host cut short.
fn cut_path(tail: &str, cells: usize) -> String {
    if text_cells(tail) <= cells {
        tail.to_string()
    } else if cells >= 2 {
        cut_cells(tail, cells)
    } else {
        String::new()
    }
}

/// A URL of any display as it fits `cells` cells: THE shortener (round 2, class fix; final residual
/// 1, round 4), used by the address field, the link tip, the suggestion rows, the Open Quickly rows
/// and the history menus, each with the width it draws in. The result is NEVER wider than `cells`
/// (`text_cells`), so no surface clips it afterwards. Host text is shown only as whole labels ending
/// at the full registrable host (public suffix and one label, `registrable_labels`): the path is cut
/// first (from the right), then the user information is shortened to a marked prefix, then
/// subdomain labels go from the LEFT behind an ellipsis, then the scheme, user information and path
/// give way to the host, and when even the registrable host does not fit, the result shows NO host
/// character at all (an ellipsis; the lock mark and a wider pane carry the address), since any part
/// of a host can form another brand's domain. A URL after a lead-in text and before a closing quote
/// (the "Navigate to" row) is fitted in what those leave. Not a URL: cut from the right.
pub fn fit_address(shown: &str, cells: usize) -> String {
    if text_cells(shown) <= cells {
        return shown.to_string();
    }
    if let Some((lead, url, trail)) = embedded_url(shown) {
        let used = text_cells(lead) + text_cells(trail);
        // No room for the URL beside the lead-in: the URL alone (it is what must be seen).
        return if cells > used { format!("{lead}{}{trail}", fit_address(url, cells - used)) } else { fit_address(url, cells) };
    }
    let Some((scheme, rest)) = shown.split_once("://").filter(|(s, _)| !s.is_empty() && !s.contains('/')) else {
        return cut_cells(shown, cells);
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
        let n = text_cells(&head);
        if n <= cells {
            return format!("{head}{}", cut_path(tail, cells - n));
        }
    }
    // The host does not fit whole: whole labels go from the left, never into the registrable part.
    let (host, port) = match hostport.rsplit_once(':').filter(|(h, _)| !h.contains(']') || h.ends_with(']')).filter(|(_, p)| p.bytes().all(|b| b.is_ascii_digit())) {
        Some((h, _)) => (h, &hostport[h.len()..]),
        None => (hostport, ""),
    };
    let labels: Vec<&str> = host.split('.').collect();
    let floor = registrable_labels(host, &labels);
    for marker in if userinfo.is_some() { ["\u{2026}@", ""] } else { ["", ""] } {
        for keep in (floor..labels.len()).rev() {
            let head = format!("{prefix}{marker}\u{2026}{}{port}", labels[labels.len() - keep..].join("."));
            let n = text_cells(&head);
            if n <= cells {
                return format!("{head}{}", cut_path(tail, cells - n));
            }
        }
    }
    // The host does not fit even with one label beside the scheme: the scheme, user information and
    // path go (the leading ellipsis stands for them), never a character of a label (final review I3).
    for with_port in [true, false] {
        for keep in (floor..=labels.len()).rev() {
            let head = format!("\u{2026}{}{}", labels[labels.len() - keep..].join("."), if with_port { port } else { "" });
            if text_cells(&head) <= cells {
                return head;
            }
        }
    }
    // The registrable host alone, when exactly it fits.
    let reg = labels[labels.len().saturating_sub(floor)..].join(".");
    if !reg.is_empty() && text_cells(&reg) <= cells {
        return reg;
    }
    // Narrower than the registrable host: no host character at all (round 4), only the marker.
    cut_cells("\u{2026}", cells)
}

/// How many trailing labels of a host are never cut: the public suffix and one label (the attacker's
/// own). The multi-label suffixes are the embedded `PUBLIC_SUFFIXES` (longest match wins); a host the
/// table does not know falls back to the shape rule (a two-letter country code behind a short second
/// level, `co.uk`, is one suffix). One trailing dot is not a label. A numeric or bracketed host is whole.
fn registrable_labels(host: &str, labels: &[&str]) -> usize {
    let n = labels.len();
    if host.starts_with('[') || labels.iter().all(|l| l.bytes().all(|b| b.is_ascii_digit())) {
        return n;
    }
    let dot = usize::from(labels.last() == Some(&""));
    let core = &labels[..n - dot];
    let m = core.len();
    let known = suffix_labels(core);
    if known > 0 {
        return (known + 1).min(m) + dot;
    }
    if m < 3 {
        return n;
    }
    (if core[m - 1].chars().count() == 2 && core[m - 2].chars().count() <= 3 { 3 } else { 2 }) + dot
}

/// How many trailing labels of `labels` the longest matching `PUBLIC_SUFFIXES` entry covers (0: none).
/// A `*` label matches any one label, a label ending in `*` is a prefix.
fn suffix_labels(labels: &[&str]) -> usize {
    let mut best = 0;
    for entry in PUBLIC_SUFFIXES {
        let parts: Vec<&str> = entry.split('.').collect();
        if parts.len() > labels.len() || parts.len() <= best {
            continue;
        }
        let tail = &labels[labels.len() - parts.len()..];
        let hit = parts.iter().zip(tail).all(|(p, l)| match p.strip_suffix('*') {
            Some(prefix) => l.len() >= prefix.len() && l.is_char_boundary(prefix.len()) && l[..prefix.len()].eq_ignore_ascii_case(prefix),
            None => p.eq_ignore_ascii_case(l),
        });
        if hit {
            best = parts.len();
        }
    }
    best
}

/// The multi-label public suffixes the shortener keeps whole, so the attacker's label in front of one
/// is never cut away: free-hosting suffixes (the private section of the Public Suffix List, the
/// high-volume ones), country second levels and the gTLD-based ones. A bounded table, display only
/// (no security decision reads it); refresh it from publicsuffix.org/list when it ages. `*` is any one
/// label, a label ending in `*` is a prefix.
pub const PUBLIC_SUFFIXES: &[&str] = &[
    // Free hosting and platform suffixes.
    "github.io", "githubusercontent.com", "gitlab.io", "googleusercontent.com", "blogspot.com", "blogspot.co.uk", "appspot.com",
    "web.app", "firebaseapp.com", "cloudfront.net", "s3.amazonaws.com", "s3.*.amazonaws.com", "s3-*.amazonaws.com",
    "s3.dualstack.*.amazonaws.com", "s3-website.*.amazonaws.com", "s3-website-*.amazonaws.com", "execute-api.*.amazonaws.com",
    "elasticbeanstalk.com", "elb.amazonaws.com", "compute.amazonaws.com", "blob.core.windows.net", "azurewebsites.net",
    "cloudapp.net", "cloudapp.azure.com", "azurestaticapps.net", "azureedge.net", "trafficmanager.net", "file.core.windows.net",
    "queue.core.windows.net", "table.core.windows.net", "web.core.windows.net", "herokuapp.com", "herokussl.com", "netlify.app",
    "netlify.com", "vercel.app", "now.sh", "pages.dev", "workers.dev", "r2.dev", "fly.dev", "onrender.com", "ngrok.io",
    "ngrok-free.app", "ngrok.app", "ngrok.dev", "glitch.me", "repl.co", "replit.app", "replit.dev", "translate.goog",
    "wordpress.com", "weebly.com", "wixsite.com", "squarespace.com", "myshopify.com", "webflow.io", "framer.app", "framer.website",
    "carrd.co", "000webhostapp.com", "infinityfreeapp.com", "rf.gd", "epizy.com", "pythonanywhere.com", "surge.sh", "render.com",
    "railway.app", "up.railway.app", "deno.dev", "web.val.run", "stackblitz.com", "codesandbox.io", "csb.app", "gitpod.io",
    "bitbucket.io", "readthedocs.io", "notion.site", "super.site", "typedream.app", "bubbleapps.io", "mystrikingly.com",
    "webnode.page", "sites.google.com", "page.link", "withgoogle.com", "run.app", "cloudfunctions.net", "ondigitalocean.app",
    "digitaloceanspaces.com", "backblazeb2.com", "dweb.link", "ipfs.io", "on.fleek.co", "4everland.app", "oraclecloud.com",
    "oraclecloudapps.com", "linodeobjects.com", "scw.cloud", "fastly-edge.com", "fastlylb.net", "global.ssl.fastly.net", "edgekey.net",
    "akamaized.net", "akamaihd.net", "b-cdn.net", "kinsta.cloud", "wpengine.com", "wpenginepowered.com", "myfritz.net",
    "duckdns.org", "no-ip.org", "no-ip.com", "ddns.net", "dyndns.org", "dyndns.biz", "hopto.org", "zapto.org", "servebeer.com",
    "serveftp.com", "servegame.com", "freeddns.org", "chickenkiller.com", "mooo.com", "afraid.org", "dynu.net", "myddns.rocks",
    "zendesk.com", "freshdesk.com", "atlassian.net", "jfrog.io", "jotform.com", "typeform.com", "wufoo.com",
    "forms.gle", "my.canva.site", "canva.site", "lovable.app", "bolt.host", "dev.tunnels.ms", "devtunnels.ms", "loca.lt",
    "trycloudflare.com", "cfargotunnel.com", "vercel.dev", "nextjs.app", "hf.space", "streamlit.app", "gradio.live",
    // gTLD-based country and region names.
    "uk.com", "us.com", "eu.com", "de.com", "gb.com", "gb.net", "uk.net", "se.net", "co.com", "ru.com", "za.com", "ar.com",
    "br.com", "cn.com", "jpn.com", "kr.com", "no.com", "qc.com", "sa.com", "se.com", "uy.com", "hu.com", "jp.net", "in.net",
    // Country second levels the shape rule misreads or misses.
    "co.uk", "org.uk", "ac.uk", "gov.uk", "me.uk", "ltd.uk", "plc.uk", "net.uk", "sch.uk", "nhs.uk", "police.uk",
    "com.au", "net.au", "org.au", "edu.au", "gov.au", "asn.au", "id.au", "co.jp", "ne.jp", "or.jp", "ac.jp", "go.jp", "ad.jp", "ed.jp",
    "gr.jp", "lg.jp", "co.nz", "org.nz", "net.nz", "govt.nz", "ac.nz", "school.nz", "geek.nz", "com.br", "net.br", "org.br", "gov.br",
    "edu.br", "com.cn", "net.cn", "org.cn", "gov.cn", "edu.cn", "ac.cn", "co.in", "net.in", "org.in", "firm.in", "gen.in", "ind.in",
    "ac.in", "edu.in", "gov.in", "res.in", "co.za", "org.za", "net.za", "gov.za", "ac.za", "web.za", "com.mx", "org.mx", "gob.mx",
    "edu.mx", "net.mx", "com.ar", "org.ar", "gob.ar", "net.ar", "edu.ar", "com.tr", "org.tr", "net.tr", "gov.tr", "edu.tr", "av.tr",
    "co.kr", "or.kr", "ne.kr", "go.kr", "ac.kr", "re.kr", "com.sg", "org.sg", "edu.sg", "gov.sg", "net.sg", "com.hk", "org.hk",
    "edu.hk", "gov.hk", "net.hk", "idv.hk", "com.tw", "org.tw", "edu.tw", "gov.tw", "net.tw", "idv.tw", "co.il", "org.il", "ac.il",
    "gov.il", "net.il", "muni.il", "co.id", "or.id", "ac.id", "go.id", "web.id", "my.id", "biz.id", "com.my", "org.my", "edu.my",
    "gov.my", "net.my", "com.ua", "org.ua", "net.ua", "gov.ua", "edu.ua", "in.ua", "kiev.ua", "com.pl", "org.pl", "net.pl", "edu.pl",
    "gov.pl", "info.pl", "biz.pl", "waw.pl", "com.ru", "org.ru", "net.ru", "msk.ru", "spb.ru", "com.ph", "org.ph", "net.ph", "gov.ph",
    "edu.ph", "com.vn", "net.vn", "org.vn", "gov.vn", "edu.vn", "co.th", "or.th", "ac.th", "go.th", "in.th", "net.th", "com.pk",
    "org.pk", "net.pk", "gov.pk", "edu.pk", "com.bd", "org.bd", "net.bd", "gov.bd", "edu.bd", "com.ng", "org.ng", "gov.ng", "edu.ng",
    "co.ke", "or.ke", "go.ke", "ac.ke", "com.eg", "org.eg", "gov.eg", "edu.eg", "com.sa", "org.sa", "gov.sa", "edu.sa", "com.co",
    "org.co", "net.co", "gov.co", "edu.co", "com.pe", "org.pe", "gob.pe", "edu.pe", "com.ve", "org.ve", "gob.ve", "com.ec", "org.ec",
    "gob.ec", "edu.ec", "com.uy", "org.uy", "gub.uy", "edu.uy", "com.gt", "com.do", "com.pa", "com.cy", "org.cy", "com.mt", "com.gr",
    "gr.com", "co.at", "or.at", "ac.at", "gv.at", "co.hu", "org.hu", "co.rs", "org.rs", "ac.rs", "co.ro", "com.ro", "org.ro",
    "k12.*.us", "lib.*.us", "cc.*.us", "pvt.k12.*.us", "co.us", "com.de", "co.de", "ac.ae", "co.ae", "net.ae", "org.ae", "gov.ae",
    "co.ve", "co.cr", "co.ug", "co.tz", "co.zw", "co.zm", "com.lk", "com.np", "com.kh", "com.mm", "com.bn", "com.bo", "com.py",
    "com.cu", "com.ni", "com.hn", "com.sv", "com.lb", "com.jo", "com.kw", "com.qa", "com.om", "com.bh", "com.ye", "com.iq",
];

/// A URL inside a longer display text: what comes before its scheme, the URL, and a closing quote.
/// None when the text is only the URL.
fn embedded_url(shown: &str) -> Option<(&str, &str, &str)> {
    let at = shown.find("://")?;
    let start = shown[..at].rfind(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))).map_or(0, |p| p + shown[p..].chars().next().map_or(1, char::len_utf8));
    if start == at {
        return None;
    }
    let (lead, url) = shown.split_at(start);
    let (url, trail) = match url.strip_suffix('\u{201d}') {
        Some(u) => (u, "\u{201d}"),
        None => (url, ""),
    };
    (!lead.is_empty() || !trail.is_empty()).then_some((lead, url, trail))
}

/// What the lock mark beside the address shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Security {
    /// Termurion's own `iterm2-about:` page or a blank one; nothing else is Termurion's.
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
        // Termurion's own pages (and the blank one, and an empty address before any load).
        "iterm2-about" => Security::Local,
        "about" if url.trim().eq_ignore_ascii_case("about:blank") => Security::Local,
        "" if url.trim().is_empty() => Security::Local,
        // data:, javascript:, other about: pages and every other scheme: not Termurion's, not secure.
        _ => Security::NotSecure,
    }
}

/// How a typed URL is loaded: web pages and Termurion's own pages directly, a `file:` URL through the
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
    /// The session indicators' strip between the field and the menu button (zero width when none
    /// show or there is no room), and each shown indicator's whole button in it; the strip clips
    /// what overflows (13x6 Task 10, `BT:217-238`, `BIV:113-156`).
    pub strip: Rect,
    pub status: [Rect; MAX_STATUS],
    pub status_n: usize,
}

/// The most indicators a strip lays out (iTerm2's ten less the ones a pane never shows).
pub const MAX_STATUS: usize = 11;
/// The URL bar's least width, the strip's width range, the space before the menu button and the
/// gap and size of one indicator button, in points (`BT:217-238`, `BIV:113-156`).
pub const MIN_URL_BAR: f32 = 250.0;
pub const MIN_STRIP: f32 = 24.0;
pub const MAX_STRIP: f32 = 120.0;
pub const STRIP_SPACING: f32 = 12.0;
pub const BUTTON_GAP: f32 = 4.0;
pub const BUTTON_W: f32 = 20.0;

/// The strip's width in points for the room between `left` and `right`: None below 250 + 24 + 12
/// points, else 30 percent of the spare room clamped to 24..120.
pub fn strip_width(left: f32, right: f32) -> Option<f32> {
    let room = right - left;
    (room >= MIN_URL_BAR + MIN_STRIP + STRIP_SPACING).then(|| ((room - MIN_URL_BAR - STRIP_SPACING) * 0.3).clamp(MIN_STRIP, MAX_STRIP))
}

/// The left x of each of `n` buttons laid out in `strip` (left, right in points): right-aligned
/// when they fit, else from the strip's left edge with the overflow clipped by the painter.
pub fn place(n: usize, strip: (f32, f32)) -> Vec<f32> {
    let total = if n == 0 { 0.0 } else { n as f32 * BUTTON_W + (n - 1) as f32 * BUTTON_GAP };
    let start = if total <= strip.1 - strip.0 { strip.1 - total } else { strip.0 };
    (0..n).map(|i| start + i as f32 * (BUTTON_W + BUTTON_GAP)).collect()
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Hit {
    /// The nth shown session indicator.
    Status(usize),
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
    geometry_status(bar, scale, dev_null, proxy, 0)
}

/// `geometry_with` and the strip of `n` session indicators; the field takes the room left.
pub fn geometry_status(bar: Rect, scale: f32, dev_null: bool, proxy: bool, n: usize) -> Geometry {
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
    // The strip sits left of the menu button with its spacing between (`BT:233-234`).
    let sc = scale.max(0.01);
    let mut strip = Rect { x: menu.x, y: bar.y, w: 0.0, h: bar.h };
    let mut status = [Rect { x: 0.0, y: 0.0, w: 0.0, h: 0.0 }; MAX_STATUS];
    let n = n.min(MAX_STATUS);
    if n > 0 {
        if let Some(w) = strip_width(fx / sc, menu.x / sc).map(|w| w * sc) {
            let right = menu.x - STRIP_SPACING * sc;
            strip = Rect { x: right - w, y: bar.y, w, h: bar.h };
            for (i, px) in place(n, (strip.x / sc, right / sc)).into_iter().enumerate() {
                status[i] = Rect { x: px * sc, y: bar.y, w: BUTTON_W * sc, h: bar.h };
            }
        }
    }
    let status_n = if strip.w > 0.0 { n } else { 0 };
    let field_end = if strip.w > 0.0 { strip.x - gap } else { menu.x - gap };
    let field = Rect { x: fx, y: bar.y + 4.0 * scale, w: (field_end - fx).max(0.0), h: (bar.h - 8.0 * scale).max(0.0) };
    let indicator = Rect { x: field.x, y: field.y, w: field.h.min(field.w), h: field.h };
    let mark = Rect { x: indicator.x + indicator.w, y: field.y, w: (field.h * 0.8).min((field.w - indicator.w).max(0.0)), h: field.h };
    let text = Rect { x: mark.x + mark.w, y: field.y, w: (field.w - indicator.w - mark.w - gap).max(0.0), h: field.h };
    Geometry { back, forward, reload, dev_null: dn, proxy: px, field, indicator, mark, text, menu, strip, status, status_n }
}

/// The part of the toolbar under a point.
pub fn hit(g: &Geometry, x: f32, y: f32) -> Option<Hit> {
    let buttons = [(g.back, Button::Back), (g.forward, Button::Forward), (g.reload, Button::Reload), (g.menu, Button::Menu)];
    if let Some((_, b)) = buttons.iter().find(|(r, _)| r.w > 0.0 && r.contains(x, y)) {
        return Some(Hit::Button(*b));
    }
    // A button the strip clips away is not there.
    if g.strip.w > 0.0 && g.strip.contains(x, y) {
        if let Some(i) = g.status[..g.status_n].iter().position(|r| r.contains(x, y)) {
            return Some(Hit::Status(i));
        }
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
    /// A new committed URL: a step Termurion asked for lands on its entry, anything else is a new
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

    /// The step Termurion expected was cancelled (a parked navigation): nothing is pending.
    pub fn forget_expected(&mut self) {
        self.pending = None;
    }

    /// Termurion is about to go `steps` entries (negative is back).
    pub fn expect(&mut self, steps: i32) {
        self.pending = Some(steps);
    }

    /// The URL `steps` entries from the current one (negative is back), when the list holds it.
    pub fn target_url(&self, steps: i32) -> Option<&str> {
        let at = i64::try_from(self.index).ok()?.checked_add(i64::from(steps))?;
        usize::try_from(at).ok().and_then(|i| self.entries.get(i)).map(|e| e.0.as_str())
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

// --- Opening links (Task 9) ---

/// Where an opened link goes (`iTermOpenStyle`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Style {
    Tab,
    Window,
    Vertical,
    Horizontal,
}

/// What opening a link does (`NSWorkspace+iTerm.m`).
#[derive(Clone, Debug, PartialEq)]
pub enum Open {
    /// The system opens it (through `guard_open` and the 13r rules, as before).
    System,
    /// A browser pane shows it.
    Local(Style),
    /// Ask first: "Open in Termurion?" (`file`: the text about a file).
    AskOpenInApp { file: bool },
    /// The WebView2 Runtime is missing: offer it.
    Upsell,
    Nothing,
}

pub const OPEN_HEADING: &str = "Open in Termurion?";
pub const OPEN_WEB_TEXT: &str = "Termurion can display web pages! Would you like to open this link in Termurion?";
pub const OPEN_FILE_TEXT: &str = "Termurion can display files like this in its built-in web browser! Would you like to open this link in Termurion?";
pub const USE_DEFAULT_BROWSER: &str = "Use Default Browser";
pub const USE_DEFAULT_APP: &str = "Use Default App";
pub const OPEN_IN_TERMURION: &str = "Open in Termurion";
pub const UPSELL_HEADING: &str = "WebView2 Runtime Required";
pub const UPSELL_TEXT: &str = "Termurion can display web pages! But first you must install the WebView2 Runtime.";
pub const DOWNLOAD: &str = "Download";
pub const USE_SYSTEM_BROWSER: &str = "Use System Browser";
/// Microsoft's own WebView2 page (Decision); opened only through `guard_open`.
pub const PLUGIN_URL: &str = "https://developer.microsoft.com/microsoft-edge/webview2/";
pub const OPEN_LINKS_ID: &str = "NoSyncOpenLinksInApp";
pub const OPEN_FILE_LINKS_ID: &str = "NoSyncOpenLinksInAppForFileURL";
pub const UPSELL_ID: &str = "NoSyncBrowserUpsell";

/// Whether the browser may show a file with this extension (`it_localBrowserIsCompatibleWithFileURL`,
/// `WS:57-83`): web formats, PDF and images. Never a runnable (iTerm2 lists JavaScript source;
/// Termurion does not, P12) and never an alternate data stream.
pub fn compatible_file(ext: &str) -> bool {
    let e = ext.strip_prefix('.').unwrap_or(ext).to_ascii_lowercase();
    matches!(e.as_str(), "html" | "htm" | "xhtml" | "xml" | "svg" | "css" | "pdf" | "png" | "jpg" | "jpeg" | "gif" | "webp" | "heic")
}

/// The http or https URL a link may open in a browser pane: a host, no white space, control or
/// bidi character, at most `MAX_URL`, credentials dropped. A terminal's text is untrusted, so this
/// is the one gate before `open_link` (P12).
pub fn web_link(url: &str) -> Option<String> {
    saved_url(&crate::web_popover::quick_look_target(url)?)
}

/// What opening `url` does (`it_openURL`, `WS:190-420`). `upsell` is true for a person's click
/// (iTerm2 passes it for every link); `allowed` is `BrowserProfiles` on with the runtime found;
/// `runtime` is false only when the browser is on and the runtime is missing (`shouldOfferPlugin`).
/// The saved values are the remembered button of the open-links and upsell warnings.
#[allow(clippy::too_many_arguments)]
pub fn open_decision(url: &str, style: Style, upsell: bool, allowed: bool, runtime: bool, saved_open_links: Option<i64>, saved_upsell: Option<i64>) -> Open {
    let scheme = crate::webhost::scheme_of(url);
    let file = scheme == "file";
    let web = matches!(scheme.as_str(), "http" | "https");
    if file {
        let name = url.split(['?', '#']).next().unwrap_or("").rsplit('/').next().unwrap_or("");
        let ext = name.rsplit_once('.').map_or("", |(_, e)| e);
        if !compatible_file(ext) {
            return Open::System;
        }
    } else if !web {
        return Open::System;
    }
    if !upsell && !allowed {
        return Open::System;
    }
    if !allowed {
        // The runtime is missing: offer it, unless Use System Browser was remembered.
        return if runtime || saved_upsell == Some(1) { Open::System } else { Open::Upsell };
    }
    match style {
        Style::Vertical | Style::Horizontal => Open::Local(style),
        Style::Tab | Style::Window => match saved_open_links {
            Some(0) => Open::System,
            Some(1) => Open::Local(style),
            _ => Open::AskOpenInApp { file },
        },
    }
}

/// Whether `url` would open in Termurion or ask to (`it_urlIsConditionallyLocallyOpenable`, test-only
/// path of `WS:330-406`): the context menu's split items and Shift+Win-click.
pub fn conditionally_openable(url: &str, allowed: bool, runtime: bool, saved_open_links: Option<i64>, saved_upsell: Option<i64>) -> bool {
    if web_link(url).is_none() {
        return false;
    }
    if !allowed {
        return !runtime && saved_upsell != Some(1);
    }
    saved_open_links != Some(0)
}

/// The warning for "open this link in Termurion?" (`WS:384-410`).
pub fn open_link_spec(file: bool) -> crate::warnings::Spec {
    crate::warnings::Spec {
        id: Some(if file { OPEN_FILE_LINKS_ID } else { OPEN_LINKS_ID }.to_string()),
        kind: crate::warnings::Kind::Permanent,
        heading: OPEN_HEADING.to_string(),
        text: if file { OPEN_FILE_TEXT } else { OPEN_WEB_TEXT }.to_string(),
        labels: vec![if file { USE_DEFAULT_APP } else { USE_DEFAULT_BROWSER }.to_string(), OPEN_IN_TERMURION.to_string(), crate::api_auth::CANCEL.to_string()],
        cancel: Some(2),
        never_remember: Vec::new(),
    }
}

/// The runtime upsell (`GW:136-160`): only Use System Browser is remembered (remembering Download
/// would ask again for ever; Cancel is never remembered).
pub fn upsell_spec() -> crate::warnings::Spec {
    crate::warnings::Spec {
        id: Some(UPSELL_ID.to_string()),
        kind: crate::warnings::Kind::Permanent,
        heading: UPSELL_HEADING.to_string(),
        text: UPSELL_TEXT.to_string(),
        labels: vec![DOWNLOAD.to_string(), USE_SYSTEM_BROWSER.to_string(), crate::api_auth::CANCEL.to_string()],
        cancel: Some(2),
        never_remember: vec![0],
    }
}

/// The longest search text (characters): the whole selection up to here goes to the search engine.
pub const MAX_SEARCH: usize = 512;

/// The text Search the Web for Selection sends (`CMH:1179-1186` searches the whole selection): white
/// space and control characters become one space, the ends are trimmed and it is cut to `MAX_SEARCH`.
/// None when nothing is left.
pub fn search_query(selection: &str) -> Option<String> {
    let cleaned: String = selection.chars().filter_map(|c| if c.is_whitespace() || c.is_control() { Some(' ') } else { (!crate::inert_files::is_hidden(c)).then_some(c) }).collect();
    let words: Vec<&str> = cleaned.split(|c: char| c.is_whitespace() || c.is_control()).filter(|w| !w.is_empty()).collect();
    let text: String = words.join(" ").chars().take(MAX_SEARCH).collect();
    let text = text.trim_end().to_string();
    (!text.is_empty()).then_some(text)
}

/// The selection items of the pane menu (`CMH:549-560`): Search is added unconditionally.
pub fn web_context_items(short: Option<&str>, locally_openable: bool, quick_look: bool) -> Vec<&'static str> {
    let mut v = Vec::new();
    if short.is_some() {
        v.push("Open Selection as URL");
        if locally_openable {
            v.push("Open URL in Vertical Split Pane");
            v.push("Open URL in Horizontal Split Pane");
        }
        if quick_look {
            v.push("Quick Look Link");
        }
    }
    v.push("Search the Web for Selection");
    v
}

#[cfg(test)]
mod tests {
    /// Review I1: two windows, A holding pane 1 (shown) and B holding pane 2 (shown). B's pass sees
    /// pane 1 as another window's: nothing is reported for it, so A's sign-in stays. A's own pass hides
    /// its pane only when its tab is not active (or it is stashed in A's handle).
    #[test]
    fn a_pane_of_another_window_is_neither_shown_nor_hidden_here() {
        use super::{placement, Placed};
        // Window B's pass over pane 1 (A's pane, A's handle): not shown in B, not B's, not in B's handle.
        assert_eq!(placement(false, false, false), Placed::Elsewhere);
        // Window A's pass over pane 1: shown; then its tab goes to the background: hidden.
        assert_eq!(placement(true, true, true), Placed::Shown);
        assert_eq!(placement(false, true, true), Placed::Hidden);
        // An Undo Close stash in A's handle with no pane in A: hidden by A.
        assert_eq!(placement(false, false, true), Placed::Hidden);
        // A pane just moved into A whose view is still parented in B: A hides it until shown.
        assert_eq!(placement(false, true, false), Placed::Hidden);
    }

    #[test]
    fn indicator_strip_follows_iterm2() {
        assert_eq!(strip_width(0.0, 285.0), None);
        assert_eq!(strip_width(0.0, 286.0), Some(24.0));
        assert_eq!(strip_width(0.0, 562.0), Some(90.0));
        assert_eq!(strip_width(0.0, 1000.0), Some(120.0));
        // Two buttons end at the strip's right edge, 4 apart.
        assert_eq!(place(2, (100.0, 220.0)), vec![176.0, 200.0]);
        // 68 points of buttons in a 60-point strip start at the left edge.
        assert_eq!(place(3, (100.0, 160.0)), vec![100.0, 124.0, 148.0]);
        assert!(place(0, (0.0, 10.0)).is_empty());
        // In the toolbar: the field gives way, the strip sits 12 left of the menu, a clipped button is not hit.
        let bar = Rect { x: 0.0, y: 0.0, w: 800.0, h: 32.0 };
        let none = geometry_status(bar, 1.0, false, false, 0);
        let g = geometry_status(bar, 1.0, false, false, 2);
        assert_eq!(g.status_n, 2);
        assert_eq!(g.strip.x + g.strip.w, g.menu.x - 12.0);
        assert!(g.field.w < none.field.w && g.field.x + g.field.w <= g.strip.x);
        let b = g.status[1];
        assert_eq!(hit(&g, b.x + 2.0, b.y + 2.0), Some(Hit::Status(1)));
        let many = geometry_status(bar, 1.0, false, false, 8);
        let last = many.status[7];
        assert!(last.x + last.w > many.strip.x + many.strip.w, "overflow runs past the strip");
        assert_ne!(hit(&many, last.x + last.w - 1.0, last.y + 2.0), Some(Hit::Status(7)), "clipped away");
        // No room: no strip and the field keeps the old width.
        let narrow = geometry_status(Rect { w: 280.0, ..bar }, 1.0, false, false, 2);
        assert_eq!((narrow.status_n, narrow.strip.w), (0, 0.0));
        assert_eq!(narrow.field, geometry_status(Rect { w: 280.0, ..bar }, 1.0, false, false, 0).field);
    }

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
        let m = menu_items(true, true, false, None);
        assert_eq!(m[0], Some(("Remove Bookmark", true)));
        assert_eq!(menu_items(false, false, false, None)[0], Some(("Add Bookmark", false)));
        fn names<'a>(v: &[Option<(&'a str, bool)>]) -> Vec<&'a str> {
            v.iter().flatten().map(|(n, _)| *n).collect()
        }
        assert_eq!(names(&m), vec!["Remove Bookmark", "Manage Bookmarks", "History", "Settings"]);
        assert_eq!(m[1], None);
        assert_eq!(m.last().copied().flatten(), Some(("Settings", true)), "Settings opens the settings page (13x8 Task 5)");
        let dev = menu_items(false, true, true, None);
        assert_eq!(names(&dev), vec!["Settings"], "Dev Null hides the bookmark and history items (`BT:344`)");
        assert_eq!(history_menu(&[("T".into(), "https://a/".into()), ("".into(), "https://b/".into()), ("x\u{202e}y".into(), "u".into())], 80), vec!["T", "https://b/", "xy"]);
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
        assert_eq!(saved_url("https://user:pw@a.test/x").as_deref(), Some("https://a.test/x"), "no credentials in Termurion's files");
        assert_eq!(saved_url("https://a.test/\u{1}"), None);
        assert_eq!(saved_url("https://a.test/\u{202e}x"), None);
        assert_eq!(saved_url("javascript:alert(1)"), None);
        assert_eq!(saved_url("file:///C:/x"), None, "an arrangement never names a local file");
        assert_eq!(saved_url("data:text/html,x"), None);
        assert_eq!(saved_url(&format!("https://a.test/{}", "x".repeat(MAX_URL))), None);
        assert_eq!(saved_url(""), None);
        assert_eq!(saved_url("about:blank").as_deref(), Some("about:blank"));
    }

    /// Final review Minor 8: the URL written to an arrangement or restoration file drops the
    /// fragment (an OAuth `#access_token=` lives there) and keeps `about:` only as `about:blank`
    /// (another party's arrangement names no other `about:` page); loading judges it the same way.
    #[test]
    fn an_arrangement_url_keeps_no_fragment() {
        assert_eq!(arrangement_url("https://a.test/cb?code=1#access_token=x").as_deref(), Some("https://a.test/cb?code=1"));
        assert_eq!(arrangement_url("https://u:p@a.test/x#").as_deref(), Some("https://a.test/x"));
        assert_eq!(arrangement_url("iterm2-about:history").as_deref(), Some("iterm2-about:history"));
        assert_eq!(arrangement_url("about:blank").as_deref(), Some("about:blank"));
        assert_eq!(arrangement_url("ABOUT:BLANK").as_deref(), Some("ABOUT:BLANK"));
        assert_eq!(arrangement_url("about:srcdoc"), None);
        assert_eq!(arrangement_url("about:blank#x").as_deref(), Some("about:blank"));
        assert_eq!(arrangement_url("javascript:alert(1)"), None);
        let a = include_str!("app/arrange_ui.rs");
        assert!(a.contains("crate::browser::arrangement_url(&io.url)") && !a.contains("crate::browser::saved_url(&io.url)"));
        let b = include_str!("app/browser_app.rs");
        assert!(b.contains("browser::arrangement_url(session.get(browser::URL_KEY)?.as_str()?)"));
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
            for (label, _) in menu_items(false, true, dev_null, Some(ReaderMenu { reader: false, distraction: true, has_url: true, ask_ai: true })).into_iter().flatten() {
                assert!(!menu_selector(label).is_empty(), "{label} has its iTerm2 action");
                assert!(!crate::menus::hidden_in_menu(menu_selector(label), false), "{label} shows in release");
            }
        }
        let live = include_str!("browser.rs").split("#[cfg(test)]\nmod tests").next().unwrap();
        let body = &live[live.find("pub fn menu_items(").unwrap()..];
        assert!(body[..body.find("\n}\n").unwrap()].contains("crate::menus::hidden_in_menu(menu_selector(label), cfg!(debug_assertions))"));
    }

    /// Review Focus 1 (`BT:313-345`): the reader block comes first, whatever Dev Null says, and only
    /// when Reader Mode is offered.
    #[test]
    fn reader_menu_follows_iterm2() {
        let rm = |reader, distraction, has_url| Some(ReaderMenu { reader, distraction, has_url, ask_ai: has_url });
        let m = menu_items(false, true, false, rm(false, false, true));
        assert_eq!(m[..5], [Some(("Reader Mode", true)), Some(("Remove Distractions", true)), None, Some(("Ask AI\u{2026}", true)), Some(("Add Bookmark", true))]);
        let on = menu_items(false, true, false, rm(true, true, true));
        assert_eq!((on[0], on[1]), (Some(("Exit Reader Mode", true)), Some(("Exit Distraction Removal", true))));
        let none = menu_items(false, true, false, rm(false, false, false));
        assert_eq!((none[0], none[1], none[3]), (Some(("Reader Mode", false)), Some(("Remove Distractions", false)), Some(("Ask AI\u{2026}", false))), "both wait for a URL");
        assert_eq!(menu_items(false, true, false, None)[0], Some(("Add Bookmark", true)), "None keeps the list as 13x built it");
        let dev = menu_items(false, true, true, rm(false, false, true));
        let names: Vec<&str> = dev.iter().flatten().map(|(n, _)| *n).collect();
        assert_eq!(names, vec!["Reader Mode", "Remove Distractions", "Ask AI\u{2026}", "Settings"], "Dev Null keeps the reader block and drops the bookmark items");
        for label in ["Reader Mode", "Exit Reader Mode", "Remove Distractions", "Exit Distraction Removal", "Ask AI\u{2026}"] {
            assert!(!menu_selector(label).is_empty(), "{label} has its iTerm2 action");
        }
    }

    /// Task 8 (Review Focus 5, `BT:313-341`): with reader content the two reader items, a separator, then
    /// Ask AI enabled; without reader content none of them and no separator; Dev Null keeps it; a
    /// page the tools may not read (`ask_ai` false) shows it disabled.
    #[test]
    fn ask_ai_follows_iterm2() {
        assert_eq!(ASK_AI_ID, "Ask AI\u{2026}");
        let on = ReaderMenu { reader: false, distraction: false, has_url: true, ask_ai: true };
        let m = menu_items(false, true, false, Some(on));
        assert_eq!(m[..4], [Some(("Reader Mode", true)), Some(("Remove Distractions", true)), None, Some((ASK_AI_ID, true))]);
        let none = menu_items(false, true, false, None);
        assert!(none.iter().flatten().all(|(n, _)| !matches!(*n, "Reader Mode" | "Remove Distractions" | ASK_AI_ID)) && none[0] == Some(("Add Bookmark", true)), "no reader content, no block and no separator");
        let dev = menu_items(false, true, true, Some(on));
        assert!(dev.contains(&Some((ASK_AI_ID, true))), "Dev Null does not hide the item (the app disables it through ask_ai)");
        let off = menu_items(false, true, true, Some(ReaderMenu { ask_ai: false, ..on }));
        assert!(off.contains(&Some((ASK_AI_ID, false))));
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

    /// Task 5 review O1 (URL spoofing, a class): no display of a URL ever drops the host. A long user
    /// name is shortened first, then the path; the host stays whole at the end of the authority.
    #[test]
    fn display_url_never_drops_the_host() {
        let user = format!("{}accounts.google.com", "a".repeat(3000));
        for url in [format!("https://{user}@evil.test/login"), format!("https://{user}:{user}@evil.test:8443/x?y={}", "z".repeat(5000)), format!("https://{user}@evil.test")] {
            let shown = display_url(&url);
            assert!(shown.chars().count() <= MAX_URL + 1, "bounded");
            assert!(shown.starts_with("https://"), "{shown}");
            let authority = shown.trim_start_matches("https://");
            let authority = &authority[..authority.find(['/', '?', '#']).unwrap_or(authority.len())];
            assert!(authority.ends_with("@evil.test") || authority.ends_with("@evil.test:8443"), "the host is whole after the @: {authority}");
            assert!(shown.contains('\u{2026}'), "the user information shows it was cut");
        }
        // A long path is cut after the host; a short user name is untouched.
        let shown = display_url(&format!("https://paypal.com@evil.test/{}", "p".repeat(5000)));
        assert!(shown.starts_with("https://paypal.com@evil.test/") && shown.chars().count() == MAX_URL + 1);
        // A host too long to be a host keeps its end.
        let long_host = format!("https://{}.evil.test/", "h".repeat(3000));
        assert!(display_url(&long_host).contains("evil.test/") || display_url(&long_host).contains("evil.test"), "{}", display_url(&long_host));
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
        for q in ["rust lang", "how to cook", "what is 1.5 times 2", "rust", "wikipedia", "r", "ru", "intranet"] {
            assert!(suggest_allowed(q), "{q} is a search (iTerm2 sends single words, BOS:30-38)");
        }
        for q in ["localhost", "LocalHost", "a.b", "10.0.0.1", "intranet:80", "h.test/p?x=1"] {
            assert!(!suggest_allowed(q), "{q} carries a dot, port, path, query, fragment, @, an IP or localhost");
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
        // A long host: whole subdomain labels go from the left, then the scheme and path give way.
        let long = "https://very.long.subdomain.chain.example-bank.test:8443/x";
        let fit = fit_address(long, 24);
        assert_eq!(fit, "\u{2026}example-bank.test:8443", "the scheme and path go before any label does");
        // Something that is not an authority URL is cut from the right.
        assert_eq!(fit_address("about:blank", 6), "about\u{2026}");
    }

    /// Task 7 re-review item 3 and Task 6 round 2 new breakage: the registrable host is the PUBLIC
    /// suffix (a bounded embedded table of the multi-label ones, hosting suffixes included) and one
    /// label, a trailing dot is not a label, and the last-resort cut never splits the attacker's label
    /// from the left.
    #[test]
    fn the_suffix_table_keeps_the_attackers_label() {
        let cases = [
            ("evil.github.io", "github.io"),
            ("acct.blob.core.windows.net", "blob.core.windows.net"),
            ("x.s3.amazonaws.com", "s3.amazonaws.com"),
            ("bucket.s3.us-west-2.amazonaws.com", "s3.us-west-2.amazonaws.com"),
            ("bucket.s3-us-west-2.amazonaws.com", "s3-us-west-2.amazonaws.com"),
            ("evil.co.uk", "co.uk"),
            ("evil.com.au", "com.au"),
            ("evil.co.jp", "co.jp"),
            ("evil.blogspot.com", "blogspot.com"),
            ("evil.web.app", "web.app"),
            ("evil.firebaseapp.com", "firebaseapp.com"),
            ("evil.pages.dev", "pages.dev"),
            ("evil.workers.dev", "workers.dev"),
            ("evil.herokuapp.com", "herokuapp.com"),
            ("evil.vercel.app", "vercel.app"),
            ("evil.netlify.app", "netlify.app"),
            ("evil.appspot.com", "appspot.com"),
            ("evil.googleusercontent.com", "googleusercontent.com"),
            ("evil.githubusercontent.com", "githubusercontent.com"),
            ("evil.azurewebsites.net", "azurewebsites.net"),
            ("evil.cloudfront.net", "cloudfront.net"),
            ("evil.fly.dev", "fly.dev"),
            ("evil.onrender.com", "onrender.com"),
            ("evil.ngrok.io", "ngrok.io"),
            ("evil.ngrok-free.app", "ngrok-free.app"),
            ("evil.uk.com", "uk.com"),
        ];
        for (host, suffix) in cases {
            let attacker = host[..host.len() - suffix.len() - 1].rsplit('.').next().unwrap();
            let url = format!("https://deep.sub.{host}/path/x");
            let reg = format!("{attacker}.{suffix}");
            for cells in 1..=70 {
                let fit = fit_address(&url, cells);
                assert!(fit.chars().count() <= cells, "{host} {cells}: {fit}");
                if cells >= reg.len() {
                    assert!(fit.contains(&reg), "{host} {cells}: the attacker's label and the suffix stay whole (final review I3): {fit}");
                } else {
                    assert_eq!(fit, "\u{2026}", "{host} {cells}: no host character when it does not fit (final residual 1)");
                }
            }
        }
        // A trailing dot is not a label: the attacker's label is still never cut.
        for cells in 19..50 {
            let fit = fit_address("https://accounts.google.com.evil.test./x", cells);
            assert!(fit.contains("evil.test."), "{cells}: {fit}");
        }
        // Last resort: no host character at all, never a label cut (final residual 1).
        assert_eq!(fit_address("https://notpaypal.com/", 11), "\u{2026}");
        assert_eq!(fit_address("https://notpaypal.co.uk/", 12), "\u{2026}");
        assert_eq!(fit_address("https://notpaypal.co.uk/", 15), "notpaypal.co.uk");
        let tiny = fit_address("https://a.b.evil.test/", 6);
        assert_eq!(tiny, "\u{2026}");
        // An IDN homograph shows its punycode, never a look-alike.
        let shown = display_url("https://p\u{0430}ypal.com/");
        assert!(shown.contains("xn--") && !fit_address(&shown, 14).contains('\u{0430}'), "{shown}");
        // The table is bounded, clean, and holds every row the review named.
        assert!(PUBLIC_SUFFIXES.len() <= 500);
        for e in PUBLIC_SUFFIXES {
            assert!(!e.is_empty() && e.is_ascii() && e.to_ascii_lowercase() == *e && !e.starts_with('.') && !e.ends_with('.') && e.contains('.'), "{e}");
        }
        for (_, suffix) in cases {
            assert!(PUBLIC_SUFFIXES.iter().any(|e| *e == suffix) || suffix_labels(&suffix.split('.').collect::<Vec<_>>()) == suffix.split('.').count(), "{suffix}");
        }
    }

    /// Final review I3 (URL spoofing, round 3): the host never loses characters. When it does not fit,
    /// whole labels go from the LEFT behind a leading ellipsis; when even the registrable host does not
    /// fit, the scheme and path go, and narrower still no host character shows (final residual 1).
    /// A label is never cut, so `paypal-verify.com` can never read `paypal\u{2026}.com`.
    /// The same holds in the address field, the suggestion rows (also behind a lead-in) and the tip.
    #[test]
    fn the_host_never_loses_characters() {
        let cases = [
            ("https://paypal-verify.com/login?next=1", "paypal-verify.com"),
            ("https://notpaypal.com/", "notpaypal.com"),
            ("https://a.b.notpaypal.co.uk/x", "notpaypal.co.uk"),
            ("https://my-very-long-project-name-for-a-demo.github.io/index.html", "my-very-long-project-name-for-a-demo.github.io"),
            ("https://deep.sub.evil.github.io/p", "evil.github.io"),
            ("https://user:pw@login.paypal-verify.com:8443/x", "paypal-verify.com"),
        ];
        for (url, reg) in cases {
            let shown = display_url(url);
            for cells in 1..=shown.chars().count() + 2 {
                for (surface, fit) in [
                    ("field", fit_address(&shown, cells)),
                    ("tip", crate::browser_tip::tip_text(url, cells)),
                    ("row", fit_address(&format!("Navigate to \u{201c}{shown}\u{201d}"), cells + 14)),
                ] {
                    assert!(fit.chars().count() <= cells + if surface == "row" { 14 } else { 0 }, "{surface} {url} {cells}: never wider than the width: {fit}");
                    if fit.trim_start_matches("Navigate to \u{201c}").trim_end_matches('\u{201d}').chars().all(|c| c == '\u{2026}') {
                        assert!(cells < reg.chars().count() + 1, "{surface} {url} {cells}: the host shows where it fits: {fit}");
                        continue;
                    }
                    assert!(fit.contains(reg), "{surface} {url} {cells}: the registrable host is whole: {fit}");
                    assert!(!fit.contains("\u{2026}."), "{surface} {url} {cells}: no label is cut from its right: {fit}");
                    let at = fit.find(reg).unwrap();
                    let before = fit[..at].chars().last();
                    assert!(matches!(before, None | Some('\u{2026}' | '.' | '/' | '@' | '\u{201c}')), "{surface} {url} {cells}: the label in front of it is whole: {fit}");
                    if surface == "field" && cells >= reg.chars().count() {
                        assert!(fit.chars().count() <= cells, "{url} {cells}: what can fit, fits: {fit}");
                    }
                }
            }
        }
        assert_eq!(fit_address("https://paypal-verify.com/", 11), "\u{2026}", "never paypal\u{2026}.com, never paypal-veri");
        assert_eq!(fit_address("https://notpaypal.com/", 11), "\u{2026}");
        assert_eq!(fit_address("https://evil.github.io/", 9), "\u{2026}", "the suffix keeps its labels too");
        assert_eq!(fit_address("https://evil.github.io/", 14), "evil.github.io");
        assert_eq!(fit_address("https://a.b.paypal-verify.com/x", 18), "\u{2026}paypal-verify.com", "the scheme goes before a label does");
        assert_eq!(fit_address("https://a.b.paypal-verify.com/x", 20), "\u{2026}b.paypal-verify.com");
        let measure = |t: &str| t.chars().count() as f32 * 7.0;
        let t = crate::browser_tip::fit_measured(&display_url("https://login.paypal-verify.com/x"), 126.0, &measure);
        assert_eq!(t, "\u{2026}paypal-verify.com");
        assert_eq!(crate::browser_tip::fit_measured(&display_url("https://login.paypal-verify.com/x"), 112.0, &measure), "\u{2026}");
    }

    /// Round 2, class fix: ONE shortener for every URL display. The registrable host (public suffix and
    /// one label) always stays whole: subdomain labels go from the LEFT, the path from the right; a
    /// long scheme or a lead-in text cannot push it out.
    #[test]
    fn one_shortener_keeps_the_registrable_host() {
        let chain = vec!["a123456789".to_string(); 25].join(".");
        assert!(chain.len() >= 249);
        let url = format!("https://{chain}.evil.test/login?x=1");
        for cells in [12usize, 14, 20, 30, 60, 100, 200] {
            let fit = fit_address(&display_url(&url), cells);
            assert!(fit.chars().count() <= cells, "{cells}: {fit}");
            assert!(fit.contains("evil.test"), "{cells}: {fit}");
        }
        // Whole labels go from the left, behind an ellipsis; the path is cut first.
        assert_eq!(fit_address("https://a.b.c.evil.test/path/long/long", 25), "https://a.b.c.evil.test/\u{2026}");
        assert_eq!(fit_address("https://a.b.c.evil.test/", 21), "https://\u{2026}c.evil.test/");
        assert_eq!(fit_address("https://a.b.c.evil.test/", 19), "https://\u{2026}evil.test/");
        // A two-letter country suffix with a short second level is one suffix (co.uk).
        assert!(fit_address("https://login.paypal.co.uk/x", 20).contains("paypal.co.uk"));
        // Narrower than the registrable host: no host character (final residual 1).
        let tiny = fit_address("https://a.b.evil.test/", 6);
        assert_eq!(tiny, "\u{2026}");
        // A lead-in and a closing quote (the Navigate row) leave the URL what they leave of the room.
        let row = format!("Navigate to \u{201c}{}\u{201d}", display_url(&url));
        for cells in [30usize, 40, 60, 90] {
            let fit = fit_address(&row, cells);
            assert!(fit.chars().count() <= cells && fit.contains("evil.test") && fit.starts_with("Navigate to") && fit.ends_with('\u{201d}'), "{cells}: {fit}");
        }
        // A scheme of any length cannot push the host out or past MAX_URL.
        let long = format!("{}://{chain}.evil.test/x", "s".repeat(5000));
        let shown = display_url(&long);
        assert!(shown.chars().count() <= MAX_URL + 1 && shown.contains("evil.test"), "{}", shown.len());
        assert!(fit_address(&shown, 40).contains("evil.test"));
        // The one shortener is the only one in the browser's displays.
        let uses = [
            ("browser_app.rs", include_str!("app/browser_app.rs"), "fit_address("),
            ("browser_tip.rs", include_str!("browser_tip.rs"), "fit_address("),
            ("open_quickly.rs", include_str!("open_quickly.rs"), "browser_tip::fit_measured("),
            ("open_quickly_ui.rs", include_str!("open_quickly_ui.rs"), "open_quickly::url_title_fit("),
            ("browser_pages.rs", include_str!("browser_pages.rs"), "browser::fit_address("),
        ];
        for (name, src, call) in uses {
            let live = &src[..src.find("#[cfg(test)]").unwrap_or(src.len())];
            assert!(live.contains(call), "{name}");
            assert!(!live.contains("tail_chars(") && !live.contains("cut_end("), "{name}: no second shortener");
        }
        let app = include_str!("app/browser_app.rs");
        let draw = &app[app.find("let main = Rect { w: r.w * 0.62").unwrap()..];
        assert!(draw[..700].contains("browser::fit_address(&s.text") && draw[..700].contains("browser::fit_address(&s.detail"), "the suggestion rows");
    }

    /// What a fitted URL shows of its host: None when it shows none (only ellipses), else the host
    /// text with a leading ellipsis dropped.
    fn shown_host(fit: &str) -> Option<String> {
        let rest = fit.split_once("://").map_or(fit, |(_, r)| r);
        let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
        let host = authority.rsplit('@').next().unwrap_or("");
        let host = host.rsplit_once(':').filter(|(_, p)| p.bytes().all(|b| b.is_ascii_digit())).map_or(host, |(h, _)| h);
        let host = host.trim_start_matches('\u{2026}');
        (!host.is_empty()).then(|| host.to_string())
    }

    /// Final residual 1 (URL spoofing, round 4, the class): the one shortener takes the width in
    /// cells and NEVER returns more, on every surface (address field, suggestion text and detail,
    /// the Navigate row, the tip by cells and by measured width, Open Quickly, the history menu), so
    /// no surface clips its end afterwards. Host text is shown only as whole labels ending at the
    /// full registrable host; when that does not fit, no host character shows at all.
    #[test]
    fn the_shortener_never_draws_past_its_width() {
        let userinfo = format!("https://{}x@evil.test/x", "paypal.com.".repeat(273));
        assert!(userinfo.len() >= 3000);
        let cases: Vec<(String, &str, &str)> = vec![
            ("https://paypal.com.co/".into(), "paypal.com.co", "paypal.com.co"),
            ("https://paypal-verify.com/login?next=1".into(), "paypal-verify.com", "paypal-verify.com"),
            ("https://notpaypal.com/".into(), "notpaypal.com", "notpaypal.com"),
            ("https://my-very-long-project-name-for-a-demo.github.io/index.html".into(), "my-very-long-project-name-for-a-demo.github.io", "my-very-long-project-name-for-a-demo.github.io"),
            ("https://x.blob.core.windows.net/c/b".into(), "x.blob.core.windows.net", "x.blob.core.windows.net"),
            ("https://accounts.google.com.evil.co.uk/signin".into(), "accounts.google.com.evil.co.uk", "evil.co.uk"),
            (userinfo, "evil.test", "evil.test"),
        ];
        let width = |t: &str| unicode_width::UnicodeWidthStr::width(t);
        let measure = |t: &str| width(t) as f32 * 7.0;
        for (url, host, reg) in &cases {
            let shown = display_url(url);
            for cells in 1..=40usize {
                let lead = "Navigate to \u{201c}";
                let row = fit_address(&format!("{lead}{shown}\u{201d}"), cells);
                let row_url = row.strip_prefix(lead).and_then(|r| r.strip_suffix('\u{201d}')).unwrap_or(&row).to_string();
                let surfaces = [
                    ("field", fit_address(&shown, cells)),
                    ("raw", fit_address(url, cells)),
                    ("tip", crate::browser_tip::tip_text(url, cells)),
                    ("measured tip", crate::browser_tip::fit_measured(&shown, cells as f32 * 7.0, &measure)),
                    ("navigate row", row.clone()),
                    ("history menu", history_menu(&[(String::new(), url.clone())], cells)[0].clone()),
                    ("open quickly", crate::open_quickly::url_title_fit(&shown, &[], cells as f32 * 7.0, &measure).0),
                ];
                for (surface, fit) in surfaces {
                    let url_part = if surface == "navigate row" { row_url.clone() } else { fit.clone() };
                    assert!(width(&fit) <= cells, "{surface} {host} {cells}: never wider than the width: {fit:?}");
                    match shown_host(&url_part) {
                        None => assert!(url_part.chars().all(|c| c == '\u{2026}'), "{surface} {host} {cells}: no host, nothing of it: {fit:?}"),
                        Some(h) => {
                            let whole = h == *reg || h.ends_with(&format!(".{reg}"));
                            let real = host.ends_with(h.as_str()) && (host.len() == h.len() || host[..host.len() - h.len()].ends_with('.'));
                            assert!(whole && real, "{surface} {host} {cells}: host text is whole labels ending at {reg}: {fit:?}");
                        }
                    }
                }
            }
        }
        // The case the round-4 review saw: `paypal.com.co` in a 10-cell field is never `paypal.com`.
        assert_eq!(fit_address("https://paypal.com.co/", 10), "\u{2026}");
        assert_eq!(fit_address("https://paypal.com.co/", 13), "paypal.com.co");
        assert_eq!(fit_address("https://paypal.com.co/", 14), "\u{2026}paypal.com.co");
        assert_eq!(fit_address("https://paypal-verify.com/", 11), "\u{2026}");
        assert_eq!(fit_address("https://evil.github.io/", 9), "\u{2026}");
        // A cut path keeps its separator: never an ellipsis right after the host (it reads as a cut host).
        assert_eq!(fit_address("https://paypal.com/login", 19), "https://paypal.com");
        assert_eq!(fit_address("https://paypal.com/login", 20), "https://paypal.com/\u{2026}");
        // Each surface passes the width it draws in: the history menu its menu cells, the palette
        // its title width for a URL title, the field and rows their exact cells (no minimum of one).
        let app = include_str!("app/browser_app.rs");
        assert!(app.contains("browser::history_menu(&pairs, HISTORY_MENU_CELLS)"));
        assert_eq!(app.matches(").floor().max(0.0) as usize)").count(), 3, "the field and both row columns");
        assert!(include_str!("open_quickly_ui.rs").contains("if row.title_url {") && include_str!("app/open_quickly_app.rs").contains("item.title == crate::browser::display_url(u)"));
        // Wide characters count their cells.
        assert_eq!(fit_address("https://a.test/\u{4e2d}\u{4e2d}\u{4e2d}", 20), "https://a.test/\u{4e2d}\u{4e2d}\u{2026}");
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
        // Only Termurion's own page is Termurion's own; every other scheme shows its creator's state.
        assert_eq!(security("blob:https://evil.test/1234", false), Security::Secure, "a blob has its creator's origin");
        assert_eq!(security("blob:http://evil.test/1234", false), Security::NotSecure);
        assert_eq!(security("blob:null/1234", false), Security::NotSecure);
        assert_eq!(security("filesystem:https://a.test/temporary/x", false), Security::Secure);
        assert_eq!(security("data:text/html,<b>x</b>", false), Security::NotSecure);
        assert_eq!(security("about:srcdoc", false), Security::NotSecure);
        assert_eq!(security("javascript:alert(1)", false), Security::NotSecure);
        assert_eq!(security("ms-msdt:id", false), Security::NotSecure);
        assert_eq!(security("ITERM2-ABOUT:history", false), Security::Local);
        assert_eq!(security("blob:iterm2-about:history/1", false), Security::NotSecure, "a blob never claims Termurion's page");
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
        assert_eq!((n.target_url(-1), n.target_url(-2), n.target_url(-3), n.target_url(1), n.target_url(0)), (Some("https://b/"), Some("https://a/"), None, None, Some("https://c/")));
        assert_eq!(n.target_url(i32::MIN), None);
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

    // --- Opening links (Task 9) ---

    const SYS: Option<i64> = Some(0);
    const LOCAL: Option<i64> = Some(1);

    #[test]
    fn open_link_decision_follows_iterm2() {
        use Style::*;
        let d = |url: &str, style, upsell, allowed, runtime, open, up| open_decision(url, style, upsell, allowed, runtime, open, up);
        // Non-web schemes open with the system (`WS:190-220`).
        assert_eq!(d("mailto:a@b", Tab, true, true, true, None, None), Open::System);
        assert_eq!(d("ftp://a/b", Tab, true, true, true, None, None), Open::System);
        assert_eq!(d("ssh://a", Tab, true, true, true, None, None), Open::System);
        // Not allowed (BrowserProfiles off or the runtime refused): the system, no nagging.
        assert_eq!(d("https://a", Tab, true, false, true, None, None), Open::System);
        // The runtime is missing and the browser is on: the upsell, unless Use System Browser was remembered.
        assert_eq!(d("https://a", Tab, true, false, false, None, None), Open::Upsell);
        assert_eq!(d("https://a", Tab, true, false, false, None, Some(1)), Open::System);
        assert_eq!(d("https://a", Tab, true, false, false, None, Some(0)), Open::Upsell, "Download is never remembered");
        // `upsell` false with the browser not allowed is the system (`WS:249-251`).
        assert_eq!(d("https://a", Tab, false, false, false, None, None), Open::System);
        // Allowed: ask, or the remembered answer.
        assert_eq!(d("https://a", Tab, true, true, true, None, None), Open::AskOpenInApp { file: false });
        assert_eq!(d("https://a", Window, true, true, true, None, None), Open::AskOpenInApp { file: false });
        assert_eq!(d("https://a", Tab, true, true, true, LOCAL, None), Open::Local(Tab));
        assert_eq!(d("https://a", Window, true, true, true, LOCAL, None), Open::Local(Window));
        assert_eq!(d("https://a", Tab, true, true, true, SYS, None), Open::System);
        assert_eq!(d("https://a", Tab, true, true, true, Some(2), None), Open::AskOpenInApp { file: false }, "Cancel is never a remembered answer");
        // A split is implied consent (`WS:414-418`), whatever was remembered.
        assert_eq!(d("https://a", Vertical, true, true, true, None, None), Open::Local(Vertical));
        assert_eq!(d("https://a", Horizontal, true, true, true, SYS, None), Open::Local(Horizontal));
        // A file with a compatible extension asks about a file; a program is never opened here.
        assert_eq!(d("file:///c:/a.pdf", Tab, true, true, true, None, None), Open::AskOpenInApp { file: true });
        assert_eq!(d("file:///c:/a.pdf", Tab, true, true, true, LOCAL, None), Open::Local(Tab));
        assert_eq!(d("file:///c:/a.exe", Tab, true, true, true, LOCAL, None), Open::System);
        assert_eq!(d("file:///c:/a", Tab, true, true, true, None, None), Open::System);
        assert_eq!(d("file:///c:/a.pdf", Tab, false, false, true, None, None), Open::System);
    }

    #[test]
    fn compatible_files_never_run() {
        for ext in ["html", "HTM", "xhtml", "xml", "svg", "css", "pdf", "png", "JPG", "jpeg", "gif", "webp", "heic"] {
            assert!(compatible_file(ext), "{ext}");
            assert!(compatible_file(&format!(".{ext}")), ".{ext}");
        }
        for ext in ["exe", "lnk", "ps1", "bat", "cmd", "hta", "js", "vbs", "msi", "scr", "com", "txt", "", "pdf:stream", "html:zone", "pdf ", "..pdf"] {
            assert!(!compatible_file(ext), "{ext:?}");
        }
    }

    #[test]
    fn upsell_follows_iterm2() {
        assert_eq!(UPSELL_HEADING, "WebView2 Runtime Required");
        assert_eq!(UPSELL_TEXT, "Termurion can display web pages! But first you must install the WebView2 Runtime.");
        assert_eq!((OPEN_HEADING, OPEN_IN_TERMURION, USE_DEFAULT_BROWSER, USE_DEFAULT_APP), ("Open in Termurion?", "Open in Termurion", "Use Default Browser", "Use Default App"));
        assert_eq!(OPEN_WEB_TEXT, "Termurion can display web pages! Would you like to open this link in Termurion?");
        assert_eq!(OPEN_FILE_TEXT, "Termurion can display files like this in its built-in web browser! Would you like to open this link in Termurion?");
        assert_eq!((DOWNLOAD, USE_SYSTEM_BROWSER), ("Download", "Use System Browser"));
        assert_eq!((OPEN_LINKS_ID, OPEN_FILE_LINKS_ID, UPSELL_ID), ("NoSyncOpenLinksInApp", "NoSyncOpenLinksInAppForFileURL", "NoSyncBrowserUpsell"));
        // Microsoft's own page and nothing else: https, the developer.microsoft.com host.
        assert_eq!(PLUGIN_URL, "https://developer.microsoft.com/microsoft-edge/webview2/");
        assert_eq!(crate::toolbelt::url_host(PLUGIN_URL).as_deref(), Some("developer.microsoft.com"));
        let u = upsell_spec();
        assert_eq!((u.id.as_deref(), u.kind, u.heading.as_str(), u.text.as_str()), (Some("NoSyncBrowserUpsell"), crate::warnings::Kind::Permanent, UPSELL_HEADING, UPSELL_TEXT));
        assert_eq!(u.labels, vec!["Download", "Use System Browser", "Cancel"]);
        assert_eq!(u.cancel, Some(2));
        let remembered: Vec<usize> = (0..3).filter(|i| u.rememberable(*i)).collect();
        assert_eq!(remembered, vec![1], "only Use System Browser is remembered (`GW:136-160`)");
        let o = open_link_spec(false);
        assert_eq!((o.id.as_deref(), o.kind, o.heading.as_str(), o.text.as_str()), (Some("NoSyncOpenLinksInApp"), crate::warnings::Kind::Permanent, "Open in Termurion?", OPEN_WEB_TEXT));
        assert_eq!(o.labels, vec!["Use Default Browser", "Open in Termurion", "Cancel"]);
        assert_eq!(o.cancel, Some(2));
        assert!(o.rememberable(0) && o.rememberable(1) && !o.rememberable(2));
        let f = open_link_spec(true);
        assert_eq!((f.id.as_deref(), f.text.as_str()), (Some("NoSyncOpenLinksInAppForFileURL"), OPEN_FILE_TEXT));
        assert_eq!(f.labels, vec!["Use Default App", "Open in Termurion", "Cancel"]);
    }

    #[test]
    fn search_reads_the_whole_selection() {
        // A long selection, a multi-line one and one with controls all search; none is dropped.
        let long = "word ".repeat(100);
        let q = search_query(&long).unwrap();
        assert_eq!(q.chars().count(), long.trim_end().chars().count());
        assert_eq!(search_query("  two\nlines\there \u{7}x  ").as_deref(), Some("two lines here x"));
        let huge = "a".repeat(MAX_SEARCH * 3);
        assert_eq!(search_query(&huge).unwrap().chars().count(), MAX_SEARCH);
        assert_eq!(search_query(" \n\t "), None);
        assert_eq!(search_query(""), None);
        // The URL is escaped: nothing in the text breaks out of the query.
        let url = search_url("https://x.test/?q=%@", &search_query("a&b=c d#e").unwrap());
        assert!(url.starts_with("https://x.test/?q=") && !url.contains(' ') && !url.contains('#'), "{url}");
        // The menu keeps the whole selection for the item, apart from the short one.
        let src = include_str!("app/ctx_menu_app.rs");
        let arm = &src[src.find("item == \"searchweb\"").unwrap()..];
        let arm = &arm[..arm.find("item == \"copylink\"").unwrap()];
        assert!(arm.contains("pend.search") && !arm.contains("pend.selection"), "{arm}");
    }

    #[test]
    fn web_context_items_follow_iterm2() {
        let split = ["Open Selection as URL", "Open URL in Vertical Split Pane", "Open URL in Horizontal Split Pane"];
        let mut all = split.to_vec();
        all.extend(["Quick Look Link", "Search the Web for Selection"]);
        assert_eq!(web_context_items(Some("https://a.b"), true, true), all);
        assert_eq!(web_context_items(Some("https://a.b"), false, true), vec!["Open Selection as URL", "Quick Look Link", "Search the Web for Selection"]);
        assert_eq!(web_context_items(Some("https://a.b"), true, false), vec![split[0], split[1], split[2], "Search the Web for Selection"]);
        assert_eq!(web_context_items(Some("a b"), false, false), vec!["Open Selection as URL", "Search the Web for Selection"]);
        assert_eq!(web_context_items(None, true, true), vec!["Search the Web for Selection"], "Search is added unconditionally (`CMH:560`)");
    }

    #[test]
    fn web_link_is_the_one_gate() {
        assert_eq!(web_link("https://a.test/x?y=1").as_deref(), Some("https://a.test/x?y=1"));
        assert_eq!(web_link("http://u:p@a.test/x").as_deref(), Some("http://a.test/x"), "credentials are dropped");
        for bad in ["javascript:alert(1)", "data:text/html,x", "file:///c:/a.html", "ftp://a/b", "mailto:a@b", "iterm2-about:history", "https://a b", "https://", "https://a.test/\u{202e}x", "https://a.test/\nx", ""] {
            assert_eq!(web_link(bad), None, "{bad:?}");
        }
        assert_eq!(web_link(&format!("https://a.test/{}", "x".repeat(MAX_URL))), None, "too long");
    }

    #[test]
    fn conditional_openability_follows_the_saved_answers() {
        assert!(conditionally_openable("https://a", true, true, None, None));
        assert!(conditionally_openable("https://a", true, true, LOCAL, None));
        assert!(!conditionally_openable("https://a", true, true, SYS, None), "Use Default Browser was remembered");
        assert!(conditionally_openable("https://a", false, false, None, None), "the upsell would show");
        assert!(!conditionally_openable("https://a", false, false, None, Some(1)), "Use System Browser was remembered");
        assert!(!conditionally_openable("https://a", false, true, None, None), "the browser is off");
        assert!(!conditionally_openable("mailto:a@b", true, true, None, None));
    }
}
