//! Running a chat's tool call on its linked pane (`PTYSession.swift` `*RemoteCommand`): the
//! read-only tools answer from a `Facts` snapshot of the pane's own screen. The writing tools
//! (Task 6) and `execute_command` (Task 7, `ai_exec_start`) join here; every call reaches this
//! file only through `ai_run_tool`, after the permission gate.

use super::App;
use crate::ai_actions::{self, FileTarget};
use crate::ai_shell::{self, ShellKind};
use crate::ai_terminal::{self, Facts, Outcome};
use crate::ai_tools::Call;
use crate::broadcast::Write;
use crate::layout::PaneId;

/// The answer to a call that reached no arm (only a browser call can, before milestone 13
/// adds its runner). Tandem's text.
pub(super) const NOT_AVAILABLE: &str = "This function is not available.";

/// The answer of a writing tool whose session is gone. Tandem's text.
pub(super) const NO_SESSION: &str = "Error: that session is no longer open.";

/// The chat's notice for a finished Get-Help (`PS:848`).
const MANPAGE_NOTICE: &str = "manpage provided to AI.";

fn done(model: impl Into<String>, notice: impl Into<String>) -> ToolStart {
    ToolStart::Done(Outcome { model: model.into(), notice: Some(notice.into()) })
}

/// How a started tool call goes on.
#[allow(dead_code)] // `Waiting` is the 13x AI browser tools plan's.
#[derive(Debug)]
pub(super) enum ToolStart {
    /// Answered now.
    Done(Outcome),
    /// A Get-Help runs on a worker; `ai_help_tick` finishes it.
    Pending,
    /// A browser call runs under this token (m13 ruling Q4); the browser tools plan records
    /// it by token and finishes it through `ai_tool_finished`. No 12c arm answers it.
    Waiting(u64),
}

fn unix_now() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64())
}

impl App {
    /// What the read-only tools read from `pane`'s own screen (never a Filter or Instant Replay
    /// view), with only what `call` needs: the history only for the two history tools, the
    /// output only for `get_command_output`, the screen JSON only for `get_screen_contents`.
    pub(super) fn ai_facts(&self, pane: PaneId, call: &Call) -> Facts {
        let mut f = Facts {
            ever_used: self.shell_history.as_ref().is_some_and(|h| h.ever_used()),
            local_user: std::env::var("USERNAME").unwrap_or_default(),
            ..Default::default()
        };
        let Some(p) = self.panes.get(&pane) else { return f };
        let s = p.own();
        let vars = self.variables_with(pane, false);
        let var = |k: &str| vars.session.get(k).cloned();
        f.at_prompt = s.at_prompt() == Some(true);
        // Tandem's typed command already ends at the cursor (Decision).
        f.typed = s.typed_command();
        f.typed_to_cursor = f.typed.clone();
        f.last_command = s.last_command_mark().map(|m| m.command.clone()).filter(|c| !c.is_empty());
        f.last_exit = s.last_exit();
        {
            use alacritty_terminal::grid::Dimensions;
            let grid = s.term().grid();
            (f.columns, f.rows) = (grid.columns(), grid.screen_lines());
        }
        f.shell = var("shell").unwrap_or_default();
        f.host = var("hostname");
        f.local = super::statusbar_content::is_this_machine(s.shell.host.as_deref(), s.shell.machine_id.as_deref());
        f.user = var("username");
        f.path = var("path").unwrap_or_default();
        match call {
            Call::GetCommandHistory { .. } | Call::SearchCommandHistory { .. } => f.history = s.prompt_commands(),
            // An empty or unknown id reads the last command's output (iTerm2).
            Call::GetCommandOutput { id } => f.output = s.command_output(id.parse().ok()),
            Call::GetScreenContents { lines } => {
                // iTerm2 asks whether claude is in the session's job chain (`GlobalJobMonitor`).
                let claude = s.shell.job.as_ref().is_some_and(|j| crate::proctree::same_job(&j.name, "claude")) || s.shell.job_ancestors.iter().any(|a| crate::proctree::same_job(a, "claude"));
                let alternate = s.term().mode().contains(alacritty_terminal::term::TermMode::ALT_SCREEN);
                let kind = ai_terminal::kind_of(claude.then_some("claude"), alternate);
                let changed = s.newest_visible_change().map(|t| unix_now() - f64::from(t)).filter(|age| *age >= 0.0).map(|age| format!("{} ago", ai_terminal::compact_age(age)));
                f.screen = Some(s.screen_contents(Some(ai_terminal::clamp_lines(*lines)), kind, changed));
            }
            _ => {}
        }
        f
    }

    /// Starts a tool call on `pane` (the one dispatcher, `ai_run_tool`, calls this after the
    /// gate). The read-only tools answer at once from `ai_facts`; Tasks 6 and 7 add their
    /// arms. A call no arm handles answers `NOT_AVAILABLE` with no notice.
    pub(super) fn ai_tool_start(&mut self, pane: PaneId, call: &Call) -> ToolStart {
        // A tool for a session that is gone answers its error, never invented facts.
        if !self.panes.contains_key(&pane) {
            return ToolStart::Done(Outcome { model: NO_SESSION.to_string(), notice: None });
        }
        // The kind first: a browser call never touches a terminal and the reverse (`PS:326-336`).
        let browser_call = call.category().is_browser();
        if let Some(o) = crate::ai_browser::kind_refusal(browser_call, self.is_browser_pane(pane)) {
            return ToolStart::Done(o);
        }
        if browser_call {
            return self.ai_browser_start(pane, call);
        }
        if let Some(started) = self.ai_write_tool(pane, call) {
            return started;
        }
        let facts = self.ai_facts(pane, call);
        match ai_terminal::answer(call, &facts) {
            Some(o) => ToolStart::Done(o),
            None => ToolStart::Done(Outcome { model: NOT_AVAILABLE.to_string(), notice: None }),
        }
    }

    /// The session's shell kind, whether it is this machine's, and its cwd: the one detector's
    /// answer (the `shell` variable, `ai_terminal::detected_shell`), classified.
    pub(super) fn ai_shell_of(&self, pane: PaneId) -> (ShellKind, bool, Option<String>) {
        let vars = self.variables_with(pane, false);
        let shell = self.panes.get(&pane).map(|p| &p.own().shell);
        let local = super::statusbar_content::is_this_machine(shell.and_then(|s| s.host.as_deref()), shell.and_then(|s| s.machine_id.as_deref()));
        (ai_shell::shell_kind(vars.session.get("shell").map(String::as_str), local), local, vars.session.get("path").cloned())
    }

    /// The writing tools. None for a call that is not one. Every byte goes through
    /// `user_write(pane, bytes, Write::Agent)`: never broadcast, no other writer.
    fn ai_write_tool(&mut self, pane: PaneId, call: &Call) -> Option<ToolStart> {
        if !matches!(call, Call::SetClipboard { .. } | Call::InsertTextAtCursor { .. } | Call::DeleteCurrentLine | Call::RestartSession | Call::GetManPage { .. } | Call::CreateFile { .. }) {
            return None;
        }
        Some(match call {
            // The agent's write is permission-gated; "AllowClipboardAccess" governs OSC 52 (Decision).
            Call::SetClipboard { text } => match self.clipboard.as_mut().map(|c| c.set_text(text.clone())) {
                Some(Ok(())) => done(ai_actions::COPIED, "Clipboard contents changed by AI."),
                Some(Err(e)) => done(format!("Error: {e}"), "Clipboard contents changed by AI."),
                None => done("Error: the clipboard is not available.", "Clipboard contents changed by AI."),
            },
            Call::InsertTextAtCursor { text } => match ai_actions::insert_plan(text) {
                Err(answer) if answer == ai_actions::LINE_END_REFUSED => done(answer, ai_actions::LINE_END_NOTICE),
                Err(answer) => done(answer, ai_actions::ESCAPE_NOTICE),
                Ok(typed) => {
                    self.user_write(pane, typed.as_bytes(), Write::Agent);
                    let prompts = self.panes.get(&pane).map_or(0, |p| p.own().prompts_seen());
                    self.ai_staged.entry(pane).or_default().push(prompts, &typed);
                    done(ai_actions::inserted(typed.chars().count()), "Text inserted by AI.")
                }
            },
            Call::DeleteCurrentLine => {
                self.user_write(pane, ai_actions::DELETE_LINE, Write::Agent);
                self.ai_staged.remove(&pane);
                done(ai_actions::DONE, "Current line deleted by AI.")
            }
            Call::RestartSession => {
                // `api_restart_session`'s rule (`isRestartable`), with the same refusals as the
                // menu's Restart Session (a git window, a kept-open pane, a recording).
                let restartable = self.panes.get(&pane).is_some_and(|p| (p.exited.is_some() || !p.gateway) && self.restartable(pane));
                if !restartable {
                    return Some(done(ai_actions::NOT_RESTARTABLE, "AI tried to restart the session but it is not restartable."));
                }
                self.enter_pane_window(pane);
                let step = self.restart_via(pane, crate::session_end::Via::Api);
                self.ai_staged.remove(&pane);
                self.redraw();
                match step {
                    // Programs the ended shell left are named to the person first.
                    crate::session_end::RestartStep::AskRunningJobs | crate::session_end::RestartStep::AskSurvivors | crate::session_end::RestartStep::Offer => done(ai_actions::WAITING, "AI asked to restart the session; waiting for the person."),
                    crate::session_end::RestartStep::Now => done(ai_actions::RESTARTING, "Session restarted by AI."),
                    crate::session_end::RestartStep::Refused => done(ai_actions::NOT_RESTARTABLE, "AI tried to restart the session but it is not restartable."),
                }
            }
            Call::GetManPage { cmd } => {
                let (kind, ..) = self.ai_shell_of(pane);
                match ai_actions::man_page_plan(cmd, kind) {
                    Some(args) => self.ai_help_start(pane, args),
                    None => done(ai_actions::man_page_unavailable(cmd), MANPAGE_NOTICE),
                }
            }
            Call::CreateFile { filename, content } => {
                let (_, local, cwd) = self.ai_shell_of(pane);
                let home = std::env::var_os("USERPROFILE").map(std::path::PathBuf::from);
                let Some(home) = home else { return Some(done(ai_actions::create_failed("the home folder is not known"), ai_actions::create_failed_notice(filename, "the home folder is not known"))) };
                match ai_actions::file_target(filename, cwd.as_deref(), local, &home) {
                    FileTarget::Refused(why) => done(why.clone(), ai_actions::create_failed_notice(filename, &why)),
                    FileTarget::Local(path) => match crate::inert_files::write_marked(&path, content.as_bytes()) {
                        Ok(()) => {
                            self.ai_reveal_file(&path);
                            done("Ok", ai_actions::created_notice(filename))
                        }
                        Err(e) => done(ai_actions::create_failed(&e.to_string()), ai_actions::create_failed_notice(filename, &e.to_string())),
                    },
                }
            }
            _ => return None,
        })
    }

    /// Shows the new file selected in Explorer (the semantic history helper); nothing runs it.
    fn ai_reveal_file(&mut self, path: &std::path::Path) {
        self.reveal_in_explorer(&path.to_string_lossy().replace('/', "\\"));
    }

    /// Starts `get_help` on a worker thread (a first Get-Help can take seconds and must never
    /// block the UI thread), wakes the loop when it finishes, and keeps the answer's receiver
    /// for `ai_help_tick`. A newer Get-Help on the pane replaces the older one.
    fn ai_help_start(&mut self, pane: PaneId, args: Vec<String>) -> ToolStart {
        #[cfg(windows)]
        {
            let (tx, rx) = std::sync::mpsc::channel();
            let notify = self.notify.clone();
            std::thread::spawn(move || {
                let _ = tx.send(ai_actions::get_help(&args));
                notify();
            });
            self.ai_helps.insert(pane, rx);
            ToolStart::Pending
        }
        #[cfg(not(windows))]
        {
            let _ = (pane, args);
            done("Error running Get-Help: PowerShell is not available.", MANPAGE_NOTICE)
        }
    }

    /// Each finished Get-Help, polled from `ai_helps` with `try_recv`; the chat maps the pane
    /// to its request (`ai_pending_tools`), as `ai_exec_tick` does for a command.
    pub(super) fn ai_help_tick(&mut self) -> Vec<(PaneId, Outcome)> {
        let mut finished = Vec::new();
        for pane in self.ai_helps.keys().copied().collect::<Vec<_>>() {
            let text = match self.ai_helps.get(&pane).map(|rx| rx.try_recv()) {
                Some(Ok(text)) => text,
                Some(Err(std::sync::mpsc::TryRecvError::Disconnected)) => "Error running Get-Help: the help reader stopped.".to_string(),
                _ => continue,
            };
            self.ai_helps.remove(&pane);
            finished.push((pane, Outcome { model: text, notice: Some(MANPAGE_NOTICE.to_string()) }));
        }
        finished
    }
}
