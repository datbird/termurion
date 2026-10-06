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
use crate::webhost_win::{self, Env};

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
    /// This run's Dev Null folder name (made on first use).
    dev_null: Option<PathBuf>,
    swept: bool,
}

impl Envs {
    /// The state and the receiving end of everything views post.
    pub(super) fn new() -> (Envs, Receiver<Posted>) {
        let (tx, rx) = channel();
        (Envs { tx, slots: HashMap::new(), runtime: RefCell::new(None), fixed: RefCell::new(None), refusal: RefCell::new(None), next_view: 1, seen: (String::new(), String::new()), dev_null: None, swept: false }, rx)
    }

    /// What a view posts through.
    pub(super) fn sink(&self) -> Sink {
        self.tx.clone()
    }
}

/// What an environment of `store` is created with: nothing beyond the browser stores' own scheme
/// (Task 8 adds the proxy argument; `args` is exactly `proxy_args` or empty).
pub(super) fn env_options(store: Store) -> EnvOptions {
    match store {
        Store::Default => EnvOptions::default(),
        Store::Browser | Store::BrowserDevNull => EnvOptions { args: String::new(), custom_schemes: vec!["iterm2-about".into()] },
    }
}

/// The runtime now: the Evergreen runtime, or the one `BrowserPluginPathHint` names after its
/// checks. A refused hint is Missing (no silent fall back) with the reason.
fn runtime_now(hint: &str) -> (Runtime, Option<PathBuf>, Option<String>) {
    if hint.trim().is_empty() {
        return (webhost::runtime_outcome(webhost_win::runtime_version(None)), None, None);
    }
    match webhost_win::fixed_runtime_check(hint) {
        Ok(folder) => (webhost::runtime_outcome(webhost_win::runtime_version(Some(&folder))), Some(folder), None),
        Err(why) => (Runtime::Missing, None, Some(why)),
    }
}

impl App {
    /// The WebView2 Runtime, cached for 30 seconds (iTerm2's gateway cache).
    pub(super) fn web_runtime(&self) -> Runtime {
        if let Some((at, r)) = self.web_envs.runtime.borrow().as_ref() {
            if at.elapsed() < RUNTIME_TTL {
                return r.clone();
            }
        }
        let (r, fixed, refusal) = runtime_now(&self.globals.string("BrowserPluginPathHint"));
        let changed = self.web_envs.runtime.borrow().as_ref().is_none_or(|(_, old)| *old != r);
        if changed {
            match &r {
                Runtime::Found(v) => crate::host::log(&format!("webhost: WebView2 Runtime {v}")),
                Runtime::Missing => crate::host::log("webhost: no WebView2 Runtime (not installed)"),
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
        let base = webhost_win::local_appdata()?;
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
                webhost_win::cleanup_files();
            }
            slot.creating.set(true);
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
                        &env_options(store),
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
        for _ in 0..DRAIN_PER_TICK {
            let Ok(posted) = self.web_events.try_recv() else { break };
            if let Some((store, result)) = &posted.env {
                match result {
                    Ok(()) => {
                        crate::host::log(&format!("webhost: environment {store:?} ready"));
                        // Views waiting for their environment are made on the next frame.
                        self.redraw();
                    }
                    Err(e) => crate::host::log(&format!("webhost: environment {store:?} failed: {e}")),
                }
            }
            if let Some(event) = &posted.event {
                self.web_route(event, event_loop);
            }
        }
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
        if !crate::webhost::held_focus_lost(hwnd, webhost_win::foreground_root()) {
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

    /// Tells the 13u keyboard hook which window's web view holds the keyboard, so it passes keys on
    /// instead of running Tandem's Win-chord commands on the terminal (security: Win+V must paste
    /// into the page, not the shell).
    pub(super) fn sync_web_keyboard(&self) {
        crate::winkeys::set_web_focus(self.web_focus_held.and_then(|w| self.window_hwnd(w)).unwrap_or(0));
    }

    /// Whether `pane` is a browser pane. No pane is one until 13x Task 5 adds `Io::Browser`.
    #[allow(dead_code)]
    pub(super) fn is_browser_pane(&self, _pane: crate::layout::PaneId) -> bool {
        false
    }

    /// The active session of the key window changed from `prev` to `next` (`PTYTab.m:955-960`): a
    /// terminal pane that hands focus to a browser pane is told it lost focus and one that takes it
    /// back is told it gained it, when its program asked for DECSET 1004. Task 5 calls this.
    #[allow(dead_code)]
    pub(super) fn browser_focus_moved(&mut self, prev: Option<crate::layout::PaneId>, next: Option<crate::layout::PaneId>) {
        let tag = |p: Option<crate::layout::PaneId>| p.map(|p| (p, self.is_browser_pane(p)));
        for (pane, gained) in crate::webhost::pane_focus_reports(tag(prev), tag(next)) {
            if let Some(p) = self.panes.get_mut(&pane) {
                if p.screen.term().mode().contains(alacritty_terminal::term::TermMode::FOCUS_IN_OUT) {
                    p.focus_report(gained);
                }
            }
        }
    }

    /// One view event, by view id. No owner exists yet: log what is ours, drop the rest.
    fn web_route(&mut self, event: &Event, _event_loop: &ActiveEventLoop) {
        if self.tool_view_event(event) && !matches!(event, Event::Created(_) | Event::Failed(..) | Event::Focus(..)) {
            return;
        }
        if self.web_popover_view_event(event) && !matches!(event, Event::Created(_) | Event::Failed(..) | Event::Focus(..)) {
            return;
        }
        match event {
            Event::Created(v) => crate::host::log(&format!("webhost: view {v} created")),
            // The text is Tandem's own (an error from COM), never the page's.
            Event::Failed(v, why) => crate::host::log(&format!("webhost: view {v} failed: {why}")),
            // A view lost focus: another window may now be in front of a window that held it.
            Event::Focus(_, false) => self.web_focus_recheck(),
            _ => {}
        }
    }

    /// A setting the host reads changed (`WebUserAgent`, `BrowserPluginPathHint`): the runtime is
    /// checked again on the next request. A new fixed-runtime folder takes effect for environments
    /// made after it; one that already exists keeps its runtime until Tandem restarts.
    pub(super) fn web_settings_changed(&mut self) {
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
        assert_eq!(env_options(Store::Default), EnvOptions::default());
        for s in [Store::Browser, Store::BrowserDevNull] {
            let o = env_options(s);
            assert_eq!(o.custom_schemes, vec!["iterm2-about".to_string()]);
            assert_eq!(o.args, "", "no Chromium switch is composed from anything but the proxy (Task 8)");
        }
    }

    #[test]
    fn the_runtime_cache_is_thirty_seconds() {
        assert_eq!(RUNTIME_TTL, Duration::from_secs(30));
    }

    /// The host's App state is app-wide: it is not a per-window field (`swap_live`).
    #[test]
    fn web_state_is_not_swapped_per_window() {
        let swap = include_str!("windows_ui.rs");
        for name in ["web_envs", "web_events", "web_focus_held"] {
            assert!(!swap.contains(name), "{name} must not be per window");
        }
    }
}
