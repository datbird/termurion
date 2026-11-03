//! The current user's registry hive, through `url_schemes`' writer.

use crate::reg::{deletable, value_deletable, Registry};

/// `HKEY_CURRENT_USER`, through `url_schemes`' writer.
pub struct HkcuRegistry;

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
