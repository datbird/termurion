//! Browser panes (milestone 13x, Task 5): a pane whose `Io` is `Io::Browser` shows a web page. The
//! app keeps one `WebView` per browser pane in the app-wide `browser_views` (keyed by pane, so it
//! is not a `swap_live` field; a view follows its pane when the pane changes windows). Pages are
//! untrusted: the view is guarded by the one navigation verdict, gets no host objects and no
//! bridge, permissions are denied by the host, and an external protocol is opened only through
//! `guard_open`. Tandem keeps nothing of a page but its URL (an arrangement) and its title.

use super::*;
use crate::browser::{self, BrowserIo, Start};
use crate::layout::Rect;
use crate::webhost::{self, Event, Runtime, ViewId};
use crate::webhost_win::{ViewSetup, WebView};

/// The view of one browser pane.
pub(super) struct BrowserView {
    /// None until the environment exists (and again after a window closed its views).
    pub(super) view: Option<WebView>,
    pub(super) id: ViewId,
    /// The URL a view made now loads (set at the start and after a failure or a window's close).
    pub(super) pending_url: Option<String>,
    /// The window handle the view is a child of.
    pub(super) parent: isize,
    /// The view was recreated once after a failure (a second failure is reported, not retried).
    pub(super) retried: bool,
    /// The page finished loading (`BST`'s `navigationSettled`).
    pub(super) settled: bool,
}

/// How a browser view is made: the browser's agent (the engine's with "Tandem" appended), developer
/// tools on, every navigation guarded, new windows posted, InPrivate for Dev Null Mode. No bridge
/// (`web_message` off: `iterm2-about:` pages come with Task 7) and no host objects.
pub(super) fn browser_setup(dev_null: bool) -> ViewSetup {
    ViewSetup { user_agent: Some(String::new()), devtools: true, new_windows: true, guard_local_schemes: true, in_private: dev_null, web_message: false, ..Default::default() }
}

/// The text a browser profile shows when there is no WebView2 Runtime (Task 9 makes it the dialog).
pub(super) const UPSELL_TEXT: &str = "Tandem can display web pages! But first you must install the WebView2 Runtime.";

impl App {
    /// Whether a profile starts a browser pane: a Browser profile with `BrowserProfiles` on.
    pub(super) fn is_browser_profile(&self, p: &Profile) -> bool {
        browser::is_browser(&p.string("Custom Command")) && self.globals.bool("BrowserProfiles")
    }

    /// Whether `pane` is a browser pane.
    pub(super) fn is_browser_pane(&self, pane: PaneId) -> bool {
        self.panes.get(&pane).is_some_and(|p| p.browser_io().is_some())
    }

    /// A screen reader's name for a browser pane (B7), None for any other pane.
    pub(super) fn browser_access_label(&self, pane: PaneId) -> Option<String> {
        self.panes.get(&pane)?.browser_io().map(|io| browser::access_label(&io.title))
    }

    /// Starts browser pane `id` at `rect` for profile `p`. False (with a banner) when there is no
    /// WebView2 Runtime: the pane is not made.
    pub(super) fn start_browser_pane(&mut self, id: PaneId, rect: Rect, p: &Profile) -> bool {
        let restored = self.browser_url_override.take();
        let url = match browser::start_plan(&p.string("Custom Command"), self.globals.bool("BrowserProfiles"), &p.string("Initial URL"), restored.as_deref()) {
            Start::Browser(u) => u,
            Start::Terminal => return false,
        };
        if self.web_runtime() == Runtime::Missing {
            self.banners.set("webview2", UPSELL_TEXT);
            return false;
        }
        let dev_null = p.bool("Dev Null Mode");
        let zoom = if p.int("Browser Zoom") > 0 { p.int("Browser Zoom") as f64 } else { 100.0 };
        let io = BrowserIo { url: url.clone(), dev_null, zoom, ..Default::default() };
        let cell = self.profile_cell(p);
        let (cols, rows) = self.grid_size_in(rect, cell, self.pane_room_for(id, p));
        let mut pane = Pane::browser(id, cols, rows, io);
        // The session name starts as the profile's name (`PTYSession.swift:1620-1630`).
        let name = p.name();
        pane.screen.set_title(Some(browser::shown_title(if name.trim().is_empty() { browser::DEFAULT_NAME } else { &name })));
        self.install_pane(id, pane, p, false);
        let view = self.next_view_id();
        self.browser_views.insert(id, BrowserView { view: None, id: view, pending_url: Some(url), parent: 0, retried: false, settled: false });
        true
    }

    /// Every frame, before the frame is built (P24: the host calls COM only for a change): each
    /// view of this window is placed in its pane's rectangle and shown, or hidden when its tab is
    /// not active, its pane is buried or zoomed away. Also the focus hand-over between panes.
    pub(super) fn place_browser_views(&mut self) {
        if self.browser_views.is_empty() {
            return;
        }
        let Some(hwnd) = self.hwnd() else { return };
        let shown: Vec<(PaneId, Rect)> = match self.tabs.get(self.active) {
            Some(tab) => self.visible(tab).into_iter().map(|(id, r)| (id, self.pane_chrome(tab, id, r).rest)).collect(),
            None => Vec::new(),
        };
        let mine: Vec<PaneId> = self.tabs.iter().flat_map(|t| t.layout.panes()).collect();
        let focused = self.focused_pane_id();
        let panes: Vec<PaneId> = self.browser_views.keys().copied().collect();
        for pane in panes {
            let rect = shown.iter().find(|(p, _)| *p == pane).map(|(_, r)| *r);
            match rect {
                Some(rect) => self.show_browser_view(pane, rect, hwnd, focused == Some(pane)),
                None => {
                    // Not shown: hidden when it is this window's pane (another tab, buried, zoomed
                    // away) or sits in this window's handle with no pane (an Undo Close stash).
                    if let Some(bv) = self.browser_views.get(&pane) {
                        if mine.contains(&pane) || bv.parent == hwnd {
                            if let Some(v) = &bv.view {
                                v.set_visible(false);
                            }
                        }
                    }
                }
            }
        }
        // Keyboard focus follows the active pane when it changes within this window.
        let seen = std::mem::replace(&mut self.focus_seen, focused);
        if seen != focused {
            let prev = seen.filter(|p| mine.contains(p));
            self.browser_focus_moved(prev, focused);
            if let Some(next) = focused.filter(|p| self.is_browser_pane(*p)) {
                if let Some(v) = self.browser_views.get(&next).and_then(|b| b.view.as_ref()) {
                    v.focus();
                }
            } else if prev.is_some_and(|p| self.is_browser_pane(p)) && self.web_focus_held == self.current_window() {
                // The keyboard was in the page; it comes back to the terminal.
                crate::webhost_win::focus_window(hwnd);
            }
        }
    }

    /// Makes (when the environment is ready), places and shows pane `pane`'s view.
    fn show_browser_view(&mut self, pane: PaneId, rect: Rect, hwnd: isize, focus_now: bool) {
        let Some(io) = self.panes.get(&pane).and_then(|p| p.browser_io()).cloned() else { return };
        let missing = self.browser_views.get(&pane).is_some_and(|b| b.view.is_none());
        if missing {
            let (id, url) = {
                let bv = &self.browser_views[&pane];
                (bv.id, bv.pending_url.clone().unwrap_or_else(|| io.url.clone()))
            };
            let sink = self.web_envs.sink();
            let Some(env) = self.web_env(browser::store_of(io.dev_null)) else { return };
            let view = WebView::create(env, hwnd, id, webhost::physical_bounds(rect), browser_setup(io.dev_null), sink);
            view.navigate_tandem(&url);
            view.set_zoom(if io.zoom > 0.0 { io.zoom / 100.0 } else { 1.0 });
            if focus_now {
                view.focus();
            }
            if let Some(bv) = self.browser_views.get_mut(&pane) {
                bv.view = Some(view);
                bv.pending_url = None;
                bv.parent = hwnd;
                bv.settled = false;
            }
        }
        let Some(bv) = self.browser_views.get_mut(&pane) else { return };
        if let Some(v) = &bv.view {
            if bv.parent != hwnd {
                v.set_parent(hwnd);
                bv.parent = hwnd;
            }
            v.set_bounds(webhost::physical_bounds(rect));
            v.set_visible(true);
        }
    }

    /// The window of this app's current window was made again: this window's views move to the
    /// new handle before the old window is destroyed.
    pub(super) fn browser_views_remade(&mut self, new_hwnd: isize) {
        let mine: Vec<PaneId> = self.tabs.iter().flat_map(|t| t.layout.panes()).collect();
        for (pane, bv) in self.browser_views.iter_mut() {
            if mine.contains(pane) && bv.parent != new_hwnd {
                if let Some(v) = &bv.view {
                    v.set_parent(new_hwnd);
                }
                bv.parent = new_hwnd;
            }
        }
    }

    /// This window is going (or hiding): its views close while their parent exists, and are made
    /// again from the pane's URL if the pane is shown again.
    pub(super) fn close_browser_views(&mut self) {
        let mine: Vec<PaneId> = self.tabs.iter().flat_map(|t| t.layout.panes()).collect();
        for pane in mine {
            let url = self.panes.get(&pane).and_then(|p| p.browser_io()).map(|io| io.url.clone());
            if let Some(bv) = self.browser_views.get_mut(&pane) {
                bv.view = None;
                bv.pending_url = url;
            }
        }
    }

    /// A pane was closed for good: its view closes.
    pub(super) fn close_browser_view(&mut self, pane: PaneId) {
        self.browser_views.remove(&pane);
        if self.focus_seen == Some(pane) {
            self.focus_seen = None;
        }
    }

    /// The pane that owns view `view`.
    fn browser_pane_of(&self, view: ViewId) -> Option<PaneId> {
        if view == 0 {
            return None;
        }
        self.browser_views.iter().find(|(_, b)| b.id == view).map(|(p, _)| *p)
    }

    /// Runs `f` with the window of `pane` current, then goes back.
    fn in_pane_window(&mut self, pane: PaneId, f: impl FnOnce(&mut App)) -> bool {
        let Some(target) = self.pane_window_id(pane) else { return false };
        let back = self.current_window();
        if !self.enter(target) {
            return false;
        }
        f(self);
        if let Some(b) = back {
            self.enter(b);
        }
        true
    }

    /// A view event of a browser pane. True when the view is a browser pane's.
    pub(super) fn browser_view_event(&mut self, event: &Event, event_loop: &ActiveEventLoop) -> bool {
        let Some(pane) = self.browser_pane_of(event.view()) else { return false };
        self.browser_event(pane, event, event_loop);
        true
    }

    /// One event of pane `pane`'s view. What a page says is bounded and cleaned before it is kept
    /// (the host already capped its size); nothing here runs a page's text.
    pub(super) fn browser_event(&mut self, pane: PaneId, e: &Event, _event_loop: &ActiveEventLoop) {
        match e {
            Event::Source(_, url) => {
                if let Some(io) = self.panes.get_mut(&pane).and_then(|p| p.browser_io_mut()) {
                    io.url = url.clone();
                }
            }
            Event::Title(_, title) => {
                if let Some(io) = self.panes.get_mut(&pane).and_then(|p| p.browser_io_mut()) {
                    io.title = browser::shown_title(title);
                }
                self.browser_name(pane);
            }
            Event::Favicon(_, bytes) => {
                if let Some(io) = self.panes.get_mut(&pane).and_then(|p| p.browser_io_mut()) {
                    io.favicon = Some(bytes.clone());
                }
            }
            Event::Loading(_, on) => {
                if let Some(io) = self.panes.get_mut(&pane).and_then(|p| p.browser_io_mut()) {
                    io.loading = *on;
                }
                if *on {
                    if let Some(bv) = self.browser_views.get_mut(&pane) {
                        bv.settled = false;
                    }
                }
                self.redraw();
            }
            Event::Navigated { .. } => {
                if let Some(bv) = self.browser_views.get_mut(&pane) {
                    bv.settled = true;
                }
                self.browser_name(pane);
            }
            Event::History { back, forward, .. } => {
                if let Some(io) = self.panes.get_mut(&pane).and_then(|p| p.browser_io_mut()) {
                    io.can_back = *back;
                    io.can_forward = *forward;
                }
            }
            // The page has the keyboard: its pane is the active one (iTerm2's session change).
            Event::Focus(_, true) => {
                self.in_pane_window(pane, |a| {
                    if let Some(t) = a.tabs.iter_mut().find(|t| t.layout.panes().contains(&pane)) {
                        t.focused = pane;
                    }
                    a.redraw();
                });
            }
            Event::NewWindow { url, .. } => {
                // Only a person's click (the host filters) and only a page's own kind of URL (P20).
                if let Some(url) = browser::saved_url(url) {
                    let guid = self.panes.get(&pane).map(|p| p.profile.clone());
                    self.browser_url_override = Some(url);
                    let opened = self.in_pane_window(pane, |a| a.new_tab(guid.as_deref()));
                    if !opened {
                        self.browser_url_override = None;
                    }
                }
            }
            Event::ExternalUri { url, user, .. } | Event::Refused { url, user, .. } => match super::web_popover_app::external_uri_run(url, *user) {
                super::web_popover_app::ExternalRun::Open(t) => {
                    if let Err(e) = super::semantic_ui::shell_open(&t, false) {
                        self.banners.set("action", format!("Could not open {t}: {e}"));
                    }
                }
                super::web_popover_app::ExternalRun::Notice(n) => self.banners.set("action", n),
                super::web_popover_app::ExternalRun::Nothing => {}
            },
            Event::Failed(_, why) => {
                crate::host::log(&format!("webhost: a browser pane's view failed: {why}"));
                let url = self.panes.get(&pane).and_then(|p| p.browser_io()).map(|io| io.url.clone());
                if let Some(bv) = self.browser_views.get_mut(&pane) {
                    bv.view = None;
                    if bv.retried {
                        bv.pending_url = None;
                        self.banners.set("window", "A browser pane's web view stopped working.");
                    } else {
                        bv.retried = true;
                        bv.pending_url = url;
                    }
                }
                self.redraw();
            }
            // Messages, requests and keys have no owner until the later tasks; pages get no bridge.
            _ => {}
        }
    }

    /// The session name after a title or a finished navigation (`BST:24-43`).
    fn browser_name(&mut self, pane: PaneId) {
        let settled = self.browser_views.get(&pane).is_some_and(|b| b.settled);
        let profile = self.panes.get(&pane).and_then(|p| self.store.get(&p.profile)).map(|p| p.name()).unwrap_or_default();
        let Some(io) = self.panes.get(&pane).and_then(|p| p.browser_io()) else { return };
        let host = crate::toolbelt::url_host(&io.url).map(str::to_string);
        let fallback = if profile.trim().is_empty() { browser::DEFAULT_NAME.to_string() } else { profile };
        let name = browser::resolved_name(Some(&io.title), host.as_deref(), &fallback, settled);
        if let (Some(name), Some(p)) = (name, self.panes.get_mut(&pane)) {
            p.screen.set_title(Some(browser::shown_title(&name)));
        }
        self.redraw();
    }
}

/// A browser's saved URL of an arrangement's session table, judged again on load.
pub(super) fn restored_url(session: &toml::Value) -> Option<String> {
    browser::saved_url(session.get(browser::URL_KEY)?.as_str()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pane::Pane;

    /// Starting a pane with a Browser profile makes a pane with no pty (13x Task 5, `GW:98-111`).
    #[test]
    fn browser_pane_starts_as_iterm2() {
        let Start::Browser(url) = browser::start_plan("Browser", true, "", None) else { panic!("a browser") };
        assert_eq!(url, browser::DEFAULT_URL);
        let p = Pane::browser(7, 80, 24, BrowserIo { url: url.clone(), zoom: 100.0, ..Default::default() });
        assert_eq!(p.browser_io().map(|io| io.url.as_str()), Some(browser::DEFAULT_URL));
        assert!(p.shell_pid().is_none() && p.exited.is_none());
        assert!(!p.screen.pty_backed, "its screen is never drawn");
        // The same profile with the setting off is a terminal.
        assert_eq!(browser::start_plan("Browser", false, "https://a.test/", None), Start::Terminal);
        assert_eq!(browser::window_cells(1000, 800), (1000, 800));
        assert_eq!(browser::window_cells(0, 5000), (1, 1250));
        assert_eq!(browser::store_of(true), webhost::Store::BrowserDevNull);
        // The views are made in the Dev Null store InPrivate, and never with a bridge or host objects.
        assert!(browser_setup(true).in_private && !browser_setup(false).in_private);
        let s = browser_setup(false);
        assert!(s.guard_local_schemes && !s.web_message && s.user_agent.as_deref() == Some(""));
    }

    #[test]
    fn a_browser_pane_writes_nothing_and_never_exits() {
        let io = crate::pane::Io::Browser(BrowserIo::default());
        assert!(io.write(b"x").is_ok() && io.resize(3, 3).is_ok() && io.clear().is_ok() && io.pid().is_none() && io.exit_code().is_none() && io.pending() == 0);
        assert!(matches!(io.try_read(), crate::pty::Read::Empty));
    }

    #[test]
    fn a_restored_url_is_judged_again() {
        let mut t = toml::Table::new();
        t.insert(browser::URL_KEY.into(), toml::Value::String("https://u:p@a.test/x".into()));
        assert_eq!(restored_url(&toml::Value::Table(t.clone())).as_deref(), Some("https://a.test/x"));
        t.insert(browser::URL_KEY.into(), toml::Value::String("javascript:alert(1)".into()));
        assert_eq!(restored_url(&toml::Value::Table(t)), None);
        assert_eq!(restored_url(&toml::Value::Table(toml::Table::new())), None);
    }

    /// Every web view kind registers with the remake hook; the browser's is in the list.
    #[test]
    fn browser_views_are_hosted_views() {
        assert!(super::super::webhost_app::HOSTED_VIEWS.iter().any(|h| h.file == "browser_app.rs"));
    }

    /// The only COM handlers are the host's; a browser pane gets no bridge and no host objects.
    #[test]
    fn browser_views_get_no_page_bridge() {
        let src = include_str!("browser_app.rs");
        let prod = &src[..src.find("#[cfg(test)]").unwrap()];
        assert!(!prod.contains("web_message: true") && !prod.contains("navigate_to_string") && !prod.contains("HostObject"));
    }
}
