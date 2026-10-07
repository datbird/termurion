//! The WebView2 host's pure model (milestone 13x, Task 1): the runtime check's outcome, the user
//! data folders, environment options, the events a view posts, the one navigation rule, the
//! bridge's message and reply texts, the caps on everything a page can post, and bounds math.
//! Nothing here touches COM (`webhost_win`) or the App (`app::webhost_app`). Web content is
//! untrusted: every size, rate and shape a page controls is bounded here, before it is queued.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use crate::layout::Rect;

pub type ViewId = u64;

/// Whether a WebView2 runtime exists: its version string, or no runtime.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Runtime {
    Found(String),
    Missing,
}

/// iTerm2's plugin check: a version is a runtime, an empty one or any failure is none
/// (`GW:98-111`).
pub fn runtime_outcome(result: Result<String, i32>) -> Runtime {
    match result {
        Ok(v) if !v.trim().is_empty() => Runtime::Found(v),
        _ => Runtime::Missing,
    }
}

/// The longest fixed-runtime hint (a Windows path).
pub const MAX_HINT: usize = 260;

/// The fixed-version runtime folder `BrowserPluginPathHint` names (preflight B2): trimmed, not
/// empty, at most 260 characters, no control character, an absolute drive path with no `..` or
/// `.` step, no share (the no-UNC invariant), and `judge` (the one path judge, `LocalPath`) accepts
/// it. The window's callers add the owner, ACL and signature checks (`webhost_win`), which need
/// the disk. None means the hint is not used.
pub fn fixed_runtime_folder(hint: &str, judge: &dyn Fn(&Path) -> bool) -> Option<PathBuf> {
    refused_hint(hint, judge).ok().map(PathBuf::from)
}

/// Why a hint is refused (the banner text), or the trimmed folder.
pub fn refused_hint(hint: &str, judge: &dyn Fn(&Path) -> bool) -> Result<String, &'static str> {
    let h = hint.trim();
    if h.is_empty() {
        return Err("the folder is not set");
    }
    if h.chars().count() > MAX_HINT {
        return Err("the folder name is too long");
    }
    if h.chars().any(|c| c.is_control()) {
        return Err("the folder name holds a control character");
    }
    let b = h.as_bytes();
    if !(b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/')) {
        return Err("the folder is not an absolute local path");
    }
    if h[2..].split(['\\', '/']).any(|s| s == ".." || s == ".") {
        return Err("the folder name has a relative step");
    }
    if h[2..].contains(':') {
        return Err("the folder name has a stream or device part");
    }
    if !judge(Path::new(h)) {
        return Err("the folder is on a share, a mapped drive or behind a link");
    }
    Ok(h.to_string())
}

/// The two settings the host reads that are text a settings import, a remote-prefs snapshot or a
/// script could write.
pub const BOUNDED_KEYS: [&str; 2] = ["BrowserPluginPathHint", "WebUserAgent"];

/// The write-point bound (`Globals::set`, `bound_values`): a hint over 260 characters or holding a
/// control character is stored empty (a cut path could name another folder); a user agent loses its
/// control characters and is cut to 512 characters. Any other key passes unchanged.
pub fn bound_global(key: &str, value: toml::Value) -> toml::Value {
    match (key, &value) {
        ("BrowserPluginPathHint", toml::Value::String(s)) => toml::Value::String(if s.chars().count() > MAX_HINT || s.chars().any(|c| c.is_control()) { String::new() } else { s.clone() }),
        ("WebUserAgent", toml::Value::String(s)) => toml::Value::String(s.chars().filter(|c| !c.is_control()).take(512).collect()),
        _ => value,
    }
}

/// Which user data folder an environment owns. WebView2 refuses a second environment on one
/// folder with different options, so each store has its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Store {
    /// Tool views, popovers, Quick Look and the icon rasterizer.
    Default,
    /// Browser panes (the proxy arguments, the `iterm2-about` scheme).
    Browser,
    /// Dev Null Mode: every view is InPrivate; the folder is fresh for each run (`dev_null_folder`).
    BrowserDevNull,
}

impl Store {
    /// The store's name in a log line (13w `dlog!`/`rlog!` take only literals and numbers).
    pub fn log_name(self) -> &'static str {
        match self {
            Store::Default => "default",
            Store::Browser => "browser",
            Store::BrowserDevNull => "dev null",
        }
    }
}

/// `<LOCALAPPDATA>\Tandem\WebView2\<Default|Browser|DevNull>`. `BrowserDevNull`'s name is the base
/// the per-run folders live beside; the folder an environment really uses is `dev_null_folder`.
pub fn data_folder(local_appdata: &Path, store: Store) -> PathBuf {
    let name = match store {
        Store::Default => "Default",
        Store::Browser => "Browser",
        Store::BrowserDevNull => "DevNull",
    };
    local_appdata.join("Tandem").join("WebView2").join(name)
}

/// The folder every per-run Dev Null folder lives in.
pub fn tmp_folder(local_appdata: &Path) -> PathBuf {
    local_appdata.join("Tandem").join("WebView2").join("Tmp")
}

/// `Tmp\DevNull-<pid>-<hex8>` (preflight P4): fresh for each run, so nothing a Dev Null view stored
/// outlives the process that made it.
pub fn dev_null_folder(local_appdata: &Path, pid: u32, nonce: u32) -> PathBuf {
    tmp_folder(local_appdata).join(format!("DevNull-{pid}-{nonce:08x}"))
}

/// The pid in a per-run folder name; None for any other name.
pub fn dev_null_pid(name: &str) -> Option<u32> {
    let rest = name.strip_prefix("DevNull-")?;
    let (pid, nonce) = rest.split_once('-')?;
    (nonce.len() == 8 && nonce.bytes().all(|b| b.is_ascii_hexdigit()) && !pid.is_empty() && pid.bytes().all(|b| b.is_ascii_digit())).then(|| pid.parse().ok())?
}

/// The names in `Tmp` a launch sweep removes: per-run folders whose process is gone (and that are
/// not `me`).
pub fn stale_dev_null(names: &[String], me: u32, alive: &dyn Fn(u32) -> bool) -> Vec<String> {
    names.iter().filter(|n| dev_null_pid(n).is_some_and(|p| p == me || !alive(p))).cloned().collect()
}

/// What an environment is created with. InPrivate is per controller (`ViewSetup.in_private`), not
/// here.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EnvOptions {
    /// `AdditionalBrowserArguments`: exactly the proxy argument or empty (Task 8 pins it).
    pub args: String,
    /// Custom scheme registrations (`iterm2-about` for the browser stores).
    pub custom_schemes: Vec<String>,
}

/// The user agent a web view gets (`WVW:127-150`): a set agent is used as written; an empty one
/// keeps the engine's with the application name appended.
pub fn user_agent(setting: &str, engine_default: &str) -> String {
    if setting.is_empty() { format!("{engine_default} Tandem") } else { setting.to_string() }
}

/// What a view tells the app. Open for extension: every consumer's `match` ends in `_ =>`.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    Created(ViewId),
    Failed(ViewId, String),
    /// A finished navigation: `url` is the URI of the matching NavigationStarting (iTerm2's
    /// `lastRequestedURL`), `nav` its `NavigationId`.
    Navigated { view: ViewId, ok: bool, status: i32, url: String, nav: u64 },
    Source(ViewId, String),
    Title(ViewId, String),
    Favicon(ViewId, Vec<u8>),
    /// A page's `postMessage`; `source` is the posting document's URL.
    Message { view: ViewId, json: String, source: String },
    ScriptDone { view: ViewId, token: u64, json: Result<String, String> },
    NewWindow { view: ViewId, url: String },
    Key { view: ViewId, vk: u32, mods: u8 },
    Focus(ViewId, bool),
    History { view: ViewId, back: bool, forward: bool },
    Loading(ViewId, bool),
    /// A request a registered filter matched; the app answers with `answer_request(token, ..)`.
    Request { view: ViewId, token: u64, url: String, context: i32, top_url: String },
    /// A page or redirect asked to open an external protocol; the view always cancels it (B4).
    ExternalUri { view: ViewId, url: String, user: bool },
    /// A top-level navigation the view refused after a person's click (`refused_click`): the app
    /// judges the URL itself (a popover link goes through `guard_open`).
    Refused { view: ViewId, url: String, user: bool },
}

impl Event {
    /// The view an event belongs to.
    pub fn view(&self) -> ViewId {
        match self {
            Event::Created(v) | Event::Failed(v, _) | Event::Source(v, _) | Event::Title(v, _) | Event::Favicon(v, _) | Event::Focus(v, _) | Event::Loading(v, _) => *v,
            Event::Navigated { view, .. }
            | Event::Message { view, .. }
            | Event::ScriptDone { view, .. }
            | Event::NewWindow { view, .. }
            | Event::Key { view, .. }
            | Event::History { view, .. }
            | Event::Request { view, .. }
            | Event::ExternalUri { view, .. }
            | Event::Refused { view, .. } => *view,
        }
    }

    /// An event the queue may drop when a page floods it. Lifecycle, navigation results, focus,
    /// script results and requests (each holds a COM deferral) are never dropped.
    pub fn droppable(&self) -> bool {
        matches!(self, Event::Source(..) | Event::Title(..) | Event::Favicon(..) | Event::Message { .. } | Event::NewWindow { .. } | Event::ExternalUri { .. } | Event::Refused { .. } | Event::Key { .. } | Event::History { .. } | Event::Loading(..))
    }

    /// The bytes an event holds that a page chose.
    pub fn weight(&self) -> usize {
        match self {
            Event::Failed(_, s) | Event::Source(_, s) | Event::Title(_, s) => s.len(),
            Event::Favicon(_, b) => b.len(),
            Event::Message { json, source, .. } => json.len() + source.len(),
            Event::Navigated { url, .. } | Event::NewWindow { url, .. } | Event::ExternalUri { url, .. } | Event::Refused { url, .. } => url.len(),
            Event::Request { url, top_url, .. } => url.len() + top_url.len(),
            Event::ScriptDone { json, .. } => json.as_ref().map_or_else(|e| e.len(), |s| s.len()),
            _ => 0,
        }
    }
}

/// What becomes of a view whose parent window closes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParentFate {
    /// Its pane shows in another window now: the view moves to that handle.
    Move(isize),
    /// Its pane shows nowhere else (closed, stashed for Undo, buried): the view closes and is made
    /// again from its URL when the pane shows again.
    Close,
}

/// The views a closing window owns, chosen by their parent handle (never by the tabs the window
/// holds: a pane that left keeps its old parent until it is shown), each with its fate. `home`
/// answers the handle of the window that shows a pane now.
pub fn closing_parent<K: Copy>(views: &[(K, isize)], closing: isize, home: impl Fn(K) -> Option<isize>) -> Vec<(K, ParentFate)> {
    if closing == 0 {
        return Vec::new();
    }
    views
        .iter()
        .filter(|(_, parent)| *parent == closing)
        .map(|(k, _)| (*k, home(*k).filter(|h| *h != closing && *h != 0).map_or(ParentFate::Close, ParentFate::Move)))
        .collect()
}

/// How the app answers a deferred request.
#[derive(Clone, Debug, PartialEq)]
pub enum Answer {
    Continue,
    Block,
    Respond { status: u16, mime: String, body: Vec<u8>, headers: Vec<(String, String)> },
}

/// The header block `CreateWebResourceResponse` takes: `Content-Type: <mime>` then each
/// `Name: value`, CRLF-separated. A header whose name or value holds CR or LF is dropped, so a
/// value can never smuggle a second header; so is an empty or colon-holding name. The mime gets
/// the same test (a mime with a line break falls back to octet-stream).
pub fn response_headers(mime: &str, headers: &[(String, String)]) -> String {
    let clean = |s: &str| !s.contains(['\r', '\n', '\0']);
    let mime = if clean(mime) { mime } else { "application/octet-stream" };
    let mut out = format!("Content-Type: {mime}");
    for (name, value) in headers {
        if clean(name) && clean(value) && !name.is_empty() && !name.contains(':') && !name.contains(' ') {
            out.push_str("\r\n");
            out.push_str(name);
            out.push_str(": ");
            out.push_str(value);
        }
    }
    out
}

/// A URL's scheme, lowercased; empty when it has none.
pub fn scheme_of(url: &str) -> String {
    let end = url.find(':').unwrap_or(0);
    let s = &url[..end];
    if !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'-' || b == b'.') { s.to_ascii_lowercase() } else { String::new() }
}

fn local_scheme(s: &str) -> bool {
    matches!(s, "file" | "iterm2-file" | "iterm2-ssh")
}

/// The one rule every guarded view's NavigationStarting asks (R13x8ai item 3, B4): may this
/// navigation go on?
/// - a `file:` target that is not `file_ok` (a share, a mapped or remote drive, a link, a
///   verbatim form: `file_url_local`) is refused for every source and initiator;
/// - a redirect to `file:`, `iterm2-file:` or `iterm2-ssh:` is refused;
/// - a navigation Tandem started (`navigate_tandem`) or the person started is allowed;
/// - a page on `iterm2-file:` or `iterm2-ssh:` may navigate nowhere;
/// - a `file:`, `iterm2-file:` or `iterm2-ssh:` target is allowed only from an `iterm2-about:`
///   source;
/// - everything else is allowed. Schemes compare ignoring ASCII case.
pub fn navigation_allowed(source: Option<&str>, target: &str, user: bool, redirect: bool, by_tandem: bool, file_ok: bool) -> bool {
    let t = scheme_of(target);
    if t == "file" && !file_ok {
        return false;
    }
    if redirect && local_scheme(&t) {
        return false;
    }
    if by_tandem || user {
        return true;
    }
    let s = source.map(scheme_of).unwrap_or_default();
    if s == "iterm2-file" || s == "iterm2-ssh" {
        return false;
    }
    if local_scheme(&t) {
        return s == "iterm2-about";
    }
    true
}

/// Whether a `file:` URL names a local file the one path judge accepts (B4): no host other than
/// empty or `localhost`, a drive-letter path with no stream, no verbatim or device form, no
/// control character, and `judge` says yes (a remote or mapped drive, a link to a share: no). A
/// URL that is not `file:` is not ok.
pub fn file_url_local(url: &str, judge: &dyn Fn(&Path) -> bool) -> bool {
    if scheme_of(url) != "file" {
        return false;
    }
    let Some(path) = crate::semantic::file_url_path(url) else { return false };
    let b = path.as_bytes();
    if path.chars().any(|c| c.is_control()) || path.starts_with("\\\\") || !(b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'\\') {
        return false;
    }
    if path[2..].contains(':') {
        return false;
    }
    judge(Path::new(&path))
}

/// One posted item: an environment result, or a view event. The charge (when there is one) is
/// released when the item is dropped, which is when the app has drained it.
pub struct Posted {
    pub env: Option<(Store, Result<(), String>)>,
    pub event: Option<Event>,
    pub charge: Option<Charge>,
}

impl Posted {
    pub fn env(store: Store, result: Result<(), String>) -> Posted {
        Posted { env: Some((store, result)), event: None, charge: None }
    }
    pub fn event(event: Event, charge: Option<Charge>) -> Posted {
        Posted { env: None, event: Some(event), charge }
    }
}

pub type Sink = std::sync::mpsc::Sender<Posted>;

// --- Caps on everything a page can post (preflight B5) ---

/// The largest `postMessage` body the host queues.
pub const MAX_MESSAGE: usize = 64 * 1024;
/// Messages of one view that may sit in the queue.
pub const MAX_QUEUED_MESSAGES: usize = 50;
/// A title, in characters.
pub const MAX_TITLE: usize = 1024;
/// A URL, in bytes.
pub const MAX_URL: usize = 8192;
/// A favicon's encoded size and side.
pub const MAX_FAVICON: usize = 256 * 1024;
pub const MAX_FAVICON_SIDE: u32 = 256;
/// Events and bytes of one view that may sit in the queue.
pub const MAX_QUEUED_EVENTS: usize = 256;
pub const MAX_QUEUED_BYTES: usize = 1024 * 1024;
/// New-window requests: per second and pending.
pub const NEW_WINDOWS_PER_SECOND: usize = 4;
pub const MAX_PENDING_NEW_WINDOWS: usize = 8;

/// A page's title made safe to show: control characters become a space, then THE display
/// sanitizer (`inert_files::display_text`, the 13w ruling) removes every other hidden character
/// (bidi, zero width, separators); at most 1024 characters.
pub fn clean_title(t: &str) -> String {
    let spaced: String = t.chars().take(MAX_TITLE.saturating_mul(4)).map(|c| if c.is_control() { ' ' } else { c }).collect();
    crate::inert_files::display_text(&spaced, MAX_TITLE, false)
}

/// A URL a page gave, cut to 8192 bytes at a character boundary; the flag says it was cut (a cut
/// URL is never stored whole and never navigated).
pub fn cap_url(u: &str) -> (String, bool) {
    if u.len() <= MAX_URL {
        return (u.to_string(), false);
    }
    let mut end = MAX_URL;
    while !u.is_char_boundary(end) {
        end -= 1;
    }
    (u[..end].to_string(), true)
}

/// A favicon the host keeps: at most 256 KiB encoded and decoding (header only) to at most 256 by
/// 256.
pub fn favicon_ok(bytes: &[u8]) -> bool {
    if bytes.is_empty() || bytes.len() > MAX_FAVICON {
        return false;
    }
    image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .ok()
        .and_then(|r| r.into_dimensions().ok())
        .is_some_and(|(w, h)| w > 0 && h > 0 && w <= MAX_FAVICON_SIDE && h <= MAX_FAVICON_SIDE)
}

/// `NewWindowRequested` limiter (P20): only a user-initiated request, at most 4 a second, never
/// more than 8 pending.
#[derive(Debug, Default)]
pub struct NewWindowLimiter {
    window_start: Option<Instant>,
    in_window: usize,
}

impl NewWindowLimiter {
    pub fn allow(&mut self, now: Instant, user_initiated: bool, pending: usize) -> bool {
        if !user_initiated || pending >= MAX_PENDING_NEW_WINDOWS {
            return false;
        }
        match self.window_start {
            Some(t) if now.duration_since(t) < Duration::from_secs(1) => {}
            _ => {
                self.window_start = Some(now);
                self.in_window = 0;
            }
        }
        if self.in_window >= NEW_WINDOWS_PER_SECOND {
            return false;
        }
        self.in_window += 1;
        true
    }
}

/// What one view has queued: events, bytes and messages. Shared by the view's handlers and every
/// `Charge` they make.
#[derive(Debug, Default)]
pub struct ViewGate {
    events: AtomicUsize,
    bytes: AtomicUsize,
    messages: AtomicUsize,
    new_windows: AtomicUsize,
    dropped: AtomicUsize,
}

/// A queued item's claim on its view's gate; dropping it (the app drained the item, or the channel
/// is gone) gives the room back.
#[derive(Debug)]
pub struct Charge {
    gate: Arc<ViewGate>,
    bytes: usize,
    message: bool,
    new_window: bool,
}

impl Drop for Charge {
    fn drop(&mut self) {
        self.gate.events.fetch_sub(1, Ordering::AcqRel);
        self.gate.bytes.fetch_sub(self.bytes, Ordering::AcqRel);
        if self.message {
            self.gate.messages.fetch_sub(1, Ordering::AcqRel);
        }
        if self.new_window {
            self.gate.new_windows.fetch_sub(1, Ordering::AcqRel);
        }
    }
}

impl ViewGate {
    pub fn new() -> Arc<ViewGate> {
        Arc::new(ViewGate::default())
    }

    /// Room for `event`? A droppable event over the caps is refused and counted (the newest is
    /// dropped: a std channel cannot drop its oldest); an event that must not be dropped is always
    /// admitted. The Charge returned is released on drop.
    pub fn admit(self: &Arc<Self>, event: &Event) -> Result<Charge, ()> {
        let weight = event.weight();
        let message = matches!(event, Event::Message { .. });
        let new_window = matches!(event, Event::NewWindow { .. });
        if event.droppable() {
            let over = self.events.load(Ordering::Acquire) >= MAX_QUEUED_EVENTS
                || self.bytes.load(Ordering::Acquire) + weight > MAX_QUEUED_BYTES
                || (message && self.messages.load(Ordering::Acquire) >= MAX_QUEUED_MESSAGES)
                || (message && weight > MAX_MESSAGE + MAX_URL)
                || (new_window && self.new_windows.load(Ordering::Acquire) >= MAX_PENDING_NEW_WINDOWS);
            if over {
                self.dropped.fetch_add(1, Ordering::AcqRel);
                return Err(());
            }
        }
        self.events.fetch_add(1, Ordering::AcqRel);
        self.bytes.fetch_add(weight, Ordering::AcqRel);
        if message {
            self.messages.fetch_add(1, Ordering::AcqRel);
        }
        if new_window {
            self.new_windows.fetch_add(1, Ordering::AcqRel);
        }
        Ok(Charge { gate: self.clone(), bytes: weight, message, new_window })
    }

    /// New-window requests queued now.
    pub fn new_windows_pending(&self) -> usize {
        self.new_windows.load(Ordering::Acquire)
    }

    /// Events queued now.
    pub fn queued(&self) -> usize {
        self.events.load(Ordering::Acquire)
    }

    /// Events dropped since the last call.
    pub fn take_dropped(&self) -> usize {
        self.dropped.swap(0, Ordering::AcqRel)
    }
}

/// Schemes a `window.open` may carry into a new tab (P20; 13x2's gate replaces the list).
pub fn new_window_scheme_ok(url: &str) -> bool {
    matches!(scheme_of(url).as_str(), "http" | "https" | "iterm2-about")
}

/// Whether `url`'s scheme is in `allowed` (ignoring ASCII case): a view that lists schemes loads only those.
pub fn scheme_ok(url: &str, allowed: &[String]) -> bool {
    let s = scheme_of(url);
    !s.is_empty() && allowed.iter().any(|a| a.eq_ignore_ascii_case(&s))
}

/// Whether the URI WebView2 reports for a navigation is the one Tandem asked for: equal, or equal
/// once WebView2's own added `/` after a bare host is ignored (P19).
pub fn same_navigation_uri(requested: &str, started: &str) -> bool {
    let norm = |u: &str| {
        let u = u.trim();
        let after = u.find("://").map(|i| i + 3);
        match after {
            Some(a) if !u[a..].contains(['/', '?', '#']) => format!("{u}/"),
            _ => u.to_string(),
        }
    };
    norm(requested) == norm(started)
}

// --- The bridge ---

/// iTerm2's `iterm2Invoke.js` with `window.webkit.messageHandlers.iterm2Invoke.postMessage`
/// replaced by `window.chrome.webview.postMessage` (the misspelled `messgeToPost` is iTerm2's), and
/// `window.onerror` reporting by a `{"error": msg}` message where iTerm2 calls `alert` (WebView2
/// has no alert delegate).
pub const INVOKE_JS: &str = "function iterm2Invoke(invocation, callback) {\n    var messgeToPost = {'invocation': invocation,\n                        'callback': callback};\n    window.chrome.webview.postMessage(messgeToPost);\n}\nwindow.onerror = function(msg) {\n    window.chrome.webview.postMessage({'error': String(msg)});\n};\n";

/// ToolWebView's logger script (`TWV:62-79`) with the same replacement: `console.log` posts its
/// arguments as strings, `window.onerror` its message. The shapes are `{console: [string...]}` and
/// `{error: string}` (`logger_message`).
pub const LOGGER_JS: &str = "(function() {\n  const oldLog = console.log;\n  console.log = function(...args) {\n    window.chrome.webview.postMessage({'console': args.map(String)});\n    oldLog.apply(console, args);\n  };\n})();\nwindow.onerror = function(message, source, lineno, colno, error) {\n    window.chrome.webview.postMessage({'error': String(message) + ' ' + String(source) + ':' + lineno + ':' + colno});\n};\n";

const MAX_INVOKE: usize = 16 * 1024;
const MAX_INVOCATION: usize = 8 * 1024;
const MAX_LOGGER_TEXT: usize = 2048;

fn callback_name_ok(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty()
        && b.len() <= 128
        && (b[0].is_ascii_alphabetic() || b[0] == b'_' || b[0] == b'$')
        && b[1..].iter().all(|c| c.is_ascii_alphanumeric() || *c == b'_' || *c == b'$' || *c == b'.')
}

/// The message `iterm2Invoke` posts: a closed set (B5). At most 16 KiB; an object whose only keys
/// are `invocation` (a string of at most 8 KiB) and `callback` (a plain name or dotted path of at
/// most 128 characters). Anything else is None.
pub fn invoke_request(json: &str) -> Option<(Option<String>, Option<String>)> {
    if json.len() > MAX_INVOKE {
        return None;
    }
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let o = v.as_object()?;
    let (mut invocation, mut callback) = (None, None);
    for (k, val) in o {
        let s = val.as_str()?;
        match k.as_str() {
            "invocation" if s.len() <= MAX_INVOCATION => invocation = Some(s.to_string()),
            "callback" if callback_name_ok(s) => callback = Some(s.to_string()),
            _ => return None,
        }
    }
    Some((invocation, callback))
}

/// A tool view's logger message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoggerMessage {
    /// `console.log` arguments.
    Console(Vec<String>),
    /// A script error.
    Error(String),
}

/// The logger script's message, a closed set: `{console: [string...]}` or `{error: string}`, each
/// text at most 2 KiB, the whole at most 16 KiB. The text is for its byte length only: it is
/// never logged (B8).
pub fn logger_message(json: &str) -> Option<LoggerMessage> {
    if json.len() > MAX_INVOKE {
        return None;
    }
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let o = v.as_object()?;
    if o.len() != 1 {
        return None;
    }
    if let Some(list) = o.get("console") {
        let items = list.as_array()?;
        let mut out = Vec::new();
        for i in items {
            let s = i.as_str()?;
            if s.len() > MAX_LOGGER_TEXT {
                return None;
            }
            out.push(s.to_string());
        }
        return Some(LoggerMessage::Console(out));
    }
    let e = o.get("error")?.as_str()?;
    (e.len() <= MAX_LOGGER_TEXT).then(|| LoggerMessage::Error(e.to_string()))
}

/// The reply script for a successful call (`WVW:237-268`).
pub fn return_script(callback: &str, value_json: &str) -> String {
    format!("try {{ {callback}({value_json}); }} catch(err) {{ alert('Caught error while sending return value to javascript: ' + err.message + '\\n' + err.stack); }}")
}

/// The reply script for a failed call: throws the message in the page.
pub fn throw_script(message: &str) -> String {
    format!("throw {}", serde_json::to_string(message).unwrap_or_else(|_| "\"\"".into()))
}

/// A rectangle in physical pixels as (left, top, right, bottom): rounded, never negative in width
/// or height, and an unusable number is zero.
pub fn physical_bounds(r: Rect) -> (i32, i32, i32, i32) {
    let n = |v: f32| if v.is_finite() { v.round().clamp(-1.0e6, 1.0e6) as i32 } else { 0 };
    let (left, top) = (n(r.x), n(r.y));
    (left, top, n(r.x + r.w).max(left), n(r.y + r.h).max(top))
}

/// What one NavigationStarting asks (the synchronous verdict, preflight stable interfaces).
#[derive(Clone, Copy, Debug)]
pub struct NavInfo<'a> {
    pub target: &'a str,
    /// The URL of the page that started the navigation, when WebView2 says.
    pub source: Option<&'a str>,
    pub user: bool,
    pub redirect: bool,
    /// Tandem's own load (`navigate_tandem`, `navigate_to_string`).
    pub by_tandem: bool,
}

/// The view's navigation policy, from its `ViewSetup`.
#[derive(Clone, Debug, Default)]
pub struct NavPolicy {
    /// Every navigation passes `navigation_allowed` (every view sets it, preflight B4c).
    pub guard_local_schemes: bool,
    /// Only these schemes may load (a Tandem load is exempt); None allows any the guard allows.
    pub allowed_schemes: Option<Vec<String>>,
}

/// The whole decision of the one NavigationStarting handler: true lets the navigation go, false
/// cancels it. `file_ok` judges a `file:` target (the one path judge); it runs only when the guard
/// asks, since it touches the disk.
pub fn nav_verdict(info: &NavInfo, policy: &NavPolicy, file_ok: &dyn Fn(&str) -> bool) -> bool {
    if let Some(list) = &policy.allowed_schemes {
        if !info.by_tandem && !scheme_ok(info.target, list) {
            return false;
        }
    }
    if !policy.guard_local_schemes {
        return true;
    }
    let file_ok = scheme_of(info.target) == "file" && file_ok(info.target);
    navigation_allowed(info.source, info.target, info.user, info.redirect, info.by_tandem, file_ok)
}

/// Whether a refused navigation is offered to the app: a person's own click on a link (not a
/// script's navigation, not a redirect, not Tandem's load) that the view's policy cancelled.
pub fn refused_click(info: &NavInfo, allowed: bool) -> bool {
    !allowed && info.user && !info.redirect && !info.by_tandem
}

// --- Focus (Task 1a) ---

/// What a `WindowEvent::Focused` means while web views exist. A click into a web view moves
/// keyboard focus to its child HWND and winit reports `Focused(false)` for the parent even though
/// the window is still the foreground window; in iTerm2 the web view is inside the key window, so
/// that is not a resignation (`PseudoTerminal.m:5421-5500` runs only when the window resigns key).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FocusStep {
    /// A real gain or loss: today's path.
    Normal,
    /// A loss whose foreground root is this window: focus went to its own child.
    IntoOwnChild,
    /// A gain while a child held the focus: the loss was never reported, so no gain is either.
    ReturnFromChild,
}

/// The step for one focus event of the window `window_hwnd`. `foreground_root` is
/// `GetAncestor(GetForegroundWindow(), GA_ROOT)`.
pub fn focus_step(gained: bool, window_hwnd: isize, foreground_root: Option<isize>, held: bool) -> FocusStep {
    match (gained, foreground_root) {
        (false, Some(root)) if root == window_hwnd => FocusStep::IntoOwnChild,
        (true, _) if held => FocusStep::ReturnFromChild,
        _ => FocusStep::Normal,
    }
}

/// A held focus whose foreground root is no longer this window: the real loss, run late. No
/// foreground window at all (a transition) answers false and is asked again on the next check.
pub fn held_focus_lost(window_hwnd: isize, foreground_root: Option<isize>) -> bool {
    foreground_root.is_some_and(|root| root != window_hwnd)
}

/// The focus reports a change of the active session owes (`PTYTab.m:955-960`: every session of
/// the key window hears it). Each pair is (pane id, is a browser pane). A terminal pane that
/// hands focus to a browser pane is told it lost focus, one that takes it back is told it
/// gained it; terminal to terminal is no change (the pane's own path reports nothing today).
pub fn pane_focus_reports(prev: Option<(crate::layout::PaneId, bool)>, next: Option<(crate::layout::PaneId, bool)>) -> Vec<(crate::layout::PaneId, bool)> {
    match (prev, next) {
        (Some((from, false)), Some((_, true))) => vec![(from, false)],
        (Some((_, true)), Some((to, false))) => vec![(to, true)],
        _ => Vec::new(),
    }
}

/// The fixed Tandem-only host that program and model content is served under (never resolvable).
pub const LOCAL_HOST: &str = "tandem-local.invalid";

/// One document Tandem serves to a view itself: a script's popover HTML or the icon wrapper page.
/// The class rule (Task 4 fix 1) replaces `NavigateToString` for program and model content: the view
/// navigates to this document's URL on `LOCAL_HOST`, the host's request filter hands every request
/// of the view to `answer`, and the content policy travels as a response header, so no markup is
/// parsed, scanned or changed on the way (a meta tag inserted after a doctype or a comment is
/// exactly the differential this removes). The URL has a random path, known before the load, so
/// the bridge's source check is an exact comparison and does not depend on what the runtime
/// reports for `NavigateToString` content.
#[derive(Debug)]
pub struct LocalDoc {
    url: String,
    html: String,
    csp: String,
    seen: bool,
    navigated: bool,
}

fn without_fragment(url: &str) -> &str {
    url.split('#').next().unwrap_or(url)
}

impl LocalDoc {
    /// A document of `html` served under the policy `csp`.
    pub fn new(html: String, csp: &str) -> LocalDoc {
        // A random host per document: each is its own origin (no shared storage between scripts).
        let url = format!("https://{}.{LOCAL_HOST}/{}/doc", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple());
        LocalDoc { url, html, csp: csp.to_string(), seen: false, navigated: false }
    }

    /// The URL to navigate to (`WebView::navigate_tandem`).
    pub fn url(&self) -> &str {
        &self.url
    }

    /// The answer to one request of the view: the document at its exact URL with the policy as a
    /// header; `data:` and `blob:` requests go on (they cannot reach a network or a disk, and the
    /// policy governs which may load); everything else is refused (403).
    pub fn answer(&self, request_url: &str) -> Answer {
        if request_url == self.url {
            return Answer::Respond {
                status: 200,
                mime: "text/html; charset=utf-8".to_string(),
                body: self.html.clone().into_bytes(),
                headers: vec![
                    ("Content-Security-Policy".to_string(), self.csp.clone()),
                    ("X-Content-Type-Options".to_string(), "nosniff".to_string()),
                    ("Cache-Control".to_string(), "no-store".to_string()),
                    ("Referrer-Policy".to_string(), "no-referrer".to_string()),
                    ("X-DNS-Prefetch-Control".to_string(), "off".to_string()),
                ],
            };
        }
        match scheme_of(request_url).as_str() {
            "data" | "blob" => Answer::Continue,
            _ => Answer::Block,
        }
    }

    /// A source the view reported. The document's own URL is the document; the blank page a new
    /// view starts on is nobody's and is ignored until the document is seen; any other source ends
    /// the trust for good.
    pub fn saw_source(&mut self, url: &str) {
        if self.navigated {
            return;
        }
        let base = without_fragment(url);
        if base == self.url {
            self.seen = true;
        } else if !(base == "about:blank" && !self.seen) {
            self.navigated = true;
        }
    }

    /// Whether a message's source is this document: its exact URL (a fragment link inside the page is
    /// not a navigation), reported by the view, and the view has not navigated since.
    pub fn trusts(&self, source: &str) -> bool {
        self.seen && !self.navigated && without_fragment(source) == self.url
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Final review I4 (class): a closing window's views are chosen by their parent handle, not by
    /// the tabs the window holds; a view whose pane now shows in another window moves there, any
    /// other closes (and is made again from its URL when its pane shows again).
    #[test]
    fn a_closing_window_takes_its_views_by_parent() {
        const A: isize = 0xA0;
        const B: isize = 0xB0;
        let views = [(1u64, A), (2, A), (3, B), (4, A), (5, 0)];
        let home = |p: u64| match p {
            1 => Some(B),
            2 => Some(A),
            3 => Some(A),
            _ => None,
        };
        let fates = closing_parent(&views, A, home);
        assert_eq!(fates, vec![(1, ParentFate::Move(B)), (2, ParentFate::Close), (4, ParentFate::Close)]);
        // Pane 3 shows in A's tabs but its view is a child of B: not A's to close.
        assert!(!fates.iter().any(|(p, _)| *p == 3));
        assert!(closing_parent(&views, 0, home).is_empty(), "no handle owns nothing");
    }

    fn yes(_: &Path) -> bool {
        true
    }

    /// Class ruling (Task 4 fix 1): program and model content is served by one function on a fixed
    /// Tandem-only origin, the policy is a response header, the markup is never parsed or changed.
    /// Task 5 fix r1 (isolation): each document is its own origin, so one script's popover cannot
    /// read another's localStorage, IndexedDB or cookies (the views share a persistent store).
    #[test]
    fn each_local_document_is_its_own_origin() {
        let host = |d: &LocalDoc| d.url().split('/').nth(2).unwrap().to_string();
        let (a, b) = (LocalDoc::new(String::new(), ""), LocalDoc::new(String::new(), ""));
        assert_ne!(host(&a), host(&b), "a random host per document");
        for d in [&a, &b] {
            let h = host(d);
            assert!(h.ends_with(&format!(".{LOCAL_HOST}")) && h.len() > LOCAL_HOST.len() + 8, "{h}");
        }
        assert_eq!(a.answer(&format!("https://{LOCAL_HOST}/{}", &a.url()[a.url().find("/doc").unwrap() - 32..])), Answer::Block, "the bare host is nobody's");
        assert_eq!(a.answer(&a.url().replace(&host(&a), &host(&b))), Answer::Block, "another document's origin is refused");
    }

    #[test]
    fn local_documents_get_their_policy_as_a_header() {
        let hostile = "<!-- --!><img src=\"https://x.test/p\"><!DOCTYPE html><script>fetch('https://x.test')</script><!--";
        let d = LocalDoc::new(hostile.to_string(), "default-src 'none'");
        assert!(d.url().starts_with("https://") && d.url().contains(".tandem-local.invalid/") && d.url().ends_with("/doc"), "{}", d.url());
        assert_ne!(d.url(), LocalDoc::new(String::new(), "").url(), "a random path per document");
        let Answer::Respond { status, mime, body, headers } = d.answer(d.url()) else { panic!("the document is served") };
        assert_eq!((status, mime.as_str()), (200, "text/html; charset=utf-8"));
        assert_eq!(body, hostile.as_bytes(), "the markup is served byte for byte, no meta inserted");
        let h = |n: &str| headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(n)).map(|(_, v)| v.as_str());
        assert_eq!(h("Content-Security-Policy"), Some("default-src 'none'"));
        assert_eq!(h("X-Content-Type-Options"), Some("nosniff"));
        assert_eq!(h("Cache-Control"), Some("no-store"));
        // Every other request of the view is refused (403); data and blob images cannot reach anything.
        for other in ["https://tandem-local.invalid/other/doc", &d.url().replace("/doc", "/other"), "https://x.test/p", "http://127.0.0.1/", "file:///C:/x", "https://tandem-local.invalid/", &format!("{}x", d.url())] {
            assert_eq!(d.answer(other), Answer::Block, "{other}");
        }
        assert_eq!(d.answer("data:image/png;base64,AAAA"), Answer::Continue);
    }

    #[test]
    fn local_document_source_is_the_exact_url() {
        let mut d = LocalDoc::new("x".into(), "");
        let url = d.url().to_string();
        assert!(!d.trusts(&url), "not seen yet");
        d.saw_source("about:blank");
        assert!(!d.trusts("about:blank") && !d.trusts(&url), "the initial blank page is nobody's");
        d.saw_source(&url);
        assert!(d.trusts(&url));
        assert!(d.trusts(&format!("{url}#top")), "a fragment link inside the page is not a navigation");
        for other in ["about:blank", "data:text/html,x", "https://evil.example/", "", &format!("{url}/x"), &url.replace("/doc", "/dox")] {
            assert!(!d.trusts(other), "{other}");
        }
        d.saw_source("https://evil.example/");
        assert!(!d.trusts(&url), "it navigated away");
        let mut e = LocalDoc::new("x".into(), "");
        e.saw_source("https://evil.example/");
        e.saw_source(e.url().to_string().as_str());
        assert!(!e.trusts(e.url()), "a first source that is not the document ends the trust for good");
    }

    #[test]
    fn runtime_outcome_follows_iterm2() {
        assert_eq!(runtime_outcome(Ok("128.0.2739.42".into())), Runtime::Found("128.0.2739.42".into()));
        assert_eq!(runtime_outcome(Ok(String::new())), Runtime::Missing);
        assert_eq!(runtime_outcome(Err(-2147024894)), Runtime::Missing);
        assert_eq!(fixed_runtime_folder("  ", &yes), None);
        assert_eq!(fixed_runtime_folder(r"C:\WV2\128", &yes), Some(PathBuf::from(r"C:\WV2\128")));
        assert_eq!(fixed_runtime_folder(r"\\srv\wv2", &yes), None);
    }

    /// B2: the hint is judged before it can name a program's folder.
    #[test]
    fn fixed_runtime_hint_is_judged() {
        let long = format!("C:\\{}", "a".repeat(300));
        for (hint, why) in [
            (r"WV2\128", "relative"),
            (r"\\srv\wv2", "UNC"),
            (r"C:\x\..\y", "a parent step"),
            (r"C:\x\.\y", "a dot step"),
            (long.as_str(), "300 characters"),
            ("C:\\x\u{7}y", "a control character"),
            (r"C:\x:stream", "a stream"),
            (r"\\?\C:\x", "verbatim"),
            (r"C:", "no folder"),
        ] {
            assert_eq!(fixed_runtime_folder(hint, &yes), None, "{why}");
        }
        // A junction or a mapped drive is the judge's: it says no.
        assert_eq!(fixed_runtime_folder(r"C:\junction\wv2", &|_| false), None);
        assert_eq!(refused_hint(r"C:\junction\wv2", &|_| false).unwrap_err(), "the folder is on a share, a mapped drive or behind a link");
        // Trimmed.
        assert_eq!(fixed_runtime_folder(r"  C:\WV2\128  ", &yes), Some(PathBuf::from(r"C:\WV2\128")));
        assert_eq!(fixed_runtime_folder("C:/WV2/128", &yes), Some(PathBuf::from("C:/WV2/128")));
    }

    /// B2: the hint and the user agent are bounded where they are written.
    #[test]
    fn fixed_runtime_hint_is_bounded_at_the_write() {
        let mut g = crate::globals::Globals::empty(Path::new("web-hint-bound-test.toml"));
        g.set("BrowserPluginPathHint", toml::Value::String(format!("C:\\{}", "a".repeat(300))));
        assert_eq!(g.string("BrowserPluginPathHint"), "");
        g.set("BrowserPluginPathHint", toml::Value::String("C:\\x\u{7}y".into()));
        assert_eq!(g.string("BrowserPluginPathHint"), "");
        g.set("BrowserPluginPathHint", toml::Value::String(r"C:\WV2\128".into()));
        assert_eq!(g.string("BrowserPluginPathHint"), r"C:\WV2\128");
        g.set("WebUserAgent", toml::Value::String(format!("A\r\nX-Evil: 1{}", "b".repeat(600))));
        let ua = g.string("WebUserAgent");
        assert!(!ua.contains(['\r', '\n']) && ua.chars().count() == 512, "{}", ua.len());
        // A file loaded with a bad value is bounded too.
        let dir = std::env::temp_dir().join(format!("tandem-hint-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let file = dir.join("g.toml");
        std::fs::write(&file, format!("BrowserPluginPathHint = \"C:\\\\{}\"\n", "a".repeat(400))).unwrap();
        let loaded = crate::globals::Globals::load(&file).unwrap();
        assert_eq!(loaded.string("BrowserPluginPathHint"), "");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn folders_and_options_follow_windows() {
        let base = Path::new(r"C:\Users\u\AppData\Local");
        assert_eq!(data_folder(base, Store::Default), PathBuf::from(r"C:\Users\u\AppData\Local\Tandem\WebView2\Default"));
        assert!(data_folder(base, Store::Browser).ends_with("Browser"));
        assert!(data_folder(base, Store::BrowserDevNull).ends_with("DevNull"));
        assert_eq!(EnvOptions::default().args, "");
        assert!(EnvOptions::default().custom_schemes.is_empty());
        assert!(!ViewSetup_in_private_default());
    }

    // `ViewSetup` is Windows code (`webhost_win`); its InPrivate default is checked there too.
    #[allow(non_snake_case)]
    fn ViewSetup_in_private_default() -> bool {
        #[cfg(windows)]
        {
            crate::webhost_win::ViewSetup::default().in_private
        }
        #[cfg(not(windows))]
        {
            false
        }
    }

    /// P4: Dev Null folders are per run and swept when their process is gone.
    #[test]
    fn dev_null_folders_are_per_run() {
        let base = Path::new(r"C:\L");
        let f = dev_null_folder(base, 4242, 0xdead_beef);
        assert_eq!(f, PathBuf::from(r"C:\L\Tandem\WebView2\Tmp\DevNull-4242-deadbeef"));
        assert_eq!(dev_null_pid("DevNull-4242-deadbeef"), Some(4242));
        for bad in ["DevNull-x-deadbeef", "DevNull-1-dead", "DevNull--deadbeef", "Other-1-deadbeef", "DevNull-1-deadbeeg", "DevNull-1"] {
            assert_eq!(dev_null_pid(bad), None, "{bad}");
        }
        let names: Vec<String> = ["DevNull-1-00000001", "DevNull-2-00000002", "DevNull-3-00000003", "keep"].map(String::from).to_vec();
        // 1 is alive, 2 is dead, 3 is this process (always removed at exit).
        let gone = stale_dev_null(&names, 3, &|p| p == 1);
        assert_eq!(gone, vec!["DevNull-2-00000002".to_string(), "DevNull-3-00000003".to_string()]);
    }

    #[test]
    fn user_agent_follows_iterm2() {
        assert_eq!(user_agent("", "Mozilla/5.0 (Windows NT 10.0) Edg/128"), "Mozilla/5.0 (Windows NT 10.0) Edg/128 Tandem");
        assert_eq!(user_agent("X/1", "ignored"), "X/1");
        let s = crate::schema::find_global("WebUserAgent").expect("WebUserAgent");
        assert_eq!(s.default, Some(toml::Value::String("Mozilla/5.0 (Macintosh; Intel Mac OS X 14_3) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.2 Safari/605.1.15".into())));
    }

    #[test]
    fn bridge_texts_follow_iterm2() {
        assert_eq!(invoke_request(r#"{"invocation":"f()","callback":"cb"}"#), Some((Some("f()".into()), Some("cb".into()))));
        assert_eq!(invoke_request(r#"{"callback":"cb"}"#), Some((None, Some("cb".into()))));
        assert_eq!(invoke_request("x"), None);
        assert_eq!(return_script("cb", "1"), "try { cb(1); } catch(err) { alert('Caught error while sending return value to javascript: ' + err.message + '\\n' + err.stack); }");
        assert_eq!(throw_script("E \"q\""), r#"throw "E \"q\"""#);
        assert!(INVOKE_JS.contains("window.chrome.webview.postMessage(messgeToPost)"));
        assert!(!INVOKE_JS.contains("webkit") && !LOGGER_JS.contains("webkit"));
        assert!(LOGGER_JS.contains("window.chrome.webview.postMessage"));
        assert_eq!(physical_bounds(Rect { x: 10.4, y: 0.6, w: 100.0, h: -3.0 }), (10, 1, 110, 1));
        assert_eq!(physical_bounds(Rect { x: f32::NAN, y: 2.0, w: 5.0, h: 5.0 }), (0, 2, 0, 7));
        assert_eq!(response_headers("text/html", &[("Content-Security-Policy".into(), "sandbox".into())]), "Content-Type: text/html\r\nContent-Security-Policy: sandbox");
        assert_eq!(
            response_headers("text/html", &[("X-A".into(), "v\r\nSet-Cookie: x=1".into()), ("X-B".into(), "ok".into()), ("Bad Name".into(), "v".into()), ("N\n".into(), "v".into())]),
            "Content-Type: text/html\r\nX-B: ok"
        );
        assert_eq!(response_headers("text/html\r\nX: 1", &[]), "Content-Type: application/octet-stream");
    }

    /// B5: the invoke message is a closed set.
    #[test]
    fn invoke_request_is_a_closed_set() {
        for bad in [
            r#"{"invocation":"f()","callback":"cb","x":1}"#,
            r#"{"invocation":5}"#,
            r#"{"callback":"a b"}"#,
            r#"{"callback":"1a"}"#,
            r#"{"callback":"a();alert(1)"}"#,
            r#"{"callback":""}"#,
            r#"["invocation"]"#,
            r#"{"invocation":null}"#,
            r#"{"error":"x"}"#,
        ] {
            assert_eq!(invoke_request(bad), None, "{bad}");
        }
        let long_cb = format!(r#"{{"callback":"{}"}}"#, "a".repeat(129));
        assert_eq!(invoke_request(&long_cb), None);
        let ok_cb = format!(r#"{{"callback":"{}"}}"#, "a".repeat(128));
        assert!(invoke_request(&ok_cb).is_some());
        let big_inv = format!(r#"{{"invocation":"{}"}}"#, "a".repeat(8 * 1024 + 1));
        assert_eq!(invoke_request(&big_inv), None);
        let huge = format!(r#"{{"invocation":"{}"}}"#, "a".repeat(17 * 1024));
        assert_eq!(invoke_request(&huge), None);
        assert_eq!(invoke_request(r#"{"callback":"window.cb_1.$x"}"#), Some((None, Some("window.cb_1.$x".into()))));
        // The logger accepts only its two shapes.
        assert_eq!(logger_message(r#"{"console":["a","b"]}"#), Some(LoggerMessage::Console(vec!["a".into(), "b".into()])));
        assert_eq!(logger_message(r#"{"error":"boom"}"#), Some(LoggerMessage::Error("boom".into())));
        for bad in [r#"{"console":[1]}"#, r#"{"console":["a"],"error":"b"}"#, r#"{"other":"x"}"#, r#"{"error":5}"#, "[]", "nope"] {
            assert_eq!(logger_message(bad), None, "{bad}");
        }
        assert_eq!(logger_message(&format!(r#"{{"error":"{}"}}"#, "e".repeat(2049))), None);
    }

    #[test]
    fn navigation_guard_follows_the_rules() {
        let n = |s: Option<&str>, t: &str, user, redirect, tandem| navigation_allowed(s, t, user, redirect, tandem, true);
        assert!(!n(Some("https://evil.test/"), "iterm2-file:///C:/Users/u/.ssh/id_ed25519", false, false, false));
        assert!(n(Some("https://evil.test/"), "iterm2-file:///C:/Users/u/.ssh/id_ed25519", true, false, false));
        assert!(!n(Some("https://evil.test/"), "iterm2-ssh://box/dev/zero", false, false, false));
        assert!(!n(Some("https://evil.test/"), "file:///C:/Windows/", false, false, false));
        assert!(n(Some("iterm2-about:welcome"), "iterm2-file:///C:/", false, false, false));
        assert!(!n(Some("iterm2-file:///C:/T/a.html"), "https://evil.test/?d=x", false, false, false));
        assert!(!n(Some("iterm2-file:///C:/T/a.html"), "iterm2-file:///C:/T/b.txt", false, false, false));
        assert!(n(Some("iterm2-file:///C:/T/"), "iterm2-file:///C:/T/b.txt", true, false, false));
        assert!(!n(Some("iterm2-ssh://box/a.html"), "https://evil.test/", false, false, false));
        assert!(!n(Some("https://a/"), "iterm2-file:///C:/x", true, true, false));
        assert!(!n(Some("https://a/"), "file:///C:/x", false, true, true));
        assert!(n(None, "iterm2-ssh://box/a.png", false, false, true));
        assert!(n(Some("https://a/"), "https://b/", false, false, false));
        assert!(!n(Some("https://evil.test/"), "FILE:///C:/x", false, false, false));
        // B4: a file: target that is not file_ok is refused for every source and initiator.
        for (user, redirect, tandem, source) in [(false, false, false, Some("iterm2-about:welcome")), (true, false, false, Some("https://a/")), (false, false, true, None), (true, false, true, Some("iterm2-about:history"))] {
            assert!(!navigation_allowed(source, "file://srv/share/a", user, redirect, tandem, false), "{user} {tandem}");
            assert!(!navigation_allowed(source, "FILE:///Z:/a", user, redirect, tandem, false));
        }
        assert!(navigation_allowed(Some("iterm2-about:welcome"), "FILE:///C:/ok", false, false, false, true));
        assert!(navigation_allowed(None, "file:///C:/ok", true, false, false, true));
        assert!(navigation_allowed(None, "file:///C:/ok", false, false, true, true));
        // file_ok does not matter for another scheme.
        assert!(navigation_allowed(Some("https://a/"), "https://b/", false, false, false, false));
    }

    #[test]
    fn file_urls_are_judged() {
        let local_c = |p: &Path| p.to_string_lossy().starts_with("C:");
        assert!(!file_url_local("file://srv/share/a", &yes));
        assert!(!file_url_local("file:////srv/s/a", &yes));
        assert!(!file_url_local("file:///Z:/a", &local_c), "a remote drive: the judge says no");
        assert!(file_url_local("FILE:///C:/ok", &local_c));
        assert!(file_url_local("file://localhost/C:/ok", &local_c));
        assert!(!file_url_local("file:///C:/ok:stream", &local_c));
        assert!(!file_url_local("file:///C:/ok%00x", &local_c));
        assert!(!file_url_local("file://./C:/x", &yes));
        assert!(!file_url_local("https://a/", &yes));
        assert!(!file_url_local("file:///C:/..%5C..%5C", &|_| false));
    }

    #[test]
    fn message_caps_follow_the_rules() {
        assert_eq!(clean_title("a\u{7}b\u{202E}c\nd"), "a bc d");
        assert_eq!(clean_title(&"x".repeat(2000)).chars().count(), MAX_TITLE);
        let (u, cut) = cap_url("https://a/");
        assert_eq!((u.as_str(), cut), ("https://a/", false));
        let long = format!("https://a/{}", "\u{e9}".repeat(9000));
        let (u, cut) = cap_url(&long);
        assert!(cut && u.len() <= MAX_URL && long.starts_with(&u));
        assert!(!favicon_ok(&[]));
        assert!(!favicon_ok(&vec![0u8; MAX_FAVICON + 1]));
        assert!(!favicon_ok(b"not an image"));
        // A 1 by 1 PNG.
        use base64::Engine as _;
        let png = base64::engine::general_purpose::STANDARD.decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==").unwrap();
        assert!(favicon_ok(&png));
        // A header that no longer says 1 by 1 is refused (too large, or not decodable).
        let mut big = png.clone();
        big[16..20].copy_from_slice(&4000u32.to_be_bytes());
        big[20..24].copy_from_slice(&4000u32.to_be_bytes());
        assert!(!favicon_ok(&big));
        let mut l = NewWindowLimiter::default();
        let t = Instant::now();
        assert!(!l.allow(t, false, 0), "a page's own window.open is not a person's click");
        for _ in 0..NEW_WINDOWS_PER_SECOND {
            assert!(l.allow(t, true, 0));
        }
        assert!(!l.allow(t, true, 0), "at most 4 a second");
        assert!(l.allow(t + Duration::from_millis(1100), true, 0), "the next second");
        assert!(!l.allow(t + Duration::from_secs(5), true, MAX_PENDING_NEW_WINDOWS), "never queued past 8");
    }

    /// B5: a view's queue is bounded; events that must not be lost are always admitted.
    #[test]
    fn event_queue_is_bounded() {
        let gate = ViewGate::new();
        let (tx, rx) = std::sync::mpsc::channel::<Posted>();
        let post = |e: Event| {
            if let Ok(c) = gate.admit(&e) {
                let _ = tx.send(Posted::event(e, Some(c)));
            }
        };
        for i in 0..10_000 {
            post(Event::Message { view: 1, json: format!("{{\"n\":{i}}}"), source: "https://a/".into() });
        }
        assert!(gate.queued() <= MAX_QUEUED_EVENTS, "{}", gate.queued());
        assert!(gate.queued() <= MAX_QUEUED_MESSAGES, "messages are capped at 50: {}", gate.queued());
        assert!(gate.take_dropped() >= 9_950);
        // A lifecycle event is admitted even when the queue is full.
        post(Event::Created(1));
        post(Event::Navigated { view: 1, ok: true, status: 200, url: "https://a/".into(), nav: 1 });
        let mut kinds = 0;
        while let Ok(p) = rx.try_recv() {
            if matches!(p.event, Some(Event::Created(1)) | Some(Event::Navigated { .. })) {
                kinds += 1;
            }
        }
        assert_eq!(kinds, 2);
        assert_eq!(gate.queued(), 0, "draining gives the room back");
        // Bytes: 1 MiB of titles.
        let gate = ViewGate::new();
        let mut held = Vec::new();
        for _ in 0..2000 {
            if let Ok(c) = gate.admit(&Event::Title(1, "t".repeat(MAX_TITLE))) {
                held.push(c);
            }
        }
        assert!(held.len() <= MAX_QUEUED_EVENTS);
        let bytes: usize = held.len() * MAX_TITLE;
        assert!(bytes <= MAX_QUEUED_BYTES);
        // A message over 64 KiB (plus its source) is never queued.
        let gate = ViewGate::new();
        assert!(gate.admit(&Event::Message { view: 1, json: "x".repeat(MAX_MESSAGE + MAX_URL + 1), source: String::new() }).is_err());
    }

    #[test]
    fn small_rules_follow_the_ruling() {
        assert!(same_navigation_uri("https://a", "https://a/"));
        assert!(same_navigation_uri("https://a/x", "https://a/x"));
        assert!(!same_navigation_uri("https://a/x", "https://a/y"));
        assert!(!same_navigation_uri("https://a", "https://b/"));
        assert!(new_window_scheme_ok("HTTPS://a/") && new_window_scheme_ok("iterm2-about:welcome") && new_window_scheme_ok("http://a"));
        for bad in ["file:///C:/x", "javascript:alert(1)", "data:text/html,x", "ms-msdt:id", "about:blank", ""] {
            assert!(!new_window_scheme_ok(bad), "{bad}");
        }
        let list = vec!["http".to_string(), "HTTPS".to_string()];
        assert!(scheme_ok("https://a/", &list) && scheme_ok("HTTP://a/", &list));
        assert!(!scheme_ok("file:///C:/", &list) && !scheme_ok("ms-msdt:id", &list) && !scheme_ok("noscheme", &list));
    }

    /// Every `Event` carries its view, and the droppable set matches the rule.
    #[test]
    fn events_know_their_view() {
        assert_eq!(Event::Created(7).view(), 7);
        assert_eq!(Event::Request { view: 9, token: 1, url: String::new(), context: 0, top_url: String::new() }.view(), 9);
        assert!(!Event::Request { view: 9, token: 1, url: String::new(), context: 0, top_url: String::new() }.droppable());
        assert!(!Event::ScriptDone { view: 1, token: 1, json: Ok("2".into()) }.droppable());
        assert!(Event::Message { view: 1, json: String::new(), source: String::new() }.droppable());
    }

    /// Review Focus 11: a focus loss into this window's own child is not a window focus loss.
    #[test]
    fn focus_into_web_view_follows_iterm2() {
        assert_eq!(focus_step(false, 100, Some(100), false), FocusStep::IntoOwnChild);
        assert_eq!(focus_step(false, 100, Some(200), false), FocusStep::Normal);
        assert_eq!(focus_step(false, 100, None, false), FocusStep::Normal);
        assert_eq!(focus_step(true, 100, Some(100), true), FocusStep::ReturnFromChild);
        assert_eq!(focus_step(true, 100, Some(100), false), FocusStep::Normal);
        assert!(held_focus_lost(100, Some(300)));
        assert!(!held_focus_lost(100, Some(100)));
        // No foreground window is a transition, not a loss.
        assert!(!held_focus_lost(100, None));
    }

    #[test]
    fn pane_focus_reports_follow_iterm2() {
        assert_eq!(pane_focus_reports(Some((1, false)), Some((2, true))), vec![(1, false)]);
        assert_eq!(pane_focus_reports(Some((2, true)), Some((1, false))), vec![(1, true)]);
        assert_eq!(pane_focus_reports(Some((1, false)), Some((3, false))), Vec::<(u64, bool)>::new());
        assert_eq!(pane_focus_reports(Some((2, true)), Some((4, true))), Vec::<(u64, bool)>::new());
        assert_eq!(pane_focus_reports(None, Some((1, false))), Vec::<(u64, bool)>::new());
    }

    /// Final review round 1, item 3: the policy function itself cancels a page's navigation into a
    /// local scheme. Called with the guard off it lets the same navigation through, so the test
    /// fails when the guard is removed.
    /// Review fix (Task 3, Important 2): a link a person clicked that the view refused is offered to
    /// the app (which judges it with `guard_open`); a script's or a redirect's refusal is not, and
    /// neither is a navigation Tandem itself made.
    #[test]
    fn a_refused_click_is_offered_to_the_app() {
        let info = |user: bool, by_tandem: bool| NavInfo { target: "https://a.example/", source: None, user, redirect: false, by_tandem };
        assert!(refused_click(&info(true, false), false));
        assert!(!refused_click(&info(false, false), false), "a script's navigation");
        assert!(!refused_click(&info(true, true), false), "Tandem's own load");
        assert!(!refused_click(&info(true, false), true), "an allowed navigation is not refused");
        let redirect = NavInfo { redirect: true, ..info(true, false) };
        assert!(!refused_click(&redirect, false), "a redirect is no click");
        let ev = Event::Refused { view: 3, url: "https://a.example/".into(), user: true };
        assert_eq!(ev.view(), 3);
        assert!(ev.droppable());
        assert_eq!(ev.weight(), 18);
    }

    #[test]
    fn nav_verdict_guard_cancels_local_navigations_from_a_page() {
        let from_page = |target| NavInfo { target, source: Some("https://example.test/"), user: false, redirect: false, by_tandem: false };
        let yes = |_: &str| true;
        let guarded = NavPolicy { guard_local_schemes: true, allowed_schemes: None };
        let open = NavPolicy::default();
        for target in ["file:///C:/Windows/", "iterm2-file:///x", "iterm2-ssh:host"] {
            assert!(!nav_verdict(&from_page(target), &guarded, &yes), "{target} from a web page");
            assert!(nav_verdict(&from_page(target), &open, &yes), "{target} passes with no guard (the guard is what cancels)");
        }
        assert!(nav_verdict(&from_page("https://example.test/next"), &guarded, &yes));
        // A file target that the path judge refuses is cancelled even for a person's click.
        let click = NavInfo { target: "file://server/share/x", source: None, user: true, redirect: false, by_tandem: false };
        assert!(!nav_verdict(&click, &guarded, &|_| false));
        // A redirect into a local scheme is cancelled even for Tandem's own load.
        let redirected = NavInfo { target: "file:///C:/x", source: None, user: false, redirect: true, by_tandem: true };
        assert!(!nav_verdict(&redirected, &guarded, &yes));
        // The scheme list cancels what it does not name, except Tandem's own load.
        let only_https = NavPolicy { guard_local_schemes: true, allowed_schemes: Some(vec!["https".into()]) };
        assert!(!nav_verdict(&from_page("http://example.test/"), &only_https, &yes));
        let own = NavInfo { target: "http://tandem.test/", source: None, user: false, redirect: false, by_tandem: true };
        assert!(nav_verdict(&own, &only_https, &yes));
    }
}
