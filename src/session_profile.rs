//! iTerm2's session-local ("divorced") profiles: when a program changes a profile setting
//! for one session (SetProfileProperty, SetBackgroundImageFile), that session gets a private
//! copy of its profile with a fresh Guid. Later edits to the shared profile still reach
//! the copy, except for the keys the session changed.

use std::collections::BTreeSet;

use toml::Value;

use crate::profile::{GUID, Profile};

/// The key that names the shared profile a session copy came from.
pub const ORIGINAL_GUID: &str = "Original Guid";

#[derive(Clone, Debug)]
pub struct SessionProfile {
    pub profile: Profile,
    /// Keys whose value differs from the shared profile's because the session set them.
    pub overridden: BTreeSet<String>,
}

/// A private copy of `shared` for one session.
pub fn divorce(shared: &Profile, new_guid: String) -> SessionProfile {
    let mut profile = shared.clone();
    profile.set(ORIGINAL_GUID, Value::String(shared.guid()));
    profile.set(GUID, Value::String(new_guid));
    SessionProfile { profile, overridden: BTreeSet::new() }
}

/// Keys that belong to the copy itself, never compared with the shared profile.
fn own(key: &str) -> bool {
    key == GUID || key == ORIGINAL_GUID
}

impl SessionProfile {
    pub fn original_guid(&self) -> String {
        self.profile.get(ORIGINAL_GUID).and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default()
    }

    /// Sets (Some) or removes (None) values, then works out which keys are overridden.
    pub fn set(&mut self, values: &[(String, Option<Value>)], shared: &Profile) {
        for (k, v) in values {
            if own(k) {
                continue;
            }
            match v {
                Some(v) => self.profile.set(k, v.clone()),
                None => {
                    self.profile.values.remove(k);
                }
            }
            if self.profile.values.get(k) != shared.values.get(k) {
                self.overridden.insert(k.clone());
            } else {
                self.overridden.remove(k);
            }
        }
    }

    /// The shared profile changed: keys the session did not override follow it.
    pub fn merge(&mut self, shared: &Profile) {
        for (k, v) in &shared.values {
            if !own(k) && !self.overridden.contains(k) {
                self.profile.values.insert(k.clone(), v.clone());
            }
        }
        let gone: Vec<String> = self.profile.values.keys().filter(|k| !own(k) && !self.overridden.contains(*k) && !shared.values.contains_key(*k)).cloned().collect();
        for k in gone {
            self.profile.values.remove(&k);
        }
        let values = &self.profile.values;
        self.overridden.retain(|k| values.get(k) != shared.values.get(k));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shared() -> Profile {
        let mut p = Profile::stock();
        p.set(GUID, Value::String("shared-guid".into()));
        p.set("Blend", Value::Float(0.5));
        p.set("Cursor Type", Value::Integer(2));
        p
    }

    #[test]
    fn divorce_copies_with_fresh_guid() {
        let s = divorce(&shared(), "new-guid".into());
        assert_eq!(s.profile.guid(), "new-guid");
        assert_eq!(s.original_guid(), "shared-guid");
        assert_eq!(s.profile.float("Blend"), 0.5);
        assert!(s.overridden.is_empty());
    }

    #[test]
    fn overrides_survive_shared_edits() {
        let mut base = shared();
        let mut s = divorce(&base, "g".into());
        s.set(&[("Blend".into(), Some(Value::Float(0.9)))], &base);
        assert!(s.overridden.contains("Blend"));
        base.set("Blend", Value::Float(0.2));
        base.set("Cursor Type", Value::Integer(1));
        s.merge(&base);
        assert_eq!(s.profile.float("Blend"), 0.9, "the session's value stays");
        assert_eq!(s.profile.int("Cursor Type"), 1, "the rest follows the shared edit");
        base.set("Blend", Value::Float(0.9));
        s.merge(&base);
        assert!(!s.overridden.contains("Blend"), "equal again: no longer an override");
        base.set("Blend", Value::Float(0.3));
        s.merge(&base);
        assert_eq!(s.profile.float("Blend"), 0.3);
        assert_eq!(s.profile.guid(), "g", "the Guids stay the session's");
        assert_eq!(s.original_guid(), "shared-guid");
        // A key the shared profile drops goes from the session too.
        base.values.remove("Cursor Type");
        s.merge(&base);
        assert!(!s.profile.values.contains_key("Cursor Type"));
    }

    #[test]
    fn none_removes_a_key() {
        let base = shared();
        let mut s = divorce(&base, "g".into());
        s.set(&[("Cursor Type".into(), None)], &base);
        assert!(!s.profile.values.contains_key("Cursor Type"));
        assert!(s.overridden.contains("Cursor Type"), "differs from the shared 2 (default is used)");
    }
}
