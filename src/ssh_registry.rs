//! The connected hosts (`SSH/ConductorRegistry.swift:9-67`): the framed conductors of the whole
//! app, by identity. Download Files (14d) is enabled while any is connected.

use crate::ssh_args::Identity;

/// What a pane tells the app about the registry: the app applies these in order.
#[derive(Clone, Debug, PartialEq)]
pub enum Change {
    Add(String, Identity),
    Remove(String),
}

/// Framed conductors by guid; identities come out by first add, one each.
#[derive(Default)]
pub struct Registry {
    entries: Vec<(String, Identity)>,
}

impl Registry {
    /// addConductor: a guid already here is replaced in place.
    pub fn add(&mut self, guid: &str, identity: Identity) {
        match self.entries.iter_mut().find(|(g, _)| g == guid) {
            Some(entry) => entry.1 = identity,
            None => self.entries.push((guid.to_string(), identity)),
        }
    }

    /// remove(conductorGUID:): an unknown guid is harmless.
    pub fn remove(&mut self, guid: &str) {
        self.entries.retain(|(g, _)| g != guid);
    }

    /// Whether any conductor is connected.
    pub fn any(&self) -> bool {
        !self.entries.is_empty()
    }

    /// The connected identities, in the order each was first added, once each.
    pub fn identities(&self) -> Vec<Identity> {
        let mut out: Vec<Identity> = Vec::new();
        for (_, identity) in &self.entries {
            if !out.contains(identity) {
                out.push(identity.clone());
            }
        }
        out
    }

    /// Applies a pane's changes in order. True when there were any.
    pub fn apply(&mut self, changes: Vec<Change>) -> bool {
        let any = !changes.is_empty();
        for change in changes {
            match change {
                Change::Add(guid, identity) => self.add(&guid, identity),
                Change::Remove(guid) => self.remove(&guid),
            }
        }
        any
    }
}

#[cfg(test)]
mod tests {
    use super::{Change, Registry};
    use crate::ssh_args::Identity;

    fn id(host: &str) -> Identity {
        Identity { host: host.into(), hostname: format!("{host}.example"), username: Some("u".into()), port: 22 }
    }

    #[test]
    fn registry_follows_iterm2() {
        let (a, b) = (id("a"), id("b"));
        let mut r = Registry::default();
        assert!(!r.any() && r.identities().is_empty());
        r.add("g1", a.clone());
        r.add("g2", a.clone());
        r.add("g3", b.clone());
        assert!(r.any());
        assert_eq!(r.identities(), [a.clone(), b.clone()]);
        r.remove("g1");
        assert_eq!(r.identities(), [a.clone(), b.clone()], "g2 still holds a");
        r.remove("nobody");
        r.remove("g1");
        assert_eq!(r.identities(), [a.clone(), b.clone()], "an unknown guid is harmless");
        r.remove("g2");
        assert_eq!(r.identities(), [b.clone()]);
        r.remove("g3");
        assert!(!r.any() && r.identities().is_empty());

        // The order is by first add; adding a guid again keeps one entry.
        r.add("g3", b.clone());
        r.add("g1", a.clone());
        r.add("g3", b.clone());
        assert_eq!(r.identities(), [b, a]);
    }

    #[test]
    fn changes_apply_in_order() {
        let mut r = Registry::default();
        let changes = vec![Change::Add("g1".into(), id("a")), Change::Add("g2".into(), id("b")), Change::Remove("g1".into())];
        assert!(r.apply(changes));
        assert_eq!(r.identities(), [id("b")]);
        assert!(!r.apply(Vec::new()), "nothing to apply changes nothing");
    }
}
