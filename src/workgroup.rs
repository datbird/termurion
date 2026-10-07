//! Workgroups (`iTermWorkgroup*`): the data model, its JSON in iTerm2's Codable form, the
//! presets, the Claude Code template and the migrations. Pure: no window, no process.
//!
//! A workgroup's commands run programs, so the model is built to be hostile-input safe: the parse
//! is bounded (size, counts, depth, string lengths) and never panics, and each workgroup carries
//! the source it came from so a later task can refuse to run an import's or a Dynamic Profile's
//! commands without the person. The stored value is iTerm2's exact JSON (`Workgroups`), so an
//! imported value round-trips; one that fails to decode yields no workgroups and stays on disk
//! until the first edit (`iTermWorkgroupModel.swift:72-82`).

use std::collections::{BTreeMap, HashSet};
use std::sync::OnceLock;

use serde_json::{Map, Value};

use crate::globals::Globals;

/// The global that holds the list (`iTermUserDefaults.m:49`).
pub const KEY: &str = "Workgroups";
/// The ids of the workgroups that came from an import, a remote folder or a Dynamic Profile (a
/// JSON array of strings): the stored Imported mark. Not an iTerm2 key.
pub const KEY_IMPORTED: &str = "WorkgroupsImported";
const KEY_SHORTCUTS: &str = "NoSyncWorkgroupShortcutsBackfilled";
const KEY_DIFF_MODE: &str = "NoSyncClaudeCodeDiffModeBackfilled";
const KEY_SYSTEM_PROMPT: &str = "NoSyncClaudeCodeReviewSystemPromptCommandBackfilled";
const KEY_AUTO_SEND: &str = "NoSyncClaudeCodeAutoSendClippingsBackfilled";
const KEY_AUTO_REQUEST: &str = "NoSyncClaudeCodeAutoRequestReviewBackfilled";

/// The bounds (preflight B8, P2). A value over `MAX_BYTES` is not stored at all.
pub const MAX_BYTES: usize = 1 << 20;
pub const MAX_WORKGROUPS: usize = 64;
pub const MAX_SESSIONS: usize = 32;
const MAX_ITEMS: usize = 32;
const MAX_COMMAND: usize = 4096;
const MAX_USAGE_COMMAND: usize = 1024;
const MAX_URL: usize = 2048;
const MAX_NAME: usize = 128;
const MAX_ID: usize = 128;
const MAX_SHORTCUT: usize = 64;
const MAX_SPACER: f64 = 4096.0;

/// Where a workgroup came from. `Person` is the person's own settings (saved by the editor or read
/// back from disk with no mark); every import path marks its workgroups, and that mark is stored
/// (`WorkgroupsImported`), so after a restart they read back as `Import`. `Stored` is only the
/// answer for an id that is not in the list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Person,
    Import,
    RemotePrefs,
    DynamicProfile,
    Stored,
}

impl Source {
    /// True only for the person's own workgroups: the one source whose commands may start without
    /// asking (`m13y1` P3 and the trust ruling).
    pub fn is_person(self) -> bool {
        self == Source::Person
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Workgroup {
    pub unique_identifier: String,
    pub name: String,
    pub sessions: Vec<SessionConfig>,
}

impl Workgroup {
    /// The session with no parent (`WG:23`).
    pub fn root(&self) -> Option<&SessionConfig> {
        self.sessions.iter().find(|s| s.parent_id.is_none())
    }

    pub fn children(&self, parent: &str) -> Vec<&SessionConfig> {
        self.sessions.iter().filter(|s| s.parent_id.as_deref() == Some(parent)).collect()
    }

    pub fn session(&self, id: &str) -> Option<&SessionConfig> {
        self.sessions.iter().find(|s| s.unique_identifier == id)
    }

    /// A workgroup with just a root session (`WG:34-49`).
    pub fn new_empty(name: &str) -> Workgroup {
        let mut root = blank_session(new_id(), None, Kind::Root);
        root.display_name = "Main".into();
        Workgroup { unique_identifier: new_id(), name: name.to_string(), sessions: vec![root] }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SessionConfig {
    pub unique_identifier: String,
    pub parent_id: Option<String>,
    pub kind: Kind,
    pub profile_guid: Option<String>,
    pub command: String,
    pub per_file_command: String,
    pub url_string: String,
    pub toolbar_items: Vec<ToolbarItem>,
    pub display_name: String,
    pub mode: Mode,
    pub peer_switch_shortcut: Option<Shortcut>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    Root,
    Peer,
    Split(SplitSettings),
    Tab,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SplitSettings {
    pub orientation: Orientation,
    pub side: Side,
    /// The fraction of the parent the new pane takes, 0 to 1.
    pub location: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Orientation {
    Vertical,
    Horizontal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    LeadingOrTop,
    TrailingOrBottom,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Regular,
    CodeReview,
    Diff,
}

impl Mode {
    pub fn title(self) -> &'static str {
        match self {
            Mode::Regular => "Regular",
            Mode::CodeReview => "Code Review",
            Mode::Diff => "Diff",
        }
    }

    fn json(self) -> &'static str {
        match self {
            Mode::Regular => "regular",
            Mode::CodeReview => "codeReview",
            Mode::Diff => "diff",
        }
    }
}

/// A key stored as `Keystroke::serialized` writes it, which is iTerm2's form (R13xy-9).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shortcut {
    pub serialized: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NavigationShortcuts {
    pub back: Option<Shortcut>,
    pub forward: Option<Shortcut>,
    pub reload: Option<Shortcut>,
}

impl NavigationShortcuts {
    /// Ctrl+Win+Left, Ctrl+Win+Right and Win+R (`WTS:21-100`; Ctrl+Cmd+arrows and Cmd+R in iTerm2).
    pub fn defaults() -> NavigationShortcuts {
        use crate::keystroke::{Keystroke, COMMAND, CONTROL, LEFT, RIGHT};
        let key = |ch: u32, flags: u64, code: u16| Some(Shortcut { serialized: Keystroke::new(ch, flags, Some(code), ch).serialized() });
        NavigationShortcuts { back: key(LEFT, CONTROL | COMMAND, 0x7b), forward: key(RIGHT, CONTROL | COMMAND, 0x7c), reload: key('r' as u32, COMMAND, 0x0f) }
    }

    pub fn reload_default() -> Shortcut {
        NavigationShortcuts::defaults().reload.expect("the default reload shortcut")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UsageProvider {
    AnthropicClaudeCode,
}

impl UsageProvider {
    pub fn display_name(self) -> &'static str {
        "Anthropic (Claude Code)"
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum ToolbarItem {
    GitStatus,
    ChangedFileSelector,
    ModeSwitcher,
    Navigation(NavigationShortcuts),
    Reload(Option<Shortcut>),
    Spacer { min_width: f64, max_width: f64 },
    GitBaseSelector,
    AutoSendClippingsWhenIdle,
    AutoRequestReviewWhenIdle,
    Usage { provider: UsageProvider, command: String, interval_seconds: f64 },
    /// Injected at runtime on non-peers; never user-added.
    Name,
}

impl ToolbarItem {
    /// The JSON discriminator (`iTermWorkgroupToolbarItemKind`).
    pub fn kind(&self) -> &'static str {
        match self {
            ToolbarItem::GitStatus => "gitStatus",
            ToolbarItem::ChangedFileSelector => "changedFileSelector",
            ToolbarItem::ModeSwitcher => "modeSwitcher",
            ToolbarItem::Navigation(_) => "navigation",
            ToolbarItem::Reload(_) => "reload",
            ToolbarItem::Spacer { .. } => "spacer",
            ToolbarItem::GitBaseSelector => "gitBaseSelector",
            ToolbarItem::AutoSendClippingsWhenIdle => "autoSendClippingsWhenIdle",
            ToolbarItem::AutoRequestReviewWhenIdle => "autoRequestReviewWhenIdle",
            ToolbarItem::Usage { .. } => "usage",
            ToolbarItem::Name => "name",
        }
    }

    /// Does the item need the workgroup's git poller (`WTB:91-97`)?
    pub fn needs_git_poller(&self) -> bool {
        matches!(self, ToolbarItem::GitStatus | ToolbarItem::ChangedFileSelector)
    }
}

/// "Untitled" for an empty name (`WMN:40-110`).
pub fn display_name(w: &Workgroup) -> &str {
    if w.name.is_empty() {
        "Untitled"
    } else {
        &w.name
    }
}

fn new_id() -> String {
    uuid::Uuid::new_v4().hyphenated().to_string().to_uppercase()
}

fn blank_session(id: String, parent: Option<&str>, kind: Kind) -> SessionConfig {
    SessionConfig {
        unique_identifier: id,
        parent_id: parent.map(str::to_string),
        kind,
        profile_guid: None,
        command: String::new(),
        per_file_command: String::new(),
        url_string: String::new(),
        toolbar_items: Vec::new(),
        display_name: String::new(),
        mode: Mode::Regular,
        peer_switch_shortcut: None,
    }
}

// --- JSON ---

fn num(f: f64) -> Value {
    if f.is_finite() && f.fract() == 0.0 && f.abs() < 1e15 {
        return Value::from(f as i64);
    }
    serde_json::Number::from_f64(f).map(Value::Number).unwrap_or_else(|| Value::from(0))
}

fn shortcut_json(s: &Shortcut) -> Value {
    let mut o = Map::new();
    o.insert("serialized".into(), Value::from(s.serialized.clone()));
    Value::Object(o)
}

fn item_json(item: &ToolbarItem) -> Value {
    let mut o = Map::new();
    o.insert("kind".into(), Value::from(item.kind()));
    match item {
        ToolbarItem::Spacer { min_width, max_width } => {
            o.insert("minWidth".into(), num(*min_width));
            o.insert("maxWidth".into(), num(*max_width));
        }
        ToolbarItem::Navigation(n) => {
            for (key, s) in [("backShortcut", &n.back), ("forwardShortcut", &n.forward), ("reloadShortcut", &n.reload)] {
                if let Some(s) = s {
                    o.insert(key.into(), shortcut_json(s));
                }
            }
        }
        ToolbarItem::Reload(Some(s)) => {
            o.insert("shortcut".into(), shortcut_json(s));
        }
        ToolbarItem::Usage { command, interval_seconds, .. } => {
            o.insert("provider".into(), Value::from("anthropicClaudeCode"));
            o.insert("command".into(), Value::from(command.clone()));
            o.insert("intervalSeconds".into(), num(*interval_seconds));
        }
        _ => {}
    }
    Value::Object(o)
}

fn kind_json(kind: &Kind) -> Value {
    let mut o = Map::new();
    match kind {
        Kind::Root => o.insert("kind".into(), Value::from("root")),
        Kind::Peer => o.insert("kind".into(), Value::from("peer")),
        Kind::Tab => o.insert("kind".into(), Value::from("tab")),
        Kind::Split(s) => {
            let mut split = Map::new();
            split.insert("orientation".into(), Value::from(if s.orientation == Orientation::Vertical { "vertical" } else { "horizontal" }));
            split.insert("side".into(), Value::from(if s.side == Side::LeadingOrTop { "leadingOrTop" } else { "trailingOrBottom" }));
            split.insert("location".into(), num(s.location));
            o.insert("kind".into(), Value::from("split"));
            o.insert("split".into(), Value::Object(split))
        }
    };
    Value::Object(o)
}

fn session_json(s: &SessionConfig) -> Value {
    let mut o = Map::new();
    o.insert("uniqueIdentifier".into(), Value::from(s.unique_identifier.clone()));
    if let Some(p) = &s.parent_id {
        o.insert("parentID".into(), Value::from(p.clone()));
    }
    o.insert("kind".into(), kind_json(&s.kind));
    if let Some(g) = &s.profile_guid {
        o.insert("profileGUID".into(), Value::from(g.clone()));
    }
    o.insert("command".into(), Value::from(s.command.clone()));
    o.insert("perFileCommand".into(), Value::from(s.per_file_command.clone()));
    o.insert("urlString".into(), Value::from(s.url_string.clone()));
    o.insert("toolbarItems".into(), Value::Array(s.toolbar_items.iter().map(item_json).collect()));
    o.insert("displayName".into(), Value::from(s.display_name.clone()));
    o.insert("mode".into(), Value::from(s.mode.json()));
    if let Some(p) = &s.peer_switch_shortcut {
        o.insert("peerSwitchShortcut".into(), shortcut_json(p));
    }
    Value::Object(o)
}

/// The stored form: iTerm2's `JSONEncoder` output for `[iTermWorkgroup]`, keys in declaration order.
pub fn encode(list: &[Workgroup]) -> String {
    let values: Vec<Value> = list
        .iter()
        .map(|w| {
            let mut o = Map::new();
            o.insert("uniqueIdentifier".into(), Value::from(w.unique_identifier.clone()));
            o.insert("name".into(), Value::from(w.name.clone()));
            o.insert("sessions".into(), Value::Array(w.sessions.iter().map(session_json).collect()));
            Value::Object(o)
        })
        .collect();
    Value::Array(values).to_string()
}

type Obj = Map<String, Value>;

fn object<'a>(v: &'a Value, what: &str) -> Result<&'a Obj, String> {
    v.as_object().ok_or_else(|| format!("{what} is not an object"))
}

/// A required string (`decode`).
fn req_str(o: &Obj, key: &str) -> Result<String, String> {
    match o.get(key) {
        Some(Value::String(s)) => Ok(s.clone()),
        _ => Err(format!("missing or bad {key}")),
    }
}

/// `decodeIfPresent`: absent or null is None, any other type is an error.
fn opt_str(o: &Obj, key: &str) -> Result<Option<String>, String> {
    match o.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        _ => Err(format!("bad {key}")),
    }
}

/// `(try? decode) ?? ""`.
fn lenient_str(o: &Obj, key: &str) -> String {
    o.get(key).and_then(Value::as_str).unwrap_or_default().to_string()
}

fn req_f64(o: &Obj, key: &str) -> Result<f64, String> {
    o.get(key).and_then(Value::as_f64).ok_or_else(|| format!("missing or bad {key}"))
}

fn opt_shortcut(o: &Obj, key: &str) -> Result<Option<Shortcut>, String> {
    match o.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => Ok(Some(Shortcut { serialized: req_str(object(v, key)?, "serialized")? })),
    }
}

fn parse_kind(v: &Value) -> Result<Kind, String> {
    let o = object(v, "kind")?;
    match req_str(o, "kind")?.as_str() {
        "root" => Ok(Kind::Root),
        "peer" => Ok(Kind::Peer),
        "tab" => Ok(Kind::Tab),
        "split" => {
            let s = object(o.get("split").ok_or("missing split")?, "split")?;
            let orientation = match req_str(s, "orientation")?.as_str() {
                "vertical" => Orientation::Vertical,
                "horizontal" => Orientation::Horizontal,
                other => return Err(format!("unknown orientation {other}")),
            };
            let side = match req_str(s, "side")?.as_str() {
                "leadingOrTop" => Side::LeadingOrTop,
                "trailingOrBottom" => Side::TrailingOrBottom,
                other => return Err(format!("unknown side {other}")),
            };
            Ok(Kind::Split(SplitSettings { orientation, side, location: req_f64(s, "location")? }))
        }
        other => Err(format!("Unknown session kind: {other}")),
    }
}

fn parse_item(v: &Value) -> Result<ToolbarItem, String> {
    let o = object(v, "toolbar item")?;
    Ok(match req_str(o, "kind")?.as_str() {
        "gitStatus" => ToolbarItem::GitStatus,
        "changedFileSelector" => ToolbarItem::ChangedFileSelector,
        "modeSwitcher" => ToolbarItem::ModeSwitcher,
        "navigation" => ToolbarItem::Navigation(NavigationShortcuts {
            back: opt_shortcut(o, "backShortcut")?,
            forward: opt_shortcut(o, "forwardShortcut")?,
            reload: opt_shortcut(o, "reloadShortcut")?,
        }),
        "reload" => ToolbarItem::Reload(opt_shortcut(o, "shortcut")?),
        "spacer" => ToolbarItem::Spacer { min_width: req_f64(o, "minWidth")?, max_width: req_f64(o, "maxWidth")? },
        "gitBaseSelector" => ToolbarItem::GitBaseSelector,
        "autoSendClippingsWhenIdle" => ToolbarItem::AutoSendClippingsWhenIdle,
        "autoRequestReviewWhenIdle" => ToolbarItem::AutoRequestReviewWhenIdle,
        "usage" => {
            // An unknown provider degrades this one item, never the list (`WTI:200-222`).
            let _provider = opt_str(o, "provider")?;
            let command = opt_str(o, "command")?.unwrap_or_default();
            let interval_seconds = match o.get("intervalSeconds") {
                None | Some(Value::Null) => 60.0,
                Some(v) => v.as_f64().ok_or("bad intervalSeconds")?,
            };
            ToolbarItem::Usage { provider: UsageProvider::AnthropicClaudeCode, command, interval_seconds }
        }
        "name" => ToolbarItem::Name,
        other => return Err(format!("unknown toolbar item {other}")),
    })
}

fn parse_session(v: &Value) -> Result<SessionConfig, String> {
    let o = object(v, "session")?;
    let items = o.get("toolbarItems").and_then(Value::as_array).ok_or("missing toolbarItems")?;
    let mode = match o.get("mode").and_then(Value::as_str) {
        Some("codeReview") => Mode::CodeReview,
        Some("diff") => Mode::Diff,
        _ => Mode::Regular,
    };
    Ok(SessionConfig {
        unique_identifier: req_str(o, "uniqueIdentifier")?,
        parent_id: opt_str(o, "parentID")?,
        kind: parse_kind(o.get("kind").ok_or("missing kind")?)?,
        profile_guid: opt_str(o, "profileGUID")?,
        command: req_str(o, "command")?,
        per_file_command: lenient_str(o, "perFileCommand"),
        url_string: req_str(o, "urlString")?,
        toolbar_items: items.iter().map(parse_item).collect::<Result<_, _>>()?,
        display_name: lenient_str(o, "displayName"),
        mode,
        peer_switch_shortcut: opt_shortcut(o, "peerSwitchShortcut")?,
    })
}

/// The list as iTerm2's `JSONDecoder` reads it, with no limit but the size: an unknown session
/// kind or toolbar item kind fails the whole decode (`WSC:209-213`).
fn parse_raw(json: &str) -> Result<Vec<Workgroup>, String> {
    if json.len() > MAX_BYTES {
        return Err("the workgroups value is too large".into());
    }
    // serde_json's recursion limit turns a deeply nested value into an error, not a stack overflow.
    let value: Value = serde_json::from_str(json).map_err(|e| e.to_string())?;
    let list = value.as_array().ok_or("the workgroups value is not a list")?;
    list.iter()
        .map(|v| {
            let o = object(v, "workgroup")?;
            let sessions = o.get("sessions").and_then(Value::as_array).ok_or("missing sessions")?;
            Ok(Workgroup {
                unique_identifier: req_str(o, "uniqueIdentifier")?,
                name: req_str(o, "name")?,
                sessions: sessions.iter().map(parse_session).collect::<Result<_, _>>()?,
            })
        })
        .collect()
}

/// Reads the stored JSON, bounded: what breaks a limit is dropped or cut (`sanitize`).
pub fn decode(json: &str) -> Result<Vec<Workgroup>, String> {
    parse_raw(json).map(sanitize)
}

// --- Bounds ---

fn id_ok(id: &str) -> bool {
    (1..=MAX_ID).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// A command a person typed: no control character (a newline types a second command), no NUL,
/// bounded.
fn command_ok(s: &str, max: usize) -> bool {
    s.chars().count() <= max && !s.chars().any(char::is_control)
}

fn shortcut_ok(s: Option<Shortcut>) -> Option<Shortcut> {
    s.filter(|s| s.serialized.len() <= MAX_SHORTCUT && crate::keystroke::parse(&s.serialized).is_some())
}

fn sanitize_item(item: ToolbarItem) -> Option<ToolbarItem> {
    Some(match item {
        ToolbarItem::Navigation(n) => ToolbarItem::Navigation(NavigationShortcuts { back: shortcut_ok(n.back), forward: shortcut_ok(n.forward), reload: shortcut_ok(n.reload) }),
        ToolbarItem::Reload(s) => ToolbarItem::Reload(shortcut_ok(s)),
        ToolbarItem::Spacer { min_width, max_width } => {
            let min = if min_width.is_finite() { min_width.clamp(0.0, MAX_SPACER) } else { 0.0 };
            let max = if max_width.is_finite() { max_width.clamp(0.0, MAX_SPACER) } else { MAX_SPACER };
            ToolbarItem::Spacer { min_width: min, max_width: max.max(min) }
        }
        ToolbarItem::Usage { provider, command, interval_seconds } => {
            if !command_ok(&command, MAX_USAGE_COMMAND) {
                return None;
            }
            let interval = if interval_seconds.is_finite() { interval_seconds.clamp(5.0, 86400.0) } else { 60.0 };
            ToolbarItem::Usage { provider, command, interval_seconds: interval }
        }
        other => other,
    })
}

/// None when a limit that cannot be repaired is broken (the whole workgroup goes).
fn sanitize_session(mut s: SessionConfig) -> Option<SessionConfig> {
    if !command_ok(&s.command, MAX_COMMAND) || !command_ok(&s.per_file_command, MAX_COMMAND) {
        return None;
    }
    s.display_name = crate::inert_files::display_text(&s.display_name, MAX_NAME, false);
    s.url_string = if s.url_string.chars().count() > MAX_URL { String::new() } else { crate::browser::arrangement_url(&s.url_string).unwrap_or_default() };
    s.profile_guid = s.profile_guid.filter(|g| g.len() == 36 && uuid::Uuid::parse_str(g).is_ok());
    s.peer_switch_shortcut = shortcut_ok(s.peer_switch_shortcut);
    if let Kind::Split(split) = &mut s.kind {
        split.location = if split.location.is_finite() { split.location.clamp(0.0, 1.0) } else { 0.5 };
    }
    s.toolbar_items.truncate(MAX_ITEMS);
    let items = std::mem::take(&mut s.toolbar_items);
    for item in items {
        s.toolbar_items.push(sanitize_item(item)?);
    }
    Some(s)
}

fn sanitize_workgroup(mut w: Workgroup) -> Option<Workgroup> {
    if !id_ok(&w.unique_identifier) || w.sessions.is_empty() || w.sessions.len() > MAX_SESSIONS {
        return None;
    }
    w.name = crate::inert_files::display_text(&w.name, MAX_NAME, false);
    // One root, first; every other session names an earlier one; ids are unique.
    let mut seen: HashSet<String> = HashSet::new();
    for (i, s) in w.sessions.iter().enumerate() {
        let shape_ok = match (&s.parent_id, &s.kind, i) {
            (None, Kind::Root, 0) => true,
            (Some(p), kind, i) if i > 0 && *kind != Kind::Root => seen.contains(p),
            _ => false,
        };
        if !id_ok(&s.unique_identifier) || !shape_ok || !seen.insert(s.unique_identifier.clone()) {
            return None;
        }
    }
    let sessions = std::mem::take(&mut w.sessions);
    for s in sessions {
        w.sessions.push(sanitize_session(s)?);
    }
    Some(w)
}

fn sanitize(list: Vec<Workgroup>) -> Vec<Workgroup> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut out = Vec::new();
    for w in list {
        if out.len() >= MAX_WORKGROUPS {
            break;
        }
        let id = w.unique_identifier.clone();
        if let Some(w) = sanitize_workgroup(w) {
            if seen.insert(id) {
                out.push(w);
            }
        }
    }
    out
}

/// The one write-point bound of the `Workgroups` global (`Globals::set` and `bound_values`; B8):
/// a value over one MiB is not stored; a value that decodes keeps its decodable, in-bounds
/// workgroups (iTerm2's bytes untouched when nothing had to change); a value that does not decode
/// is kept as written, so it stays on disk until the first edit.
pub fn bound_json(raw: &str) -> String {
    if raw.len() > MAX_BYTES {
        return String::new();
    }
    match parse_raw(raw) {
        Err(_) => raw.to_string(),
        Ok(list) => {
            let clean = sanitize(list.clone());
            if clean == list {
                raw.to_string()
            } else {
                encode(&clean)
            }
        }
    }
}

/// The stored Imported mark list, bounded at the write point: a JSON array of well-formed ids, at
/// most `MAX_WORKGROUPS` of them. Anything else is the empty string.
pub fn bound_imported(raw: &str) -> String {
    if raw.len() > MAX_BYTES {
        return String::new();
    }
    let Ok(Value::Array(items)) = serde_json::from_str::<Value>(raw) else { return String::new() };
    let ids: Vec<&str> = items.iter().filter_map(Value::as_str).filter(|id| id_ok(id)).take(MAX_WORKGROUPS).collect();
    if ids.is_empty() {
        String::new()
    } else {
        serde_json::to_string(&ids).unwrap_or_default()
    }
}

/// The keys only the model writes: the list, the stored Imported mark and the five latches. No
/// script, plist or API call writes them (13y1 trust ruling).
pub fn is_workgroup_key(key: &str) -> bool {
    [KEY, KEY_IMPORTED, KEY_SHORTCUTS, KEY_DIFF_MODE, KEY_SYSTEM_PROMPT, KEY_AUTO_SEND, KEY_AUTO_REQUEST].contains(&key)
}

/// After something other than the editor wrote the list (a remote-prefs snapshot, an import):
/// every workgroup whose data is new or changed is marked Imported, whatever the incoming data
/// says, and the old marks are put back (the writer cannot clear them). `before_*` are the
/// stored values from before the write.
pub fn mark_foreign_write(g: &mut Globals, before_list: &str, before_marks: &str) {
    let old = decode(before_list).unwrap_or_default();
    let mut marked: HashSet<String> = match serde_json::from_str::<Value>(before_marks) {
        Ok(Value::Array(items)) => items.iter().filter_map(Value::as_str).map(str::to_string).collect(),
        _ => HashSet::new(),
    };
    let new = match decode(&g.string(KEY)) {
        Ok(list) => list,
        Err(_) => return,
    };
    let mut ids = Vec::new();
    for w in &new {
        let same = old.iter().any(|o| o == w);
        if marked.contains(&w.unique_identifier) || !same {
            marked.insert(w.unique_identifier.clone());
            ids.push(w.unique_identifier.clone());
        }
    }
    let text = if ids.is_empty() { String::new() } else { serde_json::to_string(&ids).unwrap_or_default() };
    g.set(KEY_IMPORTED, toml::Value::String(text));
}

fn read_marks(g: &Globals) -> HashSet<String> {
    match serde_json::from_str::<Value>(&g.string(KEY_IMPORTED)) {
        Ok(Value::Array(items)) => items.iter().filter_map(Value::as_str).map(str::to_string).collect(),
        _ => HashSet::new(),
    }
}

// --- Presets and the Claude Code template ---

pub mod presets {
    use super::*;

    /// (identifier, display name) of the "Add Preset" menu (`WP:17-35`).
    pub fn all() -> Vec<(&'static str, &'static str)> {
        vec![("codingAgentPlusDiff", "Coding Agent + Diff"), ("codingAgentPlusDiffPlusCodeReview", "Coding Agent + Diff + Code Review")]
    }

    pub fn build(identifier: &str) -> Option<Workgroup> {
        match identifier {
            "codingAgentPlusDiff" => Some(coding_agent_plus_diff()),
            "codingAgentPlusDiffPlusCodeReview" => Some(coding_agent_plus_diff_plus_code_review(None, None)),
            _ => None,
        }
    }

    pub fn coding_agent_plus_diff() -> Workgroup {
        let (root_id, diff_id) = (new_id(), new_id());
        let mut root = blank_session(root_id.clone(), None, Kind::Root);
        root.toolbar_items = vec![ToolbarItem::ModeSwitcher, ToolbarItem::GitStatus];
        let mut diff = blank_session(diff_id, Some(&root_id), Kind::Peer);
        diff.command = "git diff \\(gitBase)".into();
        diff.per_file_command = "git diff \\(gitBase) -- \\(file)".into();
        diff.toolbar_items = vec![ToolbarItem::ModeSwitcher, ToolbarItem::ChangedFileSelector, ToolbarItem::GitBaseSelector, ToolbarItem::Navigation(NavigationShortcuts::defaults())];
        diff.display_name = "Diff".into();
        diff.mode = Mode::Diff;
        Workgroup { unique_identifier: new_id(), name: "Coding Agent + Diff".into(), sessions: vec![root, diff] }
    }

    /// Chat (root), Diff and Code Review peers. `ids` is (workgroup, root, diff, review); the Claude
    /// Code installer overrides every id and the name so its triggers keep resolving (`WP:63-133`).
    pub fn coding_agent_plus_diff_plus_code_review(ids: Option<(&str, &str, &str, &str)>, name: Option<&str>) -> Workgroup {
        let (workgroup_id, root_id, diff_id, review_id) = match ids {
            Some((w, r, d, v)) => (w.to_string(), r.to_string(), d.to_string(), v.to_string()),
            None => (new_id(), new_id(), new_id(), new_id()),
        };
        let mut main = blank_session(root_id.clone(), None, Kind::Root);
        main.toolbar_items = vec![ToolbarItem::ModeSwitcher, ToolbarItem::GitStatus, ToolbarItem::AutoRequestReviewWhenIdle];
        main.display_name = "Chat".into();
        let mut diff = blank_session(diff_id, Some(&root_id), Kind::Peer);
        diff.command = "git difftool -y -x vimdiff \\(gitBase)".into();
        diff.per_file_command = "git difftool -y -x vimdiff \\(gitBase) -- \\(file)".into();
        diff.toolbar_items = vec![ToolbarItem::ModeSwitcher, ToolbarItem::ChangedFileSelector, ToolbarItem::GitBaseSelector, ToolbarItem::Navigation(NavigationShortcuts::defaults())];
        diff.display_name = "Diff".into();
        diff.mode = Mode::Diff;
        let mut review = blank_session(review_id, Some(&root_id), Kind::Peer);
        // Task 7 binds `\(iterm2.appBundlePath)/Contents/Resources` to Tandem's resources folder (Decision).
        review.command = "claude \\(codeReviewPrompt) --append-system-prompt-file '\\(codeReviewSystemPromptFile)' --settings '\\(iterm2.appBundlePath)/Contents/Resources/code-review-settings.txt'".into();
        review.toolbar_items = vec![ToolbarItem::ModeSwitcher, ToolbarItem::Reload(Some(NavigationShortcuts::reload_default())), ToolbarItem::AutoSendClippingsWhenIdle];
        review.display_name = "Code Review".into();
        review.mode = Mode::CodeReview;
        Workgroup { unique_identifier: workgroup_id, name: name.unwrap_or("Coding Agent + Diff + Code Review").to_string(), sessions: vec![main, diff, review] }
    }
}

/// The ids the Claude Code integration's triggers and saved state refer to (`CCT:13-39`).
pub mod claude_code {
    use super::*;

    pub const WORKGROUP: &str = "builtin.claudeCode";
    pub const MAIN: &str = "builtin.claudeCode.main";
    pub const DIFF: &str = "builtin.claudeCode.diff";
    pub const REVIEW: &str = "builtin.claudeCode.review";

    pub fn config() -> Workgroup {
        presets::coding_agent_plus_diff_plus_code_review(Some((WORKGROUP, MAIN, DIFF, REVIEW)), Some("Claude Code"))
    }
}

// --- Migrations (`WM:98-322`) ---

/// The five one-time migrations' latches (`iTermUserDefaults.m:50-54`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Latches {
    pub shortcuts: bool,
    pub diff_mode: bool,
    pub system_prompt: bool,
    pub auto_send: bool,
    pub auto_request: bool,
}

impl Latches {
    fn read(g: &Globals) -> Latches {
        Latches {
            shortcuts: g.bool(KEY_SHORTCUTS),
            diff_mode: g.bool(KEY_DIFF_MODE),
            system_prompt: g.bool(KEY_SYSTEM_PROMPT),
            auto_send: g.bool(KEY_AUTO_SEND),
            auto_request: g.bool(KEY_AUTO_REQUEST),
        }
    }

    fn write(&self, g: &mut Globals) {
        for (key, on) in [(KEY_SHORTCUTS, self.shortcuts), (KEY_DIFF_MODE, self.diff_mode), (KEY_SYSTEM_PROMPT, self.system_prompt), (KEY_AUTO_SEND, self.auto_send), (KEY_AUTO_REQUEST, self.auto_request)] {
            g.set(key, toml::Value::Boolean(on));
        }
    }
}

fn system_prompt_re() -> &'static regex::Regex {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r"--append-system-prompt-file\s+'?[^']*code-review-system-prompt\.txt'?").expect("a fixed pattern"))
}

/// The Claude Code workgroup's session with this id, if the workgroup is there.
fn claude_session<'a>(list: &'a mut [Workgroup], id: &str) -> Vec<&'a mut SessionConfig> {
    list.iter_mut().filter(|w| w.unique_identifier == claude_code::WORKGROUP).flat_map(|w| w.sessions.iter_mut()).filter(|s| s.unique_identifier == id).collect()
}

fn add_item_once(list: &mut [Workgroup], session: &str, item: ToolbarItem) -> bool {
    let mut changed = false;
    for s in claude_session(list, session) {
        if !s.toolbar_items.iter().any(|i| i.kind() == item.kind()) {
            s.toolbar_items.push(item.clone());
            changed = true;
        }
    }
    changed
}

/// Applies each migration whose latch is not set, then sets every latch. True when the data changed
/// or a latch was set (`persist()` and the `defer` of `WM`).
pub fn migrate(list: &mut Vec<Workgroup>, latches: &mut Latches) -> bool {
    let before = *latches;
    let data = migrate_data(list, latches);
    data || *latches != before
}

fn migrate_data(list: &mut [Workgroup], latches: &mut Latches) -> bool {
    let mut changed = false;
    if !latches.shortcuts {
        latches.shortcuts = true;
        for s in list.iter_mut().flat_map(|w| w.sessions.iter_mut()) {
            for item in &mut s.toolbar_items {
                match item {
                    ToolbarItem::Navigation(n) if n.back.is_none() && n.forward.is_none() && n.reload.is_none() => {
                        *item = ToolbarItem::Navigation(NavigationShortcuts::defaults());
                        changed = true;
                    }
                    ToolbarItem::Reload(None) => {
                        *item = ToolbarItem::Reload(Some(NavigationShortcuts::reload_default()));
                        changed = true;
                    }
                    _ => {}
                }
            }
        }
    }
    if !latches.diff_mode {
        latches.diff_mode = true;
        for s in claude_session(list, claude_code::DIFF) {
            if s.mode == Mode::Regular {
                s.mode = Mode::Diff;
                changed = true;
            }
        }
    }
    if !latches.system_prompt {
        latches.system_prompt = true;
        for s in list.iter_mut().flat_map(|w| w.sessions.iter_mut()) {
            if let Some(m) = system_prompt_re().find(&s.command) {
                let range = m.range();
                s.command.replace_range(range, "--append-system-prompt-file '\\(codeReviewSystemPromptFile)'");
                changed = true;
            }
        }
    }
    if !latches.auto_send {
        latches.auto_send = true;
        changed |= add_item_once(list, claude_code::REVIEW, ToolbarItem::AutoSendClippingsWhenIdle);
    }
    if !latches.auto_request {
        latches.auto_request = true;
        changed |= add_item_once(list, claude_code::MAIN, ToolbarItem::AutoRequestReviewWhenIdle);
    }
    changed
}

// --- The model ---

#[derive(Clone, Debug)]
pub struct Model {
    pub workgroups: Vec<Workgroup>,
    pub latches: Latches,
    sources: BTreeMap<String, Source>,
    /// The list differs from what is stored (an edit or a migration): `save` writes it.
    dirty: bool,
    /// The stored value failed to decode: it stays untouched until the first edit.
    kept_undecodable: bool,
}

impl Default for Model {
    fn default() -> Model {
        Model { workgroups: Vec::new(), latches: Latches::default(), sources: BTreeMap::new(), dirty: false, kept_undecodable: false }
    }
}

impl Model {
    /// Reads the global and applies the migrations. The bool is true when something changed that
    /// `save` should write (a migrated list, or a latch).
    pub fn load(g: &Globals) -> (Model, bool) {
        let mut m = Model::default();
        let before = Latches::read(g);
        m.latches = before;
        m.read_list(g);
        let marks = read_marks(g);
        m.sources = m.workgroups.iter().map(|w| (w.unique_identifier.clone(), if marks.contains(&w.unique_identifier) { Source::Import } else { Source::Person })).collect();
        let data = migrate_data(&mut m.workgroups, &mut m.latches);
        m.dirty = data;
        let changed = data || m.latches != before;
        (m, changed)
    }

    fn read_list(&mut self, g: &Globals) {
        let text = g.string(KEY);
        self.kept_undecodable = false;
        self.workgroups = if text.trim().is_empty() {
            Vec::new()
        } else {
            decode(&text).unwrap_or_else(|_| {
                self.kept_undecodable = true;
                Vec::new()
            })
        };
    }

    /// Re-reads the global (after a remote-prefs load or an import wrote it); every workgroup in it
    /// is of `source`.
    pub fn reload_from(&mut self, g: &Globals, source: Source) {
        self.read_list(g);
        self.sources = self.workgroups.iter().map(|w| (w.unique_identifier.clone(), source)).collect();
        self.dirty = migrate_data(&mut self.workgroups, &mut self.latches);
    }

    /// Writes the latches, and the list when it changed. Never overwrites a value that failed to
    /// decode until an edit replaces it.
    pub fn save(&self, g: &mut Globals) {
        self.latches.write(g);
        let marks: Vec<&str> = self.workgroups.iter().filter(|w| !self.source_of(&w.unique_identifier).is_person()).map(|w| w.unique_identifier.as_str()).collect();
        let marks = if marks.is_empty() { String::new() } else { serde_json::to_string(&marks).unwrap_or_default() };
        if g.string(KEY_IMPORTED) != marks {
            g.set(KEY_IMPORTED, toml::Value::String(marks));
        }
        if self.dirty && !self.kept_undecodable {
            g.set(KEY, toml::Value::String(encode(&self.workgroups)));
        }
    }

    fn edited(&mut self) {
        self.dirty = true;
        self.kept_undecodable = false;
    }

    pub fn source_of(&self, id: &str) -> Source {
        if self.get(id).is_none() {
            return Source::Stored;
        }
        self.sources.get(id).copied().unwrap_or(Source::Stored)
    }

    pub fn add(&mut self, w: Workgroup) {
        self.add_from(w, Source::Person);
    }

    pub fn add_from(&mut self, w: Workgroup, source: Source) {
        self.sources.insert(w.unique_identifier.clone(), source);
        self.workgroups.push(w);
        self.edited();
    }

    pub fn remove(&mut self, id: &str) {
        let Some(i) = self.workgroups.iter().position(|w| w.unique_identifier == id) else { return };
        self.workgroups.remove(i);
        self.sources.remove(id);
        self.edited();
    }

    /// The person's edit: the workgroup keeps its mark, so a rename or a command edit never
    /// approves an import's commands; only `trust` does.
    pub fn replace(&mut self, id: &str, w: Workgroup) {
        let source = match self.sources.get(id) {
            Some(Source::Person) | None => Source::Person,
            Some(s) => *s,
        };
        self.replace_from(id, w, source);
    }

    /// The explicit act: the person has read this workgroup's commands and trusts it.
    pub fn trust(&mut self, id: &str) {
        if self.get(id).is_some() {
            self.sources.insert(id.to_string(), Source::Person);
            self.dirty = true;
        }
    }

    pub fn replace_from(&mut self, id: &str, w: Workgroup, source: Source) {
        let Some(i) = self.workgroups.iter().position(|w| w.unique_identifier == id) else { return };
        self.sources.remove(id);
        self.sources.insert(w.unique_identifier.clone(), source);
        self.workgroups[i] = w;
        self.edited();
    }

    /// Replaces the whole list (the editor's undo). A workgroup keeps the source its id had, so an
    /// edit or an undo never approves an import; an id that was not there is the person's.
    pub fn set_all(&mut self, list: Vec<Workgroup>) {
        let sources: BTreeMap<String, Source> = list
            .iter()
            .map(|w| (w.unique_identifier.clone(), self.sources.get(&w.unique_identifier).copied().unwrap_or(Source::Person)))
            .collect();
        self.sources = sources;
        self.workgroups = list;
        self.edited();
    }

    pub fn set_all_from(&mut self, list: Vec<Workgroup>, source: Source) {
        self.sources = list.iter().map(|w| (w.unique_identifier.clone(), source)).collect();
        self.workgroups = list;
        self.edited();
    }

    pub fn get(&self, id: &str) -> Option<&Workgroup> {
        self.workgroups.iter().find(|w| w.unique_identifier == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::globals::Globals;
    use serde_json::{json, Value};

    const BACK: &str = "0xf702-0x340000-0x7b";
    const FORWARD: &str = "0xf703-0x340000-0x7c";
    const RELOAD: &str = "0x72-0x100000-0xf";
    const REVIEW_CMD: &str = "claude \\(codeReviewPrompt) --append-system-prompt-file '\\(codeReviewSystemPromptFile)' --settings '\\(iterm2.appBundlePath)/Contents/Resources/code-review-settings.txt'";

    fn globals() -> Globals {
        Globals::empty(std::path::Path::new("workgroup-test-settings.toml"))
    }

    fn claude_json() -> Value {
        json!([{
            "uniqueIdentifier": "W",
            "name": "Claude Code",
            "sessions": [
                {"uniqueIdentifier": "R", "kind": {"kind": "root"}, "command": "", "perFileCommand": "", "urlString": "",
                 "toolbarItems": [{"kind": "modeSwitcher"}, {"kind": "gitStatus"}, {"kind": "autoRequestReviewWhenIdle"}],
                 "displayName": "Chat", "mode": "regular"},
                {"uniqueIdentifier": "D", "parentID": "R", "kind": {"kind": "peer"},
                 "command": "git difftool -y -x vimdiff \\(gitBase)", "perFileCommand": "git difftool -y -x vimdiff \\(gitBase) -- \\(file)", "urlString": "",
                 "toolbarItems": [{"kind": "modeSwitcher"}, {"kind": "changedFileSelector"}, {"kind": "gitBaseSelector"},
                     {"kind": "navigation", "backShortcut": {"serialized": BACK}, "forwardShortcut": {"serialized": FORWARD}, "reloadShortcut": {"serialized": RELOAD}}],
                 "displayName": "Diff", "mode": "diff"},
                {"uniqueIdentifier": "V", "parentID": "R", "kind": {"kind": "peer"}, "command": REVIEW_CMD, "perFileCommand": "", "urlString": "",
                 "toolbarItems": [{"kind": "modeSwitcher"}, {"kind": "reload", "shortcut": {"serialized": RELOAD}}, {"kind": "autoSendClippingsWhenIdle"}],
                 "displayName": "Code Review", "mode": "codeReview"}
            ]
        }])
    }

    fn parsed(s: &str) -> Value {
        serde_json::from_str(s).expect("valid json")
    }

    fn cc() -> Workgroup {
        presets::coding_agent_plus_diff_plus_code_review(Some(("W", "R", "D", "V")), Some("Claude Code"))
    }

    fn session(id: &str, parent: Option<&str>, kind: Kind) -> SessionConfig {
        SessionConfig {
            unique_identifier: id.into(),
            parent_id: parent.map(str::to_string),
            kind,
            profile_guid: None,
            command: String::new(),
            per_file_command: String::new(),
            url_string: String::new(),
            toolbar_items: vec![],
            display_name: String::new(),
            mode: Mode::Regular,
            peer_switch_shortcut: None,
        }
    }

    fn wg(id: &str, sessions: Vec<SessionConfig>) -> Workgroup {
        Workgroup { unique_identifier: id.into(), name: format!("wg {id}"), sessions }
    }

    fn simple(id: &str) -> Workgroup {
        wg(id, vec![session("root", None, Kind::Root)])
    }

    #[test]
    fn json_follows_iterm2() {
        assert_eq!(parsed(&encode(&[cc()])), claude_json());
        // The rest of the kinds and items: split, tab, spacer (integers when whole), usage, a peer shortcut.
        let mut split = session("s", Some("r"), Kind::Split(SplitSettings { orientation: Orientation::Vertical, side: Side::LeadingOrTop, location: 0.5 }));
        split.toolbar_items = vec![ToolbarItem::Spacer { min_width: 8.0, max_width: 100.0 }, ToolbarItem::Usage { provider: UsageProvider::AnthropicClaudeCode, command: String::new(), interval_seconds: 60.0 }];
        let mut tab = session("t", Some("r"), Kind::Tab);
        tab.profile_guid = Some("0F3C5E3A-1D2B-4C6A-9E8F-123456789ABC".into());
        let mut peer = session("p", Some("r"), Kind::Peer);
        peer.peer_switch_shortcut = Some(Shortcut { serialized: RELOAD.into() });
        let w = wg("X", vec![session("r", None, Kind::Root), split, tab, peer]);
        let v = parsed(&encode(&[w.clone()]));
        let sessions = &v[0]["sessions"];
        assert_eq!(sessions[1]["kind"], json!({"kind": "split", "split": {"orientation": "vertical", "side": "leadingOrTop", "location": 0.5}}));
        assert_eq!(sessions[1]["toolbarItems"], json!([{"kind": "spacer", "minWidth": 8, "maxWidth": 100}, {"kind": "usage", "provider": "anthropicClaudeCode", "command": "", "intervalSeconds": 60}]));
        assert_eq!(sessions[2]["kind"], json!({"kind": "tab"}));
        assert_eq!(sessions[2]["profileGUID"], json!("0F3C5E3A-1D2B-4C6A-9E8F-123456789ABC"));
        assert!(sessions[0].get("parentID").is_none() && sessions[0].get("profileGUID").is_none() && sessions[0].get("peerSwitchShortcut").is_none());
        assert_eq!(sessions[3]["peerSwitchShortcut"], json!({"serialized": RELOAD}));
        assert_eq!(decode(&encode(&[w.clone()])).unwrap(), vec![w]);
        assert_eq!(decode(&encode(&[cc()])).unwrap(), vec![cc()]);
        // iTerm2's own bytes decode to the same model.
        assert_eq!(decode(&claude_json().to_string()).unwrap(), vec![cc()]);
    }

    #[test]
    fn decoding_tolerates_as_iterm2() {
        let base = |extra: Value| {
            let mut s = json!({"uniqueIdentifier": "r", "kind": {"kind": "root"}, "command": "", "urlString": "", "toolbarItems": []});
            for (k, v) in extra.as_object().unwrap() {
                s[k] = v.clone();
            }
            json!([{"uniqueIdentifier": "w", "name": "n", "sessions": [s]}]).to_string()
        };
        let list = decode(&base(json!({}))).unwrap();
        let s = &list[0].sessions[0];
        assert_eq!((s.per_file_command.as_str(), s.display_name.as_str(), s.mode), ("", "", Mode::Regular));
        // A mode of the wrong shape is Regular too (`try?`).
        assert_eq!(decode(&base(json!({"mode": "weird"}))).unwrap()[0].sessions[0].mode, Mode::Regular);
        let usage = |item: Value| decode(&base(json!({"toolbarItems": [item, {"kind": "gitStatus"}]}))).unwrap()[0].sessions[0].toolbar_items.clone();
        let want = ToolbarItem::Usage { provider: UsageProvider::AnthropicClaudeCode, command: String::new(), interval_seconds: 60.0 };
        assert_eq!(usage(json!({"kind": "usage"}))[0], want);
        let items = usage(json!({"kind": "usage", "provider": "futureThing"}));
        assert_eq!(items, vec![want, ToolbarItem::GitStatus]);
        assert_eq!(usage(json!({"kind": "navigation"}))[0], ToolbarItem::Navigation(NavigationShortcuts { back: None, forward: None, reload: None }));
        assert_eq!(usage(json!({"kind": "name"}))[0], ToolbarItem::Name);
        // An unknown session kind or toolbar item kind fails the whole decode.
        assert!(decode(&base(json!({"kind": {"kind": "weird"}}))).is_err());
        assert!(decode(&base(json!({"toolbarItems": [{"kind": "weird"}]}))).is_err());
        // Fields iTerm2 requires are required.
        assert!(decode(&base(json!({})).replace("\"command\":\"\",", "")).is_err());
        assert!(decode("{}").is_err() && decode("not json").is_err());
        assert_eq!(decode("[]").unwrap(), vec![]);
    }

    #[test]
    fn presets_follow_iterm2() {
        assert_eq!(presets::all(), vec![("codingAgentPlusDiff", "Coding Agent + Diff"), ("codingAgentPlusDiffPlusCodeReview", "Coding Agent + Diff + Code Review")]);
        let one = presets::build("codingAgentPlusDiff").unwrap();
        assert_eq!(one.name, "Coding Agent + Diff");
        let root = one.root().unwrap();
        assert_eq!((root.display_name.as_str(), root.kind.clone()), ("", Kind::Root));
        assert_eq!(root.toolbar_items, vec![ToolbarItem::ModeSwitcher, ToolbarItem::GitStatus]);
        let diff = one.children(&root.unique_identifier)[0];
        assert_eq!((diff.command.as_str(), diff.per_file_command.as_str(), diff.mode), ("git diff \\(gitBase)", "git diff \\(gitBase) -- \\(file)", Mode::Diff));
        assert_eq!(diff.toolbar_items, vec![ToolbarItem::ModeSwitcher, ToolbarItem::ChangedFileSelector, ToolbarItem::GitBaseSelector, ToolbarItem::Navigation(NavigationShortcuts::defaults())]);
        assert_eq!((diff.kind.clone(), diff.display_name.as_str()), (Kind::Peer, "Diff"));
        assert!(presets::build("nope").is_none());
        let two = presets::build("codingAgentPlusDiffPlusCodeReview").unwrap();
        assert_eq!(two.sessions.len(), 3);
        assert_eq!(two.name, "Coding Agent + Diff + Code Review");
        // The ids are fresh upper-case hyphenated uuids.
        assert!(two.unique_identifier.len() == 36 && two.unique_identifier == two.unique_identifier.to_uppercase());
        assert_ne!(presets::build("codingAgentPlusDiff").unwrap().unique_identifier, one.unique_identifier);
        // The Claude Code template is the same builder with the four fixed ids.
        let c = claude_code::config();
        assert_eq!((c.unique_identifier.as_str(), c.name.as_str()), ("builtin.claudeCode", "Claude Code"));
        let ids: Vec<&str> = c.sessions.iter().map(|s| s.unique_identifier.as_str()).collect();
        assert_eq!(ids, ["builtin.claudeCode.main", "builtin.claudeCode.diff", "builtin.claudeCode.review"]);
        assert_eq!((claude_code::WORKGROUP, claude_code::MAIN, claude_code::DIFF, claude_code::REVIEW), ("builtin.claudeCode", "builtin.claudeCode.main", "builtin.claudeCode.diff", "builtin.claudeCode.review"));
        assert_eq!(cc().sessions[0].display_name, "Chat");
        assert_eq!(cc().sessions[0].toolbar_items, vec![ToolbarItem::ModeSwitcher, ToolbarItem::GitStatus, ToolbarItem::AutoRequestReviewWhenIdle]);
        assert_eq!(cc().sessions[1].command, "git difftool -y -x vimdiff \\(gitBase)");
        assert_eq!(cc().sessions[1].per_file_command, "git difftool -y -x vimdiff \\(gitBase) -- \\(file)");
        assert_eq!(cc().sessions[2].command, REVIEW_CMD);
        assert_eq!(cc().sessions[2].toolbar_items, vec![ToolbarItem::ModeSwitcher, ToolbarItem::Reload(Some(NavigationShortcuts::reload_default())), ToolbarItem::AutoSendClippingsWhenIdle]);
        assert_eq!(cc().sessions[2].mode, Mode::CodeReview);
        // The default shortcuts are iTerm2's serialized form, and each parses back to itself.
        let d = NavigationShortcuts::defaults();
        assert_eq!((d.back.as_ref().unwrap().serialized.as_str(), d.forward.as_ref().unwrap().serialized.as_str(), d.reload.as_ref().unwrap().serialized.as_str()), (BACK, FORWARD, RELOAD));
        assert_eq!(NavigationShortcuts::reload_default().serialized, RELOAD);
        for s in [BACK, FORWARD, RELOAD] {
            assert_eq!(crate::keystroke::parse(s).unwrap().serialized(), s);
        }
        assert_eq!(Mode::Regular.title(), "Regular");
        assert_eq!((Mode::CodeReview.title(), Mode::Diff.title()), ("Code Review", "Diff"));
        assert_eq!(UsageProvider::AnthropicClaudeCode.display_name(), "Anthropic (Claude Code)");
        assert!(ToolbarItem::GitStatus.needs_git_poller() && ToolbarItem::ChangedFileSelector.needs_git_poller());
        assert!(!ToolbarItem::GitBaseSelector.needs_git_poller() && !ToolbarItem::ModeSwitcher.needs_git_poller());
        assert_eq!(ToolbarItem::Name.kind(), "name");
        let e = Workgroup::new_empty("Mine");
        assert_eq!((e.name.as_str(), e.sessions.len(), e.sessions[0].display_name.as_str()), ("Mine", 1, "Main"));
        assert_eq!(e.root().unwrap().kind, Kind::Root);
        assert_eq!(display_name(&e), "Mine");
        assert_eq!(display_name(&Workgroup::new_empty("")), "Untitled");
    }

    #[test]
    fn migrations_follow_iterm2() {
        // The Diff peer becomes Diff once; a set latch leaves a Regular one alone.
        let mut regular = cc();
        regular.sessions[1].mode = Mode::Regular;
        regular.unique_identifier = claude_code::WORKGROUP.into();
        regular.sessions[0].unique_identifier = claude_code::MAIN.into();
        regular.sessions[1].unique_identifier = claude_code::DIFF.into();
        regular.sessions[2].unique_identifier = claude_code::REVIEW.into();
        for s in &mut regular.sessions[1..] {
            s.parent_id = Some(claude_code::MAIN.into());
        }
        let mut list = vec![regular.clone()];
        let mut latches = Latches { shortcuts: true, system_prompt: true, auto_send: true, auto_request: true, diff_mode: false };
        assert!(migrate(&mut list, &mut latches));
        assert_eq!(list[0].sessions[1].mode, Mode::Diff);
        assert!(latches.diff_mode);
        let mut again = vec![regular.clone()];
        let mut l2 = Latches { diff_mode: true, shortcuts: true, system_prompt: true, auto_send: true, auto_request: true };
        assert!(!migrate(&mut again, &mut l2));
        assert_eq!(again[0].sessions[1].mode, Mode::Regular);
        // The system prompt argument, templated or resolved and unquoted, in any workgroup.
        let templ = "claude --append-system-prompt-file '/Applications/iTerm.app/Contents/Resources/code-review-system-prompt.txt' --settings x";
        let bare = "claude --append-system-prompt-file /Users/a/code-review-system-prompt.txt --settings x";
        let other = "claude --append-system-prompt-file '/my/own.txt' --settings x";
        let mut w = wg("any", vec![session("r", None, Kind::Root), session("a", Some("r"), Kind::Peer), session("b", Some("r"), Kind::Peer), session("c", Some("r"), Kind::Peer)]);
        w.sessions[1].command = templ.into();
        w.sessions[2].command = bare.into();
        w.sessions[3].command = other.into();
        let mut list = vec![w];
        let mut latches = Latches::default();
        assert!(migrate(&mut list, &mut latches));
        let fixed = "claude --append-system-prompt-file '\\(codeReviewSystemPromptFile)' --settings x";
        assert_eq!(list[0].sessions[1].command, fixed);
        assert_eq!(list[0].sessions[2].command, fixed);
        assert_eq!(list[0].sessions[3].command, other);
        assert!(latches.shortcuts && latches.diff_mode && latches.system_prompt && latches.auto_send && latches.auto_request);
        // The review peer gains auto-send and the main session auto-request when absent, never twice.
        let mut stripped = regular.clone();
        stripped.sessions[0].toolbar_items = vec![ToolbarItem::ModeSwitcher, ToolbarItem::GitStatus];
        stripped.sessions[2].toolbar_items = vec![ToolbarItem::ModeSwitcher];
        let mut list = vec![stripped];
        let mut latches = Latches::default();
        assert!(migrate(&mut list, &mut latches));
        assert_eq!(list[0].sessions[0].toolbar_items.last(), Some(&ToolbarItem::AutoRequestReviewWhenIdle));
        assert_eq!(list[0].sessions[2].toolbar_items.last(), Some(&ToolbarItem::AutoSendClippingsWhenIdle));
        let mut latches = Latches::default();
        assert!(!migrate(&mut list, &mut latches) || latches.shortcuts);
        let count = |items: &[ToolbarItem], k: &str| items.iter().filter(|i| i.kind() == k).count();
        assert_eq!(count(&list[0].sessions[0].toolbar_items, "autoRequestReviewWhenIdle"), 1);
        assert_eq!(count(&list[0].sessions[2].toolbar_items, "autoSendClippingsWhenIdle"), 1);
        // Navigation with no shortcuts and Reload(None) get the defaults; one shortcut set is left.
        let mut w = wg("nav", vec![session("r", None, Kind::Root), session("p", Some("r"), Kind::Peer)]);
        let one = NavigationShortcuts { back: Some(Shortcut { serialized: BACK.into() }), forward: None, reload: None };
        w.sessions[1].toolbar_items = vec![ToolbarItem::Navigation(NavigationShortcuts { back: None, forward: None, reload: None }), ToolbarItem::Reload(None), ToolbarItem::Navigation(one.clone())];
        let mut list = vec![w];
        let mut latches = Latches::default();
        assert!(migrate(&mut list, &mut latches));
        assert_eq!(list[0].sessions[1].toolbar_items, vec![ToolbarItem::Navigation(NavigationShortcuts::defaults()), ToolbarItem::Reload(Some(NavigationShortcuts::reload_default())), ToolbarItem::Navigation(one)]);
        // No data to change still sets the latches (and says so).
        let mut empty: Vec<Workgroup> = vec![];
        let mut latches = Latches::default();
        assert!(migrate(&mut empty, &mut latches));
        assert!(latches.shortcuts && latches.diff_mode && latches.system_prompt && latches.auto_send && latches.auto_request);
        assert!(!migrate(&mut empty, &mut latches));
    }

    #[test]
    fn model_loads_saves_and_migrates_through_globals() {
        let mut g = globals();
        let (m, changed) = Model::load(&g);
        assert!(m.workgroups.is_empty());
        // Latches only: the list is not written for a person who has none.
        assert!(changed);
        let mut m = m;
        m.save(&mut g);
        assert_eq!(g.string("Workgroups"), "");
        assert!(g.bool("NoSyncWorkgroupShortcutsBackfilled") && g.bool("NoSyncClaudeCodeDiffModeBackfilled") && g.bool("NoSyncClaudeCodeReviewSystemPromptCommandBackfilled"));
        assert!(g.bool("NoSyncClaudeCodeAutoSendClippingsBackfilled") && g.bool("NoSyncClaudeCodeAutoRequestReviewBackfilled"));
        let (m2, changed) = Model::load(&g);
        assert!(!changed && m2.workgroups.is_empty());
        // add / replace / remove / get / set_all.
        m.add(simple("a"));
        m.add(simple("b"));
        let mut a2 = simple("a");
        a2.name = "renamed".into();
        m.replace("a", a2.clone());
        m.replace("missing", simple("zzz"));
        assert_eq!(m.get("a"), Some(&a2));
        assert_eq!(m.workgroups.len(), 2);
        m.remove("b");
        m.remove("b");
        assert_eq!(m.workgroups.len(), 1);
        m.save(&mut g);
        assert_eq!(Model::load(&g).0.workgroups, vec![a2.clone()]);
        m.set_all(vec![simple("q"), simple("r")]);
        assert_eq!(m.workgroups.len(), 2);
        // A stored iTerm2 list that needs a migration is migrated and flagged for writing.
        let mut g2 = globals();
        let mut old = claude_code::config();
        old.sessions[1].mode = Mode::Regular;
        g2.set("Workgroups", toml::Value::String(encode(&[old])));
        let (m3, changed) = Model::load(&g2);
        assert!(changed);
        assert_eq!(m3.get("builtin.claudeCode").unwrap().sessions[1].mode, Mode::Diff);
        m3.save(&mut g2);
        assert_eq!(decode(&g2.string("Workgroups")).unwrap()[0].sessions[1].mode, Mode::Diff);
    }

    #[test]
    fn an_undecodable_value_is_left_untouched_until_the_first_edit() {
        let mut g = globals();
        let bad = r#"[{"uniqueIdentifier":"w","name":"n","sessions":[{"uniqueIdentifier":"r","kind":{"kind":"future"},"command":"","urlString":"","toolbarItems":[]}]}]"#;
        g.set("Workgroups", toml::Value::String(bad.into()));
        assert_eq!(g.string("Workgroups"), bad);
        let (mut m, changed) = Model::load(&g);
        assert!(m.workgroups.is_empty() && changed);
        m.save(&mut g);
        assert_eq!(g.string("Workgroups"), bad, "migrations must not overwrite a value that failed to decode");
        m.add(simple("fresh"));
        m.save(&mut g);
        assert_eq!(decode(&g.string("Workgroups")).unwrap().len(), 1);
    }

    #[test]
    fn provenance_is_recorded_per_workgroup() {
        let mut g = globals();
        g.set("Workgroups", toml::Value::String(encode(&[simple("disk")])));
        let (mut m, _) = Model::load(&g);
        // From disk with no stored mark: the person's own settings (the trust ruling).
        assert_eq!(m.source_of("disk"), Source::Person);
        m.add(simple("mine"));
        assert_eq!(m.source_of("mine"), Source::Person);
        assert!(m.source_of("mine").is_person());
        m.add_from(simple("imp"), Source::Import);
        m.add_from(simple("rem"), Source::RemotePrefs);
        m.add_from(simple("dyn"), Source::DynamicProfile);
        assert_eq!((m.source_of("imp"), m.source_of("rem"), m.source_of("dyn")), (Source::Import, Source::RemotePrefs, Source::DynamicProfile));
        // A person's edit of an imported workgroup keeps its mark; a replace from another source marks it.
        // An edit keeps the mark (only `trust` clears it).
        m.replace("imp", simple("imp"));
        assert_eq!(m.source_of("imp"), Source::Import);
        m.replace_from("mine", simple("mine"), Source::RemotePrefs);
        assert_eq!(m.source_of("mine"), Source::RemotePrefs);
        // Unknown ids and a removed workgroup are never the person's.
        assert_eq!(m.source_of("nope"), Source::Stored);
        m.remove("mine");
        assert_eq!(m.source_of("mine"), Source::Stored);
        m.set_all_from(vec![simple("x")], Source::Import);
        assert_eq!(m.source_of("x"), Source::Import);
        assert_eq!(m.source_of("imp"), Source::Stored);
        // A reload from the globals marks everything with the stated source.
        let mut g3 = globals();
        g3.set("Workgroups", toml::Value::String(encode(&[simple("p")])));
        m.reload_from(&g3, Source::RemotePrefs);
        assert_eq!((m.workgroups.len(), m.source_of("p")), (1, Source::RemotePrefs));
    }

    #[test]
    fn every_import_path_stores_the_imported_mark() {
        // Trust ruling: workgroups from the person's own settings are the person's; every import
        // path marks its workgroups Imported and the mark is stored, so it survives a restart.
        for source in [Source::Import, Source::RemotePrefs, Source::DynamicProfile] {
            let paths: [(&str, Box<dyn Fn(&mut Model, &mut Globals)>); 4] = [
                ("add_from", Box::new(move |m, _| m.add_from(simple("w"), source))),
                ("replace_from", Box::new(move |m, _| m.replace_from("w", simple("w"), source))),
                ("set_all_from", Box::new(move |m, _| m.set_all_from(vec![simple("w")], source))),
                ("reload_from", Box::new(move |m, g| {
                    g.set("Workgroups", toml::Value::String(encode(&[simple("w")])));
                    m.reload_from(g, source)
                })),
            ];
            for (name, run) in paths {
                let mut g = globals();
                let mut m = Model::default();
                m.add(simple("w"));
                run(&mut m, &mut g);
                assert!(!m.source_of("w").is_person(), "{name} {source:?}");
                m.save(&mut g);
                let (back, _) = Model::load(&g);
                assert_eq!(back.source_of("w"), Source::Import, "{name} {source:?} after a restart");
                assert!(!back.source_of("w").is_person());
            }
        }
        // The person's own workgroups come back as the person's; an edit of an import is the person's.
        let mut g = globals();
        let mut m = Model::default();
        m.add(simple("mine"));
        m.add_from(simple("theirs"), Source::Import);
        m.save(&mut g);
        let (mut back, _) = Model::load(&g);
        assert_eq!((back.source_of("mine"), back.source_of("theirs")), (Source::Person, Source::Import));
        back.trust("theirs");
        back.save(&mut g);
        let (again, _) = Model::load(&g);
        assert_eq!((again.source_of("mine"), again.source_of("theirs")), (Source::Person, Source::Person));
        // The mark list is bounded at the one write point.
        g.set("WorkgroupsImported", toml::Value::String("not json".into()));
        assert_eq!(g.string("WorkgroupsImported"), "");
        g.set("WorkgroupsImported", toml::Value::String(format!("[\"ok\", \"bad id\", \"{}\"]", big_text(300))));
        assert_eq!(g.string("WorkgroupsImported"), "[\"ok\"]");
    }

    #[test]
    fn an_edit_keeps_the_mark_and_only_trust_clears_it() {
        let mut g = globals();
        let mut m = Model::default();
        m.add_from(simple("theirs"), Source::Import);
        m.save(&mut g);
        let (mut m, _) = Model::load(&g);
        // A rename, a command edit, an undo: none of them approves the commands.
        let mut renamed = simple("theirs");
        renamed.name = "renamed".into();
        m.replace("theirs", renamed.clone());
        assert_eq!(m.source_of("theirs"), Source::Import);
        let mut cmd = renamed.clone();
        cmd.sessions[0].command = "calc".into();
        m.set_all(vec![cmd]);
        assert_eq!(m.source_of("theirs"), Source::Import);
        m.save(&mut g);
        let (mut m, _) = Model::load(&g);
        assert_eq!(m.source_of("theirs"), Source::Import);
        // A workgroup the editor creates is the person's; copies of imports are new ids and so are too.
        m.add(simple("mine"));
        assert_eq!(m.source_of("mine"), Source::Person);
        // The explicit act.
        m.trust("theirs");
        assert_eq!(m.source_of("theirs"), Source::Person);
        m.save(&mut g);
        let (back, _) = Model::load(&g);
        assert_eq!(back.source_of("theirs"), Source::Person);
        // An import over the top marks it again, whatever the incoming data says.
        let mut m = back;
        m.replace_from("theirs", simple("theirs"), Source::Import);
        assert_eq!(m.source_of("theirs"), Source::Import);
    }

    #[test]
    fn a_foreign_write_forces_the_mark() {
        let mut g = globals();
        let mut m = Model::default();
        m.add(simple("mine"));
        m.add(simple("same"));
        m.add_from(simple("theirs"), Source::Import);
        m.save(&mut g);
        let before = (g.string(KEY), g.string(KEY_IMPORTED));
        // A snapshot or plist rewrites the list, changes "mine", adds "new", and tries to clear the marks.
        let mut changed = simple("mine");
        changed.sessions[0].command = "calc".into();
        g.set(KEY, toml::Value::String(encode(&[changed, simple("same"), simple("theirs"), simple("new")])));
        g.set(KEY_IMPORTED, toml::Value::String(String::new()));
        mark_foreign_write(&mut g, &before.0, &before.1);
        let (back, _) = Model::load(&g);
        assert_eq!(back.source_of("same"), Source::Person, "unchanged data stays the person's");
        for id in ["mine", "theirs", "new"] {
            assert_eq!(back.source_of(id), Source::Import, "{id}");
        }
    }

    fn big_text(n: usize) -> String {
        "a".repeat(n)
    }

    #[test]
    fn decode_is_bounded_and_panic_free() {
        // Count: at most 64 workgroups survive.
        let many: Vec<Workgroup> = (0..100).map(|i| simple(&format!("w{i}"))).collect();
        assert_eq!(decode(&encode(&many)).unwrap().len(), 64);
        // Sessions: 32 per workgroup; a workgroup above that is dropped, the list survives.
        let mut tree = vec![session("root", None, Kind::Root)];
        for i in 0..40 {
            tree.push(session(&format!("p{i}"), Some("root"), Kind::Peer));
        }
        let list = decode(&encode(&[wg("big", tree), simple("ok")])).unwrap();
        assert_eq!(list.iter().map(|w| w.unique_identifier.as_str()).collect::<Vec<_>>(), ["ok"]);
        // Size: one MiB and over is refused whole.
        let huge = format!("[{{\"uniqueIdentifier\":\"w\",\"name\":\"{}\",\"sessions\":[]}}]", big_text(MAX_BYTES));
        assert!(decode(&huge).is_err());
        // Depth: a deeply nested array never overflows the stack.
        let deep = format!("{}{}", "[".repeat(100_000), "]".repeat(100_000));
        assert!(decode(&deep).is_err());
        // Garbage of every shape is an error or a list, never a panic.
        for text in ["", "[", "null", "[null]", "[1]", "[{}]", "[{\"uniqueIdentifier\":1}]", "\u{0}", "[[[[[]]]]]", "{\"a\":[]}", "[{\"uniqueIdentifier\":\"w\",\"name\":\"n\",\"sessions\":5}]"] {
            let _ = decode(text);
        }
    }

    #[test]
    fn decode_drops_a_workgroup_that_breaks_a_limit_and_keeps_the_rest() {
        let ok = simple("ok");
        let dropped = |w: Workgroup| decode(&encode(&[w, ok.clone()])).unwrap().iter().map(|w| w.unique_identifier.clone()).collect::<Vec<_>>() == ["ok"];
        // Structure: one root; every parent resolves to an earlier session; ids unique.
        assert!(dropped(wg("two-roots", vec![session("a", None, Kind::Root), session("b", None, Kind::Root)])));
        assert!(dropped(wg("no-root", vec![session("a", Some("b"), Kind::Peer)])));
        assert!(dropped(wg("empty", vec![])));
        assert!(dropped(wg("forward-parent", vec![session("r", None, Kind::Root), session("a", Some("b"), Kind::Peer), session("b", Some("r"), Kind::Peer)])));
        assert!(dropped(wg("dup", vec![session("r", None, Kind::Root), session("r", Some("r"), Kind::Peer)])));
        assert!(dropped(wg("self-parent", vec![session("r", None, Kind::Root), session("a", Some("a"), Kind::Peer)])));
        // Ids: charset and length.
        assert!(dropped(wg("bad id", vec![session("r", None, Kind::Root)])));
        assert!(dropped(wg(&big_text(129), vec![session("r", None, Kind::Root)])));
        assert!(dropped(wg("w", vec![session("r/..\\x", None, Kind::Root)])));
        // Commands: no control characters or NUL, at most 4 KiB.
        for bad in ["a\nb", "a\u{0}b", "a\u{3}b", "a\tb", "a\rb"] {
            let mut s = session("r", None, Kind::Root);
            s.command = bad.into();
            assert!(dropped(wg("cmd", vec![s.clone()])), "{bad:?}");
            s.command = String::new();
            s.per_file_command = bad.into();
            assert!(dropped(wg("cmd", vec![s])), "{bad:?}");
        }
        let mut s = session("r", None, Kind::Root);
        s.command = big_text(4097);
        assert!(dropped(wg("long", vec![s.clone()])));
        s.command = big_text(4096);
        assert_eq!(decode(&encode(&[wg("long", vec![s])])).unwrap().len(), 1);
        let mut u = session("r", None, Kind::Root);
        u.toolbar_items = vec![ToolbarItem::Usage { provider: UsageProvider::AnthropicClaudeCode, command: "x\ny".into(), interval_seconds: 60.0 }];
        assert!(dropped(wg("usage", vec![u.clone()])));
        u.toolbar_items = vec![ToolbarItem::Usage { provider: UsageProvider::AnthropicClaudeCode, command: big_text(1025), interval_seconds: 60.0 }];
        assert!(dropped(wg("usage", vec![u])));
    }

    #[test]
    fn decode_normalizes_what_it_keeps() {
        let mut r = session("r", None, Kind::Root);
        r.display_name = format!("a\u{202e}b\u{0}{}", big_text(300));
        r.url_string = "javascript:alert(1)".into();
        r.profile_guid = Some("not a guid".into());
        r.toolbar_items = (0..40).map(|_| ToolbarItem::GitStatus).collect();
        let mut p = session("p", Some("r"), Kind::Peer);
        p.url_string = "https://user:pw@example.com/x#frag".into();
        p.profile_guid = Some("0F3C5E3A-1D2B-4C6A-9E8F-123456789ABC".into());
        p.toolbar_items = vec![
            ToolbarItem::Spacer { min_width: -5.0, max_width: 1e9 },
            ToolbarItem::Usage { provider: UsageProvider::AnthropicClaudeCode, command: String::new(), interval_seconds: 0.0 },
            ToolbarItem::Usage { provider: UsageProvider::AnthropicClaudeCode, command: String::new(), interval_seconds: 1e12 },
            ToolbarItem::Reload(Some(Shortcut { serialized: "garbage".into() })),
        ];
        p.peer_switch_shortcut = Some(Shortcut { serialized: "also garbage".into() });
        let mut w = wg("w", vec![r, p]);
        w.name = format!("n\u{202e}x{}", big_text(200));
        let got = decode(&encode(&[w])).unwrap().remove(0);
        assert!(got.name.chars().count() <= 128 && !got.name.contains('\u{202e}'));
        let (r, p) = (&got.sessions[0], &got.sessions[1]);
        assert!(r.display_name.chars().count() <= 128 && !r.display_name.contains('\u{202e}') && !r.display_name.contains('\u{0}'));
        assert_eq!(r.url_string, "");
        assert_eq!(r.profile_guid, None);
        assert_eq!(r.toolbar_items.len(), 32);
        assert!(!p.url_string.contains("pw") && !p.url_string.contains("frag") && p.url_string.starts_with("https://"), "{}", p.url_string);
        assert_eq!(p.profile_guid.as_deref(), Some("0F3C5E3A-1D2B-4C6A-9E8F-123456789ABC"));
        assert_eq!(p.toolbar_items[0], ToolbarItem::Spacer { min_width: 0.0, max_width: 4096.0 });
        assert_eq!(p.toolbar_items[1], ToolbarItem::Usage { provider: UsageProvider::AnthropicClaudeCode, command: String::new(), interval_seconds: 5.0 });
        assert_eq!(p.toolbar_items[2], ToolbarItem::Usage { provider: UsageProvider::AnthropicClaudeCode, command: String::new(), interval_seconds: 86400.0 });
        assert_eq!(p.toolbar_items[3], ToolbarItem::Reload(None));
        assert_eq!(p.peer_switch_shortcut, None);
    }

    #[test]
    fn workgroups_are_bounded_at_the_one_write_point() {
        let mut g = globals();
        // Globals::set cleans a value that decodes but breaks a limit, keeping the decodable workgroups.
        let mut s = session("r", None, Kind::Root);
        s.command = "a\nb".into();
        let text = encode(&[wg("evil", vec![s]), simple("ok")]);
        g.set("Workgroups", toml::Value::String(text));
        let kept = decode(&g.string("Workgroups")).unwrap();
        assert_eq!(kept.iter().map(|w| w.unique_identifier.as_str()).collect::<Vec<_>>(), ["ok"]);
        // A value over 1 MiB is not stored.
        g.set("Workgroups", toml::Value::String(big_text(MAX_BYTES + 1)));
        assert_eq!(g.string("Workgroups"), "");
        // A value of another type is not stored.
        g.set("Workgroups", toml::Value::Integer(3));
        assert_eq!(g.string("Workgroups"), "");
        // A clean value keeps iTerm2's bytes (no re-encoding).
        let theirs = claude_json().to_string();
        assert_eq!(bound_json(&theirs), theirs);
        g.set("Workgroups", toml::Value::String(theirs.clone()));
        assert_eq!(g.string("Workgroups"), theirs);
        // Bulk writers (a remote-prefs snapshot) get the same bound through bound_values.
        let mut s = session("r", None, Kind::Root);
        s.command = "x\u{0}y".into();
        let evil = encode(&[wg("evil", vec![s]), simple("fine")]);
        let path = std::env::temp_dir().join(format!("tandem-wg-bound-{}.toml", std::process::id()));
        std::fs::write(&path, toml::to_string(&toml::toml! { [settings] Workgroups = evil }).unwrap()).unwrap();
        let loaded = Globals::load(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        let ids: Vec<String> = decode(&loaded.string("Workgroups")).unwrap().into_iter().map(|w| w.unique_identifier).collect();
        assert_eq!(ids, ["fine"]);
        // An undecodable but small value is kept as written.
        assert_eq!(bound_json("[{\"uniqueIdentifier\":1}]"), "[{\"uniqueIdentifier\":1}]");
        assert_eq!(bound_json("not json"), "not json");
    }

    #[test]
    fn workgroups_stored_row_cannot_bypass_the_bound() {
        // The Stored tab writes through Globals::set like every writer; the row is a plain string
        // (no `hidden` attribute exists) and the latch rows are bools.
        for key in ["Workgroups"] {
            let row = crate::schema::find_global(key).expect(key);
            assert_eq!((row.tab.as_str(), row.kind), ("Stored", crate::schema::Kind::String), "{key}");
            assert_eq!(row.default, Some(toml::Value::String(String::new())));
        }
        for key in ["NoSyncWorkgroupShortcutsBackfilled", "NoSyncClaudeCodeDiffModeBackfilled", "NoSyncClaudeCodeReviewSystemPromptCommandBackfilled", "NoSyncClaudeCodeAutoSendClippingsBackfilled", "NoSyncClaudeCodeAutoRequestReviewBackfilled"] {
            let row = crate::schema::find_global(key).expect(key);
            assert_eq!((row.tab.as_str(), row.kind), ("Stored", crate::schema::Kind::Bool), "{key}");
            assert_eq!(row.default, Some(toml::Value::Boolean(false)));
        }
        // Typing a hostile value into that row is the same call.
        let mut g = globals();
        g.set("Workgroups", toml::Value::String(big_text(2 * MAX_BYTES)));
        assert_eq!(g.string("Workgroups"), "");
    }
}
