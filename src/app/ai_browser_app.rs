//! Browser tool calls on a pane (13x AI browser tools). Task 3 holds the return path: the
//! waiting calls by token, the gate, the sweep and the delivery re-check. Task 4 adds the runner
//! (`ai_browser_start` answers only `get_current_url` until then; every other browser call is
//! unavailable, which fails closed).

use super::ai_tools_app::ToolStart;
use super::App;
use crate::ai_browser::{Answer, Binding, Page, Waits};
use crate::ai_terminal::Outcome;
use crate::ai_tools::Call;
use crate::layout::PaneId;
use std::collections::{HashMap, HashSet};

/// What a waiting call is doing.
#[allow(dead_code)] // The runner (Task 4) builds them.
pub(super) enum Step {
    Find { query: String },
    Load,
    Search,
    Read { start: i64, count: i64 },
}

/// One browser call under a token: bound to its (chat, request id) once `ai_run_tool` has it.
pub(super) struct BrowserCall {
    pub bound: Option<(String, String)>,
    #[allow(dead_code)] // The runner (Task 4).
    pub pane: PaneId,
    #[allow(dead_code)]
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

impl Calls {
    #[allow(dead_code)] // The runner (Task 4).
    pub fn mint(&mut self, pane: PaneId, step: Step) -> u64 {
        self.next += 1;
        self.calls.insert(self.next, BrowserCall { bound: None, pane, step });
        self.next
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
    /// A browser call on `pane`. Only `get_current_url` answers until the runner lands.
    pub(super) fn ai_browser_start(&mut self, pane: PaneId, call: &Call) -> ToolStart {
        let Some(io) = self.panes.get(&pane).and_then(|p| p.browser_io()) else { return ToolStart::Done(Outcome { model: crate::ai_browser::NOT_A_BROWSER.to_string(), notice: None }) };
        match call {
            Call::GetUrl => ToolStart::Done(crate::ai_browser::current_url_outcome(&io.url, io.dev_null)),
            _ => ToolStart::Done(Outcome { model: super::ai_tools_app::NOT_AVAILABLE.to_string(), notice: None }),
        }
    }

    pub(super) fn ai_browser_bind(&mut self, token: u64, chat: &str, request_id: &str) {
        self.ai_browser.bind(token, chat, request_id);
    }

    #[allow(dead_code)] // Stop and the runner (Task 4).
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
        let epoch = self.browser_views.get(&pane).map_or(0, |b| b.reader.epoch);
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
}
