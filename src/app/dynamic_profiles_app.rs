//! Dynamic Profiles in the app (M13v2 Task 6): the watched folder, what a reload does to the
//! profile list, the error alert, and writing an edit of a rewritable profile back to its file
//! (`iTermDynamicProfileManager.m`). The pure parts (`apply_loaded`, `rewrite`) take what they
//! touch as arguments; the `App` methods wire them to the store, the scanner and the console.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::time::{Instant, SystemTime};

use serde_json::Value as Json;

use super::scripts_alerts::{ask, Ask, Then};
use super::App;
use crate::dynamic_profiles::{self as dp, Changes, Entry, Gate, Loaded, Parent};
use crate::profile::Profile;
use crate::store::Store;
use crate::warnings::Kind;

pub(super) const WARNING_ID: &str = "NoSyncDynamicProfilesWarning";
/// Keys whose change in a file already loaded is logged: they decide what a profile runs.
const RUNS_KEYS: [&str; 4] = ["Command", "Custom Command", "Initial Text", "Triggers"];

// ---- What a reload does to the list ----------------------------------------------------------

fn is_dynamic(p: &Profile) -> bool {
    p.bool(dp::DYNAMIC_KEY)
}

/// Applies what the folder holds to the store (`reallyReloadDynamicProfiles`, `DP:312-340`): a
/// dynamic profile no longer in a file is removed (as a delete does, so it never touches the
/// saved profiles), one still there is merged over its parent again and replaced where it is,
/// a new one is added at the end. A saved profile is never touched or replaced. Gives the
/// changes (`update` holds only what differs) and the unknown-parent errors, one per profile.
pub(super) fn apply_loaded(store: &mut Store, loaded: &Loaded, add_tag: bool) -> (Changes, Vec<String>) {
    let new: Vec<String> = loaded.profiles.iter().filter_map(|(e, _)| e.get("Guid").and_then(Json::as_str).map(str::to_string)).collect();
    let mut changes = dp::changes(&store.dynamic_guids(), &new);
    let mut errors = Vec::new();
    let mut updated = Vec::new();
    for (entry, _) in &loaded.profiles {
        let Some(guid) = entry.get("Guid").and_then(Json::as_str) else { continue };
        let parent = {
            let others = || store.profiles.iter().filter(|p| p.guid() != guid);
            match dp::parent_of(entry, &|g| others().any(|p| p.guid() == g), &|n| others().find(|p| p.name() == n).map(|p| p.guid())) {
                Parent::Default => store.default_profile().clone(),
                Parent::Found(g) => others().find(|p| p.guid() == g).cloned().unwrap_or_else(|| store.default_profile().clone()),
                Parent::Missing(text) => {
                    errors.push(text);
                    store.default_profile().clone()
                }
            }
        };
        let merged = dp::merge(entry, &parent, add_tag);
        match store.profiles.iter().position(|p| p.guid() == guid) {
            Some(i) if is_dynamic(&store.profiles[i]) => {
                if store.profiles[i].values != merged.values {
                    store.profiles[i] = merged;
                    updated.push(guid.to_string());
                }
            }
            // A saved profile with this Guid: never replaced (the loader refuses it first).
            Some(_) => {}
            None => store.add(merged),
        }
    }
    for guid in &changes.remove {
        store.delete(guid);
    }
    changes.update = updated;
    (changes, errors)
}

/// The alert for a reload's errors (`reallyReportError:`, `DP:176-222`): the first error and how
/// many more the log holds; OK and View Log (never remembered), and Reveal in Explorer when the
/// first error's file exists.
pub(super) fn error_ask(errors: &[(String, String)], reveal: Option<PathBuf>) -> Ask {
    let messages: Vec<String> = errors.iter().map(|(_, m)| m.clone()).collect();
    let mut labels = vec!["OK", dp::VIEW_LOG];
    if reveal.is_some() {
        labels.push(dp::REVEAL);
    }
    ask(dp::ALERT_HEADING, &dp::alert_text(&messages), &labels, Some(WARNING_ID), Kind::Temporary, Then::DynamicProfilesError(reveal))
}

// ---- Writing back (B7) -----------------------------------------------------------------------

/// What a file looked like when it was read: its size, its modified time and an FNV-1a hash of
/// its bytes. A write-back goes ahead only when the file still matches.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Stamp {
    len: usize,
    hash: u64,
    mtime: Option<SystemTime>,
}

impl Stamp {
    pub(super) fn new(bytes: &[u8], mtime: Option<SystemTime>) -> Stamp {
        let hash = bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ u64::from(*b)).wrapping_mul(0x0000_0100_0000_01b3));
        Stamp { len: bytes.len(), hash, mtime }
    }
}

/// What a write-back touches, so the tests can stand in for the disk.
pub(super) struct Fs<'a> {
    pub local: &'a dyn Fn(&Path) -> bool,
    pub read_link: &'a dyn Fn(&Path) -> Option<PathBuf>,
    /// The number of names the file has (hard links).
    pub links: &'a dyn Fn(&Path) -> u32,
    pub mtime: &'a dyn Fn(&Path) -> Option<SystemTime>,
    pub read: &'a dyn Fn(&Path) -> Result<Vec<u8>, String>,
    /// Makes a new file (it must not exist) holding the bytes.
    pub write_new: &'a dyn Fn(&Path, &[u8]) -> std::io::Result<()>,
    pub rename: &'a dyn Fn(&Path, &Path) -> std::io::Result<()>,
}

fn write_new(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(path)?;
    f.write_all(bytes)?;
    f.sync_all()
}

fn mtime_of(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

#[cfg(windows)]
fn link_count(path: &Path) -> u32 {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle};
    let Ok(f) = std::fs::File::open(path) else { return u32::MAX };
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: the handle is open for the call and `info` is a plain out structure.
    if unsafe { GetFileInformationByHandle(f.as_raw_handle() as _, &mut info) } == 0 {
        return u32::MAX;
    }
    info.nNumberOfLinks
}

#[cfg(not(windows))]
fn link_count(_path: &Path) -> u32 {
    1
}

impl Fs<'static> {
    pub(super) fn real() -> Fs<'static> {
        Fs {
            local: &dp::system_local,
            read_link: &crate::scripts::read_link,
            links: &link_count,
            mtime: &mtime_of,
            read: &|p| dp::bounded_read(p, dp::MAX_FILE),
            write_new: &write_new,
            rename: &|a, b| std::fs::rename(a, b),
        }
    }
}

#[derive(Debug)]
pub(super) enum Outcome {
    Wrote,
    /// The file is not the one that was read: nothing was written.
    Changed,
    Refused(String),
}

/// The path of `name` below `folder`: a name relative to it, or a full path inside it, made only
/// of plain names (no "..", no root, no drive).
fn below(folder: &Path, name: &str) -> Option<PathBuf> {
    let p = Path::new(name);
    let rel = if p.is_absolute() || p.has_root() || p.components().any(|c| matches!(c, Component::Prefix(_))) { p.strip_prefix(folder).ok()? } else { p };
    let mut out = PathBuf::new();
    for c in rel.components() {
        match c {
            Component::Normal(n) => out.push(n),
            _ => return None,
        }
    }
    (!out.as_os_str().is_empty()).then_some(out)
}

/// Rewrites the entry of profile `guid` in the file `name` (the profile's "Dynamic Profile
/// Filename") with `change`, the rest of the file as it is. The path is resolved again below
/// `folder`; a link (at the file or any folder below `folder`), a file with more than one name, a
/// path the network judge refuses, or a file that no longer matches `stamp` is never written
/// through. The bytes go to a hidden temporary file in the same folder (made new), then rename
/// over the original, so a failure leaves the original whole.
pub(super) fn rewrite(fs: &Fs, folder: &Path, name: &str, stamp: &Stamp, guid: &str, change: &dyn Fn(&mut Entry)) -> Outcome {
    let Some(rel) = below(folder, name) else { return Outcome::Refused("its file is not inside the Dynamic Profiles folder".into()) };
    let path = folder.join(&rel);
    if !(fs.local)(&path) {
        return Outcome::Refused("its file is on a network path or reached through a link to one".into());
    }
    let mut at = folder.to_path_buf();
    for part in rel.components() {
        at.push(part);
        if (fs.read_link)(&at).is_some() {
            return Outcome::Refused("its file is, or is inside, a link".into());
        }
    }
    if (fs.links)(&path) > 1 {
        return Outcome::Refused("its file has more than one name (a hard link)".into());
    }
    let bytes = match (fs.read)(&path) {
        Ok(b) => b,
        Err(e) => return Outcome::Refused(format!("its file could not be read: {e}")),
    };
    if Stamp::new(&bytes, (fs.mtime)(&path)) != *stamp {
        return Outcome::Changed;
    }
    let Some((list, kind)) = dp::raw_profiles(&bytes) else { return Outcome::Refused("its file could not be read as Dynamic Profiles".into()) };
    let mut entries: Vec<Entry> = list.into_iter().filter_map(|e| if let Json::Object(o) = e { Some(o) } else { None }).collect();
    let Some(mine) = entries.iter_mut().find(|e| e.get("Guid").and_then(Json::as_str) == Some(guid)) else { return Outcome::Changed };
    change(mine);
    let out = dp::write_file(entries, kind);
    if out.is_empty() {
        return Outcome::Refused("its file could not be written".into());
    }
    let Some(file_name) = path.file_name() else { return Outcome::Refused("its file has no name".into()) };
    let tmp = path.with_file_name(format!(".{}.tandem-tmp", file_name.to_string_lossy()));
    let mut made = (fs.write_new)(&tmp, &out);
    if matches!(&made, Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists) {
        // Left by a crash: ours alone (a dotfile with this suffix).
        let _ = std::fs::remove_file(&tmp);
        made = (fs.write_new)(&tmp, &out);
    }
    if let Err(e) = made {
        let _ = std::fs::remove_file(&tmp);
        return Outcome::Refused(format!("could not write: {e}"));
    }
    if let Err(e) = (fs.rename)(&tmp, &path) {
        let _ = std::fs::remove_file(&tmp);
        return Outcome::Refused(format!("could not replace the file: {e}"));
    }
    Outcome::Wrote
}

// ---- The app ---------------------------------------------------------------------------------

/// A file's text as the loader names it, with what it looked like when read.
#[derive(Clone, Debug)]
pub(super) struct Source {
    path: String,
    stamp: Option<Stamp>,
}

impl App {
    /// At launch, before the first window (an arrangement may name a dynamic profile): the
    /// folder is made, watched, and read.
    pub(super) fn dynamic_profiles_start(&mut self) {
        let appdata = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
        let home = std::env::var_os("USERPROFILE").map(PathBuf::from).unwrap_or_default();
        let folder = dp::folder(&self.globals.string("DynamicProfilesPath"), &appdata, &home);
        // Never made on a network path; the reload then reports it.
        if dp::system_local(&folder) {
            let _ = std::fs::create_dir_all(&folder);
        }
        let latency = self.globals.number("DynamicProfilesNotificationLatency").filter(|v| v.is_finite()).unwrap_or(0.1).clamp(0.01, 60.0);
        self.dynamic_scan = Some(crate::dynamic_profiles_scan::Scanner::start(folder, std::time::Duration::from_secs_f64(latency), self.notify.clone()));
        self.reload_dynamic_profiles();
    }

    /// Each loop pass: a reload the watch asked for, and the write-back an edit waits on. Gives
    /// when the write-back falls due (nothing while nothing waits).
    pub(super) fn dynamic_profiles_tick(&mut self) -> Option<Instant> {
        if self.dynamic_scan.as_ref().is_some_and(|s| s.take_reload()) {
            self.reload_dynamic_profiles();
        }
        if self.dynamic_write_due.is_some_and(|t| Instant::now() >= t) {
            self.dynamic_write_due = None;
            for guid in std::mem::take(&mut self.dynamic_write) {
                self.write_back_dynamic(&guid);
            }
        }
        self.dynamic_write_due
    }

    fn dynamic_log(&mut self, text: &str) {
        let stamp = self.history_stamp();
        self.script_history.add_dynamic_profiles_entry(super::api_ui::unix_secs());
        self.script_history.add_log(crate::script_history::DYNAMIC_PROFILES_ID, &format!("{text}\n"), &stamp);
    }

    /// Reads the folder and applies it. Errors go to the Dynamic Profiles console entry, and
    /// ask once (`DP:167-222`).
    pub(super) fn reload_dynamic_profiles(&mut self) {
        let Some(folder) = self.dynamic_scan.as_ref().map(|s| s.folder().to_path_buf()) else { return };
        let add_tag = self.globals.bool("AddDynamicTagToDynamicProfiles");
        let stamps: std::cell::RefCell<HashMap<String, Stamp>> = Default::default();
        let loaded = {
            let store = &self.store;
            // The one open of each object answers its type, its verdict and its bytes; a file's
            // bytes are stamped here for a later write-back.
            let open = |p: &Path| {
                let mtime = mtime_of(p);
                let o = dp::system_open(p)?;
                if let Ok(bytes) = &o.bytes {
                    stamps.borrow_mut().insert(p.display().to_string(), Stamp::new(bytes, mtime));
                }
                Ok(o)
            };
            let list = |f: &Path| dp::list_dir(f, &dp::system_local, dp::MAX_ENTRIES);
            let saved = |g: &str| store.profiles.iter().any(|p| p.guid() == g && !is_dynamic(p));
            dp::load_gated(&folder, &list, &saved, &Gate { local: &dp::system_local, open: &open })
        };
        // A removed profile's running sessions keep their settings on a copy of their own.
        let now: Vec<String> = loaded.profiles.iter().filter_map(|(e, _)| e.get("Guid").and_then(Json::as_str).map(str::to_string)).collect();
        let gone: Vec<String> = self.store.dynamic_guids().into_iter().filter(|g| !now.contains(g)).collect();
        let users: Vec<_> = self.panes.iter().filter(|(_, p)| gone.contains(&p.profile)).map(|(id, _)| *id).collect();
        for pane in users {
            self.divorce(pane);
        }
        let before: HashMap<String, Vec<Option<toml::Value>>> = self.store.profiles.iter().filter(|p| is_dynamic(p)).map(|p| (p.guid(), RUNS_KEYS.iter().map(|k| p.values.get(*k).cloned()).collect())).collect();
        let (changes, parent_errors) = apply_loaded(&mut self.store, &loaded, add_tag);
        for guid in &changes.update {
            let Some(p) = self.store.get(guid) else { continue };
            let after: Vec<Option<toml::Value>> = RUNS_KEYS.iter().map(|k| p.values.get(*k).cloned()).collect();
            if before.get(guid).is_some_and(|b| *b != after) {
                let line = format!("Dynamic profile {} changed what it runs (its command, initial text or triggers).", crate::profile_list::display_text(&p.name()));
                self.dynamic_log(&line);
            }
        }
        let stamps = stamps.into_inner();
        self.dynamic_files = loaded
            .profiles
            .iter()
            .filter_map(|(e, file)| Some((e.get("Guid").and_then(Json::as_str)?.to_string(), Source { path: file.clone(), stamp: stamps.get(file).cloned() })))
            .collect();
        let mut errors = loaded.errors.clone();
        errors.extend(parent_errors.into_iter().map(|m| (String::new(), m)));
        self.dynamic_report(&errors);
        if changes.add.is_empty() && changes.update.is_empty() && changes.remove.is_empty() {
            return;
        }
        self.dynamic_refresh(&changes);
    }

    /// Logs every error, and asks once unless an alert for an earlier burst still waits.
    fn dynamic_report(&mut self, errors: &[(String, String)]) {
        if errors.is_empty() {
            return;
        }
        for (_, message) in errors {
            self.dynamic_log(message);
        }
        if self.alert_waits(|t| matches!(t, Then::DynamicProfilesError(_))) {
            self.dynamic_errors_pending += errors.len();
            return;
        }
        let reveal = Some(PathBuf::from(&errors[0].0)).filter(|p| !errors[0].0.is_empty() && crate::scripts::stays_local(p) && p.is_file());
        self.ask_scripts(error_ask(errors, reveal));
    }

    /// The profiles menu, Open Quickly and Settings follow a change to the list.
    fn dynamic_refresh(&mut self, changes: &Changes) {
        let pruned = crate::url_schemes::prune_globals(&mut self.globals, &self.store);
        if pruned {
            self.save_globals();
        }
        self.menus_dirty = true;
        if self.gpu.is_none() {
            return;
        }
        for s in self.aps.values_mut() {
            s.mark_dirty();
        }
        for w in [self.settings.as_mut(), self.profiles_win.as_mut()].into_iter().flatten() {
            if changes.remove.iter().any(|g| w.selected_guid() == Some(g.as_str())) {
                let default = self.store.default_guid.clone();
                w.select(&default);
            }
            w.request_redraw();
        }
        self.refresh_panes();
        self.register_hotkeys();
        self.redraw();
    }

    /// An edit to a profile in Settings: when it is a dynamic one marked rewritable, its file is
    /// written once the edit settles (a drag edits every frame).
    pub(super) fn dynamic_edited(&mut self, guid: &str) {
        if guid.is_empty() || !self.store.profiles.iter().any(|p| p.guid() == guid && is_dynamic(p) && p.bool(dp::REWRITABLE_KEY)) {
            return;
        }
        self.dynamic_write.insert(guid.to_string());
        self.dynamic_write_due = Some(Instant::now() + super::SAVE_DELAY);
    }

    /// The edits of a rewritable dynamic profile go back to its file (`DP:676-828`): only the
    /// keys that differ from its parent, the file's own type kept. Then one reload.
    pub(super) fn write_back_dynamic(&mut self, guid: &str) {
        let Some(profile) = self.store.profiles.iter().find(|p| p.guid() == guid && is_dynamic(p) && p.bool(dp::REWRITABLE_KEY)).cloned() else { return };
        let add_tag = self.globals.bool("AddDynamicTagToDynamicProfiles");
        let (profiles, default) = (self.store.profiles.clone(), self.store.default_profile().clone());
        let change = |e: &mut Entry| {
            let others = || profiles.iter().filter(|p| p.guid() != guid);
            let parent = match dp::parent_of(e, &|g| others().any(|p| p.guid() == g), &|n| others().find(|p| p.name() == n).map(|p| p.guid())) {
                Parent::Found(g) => others().find(|p| p.guid() == g).cloned(),
                _ => None,
            }
            .unwrap_or_else(|| default.clone());
            *e = dp::rewritten(e, &profile, &parent, add_tag);
        };
        self.rewrite_dynamic(guid, &change);
    }

    /// "Mark as Rewritable": writes `Rewritable` true into the profile's own entry.
    pub(super) fn mark_rewritable(&mut self, guid: &str) {
        self.rewrite_dynamic(guid, &|e: &mut Entry| {
            e.insert(dp::REWRITABLE_KEY.into(), Json::Bool(true));
        });
    }

    fn rewrite_dynamic(&mut self, guid: &str, change: &dyn Fn(&mut Entry)) {
        let (Some(folder), Some(source)) = (self.dynamic_scan.as_ref().map(|s| s.folder().to_path_buf()), self.dynamic_files.get(guid).cloned()) else { return };
        let Some(stamp) = source.stamp.clone() else { return };
        let name = self.store.profiles.iter().find(|p| p.guid() == guid).map(|p| crate::profile_list::display_text(&p.name())).unwrap_or_default();
        match rewrite(&Fs::real(), &folder, &source.path, &stamp, guid, change) {
            Outcome::Wrote => {}
            Outcome::Changed => self.dynamic_log(&format!("Dynamic profile {name} changed on disk, not rewritten.")),
            Outcome::Refused(why) => {
                let line = format!("Dynamic profile {name} was not rewritten: {why}.");
                self.dynamic_log(&line);
                self.banners.set("dynamic", line);
            }
        }
        // One reload either way: the file's own state is what the profile shows.
        self.reload_dynamic_profiles();
    }

    /// "Reveal in Explorer" for a dynamic profile's file (Settings' warning).
    pub(super) fn reveal_dynamic(&mut self, guid: &str) {
        let (Some(folder), Some(source)) = (self.dynamic_scan.as_ref().map(|s| s.folder().to_path_buf()), self.dynamic_files.get(guid).cloned()) else { return };
        if let Some(rel) = below(&folder, &source.path) {
            let path = folder.join(rel);
            if crate::scripts::stays_local(&path) {
                self.reveal_in_explorer(&path.display().to_string());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dynamic_profiles::{Entry, FileType, Loaded};
    use crate::profile::Profile;
    use crate::store::Store;
    use serde_json::json;
    use std::path::{Path, PathBuf};

    fn entry(v: serde_json::Value) -> Entry {
        v.as_object().expect("an object").clone()
    }

    fn scratch(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
        let dir = std::env::temp_dir().join(format!("tandem-dynapp-{tag}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn store() -> Store {
        Store::fresh(&scratch("store").join("profiles.toml"))
    }

    fn loaded(entries: Vec<Entry>) -> Loaded {
        Loaded { profiles: entries.into_iter().map(|e| (e, "f.json".to_string())).collect(), errors: Vec::new() }
    }

    fn guids(s: &Store) -> Vec<String> {
        s.profiles.iter().map(|p| p.guid()).collect()
    }

    #[test]
    fn apply_follows_iterm2() {
        let mut s = store();
        let saved = s.profiles[0].guid();
        let saved_before = s.profiles[0].values.clone();
        // A and B came from an earlier load.
        let first = loaded(vec![entry(json!({"Guid": "A", "Name": "A"})), entry(json!({"Guid": "B", "Name": "B"}))]);
        let (c, errors) = apply_loaded(&mut s, &first, false);
        assert_eq!((c.add.clone(), c.update.len(), c.remove.len()), (vec!["A".to_string(), "B".to_string()], 0, 0));
        assert!(errors.is_empty());
        assert_eq!(guids(&s), [saved.as_str(), "A", "B"]);
        assert_eq!(s.dynamic_guids(), ["A", "B"]);

        // Now the folder holds B (changed) and C, whose parent is nowhere.
        let next = loaded(vec![
            entry(json!({"Guid": "B", "Name": "B again", "Columns": 120})),
            entry(json!({"Guid": "C", "Name": "C", "Dynamic Profile Parent Name": "Nope"})),
        ]);
        let (c, errors) = apply_loaded(&mut s, &next, false);
        assert_eq!(c.add, ["C"]);
        assert_eq!(c.update, ["B"]);
        assert_eq!(c.remove, ["A"]);
        // A is gone, B replaced in its place, C at the end, the saved profile untouched.
        assert_eq!(guids(&s), [saved.as_str(), "B", "C"]);
        assert_eq!(s.profiles[0].values, saved_before);
        assert_eq!(s.get("B").unwrap().name(), "B again");
        assert_eq!(s.get("B").unwrap().int("Columns"), 120);
        // The missing parent is reported once, in iTerm2's words, and the default profile stands in.
        assert_eq!(errors, [crate::dynamic_profiles::unknown_parent_text("C", None, Some("Nope"))]);
        assert!(s.get("C").unwrap().bool("Is Dynamic Profile"));

        // The same folder again changes nothing (the update list holds only what differs).
        let (c, errors) = apply_loaded(&mut s, &next, false);
        assert_eq!((c.add.len(), c.update.len(), c.remove.len()), (0, 0, 0));
        assert_eq!(errors.len(), 1, "the missing parent is still reported on every load");
        // The Dynamic tag is added only when asked.
        let tagged = loaded(vec![entry(json!({"Guid": "B", "Name": "B again", "Columns": 120}))]);
        let (c, _) = apply_loaded(&mut s, &tagged, true);
        assert_eq!(c.update, ["B"]);
        assert_eq!(s.get("B").unwrap().get("Tags"), Some(toml::Value::Array(vec![toml::Value::String("Dynamic".into())])));
    }

    #[test]
    fn a_parent_is_found_by_guid_then_by_name_and_never_the_profile_itself() {
        let mut s = store();
        let mut parent = Profile::new("Parent");
        parent.set("Columns", toml::Value::Integer(90));
        let pg = parent.guid();
        s.add(parent);
        let l = loaded(vec![
            entry(json!({"Guid": "ByGuid", "Name": "x", "Dynamic Profile Parent GUID": pg})),
            entry(json!({"Guid": "ByName", "Name": "y", "Dynamic Profile Parent Name": "Parent"})),
            entry(json!({"Guid": "Self", "Name": "z", "Dynamic Profile Parent GUID": "Self"})),
        ]);
        let (_, errors) = apply_loaded(&mut s, &l, false);
        assert_eq!(s.get("ByGuid").unwrap().int("Columns"), 90);
        assert_eq!(s.get("ByName").unwrap().int("Columns"), 90);
        assert_eq!(errors.len(), 1, "a profile is not its own parent: {errors:?}");
    }

    #[test]
    fn removing_the_last_dynamic_profiles_keeps_the_saved_ones_and_the_default() {
        let mut s = store();
        let default = s.default_guid.clone();
        apply_loaded(&mut s, &loaded(vec![entry(json!({"Guid": "A", "Name": "A"}))]), false);
        let (c, _) = apply_loaded(&mut s, &loaded(Vec::new()), false);
        assert_eq!(c.remove, ["A"]);
        assert_eq!(guids(&s), [default.as_str()]);
        assert_eq!(s.default_guid, default);
    }

    #[test]
    fn the_error_alert_follows_iterm2() {
        let errors = vec![("C:\\d\\a.json".to_string(), "one".to_string()), ("C:\\d\\b.json".to_string(), "two".to_string()), ("C:\\d\\c.json".to_string(), "three".to_string())];
        let ask = error_ask(&errors, None);
        assert_eq!(ask.spec.heading, "Dynamic Profiles Error");
        assert_eq!(ask.spec.text, "There was a problem with one of your Dynamic Profiles:\n\none\n\n2 additional errors may be seen in the log.");
        assert_eq!(ask.spec.labels, ["OK", "View Log"]);
        assert_eq!(ask.spec.id.as_deref(), Some("NoSyncDynamicProfilesWarning"));
        assert_eq!(ask.spec.kind, crate::warnings::Kind::Temporary);
        assert_eq!(ask.spec.never_remember, vec![1], "View Log is never remembered");
        // A file that exists adds Reveal in Explorer, never remembered either.
        let with = error_ask(&errors[..1], Some(PathBuf::from("C:\\d\\a.json")));
        assert_eq!(with.spec.labels, ["OK", "View Log", "Reveal in Explorer"]);
        assert_eq!(with.spec.never_remember, vec![1, 2]);
        assert_eq!(with.spec.text, "There was a problem with one of your Dynamic Profiles:\n\none");
    }

    // ---- write-back (B7) ---------------------------------------------------------------------

    fn write_json(dir: &Path, name: &str, v: serde_json::Value) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, serde_json::to_vec_pretty(&v).unwrap()).unwrap();
        p
    }

    fn stamp_of_file(p: &Path) -> Stamp {
        let bytes = std::fs::read(p).unwrap();
        Stamp::new(&bytes, std::fs::metadata(p).and_then(|m| m.modified()).ok())
    }

    fn real() -> Fs<'static> {
        Fs { local: &|_| true, ..Fs::real() }
    }

    fn rename_it(e: &mut Entry) {
        e.insert("Name".into(), json!("Renamed"));
    }

    fn names(p: &Path) -> Vec<String> {
        let (list, _) = crate::dynamic_profiles::raw_profiles(&std::fs::read(p).unwrap()).unwrap();
        list.iter().filter_map(|e| e.get("Name").and_then(|n| n.as_str()).map(str::to_string)).collect()
    }

    fn leftovers(dir: &Path) -> Vec<String> {
        std::fs::read_dir(dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().to_string()).filter(|n| n.contains("tandem-tmp")).collect()
    }

    #[test]
    fn write_back_writes_one_entry_and_keeps_the_rest() {
        let dir = scratch("wb");
        let p = write_json(&dir, "a.json", json!({"Profiles": [{"Guid": "G", "Name": "Old"}, {"Guid": "H", "Name": "Other"}]}));
        let out = rewrite(&real(), &dir, &p.display().to_string(), &stamp_of_file(&p), "G", &rename_it);
        assert!(matches!(out, Outcome::Wrote), "{out:?}");
        assert_eq!(names(&p), ["Renamed", "Other"]);
        assert!(leftovers(&dir).is_empty(), "{:?}", leftovers(&dir));
        // The same name relative to the folder works too.
        let again = rewrite(&real(), &dir, "a.json", &stamp_of_file(&p), "H", &|e| {
            e.insert("Name".into(), json!("H2"));
        });
        assert!(matches!(again, Outcome::Wrote), "{again:?}");
        assert_eq!(names(&p), ["Renamed", "H2"]);
        // A property list keeps its type.
        let plist = crate::dynamic_profiles::write_file(vec![entry(json!({"Guid": "P", "Name": "Pl"}))], FileType::PropertyList);
        let pp = dir.join("b.plist");
        std::fs::write(&pp, &plist).unwrap();
        let out = rewrite(&real(), &dir, "b.plist", &stamp_of_file(&pp), "P", &rename_it);
        assert!(matches!(out, Outcome::Wrote), "{out:?}");
        let (list, kind) = crate::dynamic_profiles::raw_profiles(&std::fs::read(&pp).unwrap()).unwrap();
        assert_eq!(kind, FileType::PropertyList);
        assert_eq!(list[0]["Name"], "Renamed");
    }

    #[test]
    fn write_back_refuses_a_link() {
        let dir = scratch("link");
        let p = write_json(&dir, "a.json", json!({"Profiles": [{"Guid": "G", "Name": "Old"}]}));
        let before = std::fs::read(&p).unwrap();
        let stamp = stamp_of_file(&p);
        // The file itself is a link.
        let itself = p.clone();
        let fs = Fs { read_link: &move |q: &Path| (q == itself).then(|| PathBuf::from("C:\\elsewhere\\x.json")), ..real() };
        assert!(matches!(rewrite(&fs, &dir, "a.json", &stamp, "G", &rename_it), Outcome::Refused(_)));
        // A folder above it, inside the Dynamic Profiles folder, is one.
        std::fs::create_dir(dir.join("sub")).unwrap();
        let q = write_json(&dir.join("sub"), "b.json", json!({"Profiles": [{"Guid": "S", "Name": "S"}]}));
        let sub = dir.join("sub");
        let fs = Fs { read_link: &move |q: &Path| (q == sub).then(|| PathBuf::from("C:\\elsewhere")), ..real() };
        assert!(matches!(rewrite(&fs, &dir, "sub/b.json", &stamp_of_file(&q), "S", &rename_it), Outcome::Refused(_)));
        // A hard link to another name is refused too.
        let fs = Fs { links: &|_| 2, ..real() };
        assert!(matches!(rewrite(&fs, &dir, "a.json", &stamp, "G", &rename_it), Outcome::Refused(_)));
        assert_eq!(std::fs::read(&p).unwrap(), before, "nothing was written through a link");
        assert!(leftovers(&dir).is_empty());
    }

    #[test]
    fn write_back_refuses_a_changed_file() {
        let dir = scratch("changed");
        let p = write_json(&dir, "a.json", json!({"Profiles": [{"Guid": "G", "Name": "Old"}]}));
        let stamp = stamp_of_file(&p);
        // The person's editor saved between the load and the write.
        std::fs::write(&p, serde_json::to_vec_pretty(&json!({"Profiles": [{"Guid": "G", "Name": "Edited by hand!"}]})).unwrap()).unwrap();
        let after = std::fs::read(&p).unwrap();
        assert!(matches!(rewrite(&real(), &dir, "a.json", &stamp, "G", &rename_it), Outcome::Changed));
        assert_eq!(std::fs::read(&p).unwrap(), after);
        // A file that is gone is not rewritten either.
        std::fs::remove_file(&p).unwrap();
        assert!(matches!(rewrite(&real(), &dir, "a.json", &stamp, "G", &rename_it), Outcome::Refused(_) | Outcome::Changed));
        assert!(!p.exists());
    }

    #[test]
    fn write_back_stays_in_the_folder() {
        let dir = scratch("stay");
        let stamp = Stamp::new(b"x", None);
        let never = Fs { read: &|_| panic!("read outside the folder"), read_link: &|_| panic!("probed outside the folder"), links: &|_| panic!("probed"), mtime: &|_| panic!("probed"), write_new: &|_, _| panic!("wrote outside the folder"), rename: &|_, _| panic!("renamed outside the folder"), ..real() };
        for name in ["..\\x.json", "..\\..\\x.json", "C:\\x.json", "\\\\h\\s\\x.json", "sub\\..\\..\\x.json", "\\x.json", "", ".", "C:x.json"] {
            assert!(matches!(rewrite(&never, &dir, name, &stamp, "G", &rename_it), Outcome::Refused(_)), "{name:?}");
        }
        // An absolute path inside another folder is outside this one.
        let other = scratch("stay-other");
        assert!(matches!(rewrite(&never, &dir, &other.join("a.json").display().to_string(), &stamp, "G", &rename_it), Outcome::Refused(_)));
        // The network judge has the last word on the path.
        let fs = Fs { local: &|_| false, read: &|_| panic!("read"), ..real() };
        assert!(matches!(rewrite(&fs, &dir, "a.json", &stamp, "G", &rename_it), Outcome::Refused(_)));
    }

    #[test]
    fn write_back_is_atomic() {
        let dir = scratch("atomic");
        let p = write_json(&dir, "a.json", json!({"Profiles": [{"Guid": "G", "Name": "Old"}]}));
        let before = std::fs::read(&p).unwrap();
        let stamp = stamp_of_file(&p);
        // The temporary file cannot be written: the original is intact and nothing is left.
        let fs = Fs { write_new: &|_, _| Err(std::io::Error::other("disk full")), ..real() };
        assert!(matches!(rewrite(&fs, &dir, "a.json", &stamp, "G", &rename_it), Outcome::Refused(_)));
        assert_eq!(std::fs::read(&p).unwrap(), before);
        assert!(leftovers(&dir).is_empty());
        // The rename fails: the original is intact and the temporary file is removed.
        let fs = Fs { rename: &|_, _| Err(std::io::Error::other("locked")), ..real() };
        assert!(matches!(rewrite(&fs, &dir, "a.json", &stamp, "G", &rename_it), Outcome::Refused(_)));
        assert_eq!(std::fs::read(&p).unwrap(), before);
        assert!(leftovers(&dir).is_empty(), "{:?}", leftovers(&dir));
        // The temporary file is a dotfile, so a reload never reads it as a profile file.
        let seen = std::cell::RefCell::new(String::new());
        let fs = Fs { write_new: &|t: &Path, b: &[u8]| { *seen.borrow_mut() = t.file_name().unwrap().to_string_lossy().to_string(); std::fs::write(t, b) }, ..real() };
        assert!(matches!(rewrite(&fs, &dir, "a.json", &stamp, "G", &rename_it), Outcome::Wrote));
        assert!(seen.borrow().starts_with('.') && seen.borrow().ends_with(".tandem-tmp"), "{}", seen.borrow());
        // A stale temporary file of an earlier crash does not block the write.
        std::fs::write(dir.join(seen.borrow().as_str()), "stale").unwrap();
        let stamp = stamp_of_file(&p);
        assert!(matches!(rewrite(&real(), &dir, "a.json", &stamp, "G", &rename_it), Outcome::Wrote));
        assert!(leftovers(&dir).is_empty());
    }

    #[test]
    fn app_code_loads_only_through_the_gate() {
        // An ungated load must not be reachable from app code (the ruling): the app form is
        // `load_gated` with the real judges.
        let src = include_str!("dynamic_profiles_app.rs");
        let body = src.split("#[cfg(test)]").next().unwrap();
        assert!(body.contains("load_gated("));
        assert!(body.contains("system_local") && body.contains("system_open"));
        assert!(!body.contains("dynamic_profiles::load("), "an ungated load");
    }
}
