//! One pane's SSH integration state: the connecting queue (`PS:20277-20281`, `PS:4411-4430`)
//! and the gate that keeps the conductor out of sessions until 14b's conductor hook exists
//! (ruling 14a-1).

use crate::ssh_handshake::{bootstrap, Handshake, CONDUCTOR_SH};

/// Whether the conductor hook exists. False in 14a: no pane enters the connecting state and
/// no conductor is typed (14b Task 7 sets it true).
pub const CONDUCTOR_READY: bool = false;
/// The most bytes the connecting queue keeps (iTerm2's is unbounded); later bytes are dropped.
pub const QUEUE_LIMIT: usize = 1 << 20;

/// A pane's SSH integration state.
#[derive(Debug, Default)]
pub struct PaneSsh {
    /// While connecting (iTerm2's `_connectingSSH`): what the pane wrote meanwhile, keys and
    /// reports alike, for the conductor to send (`_queuedConnectingSSH`).
    pub connecting: Option<Vec<u8>>,
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
}
