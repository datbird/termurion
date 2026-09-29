//! The conversation of the AI Chat window (`ChatViewController`, `ChatViewControllerModel`,
//! `DateCellView`, `MessageCellView`, `MarkdownHelpers`, `ChatWindowController` fork): which
//! messages show, the date rows, the text or parts each message draws, code blocks for Copy,
//! the fork title and clones, and the attachment files a click writes. Pure, except
//! `materialize`, which uses std::fs only.
//!
//! Everything a model or an imported database wrote is untrusted here: a name reaching a
//! markdown body goes through `consent_field` (ruling F20), a link opens only when
//! `web_link` accepts it (ruling F18), an attachment opens only when `openable` says its type
//! is inert (ruling F19), and a file is written only through `materialize` (ruling R12cd-6).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::ai_msg::Status;
use crate::chat_json::{self, Attachment};
use crate::ai_tools::{consent_field, Field};
use crate::chat_message::{hidden, one_key, ClientLocal, Content, Message, Participant, Payload, Streaming, Subpart, UNSUPPORTED_TEXT};
use crate::porthole::render::{Block, Doc};
use crate::stamps::Parts;

/// A row of the conversation: a date, the typing indicator, or the visible message at that
/// index (`ChatViewControllerModel.Item`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Item {
    Date(String),
    Typing,
    Message(usize),
}

/// The cell class a message gets (`CVC:1503-1542`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Style {
    Regular,
    System,
    Multipart,
    Command,
}

/// What a bubble draws.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Body {
    Markdown(String),
    Plain(String),
    Command { command: String, url: String },
    Parts(Vec<Part>),
}

/// One subpart of a multipart message. A context part is never a part (ruling F8).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Part {
    Text(Body),
    Code(String),
    /// A status line, in markdown.
    Status(String),
    /// A file attachment with its bytes in the message: `id` is the attachment's.
    File { id: String, name: String },
    /// A `fileID` attachment: the bytes are not in the message, so nothing can be written.
    RemoteFile { name: String },
}

pub const MAX_BUBBLE: f32 = 0.7;
pub const COPIED: &str = "Copied";
pub const COPIED_FOR: Duration = Duration::from_secs(1);
pub const WARNING_GLYPH: char = '\u{E7BA}';
pub const TIMER_GLYPH: char = '\u{E823}';
pub const WATCHER_GLYPH: char = '\u{E701}';
pub const COMMAND_GLYPH: char = '\u{E7F4}';
pub const FILE_GLYPH: char = '\u{E8A5}';
pub const COPY_GLYPH: char = '\u{E8C8}';
pub const CODE_INTERPRETER: &str = "Code Interpreter";
/// Under `%TEMP%` (ruling R12cd-6).
pub const ATTACHMENTS_DIR: &str = "TandemChatAttachments";
/// A code attachment's file name (`CVC:2207`).
pub const CODE_FILE: &str = "code.txt";
pub const REVEAL_TITLE: &str = "Reveal in Explorer";
pub const OPEN_TITLE: &str = "Open";
/// The message menu's ids are `MENU_PREFIX<verb>:<message id>`.
pub const MENU_PREFIX: &str = "aichat:msg:";

/// The messages the conversation shows: hidden ones and requests already answered left out
/// (`CVCM:140-170`); the two append kinds never reach a model of the conversation.
pub fn visible(messages: &[Message]) -> Vec<&Message> {
    let answered: HashSet<&str> = messages
        .iter()
        .filter_map(|m| match &m.content {
            Content::RemoteCommandResponse { request_id, .. } => Some(request_id.as_str()),
            _ => None,
        })
        .collect();
    messages
        .iter()
        .filter(|m| {
            !hidden(&m.content)
                && !matches!(m.content, Content::Append { .. } | Content::AppendAttachment { .. })
                && !(matches!(m.content, Content::RemoteCommandRequest { .. }) && answered.contains(m.unique_id.as_str()))
        })
        .collect()
}

/// The rows: a date before each new day (the first message's too), the typing row last.
pub fn items(visible: &[&Message], typing: bool, day: &dyn Fn(f64) -> (i32, u32, u32), label: &dyn Fn(f64) -> String) -> Vec<Item> {
    let mut out = Vec::new();
    let mut last = None;
    for (i, m) in visible.iter().enumerate() {
        let d = day(m.sent_date);
        if last != Some(d) {
            out.push(Item::Date(label(m.sent_date)));
            last = Some(d);
        }
        out.push(Item::Message(i));
    }
    if typing {
        out.push(Item::Typing);
    }
    out
}

/// Days since 1970-01-01 of a civil date (proleptic Gregorian).
fn civil_days(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Calendar days from `from` to `to` (negative when `to` is earlier).
pub fn days_between(from: &Parts, to: &Parts) -> i64 {
    civil_days(to.year.into(), to.month.into(), to.day.into()) - civil_days(from.year.into(), from.month.into(), from.day.into())
}

/// The date row's text for a message sent at `sent`, seen at `now`.
pub fn date_text(sent: &Parts, now: &Parts) -> String {
    date_label(sent, days_between(sent, now), sent.year == now.year)
}

/// `humanReadableDate` (`DCV:92-120`); English, as the ages are.
pub fn date_label(date: &Parts, days_ago: i64, same_year: bool) -> String {
    const DAYS: [&str; 7] = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let month = MONTHS.get((date.month as usize).wrapping_sub(1)).copied().unwrap_or("?");
    match days_ago {
        0 => "Today".to_string(),
        1 => "Yesterday".to_string(),
        2..=6 => DAYS.get(date.weekday as usize).copied().unwrap_or("?").to_string(),
        d if d >= 7 && same_year => format!("{month} {}", date.day),
        _ => format!("{month} {}, {}", date.day, date.year),
    }
}

pub fn style(content: &Content) -> Style {
    match content {
        Content::TerminalCommand(_) => Style::Command,
        Content::ClientLocal(_) | Content::WatcherEvent(_) | Content::Unsupported(_) => Style::System,
        Content::Multipart { .. } => Style::Multipart,
        _ => Style::Regular,
    }
}

const VECTOR_STORE_TEXT: &str = "A vector store was created";
const WAITING_TEXT: &str = "Waiting for a session to be selected\u{2026}";
const STREAM_ACTIVE: &str = "All terminal commands in the linked session will be sent to AI automatically.";
const STREAM_STOPPED: &str = "Terminal commands will no longer be sent to AI automatically.";
const STREAM_STOPPED_AUTO: &str = "Terminal commands will no longer be sent to AI automatically. Automatic sending always terminates when Tandem restarts or the current chat changes.";
const PERMISSIONS_HELP: &str = "You can use these buttons or the info button menu at the top of the chat window to control AI permissions for this chat.";
const ORCHESTRATION_OFFER: &str = "**Enable orchestration for this chat?**\n\n**Orchestration** lets the AI coordinate across multiple terminal sessions. It can read the contents of any session, and you grant a one-time approval before it controls a session instead of approving every call. It also runs in **auto mode**: each command the AI proposes is checked for safety by your AI provider, and anything risky is held for your review.";
const ENABLE_ORCHESTRATION: &str = "**Enable orchestration?**\n\nOrchestration mode lets the agent read screen contents from any session. To type into a session still requires your permission.\n\nThis is a more permissive model than when an agent is linked to a single session, where there are very fine-grained permission settings.\n\nEnabling will detach any linked terminal or browser session and switch the chat to Orchestration mode.";
const NEEDS_BROWSER: &str = "The AI agent needs to run commands in a live web browser session, but none is attached to this chat.";
/// The orchestration ids that pick a prompt's heading (`WorkgroupIntrospection.swift`).
const SPAWN_ID: &str = "spawn";
const COMMAND_APPROVAL_ID: &str = "command-approval";
const WATCH_APPROVAL_ID: &str = "watch-approval";
const SYNTHETIC_PREFIX: &str = "session:";

/// A field written by someone else, made running text that opens no link or span. A line
/// break stays one, so a multi-line summary keeps its shape.
fn field(s: &str) -> String {
    s.split('\n').map(|line| consent_field(line, Field::Text)).collect::<Vec<_>>().join("\n")
}

/// `CVC:2689-2743`, the orchestration prompts iTerm2 carries as client-local actions. Their
/// buttons are disabled until milestone 12f; the text is the same.
fn orchestration_body(v: &serde_json::Value) -> String {
    let Some((kind, b)) = one_key(v) else { return String::new() };
    let text = |k: &str| b.get(k).and_then(serde_json::Value::as_str).unwrap_or_default();
    match kind {
        "workgroupPermissionRequest" => {
            let (id, name, summary) = (text("workgroupID"), field(text("workgroupName")), field(text("summary")));
            match id {
                SPAWN_ID => format!("**Open a new session?**\n\n{summary}"),
                COMMAND_APPROVAL_ID => format!("**Run this command?**\n\n{summary}"),
                WATCH_APPROVAL_ID => format!("**Allow repeated screen reads?**\n\n{summary}"),
                _ if id.starts_with(SYNTHETIC_PREFIX) => format!("**Allow agent to control session \u{201C}{name}\u{201D}?**\n\n{summary}"),
                _ => format!("**Allow agent to control workgroup \u{201C}{name}\u{201D}?**\n\n{summary}"),
            }
        }
        "enableOrchestrationRequest" => ENABLE_ORCHESTRATION.to_string(),
        "orchestrationPermissionGranted" => {
            format!("**Granted this chat permission to control \u{201C}{}\u{201D}.**\n\nYou @-mentioned it, so the agent can act there without asking. Revoke to require approval again.", field(text("name")))
        }
        _ => String::new(),
    }
}

/// The offer to link a session (`CVC:2665-2680`). The name is a terminal program's to set, so it
/// is escaped (ruling F20).
fn offer_link(terminal: bool, name: Option<&str>) -> String {
    let name = name.map_or_else(|| "Unnamed session".to_string(), |n| consent_field(n, Field::Text));
    let kind = if terminal { "terminal" } else { "browser" };
    format!(
        "**Link this chat to {kind} session \u{201C}{name}\u{201D}, or enable orchestration?**\n\nLinking gives the AI access to this {kind} session subject to your per-call permission. **Orchestration** is an alternative mode where the AI can coordinate multiple sessions. It uses automatic safety checking and one-time per-session approval instead of fine-grained permissions."
    )
}

/// What a message draws (`attributedStringValue`, `CVC:2542-2757`).
pub fn body(content: &Content) -> Body {
    let system = |s: &str| Body::Markdown(s.to_string());
    match content {
        Content::PlainText { text, .. } => Body::Plain(trim_leading(text).to_string()),
        Content::Markdown(text) => Body::Markdown(trim_leading(text).to_string()),
        // Filled by 12e Task 3 (ruling 12c-3).
        Content::ExplanationRequest(_) | Content::ExplanationResponse(_) => Body::Plain(UNSUPPORTED_TEXT.to_string()),
        Content::RemoteCommandRequest { payload: Payload::Classic(rc), safe } => Body::Markdown(crate::ai_permissions::parked_text(&rc.call.consent_description(), rc.call.category().title(), *safe)),
        Content::RemoteCommandRequest { payload: Payload::External(v), .. } => Body::Markdown(v.get("markdownDescription").and_then(serde_json::Value::as_str).unwrap_or_default().to_string()),
        Content::RemoteCommandResponse { result: Err(e), .. } => Body::Markdown(e.message.clone()),
        Content::RemoteCommandResponse { .. } => Body::Plain(String::new()),
        Content::WatcherEvent(v) => Body::Markdown(chat_json::watcher(v).map(|(_, detail)| detail).unwrap_or_default()),
        Content::Unsupported(_) => system(UNSUPPORTED_TEXT),
        Content::SelectSessionRequest { terminal, .. } => system(if *terminal { crate::ai_permissions::NEEDS_SESSION } else { NEEDS_BROWSER }),
        Content::ClientLocal(action) => match action {
            ClientLocal::PickingSession => system(WAITING_TEXT),
            ClientLocal::ExecutingCommand(rc) => Body::Markdown(rc.call.markdown_description()),
            ClientLocal::Notice(text) => Body::Plain(text.clone()),
            ClientLocal::StreamingChanged(Streaming::Active) => system(STREAM_ACTIVE),
            ClientLocal::StreamingChanged(Streaming::Stopped) => system(STREAM_STOPPED),
            ClientLocal::StreamingChanged(Streaming::StoppedAutomatically) => system(STREAM_STOPPED_AUTO),
            ClientLocal::OfferLink { terminal, name, .. } => Body::Markdown(offer_link(*terminal, name.as_deref())),
            ClientLocal::OfferOrchestration => system(ORCHESTRATION_OFFER),
            ClientLocal::Permissions { .. } => system(PERMISSIONS_HELP),
            ClientLocal::Other(v) => Body::Markdown(orchestration_body(v)),
        },
        Content::TerminalCommand(tc) => Body::Command { command: tc.command.clone(), url: tc.url.clone() },
        Content::Multipart { parts, .. } => Body::Parts(parts.iter().filter_map(part).collect()),
        // `CVC:2550-2560`: the fall-through kept.
        Content::RenameChat(_) | Content::Append { .. } | Content::AppendAttachment { .. } | Content::Commit(_) | Content::SetPermissions(_) | Content::VectorStoreCreated(_) | Content::UserCommand(_) => Body::Plain(VECTOR_STORE_TEXT.to_string()),
    }
}

/// One subpart; None for model-only context (ruling F8) and an attachment this build cannot read.
fn part(p: &Subpart) -> Option<Part> {
    Some(match p {
        Subpart::PlainText(t) => Part::Text(Body::Plain(t.clone())),
        Subpart::Markdown(t) => Part::Text(Body::Markdown(t.clone())),
        Subpart::Context(_) => return None,
        Subpart::Attachment(v) => match chat_json::attachment(v)? {
            (_, Attachment::Code(code)) => Part::Code(code),
            (_, Attachment::Status(s)) => Part::Status(status_text(&s)),
            (id, Attachment::File { name, .. }) => Part::File { id, name },
            (_, Attachment::FileId { name, .. }) => Part::RemoteFile { name },
        },
    })
}

/// A status update as markdown: the query of a finished search bold, its own characters
/// escaped (`displayMarkdownString`, ruling F7).
pub fn status_text(s: &Status) -> String {
    fn marked(s: &Status) -> Status {
        match s {
            Status::WebSearchFinished(Some(q)) => Status::WebSearchFinished(Some(format!("**{}**", consent_field(q, Field::Text)))),
            Status::Multipart(v) => Status::Multipart(v.iter().map(marked).collect()),
            other => other.clone(),
        }
    }
    marked(s).display()
}

/// The Segoe glyph drawn before a row's text.
pub fn glyph(content: &Content) -> Option<char> {
    match content {
        Content::RemoteCommandRequest { safe: Some(false), .. } => Some(WARNING_GLYPH),
        Content::WatcherEvent(v) => chat_json::watcher(v).map(|(timer, _)| if timer { TIMER_GLYPH } else { WATCHER_GLYPH }),
        _ => None,
    }
}

/// The content of a clone: every id it holds that names a cloned message follows the map.
fn clone_content(c: &Content, ids: &HashMap<String, String>) -> Content {
    let map = |id: &String| ids.get(id).unwrap_or(id).clone();
    match c {
        Content::RemoteCommandResponse { result, request_id, function_name, function_call_id } => {
            Content::RemoteCommandResponse { result: result.clone(), request_id: map(request_id), function_name: function_name.clone(), function_call_id: function_call_id.clone() }
        }
        Content::SelectSessionRequest { original, terminal } => {
            let mut inner = (**original).clone();
            if let Some(new) = ids.get(&inner.unique_id) {
                inner.unique_id = new.clone();
                inner.content = clone_content(&inner.content, ids);
                inner.first_blob_ref = None;
                inner.seq = 0;
            }
            Content::SelectSessionRequest { original: Box::new(inner), terminal: *terminal }
        }
        Content::Append { text, uuid } => Content::Append { text: text.clone(), uuid: map(uuid) },
        Content::AppendAttachment { attachment, uuid } => Content::AppendAttachment { attachment: attachment.clone(), uuid: map(uuid) },
        Content::Commit(uuid) => Content::Commit(map(uuid)),
        other => other.clone(),
    }
}

/// `Message.clone(_:messages:)` (`Message.swift:693-704`): a fresh id recorded in `ids`, the
/// ids inside the content mapped, and no blob reference (ruling F5: the fork's own blob copies
/// are linked after they exist).
pub fn clone_for_fork(m: &Message, ids: &mut HashMap<String, String>) -> Message {
    let fresh = crate::chat_message::new_id();
    ids.insert(m.unique_id.clone(), fresh.clone());
    Message { seq: 0, unique_id: fresh, content: clone_content(&m.content, ids), first_blob_ref: None, ..m.clone() }
}

/// `trimLeadingWhitespaceForDisplay` (`CVC:227-240`).
pub fn trim_leading(text: &str) -> &str {
    text.char_indices().find(|(_, c)| !c.is_whitespace()).map_or(text, |(i, _)| &text[i..])
}

/// Edit, Fork and Delete are offered for the person's own text (`MCV`).
pub fn editable(m: &Message) -> bool {
    m.author == Participant::User && matches!(m.content, Content::PlainText { .. })
}

pub fn message_menu(editable: bool) -> Vec<&'static str> {
    if editable {
        vec!["Edit", "Copy", "Fork", "Delete"]
    } else {
        vec!["Copy"]
    }
}

/// A run of code blocks Copy takes as one: block indexes into the `Doc`, both included.
#[derive(Clone, Debug, PartialEq)]
pub struct CodeBlock {
    pub first: usize,
    pub last: usize,
    pub text: String,
}

fn block_text(b: &Block) -> String {
    if b.rule {
        String::new()
    } else {
        b.runs.iter().map(|r| r.text.as_str()).collect()
    }
}

/// Code blocks, those with only blank blocks between them merged (`MarkdownHelpers.swift:153-200`).
pub fn code_blocks(doc: &Doc) -> Vec<CodeBlock> {
    let blocks = &doc.blocks;
    let blank = |b: &Block| !b.code_bg && !b.rule && b.runs.iter().all(|r| r.text.trim().is_empty());
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    let mut i = 0;
    while i < blocks.len() {
        if blocks[i].code_bg {
            let start = i;
            while i + 1 < blocks.len() && blocks[i + 1].code_bg {
                i += 1;
            }
            match ranges.last_mut() {
                Some(prev) if (prev.1 + 1..start).all(|k| blank(&blocks[k])) => prev.1 = i,
                _ => ranges.push((start, i)),
            }
        }
        i += 1;
    }
    ranges.into_iter().map(|(first, last)| CodeBlock { first, last, text: blocks[first..=last].iter().map(block_text).collect::<Vec<_>>().join("\n") }).collect()
}

/// `forkedChatTitle` (`CWC:113-165`), English only: a suffix "(Forked at <text with a digit>)"
/// at the end is replaced, never stacked.
pub fn fork_title(original: &str, timestamp: &str) -> String {
    const PREFIX: &str = "(Forked at ";
    let suffix = format!("{PREFIX}{timestamp})");
    let base = original
        .strip_suffix(')')
        .and_then(|head| head.rfind(PREFIX).map(|at| (at, &head[at + PREFIX.len()..])))
        .filter(|(_, stamp)| stamp.chars().any(char::is_numeric))
        .map_or(original, |(at, _)| &original[..at]);
    let base = base.strip_suffix(' ').unwrap_or(base);
    if base.is_empty() {
        suffix
    } else {
        format!("{base} {suffix}")
    }
}

/// That message's id and every later one's (`messageIDs(fromIndex:)`).
pub fn ids_from(visible: &[&Message], id: &str) -> Vec<String> {
    match visible.iter().position(|m| m.unique_id == id) {
        Some(at) => visible[at..].iter().map(|m| m.unique_id.clone()).collect(),
        None => Vec::new(),
    }
}

/// Windows device names, which no file or folder may be called, with any extension.
fn reserved(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or("").trim_end_matches([' ', '.']).to_ascii_uppercase();
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$") || ((stem.starts_with("COM") || stem.starts_with("LPT")) && stem.len() == 4 && matches!(stem.as_bytes()[3], b'1'..=b'9'))
}

/// The final component of an untrusted attachment name, or None when it is refused
/// (ruling R12cd-6).
pub fn attachment_name(name: &str) -> Option<String> {
    if name.is_empty() || name.starts_with(['\\', '/']) {
        return None;
    }
    let bytes = name.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        return None;
    }
    let parts: Vec<&str> = name.split(['\\', '/']).collect();
    if parts.contains(&"..") {
        return None;
    }
    let last = *parts.last()?;
    // Bidirectional and zero-width marks can disguise an extension.
    let disguises = |c: char| matches!(c, '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{FEFF}');
    let bad = last.is_empty() || last == "." || last.chars().count() > 255 || last.ends_with(['.', ' ']) || last.chars().any(|c| c.is_control() || disguises(c) || "<>:\"|?*".contains(c)) || reserved(last);
    (!bad).then(|| last.to_string())
}

/// A folder name for an untrusted id (ruling R12cd-6): the id when it is a plain word, else
/// "h" and the 16 hex digits of its FNV-1a hash.
pub fn safe_dir(id: &str) -> String {
    let plain = (1..=64).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-') && !reserved(id);
    if plain {
        return id.to_string();
    }
    let hash = id.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3));
    format!("h{hash:016x}")
}

/// `root\<safe chat>\<safe id>\<name>`; None when the name is refused.
pub fn attachment_path(root: &Path, chat: &str, id: &str, name: &str) -> Option<PathBuf> {
    Some(root.join(safe_dir(chat)).join(safe_dir(id)).join(attachment_name(name)?))
}

/// Writes an attachment's bytes under the chat's folder and answers the path. Every refusal is
/// an Err and writes nothing: a refused name, a folder that leaves the chat's folder (a
/// junction), an existing target that is not a plain file.
pub fn materialize(root: &Path, chat: &str, id: &str, name: &str, content: &[u8]) -> Result<PathBuf, String> {
    let path = attachment_path(root, chat, id, name).ok_or_else(|| format!("the attachment name {name:?} is not allowed"))?;
    let dir = path.parent().ok_or("no folder")?;
    let chat_dir = dir.parent().ok_or("no folder")?;
    let canon = |p: &Path| p.canonicalize().map_err(|e| format!("could not resolve {}: {e}", p.display()));
    let make = |p: &Path| std::fs::create_dir_all(p).map_err(|e| format!("could not make {}: {e}", p.display()));
    // The chat's folder is checked before anything is made inside it, so a junction there
    // leaves nothing behind.
    make(chat_dir)?;
    let (root_c, chat_c) = (canon(root)?, canon(chat_dir)?);
    if chat_c.parent() != Some(root_c.as_path()) {
        return Err(format!("{} leaves the temporary folder", chat_dir.display()));
    }
    make(dir)?;
    let dir_c = canon(dir)?;
    if dir_c.parent() != Some(chat_c.as_path()) {
        return Err(format!("{} leaves the chat's folder", dir.display()));
    }
    match std::fs::symlink_metadata(&path) {
        Ok(meta) => {
            if !meta.is_file() || canon(&path)?.parent() != Some(dir_c.as_path()) {
                return Err(format!("{} is not a plain file", path.display()));
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("could not read {}: {e}", path.display())),
    }
    std::fs::write(&path, content).map_err(|e| format!("could not write {}: {e}", path.display()))?;
    Ok(path)
}

/// Whether Open may hand a file to its association: inert document and picture types only
/// (ruling F19). Anything else is revealed.
pub fn openable(name: &str) -> bool {
    const INERT: [&str; 11] = ["txt", "md", "json", "csv", "log", "pdf", "png", "jpg", "jpeg", "gif", "webp"];
    Path::new(name).extension().and_then(|e| e.to_str()).is_some_and(|e| INERT.contains(&e.to_ascii_lowercase().as_str()))
}

/// A link a bubble may open: web and mail only (ruling F18); every other scheme, `file:`, UNC
/// and `iterm2:` included, is ignored.
pub fn web_link(url: &str) -> Option<String> {
    if url.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return None;
    }
    crate::prompted::safe_url(url)
}

/// What Copy puts on the clipboard for a body: the rendered text.
pub fn copy_text(body: &Body) -> String {
    match body {
        Body::Plain(t) => t.clone(),
        Body::Markdown(md) => crate::chat_search::markdown_text(md),
        Body::Command { command, .. } => command.clone(),
        Body::Parts(parts) => parts
            .iter()
            .map(|p| match p {
                Part::Text(b) => copy_text(b),
                Part::Code(c) => c.clone(),
                Part::Status(s) => crate::chat_search::markdown_text(s),
                Part::File { name, .. } | Part::RemoteFile { name } => name.clone(),
            })
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

/// One string naming an attachment of a message, for a file item's event.
pub fn file_ref(message: &str, attachment: &str) -> String {
    format!("{message}\u{1f}{attachment}")
}

pub fn split_file_ref(r: &str) -> Option<(&str, &str)> {
    r.split_once('\u{1f}')
}

/// A message's time as whole seconds: a date that is not a positive finite number is 0
/// (ruling F26).
pub fn epoch_secs(t: f64) -> u64 {
    if t.is_finite() && t > 0.0 {
        t as u64
    } else {
        0
    }
}

/// Changes when a message's id or what it draws does; keys the rendered markdown.
pub fn body_key(id: &str, body: &Body) -> u64 {
    use std::hash::{Hash, Hasher};
    fn feed(h: &mut impl Hasher, body: &Body) {
        match body {
            Body::Markdown(t) => (0u8, t).hash(h),
            Body::Plain(t) => (1u8, t).hash(h),
            Body::Command { command, url } => (2u8, command, url).hash(h),
            Body::Parts(parts) => {
                3u8.hash(h);
                for p in parts {
                    match p {
                        Part::Text(b) => {
                            4u8.hash(h);
                            feed(h, b);
                        }
                        Part::Code(c) => (5u8, c).hash(h),
                        Part::Status(s) => (6u8, s).hash(h),
                        Part::File { id, name } => (7u8, id, name).hash(h),
                        Part::RemoteFile { name } => (8u8, name).hash(h),
                    }
                }
            }
        }
    }
    let mut h = std::collections::hash_map::DefaultHasher::new();
    id.hash(&mut h);
    feed(&mut h, body);
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai_permissions::Category;
    use crate::ai_tools::Call;
    use crate::chat_message::{RemoteCommand, TerminalCommand};
    use crate::palette::Palette;
    use crate::porthole::detect::Language;
    use crate::porthole::render::render;
    use serde_json::json;

    fn msg(author: Participant, content: Content, at: f64) -> Message {
        Message::new("C", author, content, at)
    }

    fn plain(t: &str) -> Content {
        Content::PlainText { text: t.into(), context: None }
    }

    fn classic(call: Call, safe: Option<bool>) -> Content {
        Content::RemoteCommandRequest { payload: Payload::Classic(RemoteCommand { llm_message: json!({"role":"assistant"}), call }), safe }
    }

    fn ids(v: &[&Message]) -> Vec<String> {
        v.iter().map(|m| m.unique_id.clone()).collect()
    }

    #[test]
    fn items_follow_iterm2() {
        let (d1, d2) = (86_400.0 * 100.0 + 50.0, 86_400.0 * 101.0 + 50.0);
        let hide = msg(Participant::User, Content::SetPermissions(vec![Category::RunCommands]), d1);
        let a = msg(Participant::User, plain("a"), d1);
        let r = msg(Participant::Agent, classic(Call::IsAtPrompt, None), d1);
        let answered = msg(Participant::User, Content::RemoteCommandResponse { result: Ok("x".into()), request_id: r.unique_id.clone(), function_name: "f".into(), function_call_id: None }, d1);
        let b = msg(Participant::Agent, Content::Markdown("b".into()), d2);
        let all = vec![hide, a.clone(), r, answered, b.clone()];
        let shown = visible(&all);
        assert_eq!(ids(&shown), vec![a.unique_id.clone(), b.unique_id.clone()]);
        let day = |t: f64| (2026, 9, (t / 86_400.0) as u32);
        let label = |t: f64| format!("d{}", (t / 86_400.0) as u32);
        assert_eq!(items(&shown, false, &day, &label), vec![Item::Date("d100".into()), Item::Message(0), Item::Date("d101".into()), Item::Message(1)]);
        assert_eq!(items(&shown, true, &day, &label).last(), Some(&Item::Typing));
        // A request that is not answered stays; a lone message still starts with its date.
        let open = msg(Participant::Agent, classic(Call::IsAtPrompt, None), d2);
        let one = vec![open];
        let shown = visible(&one);
        assert_eq!(items(&shown, false, &day, &label), vec![Item::Date("d101".into()), Item::Message(0)]);
        assert!(items(&[], false, &day, &label).is_empty());
    }

    #[test]
    fn date_labels_follow_iterm2() {
        let sep3 = |year, weekday| Parts { year, month: 9, day: 3, weekday, ..Default::default() };
        assert_eq!(date_label(&sep3(2026, 4), 0, true), "Today");
        assert_eq!(date_label(&sep3(2026, 3), 1, true), "Yesterday");
        assert_eq!(date_label(&sep3(2026, 1), 3, true), "Monday");
        assert_eq!(date_label(&sep3(2026, 6), 6, true), "Saturday");
        assert_eq!(date_label(&sep3(2026, 0), 2, true), "Sunday");
        assert_eq!(date_label(&sep3(2026, 4), 7, true), "Sep 3");
        assert_eq!(date_label(&sep3(2025, 3), 40, false), "Sep 3, 2025");
        assert_eq!(date_label(&sep3(2026, 4), -1, true), "Sep 3, 2026");
    }

    #[test]
    fn day_gaps_count_calendar_days() {
        let p = |year, month, day| Parts { year, month, day, ..Default::default() };
        assert_eq!(days_between(&p(2026, 9, 27), &p(2026, 9, 28)), 1);
        assert_eq!(days_between(&p(2026, 2, 28), &p(2026, 3, 1)), 1);
        assert_eq!(days_between(&p(2024, 2, 28), &p(2024, 3, 1)), 2);
        assert_eq!(days_between(&p(2025, 12, 31), &p(2026, 1, 1)), 1);
        assert_eq!(days_between(&p(2026, 9, 28), &p(2026, 9, 27)), -1);
        // Seen on the 28th: one from the 27th is yesterday, the 30th of last year is not this year's.
        let now = Parts { weekday: 1, ..p(2026, 9, 28) };
        assert_eq!(date_text(&Parts { weekday: 0, ..p(2026, 9, 27) }, &now), "Yesterday");
        assert_eq!(date_text(&Parts { weekday: 6, ..p(2026, 9, 5) }, &now), "Sep 5");
        assert_eq!(date_text(&Parts { weekday: 2, ..p(2025, 12, 30) }, &now), "Dec 30, 2025");
    }

    fn tc(command: &str) -> Content {
        Content::TerminalCommand(TerminalCommand { username: None, hostname: None, directory: None, command: command.into(), output: "o".into(), exit_code: 0, url: "iterm2:reveal-mark?s=1&m=2".into() })
    }

    fn attachment(id: &str, kind: serde_json::Value) -> Subpart {
        Subpart::Attachment(json!({"id": id, "type": kind}))
    }

    #[test]
    fn bodies_follow_iterm2() {
        assert_eq!(body(&plain("  hi")), Body::Plain("hi".into()));
        assert_eq!(body(&Content::Markdown("x".into())), Body::Markdown("x".into()));
        assert_eq!(body(&Content::ExplanationRequest(json!({}))), Body::Plain(UNSUPPORTED_TEXT.into()));
        assert_eq!(body(&Content::ExplanationResponse(json!({}))), Body::Plain(UNSUPPORTED_TEXT.into()));
        let local = |a| Content::ClientLocal(a);
        assert_eq!(body(&local(ClientLocal::PickingSession)), Body::Markdown("Waiting for a session to be selected\u{2026}".into()));
        assert_eq!(body(&local(ClientLocal::StreamingChanged(Streaming::Active))), Body::Markdown("All terminal commands in the linked session will be sent to AI automatically.".into()));
        assert_eq!(body(&local(ClientLocal::StreamingChanged(Streaming::Stopped))), Body::Markdown("Terminal commands will no longer be sent to AI automatically.".into()));
        assert_eq!(
            body(&local(ClientLocal::StreamingChanged(Streaming::StoppedAutomatically))),
            Body::Markdown("Terminal commands will no longer be sent to AI automatically. Automatic sending always terminates when Tandem restarts or the current chat changes.".into())
        );
        let link = |terminal, name: Option<&str>| body(&local(ClientLocal::OfferLink { terminal, guid: "g".into(), name: name.map(str::to_string) }));
        assert_eq!(link(true, Some("zsh")), Body::Markdown("**Link this chat to terminal session \u{201C}zsh\u{201D}, or enable orchestration?**\n\nLinking gives the AI access to this terminal session subject to your per-call permission. **Orchestration** is an alternative mode where the AI can coordinate multiple sessions. It uses automatic safety checking and one-time per-session approval instead of fine-grained permissions.".into()));
        let Body::Markdown(unnamed) = link(true, None) else { panic!() };
        assert!(unnamed.starts_with("**Link this chat to terminal session \u{201C}Unnamed session\u{201D}, or enable"));
        let Body::Markdown(browser) = link(false, Some("Tab")) else { panic!() };
        assert!(browser.contains("browser session \u{201C}Tab\u{201D}") && browser.contains("access to this browser session subject"));
        assert_eq!(
            body(&local(ClientLocal::OfferOrchestration)),
            Body::Markdown("**Enable orchestration for this chat?**\n\n**Orchestration** lets the AI coordinate across multiple terminal sessions. It can read the contents of any session, and you grant a one-time approval before it controls a session instead of approving every call. It also runs in **auto mode**: each command the AI proposes is checked for safety by your AI provider, and anything risky is held for your review.".into())
        );
        assert_eq!(body(&local(ClientLocal::Permissions { terminal: true, guid: "g".into() })), Body::Markdown("You can use these buttons or the info button menu at the top of the chat window to control AI permissions for this chat.".into()));
        assert_eq!(body(&local(ClientLocal::Notice("a *b*".into()))), Body::Plain("a *b*".into()));
        assert_eq!(body(&Content::WatcherEvent(json!({"_0": {"reason": "timerFired", "detail": "fired"}}))), Body::Markdown("fired".into()));
        assert_eq!(glyph(&Content::WatcherEvent(json!({"_0": {"reason": "timerFired", "detail": "fired"}}))), Some(TIMER_GLYPH));
        assert_eq!(glyph(&Content::WatcherEvent(json!({"_0": {"reason": "output", "detail": "d"}}))), Some(WATCHER_GLYPH));
        let call = Call::ExecuteCommand { command: "ls".into() };
        let want = |safe| Body::Markdown(crate::ai_permissions::parked_text(&call.consent_description(), Category::RunCommands.title(), safe));
        assert_eq!(body(&classic(call.clone(), None)), want(None));
        assert_eq!(body(&classic(call.clone(), Some(false))), want(Some(false)));
        assert_eq!(glyph(&classic(call.clone(), Some(false))), Some(WARNING_GLYPH));
        assert_eq!(glyph(&classic(call, Some(true))), None);
        assert_eq!(body(&Content::Unsupported("{}".into())), Body::Markdown("This message requires a newer version of Tandem to view.".into()));
        assert_eq!(body(&Content::SelectSessionRequest { original: Box::new(msg(Participant::Agent, plain("x"), 1.0)), terminal: true }), Body::Markdown("The AI agent needs to run commands in a live terminal session, but none is attached to this chat.".into()));
        let Body::Markdown(web) = body(&Content::SelectSessionRequest { original: Box::new(msg(Participant::Agent, plain("x"), 1.0)), terminal: false }) else { panic!() };
        assert!(web.contains("live web browser session"));
        for c in [Content::RenameChat("t".into()), Content::SetPermissions(vec![]), Content::Commit("u".into()), Content::VectorStoreCreated("v".into())] {
            assert_eq!(body(&c), Body::Plain("A vector store was created".into()));
        }
        assert_eq!(body(&tc("ls -l")), Body::Command { command: "ls -l".into(), url: "iterm2:reveal-mark?s=1&m=2".into() });
        let parts = Content::Multipart {
            parts: vec![
                Subpart::PlainText("hello".into()),
                attachment("ci_1", json!({"code": {"_0": "print(1)"}})),
                attachment("s1", json!({"statusUpdate": {"_0": {"webSearchFinished": {"_0": "rust"}}}})),
                attachment("f1", json!({"file": {"_0": {"name": "a.txt", "content": "aGk=", "mimeType": "text/plain"}}})),
                attachment("f2", json!({"fileID": {"id": "x", "name": "b.pdf"}})),
                Subpart::Markdown("**md**".into()),
                Subpart::Context("model only".into()),
            ],
            vector_store_id: None,
        };
        assert_eq!(
            body(&parts),
            Body::Parts(vec![
                Part::Text(Body::Plain("hello".into())),
                Part::Code("print(1)".into()),
                Part::Status("Finished searching the web for **rust**.".into()),
                Part::File { id: "f1".into(), name: "a.txt".into() },
                Part::RemoteFile { name: "b.pdf".into() },
                Part::Text(Body::Markdown("**md**".into())),
            ])
        );
        assert_eq!(status_text(&Status::WebSearchStarted), "Searching the web\u{2026}");
        assert_eq!(status_text(&Status::WebSearchFinished(None)), "Finished searching the web.");
        assert_eq!(status_text(&Status::CodeInterpreterStarted), "Executing code\u{2026}");
        assert_eq!(status_text(&Status::CodeInterpreterFinished), "Finished executing code");
        assert_eq!(status_text(&Status::ReasoningSummary("thinking".into())), "thinking");
        // Merged updates show from the last finished one on (ruling F7), the query escaped.
        let merged = Status::Multipart(vec![Status::WebSearchStarted, Status::WebSearchFinished(Some("a*b".into())), Status::CodeInterpreterStarted]);
        assert_eq!(status_text(&merged), "Finished searching the web for **a\\*b**.\nExecuting code\u{2026}");
        assert_eq!(style(&local(ClientLocal::PickingSession)), Style::System);
        assert_eq!(style(&Content::WatcherEvent(json!({}))), Style::System);
        assert_eq!(style(&Content::Unsupported("x".into())), Style::System);
        assert_eq!(style(&parts), Style::Multipart);
        assert_eq!(style(&tc("x")), Style::Command);
        assert_eq!(style(&plain("x")), Style::Regular);
        assert_eq!(style(&Content::Markdown("x".into())), Style::Regular);
    }

    #[test]
    fn orchestration_bodies_render_but_never_carry_markup_from_their_fields() {
        let other = |v| body(&Content::ClientLocal(ClientLocal::Other(v)));
        let Body::Markdown(t) = other(json!({"enableOrchestrationRequest": {"requestID": "r"}})) else { panic!() };
        assert!(t.starts_with("**Enable orchestration?**\n\nOrchestration mode lets the agent read screen contents from any session. To type into a session still requires your permission.\n\n"));
        assert!(t.ends_with("the chat to Orchestration mode."));
        let Body::Markdown(t) = other(json!({"orchestrationPermissionGranted": {"scope": "s", "name": "[x](file:///c)"}})) else { panic!() };
        let name = crate::ai_tools::consent_field("[x](file:///c)", crate::ai_tools::Field::Text);
        assert!(t.starts_with(&format!("**Granted this chat permission to control \u{201C}{name}\u{201D}.**\n\nYou @-mentioned it")), "{t}");
        assert!(!name.contains("[x](") && name.starts_with("\\["));
        let req = |id: &str, name: &str, summary: &str| other(json!({"workgroupPermissionRequest": {"requestID": "r", "workgroupID": id, "workgroupName": name, "summary": summary}}));
        assert_eq!(req("spawn", "New session", "s1"), Body::Markdown("**Open a new session?**\n\ns1".into()));
        assert_eq!(req("command-approval", "n", "s2"), Body::Markdown("**Run this command?**\n\ns2".into()));
        assert_eq!(req("watch-approval", "n", "s3"), Body::Markdown("**Allow repeated screen reads?**\n\ns3".into()));
        assert_eq!(req("session:abc", "zsh", "s4"), Body::Markdown("**Allow agent to control session \u{201C}zsh\u{201D}?**\n\ns4".into()));
        assert_eq!(req("wg1", "Team", "s5"), Body::Markdown("**Allow agent to control workgroup \u{201C}Team\u{201D}?**\n\ns5".into()));
        // A field of the model's cannot open a link or a span.
        let Body::Markdown(t) = req("wg1", "[a](file:///c)", "**b**") else { panic!() };
        let name = crate::ai_tools::consent_field("[a](file:///c)", crate::ai_tools::Field::Text);
        assert!(!t.contains("[a](") && t.contains(&name) && t.contains("\\*\\*b\\*\\*"), "{t}");
    }

    #[test]
    fn a_pane_title_cannot_add_markup_to_the_link_offer() {
        // Ruling F20: the name is escaped; controls become visible escapes.
        let offer = |name: &str| body(&Content::ClientLocal(ClientLocal::OfferLink { terminal: true, guid: "g".into(), name: Some(name.into()) }));
        let Body::Markdown(t) = offer("**x** [click](https://evil.example)\nnext") else { panic!() };
        let escaped = crate::ai_tools::consent_field("**x** [click](https://evil.example)\nnext", crate::ai_tools::Field::Text);
        assert!(t.contains(&escaped) && escaped.contains("\\*\\*x\\*\\*") && escaped.ends_with("\\nnext") && !escaped.contains("](https"), "{t}");
        // Rendered, no run of that bubble carries a link.
        let doc = render(&t, &Language::Markdown, &Palette::default(), true);
        assert!(doc.blocks.iter().flat_map(|b| &b.runs).all(|r| r.link.is_none()));
    }

    #[test]
    fn model_only_context_is_never_shown() {
        // Ruling F8: a plain text's context and a multipart's context part.
        let c = Content::PlainText { text: "visible".into(), context: Some("secret terminal contents".into()) };
        assert_eq!(body(&c), Body::Plain("visible".into()));
        let m = Content::Multipart { parts: vec![Subpart::Context("secret".into()), Subpart::PlainText("shown".into())], vector_store_id: None };
        let Body::Parts(p) = body(&m) else { panic!() };
        assert_eq!(p, vec![Part::Text(Body::Plain("shown".into()))]);
        assert!(!copy_text(&body(&m)).contains("secret"));
    }

    #[test]
    fn code_blocks_follow_iterm2() {
        let doc = |md: &str| render(md, &Language::Markdown, &Palette::default(), true);
        let merged = code_blocks(&doc("intro\n\n```\na\nb\n```\n\n```\nc\n```\n\ntext"));
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].text, "a\nb\n\nc");
        let apart = code_blocks(&doc("intro\n\n```\na\n```\nmiddle\n```\nb\n```"));
        assert_eq!(apart.iter().map(|b| b.text.as_str()).collect::<Vec<_>>(), ["a", "b"]);
        assert!(apart[0].last < apart[1].first);
        assert!(code_blocks(&doc("no code here")).is_empty());
    }

    #[test]
    fn fork_titles_follow_iterm2() {
        assert_eq!(fork_title("Plan", "9/27/26 3:04 PM"), "Plan (Forked at 9/27/26 3:04 PM)");
        assert_eq!(fork_title("Plan (Forked at 9/27/26 3:04 PM)", "9/28/26 1:00 AM"), "Plan (Forked at 9/28/26 1:00 AM)");
        assert_eq!(fork_title("Notes (Forked at noon)", "t1"), "Notes (Forked at noon) (Forked at t1)");
        assert_eq!(fork_title("", "t"), "(Forked at t)");
        assert_eq!(fork_title("A ", "t"), "A (Forked at t)");
        // Only a suffix at the end is a fork suffix; the last prefix is the one cut.
        assert_eq!(fork_title("(Forked at 1) x", "t"), "(Forked at 1) x (Forked at t)");
        assert_eq!(fork_title("(Forked at 1)", "t"), "(Forked at t)");
    }

    #[test]
    fn forks_clone_with_fresh_ids_and_mapped_references() {
        let mut ids = HashMap::new();
        let mut request = msg(Participant::Agent, classic(Call::IsAtPrompt, None), 5.0);
        request.first_blob_ref = Some("blob-1".into());
        request.seq = 9;
        let response = msg(Participant::User, Content::RemoteCommandResponse { result: Ok("x".into()), request_id: request.unique_id.clone(), function_name: "f".into(), function_call_id: None }, 6.0);
        let select = msg(Participant::Agent, Content::SelectSessionRequest { original: Box::new(request.clone()), terminal: true }, 7.0);
        let commit = msg(Participant::Agent, Content::Commit(request.unique_id.clone()), 8.0);
        let c_request = clone_for_fork(&request, &mut ids);
        let c_response = clone_for_fork(&response, &mut ids);
        let c_select = clone_for_fork(&select, &mut ids);
        let c_commit = clone_for_fork(&commit, &mut ids);
        assert_ne!(c_request.unique_id, request.unique_id);
        assert_eq!(ids[&request.unique_id], c_request.unique_id);
        assert_eq!((c_request.seq, c_request.first_blob_ref.clone(), c_request.sent_date), (0, None, 5.0));
        assert_eq!(c_request.content, request.content);
        let Content::RemoteCommandResponse { request_id, .. } = &c_response.content else { panic!() };
        assert_eq!(*request_id, c_request.unique_id);
        let Content::SelectSessionRequest { original, terminal } = &c_select.content else { panic!() };
        assert!(*terminal);
        assert_eq!(original.unique_id, c_request.unique_id);
        assert_eq!(c_commit.content, Content::Commit(c_request.unique_id.clone()));
        // A reference to a message that was not cloned stays as it was.
        let stray = msg(Participant::User, Content::Commit("gone".into()), 1.0);
        assert_eq!(clone_for_fork(&stray, &mut ids).content, Content::Commit("gone".into()));
    }

    #[test]
    fn attachment_names_are_refused() {
        assert_eq!(attachment_name("a.txt").as_deref(), Some("a.txt"));
        assert_eq!(attachment_name("sub\\b.md").as_deref(), Some("b.md"));
        assert_eq!(attachment_name("sub/b.md").as_deref(), Some("b.md"));
        assert_eq!(attachment_name("Startup\\a.cmd").as_deref(), Some("a.cmd"));
        for bad in [
            "", ".", "..", "..\\..\\x.cmd", "a/../../x", "..\\..\\..\\Roaming\\Microsoft\\Windows\\Start Menu\\Programs\\Startup\\a.cmd", "C:\\x", "C:x", "c:", "\\x", "/x", "\\\\h\\s\\x", "//h/s/x", "\\\\?\\C:\\x",
            "\\\\.\\PhysicalDrive0", "CON", "con", "nul.txt", "AUX", "PRN.log", "COM1", "com9.txt", "LPT1", "lpt9.log", "CONIN$", "CONOUT$", "a.txt:evil", "a.txt::$DATA", "a\u{7}b", "a\nb", "a\u{0}", "a\u{7f}", "a.txt.", "a.txt ",
            "sub\\", "a<b", "a|b", "a?b", "a*b", "a\"b", "CON .txt", "Con..",
        ] {
            assert_eq!(attachment_name(bad), None, "{bad:?}");
        }
        assert_eq!(attachment_name(&"a".repeat(300)), None);
        assert_eq!(attachment_name("console.txt").as_deref(), Some("console.txt"));
        assert_eq!(attachment_name("com10.txt").as_deref(), Some("com10.txt"));
    }

    #[test]
    fn attachment_paths_stay_under_the_chat() {
        let root = Path::new(r"C:\T");
        assert_eq!(attachment_path(root, "CHAT-1", "ci_1", "a.txt"), Some(PathBuf::from(r"C:\T\CHAT-1\ci_1\a.txt")));
        assert_eq!(safe_dir("..\\x"), "h72ace7a684ce2801");
        assert_eq!(safe_dir("../../y"), "h0e455457190c664a");
        assert_eq!(safe_dir("ci 1"), "hfa6fe5910f871e26");
        assert_eq!(attachment_path(root, "../../y", "..\\x", "a.txt"), Some(PathBuf::from(r"C:\T\h0e455457190c664a\h72ace7a684ce2801\a.txt")));
        assert_eq!(attachment_path(root, "CHAT-1", "ci_1", "..\\x"), None);
        // No id names a device, a dot or a separator.
        for id in ["", ".", "..", "CON", "nul", "com1", "a/b", "a\\b", "C:", &"x".repeat(65)] {
            let dir = safe_dir(id);
            assert!(dir.starts_with('h') && dir.len() == 17, "{id:?} -> {dir}");
        }
        assert_eq!(safe_dir("6F1C2A3B-0000-4000-8000-000000000001"), "6F1C2A3B-0000-4000-8000-000000000001");
        assert_eq!(safe_dir("console"), "console");
        for (chat, id) in [("CHAT-1", "ci_1"), ("../..", "..\\x"), ("a b", "c:d")] {
            let path = attachment_path(root, chat, id, "a.txt").unwrap();
            assert_eq!(path.parent().unwrap().parent().unwrap(), root.join(safe_dir(chat)));
        }
    }

    #[cfg(windows)]
    #[test]
    fn junction_cannot_escape() {
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let base = std::env::temp_dir().join(format!("tandem-junction-{}-{stamp}", std::process::id()));
        let (root, outside) = (base.join("root"), base.join("outside"));
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::create_dir_all(&root).unwrap();
        let junction = |link: &Path, target: &Path| {
            let out = std::process::Command::new("cmd").args(["/c", "mklink", "/J"]).arg(link).arg(target).output().unwrap();
            assert!(out.status.success(), "mklink: {}", String::from_utf8_lossy(&out.stdout));
        };
        let fine = materialize(&root, "C", "A", "a.txt", b"hi").unwrap();
        assert_eq!(fine, root.join("C").join("A").join("a.txt"));
        assert_eq!(std::fs::read(&fine).unwrap(), b"hi");
        // The attachment's folder is a junction to somewhere else.
        junction(&root.join("C").join("B"), &outside);
        assert!(materialize(&root, "C", "B", "a.txt", b"evil").is_err());
        assert!(!outside.join("a.txt").exists());
        // The target file is a junction.
        std::fs::remove_file(&fine).unwrap();
        junction(&fine, &outside);
        assert!(materialize(&root, "C", "A", "a.txt", b"evil").is_err());
        assert!(std::fs::read_dir(&outside).unwrap().next().is_none());
        // The chat's folder is a junction out of the root.
        junction(&root.join("D"), &outside);
        assert!(materialize(&root, "D", "A", "a.txt", b"evil").is_err());
        assert!(std::fs::read_dir(&outside).unwrap().next().is_none());
        // A refused name writes nothing.
        assert!(materialize(&root, "C", "A", "..\\x", b"evil").is_err());
        for link in [root.join("C").join("B"), fine, root.join("D")] {
            let _ = std::process::Command::new("cmd").args(["/c", "rmdir"]).arg(&link).output();
        }
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn open_is_an_allowlist_of_inert_types() {
        // Ruling F19.
        for ok in ["a.txt", "b.MD", "c.json", "d.csv", "e.log", "f.pdf", "g.png", "h.JPG", "i.jpeg", "j.gif", "k.webp"] {
            assert!(openable(ok), "{ok}");
        }
        for no in ["a.docm", "a.xlsm", "a.rdp", "a.iso", "a.html", "a.htm", "a.svg", "a.exe", "a.lnk", "a.ps1", "a.py", "a", "a.txt.exe", "a.", ".txt", "a.tx", "a.pdf.lnk", "a.LNK"] {
            assert!(!openable(no), "{no}");
        }
    }

    #[test]
    fn links_open_only_when_web_or_mail() {
        // Ruling F18 (and minor m10: an iterm2: link does nothing).
        assert_eq!(web_link("https://example.com/a?b=1").as_deref(), Some("https://example.com/a?b=1"));
        assert_eq!(web_link("HTTP://example.com").as_deref(), Some("HTTP://example.com"));
        assert_eq!(web_link("mailto:a@b.example").as_deref(), Some("mailto:a@b.example"));
        for no in ["file:///C:/x.exe", "file://host/share/x", "\\\\host\\share\\x", "//host/share", "C:\\x.exe", "ms-msdt:/id", "iterm2:reveal-mark?s=1", "javascript:alert(1)", "vscode://x", "search-ms:query=x", "https:", "", "example.com", "  https://example.com", "https://exa mple.com/\n"] {
            assert_eq!(web_link(no), None, "{no:?}");
        }
    }

    #[test]
    fn message_menu_follows_iterm2() {
        assert_eq!(message_menu(true), ["Edit", "Copy", "Fork", "Delete"]);
        assert_eq!(message_menu(false), ["Copy"]);
        let user_text = msg(Participant::User, plain("x"), 1.0);
        assert!(editable(&user_text));
        assert!(!editable(&msg(Participant::Agent, plain("x"), 1.0)));
        assert!(!editable(&msg(Participant::User, Content::Multipart { parts: vec![], vector_store_id: None }, 1.0)));
        assert!(!editable(&msg(Participant::User, Content::Markdown("x".into()), 1.0)));
        let (a, b, c) = (msg(Participant::User, plain("a"), 1.0), msg(Participant::User, plain("b"), 2.0), msg(Participant::User, plain("c"), 3.0));
        assert_eq!(ids_from(&[&a, &b, &c], &b.unique_id), vec![b.unique_id.clone(), c.unique_id.clone()]);
        assert!(ids_from(&[&a, &b], "none").is_empty());
        assert_eq!(trim_leading("\n \t x y "), "x y ");
        assert_eq!(trim_leading("  \n"), "  \n");
        assert_eq!(trim_leading(""), "");
    }

    #[test]
    fn copy_puts_the_rendered_text_on_the_clipboard() {
        assert_eq!(copy_text(&Body::Plain("a *b*".into())), "a *b*");
        assert_eq!(copy_text(&Body::Markdown("some **bold** text".into())), "some bold text");
        assert_eq!(copy_text(&Body::Command { command: "ls".into(), url: "u".into() }), "ls");
        let parts = Body::Parts(vec![Part::Text(Body::Plain("one".into())), Part::Code("two".into()), Part::Status("Finished **x**".into()), Part::File { id: "i".into(), name: "n.txt".into() }]);
        assert_eq!(copy_text(&parts), "one\ntwo\nFinished x\nn.txt");
    }

    #[test]
    fn file_references_round_trip() {
        let r = file_ref("MSG", "att");
        assert_eq!(split_file_ref(&r), Some(("MSG", "att")));
        // An id holding the separator cannot forge another message's reference.
        assert_eq!(split_file_ref(&file_ref("M", "a\u{1f}b")), Some(("M", "a\u{1f}b")));
        assert_eq!(split_file_ref("nothing"), None);
    }

    #[test]
    fn times_and_keys_follow_the_content() {
        assert_eq!(epoch_secs(12.9), 12);
        for odd in [f64::NAN, -5.0, f64::INFINITY, f64::NEG_INFINITY, 0.0] {
            assert_eq!(epoch_secs(odd), 0);
        }
        let key = |id: &str, b: Body| body_key(id, &b);
        assert_eq!(key("a", Body::Plain("x".into())), key("a", Body::Plain("x".into())));
        assert_ne!(key("a", Body::Plain("x".into())), key("a", Body::Plain("y".into())));
        assert_ne!(key("a", Body::Plain("x".into())), key("b", Body::Plain("x".into())));
        assert_ne!(key("a", Body::Plain("x".into())), key("a", Body::Markdown("x".into())));
        let parts = |t: &str| Body::Parts(vec![Part::Text(Body::Plain("one".into())), Part::Code(t.into())]);
        assert_ne!(key("a", parts("p")), key("a", parts("q")));
        let file = |n: &str| Body::Parts(vec![Part::File { id: "i".into(), name: n.into() }]);
        assert_ne!(key("a", file("a.txt")), key("a", file("b.txt")));
        assert_ne!(key("a", Body::Command { command: "ls".into(), url: "u".into() }), key("a", Body::Command { command: "ls".into(), url: "v".into() }));
    }
}
