//! The script seam of test builds (13x4 Task 2, review M8): where a browser pane has no web view, the
//! three runners (`browser_js_call`, `browser_page_call`, `browser_devtools`) record the call and
//! answer from a queue at once, so a test sees which runner a feature used and what it sent. The
//! AI browser tools plan consumes it, and 13x5 reuses it (one seam).

// The seam is test plumbing: release builds compile it but never use it.
#![allow(dead_code)]

use super::browser_scripts_app::JsDone;
use super::*;
use serde_json::Value;
use std::collections::VecDeque;

/// Which runner a call went through.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Runner {
    /// The isolated world (`browser_js_call`).
    World,
    /// The page world (`browser_page_call`).
    Page,
    /// DevTools (`browser_devtools`).
    DevTools,
}

/// The recorded calls and the answers queued for them.
#[derive(Default)]
pub(super) struct ScriptSeam {
    /// The pane, the runner, and the body (for DevTools: the method, a space, the params JSON).
    pub(super) calls: Vec<(PaneId, Runner, String)>,
    pub(super) answers: VecDeque<Result<Value, String>>,
}

impl ScriptSeam {
    /// Records the call and answers with the next queued answer.
    pub(super) fn run(&mut self, pane: PaneId, runner: Runner, text: &str) -> Result<Value, String> {
        self.calls.push((pane, runner, text.to_string()));
        self.answers.pop_front().unwrap_or_else(|| Err("no answer queued".into()))
    }
}

// The seam is per test thread (an `App` cannot be built in a unit test; the tests of a thread that
// has one queue answers here and read `calls`), so it needs no field on `App`.
#[cfg(test)]
thread_local! {
    pub(super) static SEAM: std::cell::RefCell<ScriptSeam> = std::cell::RefCell::new(ScriptSeam::default());
}

impl App {
    /// A runner's first step: in a test build a pane whose view is None is answered by the seam (the
    /// answer goes to `done` and None comes back); otherwise `done` comes back to run for real.
    pub(super) fn seam_intercept(&mut self, pane: PaneId, runner: Runner, text: &str, done: JsDone) -> Option<JsDone> {
        #[cfg(test)]
        if self.browser_views.get(&pane).is_some_and(|b| b.view.is_none()) {
            let answer = SEAM.with(|s| s.borrow_mut().run(pane, runner, text));
            done(self, pane, answer);
            return None;
        }
        let _ = (pane, runner, text);
        Some(done)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_seam_records_and_answers_in_order() {
        let mut s = ScriptSeam::default();
        s.answers.push_back(Ok(json!(true)));
        s.answers.push_back(Err("boom".into()));
        assert_eq!(s.run(3, Runner::World, "a"), Ok(json!(true)));
        assert_eq!(s.run(3, Runner::Page, "b"), Err("boom".into()));
        assert_eq!(s.run(4, Runner::DevTools, "Page.getResourceTree {}"), Err("no answer queued".into()), "none queued");
        assert_eq!(s.calls, vec![(3, Runner::World, "a".into()), (3, Runner::Page, "b".into()), (4, Runner::DevTools, "Page.getResourceTree {}".into())]);
    }

    /// The runners consult the seam first, in test builds only, and only for a pane with no view.
    #[test]
    fn the_runners_consult_the_seam() {
        let src = include_str!("browser_scripts_app.rs").split("\n#[cfg(test)]\nmod tests").next().unwrap().to_string();
        for (name, runner) in [("pub(super) fn browser_js_call(", "Runner::World"), ("pub(super) fn browser_page_call(", "Runner::Page"), ("pub(super) fn browser_page_call_capped(", "Runner::Page"), ("pub(super) fn browser_devtools(", "Runner::DevTools"), ("pub(super) fn browser_devtools_capped(", "Runner::DevTools")] {
            let body = &src[src.find(name).unwrap()..];
            let body = &body[..body.find("\n    }\n").unwrap()];
            assert!(body.contains("self.seam_intercept(pane, super::browser_test_seam::") && body.contains(&format!("{runner}, ")), "{name}");
            assert!(body[body.find(") {\n").unwrap() + 4..].trim_start().starts_with("let Some(done) = self.seam_intercept"), "{name}: the seam is the first step");
        }
    }
}
