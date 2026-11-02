//! The embedded browser. Windows hosts WebView2 (`webhost_win`, which stays a Windows-only module) and
//! shows web popovers in a popup window; Unix has no web view yet, so `available` reports Unsupported
//! and the callers say so where a page would open.

use super::Unsupported;

#[cfg(unix)]
use super::unix::browser as imp;
#[cfg(windows)]
use super::windows::browser as imp;

pub use imp::{
    Env, Rasterizer, WebPopover, WebView, cleanup_files, close_all, default_browser_name, fixed_runtime_check, focus_window, foreground_root,
    local_appdata, prepare_folder, runtime_version,
};

/// What the app tells the view about a notification it holds (`notification_report`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoteReport {
    /// Termurion showed it: `ReportShown`, the object stays held.
    Shown,
    /// It was closed (by the person, or the app took it away): `ReportClosed`, then released.
    Closed,
    /// Termurion will not show it: released without a report.
    Release,
}

/// How one view is made. Always built with `..Default::default()`: later plans add fields.
#[derive(Clone, Debug, Default)]
pub struct ViewSetup {
    /// The browser pane's permission sync gate (`perm_queue::ProfileQueue::gate`): while it reads
    /// false the main-frame NavigationStarting handler cancels and parks every navigation
    /// (`webhost::nav_parked`). None: always open.
    pub nav_gate: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    /// `WebUserAgent`'s setting: Some(setting) applies `webhost::user_agent(setting, engine)` (an
    /// empty setting keeps the engine's agent with "Termurion" appended); None leaves the engine's own.
    pub user_agent: Option<String>,
    /// Scripts run at the start of every document, in every frame.
    pub scripts: Vec<String>,
    pub devtools: bool,
    /// `NewWindowRequested` posts `Event::NewWindow` for a user-initiated http, https or `iterm2-about:` request;
    /// either way the request is handled and no popup window ever appears.
    pub new_windows: bool,
    /// Request filters (URI patterns, all contexts); every matching request is posted as `Event::Request` with a
    /// deferral and waits for `answer_request`.
    pub resource_filters: Vec<String>,
    pub transparent: bool,
    pub scale: Option<f64>,
    /// `IsInPrivateModeEnabled` on this view's controller options (R13xy-3).
    pub in_private: bool,
    /// The view's NavigationStarting handler asks `navigation_allowed` and cancels a refused
    /// navigation. Every view sets it (preflight B4c).
    pub guard_local_schemes: bool,
    /// A main-frame navigation to a `file:` URL the one verdict allowed is cancelled and posted as
    /// `Event::FileRedirect` when `webhost::file_redirect_allowed` says so (browser panes: the app shows
    /// the same path as an `iterm2-file:` page, 13x8 Task 3). WebView2 never lists a folder itself.
    pub redirect_file_urls: bool,
    /// Only these schemes may load (compared ignoring case); None allows any the guard allows.
    pub allowed_schemes: Option<Vec<String>>,
    /// `IsWebMessageEnabled`: true only for popover HTML, tool views and `iterm2-about:` pages (preflight P5).
    pub web_message: bool,
    /// The page's favicon is posted as `Event::Favicon` (browser panes): WebView2's own fetch,
    /// bounded and judged by `webhost::favicon_ok` before it is queued.
    pub favicon: bool,
    /// The page's right-click menu is the app's to decorate: `ContextMenuRequested` posts
    /// `Event::ContextMenu` and waits for `answer_context_menu` (browser panes).
    pub context_menu: bool,
    /// The app decides this view's permissions (browser panes): a request Termurion asks about is
    /// posted as `Event::Permission` and held until `answer_permission`; a page's notification
    /// is `Event::NotificationShown`; the audio state is `Event::AudioPlaying`. Off: every request
    /// is denied on the spot (tool views, popovers, Quick Look, the icon rasterizer).
    pub permissions: bool,
    /// The view reports the HTTP method of its main document requests (`Event::DocumentMethod`,
    /// 13x5): a second `WebResourceRequested` handler on a Document-context filter, no deferral.
    pub track_document_method: bool,
    /// A certificate error fails the load (`ServerCertificateErrorDetected` answers Cancel), so the
    /// app shows its error page and Edge's interstitial, with its "continue anyway", never appears.
    pub fail_cert_errors: bool,
    /// The view reports `Event::Committed` (ContentLoading) and `Event::Response` (the main
    /// document's HTTP response) for the HTTP authentication state machine (13x7). The response
    /// needs the Document filter `track_document_method` adds; browser panes set both.
    pub main_responses: bool,
    /// An HTTP (or proxy) authentication request is posted as `Event::BasicAuth` and held until
    /// `answer_basic_auth`. Off: the host cancels it on the spot; WebView2's own dialog is never shown.
    pub basic_auth: bool,
    /// The app decides this view's downloads (browser panes): `DownloadStarting` is posted as
    /// `Event::DownloadStarting` and held until `answer_download`. Off: a download is cancelled on
    /// the spot (a tool page, a popover or Quick Look starts none, and Edge's bubble never shows).
    pub downloads: bool,
}

/// The HTML popover's view: the bridge script, a message channel for it, no automatic windows, no
/// developer tools, every request answered by the popover's `LocalDoc` (the document, or 403), and
/// nothing may load after Termurion's own navigation to it (P6, B4). InPrivate: whatever the script
/// stores stays in memory and goes with the popover (each document has its own origin, so nothing
/// on disk would ever be read again).
pub fn html_setup() -> ViewSetup {
    ViewSetup {
        scripts: vec![crate::web_popover::WEBRTC_OFF_JS.to_string(), crate::webhost::INVOKE_JS.to_string()],
        guard_local_schemes: true,
        in_private: true,
        allowed_schemes: Some(Vec::new()),
        web_message: true,
        resource_filters: vec!["*".to_string()],
        ..Default::default()
    }
}

/// Quick Look's view (`WebUserAgent` as the factory applies it): no bridge, InPrivate (P7), http
/// and https only, through the one navigation verdict (B4).
pub fn quick_look_setup(user_agent: &str) -> ViewSetup {
    ViewSetup {
        user_agent: Some(user_agent.to_string()),
        in_private: true,
        guard_local_schemes: true,
        allowed_schemes: Some(vec!["http".to_string(), "https".to_string()]),
        ..Default::default()
    }
}

/// The friendly name of the registered handler of https, for the Composer's "This will open ... in
/// <browser>." line; None when there is none or the name is blank.
pub fn https_handler_name() -> Option<String> {
    imp::https_handler_name()
}

/// Ok when pages can be shown in a web view here.
pub fn available() -> Result<(), Unsupported> {
    imp::available()
}

/// Whether a new web view starts InPrivate (a fresh view's default setup says it does not).
pub fn default_view_in_private() -> bool {
    imp::default_view_in_private()
}
