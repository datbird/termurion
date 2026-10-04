//! The hook's minimal blocking Python API client (milestone 12f): tungstenite's client
//! handshake and prost, nothing else. A cookie is a secret: no header, request or error here
//! ever carries one into a message.

/// What `cc-status` reports as its library (`x-iterm2-library-version`, name and version).
pub const LIBRARY_VERSION: &str = "cc-status 1.0";

/// The request headers beyond the standard WebSocket ones. A cookie or key is sent only when
/// given; `disable-auth-ui` is always sent so a hook never raises a dialog.
pub fn request_headers(host: &str, cookie: Option<&str>, key: Option<&str>, advisory: &str) -> Vec<(String, String)> {
    let h = |k: &str, v: &str| (k.to_string(), v.to_string());
    let mut out = vec![
        h("Sec-WebSocket-Protocol", crate::api_proto::SUBPROTOCOL),
        h("Origin", &format!("http://{host}")),
        h("x-iterm2-library-version", LIBRARY_VERSION),
        h("x-iterm2-advisory-name", advisory),
        h("x-iterm2-disable-auth-ui", "true"),
    ];
    if let Some(c) = cookie {
        out.push(h("x-iterm2-cookie", c));
    }
    if let Some(k) = key {
        out.push(h("x-iterm2-key", k));
    }
    out
}

/// A connected client: one request in flight at a time, each answered within the timeout.
#[cfg(windows)]
pub struct Client {
    ws: tungstenite::WebSocket<std::net::TcpStream>,
    timeout: std::time::Duration,
    next_id: i64,
}

#[cfg(windows)]
impl Client {
    /// Connects to 127.0.0.1, then [::1], on the API port.
    pub fn connect(cookie: Option<&str>, key: Option<&str>, advisory: &str, timeout: std::time::Duration) -> Result<Client, String> {
        use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
        let port = crate::api_http::PORT;
        let first = Self::connect_to(SocketAddr::from((Ipv4Addr::LOCALHOST, port)), cookie, key, advisory, timeout);
        match first {
            Ok(c) => Ok(c),
            Err(_) => Self::connect_to(SocketAddr::from((Ipv6Addr::LOCALHOST, port)), cookie, key, advisory, timeout),
        }
    }

    pub fn connect_to(addr: std::net::SocketAddr, cookie: Option<&str>, key: Option<&str>, advisory: &str, timeout: std::time::Duration) -> Result<Client, String> {
        use tungstenite::client::IntoClientRequest;
        let stream = std::net::TcpStream::connect_timeout(&addr, timeout).map_err(|e| format!("could not connect: {e}"))?;
        stream.set_read_timeout(Some(timeout)).map_err(|e| e.to_string())?;
        stream.set_write_timeout(Some(timeout)).map_err(|e| e.to_string())?;
        let mut req = format!("ws://{addr}/").into_client_request().map_err(|e| e.to_string())?;
        for (k, v) in request_headers("localhost", cookie, key, advisory) {
            let name = tungstenite::http::HeaderName::from_bytes(k.as_bytes()).map_err(|e| e.to_string())?;
            let mut value = tungstenite::http::HeaderValue::from_str(&v).map_err(|_| "a header value is not valid".to_string())?;
            if k == "x-iterm2-cookie" || k == "x-iterm2-key" {
                value.set_sensitive(true);
            }
            req.headers_mut().insert(name, value);
        }
        // The server answers the upgrade only after the app's verdict; the read timeout bounds that wait.
        let (ws, _) = tungstenite::client::client(req, stream).map_err(|_| "Tandem did not accept the connection".to_string())?;
        Ok(Client { ws, timeout, next_id: 1 })
    }
}

#[cfg(windows)]
impl crate::cc_status::Api for Client {
    fn invoke(&mut self, session: &str, invocation: &str) -> Result<serde_json::Value, String> {
        use crate::api_proto::pb;
        use pb::client_originated_message::Submessage as Req;
        use pb::server_originated_message::Submessage as Resp;
        let id = self.next_id;
        self.next_id += 1;
        let request = pb::InvokeFunctionRequest {
            invocation: Some(invocation.to_string()),
            timeout: Some(self.timeout.as_secs_f64()),
            context: Some(pb::invoke_function_request::Context::Session(pb::invoke_function_request::Session { session_id: Some(session.to_string()) })),
        };
        let m = pb::ClientOriginatedMessage { id: Some(id), submessage: Some(Req::InvokeFunctionRequest(request)) };
        self.ws.send(tungstenite::Message::binary(prost::Message::encode_to_vec(&m))).map_err(|e| e.to_string())?;
        let deadline = std::time::Instant::now() + self.timeout;
        loop {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                return Err("timed out".to_string());
            }
            self.ws.get_ref().set_read_timeout(Some(left)).map_err(|e| e.to_string())?;
            match self.ws.read().map_err(|e| e.to_string())? {
                tungstenite::Message::Binary(b) => {
                    let Ok(r) = <pb::ServerOriginatedMessage as prost::Message>::decode(&b[..]) else { continue };
                    if r.id != Some(id) {
                        continue;
                    }
                    return match r.submessage {
                        Some(Resp::InvokeFunctionResponse(f)) => answer(f),
                        Some(Resp::Error(e)) => Err(e),
                        _ => Err("unexpected answer".to_string()),
                    };
                }
                tungstenite::Message::Close(_) => return Err("closed".to_string()),
                _ => {}
            }
        }
    }
}

/// An invocation's answer: its JSON result, or the reason it failed.
#[cfg(windows)]
fn answer(f: crate::api_proto::pb::InvokeFunctionResponse) -> Result<serde_json::Value, String> {
    use crate::api_proto::pb::invoke_function_response::{Disposition, Status};
    match f.disposition {
        Some(Disposition::Success(s)) => Ok(s.json_result.and_then(|j| serde_json::from_str(&j).ok()).unwrap_or(serde_json::Value::Null)),
        Some(Disposition::Error(e)) => Err(e.error_reason.unwrap_or_else(|| Status::try_from(e.status.unwrap_or(0)).map_or("failed".to_string(), |s| s.as_str_name().to_lowercase()))),
        None => Err("no answer".to_string()),
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use crate::api_proto::pb;
    use crate::api_server::{Event, Server};
    use crate::cc_status::Api;
    use prost::Message as _;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    fn server() -> Server {
        Server::start(0, Arc::new(|_, _| Ok(1)), Arc::new(|c: &str| c == "c"), Arc::new(|| {})).unwrap()
    }

    fn next(s: &Server) -> Event {
        let end = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(e) = s.try_event() {
                return e;
            }
            assert!(Instant::now() < end, "no event");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Plays the app: allows a connection that came with the known cookie.
    fn allow_known(s: &Server) {
        match next(s) {
            Event::Pending(p) => s.verdict(p.conn, p.upgrade.preauthorized),
            e => panic!("{e:?}"),
        }
    }

    fn reply(s: &Server, conn: u64, id: Option<i64>, d: pb::invoke_function_response::Disposition) {
        let m = pb::ServerOriginatedMessage { id, submessage: Some(pb::server_originated_message::Submessage::InvokeFunctionResponse(pb::InvokeFunctionResponse { disposition: Some(d) })) };
        s.respond(conn, m.encode_to_vec());
    }

    #[test]
    fn client_reaches_the_server() {
        use pb::invoke_function_response::{Disposition, Error, Status, Success};
        let s = server();
        let addr = s.addrs()[0];
        let t = Duration::from_secs(5);
        // A known cookie connects; the request names the session and the text.
        let h = std::thread::spawn(move || {
            let mut c = Client::connect_to(addr, Some("c"), None, "cc-status", t)?;
            let a = c.invoke("42", "iterm2.set_status(status: \"idle\")");
            let b = c.invoke("42", "iterm2.bad()");
            Ok::<_, String>((a, b))
        });
        allow_known(&s);
        let mut answered = 0;
        while answered < 2 {
            match next(&s) {
                Event::Message { conn, bytes } => {
                    let req = pb::ClientOriginatedMessage::decode(&bytes[..]).unwrap();
                    let Some(pb::client_originated_message::Submessage::InvokeFunctionRequest(r)) = req.submessage else { panic!("{req:?}") };
                    let Some(pb::invoke_function_request::Context::Session(c)) = &r.context else { panic!("{r:?}") };
                    assert_eq!(c.session_id.as_deref(), Some("42"));
                    answered += 1;
                    if answered == 1 {
                        assert_eq!(r.invocation.as_deref(), Some("iterm2.set_status(status: \"idle\")"));
                        reply(&s, conn, req.id, Disposition::Success(Success { json_result: Some("{}".into()) }));
                    } else {
                        reply(&s, conn, req.id, Disposition::Error(Error { status: Some(Status::Failed as i32), error_reason: Some("nope".into()) }));
                    }
                }
                Event::Closed { .. } => {}
                e => panic!("{e:?}"),
            }
        }
        let (a, b) = h.join().unwrap().unwrap();
        assert_eq!(a.unwrap(), serde_json::json!({}));
        assert_eq!(b.unwrap_err(), "nope");
        // No cookie: the app refuses (it applies disable-auth-ui), and the client fails within its timeout.
        let h = std::thread::spawn(move || Client::connect_to(addr, None, None, "cc-status", Duration::from_secs(3)).map(|_| ()));
        loop {
            match next(&s) {
                Event::Pending(p) => break s.verdict(p.conn, false),
                Event::Closed { .. } => {}
                e => panic!("{e:?}"),
            }
        }
        let started = Instant::now();
        assert!(h.join().unwrap().is_err());
        assert!(started.elapsed() < Duration::from_secs(4));
    }
}
