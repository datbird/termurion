//! Browser panes (milestone 13x, Task 5): a pane whose `Io` is `Io::Browser` shows a web page. The
//! app keeps one `WebView` per browser pane in the app-wide `browser_views` (keyed by pane, so it
//! is not a `swap_live` field; a view follows its pane when the pane changes windows). Pages are
//! untrusted: the view is guarded by the one navigation verdict, gets no host objects and no
//! bridge, permissions are denied by the host, and an external protocol is opened only through
//! `guard_open`. Tandem keeps nothing of a page but its URL (an arrangement) and its title.

use super::*;
use crate::browser::{self, BrowserIo, Button, Hit, Start, Suggestion};
use crate::layout::Rect;
use crate::palette::Rgb;
use crate::text_edit::{self, EditKey, EditOut};
use winit::keyboard::{Key as WKey, NamedKey};
use crate::webhost::{self, Event, ParentFate, Runtime, ViewId};
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
    /// The remake budget after failures (`webhost::Restarts`; a good start resets it).
    pub(super) restarts: webhost::Restarts,
    /// Over the budget: no view is made until the person presses Reload or navigates.
    pub(super) stopped: bool,
    /// The page finished loading (`BST`'s `navigationSettled`).
    pub(super) settled: bool,
    /// The last title the page sent for this load (kept for the history row; bounded by the host).
    pub(super) last_title: Option<String>,
    /// The toolbar's state: the address field, its suggestions, a press, the history list.
    pub(super) bar: BarState,
    /// Browser Instant Replay: this pane's frames and the state of the capture round trip.
    pub(super) replay: super::web_replay_app::Recorder,
    /// The view's own secret (13x2): filled only into Client scripts and world-side expressions; new
    /// for every view made (`browser_reset_world`).
    pub(super) secret: String,
    /// A second per-view value that the page is assumed to read (a DOM message from a copied script's
    /// child frame): never the view secret, only "this came from Tandem's detector".
    pub(super) dom_secret: String,
    /// Calls waiting for an answer, by token (`browser_scripts_app::JsDone`).
    pub(super) pending_js: HashMap<u64, super::browser_scripts_app::JsDone>,
    /// The frame keys that have a Tandem world, main frame excluded (`Contexts::frames`).
    pub(super) frames: Vec<u64>,
    /// WebView2's own frames, for page-world messages only.
    pub(super) page_frames: Vec<u64>,
    /// Tokens of page-world `execute_script` calls whose answer is the result itself.
    pub(super) eval_waiting: std::collections::HashSet<u64>,
    pub(super) client_world: crate::browser_world::ClientWorld,
    pub(super) contexts: crate::browser_world::Contexts,
    /// The `identifier`s `Page.addScriptToEvaluateOnNewDocument` answered.
    pub(super) world_scripts: Vec<String>,
    /// The next token of a call Tandem makes on this view.
    pub(super) next_token: u64,
    /// The world's setup, checks, sessions (`browser_scripts_app::WorldState`).
    pub(super) world: super::browser_scripts_app::WorldState,
    /// A text field has the focus in the page (iTerm2's `isEditingText`, `BWV:73`).
    pub(super) editing: bool,
    /// The key gate the view's accelerator handler asks (`browser_input_app`), and what it was built for.
    pub(super) gate: webhost::KeyGate,
    pub(super) gate_sig: Option<(bool, Option<String>, bool, bool, Vec<String>)>,
    /// Copy mode on this page (13x3 Task 8): iTerm2's page cursor is on and the keys are its. Gone with
    /// the view, and reset with the world's secret.
    pub(super) copy_mode: Option<crate::browser_copymode::Model>,
    /// Reader Mode and Remove Distractions (13x4): the scripts live in the isolated world, so the state
    /// dies with it (`State::reset` wherever the world restarts, is lost or a navigation starts).
    pub(super) reader: crate::browser_reader::State,
    /// The Menu press is waiting for the page's plain text; a second press is ignored (B11).
    pub(super) menu_pending: bool,
    /// The pointer table last given to the page (`browser_input_app`), and the press of a bound
    /// button that is down (button, clicks): its release runs the binding.
    pub(super) pointer_sig: Option<(Vec<String>, bool)>,
    pub(super) pointer_down: Option<(u8, u8)>,
    /// Replays of broadcast input in flight to this view (iTerm2's `receivingBroadcast`, `BM:882-899`),
    /// since when; the page's own events while it is above 0 are dropped natively, so nothing ping-pongs.
    pub(super) receiving_broadcast: u32,
    pub(super) receiving_since: Option<Instant>,
    /// The inputs a replay sent this view, so its own report of one is dropped (Task 8 review I1).
    pub(super) echo: crate::browser_keys::EchoGuard,
    /// The page's selection (iTerm2's `currentSelection`, `BWV:75-84`).
    pub(super) selection: Option<String>,
    /// When that selection may be copied (B10).
    pub(super) selection_gate: crate::browser_js::SelectionGate,
    /// The frame graph (`browser_frames_app`).
    pub(super) graph: crate::browser_frames::Frames,
    /// The page menu last shown: its token, the context and the items (`browser_ctx_app`).
    pub(super) menu: Option<(u64, crate::browser_ctx::Ctx, Vec<Option<crate::browser_ctx::Item>>)>,
    /// The announcements waiting for an answer, by bar identifier: the buttons and what answers (13x2 Task 5).
    pub(super) bars: HashMap<String, (Vec<String>, super::browser_overlay_app::BarDone)>,
    /// The find bar's search state on this page (13x3 Task 4), and the commands waiting to go out.
    pub(super) find: crate::browser_find::Model,
    pub(super) find_out: crate::browser_find::Outbox,
    /// The Find Globally search this page's global stream answers (`App.global_generation` at its start).
    pub(super) global_gen: u64,
}

/// A press on a toolbar part, waiting for its release (or, on Back and Forward, the long press).
pub(super) struct BarPress {
    hit: Hit,
    since: Instant,
    fired: bool,
}

/// What the toolbar keeps for one browser pane.
#[derive(Default)]
pub(super) struct BarState {
    /// The address field has the keyboard.
    pub(super) keys: bool,
    pub(super) edit: text_edit::TextEdit,
    pub(super) suggestions: Vec<Suggestion>,
    pub(super) selected: Option<usize>,
    pub(super) cache: browser::OpenSearchCache,
    /// The suggestion request in flight and the query it asks about.
    pub(super) call: Option<(Box<dyn crate::ai_transport::Call>, String)>,
    pub(super) press: Option<BarPress>,
    pub(super) nav: browser::NavList,
    /// How the next history row's visit began (`BrowserTransitionType`); None is "other".
    pub(super) transition: Option<i64>,
    /// How navigations began, for `transition` (`BrowserNavigationState`), and the page color's rate limit.
    pub(super) nav_state: crate::browser_restore::NavState,
    pub(super) color: crate::browser_restore::ColorLimit,
    /// The favicon decoded: a key for the texture, RGBA and its size.
    favicon: Option<(u64, std::sync::Arc<Vec<u8>>, (u32, u32))>,
}

/// The id prefixes of the toolbar's menus.
const MENU_ID: &str = "browser-menu:";
const HISTORY_ID: &str = "browser-hist:";
/// The cells a Back or Forward menu line gives a URL (a native menu widens to its longest line, so
/// this is the width the line is drawn at; the one shortener fits the URL to it).
const HISTORY_MENU_CELLS: usize = 80;

/// A color between two (`t` of the way to `b`).
fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let m = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round().clamp(0.0, 255.0) as u8;
    Rgb { r: m(a.r, b.r), g: m(a.g, b.g), b: m(a.b, b.b) }
}

fn text_w(t: &str, cw: f32) -> f32 {
    unicode_width::UnicodeWidthStr::width(t) as f32 * cw
}

/// One line of printable characters from pasted text (THE display sanitizer: controls, bidi and
/// zero width characters go).
fn one_line(t: &str) -> String {
    crate::inert_files::display_text(t, usize::MAX, false)
}

/// The favicon's pixels, or None (a page's file is untrusted: decoded by `image`, bounded).
fn decode_favicon(bytes: &[u8]) -> Option<(u64, std::sync::Arc<Vec<u8>>, (u32, u32))> {
    use std::hash::{Hash, Hasher};
    let img = image::load_from_memory(bytes).ok()?;
    let (w, h) = (img.width(), img.height());
    if w == 0 || h == 0 || w > 256 || h > 256 {
        return None;
    }
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hasher);
    Some((hasher.finish(), std::sync::Arc::new(img.to_rgba8().into_raw()), (w, h)))
}

/// How a browser view is made: the browser's agent (the engine's with "Tandem" appended), developer
/// tools on, every navigation guarded, new windows posted, InPrivate for Dev Null Mode. The only
/// bridge is the `iterm2-about:` pages' shim (it does nothing on any other page) and the filter
/// that lets Tandem serve those pages; a message counts only for an `iterm2-about:` source
/// (`browser_pages::route`). No host objects.
pub(super) fn browser_setup(dev_null: bool, adblock: bool, page_scripts: Vec<String>) -> ViewSetup {
    // With ad blocking on, every request of the page is offered to the filter ("*"); `iterm2-about:`
    // keeps its own answer first (`browser_local_request`). Off: only Tandem's own scheme.
    let mut filters = vec!["iterm2-about:*".to_string()];
    if adblock {
        filters.push("*".to_string());
    }
    ViewSetup {
        user_agent: Some(String::new()),
        devtools: true,
        new_windows: true,
        guard_local_schemes: true,
        in_private: dev_null,
        web_message: true,
        // The Instant Replay sensitive mark runs in every frame of every browser page (final review I1).
        scripts: page_scripts.into_iter().chain([crate::web_replay::sensitive_script()]).collect(),
        resource_filters: filters,
        favicon: true,
        context_menu: true,
        ..Default::default()
    }
}


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
        // A restored (or opened) URL is judged by the one navigation gate again at load time: a stored
        // file: or javascript: URL never loads, the pane starts at the profile's Initial URL.
        let restored = self.browser_url_override.take().and_then(|u| crate::browser_restore::load_gate(&u));
        let url = match browser::start_plan(&p.string("Custom Command"), self.globals.bool("BrowserProfiles"), &p.string("Initial URL"), restored.as_deref()) {
            Start::Browser(u) => u,
            Start::Terminal => return false,
        };
        if self.web_runtime() == Runtime::Missing {
            self.banners.set("webview2", browser::UPSELL_TEXT);
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
        crate::rlog!("browser pane {} started: profile {} dev null {}", id, crate::dlog::name(&name), dev_null);
        let view = self.next_view_id();
        self.browser_views.insert(id, BrowserView { view: None, id: view, pending_url: Some(url), parent: 0, restarts: webhost::Restarts::default(), stopped: false, settled: false, last_title: None, bar: BarState::default(), replay: super::web_replay_app::Recorder::new(), secret: crate::browser_js::new_secret(), dom_secret: crate::browser_js::new_secret(), pending_js: HashMap::new(), frames: Vec::new(), page_frames: Vec::new(), eval_waiting: Default::default(), client_world: crate::browser_world::ClientWorld::Setting, contexts: Default::default(), world_scripts: Vec::new(), next_token: super::browser_scripts_app::TOKEN_BASE, world: Default::default(), editing: false, selection: None, selection_gate: Default::default(), graph: Default::default(), menu: None, bars: HashMap::new(), find: crate::browser_find::fresh_model(), find_out: Default::default(), global_gen: 0, gate: Default::default(), gate_sig: None, copy_mode: None, reader: Default::default(), menu_pending: false, pointer_sig: None, pointer_down: None, receiving_broadcast: 0, receiving_since: None, echo: Default::default() });
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
            Some(tab) => self.visible(tab).into_iter().map(|(id, r)| (id, self.browser_web_rect(id, r))).collect(),
            None => Vec::new(),
        };
        let mine: Vec<PaneId> = self.tabs.iter().flat_map(|t| t.layout.panes()).collect();
        let focused = self.focused_pane_id();
        // Keyboard focus moves into a page only in the window that is in front (foreground root).
        let in_front = crate::webhost_win::foreground_root() == Some(hwnd);
        let panes: Vec<PaneId> = self.browser_views.keys().copied().collect();
        for pane in panes {
            let rect = shown.iter().find(|(p, _)| *p == pane).map(|(_, r)| *r);
            match rect {
                Some(rect) => self.show_browser_view(pane, rect, hwnd, focused == Some(pane) && in_front && self.browser_page_may_focus(pane)),
                None => {
                    // Its link tip goes with it (hidden, another tab, buried, zoomed away).
                    self.browser_tip_clear(pane);
                    // Not shown: hidden when it is this window's pane (another tab, buried, zoomed
                    // away) or sits in this window's handle with no pane (an Undo Close stash).
                    if let Some(bv) = self.browser_views.get(&pane) {
                        if mine.contains(&pane) || bv.parent == hwnd {
                            if let Some(v) = &bv.view {
                                v.set_visible(false);
                            }
                        }
                    }
                    self.web_replay_shown(pane, false);
                }
            }
        }
        // Keyboard focus follows the active pane when it changes within this window.
        let window = self.current_window();
        let step = window.and_then(|w| self.focus_seen.step(w, focused, in_front));
        if let Some(seen) = step {
            let prev = seen.filter(|p| mine.contains(p));
            self.browser_focus_moved(prev, focused);
            if let Some(next) = focused.filter(|p| self.is_browser_pane(*p) && self.browser_page_may_focus(*p)) {
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
        let missing = self.browser_views.get(&pane).is_some_and(|b| b.view.is_none() && !b.stopped);
        if missing {
            let url = self.browser_views[&pane].pending_url.clone().unwrap_or_else(|| io.url.clone());
            let sink = self.web_envs.sink();
            let adblock = self.adblock_on();
            if self.web_env(browser::store_of(io.dev_null)).is_none() {
                return;
            }
            // A fresh id per view made (final review Minor 5): an event of the view this one
            // replaces, still queued, names an id no pane owns and is dropped, never applied here.
            let id = self.next_view_id();
            // A view made now starts with a new world: new secrets, no contexts, no pending call.
            self.browser_reset_world(pane);
            let (page_scripts, _) = self.browser_wrapped_scripts(pane);
            let Some(env) = self.web_env(browser::store_of(io.dev_null)) else { return };
            let view = WebView::create(env, hwnd, id, webhost::physical_bounds(rect), browser_setup(io.dev_null, adblock, page_scripts), sink);
            view.set_zoom(if io.zoom > 0.0 { io.zoom / 100.0 } else { 1.0 });
            if focus_now {
                view.focus();
            }
            if let Some(bv) = self.browser_views.get(&pane) {
                view.set_key_gate(bv.gate.clone());
            }
            if let Some(bv) = self.browser_views.get_mut(&pane) {
                bv.view = Some(view);
                bv.gate_sig = None;
                bv.id = id;
                bv.pending_url = None;
                bv.parent = hwnd;
                bv.settled = false;
            }
            // The page loads only when the world's setup and checks are over (B4).
            self.browser_world_start(pane, Some(url));
        }
        self.browser_world_tick(pane);
        let Some(bv) = self.browser_views.get_mut(&pane) else { return };
        if let Some(v) = &bv.view {
            if bv.parent != hwnd {
                v.set_parent(hwnd);
                bv.parent = hwnd;
            }
            v.set_bounds(webhost::physical_bounds(rect));
            // A child window hides what the scene draws: while the suggestions list is open the
            // page steps aside (Decision: a list over the page cannot be drawn by the scene).
            v.set_visible(!(bv.bar.keys && !bv.bar.suggestions.is_empty()));
        }
        bv.replay.shown = true;
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

    /// The window whose handle is `closing` is going (or hiding): the closing hook of `HOSTED_VIEWS`
    /// (final review I4, a class fix). Every browser view that is a child of that handle, whatever
    /// tab holds its pane now, moves to the window that shows its pane, or closes while its parent
    /// still exists and is made again from the pane's URL when the pane is shown again (a closed
    /// window's Undo stash, a buried pane). A view left on a destroyed parent would be a blank pane
    /// that is never remade.
    pub(super) fn browser_window_closing(&mut self, closing: isize) {
        let views: Vec<(PaneId, isize)> = self.browser_views.iter().filter(|(_, b)| b.view.is_some()).map(|(p, b)| (*p, b.parent)).collect();
        let fates = webhost::closing_parent(&views, closing, |p| self.pane_window_id(p).and_then(|w| self.window_hwnd(w)));
        for (pane, fate) in fates {
            match fate {
                ParentFate::Move(h) => self.browser_view_to(pane, h),
                ParentFate::Close => self.browser_view_drop(pane),
            }
        }
    }

    /// Panes moving to window `to` (a tab dropped on another window's bar, a group moved there, a
    /// pane split into another window's tab): their views follow now, before the window they leave
    /// can close; hidden until that window places them.
    pub(super) fn browser_views_follow(&mut self, panes: &[PaneId], to: WindowId) {
        let Some(h) = self.window_hwnd(to) else { return };
        // A moved view that holds the keyboard takes the hold to its new window (N3).
        let held = self.web_focus_held.and_then(|w| self.window_hwnd(w));
        for pane in panes {
            let keyboard = self.browser_views.get(pane).is_some_and(|bv| bv.view.is_some() && Some(bv.id) == self.web_focus_view && held == Some(bv.parent));
            self.browser_view_to(*pane, h);
            if keyboard {
                self.web_focus_held = Some(to);
                self.sync_web_keyboard();
            }
        }
    }

    /// Pane `pane`'s view becomes a child of `hwnd` (hidden until placed there).
    fn browser_view_to(&mut self, pane: PaneId, hwnd: isize) {
        let Some(bv) = self.browser_views.get_mut(&pane) else { return };
        let Some(v) = &bv.view else { return };
        if bv.parent != hwnd {
            v.set_visible(false);
            v.set_parent(hwnd);
            bv.parent = hwnd;
            bv.replay.shown = false;
        }
    }

    /// Pane `pane`'s view closes and is made again from the pane's URL when the pane is shown.
    fn browser_view_drop(&mut self, pane: PaneId) {
        let url = self.panes.get(&pane).and_then(|p| p.browser_io()).map(|io| io.url.clone());
        let Some(bv) = self.browser_views.get_mut(&pane) else { return };
        let (had, id, parent) = (bv.view.take().is_some(), bv.id, bv.parent);
        self.browser_reset_world(pane);
        let Some(bv) = self.browser_views.get_mut(&pane) else { return };
        bv.replay.shown = false;
        bv.pending_url = url;
        bv.parent = 0;
        if had {
            self.web_view_dropped(id, parent);
        }
    }

    /// A pane was closed for good: its view closes.
    pub(super) fn close_browser_view(&mut self, pane: PaneId) {
        self.browser_bars_end(pane);
        self.browser_reset_world(pane);
        if let Some(bv) = self.browser_views.remove(&pane) {
            let (had, id, parent) = (bv.view.is_some(), bv.id, bv.parent);
            drop(bv);
            if had {
                self.web_view_dropped(id, parent);
            }
        }
        self.focus_seen.forget(pane);
    }

    /// The pane that owns view `view`.
    fn browser_pane_of(&self, view: ViewId) -> Option<PaneId> {
        if view == 0 {
            return None;
        }
        self.browser_views.iter().find(|(_, b)| b.id == view).map(|(p, _)| *p)
    }

    /// Runs `f` with the window of `pane` current, then goes back.
    pub(super) fn in_pane_window(&mut self, pane: PaneId, f: impl FnOnce(&mut App)) -> bool {
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
    pub(super) fn browser_event(&mut self, pane: PaneId, e: &Event, event_loop: &ActiveEventLoop) {
        // The page lost the keyboard: its link tip goes (and the page is told).
        if matches!(e, Event::Focus(_, false)) {
            self.browser_tip_clear(pane);
        }
        match e {
            // The world's own about:blank navigation (the isolation check) is not the page.
            Event::Source(..) | Event::Title(..) | Event::Favicon(..) | Event::History { .. } if self.browser_views.get(&pane).is_some_and(|b| b.world.setup.is_some()) => {}
            Event::Source(_, url) => {
                if let Some(io) = self.panes.get_mut(&pane).and_then(|p| p.browser_io_mut()) {
                    io.url = url.clone();
                }
                if let Some(bv) = self.browser_views.get_mut(&pane) {
                    bv.bar.nav.source(url);
                }
                self.redraw();
            }
            Event::Title(_, title) => {
                if let Some(io) = self.panes.get_mut(&pane).and_then(|p| p.browser_io_mut()) {
                    io.title = browser::shown_title(title);
                }
                if let Some(bv) = self.browser_views.get_mut(&pane) {
                    bv.bar.nav.title(&browser::display_title(title));
                    bv.last_title = Some(title.clone()).filter(|t| !t.is_empty());
                }
                self.browser_record_title(pane, title);
                self.browser_name(pane);
            }
            Event::Favicon(_, bytes) => {
                if let Some(bv) = self.browser_views.get_mut(&pane) {
                    bv.bar.favicon = decode_favicon(bytes);
                }
                self.redraw();
                if let Some(io) = self.panes.get_mut(&pane).and_then(|p| p.browser_io_mut()) {
                    io.favicon = Some(bytes.clone());
                }
            }
            Event::Loading(_, on) => {
                if let Some(io) = self.panes.get_mut(&pane).and_then(|p| p.browser_io_mut()) {
                    io.loading = *on;
                    if *on {
                        // A new load starts over: the globe until a favicon loads, no failure yet.
                        io.failed = false;
                        io.favicon = None;
                    }
                }
                if *on {
                    if let Some(bv) = self.browser_views.get_mut(&pane) {
                        bv.settled = false;
                        bv.last_title = None;
                        bv.bar.favicon = None;
                    }
                }
                self.redraw();
            }
            // The navigation of the world's isolation check: it answers the check and is nothing else.
            Event::Navigated { ok, nav, .. } if self.browser_world_navigated(pane, *ok, *nav) => {}
            Event::Navigated { ok, .. } => {
                // `BM:1390`: a failed navigation turns copy mode off.
                if !*ok {
                    self.browser_copy_mode_set(pane, false);
                }
                if let Some(io) = self.panes.get_mut(&pane).and_then(|p| p.browser_io_mut()) {
                    io.failed = !*ok;
                }
                self.redraw();
                if let Some(bv) = self.browser_views.get_mut(&pane) {
                    bv.settled = true;
                }
                // A page that really loaded is one history row (an error page is not), and a good
                // start for the remake budget.
                if *ok {
                    if let Some(bv) = self.browser_views.get_mut(&pane) {
                        bv.restarts.succeeded();
                    }
                    // The check's about:blank is not a page to go back to (review I1).
                    if self.browser_views.get_mut(&pane).is_some_and(|b| b.world.history.loaded()) {
                        self.browser_devtools(pane, crate::browser_world::RESET_HISTORY, serde_json::json!({}), Box::new(|_, _, _| {}));
                    }
                    self.browser_record_visit(pane);
                    self.adblock_inject(pane);
                    self.browser_color_read(pane);
                }
                self.browser_name(pane);
            }
            Event::History { back, forward, .. } => {
                if let Some(io) = self.panes.get_mut(&pane).and_then(|p| p.browser_io_mut()) {
                    io.can_back = *back;
                    io.can_forward = *forward;
                }
            }
            // The page let go of the keyboard: another window may now be in front of a window that
            // held it (final review Minor 6: as for every other kind of view).
            Event::Focus(_, false) => self.web_focus_recheck(),
            // The page has the keyboard: its pane is the active one (iTerm2's session change).
            Event::Focus(_, true) => {
                // The keyboard is in the page now: the address field and the find bar let go.
                self.browser_page_took_keyboard(pane);
                self.in_pane_window(pane, |a| {
                    if let Some(t) = a.tabs.iter_mut().find(|t| t.layout.panes().contains(&pane)) {
                        t.focused = pane;
                    }
                    a.redraw();
                });
            }
            Event::NewWindow { url, .. } => {
                // Only a person's click (the host filters) and only a page's own kind of URL (P20).
                if let Some(url) = match crate::browser_js::nav_gate_tab(url) {
                    crate::browser_js::NavGate::Navigate(u) => browser::saved_url(&u),
                    _ => None,
                } {
                    let guid = self.panes.get(&pane).map(|p| p.profile.clone());
                    self.browser_url_override = Some(url);
                    let opened = self.in_pane_window(pane, |a| a.new_tab(guid.as_deref()));
                    if !opened {
                        self.browser_url_override = None;
                    }
                }
            }
            Event::ExternalUri { url, user, .. } | Event::Refused { url, user, .. } => self.run_external(url, *user),
            // A key the page did not take as its own: Tandem's Win chords and the address
            // field's accelerators (Review: the page keeps every other key).
            Event::Key { vk, mods, taken, .. } => self.browser_accel_key(pane, *vk, *mods, *taken, event_loop),
            // A good start resets the remake budget (final review I5).
            Event::Created(_) => {
                if let Some(bv) = self.browser_views.get_mut(&pane) {
                    bv.world.controller_ready(Instant::now());
                    bv.restarts.succeeded();
                }
            }
            // The view could not be made: remade from the URL within the budget.
            Event::Failed(view, _) => {
                // The reason is COM's text: only that it happened and which view (13w B8).
                crate::rlog!("webhost: a browser pane's view {} failed", *view);
                self.browser_view_failed(pane, webhost::PROCESS_RENDER_EXITED);
            }
            // A process failed: the kind says what to do (final review I5).
            Event::ProcessFailed { view, kind } => {
                crate::rlog!("webhost: a browser pane's view {} process failed, kind {}", *view, *kind);
                self.browser_view_failed(pane, *kind);
            }
            // A local page's message and the request for a local page (Task 7): the one route judges
            // the source and the closed set; everything else a page posts is dropped there.
            Event::ScriptDone { token, json, .. } if matches!(*token, crate::web_replay::PROBE_TOKEN | crate::web_replay::CAPTURE_TOKEN) => self.web_replay_script_done(pane, *token, json),
            Event::Request { token, url, context, top_url, .. } => self.browser_local_request(pane, *token, url, *context, top_url),
            // The page's right-click menu and a pick from it (Task 3).
            Event::ContextMenu { token, target, .. } => self.browser_context_menu(pane, *token, target.clone()),
            Event::ContextMenuPicked { token, id, .. } => self.browser_ctx_event(pane, *token, *id, event_loop),
            // A navigation began: how, for the history row it makes (`BNS:60-82`); the scripts see it too.
            Event::Starting { url, kind, user, redirect, .. } => {
                self.browser_nav_starting(pane, url, *kind, *user, *redirect);
                self.browser_script_event(pane, e);
            }
            // Everything a page's script says, the world's events and the calls' answers: the router.
            _ => {
                self.browser_script_event(pane, e);
            }
        }
    }

    /// Pane `pane`'s view reported a failure of `kind` (`webhost::pane_step`): a browser process exit
    /// drops its environment (made again on the next request); the view reloads, is made again from
    /// the pane's URL, or, over the budget, is given up until the person presses Reload or navigates.
    fn browser_view_failed(&mut self, pane: PaneId, kind: i32) {
        let now = Instant::now();
        let dev_null = self.panes.get(&pane).and_then(|p| p.browser_io()).is_some_and(|io| io.dev_null);
        self.web_view_lost(browser::store_of(dev_null), kind);
        let url = self.panes.get(&pane).and_then(|p| p.browser_io()).map(|io| io.url.clone());
        let Some(bv) = self.browser_views.get_mut(&pane) else { return };
        match webhost::pane_step(kind, &mut bv.restarts, now) {
            webhost::PaneStep::Nothing => return,
            webhost::PaneStep::Reload => {
                if let Some(v) = &bv.view {
                    v.reload();
                }
                // Its world is checked in a few seconds (whether the DevTools state survives is not known).
                self.browser_world_expect(pane);
                return;
            }
            webhost::PaneStep::Remake => {}
            webhost::PaneStep::GiveUp => {
                bv.stopped = true;
                self.banners.set("window", "A browser pane's web view stopped working. Press Reload to try again.");
            }
        }
        let Some(bv) = self.browser_views.get_mut(&pane) else { return };
        let (had, id, parent) = (bv.view.take().is_some(), bv.id, bv.parent);
        self.browser_reset_world(pane);
        let Some(bv) = self.browser_views.get_mut(&pane) else { return };
        bv.pending_url = url;
        bv.parent = 0;
        bv.replay.shown = false;
        if had {
            self.web_view_dropped(id, parent);
        }
        self.redraw();
    }

    /// A browser pane's tab icon when its profile's icon is Automatic (`PS:6665-6666`): the page's
    /// favicon, or none until one loads.
    pub(super) fn browser_tab_icon(&self, pane: PaneId) -> Option<scene::TabImage> {
        let (key, pixels, size) = self.browser_views.get(&pane)?.bar.favicon.clone()?;
        Some(scene::TabImage::Picture { key, pixels, size })
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

/// The bar and suggestions of a browser pane: drawing, presses, the field's keys, the menus.
impl App {
    /// The toolbar's rectangle and the pane's, for a browser pane of the active tab.
    /// Where a browser pane's flash and bell indicator draw (13w seam S10): its toolbar strip, the
    /// part of the pane the scene still shows over the web view.
    pub(super) fn browser_flash_rect(&self, pane: PaneId) -> Option<Rect> {
        self.browser_bar_rects(pane).map(|(bar, _)| bar)
    }

    fn browser_bar_rects(&self, pane: PaneId) -> Option<(Rect, Rect)> {
        let tab = self.tabs.get(self.active)?;
        let (_, rect) = self.visible(tab).into_iter().find(|(id, _)| *id == pane)?;
        Some((self.browser_strip(tab, pane)?, rect))
    }

    /// The rows of the suggestions list of `pane`, as rectangles (empty when it is not open).
    fn browser_suggest_rects(&self, pane: PaneId, bar: Rect, area: Rect) -> Vec<Rect> {
        let Some(bv) = self.browser_views.get(&pane) else { return Vec::new() };
        if !bv.bar.keys || bv.bar.suggestions.is_empty() {
            return Vec::new();
        }
        let dev_null = self.panes.get(&pane).and_then(|p| p.browser_io()).is_some_and(|io| io.dev_null);
        let g = self.browser_geom(bar, dev_null);
        let row_h = self.cell().1 + 6.0 * self.scale();
        browser::suggestion_rects(&g, row_h, bv.bar.suggestions.len(), area.y + area.h)
    }

    /// The browser pane whose toolbar (or open suggestions list) is under a point.
    pub(super) fn browser_bar_at(&self, x: f32, y: f32) -> Option<PaneId> {
        let tab = self.tabs.get(self.active)?;
        self.visible(tab).into_iter().filter(|(id, _)| self.is_browser_pane(*id)).find_map(|(id, rect)| {
            let bar = self.browser_strip(tab, id)?;
            (bar.contains(x, y) || self.browser_suggest_rects(id, bar, rect).iter().any(|r| r.contains(x, y))).then_some(id)
        })
    }

    /// Draws a browser pane's toolbar across `bar` and, while the field has the keyboard, the
    /// suggestions list under it. Glyphs are Segoe Fluent Icons; the text rows are the cell font.
    pub(super) fn draw_browser_bar(&self, frame: &mut scene::Frame, pane: PaneId, bar: Rect, area: Rect, cell: (f32, f32)) {
        use scene::{Quad, Span, TextRow};
        let (Some(p), Some(bv)) = (self.panes.get(&pane), self.browser_views.get(&pane)) else { return };
        let Some(io) = p.browser_io() else { return };
        let scale = self.scale();
        let (cw, ch) = cell;
        let (bg, fg) = (p.screen.palette.bg, p.screen.palette.fg);
        let (bar_bg, field_bg, dim, line) = (mix(bg, fg, 0.08), mix(bg, fg, 0.16), mix(bg, fg, 0.45), mix(bg, fg, 0.3));
        let span = |text: String, color: Rgb, icon: bool| Span { text, color, bold: false, italic: false, alt_font: false, icon, prose: false, badge: false, toolbelt: false, fake_bold: false, ligatures: 0, mask: 0, font: 0 };
        let row = |x: f32, y: f32, spans: Vec<Span>, clip: Rect| TextRow { x, y, spans, clip, scale: 1.0, face: crate::render::DEFAULT_FACE };
        let g = self.browser_geom(bar, io.dev_null);
        frame.quads.push(Quad { rect: bar, color: scene::rgba(bar_bg, 1.0) });
        frame.quads.push(Quad { rect: Rect { y: bar.y + bar.h - 1.0, h: 1.0, ..bar }, color: scene::rgba(line, 1.0) });
        let centered = |r: Rect| (r.x + (r.w - cw) / 2.0, r.y + (r.h - ch) / 2.0);
        for (b, enabled) in browser::buttons(io) {
            let r = match b {
                Button::Back => g.back,
                Button::Forward => g.forward,
                Button::Reload | Button::Stop => g.reload,
                Button::Menu => g.menu,
            };
            if r.w < cw {
                continue;
            }
            let (x, y) = centered(r);
            frame.rows.push(row(x, y, vec![span(browser::button_glyph(b).to_string(), if enabled { fg } else { dim }, true)], r));
        }
        if let Some(r) = g.dev_null {
            let (x, y) = centered(r);
            frame.rows.push(row(x, y, vec![span(browser::GLYPH_DEV_NULL.to_string(), fg, true)], r));
        }
        if let Some(r) = g.proxy {
            let (x, y) = centered(r);
            frame.rows.push(row(x, y, vec![span(browser::GLYPH_PROXY.to_string(), fg, true)], r));
        }
        frame.quads.push(Quad { rect: g.field, color: scene::rgba(field_bg, 1.0) });
        // The favicon, a globe until one loads.
        if let Some((key, pixels, size)) = &bv.bar.favicon {
            let side = (16.0 * scale).min(g.indicator.w).min(g.indicator.h);
            let rect = Rect { x: g.indicator.x + (g.indicator.w - side) / 2.0, y: g.indicator.y + (g.indicator.h - side) / 2.0, w: side, h: side };
            frame.images.push(scene::ImageQuad { rect, uv: [0.0, 0.0, 1.0, 1.0], key: (*key, 0), pixels: pixels.clone(), size: *size, repeat: false });
        } else if g.indicator.w >= cw {
            let (x, y) = centered(g.indicator);
            frame.rows.push(row(x, y, vec![span(browser::GLYPH_GLOBE.to_string(), dim, true)], g.indicator));
        }
        // The mark follows the page that is really loaded, not the text being typed.
        let sec = browser::security(&io.url, io.failed);
        let mark = match sec {
            browser::Security::Secure => Some((browser::GLYPH_LOCK, fg)),
            browser::Security::NotSecure | browser::Security::Failed => Some((browser::GLYPH_WARNING, scene::FIND_ERROR)),
            browser::Security::Local | browser::Security::File => None,
        };
        if let Some((glyph, color)) = mark.filter(|_| g.mark.w >= cw) {
            let (x, y) = centered(g.mark);
            frame.rows.push(row(x, y, vec![span(glyph.to_string(), color, true)], g.mark));
        }
        // The address: the field's text while it has the keyboard, else the page's sanitized URL.
        let pad = 2.0 * scale;
        let text_y = g.text.y + (g.text.h - ch) / 2.0;
        let keys = bv.bar.keys;
        let shown = if keys { bv.bar.edit.text.clone() } else { browser::fit_address(&browser::display_url(&io.url), ((g.text.w - 2.0 * pad) / cw).floor().max(0.0) as usize) };
        let caret_x = if keys { text_w(&shown[..bv.bar.edit.caret.min(shown.len())], cw) } else { 0.0 };
        let scroll = (caret_x + cw - (g.text.w - 2.0 * pad)).max(0.0);
        let x0 = g.text.x + pad - scroll;
        if keys {
            if let Some(sel) = bv.bar.edit.selection() {
                let (a, b) = (text_w(&shown[..sel.start], cw), text_w(&shown[..sel.end], cw));
                let r = Rect { x: x0 + a, y: text_y, w: b - a, h: ch };
                frame.quads.push(Quad { rect: r, color: scene::rgba(p.screen.palette.selection, 1.0) });
            }
            frame.quads.push(Quad { rect: Rect { x: x0 + caret_x, y: text_y, w: (1.0 * scale).max(1.0), h: ch }, color: scene::rgba(fg, 1.0) });
        }
        frame.rows.push(row(x0, text_y, vec![span(shown, fg, false)], g.text));
        // The suggestions: a box with each row's text and its detail.
        let rects = self.browser_suggest_rects(pane, bar, area);
        if let (Some(first), Some(last)) = (rects.first(), rects.last()) {
            let all = Rect { x: first.x, y: first.y, w: first.w, h: last.y + last.h - first.y };
            scene::cut_out(frame, all, ch);
            frame.popups.push(Quad { rect: all, color: scene::rgba(bar_bg, 1.0) });
            for (i, r) in rects.iter().enumerate() {
                let Some(s) = bv.bar.suggestions.get(i) else { break };
                if bv.bar.selected == Some(i) {
                    frame.popups.push(Quad { rect: *r, color: scene::rgba(p.screen.palette.selection, 1.0) });
                }
                let y = r.y + (r.h - ch) / 2.0;
                let main = Rect { w: r.w * 0.62, ..*r };
                let detail = Rect { x: r.x + r.w * 0.64, w: r.w * 0.35, ..*r };
                // A URL in a row keeps its host whatever the row's width (the one shortener, given the
        // cells the row draws in, so the row's clip never cuts what it returns).
                let text = browser::fit_address(&s.text, ((main.w - 6.0 * scale) / cw).floor().max(0.0) as usize);
                let detail_text = browser::fit_address(&s.detail, (detail.w / cw).floor().max(0.0) as usize);
                frame.rows.push(row(r.x + 6.0 * scale, y, vec![span(text, fg, false)], main));
                frame.rows.push(row(detail.x, y, vec![span(detail_text, dim, false)], detail));
            }
        }
    }

    /// A left press on a browser pane's toolbar or its suggestions. Always taken (nothing under it
    /// is the program's).
    pub(super) fn browser_toolbar_press(&mut self, pane: PaneId, x: f32, y: f32, _event_loop: &ActiveEventLoop) -> bool {
        let Some((bar, area)) = self.browser_bar_rects(pane) else { return false };
        if let Some(t) = self.tabs.get_mut(self.active) {
            t.focused = pane;
        }
        // A row of the list picks.
        if let Some(i) = self.browser_suggest_rects(pane, bar, area).iter().position(|r| r.contains(x, y)) {
            if let Some(url) = self.browser_views.get(&pane).and_then(|b| b.bar.suggestions.get(i)).map(|s| s.url.clone()) {
                self.browser_url_submit(pane, &url);
            }
            return true;
        }
        let dev_null = self.panes.get(&pane).and_then(|p| p.browser_io()).is_some_and(|io| io.dev_null);
        let g = self.browser_geom(bar, dev_null);
        match browser::hit(&g, x, y) {
            Some(Hit::Field) => {
                let keys = self.browser_views.get(&pane).is_some_and(|b| b.bar.keys);
                if keys {
                    self.browser_field_click(pane, x - g.text.x - 2.0 * self.scale());
                } else {
                    self.focus_url_bar(pane);
                }
            }
            Some(h) => {
                self.blur_url_bar(pane, false);
                if let Some(bv) = self.browser_views.get_mut(&pane) {
                    bv.bar.press = Some(BarPress { hit: h, since: Instant::now(), fired: false });
                }
            }
            None => {}
        }
        self.redraw();
        true
    }

    /// The caret moves to the click (`x` from the text's left edge); a double click selects a word.
    fn browser_field_click(&mut self, pane: PaneId, x: f32) {
        let cw = self.cell().0;
        let shift = self.mods.shift;
        let measure = move |t: &str| text_w(t, cw);
        let Some(bv) = self.browser_views.get_mut(&pane) else { return };
        let lines = text_edit::wrap(&bv.bar.edit.text, 1.0e9, &measure);
        let scroll = (text_w(&bv.bar.edit.text[..bv.bar.edit.caret.min(bv.bar.edit.text.len())], cw)).max(0.0);
        let _ = scroll;
        let at = text_edit::index_at(&bv.bar.edit.text, &lines, 0, x.max(0.0), &measure);
        bv.bar.edit.click(at, false, shift, super::toolbelt_tools_ui::now_ms(), super::clippings_ui::double_click_ms());
    }

    /// The release of a toolbar press: the button acts when the pointer is still over it.
    pub(super) fn browser_toolbar_release(&mut self, button: MouseButton, _event_loop: &ActiveEventLoop) -> bool {
        if button != MouseButton::Left {
            return false;
        }
        let Some(pane) = self.browser_views.iter().find(|(_, b)| b.bar.press.is_some()).map(|(p, _)| *p) else { return false };
        let Some(press) = self.browser_views.get_mut(&pane).and_then(|b| b.bar.press.take()) else { return false };
        if press.fired {
            return true;
        }
        let (x, y) = self.cursor;
        let Some((bar, _)) = self.browser_bar_rects(pane) else { return true };
        let io = self.panes.get(&pane).and_then(|p| p.browser_io()).cloned().unwrap_or_default();
        let g = self.browser_geom(bar, io.dev_null);
        let still = browser::hit(&g, x, y).is_some_and(|h| h == press.hit || (matches!(h, Hit::Button(Button::Reload)) && matches!(press.hit, Hit::Button(Button::Reload | Button::Stop))));
        if !still {
            return true;
        }
        match press.hit {
            Hit::Button(b) => {
                // Reload and Stop share a place: the one the page's state shows now.
                let b = if matches!(b, Button::Reload | Button::Stop) { if io.loading { Button::Stop } else { Button::Reload } } else { b };
                self.browser_button(pane, b, (g.menu.x, bar.y + bar.h));
            }
            Hit::Proxy => {
                let label = self.browser_proxy_label().unwrap_or_default();
                self.banners.set("action", format!("Browser proxy is on: {label}"));
            }
            Hit::DevNull => self.banners.set("action", "Dev Null Mode: your browsing activity is not being saved. No history, bookmarks or other data are stored."),
            Hit::Mark => {
                let text = match browser::security(&io.url, io.failed) {
                    browser::Security::Secure => "This page was loaded over a secure connection (https).",
                    browser::Security::NotSecure => "This page was not loaded over a secure connection.",
                    browser::Security::Failed => "This page did not load.",
                    browser::Security::File => "This page is a file on this computer.",
                    browser::Security::Local => "This is one of Tandem's own pages.",
                };
                self.banners.set("action", text);
            }
            Hit::Indicator | Hit::Field => {}
        }
        self.redraw();
        true
    }

    /// What a toolbar button does.
    fn browser_button(&mut self, pane: PaneId, b: Button, menu_at: (f32, f32)) {
        // The world is being set up: there is no page to go back from or reload (review I2).
        if matches!(b, Button::Back | Button::Forward | Button::Reload | Button::Stop) && self.browser_views.get(&pane).is_some_and(|bv| bv.world.setup.is_some()) {
            return;
        }
        match b {
            Button::Back => {
                if let Some(bv) = self.browser_views.get_mut(&pane) {
                    bv.bar.nav.expect(-1);
                    bv.bar.transition = Some(browser::TRANSITION_BACK_FORWARD);
                    if let Some(v) = &bv.view {
                        v.go_back();
                    }
                }
            }
            Button::Forward => {
                if let Some(bv) = self.browser_views.get_mut(&pane) {
                    bv.bar.nav.expect(1);
                    bv.bar.transition = Some(browser::TRANSITION_BACK_FORWARD);
                    if let Some(v) = &bv.view {
                        v.go_forward();
                    }
                }
            }
            Button::Reload => {
                if let Some(bv) = self.browser_views.get_mut(&pane) {
                    bv.bar.transition = Some(browser::TRANSITION_RELOAD);
                    // A view given up after failures is made again (final review I5).
                    if bv.stopped {
                        bv.stopped = false;
                        bv.restarts = webhost::Restarts::default();
                        self.redraw();
                    }
                }
                if let Some(v) = self.browser_views.get(&pane).and_then(|b| b.view.as_ref()) {
                    v.reload();
                }
            }
            Button::Stop => {
                if let Some(v) = self.browser_views.get(&pane).and_then(|b| b.view.as_ref()) {
                    v.stop();
                }
            }
            Button::Menu => self.browser_menu_press(pane, menu_at),
        }
    }

    /// The Menu button's menu (`BT:306-420`); Add and Remove Bookmark, Manage Bookmarks and History
    /// act with Task 7's database and pages, Settings with 13x8.
    pub(super) fn show_browser_menu(&mut self, pane: PaneId, at: (f32, f32)) {
        use muda::ContextMenu;
        let Some(hwnd) = self.hwnd() else { return };
        let dev_null = self.panes.get(&pane).and_then(|p| p.browser_io()).is_some_and(|io| io.dev_null);
        // Add or Remove Bookmark: the page's own state, enabled for a web page.
        let (marked, can_mark) = self.browser_bookmark_state(pane);
        let items = browser::menu_items(marked, can_mark, dev_null, self.browser_reader_menu(pane));
        let menu = muda::Menu::new();
        let mut failed = None;
        for item in items {
            let r = match item {
                None => menu.append(&muda::PredefinedMenuItem::separator()),
                Some((label, enabled)) => menu.append(&muda::MenuItem::with_id(format!("{MENU_ID}{pane}:{label}"), label, enabled, None)),
            };
            if let Err(e) = r {
                failed = Some(e.to_string());
            }
        }
        if let Some(e) = failed {
            self.banners.set("menu", format!("Browser menu: {e}"));
            return;
        }
        let at = muda::dpi::Position::Physical(muda::dpi::PhysicalPosition::new(at.0 as i32, at.1 as i32));
        unsafe { menu.show_context_menu_for_hwnd(hwnd, Some(at)) };
        self.popup_menu = Some(menu);
    }

    /// The history menu of Back or Forward: the pane's own list (WebView2 has none).
    fn show_browser_history(&mut self, pane: PaneId, back: bool, at: (f32, f32)) {
        use muda::ContextMenu;
        let Some(hwnd) = self.hwnd() else { return };
        let Some(bv) = self.browser_views.get(&pane) else { return };
        let items = if back { bv.bar.nav.back_items() } else { bv.bar.nav.forward_items() };
        if items.is_empty() {
            return;
        }
        let pairs: Vec<(String, String)> = items.iter().map(|(t, u, _)| (t.clone(), u.clone())).collect();
        let labels = browser::history_menu(&pairs, HISTORY_MENU_CELLS);
        let menu = muda::Menu::new();
        for ((_, _, steps), label) in items.iter().zip(labels) {
            let _ = menu.append(&muda::MenuItem::with_id(format!("{HISTORY_ID}{pane}:{steps}"), label.replace('&', "&&"), true, None));
        }
        let at = muda::dpi::Position::Physical(muda::dpi::PhysicalPosition::new(at.0 as i32, at.1 as i32));
        unsafe { menu.show_context_menu_for_hwnd(hwnd, Some(at)) };
        self.popup_menu = Some(menu);
    }

    /// A pick from the toolbar's menus. True when the id was theirs.
    pub(super) fn browser_menu_picked(&mut self, id: &str) -> bool {
        if let Some(rest) = id.strip_prefix(HISTORY_ID) {
            let Some((pane, steps)) = rest.split_once(':').and_then(|(p, s)| Some((p.parse::<PaneId>().ok()?, s.parse::<i32>().ok()?))) else { return true };
            if let Some(bv) = self.browser_views.get_mut(&pane) {
                if steps != 0 && steps.abs() <= 100 {
                    bv.bar.nav.expect(steps);
                    bv.bar.transition = Some(browser::TRANSITION_BACK_FORWARD);
                    if let Some(v) = &bv.view {
                        v.execute_script(&format!("history.go({steps})"), 0);
                    }
                }
            }
            return true;
        }
        let Some(rest) = id.strip_prefix(MENU_ID) else { return false };
        let Some((pane, label)) = rest.split_once(':').and_then(|(p, l)| Some((p.parse::<PaneId>().ok()?, l))) else { return true };
        if matches!(label, "Add Bookmark" | "Remove Bookmark") {
            self.browser_toggle_bookmark(pane);
            return true;
        }
        match label {
            "Reader Mode" | "Exit Reader Mode" => return { self.toggle_reader_mode(pane); true },
            "Remove Distractions" | "Exit Distraction Removal" => return { self.toggle_distraction_removal(pane); true },
            _ => {}
        }
        let target = match label {
            "Manage Bookmarks" => Some("iterm2-about:bookmarks"),
            "History" => Some("iterm2-about:history"),
            _ => None,
        };
        if let Some(url) = target {
            self.browser_navigate(pane, url);
        }
        true
    }

    /// The long press of Back and Forward (`BT:520-560`) and the suggestion request in flight.
    /// The next time to look.
    pub(super) fn browser_tick(&mut self) -> Option<Instant> {
        let mut due: Option<Instant> = None;
        let now = Instant::now();
        let panes: Vec<PaneId> = self.browser_views.keys().copied().collect();
        for pane in panes {
            let press = self.browser_views.get(&pane).and_then(|b| b.bar.press.as_ref()).map(|p| (p.hit, p.since, p.fired));
            if let Some((Hit::Button(b @ (Button::Back | Button::Forward)), since, false)) = press {
                if now >= since + browser::LONG_PRESS {
                    if let Some(p) = self.browser_views.get_mut(&pane).and_then(|b| b.bar.press.as_mut()) {
                        p.fired = true;
                    }
                    if let Some((bar, _)) = self.browser_bar_rects(pane) {
                        let dev_null = self.panes.get(&pane).and_then(|p| p.browser_io()).is_some_and(|io| io.dev_null);
                        let g = self.browser_geom(bar, dev_null);
                        let r = if b == Button::Back { g.back } else { g.forward };
                        self.show_browser_history(pane, b == Button::Back, (r.x, bar.y + bar.h));
                    }
                } else {
                    let at = since + browser::LONG_PRESS;
                    due = Some(due.map_or(at, |d| d.min(at)));
                }
            }
            self.browser_poll_suggest(pane);
            if let Some(at) = self.browser_color_tick(pane) {
                due = Some(due.map_or(at, |d| d.min(at)));
            }
        }
        let ad_due = self.adblock_tick();
        let replay_due = self.web_replay_tick();
        [due, ad_due, replay_due].into_iter().flatten().min()
    }

    /// The suggestion service's answer, when it has come: cached by query and shown if it is
    /// still what the field asks about.
    fn browser_poll_suggest(&mut self, pane: PaneId) {
        let template = browser::search_template(&self.globals.string("SearchCommand"));
        let Some(bv) = self.browser_views.get_mut(&pane) else { return };
        let Some((call, query)) = bv.bar.call.as_mut() else { return };
        let Some(event) = call.try_event() else { return };
        let query = query.clone();
        match event {
            crate::ai_transport::HttpEvent::Chunk(_) => return,
            crate::ai_transport::HttpEvent::Done { body, error } => {
                bv.bar.call = None;
                if error.is_some() {
                    return;
                }
                let rows = browser::open_search_results(&body, &query, &template, 5);
                bv.bar.cache.put(&query, rows);
            }
        }
        if bv.bar.keys && bv.bar.edit.text.trim() == query {
            let text = bv.bar.edit.text.clone();
            self.browser_suggest(pane, &text);
        }
        self.redraw();
    }

    /// The address field takes the keyboard and selects its whole text (`UH`, "selects all on focus").
    pub(super) fn focus_url_bar(&mut self, pane: PaneId) {
        let Some(url) = self.panes.get(&pane).and_then(|p| p.browser_io()).map(|io| browser::display_url(&io.url)) else { return };
        if let Some(t) = self.tabs.get_mut(self.active) {
            t.focused = pane;
        }
        self.browser_find_yield_to_address(pane);
        if let Some(bv) = self.browser_views.get_mut(&pane) {
            bv.bar.edit.load(&url);
            bv.bar.edit.anchor = Some(0);
            bv.bar.suggestions.clear();
            bv.bar.selected = None;
        }
        // The keyboard comes back from the page to the window.
        if let Some(hwnd) = self.hwnd() {
            crate::webhost_win::focus_window(hwnd);
        }
        self.redraw();
    }

    /// The field lets go; the page takes the keyboard back when `to_page`.
    fn blur_url_bar(&mut self, pane: PaneId, to_page: bool) {
        let Some(bv) = self.browser_views.get_mut(&pane) else { return };
        bv.bar.keys = false;
        bv.bar.suggestions.clear();
        bv.bar.selected = None;
        bv.bar.call = None;
        if to_page {
            if let Some(v) = &bv.view {
                v.focus();
            }
        }
        self.redraw();
    }

    /// Runs an external protocol a person asked for, through `guard_open` only.
    pub(super) fn run_external(&mut self, url: &str, user: bool) {
        match super::web_popover_app::external_uri_run(url, user) {
            super::web_popover_app::ExternalRun::Open(t) => {
                if let Err(e) = super::semantic_ui::shell_open(&t, false) {
                    self.banners.set("action", format!("Could not open {t}: {e}"));
                }
            }
            super::web_popover_app::ExternalRun::Notice(n) => self.banners.set("action", n),
            super::web_popover_app::ExternalRun::Nothing => {}
        }
    }

    /// Return in the address field (or a picked row): the text becomes a URL or a search, and only
    /// web pages and Tandem's own pages load directly; a `file:` URL passes the one navigation
    /// verdict; every other scheme goes to the external-scheme handler.
    pub(super) fn browser_url_submit(&mut self, pane: PaneId, text: &str) {
        let template = browser::search_template(&self.globals.string("SearchCommand"));
        let url = browser::submitted_url(text, &template);
        match browser::typed_route(&url) {
            browser::Typed::Navigate(u) => self.browser_navigate(pane, &u),
            browser::Typed::File(u) => {
                let info = webhost::NavInfo { target: &u, source: None, user: true, redirect: false, by_tandem: false };
                let policy = webhost::NavPolicy { guard_local_schemes: true, allowed_schemes: None };
                let judge = |p: &std::path::Path| crate::localfs::LocalPath::judge(p).is_some();
                if webhost::nav_verdict(&info, &policy, &|t| webhost::file_url_local(t, &judge)) {
                    self.browser_navigate(pane, &u);
                } else {
                    self.banners.set("action", "Tandem will not open that file location.");
                }
            }
            browser::Typed::External(u) => self.run_external(&u, true),
        }
        self.blur_url_bar(pane, true);
    }

    /// Loads `url` in the pane's view (Tandem's own load: the person typed it).
    pub(super) fn browser_navigate(&mut self, pane: PaneId, url: &str) {
        if let Some(bv) = self.browser_views.get_mut(&pane) {
            bv.bar.transition = Some(browser::TRANSITION_TYPED);
            bv.bar.nav_state.will_load(url);
            // A person's own load ends the wait of a restored page.
            bv.world.restore_load = None;
            // While the world is being set up nothing loads (review I2): the URL waits.
            if bv.view.is_some() && bv.world.defer_load(url) {
                return;
            }
            match &bv.view {
                Some(v) => v.navigate_tandem(url),
                None => {
                    bv.pending_url = Some(url.to_string());
                    bv.stopped = false;
                    bv.restarts = webhost::Restarts::default();
                }
            }
        }
    }

    /// An edit of the field: bookmark and history candidates come from the database,
    /// the service's answer from the cache now and the network in the background (the 50 ms race
    /// of `BOS:43-64` is the cache answering at once and the late answer replacing it).
    pub(super) fn browser_suggest(&mut self, pane: PaneId, text: &str) {
        let search = browser::search_template(&self.globals.string("SearchCommand"));
        let service = browser::suggest_template(&self.globals.string("SearchSuggestURL"));
        let query = text.trim().to_string();
        // Anything the Enter path would navigate to is not sent: a URL could hold a password or token.
        let asks = browser::suggest_allowed(&query);
        let started = if asks {
            let cached = self.browser_views.get(&pane).is_some_and(|b| b.bar.cache.has(&query)) || self.browser_views.get(&pane).and_then(|b| b.bar.call.as_ref()).is_some_and(|(_, q)| *q == query);
            if cached {
                None
            } else {
                let req = crate::ai_transport::HttpRequest { method: "GET", url: browser::search_url(&service, &query), headers: Vec::new(), body: Vec::new(), stream: false };
                let options = crate::ai_transport::Options { idle: Duration::from_secs(5), proxy: crate::ai_transport::Proxy::System };
                Some(self.ai_transport.start(req, options, self.notify.clone()))
            }
        } else {
            None
        };
        // Bookmarks and visited sites from the database (`BSC:44-126`): bookmarks first, each with
        // its visit count; the database bounds every limit and every text.
        let dev_null = self.pane_dev_null(pane);
        let (bookmarks, history) = match self.browser_db(dev_null) {
            Some(db) => (db.bookmark_suggestions(&query, 10).into_iter().map(|(u, t)| { let v = db.visit_count(&u); (u, t, v) }).collect::<Vec<_>>(), db.history_suggestions(&query, 10)),
            None => (Vec::new(), Vec::new()),
        };
        let Some(bv) = self.browser_views.get_mut(&pane) else { return };
        if let Some(call) = started {
            if let Some((mut old, _)) = bv.bar.call.take() {
                old.cancel();
            }
            bv.bar.call = Some((call, query.clone()));
        } else if !asks {
            if let Some((mut old, _)) = bv.bar.call.take() {
                old.cancel();
            }
        }
        let open = if asks { bv.bar.cache.get(&query).unwrap_or_else(|| bv.bar.cache.fallback(&query)) } else { Vec::new() };
        let mut rows = browser::rank(text, &bookmarks, &history, &open, &search);
        rows.truncate(browser::MAX_SUGGESTIONS);
        bv.bar.suggestions = rows;
        bv.bar.selected = None;
        self.redraw();
    }

    /// The field's keys (it has the keyboard) and the accelerators that focus it. True when taken.
    pub(super) fn browser_key(&mut self, ev: &KeyEvent) -> bool {
        let Some(pane) = self.focused_id().filter(|p| self.is_browser_pane(*p)) else { return false };
        let keys = self.browser_views.get(&pane).is_some_and(|b| b.bar.keys);
        if ev.state != ElementState::Pressed {
            return keys;
        }
        let mods = self.mods;
        if super::toolbelt_tools_ui::is_modifier(&ev.logical_key) {
            return keys;
        }
        if !keys {
            let letter = |c: &str| matches!(&ev.logical_key, WKey::Character(k) if k.as_str().eq_ignore_ascii_case(c));
            let focus = (letter("l") && mods.ctrl && !mods.alt && !mods.cmd) || (letter("d") && mods.alt && !mods.ctrl && !mods.cmd) || ev.logical_key == WKey::Named(NamedKey::F6);
            if focus {
                self.focus_url_bar(pane);
            }
            return focus;
        }
        let plain = !(mods.ctrl || mods.alt || mods.cmd);
        match &ev.logical_key {
            WKey::Named(NamedKey::Escape) if plain => {
                let open = self.browser_views.get(&pane).is_some_and(|b| !b.bar.suggestions.is_empty());
                if open {
                    if let Some(bv) = self.browser_views.get_mut(&pane) {
                        bv.bar.suggestions.clear();
                        bv.bar.selected = None;
                    }
                    self.redraw();
                } else {
                    self.blur_url_bar(pane, true);
                }
                return true;
            }
            WKey::Named(NamedKey::Tab) if plain => {
                self.blur_url_bar(pane, true);
                return true;
            }
            WKey::Named(NamedKey::Enter) if plain || mods.shift => {
                let (text, picked) = match self.browser_views.get(&pane) {
                    Some(b) => (b.bar.edit.text.clone(), b.bar.selected.and_then(|i| b.bar.suggestions.get(i)).map(|s| s.url.clone())),
                    None => return true,
                };
                self.browser_url_submit(pane, &picked.unwrap_or(text));
                return true;
            }
            WKey::Named(k @ (NamedKey::ArrowDown | NamedKey::ArrowUp)) if plain => {
                if let Some(bv) = self.browser_views.get_mut(&pane) {
                    let n = bv.bar.suggestions.len();
                    if n > 0 {
                        bv.bar.selected = match (k, bv.bar.selected) {
                            (NamedKey::ArrowDown, None) => Some(0),
                            (NamedKey::ArrowDown, Some(i)) => Some((i + 1).min(n - 1)),
                            (_, Some(0) | None) => None,
                            (_, Some(i)) => Some(i - 1),
                        };
                    }
                }
                self.redraw();
                return true;
            }
            _ => {}
        }
        let Some((key, ctrl, alt)) = super::toolbelt_more_ui::edit_key(&ev.logical_key, ev.text.as_deref(), mods) else { return plain };
        if matches!(key, EditKey::Enter | EditKey::Up | EditKey::Down | EditKey::PageUp | EditKey::PageDown) || !text_edit::takes(ctrl, alt, mods.cmd, &key) {
            return false;
        }
        let key = match self.with_clipboard_text(key) {
            EditKey::Paste(t) => EditKey::Paste(one_line(&t)),
            EditKey::Text(t) => EditKey::Text(one_line(&t)),
            k => k,
        };
        self.browser_field_edit(pane, key)
    }

    /// One editing key for the field.
    fn browser_field_edit(&mut self, pane: PaneId, key: EditKey) -> bool {
        let cw = self.cell().0;
        let shift = self.mods.shift;
        let measure = move |t: &str| text_w(t, cw);
        let Some(bv) = self.browser_views.get_mut(&pane) else { return false };
        let lines = text_edit::wrap(&bv.bar.edit.text, 1.0e9, &measure);
        let out = bv.bar.edit.key(key, shift, &lines, &measure);
        let text = bv.bar.edit.text.clone();
        match out {
            EditOut::Changed => self.browser_suggest(pane, &text),
            EditOut::Copied(t) => {
                self.set_clipboard_text(t);
                // A cut changed the text too.
                self.browser_suggest(pane, &text);
            }
            EditOut::Moved | EditOut::Kept => {}
        }
        self.redraw();
        true
    }

    /// A key the page reported (`Event::Key`): a Win chord Tandem has a command for runs, and the
    /// accelerators that focus the address field. The page keeps every other key.
    pub(super) fn browser_page_key(&mut self, pane: PaneId, vk: u32, mods: u8, event_loop: &ActiveEventLoop) -> bool {
        let (shift, ctrl, alt, win) = (mods & 1 != 0, mods & 2 != 0, mods & 4 != 0, mods & 8 != 0);
        if (ctrl && !alt && !win && vk == 0x4C) || (alt && !ctrl && !win && vk == 0x44) || vk == 0x75 {
            self.focus_url_bar(pane);
            return true;
        }
        if !win {
            return false;
        }
        let Some(key) = crate::winkeys::vk_to_key(vk as u16) else { return false };
        let chord = crate::menus::Chord { key, cmd: true, alt, shift, ctrl };
        let bound = crate::keyroute::win_chord_bound(&self.route_snapshot.read().unwrap_or_else(|e| e.into_inner()), &chord);
        if bound {
            self.in_pane_window(pane, |a| a.run_chord(&chord, event_loop));
        }
        bound
    }

    /// The menu commands that act on a browser pane (View > Make Text Bigger, Smaller, Normal Size,
    /// Edit > Copy and Paste): true when the focused pane is a browser and the command was its.
    pub(super) fn browser_command(&mut self, selector: &str) -> bool {
        let Some(pane) = self.focused_id().filter(|p| self.is_browser_pane(*p)) else { return false };
        let keys = self.browser_views.get(&pane).is_some_and(|b| b.bar.keys);
        match selector {
            "biggerFont:" => self.browser_zoom(pane, 1),
            "smallerFont:" => self.browser_zoom(pane, -1),
            "returnToDefaultSize:" => self.browser_zoom(pane, 0),
            "copy:" if keys => {
                self.browser_field_edit(pane, EditKey::Copy);
            }
            "paste:" if keys => {
                let key = self.with_clipboard_text(EditKey::Paste(String::new()));
                if let EditKey::Paste(t) = key {
                    self.browser_field_edit(pane, EditKey::Paste(one_line(&t)));
                }
            }
            "copy:" => {
                if let Some(v) = self.browser_views.get(&pane).and_then(|b| b.view.as_ref()) {
                    v.execute_script("document.execCommand('copy')", 0);
                }
            }
            "paste:" => {
                // The person's own clipboard into the page's focused field; the page is never
                // given permission to read it.
                if let Some(text) = self.clipboard_text().filter(|t| t.len() <= 1_048_576) {
                    if let (Ok(json), Some(v)) = (serde_json::to_string(&text), self.browser_views.get(&pane).and_then(|b| b.view.as_ref())) {
                        v.execute_script(&format!("document.execCommand('insertText', false, {json})"), 0);
                    }
                }
            }
            _ => return false,
        }
        true
    }

    /// Make Text Bigger, Smaller or Normal Size on a browser pane: the zoom changes by five percent
    /// and is written to the pane's own copy of the profile (`PS:7967-7970`).
    fn browser_zoom(&mut self, pane: PaneId, dir: i32) {
        let Some(current) = self.panes.get(&pane).and_then(|p| p.browser_io()).map(|io| io.zoom as i64) else { return };
        let saved = self.panes.get(&pane).and_then(|p| self.store.get(&self.store.shared_guid(&p.profile))).map_or(100, |p| p.int("Browser Zoom"));
        let zoom = browser::zoom_step(current, dir, saved);
        if let Some(own) = self.divorce(pane) {
            self.store.set_session(&own, &[("Browser Zoom".to_string(), Some(toml::Value::Integer(zoom)))]);
        }
        if let Some(io) = self.panes.get_mut(&pane).and_then(|p| p.browser_io_mut()) {
            io.zoom = zoom as f64;
        }
        if let Some(v) = self.browser_views.get(&pane).and_then(|b| b.view.as_ref()) {
            v.set_zoom(zoom as f64 / 100.0);
        }
        self.redraw();
    }
}

impl App {
    /// The remembered button of a warning identifier, read through 13b (`is_silenced`, then
    /// `saved_selection`); None while it is not silenced.
    fn saved_choice(&self, id: &str) -> Option<i64> {
        let now = super::warnings_app::unix_now();
        crate::warnings::is_silenced(&self.globals, id, now).then(|| crate::warnings::saved_selection(&self.globals, id) as i64)
    }

    /// (allowed, runtime) for `open_decision`: `allowed` is `BrowserProfiles` on with the runtime
    /// found; `runtime` is false only when the browser is on and the runtime is missing.
    fn browser_gate(&self) -> (bool, bool) {
        let on = self.globals.bool("BrowserProfiles");
        let found = self.web_runtime() != Runtime::Missing;
        (on && found, found || !on)
    }

    /// Whether a link would open in Tandem or ask to (the menu's split items, Shift+Win-click).
    pub(super) fn link_locally_openable(&self, url: &str) -> bool {
        let (allowed, runtime) = self.browser_gate();
        browser::conditionally_openable(url, allowed, runtime, self.saved_choice(browser::OPEN_LINKS_ID), self.saved_choice(browser::UPSELL_ID))
    }

    /// Opens a web link a person asked for (`it_openURL`): in a browser pane, after the ask, or with
    /// the system. Only http and https reach the browser pane, and only a URL `web_link` cleaned; any
    /// other scheme keeps the old road (`guard_open`, the 13r rules). The remembered answers are
    /// 13b's: the ask is queued and `ask_scripts` answers a silenced one at once, so Suppressed Alerts
    /// and "show remembered" work.
    pub(super) fn open_link(&mut self, url: &str, style: browser::Style, background: bool, pane: Option<PaneId>) {
        let Some(link) = browser::web_link(url) else { return self.open_link_system(url, background) };
        let (allowed, runtime) = self.browser_gate();
        match browser::open_decision(&link, style, true, allowed, runtime, None, None) {
            browser::Open::Local(style) => self.open_url_in_browser(&link, style, pane),
            browser::Open::AskOpenInApp { file } => {
                let ask = super::scripts_alerts::Ask::new(browser::open_link_spec(file), super::scripts_alerts::Then::OpenLink { url: link, style, background, pane, file });
                self.ask_scripts(ask);
            }
            browser::Open::Upsell => {
                let ask = super::scripts_alerts::Ask::new(browser::upsell_spec(), super::scripts_alerts::Then::BrowserUpsell { url: link });
                self.ask_scripts(ask);
            }
            browser::Open::System => self.open_link_system(url, background),
            browser::Open::Nothing => {}
        }
    }

    /// The system opens a link the old way: only what `shell_url` allows.
    fn open_link_system(&mut self, url: &str, background: bool) {
        if crate::semantic::shell_url(url).is_some() {
            if let Err(e) = super::semantic_ui::shell_open(url, background) {
                self.banners.set("action", format!("Could not open {url}: {e}"));
            }
        }
    }

    /// The answer of the open-links ask: Use Default Browser (0) opens with the system, Open in
    /// Tandem (1) in a pane, Cancel nothing. A file ask is never raised (no route for `file:`).
    pub(super) fn open_link_answered(&mut self, url: &str, style: browser::Style, background: bool, pane: Option<PaneId>, file: bool, index: usize) {
        match index {
            0 => self.open_link_system(url, background),
            1 if !file => self.open_url_in_browser(url, style, pane),
            _ => {}
        }
    }

    /// The answer of the upsell: Download (0) opens Microsoft's WebView2 page through `guard_open`,
    /// Use System Browser (1) opens the link, Cancel nothing.
    pub(super) fn upsell_answered(&mut self, url: &str, index: usize) {
        match index {
            0 => {
                if let crate::semantic::Run::Open(t) = crate::semantic::guard_open(browser::PLUGIN_URL, false) {
                    if let Err(e) = super::semantic_ui::shell_open(&t, false) {
                        self.banners.set("action", format!("Could not open {t}: {e}"));
                    }
                }
            }
            1 => self.open_link_system(url, false),
            _ => {}
        }
    }

    /// Shows `url` in a browser pane with the default browser profile (one is made, with Initial URL
    /// the link, when none exists, `iTermController.m:2036-2069`): a tab, a window, or a split beside
    /// `near`. Tandem's own UI and `open_link` call it; it takes http, https and Tandem's local pages
    /// only.
    pub(super) fn open_url_in_browser(&mut self, url: &str, style: browser::Style, near: Option<PaneId>) {
        let Some(url) = browser::normalize_url(url).filter(|u| matches!(webhost::scheme_of(u).as_str(), "http" | "https" | "iterm2-about")).and_then(|u| browser::saved_url(&u)) else {
            return;
        };
        let default = self.store.default_profile();
        let found = if self.is_browser_profile(default) { Some(default.guid()) } else { self.store.profiles.iter().find(|p| self.is_browser_profile(p)).map(|p| p.guid()) };
        let (guid, made) = match found {
            Some(g) => (g, false),
            None => {
                let shared = self.store.default_profile().clone();
                let fresh = uuid::Uuid::new_v4().to_string().to_uppercase();
                let copy = crate::session_profile::divorce(&shared, fresh.clone());
                let values = vec![("Custom Command".to_string(), Some(browser::BROWSER_VALUE.into())), ("Initial URL".to_string(), Some(url.clone().into()))];
                (self.add_session_copy(copy, &shared, &values, fresh), true)
            }
        };
        self.browser_url_override = Some(url);
        match style {
            browser::Style::Tab => self.new_tab(Some(&guid)),
            browser::Style::Window => {
                self.pending_windows.push(super::windows_ui::NewWindow::Profile(Some(guid.clone())));
                self.redraw();
            }
            browser::Style::Vertical | browser::Style::Horizontal => {
                if let (Some(n), Some(t)) = (near, self.tabs.get_mut(self.active)) {
                    if t.layout.panes().contains(&n) {
                        t.focused = n;
                    }
                }
                self.split(if style == browser::Style::Vertical { Axis::Row } else { Axis::Column }, Some(&guid));
            }
        }
        // A window starts later and takes the override and the copy then; the others are done:
        // a copy no pane took (a refused split or tab) goes, and the override with it.
        if style != browser::Style::Window {
            if made {
                self.forget_session_profile(&guid);
            }
            self.browser_url_override = None;
        }
        self.bring_forward();
    }

    /// Shift+Win-click on a link (`PTYTextView.m:7987-8043`): a link that opens locally goes in a
    /// vertical split when the pane is wider than tall, else a horizontal one. False when the click
    /// is not on such a link (the ordinary Win-click goes on).
    pub(super) fn open_link_split(&mut self, id: PaneId, point: Point) -> bool {
        let Some((_, target, _)) = self.target_at(id, point) else { return false };
        let (crate::semantic::Target::Link(u) | crate::semantic::Target::Url(u)) = target else { return false };
        if crate::ssh_files::click_route(self.remote_text(id), Some(&u)) != crate::ssh_files::ClickRoute::Local {
            return false;
        }
        let crate::semantic::Run::Open(t) = crate::semantic::guard_open(&u, false) else { return false };
        if !self.link_locally_openable(&t) {
            return false;
        }
        let wide = self.pane_rect(id).is_some_and(|r| r.w > r.h);
        self.open_link(&t, if wide { browser::Style::Vertical } else { browser::Style::Horizontal }, false, Some(id));
        true
    }
}

/// A browser's saved URL of an arrangement's session table, judged again on load.
pub(super) fn restored_url(session: &toml::Value) -> Option<String> {
    browser::arrangement_url(session.get(browser::URL_KEY)?.as_str()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pane::Pane;

    /// 13w seams S10 and S11: the per-pane trigger loop skips a browser pane (its profile may
    /// carry triggers, its screen is empty), the job watcher finds no shell, no bell can come from
    /// it (no output feed), and its flash and bell icon draw on the toolbar strip.
    #[test]
    fn triggers_and_bells_skip_browser_panes() {
        let p = Pane::browser(7, 80, 24, BrowserIo::default());
        assert!(p.shell_pid().is_none(), "jobwatch's ancestry has nothing to read");
        assert!(!p.screen.pty_backed);
        assert_eq!(p.screen.bell_count, 0);
        let t = include_str!("triggers_ui.rs");
        let body = &t[t.find("let browser = p.browser_io().is_some();").expect("the loop knows browser panes")..];
        let skip = body.find("if browser {").unwrap();
        assert!(skip < body.find("s.check_instant_triggers(now)").unwrap() && skip < body.find("s.check_idempotent_triggers(now)").unwrap() && skip < body.find("s.check_event_timers(now)").unwrap());
        let r = include_str!("requests_ui.rs");
        let draw = &r[r.find("pub(super) fn draw_attention(").unwrap()..];
        let draw = &draw[..draw.find("\n    }\n").unwrap()];
        assert!(draw.find("self.browser_flash_rect(pane)").unwrap() < draw.find("scene::pane_flash(").unwrap(), "the flash rect is the toolbar strip");
    }

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
        assert!(browser_setup(true, false, vec![]).in_private && !browser_setup(false, false, vec![]).in_private);
        let s = browser_setup(false, false, vec![]);
        assert!(s.guard_local_schemes && s.user_agent.as_deref() == Some(""));
    }

    /// The hand-over compares per window and moves focus only for the window in front (fix r1).
    #[test]
    fn the_focus_handover_is_gated_on_the_front_window() {
        let src = include_str!("browser_app.rs");
        let prod = &src[..src.find("#[cfg(test)]").unwrap()];
        let f = &prod[prod.find("pub(super) fn place_browser_views").unwrap()..prod.find("fn show_browser_view").unwrap()];
        assert!(f.contains("foreground_root() == Some(hwnd)") && f.contains("self.focus_seen.step(w, focused, in_front)"));
        assert!(f.contains("focused == Some(pane) && in_front"), "a view made in the background is not focused");
        assert!(!f.contains("mem::replace(&mut self.focus_seen"));
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

    /// The only COM handlers are the host's; a browser pane gets no host objects, and the only bridge
    /// is the `iterm2-about:` pages' shim and the filter that serves them.
    #[test]
    fn browser_views_get_no_page_bridge() {
        let src = include_str!("browser_app.rs");
        let prod = &src[..src.find("#[cfg(test)]").unwrap()];
        assert!(!prod.contains("navigate_to_string") && !prod.contains("HostObject"));
        for dev_null in [false, true] {
            // 13x2: the Page-world scripts of `browser_user_scripts` (today the wrapped local-page
            // bridge, which does nothing off an `iterm2-about:` page) come first, then the Instant
            // Replay sensitive mark (final review I1), which talks only to its own top frame
            // (window.top.postMessage), never to the host. Client scripts never go here.
            let page = vec![crate::browser_pages::shim_js()];
            let s = browser_setup(dev_null, false, page.clone());
            assert!(s.web_message, "the local pages post messages; the host judges each by its source");
            assert_eq!(s.scripts, vec![page[0].clone(), crate::web_replay::sensitive_script()], "the bridge and the sensitive mark are the only injected scripts");
            assert!(page[0].contains("location.protocol === 'iterm2-about:'") && !page[0].contains("globalThis.__tandemClient"), "a Page script never holds the Client binding");
            assert!(!crate::web_replay::sensitive_script().contains("chrome.webview"), "the mark opens no host channel");
            assert_eq!(s.resource_filters, vec!["iterm2-about:*".to_string()], "only Tandem's own scheme is served");
            let on = browser_setup(dev_null, true, vec![]);
            assert_eq!(on.resource_filters, vec!["iterm2-about:*".to_string(), "*".to_string()], "ad blocking adds the catch-all filter");
        }
    }

    /// A typed URL loads only through the one route: web and Tandem pages directly, `file:` through
    /// the navigation verdict, everything else through the external-scheme handler; the suggestion
    /// request never carries a URL.
    #[test]
    fn typed_text_has_one_road() {
        let src = include_str!("browser_app.rs");
        let prod = &src[..src.find("#[cfg(test)]").unwrap()];
        let submit = &prod[prod.find("pub(super) fn browser_url_submit").unwrap()..prod.find("fn browser_navigate").unwrap()];
        assert!(submit.contains("typed_route") && submit.contains("nav_verdict") && submit.contains("file_url_local") && submit.contains("run_external"));
        assert!(!submit.contains(".navigate(") && !submit.contains("shell_open"));
        let nav = &prod[prod.find("fn browser_navigate").unwrap()..prod.find("pub(super) fn browser_suggest").unwrap()];
        assert!(nav.contains("navigate_tandem") && !nav.contains(".navigate("));
        let suggest = &prod[prod.find("pub(super) fn browser_suggest").unwrap()..prod.find("pub(super) fn browser_key").unwrap()];
        assert!(suggest.contains("browser::suggest_allowed(&query)"), "a URL may hold a password: never sent to the service");
        assert!(suggest.contains("Proxy::System") && suggest.contains("from_secs(5)"));
        let draw = &prod[prod.find("pub(super) fn draw_browser_bar").unwrap()..prod.find("pub(super) fn browser_toolbar_press").unwrap()];
        assert!(draw.contains("browser::display_url(&io.url)") && draw.contains("browser::fit_address(") && draw.contains("browser::security(&io.url, io.failed)"), "the field shows the sanitized page URL and the mark follows the page's state");
        assert!(!draw.contains("frame.overlay"), "never text in the overlay");
    }

    /// Opening a link: the browser pane gets only a cleaned http or https URL (or Tandem's own page
    /// from its own UI), the asks are 13b's and the old road stays for every other scheme.
    #[test]
    fn opening_links_follows_the_rules() {
        let src = include_str!("browser_app.rs");
        let p = &src[..src.find("#[cfg(test)]").unwrap()];
        let open = &p[p.find("pub(super) fn open_link(").unwrap()..p.find("fn open_link_system").unwrap()];
        assert!(open.contains("browser::web_link(url)") && open.contains("self.ask_scripts(ask)"), "the asks go through 13b");
        assert!(open.contains("open_decision(&link, style, true, allowed, runtime, None, None)"), "13b's preempt answers a remembered choice, so the catalog records it");
        assert!(!open.contains("is_silenced") && !open.contains("globals.set"), "no second warning path");
        let into = &p[p.find("pub(super) fn open_url_in_browser(").unwrap()..p.find("pub(super) fn open_link_split").unwrap()];
        assert!(into.contains("\"http\" | \"https\" | \"iterm2-about\""), "no file:, UNC or custom scheme reaches a pane");
        assert!(!into.contains("shell_open") && !into.contains("guard_open"));
        // A file ask is never answered by opening in a pane (no route for file:).
        let ans = &p[p.find("pub(super) fn open_link_answered").unwrap()..p.find("pub(super) fn upsell_answered").unwrap()];
        assert!(ans.contains("1 if !file => self.open_url_in_browser"));
        // Download opens Microsoft's page through guard_open only.
        let up = &p[p.find("pub(super) fn upsell_answered").unwrap()..p.find("pub(super) fn open_url_in_browser").unwrap()];
        assert!(up.contains("guard_open(browser::PLUGIN_URL, false)"));
        // The execute road: a web link opens through open_link, everything else as before.
        let sem = include_str!("semantic_ui.rs");
        assert!(sem.contains("self.open_link(&target, crate::browser::Style::Tab, background, Some(id));"));
        // The two Thens are not capped (a person's own click) and a closed window cancels.
        let alerts = include_str!("scripts_alerts.rs");
        let capped = &alerts[alerts.find("fn capped(&self)").unwrap()..alerts.find("pub(super) fn dropped_open_files").unwrap()];
        assert!(!capped.contains("OpenLink") && !capped.contains("BrowserUpsell"));
        assert!(alerts.contains("Then::OpenLink { .. } | Then::BrowserUpsell { .. } => Answer::Button(2)"));
        assert!(alerts.contains("Then::OpenLink { url, style, background, pane, file } => self.open_link_answered(") && alerts.contains("Then::BrowserUpsell { url } => self.upsell_answered(&url, index)"));
    }

    /// Shift+Win-click splits before the plain Win-click opens, and both Advanced rows exist.
    #[test]
    fn shift_win_click_and_the_saved_answers_are_wired() {
        let m = include_str!("mod.rs");
        assert!(m.find("self.open_link_split(id, point)").unwrap() < m.find("if self.open_target(id, point, false, event_loop)").unwrap());
        for key in [browser::OPEN_LINKS_ID, browser::UPSELL_ID] {
            let row = crate::schema::find_global(key).unwrap_or_else(|| panic!("{key}"));
            assert_eq!((row.tab.as_str(), row.section.as_str()), ("Advanced", "Warnings"), "{key}");
        }
    }
}
