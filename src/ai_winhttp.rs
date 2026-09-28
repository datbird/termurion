//! The AI transport on WinHTTP (ruling 4): one worker thread per call runs an asynchronous
//! WinHTTP request with the system proxy and TLS, posts each read as it arrives and ends with
//! `Done`. Every WinHTTP call on the request handle is made under the call's lock, and cancel
//! closes the handle under the same lock, so no call ever receives a closed handle value (the
//! calls return at once in asynchronous mode, so the lock is never held across a wait). The
//! worker waits for WinHTTP's notifications with the idle time as its limit, which is iTerm2's
//! idle timeout between bytes. A redirect is followed only to the host the consent gate checked.
//! Ports `AITerm/JSFetch.swift` (`JF:35-173`).

use crate::ai_transport::{invalid_url_events, parse_url, Call, Delivery, HttpEvent, HttpRequest, Notify, Options, Proxy, Target, Transport, EVENT_QUEUE};
use std::cell::UnsafeCell;
use std::ffi::c_void;
use std::sync::mpsc::{channel, sync_channel, Receiver, RecvTimeoutError, Sender, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::GetLastError;
use windows_sys::Win32::Networking::WinHttp::*;

/// `WINHTTP_DECOMPRESSION_FLAG_GZIP | WINHTTP_DECOMPRESSION_FLAG_DEFLATE` (URLSession decodes
/// both too).
const DECOMPRESSION_ALL: u32 = WINHTTP_DECOMPRESSION_FLAG_GZIP | WINHTTP_DECOMPRESSION_FLAG_DEFLATE;
/// Each read's buffer.
const READ_SIZE: usize = 64 * 1024;
/// The redirects one request follows (WinHTTP's own default limit).
const MAX_REDIRECTS: usize = 10;
/// How long the worker waits for WinHTTP to confirm a closed request handle before it closes the
/// connection and the session anyway.
const CLOSE_WAIT: Duration = Duration::from_secs(5);
/// How long the worker sleeps before it offers an event to a full queue again.
const POST_WAIT: Duration = Duration::from_millis(5);
/// The notifications the worker waits for; `HANDLE_CLOSING` releases the handle's context.
const NOTIFICATIONS: u32 = WINHTTP_CALLBACK_FLAG_SENDREQUEST_COMPLETE
    | WINHTTP_CALLBACK_FLAG_HEADERS_AVAILABLE
    | WINHTTP_CALLBACK_FLAG_DATA_AVAILABLE
    | WINHTTP_CALLBACK_FLAG_READ_COMPLETE
    | WINHTTP_CALLBACK_FLAG_REQUEST_ERROR
    | WINHTTP_CALLBACK_STATUS_HANDLE_CLOSING;

/// NSURLError's text for too many redirects.
pub const TOO_MANY_REDIRECTS: &str = "too many HTTP redirects";

/// A redirect the transport refuses to follow: the request would leave the host the consent gate
/// checked, or go from https to plain http.
pub fn redirect_refused(to: &str) -> String {
    format!("A redirect to {to} was not followed.")
}

/// The WinHTTP transport.
pub struct WinHttp;

/// The open request handle and why the call stopped.
struct Slot {
    request: Option<usize>,
    cancelled: bool,
    timed_out: bool,
}

impl Slot {
    /// Closes and forgets the request handle, if it is open.
    fn close(&mut self) {
        if let Some(h) = self.request.take() {
            unsafe { WinHttpCloseHandle(h as *mut c_void) };
        }
    }
}

/// Called by the worker before each WinHTTP call on the request handle, with the step's index.
#[cfg(test)]
type Hook = Box<dyn Fn(usize, &Arc<Shared>) + Send + Sync>;

/// What a call and its worker share.
struct Shared {
    slot: Mutex<Slot>,
    #[cfg(test)]
    hook: Option<Hook>,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Slot> {
        self.slot.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Stops the call: the request handle is closed at once, which ends WinHTTP's pending
    /// operation, and nothing is posted afterwards.
    fn cancel(&self) {
        let mut s = self.lock();
        s.cancelled = true;
        s.close();
    }

    fn cancelled(&self) -> bool {
        self.lock().cancelled
    }
}

struct WinCall {
    events: Receiver<HttpEvent>,
    shared: Arc<Shared>,
    /// Read only by the tests' `finished`: a dropped call never waits for its worker.
    #[cfg_attr(not(test), allow(dead_code))]
    thread: Option<JoinHandle<()>>,
}

impl Call for WinCall {
    fn try_event(&mut self) -> Option<HttpEvent> {
        if self.shared.cancelled() {
            return None;
        }
        self.events.try_recv().ok()
    }

    fn cancel(&mut self) {
        self.shared.cancel();
    }

    #[cfg(test)]
    fn finished(&self) -> bool {
        self.thread.as_ref().is_none_or(|h| h.is_finished())
    }
}

impl Drop for WinCall {
    /// A call nobody reads any more stops.
    fn drop(&mut self) {
        self.shared.cancel();
    }
}

impl Transport for WinHttp {
    fn start(&self, request: HttpRequest, options: Options, notify: Notify) -> Box<dyn Call> {
        start_call(request, options, notify, Shared { slot: new_slot(), #[cfg(test)] hook: None })
    }
}

fn new_slot() -> Mutex<Slot> {
    Mutex::new(Slot { request: None, cancelled: false, timed_out: false })
}

fn start_call(request: HttpRequest, options: Options, notify: Notify, shared: Shared) -> Box<dyn Call> {
    let shared = Arc::new(shared);
    // Bounded: a reader that falls behind makes the worker wait instead of queueing the stream.
    let (tx, rx) = sync_channel(EVENT_QUEUE);
    let thread = match parse_url(&request.url) {
        Err(_) => {
            for e in invalid_url_events() {
                let _ = tx.send(e);
            }
            notify();
            None
        }
        Ok(target) => {
            let (failed, notify_failed) = (tx.clone(), notify.clone());
            let worker = Worker { shared: shared.clone(), tx, notify, idle: options.idle, step: std::cell::Cell::new(0) };
            match std::thread::Builder::new().name("ai-http".into()).spawn(move || worker.run(&request, &target, &options)) {
                Ok(thread) => Some(thread),
                // The system refused a thread: the call fails instead of the app.
                Err(e) => {
                    let _ = failed.send(HttpEvent::Done { body: String::new(), error: Some(crate::ai_transport::transport_error(&e.to_string())) });
                    notify_failed();
                    None
                }
            }
        }
    };
    Box::new(WinCall { events: rx, shared, thread })
}

/// A NUL-terminated UTF-16 copy.
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// A session or connection handle the worker owns, closed on drop.
struct Owned(*mut c_void);

impl Drop for Owned {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { WinHttpCloseHandle(self.0) };
        }
    }
}

fn last_error() -> u32 {
    unsafe { GetLastError() }
}

/// A WinHTTP notification, as the worker sees it.
enum Note {
    Sent,
    Headers,
    Data(u32),
    Read(u32),
    Error(u32),
    Closing,
}

/// What one request handle's notifications need; owned by WinHTTP (through the handle's context
/// value) until `HANDLE_CLOSING`, so the body and the read buffer outlive every pending
/// operation, even one a cancel abandons.
struct Context {
    notes: Mutex<Sender<Note>>,
    body: Vec<u8>,
    buf: UnsafeCell<Vec<u8>>,
}

// WinHTTP writes `buf` only while a read is pending, and the worker reads it only after that
// read's `READ_COMPLETE`.
unsafe impl Sync for Context {}

/// WinHTTP's status callback, on WinHTTP's threads or inside a call on the worker's.
unsafe extern "system" fn on_status(_h: *mut c_void, context: usize, status: u32, info: *mut c_void, len: u32) {
    if context == 0 {
        return;
    }
    // SAFETY: `context` is the `Arc<Context>` pointer `Worker::open` gave WinHTTP; it stays
    // alive until the `HANDLE_CLOSING` below, WinHTTP's last notification for the handle, and
    // `info` is the notification's documented payload.
    let ctx = unsafe { &*(context as *const Context) };
    let note = match status {
        WINHTTP_CALLBACK_STATUS_SENDREQUEST_COMPLETE => Note::Sent,
        WINHTTP_CALLBACK_STATUS_HEADERS_AVAILABLE => Note::Headers,
        WINHTTP_CALLBACK_STATUS_DATA_AVAILABLE => Note::Data(unsafe { *(info as *const u32) }),
        WINHTTP_CALLBACK_STATUS_READ_COMPLETE => Note::Read(len),
        WINHTTP_CALLBACK_STATUS_REQUEST_ERROR => Note::Error(unsafe { (*(info as *const WINHTTP_ASYNC_RESULT)).dwError }),
        WINHTTP_CALLBACK_STATUS_HANDLE_CLOSING => {
            let _ = ctx.notes.lock().unwrap_or_else(|e| e.into_inner()).send(Note::Closing);
            // The last notification for this handle: WinHTTP's reference goes.
            drop(unsafe { Arc::from_raw(context as *const Context) });
            return;
        }
        _ => return,
    };
    let _ = ctx.notes.lock().unwrap_or_else(|e| e.into_inner()).send(note);
}

/// One request handle: the worker's share of its context and its notifications.
struct Hop {
    ctx: Arc<Context>,
    notes: Receiver<Note>,
    /// Until `HANDLE_CLOSING` arrives.
    open: bool,
}

/// Why an exchange stopped.
enum Fail {
    Code(u32),
    Text(String),
    Stopped,
}

/// How one request handle ended: the final response was read, or it redirects.
enum HopEnd {
    Done,
    Redirect(u32, String),
}

struct Worker {
    shared: Arc<Shared>,
    tx: SyncSender<HttpEvent>,
    notify: Notify,
    idle: Duration,
    #[cfg_attr(not(test), allow(dead_code))]
    step: std::cell::Cell<usize>,
}

impl Worker {
    /// Queues `event` for the reader; while the queue is full it waits, until a cancel.
    fn post(&self, mut event: HttpEvent) {
        loop {
            if self.shared.cancelled() {
                return;
            }
            match self.tx.try_send(event) {
                Ok(()) => {
                    (self.notify)();
                    return;
                }
                Err(TrySendError::Full(back)) => {
                    event = back;
                    std::thread::sleep(POST_WAIT);
                }
                Err(TrySendError::Disconnected(_)) => return,
            }
        }
    }

    fn run(self, request: &HttpRequest, target: &Target, options: &Options) {
        let mut delivery = Delivery::new(0, request.stream);
        let result = self.exchange(request, target, options, &mut delivery);
        let slot = self.shared.lock();
        if slot.cancelled {
            return;
        }
        let text = match result {
            Ok(()) => None,
            Err(_) if slot.timed_out => Some(error_text(ERROR_WINHTTP_TIMEOUT)),
            Err(Fail::Code(code)) => Some(error_text(code)),
            Err(Fail::Text(text)) => Some(text),
            Err(Fail::Stopped) => Some(error_text(ERROR_WINHTTP_OPERATION_CANCELLED)),
        };
        drop(slot);
        for event in delivery.done(text.as_deref()) {
            self.post(event);
        }
    }

    /// One WinHTTP call on the open request handle, made under the lock; `f` returns the call's
    /// BOOL. Stopped once the call was cancelled or timed out, whose close took the handle.
    fn call(&self, f: impl FnOnce(*mut c_void) -> i32) -> Result<(), Fail> {
        #[cfg(test)]
        {
            let n = self.step.get();
            self.step.set(n + 1);
            if let Some(hook) = &self.shared.hook {
                hook(n, &self.shared);
            }
        }
        let slot = self.shared.lock();
        let Some(h) = slot.request.filter(|_| !slot.cancelled) else {
            return Err(Fail::Stopped);
        };
        if f(h as *mut c_void) != 0 {
            Ok(())
        } else {
            Err(Fail::Code(last_error()))
        }
    }

    /// The next notification `want` accepts; the idle time bounds each wait (`JF:131-140`).
    fn wait(&self, hop: &mut Hop, want: impl Fn(&Note) -> Option<u32>) -> Result<u32, Fail> {
        loop {
            let note = if self.idle.is_zero() { hop.notes.recv().map_err(|_| RecvTimeoutError::Disconnected) } else { hop.notes.recv_timeout(self.idle) };
            match note {
                Err(RecvTimeoutError::Timeout) => {
                    let mut s = self.shared.lock();
                    s.timed_out = true;
                    s.close();
                    return Err(Fail::Code(ERROR_WINHTTP_TIMEOUT));
                }
                Err(RecvTimeoutError::Disconnected) => return Err(Fail::Stopped),
                Ok(Note::Closing) => {
                    hop.open = false;
                    return Err(Fail::Stopped);
                }
                Ok(Note::Error(code)) => return Err(Fail::Code(code)),
                Ok(note) => {
                    if let Some(v) = want(&note) {
                        return Ok(v);
                    }
                }
            }
        }
    }

    /// A request handle on `connect` with its callback and context, placed in the slot (or
    /// closed at once when the call was already cancelled).
    fn open(&self, connect: *mut c_void, method: &str, target: &Target, body: Vec<u8>) -> Result<Hop, Fail> {
        let (tx, notes) = channel();
        let ctx = Arc::new(Context { notes: Mutex::new(tx), body, buf: UnsafeCell::new(vec![0u8; READ_SIZE]) });
        let (verb, path) = (wide(method), wide(&target.path));
        let secure = if target.secure { WINHTTP_FLAG_SECURE } else { 0 };
        let handle = unsafe { WinHttpOpenRequest(connect, verb.as_ptr(), path.as_ptr(), std::ptr::null(), std::ptr::null(), std::ptr::null(), secure) };
        if handle.is_null() {
            return Err(Fail::Code(last_error()));
        }
        let previous = unsafe { WinHttpSetStatusCallback(handle, Some(on_status), NOTIFICATIONS, 0) };
        if previous.map_or(0, |f| f as usize) == usize::MAX {
            let code = last_error();
            unsafe { WinHttpCloseHandle(handle) };
            return Err(Fail::Code(code));
        }
        let raw = Arc::into_raw(ctx.clone()) as usize;
        if unsafe { WinHttpSetOption(handle, WINHTTP_OPTION_CONTEXT_VALUE, (&raw as *const usize).cast(), std::mem::size_of::<usize>() as u32) } == 0 {
            let code = last_error();
            // WinHTTP never took the context, so its closing notification carries none.
            unsafe {
                drop(Arc::from_raw(raw as *const Context));
                WinHttpCloseHandle(handle);
            }
            return Err(Fail::Code(code));
        }
        let mut s = self.shared.lock();
        if s.cancelled {
            unsafe { WinHttpCloseHandle(handle) };
        } else {
            s.request = Some(handle as usize);
        }
        Ok(Hop { ctx, notes, open: true })
    }

    /// Closes the hop's request handle if it is still open and waits for WinHTTP to let go of
    /// its context, so the connection and the session close after their child.
    fn finish(&self, hop: &mut Hop) {
        self.shared.lock().close();
        let deadline = Instant::now() + CLOSE_WAIT;
        while hop.open {
            match hop.notes.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                Ok(Note::Closing) | Err(_) => hop.open = false,
                Ok(_) => {}
            }
        }
    }

    /// The request from open to the last read, following redirects to the gated host only.
    fn exchange(&self, request: &HttpRequest, target: &Target, options: &Options, delivery: &mut Delivery) -> Result<(), Fail> {
        let check = |ok: i32| if ok != 0 { Ok(()) } else { Err(Fail::Code(last_error())) };
        let agent = wide("Tandem");
        let proxy_name = match &options.proxy {
            Proxy::HttpsOnly { host, port } => Some(wide(&format!("https={host}:{port}"))),
            _ => None,
        };
        let access = match options.proxy {
            Proxy::System => WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
            Proxy::HttpsOnly { .. } => WINHTTP_ACCESS_TYPE_NAMED_PROXY,
            Proxy::Off => WINHTTP_ACCESS_TYPE_NO_PROXY,
        };
        let proxy_ptr = proxy_name.as_ref().map_or(std::ptr::null(), |p| p.as_ptr());
        let session = Owned(unsafe { WinHttpOpen(agent.as_ptr(), access, proxy_ptr, std::ptr::null(), WINHTTP_FLAG_ASYNC) });
        if session.0.is_null() {
            return Err(Fail::Code(last_error()));
        }
        // WinHTTP's own timeouts bound name resolution, connecting, sending and receiving; the
        // worker's wait bounds the time between notifications.
        let t = if options.idle.is_zero() { 0 } else { options.idle.as_millis().clamp(1, i32::MAX as u128) as i32 };
        check(unsafe { WinHttpSetTimeouts(session.0, t, t, t, t) })?;
        // Optional: without it a compressed answer arrives as sent, which no AI server does
        // unasked.
        let flags = DECOMPRESSION_ALL;
        unsafe { WinHttpSetOption(session.0, WINHTTP_OPTION_DECOMPRESSION, (&flags as *const u32).cast(), 4) };
        // WinHTTP would follow a redirect to any host with the key headers; the worker follows
        // them itself, to the gated host only.
        let never = WINHTTP_OPTION_REDIRECT_POLICY_NEVER;
        check(unsafe { WinHttpSetOption(session.0, WINHTTP_OPTION_REDIRECT_POLICY, (&never as *const u32).cast(), 4) })?;

        let (mut target, mut method, mut body, mut headers) = (target.clone(), request.method, request.body.clone(), request.headers.clone());
        let mut redirects = 0;
        loop {
            let host = wide(&target.host);
            let connect = Owned(unsafe { WinHttpConnect(session.0, host.as_ptr(), target.port, 0) });
            if connect.0.is_null() {
                return Err(Fail::Code(last_error()));
            }
            let mut hop = self.open(connect.0, method, &target, body.clone())?;
            let end = self.hop(&mut hop, &headers, request.stream, delivery);
            self.finish(&mut hop);
            drop(connect);
            let (status, location) = match end? {
                HopEnd::Done => return Ok(()),
                HopEnd::Redirect(status, location) => (status, location),
            };
            if redirects == MAX_REDIRECTS {
                return Err(Fail::Text(TOO_MANY_REDIRECTS.to_string()));
            }
            redirects += 1;
            target = follow(&target, &location).map_err(Fail::Text)?;
            // URLSession, as browsers: 303, and 301 or 302 after a POST, repeat as a GET without
            // the body and the headers that describe it (Fetch's request-body-header names); 307
            // and 308 repeat the request as it was.
            if status == 303 || (matches!(status, 301 | 302) && method == "POST") {
                method = "GET";
                body.clear();
                headers.retain(|(n, _)| !["Content-Type", "Content-Encoding", "Content-Language", "Content-Location"].iter().any(|b| n.eq_ignore_ascii_case(b)));
            }
        }
    }

    /// One request handle: send, read the status, and either follow a redirect or read the body.
    fn hop(&self, hop: &mut Hop, headers: &[(String, String)], stream: bool, delivery: &mut Delivery) -> Result<HopEnd, Fail> {
        let lines: String = headers
            .iter()
            .filter(|(n, v)| crate::ai_policy::header_name_valid(n) && crate::ai_policy::header_value_valid(v))
            .map(|(n, v)| format!("{n}: {v}\r\n"))
            .collect();
        if !lines.is_empty() {
            let lines: Vec<u16> = lines.encode_utf16().collect();
            self.call(|h| unsafe { WinHttpAddRequestHeaders(h, lines.as_ptr(), lines.len() as u32, WINHTTP_ADDREQ_FLAG_ADD | WINHTTP_ADDREQ_FLAG_REPLACE) })?;
        }
        let ctx = Arc::as_ptr(&hop.ctx) as usize;
        let len = hop.ctx.body.len() as u32;
        let body = if hop.ctx.body.is_empty() { std::ptr::null() } else { hop.ctx.body.as_ptr().cast() };
        // WinHTTP writes Content-Length.
        self.call(|h| unsafe { WinHttpSendRequest(h, std::ptr::null(), 0, body, len, len, ctx) })?;
        self.wait(hop, |n| matches!(n, Note::Sent).then_some(0))?;
        self.call(|h| unsafe { WinHttpReceiveResponse(h, std::ptr::null_mut()) })?;
        self.wait(hop, |n| matches!(n, Note::Headers).then_some(0))?;
        let (mut status, mut size) = (0u32, 4u32);
        self.call(|h| unsafe {
            WinHttpQueryHeaders(h, WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER, std::ptr::null(), (&mut status as *mut u32).cast(), &mut size, std::ptr::null_mut())
        })?;
        if matches!(status, 301 | 302 | 303 | 307 | 308) {
            if let Some(location) = self.location()? {
                return Ok(HopEnd::Redirect(status, location));
            }
        }
        *delivery = Delivery::new(status, stream);
        let buf = hop.ctx.buf.get();
        loop {
            self.call(|h| unsafe { WinHttpQueryDataAvailable(h, std::ptr::null_mut()) })?;
            let available = self.wait(hop, |n| if let Note::Data(k) = n { Some(*k) } else { None })?;
            if available == 0 {
                return Ok(HopEnd::Done);
            }
            let want = (available as usize).min(READ_SIZE) as u32;
            self.call(|h| unsafe { WinHttpReadData(h, (*buf).as_mut_ptr().cast(), want, std::ptr::null_mut()) })?;
            let read = self.wait(hop, |n| if let Note::Read(k) = n { Some(*k) } else { None })?;
            if read == 0 {
                return Ok(HopEnd::Done);
            }
            let bytes = unsafe { &(&*buf)[..read as usize] };
            if let Some(event) = delivery.read(bytes).map_err(Fail::Text)? {
                self.post(event);
            }
        }
    }

    /// The response's Location header, if it has one.
    fn location(&self) -> Result<Option<String>, Fail> {
        let mut size = 0u32;
        match self.call(|h| unsafe { WinHttpQueryHeaders(h, WINHTTP_QUERY_LOCATION, std::ptr::null(), std::ptr::null_mut(), &mut size, std::ptr::null_mut()) }) {
            Err(Fail::Code(ERROR_WINHTTP_HEADER_NOT_FOUND)) => return Ok(None),
            Err(Fail::Code(ERROR_INSUFFICIENT_BUFFER)) | Ok(()) => {}
            Err(other) => return Err(other),
        }
        let mut text = vec![0u16; (size as usize).div_ceil(2) + 1];
        let mut bytes = (text.len() * 2) as u32;
        self.call(|h| unsafe { WinHttpQueryHeaders(h, WINHTTP_QUERY_LOCATION, std::ptr::null(), text.as_mut_ptr().cast(), &mut bytes, std::ptr::null_mut()) })?;
        Ok(Some(String::from_utf16_lossy(&text[..bytes as usize / 2])))
    }
}

/// `ERROR_INSUFFICIENT_BUFFER`.
const ERROR_INSUFFICIENT_BUFFER: u32 = 122;

/// Where a redirect from `current` to `location` goes, resolved as a browser would (RFC 3986
/// section 5: a fragment is never sent, a query-only reference keeps the path, dot segments
/// are removed), when it stays on the host the consent gate checked and does not drop https;
/// else the refusal text.
fn follow(current: &Target, location: &str) -> Result<Target, String> {
    let location = location.split('#').next().unwrap_or_default();
    let scheme = if current.secure { "https" } else { "http" };
    let host = if current.host.contains(':') { format!("[{}]", current.host) } else { current.host.clone() };
    let origin = format!("{scheme}://{host}:{}", current.port);
    let absolute = location.split_once("://").is_some_and(|(s, _)| !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"+-.".contains(&b)));
    let path = current.path.split('?').next().unwrap_or("/");
    let url = if absolute {
        location.to_string()
    } else if location.starts_with("//") {
        format!("{scheme}:{location}")
    } else if location.is_empty() {
        format!("{origin}{}", current.path)
    } else if location.starts_with('/') {
        format!("{origin}{location}")
    } else if location.starts_with('?') {
        format!("{origin}{path}{location}")
    } else {
        let dir = &path[..path.rfind('/').map_or(0, |i| i + 1)];
        format!("{origin}{dir}{location}")
    };
    let mut next = parse_url(&url).map_err(|_| redirect_refused("an address Tandem cannot read"))?;
    if next.host != current.host {
        return Err(redirect_refused(&format!("another host ({})", next.host)));
    }
    if current.secure && !next.secure {
        return Err(redirect_refused("plain http"));
    }
    next.path = without_dot_segments(&next.path);
    Ok(next)
}

/// RFC 3986's `remove_dot_segments` on the path, the query kept as it is.
fn without_dot_segments(path_and_query: &str) -> String {
    let (path, query) = path_and_query.split_once('?').map_or((path_and_query, None), |(p, q)| (p, Some(q)));
    let segments: Vec<&str> = path.split('/').skip(1).collect();
    let mut out: Vec<&str> = Vec::new();
    for (i, segment) in segments.iter().enumerate() {
        let last = i + 1 == segments.len();
        match *segment {
            "." => {}
            ".." => {
                out.pop();
            }
            other => {
                out.push(other);
                continue;
            }
        }
        // A path ending in a dot segment still names a directory.
        if last {
            out.push("");
        }
    }
    let path = format!("/{}", out.join("/"));
    match query {
        Some(q) => format!("{path}?{q}"),
        None => path,
    }
}


/// A WinHTTP error as a person reads it: Apple's NSURLError texts for the five codes a person
/// meets (so iTerm2's messages read the same), else the system's WinHTTP message (Decision 5).
pub fn error_text(code: u32) -> String {
    match code {
        ERROR_WINHTTP_TIMEOUT => "The request timed out.".to_string(),
        ERROR_WINHTTP_NAME_NOT_RESOLVED => "A server with the specified hostname could not be found.".to_string(),
        ERROR_WINHTTP_CANNOT_CONNECT => "Could not connect to the server.".to_string(),
        ERROR_WINHTTP_CONNECTION_ERROR => "The network connection was lost.".to_string(),
        ERROR_WINHTTP_SECURE_FAILURE => "An SSL error has occurred and a secure connection to the server cannot be made.".to_string(),
        _ => system_text(code).unwrap_or_else(|| format!("WinHTTP error {code}")),
    }
}

fn system_text(code: u32) -> Option<String> {
    use windows_sys::Win32::System::Diagnostics::Debug::{FormatMessageW, FORMAT_MESSAGE_FROM_HMODULE, FORMAT_MESSAGE_IGNORE_INSERTS};
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    let name = wide("winhttp.dll");
    let module = unsafe { GetModuleHandleW(name.as_ptr()) };
    if module.is_null() {
        return None;
    }
    let mut buf = [0u16; 512];
    let n = unsafe {
        FormatMessageW(FORMAT_MESSAGE_FROM_HMODULE | FORMAT_MESSAGE_IGNORE_INSERTS, module as *const c_void, code, 0, buf.as_mut_ptr(), buf.len() as u32, std::ptr::null())
    };
    let text = String::from_utf16_lossy(&buf[..n as usize]).trim().to_string();
    (!text.is_empty()).then_some(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai_transport::transport_error;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};
    use std::sync::atomic::Ordering;

    fn off(idle: Duration) -> Options {
        Options { idle, proxy: Proxy::Off }
    }

    fn no_notify() -> Notify {
        Arc::new(|| {})
    }

    /// A one-connection server on loopback running `script`; the URL base is "http://127.0.0.1:port".
    /// The server gives up when no client comes within 10 s (a cancelled call may never connect).
    fn serve(script: impl FnOnce(TcpStream) + Send + 'static) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        listener.set_nonblocking(true).unwrap();
        std::thread::spawn(move || {
            let start = Instant::now();
            while start.elapsed() < Duration::from_secs(10) {
                match listener.accept() {
                    Ok((s, _)) => {
                        s.set_nonblocking(false).unwrap();
                        script(s);
                        return;
                    }
                    Err(_) => std::thread::sleep(Duration::from_millis(2)),
                }
            }
        });
        format!("http://127.0.0.1:{port}")
    }

    /// The request head (through the blank line) and its Content-Length body.
    fn read_request(s: &mut TcpStream) -> (String, Vec<u8>) {
        let mut data = Vec::new();
        let mut byte = [0u8; 1];
        while !data.ends_with(b"\r\n\r\n") {
            if s.read(&mut byte).unwrap_or(0) == 0 {
                break;
            }
            data.push(byte[0]);
        }
        let head = String::from_utf8_lossy(&data).into_owned();
        let len = head
            .lines()
            .find_map(|l| l.split_once(':').filter(|(n, _)| n.eq_ignore_ascii_case("content-length")).map(|(_, v)| v.trim().parse::<usize>().unwrap()))
            .unwrap_or(0);
        let mut body = vec![0u8; len];
        s.read_exact(&mut body).unwrap();
        (head, body)
    }

    const STREAM_HEAD: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n";

    fn send_chunk(s: &mut TcpStream, data: &str) -> std::io::Result<()> {
        s.write_all(format!("{:x}\r\n{data}\r\n", data.len()).as_bytes())?;
        s.flush()
    }

    fn end_chunks(s: &mut TcpStream) {
        let _ = s.write_all(b"0\r\n\r\n");
        let _ = s.flush();
    }

    fn post(url: String, stream: bool) -> HttpRequest {
        HttpRequest { method: "POST", url, headers: vec![], body: b"{}".to_vec(), stream }
    }

    /// Every event until `Done` or `limit`.
    fn collect(call: &mut Box<dyn Call>, limit: Duration) -> Vec<HttpEvent> {
        let start = Instant::now();
        let mut out = Vec::new();
        while start.elapsed() < limit {
            match call.try_event() {
                Some(e) => {
                    let done = matches!(e, HttpEvent::Done { .. });
                    out.push(e);
                    if done {
                        break;
                    }
                }
                None => std::thread::sleep(Duration::from_millis(5)),
            }
        }
        out
    }

    fn next_event(call: &mut Box<dyn Call>, limit: Duration) -> Option<HttpEvent> {
        let start = Instant::now();
        while start.elapsed() < limit {
            if let Some(e) = call.try_event() {
                return Some(e);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        None
    }

    fn chunk(s: &str) -> HttpEvent {
        HttpEvent::Chunk(s.to_string())
    }

    #[test]
    fn winhttp_streams_chunks() {
        let (seen_tx, seen_rx) = mpsc::channel();
        let base = serve(move |mut s| {
            seen_tx.send(read_request(&mut s)).unwrap();
            s.write_all(STREAM_HEAD).unwrap();
            for i in 1..=3 {
                std::thread::sleep(Duration::from_millis(200));
                send_chunk(&mut s, &format!("data: {i}\n\n")).unwrap();
            }
            end_chunks(&mut s);
        });
        let request = HttpRequest {
            method: "POST",
            url: format!("{base}/v1/x?y=1"),
            headers: vec![("X-Test".into(), "a".into()), ("Content-Type".into(), "application/json".into())],
            body: br#"{"q":1}"#.to_vec(),
            stream: true,
        };
        let notified = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let n = notified.clone();
        let mut call = WinHttp.start(request, off(Duration::from_secs(10)), Arc::new(move || {
            n.fetch_add(1, Ordering::SeqCst);
        }));
        let events = collect(&mut call, Duration::from_secs(10));
        assert_eq!(
            events,
            [
                chunk("data: 1\n\n"),
                chunk("data: 2\n\n"),
                chunk("data: 3\n\n"),
                HttpEvent::Done { body: "data: 1\n\ndata: 2\n\ndata: 3\n\n".into(), error: None }
            ]
        );
        assert_eq!(notified.load(Ordering::SeqCst), 4, "one wake per event");
        let (head, body) = seen_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(head.starts_with("POST /v1/x?y=1 HTTP/1.1\r\n"), "{head}");
        let lower = head.to_ascii_lowercase();
        assert!(lower.contains("\r\nx-test: a\r\n"), "{head}");
        assert!(lower.contains("\r\ncontent-type: application/json\r\n"), "{head}");
        assert!(lower.contains("\r\ncontent-length: 7\r\n"), "{head}");
        assert!(lower.contains("\r\nuser-agent: tandem\r\n"), "{head}");
        assert_eq!(body, br#"{"q":1}"#);
        std::thread::sleep(Duration::from_millis(100));
        assert!(call.finished());
    }

    #[test]
    fn winhttp_reports_status() {
        const ERR: &str = r#"{"error":{"message":"bad key"}}"#;
        let base = serve(|mut s| {
            read_request(&mut s);
            let _ = s.write_all(format!("HTTP/1.1 401 Unauthorized\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{ERR}", ERR.len()).as_bytes());
        });
        let mut call = WinHttp.start(post(format!("{base}/v1/x"), true), off(Duration::from_secs(10)), no_notify());
        assert_eq!(collect(&mut call, Duration::from_secs(10)), [HttpEvent::Done { body: ERR.into(), error: Some("HTTP request failed with status 401.".into()) }]);

        // A server that closes without a response (WinHTTP may retry once on a new connection).
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for _ in 0..2 {
                if let Ok((mut s, _)) = listener.accept() {
                    read_request(&mut s);
                }
            }
        });
        let mut call = WinHttp.start(post(format!("http://127.0.0.1:{port}/v1/x"), true), off(Duration::from_secs(10)), no_notify());
        let events = collect(&mut call, Duration::from_secs(10));
        let accepted = [transport_error(&error_text(ERROR_WINHTTP_INVALID_SERVER_RESPONSE)), transport_error(&error_text(ERROR_WINHTTP_CONNECTION_ERROR))];
        match events.as_slice() {
            [HttpEvent::Done { body, error: Some(e) }] if body.is_empty() && accepted.contains(e) => println!("closed without a response: {e}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn winhttp_idle_timeout_is_between_bytes() {
        let base = serve(|mut s| {
            read_request(&mut s);
            s.write_all(STREAM_HEAD).unwrap();
            for _ in 0..5 {
                std::thread::sleep(Duration::from_millis(600));
                send_chunk(&mut s, "data: x\n\n").unwrap();
            }
            end_chunks(&mut s);
        });
        let mut call = WinHttp.start(post(format!("{base}/x"), true), off(Duration::from_secs(1)), no_notify());
        let events = collect(&mut call, Duration::from_secs(10));
        assert_eq!(events.last(), Some(&HttpEvent::Done { body: "data: x\n\n".repeat(5), error: None }), "a slow stream that keeps sending survives");

        let base = serve(|mut s| {
            read_request(&mut s);
            s.write_all(STREAM_HEAD).unwrap();
            send_chunk(&mut s, "data: 1\n\n").unwrap();
            std::thread::sleep(Duration::from_secs(3));
        });
        let start = Instant::now();
        let mut call = WinHttp.start(post(format!("{base}/x"), true), off(Duration::from_secs(1)), no_notify());
        let events = collect(&mut call, Duration::from_secs(10));
        assert!(start.elapsed() < Duration::from_millis(2500), "{:?} {events:?}", start.elapsed());
        assert_eq!(events, [chunk("data: 1\n\n"), HttpEvent::Done { body: "data: 1\n\n".into(), error: Some("HTTP request failed with The request timed out.".into()) }]);
    }

    /// A server that streams a chunk every 200 ms until the client goes away, then reports it.
    fn ticking(closed: mpsc::Sender<()>) -> String {
        serve(move |mut s| {
            read_request(&mut s);
            s.write_all(STREAM_HEAD).unwrap();
            for _ in 0..50 {
                if send_chunk(&mut s, "data: t\n\n").is_err() {
                    let _ = closed.send(());
                    return;
                }
                std::thread::sleep(Duration::from_millis(200));
            }
        })
    }

    #[test]
    fn winhttp_cancel_stops_only_its_call() {
        let (closed1_tx, closed1) = mpsc::channel();
        let (closed2_tx, _closed2) = mpsc::channel();
        let (base1, base2) = (ticking(closed1_tx), ticking(closed2_tx));
        let mut first = WinHttp.start(post(format!("{base1}/x"), true), off(Duration::from_secs(10)), no_notify());
        let mut second = WinHttp.start(post(format!("{base2}/x"), true), off(Duration::from_secs(10)), no_notify());
        assert_eq!(next_event(&mut first, Duration::from_secs(5)), Some(chunk("data: t\n\n")));
        assert_eq!(next_event(&mut second, Duration::from_secs(5)), Some(chunk("data: t\n\n")));
        first.cancel();
        let start = Instant::now();
        let mut more = 0;
        while start.elapsed() < Duration::from_secs(1) {
            assert_eq!(first.try_event(), None, "nothing after a cancel, not even Done");
            if let Some(e) = second.try_event() {
                assert_eq!(e, chunk("data: t\n\n"));
                more += 1;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(more >= 2, "the other call keeps streaming ({more})");
        assert!(closed1.recv_timeout(Duration::from_millis(500)).is_ok(), "the server saw the connection close");
        assert!(first.finished(), "the worker thread ended");
        assert!(!second.finished());
        second.cancel();
    }

    #[test]
    fn winhttp_ends_a_body_over_the_limit() {
        // A stream that never ends on its own: 1 MiB chunks until the client goes.
        let base = serve(|mut s| {
            read_request(&mut s);
            s.write_all(STREAM_HEAD).unwrap();
            let block = "a".repeat(1 << 20);
            for _ in 0..80 {
                if send_chunk(&mut s, &block).is_err() {
                    return;
                }
            }
            end_chunks(&mut s);
        });
        let mut call = WinHttp.start(post(format!("{base}/x"), true), off(Duration::from_secs(10)), no_notify());
        let events = collect(&mut call, Duration::from_secs(60));
        let streamed: usize = events.iter().map(|e| if let HttpEvent::Chunk(c) = e { c.len() } else { 0 }).sum();
        let Some(HttpEvent::Done { body, error }) = events.last() else { panic!("no Done in {} events", events.len()) };
        assert_eq!(error.as_deref(), Some(transport_error(crate::ai_transport::TOO_LARGE).as_str()));
        assert!(body.len() <= crate::ai_transport::MAX_BODY && body.len() > crate::ai_transport::MAX_BODY - (64 << 10), "{}", body.len());
        assert_eq!(streamed, body.len(), "every kept byte was streamed, nothing past the limit");
    }

    #[test]
    fn winhttp_cancel_ends_a_worker_waiting_on_a_full_queue() {
        let (closed_tx, closed) = mpsc::channel();
        let base = serve(move |mut s| {
            read_request(&mut s);
            s.write_all(STREAM_HEAD).unwrap();
            let block = "b".repeat(64 << 10);
            let start = Instant::now();
            while start.elapsed() < Duration::from_secs(10) {
                if send_chunk(&mut s, &block).is_err() {
                    break;
                }
            }
            let _ = closed_tx.send(());
        });
        let mut call = WinHttp.start(post(format!("{base}/x"), true), off(Duration::from_secs(10)), no_notify());
        // Nobody reads: the queue fills and the worker waits.
        std::thread::sleep(Duration::from_secs(1));
        assert!(!call.finished());
        call.cancel();
        let start = Instant::now();
        while !call.finished() && start.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(call.finished(), "the worker ended");
        assert_eq!(call.try_event(), None, "nothing after a cancel");
        assert!(closed.recv_timeout(Duration::from_secs(5)).is_ok(), "the server saw the connection close");
    }

    #[test]
    fn winhttp_bad_url() {
        let mut call = WinHttp.start(post("about:empty".into(), true), off(Duration::from_secs(1)), no_notify());
        assert!(call.finished(), "no thread");
        assert_eq!(call.try_event(), Some(HttpEvent::Done { body: String::new(), error: Some("Invalid URL".into()) }));

        // A URL the consent gate cannot name never connects, even where another parser would
        // find a loopback host in it.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        listener.set_nonblocking(true).unwrap();
        for url in [format!("http://evil.example\\@127.0.0.1:{port}/"), format!("http://evil%40@127.0.0.1:{port}/"), format!("http://127.0.0.1 :{port}/")] {
            let mut call = WinHttp.start(post(url.clone(), true), off(Duration::from_secs(1)), no_notify());
            assert!(call.finished(), "{url}: no thread");
            assert_eq!(call.try_event(), Some(HttpEvent::Done { body: String::new(), error: Some("Invalid URL".into()) }), "{url}");
        }
        std::thread::sleep(Duration::from_millis(300));
        assert!(listener.accept().is_err(), "nothing connected");
    }

    fn hooked(hook: impl Fn(usize, &Arc<Shared>) + Send + Sync + 'static) -> Shared {
        Shared { slot: new_slot(), hook: Some(Box::new(hook)) }
    }

    /// A server that streams three chunks and ends, ignoring a client that goes away.
    fn three_chunks() -> String {
        serve(|mut s| {
            read_request(&mut s);
            if s.write_all(STREAM_HEAD).is_err() {
                return;
            }
            for i in 1..=3 {
                std::thread::sleep(Duration::from_millis(20));
                if send_chunk(&mut s, &format!("data: {i}\n\n")).is_err() {
                    return;
                }
            }
            end_chunks(&mut s);
        })
    }

    fn whole_three() -> Vec<HttpEvent> {
        vec![chunk("data: 1\n\n"), chunk("data: 2\n\n"), chunk("data: 3\n\n"), HttpEvent::Done { body: "data: 1\n\ndata: 2\n\ndata: 3\n\n".into(), error: None }]
    }

    fn wait_finished(call: &dyn Call, limit: Duration) -> bool {
        let start = Instant::now();
        while start.elapsed() < limit {
            if call.finished() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        false
    }

    /// Controller ruling: no WinHTTP call ever receives a closed handle. A cancel lands before
    /// every call the worker makes on the request handle, and again just after it (while WinHTTP
    /// works); each time the worker ends, nothing is delivered, and a request started right after
    /// (which may be handed the freed handle value) arrives whole.
    #[test]
    fn winhttp_cancel_at_every_step() {
        let steps = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted = steps.clone();
        let mut call = start_call(post(format!("{}/x", three_chunks()), true), off(Duration::from_secs(10)), no_notify(), hooked(move |n, _| {
            counted.fetch_max(n + 1, Ordering::SeqCst);
        }));
        assert_eq!(collect(&mut call, Duration::from_secs(10)), whole_three());
        let steps = steps.load(Ordering::SeqCst);
        assert!(steps >= 8, "headers, send, receive, status, then queries and reads ({steps})");
        for k in 0..steps {
            for later in [false, true] {
                let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
                let flag = cancelled.clone();
                let hook = move |n: usize, shared: &Arc<Shared>| {
                    if n != k {
                        return;
                    }
                    let (shared, flag) = (shared.clone(), flag.clone());
                    let cancel = move || {
                        shared.cancel();
                        flag.store(true, Ordering::SeqCst);
                    };
                    if later {
                        std::thread::spawn(move || {
                            std::thread::sleep(Duration::from_millis(1));
                            cancel();
                        });
                    } else {
                        cancel();
                    }
                };
                let mut call = start_call(post(format!("{}/x", three_chunks()), true), off(Duration::from_secs(10)), no_notify(), hooked(hook));
                let start = Instant::now();
                while !cancelled.load(Ordering::SeqCst) && start.elapsed() < Duration::from_secs(10) {
                    std::thread::sleep(Duration::from_millis(1));
                }
                assert!(cancelled.load(Ordering::SeqCst), "step {k} later {later}: the cancel ran");
                assert!(wait_finished(call.as_ref(), Duration::from_secs(10)), "step {k} later {later}: the worker ended");
                assert_eq!(call.try_event(), None, "step {k} later {later}: nothing after the cancel");
                let mut next = WinHttp.start(post(format!("{}/x", three_chunks()), true), off(Duration::from_secs(10)), no_notify());
                assert_eq!(collect(&mut next, Duration::from_secs(10)), whole_three(), "step {k} later {later}: the next request is whole");
            }
        }
    }

    /// The idle timeout closes the handle the same safe way, before the response headers too.
    #[test]
    fn winhttp_timeout_leaves_the_next_call_whole() {
        let base = serve(|mut s| {
            read_request(&mut s);
            std::thread::sleep(Duration::from_secs(2));
        });
        let start = Instant::now();
        let mut call = WinHttp.start(post(format!("{base}/x"), true), off(Duration::from_millis(300)), no_notify());
        let events = collect(&mut call, Duration::from_secs(10));
        assert!(start.elapsed() < Duration::from_millis(1500), "{:?}", start.elapsed());
        assert_eq!(events, [HttpEvent::Done { body: String::new(), error: Some("HTTP request failed with The request timed out.".into()) }]);
        let mut next = WinHttp.start(post(format!("{}/x", three_chunks()), true), off(Duration::from_secs(10)), no_notify());
        assert_eq!(collect(&mut next, Duration::from_secs(10)), whole_three());
        assert!(wait_finished(call.as_ref(), Duration::from_secs(5)), "the timed-out worker ended");
    }

    /// Controller ruling: a redirect is followed only to the host the consent gate checked.
    #[test]
    fn redirects_stay_on_the_gated_host() {
        let t = |url: &str| parse_url(url).unwrap();
        let here = t("https://api.test:8443/v1/chat?x=1");
        assert_eq!(follow(&here, "https://API.test/v2"), Ok(t("https://api.test/v2")));
        assert_eq!(follow(&here, "/v3?y=1"), Ok(t("https://api.test:8443/v3?y=1")));
        assert_eq!(follow(&here, "other"), Ok(t("https://api.test:8443/v1/other")));
        assert_eq!(follow(&here, "//api.test/z"), Ok(t("https://api.test/z")));
        assert_eq!(follow(&t("http://[::1]:8080/a/b"), "c"), Ok(t("http://[::1]:8080/a/c")));
        assert_eq!(follow(&here, "https://evil.test/v1"), Err(redirect_refused("another host (evil.test)")));
        assert_eq!(follow(&here, "//evil.test/"), Err(redirect_refused("another host (evil.test)")));
        assert_eq!(follow(&here, "https://api.test.evil.test/"), Err(redirect_refused("another host (api.test.evil.test)")));
        assert_eq!(follow(&here, "http://api.test/v1"), Err(redirect_refused("plain http")));
        assert_eq!(follow(&here, "https://evil.test\\@api.test/"), Err(redirect_refused("an address Tandem cannot read")));
        assert_eq!(follow(&here, "ftp://api.test/"), Err(redirect_refused("an address Tandem cannot read")));
        // Relative references as RFC 3986 resolves them.
        assert_eq!(follow(&here, "?y=2"), Ok(t("https://api.test:8443/v1/chat?y=2")), "a query keeps the path");
        assert_eq!(follow(&here, "#top"), Ok(t("https://api.test:8443/v1/chat?x=1")), "a fragment alone keeps the address");
        assert_eq!(follow(&here, "other#top").map(|t| t.path), Ok("/v1/other".to_string()), "a fragment is never sent");
        assert_eq!(follow(&here, "../v2/./m?q=a/../b").map(|t| t.path), Ok("/v2/m?q=a/../b".to_string()), "dot segments go, the query is kept");
        assert_eq!(follow(&here, "/a/b/../../../c").map(|t| t.path), Ok("/c".to_string()));
        assert_eq!(follow(&here, "https://api.test/a/./b/..").map(|t| t.path), Ok("/a/".to_string()));
        assert_eq!(follow(&here, "./").map(|t| t.path), Ok("/v1/".to_string()));
        assert_eq!(follow(&here, "..").map(|t| t.path), Ok("/".to_string()));
        assert_eq!(without_dot_segments("/a//b/"), "/a//b/");

        // The same host on another port: followed, with the method, headers and body (307).
        let (seen_tx, seen_rx) = mpsc::channel();
        let second = serve(move |mut s| {
            seen_tx.send(read_request(&mut s)).unwrap();
            let _ = s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok");
        });
        let to = format!("{second}/y");
        let first = serve(move |mut s| {
            read_request(&mut s);
            let _ = s.write_all(format!("HTTP/1.1 307 Temporary Redirect\r\nLocation: {to}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").as_bytes());
        });
        let request = HttpRequest { method: "POST", url: format!("{first}/x"), headers: vec![("X-Key".into(), "k".into())], body: br#"{"q":1}"#.to_vec(), stream: false };
        let mut call = WinHttp.start(request, off(Duration::from_secs(10)), no_notify());
        assert_eq!(collect(&mut call, Duration::from_secs(10)), [HttpEvent::Done { body: "ok".into(), error: None }]);
        let (head, body) = seen_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(head.starts_with("POST /y HTTP/1.1\r\n"), "{head}");
        assert!(head.to_ascii_lowercase().contains("\r\nx-key: k\r\n"), "{head}");
        assert_eq!(body, br#"{"q":1}"#);

        // A 303 repeats as a GET without the body or its Content-Type; the fragment stays home.
        let (seen_tx, seen_rx) = mpsc::channel();
        let second = serve(move |mut s| {
            seen_tx.send(read_request(&mut s)).unwrap();
            let _ = s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok");
        });
        let to = format!("{second}/a/../y?z=1#frag");
        let first = serve(move |mut s| {
            read_request(&mut s);
            let _ = s.write_all(format!("HTTP/1.1 303 See Other\r\nLocation: {to}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").as_bytes());
        });
        let headers = vec![("Content-Type".into(), "application/json".into()), ("X-Key".into(), "k".into())];
        let request = HttpRequest { method: "POST", url: format!("{first}/x"), headers, body: br#"{"q":1}"#.to_vec(), stream: false };
        let mut call = WinHttp.start(request, off(Duration::from_secs(10)), no_notify());
        assert_eq!(collect(&mut call, Duration::from_secs(10)), [HttpEvent::Done { body: "ok".into(), error: None }]);
        let (head, body) = seen_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(head.starts_with("GET /y?z=1 HTTP/1.1\r\n"), "{head}");
        let lower = head.to_ascii_lowercase();
        assert!(lower.contains("\r\nx-key: k\r\n") && !lower.contains("content-type"), "{head}");
        assert!(body.is_empty());

        // Another host: not followed, and it sees no connection.
        let other = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = other.local_addr().unwrap().port();
        other.set_nonblocking(true).unwrap();
        let first = serve(move |mut s| {
            read_request(&mut s);
            let _ = s.write_all(format!("HTTP/1.1 302 Found\r\nLocation: http://localhost:{port}/\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").as_bytes());
        });
        let mut call = WinHttp.start(post(format!("{first}/x"), false), off(Duration::from_secs(10)), no_notify());
        assert_eq!(
            collect(&mut call, Duration::from_secs(10)),
            [HttpEvent::Done { body: String::new(), error: Some(transport_error(&redirect_refused("another host (localhost)"))) }]
        );
        std::thread::sleep(Duration::from_millis(300));
        assert!(other.accept().is_err(), "the other host saw no connection");
    }

    #[test]
    fn winhttp_error_texts() {
        assert_eq!(error_text(12002), "The request timed out.");
        assert_eq!(error_text(12007), "A server with the specified hostname could not be found.");
        assert_eq!(error_text(12029), "Could not connect to the server.");
        assert_eq!(error_text(12030), "The network connection was lost.");
        assert_eq!(error_text(12175), "An SSL error has occurred and a secure connection to the server cannot be made.");
        let other = error_text(ERROR_WINHTTP_INVALID_SERVER_RESPONSE);
        assert!(!other.is_empty() && !other.ends_with('\n'), "{other}");
        assert_eq!(error_text(0x7fff_0001), "WinHTTP error 2147418113");
    }
}
