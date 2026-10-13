//! OpenAI files and vector stores for a chat's attachments: create a store, upload a file, add
//! files to a store and wait while OpenAI ingests them. Pure over `ai_transport::Transport`;
//! ports `AITerm/LLMFiles.swift`, `LLMVectorStore.swift`, `HTTPHelpers.swift` and the file
//! paths of `AITerm.swift` (`T:706-920`).

use crate::ai_catalog::{Model, VectorStore};
use crate::ai_model::display_name;
use crate::ai_policy;
use crate::ai_settings::{Api, Vendor};
use crate::ai_transport::{Call, HttpEvent, HttpRequest, Notify, Options, Transport};
use crate::ai_turn::{could_not_decode, error_reason, provider_error, NO_KEY, REVIEW_KEY};
use crate::pwm::Secret;
use std::sync::Arc;
use std::time::{Duration, Instant};

pub const STORES_URL: &str = "https://api.openai.com/v1/vector_stores";
pub const FILES_URL: &str = "https://api.openai.com/v1/files";
/// How long a batch still being ingested waits before it is asked again (`T:807-813`).
pub const POLL_EVERY: Duration = Duration::from_millis(500);

pub const UNSUPPORTED_STORES: &str = "Vector stores are not supported with this LLM vendor";
pub const ADD_FAILED: &str = "Adding file to vector store failed";
pub const INGEST_ERROR: &str = "An error occurred while ingesting files to vector storage";
/// Shown beside `INGEST_ERROR` (12c).
pub const NOT_WELL_FORMED: &str = "The file may not be well formed.";
pub const UPLOAD_UNSUPPORTED: &str = "File upload not supported with this LLM vendor";
pub const UPLOAD_UNSUPPORTED_PROVIDER: &str = "File upload is not supported with this LLM provider";
/// A Responses model whose vector store is disabled (`LV:104-106`).
pub const ADD_UNSUPPORTED_PROVIDER: &str = "Adding to vector store is not supported with this LLM provider";
pub const STORE_NOT_ENABLED: &str = "The vector store has not been enabled in settings";
pub const NO_STORE_PROVIDER: &str = "No vector store is available for this LLM provider.";
/// Termurion's refusal for a model whose own host is not api.openai.com (Decision: iTerm2 sends
/// that model's key, custom headers or Azure api-key to OpenAI along with the file).
pub const OPENAI_MODELS_ONLY: &str = "Files can be sent to OpenAI only with a model at api.openai.com, so that another server\u{2019}s key or headers never reach OpenAI.";

/// An error envelope in the answer to adding files (`LV:152-154`).
pub fn add_failed_detail(message: &str) -> String {
    format!("Failed to add files to vector store: {message}")
}

/// `createMultipartBody` (`HH:18-46`): each field, then the file part named "file", then the
/// closing boundary. A quote, CR or LF in the file name is dropped so it cannot end the part's
/// header (iTerm2 writes the name as is).
pub fn multipart(boundary: &str, fields: &[(&str, &str)], file_name: &str, mime: &str, content: &[u8]) -> Vec<u8> {
    let file_name: String = file_name.chars().filter(|c| !matches!(c, '"' | '\r' | '\n')).collect();
    let mut body = Vec::new();
    for (name, value) in fields {
        body.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n").as_bytes());
    }
    body.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{file_name}\"\r\nContent-Type: {mime}\r\n\r\n").as_bytes());
    body.extend_from_slice(content);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    body
}

/// `openAIExtensionToMime` (`ExtensionToMime.swift:9-31`): the file types OpenAI's file search
/// takes.
const UPLOAD_MIMES: [(&str, &str); 21] = [
    ("c", "text/x-c"),
    ("cpp", "text/x-c++"),
    ("cs", "text/x-csharp"),
    ("css", "text/css"),
    ("doc", "application/msword"),
    ("docx", "application/vnd.openxmlformats-officedocument.wordprocessingml.document"),
    ("go", "text/x-golang"),
    ("html", "text/html"),
    ("java", "text/x-java"),
    ("js", "text/javascript"),
    ("json", "application/json"),
    ("md", "text/markdown"),
    ("pdf", "application/pdf"),
    ("php", "text/x-php"),
    ("pptx", "application/vnd.openxmlformats-officedocument.presentationml.presentation"),
    ("py", "text/x-python"),
    ("rb", "text/x-ruby"),
    ("sh", "application/x-sh"),
    ("tex", "text/x-tex"),
    ("ts", "application/typescript"),
    ("txt", "text/plain"),
];

/// The MIME type of the lower-cased extension (`LF:64-70`), else "application/octet-stream".
pub fn upload_mime(file_name: &str) -> &'static str {
    let ext = std::path::Path::new(file_name).extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    UPLOAD_MIMES.iter().find(|(e, _)| *e == ext).map_or("application/octet-stream", |(_, m)| m)
}

/// What a `FileRunner` does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Job {
    CreateStore { name: String },
    Upload { name: String, content: Vec<u8> },
    AddFiles { ids: Vec<String>, store: String },
}

/// What a `FileRunner` reports.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileOut {
    /// The new vector store's id.
    Created(String),
    /// The uploaded file's id.
    Uploaded(String),
    /// The files were added and ingested.
    Added,
    Failed(String),
    /// Ask the person for this vendor's key, then call `start` again with it.
    NeedKey(Vendor),
}

/// The request that is out.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Step {
    Create,
    Upload,
    Add,
    Poll,
}

enum State {
    Ready,
    Sending { call: Box<dyn Call>, step: Step, url: String },
    /// The batch is still being ingested; it is asked again at `due`.
    Waiting { due: Instant },
    Done,
}

/// One file job (`T:706-920`): the request, its answer and, for adding files, the polls while
/// OpenAI ingests them. `poll` drives it from the loop, `next_due` names the next poll.
pub struct FileRunner {
    model: Model,
    job: Job,
    transport: Arc<dyn Transport>,
    options: Options,
    notify: Notify,
    key: Option<Secret>,
    /// The file batch being ingested.
    batch: Option<String>,
    state: State,
}

/// The decode failure of an answer that is not the shape expected.
fn decode_error(e: impl std::fmt::Display) -> String {
    could_not_decode(&e.to_string())
}

/// A string field of a JSON object answer.
fn field(body: &str, name: &str) -> Result<String, String> {
    let v: serde_json::Value = serde_json::from_str(body).map_err(decode_error)?;
    v.get(name).and_then(|f| f.as_str()).map(str::to_string).ok_or_else(|| decode_error(format!("missing \u{201c}{name}\u{201d}")))
}

impl FileRunner {
    pub fn new(model: Model, job: Job, transport: Arc<dyn Transport>, options: Options, notify: Notify) -> FileRunner {
        FileRunner { model, job, transport, options, notify, key: None, batch: None, state: State::Ready }
    }

    /// Starts the job with the stored key; `NeedKey` asks for one, and `start` is called again
    /// with the answer. A runner starts once. Refusals that need no request come before the key
    /// (iTerm2 asks for the key first), so nobody is asked for a key a job would never send.
    pub fn start(&mut self, key: Option<&Secret>) -> Vec<FileOut> {
        if !matches!(self.state, State::Ready) {
            return Vec::new();
        }
        // The App Review key contacts nothing (Decision: iTerm2 would send it to OpenAI).
        if key.is_some_and(|k| k.expose() == REVIEW_KEY) {
            self.state = State::Done;
            return vec![FileOut::Failed(NO_KEY.to_string())];
        }
        let sent = match self.request() {
            Ok(sent) => sent,
            Err(text) => {
                self.state = State::Done;
                return vec![FileOut::Failed(text.to_string())];
            }
        };
        // Every file request goes to api.openai.com with the model's credentials, so only a
        // model at that host may send one (Decision; iTerm2 sends a gateway's or Azure's
        // credentials, or a private host's placeholder and the file, to OpenAI).
        if !ai_policy::host_is_openai(&self.model.url) {
            self.state = State::Done;
            return vec![FileOut::Failed(OPENAI_MODELS_ONLY.to_string())];
        }
        // iTerm2's `registration` (`T:200-219`): the given key when it is not blank. A model at
        // api.openai.com always takes the vendor key.
        self.key = key.filter(|k| !k.expose().trim_start().is_empty()).map(Secret::duplicate);
        if self.key.is_none() {
            return vec![FileOut::NeedKey(self.model.vendor)];
        }
        let (step, url, content_type, body) = sent;
        self.send(step, "POST", url, Some(content_type), body);
        Vec::new()
    }

    /// The job's request (step, URL, content type, body), or iTerm2's text for a job this model
    /// cannot do.
    fn request(&self) -> Result<(Step, String, String, Vec<u8>), &'static str> {
        let responses = self.model.api == Api::Responses;
        let openai_store = self.model.vector_store == VectorStore::OpenAI;
        match self.job.clone() {
            // `LV:165-185`.
            Job::CreateStore { name } if responses && openai_store => {
                let body = serde_json::json!({ "name": name }).to_string().into_bytes();
                Ok((Step::Create, STORES_URL.to_string(), "application/json".to_string(), body))
            }
            Job::CreateStore { .. } if ai_policy::host_is_openai(&self.model.url) => Err(STORE_NOT_ENABLED),
            Job::CreateStore { .. } => Err(NO_STORE_PROVIDER),
            // `LF:13-48`, `T:885-896`.
            Job::Upload { .. } if !responses => Err(UPLOAD_UNSUPPORTED),
            Job::Upload { .. } if !openai_store => Err(UPLOAD_UNSUPPORTED_PROVIDER),
            Job::Upload { name, content } => {
                let boundary = format!("Boundary-{}", uuid::Uuid::new_v4().to_string().to_uppercase());
                let body = multipart(&boundary, &[("purpose", "assistants")], &name, upload_mime(&name), &content);
                Ok((Step::Upload, FILES_URL.to_string(), format!("multipart/form-data; boundary={boundary}"), body))
            }
            // `LV:67-137`, `T:750-805`.
            Job::AddFiles { .. } if !responses => Err(UNSUPPORTED_STORES),
            Job::AddFiles { .. } if !openai_store => Err(ADD_UNSUPPORTED_PROVIDER),
            Job::AddFiles { ids, store } => {
                let body = serde_json::json!({ "file_ids": ids }).to_string().into_bytes();
                Ok((Step::Add, format!("{STORES_URL}/{store}/file_batches"), "application/json".to_string(), body))
            }
        }
    }

    /// The key's headers (`LLMAuthorizationProvider` on the model's URL and API), the content
    /// type when there is a body, then the model's custom headers (`LV:25-31`, `LF:74-78`).
    fn send(&mut self, step: Step, method: &'static str, url: String, content_type: Option<String>, body: Vec<u8>) {
        let key = self.key.as_ref().map_or("", |k| k.expose());
        let mut base = ai_policy::auth_headers(&self.model.url, self.model.api, key);
        if let Some(ct) = content_type {
            base.push(("Content-Type".to_string(), ct));
        }
        let headers = ai_policy::merged_headers(&base, &self.model.custom_headers);
        let request = HttpRequest { method, url: url.clone(), headers, body, stream: false };
        let call = self.transport.start(request, self.options.clone(), self.notify.clone());
        self.state = State::Sending { call, step, url };
    }

    /// Feeds the answer that arrived and sends a poll that is due; the outputs once the job
    /// ended.
    pub fn poll(&mut self, now: Instant) -> Vec<FileOut> {
        loop {
            match &mut self.state {
                State::Waiting { due } if *due <= now => {
                    // `LV:8-47`: GET with the key and the custom headers, no body.
                    let (Job::AddFiles { store, .. }, Some(batch)) = (&self.job, &self.batch) else { return Vec::new() };
                    let url = format!("{STORES_URL}/{store}/file_batches/{batch}");
                    self.send(Step::Poll, "GET", url, None, Vec::new());
                }
                State::Sending { call, step, url } => match call.try_event() {
                    None => return Vec::new(),
                    Some(HttpEvent::Chunk(_)) => {}
                    Some(HttpEvent::Done { body, error }) => {
                        let (step, url) = (*step, url.clone());
                        self.state = State::Done;
                        if let Some(out) = self.answer(step, &url, &body, error.as_deref(), now) {
                            return vec![out];
                        }
                    }
                },
                _ => return Vec::new(),
            }
        }
    }

    /// One answer: the job's result, or None when a poll is scheduled.
    fn answer(&mut self, step: Step, url: &str, body: &str, error: Option<&str>, now: Instant) -> Option<FileOut> {
        if let Some(error) = error.filter(|e| !e.is_empty()) {
            // `providerError` (`T:360-388`) with the URL this request went to.
            let message = provider_error(display_name(&self.model), error, error_reason(body.as_bytes()).as_deref());
            return Some(FileOut::Failed(ai_policy::amended(&message, error, url, self.model.api, &self.model.custom_headers)));
        }
        let result = match step {
            Step::Create => field(body, "id").map(FileOut::Created),
            Step::Upload => field(body, "id").map(FileOut::Uploaded),
            Step::Add => {
                // An error envelope wins (`LV:150-157`).
                let v: serde_json::Value = serde_json::from_str(body).unwrap_or_default();
                if let Some(message) = v["error"]["message"].as_str() {
                    return Some(FileOut::Failed(add_failed_detail(message)));
                }
                match field(body, "status").as_deref() {
                    Ok("completed") => Ok(FileOut::Added),
                    Ok("in_progress") => match field(body, "id") {
                        Ok(batch) => {
                            self.batch = Some(batch);
                            self.state = State::Waiting { due: now + POLL_EVERY };
                            return None;
                        }
                        Err(e) => Err(e),
                    },
                    Ok("cancelled" | "failed") => Ok(FileOut::Failed(ADD_FAILED.to_string())),
                    Ok(other) => Err(decode_error(format!("unknown status \u{201c}{other}\u{201d}"))),
                    Err(e) => Err(e.clone()),
                }
            }
            // `T:815-880`: the chat shows `NOT_WELL_FORMED` beside the ingest error (12c).
            Step::Poll => match field(body, "status").as_deref() {
                Ok("completed") => Ok(FileOut::Added),
                Ok("in_progress") => {
                    // Asked again without limit, as iTerm2 does.
                    self.state = State::Waiting { due: now + POLL_EVERY };
                    return None;
                }
                Ok("cancelled" | "failed") => Ok(FileOut::Failed(INGEST_ERROR.to_string())),
                Ok(other) => Err(decode_error(format!("unknown status \u{201c}{other}\u{201d}"))),
                Err(e) => Err(e.clone()),
            },
        };
        Some(result.unwrap_or_else(FileOut::Failed))
    }

    /// When the next poll is due.
    pub fn next_due(&self) -> Option<Instant> {
        match &self.state {
            State::Waiting { due, .. } => Some(*due),
            _ => None,
        }
    }

    /// Stops the job: the call is cancelled and nothing more is sent or reported.
    pub fn cancel(&mut self) {
        if let State::Sending { call, .. } = &mut self.state {
            call.cancel();
        }
        self.state = State::Done;
    }
}

impl Drop for FileRunner {
    fn drop(&mut self) {
        self.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai_model::fixture;
    use crate::ai_transport::{Proxy, Recorded, Reply};

    fn options() -> Options {
        Options { idle: Duration::from_secs(1200), proxy: Proxy::System }
    }

    fn noop() -> Notify {
        Arc::new(|| {})
    }

    fn header<'a>(r: &'a HttpRequest, name: &str) -> Option<&'a str> {
        r.headers.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }

    /// A Responses model at api.openai.com with OpenAI's vector store and one custom header.
    fn store_model() -> Model {
        let mut m = fixture("gpt-4.1", "https://api.openai.com/v1/responses", Api::Responses);
        m.vector_store = VectorStore::OpenAI;
        m.custom_headers = vec![("X-Team".to_string(), "blue".to_string())];
        m
    }

    fn key() -> Secret {
        Secret::new("sk-a".to_string())
    }

    /// Runs `job` against `replies`: start, then polls every 100 ms of a fake clock until the
    /// runner reports; answers the outputs, the requests and the times at which it polled.
    fn run(model: Model, job: Job, replies: Vec<Reply>) -> (Vec<FileOut>, Vec<HttpRequest>, Vec<Duration>) {
        let t = Arc::new(Recorded::new(replies));
        let mut r = FileRunner::new(model, job, t.clone(), options(), noop());
        let mut out = r.start(Some(&key()));
        let t0 = Instant::now();
        let mut dues = Vec::new();
        let mut step = 0;
        while out.is_empty() && step < 100 {
            let now = t0 + Duration::from_millis(100 * step);
            out = r.poll(now);
            if let Some(due) = r.next_due() {
                if dues.last() != Some(&(due - t0)) {
                    dues.push(due - t0);
                }
            }
            step += 1;
        }
        (out, t.requests(), dues)
    }

    #[test]
    fn multipart_follows_iterm2() {
        let body = multipart("B", &[("purpose", "assistants")], "a.txt", "text/plain", b"hi");
        assert_eq!(
            String::from_utf8(body).unwrap(),
            "--B\r\nContent-Disposition: form-data; name=\"purpose\"\r\n\r\nassistants\r\n--B\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a.txt\"\r\nContent-Type: text/plain\r\n\r\nhi\r\n--B--\r\n"
        );
        assert_eq!(upload_mime("X.PDF"), "application/pdf");
        assert_eq!(upload_mime("a.unknownext"), "application/octet-stream");
        assert_eq!(upload_mime("notes.md"), "text/markdown");
        assert_eq!(upload_mime("Makefile"), "application/octet-stream");
        // A quote, CR or LF cannot end the part's header.
        let body = multipart("B", &[], "a\"b\r\nX: y.txt", "text/plain", b"");
        assert!(String::from_utf8(body).unwrap().contains("filename=\"abX: y.txt\"\r\n"));
    }

    #[test]
    fn file_jobs_only_for_openai_models() {
        let jobs = || [Job::CreateStore { name: "chat".into() }, Job::Upload { name: "a.txt".into(), content: b"secret".to_vec() }, Job::AddFiles { ids: vec!["f".into()], store: "vs_1".into() }];
        // A gateway with its own credential header, an Azure deployment (api-key) and a private
        // host (the placeholder): each refuses before any request and asks for no key.
        let mut gateway = store_model();
        gateway.url = "https://gateway.example/v1/responses".into();
        gateway.custom_headers = vec![("Authorization".to_string(), "Bearer gateway-secret".to_string())];
        let mut azure = store_model();
        azure.url = "https://team.openai.azure.com/openai/v1/responses".into();
        let mut local = store_model();
        local.url = "http://localhost:1234/v1/responses".into();
        for model in [gateway, azure, local] {
            for job in jobs() {
                for key in [None, Some(key())] {
                    let t = Arc::new(Recorded::new(vec![Reply::ok(&[br#"{"id":"x"}"#])]));
                    let mut r = FileRunner::new(model.clone(), job.clone(), t.clone(), options(), noop());
                    assert_eq!(r.start(key.as_ref()), [FileOut::Failed(OPENAI_MODELS_ONLY.into())], "{} {job:?}", model.url);
                    assert_eq!(r.poll(Instant::now()), []);
                    assert!(t.requests().is_empty(), "{} sent nothing", model.url);
                }
            }
        }
        // iTerm2's own refusals still come first.
        let mut local_chat = fixture("m", "http://localhost:1234/v1/chat/completions", Api::ChatCompletions);
        local_chat.vector_store = VectorStore::OpenAI;
        let (out, reqs, _) = run(local_chat, Job::Upload { name: "a.txt".into(), content: b"x".to_vec() }, vec![]);
        assert_eq!((out, reqs.len()), (vec![FileOut::Failed(UPLOAD_UNSUPPORTED.into())], 0));
        // A model at api.openai.com still sends, with its key and headers.
        for job in jobs() {
            let (out, reqs, _) = run(store_model(), job, vec![Reply::ok(&[br#"{"id":"x","status":"completed"}"#])]);
            assert_eq!(out.len(), 1);
            assert!(!matches!(out[0], FileOut::Failed(_)), "{out:?}");
            assert_eq!(reqs.len(), 1);
            assert!(ai_policy::host_is_openai(&reqs[0].url));
            assert!(header(&reqs[0], "Authorization") == Some("Bearer sk-a"), "Authorization carries the key");
        }
    }

    #[test]
    fn file_jobs_follow_iterm2() {
        // Create a store.
        let (out, reqs, _) = run(store_model(), Job::CreateStore { name: "chat".into() }, vec![Reply::ok(&[br#"{"id":"vs_1","object":"vector_store","name":"chat","created_at":1}"#])]);
        assert_eq!(out, [FileOut::Created("vs_1".into())]);
        assert_eq!(reqs.len(), 1);
        assert_eq!((reqs[0].method, reqs[0].url.as_str()), ("POST", STORES_URL));
        assert_eq!(reqs[0].body, br#"{"name":"chat"}"#);
        assert!(!reqs[0].stream);
        assert!(header(&reqs[0], "Authorization") == Some("Bearer sk-a"), "Authorization carries the key");
        assert_eq!(header(&reqs[0], "Content-Type"), Some("application/json"));
        assert_eq!(header(&reqs[0], "X-Team"), Some("blue"));

        // Upload a file.
        let (out, reqs, _) = run(store_model(), Job::Upload { name: "notes.md".into(), content: b"# hi".to_vec() }, vec![Reply::ok(&[br#"{"id":"file_1","object":"file"}"#])]);
        assert_eq!(out, [FileOut::Uploaded("file_1".into())]);
        assert_eq!((reqs[0].method, reqs[0].url.as_str()), ("POST", FILES_URL));
        let content_type = header(&reqs[0], "Content-Type").expect("a content type").to_string();
        let boundary = content_type.strip_prefix("multipart/form-data; boundary=").expect("multipart").to_string();
        let uuid = boundary.strip_prefix("Boundary-").expect("Boundary-<UUID>");
        assert_eq!(uuid.len(), 36);
        assert_eq!(uuid, uuid.to_uppercase(), "Foundation's UUID string is upper case");
        assert_eq!(reqs[0].body, multipart(&boundary, &[("purpose", "assistants")], "notes.md", "text/markdown", b"# hi"));
        assert!(header(&reqs[0], "Authorization") == Some("Bearer sk-a"), "Authorization carries the key");
        // The source merges the model's custom headers into the upload too (`LF:74-78`).
        assert_eq!(header(&reqs[0], "X-Team"), Some("blue"));

        // Add files: completed at once.
        let add = || Job::AddFiles { ids: vec!["file_1".into()], store: "vs_1".into() };
        let batches = format!("{STORES_URL}/vs_1/file_batches");
        let (out, reqs, _) = run(store_model(), add(), vec![Reply::ok(&[br#"{"id":"vsfb_1","status":"completed"}"#])]);
        assert_eq!(out, [FileOut::Added]);
        assert_eq!((reqs[0].method, reqs[0].url.as_str()), ("POST", batches.as_str()));
        assert_eq!(reqs[0].body, br#"{"file_ids":["file_1"]}"#);
        assert_eq!(header(&reqs[0], "Content-Type"), Some("application/json"));

        // In progress: polled every 500 ms until it completes.
        let in_progress: &[u8] = br#"{"id":"vsfb_1","status":"in_progress"}"#;
        let (out, reqs, dues) = run(
            store_model(),
            add(),
            vec![Reply::ok(&[in_progress]), Reply::ok(&[br#"{"status":"in_progress"}"#]), Reply::ok(&[br#"{"status":"completed"}"#])],
        );
        assert_eq!(out, [FileOut::Added]);
        assert_eq!(reqs.len(), 3);
        assert_eq!(dues, [Duration::from_millis(500), Duration::from_millis(1000)], "each poll is due 500 ms after the last answer");
        for poll in &reqs[1..] {
            assert_eq!((poll.method, poll.url.as_str()), ("GET", format!("{batches}/vsfb_1").as_str()));
            assert!(poll.body.is_empty());
            assert!(header(poll, "Authorization") == Some("Bearer sk-a"), "the poll carries the key");
            assert_eq!(header(poll, "X-Team"), Some("blue"), "the poll carries the model's custom headers");
            assert_eq!(header(poll, "Content-Type"), None);
        }

        // Failed or cancelled on the first answer, and while polling.
        for status in ["failed", "cancelled"] {
            let first = format!(r#"{{"id":"vsfb_1","status":"{status}"}}"#);
            let (out, _, _) = run(store_model(), add(), vec![Reply::ok(&[first.as_bytes()])]);
            assert_eq!(out, [FileOut::Failed(ADD_FAILED.into())]);
            let later = format!(r#"{{"status":"{status}"}}"#);
            let (out, _, _) = run(store_model(), add(), vec![Reply::ok(&[in_progress]), Reply::ok(&[later.as_bytes()])]);
            assert_eq!(out, [FileOut::Failed(INGEST_ERROR.into())]);
        }
        let (out, _, _) = run(store_model(), add(), vec![Reply::ok(&[br#"{"error":{"message":"no such store"}}"#])]);
        assert_eq!(out, [FileOut::Failed(add_failed_detail("no such store"))]);

        // A refusal reads as iTerm2's provider error.
        let (out, _, _) = run(store_model(), add(), vec![Reply::status(401, br#"{"error":{"message":"Incorrect API key provided"}}"#)]);
        assert_eq!(out, [FileOut::Failed("Error from OpenAI: HTTP request failed with status 401. Incorrect API key provided".into())]);

        // Vendors without vector stores send nothing.
        let mut chat = fixture("gpt-4o", "https://api.openai.com/v1/chat/completions", Api::ChatCompletions);
        let (out, reqs, _) = run(chat.clone(), add(), vec![]);
        assert_eq!((out, reqs.len()), (vec![FileOut::Failed(UNSUPPORTED_STORES.into())], 0));
        let (out, _, _) = run(chat.clone(), Job::Upload { name: "a.txt".into(), content: b"x".to_vec() }, vec![]);
        assert_eq!(out, [FileOut::Failed(UPLOAD_UNSUPPORTED.into())]);
        chat.vector_store = VectorStore::OpenAI;
        let (out, _, _) = run(chat, Job::CreateStore { name: "chat".into() }, vec![]);
        assert_eq!(out, [FileOut::Failed(STORE_NOT_ENABLED.into())], "an OpenAI host names the setting");
        let mut off = store_model();
        off.vector_store = VectorStore::Disabled;
        let (out, _, _) = run(off.clone(), Job::Upload { name: "a.txt".into(), content: b"x".to_vec() }, vec![]);
        assert_eq!(out, [FileOut::Failed(UPLOAD_UNSUPPORTED_PROVIDER.into())]);
        let (out, _, _) = run(off.clone(), add(), vec![]);
        assert_eq!(out, [FileOut::Failed(ADD_UNSUPPORTED_PROVIDER.into())]);
        let (out, _, _) = run(off, Job::CreateStore { name: "chat".into() }, vec![]);
        assert_eq!(out, [FileOut::Failed(STORE_NOT_ENABLED.into())]);
        let claude = fixture("claude", "https://api.anthropic.com/v1/messages", Api::Anthropic);
        let (out, _, _) = run(claude, Job::CreateStore { name: "chat".into() }, vec![]);
        assert_eq!(out, [FileOut::Failed(NO_STORE_PROVIDER.into())]);
    }

    #[test]
    fn file_keys_follow_iterm2() {
        let t = Arc::new(Recorded::new(vec![Reply::ok(&[br#"{"id":"vs_2"}"#])]));
        let mut r = FileRunner::new(store_model(), Job::CreateStore { name: "chat".into() }, t.clone(), options(), noop());
        assert_eq!(r.start(None), [FileOut::NeedKey(Vendor::OpenAI)]);
        assert_eq!(r.start(Some(&Secret::new("  ".into()))), [FileOut::NeedKey(Vendor::OpenAI)], "a blank key is no key");
        assert!(t.requests().is_empty());
        assert_eq!(r.start(Some(&key())), []);
        assert_eq!(r.poll(Instant::now()), [FileOut::Created("vs_2".into())]);
        assert_eq!(r.start(Some(&key())), [], "a runner starts once");

        // The App Review key contacts nothing.
        let t = Arc::new(Recorded::new(vec![]));
        let mut r = FileRunner::new(store_model(), Job::Upload { name: "a.txt".into(), content: b"x".to_vec() }, t.clone(), options(), noop());
        assert_eq!(r.start(Some(&Secret::new(REVIEW_KEY.into()))), [FileOut::Failed(NO_KEY.into())]);
        assert!(t.requests().is_empty());

        // Cancel stops the call and anything due.
        let t = Arc::new(Recorded::new(vec![Reply::ok(&[br#"{"id":"vsfb_1","status":"in_progress"}"#])]));
        let mut r = FileRunner::new(store_model(), Job::AddFiles { ids: vec!["f".into()], store: "vs_1".into() }, t.clone(), options(), noop());
        r.start(Some(&key()));
        let now = Instant::now();
        assert_eq!(r.poll(now), []);
        assert_eq!(r.next_due(), Some(now + POLL_EVERY));
        r.cancel();
        assert_eq!(r.next_due(), None);
        assert_eq!(r.poll(now + Duration::from_secs(5)), []);
        assert_eq!(t.requests().len(), 1, "nothing is sent after a cancel");
    }
}
