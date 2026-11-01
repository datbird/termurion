//! A small registry seam (m13v1 Task 2): a trait over per-user keys so tests use a fake and
//! never touch the real registry. Keys are relative to `HKEY_CURRENT_USER` (never `HKLM`); `""`
//! is a key's default value. The one Windows writer is `url_schemes`' (P9); `HkcuRegistry` wraps it.

use std::collections::BTreeMap;

pub trait Registry {
    fn get(&self, key: &str, name: &str) -> Option<String>;
    fn set(&mut self, key: &str, name: &str, value: &str) -> Result<(), String>;
    fn delete_tree(&mut self, key: &str) -> Result<(), String>;
    /// Removes one value; only inside Termurion's own keys, or Termurion's `RegisteredApplications` entry.
    fn delete_value(&mut self, key: &str, name: &str) -> Result<(), String>;
    /// Removes `Software\Classes\<scheme>` for a scheme Termurion lists; the caller has checked that
    /// its command is Termurion's.
    fn delete_scheme_class(&mut self, scheme: &str) -> Result<(), String>;
}

/// Whether a value may be deleted: inside a tree Termurion made, or its one `RegisteredApplications` entry.
pub fn value_deletable(key: &str, name: &str) -> bool {
    deletable(key) || (key.trim_matches('\\').eq_ignore_ascii_case("Software\\RegisteredApplications") && name.eq_ignore_ascii_case("Termurion"))
}

/// Whether `key` is a tree Termurion made and may delete: Termurion's own software key, its protocol
/// ProgIds and its "Open with" application key. Nothing else: not a scheme (`ssh`, `http`),
/// not `Console`, not an empty key.
pub fn deletable(key: &str) -> bool {
    let k = key.trim_matches('\\').to_ascii_lowercase();
    if k.split('\\').any(|part| part.is_empty() || part == "." || part == "..") {
        return false;
    }
    let under = |root: &str| k == root || k.starts_with(&format!("{root}\\"));
    under("software\\termurion") || under(&format!("software\\classes\\clsid\\{}", crate::default_term::CLSID_TERMURION_TERMINAL.to_ascii_lowercase())) || under("software\\classes\\applications\\termurion.exe") || k.strip_prefix("software\\classes\\termurion.").is_some_and(|rest| !rest.is_empty() && !rest.starts_with('\\'))
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
            return Err(format!("Termurion does not delete the registry key {key}"));
        }
        let k = key.trim_matches('\\').to_ascii_lowercase();
        self.values.retain(|(kk, _), _| {
            let kk = kk.to_ascii_lowercase();
            !(kk == k || kk.starts_with(&format!("{k}\\")))
        });
        Ok(())
    }

    fn delete_value(&mut self, key: &str, name: &str) -> Result<(), String> {
        if !value_deletable(key, name) {
            return Err(format!("Termurion does not delete the registry value {key}\\{name}"));
        }
        if let Some(slot) = self.slot(key, name) {
            self.values.remove(&slot);
        }
        Ok(())
    }

    fn delete_scheme_class(&mut self, scheme: &str) -> Result<(), String> {
        if !crate::url_schemes::is_listed(scheme) {
            return Err(format!("Termurion does not delete the registry key for {scheme}"));
        }
        let k = format!("software\\classes\\{scheme}");
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
            return Err(format!("Termurion does not delete the registry key {key}"));
        }
        crate::url_schemes::win::delete_tree(key);
        Ok(())
    }

    fn delete_value(&mut self, key: &str, name: &str) -> Result<(), String> {
        if !value_deletable(key, name) {
            return Err(format!("Termurion does not delete the registry value {key}\\{name}"));
        }
        crate::url_schemes::win::delete_value(key, (!name.is_empty()).then_some(name));
        Ok(())
    }

    fn delete_scheme_class(&mut self, scheme: &str) -> Result<(), String> {
        if !crate::url_schemes::is_listed(scheme) {
            return Err(format!("Termurion does not delete the registry key for {scheme}"));
        }
        crate::url_schemes::win::delete_tree(&format!("Software\\Classes\\{scheme}"));
        Ok(())
    }
}

/// There is no registry on Unix; every read finds nothing and every write says so.
#[cfg(unix)]
pub struct HkcuRegistry;

#[cfg(unix)]
impl Registry for HkcuRegistry {
    fn get(&self, _key: &str, _name: &str) -> Option<String> {
        None
    }

    fn set(&mut self, _key: &str, _name: &str, _value: &str) -> Result<(), String> {
        Err(crate::platform::Unsupported { feature: "The registry" }.to_string())
    }

    fn delete_tree(&mut self, _key: &str) -> Result<(), String> {
        Err(crate::platform::Unsupported { feature: "The registry" }.to_string())
    }

    fn delete_value(&mut self, _key: &str, _name: &str) -> Result<(), String> {
        Err(crate::platform::Unsupported { feature: "The registry" }.to_string())
    }

    fn delete_scheme_class(&mut self, _scheme: &str) -> Result<(), String> {
        Err(crate::platform::Unsupported { feature: "The registry" }.to_string())
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
        r.set("Software\\Classes\\Applications\\termurion.exe", "FriendlyAppName", "Termurion").unwrap();
        let entries = vec![
            entry("Software\\Classes\\Applications\\termurion.exe", "FriendlyAppName", "Termurion"),
            entry("Software\\Classes\\Applications\\termurion.exe\\SupportedTypes", ".ps1", ""),
            entry("Software\\Classes\\Applications\\termurion.exe\\shell\\open\\command", "", "\"C:\\t.exe\" \"%1\""),
        ];
        assert_eq!(apply(&mut r, &entries).unwrap(), 2);
        assert_eq!(apply(&mut r, &entries).unwrap(), 0);
        assert_eq!(r.get("SOFTWARE\\classes\\applications\\TERMURION.EXE", "FriendlyAppName").as_deref(), Some("Termurion"), "key compare is case-insensitive");
        assert_eq!(r.get("Software\\Classes\\Applications\\termurion.exe\\SupportedTypes", ".ps1").as_deref(), Some(""));
    }

    /// P9: a tree is deleted only under Termurion's own keys; the fake refuses as the real one does.
    #[test]
    fn registry_deletes_only_termurions_keys() {
        let mut r = FakeRegistry::default();
        for k in ["Software\\Classes\\ssh", "Software\\Classes\\http", "Console", "Software\\Termurion\\Capabilities", "Software\\Classes\\Applications\\termurion.exe\\shell"] {
            r.set(k, "x", "1").unwrap();
        }
        for bad in ["Software\\Classes\\ssh", "Software\\Classes\\http", "Console", "", "Software", "Software\\Classes", "Software\\Termurionx", "Software\\Classes\\Termurionx\\..\\ssh"] {
            assert!(r.delete_tree(bad).is_err(), "{bad:?}");
        }
        assert_eq!(r.values.len(), 5);
        r.delete_tree("Software\\Termurion").unwrap();
        r.delete_tree("Software\\Classes\\Applications\\termurion.exe").unwrap();
        assert_eq!(r.values.len(), 3);
    }
}
