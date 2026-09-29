//! The chat input (iTerm2's `ChatInputView` and the send half of `ChatViewController`): when
//! Send is enabled, what Enter does, the placeholder and the hint, the message a send builds
//! (attachments included), the drop and rejection texts, and the drafts kept per chat. Pure.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::ai_gate::{Alert, Then};
use crate::chat_json;
use crate::chat_message::{new_id, Content, Subpart};

pub const PLACEHOLDER: &str = "Type a message\u{2026}";
pub const PLACEHOLDER_MENTION: &str = "Type a message, or @ to mention a session\u{2026}";
pub const HINT: &str = "Enter to submit";
pub const ATTACH_TIP: &str = "Attach files";
pub const SEND_GLYPH: char = '\u{E724}';
pub const STOP_GLYPH: char = '\u{E71A}';
/// The permanent-silence key of the drop question (`IN:507`).
pub const DROP_SILENCE: &str = "NoSyncAIChatAttachDroppedFile";
pub const DROP_BUTTONS: [&str; 2] = ["Attach", "Insert Path"];
/// The attach popup's two items (Windows has no one dialog for files and folders).
pub const ATTACH_FILES: &str = "Files\u{2026}";
pub const ATTACH_FOLDER: &str = "Folder\u{2026}";

/// The most files one dropped or picked folder adds and the largest single file read: a drop
/// of a whole drive must not read it into memory.
pub const FOLDER_FILES_MAX: usize = 200;
pub const FILE_BYTES_MAX: u64 = 32 << 20;

/// The field's placeholder (`IN:634-639`): the mention form in an orchestrating chat.
pub fn placeholder(orchestrating: bool) -> &'static str {
    if orchestrating {
        PLACEHOLDER_MENTION
    } else {
        PLACEHOLDER
    }
}

/// Whether the send button is enabled (`IN:394-411`): a whitespace-only field never sends, an
/// attachment still loading blocks it, and a typing agent (Stop) always enables it.
pub fn send_enabled(text: &str, loading: bool, stoppable: bool) -> bool {
    stoppable || (!text.trim().is_empty() && !loading)
}

/// Whether the hint label is hidden (`IN:413-443`): two lines or more, or text that reaches
/// the hint (12 points of margin and 20 of air before it).
pub fn hint_hidden(text: &str, lines: usize, text_w: f32, hint_w: f32, field_w: f32) -> bool {
    if text.is_empty() {
        return false;
    }
    lines > 1 || text_w > field_w - hint_w - 12.0 - 20.0
}

/// What Enter does (`IN:712-721`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Enter {
    Send,
    /// The agent is typing: stop it, then send when that left it idle.
    StopThenSend,
    Stop,
    /// Shift+Enter: the editor's own newline.
    Newline,
    /// Not enabled: the key goes to the editor untouched, as iTerm2's field inserts a newline.
    Nothing,
}

pub fn on_enter(shift: bool, send_enabled: bool, stoppable: bool) -> Enter {
    if shift {
        Enter::Newline
    } else if send_enabled && stoppable {
        Enter::StopThenSend
    } else if send_enabled {
        Enter::Send
    } else if stoppable {
        Enter::Stop
    } else {
        Enter::Nothing
    }
}

/// One thing waiting to go with the next message.
#[derive(Clone, Debug, PartialEq)]
pub enum Pending {
    /// A file or folder on disk, read when the message is sent.
    Path(PathBuf),
    InMemory { name: String, content: Vec<u8>, mime: String },
}

impl Pending {
    /// What the attachment strip shows.
    pub fn label(&self) -> String {
        match self {
            Pending::Path(p) => file_name(p),
            Pending::InMemory { name, .. } => name.clone(),
        }
    }
}

/// The last component of a path, as the person knows the file.
pub fn file_name(p: &Path) -> String {
    p.file_name().map_or_else(|| p.to_string_lossy().to_string(), |n| n.to_string_lossy().to_string())
}

/// The attachment object for a file (`LLM.Message.Attachment` with `.file`): the shape
/// `chat_json::attachment` reads back. Kept apart from 12c's wire codec (ruling I-3).
pub fn file_attachment(id: &str, name: &str, content: &[u8], mime: &str) -> Value {
    chat_json::file_attachment(id, name, content, mime)
}

/// The parts a pending list makes (`CVC:2269-2346`): a folder gives every regular file below
/// it named by its path relative to the folder, a file its own name, an in-memory item what it
/// carries. An unreadable file is skipped.
pub fn attachments(pending: &[Pending], read: &dyn Fn(&Path) -> Option<Vec<u8>>, list: &dyn Fn(&Path) -> Option<Vec<PathBuf>>, mime: &dyn Fn(&str) -> String) -> Vec<Subpart> {
    let part = |name: &str, bytes: &[u8]| Subpart::Attachment(file_attachment(&new_id(), name, bytes, &mime(name)));
    let mut out = Vec::new();
    for item in pending {
        match item {
            Pending::InMemory { name, content, mime } => out.push(Subpart::Attachment(file_attachment(&new_id(), name, content, mime))),
            Pending::Path(p) => match list(p) {
                Some(files) => {
                    for f in files {
                        let Ok(rel) = f.strip_prefix(p) else { continue };
                        let name = rel.components().map(|c| c.as_os_str().to_string_lossy().to_string()).collect::<Vec<_>>().join("\\");
                        if name.is_empty() {
                            continue;
                        }
                        if let Some(bytes) = read(&f) {
                            out.push(part(&name, &bytes));
                        }
                    }
                }
                None => {
                    if let Some(bytes) = read(p) {
                        out.push(part(&file_name(p), &bytes));
                    }
                }
            },
        }
    }
    out
}

/// The content a send publishes (`CVC:2352-2370`): None for whitespace-only text; plain text
/// alone, else the text then the files with the chat's vector store.
pub fn message_content(text: &str, parts: Vec<Subpart>, vector_store: Option<String>) -> Option<Content> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    if parts.is_empty() {
        return Some(Content::PlainText { text: trimmed.to_string(), context: None });
    }
    let mut all = vec![Subpart::PlainText(trimmed.to_string())];
    all.extend(parts);
    Some(Content::Multipart { parts: all, vector_store_id: vector_store })
}

/// The drop question's heading and text (`IN:496-528`), for the supported files' names.
pub fn drop_question(names: &[String]) -> (String, String) {
    if let [one] = names {
        ("Attach File?".to_string(), format!("Add \u{201C}{one}\u{201D} to your message as an attachment, or insert its path as text?"))
    } else {
        ("Attach Files?".to_string(), format!("Add the {} dropped files to your message as attachments, or insert their paths as text?", names.len()))
    }
}

/// The alert for a drop with some supported files. Attach is the first button and only it
/// attaches: a close and Escape answer "Insert Path" (ruling F2), so closing never attaches.
pub fn drop_alert(chat: &str, supported: Vec<PathBuf>, all: Vec<PathBuf>) -> Alert {
    let names: Vec<String> = supported.iter().map(|p| file_name(p)).collect();
    let (heading, text) = drop_question(&names);
    Alert { heading, text, buttons: DROP_BUTTONS.to_vec(), silence: Some(DROP_SILENCE.to_string()), then: Then::DropChoice { chat: chat.to_string(), supported, all }, gate: false, close: Some(DROP_BUTTONS[1]) }
}

/// What the answer to the drop question does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DropAction {
    Attach,
    InsertPath,
}

/// Only the first button attaches; any other index, a stale remembered one included, inserts.
pub fn drop_action(index: usize) -> DropAction {
    if index == 0 {
        DropAction::Attach
    } else {
        DropAction::InsertPath
    }
}

/// (files to attach, paths to insert as text) for an answer (`IN:518-527`): Attach adds the
/// supported ones and inserts the others' paths; Insert Path inserts every path.
pub fn drop_result(action: DropAction, supported: &[PathBuf], all: &[PathBuf]) -> (Vec<PathBuf>, Vec<PathBuf>) {
    match action {
        DropAction::Attach => (supported.to_vec(), all.iter().filter(|p| !supported.contains(p)).cloned().collect()),
        DropAction::InsertPath => (Vec::new(), all.to_vec()),
    }
}

/// (accepted, rejected) among paths: a folder is accepted (its files are filtered as it is
/// read), a network path never (it is not probed as a file; a drop of one inserts its text),
/// a file when the model accepts its MIME type.
pub fn split_by_model(paths: &[PathBuf], is_dir: &dyn Fn(&Path) -> bool, accepts: &dyn Fn(&str) -> bool) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let (mut ok, mut no) = (Vec::new(), Vec::new());
    for p in paths {
        let text = p.to_string_lossy();
        if crate::remote_prefs::is_unc(&text) {
            no.push(p.clone());
        } else if is_dir(p) || accepts(&file_name(p)) {
            ok.push(p.clone());
        } else {
            no.push(p.clone());
        }
    }
    (ok, no)
}

/// The alert text for files the model does not take (`IN:572-586`): heading and body.
pub fn rejected(names: &[String], provider: Option<&str>) -> (String, String) {
    let heading = if names.len() == 1 { "Attachment not supported" } else { "Attachments not supported" };
    let provider = provider.unwrap_or("the current AI provider");
    (heading.to_string(), format!("{provider} doesn\u{2019}t accept this file type as a chat attachment: {}.", names.join(", ")))
}

/// Paths as text (`IN:553-560`), one space between.
pub fn inserted_paths(paths: &[PathBuf]) -> String {
    paths.iter().map(|p| p.to_string_lossy().to_string()).collect::<Vec<_>>().join(" ")
}

/// `text` with `insert` appended, a space between when the text does not already end in white
/// space (Decision: the caret is not tracked, so paths go at the end).
pub fn append_text(text: &str, insert: &str) -> String {
    if insert.is_empty() {
        return text.to_string();
    }
    if text.is_empty() || text.ends_with(char::is_whitespace) {
        format!("{text}{insert}")
    } else {
        format!("{text} {insert}")
    }
}

/// Adds paths to a pending list once each (`IN:541-545`).
pub fn add_paths(pending: &mut Vec<Pending>, paths: &[PathBuf]) {
    for p in paths {
        let item = Pending::Path(p.clone());
        if !pending.contains(&item) {
            pending.push(item);
        }
    }
}

/// The most one send reads in all (every file of every pending item).
pub const TOTAL_BYTES_MAX: u64 = 64 << 20;

/// The alert for files the model does not take: OK only, nothing follows.
pub fn rejected_alert(names: &[String], provider: Option<&str>) -> Alert {
    let (heading, text) = rejected(names, provider);
    Alert { heading, text, buttons: vec!["OK"], silence: None, then: Then::Nothing, gate: false, close: None }
}

/// A file's bytes, only for a regular local file no larger than `FILE_BYTES_MAX` (a network
/// path is never probed, a device or a folder is not a regular file).
pub fn read_capped(path: &Path) -> Option<Vec<u8>> {
    if crate::remote_prefs::is_unc(&path.to_string_lossy()) {
        return None;
    }
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > FILE_BYTES_MAX {
        return None;
    }
    std::fs::read(path).ok()
}

/// Whether a directory entry is a link or junction (a reparse point): never followed below a
/// folder someone chose, so a link inside it cannot pull in files from elsewhere.
fn is_link(meta: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        meta.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        meta.file_type().is_symlink()
    }
}

/// The regular files below `dir`, sorted by path within each folder and folders after the
/// files beside them (`recursiveRegularFileIterator`), at most `max` of them whose name
/// `accepts`; links are skipped. None when `dir` is not a folder.
pub fn list_folder(dir: &Path, accepts: &dyn Fn(&str) -> bool, max: usize) -> Option<Vec<PathBuf>> {
    if crate::remote_prefs::is_unc(&dir.to_string_lossy()) || !std::fs::metadata(dir).ok()?.is_dir() {
        return None;
    }
    let mut out = Vec::new();
    let mut stack = vec![(dir.to_path_buf(), 0usize)];
    while let Some((folder, depth)) = stack.pop() {
        let Ok(read) = std::fs::read_dir(&folder) else { continue };
        let mut entries: Vec<_> = read.filter_map(Result::ok).map(|e| e.path()).collect();
        entries.sort();
        let mut folders = Vec::new();
        for path in entries {
            let Ok(meta) = std::fs::symlink_metadata(&path) else { continue };
            if is_link(&meta) {
                continue;
            }
            if meta.is_dir() {
                if depth < 16 {
                    folders.push((path, depth + 1));
                }
            } else if meta.is_file() && accepts(&file_name(&path)) {
                if out.len() >= max {
                    return Some(out);
                }
                out.push(path);
            }
        }
        // The first folder is read next.
        stack.extend(folders.into_iter().rev());
    }
    Some(out)
}

/// What a chat's input holds.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Draft {
    pub text: String,
    pub pending: Vec<Pending>,
}

impl Draft {
    pub fn is_empty(&self) -> bool {
        self.text.is_empty() && self.pending.is_empty()
    }
}

/// Unsent input kept per chat (`CVC:108-115,864-894`).
#[derive(Clone, Debug, Default)]
pub struct Drafts {
    kept: HashMap<String, Draft>,
}

impl Drafts {
    /// The input moves from chat `from` to chat `to`: `current` is kept for `from` (an empty
    /// draft is dropped), and the draft `to` had is returned, else an empty one. The same chat
    /// changes nothing. With no chat to go to the input stays as it is.
    pub fn switch(&mut self, from: Option<&str>, current: Draft, to: Option<&str>) -> Draft {
        if from == to {
            return current;
        }
        if let Some(from) = from {
            if current.is_empty() {
                self.kept.remove(from);
            } else {
                self.kept.insert(from.to_string(), current.clone());
            }
        }
        match to {
            Some(to) => self.kept.remove(to).unwrap_or_default(),
            None => current,
        }
    }

    /// A deleted chat's draft goes with it.
    pub fn forget(&mut self, chat: &str) {
        self.kept.remove(chat);
    }
}

/// The input of the shown chat: its draft, whether a terminal selection may still be offered
/// into it (`eligibleForAutoPaste`, `CVC:131,1060-1067,2240,2375`), and a counter the window
/// watches to take the focus when the app changed the text.
#[derive(Clone, Debug, PartialEq)]
pub struct InputState {
    pub draft: Draft,
    pub eligible: bool,
    pub focus: u64,
}

impl Default for InputState {
    fn default() -> InputState {
        InputState { draft: Draft::default(), eligible: true, focus: 0 }
    }
}

impl InputState {
    /// The person typed: eligible exactly while the field is empty (`textDidChange`).
    pub fn typed(&mut self, text: String) {
        self.eligible = text.is_empty();
        self.draft.text = text;
    }

    /// The app replaced the text (Edit, a fork's staged message): kept from an offer.
    pub fn set(&mut self, text: &str) {
        self.draft.text = text.to_string();
        self.eligible = text.is_empty();
        self.focus += 1;
    }

    /// A selection offered as text (`offerSelectedText`): only into an empty, untouched input.
    pub fn offer(&mut self, text: &str) -> bool {
        if !(self.eligible && self.draft.text.is_empty()) {
            return false;
        }
        self.draft.text = text.to_string();
        self.focus += 1;
        true
    }

    /// After a send: nothing pending, eligible again (`CVC:2373-2375`).
    pub fn sent(&mut self) {
        self.draft = Draft::default();
        self.eligible = true;
    }

    /// The shown chat changed: this is its draft.
    pub fn switched(&mut self, draft: Draft) {
        self.eligible = draft.text.is_empty();
        self.draft = draft;
    }

    /// Paths inserted as text (a drop), at the end.
    pub fn insert(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.draft.text = append_text(&self.draft.text, text);
        self.eligible = false;
        self.focus += 1;
    }

    pub fn remove_attachment(&mut self, index: usize) {
        if index < self.draft.pending.len() {
            self.draft.pending.remove(index);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn send_rules_follow_iterm2() {
        assert!(!send_enabled("  \n", false, false));
        assert!(send_enabled("hi", false, false));
        assert!(!send_enabled("hi", true, false), "an attachment still loading");
        assert!(send_enabled("", false, true), "stoppable");
        assert_eq!(on_enter(false, true, false), Enter::Send);
        assert_eq!(on_enter(true, true, false), Enter::Newline);
        assert_eq!(on_enter(false, false, false), Enter::Nothing);
        assert_eq!(on_enter(false, true, true), Enter::StopThenSend);
        assert_eq!(on_enter(false, false, true), Enter::Stop);
        assert_eq!(placeholder(false), PLACEHOLDER);
        assert_eq!(placeholder(true), PLACEHOLDER_MENTION);
        assert!(!hint_hidden("", 1, 0.0, 90.0, 400.0));
        assert!(!hint_hidden("", 3, 0.0, 90.0, 400.0), "an empty field always shows it");
        assert!(hint_hidden("a\nb", 2, 10.0, 90.0, 400.0));
        assert!(hint_hidden("x", 1, 300.0, 90.0, 400.0));
        assert!(!hint_hidden("x", 1, 250.0, 90.0, 400.0));
    }

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn message_building_follows_iterm2() {
        assert_eq!(message_content("  hi  ", vec![], None), Some(Content::PlainText { text: "hi".into(), context: None }));
        assert_eq!(message_content("  ", vec![], None), None);
        let file = Subpart::Attachment(file_attachment("A", "a.txt", b"hi", "text/plain"));
        assert_eq!(
            message_content("hi", vec![file.clone()], Some("vs".into())),
            Some(Content::Multipart { parts: vec![Subpart::PlainText("hi".into()), file], vector_store_id: Some("vs".into()) })
        );
        assert_eq!(message_content(" ", vec![Subpart::PlainText("x".into())], None), None, "text is required");

        assert_eq!(
            file_attachment("A", "a.txt", b"hi", "text/plain"),
            serde_json::json!({"inline":false,"id":"A","type":{"file":{"_0":{"name":"a.txt","content":"aGk=","mimeType":"text/plain"}}}})
        );

        // A folder: every regular file below it, relative names with Windows separators.
        let read = |f: &Path| -> Option<Vec<u8>> {
            match f.file_name()?.to_str()? {
                "a.txt" => Some(b"A".to_vec()),
                "b.md" => Some(b"B".to_vec()),
                _ => None,
            }
        };
        let list = |d: &Path| -> Option<Vec<PathBuf>> { (d == Path::new("C:\\d")).then(|| vec![p("C:\\d\\a.txt"), p("C:\\d\\sub\\b.md"), p("C:\\d\\gone.bin")]) };
        let mime = |n: &str| if n.ends_with(".md") { "text/markdown".to_string() } else { "text/plain".to_string() };
        let parts = attachments(&[Pending::Path(p("C:\\d"))], &read, &list, &mime);
        let read_back: Vec<_> = parts
            .iter()
            .map(|s| match s {
                Subpart::Attachment(v) => chat_json::attachment(v).unwrap().1,
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(
            read_back,
            vec![
                chat_json::Attachment::File { name: "a.txt".into(), content: b"A".to_vec(), mime: "text/plain".into() },
                chat_json::Attachment::File { name: "sub\\b.md".into(), content: b"B".to_vec(), mime: "text/markdown".into() },
            ],
            "the unreadable file is skipped"
        );
        // A single file keeps its own name; an unreadable one gives nothing; in memory keeps everything.
        let one = attachments(&[Pending::Path(p("C:\\x\\a.txt")), Pending::Path(p("C:\\x\\nope.bin")), Pending::InMemory { name: "s.png".into(), content: vec![1, 2], mime: "image/png".into() }], &read, &|_| None, &mime);
        let names: Vec<_> = one
            .iter()
            .map(|s| match s {
                Subpart::Attachment(v) => match chat_json::attachment(v).unwrap().1 {
                    chat_json::Attachment::File { name, content, mime } => (name, content, mime),
                    other => panic!("{other:?}"),
                },
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(names, vec![("a.txt".to_string(), b"A".to_vec(), "text/plain".to_string()), ("s.png".to_string(), vec![1, 2], "image/png".to_string())]);
    }

    #[test]
    fn attachment_texts_follow_iterm2() {
        assert_eq!(drop_question(&["a.txt".into()]), ("Attach File?".to_string(), "Add \u{201C}a.txt\u{201D} to your message as an attachment, or insert its path as text?".to_string()));
        assert_eq!(drop_question(&["a".into(), "b".into()]), ("Attach Files?".to_string(), "Add the 2 dropped files to your message as attachments, or insert their paths as text?".to_string()));
        assert_eq!(rejected(&["x.exe".into()], Some("OpenAI")), ("Attachment not supported".to_string(), "OpenAI doesn\u{2019}t accept this file type as a chat attachment: x.exe.".to_string()));
        assert_eq!(rejected(&["x.exe".into(), "y.dll".into()], None), ("Attachments not supported".to_string(), "the current AI provider doesn\u{2019}t accept this file type as a chat attachment: x.exe, y.dll.".to_string()));
        assert_eq!(inserted_paths(&[p("C:\\a b.txt"), p("C:\\c.txt")]), "C:\\a b.txt C:\\c.txt");
        assert_eq!(DROP_SILENCE, "NoSyncAIChatAttachDroppedFile");
        assert_eq!(DROP_BUTTONS, ["Attach", "Insert Path"]);
    }

    #[test]
    fn drafts_follow_iterm2() {
        let mut d = Drafts::default();
        let a = Draft { text: "a".into(), pending: vec![Pending::Path(p("C:\\f.txt"))] };
        assert_eq!(d.switch(Some("1"), a.clone(), Some("2")), Draft::default(), "chat 2 starts empty");
        let b = Draft { text: "b".into(), pending: vec![] };
        assert_eq!(d.switch(Some("2"), b.clone(), Some("1")), a, "chat 1 gets its text and attachments back");
        assert_eq!(d.switch(Some("1"), a.clone(), None), a, "no chat keeps the input");
        assert_eq!(d.switch(None, a.clone(), Some("2")), b, "and chat 2 is still there");
        assert_eq!(d.switch(Some("2"), b.clone(), Some("2")), b, "the same chat changes nothing");
        // An empty draft is not kept.
        assert_eq!(d.switch(Some("2"), Draft::default(), Some("3")), Draft::default());
        assert_eq!(d.switch(Some("3"), Draft::default(), Some("2")), Draft::default());
    }

    #[test]
    fn closing_the_drop_question_never_attaches() {
        let alert = drop_alert("c", vec![p("C:\\a.txt")], vec![p("C:\\a.txt"), p("C:\\b.exe")]);
        assert_eq!((alert.gate, alert.silence.as_deref(), alert.buttons.clone()), (false, Some(DROP_SILENCE), DROP_BUTTONS.to_vec()));
        let close_index = alert.buttons.iter().position(|b| Some(*b) == alert.close).unwrap();
        assert_eq!(drop_action(close_index), DropAction::InsertPath);
        assert_eq!(drop_action(0), DropAction::Attach);
        assert_eq!(drop_action(7), DropAction::InsertPath, "a stale remembered index never attaches");
        assert!(matches!(alert.then, Then::DropChoice { .. }));
        let (supported, all) = (vec![p("C:\\a.txt")], vec![p("C:\\a.txt"), p("C:\\b.exe")]);
        assert_eq!(drop_result(DropAction::Attach, &supported, &all), (vec![p("C:\\a.txt")], vec![p("C:\\b.exe")]));
        assert_eq!(drop_result(DropAction::InsertPath, &supported, &all), (vec![], all.clone()));
    }

    #[test]
    fn drops_sort_by_the_model_and_never_probe_network_paths() {
        let is_dir = |q: &Path| q == Path::new("C:\\folder");
        let accepts = |name: &str| name.ends_with(".txt");
        let paths = [p("C:\\a.txt"), p("C:\\b.exe"), p("C:\\folder"), p("\\\\host\\share\\c.txt"), p("\\\\?\\UNC\\host\\share\\d.txt")];
        let (ok, no) = split_by_model(&paths, &is_dir, &accepts);
        assert_eq!(ok, vec![p("C:\\a.txt"), p("C:\\folder")]);
        assert_eq!(no, vec![p("C:\\b.exe"), p("\\\\host\\share\\c.txt"), p("\\\\?\\UNC\\host\\share\\d.txt")]);
        // A network path is never asked whether it is a directory.
        let probing = |q: &Path| -> bool { assert!(!crate::remote_prefs::is_unc(&q.to_string_lossy()), "probed {q:?}"); false };
        split_by_model(&paths, &probing, &accepts);
    }

    #[test]
    fn inserted_text_and_pending_lists_stay_tidy() {
        assert_eq!(append_text("", "C:\\a"), "C:\\a");
        assert_eq!(append_text("see", "C:\\a"), "see C:\\a");
        assert_eq!(append_text("see ", "C:\\a"), "see C:\\a");
        assert_eq!(append_text("see", ""), "see");
        let mut pending = vec![];
        add_paths(&mut pending, &[p("C:\\a"), p("C:\\a"), p("C:\\b")]);
        assert_eq!(pending, vec![Pending::Path(p("C:\\a")), Pending::Path(p("C:\\b"))]);
        assert_eq!(Pending::Path(p("C:\\x\\a.txt")).label(), "a.txt");
    }

    #[test]
    fn the_input_state_follows_iterm2() {
        let mut s = InputState::default();
        assert!(s.eligible, "a fresh input takes an offer");
        assert!(s.offer("ls -l"));
        assert_eq!(s.draft.text, "ls -l");
        // Typing (anything, even back to empty text) decides eligibility from the text.
        s.typed("ls -lx".into());
        assert!(!s.eligible);
        assert!(!s.offer("other"), "typed text is never replaced");
        assert_eq!(s.draft.text, "ls -lx");
        s.typed(String::new());
        assert!(s.eligible, "emptied by hand, it is eligible again");
        // Even an eligible input refuses an offer when something sits in it.
        s.draft.text = "x".into();
        assert!(!s.offer("y"));
        // Sent: cleared and eligible again.
        s.draft.pending.push(Pending::Path(p("C:\\a")));
        s.sent();
        assert_eq!((s.draft.clone(), s.eligible), (Draft::default(), true));
        // Edit staging replaces the text and protects it from an offer.
        s.set("edited");
        assert_eq!((s.draft.text.as_str(), s.eligible), ("edited", false));
        assert!(!s.offer("sel"));
        // A chat switch brings the draft and sets eligibility from its text.
        s.switched(Draft { text: "kept".into(), pending: vec![] });
        assert!(!s.eligible);
        s.switched(Draft::default());
        assert!(s.eligible);
        // Inserted paths and removed attachments.
        s.insert("C:\\a.txt");
        s.insert("C:\\b.txt");
        assert_eq!(s.draft.text, "C:\\a.txt C:\\b.txt");
        s.draft.pending = vec![Pending::Path(p("C:\\1")), Pending::Path(p("C:\\2"))];
        s.remove_attachment(0);
        s.remove_attachment(9);
        assert_eq!(s.draft.pending, vec![Pending::Path(p("C:\\2"))]);
        assert!(s.focus > 0, "set, offer and insert ask for the focus");
    }

    #[test]
    fn rejected_files_make_an_ok_alert() {
        let a = rejected_alert(&["x.exe".into()], Some("OpenAI"));
        assert_eq!((a.heading.as_str(), a.buttons.clone(), a.then.clone(), a.gate, a.silence.clone()), ("Attachment not supported", vec!["OK"], Then::Nothing, false, None));
    }

    #[test]
    fn folders_list_sorted_capped_and_filtered() {
        let root = std::env::temp_dir().join(format!("tandem-chat-input-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("sub")).unwrap();
        for (name, body) in [("b.txt", "B"), ("a.txt", "A"), ("skip.exe", "X"), ("sub\\c.txt", "C"), ("sub\\d.txt", "D")] {
            std::fs::write(root.join(name), body).unwrap();
        }
        let txt = |n: &str| n.ends_with(".txt");
        let rel = |v: Vec<PathBuf>| v.iter().map(|f| f.strip_prefix(&root).unwrap().to_string_lossy().to_string()).collect::<Vec<_>>();
        assert_eq!(rel(list_folder(&root, &txt, 10).unwrap()), ["a.txt", "b.txt", "sub\\c.txt", "sub\\d.txt"], "files first in each folder, only the accepted types");
        assert_eq!(rel(list_folder(&root, &txt, 3).unwrap()), ["a.txt", "b.txt", "sub\\c.txt"], "capped");
        assert_eq!(list_folder(&root.join("a.txt"), &txt, 10), None, "a file is not a folder");
        assert_eq!(list_folder(Path::new("\\\\host\\share\\d"), &txt, 10), None, "a network path is never read");
        assert_eq!(read_capped(&root.join("a.txt")), Some(b"A".to_vec()));
        assert_eq!(read_capped(&root), None, "a folder has no bytes");
        assert_eq!(read_capped(&root.join("missing.txt")), None);
        assert_eq!(read_capped(Path::new("\\\\host\\share\\a.txt")), None, "a network path is never read");
        let _ = std::fs::remove_dir_all(&root);
    }
}
