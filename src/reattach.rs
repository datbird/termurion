//! Reattach at launch: which saved sessions take over their still-running shell in the
//! session host, which start a new shell, and which host sessions nobody claims (orphans,
//! adopted into one new window as iTerm2 does).

use crate::host_proto::Live;

/// A saved session's host session: its id and pid, and the output offset its saved contents
/// reach.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostKey {
    pub id: u64,
    pub pid: u32,
    pub offset: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Plan {
    /// (saved session, host session id, output offset to replay from).
    pub attach: Vec<(usize, u64, u64)>,
    /// Saved sessions that start a new shell (with the banner).
    pub fresh: Vec<usize>,
    /// Running host sessions no saved session claims.
    pub orphans: Vec<u64>,
    /// Ended host sessions no saved session claims: the host can forget them.
    pub forget: Vec<u64>,
}

/// `saved` is each saved session's host key (None: it ran inside Tandem); `saved_instance`
/// the host instance the state names; `instance` and `live` what the host says now.
pub fn plan(saved: &[Option<HostKey>], saved_instance: Option<&str>, instance: &str, live: &[Live]) -> Plan {
    let mut out = Plan::default();
    let same_host = saved_instance == Some(instance);
    let mut claimed = Vec::new();
    for (i, k) in saved.iter().enumerate() {
        let hit = k.as_ref().filter(|_| same_host).and_then(|k| live.iter().find(|l| l.id == k.id && l.pid == k.pid).map(|l| (l.id, k.offset)));
        match hit {
            Some((id, offset)) => {
                claimed.push(id);
                out.attach.push((i, id, offset));
            }
            None => out.fresh.push(i),
        }
    }
    for l in live.iter().filter(|l| !claimed.contains(&l.id)) {
        if l.exited.is_some() {
            out.forget.push(l.id);
        } else {
            out.orphans.push(l.id);
        }
    }
    out
}

/// A forced repaint: one column wider, then back (iTerm2's jiggle).
pub fn jiggle(cols: u16, rows: u16) -> [(u16, u16); 2] {
    let other = if cols == u16::MAX { cols - 1 } else { cols + 1 };
    [(other, rows), (cols, rows)]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn live(id: u64, pid: u32, exited: Option<u32>) -> Live {
        Live { id, pid, cmdline: "pwsh".into(), cwd: String::new(), cols: 80, rows: 24, exited, end: 500 }
    }

    fn key(id: u64, pid: u32, offset: u64) -> Option<HostKey> {
        Some(HostKey { id, pid, offset })
    }

    #[test]
    fn reattach_plan_matches_by_id() {
        let p = plan(&[key(1, 10, 100), None, key(2, 20, 0), key(9, 90, 5)], Some("A"), "A", &[live(1, 10, None), live(2, 20, None), live(3, 30, None)]);
        assert_eq!(p.attach, vec![(0, 1, 100), (2, 2, 0)]);
        assert_eq!(p.fresh, vec![1, 3], "no key, or a session the host no longer has");
        assert_eq!(p.orphans, vec![3]);
        let reused = plan(&[key(1, 99, 0)], Some("A"), "A", &[live(1, 10, None)]);
        assert_eq!((reused.attach, reused.fresh, reused.orphans), (vec![], vec![0], vec![1]), "same id, another pid: not the same shell");
    }

    #[test]
    fn other_instances_are_orphans() {
        let p = plan(&[key(1, 10, 0)], Some("old"), "new", &[live(1, 10, None)]);
        assert_eq!((p.attach, p.fresh, p.orphans), (vec![], vec![0], vec![1]));
        let none = plan(&[key(1, 10, 0)], None, "new", &[live(1, 10, None)]);
        assert_eq!(none.orphans, vec![1]);
    }

    #[test]
    fn ended_sessions_show_as_ended() {
        let p = plan(&[key(1, 10, 0)], Some("A"), "A", &[live(1, 10, Some(3)), live(2, 20, Some(0))]);
        assert_eq!(p.attach, vec![(0, 1, 0)], "a claimed ended session is attached, so its pane shows the end");
        assert_eq!((p.orphans, p.forget), (vec![], vec![2]), "an unclaimed ended session is forgotten, not adopted");
    }

    #[test]
    fn jiggle_widens_then_restores() {
        assert_eq!(jiggle(80, 24), [(81, 24), (80, 24)]);
        assert_eq!(jiggle(u16::MAX, 5), [(u16::MAX - 1, 5), (u16::MAX, 5)], "at the top it narrows instead");
    }
}
