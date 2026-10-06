//! Dynamic Profiles: profiles kept in JSON or property list files in a watched folder
//! (`iTermDynamicProfileManager.m`). This is the model: the folder rule, file parsing, parent
//! merging, the rewrite rule and the texts. The folder is untrusted input: every file is bounded
//! (size, entries, profiles, keys, strings, nesting), every path passes the one network judge
//! (`scripts::stays_local`), every profile is built through `Profile::from_values` and
//! `Profile::set` (13t's one write point), and a parse never panics.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};

use serde_json::Value as Json;
use toml::Value;

use crate::profile::Profile;

/// One profile dictionary as the file holds it (iTerm2's forms: color dictionaries, strings).
pub type Entry = serde_json::Map<String, Json>;

/// The tag `AddDynamicTagToDynamicProfiles` adds (`kProfileDynamicTag`).
pub const TAG: &str = "Dynamic";
/// The file a dynamic profile came from (`KEY_DYNAMIC_PROFILE_FILENAME`); never written to a file.
pub const FILENAME_KEY: &str = "Dynamic Profile Filename";
pub const PARENT_NAME_KEY: &str = "Dynamic Profile Parent Name";
pub const PARENT_GUID_KEY: &str = "Dynamic Profile Parent GUID";
pub const REWRITABLE_KEY: &str = "Rewritable";
pub const DYNAMIC_KEY: &str = "Is Dynamic Profile";
const SEPARATE_COLORS_KEY: &str = "Use Separate Colors for Light and Dark Mode";
const DEFAULT_BOOKMARK_KEY: &str = "Default Bookmark";

/// A file longer than this is not read (13v2 B6).
pub const MAX_FILE: usize = 1 << 20;
/// A folder with more entries than this loads nothing.
pub const MAX_ENTRIES: usize = 512;
/// A path with more parts than this is skipped.
pub const MAX_DEPTH: usize = 8;
/// Profiles across all files; a file that would pass it is refused whole.
pub const MAX_PROFILES: usize = 256;
/// Keys in one profile dictionary.
pub const MAX_KEYS: usize = 512;
/// The longest string or key in a file.
pub const MAX_STRING: usize = 64 * 1024;
/// Nesting of arrays and dictionaries in a file.
pub const MAX_JSON_DEPTH: usize = 32;
/// Values in a property list once every reference is expanded.
const MAX_PLIST_VALUES: u64 = 100_000;

pub const ALERT_HEADING: &str = "Dynamic Profiles Error";
pub const VIEW_LOG: &str = "View Log";
pub const REVEAL: &str = "Reveal in Explorer";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileType {
    Json,
    PropertyList,
}

/// The folder `DynamicProfilesPath` names (`~` is the home folder), else
/// `%APPDATA%\Tandem\DynamicProfiles`.
pub fn folder(custom: &str, appdata: &Path, home: &Path) -> PathBuf {
    let custom = custom.trim();
    if custom.is_empty() {
        return appdata.join("Tandem").join("DynamicProfiles");
    }
    if let Some(rest) = custom.strip_prefix('~') {
        if rest.is_empty() {
            return home.to_path_buf();
        }
        if rest.starts_with(['\\', '/']) {
            return home.join(rest.trim_start_matches(['\\', '/']));
        }
    }
    PathBuf::from(custom)
}

/// The names to load, sorted: a leading "." and a trailing "~" skipped (`DP:292-306`). The names
/// are paths below the folder with "/" between the parts, as `enumeratorAtPath:` gives them.
pub fn files(names: &[String]) -> Vec<String> {
    let mut kept: Vec<String> = names.iter().filter(|n| !n.starts_with('.') && !n.ends_with('~')).cloned().collect();
    kept.sort();
    kept
}

/// Reads at most `limit` bytes of a file: its size is checked first, and the read is cut one
/// byte past the limit in case it grew. A folder or device is no file.
pub fn bounded_read(path: &Path, limit: usize) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
    if !meta.is_file() {
        return Err("not a file".into());
    }
    if meta.len() > limit as u64 {
        return Err(format!("file too large (the limit is {limit} bytes)"));
    }
    let mut bytes = Vec::new();
    std::fs::File::open(path).map_err(|e| e.to_string())?.take(limit as u64 + 1).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    if bytes.len() > limit {
        return Err(format!("file too large (the limit is {limit} bytes)"));
    }
    Ok(bytes)
}

/// Every entry below `root`, subfolders included, as "/" paths (`enumeratorAtPath:`). A link or
/// junction is listed but never entered; an entry `local` refuses is neither listed nor entered;
/// nothing goes deeper than `MAX_DEPTH` folders. Stops one entry past `max`, which is enough to
/// tell a folder is too big.
pub fn list_dir(root: &Path, local: &dyn Fn(&Path) -> bool, max: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack: Vec<(PathBuf, String, usize)> = vec![(root.to_path_buf(), String::new(), 0)];
    while let Some((dir, prefix, depth)) = stack.pop() {
        let Ok(read) = std::fs::read_dir(&dir) else { continue };
        for entry in read.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            let rel = if prefix.is_empty() { name } else { format!("{prefix}/{name}") };
            let full = entry.path();
            if !local(&full) {
                continue;
            }
            out.push(rel.clone());
            if out.len() > max {
                return out;
            }
            let Ok(meta) = std::fs::symlink_metadata(&full) else { continue };
            if meta.is_dir() && !is_link(&meta) && depth < MAX_DEPTH {
                stack.push((full, rel, depth + 1));
            }
        }
    }
    out
}

fn is_link(meta: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const REPARSE_POINT: u32 = 0x400;
        meta.file_type().is_symlink() || meta.file_attributes() & REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    {
        meta.file_type().is_symlink()
    }
}

// ---- Parsing ---------------------------------------------------------------------------------

fn too_large(path: &str) -> String {
    format!("Dynamic Profiles file {path} is too large (the limit is 1 MiB).")
}

/// How deep `[` and `{` nest in JSON text, counted without recursion and skipping strings.
fn json_depth(bytes: &[u8]) -> usize {
    let (mut depth, mut max, mut in_string, mut escaped) = (0usize, 0usize, false, false);
    for &b in bytes {
        if in_string {
            match (escaped, b) {
                (true, _) => escaped = false,
                (false, b'\\') => escaped = true,
                (false, b'"') => in_string = false,
                _ => {}
            }
            continue;
        }
        match b {
            b'"' => in_string = true,
            b'[' | b'{' => {
                depth += 1;
                max = max.max(depth);
            }
            b']' | b'}' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    max
}

/// How deep `<array>` and `<dict>` nest in XML text, counted without recursion. Over-counts
/// (a comment holding a tag counts), never under-counts.
fn xml_depth(bytes: &[u8]) -> usize {
    let (mut depth, mut max, mut i) = (0usize, 0usize, 0usize);
    while i < bytes.len() {
        if bytes[i] != b'<' {
            i += 1;
            continue;
        }
        let closing = bytes.get(i + 1) == Some(&b'/');
        let start = i + 1 + usize::from(closing);
        let mut end = start;
        while end < bytes.len() && (bytes[end].is_ascii_alphanumeric() || matches!(bytes[end], b'_' | b':' | b'-')) {
            end += 1;
        }
        let name = &bytes[start..end];
        let mut close = end;
        while close < bytes.len() && bytes[close] != b'>' {
            close += 1;
        }
        if name == b"array" || name == b"dict" {
            if closing {
                depth = depth.saturating_sub(1);
            } else if close == 0 || bytes.get(close - 1) != Some(&b'/') {
                depth += 1;
                max = max.max(depth);
            }
        }
        i = close.max(i) + 1;
    }
    max
}

/// Checks a binary property list before the plist crate reads it: its declared counts and
/// offsets fit the file, no object refers back to itself, it is no deeper than `MAX_JSON_DEPTH`
/// and it holds at most `MAX_PLIST_VALUES` values once every reference is expanded (a shared
/// array of two references to the next, thirty times over, is 2^30 values in 100 bytes).
fn bplist_check(bytes: &[u8]) -> Result<(), String> {
    let len = bytes.len();
    if len < 8 + 32 {
        return Err("too short".into());
    }
    let t = &bytes[len - 32..];
    let (offset_size, ref_size) = (t[6] as usize, t[7] as usize);
    let be = |s: &[u8]| s.iter().fold(0u64, |a, &b| (a << 8) | u64::from(b));
    let (count, top, table) = (be(&t[8..16]), be(&t[16..24]), be(&t[24..32]));
    if !(1..=8).contains(&offset_size) || !(1..=8).contains(&ref_size) {
        return Err("bad offset sizes".into());
    }
    if count == 0 || count > len as u64 || count > MAX_PLIST_VALUES {
        return Err(format!("{count} objects do not fit"));
    }
    if top >= count {
        return Err("bad top object".into());
    }
    let table_end = table.checked_add(count * offset_size as u64).ok_or("bad offset table")?;
    if table < 8 || table_end > (len - 32) as u64 {
        return Err("bad offset table".into());
    }
    let offsets: Vec<usize> = (0..count as usize)
        .map(|i| {
            let at = table as usize + i * offset_size;
            be(&bytes[at..at + offset_size]) as usize
        })
        .collect();
    struct Walk<'a> {
        bytes: &'a [u8],
        offsets: Vec<usize>,
        ref_size: usize,
        end: usize,
        memo: Vec<Option<(u64, usize)>>,
        on_stack: Vec<bool>,
    }
    impl Walk<'_> {
        fn be(&self, at: usize, n: usize) -> Result<u64, String> {
            let s = self.bytes.get(at..at + n).filter(|_| at + n <= self.end).ok_or("object out of range")?;
            Ok(s.iter().fold(0u64, |a, &b| (a << 8) | u64::from(b)))
        }
        /// (expanded size, height) of an object.
        fn visit(&mut self, obj: usize, depth: usize) -> Result<(u64, usize), String> {
            if depth > MAX_JSON_DEPTH * 2 {
                return Err("nested too deeply".into());
            }
            if let Some(m) = self.memo[obj] {
                return Ok(m);
            }
            if self.on_stack[obj] {
                return Err("an object contains itself".into());
            }
            let at = self.offsets[obj];
            let marker = *self.bytes.get(at).filter(|_| at < self.end).ok_or("object out of range")?;
            let (kind, low) = (marker >> 4, (marker & 0xF) as usize);
            let result = if matches!(kind, 0xA | 0xC | 0xD) {
                let (count, mut p) = if low == 0xF {
                    let int = *self.bytes.get(at + 1).ok_or("object out of range")?;
                    if int >> 4 != 1 || (int & 0xF) > 3 {
                        return Err("bad count".into());
                    }
                    let n = 1usize << (int & 0xF);
                    (self.be(at + 2, n)? as usize, at + 2 + n)
                } else {
                    (low, at + 1)
                };
                let refs = if kind == 0xD { count.checked_mul(2) } else { Some(count) }.ok_or("bad count")?;
                if refs > self.end {
                    return Err("bad count".into());
                }
                self.on_stack[obj] = true;
                let (mut size, mut height) = (1u64, 0usize);
                for _ in 0..refs {
                    let child = self.be(p, self.ref_size)? as usize;
                    p += self.ref_size;
                    if child >= self.offsets.len() {
                        self.on_stack[obj] = false;
                        return Err("bad reference".into());
                    }
                    let (s, h) = match self.visit(child, depth + 1) {
                        Ok(v) => v,
                        Err(e) => {
                            self.on_stack[obj] = false;
                            return Err(e);
                        }
                    };
                    size = size.saturating_add(s);
                    height = height.max(h);
                    if size > MAX_PLIST_VALUES {
                        self.on_stack[obj] = false;
                        return Err("too many values".into());
                    }
                }
                self.on_stack[obj] = false;
                (size, height + 1)
            } else {
                (1, 1)
            };
            self.memo[obj] = Some(result);
            Ok(result)
        }
    }
    let n = offsets.len();
    let mut walk = Walk { bytes, offsets, ref_size, end: table as usize, memo: vec![None; n], on_stack: vec![false; n] };
    let (_, height) = walk.visit(top as usize, 0)?;
    if height > MAX_JSON_DEPTH {
        return Err("nested too deeply".into());
    }
    Ok(())
}

/// A property list value as iTerm2's dictionaries hold it: dates as text, data and UIDs dropped.
fn plist_to_json(v: &plist::Value, depth: usize) -> Option<Json> {
    if depth > MAX_JSON_DEPTH {
        return None;
    }
    Some(match v {
        plist::Value::Boolean(b) => Json::Bool(*b),
        plist::Value::Integer(i) => match i.as_signed() {
            Some(n) => Json::from(n),
            None => Json::from(i.as_unsigned()?),
        },
        plist::Value::Real(f) => Json::from(serde_json::Number::from_f64(*f)?),
        plist::Value::String(s) => Json::String(s.clone()),
        plist::Value::Date(d) => Json::String(d.to_xml_format()),
        plist::Value::Array(a) => Json::Array(a.iter().filter_map(|v| plist_to_json(v, depth + 1)).collect()),
        plist::Value::Dictionary(d) => Json::Object(d.iter().filter_map(|(k, v)| Some((k.clone(), plist_to_json(v, depth + 1)?))).collect()),
        _ => return None,
    })
}

fn json_to_plist(v: &Json) -> Option<plist::Value> {
    Some(match v {
        Json::Null => return None,
        Json::Bool(b) => plist::Value::Boolean(*b),
        Json::Number(n) => match (n.as_i64(), n.as_u64()) {
            (Some(i), _) => plist::Value::Integer(i.into()),
            (None, Some(u)) => plist::Value::Integer(u.into()),
            _ => plist::Value::Real(n.as_f64()?),
        },
        Json::String(s) => plist::Value::String(s.clone()),
        Json::Array(a) => plist::Value::Array(a.iter().filter_map(json_to_plist).collect()),
        Json::Object(o) => plist::Value::Dictionary(o.iter().filter_map(|(k, v)| Some((k.clone(), json_to_plist(v)?))).collect()),
    })
}

/// The root dictionary of a file and its type: a property list (XML or binary) first, then JSON,
/// as `profilesInFile:` does. The error is the text to report.
fn read_root(path: &str, bytes: &[u8]) -> Result<(Entry, FileType), String> {
    let trimmed = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
    let first = trimmed.iter().copied().find(|b| !b.is_ascii_whitespace());
    let plist_value = if bytes.starts_with(b"bplist00") {
        bplist_check(bytes).map_err(|e| format!("Dynamic Profiles file {path} is not a usable property list: {e}"))?;
        Some(parse_plist(bytes).ok_or_else(|| format!("Dynamic Profiles file {path} is not a usable property list."))?)
    } else if first == Some(b'<') {
        if xml_depth(bytes) > MAX_JSON_DEPTH {
            return Err(format!("Dynamic Profiles file {path} is nested too deeply (the limit is {MAX_JSON_DEPTH} levels)."));
        }
        parse_plist(bytes)
    } else {
        None
    };
    if let Some(plist::Value::Dictionary(d)) = plist_value {
        return match plist_to_json(&plist::Value::Dictionary(d), 0) {
            Some(Json::Object(o)) => Ok((o, FileType::PropertyList)),
            _ => Err(format!("Dynamic Profiles file {path} is nested too deeply (the limit is {MAX_JSON_DEPTH} levels).")),
        };
    }
    if json_depth(trimmed) > MAX_JSON_DEPTH {
        return Err(format!("Dynamic Profiles file {path} is nested too deeply (the limit is {MAX_JSON_DEPTH} levels)."));
    }
    match serde_json::from_slice::<Json>(trimmed) {
        Err(e) => Err(format!("Dynamic Profiles file {path} contains invalid JSON: {e}")),
        Ok(Json::Object(o)) => Ok((o, FileType::Json)),
        Ok(_) => Err(format!("Dynamic Profiles file {path} does not have an Object (i.e., a dictionary) as its root element")),
    }
}

/// A property list through the plist crate, a panic in it counting as unreadable.
fn parse_plist(bytes: &[u8]) -> Option<plist::Value> {
    std::panic::catch_unwind(|| plist::Value::from_reader(std::io::Cursor::new(bytes)).ok()).ok().flatten()
}

/// Whether a string value or key anywhere in `v` is over `MAX_STRING`.
fn has_long_string(v: &Json, depth: usize) -> bool {
    if depth > MAX_JSON_DEPTH + 1 {
        return true;
    }
    match v {
        Json::String(s) => s.len() > MAX_STRING,
        Json::Array(a) => a.iter().any(|v| has_long_string(v, depth + 1)),
        Json::Object(o) => o.iter().any(|(k, v)| k.len() > MAX_STRING || has_long_string(v, depth + 1)),
        _ => false,
    }
}

/// A Guid is 1 to 128 characters of letters, digits and ". _ { } -" (it becomes the profile's
/// identity, a file name in places and a session's Guid).
fn valid_guid(g: &str) -> bool {
    (1..=128).contains(&g.len()) && g.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '{' | '}' | '-'))
}

/// The entries a file keeps and the errors it gives (`profilesInFile:`, `DP:342-420`). None when
/// the file as a whole is refused; an entry without a usable Guid or Name, or whose Guid a saved
/// profile has (`saved_guids`: a dynamic profile never replaces a person's own), is dropped alone.
/// `path` only appears in the texts.
pub fn parse_file(path: &str, bytes: &[u8], saved_guids: &dyn Fn(&str) -> bool) -> (Option<(Vec<Entry>, FileType)>, Vec<String>) {
    if bytes.len() > MAX_FILE {
        return (None, vec![too_large(path)]);
    }
    let (root, kind) = match read_root(path, bytes) {
        Ok(r) => r,
        Err(e) => return (None, vec![e]),
    };
    let Some(Json::Array(list)) = root.get("Profiles") else {
        return (None, vec![format!("Dynamic Profiles file {path} does not have a \u{201c}Profiles\u{201d} key at the root.")]);
    };
    if list.len() > MAX_PROFILES {
        return (None, vec![format!("Dynamic Profiles file {path} has more than {MAX_PROFILES} profiles.")]);
    }
    for entry in list {
        if entry.as_object().is_some_and(|o| o.len() > MAX_KEYS) {
            return (None, vec![format!("Dynamic Profiles file {path} has a profile with more than {MAX_KEYS} keys.")]);
        }
        if has_long_string(entry, 0) {
            return (None, vec![format!("Dynamic Profiles file {path} has a value longer than 64 KiB.")]);
        }
    }
    let mut errors = Vec::new();
    let mut kept = Vec::new();
    for entry in list {
        let Some(guid) = entry.get("Guid").and_then(Json::as_str) else {
            errors.push(format!("Dynamic profile is missing the Guid field in file {path}"));
            continue;
        };
        if !valid_guid(guid) {
            errors.push(format!("Dynamic profile has an invalid Guid {} in file {path}", crate::profile_list::display_text(guid)));
            continue;
        }
        let Some(name) = entry.get("Name").and_then(Json::as_str) else {
            errors.push(format!("Dynamic profile with Guid {guid} is missing the \u{201c}name\u{201d} field"));
            continue;
        };
        let name = crate::profile_list::display_text(name).trim().to_string();
        if name.is_empty() {
            errors.push(format!("Dynamic profile with Guid {guid} has an empty \u{201c}name\u{201d} field"));
            continue;
        }
        if saved_guids(guid) {
            errors.push(format!("Dynamic profile with Guid {guid} conflicts with non-dynamic profile with same Guid"));
            continue;
        }
        let mut entry = entry.as_object().cloned().unwrap_or_default();
        entry.insert("Name".into(), Json::String(name));
        kept.push(entry);
    }
    (Some((kept, kind)), errors)
}

/// The profile entries of a file as they stand (no entry dropped), and its type: what a
/// write-back edits. None when the file is not one the loader would read.
pub fn raw_profiles(bytes: &[u8]) -> Option<(Vec<Json>, FileType)> {
    if bytes.len() > MAX_FILE {
        return None;
    }
    let (root, kind) = read_root("", bytes).ok()?;
    match root.get("Profiles") {
        Some(Json::Array(list)) if list.len() <= MAX_PROFILES => Some((list.clone(), kind)),
        _ => None,
    }
}

// ---- Loading ---------------------------------------------------------------------------------

/// What `load` found: the profiles with the file each came from, and the errors with their file.
#[derive(Clone, Debug, Default)]
pub struct Loaded {
    pub profiles: Vec<(Entry, String)>,
    pub errors: Vec<(String, String)>,
}

/// The two judges a folder passes before anything in it is read: `local` is the one network
/// judge, `trusted` the owner and writers check (13v2's ruling: Command and the rest of a dynamic
/// profile are honored as iTerm2 honors them only from a folder and files the person owns and
/// others cannot write). `trusted` says why when it refuses.
pub struct Gate<'a> {
    pub local: &'a dyn Fn(&Path) -> bool,
    pub trusted: &'a dyn Fn(&Path) -> Result<(), String>,
}

/// The network judge as the app uses it: the path and every folder above it, no link leading
/// to a network path (`scripts::ancestors_stay_local`).
pub fn system_local(path: &Path) -> bool {
    crate::scripts::ancestors_stay_local(path, &crate::scripts::read_link)
}

/// The trust check as the app uses it (owner is the person, no one else may write).
#[cfg(windows)]
pub fn system_trusted(path: &Path) -> Result<(), String> {
    crate::dynamic_profiles_trust::check(path)
}

#[cfg(not(windows))]
pub fn system_trusted(_path: &Path) -> Result<(), String> {
    Ok(())
}

/// `load_gated` with the network judge and no trust check: for the pure tests only. The ruling
/// is that an ungated load must not exist for app code: it is compiled in tests alone, and a
/// source test (`app_code_never_loads_without_the_gate`) fails if a non-test caller appears. The
/// app loads through `load_gated` with `system_local` and `system_trusted`.
#[cfg(test)]
pub fn load(folder: &Path, read: &dyn Fn(&Path) -> Result<Vec<u8>, String>, list: &dyn Fn(&Path) -> Vec<String>, saved_guids: &dyn Fn(&str) -> bool) -> Loaded {
    load_gated(folder, read, list, saved_guids, &Gate { local: &|p: &Path| crate::scripts::stays_local(p), trusted: &|_: &Path| Ok(()) })
}

/// Every profile in the folder (`reallyReloadDynamicProfiles`, `DP:278-340`, `:458-481`). `list`
/// gives every entry below the folder, subfolders included, as "/" paths (a subfolder's own name
/// fails to read and is reported as iTerm2 reports it); `read` gives a file's bytes (at most
/// `MAX_FILE`: `bounded_read`). A Guid already taken by an earlier file is refused.
pub fn load_gated(
    folder: &Path,
    read: &dyn Fn(&Path) -> Result<Vec<u8>, String>,
    list: &dyn Fn(&Path) -> Vec<String>,
    saved_guids: &dyn Fn(&str) -> bool,
    gate: &Gate,
) -> Loaded {
    let mut got = Loaded::default();
    let folder_text = folder.display().to_string();
    if !(gate.local)(folder) {
        got.errors.push((folder_text.clone(), format!("Dynamic Profiles folder {folder_text} was not read: it is on a network path or reached through a link to one.")));
        return got;
    }
    if let Err(why) = (gate.trusted)(folder) {
        got.errors.push((folder_text.clone(), format!("Dynamic Profiles folder {folder_text} was not loaded: {why}.")));
        return got;
    }
    let all = list(folder);
    if all.len() > MAX_ENTRIES {
        got.errors.push((folder_text.clone(), format!("Dynamic Profiles folder {folder_text} holds more than {MAX_ENTRIES} entries; none were loaded.")));
        return got;
    }
    let mut guids: HashSet<String> = HashSet::new();
    for name in files(&all) {
        let full = folder.join(name.replace('/', std::path::MAIN_SEPARATOR_STR));
        let text = full.display().to_string();
        let fail = |got: &mut Loaded, message: String| got.errors.push((text.clone(), message));
        if name.split('/').count() > MAX_DEPTH {
            fail(&mut got, format!("Dynamic Profiles file {text} is skipped: it is more than {MAX_DEPTH} folders deep."));
            continue;
        }
        if name.split('/').any(crate::inert_files::is_device_name) {
            fail(&mut got, format!("Dynamic Profiles file {text} is skipped: its name is a device name."));
            continue;
        }
        if !(gate.local)(&full) {
            fail(&mut got, format!("Dynamic Profiles file {text} was not read: it is on a network path or reached through a link to one."));
            continue;
        }
        if let Err(why) = (gate.trusted)(&full) {
            fail(&mut got, format!("Dynamic Profiles file {text} was not loaded: {why}."));
            continue;
        }
        let bytes = match read(&full) {
            Ok(b) => b,
            Err(e) => {
                fail(&mut got, format!("Could not read Dynamic Profile from file {text}: {e}"));
                fail(&mut got, ignoring_text(&text));
                continue;
            }
        };
        let (parsed, errors) = parse_file(&text, &bytes, saved_guids);
        for e in errors {
            fail(&mut got, e);
        }
        let Some((entries, _)) = parsed else {
            fail(&mut got, ignoring_text(&text));
            continue;
        };
        if got.profiles.len() + entries.len() > MAX_PROFILES {
            fail(&mut got, format!("Dynamic Profiles file {text} was not loaded: Tandem loads at most {MAX_PROFILES} dynamic profiles."));
            continue;
        }
        for mut entry in entries {
            let guid = entry.get("Guid").and_then(Json::as_str).unwrap_or_default().to_string();
            if !guids.insert(guid.clone()) {
                fail(&mut got, format!("Two dynamic profiles have the same Guid: {guid}"));
                continue;
            }
            entry.insert(FILENAME_KEY.into(), Json::String(text.clone()));
            got.profiles.push((entry, text.clone()));
        }
    }
    got
}

fn ignoring_text(path: &str) -> String {
    format!("Ignoring dynamic profiles in \u{201c}{path}\u{201d} because of an error.")
}

// ---- Merging ---------------------------------------------------------------------------------

/// The profile a dynamic entry makes over its parent (`profileByMergingParentAndApplyingHacks:`,
/// `DP:516-593`, `:626-639`): the parent's values with the child's over them, `Default Bookmark`
/// from neither, the dynamic mark set, the tag added once with `add_tag`, and separate colors
/// off when the child gives plain colors only over a parent that splits them by mode. The
/// parent's own dynamic keys, filename and session link are not inherited. Everything goes
/// through `Profile::from_values` and `Profile::set`.
pub fn merge(child: &Entry, parent: &Profile, add_tag: bool) -> Profile {
    let mut p = Profile::from_values(parent.values.clone());
    for key in [DEFAULT_BOOKMARK_KEY, FILENAME_KEY, REWRITABLE_KEY, PARENT_NAME_KEY, PARENT_GUID_KEY, DYNAMIC_KEY, crate::session_profile::ORIGINAL_GUID] {
        p.remove(key);
    }
    let mut values: BTreeMap<String, Value> = child
        .iter()
        .filter(|(k, _)| k.as_str() != DEFAULT_BOOKMARK_KEY && k.as_str() != DYNAMIC_KEY)
        .filter_map(|(k, v)| Some((k.clone(), crate::import::from_json(v)?)))
        .collect();
    crate::start_dir::scrub_map(&mut values);
    for (k, v) in values {
        p.set(&k, v);
    }
    if separate_colors_off(child, parent) {
        p.set(SEPARATE_COLORS_KEY, Value::Boolean(false));
    }
    if add_tag {
        let mut tags: Vec<Value> = p.values.get("Tags").and_then(|t| t.as_array().cloned()).unwrap_or_default();
        if !tags.iter().any(|t| t.as_str() == Some(TAG)) {
            tags.push(Value::String(TAG.into()));
        }
        p.set("Tags", Value::Array(tags));
    }
    p.set(DYNAMIC_KEY, Value::Boolean(true));
    p
}

/// `shouldDisableSeparateColorsForDynamicProfile:prototype:` (`DP:535-584`).
fn separate_colors_off(child: &Entry, parent: &Profile) -> bool {
    if child.contains_key(SEPARATE_COLORS_KEY) || !parent.bool(SEPARATE_COLORS_KEY) {
        return false;
    }
    let colors = || crate::schema::profile().iter().filter(|s| s.kind == crate::schema::Kind::Color && s.light_dark);
    let plain = colors().any(|s| child.contains_key(&s.key));
    let modeful = colors().any(|s| child.contains_key(&format!("{} (Light)", s.key)) || child.contains_key(&format!("{} (Dark)", s.key)));
    plain && !modeful
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Parent {
    /// No parent named: the default profile.
    Default,
    /// The Guid of the parent found, by Guid first, then by name.
    Found(String),
    /// A parent was named and is not there: the default profile is used and this is reported.
    Missing(String),
}

/// The parent of a dynamic entry (`prototypeForDynamicProfile:`, `DP:594-624`).
pub fn parent_of(child: &Entry, guid_exists: &dyn Fn(&str) -> bool, guid_of_name: &dyn Fn(&str) -> Option<String>) -> Parent {
    let name = child.get(PARENT_NAME_KEY).and_then(Json::as_str);
    let guid = child.get(PARENT_GUID_KEY).and_then(Json::as_str);
    if name.is_none() && guid.is_none() {
        return Parent::Default;
    }
    if let Some(g) = guid.filter(|g| guid_exists(g)) {
        return Parent::Found(g.to_string());
    }
    if let Some(g) = name.and_then(guid_of_name) {
        return Parent::Found(g);
    }
    Parent::Missing(unknown_parent_text(child.get("Name").and_then(Json::as_str).unwrap_or_default(), guid, name))
}

/// The error for a parent that is not there (`DynamicProfiles.UnknownParentByGUID` and `...ByName`):
/// the Guid form when a Guid was named.
pub fn unknown_parent_text(name: &str, parent_guid: Option<&str>, parent_name: Option<&str>) -> String {
    let shown = |s: &str| crate::profile_list::display_text(s);
    match (parent_guid, parent_name) {
        (Some(g), _) => format!("Dynamic profile {} references unknown parent GUID {}. Using default profile as parent.", shown(name), shown(g)),
        (None, p) => format!("Dynamic profile {} references unknown parent name {}. Using default profile as parent.", shown(name), shown(p.unwrap_or_default())),
    }
}

// ---- Changes and rewriting -------------------------------------------------------------------

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Changes {
    pub add: Vec<String>,
    pub update: Vec<String>,
    pub remove: Vec<String>,
}

/// What a reload does: new Guids are added, known ones updated, dynamic profiles whose Guid is
/// no longer in a file removed (`DP:312-340`).
pub fn changes(old_dynamic: &[String], new: &[String]) -> Changes {
    let old: HashSet<&String> = old_dynamic.iter().collect();
    let now: HashSet<&String> = new.iter().collect();
    let mut c = Changes::default();
    for g in new {
        if old.contains(g) { c.update.push(g.clone()) } else { c.add.push(g.clone()) }
    }
    c.remove = old_dynamic.iter().filter(|g| !now.contains(g)).cloned().collect();
    c
}

/// A value as the file's JSON (`api_profile::json_of`: colors as dictionaries, triggers cleaned).
fn json_value(key: &str, v: &Value) -> Option<Json> {
    // A flag stays true or false in the file (`json_of` answers an API client with 1 and 0).
    if let Value::Boolean(b) = v {
        return Some(Json::Bool(*b));
    }
    serde_json::from_str(&crate::api_profile::json_of(key, v)?).ok()
}

/// The entry to write back for an edited profile (`profileForRewritingBasedOn:`, `DP:716-770`):
/// the file's own entry, kept as it is where the profile still matches it, with the keys the
/// parent (or a default) supplies left out, Guid, Name, the separate-colors key and Rewritable
/// always written, never the filename or the dynamic mark, and not the added tag.
pub fn rewritten(on_disk: &Entry, updated: &Profile, parent: &Profile, add_tag: bool) -> Entry {
    let mut out = on_disk.clone();
    let keys: BTreeSet<&String> = parent.values.keys().chain(updated.values.keys()).collect();
    for key in keys {
        if key == FILENAME_KEY {
            out.remove(key);
            continue;
        }
        let Some(now) = updated.values.get(key) else {
            out.remove(key);
            continue;
        };
        if matches!(key.as_str(), "Guid" | "Name" | SEPARATE_COLORS_KEY | REWRITABLE_KEY) {
            if let Some(j) = json_value(key, now) {
                out.insert(key.clone(), j);
            }
            continue;
        }
        if parent.get(key).as_ref() == Some(now) {
            out.remove(key);
            continue;
        }
        if on_disk.get(key).and_then(crate::import::from_json).as_ref() == Some(now) {
            continue;
        }
        match json_value(key, now) {
            Some(j) => {
                out.insert(key.clone(), j);
            }
            None => {
                out.remove(key);
            }
        }
    }
    out.remove(DYNAMIC_KEY);
    if add_tag {
        if let Some(Json::Array(tags)) = out.get_mut("Tags") {
            tags.retain(|t| t.as_str() != Some(TAG));
            if tags.is_empty() {
                out.remove("Tags");
            }
        }
    }
    out
}

/// A file's bytes for `entries`: JSON pretty-printed, or a property list (`DP:790-828`).
pub fn write_file(entries: Vec<Entry>, file_type: FileType) -> Vec<u8> {
    let root = Json::Object(std::iter::once(("Profiles".to_string(), Json::Array(entries.into_iter().map(Json::Object).collect()))).collect());
    match file_type {
        FileType::Json => serde_json::to_vec_pretty(&root).unwrap_or_default(),
        FileType::PropertyList => {
            let mut out = Vec::new();
            if let Some(v) = json_to_plist(&root) {
                let _ = plist::to_writer_xml(&mut out, &v);
            }
            out
        }
    }
}

// ---- Texts -----------------------------------------------------------------------------------

/// The alert for a burst of errors: the first, and how many more are in the log (`DP:167-222`).
pub fn alert_text(errors: &[String]) -> String {
    let first = errors.first().map(String::as_str).unwrap_or_default();
    let mut text = format!("There was a problem with one of your Dynamic Profiles:\n\n{first}");
    if errors.len() > 1 {
        text.push_str(&format!("\n\n{} additional errors may be seen in the log.", errors.len() - 1));
    }
    text
}

#[cfg(test)]
mod tests {
    /// The ruling "an ungated load must not exist": no source file outside its tests calls the
    /// ungated `load`, and the function itself is test-only.
    #[test]
    fn app_code_never_loads_without_the_gate() {
        let here = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = vec![here.clone()];
        let mut seen = Vec::new();
        while let Some(dir) = files.pop() {
            for e in std::fs::read_dir(&dir).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    files.push(p);
                } else if p.extension().is_some_and(|x| x == "rs") {
                    seen.push(p);
                }
            }
        }
        let mut wrong = Vec::new();
        for p in seen {
            let text = std::fs::read_to_string(&p).unwrap();
            // Everything after the first test module is test code (the repo's layout).
            let body = text.split("#[cfg(test)]").next().unwrap_or("");
            for needle in ["dynamic_profiles::load(", "dp::load(", "dynamic_profiles::load,"] {
                if body.contains(needle) {
                    wrong.push(format!("{} calls the ungated {needle}", p.display()));
                }
            }
        }
        assert!(wrong.is_empty(), "{}", wrong.join("\n"));
        let own = include_str!("dynamic_profiles.rs");
        let at = own.find("pub fn load(").expect("load");
        assert!(own[..at].trim_end().ends_with("#[cfg(test)]"), "the ungated load is not test-only");
    }

    use super::*;
    use crate::profile::Profile;
    use serde_json::json;
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};

    type Map = serde_json::Map<String, serde_json::Value>;

    fn obj(v: serde_json::Value) -> Map {
        v.as_object().expect("an object").clone()
    }

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    fn profile(pairs: &[(&str, toml::Value)]) -> Profile {
        Profile::from_values(pairs.iter().map(|(k, v)| (k.to_string(), v.clone())).collect::<BTreeMap<_, _>>())
    }

    fn s(text: &str) -> toml::Value {
        toml::Value::String(text.into())
    }

    fn none_saved(_: &str) -> bool {
        false
    }

    fn file_of(profiles: &[(&str, &str)]) -> Vec<u8> {
        let list: Vec<_> = profiles.iter().map(|(g, n)| json!({"Guid": g, "Name": n})).collect();
        serde_json::to_vec(&json!({"Profiles": list})).unwrap()
    }

    #[test]
    fn dynamic_files_follow_iterm2() {
        assert_eq!(files(&names(&["b.json", ".hidden", "a.json", "c.json~", "Z.plist"])), names(&["Z.plist", "a.json", "b.json"]));
        assert_eq!(files(&names(&["sub/b.json", ".git/config", "sub", "a.json"])), names(&["a.json", "sub", "sub/b.json"]));

        let folder = PathBuf::from("C:\\dp");
        let list = |_: &Path| names(&["sub", "sub/b.json"]);
        let sub = folder.join("sub");
        let read = |p: &Path| if p == sub { Err("Is a directory".to_string()) } else { Ok(file_of(&[("B", "b")])) };
        let got = load(&folder, &read, &list, &none_saved);
        assert_eq!(got.profiles.len(), 1);
        assert_eq!(got.profiles[0].1, folder.join("sub").join("b.json").display().to_string());
        let ignoring = format!("Ignoring dynamic profiles in \u{201c}{}\u{201d} because of an error.", sub.display());
        assert!(got.errors.iter().any(|(f, e)| e == &ignoring && f == &sub.display().to_string()), "{:?}", got.errors);

        let appdata = Path::new("C:\\Users\\me\\AppData\\Roaming");
        let home = Path::new("C:\\Users\\me");
        assert_eq!(folder_of("", appdata, home), appdata.join("Tandem").join("DynamicProfiles"));
        assert_eq!(folder_of("~\\dp", appdata, home), home.join("dp"));
        assert_eq!(folder_of("~", appdata, home), home);
        assert_eq!(folder_of("D:\\profiles", appdata, home), PathBuf::from("D:\\profiles"));
    }

    fn folder_of(custom: &str, appdata: &Path, home: &Path) -> PathBuf {
        folder(custom, appdata, home)
    }

    #[test]
    fn dynamic_merge_follows_iterm2() {
        let parent = profile(&[
            ("Guid", s("P")),
            ("Name", s("Parent")),
            ("Font", s("Cascadia Mono 10")),
            ("Columns", toml::Value::Integer(100)),
            ("Default Bookmark", s("Yes")),
        ]);
        let child = obj(json!({"Guid": "D1", "Name": "Dev", "Font": "Consolas 12", "Default Bookmark": "Yes"}));
        let merged = merge(&child, &parent, false);
        assert_eq!(merged.values.get("Font"), Some(&s("Consolas 12")), "the child's key wins");
        assert_eq!(merged.values.get("Columns"), Some(&toml::Value::Integer(100)), "the parent supplies the rest");
        assert!(merged.values.get("Default Bookmark").is_none(), "never inherited, never taken from the child");
        assert_eq!(merged.guid(), "D1");
        assert_eq!(merged.values.get(DYNAMIC_KEY), Some(&toml::Value::Boolean(true)));
        assert!(merged.values.get("Tags").is_none());

        let tagged = merge(&child, &parent, true);
        assert_eq!(tagged.values.get("Tags"), Some(&toml::Value::Array(vec![s(TAG)])));
        let with_tag = obj(json!({"Guid": "D1", "Name": "Dev", "Tags": ["x"]}));
        assert_eq!(merge(&with_tag, &parent, true).values.get("Tags"), Some(&toml::Value::Array(vec![s("x"), s(TAG)])));
        let twice = obj(json!({"Guid": "D1", "Name": "Dev", "Tags": ["x", "Dynamic"]}));
        assert_eq!(merge(&twice, &parent, true).values.get("Tags"), Some(&toml::Value::Array(vec![s("x"), s(TAG)])), "once");

        // The dynamic keys of a dynamic parent are not the child's.
        let dyn_parent = profile(&[("Guid", s("P2")), (REWRITABLE_KEY, toml::Value::Boolean(true)), (FILENAME_KEY, s("other.json"))]);
        let inherited = merge(&child, &dyn_parent, false);
        assert!(inherited.values.get(REWRITABLE_KEY).is_none() && inherited.values.get(FILENAME_KEY).is_none());

        // Separate colors: plain color keys over a parent that splits colors by mode.
        let splitting = profile(&[("Guid", s("P")), (SEPARATE_COLORS_KEY, toml::Value::Boolean(true))]);
        let red = json!({"Red Component": 1.0, "Green Component": 0.0, "Blue Component": 0.0, "Alpha Component": 1.0, "Color Space": "sRGB"});
        let plain = obj(json!({"Guid": "D2", "Name": "Red", "Foreground Color": red}));
        assert_eq!(merge(&plain, &splitting, false).values.get(SEPARATE_COLORS_KEY), Some(&toml::Value::Boolean(false)));
        assert!(merge(&plain, &splitting, false).values.get("Foreground Color").is_some_and(|c| c.is_table()), "colors come through import's converter");
        let modeful = obj(json!({"Guid": "D3", "Name": "Red", "Foreground Color": red, "Foreground Color (Dark)": red}));
        assert_eq!(merge(&modeful, &splitting, false).values.get(SEPARATE_COLORS_KEY), Some(&toml::Value::Boolean(true)));
        let explicit = obj(json!({"Guid": "D4", "Name": "Red", "Foreground Color": red, "Use Separate Colors for Light and Dark Mode": true}));
        assert_eq!(merge(&explicit, &splitting, false).values.get(SEPARATE_COLORS_KEY), Some(&toml::Value::Boolean(true)));
        let uncolored = obj(json!({"Guid": "D5", "Name": "Plain"}));
        assert_eq!(merge(&uncolored, &splitting, false).values.get(SEPARATE_COLORS_KEY), Some(&toml::Value::Boolean(true)));
        let flat = profile(&[("Guid", s("P"))]);
        assert!(merge(&plain, &flat, false).values.get(SEPARATE_COLORS_KEY).is_none(), "a parent that does not split leaves it alone");

        // Parents.
        let exists = |g: &str| g == "P1";
        let by_name = |n: &str| (n == "Base").then(|| "P2".to_string());
        let with_guid = obj(json!({"Guid": "D", "Name": "n", "Dynamic Profile Parent GUID": "P1"}));
        assert_eq!(parent_of(&with_guid, &exists, &by_name), Parent::Found("P1".into()));
        let guid_then_name = obj(json!({"Guid": "D", "Name": "n", "Dynamic Profile Parent GUID": "nope", "Dynamic Profile Parent Name": "Base"}));
        assert_eq!(parent_of(&guid_then_name, &exists, &by_name), Parent::Found("P2".into()));
        let missing = obj(json!({"Guid": "D", "Name": "Dev", "Dynamic Profile Parent GUID": "nope", "Dynamic Profile Parent Name": "nobody"}));
        assert_eq!(parent_of(&missing, &exists, &by_name), Parent::Missing(unknown_parent_text("Dev", Some("nope"), Some("nobody"))));
        let by_name_only = obj(json!({"Guid": "D", "Name": "Dev", "Dynamic Profile Parent Name": "nobody"}));
        assert_eq!(parent_of(&by_name_only, &exists, &by_name), Parent::Missing(unknown_parent_text("Dev", None, Some("nobody"))));
        assert_eq!(parent_of(&obj(json!({"Guid": "D", "Name": "n"})), &exists, &by_name), Parent::Default);
        assert_eq!(
            unknown_parent_text("Dev", Some("G"), None),
            "Dynamic profile Dev references unknown parent GUID G. Using default profile as parent."
        );
        assert_eq!(
            unknown_parent_text("Dev", None, Some("Base")),
            "Dynamic profile Dev references unknown parent name Base. Using default profile as parent."
        );
    }

    #[test]
    fn dynamic_errors_follow_iterm2() {
        let f = "C:\\dp\\a.json";
        let saved = |g: &str| g == "S";
        let text = r#"{"Profiles": [{"Guid": "A", "Name": "a"}, {"Name": "no guid"}, {"Guid": "B"}, {"Guid": "S", "Name": "clash"}]}"#;
        let (got, errors) = parse_file(f, text.as_bytes(), &saved);
        let (entries, kind) = got.expect("a file with some good entries");
        assert_eq!(kind, FileType::Json);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0]["Guid"], "A", "a dynamic profile never replaces a profile of the person's own");
        assert_eq!(
            errors,
            vec![
                format!("Dynamic profile is missing the Guid field in file {f}"),
                "Dynamic profile with Guid B is missing the \u{201c}name\u{201d} field".to_string(),
                "Dynamic profile with Guid S conflicts with non-dynamic profile with same Guid".to_string(),
            ]
        );

        let (got, errors) = parse_file(f, b"[1]", &none_saved);
        assert!(got.is_none());
        assert_eq!(errors, vec![format!("Dynamic Profiles file {f} does not have an Object (i.e., a dictionary) as its root element")]);
        let (got, errors) = parse_file(f, b"{", &none_saved);
        assert!(got.is_none());
        assert!(errors[0].starts_with(&format!("Dynamic Profiles file {f} contains invalid JSON: ")), "{errors:?}");
        let (got, errors) = parse_file(f, b"{}", &none_saved);
        assert!(got.is_none());
        assert_eq!(errors, vec![format!("Dynamic Profiles file {f} does not have a \u{201c}Profiles\u{201d} key at the root.")]);

        // A property list (binary or XML) with the same content.
        let mut d = plist::Dictionary::new();
        let mut p = plist::Dictionary::new();
        p.insert("Guid".into(), plist::Value::String("A".into()));
        p.insert("Name".into(), plist::Value::String("a".into()));
        p.insert("Columns".into(), plist::Value::Integer(90.into()));
        d.insert("Profiles".into(), plist::Value::Array(vec![plist::Value::Dictionary(p)]));
        let root = plist::Value::Dictionary(d);
        let mut binary = Vec::new();
        plist::to_writer_binary(&mut binary, &root).unwrap();
        let mut xml = Vec::new();
        plist::to_writer_xml(&mut xml, &root).unwrap();
        for bytes in [&binary, &xml] {
            let (got, errors) = parse_file("a.plist", bytes, &none_saved);
            assert!(errors.is_empty(), "{errors:?}");
            let (entries, kind) = got.unwrap();
            assert_eq!(kind, FileType::PropertyList);
            assert_eq!((entries[0]["Guid"].as_str(), entries[0]["Columns"].as_i64()), (Some("A"), Some(90)));
        }

        // Two files, one Guid: the first wins.
        let folder = PathBuf::from("C:\\dp");
        let list = |_: &Path| names(&["a.json", "b.json"]);
        let read = |_: &Path| Ok(file_of(&[("A", "a")]));
        let got = load(&folder, &read, &list, &none_saved);
        assert_eq!(got.profiles.len(), 1);
        assert_eq!(got.profiles[0].1, folder.join("a.json").display().to_string());
        assert!(got.errors.iter().any(|(file, e)| e == "Two dynamic profiles have the same Guid: A" && file.ends_with("b.json")), "{:?}", got.errors);

        // A file that failed names itself.
        let list = |_: &Path| names(&["bad.json"]);
        let read = |_: &Path| Ok(b"{".to_vec());
        let got = load(&folder, &read, &list, &none_saved);
        let path = folder.join("bad.json").display().to_string();
        assert!(got.profiles.is_empty());
        assert!(got.errors.iter().any(|(_, e)| e == &format!("Ignoring dynamic profiles in \u{201c}{path}\u{201d} because of an error.")));
        let read = |_: &Path| Err("access denied".to_string());
        let got = load(&folder, &read, &list, &none_saved);
        assert!(got.errors.iter().any(|(_, e)| e == &format!("Could not read Dynamic Profile from file {path}: access denied")), "{:?}", got.errors);

        // A saved profile's Guid is refused across load too.
        let list = |_: &Path| names(&["a.json"]);
        let read = |_: &Path| Ok(file_of(&[("S", "clash"), ("T", "fine")]));
        let got = load(&folder, &read, &list, &saved);
        assert_eq!(got.profiles.iter().map(|(m, _)| m["Guid"].as_str().unwrap()).collect::<Vec<_>>(), vec!["T"]);

        assert_eq!(changes(&names(&["A", "B"]), &names(&["B", "C"])), Changes { add: names(&["C"]), update: names(&["B"]), remove: names(&["A"]) });

        assert_eq!(alert_text(&names(&["one"])), "There was a problem with one of your Dynamic Profiles:\n\none");
        assert_eq!(
            alert_text(&names(&["one", "two", "three"])),
            "There was a problem with one of your Dynamic Profiles:\n\none\n\n2 additional errors may be seen in the log."
        );
        assert_eq!(
            (ALERT_HEADING, VIEW_LOG, REVEAL),
            ("Dynamic Profiles Error", "View Log", "Reveal in Explorer")
        );
    }

    #[test]
    fn rewrite_follows_iterm2() {
        let on_disk = obj(json!({"Guid": "D", "Name": "Dev", "Rewritable": true, "Columns": 90}));
        let parent = profile(&[("Guid", s("P")), ("Font", s("Consolas 12")), ("Columns", toml::Value::Integer(80))]);
        let updated = profile(&[
            ("Guid", s("D")),
            ("Name", s("Dev")),
            (REWRITABLE_KEY, toml::Value::Boolean(true)),
            ("Columns", toml::Value::Integer(120)),
            ("Font", s("Consolas 12")),
            ("Tags", toml::Value::Array(vec![s(TAG)])),
            (FILENAME_KEY, s("C:\\dp\\a.json")),
            (DYNAMIC_KEY, toml::Value::Boolean(true)),
        ]);
        let out = rewritten(&on_disk, &updated, &parent, true);
        assert_eq!(out["Guid"], "D");
        assert_eq!(out["Name"], "Dev");
        assert_eq!(out["Rewritable"], true);
        assert_eq!(out["Columns"], 120);
        for gone in ["Font", FILENAME_KEY, DYNAMIC_KEY, "Tags"] {
            assert!(!out.contains_key(gone), "{gone}");
        }
        // Without the added tag, tags stay; the other tags stay either way.
        let with_x = profile(&[("Guid", s("D")), ("Name", s("Dev")), (REWRITABLE_KEY, toml::Value::Boolean(true)), ("Tags", toml::Value::Array(vec![s("x"), s(TAG)]))]);
        assert_eq!(rewritten(&on_disk, &with_x, &parent, true)["Tags"], json!(["x"]));
        assert_eq!(rewritten(&on_disk, &with_x, &parent, false)["Tags"], json!(["x", "Dynamic"]));
        // A value that matches the file keeps its on-disk form; a key removed from the profile
        // leaves the file.
        let same = profile(&[("Guid", s("D")), ("Name", s("Dev")), (REWRITABLE_KEY, toml::Value::Boolean(true)), ("Columns", toml::Value::Integer(90))]);
        assert_eq!(rewritten(&on_disk, &same, &parent, false)["Columns"], 90);
        let removed = profile(&[("Guid", s("D")), ("Name", s("Dev")), (REWRITABLE_KEY, toml::Value::Boolean(true))]);
        assert!(!rewritten(&on_disk, &removed, &parent, false).contains_key("Columns"));

        let bytes = write_file(vec![out.clone()], FileType::Json);
        let text = String::from_utf8(bytes.clone()).unwrap();
        assert!(text.contains("\n  \"Profiles\": [\n"), "pretty-printed: {text}");
        let (got, errors) = parse_file("a.json", &bytes, &none_saved);
        assert!(errors.is_empty());
        assert_eq!(got.unwrap().0, vec![out.clone()]);
        let plist = write_file(vec![out.clone()], FileType::PropertyList);
        let (got, errors) = parse_file("a.plist", &plist, &none_saved);
        assert!(errors.is_empty(), "{errors:?}");
        let (entries, kind) = got.unwrap();
        assert_eq!((kind, entries[0]["Columns"].as_i64(), entries[0]["Guid"].as_str()), (FileType::PropertyList, Some(120), Some("D")));
    }

    #[test]
    fn dynamic_limits_hold() {
        let big = vec![b' '; 2 << 20];
        let (got, errors) = parse_file("a.json", &big, &none_saved);
        assert!(got.is_none() && errors.len() == 1 && errors[0].contains("too large"), "{errors:?}");

        let many: Vec<_> = (0..300).map(|i| json!({"Guid": format!("G{i}"), "Name": "n"})).collect();
        let (got, errors) = parse_file("a.json", &serde_json::to_vec(&json!({"Profiles": many})).unwrap(), &none_saved);
        assert!(got.is_none() && !errors.is_empty(), "300 profiles in one file");

        let deep = format!(r#"{{"Profiles": [{{"Guid": "A", "Name": "a", "X": {}{}}}]}}"#, "[".repeat(40), "]".repeat(40));
        let (got, errors) = parse_file("a.json", deep.as_bytes(), &none_saved);
        assert!(got.is_none() && !errors.is_empty(), "40 levels deep");
        let deeper = format!("{}{}", "[".repeat(100_000), "]".repeat(100_000));
        let (got, errors) = parse_file("a.json", deeper.as_bytes(), &none_saved);
        assert!(got.is_none() && !errors.is_empty(), "never recursed into");

        let long = format!(r#"{{"Profiles": [{{"Guid": "A", "Name": "a", "X": "{}"}}]}}"#, "x".repeat(70 * 1024));
        let (got, errors) = parse_file("a.json", long.as_bytes(), &none_saved);
        assert!(got.is_none() && !errors.is_empty(), "a 70 KiB string");

        let keys: Map = (0..600).map(|i| (format!("K{i}"), json!(i))).chain([("Guid".to_string(), json!("A")), ("Name".to_string(), json!("a"))]).collect();
        let (got, errors) = parse_file("a.json", &serde_json::to_vec(&json!({"Profiles": [keys]})).unwrap(), &none_saved);
        assert!(got.is_none() && !errors.is_empty(), "600 keys");

        // 600 folder entries: nothing loads.
        let folder = PathBuf::from("C:\\dp");
        let list = |_: &Path| (0..600).map(|i| format!("f{i}.json")).collect::<Vec<_>>();
        let read = |_: &Path| Ok(file_of(&[("A", "a")]));
        let got = load(&folder, &read, &list, &none_saved);
        assert!(got.profiles.is_empty() && !got.errors.is_empty());
        // Too deep a path is skipped with an error.
        let deep_name = format!("{}x.json", "d/".repeat(9));
        let list = |_: &Path| vec![deep_name.clone()];
        let got = load(&folder, &read, &list, &none_saved);
        assert!(got.profiles.is_empty() && !got.errors.is_empty());

        // 256 profiles across all files: the file that would pass it is refused whole.
        let list = |_: &Path| names(&["a.json", "b.json", "c.json"]);
        let read = |p: &Path| {
            let which = p.file_name().unwrap().to_string_lossy().chars().next().unwrap();
            let list: Vec<_> = (0..100).map(|i| json!({"Guid": format!("{which}{i}"), "Name": "n"})).collect();
            Ok(serde_json::to_vec(&json!({"Profiles": list})).unwrap())
        };
        let got = load(&folder, &read, &list, &none_saved);
        assert_eq!(got.profiles.len(), 200);
        assert!(got.errors.iter().any(|(f, _)| f.ends_with("c.json")));

        // A file longer than the limit never reaches the parser through a read either.
        let list = |_: &Path| names(&["a.json"]);
        let read = |_: &Path| Ok(vec![b' '; 2 << 20]);
        assert!(load(&folder, &read, &list, &none_saved).profiles.is_empty());
    }

    /// A binary property list with `objects` objects: a chain of arrays of two references to the
    /// next one, ending in an integer.
    fn bplist(chain: usize) -> Vec<u8> {
        let mut out = b"bplist00".to_vec();
        let mut offsets = Vec::new();
        for i in 0..chain {
            offsets.push(out.len() as u8);
            out.extend([0xA2, (i + 1) as u8, (i + 1) as u8]);
        }
        offsets.push(out.len() as u8);
        out.extend([0x10, 0x01]);
        let table = out.len() as u64;
        out.extend(&offsets);
        let mut trailer = vec![0u8; 5];
        trailer.extend([0, 1, 1]);
        trailer.extend(((chain + 1) as u64).to_be_bytes());
        trailer.extend(0u64.to_be_bytes());
        trailer.extend(table.to_be_bytes());
        out.extend(trailer);
        out
    }

    #[test]
    fn dynamic_plist_counts_are_bounded() {
        // The header claims 2^32 objects in a 100 byte file.
        let mut lie = b"bplist00".to_vec();
        lie.extend([0u8; 60]);
        let mut trailer = vec![0u8; 5];
        trailer.extend([0, 1, 1]);
        trailer.extend((1u64 << 32).to_be_bytes());
        trailer.extend(0u64.to_be_bytes());
        trailer.extend(8u64.to_be_bytes());
        lie.extend(trailer);
        let (got, errors) = parse_file("a.plist", &lie, &none_saved);
        assert!(got.is_none() && !errors.is_empty(), "{errors:?}");
        // 30 levels of "two references to the next" is 2^30 values once expanded.
        let (got, errors) = parse_file("a.plist", &bplist(30), &none_saved);
        assert!(got.is_none() && !errors.is_empty(), "{errors:?}");
        // Deeper than the limit, small as it is.
        let (got, errors) = parse_file("a.plist", &bplist(40), &none_saved);
        assert!(got.is_none() && !errors.is_empty(), "{errors:?}");
        // XML nested without end.
        let xml = format!("<?xml version=\"1.0\"?><plist>{}{}</plist>", "<array>".repeat(100_000), "</array>".repeat(100_000));
        let (got, errors) = parse_file("a.plist", xml.as_bytes(), &none_saved);
        assert!(got.is_none() && !errors.is_empty(), "{errors:?}");
    }

    #[test]
    fn dynamic_guid_and_name_are_sanitized() {
        let long = "G".repeat(129);
        let text = json!({"Profiles": [
            {"Guid": "../x", "Name": "a"},
            {"Guid": "has space", "Name": "a"},
            {"Guid": long, "Name": "a"},
            {"Guid": "", "Name": "a"},
            {"Guid": "{AB-12_3.x}", "Name": "\u{202e}evil\n\u{200b}name"},
            {"Guid": "OK2", "Name": "\n\u{202e}"},
            {"Guid": "OK3", "Name": "x".repeat(300)},
        ]});
        let (got, errors) = parse_file("a.json", &serde_json::to_vec(&text).unwrap(), &none_saved);
        let entries = got.unwrap().0;
        assert_eq!(entries.iter().map(|m| m["Guid"].as_str().unwrap()).collect::<Vec<_>>(), vec!["{AB-12_3.x}", "OK3"]);
        assert_eq!(entries[0]["Name"], "evilname");
        assert_eq!(entries[1]["Name"].as_str().unwrap().chars().count(), 128);
        assert_eq!(errors.len(), 5, "{errors:?}");
    }

    #[test]
    fn dynamic_profile_goes_through_the_one_write_point() {
        let parent = profile(&[("Guid", s("P"))]);
        let tags: Vec<_> = (0..100).map(|i| format!("t{i}")).collect();
        let child = obj(json!({
            "Guid": "D",
            "Name": "Dev",
            "Tags": tags,
            "Bindings": {"Name": "x", "Foreground Color": "bgColor", "Not A Setting": "y"},
        }));
        let merged = merge(&child, &parent, true);
        let got = merged.values.get("Tags").and_then(|t| t.as_array()).unwrap();
        assert_eq!(got.len(), crate::profile_list::MAX_TAGS, "Profile::set caps tags");
        let bindings = merged.values.get("Bindings").and_then(|b| b.as_table()).unwrap();
        assert_eq!(bindings.keys().collect::<Vec<_>>(), vec!["Foreground Color"], "Profile::set cleans bindings");
        // A network start folder is scrubbed as an import's is.
        let net = obj(json!({"Guid": "D", "Name": "Dev", "Custom Directory": "Yes", "Working Directory": "\\\\host\\share"}));
        assert_ne!(merge(&net, &parent, false).values.get("Working Directory"), Some(&s("\\\\host\\share")));
    }

    #[test]
    fn dynamic_folder_uses_the_one_network_judge() {
        let folder = PathBuf::from("C:\\dp");
        let list = |_: &Path| names(&["CON.json", "a.json", "link.json", "sub/b.json"]);
        let read = |_: &Path| Ok(file_of(&[("A", "a")]));
        // The judge is the scripts one: a junction or link that reaches a share is not read.
        let remote = |p: &Path| !p.ends_with("link.json");
        let gate = Gate { local: &remote, trusted: &|_: &Path| Ok(()) };
        let got = load_gated(&folder, &read, &list, &none_saved, &gate);
        let loaded: Vec<_> = got.profiles.iter().map(|(_, f)| Path::new(f).file_name().unwrap().to_string_lossy().to_string()).collect();
        assert_eq!(loaded, vec!["a.json"], "CON.json is a device name, link.json is not local, sub/b.json repeats Guid A: {:?}", got.errors);
        assert!(got.errors.iter().any(|(f, _)| f.ends_with("CON.json")));
        assert!(got.errors.iter().any(|(f, _)| f.ends_with("link.json")));
        // A folder that is not local loads nothing and says why.
        let nowhere = |_: &Path| false;
        let gate = Gate { local: &nowhere, trusted: &|_: &Path| Ok(()) };
        let got = load_gated(&folder, &read, &list, &none_saved, &gate);
        assert!(got.profiles.is_empty() && got.errors.len() == 1, "{:?}", got.errors);
        // The real judge: a UNC folder and a link chain.
        assert!(!crate::scripts::stays_local(Path::new("\\\\host\\share\\dp")));
        assert!(crate::scripts::stays_local(Path::new("C:\\dp\\a.json")));
    }

    #[test]
    fn dynamic_trust_is_a_gate_on_the_folder_and_each_file() {
        let folder = PathBuf::from("C:\\dp");
        let list = |_: &Path| names(&["mine.json", "theirs.json"]);
        let read = |p: &Path| Ok(file_of(&[(if p.ends_with("mine.json") { "A" } else { "B" }, "n")]));
        let local = |_: &Path| true;
        let trusted = |p: &Path| if p.ends_with("theirs.json") { Err("owned by another user".to_string()) } else { Ok(()) };
        let got = load_gated(&folder, &read, &list, &none_saved, &Gate { local: &local, trusted: &trusted });
        assert_eq!(got.profiles.len(), 1, "an untrusted file loads nothing");
        assert!(got.errors.iter().any(|(f, e)| f.ends_with("theirs.json") && e.contains("not loaded") && e.contains("owned by another user")), "{:?}", got.errors);
        // An untrusted folder loads no file at all.
        let strict = |p: &Path| if p == folder { Err("writable by other users".to_string()) } else { Ok(()) };
        let got = load_gated(&folder, &read, &list, &none_saved, &Gate { local: &local, trusted: &strict });
        assert!(got.profiles.is_empty() && got.errors.len() == 1, "{:?}", got.errors);
    }

    #[test]
    fn dynamic_parse_never_panics() {
        let mut corpus: Vec<Vec<u8>> = vec![
            vec![],
            b"\xef\xbb\xbf".to_vec(),
            b"\xef\xbb\xbf{\"Profiles\": []}".to_vec(),
            b"{\"Profiles\": 5}".to_vec(),
            b"{\"Profiles\": [1, null, \"x\", [], {}]}".to_vec(),
            b"{\"Profiles\": [{\"Guid\": 1, \"Name\": 2}]}".to_vec(),
            b"{\"Profiles\": {\"a\": 1}}".to_vec(),
            b"bplist00".to_vec(),
            b"bplist00\x00".to_vec(),
            b"<?xml version=\"1.0\"?><plist><dict><key>Profiles</key>".to_vec(),
            b"<plist><array/></plist>".to_vec(),
            vec![0xff; 4000],
            bplist(3),
        ];
        let good = file_of(&[("A", "a")]);
        for cut in 0..good.len() {
            corpus.push(good[..cut].to_vec());
        }
        let mut xml = Vec::new();
        let mut d = plist::Dictionary::new();
        d.insert("Profiles".into(), plist::Value::Array(vec![plist::Value::Dictionary(plist::Dictionary::new())]));
        plist::to_writer_xml(&mut xml, &plist::Value::Dictionary(d.clone())).unwrap();
        let mut binary = Vec::new();
        plist::to_writer_binary(&mut binary, &plist::Value::Dictionary(d)).unwrap();
        for cut in 0..xml.len() {
            corpus.push(xml[..cut].to_vec());
        }
        for cut in 0..binary.len() {
            corpus.push(binary[..cut].to_vec());
        }
        for (i, bytes) in corpus.iter().enumerate() {
            let (got, errors) = parse_file("a.json", bytes, &none_saved);
            if got.is_none() {
                assert!(!errors.is_empty(), "case {i}: a refusal always says why");
            }
        }
    }

    #[test]
    fn dynamic_listing_walks_the_folder_and_never_follows_a_link() {
        let root = std::env::temp_dir().join(format!("tandem-dp-list-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("sub").join("deeper")).unwrap();
        std::fs::write(root.join("a.json"), b"{}").unwrap();
        std::fs::write(root.join("sub").join("b.json"), b"{}").unwrap();
        std::fs::write(root.join("sub").join("deeper").join("c.json"), b"{}").unwrap();
        let all = |_: &Path| true;
        let mut got = list_dir(&root, &all, 100);
        got.sort();
        assert_eq!(got, names(&["a.json", "sub", "sub/b.json", "sub/deeper", "sub/deeper/c.json"]));
        // An entry the judge refuses is not listed, and a folder it refuses is not entered.
        let no_sub = |p: &Path| !p.ends_with("sub");
        let mut got = list_dir(&root, &no_sub, 100);
        got.sort();
        assert_eq!(got, names(&["a.json"]));
        // The cap: one past it is enough to refuse the folder.
        assert_eq!(list_dir(&root, &all, 2).len(), 3);
        let file = root.join("a.json");
        assert_eq!(bounded_read(&file, 1024).unwrap(), b"{}".to_vec());
        assert!(bounded_read(&file, 1).unwrap_err().contains("too large"));
        assert!(bounded_read(&root, 1024).is_err(), "a folder is no file");
        let _ = std::fs::remove_dir_all(&root);
    }
}
