//! The Secret Service (GNOME Keyring, KeePassXC, KWallet's bridge) as the password manager's store
//! and the AI keys' store. An item has the label `Termurion: <target>` and the attributes
//! `application = termurion`, `target = <Credential Manager target>`, `user = <user>`; the password
//! manager lists by `application = termurion`, so another program's item is never read or touched.
//!
//! Every call runs on a worker thread and the caller waits at most 5 s. A call that is stuck in an
//! unlock prompt answers "locked" and later calls answer it at once while the prompt is open. A
//! create or delete the service asks a person about is dismissed when the caller gives up, so a
//! prompt the caller gave up on never makes the change later; calls meanwhile answer at once too. A call
//! that is stuck anywhere else (a hung service) trips a latch: later calls answer "did not
//! answer" at once until a background probe gets an answer. Every bus call has a 5 s bound and a
//! prompt left open is let go after `PROMPT_LIFE`, so no worker thread lives on for good. A
//! missing service is an error naming it, never "nothing stored". No secret is ever logged.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use secret_service::blocking::{Collection, SecretService};
use secret_service::{EncryptionType, Error as SsError};
use zbus::blocking::Connection;
use zbus::zvariant::OwnedObjectPath;

use crate::platform::credentials::guard;
use crate::pwm::{Account, Secret};
use crate::pwm_source::{self, Caps, Kind, Scope, Source};

const APP_VALUE: &str = "termurion";
const BOUND: Duration = Duration::from_secs(5);
/// How long a call waits on an unlock prompt before it answers "locked" and leaves the prompt open.
const PROMPT_WAIT: Duration = Duration::from_secs(3);
const LOCKED: &str = "The Secret Service is locked; unlock it and try again";
const NO_ANSWER: &str = "The Secret Service did not answer in 5 seconds.";
const UTF8: &str = "text/plain";
const BUS_NAME: &str = "org.freedesktop.secrets";
const PROMPT_IFACE: &str = "org.freedesktop.Secret.Prompt";
/// How long a prompt nobody answers keeps its worker; the connection is closed after it.
const PROMPT_LIFE: Duration = Duration::from_secs(120);
const BUSY: &str = "The Secret Service is still closing an earlier confirmation; try again";
const NOT_MADE: &str = "The Secret Service did not finish the change in time, so it may not have been made; try again";

// What a worker is doing, so a timeout can tell a prompt from a hang.
const CONNECT: u8 = 0;
const UNLOCK: u8 = 1;
const MUTATE: u8 = 2;
const READ: u8 = 3;

/// Whether calls may go to the service.
enum Gate {
    Open,
    /// A person is looking at a prompt that call `id` raised: an unlock, or (`unlock` false) a
    /// create or delete being dismissed.
    Prompting { id: u64, owner: Option<String>, unlock: bool },
    /// Call `id` hung. A probe started at `since` clears it when the service answers.
    Down { id: u64, probing: bool, since: Instant },
}

static GATE: Mutex<Gate> = Mutex::new(Gate::Open);
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn gate() -> std::sync::MutexGuard<'static, Gate> {
    GATE.lock().unwrap_or_else(|e| e.into_inner())
}

fn describe(e: SsError) -> String {
    match e {
        SsError::Unavailable => "The Secret Service is not available: no service is running on the session bus.".to_string(),
        SsError::Locked => LOCKED.to_string(),
        SsError::Prompt | SsError::PromptDisconnected => "The Secret Service unlock was dismissed; unlock it and try again".to_string(),
        SsError::NoResult => "The Secret Service has no default keyring.".to_string(),
        other => format!("The Secret Service could not be used: {other}"),
    }
}

/// Runs `f` on its own thread and gives up after `limit`; the thread is left to finish by itself.
fn within<T: Send + 'static>(limit: Duration, f: impl FnOnce() -> Option<T> + Send + 'static) -> Option<T> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(f());
    });
    rx.recv_timeout(limit).ok().flatten()
}

/// A session bus connection whose every call gives up after `limit`, so no thread waits on it for good.
fn session(limit: Duration) -> zbus::Result<Connection> {
    zbus::blocking::connection::Builder::session()?.method_timeout(limit).build()
}

/// The service's bus owner and whether its default keyring is locked; None when it cannot be asked.
fn look() -> Option<(Option<String>, bool)> {
    let limit = Duration::from_secs(1);
    within(limit, move || {
        let c = session(limit).ok()?;
        let owner = bus_owner(&c);
        let ss = SecretService::connect_with_existing(EncryptionType::Plain, c).ok()?;
        let locked = ss.get_default_collection().ok()?.is_locked().ok()?;
        Some((owner, locked))
    })
}

fn bus_owner(c: &Connection) -> Option<String> {
    let p = zbus::blocking::fdo::DBusProxy::new(c).ok()?;
    let name = zbus::names::BusName::try_from(BUS_NAME).ok()?;
    p.get_name_owner(name).ok().map(|o| o.to_string())
}

/// Decides whether a call may go to the service now.
fn admit() -> Result<(), String> {
    admit_with(look)
}

/// `admit` with the look at the keyring passed in. An open unlock prompt answers locked while the
/// keyring stays locked under the same service; the gate opens only if it still holds the prompt
/// it looked at (another call may have set a newer state meanwhile).
fn admit_with(look: impl FnOnce() -> Option<(Option<String>, bool)>) -> Result<(), String> {
    let mut g = gate();
    match &mut *g {
        Gate::Open => Ok(()),
        // A create or delete is being dismissed: its worker opens the gate when it ends.
        Gate::Prompting { unlock: false, .. } => Err(BUSY.to_string()),
        Gate::Prompting { owner, id, .. } => {
            let (was, seen) = (owner.clone(), *id);
            drop(g);
            if matches!(look(), Some((now, true)) if now == was) {
                return Err(LOCKED.to_string());
            }
            let mut g = gate();
            match &*g {
                Gate::Prompting { id, .. } if *id == seen => {
                    *g = Gate::Open;
                    Ok(())
                }
                Gate::Open => Ok(()),
                Gate::Prompting { .. } => Err(LOCKED.to_string()),
                Gate::Down { .. } => Err(NO_ANSWER.to_string()),
            }
        }
        Gate::Down { probing, since, id } => {
            let id = *id;
            if !*probing || since.elapsed() >= Duration::from_secs(30) {
                *probing = true;
                *since = Instant::now();
                std::thread::spawn(move || probe(id));
            }
            Err(NO_ANSWER.to_string())
        }
    }
}

/// Asks a service that stopped answering whether it is back. Any answer (even an error) opens the
/// gate: the next call then reports what is really wrong. No answer leaves it down.
fn probe(id: u64) {
    let _ = session(BOUND).map_err(SsError::from).and_then(|c| SecretService::connect_with_existing(EncryptionType::Plain, c)).and_then(|ss| ss.get_default_collection().map(|_| ()));
    let mut g = gate();
    if matches!(&*g, Gate::Down { id: d, .. } if *d == id) {
        *g = Gate::Open;
    }
}

/// A finished worker lets go of the gate it holds, and says it is done under the gate's lock, so a
/// caller that gives up at the same moment never holds the gate for a worker that has ended.
struct Settle {
    id: u64,
    done: Arc<AtomicBool>,
}

impl Drop for Settle {
    fn drop(&mut self) {
        let mut g = gate();
        self.done.store(true, Ordering::SeqCst);
        let mine = match &*g {
            Gate::Prompting { id, .. } | Gate::Down { id, .. } => *id == self.id,
            Gate::Open => false,
        };
        if mine {
            *g = Gate::Open;
        }
    }
}

/// The caller gave up on its worker: the gate holds it as prompting or hung, unless the worker
/// already ended. False when it had ended (its answer is waiting).
fn hold(done: &AtomicBool, gate_then: Gate) -> bool {
    let mut g = gate();
    if done.load(Ordering::SeqCst) {
        return false;
    }
    *g = gate_then;
    true
}

/// What a worker's connection learned: the service's owner and the prompt a create or delete
/// opened, so a caller that gives up can dismiss it.
#[derive(Default)]
struct Watch {
    owner: Option<String>,
    prompt: Option<OwnedObjectPath>,
    conn: Option<Connection>,
    /// The caller gave up on a create or delete: a prompt seen from now on is dismissed at once.
    dismiss: bool,
}

/// The prompt path in a reply of CreateItem `(item, prompt)`, Delete `(prompt)` or Unlock
/// `(unlocked, prompt)`; "/" means no prompt.
fn prompt_in(msg: &zbus::Message) -> Option<OwnedObjectPath> {
    let body = msg.body();
    let path = body
        .deserialize::<(OwnedObjectPath, OwnedObjectPath)>()
        .map(|b| b.1)
        .or_else(|_| body.deserialize::<(Vec<OwnedObjectPath>, OwnedObjectPath)>().map(|b| b.1))
        .or_else(|_| body.deserialize::<OwnedObjectPath>())
        .ok()?;
    // A collection or item path (ReadAlias answers with one) is not a prompt.
    (path.as_str() != "/" && path.as_str().contains("/prompt/")).then_some(path)
}

/// Dismisses the prompt on its own thread (the call is bounded by the connection's timeout).
fn dismiss(c: Connection, prompt: OwnedObjectPath) {
    std::thread::spawn(move || {
        let _ = c.call_method(Some(BUS_NAME), prompt.as_str(), Some(PROMPT_IFACE), "Dismiss", &());
    });
}

/// Reads the replies on a worker's connection for the prompt a create or delete opens. It ends
/// when the connection closes.
fn watch_prompts(c: &Connection, watch: Arc<Mutex<Watch>>) -> zbus::Result<()> {
    let rule = zbus::MatchRule::builder().msg_type(zbus::message::Type::MethodReturn).build();
    let replies = zbus::blocking::MessageIterator::for_match_rule(rule, c, Some(64))?;
    std::thread::spawn(move || {
        for msg in replies {
            let Ok(msg) = msg else { break };
            let Some(p) = prompt_in(&msg) else { continue };
            let mut w = watch.lock().unwrap_or_else(|e| e.into_inner());
            if w.dismiss {
                if let Some(c) = w.conn.clone() {
                    dismiss(c, p);
                }
            } else {
                w.prompt = Some(p);
            }
        }
    });
    Ok(())
}

/// What a worker tells the keyring body: it is about to change something (and may be prompted).
struct Run<'a> {
    col: &'a Collection<'a>,
    phase: &'a AtomicU8,
}

impl Run<'_> {
    fn mutate(&self) {
        self.phase.store(MUTATE, Ordering::SeqCst);
    }
}

/// Connects, finds the default keyring, unlocks it and runs `f` there. The caller waits at most 5 s;
/// a worker stuck in an unlock prompt keeps the gate "locked", a create or delete prompt is
/// dismissed, and one stuck anywhere else trips the latch.
fn with_keyring<T: Send + 'static>(f: impl FnOnce(&Run) -> Result<T, String> + Send + 'static) -> Result<T, String> {
    admit()?;
    let id = NEXT_ID.fetch_add(1, Ordering::SeqCst);
    let phase = Arc::new(AtomicU8::new(CONNECT));
    let abandoned = Arc::new(AtomicBool::new(false));
    let done = Arc::new(AtomicBool::new(false));
    let watch = Arc::new(Mutex::new(Watch::default()));
    let (tx, rx) = mpsc::channel();
    {
        let (phase, abandoned, done, watch) = (phase.clone(), abandoned.clone(), done.clone(), watch.clone());
        std::thread::spawn(move || {
            let _settle = Settle { id, done };
            let r = (|| {
                let c = session(BOUND).map_err(|e| describe(e.into()))?;
                let owner = bus_owner(&c);
                {
                    let mut w = watch.lock().unwrap_or_else(|e| e.into_inner());
                    w.owner = owner;
                    w.conn = Some(c.clone());
                }
                watch_prompts(&c, watch.clone()).map_err(|e| describe(e.into()))?;
                let r = (|| {
                    let ss = SecretService::connect_with_existing(EncryptionType::Dh, c.clone()).map_err(describe)?;
                    let col = ss.get_default_collection().map_err(describe)?;
                    if col.is_locked().map_err(describe)? {
                        phase.store(UNLOCK, Ordering::SeqCst);
                        col.unlock().map_err(describe)?;
                        // Unlocked after the caller left: the caller was told "locked", so do nothing.
                        if abandoned.load(Ordering::SeqCst) {
                            return Err(LOCKED.to_string());
                        }
                    }
                    phase.store(READ, Ordering::SeqCst);
                    f(&Run { col: &col, phase: &phase })
                })();
                // Ends the reply watcher, and with it every thread of this call.
                watch.lock().unwrap_or_else(|e| e.into_inner()).conn = None;
                let _ = c.close();
                r
            })();
            let _ = tx.send(r);
        });
    }
    let start = Instant::now();
    loop {
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(r) => return r,
            Err(mpsc::RecvTimeoutError::Disconnected) => return Err("The Secret Service call failed.".to_string()),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        let (waited, ph) = (start.elapsed(), phase.load(Ordering::SeqCst));
        let prompting = ph == UNLOCK || ph == MUTATE;
        if waited >= BOUND || (prompting && waited >= PROMPT_WAIT) {
            abandoned.store(true, Ordering::SeqCst);
            let (owner, conn) = {
                let mut w = watch.lock().unwrap_or_else(|e| e.into_inner());
                if ph == MUTATE {
                    // The caller is told the change may not have been made; a prompt must not make it later.
                    w.dismiss = true;
                    if let (Some(c), Some(p)) = (w.conn.clone(), w.prompt.take()) {
                        dismiss(c, p);
                    }
                }
                (w.owner.clone(), w.conn.clone())
            };
            let then = if prompting { Gate::Prompting { id, owner, unlock: ph == UNLOCK } } else { Gate::Down { id, probing: false, since: Instant::now() } };
            if !hold(&done, then) {
                // The worker ended just now: its answer is the one to give.
                return rx.recv().unwrap_or_else(|_| Err("The Secret Service call failed.".to_string()));
            }
            // A prompt nobody answers lets go of its worker after a while.
            if let Some(c) = conn {
                std::thread::spawn(move || {
                    std::thread::sleep(PROMPT_LIFE);
                    let _ = c.close();
                });
            }
            return Err(if ph == MUTATE { NOT_MADE } else if prompting { LOCKED } else { NO_ANSWER }.to_string());
        }
    }
}

fn attrs<'a>(target: &'a str) -> HashMap<&'a str, &'a str> {
    HashMap::from([("application", APP_VALUE), ("target", target)])
}

fn find<'a>(col: &'a Collection, target: &str) -> Result<Vec<secret_service::blocking::Item<'a>>, String> {
    col.search_items(attrs(target)).map_err(describe)
}

/// Deletes every item of the target, whatever its user; true when there was one.
fn delete_all(run: &Run, target: &str) -> Result<bool, String> {
    let items = find(run.col, target)?;
    run.mutate();
    for i in &items {
        i.delete().map_err(describe)?;
    }
    Ok(!items.is_empty())
}

/// Writes (creates or replaces) the item for `target`. The new item is created first, replacing one
/// of the same user; items left under another user go after, so a failure never leaves the target
/// empty. The comment has no attribute (the three are fixed); `what` is unused.
pub(crate) fn write_generic(target: &str, user: &str, secret: &Secret, _comment: &str, _what: &str) -> Result<(), String> {
    guard(target)?;
    let (t, u, s) = (target.to_string(), user.to_string(), secret.duplicate());
    with_keyring(move |run| {
        let a = HashMap::from([("application", APP_VALUE), ("target", t.as_str()), ("user", u.as_str())]);
        run.mutate();
        let made = run.col.create_item(&format!("Termurion: {t}"), a, s.expose().as_bytes(), true, UTF8).map_err(describe)?;
        for old in find(run.col, &t)? {
            if old.item_path != made.item_path {
                old.delete().map_err(describe)?;
            }
        }
        Ok(())
    })
}

/// Reads the item's secret as UTF-8 text; `Ok(None)` when there is no such item. `what` names the
/// secret in the not-UTF-8 text ("password", "API key").
pub(crate) fn read_generic(target: &str, what: &str) -> Result<Option<Secret>, String> {
    guard(target)?;
    let (t, what) = (target.to_string(), what.to_string());
    with_keyring(move |run| {
        let items = find(run.col, &t)?;
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
    with_keyring(move |run| delete_all(run, &t))
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
        with_keyring(move |run| {
            // Attributes only: no secret is fetched.
            let items = run.col.search_items(HashMap::from([("application", APP_VALUE)])).map_err(describe)?;
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
        with_keyring(move |run| {
            if new != old && !find(run.col, &new)?.is_empty() {
                return Err("An account with that name already exists.".to_string());
            }
            let items = find(run.col, &old)?;
            let Some(item) = items.first() else { return Err(not_found()) };
            let a = HashMap::from([("application", APP_VALUE), ("target", new.as_str()), ("user", new_user.as_str())]);
            run.mutate();
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

    const MARKER: &str = "TERMURION_SS_SANDBOX";

    /// m7: a change past its wait may still be made by a slow service, so the person is not told
    /// it was not made.
    #[test]
    fn a_slow_change_may_still_have_been_made() {
        assert!(NOT_MADE.contains("may not have been made"), "{NOT_MADE}");
    }

    /// How a child test gets its bus.
    #[derive(Clone, Copy, PartialEq)]
    enum Bus {
        /// An address with nothing on it.
        Dead,
        /// A private bus with its own unlocked keyring daemon.
        Keyring,
        /// A private bus that has no service files, so nothing can answer for the Secret Service.
        Bare,
    }

    /// Removes the scratch directory even when the test panics.
    struct Scratch(std::path::PathBuf);
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Runs the ignored test `inner` in a child with its own bus, keyring and scratch HOME, so nothing
    /// here can reach the real keyring and no env var of this process moves. The child gets a marker
    /// the inner bodies check (`sandboxed`), so they refuse to run anywhere else.
    fn child(inner: &str, bus: Bus) {
        let dir = std::env::temp_dir().join(format!("termurion-ss-{}-{}", std::process::id(), inner));
        let _ = std::fs::remove_dir_all(&dir);
        let _scratch = Scratch(dir.clone());
        let run = dir.join("run");
        std::fs::create_dir_all(&run).unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&run, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let exe = std::env::current_exe().unwrap();
        let name = format!("platform::linux::secrets::tests::{inner}");
        let mut cmd;
        match bus {
            Bus::Dead => {
                cmd = Command::new(&exe);
                cmd.env("DBUS_SESSION_BUS_ADDRESS", format!("unix:path={}", dir.join("none").display()));
                cmd.args(["--ignored", "--exact", &name, "--test-threads=1", "--nocapture"]);
            }
            Bus::Keyring | Bus::Bare => {
                cmd = Command::new("dbus-run-session");
                if bus == Bus::Bare {
                    let conf = dir.join("bare.conf");
                    std::fs::write(
                        &conf,
                        format!(
                            "<busconfig><type>session</type><listen>unix:tmpdir=/tmp</listen><auth>EXTERNAL</auth>\
                             <policy context=\"default\"><allow send_destination=\"*\" eavesdrop=\"true\"/>\
                             <allow eavesdrop=\"true\"/><allow own=\"*\"/></policy></busconfig>",
                        ),
                    )
                    .unwrap();
                    cmd.arg(format!("--config-file={}", conf.display()));
                }
                let daemon = if bus == Bus::Keyring {
                    "echo '' | gnome-keyring-daemon --foreground --unlock --components=secrets >/dev/null 2>&1 &\n\
                     export GKD_PID=$!; trap 'kill $GKD_PID 2>/dev/null' EXIT\n"
                } else {
                    ""
                };
                cmd.args(["--", "bash", "-c"]).arg(format!("{daemon}\"$0\" --ignored --exact \"$1\" --test-threads=1 --nocapture"));
                cmd.arg(&exe).arg(&name);
            }
        }
        let out = cmd
            .env(MARKER, &dir)
            .env("HOME", &dir)
            .env("XDG_DATA_HOME", dir.join("data"))
            .env("XDG_CONFIG_HOME", dir.join("config"))
            .env("XDG_RUNTIME_DIR", &run)
            .env_remove("GNOME_KEYRING_CONTROL")
            .env_remove("SSH_AUTH_SOCK")
            .env_remove("DISPLAY")
            .env_remove("WAYLAND_DISPLAY")
            .env_remove("WAYLAND_SOCKET")
            .env_remove("XAUTHORITY")
            .stdin(Stdio::null())
            .output()
            .expect("run the child test");
        let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        assert!(out.status.success() && text.contains("1 passed"), "child {inner} failed:\n{text}");
    }

    /// First line of every inner body: refuses to run unless the wrapper made the sandbox, so
    /// `cargo test -- --ignored` in a normal session never touches the real keyring.
    fn sandboxed() {
        let marker = std::env::var(MARKER).unwrap_or_else(|_| panic!("run through the wrapper test, not directly: no sandbox marker"));
        assert_eq!(std::env::var("HOME").ok().as_deref(), Some(marker.as_str()), "HOME is not the sandbox: run through the wrapper test");
    }

    /// Waits for the keyring daemon to own the bus name and have an unlocked default keyring.
    fn wait_ready() {
        for _ in 0..100 {
            if CredentialManager::new().accounts().is_ok() {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        panic!("the test keyring never came up: {:?}", CredentialManager::new().accounts().err());
    }

    /// The gate tests take turns: the gate is one for the process. None of them talks to a bus.
    static GATE_TESTS: Mutex<()> = Mutex::new(());

    fn gate_test() -> std::sync::MutexGuard<'static, ()> {
        let g = GATE_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        *gate() = Gate::Open;
        g
    }

    fn prompting(id: u64, unlock: bool) -> Gate {
        Gate::Prompting { id, owner: Some(":1.5".into()), unlock }
    }

    /// A dismiss that lands as the caller gives up at the 3 s mark: the worker ended first, so the
    /// gate is not left prompting for it (the next call may ask again).
    #[test]
    fn a_worker_that_ended_as_its_caller_gave_up_leaves_the_gate_open() {
        let _g = gate_test();
        let done = AtomicBool::new(false);
        drop(Settle { id: 7, done: Arc::new(AtomicBool::new(false)) });
        done.store(true, Ordering::SeqCst);
        assert!(!hold(&done, prompting(7, true)), "the ended worker's answer is given instead");
        assert!(matches!(*gate(), Gate::Open));
        let live = AtomicBool::new(false);
        assert!(hold(&live, prompting(8, true)));
        assert!(matches!(*gate(), Gate::Prompting { id: 8, .. }));
        *gate() = Gate::Open;
    }

    /// A create or delete prompt being dismissed answers later calls at once, with no look at the
    /// service and no second prompt.
    #[test]
    fn a_change_prompt_being_dismissed_answers_at_once() {
        let _g = gate_test();
        *gate() = prompting(3, false);
        let e = admit_with(|| panic!("no look at the service")).unwrap_err();
        assert_eq!(e, BUSY);
        // Its worker ends: the gate opens.
        drop(Settle { id: 3, done: Arc::new(AtomicBool::new(false)) });
        assert!(admit_with(|| None).is_ok());
    }

    /// An unlock prompt that went away opens the gate only if the gate still holds that prompt;
    /// a newer state another call set meanwhile is kept.
    #[test]
    fn an_unlock_prompt_opens_only_the_gate_it_looked_at() {
        let _g = gate_test();
        *gate() = prompting(1, true);
        assert_eq!(admit_with(|| Some((Some(":1.5".into()), true))).unwrap_err(), LOCKED, "still locked, same service");
        let e = admit_with(|| {
            *gate() = prompting(2, true);
            None
        })
        .unwrap_err();
        assert_eq!(e, LOCKED);
        assert!(matches!(*gate(), Gate::Prompting { id: 2, .. }), "the newer prompt is kept");
        assert!(admit_with(|| Some((Some(":1.9".into()), true))).is_ok(), "another service: open");
        assert!(matches!(*gate(), Gate::Open));
    }

    /// The prompt a create, a delete or an unlock opens is read from its reply; "/" and the
    /// collection ReadAlias names are not prompts.
    #[test]
    fn the_prompt_of_a_change_is_read_from_its_reply() {
        let call = zbus::Message::method_call("/org/freedesktop/secrets", "CreateItem").unwrap().build(&()).unwrap();
        let reply = |body: &dyn Fn(zbus::message::Builder<'_>) -> zbus::Message| body(zbus::Message::method_return(&call.header()).unwrap());
        let p = |s: &str| OwnedObjectPath::try_from(s).unwrap();
        let made = reply(&|b| b.build(&(p("/"), p("/org/freedesktop/secrets/prompt/u3"))).unwrap());
        assert_eq!(prompt_in(&made).map(|o| o.to_string()).as_deref(), Some("/org/freedesktop/secrets/prompt/u3"));
        let none = reply(&|b| b.build(&(p("/org/freedesktop/secrets/collection/login/1"), p("/"))).unwrap());
        assert_eq!(prompt_in(&none), None);
        let deleted = reply(&|b| b.build(&(p("/org/freedesktop/secrets/prompt/p9"),)).unwrap());
        assert!(prompt_in(&deleted).is_some());
        let alias = reply(&|b| b.build(&(p("/org/freedesktop/secrets/collection/login"),)).unwrap());
        assert_eq!(prompt_in(&alias), None);
        let unlock = reply(&|b| b.build(&(Vec::<OwnedObjectPath>::new(), p("/org/freedesktop/secrets/prompt/u4"))).unwrap());
        assert!(prompt_in(&unlock).is_some());
    }

    #[test]
    fn the_inner_bodies_refuse_to_run_outside_the_sandbox() {
        // Run the body of an inner test directly (as `--ignored` in a normal shell would), with the
        // marker removed from the child only. It must panic before any bus call.
        let exe = std::env::current_exe().unwrap();
        let dir = std::env::temp_dir().join(format!("termurion-ss-guard-{}", std::process::id()));
        let _scratch = Scratch(dir.clone());
        std::fs::create_dir_all(&dir).unwrap();
        for inner in ["a_written_key_reads_back_inner", "delete_answers_whether_it_was_there_inner", "add_edit_delete_round_trip_inner"] {
            let out = Command::new(&exe)
                .args(["--ignored", "--exact", &format!("platform::linux::secrets::tests::{inner}"), "--test-threads=1"])
                .env_remove(MARKER)
                // Nothing to reach even if the guard were missing.
                .env("DBUS_SESSION_BUS_ADDRESS", format!("unix:path={}", dir.join("none").display()))
                .env("HOME", &dir)
                .output()
                .unwrap();
            let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
            assert!(!out.status.success() && text.contains("no sandbox marker"), "{inner} ran without the sandbox:\n{text}");
        }
    }

    #[test]
    fn a_missing_secret_service_is_an_error_not_empty() {
        child("a_missing_secret_service_is_an_error_not_empty_inner", Bus::Dead);
    }

    #[test]
    #[ignore = "runs in a child under a dead bus"]
    fn a_missing_secret_service_is_an_error_not_empty_inner() {
        sandboxed();
        let e = read_generic("TermurionAI/OpenAI", "API key").unwrap_err();
        assert!(e.contains("Secret Service"), "{e}");
        let e = CredentialManager::new().available().unwrap_err();
        assert!(e.contains("Secret Service"), "{e}");
        assert!(write_generic("TermurionAI/OpenAI", "u", &Secret::new("k".into()), "c", "API key").unwrap_err().contains("Secret Service"));
        assert!(delete_generic("TermurionAI/OpenAI").unwrap_err().contains("Secret Service"));
    }

    #[test]
    fn a_live_bus_with_no_secret_service_is_an_error_naming_it() {
        child("a_live_bus_with_no_secret_service_is_an_error_naming_it_inner", Bus::Bare);
    }

    #[test]
    #[ignore = "runs in a child under a bus with no services"]
    fn a_live_bus_with_no_secret_service_is_an_error_naming_it_inner() {
        sandboxed();
        let e = read_generic("TermurionAI/OpenAI", "API key").unwrap_err();
        assert!(e.contains("Secret Service"), "{e}");
        assert!(!e.contains("did not answer"), "an absent service is not a hang: {e}");
        assert!(CredentialManager::new().available().unwrap_err().contains("Secret Service"));
        assert!(CredentialManager::new().accounts().is_err());
    }

    #[test]
    fn a_written_key_reads_back() {
        child("a_written_key_reads_back_inner", Bus::Keyring);
    }

    #[test]
    #[ignore = "runs in a child under its own keyring"]
    fn a_written_key_reads_back_inner() {
        sandboxed();
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
        let mut got: Vec<(String, String)> = attrs.iter().filter(|(k, _)| k != "xdg:schema").cloned().collect();
        got.sort();
        assert_eq!(got, want);
        // Another user: still one item, and the old one is gone only because the new one exists.
        write_generic(t, "you", &Secret::new("sk-three".into()), "c", "API key").unwrap();
        let items = items_with(&[("application", "termurion"), ("target", t)]);
        assert_eq!(items.len(), 1);
        assert_eq!(read_generic(t, "API key").unwrap().unwrap().expose(), "sk-three");
        assert!(read_generic("iTerm2", "password").is_err(), "a foreign target is refused");
    }

    /// The items matching some attributes, as (label, attributes), read straight from the service.
    fn items_with(q: &[(&str, &str)]) -> Vec<(String, Vec<(String, String)>)> {
        let ss = SecretService::connect(EncryptionType::Dh).unwrap();
        let col = ss.get_default_collection().unwrap();
        let found = col.search_items(q.iter().copied().collect()).unwrap();
        found.iter().map(|i| (i.get_label().unwrap(), i.get_attributes().unwrap().into_iter().collect())).collect()
    }

    #[test]
    fn delete_answers_whether_it_was_there() {
        child("delete_answers_whether_it_was_there_inner", Bus::Keyring);
    }

    #[test]
    #[ignore = "runs in a child under its own keyring"]
    fn delete_answers_whether_it_was_there_inner() {
        sandboxed();
        wait_ready();
        let t = "TermurionAI/Gemini";
        write_generic(t, "me", &Secret::new("k".into()), "c", "API key").unwrap();
        assert_eq!(delete_generic(t), Ok(true));
        assert_eq!(delete_generic(t), Ok(false));
        assert!(read_generic(t, "API key").unwrap().is_none());
    }

    #[test]
    fn the_password_manager_lists_only_termurion_items() {
        child("the_password_manager_lists_only_termurion_items_inner", Bus::Keyring);
    }

    #[test]
    #[ignore = "runs in a child under its own keyring"]
    fn the_password_manager_lists_only_termurion_items_inner() {
        sandboxed();
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
    fn delete_edit_and_write_leave_another_programs_item_alone() {
        child("delete_edit_and_write_leave_another_programs_item_alone_inner", Bus::Keyring);
    }

    #[test]
    #[ignore = "runs in a child under its own keyring"]
    fn delete_edit_and_write_leave_another_programs_item_alone_inner() {
        sandboxed();
        wait_ready();
        let mut cm = CredentialManager::new();
        let a = cm.add("shared", "ann", Secret::new("mine".into()), &[]).unwrap();
        let ss = SecretService::connect(EncryptionType::Dh).unwrap();
        let col = ss.get_default_collection().unwrap();
        // Another app's item carries the very same target text.
        col.create_item("other app", [("application", "other"), ("target", a.id.as_str())].into_iter().collect(), b"theirs", false, "text/plain")
            .unwrap();
        let theirs = || items_with(&[("application", "other"), ("target", a.id.as_str())]).len();
        assert_eq!(theirs(), 1);
        cm.edit_in_place(&a, None, None, Some(Secret::new("mine2".into()))).unwrap();
        cm.set_password(&a, Secret::new("mine3".into())).unwrap();
        assert_eq!(theirs(), 1, "edit and write leave it");
        assert_eq!(cm.password(&a).unwrap().0.expose(), "mine3");
        cm.delete(&a).unwrap();
        assert_eq!(theirs(), 1, "delete leaves it");
        assert!(cm.accounts().unwrap().is_empty());
    }

    #[test]
    fn add_edit_delete_round_trip() {
        child("add_edit_delete_round_trip_inner", Bus::Keyring);
    }

    #[test]
    #[ignore = "runs in a child under its own keyring"]
    fn add_edit_delete_round_trip_inner() {
        sandboxed();
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

    // ---- the prompt and the hang ----

    use std::sync::atomic::AtomicUsize;
    use zbus::zvariant::{ObjectPath, OwnedValue, Value};

    /// A stand-in for the desktop's prompter (`org.gnome.keyring.SystemPrompter`): it takes the
    /// daemon's prompt, says it is ready, and never answers until `dismiss` sends the refusal.
    struct Prompter {
        begun: Arc<Mutex<Option<(String, String)>>>,
        count: Arc<AtomicUsize>,
        performed: Arc<AtomicBool>,
        conn: Arc<Mutex<Option<zbus::blocking::Connection>>>,
    }

    const CALLBACK: &str = "org.gnome.keyring.internal.Prompter.Callback";

    #[zbus::interface(name = "org.gnome.keyring.internal.Prompter")]
    impl Prompter {
        fn begin_prompting(&self, callback: ObjectPath<'_>, #[zbus(header)] hdr: zbus::message::Header<'_>) {
            self.count.fetch_add(1, Ordering::SeqCst);
            self.performed.store(false, Ordering::SeqCst);
            let who = hdr.sender().map(|s| s.to_string()).unwrap_or_default();
            *self.begun.lock().unwrap() = Some((who.clone(), callback.to_string()));
            let (conn, path) = (self.conn.clone(), callback.to_string());
            // Ready for work, as the real prompter says right after it begins.
            std::thread::spawn(move || {
                let c = conn.lock().unwrap().clone();
                if let Some(c) = c {
                    let _ = c.call_method(Some(who.as_str()), path.as_str(), Some(CALLBACK), "PromptReady", &("", HashMap::<&str, Value>::new(), ""));
                }
            });
        }

        fn perform_prompt(&self, _callback: ObjectPath<'_>, _kind: &str, _props: HashMap<String, OwnedValue>, _exchange: &str) {
            self.performed.store(true, Ordering::SeqCst);
        }

        fn stop_prompting(&self, _callback: ObjectPath<'_>) {}
    }

    /// The prompter on the bus, and what it was asked.
    struct Stub {
        conn: zbus::blocking::Connection,
        begun: Arc<Mutex<Option<(String, String)>>>,
        count: Arc<AtomicUsize>,
        performed: Arc<AtomicBool>,
    }

    fn start_prompter() -> Stub {
        let begun = Arc::new(Mutex::new(None));
        let count = Arc::new(AtomicUsize::new(0));
        let performed = Arc::new(AtomicBool::new(false));
        let slot = Arc::new(Mutex::new(None));
        let conn = zbus::blocking::connection::Builder::session()
            .unwrap()
            .name("org.gnome.keyring.SystemPrompter")
            .unwrap()
            .serve_at(
                "/org/gnome/keyring/Prompter",
                Prompter { begun: begun.clone(), count: count.clone(), performed: performed.clone(), conn: slot.clone() },
            )
            .unwrap()
            .build()
            .unwrap();
        *slot.lock().unwrap() = Some(conn.clone());
        Stub { conn, begun, count, performed }
    }

    impl Stub {
        /// The person says no: the daemon's prompt ends dismissed.
        fn dismiss(&self) {
            for _ in 0..50 {
                if self.performed.load(Ordering::SeqCst) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            assert!(self.performed.load(Ordering::SeqCst), "the daemon never asked the prompter to perform the prompt");
            let (who, path) = self.begun.lock().unwrap().clone().expect("the daemon began a prompt");
            self.conn
                .call_method(Some(who.as_str()), path.as_str(), Some(CALLBACK), "PromptReady", &("no", HashMap::<&str, Value>::new(), ""))
                .unwrap();
        }
    }

    fn gate_is_open() -> bool {
        matches!(&*gate(), Gate::Open)
    }

    fn lock_the_keyring() {
        let ss = SecretService::connect(EncryptionType::Plain).unwrap();
        ss.get_default_collection().unwrap().lock().unwrap();
    }

    /// The person unlocks it another way: the daemon's own master-password call (what a login does).
    fn unlock_from_outside() {
        use zbus::zvariant::OwnedObjectPath;
        let c = zbus::blocking::Connection::session().unwrap();
        let svc = "/org/freedesktop/secrets";
        let opened = c
            .call_method(Some(BUS_NAME), svc, Some("org.freedesktop.Secret.Service"), "OpenSession", &("plain", Value::from("")))
            .unwrap();
        let (_out, session): (OwnedValue, OwnedObjectPath) = opened.body().deserialize().unwrap();
        let coll = ObjectPath::try_from("/org/freedesktop/secrets/collection/login").unwrap();
        c.call_method(
            Some(BUS_NAME),
            svc,
            Some("org.gnome.keyring.InternalUnsupportedGuiltRiddenInterface"),
            "UnlockWithMasterPassword",
            &(coll, (session, Vec::<u8>::new(), b"\n".to_vec(), "text/plain")),
        )
        .unwrap();
    }

    #[test]
    fn a_locked_keyring_answers_locked_while_the_prompt_is_open() {
        child("a_locked_keyring_answers_locked_while_the_prompt_is_open_inner", Bus::Keyring);
    }

    #[test]
    #[ignore = "runs in a child under its own keyring"]
    fn a_locked_keyring_answers_locked_while_the_prompt_is_open_inner() {
        sandboxed();
        wait_ready();
        write_generic("TermurionAI/OpenAI", "me", &Secret::new("sk-1".into()), "c", "API key").unwrap();
        let stub = start_prompter();
        lock_the_keyring();
        let t0 = Instant::now();
        assert_eq!(read_generic("TermurionAI/OpenAI", "API key").err().as_deref(), Some(LOCKED));
        assert!(t0.elapsed() < Duration::from_millis(4500), "the first call gives up on the prompt in about 3 s: {:?}", t0.elapsed());
        assert!(stub.count.load(Ordering::SeqCst) >= 1, "the daemon raised a prompt");
        // Prompt still open: answered at once, with the very same text, and no second prompt.
        let t1 = Instant::now();
        assert_eq!(delete_generic("TermurionAI/OpenAI").err().as_deref(), Some(LOCKED));
        assert_eq!(CredentialManager::new().available().err().as_deref(), Some(LOCKED));
        assert!(t1.elapsed() < Duration::from_millis(1500), "later calls do not wait: {:?}", t1.elapsed());
        assert_eq!(stub.count.load(Ordering::SeqCst), 1);
        // The person unlocks it: calls work again, and the secret was never touched.
        unlock_from_outside();
        assert_eq!(read_generic("TermurionAI/OpenAI", "API key").unwrap().unwrap().expose(), "sk-1");
        assert!(gate_is_open());
    }

    #[test]
    fn a_dismissed_prompt_lets_the_next_call_ask_again() {
        child("a_dismissed_prompt_lets_the_next_call_ask_again_inner", Bus::Keyring);
    }

    #[test]
    #[ignore = "runs in a child under its own keyring"]
    fn a_dismissed_prompt_lets_the_next_call_ask_again_inner() {
        sandboxed();
        wait_ready();
        let stub = start_prompter();
        lock_the_keyring();
        assert_eq!(CredentialManager::new().available().err().as_deref(), Some(LOCKED));
        stub.dismiss();
        for _ in 0..40 {
            if gate_is_open() {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(gate_is_open(), "the dismissal did not let go of the flag");
        // The worker came back with the dismissal and let go of the gate; the next call is a new prompt.
        assert!(stub.count.load(Ordering::SeqCst) >= 1);
        assert_eq!(CredentialManager::new().available().err().as_deref(), Some(LOCKED));
        assert_eq!(stub.count.load(Ordering::SeqCst), 2, "a new prompt, not a stuck flag");
    }

    #[test]
    fn a_daemon_that_dies_mid_prompt_does_not_leave_the_flag_set() {
        child("a_daemon_that_dies_mid_prompt_does_not_leave_the_flag_set_inner", Bus::Keyring);
    }

    #[test]
    #[ignore = "runs in a child under its own keyring"]
    fn a_daemon_that_dies_mid_prompt_does_not_leave_the_flag_set_inner() {
        sandboxed();
        wait_ready();
        let _stub = start_prompter();
        lock_the_keyring();
        assert_eq!(CredentialManager::new().available().err().as_deref(), Some(LOCKED));
        let pid: i32 = std::env::var("GKD_PID").expect("the wrapper exports the daemon pid").parse().unwrap();
        // SAFETY: a plain kill of the test's own scratch daemon.
        assert_eq!(unsafe { libc::kill(pid, libc::SIGKILL) }, 0);
        let mut last = None;
        for _ in 0..30 {
            let r = CredentialManager::new().available();
            if r.as_ref().err().map(String::as_str) != Some(LOCKED) {
                last = Some(r);
                break;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        let r = last.expect("the flag stayed set after the daemon died");
        if let Err(e) = r {
            assert!(e.contains("Secret Service"), "{e}");
        }
    }

    /// A service that takes the name and never answers a call.
    struct Hung;

    #[zbus::interface(name = "org.freedesktop.Secret.Service")]
    impl Hung {
        fn open_session(&self, _algorithm: &str, _input: Value<'_>) -> (OwnedValue, zbus::zvariant::OwnedObjectPath) {
            loop {
                std::thread::sleep(Duration::from_secs(3600));
            }
        }
    }

    #[test]
    fn a_hung_service_trips_the_latch_and_later_calls_fail_at_once() {
        child("a_hung_service_trips_the_latch_and_later_calls_fail_at_once_inner", Bus::Bare);
    }

    #[test]
    #[ignore = "runs in a child under a bus with a hung service"]
    fn a_hung_service_trips_the_latch_and_later_calls_fail_at_once_inner() {
        sandboxed();
        let _conn = zbus::blocking::connection::Builder::session()
            .unwrap()
            .name(BUS_NAME)
            .unwrap()
            .serve_at("/org/freedesktop/secrets", Hung)
            .unwrap()
            .build()
            .unwrap();
        let t0 = Instant::now();
        let first = read_generic("TermurionAI/OpenAI", "API key").unwrap_err();
        assert!(first.contains("Secret Service") && first.contains("did not answer"), "{first}");
        assert!(t0.elapsed() >= Duration::from_secs(4) && t0.elapsed() < Duration::from_secs(7), "{:?}", t0.elapsed());
        let t1 = Instant::now();
        let second = read_generic("TermurionAI/OpenAI", "API key").unwrap_err();
        assert_eq!(second, first, "the same text");
        assert!(CredentialManager::new().accounts().is_err());
        assert!(write_generic("TermurionAI/OpenAI", "u", &Secret::new("k".into()), "c", "API key").is_err());
        assert!(t1.elapsed() < Duration::from_millis(500), "the latch answers at once: {:?}", t1.elapsed());
    }

    #[test]
    fn the_latch_opens_when_a_probe_gets_an_answer() {
        child("the_latch_opens_when_a_probe_gets_an_answer_inner", Bus::Keyring);
    }

    #[test]
    #[ignore = "runs in a child under its own keyring"]
    fn the_latch_opens_when_a_probe_gets_an_answer_inner() {
        sandboxed();
        wait_ready();
        *gate() = Gate::Down { id: 0, probing: false, since: Instant::now() };
        let t0 = Instant::now();
        assert_eq!(CredentialManager::new().available().err().as_deref(), Some(NO_ANSWER));
        assert!(t0.elapsed() < Duration::from_millis(500));
        let mut opened = false;
        for _ in 0..50 {
            if CredentialManager::new().available().is_ok() {
                opened = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(opened, "the probe never opened the latch");
    }
}
