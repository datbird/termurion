//! The kitty drag-and-drop protocol (OSC 72), the local accept-drop direction (13o Task 9).
//! A program registers for drops (`t=a`); the window reports a drag over the pane (`t=m`) and
//! the drop (`t=M`); the program asks for the dropped data (`t=r`) and ends the drop with a
//! request that names nothing. Pure: the app hands in cells, pixels and the dropped files.
//!
//! Security (B7, P12): the program controls every byte it sends, and the replies reach its
//! input exactly like a DA answer. So no reply echoes program text: keys are one ASCII letter,
//! values and payloads keep only a short whitelist, error payloads are fixed words, data is
//! base64. Chunks, the MIME list, the files of a drop, the bytes read and served and the life
//! of a drop are all bounded.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub const MIME_URI_LIST: &str = "text/uri-list";

/// iTerm2's bounds on a registration list (`KC:171-172`): 64 KiB of text and 4,096 types.
const MAX_MIME_LIST_BYTES: usize = 64 * 1024;
const MAX_MIME_TYPES: usize = 4096;
/// One reassembled message and its chunks (B7); iTerm2 allows 512 MiB.
const MAX_MESSAGE_BYTES: usize = 1 << 20;
const MAX_CHUNKS: usize = 256;
/// What one drop carries and what is read from and served for it (B7).
const MAX_DROP_FILES: usize = 256;
pub const MAX_FILE_BYTES: usize = 16 << 20;
const MAX_DROP_BYTES: usize = 64 << 20;
const DROP_LIFETIME: Duration = Duration::from_secs(60);
/// A value, a payload word or an echoed id is cut to this many characters.
const MAX_ECHO: usize = 64;
/// Metadata keys kept from one message.
const MAX_KEYS: usize = 32;
/// Raw bytes per data chunk: their base64 is exactly 4,096 characters (`KittyDnDChunker`).
const CHUNK_RAW: usize = 3072;
/// The operation the window offers a program: copy (a dropped file is copied, never moved).
const OP_COPY: i64 = 1;

/// One OSC 72 message: `metadata [; payload]`. Metadata is `key=value` pairs joined by `:`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    pub metadata: BTreeMap<String, String>,
    pub payload: Option<String>,
}

/// Splits a message body into its metadata pairs and its payload text.
fn split(body: &str) -> (BTreeMap<String, String>, Option<String>) {
    let (meta, payload) = match body.split_once(';') {
        Some((m, p)) => (m, Some(p.to_string())),
        None => (body, None),
    };
    let mut metadata = BTreeMap::new();
    for token in meta.split(':').take(MAX_KEYS * 2) {
        let Some((key, value)) = token.split_once('=') else { continue };
        // A key is one ASCII letter: anything else is neither read nor echoed.
        if key.len() == 1 && key.as_bytes()[0].is_ascii_alphabetic() && metadata.len() < MAX_KEYS {
            metadata.insert(key.to_string(), value.to_string());
        }
    }
    (metadata, payload)
}

/// A metadata value as the program may see it again: `[A-Za-z0-9_.-]`, 64 characters.
fn value_echo(value: &str) -> String {
    value.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-')).take(MAX_ECHO).collect()
}

/// A plain-text payload as the program may see it: MIME lists and base64 only.
fn payload_echo(payload: &str) -> String {
    payload.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '+' | '=' | '.' | '_' | '-' | ' ')).collect()
}

impl Message {
    /// Reads the text between `OSC 72 ;` and the terminator.
    pub fn parse(body: &str) -> Option<Message> {
        let (metadata, payload) = split(body);
        Some(Message { metadata, payload })
    }

    /// `ESC ] 72 ; t=..:k=v ; payload ESC \`: `t` first, then the sorted keys. Every byte is
    /// built here, never echoed: CR, LF and everything off the whitelist are dropped (B7).
    pub fn serialize(&self) -> String {
        let mut parts = Vec::new();
        if let Some(t) = self.metadata.get("t") {
            parts.push(format!("t={}", value_echo(t)));
        }
        for (k, v) in &self.metadata {
            if k != "t" && k.len() == 1 && k.as_bytes()[0].is_ascii_alphabetic() {
                parts.push(format!("{k}={}", value_echo(v)));
            }
        }
        let mut out = format!("\x1b]72;{}", parts.join(":"));
        if let Some(p) = &self.payload {
            out.push(';');
            out.push_str(&payload_echo(p));
        }
        out.push_str("\x1b\\");
        out
    }

    pub fn int(&self, key: &str) -> Option<i64> {
        self.metadata.get(key)?.parse().ok()
    }

    fn kind(&self) -> &str {
        self.metadata.get("t").map_or("", String::as_str)
    }
}

/// Joins the chunks of one message (`m=1` more, `m=0` or no `m` last). Bounded: past 1 MiB or
/// 256 chunks the message is dropped, the rest of its chunks are discarded, and `overflowed`
/// reports it once.
#[derive(Default)]
pub struct Reassembler {
    pending: Option<(BTreeMap<String, String>, String, bool, usize)>,
    draining: bool,
    overflow: bool,
}

impl Reassembler {
    pub fn accept(&mut self, body: &str) -> Option<Message> {
        let (metadata, payload) = split(body);
        let more = metadata.get("m").map(String::as_str) == Some("1");
        // A different `t` mid-transfer is its own message (a query during a chunked transfer).
        if let (Some((first, ..)), Some(t)) = (&self.pending, metadata.get("t")) {
            if first.get("t") != Some(t) {
                return Some(Message { metadata, payload });
            }
        }
        if self.draining {
            if !more {
                self.reset();
            }
            return None;
        }
        let (first, text, saw, chunks) = self.pending.get_or_insert_with(|| (metadata.clone(), String::new(), false, 0));
        if let Some(p) = &payload {
            text.push_str(p);
            *saw = true;
        }
        *chunks += 1;
        if text.len() > MAX_MESSAGE_BYTES || *chunks > MAX_CHUNKS {
            self.overflow = true;
            if more {
                self.draining = true;
                self.pending.as_mut().map(|p| p.1 = String::new());
            } else {
                self.reset();
            }
            return None;
        }
        if more {
            return None;
        }
        let mut metadata = std::mem::take(first);
        let text = std::mem::take(text);
        let saw = *saw;
        self.reset();
        metadata.remove("m");
        Some(Message { metadata, payload: saw.then_some(text) })
    }

    /// True once after a message was dropped for its size.
    pub fn overflowed(&mut self) -> bool {
        std::mem::take(&mut self.overflow)
    }

    fn reset(&mut self) {
        self.pending = None;
        self.draining = false;
    }
}

/// The files of the current drop, as the program may ask for them.
struct Drop {
    uris: Vec<u8>,
    files: Vec<File>,
    read: fn(&Path) -> Option<Vec<u8>>,
    arrived: Instant,
    /// The program asked for the URI list (a file entry may be asked for only after it).
    uri_listed: bool,
    /// Bytes served so far, against `MAX_DROP_BYTES`.
    served: usize,
}

enum File {
    /// A plain local file of this size (from metadata); read when a request asks.
    Local(PathBuf, usize),
    TooBig,
    Missing,
}

/// How the window reaches a dropped file: its size (None when it is not a plain local file) and
/// its bytes. Plain function pointers: nothing captured, so a drop can keep them.
#[derive(Clone, Copy)]
pub struct Backing {
    pub size: fn(&Path) -> Option<u64>,
    pub read: fn(&Path) -> Option<Vec<u8>>,
}

/// What a program has registered for, and the drop it may read.
#[derive(Default)]
pub struct Controller {
    reassembler: Reassembler,
    accepting: bool,
    /// The program's routing id (`t=a:i=..`), stamped on what is sent after (`KC:290`).
    id: Option<String>,
    hovering: bool,
    last_move: Option<((i64, i64), (i64, i64))>,
    drop: Option<Drop>,
}

/// A URL for a dropped path: `file:///C:/a%20b.txt`, `file://host/share/n.txt`.
fn file_uri(path: &Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    let text = text.strip_prefix("//?/").unwrap_or(&text);
    let (host, rest) = match text.strip_prefix("//") {
        Some(unc) => unc.split_once('/').map_or((unc, ""), |(h, r)| (h, r)),
        None => ("", text.trim_start_matches('/')),
    };
    let mut out = format!("file://{}/", encode(host));
    for (i, part) in rest.split('/').enumerate() {
        if i > 0 {
            out.push('/');
        }
        // The drive colon stays.
        if i == 0 && host.is_empty() && part.len() == 2 && part.as_bytes()[1] == b':' && part.as_bytes()[0].is_ascii_alphabetic() {
            out.push_str(part);
        } else {
            out.push_str(&encode(part));
        }
    }
    out
}

fn encode(part: &str) -> String {
    let mut out = String::new();
    for b in part.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn msg(pairs: &[(&str, String)], payload: Option<String>) -> Message {
    Message { metadata: pairs.iter().map(|(k, v)| (k.to_string(), v.clone())).collect(), payload }
}

impl Controller {
    pub fn inbound(&mut self, body: &str) -> Vec<String> {
        self.inbound_at(Instant::now(), body)
    }

    /// One OSC 72 body from the program; the replies to write to its input.
    pub fn inbound_at(&mut self, now: Instant, body: &str) -> Vec<String> {
        self.expire(now);
        let message = self.reassembler.accept(body);
        let mut out = Vec::new();
        if self.reassembler.overflowed() {
            out.push(self.send(msg(&[("t", "E".into())], Some("EFBIG".into()))));
        }
        let Some(m) = message else { return out };
        match m.kind() {
            // The capability query echoes its id and nothing else (`KC:347`).
            "q" => {
                let mut reply = msg(&[("t", "q".into())], None);
                if let Some(i) = m.metadata.get("i") {
                    reply.metadata.insert("i".into(), i.clone());
                }
                out.push(reply.serialize());
            }
            "a" => out.extend(self.register(&m)),
            "A" => self.unregister(),
            "r" => out.extend(self.request(&m, now)),
            // Offers, drags out, acceptance replies and the rest are not built.
            _ => {}
        }
        out
    }

    fn register(&mut self, m: &Message) -> Vec<String> {
        if m.int("x") == Some(2) {
            self.unregister();
            return Vec::new();
        }
        let text = m.payload.as_deref().unwrap_or("");
        if text.len() > MAX_MIME_LIST_BYTES || text.split_whitespace().count() > MAX_MIME_TYPES {
            return vec![self.send(msg(&[("t", "E".into())], Some("EFBIG".into())))];
        }
        if let Some(i) = m.metadata.get("i") {
            self.id = Some(value_echo(i));
        }
        self.accepting = true;
        Vec::new()
    }

    fn unregister(&mut self) {
        self.accepting = false;
        self.hovering = false;
        self.last_move = None;
        self.drop = None;
    }

    pub fn accepting(&self) -> bool {
        self.accepting
    }

    /// A drag is over the pane (set by `drag_entered`, cleared by an exit or a drop).
    pub fn hovering(&self) -> bool {
        self.hovering
    }

    pub fn has_drop(&self) -> bool {
        self.drop.is_some()
    }

    /// Frees a drop that outlived its minute.
    pub fn expire(&mut self, now: Instant) {
        if self.drop.as_ref().is_some_and(|d| now.saturating_duration_since(d.arrived) >= DROP_LIFETIME) {
            self.drop = None;
        }
    }

    fn send(&self, mut m: Message) -> String {
        if let (Some(id), false) = (&self.id, m.metadata.contains_key("i")) {
            m.metadata.insert("i".into(), id.clone());
        }
        m.serialize()
    }

    /// `t=m` (a drag) or `t=M` (a drop) at a cell and pixel, with the MIME types when given.
    fn position(&self, kind: &str, cell: (i64, i64), px: (i64, i64), mimes: Option<&[String]>) -> String {
        let pairs = [("t", kind.to_string()), ("x", cell.0.to_string()), ("y", cell.1.to_string()), ("X", px.0.to_string()), ("Y", px.1.to_string()), ("o", OP_COPY.to_string())];
        let payload = mimes.map(|m| m.iter().take(MAX_ECHO).map(String::as_str).collect::<Vec<_>>().join(" "));
        self.send(msg(&pairs, payload))
    }

    /// A drag came over the pane: `t=m` with the cell, the pixel and the MIME types.
    pub fn drag_entered(&mut self, cell: (i64, i64), px: (i64, i64), mimes: &[String]) -> Vec<String> {
        if !self.accepting || self.hovering {
            return Vec::new();
        }
        self.hovering = true;
        // A new drag forgets the last drop (`KC:410`).
        self.drop = None;
        self.last_move = Some((cell, px));
        vec![self.position("m", cell, px, Some(mimes))]
    }

    /// The drag moved: a report only when the position changed. The window has no source for
    /// this tier (winit sends no move during a drag); it is kept for the protocol.
    pub fn drag_moved(&mut self, cell: (i64, i64), px: (i64, i64)) -> Vec<String> {
        if !self.accepting || self.last_move == Some((cell, px)) {
            return Vec::new();
        }
        self.last_move = Some((cell, px));
        vec![self.position("m", cell, px, None)]
    }

    pub fn drag_exited(&mut self) -> Vec<String> {
        if !self.accepting {
            return Vec::new();
        }
        self.hovering = false;
        self.last_move = None;
        vec![self.send(msg(&[("t", "m".into()), ("x", "-1".into()), ("y", "-1".into())], None))]
    }

    pub fn drop(&mut self, cell: (i64, i64), px: (i64, i64), files: Vec<PathBuf>, backing: Backing) -> Vec<String> {
        self.drop_at(Instant::now(), cell, px, files, backing)
    }

    /// The person dropped `files` on the pane: `t=M`. The files are sized now and read per request,
    /// bounded (`MAX_FILE_BYTES` each, `MAX_DROP_BYTES` in all): a request can reach only them.
    pub fn drop_at(&mut self, now: Instant, cell: (i64, i64), px: (i64, i64), mut files: Vec<PathBuf>, backing: Backing) -> Vec<String> {
        self.hovering = false;
        if !self.accepting || files.is_empty() {
            return Vec::new();
        }
        files.truncate(MAX_DROP_FILES);
        let uris: Vec<String> = files.iter().map(|f| file_uri(f)).collect();
        // Sized from metadata, never read here: a file over the per-file bound, or one that
        // would take the drop past the budget, is TooBig before any byte is read.
        let mut held = 0usize;
        let data = files
            .iter()
            .map(|f| {
                // A folder (its path ends with a separator) has no bytes to read.
                let text = f.to_string_lossy();
                if text.ends_with('\\') || text.ends_with('/') {
                    return File::Missing;
                }
                match (backing.size)(f).map(|n| usize::try_from(n).unwrap_or(usize::MAX)) {
                    None => File::Missing,
                    Some(n) if n > MAX_FILE_BYTES || held + n > MAX_DROP_BYTES => File::TooBig,
                    Some(n) => {
                        held += n;
                        File::Local(f.clone(), n)
                    }
                }
            })
            .collect();
        let list: String = uris.iter().map(|u| format!("{u}\r\n")).collect();
        self.drop = Some(Drop { uris: list.into_bytes(), files: data, read: backing.read, arrived: now, uri_listed: false, served: 0 });
        self.last_move = None;
        vec![self.position("M", cell, px, Some(&[MIME_URI_LIST.to_string()]))]
    }

    fn error(&self, addressing: &[(&str, String)], code: &str) -> String {
        // A data request is answered in kind: `t=R` with its addressing (`KC:1469-1474`), so a
        // client waiting on its `t=r` sees the error. Registration errors stay `t=E` (`KC:368`).
        let mut pairs = vec![("t", "R".to_string())];
        pairs.extend(addressing.iter().cloned());
        self.send(msg(&pairs, Some(code.into())))
    }

    /// `t=r`: a request for the data of a drop, or (naming nothing) the end of it.
    fn request(&mut self, m: &Message, now: Instant) -> Vec<String> {
        let has = |k: &str| m.metadata.contains_key(k);
        if !has("x") && !has("y") && !has("Y") {
            self.drop = None;
            return Vec::new();
        }
        // Every key the request addressed is echoed (filtered), so a client can match the answer.
        let addressing: Vec<(&str, String)> = ["x", "y", "Y"].iter().filter_map(|k| m.metadata.get(*k).map(|v| (*k, v.clone()))).collect();
        self.expire(now);
        if has("Y") {
            return vec![self.error(&addressing, "EINVAL")];
        }
        let Some(drop) = &mut self.drop else {
            // No drop to read from (`KC:515-519`).
            return vec![self.error(&addressing, "EINVAL")];
        };
        let Some(x) = m.int("x") else {
            return vec![self.error(&addressing, "EINVAL")];
        };
        if x != 1 {
            return vec![self.error(&addressing, "ENOENT")];
        }
        let pick = if has("y") {
            let Some(y) = m.int("y") else {
                return vec![self.error(&addressing, "EINVAL")];
            };
            if !drop.uri_listed {
                return vec![self.error(&addressing, "EINVAL")];
            }
            let Some(index) = usize::try_from(y).ok().and_then(|y| y.checked_sub(1)).filter(|y| *y < drop.files.len()) else {
                return vec![self.error(&addressing, "ENOENT")];
            };
            match &drop.files[index] {
                File::Local(path, size) => Some((path.clone(), *size)),
                File::TooBig => return vec![self.error(&addressing, "EFBIG")],
                File::Missing => return vec![self.error(&addressing, "ENOENT")],
            }
        } else {
            drop.uri_listed = true;
            None
        };
        let len = pick.as_ref().map_or(drop.uris.len(), |(_, size)| *size);
        if drop.served + len > MAX_DROP_BYTES {
            return vec![self.error(&addressing, "EFBIG")];
        }
        // The file is read now, for this request only; one that grew past its bound since the
        // drop is refused, and one that is gone is missing.
        let bytes = match &pick {
            Some((path, _)) => match (drop.read)(path) {
                None => return vec![self.error(&addressing, "ENOENT")],
                Some(b) if b.len() > MAX_FILE_BYTES || drop.served + b.len() > MAX_DROP_BYTES => return vec![self.error(&addressing, "EFBIG")],
                Some(b) => Some(b),
            },
            None => None,
        };
        let drop = self.drop.as_mut().expect("checked above");
        drop.served += bytes.as_ref().map_or(len, Vec::len);
        let drop = self.drop.as_ref().expect("checked above");
        self.data_reply(&addressing, bytes.as_deref().unwrap_or(&drop.uris))
    }

    /// `t=r` messages for `data`: base64 in `m=1` chunks of 4,096 characters, then `m=0`.
    fn data_reply(&self, addressing: &[(&str, String)], data: &[u8]) -> Vec<String> {
        use base64::Engine;
        let mut out = Vec::with_capacity(data.len() / CHUNK_RAW + 2);
        let base: Vec<(&str, String)> = std::iter::once(("t", "r".to_string())).chain(addressing.iter().cloned()).collect();
        for chunk in data.chunks(CHUNK_RAW) {
            let mut pairs = base.clone();
            pairs.push(("m", "1".into()));
            out.push(self.send(msg(&pairs, Some(base64::engine::general_purpose::STANDARD.encode(chunk)))));
        }
        let mut pairs = base;
        pairs.push(("m", "0".into()));
        out.push(self.send(msg(&pairs, Some(String::new()))));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;

    fn files(list: &[&str]) -> Vec<PathBuf> {
        list.iter().map(PathBuf::from).collect()
    }

    /// The base64 payloads of a run of t=r data messages, decoded and joined.
    fn data_of(replies: &[String]) -> Vec<u8> {
        let mut text = String::new();
        for r in replies {
            let body = r.strip_prefix("\x1b]72;").and_then(|b| b.strip_suffix("\x1b\\")).expect("framed");
            if let Some((_, payload)) = body.split_once(';') {
                text.push_str(payload);
            }
        }
        base64::engine::general_purpose::STANDARD.decode(text).expect("base64")
    }


    fn registered() -> Controller {
        let mut c = Controller::default();
        assert!(c.inbound("t=a;text/uri-list").is_empty());
        c
    }

    fn dropped(c: &mut Controller, list: &[&str]) -> Vec<String> {
        c.drop((3, 1), (30, 18), files(list), Backing { size: |_| Some(16), read: |p| Some(format!("data of {}", p.display()).into_bytes()) })
    }

    #[test]
    fn kitty_dnd_follows_iterm2() {
        let m = Message::parse("t=a;text/uri-list image/png").unwrap();
        assert_eq!(m.metadata.get("t").map(String::as_str), Some("a"));
        assert_eq!(m.payload.as_deref(), Some("text/uri-list image/png"));
        assert_eq!(Message::parse("t=q:i=7").unwrap().payload, None);
        assert_eq!(Message::parse("t=m:x=3:y=-1").unwrap().int("y"), Some(-1));
        assert_eq!(Message::parse("t=m:x=zz").unwrap().int("x"), None);

        let mut meta = BTreeMap::new();
        for (k, v) in [("x", "3"), ("t", "m"), ("y", "1")] {
            meta.insert(k.to_string(), v.to_string());
        }
        let m = Message { metadata: meta.clone(), payload: None };
        assert_eq!(m.serialize(), "\x1b]72;t=m:x=3:y=1\x1b\\");
        meta.insert("x".into(), "3\n4\r".into());
        assert_eq!(Message { metadata: meta, payload: None }.serialize(), "\x1b]72;t=m:x=34:y=1\x1b\\");
        let m = Message { metadata: BTreeMap::from([("t".to_string(), "r".to_string())]), payload: Some(String::new()) };
        assert_eq!(m.serialize(), "\x1b]72;t=r;\x1b\\", "an empty payload keeps its separator");

        let mut c = Controller::default();
        assert_eq!(c.inbound("t=q:i=7"), vec!["\x1b]72;t=q:i=7\x1b\\".to_string()]);
        assert_eq!(c.inbound("t=q"), vec!["\x1b]72;t=q\x1b\\".to_string()]);
        assert!(!c.accepting());
        assert!(c.inbound("t=a;text/uri-list").is_empty());
        assert!(c.accepting());
        assert!(c.inbound("t=A").is_empty());
        assert!(!c.accepting());
        c.inbound("t=a;text/uri-list");
        c.inbound("t=a:x=2");
        assert!(!c.accepting());
        // x=1 is the machine id form: it registers and keeps the old list (not a new one).
        c.inbound("t=a:x=1;1:00ff");
        assert!(c.accepting());

        // The MIME list bounds (KC:171-172): 4,096 types pass, 4,097 are EFBIG and change nothing.
        let list = |n: usize| (0..n).map(|_| "a").collect::<Vec<_>>().join(" ");
        let mut c = Controller::default();
        assert!(c.inbound(&format!("t=a;{}", list(4096))).is_empty());
        assert!(c.accepting());
        c.inbound("t=A");
        c.inbound(&format!("t=a;{}", list(4096)));
        c.inbound("t=a:x=2");
        assert!(!c.accepting());
        assert_eq!(c.inbound(&format!("t=a;{}", list(4097))), vec!["\x1b]72;t=E;EFBIG\x1b\\".to_string()]);
        assert!(!c.accepting(), "a refused list registers nothing");
        c.inbound("t=a;text/uri-list");
        assert_eq!(c.inbound(&format!("t=a;{}", list(4097))), vec!["\x1b]72;t=E;EFBIG\x1b\\".to_string()]);
        assert!(c.accepting(), "and a refused list keeps the old state");
        let long = format!("t=a;{}", "x".repeat(64 * 1024 + 1));
        assert_eq!(c.inbound(&long), vec!["\x1b]72;t=E;EFBIG\x1b\\".to_string()], "64 KiB of list text is the other bound");
        assert!(c.inbound(&format!("t=a;{}", "x".repeat(64 * 1024))).is_empty());
    }

    #[test]
    fn drops_follow_registration() {
        let mimes = vec!["text/uri-list".to_string()];
        let mut c = Controller::default();
        assert!(c.drag_entered((3, 1), (30, 18), &mimes).is_empty(), "no registration, no report");
        assert!(c.drag_moved((4, 1), (40, 18)).is_empty());
        assert!(c.drag_exited().is_empty());
        assert!(dropped(&mut c, &["C:\\a.txt"]).is_empty());

        let mut c = registered();
        let entered = c.drag_entered((3, 1), (30, 18), &mimes);
        assert_eq!(entered, vec!["\x1b]72;t=m:X=30:Y=18:o=1:x=3:y=1;text/uri-list\x1b\\".to_string()]);
        assert!(c.drag_moved((3, 1), (30, 18)).is_empty(), "the same position sends nothing");
        assert_eq!(c.drag_moved((4, 1), (41, 18)), vec!["\x1b]72;t=m:X=41:Y=18:o=1:x=4:y=1\x1b\\".to_string()]);
        assert_eq!(c.drag_exited(), vec!["\x1b]72;t=m:x=-1:y=-1\x1b\\".to_string()]);
        assert!(c.drag_moved((4, 1), (41, 18)).len() == 1, "after an exit the next move reports again");

        let drop = dropped(&mut c, &["C:\\a.txt"]);
        assert_eq!(drop, vec!["\x1b]72;t=M:X=30:Y=18:o=1:x=3:y=1;text/uri-list\x1b\\".to_string()]);
        let uris = c.inbound("t=r:x=1");
        assert!(uris.iter().all(|r| r.starts_with("\x1b]72;t=r:m=") && r.contains(":x=1")), "{uris:?}");
        assert_eq!(data_of(&uris), b"file:///C:/a.txt\r\n");
        assert!(uris.last().unwrap().contains("m=0"), "the data ends with m=0");
        // A file entry of the list is served once the list was asked for.
        let file = c.inbound("t=r:x=1:y=1");
        assert!(file.iter().all(|r| r.starts_with("\x1b]72;t=r:m=") && r.contains(":x=1:y=1")), "{file:?}");
        assert_eq!(data_of(&file), b"data of C:\\a.txt");
        // t=r with no addressing ends the drop; a later request is the stale-request error.
        assert!(c.inbound("t=r:o=1").is_empty());
        assert_eq!(c.inbound("t=r:x=1"), vec!["\x1b]72;t=R:x=1;EINVAL\x1b\\".to_string()], "no drop, no data: EINVAL as iTerm2");
        assert!(c.accepting(), "the registration outlives a drop");
    }

    #[test]
    fn uri_lists_and_errors_follow_the_spec() {
        let mut c = registered();
        dropped(&mut c, &["C:\\My Files\\a b.txt", "\\\\host\\share\\n.txt", "D:\\dir\\"]);
        let uris = c.inbound("t=r:x=1");
        assert_eq!(data_of(&uris), b"file:///C:/My%20Files/a%20b.txt\r\nfile://host/share/n.txt\r\nfile:///D:/dir/\r\n");
        assert_eq!(c.inbound("t=r:x=2"), vec!["\x1b]72;t=R:x=2;ENOENT\x1b\\".to_string()], "index out of bounds");
        assert_eq!(c.inbound("t=r:x=0"), vec!["\x1b]72;t=R:x=0;ENOENT\x1b\\".to_string()]);
        assert_eq!(c.inbound("t=r:x=1:y=4"), vec!["\x1b]72;t=R:x=1:y=4;ENOENT\x1b\\".to_string()]);
        assert_eq!(c.inbound("t=r:x=1:y=z"), vec!["\x1b]72;t=R:x=1:y=z;EINVAL\x1b\\".to_string()]);
        assert_eq!(c.inbound("t=r:y=1"), vec!["\x1b]72;t=R:y=1;EINVAL\x1b\\".to_string()], "a sub-index with no MIME index is malformed");
        assert_eq!(c.inbound("t=r:Y=5"), vec!["\x1b]72;t=R:Y=5;EINVAL\x1b\\".to_string()], "directory handles are not built");
        // A directory has no data to read.
        assert_eq!(c.inbound("t=r:x=1:y=3"), vec!["\x1b]72;t=R:x=1:y=3;ENOENT\x1b\\".to_string()]);
        // Before the list was asked for, a file entry is EINVAL.
        let mut c = registered();
        dropped(&mut c, &["C:\\a.txt"]);
        assert_eq!(c.inbound("t=r:x=1:y=1"), vec!["\x1b]72;t=R:x=1:y=1;EINVAL\x1b\\".to_string()]);
        // A new drag forgets the drop.
        c.drag_entered((0, 0), (0, 0), &["text/uri-list".to_string()]);
        assert_eq!(c.inbound("t=r:x=1"), vec!["\x1b]72;t=R:x=1;EINVAL\x1b\\".to_string()]);
        // Data over several chunks decodes whole: 10 000 bytes in 3 072-byte pieces.
        let mut c = registered();
        c.drop((0, 0), (0, 0), files(&["C:\\big.bin"]), Backing { size: |_| Some(10_000), read: |_| Some(vec![7u8; 10_000]) });
        c.inbound("t=r:x=1");
        let big = c.inbound("t=r:x=1:y=1");
        assert_eq!(big.len(), 4 + 1, "four m=1 chunks and the m=0 end");
        assert_eq!(data_of(&big), vec![7u8; 10_000]);
        assert!(big[..4].iter().all(|r| r.contains("m=1:")), "{big:?}");
    }

    #[test]
    fn chunks_reassemble_until_m0() {
        let mut r = Reassembler::default();
        assert_eq!(r.accept("t=r:x=1:m=1;AAAA"), None);
        assert_eq!(r.accept("t=r:x=1:m=1;BBBB"), None);
        let m = r.accept("t=r:x=1:m=0;CCCC").unwrap();
        assert_eq!(m.payload.as_deref(), Some("AAAABBBBCCCC"));
        assert_eq!(m.metadata.get("m"), None, "m is not part of the message");
        assert_eq!(m.int("x"), Some(1));
        // A different t in the middle is its own message and does not break the transfer.
        assert_eq!(r.accept("t=a:m=1;text/uri-list"), None);
        let q = r.accept("t=q:i=3").unwrap();
        assert_eq!(q.metadata.get("t").map(String::as_str), Some("q"));
        assert_eq!(r.accept("t=a;image/png").unwrap().payload.as_deref(), Some("text/uri-listimage/png"));
        // A message with no payload section has none.
        assert_eq!(r.accept("t=A").unwrap().payload, None);
        // A registration split over chunks registers once.
        let mut c = Controller::default();
        assert!(c.inbound("t=a:m=1;text/uri-").is_empty());
        assert!(!c.accepting());
        assert!(c.inbound("t=a:m=0;list").is_empty());
        assert!(c.accepting());
    }

    #[test]
    fn kitty_dnd_replies_never_echo_controls() {
        // Every byte a program can put in i= and in a registration stays out of the reply
        // unless it is on the whitelist, and the reply is one framed message.
        let hostile = "a\u{f}\t\u{3}\u{7f}\u{85}\u{e9}b<>\"' c".to_string();
        let mut c = Controller::default();
        let r = c.inbound(&format!("t=q:i={hostile}"));
        assert_eq!(r.len(), 1);
        assert_eq!(r[0], "\x1b]72;t=q:i=abc\x1b\\");
        // ':' and ';' cannot start another key or a payload in the echo.
        assert_eq!(c.inbound("t=q:i=ab:x=5;zzz"), vec!["\x1b]72;t=q:i=ab\x1b\\".to_string()]);
        let long = "z".repeat(500);
        assert_eq!(c.inbound(&format!("t=q:i={long}")), vec![format!("\x1b]72;t=q:i={}\x1b\\", "z".repeat(64))]);
        // The multiplexer id from a registration is stamped on later replies, filtered the same way.
        c.inbound(&format!("t=a:i={hostile};text/uri-list"));
        c.drag_entered((1, 2), (3, 4), &["text/uri-list".to_string()]);
        let m = c.drag_exited();
        assert_eq!(m, vec!["\x1b]72;t=m:i=abc:x=-1:y=-1\x1b\\".to_string()]);
        // Addressing keys echoed in an error are filtered too; an odd key is not echoed at all.
        dropped(&mut c, &["C:\\a.txt"]);
        let e = c.inbound(&format!("t=r:x=9\u{f}7:\u{3}z=1:yy=2:y={hostile}"));
        assert_eq!(e.len(), 1);
        assert!(e[0].starts_with("\x1b]72;t=R:i=abc:x=97"), "{:?}", e[0]);
        for r in c.inbound("t=r:x=9\u{f}") {
            assert!(r.bytes().all(|b| b == 0x1b || (0x20..0x7f).contains(&b)), "{r:?}");
        }
        // Message::serialize drops metadata keys that are not one ASCII letter.
        let mut meta = BTreeMap::new();
        meta.insert("t".to_string(), "m".to_string());
        meta.insert("\u{f}".to_string(), "1".to_string());
        meta.insert("ab".to_string(), "1".to_string());
        meta.insert("1".to_string(), "1".to_string());
        assert_eq!(Message { metadata: meta, payload: Some("a\u{f}/b+c=\n".to_string()) }.serialize(), "\x1b]72;t=m;a/b+c=\x1b\\");
        // The program's own text never becomes a payload: errors are the fixed words.
        for body in ["t=r:x=1", "t=r:x=1:y=1", "t=r:Y=1", "t=r:x=q"] {
            for r in c.inbound(body) {
                if let (true, Some((_, p))) = (r.starts_with("\x1b]72;t=R"), r.trim_end_matches("\x1b\\").rsplit_once(';')) {
                    assert!(["EFBIG", "ENOENT", "EINVAL"].contains(&p), "{r:?}");
                }
            }
        }
    }

    #[test]
    fn kitty_dnd_is_bounded() {
        // Chunks: the 257th chunk and a message past 1 MiB are dropped, EFBIG once, and the
        // rest of that message is discarded without a second answer.
        let mut c = Controller::default();
        for _ in 0..256 {
            assert!(c.inbound("t=a:m=1;a").is_empty());
        }
        assert_eq!(c.inbound("t=a:m=1;a"), vec!["\x1b]72;t=E;EFBIG\x1b\\".to_string()]);
        assert!(c.inbound("t=a:m=1;a").is_empty(), "drained, no second answer");
        assert!(c.inbound("t=a:m=0;a").is_empty());
        assert!(!c.accepting(), "the over-long message registered nothing");
        assert!(c.inbound("t=a;text/uri-list").is_empty(), "and the next message is read afresh");
        assert!(c.accepting());
        let mut r = Reassembler::default();
        let big = "A".repeat(600 * 1024);
        assert_eq!(r.accept(&format!("t=r:m=1;{big}")), None);
        assert_eq!(r.accept(&format!("t=r:m=0;{big}")), None, "past 1 MiB the message is gone");
        assert_eq!(r.accept("t=A").unwrap().payload, None);
        let mut c = Controller::default();
        assert!(c.inbound(&format!("t=a:m=1;{big}")).is_empty());
        assert_eq!(c.inbound(&format!("t=a:m=0;{big}")).len(), 1);

        // A drop carries at most 256 files, and the files are the only thing a request reads.
        let mut c = registered();
        let many: Vec<PathBuf> = (0..300).map(|i| PathBuf::from(format!("C:\\f{i}.txt"))).collect();
        c.drop((0, 0), (0, 0), many, Backing { size: |_| Some(1), read: |_| Some(vec![1]) });
        let list = String::from_utf8(data_of(&c.inbound("t=r:x=1"))).unwrap();
        assert_eq!(list.matches("\r\n").count(), 256);
        assert_eq!(c.inbound("t=r:x=1:y=257"), vec!["\x1b]72;t=R:x=1:y=257;ENOENT\x1b\\".to_string()]);

        // A file past 16 MiB answers EFBIG, and is never read.
        let mut c = registered();
        c.drop((0, 0), (0, 0), files(&["C:\\big.bin", "C:\\ok.bin"]), Backing { size: |p| Some(if p.ends_with("big.bin") { (16 << 20) + 1 } else { 5 }), read: |p| Some(vec![0u8; if p.ends_with("ok.bin") { 5 } else { panic!("an oversized file is never read") }]) });
        c.inbound("t=r:x=1");
        assert_eq!(c.inbound("t=r:x=1:y=1"), vec!["\x1b]72;t=R:x=1:y=1;EFBIG\x1b\\".to_string()]);
        assert_eq!(data_of(&c.inbound("t=r:x=1:y=2")), vec![0u8; 5]);
        // 16 MiB exactly is served.
        let mut c = registered();
        c.drop((0, 0), (0, 0), files(&["C:\\max.bin"]), Backing { size: |_| Some(16 << 20), read: |_| Some(vec![0u8; 16 << 20]) });
        c.inbound("t=r:x=1");
        assert_eq!(c.inbound("t=r:x=1:y=1").len(), (16usize * 1024 * 1024).div_ceil(3072) + 1);

        // The bytes served from one drop are capped, so a request loop cannot flood the input.
        let mut c = registered();
        c.drop((0, 0), (0, 0), files(&["C:\\max.bin"]), Backing { size: |_| Some(16 << 20), read: |_| Some(vec![0u8; 16 << 20]) });
        c.inbound("t=r:x=1");
        let mut served = 0;
        let mut refused = false;
        for _ in 0..8 {
            let r = c.inbound("t=r:x=1:y=1");
            if r.len() == 1 && r[0].contains("EFBIG") {
                refused = true;
                break;
            }
            served += 16;
        }
        assert!(refused && served <= 64, "served {served} MiB");

        // A drop lives 60 s.
        let t0 = Instant::now();
        let mut c = registered();
        c.drop_at(t0, (0, 0), (0, 0), files(&["C:\\a.txt"]), Backing { size: |_| Some(1), read: |_| Some(vec![1]) });
        assert!(!c.inbound_at(t0 + Duration::from_secs(59), "t=r:x=1").is_empty());
        assert!(c.inbound_at(t0 + Duration::from_secs(59), "t=r:x=1")[0].contains("m="), "still served at 59 s");
        let late = c.inbound_at(t0 + Duration::from_secs(61), "t=r:x=1");
        assert_eq!(late, vec!["\x1b]72;t=R:x=1;EINVAL\x1b\\".to_string()]);
        assert!(!c.has_drop(), "an expired drop frees its data");
        // Unregistering frees the drop too.
        let mut c = registered();
        dropped(&mut c, &["C:\\a.txt"]);
        assert!(c.has_drop());
        c.inbound("t=A");
        assert!(!c.has_drop());
    }

    static ASKED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    /// 13o final review I2: a drop is sized from metadata and read only when a request asks, so
    /// 256 big files cost nothing up front and the 64 MiB budget is spent before any read.
    #[test]
    fn a_big_drop_is_not_read_up_front() {
        use std::sync::atomic::Ordering::SeqCst;
        let mut c = registered();
        let many: Vec<PathBuf> = (0..300).map(|i| PathBuf::from(format!("C:\\f{i}.bin"))).collect();
        let backing = Backing {
            size: |_| Some(16 << 20),
            read: |_| {
                ASKED.fetch_add(16 << 20, SeqCst);
                Some(vec![0u8; 16 << 20])
            },
        };
        c.drop((0, 0), (0, 0), many, backing);
        assert!(ASKED.load(SeqCst) <= (64 << 20) + (16 << 20), "asked {} bytes before any request", ASKED.load(SeqCst));
        // Four 16 MiB files fit the 64 MiB budget; the fifth was over it when the drop arrived.
        c.inbound("t=r:x=1");
        assert_eq!(data_of(&c.inbound("t=r:x=1:y=1")).len(), 16 << 20);
        assert_eq!(c.inbound("t=r:x=1:y=5"), vec!["\x1b]72;t=R:x=1:y=5;EFBIG\x1b\\".to_string()]);
        assert!(ASKED.load(SeqCst) <= (64 << 20) + (16 << 20));
    }

    /// 13o final review I3: a drop is freed by time, not only by a later addressed request.
    #[test]
    fn an_expired_drop_is_freed_by_any_message_and_by_a_sweep() {
        let t0 = Instant::now();
        let backing = Backing { size: |_| Some(1), read: |_| Some(vec![1]) };
        let mut c = registered();
        c.drop_at(t0, (0, 0), (0, 0), files(&["C:\\a.txt"]), backing);
        c.inbound_at(t0 + Duration::from_secs(59), "t=q");
        assert!(c.has_drop(), "still held at 59 s");
        c.inbound_at(t0 + Duration::from_secs(61), "t=q");
        assert!(!c.has_drop(), "an unrelated message after the minute frees it");
        let mut c = registered();
        c.drop_at(t0, (0, 0), (0, 0), files(&["C:\\a.txt"]), backing);
        c.expire(t0 + Duration::from_secs(61));
        assert!(!c.has_drop(), "and so does a sweep");
    }
}
