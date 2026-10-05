//! A session's end (milestone 13q): the pane stays and follows "After a session ends"
//! (`brokenPipeWithError:`, `PS:4747-4822`), a detached tmux window stays or closes
//! (`tmuxDidDisconnect`, `PS:4697-4725`), and the bars that ask about it.

use super::*;
use crate::session_end::{self as se, EndAction, Exit, Message, Outcome, TmuxAfter};
use super::scripts_alerts::{Ask, Then};

/// The bar that lists programs an exited shell left running (13q P8; Tandem's own).
const SURVIVORS_BAR: &str = "session-end-survivors";

/// The panes whose end is new: exited and not yet handled. Each end is handled once.
pub(super) fn newly_ended(panes: &[(PaneId, bool, bool)]) -> Vec<PaneId> {
    panes.iter().filter(|(_, exited, ended)| *exited && !*ended).map(|(id, _, _)| *id).collect()
}

/// A pane's end action: a git window's own (No Action), a kept-open pane's (No Action), else
/// its profile's value (`PS:4825-4837`).
pub(super) fn end_action_for(single_use: Option<EndAction>, hold_open: bool, profile_value: i64) -> EndAction {
    match single_use {
        Some(a) => a,
        None if hold_open => EndAction::NoAction,
        None => EndAction::from_int(profile_value),
    }
}

/// Whether a pane has a program of its own to start again (`isRestartable`, `PS:4849-4854`):
/// not a tmux client, a loaded recording, a single-use window or a kept-open pane.
pub(super) fn restartable_for(tmux: bool, read_only: bool, single_use: bool, hold_open: bool) -> bool {
    !(tmux || read_only || single_use || hold_open)
}

impl App {
    /// The warning when an ended session is closed within `ShortLivedSessionDuration` of its start
    /// (`terminate`, `PS:3887-3898`): measured now, keyed by the saved profile's Guid for a session
    /// copy (`PS:3767-3772`). 13b's queue shows nothing for a silenced key.
    pub(super) fn warn_short_lived(&mut self, id: PaneId) {
        let Some(p) = self.panes.get(&id) else { return };
        let facts = se::CloseFacts {
            exited: p.exited.is_some(),
            single_use: self.single_use.contains_key(&id),
            tmux: p.tmux_io().is_some(),
            read_only: p.dvr.read_only,
            hold_open: p.hold_open,
            // Archives restore panes that ended long ago; none is flagged yet (Task 8).
            archived: false,
            lived: p.created_at.elapsed(),
            limit_seconds: self.globals.number("ShortLivedSessionDuration").unwrap_or(3.0),
        };
        if !se::short_lived_due(facts) {
            return;
        }
        let saved = self.store.shared_guid(&p.profile);
        let name = self.store.get(&saved).or_else(|| self.store.get(&p.profile)).map(|pr| pr.name()).unwrap_or_default();
        self.ask_scripts(Ask::new(se::short_lived_spec(&saved, &name), Then::ShortLived));
    }

    /// What the pane does when its session ends.
    pub(super) fn end_action_of(&self, id: PaneId) -> EndAction {
        let single = self.single_use.get(&id).map(|s| s.end_action);
        let Some(p) = self.panes.get(&id) else { return EndAction::NoAction };
        let value = self.store.get(&p.profile).unwrap_or_else(|| self.store.default_profile()).int(se::END_ACTION_KEY);
        end_action_for(single, p.hold_open, value)
    }

    /// Whether the pane can start its program again.
    pub(super) fn restartable(&self, id: PaneId) -> bool {
        self.panes.get(&id).is_some_and(|p| restartable_for(p.tmux_io().is_some(), p.dvr.read_only, self.single_use.contains_key(&id), p.hold_open))
    }

    /// Handles the end of every pane whose shell exited and whose output has drained. Returns the
    /// panes handled and when the next one is due.
    pub(super) fn handle_ended(&mut self, event_loop: &ActiveEventLoop) -> (Vec<PaneId>, Option<Instant>) {
        let now = Instant::now();
        let facts: Vec<(PaneId, bool, bool)> = self.panes.iter().map(|(id, p)| (*id, p.exited.is_some(), p.ended)).collect();
        let mut ready = Vec::new();
        let mut next: Option<Instant> = None;
        for id in newly_ended(&facts) {
            let Some(p) = self.panes.get_mut(&id) else { continue };
            if p.end_ready(now) {
                // Set first: the end is reported once, whatever happens below.
                p.ended = true;
                ready.push(id);
            } else {
                let due = p.end_due_at().map_or(now + Duration::from_millis(20), |d| d.max(now + Duration::from_millis(20)));
                next = Some(next.map_or(due, |n| n.min(due)));
            }
        }
        for id in &ready {
            self.session_exited(*id, event_loop);
        }
        (ready, next)
    }

    /// The timer pass: ends that came due are handled and drawn; the earliest one left is returned.
    pub(super) fn tick_ended(&mut self, event_loop: &ActiveEventLoop) -> Option<Instant> {
        let (done, next) = self.handle_ended(event_loop);
        if !done.is_empty() {
            self.after_output(&done, Instant::now());
        }
        next
    }

    /// The pane's session ended (R3: the pane stays unless its end action says Close).
    pub(super) fn session_exited(&mut self, id: PaneId, event_loop: &ActiveEventLoop) {
        let Some(p) = self.panes.get_mut(&id) else { return };
        let tmux = p.tmux_io().is_some();
        // The pid is reusable from now on: the watcher forgets it (B7; the survivors were
        // surveyed when the exit was seen).
        if let Some(pid) = p.exit_pid() {
            self.jobs.forget(pid);
        }
        p.inline_paused_off();
        // Everything bound to the program goes; the message below is written to the pane's own
        // screen, never a Filter or Zoom view or a replay frame (B9).
        p.own_mut().program_ended();
        let ssh = p.conductor.is_some() || p.was_ssh_session();
        if ssh {
            if let Some(mut p) = self.panes.remove(&id) {
                self.ssh_pane_dropped(&p);
                p.drop_ssh_at_exit();
                self.panes.insert(id, p);
            }
        }
        self.close_inline_questions_of(id);
        // A git command's window ends its command here, and stays (R3).
        let short_lived = self.short_lived(id);
        self.git_command_ended(id);
        self.deps_command_ended(id);
        if !short_lived {
            self.session_ended(id);
        }
        if let Some(p) = self.panes.get_mut(&id) {
            p.own_mut().fire_event(crate::trigger_events::EventKind::SessionEnded, &Default::default(), &[]);
        }
        self.take_requests();
        let custom = self.globals.string("sessionEndMessageText");
        let finished = self.globals.string("SessionFinishedMessageText");
        let divider = self.globals.string("SessionEndMessageDividerCharacter");
        let action = self.end_action_of(id);
        if tmux {
            let text = se::message_text(Message::TmuxDetached, &custom, &finished);
            self.end_message(id, &text, &divider);
            let close_after = self.globals.optional_bool("TmuxWindowsShouldCloseAfterDetach");
            match se::after_tmux_detach(action, close_after) {
                TmuxAfter::Close => self.close_pane(id, event_loop),
                TmuxAfter::Keep => {}
                TmuxAfter::Ask => {
                    self.ask(id, se::TMUX_CLOSE_BAR, se::TMUX_CLOSE_QUESTION, &[se::ALWAYS, se::NEVER], true);
                }
            }
            self.redraw();
            return;
        }
        let exit = Exit { action, restartable: self.restartable(id), short_lived_single_use: short_lived };
        let (message, outcome) = se::after_exit(exit, self.globals.bool("SuppressRestartAnnouncement"));
        let text = se::message_text(message, &custom, &finished);
        self.end_message(id, &text, &divider);
        // Programs the shell left running end with the pane: it stays until the person
        // closes it and confirms (P8).
        let left = self.panes.get(&id).map(|p| p.live_survivors()).unwrap_or_default();
        match outcome {
            Outcome::Close | Outcome::Finished { close: true } if !left.is_empty() => {
                let text = format!("{} still running: {}. Closing this pane ends them.", if left.len() == 1 { "1 program is".to_string() } else { format!("{} programs are", left.len()) }, left.join(", "));
                self.ask(id, SURVIVORS_BAR, &text, &["OK"], true);
            }
            Outcome::Close => self.close_pane(id, event_loop),
            Outcome::Keep { offer_restart } => {
                if offer_restart {
                    self.offer_restart(id);
                }
            }
            Outcome::RestartAfter(delay) => {
                // A pane that keeps ending stops restarting and offers instead (P11).
                let now = Instant::now();
                let history = self.restart_history.entry(id).or_default();
                history.retain(|t| now.saturating_duration_since(*t) < se::STORM_WINDOW);
                if se::restart_allowed(history, now) {
                    history.push(now);
                    self.pending_restarts.insert(id, now + delay);
                } else {
                    self.ask(id, se::RESTART_BAR, se::KEEPS_ENDING, &[se::RESTART, se::DONT_ASK_AGAIN], true);
                }
            }
            Outcome::Finished { close } => {
                // A buried window comes back to show it (`PS:4786`).
                if self.buried.contains(id) {
                    self.disinter(id);
                }
                if close {
                    self.close_pane(id, event_loop);
                }
            }
        }
        self.redraw();
    }

    /// The restart offer: "Session ended (command exited). Restart it?" (`NC:318-327`).
    pub(super) fn offer_restart(&mut self, id: PaneId) {
        self.ask(id, se::RESTART_BAR, se::RESTART_OFFER, &[se::RESTART, se::DONT_ASK_AGAIN], true);
    }

    /// The timer pass of the Restart end action: each pane whose second is up restarts, if it is
    /// still ended and restartable (`PS:4856-4864`). Returns the next deadline.
    pub(super) fn tick_restarts(&mut self) -> Option<Instant> {
        let (due, next) = se::restart_due(Instant::now(), &self.pending_restarts);
        for id in due {
            self.pending_restarts.remove(&id);
            if self.panes.get(&id).is_some_and(|p| p.ended) && self.restartable(id) {
                self.enter_pane_window(id);
                self.restart_pane(id);
                self.redraw();
            }
        }
        next
    }

    /// Writes the end message into the pane's own screen.
    fn end_message(&mut self, id: PaneId, text: &str, divider: &str) {
        if let Some(p) = self.panes.get_mut(&id) {
            p.own_mut().append_end_message(text, divider);
        }
    }

    /// The end bars' answers. False for other bars.
    pub(super) fn session_end_bar(&mut self, pane: PaneId, id: &str, button: Option<&str>, event_loop: &ActiveEventLoop) -> bool {
        match id {
            SURVIVORS_BAR => true,
            se::RESTART_BAR => {
                match button {
                    Some(b) if b == se::RESTART => self.restart_answered(pane),
                    Some(b) if b == se::DONT_ASK_AGAIN => {
                        self.globals.set("SuppressRestartAnnouncement", toml::Value::Boolean(true));
                        self.save_globals();
                    }
                    _ => {}
                }
                true
            }
            se::TMUX_CLOSE_BAR => {
                let always = match button {
                    Some(b) if b == se::ALWAYS => true,
                    Some(b) if b == se::NEVER => false,
                    _ => return true,
                };
                self.globals.set("TmuxWindowsShouldCloseAfterDetach", toml::Value::Boolean(always));
                self.save_globals();
                // Every session asking hears the answer (`NC:878-893`).
                let mut asking: Vec<PaneId> = self.bars.iter().filter(|(_, b)| b.has(se::TMUX_CLOSE_BAR)).map(|(p, _)| *p).collect();
                if !asking.contains(&pane) {
                    asking.push(pane);
                }
                for p in &asking {
                    if let Some(b) = self.bars.get_mut(p) {
                        b.retain(|x| x.id != se::TMUX_CLOSE_BAR);
                    }
                }
                if always {
                    for p in asking {
                        if self.panes.contains_key(&p) {
                            self.close_pane(p, event_loop);
                        }
                    }
                }
                true
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exits_are_handled_once() {
        assert_eq!(newly_ended(&[(1, true, false), (2, true, true), (3, false, false)]), vec![1]);
    }

    #[test]
    fn held_panes_end_as_no_action() {
        // A REPL window or a dependency run: No Action whatever its profile says, never restarted.
        assert_eq!(end_action_for(None, true, 1), EndAction::NoAction);
        assert_eq!(end_action_for(None, false, 1), EndAction::Close);
        assert_eq!(end_action_for(None, false, 2), EndAction::Restart);
        assert_eq!(end_action_for(Some(EndAction::NoAction), false, 1), EndAction::NoAction, "a git window follows no profile");
        assert!(!restartable_for(false, false, false, true));
        assert!(!restartable_for(false, false, true, false), "a single-use window never restarts");
        assert!(!restartable_for(true, false, false, false));
        assert!(!restartable_for(false, true, false, false));
        assert!(restartable_for(false, false, false, false));
    }
}
