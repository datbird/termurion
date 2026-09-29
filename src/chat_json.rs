//! Readers for the Swift JSON that 12c keeps opaque: a message's attachments and the watcher
//! event (`LLM.swift:106-160`, `Message.swift:93-130`). Pure. The status enum is 12b's
//! `ai_msg::Status` (ruling F7), not a second copy.

use base64::Engine;
use serde_json::Value;

pub use crate::ai_msg::Status;
use crate::chat_message::{one_key, status_of};

/// What an attachment carries.
#[derive(Clone, Debug, PartialEq)]
pub enum Attachment {
    Code(String),
    Status(Status),
    File { name: String, content: Vec<u8>, mime: String },
    FileId { id: String, name: String },
}

/// An attachment object's `id` and what it carries; None for a type this build does not know
/// or a body that does not read.
pub fn attachment(v: &Value) -> Option<(String, Attachment)> {
    let id = v.get("id")?.as_str()?.to_string();
    let (kind, body) = one_key(v.get("type")?)?;
    let text = |b: &Value, key: &str| b.get(key).and_then(Value::as_str).map(str::to_string);
    let found = match kind {
        "code" => Attachment::Code(text(body, "_0")?),
        "statusUpdate" => Attachment::Status(status(body.get("_0")?)?),
        "file" => {
            let f = body.get("_0")?;
            let content = base64::engine::general_purpose::STANDARD.decode(text(f, "content")?).ok()?;
            Attachment::File { name: text(f, "name")?, content, mime: text(f, "mimeType")? }
        }
        "fileID" => Attachment::FileId { id: text(body, "id")?, name: text(body, "name")? },
        _ => return None,
    };
    Some((id, found))
}

/// A file attachment object in the Swift shape (`LLM.Message.Attachment` with `.file`): what
/// `attachment` reads back. The input builds these for a message it sends.
pub fn file_attachment(id: &str, name: &str, content: &[u8], mime: &str) -> Value {
    let content = base64::engine::general_purpose::STANDARD.encode(content);
    serde_json::json!({"inline": false, "id": id, "type": {"file": {"_0": {"name": name, "content": content, "mimeType": mime}}}})
}

/// A status update (`StatusUpdate`).
pub fn status(v: &Value) -> Option<Status> {
    status_of(v)
}

/// A watcher event's case body `{"_0": {reason, detail, ..}}` (ruling F9): whether a timer
/// fired, and the detail.
pub fn watcher(v: &Value) -> Option<(bool, String)> {
    let event = v.get("_0")?;
    let reason = event.get("reason")?.as_str()?;
    let detail = event.get("detail")?.as_str()?.to_string();
    Some((reason == "timerFired", detail))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn swift_json_reads_follow_iterm2() {
        assert_eq!(attachment(&json!({"inline":false,"id":"ci_1","type":{"code":{"_0":"print(1)"}}})), Some(("ci_1".to_string(), Attachment::Code("print(1)".into()))));
        let update = |body: Value| attachment(&json!({"inline":true,"id":"w","type":{"statusUpdate":{"_0":body}}})).map(|(_, a)| a);
        assert_eq!(update(json!({"webSearchFinished":{"_0":"rust"}})), Some(Attachment::Status(Status::WebSearchFinished(Some("rust".into())))));
        assert_eq!(update(json!({"webSearchStarted":{}})), Some(Attachment::Status(Status::WebSearchStarted)));
        assert_eq!(
            update(json!({"multipart":{"_0":[{"webSearchStarted":{}},{"codeInterpreterFinished":{}}]}})),
            Some(Attachment::Status(Status::Multipart(vec![Status::WebSearchStarted, Status::CodeInterpreterFinished])))
        );
        assert_eq!(status(&json!({"reasoningSummaryUpdate":{"_0":"thinking"}})), Some(Status::ReasoningSummary("thinking".into())));
        assert_eq!(status(&json!({"nope":{}})), None);
        let file = attachment(&json!({"inline":false,"id":"f","type":{"file":{"_0":{"name":"a.txt","content":"aGk=","mimeType":"text/plain"}}}}));
        assert_eq!(file, Some(("f".to_string(), Attachment::File { name: "a.txt".into(), content: b"hi".to_vec(), mime: "text/plain".into() })));
        assert_eq!(attachment(&json!({"inline":false,"id":"f","type":{"file":{"_0":{"name":"a.txt","content":"!!","mimeType":"text/plain"}}}})), None, "bad base64");
        assert_eq!(attachment(&json!({"inline":false,"id":"x","type":{"fileID":{"id":"f1","name":"a.pdf"}}})), Some(("x".to_string(), Attachment::FileId { id: "f1".into(), name: "a.pdf".into() })));
        assert_eq!(attachment(&json!({"inline":false,"id":"x","type":{"mystery":{}}})), None);
        assert_eq!(attachment(&json!({"inline":false,"type":{"code":{"_0":"x"}}})), None, "no id");
        // The writer is the reader's inverse and exactly the Swift shape.
        assert_eq!(
            file_attachment("A", "a.txt", b"hi", "text/plain"),
            json!({"inline":false,"id":"A","type":{"file":{"_0":{"name":"a.txt","content":"aGk=","mimeType":"text/plain"}}}})
        );
        assert_eq!(attachment(&file_attachment("B", "b.bin", &[0, 255, 7], "application/octet-stream")), Some(("B".to_string(), Attachment::File { name: "b.bin".into(), content: vec![0, 255, 7], mime: "application/octet-stream".into() })));
        // The case body wraps the event (ruling F9).
        assert_eq!(watcher(&json!({"_0":{"reason":"timerFired","detail":"d","watcherID":"w"}})), Some((true, "d".to_string())));
        assert_eq!(watcher(&json!({"_0":{"reason":"stateReached","detail":"d"}})), Some((false, "d".to_string())));
        assert_eq!(watcher(&json!({"reason":"timerFired","detail":"d"})), None, "the event is under _0");
    }
}
