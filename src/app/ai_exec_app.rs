//! Running the model's commands on a pane (`PTYSession.swift:511-595`): `ai_exec_start` types
//! the command through `user_write(.., Write::Agent)` and `ai_exec_tick` finds its end. Every
//! call reaches here only through `ai_run_tool`, after the permission gate.

use super::App;
use crate::ai_exec::{self, MarkerFound, Path, Running};
use crate::ai_terminal::Outcome;
use crate::broadcast::Write;
use crate::layout::PaneId;
use crate::screen::Screen;
use std::collections::HashMap;

/// A command's finish, keyed the way Task 11 delivers it: (chat, request id, outcome).
pub(super) type Finished = (String, String, Outcome);

/// The text a Prompt-path command produced, once the next prompt has arrived: None while it
/// still runs; Some(None) when the start line was dropped from the scrollback; Some(Some(text))
/// the lines from the submit line to the line before the new prompt, trailing whitespace
/// trimmed and uncapped (`contentAfter`).
fn prompt_capture(screen: &Screen, running: &Running) -> Option<Option<String>> {
    if screen.prompts_seen() <= running.prompts_before {
        return None;
    }
    let end = screen.current_prompt_line().map_or_else(|| screen.abs_cursor_line(), |l| screen.abs_line(l));
    Some(screen.abs_text(running.start_line, end).map(|t| t.trim_end().to_string()))
}

/// A Marker-path command's state: None while it waits, else the outcome.
fn marker_capture(screen: &Screen, running: &Running, uuid: &str) -> Option<Outcome> {
    let text = screen.abs_text(running.start_line, screen.abs_cursor_line() + 1);
    let Some(text) = text else { return Some(ai_exec::unavailable_outcome(&running.command)) };
    match ai_exec::marker_output(&text, uuid) {
        MarkerFound::Waiting => None,
        MarkerFound::Output(t) => Some(ai_exec::output_outcome(&running.command, t)),
        MarkerFound::Lost(t) => Some(ai_exec::lost_outcome(&running.command, t)),
        MarkerFound::Empty => Some(ai_exec::unavailable_outcome(&running.command)),
    }
}

/// Records `running` for `pane`. A command already running there is cancelled (`PS:342`): its
/// (chat, request id) goes to `canceled`, for the tick to answer.
fn replace_running(execs: &mut HashMap<PaneId, Running>, canceled: &mut Vec<(String, String)>, pane: PaneId, running: Running) {
    if let Some(old) = execs.insert(pane, running) {
        canceled.push((old.chat, old.request_id));
    }
}

impl App {
    /// Types `command` on `pane` and starts waiting for its end. `command` is what is typed:
    /// the model's own, never the staged prefix the check saw, which is already on the prompt
    /// (ruling R12cd-2). A command already running on the pane is cancelled first.
    #[allow(dead_code)] // Called by `ai_run_tool` (Task 11).
    pub(super) fn ai_exec_start(&mut self, pane: PaneId, chat: &str, request_id: &str, command: &str) {
        let (kind, ..) = self.ai_shell_of(pane);
        let ever_used = self.shell_history.as_ref().is_some_and(|h| h.ever_used());
        let Some(p) = self.panes.get(&pane) else { return };
        let s = p.own();
        let uuid = uuid::Uuid::new_v4().to_string().to_uppercase();
        let plan = ai_exec::plan(command, kind, ever_used, s.at_prompt() == Some(true), &uuid);
        let running = Running { chat: chat.to_string(), request_id: request_id.to_string(), command: command.to_string(), path: plan.path, start_line: s.abs_cursor_line(), prompts_before: s.prompts_seen() };
        replace_running(&mut self.ai_execs, &mut self.ai_exec_canceled, pane, running);
        self.user_write(pane, &plan.bytes, Write::Agent);
        // The line was submitted: nothing the AI typed earlier is staged any more.
        self.ai_staged.remove(&pane);
    }

    /// Cancels the command running on `pane`; its (chat, request id), answered
    /// `CANCELED_MODEL` with no notice by the caller. Nothing else answers it: the command is
    /// forgotten.
    #[allow(dead_code)] // Called by `ai_run_tool` (Task 11).
    pub(super) fn ai_exec_cancel(&mut self, pane: PaneId) -> Option<(String, String)> {
        self.ai_execs.remove(&pane).map(|r| (r.chat, r.request_id))
    }

    /// Each finished command (each `about_to_wait`; only panes in `ai_execs` cost anything).
    /// A command a newer one replaced, and one whose pane closed, answer `CANCELED_MODEL`.
    /// No timeout (iTerm2).
    pub(super) fn ai_exec_tick(&mut self) -> Vec<Finished> {
        let mut out: Vec<Finished> = self.ai_exec_canceled.drain(..).map(|(c, r)| (c, r, ai_exec::canceled_outcome())).collect();
        for pane in self.ai_execs.keys().copied().collect::<Vec<_>>() {
            let Some(running) = self.ai_execs.get(&pane) else { continue };
            let done = match self.panes.get(&pane) {
                None => Some(ai_exec::canceled_outcome()),
                Some(p) => match &running.path {
                    Path::Prompt => prompt_capture(p.own(), running).map(|c| match c {
                        Some(text) => ai_exec::output_outcome(&running.command, text),
                        None => ai_exec::unavailable_outcome(&running.command),
                    }),
                    Path::Marker { uuid } => marker_capture(p.own(), running, uuid),
                },
            };
            if let Some(outcome) = done {
                if let Some(r) = self.ai_execs.remove(&pane) {
                    out.push((r.chat, r.request_id, outcome));
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn running(path: Path, screen: &Screen) -> Running {
        Running { chat: "c".into(), request_id: "r".into(), command: "dir".into(), path, start_line: screen.abs_cursor_line(), prompts_before: screen.prompts_seen() }
    }

    #[test]
    fn exec_capture_follows_iterm2() {
        let mut s = Screen::new(40, 10);
        s.feed(b"\x1b]133;A\x07\x1b]133;B\x07");
        let r = running(Path::Prompt, &s);
        assert_eq!(r.prompts_before, 1);
        assert_eq!(prompt_capture(&s, &r), None);
        s.feed(b"dir\r\na\r\nb\r\n");
        assert_eq!(prompt_capture(&s, &r), None, "before the new prompt");
        s.feed(b"\x1b]133;D;0\x07\x1b]133;A\x07\x1b]133;B\x07");
        assert_eq!(prompt_capture(&s, &r), Some(Some("dir\na\nb".to_string())));
        // A start line that fell off the scrollback gives Some(None).
        let mut small = Screen::new(40, 3);
        small.feed(b"\x1b]133;A\x07\x1b]133;B\x07");
        let mut lost = running(Path::Prompt, &small);
        lost.start_line -= 100_000;
        small.feed(b"x\r\n\x1b]133;D;0\x07\x1b]133;A\x07");
        assert_eq!(prompt_capture(&small, &lost), Some(None));
    }

    #[test]
    fn marker_capture_finds_the_output() {
        let mut s = Screen::new(60, 10);
        s.feed(b"C:\\> ");
        let r = running(Path::Marker { uuid: "U".into() }, &s);
        s.feed(b"dir & echo -- FINISHED U\r\na\r\n");
        assert_eq!(marker_capture(&s, &r, "U"), None);
        s.feed(b"-- FINISHED U\r\nC:\\> ");
        let o = marker_capture(&s, &r, "U").unwrap();
        assert_eq!(o.notice.as_deref(), Some("Ran dir"));
        assert_eq!(o.model, "\na\n");
    }

    #[test]
    fn exec_cancel_follows_iterm2() {
        let s = Screen::new(10, 3);
        let mut execs = HashMap::new();
        let mut canceled = Vec::new();
        let mut first = running(Path::Prompt, &s);
        first.request_id = "one".into();
        replace_running(&mut execs, &mut canceled, 1, first);
        assert!(canceled.is_empty());
        let mut second = running(Path::Prompt, &s);
        second.request_id = "two".into();
        replace_running(&mut execs, &mut canceled, 1, second);
        assert_eq!(canceled, vec![("c".to_string(), "one".to_string())]);
        assert_eq!(execs[&1].request_id, "two");
        // The tick answers a cancelled command CANCELED_MODEL with no notice.
        let o = ai_exec::canceled_outcome();
        assert_eq!(o.model, ai_exec::CANCELED_MODEL);
        assert_eq!(o.notice, None);
    }
}
