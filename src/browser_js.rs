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
        "broadcast-input.js" => include_str!("../resources/browser/broadcast-input.js"),
        "bookmarks-page.html" => include_str!("../resources/browser/bookmarks-page.html"),
        "bookmarks-page.js" => include_str!("../resources/browser/bookmarks-page.js"),
        "challenge-frame-detection.js" => include_str!("../resources/browser/challenge-frame-detection.js"),
        "check-iframe-origins.js" => include_str!("../resources/browser/check-iframe-origins.js"),
        "console-log.js" => include_str!("../resources/browser/console-log.js"),
        "editing-detector.js" => include_str!("../resources/browser/editing-detector.js"),
        "extend-selection-bigword.js" => include_str!("../resources/browser/extend-selection-bigword.js"),
        "extend-selection-character.js" => include_str!("../resources/browser/extend-selection-character.js"),
        "extend-selection-line.js" => include_str!("../resources/browser/extend-selection-line.js"),
        "extend-selection-word.js" => include_str!("../resources/browser/extend-selection-word.js"),
        "extract-text-at-point.js" => include_str!("../resources/browser/extract-text-at-point.js"),
        "extend-selection-to-point.js" => include_str!("../resources/browser/extend-selection-to-point.js"),
        "open-link-at-point.js" => include_str!("../resources/browser/open-link-at-point.js"),
        "select-text.js" => include_str!("../resources/browser/select-text.js"),
        "urls-at-point.js" => include_str!("../resources/browser/urls-at-point.js"),
        "copy-mode.js" => include_str!("../resources/browser/copy-mode.js"),
        "copy-mode-util.js" => include_str!("../resources/browser/copy-mode-util.js"),
        "copy-mode-cursor.js" => include_str!("../resources/browser/copy-mode-cursor.js"),
        "copy-mode-cursor-movement.js" => include_str!("../resources/browser/copy-mode-cursor-movement.js"),
        "copy-mode-impl.js" => include_str!("../resources/browser/copy-mode-impl.js"),
        "custom-find.js" => include_str!("../resources/browser/custom-find.js"),
        "find-api.js" => include_str!("../resources/browser/find-api.js"),
        "find-constants.js" => include_str!("../resources/browser/find-constants.js"),
        "find-engine.js" => include_str!("../resources/browser/find-engine.js"),
        "find-global-click.js" => include_str!("../resources/browser/find-global-click.js"),
        "find-highlighter.js" => include_str!("../resources/browser/find-highlighter.js"),
        "find-iframe-segment.js" => include_str!("../resources/browser/find-iframe-segment.js"),
        "find-match.js" => include_str!("../resources/browser/find-match.js"),
        "find-nav.js" => include_str!("../resources/browser/find-nav.js"),
        "find-security.js" => include_str!("../resources/browser/find-security.js"),
        "find-segment.js" => include_str!("../resources/browser/find-segment.js"),
        "find-text-segment.js" => include_str!("../resources/browser/find-text-segment.js"),
        "find-utils.js" => include_str!("../resources/browser/find-utils.js"),
        "graph-discovery-api.js" => include_str!("../resources/browser/graph-discovery-api.js"),
        "graph-discovery.js" => include_str!("../resources/browser/graph-discovery.js"),
        "has-selection.js" => include_str!("../resources/browser/has-selection.js"),
        "history-page.html" => include_str!("../resources/browser/history-page.html"),
        "history-page.js" => include_str!("../resources/browser/history-page.js"),
        "hover-link-detector.js" => include_str!("../resources/browser/hover-link-detector.js"),
        "insert-text.js" => include_str!("../resources/browser/insert-text.js"),
        "iterm-browser-common.css" => include_str!("../resources/browser/iterm-browser-common.css"),
        "log-errors.js" => include_str!("../resources/browser/log-errors.js"),
        "monitor-selection.js" => include_str!("../resources/browser/monitor-selection.js"),
        "pointer-gate.js" => include_str!("../resources/browser/pointer-gate.js"),
        "selection-gesture.js" => include_str!("../resources/browser/selection-gesture.js"),
        "tandem-cloak.js" => include_str!("../resources/browser/tandem-cloak.js"),
        "view-source.html" => include_str!("../resources/browser/view-source.html"),
        "welcome-page.html" => include_str!("../resources/browser/welcome-page.html"),
        "welcome-page.js" => include_str!("../resources/browser/welcome-page.js"),
        "Readability.js" => include_str!("../resources/browser/Readability.js"),
        "reader-mode.css" => include_str!("../resources/browser/reader-mode.css"),
        "reader-mode-with-styles.js" => include_str!("../resources/browser/reader-mode-with-styles.js"),
        "load-readability.js" => include_str!("../resources/browser/load-readability.js"),
        "load-reader-mode.js" => include_str!("../resources/browser/load-reader-mode.js"),
        "enter-reader-mode.js" => include_str!("../resources/browser/enter-reader-mode.js"),
        "exit-reader-mode.js" => include_str!("../resources/browser/exit-reader-mode.js"),
        "distraction-removal.js" => include_str!("../resources/browser/distraction-removal.js"),
        "distraction-removal-helpers.js" => include_str!("../resources/browser/distraction-removal-helpers.js"),
        "enter-distraction-removal.js" => include_str!("../resources/browser/enter-distraction-removal.js"),
        "exit-distraction-removal.js" => include_str!("../resources/browser/exit-distraction-removal.js"),
        "remove-element-at-point.js" => include_str!("../resources/browser/remove-element-at-point.js"),
        "extract-plain-text.js" => include_str!("../resources/browser/extract-plain-text.js"),
        "extract-resources.js" => include_str!("../resources/browser/extract-resources.js"),
        "add-saved-attributes.js" => include_str!("../resources/browser/add-saved-attributes.js"),
        "convert-to-markdown.js" => include_str!("../resources/browser/convert-to-markdown.js"),
        "turndown.browser.umd.js" => include_str!("../resources/browser/turndown.browser.umd.js"),
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

/// B2 (13x3): the find engine posts every match of a page in one message, which a long page pushes
/// past the bridge's limits (the count would never update). For the find handler only, the Client
/// bridge cuts each match text and context to 200 UTF-16 units (never inside a surrogate pair, a lone
/// surrogate made well-formed, or serde_json would refuse the whole message) and keeps the first N
/// entries of `matchIdentifiers` and `contexts`; `totalMatches` and `currentMatch` stay whole. N is
/// found by weighing what `ViewGate::admit` weighs (13x3 Task 1 review I3): the payload as the escaped
/// string inside the `Runtime.bindingCalled` event, a quote or backslash 2 bytes, a unit outside
/// printable ASCII 6 (`\uXXXX`), the rest 1. That weight also bounds the decoded UTF-8 size. The
/// copied find scripts are not edited.
const CLIENT_CAP: &str = "function (name, body) { if (name !== 'iTermCustomFind' || !body || typeof body !== 'object' || !body.data || typeof body.data !== 'object') { return body; } const ids = Array.isArray(body.data.matchIdentifiers) ? body.data.matchIdentifiers : null; const ctx = Array.isArray(body.data.contexts) ? body.data.contexts : null; if (!ids && !ctx) { return body; } const cut = function (s) { if (typeof s !== 'string') { return s; } if (typeof s.toWellFormed === 'function') { s = s.toWellFormed(); } if (s.length <= 200) { return s; } const c = s.charCodeAt(199); return s.slice(0, c >= 0xD800 && c <= 0xDBFF ? 199 : 200); }; const build = function (n) { const data = Object.assign({}, body.data); if (ids) { data.matchIdentifiers = ids.slice(0, n).map(function (m) { return Object.assign({}, m, {text: cut(m && m.text)}); }); } if (ctx) { data.contexts = ctx.slice(0, n).map(function (c) { return Object.assign({}, c, {contextBefore: cut(c && c.contextBefore), contextAfter: cut(c && c.contextAfter)}); }); } return Object.assign({}, body, {data: data}); }; const weigh = function (o) { const s = JSON.stringify({h: name, b: o}); let w = 0; for (let i = 0; i < s.length; i++) { const c = s.charCodeAt(i); w += (c === 34 || c === 92) ? 2 : (c >= 32 && c <= 126) ? 1 : 6; } return w; }; let n = Math.max(ids ? ids.length : 0, ctx ? ctx.length : 0); let out = build(n); let size = weigh(out); while (size > 60000 && n > 0) { const next = Math.floor(n * 60000 / size * 0.9); n = next < n ? next : n - 1; out = build(n); size = weigh(out); } return out; }";

/// The find cap's budget, in the bytes the DevTools event spends on the payload (`CLIENT_CAP`): room
/// under `MAX_MESSAGE + MAX_URL` (73728) for the event's own fields, and under `MAX_MESSAGE` decoded.
pub const CAP_BUDGET: usize = 60000;

/// The closure-local `webkit` object: a Proxy answering `{postMessage}` for any handler name.
fn bridge_object(world: World) -> String {
    match world {
        World::Client => "const __tandemCap = __CAP__;\nconst __tandemWebkit = { messageHandlers: new Proxy({}, { get: function (_t, n) { const name = String(n); return { postMessage: function (body) { body = __tandemCap(name, body); if (typeof __tandemPost === 'function') { __tandemPost(JSON.stringify({h: name, b: body})); } } }; } }) };".replace("__CAP__", CLIENT_CAP),
        World::Page => "const __tandemWebkit = { messageHandlers: new Proxy({}, { get: function (_t, n) { const name = String(n); return { postMessage: function (body) { if (__tandemPost) { __tandemPost({h: name, b: body}); } } }; } }) };".to_string(),
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
    s.push_str(&bridge_object(script.world));
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

// --- The always-on scripts (13x2 Task 2) ---

pub const CONSOLE_HANDLER: &str = "iTerm2ConsoleLog";
pub const EDITING_HANDLER: &str = "iTerm2EditingDetector";
pub const SELECTION_HANDLER: &str = "iTermSelectionMonitor";
/// Tandem's own: the person used the pointer or keyboard (`selection-gesture.js`).
pub const SELECTION_GESTURE_HANDLER: &str = "TandemSelectionGesture";
/// The longest selection Tandem stores or copies (B10).
pub const MAX_SELECTION: usize = 1024 * 1024;

fn script(id: &'static str, code: String, world: World) -> UserScript {
    UserScript { id, code, inject: Inject::DocumentStart, main_frame_only: false, world }
}

/// The scripts every browser view carries, in iTerm2's order (`BM:215-372`): Tandem's cloak first of
/// the page world, the console wrapper in both worlds, then the isolated world's graph discovery,
/// selection monitor (with Tandem's gesture script) and editing detector, then the local pages'
/// bridge. `secret` is the view's, `dom_secret` the editing detector's separate one (B9). With
/// `client_ok` false (no isolated world) no Client script is offered: never moved to the page world.
pub fn always_on_scripts(secret: &str, dom_secret: &str, client_ok: bool, debug: bool) -> Vec<UserScript> {
    let load = |name: &str, subs: &[(&str, &str)]| fill_template(resource(name).unwrap_or_default(), subs);
    let log_errors = if debug { resource("log-errors.js").unwrap_or_default() } else { "" };
    let console = load("console-log.js", &[("LOG_ERRORS", log_errors)]);
    let mut v = vec![script("tandem-cloak", load("tandem-cloak.js", &[]), World::Page), script("console-log", console.clone(), World::Page)];
    if client_ok {
        // The isolated world's console copy carries graph discovery's debug lines: debug builds only
        // (the app counts console lines and never reads them, so a release build gains nothing).
        if debug {
            v.push(script("console-log", console, World::Client));
        }
        v.push(crate::browser_frames::user_script());
        v.push(script("monitor-selection", load("monitor-selection.js", &[("SECRET", secret)]), World::Client));
        v.push(script("selection-gesture", load("selection-gesture.js", &[("SECRET", secret)]), World::Client));
        v.push(script("editing-detector", load("editing-detector.js", &[("SECRET", dom_secret)]), World::Client));
        // The hovered link (`BM:331-339`): the main frame only, in the isolated world.
        v.push(UserScript { main_frame_only: true, ..script("hover-link-detector", load("hover-link-detector.js", &[("SECRET", secret)]), World::Client) });
    }
    v.push(local_page_script());
    v
}

/// `BM:1066-1087`: the level and the message of a console line.
pub fn console_line(body: &Value) -> Option<(String, String)> {
    Some((body.get("level")?.as_str()?.to_string(), body.get("msg")?.as_str()?.to_string()))
}

/// iTerm2's own check in every handler: the body's `sessionSecret` is the view's. An empty secret is
/// never a match.
pub fn secret_ok(body: &Value, secret: &str) -> bool {
    !secret.is_empty() && body.get("sessionSecret").and_then(Value::as_str) == Some(secret)
}

#[derive(Clone, Debug, PartialEq)]
pub enum AlwaysOn {
    /// A console line: only its level and size are kept, never its text.
    Console { level: &'static str, size: usize },
    Selection(String),
    /// A trusted press (true) or release (false).
    Gesture(bool),
    Editing(bool),
    /// The link under the pointer, or none (`HLH`).
    Hover(crate::browser_tip::Hover),
    /// A trusted press the pointer gate took (Task 7).
    Pointer(crate::browser_pointer::Press),
    /// What the person typed, to be typed into the other browser panes of the broadcast set (Task 8).
    Broadcast(crate::browser_keys::BroadcastEvent),
}

/// What an always-on handler's message means. The console log comes from either world and needs no
/// secret; every other handler is the isolated world's alone and checks its secret (`SEM:24-32`,
/// `EDH:28-36`): the editing detector's is `dom_secret`.
pub fn always_on(r: &Routed, secret: &str, dom_secret: &str) -> Option<AlwaysOn> {
    if r.handler == CONSOLE_HANDLER {
        let (level, msg) = console_line(&r.body)?;
        let level = match level.as_str() {
            "debug" => "debug",
            "warn" => "warn",
            "error" => "error",
            _ => "log",
        };
        return Some(AlwaysOn::Console { level, size: msg.len() });
    }
    if r.world != World::Client {
        return None;
    }
    match r.handler.as_str() {
        SELECTION_HANDLER if secret_ok(&r.body, secret) => Some(AlwaysOn::Selection(r.body.get("selection")?.as_str()?.to_string())),
        SELECTION_GESTURE_HANDLER if secret_ok(&r.body, secret) => Some(AlwaysOn::Gesture(r.body.get("phase").and_then(Value::as_str) == Some("down"))),
        EDITING_HANDLER if secret_ok(&r.body, dom_secret) => Some(AlwaysOn::Editing(r.body.get("editable")?.as_bool()?)),
        crate::browser_tip::HOVER_HANDLER if secret_ok(&r.body, secret) => crate::browser_tip::hover_of(&r.body).map(AlwaysOn::Hover),
        crate::browser_keys::BROADCAST_HANDLER if secret_ok(&r.body, secret) && r.frame.is_none() => crate::browser_keys::broadcast_of(&r.body).map(AlwaysOn::Broadcast),
        crate::browser_pointer::GATE_HANDLER if secret_ok(&r.body, secret) && r.frame.is_none() => crate::browser_pointer::press_of(&r.body).map(AlwaysOn::Pointer),
        _ => None,
    }
}

/// The selection as stored: at most `MAX_SELECTION` bytes, cut at a character boundary.
pub fn store_selection(text: &str) -> String {
    if text.len() <= MAX_SELECTION {
        return text.to_string();
    }
    let mut end = MAX_SELECTION;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_string()
}

/// How long a press (a mouse button or a key) is taken to still be down.
const MAX_PRESS: std::time::Duration = std::time::Duration::from_secs(30);
/// How soon after a trusted release a selection change still counts as the person's (a double click).
const AFTER_RELEASE: std::time::Duration = std::time::Duration::from_secs(1);

/// When a page selection may be copied (B10): only for a selection the PERSON made, never empty,
/// never above `MAX_SELECTION`, once per distinct text, and only with copy on selection on. A trusted
/// press (mouse down, key down) starts a selection: the changes while it is down are held and the
/// trusted release copies the latest, so a drag or Shift+arrows copies once, as iTerm2 does for every
/// change. A change within a second after a trusted release (a double click) copies at once. A
/// selection no trusted press or release stands behind (a script's) is never copied.
#[derive(Default, Debug)]
pub struct SelectionGate {
    pressed: Option<std::time::Instant>,
    last_up: Option<std::time::Instant>,
    held: Option<String>,
    last_copied: Option<String>,
}

impl SelectionGate {
    /// A trusted press began.
    pub fn down(&mut self, now: std::time::Instant) {
        self.pressed = Some(now);
        self.held = None;
    }

    /// A trusted release: the text held during the press, if it is to be copied.
    pub fn up(&mut self, now: std::time::Instant, copy_on: bool) -> Option<String> {
        let was_down = self.pressed.take().is_some_and(|p| now.saturating_duration_since(p) < MAX_PRESS);
        self.last_up = Some(now);
        let held = self.held.take();
        if !was_down || !copy_on {
            return None;
        }
        held.and_then(|t| self.take_copy(t))
    }

    /// The selection changed to `text`: the text to copy now, if any.
    pub fn accept(&mut self, text: &str, now: std::time::Instant, copy_on: bool) -> Option<String> {
        if text.is_empty() {
            self.last_copied = None;
            self.held = None;
            return None;
        }
        if text.len() > MAX_SELECTION {
            self.held = None;
            return None;
        }
        if self.pressed.is_some_and(|p| now.saturating_duration_since(p) < MAX_PRESS) {
            self.held = Some(text.to_string());
            return None;
        }
        let recent = self.last_up.is_some_and(|u| now.saturating_duration_since(u) < AFTER_RELEASE);
        if !copy_on || !recent {
            return None;
        }
        self.take_copy(text.to_string())
    }

    fn take_copy(&mut self, text: String) -> Option<String> {
        if self.last_copied.as_deref() == Some(text.as_str()) {
            return None;
        }
        self.last_copied = Some(text.clone());
        Some(text)
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }
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

    /// B2: a find result is capped inside the Client bridge, before it is posted, so the copied find
    /// engine stays unedited and a long page's result still fits `MAX_MESSAGE`.
    #[test]
    fn the_client_bridge_caps_find_results() {
        let c = wrap(&script("window.webkit.messageHandlers.iTermCustomFind.postMessage({a: 1})", World::Client));
        assert!(c.contains("const __tandemCap = function (name, body)"), "{c}");
        assert!(c.contains("name !== 'iTermCustomFind'"), "only the find handler is capped: {c}");
        assert!(c.contains("body = __tandemCap(name, body);"), "applied before the post: {c}");
        assert!(c.contains(&format!("size > {CAP_BUDGET}")), "the byte budget: {c}");
        assert!(!c.to_ascii_lowercase().contains("secret"), "the cap copies the body whole and names no secret");
        // The page-world bridge (local pages) is not touched.
        assert!(!wrap(&script("x", World::Page)).contains("__tandemCap"));
        assert!(CAP_BUDGET + 4096 < crate::webhost::MAX_MESSAGE + crate::webhost::MAX_URL && CAP_BUDGET < crate::webhost::MAX_MESSAGE, "the budget leaves room for the event's own fields");
        // A surrogate pair is never split at the cut, and a lone one is made well-formed.
        assert!(c.contains("c >= 0xD800 && c <= 0xDBFF ? 199 : 200") && c.contains("s.toWellFormed()"), "{c}");
    }

    /// 13x3 Task 1 review I3: the find cap weighs what `ViewGate::admit` weighs, the escaped
    /// `Runtime.bindingCalled` event, where the payload is a JSON string inside JSON: a quote or a
    /// backslash costs 2 bytes and a UTF-16 unit outside printable ASCII 6 (`\uXXXX`). A payload of
    /// CJK, accented or astral text right at the budget passes the gate and the decoded-size check.
    #[test]
    fn the_cap_budgets_the_escaped_event() {
        assert!(CLIENT_CAP.contains("(c === 34 || c === 92) ? 2 : (c >= 32 && c <= 126) ? 1 : 6"), "the escaped weight per unit: {CLIENT_CAP}");
        assert!(CLIENT_CAP.contains(&format!("> {CAP_BUDGET}")), "the budget: {CLIENT_CAP}");
        assert!(!CLIENT_CAP.contains("TextEncoder"), "never the UTF-8 size");
        fn weigh(s: &str) -> usize {
            s.encode_utf16().map(|c| match c {
                34 | 92 => 2,
                32..=126 => 1,
                _ => 6,
            }).sum()
        }
        // The payload as the DevTools event writes it (the worst case: every unit outside ASCII escaped).
        fn devtools_string(s: &str) -> String {
            let mut o = String::from("\"");
            for c in s.encode_utf16() {
                match c {
                    34 => o.push_str("\\\""),
                    92 => o.push_str("\\\\"),
                    32..=126 => o.push(c as u8 as char),
                    _ => o.push_str(&format!("\\u{c:04x}")),
                }
            }
            o.push('"');
            o
        }
        let session = Some("0".repeat(64));
        for unit in ["\u{691c}\u{7d22}\u{8a9e} ", "e\u{301}\u{e9}\u{e8}\"\\a", "\u{1f600}\u{4e2d}"] {
            let payload = |n: usize| serde_json::json!({"h": "iTermCustomFind", "b": {"action": "resultsUpdated", "data": {"totalMatches": 6000, "contexts": [{"contextBefore": unit.repeat(n), "contextAfter": ""}]}}}).to_string();
            let base = weigh(&payload(0));
            let step = weigh(&payload(1)) - base;
            let n = (CAP_BUDGET - base) / step;
            let p = payload(n);
            assert!(weigh(&p) <= CAP_BUDGET && weigh(&p) + step > CAP_BUDGET, "near the limit: {}", weigh(&p));
            let event = format!("{{\"name\":\"{}\",\"payload\":{},\"executionContextId\":-2147483648}}", crate::browser_world::BINDING, devtools_string(&p));
            assert_eq!(devtools_string(&p).len() - 2, weigh(&p), "the escaped payload is the weight");
            let gate = crate::webhost::ViewGate::new();
            let e = crate::webhost::Event::DevTools { view: 1, session: session.clone(), name: crate::webhost::BINDING_EVENT.into(), json: event };
            assert!(gate.admit(&e).is_ok(), "a capped result is admitted ({unit:?}, {} bytes)", e.weight());
            assert!(p.len() <= MAX_MESSAGE, "and its decoded size is within the message limit");
            assert!(route(&p, World::Client, None, "o").is_some());
        }
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

    fn routed(handler: &str, world: World, body: serde_json::Value) -> Routed {
        Routed { handler: handler.into(), world, frame: None, source: "https://a.test/".into(), body }
    }

    #[test]
    fn always_on_scripts_follow_iterm2() {
        assert_eq!(console_line(&json!({"msg": "hi", "level": "log"})), Some(("log".to_string(), "hi".to_string())));
        assert_eq!(console_line(&json!({"msg": 5, "level": "log"})), None);
        assert_eq!(console_line(&json!({"msg": "hi"})), None);
        assert_eq!(console_line(&json!("hi")), None);
        assert!(secret_ok(&json!({"sessionSecret": "S"}), "S"));
        assert!(!secret_ok(&json!({"sessionSecret": "T"}), "S"));
        assert!(!secret_ok(&json!({}), "S") && !secret_ok(&json!({"sessionSecret": ""}), ""), "no secret is never a match");
        let list = always_on_scripts("SECRET1", "DOM1", true, false);
        let ids: Vec<(&str, World, Inject, bool)> = list.iter().map(|s| (s.id, s.world, s.inject, s.main_frame_only)).collect();
        let st = Inject::DocumentStart;
        assert_eq!(
            ids,
            vec![
                ("tandem-cloak", World::Page, st, false),
                ("console-log", World::Page, st, false),
                ("GraphDiscovery", World::Client, st, false),
                ("monitor-selection", World::Client, st, false),
                ("selection-gesture", World::Client, st, false),
                ("editing-detector", World::Client, st, false),
                ("hover-link-detector", World::Client, st, true),
                ("local-page-bridge", World::Page, st, true),
            ]
        );
        // The cloak is the first of the page world's scripts.
        assert_eq!(list.iter().find(|s| s.world == World::Page).unwrap().id, "tandem-cloak");
        // Secrets: the view's in the selection scripts, the separate DOM one in the editing detector,
        // neither in a page script, and no template key is left over.
        let code = |id: &str| list.iter().find(|s| s.id == id).unwrap().code.clone();
        assert!(code("monitor-selection").contains("SECRET1") && code("selection-gesture").contains("SECRET1"));
        assert!(code("editing-detector").contains("DOM1") && !code("editing-detector").contains("SECRET1"));
        for s in list.iter().filter(|s| s.world == World::Page) {
            assert!(!s.code.contains("SECRET1") && !s.code.contains("DOM1"), "{} holds no secret", s.id);
        }
        for s in &list {
            assert!(!s.code.contains("{{"), "{} has an unfilled key", s.id);
        }
        // The console wrapper's log-errors part, and its isolated-world copy (graph discovery's debug
        // lines), are in debug builds only (Task 2 review I2).
        let dbg = always_on_scripts("S", "D", true, true);
        assert!(dbg.iter().any(|s| s.id == "console-log" && s.world == World::Client));
        assert!(!list.iter().any(|s| s.id == "console-log" && s.world == World::Client));
        assert!(dbg.iter().any(|s| s.id == "console-log" && s.code.contains("[Injected JS Error]")));
        assert!(!list.iter().any(|s| s.code.contains("[Injected JS Error]")));
        // Without the isolated world no Client script is offered, never moved to the page world.
        let none = always_on_scripts("S", "D", false, false);
        assert!(none.iter().all(|s| s.world == World::Page));
        assert_eq!(none.iter().map(|s| s.id).collect::<Vec<_>>(), vec!["tandem-cloak", "console-log", "local-page-bridge"]);
        // The cloak removes only the page-world channel, only in a challenge frame.
        let cloak = code("tandem-cloak");
        assert!(cloak.contains("delete window.chrome.webview") && cloak.contains("hcaptcha") && !cloak.contains("webkit"));
        // Every listener of a Tandem script checks isTrusted first (P26).
        for name in ["selection-gesture.js", "tandem-cloak.js", "pointer-gate.js", "broadcast-input.js"] {
            let text = resource(name).unwrap();
            assert!(text.contains("Tandem's own script"), "{name} says it is Tandem's");
            let mut at = 0;
            while let Some(i) = text[at..].find("addEventListener(") {
                let tail = &text[at + i..];
                assert!(tail[..tail.len().min(160)].contains("if (!e.isTrusted)"), "{name}: a listener without the isTrusted check");
                at += i + 1;
            }
        }
    }

    #[test]
    fn always_on_handlers_follow_iterm2() {
        let sel = |world, secret: &str| routed(SELECTION_HANDLER, world, json!({"selection": "abc", "sessionSecret": secret}));
        assert_eq!(always_on(&sel(World::Client, "S"), "S", "D"), Some(AlwaysOn::Selection("abc".into())));
        assert_eq!(always_on(&sel(World::Page, "S"), "S", "D"), None, "a page cannot report a selection (SEM:24-32)");
        assert_eq!(always_on(&sel(World::Client, "T"), "S", "D"), None);
        let edit = |world, v: serde_json::Value, secret: &str| routed(EDITING_HANDLER, world, json!({"editable": v, "sessionSecret": secret}));
        assert_eq!(always_on(&edit(World::Client, json!(true), "D"), "S", "D"), Some(AlwaysOn::Editing(true)));
        assert_eq!(always_on(&edit(World::Client, json!(false), "D"), "S", "D"), Some(AlwaysOn::Editing(false)));
        assert_eq!(always_on(&edit(World::Client, json!(true), "S"), "S", "D"), None, "the view secret is not the detector's");
        assert_eq!(always_on(&edit(World::Client, json!("yes"), "D"), "S", "D"), None);
        assert_eq!(always_on(&edit(World::Page, json!(true), "D"), "S", "D"), None);
        let gest = |world, secret: &str| routed(SELECTION_GESTURE_HANDLER, world, json!({"t": "gesture", "sessionSecret": secret}));
        assert_eq!(always_on(&gest(World::Client, "S"), "S", "D"), Some(AlwaysOn::Gesture(false)));
        assert_eq!(always_on(&gest(World::Page, "S"), "S", "D"), None);
        assert_eq!(always_on(&gest(World::Client, "x"), "S", "D"), None);
        // The console log comes from both worlds, with no secret; it is only counted.
        let con = |world| routed(CONSOLE_HANDLER, world, json!({"msg": "hello", "level": "warn"}));
        // The hovered link: the isolated world's alone, with the view's secret (HLH:34-41).
        let hov = |world, secret: &str| routed(crate::browser_tip::HOVER_HANDLER, world, json!({"type": "hover", "url": "https://a", "x": 1.0, "y": 2.0, "width": 3.0, "height": 4.0, "sessionSecret": secret}));
        assert_eq!(always_on(&hov(World::Client, "S"), "S", "D"), Some(AlwaysOn::Hover(crate::browser_tip::Hover::Show { url: "https://a".into(), rect: (1.0, 2.0, 3.0, 4.0) })));
        assert_eq!(always_on(&hov(World::Page, "S"), "S", "D"), None, "a page cannot show a link");
        assert_eq!(always_on(&hov(World::Client, "D"), "S", "D"), None, "the editing detector's value is not the view's");
        assert_eq!(always_on(&con(World::Page), "S", "D"), Some(AlwaysOn::Console { level: "warn", size: 5 }));
        assert_eq!(always_on(&con(World::Client), "S", "D"), Some(AlwaysOn::Console { level: "warn", size: 5 }));
        assert_eq!(always_on(&routed(CONSOLE_HANDLER, World::Page, json!({"msg": "x", "level": "weird"})), "S", "D"), Some(AlwaysOn::Console { level: "log", size: 1 }));
        assert_eq!(always_on(&routed("other", World::Client, json!({})), "S", "D"), None);
    }

    /// B10: the clipboard and paste history are written only for a selection that follows a trusted
    /// gesture within a second, once per distinct text, bounded, and only with CopySelection on.
    #[test]
    fn selection_copy_needs_a_recent_trusted_gesture() {
        use std::time::{Duration, Instant};
        let ms = Duration::from_millis;
        let t0 = Instant::now();
        let mut g = SelectionGate::default();
        // A page-made selection: no press, no release, no copy (and a release alone copies nothing).
        assert_eq!(g.accept("abc", t0, true), None, "no gesture, no copy");
        assert_eq!(g.up(t0 + ms(50), true), None, "a selection no press led to is not copied by a release");
        // A mouse drag or Shift+arrows: the selection changes while the press is down, the release copies the last one, once.
        let t = t0 + Duration::from_secs(10);
        g.down(t);
        assert_eq!(g.accept("a", t + ms(100), true), None, "held until the release");
        assert_eq!(g.accept("ab", t + ms(200), true), None);
        assert_eq!(g.up(t + ms(300), true), Some("ab".to_string()), "the release copies the latest selection");
        assert_eq!(g.up(t + ms(310), true), None, "once");
        // A double click: the word is selected after the release's gesture, within a second.
        assert_eq!(g.accept("word", t + ms(400), true), Some("word".to_string()));
        assert_eq!(g.accept("word", t + ms(450), true), None, "the same text is not copied again");
        assert_eq!(g.accept("more", t + ms(1400), true), None, "a second after the release is too late");
        // CopySelection off copies nothing and drops what was held.
        let t = t0 + Duration::from_secs(20);
        g.down(t);
        g.accept("held", t + ms(10), true);
        assert_eq!(g.up(t + ms(20), false), None, "CopySelection off copies nothing (I6)");
        assert_eq!(g.up(t + ms(30), true), None, "and nothing stays held");
        // A press that never ended does not make a selection a person's for ever.
        let t = t0 + Duration::from_secs(30);
        g.down(t);
        assert_eq!(g.accept("late", t + Duration::from_secs(31), true), None);
        assert_eq!(g.up(t + Duration::from_secs(32), true), None);
        // An empty selection copies nothing and lets the same text copy again.
        let t = t0 + Duration::from_secs(100);
        g.down(t);
        g.accept("abd", t + ms(10), true);
        assert_eq!(g.up(t + ms(20), true), Some("abd".to_string()));
        assert_eq!(g.accept("", t + ms(30), true), None);
        g.down(t + ms(40));
        g.accept("abd", t + ms(50), true);
        assert_eq!(g.up(t + ms(60), true), Some("abd".to_string()), "after a deselect the same text copies again");
        // Never copied above the bound; stored bounded.
        g.down(t + ms(100));
        let big = "x".repeat(MAX_SELECTION + 1);
        g.accept(&big, t + ms(110), true);
        assert_eq!(g.up(t + ms(120), true), None, "never copied above the bound");
        assert!(store_selection(&big).len() <= MAX_SELECTION, "stored bounded");
        assert_eq!(store_selection("abc"), "abc");
        // A reset (a navigation) forgets a press.
        g.down(t + ms(200));
        g.reset();
        g.accept("z", t + ms(210), true);
        assert_eq!(g.up(t + ms(220), true), None);
    }

    /// The gesture message says which half it is: a press or a release; none is a release.
    #[test]
    fn gestures_say_press_or_release() {
        let gest = |phase: Option<&str>| {
            let mut b = json!({"t": "gesture", "sessionSecret": "S"});
            if let Some(p) = phase {
                b["phase"] = json!(p);
            }
            routed(SELECTION_GESTURE_HANDLER, World::Client, b)
        };
        assert_eq!(always_on(&gest(Some("down")), "S", "D"), Some(AlwaysOn::Gesture(true)));
        assert_eq!(always_on(&gest(Some("up")), "S", "D"), Some(AlwaysOn::Gesture(false)));
        assert_eq!(always_on(&gest(None), "S", "D"), Some(AlwaysOn::Gesture(false)));
        let text = resource("selection-gesture.js").unwrap();
        for ev in ["pointerdown", "mousedown", "keydown", "pointerup", "mouseup", "keyup"] {
            assert!(text.contains(&format!("\"{ev}\"")), "{ev}");
        }
        assert!(text.contains("e.repeat"), "a held key's repeats are not presses");
    }
}
