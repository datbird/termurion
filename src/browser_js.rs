//! The page-script host's pure model (milestone 13x2, Task 1): templates and includes, the bridge
//! rewrite that lets iTerm2's copied scripts post through `window.webkit.messageHandlers`, the
//! wrapping of a user script, the routing of a posted message, async calls into the isolated world,
//! the gate every page-derived URL passes before Tandem opens it, and coordinate math. Web content
//! is untrusted: the world of a message comes from the transport that carried it (the DevTools
//! binding of Tandem's isolated world, or WebMessageReceived), never from its JSON, and nothing
//! here holds or sends a secret in an envelope.

use serde_json::Value;

use crate::semantic::{self, Run};
use crate::webhost::{MAX_MESSAGE, scheme_of};

// --- Resources and templates ---

/// Every file of `resources/browser/` (a later plan that copies a file adds a line).
pub fn resource(name: &str) -> Option<&'static str> {
    Some(match name {
        "bookmarks-page.html" => include_str!("../resources/browser/bookmarks-page.html"),
        "bookmarks-page.js" => include_str!("../resources/browser/bookmarks-page.js"),
        "challenge-frame-detection.js" => include_str!("../resources/browser/challenge-frame-detection.js"),
        "graph-discovery.js" => include_str!("../resources/browser/graph-discovery.js"),
        "history-page.html" => include_str!("../resources/browser/history-page.html"),
        "history-page.js" => include_str!("../resources/browser/history-page.js"),
        "hover-link-detector.js" => include_str!("../resources/browser/hover-link-detector.js"),
        "iterm-browser-common.css" => include_str!("../resources/browser/iterm-browser-common.css"),
        "welcome-page.html" => include_str!("../resources/browser/welcome-page.html"),
        "welcome-page.js" => include_str!("../resources/browser/welcome-page.js"),
        _ => return None,
    })
}

/// iTerm2's template loader (`TL:29-73`): `{{COMMON_CSS}}` first, then every `{{INCLUDE:name}}`,
/// then each `{{KEY}}` of `subs`. A text that was put in is never read again for a later key. A
/// missing include is iTerm2's `it_fatalError` in a debug build and stays in place in a release build.
pub fn fill_template(src: &str, subs: &[(&str, &str)]) -> String {
    let css = resource("iterm-browser-common.css").unwrap_or_default();
    let with_css = src.replace("{{COMMON_CSS}}", css);
    // Includes, left to right, each replaced by the file's text as it is (not scanned again).
    let mut included = String::with_capacity(with_css.len());
    let mut rest = with_css.as_str();
    while let Some(at) = rest.find("{{INCLUDE:") {
        included.push_str(&rest[..at]);
        let tail = &rest[at + "{{INCLUDE:".len()..];
        match tail.find("}}").filter(|end| !tail[..*end].contains('}')) {
            Some(end) => {
                let name = &tail[..end];
                match resource(name) {
                    Some(text) => included.push_str(text),
                    None => {
                        if cfg!(debug_assertions) {
                            panic!("Could not load included file {name}");
                        }
                        included.push_str(&rest[at..at + "{{INCLUDE:".len() + end + 2]);
                    }
                }
                rest = &tail[end + 2..];
            }
            None => {
                included.push_str("{{INCLUDE:");
                rest = tail;
            }
        }
    }
    included.push_str(rest);
    // Keys, left to right.
    let mut out = String::with_capacity(included.len());
    let mut rest = included.as_str();
    while let Some(at) = rest.find("{{") {
        out.push_str(&rest[..at]);
        let tail = &rest[at + 2..];
        match tail.find("}}") {
            Some(end) => {
                let key = &tail[..end];
                match subs.iter().find(|(k, _)| *k == key) {
                    Some((_, value)) => out.push_str(value),
                    None => {
                        out.push_str("{{");
                        out.push_str(key);
                        out.push_str("}}");
                    }
                }
                rest = &tail[end + 2..];
            }
            None => {
                out.push_str("{{");
                rest = tail;
            }
        }
    }
    out.push_str(rest);
    out
}

/// A fresh per-view secret: 16 random bytes as 32 lowercase hex characters (`String+iTerm.swift:401`).
pub fn new_secret() -> String {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).expect("the system random source failed");
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

// --- Scripts ---

/// iTerm2's two content worlds: `.page` and `.defaultClient` (Tandem's isolated world).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum World {
    Page,
    Client,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Inject {
    DocumentStart,
    DocumentEnd,
}

#[derive(Clone, Debug, PartialEq)]
pub struct UserScript {
    pub id: &'static str,
    pub code: String,
    pub inject: Inject,
    pub main_frame_only: bool,
    pub world: World,
}

fn is_ident(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'$' || b >= 0x80
}

/// `window.webkit` and `webkit.messageHandlers` become the closure-local `__tandemWebkit`, at
/// identifier boundaries only: not after `.` or an identifier character, not before one, so
/// `window.webkitAudioContext`, strings and properties are untouched.
fn rewrite_bridge(code: &str) -> String {
    let b = code.as_bytes();
    let mut out = String::with_capacity(code.len());
    let (mut last, mut i) = (0, 0);
    while i < b.len() {
        let before_ok = i == 0 || !(is_ident(b[i - 1]) || b[i - 1] == b'.');
        if before_ok {
            let window = b[i..].starts_with(b"window.webkit") && b.get(i + 13).is_none_or(|c| !is_ident(*c));
            let bare = !window && b[i..].starts_with(b"webkit") && (b[i + 6..].starts_with(b".messageHandlers") || b[i + 6..].starts_with(b"?.messageHandlers"));
            if window || bare {
                out.push_str(&code[last..i]);
                out.push_str("__tandemWebkit");
                i += if window { 13 } else { 6 };
                last = i;
                continue;
            }
        }
        i += 1;
    }
    out.push_str(&code[last..]);
    out
}

const PAGE_CAPTURE: &str = "const __tandemPost = (window.chrome && window.chrome.webview) ? window.chrome.webview.postMessage.bind(window.chrome.webview) : null;";
const CLIENT_CAPTURE: &str = "const __tandemPost = globalThis.__tandemClient;";

/// The closure-local `webkit` object: a Proxy answering `{postMessage}` for any handler name.
fn bridge_object(world: World) -> &'static str {
    match world {
        World::Client => "const __tandemWebkit = { messageHandlers: new Proxy({}, { get: function (_t, n) { const name = String(n); return { postMessage: function (body) { if (typeof __tandemPost === 'function') { __tandemPost(JSON.stringify({h: name, b: body})); } } }; } }) };",
        World::Page => "const __tandemWebkit = { messageHandlers: new Proxy({}, { get: function (_t, n) { const name = String(n); return { postMessage: function (body) { if (__tandemPost) { __tandemPost({h: name, b: body}); } } }; } }) };",
    }
}

fn capture(world: World) -> &'static str {
    match world {
        World::Client => CLIENT_CAPTURE,
        World::Page => PAGE_CAPTURE,
    }
}

/// A user script as it is injected: its own function (two scripts in one world never collide), the
/// transport captured before the body runs, the bridge rewritten, then the main-frame guard and the
/// document-end wait. A Client script posts `JSON.stringify({h, b})` to the isolated world's binding;
/// a Page script posts `{h, b}` through `window.chrome.webview` (a null post is a no-op).
pub fn wrap(script: &UserScript) -> String {
    let body = rewrite_bridge(&script.code);
    let mut s = String::from("(function () {\n");
    s.push_str(capture(script.world));
    s.push('\n');
    if script.main_frame_only {
        s.push_str("if (window.top !== window) return;\n");
    }
    s.push_str(bridge_object(script.world));
    s.push('\n');
    match script.inject {
        Inject::DocumentStart => {
            s.push_str(&body);
            s.push('\n');
        }
        Inject::DocumentEnd => {
            s.push_str("const __tandemRun = function () {\n");
            s.push_str(&body);
            s.push_str("\n};\nif (document.readyState === 'loading') { document.addEventListener('DOMContentLoaded', __tandemRun, {once: true}); } else { __tandemRun(); }\n");
        }
    }
    s.push_str("})();");
    s
}

/// What an `iterm2-about:` page needs: iTerm2's local pages call `window.webkit.messageHandlers[...]`
/// themselves, so `window.webkit` is defined (not configurable) on those pages only, by the same
/// bridge text every script uses. Any other page never has it.
pub fn local_page_script() -> UserScript {
    UserScript {
        id: "local-page-bridge",
        code: "if (location.protocol === 'iterm2-about:') { Object.defineProperty(window, 'webkit', { value: window.webkit, configurable: false }); }".into(),
        inject: Inject::DocumentStart,
        main_frame_only: true,
        world: World::Page,
    }
}

// --- Routing ---

#[derive(Clone, Debug, PartialEq)]
pub struct Routed {
    pub handler: String,
    pub world: World,
    /// None is the main frame.
    pub frame: Option<u64>,
    /// The posting document's URL (a Page message) or the origin of its Tandem context (a Client one).
    pub source: String,
    pub body: Value,
}

const MAX_HANDLER: usize = 128;

/// One message: `{h, b}` (the bridge) or `{handler, body}` (a local page's own form). The world is
/// the transport's: the JSON never names it. Oversize, malformed, or a handler that is empty, long
/// or holds a control character is None.
pub fn route(json: &str, world: World, frame: Option<u64>, source: &str) -> Option<Routed> {
    if json.len() > MAX_MESSAGE {
        return None;
    }
    let Value::Object(mut o) = serde_json::from_str::<Value>(json).ok()? else { return None };
    let (h, b) = if o.contains_key("h") { ("h", "b") } else { ("handler", "body") };
    let handler = o.get(h)?.as_str()?.to_string();
    if handler.is_empty() || handler.len() > MAX_HANDLER || handler.chars().any(char::is_control) {
        return None;
    }
    let body = o.remove(b).unwrap_or(Value::Null);
    Some(Routed { handler, world, frame, source: source.to_string(), body })
}

/// A local page's message: the Page world, the main frame, and an `iterm2-about:` source.
pub fn local_page_ok(r: &Routed) -> bool {
    r.world == World::Page && r.frame.is_none() && crate::browser::local_source_ok(&r.source)
}

// --- Calls ---

/// A JSON string literal, for a value put into a script.
pub fn js_string(text: &str) -> String {
    Value::String(text.to_string()).to_string()
}

fn identifier_ok(name: &str) -> bool {
    let b = name.as_bytes();
    !b.is_empty() && !b.starts_with(b"__tandem") && (b[0].is_ascii_alphabetic() || b[0] == b'_' || b[0] == b'$') && b.iter().all(|c| c.is_ascii_alphanumeric() || *c == b'_' || *c == b'$')
}

/// iTerm2's `callAsyncJavaScript` (`BWV:140-149`): each argument a local constant, the body awaited.
/// The expression is for `Runtime.evaluate` with `awaitPromise` and `returnByValue`; it holds no secret.
/// A body that posts through the bridge gets the Client rewrite, the binding captured at its top.
pub fn call_js(body: &str, args: &Value) -> Result<String, String> {
    let mut decls = String::new();
    match args {
        Value::Null => {}
        Value::Object(m) => {
            for (k, v) in m {
                if !identifier_ok(k) {
                    return Err("bad argument name".into());
                }
                decls.push_str(&format!("const {k} = {v}; "));
            }
        }
        _ => return Err("bad argument name".into()),
    }
    let rewritten = rewrite_bridge(body);
    let prefix = if rewritten != body { format!("{CLIENT_CAPTURE} {} ", bridge_object(World::Client)) } else { String::new() };
    Ok(format!("(async () => {{ {prefix}{decls}{rewritten} }})()"))
}

/// A synchronous script for the PAGE world, run with no bridge and no argument (iTerm2's
/// `evaluateJavaScript` in `.page`); the answer is page-controlled data.
pub fn page_call_script(body: &str, args: &Value) -> Result<String, String> {
    let empty = match args {
        Value::Null => true,
        Value::Object(m) => m.is_empty(),
        _ => false,
    };
    if !empty {
        return Err("page calls take no arguments".into());
    }
    Ok(format!("(function() {{\n{body}\n}})()"))
}

/// `makeSafe` (`BWV:151-160`): an `undefined` result is null (a crash on some macOS versions; here
/// the JSON answer would be empty).
pub fn make_safe(script: &str) -> String {
    format!("(() => {{\n    let __result__ = (function() {{\n        return {script}\n    }})();\n    return typeof __result__ === 'undefined' ? null : __result__;\n}})()")
}

/// `iife` (`BWV:1051-1053`).
pub fn iife(body: &str) -> String {
    format!("(function() {{{body}}})();")
}

// --- Coordinates (WVA:96-123) ---

/// View pixels to page coordinates: over the monitor scale and the page zoom.
pub fn to_js(px: (f64, f64), scale: f64, zoom: f64) -> (f64, f64) {
    (px.0 / scale / zoom, px.1 / scale / zoom)
}

pub fn from_js(js: (f64, f64), scale: f64, zoom: f64) -> (f64, f64) {
    (js.0 * zoom * scale, js.1 * zoom * scale)
}

// --- The gate ---

#[derive(Clone, Debug, PartialEq)]
pub enum NavGate {
    Navigate(String),
    Open(Run),
    Blocked(String),
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let (mut out, mut i) = (Vec::with_capacity(b.len()), 0);
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() && b[i + 1].is_ascii_hexdigit() && b[i + 2].is_ascii_hexdigit() {
            out.push(u8::from_str_radix(&s[i + 1..i + 3], 16).unwrap_or(b'%'));
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

/// The network rule every open passes first, for the browser and the terminal alike: a `file:` URL
/// whose host is neither empty nor `localhost`, or whose percent-decoded path is a network or device
/// path, and any bare target `scripts::is_network_path` accepts. Nothing is probed.
pub fn gate_refusal(target: &str) -> Option<String> {
    let t = target.trim_start();
    let refuse = || Some(format!("Tandem does not open network file links: {t}"));
    if scheme_of(t) == "file" {
        let rest = &t["file:".len()..];
        let path = match rest.strip_prefix("//") {
            Some(r) => {
                let (host, path) = r.split_at(r.find('/').unwrap_or(r.len()));
                if !(host.is_empty() || host.eq_ignore_ascii_case("localhost")) {
                    return refuse();
                }
                path
            }
            None => rest,
        };
        if crate::scripts::is_network_path(&percent_decode(path)) || crate::scripts::is_network_path(path) {
            return refuse();
        }
        return None;
    }
    if crate::scripts::is_network_path(t) { refuse() } else { None }
}

/// The scheme a URL parser would read: leading control characters and spaces go, and so do tabs and
/// line breaks anywhere, whatever the case.
fn dangerous_scheme(url: &str) -> Option<&'static str> {
    let cleaned: String = url.chars().filter(|c| !matches!(c, '\t' | '\n' | '\r')).collect();
    let lower = cleaned.trim_start_matches(|c: char| c <= ' ').to_ascii_lowercase();
    ["javascript", "data", "vbscript"].into_iter().find(|s| lower.starts_with(&format!("{s}:")))
}

fn plain_navigation(t: &str) -> bool {
    matches!(scheme_of(t).as_str(), "http" | "https" | "iterm2-about") || t.eq_ignore_ascii_case("about:blank")
}

/// What Tandem does with a URL a page, page text or an arrangement supplied (I1). `http`, `https`,
/// `iterm2-about:` and `about:blank` navigate; script-bearing schemes and network targets are refused;
/// everything else, EVERY `file:` target included, goes through `semantic::guard_open` (a local file
/// opens in its default app or is revealed, never loaded into the pane: the 13x verdict is the one
/// navigation rule).
pub fn nav_gate(url: &str) -> NavGate {
    let t = url.trim_start_matches(|c: char| c <= ' ');
    if let Some(s) = dangerous_scheme(t) {
        return NavGate::Blocked(format!("Tandem does not open {s}: links."));
    }
    if let Some(why) = gate_refusal(t) {
        return NavGate::Blocked(why);
    }
    if plain_navigation(t) {
        return NavGate::Navigate(t.to_string());
    }
    NavGate::Open(semantic::guard_open(t, false))
}

/// A new tab or window a page asked for (`NewWindow`): the same, but `file:` is refused outright (a
/// page asked, not the person), and no banner is meant for a refusal.
pub fn nav_gate_tab(url: &str) -> NavGate {
    let t = url.trim_start_matches(|c: char| c <= ' ');
    if scheme_of(t) == "file" || t.get(..5).is_some_and(|p| p.eq_ignore_ascii_case("file:")) {
        return NavGate::Blocked("Tandem does not open file: links from a page.".into());
    }
    nav_gate(url)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::semantic::{Run, guard_open};
    use serde_json::json;

    #[test]
    fn template_follows_iterm2() {
        assert_eq!(fill_template("a {{SECRET}} b {{SECRET}}", &[("SECRET", "s1")]), "a s1 b s1");
        assert_eq!(fill_template("x {{X}} y", &[("SECRET", "s1")]), "x {{X}} y", "an unknown key stays");
        // An include is the resource's whole text, and keys inside it are filled (includes first).
        let inc = resource("challenge-frame-detection.js").expect("the resource");
        let out = fill_template("[{{INCLUDE:challenge-frame-detection.js}}]", &[]);
        assert_eq!(out, format!("[{inc}]"));
        let css = resource("iterm-browser-common.css").unwrap();
        assert_eq!(fill_template("<style>{{COMMON_CSS}}</style>", &[]), format!("<style>{css}</style>"));
        // The text of a filled value is never read again for another key.
        assert_eq!(fill_template("{{A}}", &[("A", "{{B}}"), ("B", "no")]), "{{B}}");
        assert_eq!(fill_template("{{NONCE}}", &[("NONCE", "n0")]), "n0");
        // A key inside an included file is filled.
        let welcome = fill_template("{{INCLUDE:welcome-page.js}}", &[("SECRET", "ZZZ")]);
        assert!(welcome.contains("ZZZ") && !welcome.contains("{{SECRET}}"), "a key in an included file is filled");
        let a = new_secret();
        let b = new_secret();
        assert_eq!(a.len(), 32);
        assert!(a.bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)), "{a}");
        assert_ne!(a, b);
    }

    /// iTerm2's `it_fatalError` for a missing include: a panic in debug builds.
    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "Could not load included file nope.js")]
    fn a_missing_include_panics_in_debug() {
        let _ = fill_template("{{INCLUDE:nope.js}}", &[]);
    }

    fn script(code: &str, world: World) -> UserScript {
        UserScript { id: "t", code: code.into(), inject: Inject::DocumentStart, main_frame_only: false, world }
    }

    #[test]
    fn wrap_rewrites_the_bridge() {
        let code = "window.webkit.messageHandlers.iTermHoverLink.postMessage({a: 1})";
        let c = wrap(&script(code, World::Client));
        assert!(!c.contains("window.webkit"), "{c}");
        assert!(c.starts_with("(function () {\nconst __tandemPost = globalThis.__tandemClient;\n"), "the binding is captured before the body: {c}");
        assert!(c.contains("__tandemPost(JSON.stringify({h: name, b: body}))"), "{c}");
        assert!(!c.to_ascii_lowercase().contains("secret"), "no secret field in the envelope");
        assert!(c.contains("__tandemWebkit.messageHandlers.iTermHoverLink.postMessage({a: 1})"));
        let p = wrap(&script(code, World::Page));
        assert!(p.starts_with("(function () {\nconst __tandemPost = (window.chrome && window.chrome.webview) ? window.chrome.webview.postMessage.bind(window.chrome.webview) : null;\n"), "{p}");
        assert!(p.contains("__tandemPost({h: name, b: body})") && !p.contains("JSON.stringify({h"), "{p}");
        assert!(p.contains("if (__tandemPost)"), "a null post is a no-op");

        for (src, want) in [
            ("webkit.messageHandlers.x.postMessage(1)", "__tandemWebkit.messageHandlers.x.postMessage(1)"),
            ("window.webkit?.messageHandlers?.x?.postMessage({a: 1})", "__tandemWebkit?.messageHandlers?.x?.postMessage({a: 1})"),
            ("if (window.webkit && window.webkit.messageHandlers) {}", "if (__tandemWebkit && __tandemWebkit.messageHandlers) {}"),
            ("webkit?.messageHandlers.x", "__tandemWebkit?.messageHandlers.x"),
        ] {
            assert!(wrap(&script(src, World::Client)).contains(want), "{src}");
        }
        // Identifier boundaries: a longer name, a string and a property are left alone.
        for keep in ["window.webkitAudioContext", "'webkitAudioContext'", "RealWAC = window.webkitAudioContext", "x.webkit.messageHandlers", "mywindow.webkit", "style.webkitTransform", "'-webkit-box'", "window.webkit_x"] {
            let w = wrap(&script(keep, World::Client));
            assert!(w.contains(keep), "{keep} was changed: {w}");
        }
        assert!(!wrap(&script("RealWAC = window.webkitAudioContext", World::Client)).contains("RealWAC = __tandem"));
        // Main frame only, and document end.
        let mut s = script("run();", World::Client);
        s.main_frame_only = true;
        assert!(wrap(&s).contains("if (window.top !== window) return;"));
        s.main_frame_only = false;
        assert!(!wrap(&s).contains("window.top"));
        s.inject = Inject::DocumentEnd;
        let e = wrap(&s);
        assert!(e.contains("document.readyState === 'loading'") && e.contains("DOMContentLoaded") && e.contains("__tandemRun()"), "{e}");
        let start = wrap(&script("run();", World::Client));
        assert!(!start.contains("DOMContentLoaded"));
        // Two wrapped scripts never collide when they share one global (each is its own function).
        assert_eq!(start.matches("(function () {").count(), 1);
        assert!(start.trim_end().ends_with("})();"));
    }

    #[test]
    fn the_local_page_bridge_is_the_same_text() {
        let s = local_page_script();
        assert_eq!(s.world, World::Page);
        assert!(s.main_frame_only);
        let w = wrap(&s);
        assert!(w.contains("location.protocol === 'iterm2-about:'") && w.contains("Object.defineProperty(window, 'webkit'"), "{w}");
        assert!(w.contains("value: __tandemWebkit") && w.contains("configurable: false"), "{w}");
        assert!(w.contains("__tandemPost({h: name, b: body})"), "the same bridge text, posting {{h, b}}");
        assert!(!w.contains("secret"));
    }

    #[test]
    fn route_follows_the_transport() {
        let r = route(r#"{"h":"iTermHoverLink","b":{"x":1}}"#, World::Client, None, "https://a/").expect("routed");
        assert_eq!(r, Routed { handler: "iTermHoverLink".into(), world: World::Client, frame: None, source: "https://a/".into(), body: json!({"x": 1}) });
        let p = route(r#"{"h":"iTermHoverLink","b":{"x":1}}"#, World::Page, None, "https://a/").unwrap();
        assert_eq!(p.world, World::Page);
        // The JSON never picks the world.
        let stray = route(r#"{"h":"iTermHoverLink","b":{"x":1},"s":"abc","world":"Client"}"#, World::Page, None, "https://a/").unwrap();
        assert_eq!(stray.world, World::Page);
        assert_eq!(stray.body, json!({"x": 1}));
        let local = route(r#"{"handler":"iterm2-about:history","body":{"action":"clearAllHistory"}}"#, World::Page, None, "iterm2-about:history").unwrap();
        assert_eq!((local.handler.as_str(), local.world, local.body.clone()), ("iterm2-about:history", World::Page, json!({"action": "clearAllHistory"})));
        assert!(local_page_ok(&local));
        let evil = route(r#"{"handler":"iterm2-about:history","body":{"action":"clearAllHistory"}}"#, World::Page, None, "https://evil.invalid/").unwrap();
        assert!(!local_page_ok(&evil));
        let framed = route(r#"{"handler":"iterm2-about:history","body":{}}"#, World::Page, Some(7), "iterm2-about:history").unwrap();
        assert_eq!(framed.frame, Some(7));
        assert!(!local_page_ok(&framed));
        let client = route(r#"{"h":"x","b":1}"#, World::Client, None, "iterm2-about:history").unwrap();
        assert!(!local_page_ok(&client), "a Client message is never a local page's");
        for bad in ["not json", "[]", "{}", r#"{"h":5}"#, r#"{"h":null,"b":1}"#, r#"{"b":1}"#, r#"{"h":""}"#, r#"{"h":"a\u0000b"}"#] {
            assert!(route(bad, World::Client, None, "o").is_none(), "{bad}");
        }
        // A missing body is null; a long handler name or a huge message is refused.
        assert_eq!(route(r#"{"h":"x"}"#, World::Page, None, "o").unwrap().body, serde_json::Value::Null);
        assert!(route(&format!(r#"{{"h":"{}"}}"#, "a".repeat(200)), World::Page, None, "o").is_none());
        assert!(route(&format!(r#"{{"h":"x","b":"{}"}}"#, "a".repeat(crate::webhost::MAX_MESSAGE)), World::Page, None, "o").is_none());
    }

    #[test]
    fn call_js_follows_iterm2() {
        assert_eq!(call_js("return a + b;", &json!({"a": 1, "b": 2})).unwrap(), "(async () => { const a = 1; const b = 2; return a + b; })()");
        assert_eq!(call_js("return 1;", &json!({})).unwrap(), "(async () => { return 1; })()");
        assert_eq!(call_js("return 1;", &serde_json::Value::Null).unwrap(), "(async () => { return 1; })()");
        let s = call_js("return s;", &json!({"s": "x\"y\\z"})).unwrap();
        assert!(s.contains(r#"const s = "x\"y\\z";"#), "{s}");
        for bad in ["a b", "1a", "a-b", "", "a;b", "__tandemPost", "a.b"] {
            assert_eq!(call_js("return 1;", &json!({ bad: 1 })), Err("bad argument name".into()), "{bad}");
        }
        assert_eq!(call_js("return 1;", &json!([1])), Err("bad argument name".into()));
        // A body that posts gets the Client bridge, the binding captured at the top of the expression.
        for body in ["window.webkit?.messageHandlers?.x?.postMessage({a: 1}); return 1;", "window.webkit.messageHandlers.x.postMessage({a: 1}); return 1;", "webkit.messageHandlers.x.postMessage({a: 1}); return 1;"] {
            let e = call_js(body, &json!({"a": 1})).unwrap();
            assert!(e.starts_with("(async () => { const __tandemPost = globalThis.__tandemClient;"), "{e}");
            assert!(e.contains("__tandemPost(JSON.stringify({h: name, b: body}))") && !e.contains("window.webkit"), "{e}");
            assert!(!e.to_ascii_lowercase().contains("secret"));
        }
    }

    #[test]
    fn page_call_takes_no_arguments() {
        assert_eq!(page_call_script("return 1;", &json!({})).unwrap(), "(function() {\nreturn 1;\n})()");
        assert_eq!(page_call_script("return 1;", &serde_json::Value::Null).unwrap(), "(function() {\nreturn 1;\n})()");
        assert_eq!(page_call_script("return 1;", &json!({"a": 1})), Err("page calls take no arguments".into()));
        // No bridge, no secret: the body goes in as written.
        assert!(!page_call_script("window.webkit.x", &json!({})).unwrap().contains("__tandem"));
    }

    #[test]
    fn nav_gate_refuses_network_files() {
        for ok in ["https://x/", "http://x/y?z", "iterm2-about:history", "about:blank", "ABOUT:BLANK", "HTTPS://X/"] {
            assert_eq!(nav_gate(ok), NavGate::Navigate(ok.to_string()), "{ok}");
        }
        // B3: every file: target goes through guard_open, never Navigate (the 13x verdict is the one rule).
        for f in ["file:///C:/a/b.txt", "file://localhost/C:/a"] {
            assert_eq!(nav_gate(f), NavGate::Open(guard_open(f, false)), "{f}");
            assert!(matches!(nav_gate(f), NavGate::Open(Run::OpenLocal { .. })), "{f}");
        }
        let text = |u: &str| format!("Tandem does not open network file links: {u}");
        for net in ["file://evil/s/x", "file:////evil/s/x", "file://///evil/s/x", "\\\\evil\\s\\x", "//evil/s/x", "file:///%5C%5Cevil%5Cs%5Cx", "\\\\?\\UNC\\h\\s", "file:\\\\evil\\s"] {
            assert_eq!(nav_gate(net), NavGate::Blocked(text(net)), "{net}");
        }
        for bad in ["javascript:alert(1)", " JAVASCRIPT:x", "data:text/html,x", "vbscript:x", "\tjava\nscript:alert(1)", "DaTa:x"] {
            assert!(matches!(nav_gate(bad), NavGate::Blocked(_)), "{bad}");
        }
        assert_eq!(nav_gate("mailto:a@b"), NavGate::Open(guard_open("mailto:a@b", false)));
        assert!(matches!(nav_gate("ms-msdt:x"), NavGate::Open(Run::Blocked(_))));
        for ok in ["https://x/", "iterm2-about:history", "about:blank"] {
            assert_eq!(nav_gate_tab(ok), NavGate::Navigate(ok.to_string()), "{ok}");
        }
        for bad in ["JAVASCRIPT:alert(1)", " javascript:alert(1)", "data:text/html,<b>x</b>", "vbscript:x", "file:///C:/Windows/win.ini", "FILE:///C:/x"] {
            assert!(matches!(nav_gate_tab(bad), NavGate::Blocked(_)), "{bad}");
        }
        assert_eq!(nav_gate_tab("mailto:a@b"), nav_gate("mailto:a@b"));
        assert_eq!(nav_gate_tab("//evil/s/x"), nav_gate("//evil/s/x"));
        // The gate's rule on its own.
        assert_eq!(gate_refusal(r"\\?\D:\s"), None);
        assert_eq!(gate_refusal("https://x/"), None);
        assert_eq!(gate_refusal("file:///C:/x"), None);
        assert_eq!(gate_refusal("  //h/s"), Some(text("//h/s")));
        assert_eq!(gate_refusal("file://LOCALHOST/C:/x"), None);
    }

    /// B3: `browser_js` has no second navigation rule: no `file:` decoding and no `file` arm that navigates.
    #[test]
    fn nav_gate_has_no_file_navigate_arm() {
        let src = include_str!("browser_js.rs");
        let live = src.split("#[cfg(test)]").next().unwrap();
        assert!(!live.contains("file_url_path") && !live.contains("file_url_local"));
        let gate = &live[live.find("pub fn nav_gate(").unwrap()..live.find("pub fn nav_gate_tab(").unwrap()];
        assert!(!gate.contains("\"file\" =>") && !gate.contains("\"file\" |"), "no file arm");
    }

    #[test]
    fn make_safe_follows_iterm2() {
        assert_eq!(make_safe("1+1"), "(() => {\n    let __result__ = (function() {\n        return 1+1\n    })();\n    return typeof __result__ === 'undefined' ? null : __result__;\n})()");
        assert_eq!(iife("return 2;"), "(function() {return 2;})();");
        assert_eq!(js_string("a\"b\n"), "\"a\\\"b\\n\"");
    }

    #[test]
    fn coordinates_follow_iterm2() {
        assert_eq!(to_js((300.0, 150.0), 1.5, 2.0), (100.0, 50.0));
        assert_eq!(from_js((100.0, 50.0), 1.5, 2.0), (300.0, 150.0));
    }

    #[test]
    fn the_resource_table_lists_every_file() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("resources").join("browser");
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with("LICENSE") {
                continue;
            }
            assert_eq!(resource(&name), Some(std::fs::read_to_string(e.path()).unwrap().as_str()), "{name} is in the table");
        }
        assert!(resource("nope.js").is_none() && resource("../Cargo.toml").is_none());
    }
}
