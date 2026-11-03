//! Bonjour hosts as profiles (m13t Task 9; iTerm2 `ITAddressBookMgr.m:342-353`, `:396-729`).
//! What the local network advertises is untrusted input: a name or address is cleaned and checked
//! here before a profile (and so a command line) is made from it, and nothing connects until the
//! person picks the profile. The `Browser` (under `platform::mdns`) runs `DnsServiceBrowse` and
//! `DnsServiceResolve`; everything else is pure.

use crate::profile::Profile;
use toml::Value;

/// A host a resolved Bonjour service stands for (`netServiceDidResolveAddress:`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    pub name: String,
    pub service: String,
    pub address: String,
    pub port: u16,
    pub user: Option<String>,
}

/// What the browser tells the app.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    Found(Found),
    /// A resolved service said goodbye.
    Lost(Lost),
}

/// A service that went away: the name, service and address it was found with (P5: a goodbye
/// matches name AND address; the service keeps a same-named ftp or telnet instance from
/// removing an ssh host).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Lost {
    pub name: String,
    pub service: String,
    pub address: String,
}

/// Goodbyes waiting for room in the UI queue, at most.
pub const MAX_WAITING_LOST: usize = MAX_HOSTS;

/// Events on their way to the UI thread's bounded queue. A host that does not fit is reported
/// back, so the worker forgets the instance and its next announcement resolves it again; a
/// goodbye that does not fit waits (at most `MAX_WAITING_LOST`) and is sent before anything
/// else, so a later host never overtakes the goodbye before it.
#[derive(Default)]
pub struct Outbox {
    lost: std::collections::VecDeque<Event>,
}

impl Outbox {
    /// Sends `e` through `send` (false: no room). False only for a host that was not sent.
    pub fn emit(&mut self, e: Event, send: &mut dyn FnMut(Event) -> bool) -> bool {
        self.flush(send);
        match e {
            Event::Found(_) => self.lost.is_empty() && send(e),
            Event::Lost(_) => {
                if !(self.lost.is_empty() && send(e.clone())) && self.lost.len() < MAX_WAITING_LOST {
                    self.lost.push_back(e);
                }
                true
            }
        }
    }

    /// Sends the waiting goodbyes, in order, while there is room.
    pub fn flush(&mut self, send: &mut dyn FnMut(Event) -> bool) {
        while let Some(e) = self.lost.front() {
            if !send(e.clone()) {
                return;
            }
            self.lost.pop_front();
        }
    }

    /// Whether a goodbye waits (the worker then wakes to retry).
    pub fn waiting(&self) -> bool {
        !self.lost.is_empty()
    }
}

/// The services iTerm2 browses (`:416-428`); only ssh, scp and sftp make profiles.
pub const SERVICES: [&str; 3] = ["_ssh._tcp.local", "_ftp._tcp.local", "_telnet._tcp.local"];

/// The tag every Bonjour profile carries.
pub const TAG: &str = "bonjour";
/// The key that holds the destination (display data only; transience is the store's guid set).
pub const ADDRESS_KEY: &str = "Bonjour Service Address";
/// Live Bonjour profiles at most; newer hosts are ignored (P5).
pub const MAX_HOSTS: usize = 128;
/// A user name longer than this is refused (P5).
pub const MAX_USER: usize = 64;
/// A cleaned service name is cut to this many characters (P5).
pub const MAX_NAME: usize = 128;

/// `getBookmarksType`: the service between the leading "_" and the first "." (`:442-449`).
pub fn service_type(t: &str) -> Option<String> {
    let rest = t.strip_prefix('_')?;
    let end = rest.find('.').unwrap_or(rest.len());
    (end > 0).then(|| rest[..end].to_string())
}

/// iTerm2's `usernameIsSafe:`: letters, digits and "_" only.
pub fn user_is_safe(u: &str) -> bool {
    u.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') && !u.is_empty()
}

/// The user a TXT record names: "u", else "username" (`usernameFromTXTRecord:`).
pub fn user_from_txt(pairs: &[(String, String)]) -> Option<String> {
    let find = |key: &str| pairs.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone());
    find("u").or_else(|| find("username")).filter(|u| !u.is_empty())
}

/// The profile a found host makes, or None for a service that makes none or text that is not
/// safe to put on a command line (`_addBonjourHostProfileWithName:`, `:566-630`).
pub fn profile_for(found: &Found, prototype: &Profile, ssh_path: &str) -> Option<Profile> {
    let service = found.service.as_str();
    if !matches!(service, "ssh" | "scp" | "sftp") {
        return None;
    }
    let name = crate::profile_list::display_text(&found.name).trim().to_string();
    if name.is_empty() {
        return None;
    }
    let address = safe_address(&found.address)?;
    let user = found.user.as_deref().filter(|u| user_is_safe(u) && u.len() <= MAX_USER && crate::ssh_jumps::user_is_safe(u));
    let (user_arg, destination) = match user {
        Some(u) if service == "ssh" => (format!("-l {u} "), address.clone()),
        Some(u) => (String::new(), format!("{u}@{address}")),
        None => (String::new(), address.clone()),
    };
    let port_arg = if service == "ssh" && found.port != 22 && found.port != 0 { format!("-p {} ", found.port) } else { String::new() };
    let program = if service == "ssh" && crate::ssh_url::scheme_path_usable(ssh_path) { ssh_path } else { service };
    let mut p = prototype.clone();
    for key in ["Shortcut", "Initial Text", "Session Hotkey", ADDRESS_KEY, crate::session_profile::ORIGINAL_GUID] {
        p.values.remove(key);
    }
    crate::start_dir::scrub_map(&mut p.values);
    p.set("Name", Value::String(name.clone()));
    p.set("Description", Value::String(name));
    p.set("Command", Value::String(format!("{program} {user_arg}{port_arg}{destination}")));
    p.set("Working Directory", Value::String(String::new()));
    p.set("Custom Command", Value::String("Yes".into()));
    p.set("Custom Directory", Value::String("Home".into()));
    p.set(ADDRESS_KEY, Value::String(destination));
    p.set("Tags", Value::Array(vec![Value::String(TAG.into())]));
    p.set("Guid", Value::String(uuid::Uuid::new_v4().to_string().to_uppercase()));
    p.set("Default Bookmark", Value::String("No".into()));
    Some(p)
}

/// The address text a command line may carry: an IP formatted back from its parse (never the DNS
/// text), or a host name that passes the ssh URL rule and is not digits and dots that failed to
/// parse as an IP (ssh would read "010.1.1.1" as octal). Unspecified and multicast addresses
/// are no host.
fn safe_address(text: &str) -> Option<String> {
    if let Ok(ip) = text.parse::<std::net::IpAddr>() {
        return (!ip.is_unspecified() && !ip.is_multicast()).then(|| ip.to_string());
    }
    let numeric = text.chars().all(|c| c.is_ascii_digit() || c == '.');
    (!numeric && text.len() <= 253 && crate::ssh_url::hostname_ok(text)).then(|| text.to_string())
}

/// One live Bonjour profile: its guid, the service name, service and address it came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Live {
    pub guid: String,
    pub name: String,
    pub service: String,
    pub address: String,
}

/// Whether a found host becomes a profile now: not a repeat of a live (name, address), and room
/// under `MAX_HOSTS`.
pub fn accept(live: &[Live], found: &Found) -> bool {
    live.len() < MAX_HOSTS && !live.iter().any(|l| l.name == found.name && l.address == found.address)
}

/// The guids a lost service removes: same name, service and address (P5; iTerm2 matches by name
/// alone, `:521-541`).
pub fn lost_guids(live: &[Live], lost: &Lost) -> Vec<String> {
    live.iter().filter(|l| l.name == lost.name && l.service == lost.service && l.address == lost.address).map(|l| l.guid.clone()).collect()
}

/// A profile name that is free: `base`, else "base (address)", else numbered.
pub fn unique_name(base: &str, address: &str, taken: &dyn Fn(&str) -> bool) -> String {
    if !taken(base) {
        return base.to_string();
    }
    let with_address = format!("{base} ({address})");
    if !taken(&with_address) {
        return with_address;
    }
    (2..)
        .map(|n| format!("{with_address} {n}"))
        .find(|c| !taken(c))
        .unwrap_or(with_address)
}

/// The service name of a browse answer `Name._ssh._tcp.local`: the label before `.service`
/// with DNS escapes undone, None when it does not end with the service.
pub fn instance_label(full: &str, service: &str) -> Option<String> {
    let full = full.strip_suffix('.').unwrap_or(full);
    let service = service.strip_suffix('.').unwrap_or(service);
    let cut = full.len().checked_sub(service.len() + 1)?;
    // Bytes first: a '.' at `cut` makes both `cut` and `cut + 1` character boundaries, so the
    // slice below cannot split a multi-byte character a hostile name puts there.
    let bytes = full.as_bytes();
    if bytes[cut] != b'.' || !bytes[cut + 1..].eq_ignore_ascii_case(service.as_bytes()) {
        return None;
    }
    let raw = &full[..cut];
    let mut out = String::new();
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.peek().copied() {
            Some(d) if d.is_ascii_digit() => {
                let digits: String = (0..3).filter_map(|_| chars.next_if(char::is_ascii_digit)).collect();
                out.push(char::from_u32(digits.parse::<u32>().ok()?)?);
            }
            Some(d) => {
                out.push(d);
                chars.next();
            }
            None => {}
        }
    }
    (!out.is_empty()).then_some(out)
}

/// Whether a profile is a Bonjour one by its tag (Open All skips these, P5).
pub fn is_bonjour(p: &Profile) -> bool {
    p.get("Tags").and_then(|t| t.as_array().cloned()).is_some_and(|a| a.iter().any(|t| t.as_str() == Some(TAG)))
}

/// The ids of DNS-SD requests whose last callback has run (Microsoft: `DnsServiceBrowseCancel`
/// "causes one further invocation of the browse callback, with status ERROR_CANCELLED"; a resolve
/// calls back once, on completion or cancel). The callbacks mark an id as their last act;
/// `Requests` frees a request only after its id is marked.
pub struct Ended {
    ids: std::sync::Mutex<Vec<u64>>,
    changed: std::sync::Condvar,
}

impl Default for Ended {
    fn default() -> Ended {
        Ended::new()
    }
}

impl Ended {
    pub const fn new() -> Ended {
        Ended { ids: std::sync::Mutex::new(Vec::new()), changed: std::sync::Condvar::new() }
    }

    /// Called by a callback when it will not run again for `id`.
    pub fn mark(&self, id: u64) {
        let mut ids = self.ids.lock().unwrap_or_else(|p| p.into_inner());
        ids.push(id);
        self.changed.notify_all();
    }

    fn has(&self, id: u64) -> bool {
        self.ids.lock().unwrap_or_else(|p| p.into_inner()).contains(&id)
    }

    /// Takes the marked ids `open` holds.
    fn take(&self, open: &dyn Fn(u64) -> bool) -> Vec<u64> {
        let mut ids = self.ids.lock().unwrap_or_else(|p| p.into_inner());
        let (mine, rest): (Vec<u64>, Vec<u64>) = ids.iter().partition(|id| open(**id));
        *ids = rest;
        mine
    }

    /// Waits until every id `pending` names is marked, or `deadline`.
    fn wait(&self, pending: &dyn Fn(&[u64]) -> bool, deadline: std::time::Instant) {
        let mut ids = self.ids.lock().unwrap_or_else(|p| p.into_inner());
        while pending(&ids) {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                return;
            }
            ids = self.changed.wait_timeout(ids, left).map(|(g, _)| g).unwrap_or_else(|p| p.into_inner().0);
        }
    }
}

/// What `Requests` needs from DNS-SD: cancelling a running request. `Request` owns every buffer
/// the system was handed (the request structure, its query name and the cancel handle).
pub trait DnsSd {
    type Request;
    fn cancel(&self, request: &Self::Request);
}

/// The one owner of every running DNS-SD request. A request's buffers are freed only after its
/// last callback ran (`Ended`); dropping the owner (normal stop, a failed thread spawn, or an
/// unwind) cancels every open request first, waits up to `grace` for their last callbacks, frees
/// those and leaks the rest, since the system may still hold them.
pub struct Requests<A: DnsSd> {
    api: A,
    ended: &'static Ended,
    /// Open requests and whether each was cancelled.
    open: std::collections::HashMap<u64, (A::Request, bool)>,
    grace: std::time::Duration,
}

impl<A: DnsSd> Requests<A> {
    pub fn new(api: A, ended: &'static Ended, grace: std::time::Duration) -> Requests<A> {
        Requests { api, ended, open: std::collections::HashMap::new(), grace }
    }

    /// Takes a request the system accepted (it answered DNS_REQUEST_PENDING).
    pub fn add(&mut self, id: u64, request: A::Request) {
        self.open.insert(id, (request, false));
    }

    /// Open requests, cancelled ones whose last callback has not run included.
    pub fn len(&self) -> usize {
        self.open.len()
    }

    pub fn is_empty(&self) -> bool {
        self.open.is_empty()
    }

    /// Cancels a request once; one that already ended (a completed resolve) is not cancelled.
    pub fn cancel(&mut self, id: u64) {
        if let Some((request, cancelled)) = self.open.get_mut(&id) {
            if !*cancelled && !self.ended.has(id) {
                self.api.cancel(request);
            }
            *cancelled = true;
        }
    }

    /// Frees every request whose last callback ran.
    pub fn reap(&mut self) {
        let open = &self.open;
        for id in self.ended.take(&|id| open.contains_key(&id)) {
            self.open.remove(&id);
        }
    }

    /// Frees `id` once its last callback ran (it is about to: its message arrived), waiting a
    /// moment for the mark. Returns whether it was freed.
    pub fn finish(&mut self, id: u64) -> bool {
        if !self.open.contains_key(&id) {
            // Not ours (an older Browser's late callback) or already freed.
            return true;
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(100);
        self.ended.wait(&|ids| !ids.contains(&id), deadline);
        self.reap();
        !self.open.contains_key(&id)
    }

    /// Cancels every open request, waits for their last callbacks, frees what ended and leaks
    /// the rest. Runs once (Drop calls it; the owner may call it earlier).
    pub fn shutdown(&mut self) {
        if self.open.is_empty() {
            return;
        }
        let ids: Vec<u64> = self.open.keys().copied().collect();
        for id in &ids {
            self.cancel(*id);
        }
        self.ended.wait(&|marked| ids.iter().any(|id| !marked.contains(id)), std::time::Instant::now() + self.grace);
        self.reap();
        for (_, (request, _)) in std::mem::take(&mut self.open) {
            std::mem::forget(request);
        }
    }
}

impl<A: DnsSd> Drop for Requests<A> {
    fn drop(&mut self) {
        self.shutdown();
    }
}

// Discovery lives under platform (Windows browses with the DNS-SD API, Unix says Unsupported).
pub use crate::platform::mdns::Browser;

#[cfg(test)]
mod tests {
    use super::*;

    fn found(service: &str, user: Option<&str>, port: u16) -> Found {
        Found { name: "Bob's Mac".into(), service: service.into(), address: "10.0.0.5".into(), port, user: user.map(String::from) }
    }

    fn command(f: &Found, ssh_path: &str) -> String {
        profile_for(f, &Profile::new("Default"), ssh_path).unwrap().string("Command")
    }

    #[test]
    fn bonjour_profile_follows_iterm2() {
        assert_eq!(command(&found("ssh", Some("bob"), 22), "ssh"), "ssh -l bob 10.0.0.5");
        assert_eq!(command(&found("ssh", Some("bob"), 2222), "ssh"), "ssh -l bob -p 2222 10.0.0.5");
        assert_eq!(command(&found("ssh", Some("b;ob"), 22), "ssh"), "ssh 10.0.0.5", "an unsafe user is left out");
        assert_eq!(command(&found("ssh", None, 22), "ssh"), "ssh 10.0.0.5");
        assert_eq!(command(&found("sftp", Some("bob"), 22), "ssh"), "sftp bob@10.0.0.5");
        assert_eq!(command(&found("scp", Some("bob"), 2222), "ssh"), "scp bob@10.0.0.5", "only ssh takes a port");
        assert!(profile_for(&found("ftp", None, 21), &Profile::new("Default"), "ssh").is_none());
        assert!(profile_for(&found("telnet", None, 23), &Profile::new("Default"), "ssh").is_none());
        assert_eq!(command(&found("ssh", Some("bob"), 22), "C:\\tools\\ssh.exe"), "C:\\tools\\ssh.exe -l bob 10.0.0.5");
        assert_eq!(command(&found("ssh", Some("bob"), 22), "-"), "ssh -l bob 10.0.0.5", "no letter or digit: not usable");
        assert_eq!(command(&found("sftp", None, 22), "C:\\tools\\ssh.exe"), "sftp 10.0.0.5", "the ssh path is for ssh only");

        let p = profile_for(&found("ssh", Some("bob"), 22), &Profile::new("Default"), "ssh").unwrap();
        assert_eq!(p.name(), "Bob's Mac");
        assert_eq!(p.string("Description"), "Bob's Mac");
        assert_eq!(p.string("Custom Command"), "Yes");
        assert_eq!(p.string("Working Directory"), "");
        assert_eq!(p.string("Custom Directory"), "Home");
        assert_eq!(p.string(ADDRESS_KEY), "10.0.0.5");
        assert_eq!(p.string("Default Bookmark"), "No");
        assert!(p.get("Shortcut").is_none());
        assert_eq!(p.get("Tags"), Some(Value::Array(vec![Value::String("bonjour".into())])));
        assert!(is_bonjour(&p));
        assert!(!is_bonjour(&Profile::new("x")));
        let q = profile_for(&found("ssh", Some("bob"), 22), &Profile::new("Default"), "ssh").unwrap();
        assert_ne!(p.guid(), q.guid(), "a fresh guid each time");
        let dest = profile_for(&found("sftp", Some("bob"), 22), &Profile::new("Default"), "ssh").unwrap();
        assert_eq!(dest.string(ADDRESS_KEY), "bob@10.0.0.5", "the destination");
    }

    #[test]
    fn service_helpers_follow_iterm2() {
        assert_eq!(service_type("_ssh._tcp.local").as_deref(), Some("ssh"));
        assert_eq!(service_type("_sftp-ssh._tcp.").as_deref(), Some("sftp-ssh"));
        assert_eq!(service_type("ssh._tcp"), None);
        assert_eq!(service_type("_._tcp"), None);
        assert!(user_is_safe("bob_1") && user_is_safe("B0B"));
        assert!(!user_is_safe("") && !user_is_safe("b;ob") && !user_is_safe("a b") && !user_is_safe("-x") && !user_is_safe("a.b"));
        let pair = |k: &str, v: &str| (k.to_string(), v.to_string());
        assert_eq!(user_from_txt(&[pair("username", "long"), pair("u", "short")]).as_deref(), Some("short"));
        assert_eq!(user_from_txt(&[pair("username", "long")]).as_deref(), Some("long"));
        assert_eq!(user_from_txt(&[pair("x", "y")]), None);
        assert_eq!(user_from_txt(&[pair("u", "")]), None);
    }

    #[test]
    fn instance_labels_undo_dns_escapes() {
        assert_eq!(instance_label("Bobs Mac._ssh._tcp.local", "_ssh._tcp.local").as_deref(), Some("Bobs Mac"));
        assert_eq!(instance_label("Bob\\.s\\032Mac._SSH._tcp.local.", "_ssh._tcp.local").as_deref(), Some("Bob.s Mac"));
        assert_eq!(instance_label("a\\\\b._ssh._tcp.local", "_ssh._tcp.local").as_deref(), Some("a\\b"));
        assert_eq!(instance_label("_ssh._tcp.local", "_ssh._tcp.local"), None, "no instance name");
        assert_eq!(instance_label("x._ftp._tcp.local", "_ssh._tcp.local"), None);
    }

    /// A fake DNS-SD API: no traffic, it logs each cancel; a request logs its free when dropped.
    struct FakeApi(std::sync::Arc<std::sync::Mutex<Vec<String>>>);
    struct FakeRequest(u64, std::sync::Arc<std::sync::Mutex<Vec<String>>>);
    impl Drop for FakeRequest {
        fn drop(&mut self) {
            self.1.lock().unwrap().push(format!("free {}", self.0));
        }
    }
    impl DnsSd for FakeApi {
        type Request = FakeRequest;
        fn cancel(&self, r: &FakeRequest) {
            self.0.lock().unwrap().push(format!("cancel {}", r.0));
        }
    }

    fn fake(ended: &'static Ended) -> (Requests<FakeApi>, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
        let log = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        (Requests::new(FakeApi(log.clone()), ended, std::time::Duration::from_millis(300)), log)
    }

    /// The system's side: ends a request (its last callback returns) once it was cancelled.
    fn end_after_cancel(ended: &'static Ended, log: std::sync::Arc<std::sync::Mutex<Vec<String>>>, id: u64) -> std::thread::JoinHandle<()> {
        std::thread::spawn(move || {
            for _ in 0..200 {
                if log.lock().unwrap().contains(&format!("cancel {id}")) {
                    std::thread::sleep(std::time::Duration::from_millis(20));
                    ended.mark(id);
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        })
    }

    fn lines(log: &std::sync::Arc<std::sync::Mutex<Vec<String>>>) -> Vec<String> {
        log.lock().unwrap().clone()
    }

    #[test]
    fn requests_are_freed_only_after_their_last_callback() {
        static ENDED: Ended = Ended::new();
        let (mut r, log) = fake(&ENDED);
        let h = |n: u64| FakeRequest(n, log.clone());
        r.add(1, h(1));
        r.add(2, h(2));
        r.add(3, h(3));
        // A resolve that completed on its own: freed once its callback ended, never cancelled.
        ENDED.mark(2);
        r.cancel(2);
        r.reap();
        assert_eq!(lines(&log), vec!["free 2"]);
        // A cancelled request keeps its buffers until its last (cancelled) callback.
        r.cancel(1);
        r.reap();
        assert_eq!(lines(&log), vec!["free 2", "cancel 1"]);
        r.cancel(1);
        assert_eq!(lines(&log).len(), 2, "one cancel per request");
        ENDED.mark(1);
        assert!(r.finish(1));
        assert_eq!(lines(&log), vec!["free 2", "cancel 1", "free 1"]);
        assert_eq!(r.len(), 1);
        // A message for a request it does not hold (an older Browser's) does not wait.
        let start = std::time::Instant::now();
        assert!(r.finish(99));
        assert!(start.elapsed() < std::time::Duration::from_millis(50));
        // Drop cancels what is open and waits for its end before freeing.
        let t = end_after_cancel(&ENDED, log.clone(), 3);
        drop(r);
        t.join().unwrap();
        assert_eq!(lines(&log), vec!["free 2", "cancel 1", "free 1", "cancel 3", "free 3"]);
    }

    #[test]
    fn a_request_that_never_ends_is_leaked_not_freed() {
        static ENDED: Ended = Ended::new();
        let (mut r, log) = fake(&ENDED);
        r.add(7, FakeRequest(7, log.clone()));
        let start = std::time::Instant::now();
        drop(r);
        assert!(start.elapsed() < std::time::Duration::from_secs(5), "the wait is bounded");
        assert_eq!(lines(&log), vec!["cancel 7"], "cancelled, and never freed while the system may hold it");
    }

    #[test]
    fn a_panic_still_cancels_before_freeing() {
        static ENDED: Ended = Ended::new();
        let (mut r, log) = fake(&ENDED);
        r.add(4, FakeRequest(4, log.clone()));
        r.add(5, FakeRequest(5, log.clone()));
        let t4 = end_after_cancel(&ENDED, log.clone(), 4);
        let t5 = end_after_cancel(&ENDED, log.clone(), 5);
        let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let _owner = r;
            panic!("a hostile name");
        }));
        assert!(unwound.is_err());
        t4.join().unwrap();
        t5.join().unwrap();
        let l = lines(&log);
        for id in [4, 5] {
            let cancel = l.iter().position(|x| *x == format!("cancel {id}")).expect("cancelled");
            let free = l.iter().position(|x| *x == format!("free {id}")).expect("freed");
            assert!(cancel < free, "{l:?}");
        }
    }

    #[test]
    fn instance_label_survives_multibyte_and_malformed_names() {
        // The byte before the service starts a two-byte character: the old code sliced inside it.
        assert_eq!(instance_label("X\u{e9}ssh._tcp.local", "_ssh._tcp.local"), None);
        assert_eq!(instance_label("\u{e9}._ssh._tcp.local", "_ssh._tcp.local").as_deref(), Some("\u{e9}"));
        assert_eq!(instance_label("M\u{1f600}c._ssh._tcp.local", "_ssh._tcp.local").as_deref(), Some("M\u{1f600}c"));
        assert_eq!(instance_label("\u{1f600}_ssh._tcp.local", "_ssh._tcp.local"), None);
        assert_eq!(instance_label("a._\u{e9}sh._tcp.local", "_ssh._tcp.local"), None);
        assert_eq!(instance_label("a\\", "_ssh._tcp.local"), None);
        assert_eq!(instance_label("\\999\\0001._ssh._tcp.local", "_ssh._tcp.local").as_deref(), Some("\u{3e7}\u{0}1"));
        assert_eq!(instance_label("", ""), None);
        assert_eq!(instance_label(".", "."), None);
        // Fuzz: names built from multi-byte characters, dots, escapes and digits never panic.
        let parts = ["\u{e9}", "\u{1f600}", "\u{800}", ".", "\\", "\\0", "\\32", "\\999", "_ssh", "_tcp", "local", "s", "h", "X", "1"];
        let services = ["_ssh._tcp.local", "_ftp._tcp.local", "_\u{e9}._tcp", "", "."];
        let mut seed: u64 = 0x2545_f491_4f6c_dd1d;
        for _ in 0..20_000 {
            let mut name = String::new();
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            for k in 0..(seed % 12) {
                name.push_str(parts[((seed >> (k * 4)) % parts.len() as u64) as usize]);
            }
            if seed & 1 == 0 {
                name.push_str("._ssh._tcp.local");
            }
            for s in services {
                let _ = instance_label(&name, s);
                let _ = service_type(&name);
            }
        }
    }

    #[test]
    fn hostile_advertisements_are_dropped_or_cleaned() {
        let proto = Profile::new("Default");
        let make = |name: &str, address: &str, user: Option<&str>| profile_for(&Found { name: name.into(), service: "ssh".into(), address: address.into(), port: 22, user: user.map(String::from) }, &proto, "ssh");
        // Control, bidi and zero-width characters leave the name; an empty name is refused.
        let p = make("a\u{202e}b\u{200b}c\nd", "10.0.0.5", None).unwrap();
        assert_eq!(p.name(), "abcd");
        assert!(make("\u{202e}\n", "10.0.0.5", None).is_none());
        assert_eq!(make(&"x".repeat(500), "10.0.0.5", None).unwrap().name().chars().count(), MAX_NAME);
        // The address is an IP formatted back, or a safe host name; anything else is dropped.
        assert_eq!(make("h", "010.000.000.005", None).map(|p| p.string("Command")), None, "not an IP and not a host");
        assert_eq!(make("h", "::1", None).unwrap().string("Command"), "ssh ::1");
        assert!(make("h", "-oProxyCommand=calc", None).is_none());
        assert!(make("h", "10.0.0.5 -o x", None).is_none());
        assert!(make("h", "host;calc", None).is_none());
        assert!(make("h", "0.0.0.0", None).is_none(), "an unspecified address");
        assert!(make("h", "224.0.0.251", None).is_none(), "a multicast address");
        assert_eq!(make("h", "printer.local", None).unwrap().string("Command"), "ssh printer.local");
        // A user passes the safe set and 64 characters; otherwise it is left out, not the host.
        assert_eq!(make("h", "10.0.0.5", Some(&"a".repeat(64))).unwrap().string("Command"), format!("ssh -l {} 10.0.0.5", "a".repeat(64)));
        assert_eq!(make("h", "10.0.0.5", Some(&"a".repeat(65))).unwrap().string("Command"), "ssh 10.0.0.5");
        // The prototype's typed-at-login text is not copied to a LAN host; its Shortcut is not either.
        let mut proto = Profile::new("Default");
        proto.set("Initial Text", Value::String("secret".into()));
        proto.set("Shortcut", Value::String("Q".into()));
        let p = profile_for(&found("ssh", None, 22), &proto, "ssh").unwrap();
        assert_eq!(p.string("Initial Text"), "");
        assert!(p.get("Shortcut").is_none());
    }

    #[test]
    fn a_full_ui_queue_loses_no_goodbye_and_no_host_for_good() {
        struct Queue {
            room: usize,
            sent: Vec<Event>,
        }
        impl Queue {
            fn push(&mut self, e: Event) -> bool {
                if self.room == 0 {
                    return false;
                }
                self.room -= 1;
                self.sent.push(e);
                true
            }
        }
        let host = |n: &str| Event::Found(Found { name: n.into(), service: "ssh".into(), address: "10.0.0.5".into(), port: 22, user: None });
        let gone = |n: &str| Event::Lost(Lost { name: n.into(), service: "ssh".into(), address: "10.0.0.5".into() });
        let mut q = Queue { room: 1, sent: Vec::new() };
        let mut out = Outbox::default();
        assert!(out.emit(host("a"), &mut |e| q.push(e)));
        // A host that does not fit is reported, so the worker forgets it and resolves it again.
        assert!(!out.emit(host("b"), &mut |e| q.push(e)));
        // A goodbye that does not fit waits, and holds back later hosts so the order stays.
        assert!(out.emit(gone("a"), &mut |e| q.push(e)));
        assert!(out.waiting());
        // With room for one, the waiting goodbye goes first and the host behind it is refused.
        q.room = 1;
        assert!(!out.emit(host("c"), &mut |e| q.push(e)), "behind a waiting goodbye");
        assert!(!out.waiting());
        q.room = 5;
        assert!(out.emit(host("c"), &mut |e| q.push(e)));
        assert_eq!(q.sent, vec![host("a"), gone("a"), host("c")]);
        // Waiting goodbyes are bounded.
        let mut full = Queue { room: 0, sent: Vec::new() };
        for i in 0..2 * MAX_WAITING_LOST {
            out.emit(gone(&i.to_string()), &mut |e| full.push(e));
        }
        assert_eq!(out.lost.len(), MAX_WAITING_LOST);
    }

    #[test]
    fn live_hosts_are_bounded_and_matched() {
        let f = found("ssh", None, 22);
        let live = vec![Live { guid: "g1".into(), name: f.name.clone(), service: "ssh".into(), address: f.address.clone() }];
        assert!(!accept(&live, &f), "the same name and address twice");
        assert!(accept(&live, &Found { address: "10.0.0.6".into(), ..f.clone() }), "same name, other address");
        let full: Vec<Live> = (0..MAX_HOSTS).map(|i| Live { guid: i.to_string(), name: format!("n{i}"), service: "ssh".into(), address: "10.0.0.9".into() }).collect();
        assert!(!accept(&full, &f), "at the cap");
        assert!(accept(&full[..MAX_HOSTS - 1], &f));
        let lost = |name: &str, service: &str, address: &str| Lost { name: name.into(), service: service.into(), address: address.into() };
        assert_eq!(lost_guids(&live, &lost("Bob's Mac", "ssh", "10.0.0.5")), vec!["g1".to_string()]);
        assert!(lost_guids(&live, &lost("bob's mac", "ssh", "10.0.0.5")).is_empty());
        // P5: name AND address; and a same-named ftp or telnet instance going leaves the ssh host.
        assert!(lost_guids(&live, &lost("Bob's Mac", "ssh", "10.0.0.6")).is_empty(), "another host of the same name");
        assert!(lost_guids(&live, &lost("Bob's Mac", "ftp", "10.0.0.5")).is_empty(), "the ftp service of the same host");
        let none = |_: &str| false;
        assert_eq!(unique_name("h", "10.0.0.5", &none), "h");
        assert_eq!(unique_name("h", "10.0.0.5", &|n| n == "h"), "h (10.0.0.5)");
        assert_eq!(unique_name("h", "10.0.0.5", &|n| n == "h" || n == "h (10.0.0.5)"), "h (10.0.0.5) 2");
    }
}
