//! Reader Mode and Remove Distractions in a browser pane (milestone 13x4). Task 1 holds the message
//! handler only: the reader scripts run in Termurion's isolated world and post their state through the
//! `readerMode` handler, which arrives as a Client message from the world's binding and nowhere else.

use super::browser_scripts_app::JsDone;
use super::*;
use crate::browser_js::{Routed, World};
use crate::browser_reader::{self as reader, Failure};
use serde_json::{Value, json};

/// The reader's failure on the log: the line of `RMM`, with the runner's error KIND (a script's
/// error text is the page's and is never logged, as `log_js_failure`).
fn log_failure(f: Failure, e: &str) {
    log_failure_as(f, super::browser_scripts_app::failure_kind(e));
}

/// `log_failure` with the text to show already chosen: a kind, or Termurion's own error text.
fn log_failure_as(f: Failure, what: &str) {
    let line = reader::failure_line(f, what);
    crate::rlog!("{}", crate::dlog::name(&line));
}

/// What a call answers when the page changed under it (Task 4 finding I1).
const PAGE_CHANGED: &str = "the page changed";

/// The plain text of a page, or None (`RMM:35-61`).
pub(super) type TextDone = Box<dyn FnOnce(&mut App, PaneId, Option<String>)>;

impl App {
    /// One `readerMode` message. Advisory: only the isolated world's, only with a proven world, and
    /// only the typed `action` is read (the page's text it may carry, the title, is never read, logged
    /// or shown). A page-world message is dropped before it is read.
    pub(super) fn browser_reader_message(&mut self, pane: PaneId, r: &Routed) {
        if r.world != World::Client || !self.client_open(pane) {
            return;
        }
        let Some(action) = r.body.get("action").and_then(|a| a.as_str()) else { return };
        if let Some(bv) = self.browser_views.get_mut(&pane) {
            bv.reader.on_message(action);
        }
    }

    /// A main-frame navigation began: the new page has no reader and no cached text (`BM:1252`).
    pub(super) fn reader_navigation_started(&mut self, pane: PaneId) {
        if let Some(bv) = self.browser_views.get_mut(&pane) {
            bv.reader.reset();
        }
        // The page that asked is gone: its permission questions go with it (13x6 I3).
        self.browser_permission_dismiss();
        // ... and so does its notification still on the balloon.
        self.browser_web_note_dropped(pane);
    }

    /// Loads Readability, then reader mode with its CSS, in the isolated world once per navigation
    /// (`RMM:168-203`). `then` gets `Ok(true)` or the error (already logged). Every body goes
    /// through `browser_js_call`, never the page world.
    pub(super) fn ensure_reader_scripts(&mut self, pane: PaneId, then: JsDone) {
        if self.browser_views.get(&pane).is_some_and(|b| b.reader.injected) {
            return then(self, pane, Ok(json!(true)));
        }
        let [first, second] = match reader::loader_bodies() {
            Ok(b) => b,
            Err(e) => {
                // Termurion's own text (a missing resource, a patch that did not apply), never the page's.
                log_failure_as(Failure::Inject, &e);
                return then(self, pane, Err(e));
            }
        };
        // The page this call is for: a navigation (or a lost world) in between makes it another page's.
        let epoch = self.browser_views.get(&pane).map_or(0, |b| b.reader.epoch);
        self.browser_js_call(pane, &first, json!({}), Box::new(move |app, pane, r| {
            if let Err(e) = r {
                log_failure(Failure::Inject, &e);
                return then(app, pane, Err(e));
            }
            app.browser_js_call(pane, &second, json!({}), Box::new(move |app, pane, r| match r {
                Ok(_) => {
                    let same = app.browser_views.get_mut(&pane).is_some_and(|bv| bv.reader.mark_injected_at(epoch));
                    if !same {
                        return then(app, pane, Err(PAGE_CHANGED.into()));
                    }
                    then(app, pane, Ok(json!(true)))
                }
                Err(e) => {
                    log_failure(Failure::Inject, &e);
                    then(app, pane, Err(e))
                }
            }));
        }));
    }

    /// Reader Mode from the menu: exit when on, else enter (`RMM:29-107`).
    pub(super) fn toggle_reader_mode(&mut self, pane: PaneId) {
        if !self.browser_views.get(&pane).is_some_and(|b| b.reader.reader) {
            return self.enter_reader_mode(pane);
        }
        // Exit clears the flag only after the script ran without error (`RMM:97-107`).
        self.browser_js_call(pane, &reader::exit_reader_body(), json!({}), Box::new(|app, pane, r| match r {
            Ok(_) => {
                if let Some(bv) = app.browser_views.get_mut(&pane) {
                    bv.reader.reader = false;
                }
            }
            Err(e) => log_failure(Failure::Exit, &e),
        }));
    }

    /// iTerm2's `enterReaderMode` (ruling Q48): nothing when already on, else load the scripts and
    /// enter; the flag goes on only when the script answers true (`RMM:78-95`). The Enter Reader Mode
    /// trigger (13x5) calls this.
    pub(super) fn enter_reader_mode(&mut self, pane: PaneId) {
        if self.browser_views.get(&pane).is_none_or(|b| b.reader.reader) {
            return;
        }
        self.ensure_reader_scripts(pane, Box::new(|app, pane, r| {
            if r.is_err() {
                return;
            }
            app.browser_js_call(pane, &reader::enter_reader_body(), json!({}), Box::new(|app, pane, r| match r {
                Ok(v) if reader::answered_true(&v) => {
                    if let Some(bv) = app.browser_views.get_mut(&pane) {
                        bv.reader.reader = true;
                    }
                }
                Ok(_) => {}
                Err(e) => log_failure(Failure::Enter, &e),
            }));
        }));
    }

    /// Remove Distractions from the menu (`RMM:125-166`).
    pub(super) fn toggle_distraction_removal(&mut self, pane: PaneId) {
        if self.browser_views.get(&pane).is_some_and(|b| b.reader.distraction) {
            return self.browser_js_call(pane, &reader::exit_distraction_body(), json!({}), Box::new(|app, pane, r| match r {
                Ok(_) => {
                    if let Some(bv) = app.browser_views.get_mut(&pane) {
                        bv.reader.distraction = false;
                    }
                }
                Err(e) => log_failure(Failure::ExitDistraction, &e),
            }));
        }
        self.ensure_reader_scripts(pane, Box::new(|app, pane, r| {
            if r.is_err() {
                return;
            }
            let [load, enter] = reader::distraction_bodies();
            app.browser_js_call(pane, &load, json!({}), Box::new(move |app, pane, r| {
                if let Err(e) = r {
                    return log_failure(Failure::EnterDistraction, &e);
                }
                app.browser_js_call(pane, &enter, json!({}), Box::new(|app, pane, r| match r {
                    Ok(v) if reader::answered_true(&v) => {
                        if let Some(bv) = app.browser_views.get_mut(&pane) {
                            bv.reader.distraction = true;
                        }
                    }
                    Ok(_) => {}
                    Err(e) => log_failure(Failure::EnterDistraction, &e),
                }));
            }));
        }));
    }

    /// The page's plain text, cached per navigation, "" included (`RMM:35-61`): a cached text is
    /// answered at once; otherwise Readability's article text, or None. An error is not cached
    /// (the page may have changed under the call).
    pub(super) fn browser_plain_text(&mut self, pane: PaneId, done: TextDone) {
        if let Some(cached) = self.browser_views.get(&pane).and_then(|b| b.reader.cached_plain()) {
            return done(self, pane, Some(cached));
        }
        // I1: a navigation cancels the call: an answer from another epoch is dropped, never cached.
        let epoch = self.browser_views.get(&pane).map_or(0, |b| b.reader.epoch);
        self.ensure_reader_scripts(pane, Box::new(move |app, pane, r| {
            // `ensure` logged a failure; there is no text.
            if r.is_err() {
                return done(app, pane, None);
            }
            app.browser_js_call(pane, &reader::plain_text_body(), json!({}), Box::new(move |app, pane, r| {
                let text = match r {
                    Ok(Value::String(s)) => Some(reader::bound_text(&s)),
                    Ok(_) => None,
                    Err(_) => return done(app, pane, None),
                };
                let answer = app.browser_views.get_mut(&pane).and_then(|b| b.reader.store_plain_at(epoch, text.as_deref())).flatten();
                done(app, pane, answer)
            }));
        }));
    }

    /// The page as markdown with chrome kept (`PTS:1009`, `BVC:687`): iTerm2's read-a-section and
    /// find-for-AI path. No caller until the AI browser tools plan (its read-a-section tool); kept
    /// with its tests so that plan has the one markdown entry.
    #[allow(dead_code)] // The AI tools use the `_with` form (they pass skip_chrome themselves).
    pub(super) fn browser_page_markdown(&mut self, pane: PaneId, done: JsDone) {
        self.browser_page_markdown_with(pane, false, done)
    }

    /// The page as markdown (`RMM:63-68`), nav, footer, aside and header dropped when `skip_chrome`
    /// (`PTS:990`, web search). Isolated world, needs no reader script, never cached. `done` gets the
    /// markdown (bounded), `NO_CONTENT` when the script answered no string, or the script's error text
    /// cleaned for display. The markdown is page-derived data: a caller that gives it to a model wraps
    /// it with `page_derived_block`. No caller until the AI browser tools plan (web search).
    pub(super) fn browser_page_markdown_with(&mut self, pane: PaneId, skip_chrome: bool, done: JsDone) {
        self.browser_js_call(pane, &reader::markdown_body(skip_chrome), json!({}), Box::new(move |app, pane, r| {
            let answer = match r {
                Ok(v) => Ok(Value::String(reader::bound_text(&reader::markdown_answer(&v)))),
                Err(e) => Err(crate::inert_files::display_text(&e, 200, false)),
            };
            done(app, pane, answer)
        }));
    }

    /// The page for the AI (`BM:607-618`), the one entry point for page text: `done` gets
    /// `{"title", "content"}` or null (no URL, or no text). The text is the cache's ("" included) or
    /// Readability's, cached through `store_plain`. Page-derived, title and content: see `page_derived_block`.
    pub(super) fn browser_page_content(&mut self, pane: PaneId, done: JsDone) {
        let (url, title) = match self.panes.get(&pane).and_then(|p| p.browser_io()) {
            Some(io) => (io.url.clone(), io.title.clone()),
            None => (String::new(), String::new()),
        };
        if url.is_empty() {
            return done(self, pane, Ok(Value::Null));
        }
        // I1: the url and title belong to the page of this epoch; if it moved the answer is null, never
        // one page's text under another's address.
        let epoch = self.browser_views.get(&pane).map_or(0, |b| b.reader.epoch);
        self.browser_plain_text(pane, Box::new(move |app, pane, plain| {
            let same = app.browser_views.get(&pane).is_some_and(|b| b.reader.current(epoch));
            let content = if same { reader::page_content_json(&url, &title, plain.as_deref()) } else { Value::Null };
            done(app, pane, Ok(content))
        }));
    }

    /// Remove Element at a page point (`RMM:109-123`): the overlay under the right-click goes, in the
    /// isolated world (the DOM is shared with the page). The point is the context's, already in page
    /// coordinates, and reaches the script only through `remove_element_body`'s number formatting.
    /// The body posts nothing and needs no reader script; an error is logged and nothing is shown.
    pub(super) fn remove_element(&mut self, pane: PaneId, x: f64, y: f64) {
        let body = match reader::remove_element_body(x, y) {
            Ok(b) => b,
            Err(e) => return log_failure_as(Failure::RemoveElement, &e),
        };
        self.browser_js_call(pane, &body, json!({}), Box::new(|_, _, r| {
            if let Err(e) = r {
                log_failure(Failure::RemoveElement, &e);
            }
        }));
    }

    /// The Menu button (`BT:306-345`): the offer needs the page's plain text, so the menu opens after
    /// it is known. A second press while waiting is ignored; a pane gone meanwhile shows nothing.
    pub(super) fn browser_menu_press(&mut self, pane: PaneId, at: (f32, f32)) {
        let Some(bv) = self.browser_views.get_mut(&pane) else { return };
        if bv.menu_pending {
            return;
        }
        if bv.reader.cached.is_some() {
            return self.show_browser_menu(pane, at);
        }
        bv.menu_pending = true;
        // M4 (Task 2 review): the one entry point for page text is `browser_page_content`.
        self.browser_page_content(pane, Box::new(move |app, pane, _| {
            let Some(bv) = app.browser_views.get_mut(&pane) else { return };
            bv.menu_pending = false;
            // The answer can be late (Readability is slow, a busy page holds the runner): a pane that
            // is no longer in the shown tab of the live window gets no popup.
            if app.tabs.get(app.active).is_some_and(|t| t.layout.panes().contains(&pane)) {
                app.show_browser_menu(pane, at);
            }
        }));
    }

    /// What the menu shows of the reader: None unless the plain text offers it (`BVC:1053-1055`).
    pub(super) fn browser_reader_menu(&self, pane: PaneId) -> Option<crate::browser::ReaderMenu> {
        let bv = self.browser_views.get(&pane)?;
        if !reader::offers_reader(bv.reader.cached.as_deref()) {
            return None;
        }
        let (has_url, ask_ai) = match self.panes.get(&pane).and_then(|p| p.browser_io()) {
            Some(io) => (!io.url.is_empty(), !io.url.is_empty() && crate::ai_browser::offerable(&io.url, io.dev_null)),
            None => (false, false),
        };
        Some(crate::browser::ReaderMenu { reader: bv.reader.reader, distraction: bv.reader.distraction, has_url, ask_ai })
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    fn prod(src: &str) -> &str {
        src.split("\n#[cfg(test)]").next().unwrap_or(src)
    }

    fn between<'a>(src: &'a str, from: &str, to: &str) -> &'a str {
        let a = src.find(from).unwrap_or_else(|| panic!("missing {from}"));
        let b = a + src[a..].find(to).unwrap_or_else(|| panic!("missing {to}"));
        &src[a..b]
    }

    /// B10: the `readerMode` handler is a listed Client handler: world checked, gate asked, and the
    /// router has its arm before the drop. B8: the reader state dies with the isolated world.
    #[test]
    fn reader_message_is_a_gated_client_handler() {
        let me = prod(include_str!("browser_reader_app.rs"));
        let msg = between(me, "pub(super) fn browser_reader_message", "\n    }\n");
        assert!(msg.contains("r.world != World::Client") && msg.contains("!self.client_open(pane)"));
        assert!(msg.find("World::Client").unwrap() < msg.find("\"action\"").unwrap(), "a page-world message is dropped before it is read");
        assert!(!msg.contains("title") && !msg.contains("rlog!"), "the page's title is never read or logged");
        let router = prod(include_str!("browser_scripts_app.rs"));
        let arm = "h if h == crate::browser_reader::HANDLER => self.browser_reader_message(pane, &r),";
        assert!(router.contains(arm) && router.find(arm).unwrap() < router.find("browser message dropped").unwrap());
        assert!(include_str!("browser_scripts_app.rs").contains("\"browser_reader_message\", \"browser_trigger_message\"]"), "listed in the router test");
    }

    /// Security (Review Focus 6, R13x45-1): every call this file makes is the isolated-world runner's;
    /// the page-world runners, the view secret and the bridge never appear.
    #[test]
    fn reader_calls_use_the_isolated_world() {
        let me = prod(include_str!("browser_reader_app.rs"));
        assert!(me.matches(".browser_js_call(").count() >= 7);
        for bad in ["browser_page_call", "browser_eval", "browser_devtools", "secret", "chrome.webview", "execute_script"] {
            assert!(!me.contains(bad), "{bad}");
        }
    }

    /// `RMM:78-107`, `:125-166`: the flags follow the scripts' answers, and an error is logged by kind.
    #[test]
    fn reader_flows_follow_iterm2() {
        let me = prod(include_str!("browser_reader_app.rs"));
        let enter = between(me, "pub(super) fn enter_reader_mode", "/// Remove Distractions");
        assert!(enter.contains("b.reader.reader) {\n            return;") && enter.contains("answered_true(&v)") && enter.contains("bv.reader.reader = true"), "enter: nothing when on, on only for true");
        assert!(enter.contains("Failure::Enter"));
        let toggle = between(me, "pub(super) fn toggle_reader_mode", "/// iTerm2's `enterReaderMode`");
        assert!(toggle.contains("self.enter_reader_mode(pane)") && toggle.contains("Ok(_) => {\n                if let Some(bv)") && toggle.contains("bv.reader.reader = false") && toggle.contains("Failure::Exit"), "exit clears only after success");
        let dr = between(me, "pub(super) fn toggle_distraction_removal", "/// The page's plain text");
        assert!(dr.contains("exit_distraction_body") && dr.contains("distraction_bodies") && dr.contains("answered_true(&v)") && dr.contains("Failure::ExitDistraction") && dr.contains("Failure::EnterDistraction"));
        assert!(dr.find("ensure_reader_scripts").unwrap() < dr.find("distraction_bodies").unwrap());
        let ensure = between(me, "pub(super) fn ensure_reader_scripts", "/// Reader Mode from the menu");
        assert!(ensure.contains("mark_injected_at(epoch)") && ensure.contains("Failure::Inject") && ensure.contains("loader_bodies()"));
        let text = between(me, "pub(super) fn browser_plain_text", "/// The Menu button");
        assert!(text.contains("cached_plain()") && text.contains("Err(_) => return done(app, pane, None)"), "an error is not cached");
    }

    /// B11: the press waits for the plain text, a second press is ignored, a gone pane shows nothing,
    /// and the entries are picked by their labels.
    #[test]
    fn the_menu_press_waits_for_the_offer() {
        let me = prod(include_str!("browser_reader_app.rs"));
        let press = between(me, "pub(super) fn browser_menu_press", "/// What the menu shows");
        assert!(press.contains("if bv.menu_pending {\n            return;") && press.contains("bv.menu_pending = true") && press.contains("bv.menu_pending = false") && press.contains("app.browser_views.get_mut(&pane) else { return }"));
        let late = &press[press.find("bv.menu_pending = false").unwrap()..];
        assert!(late.contains("app.tabs.get(app.active).is_some_and(|t| t.layout.panes().contains(&pane))") && late.find("is_some_and").unwrap() < late.find("show_browser_menu").unwrap(), "a late menu opens only for a pane in the shown tab");
        let app = prod(include_str!("browser_app.rs"));
        assert!(app.contains("Button::Menu => self.browser_menu_press(pane, menu_at)"));
        assert!(app.contains("\"Reader Mode\" | \"Exit Reader Mode\" => return { self.toggle_reader_mode(pane); true }"));
        assert!(app.contains("\"Remove Distractions\" | \"Exit Distraction Removal\" => return { self.toggle_distraction_removal(pane); true }"));
        assert!(app.contains("self.browser_reader_menu(pane)"));
    }

    /// Task 4: the three producers. Markdown runs in the isolated world with no reader script and is
    /// never cached; the page content comes from the cache or Readability and is page-derived.
    #[test]
    fn page_text_producers_follow_iterm2() {
        let me = prod(include_str!("browser_reader_app.rs"));
        let md = between(me, "pub(super) fn browser_page_markdown_with", "/// The page for the AI");
        assert!(md.contains("self.browser_js_call(pane, &reader::markdown_body(skip_chrome)") && !md.contains("ensure_reader_scripts") && !md.contains("store_plain") && !md.contains("cached"));
        assert!(md.contains("reader::bound_text(&reader::markdown_answer(&v))") && md.contains("display_text(&e, 200, false)"), "bounded; the page's error text cleaned");
        let off = between(me, "pub(super) fn browser_page_markdown(", "/// The page as markdown (`RMM");
        assert!(off.contains("browser_page_markdown_with(pane, false, done)"));
        let content = between(me, "pub(super) fn browser_page_content", "/// Remove Element at a page point");
        assert!(content.contains("url.is_empty()") && content.contains("self.browser_plain_text(") && content.contains("reader::page_content_json(&url, &title, plain.as_deref())"));
        let text = between(me, "pub(super) fn browser_plain_text", "/// The page as markdown");
        assert!(text.contains("reader::bound_text(&s)") && text.contains("store_plain_at(epoch, text.as_deref())") && !text.contains(".store_plain("), "I1: stored only for the epoch it began in");
        assert!(text.find("let epoch").unwrap() < text.find("ensure_reader_scripts").unwrap(), "the epoch is read before anything runs");
        assert!(content.contains("reader.current(epoch)") && content.find("let epoch").unwrap() < content.find("browser_plain_text(").unwrap() && content.contains("Value::Null"), "I1: null if the page changed");
        let ensure = between(me, "pub(super) fn ensure_reader_scripts", "/// Reader Mode from the menu");
        assert!(ensure.contains("mark_injected_at(epoch)") && !ensure.contains("injected = true") && ensure.find("let epoch").unwrap() < ensure.find("browser_js_call").unwrap(), "I1 twin: injected only for the same page");
        assert!(!md.contains("browser_page_call"), "M3: turndown runs in the isolated world only");
        let press = between(me, "pub(super) fn browser_menu_press", "/// What the menu shows");
        assert!(press.contains("self.browser_page_content(pane,") && !press.contains("browser_plain_text"), "M4");
    }

    /// `RMM:35-61` through `BM:607-618` (review M2): what the cache and the plain-text script give the
    /// page content, step by step. An App cannot be built in a unit test; the producers' wiring is
    /// pinned above and this follows the pure parts they call.
    #[test]
    fn plain_text_cache_follows_iterm2() {
        use crate::browser_reader::{State, page_content_json};
        let content = |s: &State, title: &str| page_content_json("https://a/", title, s.cached_plain().as_deref());
        let mut s = State { cached: Some("cached text".into()), ..State::default() };
        assert_eq!(content(&s, "T"), json!({"title": "T", "content": "cached text"}));
        s.cached = Some(String::new());
        assert_eq!(content(&s, "T"), json!({"title": "T", "content": ""}), "a cached empty text is answered");
        s.cached = None;
        assert_eq!(content(&s, "T"), Value::Null, "no text yet: the script runs");
        assert_eq!(s.store_plain(Some("")), None, "an empty answer is null the first time");
        assert_eq!(content(&s, "T"), json!({"title": "T", "content": ""}), "then \"\" from the cache, with no call");
        s.reset();
        assert_eq!(s.cached_plain(), None, "a navigation runs the scripts again");
        assert_eq!(s.store_plain(Some("body")), Some("body".into()));
        assert_eq!(content(&s, ""), json!({"title": "https://a/", "content": "body"}));
    }

    #[test]
    fn the_reader_state_dies_with_the_world() {
        let r = prod(include_str!("browser_scripts_app.rs"));
        assert!(r.contains("bv.copy_mode = None;\n            bv.reader.reset();\n            dones"), "view close");
        let restart = between(r, "fn browser_world_restart", "pub(super) fn browser_world_tick");
        assert!(restart.contains("bv.reader.reset()"), "restart");
        let tick = between(r, "pub(super) fn browser_world_tick", "let late =");
        assert!(tick.contains("bv.reader.reset()"), "lost after a process failure");
        let starting = between(r, "Event::Starting { redirect: false, .. } => {", "self.browser_copy_mode_set(pane, false)");
        assert!(starting.contains("self.reader_navigation_started(pane)"), "a navigation start");
        let created = between(r, "\"Runtime.executionContextCreated\" => {", "\"Runtime.executionContextDestroyed\"");
        assert!(created.contains("bv.reader.reset()") && created.contains("is_main_context"), "the main frame's new Termurion context");
    }
}
