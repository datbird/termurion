//! The names and identity of this computer, for deciding whether a session reported by a program
//! runs here: iTerm2's `NSHost+iTerm.m` local host names (with its fake name override) and
//! `iTermMachineIdentity.swift` (the OSC 7 `machineID` token). Everything a program reports (a
//! host name, a token) is untrusted: it is cleaned and bounded where it is read, never logged,
//! and a locality answer only ever means "this machine's name"; the callers that decide what a
//! pane may open or run still ask `remote_text` first.

use sha2::{Digest, Sha256};
#[cfg(not(test))]
use std::sync::Mutex;

/// iTerm2's HMAC key for the machine identity (`iTermMachineIdentity.swift`).
pub const HMAC_KEY: &str = "iterm2-osc7-machine-id";

/// The longest host name kept from a program (a DNS name's limit).
pub const MAX_HOST: usize = 255;

/// The longest machine token kept: "1:" and a 128-character value.
pub const MAX_TOKEN: usize = 130;

/// Whether the session reported a host on this machine, from the token and the names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Locality {
    Local,
    Remote,
    Unknown,
}

/// The names this computer answers to (`NSHost+iTerm.m:86-100`): the fake alone when it is not
/// empty, else "localhost", the computer name and the DNS name, without empties or duplicates
/// (compared ignoring case, as Windows names are).
pub fn local_names(fake: &str, computer: &str, dns: &str) -> Vec<String> {
    if !fake.is_empty() {
        return vec![fake.to_string()];
    }
    let mut out: Vec<String> = Vec::new();
    for n in ["localhost", computer, dns] {
        if !n.is_empty() && !out.iter().any(|o| o.eq_ignore_ascii_case(n)) {
            out.push(n.to_string());
        }
    }
    out
}

/// Whether `host` is one of `names` (ignoring case; an empty name is never local).
pub fn is_local_name(host: &str, names: &[String]) -> bool {
    names.iter().any(|n| !n.is_empty() && n.eq_ignore_ascii_case(host))
}

/// `iTermMachineIdentity.swift:32-54`: the token is "<version>:<value>"; only version 1 is
/// known; without our own value nothing can be said; an empty value is "not this machine".
pub fn locality(token: Option<&str>, ours: Option<&str>) -> Locality {
    let Some((version, value)) = token.and_then(|t| t.split_once(':')) else { return Locality::Unknown };
    if version != "1" {
        return Locality::Unknown;
    }
    let Some(ours) = ours else { return Locality::Unknown };
    if value.is_empty() {
        Locality::Remote
    } else if value.eq_ignore_ascii_case(ours) {
        Locality::Local
    } else {
        Locality::Remote
    }
}

/// Whether a reported session is on this machine: the token decides when it is known, else the
/// host name (none, or one of `names`, is local).
pub fn is_local_session(host: Option<&str>, token: Option<&str>, names: &[String], ours: Option<&str>) -> bool {
    match locality(token, ours) {
        Locality::Local => true,
        Locality::Remote => false,
        Locality::Unknown => host.is_none_or(|h| is_local_name(h, names)),
    }
}

/// A host name from a program, as it may be kept and shown: controls and bidirectional
/// formatting characters are cut, the length is bounded, an empty result is no host.
pub fn clean_host(raw: &str) -> Option<String> {
    let clean = crate::inert_files::display_text(raw, MAX_HOST, false);
    (!clean.is_empty()).then_some(clean)
}

/// A machine token from a program: kept only when it is short and plain (`[0-9A-Za-z:]`).
pub fn clean_token(raw: &str) -> Option<String> {
    (raw.len() <= MAX_TOKEN && raw.bytes().all(|b| b.is_ascii_alphanumeric() || b == b':')).then(|| raw.to_string())
}

/// HMAC-SHA256 (RFC 2104) as lowercase hex.
pub fn hmac_sha256_hex(key: &[u8], msg: &[u8]) -> String {
    const BLOCK: usize = 64;
    let mut k = [0u8; BLOCK];
    if key.len() > BLOCK {
        k[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let mut inner = Sha256::new();
    inner.update(k.map(|b| b ^ 0x36));
    inner.update(msg);
    let mut outer = Sha256::new();
    outer.update(k.map(|b| b ^ 0x5c));
    outer.update(inner.finalize());
    outer.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// This boot's identity on Windows: the machine's GUID and the boot counter.
pub fn boot_identity(machine_guid: &str, boot_id: u32) -> String {
    format!("{machine_guid}:{boot_id}")
}

/// What a token from this machine holds (`iTermMachineIdentity.swift:94-113`): the HMAC of the
/// fake when one is set, else of the real identity; never the raw identity.
pub fn our_value(fake: &str, real: Option<&str>) -> Option<String> {
    let source = if fake.is_empty() { real? } else { fake };
    Some(hmac_sha256_hex(HMAC_KEY.as_bytes(), source.as_bytes()))
}

/// The real identity, read once per process (as iTerm2 computes it once; only the fake is live).
#[cfg(windows)]
pub fn real_identity() -> Option<&'static str> {
    static REAL: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    REAL.get_or_init(machine_source).as_deref()
}

#[cfg(not(windows))]
pub fn real_identity() -> Option<&'static str> {
    None
}

/// `HKLM\SOFTWARE\Microsoft\Cryptography\MachineGuid` and
/// `HKLM\SYSTEM\CurrentControlSet\Control\Session Manager\Memory Management\PrefetchParameters\BootId`
/// joined by `boot_identity`; None when either cannot be read. Read only.
#[cfg(windows)]
pub fn machine_source() -> Option<String> {
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegGetValueW};
    let wide = |s: &str| s.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
    let guid = {
        let (key, name) = (wide("SOFTWARE\\Microsoft\\Cryptography"), wide("MachineGuid"));
        let mut buf = vec![0u16; 128];
        let mut bytes = (buf.len() * 2) as u32;
        let rc = unsafe { RegGetValueW(HKEY_LOCAL_MACHINE, key.as_ptr(), name.as_ptr(), RRF_RT_REG_SZ, std::ptr::null_mut(), buf.as_mut_ptr() as *mut _, &mut bytes) };
        if rc != ERROR_SUCCESS {
            return None;
        }
        let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        String::from_utf16_lossy(&buf[..end])
    };
    let boot = {
        let key = wide("SYSTEM\\CurrentControlSet\\Control\\Session Manager\\Memory Management\\PrefetchParameters");
        let name = wide("BootId");
        let mut data = 0u32;
        let mut bytes = 4u32;
        let rc = unsafe { RegGetValueW(HKEY_LOCAL_MACHINE, key.as_ptr(), name.as_ptr(), RRF_RT_REG_DWORD, std::ptr::null_mut(), &mut data as *mut u32 as *mut _, &mut bytes) };
        if rc != ERROR_SUCCESS {
            return None;
        }
        data
    };
    (!guid.is_empty()).then(|| boot_identity(&guid, boot))
}

/// The two fake values, refreshed by the app from the settings each pass, so a change applies to
/// hosts reported afterward (`NSHost+iTerm.m:70-75`).
#[cfg(not(test))]
static FAKES: Mutex<(String, String)> = Mutex::new((String::new(), String::new()));

// Tests run beside each other on threads of one process: each sees only the fakes it set itself.
#[cfg(test)]
thread_local! {
    static FAKES: std::cell::RefCell<(String, String)> = const { std::cell::RefCell::new((String::new(), String::new())) };
}

pub fn set_fakes(name: &str, boot: &str) {
    #[cfg(not(test))]
    if let Ok(mut f) = FAKES.lock() {
        *f = (name.to_string(), boot.to_string());
    }
    #[cfg(test)]
    FAKES.with(|f| *f.borrow_mut() = (name.to_string(), boot.to_string()));
}

/// The fake host name and fake boot identity now in effect.
pub fn fakes() -> (String, String) {
    #[cfg(not(test))]
    return FAKES.lock().map(|f| f.clone()).unwrap_or_default();
    #[cfg(test)]
    FAKES.with(|f| f.borrow().clone())
}

/// `our_value` for the fake boot identity given, over `real` (the cached real identity when None
/// is passed by tests; callers use `current_ours`).
pub fn current_ours_with(fake_boot: &str, real: Option<&str>) -> Option<String> {
    our_value(fake_boot, real)
}

/// How many times `cached_ours` really computed the HMAC (a test reads it).
static OURS_COMPUTED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// `our_value`, remembered for the inputs it was last asked with: the status bar asks every
/// frame, and the HMAC is computed again only when the fake boot identity changes.
pub fn cached_ours(fake: &str, real: Option<&str>) -> Option<String> {
    static LAST: std::sync::Mutex<Option<(String, Option<String>, Option<String>)>> = std::sync::Mutex::new(None);
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((f, r, out)) = last.as_ref()
        && f == fake && r.as_deref() == real {
            return out.clone();
        }
    OURS_COMPUTED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let out = our_value(fake, real);
    *last = Some((fake.to_string(), real.map(str::to_string), out.clone()));
    out
}

/// This machine's token value now.
pub fn current_ours() -> Option<String> {
    cached_ours(&fakes().1, real_identity())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names() -> Vec<String> {
        local_names("", "PC1", "pc1.corp")
    }

    /// The status bar asks every frame: the HMAC is computed once per fake boot identity.
    #[test]
    fn our_value_is_cached_per_fake_identity() {
        use std::sync::atomic::Ordering::Relaxed;
        // Inputs no other test uses, so the cache starts cold for them.
        let (a, b) = ("cache-test-a", "cache-test-b");
        let before = OURS_COMPUTED.load(Relaxed);
        let first = cached_ours(a, Some("real"));
        assert_eq!(first, our_value(a, Some("real")));
        for _ in 0..50 {
            assert_eq!(cached_ours(a, Some("real")), first);
        }
        // Other tests may fill the cache between calls, so count only a lower bound that a
        // per-call HMAC would break: 51 calls on one input compute far fewer than 51 times.
        assert!(OURS_COMPUTED.load(Relaxed) - before < 10, "the HMAC is not computed per call");
        assert_ne!(cached_ours(b, Some("real")), first, "a different fake identity gives its own value");
        assert_eq!(cached_ours(b, Some("real")), our_value(b, Some("real")));
    }

    /// `NSHost+iTerm.m:86-100`.
    #[test]
    fn local_names_follow_iterm2() {
        assert_eq!(names(), ["localhost", "PC1", "pc1.corp"]);
        assert_eq!(local_names("fake.host", "PC1", "pc1.corp"), ["fake.host"]);
        assert_eq!(local_names("", "PC1", ""), ["localhost", "PC1"], "the empty name is dropped");
        assert_eq!(local_names("", "PC1", "pc1"), ["localhost", "PC1"], "duplicates ignoring case");
        assert!(is_local_name("PC1.CORP", &names()));
        assert!(is_local_name("LOCALHOST", &names()));
        assert!(!is_local_name("other", &names()));
        assert!(!is_local_name("", &["".to_string()]), "an empty name is never local");
    }

    /// `iTermMachineIdentity.swift:32-54`.
    #[test]
    fn locality_follows_iterm2() {
        assert_eq!(locality(None, Some("ab")), Locality::Unknown);
        assert_eq!(locality(Some("x"), Some("ab")), Locality::Unknown, "no colon");
        assert_eq!(locality(Some("2:ab"), Some("ab")), Locality::Unknown, "unknown version");
        assert_eq!(locality(Some("1:ab"), None), Locality::Unknown);
        assert_eq!(locality(Some("1:"), Some("ab")), Locality::Remote);
        assert_eq!(locality(Some("1:AB"), Some("ab")), Locality::Local);
        assert_eq!(locality(Some("1:cd"), Some("ab")), Locality::Remote);
        assert_eq!(locality(Some("0:"), Some("ab")), Locality::Unknown, "version 0 is not ours");
        let n = names();
        assert!(is_local_session(Some("elsewhere"), Some("1:ab"), &n, Some("ab")), "the token wins");
        assert!(!is_local_session(Some("PC1"), Some("1:"), &n, Some("ab")));
        assert!(is_local_session(Some("PC1"), None, &n, Some("ab")));
        assert!(is_local_session(None, None, &n, Some("ab")));
        assert!(!is_local_session(Some("elsewhere"), Some("0:"), &n, Some("ab")), "unknown falls back to the name");
        assert!(!is_local_session(Some("elsewhere"), None, &n, None));
    }

    #[test]
    fn identity_hash_follows_windows() {
        assert_eq!(hmac_sha256_hex(b"key", b"The quick brown fox jumps over the lazy dog"), "f7bc83f430538424b13298e6aa6fb143ef4d59a14946175997479dbc2d1a3cd8");
        // A key longer than the block is hashed first (RFC 4231 case 6).
        assert_eq!(
            hmac_sha256_hex(&[0xaa; 131], b"Test Using Larger Than Block-Size Key - Hash Key First"),
            "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"
        );
        assert_eq!(boot_identity("1234-ab", 7), "1234-ab:7");
        assert_eq!(our_value("fake", Some("1234-ab:7")), Some(hmac_sha256_hex(HMAC_KEY.as_bytes(), b"fake")));
        assert_eq!(our_value("", Some("1234-ab:7")), Some(hmac_sha256_hex(HMAC_KEY.as_bytes(), b"1234-ab:7")));
        assert_eq!(our_value("", None), None);
        assert_eq!(our_value("fake", None), Some(hmac_sha256_hex(HMAC_KEY.as_bytes(), b"fake")), "the fake needs no real identity");
    }

    /// A program names the host: controls, bidi marks and length are cut wherever it is kept.
    #[test]
    fn host_names_from_programs_are_cleaned() {
        assert_eq!(clean_host("box.example"), Some("box.example".to_string()));
        assert_eq!(clean_host("bo\u{1b}[31mx\u{7}"), Some("bo[31mx".to_string()));
        assert_eq!(clean_host("a\u{202e}b\u{2066}c\u{200f}d"), Some("abcd".to_string()));
        assert_eq!(clean_host("a\u{200b}b\u{feff}c\u{180e}d"), Some("abcd".to_string()), "zero-width characters are cut like the other shown text");
        assert_eq!(clean_host("\u{202e}"), None);
        assert_eq!(clean_host(""), None);
        assert_eq!(clean_host(&"h".repeat(1000)).unwrap().len(), MAX_HOST);
    }

    /// A fake name is the only local name, so a real name stops being local while it is set,
    /// and a session that is framed by a conductor stays remote whatever it reports (P7: the
    /// guard is `remote_text`, tested where it lives; here the names alone).
    #[test]
    fn a_fake_name_replaces_every_real_one() {
        let fake = local_names("fake.host", "PC1", "pc1.corp");
        assert!(!is_local_name("localhost", &fake));
        assert!(!is_local_name("PC1", &fake));
        assert!(is_local_name("FAKE.host", &fake));
    }

    /// T4 m2: tests run on threads of one process; a test that sets the fakes must not change what a
    /// test running beside it reads.
    #[test]
    fn fakes_set_in_a_test_do_not_reach_other_tests() {
        set_fakes("only.here", "boot-x");
        let other = std::thread::spawn(fakes).join().unwrap();
        assert_eq!(other, (String::new(), String::new()));
        set_fakes("", "");
    }

    /// The live fake is read from the process-wide cell the app refreshes each pass.
    #[test]
    fn the_fakes_are_read_live() {
        let before = fakes();
        set_fakes("fake.one", "boot-a");
        assert_eq!(fakes(), ("fake.one".to_string(), "boot-a".to_string()));
        assert_eq!(current_ours_with(&fakes().1, None), Some(hmac_sha256_hex(HMAC_KEY.as_bytes(), b"boot-a")));
        set_fakes("fake.two", "");
        assert_eq!(fakes().0, "fake.two", "a change applies to hosts reported afterward");
        set_fakes(&before.0, &before.1);
    }

    #[cfg(windows)]
    #[test]
    fn powershell_and_rust_agree() {
        // The script's function is cut out of the resource and run alone: loading the whole
        // script needs a console host (prompt, PSReadLine). No registry value, no network.
        let script = include_str!("../resources/shell_integration/termurion_shell_integration.ps1");
        let start = script.find("function global:Get-TermurionMachineIdHash").expect("the hash function");
        let end = start + script[start..].find("\n}\n").expect("its end") + 3;
        let body = &script[start..end];
        let root = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".to_string());
        let exe = format!("{root}\\System32\\WindowsPowerShell\\v1.0\\powershell.exe");
        let out = std::process::Command::new(exe)
            .args(["-NoProfile", "-NonInteractive", "-Command", &format!("{body}\nGet-TermurionMachineIdHash '1234-ab:7'")])
            .output()
            .expect("powershell runs");
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), hmac_sha256_hex(HMAC_KEY.as_bytes(), b"1234-ab:7"));
    }

    #[cfg(windows)]
    #[test]
    fn the_real_identity_is_read_from_the_registry() {
        let id = machine_source().expect("MachineGuid and BootId are readable by a normal user");
        let (guid, boot) = id.rsplit_once(':').unwrap();
        assert!(guid.len() >= 32 && boot.parse::<u32>().is_ok(), "{} chars", id.len());
        assert_eq!(real_identity(), Some(id.as_str()));
    }

    /// R3: the script sent `1:` with no value, which iTerm2's rule reads as "not this machine".
    #[test]
    fn resources_ps1_computes_the_identity() {
        let script = include_str!("../resources/shell_integration/termurion_shell_integration.ps1");
        assert!(script.contains("iterm2-osc7-machine-id") && script.contains("HMACSHA256"));
        assert!(script.contains("MachineGuid") && script.contains("BootId") && script.contains("Get-ItemPropertyValue"));
        assert!(!script.contains("machineID=1:\")"), "the empty value is gone");
        assert!(script.contains("?machineID=$global:__TermurionMachineId"));
        assert!(script.contains("'0:'"), "identity unavailable is 0:");
        assert!(!script.contains("Invoke-WebRequest") && !script.contains("Invoke-RestMethod"));
    }
}
