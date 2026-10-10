//! Browser named marks, the app side, adding (milestone 13x5, Task 3): the three ways a person asks
//! for a mark (Edit menu, the page's context menu, the Named Marks tool), the prompt that names it,
//! the page's point, the stored mark, the app-wide cache. Adding is only ever a person's action:
//! no page message reaches `browser_add_named_mark`. What the page answers (the centre, the XPath)
//! is validated and bounded before anything is stored, a page that moved on since the request
//! cannot receive the mark (the navigation epoch), and Tandem's own `iterm2-about:` pages take none.
//! The decisions are pure functions in `browser_marks`; this file applies them.

use super::*;
use crate::browser_marks::{self as marks, AddPath, CENTER_JS, Location, Mark};
use crate::browser_marks_db::ALL_LIMIT;
use crate::webhost::Event;
use serde_json::{Value, json};

/// What the shared Set Named Mark window is naming.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum MarkPrompt {
    /// A new mark; `at` is the context menu's click in page pixels, None for the viewport centre.
    /// `epoch` is the navigation epoch the request began in (`MethodState::epoch`).
    Add { at: Option<(f64, f64)>, epoch: u64 },
    /// A rename (Task 5).
    Rename { guid: String },
}

/// The prompt a request opens, or None when the path's gate refuses it (review M11, B5).
pub(super) fn add_request(path: AddPath, url: &str, method: &str, at: Option<(f64, f64)>, epoch: u64) -> Option<MarkPrompt> {
    marks::gate_before_prompt(path, url, Some(method)).is_none().then_some(MarkPrompt::Add { at, epoch })
}

/// Whether a stored mark may be made for `url` now: not one of Tandem's pages, and not a URL the
/// host cut at its cap (`cap_url` leaves up to three bytes off a multi-byte character).
fn url_markable(url: &str) -> bool {
    !marks::is_local_page(url) && url.len() + 4 <= marks::MAX_URL
}

impl App {
    /// A person asked for a named mark on browser pane `pane` (`BVC:1401-1425`). Nothing opens here: the
    /// request is queued and `about_to_wait` opens the window (review I2). A request the path's gate
    /// refuses does nothing (Tandem's menu items are enabled once, Decision).
    pub(super) fn browser_add_named_mark(&mut self, pane: PaneId, at: Option<(f64, f64)>, path: AddPath) {
        let Some(bv) = self.browser_views.get(&pane) else { return };
        let Some(io) = self.panes.get(&pane).and_then(|p| p.browser_io()) else { return };
        let Some(prompt) = add_request(path, &io.url, bv.marks_method.method(), at, bv.marks_method.epoch()) else { return };
        self.pending_mark_prompt = Some((pane, prompt));
        self.mark_prompt_focused = false;
    }

    /// Opens the queued mark window in the shared prompt slot, or focuses the one that is open (once)
    /// and keeps the request queued until the slot is free. A request for a pane that is gone is dropped.
    pub(super) fn open_pending_mark_prompt(&mut self, event_loop: &ActiveEventLoop) {
        let Some((pane, prompt)) = self.pending_mark_prompt.take() else { return };
        if !self.is_browser_pane(pane) {
            return;
        }
        if let Some((w, ..)) = &self.prompt {
            if !self.mark_prompt_focused {
                w.focus();
                self.mark_prompt_focused = true;
            }
            self.pending_mark_prompt = Some((pane, prompt));
            return;
        }
        let Some(g) = self.gpu.as_ref() else {
            self.pending_mark_prompt = Some((pane, prompt));
            return;
        };
        let stamp = crate::prompt_ui::local_stamp();
        match crate::prompt_ui::PromptWindow::open(event_loop, &g.instance, &g.adapter, &g.device, &g.queue, marks::PROMPT_TITLE, marks::PROMPT_LABEL, &stamp) {
            Ok(w) => self.prompt = Some((w, pane, super::marks_ui::PromptFor::Browser(prompt))),
            Err(e) => self.banners.set("window", format!("Could not open the window: {e}")),
        }
    }

    /// OK in the window with a non-empty name (`BDV:34-36`). A refusal here logs nothing and adds
    /// nothing (`NMM:142-145`); otherwise the point is found in the isolated world, then the XPath.
    pub(super) fn browser_mark_prompt_done(&mut self, pane: PaneId, prompt: MarkPrompt, name: String) {
        let MarkPrompt::Add { at, epoch } = prompt else { return };
        if !self.mark_request_current(pane, epoch) {
            return;
        }
        match at {
            Some((x, y)) => match marks::click_point(x, y) {
                Some(point) => self.mark_locate(pane, epoch, name, point),
                None => crate::rlog!("Bad result from js"),
            },
            None => self.browser_js_call(
                pane,
                CENTER_JS,
                json!({}),
                Box::new(move |app, pane, r| match r.ok().and_then(|v| marks::center_point(&v)) {
                    Some(point) => app.mark_locate(pane, epoch, name, point),
                    None => crate::rlog!("Bad result from js"),
                }),
            ),
        }
    }

    /// Whether pane `pane` still shows the page the request began on and may take a mark: the
    /// navigation epoch is the same, the known method is GET or unknown, the page is not Tandem's own.
    fn mark_request_current(&self, pane: PaneId, epoch: u64) -> bool {
        let Some(bv) = self.browser_views.get(&pane) else { return false };
        let Some(io) = self.panes.get(&pane).and_then(|p| p.browser_io()) else { return false };
        bv.marks_method.epoch() == epoch && marks::gate_on_add(Some(bv.marks_method.method())).is_none() && url_markable(&io.url)
    }

    /// The XPath at a page point (`NMM:408-418`), in the isolated world (`NMM:420`).
    fn mark_locate(&mut self, pane: PaneId, epoch: u64, name: String, (x, y): (i64, i64)) {
        let Some(body) = marks::xpath_script(x, y) else { return };
        self.browser_js_call(pane, &body, json!({}), Box::new(move |app, pane, r| app.mark_located(pane, epoch, name, r)));
    }

    /// The page's answer: a location or nothing (`NMM:422-427`). Page data, validated and bounded by
    /// `Location::from_json` and `mark_url`; the page must still be the one the request began on.
    fn mark_located(&mut self, pane: PaneId, epoch: u64, name: String, answer: Result<Value, String>) {
        let Some(loc) = answer.ok().and_then(|v| Location::from_json(&v)) else { return crate::rlog!("Bad result from js") };
        if !self.mark_request_current(pane, epoch) {
            return;
        }
        let Some(io) = self.panes.get(&pane).and_then(|p| p.browser_io()) else { return };
        let (dev_null, page) = (io.dev_null, io.url.clone());
        let Some(url) = marks::mark_url(&page, &loc) else { return crate::rlog!("Bad result from js") };
        let guid = uuid::Uuid::new_v4().hyphenated().to_string().to_uppercase();
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64());
        let saved = self.browser_db(dev_null).is_some_and(|db| db.add_named_mark(&guid, &url, &name, "", now));
        if !saved {
            return crate::rlog!("Failed to save named mark to database");
        }
        self.browser_marks_load(dev_null);
        self.browser_marks_changed();
        // Task 4: the page's annotations are shown again here (`NMM:156-161`).
    }

    /// A browser pane's event that tells the method of the page's document (P5, Decision): every
    /// `Starting` resets it to GET and begins a new epoch; the `DocumentMethod` of the same
    /// navigation sets it. Another view's event leaves it alone.
    pub(super) fn marks_method_event(&mut self, pane: PaneId, e: &Event) {
        let Some(bv) = self.browser_views.get_mut(&pane) else { return };
        if e.view() != bv.id {
            return;
        }
        match e {
            Event::Starting { nav, .. } => bv.marks_method.starting(*nav),
            Event::DocumentMethod { nav, method, .. } => bv.marks_method.document(*nav, method),
            _ => {}
        }
    }

    /// The store's marks, newest first, as the manager's cache holds them (`NMM:552-572`).
    pub(super) fn browser_marks_load(&mut self, dev_null: bool) {
        let rows = self.browser_db(dev_null).map(|db| db.named_marks_page(None, 0, ALL_LIMIT)).unwrap_or_default();
        let cache = self.browser_marks.entry(dev_null).or_default();
        cache.marks = rows.into_iter().map(|r| Mark { guid: r.guid, url: r.url, name: r.name, sort: r.sort }).collect();
        cache.generation += 1;
    }

    /// The cached marks of one store (empty before the first load).
    pub(super) fn browser_marks_cache(&self, dev_null: bool) -> &[Mark] {
        self.browser_marks.get(&dev_null).map_or(&[], |c| c.marks.as_slice())
    }

    /// iTerm2's `NamedMarksDidChangeNotification`: the menus and the tool rebuild.
    pub(super) fn browser_marks_changed(&mut self) {
        for c in self.browser_marks.values_mut() {
            c.generation += 1;
        }
        self.tools_changed();
        self.refresh_named_marks();
    }

    /// A pane that closed takes its queued mark window request with it (an open window for it adds
    /// nothing: `mark_request_current` finds no pane).
    pub(super) fn marks_pane_gone(&mut self, pane: PaneId) {
        if self.pending_mark_prompt.as_ref().is_some_and(|(p, _)| *p == pane) {
            self.pending_mark_prompt = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prod() -> &'static str {
        let src = include_str!("browser_marks_app.rs");
        &src[..src.find("#[cfg(test)]").unwrap()]
    }

    #[test]
    fn mark_prompt_queue_follows_iterm2() {
        // The context menu on about:blank after a POST queues (iTerm2 refuses only after OK, M11).
        assert_eq!(add_request(AddPath::ContextMenu, "about:blank", "POST", Some((3.0, 4.0)), 5), Some(MarkPrompt::Add { at: Some((3.0, 4.0)), epoch: 5 }));
        // The same page from the Edit menu queues nothing.
        assert_eq!(add_request(AddPath::Menu, "about:blank", "POST", None, 5), None);
        assert_eq!(add_request(AddPath::Menu, "https://a/", "POST", None, 5), None);
        assert_eq!(add_request(AddPath::Menu, "https://a/", "GET", None, 5), Some(MarkPrompt::Add { at: None, epoch: 5 }));
        // Tandem's own pages carry no marks, by any path (B5).
        assert_eq!(add_request(AddPath::Tool, "iterm2-about:history", "GET", None, 5), None);
        // A refused add after OK: a known non-GET method.
        assert!(marks::gate_on_add(Some("POST")).is_some());
        assert!(!url_markable("iterm2-about:history"));
        assert!(url_markable("https://a/"));
        assert!(!url_markable(&format!("https://a/{}", "x".repeat(marks::MAX_URL))));
    }

    /// Source shape (an `App` cannot be built in a unit test, B1): the queue is dropped with its pane,
    /// the prompt opens only from the event loop, OK runs the two scripts in the isolated world, the
    /// request is checked against the navigation epoch before and after the page answers, and a
    /// stored mark is validated, with a fresh upper-case guid.
    #[test]
    fn adding_is_a_persons_action_and_bound_to_its_page() {
        let p = prod();
        assert!(p.contains("pub(super) fn browser_add_named_mark(&mut self, pane: PaneId, at: Option<(f64, f64)>, path: AddPath)"));
        let done = &p[p.find("pub(super) fn browser_mark_prompt_done").unwrap()..p.find("fn mark_request_current").unwrap()];
        assert!(done.contains("self.mark_request_current(pane, epoch)") && done.contains("CENTER_JS") && done.contains("self.browser_js_call("));
        assert!(!p.contains("browser_page_call") && !p.contains("browser_eval"), "the page world is never used for a mark");
        let located = &p[p.find("fn mark_located").unwrap()..p.find("pub(super) fn marks_method_event").unwrap()];
        assert!(located.contains("Location::from_json") && located.contains("mark_request_current(pane, epoch)") && located.contains("marks::mark_url(&page, &loc)"));
        assert!(located.contains(".hyphenated().to_string().to_uppercase()") && located.contains("\"Failed to save named mark to database\"") && located.contains("\"Bad result from js\""));
        assert!(located.contains("db.add_named_mark(&guid, &url, &name, \"\", now)"));
        assert!(located.find("Location::from_json").unwrap() < located.find("add_named_mark").unwrap());
        let open = &p[p.find("pub(super) fn open_pending_mark_prompt").unwrap()..p.find("pub(super) fn browser_mark_prompt_done").unwrap()];
        assert!(open.contains("marks::PROMPT_TITLE, marks::PROMPT_LABEL, &stamp") && open.contains("PromptFor::Browser(prompt)") && open.contains("w.focus()"));
    }

    /// Who may ask: the Edit menu commands, the context menu pick and the Named Marks tool's Add, each
    /// with its path; no message handler, script event or page-driven code reaches the request.
    #[test]
    fn only_a_persons_actions_ask_for_a_mark() {
        let files: [(&str, &str); 6] = [
            ("browser_app.rs", include_str!("browser_app.rs")),
            ("browser_scripts_app.rs", include_str!("browser_scripts_app.rs")),
            ("browser_db_app.rs", include_str!("browser_db_app.rs")),
            ("browser_reader_app.rs", include_str!("browser_reader_app.rs")),
            ("browser_input_app.rs", include_str!("browser_input_app.rs")),
            ("browser_frames_app.rs", include_str!("browser_frames_app.rs")),
        ];
        for (name, src) in files {
            let live = src.split("#[cfg(test)]").next().unwrap();
            assert!(!live.contains("browser_add_named_mark"), "{name}: a page-driven path asks for a mark");
        }
        let ctx = include_str!("browser_ctx_app.rs");
        assert!(ctx.contains("a.browser_add_named_mark(pane, Some((ctx.x, ctx.y)), AddPath::ContextMenu)") && ctx.contains("named_marks: true"));
        let ui = include_str!("marks_ui.rs");
        assert_eq!(ui.matches("self.browser_add_named_mark(id, None, AddPath::Menu)").count(), 1);
        let tools = include_str!("toolbelt_tools_ui.rs");
        assert!(tools.contains("self.browser_add_named_mark(pane, None, crate::browser_marks::AddPath::Tool)"));
    }

    /// The event routing: every Starting (redirects included) and the DocumentMethod reach the
    /// tracker, which reads the pane's own view and the navigation id; the state lives in the view
    /// and goes with its world (B3).
    #[test]
    fn method_events_reach_the_tracker() {
        let app = include_str!("browser_app.rs");
        let live = app.split("#[cfg(test)]").next().unwrap();
        let starting = &live[live.find("Event::Starting { url, kind, user, redirect, .. } => {").unwrap()..];
        assert!(starting[..starting.find("browser_nav_starting").unwrap()].contains("self.marks_method_event(pane, e);"), "before the redirect filter, so redirects count");
        assert!(live.contains("Event::DocumentMethod { .. } => self.marks_method_event(pane, e),"));
        assert!(live.contains("track_document_method: true"));
        assert!(live.contains("marks_method: Default::default()"));
        let scripts = include_str!("browser_scripts_app.rs");
        let reset = &scripts[scripts.find("pub(super) fn browser_reset_world").unwrap()..];
        assert!(reset[..reset.find("self.global_pane_gone(pane);").unwrap()].contains("bv.marks_method.reset();"));
        let p = prod();
        let ev = &p[p.find("pub(super) fn marks_method_event").unwrap()..p.find("pub(super) fn browser_marks_load").unwrap()];
        assert!(ev.contains("e.view() != bv.id") && ev.contains("starting(*nav)") && ev.contains("document(*nav, method)"));
        let modrs = include_str!("mod.rs");
        let gone = &modrs[modrs.find("pub(super) fn pane_gone").unwrap()..];
        assert!(gone[..gone.find("self.close_browser_view(id);").unwrap()].contains("self.marks_pane_gone(id);"));
        assert!(modrs.contains("self.open_pending_mark_prompt(event_loop);"));
    }
}
