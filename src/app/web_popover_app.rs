//! Web popovers on the App (milestone 13x, Task 3): a status bar component's HTML popover and URL
//! Quick Look. One popover at a time, app-wide (it goes with its pane, or when it loses the
//! foreground). The window is a popup that is never registered as a terminal window; its view is
//! made when the Default environment exists. What a page posts is judged by `web_popover`'s closed
//! set and only from the popover's own document (P6); every COM callback is the host's, behind its
//! unwind guard.

use std::time::{Duration, Instant};

use super::*;
use crate::api_vars::Values;
use crate::expr::{Failure, Value};
use crate::script_eval::Timeout;
use crate::web_popover::{self, Act, Kind, WebPopover};
use crate::webhost::{self, Event, Runtime, Store, ViewId};
use crate::webhost_win::WebView;

/// How long a popover waits for its environment before it gives up.
const VIEW_WAIT: Duration = Duration::from_secs(15);

/// What an answered invocation does for the page (`WVW:237-268`, `BASE:433-436`).
#[derive(Debug, PartialEq)]
pub(super) enum BridgeAnswer {
    /// Run this in the page.
    Script(String),
    /// Log this to the connection that hosts the function.
    Log(String),
}

/// A success runs the callback with the value as JSON; a failure, or a missing callback, is logged.
pub(super) fn bridge_answer(callback: &str, r: &Result<Value, Failure>) -> BridgeAnswer {
    match r {
        Err(f) => BridgeAnswer::Log(f.reason.clone()),
        Ok(_) if callback.is_empty() => BridgeAnswer::Log("missing callback".to_string()),
        Ok(v) => BridgeAnswer::Script(webhost::return_script(callback, &crate::expr::json_text(v))),
    }
}

/// What an external protocol a popover's page asked for does (B4).
#[derive(Debug, PartialEq)]
pub(super) enum ExternalRun {
    Nothing,
    /// A web or mail link: opened by the system.
    Open(String),
    /// Refused: the text to show.
    Notice(String),
}

/// The view always cancels an external protocol; the app opens it only for a person's gesture and
/// only through `semantic::guard_open` (http, https, mailto, ftp).
pub(super) fn external_uri_run(url: &str, user: bool) -> ExternalRun {
    if !user {
        return ExternalRun::Nothing;
    }
    match crate::semantic::guard_open(url, false) {
        crate::semantic::Run::Open(t) => ExternalRun::Open(t),
        crate::semantic::Run::Blocked(why) => ExternalRun::Notice(why),
        _ => ExternalRun::Notice(format!("Tandem does not open {}: links.", webhost::scheme_of(url))),
    }
}

impl App {
    /// Whether a status bar component's HTML can be shown in a web popover: the runtime exists and
    /// the HTML is within the bound (else the text popover, with a log line for a large one).
    pub(super) fn web_popover_possible(&self, html: &str) -> bool {
        if self.web_runtime() == Runtime::Missing {
            return false;
        }
        match web_popover::popover_document(html) {
            Ok(_) => true,
            Err(why) => {
                crate::host::log(&format!("webhost: {why}; showing the text popover"));
                false
            }
        }
    }

    /// The popover is anchored in the current window (its session is one of this window's panes).
    fn popover_in_current_window(&self) -> bool {
        self.web_popover.as_ref().and_then(|(p, _)| *p).is_some_and(|pane| self.tabs.iter().any(|t| t.layout.panes().contains(&pane)))
    }

    /// The window was made again: a popover anchored on it closes (its anchor is gone). A Quick
    /// Look is the person's own pointer and stays; its parent is its own window, never this one.
    pub(super) fn popovers_remade(&mut self, _new_hwnd: isize) {
        if self.popover_in_current_window() {
            self.web_popover = None;
        }
    }

    /// The window is closing: a popover anchored on it closes with it.
    pub(super) fn popovers_window_closing(&mut self) {
        if self.popover_in_current_window() {
            self.web_popover = None;
        }
    }

    /// Opens a popover (replacing any open one) of `kind` at `size` points with its bottom
    /// centered on `at` (screen pixels). `pane` is the session whose scope the bridge runs in.
    pub(super) fn open_web_popover(&mut self, pane: Option<PaneId>, kind: Kind, size: (f64, f64), at: (f64, f64), event_loop: &ActiveEventLoop) {
        self.web_popover = None;
        self.text_popover = None;
        let work = self.monitor_rects().map(|(w, _)| w);
        let scale = f64::from(self.scale());
        let Some(g) = self.gpu.as_ref() else { return };
        let size = web_popover::content_size(&kind, size, work, scale, f64::from(g.device.limits().max_texture_dimension_2d));
        match WebPopover::open(event_loop, &g.instance, &g.adapter, &g.device, &g.queue, kind, size, at, work) {
            Ok(p) => self.web_popover = Some((pane, p)),
            Err(e) => self.banners.set("window", format!("Could not open the window: {e}")),
        }
    }

    /// Quick Look of `url` (`ARC:976-994`) at `at` (screen pixels). Without the runtime, or for a
    /// URL Quick Look may not show, the link opens in the default app as before.
    pub(super) fn quick_look_url(&mut self, url: &str, at: (f64, f64), event_loop: &ActiveEventLoop) {
        // Only an http or https URL with a host is ever shown or opened here.
        let Some(url) = web_popover::quick_look_target(url) else { return };
        if self.web_runtime() == Runtime::Missing {
            match external_uri_run(&url, true) {
                ExternalRun::Open(t) => {
                    if let Err(e) = super::semantic_ui::shell_open(&t, false) {
                        self.banners.set("action", format!("Could not open {t}: {e}"));
                    }
                }
                ExternalRun::Notice(n) => self.banners.set("action", n),
                ExternalRun::Nothing => {}
            }
            return;
        }
        let kind = Kind::QuickLook { url, browser: web_popover::default_browser_name() };
        self.open_web_popover(None, kind, web_popover::QUICK_LOOK_SIZE, at, event_loop);
    }

    /// The pointer in screen pixels.
    pub(super) fn pointer_screen(&self) -> (f64, f64) {
        let origin = self.gpu.as_ref().and_then(|g| g.window.inner_position().ok()).unwrap_or_default();
        (f64::from(origin.x) + f64::from(self.cursor.0), f64::from(origin.y) + f64::from(self.cursor.1))
    }

    /// Makes the popover's view once the environment exists; closes the popover that waited too
    /// long, and the one that lost the foreground. Called from `web_tick`.
    pub(super) fn web_popover_tick(&mut self) {
        let Some((_, p)) = self.web_popover.as_mut() else { return };
        if p.lost_foreground() {
            self.web_popover = None;
            return;
        }
        if p.has_view() {
            return;
        }
        if p.age() > VIEW_WAIT {
            crate::host::log("webhost: a popover's web view did not start in time");
            self.banners.set("window", "The WebView2 Runtime did not start the popover.");
            self.web_popover = None;
            return;
        }
        let Some(hwnd) = p.hwnd() else { return };
        let kind = p.kind().clone();
        let bounds = webhost::physical_bounds(p.web_rect());
        let id = self.next_view_id();
        let sink = self.web_envs.sink();
        let setup = match &kind {
            Kind::Html { .. } => web_popover::html_setup(),
            Kind::QuickLook { .. } => web_popover::quick_look_setup(&self.globals.string("WebUserAgent")),
        };
        let Some(env) = self.web_env(Store::Default) else { return };
        let view = WebView::create(env, hwnd, id, bounds, setup, sink);
        // A script's HTML is served by the host's one function on Tandem's own origin, never loaded
        // with NavigateToString; Quick Look is the page the person pointed at.
        let mut served = None;
        match &kind {
            Kind::Html { html } => match web_popover::popover_document(html) {
                Ok(doc) => {
                    view.navigate_tandem(doc.url());
                    served = Some(doc);
                }
                Err(_) => {
                    self.web_popover = None;
                    return;
                }
            },
            Kind::QuickLook { url, .. } => view.navigate(url),
        }
        if let Some((_, p)) = self.web_popover.as_mut() {
            p.attach(view, id);
            if let Some(doc) = served {
                p.set_doc(doc);
            }
        }
        self.redraw();
    }

    /// Events of the popover's window. False for other windows.
    pub(super) fn web_popover_event(&mut self, window: WindowId, event: &WindowEvent) -> bool {
        let Some((_, p)) = self.web_popover.as_mut().filter(|(_, p)| p.id() == window) else { return false };
        match p.event(event) {
            Act::Keep => {}
            Act::Close => self.web_popover = None,
            Act::OpenInBrowser => {
                // Judged again, by the one judge: what the view says now reaches the shell only as
                // http or https (`guard_open`), and only for a person's click.
                if let Some(target) = p.open_in_target().filter(|t| web_popover::quick_look_scheme_ok(t)) {
                    match external_uri_run(&target, true) {
                        ExternalRun::Open(t) => {
                            if let Err(e) = super::semantic_ui::shell_open(&t, false) {
                                self.banners.set("action", format!("Could not open {t}: {e}"));
                            }
                        }
                        ExternalRun::Notice(n) => self.banners.set("action", n),
                        ExternalRun::Nothing => {}
                    }
                }
            }
        }
        true
    }

    /// A popover view's event. True when the view is the popover's (the event is consumed).
    pub(super) fn web_popover_view_event(&mut self, event: &Event) -> bool {
        let view = event.view();
        let Some((_, p)) = self.web_popover.as_mut().filter(|(_, p)| p.view_id() == view && view != 0) else { return false };
        match event {
            Event::Source(_, url) => p.set_current(url),
            // Every request of the view goes to the one serving function: the document, or 403.
            Event::Request { token, url, .. } => p.answer(*token, url),
            Event::Message { json, source, .. } => {
                if p.trusts(source) && p.limiter.allow(Instant::now()) {
                    let json = json.clone();
                    self.web_invoke(view, &json);
                }
            }
            // The one script this module runs is the callback; its failure text is the page's, so only its size is kept.
            Event::ScriptDone { json: Err(e), .. } => crate::host::log(&format!("webhost: popover callback failed ({} bytes of error text)", e.len())),
            Event::Key { vk: 0x1B, .. } => self.web_popover = None,
            Event::Failed(..) => {
                crate::host::log("webhost: a popover's web view failed");
                self.banners.set("window", "The popover's web view could not be shown.");
                self.web_popover = None;
            }
            Event::Focus(_, false) => {
                if p.lost_foreground() {
                    self.web_popover = None;
                }
            }
            // An external protocol, or a link the person clicked that the view refused: both are
            // opened only on a person's gesture and only through `guard_open`.
            Event::ExternalUri { url, user, .. } | Event::Refused { url, user, .. } => match external_uri_run(url, *user) {
                ExternalRun::Open(t) => {
                    if let Err(e) = super::semantic_ui::shell_open(&t, false) {
                        self.banners.set("action", format!("Could not open {t}: {e}"));
                    }
                }
                ExternalRun::Notice(n) => self.banners.set("action", n),
                ExternalRun::Nothing => {}
            },
            _ => {}
        }
        true
    }

    /// The bridge (`WVW:189-215`): a page's `iterm2Invoke` message, already known to come from the
    /// popover's own document. A call runs in the component's session scope with no timeout and
    /// side effects on; the answer goes back through `web_invoke_answered`.
    pub(super) fn web_invoke(&mut self, view: ViewId, json: &str) {
        let Some((pane, p)) = self.web_popover.as_ref().filter(|(_, p)| p.view_id() == view) else { return };
        let pane = *pane;
        match web_popover::bridge_call(matches!(p.kind(), Kind::Html { .. }), json) {
            web_popover::BridgeCall::Ignore => {}
            web_popover::BridgeCall::Missing { reply, log } => {
                if let (Some(js), Some(v)) = (reply, p.view()) {
                    v.execute_script(&js, 1);
                }
                self.script_log(None, log.to_string());
            }
            web_popover::BridgeCall::Invoke { invocation, callback } => {
                let Some(pane) = pane else { return };
                let signature = crate::expr::parse_calls(&invocation).ok().and_then(|c| c.first().map(|c| c.signature())).unwrap_or_default();
                let callback = callback.unwrap_or_default();
                let scope = script_ui::ScopeSpec::Session { pane, frame: Values::new() };
                self.invoke(&invocation, scope, Timeout::Never, true, script_ui::Origin::WebView { view, callback, signature });
            }
        }
    }

    /// An invocation the bridge started finished.
    pub(super) fn web_invoke_answered(&mut self, view: ViewId, callback: &str, signature: &str, r: Result<Value, Failure>) {
        match bridge_answer(callback, &r) {
            BridgeAnswer::Log(text) => {
                let owner = self.api_fns.owner(signature);
                self.script_log(owner, text);
            }
            BridgeAnswer::Script(js) => {
                // The popover may be gone, or another one may have replaced it.
                if let Some(v) = self.web_popover.as_ref().filter(|(_, p)| p.view_id() == view).and_then(|(_, p)| p.view()) {
                    v.execute_script(&js, 1);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expr::Failure;
    use serde_json::json;

    /// `WVW:211-215`, `BASE:433-436`: a success runs the callback in the page; a failure is logged.
    #[test]
    fn bridge_answers_follow_iterm2() {
        let ok: Result<serde_json::Value, Failure> = Ok(json!({"a": 1}));
        assert_eq!(bridge_answer("cb", &ok), BridgeAnswer::Script(crate::webhost::return_script("cb", r#"{"a":1}"#)));
        let err: Result<serde_json::Value, Failure> = Err(Failure::plain("no such function"));
        assert_eq!(bridge_answer("cb", &err), BridgeAnswer::Log("no such function".to_string()));
        // No callback to answer: the value cannot be returned (`WVW:225-231`).
        assert_eq!(bridge_answer("", &ok), BridgeAnswer::Log("missing callback".to_string()));
    }

    /// B4: an external protocol a Quick Look page or redirect asked for is handled by the one guard.
    #[test]
    fn external_uris_go_through_the_guard() {
        assert_eq!(external_uri_run("mailto:a@b.c", true), ExternalRun::Open("mailto:a@b.c".to_string()));
        assert_eq!(external_uri_run("https://a.b/", true), ExternalRun::Open("https://a.b/".to_string()));
        assert_eq!(external_uri_run("mailto:a@b.c", false), ExternalRun::Nothing, "only a person's gesture");
        for bad in ["ms-msdt:id", "search-ms:query=x", "ms-officecmd:x", "file:///C:/Windows/system32/calc.exe", "file://srv/share/x", "ssh://host"] {
            match external_uri_run(bad, true) {
                ExternalRun::Notice(n) => assert!(n.contains("Tandem"), "{bad}: {n}"),
                other => panic!("{bad}: {other:?}"),
            }
        }
    }

    /// Review fix (Task 3, Important 2 and Minor 3): a link a person clicks in a popover is refused
    /// by the view and offered to the app, which opens it only through `guard_open`; "Open in" and
    /// the no-runtime Quick Look fallback ask the same judge.
    #[test]
    fn popover_links_and_open_in_go_through_guard_open() {
        let me = include_str!("web_popover_app.rs");
        let prod = &me[..me.find("#[cfg(test)]\nmod tests").unwrap()];
        let arm = &prod[prod.find("fn web_popover_view_event").unwrap()..];
        let arm = &arm[..arm.find("\n    }\n").unwrap()];
        assert!(arm.contains("Event::Refused"), "the refused click is routed");
        assert!(arm.contains("external_uri_run(url, *user)"));
        let open = &prod[prod.find("fn web_popover_event").unwrap()..];
        let open = &open[..open.find("\n    }\n").unwrap()];
        assert!(open.contains("external_uri_run(") && !open.contains("shell_open(&target"), "Open in asks guard_open");
        let ql = &prod[prod.find("fn quick_look_url").unwrap()..];
        let ql = &ql[..ql.find("\n    }\n").unwrap()];
        assert!(ql.contains("external_uri_run("), "the fallback asks guard_open");
        let host = include_str!("../webhost_win.rs");
        assert_eq!(host.matches("Event::Refused").count(), 1, "one place posts it");
        assert!(host.find("refused_click(").unwrap() < host.find("Event::Refused").unwrap());
        assert_eq!(external_uri_run("https://a.example/x", true), ExternalRun::Open("https://a.example/x".to_string()));
        assert!(matches!(external_uri_run("file:///C:/x.html", true), ExternalRun::Notice(_)));
    }

    /// The popover's state is app-wide, its windows are not terminal windows (13w seam S9), and
    /// the pieces the brief names exist.
    #[test]
    fn popovers_are_app_wide_and_not_terminal_windows() {
        let swap = include_str!("windows_ui.rs");
        assert!(!swap.contains("web_popover"), "not per window");
        let m = include_str!("mod.rs");
        assert!(m.contains("web_popover: Option<(Option<PaneId>, crate::web_popover::WebPopover)>"));
        assert!(m.contains("self.web_popover_event(id, &event)"), "the window-event chain");
        let me = include_str!("web_popover_app.rs");
        let prod = &me[..me.find("#[cfg(test)]\nmod tests").unwrap()];
        assert!(!prod.contains("registry"), "a popover is never registered as a terminal window");
        assert!(include_str!("script_ui.rs").contains("WebView { view: ViewId, callback: String, signature: String }"));
        // The pane's going closes its popover.
        let pg = &m[m.find("fn pane_gone").unwrap()..];
        assert!(pg[..pg.find("\n    }\n").unwrap()].contains("web_popover"));
    }

    /// P18 and the class ruling: no text from a program, a page or a model is ever loaded with
    /// NavigateToString; a status bar component's HTML is served by `LocalDoc` and nothing else.
    #[test]
    fn program_content_is_served_never_navigated_to_string() {
        let me = include_str!("web_popover_app.rs");
        let prod = &me[..me.find("#[cfg(test)]\nmod tests").unwrap()];
        assert_eq!(prod.matches(".navigate_to_string(").count(), 0);
        assert!(prod.contains("navigate_tandem(doc.url())") && prod.contains("popover_document("));
        assert_eq!(prod.matches("WebView::create(").count(), 1);
        assert!(prod.contains("quick_look_setup(") && prod.contains("html_setup()"));
        assert!(prod.contains("quick_look_scheme_ok(") , "what 'Open in' reads back is judged again");
        // Nowhere in the application's own code.
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut stack = vec![dir];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).unwrap() {
                let path = e.unwrap().path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().is_some_and(|x| x == "rs") && !path.ends_with("webhost_win.rs") {
                    let text = std::fs::read_to_string(&path).unwrap();
                    let prod = &text[..text.find("#[cfg(test)]").unwrap_or(text.len())];
                    assert!(!prod.contains(".navigate_to_string("), "{}", path.display());
                }
            }
        }
    }

    /// Every popover COM-facing step runs behind the host's unwind guard: this module makes no
    /// handler of its own.
    #[test]
    fn popovers_add_no_com_handlers() {
        let me = include_str!("web_popover_app.rs");
        let prod = &me[..me.find("#[cfg(test)]\nmod tests").unwrap()];
        assert!(!prod.contains("::create(Box::new"));
        let wp = include_str!("../web_popover.rs");
        let prod = &wp[..wp.find("#[cfg(test)]\nmod tests").unwrap()];
        assert!(!prod.contains("Handler::create"));
    }
}
