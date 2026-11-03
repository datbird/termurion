//! The macOS Keychain as the password manager's store (iTerm2's `KeychainPasswordDataSource`) and
//! as the AI keys' store. Same behavior as `windows/credentials.rs`: every generic call checks
//! `guard` first, so nothing here touches another program's item. An item is a generic password
//! with service = target, account = user, label = target, comment = the caller's. One item per
//! target: a write replaces it. `accounts()` reads attributes only, never secret data. No secret
//! is ever logged.

use security_framework::base::Error;
use security_framework::item::{ItemAddOptions, ItemAddValue, ItemClass, ItemSearchOptions, ItemUpdateOptions, ItemUpdateValue, SearchResult};

use crate::platform::credentials::guard;
use crate::pwm::{Account, Secret};
use crate::pwm_source::{self, Caps, Kind, Scope, Source};

/// The comment written on every item the password manager stores.
const COMMENT: &str = "Termurion password manager";

/// `ERR_SEC_ITEM_NOT_FOUND`.
const ERR_SEC_ITEM_NOT_FOUND: i32 = -25300;

fn text(e: Error) -> String {
    e.message().unwrap_or_else(|| format!("Keychain error {}.", e.code()))
}

fn not_found() -> String {
    text(Error::from_code(ERR_SEC_ITEM_NOT_FOUND))
}

fn is_missing(e: &Error) -> bool {
    e.code() == ERR_SEC_ITEM_NOT_FOUND
}

/// A search for the generic password items of one service.
fn by_service(target: &str) -> ItemSearchOptions {
    let mut q = ItemSearchOptions::new();
    q.class(ItemClass::generic_password()).service(target);
    q
}

/// Writes (creates or replaces) the item for `target`. `what` is unused: the Keychain has no
/// size limit that needs the secret named.
pub(crate) fn write_generic(target: &str, user: &str, secret: &Secret, comment: &str, _what: &str) -> Result<(), String> {
    guard(target)?;
    let data = core_foundation::data::CFData::from_buffer(secret.expose().as_bytes());
    // Update in place first (keeps the item and its access list), else add.
    let mut up = ItemUpdateOptions::new();
    up.set_value(ItemUpdateValue::Data(data.clone())).set_account_name(user).set_label(target).set_comment(comment);
    match security_framework::item::update_item(&by_service(target), &up) {
        Ok(()) => Ok(()),
        Err(e) if is_missing(&e) => {
            let mut add = ItemAddOptions::new(ItemAddValue::Data { class: ItemClass::generic_password(), data });
            add.set_service(target).set_account_name(user).set_label(target).set_comment(comment);
            add.add().map_err(text)
        }
        Err(e) => Err(text(e)),
    }
}

/// Wipes a buffer that held a secret.
fn wipe(buf: &mut [u8]) {
    for b in buf.iter_mut() {
        // SAFETY: a plain byte write into our own buffer; volatile so it is not optimized away.
        unsafe { std::ptr::write_volatile(b, 0) };
    }
    std::sync::atomic::compiler_fence(std::sync::atomic::Ordering::SeqCst);
}

/// Reads the item's secret as UTF-8 text; `Ok(None)` when there is no such item. `what` names the
/// secret in the not-UTF-8 text ("password", "API key").
pub(crate) fn read_generic(target: &str, what: &str) -> Result<Option<Secret>, String> {
    guard(target)?;
    let mut q = by_service(target);
    q.load_data(true).limit(1);
    let mut found = match q.search() {
        Ok(v) => v,
        Err(e) if is_missing(&e) => return Ok(None),
        Err(e) => return Err(text(e)),
    };
    let Some(first) = found.pop() else { return Ok(None) };
    let SearchResult::Data(bytes) = first else { return Err(format!("The stored {what} could not be read.")) };
    // The Secret takes the buffer (from_utf8 does not copy); a buffer that is not UTF-8 is wiped.
    match String::from_utf8(bytes) {
        Ok(s) => Ok(Some(Secret::new(s))),
        Err(e) => {
            let mut raw = e.into_bytes();
            wipe(&mut raw);
            Err(format!("The stored {what} is not UTF-8 text."))
        }
    }
}

/// Deletes the item: `Ok(true)` when one was deleted, `Ok(false)` when there was none.
pub(crate) fn delete_generic(target: &str) -> Result<bool, String> {
    guard(target)?;
    match by_service(target).delete() {
        Ok(()) => Ok(true),
        Err(e) if is_missing(&e) => Ok(false),
        Err(e) => Err(text(e)),
    }
}

/// The accounts a list of service names holds for one scope; anything else is skipped.
pub fn keep_scope(scope: Scope, services: &[&str]) -> Vec<Account> {
    services.iter().filter_map(|s| pwm_source::parse_target_in(scope, s)).collect()
}

/// The account's target, refused unless it is one of the scope's.
fn target(scope: Scope, a: &Account) -> Result<&str, String> {
    match pwm_source::parse_target_in(scope, &a.id) {
        Some(_) => Ok(&a.id),
        None => Err("Not a Termurion credential.".to_string()),
    }
}

fn write(target: &str, user: &str, p: &Secret) -> Result<(), String> {
    write_generic(target, user, p, COMMENT, "password")
}

/// The Keychain, always available, one account, no one-time codes.
#[derive(Default)]
pub struct CredentialManager {
    scope: Scope,
}

impl CredentialManager {
    pub fn new() -> CredentialManager {
        CredentialManager::for_scope(Scope::Terminal)
    }

    /// The Keychain source of one password manager scope (`Termurion/` or `Termurion-Browser/` items).
    pub fn for_scope(scope: Scope) -> CredentialManager {
        CredentialManager { scope }
    }
}

impl Source for CredentialManager {
    fn kind(&self) -> Kind {
        Kind::CredentialManager
    }

    fn name(&self) -> String {
        "Keychain".to_string()
    }

    fn caps(&self) -> Caps {
        pwm_source::credman_caps()
    }

    fn available(&mut self) -> Result<(), String> {
        Ok(())
    }

    fn accounts(&mut self) -> Result<Vec<Account>, String> {
        // Attributes only: no load_data, so no secret is fetched and no item asks for access.
        let mut q = ItemSearchOptions::new();
        q.class(ItemClass::generic_password()).load_attributes(true).limit(security_framework::item::Limit::All);
        let found = match q.search() {
            Ok(v) => v,
            Err(e) if is_missing(&e) => return Ok(Vec::new()),
            Err(e) => return Err(text(e)),
        };
        let services: Vec<String> = found.iter().filter_map(|r| r.simplify_dict()).filter_map(|mut d| d.remove("svce")).collect();
        let refs: Vec<&str> = services.iter().map(String::as_str).collect();
        Ok(keep_scope(self.scope, &refs))
    }

    fn password(&mut self, a: &Account) -> Result<(Secret, Option<Secret>), String> {
        match read_generic(target(self.scope, a)?, "password")? {
            Some(p) => Ok((p, None)),
            None => Err(not_found()),
        }
    }

    fn set_password(&mut self, a: &Account, p: Secret) -> Result<(), String> {
        write(target(self.scope, a)?, &a.user_name, &p)
    }

    fn delete(&mut self, a: &Account) -> Result<(), String> {
        if delete_generic(target(self.scope, a)?)? {
            Ok(())
        } else {
            Err(not_found())
        }
    }

    fn add(&mut self, name: &str, user: &str, p: Secret, _flags: &[(String, bool)]) -> Result<Account, String> {
        // The Keychain has no Add Account toggles, so flags is ignored (as iTerm2's keychain).
        let t = pwm_source::target_name_in(self.scope, name, user);
        write(&t, user, &p)?;
        pwm_source::parse_target_in(self.scope, &t).ok_or_else(|| "Not a Termurion credential.".to_string())
    }

    fn edit_in_place(&mut self, _a: &Account, _name: Option<&str>, _user: Option<&str>, _p: Option<Secret>) -> Result<(), String> {
        // Never called: caps().in_place is false, so the window deletes and re-adds.
        Err("In-place edit is not supported.".to_string())
    }

    fn toggle_otp(&mut self, _a: &Account) -> Result<Account, String> {
        // Never called: no account has a one-time code (iTerm2's keychain traps here).
        Err("One-time codes are not supported.".to_string())
    }

    fn reload(&mut self) {}

    fn reset_configuration(&mut self) {}

    fn switch_account(&mut self) -> Result<(), String> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pwm_source::target_name_in;

    #[test]
    fn the_scope_keeps_its_own_items() {
        let t = target_name_in(Scope::Terminal, "a", "u");
        let b = target_name_in(Scope::Browser, "b", "v");
        let all = [t.as_str(), b.as_str(), "TermurionAI/OpenAI", "iTerm2"];
        let term = keep_scope(Scope::Terminal, &all);
        assert_eq!(term.len(), 1);
        assert_eq!(term[0].account_name, "a");
        let br = keep_scope(Scope::Browser, &all);
        assert_eq!(br.len(), 1);
        assert_eq!(br[0].account_name, "b");
    }

    #[test]
    fn foreign_targets_are_refused_before_the_keychain() {
        let refused = "Not a Termurion credential.".to_string();
        assert_eq!(write_generic("iTerm2", "u", &Secret::new("k".into()), "c", "password"), Err(refused.clone()));
        assert_eq!(read_generic("iTerm2", "password").err(), Some(refused.clone()));
        assert_eq!(delete_generic("iTerm2"), Err(refused));
    }

    /// Deletes its items at the end, even when an assert fails.
    struct Cleanup(Vec<String>);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            for t in &self.0 {
                let _ = delete_generic(t);
            }
        }
    }

    #[test]
    #[ignore = "uses the real login Keychain"]
    fn keychain_round_trip_live() {
        let id = format!("{:x}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos());
        let ai = format!("TermurionAI/p2-test-{id}");
        let added = target_name_in(Scope::Terminal, "p2acct", "u");
        let _guard = Cleanup(vec![ai.clone(), added.clone()]);

        write_generic(&ai, "p2user", &Secret::new("k1".into()), "p2 test", "API key").unwrap();
        assert_eq!(read_generic(&ai, "API key").unwrap().unwrap().expose(), "k1");
        write_generic(&ai, "p2user", &Secret::new("k2".into()), "p2 test", "API key").unwrap();
        assert_eq!(read_generic(&ai, "API key").unwrap().unwrap().expose(), "k2");

        let mut cm = CredentialManager::for_scope(Scope::Terminal);
        assert!(cm.accounts().unwrap().iter().all(|a| a.id != ai), "an AI key is not a password manager account");

        let acct = cm.add("p2acct", "u", Secret::new("pw".into()), &[]).unwrap();
        assert!(cm.accounts().unwrap().iter().any(|a| a.account_name == "p2acct"));
        assert_eq!(cm.password(&acct).unwrap().0.expose(), "pw");

        assert_eq!(delete_generic(&ai), Ok(true));
        cm.delete(&acct).unwrap();
        assert!(read_generic(&ai, "API key").unwrap().is_none());
        assert!(read_generic(&added, "password").unwrap().is_none());
        assert_eq!(delete_generic(&ai), Ok(false));
    }
}
