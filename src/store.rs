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
}

#[derive(Serialize, Deserialize)]
struct FileFormat {
    default: String,
    profile: Vec<BTreeMap<String, Value>>,
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
        let profiles: Vec<Profile> = file.profile.into_iter().map(|values| Profile { values }).collect();
        if profiles.is_empty() {
            return Err(format!("{}: no profiles", path.display()));
        }
        let default_guid = if profiles.iter().any(|p| p.guid() == file.default) { file.default } else { profiles[0].guid() };
        Ok(Store { profiles, default_guid, path: path.to_path_buf() })
    }

    /// One "Default" profile, saved at `path`.
    pub fn fresh(path: &Path) -> Store {
        let p = Profile::new("Default");
        Store { default_guid: p.guid(), profiles: vec![p], path: path.to_path_buf() }
    }

    pub fn save(&self) -> Result<(), String> {
        let profile = self
            .profiles
            .iter()
            .map(|p| {
                p.values
                    .iter()
                    .filter(|(k, v)| {
                        k.as_str() == GUID || k.as_str() == NAME || schema::find(k).and_then(|s| s.default.as_ref()) != Some(*v)
                    })
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect()
            })
            .collect();
        let text = toml::to_string(&FileFormat { default: self.default_guid.clone(), profile })
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

    pub fn get(&self, guid: &str) -> Option<&Profile> {
        self.profiles.iter().find(|p| p.guid() == guid)
    }

    pub fn get_mut(&mut self, guid: &str) -> Option<&mut Profile> {
        self.profiles.iter_mut().find(|p| p.guid() == guid)
    }

    pub fn add(&mut self, profile: Profile) {
        self.profiles.push(profile);
    }

    /// Copies a profile under a new Guid and "<name> copy". Returns the new Guid.
    pub fn duplicate(&mut self, guid: &str) -> Option<String> {
        let mut copy = self.get(guid)?.clone();
        let new_guid = uuid::Uuid::new_v4().to_string().to_uppercase();
        copy.set(GUID, Value::String(new_guid.clone()));
        copy.set(NAME, Value::String(format!("{} copy", copy.name())));
        self.profiles.push(copy);
        Some(new_guid)
    }

    /// Deletes a profile. The last profile cannot be deleted. Deleting the default
    /// makes the first remaining profile the default.
    pub fn delete(&mut self, guid: &str) -> bool {
        if self.profiles.len() < 2 {
            return false;
        }
        let before = self.profiles.len();
        self.profiles.retain(|p| p.guid() != guid);
        if self.default_guid == guid {
            self.default_guid = self.profiles[0].guid();
        }
        self.profiles.len() < before
    }
}

#[cfg(test)]
mod tests {
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
    fn the_last_profile_cannot_be_deleted() {
        let mut s = Store::load(&scratch("delete")).unwrap();
        let first = s.default_guid.clone();
        assert!(!s.delete(&first));
        let second = s.duplicate(&first).unwrap();
        assert!(s.delete(&first));
        assert_eq!(s.default_guid, second);
        assert_eq!(s.profiles.len(), 1);
    }
}
