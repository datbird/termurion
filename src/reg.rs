//! A small registry seam (m13v1 Task 2): a trait over per-user keys so tests use a fake and
//! never touch the real registry. Keys are relative to `HKEY_CURRENT_USER` (never `HKLM`); `""`
//! is a key's default value. The one Windows writer is `url_schemes`' (P9); `HkcuRegistry` wraps it.

use std::collections::BTreeMap;

pub trait Registry {
    fn get(&self, key: &str, name: &str) -> Option<String>;
    fn set(&mut self, key: &str, name: &str, value: &str) -> Result<(), String>;
    fn delete_tree(&mut self, key: &str) -> Result<(), String>;
}

/// Whether `key` is a tree Tandem made and may delete: Tandem's own software key, its protocol
/// ProgIds and its "Open with" application key. Nothing else: not a scheme (`ssh`, `http`),
/// not `Console`, not an empty key.
pub fn deletable(key: &str) -> bool {
    let k = key.trim_matches('\\').to_ascii_lowercase();
    if k.split('\\').any(|part| part.is_empty() || part == "." || part == "..") {
        return false;
    }
    let under = |root: &str| k == root || k.starts_with(&format!("{root}\\"));
    under("software\\tandem") || under("software\\classes\\applications\\tandem.exe") || k.strip_prefix("software\\classes\\tandem.").is_some_and(|rest| !rest.is_empty() && !rest.starts_with('\\'))
}

/// Writes only the entries that differ from what is there; returns how many were written.
pub fn apply(r: &mut dyn Registry, entries: &[(String, String, String)]) -> Result<usize, String> {
    let mut written = 0;
    for (key, name, value) in entries {
        if r.get(key, name).as_deref() != Some(value.as_str()) {
            r.set(key, name, value)?;
            written += 1;
        }
    }
    Ok(written)
}

/// A registry in memory; key and value names compare case-insensitively, as the registry's do.
#[derive(Debug, Default)]
pub struct FakeRegistry {
    pub values: BTreeMap<(String, String), String>,
}

impl FakeRegistry {
    fn slot(&self, key: &str, name: &str) -> Option<(String, String)> {
        self.values.keys().find(|(k, n)| k.eq_ignore_ascii_case(key) && n.eq_ignore_ascii_case(name)).cloned()
    }
}

impl Registry for FakeRegistry {
    fn get(&self, key: &str, name: &str) -> Option<String> {
        self.slot(key, name).and_then(|s| self.values.get(&s).cloned())
    }

    fn set(&mut self, key: &str, name: &str, value: &str) -> Result<(), String> {
        let slot = self.slot(key, name).unwrap_or((key.to_string(), name.to_string()));
        self.values.insert(slot, value.to_string());
        Ok(())
    }

    fn delete_tree(&mut self, key: &str) -> Result<(), String> {
        if !deletable(key) {
            return Err(format!("Tandem does not delete the registry key {key}"));
        }
        let k = key.trim_matches('\\').to_ascii_lowercase();
        self.values.retain(|(kk, _), _| {
            let kk = kk.to_ascii_lowercase();
            !(kk == k || kk.starts_with(&format!("{k}\\")))
        });
        Ok(())
    }
}

/// `HKEY_CURRENT_USER`, through `url_schemes`' writer.
#[cfg(windows)]
pub struct HkcuRegistry;

#[cfg(windows)]
impl Registry for HkcuRegistry {
    fn get(&self, key: &str, name: &str) -> Option<String> {
        crate::url_schemes::win::get(key, (!name.is_empty()).then_some(name))
    }

    fn set(&mut self, key: &str, name: &str, value: &str) -> Result<(), String> {
        crate::url_schemes::win::set(key, (!name.is_empty()).then_some(name), value)
    }

    fn delete_tree(&mut self, key: &str) -> Result<(), String> {
        if !deletable(key) {
            return Err(format!("Tandem does not delete the registry key {key}"));
        }
        crate::url_schemes::win::delete_tree(key);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(k: &str, n: &str, v: &str) -> (String, String, String) {
        (k.to_string(), n.to_string(), v.to_string())
    }

    #[test]
    fn registry_apply_writes_only_changes() {
        let mut r = FakeRegistry::default();
        r.set("Software\\Classes\\Applications\\tandem.exe", "FriendlyAppName", "Tandem").unwrap();
        let entries = vec![
            entry("Software\\Classes\\Applications\\tandem.exe", "FriendlyAppName", "Tandem"),
            entry("Software\\Classes\\Applications\\tandem.exe\\SupportedTypes", ".ps1", ""),
            entry("Software\\Classes\\Applications\\tandem.exe\\shell\\open\\command", "", "\"C:\\t.exe\" \"%1\""),
        ];
        assert_eq!(apply(&mut r, &entries).unwrap(), 2);
        assert_eq!(apply(&mut r, &entries).unwrap(), 0);
        assert_eq!(r.get("SOFTWARE\\classes\\applications\\TANDEM.EXE", "FriendlyAppName").as_deref(), Some("Tandem"), "key compare is case-insensitive");
        assert_eq!(r.get("Software\\Classes\\Applications\\tandem.exe\\SupportedTypes", ".ps1").as_deref(), Some(""));
    }

    /// P9: a tree is deleted only under Tandem's own keys; the fake refuses as the real one does.
    #[test]
    fn registry_deletes_only_tandems_keys() {
        let mut r = FakeRegistry::default();
        for k in ["Software\\Classes\\ssh", "Software\\Classes\\http", "Console", "Software\\Tandem\\Capabilities", "Software\\Classes\\Applications\\tandem.exe\\shell"] {
            r.set(k, "x", "1").unwrap();
        }
        for bad in ["Software\\Classes\\ssh", "Software\\Classes\\http", "Console", "", "Software", "Software\\Classes", "Software\\Tandemx", "Software\\Classes\\Tandemx\\..\\ssh"] {
            assert!(r.delete_tree(bad).is_err(), "{bad:?}");
        }
        assert_eq!(r.values.len(), 5);
        r.delete_tree("Software\\Tandem").unwrap();
        r.delete_tree("Software\\Classes\\Applications\\tandem.exe").unwrap();
        assert_eq!(r.values.len(), 3);
    }
}
