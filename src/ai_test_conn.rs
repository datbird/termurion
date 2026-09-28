//! Test Connection in the manual model editor (`AITerm/AIConnectionTester.swift`, `CT:28-216`):
//! the key a real request would send (the self-hosted placeholder for a private host, else the
//! vendor's stored key, prompting when there is none), one minimal non-streaming "Hi" through
//! the model's own wire format, and the outcome as the alert's text. Pure over
//! `ai_transport::Transport`; the editor drives it (`settings_ai`).

use crate::ai_catalog::{Feature, Model};
use crate::ai_models::{manual_vendor, Endpoint};
use crate::ai_msg::{Message, Role};
use crate::ai_policy::{self, KeyPolicy, SELF_HOSTED_PLACEHOLDER};
use crate::ai_settings::{Api, Vendor};
use crate::ai_transport::{self, Call, HttpEvent, Notify, Options, Transport};
use crate::ai_turn::{self, error_reason, Build, Hosted, Wire, APPLE_UNAVAILABLE, REVIEW_KEY};
use crate::pwm::Secret;
use std::collections::BTreeSet;
use std::sync::Arc;

pub const SUCCEEDED: &str = "The connection succeeded. The model responded normally.";
pub const BAD_URL: &str = "The URL is not valid.";
pub const PLACEHOLDER_IN_USE: &str = "Placeholder key in use: AI responses are simulated for App Review and no external service is contacted.";
pub const MISSING_TITLE: &str = "Missing Information";
pub const MISSING_TEXT: &str = "Enter a model name and URL before testing the connection.";
pub const MISSING_DISCOVERED: &str = "Choose a model, or click Refresh Models first, so there is something to test with.";
pub const TESTING: &str = "Testing\u{2026}";
pub const SUCCEEDED_TITLE: &str = "Connection Succeeded";
pub const FAILED_TITLE: &str = "Connection Failed";

pub fn could_not_build(e: &str) -> String {
    format!("Could not build a request: {e}")
}

pub fn not_understood(e: &str) -> String {
    format!("The server responded but its reply could not be understood: {e}")
}

/// How a test ended. `Canceled` (the key prompt was refused) shows nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Success(String),
    Failure(String),
    Canceled,
}

/// The editor's form as the test reads it (`G:1460-1510`): trimmed name and URL, the API, the
/// Function calling and Supports temperature boxes, and the named custom headers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Probe {
    pub name: String,
    pub url: String,
    pub api: Api,
    pub function_calling: bool,
    pub supports_temperature: bool,
    pub headers: Vec<(String, String)>,
}

/// Why the form cannot be tested yet (`G:1466-1495`), or None. In auto-discovery mode `name` is
/// the popup's choice and `cached_first` the first discovered tag, which stands in for "All
/// installed models". A missing URL wins.
pub fn missing(name: &str, url: &str, dynamic: bool, cached_first: Option<&str>) -> Option<&'static str> {
    if url.trim().is_empty() {
        return Some(MISSING_TEXT);
    }
    let name = name.trim();
    if dynamic {
        (name.is_empty() && cached_first.is_none_or(str::is_empty)).then_some(MISSING_DISCOVERED)
    } else {
        name.is_empty().then_some(MISSING_TEXT)
    }
}

/// Where the probe goes, for the consent gate and the key prompt: the vendor 12a's manual
/// classification gives, and the probe's name, URL, API and headers (`CT:40`).
pub fn endpoint(p: &Probe) -> Endpoint {
    Endpoint { vendor: manual_vendor(p.api, &p.url, &p.name), model: p.name.clone(), url: p.url.clone(), api: p.api, custom_headers: p.headers.clone() }
}

/// The model the probe sends as (`CT:108-128`): 8192 context and 64 response tokens, function
/// calling as the editor's box says, no streaming and no vector store, the editor's temperature
/// box and headers.
fn probe_model(p: &Probe) -> Model {
    let features = if p.function_calling { BTreeSet::from([Feature::FunctionCalling]) } else { BTreeSet::new() };
    let mut m = crate::ai_model::model(&p.name, &p.url, p.api, manual_vendor(p.api, &p.url, &p.name), 8192, 64, features);
    m.supports_temperature = p.supports_temperature;
    m.custom_headers = p.headers.clone();
    m
}

enum State {
    Ready,
    NeedKey(Vendor),
    Running(Box<dyn Call>),
    Done,
}

/// One Test Connection (`CT:28-216`). `start` with the vendor's stored key; when `needs_key`
/// names a vendor, the editor asks for its key and passes the answer to `key_given`; `poll`
/// until the outcome.
pub struct Tester {
    probe: Probe,
    model: Model,
    wire: Wire,
    transport: Arc<dyn Transport>,
    options: Options,
    notify: Notify,
    state: State,
}

impl Tester {
    pub fn new(p: Probe, wire: Wire, transport: Arc<dyn Transport>, options: Options, notify: Notify) -> Tester {
        let model = probe_model(&p);
        Tester { probe: p, model, wire, transport, options, notify, state: State::Ready }
    }

    /// The form it tests.
    pub fn probe(&self) -> &Probe {
        &self.probe
    }

    /// Whether `start` ran.
    pub fn started(&self) -> bool {
        !matches!(self.state, State::Ready)
    }

    /// Starts the test with the vendor's stored key (`CT:36-80`): the review key wins over the
    /// placeholder policy; a private host gets the placeholder without a prompt; else the stored
    /// key, or `needs_key`. None while it runs or waits for a key.
    pub fn start(&mut self, stored: Option<&Secret>) -> Option<Outcome> {
        if self.started() {
            return None;
        }
        let review = stored.is_some_and(|k| k.expose() == REVIEW_KEY);
        if !review && ai_policy::key_policy(&self.probe.url, self.probe.api) != KeyPolicy::VendorKey {
            return self.send(SELF_HOSTED_PLACEHOLDER);
        }
        // A registration needs a key that is not blank (`T:83-96`).
        match stored.filter(|k| !k.expose().trim_start().is_empty()) {
            Some(key) => self.send(key.expose()),
            None => {
                self.state = State::NeedKey(self.model.vendor);
                None
            }
        }
    }

    /// The vendor whose key the test waits for.
    pub fn needs_key(&self) -> Option<Vendor> {
        match self.state {
            State::NeedKey(v) => Some(v),
            _ => None,
        }
    }

    /// The key prompt's answer: None (or a blank key) cancels silently (`CT:69-72`).
    pub fn key_given(&mut self, key: Option<Secret>) -> Option<Outcome> {
        if self.needs_key().is_none() {
            return None;
        }
        match key.filter(|k| !k.expose().trim_start().is_empty()) {
            Some(key) => self.send(key.expose()),
            None => {
                self.state = State::Done;
                Some(Outcome::Canceled)
            }
        }
    }

    /// The probe request (`CT:94-160`).
    fn send(&mut self, key: &str) -> Option<Outcome> {
        self.state = State::Done;
        if key == REVIEW_KEY {
            return Some(Outcome::Success(PLACEHOLDER_IN_USE.to_string()));
        }
        let Some(dialect) = ai_turn::dialect_for(self.model.api) else {
            return Some(Outcome::Failure(could_not_build(APPLE_UNAVAILABLE)));
        };
        let messages = [Message::text(Role::User, "Hi")];
        let b = Build {
            model: &self.model,
            key,
            messages: &messages,
            tools: &[],
            stream: false,
            hosted: &Hosted::default(),
            previous_response_id: None,
            should_think: None,
            effort: None,
            tier: None,
            volatile: None,
            frozen: None,
            wire: &self.wire,
        };
        // `urlIsValid` (`P:68-70`): a URL the transport cannot send.
        if ai_transport::parse_url(&dialect.url(&b)).is_err() {
            return Some(Outcome::Failure(BAD_URL.to_string()));
        }
        let request = match ai_turn::request(&b, dialect.as_ref()) {
            Ok(r) => r,
            Err(e) => return Some(Outcome::Failure(could_not_build(&e))),
        };
        self.state = State::Running(self.transport.start(request, self.options.clone(), self.notify.clone()));
        None
    }

    /// The outcome once the answer arrived.
    pub fn poll(&mut self) -> Option<Outcome> {
        let State::Running(call) = &mut self.state else { return None };
        while let Some(event) = call.try_event() {
            if let HttpEvent::Done { body, error } = event {
                self.state = State::Done;
                return Some(self.outcome(&body, error.as_deref().unwrap_or("")));
            }
        }
        None
    }

    /// `outcome(for:provider:)` (`CT:193-215`): a parsed error body wins, then the transport's
    /// error, then the reply must parse. A failure gets the withheld-key advice when the refusal
    /// reads like one over credentials (`CT:166-185`).
    fn outcome(&self, body: &str, error: &str) -> Outcome {
        let reason = error_reason(body.as_bytes()).filter(|r| !r.is_empty());
        let failure = match reason {
            Some(reason) => reason,
            None if !error.is_empty() => error.to_string(),
            None => match ai_turn::dialect_for(self.model.api).map(|d| d.parse(body.as_bytes())) {
                Some(Err(e)) => not_understood(&e),
                _ => return Outcome::Success(SUCCEEDED.to_string()),
            },
        };
        let p = &self.probe;
        Outcome::Failure(ai_policy::amended(&failure, error, &p.url, p.api, &p.headers))
    }
}

impl Drop for Tester {
    fn drop(&mut self) {
        if let State::Running(call) = &mut self.state {
            call.cancel();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai_gate::{check_endpoint, Check};
    use crate::ai_transport::{HttpRequest, Proxy, Recorded, Reply};
    use crate::ai_turn::Dialect;
    use crate::globals::Globals;
    use serde_json::json;
    use std::time::Duration;

    const LOCAL: &str = "http://localhost:1234/v1/chat/completions";
    const OPENAI: &str = "https://api.openai.com/v1/chat/completions";
    const ANSWER: &[u8] = br#"{"choices":[{"index":0,"message":{"role":"assistant","content":"Hello"},"finish_reason":"stop"}]}"#;

    fn wire() -> Wire {
        Wire { token_limit: 128000, response_limit: 8192, zdr: false, ollama_num_ctx: 0, ollama_keep_alive: "30m".into() }
    }

    fn probe(url: &str) -> Probe {
        Probe {
            name: "qwen3".into(),
            url: url.into(),
            api: Api::ChatCompletions,
            function_calling: true,
            supports_temperature: true,
            headers: vec![("X-Team".into(), "blue".into())],
        }
    }

    fn tester(p: Probe, replies: Vec<Reply>) -> (Tester, Arc<Recorded>) {
        let t = Arc::new(Recorded::new(replies));
        let options = Options { idle: Duration::from_secs(1200), proxy: Proxy::System };
        (Tester::new(p, wire(), t.clone(), options, Arc::new(|| {})), t)
    }

    /// Starts with `stored` and polls to the end.
    #[track_caller]
    fn outcome(p: Probe, stored: Option<&Secret>, reply: Reply) -> (Outcome, Vec<HttpRequest>) {
        let (mut t, rec) = tester(p, vec![reply]);
        let o = t.start(stored).or_else(|| t.poll()).expect("an outcome");
        (o, rec.requests())
    }

    fn header<'a>(r: &'a HttpRequest, name: &str) -> Option<&'a str> {
        r.headers.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }

    #[test]
    fn test_connection_follows_iterm2() {
        assert_eq!(missing("", "http://h", false, None), Some(MISSING_TEXT));
        assert_eq!(missing("m", "", false, None), Some(MISSING_TEXT));
        assert_eq!(missing("", "", true, None), Some(MISSING_TEXT), "a missing URL wins");
        assert_eq!(missing("m", "http://h", false, None), None);
        assert_eq!(missing("", "http://h", true, None), Some(MISSING_DISCOVERED));
        assert_eq!(missing("", "http://h", true, Some("llama3.3")), None);
        assert_eq!(missing("qwen3", "http://h", true, None), None);
        assert_eq!(
            [MISSING_TITLE, TESTING, SUCCEEDED_TITLE, FAILED_TITLE, BAD_URL],
            ["Missing Information", "Testing\u{2026}", "Connection Succeeded", "Connection Failed", "The URL is not valid."]
        );

        // A private host: no prompt, the placeholder, one non-streaming "Hi".
        let (o, reqs) = outcome(probe(LOCAL), None, Reply::ok(&[ANSWER]));
        assert_eq!(o, Outcome::Success(SUCCEEDED.into()));
        assert_eq!(reqs.len(), 1);
        let r = &reqs[0];
        assert_eq!((r.method, r.url.as_str(), r.stream), ("POST", LOCAL, false));
        assert_eq!(header(r, "Authorization"), Some("Bearer Placeholder for self-hosted"));
        assert_eq!(header(r, "X-Team"), Some("blue"));
        let body: serde_json::Value = serde_json::from_slice(&r.body).unwrap();
        assert_eq!(body["messages"], json!([{"role": "user", "content": "Hi"}]));
        assert_eq!(body["max_tokens"], json!(64));
        assert_eq!(body["stream"], json!(false));
        assert_eq!(body["temperature"], json!(0));
        assert!(body.get("tools").is_none(), "no tools");
        // A stored key does not change that (issue 13021).
        let (_, reqs) = outcome(probe(LOCAL), Some(&Secret::new("sk-stored-0123456789abcdef".into())), Reply::ok(&[ANSWER]));
        assert_eq!(header(&reqs[0], "Authorization"), Some("Bearer Placeholder for self-hosted"));
        let mut cold = probe(LOCAL);
        cold.supports_temperature = false;
        let (_, reqs) = outcome(cold, None, Reply::ok(&[ANSWER]));
        let body: serde_json::Value = serde_json::from_slice(&reqs[0].body).unwrap();
        assert!(body.get("temperature").is_none(), "the editor's temperature box is honored");

        // Failures.
        let garbage: &[u8] = b"<html>";
        let parse_error = crate::ai_chat::ChatCompletions.parse(garbage).expect_err("not JSON");
        assert_eq!(outcome(probe(LOCAL), None, Reply::ok(&[garbage])).0, Outcome::Failure(not_understood(&parse_error)));
        assert_eq!(outcome(probe(LOCAL), None, Reply::ok(&[br#"{"error":{"message":"no such model"}}"#])).0, Outcome::Failure("no such model".into()));
        let advice = ai_policy::withheld_key_hint(LOCAL, Api::ChatCompletions, &[]).expect("advice for a private host");
        let mut bare = probe(LOCAL);
        bare.headers.clear();
        assert_eq!(outcome(bare, None, Reply::status(401, br#"{"error":{"message":"unauthorized"}}"#)).0, Outcome::Failure(format!("unauthorized\n\n{advice}")));
        assert_eq!(outcome(probe(LOCAL), None, Reply::failed("Could not connect to the server.")).0, Outcome::Failure("HTTP request failed with Could not connect to the server.".into()));
        // A URL with no host gets the vendor key, so the key comes first (`CT:59-80`), then the
        // URL check.
        let (mut t, rec) = tester(probe("not a url"), vec![]);
        assert_eq!(t.start(None), None);
        assert_eq!(t.key_given(Some(Secret::new("sk-a".into()))), Some(Outcome::Failure(BAD_URL.into())));
        assert!(rec.requests().is_empty());

        // An OpenAI host with no stored key asks for one.
        let (mut t, rec) = tester(probe(OPENAI), vec![Reply::ok(&[ANSWER])]);
        assert_eq!(t.start(None), None);
        assert_eq!(t.needs_key(), Some(Vendor::OpenAI));
        assert_eq!(t.poll(), None);
        assert_eq!(t.key_given(None), Some(Outcome::Canceled));
        assert!(rec.requests().is_empty());
        let (mut t, rec) = tester(probe(OPENAI), vec![Reply::ok(&[ANSWER])]);
        assert_eq!(t.start(Some(&Secret::new(" ".into()))), None, "a blank stored key is no key");
        assert_eq!(t.key_given(Some(Secret::new("sk-typed".into()))), None);
        assert_eq!(t.needs_key(), None);
        assert_eq!(t.poll(), Some(Outcome::Success(SUCCEEDED.into())));
        assert!(header(&rec.requests()[0], "Authorization") == Some("Bearer sk-typed"), "the typed key is sent");
        let (o, reqs) = outcome(probe(OPENAI), Some(&Secret::new("sk-stored".into())), Reply::ok(&[ANSWER]));
        assert_eq!(o, Outcome::Success(SUCCEEDED.into()));
        assert!(header(&reqs[0], "Authorization") == Some("Bearer sk-stored"), "the stored key is sent");

        // The App Review key succeeds without a request, even for a private host.
        for url in [OPENAI, LOCAL] {
            let (mut t, rec) = tester(probe(url), vec![]);
            assert_eq!(t.start(Some(&Secret::new(REVIEW_KEY.into()))), Some(Outcome::Success(PLACEHOLDER_IN_USE.into())));
            assert!(rec.requests().is_empty());
        }

        // The consent gate names the probe's own host.
        let mut g = Globals::empty(std::path::Path::new("ai-test-conn-test.toml"));
        g.set(crate::ai_settings::ENABLE_AI, toml::Value::Boolean(true));
        assert_eq!(check_endpoint(&g, Some(&endpoint(&probe(OPENAI)))), Check::NeedsConsent { provider: "OpenAI".into(), host: "api.openai.com".into() });
        assert!(matches!(check_endpoint(&g, Some(&endpoint(&probe(LOCAL)))), Check::NeedsConsent { host, .. } if host == "localhost"));
        let e = endpoint(&probe(LOCAL));
        assert_eq!((e.model.as_str(), e.url.as_str(), e.api, e.custom_headers.len()), ("qwen3", LOCAL, Api::ChatCompletions, 1));
        assert_eq!(e.vendor, manual_vendor(Api::ChatCompletions, LOCAL, "qwen3"));
    }
}
