//! There is no registry on Unix.

use crate::reg::Registry;

/// There is no registry on Unix; every read finds nothing and every write says so.
pub struct HkcuRegistry;

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
