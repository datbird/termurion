//! SSH recovery data (milestone 14c): the parser tree a restored or recovered hook follows
//! (`CD:1551-1572`, `CD:3106-3114`), the recovery lines framer.py prints (`F:1300-1321`) and
//! the keys a saved session keeps them under (`PS:7333-7337`). Pure. Every value here can come
//! from the remote host (a recovery line) or a state file (a saved tree): sizes and shapes are
//! checked before anything is built from them.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The session table's keys (`PS:7333-7337`).
pub const CONDUCTOR_KEY: &str = "Conductor";
pub const DCS_ID_KEY: &str = "Conductor DCS ID";
pub const TREE_KEY: &str = "Conductor Parser Tree";
/// Termurion's (F27): the session is or was an ssh session, so a reattached pane may recover in
/// band. Saved with the host key.
pub const SESSION_KEY: &str = "SSH Session";

/// The banners of `TD:4056-4090`.
pub const RECOVERING: &str = "Recovering ssh connection\u{2026}";
pub const RECOVERED: &str = "ssh connection recovered!";

/// The most nodes and the longest id a saved tree may hold (Termurion's): a state file is read
/// as untrusted, and each node makes a hook.
pub const TREE_MAX_NODES: usize = 64;
const ID_MAX: usize = 256;
/// The longest value one recovery line may set (F13: a login reply's bound).
pub const VALUE_MAX: usize = 16 * 1024;

/// The parser tree (`VP:481-532`): a framed pid, or 0 for "no special parsing here", to the
/// dcs id and the subtree under it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Tree(pub BTreeMap<i32, (String, Tree)>);

impl Tree {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    fn walk(&self, depth: usize, nodes: &mut usize) -> Option<()> {
        // One level per conductor: a hook past `MAX_DEPTH` never hooks, so no deeper tree means
        // anything.
        if depth > crate::ssh_hook::MAX_DEPTH as usize + 1 {
            return None;
        }
        for (pid, (id, sub)) in &self.0 {
            *nodes += 1;
            if *pid < 0 || id.len() > ID_MAX || *nodes > TREE_MAX_NODES {
                return None;
            }
            sub.walk(depth + 1, nodes)?;
        }
        Some(())
    }

    /// Whether the tree is one Termurion would have saved: no negative pid, ids of a sane length,
    /// at most `TREE_MAX_NODES` nodes and one level per possible hook.
    pub fn valid(&self) -> bool {
        self.walk(0, &mut 0).is_some()
    }
}

/// The tree of a chain of conductors, outermost first, each its framed pid (None when it is
/// not framing) and dcs id (`CD:1551-1572`): a framing one keys its pid, a non-framing one 0,
/// and the innermost not framing adds nothing.
pub fn chain_tree(links: &[(Option<i32>, String)]) -> Tree {
    let Some(((pid, id), rest)) = links.split_first() else { return Tree::default() };
    if rest.is_empty() {
        return match pid {
            Some(pid) => Tree(BTreeMap::from([(*pid, (id.clone(), Tree::default()))])),
            None => Tree::default(),
        };
    }
    Tree(BTreeMap::from([(pid.unwrap_or(0), (id.clone(), chain_tree(rest)))]))
}

/// What the recovery lines have said so far (`RecoveryInfo`, `CD:1438-1466`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RecoveryInfo {
    pub login: Option<i32>,
    pub dcs_id: Option<String>,
    pub sshargs: Option<String>,
    pub bool_args: Option<String>,
    pub client_unique_id: Option<String>,
    pub version: Option<i32>,
}

impl RecoveryInfo {
    /// One `:recovery: <key> <value>` line's key and value (`CD:2775-2800`). False for a line
    /// iTerm2 skips: a missing value, an unknown key, a number that is not one, a login that is
    /// not a pid (Termurion: a pid is above 0, since the autopoll and notification pseudo pids
    /// and 0 name no process), or a value past `VALUE_MAX`.
    pub fn set(&mut self, key: &str, value: &str) -> bool {
        if value.len() > VALUE_MAX {
            return false;
        }
        match key {
            "version" => match value.parse::<i32>() {
                Ok(v) => self.version = Some(v),
                Err(_) => return false,
            },
            "login" => match value.parse::<i32>() {
                Ok(pid) if pid > 0 => self.login = Some(pid),
                _ => return false,
            },
            "dcsID" => self.dcs_id = Some(value.to_string()),
            "sshargs" => self.sshargs = Some(value.to_string()),
            "boolArgs" => self.bool_args = Some(value.to_string()),
            "clientUniqueID" => self.client_unique_id = Some(value.to_string()),
            _ => return false,
        }
        true
    }

    /// `finished` (`CD:1448-1465`): everything but the version is needed; a missing version is
    /// 1. `parentage` is the conductors above the recovered one, outermost first.
    pub fn finished(&self, parentage: Vec<(i32, String)>) -> Option<Finished> {
        Some(Finished {
            login: self.login?,
            dcs_id: self.dcs_id.clone()?,
            sshargs: self.sshargs.clone()?,
            bool_args: self.bool_args.clone()?,
            client_unique_id: self.client_unique_id.clone()?,
            version: self.version.unwrap_or(1),
            parentage,
        })
    }
}

/// A finished recovery (`FinishedRecoveryInfo`, `CD:1424-1436`).
#[derive(Clone, Debug, PartialEq)]
pub struct Finished {
    pub login: i32,
    pub dcs_id: String,
    pub sshargs: String,
    pub bool_args: String,
    pub client_unique_id: String,
    pub version: i32,
    /// The framed pid (0: not framing) and dcs id of each conductor above, outermost first.
    pub parentage: Vec<(i32, String)>,
}

impl Finished {
    /// The parser tree of the whole chain down to the recovered conductor (`CD:3106-3114`).
    pub fn tree(&self) -> Tree {
        let mut links: Vec<(Option<i32>, String)> = self.parentage.iter().map(|(pid, id)| ((*pid != 0).then_some(*pid), id.clone())).collect();
        links.push((Some(self.login), self.dcs_id.clone()));
        chain_tree(&links)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(entries: Vec<(i32, &str, Tree)>) -> Tree {
        Tree(entries.into_iter().map(|(pid, id, sub)| (pid, (id.to_string(), sub))).collect())
    }

    #[test]
    fn tree_follows_iterm2() {
        let one = chain_tree(&[(Some(100), "D0".into())]);
        assert_eq!(one, t(vec![(100, "D0", Tree::default())]));
        let two = chain_tree(&[(Some(100), "D0".into()), (Some(200), "D1".into())]);
        assert_eq!(two, t(vec![(100, "D0", t(vec![(200, "D1", Tree::default())]))]));
        // A non-framing outer conductor needs no special parsing: key 0.
        let outer_plain = chain_tree(&[(None, "D0".into()), (Some(200), "D1".into())]);
        assert_eq!(outer_plain, t(vec![(0, "D0", t(vec![(200, "D1", Tree::default())]))]));
        // An innermost that is not framing adds nothing (`mysubtree` is empty).
        assert_eq!(chain_tree(&[(None, "D0".into())]), Tree::default());
        assert_eq!(chain_tree(&[(Some(100), "D0".into()), (None, "D1".into())]), t(vec![(100, "D0", Tree::default())]));
        assert_eq!(chain_tree(&[]), Tree::default());
        // The JSON round trip keeps it (the key is the pid as text).
        let json = serde_json::to_string(&two).unwrap();
        assert_eq!(json, r#"{"100":["D0",{"200":["D1",{}]}]}"#);
        assert_eq!(serde_json::from_str::<Tree>(&json).unwrap(), two);
        // A finished recovery's tree: its parentage then the login.
        let f = Finished { login: 4242, dcs_id: "D0".into(), sshargs: String::new(), bool_args: String::new(), client_unique_id: String::new(), version: 2, parentage: vec![(100, "P".into())] };
        assert_eq!(f.tree(), t(vec![(100, "P", t(vec![(4242, "D0", Tree::default())]))]));
        // A parent that does not frame is key 0.
        let g = Finished { parentage: vec![(0, "P".into())], ..f };
        assert_eq!(g.tree(), t(vec![(0, "P", t(vec![(4242, "D0", Tree::default())]))]));
    }

    #[test]
    fn a_saved_tree_is_checked() {
        assert!(chain_tree(&[(Some(100), "D0".into()), (Some(200), "D1".into())]).valid());
        assert!(Tree::default().valid());
        // A negative pid, a long id, too many levels or too many nodes is not a tree Termurion saved.
        assert!(!t(vec![(-5, "D", Tree::default())]).valid());
        assert!(!t(vec![(1, &"x".repeat(300), Tree::default())]).valid());
        let mut deep = Tree::default();
        for pid in 1..=20 {
            deep = t(vec![(pid, "D", deep)]);
        }
        assert!(!deep.valid());
        let wide = Tree((1..=100).map(|pid| (pid, ("D".to_string(), Tree::default()))).collect());
        assert!(!wide.valid());
        // serde_json refuses text nested past its own limit, so a hostile file cannot overflow
        // the stack.
        let bomb = format!("{}{}", r#"{"1":["d","#.repeat(500), "{}".to_string() + &"]}".repeat(500));
        assert!(serde_json::from_str::<Tree>(&bomb).is_err());
    }

    #[test]
    fn recovery_values_are_checked() {
        let mut info = RecoveryInfo::default();
        assert!(info.set("version", "2") && info.set("login", "4242") && info.set("dcsID", "D0"));
        assert!(info.set("sshargs", "-p 22 h") && info.set("boolArgs", "46") && info.set("clientUniqueID", "99"));
        // Text that is not a number, a login that names no process, an unknown key and a
        // value past the cap change nothing.
        assert!(!info.set("version", "x") && !info.set("login", "0") && !info.set("login", "-1000") && !info.set("login", "abc"));
        assert!(!info.set("bogus", "1") && !info.set("sshargs", &"a".repeat(VALUE_MAX + 1)));
        assert_eq!((info.login, info.version, info.sshargs.as_deref()), (Some(4242), Some(2), Some("-p 22 h")));
        let f = info.finished(vec![(100, "P".into())]).unwrap();
        assert_eq!((f.login, f.version, f.parentage.len()), (4242, 2, 1));
        // No version is 1; a missing login is not finished.
        info.version = None;
        assert_eq!(info.finished(vec![]).unwrap().version, 1);
        info.login = None;
        assert!(info.finished(vec![]).is_none());
        assert_eq!(RECOVERING, "Recovering ssh connection\u{2026}");
        assert_eq!(RECOVERED, "ssh connection recovered!");
    }
}
