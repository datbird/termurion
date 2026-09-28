//! One pane's SSH integration state: the connecting queue (`PS:20277-20281`, `PS:4411-4430`),
//! the hop chain EndSSH ends (`PS:20253-20275`) and the gate that kept the conductor out of
//! sessions until 14b's conductor hook existed (ruling 14a-1).

use crate::ssh_handshake::{bootstrap, end_banner, Handshake, CONDUCTOR_SH};
use std::path::{Path, PathBuf};

/// Whether the conductor hook exists: true since 14b (it was false in 14a, when no pane
/// entered the connecting state and no conductor was typed).
pub const CONDUCTOR_READY: bool = true;
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

    /// A conductor hooked: its hop becomes the innermost.
    pub fn push(&mut self, hop: Hop) {
        self.hops.push(hop);
    }

    /// A conductor went away: its hop, the innermost, goes too.
    pub fn pop(&mut self) -> Option<Hop> {
        self.hops.pop()
    }

    /// How many hops `end` would remove for `unique_id`, removing none; 0 when no hop has it.
    pub fn count_to(&self, unique_id: &str) -> usize {
        self.hops.iter().rposition(|h| h.unique_id == unique_id).map_or(0, |i| self.hops.len() - i)
    }

    /// Where the innermost remaining hop runs (`screenSSHLocation`); None with no hops.
    pub fn location(&self) -> Option<&str> {
        self.hops.last().map(|h| h.location.as_str())
    }
}

/// A pane's SSH integration state.
#[derive(Debug, Default)]
pub struct PaneSsh {
    /// While connecting (iTerm2's `_connectingSSH`): the typed input and pastes the pane wrote
    /// meanwhile, for the conductor to send (`_queuedConnectingSSH`). Terminal reports and
    /// other no-broadcast writes pass it (`Pane::reply`, `write_no_broadcast`).
    pub connecting: Option<Vec<u8>>,
    /// The conductors running in this pane (14b keeps it in step with them).
    pub chain: Chain,
    /// The pane was started from an SSH profile (iTerm2's `_sshState == iTermSSHStateProfile`);
    /// 14b's conductor clears it once it hooks (`PS:20129-20132`).
    pub profile_ssh: bool,
    /// The SSH profile's remote initial directory (`profile::ssh_initial_directory`), used
    /// only while `profile_ssh` and there is no conductor yet.
    pub initial_directory: Option<String>,
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

/// The pane banner of an SSH profile with SSH Integration on when `it2ssh.exe` is missing.
pub const NO_IT2SSH: &str = "The utilities folder has no it2ssh.exe, so this SSH profile runs plain ssh.";

/// Why an SSH profile with SSH Integration on runs plain ssh: `it2ssh.exe` is missing
/// (`NO_IT2SSH`); None otherwise.
pub fn profile_banner(ssh: bool, integration_on: bool, it2ssh_present: bool) -> Option<&'static str> {
    (ssh && integration_on && !it2ssh_present).then_some(NO_IT2SSH)
}

/// The `it2ssh.exe` an SSH profile runs: in the utilities folder `utilities::ensure` made
/// (None when it refused one, so its network and link checks hold), looked for only once the
/// conductor hook exists; None when it is missing.
pub fn profile_it2ssh(folder: Option<&Path>, ready: bool, is_file: impl Fn(&Path) -> bool) -> Option<PathBuf> {
    if !ready {
        return None;
    }
    folder.map(|f| f.join("it2ssh.exe")).filter(|exe| is_file(exe))
}

/// The bootstrap to type for a version 3 `SendConductor`, when the conductor hook exists.
pub fn conductor_bytes(h: &Handshake, ready: bool) -> Option<Vec<u8>> {
    ready.then(|| bootstrap(CONDUCTOR_SH, h))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_banner_follows_the_gate() {
        assert_eq!(profile_banner(true, true, false), Some(NO_IT2SSH));
        assert_eq!(profile_banner(true, true, true), None);
        assert_eq!(profile_banner(true, false, false), None);
        assert_eq!(profile_banner(false, true, false), None);
        assert_eq!(NO_IT2SSH, "The utilities folder has no it2ssh.exe, so this SSH profile runs plain ssh.");
    }

    #[test]
    fn profile_it2ssh_follows_ensure() {
        let asked = std::cell::Cell::new(0);
        let yes = |_: &Path| {
            asked.set(asked.get() + 1);
            true
        };
        let folder = Path::new(r"C:\Tandem\utilities");
        assert_eq!(profile_it2ssh(Some(folder), false, yes), None);
        assert_eq!(asked.get(), 0, "nothing is looked at before the conductor hook exists");
        assert_eq!(profile_it2ssh(None, true, yes), None, "a folder ensure refused is never probed");
        assert_eq!(asked.get(), 0);
        assert_eq!(profile_it2ssh(Some(folder), true, |_| false), None);
        assert_eq!(profile_it2ssh(Some(folder), true, yes), Some(folder.join("it2ssh.exe")));
        assert_eq!(asked.get(), 1);
    }

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
    fn chain_mirrors_the_conductors() {
        let (a, b) = (hop("u1", "A"), hop("u2", "B"));
        let mut chain = Chain::default();
        chain.push(a.clone());
        chain.push(b.clone());
        assert_eq!(chain.count_to("u1"), 2);
        assert_eq!(chain.count_to("u2"), 1);
        let before = chain.clone();
        assert_eq!(chain.count_to("zz"), 0);
        assert_eq!(chain, before, "counting removes nothing");
        assert_eq!(chain.pop(), Some(b));
        assert_eq!(chain.hops, vec![a]);
        let ready = CONDUCTOR_READY;
        assert!(ready);
    }

    #[test]
    fn end_ssh_releases_the_local_side() {
        let mut s = PaneSsh { connecting: Some(b"typed".to_vec()), chain: Chain { hops: vec![hop("u1", "A"), hop("u2", "B")] }, ..Default::default() };
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
