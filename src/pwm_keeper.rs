//! The Keeper adapter's logic (`iterm2-keeper-adapter`), a port of iTerm2's
//! `pwmplugin/Sources/iterm2-keeper-adapter/main.swift` and `KeeperCommanderClient.swift`
//! (`KCC`) over an `Http` trait, so it is testable with a fake. The executable in `src/bin` wires
//! `WinHttpBlocking` (12b's `ai_winhttp`) and stdio to `pwm_adapter::run_main_with`.
//!
//! Keeper Commander Service Mode REST v2: `POST <base>/executecommand-async` with the command,
//! then `GET status/<id>` until completed, failed or expired, then `GET result/<id>`. The "master
//! password" of this adapter is the service's API key; it travels only in the `api-key` header
//! (never the URL or a body), is a `Secret` from the request or token on, and is never logged,
//! printed, written to a file or put in an argument or environment. A new record password
//! travels only inside a command as `password=$BASE64:<b64>` in the POST body. The session token
//! the protocol requires carries the key (iTerm2's `encodeToken`; no state between calls).
//!
//! The service's answers are untrusted: capped, names and error texts go through
//! `inert_files::display_text`, and a secret the server echoes back is replaced before an error
//! is shown. The service URL comes from the request header (the person's own setting); only
//! http and https are accepted, without userinfo, and plain http only to this machine.

use std::time::{Duration, Instant};

use base64::Engine;
use serde::Serialize;
use serde_json::Value;

use crate::ai_policy::url_parts;
use crate::inert_files::display_text;
use crate::pwm::Secret;
use crate::pwm_adapter::*;
use crate::pwm_source::{Field, Toggle};

/// One blocking HTTP exchange. `Err` is a transport failure's plain text; any HTTP status is
/// an `Ok`. `pause` is the poll interval's wait (a test's fake only counts it).
pub trait Http {
    fn send(&mut self, method: &str, url: &str, headers: &[(String, String)], body: &[u8], timeout: Duration) -> Result<(u16, Vec<u8>), String>;
    fn pause(&mut self, d: Duration);
}

// Timeouts. iTerm2's long request is 300 s; Tandem's adapter is killed at 180 s (pwm_adapter
// `LONG_TIMEOUT`), so the longest wait here is shorter, and a read stays under its 30 s.
const LONG_REQUEST: Duration = Duration::from_secs(150);
const VALIDATION_REQUEST: Duration = Duration::from_secs(120);
const STATUS_POLL: Duration = Duration::from_secs(30);
const INTERACTIVE_READ: Duration = Duration::from_secs(15);
const INITIAL_INTERVAL: Duration = Duration::from_millis(200);
const MAX_INTERVAL: Duration = Duration::from_secs(2);
const QUEUED_STUCK: Duration = Duration::from_secs(10);
const MAX_UNPARSEABLE: u32 = 15;

const MAX_ACCOUNTS: usize = 5000;
const MAX_NAME: usize = 200;
const MAX_TEXT: usize = 300;
const MAX_ID: usize = 128;

const ADVICE: &str = "Update your Service URL and API key in Password Manager settings, and confirm your Keeper Commander server is running.";
const WEDGED: &str = "The Keeper Commander service accepted the request but never started processing it (it stayed queued). The Commander service worker appears to be stuck; restart your Keeper Commander service and try again.";

/// Why a step failed: a text safe to show.
struct Failure(String);

fn fail(text: &str) -> Failure {
    Failure(text.to_string())
}

fn bad_request(e: String) -> Failure {
    Failure(e)
}

// --- URLs ---

/// The v2 API base of a service URL (`KCC:233-243`): a query and fragment dropped, trailing
/// slashes trimmed, and `/api/v2` appended unless the path already ends with `api/v2`.
pub fn v2_base(url: &str) -> String {
    let cut = url.find(['?', '#']).unwrap_or(url.len());
    let mut base = url[..cut].trim_end_matches('/').to_string();
    if !base.ends_with("api/v2") {
        base.push_str("/api/v2");
    }
    base
}

fn is_loopback(host: &str) -> bool {
    let h = host.to_ascii_lowercase();
    h == "localhost" || h == "::1" || h.strip_prefix("127.").is_some_and(|rest| rest.split('.').count() == 3 && rest.split('.').all(|p| p.parse::<u8>().is_ok()))
}

/// The service URL from the header (`KCC:463-480`: `serviceURL`, `apiURL`, then the database
/// path), accepted only as http or https with a host, no userinfo, and plain http only to this
/// machine (the API key would cross the network in clear text otherwise).
fn service_url(header: &RequestHeader) -> Result<String, Failure> {
    let get = |k: &str| header.settings.as_ref().and_then(|s| s.get(k)).map(|v| v.trim().to_string());
    let raw = [get("serviceURL"), get("apiURL"), header.path_to_database.as_ref().map(|p| p.trim().to_string())]
        .into_iter()
        .flatten()
        .find(|v| !v.is_empty())
        .ok_or_else(|| fail("Commander API URL is required in request header (pathToDatabase)."))?;
    let invalid = || fail("Invalid Commander API URL.");
    if raw.chars().any(|c| c.is_whitespace() || c.is_control() || c == '\\') {
        return Err(invalid());
    }
    let parts = url_parts(&raw).ok_or_else(invalid)?;
    let secure = match parts.scheme.to_ascii_lowercase().as_str() {
        "https" => true,
        "http" => false,
        _ => return Err(invalid()),
    };
    if !matches!(parts.after_host, "") && !parts.after_host.strip_prefix(':').is_some_and(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit())) {
        return Err(invalid());
    }
    let authority = raw.split_once("://").map_or("", |(_, r)| r).split(['/', '?', '#']).next().unwrap_or("");
    if authority.contains('@') {
        return Err(fail("The Commander API URL may not contain a user name or password."));
    }
    if !secure && !is_loopback(&parts.host) {
        return Err(fail("A Commander API URL that is not on this computer must use https, so the API key is not sent in clear text."));
    }
    Ok(raw)
}

// --- Records ---

/// One record of a listing (`KeeperRecord`), read leniently from the several shapes Commander
/// answers in.
#[derive(Clone, Default)]
struct Rec {
    uid: Option<String>,
    record_uid: Option<String>,
    upper_uid: Option<String>,
    typ: Option<String>,
    upper_type: Option<String>,
    item_type: Option<String>,
    title: Option<String>,
    upper_title: Option<String>,
    name: Option<String>,
    description: Option<String>,
    details: Option<String>,
    source: Option<String>,
    login: Option<String>,
    username: Option<String>,
    record_category: Option<String>,
}

fn nonempty(s: &Option<String>) -> Option<String> {
    s.as_deref().map(str::trim).filter(|t| !t.is_empty()).map(str::to_string)
}

impl Rec {
    fn from_value(v: &Value) -> Rec {
        let s = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
        Rec {
            uid: s("uid"),
            record_uid: s("record_uid"),
            upper_uid: s("UID"),
            typ: s("type"),
            upper_type: s("Type"),
            item_type: s("Item Type"),
            title: s("title"),
            upper_title: s("Title"),
            name: s("name"),
            description: s("description"),
            details: s("details"),
            source: s("source"),
            login: s("login"),
            username: s("username"),
            record_category: s("record_category"),
        }
    }

    fn effective_uid(&self) -> Option<String> {
        self.uid.clone().or_else(|| self.record_uid.clone()).or_else(|| self.upper_uid.clone())
    }

    fn is_folder(&self) -> bool {
        let is = |t: &Option<String>| t.as_deref().is_some_and(|t| t.eq_ignore_ascii_case("folder"));
        is(&self.typ.clone().or_else(|| self.upper_type.clone())) || is(&self.item_type)
    }

    fn display_title(&self) -> String {
        [&self.title, &self.upper_title, &self.name].into_iter().find_map(nonempty).unwrap_or_else(|| "Untitled".to_string())
    }

    fn details_description(&self) -> Option<String> {
        let d = self.details.as_deref()?;
        let raw = d.split_once("Description:")?.1.trim();
        (!raw.is_empty() && !raw.eq_ignore_ascii_case("none")).then(|| raw.to_string())
    }

    fn list_user_name(&self) -> String {
        if let Some(l) = nonempty(&self.login).or_else(|| nonempty(&self.username)) {
            return l;
        }
        for d in [nonempty(&self.description), self.details_description()].into_iter().flatten() {
            return login_from_description(&d);
        }
        String::new()
    }

    fn source_label(&self) -> Option<&'static str> {
        normalized_source_label(self.record_category.as_deref()).or_else(|| normalized_source_label(self.source.as_deref()))
    }

    fn with_category(mut self, category: &str) -> Rec {
        if self.source_label().is_none() {
            self.record_category = Some(category.to_string());
        }
        self.clone()
    }
}

fn normalized_source_label(raw: Option<&str>) -> Option<&'static str> {
    match raw?.trim().to_lowercase().as_str() {
        "classic" | "legacy" => Some("Classic"),
        "nested" | "nested share folder" | "nested share subfolder" | "nested share subfolders" => Some("Nested"),
        // A closed vocabulary: an unrecognized category is not a vault-type indicator.
        _ => None,
    }
}

/// `login @ host` listing descriptions lose the connection info.
fn login_from_description(raw: &str) -> String {
    let t = raw.trim();
    let Some((head, tail)) = t.split_once(" @ ") else { return t.to_string() };
    let (head, tail) = (head.trim(), tail.trim());
    if tail.is_empty() {
        return t.to_string();
    }
    if suffix_is_connection_info(tail) {
        return if head.is_empty() { t.to_string() } else { head.to_string() };
    }
    t.to_string()
}

fn suffix_is_connection_info(tail: &str) -> bool {
    let lower = tail.to_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        return true;
    }
    let (host, port) = match tail.split_once(':') {
        Some((h, p)) => (h, Some(p)),
        None => (tail, None),
    };
    let port_ok = |p: &str| !p.is_empty() && p.len() <= 5 && p.bytes().all(|b| b.is_ascii_digit());
    let ipv4 = {
        let octets: Vec<&str> = host.split('.').collect();
        octets.len() == 4 && octets.iter().all(|o| !o.is_empty() && o.len() <= 3 && o.bytes().all(|b| b.is_ascii_digit()))
    };
    if ipv4 && port.is_none_or(port_ok) {
        return true;
    }
    // host:port, the host starting alphanumeric.
    host.chars().next().is_some_and(|c| c.is_ascii_alphanumeric()) && host.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-') && port.is_some_and(port_ok)
}

/// One `ls` line of Commander's table (`parseLsRecordLine`, `KCC:393-415`): the row number, the
/// record UID, the type, the title, then the description.
fn parse_ls_record_line(line: &str) -> Option<Rec> {
    let t = line.trim();
    if t.starts_with('#') || t.starts_with("---") {
        return None;
    }
    let parts: Vec<&str> = t.split("  ").map(str::trim).filter(|p| !p.is_empty()).collect();
    if parts.len() < 4 || parts[0].parse::<i64>().is_err() {
        return None;
    }
    let uid = parts[1];
    if uid.chars().count() < 15 || !uid.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
        return None;
    }
    let description = if parts.len() > 4 { parts[4..].join(" ") } else { String::new() };
    Some(Rec { record_uid: Some(uid.to_string()), typ: Some(parts[2].to_string()), title: Some(if parts[3].is_empty() { "Untitled".into() } else { parts[3].to_string() }), description: Some(description), ..Rec::default() })
}

/// The records of a listing answer, in any of the shapes Commander and its wrapper use.
fn parse_listing_payload(data: &[u8]) -> Vec<Rec> {
    let Ok(root) = serde_json::from_slice::<Value>(data) else { return Vec::new() };
    let mut payloads = vec![root.clone()];
    if root.get("status").and_then(Value::as_str) == Some("success") {
        let inner = match root.get("result") {
            Some(Value::String(s)) if !s.is_empty() => serde_json::from_str::<Value>(s).ok(),
            Some(v @ Value::Object(_)) => Some(v.clone()),
            _ => None,
        };
        if let Some(i) = inner {
            payloads.insert(0, i);
        }
    }
    for p in &payloads {
        if p.get("command").and_then(Value::as_str) == Some("ls") {
            if let Some(rows) = p.pointer("/data/records").and_then(Value::as_array).filter(|r| !r.is_empty()) {
                let recs: Vec<Rec> = rows.iter().filter_map(|r| r.get("title").and_then(Value::as_str)).filter_map(parse_ls_record_line).collect();
                if !recs.is_empty() {
                    return recs;
                }
            }
        }
        if p.get("status").and_then(Value::as_str) == Some("success") {
            let rows = p.pointer("/data/records").and_then(Value::as_array).or_else(|| p.get("data").and_then(Value::as_array));
            if let Some(rows) = rows.filter(|r| !r.is_empty()) {
                return rows.iter().filter(|r| r.is_object()).map(Rec::from_value).collect();
            }
        }
    }
    Vec::new()
}

// --- get answers ---

fn trimmed(s: &str) -> Option<String> {
    let t = s.trim();
    (!t.is_empty()).then(|| t.to_string())
}

/// A field of a `get` record: `key` directly, or the `fields` entry of that type.
fn field_of(rec: &Value, keys: &[&str], kind: &str) -> Option<String> {
    for k in keys {
        if let Some(s) = rec.get(*k).and_then(Value::as_str) {
            return trimmed(s);
        }
    }
    for f in rec.get("fields").and_then(Value::as_array)? {
        if f.get("type").and_then(Value::as_str) != Some(kind) {
            continue;
        }
        match f.get("value") {
            Some(Value::Array(a)) => {
                if let Some(first) = a.first().and_then(Value::as_str) {
                    return trimmed(first);
                }
            }
            Some(Value::String(s)) => return trimmed(s),
            _ => {}
        }
    }
    None
}

fn from_get_json(data: &[u8], keys: &[&str], kind: &str) -> Option<String> {
    let parsed: Value = serde_json::from_slice(data).ok()?;
    let parsed = parsed.as_object()?;
    if let Some(d) = parsed.get("data") {
        match d {
            Value::String(s) => {
                if let Some(v) = serde_json::from_str::<Value>(s).ok().filter(Value::is_object).and_then(|r| field_of(&r, keys, kind)) {
                    return Some(v);
                }
            }
            Value::Object(_) => {
                if let Some(v) = field_of(d, keys, kind) {
                    return Some(v);
                }
            }
            _ => {}
        }
    }
    parsed.get("record").filter(|r| r.is_object()).and_then(|r| field_of(r, keys, kind))
}

/// `passwordFromGetJSONResponse` (`KCC:417-437`).
fn password_from_get_json(data: &[u8]) -> Option<String> {
    from_get_json(data, &["password"], "password")
}

/// `loginFromGetJSONResponse` (`KCC:439-460`); kept for the Edit panel's login prefill.
#[cfg_attr(not(test), allow(dead_code))]
fn login_from_get_json(data: &[u8]) -> Option<String> {
    from_get_json(data, &["login", "username"], "login")
}

/// A `get <uid> --format=password` answer (`extractPasswordFromRawGet`): an error or status
/// envelope is never the password.
fn password_from_raw_get(data: &[u8]) -> Option<String> {
    if let Ok(v) = serde_json::from_slice::<Value>(data) {
        match &v {
            Value::String(s) => {
                if let Some(t) = trimmed(s) {
                    return Some(t);
                }
            }
            Value::Object(o) => {
                let status = o.get("status").and_then(Value::as_str).map(str::to_lowercase);
                let done = matches!(status.as_deref(), Some("success" | "completed"));
                if done {
                    if let Some(t) = o.get("result").and_then(Value::as_str).and_then(trimmed) {
                        return Some(t);
                    }
                }
                if o.contains_key("error") || status.is_some() && !done {
                    return None;
                }
                if let Some(t) = o.get("result").and_then(Value::as_str).and_then(trimmed) {
                    return Some(t);
                }
                if done {
                    if let Some(t) = o.get("message").and_then(Value::as_array).and_then(|m| m.first()).and_then(Value::as_str).and_then(trimmed) {
                        let lower = t.to_lowercase();
                        if !lower.starts_with("command executed successfully") && !lower.contains("no output") {
                            return Some(t);
                        }
                    }
                }
                return None;
            }
            Value::Array(_) => return None,
            _ => {}
        }
    }
    // A bare body that is not a JSON object or array is the secret itself.
    let t = trimmed(std::str::from_utf8(data).ok()?)?;
    let json_like = matches!(serde_json::from_str::<Value>(&t), Ok(Value::Object(_) | Value::Array(_)));
    (!json_like).then_some(t)
}

// --- identifiers and command text ---

#[derive(Clone, Copy, PartialEq)]
enum Source {
    Classic,
    Nested,
}

fn source_from_label(label: Option<&str>) -> Option<Source> {
    match label?.to_lowercase().as_str() {
        "classic" => Some(Source::Classic),
        "nested" => Some(Source::Nested),
        _ => None,
    }
}

/// Bare UIDs and the legacy `classic:` / `nested:` prefixes.
fn parse_account_identifier(raw: &str) -> (Option<Source>, String) {
    let t = raw.trim();
    if let Some((prefix, rest)) = t.split_once(':') {
        if let Some(s) = source_from_label(Some(prefix)).filter(|_| !rest.is_empty()) {
            return (Some(s), rest.to_string());
        }
    }
    (None, t.to_string())
}

fn valid_uid_chars(uid: &str) -> bool {
    uid.len() >= 15 && uid.len() <= 64 && uid.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// A record UID goes into a command, so it must be 15 or more of `[A-Za-z0-9_-]`.
fn validated_uid(raw: &str) -> Result<String, Failure> {
    let uid = raw.trim();
    if valid_uid_chars(uid) { Ok(uid.to_string()) } else { Err(fail("Invalid record identifier.")) }
}

/// A UID inside free text (a record-add answer): the first run of exactly 22 UID characters.
fn extract_record_uid(text: &str) -> Option<String> {
    let t = text.trim();
    if let Ok(u) = validated_uid(t) {
        return Some(u);
    }
    let is = |c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-';
    let mut run = String::new();
    for c in t.chars().chain(std::iter::once(' ')) {
        if is(c) {
            run.push(c);
        } else {
            if run.len() == 22 {
                return Some(run);
            }
            run.clear();
        }
    }
    None
}

/// A value inside a double-quoted field of a command (`escapeForKeeperDoubleQuotedCommandField`);
/// control characters become spaces so one value can never become a second command.
fn escape_field(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '\\' | '"' | '$' | '`' | '!' => {
                out.push('\\');
                out.push(c);
            }
            c if c.is_control() => out.push(' '),
            c => out.push(c),
        }
    }
    out
}

fn human_readable_error(data: &[u8]) -> Option<String> {
    let v: Value = serde_json::from_slice(data).ok()?;
    ["error", "message"].iter().find_map(|k| v.get(*k).and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string))
}

fn user_facing_password_update_error(detail: &str) -> String {
    let lower = detail.to_lowercase();
    if lower.contains("base64") || lower.contains("pwd") || (lower.contains("password") && ["failed", "invalid", "required", "empty"].iter().any(|w| lower.contains(w))) {
        return "Password field is required.".to_string();
    }
    detail.to_string()
}

/// True when Nested Shared Folder commands are missing or not enabled.
fn is_nsf_unavailable(text: &str) -> bool {
    let s = text.to_lowercase();
    (s.contains("nested shared folder") && (s.contains("not available") || s.contains("unavailable") || s.contains("not enabled")))
        || s.contains("invalid json response")
        || s.contains("unknown command")
        || s.contains("unrecognized command")
        || s.contains("invalid command")
        || (s.contains("not allowed") && (s.contains("nsf") || s.contains("command")))
        || s.contains("available commands")
        || s.contains("type help <command>")
}

/// `keeperCommandStatusIsSuccess`: the status, or an inner payload's own status when the answer
/// is the `{status, result}` envelope.
fn status_is_success(data: &[u8]) -> bool {
    let Ok(Value::Object(o)) = serde_json::from_slice::<Value>(data) else { return false };
    let Some(status) = o.get("status").and_then(Value::as_str) else { return false };
    if !status.eq_ignore_ascii_case("success") {
        return false;
    }
    let inner = match o.get("result") {
        Some(v @ Value::Object(_)) => Some(v.clone()),
        Some(Value::String(s)) => serde_json::from_str::<Value>(s).ok().filter(Value::is_object),
        _ => None,
    };
    match inner.as_ref().and_then(|i| i.get("status")).and_then(Value::as_str) {
        Some(inner) => inner.eq_ignore_ascii_case("success"),
        None => true,
    }
}

// --- The service client ---

#[derive(Serialize)]
struct Exec<'a> {
    command: &'a str,
}

struct Cmdr<'a> {
    http: &'a mut dyn Http,
    base: String,
    key: Secret,
    /// Secrets an error text must not show (the key, and a password being set).
    hide: Vec<Secret>,
}

fn valid_request_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= MAX_ID && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

impl<'a> Cmdr<'a> {
    fn new(http: &'a mut dyn Http, header: &RequestHeader, key: Secret) -> Result<Cmdr<'a>, Failure> {
        let base = v2_base(&service_url(header)?);
        let hide = vec![key.duplicate()];
        Ok(Cmdr { http, base, key, hide })
    }

    /// Server text as it may be shown: no hidden characters, capped, no secret.
    fn clean(&self, text: &str) -> String {
        // Secrets are replaced on the whole text first; a cut before it could keep a prefix.
        // One to three character secrets are not replaced (every word would turn into the mask
        // and show the pattern).
        let mut s = text.to_string();
        for h in &self.hide {
            if h.expose().chars().count() >= 4 {
                s = s.replace(h.expose(), "<hidden>");
            }
        }
        display_text(&s, MAX_TEXT, false)
    }

    fn headers(&self, json: bool) -> Vec<(String, String)> {
        let mut h = Vec::new();
        if json {
            h.push(("Content-Type".to_string(), "application/json".to_string()));
        }
        h.push(("api-key".to_string(), self.key.expose().to_string()));
        h
    }

    fn exchange(&mut self, method: &str, url: &str, body: &[u8], timeout: Duration, json: bool) -> Result<(u16, Vec<u8>), Failure> {
        let headers = self.headers(json);
        let result = self.http.send(method, url, &headers, body, timeout);
        // The header copies of the key are wiped as they are dropped.
        for (_, v) in headers {
            drop(Secret::new(v));
        }
        match result {
            Ok((code, data)) if data.len() > MAX_CLI_JSON => {
                let _ = code;
                Err(fail("Keeper Commander's answer was too large."))
            }
            Ok(r) => Ok(r),
            Err(text) => Err(Failure(format!("Could not reach Keeper Commander ({}). {ADVICE}", self.clean(&text)))),
        }
    }

    /// `keeperConnectivityErrorMessage(statusCode:data:)`.
    fn http_error(&self, code: u16, data: &[u8]) -> Failure {
        if let Some(detail) = human_readable_error(data) {
            return Failure(self.clean(&detail));
        }
        let head = String::from_utf8_lossy(&data[..data.len().min(2048)]).to_lowercase();
        if !data.is_empty() && (head.contains("<!doctype html") || head.contains("<html") || head.contains("<body")) {
            return Failure(format!("The Service URL returned an HTML error page (the server may be offline, the URL may be wrong, or a proxy/tunnel may be misconfigured). {ADVICE}"));
        }
        Failure(match code {
            401 | 403 => format!("API key rejected (HTTP {code}). {ADVICE}"),
            c if c >= 500 => format!("Keeper Commander returned an error (HTTP {c}). {ADVICE}"),
            c => format!("Unexpected response from Keeper Commander (HTTP {c}). {ADVICE}"),
        })
    }

    /// Runs one Commander command and returns its result body.
    fn post_command(&mut self, command: &Secret, timeout: Duration) -> Result<Vec<u8>, Failure> {
        let mut body = serde_json::to_vec(&Exec { command: command.expose() }).map_err(|_| fail("Failed to encode the command."))?;
        let url = format!("{}/executecommand-async", self.base);
        let sent = self.exchange("POST", &url, &body, timeout, true);
        zero_bytes(&mut body);
        let (code, data) = sent?;
        if code != 202 {
            return Err(self.http_error(code, &data));
        }
        let queued: Value = serde_json::from_slice(&data).map_err(|_| self.http_error(code, &data))?;
        let id = queued.get("request_id").and_then(Value::as_str).filter(|s| !s.is_empty()).ok_or_else(|| fail("No request_id in v2 response"))?;
        if !valid_request_id(id) {
            return Err(fail("Keeper Commander returned an invalid request_id."));
        }
        let id = id.to_string();
        self.poll(&id, timeout)
    }

    fn poll(&mut self, id: &str, total: Duration) -> Result<Vec<u8>, Failure> {
        let start = Instant::now();
        let mut waited = Duration::ZERO;
        let mut interval = INITIAL_INTERVAL;
        let max_polls = (total.as_millis() / INITIAL_INTERVAL.as_millis()) as u32 + 30;
        let (mut polls, mut unparseable, mut saw_non_queued) = (0u32, 0u32, false);
        let status_url = format!("{}/status/{id}", self.base);
        loop {
            polls += 1;
            let elapsed = start.elapsed().max(waited);
            if elapsed > total {
                return Err(fail(if saw_non_queued { "Keeper service v2 request timed out" } else { WEDGED }));
            }
            if polls > max_polls {
                return Err(fail("Keeper service did not complete the request in time"));
            }
            let (code, data) = self.exchange("GET", &status_url, &[], STATUS_POLL, false)?;
            if code != 200 {
                return Err(self.http_error(code, &data));
            }
            let status = serde_json::from_slice::<Value>(&data).ok().and_then(|v| v.get("status").and_then(Value::as_str).map(str::to_string));
            let Some(status) = status else {
                unparseable += 1;
                if unparseable >= MAX_UNPARSEABLE {
                    return Err(self.http_error(code, &data));
                }
                self.http.pause(interval);
                waited += interval;
                interval = (interval * 2).min(MAX_INTERVAL);
                continue;
            };
            unparseable = 0;
            match status.as_str() {
                "completed" => {
                    let (rcode, rdata) = self.exchange("GET", &format!("{}/result/{id}", self.base), &[], total, false)?;
                    if rdata.is_empty() {
                        return Err(fail("No data"));
                    }
                    if rcode != 200 {
                        return Err(self.http_error(rcode, &rdata));
                    }
                    return Ok(rdata);
                }
                "failed" | "expired" => return Err(Failure(format!("Keeper command {status}"))),
                other => {
                    if other.eq_ignore_ascii_case("queued") {
                        if !saw_non_queued && elapsed > QUEUED_STUCK {
                            return Err(fail(WEDGED));
                        }
                    } else {
                        saw_non_queued = true;
                    }
                    self.http.pause(interval);
                    waited += interval;
                    interval = (interval * 2).min(MAX_INTERVAL);
                }
            }
        }
    }

    fn run(&mut self, command: &str, timeout: Duration) -> Result<Vec<u8>, Failure> {
        self.post_command(&Secret::new(command.to_string()), timeout)
    }

    /// Tries each attempt until one answers `status == success`; a transport error from an
    /// earlier attempt is preferred over a later body failure (`runKeeperMutationAttempts`).
    fn mutate(&mut self, attempts: &[Secret], format_failure: fn(&str) -> String) -> Result<(), Failure> {
        let mut first_response: Option<Vec<u8>> = None;
        let mut first_error: Option<Failure> = None;
        for command in attempts {
            match self.post_command(command, LONG_REQUEST) {
                Ok(data) if status_is_success(&data) => return Ok(()),
                Ok(data) => {
                    first_response.get_or_insert(data);
                }
                Err(e) => {
                    first_error.get_or_insert(e);
                }
            }
        }
        if let Some(e) = first_error {
            return Err(e);
        }
        let detail = first_response.as_deref().and_then(human_readable_error).map(|d| self.clean(&d));
        Err(Failure(format_failure(detail.as_deref().unwrap_or(""))))
    }
}

/// A password as the `password=$BASE64:<b64>` command fragment, and the base64 to hide.
fn password_fragment(password: &str) -> (Secret, Secret) {
    let b64 = Secret::new(base64::engine::general_purpose::STANDARD.encode(password.as_bytes()));
    let mut fragment = Secret::new(String::with_capacity(b64.expose().len() + 16));
    fragment.push_str("password=$BASE64:");
    fragment.push_str(b64.expose());
    (fragment, b64)
}

// --- Subcommands ---

fn key_of(token: Option<&str>, master: Option<Secret>) -> Result<Secret, Failure> {
    for s in [master, decode_token(token)].into_iter().flatten() {
        let t = s.expose().trim();
        if !t.is_empty() {
            return Ok(Secret::new(t.to_string()));
        }
    }
    Err(fail("Invalid or missing API key"))
}

fn handshake(stdin: &[u8]) -> Result<Vec<u8>, Failure> {
    let request: HandshakeRequest = decode_request(stdin, "handshake").map_err(bad_request)?;
    if request.max_protocol_version < PROTOCOL {
        return Err(fail("Protocol version 0 is required"));
    }
    Ok(ok_json(&HandshakeResponse {
        protocol_version: PROTOCOL,
        name: "Keeper Security".to_string(),
        requires_master_password: true,
        can_set_passwords: true,
        user_accounts: None,
        needs_path_to_database: true,
        database_extension: None,
        needs_path_to_executable: None,
        path_to_database_kind: Some(PathKind::Url),
        path_to_database_prompt: Some("Keeper Commander Service Mode API URL".to_string()),
        path_to_database_placeholder: Some("http://127.0.0.1:8900/api/v2".to_string()),
        master_password_label: Some("API key".to_string()),
        persists_credentials: Some(true),
        custom_commands: Some(vec![CustomCommand { name: "sync-down".to_string(), label: "Sync Down".to_string(), icon: Some("arrow.clockwise".to_string()) }]),
        settings_fields: Some(vec![Field {
            key: "serviceURL".to_string(),
            label: "API URL:".to_string(),
            placeholder: Some("http://127.0.0.1:8900/api/v2".to_string()),
            is_secret: false,
            note: Some("Note: Append /api/v2 in your API URL".to_string()),
            persist_in_keychain: false,
        }]),
        add_account_toggles: Some(vec![Toggle {
            key: "useClassicPermission".to_string(),
            label: "Use classic permission model".to_string(),
            note: Some("Limits sharing to basic access levels. Recommended only for compatibility with older workflows.".to_string()),
            default_value: false,
        }]),
        legacy_credential_migrations: Some(vec![LegacyCredentialMigration { from_keychain_account: "apiKey".to_string() }]),
        can_edit_in_place: Some(true),
        requires_password_for_add: None,
    }))
}

fn login(stdin: &[u8], http: &mut dyn Http) -> Result<Vec<u8>, Failure> {
    let mut request: LoginRequest = decode_request(stdin, "login").map_err(bad_request)?;
    let master = request.master_password.take().map(Secret::new);
    service_url(&request.header)?;
    let key = key_of(None, master)?;
    let mut c = Cmdr::new(http, &request.header, key.duplicate())?;
    c.run("list --format=json", VALIDATION_REQUEST)?;
    Ok(ok_json(&LoginResponse { token: Some(encode_token(&key)) }))
}

/// A listing fetched from Commander: the classic vault, then the nested one.
fn list_accounts(stdin: &[u8], http: &mut dyn Http) -> Result<Vec<u8>, Failure> {
    let request: ListAccountsRequest = decode_request(stdin, "list-accounts").map_err(bad_request)?;
    service_url(&request.header)?;
    let key = key_of(request.token.as_deref(), None)?;
    let mut c = Cmdr::new(http, &request.header, key)?;
    let classic: Vec<Rec> = parse_listing_payload(&c.run("list --format=json", LONG_REQUEST)?).into_iter().map(|r| r.with_category("Classic")).collect();
    let mut warning = None;
    let nested: Vec<Rec> = match c.run("nsf-list --records --format=json", LONG_REQUEST) {
        Ok(data) => {
            let peek: Option<Value> = serde_json::from_slice(&data).ok();
            let bad = peek.as_ref().and_then(|p| p.get("status")).and_then(Value::as_str).map(str::to_lowercase).filter(|s| s != "success" && s != "completed");
            if bad.is_some() {
                let msg = peek.as_ref().and_then(|p| p.get("error")).and_then(Value::as_str).map(str::to_string).or_else(|| human_readable_error(&data)).unwrap_or_else(|| "nsf-list failed".to_string());
                let msg = c.clean(&msg);
                if !is_nsf_unavailable(&msg) {
                    warning = Some(nested_warning(&msg));
                }
                Vec::new()
            } else {
                parse_listing_payload(&data).into_iter().map(|r| r.with_category("Nested")).collect()
            }
        }
        Err(e) => {
            if !is_nsf_unavailable(&e.0) {
                warning = Some(nested_warning(&e.0));
            }
            Vec::new()
        }
    };
    // Classic wins when a UID appears in both lists.
    let mut seen = std::collections::HashSet::new();
    let mut accounts = Vec::new();
    for rec in classic.into_iter().chain(nested) {
        let Some(uid) = rec.effective_uid().filter(|u| valid_uid_chars(u)) else { continue };
        if !seen.insert(uid.clone()) || rec.is_folder() {
            continue;
        }
        let name = display_text(&rec.display_title(), MAX_NAME, false);
        accounts.push(Account {
            identifier: AccountIdentifier { account_id: uid },
            user_name: display_text(&rec.list_user_name(), MAX_NAME, false),
            account_name: if name.trim().is_empty() { "Untitled".to_string() } else { name },
            has_otp: false,
            source_label: rec.source_label().map(str::to_string),
        });
    }
    if accounts.len() > MAX_ACCOUNTS {
        accounts.truncate(MAX_ACCOUNTS);
        warning.get_or_insert_with(|| format!("The vault has more than {MAX_ACCOUNTS} records; only the first {MAX_ACCOUNTS} are shown."));
    }
    Ok(ok_json(&ListAccountsResponse { accounts, warning }))
}

fn nested_warning(msg: &str) -> String {
    format!("Nested Shared Folder listing failed ({msg}). Showing classic vault accounts only; Nested accounts may be missing until you retry or run Sync Down.")
}

fn get_password(stdin: &[u8], http: &mut dyn Http) -> Result<Vec<u8>, Failure> {
    let request: GetPasswordRequest = decode_request(stdin, "get-password").map_err(bad_request)?;
    service_url(&request.header)?;
    let key = key_of(request.token.as_deref(), None)?;
    let uid = validated_uid(&parse_account_identifier(&request.account_identifier.account_id).1)?;
    let mut c = Cmdr::new(http, &request.header, key)?;
    let json = c.run(&format!("get {uid} --format=json"), INTERACTIVE_READ)?;
    if let Some(p) = password_from_get_json(&json) {
        return Ok(ok_json(&Password { password: p, otp: None }));
    }
    let raw = c.run(&format!("get {uid} --format=password"), INTERACTIVE_READ)?;
    let p = password_from_raw_get(&raw).ok_or_else(|| fail("Keeper returned no password for this record."))?;
    Ok(ok_json(&Password { password: p, otp: None }))
}

fn set_password(stdin: &[u8], http: &mut dyn Http) -> Result<Vec<u8>, Failure> {
    let mut request: SetPasswordRequest = decode_request(stdin, "set-password").map_err(bad_request)?;
    service_url(&request.header)?;
    let key = key_of(request.token.as_deref(), None)?;
    let new_password = request.new_password.take().map(Secret::new).filter(|p| !p.is_empty());
    let mut fields: Vec<String> = Vec::new();
    // `--title="..."` binds a title that starts with "-" as the value.
    if let Some(t) = &request.new_account_name {
        fields.push(format!("--title=\"{}\"", escape_field(t)));
    }
    if let Some(u) = &request.new_user_name {
        fields.push(format!("login=\"{}\"", escape_field(u)));
    }
    if fields.is_empty() && new_password.is_none() {
        return Err(fail("Nothing to update."));
    }
    let (parsed_source, raw_uid) = parse_account_identifier(&request.account_identifier.account_id);
    let uid = validated_uid(&raw_uid)?;
    let source = source_from_label(request.source_label.as_deref()).or(parsed_source);
    let verb = match source {
        Some(Source::Classic) => "record-update",
        Some(Source::Nested) => "nsf-record-update",
        None => return Err(fail("Cannot update record: vault is unknown. Reload accounts and try again.")),
    };
    let mut c = Cmdr::new(http, &request.header, key)?;
    let mut command = Secret::new(String::with_capacity(96 + fields.iter().map(|f| f.len() + 1).sum::<usize>() + new_password.as_ref().map_or(0, |p| p.expose().len() * 2)));
    command.push_str(&format!("{verb} --force -r {uid}"));
    for f in &fields {
        command.push_str(" ");
        command.push_str(f);
    }
    if let Some(p) = &new_password {
        let (fragment, b64) = password_fragment(p.expose());
        command.push_str(" ");
        command.push_str(fragment.expose());
        c.hide.push(b64);
        c.hide.push(p.duplicate());
    }
    c.mutate(&[command], |d| user_facing_password_update_error(if d.is_empty() { "Update failed" } else { d }))?;
    Ok(ok_json(&SetPasswordResponse {}))
}

fn delete_account(stdin: &[u8], http: &mut dyn Http) -> Result<Vec<u8>, Failure> {
    let request: DeleteAccountRequest = decode_request(stdin, "delete-account").map_err(bad_request)?;
    service_url(&request.header)?;
    let key = key_of(request.token.as_deref(), None)?;
    let (parsed_source, raw_uid) = parse_account_identifier(&request.account_identifier.account_id);
    let uid = validated_uid(&raw_uid)?;
    let source = source_from_label(request.source_label.as_deref()).or(parsed_source);
    let classic = Secret::new(format!("rm -f {uid}"));
    let nested = Secret::new(format!("nsf-rm {uid} -f"));
    // Classic rm errors on a nested UID, so an unknown vault tries both.
    let attempts = match source {
        Some(Source::Classic) => vec![classic],
        Some(Source::Nested) => vec![nested],
        None => vec![classic, nested],
    };
    let mut c = Cmdr::new(http, &request.header, key)?;
    c.mutate(&attempts, |d| if d.is_empty() { "Delete failed".to_string() } else { d.to_string() })?;
    Ok(ok_json(&DeleteAccountResponse {}))
}

fn add_once(c: &mut Cmdr, verb: &str, user: &str, name: &str, password: Option<&Secret>) -> Result<String, Failure> {
    let mut command = Secret::new(String::with_capacity(128 + name.len() + user.len() + password.map_or(0, |p| p.expose().len() * 2)));
    command.push_str(&format!("{verb} --force --record-type=login --title=\"{}\"", escape_field(name)));
    if !user.is_empty() {
        command.push_str(&format!(" login=\"{}\"", escape_field(user)));
    }
    if let Some(p) = password.filter(|p| !p.is_empty()) {
        let (fragment, b64) = password_fragment(p.expose());
        command.push_str(" ");
        command.push_str(fragment.expose());
        c.hide.push(b64);
        c.hide.push(p.duplicate());
    }
    let data = c.post_command(&command, LONG_REQUEST)?;
    let v: Value = serde_json::from_slice(&data).map_err(|_| fail("Add failed"))?;
    if v.get("status").and_then(Value::as_str) != Some("success") {
        return Err(Failure(human_readable_error(&data).map(|d| c.clean(&d)).unwrap_or_else(|| "Add failed".to_string())));
    }
    // `data` is an object, or a JSON string holding one, or text holding a UID.
    let from_data = match v.get("data") {
        Some(d @ Value::Object(_)) => ["record_uid", "uid"].iter().find_map(|k| d.get(*k).and_then(Value::as_str)).and_then(trimmed),
        Some(Value::String(s)) => match serde_json::from_str::<Value>(s) {
            Ok(inner @ Value::Object(_)) => ["record_uid", "uid"].iter().find_map(|k| inner.get(*k).and_then(Value::as_str)).and_then(trimmed),
            _ => extract_record_uid(s),
        },
        _ => None,
    };
    let message = match v.get("message") {
        Some(Value::String(s)) => Some(s.clone()),
        Some(Value::Array(a)) => Some(a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(" ")),
        _ => None,
    };
    if let Some(u) = from_data {
        return validated_uid(&u);
    }
    message.as_deref().and_then(extract_record_uid).ok_or_else(|| fail("Add failed"))
}

fn add_account(stdin: &[u8], http: &mut dyn Http) -> Result<Vec<u8>, Failure> {
    let mut request: AddAccountRequest = decode_request(stdin, "add-account").map_err(bad_request)?;
    service_url(&request.header)?;
    let key = key_of(request.token.as_deref(), None)?;
    let password = request.password.take().map(Secret::new);
    let classic = request.flags.as_ref().and_then(|f| f.get("useClassicPermission")).copied().unwrap_or(false);
    let mut c = Cmdr::new(http, &request.header, key)?;
    let (user, name) = (request.user_name.as_str(), request.account_name.as_str());
    let uid = if classic {
        add_once(&mut c, "record-add", user, name, password.as_ref())?
    } else {
        // Nested only; classic solely when Nested Shared Folders are unavailable.
        match add_once(&mut c, "nsf-record-add", user, name, password.as_ref()) {
            Ok(u) => u,
            Err(e) if is_nsf_unavailable(&e.0) => add_once(&mut c, "record-add", user, name, password.as_ref())?,
            Err(e) => return Err(e),
        }
    };
    Ok(ok_json(&AddAccountResponse { account_identifier: AccountIdentifier { account_id: uid } }))
}

fn custom_command(name: &str, stdin: &[u8], http: &mut dyn Http) -> Result<Vec<u8>, Failure> {
    let request: CustomCommandRequest = decode_request(stdin, "custom command").map_err(bad_request)?;
    service_url(&request.header)?;
    let key = key_of(request.token.as_deref(), None)?;
    match name {
        "sync-down" => {
            Cmdr::new(http, &request.header, key)?.run("sync-down", LONG_REQUEST)?;
            Ok(ok_json(&CustomCommandResponse { message: Some("Sync completed".to_string()) }))
        }
        other => Err(Failure(format!("Unknown command: {}", display_text(other, 64, false)))),
    }
}

/// One adapter call: the JSON to print (an `ErrorResponse` for any failure).
pub fn handle(command: &str, stdin: &[u8], http: &mut dyn Http) -> Vec<u8> {
    let result = match command {
        "handshake" => handshake(stdin),
        "login" => login(stdin, http),
        "list-accounts" => list_accounts(stdin, http),
        "get-password" => get_password(stdin, http),
        "set-password" => set_password(stdin, http),
        "add-account" => add_account(stdin, http),
        "delete-account" => delete_account(stdin, http),
        other => custom_command(other, stdin, http),
    };
    result.unwrap_or_else(|f| error_json(&f.0))
}

// --- The real client ---

/// The status in `ai_transport`'s non-200 error text ("HTTP request failed with status N.").
pub fn status_from_error(error: &str) -> Option<u16> {
    error.strip_prefix("HTTP request failed with status ")?.strip_suffix('.')?.parse().ok()
}

/// 12b's WinHTTP transport driven to its end. No proxy: the API key goes only where the
/// person's URL says. Redirects off the host are refused by the transport.
#[cfg(windows)]
pub struct WinHttpBlocking;

#[cfg(windows)]
impl WinHttpBlocking {
    pub fn new() -> WinHttpBlocking {
        WinHttpBlocking
    }
}

#[cfg(windows)]
impl Default for WinHttpBlocking {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(windows)]
impl Http for WinHttpBlocking {
    fn send(&mut self, method: &str, url: &str, headers: &[(String, String)], body: &[u8], timeout: Duration) -> Result<(u16, Vec<u8>), String> {
        use crate::ai_transport::{Bounds, HttpEvent, HttpRequest, Options, Proxy, Transport, MAX_BODY};
        let method: &'static str = match method {
            "GET" => "GET",
            "POST" => "POST",
            _ => return Err("Unsupported method.".to_string()),
        };
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        let tx = std::sync::Mutex::new(tx);
        let notify: crate::ai_transport::Notify = std::sync::Arc::new(move || {
            if let Ok(t) = tx.lock() {
                let _ = t.send(());
            }
        });
        let request = HttpRequest { method, url: url.to_string(), headers: headers.to_vec(), body: body.to_vec(), stream: false };
        let options = Options { idle: timeout, proxy: Proxy::Off };
        let bounds = Bounds { limit: MAX_CLI_JSON.min(MAX_BODY), total: Some(timeout) };
        let mut call = crate::ai_winhttp::WinHttp.start_bounded(request, options, bounds, notify);
        loop {
            while let Some(event) = call.try_event() {
                if let HttpEvent::Done { body, error } = event {
                    return match error {
                        None => Ok((200, body.into_bytes())),
                        Some(e) => match status_from_error(&e) {
                            Some(code) => Ok((code, body.into_bytes())),
                            None => Err(e),
                        },
                    };
                }
            }
            if rx.recv_timeout(timeout + Duration::from_secs(5)).is_err() {
                call.cancel();
                return Err("The request timed out.".to_string());
            }
        }
    }

    fn pause(&mut self, d: Duration) {
        std::thread::sleep(d);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    const UID: &str = "AbCdEfGhIjKlMnOpQrStUv";
    const UID2: &str = "NnNnNnNnNnNnNnNnNnNnNn";
    const KEY: &str = "key-SECRET-123";

    #[derive(Clone, Debug)]
    struct Sent {
        method: String,
        url: String,
        headers: Vec<(String, String)>,
        body: String,
        timeout: Duration,
    }

    /// A Commander Service Mode fake: a POST queues a request and remembers the reply whose
    /// command prefix matches; status answers come from `statuses` ("completed" when empty).
    struct Fake {
        sent: Vec<Sent>,
        rules: Vec<(String, String)>,
        statuses: VecDeque<String>,
        post_status: u16,
        post_body: Option<String>,
        pending: Vec<(String, String)>,
        paused: Duration,
        transport_error: Option<String>,
    }

    impl Fake {
        fn new(rules: &[(&str, &str)]) -> Fake {
            Fake {
                sent: Vec::new(),
                rules: rules.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect(),
                statuses: VecDeque::new(),
                post_status: 202,
                post_body: None,
                pending: Vec::new(),
                paused: Duration::ZERO,
                transport_error: None,
            }
        }
        fn commands(&self) -> Vec<String> {
            self.sent
                .iter()
                .filter(|s| s.method == "POST")
                .map(|s| serde_json::from_str::<serde_json::Value>(&s.body).unwrap()["command"].as_str().unwrap().to_string())
                .collect()
        }
    }

    impl Http for Fake {
        fn send(&mut self, method: &str, url: &str, headers: &[(String, String)], body: &[u8], timeout: Duration) -> Result<(u16, Vec<u8>), String> {
            self.sent.push(Sent { method: method.into(), url: url.into(), headers: headers.to_vec(), body: String::from_utf8_lossy(body).into(), timeout });
            if let Some(e) = &self.transport_error {
                return Err(e.clone());
            }
            if method == "POST" {
                if self.post_status != 202 {
                    return Ok((self.post_status, self.post_body.clone().unwrap_or_default().into_bytes()));
                }
                let command = serde_json::from_slice::<serde_json::Value>(body).unwrap()["command"].as_str().unwrap().to_string();
                let reply = self.rules.iter().find(|(p, _)| command.starts_with(p.as_str())).map(|(_, r)| r.clone()).unwrap_or_else(|| "{\"status\":\"error\",\"error\":\"no rule\"}".into());
                let id = format!("req{}", self.pending.len() + 1);
                self.pending.push((id.clone(), reply));
                return Ok((202, format!("{{\"request_id\":\"{id}\",\"status\":\"queued\"}}").into_bytes()));
            }
            let id = url.rsplit('/').next().unwrap().to_string();
            if url.contains("/status/") {
                let s = self.statuses.pop_front().unwrap_or_else(|| "completed".into());
                return Ok((200, format!("{{\"status\":\"{s}\"}}").into_bytes()));
            }
            let reply = self.pending.iter().find(|(i, _)| *i == id).map(|(_, r)| r.clone()).unwrap_or_default();
            Ok((200, reply.into_bytes()))
        }
        fn pause(&mut self, d: Duration) {
            self.paused += d;
        }
    }

    fn header(url: &str) -> serde_json::Value {
        serde_json::json!({"mode": "terminal", "settings": {"serviceURL": url}})
    }

    fn token() -> String {
        encode_token(&Secret::new(KEY.to_string()))
    }

    fn call(command: &str, request: serde_json::Value, fake: &mut Fake) -> serde_json::Value {
        let out = handle(command, request.to_string().as_bytes(), fake);
        serde_json::from_slice(&out).unwrap()
    }

    fn req(url: &str, extra: serde_json::Value) -> serde_json::Value {
        let mut v = serde_json::json!({"header": header(url), "token": token()});
        for (k, val) in extra.as_object().unwrap() {
            v[k] = val.clone();
        }
        v
    }

    const OK: &str = "{\"status\":\"success\"}";

    fn ls_line(n: u32, uid: &str, title: &str, desc: &str) -> String {
        format!("{n}  {uid}  login  {title}  {desc}")
    }

    fn classic_listing() -> String {
        serde_json::json!({"status":"success","command":"ls","data":{"records":[
            {"title": "#  Record UID  Type  Title  Description"},
            {"title": ls_line(1, UID, "Mail", "me@example.com @ https://mail.example.com")}
        ]}}).to_string()
    }

    fn nested_listing() -> String {
        serde_json::json!({"status":"success","data":{"records":[
            {"uid": UID2, "title": "Nested One", "login": "nu", "record_category": "nested"},
            {"uid": UID, "title": "Dup", "login": "dup", "record_category": "nested"},
            {"uid": "FFFFFFFFFFFFFFFFFFFFFF", "title": "Dir", "type": "folder"}
        ]}}).to_string()
    }

    #[test]
    fn v2_base_follows_iterm2() {
        assert_eq!(v2_base("http://localhost:8080"), "http://localhost:8080/api/v2");
        assert_eq!(v2_base("http://localhost:8080/"), "http://localhost:8080/api/v2");
        assert_eq!(v2_base("http://localhost:8080/api/v2"), "http://localhost:8080/api/v2");
        assert_eq!(v2_base("http://localhost:8080/api/v2/"), "http://localhost:8080/api/v2");
        assert_eq!(v2_base("https://h.example/keeper"), "https://h.example/keeper/api/v2");
    }

    #[test]
    fn keeper_adapter_follows_iterm2() {
        // Handshake.
        let mut fake = Fake::new(&[]);
        let h = call("handshake", serde_json::json!({"iTermVersion": "3", "minProtocolVersion": 0, "maxProtocolVersion": 0}), &mut fake);
        assert_eq!(h["name"], "Keeper Security");
        assert_eq!(h["requiresMasterPassword"], true);
        assert_eq!(h["canSetPasswords"], true);
        assert_eq!(h["canEditInPlace"], true);
        assert_eq!(h["masterPasswordLabel"], "API key");
        assert_eq!(h["pathToDatabaseKind"], "url");
        assert_eq!(h["settingsFields"][0]["key"], "serviceURL");
        assert_eq!(h["addAccountToggles"][0]["key"], "useClassicPermission");
        assert_eq!(h["customCommands"][0]["name"], "sync-down");
        assert!(fake.sent.is_empty());

        // Login validates the key with list and returns it as the token; the key travels only in
        // the api-key header of POST and polls.
        let mut fake = Fake::new(&[("list --format=json", &classic_listing())]);
        let r = call("login", serde_json::json!({"header": header("http://localhost:8080"), "masterPassword": format!(" {KEY}\n")}), &mut fake);
        assert_eq!(decode_token(r["token"].as_str()).unwrap().expose(), KEY);
        assert_eq!(fake.commands(), ["list --format=json"]);
        assert_eq!(fake.sent[0].url, "http://localhost:8080/api/v2/executecommand-async");
        assert_eq!(fake.sent[1].url, "http://localhost:8080/api/v2/status/req1");
        assert_eq!(fake.sent[2].url, "http://localhost:8080/api/v2/result/req1");
        for s in &fake.sent {
            assert!(s.headers.iter().any(|(n, v)| n == "api-key" && v == KEY), "{s:?}");
            assert!(!s.url.contains(KEY) && !s.body.contains(KEY));
            assert!(s.headers.iter().filter(|(n, _)| n != "api-key").all(|(_, v)| !v.contains(KEY)));
        }
        assert_eq!(fake.sent[0].method, "POST");
        assert_eq!(fake.sent[1].method, "GET");

        // List: classic then nested, merged (classic wins), folders skipped, source labels.
        let mut fake = Fake::new(&[("list --format=json", &classic_listing()), ("nsf-list --records --format=json", &nested_listing())]);
        let r = call("list-accounts", req("http://localhost:8080/api/v2", serde_json::json!({})), &mut fake);
        assert_eq!(fake.commands(), ["list --format=json", "nsf-list --records --format=json"]);
        let a = r["accounts"].as_array().unwrap();
        assert_eq!(a.len(), 2, "{r}");
        assert_eq!(a[0]["identifier"]["accountID"], UID);
        assert_eq!(a[0]["accountName"], "Mail");
        assert_eq!(a[0]["userName"], "me@example.com");
        assert_eq!(a[0]["sourceLabel"], "Classic");
        assert_eq!(a[1]["identifier"]["accountID"], UID2);
        assert_eq!(a[1]["userName"], "nu");
        assert_eq!(a[1]["sourceLabel"], "Nested");
        assert_eq!(a[1]["hasOTP"], false);
        assert!(r.get("warning").is_none() || r["warning"].is_null());

        // Get password: the JSON answer first, then the raw one.
        let json = serde_json::json!({"status":"success","data":{"fields":[{"type":"password","value":["s3cret"]}]}}).to_string();
        let mut fake = Fake::new(&[(&format!("get {UID} --format=json"), &json)]);
        let r = call("get-password", req("http://localhost:8080", serde_json::json!({"accountIdentifier": {"accountID": UID}})), &mut fake);
        assert_eq!(r["password"], "s3cret");
        assert_eq!(fake.commands(), [format!("get {UID} --format=json")]);
        let mut fake = Fake::new(&[(&format!("get {UID} --format=json"), "{\"status\":\"success\"}"), (&format!("get {UID} --format=password"), "{\"status\":\"success\",\"result\":\"  raw-pw \"}")]);
        let r = call("get-password", req("http://localhost:8080", serde_json::json!({"accountIdentifier": {"accountID": UID}})), &mut fake);
        assert_eq!(r["password"], "raw-pw");
        assert_eq!(fake.commands(), [format!("get {UID} --format=json"), format!("get {UID} --format=password")]);

        // Delete: classic rm, nested nsf-rm, unknown tries both.
        let id = serde_json::json!({"accountID": UID});
        let mut fake = Fake::new(&[("rm -f", OK)]);
        call("delete-account", req("http://localhost:8080", serde_json::json!({"accountIdentifier": id, "sourceLabel": "Classic"})), &mut fake);
        assert_eq!(fake.commands(), [format!("rm -f {UID}")]);
        let mut fake = Fake::new(&[("nsf-rm", OK)]);
        call("delete-account", req("http://localhost:8080", serde_json::json!({"accountIdentifier": id, "sourceLabel": "Nested"})), &mut fake);
        assert_eq!(fake.commands(), [format!("nsf-rm {UID} -f")]);
        let mut fake = Fake::new(&[("rm -f", "{\"status\":\"error\",\"error\":\"nope\"}"), ("nsf-rm", OK)]);
        let r = call("delete-account", req("http://localhost:8080", serde_json::json!({"accountIdentifier": id})), &mut fake);
        assert!(r.get("error").is_none(), "{r}");
        assert_eq!(fake.commands(), [format!("rm -f {UID}"), format!("nsf-rm {UID} -f")]);

        // Set: record-update in place with the password as base64.
        let mut fake = Fake::new(&[("record-update", OK)]);
        let r = call("set-password", req("http://localhost:8080", serde_json::json!({"accountIdentifier": id, "sourceLabel": "Classic", "newPassword": "pa$$ w0rd"})), &mut fake);
        assert!(r.get("error").is_none(), "{r}");
        use base64::Engine;
        let b64 = base64::engine::general_purpose::STANDARD.encode("pa$$ w0rd");
        assert_eq!(fake.commands(), [format!("record-update --force -r {UID} password=$BASE64:{b64}")]);
        let mut fake = Fake::new(&[("nsf-record-update", OK)]);
        call("set-password", req("http://localhost:8080", serde_json::json!({"accountIdentifier": id, "sourceLabel": "Nested", "newAccountName": "Ti\"t$le", "newUserName": "u"})), &mut fake);
        assert_eq!(fake.commands(), [format!("nsf-record-update --force -r {UID} --title=\"Ti\\\"t\\$le\" login=\"u\"")]);
        // An unknown vault is refused, and nothing is sent.
        let mut fake = Fake::new(&[]);
        let r = call("set-password", req("http://localhost:8080", serde_json::json!({"accountIdentifier": id, "newPassword": "x"})), &mut fake);
        assert!(r["error"].as_str().unwrap().contains("vault is unknown"), "{r}");
        assert!(fake.sent.is_empty());

        // Add: nested first, classic with the toggle or when nested is unavailable.
        let added = serde_json::json!({"status":"success","data":{"record_uid": UID2}}).to_string();
        let mut fake = Fake::new(&[("nsf-record-add", &added)]);
        let r = call("add-account", req("http://localhost:8080", serde_json::json!({"userName": "bob", "accountName": "Site", "password": "pw"})), &mut fake);
        assert_eq!(r["accountIdentifier"]["accountID"], UID2);
        let b64 = base64::engine::general_purpose::STANDARD.encode("pw");
        assert_eq!(fake.commands(), [format!("nsf-record-add --force --record-type=login --title=\"Site\" login=\"bob\" password=$BASE64:{b64}")]);
        let mut fake = Fake::new(&[("record-add", &added)]);
        call("add-account", req("http://localhost:8080", serde_json::json!({"userName": "", "accountName": "Site", "flags": {"useClassicPermission": true}})), &mut fake);
        assert_eq!(fake.commands(), ["record-add --force --record-type=login --title=\"Site\""]);
        let mut fake = Fake::new(&[("nsf-record-add", "{\"status\":\"error\",\"error\":\"Invalid JSON response\"}"), ("record-add", &added)]);
        let r = call("add-account", req("http://localhost:8080", serde_json::json!({"userName": "bob", "accountName": "Site"})), &mut fake);
        assert_eq!(r["accountIdentifier"]["accountID"], UID2);
        assert_eq!(fake.commands().len(), 2);

        // sync-down is the custom command.
        let mut fake = Fake::new(&[("sync-down", OK)]);
        let r = call("sync-down", req("http://localhost:8080", serde_json::json!({"commandName": "sync-down"})), &mut fake);
        assert_eq!(r["message"], "Sync completed");
        assert_eq!(fake.commands(), ["sync-down"]);
        let r = call("bogus", req("http://localhost:8080", serde_json::json!({"commandName": "bogus"})), &mut Fake::new(&[]));
        assert_eq!(r["error"], "Unknown command: bogus");
    }

    #[test]
    fn polling_follows_iterm2() {
        // queued, processing, completed.
        let mut fake = Fake::new(&[("list", &classic_listing())]);
        fake.statuses = ["queued", "processing", "completed"].iter().map(|s| s.to_string()).collect();
        let r = call("list-accounts", req("http://localhost:8080", serde_json::json!({})), &mut fake);
        assert!(r["accounts"].as_array().is_some_and(|a| !a.is_empty()), "{r}");
        assert!(fake.paused >= Duration::from_millis(200), "polled with pauses: {:?}", fake.paused);
        // The first poll is 0.2 s, doubling to 2 s at most.
        let mut fake = Fake::new(&[("list", &classic_listing())]);
        fake.statuses = std::iter::repeat_n("processing".to_string(), 8).collect();
        call("login", serde_json::json!({"header": header("http://localhost:8080"), "masterPassword": KEY}), &mut fake);
        assert_eq!(fake.paused, Duration::from_millis(200 + 400 + 800 + 1600 + 2000 * 4));
        // failed and expired.
        for s in ["failed", "expired"] {
            let mut fake = Fake::new(&[("rm", OK)]);
            fake.statuses = VecDeque::from([s.to_string()]);
            let r = call("delete-account", req("http://localhost:8080", serde_json::json!({"accountIdentifier": {"accountID": UID}, "sourceLabel": "Classic"})), &mut fake);
            assert_eq!(r["error"], format!("Keeper command {s}"));
        }
        // Stuck in queued: the worker is wedged, told early.
        let mut fake = Fake::new(&[("list", &classic_listing())]);
        fake.statuses = std::iter::repeat_n("queued".to_string(), 200).collect();
        let r = call("login", serde_json::json!({"header": header("http://localhost:8080"), "masterPassword": KEY}), &mut fake);
        assert!(r["error"].as_str().unwrap().contains("never started processing it"), "{r}");
        assert!(fake.paused < Duration::from_secs(30));
        // Never completes: the total timeout.
        let mut fake = Fake::new(&[("list", &classic_listing())]);
        fake.statuses = std::iter::repeat_n("processing".to_string(), 500).collect();
        let r = call("login", serde_json::json!({"header": header("http://localhost:8080"), "masterPassword": KEY}), &mut fake);
        assert_eq!(r["error"], "Keeper service v2 request timed out");
        assert!(fake.paused >= Duration::from_secs(120) && fake.paused < Duration::from_secs(130), "{:?}", fake.paused);
        // The interactive read is bounded at 15 s.
        let mut fake = Fake::new(&[("get", "{}")]);
        fake.statuses = std::iter::repeat_n("processing".to_string(), 500).collect();
        let r = call("get-password", req("http://localhost:8080", serde_json::json!({"accountIdentifier": {"accountID": UID}})), &mut fake);
        assert_eq!(r["error"], "Keeper service v2 request timed out");
        assert!(fake.paused < Duration::from_secs(20), "{:?}", fake.paused);
    }

    #[test]
    fn errors_are_plain_and_scrubbed() {
        let mut fake = Fake::new(&[]);
        fake.post_status = 401;
        let r = call("login", serde_json::json!({"header": header("http://localhost:8080"), "masterPassword": KEY}), &mut fake);
        assert!(r["error"].as_str().unwrap().starts_with("API key rejected (HTTP 401)."), "{r}");
        fake.post_status = 503;
        let r = call("login", serde_json::json!({"header": header("http://localhost:8080"), "masterPassword": KEY}), &mut fake);
        assert!(r["error"].as_str().unwrap().starts_with("Keeper Commander returned an error (HTTP 503)."), "{r}");
        fake.post_status = 200;
        fake.post_body = Some("<!DOCTYPE html><html>x</html>".into());
        let r = call("login", serde_json::json!({"header": header("http://localhost:8080"), "masterPassword": KEY}), &mut fake);
        assert!(r["error"].as_str().unwrap().starts_with("The Service URL returned an HTML error page"), "{r}");
        fake.post_status = 400;
        fake.post_body = Some(format!("{{\"error\":\"bad \\u0007 {KEY} here\"}}"));
        let r = call("login", serde_json::json!({"header": header("http://localhost:8080"), "masterPassword": KEY}), &mut fake);
        let e = r["error"].as_str().unwrap();
        assert!(e.starts_with("bad ") && !e.contains(KEY) && !e.contains('\u{7}'), "{e}");
        // A transport failure is plain too.
        let mut fake = Fake::new(&[]);
        fake.transport_error = Some("Could not connect to the server.".into());
        let r = call("login", serde_json::json!({"header": header("http://localhost:8080"), "masterPassword": KEY}), &mut fake);
        assert!(r["error"].as_str().unwrap().starts_with("Could not reach Keeper Commander"), "{r}");
        // A missing or empty key.
        let r = call("list-accounts", serde_json::json!({"header": header("http://localhost:8080")}), &mut Fake::new(&[]));
        assert_eq!(r["error"], "Invalid or missing API key");
        // A failed mutation: the server's text is shown, the new password never is.
        let mut fake = Fake::new(&[("record-update", "{\"status\":\"error\",\"error\":\"bad pwd hunter2 given\"}")]);
        let r = call("set-password", req("http://localhost:8080", serde_json::json!({"accountIdentifier": {"accountID": UID}, "sourceLabel": "Classic", "newPassword": "hunter2"})), &mut fake);
        let e = r["error"].as_str().unwrap();
        assert!(!e.contains("hunter2"), "{e}");
        assert_eq!(e, "Password field is required.");
        // Output is capped.
        let long = "x".repeat(5000);
        let mut fake = Fake::new(&[("rm", &format!("{{\"status\":\"error\",\"error\":\"{long}\"}}"))]);
        let r = call("delete-account", req("http://localhost:8080", serde_json::json!({"accountIdentifier": {"accountID": UID}, "sourceLabel": "Classic"})), &mut fake);
        assert!(r["error"].as_str().unwrap().chars().count() <= 400);
    }

    #[test]
    fn inputs_are_checked_before_anything_is_sent() {
        let id = |s: &str| serde_json::json!({"accountID": s});
        for bad in ["a/b", "short", "../../etc/passwd/xxxx", "AbCdEfGhIjKlMnOp QrStUv", "AbCdEfGhIjKlMnOpQrStUv\n", &"A".repeat(200)] {
            for sub in ["get-password", "delete-account"] {
                let mut fake = Fake::new(&[]);
                let r = call(sub, req("http://localhost:8080", serde_json::json!({"accountIdentifier": id(bad), "sourceLabel": "Classic"})), &mut fake);
                // A trailing newline is trimmed like iTerm2 does; that one is a valid id.
                if bad.trim() == UID {
                    continue;
                }
                assert_eq!(r["error"], "Invalid record identifier.", "{sub} {bad:?}");
                assert!(fake.sent.is_empty());
            }
        }
        // The legacy "classic:" prefix is accepted and routes.
        let mut fake = Fake::new(&[("rm", OK)]);
        call("delete-account", req("http://localhost:8080", serde_json::json!({"accountIdentifier": id(&format!("classic:{UID}"))})), &mut fake);
        assert_eq!(fake.commands(), [format!("rm -f {UID}")]);
        // Service URLs: http and https only, no userinfo, no cleartext off the machine.
        for (url, ok) in [
            ("http://localhost:8900", true),
            ("http://127.0.0.1:8900/api/v2", true),
            ("http://[::1]:8900", true),
            ("https://keeper.example.com/api/v2", true),
            ("ftp://localhost:8900", false),
            ("file:///etc/passwd", false),
            ("localhost:8900", false),
            ("http://user:pw@localhost:8900", false),
            ("https://user@keeper.example.com", false),
            ("http://keeper.example.com", false),
            ("http://192.168.1.5:8900", false),
            ("http://evil.example\\@localhost/", false),
            ("http://localhost.evil.com", false),
            ("http://127.0.0.1.nip.io", false),
            ("http://[::ffff:127.0.0.1]", false),
            ("http://2130706433", false),
            ("", false),
        ] {
            let mut fake = Fake::new(&[("list", &classic_listing())]);
            let r = call("login", serde_json::json!({"header": header(url), "masterPassword": KEY}), &mut fake);
            assert_eq!(r.get("error").is_none(), ok, "{url}: {r}");
            if !ok {
                assert!(fake.sent.is_empty(), "{url}");
            }
        }
        // The URL falls back to apiURL, then to the database path.
        let mut fake = Fake::new(&[("list", &classic_listing())]);
        let h = serde_json::json!({"mode": "terminal", "pathToDatabase": "http://localhost:9000"});
        call("login", serde_json::json!({"header": h, "masterPassword": KEY}), &mut fake);
        assert!(fake.sent[0].url.starts_with("http://localhost:9000/api/v2/"));
        // A request id that could change the path is refused.
        struct Evil(Fake);
        impl Http for Evil {
            fn send(&mut self, m: &str, u: &str, h: &[(String, String)], b: &[u8], t: Duration) -> Result<(u16, Vec<u8>), String> {
                if m == "POST" {
                    self.0.sent.push(Sent { method: m.into(), url: u.into(), headers: h.to_vec(), body: String::new(), timeout: t });
                    return Ok((202, b"{\"request_id\":\"../../admin\",\"status\":\"queued\"}".to_vec()));
                }
                self.0.send(m, u, h, b, t)
            }
            fn pause(&mut self, _: Duration) {}
        }
        let mut evil = Evil(Fake::new(&[]));
        let out = handle("login", serde_json::json!({"header": header("http://localhost:8080"), "masterPassword": KEY}).to_string().as_bytes(), &mut evil);
        let r: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert!(r["error"].as_str().unwrap().contains("request_id"), "{r}");
        assert_eq!(evil.0.sent.len(), 1);
        // A name or user with a control character is cut to a space, never a second command.
        let mut fake = Fake::new(&[("nsf-record-add", &serde_json::json!({"status":"success","data":{"uid": UID2}}).to_string())]);
        call("add-account", req("http://localhost:8080", serde_json::json!({"userName": "a\nb", "accountName": "t\r\"x"})), &mut fake);
        assert_eq!(fake.commands(), ["nsf-record-add --force --record-type=login --title=\"t \\\"x\" login=\"a b\""]);
    }

    #[test]
    fn parsers_follow_iterm2() {
        let r = parse_ls_record_line(&ls_line(7, UID, "Mail", "me@x.com @ https://m.x.com")).unwrap();
        assert_eq!(r.effective_uid().as_deref(), Some(UID));
        assert_eq!(r.display_title(), "Mail");
        assert_eq!(r.list_user_name(), "me@x.com");
        assert!(parse_ls_record_line("#  Record UID  Type  Title").is_none());
        assert!(parse_ls_record_line("---  ---  ---  ---").is_none());
        assert!(parse_ls_record_line(&ls_line(1, "short", "T", "d")).is_none());
        assert!(parse_ls_record_line("x  AbCdEfGhIjKlMnOpQrStUv  login  T").is_none());
        // A description that is not connection info stays whole.
        let r = parse_ls_record_line(&ls_line(1, UID, "T", "a @ b c")).unwrap();
        assert_eq!(r.list_user_name(), "a @ b c");
        // Passwords and logins from a get answer, in each shape.
        let p = |s: &str| password_from_get_json(s.as_bytes());
        assert_eq!(p(r#"{"data":{"password":" a "}}"#).as_deref(), Some("a"));
        assert_eq!(p(r#"{"data":"{\"fields\":[{\"type\":\"password\",\"value\":\"b\"}]}"}"#).as_deref(), Some("b"));
        assert_eq!(p(r#"{"record":{"fields":[{"type":"password","value":["c"]}]}}"#).as_deref(), Some("c"));
        assert_eq!(p(r#"{"data":{"password":"  "}}"#), None);
        assert_eq!(p("not json"), None);
        let l = |s: &str| login_from_get_json(s.as_bytes());
        assert_eq!(l(r#"{"data":{"login":"u"}}"#).as_deref(), Some("u"));
        assert_eq!(l(r#"{"data":{"fields":[{"type":"login","value":["v"]}]}}"#).as_deref(), Some("v"));
        assert_eq!(l(r#"{"data":{}}"#), None);
        // An error envelope is never read as the password.
        let mut fake = Fake::new(&[("get", "{\"status\":\"error\",\"error\":\"Record not found\"}")]);
        let r = call("get-password", req("http://localhost:8080", serde_json::json!({"accountIdentifier": {"accountID": UID}})), &mut fake);
        assert_eq!(r["error"], "Keeper returned no password for this record.");
        let mut fake = Fake::new(&[("get", "{\"message\":[\"Record not found\"]}")]);
        let r = call("get-password", req("http://localhost:8080", serde_json::json!({"accountIdentifier": {"accountID": UID}})), &mut fake);
        assert_eq!(r["error"], "Keeper returned no password for this record.");
        // A success envelope whose inner result failed is a failure.
        let mut fake = Fake::new(&[("rm", "{\"status\":\"success\",\"result\":\"{\\\"status\\\":\\\"error\\\",\\\"error\\\":\\\"inner\\\"}\"}")]);
        let r = call("delete-account", req("http://localhost:8080", serde_json::json!({"accountIdentifier": {"accountID": UID}, "sourceLabel": "Classic"})), &mut fake);
        assert!(r["error"].is_string(), "{r}");
        // A listing in the wrapped envelope.
        let wrapped = serde_json::json!({"status":"success","result": classic_listing()}).to_string();
        let mut fake = Fake::new(&[("list", &wrapped)]);
        let r = call("list-accounts", req("http://localhost:8080", serde_json::json!({})), &mut fake);
        assert_eq!(r["accounts"][0]["identifier"]["accountID"], UID, "{r}");
    }

    #[test]
    fn a_nested_listing_failure_warns_and_a_missing_nsf_does_not() {
        let mut fake = Fake::new(&[("list", &classic_listing()), ("nsf-list", "{\"status\":\"error\",\"error\":\"Invalid JSON response\"}")]);
        let r = call("list-accounts", req("http://localhost:8080", serde_json::json!({})), &mut fake);
        assert_eq!(r["accounts"].as_array().unwrap().len(), 1);
        assert!(r.get("warning").is_none() || r["warning"].is_null(), "{r}");
        let mut fake = Fake::new(&[("list", &classic_listing()), ("nsf-list", "{\"status\":\"error\",\"error\":\"database is locked\"}")]);
        let r = call("list-accounts", req("http://localhost:8080", serde_json::json!({})), &mut fake);
        assert!(r["warning"].as_str().unwrap().starts_with("Nested Shared Folder listing failed (database is locked)."), "{r}");
        // Names are shown clean.
        let rec = serde_json::json!({"status":"success","command":"ls","data":{"records":[{"title": ls_line(1, UID, "Ma\u{202e}il\u{7}", "d")}]}}).to_string();
        let mut fake = Fake::new(&[("list", &rec), ("nsf-list", OK)]);
        let r = call("list-accounts", req("http://localhost:8080", serde_json::json!({})), &mut fake);
        assert_eq!(r["accounts"][0]["accountName"], "Mail");
    }

    #[test]
    fn the_transports_status_text_is_read_back() {
        assert_eq!(status_from_error(&crate::ai_transport::status_error(202)), Some(202));
        assert_eq!(status_from_error(&crate::ai_transport::status_error(401)), Some(401));
        assert_eq!(status_from_error(&crate::ai_transport::transport_error("x")), None);
    }

    #[test]
    fn the_executable_uses_only_the_shared_framework() {
        let src = include_str!("bin/iterm2-keeper-adapter.rs");
        assert!(src.contains("tandem::pwm_keeper"));
        for line in src.lines().filter(|l| l.contains("println!")) {
            assert!(line.contains("runs on Windows only"), "{line}");
        }
        for line in src.lines().filter(|l| l.contains("tandem::")) {
            assert!(["pwm_adapter", "pwm_keeper", "dll::"].iter().any(|ok| line.contains(ok)), "{line}");
        }
        let me = include_str!("pwm_keeper.rs");
        let prod = me.split("#[cfg(test)]").next().unwrap();
        for banned in ["println!", "eprintln!", "dbg!", "log::", "env::var", "temp_dir", "File::create", "fs::write"] {
            assert!(!prod.contains(banned), "{banned}");
        }
    }
}
