//! Browser tool calls on a pane (13x AI browser tools). Task 3 holds the return path: the
//! waiting calls by token, the gate, the sweep and the delivery re-check. Task 4 adds the runner
//! (`ai_browser_start` answers only `get_current_url` until then; every other browser call is
//! unavailable, which fails closed).
//!
//! Task 4: the runner. `Calls` is App-free (it returns the `Act`s the App then performs), so the
//! whole state machine is tested without a window. A load is started ONLY through the guarded
//! `WebView::navigate` (never Tandem's own `navigate_tandem`), so `webhost::navigation_allowed`
//! judges it and every redirect; page text is read only through `browser_page_markdown(_with)`
//! (the isolated world) and reaches the model only as a `Page` answer. Arguments (URLs, queries) and
//! page text are never logged.

use super::ai_tools_app::ToolStart;
use super::App;
use crate::ai_browser::{Answer, Binding, Page, Waits};
use crate::ai_terminal::Outcome;
use crate::ai_tools::Call;
use crate::layout::PaneId;
use std::collections::{HashMap, HashSet};

/// What a waiting call is doing.
pub(super) enum Step {
    Find { query: String },
    Load,
    Search,
    Read { start: i64, count: i64 },
}

/// One browser call under a token: bound to its (chat, request id) once `ai_run_tool` has it.
pub(super) struct BrowserCall {
    pub bound: Option<(String, String)>,
    #[allow(dead_code)] // Kept with the call for diagnostics; the waits hold the pane.
    pub pane: PaneId,
    pub step: Step,
}

/// A finished call's answer: fixed text, or page text still tied to the page it came from.
pub(super) enum Ready {
    Plain(Outcome),
    Page(Answer),
}

/// App-wide state of the waiting calls (never swapped with a tab).
#[derive(Default)]
pub(super) struct Calls {
    pub calls: HashMap<u64, BrowserCall>,
    pub waits: Waits,
    pub done: Vec<(u64, Ready)>,
    pub next: u64,
}

/// What the App does next for a call.
#[derive(Debug, PartialEq)]
pub(super) enum Act {
    /// Load `url` in the pane's view through the guarded `navigate`.
    Navigate { pane: PaneId, token: u64, url: String },
    /// Read the pane's page as markdown (chrome dropped for a web search) for the token.
    Markdown { pane: PaneId, token: u64, skip_chrome: bool },
}

/// How a call begins.
#[derive(Debug, PartialEq)]
pub(super) enum Started {
    /// Answered at once; no token.
    Done(Outcome),
    /// Waiting under the token, after the act.
    Wait(u64, Act),
}

/// The fixed answer of a web search that cannot run or finish.
fn search_unavailable() -> Outcome {
    Outcome { model: crate::ai_browser::SEARCH_UNAVAILABLE.to_string(), notice: Some(crate::ai_browser::N_SEARCH_FAILED.to_string()) }
}

/// A call that was waiting for a navigation, which did not complete as `l` says.
fn load_failed(step: &Step, l: &crate::ai_browser::Loaded) -> Outcome {
    match step {
        Step::Search => search_unavailable(),
        _ => crate::ai_browser::load_outcome(l),
    }
}

/// A call that was waiting for the page text, whose pane closed.
fn read_gone(step: &Step) -> Outcome {
    use crate::ai_browser as b;
    let o = |model: String, notice: &str| Outcome { model, notice: Some(notice.to_string()) };
    match step {
        Step::Search => search_unavailable(),
        Step::Find { .. } => o(b::FIND_CONVERT_FAILED.to_string(), b::N_FIND_FAILED),
        _ => o(format!("{}{}", b::READ_FAILED_PREFIX, b::PANE_GONE), b::N_READ_FAILED),
    }
}

impl Calls {
    pub fn mint(&mut self, pane: PaneId, step: Step) -> u64 {
        self.next += 1;
        self.calls.insert(self.next, BrowserCall { bound: None, pane, step });
        self.next
    }


    fn push_plain(&mut self, token: u64, o: Outcome) {
        self.done.push((token, Ready::Plain(o)));
    }

    /// `load_url`: http and https only (`url_refusal`), then one AI load per pane; the load it
    /// interrupted is answered at once.
    pub fn start_load(&mut self, pane: PaneId, url: &str) -> Started {
        if let Some(o) = crate::ai_browser::url_refusal(url) {
            return Started::Done(o);
        }
        self.begin_load(pane, url.to_string(), Step::Load)
    }

    /// `web_search_in_browser`: the person's search engine template with the query encoded; a URL
    /// that is not a plain web page answers `SEARCH_UNAVAILABLE` and navigates nothing.
    pub fn start_search(&mut self, pane: PaneId, template: &str, query: &str) -> Started {
        let url = crate::browser::search_url(template, query);
        if crate::ai_browser::url_refusal(&url).is_some() {
            return Started::Done(search_unavailable());
        }
        self.begin_load(pane, url, Step::Search)
    }

    fn begin_load(&mut self, pane: PaneId, url: String, step: Step) -> Started {
        let token = self.mint(pane, step);
        if let Some(old) = self.waits.start_load(pane, token) {
            if let Some(step) = self.calls.get(&old).map(|c| &c.step) {
                let o = load_failed(step, &crate::ai_browser::Loaded::Interrupted);
                self.push_plain(old, o);
            }
        }
        Started::Wait(token, Act::Navigate { pane, token, url })
    }

    /// `read_web_page_section` or `find_on_page`: waits for the page's markdown.
    pub fn start_read(&mut self, pane: PaneId, step: Step) -> Started {
        let token = self.mint(pane, step);
        self.waits.start_read(pane, token);
        Started::Wait(token, Act::Markdown { pane, token, skip_chrome: false })
    }

    /// The pane's next main-frame navigation start is the waiting load's own (WebView2 reports an
    /// API navigation as user-initiated, so `user` cannot tell it apart): it takes the id, once.
    /// A redirect never binds; a navigation started later never finishes the load (`Waits::navigated`).
    pub fn starting(&mut self, pane: PaneId, redirect: bool, nav: u64) {
        if !redirect {
            self.waits.bind_nav(pane, nav);
        }
    }

    /// Navigation `nav` finished. Only the waiting load's own navigation counts; a web search
    /// whose load worked goes on to read the page.
    pub fn navigated(&mut self, pane: PaneId, nav: u64, ok: bool, status: i32) -> Option<Act> {
        let token = self.waits.navigated(pane, nav)?;
        let search = match self.calls.get(&token).map(|c| &c.step) {
            Some(Step::Search) => true,
            Some(Step::Load) => false,
            _ => return None,
        };
        if !ok {
            let l = crate::ai_browser::Loaded::Failed(crate::ai_browser::nav_error_text(status));
            let o = if search { search_unavailable() } else { crate::ai_browser::load_outcome(&l) };
            self.push_plain(token, o);
            return None;
        }
        if search {
            self.waits.start_read(pane, token);
            return Some(Act::Markdown { pane, token, skip_chrome: true });
        }
        self.push_plain(token, crate::ai_browser::load_outcome(&crate::ai_browser::Loaded::Ok));
        None
    }

    /// The page's markdown answer for a token: dropped unless the call still waits for it (a closed
    /// pane, a swept chat); otherwise built from `page` (its binding is re-checked on delivery).
    pub fn read_answer(&mut self, pane: PaneId, token: u64, page: &Page, markdown: Result<&str, String>) {
        if !self.waits.read_done(pane, token) {
            return;
        }
        let answer = match self.calls.get(&token).map(|c| &c.step) {
            Some(Step::Find { query }) => page.find(markdown, query),
            Some(Step::Read { start, count }) => page.section(markdown, *start, *count),
            Some(Step::Search) => page.web_search(&crate::ai_browser::Loaded::Ok, Some(markdown)),
            _ => return,
        };
        self.done.push((token, Ready::Page(answer)));
    }

    /// A read that cannot go on (the page is not one the model may read): a fixed answer.
    pub fn read_refused(&mut self, pane: PaneId, token: u64, o: Outcome) {
        if self.waits.read_done(pane, token) {
            self.push_plain(token, o);
        }
    }

    /// The pane closed: every call waiting on it answers a failure.
    pub fn pane_gone(&mut self, pane: PaneId) {
        for token in self.waits.pane_gone(pane) {
            let Some(step) = self.calls.get(&token).map(|c| &c.step) else { continue };
            let o = match step {
                Step::Load => crate::ai_browser::load_outcome(&crate::ai_browser::Loaded::PaneGone),
                s => read_gone(s),
            };
            self.push_plain(token, o);
        }
    }

    /// A load that could not be started: its wait ends with the failure.
    pub fn load_not_started(&mut self, pane: PaneId, token: u64, why: &str) {
        if self.waits.cancel_load(pane, token) {
            let Some(step) = self.calls.get(&token).map(|c| &c.step) else { return };
            let o = load_failed(step, &crate::ai_browser::Loaded::Failed(why.to_string()));
            self.push_plain(token, o);
        }
    }

    /// A chat that stopped: its calls and their late answers are dropped.
    pub fn forget_chat(&mut self, chat: &str) {
        self.sweep(&|c| c != chat);
    }

    /// The person unlinked the chat's browser session: every call of the chat, waiting or answered
    /// but not yet delivered, answers "page changed" now. Nothing read from the page is delivered.
    pub fn unlinked(&mut self, chat: &str) {
        let tokens: Vec<u64> = self.calls.iter().filter(|(_, c)| c.bound.as_ref().is_some_and(|(ch, _)| ch == chat)).map(|(t, _)| *t).collect();
        for t in tokens {
            self.waits.forget(t);
            self.done.retain(|(d, _)| *d != t);
            self.push_plain(t, crate::ai_terminal::Outcome { model: crate::ai_browser::PAGE_CHANGED.to_string(), notice: Some(crate::ai_browser::N_PAGE_CHANGED.to_string()) });
        }
    }

    /// Binds a token to the chat and request it answers; once.
    pub fn bind(&mut self, token: u64, chat: &str, request_id: &str) {
        if let Some(c) = self.calls.get_mut(&token).filter(|c| c.bound.is_none()) {
            c.bound = Some((chat.to_string(), request_id.to_string()));
        }
    }

    /// A token and everything it waits for or has answered: later answers are dropped.
    pub fn forget(&mut self, token: u64) {
        self.calls.remove(&token);
        self.waits.forget(token);
        self.done.retain(|(t, _)| *t != token);
    }

    /// Forgets every call bound to a chat `keep` refuses (gate closed, turn ended).
    pub fn sweep(&mut self, keep: &dyn Fn(&str) -> bool) {
        let gone: Vec<u64> = self.calls.iter().filter(|(_, c)| c.bound.as_ref().is_some_and(|(chat, _)| !keep(chat))).map(|(t, _)| *t).collect();
        for t in gone {
            self.forget(t);
        }
    }

    /// The chats with a bound call.
    pub fn chats(&self) -> HashSet<String> {
        self.calls.values().filter_map(|c| c.bound.as_ref().map(|(chat, _)| chat.clone())).collect()
    }

    /// Drains the answers: an unbound or forgotten token is dropped, and a page answer is
    /// delivered only when `now(pane)` is still the page it was read from.
    pub fn finish(&mut self, now: &dyn Fn(PaneId) -> Option<Binding>) -> Vec<(String, String, Outcome)> {
        let mut out = Vec::new();
        for (token, ready) in std::mem::take(&mut self.done) {
            let Some(call) = self.calls.remove(&token) else { continue };
            self.waits.forget(token);
            let Some((chat, request_id)) = call.bound else { continue };
            let outcome = match ready {
                Ready::Plain(o) => o,
                Ready::Page(a) => {
                    let current = now(a.binding().pane());
                    a.deliver(current.as_ref())
                }
            };
            out.push((chat, request_id, outcome));
        }
        out
    }
}

impl App {
    /// A browser call on `pane`: `get_current_url` answers at once, the others wait under a token
    /// (`ToolStart::Waiting`) that `chat_tick` answers through `ai_browser_finish`.
    pub(super) fn ai_browser_start(&mut self, pane: PaneId, call: &Call) -> ToolStart {
        let Some(io) = self.panes.get(&pane).and_then(|p| p.browser_io()) else { return ToolStart::Done(Outcome { model: crate::ai_browser::NOT_A_BROWSER.to_string(), notice: None }) };
        let started = match call {
            Call::GetUrl => return ToolStart::Done(crate::ai_browser::current_url_outcome(&io.url, io.dev_null)),
            Call::LoadUrl { url } => self.ai_browser.start_load(pane, url),
            Call::WebSearch { query } => {
                let template = self.globals.string("SearchCommand");
                self.ai_browser.start_search(pane, &template, query)
            }
            Call::ReadWebPage { starting_line_number, number_of_lines } => match self.ai_browser_page(pane) {
                Ok(_) => self.ai_browser.start_read(pane, Step::Read { start: *starting_line_number, count: *number_of_lines }),
                Err(o) => return ToolStart::Done(o),
            },
            Call::FindOnPage { query } => match self.ai_browser_page(pane) {
                Ok(_) => self.ai_browser.start_read(pane, Step::Find { query: query.clone() }),
                Err(o) => return ToolStart::Done(o),
            },
            _ => return ToolStart::Done(Outcome { model: super::ai_tools_app::NOT_AVAILABLE.to_string(), notice: None }),
        };
        match started {
            Started::Done(o) => ToolStart::Done(o),
            Started::Wait(token, act) => {
                self.ai_browser_act(act, None);
                ToolStart::Waiting(token)
            }
        }
    }

    /// The pane's page now as a readable `Page` (a missing view is epoch 0 of nothing: refused),
    /// or the fixed refusal (private pane, power or local page).
    fn ai_browser_page(&self, pane: PaneId) -> Result<Page, Outcome> {
        let Some(io) = self.panes.get(&pane).and_then(|p| p.browser_io()) else { return Err(Outcome { model: crate::ai_browser::NOT_A_BROWSER.to_string(), notice: None }) };
        // No view is no page: refused, never epoch 0 (task 3 review minor 2).
        let Some(epoch) = self.browser_views.get(&pane).map(|b| b.reader.epoch) else { return Err(Outcome { model: crate::ai_browser::NOT_OFFERED.to_string(), notice: None }) };
        Page::open(pane, epoch, &io.url, &io.title, io.dev_null)
    }

    /// Performs what `Calls` asked for. `page` is the page a read was started on (None for a web
    /// search's read, which is of the results page as it stands when its load finishes).
    fn ai_browser_act(&mut self, act: Act, page: Option<Page>) {
        match act {
            Act::Navigate { pane, token, url } => {
                if let Err(why) = self.ai_browser_load(pane, &url) {
                    // Nothing was started: the wait ends with a failure, never a hang.
                    self.ai_browser.load_not_started(pane, token, why);
                }
            }
            Act::Markdown { pane, token, skip_chrome } => {
                let page = match page {
                    Some(p) => p,
                    None => match self.ai_browser_page(pane) {
                        Ok(p) => p,
                        Err(o) => return self.ai_browser.read_refused(pane, token, o),
                    },
                };
                self.browser_page_markdown_with(pane, skip_chrome, Box::new(move |app, pane, r| {
                    let r = match r {
                        Ok(serde_json::Value::String(s)) => Ok(s),
                        Ok(_) => Err(String::new()),
                        Err(e) => Err(e),
                    };
                    app.ai_browser.read_answer(pane, token, &page, r.as_deref().map_err(|e| e.clone()));
                }));
            }
        }
    }

    /// The one place a model's URL is loaded: the GUARDED `WebView::navigate` (never Tandem's own
    /// load), so `navigation_allowed` judges it and each redirect; with no view yet the URL waits in
    /// `pending_url`, which the view's creation loads the same way. While the permission store is
    /// not synced or the world is being set up nothing loads (the parked/deferred loads are
    /// Tandem's own, which a model's URL must never become): a fixed failure.
    fn ai_browser_load(&mut self, pane: PaneId, url: &str) -> Result<(), &'static str> {
        const NOT_READY: &str = "the browser is not ready, try again";
        let synced = self.perm_synced(pane);
        let bv = self.browser_views.get_mut(&pane).ok_or(NOT_READY)?;
        match &bv.view {
            Some(v) => {
                if !synced || bv.world.setup.is_some() {
                    return Err(NOT_READY);
                }
                v.navigate(url);
            }
            None => {
                bv.pending_url = Some(url.to_string());
                bv.stopped = false;
            }
        }
        Ok(())
    }

    /// A main-frame navigation of `pane` began (`Event::Starting`).
    pub(super) fn ai_browser_starting(&mut self, pane: PaneId, redirect: bool, nav: u64) {
        self.ai_browser.starting(pane, redirect, nav);
    }

    /// Navigation `nav` of `pane` finished (`Event::Navigated`; a person's own finishes nothing).
    pub(super) fn ai_browser_navigated(&mut self, pane: PaneId, nav: u64, ok: bool, status: i32) {
        let Some(act) = self.ai_browser.navigated(pane, nav, ok, status) else { return };
        let Act::Markdown { token, .. } = &act else { return };
        let token = *token;
        // The results page, as it stands now.
        match self.ai_browser_page(pane) {
            Ok(page) => self.ai_browser_act(act, Some(page)),
            Err(o) => self.ai_browser.read_refused(pane, token, o),
        }
    }

    /// The pane is closing: every call waiting on it answers a failure (before its view closes).
    pub(super) fn ai_browser_pane_gone(&mut self, pane: PaneId) {
        self.ai_browser.pane_gone(pane);
    }

    /// Stop: the chat's waiting browser calls are dropped.
    pub(super) fn ai_browser_stopped(&mut self, chat: &str) {
        self.ai_browser.forget_chat(chat);
    }

    /// Unlinking takes effect at once (`unlink_chat`): in-flight calls are answered "page changed".
    pub(super) fn ai_browser_unlinked(&mut self, chat: &str) {
        self.ai_browser.unlinked(chat);
    }

    pub(super) fn ai_browser_bind(&mut self, token: u64, chat: &str, request_id: &str) {
        self.ai_browser.bind(token, chat, request_id);
    }

    #[allow(dead_code)] // Kept as the token-level API; Stop forgets by chat.
    pub(super) fn ai_browser_forget(&mut self, token: u64) {
        self.ai_browser.forget(token);
    }

    /// The chat's turn gate (admin switch, "Enable generative AI features", per-host consent)
    /// for the endpoint of the model its turn runs on; false with no turn or model.
    pub(super) fn ai_browser_gate(&self, chat: &str) -> bool {
        let model = self.chat_agents.get(chat).and_then(|a| a.model.as_ref());
        model.is_some_and(|m| crate::chat_gate::gate_open(&self.globals, &crate::chat_gate::endpoint_of(m)))
    }

    fn ai_browser_turn_active(&self, chat: &str) -> bool {
        self.chat_broker.turn_in_progress(chat) || self.chat_agents.get(chat).is_some_and(|a| a.in_turn || a.driver.is_some() || a.running.is_some())
    }

    /// The page a pane shows now, as a binding; None when the pane is gone, private or not an
    /// ordinary page.
    fn ai_browser_now(&self, pane: PaneId) -> Option<Binding> {
        let io = self.panes.get(&pane)?.browser_io()?;
        let epoch = self.browser_views.get(&pane)?.reader.epoch;
        Page::open(pane, epoch, &io.url, "", io.dev_null).ok().map(|p| p.binding().clone())
    }

    /// Each `chat_tick` after `gate_tick`: forgets the calls of a chat whose gate closed or whose
    /// turn ended.
    pub(super) fn ai_browser_sweep(&mut self) {
        let keep: HashSet<String> = self.ai_browser.chats().into_iter().filter(|c| self.ai_browser_gate(c) && self.ai_browser_turn_active(c)).collect();
        self.ai_browser.sweep(&|chat| keep.contains(chat));
    }

    /// The answers ready for the one return path, `ai_tool_finished`.
    pub(super) fn ai_browser_finish(&mut self) -> Vec<(String, String, Outcome)> {
        if self.ai_browser.done.is_empty() {
            return Vec::new();
        }
        let now: HashMap<PaneId, Option<Binding>> = self.ai_browser.done.iter().filter_map(|(_, r)| if let Ready::Page(a) = r { Some(a.binding().pane()) } else { None }).map(|p| (p, self.ai_browser_now(p))).collect();
        self.ai_browser.finish(&|pane| now.get(&pane).cloned().flatten())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai_browser::{Page, Waits};
    use crate::ai_terminal::Outcome;

    fn plain(s: &str) -> Ready {
        Ready::Plain(Outcome { model: s.into(), notice: None })
    }
    fn call(pane: u64) -> BrowserCall {
        BrowserCall { bound: None, pane, step: Step::Load }
    }
    fn page_answer(epoch: u64, url: &str) -> (Ready, crate::ai_browser::Binding) {
        let p = Page::open(7, epoch, url, "T", false).unwrap();
        let a = p.web_search(&crate::ai_browser::Loaded::Ok, Some(Ok("# secret page")));
        (Ready::Page(a), p.binding().clone())
    }

    #[test]
    fn unlinking_answers_page_changed_at_once() {
        let mut s = Calls::default();
        s.calls.insert(1, call(7));
        s.bind(1, "C", "r1");
        s.calls.insert(2, call(7));
        s.bind(2, "D", "r2");
        let (ready, _) = page_answer(1, "https://a.example/");
        s.done.push((2, ready));
        s.unlinked("C");
        s.unlinked("D");
        let out = s.finish(&|_| None);
        assert_eq!(out.len(), 2);
        for (_, _, o) in &out {
            assert_eq!(o.model, crate::ai_browser::PAGE_CHANGED);
            assert!(!o.model.contains("secret"));
        }
    }

    #[test]
    fn an_unbound_token_never_finishes() {
        let mut s = Calls::default();
        s.calls.insert(1, call(7));
        s.done.push((1, plain("x")));
        assert!(s.finish(&|_| None).is_empty());
        assert!(s.calls.is_empty());
    }

    #[test]
    fn a_bound_token_finishes_once_for_its_chat() {
        let mut s = Calls::default();
        s.calls.insert(1, call(7));
        s.bind(1, "C", "r1");
        s.bind(1, "D", "r2");
        s.done.push((1, plain("x")));
        let out = s.finish(&|_| None);
        assert_eq!(out.len(), 1);
        assert_eq!((out[0].0.as_str(), out[0].1.as_str(), out[0].2.model.as_str()), ("C", "r1", "x"));
        assert!(s.finish(&|_| None).is_empty());
    }

    #[test]
    fn the_sweep_forgets_a_closed_chat_and_its_late_answers() {
        let mut s = Calls::default();
        s.calls.insert(1, call(7));
        s.calls.insert(2, call(7));
        s.bind(1, "C", "r1");
        s.bind(2, "D", "r2");
        s.waits.start_read(7, 1);
        s.waits.start_read(7, 2);
        s.sweep(&|chat| chat == "D");
        s.done.push((1, plain("late")));
        s.done.push((2, plain("kept")));
        assert!(!s.waits.read_done(7, 1), "a forgotten read is not waiting");
        assert!(s.waits.read_done(7, 2));
        let out = s.finish(&|_| None);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].0, "D");
        assert!(!s.calls.contains_key(&1));
    }

    #[test]
    fn a_page_answer_is_delivered_only_for_the_page_it_was_read_from() {
        let (ready, binding) = page_answer(3, "https://a.example/x");
        let mut s = Calls::default();
        s.calls.insert(1, call(7));
        s.bind(1, "C", "r1");
        s.done.push((1, ready));
        let out = s.finish(&|_| Some(binding.clone()));
        assert!(out[0].2.model.contains("secret page"));
        // The page navigated (epoch), changed origin, closed, or became private: nothing of it.
        for now in [Page::open(7, 4, "https://a.example/x", "", false).ok(), Page::open(7, 3, "https://b.example/x", "", false).ok(), None, Page::open(7, 3, "https://a.example/x", "", true).ok(), Page::open(7, 3, "iterm2-about:settings", "", false).ok()] {
            let (ready, _) = page_answer(3, "https://a.example/x");
            s.calls.insert(1, call(7));
            s.bind(1, "C", "r1");
            s.done.push((1, ready));
            let now = now.map(|p| p.binding().clone());
            let out = s.finish(&|_| now.clone());
            assert_eq!(out[0].2.model, crate::ai_browser::PAGE_CHANGED);
            assert!(!out[0].2.model.contains("secret"));
        }
    }

    #[test]
    fn waits_forget_a_token() {
        let mut w = Waits::default();
        w.start_load(7, 1);
        w.start_read(7, 2);
        w.forget(1);
        w.forget(2);
        assert!(w.pane_gone(7).is_empty());
    }

    #[test]
    fn the_kind_check_answers_the_fixed_texts() {
        use crate::ai_browser::{kind_refusal, NOT_A_BROWSER, NOT_A_TERMINAL};
        assert_eq!(kind_refusal(true, false).unwrap().model, NOT_A_BROWSER);
        assert_eq!(kind_refusal(false, true).unwrap().model, NOT_A_TERMINAL);
        assert!(kind_refusal(true, true).is_none() && kind_refusal(false, false).is_none());
    }

    // ---- Task 4: the runner (App-free: `Calls` returns the acts the App performs) ----

    const SEARCH: &str = "https://google.com/search?q=%@";

    fn wait(s: Started) -> (u64, Act) {
        match s {
            Started::Wait(t, a) => (t, a),
            Started::Done(o) => panic!("answered at once: {}", o.model),
        }
    }
    fn page() -> Page {
        Page::open(7, 1, "https://a.example/x", "T", false).unwrap()
    }
    fn ids(out: &[(String, String, Outcome)]) -> Vec<(&str, &str)> {
        out.iter().map(|(c, r, _)| (c.as_str(), r.as_str())).collect()
    }
    fn here(_: PaneId) -> Option<Binding> {
        Some(page().binding().clone())
    }

    #[test]
    fn browser_calls_finish_through_the_tick() {
        use crate::ai_browser::*;
        // A load finishes for its own navigation only.
        let mut s = Calls::default();
        let (t, act) = wait(s.start_load(7, "https://a"));
        assert_eq!(act, Act::Navigate { pane: 7, token: t, url: "https://a".into() });
        s.bind(t, "C", "r1");
        s.starting(7, false, 5);
        assert!(s.navigated(7, 9, true, 0).is_none(), "a person's navigation finishes nothing");
        assert!(s.finish(&here).is_empty());
        assert!(s.navigated(7, 5, true, 0).is_none());
        let out = s.finish(&here);
        assert_eq!(ids(&out), vec![("C", "r1")]);
        assert_eq!((out[0].2.model.as_str(), out[0].2.notice.as_deref()), (LOADED, Some(N_NAV_OK)));
        // A failed load answers the table text.
        let (t, _) = wait(s.start_load(7, "https://a"));
        s.bind(t, "C", "r2");
        s.starting(7, false, 6);
        s.navigated(7, 6, false, 13);
        assert_eq!(s.finish(&here)[0].2.model, format!("{LOAD_FAILED_PREFIX}the host name could not be resolved"));
        // A second load interrupts the first at once.
        let (a, _) = wait(s.start_load(7, "https://a"));
        s.bind(a, "C", "r3");
        let (b, _) = wait(s.start_load(7, "https://b"));
        s.bind(b, "C", "r4");
        let out = s.finish(&here);
        assert_eq!(ids(&out), vec![("C", "r3")]);
        assert_eq!(out[0].2.model, format!("{LOAD_FAILED_PREFIX}{INTERRUPTED}"));
        // Other schemes and local files are refused and navigate nothing.
        for url in ["javascript:x", "file:///C:/Users/u/.ssh/id_ed25519", "https://u:p@h/", "iterm2-about:settings"] {
            let Started::Done(o) = s.start_load(8, url) else { panic!("{url}") };
            assert_eq!(o.notice.as_deref(), Some(N_NAV_FAILED));
        }
        assert!(s.calls.len() <= 2, "a refusal mints no token");
    }

    #[test]
    fn web_search_loads_then_reads_the_results() {
        use crate::ai_browser::*;
        let mut s = Calls::default();
        let (t, act) = wait(s.start_search(7, SEARCH, "rust lang"));
        assert_eq!(act, Act::Navigate { pane: 7, token: t, url: "https://google.com/search?q=rust%20lang".into() });
        s.bind(t, "C", "r1");
        s.starting(7, false, 3);
        s.starting(7, true, 4);
        let next = s.navigated(7, 3, true, 0);
        assert_eq!(next, Some(Act::Markdown { pane: 7, token: t, skip_chrome: true }));
        assert!(s.finish(&here).is_empty(), "nothing until the page is read");
        s.read_answer(7, t, &page(), Ok("# hits"));
        let out = s.finish(&here);
        assert!(out[0].2.model.contains("# hits"));
        assert_eq!(out[0].2.notice.as_deref(), Some(N_SEARCH_OK));
        // A failed load answers unavailable and reads nothing.
        let (t, _) = wait(s.start_search(7, SEARCH, "x"));
        s.bind(t, "C", "r2");
        s.starting(7, false, 8);
        assert!(s.navigated(7, 8, false, 6).is_none());
        assert_eq!(s.finish(&here)[0].2.model, SEARCH_UNAVAILABLE);
        // A load while the search waits answers the search unavailable.
        let (t, _) = wait(s.start_search(7, SEARCH, "x"));
        s.bind(t, "C", "r3");
        wait(s.start_load(7, "https://b"));
        assert_eq!(s.finish(&here)[0].2.model, SEARCH_UNAVAILABLE);
        // A template that is not a web page navigates nothing.
        for tpl in ["file:///C:/%@", "javascript:%@", "https://%@@evil/"] {
            let Started::Done(o) = s.start_search(9, tpl, "q") else { panic!("{tpl}") };
            assert_eq!(o.model, SEARCH_UNAVAILABLE);
        }
        // The query is encoded, never raw.
        let (_, act) = wait(s.start_search(9, SEARCH, "a b&c#d\u{e9}"));
        let Act::Navigate { url, .. } = act else { panic!() };
        assert!(!url.contains(' ') && !url.contains('#') && !url.contains('\u{e9}'), "{url}");
    }

    #[test]
    fn read_and_find_answer_from_the_markdown_of_their_page() {
        let mut s = Calls::default();
        let (t, act) = wait(s.start_read(7, Step::Read { start: 0, count: 5 }));
        assert_eq!(act, Act::Markdown { pane: 7, token: t, skip_chrome: false });
        s.bind(t, "C", "r1");
        s.read_answer(7, t, &page(), Ok("# A\nRust here"));
        let (f, _) = wait(s.start_read(7, Step::Find { query: "rust".into() }));
        s.bind(f, "C", "r2");
        s.read_answer(7, f, &page(), Ok("# A\nRust here"));
        let out = s.finish(&here);
        assert!(out[0].2.model.contains("# A") && out[0].2.model.contains("[There are a total of 2 lines"));
        assert!(out[1].2.model.contains("\"matchingLine\":\"Rust here\""));
        // A read that fails says so; an unbound one never appears.
        let (u, _) = wait(s.start_read(7, Step::Read { start: 0, count: 5 }));
        s.read_answer(7, u, &page(), Err("boom".into()));
        assert!(s.finish(&here).is_empty());
        // A second answer for the same token is dropped.
        s.read_answer(7, t, &page(), Ok("again"));
        assert!(s.finish(&here).is_empty());
    }

    #[test]
    fn a_closed_pane_answers_its_calls() {
        use crate::ai_browser::*;
        let mut s = Calls::default();
        let (l, _) = wait(s.start_load(7, "https://a"));
        let (r, _) = wait(s.start_read(7, Step::Read { start: 0, count: 5 }));
        let (o, _) = wait(s.start_load(8, "https://a"));
        s.bind(l, "C", "r1");
        s.bind(r, "C", "r2");
        s.bind(o, "D", "r3");
        s.pane_gone(7);
        let out = s.finish(&here);
        assert_eq!(out.len(), 2);
        let texts: Vec<&str> = out.iter().map(|x| x.2.model.as_str()).collect();
        assert!(texts.contains(&format!("{LOAD_FAILED_PREFIX}{PANE_GONE}").as_str()) && texts.contains(&format!("{READ_FAILED_PREFIX}{PANE_GONE}").as_str()));
        // The read's markdown arriving afterwards is dropped.
        s.read_answer(7, r, &page(), Ok("late"));
        assert!(s.finish(&here).is_empty());
        assert!(s.waits.navigated(8, 1).is_none());
    }

    #[test]
    fn a_forgotten_call_reads_nothing_and_answers_nothing() {
        let mut s = Calls::default();
        let (t, _) = wait(s.start_search(7, SEARCH, "x"));
        s.bind(t, "C", "r1");
        s.starting(7, false, 3);
        let (r, _) = wait(s.start_read(7, Step::Read { start: 0, count: 5 }));
        s.bind(r, "C", "r2");
        s.sweep(&|_| false);
        assert!(s.navigated(7, 3, true, 0).is_none(), "the search's page is not read");
        s.read_answer(7, r, &page(), Ok("late"));
        assert!(s.finish(&here).is_empty() && s.calls.is_empty());
        // Stop forgets by chat.
        let (t, _) = wait(s.start_load(7, "https://a"));
        s.bind(t, "C", "r3");
        s.forget_chat("C");
        assert!(s.calls.is_empty() && s.waits.pane_gone(7).is_empty());
    }

    #[test]
    fn a_load_that_cannot_start_ends_with_a_failure() {
        let mut s = Calls::default();
        let (t, _) = wait(s.start_load(7, "https://a"));
        s.bind(t, "C", "r1");
        s.load_not_started(7, t, "the browser is not ready, try again");
        assert!(s.finish(&here)[0].2.model.ends_with("not ready, try again"));
        s.load_not_started(7, t, "again");
        assert!(s.finish(&here).is_empty());
    }

    /// The production code without its comments.
    fn prod() -> String {
        let src = include_str!("ai_browser_app.rs");
        src[..src.find("#[cfg(test)]").unwrap()].lines().filter(|l| !l.trim_start().starts_with("//")).collect::<Vec<_>>().join("\n")
    }

    /// The runner navigates only through the guarded `navigate` and reads only through the one
    /// isolated-world markdown entry, wired to the right events.
    #[test]
    fn the_runner_stays_on_the_guarded_paths() {
        let p = prod();
        assert!(!p.contains("navigate_tandem") && !p.contains("defer_load") && !p.contains("browser_navigate("), "a model's URL never takes Tandem's own load");
        assert_eq!(p.matches("v.navigate(url)").count(), 1);
        assert_eq!(p.matches("browser_page_markdown_with(").count(), 1);
        for banned in ["browser_page_call", "browser_devtools", "execute_script", "password", "autofill", "cookie", "log::", "dlog"] {
            assert!(!p.contains(banned), "{banned}");
        }
        let b = include_str!("browser_app.rs");
        let b = &b[..b.find("#[cfg(test)]").unwrap()];
        assert!(b.contains("self.ai_browser_starting(pane, *redirect, *nav)") && b.contains("self.ai_browser_navigated(pane, *nav, *ok, *status)"));
        let m = include_str!("mod.rs");
        assert!(m.contains("self.ai_browser_pane_gone(id);\n        self.close_browser_view(id);"), "answered before the view closes");
        assert!(include_str!("chat_app.rs").contains("self.ai_browser_stopped(chat);"));
    }

    /// The markdown script drops secret fields from its copy of the page.
    #[test]
    fn the_markdown_script_drops_secret_fields() {
        let js = crate::browser_reader::markdown_body(false);
        for needle in ["input[type=\"password\" i]", "input[type=\"hidden\" i]", "autocomplete*=\"password\" i", "[hidden]"] {
            assert!(js.contains(needle), "{needle}");
        }
    }
}
