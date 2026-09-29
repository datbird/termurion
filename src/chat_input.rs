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
/// The deepest folder a listing enters, and the most entries it looks at and names as left out.
pub const FOLDER_DEPTH_MAX: usize = 16;
const FOLDER_ENTRIES_MAX: usize = 20_000;
const SKIPPED_KEPT_MAX: usize = 1000;
/// The names one notice spells out before it says how many more.
const NOTICE_NAMES_MAX: usize = 10;

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

/// Why a file was left out of a message (told to the person after the send).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    Unsupported,
    TooLarge,
    OverTotal,
    Unreadable,
    OverCount,
    TooDeep,
    /// A link or junction that leaves the folder.
    Link,
    /// A network location, or a link to one: never read.
    Network,
}

impl Reason {
    pub fn text(self) -> String {
        match self {
            Reason::Unsupported => "the model does not accept this type".to_string(),
            Reason::TooLarge => format!("larger than {} MiB", FILE_BYTES_MAX >> 20),
            Reason::OverTotal => format!("over the {} MiB one message can carry", TOTAL_BYTES_MAX >> 20),
            Reason::Unreadable => "could not be read".to_string(),
            Reason::OverCount => format!("past the {FOLDER_FILES_MAX} file limit"),
            Reason::TooDeep => format!("more than {FOLDER_DEPTH_MAX} folders down"),
            Reason::Link => "a link that leaves the folder".to_string(),
            Reason::Network => "a network location, never read".to_string(),
        }
    }
}

/// One file left out, by the name the person knows (relative to its folder).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Skipped {
    pub name: String,
    pub reason: Reason,
}

/// The files a folder gives, and what it left out.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Listing {
    pub files: Vec<PathBuf>,
    pub skipped: Vec<Skipped>,
}

/// What one send may read in all: a file counts only when it is kept, so one file too big for
/// what is left never blocks the smaller ones after it.
#[derive(Debug, Default)]
pub struct Budget {
    used: u64,
}

impl Budget {
    pub fn take(&mut self, n: u64, max: u64) -> bool {
        if self.used.saturating_add(n) > max {
            return false;
        }
        self.used += n;
        true
    }
}

/// The one notice a send shows when it left files out: each name (one line, no control
/// characters) with its reason, ten at most.
pub fn skipped_notice(list: &[Skipped]) -> String {
    let one_line = |name: &str| name.chars().map(|c| if c.is_control() { '?' } else { c }).collect::<String>();
    let mut items: Vec<String> = list.iter().take(NOTICE_NAMES_MAX).map(|s| format!("{} ({})", one_line(&s.name), s.reason.text())).collect();
    if list.len() > NOTICE_NAMES_MAX {
        items.push(format!("and {} more", list.len() - NOTICE_NAMES_MAX));
    }
    format!("These files were not sent to the model: {}.", items.join("; "))
}

/// The parts a pending list makes (`CVC:2269-2346`): a folder gives every regular file below
/// it named by its path relative to the folder, a file its own name, an in-memory item what it
/// carries. An unreadable file is skipped.
pub fn attachments(pending: &[Pending], read: &dyn Fn(&Path) -> Result<Vec<u8>, Reason>, list: &dyn Fn(&Path) -> Option<Listing>, mime: &dyn Fn(&str) -> String) -> (Vec<Subpart>, Vec<Skipped>) {
    let part = |name: &str, bytes: &[u8]| Subpart::Attachment(file_attachment(&new_id(), name, bytes, &mime(name)));
    let mut out = Vec::new();
    let mut skipped = Vec::new();
    for item in pending {
        match item {
            Pending::InMemory { name, content, mime } => out.push(Subpart::Attachment(file_attachment(&new_id(), name, content, mime))),
            Pending::Path(p) => match list(p) {
                Some(Listing { files, skipped: left_out }) => {
                    skipped.extend(left_out);
                    for f in files {
                        let Ok(rel) = f.strip_prefix(p) else { continue };
                        let name = rel.components().map(|c| c.as_os_str().to_string_lossy().to_string()).collect::<Vec<_>>().join("\\");
                        if name.is_empty() {
                            continue;
                        }
                        match read(&f) {
                            Ok(bytes) => out.push(part(&name, &bytes)),
                            Err(reason) => skipped.push(Skipped { name, reason }),
                        }
                    }
                }
                None => match read(p) {
                    Ok(bytes) => out.push(part(&file_name(p), &bytes)),
                    Err(reason) => skipped.push(Skipped { name: file_name(p), reason }),
                },
            },
        }
    }
    (out, skipped)
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
pub fn read_capped(path: &Path, max: u64, fits: &mut dyn FnMut(u64) -> bool) -> Result<Vec<u8>, Reason> {
    if crate::remote_prefs::is_unc(&path.to_string_lossy()) {
        return Err(Reason::Network);
    }
    let meta = std::fs::metadata(path).map_err(|_| Reason::Unreadable)?;
    if !meta.is_file() {
        return Err(Reason::Unreadable);
    }
    if meta.len() > max {
        return Err(Reason::TooLarge);
    }
    if !fits(meta.len()) {
        return Err(Reason::OverTotal);
    }
    std::fs::read(path).map_err(|_| Reason::Unreadable)
}

/// A path as comparable parts: the drive (a verbatim prefix names the same one), then each
/// name in lower case, `.` and `..` resolved.
fn parts(path: &Path) -> Vec<String> {
    use std::path::{Component, Prefix};
    let (mut out, mut base) = (Vec::new(), 0);
    for c in path.components() {
        match c {
            Component::Prefix(p) => {
                out.push(match p.kind() {
                    Prefix::VerbatimDisk(d) | Prefix::Disk(d) => format!("{}:", (d as char).to_ascii_lowercase()),
                    _ => p.as_os_str().to_string_lossy().to_lowercase(),
                });
                base += 1;
            }
            Component::RootDir => {
                out.push("\\".to_string());
                base += 1;
            }
            Component::CurDir => {}
            Component::ParentDir => {
                if out.len() > base {
                    out.pop();
                }
            }
            Component::Normal(n) => out.push(n.to_string_lossy().to_lowercase()),
        }
    }
    out
}

/// Whether `target` lies below `root` (compared by name, case-insensitively, `..` resolved;
/// nothing is read from the disk).
pub fn inside(root: &Path, target: &Path) -> bool {
    let (r, t) = (parts(root), parts(target));
    t.len() > r.len() && t.starts_with(&r)
}

/// `path` with `.` and `..` resolved by name.
fn clean(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Whether a directory entry is a symbolic link or a junction. Any other reparse point is an
/// ordinary file: a OneDrive or Files On-Demand placeholder is read (and fetched) like any file.
fn is_link(meta: &std::fs::Metadata) -> bool {
    meta.file_type().is_symlink()
}

/// A link inside a chosen folder is followed only when it ends at a local place below that
/// folder, by way of no other link (each hop is read by name, never opened, so a link to a
/// network share is never touched). Otherwise why it is not.
fn check_link(link: &Path, roots: &[PathBuf]) -> Result<(), Reason> {
    let mut cur = link.to_path_buf();
    for _ in 0..8 {
        let target = std::fs::read_link(&cur).map_err(|_| Reason::Unreadable)?;
        let abs = clean(&if target.is_absolute() { target } else { cur.parent().unwrap_or(Path::new("")).join(target) });
        if crate::remote_prefs::is_unc(&abs.to_string_lossy()) {
            return Err(Reason::Network);
        }
        let Some(root) = roots.iter().find(|r| inside(r, &abs)) else { return Err(Reason::Link) };
        // Every folder between the chosen one and the target is looked at from the top down,
        // and a link among them ends the walk.
        let mut between: Vec<&Path> = abs.ancestors().skip(1).take_while(|a| inside(root, a)).collect();
        between.reverse();
        for a in between {
            match std::fs::symlink_metadata(a) {
                Ok(m) if is_link(&m) => return Err(Reason::Link),
                Ok(_) => {}
                Err(_) => return Err(Reason::Unreadable),
            }
        }
        match std::fs::symlink_metadata(&abs) {
            Ok(m) if is_link(&m) => cur = abs,
            Ok(_) => return Ok(()),
            Err(_) => return Err(Reason::Unreadable),
        }
    }
    Err(Reason::Link)
}

/// The regular files below `dir`, sorted by path within each folder and folders after the
/// files beside them (`recursiveRegularFileIterator`), at most `max` of them whose name
/// `accepts`. What is left out is named with its reason: types the model does not take, files
/// past `max`, folders too deep, links that leave `dir`. None when `dir` is not a folder.
pub fn list_folder(dir: &Path, accepts: &dyn Fn(&str) -> bool, max: usize) -> Option<Listing> {
    if crate::remote_prefs::is_unc(&dir.to_string_lossy()) || !std::fs::metadata(dir).ok()?.is_dir() {
        return None;
    }
    let mut roots = vec![dir.to_path_buf()];
    roots.extend(std::fs::canonicalize(dir).ok());
    let rel = |p: &Path| p.strip_prefix(dir).map_or_else(|_| file_name(p), |r| r.to_string_lossy().to_string());
    let mut listing = Listing::default();
    let skip = |listing: &mut Listing, path: &Path, reason: Reason| {
        if listing.skipped.len() < SKIPPED_KEPT_MAX {
            listing.skipped.push(Skipped { name: rel(path), reason });
        }
    };
    let (mut stack, mut seen) = (vec![(dir.to_path_buf(), 0usize)], 0usize);
    while let Some((folder, depth)) = stack.pop() {
        let Ok(read) = std::fs::read_dir(&folder) else { continue };
        let mut entries: Vec<_> = read.filter_map(Result::ok).map(|e| e.path()).collect();
        entries.sort();
        let mut folders = Vec::new();
        for path in entries {
            seen += 1;
            if seen > FOLDER_ENTRIES_MAX {
                skip(&mut listing, Path::new("the rest of the folder"), Reason::OverCount);
                return Some(listing);
            }
            let Ok(meta) = std::fs::symlink_metadata(&path) else { continue };
            let (is_dir, is_file) = if is_link(&meta) {
                if let Err(reason) = check_link(&path, &roots) {
                    skip(&mut listing, &path, reason);
                    continue;
                }
                match std::fs::metadata(&path) {
                    Ok(m) => (m.is_dir(), m.is_file()),
                    Err(_) => {
                        skip(&mut listing, &path, Reason::Unreadable);
                        continue;
                    }
                }
            } else {
                (meta.is_dir(), meta.is_file())
            };
            if is_dir {
                if depth < FOLDER_DEPTH_MAX {
                    folders.push((path, depth + 1));
                } else {
                    skip(&mut listing, &path, Reason::TooDeep);
                }
            } else if is_file {
                if !accepts(&file_name(&path)) {
                    skip(&mut listing, &path, Reason::Unsupported);
                } else if listing.files.len() >= max {
                    skip(&mut listing, &path, Reason::OverCount);
                } else {
                    listing.files.push(path);
                }
            }
        }
        // The first folder is read next.
        stack.extend(folders.into_iter().rev());
    }
    Some(listing)
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

    /// The app replaced the text (Edit, a fork's staged message). Eligibility is left as it
    /// was: setting a field from code is no `textDidChange`, so a selection still replaces this
    /// text until the person types (`CVC:1063-1067`).
    pub fn set(&mut self, text: &str) {
        self.draft.text = text.to_string();
        self.focus += 1;
    }

    /// A selection offered as text (`offerSelectedText`): it replaces the text for as long as
    /// the person has not typed in the field.
    pub fn offer(&mut self, text: &str) -> bool {
        if !self.eligible {
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
        let read = |f: &Path| -> Result<Vec<u8>, Reason> {
            match f.file_name().and_then(|n| n.to_str()) {
                Some("a.txt") => Ok(b"A".to_vec()),
                Some("b.md") => Ok(b"B".to_vec()),
                Some("big.bin") => Err(Reason::TooLarge),
                _ => Err(Reason::Unreadable),
            }
        };
        let list = |d: &Path| -> Option<Listing> {
            (d == Path::new("C:\\d")).then(|| Listing {
                files: vec![p("C:\\d\\a.txt"), p("C:\\d\\sub\\b.md"), p("C:\\d\\gone.bin"), p("C:\\d\\big.bin")],
                skipped: vec![Skipped { name: "x.exe".into(), reason: Reason::Unsupported }],
            })
        };
        let mime = |n: &str| if n.ends_with(".md") { "text/markdown".to_string() } else { "text/plain".to_string() };
        let (parts, skipped) = attachments(&[Pending::Path(p("C:\\d"))], &read, &list, &mime);
        assert_eq!(
            skipped,
            vec![
                Skipped { name: "x.exe".into(), reason: Reason::Unsupported },
                Skipped { name: "gone.bin".into(), reason: Reason::Unreadable },
                Skipped { name: "big.bin".into(), reason: Reason::TooLarge },
            ],
            "a folder's own skips, then each file that could not be read, named as the person knows them"
        );
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
            "the unreadable files are left out of the message"
        );
        // A single file keeps its own name; an unreadable one gives nothing; in memory keeps everything.
        let (one, one_skipped) = attachments(&[Pending::Path(p("C:\\x\\a.txt")), Pending::Path(p("C:\\x\\nope.bin")), Pending::InMemory { name: "s.png".into(), content: vec![1, 2], mime: "image/png".into() }], &read, &|_| None, &mime);
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
        assert_eq!(one_skipped, vec![Skipped { name: "nope.bin".into(), reason: Reason::Unreadable }], "a single file that cannot be read is named too");
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
        // A second selection replaces the first for as long as nobody typed (`CVC:1063-1067`).
        assert!(s.offer("first"));
        assert!(s.offer("second"));
        assert_eq!(s.draft.text, "second");
        // Sent: cleared and eligible again.
        s.draft.pending.push(Pending::Path(p("C:\\a")));
        s.sent();
        assert_eq!((s.draft.clone(), s.eligible), (Draft::default(), true));
        // Edit staging replaces the text and leaves eligibility alone, as iTerm2 does (setting a
        // field from code is no textDidChange): a selection still replaces it until a key is typed.
        s.set("edited");
        assert_eq!((s.draft.text.as_str(), s.eligible), ("edited", true));
        assert!(s.offer("sel"));
        assert_eq!(s.draft.text, "sel");
        s.typed("sel!".into());
        s.set("edited again");
        assert!(!s.eligible, "typed text stays protected across a code change");
        assert!(!s.offer("nope"));
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
        let rel = |l: Listing| l.files.iter().map(|f| f.strip_prefix(&root).unwrap().to_string_lossy().to_string()).collect::<Vec<_>>();
        let all = list_folder(&root, &txt, 10).unwrap();
        assert_eq!(all.skipped, vec![Skipped { name: "skip.exe".into(), reason: Reason::Unsupported }], "a type the model does not take is reported");
        assert_eq!(rel(all), ["a.txt", "b.txt", "sub\\c.txt", "sub\\d.txt"], "files first in each folder, only the accepted types");
        let capped = list_folder(&root, &txt, 3).unwrap();
        assert_eq!(capped.skipped.iter().map(|s| (s.name.as_str(), s.reason)).collect::<Vec<_>>(), [("skip.exe", Reason::Unsupported), ("sub\\d.txt", Reason::OverCount)], "the files past the cap are named");
        assert_eq!(rel(capped), ["a.txt", "b.txt", "sub\\c.txt"], "capped");
        assert_eq!(list_folder(&root.join("a.txt"), &txt, 10), None, "a file is not a folder");
        assert_eq!(list_folder(Path::new("\\\\host\\share\\d"), &txt, 10), None, "a network path is never read");
        let mut always = |_: u64| true;
        assert_eq!(read_capped(&root.join("a.txt"), FILE_BYTES_MAX, &mut always), Ok(b"A".to_vec()));
        assert_eq!(read_capped(&root, FILE_BYTES_MAX, &mut always), Err(Reason::Unreadable), "a folder has no bytes");
        assert_eq!(read_capped(&root.join("missing.txt"), FILE_BYTES_MAX, &mut always), Err(Reason::Unreadable));
        assert_eq!(read_capped(Path::new("\\\\host\\share\\a.txt"), FILE_BYTES_MAX, &mut always), Err(Reason::Network), "a network path is never read");
        assert_eq!(read_capped(&root.join("a.txt"), 0, &mut always), Err(Reason::TooLarge), "over the single file limit");
        let mut never = |_: u64| false;
        assert_eq!(read_capped(&root.join("a.txt"), FILE_BYTES_MAX, &mut never), Err(Reason::OverTotal), "over what is left of the total");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_total_counts_only_the_files_kept() {
        let mut b = Budget::default();
        let max = 64 << 20;
        assert!(b.take(60 << 20, max));
        assert!(!b.take(10 << 20, max), "too big for what is left");
        assert!(b.take(1 << 20, max), "a small file after it still fits");
        assert!(b.take(3 << 20, max), "exactly to the limit");
        assert!(!b.take(1, max));
    }

    #[test]
    fn what_a_send_left_out_is_told_in_one_line_each() {
        for r in [Reason::Unsupported, Reason::TooLarge, Reason::OverTotal, Reason::Unreadable, Reason::OverCount, Reason::TooDeep, Reason::Link, Reason::Network] {
            assert!(!r.text().is_empty());
        }
        assert!(Reason::TooLarge.text().contains("32 MiB"));
        assert!(Reason::OverTotal.text().contains("64 MiB"));
        assert!(Reason::OverCount.text().contains("200"));
        let n = skipped_notice(&[Skipped { name: "a.exe".into(), reason: Reason::Unsupported }, Skipped { name: "b\nc\u{7}.bin".into(), reason: Reason::TooLarge }]);
        assert!(n.starts_with("These files were not sent to the model: "), "{n}");
        assert!(n.contains("a.exe (") && n.contains("b?c?.bin ("), "{n}");
        assert!(!n.contains('\n'), "one line");
        let many: Vec<Skipped> = (0..25).map(|i| Skipped { name: format!("f{i}.bin"), reason: Reason::OverCount }).collect();
        let n = skipped_notice(&many);
        assert!(n.contains("f9.bin") && !n.contains("f10.bin") && n.ends_with("and 15 more."), "{n}");
    }

    #[test]
    fn links_are_followed_only_inside_the_folder() {
        assert!(inside(Path::new("C:\\d"), Path::new("C:\\d\\sub\\x.txt")));
        assert!(inside(Path::new("C:\\D"), Path::new("c:\\d\\..\\d\\x")), "case and .. are resolved");
        assert!(!inside(Path::new("C:\\d"), Path::new("C:\\d\\..\\other\\x")));
        assert!(!inside(Path::new("C:\\d"), Path::new("C:\\dx\\y")), "a longer name is not a child");
        assert!(!inside(Path::new("C:\\d"), Path::new("C:\\d")), "the folder itself is not below it");
        assert!(!inside(Path::new("C:\\d"), Path::new("D:\\d\\x")));
        assert!(inside(Path::new("C:\\d"), Path::new("\\\\?\\C:\\d\\x")), "a verbatim path names the same place");
        assert!(!inside(Path::new("C:\\d"), Path::new("\\\\host\\share\\d\\x")));
    }

    #[cfg(windows)]
    #[test]
    fn a_junction_out_of_the_folder_is_skipped_and_reported() {
        let root = std::env::temp_dir().join(format!("tandem-chat-links-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (inner, outer) = (root.join("dropped"), root.join("elsewhere"));
        std::fs::create_dir_all(inner.join("real")).unwrap();
        std::fs::create_dir_all(&outer).unwrap();
        std::fs::write(inner.join("real\\in.txt"), "I").unwrap();
        std::fs::write(outer.join("out.txt"), "O").unwrap();
        let junction = |link: PathBuf, target: &Path| {
            let ok = std::process::Command::new("cmd").args(["/C", "mklink", "/J"]).arg(&link).arg(target).output().map(|o| o.status.success()).unwrap_or(false);
            assert!(ok, "mklink /J {link:?}");
        };
        junction(inner.join("to_out"), &outer);
        junction(inner.join("to_in"), &inner.join("real"));
        let txt = |n: &str| n.ends_with(".txt");
        let l = list_folder(&inner, &txt, 10).unwrap();
        let names: Vec<String> = l.files.iter().map(|f| f.strip_prefix(&inner).unwrap().to_string_lossy().to_string()).collect();
        assert!(names.contains(&"real\\in.txt".to_string()), "{names:?}");
        assert!(names.contains(&"to_in\\in.txt".to_string()), "a link to a folder inside is followed: {names:?}");
        assert!(!names.iter().any(|n| n.contains("out.txt")), "nothing from outside: {names:?}");
        assert!(l.skipped.contains(&Skipped { name: "to_out".into(), reason: Reason::Link }), "{:?}", l.skipped);
        let _ = std::fs::remove_dir_all(&root);
    }
}
