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
fn new_nonce() -> String {
    uuid::Uuid::new_v4().simple().to_string()
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
}

const COMMON_CSS: &str = include_str!("../resources/browser/iterm-browser-common.css");

/// The page a URL names, by exact name: the scheme compared ignoring ASCII case, the name one of
/// a closed list (no query, fragment, slash, `..`, escape, NUL or long name; P8).
pub fn page_name(url: &str) -> Option<&str> {
    let (scheme, rest) = url.split_once(':')?;
    if !scheme.eq_ignore_ascii_case("iterm2-about") || rest.is_empty() || rest.len() > 64 {
        return None;
    }
    matches!(rest, "welcome" | "history" | "bookmarks" | "iterm-browser-common.css").then_some(rest)
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
        "iterm-browser-common.css" => (COMMON_CSS.to_string(), "text/css; charset=utf-8"),
        _ => return None,
    })
}

fn response_headers(nonce: &str) -> Vec<(String, String)> {
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
