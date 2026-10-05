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

/// A command's finish, keyed the way `chat_tick` delivers it: (chat, request id, outcome).
pub(super) type Finished = (String, String, Outcome);

/// A Prompt-path command's state once the next prompt has arrived: None while it still runs,
/// else the outcome: the lines from the submit line to the line before the new prompt, trailing
/// whitespace trimmed and uncapped (`contentAfter`). A start line that left the scrollback does
/// not end the wait or answer "unavailable": the text is read from the oldest line kept and
/// answers the lost-output notice, as the marker path does.
fn prompt_capture(screen: &Screen, running: &Running) -> Option<Outcome> {
    if screen.prompts_seen() <= running.prompts_before {
        return None;
    }
    let end = screen.current_prompt_line().map_or_else(|| screen.abs_cursor_line(), |l| screen.abs_line(l));
    let from = running.start_line.max(screen.oldest_abs_line());
    let Some(text) = screen.abs_text(from, end).map(|t| t.trim_end().to_string()) else { return Some(ai_exec::unavailable_outcome(&running.command)) };
    Some(if running.start_line < from { ai_exec::lost_outcome(&running.command, text) } else { ai_exec::output_outcome(&running.command, text) })
}

/// A Marker-path command's state: None while it waits, else the outcome. A start line that
/// left the scrollback does not end the wait: the marker is still awaited and the text is read
/// from the oldest line kept, which answers the lost-output notice (iTerm2).
fn marker_capture(screen: &Screen, running: &Running, uuid: &str) -> Option<Outcome> {
    let from = running.start_line.max(screen.oldest_abs_line());
    let Some(text) = screen.abs_text(from, screen.abs_cursor_line() + 1) else { return None };
    match ai_exec::marker_output(&text, uuid) {
        MarkerFound::Waiting => None,
        MarkerFound::Output(t) => Some(ai_exec::output_outcome(&running.command, t)),
        MarkerFound::Lost(t) => Some(ai_exec::lost_outcome(&running.command, t)),
        MarkerFound::Empty => Some(ai_exec::unavailable_outcome(&running.command)),
    }
}

/// A running command's state on its pane's screen: None while it runs. A pane that is gone
/// cancels it.
fn capture(screen: Option<&Screen>, running: &Running) -> Option<Outcome> {
    let Some(screen) = screen else { return Some(ai_exec::canceled_outcome()) };
    match &running.path {
        Path::Prompt => prompt_capture(screen, running),
        Path::Marker { uuid } => marker_capture(screen, running, uuid),
    }
}

/// A running command's start line after rows were spliced into the history: a row on the
/// screen (at or after `api_from`) now has a number `by` higher; a history row keeps its own.
pub(super) fn rebased_start(start_line: i64, api_from: i64, by: i64) -> i64 {
    if start_line >= api_from { start_line + by } else { start_line }
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
    ///
    /// Answers at once, typing nothing, when the command holds a refused control key
    /// (`ai_exec::refusal`, checked again here so no caller can skip it) or the pane is gone
    /// (`NO_SESSION`); None when it started.
    pub(super) fn ai_exec_start(&mut self, pane: PaneId, chat: &str, request_id: &str, command: &str) -> Option<Outcome> {
        if let Some(refused) = ai_exec::refusal(command) {
            return Some(refused);
        }
        let (kind, ..) = self.ai_shell_of(pane);
        let ever_used = self.shell_history.as_ref().is_some_and(|h| h.ever_used());
        let Some(p) = self.panes.get(&pane) else { return Some(Outcome { model: super::ai_tools_app::NO_SESSION.to_string(), notice: None }) };
        let s = p.own();
        let uuid = uuid::Uuid::new_v4().to_string().to_uppercase();
        let plan = ai_exec::plan(command, kind, ever_used, s.at_prompt() == Some(true), &uuid);
        let running = Running { chat: chat.to_string(), request_id: request_id.to_string(), command: command.to_string(), path: plan.path, start_line: s.abs_cursor_line(), prompts_before: s.prompts_seen() };
        replace_running(&mut self.ai_execs, &mut self.ai_exec_canceled, pane, running);
        self.user_write(pane, &plan.bytes, Write::Agent);
        // The line was submitted: nothing the AI typed earlier is staged any more.
        self.ai_staged.remove(&pane);
        None
    }

    /// Cancels the command running on `pane`; its (chat, request id), answered
    /// `CANCELED_MODEL` with no notice by the caller. Nothing else answers it: the command is
    /// forgotten.
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
            let done = capture(self.panes.get(&pane).map(|p| p.own()), running);
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
        assert_eq!(prompt_capture(&s, &r), Some(ai_exec::output_outcome("dir", "dir\na\nb".to_string())));
        // A start line that fell off the scrollback answers the lost-output notice.
        let mut small = Screen::new(40, 3);
        small.feed(b"\x1b]133;A\x07\x1b]133;B\x07");
        let mut lost = running(Path::Prompt, &small);
        lost.start_line -= 100_000;
        small.feed(b"x\r\n\x1b]133;D;0\x07\x1b]133;A\x07");
        let o = capture(Some(&small), &lost).expect("the new prompt ended the command");
        assert_eq!(o.notice.as_deref(), Some(ai_exec::lost_notice("dir").as_str()), "lost-output notice, as the marker path");
        assert_ne!(o.model, ai_exec::UNAVAILABLE_MODEL);
        assert!(o.model.contains('x'), "the text from the oldest line kept: {:?}", o.model);
    }

    /// Task 4 fix round 2: an alternate-screen session that scrolls splices rows into the history
    /// above the screen; a command the AI ran before it still starts at its own row.
    #[test]
    fn exec_start_line_survives_an_alternate_session() {
        let mut s = Screen::new(40, 6);
        s.feed(b"\x1b]133;A\x07\x1b]133;B\x07");
        let mut r = running(Path::Prompt, &s);
        s.feed(b"git log\r\n\x1b[?1049h");
        for i in 0..20 {
            s.feed(format!("pager{i}\r\n").as_bytes());
        }
        s.feed(b"\x1b[?1049lback\r\n\x1b]133;D;0\x07\x1b]133;A\x07\x1b]133;B\x07");
        for req in s.take_requests() {
            if let crate::screen::Request::HistoryRebased { api_from, by, .. } = req {
                r.start_line = rebased_start(r.start_line, api_from, by as i64);
            }
        }
        let o = prompt_capture(&s, &r).expect("the new prompt ended the command");
        assert!(o.model.starts_with("git log"), "{:?}", o.model);
        assert!(!o.model.contains("pager"), "{:?}", o.model);
        // A command that began in the history keeps its number.
        assert_eq!(rebased_start(3, 10, 5), 3);
        assert_eq!(rebased_start(10, 10, 5), 15);
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
        // A pane that closed cancels its running command, of either path.
        for path in [Path::Prompt, Path::Marker { uuid: "U".into() }] {
            let o = capture(None, &running(path, &s)).expect("a closed pane answers");
            assert_eq!((o.model.as_str(), o.notice), (ai_exec::CANCELED_MODEL, None));
        }
    }

    #[test]
    fn marker_survives_scrollback_loss() {
        let mut s = Screen::new(60, 3);
        s.feed(b"C:\\> ");
        let mut r = running(Path::Marker { uuid: "U".into() }, &s);
        // The start line is long gone (a huge output): the command still runs.
        r.start_line -= 100_000;
        s.feed(b"out1\r\nout2\r\n");
        assert!(capture(Some(&s), &r).is_none(), "the marker has not printed: keep waiting, never unavailable");
        s.feed(b"-- FINISHED U\r\nC:\\> ");
        let o = capture(Some(&s), &r).expect("the marker printed");
        assert_eq!(o.notice.as_deref(), Some(ai_exec::lost_notice("dir").as_str()));
        assert!(o.model.contains("out1") && !o.model.contains("FINISHED"), "{:?}", o.model);
    }

    #[test]
    fn exec_start_answers_a_missing_session() {
        assert_eq!(super::super::ai_tools_app::NO_SESSION, "Error: that session is no longer open.");
    }
}
