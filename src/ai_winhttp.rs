//! The AI transport on WinHTTP (ruling 4): one worker thread per call runs a synchronous WinHTTP
//! request with the system proxy and TLS, posts each read as it arrives and ends with `Done`.
//! Cancelling a call closes its request handle, which aborts the worker's blocking call; nothing
//! is posted after a cancel. The idle timeout is kept by a watchdog that closes the handle the
//! same way, because a synchronous `WinHttpQueryDataAvailable` waits for the next chunk past
//! WinHTTP's own receive timeout. Ports `AITerm/JSFetch.swift` (`JF:35-173`).

use crate::ai_transport::{invalid_url_events, parse_url, Call, Delivery, HttpEvent, HttpRequest, Notify, Options, Proxy, Target, Transport};
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::GetLastError;
use windows_sys::Win32::Networking::WinHttp::*;

/// `WINHTTP_DECOMPRESSION_FLAG_GZIP | WINHTTP_DECOMPRESSION_FLAG_DEFLATE` (URLSession decodes
/// both too).
const DECOMPRESSION_ALL: u32 = WINHTTP_DECOMPRESSION_FLAG_GZIP | WINHTTP_DECOMPRESSION_FLAG_DEFLATE;
/// Each read's buffer.
const READ_SIZE: usize = 64 * 1024;

/// The WinHTTP transport.
pub struct WinHttp;

/// What a call and its worker share.
struct Shared {
    cancelled: AtomicBool,
    /// The watchdog closed the request because no byte came within the idle time.
    timed_out: AtomicBool,
    /// The request handle while the worker has one; whoever takes it closes it, exactly once.
    request: Mutex<Option<usize>>,
    watch: Mutex<Watch>,
    wake: Condvar,
}

/// The idle clock: the last sign of progress, and whether the worker is done.
struct Watch {
    last: Instant,
    done: bool,
}

impl Shared {
    /// Takes and closes the request handle, if nobody has yet.
    fn close_request(&self) {
        if let Some(h) = self.request.lock().unwrap_or_else(|e| e.into_inner()).take() {
            unsafe { WinHttpCloseHandle(h as *mut c_void) };
        }
    }

    /// Progress: the idle time starts again.
    fn pet(&self) {
        self.watch.lock().unwrap_or_else(|e| e.into_inner()).last = Instant::now();
    }

    /// Closes the request once `idle` passes without progress (URLSession's
    /// `timeoutIntervalForRequest`, `JF:131-140`); returns when the worker is done.
    fn watchdog(&self, idle: Duration) {
        let mut w = self.watch.lock().unwrap_or_else(|e| e.into_inner());
        while !w.done {
            let now = Instant::now();
            let deadline = w.last + idle;
            if now >= deadline {
                self.timed_out.store(true, Ordering::SeqCst);
                drop(w);
                self.close_request();
                return;
            }
            w = self.wake.wait_timeout(w, deadline - now).unwrap_or_else(|e| e.into_inner()).0;
        }
    }
}

struct WinCall {
    events: Receiver<HttpEvent>,
    shared: Arc<Shared>,
    /// Read only by the tests' `finished`: a dropped call never waits for its worker.
    #[cfg_attr(not(test), allow(dead_code))]
    thread: Option<JoinHandle<()>>,
    cancelled: bool,
}

impl Call for WinCall {
    fn try_event(&mut self) -> Option<HttpEvent> {
        if self.cancelled {
            return None;
        }
        self.events.try_recv().ok()
    }

    fn cancel(&mut self) {
        if self.cancelled {
            return;
        }
        self.cancelled = true;
        self.shared.cancelled.store(true, Ordering::SeqCst);
        // Closing the handle from this thread aborts the worker's blocking call
        // (ERROR_WINHTTP_OPERATION_CANCELLED).
        self.shared.close_request();
    }

    #[cfg(test)]
    fn finished(&self) -> bool {
        self.thread.as_ref().is_none_or(|h| h.is_finished())
    }
}

impl Drop for WinCall {
    /// A call nobody reads any more stops.
    fn drop(&mut self) {
        self.cancel();
    }
}

impl Transport for WinHttp {
    fn start(&self, request: HttpRequest, options: Options, notify: Notify) -> Box<dyn Call> {
        let shared = Arc::new(Shared {
            cancelled: AtomicBool::new(false),
            timed_out: AtomicBool::new(false),
            request: Mutex::new(None),
            watch: Mutex::new(Watch { last: Instant::now(), done: false }),
            wake: Condvar::new(),
        });
        let (tx, rx) = channel();
        let thread = match parse_url(&request.url) {
            Err(_) => {
                for e in invalid_url_events() {
                    let _ = tx.send(e);
                }
                notify();
                None
            }
            Ok(target) => {
                let worker = Worker { shared: shared.clone(), tx, notify };
                Some(std::thread::spawn(move || worker.run(&request, &target, &options)))
            }
        };
        Box::new(WinCall { events: rx, shared, thread, cancelled: false })
    }
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

struct Worker {
    shared: Arc<Shared>,
    tx: Sender<HttpEvent>,
    notify: Notify,
}

impl Worker {
    fn cancelled(&self) -> bool {
        self.shared.cancelled.load(Ordering::SeqCst)
    }

    fn post(&self, event: HttpEvent) {
        if !self.cancelled() && self.tx.send(event).is_ok() {
            (self.notify)();
        }
    }

    fn run(self, request: &HttpRequest, target: &Target, options: &Options) {
        let watchdog = (!options.idle.is_zero()).then(|| {
            let (shared, idle) = (self.shared.clone(), options.idle);
            std::thread::spawn(move || shared.watchdog(idle))
        });
        let mut delivery = Delivery::new(0, request.stream);
        let mut failure = self.exchange(request, target, options, &mut delivery).err();
        self.shared.watch.lock().unwrap_or_else(|e| e.into_inner()).done = true;
        self.shared.wake.notify_all();
        if let Some(w) = watchdog {
            let _ = w.join();
        }
        // The worker closes the request handle unless a cancel or the watchdog already did.
        self.shared.close_request();
        if self.cancelled() {
            return;
        }
        // The watchdog's close fails the blocking call with "cancelled"; a call that completed
        // just before the watchdog fired keeps its success.
        if failure.is_some() && self.shared.timed_out.load(Ordering::SeqCst) {
            failure = Some(ERROR_WINHTTP_TIMEOUT);
        }
        let text = failure.map(error_text);
        for event in delivery.done(text.as_deref()) {
            self.post(event);
        }
    }

    /// The request from open to the last read; Err is the WinHTTP error code.
    fn exchange(&self, request: &HttpRequest, target: &Target, options: &Options, delivery: &mut Delivery) -> Result<(), u32> {
        let check = |ok: i32| if ok != 0 { Ok(()) } else { Err(last_error()) };
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
        let session = Owned(unsafe { WinHttpOpen(agent.as_ptr(), access, proxy_ptr, std::ptr::null(), 0) });
        if session.0.is_null() {
            return Err(last_error());
        }
        // WinHTTP's own timeouts bound name resolution, connecting, sending and the wait for the
        // headers; the watchdog bounds the wait for each chunk.
        let t = if options.idle.is_zero() { 0 } else { options.idle.as_millis().clamp(1, i32::MAX as u128) as i32 };
        check(unsafe { WinHttpSetTimeouts(session.0, t, t, t, t) })?;
        let flags = DECOMPRESSION_ALL;
        unsafe { WinHttpSetOption(session.0, WINHTTP_OPTION_DECOMPRESSION, (&flags as *const u32).cast(), 4) };

        let host = wide(&target.host);
        let connect = Owned(unsafe { WinHttpConnect(session.0, host.as_ptr(), target.port, 0) });
        if connect.0.is_null() {
            return Err(last_error());
        }
        let (verb, path) = (wide(request.method), wide(&target.path));
        let secure = if target.secure { WINHTTP_FLAG_SECURE } else { 0 };
        let handle = unsafe { WinHttpOpenRequest(connect.0, verb.as_ptr(), path.as_ptr(), std::ptr::null(), std::ptr::null(), std::ptr::null(), secure) };
        if handle.is_null() {
            return Err(last_error());
        }
        {
            let mut slot = self.shared.request.lock().unwrap_or_else(|e| e.into_inner());
            if self.cancelled() || self.shared.timed_out.load(Ordering::SeqCst) {
                unsafe { WinHttpCloseHandle(handle) };
                return Err(ERROR_WINHTTP_TIMEOUT);
            }
            *slot = Some(handle as usize);
        }

        let lines: String = request
            .headers
            .iter()
            .filter(|(n, v)| crate::ai_policy::header_name_valid(n) && crate::ai_policy::header_value_valid(v))
            .map(|(n, v)| format!("{n}: {v}\r\n"))
            .collect();
        if !lines.is_empty() {
            let lines: Vec<u16> = lines.encode_utf16().collect();
            check(unsafe { WinHttpAddRequestHeaders(handle, lines.as_ptr(), lines.len() as u32, WINHTTP_ADDREQ_FLAG_ADD | WINHTTP_ADDREQ_FLAG_REPLACE) })?;
        }
        let len = request.body.len() as u32;
        let body = if request.body.is_empty() { std::ptr::null() } else { request.body.as_ptr().cast() };
        // WinHTTP writes Content-Length.
        check(unsafe { WinHttpSendRequest(handle, std::ptr::null(), 0, body, len, len, 0) })?;
        self.shared.pet();
        check(unsafe { WinHttpReceiveResponse(handle, std::ptr::null_mut()) })?;
        self.shared.pet();
        let (mut status, mut size) = (0u32, 4u32);
        check(unsafe {
            WinHttpQueryHeaders(
                handle,
                WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
                std::ptr::null(),
                (&mut status as *mut u32).cast(),
                &mut size,
                std::ptr::null_mut(),
            )
        })?;
        *delivery = Delivery::new(status, request.stream);

        let mut buf = vec![0u8; READ_SIZE];
        loop {
            let mut available = 0u32;
            check(unsafe { WinHttpQueryDataAvailable(handle, &mut available) })?;
            if available == 0 {
                return Ok(());
            }
            let want = (available as usize).min(READ_SIZE) as u32;
            let mut read = 0u32;
            check(unsafe { WinHttpReadData(handle, buf.as_mut_ptr().cast(), want, &mut read) })?;
            if read == 0 {
                return Ok(());
            }
            self.shared.pet();
            if let Some(event) = delivery.read(&buf[..read as usize]) {
                self.post(event);
            }
        }
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

    fn off(idle: Duration) -> Options {
        Options { idle, proxy: Proxy::Off }
    }

    fn no_notify() -> Notify {
        Arc::new(|| {})
    }

    /// A one-connection server on loopback running `script`; the URL base is "http://127.0.0.1:port".
    fn serve(script: impl FnOnce(TcpStream) + Send + 'static) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            if let Ok((s, _)) = listener.accept() {
                script(s);
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
