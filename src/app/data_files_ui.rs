//! Snippets, notes and icon customizations through the settings folder, the app side
//! (milestone 13y2 Task 9a; iTerm2's `iTermRemoteDataFileSync` and the reconcile in
//! `iTermRemotePreferences.m`). The decisions are `crate::data_file_sync`.
//!
//! The one sync owner: this module, called from `prefs_ui` (the launch load, Save Now, the
//! automatic save and the quit question). The folder's files are untrusted input (13y1 Imported
//! trust): only the four allowlisted names are ever touched, each read through the local judge
//! with a size cap and never through a link, checked for shape before it replaces a local file,
//! and written through a temporary file and a rename. A local file that is replaced or removed
//! is copied to `Backups\DataSync` first. Nothing in a file runs.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::mpsc;

use toml::Value;

use super::scripts_alerts::{ask, Ask, Then};
use super::App;
use crate::data_file_sync::{self as dfs, Reconcile, ALLOWLIST};
use crate::globals::Globals;
use crate::localfs::{self, LocalPath};
use crate::remote_prefs as rp;
use crate::warnings::Kind;

/// The allowlisted files of one side. `refused` lists the ones that exist but may not be read
/// (a link, too large, not a regular file): they count as absent for the signature and are
/// neither copied nor deleted by a pull.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Set {
    pub files: Vec<(&'static str, Option<Vec<u8>>)>,
    pub refused: Vec<&'static str>,
}

impl Set {
    pub fn signature(&self) -> String {
        dfs::signature(&self.files)
    }

    pub fn has_any(&self) -> bool {
        self.files.iter().any(|(_, c)| c.is_some())
    }

    fn get(&self, name: &str) -> Option<&Vec<u8>> {
        self.files.iter().find(|(n, _)| *n == name).and_then(|(_, c)| c.as_ref())
    }
}

/// `%APPDATA%\Termurion`, where the local copies live.
pub(super) fn local_dir() -> PathBuf {
    super::snippets_ui::snippets_path().parent().map(Path::to_path_buf).unwrap_or_default()
}

/// One file of a judged folder: Ok(None) when absent, Err when it may not be read.
fn read_one(dir: &LocalPath, name: &str) -> Result<Option<Vec<u8>>, ()> {
    let Some(path) = dir.child(name) else { return Err(()) };
    match localfs::lstat(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(()),
        Ok(st) if !st.is_file || st.is_reparse || st.len > dfs::MAX_FILE_BYTES => return Err(()),
        Ok(_) => {}
    }
    let file = localfs::open(&path).map_err(|_| ())?;
    let meta = localfs::file_stat(&file).map_err(|_| ())?;
    if !meta.is_file || meta.len > dfs::MAX_FILE_BYTES {
        return Err(());
    }
    let mut bytes = Vec::new();
    file.take(dfs::MAX_FILE_BYTES + 1).read_to_end(&mut bytes).map_err(|_| ())?;
    if bytes.len() as u64 > dfs::MAX_FILE_BYTES {
        return Err(());
    }
    Ok(Some(bytes))
}

/// The allowlisted files of `dir`. A folder that is not local is read as having none.
pub(super) fn read_set(dir: &Path) -> Set {
    let mut set = Set::default();
    let judged = LocalPath::judge(dir);
    for name in ALLOWLIST {
        match judged.as_ref().map(|d| read_one(d, name)) {
            Some(Ok(content)) => set.files.push((name, content)),
            _ => {
                set.files.push((name, None));
                if judged.is_some() {
                    set.refused.push(name);
                }
            }
        }
    }
    set
}

/// The folder's Data files, read on a worker so an offline share cannot hold the launch: None
/// when the read did not finish in time (the decision waits for the next launch).
pub(super) fn read_remote(folder: &Path, timeout: std::time::Duration) -> Option<Set> {
    let data = folder.join(dfs::DATA_DIR);
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(read_set(&data));
    });
    rx.recv_timeout(timeout).ok()
}

fn write_atomic(dir: &LocalPath, name: &str, bytes: &[u8]) -> Result<(), String> {
    let tmp = dir.child(format!("{name}.tmp")).ok_or("refused")?;
    let to = dir.child(name).ok_or("refused")?;
    localfs::write(&tmp, bytes).map_err(|e| e.to_string())?;
    localfs::rename(&tmp, &to).map_err(|e| {
        let _ = localfs::remove_file(&tmp);
        e.to_string()
    })
}

/// A copy of the local file before it is replaced or removed.
/// False when the copy failed: the caller then leaves the file alone.
fn backup(local: &LocalPath, name: &str) -> bool {
    let Some(from) = local.child(name) else { return false };
    if !localfs::exists(&from) {
        return true;
    }
    let Some(dir) = local.join("Backups").and_then(|d| d.join("DataSync")) else { return false };
    localfs::create_dir_all(&dir).is_ok() && dir.child(name).is_some_and(|to| localfs::copy(&from, &to).is_ok())
}

/// The folder's copy replaces the local one: each folder file that passes `valid_content` is
/// written, a folder that lacks a file removes the local one, a refused or invalid folder file
/// leaves the local file alone. Returns whether anything changed.
pub(super) fn apply_pull(local: &Path, remote: &Set) -> bool {
    pull(local, remote).0
}

/// `apply_pull`, and whether it was complete: no backup, write or removal failed. The baseline
/// moves to the folder's state only after a complete pull; otherwise the next reconcile pulls
/// again instead of seeing the unpulled local files as the person's newer edits and pushing them
/// over the folder (13y2 review 9a N1). A refused or invalid folder file does not make a pull
/// incomplete: it is left alone by design.
pub(super) fn pull(local: &Path, remote: &Set) -> (bool, bool) {
    let Some(dir) = LocalPath::judge(local) else { return (false, false) };
    if localfs::create_dir_all(&dir).is_err() {
        return (false, false);
    }
    let (mut changed, mut complete) = (false, true);
    for name in ALLOWLIST {
        if remote.refused.contains(&name) {
            continue;
        }
        let current = read_one(&dir, name).ok().flatten();
        match remote.get(name) {
            Some(bytes) if dfs::valid_content(name, bytes) => {
                if current.as_deref() != Some(bytes.as_slice()) {
                    let done = backup(&dir, name) && write_atomic(&dir, name, bytes).is_ok();
                    changed |= done;
                    complete &= done;
                }
            }
            Some(_) => {}
            None => {
                if current.is_some() {
                    let done = backup(&dir, name) && dir.child(name).is_some_and(|p| localfs::remove_file(&p).is_ok());
                    changed |= done;
                    complete &= done;
                }
            }
        }
    }
    (changed, complete)
}

/// The local files go to `<folder>\Data`, creating it. A union: a file the folder holds and the
/// local side lacks is never deleted. False when the folder could not be written.
pub(super) fn apply_push(folder: &Path, local: &Set) -> bool {
    let data = folder.join(dfs::DATA_DIR);
    let Some(dir) = LocalPath::judge(&data) else { return false };
    if localfs::create_dir_all(&dir).is_err() {
        return false;
    }
    let mut ok = true;
    for (name, content) in &local.files {
        let Some(bytes) = content else { continue };
        if read_one(&dir, name).ok().flatten().as_ref() == Some(bytes) {
            continue;
        }
        ok &= write_atomic(&dir, name, bytes).is_ok();
    }
    ok
}

/// "Lose Changes": the local files are removed (copied aside first). True when every removal
/// worked; the caller records a pending discard otherwise.
pub(super) fn discard_local(local: &Path) -> bool {
    let Some(dir) = LocalPath::judge(local) else { return false };
    let mut ok = true;
    for name in ALLOWLIST {
        let Some(p) = dir.child(name) else { continue };
        if localfs::lstat(&p).is_ok() {
            ok &= backup(&dir, name) && localfs::remove_file(&p).is_ok();
        }
    }
    ok
}

fn entry(g: &Globals, dict: &str, folder: &str) -> Option<Value> {
    g.table(dict).get(folder).cloned()
}

fn set_entry(g: &mut Globals, dict: &str, folder: &str, value: Option<Value>) {
    let mut t = g.table(dict);
    match value {
        Some(v) => {
            t.insert(folder.to_string(), v);
        }
        None => {
            t.remove(folder);
        }
    }
    g.set(dict, Value::Table(t));
}

fn text_entry(g: &Globals, dict: &str, folder: &str) -> Option<String> {
    entry(g, dict, folder).and_then(|v| v.as_str().map(str::to_string))
}

fn set_text(g: &mut Globals, dict: &str, folder: &str, text: &str) {
    set_entry(g, dict, folder, Some(Value::String(text.to_string())));
}

fn consent(g: &Globals, folder: &str) -> Option<bool> {
    entry(g, dfs::CONSENT_KEY, folder).and_then(|v| v.as_bool())
}

fn consent_ask(folder: &Path) -> Ask {
    let text = dfs::with_folder(dfs::CONSENT_TEXT, &folder.display().to_string());
    ask(crate::script_new::WARNING, &text, &[dfs::SYNC_TOO, dfs::JUST_SETTINGS], None, Kind::Persistent, Then::DataConsent(folder.to_path_buf()))
}

fn conflict_ask(folder: &Path) -> Ask {
    let text = dfs::with_folder(dfs::CONFLICT_TEXT, &folder.display().to_string());
    ask(crate::script_new::WARNING, &text, &[dfs::USE_THIS_PC, dfs::USE_FOLDER], None, Kind::Persistent, Then::DataConflict(folder.to_path_buf()))
}

/// Records the signature both sides now have.
fn adopt(g: &mut Globals, key: &str, sig: &str) {
    set_text(g, dfs::BASELINE_KEY, key, sig);
}

/// The reconcile (`iTermRemotePreferences.m` `reconcileDataFiles`): the decision for a folder,
/// done when it needs no question. `remote` None (the read timed out) defers. Returns the
/// question to ask, and whether the local files changed.
pub(super) fn reconcile_with(g: &mut Globals, local_dir: &Path, folder: &Path, remote: Option<Set>) -> (Vec<Ask>, bool) {
    let key = folder.display().to_string();
    // A discard that failed at the last quit is tried again first.
    let mut pulled = false;
    if entry(g, dfs::DISCARD_PENDING_KEY, &key).is_some() {
        if !discard_local(local_dir) {
            // Still not gone: reconciling now could publish the edits the person chose to lose.
            return (Vec::new(), false);
        }
        set_entry(g, dfs::DISCARD_PENDING_KEY, &key, None);
        adopt(g, &key, &dfs::empty_signature());
        set_entry(g, dfs::LAST_PUSHED_KEY, &key, None);
        pulled = true;
    }
    let Some(remote) = remote else { return (Vec::new(), pulled) };
    let local = read_set(local_dir);
    let (l, r) = (local.signature(), remote.signature());
    let baseline = text_entry(g, dfs::BASELINE_KEY, &key);
    let mut asks = Vec::new();
    match dfs::reconcile(consent(g, &key), remote.has_any(), &l, &r, baseline.as_deref()) {
        Reconcile::Nothing => {
            if consent(g, &key) == Some(true) && l == r && baseline.as_deref() != Some(&r) {
                adopt(g, &key, &r);
            }
        }
        Reconcile::AskConsent => asks.push(consent_ask(folder)),
        Reconcile::Pull => {
            let (changed, complete) = pull(local_dir, &remote);
            pulled |= changed;
            if complete {
                adopt(g, &key, &r);
            }
        }
        Reconcile::Push => {
            if apply_push(folder, &local) {
                adopt_after_push(g, &key, folder, &l);
            }
        }
        Reconcile::Conflict => asks.push(conflict_ask(folder)),
    }
    let _ = g.save();
    (asks, pulled)
}

fn adopt_after_push(g: &mut Globals, key: &str, folder: &Path, local_sig: &str) {
    let now = read_set(&folder.join(dfs::DATA_DIR)).signature();
    adopt(g, key, &now);
    set_text(g, dfs::LAST_PUSHED_KEY, key, local_sig);
}

/// The launch's reconcile, before the snippets, notes and icon maps are loaded (so a pull needs
/// no reload). Only a local folder syncs.
pub(super) fn reconcile_at_start(g: &mut Globals, folder: &Path) -> Vec<Ask> {
    let remote = read_remote(folder, dfs::REMOTE_READ_TIMEOUT);
    reconcile_with(g, &local_dir(), folder, remote).0
}

/// A silent push (`:533-600`) when it is allowed; returns whether the folder was written.
pub(super) fn push_if_allowed(g: &mut Globals, local_dir: &Path, folder: &Path, remote: Option<Set>) -> bool {
    let key = folder.display().to_string();
    let Some(remote) = remote else { return false };
    if entry(g, dfs::DISCARD_PENDING_KEY, &key).is_some() {
        return false;
    }
    let local = read_set(local_dir);
    let l = local.signature();
    let baseline = text_entry(g, dfs::BASELINE_KEY, &key);
    let last = text_entry(g, dfs::LAST_PUSHED_KEY, &key);
    if !dfs::push_allowed(consent(g, &key) == Some(true), baseline.as_deref(), &l, last.as_deref(), &remote.signature()) {
        return false;
    }
    if !apply_push(folder, &local) {
        return false;
    }
    adopt_after_push(g, &key, folder, &l);
    let _ = g.save();
    true
}

/// Whether the local files changed since the folder had them (what the quit question names).
pub(super) fn differs(g: &Globals, local_dir: &Path, folder: &Path) -> bool {
    let key = folder.display().to_string();
    if consent(g, &key) != Some(true) {
        return false;
    }
    let Some(baseline) = text_entry(g, dfs::BASELINE_KEY, &key) else { return false };
    let l = read_set(local_dir).signature();
    l != baseline && text_entry(g, dfs::LAST_PUSHED_KEY, &key).as_deref() != Some(&l)
}

impl App {
    /// The folder the data files sync with: the settings folder when loading from one is on and
    /// it is a local folder (never a URL or a network path).
    pub(super) fn data_folder(&self) -> Option<PathBuf> {
        if !self.globals.bool(rp::LOAD_KEY) {
            return None;
        }
        self.prefs_file()?;
        let (_, location) = self.prefs_location();
        let base = PathBuf::from(location);
        Some(if localfs::is_local_file(&base) { base.parent()?.to_path_buf() } else { base })
    }

    /// Reads the folder again and does what the decision says (launch answers, Save Now).
    pub(super) fn data_files_reconcile(&mut self, folder: &Path) {
        let _ = self.global_note.flush();
        let remote = read_remote(folder, dfs::REMOTE_READ_TIMEOUT);
        let (asks, pulled) = reconcile_with(&mut self.globals, &local_dir(), folder, remote);
        for a in asks {
            self.ask_scripts(a);
        }
        if pulled {
            self.data_files_reload();
        }
    }

    /// A save pushes the files when that is allowed; an interactive save that finds the folder
    /// moved reconciles instead (it may ask which copy to keep).
    pub(super) fn data_files_push(&mut self, folder: &Path, interactive: bool) {
        let remote = read_remote(folder, dfs::REMOTE_READ_TIMEOUT);
        if !push_if_allowed(&mut self.globals, &local_dir(), folder, remote.clone()) && interactive {
            self.data_files_reconcile(folder);
        }
    }

    /// Whether the local files changed since the last sync, for the quit question.
    pub(super) fn data_files_differ(&self) -> bool {
        self.data_folder().is_some_and(|f| differs(&self.globals, &local_dir(), &f))
    }

    /// "Lose Changes": the local files go (copied aside), and the folder's come at the next
    /// reconcile; a removal that failed is recorded for the next launch.
    pub(super) fn data_files_discard(&mut self, folder: &Path) {
        let _ = self.global_note.flush();
        let key = folder.display().to_string();
        if discard_local(&local_dir()) {
            adopt(&mut self.globals, &key, &dfs::empty_signature());
            set_entry(&mut self.globals, dfs::LAST_PUSHED_KEY, &key, None);
        } else {
            set_entry(&mut self.globals, dfs::DISCARD_PENDING_KEY, &key, Some(Value::Boolean(true)));
        }
        let _ = self.globals.save();
    }

    /// After a pull the snippets and the global note are read from disk without writing back,
    /// and the icon tables again.
    fn data_files_reload(&mut self) {
        self.snippets = super::snippets_ui::load_snippets();
        let appdata = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
        self.global_note = crate::notes::GlobalNote::new(super::toolbelt_more_ui::notes_path(&appdata));
        self.tools_changed();
        self.reload_tab_icon_maps();
        self.redraw();
    }

    /// The answers to the consent and conflict questions.
    pub(super) fn data_files_answered(&mut self, then: Then, index: usize) {
        match (then, index) {
            (Then::DataConsent(folder), i) => {
                let key = folder.display().to_string();
                set_entry(&mut self.globals, dfs::CONSENT_KEY, &key, Some(Value::Boolean(i == 0)));
                let _ = self.globals.save();
                if i == 0 {
                    self.data_files_reconcile(&folder);
                }
            }
            (Then::DataConflict(folder), i) => {
                let _ = self.global_note.flush();
                let key = folder.display().to_string();
                let local_dir = local_dir();
                if i == 0 {
                    let local = read_set(&local_dir);
                    if apply_push(&folder, &local) {
                        adopt_after_push(&mut self.globals, &key, &folder, &local.signature());
                    }
                } else if let Some(remote) = read_remote(&folder, dfs::REMOTE_READ_TIMEOUT) {
                    let (pulled, complete) = pull(&local_dir, &remote);
                    if complete {
                        adopt(&mut self.globals, &key, &remote.signature());
                    }
                    if pulled {
                        self.data_files_reload();
                    }
                }
                let _ = self.globals.save();
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("termurion-data-sync-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn globals(dir: &Path) -> Globals {
        Globals::empty(&dir.join("settings.toml"))
    }

    #[test]
    fn a_push_is_a_union_and_a_pull_replaces() {
        let root = temp("union");
        let (local, folder) = (root.join("local"), root.join("folder"));
        std::fs::create_dir_all(&local).unwrap();
        std::fs::create_dir_all(folder.join("Data")).unwrap();
        std::fs::write(local.join("notes.txt"), "mine").unwrap();
        std::fs::write(folder.join("Data").join("snippets.json"), r#"{"snippets": []}"#).unwrap();
        // A push copies the local note and leaves the folder's snippets.
        assert!(apply_push(&folder, &read_set(&local)));
        assert_eq!(std::fs::read_to_string(folder.join("Data").join("notes.txt")).unwrap(), "mine");
        assert!(folder.join("Data").join("snippets.json").exists());
        // A file outside the allowlist is never touched or copied.
        std::fs::write(local.join("evil.exe"), "x").unwrap();
        assert!(apply_push(&folder, &read_set(&local)));
        assert!(!folder.join("Data").join("evil.exe").exists());
        // A pull takes the folder's snippets, and deletes the local file the folder lacks.
        std::fs::remove_file(folder.join("Data").join("notes.txt")).unwrap();
        assert!(apply_pull(&local, &read_set(&folder.join("Data"))));
        assert!(local.join("snippets.json").exists());
        assert!(!local.join("notes.txt").exists());
        assert!(local.join("Backups").join("DataSync").join("notes.txt").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_pull_refuses_what_is_not_the_file_it_claims_to_be() {
        let root = temp("refuse");
        let (local, folder) = (root.join("local"), root.join("folder"));
        std::fs::create_dir_all(&local).unwrap();
        std::fs::create_dir_all(folder.join("Data")).unwrap();
        std::fs::write(local.join("snippets.json"), r#"{"snippets": []}"#).unwrap();
        std::fs::write(folder.join("Data").join("snippets.json"), "MZ not json").unwrap();
        std::fs::write(folder.join("Data").join("notes.txt"), vec![b'a'; (dfs::MAX_FILE_BYTES + 1) as usize]).unwrap();
        apply_pull(&local, &read_set(&folder.join("Data")));
        assert_eq!(std::fs::read_to_string(local.join("snippets.json")).unwrap(), r#"{"snippets": []}"#);
        assert!(!local.join("notes.txt").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_flow_asks_then_pulls_then_pushes() {
        let root = temp("flow");
        let (local, folder) = (root.join("local"), root.join("folder"));
        std::fs::create_dir_all(&local).unwrap();
        std::fs::create_dir_all(folder.join("Data")).unwrap();
        std::fs::write(folder.join("Data").join("notes.txt"), "from folder").unwrap();
        let mut g = globals(&root);
        let key = folder.display().to_string();
        let remote = || Some(read_set(&folder.join("Data")));
        // No answer yet: ask, change nothing.
        let (asks, pulled) = reconcile_with(&mut g, &local, &folder, remote());
        assert_eq!(asks.len(), 1);
        assert!(!pulled && !local.join("notes.txt").exists());
        // Consented: the first reconcile pulls.
        set_entry(&mut g, dfs::CONSENT_KEY, &key, Some(Value::Boolean(true)));
        let (asks, pulled) = reconcile_with(&mut g, &local, &folder, remote());
        assert!(asks.is_empty() && pulled);
        assert_eq!(std::fs::read_to_string(local.join("notes.txt")).unwrap(), "from folder");
        // A local edit pushes silently, and a second push finds nothing to do.
        std::fs::write(local.join("notes.txt"), "edited").unwrap();
        assert!(differs(&g, &local, &folder));
        assert!(push_if_allowed(&mut g, &local, &folder, remote()));
        assert_eq!(std::fs::read_to_string(folder.join("Data").join("notes.txt")).unwrap(), "edited");
        assert!(!differs(&g, &local, &folder));
        assert!(!push_if_allowed(&mut g, &local, &folder, remote()));
        // Both sides move: a question, nothing overwritten.
        std::fs::write(local.join("notes.txt"), "local two").unwrap();
        std::fs::write(folder.join("Data").join("notes.txt"), "folder two").unwrap();
        let (asks, _) = reconcile_with(&mut g, &local, &folder, remote());
        assert_eq!(asks.len(), 1);
        assert_eq!(std::fs::read_to_string(local.join("notes.txt")).unwrap(), "local two");
        // A read that timed out decides nothing.
        let (asks, pulled) = reconcile_with(&mut g, &local, &folder, None);
        assert!(asks.is_empty() && !pulled);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_lost_change_is_removed_and_the_folders_comes_next() {
        let root = temp("lose");
        let (local, folder) = (root.join("local"), root.join("folder"));
        std::fs::create_dir_all(&local).unwrap();
        std::fs::create_dir_all(folder.join("Data")).unwrap();
        std::fs::write(local.join("notes.txt"), "mine").unwrap();
        std::fs::write(folder.join("Data").join("notes.txt"), "theirs").unwrap();
        let mut g = globals(&root);
        let key = folder.display().to_string();
        set_entry(&mut g, dfs::CONSENT_KEY, &key, Some(Value::Boolean(true)));
        adopt(&mut g, &key, &read_set(&folder.join("Data")).signature());
        assert!(discard_local(&local));
        adopt(&mut g, &key, &dfs::empty_signature());
        assert!(!local.join("notes.txt").exists());
        assert!(local.join("Backups").join("DataSync").join("notes.txt").exists());
        let (_, pulled) = reconcile_with(&mut g, &local, &folder, Some(read_set(&folder.join("Data"))));
        assert!(pulled);
        assert_eq!(std::fs::read_to_string(local.join("notes.txt")).unwrap(), "theirs");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A failed backup stops the overwrite or removal; a discard that cannot finish never lets the
    /// reconcile publish the edits the person chose to lose.
    #[test]
    fn a_failed_backup_keeps_the_file_and_a_pending_discard_publishes_nothing() {
        let root = temp("backupfail");
        let (local, folder) = (root.join("local"), root.join("folder"));
        std::fs::create_dir_all(&local).unwrap();
        std::fs::create_dir_all(folder.join("Data")).unwrap();
        std::fs::write(local.join("notes.txt"), "mine").unwrap();
        // "Backups" is a file, so the backup directory cannot be made.
        std::fs::write(local.join("Backups"), "x").unwrap();
        assert!(!discard_local(&local));
        assert!(local.join("notes.txt").exists(), "no backup, no removal");
        let mut g = globals(&root);
        let key = folder.display().to_string();
        set_entry(&mut g, dfs::CONSENT_KEY, &key, Some(Value::Boolean(true)));
        adopt(&mut g, &key, &dfs::empty_signature());
        set_entry(&mut g, dfs::DISCARD_PENDING_KEY, &key, Some(Value::Boolean(true)));
        let (asks, _) = reconcile_with(&mut g, &local, &folder, Some(read_set(&folder.join("Data"))));
        assert!(asks.is_empty());
        assert!(!folder.join("Data").join("notes.txt").exists(), "the discarded edits are not published");
        assert!(!push_if_allowed(&mut g, &local, &folder, Some(read_set(&folder.join("Data")))));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 9a N1: a pull a failed backup stopped leaves the baseline alone, so the next sync sees the
    /// folder's newer data (a pull again), never a local push over it.
    #[test]
    fn a_pull_stopped_by_a_failed_backup_keeps_the_baseline() {
        let root = temp("pullbaseline");
        let (local, folder) = (root.join("local"), root.join("folder"));
        std::fs::create_dir_all(&local).unwrap();
        std::fs::create_dir_all(folder.join("Data")).unwrap();
        std::fs::write(local.join("notes.txt"), "old").unwrap();
        std::fs::write(local.join("Backups"), "x").unwrap();
        let mut g = globals(&root);
        let key = folder.display().to_string();
        set_entry(&mut g, dfs::CONSENT_KEY, &key, Some(Value::Boolean(true)));
        adopt(&mut g, &key, &read_set(&local).signature());
        let baseline = text_entry(&g, dfs::BASELINE_KEY, &key);
        std::fs::write(folder.join("Data").join("notes.txt"), "newer from another PC").unwrap();
        let (asks, pulled) = reconcile_with(&mut g, &local, &folder, Some(read_set(&folder.join("Data"))));
        assert!(asks.is_empty() && !pulled);
        assert_eq!(std::fs::read_to_string(local.join("notes.txt")).unwrap(), "old", "no backup, no overwrite");
        assert_eq!(text_entry(&g, dfs::BASELINE_KEY, &key), baseline, "the baseline did not move");
        assert!(!push_if_allowed(&mut g, &local, &folder, Some(read_set(&folder.join("Data")))), "the stale file is not pushed");
        assert_eq!(std::fs::read_to_string(folder.join("Data").join("notes.txt")).unwrap(), "newer from another PC");
        // With the backup possible again the same reconcile pulls and adopts.
        std::fs::remove_file(local.join("Backups")).unwrap();
        let (_, pulled) = reconcile_with(&mut g, &local, &folder, Some(read_set(&folder.join("Data"))));
        assert!(pulled);
        assert_eq!(std::fs::read_to_string(local.join("notes.txt")).unwrap(), "newer from another PC");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// I2: a closed conflict window answers nothing (changes neither side, asks again at launch).
    #[test]
    fn a_closed_conflict_window_is_no_answer() {
        assert_eq!(super::super::scripts_alerts::on_close(&Then::DataConflict(PathBuf::from("f")), 0), super::super::scripts_alerts::Answer::Closed);
    }

    #[test]
    fn the_quit_questions_pick_iterm2s_text() {
        let both = super::super::prefs_ui::quit_ask_for_test("D:\\sync", true);
        assert!(both.contains("Settings and your snippets, notes, or icon customizations have changed. Copy them to D:\\sync?"));
        let only = super::super::prefs_ui::quit_ask_for_test("D:\\sync", false);
        assert!(only.contains("Settings have changed. Copy them to D:\\sync?"));
    }

    /// Merge seam (13y2 Task 9a with 13x8's settings, onboarding and extensions pages): the pages
    /// write settings through `self.globals` and `apply_edit` like the Settings window, never a data
    /// file or a sync key; the sync writes only its own NoSync keys (never carried by remote prefs)
    /// and the four data files; prefs_ui is its one caller.
    #[test]
    fn the_settings_pages_and_the_data_sync_share_no_writer() {
        use crate::data_file_sync as dfs;
        let keys = [dfs::CONSENT_KEY, dfs::BASELINE_KEY, dfs::LAST_PUSHED_KEY, dfs::DISCARD_PENDING_KEY];
        for k in keys {
            assert!(k.starts_with("NoSync") && !crate::remote_prefs::syncable(k), "{k}");
        }
        let pages = [
            ("browser_pages_app.rs", include_str!("browser_pages_app.rs")),
            ("browser_extensions_app.rs", include_str!("browser_extensions_app.rs")),
            ("browser_settings_page.rs", include_str!("../browser_settings_page.rs")),
            ("browser_onboarding.rs", include_str!("../browser_onboarding.rs")),
        ];
        for (name, text) in pages {
            let prod = text.split("#[cfg(test)]").next().unwrap();
            for f in dfs::ALLOWLIST {
                assert!(!prod.contains(f), "{name} names the data file {f}");
            }
            for k in keys {
                assert!(!prod.contains(k), "{name} writes the sync key {k}");
            }
            assert!(!prod.contains("data_file_sync") && !prod.contains("data_files_") && !prod.contains("Globals::load") && !prod.contains("Globals::empty"), "{name}: one settings writer");
        }
        let own = include_str!("data_files_ui.rs");
        let prod = &own[..own.find("#[cfg(test)]\nmod tests").unwrap()];
        let pages_prod = include_str!("browser_pages_app.rs").split("#[cfg(test)]").next().unwrap();
        for k in ["WebKitAdblockEnabled", "AdblockListURL", "SearchCommand", "SearchSuggestURL", "BrowserProxyEnabled", "BrowserProxyHost", "BrowserProxyPort"] {
            assert!(pages_prod.contains(&format!("self.globals.set(\"{k}\"")), "{k} is a settings page key");
            assert!(!prod.contains(k), "the data sync never writes {k}");
        }
        assert!(!prod.contains("self.globals.set(") && !prod.contains("Globals::load"), "the sync writes globals only through set_entry");
        for f in ["browser_pages_app.rs", "browser_extensions_app.rs"] {
            let t = std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/app").join(f)).unwrap();
            assert!(!t.contains("data_files_push") && !t.contains("data_files_reconcile"), "{f}");
        }
    }
}
