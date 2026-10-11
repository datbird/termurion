//! Bonjour hosts as profiles on the app side (m13t Task 9, `ITAddressBookMgr.m:342-353`). The
//! browser runs while `EnableRendezvous` is on; each host it finds becomes a "bonjour"-tagged
//! profile in memory only (`Store::transient`), and a lost host or the key turning off removes it.
//! What the network advertises is untrusted: `bonjour::profile_for` vets every word, at most
//! `MAX_HOSTS` profiles exist, and nothing connects until the person opens one.

use super::*;
use crate::bonjour::{self, Event, Live};
use crate::store::Store;

/// Applies one event to the store. Returns whether the profile list changed.
pub(super) fn apply_event(store: &mut Store, live: &mut Vec<Live>, event: Event, ssh_path: &str) -> bool {
    match event {
        Event::Found(found) => {
            if !bonjour::accept(live, &found) {
                return false;
            }
            let prototype = store.default_profile().clone();
            let Some(mut profile) = bonjour::profile_for(&found, &prototype, ssh_path) else { return false };
            let name = bonjour::unique_name(&profile.name(), &found.address, &|n| store.profiles.iter().any(|p| p.name() == n));
            profile.set("Name", toml::Value::String(name));
            let guid = profile.guid();
            store.add(profile);
            store.transient.insert(guid.clone());
            live.push(Live { guid, name: found.name, service: found.service, address: found.address });
            true
        }
        Event::Lost(lost) => {
            let gone = bonjour::lost_guids(live, &lost);
            for guid in &gone {
                store.delete(guid);
            }
            live.retain(|l| !gone.contains(&l.guid));
            !gone.is_empty()
        }
    }
}

/// Drops the live entries whose profile is gone (deleted in Settings or by the API:
/// `Store::delete` takes the guid out of `transient`), so the slot frees and the host can be
/// added again. Costs nothing unless a delete happened.
pub(super) fn prune(store: &Store, live: &mut Vec<Live>) {
    if live.len() != store.transient.len() {
        live.retain(|l| store.transient.contains(&l.guid));
    }
}

/// The profiles were replaced wholesale (a remote-prefs load): transient guids that name no
/// profile go, and the live list follows.
pub(super) fn after_profiles_replaced(store: &mut Store, live: &mut Vec<Live>) {
    let present: std::collections::HashSet<String> = store.profiles.iter().map(|p| p.guid()).collect();
    store.transient.retain(|g| present.contains(g));
    live.retain(|l| store.transient.contains(&l.guid));
}

/// Removes every Bonjour profile (`removeBonjourProfiles`): the key went off.
pub(super) fn remove_all(store: &mut Store, live: &mut Vec<Live>) -> bool {
    let before = store.profiles.len();
    let transient = std::mem::take(&mut store.transient);
    store.profiles.retain(|p| !transient.contains(&p.guid()));
    live.clear();
    if transient.contains(&store.default_guid) {
        store.default_guid = store.profiles[0].guid();
    }
    store.profiles.len() != before
}

/// `guids` without the Bonjour profiles, when more than one opens at once (Open All and a
/// multiple selection): a rogue host on the LAN cannot fan out sessions (P5).
pub(super) fn without_bonjour(store: &Store, guids: Vec<String>) -> Vec<String> {
    if guids.len() < 2 {
        return guids;
    }
    guids.into_iter().filter(|g| !store.get(g).is_some_and(bonjour::is_bonjour)).collect()
}

/// The command `sshSchemePath` names, when a Bonjour profile may run it: iTerm2's rule, a
/// program found on disk, and nothing a command line would split or a batch file would reread.
fn usable_ssh_path(path: &str) -> bool {
    crate::ssh_url::scheme_path_usable(path)
        && !path.contains(char::is_whitespace)
        && !crate::semantic::batch_unsafe(path)
        && crate::semantic::resolve_on_path(std::path::Path::new(path), std::env::var_os("PATH").as_deref()).is_some()
}

impl App {
    /// Starts or stops the browser to match `EnableRendezvous` (called when settings change and
    /// once at the start).
    pub(super) fn bonjour_sync(&mut self) {
        let want = self.globals.bool("EnableRendezvous");
        if self.bonjour_wanted == Some(want) {
            return;
        }
        self.bonjour_wanted = Some(want);
        if want {
            let notify = self.notify.clone();
            match crate::bonjour::Browser::start(notify) {
                Ok(b) => self.bonjour = Some(b),
                Err(e) => self.banners.set("bonjour", format!("Bonjour: {e}")),
            }
        } else {
            self.bonjour = None;
            self.remove_bonjour_profiles();
        }
    }

    /// The browser's events become profiles. The callbacks wake the loop through `Notify`, so
    /// this costs nothing while nothing is found.
    pub(super) fn bonjour_tick(&mut self) {
        self.bonjour_sync();
        prune(&self.store, &mut self.bonjour_live);
        let mut changed = false;
        while let Some(event) = self.bonjour.as_ref().and_then(|b| b.try_event()) {
            let ssh = self.globals.string("SshSchemePath");
            let ssh = if usable_ssh_path(&ssh) { ssh } else { "ssh".to_string() };
            changed |= apply_event(&mut self.store, &mut self.bonjour_live, event, &ssh);
        }
        if changed {
            self.menus_dirty = true;
            self.redraw();
        }
    }

    /// A remote-prefs load replaced every profile: the stale transient and live entries go and
    /// the browser starts again (if still on), so the hosts are found again.
    pub(super) fn bonjour_profiles_replaced(&mut self) {
        after_profiles_replaced(&mut self.store, &mut self.bonjour_live);
        self.bonjour = None;
        self.bonjour_wanted = None;
    }

    /// Removes every Bonjour profile (the key turned off, or the browser stopped).
    pub(super) fn remove_bonjour_profiles(&mut self) {
        if remove_all(&mut self.store, &mut self.bonjour_live) {
            self.menus_dirty = true;
            self.redraw();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bonjour::Found;

    fn found(name: &str, address: &str) -> Event {
        Event::Found(Found { name: name.into(), service: "ssh".into(), address: address.into(), port: 22, user: Some("bob".into()) })
    }

    fn lost(name: &str, service: &str, address: &str) -> Event {
        Event::Lost(bonjour::Lost { name: name.into(), service: service.into(), address: address.into() })
    }

    fn store() -> Store {
        Store::fresh(&std::env::temp_dir().join(format!("tandem-bonjour-{}.toml", std::process::id())))
    }

    #[test]
    fn bonjour_profiles_come_and_go() {
        let mut s = store();
        let mut live = Vec::new();
        assert!(apply_event(&mut s, &mut live, found("Mac", "10.0.0.5"), "ssh"));
        assert_eq!(s.profiles.len(), 2);
        let p = &s.profiles[1];
        assert_eq!(p.name(), "Mac");
        assert!(s.transient.contains(&p.guid()));
        assert_eq!(p.string("Command"), "ssh -l bob 10.0.0.5");
        // The same host again changes nothing; a name clash gets its address.
        assert!(!apply_event(&mut s, &mut live, found("Mac", "10.0.0.5"), "ssh"));
        assert!(apply_event(&mut s, &mut live, found("Mac", "10.0.0.6"), "ssh"));
        assert_eq!(s.profiles[2].name(), "Mac (10.0.0.6)");
        // A service that makes no profile, and an odd name, are dropped.
        let ftp = Event::Found(Found { name: "NAS".into(), service: "ftp".into(), address: "10.0.0.7".into(), port: 21, user: None });
        assert!(!apply_event(&mut s, &mut live, ftp, "ssh"));
        assert!(!apply_event(&mut s, &mut live, found("\u{202e}\n", "10.0.0.8"), "ssh"));
        assert_eq!(s.profiles.len(), 3);
        // The saved file never holds them.
        s.save().unwrap();
        let back = Store::load(&std::env::temp_dir().join(format!("tandem-bonjour-{}.toml", std::process::id()))).unwrap();
        assert_eq!(back.profiles.len(), 1);
        // An ftp goodbye of the same name, and an ssh goodbye from another address, keep the
        // ssh profile; the host's own goodbye removes the profile it made.
        assert!(!apply_event(&mut s, &mut live, lost("Mac", "ftp", "10.0.0.5"), "ssh"));
        assert!(!apply_event(&mut s, &mut live, lost("Mac", "ssh", "10.0.0.9"), "ssh"));
        assert_eq!(s.profiles.len(), 3);
        assert!(apply_event(&mut s, &mut live, lost("Mac", "ssh", "10.0.0.5"), "ssh"));
        assert_eq!(s.profiles.iter().map(|p| p.name()).collect::<Vec<_>>(), vec!["Default", "Mac (10.0.0.6)"]);
        assert!(apply_event(&mut s, &mut live, lost("Mac", "ssh", "10.0.0.6"), "ssh"));
        assert_eq!(s.profiles.len(), 1);
        assert!(s.transient.is_empty() && live.is_empty());
        assert!(!apply_event(&mut s, &mut live, lost("Mac", "ssh", "10.0.0.5"), "ssh"));
        // Turning the key off removes every Bonjour profile.
        apply_event(&mut s, &mut live, found("A", "10.0.1.1"), "ssh");
        apply_event(&mut s, &mut live, found("B", "10.0.1.2"), "ssh");
        s.add(crate::profile::Profile::new("kept"));
        assert!(remove_all(&mut s, &mut live));
        assert_eq!(s.profiles.iter().map(|p| p.name()).collect::<Vec<_>>(), vec!["Default", "kept"]);
        assert!(s.transient.is_empty() && live.is_empty());
        let _ = std::fs::remove_file(std::env::temp_dir().join(format!("tandem-bonjour-{}.toml", std::process::id())));
    }

    #[test]
    fn bonjour_hosts_are_bounded() {
        let mut s = store();
        let mut live = Vec::new();
        for i in 0..bonjour::MAX_HOSTS + 50 {
            apply_event(&mut s, &mut live, found(&format!("h{i}"), &format!("10.1.{}.{}", i / 250, i % 250 + 1)), "ssh");
        }
        assert_eq!(live.len(), bonjour::MAX_HOSTS);
        assert_eq!(s.transient.len(), bonjour::MAX_HOSTS);
        // A lost host frees a slot.
        assert!(apply_event(&mut s, &mut live, lost("h0", "ssh", "10.1.0.1"), "ssh"));
        assert!(apply_event(&mut s, &mut live, found("late", "10.9.9.9"), "ssh"));
    }

    #[test]
    fn a_deleted_or_replaced_bonjour_profile_frees_its_slot() {
        let mut s = store();
        let mut live = Vec::new();
        apply_event(&mut s, &mut live, found("Mac", "10.0.0.5"), "ssh");
        // Deleted in Settings or by the API: the live entry goes, and the host can come back.
        let guid = live[0].guid.clone();
        assert!(s.delete(&guid));
        prune(&s, &mut live);
        assert!(live.is_empty());
        assert!(apply_event(&mut s, &mut live, found("Mac", "10.0.0.5"), "ssh"));
        // A remote-prefs load replaces every profile: nothing transient or live is left.
        s.profiles = vec![crate::profile::Profile::new("Remote")];
        after_profiles_replaced(&mut s, &mut live);
        assert!(s.transient.is_empty() && live.is_empty());
        assert!(apply_event(&mut s, &mut live, found("Mac", "10.0.0.5"), "ssh"));
    }

    #[test]
    fn open_all_skips_bonjour_profiles() {
        let mut s = store();
        let mut live = Vec::new();
        apply_event(&mut s, &mut live, found("Mac", "10.0.0.5"), "ssh");
        let all: Vec<String> = s.profiles.iter().map(|p| p.guid()).collect();
        assert_eq!(without_bonjour(&s, all.clone()), vec![all[0].clone()]);
        assert_eq!(without_bonjour(&s, vec![all[1].clone()]), vec![all[1].clone()], "a single pick is the person's choice");
    }

    #[test]
    fn bonjour_sweeps_leave_dynamic_profiles_alone() {
        let mut s = store();
        let mut live = Vec::new();
        let mut d = crate::profile::Profile::new("dyn");
        d.set("Is Dynamic Profile", toml::Value::Boolean(true));
        let guid = d.guid();
        s.add(d);
        apply_event(&mut s, &mut live, found("A", "10.0.1.1"), "ssh");
        assert!(!s.transient.contains(&guid));
        assert!(remove_all(&mut s, &mut live));
        assert!(s.get(&guid).is_some(), "Remove Bonjour profiles keeps the dynamic one");
        after_profiles_replaced(&mut s, &mut live);
        assert!(s.get(&guid).is_some());
    }
}
