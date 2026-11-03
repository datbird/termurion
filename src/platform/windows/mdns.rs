//! `DnsServiceBrowse` for the three services and `DnsServiceResolve` for each instance found
//! (`ABM:396-541`, `:676-729`). The system's callbacks only copy what they are given and send it
//! to one worker thread (never blocking, never touching the app); the worker starts and times
//! out the resolves and hands `Event`s to the UI thread through a bounded channel and `Notify`.
//! Nothing is polled: the worker sleeps until a message or the next resolve deadline.
//! Every buffer handed to the system lives in one `Requests` owner, freed only after the
//! request's last callback; no early return, failed spawn or panic can free one sooner.

use crate::bonjour::{instance_label, DnsSd, Ended, Event, Found, Lost, Outbox, Requests, MAX_HOSTS, SERVICES};
use crate::platform::pty::Notify;
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
/// Events waiting for the UI thread (`Outbox`: a host that does not fit is resolved again
/// at its next announcement, a goodbye waits and is retried every `RETRY`).
const UI_QUEUE: usize = 256;
const RETRY: Duration = Duration::from_millis(250);
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
    (status == DNS_REQUEST_PENDING).then_some(Request::Resolve { _request: request, _name: name, cancel })
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
            if rec.wType == DNS_TYPE_PTR
                && let Some(full) = unsafe { read_wide(rec.Data.PTR.pNameHost) } {
                    send(Msg::Browsed { id, full, gone: rec.dwTtl == 0 });
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
    // Instance names seen and not gone, by lowercase full name: None while resolving, then
    // what a goodbye reports.
    let mut known: HashMap<String, Option<Lost>> = HashMap::new();
    let mut outbox = Outbox::default();
    let mut send = |e: Event| {
        let sent = ui.try_send(e).is_ok();
        if sent {
            notify();
        }
        sent
    };
    loop {
        requests.reap();
        outbox.flush(&mut send);
        let retry = outbox.waiting().then(|| Instant::now() + RETRY);
        let next = resolving.values().map(|r| r.deadline).chain(retry).min();
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
                    if let Some(Some(lost)) = known.remove(&key) {
                        outbox.emit(Event::Lost(lost), &mut send);
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
                let Some(service) = crate::bonjour::service_type(SERVICES[r.service]) else { continue };
                let key = r.full.to_ascii_lowercase();
                known.insert(key.clone(), Some(Lost { name: name.clone(), service: service.clone(), address: address.clone() }));
                if !outbox.emit(Event::Found(Found { name, service, address, port, user: crate::bonjour::user_from_txt(&txt) }), &mut send) {
                    // No room: forgotten, so the next announcement resolves it again.
                    known.remove(&key);
                }
            }
        }
    }
    // The session's Drop cancels what is open, waits for the last callbacks, then clears the
    // sink.
}
