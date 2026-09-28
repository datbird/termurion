//! Running a chat's tool call on its linked pane (`PTYSession.swift` `*RemoteCommand`): the
//! read-only tools answer from a `Facts` snapshot of the pane's own screen. The writing tools
//! (Task 6) and `execute_command` (Task 7, `ai_exec_start`) join here; every call reaches this
//! file only through `ai_run_tool`, after the permission gate.

use super::App;
use crate::ai_terminal::{self, Facts, Outcome};
use crate::ai_tools::Call;
use crate::layout::PaneId;

/// The answer to a call that reached no arm (only a browser call can, before milestone 13
/// adds its runner). Tandem's text.
pub(super) const NOT_AVAILABLE: &str = "This function is not available.";

/// How a started tool call goes on.
#[allow(dead_code)] // `Pending` is Task 6's, `Waiting` the 13x AI browser tools plan's.
#[derive(Debug)]
pub(super) enum ToolStart {
    /// Answered now.
    Done(Outcome),
    /// A Get-Help runs on a worker (Task 6); `ai_help_tick` finishes it.
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
    #[allow(dead_code)] // Called by `ai_tool_start` and Task 8's context block.
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
        f.local = super::statusbar_content::is_localhost(f.host.as_deref());
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
    #[allow(dead_code)] // Called by `ai_run_tool` (Task 11).
    pub(super) fn ai_tool_start(&mut self, pane: PaneId, call: &Call) -> ToolStart {
        let facts = self.ai_facts(pane, call);
        match ai_terminal::answer(call, &facts) {
            Some(o) => ToolStart::Done(o),
            None => ToolStart::Done(Outcome { model: NOT_AVAILABLE.to_string(), notice: None }),
        }
    }
}
