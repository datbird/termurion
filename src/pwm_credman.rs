//! Windows Credential Manager as a password manager source (iTerm2's `KeychainPasswordDataSource`
//! in Windows words). Accounts are the current user's generic credentials whose target starts
//! with `Tandem/` (`pwm_source::target_name` and `parse_target` hold the naming); the password is
//! the credential's blob as UTF-8. A blob is copied straight into a `Secret` and the copy Windows
//! handed back is zeroed before `CredFree`. The generic-target functions (`write_generic`,
//! `read_generic`, `delete_generic`) also serve the AI keys under `TandemAI/` (`ai_keys`), and
//! refuse any target outside Tandem's two prefixes before calling Windows. No test calls
//! Credential Manager; the milestone's checklist covers that by hand.

use crate::pwm::{Account, Secret};
use crate::pwm_source::{self, Caps, Kind, Scope, Source};
use windows_sys::Win32::Foundation::{GetLastError, ERROR_NOT_FOUND};
use windows_sys::Win32::Security::Credentials::{
    CredDeleteW, CredEnumerateW, CredFree, CredReadW, CredWriteW, CREDENTIALW, CRED_MAX_CREDENTIAL_BLOB_SIZE,
    CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC,
};

/// The comment written on every credential Tandem stores (shown in Control Panel).
const COMMENT: &str = "Tandem password manager";

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn last_error() -> String {
    std::io::Error::last_os_error().to_string()
}

/// The enumeration filter: every target named under the scope's service. `Tandem/*` never
/// matches `Tandem-Browser/...`, so the two scopes do not mix.
fn filter(scope: Scope) -> Vec<u16> {
    wide(&format!("{}/*", scope.service()))
}

/// A NUL-terminated wide string Windows owns, copied out (target names, never secrets). None
/// when it is not valid UTF-16: a name decoded with replacements would no longer name the
/// credential, so every later call on it would fail.
///
/// # Safety
/// `p` is null or points at a NUL-terminated UTF-16 string.
unsafe fn from_wide(p: *const u16) -> Option<String> {
    if p.is_null() {
        return Some(String::new());
    }
    // SAFETY: the caller's contract; the scan stops at the terminating NUL.
    unsafe {
        let mut n = 0;
        while *p.add(n) != 0 {
            n += 1;
        }
        String::from_utf16(std::slice::from_raw_parts(p, n)).ok()
    }
}

/// Overwrites a credential's blob with zeros before it is freed.
///
/// # Safety
/// `c` points at a `CREDENTIALW` Windows returned, whose blob is writable for its size.
unsafe fn zero_blob(c: *mut CREDENTIALW) {
    // SAFETY: the caller's contract; writes stay within the blob's size.
    unsafe {
        let blob = (*c).CredentialBlob;
        if !blob.is_null() {
            for i in 0..(*c).CredentialBlobSize as usize {
                std::ptr::write_volatile(blob.add(i), 0);
            }
        }
    }
    std::sync::atomic::compiler_fence(std::sync::atomic::Ordering::SeqCst);
}

/// The account's target, refused unless it is one of Tandem's (so nothing here ever reads,
/// writes or deletes another program's credential).
fn target(scope: Scope, a: &Account) -> Result<&str, String> {
    match pwm_source::parse_target_in(scope, &a.id) {
        Some(_) => Ok(&a.id),
        None => Err("Not a Tandem credential.".to_string()),
    }
}

/// The prefixes of every target Tandem owns: the password manager's `Tandem/` and
/// `Tandem-Browser/` and the AI keys' `TandemAI/` (compared case-sensitively).
const OWNED_PREFIXES: [&str; 3] = ["Tandem/", "Tandem-Browser/", "TandemAI/"];

/// Whether a target is one of Tandem's. The generic functions below refuse any other target
/// before calling Windows, so nothing here ever touches another program's credential.
pub(crate) fn owned(target: &str) -> bool {
    OWNED_PREFIXES.iter().any(|p| target.starts_with(p))
}

fn guard(target: &str) -> Result<(), String> {
    if owned(target) {
        Ok(())
    } else {
        Err("Not a Tandem credential.".to_string())
    }
}

/// Writes (creates or replaces) a generic credential for the current user on this machine.
/// `what` names the secret in the too-long text ("password", "API key").
pub(crate) fn write_generic(target: &str, user: &str, secret: &Secret, comment: &str, what: &str) -> Result<(), String> {
    guard(target)?;
    let bytes = secret.expose().as_bytes();
    if bytes.len() > CRED_MAX_CREDENTIAL_BLOB_SIZE as usize {
        return Err(format!(
            "The {what} is too long for Windows Credential Manager (at most {CRED_MAX_CREDENTIAL_BLOB_SIZE} bytes)."
        ));
    }
    let mut target_w = wide(target);
    let mut comment_w = wide(comment);
    let mut user_w = wide(user);
    // SAFETY: CREDENTIALW is plain data; all-zero is a valid empty credential (null pointers,
    // no attributes), and every field Windows reads is set below.
    let mut c: CREDENTIALW = unsafe { std::mem::zeroed() };
    c.Type = CRED_TYPE_GENERIC;
    c.TargetName = target_w.as_mut_ptr();
    c.Comment = comment_w.as_mut_ptr();
    c.CredentialBlobSize = bytes.len() as u32;
    // CredWriteW only reads the blob; the pointer is mutable in the struct's type alone. An
    // empty secret has no blob (null, not an empty slice's dangling pointer).
    if !bytes.is_empty() {
        c.CredentialBlob = bytes.as_ptr() as *mut u8;
    }
    c.Persist = CRED_PERSIST_LOCAL_MACHINE;
    if !user.is_empty() {
        c.UserName = user_w.as_mut_ptr();
    }
    // SAFETY: every pointer in `c` outlives the call (the wide buffers and the secret's bytes).
    if unsafe { CredWriteW(&c, 0) } == 0 {
        return Err(last_error());
    }
    Ok(())
}

/// Reads a generic credential's blob as UTF-8 text; `Ok(None)` when there is no such
/// credential. `what` names the secret in the not-UTF-8 text ("password", "API key").
pub(crate) fn read_generic(target: &str, what: &str) -> Result<Option<Secret>, String> {
    guard(target)?;
    let target_w = wide(target);
    let mut c: *mut CREDENTIALW = std::ptr::null_mut();
    // SAFETY: valid target and out-pointer; on success Windows owns `c` until CredFree.
    if unsafe { CredReadW(target_w.as_ptr(), CRED_TYPE_GENERIC, 0, &mut c) } == 0 {
        // SAFETY: plain thread-local read right after the failing call.
        if unsafe { GetLastError() } == ERROR_NOT_FOUND {
            return Ok(None);
        }
        return Err(last_error());
    }
    // SAFETY: `c` is a valid credential until CredFree; the blob is `CredentialBlobSize`
    // bytes. The copy goes into a buffer the Secret then owns (from_utf8 does not copy), and
    // a buffer that is not UTF-8 is wiped before it is dropped.
    let decoded = unsafe {
        let size = (*c).CredentialBlobSize as usize;
        let mut bytes = Vec::with_capacity(size);
        if size > 0 {
            bytes.extend_from_slice(std::slice::from_raw_parts((*c).CredentialBlob, size));
        }
        zero_blob(c);
        CredFree(c as *const _);
        String::from_utf8(bytes)
    };
    match decoded {
        Ok(s) => Ok(Some(Secret::new(s))),
        Err(e) => {
            let mut raw = e.into_bytes();
            for b in raw.iter_mut() {
                // SAFETY: a plain byte write into our own buffer.
                unsafe { std::ptr::write_volatile(b, 0) };
            }
            Err(format!("The stored {what} is not UTF-8 text."))
        }
    }
}

/// Deletes a generic credential: `Ok(true)` when one was deleted, `Ok(false)` when there was
/// none (still a success).
pub(crate) fn delete_generic(target: &str) -> Result<bool, String> {
    guard(target)?;
    let target_w = wide(target);
    // SAFETY: a valid NUL-terminated target.
    if unsafe { CredDeleteW(target_w.as_ptr(), CRED_TYPE_GENERIC, 0) } == 0 {
        // SAFETY: plain thread-local read right after the failing call.
        if unsafe { GetLastError() } == ERROR_NOT_FOUND {
            return Ok(false);
        }
        return Err(last_error());
    }
    Ok(true)
}

/// The text `last_error()` gives for `ERROR_NOT_FOUND`: the password manager still reports a
/// missing credential as an error.
fn not_found() -> String {
    std::io::Error::from_raw_os_error(ERROR_NOT_FOUND as i32).to_string()
}

/// The password manager's write: its comment, and "password" in the too-long text.
fn write(target: &str, user: &str, p: &Secret) -> Result<(), String> {
    write_generic(target, user, p, COMMENT, "password")
}

/// Windows Credential Manager, always available, one account, no one-time codes.
#[derive(Default)]
pub struct CredentialManager {
    scope: Scope,
}

impl CredentialManager {
    pub fn new() -> CredentialManager {
        CredentialManager::for_scope(Scope::Terminal)
    }

    /// The Credential Manager source of one password manager scope (`Tandem/` or
    /// `Tandem-Browser/` targets).
    pub fn for_scope(scope: Scope) -> CredentialManager {
        CredentialManager { scope }
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
        pwm_source::credman_caps()
    }

    fn available(&mut self) -> Result<(), String> {
        Ok(())
    }

    fn accounts(&mut self) -> Result<Vec<Account>, String> {
        let mut count = 0u32;
        let mut list: *mut *mut CREDENTIALW = std::ptr::null_mut();
        let filter = filter(self.scope);
        // SAFETY: out-pointers are valid; on success Windows owns `list` until CredFree.
        if unsafe { CredEnumerateW(filter.as_ptr(), 0, &mut count, &mut list) } == 0 {
            // SAFETY: plain thread-local read right after the failing call.
            if unsafe { GetLastError() } == ERROR_NOT_FOUND {
                return Ok(Vec::new());
            }
            return Err(last_error());
        }
        let mut out = Vec::with_capacity(count as usize);
        for i in 0..count as usize {
            // SAFETY: `list` holds `count` valid credential pointers until CredFree below.
            unsafe {
                let c = *list.add(i);
                // The filter is case-insensitive; parse_target keeps only Tandem's exact prefix.
                // A name that is not valid UTF-16 is skipped (it could not be used).
                if let Some(a) = from_wide((*c).TargetName).and_then(|t| pwm_source::parse_target_in(self.scope, &t)) {
                    out.push(a);
                }
                // Enumeration returns every blob too; none is read, all are zeroed.
                zero_blob(c);
            }
        }
        // SAFETY: `list` came from CredEnumerateW and is freed once.
        unsafe { CredFree(list as *const _) };
        Ok(out)
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
        // Credential Manager has no Add Account toggles, so flags is ignored (as iTerm2's keychain).
        let t = pwm_source::target_name_in(self.scope, name, user);
        write(&t, user, &p)?;
        pwm_source::parse_target_in(self.scope, &t).ok_or_else(|| "Not a Tandem credential.".to_string())
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

    #[test]
    fn names_decode_strictly_and_the_filter_follows_service() {
        assert_eq!(filter(Scope::Terminal), wide("Tandem/*"));
        let good = wide("Tandem/site/me");
        assert_eq!(unsafe { from_wide(good.as_ptr()) }.as_deref(), Some("Tandem/site/me"));
        // An unpaired surrogate: skipped, never shown as a row that names nothing.
        let bad = [0x54u16, 0xD800, 0x61, 0];
        assert_eq!(unsafe { from_wide(bad.as_ptr()) }, None);
    }

    #[test]
    fn credman_scopes_do_not_mix() {
        assert_eq!(filter(Scope::Browser), wide("Tandem-Browser/*"));
        assert_eq!(filter(Scope::Terminal), wide("Tandem/*"));
        let a = pwm_source::parse_target_in(Scope::Browser, "Tandem-Browser/https://x.y").unwrap();
        assert!(target(Scope::Browser, &a).is_ok());
        assert!(target(Scope::Terminal, &a).is_err());
        assert!(owned("Tandem-Browser/https://x.y"));
        assert!(!owned("Tandem-Browse/x"));
    }

    #[test]
    fn generic_targets_are_guarded() {
        assert!(owned("TandemAI/OpenAI"));
        assert!(owned("Tandem/site"));
        assert!(!owned("TandemAIX/x"));
        assert!(!owned("tandemai/x"));
        assert!(!owned("Other/x"));
        // Each refusal returns before any Win32 call.
        let refused = Err("Not a Tandem credential.".to_string());
        assert_eq!(write_generic("Other/x", "", &Secret::new("k".into()), "c", "API key"), refused);
        assert!(matches!(read_generic("Other/x", "API key"), Err(e) if e == "Not a Tandem credential."));
        assert_eq!(delete_generic("Other/x"), Err("Not a Tandem credential.".to_string()));
    }

    #[test]
    fn too_long_secrets_are_refused_with_their_name() {
        // One byte over the limit: refused before any Win32 call, naming the secret, never
        // showing it.
        let long = Secret::new("k".repeat(CRED_MAX_CREDENTIAL_BLOB_SIZE as usize + 1));
        assert_eq!(
            write_generic("Tandem/site/me", "me", &long, "c", "password"),
            Err("The password is too long for Windows Credential Manager (at most 2560 bytes).".to_string())
        );
        assert_eq!(
            write_generic("TandemAI/OpenAI", "OpenAI API Key for Tandem", &long, "c", "API key"),
            Err("The API key is too long for Windows Credential Manager (at most 2560 bytes).".to_string())
        );
    }
}
