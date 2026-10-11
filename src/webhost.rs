//! The WebView2 host's pure model (milestone 13x, Task 1): the runtime check's outcome, the user
//! data folders, environment options, the events a view posts, the one navigation rule, the
//! bridge's message and reply texts, the caps on everything a page can post, and bounds math.
//! Nothing here touches COM (`webhost_win`) or the App (`app::webhost_app`). Web content is
//! untrusted: every size, rate and shape a page controls is bounded here, before it is queued.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use crate::layout::Rect;

pub type ViewId = u64;

/// The key gate's table (13x2 Task 6): `take(vk, mods)` says whether Tandem takes a key the page
/// raised. Read synchronously on the UI thread inside the COM handler, so it holds no App borrow; the
/// app replaces `take` whenever the maps, the editing state or the bar change.
pub struct KeyTable {
    pub take: Box<dyn Fn(u32, u8) -> bool>,
}

impl Default for KeyTable {
    fn default() -> Self {
        KeyTable { take: Box::new(|_, _| false) }
    }
}

#[derive(Clone, Default)]
pub struct KeyGate(pub std::rc::Rc<std::cell::RefCell<KeyTable>>);

impl KeyGate {
    /// The answer for one key (false while the table is being replaced).
    pub fn takes(&self, vk: u32, mods: u8) -> bool {
        self.0.try_borrow().is_ok_and(|t| (t.take)(vk, mods))
    }
}

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

/// The one browser argument beyond the proxy's: the Windows integrated-authentication allowlist (NTLM,
/// Negotiate/Kerberos default credentials) holds a single host that cannot exist, so WebView2 never
/// sends the person's Windows credentials to a SITE by itself (Chromium otherwise allows them to
/// intranet-zone hosts when the list is empty). An NTLM or Negotiate challenge from a site then
/// reaches the coordinator like any other and is prompt-only (R13x710-1).
/// Not covered (probe run 2026-10-11b, `integrated_auth_is_never_sent_by_itself`): a PROXY's NTLM or
/// Negotiate 407 is still answered by the engine with the person's Windows credentials (Chromium's
/// allowlist passes every proxy), and `--auth-schemes` is ignored by this runtime, so no browser
/// argument removes the schemes. Recorded as a residual for Robert (R13x710-1).
pub const INTEGRATED_AUTH_ARG: &str = "--auth-server-allowlist=tandem-no-integrated-auth.invalid";

/// `AdditionalBrowserArguments` as the environment is made: `args` (the proxy's, already checked by
/// `adblock::proxy_args_ok`) and the integrated-authentication allowlist.
pub fn browser_arguments(args: &str) -> String {
    if args.is_empty() { INTEGRATED_AUTH_ARG.to_string() } else { format!("{args} {INTEGRATED_AUTH_ARG}") }
}

/// The user agent a web view gets (`WVW:127-150`): a set agent is used as written; an empty one
/// keeps the engine's with the application name appended.
pub fn user_agent(setting: &str, engine_default: &str) -> String {
    if setting.is_empty() { format!("{engine_default} Tandem") } else { setting.to_string() }
}

/// What a right-click landed on (WebView2's `ICoreWebView2ContextMenuTarget` and the location, in
/// physical pixels of the view). Everything a page chose is capped by `MenuTarget::capped` before it
/// is queued.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MenuTarget {
    /// `COREWEBVIEW2_CONTEXT_MENU_TARGET_KIND`: page 0, image 1, selected text 2, audio 3, video 4.
    pub kind: i32,
    pub editable: bool,
    pub main_frame: bool,
    pub page_uri: String,
    pub frame_uri: String,
    pub link: Option<String>,
    pub link_text: Option<String>,
    pub source: Option<String>,
    pub selection: Option<String>,
    pub x: i32,
    pub y: i32,
}

/// The longest selection a menu target carries, in characters (iTerm2's helpers stop at 1,000,000).
pub const MAX_MENU_SELECTION: usize = 1_000_000;
/// The longest link text a menu target carries, in characters.
pub const MAX_MENU_LINK_TEXT: usize = 1024;
/// Menus waiting for an answer in one view (a further request shows WebView2's own menu).
pub const MAX_PENDING_MENUS: usize = 4;

impl MenuTarget {
    /// The target with every page-chosen string bounded: URLs cut by `cap_url` (a cut URL is dropped,
    /// never used whole), the selection and the link text cut at a character boundary.
    pub fn capped(mut self) -> MenuTarget {
        let url = |u: &str| {
            let (c, cut) = cap_url(u);
            if cut { String::new() } else { c }
        };
        let text = |s: &str, max: usize| match s.char_indices().nth(max) {
            Some((at, _)) => s[..at].to_string(),
            None => s.to_string(),
        };
        self.page_uri = url(&self.page_uri);
        self.frame_uri = url(&self.frame_uri);
        self.link = self.link.map(|l| url(&l)).filter(|l| !l.is_empty());
        self.source = self.source.map(|l| url(&l)).filter(|l| !l.is_empty());
        self.link_text = self.link_text.map(|t| text(&t, MAX_MENU_LINK_TEXT));
        self.selection = self.selection.map(|t| text(&t, MAX_MENU_SELECTION));
        self
    }
}

/// One entry of the menu the app answers a `ContextMenu` event with (`WebView::answer_context_menu`).
#[derive(Clone, Debug, PartialEq)]
pub enum MenuEntry {
    /// A Tandem item; `icon_png` is an optional PNG (the color swatch).
    Custom { id: u32, label: String, enabled: bool, icon_png: Option<Vec<u8>> },
    Separator,
    /// Where WebView2's own items go.
    Native,
}

/// WebView2's own items Tandem replaces (their `Name`): iTerm2's menu has its own Save Page As and Print.
pub const REMOVED_NATIVE: &[&str] = &["saveAs", "print"];

/// What a view tells the app. Open for extension: every consumer's `match` ends in `_ =>`.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    Created(ViewId),
    /// The view could not be made or set up (never a page's text).
    Failed(ViewId, String),
    /// One of the view's processes failed: `kind` is `COREWEBVIEW2_PROCESS_FAILED_KIND`, and
    /// `failure_action` says what it means.
    ProcessFailed { view: ViewId, kind: i32 },
    /// A finished navigation: `url` is the URI of the matching NavigationStarting (iTerm2's
    /// `lastRequestedURL`), `nav` its `NavigationId`.
    /// `status`: the HTTP status on success, `COREWEBVIEW2_WEB_ERROR_STATUS` on failure. `http`: the
    /// HTTP status whenever a response arrived (a server's 4xx or 5xx fails with one), else 0.
    Navigated { view: ViewId, ok: bool, status: i32, http: i32, url: String, nav: u64 },
    Source(ViewId, String),
    Title(ViewId, String),
    Favicon(ViewId, Vec<u8>),
    /// A page's `postMessage`; `source` is the posting document's URL.
    Message { view: ViewId, json: String, source: String },
    ScriptDone { view: ViewId, token: u64, json: Result<String, String> },
    NewWindow { view: ViewId, url: String },
    /// A key the page raised; `taken` is the key gate's synchronous answer (Tandem took it and the
    /// page never saw it).
    Key { view: ViewId, vk: u32, mods: u8, taken: bool },
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
    /// A main-frame navigation to a `file:` URL was cancelled because the view shows local files as
    /// `iterm2-file:` pages (`ViewSetup.redirect_file_urls`, 13x8 Task 3) and the one verdict, then
    /// `file_redirect_allowed`, let it go. The app loads `iterm2_file_url(uri)` as Tandem's own load.
    FileRedirect { view: ViewId, uri: String },
    /// `WebView::clear_browsing_data` finished (13x8 Task 5); `ok` is whether the web view cleared it.
    /// Never dropped: the settings page waits on it.
    DataCleared { view: ViewId, ok: bool },
    /// A main-frame navigation was cancelled because the store's profile permissions are not
    /// synced yet (13x6, the one gate in the NavigationStarting handler): `url` is its TARGET, which
    /// the app loads once the sync lands. Never dropped.
    /// `by_tandem`, `user`, `redirect` and `source` are how the handler judged it (the app judges the
    /// release again with them, never as Tandem's own); `history` is a back or forward step.
    Parked { view: ViewId, url: String, user: bool, redirect: bool, by_tandem: bool, history: bool, source: Option<String> },
    /// A DevTools protocol event the view subscribed to (`devtools_subscribe`); `session` is the
    /// child session that raised it, None for the view's own target. Never trusted as Tandem's:
    /// `browser_world::Contexts` decides what a context is.
    DevTools { view: ViewId, session: Option<String>, name: String, json: String },
    /// The view's `browser_world::EventFilter` holds lifecycle events: the app takes them with
    /// `WebView::take_devtools_held` (one is queued at a time; final review I4).
    DevToolsHeld { view: ViewId },
    /// An iframe of the page appeared (the page world's view of it: WebView2's frame id, the
    /// `name` attribute, bounded). Posted only for a view that asked (`WebView::watch_frames`).
    FrameCreated { view: ViewId, frame: u64, name: String },
    FrameGone { view: ViewId, frame: u64 },
    /// A Page-world `postMessage` from an iframe; `source` is the posting document's URL.
    FrameMessage { view: ViewId, frame: u64, json: String, source: String },
    /// A main-frame navigation began (and the one verdict let it go): the single page-load-start
    /// event. `kind` is `COREWEBVIEW2_NAVIGATION_KIND`; a consumer that wants iTerm2's
    /// `didStartProvisionalNavigation` ignores `redirect == true`, which fires once per navigation.
    Starting { view: ViewId, url: String, kind: i32, user: bool, redirect: bool, nav: u64 },
    /// The HTTP method of a main-frame document request (browser panes that ask,
    /// `ViewSetup::track_document_method`): `nav` is the navigation the request belongs to, `method`
    /// is bounded by `bounded_method`. Posted after the navigation's `Starting`; no deferral.
    DocumentMethod { view: ViewId, nav: u64, method: String },
    /// A right-click on the page: the view holds the menu back (a deferral) until the app answers with
    /// `answer_context_menu(token, ..)`. Never dropped; its strings are capped by `MenuTarget::capped`.
    ContextMenu { view: ViewId, token: u64, target: MenuTarget },
    /// A custom item of the answered menu was chosen: `id` is the `MenuEntry::Custom` id.
    ContextMenuPicked { view: ViewId, token: u64, id: u32 },
    /// A page asked for a permission Tandem asks about (`browser_perms::route_kind`); the view holds
    /// the request (a deferral) until `answer_permission(token, ..)`. `kind` is
    /// `COREWEBVIEW2_PERMISSION_KIND`, `uri` the requesting frame's URI. Every other kind, and a
    /// request over the limits (`PermissionLimiter`), is denied by the host and never posted.
    Permission { view: ViewId, token: u64, kind: i32, uri: String },
    /// A page's notification (`NotificationReceived`, marked handled so WebView2 shows none); the
    /// view holds the notification object under `token` until `notification_report`. `origin` is
    /// the sender's origin; `title` and `body` are cut by `cap_note_title` and `cap_note_body`.
    NotificationShown { view: ViewId, token: u64, origin: String, title: String, body: String },
    /// The page closed the notification held under `token`.
    NotificationClosed { view: ViewId, token: u64 },
    /// `IsDocumentPlayingAudio` changed (it stays true while the view is muted).
    AudioPlaying { view: ViewId, playing: bool },
    /// The answer to `read_permission_states`: kind, origin as WebView2 reports it, state. `complete`
    /// is false when the read failed, an item could not be read or the list was cut at
    /// `MAX_PERMISSION_STATES`: the start-up sync must then fail closed (revoke every profile
    /// state the Tandem table does not hold), never treat the list as the whole profile.
    PermissionStates { view: ViewId, states: Vec<(i32, String, i32)>, complete: bool, workers_cleared: bool },
    /// The completion of `set_permission_state(seq, ..)`: `ok` is false when the runtime cannot
    /// set the profile or the write failed. Always posted once per call while the view lives (the
    /// app's write queue, `perm_queue`, waits for it).
    PermissionWritten { view: ViewId, seq: u64, ok: bool },
    /// The main document of navigation `nav` began to load (`ContentLoading`; browser panes with
    /// `ViewSetup::main_responses`). Never dropped.
    Committed { view: ViewId, nav: u64 },
    /// The final-or-redirect HTTP response of navigation `nav`'s main document (browser panes with
    /// `main_responses`): `status` is its code. Never dropped.
    Response { view: ViewId, nav: u64, status: i32 },
    /// The server (or the proxy) asked for HTTP authentication (`BasicAuthenticationRequested`;
    /// browser panes with `ViewSetup::basic_auth`). The view holds the request (a deferral) until
    /// `answer_basic_auth(token, ..)`; a request over the limits (`PermissionLimiter`, shared with
    /// permissions) or a view without `basic_auth` is cancelled by the host and never posted.
    /// `uri` is the URI WebView2 reports, `challenge` the header text; both are capped.
    BasicAuth { view: ViewId, token: u64, uri: String, challenge: String },
    /// A download is starting (`DownloadStarting`; browser panes with `ViewSetup::downloads`). The
    /// view holds it (a deferral, cancelled until answered) until `answer_download(token, id, ..)`;
    /// beyond the view's limits it is cancelled by the host and never posted. `url` is capped.
    /// Never dropped.
    DownloadStarting { view: ViewId, token: u64, url: String, total: Option<u64> },
    /// Bytes received so far by download `id` (the id `answer_download` was given). A later one
    /// corrects a dropped one.
    DownloadProgress { view: ViewId, id: u64, received: u64, total: Option<u64> },
    /// Download `id` ended: Ok when complete; Err holds fixed words, never the page's text. Never dropped.
    DownloadDone { view: ViewId, id: u64, result: Result<(), String> },
}

/// Longest challenge header text an event carries (characters); the rest is cut, and a cut
/// challenge is cancelled by the host.
pub const MAX_CHALLENGE: usize = 1024;

/// How the app answers a `Event::Permission`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PermAnswer {
    /// Refuse (`COREWEBVIEW2_PERMISSION_STATE_DENY`, nothing saved in the profile). There is no
    /// "leave it to WebView2": that would be a grant nobody asked Tandem about (pre-flight B3).
    Deny,
    /// `state` is a `browser_perms::STATE_*`. WebView2 never stores a decision (`SavesInProfile` is
    /// always false). `saves` tells the APP to mirror an Allow of a notification or location
    /// request into the profile through its write queue (`perm_queue`) and to answer only once
    /// that write came back, so the page works at once; the host itself never writes the profile
    /// on an answer. The Tandem table stays the only source; the start-up sync revokes whatever
    /// it does not hold as Granted. A Deny is never mirrored.
    State { state: i32, saves: bool },
}

/// A notification title is cut to this many characters before it is queued.
pub const NOTE_TITLE_CHARS: usize = 256;
/// A notification body likewise.
pub const NOTE_BODY_CHARS: usize = 1024;
/// Permission settings one `PermissionStates` carries.
pub const MAX_PERMISSION_STATES: usize = 2048;

fn cap_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// A page's notification title, cut (the app cleans it for display).
pub fn cap_note_title(s: &str) -> String {
    cap_chars(s, NOTE_TITLE_CHARS)
}

/// A page's notification body, cut.
pub fn cap_note_body(s: &str) -> String {
    cap_chars(s, NOTE_BODY_CHARS)
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
            | Event::Refused { view, .. }
            | Event::FileRedirect { view, .. }
            | Event::DataCleared { view, .. }
            | Event::Parked { view, .. }
            | Event::ProcessFailed { view, .. }
            | Event::DevTools { view, .. }
            | Event::DevToolsHeld { view }
            | Event::FrameCreated { view, .. }
            | Event::FrameGone { view, .. }
            | Event::FrameMessage { view, .. }
            | Event::Starting { view, .. }
            | Event::DocumentMethod { view, .. }
            | Event::ContextMenu { view, .. }
            | Event::ContextMenuPicked { view, .. }
            | Event::Permission { view, .. }
            | Event::NotificationShown { view, .. }
            | Event::NotificationClosed { view, .. }
            | Event::AudioPlaying { view, .. }
            | Event::PermissionStates { view, .. }
            | Event::PermissionWritten { view, .. }
            | Event::Committed { view, .. }
            | Event::Response { view, .. }
            | Event::BasicAuth { view, .. }
            | Event::DownloadStarting { view, .. }
            | Event::DownloadProgress { view, .. }
            | Event::DownloadDone { view, .. } => *view,
        }
    }

    /// A console line (`iTerm2ConsoleLog`) from the page world (`Message`, `FrameMessage`) or the
    /// isolated world (a binding call, whose payload is a JSON string inside the event). Only a
    /// shape check: a message that merely looks like one is held to the stricter console budget.
    pub fn is_console(&self) -> bool {
        let handler = crate::browser_js::CONSOLE_HANDLER;
        match self {
            Event::Message { json, .. } | Event::FrameMessage { json, .. } => json.get(..json.len().min(48)).is_some_and(|head| head.contains(&format!("\"h\":\"{handler}\""))),
            Event::DevTools { name, json, .. } if name == BINDING_EVENT => json.contains(&format!("{{\\\"h\\\":\\\"{handler}\\\"")),
            _ => false,
        }
    }

    /// An event the queue may drop when a page floods it. Lifecycle, navigation results, focus,
    /// script results and requests (each holds a COM deferral) are never dropped.
    pub fn droppable(&self) -> bool {
        // Only a binding call (a page can make them at will) is droppable. A context or target
        // lifecycle event never is: a lost Destroyed would leave a stale context in the table, and an
        // attached child waits until it is resumed (B12, fix round 1). The host drops the ones the
        // app ignores before they are queued and holds the rest behind one `DevToolsHeld` wake
        // (`browser_world::EventFilter`, final review I4), which is never dropped either.
        if let Event::DevTools { name, .. } = self {
            return name == BINDING_EVENT;
        }
        // A new audio state is corrected by the next change (and a new document resets it); the
        // permission and notification events hold a COM object or answer a request, so none is
        // dropped: the host bounds them where they are made (`PermissionLimiter`, `NotificationLimiter`).
        if matches!(self, Event::AudioPlaying { .. } | Event::DownloadProgress { .. }) {
            return true;
        }
        // A key Tandem took is already hidden from the page (`SetHandled(true)`): dropping it would
        // lose the press to both (final review minor 6). Only a person's key makes one.
        matches!(self, Event::Source(..) | Event::Title(..) | Event::Favicon(..) | Event::Message { .. } | Event::FrameMessage { .. } | Event::FrameCreated { .. } | Event::FrameGone { .. } | Event::NewWindow { .. } | Event::ExternalUri { .. } | Event::Refused { .. } | Event::Key { taken: false, .. } | Event::History { .. } | Event::Loading(..))
    }

    /// The bytes an event holds that a page chose.
    pub fn weight(&self) -> usize {
        match self {
            Event::Failed(_, s) | Event::Source(_, s) | Event::Title(_, s) => s.len(),
            Event::Favicon(_, b) => b.len(),
            Event::Message { json, source, .. } | Event::FrameMessage { json, source, .. } => json.len() + source.len(),
            Event::FrameCreated { name, .. } => name.len(),
            Event::Starting { url, .. } => url.len(),
            Event::DocumentMethod { method, .. } => method.len(),
            Event::ContextMenu { target: t, .. } => t.page_uri.len() + t.frame_uri.len() + t.link.as_ref().map_or(0, String::len) + t.link_text.as_ref().map_or(0, String::len) + t.source.as_ref().map_or(0, String::len) + t.selection.as_ref().map_or(0, String::len),
            Event::DevTools { json, name, session, .. } => json.len() + name.len() + session.as_ref().map_or(0, |s| s.len()),
            Event::Navigated { url, .. } | Event::NewWindow { url, .. } | Event::ExternalUri { url, .. } | Event::Refused { url, .. } | Event::Parked { url, .. } => url.len(),
            Event::FileRedirect { uri, .. } => uri.len(),
            Event::Request { url, top_url, .. } => url.len() + top_url.len(),
            Event::ScriptDone { json, .. } => json.as_ref().map_or_else(|e| e.len(), |s| s.len()),
            Event::Permission { uri, .. } => uri.len(),
            Event::BasicAuth { uri, challenge, .. } => uri.len() + challenge.len(),
            Event::DownloadStarting { url, .. } => url.len(),
            Event::DownloadDone { result: Err(why), .. } => why.len(),
            Event::NotificationShown { origin, title, body, .. } => origin.len() + title.len() + body.len(),
            Event::PermissionStates { states, .. } => states.iter().map(|(_, o, _)| o.len()).sum(),
            _ => 0,
        }
    }
}

/// A request method as an event carries it: at most 16 ASCII token characters, upper-cased. Anything
/// else is "UNKNOWN", which no add accepts (a known method other than GET refuses, `NMM:142-145`).
pub fn bounded_method(m: &str) -> String {
    let token = |c: char| c.is_ascii_alphanumeric() || c == '-';
    if m.is_empty() || m.len() > 16 || !m.chars().all(token) {
        return "UNKNOWN".to_string();
    }
    m.to_ascii_uppercase()
}

/// Whether a WebView2 resource filter pattern (`*` any run, `?` one character, case ignored) matches
/// `uri`. A view with a Document-context filter of its own asks this of its other filters, so the
/// requests only the new filter matched do not take the deferral path.
pub fn filter_matches(pattern: &str, uri: &str) -> bool {
    fn go(p: &[char], u: &[char]) -> bool {
        match p.split_first() {
            None => u.is_empty(),
            Some(('*', rest)) => (0..=u.len()).any(|i| go(rest, &u[i..])),
            Some((c, rest)) => u.split_first().is_some_and(|(x, tail)| (*c == '?' || c.eq_ignore_ascii_case(x)) && go(rest, tail)),
        }
    }
    let (p, u): (Vec<char>, Vec<char>) = (pattern.chars().collect(), uri.chars().collect());
    go(&p, &u)
}

/// The one DevTools event a page can raise at will.
pub const BINDING_EVENT: &str = "Runtime.bindingCalled";

/// The largest DevTools event body the host queues (a binding payload and a URL).
pub const fn devtools_json_cap() -> usize {
    MAX_MESSAGE + MAX_URL
}

/// `COREWEBVIEW2_PROCESS_FAILED_KIND` values (the WebView2 headers).
pub const PROCESS_BROWSER_EXITED: i32 = 0;
pub const PROCESS_RENDER_EXITED: i32 = 1;
pub const PROCESS_RENDER_UNRESPONSIVE: i32 = 2;
pub const PROCESS_FRAME_RENDER_EXITED: i32 = 3;
pub const PROCESS_UTILITY_EXITED: i32 = 4;
pub const PROCESS_GPU_EXITED: i32 = 6;

/// What a process failure means for the view that reports it (final review I5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailAction {
    /// The browser process exited: every view of its environment is dead, and so is the
    /// environment; it is made again, then the view.
    Environment,
    /// The page's renderer exited: the view is made again.
    Recreate,
    /// The page hangs, a frame's renderer exited (any third-party frame can cause it) or the GPU
    /// process exited (a driver reset): the view reloads; nothing else is touched.
    Reload,
    /// A utility, sandbox helper, plugin or unknown process: the runtime recovers it; logged only.
    Log,
}

/// The action for a `COREWEBVIEW2_PROCESS_FAILED_KIND`.
pub fn failure_action(kind: i32) -> FailAction {
    match kind {
        PROCESS_BROWSER_EXITED => FailAction::Environment,
        PROCESS_RENDER_EXITED => FailAction::Recreate,
        PROCESS_RENDER_UNRESPONSIVE | PROCESS_FRAME_RENDER_EXITED | PROCESS_GPU_EXITED => FailAction::Reload,
        _ => FailAction::Log,
    }
}

/// Remakes (or reloads) in a row with no good start between them before a view is given up.
pub const RESTARTS_IN_A_ROW: u32 = 2;
/// Remakes (or reloads) in any minute before a view is given up (a page that crashes on load).
pub const RESTARTS_PER_MINUTE: usize = 5;

/// A view's restart budget: `RESTARTS_IN_A_ROW` without a good start, and `RESTARTS_PER_MINUTE`.
/// A good start (`succeeded`: the view was made, or a page loaded) resets the count in a row, so
/// unrelated failures far apart are each recovered.
#[derive(Clone, Debug, Default)]
pub struct Restarts {
    in_a_row: u32,
    recent: VecDeque<Instant>,
}

impl Restarts {
    /// A restart at `now`: true when the budget allows it (and it is counted).
    pub fn allow(&mut self, now: Instant) -> bool {
        if self.in_a_row >= RESTARTS_IN_A_ROW || !self.rate_ok(now) {
            return false;
        }
        self.in_a_row += 1;
        self.recent.push_back(now);
        true
    }

    /// The per-minute bound alone (a view whose good start the app does not see).
    pub fn allow_rate(&mut self, now: Instant) -> bool {
        if !self.rate_ok(now) {
            return false;
        }
        self.recent.push_back(now);
        true
    }

    fn rate_ok(&mut self, now: Instant) -> bool {
        while self.recent.front().is_some_and(|t| now.saturating_duration_since(*t) >= Duration::from_secs(60)) {
            self.recent.pop_front();
        }
        self.recent.len() < RESTARTS_PER_MINUTE
    }

    /// The view started (or a page loaded) after a restart.
    pub fn succeeded(&mut self) {
        self.in_a_row = 0;
    }
}

/// What a browser pane does after its view reported a process failure of `kind`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaneStep {
    Nothing,
    Reload,
    /// The view is made again from the pane's URL (after its environment, for a browser process exit).
    Remake,
    /// Over the budget: the pane shows no view and says so, until the person presses Reload or
    /// navigates.
    GiveUp,
}

/// The step for a failure of `kind` under `budget` (counted when it acts).
pub fn pane_step(kind: i32, budget: &mut Restarts, now: Instant) -> PaneStep {
    match failure_action(kind) {
        FailAction::Log => PaneStep::Nothing,
        FailAction::Reload if budget.allow(now) => PaneStep::Reload,
        FailAction::Reload => PaneStep::Nothing,
        FailAction::Recreate | FailAction::Environment if budget.allow(now) => PaneStep::Remake,
        FailAction::Recreate | FailAction::Environment => PaneStep::GiveUp,
    }
}

/// Whether dropping `view` (a child of `parent`) hands the keyboard back to `parent` (final review
/// I7): the window whose handle is `held` holds the keyboard in a page (the 13u hook passes its Win
/// chords) and no other view of it is known to have the keyboard (`focus_view`, the last view that
/// reported gaining it).
pub fn drop_returns_focus(held: Option<isize>, parent: isize, focus_view: Option<ViewId>, view: ViewId) -> bool {
    parent != 0 && held == Some(parent) && focus_view.is_none_or(|v| v == view)
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

/// What the permission sync gate does with a main-frame navigation the one verdict ALLOWED.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GateAct {
    /// It proceeds.
    Open,
    /// Cancelled and forgotten: the page stays (a scheme that may not wait, or a URL cut short).
    Cancel,
    /// Cancelled and its target posted for the app to load after the sync, judged again then.
    Park,
}

/// The permission sync gate of the one main-frame NavigationStarting handler (13x6), applied AFTER
/// `nav_verdict` (a refused navigation is never parked): while the view's store is not synced
/// (`gate` Some(false)) every allowed main-frame navigation is cancelled. Only a web page (http or
/// https) of any initiator, or Tandem's own load, is parked; any other scheme (file:, iterm2-file:,
/// iterm2-ssh:, data:, ...) is cancelled and forgotten, because a parked target is later loaded by
/// Tandem and must never launder a page's choice. Tandem's own about:blank and Tandem's own
/// `iterm2-about:` pages (they use no web permission) pass; a URL over the cap is never parked
/// truncated. No gate (popovers, tools) is always open.
pub fn nav_gate(gate: Option<bool>, uri: &str, by_tandem: bool, over_long: bool) -> GateAct {
    if gate != Some(false) {
        return GateAct::Open;
    }
    let scheme = scheme_of(uri);
    if scheme == "iterm2-about" || (by_tandem && uri.eq_ignore_ascii_case("about:blank")) {
        return GateAct::Open;
    }
    if over_long || !(by_tandem || matches!(scheme.as_str(), "http" | "https")) {
        return GateAct::Cancel;
    }
    GateAct::Park
}

/// Whether a navigation of a SUBFRAME is cancelled: any `iterm2-about:` target. The local pages
/// are never framed (their policy refuses framing), so only the main frame may request one; this
/// leaves a frame no way to race the main frame for the View Source page.
pub fn frame_nav_cancelled(uri: &str) -> bool {
    matches!(scheme_of(uri).as_str(), "iterm2-about" | "iterm2-file" | "iterm2-ssh")
}

/// How a custom scheme is registered (`ICoreWebView2CustomSchemeRegistration`; 13x8 review I4):
/// the allowed origins and whether its URLs carry a host. None of them is secure-origin. `iterm2-file`
/// pages link to folders and ".." (their own scheme) and may be opened from Tandem's pages;
/// `iterm2-ssh` URLs carry a host.
pub fn scheme_registration(scheme: &str) -> (Vec<String>, bool) {
    let own = |other: &str| vec![format!("{other}:"), format!("{scheme}:")];
    match scheme {
        "iterm2-file" => (own("iterm2-about"), false),
        "iterm2-ssh" => (own("iterm2-about"), true),
        _ => (Vec::new(), false),
    }
}

/// Whether `url` is an `iterm2-about:` page whose buttons change settings: the settings page and the
/// two onboarding steps that write.
pub fn power_page(url: &str) -> bool {
    let (scheme, rest) = match url.split_once(':') {
        Some(p) => p,
        None => return false,
    };
    scheme.eq_ignore_ascii_case("iterm2-about") && ["settings", "downloads", "onboarding-setup", "onboarding-profile"].iter().any(|n| rest.eq_ignore_ascii_case(n) || rest.get(..n.len()).is_some_and(|h| h.eq_ignore_ascii_case(n)) && matches!(rest.as_bytes().get(n.len()), Some(b'#' | b'?' | b'/')))
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
    let s = source.map(scheme_of).unwrap_or_default();
    // A page that acts with power (settings, onboarding steps) is opened by Tandem, by the person
    // (no source: the address bar) or from a Tandem page, never from a web page, whether by a click,
    // a script or a redirect (preflight B5, 13x8 Task 5).
    if !by_tandem && power_page(target) {
        return s == "iterm2-about" || (s.is_empty() && user && !redirect);
    }
    // A page of the web cannot start a file or ssh page, not even by a click the person is tricked
    // into (preflight B5): only Tandem, or the local pages themselves (a listing's links).
    if user && !by_tandem && matches!(t.as_str(), "iterm2-file" | "iterm2-ssh") {
        return matches!(s.as_str(), "iterm2-about" | "iterm2-file" | "iterm2-ssh");
    }
    if by_tandem || user {
        return true;
    }
    if s == "iterm2-file" || s == "iterm2-ssh" {
        return false;
    }
    if local_scheme(&t) {
        return s == "iterm2-about";
    }
    true
}

/// The step a view's setup fails at when the runtime has no `ServerCertificateErrorDetected`, and what
/// the person is told (13x8 Task 2 review M4).
pub const OLD_RUNTIME_WHAT: &str = "certificate errors";
pub const OLD_RUNTIME_TEXT: &str = "This WebView2 runtime is too old; update Microsoft Edge WebView2.";

/// Whether a `file:` navigation the one verdict allowed may become an `iterm2-file:` page (13x8 Task 3,
/// preflight B5): Tandem's own load, or a source that is not a web page (an `iterm2-about:` or
/// `iterm2-file:` page, or no source at all). A person's click on a link of a WEB page is refused: a
/// clickjacked click must not make Tandem list or read a local path.
pub fn file_redirect_allowed(source: Option<&str>, by_tandem: bool) -> bool {
    if by_tandem {
        return true;
    }
    match source.map(scheme_of).as_deref() {
        None | Some("") => true,
        Some(s) => matches!(s, "iterm2-about" | "iterm2-file" | "iterm2-ssh"),
    }
}

/// `file:///C:/a%20b/` as the `iterm2-file:` URL of the same path (13x8 Task 3); None for a host
/// other than empty or `localhost`, a UNC path, or a path that does not start at a drive letter.
/// Nothing is probed. The escapes stay as they are, so the one decoder (`browser_pages::file_path_of`)
/// reads both forms alike.
pub fn iterm2_file_url(file_url: &str) -> Option<String> {
    if scheme_of(file_url) != "file" {
        return None;
    }
    let rest = file_url.get("file:".len()..)?.strip_prefix("//")?;
    let (host, path) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
    if !(host.is_empty() || host.eq_ignore_ascii_case("localhost")) {
        return None;
    }
    let b = path.as_bytes();
    let drive = b.len() >= 3 && b[0] == b'/' && b[1].is_ascii_alphabetic() && b[2] == b':' && (b.len() == 3 || matches!(b[3], b'/' | b'?' | b'#'));
    drive.then(|| format!("iterm2-file://{path}"))
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
/// Console lines (either world) of one view that may sit in the queue: their own budget, so heavy
/// logging never takes room from the isolated world's real messages (Task 2 review I2).
pub const MAX_QUEUED_CONSOLE: usize = 20;
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

/// Permission requests a view may raise per second, and may have waiting for an answer. The rest
/// are denied at once and never queued (pre-flight B2).
pub const PERMISSIONS_PER_SECOND: usize = 4;
pub const MAX_PENDING_PERMISSIONS: usize = 8;

/// `PermissionRequested` limiter, the `NewWindowLimiter` pattern.
#[derive(Debug, Default)]
pub struct PermissionLimiter {
    window_start: Option<Instant>,
    in_window: usize,
}

impl PermissionLimiter {
    pub fn allow(&mut self, now: Instant, pending: usize) -> bool {
        if pending >= MAX_PENDING_PERMISSIONS {
            return false;
        }
        match self.window_start {
            Some(t) if now.duration_since(t) < Duration::from_secs(1) => {}
            _ => {
                self.window_start = Some(now);
                self.in_window = 0;
            }
        }
        if self.in_window >= PERMISSIONS_PER_SECOND {
            return false;
        }
        self.in_window += 1;
        true
    }
}

/// Notifications a view may raise per minute, and the objects it may hold (each is a COM object).
pub const NOTES_PER_MINUTE: usize = 30;
pub const MAX_HELD_NOTES: usize = 16;

/// `NotificationReceived` limiter: what it refuses is reported closed to the page and released.
#[derive(Debug, Default)]
pub struct NotificationLimiter {
    window_start: Option<Instant>,
    in_window: usize,
}

impl NotificationLimiter {
    pub fn allow(&mut self, now: Instant, held: usize) -> bool {
        if held >= MAX_HELD_NOTES {
            return false;
        }
        match self.window_start {
            Some(t) if now.duration_since(t) < Duration::from_secs(60) => {}
            _ => {
                self.window_start = Some(now);
                self.in_window = 0;
            }
        }
        if self.in_window >= NOTES_PER_MINUTE {
            return false;
        }
        self.in_window += 1;
        true
    }
}

/// `COREWEBVIEW2_WEB_RESOURCE_REQUEST_SOURCE_KINDS_SERVICE_WORKER`.
pub const SOURCE_SERVICE_WORKER: i32 = 4;

/// Ruling R13x6-SW (final review M5): WebView2 raises `NotificationReceived` on the view for
/// non-persistent notifications only; a service worker's `showNotification` (a persistent one) goes
/// to a profile event this SDK lacks and, unhandled, to WebView2's own toast, past Tandem's label,
/// limits and table. Service workers are therefore refused: every request a service worker makes, and
/// the script fetch that would install one (sent with a `Service-Worker` header), gets a 403.
pub fn refuse_worker_request(source_kinds: i32, service_worker_header: bool) -> bool {
    source_kinds & SOURCE_SERVICE_WORKER != 0 || service_worker_header
}

/// Notifications are asked about only when the view raises them to Tandem AND no service worker can
/// raise one past it (fail closed): new workers are refused (`refuse_worker_request`) and the
/// registrations an older build let sites make were cleared from the profile (last-gate LG-1; such
/// a worker starts from its cache with no fetch, so the refusal alone never reaches it).
pub fn notifications_offered(events: bool, workers_refused: bool, old_workers_cleared: bool) -> bool {
    events && workers_refused && old_workers_cleared
}

/// What one view has queued: events, bytes and messages. Shared by the view's handlers and every
/// `Charge` they make.
#[derive(Debug, Default)]
pub struct ViewGate {
    events: AtomicUsize,
    bytes: AtomicUsize,
    messages: AtomicUsize,
    console: AtomicUsize,
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
    console: bool,
    new_window: bool,
}

impl Drop for Charge {
    fn drop(&mut self) {
        self.gate.events.fetch_sub(1, Ordering::AcqRel);
        self.gate.bytes.fetch_sub(self.bytes, Ordering::AcqRel);
        if self.message {
            self.gate.messages.fetch_sub(1, Ordering::AcqRel);
        }
        if self.console {
            self.gate.console.fetch_sub(1, Ordering::AcqRel);
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
        let console = event.is_console();
        let message = !console && (matches!(event, Event::Message { .. } | Event::FrameMessage { .. }) || matches!(event, Event::DevTools { name, .. } if name == BINDING_EVENT));
        let new_window = matches!(event, Event::NewWindow { .. });
        if event.droppable() {
            let over = self.events.load(Ordering::Acquire) >= MAX_QUEUED_EVENTS
                || self.bytes.load(Ordering::Acquire) + weight > MAX_QUEUED_BYTES
                || (message && self.messages.load(Ordering::Acquire) >= MAX_QUEUED_MESSAGES)
                || (console && self.console.load(Ordering::Acquire) >= MAX_QUEUED_CONSOLE)
                || (console && weight > MAX_MESSAGE + MAX_URL)
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
        if console {
            self.console.fetch_add(1, Ordering::AcqRel);
        }
        if new_window {
            self.new_windows.fetch_add(1, Ordering::AcqRel);
        }
        Ok(Charge { gate: self.clone(), bytes: weight, message, console, new_window })
    }

    /// New-window requests queued now.
    pub fn new_windows_pending(&self) -> usize {
        self.new_windows.load(Ordering::Acquire)
    }

    /// Events queued now.
    pub fn queued(&self) -> usize {
        self.events.load(Ordering::Acquire)
    }

    /// Counts one event the handler refused before queueing it.
    pub fn note_dropped(&self) {
        self.dropped.fetch_add(1, Ordering::AcqRel);
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
/// The `NavigationKind` value of a history step (Back, Forward, `history.go`).
pub const NAV_KIND_HISTORY: i32 = 1;

/// A history step never lands on a power page, whoever started it and whatever the source (13x8 Task 5
/// review I1, redesigned): no flag a page could take. Tandem's own Back, Forward and history pick look
/// at the pane's record first and, for such a target, load the page fresh (`needs_fresh_load`).
pub fn history_refused(kind: i32, target: &str) -> bool {
    kind == NAV_KIND_HISTORY && power_page(target)
}

/// Whether Tandem loads a history target as a fresh Tandem navigation instead of stepping to it: a
/// power page or a local (`file:`, `iterm2-file:`, `iterm2-ssh:`) page, which only Tandem's own load
/// may open.
pub fn needs_fresh_load(target: &str) -> bool {
    power_page(target) || local_scheme(&scheme_of(target))
}

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
    /// Tandem's own load (`navigate_tandem`).
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

    /// Final review I7: a view that goes away while its window's page holds the keyboard hands the
    /// keyboard back to that window, unless another view of it is known to hold it.
    #[test]
    fn a_dropped_view_returns_the_keyboard() {
        const W: isize = 0x10;
        assert!(drop_returns_focus(Some(W), W, Some(7), 7), "the view that had it");
        assert!(drop_returns_focus(Some(W), W, None, 7), "no view is known to have it");
        assert!(!drop_returns_focus(Some(W), W, Some(8), 7), "another view of the window has it");
        assert!(!drop_returns_focus(Some(0x20), W, Some(7), 7), "the hold is another window's");
        assert!(!drop_returns_focus(None, W, Some(7), 7), "no hold");
        assert!(!drop_returns_focus(Some(W), 0, None, 7), "a view with no parent");
    }

    /// Final review I5: a process failure acts on its kind. The browser process takes the
    /// environment with it; a page's renderer is remade or reloaded; a frame's renderer and the GPU
    /// process reload that view only; the rest (utility, sandbox helper, plugins, unknown) recover on
    /// their own and are only logged.
    #[test]
    fn process_failures_act_on_the_kind() {
        assert_eq!(failure_action(PROCESS_BROWSER_EXITED), FailAction::Environment);
        assert_eq!(failure_action(PROCESS_RENDER_EXITED), FailAction::Recreate);
        for k in [PROCESS_RENDER_UNRESPONSIVE, PROCESS_FRAME_RENDER_EXITED, PROCESS_GPU_EXITED] {
            assert_eq!(failure_action(k), FailAction::Reload, "{k}");
        }
        for k in [4, 5, 7, 8, 9, -1, 42] {
            assert_eq!(failure_action(k), FailAction::Log, "{k}");
        }
        let e = Event::ProcessFailed { view: 7, kind: 3 };
        assert_eq!(e.view(), 7);
        assert!(!e.droppable(), "a failure is never dropped by a flood");
    }

    /// Final review I5: failures injected in order. The counter resets after a good start, so two
    /// unrelated failures never leave a pane blank; failures in a row, or a crash loop, end in a
    /// give-up the person can undo with Reload.
    #[test]
    fn injected_failures_never_blank_a_pane_for_good() {
        let t0 = Instant::now();
        let s = |n: u64| t0 + Duration::from_secs(n);
        // Two unrelated renderer exits, a good start between them: both remade.
        let mut b = Restarts::default();
        assert_eq!(pane_step(PROCESS_RENDER_EXITED, &mut b, s(0)), PaneStep::Remake);
        b.succeeded();
        assert_eq!(pane_step(PROCESS_RENDER_EXITED, &mut b, s(600)), PaneStep::Remake);
        // A GPU reset and a frame's crash: the view is reloaded, never remade or given up.
        let mut b = Restarts::default();
        assert_eq!(pane_step(PROCESS_GPU_EXITED, &mut b, s(0)), PaneStep::Reload);
        b.succeeded();
        assert_eq!(pane_step(PROCESS_FRAME_RENDER_EXITED, &mut b, s(5)), PaneStep::Reload);
        b.succeeded();
        assert_eq!(pane_step(PROCESS_RENDER_EXITED, &mut b, s(10)), PaneStep::Remake);
        // Logged kinds touch nothing and use no budget.
        let mut b = Restarts::default();
        for _ in 0..10 {
            assert_eq!(pane_step(PROCESS_UTILITY_EXITED, &mut b, s(0)), PaneStep::Nothing);
        }
        assert_eq!(pane_step(PROCESS_BROWSER_EXITED, &mut b, s(0)), PaneStep::Remake);
        // In a row with no good start: the third is given up; a reload that is not allowed is nothing.
        let mut b = Restarts::default();
        assert_eq!(pane_step(PROCESS_RENDER_EXITED, &mut b, s(0)), PaneStep::Remake);
        assert_eq!(pane_step(PROCESS_RENDER_EXITED, &mut b, s(1)), PaneStep::Remake);
        assert_eq!(pane_step(PROCESS_RENDER_EXITED, &mut b, s(2)), PaneStep::GiveUp);
        assert_eq!(pane_step(PROCESS_GPU_EXITED, &mut b, s(3)), PaneStep::Nothing);
        // A crash loop (each remake starts, then dies): five a minute, then given up; a minute later
        // it may try again.
        let mut b = Restarts::default();
        for n in 0..RESTARTS_PER_MINUTE as u64 {
            assert_eq!(pane_step(PROCESS_RENDER_EXITED, &mut b, s(n)), PaneStep::Remake, "{n}");
            b.succeeded();
        }
        assert_eq!(pane_step(PROCESS_RENDER_EXITED, &mut b, s(10)), PaneStep::GiveUp);
        b.succeeded();
        assert_eq!(pane_step(PROCESS_RENDER_EXITED, &mut b, s(61)), PaneStep::Remake);
        // The rate alone (tool views): no counter in a row, only five a minute.
        let mut b = Restarts::default();
        for n in 0..5 {
            assert!(b.allow_rate(s(n)));
        }
        assert!(!b.allow_rate(s(6)));
        assert!(b.allow_rate(s(70)));
    }

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
    fn history_never_lands_on_a_power_page() {
        for target in ["iterm2-about:settings", "ITERM2-ABOUT:Settings#x", "iterm2-about:onboarding-setup", "iterm2-about:downloads"] {
            assert!(history_refused(NAV_KIND_HISTORY, target), "{target}");
            assert!(!history_refused(0, target), "a typed or Tandem load is the verdict's, not this rule's: {target}");
            assert!(needs_fresh_load(target));
        }
        assert!(!history_refused(NAV_KIND_HISTORY, "https://a.example/") && !needs_fresh_load("https://a.example/"));
        assert!(needs_fresh_load("iterm2-file:///C:/x") && needs_fresh_load("iterm2-ssh://h/x") && needs_fresh_load("file:///C:/x"));
        // A web page's script navigation to a power page stays refused by the verdict as before.
        assert!(!navigation_allowed(Some("https://evil.test/"), "iterm2-about:settings", false, false, false, true));
        assert!(navigation_allowed(Some("https://evil.test/"), "iterm2-about:settings", false, false, true, true), "Tandem's own load");
    }

    #[test]
    fn navigation_guard_follows_the_rules() {
        let n = |s: Option<&str>, t: &str, user, redirect, tandem| navigation_allowed(s, t, user, redirect, tandem, true);
        assert!(!n(Some("https://evil.test/"), "iterm2-file:///C:/Users/u/.ssh/id_ed25519", false, false, false));
        // A click the person makes on a hostile page still starts nothing (B5); a local page's link does.
        assert!(!n(Some("https://evil.test/"), "iterm2-file:///C:/Users/u/.ssh/id_ed25519", true, false, false));
        assert!(!n(Some("https://evil.test/"), "iterm2-ssh://box/etc/shadow", true, false, false));
        assert!(!n(Some("about:blank"), "iterm2-file:///C:/", true, false, false));
        assert!(n(Some("https://evil.test/"), "iterm2-file:///C:/Users/u/.ssh/id_ed25519", true, false, true), "Tandem's own load");
        assert!(n(Some("iterm2-about:welcome"), "iterm2-file:///C:/", true, false, false));
        assert!(!n(Some("https://evil.test/"), "iterm2-ssh://box/dev/zero", false, false, false));
        assert!(!n(Some("https://evil.test/"), "file:///C:/Windows/", false, false, false));
        assert!(n(Some("iterm2-about:welcome"), "iterm2-file:///C:/", false, false, false));
        assert!(!n(Some("iterm2-file:///C:/T/a.html"), "https://evil.test/?d=x", false, false, false));
        assert!(!n(Some("iterm2-file:///C:/T/a.html"), "iterm2-file:///C:/T/b.txt", false, false, false));
        assert!(n(Some("iterm2-file:///C:/T/"), "iterm2-file:///C:/T/b.txt", true, false, false));
        assert!(!n(Some("iterm2-ssh://box/a.html"), "https://evil.test/", false, false, false));
        assert!(!n(Some("https://a/"), "iterm2-file:///C:/x", true, true, false));
        assert!(!n(Some("https://a/"), "file:///C:/x", false, true, true));
        // The settings and onboarding pages: not from a web page by any road; Tandem, the address bar and
        // Tandem's own pages may.
        for t in ["iterm2-about:settings", "ITERM2-ABOUT:Settings#x", "iterm2-about:onboarding-setup", "iterm2-about:onboarding-profile?x"] {
            for (user, redirect) in [(true, false), (false, false), (false, true), (true, true)] {
                assert!(!n(Some("https://evil.test/"), t, user, redirect, false), "{t} {user} {redirect}");
                assert!(!n(Some("about:blank"), t, user, redirect, false));
                assert!(!n(Some("iterm2-file:///C:/T/a.html"), t, user, redirect, false));
            }
            assert!(n(Some("https://evil.test/"), t, true, false, true), "Tandem's own load");
            assert!(n(Some("iterm2-about:welcome"), t, true, false, false) && n(Some("iterm2-about:permissions"), t, false, false, false));
            assert!(n(None, t, true, false, false), "typed by the person");
            assert!(!n(None, t, false, false, false));
        }
        assert!(n(Some("https://evil.test/"), "iterm2-about:permissions", true, false, false), "other pages keep ruling 4");
        assert!(!power_page("iterm2-about:settingsx") && !power_page("iterm2-about:onboarding-intro") && !power_page("https://a/settings"));
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
        post(Event::Navigated { view: 1, ok: true, status: 200, http: 200, url: "https://a/".into(), nav: 1 });
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
        // Final review minor 6: a key Tandem took (`SetHandled(true)`: the page never sees it) is
        // never dropped, or the press would be lost to both; a key the page kept may be.
        assert!(!Event::Key { view: 1, vk: 0x4a, mods: 3, taken: true }.droppable());
        assert!(Event::Key { view: 1, vk: 0x4a, mods: 0, taken: false }.droppable());
        let gate = ViewGate::new();
        let mut held = Vec::new();
        for _ in 0..(MAX_QUEUED_EVENTS + 10) {
            held.push(gate.admit(&Event::Key { view: 1, vk: 0x4a, mods: 3, taken: true }).expect("a taken key is never refused"));
        }
        drop(held);
    }

    /// Fix round 1, item 1 (B12): only a binding call is droppable (and counts as a message); every
    /// context and target lifecycle event is admitted over any cap.
    #[test]
    fn devtools_events_follow_the_caps() {
        let ev = |name: &str, json: &str| Event::DevTools { view: 1, session: None, name: name.into(), json: json.into() };
        for lifecycle in ["Runtime.executionContextCreated", "Runtime.executionContextDestroyed", "Runtime.executionContextsCleared", "Page.frameNavigated", "Target.attachedToTarget", "Target.detachedFromTarget"] {
            assert!(!ev(lifecycle, "{}").droppable(), "{lifecycle}");
        }
        assert!(ev("Runtime.bindingCalled", "{}").droppable());
        let gate = ViewGate::new();
        let mut held = Vec::new();
        for _ in 0..MAX_QUEUED_MESSAGES {
            held.push(gate.admit(&ev("Runtime.bindingCalled", "{}")).expect("room"));
        }
        assert!(gate.admit(&ev("Runtime.bindingCalled", "{}")).is_err(), "a binding call counts against the message cap");
        for _ in 0..(MAX_QUEUED_EVENTS + 10) {
            held.push(gate.admit(&ev("Runtime.executionContextDestroyed", "{}")).expect("lifecycle is never refused"));
        }
        assert!(gate.admit(&Event::Message { view: 1, json: "{}".into(), source: String::new() }).is_err());
        assert!(gate.admit(&ev("Runtime.bindingCalled", &"x".repeat(MAX_MESSAGE + MAX_URL + 1))).is_err());
        assert_eq!(devtools_json_cap(), MAX_MESSAGE + MAX_URL);
        drop(held);
    }

    /// Task 2 review I2: console lines (either world) have their own small queue, so a page that logs
    /// heavily never makes the host drop the isolated world's real messages.
    #[test]
    fn console_lines_do_not_starve_real_messages() {
        let console_page = || Event::Message { view: 1, json: r#"{"h":"iTerm2ConsoleLog","b":{"msg":"x","level":"log"}}"#.into(), source: String::new() };
        let console_client = || Event::DevTools { view: 1, session: None, name: BINDING_EVENT.into(), json: r#"{"name":"tandemBinding","payload":"{\"h\":\"iTerm2ConsoleLog\",\"b\":{\"msg\":\"x\",\"level\":\"debug\"}}","executionContextId":3}"#.into() };
        let register = || Event::DevTools { view: 1, session: None, name: BINDING_EVENT.into(), json: r#"{"name":"tandemBinding","payload":"{\"h\":\"iTerm2GraphDiscovery\",\"b\":{\"type\":\"REGISTER_FRAME\"}}","executionContextId":3}"#.into() };
        let page_message = || Event::Message { view: 1, json: r#"{"h":"x","b":1}"#.into(), source: String::new() };
        assert!(console_page().is_console() && console_client().is_console());
        assert!(!register().is_console() && !page_message().is_console());
        let gate = ViewGate::new();
        let mut held = Vec::new();
        let mut admitted = 0;
        for i in 0..200 {
            let ev = if i % 2 == 0 { console_page() } else { console_client() };
            if let Ok(c) = gate.admit(&ev) {
                admitted += 1;
                held.push(c);
            }
        }
        assert_eq!(admitted, MAX_QUEUED_CONSOLE, "console lines stop at their own cap");
        assert!(gate.admit(&register()).is_ok(), "a frame registration is still admitted after 200 console lines");
        assert!(gate.admit(&page_message()).is_ok());
        // The ordinary message cap is untouched by the console lines.
        let mut real = Vec::new();
        for _ in 0..MAX_QUEUED_MESSAGES {
            real.push(gate.admit(&register()).expect("room"));
        }
        assert!(gate.admit(&register()).is_err());
        drop((held, real));
        assert!(gate.admit(&console_page()).is_ok(), "room comes back");
    }

    /// 13x2 Task 3 (B12): a menu request is never dropped (it holds a deferral), a pick is not
    /// either, and the target's strings are bounded before the event is queued.
    #[test]
    fn menu_events_follow_the_caps() {
        let target = MenuTarget { selection: Some("a".repeat(MAX_MENU_SELECTION + 5)), link: Some(format!("https://x/{}", "y".repeat(MAX_URL))), source: Some("https://i/p.png".into()), link_text: Some("t".repeat(MAX_MENU_LINK_TEXT + 9)), page_uri: "https://p/".into(), ..MenuTarget::default() };
        let capped = target.capped();
        assert_eq!(capped.selection.as_ref().unwrap().chars().count(), MAX_MENU_SELECTION);
        assert_eq!(capped.link, None, "a cut URL is dropped, never used whole");
        assert_eq!(capped.link_text.as_ref().unwrap().chars().count(), MAX_MENU_LINK_TEXT);
        assert_eq!(capped.source.as_deref(), Some("https://i/p.png"));
        let ask = Event::ContextMenu { view: 3, token: 1, target: capped };
        let pick = Event::ContextMenuPicked { view: 3, token: 1, id: 2 };
        assert_eq!((ask.view(), pick.view()), (3, 3));
        assert!(!ask.droppable() && !pick.droppable());
        let gate = ViewGate::new();
        let mut held = Vec::new();
        for _ in 0..(MAX_QUEUED_EVENTS + 2) {
            held.push(gate.admit(&ask).expect("a menu request is admitted over any cap"));
        }
        drop(held);
        // Multi-byte text is cut on a character boundary.
        let t = MenuTarget { selection: Some("\u{e9}".repeat(MAX_MENU_SELECTION + 1)), ..MenuTarget::default() }.capped();
        assert_eq!(t.selection.unwrap().chars().count(), MAX_MENU_SELECTION);
        assert_eq!(REMOVED_NATIVE, &["saveAs", "print"]);
    }

    /// 13x2 Task 1: the new events know their view; a frame message is capped like a message, a
    /// navigation start is never dropped.
    #[test]
    fn frame_and_starting_events_follow_the_caps() {
        let msg = |json: &str| Event::FrameMessage { view: 4, frame: 9, json: json.into(), source: "https://a/".into() };
        assert_eq!(msg("{}").view(), 4);
        assert!(msg("{}").droppable());
        assert_eq!(msg("ab").weight(), 2 + "https://a/".len());
        let starting = Event::Starting { view: 5, url: "https://a/".into(), kind: 2, user: true, redirect: false, nav: 7 };
        assert_eq!(starting.view(), 5);
        assert!(!starting.droppable());
        assert_eq!(starting.weight(), 10);
        assert_eq!(Event::FrameCreated { view: 6, frame: 1, name: "n".into() }.view(), 6);
        assert_eq!(Event::FrameGone { view: 6, frame: 1 }.view(), 6);
        let gate = ViewGate::new();
        let mut held = Vec::new();
        for _ in 0..MAX_QUEUED_MESSAGES {
            held.push(gate.admit(&msg("{}")).expect("room"));
        }
        assert!(gate.admit(&msg("{}")).is_err(), "frame messages share the message cap");
        assert!(gate.admit(&Event::Message { view: 4, json: "{}".into(), source: String::new() }).is_err());
        held.push(gate.admit(&starting).expect("a start is always admitted"));
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
        assert_eq!(nav_gate(Some(false), "https://a.example/", false, false), GateAct::Park, "a page's navigation waits for the sync");
        assert_eq!(nav_gate(Some(false), "https://a.example/", true, false), GateAct::Park, "so does a Tandem load that slipped past its call site");
        assert_eq!(nav_gate(Some(false), "about:blank", true, false), GateAct::Open, "Tandem's own blank page is the parking spot");
        assert_eq!(nav_gate(Some(false), "about:blank", false, false), GateAct::Cancel, "a page's blank is not Tandem's, and is not parked");
        assert_eq!(nav_gate(Some(false), "iterm2-about:permissions", false, false), GateAct::Open, "Tandem's own pages use no web permission");
        assert_eq!(nav_gate(Some(false), "https://a.example/", false, true), GateAct::Cancel, "never parked truncated");
        assert_eq!(nav_gate(Some(true), "https://a.example/", false, false), GateAct::Open);
        assert_eq!(nav_gate(None, "https://a.example/", false, false), GateAct::Open);
        let parked = Event::Parked { view: 4, url: "https://a.example/".into(), user: false, redirect: false, by_tandem: false, history: false, source: None };
        assert_eq!((parked.view(), parked.droppable(), parked.weight()), (4, false, 18), "a parked target is never dropped");
        let ev = Event::Refused { view: 3, url: "https://a.example/".into(), user: true };
        assert_eq!(ev.view(), 3);
        assert!(ev.droppable());
        assert_eq!(ev.weight(), 18);
    }

    #[test]
    fn local_schemes_register_with_their_own_origins() {
        assert_eq!(scheme_registration("iterm2-about"), (Vec::new(), false));
        assert_eq!(scheme_registration("iterm2-file"), (vec!["iterm2-about:".to_string(), "iterm2-file:".to_string()], false));
        assert_eq!(scheme_registration("iterm2-ssh"), (vec!["iterm2-about:".to_string(), "iterm2-ssh:".to_string()], true));
    }

    #[test]
    fn subframes_never_navigate_to_local_pages() {
        for u in ["iterm2-about:source", "ITERM2-ABOUT:history", "iterm2-about:blank", "iterm2-file:///C:/T/", "ITERM2-SSH://box/etc/passwd"] {
            assert!(frame_nav_cancelled(u), "{u}");
        }
        for u in ["https://a/", "about:blank", "iterm2-aboutx:y", "data:text/html,x", "https://a/iterm2-about:source"] {
            assert!(!frame_nav_cancelled(u), "{u}");
        }
    }

    /// 13x8 Task 3 (Review Focus 2): a `file:` URL becomes the `iterm2-file:` URL of the same drive
    /// path; a host, a UNC path or a non-drive path answers None.
    #[test]
    fn file_urls_follow_windows() {
        assert_eq!(iterm2_file_url("file:///C:/Users/u/a%20b.txt").as_deref(), Some("iterm2-file:///C:/Users/u/a%20b.txt"));
        assert_eq!(iterm2_file_url("file://localhost/C:/x").as_deref(), Some("iterm2-file:///C:/x"));
        assert_eq!(iterm2_file_url("FILE:///d:/").as_deref(), Some("iterm2-file:///d:/"));
        assert_eq!(iterm2_file_url("file:///C:").as_deref(), Some("iterm2-file:///C:"));
        for none in ["file://server/share/x", "file:////server/share", "file:///x", "file:///", "file:", "file:C:/x", "file:///C:x", "https://a/", "iterm2-file:///C:/x", ""] {
            assert_eq!(iterm2_file_url(none), None, "{none}");
        }
    }

    /// 13x8 Task 3 (preflight B5): only Tandem's load or a local page's click becomes a file page; a
    /// click on a link of a web page does not.
    #[test]
    fn a_web_page_cannot_start_a_file_page() {
        assert!(file_redirect_allowed(None, false) && file_redirect_allowed(Some("https://a/"), true));
        assert!(file_redirect_allowed(Some("iterm2-about:welcome"), false) && file_redirect_allowed(Some("iterm2-file:///C:/"), false));
        for web in ["https://a/", "http://a/", "about:blank", "data:text/html,x", "file:///C:/a.html", "blob:https://a/1"] {
            assert!(!file_redirect_allowed(Some(web), false), "{web}");
        }
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

    #[test]
    fn document_method_events_are_bounded() {
        let e = Event::DocumentMethod { view: 4, nav: 2, method: "POST".into() };
        assert_eq!(e.view(), 4);
        assert!(!e.droppable());
        assert_eq!(e.weight(), 4);
        assert_eq!(bounded_method("post"), "POST");
        assert_eq!(bounded_method("GET"), "GET");
        assert_eq!(bounded_method(""), "UNKNOWN");
        assert_eq!(bounded_method("A VERY LONG METHOD NAME"), "UNKNOWN");
        assert_eq!(bounded_method("GE\u{0}T"), "UNKNOWN");
    }

    #[test]
    fn resource_filters_match_like_webview2() {
        assert!(filter_matches("*", "https://a/b"));
        assert!(filter_matches("iterm2-about:*", "iterm2-about:history"));
        assert!(filter_matches("ITERM2-ABOUT:*", "iterm2-about:history"));
        assert!(!filter_matches("iterm2-about:*", "https://iterm2-about:x"));
        assert!(filter_matches("https://t.test/?x", "https://t.test/ax"));
        assert!(!filter_matches("https://t.test/a", "https://t.test/ab"));
    }

    /// Task 3: the permission, notification and audio events are plain data (they cross the
    /// channel as `Posted` values), and a deny answer differs from any state.
    #[test]
    fn permission_events_are_plain_data() {
        let events = vec![
            Event::Permission { view: 1, token: 2, kind: 4, uri: "https://a.b/".into() },
            Event::PermissionStates { view: 1, states: vec![(4, "https://a.b".into(), 1)], complete: true, workers_cleared: true },
            Event::NotificationShown { view: 1, token: 3, origin: "https://a.b".into(), title: "T".into(), body: "B".into() },
            Event::NotificationClosed { view: 1, token: 3 },
            Event::AudioPlaying { view: 1, playing: true },
            Event::PermissionWritten { view: 1, seq: 9, ok: false },
        ];
        for e in events {
            let posted = Posted { env: None, event: Some(e.clone()), charge: None };
            assert_eq!(posted.event, Some(e.clone()));
            assert_eq!(e.view(), 1);
        }
        assert_ne!(PermAnswer::State { state: 2, saves: false }, PermAnswer::Deny);
        assert_ne!(PermAnswer::State { state: 2, saves: false }, PermAnswer::State { state: 2, saves: true });
    }

    /// 13x8 Task 7: a download that is starting or ended holds a deferral or a result and is never
    /// dropped; progress is corrected by the next one; the page's URL is weighed.
    #[test]
    fn download_events_are_classed() {
        let start = Event::DownloadStarting { view: 2, token: 1, url: "https://h/f".into(), total: Some(5) };
        let done = Event::DownloadDone { view: 2, id: 9, result: Err("The download was interrupted".into()) };
        let progress = Event::DownloadProgress { view: 2, id: 9, received: 1, total: None };
        assert!(!start.droppable() && !done.droppable() && progress.droppable());
        assert_eq!((start.view(), done.view(), progress.view()), (2, 2, 2));
        assert_eq!((start.weight(), done.weight()), (11, 28));
    }

    /// 13x7 Task 10: the HTTP authentication events are never dropped (a challenge holds a COM
    /// deferral), what the server chose is weighed, and the navigation ids reach the tracker unchanged.
    #[test]
    fn integrated_authentication_is_never_offered_to_sites() {
        assert_eq!(browser_arguments(""), INTEGRATED_AUTH_ARG);
        assert_eq!(browser_arguments("--proxy-server=http://h:1"), format!("--proxy-server=http://h:1 {INTEGRATED_AUTH_ARG}"));
        assert!(INTEGRATED_AUTH_ARG.ends_with(".invalid"));
        let host = include_str!("webhost_win.rs");
        assert!(host.contains("set_allow_single_sign_on_using_os_primary_account(false)"));
        assert!(host.contains("webhost::browser_arguments(&opts.args)"));
    }

    #[test]
    fn navigation_ids_are_carried() {
        let auth = Event::BasicAuth { view: 1, token: 2, uri: "https://a.b/".into(), challenge: "Basic realm=\"R\"".into() };
        assert!(!auth.droppable() && !Event::Committed { view: 1, nav: 7 }.droppable() && !Event::Response { view: 1, nav: 7, status: 200 }.droppable());
        assert_eq!(auth.weight(), 12 + 15);
        assert_eq!((auth.view(), Event::Committed { view: 3, nav: 7 }.view(), Event::Response { view: 4, nav: 7, status: 401 }.view()), (1, 3, 4));
        let (start, done) = (
            Event::Starting { view: 1, url: "https://a.b/".into(), kind: 0, user: true, redirect: false, nav: 7 },
            Event::Navigated { view: 1, ok: true, status: 0, http: 0, url: "https://a.b/".into(), nav: 7 },
        );
        let mut t = crate::browser_basic_auth::NavTracker::default();
        let (Event::Starting { url, redirect, nav, .. }, Event::Navigated { ok, nav: done_nav, .. }) = (&start, &done) else { unreachable!() };
        assert!(!t.starting(*nav, url, *redirect).is_empty());
        assert_eq!(*done_nav, 7);
        assert!(t.completed(*done_nav, *ok).is_empty(), "the same id: a completed navigation after no commit still ends the live one once");
    }

    /// Pre-flight B2: the held ones are never dropped, the cheap state ones may be, and what a page
    /// chose is counted, so a flood cannot grow the queue unseen.
    #[test]
    fn new_events_are_weighed_and_dropped_by_the_rules() {
        let held = [
            Event::Permission { view: 1, token: 2, kind: 4, uri: "https://a.b/".into() },
            Event::NotificationShown { view: 1, token: 3, origin: "o".into(), title: "t".into(), body: "b".into() },
            Event::NotificationClosed { view: 1, token: 3 },
            Event::PermissionStates { view: 1, states: vec![], complete: false, workers_cleared: false },
            Event::PermissionWritten { view: 1, seq: 9, ok: true },
        ];
        for e in &held {
            assert!(!e.droppable(), "{e:?} holds a COM object or answers a request");
        }
        assert!(Event::AudioPlaying { view: 1, playing: true }.droppable(), "a state change the next one corrects");
        let shown = Event::NotificationShown { view: 1, token: 3, origin: "ooo".into(), title: "tt".into(), body: "b".into() };
        assert_eq!(shown.weight(), 6);
        assert_eq!(Event::Permission { view: 1, token: 2, kind: 4, uri: "https://a.b/".into() }.weight(), 12);
        let states = Event::PermissionStates { view: 1, states: vec![(4, "https://a.b".into(), 1), (3, "x".into(), 2)], complete: true, workers_cleared: true };
        assert_eq!(states.weight(), 12);
    }

    /// B2: a page's notification text is cut in characters before it is queued.
    #[test]
    fn notification_text_is_capped() {
        let long = "\u{e9}".repeat(5000);
        assert_eq!(cap_note_title(&long).chars().count(), NOTE_TITLE_CHARS);
        assert_eq!(cap_note_body(&long).chars().count(), NOTE_BODY_CHARS);
        assert_eq!(cap_note_title("short"), "short");
    }

    /// B2: at most 4 permission requests a second and 8 pending; the rest are denied at once.
    #[test]
    fn permission_requests_are_limited() {
        let mut l = PermissionLimiter::default();
        let t = Instant::now();
        for _ in 0..PERMISSIONS_PER_SECOND {
            assert!(l.allow(t, 0));
        }
        assert!(!l.allow(t, 0), "over the per-second budget");
        assert!(l.allow(t + Duration::from_millis(1100), 0), "a new second");
        assert!(!l.allow(t + Duration::from_secs(5), MAX_PENDING_PERMISSIONS), "too many pending");
    }

    /// B2: notifications are limited per minute and by how many the host holds.
    #[test]
    fn notifications_are_limited() {
        let mut l = NotificationLimiter::default();
        let t = Instant::now();
        for _ in 0..NOTES_PER_MINUTE {
            assert!(l.allow(t, 0));
        }
        assert!(!l.allow(t + Duration::from_secs(30), 0), "over the per-minute budget");
        assert!(l.allow(t + Duration::from_secs(61), 0), "a new minute");
        assert!(!l.allow(t + Duration::from_secs(200), MAX_HELD_NOTES), "too many held");
    }

    /// Final review M5 (ruling R13x6-SW): a service worker's request is refused, by its source kind or
    /// by the `Service-Worker` header of its script fetch, so no worker runs and no persistent
    /// notification reaches WebView2's own toast; notifications are offered only when both the
    /// events and that refusal are in place.
    #[test]
    fn service_workers_are_refused() {
        assert!(refuse_worker_request(SOURCE_SERVICE_WORKER, false), "a request from a service worker");
        assert!(refuse_worker_request(SOURCE_SERVICE_WORKER | 1, false), "a mask holding the service worker bit");
        assert!(refuse_worker_request(1, true), "a document request carrying the worker script header");
        assert!(!refuse_worker_request(1, false), "a page's own request continues");
        assert!(!refuse_worker_request(2, false), "a shared worker's request continues");
        assert!(notifications_offered(true, true, true));
        assert!(!notifications_offered(true, false, true), "events without the worker refusal: WebView2 would toast a persistent one itself");
        assert!(!notifications_offered(false, true, true), "no events: WebView2 would toast every one itself");
        // Last-gate LG-1: a worker an older build let install still runs from its cache until the
        // profile's registrations are cleared; until that clear succeeded, kind 4 is not offered.
        assert!(!notifications_offered(true, true, false), "registrations not cleared: an old worker could still toast");
    }
}
