//! A pane's host stack (m14e Task 4; iTerm2 `PTYSession.m` `pushOrPopHostState`, `_hostStack`):
//! when a session moves to another host the keyboard mode (and the title) the previous host left
//! is saved, and coming back to a host on the stack gives them back. Pure.
//!
//! Host reports come from the remote (OSC 1337 RemoteHost, OSC 7), so a hostile remote may send
//! many different hosts: the stack is bounded (`MAX_DEPTH`; the second-oldest entry goes, so the
//! first host's is kept) and so is each saved title (`MAX_TITLE`).

use crate::keyproto::KeyMode;

/// The most entries one pane's stack keeps.
pub const MAX_DEPTH: usize = 32;
/// The longest title an entry keeps, in characters (the remote writes titles).
pub const MAX_TITLE: usize = 256;

/// `title` cut to `MAX_TITLE` characters.
pub fn capped_title(title: &str) -> String {
    title.chars().take(MAX_TITLE).collect()
}

pub const RESTORE_TITLES: &str = "Automatically restore the tab and window title when an ssh session ends?";
pub const ONLY_THIS_TIME: &str = "_Only This Time";
pub const ALWAYS: &str = "Always";
pub const NEVER: &str = "Never";
pub const RESTORE_ID: &str = "RestoreIconAndWindowName";
pub const RESTORE_KEY: &str = "NoSyncRestoreIconAndWindowNameOnHostChange";

/// A reported account. `local` is decided when the host is reported and kept (R14de-8, iTerm2's
/// frozen `localityState`); it never feeds an open or run decision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Remote {
    pub user: Option<String>,
    pub host: String,
    pub local: bool,
}

impl Remote {
    /// `VT100RemoteHost.m:89-100`: two local hosts are the same when their users are (a local
    /// name drifts; `su` makes another user); otherwise host and user must both match.
    pub fn same_host(&self, other: &Remote) -> bool {
        if self.local && other.local {
            return self.user == other.user;
        }
        self.host == other.host && self.user == other.user
    }
}

/// What was left behind when the session moved on.
#[derive(Clone, Debug, PartialEq)]
pub struct Saved {
    pub remote: Remote,
    pub mode: KeyMode,
    pub title: Option<String>,
}

/// What coming back gives back: a `Some` field means "set this".
#[derive(Clone, Debug, PartialEq)]
pub struct Restore {
    pub mode: Option<KeyMode>,
    pub title: Option<Option<String>>,
}

#[derive(Clone, Debug, Default)]
pub struct Stack {
    saved: Vec<Saved>,
}

impl Stack {
    /// `pushOrPopHostState` (`PS:18007-18033`): `new` is the host just reported, `previous` the one
    /// left, `mode` and `title` what the session has now. Not on the stack: push the previous
    /// host's state and give nothing. On it: pop it and everything above, and give back what
    /// differs (kitty key flags are not kept, iTerm2's TODO).
    pub fn push_or_pop(&mut self, new: &Remote, previous: &Remote, mode: KeyMode, title: Option<&str>) -> Option<Restore> {
        let Some(i) = self.saved.iter().position(|s| s.remote.same_host(new)) else {
            // The bottom entry (the first host, the local one) is never the one dropped.
            if self.saved.len() == MAX_DEPTH {
                self.saved.remove(1);
            }
            self.saved.push(Saved { remote: previous.clone(), mode, title: title.map(capped_title) });
            return None;
        };
        let state = self.saved[i].clone();
        self.saved.truncate(i);
        // Names compare with none as empty (`isEqual:` on `?: @""`).
        let same_title = title.unwrap_or("") == state.title.as_deref().unwrap_or("");
        Some(Restore { mode: (mode != state.mode).then_some(state.mode), title: (!same_title).then_some(state.title) })
    }

    pub fn len(&self) -> usize {
        self.saved.len()
    }

    pub fn is_empty(&self) -> bool {
        self.saved.is_empty()
    }
}

/// `PS:17926-17943`: a host report changes the stack only when a previous host existed, the host
/// name changed (not a user-only change), the change did not come from the ssh integration layer,
/// the two are not both local (iTerm2 skips `su` and a drifted local name), and the setting is on.
/// The caller passes `both_local` as `previous.local && current.local`.
pub fn should_track(previous: Option<&Remote>, current: &Remote, both_local: bool, via_ssh_integration: bool, enabled: bool) -> bool {
    previous.is_some_and(|p| p.host != current.host) && !both_local && !via_ssh_integration && enabled
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TitleChoice {
    Restore,
    Offer,
    Nothing,
}

/// `PSA:244-254`: needs Allow Title Setting; the remembered answer true restores, false does
/// nothing, unset offers.
pub fn title_choice(allow_title_setting: bool, remembered: Option<bool>) -> TitleChoice {
    match (allow_title_setting, remembered) {
        (false, _) | (true, Some(false)) => TitleChoice::Nothing,
        (true, Some(true)) => TitleChoice::Restore,
        (true, None) => TitleChoice::Offer,
    }
}

/// What an answer to the title offer does (`NC:619-690`): the remembered answer to store, whether
/// this pane's title comes back, and what happens to the other panes waiting on the same offer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Answer {
    pub remember: Option<bool>,
    pub restore_this: bool,
    pub others: Others,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Others {
    Leave,
    Restore,
    Drop,
}

/// Only This Time restores; Always remembers true, restores and restores the waiting panes;
/// Never remembers false and drops their offers; closing drops only this offer.
pub fn answer(button: Option<&str>) -> Answer {
    match button {
        Some(ONLY_THIS_TIME) => Answer { remember: None, restore_this: true, others: Others::Leave },
        Some(ALWAYS) => Answer { remember: Some(true), restore_this: true, others: Others::Restore },
        Some(NEVER) => Answer { remember: Some(false), restore_this: false, others: Others::Drop },
        _ => Answer { remember: None, restore_this: false, others: Others::Leave },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local(user: &str, host: &str) -> Remote {
        Remote { user: Some(user.into()), host: host.into(), local: true }
    }
    fn remote(user: &str, host: &str) -> Remote {
        Remote { user: Some(user.into()), host: host.into(), local: false }
    }

    #[test]
    fn host_stack_follows_iterm2() {
        let (l, r, s) = (local("me", "pc"), remote("u", "box"), remote("u", "other"));
        let mut st = Stack::default();
        assert_eq!(st.push_or_pop(&r, &l, KeyMode::Standard, Some("local")), None);
        assert_eq!(st.len(), 1);
        assert_eq!(st.saved[0], Saved { remote: l.clone(), mode: KeyMode::Standard, title: Some("local".into()) });
        let back = st.push_or_pop(&l, &r, KeyMode::CsiU, Some("vim"));
        assert_eq!(back, Some(Restore { mode: Some(KeyMode::Standard), title: Some(Some("local".into())) }));
        assert_eq!(st.len(), 0);
        // Nothing differs: found, popped, nothing to set.
        st.push_or_pop(&r, &l, KeyMode::Standard, Some("t"));
        assert_eq!(st.push_or_pop(&l, &r, KeyMode::Standard, Some("t")), Some(Restore { mode: None, title: None }));
        assert!(st.is_empty());
        // L, R, S, back to L pops everything above L (and L).
        st.push_or_pop(&r, &l, KeyMode::Standard, None);
        st.push_or_pop(&s, &r, KeyMode::Mok1, None);
        assert_eq!(st.len(), 2);
        let back = st.push_or_pop(&l, &s, KeyMode::Raw, None).unwrap();
        assert_eq!(back.mode, Some(KeyMode::Standard));
        assert!(st.is_empty());
        // Back to R from S pops to R's entry; L stays.
        st.push_or_pop(&r, &l, KeyMode::Standard, None);
        st.push_or_pop(&s, &r, KeyMode::CsiU, None);
        let back = st.push_or_pop(&r, &s, KeyMode::Raw, None).unwrap();
        assert_eq!(back.mode, Some(KeyMode::CsiU));
        assert_eq!(st.len(), 1);
        assert_eq!(st.saved[0].remote, l);
        // Remotes compare by user and host.
        assert!(r.same_host(&remote("u", "box")));
        assert!(!r.same_host(&remote("v", "box")));
        assert!(!r.same_host(&remote("u", "elsewhere")));
        let mut st = Stack::default();
        st.push_or_pop(&r, &l, KeyMode::Standard, None);
        assert_eq!(st.push_or_pop(&remote("v", "box"), &r, KeyMode::Standard, None), None, "another user is another host");
        assert_eq!(st.len(), 2);
    }

    #[test]
    fn two_local_hosts_compare_by_user_only() {
        let l = local("me", "pc");
        assert!(l.same_host(&local("me", "pc.lan")), "a drifted local name");
        assert!(!l.same_host(&local("root", "pc")), "su");
        assert!(l.same_host(&remote("me", "pc")), "not both local: host and user decide");
        assert!(!l.same_host(&remote("me", "pc.lan").clone()), "not both local: a drifted name is another host");
        assert!(remote("me", "pc").same_host(&remote("me", "pc")));
        // me@pc, root@pc, back to me@pc: one entry pushed, then popped.
        let mut st = Stack::default();
        assert_eq!(st.push_or_pop(&local("root", "pc"), &l, KeyMode::Standard, None), None);
        assert_eq!(st.len(), 1);
        assert!(st.push_or_pop(&l, &local("root", "pc"), KeyMode::Standard, None).is_some());
        assert!(st.is_empty());
        // The stack is not told apart by a drifted local name.
        st.push_or_pop(&remote("u", "box"), &l, KeyMode::CsiU, None);
        assert!(st.push_or_pop(&local("me", "pc.lan"), &remote("u", "box"), KeyMode::Standard, None).is_some());
    }

    #[test]
    fn should_track_follows_iterm2() {
        let (l, r) = (local("me", "pc"), remote("u", "box"));
        assert!(should_track(Some(&l), &r, false, false, true));
        assert!(!should_track(None, &r, false, false, true), "the first report");
        assert!(!should_track(Some(&r), &remote("root", "box"), false, false, true), "a user-only change");
        assert!(!should_track(Some(&l), &local("root", "pc"), true, false, true), "both local (su): iTerm2 skips it");
        assert!(!should_track(Some(&l), &r, false, true, true), "from the ssh integration layer");
        assert!(!should_track(Some(&l), &r, false, false, false), "the setting is off");
    }

    #[test]
    fn a_hostile_remote_cannot_grow_the_stack() {
        let l = local("me", "pc");
        let mut st = Stack::default();
        let mut prev = l.clone();
        for i in 0..1000 {
            let next = remote("u", &format!("h{i}"));
            st.push_or_pop(&next, &prev, KeyMode::Standard, Some("t"));
            prev = next;
        }
        assert_eq!(st.len(), MAX_DEPTH);
        // The newest entries are the ones kept.
        assert_eq!(st.saved.last().unwrap().remote.host, "h998");
        assert_eq!(st.saved[0].remote, l, "the first (local) entry is never dropped");
        assert!(st.push_or_pop(&remote("u", "h990"), &prev, KeyMode::Standard, None).is_some());
        assert!(st.len() < MAX_DEPTH);
        assert!(st.push_or_pop(&l, &remote("u", "h990"), KeyMode::Standard, None).is_some(), "back home after the flood");
        assert_eq!(st.len(), 0);
    }

    #[test]
    fn a_saved_title_is_capped() {
        let (l, r) = (local("me", "pc"), remote("u", "box"));
        let mut st = Stack::default();
        let long = "\u{e9}".repeat(1000);
        st.push_or_pop(&r, &l, KeyMode::Standard, Some(&long));
        let back = st.push_or_pop(&l, &r, KeyMode::Standard, None).unwrap();
        let title = back.title.unwrap().unwrap();
        assert_eq!(title.chars().count(), MAX_TITLE);
        assert!(title.chars().all(|c| c == '\u{e9}'), "cut on a character boundary");
        assert_eq!(capped_title("short"), "short");
    }

    #[test]
    fn title_offer_answers_follow_iterm2() {
        let a = |b| answer(b);
        assert_eq!(a(Some(ONLY_THIS_TIME)), Answer { remember: None, restore_this: true, others: Others::Leave });
        assert_eq!(a(Some(ALWAYS)), Answer { remember: Some(true), restore_this: true, others: Others::Restore });
        assert_eq!(a(Some(NEVER)), Answer { remember: Some(false), restore_this: false, others: Others::Drop });
        assert_eq!(a(None), Answer { remember: None, restore_this: false, others: Others::Leave }, "closed");
    }

    #[test]
    fn title_offer_follows_iterm2() {
        for remembered in [None, Some(true), Some(false)] {
            assert_eq!(title_choice(false, remembered), TitleChoice::Nothing, "Allow Title Setting off");
        }
        assert_eq!(title_choice(true, Some(true)), TitleChoice::Restore);
        assert_eq!(title_choice(true, Some(false)), TitleChoice::Nothing);
        assert_eq!(title_choice(true, None), TitleChoice::Offer);
        assert_eq!(RESTORE_TITLES, "Automatically restore the tab and window title when an ssh session ends?");
        assert_eq!((ONLY_THIS_TIME, ALWAYS, NEVER), ("_Only This Time", "Always", "Never"));
        assert_eq!(RESTORE_ID, "RestoreIconAndWindowName");
        assert_eq!(RESTORE_KEY, "NoSyncRestoreIconAndWindowNameOnHostChange");
    }
}
