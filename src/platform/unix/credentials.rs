//! No credential store on Unix yet (Keychain and Secret Service are later). Every call reports
//! Unsupported: a read never answers "nothing stored", so a key that exists elsewhere is not shadowed.

use super::super::Unsupported;
use crate::pwm::{Account, Secret};
use crate::pwm_source::{Caps, Kind, Scope, Source};

const FEATURE: &str = "Password manager";

fn no<T>() -> Result<T, String> {
    Err(Unsupported { feature: FEATURE }.into())
}

pub(crate) fn write_generic(_target: &str, _user: &str, _secret: &Secret, _comment: &str, _what: &str) -> Result<(), String> {
    no()
}

pub(crate) fn read_generic(_target: &str, _what: &str) -> Result<Option<Secret>, String> {
    no()
}

pub(crate) fn delete_generic(_target: &str) -> Result<bool, String> {
    no()
}

/// The Credential Manager source, here only a name that reports Unsupported for everything.
#[derive(Default)]
pub struct CredentialManager {
    _scope: Scope,
}

impl CredentialManager {
    pub fn new() -> CredentialManager {
        CredentialManager::for_scope(Scope::Terminal)
    }

    pub fn for_scope(scope: Scope) -> CredentialManager {
        CredentialManager { _scope: scope }
    }
}

impl Source for CredentialManager {
    fn kind(&self) -> Kind {
        Kind::CredentialManager
    }

    fn name(&self) -> String {
        "Credential Manager".to_string()
    }

    fn caps(&self) -> Caps {
        Caps::default()
    }

    fn available(&mut self) -> Result<(), String> {
        no()
    }

    fn accounts(&mut self) -> Result<Vec<Account>, String> {
        no()
    }

    fn password(&mut self, _a: &Account) -> Result<(Secret, Option<Secret>), String> {
        no()
    }

    fn set_password(&mut self, _a: &Account, _p: Secret) -> Result<(), String> {
        no()
    }

    fn delete(&mut self, _a: &Account) -> Result<(), String> {
        no()
    }

    fn add(&mut self, _name: &str, _user: &str, _p: Secret, _flags: &[(String, bool)]) -> Result<Account, String> {
        no()
    }

    fn edit_in_place(&mut self, _a: &Account, _name: Option<&str>, _user: Option<&str>, _p: Option<Secret>) -> Result<(), String> {
        no()
    }

    fn toggle_otp(&mut self, _a: &Account) -> Result<Account, String> {
        no()
    }

    fn reload(&mut self) {}

    fn reset_configuration(&mut self) {}

    fn switch_account(&mut self) -> Result<(), String> {
        no()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unix_credentials_unsupported() {
        let e = read_generic("TermurionAI/OpenAI", "API key").unwrap_err();
        assert!(e.contains("Password manager is not available"), "{e}");
        assert!(write_generic("TermurionAI/OpenAI", "u", &Secret::new("k".into()), "c", "API key").is_err());
        assert!(delete_generic("TermurionAI/OpenAI").is_err());
        assert!(CredentialManager::new().available().is_err());
    }
}
