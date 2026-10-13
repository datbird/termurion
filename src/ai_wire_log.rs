//! The raw AI wire log (`aiChatRawWireLogging`) and the verbose console log
//! (`aiChatVerboseConsoleLogging`). Record formats and credential scrubbing are pure; the file
//! writer and the console are Windows-only. Ports `AITerm/AIChatWireLogger.swift` (`WL:12-305`)
//! and `ChatAgentConsoleLogger.swift`.

use crate::ai_transport::HttpRequest;
use base64::Engine;
use std::sync::OnceLock;
use std::time::Duration;

/// The `aiChatRawWireLogging` help (ruling 10: it says what the code does).
pub const HELP: &str = "Log raw AI API requests and responses to disk.\nWrites every byte sent to and received from the AI vendor (request headers, body, streaming chunks, final response, errors, and timing) to %APPDATA%\\Tandem\\AIChatWire\\. One log file per app launch; rotate or delete it yourself when done. Every header value except known non-secret ones (such as Content-Type, Accept, User-Agent and anthropic-version), and keys that appear in URLs or bodies, are replaced by fingerprints (the first 8 and last 4 characters); prompt and response content is logged verbatim. Off by default; turn on only while reproducing an issue and delete the files when finished.";

/// Credential headers whose value may start with a scheme ("Bearer") the log keeps
/// (`WL:239-241`, plus Proxy-Authorization), lowercased.
const SCHEME_HEADERS: [&str; 5] = ["authorization", "proxy-authorization", "x-api-key", "api-key", "x-goog-api-key"];

/// The headers whose values are logged, known to carry no secret; every other header's value is
/// fingerprinted, since a custom header is usually a credential (final review ruling; iTerm2
/// fingerprints only the four vendor key headers). Lowercased.
const SAFE_HEADERS: [&str; 11] = [
    "accept",
    "accept-encoding",
    "accept-language",
    "anthropic-beta",
    "anthropic-version",
    "cache-control",
    "content-encoding",
    "content-length",
    "content-type",
    "openai-beta",
    "user-agent",
];

fn is_safe(name: &str) -> bool {
    SAFE_HEADERS.contains(&name.to_ascii_lowercase().as_str())
}

/// Key-shaped tokens (`WL:253-261`): OpenAI, Anthropic and DeepSeek `sk-` keys and Google's
/// `AIza` keys, never inside a longer identifier.
fn secret_patterns() -> &'static [fancy_regex::Regex; 2] {
    static PATTERNS: OnceLock<[fancy_regex::Regex; 2]> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        [
            fancy_regex::Regex::new(r"(?<![A-Za-z0-9_-])sk-[A-Za-z0-9_-]{16,}").expect("sk pattern"),
            fancy_regex::Regex::new(r"(?<![A-Za-z0-9_-])AIza[0-9A-Za-z_-]{35}").expect("AIza pattern"),
        ]
    })
}

/// Enough of a secret to tell which key it was (`WL:292-304`): under 32 characters nothing, else
/// the first 8 and last 4. Lengths count characters (Decision 14).
pub fn fingerprint(secret: &str) -> String {
    let n = secret.chars().count();
    if n < 32 {
        return format!("[redacted {n} chars]");
    }
    let prefix: String = secret.chars().take(8).collect();
    let suffix: String = secret.chars().skip(n - 4).collect();
    format!("{prefix}\u{2026}{suffix} [redacted {n} chars]")
}

/// Every key-shaped token replaced by its fingerprint (`WL:278-290`).
pub fn scrub(text: &str) -> String {
    let mut result = text.to_string();
    for re in secret_patterns() {
        let mut out = String::with_capacity(result.len());
        let mut last = 0;
        for m in re.find_iter(&result) {
            // A regex runtime error (backtrack limit) would leave the rest unscrubbed: omit it
            // rather than write it.
            let Ok(m) = m else {
                out.push_str("[unscrubbed rest omitted]");
                last = result.len();
                break;
            };
            out.push_str(&result[last..m.start()]);
            out.push_str(&fingerprint(m.as_str()));
            last = m.end();
        }
        out.push_str(&result[last..]);
        result = out;
    }
    result
}

/// `text` with the request's key replaced by its fingerprint wherever it appears, as typed,
/// trimmed or percent-encoded (the Gemini URL form), whatever its shape (Task 7 review); None or
/// a blank key changes nothing.
pub fn redact_key(text: &str, key: Option<&str>) -> String {
    let Some(key) = key.filter(|k| !k.trim().is_empty()) else {
        return text.to_string();
    };
    let encoded = |k: &str| k.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-._~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect::<String>();
    let mut forms = vec![key.to_string(), key.trim().to_string(), encoded(key), encoded(key.trim())];
    // Longest first, so a form inside another is not replaced half.
    forms.sort_by_key(|f| std::cmp::Reverse(f.len()));
    forms.dedup();
    let mut out = text.to_string();
    for form in forms {
        out = out.replace(&form, &fingerprint(&form));
    }
    out
}

/// Every key-shaped token, and the request's own key, replaced by fingerprints.
fn clean(text: &str, key: Option<&str>) -> String {
    scrub(&redact_key(text, key))
}

/// The URL as logged: the value of every `key` query parameter fingerprinted whatever its shape
/// (Gemini carries the key there), then cleaned.
pub fn url_line(url: &str, key: Option<&str>) -> String {
    let Some(q) = url.find('?') else {
        return clean(url, key);
    };
    let (head, rest) = url.split_at(q + 1);
    let (query, fragment) = match rest.find('#') {
        Some(at) => rest.split_at(at),
        None => (rest, ""),
    };
    let params: Vec<String> = query
        .split('&')
        .map(|p| match p.strip_prefix("key=") {
            Some(value) if !value.is_empty() => format!("key={}", fingerprint(value)),
            _ => p.to_string(),
        })
        .collect();
    clean(&format!("{head}{}{fragment}", params.join("&")), key)
}

/// A header value as logged (`WL:263-276`): a known non-secret header is scrubbed; any other is
/// fingerprinted whole, and a credential header keeps a scheme ("Bearer") that ends within the
/// first 16 characters.
pub fn header_value(name: &str, value: &str) -> String {
    if is_safe(name) {
        return scrub(value);
    }
    if SCHEME_HEADERS.contains(&name.to_ascii_lowercase().as_str()) {
        if let Some((at, _)) = value.char_indices().find(|(_, c)| *c == ' ') {
            if value[..at].chars().count() <= 16 {
                return format!("{} {}", &value[..at], fingerprint(&value[at + 1..]));
            }
        }
    }
    fingerprint(value)
}

/// The per-launch file name, UTC to the second (`WL:209-218`).
pub fn file_name(utc: (u16, u16, u16, u16, u16, u16), pid: u32) -> String {
    let (y, mo, d, h, mi, s) = utc;
    format!("aichat-wire-{y:04}{mo:02}{d:02}T{h:02}{mi:02}{s:02}-pid{pid}.log")
}

/// The file's first line (`WL:181-183`).
pub fn header_line(stamp: &str, pid: u32) -> String {
    format!("# Termurion AI chat wire log, started {stamp} pid {pid}\n\n")
}

fn push_text(out: &mut String, text: &str) {
    out.push_str(text);
    if !text.ends_with('\n') {
        out.push('\n');
    }
}

/// A request (`WL:63-93`): headers sorted by name, a UTF-8 body scrubbed, any other base64.
/// `key` is the key the request carries, hidden wherever it appears.
pub fn request_record(id: &str, stamp: &str, r: &HttpRequest, key: Option<&str>) -> String {
    let mut text = format!("=== call {id} request {stamp} ===\n{} {}\nHeaders ({}):\n", r.method, url_line(&r.url, key), r.headers.len());
    let mut headers: Vec<&(String, String)> = r.headers.iter().collect();
    headers.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    for (name, value) in headers {
        // A logged value still hides the key inside it; any other is fingerprinted whole.
        let value = if is_safe(name) { redact_key(value, key) } else { value.clone() };
        text.push_str(&format!("{name}: {}\n", header_value(name, &value)));
    }
    match std::str::from_utf8(&r.body) {
        Ok(s) => {
            text.push_str(&format!("Body (string, {} bytes):\n", s.len()));
            push_text(&mut text, &clean(s, key));
        }
        Err(_) => {
            text.push_str(&format!("Body (binary, {} bytes, base64):\n", r.body.len()));
            text.push_str(&base64::engine::general_purpose::STANDARD.encode(&r.body));
            text.push('\n');
        }
    }
    text.push_str(&format!("--- end request {id} ---\n\n"));
    text
}

/// One streamed chunk (`WL:99-110`).
pub fn chunk_record(id: &str, stamp: &str, chunk: &str, key: Option<&str>) -> String {
    let mut text = format!("=== call {id} stream chunk {stamp} ({} bytes) ===\n", chunk.len());
    push_text(&mut text, &clean(chunk, key));
    text.push_str(&format!("--- end chunk {id} ---\n\n"));
    text
}

/// The completed response, with an in-band error when there is one (`WL:112-128`).
pub fn response_record(id: &str, stamp: &str, elapsed: Duration, body: &str, error: Option<&str>, key: Option<&str>) -> String {
    let mut text = format!("=== call {id} response {stamp} elapsed={:.3}s ===\nBody ({} bytes):\n", elapsed.as_secs_f64(), body.len());
    push_text(&mut text, &clean(body, key));
    if let Some(e) = error.filter(|e| !e.is_empty()) {
        text.push_str(&format!("Error (in-band): {}\n", clean(e, key)));
    }
    text.push_str(&format!("--- end response {id} ---\n\n"));
    text
}

/// A call that failed before a response (`WL:130-140`); the text is scrubbed too (Decision 7).
pub fn failure_record(id: &str, stamp: &str, elapsed: Duration, text: &str, key: Option<&str>) -> String {
    format!("=== call {id} error {stamp} elapsed={:.3}s ===\n{}\n--- end error {id} ---\n\n", elapsed.as_secs_f64(), clean(text, key))
}

/// Year, month, day, hour, minute, second and millisecond of a Unix time in UTC.
fn utc_parts(unix_millis: u64) -> (u16, u16, u16, u16, u16, u16, u16) {
    let secs = unix_millis / 1000;
    let days = (secs / 86400) as i64;
    let rem = secs % 86400;
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y as u16, m as u16, d as u16, (rem / 3600) as u16, (rem / 60 % 60) as u16, (rem % 60) as u16, (unix_millis % 1000) as u16)
}

/// ISO 8601 UTC with milliseconds (`WL:55-59`).
pub fn iso_stamp(unix_millis: u64) -> String {
    let (y, mo, d, h, mi, s, ms) = utc_parts(unix_millis);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}.{ms:03}Z")
}

/// Milliseconds since the Unix epoch now.
pub fn now_millis() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// The per-launch wire log file under `%APPDATA%\Tandem\AIChatWire\`, opened on the first record
/// written while the setting is on; a failure to open disables it for the launch (`WL:157-189`).
#[cfg(windows)]
pub struct WireLog {
    dir: std::path::PathBuf,
    file: Option<std::fs::File>,
    path: Option<std::path::PathBuf>,
    attempted: bool,
}

#[cfg(windows)]
impl Default for WireLog {
    fn default() -> WireLog {
        WireLog::new()
    }
}

#[cfg(windows)]
impl WireLog {
    pub fn new() -> WireLog {
        let data = crate::globals::default_path().parent().map(std::path::Path::to_path_buf).unwrap_or_default();
        WireLog::in_dir(data.join("AIChatWire"))
    }

    fn in_dir(dir: std::path::PathBuf) -> WireLog {
        WireLog { dir, file: None, path: None, attempted: false }
    }

    /// Appends `record` when `enabled` (the setting is read per record, `WL:51-53`).
    pub fn log(&mut self, enabled: bool, record: &str) {
        use std::io::Write;
        if !enabled {
            return;
        }
        if let Some(f) = self.open() {
            let _ = f.write_all(record.as_bytes());
        }
    }

    /// The file, once it is open.
    pub fn path(&self) -> Option<&std::path::Path> {
        self.path.as_deref()
    }

    fn open(&mut self) -> Option<&mut std::fs::File> {
        use std::io::Write;
        if self.file.is_none() && !self.attempted {
            self.attempted = true;
            if std::fs::create_dir_all(&self.dir).is_ok() && self.dir.is_dir() {
                let now = now_millis();
                let (y, mo, d, h, mi, s, _) = utc_parts(now);
                let path = self.dir.join(file_name((y, mo, d, h, mi, s), std::process::id()));
                if let Ok(mut f) = std::fs::File::create(&path) {
                    let _ = f.write_all(header_line(&iso_stamp(now), std::process::id()).as_bytes());
                    self.file = Some(f);
                    self.path = Some(path);
                }
            }
        }
        self.file.as_mut()
    }
}

/// One verbose console line, "Termurion AI: <line>", for DebugView or a debugger (Decision 6); a
/// no-op when disabled and off Windows.
pub fn console(enabled: bool, line: &str) {
    if !enabled {
        return;
    }
    #[cfg(windows)]
    {
        let wide: Vec<u16> = format!("Termurion AI: {line}\n").encode_utf16().chain(std::iter::once(0)).collect();
        unsafe { windows_sys::Win32::System::Diagnostics::Debug::OutputDebugStringW(wide.as_ptr()) };
    }
    #[cfg(not(windows))]
    let _ = line;
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY40: &str = "sk-proj-ABCDEFGHIJKLMNOPQRSTUVWXYZ012345";

    #[test]
    fn wire_log_redacts_follows_iterm2() {
        assert_eq!(KEY40.chars().count(), 40);
        assert_eq!(fingerprint("short"), "[redacted 5 chars]");
        assert_eq!(fingerprint(KEY40), "sk-proj-\u{2026}2345 [redacted 40 chars]");
        assert_eq!(header_value("Authorization", &format!("Bearer {KEY40}")), "Bearer sk-proj-\u{2026}2345 [redacted 40 chars]");
        assert_eq!(header_value("X-API-KEY", "k"), "[redacted 1 chars]");
        assert_eq!(header_value("api-key", KEY40), "sk-proj-\u{2026}2345 [redacted 40 chars]");
        let google = format!("AIza{}", "B".repeat(35));
        assert_eq!(header_value("x-goog-api-key", &google), "AIzaBBBB\u{2026}BBBB [redacted 39 chars]");
        // A scheme longer than 16 characters is part of the credential.
        assert_eq!(header_value("Authorization", "AVeryLongSchemeName x"), "[redacted 21 chars]");
        assert_eq!(header_value("Content-Type", "a sk-ABCDEFGHIJKLMNOPQRSTUV b"), "a [redacted 25 chars] b", "a logged value is scrubbed");
        assert_eq!(header_value("content-type", "application/json"), "application/json");
        assert_eq!(header_value("X-Custom", "plain"), "[redacted 5 chars]", "an unknown header is fingerprinted");
        let url = format!("key={google}&alt=sse");
        assert_eq!(scrub(&url), "key=AIzaBBBB\u{2026}BBBB [redacted 39 chars]&alt=sse");
        assert_eq!(scrub("mask-ABCDEFGHIJKLMNOPQRSTUV"), "mask-ABCDEFGHIJKLMNOPQRSTUV");
        assert_eq!(scrub("sk-short"), "sk-short");
        assert_eq!(scrub(&format!("{KEY40} and {KEY40}")), format!("{0} and {0}", fingerprint(KEY40)));
    }

    /// Controller ruling (Task 7 review): the configured key and any `key=` query value are
    /// redacted whatever their shape, not only when a key pattern matches.
    #[test]
    fn wire_log_redacts_any_key_shape() {
        for (key, encoded) in [("AIzaK", "AIzaK"), ("not a google key!", "not%20a%20google%20key%21")] {
            let r = HttpRequest {
                method: "POST",
                url: format!("https://g.test/v1/models/m:generateContent?alt=sse&key={encoded}#f"),
                headers: vec![("X-Custom".into(), format!("token {key}")), ("Content-Type".into(), "application/json".into())],
                body: format!("{{\"echo\":\"{key}\"}}").into_bytes(),
                stream: true,
            };
            let text = request_record("U", "T", &r, Some(key));
            assert!(!text.contains(key) && !text.contains(encoded), "the key is in no request line ({} chars)", key.len());
            let url_line = text.lines().nth(1).unwrap_or_default();
            assert!(url_line.starts_with("POST https://g.test/v1/models/m:generateContent?alt=sse&key=[redacted "), "the key= value is fingerprinted");
            assert!(url_line.ends_with(" chars]#f"), "the fragment stays");
            assert!(text.contains("X-Custom: [redacted "), "a custom header is fingerprinted whole");
            // Without the key known, the key= value is still hidden.
            let unknown = request_record("U", "T", &r, None);
            assert!(!unknown.lines().nth(1).unwrap_or_default().contains(encoded), "any key= value is hidden");
            for record in [
                chunk_record("U", "T", &format!("data: {key}\n"), Some(key)),
                response_record("U", "T", Duration::from_millis(1), &format!("body {key}"), Some(&format!("failed {key}")), Some(key)),
                failure_record("U", "T", Duration::from_millis(1), &format!("bad {key}"), Some(key)),
            ] {
                assert!(!record.contains(key), "no key in a chunk, a response or an error");
            }
        }
        // Only a parameter named exactly "key" is a key.
        let r = HttpRequest { method: "GET", url: "https://h.test/x?monkey=1&keys=2&key=3".into(), headers: vec![], body: Vec::new(), stream: false };
        assert!(request_record("U", "T", &r, None).contains("GET https://h.test/x?monkey=1&keys=2&key=[redacted 1 chars]\n"));
    }

    /// Final review ruling: every header value is fingerprinted except known non-secret names.
    #[test]
    fn wire_log_fingerprints_every_header_but_the_safe_ones() {
        let secret = "gateway-secret-0123456789";
        let r = HttpRequest {
            method: "POST",
            url: "https://gw.test/v1/chat/completions".into(),
            headers: vec![
                ("X-Auth-Token".into(), secret.into()),
                ("Proxy-Authorization".into(), format!("Basic {secret}")),
                ("X-Access-Token".into(), secret.into()),
                ("x-custom-thing".into(), format!("tok {secret}")),
                ("Content-Type".into(), "application/json".into()),
                ("Accept".into(), "text/event-stream".into()),
                ("User-Agent".into(), "Termurion".into()),
                ("anthropic-version".into(), "2023-06-01".into()),
                ("OpenAI-Beta".into(), "assistants=v2".into()),
                ("Content-Length".into(), "2".into()),
            ],
            body: b"{}".to_vec(),
            stream: true,
        };
        let text = request_record("U", "T", &r, None);
        assert!(!text.contains("secret-0123456789"), "{text}");
        assert!(text.contains("X-Auth-Token: [redacted 25 chars]\n"), "{text}");
        assert!(text.contains("Proxy-Authorization: Basic [redacted 25 chars]\n"), "the scheme stays: {text}");
        assert!(text.contains("X-Access-Token: [redacted 25 chars]\n"));
        assert!(text.contains("x-custom-thing: [redacted 29 chars]\n"), "an unknown header keeps no first word");
        for line in ["Content-Type: application/json", "Accept: text/event-stream", "User-Agent: Termurion", "anthropic-version: 2023-06-01", "OpenAI-Beta: assistants=v2", "Content-Length: 2"] {
            assert!(text.contains(&format!("{line}\n")), "{line} is logged");
        }
    }

    #[test]
    fn wire_log_records_follow_iterm2() {
        assert_eq!(file_name((2026, 9, 27, 14, 30, 5), 4242), "aichat-wire-20260927T143005-pid4242.log");
        assert_eq!(header_line("2026-09-27T14:30:05.123Z", 4242), "# Termurion AI chat wire log, started 2026-09-27T14:30:05.123Z pid 4242\n\n");
        let r = HttpRequest {
            method: "POST",
            url: "https://h.test/x".into(),
            headers: vec![("x-api-key".into(), "k".into()), ("Content-Type".into(), "application/json".into())],
            body: b"{}".to_vec(),
            stream: true,
        };
        assert_eq!(
            request_record("U", "T", &r, None),
            "=== call U request T ===\nPOST https://h.test/x\nHeaders (2):\nContent-Type: application/json\nx-api-key: [redacted 1 chars]\nBody (string, 2 bytes):\n{}\n--- end request U ---\n\n"
        );
        let bin = HttpRequest { body: vec![0xff, 0x00, 0x01], headers: vec![], ..r.clone() };
        assert_eq!(request_record("U", "T", &bin, None), "=== call U request T ===\nPOST https://h.test/x\nHeaders (0):\nBody (binary, 3 bytes, base64):\n/wAB\n--- end request U ---\n\n");
        let keyed = HttpRequest { url: format!("https://h.test/x?key=AIza{}", "C".repeat(35)), body: format!("{{\"k\":\"{KEY40}\"}}\n").into_bytes(), headers: vec![], ..r };
        let text = request_record("U", "T", &keyed, None);
        assert!(!text.contains(&"C".repeat(35)) && !text.contains(KEY40), "keys in the URL and body are scrubbed");
        assert!(text.contains("Body (string, 49 bytes):\n{\"k\":\"sk-proj-\u{2026}2345 [redacted 40 chars]\"}\n--- end"), "no second newline");
        assert_eq!(chunk_record("U", "T", "data: x\n", None), "=== call U stream chunk T (8 bytes) ===\ndata: x\n--- end chunk U ---\n\n");
        assert_eq!(chunk_record("U", "T", "x", None), "=== call U stream chunk T (1 bytes) ===\nx\n--- end chunk U ---\n\n");
        assert_eq!(
            response_record("U", "T", Duration::from_millis(1500), "{}", Some("HTTP request failed with status 500."), None),
            "=== call U response T elapsed=1.500s ===\nBody (2 bytes):\n{}\nError (in-band): HTTP request failed with status 500.\n--- end response U ---\n\n"
        );
        assert_eq!(response_record("U", "T", Duration::from_millis(2), "ok\n", Some(""), None), "=== call U response T elapsed=0.002s ===\nBody (3 bytes):\nok\n--- end response U ---\n\n");
        assert_eq!(
            failure_record("U", "T", Duration::from_millis(250), &format!("bad {KEY40}"), None),
            format!("=== call U error T elapsed=0.250s ===\nbad {}\n--- end error U ---\n\n", fingerprint(KEY40))
        );
        assert_eq!(iso_stamp(1790519405123), "2026-09-27T14:30:05.123Z");
        assert_eq!(iso_stamp(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(iso_stamp(951_782_400_007), "2000-02-29T00:00:00.007Z");
    }

    #[test]
    fn wire_log_help_says_what_the_code_does() {
        let row = crate::schema::find_global("AiChatRawWireLogging").expect("12a's row");
        assert_eq!(row.help, HELP);
        assert!(HELP.contains("fingerprints"));
        assert!(HELP.contains("%APPDATA%\\Tandem\\AIChatWire"));
        assert!(HELP.contains("Every header value except known non-secret ones"), "says every other header is hidden");
        for name in ["Content-Type", "Accept", "User-Agent", "anthropic-version"] {
            assert!(SAFE_HEADERS.contains(&name.to_ascii_lowercase().as_str()), "the help's examples are logged: {name}");
        }
    }

    #[cfg(windows)]
    #[test]
    fn wire_log_opens_one_file_on_the_first_record() {
        let dir = std::env::temp_dir().join(format!("tandem-wire-log-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut log = WireLog::in_dir(dir.clone());
        log.log(false, "off\n");
        assert!(log.path().is_none() && !dir.exists(), "nothing opens while the setting is off");
        log.log(true, "one\n");
        log.log(true, "two\n");
        let path = log.path().expect("opened").to_path_buf();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.starts_with("aichat-wire-") && name.ends_with(&format!("-pid{}.log", std::process::id())), "{name}");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("# Termurion AI chat wire log, started "), "{text}");
        assert!(text.ends_with("\n\none\ntwo\n"), "{text}");
        drop(log);
        std::fs::remove_dir_all(&dir).unwrap();

        // A folder that cannot be made disables the log for the launch.
        let blocker = std::env::temp_dir().join(format!("tandem-wire-log-block-{}", std::process::id()));
        std::fs::write(&blocker, b"x").unwrap();
        let mut blocked = WireLog::in_dir(blocker.join("AIChatWire"));
        blocked.log(true, "one\n");
        assert!(blocked.path().is_none());
        std::fs::remove_file(&blocker).unwrap();
        blocked.log(true, "two\n");
        assert!(blocked.path().is_none(), "one attempt per launch");
        console(true, "wire log test");
        console(false, "never shown");
    }
}
