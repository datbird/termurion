//! File requests over a pane's framing conductor (milestone 14d): each is sent with an id
//! Tandem chose, remembered with what is waiting for it, and routed when its reply comes
//! (`CD:1953-1960`, `CD:2273-2291`). Later tasks add the waiters that need the transfer
//! engine, the panel, suggestions and search; this is the one dispatch point (`ssh_route`).

use super::*;
use crate::conductor::{Command, Conductor, Handler};
use crate::ssh_files::FileSub;

/// What waits for one request's answer.
#[derive(Clone, Debug, PartialEq)]
#[allow(dead_code)]
pub(super) enum Waiter {
    /// Nobody: the answer is dropped (a fire-and-forget request, or one whose caller is gone).
    Ignore,
    /// A step of the transfer engine's job `job`, tagged `tag` (Task 3).
    Transfer { job: u64, tag: u64 },
}

// Used by the transfer, panel, suggestion and search tasks that follow.
#[allow(dead_code)]
impl App {
    /// The pane's innermost conductor, only while its framer runs to answer (P7: framing and
    /// not unhooked; `Conductor::framing` stays true after a disconnect, and a request sent
    /// then would queue forever).
    pub(super) fn framing(&self, pane: PaneId) -> Option<&Conductor> {
        self.panes.get(&pane)?.conductor.as_deref().filter(|c| c.frames_keys())
    }

    pub(super) fn framing_mut(&mut self, pane: PaneId) -> Option<&mut Conductor> {
        self.panes.get_mut(&pane)?.conductor.as_deref_mut().filter(|c| c.frames_keys())
    }

    /// Sends a file request. False, with nothing sent and nothing remembered, when the pane
    /// has no framing conductor: the caller answers its own waiter (a -1 reply), or a job
    /// would wait forever.
    pub(super) fn ssh_send(&mut self, pane: PaneId, sub: FileSub, high: bool, waiter: Waiter) -> bool {
        let id = self.ssh_next_file_id;
        let Some(c) = self.framing_mut(pane) else { return false };
        c.send(Command::FramerFile(sub), Handler::File { id, lines: Vec::new() }, high);
        self.ssh_next_file_id += 1;
        self.ssh_waiters.insert((pane, id), waiter);
        // The write goes out through the effect loop.
        self.ssh_apply(pane);
        true
    }

    /// Cancels the queued requests `pred` matches (a request already written is not
    /// recalled). Each comes back as a -1 reply when its turn comes, and is routed like any other.
    pub(super) fn ssh_cancel_queued(&mut self, pane: PaneId, pred: &dyn Fn(&FileSub) -> bool) {
        let Some(c) = self.framing_mut(pane) else { return };
        c.cancel_enqueued(&|cmd| matches!(cmd, Command::FramerFile(sub) if pred(sub)));
        self.ssh_apply(pane);
    }

    /// Takes the answers the pane's conductors gave and routes them.
    pub(super) fn ssh_files_drain(&mut self, pane: PaneId) {
        let done = self.panes.get_mut(&pane).map(|p| p.take_ssh_files()).unwrap_or_default();
        for (id, content, status) in done {
            self.ssh_file_done(pane, id, content, status);
        }
    }

    /// An answer: its waiter is removed and told. An answer nobody waits for (a duplicate, or
    /// one for a request already failed) is ignored.
    pub(super) fn ssh_file_done(&mut self, pane: PaneId, id: u64, content: String, status: i32) {
        let Some(waiter) = self.ssh_waiters.remove(&(pane, id)) else { return };
        self.ssh_route(pane, waiter, content, status);
    }

    /// The one dispatch point: later tasks add their waiters here.
    pub(super) fn ssh_route(&mut self, _pane: PaneId, waiter: Waiter, _content: String, _status: i32) {
        match waiter {
            Waiter::Ignore => {}
            // Task 3 fills it.
            Waiter::Transfer { .. } => {}
        }
    }

    /// Answers -1 to every waiter of `pane` (its conductor went or the pane did), and
    /// forgets them: a request is never left unanswered (P5).
    pub(super) fn ssh_files_pane_gone(&mut self, pane: PaneId) {
        let ids: Vec<u64> = self.ssh_waiters.keys().filter(|(p, _)| *p == pane).map(|(_, id)| *id).collect();
        for id in ids {
            self.ssh_file_done(pane, id, String::new(), -1);
        }
    }

    /// Fails the waiters of every pane that no longer has a framing conductor. The conductors
    /// answer what they hold themselves (a fail, an unhook and a pop tell their file
    /// requests); this catches what they could not.
    pub(super) fn ssh_files_settle(&mut self) {
        if self.ssh_waiters.is_empty() {
            return;
        }
        let mut gone: Vec<PaneId> = self.ssh_waiters.keys().map(|(p, _)| *p).filter(|p| self.framing(*p).is_none()).collect();
        gone.sort_unstable();
        gone.dedup();
        for pane in gone {
            self.ssh_files_pane_gone(pane);
        }
    }
}
