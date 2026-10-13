//! The chat input (iTerm2's `ChatInputView` and the send half of `ChatViewController`): when
//! Send is enabled, what Enter does, the placeholder and the hint, the message a send builds
//! (attachments included), the drop and rejection texts, and the drafts kept per chat. Pure.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::localfs::{self, LocalPath};

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
    /// Neither a regular file nor a folder (a device, a pipe).
    Special,
    /// The listing stopped at its entry limit; the rest of the folder was not looked at.
    EntryLimit,
    /// This many more names were left out than the notice keeps.
    More(usize),
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
            Reason::Special => "not a regular file or folder".to_string(),
            Reason::EntryLimit => format!("the folder holds more than {FOLDER_ENTRIES_MAX} entries, the rest were not looked at"),
            Reason::More(n) => format!("{n} more"),
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
    used: std::cell::Cell<u64>,
}

impl Budget {
    /// Whether `n` more bytes fit under `max`.
    pub fn fits(&self, n: u64, max: u64) -> bool {
        self.used.get().saturating_add(n) <= max
    }

    /// Counts `n` bytes when they fit.
    pub fn take(&self, n: u64, max: u64) -> bool {
        if !self.fits(n, max) {
            return false;
        }
        self.used.set(self.used.get() + n);
        true
    }

    pub fn used(&self) -> u64 {
        self.used.get()
    }
}

/// The one notice a send shows when it left files out: each name (one line, no control
/// characters) with its reason, ten at most.
pub fn skipped_notice(list: &[Skipped]) -> String {
    let one_line = |name: &str| name.chars().map(|c| if c.is_control() { '?' } else { c }).collect::<String>();
    // A `More` entry stands for names a listing only counted.
    let counted: usize = list.iter().map(|s| if let Reason::More(n) = s.reason { n } else { 0 }).sum();
    // Why a listing stopped comes first: it is never among the names the cap cuts off.
    let mut named: Vec<&Skipped> = list.iter().filter(|s| !matches!(s.reason, Reason::More(_))).collect();
    named.sort_by_key(|s| s.reason != Reason::EntryLimit);
    let mut items: Vec<String> = named.iter().take(NOTICE_NAMES_MAX).map(|s| format!("{} ({})", one_line(&s.name), s.reason.text())).collect();
    let more = named.len().saturating_sub(NOTICE_NAMES_MAX) + counted;
    if more > 0 {
        items.push(format!("and {more} more"));
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

/// (accepted, rejected) among paths: a folder when the model takes an upload of no known type
/// (its files are filtered as it is read), a network path never (it is not probed as a file; a drop of one inserts its text),
/// a file when the model accepts its MIME type.
pub fn split_by_model(paths: &[PathBuf], network: &dyn Fn(&Path) -> bool, is_dir: &dyn Fn(&Path) -> bool, accepts: &dyn Fn(&str) -> bool) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let (mut ok, mut no) = (Vec::new(), Vec::new());
    for p in paths {
        let text = p.to_string_lossy();
        if crate::remote_prefs::is_unc(&text) || network(p) {
            no.push(p.clone());
        } else if is_dir(p) {
            // A folder has no extension, and iTerm2 asks the vendor about "" (a type only an
            // upload-capable one takes, `LLMProvider.swift:165-212`).
            if accepts("") {
                ok.push(p.clone());
            } else {
                no.push(p.clone());
            }
        } else if accepts(&file_name(p)) {
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
pub fn read_capped(path: &Path, max: u64, budget: &Budget, total_max: u64) -> Result<Vec<u8>, Reason> {
    let path = LocalPath::judge(path).ok_or(Reason::Network)?;
    let meta = localfs::stat(&path).map_err(|_| Reason::Unreadable)?;
    if !meta.is_file {
        return Err(Reason::Unreadable);
    }
    if meta.len > max {
        return Err(Reason::TooLarge);
    }
    if !budget.fits(meta.len, total_max) {
        return Err(Reason::OverTotal);
    }
    // Counted once it is read: a file that fails to read takes nothing from the others.
    let bytes = localfs::read(&path).map_err(|_| Reason::Unreadable)?;
    budget.take(bytes.len() as u64, total_max);
    Ok(bytes)
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

/// A link inside a chosen folder is followed only when it ends at a local place below that
/// folder, by way of no other link (each hop is read by name, never opened, so a link to a
/// network share is never touched). Otherwise why it is not.
fn check_link(link: &Path, roots: &[PathBuf]) -> Result<(), Reason> {
    // The one judge first: every hop's target and its folders are local before any is read here.
    let mut cur = LocalPath::judge(link).ok_or(Reason::Network)?;
    let lstat = |p: &Path| LocalPath::judge(p).ok_or(Reason::Network).and_then(|p| localfs::lstat(&p).map_err(|_| Reason::Unreadable));
    for _ in 0..8 {
        let target = localfs::read_link(&cur).ok_or(Reason::Unreadable)?;
        let cur_path = cur.path().to_path_buf();
        let abs = clean(&if target.is_absolute() { target } else { cur_path.parent().unwrap_or(Path::new("")).join(target) });
        if crate::remote_prefs::is_unc(&abs.to_string_lossy()) {
            return Err(Reason::Network);
        }
        let Some(root) = roots.iter().find(|r| inside(r, &abs)) else { return Err(Reason::Link) };
        // Every folder between the chosen one and the target is looked at from the top down,
        // and a link among them ends the walk.
        let mut between: Vec<&Path> = abs.ancestors().skip(1).take_while(|a| inside(root, a)).collect();
        between.reverse();
        for a in between {
            if lstat(a)?.is_link {
                return Err(Reason::Link);
            }
        }
        if !lstat(&abs)?.is_link {
            return Ok(());
        }
        cur = LocalPath::judge(&abs).ok_or(Reason::Network)?;
    }
    Err(Reason::Link)
}

/// A dropped or picked item that is, or sits under, a link to a network location.
/// The one judge (`localfs::LocalPath`): a network path, a mapped or SUBST drive onto a share, or a
/// link to one anywhere on the way.
pub fn links_to_network(path: &Path) -> bool {
    LocalPath::judge(path).is_none()
}

/// The regular files below `dir`, sorted by path within each folder and folders after the
/// files beside them (`recursiveRegularFileIterator`), at most `max` of them whose name
/// `accepts`. What is left out is named with its reason: types the model does not take, files
/// past `max`, folders too deep, links that leave `dir`. None when `dir` is not a folder.
pub fn list_folder(dir: &Path, accepts: &dyn Fn(&str) -> bool, max: usize) -> Option<Listing> {
    // The item itself first, by its own entry: a link to a network path is never followed (a send
    // then reads it as a file and names it as one that was never read).
    let judged = LocalPath::judge(dir)?;
    if !localfs::stat(&judged).ok()?.is_dir {
        return None;
    }
    // A folder on the stack is judged before it is listed; its entries' kinds come from the listing
    // itself (no further I/O), so a plain entry of a judged folder is never probed again.
    let known = std::cell::RefCell::new(std::collections::HashMap::<PathBuf, Kind>::new());
    let of = |s: &localfs::Stat| Kind { link: s.is_link, dir: s.is_dir, file: s.is_file };
    let entries = |folder: &Path| -> std::io::Result<Vec<std::io::Result<PathBuf>>> {
        let listed = localfs::entries(&localfs::judge_io(folder)?)?;
        let mut known = known.borrow_mut();
        Ok(listed.into_iter().map(|e| {
            known.insert(e.path().to_path_buf(), of(&e.stat));
            Ok(e.path().to_path_buf())
        }).collect())
    };
    let kind = |path: &Path| match known.borrow().get(path) {
        Some(k) => Ok(*k),
        None => localfs::judge_io(path).and_then(|p| localfs::lstat(&p)).map(|s| of(&s)),
    };
    Some(list_with(dir, accepts, max, &Disk { entries: &entries, kind: &kind }))
}

/// What an entry is, by its own record (a link is not followed to find out).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Kind {
    pub link: bool,
    pub dir: bool,
    pub file: bool,
}

/// The two reads a listing makes of the disk that can fail, so a test can make each fail.
pub struct Disk<'a> {
    pub entries: &'a dyn Fn(&Path) -> std::io::Result<Vec<std::io::Result<PathBuf>>>,
    pub kind: &'a dyn Fn(&Path) -> std::io::Result<Kind>,
}

/// `list_folder` over a disk.
pub fn list_with(dir: &Path, accepts: &dyn Fn(&str) -> bool, max: usize, disk: &Disk) -> Listing {
    let mut roots = vec![dir.to_path_buf()];
    roots.extend(LocalPath::judge(dir).and_then(|d| localfs::canonicalize(&d)).map(LocalPath::into_path_buf));
    let rel = |p: &Path| {
        let r = p.strip_prefix(dir).map_or_else(|_| file_name(p), |r| r.to_string_lossy().to_string());
        if r.is_empty() { file_name(dir) } else { r }
    };
    let mut listing = Listing::default();
    // Past the kept limit a name is only counted, in one trailing `More` entry.
    let keep = |listing: &mut Listing, name: String, reason: Reason| {
        let at = listing.skipped.iter().position(|s| matches!(s.reason, Reason::More(_))).unwrap_or(listing.skipped.len());
        listing.skipped.insert(at, Skipped { name, reason });
    };
    let skip = |listing: &mut Listing, name: String, reason: Reason| {
        if listing.skipped.iter().filter(|s| !matches!(s.reason, Reason::More(_))).count() < SKIPPED_KEPT_MAX {
            keep(listing, name, reason);
        } else if let Some(Skipped { reason: Reason::More(n), .. }) = listing.skipped.last_mut() {
            *n += 1;
        } else {
            listing.skipped.push(Skipped { name: String::new(), reason: Reason::More(1) });
        }
    };
    // The real place of each folder on the way down: a link back to one of them is a loop and is
    // not entered (the same files again and again until a cap).
    let place = |p: &Path| LocalPath::judge(p).and_then(|p| localfs::canonicalize(&p)).map_or_else(|| p.to_path_buf(), LocalPath::into_path_buf).to_string_lossy().to_lowercase();
    let (mut stack, mut seen) = (vec![(dir.to_path_buf(), 0usize, Vec::<String>::new())], 0usize);
    while let Some((folder, depth, mut above)) = stack.pop() {
        above.push(place(&folder));
        let read = match (disk.entries)(&folder) {
            Ok(read) => read,
            Err(_) => {
                skip(&mut listing, rel(&folder), Reason::Unreadable);
                continue;
            }
        };
        let (mut entries, mut errored) = (Vec::new(), 0usize);
        for e in read {
            match e {
                Ok(path) => entries.push(path),
                Err(_) => errored += 1,
            }
        }
        for _ in 0..errored {
            skip(&mut listing, format!("an entry in {}", rel(&folder)), Reason::Unreadable);
        }
        entries.sort();
        let mut folders = Vec::new();
        for path in entries {
            seen += 1;
            if seen > FOLDER_ENTRIES_MAX {
                // Why the listing stopped is always told, whatever else was only counted.
                keep(&mut listing, "the rest of the folder".to_string(), Reason::EntryLimit);
                return listing;
            }
            let meta = match (disk.kind)(&path) {
                Ok(meta) => meta,
                Err(_) => {
                    skip(&mut listing, rel(&path), Reason::Unreadable);
                    continue;
                }
            };
            let (is_dir, is_file) = if meta.link {
                if let Err(reason) = check_link(&path, &roots) {
                    skip(&mut listing, rel(&path), reason);
                    continue;
                }
                match localfs::judge_io(&path).and_then(|p| localfs::stat(&p)) {
                    Ok(m) => (m.is_dir, m.is_file),
                    Err(_) => {
                        skip(&mut listing, rel(&path), Reason::Unreadable);
                        continue;
                    }
                }
            } else {
                (meta.dir, meta.file)
            };
            if is_dir {
                if meta.link && above.contains(&place(&path)) {
                    skip(&mut listing, rel(&path), Reason::Link);
                } else if depth < FOLDER_DEPTH_MAX {
                    folders.push((path, depth + 1, above.clone()));
                } else {
                    skip(&mut listing, rel(&path), Reason::TooDeep);
                }
            } else if is_file {
                if !accepts(&file_name(&path)) {
                    skip(&mut listing, rel(&path), Reason::Unsupported);
                } else if listing.files.len() >= max {
                    skip(&mut listing, rel(&path), Reason::OverCount);
                } else {
                    listing.files.push(path);
                }
            } else {
                skip(&mut listing, rel(&path), Reason::Special);
            }
        }
        // The first folder is read next.
        stack.extend(folders.into_iter().rev());
    }
    listing
}

/// What a chat's input holds.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Draft {
    pub text: String,
    pub pending: Vec<Pending>,
    /// The grants of this draft: sessions the person picked in the `@` picker, by stable id. A
    /// pick is a token in the text where the query was, and an edit inside it drops the grant.
    /// Text that was typed, pasted, dropped, offered, set by the app or restored holds tokens
    /// but grants nothing.
    pub mentions: Vec<String>,
}

impl Draft {
    pub fn is_empty(&self) -> bool {
        self.text.is_empty() && self.pending.is_empty() && self.mentions.is_empty()
    }
}

/// What a send grants the chat control of: only ids this draft's own picks
/// recorded (`Draft::mentions`) that are still tokens in the text. Task 10's
/// `orch_grant_mentions` takes this and never parses a stored message.
pub fn send_grants(draft: &Draft) -> Vec<String> {
    crate::mention::send_grants(&draft.text, &draft.mentions)
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
            // A kept draft keeps its text but no grants: they were the person's act in that draft.
            let kept = Draft { mentions: Vec::new(), ..current.clone() };
            if kept.is_empty() {
                self.kept.remove(from);
            } else {
                self.kept.insert(from.to_string(), kept);
            }
        }
        match to {
            Some(to) => self.kept.remove(to).unwrap_or_default(),
            None => current,
        }
    }

    /// An attachment for a chat that is not the one shown (it joins that chat's kept draft).
    pub fn attach(&mut self, chat: &str, p: Pending) {
        self.kept.entry(chat.to_string()).or_default().pending.push(p);
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
    /// The `@` picker under the field (orchestration chats only), and the "@query" an Escape closed.
    pub picker: Option<crate::mention::Picker>,
    pub dismissed: Option<(usize, String)>,
}

impl Default for InputState {
    fn default() -> InputState {
        InputState { draft: Draft::default(), eligible: true, focus: 0, picker: None, dismissed: None }
    }
}

impl InputState {
    /// The person typed: eligible exactly while the field is empty (`textDidChange`). A grant
    /// survives only while its picked token is still whole in the text: any edit inside a token
    /// removes the grant and the token is plain text from then on, and typing or pasting a token
    /// never grants (the Task 5 fix round 2 ruling: only a pick in the picker grants).
    pub fn typed(&mut self, text: String) {
        self.eligible = text.is_empty();
        let whole = crate::mention::session_tokens(&text);
        self.draft.mentions.retain(|id| whole.contains(id));
        self.draft.text = text;
    }

    /// The app replaced the text (Edit, a fork's staged message). Eligibility is left as it
    /// was: setting a field from code is no `textDidChange`, so a selection still replaces this
    /// text until the person types (`CVC:1063-1067`).
    pub fn set(&mut self, text: &str) {
        self.draft.text = text.to_string();
        self.draft.mentions.clear();
        self.focus += 1;
    }

    /// A selection offered as text (`offerSelectedText`): it replaces the text for as long as
    /// the person has not typed in the field.
    pub fn offer(&mut self, text: &str) -> bool {
        if !self.eligible {
            return false;
        }
        self.draft.text = text.to_string();
        self.draft.mentions.clear();
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
        self.draft = Draft { mentions: Vec::new(), ..draft };
        self.picker = None;
        self.dismissed = None;
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

    /// The text moved the picker (`CW:985-1034`): it follows the first "@" at a word start while
    /// `orchestrating`, with that query, and stays closed after an Escape until the target changes.
    /// A picker already open for the same query is kept, so the choice does not jump.
    pub fn follow_mention(&mut self, orchestrating: bool, world: &crate::cockpit_rows::World) {
        let raw = crate::mention::open_target(&self.draft.text);
        if self.dismissed != raw {
            self.dismissed = None;
        }
        let Some((_, query)) = raw.filter(|_| orchestrating && self.dismissed.is_none()) else {
            self.picker = None;
            return;
        };
        if self.picker.as_ref().is_some_and(|p| p.query == query) {
            return;
        }
        let mut p = crate::mention::Picker::new(world, &query);
        // Window and tab rows only group sessions here, so the first session is the choice.
        p.selected = p.items.iter().position(|i| matches!(i.chip, crate::mention::Chip::Session { .. })).unwrap_or(0);
        self.picker = Some(p);
    }

    /// Commits the chosen item (Return, Tab or a click): a session's `@query` text leaves the field and
    /// its id joins the chips. A window or tab row only groups sessions and commits nothing.
    pub fn commit_mention(&mut self, index: Option<usize>) -> bool {
        let Some(p) = self.picker.as_mut() else { return false };
        if let Some(i) = index.filter(|i| *i < p.items.len()) {
            p.selected = i;
        }
        let Some(crate::mention::Chip::Session { stable_id }) = p.current().map(|i| i.chip.clone()) else { return false };
        if let Some(text) = crate::mention::place_token(&self.draft.text, &format!("@{stable_id}")) {
            self.draft.text = text;
        }
        if !self.draft.mentions.contains(&stable_id) {
            self.draft.mentions.push(stable_id);
        }
        self.picker = None;
        self.focus += 1;
        true
    }

    /// Escape: the picker closes and stays closed for this "@query".
    pub fn close_mention(&mut self) {
        self.dismissed = crate::mention::open_target(&self.draft.text);
        self.picker = None;
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
        let a = Draft { text: "a".into(), pending: vec![Pending::Path(p("C:\\f.txt"))], ..Default::default() };
        assert_eq!(d.switch(Some("1"), a.clone(), Some("2")), Draft::default(), "chat 2 starts empty");
        let b = Draft { text: "b".into(), pending: vec![], ..Default::default() };
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
        let (ok, no) = split_by_model(&paths, &|_| false, &is_dir, &accepts);
        // A folder has no extension, and iTerm2 asks about "": only a model that takes any
        // upload takes a folder (LLMProvider.swift:165-212), so this text-only one does not.
        assert_eq!(ok, vec![p("C:\\a.txt")]);
        assert_eq!(no, vec![p("C:\\b.exe"), p("C:\\folder"), p("\\\\host\\share\\c.txt"), p("\\\\?\\UNC\\host\\share\\d.txt")]);
        let (ok, no) = split_by_model(&[p("C:\\folder"), p("C:\\b.exe")], &|_| false, &is_dir, &|_| true);
        assert_eq!((ok, no), (vec![p("C:\\folder"), p("C:\\b.exe")], vec![]), "a model that takes any upload takes the folder");
        // A network path is never asked whether it is a directory.
        let probing = |q: &Path| -> bool { assert!(!crate::remote_prefs::is_unc(&q.to_string_lossy()), "probed {q:?}"); false };
        split_by_model(&paths, &|_| false, &probing, &accepts);
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
        s.switched(Draft { text: "kept".into(), pending: vec![], ..Default::default() });
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
        let root = std::env::temp_dir().join(format!("termurion-chat-input-{}", std::process::id()));
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
        let room = Budget::default();
        let total = TOTAL_BYTES_MAX;
        assert_eq!(read_capped(&root.join("a.txt"), FILE_BYTES_MAX, &room, total), Ok(b"A".to_vec()));
        assert_eq!(read_capped(&root, FILE_BYTES_MAX, &room, total), Err(Reason::Unreadable), "a folder has no bytes");
        assert_eq!(read_capped(&root.join("missing.txt"), FILE_BYTES_MAX, &room, total), Err(Reason::Unreadable));
        assert_eq!(read_capped(Path::new("\\\\host\\share\\a.txt"), FILE_BYTES_MAX, &room, total), Err(Reason::Network), "a network path is never read");
        assert_eq!(read_capped(&root.join("a.txt"), 0, &room, total), Err(Reason::TooLarge), "over the single file limit");
        assert_eq!(read_capped(&root.join("a.txt"), FILE_BYTES_MAX, &room, 0), Err(Reason::OverTotal), "over what is left of the total");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_total_counts_only_the_files_kept() {
        let b = Budget::default();
        let max = 64 << 20;
        assert!(b.take(60 << 20, max));
        assert!(!b.take(10 << 20, max), "too big for what is left");
        assert!(b.take(1 << 20, max), "a small file after it still fits");
        assert!(b.take(3 << 20, max), "exactly to the limit");
        assert!(!b.take(1, max));
    }

    #[test]
    fn what_a_send_left_out_is_told_in_one_line_each() {
        for r in [Reason::Unsupported, Reason::TooLarge, Reason::OverTotal, Reason::Unreadable, Reason::OverCount, Reason::TooDeep, Reason::Link, Reason::Network, Reason::Special, Reason::EntryLimit, Reason::More(3)] {
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
        let root = std::env::temp_dir().join(format!("termurion-chat-links-{}", std::process::id()));
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

    /// Round 3: a dropped or picked item is judged by the one judge (`netpath::judge`), the same
    /// scenarios the old per-module walk had: the item or a folder above it a link to a share, a chain
    /// ending at one, a loop, a hop whose own folder is a link to a share; nothing below such a link
    /// is read.
    #[test]
    fn a_dropped_item_that_links_to_a_network_path_is_never_touched() {
        let links: std::collections::HashMap<&str, &str> = [
            ("C:\\drop\\share", "\\\\host\\share\\x"),
            ("C:\\drop\\hop", "C:\\drop\\share"),
            ("C:\\up", "\\\\?\\UNC\\host\\share"),
            ("C:\\drop\\local", "C:\\elsewhere\\f"),
            ("C:\\drop\\loop", "C:\\drop\\loop"),
            ("C:\\mnt", "\\\\host\\s"),
            ("C:\\drop\\x", "C:\\mnt\\f"),
            ("C:\\drop\\odd", crate::netpath::UNREADABLE_LINK),
        ]
        .into_iter()
        .collect();
        let asked = std::cell::RefCell::new(Vec::<String>::new());
        let read_link = |q: &Path| {
            asked.borrow_mut().push(q.to_string_lossy().to_string());
            links.get(q.to_string_lossy().as_ref()).map(PathBuf::from)
        };
        let local = |t: &str| crate::netpath::judge(Path::new(t), &read_link, &crate::netpath::LocalDrives);
        for t in ["C:\\drop\\share", "C:\\drop\\hop", "C:\\up\\f.txt", "C:\\drop\\loop", "C:\\drop\\x", "C:\\drop\\odd", "C:\\drop\\odd\\f.txt"] {
            assert!(!local(t), "{t}");
        }
        assert!(local("C:\\drop\\local") && local("C:\\drop\\plain.txt"));
        assert!(!asked.borrow().iter().any(|a| a.starts_with("\\\\") || a.starts_with("C:\\up\\") || a.starts_with("C:\\mnt\\")), "{:?}", asked.borrow());
        assert!(links_to_network(Path::new("\\\\host\\share\\x")) && links_to_network(Path::new("relative.txt")));
        // A drop sorts such an item out before it is asked whether it is a folder.
        let paths = [p("C:\\drop\\share"), p("C:\\a.txt")];
        let network = |q: &Path| q == Path::new("C:\\drop\\share");
        let probing = |q: &Path| -> bool { assert_ne!(q, Path::new("C:\\drop\\share"), "probed the link"); false };
        let (ok, no) = split_by_model(&paths, &network, &probing, &|_| true);
        assert_eq!((ok, no), (vec![p("C:\\a.txt")], vec![p("C:\\drop\\share")]));
    }

    fn io_err() -> std::io::Error {
        std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied")
    }

    #[test]
    fn a_folder_names_everything_it_leaves_out() {
        // root: ok.txt, locked (cannot be opened), a bad entry, gone (its record cannot be read), pipe (neither
        // a file nor a folder); locked holds a hidden file nobody could see.
        let file = Kind { link: false, dir: false, file: true };
        let dir = Kind { link: false, dir: true, file: false };
        let entries = |q: &Path| -> std::io::Result<Vec<std::io::Result<PathBuf>>> {
            match q.to_string_lossy().as_ref() {
                "C:\\d" => Ok(vec![Ok(p("C:\\d\\ok.txt")), Ok(p("C:\\d\\locked")), Err(io_err()), Ok(p("C:\\d\\gone")), Ok(p("C:\\d\\pipe"))]),
                _ => Err(io_err()),
            }
        };
        let kind = |q: &Path| -> std::io::Result<Kind> {
            match q.to_string_lossy().as_ref() {
                "C:\\d\\ok.txt" => Ok(file),
                "C:\\d\\locked" => Ok(dir),
                "C:\\d\\pipe" => Ok(Kind { link: false, dir: false, file: false }),
                _ => Err(io_err()),
            }
        };
        let l = list_with(Path::new("C:\\d"), &|_| true, 10, &Disk { entries: &entries, kind: &kind });
        assert_eq!(l.files, vec![p("C:\\d\\ok.txt")]);
        let named: Vec<(String, Reason)> = l.skipped.iter().map(|s| (s.name.clone(), s.reason)).collect();
        assert!(named.contains(&("locked".to_string(), Reason::Unreadable)), "an unopenable subfolder: {named:?}");
        assert!(named.contains(&("gone".to_string(), Reason::Unreadable)), "an entry whose record fails: {named:?}");
        assert!(named.contains(&("pipe".to_string(), Reason::Special)), "neither file nor folder: {named:?}");
        assert!(named.contains(&("an entry in d".to_string(), Reason::Unreadable)), "an entry that errored is named: {named:?}");
        assert_eq!(named.len(), 4, "{named:?}");
        // The folder itself that cannot be opened is named too.
        let l = list_with(Path::new("C:\\nope"), &|_| true, 10, &Disk { entries: &entries, kind: &kind });
        assert_eq!(l.skipped.len(), 1, "{:?}", l.skipped);
    }

    #[test]
    fn the_limits_are_told_in_their_own_words_and_counted() {
        // 1,100 files that are not taken: the notice names ten and counts every one left.
        let many = Disk {
            entries: &|q: &Path| Ok(if q == Path::new("C:\\d") { (0..1100).map(|i| Ok(p(&format!("C:\\d\\f{i:04}.bin")))).collect() } else { vec![] }),
            kind: &|_: &Path| Ok(Kind { link: false, dir: false, file: true }),
        };
        let l = list_with(Path::new("C:\\d"), &|_| false, 10, &many);
        let n = skipped_notice(&l.skipped);
        assert!(n.ends_with("and 1090 more."), "{n}");
        // More than the entry limit stops the listing with a reason about entries, not the file limit.
        let huge = Disk {
            entries: &|q: &Path| Ok(if q == Path::new("C:\\d") { (0..FOLDER_ENTRIES_MAX + 5).map(|i| Ok(p(&format!("C:\\d\\f{i}.bin")))).collect() } else { vec![] }),
            kind: &|_: &Path| Ok(Kind { link: false, dir: false, file: true }),
        };
        let l = list_with(Path::new("C:\\d"), &|_| true, 5, &huge);
        let stop = l.skipped.iter().find(|s| s.reason == Reason::EntryLimit).expect("the stop is named even when others were only counted");
        assert!(stop.reason.text().contains("20000") && !stop.reason.text().contains("200 file"), "{}", stop.reason.text());
        assert!(skipped_notice(&l.skipped).contains("rest of the folder"));
    }

    #[cfg(windows)]
    #[test]
    fn a_failed_read_does_not_use_up_the_total() {
        use std::os::windows::fs::OpenOptionsExt;
        let root = std::env::temp_dir().join(format!("termurion-chat-lock-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let locked = root.join("locked.txt");
        std::fs::write(&locked, "0123456789").unwrap();
        // Held with no sharing: the metadata is readable, the read fails.
        let _hold = std::fs::OpenOptions::new().read(true).write(true).share_mode(0).open(&locked).unwrap();
        let budget = Budget::default();
        assert_eq!(read_capped(&locked, FILE_BYTES_MAX, &budget, TOTAL_BYTES_MAX), Err(Reason::Unreadable));
        assert_eq!(budget.used(), 0, "a file that was not read counts for nothing");
        drop(_hold);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(windows)]
    #[test]
    fn a_folder_link_back_to_its_parent_is_not_followed_again() {
        let root = std::env::temp_dir().join(format!("termurion-chat-loop-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let top = root.join("d");
        std::fs::create_dir_all(top.join("a")).unwrap();
        std::fs::write(top.join("a\\x.txt"), "X").unwrap();
        let ok = std::process::Command::new("cmd").args(["/C", "mklink", "/J"]).arg(top.join("a\\loop")).arg(top.join("a")).output().map(|o| o.status.success()).unwrap_or(false);
        assert!(ok, "mklink /J");
        let l = list_folder(&top, &|n: &str| n.ends_with(".txt"), 200).unwrap();
        let names: Vec<String> = l.files.iter().map(|f| f.strip_prefix(&top).unwrap().to_string_lossy().to_string()).collect();
        assert_eq!(names, ["a\\x.txt"], "the same files are not attached again through the loop");
        assert!(l.skipped.contains(&Skipped { name: "a\\loop".into(), reason: Reason::Link }), "{:?}", l.skipped);
        let _ = std::fs::remove_dir_all(&root);
    }

    fn world_with(id: &str) -> crate::cockpit_rows::World {
        use crate::cockpit_rows::{SessionFact, TabFact, WindowFact};
        let fact = SessionFact { pane: 1, stable_id: id.into(), name: "build".into(), status: Default::default(), exited: false, alt_screen: false, ancestors: Vec::new(), armed: false, role: None };
        crate::cockpit_rows::World { windows: vec![WindowFact { guid: "g".into(), number: 1, title: String::new(), active_tab: 0, armed: false, tabs: vec![TabFact { id: 5, title: String::new(), sessions: vec![fact] }] }], buried: Vec::new(), ..Default::default() }
    }

    #[test]
    fn mentions_ride_the_draft() {
        const ID: &str = "ptys_0123456789ABW";
        let world = world_with(ID);
        let mut s = InputState::default();
        s.typed("ask @bu".into());
        // Only an orchestration chat has the picker.
        s.follow_mention(false, &world);
        assert!(s.picker.is_none());
        s.follow_mention(true, &world);
        let p = s.picker.as_ref().expect("the picker opens for the query");
        assert_eq!(p.query, "bu");
        // The window and tab rows group sessions; the choice starts on the session.
        assert_eq!(p.current().map(|i| i.label.as_str()), Some("build"));
        // Escape closes it for this query only.
        s.close_mention();
        s.follow_mention(true, &world);
        assert!(s.picker.is_none());
        s.typed("ask @b".into());
        s.follow_mention(true, &world);
        assert!(s.picker.is_some());
        // Return commits a session: the typed query leaves the text and the id is a chip.
        assert!(s.commit_mention(None));
        assert_eq!((s.draft.text.as_str(), s.draft.mentions.clone()), ("ask @ptys_0123456789ABW ", vec![ID.to_string()]));
        assert!(s.picker.is_none());
        assert!(!s.draft.is_empty());
        // A window row commits nothing.
        s.typed("@".into());
        s.follow_mention(true, &world);
        assert!(!s.commit_mention(Some(0)), "item 0 is the window");
        assert!(s.draft.mentions.is_empty(), "the field was retyped: the earlier pick is gone with its token");
        assert!(s.commit_mention(Some(2)));
        assert_eq!(s.draft.mentions.len(), 1, "the same session is one chip");
        // Switching chats drops the open picker; a sent draft is clean.
        s.typed("@x".into());
        s.follow_mention(true, &world);
        s.switched(Draft::default());
        assert!(s.picker.is_none());
        s.draft.mentions.push(ID.into());
        s.sent();
        assert!(s.draft.is_empty());
    }

    fn valid_id() -> String {
        crate::stable_id::generate()
    }

    #[test]
    fn pasted_text_grants_nothing() {
        let id = valid_id();
        let mut s = InputState::default();
        // A paste (or a drop, an offered selection, a script-set text) holds a token, never a grant.
        s.typed(format!("see @{id}"));
        assert!(s.draft.mentions.is_empty());
        assert!(send_grants(&s.draft).is_empty());
        s.set(&format!("see @{id}"));
        assert!(send_grants(&s.draft).is_empty());
        let mut offered = InputState::default();
        assert!(offered.offer(&format!("@{id}")));
        assert!(send_grants(&offered.draft).is_empty());
        s.insert(&format!("@{id}"));
        assert!(send_grants(&s.draft).is_empty());
        // Typed keystrokes that form a token grant nothing (`only_a_pick_grants` covers a pick).
        let mut s = InputState::default();
        let typed = format!("@{id}");
        for end in 1..=typed.len() {
            s.typed(typed[..end].to_string());
        }
        assert!(send_grants(&s.draft).is_empty());
        let other = valid_id();
        s.typed(format!("{typed} @{other}"));
        assert!(send_grants(&s.draft).is_empty(), "a pasted token beside a typed one");
        s.typed("gone".into());
        assert!(send_grants(&s.draft).is_empty());
    }

    #[test]
    fn a_restored_draft_grants_nothing() {
        let id = valid_id();
        let draft = Draft { text: format!("run @{id}"), pending: Vec::new(), mentions: vec![id.clone()] };
        let mut drafts = Drafts::default();
        drafts.switch(Some("a"), draft.clone(), Some("b"));
        let back = drafts.switch(Some("b"), Draft::default(), Some("a"));
        assert_eq!(back.text, draft.text, "the text is kept");
        assert!(back.mentions.is_empty());
        assert!(send_grants(&back).is_empty());
        let mut s = InputState::default();
        s.switched(draft);
        assert!(send_grants(&s.draft).is_empty());
    }

    #[test]
    fn a_picked_mention_is_a_token_in_place() {
        let world = world_with("ptys_0123456789ABW");
        let id = "ptys_0123456789ABW";
        let mut s = InputState::default();
        s.typed("compare @bu with @x".into());
        s.follow_mention(true, &world);
        assert_eq!(s.picker.as_ref().map(|p| p.query.as_str()), Some("bu"));
        assert!(s.commit_mention(None));
        // The token takes the query's place, so the sentence keeps its order and shows the grant.
        assert_eq!(s.draft.text, format!("compare @{id} with @x"));
        assert_eq!(s.draft.mentions, vec![id.to_string()]);
        assert_eq!(send_grants(&s.draft), vec![id.to_string()]);
        // The next target is the open "@x", not the token just placed.
        s.follow_mention(true, &world);
        assert_eq!(s.picker.as_ref().map(|p| p.query.as_str()), Some("x"));
        // At the end of the text a space follows, so the person can go on typing.
        let mut s = InputState::default();
        s.typed("ask @bu".into());
        s.follow_mention(true, &world);
        assert!(s.commit_mention(None));
        assert_eq!(s.draft.text, format!("ask @{id} "));
        s.follow_mention(true, &world);
        assert!(s.picker.is_none(), "a placed token does not reopen the picker");
    }

    /// Only a pick in the `@` picker grants (the Task 5 fix round 2 ruling): a token typed, pasted or
    /// edited into validity is plain text, and an edit inside a picked token drops its grant.
    #[test]
    fn only_a_pick_grants() {
        let id = valid_id();
        // Paste an id with a trailing character, then Backspace: the token appears, nothing is granted.
        let mut s = InputState::default();
        s.typed(format!("run @{id}x"));
        s.typed(format!("run @{id}"));
        assert!(send_grants(&s.draft).is_empty());
        assert!(s.draft.mentions.is_empty());
        // Paste an id missing its last character, then type it: nothing is granted.
        let mut s = InputState::default();
        let short = &id[..id.len() - 1];
        s.typed(format!("run @{short}"));
        s.typed(format!("run @{id}"));
        assert!(send_grants(&s.draft).is_empty());
        // Every keystroke of a whole token still grants nothing.
        let mut s = InputState::default();
        let typed = format!("@{id}");
        for end in 1..=typed.len() {
            s.typed(typed[..end].to_string());
        }
        assert!(send_grants(&s.draft).is_empty(), "typing is not a pick");
        // A pick grants; an edit inside it removes the grant and a retype does not bring it back.
        let world = world_with(&id);
        let mut s = InputState::default();
        s.typed("ask @bu".into());
        s.follow_mention(true, &world);
        assert!(s.commit_mention(None));
        assert_eq!(send_grants(&s.draft), vec![id.clone()]);
        let picked = s.draft.text.clone();
        s.typed(format!("{} more", picked.trim_end()));
        assert_eq!(send_grants(&s.draft), vec![id.clone()], "an edit outside the token keeps it");
        let cut = picked.trim_end().strip_suffix(&id[id.len() - 1..]).unwrap().to_string();
        s.typed(cut);
        assert!(send_grants(&s.draft).is_empty(), "Backspace inside the token");
        s.typed(picked.trim_end().to_string());
        assert!(send_grants(&s.draft).is_empty(), "typing it back is not a pick");
        assert!(s.draft.mentions.is_empty());
    }
}
