//! Ollama model discovery and its cache: the `/api/tags` and `/api/show` requests over the
//! `Transport`, their parsing, and the per-server cache the model pickers and dynamic entries
//! read. Pure over `Transport`; ports `AITerm/OllamaModelDiscovery.swift` (`OD`) and
//! `AITerm/OllamaModelCache.swift` (`OC`), with the Settings texts of
//! `Settings/GeneralPreferencesViewController.m` (`G`). Discovery sends no user content and
//! bypasses the consent gate and `aiProxy`, as iTerm2's direct URLSession calls do (ruling
//! 12b-5, Review Focus 9).

use crate::ai_catalog::{Feature, Model};
use crate::ai_model::{Discovered, Server, DEFAULT_CONTEXT};
use crate::ai_models::{self, OLLAMA_ENDPOINT};
use crate::ai_policy;
use crate::ai_transport::{self, Call, HttpEvent, HttpRequest, Notify, Options, Proxy, Transport};
use crate::globals::Globals;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Each discovery request's idle timeout (`OC:110`).
pub const TIMEOUT: Duration = Duration::from_secs(10);
/// A healthy server is asked again after this long (`OC:111`).
pub const HEALTHY_TTL: Duration = Duration::from_secs(300);
/// A failing server is asked again after this long (`OC:112`).
pub const RETRY_TTL: Duration = Duration::from_secs(5);
/// Automatic retries after a failure; a forced refresh always runs (`OC:115`, `OC:358`).
pub const MAX_RETRIES: u32 = 6;
/// Empty lists in a row after which a server that had models is believed empty (`OC:120`).
pub const EMPTY_STREAK: u32 = 3;
/// iTerm2's defaults key for the persisted models, the top-level key of the cache file.
pub const PERSIST_KEY: &str = "NoSyncOllamaDiscoveredModels";

/// The chat paths discovery strips to find the server's base (`OD:19-31`).
const CHAT_SUFFIXES: [&str; 4] = ["/api/chat", "/api/generate", "/v1/chat/completions", "/v1/completions"];

/// A URL cut at the end of its authority and at the end of its path: ("scheme://authority",
/// path, "?query#fragment").
fn split_url(url: &str) -> Option<(&str, &str, &str)> {
    let (scheme, rest) = url.split_once("://")?;
    let end = scheme.len() + 3 + rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let after = &url[end..];
    let path_end = after.find(['?', '#']).unwrap_or(after.len());
    Some((&url[..end], &after[..path_end], &after[path_end..]))
}

/// The endpoint with a scheme: a scheme-less "host:port[/path]" reads as http (`OD:36-42`).
fn with_scheme(endpoint: &str) -> String {
    if ai_policy::url_parts(endpoint).is_none() && !endpoint.contains("://") {
        format!("http://{endpoint}")
    } else {
        endpoint.to_string()
    }
}

/// A discovery URL at the same base as the chat path: the scheme, authority (userinfo too),
/// query and fragment kept, a recognized chat suffix replaced, any other path dropped
/// (`OD:35-55`).
fn discovery_url(endpoint: &str, api_path: &str) -> Result<String, String> {
    let url = with_scheme(endpoint);
    let parts = ai_policy::url_parts(&url).and(split_url(&url));
    let Some((head, path, tail)) = parts else { return Err(no_tags_url(endpoint)) };
    let base = CHAT_SUFFIXES.iter().find_map(|s| path.strip_suffix(s)).unwrap_or("");
    Ok(format!("{head}{base}{api_path}{tail}"))
}

/// The server's `/api/tags` (`OD:60-62`).
pub fn tags_url(endpoint: &str) -> Result<String, String> {
    discovery_url(endpoint, "/api/tags")
}

/// The server's `/api/show` (`OD:333-335`).
pub fn show_url(endpoint: &str) -> Result<String, String> {
    discovery_url(endpoint, "/api/show")
}

/// "scheme://host:port" of the server behind an endpoint, an IPv6 host bracketed, without the
/// userinfo; the endpoint itself when it names no server (`OD:202-214`).
pub fn server_label(endpoint: &str) -> String {
    let Ok(url) = tags_url(endpoint) else { return endpoint.to_string() };
    let Some((head, _, _)) = split_url(&url) else { return endpoint.to_string() };
    let (scheme, authority) = head.split_once("://").unwrap_or(("", head));
    let hostport = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let (host, after) = match hostport.strip_prefix('[') {
        Some(inner) => inner.split_once(']').unwrap_or((inner, "")),
        None => hostport.split_once(':').map_or((hostport, ""), |(h, p)| (h, p)),
    };
    let port = after.trim_start_matches(':');
    let host = if host.contains(':') { format!("[{host}]") } else { host.to_string() };
    if !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) {
        format!("{scheme}://{host}:{port}")
    } else {
        format!("{scheme}://{host}")
    }
}

/// One installed model as `/api/tags` describes it; `/api/show` may complete it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    pub name: String,
    pub context: i64,
    pub capabilities: Option<Vec<String>>,
}

/// One `models` element; None when it is malformed, which skips only that element (`OD:78-99`).
fn found_of(v: &Value) -> Option<Found> {
    let o = v.as_object()?;
    let name = o.get("name")?.as_str()?.to_string();
    let capabilities = match o.get("capabilities") {
        None | Some(Value::Null) => None,
        Some(Value::Array(a)) => Some(a.iter().map(|c| c.as_str().map(str::to_string)).collect::<Option<Vec<_>>>()?),
        Some(_) => return None,
    };
    let context = match o.get("details") {
        None | Some(Value::Null) => None,
        Some(Value::Object(d)) => match d.get("context_length") {
            None | Some(Value::Null) => None,
            Some(n) => Some(n.as_i64()?),
        },
        Some(_) => return None,
    };
    Some(Found { name, context: context.unwrap_or(DEFAULT_CONTEXT), capabilities })
}

/// The `models` array of a body, or None when the body is not `{models: [...]}`.
fn models_array(body: &[u8]) -> Option<Vec<Value>> {
    match serde_json::from_slice::<Value>(body).ok()? {
        Value::Object(mut o) => match o.remove("models")? {
            Value::Array(a) => Some(a),
            _ => None,
        },
        _ => None,
    }
}

/// An `/api/tags` body in server order (`OD:136-157`); Err when it is not a tags answer at all
/// (an error page, a proxy's 401 page), which the cache counts as a failure.
pub fn parse_tags(body: &[u8]) -> Result<Vec<Found>, String> {
    let list = models_array(body).ok_or_else(|| "not an /api/tags answer".to_string())?;
    Ok(list.iter().filter_map(found_of).collect())
}

/// The models `/api/tags` fully described (capabilities and a positive context), which need no
/// `/api/show` (`OD:106-113`).
fn fully_described(body: &[u8]) -> HashSet<String> {
    let Some(list) = models_array(body) else { return HashSet::new() };
    list.iter()
        .filter(|v| v.get("capabilities").is_some_and(|c| !c.is_null()) && v.pointer("/details/context_length").and_then(Value::as_i64).is_some_and(|n| n > 0))
        .filter_map(found_of)
        .map(|f| f.name)
        .collect()
}

/// A JSON number as `NSNumber.intValue` reads it.
fn int_of(v: &Value) -> Option<i64> {
    v.as_i64().or_else(|| v.as_f64().map(|f| f as i64))
}

/// Whether a `model_info` key belongs to a vision or projector block (`OD:397-399`).
fn projector_key(key: &str) -> bool {
    key.starts_with("clip.") || key.starts_with("mmproj") || key.contains(".vision.")
}

/// An `/api/show` body's capabilities (an empty list is authoritative) and context window: the
/// model's own architecture's, else the largest text-model one (`OD:361-393`).
pub fn parse_show(body: &[u8]) -> (Option<Vec<String>>, Option<i64>) {
    let Ok(Value::Object(json)) = serde_json::from_slice::<Value>(body) else { return (None, None) };
    let capabilities = json.get("capabilities").and_then(Value::as_array).and_then(|a| a.iter().map(|c| c.as_str().map(str::to_string)).collect::<Option<Vec<_>>>());
    let context = json.get("model_info").and_then(Value::as_object).and_then(|info| {
        let own = info.get("general.architecture").and_then(Value::as_str).and_then(|arch| info.get(&format!("{arch}.context_length"))).and_then(int_of);
        own.or_else(|| info.iter().filter(|(k, _)| k.ends_with(".context_length") && !projector_key(k)).filter_map(|(_, v)| int_of(v)).max())
    });
    (capabilities, context)
}

/// A discovered model as a request uses it (`OD:140-168`): always streaming, the capabilities
/// as features, the context as both token limits.
pub fn to_model(found: &Found, endpoint: &str) -> Model {
    let caps: HashSet<&str> = found.capabilities.iter().flatten().map(String::as_str).collect();
    let mut m = crate::ai_model::pending_ollama_model();
    m.name = found.name.clone();
    m.url = endpoint.to_string();
    m.context_window_tokens = found.context;
    m.max_response_tokens = found.context;
    for (cap, feature) in [("tools", Feature::FunctionCalling), ("thinking", Feature::ConfigurableThinking), ("vision", Feature::Vision)] {
        if caps.contains(cap) {
            m.features.insert(feature);
        }
    }
    m
}

/// The cache's key for an endpoint reached with a set of headers (`OC:92-108`): the endpoint
/// alone without headers, else "|" and the first 8 bytes of SHA-256 over the sorted
/// "name\u{1}value" pairs joined by "\u{1}", so a credential never appears in the key.
pub fn bucket_key(endpoint: &str, headers: &[(String, String)]) -> String {
    use sha2::{Digest, Sha256};
    if headers.is_empty() {
        return endpoint.to_string();
    }
    let mut fields: Vec<String> = headers.iter().map(|(n, v)| format!("{n}\u{1}{v}")).collect();
    fields.sort();
    let digest = Sha256::digest(fields.join("\u{1}").as_bytes());
    let hex: String = digest[..8].iter().map(|b| format!("{b:02x}")).collect();
    format!("{endpoint}|{hex}")
}

/// The models `found` holds for a server reached with these headers (one bucket).
pub fn models_at<'a>(found: &'a Discovered, endpoint: &str, headers: &[(String, String)]) -> &'a [Model] {
    let key = bucket_key(endpoint, headers);
    found.servers.iter().find(|s| bucket_key(&s.endpoint, &s.headers) == key).map_or(&[], |s| &s.models)
}

/// Their names, in server order (`OC:175-181`, the editor's popups).
pub fn names_at(found: &Discovered, endpoint: &str, headers: &[(String, String)]) -> Vec<String> {
    models_at(found, endpoint, headers).iter().map(|m| m.name.clone()).collect()
}

/// The names path's reading of an `/api/tags` body (`OD:219-254`): the names in server order,
/// or `NO_MODELS` for an empty list or a body that is not a tags answer.
pub fn names(outcome_body: &[u8]) -> Result<Vec<String>, String> {
    let names: Vec<String> = parse_tags(outcome_body).unwrap_or_default().into_iter().map(|f| f.name).collect();
    if names.is_empty() {
        Err(NO_MODELS.to_string())
    } else {
        Ok(names)
    }
}

/// Single-model Fetch Models' answer once the refresh finished (`G:1400-1424`): a transport
/// failure's text, `NO_MODELS` for an unparseable answer or an empty server, else the names the
/// cache now holds for that server.
pub fn fetched_names(o: &Outcome, names: &[String]) -> Result<Vec<String>, String> {
    if o.failed {
        return Err(o.error.clone().unwrap_or_else(|| NO_MODELS.to_string()));
    }
    if names.is_empty() {
        return Err(NO_MODELS.to_string());
    }
    Ok(names.to_vec())
}

// Texts (`OD:224-249`, `G:1045-1058`, `G:1350-1424`, `G:2798-2812`, `G:3064-3104`).
pub fn no_tags_url(endpoint: &str) -> String {
    format!("Could not derive an /api/tags URL from \u{201c}{endpoint}\u{201d}.")
}
pub fn unreachable(host: &str, error: &str) -> String {
    format!("Could not reach Ollama at {host}: {error}")
}
/// iTerm2's text for a URLSession answer with no data, which WinHTTP never gives (a completed
/// request always has a body, possibly empty); kept for the record, unused.
pub const NO_RESPONSE: &str = "Ollama returned no response.";
pub const NO_MODELS: &str = "No installed models were found on the Ollama server.";
pub const PROBE_TITLE: &str = "Contacting Ollama\u{2026}";
pub const PROBE_BODY: &str = "Fetching the models installed on your local Ollama server.";
pub const UNREACHABLE_TITLE: &str = "Could Not Reach Ollama";
pub fn unreachable_body(endpoint: &str) -> String {
    format!("Could not reach the Ollama server at {endpoint}. Make sure Ollama is running, then try again.")
}
pub const NO_MODELS_FOUND: &str = "No models found (is Ollama running?)";
pub const SAME_AS_REGULAR: &str = "Same as regular model";
pub use crate::ai_models::{FETCH_MODELS, REFRESH_MODELS};
pub const FETCHING: &str = "Fetching\u{2026}";
pub const NO_MODELS_DISCOVERED: &str = "No models discovered (click Refresh Models)";
pub const REACH_SERVER_TITLE: &str = "Could Not Reach Server";
pub const REACH_SERVER_BODY: &str = "Could not reach the Ollama server to list its models. Check that it is running and the URL is correct.";
pub const REFRESHED_TITLE: &str = "Models Refreshed";
pub fn refreshed_body(count: usize) -> String {
    format!("Found {count} installed models. Choose one from the Model popup, or \u{201c}All installed models\u{201d} to expose every one.")
}
pub const FETCH_FAILED_TITLE: &str = "Could Not Fetch Models";
pub const ALL_INSTALLED: &str = "All installed models";
pub const MISSING_URL_TITLE: &str = "Missing URL";
pub const MISSING_URL_BODY: &str = "Enter the server URL before fetching models.";

/// Names one refresh, so its caller can collect the outcome.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Ticket(u64);

/// How a refresh ended (iTerm2's `(count, failed)` completion): the models the cache holds for
/// the server after it, and for a transport failure the text the names path shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outcome {
    pub count: usize,
    pub failed: bool,
    pub error: Option<String>,
}

/// One server's cached state (`OC:29-42`).
struct Entry {
    endpoint: String,
    headers: Vec<(String, String)>,
    models: Vec<Model>,
    have_succeeded: bool,
    /// None for a snapshot restored from the file, which is due at once.
    last_attempt: Option<Instant>,
    failures: u32,
    empties: u32,
}

/// Where a running refresh is.
enum Stage {
    Tags(Box<dyn Call>),
    /// The tags answer, and the `/api/show` calls still out, by model index.
    Show { found: Vec<Found>, calls: Vec<(usize, Box<dyn Call>)> },
}

/// A refresh in flight, and the tickets waiting on it (none for an automatic one).
struct Run {
    endpoint: String,
    headers: Vec<(String, String)>,
    tickets: Vec<Ticket>,
    stage: Stage,
}

/// What a finished run found: the models, or a failure with the names path's text.
type Fetched = Result<Vec<Found>, Option<String>>;

/// The discovered models of every server Tandem asked, by bucket (`OC:22-125`). At most one
/// refresh runs per bucket; another request joins it. Refreshes run on the loop's ticks: a
/// configured server is asked when first seen, 300 s after a healthy answer, and 5 s after a
/// failure, up to `MAX_RETRIES` automatic retries.
#[derive(Default)]
pub struct Cache {
    entries: BTreeMap<String, Entry>,
    running: BTreeMap<String, Run>,
    done: HashMap<Ticket, Outcome>,
    next_ticket: u64,
    /// The cache file; None keeps everything in memory.
    path: Option<PathBuf>,
    /// The buckets the last tick's configurations name; None before the first tick.
    configured: Option<BTreeSet<String>>,
    /// Buckets restored from the file, dropped at the first tick unless configured.
    restored: BTreeSet<String>,
}

/// The fixed options of every discovery request: the system proxy, never `aiProxy`.
fn options() -> Options {
    Options { idle: TIMEOUT, proxy: Proxy::System }
}

/// A transport error's own text: iTerm2 shows URLSession's `localizedDescription` bare, and
/// ignores the status (`OD:234-250`). None for an answer that arrived (whatever its status),
/// which is parsed.
fn transport_failure(error: &Option<String>) -> Option<String> {
    let e = error.as_deref()?;
    let status = e.strip_prefix("HTTP request failed with status ").and_then(|r| r.strip_suffix('.')).is_some_and(|n| n.parse::<u32>().is_ok());
    if status {
        return None;
    }
    Some(e.strip_prefix(&ai_transport::transport_error("")).unwrap_or(e).to_string())
}

/// The last event of a call once it ended; None while it runs.
fn poll(call: &mut dyn Call) -> Option<(String, Option<String>)> {
    while let Some(ev) = call.try_event() {
        if let HttpEvent::Done { body, error } = ev {
            return Some((body, error));
        }
    }
    None
}

impl Cache {
    /// The cache seeded from the file at `path` (a missing or unreadable file seeds nothing);
    /// later `save`s write there (`OC:412-486`). Restored models answer at once and are due
    /// for a refresh.
    pub fn load(path: &Path) -> Cache {
        let mut c = Cache { path: Some(path.to_path_buf()), ..Cache::default() };
        let Ok(text) = std::fs::read(path) else { return c };
        let Ok(Value::Object(mut root)) = serde_json::from_slice::<Value>(&text) else { return c };
        let Some(Value::Object(buckets)) = root.remove(PERSIST_KEY) else { return c };
        for (bucket, value) in buckets {
            let (endpoint, list) = match &value {
                Value::Object(o) => match (o.get("endpoint").and_then(Value::as_str), o.get("models").and_then(Value::as_array)) {
                    (Some(e), Some(l)) => (e.to_string(), l.clone()),
                    _ => continue,
                },
                // Written before buckets existed: keyed by the endpoint, the list itself.
                Value::Array(l) => (bucket.clone(), l.clone()),
                _ => continue,
            };
            let models = list.iter().filter_map(|d| restored_model(d, &endpoint)).collect();
            c.entries.insert(bucket.clone(), Entry { endpoint, headers: Vec::new(), models, have_succeeded: true, last_attempt: None, failures: 0, empties: 0 });
            c.restored.insert(bucket);
        }
        c
    }

    /// Writes the models of the configured servers that answered (`OC:443-453`, `OC:488-496`);
    /// nothing before the first tick has named the configured servers. A failed write is not
    /// shown: the file only saves a refresh at the next launch.
    pub fn save(&self) {
        let (Some(path), Some(configured)) = (&self.path, &self.configured) else { return };
        let mut buckets = serde_json::Map::new();
        for (bucket, e) in self.entries.iter().filter(|(b, e)| e.have_succeeded && configured.contains(*b)) {
            let models: Vec<Value> = e.models.iter().map(persisted_model).collect();
            buckets.insert(bucket.clone(), serde_json::json!({ "endpoint": e.endpoint, "models": models }));
        }
        let mut root = serde_json::Map::new();
        root.insert(PERSIST_KEY.to_string(), Value::Object(buckets));
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(path, Value::Object(root).to_string());
    }

    /// The cached models of one server (`OC:175-181`), without asking it.
    pub fn models(&self, endpoint: &str, headers: &[(String, String)]) -> Vec<Model> {
        self.entries.get(&bucket_key(endpoint, headers)).map(|e| e.models.clone()).unwrap_or_default()
    }

    /// The cached models of these configurations, for model resolution.
    pub fn discovered(&self, configs: &[(String, Vec<(String, String)>)]) -> Discovered {
        let servers = configs
            .iter()
            .filter_map(|(endpoint, headers)| {
                let e = self.entries.get(&bucket_key(endpoint, headers))?;
                Some(Server { endpoint: endpoint.clone(), headers: headers.clone(), models: e.models.clone() })
            })
            .collect();
        Discovered { servers }
    }

    /// Every server asked this session, configured or only probed from the editor (the
    /// editor's popups read a URL that is not saved yet, `OC:172-181`).
    pub fn all(&self) -> Discovered {
        let servers = self.entries.values().map(|e| Server { endpoint: e.endpoint.clone(), headers: e.headers.clone(), models: e.models.clone() }).collect();
        Discovered { servers }
    }

    fn ticket(&mut self) -> Ticket {
        self.next_ticket += 1;
        Ticket(self.next_ticket)
    }

    /// Asks a server now (`OC:187-260`) and names the answer's ticket. A request while one runs
    /// for the same server joins it, so a background refresh never swallows the person's.
    /// `force` is iTerm2's flag for a person's request; since a join always reports the
    /// running refresh's answer, both kinds behave alike here.
    pub fn refresh(&mut self, endpoint: &str, headers: &[(String, String)], force: bool, transport: &dyn Transport, notify: Notify) -> Ticket {
        let _ = force;
        let t = self.ticket();
        self.start(endpoint, headers, Some(t), transport, notify, None);
        t
    }

    /// Starts a refresh of a server unless one runs, which the ticket joins. `now` stamps an
    /// immediate failure; None stamps it at the next tick's time (a direct `refresh`).
    fn start(&mut self, endpoint: &str, headers: &[(String, String)], ticket: Option<Ticket>, transport: &dyn Transport, notify: Notify, now: Option<Instant>) {
        let bucket = bucket_key(endpoint, headers);
        if let Some(run) = self.running.get_mut(&bucket) {
            run.tickets.extend(ticket);
            return;
        }
        let tickets: Vec<Ticket> = ticket.into_iter().collect();
        let url = match tags_url(endpoint) {
            Ok(url) => url,
            Err(e) => {
                self.finish(&bucket, endpoint, headers, &tickets, Err(Some(e)), now.unwrap_or_else(Instant::now));
                return;
            }
        };
        let request = HttpRequest { method: "GET", url, headers: ai_policy::merged_headers(&[], headers), body: Vec::new(), stream: false };
        let call = transport.start(request, options(), notify);
        self.running.insert(bucket, Run { endpoint: endpoint.to_string(), headers: headers.to_vec(), tickets, stage: Stage::Tags(call) });
    }

    /// Advances every running refresh and starts the configured servers that are due (the
    /// default endpoint and every dynamic entry, `configs`). True when any server's models
    /// changed, which the App answers by redrawing the pickers and saving.
    pub fn tick(&mut self, now: Instant, transport: &dyn Transport, configs: &[(String, Vec<(String, String)>)], notify: Notify) -> bool {
        let configured: BTreeSet<String> = configs.iter().map(|(e, h)| bucket_key(e, h)).collect();
        // The file keeps no header values: a restored bucket learns them from its configuration.
        for (endpoint, headers) in configs {
            if let Some(e) = self.entries.get_mut(&bucket_key(endpoint, headers)) {
                e.headers = headers.clone();
            }
        }
        for bucket in std::mem::take(&mut self.restored) {
            if !configured.contains(&bucket) && !self.running.contains_key(&bucket) {
                self.entries.remove(&bucket);
            }
        }
        self.configured = Some(configured);
        let mut changed = false;
        let buckets: Vec<String> = self.running.keys().cloned().collect();
        for bucket in buckets {
            changed |= self.advance(&bucket, transport, &notify, now);
        }
        for (endpoint, headers) in configs {
            let bucket = bucket_key(endpoint, headers);
            if !self.running.contains_key(&bucket) && self.due_at(&bucket, now).is_some_and(|at| at <= now) {
                self.start(endpoint, headers, None, transport, notify.clone(), Some(now));
            }
        }
        changed
    }

    /// When a server not being asked is due (`OC:144-156`): at once when never asked or only
    /// restored, else after the healthy or retry interval; never once automatic retries ran
    /// out.
    fn due_at(&self, bucket: &str, now: Instant) -> Option<Instant> {
        let Some(e) = self.entries.get(bucket) else { return Some(now) };
        if e.failures > MAX_RETRIES {
            return None;
        }
        let Some(last) = e.last_attempt else { return Some(now) };
        let healthy = e.have_succeeded && e.failures == 0;
        Some(last + if healthy { HEALTHY_TTL } else { RETRY_TTL })
    }

    /// Polls one running refresh; true when it finished and changed the models.
    fn advance(&mut self, bucket: &str, transport: &dyn Transport, notify: &Notify, now: Instant) -> bool {
        let Some(mut run) = self.running.remove(bucket) else { return false };
        let result: Option<Fetched> = match &mut run.stage {
            Stage::Tags(call) => match poll(call.as_mut()) {
                None => None,
                Some((body, error)) => {
                    if let Some(bare) = transport_failure(&error) {
                        let host = tags_url(&run.endpoint).ok().and_then(|u| ai_policy::url_parts(&u).map(|p| p.host)).unwrap_or_else(|| "the server".to_string());
                        Some(Err(Some(unreachable(&host, &bare))))
                    } else {
                        match parse_tags(body.as_bytes()) {
                            Err(_) => Some(Err(None)),
                            Ok(found) => {
                                let complete = fully_described(body.as_bytes());
                                let calls = found
                                    .iter()
                                    .enumerate()
                                    .filter(|(_, f)| !complete.contains(&f.name))
                                    .filter_map(|(i, f)| {
                                        let url = show_url(&run.endpoint).ok()?;
                                        let headers = ai_policy::merged_headers(&[("Content-Type".to_string(), "application/json".to_string())], &run.headers);
                                        let body = serde_json::json!({ "model": f.name }).to_string().into_bytes();
                                        Some((i, transport.start(HttpRequest { method: "POST", url, headers, body, stream: false }, options(), notify.clone())))
                                    })
                                    .collect::<Vec<_>>();
                                if calls.is_empty() {
                                    Some(Ok(found))
                                } else {
                                    run.stage = Stage::Show { found, calls };
                                    None
                                }
                            }
                        }
                    }
                }
            },
            Stage::Show { found, calls } => {
                // A failed /api/show keeps what the tags said (`OD:295-326`).
                calls.retain_mut(|(i, call)| match poll(call.as_mut()) {
                    None => true,
                    Some((body, error)) => {
                        if transport_failure(&error).is_none() {
                            let (caps, context) = parse_show(body.as_bytes());
                            if let Some(caps) = caps {
                                found[*i].capabilities = Some(caps);
                            }
                            if let Some(n) = context.filter(|n| *n > 0) {
                                found[*i].context = n;
                            }
                        }
                        false
                    }
                });
                calls.is_empty().then(|| Ok(std::mem::take(found)))
            }
        };
        match result {
            None => {
                self.running.insert(bucket.to_string(), run);
                false
            }
            Some(fetched) => self.finish(bucket, &run.endpoint, &run.headers, &run.tickets, fetched, now),
        }
    }

    /// Records a finished refresh (`OC:281-389`, one fetch per round here) and answers its
    /// tickets. A server that had models and answers an empty list keeps them, as a soft
    /// failure, until the `EMPTY_STREAK`th empty list in a row. True when the models changed.
    fn finish(&mut self, bucket: &str, endpoint: &str, headers: &[(String, String)], tickets: &[Ticket], fetched: Fetched, now: Instant) -> bool {
        let e = self.entries.entry(bucket.to_string()).or_insert_with(|| Entry {
            endpoint: endpoint.to_string(),
            headers: headers.to_vec(),
            models: Vec::new(),
            have_succeeded: false,
            last_attempt: None,
            failures: 0,
            empties: 0,
        });
        e.endpoint = endpoint.to_string();
        e.headers = headers.to_vec();
        e.last_attempt = Some(now);
        let result = fetched.as_ref().ok().map(|found| found.iter().map(|f| to_model(f, endpoint)).collect::<Vec<Model>>());
        let suspicious = result.as_ref().is_some_and(|r| r.is_empty()) && e.have_succeeded && !e.models.is_empty();
        let accept_empty = suspicious && e.empties + 1 >= EMPTY_STREAK;
        let soft = suspicious && !accept_empty;
        let mut changed = false;
        match result {
            Some(models) if !soft => {
                changed = !e.have_succeeded || e.models != models;
                e.models = models;
                e.have_succeeded = true;
                e.failures = 0;
                e.empties = 0;
            }
            _ => {
                e.empties = if soft { e.empties + 1 } else { 0 };
                e.failures += 1;
            }
        }
        let succeeded = fetched.is_ok();
        let outcome = Outcome { count: if succeeded { e.models.len() } else { 0 }, failed: !succeeded, error: fetched.err().flatten() };
        for t in tickets {
            self.done.insert(*t, outcome.clone());
        }
        changed
    }

    /// A refresh's outcome once it finished, taken once.
    pub fn finished(&mut self, t: Ticket) -> Option<Outcome> {
        self.done.remove(&t)
    }

    /// When the next automatic refresh of a configured server is due (running refreshes wake the
    /// loop through their `notify`); None when none will start by itself.
    pub fn next_due(&self, now: Instant) -> Option<Instant> {
        self.configured.as_ref()?.iter().filter(|b| !self.running.contains_key(*b)).filter_map(|b| self.due_at(b, now)).min()
    }
}

/// iTerm2's persisted features, in its order (`OC:498-503`).
const PERSISTED_FEATURES: [(Feature, &str); 4] =
    [(Feature::Streaming, "streaming"), (Feature::FunctionCalling, "functionCalling"), (Feature::ConfigurableThinking, "configurableThinking"), (Feature::Vision, "vision")];

fn persisted_model(m: &Model) -> Value {
    let features: Vec<&str> = PERSISTED_FEATURES.iter().filter(|(f, _)| m.features.contains(f)).map(|(_, n)| *n).collect();
    serde_json::json!({ "name": m.name, "ctx": m.context_window_tokens, "features": features })
}

fn restored_model(d: &Value, endpoint: &str) -> Option<Model> {
    let name = d.get("name")?.as_str()?.to_string();
    let context = d.get("ctx").and_then(Value::as_i64).unwrap_or(DEFAULT_CONTEXT);
    let names: HashSet<&str> = d.get("features").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
    let mut m = to_model(&Found { name, context, capabilities: None }, endpoint);
    m.features = PERSISTED_FEATURES.iter().filter(|(_, n)| names.contains(n)).map(|(f, _)| *f).collect();
    Some(m)
}

/// The cache file, `%APPDATA%\Tandem\ollama-models.json` beside `settings.toml` (Decision 8).
#[cfg(windows)]
pub fn default_path() -> PathBuf {
    crate::globals::default_path().with_file_name("ollama-models.json")
}

/// The servers discovery keeps current (`MD:415-428`): the built-in Ollama vendor's default
/// endpoint without headers, then every dynamic entry with a URL, with its headers.
pub fn configs(g: &Globals) -> Vec<(String, Vec<(String, String)>)> {
    let mut out = vec![(OLLAMA_ENDPOINT.to_string(), Vec::new())];
    out.extend(ai_models::read(g).into_iter().filter(|m| m.dynamic_models && !m.url.is_empty()).map(|m| (m.url, m.custom_headers)));
    out
}

/// What the Ollama probe sheet does with the answer (`G:3055-3104`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Probe {
    /// The server answered: the Ollama vendor stays the default.
    Keep,
    /// Cancelled: the prior default comes back silently.
    Restore,
    /// Unreachable: the prior default comes back and this is shown under `UNREACHABLE_TITLE`.
    RestoreAndTell(String),
}

pub fn probe_outcome(o: &Outcome, canceled: bool) -> Probe {
    if canceled {
        Probe::Restore
    } else if o.failed {
        Probe::RestoreAndTell(unreachable_body(OLLAMA_ENDPOINT))
    } else {
        Probe::Keep
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai_catalog::Feature;
    use crate::ai_models::OLLAMA_ENDPOINT;
    use crate::ai_settings::{Api, Vendor};
    use crate::ai_transport::{options_from, Options, Proxy, Recorded, Reply};
    use std::collections::BTreeSet;
    use std::sync::Arc;
    use toml::{Table, Value};

    const TAGS: &[u8] = br#"{"models":[{"name":"llama3.3","details":{"context_length":131072},"capabilities":["completion","tools"]},{"name":"qwen3:8b"}]}"#;
    /// Every model fully described, so no /api/show follows.
    const TAGS_FULL: &[u8] = br#"{"models":[{"name":"llama3.3","details":{"context_length":131072},"capabilities":["tools"]},{"name":"phi4","details":{"context_length":16384},"capabilities":["vision","thinking"]}]}"#;
    const SHOW: &[u8] = br#"{"capabilities":["completion","thinking"],"model_info":{"general.architecture":"qwen3","qwen3.context_length":40960}}"#;
    const EMPTY: &[u8] = br#"{"models":[]}"#;
    const GPU: &str = "http://gpu.test:11434/api/chat";

    fn noop() -> Notify {
        Arc::new(|| {})
    }

    fn strings(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
        list.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect()
    }

    /// Ticks until the refresh behind `t` finishes.
    fn run(c: &mut Cache, transport: &dyn Transport, t: Ticket) -> Outcome {
        for _ in 0..10 {
            c.tick(Instant::now(), transport, &[], noop());
            if let Some(o) = c.finished(t) {
                return o;
            }
        }
        panic!("the refresh never finished");
    }

    fn refreshed(replies: Vec<Reply>, endpoint: &str, headers: &[(String, String)]) -> (Cache, Recorded, Outcome) {
        let rec = Recorded::new(replies);
        let mut c = Cache::default();
        let t = c.refresh(endpoint, headers, true, &rec, noop());
        let o = run(&mut c, &rec, t);
        (c, rec, o)
    }

    fn names_of(models: &[Model]) -> Vec<&str> {
        models.iter().map(|m| m.name.as_str()).collect()
    }

    #[test]
    fn tags_urls_follow_iterm2() {
        for (endpoint, expected) in [
            ("http://localhost:11434/api/chat", "http://localhost:11434/api/tags"),
            ("https://h.test/proxy/v1/chat/completions", "https://h.test/proxy/api/tags"),
            ("https://h.test/p/api/generate", "https://h.test/p/api/tags"),
            ("http://h:1/v1/completions", "http://h:1/api/tags"),
            ("http://h:1/weird", "http://h:1/api/tags"),
            ("http://h:1", "http://h:1/api/tags"),
            ("localhost:11434", "http://localhost:11434/api/tags"),
            ("http://[::1]:11434/api/chat", "http://[::1]:11434/api/tags"),
            ("http://u:p@h:1/api/chat", "http://u:p@h:1/api/tags"),
        ] {
            assert_eq!(tags_url(endpoint).as_deref(), Ok(expected), "{endpoint}");
            assert_eq!(show_url(endpoint).as_deref(), Ok(expected.replace("/api/tags", "/api/show").as_str()), "{endpoint}");
        }
        assert_eq!(tags_url(""), Err("Could not derive an /api/tags URL from \u{201c}\u{201d}.".to_string()));
        assert_eq!(show_url("http://"), Err(no_tags_url("http://")));
        assert_eq!(server_label("http://h.test:8080/api/chat"), "http://h.test:8080");
        assert_eq!(server_label("http://[::1]:11434/api/chat"), "http://[::1]:11434");
        assert_eq!(server_label("localhost:11434"), "http://localhost:11434");
        assert_eq!(server_label("https://h.test/v1/chat/completions"), "https://h.test");
        assert_eq!(server_label("http://u:p@h:1/api/chat"), "http://h:1", "the label names the server, not the credentials");
        assert_eq!(server_label(""), "");
    }

    #[test]
    fn tags_and_show_follow_iterm2() {
        let found = parse_tags(br#"{"models":[{"name":"llama3.3","details":{"context_length":131072},"capabilities":["completion","tools"]},{"name":"qwen3:8b"},{"bad":1}]}"#).expect("parses");
        assert_eq!(
            found,
            [
                Found { name: "llama3.3".into(), context: 131072, capabilities: Some(strings(&["completion", "tools"])) },
                Found { name: "qwen3:8b".into(), context: 32768, capabilities: None },
            ]
        );
        assert!(parse_tags(b"<html>404</html>").is_err());
        assert!(parse_tags(br#"{"other":[]}"#).is_err());
        assert_eq!(parse_tags(EMPTY), Ok(Vec::new()));

        let show = br#"{"capabilities":["tools","thinking","vision"],"model_info":{"general.architecture":"llama","llama.context_length":8192,"clip.context_length":999}}"#;
        assert_eq!(parse_show(show), (Some(strings(&["tools", "thinking", "vision"])), Some(8192)));
        let no_arch = br#"{"model_info":{"a.context_length":4096,"b.context_length":8192,"clip.context_length":999999,"mmproj.context_length":999999,"x.vision.context_length":999999}}"#;
        assert_eq!(parse_show(no_arch), (None, Some(8192)));
        assert_eq!(parse_show(br#"{"capabilities":[]}"#), (Some(Vec::new()), None), "an empty list is authoritative");
        assert_eq!(parse_show(b"nope"), (None, None));

        let m = to_model(&Found { name: "q".into(), context: 8192, capabilities: Some(strings(&["tools", "thinking", "vision", "completion"])) }, GPU);
        assert_eq!((m.name.as_str(), m.url.as_str(), m.api, m.vendor), ("q", GPU, Api::Llama, Vendor::Llama));
        assert_eq!((m.context_window_tokens, m.max_response_tokens), (8192, 8192));
        assert_eq!(m.features, BTreeSet::from([Feature::Streaming, Feature::FunctionCalling, Feature::ConfigurableThinking, Feature::Vision]));
        let bare = to_model(&Found { name: "b".into(), context: 32768, capabilities: None }, GPU);
        assert_eq!(bare.features, BTreeSet::from([Feature::Streaming]));
    }

    #[test]
    fn discovery_flow_follows_iterm2() {
        let (c, rec, o) = refreshed(vec![Reply::status(200, TAGS), Reply::status(200, SHOW)], OLLAMA_ENDPOINT, &[]);
        assert_eq!(o, Outcome { count: 2, failed: false, error: None });
        let reqs = rec.requests();
        assert_eq!(reqs.len(), 2, "only the model lacking a context or capabilities is shown");
        assert_eq!((reqs[0].method, reqs[0].url.as_str(), reqs[0].stream), ("GET", "http://localhost:11434/api/tags", false));
        assert!(reqs[0].headers.is_empty(), "no auth header");
        assert!(reqs[0].body.is_empty());
        assert_eq!((reqs[1].method, reqs[1].url.as_str()), ("POST", "http://localhost:11434/api/show"));
        assert_eq!(reqs[1].headers, pairs(&[("Content-Type", "application/json")]));
        assert_eq!(reqs[1].body, br#"{"model":"qwen3:8b"}"#);
        assert!(rec.options().iter().all(|o| *o == Options { idle: TIMEOUT, proxy: Proxy::System }));
        let ms = c.models(OLLAMA_ENDPOINT, &[]);
        assert_eq!(names_of(&ms), ["llama3.3", "qwen3:8b"]);
        assert_eq!((ms[0].context_window_tokens, ms[0].features.clone()), (131072, BTreeSet::from([Feature::Streaming, Feature::FunctionCalling])));
        assert_eq!((ms[1].context_window_tokens, ms[1].features.clone()), (40960, BTreeSet::from([Feature::Streaming, Feature::ConfigurableThinking])));

        // A dynamic entry's headers go on both requests.
        let auth = pairs(&[("Authorization", "Bearer t")]);
        let (c, rec, o) = refreshed(vec![Reply::status(200, TAGS), Reply::status(200, SHOW)], GPU, &auth);
        assert_eq!(o.count, 2);
        let reqs = rec.requests();
        assert_eq!(reqs[0].url, "http://gpu.test:11434/api/tags");
        assert_eq!(reqs[0].headers, auth);
        assert_eq!(reqs[1].headers, pairs(&[("Content-Type", "application/json"), ("Authorization", "Bearer t")]));
        assert_eq!(c.models(GPU, &auth).len(), 2);
        assert!(c.models(GPU, &[]).is_empty(), "another header set is another bucket");

        // A failed /api/show keeps what the tags said.
        let (c, _, o) = refreshed(vec![Reply::status(200, TAGS), Reply::failed("The request timed out.")], OLLAMA_ENDPOINT, &[]);
        assert_eq!(o.count, 2);
        let q = &c.models(OLLAMA_ENDPOINT, &[])[1];
        assert_eq!((q.context_window_tokens, q.features.clone()), (32768, BTreeSet::from([Feature::Streaming])));

        // Transport failures read as iTerm2's URLSession error, without the transport prefix.
        let (_, _, o) = refreshed(vec![Reply::failed("Could not connect to the server.")], OLLAMA_ENDPOINT, &[]);
        assert_eq!(o, Outcome { count: 0, failed: true, error: Some("Could not reach Ollama at localhost: Could not connect to the server.".into()) });
        // The status is never checked: a 401 with a list is a success, a 404 page a failure.
        let (_, _, o) = refreshed(vec![Reply::status(401, EMPTY)], OLLAMA_ENDPOINT, &[]);
        assert_eq!(o, Outcome { count: 0, failed: false, error: None });
        let (_, _, o) = refreshed(vec![Reply::status(404, b"<html>404 page not found</html>")], OLLAMA_ENDPOINT, &[]);
        assert_eq!((o.count, o.failed), (0, true));
        let (_, _, o) = refreshed(vec![Reply::status(200, EMPTY)], OLLAMA_ENDPOINT, &[]);
        assert_eq!(o, Outcome { count: 0, failed: false, error: None });
        // A URL that yields no /api/tags URL sends nothing.
        let (_, rec, o) = refreshed(Vec::new(), "", &[]);
        assert_eq!(o, Outcome { count: 0, failed: true, error: Some(no_tags_url("")) });
        assert!(rec.requests().is_empty());

        // The names path (single-model Fetch Models).
        assert_eq!(names(TAGS), Ok(strings(&["llama3.3", "qwen3:8b"])));
        for body in [EMPTY, b"".as_slice(), b"<html>".as_slice()] {
            assert_eq!(names(body), Err("No installed models were found on the Ollama server.".to_string()));
        }
        assert_eq!(NO_RESPONSE, "Ollama returned no response.");
        let ok = Outcome { count: 2, failed: false, error: None };
        assert_eq!(fetched_names(&ok, &strings(&["a", "b"])), Ok(strings(&["a", "b"])));
        assert_eq!(fetched_names(&Outcome { count: 0, failed: false, error: None }, &[]), Err(NO_MODELS.to_string()));
        assert_eq!(fetched_names(&Outcome { count: 0, failed: true, error: None }, &[]), Err(NO_MODELS.to_string()), "an unparseable body");
        assert_eq!(fetched_names(&Outcome { count: 0, failed: true, error: Some("E".into()) }, &[]), Err("E".to_string()));
    }

    #[test]
    fn discovery_ignores_ai_proxy() {
        let mut g = Globals::empty(&std::env::temp_dir().join("tandem-ai-discovery-test.toml"));
        g.set("aiProxy", Value::String("proxy.test:8080".into()));
        let t: Table = r#"v = [{name="d", url="https://gpu.test/api/chat", dynamicModels=true}]"#.parse().unwrap();
        g.set(crate::ai_settings::MANUAL_MODELS, t["v"].clone());
        assert!(matches!(options_from(&g).proxy, Proxy::HttpsOnly { .. }), "model requests would use it");
        let rec = Recorded::new(vec![Reply::status(200, EMPTY), Reply::status(200, EMPTY)]);
        let mut c = Cache::default();
        let now = Instant::now();
        for _ in 0..3 {
            c.tick(now, &rec, &configs(&g), noop());
        }
        let reqs = rec.requests();
        assert_eq!(reqs.iter().map(|r| r.url.as_str()).collect::<Vec<_>>(), ["http://localhost:11434/api/tags", "https://gpu.test/api/tags"]);
        assert!(rec.options().iter().all(|o| o.proxy == Proxy::System));
        assert_eq!(rec.options().len(), 2);
    }

    #[test]
    fn cache_follows_iterm2() {
        let e = "http://h.test:11434/api/chat";
        let cfg = vec![(e.to_string(), Vec::new())];
        let none = Recorded::new(Vec::new());
        let mut c = Cache::default();
        let t0 = Instant::now();
        assert_eq!(c.next_due(t0), None, "nothing configured yet");
        // First sight: due at once.
        let rec = Recorded::new(vec![Reply::status(200, TAGS_FULL)]);
        assert!(!c.tick(t0, &rec, &cfg, noop()));
        assert!(c.tick(t0, &rec, &cfg, noop()), "the models arrived");
        assert_eq!(names_of(&c.models(e, &[])), ["llama3.3", "phi4"]);
        // Healthy: again 300 s later.
        assert_eq!(c.next_due(t0), Some(t0 + HEALTHY_TTL));
        c.tick(t0 + Duration::from_secs(299), &none, &cfg, noop());
        assert!(none.requests().is_empty());
        // A failure keeps the models and retries 5 s later.
        let mut now = t0 + HEALTHY_TTL;
        let fail = Recorded::new((0..8).map(|_| Reply::failed("Could not connect to the server.")).collect());
        c.tick(now, &fail, &cfg, noop());
        assert!(!c.tick(now, &fail, &cfg, noop()));
        assert_eq!(fail.requests().len(), 1);
        assert_eq!(c.models(e, &[]).len(), 2, "a failure keeps the last good models");
        assert_eq!(c.next_due(now), Some(now + RETRY_TTL));
        c.tick(now + Duration::from_secs(4), &none, &cfg, noop());
        assert!(none.requests().is_empty());
        // Six automatic retries after the first failure (iTerm2 retries while failures <= 6), then
        // nothing until a forced refresh.
        for attempt in 2..=7 {
            now += RETRY_TTL;
            c.tick(now, &fail, &cfg, noop());
            c.tick(now, &fail, &cfg, noop());
            assert_eq!(fail.requests().len(), attempt);
        }
        assert_eq!(c.next_due(now), None);
        c.tick(now + Duration::from_secs(3600), &none, &cfg, noop());
        assert!(none.requests().is_empty(), "no automatic retry after {MAX_RETRIES} retries");
        let rec = Recorded::new(vec![Reply::status(200, TAGS_FULL)]);
        let t = c.refresh(e, &[], true, &rec, noop());
        assert_eq!(run(&mut c, &rec, t), Outcome { count: 2, failed: false, error: None });

        // A server that had models and answers an empty list keeps them until the third empty list
        // in a row; the refresh reports what the cache holds.
        let rec = Recorded::new(vec![Reply::status(200, EMPTY), Reply::status(200, EMPTY), Reply::status(200, EMPTY)]);
        for (i, kept) in [(1, 2), (2, 2), (3, 0)] {
            let t = c.refresh(e, &[], true, &rec, noop());
            let o = run(&mut c, &rec, t);
            assert_eq!((o.count, o.failed), (kept, false), "empty list {i}");
            assert_eq!(c.models(e, &[]).len(), kept, "empty list {i}");
        }

        // A forced refresh while one runs joins it: one request, both tickets finish.
        let rec = Recorded::new(vec![Reply::status(200, TAGS_FULL)]);
        let mut c = Cache::default();
        let a = c.refresh(e, &[], false, &rec, noop());
        let b = c.refresh(e, &[], true, &rec, noop());
        assert_ne!(a, b);
        assert_eq!(run(&mut c, &rec, a).count, 2);
        assert_eq!(c.finished(b).map(|o| o.count), Some(2));
        assert_eq!(rec.requests().len(), 1);
        assert_eq!(c.finished(b), None, "an outcome is taken once");

        // Bucket keys.
        assert_eq!(bucket_key("http://h/api/chat", &[]), "http://h/api/chat");
        assert_eq!(bucket_key("http://h/api/chat", &pairs(&[("B", "2"), ("A", "1")])), "http://h/api/chat|ee79863e72d635a6");
        assert_eq!(bucket_key("http://h/api/chat", &pairs(&[("A", "1"), ("B", "2")])), bucket_key("http://h/api/chat", &pairs(&[("B", "2"), ("A", "1")])));
        assert_eq!(bucket_key("http://h/api/chat", &pairs(&[("Authorization", "Bearer t")])), "http://h/api/chat|85fe928ef19ede36");
    }

    #[test]
    fn cache_persists_like_iterm2() {
        let path = std::env::temp_dir().join(format!("tandem-ollama-models-test-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let auth = pairs(&[("Authorization", "Bearer t")]);
        let cfg = vec![(OLLAMA_ENDPOINT.to_string(), Vec::new()), (GPU.to_string(), auth.clone())];
        let mut c = Cache::load(&path);
        let rec = Recorded::new(vec![Reply::status(200, TAGS_FULL), Reply::status(200, TAGS_FULL)]);
        let now = Instant::now();
        c.tick(now, &rec, &cfg, noop());
        assert!(c.tick(now, &rec, &cfg, noop()));
        c.save();
        let text = std::fs::read_to_string(&path).expect("saved");
        assert!(!text.contains("Bearer"), "the header value never reaches the file");
        let v: serde_json::Value = serde_json::from_str(&text).expect("JSON");
        let buckets = &v[PERSIST_KEY];
        let gpu = &buckets[bucket_key(GPU, &auth).as_str()];
        assert_eq!(
            gpu,
            &serde_json::json!({"endpoint": GPU, "models": [
                {"name": "llama3.3", "ctx": 131072, "features": ["streaming", "functionCalling"]},
                {"name": "phi4", "ctx": 16384, "features": ["streaming", "configurableThinking", "vision"]},
            ]})
        );
        assert_eq!(buckets[OLLAMA_ENDPOINT]["endpoint"], OLLAMA_ENDPOINT);

        // Loading restores the models at once (a stale snapshot is due for a refresh) and drops
        // buckets no longer configured.
        let mut c2 = Cache::load(&path);
        assert_eq!(c2.models(GPU, &auth), c.models(GPU, &auth));
        assert_eq!(c2.next_due(now), None, "no configs seen yet");
        let rec = Recorded::new(vec![Reply::failed("Could not connect to the server.")]);
        c2.tick(now, &rec, &cfg[..1], noop());
        assert_eq!(rec.requests().len(), 1, "a restored bucket is refreshed");
        assert!(c2.models(GPU, &auth).is_empty(), "an unconfigured bucket was pruned");
        c2.tick(now, &rec, &cfg[..1], noop());
        assert_eq!(c2.models(OLLAMA_ENDPOINT, &[]).len(), 2, "a failure keeps the restored models");
        c2.save();
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(v[PERSIST_KEY].as_object().map(|o| o.len()), Some(1));
        // discovered() answers the configured buckets.
        let d = c.discovered(&cfg[..1]);
        assert_eq!(d.servers.len(), 1);
        assert_eq!((d.servers[0].endpoint.as_str(), d.servers[0].models.len()), (OLLAMA_ENDPOINT, 2));
        assert_eq!(c.discovered(&cfg).servers.len(), 2);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn configs_follow_iterm2() {
        let mut g = Globals::empty(&std::env::temp_dir().join("tandem-ai-discovery-test.toml"));
        assert_eq!(configs(&g), [(OLLAMA_ENDPOINT.to_string(), Vec::new())]);
        let t: Table = r#"v = [{name="a", url="http://a/v1"}, {name="d", url="http://gpu:11434/api/chat", dynamicModels=true, customHeaders=[{name="X-A", value="1"}]}, {name="e", url="", dynamicModels=true}]"#.parse().unwrap();
        g.set(crate::ai_settings::MANUAL_MODELS, t["v"].clone());
        assert_eq!(configs(&g), [(OLLAMA_ENDPOINT.to_string(), Vec::new()), ("http://gpu:11434/api/chat".to_string(), pairs(&[("X-A", "1")]))]);
    }

    #[test]
    fn settings_probe_follows_iterm2() {
        let ok = Outcome { count: 1, failed: false, error: None };
        let failed = Outcome { count: 0, failed: true, error: Some("x".into()) };
        assert_eq!(probe_outcome(&ok, false), Probe::Keep);
        assert_eq!(probe_outcome(&ok, true), Probe::Restore);
        assert_eq!(probe_outcome(&failed, true), Probe::Restore);
        assert_eq!(
            probe_outcome(&failed, false),
            Probe::RestoreAndTell("Could not reach the Ollama server at http://localhost:11434/api/chat. Make sure Ollama is running, then try again.".into())
        );
        assert_eq!(refreshed_body(3), "Found 3 installed models. Choose one from the Model popup, or \u{201c}All installed models\u{201d} to expose every one.");
        for (text, expected) in [
            (PROBE_TITLE, "Contacting Ollama\u{2026}"),
            (PROBE_BODY, "Fetching the models installed on your local Ollama server."),
            (UNREACHABLE_TITLE, "Could Not Reach Ollama"),
            (NO_MODELS_FOUND, "No models found (is Ollama running?)"),
            (SAME_AS_REGULAR, "Same as regular model"),
            (FETCH_MODELS, "Fetch Models"),
            (REFRESH_MODELS, "Refresh Models"),
            (FETCHING, "Fetching\u{2026}"),
            (NO_MODELS_DISCOVERED, "No models discovered (click Refresh Models)"),
            (REACH_SERVER_TITLE, "Could Not Reach Server"),
            (REACH_SERVER_BODY, "Could not reach the Ollama server to list its models. Check that it is running and the URL is correct."),
            (REFRESHED_TITLE, "Models Refreshed"),
            (FETCH_FAILED_TITLE, "Could Not Fetch Models"),
            (ALL_INSTALLED, "All installed models"),
            (MISSING_URL_TITLE, "Missing URL"),
            (MISSING_URL_BODY, "Enter the server URL before fetching models."),
            (NO_MODELS, "No installed models were found on the Ollama server."),
        ] {
            assert_eq!(text, expected);
        }
        assert_eq!(unreachable("h", "e"), "Could not reach Ollama at h: e");
    }
}
