//! The "Session Parameter" prompt (13y2 Task 9e): a new session whose command, name or folder
//! still holds a `$$NAME$$` asks for each name, one window at a time, before its program starts
//! (`iTermSessionFactory.m:215-297`, `:484-523`). The pure half is `session_params`.
//!
//! The pane exists while the prompt is up (iTerm2 made the session before asking) but is idle:
//! its program starts after the last answer, through the restart road with the answered profile.
//! Cancel, or the window closed, closes the pane without the close question (`SF:223-242`).
//! A restored arrangement never prompts: every name is "" (`PTYSession.m:2516`, canPrompt NO).
//!
//! Security (m13y2 B2): answers are the person's typed text, inserted as typed (no shell
//! quoting, as iTerm2) and never into a URL (14e-D43); names are untrusted and shown cleaned and
//! capped; the window drops input in its first second (`InputGuard`), since a URL, script or
//! hotkey launch can raise it while the person types elsewhere; nothing typed is logged.

use super::*;
use crate::prompt_ui::{InputGuard, PromptResult, PromptWindow};
use crate::session_params as sp;

/// One session waiting for its answers.
pub(super) struct Pending {
    pub pane: PaneId,
    queue: sp::Queue,
    /// The profile the session was made with; the answers go into a copy.
    profile: Profile,
    window: Option<PromptWindow>,
    guard: InputGuard,
    cancelled: bool,
}

/// What `start_pane_as` does next.
pub(super) enum Gate {
    /// Start now, with this profile when the texts changed (an escape, or empty answers).
    Start(Option<Profile>),
    /// The pane is installed idle and the prompt will start it.
    Deferred,
    /// Not started, and why.
    Refused(String),
}

/// `p` with the answers in its command, name and custom folder.
pub(super) fn answered(p: &Profile, answers: &[(String, String)]) -> Profile {
    let mut out = p.clone();
    for key in ["Command", "Name", "Working Directory"] {
        let before = p.string(key);
        let after = sp::apply(&before, answers);
        if after != before {
            out.set(key, toml::Value::String(after));
        }
    }
    out
}

/// The three texts a session start reads from `p`: command, name and custom folder.
fn texts(p: &Profile) -> (String, String, String) {
    let dir = if p.string("Custom Directory") == "Yes" { p.string("Working Directory") } else { String::new() };
    (profile::command_line(p, "", None), p.string("Name"), dir)
}

impl App {
    /// Called by `start_pane_as` for a profile start. A command override or a handed-over console
    /// is not profile text and never prompts.
    pub(super) fn param_gate(&mut self, id: PaneId, rect: Rect, guid: Option<&str>, p: &Profile) -> Gate {
        if self.start_handoff.is_some() || self.start_command_override.is_some() {
            return Gate::Start(None);
        }
        let (command, name, dir) = texts(p);
        if ![&command, &name, &dir].iter().any(|t| t.contains("$$")) {
            return Gate::Start(None);
        }
        let vars = sp::missing(&command, &name, &dir, &[]);
        if vars.is_empty() {
            return Gate::Start(Some(answered(p, &[])));
        }
        if self.restoring_session {
            let answers: Vec<(String, String)> = vars.into_iter().map(|v| (v, sp::answer_when_disabled())).collect();
            return Gate::Start(Some(answered(p, &answers)));
        }
        if let Some(why) = sp::refusal(&vars) {
            return Gate::Refused(why);
        }
        let cell = self.profile_cell(p);
        let (cols, rows) = self.grid_size_in(rect, cell, self.pane_room_for(id, p));
        self.install_pane(id, Pane::idle(id, cols, rows), p, false);
        self.param_prompts.push(Pending { pane: id, queue: sp::Queue::new(vars), profile: p.clone(), window: None, guard: InputGuard::new(Instant::now()), cancelled: false });
        // The guid is read again from the pane when the program starts.
        let _ = guid;
        Gate::Deferred
    }

    /// Opens the next waiting name's window, closes the sessions whose prompt was cancelled and
    /// forgets the ones whose pane is gone. Called every tick.
    pub(super) fn param_prompt_next(&mut self, event_loop: &ActiveEventLoop) {
        self.param_prompts.retain(|p| self.panes.contains_key(&p.pane));
        while let Some(at) = self.param_prompts.iter().position(|p| p.cancelled) {
            let gone = self.param_prompts.remove(at);
            self.close_pane(gone.pane, event_loop);
        }
        // One window at a time, the oldest session first.
        if self.param_prompts.iter().any(|p| p.window.is_some()) {
            return;
        }
        let Some(first) = self.param_prompts.first_mut() else { return };
        let Some(var) = first.queue.current().map(str::to_string) else { return };
        let Some(g) = self.gpu.as_ref() else { return };
        match PromptWindow::open(event_loop, &g.instance, &g.adapter, &g.device, &g.queue, sp::TITLE, &sp::label(&var), "") {
            Ok(w) => {
                w.focus();
                first.guard = InputGuard::new(Instant::now());
                first.window = Some(w);
            }
            Err(e) => {
                first.cancelled = true;
                self.banners.set("window", format!("Could not open the window: {e}"));
            }
        }
    }

    /// Handles an event for the prompt window. False when `window` is not it.
    pub(super) fn param_prompt_event(&mut self, window: winit::window::WindowId, event: &WindowEvent) -> bool {
        let Some(at) = self.param_prompts.iter().position(|p| p.window.as_ref().is_some_and(|w| w.id() == window)) else { return false };
        let pending = &mut self.param_prompts[at];
        let Some(w) = pending.window.as_mut() else { return false };
        let now = Instant::now();
        pending.guard.window_event(event, now);
        match event {
            WindowEvent::CloseRequested => {
                pending.window = None;
                pending.cancelled = true;
            }
            WindowEvent::RedrawRequested => match w.redraw() {
                PromptResult::Open | PromptResult::Extra(_) => {}
                PromptResult::Cancel => {
                    pending.window = None;
                    pending.cancelled = true;
                }
                PromptResult::Ok(typed) => {
                    pending.window = None;
                    if pending.queue.answer(&typed) {
                        let done = self.param_prompts.remove(at);
                        self.param_launch(done);
                    }
                }
            },
            other => {
                // A key or click already in flight for another window cannot answer this one.
                if !pending.guard.drops(other, now) {
                    w.event(other);
                }
            }
        }
        true
    }

    /// Every name answered: the program starts in the waiting pane.
    fn param_launch(&mut self, done: Pending) {
        let answers = done.queue.answers().to_vec();
        self.param_profile = Some(answered(&done.profile, &answers));
        self.restart_confirmed(done.pane);
        self.param_profile = None;
        self.redraw();
    }

    /// A pane that closed takes its waiting prompt with it.
    pub(super) fn param_pane_gone(&mut self, id: PaneId) {
        self.param_prompts.retain(|p| p.pane != id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body<'a>(src: &'a str, name: &str) -> &'a str {
        let rest = src.split(name).nth(1).unwrap_or_else(|| panic!("{name}"));
        rest.split("\n    }\n").next().unwrap()
    }

    #[test]
    fn answered_profile_follows_iterm2() {
        let mut p = Profile::default();
        p.set("Custom Command", toml::Value::String("Yes".into()));
        p.set("Command", toml::Value::String("ssh $$TARGET$$".into()));
        p.set("Name", toml::Value::String("to $$TARGET$$".into()));
        let q = answered(&p, &[("$$TARGET$$".into(), "box".into())]);
        assert_eq!(q.string("Command"), "ssh box");
        assert_eq!(q.string("Name"), "to box");
        let q = answered(&p, &[("$$TARGET$$".into(), String::new())]);
        assert_eq!(q.string("Command"), "ssh ", "a restored session's name is empty");
        let mut e = Profile::default();
        e.set("Custom Command", toml::Value::String("Yes".into()));
        e.set("Command", toml::Value::String("echo $$$$".into()));
        assert_eq!(answered(&e, &[]).string("Command"), "echo $$");
        assert!(sp::missing(&texts(&e).0, "", "", &[]).is_empty(), "only the escape: no prompt");
    }

    #[test]
    fn parameter_prompt_follows_iterm2() {
        let m = include_str!("mod.rs");
        let me = include_str!("session_params_app.rs");
        // start_pane_as asks the gate after the browser check, before any program starts.
        let start = body(m, "    fn start_pane_as(");
        assert!(start.find("is_browser_profile").unwrap() < start.find("self.param_gate(").unwrap());
        assert!(start.find("self.param_gate(").unwrap() < start.find("self.spawn_pane(").unwrap());
        assert!(start.contains("Gate::Deferred => return true"), "the pane exists, its program waits");
        assert!(start.contains("Gate::Refused(why)"));
        // The profile with the answers is what spawn_pane reads.
        assert!(body(m, "    fn profile(&self, guid").contains("self.param_profile"));
        // The prompt is a native window with the guard, one at a time, titled and labelled by the pure half.
        let next = body(me, "pub(super) fn param_prompt_next");
        assert!(next.contains("sp::TITLE") && next.contains("sp::label(&var)") && next.contains("any(|p| p.window.is_some())"));
        let ev = body(me, "pub(super) fn param_prompt_event");
        assert!(ev.contains("pending.guard.drops(other, now)") && ev.contains("pending.cancelled = true"));
        assert!(ev.contains("PromptResult::Cancel") && ev.contains("WindowEvent::CloseRequested"));
        // Cancel closes the pane through close_pane (no close question); the last answer restarts it with the answers.
        assert!(next.contains("self.close_pane(gone.pane, event_loop)"));
        assert!(body(me, "fn param_launch").contains("self.restart_confirmed(done.pane)"));
        // The window chain, the tick, the close hook and the restore flag.
        assert!(m.contains("self.param_prompt_event(id, &event)"));
        assert!(m.contains("self.param_prompt_next(event_loop)"));
        assert!(body(m, "    fn close_pane(").contains("self.param_pane_gone(id)"));
        let restore = include_str!("arrange_ui.rs");
        let r = restore.find("self.restoring_session = true").expect("restore sets the flag");
        assert!(r < restore.find("None => self.start_pane(area, guid.as_deref())").unwrap());
        assert!(restore.contains("self.restoring_session = false"));
        // Nothing typed is logged.
        assert!(!me.split("#[cfg(test)]").next().unwrap().contains("rlog!"));
        // No URL substitution anywhere in the prompt.
        assert!(!me.split("#[cfg(test)]").next().unwrap().contains("url_open"));
    }
}
