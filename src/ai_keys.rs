//! AI API keys, one per vendor, kept in Windows Credential Manager under `TandemAI/<provider>`
//! (ruling 3), a prefix the password manager neither enumerates nor parses. Ports iTerm2's key
//! cache (`AITermControllerObjC.swift:100-339`): a vendor's key is read once and cached for the
//! session, a hard read error is not cached, a key is trimmed before it is stored, an empty key
//! deletes, and the cache is updated before the store is written. Tandem never had iTerm2's
//! shared legacy account, so there is no adoption step. Pure but for `CredStore`; every test
//! uses a fake store and fake keys.

use crate::ai_settings::Vendor;
use crate::pwm::Secret;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// The Credential Manager target prefix of every AI key.
pub const TARGET_PREFIX: &str = "TandemAI/";
/// The comment written on every AI key credential (shown in Control Panel).
pub const COMMENT: &str = "Tandem AI API key";

/// The credential's target: `TandemAI/` and the provider name.
pub fn target(v: Vendor) -> String {
    format!("{TARGET_PREFIX}{}", v.provider_name())
}

/// The credential's user name, iTerm2's keychain account with "Tandem" for "iTerm2"
/// (`KC:299-318`; Llama keeps iTerm2's "Llama").
pub fn account(v: Vendor) -> String {
    let name = match v {
        Vendor::Llama => "Llama",
        _ => v.provider_name(),
    };
    format!("{name} API Key for Tandem")
}

/// The key as stored: trimmed of whitespace and line ends, None when nothing is left
/// (iTerm2's `keyIsEmpty` and `setAPIKeyOnQueue`).
pub fn normalized(key: &str) -> Option<&str> {
    let t = key.trim();
    (!t.is_empty()).then_some(t)
}

/// Whether a key looks like the vendor's (`KC:322-339`).
pub fn key_matches(key: &str, v: Vendor) -> bool {
    let t = key.trim();
    match v {
        Vendor::Anthropic => t.starts_with("sk-ant-"),
        Vendor::Gemini => t.starts_with("AIza"),
        Vendor::OpenAI | Vendor::DeepSeek => !t.starts_with("sk-ant-") && !t.starts_with("AIza"),
        Vendor::Llama | Vendor::Apple => true,
    }
}

/// Where keys live. `read` answers `Ok(None)` for a key that is not stored and `Err` only for a
/// genuine failure; `delete` of a key that is not stored succeeds. No error text ever holds a key.
pub trait KeyStore {
    fn read(&mut self, target: &str) -> Result<Option<Secret>, String>;
    fn write(&mut self, target: &str, user: &str, key: &Secret) -> Result<(), String>;
    fn delete(&mut self, target: &str) -> Result<(), String>;
}

/// The session's key cache, keyed by the vendor's raw value. A vendor with no entry has not
/// been read yet (or its last read failed); `Some(None)` is a known absence.
#[derive(Default)]
pub struct Keys {
    cache: HashMap<i64, Option<Secret>>,
}

impl Keys {
    /// The vendor's key, read from the store on first use and cached; None when there is no
    /// key or the read failed (a failure is retried on the next call).
    pub fn get(&mut self, v: Vendor, store: &mut dyn KeyStore) -> Option<Secret> {
        if let Some(cached) = self.cache.get(&v.raw()) {
            return cached.as_ref().map(Secret::duplicate);
        }
        // A hard error is not cached: the next call retries (`KC:197-203`). A stored key that
        // is only whitespace is no key (`keyIsEmpty`), and that absence is cached.
        let stored = store.read(&target(v)).ok()?.filter(|k| normalized(k.expose()).is_some());
        let out = stored.as_ref().map(Secret::duplicate);
        self.cache.insert(v.raw(), stored);
        out
    }

    /// Whether the vendor has a key (through the same cache as `get`).
    pub fn is_set(&mut self, v: Vendor, store: &mut dyn KeyStore) -> bool {
        self.get(v, store).is_some()
    }

    /// Stores the trimmed key, or deletes it when only whitespace is left. The cache takes the
    /// new value first, so a failed write still answers the new key for the session
    /// (`KC:257-265`); the store's error is returned.
    pub fn set(&mut self, v: Vendor, key: &Secret, store: &mut dyn KeyStore) -> Result<(), String> {
        let value = normalized(key.expose()).map(|k| Secret::new(k.to_string()));
        let t = target(v);
        match &value {
            Some(k) => {
                self.cache.insert(v.raw(), Some(k.duplicate()));
                store.write(&t, &account(v), k)
            }
            None => {
                self.cache.insert(v.raw(), None);
                store.delete(&t)
            }
        }
    }
}

/// The app's one key cache (12b's providers and the settings window share it).
pub fn shared() -> &'static Mutex<Keys> {
    static SHARED: OnceLock<Mutex<Keys>> = OnceLock::new();
    SHARED.get_or_init(|| Mutex::new(Keys::default()))
}

/// The system's key store: Credential Manager on Windows.
#[cfg(windows)]
pub fn system_store() -> Box<dyn KeyStore> {
    Box::new(CredStore)
}

/// The system's key store: none off Windows (reads find nothing, writes are refused).
#[cfg(not(windows))]
pub fn system_store() -> Box<dyn KeyStore> {
    Box::new(NoStore)
}

/// Off Windows there is no key store: nothing is stored and nothing can be.
#[cfg(not(windows))]
struct NoStore;

#[cfg(not(windows))]
impl KeyStore for NoStore {
    fn read(&mut self, _target: &str) -> Result<Option<Secret>, String> {
        Ok(None)
    }

    fn write(&mut self, _target: &str, _user: &str, _key: &Secret) -> Result<(), String> {
        Err("Credential Manager is only on Windows.".to_string())
    }

    fn delete(&mut self, _target: &str) -> Result<(), String> {
        Err("Credential Manager is only on Windows.".to_string())
    }
}

/// Windows Credential Manager, through `pwm_credman`'s generic-target functions (which refuse
/// any target that is not Tandem's).
#[cfg(windows)]
pub struct CredStore;

#[cfg(windows)]
impl KeyStore for CredStore {
    fn read(&mut self, target: &str) -> Result<Option<Secret>, String> {
        crate::pwm_credman::read_generic(target, "API key")
    }

    fn write(&mut self, target: &str, user: &str, key: &Secret) -> Result<(), String> {
        crate::pwm_credman::write_generic(target, user, key, COMMENT, "API key")
    }

    /// Clearing a key that is already gone is fine, so the bool is dropped.
    fn delete(&mut self, target: &str) -> Result<(), String> {
        crate::pwm_credman::delete_generic(target).map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pwm_source;
    use std::collections::VecDeque;

    /// A map of target to key, a log of (op, target, user) that never holds key text, and
    /// scripted answers that win over the map.
    #[derive(Default)]
    struct FakeStore {
        map: HashMap<String, Secret>,
        log: Vec<(&'static str, String, String)>,
        read_answers: VecDeque<Result<Option<Secret>, String>>,
        write_error: Option<String>,
    }

    impl KeyStore for FakeStore {
        fn read(&mut self, target: &str) -> Result<Option<Secret>, String> {
            self.log.push(("read", target.to_string(), String::new()));
            if let Some(a) = self.read_answers.pop_front() {
                return a;
            }
            Ok(self.map.get(target).map(Secret::duplicate))
        }

        fn write(&mut self, target: &str, user: &str, key: &Secret) -> Result<(), String> {
            self.log.push(("write", target.to_string(), user.to_string()));
            if let Some(e) = &self.write_error {
                return Err(e.clone());
            }
            self.map.insert(target.to_string(), key.duplicate());
            Ok(())
        }

        fn delete(&mut self, target: &str) -> Result<(), String> {
            self.log.push(("delete", target.to_string(), String::new()));
            self.map.remove(target);
            Ok(())
        }
    }

    fn reads(s: &FakeStore) -> usize {
        s.log.iter().filter(|(op, _, _)| *op == "read").count()
    }

    /// The key text compared without ever printing it.
    fn is(k: &Option<Secret>, want: &str) -> bool {
        k.as_ref().is_some_and(|s| s.expose() == want)
    }

    #[test]
    fn key_targets_stay_out_of_the_password_manager() {
        let want = [
            (Vendor::OpenAI, "TandemAI/OpenAI"),
            (Vendor::Anthropic, "TandemAI/Anthropic"),
            (Vendor::Gemini, "TandemAI/Gemini"),
            (Vendor::DeepSeek, "TandemAI/DeepSeek"),
            (Vendor::Llama, "TandemAI/Ollama"),
            (Vendor::Apple, "TandemAI/Apple Intelligence"),
        ];
        for (v, t) in want {
            let got = target(v);
            assert_eq!(got, t);
            assert!(got.starts_with(TARGET_PREFIX));
            assert!(pwm_source::parse_target(&got).is_none(), "{got}");
            assert!(!got.starts_with(&format!("{}/", pwm_source::SERVICE)), "{got}");
        }
        assert_eq!(account(Vendor::OpenAI), "OpenAI API Key for Tandem");
        assert_eq!(account(Vendor::Anthropic), "Anthropic API Key for Tandem");
        assert_eq!(account(Vendor::Gemini), "Gemini API Key for Tandem");
        assert_eq!(account(Vendor::DeepSeek), "DeepSeek API Key for Tandem");
        assert_eq!(account(Vendor::Llama), "Llama API Key for Tandem");
        assert_eq!(account(Vendor::Apple), "Apple Intelligence API Key for Tandem");
        assert!(!format!("{:?}", Secret::new("sk-ant-1".into())).contains("sk-ant-1"));
        assert_eq!(COMMENT, "Tandem AI API key");
    }

    #[test]
    fn keys_follow_iterm2() {
        let mut s = FakeStore::default();
        let mut k = Keys::default();
        assert!(k.set(Vendor::Anthropic, &Secret::new("  sk-ant-1 \n".into()), &mut s).is_ok());
        assert_eq!(s.log, vec![("write", "TandemAI/Anthropic".to_string(), "Anthropic API Key for Tandem".to_string())]);
        assert!(is(&s.map.get("TandemAI/Anthropic").map(Secret::duplicate), "sk-ant-1"));
        assert!(is(&k.get(Vendor::Anthropic, &mut s), "sk-ant-1"));
        assert_eq!(reads(&s), 0);

        s.log.clear();
        assert!(k.set(Vendor::Anthropic, &Secret::new("   ".into()), &mut s).is_ok());
        assert_eq!(s.log, vec![("delete", "TandemAI/Anthropic".to_string(), String::new())]);
        assert!(!s.map.contains_key("TandemAI/Anthropic"));
        assert!(!k.is_set(Vendor::Anthropic, &mut s));
        assert!(k.get(Vendor::Anthropic, &mut s).is_none());
        assert_eq!(reads(&s), 0);

        // One write: iTerm2's second (legacy account) write names the same item.
        s.log.clear();
        assert!(k.set(Vendor::OpenAI, &Secret::new("k".into()), &mut s).is_ok());
        assert_eq!(s.log, vec![("write", "TandemAI/OpenAI".to_string(), "OpenAI API Key for Tandem".to_string())]);

        // A fresh cache reads once.
        let mut s = FakeStore::default();
        s.map.insert("TandemAI/Gemini".into(), Secret::new("g".into()));
        let mut k = Keys::default();
        assert!(is(&k.get(Vendor::Gemini, &mut s), "g"));
        assert!(is(&k.get(Vendor::Gemini, &mut s), "g"));
        assert!(k.is_set(Vendor::Gemini, &mut s));
        assert_eq!(s.log, vec![("read", "TandemAI/Gemini".to_string(), String::new())]);

        // A failed write is returned, and the new key is still cached for the session.
        let mut s = FakeStore { write_error: Some("boom".into()), ..Default::default() };
        let mut k = Keys::default();
        assert_eq!(k.set(Vendor::Anthropic, &Secret::new("sk-ant-2".into()), &mut s), Err("boom".to_string()));
        assert!(is(&k.get(Vendor::Anthropic, &mut s), "sk-ant-2"));
        assert_eq!(reads(&s), 0);

        assert_eq!(normalized("  "), None);
        assert_eq!(normalized(""), None);
        assert_eq!(normalized(" a "), Some("a"));
        assert_eq!(normalized("\ta\r\n"), Some("a"));
    }

    #[test]
    fn hard_errors_are_not_cached() {
        let mut s = FakeStore::default();
        s.read_answers = VecDeque::from([Err("locked".to_string()), Err("locked".to_string()), Ok(Some(Secret::new("d".into())))]);
        let mut k = Keys::default();
        assert!(k.get(Vendor::DeepSeek, &mut s).is_none());
        assert!(k.get(Vendor::DeepSeek, &mut s).is_none());
        assert!(is(&k.get(Vendor::DeepSeek, &mut s), "d"));
        assert_eq!(reads(&s), 3);
        assert!(is(&k.get(Vendor::DeepSeek, &mut s), "d"));
        assert!(k.is_set(Vendor::DeepSeek, &mut s));
        assert_eq!(reads(&s), 3);
        // A key stored as whitespace reads as no key (iTerm2's keyIsEmpty), and that is cached.
        let mut s = FakeStore::default();
        s.map.insert("TandemAI/OpenAI".into(), Secret::new(" \n".into()));
        let mut k = Keys::default();
        assert!(!k.is_set(Vendor::OpenAI, &mut s));
        assert!(k.get(Vendor::OpenAI, &mut s).is_none());
        assert_eq!(reads(&s), 1);
    }

    #[test]
    fn prefix_checks_follow_iterm2() {
        assert!(key_matches("sk-ant-x", Vendor::Anthropic));
        assert!(!key_matches("sk-x", Vendor::Anthropic));
        assert!(key_matches("AIza1", Vendor::Gemini));
        assert!(!key_matches("sk-1", Vendor::Gemini));
        assert!(!key_matches("sk-ant-x", Vendor::OpenAI));
        assert!(!key_matches("AIza", Vendor::DeepSeek));
        assert!(key_matches(" sk-1 ", Vendor::OpenAI));
        assert!(key_matches("anything", Vendor::Llama));
        assert!(key_matches("anything", Vendor::Apple));
        assert!(key_matches("sk-1", Vendor::DeepSeek));
    }

    #[cfg(not(windows))]
    #[test]
    fn the_system_store_is_empty_off_windows() {
        let mut s = system_store();
        assert!(matches!(s.read("TandemAI/OpenAI"), Ok(None)));
        assert_eq!(s.write("TandemAI/OpenAI", "u", &Secret::new("k".into())), Err("Credential Manager is only on Windows.".to_string()));
    }
}
