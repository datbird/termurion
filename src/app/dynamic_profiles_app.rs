//! Dynamic Profiles in the app (M13v2 Task 6): the watched folder, what a reload does to the
//! profile list, the error alert, and writing an edit of a rewritable profile back to its file
//! (`iTermDynamicProfileManager.m`). The pure parts (`apply_loaded`, `rewrite`) take what they
//! touch as arguments; the `App` methods wire them to the store, the scanner and the console.

use std::collections::{BTreeSet, HashMap};
use std::path::{Component, Path, PathBuf};
use std::time::Instant;

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

/// What a file looked like when it was read: its bytes (size and an FNV-1a hash), its last write
/// time and its identity (volume and file index), all from the one handle the loader opened it
/// with. A write-back goes ahead only when the file still matches.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Stamp {
    len: usize,
    hash: u64,
    stat: dp::Stat,
}

impl Stamp {
    pub(super) fn new(bytes: &[u8], stat: dp::Stat) -> Stamp {
        let hash = bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ u64::from(*b)).wrapping_mul(0x0000_0100_0000_01b3));
        Stamp { len: bytes.len(), hash, stat: dp::Stat { links: 0, ..stat } }
    }
}

/// What a write-back touches, so the tests can stand in for the disk. Every judgment of an object
/// comes from `open`, the loader's own one no-follow handle (`dynamic_profiles_trust::open`): a
/// link, a second name, another owner, a writer that is not the person and the Mark of the Web
/// are refused the same way in both.
pub(super) struct Fs<'a> {
    pub local: &'a dyn Fn(&Path) -> bool,
    pub open: &'a dyn Fn(&Path) -> Result<dp::Opened, String>,
    /// Makes a new file (it must not exist) holding the bytes.
    pub write_new: &'a dyn Fn(&Path, &[u8]) -> std::io::Result<()>,
    /// Renames the new file `tmp` over `target`, both bare names in `folder`, through a handle on
    /// the folder that must be the one judged (`id`: volume serial and file index), not a path that
    /// could be swapped under the call.
    pub rename: &'a dyn Fn(&Path, Option<(u32, u64)>, &str, &str) -> std::io::Result<()>,
}

fn write_new(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let path = crate::localfs::judge_io(path)?;
    let mut f = crate::localfs::open_custom(&path, |o| {
        o.write(true).create_new(true);
    })?;
    f.write_all(bytes)?;
    f.sync_all()
}

/// The handle-based rename: the folder is opened without following a link and without the
/// delete share right, so it cannot be renamed or replaced while the call runs; the new file is
/// opened for DELETE (no link followed) and renamed with `FileRenameInfo` to `target` in that
/// folder, replacing the old file. The folder must still be the judged one (`judged`: volume serial
/// and file index). The target is the bare file name joined to the path the folder's own handle
/// reports (links resolved), so the destination is never resolved again from the textual path.
#[cfg(windows)]
fn rename_in_folder(folder: &Path, judged: Option<(u32, u64)>, tmp: &str, target: &str) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::Storage::FileSystem::{BY_HANDLE_FILE_INFORMATION, DELETE, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, FileRenameInfo, GetFileInformationByHandle, GetFinalPathNameByHandleW, SetFileInformationByHandle};
    // The folder through 13v1's one path judge, then opened as itself (no link followed).
    let folder = crate::localfs::judge_io(folder)?;
    let dir = crate::localfs::open_unfollowed(&crate::localfs::Unfollowed::from(folder.clone()), |o| {
        o.access_mode(FILE_READ_ATTRIBUTES).share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE);
    })?;
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: the handle is open for the call and `info` is a plain out structure.
    if unsafe { GetFileInformationByHandle(dir.as_raw_handle() as HANDLE, &mut info) } == 0 {
        return Err(std::io::Error::last_os_error());
    }
    if info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 || info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY == 0 {
        return Err(std::io::Error::other("the folder is a link"));
    }
    // The folder reached by this path now is the folder that was judged, not one swapped in since.
    let now = (info.dwVolumeSerialNumber, (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow));
    // An unknown identity is a refusal too: the folder cannot be shown to be the judged one.
    if judged != Some(now) {
        return Err(std::io::Error::other("the folder is not the one that was judged"));
    }
    let tmp = folder.unfollowed(tmp).ok_or_else(crate::localfs::refused)?;
    let new = crate::localfs::open_unfollowed(&tmp, |o| {
        o.access_mode(DELETE).share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE);
    })?;
    // FILE_RENAME_INFO: ReplaceIfExists (4 bytes, padded to 8), RootDirectory (8, none), FileNameLength
    // (4, in bytes), then the name.
    // The folder's own path as the HANDLE reports it (links resolved), then the bare file name: a
    // bare name alone is resolved against the process's current folder by the call.
    let mut base = vec![0u16; 4096];
    // SAFETY: the handle is open; the buffer length is passed.
    let n = unsafe { GetFinalPathNameByHandleW(dir.as_raw_handle() as HANDLE, base.as_mut_ptr(), base.len() as u32, 0) } as usize;
    if n == 0 || n >= base.len() {
        return Err(std::io::Error::other("the folder's path could not be read from its handle"));
    }
    base.truncate(n);
    if base.last() != Some(&(b'\\' as u16)) {
        base.push(b'\\' as u16);
    }
    let name: Vec<u16> = base.into_iter().chain(std::ffi::OsStr::new(target).encode_wide()).collect();
    let mut buf = vec![0u8; 24 + name.len() * 2];
    buf[0] = 1;
    buf[16..20].copy_from_slice(&((name.len() * 2) as u32).to_le_bytes());
    for (i, u) in name.iter().enumerate() {
        buf[20 + i * 2..22 + i * 2].copy_from_slice(&u.to_le_bytes());
    }
    // SAFETY: the handle is open; the buffer is a FILE_RENAME_INFO of the size passed.
    let ok = unsafe { SetFileInformationByHandle(new.as_raw_handle() as HANDLE, FileRenameInfo, buf.as_ptr().cast(), buf.len() as u32) };
    drop(dir);
    if ok == 0 { Err(std::io::Error::last_os_error()) } else { Ok(()) }
}

#[cfg(not(windows))]
fn rename_in_folder(folder: &Path, _judged: Option<(u32, u64)>, tmp: &str, target: &str) -> std::io::Result<()> {
    let folder = crate::localfs::judge_io(folder)?;
    let (Some(from), Some(to)) = (folder.child(tmp), folder.child(target)) else { return Err(crate::localfs::refused()) };
    crate::localfs::rename(&from, &to)
}

/// Removes a temporary file the write-back made (judged first, as every path).
fn remove_tmp(tmp: &Path) {
    if let Some(t) = crate::localfs::judge(tmp) {
        let _ = crate::localfs::remove_file(&t);
    }
}

impl Fs<'static> {
    pub(super) fn real() -> Fs<'static> {
        Fs { local: &dp::system_local, open: &dp::system_open, write_new: &write_new, rename: &rename_in_folder }
    }
}

#[derive(Debug)]
pub(super) enum Outcome {
    /// Written; the file's stamp as it is now.
    Wrote(Option<Stamp>),
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
/// `folder`, then the folder, every folder below it and the file are each opened once through
/// the loader's no-follow handle (`fs.open`): a link, a second name, an owner or writer that is
/// not the person, the Mark of the Web, a path the network judge refuses, or a file whose bytes,
/// write time or identity no longer match `stamp` is never written through. The bytes go to a
/// hidden temporary file in the same folder (made new), then rename over the original by a handle
/// on that folder, so a failure leaves the original whole. Gives the file's new stamp.
pub(super) fn rewrite(fs: &Fs, folder: &Path, name: &str, stamp: &Stamp, guid: &str, change: &dyn Fn(&mut Entry)) -> Outcome {
    let Some(rel) = below(folder, name) else { return Outcome::Refused("its file is not inside the Dynamic Profiles folder".into()) };
    let path = folder.join(&rel);
    if !(fs.local)(&path) {
        return Outcome::Refused("its file is on a network path or reached through a link to one".into());
    }
    let trusted_folder = |p: &Path| match (fs.open)(p) {
        Ok(o) if o.dir && o.trust.is_ok() => Ok(o.stat.id),
        Ok(o) if o.dir => Err(o.trust.err().unwrap_or_default()),
        Ok(_) => Err("it is not a folder".to_string()),
        Err(e) => Err(e),
    };
    let mut at = folder.to_path_buf();
    // The identity of the folder judged last is the one the rename must find again.
    let mut parent_id = match trusted_folder(&at) {
        Ok(id) => id,
        Err(e) => return Outcome::Refused(format!("the Dynamic Profiles folder is not trusted: {e}")),
    };
    let parts: Vec<_> = rel.components().collect();
    for part in &parts[..parts.len() - 1] {
        at.push(part);
        match trusted_folder(&at) {
            Ok(id) => parent_id = id,
            Err(e) => return Outcome::Refused(format!("its folder is not trusted: {e}")),
        }
    }
    let opened = match (fs.open)(&path) {
        Ok(o) => o,
        Err(e) => return Outcome::Refused(format!("its file could not be opened: {e}")),
    };
    if opened.dir {
        return Outcome::Refused("its file is a folder".into());
    }
    if let Err(e) = &opened.trust {
        return Outcome::Refused(format!("its file is not trusted: {e}"));
    }
    if opened.stat.links > 1 {
        return Outcome::Refused("its file has more than one name (a hard link)".into());
    }
    let bytes = match opened.bytes {
        Ok(b) => b,
        Err(e) => return Outcome::Refused(format!("its file could not be read: {e}")),
    };
    if Stamp::new(&bytes, opened.stat) != *stamp {
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
    let (Some(file_name), Some(parent)) = (path.file_name().map(|n| n.to_string_lossy().to_string()), path.parent()) else { return Outcome::Refused("its file has no name".into()) };
    let tmp_name = format!(".{file_name}.termurion-tmp");
    let tmp = parent.join(&tmp_name);
    let mut made = (fs.write_new)(&tmp, &out);
    if matches!(&made, Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists) {
        // Left by a crash: ours alone (a dotfile with this suffix).
        remove_tmp(&tmp);
        made = (fs.write_new)(&tmp, &out);
    }
    if let Err(e) = made {
        remove_tmp(&tmp);
        return Outcome::Refused(format!("could not write: {e}"));
    }
    if let Err(e) = (fs.rename)(parent, parent_id, &tmp_name, &file_name) {
        remove_tmp(&tmp);
        return Outcome::Refused(format!("could not replace the file: {e}"));
    }
    // What the file is now, for the next write-back (a second profile in the same file).
    let now = (fs.open)(&path).ok().and_then(|o| o.bytes.ok().map(|b| Stamp::new(&b, o.stat)));
    Outcome::Wrote(now)
}

// ---- The app ---------------------------------------------------------------------------------

/// A file's text as the loader names it, with what it looked like when read.
#[derive(Clone, Debug)]
pub(super) struct Source {
    path: String,
    stamp: Option<Stamp>,
}

/// Rewrites the file profile `guid` came from with `change`. A written file's new stamp goes to
/// every profile of that file (a second edit in the same file must not find it "changed"). None
/// when the profile has no recorded file or stamp.
pub(super) fn rewrite_source(fs: &Fs, folder: &Path, files: &mut HashMap<String, Source>, guid: &str, change: &dyn Fn(&mut Entry)) -> Option<Outcome> {
    let source = files.get(guid)?.clone();
    let Some(stamp) = source.stamp.clone() else {
        // The write before this one left the file in a state that could not be read again: say so
        // (the edit is not written, and never silently dropped).
        return Some(Outcome::Refused("its file's state after the last write could not be read".into()));
    };
    let out = rewrite(fs, folder, &source.path, &stamp, guid, change);
    if let Outcome::Wrote(now) = &out {
        for other in files.values_mut().filter(|o| o.path == source.path) {
            other.stamp = now.clone();
        }
    }
    Some(out)
}

/// The edits of the rewritable dynamic profile `guid` go back to its file: only the keys that
/// differ from its parent (`DP:676-828`). None when it is not a rewritable dynamic profile.
pub(super) fn write_back(fs: &Fs, folder: &Path, files: &mut HashMap<String, Source>, store: &Store, add_tag: bool, guid: &str) -> Option<Outcome> {
    let profile = store.profiles.iter().find(|p| p.guid() == guid && is_dynamic(p) && p.bool(dp::REWRITABLE_KEY))?.clone();
    let (profiles, default) = (&store.profiles, store.default_profile().clone());
    let change = |e: &mut Entry| {
        let others = || profiles.iter().filter(|p| p.guid() != guid);
        let parent = match dp::parent_of(e, &|g| others().any(|p| p.guid() == g), &|n| others().find(|p| p.name() == n).map(|p| p.guid())) {
            Parent::Found(g) => others().find(|p| p.guid() == g).cloned(),
            _ => None,
        }
        .unwrap_or_else(|| default.clone());
        *e = dp::rewritten(e, &profile, &parent, add_tag);
    };
    rewrite_source(fs, folder, files, guid, &change)
}

/// Writes every pending profile back, one after the other, and never reloads in between: a reload
/// would replace a profile still waiting with the file's old values (B7, "never lose edits").
pub(super) fn flush_writes(fs: &Fs, folder: &Path, files: &mut HashMap<String, Source>, store: &Store, add_tag: bool, pending: BTreeSet<String>) -> Vec<(String, Outcome)> {
    pending.into_iter().filter_map(|g| write_back(fs, folder, files, store, add_tag, &g).map(|o| (g, o))).collect()
}

/// Whether the tick reloads: the watch asked, or an edit's write-back fell due (the reload writes
/// every pending edit first, so a watch reload never reverts one).
pub(super) fn reload_due(asked: bool, due: Option<Instant>, now: Instant) -> bool {
    asked || due.is_some_and(|t| now >= t)
}

impl App {
    /// At launch, before the first window (an arrangement may name a dynamic profile): the
    /// folder is made, watched, and read.
    pub(super) fn dynamic_profiles_start(&mut self) {
        let appdata = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
        let home = std::env::var_os("USERPROFILE").map(PathBuf::from).unwrap_or_default();
        let folder = dp::folder(&self.globals.string("DynamicProfilesPath"), &appdata, &home);
        // Never made on a network path; the reload then reports it.
        if let Some(judged) = crate::localfs::judge(&folder) {
            let _ = crate::localfs::create_dir_all(&judged);
        }
        let latency = self.globals.number("DynamicProfilesNotificationLatency").filter(|v| v.is_finite()).unwrap_or(0.1).clamp(0.01, 60.0);
        self.dynamic_scan = Some(crate::dynamic_profiles_scan::Scanner::start(folder, std::time::Duration::from_secs_f64(latency), self.notify.clone()));
        self.reload_dynamic_profiles();
    }

    /// Each loop pass: a reload the watch asked for or an edit's write-back fell due (the reload
    /// writes the pending edits first). Gives when a write-back falls due (nothing while nothing waits).
    pub(super) fn dynamic_profiles_tick(&mut self) -> Option<Instant> {
        let asked = self.dynamic_scan.as_ref().is_some_and(|s| s.take_reload());
        if reload_due(asked, self.dynamic_write_due, Instant::now()) {
            self.reload_dynamic_profiles();
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
        // Edits still waiting for their write-back go to the files first: the reload would
        // otherwise replace them with what the files say.
        self.flush_dynamic_writes();
        let add_tag = self.globals.bool("AddDynamicTagToDynamicProfiles");
        let stamps: std::cell::RefCell<HashMap<String, Stamp>> = Default::default();
        let loaded = {
            let store = &self.store;
            // The one open of each object answers its type, its verdict and its bytes; a file's
            // bytes are stamped here for a later write-back.
            let open = |p: &Path| {
                let o = dp::system_open(p)?;
                if let Ok(bytes) = &o.bytes {
                    stamps.borrow_mut().insert(p.display().to_string(), Stamp::new(bytes, o.stat));
                }
                Ok(o)
            };
            let list = |f: &Path| crate::localfs::judge(f).map(|f| dp::list_dir(&f, &dp::system_local, dp::MAX_ENTRIES)).unwrap_or_default();
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
        let reveal = Some(PathBuf::from(&errors[0].0)).filter(|p| !errors[0].0.is_empty() && crate::localfs::is_local_file(p));
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

    /// Writes every pending edit back (no reload) and logs what was not written.
    pub(super) fn flush_dynamic_writes(&mut self) {
        self.dynamic_write_due = None;
        let pending = std::mem::take(&mut self.dynamic_write);
        let Some(folder) = self.dynamic_scan.as_ref().map(|s| s.folder().to_path_buf()).filter(|_| !pending.is_empty()) else { return };
        let add_tag = self.globals.bool("AddDynamicTagToDynamicProfiles");
        let asked = pending.clone();
        let results = flush_writes(&Fs::real(), &folder, &mut self.dynamic_files, &self.store, add_tag, pending);
        // An edit for a profile that is no longer in the list or has no file is never silent.
        let lost: Vec<String> = asked.into_iter().filter(|g| !results.iter().any(|(r, _)| r == g)).collect();
        for guid in lost {
            self.dynamic_log(&format!("An edit to the dynamic profile {guid} could not be written back: its profile or file is gone."));
        }
        for (guid, out) in results {
            self.dynamic_outcome(&guid, out);
        }
    }

    /// "Mark as Rewritable": writes `Rewritable` true into the profile's own entry, then reloads.
    pub(super) fn mark_rewritable(&mut self, guid: &str) {
        let Some(folder) = self.dynamic_scan.as_ref().map(|s| s.folder().to_path_buf()) else { return };
        let change = |e: &mut Entry| {
            e.insert(dp::REWRITABLE_KEY.into(), Json::Bool(true));
        };
        if let Some(out) = rewrite_source(&Fs::real(), &folder, &mut self.dynamic_files, guid, &change) {
            self.dynamic_outcome(guid, out);
        }
        // One reload: the file's own state is what the profile shows.
        self.reload_dynamic_profiles();
    }

    fn dynamic_outcome(&mut self, guid: &str, out: Outcome) {
        let name = self.store.profiles.iter().find(|p| p.guid() == guid).map(|p| crate::profile_list::display_text(&p.name())).unwrap_or_default();
        match out {
            Outcome::Wrote(_) => {}
            Outcome::Changed => self.dynamic_log(&format!("Dynamic profile {name} changed on disk, not rewritten.")),
            Outcome::Refused(why) => {
                let line = format!("Dynamic profile {name} was not rewritten: {why}.");
                self.dynamic_log(&line);
                self.banners.set("dynamic", line);
            }
        }
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
    /// Final review 7: the rename refuses a folder whose judged identity is unknown.
    #[test]
    fn the_rename_refuses_an_unknown_judged_folder() {
        let src = include_str!("dynamic_profiles_app.rs");
        let at = src.find("fn rename_in_folder(folder: &Path, judged").unwrap();
        let body = &src[at..at + src[at..].find("\n}\n").unwrap()];
        assert!(body.contains("judged != Some(now)"));
        assert!(!body.contains("judged.is_some_and"));
    }

    use super::*;
    use crate::dynamic_profiles::{Entry, FileType, Loaded};
    use crate::profile::Profile;
    use crate::store::Store;
    use serde_json::json;
    use std::path::{Path, PathBuf};

    fn entry(v: serde_json::Value) -> Entry {
        v.as_object().expect("an object").clone()
    }

    /// A fresh folder under %APPDATA% (the temp folder is writable by other users, which the
    /// loader's own judge refuses, so the real handle is only happy here). Folders of earlier runs
    /// (another process id, more than an hour old) are removed once per run.
    fn scratch(tag: &str) -> PathBuf {
        static SWEEP: std::sync::Once = std::sync::Once::new();
        let base = std::env::var_os("APPDATA").map_or_else(std::env::temp_dir, PathBuf::from);
        SWEEP.call_once(|| {
            for e in std::fs::read_dir(&base).into_iter().flatten().flatten() {
                let old = e.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age.as_secs() > 3600);
                if old && e.file_name().to_string_lossy().starts_with("termurion-dynapp-") {
                    let _ = std::fs::remove_dir_all(e.path());
                }
            }
        });
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
        let dir = base.join(format!("termurion-dynapp-{tag}-{}-{nanos}", std::process::id()));
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

    /// The stamp the loader would record: one open of the file through the loader's handle.
    fn stamp_of_file(p: &Path) -> Stamp {
        let o = dp::system_open(p).unwrap();
        Stamp::new(&o.bytes.unwrap(), o.stat)
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
        std::fs::read_dir(dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().to_string()).filter(|n| n.contains("termurion-tmp")).collect()
    }

    #[test]
    fn write_back_writes_one_entry_and_keeps_the_rest() {
        let dir = scratch("wb");
        let p = write_json(&dir, "a.json", json!({"Profiles": [{"Guid": "G", "Name": "Old"}, {"Guid": "H", "Name": "Other"}]}));
        let out = rewrite(&real(), &dir, &p.display().to_string(), &stamp_of_file(&p), "G", &rename_it);
        assert!(matches!(out, Outcome::Wrote(_)), "{out:?}");
        assert_eq!(names(&p), ["Renamed", "Other"]);
        assert!(leftovers(&dir).is_empty(), "{:?}", leftovers(&dir));
        // The same name relative to the folder works too.
        let again = rewrite(&real(), &dir, "a.json", &stamp_of_file(&p), "H", &|e| {
            e.insert("Name".into(), json!("H2"));
        });
        assert!(matches!(again, Outcome::Wrote(_)), "{again:?}");
        assert_eq!(names(&p), ["Renamed", "H2"]);
        // A property list keeps its type.
        let plist = crate::dynamic_profiles::write_file(vec![entry(json!({"Guid": "P", "Name": "Pl"}))], FileType::PropertyList);
        let pp = dir.join("b.plist");
        std::fs::write(&pp, &plist).unwrap();
        let out = rewrite(&real(), &dir, "b.plist", &stamp_of_file(&pp), "P", &rename_it);
        assert!(matches!(out, Outcome::Wrote(_)), "{out:?}");
        let (list, kind) = crate::dynamic_profiles::raw_profiles(&std::fs::read(&pp).unwrap()).unwrap();
        assert_eq!(kind, FileType::PropertyList);
        assert_eq!(list[0]["Name"], "Renamed");
    }

    /// An `open` that answers as the real handle does, except for the objects `tweak` changes.
    fn open_with(tweak: impl Fn(&Path, &mut dp::Opened) + 'static) -> impl Fn(&Path) -> Result<dp::Opened, String> {
        move |p: &Path| {
            let mut o = dp::system_open(p)?;
            tweak(p, &mut o);
            Ok(o)
        }
    }

    #[test]
    fn write_back_refuses_a_link() {
        let dir = scratch("link");
        let p = write_json(&dir, "a.json", json!({"Profiles": [{"Guid": "G", "Name": "Old"}]}));
        let before = std::fs::read(&p).unwrap();
        let stamp = stamp_of_file(&p);
        // The file itself is a link: the handle says so (trust is the loader's verdict).
        let itself = p.clone();
        let open = open_with(move |q, o| {
            if q == itself {
                o.trust = Err("it is a link (a symbolic link or junction), which Tandem does not follow".into());
            }
        });
        let fs = Fs { open: &open, ..real() };
        assert!(matches!(rewrite(&fs, &dir, "a.json", &stamp, "G", &rename_it), Outcome::Refused(_)));
        // A folder above it, inside the Dynamic Profiles folder, is one.
        std::fs::create_dir(dir.join("sub")).unwrap();
        let q = write_json(&dir.join("sub"), "b.json", json!({"Profiles": [{"Guid": "S", "Name": "S"}]}));
        let sub = dir.join("sub");
        let open = open_with(move |q, o| {
            if q == sub {
                o.trust = Err("it is a link".into());
            }
        });
        let fs = Fs { open: &open, ..real() };
        assert!(matches!(rewrite(&fs, &dir, "sub/b.json", &stamp_of_file(&q), "S", &rename_it), Outcome::Refused(_)));
        // The Dynamic Profiles folder itself is one: the root is judged too.
        let root = dir.clone();
        let open = open_with(move |q, o| {
            if q == root {
                o.trust = Err("it is a link".into());
            }
        });
        let fs = Fs { open: &open, ..real() };
        assert!(matches!(rewrite(&fs, &dir, "a.json", &stamp, "G", &rename_it), Outcome::Refused(m) if m.contains("folder")));
        // A hard link to another name is refused too.
        let open = open_with(|_, o| {
            if !o.dir {
                o.stat.links = 2;
            }
        });
        let fs = Fs { open: &open, ..real() };
        assert!(matches!(rewrite(&fs, &dir, "a.json", &stamp, "G", &rename_it), Outcome::Refused(_)));
        // Another owner, another writer or the Mark of the Web: the handle's verdict is final.
        let open = open_with(|_, o| {
            if !o.dir {
                o.trust = Err("writable by other users (S-1-1-0 may write)".into());
            }
        });
        let fs = Fs { open: &open, ..real() };
        assert!(matches!(rewrite(&fs, &dir, "a.json", &stamp, "G", &rename_it), Outcome::Refused(m) if m.contains("writable by other users")));
        assert_eq!(std::fs::read(&p).unwrap(), before, "nothing was written through a link");
        assert!(leftovers(&dir).is_empty());
    }

    /// A file replaced by another with the same bytes (a different identity) or touched since the
    /// load is not the one that was read.
    #[test]
    fn write_back_refuses_a_swapped_identity() {
        let dir = scratch("identity");
        let p = write_json(&dir, "a.json", json!({"Profiles": [{"Guid": "G", "Name": "Old"}]}));
        let stamp = stamp_of_file(&p);
        let open = open_with(|_, o| {
            if !o.dir {
                o.stat.id = Some((9, 9));
            }
        });
        let fs = Fs { open: &open, ..real() };
        assert!(matches!(rewrite(&fs, &dir, "a.json", &stamp, "G", &rename_it), Outcome::Changed));
        let open = open_with(|_, o| {
            if !o.dir {
                o.stat.mtime = Some(std::time::UNIX_EPOCH);
            }
        });
        let fs = Fs { open: &open, ..real() };
        assert!(matches!(rewrite(&fs, &dir, "a.json", &stamp, "G", &rename_it), Outcome::Changed));
        assert!(matches!(rewrite(&real(), &dir, "a.json", &stamp, "G", &rename_it), Outcome::Wrote(_)), "the real file still matches");
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
        let stamp = Stamp::new(b"x", dp::Stat::default());
        let never = Fs { open: &|_| panic!("opened outside the folder"), write_new: &|_, _| panic!("wrote outside the folder"), rename: &|_, _, _, _| panic!("renamed outside the folder"), ..real() };
        for name in ["..\\x.json", "..\\..\\x.json", "C:\\x.json", "\\\\h\\s\\x.json", "sub\\..\\..\\x.json", "\\x.json", "", ".", "C:x.json"] {
            assert!(matches!(rewrite(&never, &dir, name, &stamp, "G", &rename_it), Outcome::Refused(_)), "{name:?}");
        }
        // An absolute path inside another folder is outside this one.
        let other = scratch("stay-other");
        assert!(matches!(rewrite(&never, &dir, &other.join("a.json").display().to_string(), &stamp, "G", &rename_it), Outcome::Refused(_)));
        // The network judge has the last word on the path.
        let fs = Fs { local: &|_| false, open: &|_| panic!("opened"), ..real() };
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
        let fs = Fs { rename: &|_, _, _, _| Err(std::io::Error::other("locked")), ..real() };
        assert!(matches!(rewrite(&fs, &dir, "a.json", &stamp, "G", &rename_it), Outcome::Refused(_)));
        assert_eq!(std::fs::read(&p).unwrap(), before);
        assert!(leftovers(&dir).is_empty(), "{:?}", leftovers(&dir));
        // The temporary file is a dotfile, so a reload never reads it as a profile file.
        let seen = std::cell::RefCell::new(String::new());
        let fs = Fs { write_new: &|t: &Path, b: &[u8]| { *seen.borrow_mut() = t.file_name().unwrap().to_string_lossy().to_string(); std::fs::write(t, b) }, ..real() };
        assert!(matches!(rewrite(&fs, &dir, "a.json", &stamp, "G", &rename_it), Outcome::Wrote(_)));
        assert!(seen.borrow().starts_with('.') && seen.borrow().ends_with(".termurion-tmp"), "{}", seen.borrow());
        // A stale temporary file of an earlier crash does not block the write.
        std::fs::write(dir.join(seen.borrow().as_str()), "stale").unwrap();
        let stamp = stamp_of_file(&p);
        assert!(matches!(rewrite(&real(), &dir, "a.json", &stamp, "G", &rename_it), Outcome::Wrote(_)));
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

    /// 13v2 fix r1 item 2 (class): the write-back judges with the loader's one no-follow handle and
    /// nothing else: no path-based link, size, time or read probe is left in it.
    #[test]
    fn write_back_uses_the_loaders_handle() {
        let src = include_str!("dynamic_profiles_app.rs");
        let body = src.split("#[cfg(test)]").next().unwrap();
        assert!(body.contains("system_open"));
        for old in ["bounded_read(", "link_count", "read_link", "mtime_of", "std::fs::metadata", "File::open("] {
            assert!(!body.contains(old), "the write-back still probes by path: {old}");
        }
        let rewrite = body.split("pub(super) fn rewrite(").nth(1).unwrap().split("// ---- The app").next().unwrap();
        assert!(rewrite.contains("fs.open") && rewrite.contains("fs.rename"));
    }

    // ---- pending edits are never reverted (B7, fix r1 item 1) ---------------------------------

    /// A rewritable file of two profiles, a store that loaded it, and the loader's record of it.
    fn two_in_one_file(tag: &str) -> (PathBuf, PathBuf, Store, HashMap<String, Source>) {
        let dir = scratch(tag);
        let p = write_json(&dir, "a.json", json!({"Profiles": [{"Guid": "A", "Name": "A", "Rewritable": true}, {"Guid": "B", "Name": "B", "Rewritable": true}]}));
        let mut s = store();
        let read = |q: &Path| std::fs::read(q).map_err(|e| e.to_string());
        let list = |f: &Path| crate::localfs::judge(f).map(|f| crate::dynamic_profiles::list_dir(&f, &|_| true, 100)).unwrap_or_default();
        apply_loaded(&mut s, &crate::dynamic_profiles::load(&dir, &read, &list, &|_| false), false);
        let stamp = stamp_of_file(&p);
        let files = ["A", "B"].iter().map(|g| (g.to_string(), Source { path: p.display().to_string(), stamp: Some(stamp.clone()) })).collect();
        (dir, p, s, files)
    }

    fn rename_in_store(s: &mut Store, guid: &str, name: &str) {
        s.profiles.iter_mut().find(|p| p.guid() == guid).unwrap().set("Name", toml::Value::String(name.into()));
    }

    /// Two edits waiting in one file are both written (the first write refreshes the stamp the
    /// second needs), and a reload after the flush keeps both; reloading first would revert them.
    #[test]
    fn two_pending_edits_in_one_file_both_land() {
        let (dir, p, mut s, mut files) = two_in_one_file("pending");
        rename_in_store(&mut s, "A", "A2");
        rename_in_store(&mut s, "B", "B2");
        let pending: BTreeSet<String> = ["A", "B"].iter().map(|g| g.to_string()).collect();
        let out = flush_writes(&real(), &dir, &mut files, &s, false, pending);
        assert_eq!(out.len(), 2);
        assert!(out.iter().all(|(_, o)| matches!(o, Outcome::Wrote(_))), "{out:?}");
        assert_eq!(names(&p), ["A2", "B2"]);
        // The reload after the flush reads what was written and changes nothing.
        let read = |q: &Path| std::fs::read(q).map_err(|e| e.to_string());
        let list = |f: &Path| crate::localfs::judge(f).map(|f| crate::dynamic_profiles::list_dir(&f, &|_| true, 100)).unwrap_or_default();
        let again = crate::dynamic_profiles::load(&dir, &read, &list, &|_| false);
        let (changes, _) = apply_loaded(&mut s, &again, false);
        assert!(changes.update.is_empty(), "{changes:?}");
        assert_eq!((s.get("A").unwrap().name(), s.get("B").unwrap().name()), ("A2".to_string(), "B2".to_string()));
        // The old order: a reload first reverts both edits.
        let (dir, _, mut s, files) = two_in_one_file("pending-old");
        rename_in_store(&mut s, "A", "A2");
        rename_in_store(&mut s, "B", "B2");
        let stale = crate::dynamic_profiles::load(&dir, &read, &list, &|_| false);
        apply_loaded(&mut s, &stale, false);
        assert_eq!(s.get("A").unwrap().name(), "A", "what the reload does to an unflushed edit");
        drop(files);
    }

    /// A missing stamp after a write is reported for the next pending edit of that file, never
    /// dropped without a word.
    #[test]
    fn a_missing_stamp_is_reported_not_dropped() {
        let (dir, _p, s, mut files) = two_in_one_file("nostamp");
        for src in files.values_mut() {
            src.stamp = None;
        }
        let pending: BTreeSet<String> = ["A", "B"].iter().map(|g| g.to_string()).collect();
        let out = flush_writes(&real(), &dir, &mut files, &s, false, pending);
        assert_eq!(out.len(), 2, "every pending edit has an outcome: {out:?}");
        assert!(out.iter().all(|(_, o)| matches!(o, Outcome::Refused(_))), "{out:?}");
    }

    /// m13v2 Task 6 fix: the rename names the folder judged (volume serial and file index), asks the
    /// handle for the bare file name, and refuses a folder that is not the judged one.
    #[test]
    fn rename_names_the_judged_folder() {
        let dir = scratch("judged");
        let other = scratch("judged-other");
        let id_of = |d: &Path| dp::system_open(d).unwrap().stat.id;
        let (a, b) = (id_of(&dir), id_of(&other));
        assert!(a.is_some() && a != b);
        std::fs::write(dir.join("t.tmp"), "new").unwrap();
        std::fs::write(dir.join("a.json"), "old").unwrap();
        let real_rename = Fs::real().rename;
        assert!(real_rename(&dir, b, "t.tmp", "a.json").is_err(), "a folder that is not the judged one");
        assert_eq!(std::fs::read_to_string(dir.join("a.json")).unwrap(), "old");
        real_rename(&dir, a, "t.tmp", "a.json").unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("a.json")).unwrap(), "new");
        // rewrite hands the rename the judged folder's identity and the bare names.
        let p = write_json(&dir, "c.json", json!({"Profiles": [{"Guid": "G", "Name": "Old"}]}));
        let seen = std::cell::RefCell::new(None);
        let fs = Fs { rename: &|f: &Path, id, t: &str, n: &str| { *seen.borrow_mut() = Some((id, t.to_string(), n.to_string())); std::fs::rename(f.join(t), f.join(n)) }, ..real() };
        assert!(matches!(rewrite(&fs, &dir, "c.json", &stamp_of_file(&p), "G", &rename_it), Outcome::Wrote(_)));
        let (id, tmp, target) = seen.borrow().clone().unwrap();
        assert_eq!((id, target.as_str()), (a, "c.json"));
        assert!(tmp.starts_with('.') && !tmp.contains(['\\', '/']));
    }

    #[test]
    fn the_tick_reloads_when_asked_or_due() {
        let now = Instant::now();
        let (past, future) = (now - std::time::Duration::from_secs(1), now + std::time::Duration::from_secs(60));
        assert!(reload_due(true, None, now));
        assert!(reload_due(true, Some(future), now), "a watch reload also writes what waits");
        assert!(reload_due(false, Some(past), now));
        assert!(!reload_due(false, Some(future), now));
        assert!(!reload_due(false, None, now));
    }

    /// Every reload writes the pending edits first (before it reads a file), and the tick never
    /// writes on its own: the write is part of the reload.
    #[test]
    fn a_reload_flushes_before_it_reads() {
        let body = include_str!("dynamic_profiles_app.rs").split("#[cfg(test)]").next().unwrap();
        let reload = body.split("pub(super) fn reload_dynamic_profiles(").nth(1).unwrap();
        let (flush, read) = (reload.find("self.flush_dynamic_writes()").expect("flush"), reload.find("load_gated(").expect("load"));
        assert!(flush < read, "the pending edits are written before the folder is read");
        let tick = body.split("pub(super) fn dynamic_profiles_tick(").nth(1).unwrap().split("fn dynamic_log").next().unwrap();
        assert!(!tick.contains("flush_writes") && !tick.contains("write_back"), "the tick writes only through a reload");
    }
}
