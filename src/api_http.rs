//! The HTTP side of iTerm2's Python API server: the request parser (`HC:139-235`), the WebSocket
//! upgrade checks in iTerm2's order with its reasons and status codes (`S:497-528`, `W:74-175`,
//! `S:604-608`) and the responses (`HC:85-132`, `W:520-547`). Pure; the sockets are
//! `api_server`'s.

use std::collections::BTreeMap;
use std::time::Duration;

use crate::api_proto::{PROTOCOL_VERSION, SUBPROTOCOL};

/// iTerm2's API port.
pub const PORT: u16 = 1912;

/// The most bytes read while waiting for the end of the request head.
pub const MAX_REQUEST: usize = 64 * 1024;

/// How long a connection has to send its request (`HC:135-137`).
pub const REQUEST_DEADLINE: Duration = Duration::from_secs(30);

/// The oldest `python` library iTerm2 accepts (`W:31`).
pub const MIN_PYTHON_LIBRARY: &str = "0.24";

/// The WebSocket version iTerm2 speaks (`W:21`).
pub const WEBSOCKET_VERSION: u32 = 13;

/// The reason logged when no request could be read (`S:505`).
pub const READ_FAILED: &str = "Failed to read request from connection";

/// The start of the one refusal that answers 406 instead of 400 (`W:22`, `S:604-608`).
const TOO_OLD: &str = "Library version too old";

/// iTerm2's longest header line (`HC:214`) and header count (`HC:190`).
const MAX_LINE: usize = 4096;
const MAX_HEADERS: usize = 100;

/// A parsed request head.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub method: String,
    /// The request target as sent (`url_path` gives the path iTerm2 compares).
    pub path: String,
    /// Names lowercased, values trimmed; a repeated header's last value wins.
    pub headers: BTreeMap<String, String>,
}

/// A request that passed every check, with what the rest of the server needs from it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Upgrade {
    /// `x-iterm2-key`, or a fresh key when absent.
    pub key: String,
    pub ws_key: String,
    pub ws_version: u32,
    /// A known `x-iterm2-cookie` was presented (and consumed).
    pub preauthorized: bool,
    pub advisory_name: Option<String>,
    pub library_version: String,
    /// `x-iterm2-disable-auth-ui` was present, whatever its value (`S:540`).
    pub disable_auth_ui: bool,
    /// The known cookie the client presented (a secret: never shown by `Debug`), kept so the app
    /// can tell whose cookie it was.
    pub cookie: Option<Presented>,
}

/// A cookie text a client presented. `Debug` hides it.
#[derive(Clone, PartialEq, Eq)]
pub struct Presented(String);

impl Presented {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Presented {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Presented(..)")
    }
}

/// Why an upgrade was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    BadRequest(String),
    Unacceptable(String),
}

impl Refusal {
    pub fn code(&self) -> u16 {
        match self {
            Refusal::BadRequest(_) => 400,
            Refusal::Unacceptable(_) => 406,
        }
    }

    pub fn reason(&self) -> &str {
        match self {
            Refusal::BadRequest(r) | Refusal::Unacceptable(r) => r,
        }
    }
}

/// The index just past the `\r\n\r\n` that ends a request head.
pub fn head_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n").map(|i| i + 4)
}

/// Parses a request head as iTerm2's `readRequest` does (`HC:139-235`): a request line of exactly
/// three space-separated parts ending in `HTTP/1.1`, lines of at most 4096 bytes read as Latin-1,
/// fewer than 100 headers, each with a colon that is not its last character, and no
/// `content-length`. None where it gives up.
pub fn parse(head: &[u8]) -> Option<Request> {
    let head = head.strip_suffix(b"\r\n\r\n").unwrap_or(head);
    let mut lines = crlf_lines(head);
    let line = latin1(lines.next()?)?;
    let parts: Vec<&str> = line.split(' ').collect();
    let [method, path, protocol] = parts[..] else {
        return None;
    };
    if protocol != "HTTP/1.1" {
        return None;
    }
    let mut headers = BTreeMap::new();
    for line in lines {
        if line.is_empty() {
            break;
        }
        let line = latin1(line)?;
        let colon = line.find(':')?;
        if colon + 1 == line.len() {
            return None;
        }
        headers.insert(line[..colon].to_lowercase(), trim(&line[colon + 1..]).to_string());
        if headers.len() == MAX_HEADERS {
            return None;
        }
    }
    if headers.contains_key("content-length") {
        return None;
    }
    Some(Request { method: method.to_string(), path: path.to_string(), headers })
}

/// The lines of a head split at each `\r\n` only, as iTerm2's `nextLine` reads them.
fn crlf_lines(mut head: &[u8]) -> impl Iterator<Item = &[u8]> {
    std::iter::from_fn(move || {
        if head.is_empty() {
            return None;
        }
        let end = head.windows(2).position(|w| w == b"\r\n");
        let (line, rest) = match end {
            Some(i) => (&head[..i], &head[i + 2..]),
            None => (head, &head[head.len()..]),
        };
        head = rest;
        Some(line)
    })
}

/// A line as iTerm2 decodes it (Latin-1), None when longer than its limit.
fn latin1(line: &[u8]) -> Option<String> {
    (line.len() + 2 <= MAX_LINE).then(|| line.iter().map(|&b| b as char).collect())
}

/// Trims what `whitespaceCharacterSet` holds among Latin-1: space, tab and no-break space.
fn trim(s: &str) -> &str {
    s.trim_matches([' ', '\t', '\u{a0}'])
}

/// The path of a request target, as `NSURL.path` gives it for the targets a client sends: the
/// query and fragment dropped, and for an absolute target the scheme and authority too.
pub fn url_path(target: &str) -> &str {
    let rest = match target.find("://") {
        Some(i) if !target[..i].contains('/') => {
            let after = &target[i + 3..];
            &after[after.find(['/', '?', '#']).unwrap_or(after.len())..]
        }
        _ => target,
    };
    &rest[..rest.find(['?', '#']).unwrap_or(rest.len())]
}

/// The host of a URL as `NSURL.host` gives it for the origins a client sends; None without a
/// `scheme://` authority.
fn url_host(url: &str) -> Option<&str> {
    let i = url.find("://")?;
    let after = &url[i + 3..];
    let authority = &after[..after.find(['/', '?', '#']).unwrap_or(after.len())];
    let hostport = &authority[authority.rfind('@').map_or(0, |a| a + 1)..];
    let host = match hostport.strip_prefix('[') {
        Some(v6) => &v6[..v6.find(']')?],
        None => &hostport[..hostport.find(':').unwrap_or(hostport.len())],
    };
    (!host.is_empty()).then_some(host)
}

/// `-[NSString integerValue]`: leading spaces, an optional sign, then digits; 0 without any.
fn integer_value(s: &str) -> i64 {
    let s = s.trim_start();
    let (neg, digits) = match s.as_bytes().first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    let n = digits.bytes().take_while(u8::is_ascii_digit).fold(0i64, |n, d| n.saturating_mul(10).saturating_add((d - b'0') as i64));
    if neg { -n } else { n }
}

/// iTerm2's checks in its order (`S:510-528`, `W:74-175`).
pub fn check(r: &Request, consume_cookie: &mut dyn FnMut(&str) -> bool, fresh_key: &mut dyn FnMut() -> String) -> Result<Upgrade, Refusal> {
    let bad = |s: String| Err(Refusal::BadRequest(s));
    let header = |name: &str| r.headers.get(name).map(String::as_str);
    let path = url_path(&r.path);
    if path != "/" {
        return bad(format!("Bad request. URL path was {path}, but should be /"));
    }
    if r.method != "GET" {
        return bad(format!("HTTP method {} not accepted (must be GET)", r.method));
    }
    if path.is_empty() {
        return bad("Request had an empty path in the HTTP request".to_string());
    }
    for (name, required) in [("upgrade", "websocket"), ("connection", "Upgrade"), ("sec-websocket-protocol", SUBPROTOCOL)] {
        // A missing header passes: iTerm2 compares by messaging nil, which answers
        // NSOrderedSame.
        if let Some(value) = header(name)
            && !value.eq_ignore_ascii_case(required)
        {
            return bad(format!("Header {name} has value <{value}> but I require <{required}>"));
        }
    }
    for name in ["sec-websocket-key", "sec-websocket-version", "host", "origin"] {
        if header(name).is_none_or(str::is_empty) {
            return bad(format!("Empty or missing value for required header {name}"));
        }
    }
    let origin = header("origin").unwrap_or_default();
    let origin_host = url_host(origin);
    if origin_host != Some("localhost") {
        return bad(format!("Origin's host is not localhost: is {} (string value is {origin})", origin_host.unwrap_or_default()));
    }
    let host = header("host").unwrap_or_default();
    let host = &host[..host.rfind(':').unwrap_or(host.len())];
    if !["localhost", "127.0.0.1", "[::1]"].contains(&host) {
        return bad(format!("Host header is {host}, but must be localhost, 127.0.01, or [::1]."));
    }
    let version = header("sec-websocket-version").unwrap_or_default();
    let ws_version = integer_value(version);
    if ws_version < WEBSOCKET_VERSION as i64 {
        return bad(format!("sec-websocket-version of {version} is older than {WEBSOCKET_VERSION}"));
    }
    // An unknown cookie only leaves the connection unauthorized; it is consumed either way.
    let presented = header("x-iterm2-cookie").filter(|c| consume_cookie(c)).map(|c| Presented(c.to_string()));
    let preauthorized = presented.is_some();
    let Some(library_version) = header("x-iterm2-library-version") else {
        return bad("The \u{201c}x-iterm2-library-version\u{201d} header was not set. You may need to update your Python runtime to a newer version.".to_string());
    };
    let parts: Vec<&str> = library_version.split(' ').collect();
    let [name, reported] = parts[..] else {
        return bad(format!("The \u{201c}x-iterm2-library-version\u{201d} header was malformed: \u{201d}{library_version}\u{201d}"));
    };
    if name == "python" && library_too_old(reported, MIN_PYTHON_LIBRARY) {
        return Err(Refusal::Unacceptable(format!(
            "{TOO_OLD}. {name} library version reported as {}. Minimum supported by this version of Termurion is {}",
            Decimal::parse(reported).map_or("NaN".to_string(), |d| d.to_string()),
            Decimal::parse(MIN_PYTHON_LIBRARY).map_or("NaN".to_string(), |d| d.to_string()),
        )));
    }
    Ok(Upgrade {
        key: header("x-iterm2-key").map_or_else(fresh_key, str::to_string),
        ws_key: header("sec-websocket-key").unwrap_or_default().to_string(),
        ws_version: ws_version.min(u32::MAX as i64) as u32,
        preauthorized,
        advisory_name: header("x-iterm2-advisory-name").map(str::to_string),
        library_version: library_version.to_string(),
        disable_auth_ui: header("x-iterm2-disable-auth-ui").is_some(),
        cookie: presented,
    })
}

/// A decimal as `NSDecimalNumber decimalNumberWithString:` reads the leading number of a string:
/// an optional sign, digits, an optional fraction. Kept without leading or trailing zeros.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Decimal {
    negative: bool,
    int: String,
    frac: String,
}

impl Decimal {
    fn parse(s: &str) -> Option<Decimal> {
        let (negative, s) = match s.as_bytes().first() {
            Some(b'-') => (true, &s[1..]),
            Some(b'+') => (false, &s[1..]),
            _ => (false, s),
        };
        let int: String = s.chars().take_while(char::is_ascii_digit).collect();
        let frac: String = match s[int.len()..].strip_prefix('.') {
            Some(f) => f.chars().take_while(char::is_ascii_digit).collect(),
            None => String::new(),
        };
        if int.is_empty() && frac.is_empty() {
            return None;
        }
        let int = int.trim_start_matches('0').to_string();
        let frac = frac.trim_end_matches('0').to_string();
        let zero = int.is_empty() && frac.is_empty();
        Some(Decimal { negative: negative && !zero, int, frac })
    }

    fn magnitude_cmp(&self, o: &Decimal) -> std::cmp::Ordering {
        self.int.len().cmp(&o.int.len()).then_with(|| self.int.cmp(&o.int)).then_with(|| self.frac.cmp(&o.frac))
    }

    fn cmp(&self, o: &Decimal) -> std::cmp::Ordering {
        match (self.negative, o.negative) {
            (false, true) => std::cmp::Ordering::Greater,
            (true, false) => std::cmp::Ordering::Less,
            (false, false) => self.magnitude_cmp(o),
            (true, true) => o.magnitude_cmp(self),
        }
    }
}

impl std::fmt::Display for Decimal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let sign = if self.negative { "-" } else { "" };
        let int = if self.int.is_empty() { "0" } else { &self.int };
        if self.frac.is_empty() { write!(f, "{sign}{int}") } else { write!(f, "{sign}{int}.{}", self.frac) }
    }
}

/// True when `version` is older than `min`, both compared as decimals (`W:155-163`). A version
/// that is not a number is too old.
pub fn library_too_old(version: &str, min: &str) -> bool {
    match (Decimal::parse(version), Decimal::parse(min)) {
        (Some(v), Some(m)) => m.cmp(&v) == std::cmp::Ordering::Greater,
        (None, Some(_)) => true,
        (_, None) => false,
    }
}

/// The 101 answer (`W:520-547`), with iTerm2's version line when the client's is newer.
pub fn accept_response(ws_key: &str, ws_version: u32) -> String {
    let accept = tungstenite::handshake::derive_accept_key(ws_key.as_bytes());
    let mut out = format!(
        "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
         Sec-WebSocket-Accept: {accept}\r\nSec-WebSocket-Protocol: {SUBPROTOCOL}\r\n\
         X-iTerm2-Protocol-Version: {PROTOCOL_VERSION}\r\n"
    );
    if ws_version > WEBSOCKET_VERSION {
        out.push_str(&format!("Sec-Websocket-Version: {WEBSOCKET_VERSION}\r\n"));
    }
    out.push_str("\r\n");
    out
}

/// A refusal's answer (`HC:122-132`); the connection closes after it.
pub fn status_response(code: u16) -> String {
    let reason = match code {
        400 => "Bad Request",
        401 => "Unauthorized",
        406 => "Unacceptable (upgrade Python library)",
        // Termurion's own: a connection cap (iTerm2 has none).
        503 => "Service Unavailable",
        _ => "Error",
    };
    format!("HTTP/1.1 {code} {reason}\r\n\r\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn python_head() -> String {
        [
            "GET / HTTP/1.1",
            "Host: localhost:1912",
            "Upgrade: websocket",
            "Connection: Upgrade",
            "Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==",
            "Sec-WebSocket-Version: 13",
            "Sec-WebSocket-Extensions: permessage-deflate; client_max_window_bits",
            "Sec-WebSocket-Protocol: api.iterm2.com",
            "origin: ws://localhost/",
            "x-iterm2-library-version: python 2.24",
            "x-iterm2-disable-auth-ui: true",
            "x-iterm2-advisory-name: demo.py",
            "User-Agent: Python/3.12 websockets/10.4",
            "",
            "",
        ]
        .join("\r\n")
    }

    fn valid() -> Request {
        parse(python_head().as_bytes()).unwrap()
    }

    fn with(name: &str, value: Option<&str>) -> Request {
        let mut r = valid();
        match value {
            Some(v) => r.headers.insert(name.to_string(), v.to_string()),
            None => r.headers.remove(name),
        };
        r
    }

    fn run(r: &Request) -> Result<Upgrade, Refusal> {
        check(r, &mut |_| false, &mut || "FRESH".to_string())
    }

    fn bad(r: &Request) -> String {
        match run(r) {
            Err(Refusal::BadRequest(s)) => s,
            other => panic!("expected a 400, got {other:?}"),
        }
    }

    #[test]
    fn upgrade_follows_iterm2() {
        assert!(run(&valid()).is_ok());

        let mut r = valid();
        r.path = "/x".into();
        assert_eq!(run(&r), Err(Refusal::BadRequest("Bad request. URL path was /x, but should be /".into())));
        let mut r = valid();
        r.method = "POST".into();
        assert_eq!(bad(&r), "HTTP method POST not accepted (must be GET)");

        assert_eq!(bad(&with("upgrade", Some("h2c"))), "Header upgrade has value <h2c> but I require <websocket>");
        assert!(run(&with("upgrade", Some("WebSocket"))).is_ok());
        assert_eq!(bad(&with("connection", Some("keep-alive, Upgrade"))), "Header connection has value <keep-alive, Upgrade> but I require <Upgrade>");
        assert_eq!(bad(&with("sec-websocket-protocol", Some("x"))), "Header sec-websocket-protocol has value <x> but I require <api.iterm2.com>");
        // The order of the three: upgrade, connection, sec-websocket-protocol.
        let mut r = with("sec-websocket-protocol", Some("x"));
        r.headers.insert("connection".into(), "close".into());
        r.headers.insert("upgrade".into(), "h2c".into());
        assert!(bad(&r).starts_with("Header upgrade "));
        r.headers.insert("upgrade".into(), "websocket".into());
        assert!(bad(&r).starts_with("Header connection "));
        // A missing one passes: iTerm2 compares by messaging nil, which answers NSOrderedSame.
        for name in ["upgrade", "connection", "sec-websocket-protocol"] {
            assert!(run(&with(name, None)).is_ok(), "{name}");
        }

        assert_eq!(bad(&with("origin", None)), "Empty or missing value for required header origin");
        assert_eq!(bad(&with("origin", Some(""))), "Empty or missing value for required header origin");
        // The order of the four: sec-websocket-key, sec-websocket-version, host, origin.
        let mut r = with("origin", None);
        r.headers.remove("host");
        assert_eq!(bad(&r), "Empty or missing value for required header host");
        r.headers.remove("sec-websocket-version");
        assert_eq!(bad(&r), "Empty or missing value for required header sec-websocket-version");
        r.headers.remove("sec-websocket-key");
        assert_eq!(bad(&r), "Empty or missing value for required header sec-websocket-key");

        assert_eq!(bad(&with("origin", Some("http://evil/"))), "Origin's host is not localhost: is evil (string value is http://evil/)");
        assert!(run(&with("origin", Some("http://localhost:1912"))).is_ok());

        assert_eq!(bad(&with("host", Some("example.com:1912"))), "Host header is example.com, but must be localhost, 127.0.01, or [::1].");
        for host in ["localhost:1912", "127.0.0.1:1912", "[::1]:1912", "localhost"] {
            assert!(run(&with("host", Some(host))).is_ok(), "{host}");
        }

        assert_eq!(bad(&with("sec-websocket-version", Some("12"))), "sec-websocket-version of 12 is older than 13");
        assert_eq!(run(&with("sec-websocket-version", Some("14"))).unwrap().ws_version, 14);

        // An unknown cookie is consumed once and is not a refusal.
        let mut calls = Vec::new();
        let u = check(&with("x-iterm2-cookie", Some("abc")), &mut |c| {
            calls.push(c.to_string());
            false
        }, &mut || "FRESH".into())
        .unwrap();
        assert!(!u.preauthorized);
        assert_eq!(calls, ["abc"]);
        let u = check(&with("x-iterm2-cookie", Some("good")), &mut |c| c == "good", &mut || "FRESH".into()).unwrap();
        assert!(u.preauthorized);
        assert_eq!(u.cookie.as_ref().map(Presented::as_str), Some("good"), "the app can tell whose cookie it was");
        assert!(!format!("{u:?}").contains("good"), "a cookie is a secret: Debug hides it");
        // No cookie header: nothing consumed.
        let mut called = false;
        check(&valid(), &mut |_| {
            called = true;
            true
        }, &mut || "FRESH".into())
        .unwrap();
        assert!(!called);

        assert_eq!(run(&valid()).unwrap().key, "FRESH");
        assert_eq!(run(&with("x-iterm2-key", Some("K1"))).unwrap().key, "K1");
        assert!(run(&with("x-iterm2-disable-auth-ui", Some("no"))).unwrap().disable_auth_ui);
        assert!(!run(&with("x-iterm2-disable-auth-ui", None)).unwrap().disable_auth_ui);
    }

    #[test]
    fn library_version_follows_iterm2() {
        let lib = "x-iterm2-library-version";
        assert_eq!(run(&with(lib, None)), Err(Refusal::BadRequest("The \u{201c}x-iterm2-library-version\u{201d} header was not set. You may need to update your Python runtime to a newer version.".into())));
        assert_eq!(run(&with(lib, Some("python"))), Err(Refusal::BadRequest("The \u{201c}x-iterm2-library-version\u{201d} header was malformed: \u{201d}python\u{201d}".into())));
        let old = run(&with(lib, Some("python 0.23")));
        assert_eq!(old, Err(Refusal::Unacceptable("Library version too old. python library version reported as 0.23. Minimum supported by this version of Termurion is 0.24".into())));
        assert_eq!(old.unwrap_err().code(), 406);
        for ok in ["python 0.3", "python 2.24", "python 0.24", "swift 0.1"] {
            assert!(run(&with(lib, Some(ok))).is_ok(), "{ok}");
        }
        assert!(matches!(run(&with(lib, Some("python x"))), Err(Refusal::Unacceptable(_))));

        // The cookie is consumed before the library check refuses (and stays consumed).
        let mut r = with(lib, Some("python 0.1"));
        r.headers.insert("x-iterm2-cookie".into(), "c".into());
        let mut consumed = 0;
        assert!(check(&r, &mut |_| {
            consumed += 1;
            true
        }, &mut || "FRESH".into())
        .is_err());
        assert_eq!(consumed, 1);

        assert!(library_too_old("0.23", "0.24"));
        assert!(!library_too_old("0.3", "0.24"));
        assert!(!library_too_old("0.240", "0.24"));
        assert!(!library_too_old("10.0", "9.99"));
        assert!(library_too_old("x", "0.24"));
    }

    #[test]
    fn python_request_is_accepted() {
        let u = run(&valid()).unwrap();
        assert_eq!(u.library_version, "python 2.24");
        assert_eq!(u.advisory_name.as_deref(), Some("demo.py"));
        assert!(u.disable_auth_ui);
        assert_eq!(u.ws_version, 13);
        assert_eq!(u.ws_key, "dGhlIHNhbXBsZSBub25jZQ==");
        assert!(!u.preauthorized);
    }

    #[test]
    fn responses_follow_iterm2() {
        let expected = "HTTP/1.1 101 Switching Protocols\r\n\
                        Upgrade: websocket\r\n\
                        Connection: Upgrade\r\n\
                        Sec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo=\r\n\
                        Sec-WebSocket-Protocol: api.iterm2.com\r\n\
                        X-iTerm2-Protocol-Version: 1.19\r\n\
                        \r\n";
        assert_eq!(accept_response("dGhlIHNhbXBsZSBub25jZQ==", 13), expected);
        let v14 = accept_response("dGhlIHNhbXBsZSBub25jZQ==", 14);
        assert!(v14.ends_with("X-iTerm2-Protocol-Version: 1.19\r\nSec-Websocket-Version: 13\r\n\r\n"), "{v14}");
        assert_eq!(status_response(400), "HTTP/1.1 400 Bad Request\r\n\r\n");
        assert_eq!(status_response(401), "HTTP/1.1 401 Unauthorized\r\n\r\n");
        assert_eq!(status_response(406), "HTTP/1.1 406 Unacceptable (upgrade Python library)\r\n\r\n");
        assert_eq!(status_response(503), "HTTP/1.1 503 Service Unavailable\r\n\r\n");

        assert_eq!(head_end(b"GET / HTTP/1.1\r\nHost: x\r\n"), None);
        assert_eq!(head_end(b"GET / HTTP/1.1\r\n\r\n"), Some(18));
        assert_eq!(head_end(b"GET / HTTP/1.1\r\n\r\nrest"), Some(18));
        assert_eq!(head_end(b"a\n\nb"), None);

        assert_eq!(parse(b"GET HTTP/1.1\r\n\r\n"), None);
        assert_eq!(parse(b"GET / HTTP/1.0\r\n\r\n"), None);
        assert_eq!(parse(b"GET / HTTP/1.1\r\nno colon\r\n\r\n"), None);
        assert_eq!(parse(b"GET / HTTP/1.1\r\nx:\r\n\r\n"), None);
        assert_eq!(parse(b"GET / HTTP/1.1\r\nContent-Length: 0\r\n\r\n"), None);
        let r = parse(b"GET /p HTTP/1.1\r\nX-A:  one \r\nx-a: two\r\n\r\n").unwrap();
        assert_eq!((r.method.as_str(), r.path.as_str()), ("GET", "/p"));
        assert_eq!(r.headers.get("x-a").map(String::as_str), Some("two"));
        let many: String = (0..MAX_HEADERS).map(|i| format!("h{i}: v\r\n")).collect();
        assert_eq!(parse(format!("GET / HTTP/1.1\r\n{many}\r\n").as_bytes()), None);
        let long = "a".repeat(MAX_LINE);
        assert_eq!(parse(format!("GET / HTTP/1.1\r\nh: {long}\r\n\r\n").as_bytes()), None);

        assert_eq!(url_path("/"), "/");
        assert_eq!(url_path("/?a=b"), "/");
        assert_eq!(url_path("/x#f"), "/x");
        assert_eq!(url_path("ws://localhost:1912/"), "/");
    }
}
