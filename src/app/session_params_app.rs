//! The "Session Parameter" prompt (13y2 Task 9e): a new session whose command, name or folder
//! still holds a `$$NAME$$` asks for each name, one window at a time, before its program starts
//! (`iTermSessionFactory.m:215-297`, `:484-523`). The pure half is `session_params`.
//!
//! The pane exists while the prompt is up (iTerm2 made the session before asking) but is idle:
//! its program starts after the last answer, in the pane's own window, through the restart road
//! with the answered profile. The answers stay with the pane (`param_answers`, iTerm2's
//! `_substitutions`, `PTYSession.m:3507-3511`), so Restart Session reruns the answered program
//! (`PS:4889-4899`), and the answered Name is the session's own value (`SF:260-268`).
//! Cancel, or the window closed, closes the pane without the close question (`SF:223-242`).
//! A restored arrangement never prompts: every name is "" (`PTYSession.m:2516`, canPrompt NO).
//!
//! Security (m13y2 B2): answers are the person's typed text, inserted as typed (no shell
//! quoting, as iTerm2) and never into a URL (14e-D43); names are untrusted and shown cleaned and
//! capped; the window drops input in its first second (`InputGuard`), since a URL, script or
//! hotkey launch can raise it while the person types elsewhere; nothing typed is logged (the start
//! log line names the shared profile and the program before the answers, `start_log_command`).

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
    /// Start now; with the answers (an escape alone, or a restore's empty ones) when the texts change.
    Start(Option<Vec<(String, String)>>),
    /// The pane is installed idle and the prompt will start it.
    Deferred,
    /// Not started, and why.
    Refused(String),
}

/// `p` with the answers in its command, name and custom folder.
pub(super) fn answered(p: &Profile, answers: &[(String, String)]) -> Profile {
    answered_keys(p, answers, &["Command", "Name", "Working Directory"])
}

fn answered_keys(p: &Profile, answers: &[(String, String)], keys: &[&str]) -> Profile {
    let mut out = p.clone();
    for key in keys {
        let before = p.string(key);
        let after = sp::apply(&before, answers);
        if after != before {
            out.set(key, toml::Value::String(after));
        }
    }
    out
}

/// The three texts a session start reads from `p`: command, name and working directory. The folder
/// is scanned whatever Custom Directory says (`SF:103`, `:279`).
fn texts(p: &Profile) -> (String, String, String) {
    (profile::command_line(p, "", None), p.string("Name"), p.string("Working Directory"))
}

/// The session's own Name after the answers (`SF:260-268`), when they change it.
pub(super) fn answered_name(p: &Profile, answers: &[(String, String)]) -> Option<String> {
    let before = p.string("Name");
    let after = sp::apply(&before, answers);
    (after != before).then_some(after)
}

/// The profile a (re)start of a pane with `answers` runs: `base` with the answers in, or None for a
/// pane that never had a parameter.
pub(super) fn restart_profile(base: &Profile, answers: Option<&[(String, String)]>) -> Option<Profile> {
    // The session copy's Name already holds the answers (`answered_name` at the first start): a
    // restart does not scan it again, so an answer that reads `$$OTHER$$` stays text (9e review N3).
    answers.map(|a| answered_keys(base, a, &["Command", "Working Directory"]))
}

/// The command whose program the start log line names (review I4): the command before the answers
/// for a start that has them, so nothing typed into the prompt reaches the log. The line names the
/// shared profile (a session's own Name may hold an answer).
pub(super) fn start_log_command<'a>(cmd: &'a str, unanswered: Option<&'a str>) -> &'a str {
    unanswered.unwrap_or(cmd)
}

impl App {
    /// Called by `start_pane_as` for a profile start. A command override or a handed-over console
    /// is not profile text and never prompts.
    pub(super) fn param_gate(&mut self, id: PaneId, rect: Rect, p: &Profile) -> Gate {
        if self.start_handoff.is_some() || self.start_command_override.is_some() {
            return Gate::Start(None);
        }
        let (command, name, dir) = texts(p);
        if ![&command, &name, &dir].iter().any(|t| t.contains("$$")) {
            return Gate::Start(None);
        }
        let vars = sp::missing(&command, &name, &dir, &[]);
        if vars.is_empty() {
            return Gate::Start(Some(Vec::new()));
        }
        if self.restoring_session {
            return Gate::Start(Some(vars.into_iter().map(|v| (v, sp::answer_when_disabled())).collect()));
        }
        if let Some(why) = sp::refusal(&vars) {
            return Gate::Refused(why);
        }
        let cell = self.profile_cell(p);
        let (cols, rows) = self.grid_size_in(rect, cell, self.pane_room_for(id, p));
        self.install_pane(id, Pane::idle(id, cols, rows), p, false);
        self.param_prompts.push(Pending { pane: id, queue: sp::Queue::new(vars), profile: p.clone(), window: None, guard: InputGuard::new(Instant::now()), cancelled: false });
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

    /// Every name answered: the program starts in the waiting pane, in its own window (review I3:
    /// `restart_confirmed` finds a pane's rectangle in the current window only).
    fn param_launch(&mut self, done: Pending) {
        let answers = done.queue.answers().to_vec();
        self.param_settle(done.pane, &done.profile, answers);
        self.enter_pane_window(done.pane);
        self.restart_confirmed(done.pane);
        self.redraw();
    }

    /// A session that started with `answers` keeps them for its restarts and wears the answered
    /// Name as its own value (iTerm2's `_substitutions` and `setSessionSpecificProfileValues`).
    pub(super) fn param_settle(&mut self, pane: PaneId, p: &Profile, answers: Vec<(String, String)>) {
        if let Some(name) = answered_name(p, &answers) {
            self.set_session_values(pane, &[("Name".to_string(), Some(toml::Value::String(name)))]);
        }
        self.param_answers.insert(pane, answers);
    }

    /// The answers a restart of `pane` runs with (None: it never had a parameter).
    pub(super) fn param_answers_of(&self, pane: PaneId) -> Option<&[(String, String)]> {
        self.param_answers.get(&pane).map(Vec::as_slice)
    }

    /// Whether `pane` still waits for its answers: nothing restarts it before the last one.
    pub(super) fn param_waiting(&self, pane: PaneId) -> bool {
        self.param_prompts.iter().any(|p| p.pane == pane)
    }

    /// A pane that closed takes its waiting prompt and its answers with it.
    pub(super) fn param_pane_gone(&mut self, id: PaneId) {
        self.param_prompts.retain(|p| p.pane != id);
        self.param_answers.remove(&id);
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

    /// Review I1, I2, M1: the answers stay with the session. A restart reruns the answered command
    /// and folder (`PS:3507-3511`, `:4889-4899`), the answered Name is the session's own value
    /// (`SF:260-268`), and the working directory is scanned whatever Custom Directory says (`SF:103`).
    #[test]
    fn answers_stay_with_the_session() {
        let mut p = Profile::default();
        p.set("Custom Command", toml::Value::String("Yes".into()));
        p.set("Command", toml::Value::String("ssh $$TARGET$$".into()));
        p.set("Name", toml::Value::String("to $$TARGET$$".into()));
        p.set("Working Directory", toml::Value::String("C:\\$$DIR$$".into()));
        let answers = vec![("$$DIR$$".to_string(), "work".to_string()), ("$$TARGET$$".to_string(), "box".to_string())];
        let again = restart_profile(&p, Some(&answers)).expect("a parameterised pane restarts answered");
        assert_eq!(again.string("Command"), "ssh box");
        assert_eq!(again.string("Working Directory"), "C:\\work");
        assert!(restart_profile(&p, None).is_none(), "a pane with no parameter restarts from its profile");
        // After the Name became the session's own value, a restart still answers the command.
        let mut own = p.clone();
        own.set("Name", toml::Value::String("to box".into()));
        assert_eq!(restart_profile(&own, Some(&answers)).unwrap().string("Name"), "to box");
        own.set("Name", toml::Value::String("to $$OTHER$$".into()));
        let more = vec![("$$OTHER$$".to_string(), "x".to_string())];
        assert_eq!(restart_profile(&own, Some(&more)).unwrap().string("Name"), "to $$OTHER$$", "a restart never rescans the answered Name");
        own.set("Name", toml::Value::String("to box".into()));
        assert_eq!(answered_name(&p, &answers).as_deref(), Some("to box"));
        assert_eq!(answered_name(&own, &answers), None, "nothing to set when the Name has no parameter");
        // M1: the folder is asked for even with Custom Directory off.
        assert_ne!(p.string("Custom Directory"), "Yes");
        let (c, n, d) = texts(&p);
        assert_eq!(sp::missing(&c, &n, &d, &[]), vec!["$$DIR$$".to_string(), "$$TARGET$$".to_string()]);
    }

    /// Review I4: the start log line never holds an answer.
    #[test]
    fn nothing_typed_reaches_the_start_log() {
        let program = crate::dlog::program(start_log_command("secrethost --port 22", Some("$$TARGET$$ --port 22"))).to_string();
        assert!(!program.contains("secrethost"), "{program}");
        assert_eq!(crate::dlog::program(start_log_command("pwsh.exe -NoLogo", None)).to_string(), "pwsh");
        let m = include_str!("mod.rs");
        let spawn = body(m, "    fn spawn_pane(");
        assert!(spawn.contains("crate::dlog::name(&shared_name), crate::dlog::program(session_params_app::start_log_command(&cmd, unanswered.as_deref()))"));
        assert!(spawn.contains("self.param_profile.is_some().then(|| profile::command_line(base, &self.shell, None))"));
        assert!(spawn.contains("crate::session_profile::ORIGINAL_GUID"), "a session's own Name is never logged");
        assert!(!spawn.contains("crate::dlog::name(&p.name())"));
    }

    /// Review I1, I3: a restart applies the kept answers and never runs before the last answer; the
    /// launch enters the pane's own window first; a closed pane drops its answers.
    #[test]
    fn restart_and_launch_use_the_answers() {
        let m = include_str!("mod.rs");
        let me = include_str!("session_params_app.rs");
        let restart = body(m, "    fn restart_confirmed(");
        assert!(restart.contains("if !self.restartable(id) || self.param_waiting(id)"));
        let kept = restart.find("let answered = self.param_answers_of(id)").unwrap();
        let applied = restart.find("session_params_app::restart_profile(&self.profile(guid.as_deref()), answered.as_deref())").unwrap();
        assert!(kept < applied && applied < restart.find("self.spawn_pane(id, rect").unwrap());
        let launch = body(me, "fn param_launch");
        let settle = launch.find("self.param_settle(done.pane").unwrap();
        let enter = launch.find("self.enter_pane_window(done.pane)").unwrap();
        assert!(settle < enter && enter < launch.find("self.restart_confirmed(done.pane)").unwrap());
        assert!(!launch.contains("self.param_profile"), "the restart road builds the answered profile");
        let settle = body(me, "pub(super) fn param_settle");
        assert!(settle.contains("self.set_session_values(pane, &[(\"Name\".to_string()") && settle.contains("self.param_answers.insert(pane, answers)"));
        let start = body(m, "    fn start_pane_as(");
        assert!(start.contains("self.param_settle(id, &base, answers)"), "a restore's and an escape's answers are kept too");
        assert!(body(me, "pub(super) fn param_pane_gone").contains("self.param_answers.remove(&id)"));
    }
}
