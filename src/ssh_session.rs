//! One pane's SSH integration state: the connecting queue (`PS:20277-20281`, `PS:4411-4430`),
//! the hop chain EndSSH ends (`PS:20253-20275`) and the gate that keeps the conductor out of
//! sessions until 14b's conductor hook exists (ruling 14a-1).

use crate::ssh_handshake::{bootstrap, end_banner, Handshake, CONDUCTOR_SH};

/// Whether the conductor hook exists. False in 14a: no pane enters the connecting state and
/// no conductor is typed (14b Task 7 sets it true).
pub const CONDUCTOR_READY: bool = false;
/// The most bytes the connecting queue keeps (iTerm2's is unbounded); later bytes are dropped.
pub const QUEUE_LIMIT: usize = 1 << 20;

/// Written past the connecting queue when EndSSH ends hops: it2ssh's local read loop waits
/// for a line, in case ssh died while the conductor was being typed (`PS:20267-20269`).
pub const END_WRITE: &[u8] = b"\x03\n";

/// One conductor in a pane's chain: it2ssh's unique id and where it runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hop {
    /// The `clientUniqueID` it2ssh sent.
    pub unique_id: String,
    /// Where the session is while this hop is the innermost (the banner's "now at").
    pub location: String,
}

/// A pane's nested conductors, outermost first (iTerm2's conductor and its parents).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Chain {
    pub hops: Vec<Hop>,
}

impl Chain {
    /// Whether any hop has `unique_id` (`ancestryContainsClientUniqueID:`).
    pub fn contains(&self, unique_id: &str) -> bool {
        self.hops.iter().any(|h| h.unique_id == unique_id)
    }

    /// Removes hops from the innermost outward through the one with `unique_id`, and counts
    /// them; 0 and nothing removed when no hop has it (`PS:20256-20266`).
    pub fn end(&mut self, unique_id: &str) -> usize {
        match self.hops.iter().rposition(|h| h.unique_id == unique_id) {
            Some(i) => self.hops.drain(i..).count(),
            None => 0,
        }
    }

    /// Where the innermost remaining hop runs (`screenSSHLocation`); None with no hops.
    pub fn location(&self) -> Option<&str> {
        self.hops.last().map(|h| h.location.as_str())
    }
}

/// A pane's SSH integration state.
#[derive(Debug, Default)]
pub struct PaneSsh {
    /// While connecting (iTerm2's `_connectingSSH`): what the pane wrote meanwhile, keys and
    /// reports alike, for the conductor to send (`_queuedConnectingSSH`).
    pub connecting: Option<Vec<u8>>,
    /// The conductors running in this pane (14b keeps it in step with them).
    pub chain: Chain,
}

impl PaneSsh {
    /// `it2ssh=` arrived (`screenWillBeginSSHIntegration`): a fresh empty queue, when the
    /// conductor hook exists.
    pub fn begin(&mut self, ready: bool) {
        if ready {
            self.connecting = Some(Vec::new());
        }
    }

    /// Keeps `bytes` while connecting (true), up to `QUEUE_LIMIT` in all; false when they go
    /// to the session as usual.
    pub fn queue(&mut self, bytes: &[u8]) -> bool {
        let Some(q) = self.connecting.as_mut() else { return false };
        let room = QUEUE_LIMIT.saturating_sub(q.len());
        q.extend_from_slice(&bytes[..bytes.len().min(room)]);
        true
    }

    /// Ends connecting and hands over what was queued (`conductorStopQueueingInput`).
    pub fn stop_queueing(&mut self) -> Vec<u8> {
        self.connecting.take().unwrap_or_default()
    }

    /// EndSSH (`PS:20253-20275`): connecting ends and its queue is dropped; when hops ended,
    /// the banner to show, and the caller writes `END_WRITE` first.
    pub fn end_ssh(&mut self, unique_id: &str) -> Option<String> {
        // 14a drops the queue even when no hop matches; iTerm2 keeps it for a later conductor.
        self.connecting = None;
        match self.chain.end(unique_id) {
            0 => None,
            count => Some(end_banner(count, self.chain.location())),
        }
    }
}

/// The bootstrap to type for a version 3 `SendConductor`, when the conductor hook exists.
pub fn conductor_bytes(h: &Handshake, ready: bool) -> Option<Vec<u8>> {
    ready.then(|| bootstrap(CONDUCTOR_SH, h))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handshake_waits_for_the_conductor_hook() {
        let h = Handshake { token: "none".into(), unique_id: "1".into(), encoded_ba: "QQ==".into(), ssh_args: "aA==".into() };
        let mut s = PaneSsh::default();
        s.begin(false);
        assert_eq!(s.connecting, None);
        assert!(!s.queue(b"ls\r"));
        assert_eq!(conductor_bytes(&h, false), None);
        s.begin(true);
        assert_eq!(s.connecting, Some(vec![]));
        assert!(s.queue(b"ls\r"));
        assert_eq!(s.stop_queueing(), b"ls\r".to_vec());
        assert_eq!(s.connecting, None);
        assert_eq!(conductor_bytes(&h, true), Some(bootstrap(CONDUCTOR_SH, &h)));
        // Removed by 14b Task 7.
        let ready = CONDUCTOR_READY;
        assert!(!ready);
        s.begin(true);
        assert!(s.queue(&vec![b'x'; QUEUE_LIMIT + 10]));
        assert!(s.queue(b"more"));
        assert_eq!(s.connecting.as_ref().map(Vec::len), Some(QUEUE_LIMIT));
    }

    fn hop(unique_id: &str, location: &str) -> Hop {
        Hop { unique_id: unique_id.into(), location: location.into() }
    }

    #[test]
    fn chain_follows_iterm2() {
        let (a, b, c) = (hop("u1", "A"), hop("u2", "B"), hop("u3", "C"));
        let mut chain = Chain { hops: vec![a.clone(), b, c] };
        assert!(chain.contains("u1"));
        assert!(!chain.contains("u9"));
        assert_eq!(chain.location(), Some("C"));
        let before = chain.clone();
        assert_eq!(chain.end("u9"), 0);
        assert_eq!(chain, before);
        assert_eq!(chain.end("u2"), 2);
        assert_eq!(chain.hops, vec![a]);
        assert_eq!(chain.location(), Some("A"));
        assert_eq!(chain.end("u1"), 1);
        assert_eq!(chain.location(), None);
        assert!(chain.hops.is_empty());
        assert_eq!(chain.end("u1"), 0);
    }

    #[test]
    fn end_ssh_releases_the_local_side() {
        let mut s = PaneSsh { connecting: Some(b"typed".to_vec()), chain: Chain { hops: vec![hop("u1", "A"), hop("u2", "B")] } };
        assert_eq!(s.end_ssh("u2").as_deref(), Some("ssh exited \u{2014} now at A."));
        assert_eq!(s.connecting, None);
        assert_eq!(s.chain.hops, vec![hop("u1", "A")]);
        s.connecting = Some(b"typed".to_vec());
        assert_eq!(s.end_ssh("zz"), None);
        assert_eq!(s.connecting, None);
        assert_eq!(s.chain.hops, vec![hop("u1", "A")]);
        assert_eq!(s.end_ssh("u1").as_deref(), Some("ssh exited."));
        assert_eq!(END_WRITE, b"\x03\n");
    }
}
