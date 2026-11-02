//! The App's side of the WebView2 host (milestone 13x, Task 1): the runtime check, one environment
//! per user data folder (created lazily, asynchronously), the drain of what views post, and the
//! settings that change them. App-wide state only (never in `swap_live`): environments belong to
//! the process, and each view is held by whoever made it. Until Tasks 2 to 10 land, view events
//! are logged (without any page text) and dropped.

// The environments and the runtime check have no caller until Tasks 2 to 10 make views.
#![allow(dead_code)]

use std::cell::{Cell, OnceCell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use super::*;
use crate::webhost::{self, EnvOptions, Event, Posted, Runtime, Sink, Store, ViewId};
use crate::platform::browser::{self as browser_host, Env};

/// iTerm2's `ExpiringValue` duration for the browser gateway's answers (`GW:40`).
pub(super) const RUNTIME_TTL: Duration = Duration::from_secs(30);

/// How many posted items one tick drains (the rest wait for the next pass).
const DRAIN_PER_TICK: usize = 512;

/// One store's environment: made once, retried only on the next request after a failure.
#[derive(Default)]
struct Slot {
    env: OnceCell<Env>,
    creating: Cell<bool>,
    failed: RefCell<Option<String>>,
}

/// The host's state on the App: the environments, the cached runtime check and the ids.
pub(super) struct Envs {
    tx: Sink,
    slots: HashMap<Store, Rc<Slot>>,
    runtime: RefCell<Option<(Instant, Runtime)>>,
    /// The fixed-version runtime folder the last check accepted.
    fixed: RefCell<Option<PathBuf>>,
    /// A hint refusal the next tick shows once.
    refusal: RefCell<Option<String>>,
    next_view: ViewId,
    seen: (String, String),
    /// The proxy argument the browser environments were last made with, and whether ad blocking was on:
    /// Seeded at launch by `Envs::seeded` from the settings.
    pub(super) proxy_seen: Option<String>,
    pub(super) adblock_seen: Option<bool>,
    /// Until when a failed browser environment is tried again (the old browser process may still
    /// hold the folder after a proxy change).
    pub(super) rebuild_until: Option<Instant>,
    pub(super) retry_at: Option<Instant>,
    /// This run's Dev Null folder name (made on first use).
    dev_null: Option<PathBuf>,
    swept: bool,
    /// When each store's environment was last dropped after its browser process exited.
    lost_at: HashMap<Store, Instant>,
}

impl Envs {
    /// The state and the receiving end of everything views post.
    pub(super) fn new() -> (Envs, Receiver<Posted>) {
        let (tx, rx) = channel();
        (Envs { tx, slots: HashMap::new(), runtime: RefCell::new(None), fixed: RefCell::new(None), refusal: RefCell::new(None), next_view: 1, seen: (String::new(), String::new()), proxy_seen: None, adblock_seen: None, rebuild_until: None, retry_at: None, dev_null: None, swept: false, lost_at: HashMap::new() }, rx)
    }

    /// The state at launch: `proxy` and `ad` are what the browser environments and views will be
    /// made with, so the first change after launch is a change (no silent direct traffic while the
    /// toolbar says the proxy is on).
    pub(super) fn seeded(proxy: String, ad: bool) -> (Envs, Receiver<Posted>) {
        let (mut envs, rx) = Envs::new();
        envs.proxy_seen = Some(proxy);
        envs.adblock_seen = Some(ad);
        (envs, rx)
    }

    /// The browser stores' environments go (a proxy change); they are made again on the next request.
    pub(super) fn drop_browser_stores(&mut self) {
        self.slots.remove(&Store::Browser);
        self.slots.remove(&Store::BrowserDevNull);
    }

    /// A browser process of `store` exited (final review I5): its environment is dead, so the slot
    /// goes and the next request makes a new one. Every view of the store reports the same exit;
    /// within 2 s of a drop the later reports keep the new slot. True when the slot was dropped.
    pub(super) fn env_lost(&mut self, store: Store, now: Instant) -> bool {
        if self.lost_at.get(&store).is_some_and(|t| now.saturating_duration_since(*t) < Duration::from_secs(2)) {
            return false;
        }
        self.lost_at.insert(store, now);
        self.slots.remove(&store);
        true
    }

    /// What a view posts through.
    pub(super) fn sink(&self) -> Sink {
        self.tx.clone()
    }
}

/// What an environment of `store` is created with: the browser stores' own scheme and, for them
/// only, `proxy` (exactly `adblock::proxy_args`' answer or empty, P15). Tools, popovers and icons
/// never use the proxy (`BM:1854-1870` applies it to the browser's data store only).
pub(super) fn env_options(store: Store, proxy: &str) -> EnvOptions {
    match store {
        Store::Default => EnvOptions::default(),
        Store::Browser | Store::BrowserDevNull => EnvOptions {
            args: proxy.to_string(),
            custom_schemes: vec![crate::browser_pages::ABOUT.into(), crate::browser_pages::FILE_SCHEME.into(), crate::browser_pages::SSH_SCHEME.into()],
            // Extensions only in the persistent store: Dev Null "writes nothing" (13x8 Task 10).
            extensions: store == Store::Browser,
        },
    }
}

/// The runtime now: the Evergreen runtime, or the one `BrowserPluginPathHint` names after its
/// checks. A refused hint is Missing (no silent fall back) with the reason.
fn runtime_now(hint: &str) -> (Runtime, Option<PathBuf>, Option<String>) {
    if hint.trim().is_empty() {
        return (webhost::runtime_outcome(browser_host::runtime_version(None)), None, None);
    }
    match browser_host::fixed_runtime_check(hint) {
        Ok(folder) => (webhost::runtime_outcome(browser_host::runtime_version(Some(&folder))), Some(folder), None),
        Err(why) => (Runtime::Missing, None, Some(why)),
    }
}

/// One kind of hosted web view and what a window's remake or close does to it (review fix 1, a
/// class fix). Every file that makes views with `WebView::create` is listed here (a source test
/// pins it), so a later task's views are covered by the same two hooks: `remade` moves them to the
/// new window handle (or closes them) before the old window is dropped; `closing` (given the
/// closing window's handle) moves or closes every view that is a child of it while it still exists.
pub(super) struct HostedView {
    pub(super) file: &'static str,
    pub(super) remade: fn(&mut App, isize),
    pub(super) closing: fn(&mut App, isize),
}

/// Every kind of hosted web view. Add a kind's entry with its first `WebView::create`.
pub(super) const HOSTED_VIEWS: &[HostedView] = &[
    HostedView { file: "tool_web_ui.rs", remade: App::tool_views_remade, closing: App::tool_views_window_closing },
    HostedView { file: "browser_app.rs", remade: App::browser_views_remade, closing: App::browser_window_closing },
    HostedView { file: "web_popover_app.rs", remade: App::popovers_remade, closing: App::popovers_window_closing },
];

impl App {
    /// The current window was made again and `new_hwnd` is its handle: every hosted view follows,
    /// before the old window (and with it a child's parent) is destroyed.
    pub(super) fn window_remade(&mut self, new_hwnd: isize) {
        for h in HOSTED_VIEWS {
            (h.remade)(self, new_hwnd);
        }
    }

    /// The current window (handle `hwnd`) is going: every view that is its child moves to the
    /// window that shows it now or closes, while the parent still exists.
    pub(super) fn close_hosted_views(&mut self, hwnd: isize) {
        for h in HOSTED_VIEWS {
            (h.closing)(self, hwnd);
        }
    }

    /// A parked window (handle `hwnd`, its state `slot`, not current) is dropped (Merge All
    /// Windows): the same hooks for the views that live outside the slot, and the slot's own tool
    /// views close and are forgotten.
    pub(super) fn window_slot_closing(&mut self, slot: &mut super::windows_ui::WinSlot, hwnd: isize) {
        self.browser_window_closing(hwnd);
        for id in slot.close_tool_views() {
            self.tool_view_ids.remove(&id);
        }
    }

    /// The WebView2 Runtime, cached for 30 seconds (iTerm2's gateway cache).
    pub(super) fn web_runtime(&self) -> Runtime {
        if let Some((at, r)) = self.web_envs.runtime.borrow().as_ref()
            && at.elapsed() < RUNTIME_TTL {
                return r.clone();
            }
        let (r, fixed, refusal) = runtime_now(&self.globals.string("BrowserPluginPathHint"));
        let changed = self.web_envs.runtime.borrow().as_ref().is_none_or(|(_, old)| *old != r);
        if changed {
            match &r {
                Runtime::Found(v) => crate::rlog!("webhost: WebView2 Runtime {}", crate::dlog::name(v)),
                Runtime::Missing => crate::rlog!("webhost: no WebView2 Runtime (not installed)"),
            }
        }
        *self.web_envs.fixed.borrow_mut() = fixed;
        if refusal.is_some() {
            *self.web_envs.refusal.borrow_mut() = refusal;
        }
        *self.web_envs.runtime.borrow_mut() = Some((Instant::now(), r.clone()));
        r
    }

    /// The user data folder an environment of `store` uses.
    fn web_folder(&mut self, store: Store) -> Option<PathBuf> {
        let base = browser_host::local_appdata()?;
        Some(match store {
            Store::BrowserDevNull => self.web_envs.dev_null.get_or_insert_with(|| webhost::dev_null_folder(&base, std::process::id(), uuid::Uuid::new_v4().as_u128() as u32)).clone(),
            other => webhost::data_folder(&base, other),
        })
    }

    /// The environment for `store`: starts creating it on first use (and again on a request after
    /// a failure) and answers None until it exists, and while there is no runtime.
    pub(super) fn web_env(&mut self, store: Store) -> Option<&Env> {
        if self.web_runtime() == Runtime::Missing {
            return None;
        }
        let slot = self.web_envs.slots.entry(store).or_default().clone();
        if slot.env.get().is_none() && !slot.creating.get() {
            if !std::mem::replace(&mut self.web_envs.swept, true) {
                // Folders a dead run left behind (and this one's, should the pid repeat).
                browser_host::cleanup_files();
            }
            slot.creating.set(true);
            let proxy = self.browser_proxy_args();
            let tx = self.web_envs.sink();
            match self.web_folder(store) {
                None => {
                    slot.creating.set(false);
                    let why = "the local application data folder is not usable".to_string();
                    *slot.failed.borrow_mut() = Some(why.clone());
                    let _ = tx.send(Posted::env(store, Err(why)));
                }
                Some(folder) => {
                    let fixed = self.web_envs.fixed.borrow().clone();
                    let done_slot = slot.clone();
                    Env::create(
                        store,
                        &folder,
                        fixed.as_deref(),
                        &env_options(store, &proxy),
                        Box::new(move |r| {
                            done_slot.creating.set(false);
                            let posted = match r {
                                Ok(env) => {
                                    *done_slot.failed.borrow_mut() = None;
                                    let _ = done_slot.env.set(env);
                                    Ok(())
                                }
                                Err(e) => {
                                    *done_slot.failed.borrow_mut() = Some(e.clone());
                                    Err(e)
                                }
                            };
                            let _ = tx.send(Posted::env(store, posted));
                        }),
                    );
                }
            }
        }
        self.web_envs.slots.get(&store).and_then(|s| s.env.get())
    }

    /// A view of `store` reported a process failure of `kind` (the one call every view kind makes,
    /// final review residual): a browser process exit drops the store's dead environment, made
    /// again on the next `web_env`. True when `kind` was a browser process exit.
    pub(super) fn web_view_lost(&mut self, store: Store, kind: i32) -> bool {
        if webhost::failure_action(kind) != webhost::FailAction::Environment {
            return false;
        }
        if self.web_envs.env_lost(store, Instant::now()) {
            self.perm_store_lost(store == Store::BrowserDevNull);
        }
        true
    }

    /// A fresh view id.
    pub(super) fn next_view_id(&mut self) -> ViewId {
        let id = self.web_envs.next_view;
        self.web_envs.next_view += 1;
        id
    }

    /// Drains what the views posted; never blocks and never pumps messages. Events are routed by
    /// view id to the tasks that own them (Tasks 2 to 10); until they land, they are logged
    /// (never with page text) and dropped.
    pub(super) fn web_tick(&mut self, event_loop: &ActiveEventLoop) {
        let refusal = self.web_envs.refusal.borrow_mut().take();
        if let Some(why) = refusal {
            self.banners.set("webview2", format!("The WebView2 Runtime folder in Advanced > Web Browser was not used: {why}"));
        }
        self.web_focus_recheck();
        self.web_popover_tick();
        self.browser_tip_tick(event_loop);
        self.browser_gates_sync();
        self.files_tick();
        self.adblock.judged.set(0);
        for _ in 0..DRAIN_PER_TICK {
            let Ok(posted) = self.web_events.try_recv() else { break };
            if let Some((store, result)) = &posted.env {
                match result {
                    Ok(()) => {
                        crate::rlog!("webhost: the {} store is ready", store.log_name());
                        // Views waiting for their environment are made on the next frame.
                        self.redraw();
                    }
                    // COM's reason stays out of the log (13w B8); the store says which.
                    Err(_) => {
                        crate::rlog!("webhost: the {} store failed to start", store.log_name());
                        self.web_env_failed(*store);
                    }
                }
            }
            if let Some(event) = &posted.event {
                self.web_route(event, event_loop);
            }
        }
        // Grouping works across one drain (a camera and microphone request may be two events).
        self.flush_permission_requests();
    }

    /// A held focus (a web view has keyboard focus, so the parent saw `Focused(false)` while still the
    /// foreground window): once another window is in front the real loss runs, once, in the window
    /// that held it (13x Task 1a). The parent gets no event for that deactivation.
    pub(super) fn web_focus_recheck(&mut self) {
        let Some(held) = self.web_focus_held else { return };
        let Some(hwnd) = self.window_hwnd(held) else {
            // The window is gone.
            self.web_focus_held = None;
            self.sync_web_keyboard();
            return;
        };
        if !crate::webhost::held_focus_lost(hwnd, browser_host::foreground_root()) {
            return;
        }
        self.web_focus_held = None;
        self.sync_web_keyboard();
        let back = self.current_window();
        if self.enter(held) {
            self.apply_focus(crate::webhost::FocusStep::Normal, false, held);
            if let Some(b) = back {
                self.enter(b);
            }
        }
    }

    /// A web view (`view`, a child of `parent`) was closed, remade or lost (final review I7, one helper
    /// for every drop site): when its window's page held the keyboard and no other view of that
    /// window is known to have it, the hold ends, the 13u hook is told (Termurion's Win chords and
    /// remaps act again in that window, never on the shell) and the window takes the keyboard back.
    pub(super) fn web_view_dropped(&mut self, view: ViewId, parent: isize) {
        // A profile write or read in flight on it is sent again on another view (13x6).
        self.perm_view_gone(view);
        let held = self.web_focus_held.and_then(|w| self.window_hwnd(w));
        if !webhost::drop_returns_focus(held, parent, self.web_focus_view, view) {
            return;
        }
        self.web_focus_held = None;
        self.web_focus_view = None;
        self.sync_web_keyboard();
        browser_host::focus_window(parent);
    }

    /// Tells the 13u keyboard hook which window's web view holds the keyboard, so it passes keys on
    /// instead of running Termurion's Win-chord commands on the terminal (security: Win+V must paste
    /// into the page, not the shell).
    pub(super) fn sync_web_keyboard(&self) {
        crate::platform::keys::set_web_focus(self.web_focus_held.and_then(|w| self.window_hwnd(w)).unwrap_or(0));
    }

    /// The active session of the key window changed from `prev` to `next` (`PTYTab.m:951-960`):
    /// every pane `pane_focus_reports` names is told through `report_focus_to`, so a pane is
    /// told only a real change, once.
    pub(super) fn browser_focus_moved(&mut self, prev: Option<crate::layout::PaneId>, next: Option<crate::layout::PaneId>) {
        let tag = |p: Option<crate::layout::PaneId>| p.map(|p| (p, self.is_browser_pane(p)));
        for (pane, gained) in crate::webhost::pane_focus_reports(tag(prev), tag(next)) {
            self.report_focus_to(pane, gained);
        }
    }

    /// `PTYSession setFocused:`: writes the DECSET 1004 report when `pane` is not a gateway, is
    /// not held, its program asked, and its last told state differs; records it.
    pub(super) fn report_focus_to(&mut self, pane: crate::layout::PaneId, gained: bool) {
        use alacritty_terminal::term::TermMode;
        let Some(p) = self.panes.get(&pane) else { return };
        if p.browser_io().is_some() {
            return;
        }
        let facts = crate::webhost::FocusFacts { held: p.no_focus_reports > 0, gateway: p.gateway, wants: p.screen.term().mode().contains(TermMode::FOCUS_IN_OUT) };
        if self.focus_reported.report(pane, gained, facts)
            && let Some(p) = self.panes.get_mut(&pane) {
                p.focus_report(gained);
            }
    }

    /// Once per pass for the entered window: when the active pane changed while the window is in
    /// front (and its keyboard is not in its own web view), the panes that lost and gained it hear
    /// it. The per-pane ledger keeps this and the window's own gain or loss from reporting twice.
    pub(super) fn pane_focus_tick(&mut self) {
        let Some(hwnd) = self.hwnd() else { return };
        let in_front = browser_host::foreground_root() == Some(hwnd) && self.web_focus_held != self.current_window();
        let Some(window) = self.current_window() else { return };
        let focused = self.focused_pane_id();
        let moved = match self.report_seen.step(window, focused, in_front) {
            Some(seen) => {
                let mine: Vec<crate::layout::PaneId> = self.tabs.iter().flat_map(|t| t.layout.panes()).collect();
                let tag = |p: Option<crate::layout::PaneId>| p.map(|p| (p, self.is_browser_pane(p)));
                crate::webhost::pane_focus_reports(tag(seen.filter(|p| mine.contains(p))), tag(focused))
            }
            None => Vec::new(),
        };
        let active_terminal = focused.filter(|p| !self.is_browser_pane(*p));
        // Only a pane the ledger last told "gained" is looked up, so an idle pass allocates nothing.
        let reports = crate::webhost::focus_tick(moved, &self.focus_reported, active_terminal, in_front, |p| self.tabs.iter().any(|t| t.layout.panes().contains(&p)));
        for (pane, gained) in reports {
            self.report_focus_to(pane, gained);
        }
    }

    /// One view event, by view id. No owner exists yet: log what is ours, drop the rest.
    fn web_route(&mut self, event: &Event, event_loop: &ActiveEventLoop) {
        // Which view has the keyboard, whatever its kind (for `web_view_dropped`).
        match event {
            Event::Focus(v, true) => self.web_focus_view = Some(*v),
            Event::Focus(v, false) if self.web_focus_view == Some(*v) => self.web_focus_view = None,
            _ => {}
        }
        let lifecycle = matches!(event, Event::Created(_) | Event::Failed(..) | Event::ProcessFailed { .. } | Event::Focus(..));
        if self.tool_view_event(event) && !lifecycle {
            return;
        }
        if self.icon_view_event(event) && !lifecycle {
            return;
        }
        if self.web_popover_view_event(event) && !lifecycle {
            return;
        }
        if self.browser_view_event(event, event_loop) {
            return;
        }
        match event {
            // A view that raised one of these has no pane to answer it (the pane went first): the
            // view's drop completes the request refused (`shut`), so only the fact is logged.
            Event::Permission { view, .. } | Event::NotificationShown { view, .. } => crate::dlog!("webhost: view {} raised a permission event with no pane", *view),
            Event::Created(v) => crate::dlog!("webhost: view {} created", *v),
            // A view's process failed: which view, never the reason's text (13w B8).
            Event::Failed(v, _) => crate::rlog!("webhost: view {} failed", *v),
            // Which view and the kind only (a number), never a reason's text (13w B8).
            Event::ProcessFailed { view, kind } => crate::rlog!("webhost: view {} process failed, kind {}", *view, *kind),
            // A view lost focus: another window may now be in front of a window that held it.
            Event::Focus(_, false) => self.web_focus_recheck(),
            _ => {}
        }
    }

    /// A setting the host reads changed (`WebUserAgent`, `BrowserPluginPathHint`): the runtime is
    /// checked again on the next request. A new fixed-runtime folder takes effect for environments
    /// made after it; one that already exists keeps its runtime until Termurion restarts.
    pub(super) fn web_settings_changed(&mut self) {
        self.browser_settings_changed();
        let now = (self.globals.string("WebUserAgent"), self.globals.string("BrowserPluginPathHint"));
        if now != self.web_envs.seen {
            if now.1 != self.web_envs.seen.1 {
                *self.web_envs.runtime.borrow_mut() = None;
            }
            self.web_envs.seen = now;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_stores_register_the_local_scheme_only() {
        assert_eq!(env_options(Store::Default, ""), EnvOptions::default());
        for s in [Store::Browser, Store::BrowserDevNull] {
            let o = env_options(s, "");
            assert_eq!(o.custom_schemes, vec!["iterm2-about".to_string(), "iterm2-file".to_string(), "iterm2-ssh".to_string()]);
            assert_eq!(o.args, "", "no Chromium switch is composed from anything but the proxy");
            assert_eq!(o.extensions, s == Store::Browser, "extension support is the persistent store's alone");
        }
    }

    /// Task 8 (P15): the proxy reaches the two browser stores and only them, and the app builds it
    /// only from `adblock::proxy_args` over the three globals.
    #[test]
    fn only_the_browser_stores_carry_the_proxy() {
        let arg = crate::adblock::proxy_args(true, "127.0.0.1", 8118);
        assert_eq!(arg, "--proxy-server=http://127.0.0.1:8118");
        assert_eq!(env_options(Store::Default, &arg).args, "", "tools, popovers and icons never use the proxy");
        for s in [Store::Browser, Store::BrowserDevNull] {
            assert_eq!(env_options(s, &arg).args, arg);
        }
        let hostile = crate::adblock::proxy_args(true, "http://h:1 --remote-debugging-port=9222", 1);
        assert_eq!(env_options(Store::Browser, &hostile).args, "");
        let app = include_str!("adblock_app.rs");
        let live = app.split("#[cfg(test)]\nmod tests").next().unwrap();
        assert!(live.contains("crate::adblock::proxy_args(g.bool(\"BrowserProxyEnabled\"), &g.string(\"BrowserProxyHost\"), g.int(\"BrowserProxyPort\"))"), "the one place the argument is made");
        let me = include_str!("webhost_app.rs");
        let me = me.split("#[cfg(test)]\nmod tests").next().unwrap();
        assert!(!me.contains("--proxy-server"), "no other place composes a switch");
    }

    /// The 13w merge (S1, B8): every web view log site goes through `dlog!`/`rlog!` (LogSafe
    /// arguments only), never `host::log` with a formatted string; a page's console is logged as
    /// its origin and length (`dlog::origin`), and a URL never reaches a log any other way.
    #[test]
    fn browser_logs_go_through_the_debug_log() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let files = ["webhost.rs", "webhost_win.rs", "browser.rs", "web_popover.rs", "svg_icon.rs", "svg_raster_win.rs", "app/browser_app.rs", "app/web_popover_app.rs", "app/webhost_app.rs", "app/tool_web_ui.rs", "app/adblock_app.rs"];
        for f in files {
            let text = std::fs::read_to_string(root.join(f)).unwrap();
            let live = text.split("#[cfg(test)]\nmod tests").next().unwrap();
            assert!(!live.contains("host::log("), "{f}: a web view log line goes through dlog!/rlog!, not host::log");
            assert!(!live.contains("eprintln!"), "{f}: no stderr fallback (B8)");
        }
        // The chat icon's lines (13x Task 4) too.
        let chat = include_str!("chat_window_app.rs");
        let icons = &chat[chat.find("fn icon_").unwrap()..chat.find("#[cfg(test)]\nmod tests").unwrap()];
        assert!(!icons.contains("host::log("), "the chat icon logs through dlog!");
        let tool = include_str!("tool_web_ui.rs");
        assert!(tool.contains("let from = crate::dlog::origin(source);"), "the console line names the page's origin only");
        // The low-frequency facts are in the retrospective ring (13w Log sites).
        let me = include_str!("webhost_app.rs");
        for fact in ["crate::rlog!(\"webhost: WebView2 Runtime {}\"", "crate::rlog!(\"webhost: no WebView2 Runtime", "crate::rlog!(\"webhost: the {} store is ready\"", "crate::rlog!(\"webhost: the {} store failed to start\"", "crate::rlog!(\"webhost: view {} failed\""] {
            assert!(me.contains(fact), "{fact}");
        }
        assert!(include_str!("browser_app.rs").contains("crate::rlog!(\"browser pane {} started: profile {} dev null {}\", id, crate::dlog::name(&name), dev_null);"));
    }

    /// Final review I5: a browser process exit drops its store's environment (made again on the
    /// next request), once per crash: the other views' reports of the same exit keep the new one.
    #[test]
    fn a_lost_browser_process_drops_its_environment_once() {
        let (mut envs, _rx) = Envs::new();
        let t0 = Instant::now();
        envs.slots.insert(Store::Browser, Rc::new(Slot::default()));
        envs.slots.insert(Store::Default, Rc::new(Slot::default()));
        assert!(envs.env_lost(Store::Browser, t0));
        assert!(!envs.slots.contains_key(&Store::Browser));
        assert!(envs.slots.contains_key(&Store::Default), "only that store");
        envs.slots.insert(Store::Browser, Rc::new(Slot::default()));
        assert!(!envs.env_lost(Store::Browser, t0 + Duration::from_secs(1)), "the same crash, told by another view");
        assert!(envs.slots.contains_key(&Store::Browser));
        assert!(envs.env_lost(Store::Browser, t0 + Duration::from_secs(5)), "a later crash");
        // The app side: every view kind acts on the kind, and the browser pane counts good starts.
        let b = include_str!("browser_app.rs");
        let prod = &b[..b.find("#[cfg(test)]").unwrap()];
        assert!(prod.contains("webhost::pane_step(kind, &mut bv.restarts, now)") && prod.contains("self.browser_view_failed(pane, *kind);"));
        assert!(prod.contains("Event::Created(_) => {") && prod.contains("bv.restarts.succeeded();"));
        assert!(prod.contains("self.web_view_lost(browser::store_of(dev_null), kind)"));
        let t = include_str!("tool_web_ui.rs");
        assert!(t.contains("Event::ProcessFailed { kind, .. } => self.tool_view_failed(view, *kind)"));
        let p = include_str!("web_popover_app.rs");
        assert!(p.contains("Event::ProcessFailed { kind, .. } if crate::webhost::failure_action(*kind) != crate::webhost::FailAction::Log"));
        assert!(include_str!("../svg_raster_win.rs").contains("Event::ProcessFailed { kind, .. } if webhost::failure_action(*kind) != webhost::FailAction::Log"));
    }

    /// Final review residual (class): ONE environment registry. Every view kind gets its
    /// environment from `web_env` (no other production `Env::create`), and every consumer of a
    /// process failure tells the registry (`web_view_lost`), so a browser process exit drops the
    /// dead environment whichever store it was.
    #[test]
    fn every_view_kind_gets_its_environment_from_the_registry() {
        let prod = |src: &'static str| &src[..src.find("#[cfg(test)]").unwrap_or(src.len())];
        let apps: [(&str, &str); 4] = [
            ("tool_web_ui.rs", include_str!("tool_web_ui.rs")),
            ("browser_app.rs", include_str!("browser_app.rs")),
            ("web_popover_app.rs", include_str!("web_popover_app.rs")),
            ("chat_window_app.rs", include_str!("chat_window_app.rs")),
        ];
        for (name, src) in apps {
            let p = prod(src);
            assert!(p.contains("self.web_env("), "{name} gets its environment from the registry");
            assert!(!p.contains("Env::create("), "{name} makes no environment of its own");
            // Every consumer of a process failure tells the registry.
            assert!(p.contains("self.web_view_lost("), "{name} reports a lost browser process to the registry");
        }
        for (name, src) in [("web_popover.rs", include_str!("../web_popover.rs")), ("svg_raster_win.rs", include_str!("../svg_raster_win.rs")), ("webhost_win.rs", include_str!("../webhost_win.rs")), ("adblock_app.rs", include_str!("adblock_app.rs"))] {
            assert!(!prod(src).contains("Env::create("), "{name}: only the registry makes an environment in production");
        }
        // The registry's helper acts on the kind and only then drops the store's slot.
        let me = prod(include_str!("webhost_app.rs"));
        assert!(me.contains("pub(super) fn web_view_lost(&mut self, store: Store, kind: i32)"));
        assert!(me.contains("self.web_envs.env_lost(store, Instant::now())"));
        // Tool views follow the browser pane's retry rule: by kind, reset after a good start,
        // bounded (no endless per-minute retry).
        let t = prod(include_str!("tool_web_ui.rs"));
        assert!(t.contains("webhost::pane_step(kind,"), "tool views choose their step by kind");
        assert!(!t.contains("allow_rate("), "no per-minute-only retry");
        assert!(t.contains(".succeeded()"), "a good start resets the count");
        // Only the registry's own store set decides a drop: every store, not only the browser's.
        let (mut envs, _rx) = Envs::new();
        envs.slots.insert(Store::Default, Rc::new(Slot::default()));
        assert!(envs.env_lost(Store::Default, Instant::now()));
        assert!(!envs.slots.contains_key(&Store::Default));
    }

    /// Final review I7 (class): every place a browser or tool view is dropped or remade runs the one
    /// helper that ends the window's "page has the keyboard" state when that view held it.
    #[test]
    fn every_view_drop_site_returns_the_keyboard() {
        let body_of = |src: &'static str, name: &str| -> &'static str {
            let at = src.find(name).unwrap_or_else(|| panic!("{name}"));
            let rest = &src[at..];
            &rest[..rest.find("\n    }\n").unwrap()]
        };
        let b = include_str!("browser_app.rs");
        for f in ["fn browser_view_drop(", "pub(super) fn close_browser_view(", "fn browser_view_failed("] {
            assert!(body_of(b, f).contains("self.web_view_dropped("), "{f}");
        }
        assert!(body_of(include_str!("adblock_app.rs"), "fn browser_views_reset(").contains("self.web_view_dropped("));
        assert!(body_of(include_str!("tool_web_ui.rs"), "fn close_tool_view(").contains("self.web_view_dropped("));
        let me = include_str!("webhost_app.rs");
        let helper = body_of(me, "pub(super) fn web_view_dropped(");
        assert!(helper.contains("webhost::drop_returns_focus(") && helper.contains("self.sync_web_keyboard();") && helper.contains("browser_host::focus_window(parent);"));
        // The view that has the keyboard is known (any kind), and a browser view's loss is rechecked
        // like any other's (final review Minor 6).
        let route = body_of(me, "fn web_route(");
        assert!(route.contains("self.web_focus_view = Some(*v)"), "{route}");
        assert!(b.contains("Event::Focus(_, false) => self.web_focus_recheck(),"));
    }

    #[test]
    fn the_runtime_cache_is_thirty_seconds() {
        assert_eq!(RUNTIME_TTL, Duration::from_secs(30));
    }

    /// The host's App state is app-wide: it is not a per-window field (`swap_live`).
    #[test]
    fn web_state_is_not_swapped_per_window() {
        let swap = include_str!("windows_ui.rs");
        for name in ["web_envs", "web_events", "web_focus_held", "web_focus_view"] {
            assert!(!swap.contains(name), "{name} must not be per window");
        }
    }

    /// Review fix 1 (class): a window that is made again (transparency) or closed must not leave
    /// a hosted web view behind a destroyed parent. Every kind of hosted view is in ONE list,
    /// `HOSTED_VIEWS`, and the window code runs the list's hooks.
    #[test]
    fn every_web_view_kind_registers_with_the_remake_hook() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src").join("app");
        let mut makers = Vec::new();
        for e in std::fs::read_dir(&dir).unwrap() {
            let path = e.unwrap().path();
            if path.extension().is_some_and(|x| x == "rs") {
                let text = std::fs::read_to_string(&path).unwrap();
                let prod = &text[..text.find("#[cfg(test)]").unwrap_or(text.len())];
                if prod.contains("WebView::create(") {
                    makers.push(path.file_name().unwrap().to_string_lossy().to_string());
                }
            }
        }
        makers.sort();
        assert!(makers.len() >= 2, "tool views and popovers: {makers:?}");
        for m in &makers {
            assert!(HOSTED_VIEWS.iter().any(|h| h.file == m), "{m} makes web views but is not in HOSTED_VIEWS");
        }
        for h in HOSTED_VIEWS {
            assert!(makers.iter().any(|m| m == h.file), "{} is listed but makes no view", h.file);
        }
    }

    #[test]
    fn a_remade_window_moves_its_views_before_the_old_one_drops() {
        let t = include_str!("transparency_ui.rs");
        let body = &t[t.find("fn recreate_window_for_alpha").unwrap()..];
        let body = &body[..body.find("\n    }\n").unwrap()];
        let hook = body.find("self.window_remade(").expect("the remake hook is called");
        assert!(hook < body.find("drop(old_gpu)").unwrap(), "before the old surface and window go");
        assert!(hook > body.find("self.gpu.replace(").unwrap(), "after the new window is the current one");
    }

    /// Final review I4: every function that destroys a window runs the closing hook with that
    /// window's handle before the window drops, the browser hook chooses views by parent, and every
    /// path that moves a pane to an existing window moves its view there before the old one closes.
    #[test]
    fn a_closing_window_moves_or_closes_its_views_first() {
        let body_of = |src: &'static str, name: &str| -> &'static str {
            let at = src.find(name).unwrap_or_else(|| panic!("{name}"));
            let rest = &src[at..];
            &rest[..rest.find("\n    }\n").unwrap()]
        };
        let w = include_str!("windows_ui.rs");
        let drop_fn = body_of(w, "fn drop_current_window");
        assert!(drop_fn.find("self.close_hosted_views(hwnd)").unwrap() < drop_fn.find("self.windows.close_current(").unwrap());
        let merge = body_of(w, "pub(super) fn merge_all_windows");
        assert!(merge.find("self.window_slot_closing(&mut slot, h)").expect("the parked slot's views") < merge.find("drop(slot);").unwrap());
        assert!(merge.find("self.tabs.append(&mut moved);").unwrap() < merge.find("self.window_slot_closing(").unwrap(), "after the panes joined this window");
        let r = include_str!("resident_ui.rs");
        assert!(body_of(r, "fn go_resident(").contains("self.close_hosted_views(hwnd)"));
        // The browser hook: by parent, through the one pure choice.
        let b = include_str!("browser_app.rs");
        let hook = body_of(b, "pub(super) fn browser_window_closing");
        assert!(hook.contains("webhost::closing_parent(") && !hook.contains("self.tabs"), "{hook}");
        assert!(include_str!("webhost_app.rs").contains("HostedView { file: \"browser_app.rs\", remade: App::browser_views_remade, closing: App::browser_window_closing }"));
        // Moves to an existing window: the view follows before the old window can drop.
        let t = include_str!("tab_drag_ui.rs");
        for f in ["fn drop_in_window", "fn move_group_to_window"] {
            let body = body_of(t, f);
            let follow = body.find("self.browser_views_follow(").unwrap_or_else(|| panic!("{f}"));
            let drop_at = body.find("self.drop_current_window()").or_else(|| body.find("self.after_tabs_left()")).unwrap();
            assert!(follow < drop_at, "{f}");
        }
        let pm = include_str!("pane_move_ui.rs");
        let split = &pm[pm.find("pane_move::Drop::Split { axis, before } => {").unwrap()..];
        assert!(split.find("self.browser_views_follow(").unwrap() < split.find("self.drop_current_window()").unwrap());
        // A torn-off tab is in its new window before the old one drops (the hook moves the view).
        let tear = body_of(t, "fn tear_off(");
        assert!(tear.find("self.open_pending(").unwrap() < tear.find("self.drop_current_window()").unwrap());
    }

    #[test]
    fn closing_a_window_closes_its_hosted_views() {
        let w = include_str!("windows_ui.rs");
        let body = &w[w.find("fn drop_current_window").unwrap()..];
        assert!(body[..body.find("\n    }\n").unwrap()].contains("self.close_hosted_views("));
        let r = include_str!("resident_ui.rs");
        let body = &r[r.find("fn go_resident(").unwrap()..];
        let body = &body[..body.find("\n    }\n").unwrap()];
        assert!(body.find("self.close_hosted_views(").unwrap() < body.find("self.swap_live(").unwrap(), "before the slot is swapped out");
    }
}
