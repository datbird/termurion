//! The HTTP transport contract for AI requests: the request and its events, the `Transport` and
//! `Call` traits, URL and proxy parsing, the UTF-8 carry across chunk boundaries and iTerm2's
//! error texts. Pure; ports `AITerm/JSFetch.swift` (`JF:35-173`). The real transport is
//! `ai_winhttp`; tests use `Recorded`, which answers recorded bytes through the same rules.

use crate::globals::Globals;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

/// Wakes the event loop when a call has an event (the same type as `pty::Notify`, which is
/// Windows-only).
pub type Notify = Arc<dyn Fn() + Send + Sync>;

/// One HTTP request. Header values are built at send time and may hold a key, so `Debug` prints
/// header names only and the URL without its query.
#[derive(Clone, PartialEq, Eq)]
pub struct HttpRequest {
    pub method: &'static str,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub stream: bool,
}

impl fmt::Debug for HttpRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Gemini's key rides in the query.
        let url = hide_query(&self.url);
        let names: Vec<&str> = self.headers.iter().map(|(n, _)| n.as_str()).collect();
        f.debug_struct("HttpRequest")
            .field("method", &self.method)
            .field("url", &url)
            .field("headers", &names)
            .field("body_bytes", &self.body.len())
            .field("stream", &self.stream)
            .finish()
    }
}

/// How a call reaches the server.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Options {
    /// The longest wait for the next byte; `Duration::ZERO` waits forever.
    pub idle: Duration,
    pub proxy: Proxy,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Proxy {
    /// The system's proxy settings (WPAD, PAC and the Windows proxy).
    System,
    /// `aiProxy`: a proxy for https requests only; plain http goes direct (`JF:149-160`).
    HttpsOnly { host: String, port: u16 },
    /// No proxy at all (the transport's own tests).
    Off,
}

/// `AI Timeout` in seconds as an idle timeout (`JF:131-140`; 0 or below waits forever,
/// Decision 15) and `aiProxy`.
pub fn options_from(g: &Globals) -> Options {
    let secs = g.number(crate::ai_settings::TIMEOUT).unwrap_or(1200.0);
    let idle = if secs > 0.0 { Duration::try_from_secs_f64(secs).unwrap_or(Duration::MAX) } else { Duration::ZERO };
    Options { idle, proxy: parse_proxy(&g.string("AiProxy")) }
}

/// "host:port" with an integer port is an https proxy; anything else is ignored (`JF:149-160`;
/// Swift's `split` drops an empty host or port).
pub fn parse_proxy(text: &str) -> Proxy {
    match text.split_once(':') {
        Some((host, port)) if !host.is_empty() => match port.parse::<u16>() {
            Ok(port) => Proxy::HttpsOnly { host: host.to_string(), port },
            Err(_) => Proxy::System,
        },
        _ => Proxy::System,
    }
}

/// What a call delivers (iTerm2's `WebResponse {data, error}`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HttpEvent {
    /// Streamed text as it arrives; only for a streaming request answered 200.
    Chunk(String),
    /// The whole body; `error` None is success.
    Done { body: String, error: Option<String> },
}

/// One request in flight.
pub trait Call: Send {
    /// The next event, if one has arrived.
    fn try_event(&mut self) -> Option<HttpEvent>;
    /// Stops this call only; no event arrives after it.
    fn cancel(&mut self);
    /// Whether the call's worker has ended.
    #[cfg(test)]
    fn finished(&self) -> bool;
}

pub trait Transport: Send + Sync {
    /// Starts a request with the largest body and no total deadline; `notify` is called whenever
    /// an event is ready.
    fn start(&self, request: HttpRequest, options: Options, notify: Notify) -> Box<dyn Call> {
        self.start_bounded(request, options, Bounds::default(), notify)
    }

    /// Starts a request that keeps at most `bounds.limit` bytes (the read that would pass it ends
    /// the call with `TOO_LARGE`) and ends at `bounds.total` however the bytes arrive.
    fn start_bounded(&self, request: HttpRequest, options: Options, bounds: Bounds, notify: Notify) -> Box<dyn Call>;
}

/// What one call may take beyond the idle time: the body it keeps (`limit`, at most `MAX_BODY`)
/// and an overall deadline (`total`), which an idle timeout alone cannot give (a server that sends
/// a byte every few seconds keeps a call alive for ever).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bounds {
    pub limit: usize,
    pub total: Option<Duration>,
}

impl Default for Bounds {
    fn default() -> Bounds {
        Bounds { limit: MAX_BODY, total: None }
    }
}

/// Where a request connects. `Debug` hides the query, where Gemini's key rides.
#[derive(Clone, PartialEq, Eq)]
pub struct Target {
    pub secure: bool,
    /// Exactly `ai_policy::host_of(url)`: the host the consent gate checked.
    pub host: String,
    pub port: u16,
    /// The path and query, "/" when empty; the fragment is not sent.
    pub path: String,
}

impl fmt::Debug for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Target").field("secure", &self.secure).field("host", &self.host).field("port", &self.port).field("path", &hide_query(&self.path)).finish()
    }
}

/// A URL or path with its query replaced by "<query hidden>".
fn hide_query(url: &str) -> String {
    match url.split_once('?') {
        Some((base, _)) => format!("{base}?<query hidden>"),
        None => url.to_string(),
    }
}

pub const INVALID_URL: &str = "Invalid URL";

/// An http or https URL (`JF:113-117`). The host comes from 12a's `ai_policy::url_parts`, the
/// parse `host_of` uses, so a URL the gate cannot name is never sent and the host connected to
/// is the host the person allowed. Userinfo is dropped (Decision 13).
pub fn parse_url(url: &str) -> Result<Target, String> {
    let invalid = || INVALID_URL.to_string();
    let parts = crate::ai_policy::url_parts(url).ok_or_else(invalid)?;
    let secure = match parts.scheme.to_ascii_lowercase().as_str() {
        "https" => true,
        "http" => false,
        _ => return Err(invalid()),
    };
    let port = match parts.after_host {
        "" | ":" => {
            if secure {
                443
            } else {
                80
            }
        }
        text => text.strip_prefix(':').filter(|p| p.bytes().all(|b| b.is_ascii_digit())).and_then(|p| p.parse::<u16>().ok()).ok_or_else(invalid)?,
    };
    let rest = parts.rest.split_once('#').map_or(parts.rest, |(before, _)| before);
    let path = if rest.starts_with('/') { rest.to_string() } else { format!("/{rest}") };
    Ok(Target { secure, host: parts.host, port, path })
}

/// A status other than 200 (`JF:52-57`).
pub fn status_error(code: u32) -> String {
    format!("HTTP request failed with status {code}.")
}

/// A transport failure (`JF:48-50`).
pub fn transport_error(text: &str) -> String {
    format!("HTTP request failed with {text}")
}

/// The largest body one call keeps; a longer answer ends the call with `TOO_LARGE` (final
/// review ruling: chat, discovery and Test Connection alike).
pub const MAX_BODY: usize = 64 * 1024 * 1024;

/// NSURLError's text for a body over the limit (`NSURLErrorDataLengthExceedsMaximum`).
pub const TOO_LARGE: &str = "resource exceeds maximum size";

/// How many events a call holds for its reader before its worker waits.
pub const EVENT_QUEUE: usize = 64;

/// Turns bytes into text across chunk boundaries: a character split between two reads comes out
/// whole with the second (iTerm2 drops such a chunk, ruling 12b-2).
#[derive(Default)]
pub struct Utf8Carry {
    pending: Vec<u8>,
}

impl Utf8Carry {
    pub fn new() -> Utf8Carry {
        Utf8Carry::default()
    }

    /// The complete characters so far. An invalid sequence becomes U+FFFD at once; a trailing
    /// incomplete sequence (at most 3 bytes) waits for the next push.
    pub fn push(&mut self, bytes: &[u8]) -> String {
        self.pending.extend_from_slice(bytes);
        let mut out = String::new();
        let mut at = 0;
        loop {
            match std::str::from_utf8(&self.pending[at..]) {
                Ok(s) => {
                    out.push_str(s);
                    at = self.pending.len();
                    break;
                }
                Err(e) => {
                    let good = at + e.valid_up_to();
                    out.push_str(std::str::from_utf8(&self.pending[at..good]).expect("valid prefix"));
                    match e.error_len() {
                        Some(n) => {
                            out.push('\u{FFFD}');
                            at = good + n;
                        }
                        None => {
                            at = good;
                            break;
                        }
                    }
                }
            }
        }
        self.pending.drain(..at);
        out
    }

    /// Whatever is left, lossy.
    pub fn finish(&mut self) -> String {
        let out = String::from_utf8_lossy(&self.pending).into_owned();
        self.pending.clear();
        out
    }
}

/// The delivery rules every transport follows (`JF:35-64`): each read is appended to the body
/// and, for a streaming request answered 200, passed on as text at once; the end is a transport
/// failure, a status other than 200, or success. A body over `MAX_BODY` is a failure.
pub struct Delivery {
    stream: bool,
    status: u32,
    carry: Utf8Carry,
    body: Vec<u8>,
    limit: usize,
}

impl Delivery {
    /// `status` is 0 until a response arrives.
    pub fn new(status: u32, stream: bool) -> Delivery {
        Delivery { stream, status, carry: Utf8Carry::new(), body: Vec::new(), limit: MAX_BODY }
    }

    /// The same with a smaller limit: a call's `Bounds::limit`, and the tests'.
    pub fn limited(status: u32, stream: bool, limit: usize) -> Delivery {
        Delivery { limit, ..Delivery::new(status, stream) }
    }

    fn streams(&self) -> bool {
        self.stream && self.status == 200
    }

    /// One read; a `Chunk` when it completes any characters of a streamed answer. Err(TOO_LARGE)
    /// when the body would pass the limit: the read is dropped and the call must end with it.
    pub fn read(&mut self, bytes: &[u8]) -> Result<Option<HttpEvent>, String> {
        if bytes.len() > self.limit - self.body.len() {
            return Err(TOO_LARGE.to_string());
        }
        self.body.extend_from_slice(bytes);
        if !self.streams() {
            return Ok(None);
        }
        let text = self.carry.push(bytes);
        Ok((!text.is_empty()).then_some(HttpEvent::Chunk(text)))
    }

    /// The end: an incomplete character left in the carry is streamed lossy, then `Done`.
    pub fn done(mut self, failure: Option<&str>) -> Vec<HttpEvent> {
        let mut out = Vec::new();
        if self.streams() {
            let rest = self.carry.finish();
            if !rest.is_empty() {
                out.push(HttpEvent::Chunk(rest));
            }
        }
        // Valid UTF-8, the usual case, is not copied again.
        let body = String::from_utf8(self.body).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned());
        let error = match failure {
            Some(text) => Some(transport_error(text)),
            None if self.status != 200 => Some(status_error(self.status)),
            None => None,
        };
        out.push(HttpEvent::Done { body, error });
        out
    }
}

/// Every event of a call that received `chunks` with `status`, then ended in `failure` or not.
pub fn events_for(status: u32, chunks: &[Vec<u8>], stream: bool, failure: Option<&str>) -> Vec<HttpEvent> {
    events_through(Delivery::new(status, stream), chunks, failure)
}

/// The same through `d`: a read over its limit ends the call.
fn events_through(mut d: Delivery, chunks: &[Vec<u8>], failure: Option<&str>) -> Vec<HttpEvent> {
    let mut out = Vec::new();
    for c in chunks {
        match d.read(c) {
            Ok(event) => out.extend(event),
            Err(text) => {
                out.extend(d.done(Some(&text)));
                return out;
            }
        }
    }
    out.extend(d.done(failure));
    out
}

/// The events of a request whose URL cannot be sent.
pub fn invalid_url_events() -> Vec<HttpEvent> {
    vec![HttpEvent::Done { body: String::new(), error: Some(INVALID_URL.to_string()) }]
}

/// A recorded answer for `Recorded`.
#[cfg(test)]
#[derive(Clone)]
pub struct Reply {
    pub status: u32,
    pub chunks: Vec<Vec<u8>>,
    pub failure: Option<String>,
    /// Bytes in the final body that no chunk delivered: iTerm2's race, where the completion
    /// arrives before the last chunks were read.
    pub late: Vec<u8>,
}

#[cfg(test)]
impl Reply {
    /// 200 with these reads.
    pub fn ok(chunks: &[&[u8]]) -> Reply {
        Reply { status: 200, chunks: chunks.iter().map(|c| c.to_vec()).collect(), failure: None, late: Vec::new() }
    }

    /// `code` with the whole body in one read.
    pub fn status(code: u32, body: &[u8]) -> Reply {
        Reply { status: code, chunks: vec![body.to_vec()], failure: None, late: Vec::new() }
    }

    /// No response: the transport failed with `text` (for example "The request timed out.").
    pub fn failed(text: &str) -> Reply {
        Reply { status: 0, chunks: Vec::new(), failure: Some(text.to_string()), late: Vec::new() }
    }

    /// 200 whose final body holds `late` after what the chunks delivered.
    pub fn raced(chunks: &[&[u8]], late: &[u8]) -> Reply {
        Reply { late: late.to_vec(), ..Reply::ok(chunks) }
    }
}

/// A transport that answers recorded bytes: each `start` takes the next reply, its events are
/// ready at once and `notify` is called once. A URL the real transport refuses is refused the
/// same way, without taking a reply or recording the request.
#[cfg(test)]
pub struct Recorded {
    replies: std::sync::Mutex<std::collections::VecDeque<Reply>>,
    requests: std::sync::Mutex<Vec<HttpRequest>>,
    options: std::sync::Mutex<Vec<Options>>,
    bounds: std::sync::Mutex<Vec<Bounds>>,
    cancels: Arc<std::sync::atomic::AtomicUsize>,
}

#[cfg(test)]
impl Recorded {
    pub fn new(replies: Vec<Reply>) -> Recorded {
        Recorded { replies: std::sync::Mutex::new(replies.into()), requests: std::sync::Mutex::new(Vec::new()), options: std::sync::Mutex::new(Vec::new()), bounds: std::sync::Mutex::new(Vec::new()), cancels: Arc::default() }
    }

    /// How many of its calls were cancelled.
    pub fn cancels(&self) -> usize {
        self.cancels.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// The bounds each call was started with, in order.
    pub fn bounds(&self) -> Vec<Bounds> {
        self.bounds.lock().unwrap().clone()
    }

    /// The requests sent so far, in order.
    pub fn requests(&self) -> Vec<HttpRequest> {
        self.requests.lock().unwrap().clone()
    }

    /// The options each sent request was started with, in the same order.
    pub fn options(&self) -> Vec<Options> {
        self.options.lock().unwrap().clone()
    }
}

#[cfg(test)]
struct RecordedCall {
    events: std::collections::VecDeque<HttpEvent>,
    cancels: Arc<std::sync::atomic::AtomicUsize>,
}

#[cfg(test)]
impl Call for RecordedCall {
    fn try_event(&mut self) -> Option<HttpEvent> {
        self.events.pop_front()
    }

    fn cancel(&mut self) {
        self.events.clear();
        self.cancels.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }

    fn finished(&self) -> bool {
        true
    }
}

#[cfg(test)]
impl Transport for Recorded {
    fn start_bounded(&self, request: HttpRequest, options: Options, bounds: Bounds, notify: Notify) -> Box<dyn Call> {
        let events = if parse_url(&request.url).is_err() {
            invalid_url_events()
        } else {
            let reply = self.replies.lock().unwrap().pop_front().expect("Recorded has a reply for every request");
            let mut events = events_through(Delivery::limited(reply.status, request.stream, bounds.limit), &reply.chunks, reply.failure.as_deref());
            if let Some(HttpEvent::Done { body, .. }) = events.last_mut() {
                body.push_str(&String::from_utf8_lossy(&reply.late));
            }
            self.requests.lock().unwrap().push(request);
            self.options.lock().unwrap().push(options);
            self.bounds.lock().unwrap().push(bounds);
            events
        };
        notify();
        Box::new(RecordedCall { events: events.into(), cancels: self.cancels.clone() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai_policy::host_of;
    use toml::Value;

    fn chunks(parts: &[&[u8]]) -> Vec<Vec<u8>> {
        parts.iter().map(|p| p.to_vec()).collect()
    }

    fn done(body: &str, error: Option<&str>) -> HttpEvent {
        HttpEvent::Done { body: body.to_string(), error: error.map(str::to_string) }
    }

    fn chunk(s: &str) -> HttpEvent {
        HttpEvent::Chunk(s.to_string())
    }

    #[test]
    fn utf8_carry_keeps_split_characters() {
        let mut c = Utf8Carry::new();
        assert_eq!(c.push(b"h\xc3"), "h");
        assert_eq!(c.push(b"\xa9!"), "\u{e9}!");
        let smile = "\u{1F600}".as_bytes();
        assert_eq!(smile.len(), 4);
        assert_eq!(c.push(&smile[..1]), "");
        assert_eq!(c.push(&smile[1..2]), "");
        assert_eq!(c.push(&smile[2..3]), "");
        assert_eq!(c.push(&smile[3..]), "\u{1F600}");
        assert_eq!(c.push(b"\xff"), "\u{FFFD}");
        assert_eq!(c.push(b"a\xff\xc3"), "a\u{FFFD}", "an invalid byte in the middle is replaced, the tail waits");
        assert_eq!(c.push(b"\xa9"), "\u{e9}");
        assert_eq!(c.push(b"a\xe2\x82"), "a");
        assert_eq!(c.finish(), "\u{FFFD}");
        assert_eq!(c.finish(), "");
    }

    #[test]
    fn urls_follow_iterm2() {
        let t = |secure: bool, host: &str, port: u16, path: &str| Ok(Target { secure, host: host.into(), port, path: path.into() });
        assert_eq!(parse_url("https://api.openai.com/v1/responses"), t(true, "api.openai.com", 443, "/v1/responses"));
        assert_eq!(parse_url("http://localhost:11434/api/chat"), t(false, "localhost", 11434, "/api/chat"));
        assert_eq!(parse_url("https://h.test"), t(true, "h.test", 443, "/"));
        assert_eq!(parse_url("https://h.test/p?key=AIza1&alt=sse"), t(true, "h.test", 443, "/p?key=AIza1&alt=sse"));
        assert_eq!(parse_url("http://user:pw@[::1]:8080/x"), t(false, "::1", 8080, "/x"));
        assert_eq!(parse_url("http://h.test?q=1"), t(false, "h.test", 80, "/?q=1"));
        assert_eq!(parse_url("http://h.test/x#frag"), t(false, "h.test", 80, "/x"));
        assert_eq!(parse_url("HTTPS://H.Test./x"), t(true, "h.test", 443, "/x"));
        for bad in ["ftp://h/x", "about:empty", "https://", "", "http://h:http/", "http://h:70000/", "http://h:1:2/", "http://[::1]junk/", "http://h:+80/"] {
            assert_eq!(parse_url(bad), Err(INVALID_URL.to_string()), "{bad}");
        }
    }

    /// Controller ruling: the transport connects only to the host the consent gate names.
    #[test]
    fn target_host_is_the_gate_host() {
        let mut accepted: Vec<String> = [
            "https://api.openai.com/v1/responses",
            "http://localhost:11434/api/chat",
            "http://user:pw@[::1]:8080/x",
            "https://API.OpenAI.com./v1",
            "http://user:pw@192.168.0.2:1234/",
            "http://h?x=1",
            "http://localhost:11434/a%20b\\c d",
            "https://x.openai.azure.com/o",
        ]
        .map(String::from)
        .to_vec();
        accepted.extend(crate::ai_catalog::bundled().models.iter().filter(|m| !m.url.is_empty()).map(|m| m.url.replace("{{MODEL}}", &m.name)));
        for url in &accepted {
            let target = parse_url(url).unwrap_or_else(|e| panic!("{url}: {e}"));
            assert_eq!(Some(target.host), host_of(url), "{url}");
        }
        // Every URL the gate cannot name is refused, and nothing is sent.
        let refused = [
            "http://evil.example\\@localhost/v1/chat/completions",
            "http://evil.example\\localhost/",
            "http://evil example@localhost/",
            "http://localhost\t/",
            "http://localhost\n.evil.example/",
            "http://evil\u{7f}@localhost/",
            "http://evil.example%40@localhost/",
            "http://%6cocalhost/",
            "https:///path",
            "localhost:11434",
        ];
        let recorded = Recorded::new(Vec::new());
        for url in refused {
            assert_eq!(host_of(url), None, "{url}");
            assert_eq!(parse_url(url), Err(INVALID_URL.to_string()), "{url}");
            let mut call = recorded.start(HttpRequest { method: "POST", url: url.into(), headers: vec![], body: vec![], stream: true }, options(), Arc::new(|| {}));
            assert_eq!(call.try_event(), Some(done("", Some(INVALID_URL))), "{url}");
        }
        assert!(recorded.requests().is_empty(), "a URL the gate refuses is never sent");
    }

    fn options() -> Options {
        Options { idle: Duration::from_secs(1200), proxy: Proxy::System }
    }

    #[test]
    fn proxy_follows_iterm2() {
        assert_eq!(parse_proxy(""), Proxy::System);
        assert_eq!(parse_proxy("proxy.test:8080"), Proxy::HttpsOnly { host: "proxy.test".into(), port: 8080 });
        assert_eq!(parse_proxy("proxy.test"), Proxy::System);
        assert_eq!(parse_proxy("proxy.test:http"), Proxy::System);
        assert_eq!(parse_proxy("proxy.test:70000"), Proxy::System);
        assert_eq!(parse_proxy(":8080"), Proxy::System);
        assert_eq!(parse_proxy("proxy.test:"), Proxy::System);

        let mut g = Globals::empty(&std::env::temp_dir().join("tandem-ai-transport-test.toml"));
        assert_eq!(options_from(&g), Options { idle: Duration::from_secs(1200), proxy: Proxy::System });
        g.set("AI Timeout", Value::Float(2.5));
        assert_eq!(options_from(&g).idle, Duration::from_millis(2500));
        g.set("AI Timeout", Value::Integer(0));
        assert_eq!(options_from(&g).idle, Duration::ZERO);
        g.set("AI Timeout", Value::Integer(-3));
        assert_eq!(options_from(&g).idle, Duration::ZERO);
        g.set("AiProxy", Value::String("proxy.test:8080".into()));
        assert_eq!(options_from(&g).proxy, Proxy::HttpsOnly { host: "proxy.test".into(), port: 8080 });
    }

    #[test]
    fn events_follow_iterm2() {
        let sse = chunks(&[b"data: a\n", b"\n"]);
        assert_eq!(events_for(200, &sse, true, None), [chunk("data: a\n"), chunk("\n"), done("data: a\n\n", None)]);
        assert_eq!(events_for(200, &sse, false, None), [done("data: a\n\n", None)]);
        let err = br#"{"error":{"message":"bad key"}}"#;
        assert_eq!(
            events_for(401, &chunks(&[err]), true, None),
            [done(r#"{"error":{"message":"bad key"}}"#, Some("HTTP request failed with status 401."))]
        );
        assert_eq!(events_for(201, &chunks(&[b"{}"]), true, None), [done("{}", Some("HTTP request failed with status 201."))]);
        assert_eq!(
            events_for(200, &chunks(&[b"data: a\n"]), true, Some("The request timed out.")),
            [chunk("data: a\n"), done("data: a\n", Some("HTTP request failed with The request timed out."))]
        );
        // A chunk that ends inside a character: the character comes with the next chunk.
        assert_eq!(events_for(200, &chunks(&[b"caf\xc3", b"\xa9!"]), true, None), [chunk("caf"), chunk("\u{e9}!"), done("caf\u{e9}!", None)]);
        // A body that ends inside a character: the rest is streamed lossy before Done.
        assert_eq!(events_for(200, &chunks(&[b"a\xe2\x82"]), true, None), [chunk("a"), chunk("\u{FFFD}"), done("a\u{FFFD}", None)]);
        assert_eq!(events_for(0, &[], true, Some("Could not connect to the server.")), [done("", Some("HTTP request failed with Could not connect to the server."))]);
    }

    #[test]
    fn a_body_over_the_limit_ends_the_call() {
        let limited = |status, stream| events_through(Delivery::limited(status, stream, 8), &chunks(&[b"data: ", b"ab", b"c"]), None);
        let too_large = Some("HTTP request failed with resource exceeds maximum size");
        assert_eq!(limited(200, true), [chunk("data: "), chunk("ab"), done("data: ab", too_large)], "the read that passes the limit is dropped");
        assert_eq!(limited(200, false), [done("data: ab", too_large)]);
        assert_eq!(limited(500, true), [done("data: ab", too_large)], "an error body too");
        let exact = events_through(Delivery::limited(200, false, 8), &chunks(&[b"data: ", b"ab"]), None);
        assert_eq!(exact, [done("data: ab", None)], "exactly the limit is kept");
        assert_eq!(MAX_BODY, 64 << 20);
    }

    #[test]
    fn request_debug_hides_values() {
        let r = HttpRequest {
            method: "POST",
            url: "https://generativelanguage.googleapis.com/v1beta/models/m:streamGenerateContent?key=AIzaSECRET&alt=sse".into(),
            headers: vec![("Authorization".into(), "Bearer sk-secret-0123456789abcdef".into())],
            body: b"{}".to_vec(),
            stream: true,
        };
        let text = format!("{r:?}");
        assert!(text.contains("Authorization"), "the header name is shown");
        assert!(!text.contains("sk-secret"), "the Authorization value is hidden");
        assert!(!text.contains("AIzaSECRET"), "the query is hidden");
        let target = format!("{:?}", parse_url(&r.url).expect("a Gemini URL parses"));
        assert!(target.contains("streamGenerateContent"), "the path is shown");
        assert!(!target.contains("AIzaSECRET"), "the target's query is hidden");
    }

    #[test]
    fn recorded_follows_the_transport_rules() {
        let t = Recorded::new(vec![Reply::ok(&[b"data: \xc3", b"\xa9\n\n"]), Reply::status(500, b"oops"), Reply::failed("The request timed out.")]);
        let notified = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let n = notified.clone();
        let notify: Notify = Arc::new(move || {
            n.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        });
        let req = |stream| HttpRequest { method: "POST", url: "https://h.test/x".into(), headers: vec![], body: b"{}".to_vec(), stream };
        let mut a = t.start(req(true), options(), notify.clone());
        assert_eq!(notified.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(a.try_event(), Some(chunk("data: ")));
        assert_eq!(a.try_event(), Some(chunk("\u{e9}\n\n")));
        assert_eq!(a.try_event(), Some(done("data: \u{e9}\n\n", None)));
        assert_eq!(a.try_event(), None);
        assert!(a.finished());
        let mut b = t.start(req(true), options(), notify.clone());
        assert_eq!(b.try_event(), Some(done("oops", Some("HTTP request failed with status 500."))));
        let mut c = t.start(req(false), options(), notify);
        c.cancel();
        assert_eq!(c.try_event(), None, "nothing after a cancel");
        assert_eq!(t.requests().len(), 3);
        assert_eq!(t.requests()[2].stream, false);
    }

    #[test]
    fn texts_follow_iterm2() {
        assert_eq!(status_error(401), "HTTP request failed with status 401.");
        assert_eq!(transport_error("The request timed out."), "HTTP request failed with The request timed out.");
        assert_eq!(INVALID_URL, "Invalid URL");
    }

    /// A call's body limit is the transport's, while it streams, not a check after delivery.
    #[test]
    fn a_body_limit_ends_the_call() {
        let t = Recorded::new(vec![Reply::ok(&[b"abcd", b"efgh", b"ijkl"])]);
        let req = HttpRequest { method: "GET", url: "https://a.test/x".into(), headers: vec![], body: vec![], stream: false };
        let mut call = t.start_bounded(req, Options { idle: Duration::ZERO, proxy: Proxy::Off }, Bounds { limit: 8, total: None }, Arc::new(|| {}));
        let mut last = None;
        while let Some(e) = call.try_event() {
            last = Some(e);
        }
        let Some(HttpEvent::Done { body, error }) = last else { panic!("no Done") };
        assert_eq!(body, "abcdefgh", "nothing past the limit is kept");
        assert_eq!(error.as_deref(), Some(transport_error(TOO_LARGE).as_str()));
        assert_eq!(Bounds::default(), Bounds { limit: MAX_BODY, total: None });
    }
}
