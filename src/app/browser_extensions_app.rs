//! Browser extensions on the app side (milestone 13x8, Task 10). WebView2 loads an extension from a
//! folder on disk, so everything here is about WHICH folders it may be told about:
//!
//! - A folder is only ever one the person chose in the native folder picker (`browser_extensions_choose`
//!   writes the root key) and only after the person approved THAT folder's manifest, for THIS saved
//!   profile, in a native Termurion window that lists the manifest's permissions and sites
//!   (`Then::BrowserExtension`, the guarded `ConfirmWindow`). The approval is bound to a hash of the
//!   manifest, so a changed manifest asks again. An import, a sync, a page or a URL can write the profile
//!   keys but cannot make an approval: the approvals file is written only by `approve_in_worker`.
//! - Nothing here runs for a Dev Null pane (`Store::BrowserDevNull` gets no extension support at all).
//! - The sync runs when a browser view is created, when the settings page asks, and after each add. It
//!   never asks a question; only choosing a folder does.
//! - Folders and manifests are read on workers (never on the UI thread), bounded, and never from a
//!   network path.
//! - One owner removes an extension: `browser_extension_remove` asks WebView2 and drops Termurion's records
//!   (approval id, active id, this run's folder map) together.
//! Nothing is logged but counts; names and paths never reach a log line. App-wide state only (never in
//! `swap_live`): the persistent store is the one extension profile.

use std::collections::{HashMap, HashSet, VecDeque};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::mpsc::{Receiver, Sender, channel};

use super::*;
use crate::browser_extensions::{self as be, Approvals, Manifest};
use crate::localfs::{self, LocalPath};
use crate::webhost::Store;
use crate::webhost_win;

const FILE: &str = "browser-extensions.json";
/// The approvals file is read, changed and written by one worker at a time.
static FILE_LOCK: Mutex<()> = Mutex::new(());
/// Questions waiting for the person, at most (a folder of hostile manifests cannot queue more).
const MAX_ASKS: usize = 8;

/// One folder the scan looked at.
#[derive(Clone)]
struct Found {
    folder: PathBuf,
    key: String,
    manifest: Manifest,
    hash: String,
    /// Approved for the shared browser data: Some(the recorded id, if it was added before).
    approval: Option<Option<String>>,
    /// The shared switch (approvals file).
    enabled: bool,
}

enum Done {
    Scan { pane: PaneId, found: Vec<Found>, readable: bool, choose: bool, report: bool },
    Approved { pane: PaneId, ok: bool },
}

/// A sync waiting for WebView2's answers.
struct Sync {
    pane: PaneId,
    found: Vec<Found>,
    active: Vec<String>,
    report: bool,
    failed: HashSet<String>,
    /// Adds started and not yet completed.
    adds: usize,
}

/// A manifest waiting for the person's question.
struct Candidate {
    pane: PaneId,
    folder: PathBuf,
    manifest: Manifest,
    hash: String,
}

#[derive(Default)]
pub(super) struct Extensions {
    /// This run's (store, folder) to WebView2 id map, filled from each `ExtensionAdded` (the interface
    /// has no folder, review I6).
    ids: HashMap<(Store, String), String>,
    /// Ids the last scan found approved (a toggle from the page is only for these).
    known: HashSet<String>,
    chan: Option<(Sender<Done>, Receiver<Done>)>,
    busy: bool,
    /// A request that arrived while a scan ran: (pane, choose, report), merged.
    queued: Option<(PaneId, bool, bool)>,
    sync: Option<Sync>,
    asks: VecDeque<Candidate>,
    asking: bool,
}

fn data_dir() -> std::io::Result<LocalPath> {
    let base = webhost_win::local_appdata().ok_or_else(localfs::refused)?;
    let dir = localfs::judge_io(base.join("Termurion"))?;
    localfs::create_dir_all(&dir)?;
    Ok(dir)
}

fn load_approvals(dir: &LocalPath) -> Approvals {
    let Some(p) = dir.child(FILE) else { return Approvals::default() };
    match localfs::lstat(&p) {
        Ok(st) if st.is_file && !st.is_reparse && st.len <= be::MAX_APPROVALS_BYTES as u64 => localfs::read_to_string(&p).map(|t| Approvals::parse(&t)).unwrap_or_default(),
        _ => Approvals::default(),
    }
}

fn save_approvals(dir: &LocalPath, a: &Approvals) -> std::io::Result<()> {
    let (tmp, file) = (dir.unfollowed(format!("{FILE}.tmp")), dir.unfollowed(FILE));
    let (Some(tmp), Some(file)) = (tmp, file) else { return Err(localfs::refused()) };
    let _ = localfs::remove_unfollowed(&tmp);
    let mut f = localfs::create_owner_only(&tmp)?;
    f.write_all(a.to_json().as_bytes())?;
    f.sync_all()?;
    drop(f);
    localfs::rename_unfollowed(&tmp, &file)
}

/// Reads, changes and writes the approvals file under the lock (always on a worker).
fn edit_approvals(change: impl FnOnce(&mut Approvals)) -> bool {
    let _guard = FILE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let Ok(dir) = data_dir() else { return false };
    let mut a = load_approvals(&dir);
    change(&mut a);
    save_approvals(&dir, &a).is_ok()
}

/// What the shared browser data should run: every approved folder whose manifest still hashes to what
/// was approved (wherever it is: approvals are for the shared WebView2 profile, not for a Termurion
/// profile), plus the folders under `root` (or `root` itself when it holds a manifest) with their
/// manifests, which may be new. A pane with no root still gets the approved set, so a sync from any
/// pane leaves no orphan running. The bool is whether the root could be read at all. Worker code.
fn scan(root: Option<&Path>) -> (Vec<Found>, bool) {
    let approvals = data_dir().map(|d| load_approvals(&d)).unwrap_or_default();
    let mut dirs: Vec<PathBuf> = Vec::new();
    let mut readable = true;
    if let Some(root) = root {
        match localfs::judge(root).filter(|_| be::root_ok(root)) {
            None => readable = false,
            Some(judged) => {
                if judged.child("manifest.json").is_some_and(|m| localfs::is_file(&m)) {
                    dirs.push(judged.path().to_path_buf());
                } else {
                    match localfs::list_dir(&judged, 1000) {
                        Ok(names) => dirs.extend(be::folders(&names).iter().filter_map(|n| judged.child(n)).map(LocalPath::into_path_buf)),
                        Err(_) => readable = false,
                    }
                }
            }
        }
    }
    if !readable {
        return (Vec::new(), false);
    }
    for a in approvals.list.iter().filter(|a| a.profile == be::STORE) {
        let d = PathBuf::from(&a.folder);
        if !dirs.iter().any(|x| be::folder_key(x) == a.folder) && be::root_ok(&d) {
            dirs.push(d);
        }
    }
    let found = dirs
        .into_iter()
        .filter_map(|d| {
            let (manifest, hash) = be::read_folder_manifest(&d)?;
            let key = be::folder_key(&d);
            let rec = approvals.find(be::STORE, &key, &hash);
            let (approval, enabled) = (rec.map(|a| a.id.clone()), rec.is_none_or(|a| a.enabled));
            Some(Found { folder: d, key, manifest, hash, approval, enabled })
        })
        .collect();
    (found, true)
}

impl App {
    /// An extension is possible only in a persistent browser pane that has a view.
    fn browser_ext_pane_ok(&self, pane: PaneId) -> bool {
        self.panes.get(&pane).and_then(|p| p.browser_io()).is_some_and(|io| !io.dev_null) && self.browser_views.get(&pane).is_some_and(|b| b.view.is_some())
    }

    /// What an approval belongs to: the shared browser data, the same for every Termurion browser profile
    /// (they share one WebView2 profile, so one extension list).
    fn browser_ext_profile(&self, pane: PaneId) -> Option<String> {
        self.panes.get(&pane)?;
        Some(be::STORE.to_string())
    }

    fn browser_ext_value(&self, pane: PaneId, key: &str) -> Option<toml::Value> {
        let guid = self.panes.get(&pane)?.profile.clone();
        self.store.get(&guid)?.get(key)
    }

    /// Writes a profile key to the pane's own copy and to the saved profile (the profile mutator's way),
    /// then saves.
    fn browser_ext_write(&mut self, pane: PaneId, key: &str, value: Option<toml::Value>) {
        self.set_session_values(pane, &[(key.to_string(), value.clone())]);
        let Some(guid) = self.panes.get(&pane).map(|p| p.profile.clone()) else { return };
        let shared = self.store.shared_guid(&guid);
        let written = if let Some(p) = self.store.get_mut(&shared) {
            match value {
                Some(v) => p.set(key, v),
                None => {
                    p.remove(key);
                }
            }
            true
        } else {
            false
        };
        let _ = self.save_store();
        // The Settings window's owner: every open window and pane of that profile sees the change
        // (13x8 final review M5).
        if written {
            self.apply_edit(Edit::Changed(shared));
        }
    }

    /// Looks at the pane's chosen root (if any) and the approved folders on a worker, then syncs WebView2
    /// with what the person approved. `choose`: a folder was just chosen, so unapproved manifests are
    /// asked about (one window at a time). `report`: the settings page wants the list, and always gets
    /// an answer. With no chosen root the sync still runs for the approved set.
    pub(super) fn browser_extensions_request(&mut self, pane: PaneId, choose: bool, report: bool) {
        if !self.browser_ext_pane_ok(pane) {
            if report {
                self.browser_settings_run(pane, vec![be::ui_script("[]")]);
            }
            return;
        }
        let root = self.browser_ext_value(pane, be::ROOT_KEY).and_then(|v| v.as_str().map(str::to_string)).filter(|r| !r.trim().is_empty());
        if std::mem::replace(&mut self.browser_ext.busy, true) {
            let q = self.browser_ext.queued.get_or_insert((pane, false, false));
            *q = (pane, q.1 || choose, q.2 || report);
            return;
        }
        let tx = self.browser_ext.chan.get_or_insert_with(channel).0.clone();
        let notify = self.notify.clone();
        std::thread::spawn(move || {
            let (found, readable) = scan(root.as_deref().map(Path::new));
            let _ = tx.send(Done::Scan { pane, found, readable, choose, report });
            notify();
        });
    }

    /// From the tick: results of the workers.
    pub(super) fn browser_extensions_poll(&mut self) {
        let got: Vec<Done> = self.browser_ext.chan.as_ref().map(|(_, rx)| rx.try_iter().collect()).unwrap_or_default();
        for d in got {
            match d {
                Done::Scan { pane, found, readable, choose, report } => {
                    self.browser_ext.busy = false;
                    self.browser_ext_scanned(pane, found, readable, choose, report);
                    if let Some((p, c, r)) = self.browser_ext.queued.take() {
                        self.browser_extensions_request(p, c, r);
                    }
                }
                Done::Approved { pane, ok } => {
                    if ok {
                        self.browser_extensions_request(pane, false, false);
                    }
                }
            }
        }
    }

    fn browser_ext_scanned(&mut self, pane: PaneId, found: Vec<Found>, readable: bool, choose: bool, report: bool) {
        if !self.browser_ext_pane_ok(pane) {
            return;
        }
        if !readable {
            // An unreadable root changes nothing (a missing drive must not remove what was approved).
            if report {
                self.browser_settings_run(pane, vec![be::ui_script("[]")]);
            }
            return;
        }
        if choose {
            for f in found.iter().filter(|f| f.approval.is_none()) {
                if self.browser_ext.asks.len() < MAX_ASKS && !self.browser_ext.asks.iter().any(|c| c.folder == f.folder && c.hash == f.hash) {
                    self.browser_ext.asks.push_back(Candidate { pane, folder: f.folder.clone(), manifest: f.manifest.clone(), hash: f.hash.clone() });
                }
            }
            self.browser_ext_next_ask();
        }
        for f in &found {
            if let Some(Some(id)) = &f.approval {
                self.browser_ext.known.insert(id.clone());
            }
        }
        let active: Vec<String> = found.iter().filter(|f| f.enabled).filter_map(|f| f.approval.clone().flatten()).collect();
        self.browser_ext.sync = Some(Sync { pane, found, active, report, failed: HashSet::new(), adds: 0 });
        if let Some(v) = self.browser_views.get(&pane).and_then(|b| b.view.as_ref()) {
            v.list_extensions();
        }
    }

    /// WebView2 listed the profile's extensions (`Event::Extensions`): apply the plan.
    pub(super) fn browser_extensions_listed(&mut self, pane: PaneId, list: &[(String, String, bool)]) {
        let Some(s) = self.browser_ext.sync.take_if(|s| s.pane == pane && s.adds == 0) else { return };
        if self.browser_views.get(&pane).and_then(|b| b.view.as_ref()).is_none() {
            return;
        }
        let (plan, to_add) = {
            let approved: Vec<&Found> = s.found.iter().filter(|f| f.approval.is_some() && !s.failed.contains(&f.key)).collect();
            let folders: Vec<String> = approved.iter().map(|f| f.key.clone()).collect();
            // The ids this run produced, and the inactive approved folders (not added, not removed).
            let mut added: Vec<(String, String)> = Vec::new();
            for f in &approved {
                if let Some(id) = self.browser_ext.ids.get(&(Store::Browser, f.key.clone())) {
                    added.push((f.key.clone(), id.clone()));
                } else if let Some(Some(id)) = &f.approval
                    && !s.active.contains(id) {
                        added.push((f.key.clone(), id.clone()));
                    }
            }
            let installed: Vec<(String, bool)> = list.iter().map(|(i, _, on)| (i.clone(), *on)).collect();
            let plan = be::sync_plan(&installed, &added, &folders, &s.active);
            let to_add: Vec<PathBuf> = plan.add.iter().filter_map(|key| approved.iter().find(|f| &f.key == key)).map(|f| f.folder.clone()).collect();
            (plan, to_add)
        };
        if !to_add.is_empty() {
            if let Some(v) = self.browser_views.get(&pane).and_then(|b| b.view.as_ref()) {
                for folder in &to_add {
                    v.add_extension(folder);
                }
            }
            self.browser_ext.sync = Some(Sync { adds: to_add.len(), ..s });
            return;
        }
        for id in &plan.remove {
            self.browser_extension_remove(pane, id);
        }
        if let Some(v) = self.browser_views.get(&pane).and_then(|b| b.view.as_ref()) {
            for id in &plan.enable {
                v.set_extension_enabled(id, true);
            }
            for id in &plan.disable {
                v.set_extension_enabled(id, false);
            }
        }
        if s.report {
            let rows: Vec<(String, Manifest, bool)> = s
                .found
                .iter()
                .filter(|f| f.approval.is_some() && !s.failed.contains(&f.key))
                .filter_map(|f| {
                    let id = self.browser_ext.ids.get(&(Store::Browser, f.key.clone())).cloned().or_else(|| f.approval.clone().flatten())?;
                    let on = list.iter().find(|(i, _, _)| *i == id).is_some_and(|(_, _, on)| *on && !plan.disable.contains(&id)) || plan.enable.contains(&id);
                    Some((id, f.manifest.clone(), on))
                })
                .collect();
            self.browser_settings_run(pane, vec![be::ui_script(&be::extensions_json(&rows))]);
        }
    }

    /// An add finished (`Event::ExtensionAdded`).
    pub(super) fn browser_extension_added(&mut self, pane: PaneId, folder: &Path, result: &Result<(String, String), String>) {
        let key = be::folder_key(folder);
        let Some(profile) = self.browser_ext_profile(pane) else { return };
        let Some(s) = self.browser_ext.sync.as_mut().filter(|s| s.pane == pane && s.adds > 0) else { return };
        s.adds -= 1;
        let done = s.adds == 0;
        let newly = s.found.iter().find(|f| f.key == key).is_some_and(|f| f.approval == Some(None));
        match result {
            Ok((id, _name)) if be::valid_id(id) => {
                self.browser_ext.ids.insert((Store::Browser, key.clone()), id.clone());
                self.browser_ext.known.insert(id.clone());
                if newly {
                    // Approving a folder makes its extension active (a new record is on).
                    if let Some(s) = self.browser_ext.sync.as_mut() {
                        s.active.push(id.clone());
                    }
                }
                let id = id.clone();
                let notify = self.notify.clone();
                std::thread::spawn(move || {
                    edit_approvals(|a| a.set_id(&profile, &key, &id));
                    notify();
                });
            }
            _ => {
                if let Some(s) = self.browser_ext.sync.as_mut() {
                    s.failed.insert(key);
                }
            }
        }
        if done
            && let Some(v) = self.browser_views.get(&pane).and_then(|b| b.view.as_ref()) {
                v.list_extensions();
            }
    }

    /// THE removal: WebView2 forgets the extension, and Termurion's approval, active id and folder map
    /// entry go in the same call.
    pub(super) fn browser_extension_remove(&mut self, pane: PaneId, id: &str) {
        if !be::valid_id(id) {
            return;
        }
        if let Some(v) = self.browser_views.get(&pane).and_then(|b| b.view.as_ref()) {
            v.remove_extension(id);
        }
        self.browser_ext.ids.retain(|_, v| v != id);
        self.browser_ext.known.remove(id);
        if let Some(profile) = self.browser_ext_profile(pane) {
            let id = id.to_string();
            std::thread::spawn(move || {
                edit_approvals(|a| a.forget_id(&profile, &id));
            });
        }
    }

    /// The settings page toggled an extension. Only an id that was approved here counts.
    pub(super) fn browser_extension_set(&mut self, pane: PaneId, id: &str, enabled: bool) -> Vec<String> {
        use crate::browser_settings_page::status_script;
        let known = self.browser_ext.known.contains(id) || self.browser_ext.ids.values().any(|v| v == id);
        if !be::valid_id(id) || !known || !self.browser_ext_pane_ok(pane) {
            return vec![status_script(be::NOT_AVAILABLE, "error")];
        }
        // The switch lives with the approval, shared by every browser profile; the sync runs when it is saved.
        let (tx, id) = (self.browser_ext.chan.get_or_insert_with(channel).0.clone(), id.to_string());
        let notify = self.notify.clone();
        std::thread::spawn(move || {
            let ok = edit_approvals(|a| a.set_enabled(&id, enabled));
            let _ = tx.send(Done::Approved { pane, ok });
            notify();
        });
        vec![status_script(if enabled { be::ENABLED } else { be::DISABLED }, "success")]
    }

    /// Reveal in Explorer: never a network root; created only here, on the person's request.
    pub(super) fn browser_extensions_reveal(&mut self, pane: PaneId) -> Vec<String> {
        use crate::browser_settings_page::status_script;
        if !self.browser_ext_pane_ok(pane) {
            return vec![status_script(be::NOT_ALLOWED, "error")];
        }
        let value = self.browser_ext_value(pane, be::ROOT_KEY).and_then(|v| v.as_str().map(str::to_string));
        let appdata = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
        let root = be::root(value.as_deref(), &appdata);
        let refuse = || vec![status_script(&be::create_failed("UNC paths are not supported"), "error")];
        if !be::root_ok(&root) {
            return refuse();
        }
        let Some(judged) = localfs::judge(&root) else { return refuse() };
        if let Err(e) = localfs::create_dir_all(&judged) {
            return vec![status_script(&be::create_failed(&e.to_string()), "error")];
        }
        self.execute(pane, crate::semantic::Run::Reveal(judged.path().to_string_lossy().to_string()), None, false);
        vec![status_script(be::REVEALED, "success")]
    }

    /// The native folder picker: the person's choice becomes the pane's root, and every manifest in it
    /// that was not approved before is asked about.
    pub(super) fn browser_extensions_choose(&mut self, pane: PaneId) -> Vec<String> {
        use crate::browser_settings_page::status_script;
        if !self.browser_ext_pane_ok(pane) {
            return vec![status_script(be::NOT_ALLOWED, "error")];
        }
        let Some(folder) = localfs::pick_folder(rfd::FileDialog::new().set_title("Choose a browser extensions folder")) else { return Vec::new() };
        if !be::root_ok(folder.path()) {
            return vec![status_script(&be::create_failed("UNC paths are not supported"), "error")];
        }
        self.browser_ext_write(pane, be::ROOT_KEY, Some(toml::Value::String(folder.path().to_string_lossy().to_string())));
        self.browser_extensions_request(pane, true, true);
        vec![status_script(be::CHOSEN, "success")]
    }

    /// Raises the next waiting question, one at a time.
    fn browser_ext_next_ask(&mut self) {
        while !self.browser_ext.asking {
            let Some(c) = self.browser_ext.asks.pop_front() else { return };
            if !self.browser_ext_pane_ok(c.pane) {
                continue;
            }
            let text = be::approval_text(&c.manifest, &c.folder.to_string_lossy());
            let then = super::scripts_alerts::Then::BrowserExtension { pane: c.pane, folder: c.folder, hash: c.hash };
            let ask = super::scripts_alerts::ask(be::APPROVAL_HEADING, &text, &[be::APPROVAL_BUTTON, crate::api_auth::CANCEL], None, crate::warnings::Kind::Persistent, then);
            self.browser_ext.asking = true;
            self.ask_scripts(ask);
        }
    }

    /// The person answered the approval window (or it could not open: Cancel). Only Enable (0) records
    /// an approval, and only if the manifest is still the one that was shown.
    pub(super) fn browser_extension_answered(&mut self, pane: PaneId, folder: PathBuf, hash: String, index: usize) {
        self.browser_ext.asking = false;
        if index == 0 && self.browser_ext_pane_ok(pane)
            && let Some(profile) = self.browser_ext_profile(pane) {
                let tx = self.browser_ext.chan.get_or_insert_with(channel).0.clone();
                let notify = self.notify.clone();
                std::thread::spawn(move || {
                    let same = be::read_folder_manifest(&folder).is_some_and(|(_, h)| h == hash);
                    let ok = same && edit_approvals(|a| a.approve(&profile, &be::folder_key(&folder), &hash));
                    let _ = tx.send(Done::Approved { pane, ok });
                    notify();
                });
            }
        self.browser_ext_next_ask();
    }

    /// A view or pane is gone: its waiting questions and sync go.
    pub(super) fn browser_extensions_pane_gone(&mut self, pane: PaneId) {
        self.browser_ext.asks.retain(|c| c.pane != pane);
        if self.browser_ext.sync.as_ref().is_some_and(|s| s.pane == pane) {
            self.browser_ext.sync = None;
        }
    }
}

#[cfg(test)]
mod tests {
    fn live() -> &'static str {
        let src = include_str!("browser_extensions_app.rs");
        &src[..src.find("#[cfg(test)]").unwrap()]
    }
    fn body<'a>(s: &'a str, from: &str, to: &str) -> &'a str {
        &s[s.find(from).unwrap()..s.find(to).unwrap()]
    }

    /// Extensions are the largest attack surface of the milestone: these pin the rules that keep an
    /// import, a page or a sync from loading one.
    #[test]
    fn extensions_load_only_what_the_person_approved() {
        let l = live();
        // Never in Dev Null, and a pane needs a view.
        assert!(body(l, "fn browser_ext_pane_ok", "/// What an approval belongs to").contains("!io.dev_null"));
        let req = body(l, "pub(super) fn browser_extensions_request", "/// From the tick");
        assert!(req.find("browser_ext_pane_ok").unwrap() < req.find("std::thread::spawn").unwrap(), "checked before any work");
        assert!(req.contains("filter(|r| !r.trim().is_empty())") && !req.contains("else { return };"), "no chosen root: the approved set still syncs, no orphan");
        // Reveal, like the picker, refuses a Dev Null pane; the page is always answered.
        let reveal = body(l, "pub(super) fn browser_extensions_reveal", "/// The native folder picker");
        assert!(reveal.find("browser_ext_pane_ok").unwrap() < reveal.find("create_dir_all").unwrap() && reveal.contains("be::NOT_ALLOWED"));
        assert!(req.contains("if report {") && req.find("if report {").unwrap() < req.find("std::thread::spawn").unwrap(), "GetExtensions is answered even when no sync runs");
        assert!(!l.contains("a.profile == profile") && l.contains("be::STORE"), "approvals are for the shared browser data");
        // A folder reaches WebView2 only from `browser_extensions_listed`, only among approved folders.
        assert_eq!(l.matches("add_extension(").count(), 1);
        let listed = body(l, "pub(super) fn browser_extensions_listed", "/// An add finished");
        assert!(listed.contains("f.approval.is_some()") && listed.find("f.approval.is_some()").unwrap() < listed.find("add_extension(").unwrap());
        // An approval is recorded in one place: the answer to the native window, after the hash is checked again.
        assert_eq!(l.matches(".approve(").count(), 1);
        let ans = body(l, "pub(super) fn browser_extension_answered", "/// A view or pane is gone");
        assert!(ans.contains("index == 0") && ans.find("read_folder_manifest").unwrap() < ans.find(".approve(").unwrap() && ans.contains("h == hash"));
        // Questions come only from choosing a folder; the sync never asks.
        assert_eq!(l.matches("browser_extensions_request(pane, true").count(), 1);
        // The folder setting goes through the Settings window's owner (final review M5).
        let w = body(l, "fn browser_ext_write", "/// Looks at the pane's chosen root");
        assert!(w.find("self.save_store()").unwrap() < w.find("self.apply_edit(Edit::Changed(shared))").unwrap());
        assert!(body(l, "pub(super) fn browser_extensions_choose", "/// Raises the next").contains("localfs::pick_folder("));
        assert!(include_str!("browser_app.rs").contains("if !dev_null {\n                    self.browser_extensions_request(pane, false, false);"));
        // An unreadable root never removes anything.
        assert!(body(l, "fn browser_ext_scanned", "/// WebView2 listed").contains("if !readable {"));
        // The manifest is never trusted as markup, and no outside tool or log line is involved.
        for banned in ["Command::new", "tar.exe", "rlog!", "dlog!", "innerHTML", "extract::"] {
            assert!(!l.contains(banned), "{banned}");
        }
        // The root is judged: a network location is refused before it is read.
        assert!(body(l, "fn scan(", "impl App").contains("be::root_ok(root)"));
        assert!(body(l, "pub(super) fn browser_extensions_reveal", "/// The native folder picker").find("root_ok").unwrap() < body(l, "pub(super) fn browser_extensions_reveal", "/// The native folder picker").find("create_dir_all").unwrap());
    }

    /// Removal has one owner, and the approval window is guarded, scrolled to its end and a Cancel when closed.
    #[test]
    fn removal_and_the_question_have_one_owner_each() {
        let l = live();
        assert_eq!(l.matches("remove_extension(").count(), 1);
        let rm = body(l, "pub(super) fn browser_extension_remove", "/// The settings page toggled");
        assert!(rm.contains("remove_extension(id)") && rm.contains("ids.retain") && rm.contains("forget_id"));
        let alerts = include_str!("scripts_alerts.rs");
        assert!(alerts.contains("Then::BrowserPermission(_) | Then::BrowserClearData { .. } | Then::BrowserExtension { .. }") && alerts.contains("w.require_read(0)"));
        assert!(alerts.contains("Then::BrowserExtension { .. } => Answer::Button(1)"));
        assert!(alerts.contains("Then::BrowserExtension { pane, folder, hash } => self.browser_extension_answered(pane, folder, hash, 1)"), "a question that cannot open is Cancel");
        assert!(include_str!("mod.rs").contains("self.browser_extensions_pane_gone(id);"));
        let router = include_str!("browser_scripts_app.rs");
        assert!(router.contains("scheme_of(&r.source) == \"chrome-extension\""), "an extension page reaches no handler");
        let page = include_str!("browser_pages_app.rs");
        for arm in ["sp::Msg::GetExtensions => self.browser_extensions_request(pane, false, true)", "self.browser_extension_set(pane, &id, enabled)", "self.browser_extensions_reveal(pane)", "self.browser_extensions_choose(pane)"] {
            assert!(page.contains(arm), "{arm}");
        }
        assert!(include_str!("browser_app.rs").contains("Event::Extensions { list, .. } => self.browser_extensions_listed(pane, list)"));
    }
}
