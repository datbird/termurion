//! No HTTP client on Unix yet (P1 picks one). A request ends at once with an Unsupported error.

use std::sync::Arc;

use super::super::Unsupported;
use crate::ai_transport::{Bounds, Call, HttpEvent, HttpRequest, Notify, Options, Transport};

const FEATURE: &str = "AI chat";

/// No fetch tool is used on Unix yet.
pub fn fetch_text(_url: &str, _args: &[String]) -> super::super::http::Fetched {
    Err((-1, Unsupported { feature: "Settings from a URL" }.to_string()))
}

struct NoTransport;

/// A call that has one event to give: the failure.
struct Failed {
    event: Option<HttpEvent>,
}

impl Call for Failed {
    fn try_event(&mut self) -> Option<HttpEvent> {
        self.event.take()
    }

    fn cancel(&mut self) {
        self.event = None;
    }

    #[cfg(test)]
    fn finished(&self) -> bool {
        true
    }
}

impl Transport for NoTransport {
    fn start_bounded(&self, _request: HttpRequest, _options: Options, _bounds: Bounds, notify: Notify) -> Box<dyn Call> {
        let error = Unsupported { feature: FEATURE }.to_string();
        notify();
        Box::new(Failed { event: Some(HttpEvent::Done { body: String::new(), error: Some(error) }) })
    }
}

pub fn system() -> Arc<dyn Transport> {
    Arc::new(NoTransport)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai_transport::Proxy;

    #[test]
    fn unix_http_unsupported() {
        let request = HttpRequest { method: "GET", url: "https://example.test/".to_string(), headers: Vec::new(), body: Vec::new(), stream: false };
        let options = Options { idle: std::time::Duration::from_secs(1), proxy: Proxy::Off };
        let notify: Notify = Arc::new(|| {});
        let mut call = system().start(request, options, notify);
        match call.try_event() {
            Some(HttpEvent::Done { error: Some(e), .. }) => assert!(e.contains("AI chat is not available"), "{e}"),
            other => panic!("{other:?}"),
        }
        assert!(call.try_event().is_none());
    }
}
