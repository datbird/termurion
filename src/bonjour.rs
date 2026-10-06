//! Bonjour hosts as profiles (m13t Task 9; iTerm2 `ITAddressBookMgr.m:342-353`, `:396-729`).
//! What the local network advertises is untrusted input: a name or address is cleaned and checked
//! here before a profile (and so a command line) is made from it, and nothing connects until the
//! person picks the profile. The `#[cfg(windows)]` `Browser` runs `DnsServiceBrowse` and
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
    /// A service went away, by its (cleaned) service name.
    Lost(String),
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

/// One live Bonjour profile: its guid, the service name it came from and its address.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Live {
    pub guid: String,
    pub name: String,
    pub address: String,
}

/// Whether a found host becomes a profile now: not a repeat of a live (name, address), and room
/// under `MAX_HOSTS`.
pub fn accept(live: &[Live], found: &Found) -> bool {
    live.len() < MAX_HOSTS && !live.iter().any(|l| l.name == found.name && l.address == found.address)
}

/// The guids a lost service removes (iTerm2 matches by name, `:521-541`).
pub fn lost_guids(live: &[Live], name: &str) -> Vec<String> {
    live.iter().filter(|l| l.name == name).map(|l| l.guid.clone()).collect()
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

#[cfg(windows)]
pub use browser::Browser;

/// `DnsServiceBrowse` for the three services and `DnsServiceResolve` for each instance found
/// (`ABM:396-541`, `:676-729`). The system's callbacks only copy what they are given and send it
/// to one worker thread (never blocking, never touching the app); the worker starts and times
/// out the resolves and hands `Event`s to the UI thread through a bounded channel and `Notify`.
/// Nothing is polled: the worker sleeps until a message or the next resolve deadline.
/// Every buffer handed to the system lives in one `Requests` owner, freed only after the
/// request's last callback; no early return, failed spawn or panic can free one sooner.
#[cfg(windows)]
mod browser {
    use super::{instance_label, DnsSd, Ended, Event, Found, Requests, MAX_HOSTS, SERVICES};
    use crate::pty::Notify;
    use std::collections::HashMap;
    use std::ffi::c_void;
    use std::net::{Ipv4Addr, Ipv6Addr};
    use std::panic::{catch_unwind, AssertUnwindSafe};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender};
    use std::sync::Mutex;
    use std::thread::JoinHandle;
    use std::time::{Duration, Instant};
    use windows_sys::Win32::NetworkManagement::Dns::*;

    /// A resolve that has not answered in this long is cancelled (iTerm2: `resolveWithTimeout:5`).
    const RESOLVE_TIMEOUT: Duration = Duration::from_secs(5);
    /// Resolves in flight at most, and instance names remembered at most (a hostile network can
    /// advertise any number).
    const MAX_RESOLVING: usize = 32;
    const MAX_KNOWN: usize = 4 * MAX_HOSTS;
    /// Longest wide string, and most TXT pairs, read from a system structure.
    const MAX_WIDE: usize = 256;
    const MAX_TXT: usize = 32;
    /// Events waiting for the UI thread; a full queue drops the newest (the next announcement
    /// repeats it).
    const UI_QUEUE: usize = 256;
    /// DnsServiceBrowse and DnsServiceResolve answer this when the request is running.
    const DNS_REQUEST_PENDING: i32 = 9506;
    const STATUS_OK: u32 = 0;
    /// The status of a request's last callback after its cancel.
    const ERROR_CANCELLED: u32 = 1223;
    /// How long a stop waits for the system's last callbacks before leaking what it still holds.
    const GRACE: Duration = Duration::from_secs(2);

    enum Msg {
        /// A PTR answer of browse `id`: the full instance name, and whether it is a goodbye (TTL 0).
        Browsed { id: u64, full: String, gone: bool },
        Resolved { id: u64, found: Option<(String, u16, Vec<(String, String)>)> },
        Stop,
    }

    /// Where the system's callbacks send. Set by `start`, cleared when the `Session` that owns
    /// every request has ended them. A callback holds this lock only to clone the sender.
    static SINK: Mutex<Option<Sender<Msg>>> = Mutex::new(None);
    /// Context numbers for requests; a callback of an older Browser finds no entry.
    static NEXT_ID: AtomicU64 = AtomicU64::new(1);
    /// The requests whose last callback ran.
    static ENDED: Ended = Ended::new();

    fn send(m: Msg) {
        let tx = SINK.lock().ok().and_then(|g| g.clone());
        if let Some(tx) = tx {
            let _ = tx.send(m);
        }
    }

    /// Everything one request handed the system (Microsoft: the cancel handle "must remain valid
    /// until the query is canceled"; the request and its name are kept as long). Boxes and the
    /// name's heap buffer keep their addresses when this moves.
    enum Request {
        Browse { _request: Box<DNS_SERVICE_BROWSE_REQUEST>, _name: Vec<u16>, cancel: Box<DNS_SERVICE_CANCEL> },
        Resolve { _request: Box<DNS_SERVICE_RESOLVE_REQUEST>, _name: Vec<u16>, cancel: Box<DNS_SERVICE_CANCEL> },
    }
    // SAFETY: the system only reads these buffers; on our side only the thread that owns the
    // `Requests` (the starter, then the worker) touches them.
    unsafe impl Send for Request {}

    struct System;

    impl DnsSd for System {
        type Request = Request;
        fn cancel(&self, r: &Request) {
            // SAFETY: the handle was filled by the start call and is kept until the last callback.
            unsafe {
                match r {
                    Request::Browse { cancel, .. } => DnsServiceBrowseCancel(&**cancel),
                    Request::Resolve { cancel, .. } => DnsServiceResolveCancel(&**cancel),
                };
            }
        }
    }

    /// The requests of one Browser. Dropping it (the worker ends, unwinds, or never started)
    /// cancels every request and waits for its last callback before freeing, then clears the
    /// callbacks' sink.
    struct Session {
        requests: Requests<System>,
        /// Browse ids and the `SERVICES` index each browses.
        browses: Vec<(u64, usize)>,
    }

    impl Drop for Session {
        fn drop(&mut self) {
            self.requests.shutdown();
            *SINK.lock().unwrap_or_else(|p| p.into_inner()) = None;
        }
    }

    fn browse(service: &str, id: u64) -> Result<Request, i32> {
        let name = wide(service);
        let request = Box::new(DNS_SERVICE_BROWSE_REQUEST {
            Version: DNS_QUERY_REQUEST_VERSION1,
            InterfaceIndex: 0,
            QueryName: name.as_ptr(),
            Anonymous: DNS_SERVICE_BROWSE_REQUEST_0 { pBrowseCallback: Some(on_browse) },
            pQueryContext: id as usize as *mut c_void,
        });
        let mut cancel = Box::new(DNS_SERVICE_CANCEL { reserved: std::ptr::null_mut() });
        // SAFETY: the request, name and cancel handle go into the returned `Request`, which the
        // caller hands to `Requests` (kept until the last callback); on failure nothing runs.
        let status = unsafe { DnsServiceBrowse(&*request, &mut *cancel) };
        if status == DNS_REQUEST_PENDING {
            Ok(Request::Browse { _request: request, _name: name, cancel })
        } else {
            Err(status)
        }
    }

    fn resolve(full: &str, id: u64) -> Option<Request> {
        let mut name = wide(full);
        let request = Box::new(DNS_SERVICE_RESOLVE_REQUEST {
            Version: DNS_QUERY_REQUEST_VERSION1,
            InterfaceIndex: 0,
            QueryName: name.as_mut_ptr(),
            pResolveCompletionCallback: Some(on_resolve),
            pQueryContext: id as usize as *mut c_void,
        });
        let mut cancel = Box::new(DNS_SERVICE_CANCEL { reserved: std::ptr::null_mut() });
        // SAFETY: as in `browse`.
        let status = unsafe { DnsServiceResolve(&*request, &mut *cancel) };
        (status == DNS_REQUEST_PENDING).then(|| Request::Resolve { _request: request, _name: name, cancel })
    }

    pub struct Browser {
        events: Receiver<Event>,
        stop: Sender<Msg>,
        worker: Option<JoinHandle<()>>,
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// A NUL-terminated wide string, at most `MAX_WIDE` units, lossy.
    unsafe fn read_wide(p: *const u16) -> Option<String> {
        if p.is_null() {
            return None;
        }
        let mut units = Vec::new();
        for i in 0..MAX_WIDE {
            let u = unsafe { *p.add(i) };
            if u == 0 {
                return Some(String::from_utf16_lossy(&units));
            }
            units.push(u);
        }
        None
    }

    /// Never unwinds into the system: the work is caught, and the end is marked last.
    unsafe extern "system" fn on_browse(status: u32, context: *const c_void, record: *const DNS_RECORDW) {
        let id = context as usize as u64;
        let _ = catch_unwind(AssertUnwindSafe(|| {
            let mut r = record;
            // The records are the system's: copy what is needed, then free the list.
            while !r.is_null() {
                let rec = unsafe { &*r };
                if rec.wType == DNS_TYPE_PTR {
                    if let Some(full) = unsafe { read_wide(rec.Data.PTR.pNameHost) } {
                        send(Msg::Browsed { id, full, gone: rec.dwTtl == 0 });
                    }
                }
                r = rec.pNext;
            }
            if !record.is_null() {
                unsafe { DnsFree(record as *const c_void, DnsFreeRecordList) };
            }
        }));
        if status == ERROR_CANCELLED {
            ENDED.mark(id);
        }
    }

    /// A resolve calls back once (completion, timeout or cancel): the end is marked last.
    unsafe extern "system" fn on_resolve(status: u32, context: *const c_void, instance: *const DNS_SERVICE_INSTANCE) {
        let id = context as usize as u64;
        let _ = catch_unwind(AssertUnwindSafe(|| {
            let mut found = None;
            if status == STATUS_OK && !instance.is_null() {
                let i = unsafe { &*instance };
                let address = if !i.ip4Address.is_null() {
                    Some(Ipv4Addr::from(unsafe { *i.ip4Address }.to_le_bytes()).to_string())
                } else if !i.ip6Address.is_null() {
                    Some(Ipv6Addr::from(unsafe { i.ip6Address.read().IP6Byte }).to_string())
                } else {
                    None
                };
                let mut txt = Vec::new();
                if !i.keys.is_null() && !i.values.is_null() {
                    for n in 0..(i.dwPropertyCount as usize).min(MAX_TXT) {
                        let k = unsafe { read_wide(*i.keys.add(n)) };
                        let v = unsafe { read_wide(*i.values.add(n)) };
                        if let (Some(k), Some(v)) = (k, v) {
                            txt.push((k, v));
                        }
                    }
                }
                found = address.map(|a| (a, i.wPort, txt));
            }
            if !instance.is_null() {
                unsafe { DnsServiceFreeInstance(instance) };
            }
            send(Msg::Resolved { id, found });
        }));
        ENDED.mark(id);
    }

    impl Browser {
        /// Starts browsing; `notify` wakes the event loop when an event is waiting.
        pub fn start(notify: Notify) -> Result<Browser, String> {
            let (tx, rx) = mpsc::channel::<Msg>();
            let (ui_tx, ui_rx) = mpsc::sync_channel::<Event>(UI_QUEUE);
            *SINK.lock().map_err(|_| "Bonjour is unavailable".to_string())? = Some(tx.clone());
            // From here every return path drops `session`, which cancels what was started.
            let mut session = Session { requests: Requests::new(System, &ENDED, GRACE), browses: Vec::new() };
            for (index, service) in SERVICES.iter().enumerate() {
                let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
                match browse(service, id) {
                    Ok(request) => {
                        session.requests.add(id, request);
                        session.browses.push((id, index));
                    }
                    Err(status) => return Err(format!("DnsServiceBrowse({service}) failed: {status}")),
                }
            }
            let worker = std::thread::Builder::new()
                .name("bonjour".into())
                .spawn(move || {
                    let mut session = session;
                    let _ = catch_unwind(AssertUnwindSafe(|| run(&rx, &ui_tx, &notify, &mut session)));
                })
                .map_err(|e| format!("Bonjour thread: {e}"))?;
            Ok(Browser { events: ui_rx, stop: tx, worker: Some(worker) })
        }

        pub fn try_event(&self) -> Option<Event> {
            self.events.try_recv().ok()
        }
    }

    impl Drop for Browser {
        fn drop(&mut self) {
            let _ = self.stop.send(Msg::Stop);
            if let Some(w) = self.worker.take() {
                let _ = w.join();
            }
        }
    }

    struct Resolving {
        deadline: Instant,
        full: String,
        service: usize,
    }

    fn run(rx: &Receiver<Msg>, ui: &SyncSender<Event>, notify: &Notify, session: &mut Session) {
        let requests = &mut session.requests;
        let browses = &session.browses;
        let mut resolving: HashMap<u64, Resolving> = HashMap::new();
        // Instance names seen and not gone, by lowercase full name (resolving or resolved).
        let mut known: HashMap<String, Option<String>> = HashMap::new();
        let emit = |e: Event| {
            if ui.try_send(e).is_ok() {
                notify();
            }
        };
        loop {
            requests.reap();
            let next = resolving.values().map(|r| r.deadline).min();
            let msg = match next {
                Some(d) => match rx.recv_timeout(d.saturating_duration_since(Instant::now())) {
                    Ok(m) => Some(m),
                    Err(RecvTimeoutError::Timeout) => None,
                    Err(RecvTimeoutError::Disconnected) => break,
                },
                None => match rx.recv() {
                    Ok(m) => Some(m),
                    Err(_) => break,
                },
            };
            let now = Instant::now();
            let expired: Vec<u64> = resolving.iter().filter(|(_, r)| r.deadline <= now).map(|(id, _)| *id).collect();
            for id in expired {
                if let Some(r) = resolving.remove(&id) {
                    // Freed when its cancelled callback runs (a completed one is not cancelled).
                    requests.cancel(id);
                    known.remove(&r.full.to_ascii_lowercase());
                }
            }
            match msg {
                None => {}
                Some(Msg::Stop) => break,
                Some(Msg::Browsed { id, full, gone }) => {
                    let Some(service) = browses.iter().find(|(b, _)| *b == id).map(|(_, s)| *s) else { continue };
                    let key = full.to_ascii_lowercase();
                    if instance_label(&full, SERVICES[service]).is_none() {
                        continue;
                    }
                    if gone {
                        if let Some(Some(name)) = known.remove(&key) {
                            emit(Event::Lost(name));
                        } else if let Some(id) = resolving.iter().find(|(_, r)| r.full.eq_ignore_ascii_case(&full)).map(|(id, _)| *id) {
                            resolving.remove(&id);
                            requests.cancel(id);
                        }
                        continue;
                    }
                    // Cancelled resolves count until their last callback frees them.
                    if known.contains_key(&key) || known.len() >= MAX_KNOWN || requests.len() >= browses.len() + MAX_RESOLVING {
                        continue;
                    }
                    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
                    if let Some(request) = resolve(&full, id) {
                        requests.add(id, request);
                        known.insert(key, None);
                        resolving.insert(id, Resolving { deadline: Instant::now() + RESOLVE_TIMEOUT, full, service });
                    }
                }
                Some(Msg::Resolved { id, found }) => {
                    requests.finish(id);
                    let Some(r) = resolving.remove(&id) else { continue };
                    let Some((address, port, txt)) = found else {
                        known.remove(&r.full.to_ascii_lowercase());
                        continue;
                    };
                    let Some(name) = instance_label(&r.full, SERVICES[r.service]) else { continue };
                    let Some(service) = super::service_type(SERVICES[r.service]) else { continue };
                    known.insert(r.full.to_ascii_lowercase(), Some(name.clone()));
                    emit(Event::Found(Found { name, service, address, port, user: super::user_from_txt(&txt) }));
                }
            }
        }
        // The session's Drop cancels what is open, waits for the last callbacks, then clears the
        // sink.
    }
}

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
    fn live_hosts_are_bounded_and_matched() {
        let f = found("ssh", None, 22);
        let live = vec![Live { guid: "g1".into(), name: f.name.clone(), address: f.address.clone() }];
        assert!(!accept(&live, &f), "the same name and address twice");
        assert!(accept(&live, &Found { address: "10.0.0.6".into(), ..f.clone() }), "same name, other address");
        let full: Vec<Live> = (0..MAX_HOSTS).map(|i| Live { guid: i.to_string(), name: format!("n{i}"), address: "10.0.0.9".into() }).collect();
        assert!(!accept(&full, &f), "at the cap");
        assert!(accept(&full[..MAX_HOSTS - 1], &f));
        assert_eq!(lost_guids(&live, "Bob's Mac"), vec!["g1".to_string()]);
        assert!(lost_guids(&live, "bob's mac").is_empty());
        let none = |_: &str| false;
        assert_eq!(unique_name("h", "10.0.0.5", &none), "h");
        assert_eq!(unique_name("h", "10.0.0.5", &|n| n == "h"), "h (10.0.0.5)");
        assert_eq!(unique_name("h", "10.0.0.5", &|n| n == "h" || n == "h (10.0.0.5)"), "h (10.0.0.5) 2");
    }
}
