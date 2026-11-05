//! The Secret Service (GNOME Keyring, KeePassXC, KWallet's bridge) as the password manager's store
//! and the AI keys' store. An item has the label `Termurion: <target>` and the attributes
//! `application = termurion`, `target = <Credential Manager target>`, `user = <user>`; the password
//! manager lists by `application = termurion`, so another program's item is never read or touched.
//! Every call runs on its own thread with a 5 s bound. A locked keyring is unlocked through the
//! service's own prompt on a worker that may wait on a person; a call made while that prompt is open
//! answers "locked". A missing service is an error naming it, never "nothing stored". No secret is
//! ever logged.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::Duration;

use secret_service::blocking::{Collection, Item, SecretService};
use secret_service::{EncryptionType, Error as SsError};

use crate::platform::credentials::guard;
use crate::pwm::{Account, Secret};
use crate::pwm_source::{self, Caps, Kind, Scope, Source};

const APP_VALUE: &str = "termurion";
const BOUND: Duration = Duration::from_secs(5);
/// How long a call waits for an unlock before it answers "locked" and leaves the prompt open.
const UNLOCK_WAIT: Duration = Duration::from_secs(3);
const LOCKED: &str = "The Secret Service is locked; unlock it and try again";
const UTF8: &str = "text/plain";

/// A person is looking at the unlock prompt.
static UNLOCKING: AtomicBool = AtomicBool::new(false);

fn describe(e: SsError) -> String {
    match e {
        SsError::Unavailable => "The Secret Service is not available: no service is running on the session bus.".to_string(),
        SsError::Locked => LOCKED.to_string(),
        SsError::Prompt | SsError::PromptDisconnected => "The Secret Service unlock was dismissed; unlock it and try again".to_string(),
        SsError::NoResult => "The Secret Service has no default keyring.".to_string(),
        other => format!("The Secret Service could not be used: {other}"),
    }
}

/// Runs `f` on its own thread and gives up after `BOUND`; the thread is left to finish by itself.
fn bounded<T: Send + 'static>(f: impl FnOnce() -> Result<T, String> + Send + 'static) -> Result<T, String> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(f());
    });
    rx.recv_timeout(BOUND).unwrap_or_else(|_| Err("The Secret Service did not answer in 5 seconds.".to_string()))
}

/// Makes sure the default keyring is unlocked. The prompt runs on a worker with no bound (it waits on
/// a person); this call waits a few seconds for it, then answers "locked" with the prompt still open.
fn unlocked(col: &Collection) -> Result<(), String> {
    if !col.is_locked().map_err(describe)? {
        return Ok(());
    }
    if UNLOCKING.swap(true, Ordering::SeqCst) {
        return Err(LOCKED.to_string());
    }
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let r = SecretService::connect(EncryptionType::Dh)
            .and_then(|ss| ss.get_default_collection().and_then(|c| c.unlock()))
            .map_err(describe);
        UNLOCKING.store(false, Ordering::SeqCst);
        let _ = tx.send(r);
    });
    match rx.recv_timeout(UNLOCK_WAIT) {
        Ok(r) => r,
        Err(_) => Err(LOCKED.to_string()),
    }
}

/// Connects, finds the default keyring, unlocks it and runs `f` there, all within the bound.
fn with_keyring<T: Send + 'static>(f: impl FnOnce(&Collection) -> Result<T, String> + Send + 'static) -> Result<T, String> {
    bounded(move || {
        let ss = SecretService::connect(EncryptionType::Dh).map_err(describe)?;
        let col = ss.get_default_collection().map_err(describe)?;
        unlocked(&col)?;
        f(&col)
    })
}

fn attrs<'a>(target: &'a str) -> HashMap<&'a str, &'a str> {
    HashMap::from([("application", APP_VALUE), ("target", target)])
}

fn find<'a>(col: &'a Collection, target: &str) -> Result<Vec<Item<'a>>, String> {
    col.search_items(attrs(target)).map_err(describe)
}

/// Deletes every item of the target, whatever its user; true when there was one.
fn delete_all(col: &Collection, target: &str) -> Result<bool, String> {
    let items = find(col, target)?;
    for i in &items {
        i.delete().map_err(describe)?;
    }
    Ok(!items.is_empty())
}

/// Writes (creates or replaces) the item for `target`: any old item goes first, so a rewrite under
/// another user leaves one item. The comment has no attribute (the three are fixed); `what` is unused.
pub(crate) fn write_generic(target: &str, user: &str, secret: &Secret, _comment: &str, _what: &str) -> Result<(), String> {
    guard(target)?;
    let (t, u, s) = (target.to_string(), user.to_string(), secret.duplicate());
    with_keyring(move |col| {
        delete_all(col, &t)?;
        let a = HashMap::from([("application", APP_VALUE), ("target", t.as_str()), ("user", u.as_str())]);
        col.create_item(&format!("Termurion: {t}"), a, s.expose().as_bytes(), false, UTF8).map_err(describe)?;
        Ok(())
    })
}

/// Reads the item's secret as UTF-8 text; `Ok(None)` when there is no such item. `what` names the
/// secret in the not-UTF-8 text ("password", "API key").
pub(crate) fn read_generic(target: &str, what: &str) -> Result<Option<Secret>, String> {
    guard(target)?;
    let (t, what) = (target.to_string(), what.to_string());
    with_keyring(move |col| {
        let items = find(col, &t)?;
        let Some(item) = items.first() else { return Ok(None) };
        let bytes = item.get_secret().map_err(describe)?;
        match String::from_utf8(bytes) {
            Ok(s) => Ok(Some(Secret::new(s))),
            Err(e) => {
                let mut raw = e.into_bytes();
                raw.fill(0);
                Err(format!("The stored {what} is not UTF-8 text."))
            }
        }
    })
}

/// Deletes the item: `Ok(true)` when one was deleted, `Ok(false)` when there was none.
pub(crate) fn delete_generic(target: &str) -> Result<bool, String> {
    guard(target)?;
    let t = target.to_string();
    with_keyring(move |col| delete_all(col, &t))
}

/// The account's target, refused unless it is one of the scope's.
fn target(scope: Scope, a: &Account) -> Result<&str, String> {
    match pwm_source::parse_target_in(scope, &a.id) {
        Some(_) => Ok(&a.id),
        None => Err("Not a Termurion credential.".to_string()),
    }
}

fn not_found() -> String {
    "The Secret Service has no such password.".to_string()
}

/// The Secret Service, one account, no one-time codes.
#[derive(Default)]
pub struct CredentialManager {
    scope: Scope,
}

impl CredentialManager {
    pub fn new() -> CredentialManager {
        CredentialManager::for_scope(Scope::Terminal)
    }

    /// The source of one password manager scope (`Termurion/` or `Termurion-Browser/` items).
    pub fn for_scope(scope: Scope) -> CredentialManager {
        CredentialManager { scope }
    }
}

impl Source for CredentialManager {
    fn kind(&self) -> Kind {
        Kind::CredentialManager
    }

    fn name(&self) -> String {
        "Secret Service".to_string()
    }

    fn caps(&self) -> Caps {
        pwm_source::credman_caps()
    }

    fn available(&mut self) -> Result<(), String> {
        with_keyring(|_| Ok(()))
    }

    fn accounts(&mut self) -> Result<Vec<Account>, String> {
        let scope = self.scope;
        with_keyring(move |col| {
            // Attributes only: no secret is fetched.
            let items = col.search_items(HashMap::from([("application", APP_VALUE)])).map_err(describe)?;
            let mut out = Vec::new();
            for i in &items {
                if let Some(t) = i.get_attributes().map_err(describe)?.get("target") {
                    out.extend(pwm_source::parse_target_in(scope, t));
                }
            }
            Ok(out)
        })
    }

    fn password(&mut self, a: &Account) -> Result<(Secret, Option<Secret>), String> {
        match read_generic(target(self.scope, a)?, "password")? {
            Some(p) => Ok((p, None)),
            None => Err(not_found()),
        }
    }

    fn set_password(&mut self, a: &Account, p: Secret) -> Result<(), String> {
        write_generic(target(self.scope, a)?, &a.user_name, &p, "", "password")
    }

    fn delete(&mut self, a: &Account) -> Result<(), String> {
        if delete_generic(target(self.scope, a)?)? {
            Ok(())
        } else {
            Err(not_found())
        }
    }

    fn add(&mut self, name: &str, user: &str, p: Secret, _flags: &[(String, bool)]) -> Result<Account, String> {
        let t = pwm_source::target_name_in(self.scope, name, user);
        write_generic(&t, user, &p, "", "password")?;
        pwm_source::parse_target_in(self.scope, &t).ok_or_else(|| "Not a Termurion credential.".to_string())
    }

    /// Renames or re-users the item where it stands, and sets the password when one is given.
    fn edit_in_place(&mut self, a: &Account, name: Option<&str>, user: Option<&str>, p: Option<Secret>) -> Result<(), String> {
        let old = target(self.scope, a)?.to_string();
        let new_name = name.unwrap_or(&a.account_name);
        let new_user = user.unwrap_or(&a.user_name).to_string();
        let new = pwm_source::target_name_in(self.scope, new_name, &new_user);
        guard(&new)?;
        let p = p.map(|p| p.duplicate());
        with_keyring(move |col| {
            if new != old && !find(col, &new)?.is_empty() {
                return Err("An account with that name already exists.".to_string());
            }
            let items = find(col, &old)?;
            let Some(item) = items.first() else { return Err(not_found()) };
            let a = HashMap::from([("application", APP_VALUE), ("target", new.as_str()), ("user", new_user.as_str())]);
            item.set_attributes(a).map_err(describe)?;
            item.set_label(&format!("Termurion: {new}")).map_err(describe)?;
            if let Some(p) = p {
                item.set_secret(p.expose().as_bytes(), UTF8).map_err(describe)?;
            }
            Ok(())
        })
    }

    fn toggle_otp(&mut self, _a: &Account) -> Result<Account, String> {
        // Never called: no account has a one-time code.
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
    use std::process::{Command, Stdio};

    /// Runs the ignored test `inner` in a child under its own session bus and keyring daemon with a
    /// scratch HOME, so nothing here can reach the real keyring and no env var of this process moves.
    /// `dead_bus` forces a bus address with nothing on it, else it is a fresh `dbus-run-session`.
    fn child(inner: &str, dead_bus: bool) {
        let dir = std::env::temp_dir().join(format!("termurion-ss-{}-{}", std::process::id(), inner));
        let _ = std::fs::remove_dir_all(&dir);
        let run = dir.join("run");
        std::fs::create_dir_all(&run).unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&run, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let exe = std::env::current_exe().unwrap();
        let name = format!("platform::linux::secrets::tests::{inner}");
        let mut cmd;
        if dead_bus {
            cmd = Command::new(&exe);
            cmd.env("DBUS_SESSION_BUS_ADDRESS", format!("unix:path={}", dir.join("none").display()));
        } else {
            cmd = Command::new("dbus-run-session");
            cmd.args(["--", "bash", "-c"]).arg(
                "echo '' | gnome-keyring-daemon --foreground --unlock --components=secrets >/dev/null 2>&1 &\n\
                 pid=$!; trap 'kill $pid 2>/dev/null' EXIT\n\
                 \"$0\" --ignored --exact \"$1\" --test-threads=1 --nocapture",
            );
            cmd.arg(&exe);
        }
        if dead_bus {
            cmd.args(["--ignored", "--exact", &name, "--test-threads=1", "--nocapture"]);
        } else {
            cmd.arg(&name);
        }
        let out = cmd
            .env("HOME", &dir)
            .env("XDG_DATA_HOME", dir.join("data"))
            .env("XDG_CONFIG_HOME", dir.join("config"))
            .env("XDG_RUNTIME_DIR", &run)
            .env_remove("GNOME_KEYRING_CONTROL")
            .env_remove("SSH_AUTH_SOCK")
            .stdin(Stdio::null())
            .output()
            .expect("run the child test");
        let _ = std::fs::remove_dir_all(&dir);
        let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        assert!(out.status.success() && text.contains("1 passed"), "child {inner} failed:\n{text}");
    }

    /// Waits for the keyring daemon to own the bus name and have an unlocked default keyring.
    fn wait_ready() {
        for _ in 0..100 {
            if CredentialManager::new().accounts().is_ok() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        panic!("the test keyring never came up: {:?}", CredentialManager::new().accounts().err());
    }

    #[test]
    fn a_missing_secret_service_is_an_error_not_empty() {
        child("a_missing_secret_service_is_an_error_not_empty_inner", true);
    }

    #[test]
    #[ignore = "runs in a child under a dead bus"]
    fn a_missing_secret_service_is_an_error_not_empty_inner() {
        let e = read_generic("TermurionAI/OpenAI", "API key").unwrap_err();
        assert!(e.contains("Secret Service"), "{e}");
        let e = CredentialManager::new().available().unwrap_err();
        assert!(e.contains("Secret Service"), "{e}");
        assert!(write_generic("TermurionAI/OpenAI", "u", &Secret::new("k".into()), "c", "API key").unwrap_err().contains("Secret Service"));
        assert!(delete_generic("TermurionAI/OpenAI").unwrap_err().contains("Secret Service"));
    }

    #[test]
    fn a_written_key_reads_back() {
        child("a_written_key_reads_back_inner", false);
    }

    #[test]
    #[ignore = "runs in a child under its own keyring"]
    fn a_written_key_reads_back_inner() {
        wait_ready();
        let t = "TermurionAI/OpenAI";
        write_generic(t, "me", &Secret::new("sk-one".into()), "c", "API key").unwrap();
        assert_eq!(read_generic(t, "API key").unwrap().unwrap().expose(), "sk-one");
        write_generic(t, "me", &Secret::new("sk-two".into()), "c", "API key").unwrap();
        assert_eq!(read_generic(t, "API key").unwrap().unwrap().expose(), "sk-two");
        assert!(read_generic("TermurionAI/Other", "API key").unwrap().is_none());
        // One item, with the three attributes and the label.
        let items = items_with(&[("application", "termurion"), ("target", t)]);
        assert_eq!(items.len(), 1, "a rewrite replaces the item");
        let (label, attrs) = &items[0];
        assert_eq!(label, "Termurion: TermurionAI/OpenAI");
        let mut want: Vec<(String, String)> =
            vec![("application".into(), "termurion".into()), ("target".into(), t.into()), ("user".into(), "me".into())];
        want.sort();
        let mut got: Vec<(String, String)> = attrs.clone();
        got.sort();
        assert_eq!(got, want);
        assert!(read_generic("iTerm2", "password").is_err(), "a foreign target is refused");
    }

    /// The items matching some attributes, as (label, attributes), read straight from the service.
    fn items_with(q: &[(&str, &str)]) -> Vec<(String, Vec<(String, String)>)> {
        use secret_service::blocking::SecretService;
        use secret_service::EncryptionType;
        let ss = SecretService::connect(EncryptionType::Dh).unwrap();
        let col = ss.get_default_collection().unwrap();
        let found = col.search_items(q.iter().copied().collect()).unwrap();
        found.iter().map(|i| (i.get_label().unwrap(), i.get_attributes().unwrap().into_iter().collect())).collect()
    }

    #[test]
    fn delete_answers_whether_it_was_there() {
        child("delete_answers_whether_it_was_there_inner", false);
    }

    #[test]
    #[ignore = "runs in a child under its own keyring"]
    fn delete_answers_whether_it_was_there_inner() {
        wait_ready();
        let t = "TermurionAI/Gemini";
        write_generic(t, "me", &Secret::new("k".into()), "c", "API key").unwrap();
        assert_eq!(delete_generic(t), Ok(true));
        assert_eq!(delete_generic(t), Ok(false));
        assert!(read_generic(t, "API key").unwrap().is_none());
    }

    #[test]
    fn the_password_manager_lists_only_termurion_items() {
        child("the_password_manager_lists_only_termurion_items_inner", false);
    }

    #[test]
    #[ignore = "runs in a child under its own keyring"]
    fn the_password_manager_lists_only_termurion_items_inner() {
        use secret_service::blocking::SecretService;
        use secret_service::EncryptionType;
        wait_ready();
        let mine = target_name_in(Scope::Terminal, "site", "bob");
        write_generic(&mine, "bob", &Secret::new("pw".into()), "c", "password").unwrap();
        // Another program's item that even borrows our target text.
        let ss = SecretService::connect(EncryptionType::Dh).unwrap();
        let col = ss.get_default_collection().unwrap();
        let foreign = target_name_in(Scope::Terminal, "stolen", "x");
        col.create_item("other app", [("application", "other"), ("target", foreign.as_str())].into_iter().collect(), b"x", false, "text/plain")
            .unwrap();
        col.create_item("no app", [("target", foreign.as_str())].into_iter().collect(), b"x", false, "text/plain").unwrap();
        let ai = "TermurionAI/OpenAI";
        write_generic(ai, "me", &Secret::new("k".into()), "c", "API key").unwrap();
        let names: Vec<String> = CredentialManager::new().accounts().unwrap().into_iter().map(|a| a.account_name).collect();
        assert_eq!(names, ["site"], "only Termurion's own password manager items: {names:?}");
        assert!(CredentialManager::for_scope(Scope::Browser).accounts().unwrap().is_empty());
    }

    #[test]
    fn add_edit_delete_round_trip() {
        child("add_edit_delete_round_trip_inner", false);
    }

    #[test]
    #[ignore = "runs in a child under its own keyring"]
    fn add_edit_delete_round_trip_inner() {
        wait_ready();
        let mut cm = CredentialManager::new();
        assert!(cm.available().is_ok());
        let a = cm.add("bank", "ann", Secret::new("hunter2".into()), &[]).unwrap();
        assert_eq!(a.account_name, "bank");
        assert_eq!(a.user_name, "ann");
        assert_eq!(cm.password(&a).unwrap().0.expose(), "hunter2");
        cm.edit_in_place(&a, None, Some("bea"), None).unwrap();
        let listed = cm.accounts().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].user_name, "bea");
        assert_eq!(cm.password(&listed[0]).unwrap().0.expose(), "hunter2", "the password stays through a rename");
        cm.set_password(&listed[0], Secret::new("new".into())).unwrap();
        assert_eq!(cm.password(&listed[0]).unwrap().0.expose(), "new");
        cm.delete(&listed[0]).unwrap();
        assert!(cm.accounts().unwrap().is_empty());
        assert!(cm.delete(&listed[0]).is_err());
        let c = cm.caps();
        assert!(c.can_edit_password && !c.generated_only && cm.name() == "Secret Service");
    }

    #[test]
    fn foreign_targets_are_refused_before_the_bus() {
        let refused = "Not a Termurion credential.".to_string();
        assert_eq!(write_generic("iTerm2", "u", &Secret::new("k".into()), "c", "password"), Err(refused.clone()));
        assert_eq!(read_generic("iTerm2", "password").err(), Some(refused.clone()));
        assert_eq!(delete_generic("iTerm2"), Err(refused));
    }
}
