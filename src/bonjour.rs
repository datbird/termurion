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
    if !full.is_char_boundary(cut) || !full[cut + 1..].eq_ignore_ascii_case(service) || full.as_bytes()[cut] != b'.' {
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

#[cfg(windows)]
pub use browser::Browser;

/// `DnsServiceBrowse` for the three services and `DnsServiceResolve` for each instance found
/// (`ABM:396-541`, `:676-729`). The system's callbacks only copy what they are given and send it
/// to one worker thread (never blocking, never touching the app); the worker starts and times
/// out the resolves and hands `Event`s to the UI thread through a bounded channel and `Notify`.
/// Nothing is polled: the worker sleeps until a message or the next resolve deadline.
#[cfg(windows)]
mod browser {
    use super::{instance_label, Event, Found, MAX_HOSTS, SERVICES};
    use crate::pty::Notify;
    use std::collections::HashMap;
    use std::ffi::c_void;
    use std::net::{Ipv4Addr, Ipv6Addr};
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

    enum Msg {
        /// A PTR answer: the full instance name, and whether it is a goodbye (TTL 0).
        Browsed { service: usize, full: String, gone: bool },
        Resolved { id: u64, found: Option<(String, u16, Vec<(String, String)>)> },
        Stop,
    }

    /// Where the system's callbacks send. Set by `start`, cleared by Drop after every request is
    /// cancelled. A callback holds this lock only to clone the sender.
    static SINK: Mutex<Option<Sender<Msg>>> = Mutex::new(None);
    /// Context numbers for requests; a callback of an older Browser finds no entry.
    static NEXT_ID: AtomicU64 = AtomicU64::new(1);

    fn send(m: Msg) {
        let tx = SINK.lock().ok().and_then(|g| g.clone());
        if let Some(tx) = tx {
            let _ = tx.send(m);
        }
    }

    /// A request's cancel handle: the system keeps the address until the request ends.
    struct Cancel(Box<DNS_SERVICE_CANCEL>);
    // SAFETY: only the worker (and `start`, before the worker runs) touches the handle.
    unsafe impl Send for Cancel {}

    struct Resolving {
        cancel: Cancel,
        _name: Vec<u16>,
        deadline: Instant,
        full: String,
        service: usize,
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

    unsafe extern "system" fn on_browse(_status: u32, context: *const c_void, record: *const DNS_RECORDW) {
        let service = context as usize;
        let mut r = record;
        // The records are the system's: copy what is needed, then free the list.
        while !r.is_null() {
            let rec = unsafe { &*r };
            if rec.wType == DNS_TYPE_PTR {
                if let Some(full) = unsafe { read_wide(rec.Data.PTR.pNameHost) } {
                    send(Msg::Browsed { service, full, gone: rec.dwTtl == 0 });
                }
            }
            r = rec.pNext;
        }
        if !record.is_null() {
            unsafe { DnsFree(record as *const c_void, DnsFreeRecordList) };
        }
    }

    unsafe extern "system" fn on_resolve(status: u32, context: *const c_void, instance: *const DNS_SERVICE_INSTANCE) {
        let id = context as usize as u64;
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
    }

    impl Browser {
        /// Starts browsing; `notify` wakes the event loop when an event is waiting.
        pub fn start(notify: Notify) -> Result<Browser, String> {
            let (tx, rx) = mpsc::channel::<Msg>();
            let (ui_tx, ui_rx) = mpsc::sync_channel::<Event>(UI_QUEUE);
            *SINK.lock().map_err(|_| "Bonjour is unavailable".to_string())? = Some(tx.clone());
            let mut browses: Vec<(Cancel, Vec<u16>)> = Vec::new();
            let mut failed = None;
            for (index, service) in SERVICES.iter().enumerate() {
                let name = wide(service);
                let mut cancel = Box::new(DNS_SERVICE_CANCEL { reserved: std::ptr::null_mut() });
                let request = DNS_SERVICE_BROWSE_REQUEST {
                    Version: DNS_QUERY_REQUEST_VERSION1,
                    InterfaceIndex: 0,
                    QueryName: name.as_ptr(),
                    Anonymous: DNS_SERVICE_BROWSE_REQUEST_0 { pBrowseCallback: Some(on_browse) },
                    pQueryContext: index as *mut c_void,
                };
                // SAFETY: the request, name and cancel handle are valid for the call; the name and
                // handle outlive the request (kept in `browses` until it is cancelled).
                let status = unsafe { DnsServiceBrowse(&request, &mut *cancel) };
                if status != DNS_REQUEST_PENDING {
                    failed = Some(format!("DnsServiceBrowse({service}) failed: {status}"));
                    break;
                }
                browses.push((Cancel(cancel), name));
            }
            if let Some(e) = failed {
                for (c, _) in &browses {
                    unsafe { DnsServiceBrowseCancel(&*c.0) };
                }
                *SINK.lock().unwrap_or_else(|p| p.into_inner()) = None;
                return Err(e);
            }
            let worker = std::thread::Builder::new()
                .name("bonjour".into())
                .spawn(move || run(rx, ui_tx, notify, browses))
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

    fn run(rx: Receiver<Msg>, ui: SyncSender<Event>, notify: Notify, browses: Vec<(Cancel, Vec<u16>)>) {
        let mut resolving: HashMap<u64, Resolving> = HashMap::new();
        // Instance names seen and not gone, by lowercase full name (resolving or resolved).
        let mut known: HashMap<String, Option<String>> = HashMap::new();
        let emit = |e: Event| {
            if ui.try_send(e).is_ok() {
                notify();
            }
        };
        loop {
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
                    unsafe { DnsServiceResolveCancel(&*r.cancel.0) };
                    known.remove(&r.full.to_ascii_lowercase());
                }
            }
            match msg {
                None => {}
                Some(Msg::Stop) => break,
                Some(Msg::Browsed { service, full, gone }) => {
                    let key = full.to_ascii_lowercase();
                    if SERVICES.get(service).and_then(|s| instance_label(&full, s)).is_none() {
                        continue;
                    }
                    if gone {
                        if let Some(Some(name)) = known.remove(&key) {
                            emit(Event::Lost(name));
                        } else if let Some(id) = resolving.iter().find(|(_, r)| r.full.eq_ignore_ascii_case(&full)).map(|(id, _)| *id) {
                            if let Some(r) = resolving.remove(&id) {
                                unsafe { DnsServiceResolveCancel(&*r.cancel.0) };
                            }
                        }
                        continue;
                    }
                    if known.contains_key(&key) || known.len() >= MAX_KNOWN || resolving.len() >= MAX_RESOLVING {
                        continue;
                    }
                    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
                    let mut name = wide(&full);
                    let mut cancel = Box::new(DNS_SERVICE_CANCEL { reserved: std::ptr::null_mut() });
                    let request = DNS_SERVICE_RESOLVE_REQUEST {
                        Version: DNS_QUERY_REQUEST_VERSION1,
                        InterfaceIndex: 0,
                        QueryName: name.as_mut_ptr(),
                        pResolveCompletionCallback: Some(on_resolve),
                        pQueryContext: id as usize as *mut c_void,
                    };
                    // SAFETY: as in `start`; the name and handle are kept in `resolving`.
                    let status = unsafe { DnsServiceResolve(&request, &mut *cancel) };
                    if status == DNS_REQUEST_PENDING {
                        known.insert(key, None);
                        resolving.insert(id, Resolving { cancel: Cancel(cancel), _name: name, deadline: Instant::now() + RESOLVE_TIMEOUT, full: full.clone(), service });
                    }
                }
                Some(Msg::Resolved { id, found }) => {
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
        // Cancel everything before the callbacks' sink goes (a cancel waits for a running callback,
        // and a callback only ever sends).
        for (c, _) in &browses {
            unsafe { DnsServiceBrowseCancel(&*c.0) };
        }
        for r in resolving.values() {
            unsafe { DnsServiceResolveCancel(&*r.cancel.0) };
        }
        *SINK.lock().unwrap_or_else(|p| p.into_inner()) = None;
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
