//! Saves and loads profiles in one TOML file. Only values that differ from the
//! schema defaults are written, so the file stays small and readable.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use toml::Value;

use crate::profile::{GUID, NAME, Profile};
use crate::schema;

pub struct Store {
    pub profiles: Vec<Profile>,
    pub default_guid: String,
    path: PathBuf,
    /// Session-local profile copies (never saved).
    pub sessions: Vec<crate::session_profile::SessionProfile>,
    /// Guids of profiles that live in memory only (Bonjour hosts, 13t B1): never written to the
    /// file, the remote-prefs snapshot or Export All. Filled and emptied by `bonjour_ui` only.
    pub transient: std::collections::HashSet<String>,
}

#[derive(Serialize, Deserialize)]
struct FileFormat {
    default: String,
    profile: Vec<BTreeMap<String, Value>>,
}

/// Gives a new Guid to any profile with none, or with one an earlier profile already has.
/// Profiles are found by Guid, so a repeat would make two rows act as one.
pub fn ensure_unique_guids(profiles: &mut [Profile]) {
    let mut seen = std::collections::HashSet::new();
    for p in profiles.iter_mut() {
        if p.guid().is_empty() || !seen.insert(p.guid()) {
            p.renew_guid();
            seen.insert(p.guid());
        }
    }
}

pub fn default_path() -> PathBuf {
    let base = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    base.join("Tandem").join("profiles.toml")
}

impl Store {
    pub fn load(path: &Path) -> Result<Store, String> {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Store::fresh(path)),
            Err(e) => return Err(format!("{}: {e}", path.display())),
        };
        let file: FileFormat = toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut profiles: Vec<Profile> = file
            .profile
            .into_iter()
            .map(Profile::from_values)
            .collect();
        ensure_unique_guids(&mut profiles);
        if profiles.is_empty() {
            return Err(format!("{}: no profiles", path.display()));
        }
        let default_guid = if profiles.iter().any(|p| p.guid() == file.default) { file.default } else { profiles[0].guid() };
        Ok(Store { profiles, default_guid, path: path.to_path_buf(), sessions: Vec::new(), transient: Default::default() })
    }

    /// One "Default" profile, saved at `path`.
    pub fn fresh(path: &Path) -> Store {
        let p = Profile::new("Default");
        Store { default_guid: p.guid(), profiles: vec![p], path: path.to_path_buf(), sessions: Vec::new(), transient: Default::default() }
    }

    /// The profiles as the file holds them: values that differ from the defaults, plus Guid and
    /// Name.
    pub fn file_profiles(&self) -> Vec<BTreeMap<String, Value>> {
        self.profiles
            .iter()
            .filter(|p| !self.transient.contains(&p.guid()))
            .map(|p| {
                p.values
                    .iter()
                    .filter(|(k, v)| {
                        k.as_str() == GUID || k.as_str() == NAME || schema::find(k).and_then(|s| s.default.as_ref()) != Some(*v)
                    })
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect()
            })
            .collect()
    }

    /// The default Guid as the file holds it: a transient default is written as the first saved
    /// profile's Guid.
    pub fn saved_default_guid(&self) -> String {
        if !self.transient.contains(&self.default_guid) {
            return self.default_guid.clone();
        }
        self.profiles.iter().find(|p| !self.transient.contains(&p.guid())).map_or_else(|| self.default_guid.clone(), |p| p.guid())
    }

    /// Makes `guid` the default profile. A transient profile (a Bonjour host) cannot be it, and
    /// neither can an unknown Guid (13t B1).
    pub fn set_default(&mut self, guid: &str) -> bool {
        if self.transient.contains(guid) || !self.profiles.iter().any(|p| p.guid() == guid) {
            return false;
        }
        self.default_guid = guid.to_string();
        true
    }

    pub fn save(&self) -> Result<(), String> {
        let profile = self.file_profiles();
        let text = toml::to_string(&FileFormat { default: self.saved_default_guid(), profile })
            .map_err(|e| format!("saving profiles: {e}"))?;
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        // Write a new file, then rename it over the old one, so a crash never leaves half a file.
        let tmp = self.path.with_extension("toml.tmp");
        std::fs::write(&tmp, text).map_err(|e| format!("{}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, &self.path).map_err(|e| format!("{}: {e}", self.path.display()))
    }

    pub fn default_profile(&self) -> &Profile {
        self.get(&self.default_guid).unwrap_or(&self.profiles[0])
    }

    /// A saved profile or a session's private copy.
    pub fn get(&self, guid: &str) -> Option<&Profile> {
        self.profiles.iter().find(|p| p.guid() == guid).or_else(|| self.sessions.iter().map(|s| &s.profile).find(|p| p.guid() == guid))
    }

    pub fn get_mut(&mut self, guid: &str) -> Option<&mut Profile> {
        self.profiles.iter_mut().find(|p| p.guid() == guid)
    }

    /// The saved profile a Guid stands for: itself, or a session copy's original.
    pub fn shared_guid(&self, guid: &str) -> String {
        self.sessions.iter().find(|s| s.profile.guid() == guid).map_or_else(|| guid.to_string(), |s| s.original_guid())
    }

    /// The Guid of the private copy of `guid`'s profile: `guid` itself when it already is one,
    /// else a new copy of the saved profile (the default profile when it is gone).
    pub fn divorce(&mut self, guid: &str) -> String {
        if self.sessions.iter().any(|s| s.profile.guid() == guid) {
            return guid.to_string();
        }
        let shared = self.get(guid).cloned().unwrap_or_else(|| self.default_profile().clone());
        let copy = crate::session_profile::divorce(&shared, uuid::Uuid::new_v4().to_string().to_uppercase());
        let new_guid = copy.profile.guid();
        self.sessions.push(copy);
        new_guid
    }

    /// Whether `guid` is a session's private copy.
    pub fn is_session(&self, guid: &str) -> bool {
        self.sessions.iter().any(|s| s.profile.guid() == guid)
    }

    /// Sets or removes (None) values on the session copy `guid` and works out what it overrides.
    pub fn set_session(&mut self, guid: &str, values: &[(String, Option<Value>)]) {
        let Some(i) = self.sessions.iter().position(|s| s.profile.guid() == guid) else { return };
        let original = self.sessions[i].original_guid();
        let shared = self.profiles.iter().find(|p| p.guid() == original).cloned().unwrap_or_else(|| self.default_profile().clone());
        self.sessions[i].set(values, &shared);
    }

    /// After a saved profile changed: its session copies follow it where they did not
    /// override a key. Copies whose profile is gone keep their values.
    pub fn merge_sessions(&mut self) {
        for s in self.sessions.iter_mut() {
            if let Some(shared) = self.profiles.iter().find(|p| p.guid() == s.original_guid()) {
                s.merge(shared);
            }
        }
    }

    pub fn add(&mut self, profile: Profile) {
        self.profiles.push(profile);
    }

    /// The profile named "tmux", made from the default one the first time (iTerm2's
    /// ProfileModel tmuxProfile: no hotkey, 1000 lines of scrollback). Returns its Guid and
    /// whether it was made now.
    pub fn tmux_profile(&mut self) -> (String, bool) {
        if let Some(p) = self.profiles.iter().find(|p| p.name() == "tmux") {
            return (p.guid(), false);
        }
        let mut p = self.default_profile().clone();
        let guid = uuid::Uuid::new_v4().to_string().to_uppercase();
        p.set("Has Hotkey", Value::Boolean(false));
        p.set(NAME, Value::String("tmux".into()));
        p.set(GUID, Value::String(guid.clone()));
        p.set("Scrollback Lines", Value::Integer(1000));
        self.profiles.push(p);
        (guid, true)
    }

    /// Copies a profile under a new Guid and "<name> copy". Returns the new Guid.
    pub fn duplicate(&mut self, guid: &str) -> Option<String> {
        let mut copy = self.get(guid)?.clone();
        // A copy of a transient profile is an ordinary saved profile (13t B1).
        copy.values.remove("Bonjour Service Address");
        if self.transient.contains(guid) {
            let tags: Vec<Value> = copy.get("Tags").and_then(|t| t.as_array().cloned()).map(|a| a.into_iter().filter(|t| t.as_str() != Some("bonjour")).collect()).unwrap_or_default();
            copy.set("Tags", Value::Array(tags));
        }
        let new_guid = uuid::Uuid::new_v4().to_string().to_uppercase();
        copy.set(GUID, Value::String(new_guid.clone()));
        copy.set(NAME, Value::String(format!("{} copy", copy.name())));
        self.profiles.push(copy);
        Some(new_guid)
    }

    /// Deletes a profile. The last profile cannot be deleted. Deleting the default
    /// makes the first remaining profile the default.
    pub fn delete(&mut self, guid: &str) -> bool {
        // The file needs one saved profile at least, so transient ones do not count.
        let saved = self.profiles.iter().filter(|p| !self.transient.contains(&p.guid())).count();
        if saved < 2 && !self.transient.contains(guid) {
            return false;
        }
        // Remove exactly one profile. Guids are unique after load, but never delete more than one.
        let Some(i) = self.profiles.iter().position(|p| p.guid() == guid) else { return false };
        self.profiles.remove(i);
        self.transient.remove(guid);
        if self.default_guid == guid {
            self.default_guid = self.profiles.iter().find(|p| !self.transient.contains(&p.guid())).unwrap_or(&self.profiles[0]).guid();
        }
        true
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn session_guid_maps_to_its_saved_profile() {
        let path = std::env::temp_dir().join(format!("tandem-store-map-{}.toml", std::process::id()));
        let mut st = Store::fresh(&path);
        let shared = st.profiles[0].guid();
        st.sessions.push(crate::session_profile::divorce(&st.profiles[0].clone(), "copy".into()));
        assert_eq!(st.shared_guid("copy"), shared, "new panes and Edit Session use the saved profile");
        assert_eq!(st.shared_guid(&shared), shared);
        assert_eq!(st.shared_guid("unknown"), "unknown");
    }

    #[test]
    fn sessions_are_not_saved() {
        let path = std::env::temp_dir().join(format!("tandem-store-{}.toml", std::process::id()));
        let mut st = Store::fresh(&path);
        let shared = st.profiles[0].clone();
        let copy = crate::session_profile::divorce(&shared, "session-guid".into());
        st.sessions.push(copy);
        assert_eq!(st.get("session-guid").map(|p| p.guid()), Some("session-guid".to_string()), "get finds session copies");
        st.save().unwrap();
        let back = Store::load(&path).unwrap();
        assert!(back.get("session-guid").is_none());
        assert_eq!(back.profiles.len(), 1);
        let _ = std::fs::remove_file(&path);
    }

    use super::*;
    use toml::Value;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tandem-store-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("profiles.toml")
    }

    #[test]
    fn a_missing_file_gives_one_default_profile() {
        let s = Store::load(&scratch("missing")).unwrap();
        assert_eq!(s.profiles.len(), 1);
        assert_eq!(s.default_profile().name(), "Default");
        assert_eq!(s.default_guid, s.profiles[0].guid());
    }

    #[test]
    fn save_and_load_round_trip() {
        let path = scratch("roundtrip");
        let mut s = Store::load(&path).unwrap();
        let guid = s.default_guid.clone();
        s.get_mut(&guid).unwrap().set("Columns", Value::Integer(132));
        s.save().unwrap();
        let t = Store::load(&path).unwrap();
        assert_eq!(t.get(&guid).unwrap().int("Columns"), 132);
        assert_eq!(t.default_guid, guid);
    }

    #[test]
    fn tmux_profile_is_made_once() {
        let dir = std::env::temp_dir().join(format!("tandem-tmuxprof-{}", std::process::id()));
        let mut s = Store::fresh(&dir.join("profiles.toml"));
        let (guid, made) = s.tmux_profile();
        assert!(made);
        let p = s.get(&guid).unwrap().clone();
        assert_eq!(p.name(), "tmux");
        assert_eq!(p.int("Scrollback Lines"), 1000);
        assert!(!p.bool("Has Hotkey"));
        assert_ne!(guid, s.default_profile().guid(), "a new profile, not the default");
        assert_eq!(s.tmux_profile(), (guid, false), "made once");
        // A profile the person named tmux is the one used.
        let mut s = Store::fresh(&dir.join("profiles.toml"));
        let mine = s.duplicate(&s.default_profile().guid()).unwrap();
        s.get_mut(&mine).unwrap().set(NAME, Value::String("tmux".into()));
        assert_eq!(s.tmux_profile(), (mine, false));
    }

    #[test]
    fn saved_files_omit_defaults() {
        let path = scratch("omit");
        let mut s = Store::load(&path).unwrap();
        let guid = s.default_guid.clone();
        s.get_mut(&guid).unwrap().set("Rows", Value::Integer(25));
        s.save().unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("Rows"), "{text}");
        assert!(text.contains("Guid"), "{text}");
    }

    #[test]
    fn a_corrupt_file_is_an_error_not_a_reset() {
        let path = scratch("corrupt");
        std::fs::write(&path, "this is [not toml").unwrap();
        let err = Store::load(&path).err().unwrap();
        assert!(err.contains("profiles.toml"), "{err}");
    }

    #[test]
    fn duplicate_and_missing_guids_are_made_unique_on_load() {
        // A hand-copied [[profile]] block repeats a Guid; a hand-written one may have none.
        let path = scratch("dupes");
        std::fs::write(&path, "default = \"A\"\n[[profile]]\nGuid = \"A\"\nName = \"one\"\n[[profile]]\nGuid = \"A\"\nName = \"two\"\n[[profile]]\nName = \"three\"\n").unwrap();
        let mut s = Store::load(&path).unwrap();
        let ids: std::collections::HashSet<String> = s.profiles.iter().map(|p| p.guid()).collect();
        assert_eq!(ids.len(), 3);
        assert!(!ids.contains(""));
        assert_eq!(s.default_profile().name(), "one");
        let second = s.profiles[1].guid();
        assert!(s.delete(&second));
        assert_eq!(s.profiles.len(), 2);
    }

    #[test]
    fn the_last_profile_cannot_be_deleted() {
        let mut s = Store::load(&scratch("delete")).unwrap();
        let first = s.default_guid.clone();
        assert!(!s.delete(&first));
        let second = s.duplicate(&first).unwrap();
        assert!(s.delete(&first));
        assert_eq!(s.default_guid, second);
        assert_eq!(s.profiles.len(), 1);
    }
    fn with_bonjour(name: &str) -> (Store, String) {
        let mut s = Store::fresh(&scratch(name));
        let mut p = Profile::new("host");
        p.set("Bonjour Service Address", Value::String("10.0.0.5".into()));
        p.set("Tags", Value::Array(vec![Value::String("bonjour".into())]));
        let guid = p.guid();
        s.add(p);
        s.transient.insert(guid.clone());
        (s, guid)
    }

    #[test]
    fn transient_profiles_never_reach_file_profiles() {
        let (mut s, guid) = with_bonjour("transient");
        assert_eq!(s.file_profiles().len(), 1, "only the saved profile");
        s.default_guid = guid.clone();
        s.save().unwrap();
        let back = Store::load(&s.path).unwrap();
        assert_eq!(back.profiles.len(), 1);
        assert_eq!(back.default_guid, back.profiles[0].guid(), "a transient default is written as a saved guid");
        assert_ne!(s.saved_default_guid(), guid);
    }

    #[test]
    fn imported_bonjour_tagged_profile_is_saved() {
        let mut s = Store::fresh(&scratch("imported"));
        let mut p = Profile::new("imported");
        p.set("Bonjour Service Address", Value::String("10.0.0.5".into()));
        p.set("Tags", Value::Array(vec![Value::String("bonjour".into())]));
        s.add(p);
        assert_eq!(s.file_profiles().len(), 2, "tag and key alone do not make a profile transient");
    }

    #[test]
    fn export_all_omits_transient_profiles() {
        let (s, _) = with_bonjour("export");
        let snap = crate::remote_prefs::Snapshot::from_local(&Default::default(), &s.saved_default_guid(), &s.file_profiles());
        assert_eq!(snap.profiles.len(), 1);
        assert!(snap.profiles.iter().all(|p| !p.contains_key("Bonjour Service Address")));
    }

    #[test]
    fn set_as_default_refuses_a_transient_profile() {
        let (mut s, guid) = with_bonjour("default");
        let before = s.default_guid.clone();
        assert!(!s.set_default(&guid));
        assert_eq!(s.default_guid, before);
        let saved = s.profiles[0].guid();
        assert!(s.set_default(&saved));
    }

    #[test]
    fn duplicate_of_a_transient_profile_is_saved_without_the_key() {
        let (mut s, guid) = with_bonjour("dup");
        let copy = s.duplicate(&guid).unwrap();
        assert!(!s.transient.contains(&copy));
        let p = s.get(&copy).unwrap();
        assert!(p.get("Bonjour Service Address").is_none());
        assert!(p.get("Tags").is_none_or(|t| t.as_array().is_some_and(|a| a.is_empty())));
        assert_eq!(s.file_profiles().len(), 2);
    }

    #[test]
    fn deleting_counts_only_saved_profiles() {
        let (mut s, guid) = with_bonjour("delete");
        let only = s.profiles[0].guid();
        assert!(!s.delete(&only), "the last saved profile stays although a transient one exists");
        assert!(s.delete(&guid));
        assert!(s.transient.is_empty());
    }
}
