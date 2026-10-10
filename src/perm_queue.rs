//! The one ordered queue of permission writes of a WebView2 profile (13x6, Task 5 fix). Every
//! profile write (a mirrored grant, a start-up sync, a revoke, a reset) and every read of the
//! profile's states goes through the queue of its store, one at a time, in the order it was asked:
//! the next starts only when the last one's completion came back. A revoke or reset starts a new
//! generation: a write of an older generation that is still queued for the same entry is dropped,
//! and an answer waiting behind a mirrored grant of that entry becomes a Deny, so nothing older
//! lands after it. A failed write is tried again a bounded number of times, then the pages using
//! its origin are reloaded (fail closed).
//!
//! Pure: the `App` issues what `step` returns on a view of the store and reports each completion.

use std::collections::VecDeque;

use crate::layout::PaneId;
use crate::webhost::{PermAnswer, ViewId};

/// How many times one write is tried before the pages using its origin are reloaded.
pub const WRITE_TRIES: u8 = 3;
/// How many reads the start-up sync makes before it gives up until the next view of the store.
pub const SYNC_TRIES: u8 = 3;

/// The start-up sync of a store: its profile agrees with the Tandem table only once a COMPLETE
/// read's plan has landed. Until then it fails closed (no page loads, `App::browser_sync_gate`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Sync {
    /// Not started (a new environment).
    #[default]
    Idle,
    /// A read, or a plan, is in the queue.
    Pending,
    /// A complete read's plan landed.
    Done,
    /// `SYNC_TRIES` reads in a row were incomplete, lost or their plan failed: nothing more until
    /// the next view of the store (or a navigation the person asks for) starts it again.
    GaveUp,
}

/// One queued step.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Item {
    /// `SetPermissionState(kind, origin, state)`.
    Write { generation: u64, kind: i32, origin: String, state: i32, tries: u8 },
    /// `GetNonDefaultPermissionSettings`.
    Read,
    /// A held request's answer, given once the mirrored grant before it has landed.
    Answer { generation: u64, kind: i32, origin: String, pane: PaneId, token: u64, epoch: u64, answer: PermAnswer },
    /// Every page using `origin` reloads (a revoke, or a write that kept failing).
    Reload { origin: String },
    /// The plan of a complete read landed: the sync is done.
    Synced,
}

/// What the `App` carries out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Out {
    Write { view: ViewId, seq: u64, kind: i32, origin: String, state: i32 },
    Read { view: ViewId },
    Answer { pane: PaneId, token: u64, epoch: u64, answer: PermAnswer },
    Reload { origin: String },
    /// The store's sync is done: its waiting pages may load.
    Synced,
    /// The sync gave up after `SYNC_TRIES` tries.
    GaveUp,
}

/// The queue of one store's profile.
#[derive(Debug, Default)]
pub struct ProfileQueue {
    generation: u64,
    seq: u64,
    items: VecDeque<Item>,
    /// The head item is in flight on this view under this sequence number.
    flight: Option<(ViewId, u64)>,
    sync: Sync,
    /// Failed sync tries in a row.
    sync_tries: u8,
}

impl ProfileQueue {
    /// A write of the current generation (a start-up sync's plan entry).
    pub fn write(&mut self, kind: i32, origin: &str, state: i32) {
        self.items.push_back(Item::Write { generation: self.generation, kind, origin: origin.to_string(), state, tries: 0 });
    }

    /// A saved grant: the profile gets it first, then the request is answered.
    pub fn mirror_then_answer(&mut self, kind: i32, origin: &str, pane: PaneId, token: u64, epoch: u64, answer: PermAnswer) {
        self.write(kind, origin, crate::browser_perms::STATE_ALLOW);
        self.items.push_back(Item::Answer { generation: self.generation, kind, origin: origin.to_string(), pane, token, epoch, answer });
    }

    /// A revoke or a reset: a new generation. Older writes of the same entries still queued are
    /// dropped and older answers of them become a Deny; then its writes; then, when `reload` names
    /// an origin, the reload of its pages, which runs only after those writes came back.
    pub fn revoke(&mut self, states: Vec<(i32, String, i32)>, reload: Option<String>) {
        self.generation += 1;
        let now = self.generation;
        let hit = |k: i32, o: &str| states.iter().any(|(sk, so, _)| *sk == k && so == o);
        // The head is never touched while it is in flight: it lands first, then these.
        let keep_head = usize::from(self.flight.is_some());
        let mut i = keep_head;
        while i < self.items.len() {
            match &mut self.items[i] {
                Item::Write { generation, kind, origin, .. } if *generation < now && hit(*kind, origin) => {
                    self.items.remove(i);
                    continue;
                }
                Item::Answer { generation, kind, origin, answer, .. } if *generation < now && hit(*kind, origin) => *answer = PermAnswer::Deny,
                _ => {}
            }
            i += 1;
        }
        for (kind, origin, state) in states {
            self.items.push_back(Item::Write { generation: now, kind, origin, state, tries: 0 });
        }
        if let Some(origin) = reload {
            self.items.push_back(Item::Reload { origin });
        }
    }

    /// A read of the profile's states (one queued at a time).
    pub fn read(&mut self) {
        let skip = usize::from(self.flight.is_some());
        if !self.items.iter().skip(skip).any(|i| *i == Item::Read) {
            self.items.push_back(Item::Read);
        }
    }

    /// What to do now: the markers at the head, then at most one write or read, issued on `view`
    /// (None: no live view of the store, the queue waits).
    pub fn step(&mut self, view: Option<ViewId>) -> Vec<Out> {
        let mut out = Vec::new();
        if self.flight.is_some() {
            return out;
        }
        while let Some(head) = self.items.front() {
            match head {
                Item::Answer { .. } | Item::Reload { .. } | Item::Synced => match self.items.pop_front() {
                    Some(Item::Answer { pane, token, epoch, answer, .. }) => out.push(Out::Answer { pane, token, epoch, answer }),
                    Some(Item::Reload { origin }) => out.push(Out::Reload { origin }),
                    Some(Item::Synced) if self.sync == Sync::Pending => {
                        self.sync = Sync::Done;
                        self.sync_tries = 0;
                        out.push(Out::Synced);
                    }
                    _ => {}
                },
                Item::Write { kind, origin, state, .. } => {
                    let Some(v) = view else { break };
                    self.seq += 1;
                    self.flight = Some((v, self.seq));
                    out.push(Out::Write { view: v, seq: self.seq, kind: *kind, origin: origin.clone(), state: *state });
                    break;
                }
                Item::Read => {
                    let Some(v) = view else { break };
                    self.seq += 1;
                    self.flight = Some((v, self.seq));
                    out.push(Out::Read { view: v });
                    break;
                }
            }
        }
        out
    }

    /// The completion of write `seq` on `view`. A late or unknown one is ignored.
    pub fn written(&mut self, view: ViewId, seq: u64, ok: bool) -> Vec<Out> {
        if self.flight != Some((view, seq)) || !matches!(self.items.front(), Some(Item::Write { .. })) {
            return Vec::new();
        }
        self.flight = None;
        if ok {
            self.items.pop_front();
            return Vec::new();
        }
        self.failed_head()
    }

    /// The write at the head failed (or its view went before it came back). A later write of the
    /// same entry decides it anyway: it is dropped. Else it is tried again, up to `WRITE_TRIES`
    /// times; then it is dropped and the pages using its origin reload, unless a reload of that
    /// origin is already queued behind it.
    fn failed_head(&mut self) -> Vec<Out> {
        let Some(Item::Write { kind, origin, tries, .. }) = self.items.front_mut() else { return Vec::new() };
        *tries += 1;
        let (kind, origin, tries) = (*kind, origin.clone(), *tries);
        let later = |i: &Item| matches!(i, Item::Write { kind: k, origin: o, .. } if *k == kind && *o == origin);
        if self.items.iter().skip(1).any(later) {
            self.items.pop_front();
            return Vec::new();
        }
        if tries < WRITE_TRIES {
            return Vec::new();
        }
        self.items.pop_front();
        crate::dlog!("webhost: a profile permission write of kind {} failed {} times; its pages reload", kind, tries);
        let mut out = Vec::new();
        if !self.items.iter().any(|i| matches!(i, Item::Reload { origin: o } if *o == origin)) {
            out.push(Out::Reload { origin });
        }
        // The profile may now disagree with the table: the sync is not done until it read again.
        out.extend(self.sync_failed(None));
        out
    }

    /// The answer to the read in flight on `view`: `plan` (the writes it calls for) goes first.
    /// None when no read was in flight there (a stale answer, ignored).
    pub fn read_answer(&mut self, view: ViewId, plan: Vec<(i32, String, i32)>, complete: bool) -> Option<Vec<Out>> {
        if !self.flight.is_some_and(|(v, _)| v == view) || self.items.front() != Some(&Item::Read) {
            return None;
        }
        self.flight = None;
        self.items.pop_front();
        let n = plan.len();
        for (kind, origin, state) in plan.into_iter().rev() {
            self.items.push_front(Item::Write { generation: self.generation, kind, origin, state, tries: 0 });
        }
        if complete {
            // Done once the plan has landed: the marker fires only after its writes came back.
            self.items.insert(n, Item::Synced);
            return Some(Vec::new());
        }
        // Fail closed: what was read is rewritten, then the profile is read again.
        Some(self.sync_failed(Some(n)))
    }

    /// One sync try failed (an incomplete or lost read, or a write of its plan that kept failing):
    /// a new read after the first `at` items (None: at the end), or, after `SYNC_TRIES` in a row,
    /// give up until the next `sync`.
    fn sync_failed(&mut self, at: Option<usize>) -> Vec<Out> {
        self.items.retain(|i| *i != Item::Synced);
        if self.sync == Sync::Done {
            self.sync = Sync::Pending;
            self.sync_tries = 0;
        }
        if self.sync != Sync::Pending {
            return Vec::new();
        }
        self.sync_tries += 1;
        if self.sync_tries >= SYNC_TRIES {
            self.sync = Sync::GaveUp;
            self.items.retain(|i| *i != Item::Read);
            return vec![Out::GaveUp];
        }
        let skip = usize::from(self.flight.is_some());
        if !self.items.iter().skip(skip).any(|i| *i == Item::Read) {
            match at {
                Some(n) => self.items.insert(n.min(self.items.len()), Item::Read),
                None => self.items.push_back(Item::Read),
            }
        }
        Vec::new()
    }

    /// Starts the store's sync unless it runs or is done (a new view; a given-up sync starts over).
    pub fn sync(&mut self) {
        if matches!(self.sync, Sync::Pending | Sync::Done) {
            return;
        }
        self.sync = Sync::Pending;
        self.sync_tries = 0;
        self.read();
    }

    /// Whether the store's profile was synced from the table (pages may load).
    pub fn synced(&self) -> bool {
        self.sync == Sync::Done
    }

    /// `view` went away: what was in flight on it is tried again (a write counts a try).
    pub fn view_gone(&mut self, view: ViewId) -> Vec<Out> {
        if !self.flight.is_some_and(|(v, _)| v == view) {
            return Vec::new();
        }
        self.flight = None;
        match self.items.front() {
            Some(Item::Write { .. }) => self.failed_head(),
            // The read is asked again on another view; it counts as a failed sync try.
            Some(Item::Read) => self.sync_failed(Some(0)),
            _ => Vec::new(),
        }
    }

    /// The environment was lost: everything queued goes; waiting answers are Denies.
    pub fn lost(&mut self) -> Vec<Out> {
        self.flight = None;
        self.sync = Sync::Idle;
        self.sync_tries = 0;
        std::mem::take(&mut self.items)
            .into_iter()
            .filter_map(|i| match i {
                Item::Answer { pane, token, epoch, .. } => Some(Out::Answer { pane, token, epoch, answer: PermAnswer::Deny }),
                _ => None,
            })
            .collect()
    }

    /// Nothing queued and nothing in flight.
    pub fn idle(&self) -> bool {
        self.items.is_empty() && self.flight.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALLOW: PermAnswer = PermAnswer::State { state: 1, saves: true };

    fn w(view: ViewId, seq: u64, kind: i32, origin: &str, state: i32) -> Out {
        Out::Write { view, seq, kind, origin: origin.into(), state }
    }

    #[test]
    fn writes_go_one_at_a_time_in_order() {
        let mut q = ProfileQueue::default();
        q.write(4, "https://a.b", 1);
        q.write(3, "https://a.b", 1);
        assert!(q.step(None).is_empty(), "no live view: the queue waits");
        assert_eq!(q.step(Some(7)), vec![w(7, 1, 4, "https://a.b", 1)]);
        assert!(q.step(Some(7)).is_empty(), "the next waits for the completion");
        assert!(q.written(7, 99, true).is_empty() && q.step(Some(7)).is_empty(), "an unknown completion is ignored");
        q.written(7, 1, true);
        assert_eq!(q.step(Some(7)), vec![w(7, 2, 3, "https://a.b", 1)]);
        q.written(7, 2, true);
        assert!(q.step(Some(7)).is_empty() && q.idle());
    }

    #[test]
    fn a_mirrored_grant_is_answered_after_it_lands() {
        let mut q = ProfileQueue::default();
        q.mirror_then_answer(4, "https://a.b", 1, 10, 7, ALLOW);
        assert_eq!(q.step(Some(3)), vec![w(3, 1, 4, "https://a.b", 1)], "the profile first");
        q.written(3, 1, true);
        assert_eq!(q.step(Some(3)), vec![Out::Answer { pane: 1, token: 10, epoch: 7, answer: ALLOW }]);
    }

    #[test]
    fn an_older_write_never_lands_after_a_newer_revoke() {
        let mut q = ProfileQueue::default();
        // A sync's grant is in flight, another and a grant's answer are queued behind it.
        q.write(4, "https://a.b", 1);
        q.write(3, "https://a.b", 1);
        q.mirror_then_answer(4, "https://a.b", 1, 10, 7, ALLOW);
        q.write(4, "https://c.d", 1);
        assert_eq!(q.step(Some(3)), vec![w(3, 1, 4, "https://a.b", 1)]);
        q.revoke(vec![(4, "https://a.b".into(), 0)], None);
        q.written(3, 1, true);
        // The queued kind-4 grant of a.b is gone; its answer is a Deny; kind 3 and c.d are kept.
        let mut seen = Vec::new();
        for seq in 2..10 {
            let outs = q.step(Some(3));
            if outs.is_empty() {
                break;
            }
            seen.extend(outs.clone());
            q.written(3, seq, true);
        }
        assert_eq!(
            seen,
            vec![
                w(3, 2, 3, "https://a.b", 1),
                Out::Answer { pane: 1, token: 10, epoch: 7, answer: PermAnswer::Deny },
                w(3, 3, 4, "https://c.d", 1),
                w(3, 4, 4, "https://a.b", 0),
            ]
        );
    }

    #[test]
    fn a_failed_write_is_retried_then_its_pages_reload() {
        let mut q = ProfileQueue::default();
        q.write(4, "https://a.b", 0);
        q.write(3, "https://c.d", 0);
        for seq in 1..WRITE_TRIES as u64 {
            assert_eq!(q.step(Some(3)), vec![w(3, seq, 4, "https://a.b", 0)], "try {seq}");
            assert!(q.written(3, seq, false).is_empty());
        }
        assert_eq!(q.step(Some(3)), vec![w(3, WRITE_TRIES as u64, 4, "https://a.b", 0)]);
        assert_eq!(q.written(3, WRITE_TRIES as u64, false), vec![Out::Reload { origin: "https://a.b".into() }], "fail closed");
        assert_eq!(q.step(Some(3)), vec![w(3, WRITE_TRIES as u64 + 1, 3, "https://c.d", 0)], "the rest goes on");
        // A failed write that a later write of the same entry overrides is not tried again.
        let mut q = ProfileQueue::default();
        q.write(4, "https://a.b", 1);
        q.step(Some(3));
        q.revoke(vec![(4, "https://a.b".into(), 0)], None);
        assert!(q.written(3, 1, false).is_empty());
        assert_eq!(q.step(Some(3)), vec![w(3, 2, 4, "https://a.b", 0)]);
    }

    #[test]
    fn a_revoke_reloads_only_after_its_writes() {
        let mut q = ProfileQueue::default();
        q.revoke(vec![(4, "https://a.b".into(), 0), (3, "https://a.b".into(), 0)], Some("https://a.b".into()));
        assert_eq!(q.step(Some(3)), vec![w(3, 1, 4, "https://a.b", 0)]);
        q.written(3, 1, true);
        assert_eq!(q.step(Some(3)), vec![w(3, 2, 3, "https://a.b", 0)], "no reload yet");
        q.written(3, 2, true);
        assert_eq!(q.step(Some(3)), vec![Out::Reload { origin: "https://a.b".into() }]);
        // A write that kept failing before a reload of the same origin reloads once, not twice.
        let mut q = ProfileQueue::default();
        q.revoke(vec![(4, "https://a.b".into(), 0)], Some("https://a.b".into()));
        let mut outs = Vec::new();
        for seq in 1..=WRITE_TRIES as u64 {
            q.step(Some(3));
            outs.extend(q.written(3, seq, false));
        }
        outs.extend(q.step(Some(3)));
        assert_eq!(outs, vec![Out::Reload { origin: "https://a.b".into() }]);
    }

    #[test]
    fn reads_are_serialized_with_the_writes() {
        let mut q = ProfileQueue::default();
        q.write(4, "https://a.b", 1);
        q.read();
        q.read();
        assert_eq!(q.step(Some(3)), vec![w(3, 1, 4, "https://a.b", 1)]);
        assert!(q.read_answer(3, vec![], true).is_none(), "no read in flight");
        q.written(3, 1, true);
        assert_eq!(q.step(Some(3)), vec![Out::Read { view: 3 }]);
        assert!(q.read_answer(4, vec![], true).is_none(), "another view's answer is not this read's");
        assert!(q.read_answer(3, vec![(4, "https://x.y".into(), 0)], true).is_some());
        assert_eq!(q.step(Some(3)), vec![w(3, 3, 4, "https://x.y", 0)], "the plan goes first; one read was queued");
        q.written(3, 3, true);
        assert!(q.step(Some(3)).is_empty());
    }

    #[test]
    fn a_view_that_goes_takes_nothing_with_it() {
        let mut q = ProfileQueue::default();
        q.write(4, "https://a.b", 0);
        assert_eq!(q.step(Some(3)), vec![w(3, 1, 4, "https://a.b", 0)]);
        assert!(q.view_gone(9).is_empty());
        assert!(q.view_gone(3).is_empty());
        assert_eq!(q.step(Some(5)), vec![w(5, 2, 4, "https://a.b", 0)], "tried again on another view");
        assert!(q.written(3, 1, true).is_empty() && q.step(Some(5)).is_empty(), "the old completion is ignored");
        q.written(5, 2, true);
        q.read();
        assert_eq!(q.step(Some(5)), vec![Out::Read { view: 5 }]);
        q.view_gone(5);
        assert_eq!(q.step(Some(6)), vec![Out::Read { view: 6 }], "the read is asked again");
        // A lost environment: waiting answers are refused, the rest is dropped.
        q.mirror_then_answer(4, "https://a.b", 1, 10, 7, ALLOW);
        assert_eq!(q.lost(), vec![Out::Answer { pane: 1, token: 10, epoch: 7, answer: PermAnswer::Deny }]);
        assert!(q.idle() && q.step(Some(6)).is_empty());
    }

    /// Runs every write `q` issues on view 3 as succeeded, collecting what came out.
    fn drain(q: &mut ProfileQueue) -> Vec<Out> {
        let mut seen = Vec::new();
        loop {
            let outs = q.step(Some(3));
            if outs.is_empty() {
                return seen;
            }
            for o in &outs {
                if let Out::Write { seq, .. } = o {
                    let seq = *seq;
                    seen.push(o.clone());
                    seen.extend(q.written(3, seq, true));
                } else {
                    seen.push(o.clone());
                }
            }
            if matches!(outs.last(), Some(Out::Read { .. })) {
                return seen;
            }
        }
    }

    #[test]
    fn the_sync_is_done_only_when_a_complete_reads_plan_landed() {
        let mut q = ProfileQueue::default();
        assert!(!q.synced());
        q.sync();
        q.sync();
        assert_eq!(q.step(Some(3)), vec![Out::Read { view: 3 }], "one read");
        assert!(!q.synced(), "asked is not done");
        let outs = q.read_answer(3, vec![(4, "https://a.b".into(), 0)], true).unwrap();
        assert!(outs.is_empty());
        assert!(!q.synced(), "answered is not done: the plan must land");
        let seen = drain(&mut q);
        assert_eq!(seen, vec![w(3, 2, 4, "https://a.b", 0), Out::Synced]);
        assert!(q.synced());
        q.sync();
        assert!(q.step(Some(3)).is_empty(), "done: no second read");
        // A lost environment syncs again from the start.
        q.lost();
        assert!(!q.synced());
        q.sync();
        assert_eq!(q.step(Some(3)), vec![Out::Read { view: 3 }]);
    }

    #[test]
    fn an_incomplete_or_lost_read_is_tried_again_then_gives_up() {
        let mut q = ProfileQueue::default();
        q.sync();
        // An incomplete answer: what was read is rewritten (fail closed), then it reads again.
        assert_eq!(q.step(Some(3)), vec![Out::Read { view: 3 }]);
        assert!(q.read_answer(3, vec![(4, "https://a.b".into(), 0)], false).unwrap().is_empty());
        assert_eq!(drain(&mut q), vec![w(3, 2, 4, "https://a.b", 0), Out::Read { view: 3 }], "the plan, then a new read");
        assert!(!q.synced());
        // The view goes before it answers: read again on another view.
        assert!(q.view_gone(3).is_empty());
        assert_eq!(q.step(Some(5)), vec![Out::Read { view: 5 }]);
        // The third failure in a row gives up until the next view of the store.
        assert_eq!(q.read_answer(5, vec![], false).unwrap(), vec![Out::GaveUp]);
        assert!(q.step(Some(5)).is_empty() && !q.synced());
        q.sync();
        assert_eq!(q.step(Some(5)), vec![Out::Read { view: 5 }], "a new view starts it over");
        assert!(q.read_answer(5, vec![], true).unwrap().is_empty());
        assert_eq!(q.step(Some(5)), vec![Out::Synced]);
        assert!(q.synced(), "a complete answer with nothing to write is done at once");
    }

    #[test]
    fn a_sync_write_that_keeps_failing_reads_again() {
        let mut q = ProfileQueue::default();
        q.sync();
        q.step(Some(3));
        q.read_answer(3, vec![(4, "https://a.b".into(), 0)], true).unwrap();
        let mut outs = Vec::new();
        for seq in 2..2 + WRITE_TRIES as u64 {
            assert_eq!(q.step(Some(3)), vec![w(3, seq, 4, "https://a.b", 0)]);
            outs.extend(q.written(3, seq, false));
        }
        assert_eq!(outs, vec![Out::Reload { origin: "https://a.b".into() }]);
        assert_eq!(q.step(Some(3)), vec![Out::Read { view: 3 }], "no Synced: the plan did not land, so it reads again");
        assert!(!q.synced());
        // A failure after the sync was done makes it read again too (new pages wait meanwhile).
        q.read_answer(3, vec![], true).unwrap();
        assert_eq!(q.step(Some(3)), vec![Out::Synced]);
        q.revoke(vec![(4, "https://c.d".into(), 0)], None);
        for seq in 0..WRITE_TRIES {
            let outs = q.step(Some(3));
            let Some(Out::Write { seq: s, .. }) = outs.first() else { panic!("{outs:?} at {seq}") };
            q.written(3, *s, false);
        }
        assert!(!q.synced(), "a failed write: the profile may disagree with the table");
        assert!(matches!(q.step(Some(3))[..], [Out::Read { .. }]));
    }

    /// Review r1 New 1: a revoke's reset that keeps failing reloads the origin's pages, but the store
    /// is unsynced from then on, so the reload is parked by the page gate and the stale grant never
    /// runs: only a sync landing (a complete read and its plan) lets the page load again.
    #[test]
    fn a_failed_default_write_leaves_the_store_unsynced_for_the_reload_gate() {
        let mut q = ProfileQueue::default();
        q.sync();
        q.step(Some(3));
        q.read_answer(3, vec![], true);
        drain(&mut q);
        assert!(q.synced());
        q.revoke(vec![(4, "https://a.b".into(), 0)], Some("https://a.b".into()));
        let mut outs = Vec::new();
        for seq in 1..=WRITE_TRIES as u64 {
            let step = q.step(Some(3));
            let seq = step.iter().find_map(|o| if let Out::Write { seq, .. } = o { Some(*seq) } else { None }).unwrap_or(seq);
            outs.extend(q.written(3, seq, false));
        }
        outs.extend(q.step(Some(3)));
        assert!(outs.contains(&Out::Reload { origin: "https://a.b".into() }), "its pages are asked to reload: {outs:?}");
        assert!(!q.synced(), "the profile may still hold the Allow");
    }
}
