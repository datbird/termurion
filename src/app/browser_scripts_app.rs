//! The page-script host on the app side (milestone 13x2, Task 1): Tandem's isolated world for each
//! browser view, the message router, calls into the page and into the world, and the one funnel
//! that opens a URL a page supplied. Web content is untrusted: a message is World::Client only
//! when it arrives through `Runtime.bindingCalled` from a context `browser_world::Contexts` knows
//! as Tandem's (never because of its JSON), the per-view secrets exist only in Client scripts and
//! in expressions the world evaluates, a script call is never logged (only the kind of its failure),
//! and no page-world code can make Tandem evaluate anything.

// The calls and fields below are the host's API for the plans that build on it (13x3 to 13x8).
#![allow(dead_code)]

use std::collections::HashSet;
use std::time::Duration;

use super::*;
use crate::browser_js::{self, NavGate, Routed, UserScript, World};
use crate::browser_world::{self, Attach, ClientWorld, Check, SessionSetup, Setup};
use crate::layout::Axis;
use serde_json::{Value, json};

/// What a script call hands back: the value, or the error text (a kind, never page text).
pub(super) type JsDone = Box<dyn FnOnce(&mut App, PaneId, Result<Value, String>)>;

/// Tokens of Tandem's own script and DevTools calls start here: far from 0, 1 (the popovers') and
/// `web_replay`'s two.
pub(super) const TOKEN_BASE: u64 = 1 << 32;
/// How long the world's setup and checks may take before the view loads without it.
const SETUP_TIMEOUT: Duration = Duration::from_secs(5);
/// A page-world script result Tandem will read.
const MAX_RESULT: usize = 4 * 1024 * 1024;
/// Page-world frames remembered per view.
const MAX_PAGE_FRAMES: usize = 512;

/// How a URL a page supplied is opened (`browser_page_open`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum PageOpen {
    /// In the pane itself.
    Here,
    Tab,
    Window,
    Split(Axis),
    /// Quick Look at a point of the screen.
    QuickLook((f64, f64)),
}

/// The run-time state of one view's world that is not a field of its own.
#[derive(Default)]
pub(super) struct WorldState {
    /// The setup and its isolation checks while they run.
    pub(super) setup: Option<Setup>,
    pub(super) setup_started: Option<Instant>,
    /// The URL to load once the setup is over (Ready or not).
    pub(super) load_after: Option<String>,
    /// A URL whose load waits for the launch's window restoration to end (iTerm2 waits for
    /// `iTermSessionRestorationDidCompleteNotification`, `BVC:871-910`).
    pub(super) restore_load: Option<String>,
    /// The check that waits for the navigation to about:blank, and that navigation's id.
    pub(super) reload_wait: bool,
    pub(super) reload_nav: Option<u64>,
    /// After a renderer or frame process failure the view reloads: when this passes, the main frame
    /// must have its Tandem context again (whether Page and Runtime enablement survive a renderer
    /// crash is not known), else the setup runs once more, then the world is Unavailable.
    pub(super) verify_at: Option<Instant>,
    pub(super) restarted: bool,
    /// The world check that waits for the main frame's context.
    pub(super) world_check: Option<&'static str>,
    /// The wrapped Client scripts of this view (a child session gets the same).
    pub(super) client_sources: Vec<String>,
    /// Child sessions with a world, and the ones still being set up.
    pub(super) live_sessions: HashSet<String>,
    pub(super) sessions: HashMap<String, SessionSetup>,
    /// When a dropped message was last logged.
    pub(super) drop_logged: Option<Instant>,
    /// The check's about:blank is cleared from the back list after the first load (review I1).
    pub(super) history: browser_world::HistoryReset,
    /// Tokens of the setup's own DevTools calls: they outlive the navigation that starts the page
    /// load (review I3), so a late identifier is still seen and removed.
    pub(super) setup_calls: HashSet<u64>,
    /// The view's controller has been created (a restart's setup starts its timer at once).
    pub(super) controller_up: bool,
    /// When a console line was last counted in the log.
    pub(super) console_logged: Option<Instant>,
}

impl WorldState {
    /// The call that ends with a main-frame navigation: the page's calls are over, the setup's own
    /// stay.
    pub(super) fn take_navigated(&mut self, pending: &mut HashMap<u64, JsDone>) -> Vec<JsDone> {
        let keep = &self.setup_calls;
        let gone: Vec<u64> = pending.keys().copied().filter(|t| !keep.contains(t)).collect();
        gone.into_iter().filter_map(|t| pending.remove(&t)).collect()
    }

    /// The controller exists: the setup's time starts now (once).
    pub(super) fn controller_ready(&mut self, now: Instant) {
        self.controller_up = true;
        if self.setup.is_some() && self.setup_started.is_none() {
            self.setup_started = Some(now);
        }
    }

    /// The setup has run for more than its time since the controller came up.
    pub(super) fn timed_out(&self, now: Instant) -> bool {
        self.setup.is_some() && self.setup_started.is_some_and(|t| now.saturating_duration_since(t) > SETUP_TIMEOUT)
    }

    /// A navigation that arrives while the world is being set up (review I2): kept, the latest
    /// winning, and run when the setup is over. False when no setup runs: the caller navigates.
    pub(super) fn defer_load(&mut self, url: &str) -> bool {
        if self.setup.is_none() {
            return false;
        }
        self.load_after = Some(url.to_string());
        true
    }
}

/// The kind of a failed script call, the only thing about it that is logged (never its text).
pub(super) fn failure_kind(err: &str) -> &'static str {
    match err {
        "isolated world unavailable" => "unavailable",
        "no context" => "no context",
        "no view" | "view closed" => "no view",
        "navigated" => "navigated",
        "page calls take no arguments" | "bad argument name" => "bad call",
        _ => "exception",
    }
}

thread_local! {
    static LAST_FAILURE_LOG: std::cell::Cell<Option<Instant>> = const { std::cell::Cell::new(None) };
}

/// One line for a failed script call, at most one a second: its kind only. The body, the arguments
/// and the error's own text (which can quote page-chosen text) are never logged.
pub(super) fn log_js_failure(err: &str) {
    let due = LAST_FAILURE_LOG.with(|l| {
        let due = l.get().is_none_or(|t| t.elapsed() >= Duration::from_secs(1));
        if due {
            l.set(Some(Instant::now()));
        }
        due
    });
    if due {
        crate::rlog!("browser script call failed: {}", failure_kind(err));
    }
}

impl App {
    // --- Scripts ---

    /// The scripts a browser view carries, one line per feature (each later plan appends its own).
    pub(super) fn browser_user_scripts(&self, pane: PaneId) -> Vec<UserScript> {
        let bv = self.browser_views.get(&pane);
        let (secret, dom) = bv.map_or(("", ""), |b| (b.secret.as_str(), b.dom_secret.as_str()));
        // Offered while the world is being set up (that is when they are registered) or ready, never
        // once it is Unavailable; messages are acted on only when it is Ready.
        let client_ok = bv.is_none_or(|b| !matches!(b.client_world, ClientWorld::Unavailable(_)));
        let mut scripts = browser_js::always_on_scripts(secret, dom, client_ok, cfg!(debug_assertions));
        // Tandem's pointer gate (Task 7): the table as it is now; a later change reaches the live
        // worlds through `browser_pointer_sync`.
        if client_ok {
            let (keys, four, focus) = self.browser_pointer_table();
            scripts.push(crate::browser_pointer::user_script(secret, &keys, four, focus));
            // Tandem's broadcast script (Task 8): always installed, so turning broadcast on works at
            // once without a reload; `browser_always_on` drops its messages unless the pane sends.
            scripts.push(crate::browser_keys::user_script(secret));
            // iTerm2's find script (13x3 Task 1): every browser pane, the isolated world only. No
            // script when it cannot be assembled: find then finds nothing.
            match crate::browser_find::user_script(secret) {
                Ok(s) => scripts.push(s),
                Err(_) => crate::rlog!("browser find script not registered"),
            }
            // iTerm2's copy mode cursor and Tandem's key listener (13x3 Task 8): both or neither, and
            // only with an isolated world (this block); without them Edit > Copy Mode is not offered.
            // (They fail together only for a secret that is not plain letters and digits, which the find
            // script above would already have logged.)
            if let (Ok(cursor), Ok(keys)) = (crate::browser_copymode::user_script(secret), crate::browser_copymode::keys_script(secret)) {
                scripts.extend([cursor, keys]);
            }
        }
        scripts
    }

    /// The one gate of every Client-world handler: the view's isolated world was proven (`Ready`).
    pub(super) fn client_open(&self, pane: PaneId) -> bool {
        self.browser_views.get(&pane).is_some_and(|b| browser_world::client_gate(&b.client_world))
    }

    /// The per-view secret (filled only into Client scripts and world-side expressions).
    pub(super) fn browser_view_secret(&self, pane: PaneId) -> Option<&str> {
        self.browser_views.get(&pane).map(|b| b.secret.as_str()).filter(|s| !s.is_empty())
    }

    /// The scripts of `pane` split by world and wrapped: (Page, Client).
    pub(super) fn browser_wrapped_scripts(&self, pane: PaneId) -> (Vec<String>, Vec<String>) {
        let scripts = self.browser_user_scripts(pane);
        let wrap = |w: World| scripts.iter().filter(|s| s.world == w).map(browser_js::wrap).collect::<Vec<_>>();
        (wrap(World::Page), wrap(World::Client))
    }

    /// Makes a view's world state new: new secrets, no contexts, no pending call (each answered
    /// `Err("view closed")`). Runs at every site that makes or drops a view (B4).
    pub(super) fn browser_reset_world(&mut self, pane: PaneId) {
        let mut dones = Vec::new();
        if let Some(bv) = self.browser_views.get_mut(&pane) {
            bv.secret = browser_js::new_secret();
            bv.dom_secret = browser_js::new_secret();
            bv.client_world = ClientWorld::Setting;
            bv.contexts = Default::default();
            bv.world_scripts.clear();
            bv.frames.clear();
            bv.page_frames.clear();
            bv.eval_waiting.clear();
            bv.world = WorldState::default();
            bv.pointer_sig = None;
            bv.pointer_down = None;
            bv.receiving_broadcast = 0;
            bv.receiving_since = None;
            bv.echo = Default::default();
            // The new secret makes the old search and its queued commands the old page's.
            bv.find = crate::browser_find::fresh_model();
            bv.find_out.reset();
            // And the old page's copy mode: its switch and cursor lived in the old world.
            bv.copy_mode = None;
            bv.reader.reset();
            dones = bv.pending_js.drain().map(|(_, d)| d).collect();
        }
        self.global_pane_gone(pane);
        for done in dones {
            done(self, pane, Err("view closed".into()));
        }
    }

    // --- Setup ---

    /// After `WebView::create`: subscribes to the DevTools events, then runs the setup calls one at
    /// a time, then the isolation checks; the pending URL loads only when that is over.
    pub(super) fn browser_world_start(&mut self, pane: PaneId, load: Option<String>) {
        let (_, client) = self.browser_wrapped_scripts(pane);
        let Some(bv) = self.browser_views.get_mut(&pane) else { return };
        let Some(view) = &bv.view else { return };
        for name in browser_world::EVENTS {
            view.devtools_subscribe(name);
        }
        bv.world.setup = Some(Setup::new(&client));
        // The time starts when the controller exists: now for a restart, at `Created` for a new view.
        bv.world.setup_started = bv.world.controller_up.then(Instant::now);
        bv.world.load_after = load;
        bv.world.client_sources = client;
        self.browser_world_pump(pane);
    }

    /// Sends the next setup call or check, or finishes.
    fn browser_world_pump(&mut self, pane: PaneId) {
        loop {
            let Some(bv) = self.browser_views.get_mut(&pane) else { return };
            // A world check that waited for the main frame's context.
            if let Some(expr) = bv.world.world_check {
                if bv.contexts.unique_for(None).is_none() {
                    return;
                }
                bv.world.world_check = None;
                self.browser_world_check_world(pane, expr);
                return;
            }
            let Some(setup) = bv.world.setup.as_mut() else { return };
            if *setup.state() != ClientWorld::Setting {
                self.browser_world_finish(pane);
                return;
            }
            if let Some((method, params)) = setup.next_call() {
                let token = self.devtools_send(pane, None, method, &params, Box::new(move |app, pane, r| app.browser_world_answered(pane, method, r)));
                if let (Some(t), Some(bv)) = (token, self.browser_views.get_mut(&pane)) {
                    bv.world.setup_calls.insert(t);
                }
                return;
            }
            // Nothing to send while a call or a check is in flight (or after the end, which the next
            // pass finishes).
            let Some(check) = setup.next_check() else { return };
            match check {
                Check::Reload => {
                    bv.world.reload_wait = true;
                    if let Some(v) = &bv.view {
                        v.navigate_tandem("about:blank");
                    }
                    return;
                }
                Check::Page(expr) => {
                    let token = bv.next_token;
                    bv.next_token += 1;
                    bv.eval_waiting.insert(token);
                    bv.pending_js.insert(token, Box::new(|app, pane, r| app.browser_world_checked(pane, r)));
                    if let Some(v) = &bv.view {
                        v.execute_script(expr, token);
                    }
                    return;
                }
                Check::World(expr) => {
                    bv.world.world_check = Some(expr);
                }
            }
        }
    }

    fn browser_world_check_world(&mut self, pane: PaneId, expr: &'static str) {
        self.browser_evaluate_in(pane, None, expr.to_string(), Box::new(|app, pane, r| app.browser_world_checked(pane, r)), true);
    }

    /// An answer to a check: the value as JSON text, or the error.
    fn browser_world_checked(&mut self, pane: PaneId, r: Result<Value, String>) {
        if let Some(setup) = self.browser_views.get_mut(&pane).and_then(|b| b.world.setup.as_mut()) {
            setup.checked(r.map(|v| v.to_string()));
        }
        self.browser_world_pump(pane);
    }

    /// An answer to a setup call.
    fn browser_world_answered(&mut self, pane: PaneId, method: &'static str, r: Result<Value, String>) {
        let Some(bv) = self.browser_views.get_mut(&pane) else { return };
        // Fail closed: a script registered after the setup ended, or in a view with no world, comes off.
        if let Some(id) = browser_world::late_removal(bv.world.setup.is_none(), &bv.client_world, method, &r) {
            self.devtools_send(pane, None, "Page.removeScriptToEvaluateOnNewDocument", &json!({"identifier": id}), Box::new(|_, _, _| {}));
            return;
        }
        if let Ok(v) = &r {
            match method {
                "Page.addScriptToEvaluateOnNewDocument" => {
                    if let Some(id) = v.get("identifier").and_then(Value::as_str).filter(|s| s.len() <= 128) {
                        bv.world_scripts.push(id.to_string());
                    }
                }
                "Page.getFrameTree" => {
                    if let Some((main, _)) = browser_world::frame_ids(&v.to_string()) {
                        bv.contexts.set_main_frame(&main);
                    }
                }
                _ => {}
            }
        }
        if let Some(setup) = bv.world.setup.as_mut() {
            setup.answered(r.map(|v| v.to_string()));
        }
        self.browser_world_pump(pane);
    }

    /// The setup is over: the world is Ready or Unavailable (then no Client script stays registered),
    /// and the pending URL loads.
    fn browser_world_finish(&mut self, pane: PaneId) {
        let restoring = self.browser_restoring();
        let Some(bv) = self.browser_views.get_mut(&pane) else { return };
        let Some(setup) = bv.world.setup.take() else { return };
        bv.client_world = setup.state().clone();
        bv.world.reload_wait = false;
        bv.world.world_check = None;
        let load = bv.world.load_after.take();
        // While the launch restores windows the page waits: it is loaded when the restoration ends.
        let load = if restoring {
            bv.world.restore_load = load;
            None
        } else {
            load
        };
        let ids = if matches!(bv.client_world, ClientWorld::Unavailable(_)) { std::mem::take(&mut bv.world_scripts) } else { Vec::new() };
        let unavailable = matches!(bv.client_world, ClientWorld::Unavailable(_));
        if unavailable {
            bv.world.client_sources.clear();
            // A script whose isolation was never proven may have seen the secrets (a late
            // registration on the timeout path): they are worthless from now on (final review minor 10).
            bv.secret = browser_js::new_secret();
            bv.dom_secret = browser_js::new_secret();
            // The reason is a fixed text of ours, never a page's; one line.
            crate::rlog!("browser view {} has no isolated world", bv.id);
        }
        for id in ids {
            self.devtools_send(pane, None, "Page.removeScriptToEvaluateOnNewDocument", &json!({"identifier": id}), Box::new(|_, _, _| {}));
        }
        if let Some(url) = load {
            self.browser_load_own(pane, &url);
        }
    }

    /// The navigation of the isolation check finished (it is the one that started with the id
    /// `Starting` recorded): true when it was.
    pub(super) fn browser_world_navigated(&mut self, pane: PaneId, ok: bool, nav: u64) -> bool {
        let Some(bv) = self.browser_views.get_mut(&pane) else { return false };
        if !bv.world.reload_wait || bv.world.reload_nav != Some(nav) {
            return false;
        }
        bv.world.reload_wait = false;
        bv.world.reload_nav = None;
        if let Some(setup) = bv.world.setup.as_mut() {
            setup.checked(if ok { Ok(String::new()) } else { Err("the check navigation failed".into()) });
        }
        self.browser_world_pump(pane);
        true
    }

    /// The view reloaded after a process failure: its world is verified in a few seconds.
    pub(super) fn browser_world_expect(&mut self, pane: PaneId) {
        if let Some(bv) = self.browser_views.get_mut(&pane) {
            if bv.client_world == ClientWorld::Ready {
                bv.world.verify_at = Some(Instant::now());
            }
        }
    }

    /// The world did not come back after a reload: take the old scripts off and set it up once more.
    fn browser_world_restart(&mut self, pane: PaneId) {
        let ids = self.browser_views.get_mut(&pane).map(|b| std::mem::take(&mut b.world_scripts)).unwrap_or_default();
        for id in ids {
            self.devtools_send(pane, None, "Page.removeScriptToEvaluateOnNewDocument", &json!({"identifier": id}), Box::new(|_, _, _| {}));
        }
        let url = self.panes.get(&pane).and_then(|p| p.browser_io()).map(|io| io.url.clone());
        if let Some(bv) = self.browser_views.get_mut(&pane) {
            bv.secret = browser_js::new_secret();
            bv.dom_secret = browser_js::new_secret();
            bv.contexts = Default::default();
            bv.client_world = ClientWorld::Setting;
            // The old world's copy mode died with it (its switch and cursor lived there).
            bv.copy_mode = None;
            bv.reader.reset();
            bv.world.restarted = true;
            bv.world.verify_at = None;
        }
        self.browser_world_start(pane, url);
    }

    /// Called as the view is shown: a setup that took too long ends as Unavailable, and a world that
    /// is missing after a reload is made again.
    pub(super) fn browser_world_tick(&mut self, pane: PaneId) {
        let verify = self.browser_views.get(&pane).and_then(|b| b.world.verify_at).is_some_and(|t| t.elapsed() > Duration::from_secs(3));
        if verify {
            let missing = self.browser_views.get(&pane).is_some_and(|b| b.contexts.unique_for(None).is_none());
            let again = self.browser_views.get(&pane).is_some_and(|b| !b.world.restarted);
            if let Some(bv) = self.browser_views.get_mut(&pane) {
                bv.world.verify_at = None;
            }
            if !missing {
                if let Some(bv) = self.browser_views.get_mut(&pane) {
                    bv.world.restarted = false;
                }
            } else if again {
                self.browser_world_restart(pane);
            } else if let Some(bv) = self.browser_views.get_mut(&pane) {
                bv.client_world = ClientWorld::Unavailable("lost after a process failure".into());
                bv.copy_mode = None;
                bv.reader.reset();
            }
        }
        let late = self.browser_views.get(&pane).is_some_and(|b| b.world.timed_out(Instant::now()));
        if late {
            if let Some(bv) = self.browser_views.get_mut(&pane) {
                if let Some(setup) = bv.world.setup.as_mut() {
                    setup.fail("the setup took too long");
                }
            }
            self.browser_world_finish(pane);
        }
    }

    // --- DevTools ---

    /// The one DevTools sender of the app: `done` gets the answer (the result object) or the error.
    /// A method outside `browser_world::ALLOWED` is refused here and in the host.
    pub(super) fn devtools_send(&mut self, pane: PaneId, session: Option<&str>, method: &str, params: &Value, done: JsDone) -> Option<u64> {
        let why = match self.browser_views.get_mut(&pane) {
            Some(bv) if bv.view.is_some() && browser_world::method_allowed(method) => {
                let token = bv.next_token;
                bv.next_token += 1;
                bv.pending_js.insert(token, done);
                if let Some(v) = &bv.view {
                    match session {
                        None => v.devtools_call(method, &params.to_string(), token),
                        Some(s) => v.devtools_call_for_session(s, method, &params.to_string(), token),
                    }
                }
                return Some(token);
            }
            Some(bv) if bv.view.is_some() => "that DevTools method is not allowed",
            _ => "no view",
        };
        done(self, pane, Err(why.into()));
        None
    }

    /// A DevTools call for a later plan: the method must be in `browser_world::ALLOWED`.
    pub(super) fn browser_devtools(&mut self, pane: PaneId, method: &str, params: Value, done: JsDone) {
        let Some(done) = self.seam_intercept(pane, super::browser_test_seam::Runner::DevTools, &format!("{method} {params}"), done) else { return };
        self.devtools_send(pane, None, method, &params, done);
    }

    /// The lifecycle events the view's host filter held (final review I4), in order.
    fn browser_devtools_held(&mut self, pane: PaneId) {
        let held = self.browser_views.get(&pane).and_then(|b| b.view.as_ref()).map(|v| v.take_devtools_held()).unwrap_or_default();
        for (session, name, json) in held {
            self.browser_devtools_event(pane, session.as_deref(), &name, &json);
        }
    }

    /// One DevTools event of a view (`browser_world::EVENTS`).
    fn browser_devtools_event(&mut self, pane: PaneId, session: Option<&str>, name: &str, json: &str) {
        let Some(bv) = self.browser_views.get_mut(&pane) else { return };
        match name {
            "Runtime.executionContextCreated" => {
                // The pointer table goes only into a context the table accepted, once (final review I4).
                let recorded = bv.contexts.created(session, json);
                // A new Tandem context in the main frame has none of the reader's objects (B8).
                if recorded && bv.contexts.is_main_context(json) {
                    bv.reader.reset();
                }
                bv.frames = bv.contexts.frames();
                self.browser_world_pump(pane);
                if recorded {
                    self.browser_pointer_push_new(pane, session, json);
                    self.browser_copy_mode_new_context(pane, session, json);
                }
            }
            "Runtime.executionContextDestroyed" => {
                bv.contexts.destroyed(session, json);
                Self::browser_frames_changed(bv);
            }
            "Runtime.executionContextsCleared" => {
                bv.contexts.cleared(session);
                Self::browser_frames_changed(bv);
            }
            "Page.frameNavigated" => bv.contexts.navigated(session, json),
            "Target.attachedToTarget" => self.browser_attached(pane, session, json),
            "Target.detachedFromTarget" => {
                if let Some(s) = serde_json::from_str::<Value>(json).ok().and_then(|v| v.get("sessionId").and_then(Value::as_str).map(str::to_string)) {
                    bv.contexts.detached(&s);
                    bv.world.live_sessions.remove(&s);
                    bv.world.sessions.remove(&s);
                    Self::browser_frames_changed(bv);
                }
            }
            "Runtime.bindingCalled" => {
                // A message is the Tandem world's only when the binding was called from a context the
                // table knows as Tandem's; the JSON never says so.
                if let Some((frame, origin, payload)) = bv.contexts.binding_called(session, json) {
                    if let Some(r) = browser_js::route(&payload, World::Client, frame, &origin) {
                        self.browser_message(pane, r);
                    }
                }
            }
            _ => {}
        }
    }

    /// The frames with a Tandem world changed: the graph forgets the ones that lost it.
    fn browser_frames_changed(bv: &mut super::browser_app::BrowserView) {
        let now = bv.contexts.frames();
        for lost in bv.frames.iter().filter(|f| !now.contains(f)) {
            bv.graph.gone(*lost);
        }
        bv.frames = now;
    }

    /// `Target.attachedToTarget` (B11): an iframe within the cap gets the world; everything else is
    /// resumed at once and let go.
    fn browser_attached(&mut self, pane: PaneId, parent: Option<&str>, json: &str) {
        let ready = self.client_open(pane);
        let Some(bv) = self.browser_views.get_mut(&pane) else { return };
        let what = match browser_world::attached(json, bv.world.live_sessions.len()) {
            // A frame of a view whose world is not Ready gets no secret-bearing script: resumed and let go.
            Attach::Frame { session } if !ready => Attach::Resume { session },
            other => other,
        };
        match what {
            Attach::Frame { session } => {
                bv.world.live_sessions.insert(session.clone());
                let sources = bv.world.client_sources.clone();
                bv.world.sessions.insert(session.clone(), SessionSetup::new(&session, &sources));
                self.browser_session_pump(pane, &session);
            }
            Attach::Resume { session } => {
                self.devtools_send(pane, Some(&session), browser_world::RESUME, &json!({}), Box::new(|_, _, _| {}));
                self.devtools_send(pane, parent, "Target.detachFromTarget", &browser_world::detach_params(&session), Box::new(|_, _, _| {}));
            }
            Attach::Ignore => {}
        }
    }

    fn browser_session_pump(&mut self, pane: PaneId, session: &str) {
        let Some(bv) = self.browser_views.get_mut(&pane) else { return };
        let Some(s) = bv.world.sessions.get_mut(session) else { return };
        if let Some((method, params)) = s.next_call() {
            let owned = session.to_string();
            self.devtools_send(pane, Some(session), method, &params, Box::new(move |app, pane, r| {
                if let Some(bv) = app.browser_views.get_mut(&pane) {
                    if let Some(s) = bv.world.sessions.get_mut(&owned) {
                        s.answered(r.map(|v| v.to_string()));
                    }
                }
                app.browser_session_pump(pane, &owned);
            }));
        } else if s.done() {
            bv.world.sessions.remove(session);
        }
    }

    // --- Calls ---

    /// `Runtime.evaluate` of `expression` in the Tandem world of `frame` (None is the main frame),
    /// by the context's unique id; `done` gets the value or the error.
    fn browser_evaluate(&mut self, pane: PaneId, frame: Option<u64>, expression: String, done: JsDone) {
        self.browser_evaluate_in(pane, frame, expression, done, false);
    }

    /// `allow_setup` is the isolation check's own call, the only one that may run before the world is Ready.
    fn browser_evaluate_in(&mut self, pane: PaneId, frame: Option<u64>, expression: String, done: JsDone, allow_setup: bool) {
        let target = match self.browser_views.get(&pane) {
            None => Err("no view"),
            Some(bv) if bv.view.is_none() => Err("no view"),
            Some(bv) if bv.client_world != ClientWorld::Ready && !(allow_setup && bv.world.setup.is_some()) => Err("isolated world unavailable"),
            Some(bv) => bv.contexts.unique_for(frame).ok_or("no context"),
        };
        let (session, unique) = match target {
            Ok(t) => t,
            Err(why) => {
                log_js_failure(why);
                return done(self, pane, Err(why.into()));
            }
        };
        let params = browser_world::evaluate_params(&unique, &expression);
        self.devtools_send(pane, session.as_deref(), "Runtime.evaluate", &params, Box::new(move |app, pane, r| {
            let r = r.and_then(|v| browser_world::evaluate_result(&v.to_string()));
            if let Err(e) = &r {
                log_js_failure(e);
            }
            done(app, pane, r)
        }));
    }

    /// `callAsyncJavaScript` in the Tandem world of the main frame. `Err("isolated world unavailable")`
    /// at once when the view has none. The script and the arguments are never logged.
    pub(super) fn browser_js_call(&mut self, pane: PaneId, body: &str, args: Value, done: JsDone) {
        let Some(done) = self.seam_intercept(pane, super::browser_test_seam::Runner::World, body, done) else { return };
        self.browser_js_call_in_frame(pane, None, body, args, done);
    }

    pub(super) fn browser_js_call_in_frame(&mut self, pane: PaneId, frame: Option<u64>, body: &str, args: Value, done: JsDone) {
        match browser_js::call_js(body, &args) {
            Ok(expr) => self.browser_evaluate(pane, frame, expr, done),
            Err(e) => {
                log_js_failure(&e);
                done(self, pane, Err(e))
            }
        }
    }

    /// `safelyEvaluateJavaScript` in the PAGE world: `make_safe`, then `ExecuteScript`; the answer is
    /// page-controlled data and never decides a capability.
    pub(super) fn browser_eval(&mut self, pane: PaneId, script: &str, done: JsDone) {
        let Some(bv) = self.browser_views.get_mut(&pane) else { return done(self, pane, Err("no view".into())) };
        let Some(view) = &bv.view else { return done(self, pane, Err("no view".into())) };
        let token = bv.next_token;
        bv.next_token += 1;
        bv.eval_waiting.insert(token);
        bv.pending_js.insert(token, done);
        view.execute_script(&browser_js::make_safe(script), token);
    }

    /// The same in the Tandem world of one frame (never the page world).
    pub(super) fn browser_eval_in_frame(&mut self, pane: PaneId, frame: Option<u64>, script: &str, done: JsDone) {
        self.browser_evaluate(pane, frame, browser_js::make_safe(script), done);
    }

    /// The page-world runner of later plans: no bridge, no secret, no argument.
    pub(super) fn browser_page_call(&mut self, pane: PaneId, body: &str, args: Value, done: JsDone) {
        let Some(done) = self.seam_intercept(pane, super::browser_test_seam::Runner::Page, body, done) else { return };
        match browser_js::page_call_script(body, &args) {
            Ok(script) => self.browser_eval(pane, &script, done),
            Err(e) => done(self, pane, Err(e)),
        }
    }

    /// Runs `expression` in every live Tandem context of the view (a table a script reads changes
    /// without a reload).
    pub(super) fn browser_world_update(&mut self, pane: PaneId, expression: &str) {
        let frames: Vec<Option<u64>> = match self.browser_views.get(&pane) {
            Some(bv) => std::iter::once(None).chain(bv.frames.iter().copied().map(Some)).collect(),
            None => return,
        };
        for f in frames {
            self.browser_evaluate(pane, f, expression.to_string(), Box::new(|_, _, _| {}));
        }
    }

    /// An answer to a script or DevTools call that Tandem made: resolves its callback. A page-world
    /// answer is parsed as JSON (it is page-controlled); an answer over `MAX_RESULT` is an error.
    pub(super) fn browser_script_done(&mut self, pane: PaneId, token: u64, json: &Result<String, String>) -> bool {
        let Some(bv) = self.browser_views.get_mut(&pane) else { return false };
        let page = bv.eval_waiting.remove(&token);
        bv.world.setup_calls.remove(&token);
        let Some(done) = bv.pending_js.remove(&token) else { return page };
        let result = match json {
            Err(e) => Err(e.clone()),
            Ok(text) if text.len() > MAX_RESULT => Err("the result is too large".into()),
            Ok(text) if text.is_empty() => Ok(Value::Null),
            Ok(text) => serde_json::from_str::<Value>(text).map_err(|_| "the answer is not JSON".to_string()),
        };
        done(self, pane, result);
        true
    }

    /// Whether `token` is a call Tandem made on this view.
    pub(super) fn browser_owns_token(&self, pane: PaneId, token: u64) -> bool {
        self.browser_views.get(&pane).is_some_and(|b| b.pending_js.contains_key(&token) || b.eval_waiting.contains(&token))
    }

    // --- Events ---

    /// The scripts' events of one browser view; true when the event was one of them.
    pub(super) fn browser_script_event(&mut self, pane: PaneId, e: &crate::webhost::Event) -> bool {
        use crate::webhost::Event;
        match e {
            Event::Message { json, source, .. } => {
                if let Some(r) = browser_js::route(json, World::Page, None, source) {
                    self.browser_message(pane, r);
                }
                true
            }
            Event::FrameMessage { frame, json, source, .. } => {
                if let Some(r) = browser_js::route(json, World::Page, Some(*frame), source) {
                    self.browser_message(pane, r);
                }
                true
            }
            Event::DevTools { session, name, json, .. } => {
                // The lifecycle the host held first, so a context is known before its call.
                self.browser_devtools_held(pane);
                self.browser_devtools_event(pane, session.as_deref(), name, json);
                true
            }
            Event::DevToolsHeld { .. } => {
                self.browser_devtools_held(pane);
                true
            }
            Event::FrameCreated { frame, .. } => {
                if let Some(bv) = self.browser_views.get_mut(&pane) {
                    if bv.page_frames.len() < MAX_PAGE_FRAMES && !bv.page_frames.contains(frame) {
                        bv.page_frames.push(*frame);
                    }
                }
                true
            }
            Event::FrameGone { frame, .. } => {
                if let Some(bv) = self.browser_views.get_mut(&pane) {
                    bv.page_frames.retain(|f| f != frame);
                }
                true
            }
            // The world's own check navigation: its id is the one its completion must carry.
            Event::Starting { url, nav, redirect: false, .. } if self.browser_views.get(&pane).is_some_and(|b| b.world.reload_wait && b.world.reload_nav.is_none()) && url == "about:blank" => {
                if let Some(bv) = self.browser_views.get_mut(&pane) {
                    bv.world.reload_nav = Some(*nav);
                }
                true
            }
            // A main-frame navigation began: every call still waiting is over (iTerm2's `willNavigate`).
            Event::Starting { redirect: false, .. } => {
                if let Some(bv) = self.browser_views.get_mut(&pane) {
                    // iTerm2's `willNavigate` and the editing reset (`GDH:16-18`, `BM:1258`).
                    bv.editing = false;
                    bv.selection = None;
                    bv.selection_gate.reset();
                    bv.graph.will_navigate();
                }
                // `RMM:216-221`, `BM:1252`: a new page has no reader and no cached text.
                self.reader_navigation_started(pane);
                // `BM:1254`: a navigation that starts turns copy mode off (the page's cursor is gone).
                self.browser_copy_mode_set(pane, false);
                // The rows Find Globally holds for this pane are the old page's text.
                self.global_pane_gone(pane);
                let dones: Vec<JsDone> = match self.browser_views.get_mut(&pane) {
                    // The setup's own calls and checks are not a page's: they are kept.
                    Some(bv) if bv.world.setup.is_none() => {
                        bv.eval_waiting.clear();
                        // The setup's own calls that are still out (a timed-out setup) stay: their
                        // late answers are removed by `late_removal`.
                        bv.world.take_navigated(&mut bv.pending_js)
                    }
                    _ => Vec::new(),
                };
                for d in dones {
                    d(self, pane, Err("navigated".into()));
                }
                // The link under the pointer belongs to the page that is going.
                self.browser_tip_clear(pane);
                false
            }
            Event::ScriptDone { token, json, .. } if self.browser_owns_token(pane, *token) => self.browser_script_done(pane, *token, json),
            _ => false,
        }
    }

    /// The router: every message of a view, with its world. The set of handlers is closed (each later
    /// plan adds its arm; an arm for a Client handler ignores a Page message). Anything else is
    /// counted in the log, never its text.
    pub(super) fn browser_message(&mut self, pane: PaneId, r: Routed) {
        match r.handler.as_str() {
            _ if browser_js::local_page_ok(&r) => {
                // The page's own words are rebuilt from the routed parts, never forwarded.
                let json = json!({"handler": r.handler, "body": r.body}).to_string();
                self.local_page_message(pane, &r.source, &json);
            }
            h if h == crate::browser_frames::HANDLER => self.browser_graph_message(pane, &r),
            h if h == crate::browser_find::HANDLER => self.browser_find_message(pane, &r),
            h if h == crate::browser_copymode::KEY_HANDLER => self.browser_copy_mode_key(pane, &r),
            h if h == crate::browser_reader::HANDLER => self.browser_reader_message(pane, &r),
            _ if browser_js::always_on(&r, self.browser_view_secret(pane).unwrap_or(""), self.browser_views.get(&pane).map_or("", |b| b.dom_secret.as_str())).is_some() => self.browser_always_on(pane, &r),
            _ => {
                if let Some(bv) = self.browser_views.get_mut(&pane) {
                    if bv.world.drop_logged.is_none_or(|t| t.elapsed() >= Duration::from_secs(1)) {
                        bv.world.drop_logged = Some(Instant::now());
                        let world: &'static str = if r.world == World::Client { "client" } else { "page" };
                        crate::rlog!("browser message dropped: {} world", world);
                    }
                }
            }
        }
    }

    /// The console log, the selection monitor, the gesture script and the editing detector. The
    /// decision (world, secret, shape) is `browser_js::always_on`; this applies it.
    fn browser_always_on(&mut self, pane: PaneId, r: &Routed) {
        let open = self.client_open(pane);
        let copy_on = self.globals.bool("CopySelection");
        let Some(bv) = self.browser_views.get_mut(&pane) else { return };
        let Some(what) = browser_js::always_on(r, &bv.secret, &bv.dom_secret) else { return };
        // The console line is a count only and carries no secret; every other handler is the isolated
        // world's and acts only when that world was proven (`client_open`).
        if !open && !matches!(what, browser_js::AlwaysOn::Console { .. }) {
            return;
        }
        let mut copy = None;
        let mut hover = None;
        let mut pointer = None;
        let mut broadcast = None;
        match what {
            browser_js::AlwaysOn::Console { level, size } => {
                // iTerm2 logs the text (`BM:1066-1087`); a page's words are never logged here (B6): a
                // byte count and the level, at most once a second, the debug level in debug builds only.
                if (level != "debug" || cfg!(debug_assertions)) && bv.world.console_logged.is_none_or(|t| t.elapsed() >= Duration::from_secs(1)) {
                    bv.world.console_logged = Some(Instant::now());
                    crate::rlog!("browser console: {} bytes, level {}", size, level);
                }
            }
            browser_js::AlwaysOn::Gesture(true) => bv.selection_gate.down(Instant::now()),
            browser_js::AlwaysOn::Gesture(false) => copy = bv.selection_gate.up(Instant::now(), copy_on),
            browser_js::AlwaysOn::Editing(e) => bv.editing = e,
            browser_js::AlwaysOn::Hover(h) => hover = Some(h),
            browser_js::AlwaysOn::Pointer(p) => pointer = Some(p),
            browser_js::AlwaysOn::Broadcast(e) => broadcast = Some(e),
            browser_js::AlwaysOn::Selection(text) => {
                bv.selection = Some(browser_js::store_selection(&text));
                copy = bv.selection_gate.accept(&text, Instant::now(), copy_on);
            }
        }
        if let Some(text) = copy {
            self.set_clipboard_text(text.clone());
            self.remember_paste(&text);
        }
        if let Some(h) = hover {
            self.browser_hover(pane, h);
        }
        if let Some(p) = pointer {
            self.browser_pointer(pane, p);
        }
        // Only a pane that is a broadcast source now, with no replay in flight to it, sends.
        if let Some(e) = broadcast {
            if self.browser_broadcast_accepts(pane, &e) {
                self.browser_broadcast(pane, e);
            }
        }
    }

    // --- The one funnel for a URL a page supplied ---

    /// The only function that turns URL text a page, page text or an arrangement supplied into an
    /// open: `nav_gate` first (the network rule, then the scheme), then the pane, a new tab or window,
    /// a split, Quick Look, or `guard_open`'s run; a refusal is a banner.
    pub(super) fn browser_page_open(&mut self, pane: PaneId, raw: &str, how: PageOpen, event_loop: &ActiveEventLoop) {
        use crate::browser::Style;
        match browser_js::nav_gate(raw) {
            NavGate::Navigate(url) => match how {
                PageOpen::Here => self.browser_navigate(pane, &url),
                PageOpen::Tab => self.open_url_in_browser(&url, Style::Tab, Some(pane)),
                PageOpen::Window => self.open_url_in_browser(&url, Style::Window, Some(pane)),
                PageOpen::Split(Axis::Row) => self.open_url_in_browser(&url, Style::Vertical, Some(pane)),
                PageOpen::Split(_) => self.open_url_in_browser(&url, Style::Horizontal, Some(pane)),
                PageOpen::QuickLook(at) => self.quick_look_url(&url, at, event_loop),
            },
            NavGate::Open(run) => self.execute_with(pane, run, None, super::semantic_ui::Exec { background: false, from_file: false, event_loop: Some(event_loop) }),
            NavGate::Blocked(why) => {
                self.banners.set("action", why);
                self.redraw();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prod() -> &'static str {
        let src = include_str!("browser_scripts_app.rs");
        &src[..src.find("#[cfg(test)]").unwrap()]
    }

    /// Q3 and B6: a failed call logs one line with its kind and nothing else: not the body, not an
    /// argument, not the error's own text.
    #[test]
    fn js_calls_are_never_logged() {
        log_js_failure("Error: SECRET_MARKER_1 thrown by a page");
        let ring = crate::dlog::ring_text();
        assert!(!ring.contains("SECRET_MARKER_1"), "the marker reached the log");
        assert!(ring.contains("browser script call failed: exception") || ring.contains("browser script call failed"), "the kind is logged");
        assert_eq!(failure_kind("isolated world unavailable"), "unavailable");
        assert_eq!(failure_kind("no context"), "no context");
        assert_eq!(failure_kind("navigated"), "navigated");
        assert_eq!(failure_kind("anything the page said"), "exception");
        // The code that fails a call never puts a body, an argument or an expression in a log line.
        let p = prod();
        for line in p.lines().filter(|l| l.contains("rlog!") || l.contains("dlog!")) {
            for bad in ["body", "expression", "args", "params", "json"] {
                assert!(!line.contains(bad), "a log line names {bad}: {line}");
            }
        }
    }

    /// The page-world runner takes no argument and no bridge, and the host makes the world answer.
    #[test]
    fn page_call_runs_without_a_bridge() {
        let p = prod();
        let f = &p[p.find("pub(super) fn browser_page_call").unwrap()..p.find("pub(super) fn browser_world_update").unwrap()];
        assert!(f.contains("page_call_script(body, &args)") && f.contains("self.browser_eval(pane, &script, done)"));
        assert!(!f.contains("secret") && !f.contains("wrap("), "no secret and no bridge");
        let e = &p[p.find("pub(super) fn browser_eval(").unwrap()..p.find("pub(super) fn browser_eval_in_frame").unwrap()];
        assert!(e.contains("make_safe(script)") && e.contains("execute_script("), "the page world, made safe");
        let w = &p[p.find("pub(super) fn browser_eval_in_frame").unwrap()..p.find("pub(super) fn browser_page_call").unwrap()];
        assert!(w.contains("browser_evaluate(") && !w.contains("execute_script"), "a frame's eval is the Tandem world's, never the page's");
    }

    /// A world call never runs without a ready world, and every call to the page's frames is the
    /// Tandem world's by unique context id.
    #[test]
    fn calls_need_a_ready_world() {
        let p = prod();
        let f = &p[p.find("fn browser_evaluate(").unwrap()..p.find("pub(super) fn browser_js_call(").unwrap()];
        assert!(f.contains("\"isolated world unavailable\"") && f.contains("unique_for(frame)") && f.contains("evaluate_params(&unique"));
        assert!(!f.contains("execute_script") && !f.contains("context_for"), "never by numeric id, never the page world");
        // One DevTools sender, which asks the allow-list.
        assert_eq!(p.matches("v.devtools_call(").count(), 1);
        assert!(p.contains("browser_world::method_allowed(method)"));
    }

    /// Messages: the world comes from the transport. The page's messages are routed as Page, the
    /// binding's as Client, and the one local-page arm rebuilds the JSON from the routed parts.
    #[test]
    fn the_world_comes_from_the_transport() {
        let p = prod();
        assert!(p.contains("route(json, World::Page, None, source)") && p.contains("route(json, World::Page, Some(*frame), source)"));
        assert_eq!(p.matches("World::Client, frame, &origin").count(), 1, "one Client route: the binding");
        let b = &p[p.find("pub(super) fn browser_message").unwrap()..p.find("pub(super) fn browser_page_open").unwrap()];
        assert!(b.contains("local_page_ok(&r)") && b.contains("json!({\"handler\": r.handler, \"body\": r.body})"));
        assert!(!b.contains("serde_json::from_str"), "the router never re-reads a page's JSON");
        // The binding is accepted only through the context table.
        let d = &p[p.find("\"Runtime.bindingCalled\" =>").unwrap()..p.find("_ => {}\n        }\n    }\n\n    /// The frames with a Tandem world changed").unwrap()];
        assert!(d.contains("contexts.binding_called(session, json)"));
    }

    /// The funnel asks the gate first and opens nothing else; every other page-derived open of Task 1
    /// goes through it or `nav_gate_tab`.
    #[test]
    fn page_opens_go_through_the_gate() {
        let p = prod();
        let f = &p[p.find("pub(super) fn browser_page_open").unwrap()..];
        let f = &f[..f.find("\n    }\n").unwrap()];
        assert!(f.starts_with("pub(super) fn browser_page_open") && f.find("nav_gate(raw)").unwrap() < f.find("execute_with").unwrap());
        assert!(f.contains("NavGate::Blocked(why)") && f.contains("quick_look_url(&url, at, event_loop)"));
        let app = include_str!("browser_app.rs");
        let app = &app[..app.find("#[cfg(test)]").unwrap()];
        let nw = &app[app.find("Event::NewWindow { url, .. } =>").unwrap()..app.find("Event::ExternalUri").unwrap()];
        assert!(nw.contains("nav_gate_tab(url)"), "a page's window.open passes the tab gate");
    }

    /// Every source file of the app folder, with its test module cut off.
    fn app_sources() -> Vec<(String, String)> {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src").join("app");
        let mut files: Vec<(String, String)> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().is_some_and(|x| x == "rs"))
            .map(|e| (format!("app/{}", e.file_name().to_string_lossy()), std::fs::read_to_string(e.path()).unwrap()))
            .collect();
        files.sort();
        files
    }

    /// Task 3 review minor 2: a function of any file in src/app that takes a routed message is the
    /// router or is a listed Client handler that asks `client_open` before it acts.
    fn client_handler_violations(files: &[(String, String)]) -> Vec<String> {
        let known = ["browser_graph_message", "browser_always_on", "browser_find_message", "browser_copy_mode_key", "browser_reader_message"];
        let routers = ["browser_message", "local_page_message", "browser_page_message"];
        let mut out = Vec::new();
        for (file, src) in files {
            let prod = &src[..src.find("#[cfg(test)]").unwrap_or(src.len())];
            let mut at = 0;
            while let Some(i) = prod[at..].find("fn ") {
                let start = at + i;
                at = start + 3;
                let sig_end = prod[start..].find('{').map_or(prod.len(), |e| start + e);
                let sig = &prod[start..sig_end];
                // The type `Routed` as a word in the parameters (a function that only returns another
                // module's `Routed`, as frames_ui's does, takes no message).
                let params = sig.split("->").next().unwrap_or(sig);
                let routed = params.match_indices("Routed").any(|(i, m)| {
                    let word = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_');
                    !word(params[..i].chars().next_back()) && !word(params[i + m.len()..].chars().next())
                });
                if !routed {
                    continue;
                }
                let name: String = sig[3..].chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
                if routers.contains(&name.as_str()) {
                    continue;
                }
                if !known.contains(&name.as_str()) {
                    out.push(format!("{file}: {name} takes a routed message but is not a listed Client handler"));
                    continue;
                }
                let body_end = prod[sig_end..].find("\n    }\n").map_or(prod.len(), |e| sig_end + e);
                if !prod[sig_end..body_end].contains("self.client_open(") {
                    out.push(format!("{file}: {name} acts without the one gate"));
                }
            }
        }
        out
    }

    /// Task 2 review I3 (a class): every Client-world handler asks the one gate (`client_open`) before
    /// it acts, and a Client script is never given to an auto-attached frame of a view that is not
    /// Ready. A function of the browser files that takes a routed message is listed here or is the
    /// router (a new handler fails this test until it asks the gate).
    #[test]
    fn every_client_handler_asks_the_one_gate() {
        let violations = client_handler_violations(&app_sources());
        assert!(violations.is_empty(), "{}", violations.join("\n"));
        // The scan sees a handler in a file no one listed: a planted one is reported, and a gated one passes.
        let mut files = app_sources();
        files.push(("app/browser_planted_app.rs".into(), "impl App {\n    pub(super) fn hover_tip(&mut self, pane: PaneId, r: &Routed) {\n        let _ = pane;\n    }\n}\n".into()));
        let v = client_handler_violations(&files);
        assert!(v.iter().any(|w| w.contains("browser_planted_app.rs") && w.contains("hover_tip")), "{v:?}");
        let mut files = app_sources();
        files.push(("app/browser_planted_app.rs".into(), "impl App {\n    pub(super) fn hover_tip(&mut self, pane: PaneId, r: &Routed) {\n        if !self.client_open(pane) { return; }\n    }\n}\n".into()));
        assert!(client_handler_violations(&files).iter().any(|w| w.contains("not a listed Client handler")), "an unlisted name still fails until it is listed");
        let p = prod();
        let attached = &p[p.find("fn browser_attached").unwrap()..p.find("fn browser_session_pump").unwrap()];
        assert!(attached.contains("self.client_open(pane)"), "an iframe of a view that is not Ready gets no secret-bearing script");
        let finish = &p[p.find("fn browser_world_finish").unwrap()..p.find("pub(super) fn browser_world_navigated").unwrap()];
        assert!(finish.contains("client_sources.clear()"), "an Unavailable view keeps no Client source for later frames");
    }

    /// Every drop and remake of a view makes its world new (B4): each function that takes a view
    /// resets the world first or in the same function.
    #[test]
    fn every_view_drop_site_resets_the_world() {
        for (file, src) in [("browser_app.rs", include_str!("browser_app.rs")), ("adblock_app.rs", include_str!("adblock_app.rs"))] {
            let prod = &src[..src.find("#[cfg(test)]").unwrap_or(src.len())];
            let mut at = 0;
            while let Some(i) = prod[at..].find("view.take()") {
                let pos = at + i;
                let start = prod[..pos].rfind("\n    fn ").or_else(|| prod[..pos].rfind("\n    pub(super) fn ")).unwrap();
                let end = prod[pos..].find("\n    }\n").map_or(prod.len(), |e| pos + e);
                let body = &prod[start..end];
                assert!(body.contains("browser_reset_world("), "{file}: a function that takes a view does not reset its world:\n{body}");
                at = pos + 1;
            }
        }
        let app = include_str!("browser_app.rs");
        let app = &app[..app.find("#[cfg(test)]").unwrap()];
        let close = &app[app.find("pub(super) fn close_browser_view").unwrap()..];
        assert!(close[..close.find("\n    }\n").unwrap()].contains("browser_reset_world("));
        let show = &app[app.find("fn show_browser_view").unwrap()..];
        let show = &show[..show.find("\n    }\n").unwrap()];
        assert!(show.find("browser_reset_world(").unwrap() < show.find("WebView::create(").unwrap(), "a new view starts with a new world");
        assert!(show.find("browser_world_start(").unwrap() > show.find("WebView::create(").unwrap());
        assert!(!show.contains("navigate_tandem"), "the pending URL loads when the world's setup is over");
    }

    /// Final review minor 10: a view whose world ends Unavailable gets new secrets, so a value that
    /// may have reached a script whose isolation was never proven (deferred minor 193) is worthless.
    #[test]
    fn an_unavailable_world_rotates_its_secrets() {
        let p = prod();
        let f = &p[p.find("fn browser_world_finish").unwrap()..p.find("pub(super) fn browser_world_navigated").unwrap()];
        let un = &f[f.find("if unavailable {").unwrap()..];
        let un = &un[..un.find("\n        }\n").unwrap()];
        assert!(un.contains("bv.secret = browser_js::new_secret();") && un.contains("bv.dom_secret = browser_js::new_secret();"), "{un}");
    }

    /// Review I1, the wiring: a load that follows a check arms the reset and the first real
    /// `Navigated` sends it.
    #[test]
    fn the_history_reset_is_wired() {
        let p = prod();
        let f = &p[p.find("fn browser_world_finish").unwrap()..p.find("pub(super) fn browser_world_navigated").unwrap()];
        assert!(f.contains("self.browser_load_own(pane, &url)"));
        let restore = include_str!("browser_restore_app.rs");
        let own = &restore[restore.find("pub(super) fn browser_load_own").unwrap()..restore.find("pub(super) fn browser_nav_starting").unwrap()];
        assert!(own.contains("world.history.arm()"));
        let app = include_str!("browser_app.rs");
        let app = &app[..app.find("#[cfg(test)]").unwrap()];
        let nav = &app[app.find("Event::Navigated { ok, .. } => {").unwrap()..app.find("Event::History { back, forward, .. } =>").unwrap()];
        assert!(nav.contains("world.history.loaded()") && nav.contains("browser_world::RESET_HISTORY"));
    }

    /// Review I2: a navigation that arrives while the world is being set up is kept (the latest
    /// wins) and runs when the setup is over; nothing loads before the world exists.
    #[test]
    fn navigation_during_setup_is_deferred() {
        let mut w = WorldState::default();
        assert!(!w.defer_load("https://a.test/"), "no setup: the caller navigates");
        w.setup = Some(Setup::new(&[]));
        w.load_after = Some("https://old.test/".into());
        assert!(w.defer_load("https://typed.test/"));
        assert!(w.defer_load("about:blank"), "a typed about:blank is kept too, never taken for the check");
        assert_eq!(w.load_after.as_deref(), Some("about:blank"));
        let p = prod();
        let app = include_str!("browser_app.rs");
        let app = &app[..app.find("#[cfg(test)]").unwrap()];
        let nav = &app[app.find("fn browser_navigate").unwrap()..app.find("pub(super) fn browser_suggest").unwrap()];
        assert!(nav.contains("defer_load(") && nav.find("defer_load(").unwrap() < nav.find("navigate_tandem").unwrap());
        let btn = &app[app.find("fn browser_button").unwrap()..];
        let btn = &btn[..btn.find("Button::Menu =>").unwrap()];
        assert!(btn.contains("world.setup.is_some()"), "back, forward and reload wait for the world too");
        assert!(p.contains("pub(super) fn defer_load"));
    }

    /// Review I3: the setup's own calls survive the navigation that starts the page load, and the
    /// timer starts when the controller exists.
    #[test]
    fn setup_calls_survive_navigation_and_the_timer_waits_for_the_controller() {
        let mut w = WorldState::default();
        let mut pending: HashMap<u64, JsDone> = HashMap::new();
        for t in [1u64, 2, 3] {
            pending.insert(t, Box::new(|_, _, _| {}));
        }
        w.setup_calls.insert(2);
        let dropped = w.take_navigated(&mut pending);
        assert_eq!(dropped.len(), 2, "the page's calls end with the navigation");
        assert!(pending.contains_key(&2) && pending.len() == 1, "the setup's call is kept");
        assert!(!w.timed_out(Instant::now() + Duration::from_secs(60)), "no setup: nothing to time out");
        w.setup = Some(Setup::new(&[]));
        assert!(!w.timed_out(Instant::now() + Duration::from_secs(60)), "the timer has not started: the controller does not exist yet");
        let t0 = Instant::now();
        w.controller_ready(t0);
        w.controller_ready(t0 + Duration::from_secs(4));
        assert!(!w.timed_out(t0 + Duration::from_secs(4)));
        assert!(w.timed_out(t0 + SETUP_TIMEOUT + Duration::from_millis(1)), "from the first Created, not the second");
        let p = prod();
        let ans = &p[p.find("fn browser_world_answered").unwrap()..p.find("fn browser_world_finish").unwrap()];
        assert!(ans.contains("late_removal("), "a late answer is checked first");
        let app = include_str!("browser_app.rs");
        let app = &app[..app.find("#[cfg(test)]").unwrap()];
        assert!(app[app.find("Event::Created(_) =>").unwrap()..].contains("controller_ready("));
        assert!(!p[p.find("fn browser_world_start").unwrap()..p.find("fn browser_world_pump").unwrap()].contains("setup_started = Some") && p.contains("controller_up.then(Instant::now)"));
    }
}
