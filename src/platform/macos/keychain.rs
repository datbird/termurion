//! The macOS Keychain as the password manager's store (iTerm2's `KeychainPasswordDataSource`) and
//! as the AI keys' store. Same behavior as `windows/credentials.rs`: every generic call checks
//! `guard` first, so nothing here touches another program's item. An item is a generic password
//! with service = target, account = user, label = target, comment = the caller's. One item per
//! target: a write replaces it. `accounts()` reads attributes only, never secret data. No secret
//! is ever logged.

use security_framework::base::Error;
use core_foundation::base::TCFType;
use security_framework::item::{ItemAddOptions, ItemAddValue, ItemClass, ItemSearchOptions, SearchResult};

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

/// What a write needs from the store, so the order of its steps can be tested without a Keychain.
trait Ops {
    fn delete(&mut self, target: &str) -> Result<bool, String>;
    fn add(&mut self, target: &str, user: &str, secret: &Secret, comment: &str) -> Result<(), String>;
}

struct Real;

impl Ops for Real {
    fn delete(&mut self, target: &str) -> Result<bool, String> {
        delete_item(target)
    }

    fn add(&mut self, target: &str, user: &str, secret: &Secret, comment: &str) -> Result<(), String> {
        let bytes = secret.expose().as_bytes();
        // SAFETY: kCFAllocatorNull never frees the bytes, and the CFData is dropped (at the end of
        // this function) before `secret`, so it borrows them without a second copy to wipe.
        let data = unsafe {
            let d = core_foundation_sys::data::CFDataCreateWithBytesNoCopy(
                std::ptr::null(),
                bytes.as_ptr(),
                bytes.len() as isize,
                core_foundation_sys::base::kCFAllocatorNull,
            );
            if d.is_null() {
                return Err("The Keychain could not take the secret.".to_string());
            }
            core_foundation::data::CFData::wrap_under_create_rule(d)
        };
        let mut add = ItemAddOptions::new(ItemAddValue::Data { class: ItemClass::generic_password(), data });
        add.set_service(target).set_account_name(user).set_label(target).set_comment(comment);
        add.add().map_err(text)
    }
}

/// The write: delete whatever holds the service, then add. An item another app planted under our
/// name (open access list) is never reused, so the new item gets Termurion's own access list. If the
/// delete fails the error is returned and the secret is not written.
fn write_with(ops: &mut dyn Ops, target: &str, user: &str, secret: &Secret, comment: &str) -> Result<(), String> {
    ops.delete(target)?;
    ops.add(target, user, secret, comment)
}

/// Writes (creates or replaces) the item for `target`. `what` is unused: the Keychain has no
/// size limit that needs the secret named.
pub(crate) fn write_generic(target: &str, user: &str, secret: &Secret, comment: &str, _what: &str) -> Result<(), String> {
    guard(target)?;
    write_with(&mut Real, target, user, secret, comment)
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
    delete_item(target)
}

/// Deletes every item under the service, whatever its account: one delete can leave others (a
/// delete without an account removes one match), so it repeats until none is left. Bounded.
fn delete_item(target: &str) -> Result<bool, String> {
    let mut any = false;
    for _ in 0..16 {
        match by_service(target).delete() {
            Ok(()) => any = true,
            Err(e) if is_missing(&e) => return Ok(any),
            Err(e) => return Err(text(e)),
        }
    }
    Err("The Keychain still holds an item for this entry.".to_string())
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

    /// Records the steps of a write.
    struct Fake {
        log: Vec<&'static str>,
        delete_fails: bool,
    }
    impl Ops for Fake {
        fn delete(&mut self, _t: &str) -> Result<bool, String> {
            self.log.push("delete");
            if self.delete_fails { Err("denied".into()) } else { Ok(true) }
        }
        fn add(&mut self, _t: &str, _u: &str, _s: &Secret, _c: &str) -> Result<(), String> {
            self.log.push("add");
            Ok(())
        }
    }

    #[test]
    fn a_write_deletes_first_and_never_writes_after_a_failed_delete() {
        let s = Secret::new("k".into());
        let mut f = Fake { log: vec![], delete_fails: false };
        write_with(&mut f, "TermurionAI/x", "u", &s, "c").unwrap();
        assert_eq!(f.log, ["delete", "add"]);
        let mut f = Fake { log: vec![], delete_fails: true };
        assert_eq!(write_with(&mut f, "TermurionAI/x", "u", &s, "c"), Err("denied".to_string()));
        assert_eq!(f.log, ["delete"], "no secret is written when the old item cannot be removed");
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

        // A pre-existing item under our name is replaced, not adopted: after a write there is one item.
        let mut count = ItemSearchOptions::new();
        count.class(ItemClass::generic_password()).service(&ai).load_attributes(true).limit(security_framework::item::Limit::All);
        assert_eq!(count.search().unwrap().len(), 1);

        // An item another app planted (open access, another account) goes too: one item remains, ours.
        let plant = format!("TermurionAI/p2-test-plant-{id}");
        let _plant_guard = Cleanup(vec![plant.clone()]);
        let st = std::process::Command::new("security")
            .args(["add-generic-password", "-A", "-s", &plant, "-a", "other", "-w", "x"])
            .status()
            .unwrap();
        assert!(st.success());
        write_generic(&plant, "p2user", &Secret::new("k3".into()), "p2 test", "API key").unwrap();
        let mut q = ItemSearchOptions::new();
        q.class(ItemClass::generic_password()).service(&plant).load_attributes(true).limit(security_framework::item::Limit::All);
        let found = q.search().unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].simplify_dict().unwrap().get("acct").map(String::as_str), Some("p2user"));
        assert_eq!(read_generic(&plant, "API key").unwrap().unwrap().expose(), "k3");

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
