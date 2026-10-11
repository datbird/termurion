//! The `iterm2-about:` pages (milestone 13x, Task 7): iTerm2's welcome, history and bookmarks pages
//! served from the binary and the closed set of messages they may send back. Everything here is
//! pure: the app feeds it a request or a message and acts on the answer. A page is Tandem's HTML
//! but its DATA (titles, URLs) is a web page's, so the data is escaped by the pages' scripts and
//! the host never trusts what a page says it is (B3, P8, P9): a message counts only from an
//! `iterm2-about:` source (never because of its JSON), only from the page whose handler it names,
//! with typed and bounded arguments from a closed set, and a navigation only to a URL already in
//! the database that the browser loads from here (http, https, `about:blank`).

use crate::browser;
use crate::browser_db::Db;
use crate::webhost::{scheme_of, Answer};
use serde_json::{json, Map, Value};

/// The policy every local page response carries (B3b, tightened by the Task 7 review ruling): no
/// remote script, frame, fetch or form. Script runs only with this response's nonce (no
/// `'unsafe-inline'`, no `'unsafe-eval'`), so a missed escape in a page cannot run script with the
/// bridge; inline STYLE stays; images may come from the web (favicons) and `data:`.
pub fn csp(nonce: &str) -> String {
    format!("default-src 'none'; script-src 'nonce-{nonce}'; style-src 'unsafe-inline'; img-src data: http: https:; connect-src 'none'; form-action 'none'; base-uri 'none'; frame-ancestors 'none'")
}

/// A fresh nonce for one response: a v4 UUID's 122 random bits, hex.
pub fn new_nonce() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

// --- Local pages' scheme names, templates, glyphs and the static pages (13x8 Task 1) ---

/// The scheme of Tandem's own pages (`LPM:58-64`), and of the file and ssh pages.
pub const ABOUT: &str = "iterm2-about";
pub const SSH_SCHEME: &str = "iterm2-ssh";
pub const FILE_SCHEME: &str = "iterm2-file";

/// The empty body of a refusal or a missing page (iTerm2 answers a 404 with no text).
pub const NOT_FOUND_HTML: &str = "";

/// The glyph spans' rule, appended to the common CSS wherever a page gets it (inlined by `page`,
/// or served as `iterm-browser-common.css`).
pub const GLYPH_CSS: &str = "\n.tandem-glyph { font-family: \"Segoe Fluent Icons\"; font-style: normal; }\n";

/// iTerm2's emoji and the Segoe Fluent Icons glyph each becomes (the m12 "UI glyphs" ruling). A key
/// ends in U+FE0F where iTerm2's text does; one without it takes a following U+FE0F with it. The
/// check marks (U+2713), the trigram (U+2630) and a bare U+26A0 stay text.
pub const GLYPHS: &[(&str, char)] = &[
    ("\u{1F512}", '\u{E72E}'),
    ("\u{1F4E6}", '\u{E7B8}'),
    ("\u{1F511}", '\u{E8D7}'),
    ("\u{1F310}", '\u{E774}'),
    ("\u{1F4C1}", '\u{E8B7}'),
    ("\u{1F4C4}", '\u{E8A5}'),
    ("\u{2B06}\u{FE0F}", '\u{E74A}'),
    ("\u{1F464}", '\u{E77B}'),
    ("\u{1F6E1}\u{FE0F}", '\u{EA18}'),
    ("\u{1F3AC}", '\u{E714}'),
    ("\u{1F680}", '\u{E768}'),
    ("\u{2728}", '\u{E734}'),
    ("\u{26A0}\u{FE0F}", '\u{E7BA}'),
];

const VARIATION_SELECTOR: char = '\u{FE0F}';

/// The glyph entry whose emoji starts `at`, with the bytes it and its selector take.
fn glyph_at(at: &str) -> Option<(char, usize)> {
    GLYPHS.iter().find_map(|(emoji, glyph)| {
        let rest = at.strip_prefix(emoji)?;
        let selector = if emoji.ends_with(VARIATION_SELECTOR) { 0 } else { rest.starts_with(VARIATION_SELECTOR) as usize * VARIATION_SELECTOR.len_utf8() };
        Some((*glyph, emoji.len() + selector))
    })
}

/// Whether `before` ends in a CSS `content: "` and `after` goes on with `"` and `;`: the emoji
/// between them is a pseudo-element's content, not text.
fn css_content(before: &str, after: &str) -> bool {
    let Some(head) = before.strip_suffix('"') else { return false };
    let head = head.trim_end();
    let Some(head) = head.strip_suffix(':') else { return false };
    head.trim_end().ends_with("content") && after.strip_prefix('"').is_some_and(|a| a.trim_start().starts_with(';'))
}

/// iTerm2's emoji in a template become Segoe Fluent Icons glyphs, scanned once: a CSS
/// `content: "<emoji>";` keeps the glyph as the content and gains the icon font, any other emoji
/// becomes a `tandem-glyph` span. Only template and included text goes through here, never a
/// substituted value.
pub fn glyph_page(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut i = 0;
    while i < html.len() {
        let rest = &html[i..];
        let ch = rest.chars().next().unwrap_or('\0');
        // Every emoji of the table starts with a non-ASCII character; skip the rest cheaply.
        match Some(ch).filter(|c| !c.is_ascii()).and_then(|_| glyph_at(rest)) {
            Some((glyph, taken)) => {
                let after = &html[i + taken..];
                if css_content(&html[..i], after) {
                    // `"` + optional spaces + `;` follow: keep them, add the font after the `;`.
                    let semi = after.find(';').map_or(0, |n| n + 1);
                    out.push(glyph);
                    out.push_str(&after[..semi]);
                    out.push_str(" font-family: \"Segoe Fluent Icons\";");
                    i += taken + semi;
                } else {
                    out.push_str("<span class=\"tandem-glyph\">");
                    out.push(glyph);
                    out.push_str("</span>");
                    i += taken;
                }
            }
            _ => {
                out.push(ch);
                i += ch.len_utf8();
            }
        }
    }
    out
}

/// The one template loader (ruling Q2): the common CSS (with the glyph rule) and every
/// `{{INCLUDE:...}}` first, then the glyph step over template and included text only, then the
/// `{{KEY}}` substitutions, so a substituted value is never rewritten (`TL:29-74`; review I9).
pub fn page(template: &str, subs: &[(&str, &str)]) -> String {
    use crate::browser_js::{fill_template, resource};
    let css = format!("{}{GLYPH_CSS}", resource("iterm-browser-common.css").unwrap_or_default());
    let with_css = template.replace("{{COMMON_CSS}}", &css);
    fill_template(&glyph_page(&fill_template(&with_css, &[])), subs)
}

/// The copied intro page as served: iTerm2's file with Tandem's visible names, each target found
/// exactly once or no page is served (R13x3-1/-2 pattern). (name, iTerm2's text, Tandem's text)
pub const INTRO_PATCHES: [(&str, &str, &str); 4] = [
    ("TITLE", "<title>Welcome to iTerm2 Browser</title>", "<title>Welcome to Tandem Browser</title>"),
    ("HEADING", "Meet the iTerm2 Browser", "Meet the Tandem Browser"),
    ("PROFILES", "In iTerm2, profiles define", "In Tandem, profiles define"),
    ("EXPERIENCE", "same iTerm2 experience", "same Tandem experience"),
];

pub const FEATURES_PATCHES: [(&str, &str, &str); 5] = [
    ("TITLE", "<title>iTerm2 Browser - Features</title>", "<title>Tandem Browser - Features</title>"),
    ("SUPERPOWERS", "<strong>iTerm2 Superpowers:</strong>", "<strong>Tandem Superpowers:</strong>"),
    ("PRODUCTIVITY", "bring iTerm2 features into the browser", "bring Tandem features into the browser"),
    ("PASSWORDS", "Integrates with the iTerm2 password manager, Webkit adblock lists, and iTerm2\u{2019}s AI chat feature.", "Integrates with the Tandem password manager, Webkit adblock lists, and Tandem\u{2019}s AI chat feature."),
    ("SECONDARY", "iTerm2 doesn\u{2019}t expect", "Tandem doesn\u{2019}t expect"),
];

/// The template of a static `iterm2-about:` page, patched, or None (iTerm2's debug-only pages
/// answer None too: Tandem has no debug build of the browser, `SPH:170-187`).
pub fn static_page(url: &str) -> Option<String> {
    use crate::view_source::apply_patches;
    let (file, patches): (&str, Vec<_>) = match page_name(url)? {
        "onboarding-intro" => ("onboarding-intro.html", INTRO_PATCHES.to_vec()),
        "onboarding-features" => ("onboarding-features.html", FEATURES_PATCHES.to_vec()),
        _ => return None,
    };
    match apply_patches(crate::browser_js::resource(file)?, &patches) {
        Ok(html) => Some(page(&html, &[])),
        Err(e) => {
            crate::rlog!("{}", crate::dlog::name(&e));
            None
        }
    }
}

// --- The error page (13x8 Task 2; EH, EP, BM) ---

/// The error page's URL (`EH:15`). Its content is never read from this URL: it is Tandem's state
/// for the pane that asked (`BrowserView.pending_error`), served by the app (`browser_error_answer`),
/// so a web page that navigates here shows only the generic page.
pub const ERROR_URL: &str = "iterm2-about:error";

/// Why a load failed, in iTerm2's cases (`EH:89-133`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    Offline,
    HostNotFound,
    TimedOut,
    CannotConnect,
    ConnectionLost,
    TooManyRedirects,
    Certificate(i32),
    Cancelled,
    Other(i32),
}

/// `COREWEBVIEW2_WEB_ERROR_STATUS` to iTerm2's case.
pub fn failure_of(status: i32) -> Failure {
    match status {
        1..=5 => Failure::Certificate(status),
        6 | 12 => Failure::CannotConnect,
        7 => Failure::TimedOut,
        9 | 10 => Failure::ConnectionLost,
        11 => Failure::Offline,
        13 => Failure::HostNotFound,
        14 => Failure::Cancelled,
        15 => Failure::TooManyRedirects,
        other => Failure::Other(other),
    }
}

/// Tandem's stand-in for Apple's `localizedDescription` (no Windows source for it).
pub fn describe(status: i32) -> &'static str {
    match status {
        1 => "The certificate's common name does not match the host name.",
        2 => "The certificate has expired.",
        3 => "The client certificate contains errors.",
        4 => "The certificate has been revoked.",
        5 => "The certificate is invalid.",
        6 => "The server is unreachable.",
        7 => "The connection timed out.",
        8 => "The server returned an invalid response.",
        9 => "The connection was aborted.",
        10 => "The connection was reset.",
        11 => "The internet connection was lost.",
        12 => "Could not connect to the server.",
        13 => "The host name could not be resolved.",
        14 => "The operation was canceled.",
        15 => "The redirect failed.",
        16 => "An unexpected error occurred.",
        17 => "Authentication credentials are required.",
        18 => "Proxy authentication is required.",
        _ => "An unknown error occurred.",
    }
}

/// The message HTML of a case: `<strong>` text `</strong><br/><br/>` detail (`EH:131-132`). The
/// text is Tandem's own; the detail is escaped (it may be a caller's description).
fn message_html(text: &str, detail: &str) -> String {
    format!("<strong>{text}</strong><br/><br/>{}", escape_slot(detail))
}

/// A value for an HTML slot: escaped, and `{` as `&#123;` so no later fill (the nonce) can match in it.
fn escape_slot(s: &str) -> String {
    crate::view_source::escape_html(s).replace('{', "&#123;")
}

/// Whether a failed navigation still got a server's answer (an HTTP status, `http` of
/// `Event::Navigated`): its own page is shown, never Tandem's error page (Task 2 review I1;
/// iTerm2's `didFailProvisionalNavigation` never fires for an HTTP error response).
pub fn server_answered(http: i32) -> bool {
    http >= 100
}

/// The title and message HTML of a failure, None for a cancelled load (`BM:781-800`).
pub fn error_texts(f: Failure) -> Option<(&'static str, String)> {
    Some(match f {
        Failure::Offline => ("No Internet Connection", message_html("Your computer appears to be offline. Check your internet connection and try again.", describe(11))),
        Failure::HostNotFound => ("Server Not Found", message_html("Tandem can\u{2019}t find the server. Check that the web address is correct and try again.", describe(13))),
        Failure::TimedOut => ("The Connection Timed Out", message_html("The server didn\u{2019}t respond in time. The site may be temporarily unavailable or overloaded.", describe(7))),
        Failure::CannotConnect => ("Can\u{2019}t Connect to Server", message_html("Tandem can\u{2019}t establish a secure connection to the server. The server may be down or unreachable.", describe(12))),
        Failure::ConnectionLost => ("Network Connection Lost", message_html("The network connection was lost. Check your internet connection and try again.", describe(10))),
        Failure::TooManyRedirects => ("Too Many Redirects", message_html("Tandem can\u{2019}t open the page because the server redirected too many times.", describe(15))),
        Failure::Certificate(n) => {
            let detail = match n {
                1 => "The hostname does not match the certificate.",
                2 => "The certificate is expired.",
                5 => "The presented chain is not valid (e.g., self-signed without trust).",
                other => describe(other),
            };
            ("Secure Connection Failed", message_html("Tandem can\u{2019}t verify the identity of the website. The connection may not be secure.", detail))
        }
        Failure::Cancelled => return None,
        Failure::Other(n) => described(describe(n)),
    })
}

/// The default case for an error whose description is `description` (`EH:127-133`).
fn described(description: &str) -> (&'static str, String) {
    let text = format!("An error occurred while loading this page. {}", escape_slot(description));
    ("Page Can\u{2019}t Be Loaded", message_html(&text, description))
}

/// The longest URL shown on the page, and the longest one a retry is given (a URL the host cut at
/// 8192 bytes is never navigated, so the retry target stops short of it).
pub const SHOWN_URL_MAX: usize = 512;
pub const RETRY_URL_MAX: usize = 8000;

/// A URL as the error page shows it: hidden characters removed, at most `SHOWN_URL_MAX`
/// characters, and the END of the host kept (`inert_files::display_text`'s removal set): a long
/// user-info or host part shows its tail, a long path loses its tail, so what the person reads
/// still ends where the real host ends.
pub fn shown_url(url: &str) -> String {
    let clean = crate::inert_files::display_text(url, usize::MAX, false);
    let chars: Vec<char> = clean.chars().collect();
    if chars.len() <= SHOWN_URL_MAX {
        return clean;
    }
    let scheme_end = clean.find("://").map_or(0, |n| clean[..n].chars().count() + 3);
    let auth_end = chars[scheme_end..].iter().position(|c| matches!(c, '/' | '?' | '#')).map_or(chars.len(), |n| scheme_end + n);
    let room = SHOWN_URL_MAX - 1;
    let (head, tail_from) = if auth_end <= room {
        (&chars[..auth_end], None)
    } else {
        // The authority alone is over the cap: its start gives way, its end stays.
        (&chars[..0], Some(auth_end - room))
    };
    let mut out = String::new();
    match tail_from {
        None => {
            out.extend(head);
            out.extend(&chars[auth_end..auth_end + (room - head.len())]);
            out.push('\u{2026}');
        }
        Some(from) => {
            out.push('\u{2026}');
            out.extend(&chars[from..auth_end]);
        }
    }
    out
}

/// `s` inside a single-quoted JavaScript string in a `<script>` element: backslash, quotes, `<`,
/// `>`, `&`, `{`, line and control characters escaped as `\uXXXX`.
fn js_single_quoted(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if matches!(c, '\\' | '\'' | '"' | '<' | '>' | '&' | '{' | '}' | '`' | '\u{2028}' | '\u{2029}') || c.is_control() {
            let mut buf = [0u16; 2];
            for unit in c.encode_utf16(&mut buf) {
                out.push_str(&format!("\\u{unit:04x}"));
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Whether the retry button may take the person to `url`: a scheme Tandem loads itself.
fn retry_target(url: &str) -> bool {
    url.len() <= RETRY_URL_MAX && matches!(crate::webhost::scheme_of(url).as_str(), "http" | "https" | "file" | "iterm2-ssh")
}

/// The copied template's load-time patches (R13x3-1/-2 pattern; each target exactly once, else no
/// page): the script carries the response's nonce, the inline `onclick` becomes a listener that
/// counts only a click the person made (`isTrusted`), and so does the key handler. (name, iTerm2's
/// text, Tandem's text)
pub const ERROR_PATCHES: [(&str, &str, &str); 4] = [
    ("SCRIPT", "    <script>\n        function retryLoad() {", "    <script nonce=\"{{NONCE}}\">\n        function retryLoad() {"),
    ("BUTTON", "<button class=\"button retry-button\" onclick=\"retryLoad()\">", "<button class=\"button retry-button\" id=\"tandem-retry\">"),
    ("LISTENER", "        // Add keyboard support\n", "        document.getElementById('tandem-retry').addEventListener('click', function(event) {\n            if (event.isTrusted) { retryLoad(); }\n        });\n\n        // Add keyboard support\n"),
    ("KEYS", "if (event.key === 'Enter' || event.key === ' ') {", "if (event.isTrusted && (event.key === 'Enter' || event.key === ' ')) {"),
];

/// The template filled (`EH:73-87`). The title and message are Tandem's text; the URL is escaped
/// for its HTML slot and for its script slot, `{` is escaped in both so no `{{` of a URL can be
/// read as a slot, and the nonce is left as `{{NONCE}}` for `with_nonce`. An empty string when a
/// patch missed (no page is served).
fn error_html(title: &str, message: &str, url: Option<&str>) -> String {
    let template = match crate::view_source::apply_patches(crate::browser_js::resource("error-page.html").unwrap_or_default(), &ERROR_PATCHES) {
        Ok(t) => t,
        Err(e) => {
            crate::rlog!("{}", crate::dlog::name(&e));
            return String::new();
        }
    };
    let url = url.filter(|u| !u.is_empty());
    let shown = url.map(|u| format!("<div class=\"error-url\">{}</div>", escape_slot(&shown_url(u)))).unwrap_or_default();
    let original = url.filter(|u| retry_target(u)).map(js_single_quoted).unwrap_or_default();
    page(&template, &[("TITLE", title), ("MESSAGE", message), ("URL_DISPLAY", &shown), ("ORIGINAL_URL", &original)])
}

/// The page for a failure and the URL that failed (`EH:73-87`); a cancelled load has no page, so
/// the generic one.
pub fn error_page(f: Failure, failed_url: Option<&str>) -> String {
    match error_texts(f) {
        Some((title, message)) => error_html(title, &message, failed_url),
        None => default_error_page(),
    }
}

/// `iterm2-about:error` with nothing pending (`EH:52-58`): "Page Not Available" as
/// `NSURLErrorResourceUnavailable`.
pub fn default_error_page() -> String {
    error_html("Page Unavailable", &message_html("The requested page is currently unavailable. Try again later.", "Page Not Available"), None)
}

/// The default case for an error described by `description` (`EH:127-133`): "Invalid file path"
/// and a failed `iterm2-ssh:` transfer. The description is escaped and cut.
pub fn described_error_page(description: &str, failed_url: Option<&str>) -> String {
    let description = crate::inert_files::display_text(description, 512, false);
    let (title, message) = described(&description);
    error_html(title, &message, failed_url)
}

/// The served page: this response's nonce in the script (the only place `{{NONCE}}` can be).
pub fn with_nonce(html: &str, nonce: &str) -> String {
    html.replace("{{NONCE}}", nonce)
}

/// Whether to show the page for a failed URL: not the one that failed last (`BM:1348-1352`).
pub fn should_show_error(failed: Option<&str>, last_failed: Option<&str>) -> bool {
    failed.is_some() && failed != last_failed
}

/// The pane's cap on error pages (preflight P6): the loop guard keys on the URL, so a page that
/// redirects to a fresh failing URL each time would load a page for each; at most `LIMIT` in `WINDOW_MS`.
#[derive(Debug, Default)]
pub struct ErrorBurst {
    shown: std::collections::VecDeque<u64>,
}

impl ErrorBurst {
    pub const LIMIT: usize = 5;
    pub const WINDOW_MS: u64 = 10_000;

    /// Whether one more error page may be shown at `now_ms` (counted when it may).
    pub fn allow(&mut self, now_ms: u64) -> bool {
        while self.shown.front().is_some_and(|t| now_ms.saturating_sub(*t) >= Self::WINDOW_MS) {
            self.shown.pop_front();
        }
        if self.shown.len() >= Self::LIMIT {
            return false;
        }
        self.shown.push_back(now_ms);
        true
    }
}

// --- File and ssh pages: what a page of those schemes may load (R13x8-1) ---

/// The policy of every `iterm2-file:` and `iterm2-ssh:` response: sandboxed with no
/// `allow-same-origin` (an opaque origin), so a page cannot read another file or reach any host;
/// scripts may run for the page's own sake only.
pub const SANDBOX_CSP: &str = "sandbox allow-scripts; default-src 'none'; script-src 'unsafe-inline' iterm2-file: iterm2-ssh:; style-src 'unsafe-inline' iterm2-file: iterm2-ssh:; img-src iterm2-file: iterm2-ssh: data:; media-src iterm2-file: iterm2-ssh:; font-src iterm2-file: iterm2-ssh:; connect-src 'none'; form-action 'none'; frame-src 'none'; object-src 'none'; base-uri 'none'";

pub fn sandbox_headers() -> Vec<(String, String)> {
    vec![("Content-Security-Policy".to_string(), SANDBOX_CSP.to_string())]
}

/// `COREWEBVIEW2_WEB_RESOURCE_CONTEXT` values this module judges.
pub const CONTEXT_SCRIPT: i32 = 6;
pub const CONTEXT_XML_HTTP_REQUEST: i32 = 7;
pub const CONTEXT_FETCH: i32 = 8;
pub const CONTEXT_EVENT_SOURCE: i32 = 10;
pub const CONTEXT_WEBSOCKET: i32 = 11;

/// The tree a file or ssh page may load from: its scheme, its host (ssh) and its folder, decoded,
/// `/`-separated and ending in `/` (a listing's own folder, a file's parent).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageRoot {
    pub scheme: &'static str,
    pub host: Option<String>,
    pub folder: String,
}

/// A local-scheme URL taken apart: (scheme, authority, decoded path). None for another scheme, a
/// path that does not decode to UTF-8, or one holding `\`, a control character or a `..` segment.
fn local_parts(url: &str) -> Option<(&'static str, String, String)> {
    let (scheme, rest) = url.split_once(':')?;
    let scheme = if scheme.eq_ignore_ascii_case(FILE_SCHEME) {
        FILE_SCHEME
    } else if scheme.eq_ignore_ascii_case(SSH_SCHEME) {
        SSH_SCHEME
    } else {
        return None;
    };
    let rest = rest.strip_prefix("//")?;
    let rest = rest.split(['?', '#']).next().unwrap_or("");
    let (authority, raw_path) = match rest.find('/') {
        Some(at) => (&rest[..at], &rest[at..]),
        None => (rest, "/"),
    };
    let path = percent_decode(raw_path)?;
    let bad = path.chars().any(|c| c == '\\' || c.is_control()) || path.split('/').any(|seg| seg == "..");
    (!bad && path.len() <= MAX_STRING).then(|| (scheme, authority.to_string(), path))
}

fn percent_decode(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = s.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// The folder part of a decoded path in the form the roots use: a file page's path loses its
/// leading `/` (`C:/T/a.html`), an ssh path keeps it.
fn folder_of(scheme: &str, path: &str) -> String {
    let path = if scheme == FILE_SCHEME { path.strip_prefix('/').unwrap_or(path) } else { path };
    match path.rfind('/') {
        Some(at) => path[..=at].to_string(),
        None => String::new(),
    }
}

/// The root of an `iterm2-file:` or `iterm2-ssh:` page (a trailing `/` marks a folder), or None.
/// A file page has no host; an ssh page must have one.
pub fn page_root(url: &str) -> Option<PageRoot> {
    let (scheme, authority, path) = local_parts(url)?;
    let host = match scheme {
        FILE_SCHEME if authority.is_empty() => None,
        SSH_SCHEME if !authority.is_empty() => Some(authority),
        _ => return None,
    };
    let folder = folder_of(scheme, &path);
    // A file page's folder starts at an explicit drive (`C:/`): `iterm2-file:///` and `///C:` would
    // otherwise admit every path of every drive (Task 1 review, minor 1).
    let drive = |f: &str| f.as_bytes().get(..3).is_some_and(|b| b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'/');
    if scheme == FILE_SCHEME && !drive(&folder) {
        return None;
    }
    Some(PageRoot { scheme, host, folder })
}

/// Whether a page rooted at `root` may load `url` as a subresource: the same scheme and host, a
/// path inside the root folder (a path prefix, not a string prefix), and never a script-driven
/// request (fetch, XHR, WebSocket, EventSource).
pub fn subresource_allowed(root: Option<&PageRoot>, url: &str, context: i32) -> bool {
    let Some(root) = root else { return false };
    if matches!(context, CONTEXT_FETCH | CONTEXT_XML_HTTP_REQUEST | CONTEXT_WEBSOCKET | CONTEXT_EVENT_SOURCE | CONTEXT_DOCUMENT) {
        return false;
    }
    let Some((scheme, authority, path)) = local_parts(url) else { return false };
    if scheme != root.scheme {
        return false;
    }
    let host_ok = match &root.host {
        Some(host) => authority.eq_ignore_ascii_case(host),
        None => authority.is_empty(),
    };
    let shown = if scheme == FILE_SCHEME { path.strip_prefix('/').unwrap_or(&path) } else { path.as_str() };
    let inside = shown.get(..root.folder.len()).is_some_and(|head| if scheme == FILE_SCHEME { head.eq_ignore_ascii_case(&root.folder) } else { head == root.folder });
    host_ok && inside && shown.len() > root.folder.len()
}

/// Whether a request for a file or ssh URL may be answered at all. A Document request only for the
/// URL of the pane's pending main-frame navigation (`pending`, set from `Event::Starting`, so a
/// frame or a script cannot ask for a page the person did not open). Every other context only as a
/// subresource of the COMMITTED page's tree, and only when the document making the request
/// (`top_url`, the view's Source) is itself a page of that same root: a web page that still holds
/// the main frame because a file navigation never committed gets a 403 (Task 1 review, I1).
pub fn page_request_allowed(pending: Option<(&PageRoot, &str)>, committed: Option<&PageRoot>, top_url: &str, url: &str, context: i32) -> bool {
    if context == CONTEXT_DOCUMENT {
        return pending.is_some_and(|(_, nav_url)| nav_url == url);
    }
    committed.is_some() && page_root(top_url).as_ref() == committed && subresource_allowed(committed, url, context)
}

/// The script that makes `window.webkit.messageHandlers[name].postMessage(body)` (iTerm2's pages call
/// it) post to the host, on `iterm2-about:` pages only: `browser_js::local_page_script` through
/// `browser_js::wrap`, the one bridge text every script uses. It holds no secret: the host decides by
/// the message's source (`browser_js::local_page_ok`), not by anything in here.
pub fn shim_js() -> String {
    crate::browser_js::wrap(&crate::browser_js::local_page_script())
}

pub const MAX_MESSAGE: usize = 64 * 1024;
pub const MAX_STRING: usize = 8192;
/// `COREWEBVIEW2_WEB_RESOURCE_CONTEXT_DOCUMENT`.
pub const CONTEXT_DOCUMENT: i32 = 1;
const MAX_OFFSET: i64 = 100_000;
const MAX_PAGE: i64 = 100;
/// The cells a local page's URL line gets from the one shortener (the page wraps it, never clips).
const PAGE_URL_CELLS: usize = 160;
const SCHEME_PREFIX: &str = "iterm2-about:";

/// What a message asks the app to do.
#[derive(Clone, Debug, PartialEq)]
pub enum Reply {
    /// Run in the page that asked (guarded: it does nothing if that page is gone).
    Script(String),
    /// Load this URL in the pane (Tandem's own load).
    Navigate(String),
    /// A change to the saved permissions, applied by the app through the one write queue (Task 9).
    Permission(PermissionOp),
}

/// What the permissions page asked for, already typed: the origin is a normalized one.
#[derive(Clone, Debug, PartialEq)]
pub enum PermissionOp {
    Revoke { origin: String, kind: crate::browser_perms::PermissionType },
    RevokeAll { origin: String },
    ClearAll,
}

const COMMON_CSS: &str = include_str!("../resources/browser/iterm-browser-common.css");

/// The page a URL names, by exact name: the scheme compared ignoring ASCII case, the name one of
/// a closed list (no query, fragment, slash, `..`, escape, NUL or long name; P8).
pub fn page_name(url: &str) -> Option<&str> {
    let (scheme, rest) = url.split_once(':')?;
    if !scheme.eq_ignore_ascii_case("iterm2-about") || rest.is_empty() || rest.len() > 64 {
        return None;
    }
    matches!(rest, "welcome" | "history" | "bookmarks" | "permissions" | "onboarding-intro" | "onboarding-features" | "iterm-browser-common.css").then_some(rest)
}

/// A page's document and its MIME type, its `{{...}}` slots filled as iTerm2's template loader
/// does (`browser_js::fill_template`, the one path). Served from the binary, never from disk.
pub fn document(name: &str, nonce: &str) -> Option<(String, &'static str)> {
    const HTML: &str = "text/html; charset=utf-8";
    use crate::browser_js::{fill_template, resource};
    let page = |html: &str, slot: &str, script: &str| fill_template(html, &[("NONCE", nonce), (slot, script)]);
    Some(match name {
        "welcome" => {
            // iTerm2 puts a per-run secret into this script; here the host decides by the source.
            let script = fill_template(resource("welcome-page.js")?, &[("SECRET", "")]);
            (page(resource("welcome-page.html")?, "WELCOME_SCRIPT", &script), HTML)
        }
        "history" => (page(resource("history-page.html")?, "HISTORY_SCRIPT", resource("history-page.js")?), HTML),
        "bookmarks" => (page(resource("bookmarks-page.html")?, "BOOKMARKS_SCRIPT", resource("bookmarks-page.js")?), HTML),
        "permissions" => (permissions_document(nonce)?, HTML),
        "onboarding-intro" | "onboarding-features" => (static_page(&format!("{SCHEME_PREFIX}{name}"))?, HTML),
        "iterm-browser-common.css" => (format!("{COMMON_CSS}{GLYPH_CSS}"), "text/css; charset=utf-8"),
        _ => return None,
    })
}

/// The copied permissions page stays byte-identical on disk. The nonce policy forbids its bare
/// `<script>` and the inline `onclick` handlers (three in the HTML, two built from data in the JS),
/// so they are adapted at load time (R13x3-1/-2 pattern, the 13x4 view-source precedent): each
/// target must be found exactly once or no page is served. (name, iTerm2's text, Tandem's text)
pub const HTML_PATCHES: [(&str, &str, &str); 4] = [
    ("SCRIPT", "    <script>\n        {{PERMISSIONS_SCRIPT}}\n    </script>\n", "    <script nonce=\"{{NONCE}}\">\n        {{PERMISSIONS_SCRIPT}}\n    </script>\n    <script nonce=\"{{NONCE}}\">\n        {{TANDEM_SCRIPT}}\n    </script>\n"),
    ("CLEAR_ALL", "onclick=\"clearAllPermissions()\"", "data-tandem-action=\"clearAll\""),
    ("CLEAR_SEARCH", "onclick=\"clearSearch()\"", "data-tandem-action=\"clearSearch\""),
    // The page's own load handler already listens to this button's click.
    ("LOAD_MORE", "onclick=\"loadMore()\" ", ""),
];
pub const JS_PATCHES: [(&str, &str, &str); 2] = [
    ("REVOKE_ALL", "onclick=\"revokeAllPermissions('${escapeAttribute(origin)}')\"", "data-tandem-action=\"revokeAll\""),
    ("REVOKE", "onclick=\"revokePermission('${escapeAttribute(permission.origin)}', '${escapeAttribute(permission.permissionType)}')\"", "data-tandem-action=\"revoke\""),
];

/// Tandem's own script for the page: one delegated click listener. It ignores a click a script
/// made up (`isTrusted`), and reads the origin and type from attributes the page set with
/// `setAttribute`, never from markup built from data. The page's own functions still ask `confirm()`.
pub const TANDEM_SCRIPT: &str = "document.addEventListener('click', function (e) {
            if (!e.isTrusted || !e.target || !e.target.closest) { return; }
            var el = e.target.closest('[data-tandem-action]');
            if (!el) { return; }
            var act = el.getAttribute('data-tandem-action');
            var entry = el.closest('.permission-entry');
            var section = el.closest('.origin-section');
            if (act === 'revoke' && entry) {
                revokePermission(entry.getAttribute('data-permission-origin'), entry.getAttribute('data-permission-type'));
            } else if (act === 'revokeAll' && section) {
                revokeAllPermissions(section.getAttribute('data-origin'));
            } else if (act === 'clearAll') {
                clearAllPermissions();
            } else if (act === 'clearSearch') {
                clearSearch();
            }
        });";

/// The permissions page as served, or None (a patch missed: the page is a 404, fail closed).
fn permissions_document(nonce: &str) -> Option<String> {
    use crate::browser_js::{fill_template, resource};
    use crate::view_source::apply_patches;
    let patched = apply_patches(resource("permissions-page.html")?, &HTML_PATCHES).and_then(|h| apply_patches(resource("permissions-page.js").unwrap_or_default(), &JS_PATCHES).map(|j| (h, j)));
    match patched {
        Ok((html, js)) => Some(fill_template(&html, &[("NONCE", nonce), ("PERMISSIONS_SCRIPT", &js), ("TANDEM_SCRIPT", TANDEM_SCRIPT)])),
        Err(e) => {
            crate::rlog!("{}", crate::dlog::name(&e));
            None
        }
    }
}

pub fn response_headers(nonce: &str) -> Vec<(String, String)> {
    vec![
        ("Content-Security-Policy".to_string(), csp(nonce)),
        ("X-Content-Type-Options".to_string(), "nosniff".to_string()),
        ("Cache-Control".to_string(), "no-store".to_string()),
    ]
}

/// The answer to one request for an `iterm2-about:` URL: the page, or a 404, always under the
/// policy. A web page's script, image or fetch for a local page's URL is a 404 (P8): only a
/// document navigation (the host cannot tell a frame from the main frame here; the policy's
/// `frame-ancestors 'none'` refuses a frame) or a request made by a local page is answered.
pub fn answer(url: &str, context: i32, top_url: &str) -> Answer {
    let local_top = scheme_of(top_url) == "iterm2-about";
    let nonce = new_nonce();
    let served = page_name(url).filter(|_| context == CONTEXT_DOCUMENT || local_top).and_then(|n| document(n, &nonce));
    match served {
        Some((body, mime)) => Answer::Respond { status: 200, mime: mime.to_string(), body: body.into_bytes(), headers: response_headers(&nonce) },
        None => Answer::Respond { status: 404, mime: "text/plain; charset=utf-8".to_string(), body: b"Not found".to_vec(), headers: response_headers(&nonce) },
    }
}

/// Whether the browser loads this URL from a local page's message: http, https or `about:blank`.
pub fn navigable(url: &str) -> bool {
    matches!(scheme_of(url).as_str(), "http" | "https") || url.eq_ignore_ascii_case("about:blank")
}

/// A navigation a message asks for: honored only for a URL already in the database that is loadable.
fn navigate_known(db: &Db, url: &str) -> Option<Vec<Reply>> {
    (url.len() <= MAX_STRING && navigable(url) && db.knows_url(url)).then(|| vec![Reply::Navigate(url.to_string())])
}

/// A script that runs `call` only on the page that asked: a page that navigated away since gets
/// nothing (the data is the person's history).
fn script(page: &str, call: &str) -> Vec<Reply> {
    let src = Value::String(format!("{SCHEME_PREFIX}{page}"));
    vec![Reply::Script(format!("(function () {{ if (location.protocol !== 'iterm2-about:' || location.href.split('#')[0] !== {src}) {{ return 0; }} {call}; return 1; }})()"))]
}

fn call(name: &str, arg: &Value) -> String {
    format!("window.{name} && window.{name}({arg})")
}

// --- Typed arguments: a wrong type, range or length rejects the whole message ---

fn keys_within(body: &Map<String, Value>, allowed: &[&str]) -> Option<()> {
    body.keys().all(|k| allowed.contains(&k.as_str())).then_some(())
}

fn int(body: &Map<String, Value>, key: &str, default: i64, lo: i64, hi: i64) -> Option<i64> {
    match body.get(key) {
        None => Some(default),
        Some(v) => v.as_i64().filter(|n| (lo..=hi).contains(n)),
    }
}

fn text(body: &Map<String, Value>, key: &str) -> Option<String> {
    match body.get(key) {
        None => Some(String::new()),
        Some(Value::String(s)) if s.len() <= MAX_STRING => Some(s.clone()),
        _ => None,
    }
}

fn required_text(body: &Map<String, Value>, key: &str) -> Option<String> {
    match body.get(key) {
        Some(Value::String(s)) if s.len() <= MAX_STRING => Some(s.clone()),
        _ => None,
    }
}

/// One message from a local page. Nothing here trusts the page: the source must be an
/// `iterm2-about:` page, the handler must be that page's own, the envelope is exactly
/// `{handler, body}`, the body is a closed set of actions with typed bounded arguments.
pub fn route(db: &Db, json: &str, source: &str) -> Vec<Reply> {
    if json.len() > MAX_MESSAGE || !browser::local_source_ok(source) {
        return Vec::new();
    }
    let Some(page) = page_name(source) else { return Vec::new() };
    let Ok(Value::Object(envelope)) = serde_json::from_str::<Value>(json) else { return Vec::new() };
    if envelope.len() != 2 {
        return Vec::new();
    }
    let (Some(Value::String(handler)), Some(Value::Object(body))) = (envelope.get("handler"), envelope.get("body")) else { return Vec::new() };
    if handler.strip_prefix(SCHEME_PREFIX) != Some(page) {
        return Vec::new();
    }
    let out = match page {
        "history" => history(db, body, page),
        "bookmarks" => bookmarks(db, body, page),
        "welcome" => welcome(db, body, page),
        "permissions" => permissions(db, body, page),
        _ => None,
    };
    out.unwrap_or_default()
}

/// A URL as a local page draws it: the one display and the one shortener at PAGE_URL_CELLS (the
/// page's CSS wraps the line, so nothing clips it; final residual 1).
fn page_url(url: &str) -> String {
    browser::fit_address(&browser::display_url(url), PAGE_URL_CELLS)
}

fn history(db: &Db, body: &Map<String, Value>, page: &str) -> Option<Vec<Reply>> {
    match body.get("action")?.as_str()? {
        "loadEntries" => {
            keys_within(body, &["action", "offset", "limit", "searchQuery"])?;
            let offset = int(body, "offset", 0, 0, MAX_OFFSET)? as usize;
            let limit = int(body, "limit", 50, 1, MAX_PAGE)? as usize;
            let query = text(body, "searchQuery")?;
            // One more than asked, to know whether there is more (`BHVH:99-111`).
            let rows = if query.trim().is_empty() { db.recent_history(offset, limit + 1) } else { db.search_history(&query, offset, limit + 1) };
            let more = rows.len() > limit;
            let entries: Vec<Value> = rows.into_iter().take(limit).map(|r| json!({"id": r.id, "displayUrl": page_url(&r.url), "url": r.url, "title": r.title.unwrap_or_default(), "visitDate": r.visit_date, "transitionType": r.transition})).collect();
            Some(script(page, &call("onHistoryEntriesLoaded", &json!({"entries": entries, "hasMore": more}))))
        }
        "deleteEntry" => {
            keys_within(body, &["action", "entryId"])?;
            let id = required_text(body, "entryId")?;
            uuid::Uuid::parse_str(&id).ok().filter(|_| id.len() == 36)?;
            db.delete_history(&id);
            Some(script(page, &call("onHistoryEntryDeleted", &Value::String(id))))
        }
        "navigateToURL" => {
            keys_within(body, &["action", "url"])?;
            navigate_known(db, &required_text(body, "url")?)
        }
        "clearAllHistory" => {
            keys_within(body, &["action"])?;
            db.clear_history();
            Some(script(page, "window.onHistoryCleared && window.onHistoryCleared()"))
        }
        _ => None,
    }
}

/// An origin a page names: exactly the normalized form (the database's keys), else rejected.
fn origin_arg(body: &Map<String, Value>) -> Option<String> {
    let origin = required_text(body, "origin")?;
    (crate::browser_perms::normalize_origin(&origin).as_deref() == Some(origin.as_str())).then_some(origin)
}

fn permissions(db: &Db, body: &Map<String, Value>, page: &str) -> Option<Vec<Reply>> {
    use crate::browser_perms as bp;
    match body.get("action")?.as_str()? {
        "loadPermissions" => {
            keys_within(body, &["action", "offset", "limit", "searchQuery", "permissionTypeFilter", "statusFilter"])?;
            int(body, "offset", 0, 0, MAX_OFFSET)?;
            int(body, "limit", 50, 1, MAX_PAGE)?;
            for key in ["searchQuery", "permissionTypeFilter", "statusFilter"] {
                text(body, key)?;
            }
            let q = bp::PageQuery::from_message(&Value::Object(body.clone()));
            let (rows, more) = bp::page_rows(&db.all_permissions(), &q);
            Some(script(page, &call("onPermissionsLoaded", &json!({"permissions": rows, "hasMore": more}))))
        }
        "revokePermission" => {
            keys_within(body, &["action", "origin", "permissionType"])?;
            let origin = origin_arg(body)?;
            let kind = bp::PermissionType::parse(&required_text(body, "permissionType")?)?;
            let reply = format!("window.onPermissionRevoked && window.onPermissionRevoked({}, {})", Value::String(origin.clone()), Value::String(kind.raw().to_string()));
            let mut out = vec![Reply::Permission(PermissionOp::Revoke { origin, kind })];
            out.extend(script(page, &reply));
            Some(out)
        }
        "revokeAllPermissions" => {
            keys_within(body, &["action", "origin"])?;
            let origin = origin_arg(body)?;
            let reply = call("onAllPermissionsRevoked", &Value::String(origin.clone()));
            let mut out = vec![Reply::Permission(PermissionOp::RevokeAll { origin })];
            out.extend(script(page, &reply));
            Some(out)
        }
        "clearAllPermissions" => {
            keys_within(body, &["action"])?;
            let mut out = vec![Reply::Permission(PermissionOp::ClearAll)];
            out.extend(script(page, "window.onAllPermissionsCleared && window.onAllPermissionsCleared()"));
            Some(out)
        }
        _ => None,
    }
}

fn bookmarks(db: &Db, body: &Map<String, Value>, page: &str) -> Option<Vec<Reply>> {
    match body.get("action")?.as_str()? {
        "loadBookmarks" => {
            keys_within(body, &["action", "offset", "limit", "searchQuery", "sortBy", "tags"])?;
            let offset = int(body, "offset", 0, 0, MAX_OFFSET)? as usize;
            let limit = int(body, "limit", 50, 1, MAX_PAGE)? as usize;
            let query = text(body, "searchQuery")?;
            let sort = match body.get("sortBy") {
                None => "dateAdded",
                Some(Value::String(s)) if matches!(s.as_str(), "dateAdded" | "title" | "url") => s.as_str(),
                _ => return None,
            };
            let tags: Vec<&Value> = match body.get("tags") {
                None => Vec::new(),
                Some(Value::Array(a)) if a.len() <= 16 && a.iter().all(|t| t.as_str().is_some_and(|t| t.len() <= 128)) => a.iter().collect(),
                _ => return None,
            };
            // Tags are not kept (the tag table is not created), so a tag filter finds nothing.
            let rows: Vec<(String, Option<String>, i64)> = if !tags.is_empty() {
                Vec::new()
            } else if query.trim().is_empty() {
                db.all_bookmarks(sort, offset, limit + 1).into_iter().map(|b| (b.url, b.title, b.date_added)).collect()
            } else {
                db.search_bookmarks(&query, offset, limit + 1)
            };
            let more = rows.len() > limit;
            let list: Vec<Value> = rows.into_iter().take(limit).map(|(url, title, added)| json!({"displayUrl": page_url(&url), "url": url, "title": title.unwrap_or_default(), "dateAdded": added})).collect();
            Some(script(page, &call("onBookmarksLoaded", &json!({"bookmarks": list, "hasMore": more}))))
        }
        "loadTags" => {
            keys_within(body, &["action"])?;
            Some(script(page, &call("onTagsLoaded", &json!({"tags": []}))))
        }
        "deleteBookmark" => {
            keys_within(body, &["action", "url"])?;
            let url = required_text(body, "url")?;
            db.remove_bookmark(&url).then(|| script(page, &call("onBookmarkDeleted", &Value::String(url))))
        }
        "navigateToURL" => {
            keys_within(body, &["action", "url"])?;
            navigate_known(db, &required_text(body, "url")?)
        }
        "clearAllBookmarks" => {
            keys_within(body, &["action"])?;
            db.clear_bookmarks();
            Some(script(page, "window.onBookmarksCleared && window.onBookmarksCleared()"))
        }
        _ => None,
    }
}

fn welcome(db: &Db, body: &Map<String, Value>, page: &str) -> Option<Vec<Reply>> {
    // The page sends iTerm2's `sessionSecret`; it is not used (the source decides), only bounded.
    text(body, "sessionSecret")?;
    match body.get("action")?.as_str()? {
        "loadTopSites" => {
            keys_within(body, &["action", "sessionSecret"])?;
            let sites: Vec<Value> = db
                .top_sites(5)
                .into_iter()
                .filter(|s| navigable(&s.url))
                .map(|s| json!({"displayHost": browser::display_host(&s.url), "url": s.url, "title": s.title.filter(|t| !t.is_empty()).unwrap_or_default(), "hostname": s.hostname, "visitCount": s.visits}))
                .collect();
            Some(script(page, &call("handleTopSitesResponse", &Value::Array(sites))))
        }
        "navigateToURL" => {
            keys_within(body, &["action", "url", "sessionSecret"])?;
            navigate_known(db, &required_text(body, "url")?)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EVIL_TITLE: &str = "\"><img src=x onerror=alert(1)>";
    const EVIL_URL: &str = "https://evil.test/');alert(1);//\"q";

    fn msg(handler: &str, body: serde_json::Value) -> String {
        serde_json::json!({ "handler": handler, "body": body }).to_string()
    }

    fn scripts(r: &[Reply]) -> Vec<&str> {
        r.iter().filter_map(|r| if let Reply::Script(s) = r { Some(s.as_str()) } else { None }).collect()
    }

    fn db() -> Db {
        let db = Db::open_in_memory();
        db.record_visit("https://a.test/", Some("A"), None, None, 0, 100);
        db.record_visit(EVIL_URL, Some(EVIL_TITLE), None, None, 1, 200);
        db.add_bookmark("https://bk.test/", Some("Mark"), 300);
        db
    }

    #[test]
    fn local_page_lookup_is_exact() {
        for good in ["iterm2-about:welcome", "ITERM2-ABOUT:history", "iterm2-about:bookmarks", "Iterm2-About:iterm-browser-common.css"] {
            assert!(page_name(good).is_some(), "{good}");
        }
        assert_eq!(page_name("iterm2-about:history"), Some("history"));
        let long = format!("iterm2-about:{}", "a".repeat(5000));
        for bad in [
            "iterm2-about:../Cargo.toml",
            "iterm2-about:history/../../x",
            "iterm2-about://history",
            "iterm2-about:%2e%2e/x",
            "iterm2-about:history?x=1",
            "iterm2-about:history#frag",
            "iterm2-about:history/",
            "iterm2-about:History",
            "iterm2-about:history\0",
            "iterm2-about:",
            "iterm2-aboutx:history",
            "https://iterm2-about:history",
            "https://evil.test/iterm2-about:history",
            "",
            long.as_str(),
        ] {
            assert_eq!(page_name(bad), None, "{bad:?}");
        }
        for name in ["welcome", "history", "bookmarks"] {
            let (html, mime) = document(name, "n0nce").unwrap();
            assert_eq!(mime, "text/html; charset=utf-8");
            assert!(!html.contains("{{"), "{name}: every slot is filled");
            assert!(html.contains("<script nonce=\"n0nce\">") && html.contains("</html>"));
        }
        assert_eq!(document("iterm-browser-common.css", "n").unwrap().1, "text/css; charset=utf-8");
        assert!(document("settings", "n").is_none() && document("../x", "n").is_none());
        let welcome = document("welcome", "n").unwrap().0;
        assert!(welcome.contains("Welcome to Tandem Browser") && !welcome.contains("iTerm2 Browser"));
        assert!(!welcome.contains("sessionSecret = '{{") && welcome.contains("const sessionSecret = '';"), "no secret is served");
    }

    #[test]
    fn local_page_headers_carry_csp() {
        let want_for = |n: &str| format!("default-src 'none'; script-src 'nonce-{n}'; style-src 'unsafe-inline'; img-src data: http: https:; connect-src 'none'; form-action 'none'; base-uri 'none'; frame-ancestors 'none'");
        assert_eq!(csp("abc"), want_for("abc"));
        for url in ["iterm2-about:history", "iterm2-about:welcome", "iterm2-about:bookmarks", "iterm2-about:nope", "iterm2-about:../x"] {
            let Answer::Respond { status, mime, headers, body } = answer(url, CONTEXT_DOCUMENT, "") else { panic!("{url}: a response") };
            let h = |n: &str| headers.iter().find(|(k, _)| k == n).map(|(_, v)| v.as_str());
            let got = h("Content-Security-Policy").unwrap_or_default();
            assert_eq!(got, want_for(got.split("'nonce-").nth(1).and_then(|r| r.split('\'').next()).unwrap_or("")), "{url}");
            assert_eq!(h("X-Content-Type-Options"), Some("nosniff"));
            assert_eq!(h("Cache-Control"), Some("no-store"));
            assert_eq!(status == 200, url != "iterm2-about:nope" && url != "iterm2-about:../x", "{url}");
            if status != 200 {
                assert_eq!(status, 404);
                assert!(mime.starts_with("text/plain") && !body.is_empty());
            }
        }
    }

    /// Review ruling (Task 7 round 1, item 1): a per-response nonce, no `'unsafe-inline'` for
    /// script, and no inline handler attribute in any page, so a missed escape cannot run script.
    #[test]
    fn local_pages_use_a_nonce_and_no_inline_handlers() {
        let nonce_of = |csp: &str| -> String {
            let script = csp.split(';').map(str::trim).find(|d| d.starts_with("script-src")).expect("a script-src").to_string();
            assert!(!script.contains("unsafe-inline") && !script.contains("unsafe-eval") && !script.contains('*'), "{script}");
            script.split("'nonce-").nth(1).expect("a nonce source").split('\'').next().unwrap().to_string()
        };
        let handler = regex::Regex::new(r#"(?is)<[a-z][^<>]*?\son[a-z]+\s*="#).unwrap();
        // The detector sees every way to write a handler attribute inside a tag, quoted or not.
        for planted in ["<a onclick=\"x()\">", "<a onclick='x()'>", "<a onclick=x()>", "<img src=y onerror=x()>", "<a\nonmouseover = x>"] {
            assert!(handler.is_match(planted), "{planted}");
        }
        for fine in ["window.onload = function () {}", "<a data-url=\"x\">", "<script nonce=\"a\">"] {
            assert!(!handler.is_match(fine), "{fine}");
        }
        let mut seen = Vec::new();
        for url in ["iterm2-about:history", "iterm2-about:welcome", "iterm2-about:bookmarks", "iterm2-about:history"] {
            let Answer::Respond { headers, body, .. } = answer(url, CONTEXT_DOCUMENT, "") else { panic!("{url}") };
            let csp = headers.iter().find(|(k, _)| k == "Content-Security-Policy").map(|(_, v)| v.clone()).unwrap();
            let nonce = nonce_of(&csp);
            assert!(nonce.len() >= 16 && nonce.bytes().all(|b| b.is_ascii_alphanumeric()), "{nonce}");
            let html = String::from_utf8(body).unwrap();
            assert!(html.contains(&format!("<script nonce=\"{nonce}\">")), "{url}: the page's script carries this response's nonce");
            assert_eq!(html.matches("<script").count(), html.matches("<script nonce=").count(), "{url}: every script has the nonce");
            assert!(!handler.is_match(&html), "{url}: no inline handler attribute: {:?}", handler.find(&html).map(|m| m.as_str()));
            assert!(!html.contains("javascript:"), "{url}");
            seen.push(nonce);
        }
        assert_ne!(seen[0], seen[3], "a new nonce for every response");
        // The 404 is under the same policy and holds no script.
        let Answer::Respond { headers, .. } = answer("iterm2-about:nope", CONTEXT_DOCUMENT, "") else { panic!() };
        let csp404 = headers.iter().find(|(k, _)| k == "Content-Security-Policy").map(|(_, v)| v.clone()).unwrap();
        assert_eq!(nonce_of(&csp404).len(), 32);
    }

    #[test]
    fn local_page_needs_a_local_top() {
        // An https page that fetches or embeds a local page's resource gets a 404; a document
        // navigation (a person's click or Tandem's own load) and a local top are answered.
        let img = 3;
        for ctx in [img, 2, 6, 7] {
            let Answer::Respond { status, .. } = answer("iterm2-about:history", ctx, "https://evil.test/") else { panic!() };
            assert_eq!(status, 404, "context {ctx} from a web page");
        }
        let Answer::Respond { status, .. } = answer("iterm2-about:history", CONTEXT_DOCUMENT, "https://a.test/") else { panic!() };
        assert_eq!(status, 200, "a document navigation (the frame is refused by frame-ancestors)");
        let Answer::Respond { status, .. } = answer("iterm2-about:history", img, "iterm2-about:welcome") else { panic!() };
        assert_eq!(status, 200, "a local page's own resources");
        let Answer::Respond { status, .. } = answer("iterm2-about:history", img, "") else { panic!() };
        assert_eq!(status, 404, "no top, not a document");
    }

    #[test]
    fn local_pages_escape_attributes() {
        let pages = [
            ("history-page.js", include_str!("../resources/browser/history-page.js")),
            ("bookmarks-page.js", include_str!("../resources/browser/bookmarks-page.js")),
            ("welcome-page.js", include_str!("../resources/browser/welcome-page.js")),
        ];
        for (name, text) in pages {
            // One attribute escaper that maps all five characters.
            assert_eq!(text.matches("function escapeAttr(").count(), 1, "{name}");
            let body = &text[text.find("function escapeAttr(").unwrap()..];
            let body = &body[..body.find("\n}").unwrap_or(body.len().min(400))];
            for ch in ["&amp;", "&lt;", "&gt;", "&quot;", "&#39;"] {
                assert!(body.contains(ch), "{name}: escapeAttr maps {ch}");
            }
            assert!(!text.contains("escapeAttribute"), "{name}: the old escaper (it let ' break out) is gone");
        }
        for (name, text) in pages.iter().chain([("history-page.html", include_str!("../resources/browser/history-page.html")), ("bookmarks-page.html", include_str!("../resources/browser/bookmarks-page.html")), ("welcome-page.html", include_str!("../resources/browser/welcome-page.html"))].iter()) {
            // No inline handler is built from data: an `onclick` holding a template slot, or any
            // handler attribute inside a template literal.
            for attr in ["onclick=\"", "onerror=\"", "onclick='", "onerror='"] {
                for (at, _) in text.match_indices(attr) {
                    let rest = &text[at + attr.len()..];
                    let value = &rest[..rest.find(attr.chars().last().unwrap()).unwrap_or(rest.len())];
                    assert!(!value.contains("${") && !value.contains("{{"), "{name}: {attr}{value}");
                }
            }
            assert!(!text.contains("onclick=\"navigateToURL(") && !text.contains("onclick=\"deleteHistoryEntry(") && !text.contains("onclick=\"deleteBookmark("), "{name}");
        }
        // The copied pages kept their own structure: the data attributes and listeners replace the handlers.
        let h = pages[0].1;
        assert!(h.contains("data-url=\"${escapeAttr(url)}\"") && h.contains("data-id=\"${escapeAttr(entry.id)}\"") && h.contains("addEventListener('click'"));
        let b = pages[1].1;
        assert_eq!(b.matches("data-url=\"${escapeAttr(url)}\"").count(), 2);
    }

    /// Final residual 1 (the URL display class): the history, bookmarks and welcome pages draw a URL
    /// or a host from the one display (`displayUrl`: display_url through fit_address at PAGE_URL_CELLS;
    /// `displayHost`: the display host), and their CSS wraps it: no line of theirs clips a URL's end.
    #[test]
    fn local_pages_never_clip_a_url() {
        let db = Db::open_in_memory();
        let url = format!("https://accounts.google.com.paypal.com.co/{}", "p".repeat(400));
        db.record_visit(&url, None, None, None, 0, 100);
        db.add_bookmark(&url, None, 300);
        let want = browser::fit_address(&browser::display_url(&url), PAGE_URL_CELLS);
        assert!(want.contains("paypal.com.co/") && want.chars().count() <= PAGE_URL_CELLS, "{want}");
        let want = serde_json::to_string(&want).unwrap();
        let h = route(&db, &msg("iterm2-about:history", serde_json::json!({"action": "loadEntries", "offset": 0, "limit": 50, "searchQuery": ""})), "iterm2-about:history");
        assert!(scripts(&h)[0].contains(&format!("\"displayUrl\":{want}")), "history: {}", scripts(&h)[0]);
        for q in ["", "paypal"] {
            let b = route(&db, &msg("iterm2-about:bookmarks", serde_json::json!({"action": "loadBookmarks", "offset": 0, "limit": 50, "searchQuery": q})), "iterm2-about:bookmarks");
            assert!(scripts(&b)[0].contains(&format!("\"displayUrl\":{want}")), "bookmarks {q:?}: {}", scripts(&b)[0]);
        }
        let w = route(&db, &msg("iterm2-about:welcome", serde_json::json!({"action": "loadTopSites", "sessionSecret": ""})), "iterm2-about:welcome");
        assert!(scripts(&w)[0].contains("\"displayHost\":\"accounts.google.com.paypal.com.co\""), "welcome: {}", scripts(&w)[0]);
        // The pages draw those, and wrap them.
        assert!(include_str!("../resources/browser/history-page.js").contains("${escapeHtml(entry.displayUrl)}"));
        assert!(include_str!("../resources/browser/bookmarks-page.js").contains("${escapeHtml(bookmark.displayUrl)}"));
        let welcome = include_str!("../resources/browser/welcome-page.js");
        assert!(welcome.contains("${escapeHtml(site.displayHost)}") && !welcome.contains("escapeHtml(site.hostname)") && !welcome.contains("getInitial(site.title || site.hostname)"));
        let rule = |css: &str, sel: &str| css[css.find(&format!("{sel} {{")).unwrap()..].split('}').next().unwrap().to_string();
        for (css, sel) in [(COMMON_CSS, ".entry-url"), (include_str!("../resources/browser/welcome-page.html"), ".site-url"), (include_str!("../resources/browser/welcome-page.html"), ".site-title.host")] {
            let r = rule(css, sel);
            assert!(!r.contains("nowrap") && !r.contains("ellipsis") && r.contains("overflow-wrap: anywhere"), "{sel}: {r}");
        }
    }

    #[test]
    fn the_license_copy_matches_the_one_in_resources() {
        assert_eq!(include_bytes!("../resources/browser/LICENSE-iTerm2").as_slice(), include_bytes!("../resources/LICENSE-iTerm2").as_slice());
    }

    #[test]
    fn local_page_messages_need_their_source() {
        let db = db();
        let clear = msg("iterm2-about:history", serde_json::json!({"action": "clearAllHistory"}));
        for source in ["https://evil.invalid/", "", "iterm2-aboutx:y", "iterm2-about:bookmarks", "iterm2-about:welcome", "blob:iterm2-about:history/1"] {
            assert!(route(&db, &clear, source).is_empty(), "{source:?}");
            assert_eq!(db.recent_history(0, 10).len(), 2, "history keeps its rows ({source:?})");
        }
        let r = route(&db, &clear, "ITERM2-ABOUT:history");
        assert_eq!(scripts(&r).len(), 1);
        assert!(db.recent_history(0, 10).is_empty(), "the history page may clear history");
    }

    #[test]
    fn local_page_navigate_needs_a_known_url() {
        let db = db();
        let nav = |url: &str| route(&db, &msg("iterm2-about:history", serde_json::json!({"action": "navigateToURL", "url": url})), "iterm2-about:history");
        assert_eq!(nav("https://a.test/"), vec![Reply::Navigate("https://a.test/".into())]);
        assert_eq!(nav(EVIL_URL), vec![Reply::Navigate(EVIL_URL.into())], "a stored URL with a quote is just a URL");
        for unknown in ["https://other.test/", "file:///C:/Windows/win.ini", "javascript:alert(1)", "https://user:p@a.test/", ""] {
            assert!(nav(unknown).is_empty(), "{unknown}");
        }
        // A stored row with a scheme the browser never loads from here is not navigated.
        let d = Db::open_in_memory();
        for bad in ["javascript:alert(1)", "file:///C:/x", "data:text/html,x", "iterm2-ssh://h", "ms-msdt:id"] {
            d.record_visit(bad, Some("t"), None, None, 1, 1);
            d.add_bookmark(bad, Some("t"), 1);
            let r = route(&d, &msg("iterm2-about:bookmarks", serde_json::json!({"action": "navigateToURL", "url": bad})), "iterm2-about:bookmarks");
            assert!(r.is_empty(), "{bad}");
            let r = route(&d, &msg("iterm2-about:welcome", serde_json::json!({"action": "navigateToURL", "url": bad})), "iterm2-about:welcome");
            assert!(r.is_empty(), "{bad}");
        }
        assert!(navigable("https://a/") && navigable("HTTP://a/") && navigable("about:blank"));
        assert!(!navigable("about:srcdoc") && !navigable("iterm2-about:history") && !navigable("file:///x") && !navigable("javascript:1"));
        // The welcome page navigates to a visited site too.
        let r = route(&db, &msg("iterm2-about:welcome", serde_json::json!({"action": "navigateToURL", "url": "https://a.test/", "sessionSecret": ""})), "iterm2-about:welcome");
        assert_eq!(r, vec![Reply::Navigate("https://a.test/".into())]);
    }

    #[test]
    fn a_message_is_a_closed_set() {
        let db = db();
        let h = "iterm2-about:history";
        let load = |body: serde_json::Value| route(&db, &msg(h, body), h);
        assert_eq!(scripts(&load(serde_json::json!({"action": "loadEntries", "offset": 0, "limit": 50, "searchQuery": ""}))).len(), 1);
        // Unknown action, unknown key, wrong type, out of range, too long: nothing.
        for body in [
            serde_json::json!({"action": "eval", "code": "1"}),
            serde_json::json!({"action": "loadEntries", "offset": "0"}),
            serde_json::json!({"action": "loadEntries", "offset": -1}),
            serde_json::json!({"action": "loadEntries", "limit": 0}),
            serde_json::json!({"action": "loadEntries", "limit": 101}),
            serde_json::json!({"action": "loadEntries", "offset": 1.5}),
            serde_json::json!({"action": "loadEntries", "extra": 1}),
            serde_json::json!({"action": "loadEntries", "searchQuery": "x".repeat(MAX_STRING + 1)}),
            serde_json::json!({"action": "loadEntries", "searchQuery": 5}),
            serde_json::json!({"action": "deleteEntry", "entryId": "not-a-uuid"}),
            serde_json::json!({"action": "deleteEntry", "entryId": "x' or '1'='1"}),
            serde_json::json!({"action": "deleteEntry"}),
            serde_json::json!({"action": "navigateToURL", "url": 7}),
            serde_json::json!({"action": "clearAllHistory", "all": true}),
            serde_json::json!({"action": 5}),
            serde_json::json!({}),
            serde_json::json!([1, 2]),
        ] {
            assert!(load(body.clone()).is_empty(), "{body}");
        }
        assert_eq!(db.recent_history(0, 10).len(), 2);
        // The envelope: exactly handler and body, bounded.
        assert!(route(&db, "not json", h).is_empty());
        assert!(route(&db, &serde_json::json!({"handler": h}).to_string(), h).is_empty());
        assert!(route(&db, &serde_json::json!({"handler": h, "body": {"action": "loadEntries"}, "x": 1}).to_string(), h).is_empty());
        assert!(route(&db, &serde_json::json!({"handler": 3, "body": {}}).to_string(), h).is_empty());
        let big = msg(h, serde_json::json!({"action": "loadEntries", "searchQuery": "y".repeat(MAX_MESSAGE)}));
        assert!(big.len() > MAX_MESSAGE && route(&db, &big, h).is_empty());
        // A handler must be the source's own page.
        assert!(route(&db, &msg("iterm2-about:bookmarks", serde_json::json!({"action": "loadTags"})), h).is_empty());
        assert!(route(&db, &msg("iterm2-about:settings", serde_json::json!({"action": "loadTags"})), "iterm2-about:settings").is_empty());
        // deleteEntry takes a real id.
        let id = db.recent_history(0, 10)[0].id.clone();
        let r = load(serde_json::json!({"action": "deleteEntry", "entryId": id}));
        assert_eq!(scripts(&r).len(), 1);
        assert_eq!(db.recent_history(0, 10).len(), 1);
    }

    #[test]
    fn replies_are_json_and_guarded() {
        let db = db();
        let h = "iterm2-about:history";
        let r = route(&db, &msg(h, serde_json::json!({"action": "loadEntries", "offset": 0, "limit": 50, "searchQuery": ""})), h);
        let s = scripts(&r)[0].to_string();
        assert!(s.contains("window.onHistoryEntriesLoaded"), "{s}");
        // The title and the URL are JSON string literals: the quote that would end a string is escaped.
        assert!(s.contains("\\\"><img src=x onerror=alert(1)>"), "{s}");
        assert!(s.contains("https://evil.test/');alert(1);//\\\"q"), "{s}");
        assert!(!s.contains("\"title\":\"\"><img"), "an unescaped quote closes the string: {s}");
        // The script runs only on the page that asked (a page that navigated away gets nothing).
        assert!(s.contains("location.protocol") && s.contains("iterm2-about:") && s.contains("\"iterm2-about:history\""), "{s}");
        let id = db.recent_history(0, 1)[0].id.clone();
        let r = route(&db, &msg(h, serde_json::json!({"action": "deleteEntry", "entryId": id})), h);
        let s = scripts(&r)[0];
        assert!(s.contains("onHistoryEntryDeleted") && s.contains(&format!("\"{id}\"")) && !s.contains(&format!("'{id}'")));
        // Bookmarks: the deleted URL goes back as a JSON string, not between single quotes.
        let b = "iterm2-about:bookmarks";
        db.add_bookmark(EVIL_URL, Some(EVIL_TITLE), 5);
        let r = route(&db, &msg(b, serde_json::json!({"action": "deleteBookmark", "url": EVIL_URL})), b);
        let s = scripts(&r)[0];
        assert!(s.contains("onBookmarkDeleted") && s.contains("\\\"q") && !s.contains("('https://"), "{s}");
        assert!(!db.is_bookmarked(EVIL_URL));
        // Tags: none are kept; a tag filter finds nothing.
        let r = route(&db, &msg(b, serde_json::json!({"action": "loadTags"})), b);
        assert!(scripts(&r)[0].contains("onTagsLoaded") && scripts(&r)[0].contains("\"tags\":[]"));
        let r = route(&db, &msg(b, serde_json::json!({"action": "loadBookmarks", "offset": 0, "limit": 50, "searchQuery": "", "sortBy": "title", "tags": ["x"]})), b);
        assert!(scripts(&r)[0].contains("\"bookmarks\":[]"));
        let r = route(&db, &msg(b, serde_json::json!({"action": "loadBookmarks", "offset": 0, "limit": 50, "searchQuery": "", "sortBy": "dateAdded", "tags": []})), b);
        assert!(scripts(&r)[0].contains("https://bk.test/"));
        assert!(route(&db, &msg(b, serde_json::json!({"action": "loadBookmarks", "sortBy": "'; drop"})), b).is_empty());
        // The welcome page: top sites, hostname without the leading period.
        let w = "iterm2-about:welcome";
        let r = route(&db, &msg(w, serde_json::json!({"action": "loadTopSites", "sessionSecret": ""})), w);
        let s = scripts(&r)[0];
        assert!(s.contains("handleTopSitesResponse") && s.contains("\"hostname\":\"a.test\"") && s.contains("\"visitCount\":1"), "{s}");
    }

    #[test]
    fn the_shim_does_nothing_off_a_local_page_and_holds_no_secret() {
        let shim = shim_js();
        assert!(shim.contains("location.protocol === 'iterm2-about:'") && shim.contains("window.chrome.webview.postMessage"));
        assert!(shim.contains("messageHandlers") && shim.contains("{h: name, b: body}"));
        assert!(!shim.to_ascii_lowercase().contains("secret") && !shim.contains("uuid"));
        // The page's own text only becomes `window.webkit` behind the protocol test.
        assert!(shim.find("location.protocol").unwrap() < shim.find("Object.defineProperty(window, 'webkit'").unwrap());
        // One path: the document is filled by the template loader, nonce and slots included.
        let (html, _) = document("history", "N0NCE").unwrap();
        assert!(html.contains("nonce=\"N0NCE\"") && !html.contains("{{"), "every slot is filled");
    }
}

#[cfg(test)]
mod permissions_tests {
    use super::*;
    use crate::browser_perms::{Decision, PermissionType};
    use serde_json::json;

    const SRC: &str = "iterm2-about:permissions";

    fn msg(body: serde_json::Value) -> String {
        json!({"handler": SRC, "body": body}).to_string()
    }

    fn db() -> Db {
        let db = Db::open_in_memory();
        db.save_permission("https://a.b", PermissionType::Camera, Decision::Granted, 10.0);
        db.save_permission("https://c.d", PermissionType::Geolocation, Decision::Denied, 20.0);
        db
    }

    fn only_script(r: &[Reply]) -> &str {
        match r {
            [Reply::Script(s)] => s,
            other => panic!("one script expected: {other:?}"),
        }
    }

    #[test]
    fn the_permissions_page_is_a_local_page_under_the_nonce_policy() {
        assert_eq!(page_name(SRC), Some("permissions"));
        assert_eq!(page_name("iterm2-about:permissions?x"), None);
        let (html, mime) = document("permissions", "n0nce").expect("served");
        assert_eq!(mime, "text/html; charset=utf-8");
        assert!(html.contains("<title>Site Permissions</title>") && html.contains("window.onPermissionsLoaded = function(data)"));
        assert!(!html.contains("{{"), "every slot is filled");
        assert_eq!(html.matches("<script").count(), html.matches("<script nonce=\"n0nce\">").count(), "every script carries the nonce");
        let handler = regex::Regex::new(r#"(?is)<[a-z][^<>]*?\son[a-z]+\s*="#).unwrap();
        assert!(!handler.is_match(&html), "no inline handler attribute: {:?}", handler.find(&html).map(|m| m.as_str()));
        assert!(!html.contains("onclick=") && !html.contains("javascript:"), "the data-built buttons use no inline handler either");
        let Answer::Respond { status, headers, .. } = answer(SRC, CONTEXT_DOCUMENT, "") else { panic!() };
        assert_eq!(status, 200);
        let csp = headers.iter().find(|(k, _)| k == "Content-Security-Policy").map(|(_, v)| v.as_str()).unwrap();
        assert!(csp.contains("frame-ancestors 'none'") && csp.contains("script-src 'nonce-") && !csp.contains("unsafe-inline'; style") );
        let Answer::Respond { status, .. } = answer(SRC, 3, "https://evil.test/") else { panic!() };
        assert_eq!(status, 404, "a web page cannot fetch it");
    }

    /// The copied files stay byte-identical on disk; the adaptations are load-time and fail closed.
    #[test]
    fn the_copies_are_patched_at_load_time_exactly_once() {
        use crate::browser_js::resource;
        let html = resource("permissions-page.html").unwrap();
        let js = resource("permissions-page.js").unwrap();
        for (name, raw, _) in HTML_PATCHES.iter().chain(JS_PATCHES.iter()) {
            let in_html = html.matches(raw).count();
            let in_js = js.matches(raw).count();
            assert_eq!(in_html + in_js, 1, "{name} is found exactly once in the copy");
        }
        let broken = html.replace("onclick=\"clearSearch()\"", "onclick=\"clearSearch( )\"");
        assert!(crate::view_source::apply_patches(&broken, &HTML_PATCHES).is_err(), "a moved target serves no page");
        let broken_js = js.replace("revokeAllPermissions('${escapeAttribute(origin)}')", "revokeAllPermissions('${origin}')");
        assert!(crate::view_source::apply_patches(&broken_js, &JS_PATCHES).is_err());
        let patched = crate::view_source::apply_patches(js, &JS_PATCHES).unwrap();
        assert!(!patched.contains("onclick=") && patched.contains("data-tandem-action=\"revoke\"") && patched.contains("data-tandem-action=\"revokeAll\""));
        // The delegated listener ignores a click a script made up and reads its data from attributes
        // the page set with setAttribute, never from markup.
        assert!(TANDEM_SCRIPT.contains("e.isTrusted") && TANDEM_SCRIPT.contains("closest("));
        assert!(!TANDEM_SCRIPT.contains("innerHTML") && !TANDEM_SCRIPT.contains("eval("));
    }

    #[test]
    fn permissions_reply_scripts_follow_iterm2_and_stay_on_the_page() {
        let r = route(&db(), &msg(json!({"action": "loadPermissions"})), SRC);
        let s = only_script(&r);
        assert!(s.contains("location.href.split('#')[0] !== \"iterm2-about:permissions\""), "the reply runs only if the page is still the permissions page");
        let call = s.split("window.onPermissionsLoaded && window.onPermissionsLoaded(").nth(1).expect("the call");
        let arg = &call[..call.find(");").unwrap()];
        let v: serde_json::Value = serde_json::from_str(arg).expect("the argument is JSON");
        assert_eq!(v["hasMore"], json!(false));
        assert_eq!(v["permissions"].as_array().unwrap().len(), 2);
        assert_eq!(v["permissions"][0]["origin"], json!("https://c.d"), "newest first");
        // Paging goes through the one query type.
        let r = route(&db(), &msg(json!({"action": "loadPermissions", "limit": 1, "permissionTypeFilter": "camera"})), SRC);
        assert!(only_script(&r).contains("\"https://a.b\""));
        // Revoke: the side effect first, then the page's reply with the origin as a JSON string.
        let r = route(&db(), &msg(json!({"action": "revokePermission", "origin": "https://a.b", "permissionType": "camera"})), SRC);
        assert_eq!(r.len(), 2);
        assert_eq!(r[0], Reply::Permission(PermissionOp::Revoke { origin: "https://a.b".into(), kind: PermissionType::Camera }));
        assert!(matches!(&r[1], Reply::Script(s) if s.contains("window.onPermissionRevoked && window.onPermissionRevoked(\"https://a.b\", \"camera\")")));
        let r = route(&db(), &msg(json!({"action": "revokeAllPermissions", "origin": "https://a.b"})), SRC);
        assert_eq!(r[0], Reply::Permission(PermissionOp::RevokeAll { origin: "https://a.b".into() }));
        assert!(matches!(&r[1], Reply::Script(s) if s.contains("window.onAllPermissionsRevoked && window.onAllPermissionsRevoked(\"https://a.b\")")));
        let r = route(&db(), &msg(json!({"action": "clearAllPermissions"})), SRC);
        assert_eq!(r[0], Reply::Permission(PermissionOp::ClearAll));
        assert!(matches!(&r[1], Reply::Script(s) if s.contains("window.onAllPermissionsCleared && window.onAllPermissionsCleared()")));
        // Routing itself changes no row: the app applies the operation.
        assert_eq!(db().all_permissions().len(), 2);
    }

    #[test]
    fn permission_messages_are_closed_and_typed() {
        let db = db();
        let go = |source: &str, handler: &str, body: serde_json::Value| route(&db, &json!({"handler": handler, "body": body}).to_string(), source);
        let revoke = json!({"action": "revokePermission", "origin": "https://a.b", "permissionType": "camera"});
        assert!(!go(SRC, SRC, revoke.clone()).is_empty());
        for (source, handler) in [("iterm2-about:history", SRC), ("https://evil.test/", SRC), (SRC, "iterm2-about:history"), ("iterm2-about:history", "iterm2-about:history")] {
            let r = go(source, handler, revoke.clone());
            assert!(r.iter().all(|r| !matches!(r, Reply::Permission(_))), "{source} {handler}");
        }
        for bad in [
            json!({"action": "revokePermission", "origin": "https://a.b", "permissionType": "Camera"}),
            json!({"action": "revokePermission", "origin": "https://a.b", "permissionType": "x"}),
            json!({"action": "revokePermission", "origin": "https://a.b/path", "permissionType": "camera"}),
            json!({"action": "revokePermission", "origin": "https://A.B", "permissionType": "camera"}),
            json!({"action": "revokePermission", "origin": "https://a.b'); alert(1);//", "permissionType": "camera"}),
            json!({"action": "revokePermission", "origin": 5, "permissionType": "camera"}),
            json!({"action": "revokePermission", "origin": "https://a.b"}),
            json!({"action": "revokePermission", "origin": "https://a.b", "permissionType": "camera", "extra": 1}),
            json!({"action": "revokeAllPermissions", "origin": "javascript:alert(1)"}),
            json!({"action": "revokeAllPermissions"}),
            json!({"action": "clearAllPermissions", "origin": "https://a.b"}),
            json!({"action": "loadPermissions", "offset": "x"}),
            json!({"action": "loadPermissions", "limit": 1000}),
            json!({"action": "loadPermissions", "surprise": true}),
            json!({"action": "unknown"}),
            json!({}),
        ] {
            assert!(go(SRC, SRC, bad.clone()).is_empty(), "{bad}");
        }
        // A pane that is not the permissions page gets nothing (the app also checks its main frame).
        assert!(route(&db, &msg(json!({"action": "loadPermissions"})), "iterm2-about:welcome").is_empty());
    }
}

#[cfg(test)]
mod plumbing_tests {
    use super::*;

    #[test]
    fn glyph_page_follows_the_ui_glyph_ruling() {
        assert_eq!(glyph_page("<h3>\u{1F512} /dev/null Mode Active</h3>"), "<h3><span class=\"tandem-glyph\">\u{E72E}</span> /dev/null Mode Active</h3>");
        assert_eq!(glyph_page("a::before { content: \"\u{1F4C1}\"; }"), "a::before { content: \"\u{E8B7}\"; font-family: \"Segoe Fluent Icons\"; }");
        assert_eq!(glyph_page("\u{26A0}\u{FE0F}"), "<span class=\"tandem-glyph\">\u{E7BA}</span>");
        for kept in ["\u{26A0}", "\u{2713}", "\u{2630}"] {
            assert_eq!(glyph_page(kept), kept);
        }
        for (emoji, glyph) in GLYPHS {
            let out = glyph_page(&format!("<b>{emoji}</b>"));
            assert_eq!(out, format!("<b><span class=\"tandem-glyph\">{glyph}</span></b>"), "{emoji:?}");
        }
        // A selector after an emoji whose entry has none goes with it.
        assert_eq!(glyph_page("\u{1F512}\u{FE0F}"), "<span class=\"tandem-glyph\">\u{E72E}</span>");
        assert!(page("{{COMMON_CSS}}x", &[]).starts_with(crate::browser_js::resource("iterm-browser-common.css").unwrap()));
        assert!(page("<style>{{INCLUDE:file-page.css}}</style>", &[]).contains(".directory-content"));
        assert_eq!(page("{{A}}", &[("A", "b")]), "b");
        assert!(GLYPH_CSS.contains("font-family: \"Segoe Fluent Icons\"") && GLYPH_CSS.contains(".tandem-glyph"));
        assert!(document("iterm-browser-common.css", "n").unwrap().0.contains(".tandem-glyph"));
        assert!(page("{{COMMON_CSS}}", &[]).contains(".tandem-glyph"), "a page that inlines the common CSS draws its glyph spans");
        assert_eq!(page("<p>{{U}}</p>", &[("U", "\u{1F512}")]), "<p>\u{1F512}</p>", "a substituted value is never rewritten");
        // The CSS content of an included file is rewritten, too.
        let css = page("{{INCLUDE:file-page.css}}", &[]);
        assert!(!css.contains('\u{1F4C1}') && !css.contains('\u{2B06}') && css.contains("content: \"\u{E8B7}\"; font-family: \"Segoe Fluent Icons\";"));
    }

    #[test]
    fn static_pages_follow_iterm2() {
        let intro = static_page("iterm2-about:onboarding-intro").unwrap();
        assert!(intro.contains("Meet the Tandem Browser") && !intro.contains("iTerm2"));
        let features = static_page("iterm2-about:onboarding-features").unwrap();
        assert!(!features.contains("iTerm2") && features.contains("Tandem"));
        for none in ["iterm2-about:dev", "iterm2-about:notifications-demo", "iterm2-about:nope", "https://a/"] {
            assert!(static_page(none).is_none(), "{none}");
        }
        // Served through the one nonce path: the policy header, no inline handler, glyphs drawn.
        for name in ["onboarding-intro", "onboarding-features"] {
            let url = format!("iterm2-about:{name}");
            assert_eq!(page_name(&url), Some(name));
            let Answer::Respond { status, mime, body, headers } = answer(&url, CONTEXT_DOCUMENT, "") else { panic!() };
            let body = String::from_utf8(body).unwrap();
            assert_eq!((status, mime.as_str()), (200, "text/html; charset=utf-8"));
            assert!(headers.iter().any(|(k, v)| k == "Content-Security-Policy" && v.contains("frame-ancestors 'none'") && v.contains("script-src 'nonce-")));
            assert!(!body.contains("onclick") && !body.contains("<script") && !body.contains("{{") && !body.contains('\u{1F310}') && !body.contains('\u{2728}'), "{name}");
            assert!(body.contains("class=\"tandem-glyph\""));
        }
        // The copies hold iTerm2's text; the changes are load-time, exactly once, and fail closed.
        let raw = crate::browser_js::resource("onboarding-intro.html").unwrap();
        assert!(raw.contains("iTerm2 Browser") && raw.contains('\u{1F310}'));
        assert!(crate::view_source::apply_patches(&raw.replace("Meet the iTerm2 Browser", "x"), &INTRO_PATCHES).is_err());
        assert!(crate::view_source::apply_patches(&format!("{raw}Meet the iTerm2 Browser"), &INTRO_PATCHES).is_err(), "a second copy of a target is a miss, too");
    }

    const IMG: i32 = 3;

    #[test]
    fn page_requests_follow_the_main_frame_navigation() {
        let url = "iterm2-file:///C:/T/site/a.html";
        let root = page_root(url).unwrap();
        let pending = Some((&root, url));
        let none: Option<(&PageRoot, &str)> = None;
        assert!(page_request_allowed(pending, None, "", url, CONTEXT_DOCUMENT));
        assert!(!page_request_allowed(pending, None, "", "iterm2-file:///C:/T/site/b.html", CONTEXT_DOCUMENT), "a document nobody navigated to");
        assert!(!page_request_allowed(none, None, "", url, CONTEXT_DOCUMENT));
        // A subresource: the committed root, and a top document of that root.
        assert!(page_request_allowed(None, Some(&root), url, "iterm2-file:///C:/T/site/i.png", IMG));
        assert!(!page_request_allowed(None, Some(&root), url, "iterm2-file:///C:/T/x/i.png", IMG));
        // Never committed: a web page holds the main frame while the root is the navigation's (or stale).
        assert!(!page_request_allowed(pending, None, "https://evil.example/", "iterm2-file:///C:/T/site/i.png", IMG), "no committed root");
        assert!(!page_request_allowed(None, Some(&root), "https://evil.example/", "iterm2-file:///C:/T/site/i.png", IMG), "a web top document");
        assert!(!page_request_allowed(None, Some(&root), "iterm2-file:///C:/T/other/a.html", "iterm2-file:///C:/T/site/i.png", IMG), "a top document of another folder");
        assert!(!page_request_allowed(None, Some(&root), "", "iterm2-file:///C:/T/site/i.png", IMG));
    }

    #[test]
    fn a_file_root_names_a_drive() {
        for bad in ["iterm2-file:///", "iterm2-file:///C:", "iterm2-file:///a.html", "iterm2-file:////a.html"] {
            assert_eq!(page_root(bad), None, "{bad}");
        }
        assert!(page_root("iterm2-file:///C:/a.html").is_some_and(|r| r.folder == "C:/"));
    }

    #[test]
    fn pages_are_sandboxed() {
        assert_eq!(sandbox_headers(), vec![("Content-Security-Policy".to_string(), SANDBOX_CSP.to_string())]);
        assert!(SANDBOX_CSP.starts_with("sandbox allow-scripts;") && SANDBOX_CSP.contains("connect-src 'none'") && SANDBOX_CSP.contains("form-action 'none'"));
        for never in ["allow-same-origin", "allow-popups", "allow-forms", "allow-top-navigation"] {
            assert!(!SANDBOX_CSP.contains(never), "{never}");
        }
        let file = PageRoot { scheme: "iterm2-file", host: None, folder: "C:/T/site/".to_string() };
        assert_eq!(page_root("iterm2-file:///C:/T/site/a.html"), Some(file.clone()));
        assert_eq!(page_root("iterm2-file:///C:/T/site/"), Some(file.clone()));
        assert_eq!(page_root("https://a/"), None);
        assert_eq!(page_root("iterm2-file:///C:/T/site/../a.html"), None);
        assert_eq!(page_root("iterm2-file:///C:/T/s%5Cx/a.html"), None);
        assert_eq!(page_root("iterm2-file:///C:/T/s%00x/a.html"), None);
        assert_eq!(page_root("iterm2-file://host/C:/T/a.html"), None, "a file page has no host");
        let r = Some(&file);
        for (url, ok) in [
            ("iterm2-file:///C:/T/site/img/a.png", true),
            ("iterm2-file:///C:/T/other/secret.txt", false),
            ("iterm2-file:///C:/T/site/../other/secret.txt", false),
            ("iterm2-file:///C:/T/site/%2E%2E/other/secret.txt", false),
            ("iterm2-file:///C:/T/site/..%5Cother%5Csecret.txt", false),
            ("iterm2-file:///C:/T/sitex/a.png", false),
            ("iterm2-ssh://box/C:/T/site/a.png", false),
            ("http://127.0.0.1/x", false),
            ("iterm2-file:///C:/T/site/a%00.png", false),
        ] {
            assert_eq!(subresource_allowed(r, url, IMG), ok, "{url}");
        }
        for (ctx, ok) in [(CONTEXT_FETCH, false), (CONTEXT_XML_HTTP_REQUEST, false), (CONTEXT_WEBSOCKET, false), (CONTEXT_EVENT_SOURCE, false), (CONTEXT_SCRIPT, true)] {
            assert_eq!(subresource_allowed(r, "iterm2-file:///C:/T/site/a.js", ctx), ok, "{ctx}");
        }
        assert!(!subresource_allowed(None, "iterm2-file:///C:/T/site/a.png", IMG));
        let ssh = page_root("iterm2-ssh://box/home/u/a.html");
        assert_eq!(ssh, Some(PageRoot { scheme: "iterm2-ssh", host: Some("box".to_string()), folder: "/home/u/".to_string() }));
        let s = ssh.as_ref();
        for (url, ok) in [
            ("iterm2-ssh://box/home/u/i/a.png", true),
            ("iterm2-ssh://BOX/home/u/a.png", true),
            ("iterm2-ssh://other/home/u/a.png", false),
            ("iterm2-ssh://box/etc/passwd", false),
            ("iterm2-file:///C:/a.png", false),
            ("iterm2-ssh://box/home/u/../../etc/passwd", false),
        ] {
            assert_eq!(subresource_allowed(s, url, IMG), ok, "{url}");
        }
    }
}

#[cfg(test)]
mod error_tests {
    use super::*;

    #[test]
    fn error_pages_follow_iterm2() {
        for (status, want) in [(13, Failure::HostNotFound), (7, Failure::TimedOut), (6, Failure::CannotConnect), (12, Failure::CannotConnect), (9, Failure::ConnectionLost), (10, Failure::ConnectionLost), (11, Failure::Offline), (15, Failure::TooManyRedirects), (1, Failure::Certificate(1)), (2, Failure::Certificate(2)), (3, Failure::Certificate(3)), (4, Failure::Certificate(4)), (5, Failure::Certificate(5)), (14, Failure::Cancelled), (16, Failure::Other(16)), (0, Failure::Other(0)), (17, Failure::Other(17))] {
            assert_eq!(failure_of(status), want, "{status}");
        }
        assert_eq!(error_texts(Failure::HostNotFound), Some(("Server Not Found", "<strong>Tandem can\u{2019}t find the server. Check that the web address is correct and try again.</strong><br/><br/>The host name could not be resolved.".to_string())));
        let (t, m) = error_texts(Failure::Offline).unwrap();
        assert!(t == "No Internet Connection" && m.contains("Your computer appears to be offline. Check your internet connection and try again."));
        assert_eq!(error_texts(Failure::TimedOut).unwrap().0, "The Connection Timed Out");
        let (t, m) = error_texts(Failure::CannotConnect).unwrap();
        assert!(t == "Can\u{2019}t Connect to Server" && m.contains("Tandem can\u{2019}t establish a secure connection to the server. The server may be down or unreachable."));
        assert_eq!(error_texts(Failure::ConnectionLost).unwrap().0, "Network Connection Lost");
        let (t, m) = error_texts(Failure::TooManyRedirects).unwrap();
        assert!(t == "Too Many Redirects" && m.contains("Tandem can\u{2019}t open the page because the server redirected too many times."));
        let (t, m) = error_texts(Failure::Certificate(2)).unwrap();
        assert_eq!(t, "Secure Connection Failed");
        assert_eq!(m, "<strong>Tandem can\u{2019}t verify the identity of the website. The connection may not be secure.</strong><br/><br/>The certificate is expired.");
        assert!(error_texts(Failure::Certificate(1)).unwrap().1.ends_with("The hostname does not match the certificate."));
        assert!(error_texts(Failure::Certificate(5)).unwrap().1.ends_with("The presented chain is not valid (e.g., self-signed without trust)."));
        let (t, m) = error_texts(Failure::Other(16)).unwrap();
        assert!(t == "Page Can\u{2019}t Be Loaded" && m.contains(&format!("An error occurred while loading this page. {}", describe(16))));
        assert_eq!(error_texts(Failure::Cancelled), None);

        let page = error_page(Failure::HostNotFound, Some("https://a.b/"));
        assert!(page.contains("<div class=\"error-url\">https://a.b/</div>") && page.contains("const originalURL = 'https://a.b/';"));
        assert!(!error_page(Failure::HostNotFound, None).contains("error-url\">"));
        assert!(default_error_page().contains("<title>Page Unavailable</title>"));
        // A hostile URL cannot leave its slots.
        let bad = "https://a/'</script><img src=x onerror=alert(1)>{{NONCE}}\u{2028}\\";
        let page = error_page(Failure::HostNotFound, Some(bad));
        assert!(!page.contains("</script><img") && !page.contains("<img src=x") && !page.contains("onerror=alert(1)>"));
        assert!(page.contains("const originalURL = 'https://a/\\u0027\\u003c/script\\u003e"), "{page}");
        assert_eq!(page.matches("{{NONCE}}").count(), 1, "only the script's own nonce slot is left");
        assert_eq!(page.matches("<script").count(), 1);
        // Glyphs are not applied to a substituted value (review I9).
        let page = error_page(Failure::HostNotFound, Some("https://a/\u{1F512}"));
        assert!(page.contains("const originalURL = 'https://a/\u{1F512}';") && !page.contains("originalURL = 'https://a/<span"));
        let d = described_error_page("Invalid file path: C:\\x", Some("file:///C:/x"));
        assert!(d.contains("<title>Page Can\u{2019}t Be Loaded</title>"));
        assert!(d.contains("<strong>An error occurred while loading this page. Invalid file path: C:\\x</strong><br/><br/>Invalid file path: C:\\x"));
        assert!(described_error_page("<b>x</b>", None).contains("&lt;b&gt;x&lt;/b&gt;"));
    }

    #[test]
    fn a_server_that_answered_keeps_its_page() {
        // Task 2 review I1: a 404 or 500 fails with IsSuccess false but a response; no error page.
        // The engine's real values for a 404, a 500 and a 204 need the runtime probe.
        for http in [400, 401, 404, 500, 503, 204, 100] {
            assert!(server_answered(http), "{http}");
        }
        for http in [0, -1, 99] {
            assert!(!server_answered(http), "{http}: no response, the error page");
        }
    }

    #[test]
    fn a_description_never_carries_a_fill_key() {
        // Task 2 review M2: a description (a path, a remote error) holding a key is shown as text and
        // the nonce lands only in the script.
        let html = described_error_page("x {{NONCE}} {{TITLE}}", Some("https://a.test/{{NONCE}}"));
        assert!(!html.is_empty());
        let served = with_nonce(&html, "N0NCE");
        assert!(!html.contains("x {{NONCE}}") && html.contains("x &#123;&#123;NONCE}} &#123;&#123;TITLE}}"));
        let in_script = served.matches("N0NCE").count();
        assert_eq!(in_script, html.matches("{{NONCE}}").count(), "only the template's own keys are filled");
        assert!(!served.contains("x N0NCE"));
    }

    #[test]
    fn error_loop_guard_follows_iterm2() {
        assert!(should_show_error(Some("u"), None));
        assert!(!should_show_error(Some("u"), Some("u")));
        assert!(should_show_error(Some("u"), Some("v")));
        assert!(!should_show_error(None, None));
        let mut b = ErrorBurst::default();
        assert!((0..5).all(|i| b.allow(i * 100)));
        assert!(!b.allow(900), "a sixth in ten seconds");
        assert!(b.allow(10_100), "the window moves on");
    }

    /// The page is the copied template, patched exact-once; its script has the nonce, no inline
    /// handler stays, and a click or key counts only when the person made it.
    #[test]
    fn the_error_page_is_a_patched_copy_under_the_nonce_policy() {
        let raw = crate::browser_js::resource("error-page.html").unwrap();
        for (name, target, _) in ERROR_PATCHES {
            assert_eq!(raw.matches(target).count(), 1, "{name}");
            assert!(crate::view_source::apply_patches(&format!("{raw}{target}"), &ERROR_PATCHES).is_err(), "a second copy of {name} is a miss");
        }
        let html = with_nonce(&error_page(Failure::TimedOut, Some("https://a.b/")), "n0nce");
        assert!(html.contains("<script nonce=\"n0nce\">") && !html.contains("onclick") && html.matches("<script").count() == 1);
        assert!(html.contains("event.isTrusted") && html.contains("window.location.href = originalURL"), "Try Again is a plain navigation: the one verdict judges it");
        assert!(!html.contains("{{"), "every slot filled");
        let headers = response_headers("n0nce");
        assert!(headers.iter().any(|(k, v)| k == "Content-Security-Policy" && v.contains("frame-ancestors 'none'") && v.contains("script-src 'nonce-n0nce'")));
    }

    #[test]
    fn a_long_url_keeps_the_end_of_its_host() {
        let long_path = format!("https://example.com/{}", "p".repeat(2000));
        let s = shown_url(&long_path);
        assert!(s.starts_with("https://example.com/") && s.chars().count() <= SHOWN_URL_MAX && s.ends_with('\u{2026}'));
        let long_host = format!("https://{}.evil.example/x", "u".repeat(900));
        let s = shown_url(&long_host);
        assert!(s.chars().count() <= SHOWN_URL_MAX && s.ends_with(".evil.example") && s.starts_with('\u{2026}'), "{s}");
        assert_eq!(shown_url("https://a\u{202E}b/"), "https://ab/", "hidden characters go");
        // Too long to retry: no target, never a cut URL.
        let page = error_page(Failure::TimedOut, Some(&format!("https://a/{}", "p".repeat(RETRY_URL_MAX))));
        assert!(page.contains("const originalURL = '';"));
        let page = error_page(Failure::TimedOut, Some("javascript:alert(1)"));
        assert!(page.contains("const originalURL = '';"), "only a scheme Tandem loads is retried");
    }
}
