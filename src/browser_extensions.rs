//! Browser extensions (milestone 13x8, Task 10). Pure rules: where the extension folders live, which
//! of them WebView2 is told about, the manifest summary the person approves and the settings page
//! shows, and the approvals Tandem keeps. A manifest is untrusted input: it is parsed in this process
//! with a size cap and a depth cap, every text goes through `inert_files::display_text` and is cut,
//! and nothing here reads a file except `read_folder_manifest` (bounded, regular files only).
//! Loading is never automatic: only a folder the person chose, whose manifest hash the person
//! approved in a native window for this profile, is ever added (`Approvals`).

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

pub const ROOT_KEY: &str = "Browser Extensions Root";
pub const ACTIVE_KEY: &str = "Browser Extension Active IDs";

/// The largest `manifest.json` read (bytes).
pub const MAX_MANIFEST_BYTES: usize = 256 * 1024;
/// The deepest JSON nesting a manifest may have.
pub const MAX_DEPTH: usize = 16;
/// Extension folders considered under one root.
pub const MAX_FOLDERS: usize = 64;
pub const MAX_NAME: usize = 100;
pub const MAX_DESCRIPTION: usize = 300;
pub const MAX_VERSION: usize = 32;
/// Permission strings kept per list, and the length of each.
pub const MAX_ENTRIES: usize = 64;
pub const MAX_ENTRY: usize = 128;
/// The approvals file: bytes read and records kept.
pub const MAX_APPROVALS_BYTES: usize = 256 * 1024;
pub const MAX_APPROVALS: usize = 256;

pub const REVEALED: &str = "Extensions directory revealed in Explorer";
pub const ENABLED: &str = "Extension enabled";
pub const DISABLED: &str = "Extension disabled";
pub const NOT_AVAILABLE: &str = "Extension management not available";
pub const CHOSEN: &str = "Extensions folder chosen";
pub const NOT_ALLOWED: &str = "Extensions are not available here";
pub const APPROVAL_HEADING: &str = "Enable Browser Extension?";
pub const APPROVAL_BUTTON: &str = "Enable";

pub fn create_failed(e: &str) -> String {
    format!("Failed to create extensions directory: {}", crate::inert_files::display_text(e, 200, false))
}

/// The extensions root: the profile's value, else `<APPDATA>\Tandem\BrowserExtensions` (`IPP:1656-1666`).
pub fn root(profile_value: Option<&str>, appdata: &Path) -> PathBuf {
    match profile_value.map(str::trim).filter(|v| !v.is_empty()) {
        Some(v) => PathBuf::from(v),
        None => appdata.join("Tandem").join("BrowserExtensions"),
    }
}

/// The directory names under the root, every one (`EM:171-190`), at most `MAX_FOLDERS`. `names` holds
/// (name, is a directory).
pub fn folders(names: &[(String, bool)]) -> Vec<String> {
    names.iter().filter(|(_, dir)| *dir).map(|(n, _)| n.clone()).take(MAX_FOLDERS).collect()
}

/// Whether a root may be touched at all: a drive path only. A network share, `\\?\`, `\\.\` and a
/// relative path are refused before anything reads them (review I8).
pub fn root_ok(root: &Path) -> bool {
    let s = root.to_string_lossy();
    let b = s.as_bytes();
    b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/') && !s.contains('\0')
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SyncPlan {
    pub add: Vec<String>,
    pub remove: Vec<String>,
    pub enable: Vec<String>,
    pub disable: Vec<String>,
}

/// What a sync does (review I6, `EM:210-240`). `installed` is the profile's (id, enabled) list,
/// `added` this run's (folder, id) completions, `folders` the folders to be loaded, `active` the
/// active ids. Removal waits for every add.
pub fn sync_plan(installed: &[(String, bool)], added: &[(String, String)], folders: &[String], active: &[String]) -> SyncPlan {
    let mut plan = SyncPlan::default();
    plan.add = folders.iter().filter(|f| !added.iter().any(|(af, _)| af == *f)).cloned().collect();
    // The ids the present folders produced this run.
    let produced: Vec<&str> = added.iter().filter(|(f, _)| folders.contains(f)).map(|(_, id)| id.as_str()).collect();
    if plan.add.is_empty() {
        for (id, _) in installed {
            if !produced.contains(&id.as_str()) && !plan.remove.contains(id) {
                plan.remove.push(id.clone());
            }
        }
        for (f, id) in added {
            if !folders.contains(f) && !plan.remove.contains(id) {
                plan.remove.push(id.clone());
            }
        }
    }
    for (id, on) in installed {
        if !produced.contains(&id.as_str()) {
            continue;
        }
        let wanted = active.contains(id);
        if wanted && !on {
            plan.enable.push(id.clone());
        } else if !wanted && *on {
            plan.disable.push(id.clone());
        }
    }
    plan
}

/// `ids` with `id` appended when `on` and absent, removed when not `on` (order kept, `EM:153-169`).
pub fn set_active(ids: &[String], id: &str, on: bool) -> Vec<String> {
    let mut out: Vec<String> = ids.to_vec();
    if on {
        if !out.iter().any(|i| i == id) {
            out.push(id.to_string());
        }
    } else {
        out.retain(|i| i != id);
    }
    out
}

/// An id WebView2 gives an extension: 32 letters a to p.
pub fn valid_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|b| (b'a'..=b'p').contains(&b))
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Manifest {
    pub name: String,
    pub description: Option<String>,
    pub version: String,
    pub permissions: Vec<String>,
    pub host_permissions: Vec<String>,
    /// What it may ask for at run time (`chrome.permissions.request`): shown in the approval too.
    pub optional_permissions: Vec<String>,
    pub optional_host_permissions: Vec<String>,
    /// The `matches` of the content scripts: sites the extension reads and changes too.
    pub content_matches: Vec<String>,
}

impl Manifest {
    /// Every site pattern the extension can reach: host permissions, then content script matches.
    pub fn sites(&self) -> Vec<String> {
        let mut out = self.host_permissions.clone();
        for m in &self.content_matches {
            if !out.contains(m) {
                out.push(m.clone());
            }
        }
        out
    }
}

/// Whether a JSON text nests deeper than `max` (brackets inside strings do not count).
fn too_deep(s: &str, max: usize) -> bool {
    let (mut depth, mut in_str, mut esc) = (0usize, false, false);
    for b in s.bytes() {
        if in_str {
            match (esc, b) {
                (true, _) => esc = false,
                (false, b'\\') => esc = true,
                (false, b'"') => in_str = false,
                _ => {}
            }
            continue;
        }
        match b {
            b'"' => in_str = true,
            b'{' | b'[' => {
                depth += 1;
                if depth > max {
                    return true;
                }
            }
            b'}' | b']' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    false
}

fn text_list(v: Option<&Value>) -> Vec<String> {
    let Some(Value::Array(a)) = v else { return Vec::new() };
    a.iter().filter_map(Value::as_str).map(|s| crate::inert_files::display_text(s, MAX_ENTRY, false)).filter(|s| !s.trim().is_empty()).take(MAX_ENTRIES).collect()
}

/// `manifest.json` of an untrusted folder: None when it is too big, too deep, not a JSON object or
/// has no name.
pub fn manifest(json: &str) -> Option<Manifest> {
    if json.len() > MAX_MANIFEST_BYTES || too_deep(json, MAX_DEPTH) {
        return None;
    }
    let v: Value = serde_json::from_str(json).ok()?;
    let o = v.as_object()?;
    let name = crate::inert_files::display_text(o.get("name")?.as_str()?, MAX_NAME, false);
    if name.trim().is_empty() {
        return None;
    }
    let description = o.get("description").and_then(Value::as_str).map(|d| crate::inert_files::display_text(d, MAX_DESCRIPTION, false)).filter(|d| !d.trim().is_empty());
    let version = o.get("version").and_then(Value::as_str).map(|x| crate::inert_files::display_text(x, MAX_VERSION, false)).unwrap_or_default();
    let mut content_matches = Vec::new();
    if let Some(Value::Array(scripts)) = o.get("content_scripts") {
        for sc in scripts.iter().take(MAX_ENTRIES) {
            for m in text_list(sc.get("matches")) {
                if content_matches.len() < MAX_ENTRIES && !content_matches.contains(&m) {
                    content_matches.push(m);
                }
            }
        }
    }
    Some(Manifest { name, description, version, permissions: text_list(o.get("permissions")), host_permissions: text_list(o.get("host_permissions")), optional_permissions: text_list(o.get("optional_permissions")), optional_host_permissions: text_list(o.get("optional_host_permissions")), content_matches })
}

/// The page's list (`SH:461-472`): rows of (id, manifest, enabled).
pub fn extensions_json(rows: &[(String, Manifest, bool)]) -> String {
    let list: Vec<Value> = rows
        .iter()
        .map(|(id, m, enabled)| {
            json!({
                "id": id,
                "name": m.name,
                "description": m.description.clone().unwrap_or_else(|| "No description available".to_string()),
                "version": m.version,
                "permissions": m.permissions,
                "hostPermissions": m.sites(),
                "enabled": enabled,
            })
        })
        .collect();
    Value::Array(list).to_string()
}

pub fn ui_script(json: &str) -> String {
    format!("updateExtensionsUI({json});")
}

/// SHA-256 of the manifest bytes, lower-case hex: what an approval is bound to.
pub fn manifest_hash(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// The text of the approval window: the manifest's permissions and sites, all of them.
pub fn approval_text(m: &Manifest, folder: &str) -> String {
    // Permissions that reach outside the page sandbox are named first, whether asked now or later.
    let mut t = String::new();
    for (perm, what) in [("debugger", "attach a debugger to pages"), ("nativeMessaging", "start programs installed on this computer")] {
        if m.permissions.iter().chain(m.optional_permissions.iter()).any(|p| p == perm) {
            t.push_str(&format!("WARNING: this extension asks for \"{perm}\": it can {what}.\n"));
        }
    }
    if !t.is_empty() {
        t.push('\n');
    }
    t.push_str(&format!("\"{}\" version {}\n", m.name, if m.version.is_empty() { "unknown" } else { &m.version }));
    if let Some(d) = &m.description {
        t.push_str(d);
        t.push('\n');
    }
    t.push_str(&format!("\nFolder: {}\n\nIt asks for these permissions:\n", crate::inert_files::display_text(folder, 200, false)));
    let list = |t: &mut String, items: &[String], none: &str| {
        if items.is_empty() {
            t.push_str(none);
            t.push('\n');
        }
        for i in items {
            t.push_str(&format!("  {i}\n"));
        }
    };
    list(&mut t, &m.permissions, "  No special permissions.");
    t.push_str("\nIt can read and change pages on these sites:\n");
    list(&mut t, &m.sites(), "  No sites.");
    let later: Vec<String> = m.optional_permissions.iter().chain(m.optional_host_permissions.iter()).cloned().collect();
    if !later.is_empty() {
        t.push_str("\nIt may ask later for:\n");
        list(&mut t, &later, "");
    }
    t.push_str("\nEnable only an extension you trust. It is loaded into all browser profiles that share this browser data, never into Dev Null windows.");
    t
}

/// A folder as the approvals compare it (Windows paths ignore case and take either slash).
pub fn folder_key(folder: &Path) -> String {
    folder.to_string_lossy().replace('/', "\\").trim_end_matches('\\').to_lowercase()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Approval {
    /// Always `STORE`: every Tandem browser profile shares one WebView2 profile (one extension list),
    /// so an approval belongs to that shared browser data (13x8 Task 10 review I1). The field keeps
    /// its name so a file written earlier still parses; such a record no longer matches and is dropped
    /// at the next sync (the extension is removed until it is approved again).
    pub profile: String,
    pub folder: String,
    pub hash: String,
    /// The WebView2 id, once added.
    pub id: Option<String>,
    /// Whether the extension is turned on (the settings page's switch); shared like the approval.
    pub enabled: bool,
}

/// The approvals' `profile` value: the one shared WebView2 profile of `Store::Browser`.
pub const STORE: &str = "browser-data";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Approvals {
    pub list: Vec<Approval>,
}

impl Approvals {
    /// Anything that is not a well-formed file is no approval at all.
    pub fn parse(json: &str) -> Approvals {
        if json.len() > MAX_APPROVALS_BYTES || too_deep(json, 8) {
            return Approvals::default();
        }
        let Ok(Value::Array(rows)) = serde_json::from_str::<Value>(json) else { return Approvals::default() };
        let text = |r: &Value, k: &str| r.get(k).and_then(Value::as_str).map(str::to_string);
        let list = rows
            .iter()
            .filter_map(|r| {
                let id = text(r, "id").filter(|i| valid_id(i));
                Some(Approval { profile: text(r, "profile")?, folder: text(r, "folder")?, hash: text(r, "hash")?, id, enabled: r.get("enabled").and_then(Value::as_bool).unwrap_or(true) })
            })
            .take(MAX_APPROVALS)
            .collect();
        Approvals { list }
    }

    pub fn to_json(&self) -> String {
        let rows: Vec<Value> = self.list.iter().map(|a| json!({"profile": a.profile, "folder": a.folder, "hash": a.hash, "id": a.id, "enabled": a.enabled})).collect();
        Value::Array(rows).to_string()
    }

    /// The record that approves exactly this profile, folder and manifest.
    pub fn find(&self, profile: &str, folder: &str, hash: &str) -> Option<&Approval> {
        self.list.iter().find(|a| a.profile == profile && a.folder == folder && a.hash == hash)
    }

    /// Records the person's approval (replacing an older record of the same profile and folder).
    pub fn approve(&mut self, profile: &str, folder: &str, hash: &str) {
        self.list.retain(|a| !(a.profile == profile && a.folder == folder));
        if self.list.len() < MAX_APPROVALS {
            self.list.push(Approval { profile: profile.to_string(), folder: folder.to_string(), hash: hash.to_string(), id: None, enabled: true });
        }
    }

    pub fn set_id(&mut self, profile: &str, folder: &str, id: &str) {
        if !valid_id(id) {
            return;
        }
        if let Some(a) = self.list.iter_mut().find(|a| a.profile == profile && a.folder == folder) {
            a.id = Some(id.to_string());
        }
    }

    /// Turns the extension with `id` on or off (the one switch, for every browser profile).
    pub fn set_enabled(&mut self, id: &str, on: bool) {
        for a in self.list.iter_mut().filter(|a| a.id.as_deref() == Some(id)) {
            a.enabled = on;
        }
    }

    /// The ids of every approved extension that was added.
    pub fn known_ids(&self) -> Vec<String> {
        self.list.iter().filter_map(|a| a.id.clone()).collect()
    }

    /// Drops the record of an extension that was removed.
    pub fn forget_id(&mut self, profile: &str, id: &str) {
        self.list.retain(|a| !(a.profile == profile && a.id.as_deref() == Some(id)));
    }
}

/// `manifest.json` of `dir` and its hash: a regular file (not a link) of at most `MAX_MANIFEST_BYTES`
/// on a local path, read with a hard limit.
pub fn read_folder_manifest(dir: &Path) -> Option<(Manifest, String)> {
    use std::io::Read;
    let dir = crate::localfs::judge(dir)?;
    let file = dir.child("manifest.json")?;
    let meta = crate::localfs::lstat(&file).ok()?;
    if !meta.is_file || meta.is_reparse || meta.len > MAX_MANIFEST_BYTES as u64 {
        return None;
    }
    let mut bytes = Vec::new();
    crate::localfs::open(&file).ok()?.take(MAX_MANIFEST_BYTES as u64 + 1).read_to_end(&mut bytes).ok()?;
    if bytes.len() > MAX_MANIFEST_BYTES {
        return None;
    }
    let m = manifest(std::str::from_utf8(&bytes).ok()?.trim_start_matches('\u{feff}'))?;
    Some((m, manifest_hash(&bytes)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }
    fn pairs(v: &[(&str, bool)]) -> Vec<(String, bool)> {
        v.iter().map(|(a, b)| (a.to_string(), *b)).collect()
    }
    fn two(v: &[(&str, &str)]) -> Vec<(String, String)> {
        v.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect()
    }

    #[test]
    fn extension_folders_follow_iterm2() {
        assert_eq!(root(None, Path::new(r"C:\Users\u\AppData\Roaming")), PathBuf::from(r"C:\Users\u\AppData\Roaming\Tandem\BrowserExtensions"));
        assert_eq!(root(Some(r"D:\ext"), Path::new(r"C:\x")), PathBuf::from(r"D:\ext"));
        assert_eq!(root(Some("  "), Path::new(r"C:\x")), PathBuf::from(r"C:\x\Tandem\BrowserExtensions"));
        assert_eq!(folders(&pairs(&[("a", true), ("b.txt", false), ("c", true)])), s(&["a", "c"]));
        let many: Vec<(String, bool)> = (0..200).map(|i| (format!("d{i}"), true)).collect();
        assert_eq!(folders(&many).len(), MAX_FOLDERS);
        let key = |k: &str| crate::schema::find(k).unwrap_or_else(|| panic!("{k}"));
        assert!(key(ROOT_KEY).default.is_none() && key(ROOT_KEY).kind == crate::schema::Kind::String);
        assert_eq!(key(ACTIVE_KEY).default.as_ref().and_then(|v| v.as_array()).map(|a| a.len()), Some(0));
        assert!(key(ACTIVE_KEY).kind == crate::schema::Kind::List);
        for k in [ROOT_KEY, ACTIVE_KEY] {
            assert_eq!(key(k).mode.as_deref(), Some("browser"), "{k}");
        }
        assert!(!root_ok(Path::new(r"\\srv\ext")));
        assert!(!root_ok(Path::new(r"\\?\UNC\srv\e")));
        assert!(!root_ok(Path::new(r"\\.\x")));
        assert!(!root_ok(Path::new(r"//srv/ext")));
        assert!(!root_ok(Path::new(r"ext")));
        assert!(root_ok(Path::new(r"D:\ext")));
    }

    #[test]
    fn extension_sync_follows_iterm2() {
        let inst = |v: &[(&str, bool)]| pairs(v);
        let p = sync_plan(&inst(&[("id-a", true), ("id-old", true)]), &two(&[("a", "id-a"), ("gone", "id-g")]), &s(&["a", "b"]), &s(&["id-a"]));
        assert_eq!(p, SyncPlan { add: s(&["b"]), remove: vec![], enable: vec![], disable: vec![] }, "an add is pending");
        let p = sync_plan(&inst(&[("id-a", true), ("id-old", true)]), &two(&[("a", "id-a"), ("gone", "id-g")]), &s(&["a"]), &s(&["id-a"]));
        assert_eq!(p, SyncPlan { add: vec![], remove: s(&["id-old", "id-g"]), enable: vec![], disable: vec![] });
        let p = sync_plan(&inst(&[("id-a", false)]), &two(&[("a", "id-a")]), &s(&["a"]), &s(&["id-a"]));
        assert_eq!(p.enable, s(&["id-a"]));
        let p = sync_plan(&inst(&[("id-a", true)]), &two(&[("a", "id-a")]), &s(&["a"]), &[]);
        assert_eq!(p.disable, s(&["id-a"]));
        assert!(p.enable.is_empty() && p.remove.is_empty() && p.add.is_empty());
    }

    #[test]
    fn active_ids_follow_iterm2() {
        assert_eq!(set_active(&s(&["a"]), "b", true), s(&["a", "b"]));
        assert_eq!(set_active(&s(&["a", "b"]), "a", true), s(&["a", "b"]));
        assert_eq!(set_active(&s(&["a", "b"]), "a", false), s(&["b"]));
        assert_eq!(set_active(&s(&["b"]), "a", false), s(&["b"]));
    }

    #[test]
    fn extensions_json_follows_iterm2() {
        let m = manifest(r#"{"name":"X","version":"1.0","permissions":["storage"],"host_permissions":["*://*/*"]}"#).unwrap();
        assert_eq!(m.description, None);
        assert_eq!(
            extensions_json(&[("id1".to_string(), m, true)]),
            r#"[{"id":"id1","name":"X","description":"No description available","version":"1.0","permissions":["storage"],"hostPermissions":["*://*/*"],"enabled":true}]"#
        );
        assert_eq!(manifest("x"), None);
        assert_eq!(ui_script("[]"), "updateExtensionsUI([]);");
    }

    /// The manifest is untrusted (preflight B2): markup stays text, hidden characters go, sizes and
    /// depth are capped, content script sites are shown as sites.
    #[test]
    fn a_hostile_manifest_is_bounded() {
        let evil = r#"{"name":"<img src=x onerror=alert(1)>\u202e\u200bA","version":"1\u0000.0","description":"d","permissions":["a","b"]}"#;
        let m = manifest(evil).unwrap();
        assert_eq!(m.name, "<img src=x onerror=alert(1)>A");
        assert!(!m.version.contains('\0'));
        let json = extensions_json(&[("x".into(), m, false)]);
        assert!(!json.contains('\u{202e}') && json.contains("<img src=x onerror=alert(1)>"), "the markup travels as text; the page renders it with textContent");
        let long = format!(r#"{{"name":"{}","version":"{}"}}"#, "n".repeat(500), "v".repeat(500));
        let m = manifest(&long).unwrap();
        assert_eq!((m.name.chars().count(), m.version.chars().count()), (MAX_NAME, MAX_VERSION));
        let perms = (0..500).map(|i| format!("\"p{i}\"")).collect::<Vec<_>>().join(",");
        let m = manifest(&format!(r#"{{"name":"X","version":"1","permissions":[{perms}]}}"#)).unwrap();
        assert_eq!(m.permissions.len(), MAX_ENTRIES);
        let deep = format!(r#"{{"name":"X","version":"1","x":{}{}}}"#, "[".repeat(MAX_DEPTH + 1), "]".repeat(MAX_DEPTH + 1));
        assert_eq!(manifest(&deep), None, "too deep");
        let shallow = format!(r#"{{"name":"X","version":"1","x":{}{}}}"#, "[".repeat(MAX_DEPTH - 2), "]".repeat(MAX_DEPTH - 2));
        assert!(manifest(&shallow).is_some());
        let big = format!(r#"{{"name":"X","version":"1","x":"{}"}}"#, "a".repeat(MAX_MANIFEST_BYTES));
        assert_eq!(manifest(&big), None, "too big");
        assert_eq!(manifest(r#"{"version":"1"}"#), None, "no name");
        assert_eq!(manifest(r#"[1]"#), None);
        // A bracket inside a string is not nesting.
        assert!(manifest(&format!(r#"{{"name":"{}","version":"1"}}"#, "[".repeat(100))).is_some());
        let m = manifest(r#"{"name":"X","version":"1","host_permissions":["https://a/*"],"content_scripts":[{"matches":["https://a/*","<all_urls>"],"js":["x.js"]}]}"#).unwrap();
        assert_eq!(m.sites(), s(&["https://a/*", "<all_urls>"]));
    }

    #[test]
    fn the_approval_lists_everything_the_manifest_asks_for() {
        let m = manifest(r#"{"name":"Tabs Tool","version":"2.1","description":"Does things","permissions":["tabs","storage"],"host_permissions":["*://*/*"],"content_scripts":[{"matches":["https://x.test/*"]}]}"#).unwrap();
        let t = approval_text(&m, r"D:\ext\one");
        for want in ["Tabs Tool", "2.1", "Does things", r"D:\ext\one", "tabs", "storage", "*://*/*", "https://x.test/*"] {
            assert!(t.contains(want), "{want}: {t}");
        }
        let none = approval_text(&manifest(r#"{"name":"N","version":"1"}"#).unwrap(), "D:\\e");
        assert!(none.contains("No special permissions") && none.contains("No sites"), "{none}");
        assert!(!t.contains('\u{202e}'));
    }

    /// 13x8 Task 10 review minor 2: permissions that reach outside the page sandbox get a warning on top.
    #[test]
    fn debugger_and_native_messaging_are_warned_about() {
        for (field, perm) in [("permissions", "debugger"), ("permissions", "nativeMessaging"), ("optional_permissions", "debugger"), ("optional_permissions", "nativeMessaging")] {
            let m = manifest(&format!(r#"{{"name":"X","version":"1","{field}":["{perm}"]}}"#)).unwrap();
            let t = approval_text(&m, "D:\\e");
            assert!(t.starts_with("WARNING:") && t.contains(perm), "{field} {perm}: {t}");
        }
        let t = approval_text(&manifest(r#"{"name":"X","version":"1","permissions":["storage"]}"#).unwrap(), "D:\\e");
        assert!(!t.contains("WARNING:"), "{t}");
    }

    /// 13x8 Task 10 review I2: what an extension may ask for later is shown too.
    #[test]
    fn the_approval_lists_what_it_may_ask_for_later() {
        let m = manifest(r#"{"name":"X","version":"1","optional_host_permissions":["<all_urls>"],"optional_permissions":["tabs","cookies"]}"#).unwrap();
        assert_eq!(m.optional_permissions, vec!["tabs".to_string(), "cookies".to_string()]);
        assert_eq!(m.optional_host_permissions, vec!["<all_urls>".to_string()]);
        let t = approval_text(&m, "D:\\e");
        assert!(t.contains("It may ask later for:") && t.contains("  tabs") && t.contains("  cookies") && t.contains("  <all_urls>"), "{t}");
        let plain = approval_text(&manifest(r#"{"name":"X","version":"1"}"#).unwrap(), "D:\\e");
        assert!(!plain.contains("It may ask later for:"), "{plain}");
        let many: Vec<String> = (0..500).map(|i| format!("\"p{i}\"")).collect();
        let big = manifest(&format!(r#"{{"name":"X","optional_permissions":[{}],"optional_host_permissions":[{}]}}"#, many.join(","), many.join(","))).unwrap();
        assert_eq!((big.optional_permissions.len(), big.optional_host_permissions.len()), (MAX_ENTRIES, MAX_ENTRIES));
    }

    #[test]
    fn approvals_bind_profile_folder_and_manifest() {
        let h1 = manifest_hash(b"one");
        assert_eq!(h1.len(), 64);
        assert_ne!(h1, manifest_hash(b"two"));
        let mut a = Approvals::default();
        assert!(a.find("P", "d:\\e", &h1).is_none(), "nothing is approved by default");
        a.approve("P", &folder_key(Path::new(r"D:\Ext")), &h1);
        let key = folder_key(Path::new("d:/ext"));
        assert!(a.find("P", &key, &h1).is_some(), "case and slash do not matter");
        assert!(a.find("Q", &key, &h1).is_none(), "another profile did not approve it");
        assert!(a.find("P", &key, &manifest_hash(b"two")).is_none(), "a changed manifest needs a new approval");
        assert!(a.find("P", &folder_key(Path::new(r"D:\other")), &h1).is_none());
        let id = "a".repeat(32);
        a.set_id("P", &key, "not an id");
        assert_eq!(a.find("P", &key, &h1).unwrap().id, None);
        a.set_id("P", &key, &id);
        assert_eq!(a.find("P", &key, &h1).unwrap().id.as_deref(), Some(id.as_str()));
        let back = Approvals::parse(&a.to_json());
        assert_eq!(back, a);
        a.approve("P", &key, &manifest_hash(b"three"));
        assert_eq!(a.list.len(), 1, "one record per profile and folder");
        a.forget_id("P", &id);
        assert!(a.list.iter().all(|r| r.id.is_none()) || a.list.is_empty());
        for bad in ["", "x", "[1]", "{\"list\":5}", &"[".repeat(100000)] {
            assert!(Approvals::parse(bad).list.is_empty(), "{bad:.20}");
        }
        let many = Approvals { list: (0..1000).map(|i| Approval { profile: "P".into(), folder: format!("f{i}"), hash: h1.clone(), id: None, enabled: true }).collect() };
        assert_eq!(Approvals::parse(&many.to_json()).list.len(), MAX_APPROVALS);
        assert!(Approvals::parse(&format!("{}{}", " ".repeat(MAX_APPROVALS_BYTES), many.to_json())).list.is_empty(), "too big");
    }

    /// 13x8 Task 10 review I1: every Tandem browser profile shares one WebView2 profile, so an approval
    /// belongs to that shared browser data and says so; whether it is on is shared too.
    #[test]
    fn an_approval_is_for_the_shared_browser_data() {
        let m = manifest(r#"{"name":"X","version":"1"}"#).unwrap();
        let t = approval_text(&m, "D:\\e");
        assert!(t.contains("all browser profiles that share this browser data"), "{t}");
        assert!(!t.contains("this browser profile only"), "{t}");
        assert!(t.contains("never into Dev Null"), "{t}");
        let (h, id) = (manifest_hash(b"one"), "a".repeat(32));
        let mut a = Approvals::default();
        a.approve(STORE, "d:\\e", &h);
        a.set_id(STORE, "d:\\e", &id);
        assert!(a.find(STORE, "d:\\e", &h).unwrap().enabled, "approving turns it on");
        a.set_enabled(&id, false);
        assert!(!a.find(STORE, "d:\\e", &h).unwrap().enabled);
        assert_eq!(Approvals::parse(&a.to_json()), a, "off survives the file");
        let old = format!(r#"[{{"profile":"P","folder":"f","hash":"{h}","id":null}}]"#);
        assert!(Approvals::parse(&old).list[0].enabled, "a record with no flag is on");
        assert_eq!(a.known_ids(), vec![id.clone()]);
        a.forget_id(STORE, &id);
        assert!(a.known_ids().is_empty());
    }

    #[test]
    fn a_folder_manifest_is_read_with_a_limit() {
        let base = std::env::temp_dir().join(format!("tandem-ext-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let ok = base.join("ok");
        let big = base.join("big");
        let none = base.join("none");
        for d in [&ok, &big, &none] {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::write(ok.join("manifest.json"), r#"{"name":"X","version":"1"}"#).unwrap();
        std::fs::write(big.join("manifest.json"), vec![b' '; MAX_MANIFEST_BYTES + 10]).unwrap();
        let (m, h) = read_folder_manifest(&ok).unwrap();
        assert_eq!(m.name, "X");
        assert_eq!(h, manifest_hash(br#"{"name":"X","version":"1"}"#));
        assert!(read_folder_manifest(&big).is_none() && read_folder_manifest(&none).is_none());
        let dir_as_manifest = base.join("dm");
        std::fs::create_dir_all(dir_as_manifest.join("manifest.json")).unwrap();
        assert!(read_folder_manifest(&dir_as_manifest).is_none(), "a directory named manifest.json");
        assert!(read_folder_manifest(Path::new(r"\\srv\share\ext")).is_none(), "a network path is never read");
        let _ = std::fs::remove_dir_all(&base);
    }
}
