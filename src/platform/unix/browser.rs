//! No embedded browser on Unix yet. Every constructor reports Unsupported ("Browser"), so the stand-in
//! types below can never exist; their methods are `match self.never {}` like `platform::unix::pty::Pty`.

use std::convert::Infallible;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::WindowId;

use super::super::Unsupported;
use super::super::browser::NoteReport;
use crate::layout::Rect;
use crate::web_popover::{Act, Kind};
use crate::webhost::{self, Answer, EnvOptions, Event, Sink, Store, ViewId};

const FEATURE: &str = "Browser";

fn no() -> String {
    Unsupported { feature: FEATURE }.to_string()
}

pub fn available() -> Result<(), Unsupported> {
    super::super::unsupported(FEATURE)
}

/// The registered handler of http is not looked up on Unix yet; the button says "Default Browser".
pub fn default_browser_name() -> Option<String> {
    None
}

/// The handler of https is not looked up on Unix yet.
pub fn https_handler_name() -> Option<String> {
    None
}

/// A fresh view's default setup (nothing starts InPrivate).
pub fn default_view_in_private() -> bool {
    ViewSetup::default().in_private
}

/// There is no web view runtime; the Windows call reports a missing runtime the same way (Err).
pub fn runtime_version(_fixed: Option<&Path>) -> Result<String, i32> {
    Err(-1)
}

/// A fixed-runtime hint cannot be used without a browser.
pub fn fixed_runtime_check(_hint: &str) -> Result<PathBuf, String> {
    Err(no())
}

/// No browser data folder is known, so every browser store reports "no folder".
pub fn local_appdata() -> Option<PathBuf> {
    None
}

pub fn prepare_folder(_folder: &Path) -> Result<(), String> {
    Err(no())
}

/// Nothing is on disk for a browser that cannot run (process teardown calls it on every OS).
pub fn cleanup_files() {}

/// No view is ever live, so none closes (process teardown calls it on every OS).
pub fn close_all() -> usize {
    0
}

/// No web view takes the foreground or the keyboard here.
pub fn foreground_root() -> Option<isize> {
    None
}

/// No web view holds the keyboard here, so there is nothing to give back (a documented no-op).
pub fn focus_window(_hwnd: isize) {}

/// One browser environment. Nothing makes one on Unix.
#[derive(Clone)]
pub struct Env {
    pub store: Store,
    pub(super) never: Infallible,
}

impl Env {
    /// Calls `done` at once with Err(Unsupported text), like the Windows call when nothing can be started.
    pub fn create(_store: Store, _folder: &Path, _fixed: Option<&Path>, _opts: &EnvOptions, done: Box<dyn FnOnce(Result<Env, String>)>) {
        done(Err(no()));
    }
}

/// How one view is made; mirrors the public fields of the Windows setup (a setup is data, it is
/// built on every OS and read by nothing here).
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

/// One web view. Nothing makes one on Unix.
pub struct WebView {
    never: Infallible,
}

impl WebView {
    /// An `Env` cannot exist here, so neither can a view.
    pub fn create(env: &Env, _parent_hwnd: isize, _id: ViewId, _bounds: (i32, i32, i32, i32), _setup: ViewSetup, _sink: Sink) -> WebView {
        match env.never {}
    }

    pub fn navigate(&self, url: &str) {
        match self.never {}
    }

    pub fn navigate_model(&self, url: &str) {
        match self.never {}
    }

    pub fn navigate_termurion(&self, url: &str) {
        match self.never {}
    }

    pub fn set_bounds(&self, b: (i32, i32, i32, i32)) {
        match self.never {}
    }

    pub fn set_visible(&self, on: bool) {
        match self.never {}
    }

    pub fn set_parent(&self, hwnd: isize) {
        match self.never {}
    }

    pub fn set_key_gate(&self, gate: webhost::KeyGate) {
        match self.never {}
    }

    pub fn engine_autofill(&self) -> Option<(bool, bool)> {
        match self.never {}
    }

    pub fn focus(&self) {
        match self.never {}
    }

    pub fn execute_script(&self, js: &str, token: u64) {
        match self.never {}
    }

    pub fn devtools_call(&self, method: &str, params_json: &str, token: u64) {
        match self.never {}
    }

    pub fn devtools_call_for_session(&self, session: &str, method: &str, params_json: &str, token: u64) {
        match self.never {}
    }

    pub fn take_devtools_held(&self) -> Vec<(Option<String>, String, String)> {
        match self.never {}
    }

    pub fn devtools_subscribe(&self, event: &str) {
        match self.never {}
    }

    pub fn watch_frames(&self) {
        match self.never {}
    }

    pub fn execute_script_in_frame(&self, frame: u64, js: &str, token: u64) {
        match self.never {}
    }

    pub fn post_json(&self, json: &str) {
        match self.never {}
    }

    pub fn go_back(&self) {
        match self.never {}
    }

    pub fn go_forward(&self) {
        match self.never {}
    }

    pub fn delete_all_cookies(&self) {
        match self.never {}
    }

    pub fn clear_browsing_data(&self) {
        match self.never {}
    }

    pub fn set_local_root(&self, root: Option<crate::browser_pages::PageRoot>) {
        match self.never {}
    }

    pub fn local_root(&self) -> Option<crate::browser_pages::PageRoot> {
        match self.never {}
    }

    pub fn add_extension(&self, folder: &Path) {
        match self.never {}
    }

    pub fn list_extensions(&self) {
        match self.never {}
    }

    pub fn set_extension_enabled(&self, ext_id: &str, on: bool) {
        match self.never {}
    }

    pub fn remove_extension(&self, ext_id: &str) {
        match self.never {}
    }

    pub fn reload(&self) {
        match self.never {}
    }

    pub fn stop(&self) {
        match self.never {}
    }

    pub fn set_zoom(&self, factor: f64) {
        match self.never {}
    }

    pub fn capture_png(&self, token: u64) {
        match self.never {}
    }

    pub fn capture(&self, token: u64, jpeg: bool) {
        match self.never {}
    }

    pub fn answer_request(&self, token: u64, answer: Answer) {
        match self.never {}
    }

    pub fn print_ui(&self) {
        match self.never {}
    }

    pub fn answer_context_menu(&self, token: u64, entries: Vec<webhost::MenuEntry>) {
        match self.never {}
    }

    pub fn answer_permission(&self, token: u64, answer: webhost::PermAnswer) {
        match self.never {}
    }

    pub fn answer_basic_auth(&self, token: u64, answer: Option<(String, crate::pwm::Secret)>) {
        match self.never {}
    }

    pub fn answer_download(&self, token: u64, id: u64, path: Option<&Path>) {
        match self.never {}
    }

    pub fn cancel_download(&self, id: u64) {
        match self.never {}
    }

    pub fn notification_report(&self, token: u64, r: NoteReport) {
        match self.never {}
    }

    pub fn set_muted(&self, on: bool) {
        match self.never {}
    }

    pub fn is_muted(&self) -> bool {
        match self.never {}
    }

    pub fn set_permission_state(&self, seq: u64, kind: i32, origin: &str, state: i32) {
        match self.never {}
    }

    pub fn read_permission_states(&self) {
        match self.never {}
    }

    pub fn workers_cleared(&self) -> bool {
        match self.never {}
    }

    pub fn close(self) {
        match self.never {}
    }
}

/// A popover window. Nothing makes one on Unix.
pub struct WebPopover {
    never: Infallible,
    pub limiter: crate::web_popover::BridgeLimiter,
}

impl WebPopover {
    #[allow(clippy::too_many_arguments)]
    pub fn open(_event_loop: &ActiveEventLoop, _instance: &wgpu::Instance, _adapter: &wgpu::Adapter, _device: &wgpu::Device, _queue: &wgpu::Queue, _kind: Kind, _size: (f64, f64), _at: (f64, f64), _work: Option<Rect>) -> Result<WebPopover, String> {
        Err(no())
    }

    pub fn id(&self) -> WindowId {
        match self.never {}
    }

    pub fn hwnd(&self) -> Option<isize> {
        match self.never {}
    }

    pub fn kind(&self) -> &Kind {
        match self.never {}
    }

    pub fn view_id(&self) -> ViewId {
        match self.never {}
    }

    pub fn has_view(&self) -> bool {
        match self.never {}
    }

    pub fn view(&self) -> Option<&WebView> {
        match self.never {}
    }

    pub fn age(&self) -> Duration {
        match self.never {}
    }

    pub fn web_rect(&self) -> Rect {
        match self.never {}
    }

    pub fn attach(&mut self, _view: WebView, _id: ViewId) {
        match self.never {}
    }

    pub fn set_current(&mut self, _url: &str) {
        match self.never {}
    }

    pub fn set_doc(&mut self, _doc: webhost::LocalDoc) {
        match self.never {}
    }

    pub fn trusts(&self, _source: &str) -> bool {
        match self.never {}
    }

    pub fn answer(&self, _token: u64, _url: &str) {
        match self.never {}
    }

    pub fn open_in_target(&self) -> Option<String> {
        match self.never {}
    }

    pub fn lost_foreground(&mut self) -> bool {
        match self.never {}
    }

    pub fn event(&mut self, _e: &WindowEvent) -> Act {
        match self.never {}
    }
}

/// One SVG render. Nothing starts one on Unix.
pub struct Rasterizer {
    never: Infallible,
}

impl Rasterizer {
    /// An `Env` cannot exist here, so a render cannot start.
    pub fn start(env: &Env, _svg: &str, _id: ViewId, _sink: Sink) -> Result<Rasterizer, String> {
        match env.never {}
    }

    pub fn id(&self) -> ViewId {
        match self.never {}
    }

    pub fn started(&self) -> Instant {
        match self.never {}
    }

    pub fn expired(&self) -> bool {
        match self.never {}
    }

    pub fn on_event(&mut self, _e: &Event) -> Option<Result<Vec<u8>, ()>> {
        match self.never {}
    }

    pub fn limit(&mut self) -> Option<Result<Vec<u8>, ()>> {
        match self.never {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unix_browser_unsupported() {
        assert!(available().unwrap_err().to_string().contains("Browser is not available"));
        assert_eq!(default_browser_name(), None);
    }

    #[test]
    fn unix_browser_reports_unsupported() {
        let got = std::rc::Rc::new(std::cell::RefCell::new(None));
        let g = got.clone();
        Env::create(Store::Default, Path::new("x"), None, &EnvOptions::default(), Box::new(move |r| *g.borrow_mut() = Some(r.map(|_| ()))));
        let r = got.borrow_mut().take().expect("done runs at once");
        assert!(r.unwrap_err().contains("Browser is not available"));
        assert!(runtime_version(None).is_err());
        assert!(fixed_runtime_check("x").unwrap_err().contains("Browser is not available"));
        assert!(prepare_folder(Path::new("x")).unwrap_err().contains("Browser is not available"));
    }
}
