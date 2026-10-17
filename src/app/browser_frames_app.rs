//! The frame graph on the app side (milestone 13x2, Task 2): what `browser_frames::Frames` plans is
//! evaluated in the Termurion world of the frame it names, and every answer comes back to the graph.
//! Web content is untrusted: only a message of the isolated world is read, and nothing here runs a
//! page-world script.

use super::*;
use crate::browser_frames::Act;
use crate::browser_js::{Routed, World};

impl App {
    /// One message of `graph-discovery.js`. Anything that is not the isolated world's returns before
    /// the message is read (R13x23-2).
    pub(super) fn browser_graph_message(&mut self, pane: PaneId, r: &Routed) {
        if r.world != World::Client {
            return;
        }
        let ready = self.client_open(pane);
        let Some(bv) = self.browser_views.get_mut(&pane) else { return };
        let acts = bv.graph.handle(r, ready, &mut crate::browser_js::new_secret);
        self.graph_run(pane, acts);
    }

    /// An answer to one of the graph's evaluations; true when the token was the graph's.
    pub(super) fn graph_script_done(&mut self, pane: PaneId, token: u64, value: &Result<serde_json::Value, String>) -> bool {
        let Some(bv) = self.browser_views.get_mut(&pane) else { return false };
        let (own, acts) = bv.graph.answered(token, value);
        self.graph_run(pane, acts);
        own
    }

    /// Every graph script runs wrapped, in the Termurion world, and each answer is `graph_script_done`'s.
    fn graph_run(&mut self, pane: PaneId, acts: Vec<Act>) {
        for act in acts {
            let token = act.token;
            self.browser_eval_in_frame(pane, act.frame, &act.code, Box::new(move |app, pane, r| {
                app.graph_script_done(pane, token, &r);
            }));
        }
    }
}

#[cfg(test)]
mod tests {
    fn prod(src: &str) -> &str {
        &src[..src.find("#[cfg(test)]").unwrap()]
    }

    /// R13x23-2, C2: the graph arm returns before anything is parsed for a message that is not the
    /// isolated world's, and Termurion evaluates only what `Frames` planned.
    #[test]
    fn graph_refuses_page_world() {
        let src = include_str!("browser_frames_app.rs");
        let p = prod(src);
        let f = &p[p.find("pub(super) fn browser_graph_message").unwrap()..p.find("pub(super) fn graph_script_done").unwrap()];
        assert!(f.find("World::Client").unwrap() < f.find(".handle(").unwrap(), "the world is checked before the message is read");
        assert!(f.contains("return;") && f.contains("self.client_open(pane)"));
        // The one evaluation site runs the planned acts through the Termurion world, never the page's.
        assert_eq!(p.matches("browser_eval_in_frame(").count(), 1);
        assert!(!p.contains("execute_script") && !p.contains("browser_eval(pane"));
        let router = include_str!("browser_scripts_app.rs");
        let router = prod(router);
        let m = &router[router.find("pub(super) fn browser_message").unwrap()..router.find("pub(super) fn browser_page_open").unwrap()];
        assert!(m.contains("browser_frames::HANDLER") && m.contains("browser_graph_message(pane, &r)"));
        // With no isolated world the script is not offered at all.
        let scripts = &router[router.find("pub(super) fn browser_user_scripts").unwrap()..router.find("pub(super) fn browser_view_secret").unwrap()];
        assert!(scripts.contains("ClientWorld::Unavailable") && scripts.contains("always_on_scripts("));
    }

    /// The handlers of the always-on scripts are wired as iTerm2 wires them.
    #[test]
    fn always_on_handlers_follow_iterm2() {
        let router = include_str!("browser_scripts_app.rs");
        let router = prod(router);
        let m = &router[router.find("pub(super) fn browser_message").unwrap()..router.find("pub(super) fn browser_page_open").unwrap()];
        assert!(m.contains("browser_js::always_on(&r,"), "one pure decision for the handlers");
        let a = &router[router.find("fn browser_always_on").unwrap()..];
        let a = &a[..a.find("\n    }\n").unwrap()];
        // Selection: stored, copied only through the gate, with the global, to the clipboard and paste history.
        assert!(a.contains("bv.selection = Some(") && a.contains("selection_gate.accept(") && a.contains("globals.bool(\"CopySelection\")"));
        assert!(a.contains("set_clipboard_text(") && a.contains("remember_paste("));
        assert!(a.contains("selection_gate.down(") && a.contains("selection_gate.up("));
        assert!(a.contains("bv.editing = e"));
        // The console arm counts and never logs the text.
        assert!(a.contains("crate::rlog!(\"browser console: {} bytes, level {}\", size, level)"));
        assert!(!a.contains("msg") && !a.contains("body"));
        // A main-frame navigation resets editing and the selection and the graph; a destroyed context
        // forgets its frame.
        let ev = &router[router.find("pub(super) fn browser_script_event").unwrap()..router.find("pub(super) fn browser_message").unwrap()];
        let nav = &ev[ev.find("Event::Starting { redirect: false, .. } =>").unwrap()..];
        assert!(nav.contains("bv.editing = false") && nav.contains("graph.will_navigate()") && nav.contains("bv.selection = None"));
        let d = &router[router.find("fn browser_devtools_event").unwrap()..router.find("fn browser_attached").unwrap()];
        assert!(d.contains("graph.gone(") && !d.contains("fn_frames_lost"));
        // The new files' log sites are listed.
        assert!(include_str!("../dlog.rs").contains("(\"app/browser_scripts_app.rs\", 5)"));
    }
}
