//! "Load settings from a custom folder or URL", the app side (iTerm2's `iTermRemotePreferences`,
//! `RP`, and `GP:4262-4292`): the load at launch, the automatic save, Save Now, and the question
//! at quit. The model is `crate::remote_prefs`; the rows are `settings_keys::prefs_rows`.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use toml::Value;
use winit::event_loop::ActiveEventLoop;

use super::App;
use super::scripts_alerts::{ask, Ask, Then};
use crate::warnings::Kind;
use crate::globals::Globals;
use crate::profile::Profile;
use crate::remote_prefs::{self as rp, QuitStep, Snapshot};
use crate::settings_ui::Edit;
use crate::store::Store;

/// `setNeedsSave`'s delay (`RP:417-435`).
const SAVE_DELAY: Duration = Duration::from_millis(500);

/// A fetch of the URL: its text, or curl's exit code and message.
pub(super) type Fetched = Result<String, (i32, String)>;

/// What the launch found (`freshCopyOfRemotePreferences`, `RP:234-313`).
pub(super) enum Start {
    /// The setting is off.
    Off,
    Loaded(Snapshot),
    /// A question or a failure to show once the first window is open; nothing was loaded.
    Ask(Ask),
}

/// What Save Now does (`RP:335-392`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SavePlan {
    /// "Settings cannot be copied to a URL."
    CannotCopyToUrl,
    /// The file changed since Tandem loaded it: "Overwrite it?"
    AskOverwrite,
    Write,
}

/// `saveLocalUserDefaultsToRemotePrefs`' order: a URL is never written; a file changed since
/// launch is not overwritten without asking.
pub(super) fn save_plan(url: bool, remote_changed: bool) -> SavePlan {
    if url {
        SavePlan::CannotCopyToUrl
    } else if remote_changed {
        SavePlan::AskOverwrite
    } else {
        SavePlan::Write
    }
}

/// `customFolderChanged` (`GP:2213-2216`): the location as it was at launch, until it first
/// differs; then None for the rest of the session, even if it is typed back.
pub(super) fn launch_location_after(launch: Option<String>, now: &str) -> Option<String> {
    launch.filter(|l| l == now)
}

/// Whether the exit waits: any alert showing or queued, so a question at quit, the failure
/// to copy after it (`RP:376-386`) or any other alert is seen before Tandem ends.
pub(super) fn exit_waits(showing: Option<&Ask>, queued: &std::collections::VecDeque<Ask>) -> bool {
    showing.is_some() || !queued.is_empty()
}

fn home() -> PathBuf {
    std::env::var_os("USERPROFILE").map(PathBuf::from).unwrap_or_default()
}

/// `noSyncDownloadPrefsTimeout` (`ASM:550`).
fn timeout(g: &Globals) -> f64 {
    g.number(rp::TIMEOUT_KEY).unwrap_or(5.0)
}


/// "Failed to load settings from custom directory. ..." (`RP:304-310`): the detail names the
/// location as typed.
fn failed_folder(text: &str) -> Ask {
    ask(rp::FAILED_FOLDER, &rp::missing_file(text), &[crate::api_auth::OK], None, Kind::Persistent, Then::Nothing)
}

/// "Error Loading Settings" (`RP:295-303`).
fn location_not_set() -> Ask {
    let buttons = [rp::DONT_LOAD, crate::api_auth::CANCEL];
    ask(rp::ERROR_LOADING, rp::LOCATION_NOT_SET, &buttons, None, Kind::Persistent, Then::PrefsLocationNotSet)
}

/// The remembered URL's question (`RP:241-272`), an iTerm2 warning that cannot be silenced.
fn url_prompt(url: &str) -> Ask {
    let buttons = [rp::KEEP_LOCAL, rp::DISABLE_URL, rp::DISCARD_LOCAL];
    ask(crate::script_new::WARNING, rp::URL_PROMPT, &buttons, None, Kind::Persistent, Then::PrefsUrlPrompt(url.to_string()))
}

/// `didFailToLoadFromURL:withError:` (`RP:209-232`): curl's exit code 28 is the timeout.
fn failed_url(url: &str, code: i32, error: &str) -> Ask {
    let timed_out = code == rp::CURL_TIMED_OUT;
    let mut buttons = vec![crate::api_auth::OK, rp::REVEAL_IN_SETTINGS];
    if timed_out {
        buttons.push(rp::TRY_AGAIN);
    }
    ask(rp::FAILED_URL, &rp::http_failed(error), &buttons, None, Kind::Persistent, Then::PrefsFailedUrl { url: url.to_string(), timed_out })
}

/// "Settings at ... changed since Tandem started. Overwrite it?" (`RP:338-347`), naming the
/// location as typed; it cannot be silenced.
pub(super) fn overwrite_ask(text: &str) -> Ask {
    let buttons = [rp::OVERWRITE, rp::DISCARD_LOCAL];
    ask(crate::script_new::WARNING, &rp::remote_changed(text), &buttons, None, Kind::Persistent, Then::PrefsOverwrite)
}

/// "Settings have changed. Copy them to ...?" at quit (`RP:1682-1688`), silenced under the Save
/// changes popup's key. Lose Changes is never remembered, so a saved Lose Changes (Manually)
/// still asks, as in iTerm2.
fn quit_ask(text: &str, data: bool) -> Ask {
    let buttons = [rp::COPY, rp::LOSE_CHANGES];
    let question = if data { crate::data_file_sync::with_folder(crate::data_file_sync::COPY_BOTH_TEXT, text) } else { rp::settings_changed(text) };
    ask(crate::script_new::WARNING, &question, &buttons, Some(rp::HAVE_SELECTION_KEY), Kind::Permanent, Then::PrefsQuit)
}

/// The quit question's text for the test.
#[cfg(test)]
pub(super) fn quit_ask_for_test(folder: &str, data: bool) -> String {
    quit_ask(folder, data).spec.text.clone()
}

/// iTerm2's silenceable key for the data-only quit question (`RP:1724-1726`); only Copy is remembered.
const DATA_QUIT_KEY: &str = "NoSyncNeverRemindDataFileChangesLostForFile";

/// "Your snippets, notes, or icon customizations changed. Copy them to ...?" at quit when only
/// the data files changed; Lose Changes is never remembered.
fn data_quit_ask(folder: &str) -> Ask {
    let buttons = [rp::COPY, rp::LOSE_CHANGES];
    ask(crate::script_new::WARNING, &crate::data_file_sync::with_folder(crate::data_file_sync::COPY_DATA_TEXT, folder), &buttons, Some(DATA_QUIT_KEY), Kind::Permanent, Then::PrefsQuitData)
}

/// The URL's text as loaded: the snapshot, or the failure (an empty or malformed file falls
/// back with the folder's alert, as iTerm2's empty dictionary does, `RP:293-311`).
fn fetched(text: &str, url: &str, result: Fetched) -> Start {
    match result {
        Ok(body) => Snapshot::parse(&body).map_or_else(|| Start::Ask(failed_folder(text)), Start::Loaded),
        Err((code, error)) => Start::Ask(failed_url(url, code, &error)),
    }
}

/// What the launch finds at the location (`freshCopyOfRemotePreferences`). A network path, or a
/// folder or file reached through a link to one (`rp::local_folder`), is never read; an empty
/// location reads nothing.
fn start(g: &Globals, home: &Path, read_link: &dyn Fn(&Path) -> Option<PathBuf>, fetch: &dyn Fn(&str, Option<f64>) -> Fetched) -> Start {
    if !g.bool(rp::LOAD_KEY) {
        return Start::Off;
    }
    let text = g.string(rp::FOLDER_KEY);
    let location = rp::expanded(&text, home);
    if rp::is_url(&location) {
        if rp::should_prompt_url(&g.string(rp::PROMPT_URL_KEY), &location) {
            return Start::Ask(url_prompt(&location));
        }
        return fetched(&text, &location, fetch(&location, Some(timeout(g))));
    }
    if text.is_empty() {
        return Start::Ask(location_not_set());
    }
    if rp::is_unc(&text) || !rp::local_folder(Path::new(&location), read_link) {
        return Start::Ask(failed_folder(&text));
    }
    let file = rp::prefs_file(Path::new(&location), &|p| crate::localfs::is_local_file(p));
    if !rp::local_folder(&file, read_link) {
        return Start::Ask(failed_folder(&text));
    }
    match crate::localfs::judge_io(&file).and_then(|f| crate::localfs::read_to_string(&f)).ok().and_then(|t| Snapshot::parse(&t)) {
        Some(snap) => Start::Loaded(snap),
        None => Start::Ask(failed_folder(&text)),
    }
}

/// `copyRemotePrefsToLocalUserDefaultsPreserving:` (`RP:1531-1551`): every local syncable
/// setting goes and the snapshot's come; the profiles become the snapshot's (one new "Default"
/// when it has none, as Tandem always has a profile).
pub(super) fn apply_snapshot(snap: &Snapshot, g: &mut Globals, store: &mut Store) {
    let before = (g.string(crate::workgroup::KEY), g.string(crate::workgroup::KEY_IMPORTED));
    rp::apply(snap, &mut g.values);
    // The snapshot's values are bounded as the one write point bounds them (custom presets).
    g.bound_values();
    // Its workgroups are marked Imported and it cannot carry or clear the marks (13y1 trust ruling).
    crate::workgroup::mark_foreign_write(g, &before.0, &before.1);
    // A start folder naming a network path from the loaded file is emptied: it was not typed
    // in Settings (`start_dir::scrub_map`).
    let mut profiles: Vec<Profile> = snap.profiles.iter().cloned().map(|mut values| {
        crate::start_dir::scrub_map(&mut values);
        Profile::from_values(values)
    }).collect();
    if profiles.is_empty() {
        profiles.push(Profile::new("Default"));
    }
    // The snapshot never holds Dynamic Profiles (`file_profiles` leaves them out), so the ones
    // running stay, unless the snapshot holds the same Guid (then the file's entry is refused as a
    // clash by the reload that follows).
    let running: Vec<Profile> = store.profiles.iter().filter(|p| p.bool(crate::dynamic_profiles::DYNAMIC_KEY)).filter(|p| !profiles.iter().any(|q| q.guid() == p.guid())).cloned().collect();
    crate::store::ensure_unique_guids(&mut profiles);
    store.default_guid = if profiles.iter().any(|p| p.guid() == snap.default) { snap.default.clone() } else { profiles[0].guid() };
    profiles.extend(running);
    store.profiles = profiles;
}

/// The launch's load (`RP:1452-1535`): with the setting on, the location's settings replace
/// the local ones. Returns the snapshot loaded (iTerm2's `savedRemotePrefs`) and the questions
/// or failures to show after the first window opens.
pub(super) fn load_at_start(g: &mut Globals, store: &mut Store, fetch: &dyn Fn(&str, Option<f64>) -> Fetched) -> (Option<Snapshot>, Vec<Ask>) {
    match start(g, &home(), &crate::netpath::read_reparse, fetch) {
        Start::Off => (None, Vec::new()),
        Start::Ask(ask) => (None, vec![ask]),
        Start::Loaded(snap) => {
            apply_snapshot(&snap, g, store);
            // The data files reconcile before the snippets, notes and icon maps are read.
            let location = rp::expanded(&g.string(rp::FOLDER_KEY), &home());
            let asks = if rp::is_url(&location) || rp::is_unc(&location) { Vec::new() } else {
                let base = PathBuf::from(&location);
                let folder = if crate::localfs::is_local_file(&base) { base.parent().map(Path::to_path_buf).unwrap_or(base) } else { base };
                super::data_files_ui::reconcile_at_start(g, &folder)
            };
            (Some(snap), asks)
        }
    }
}

/// Loads a URL with curl.exe from System32, hidden: the body, or curl's exit code and what it
/// wrote to stderr.
#[cfg(windows)]
pub(super) fn curl_fetch(url: &str, timeout: Option<f64>) -> Fetched {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let out = std::process::Command::new(crate::script_provision::RealRunner::system_tool("curl.exe").map_err(|e| (-1, e))?)
        .args(rp::curl_args(url, timeout))
        .stdin(std::process::Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| (-1, e.to_string()))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err((out.status.code().unwrap_or(-1), String::from_utf8_lossy(&out.stderr).trim().to_string()))
    }
}

/// Writes the snapshot through a temporary file and a rename. A network path, or a file reached
/// through a link to one, is never written.
fn write_snapshot(snap: &Snapshot, file: &Path, read_link: &dyn Fn(&Path) -> Option<PathBuf>) -> Result<(), String> {
    if !rp::local_folder(file, read_link) {
        return Err(format!("{}: network paths are not supported", file.display()));
    }
    let mut tmp = file.as_os_str().to_owned();
    tmp.push(".tmp");
    let judged = |p: &Path| crate::localfs::judge_io(p).map_err(|e| format!("{}: {e}", p.display()));
    let (tmp, file) = (judged(&PathBuf::from(tmp))?, judged(file)?);
    crate::localfs::write(&tmp, snap.to_toml()).map_err(|e| format!("{}: {e}", tmp.path().display()))?;
    crate::localfs::rename(&tmp, &file).map_err(|e| format!("{}: {e}", file.path().display()))
}

impl App {
    /// The location as typed and as used (tilde-expanded, a URL as is).
    pub(super) fn prefs_location(&self) -> (String, String) {
        let text = self.globals.string(rp::FOLDER_KEY);
        let location = rp::expanded(&text, &home());
        (text, location)
    }

    /// The settings file in the folder; None for a URL, a blank location, a network path or a
    /// folder or file reached through a link to one, which are never touched as files.
    pub(super) fn prefs_file(&self) -> Option<PathBuf> {
        let (text, location) = self.prefs_location();
        let read_link = crate::netpath::read_reparse;
        if rp::is_url(&location) || text.trim().is_empty() || rp::is_unc(&text) || !rp::local_folder(Path::new(&location), &read_link) {
            return None;
        }
        Some(rp::prefs_file(Path::new(&location), &|p| crate::localfs::is_local_file(p))).filter(|file| rp::local_folder(file, &read_link))
    }

    /// The local settings as the folder would hold them.
    fn prefs_local(&self) -> Snapshot {
        Snapshot::from_local(&self.globals.values, &self.store.saved_default_guid(), &self.store.file_profiles())
    }

    /// A setting or profile was saved (`RP:1460-1478`): with "Automatically", a folder, and a
    /// difference from what the folder held (or nothing held), the folder is written half a
    /// second later.
    pub(super) fn prefs_changed(&mut self) {
        // The cached pointer shape may depend on a preference (UseSystemCursorWhenPossible):
        // forget it in every window so the next move redraws it.
        for g in self.gpu.iter_mut().chain(self.windows.parked_mut().filter_map(|(_, s)| s.gpu.as_mut())) {
            g.shape = None;
        }
        self.prefs_note_location();
        // Copy options ride on each screen so every reader of the selection sees them.
        let copy = crate::seltext::Options::from_globals(&self.globals);
        for pane in self.panes.values_mut() {
            pane.set_copy_options(copy);
        }
        let g = &self.globals;
        if !g.bool(rp::LOAD_KEY) || rp::save_mode(g.bool(rp::HAVE_SELECTION_KEY), g.int(rp::SELECTION_KEY)) != rp::SaveMode::Always || self.prefs_save_due.is_some() {
            return;
        }
        let (text, location) = self.prefs_location();
        if rp::is_url(&location) || text.trim().is_empty() || rp::is_unc(&text) || rp::is_unc(&location) {
            return;
        }
        if self.prefs_saved.as_ref().is_some_and(|saved| !saved.is_empty() && !rp::differs(saved, &self.prefs_local())) {
            return;
        }
        self.prefs_save_due = Some(Instant::now() + SAVE_DELAY);
    }

    /// `saveIfNeeded` (`RP:501-525`): the automatic save writes without asking and says nothing
    /// when it fails.
    fn prefs_autosave(&mut self) {
        let Some(file) = self.prefs_file() else { return };
        let snap = self.prefs_local();
        if write_snapshot(&snap, &file, &crate::netpath::read_reparse).is_ok() {
            self.prefs_saved = Some(snap);
            if let Some(folder) = self.data_folder() {
                self.data_files_push(&folder, false);
            }
        }
    }

    /// Writes the folder, or says it could not (`RP:376-386`). iTerm2 names the file twice.
    fn prefs_write(&mut self, interactive: bool) {
        if !self.prefs_write_settings() {
            return;
        }
        // The data files go with the settings.
        if let Some(folder) = self.data_folder() {
            self.data_files_push(&folder, interactive);
        }
    }

    fn prefs_write_settings(&mut self) -> bool {
        let snap = self.prefs_local();
        let file = self.prefs_file();
        if let Some(f) = &file {
            if write_snapshot(&snap, f, &crate::netpath::read_reparse).is_ok() {
                self.prefs_saved = Some(snap);
                return true;
            }
        }
        let shown = file.map_or_else(|| self.prefs_location().1, |f| f.display().to_string());
        self.ask_scripts(ask(rp::FAILED_COPY, &rp::tried_to_copy(&shown, &shown), &[crate::api_auth::OK], None, Kind::Persistent, Then::Nothing));
        false
    }

    /// `remotePrefsHaveChanged` (`RP:1625-1644`): the folder's file no longer holds what was
    /// loaded or last written. False when nothing was.
    fn prefs_remote_changed(&self) -> bool {
        let (Some(saved), Some(file)) = (&self.prefs_saved, self.prefs_file()) else { return false };
        let fresh = crate::localfs::judge_io(file).and_then(|f| crate::localfs::read_to_string(&f)).ok().and_then(|t| Snapshot::parse(&t));
        fresh.as_ref() != Some(saved)
    }

    /// Save Now, the Copy answers and the save at quit (`saveLocalUserDefaultsToRemotePrefs`,
    /// `RP:335-392`). `interactive` lets the Data folder push reconcile (and ask) when the folder
    /// moved since the last sync.
    pub(super) fn prefs_save_now(&mut self, interactive: bool) {
        let (text, location) = self.prefs_location();
        let url = rp::is_url(&location);
        match save_plan(url, !url && self.prefs_remote_changed()) {
            SavePlan::CannotCopyToUrl => {
                let ask = ask(rp::CANNOT_COPY_URL, rp::CANNOT_COPY_URL_DETAIL, &[crate::api_auth::OK], None, Kind::Persistent, Then::Nothing);
                self.ask_scripts(ask);
            }
            SavePlan::AskOverwrite => self.ask_scripts(overwrite_ask(&text)),
            SavePlan::Write => self.prefs_write(interactive),
        }
    }

    /// "Copy local settings to custom folder now?" (`GP:4276-4284`), after a usable folder was
    /// picked with the checkbox.
    pub(super) fn prefs_offer_copy(&mut self) {
        let buttons = [rp::COPY, rp::DONT_COPY];
        self.ask_scripts(ask(rp::COPY_NOW, "", &buttons, None, Kind::Persistent, Then::PrefsCopyNow));
    }

    /// The location's settings replace the running ones, as an import does.
    pub(super) fn prefs_apply(&mut self, snap: Snapshot) {
        // Edits still waiting for their file go first: the snapshot replaces the list under them.
        self.flush_dynamic_writes();
        apply_snapshot(&snap, &mut self.globals, &mut self.store);
        self.workgroup_model = crate::workgroup::Model::load(&self.globals).0;
        self.reload_dynamic_profiles();
        self.bonjour_profiles_replaced();
        let n = snap.profiles.len();
        self.prefs_saved = Some(snap);
        self.apply_edit(Edit::Imported(n));
        self.globals_changed(true);
    }

    /// Loads the URL off the UI thread; `prefs_tick` takes the result.
    fn prefs_fetch_start(&mut self, url: String, timeout: Option<f64>) {
        let (tx, rx) = std::sync::mpsc::channel();
        let notify = self.notify.clone();
        std::thread::spawn(move || {
            let _ = tx.send(curl_fetch(&url, timeout));
            notify();
        });
        self.prefs_fetch = Some(rx);
    }

    /// A fetch finished: applied, or its failure asks again.
    fn prefs_fetched(&mut self, result: Fetched) {
        let (text, location) = self.prefs_location();
        match fetched(&text, &location, result) {
            Start::Loaded(snap) => self.prefs_apply(snap),
            Start::Ask(ask) => self.ask_scripts(ask),
            Start::Off => {}
        }
    }

    /// Acts on an answer to one of the settings folder's questions.
    pub(super) fn prefs_answered(&mut self, then: Then, index: usize) {
        match (then, index) {
            (Then::PrefsLocationNotSet, 0) => {
                self.globals.set(rp::LOAD_KEY, Value::Boolean(false));
                self.globals_changed(true);
            }
            (Then::PrefsFailedUrl { .. }, 1) => self.prefs_reveal = true,
            (Then::PrefsFailedUrl { url, .. }, 2) => self.prefs_fetch_start(url, None),
            (Then::PrefsUrlPrompt(_), 1) => {
                self.globals.set(rp::LOAD_KEY, Value::Boolean(false));
                self.globals.unset(rp::PROMPT_URL_KEY);
                self.globals_changed(true);
            }
            (Then::PrefsUrlPrompt(url), 2) => {
                self.globals.unset(rp::PROMPT_URL_KEY);
                self.save_globals();
                let t = timeout(&self.globals);
                self.prefs_fetch_start(url, Some(t));
            }
            (Then::PrefsOverwrite, 0) => self.prefs_write(true),
            (Then::PrefsQuit, 0) => self.prefs_save_now(false),
            (Then::PrefsQuit, 1) => {
                if let Some(folder) = self.data_folder().filter(|_| self.data_files_differ()) {
                    self.data_files_discard(&folder);
                }
            }
            (Then::PrefsQuitData, 0) => {
                if let Some(folder) = self.data_folder() {
                    self.data_files_push(&folder, false);
                }
            }
            (Then::PrefsQuitData, 1) => {
                if let Some(folder) = self.data_folder() {
                    self.data_files_discard(&folder);
                }
            }
            (Then::PrefsCopyNow, 0) => self.prefs_save_now(true),
            _ => {}
        }
    }

    /// Whether an alert the exit waits for is showing or queued.
    fn prefs_quit_question_open(&self) -> bool {
        exit_waits(self.scripts_alert.as_ref().map(|(_, a)| a), &self.scripts_alerts)
    }

    /// Notes an edit of the location since launch (`customFolderChanged`).
    fn prefs_note_location(&mut self) {
        let now = self.globals.string(rp::FOLDER_KEY);
        self.prefs_launch_folder = launch_location_after(self.prefs_launch_folder.take(), &now);
    }

    /// `applicationWillTerminate` (`RP:1646-1744`), the last step before Tandem ends. True: end
    /// now. False: the step saved (Automatically) or asked (When Quitting, Manually), and an
    /// alert is showing or queued; `prefs_tick` ends Tandem once it and any alert after it are
    /// answered. Only those two branches wait: the others (nothing to do, remembering or
    /// clearing the URL) end at once, whatever alerts are queued. Skipped, ending at once, when
    /// the location was edited this session (`iTermApplicationDelegate.m:1115-1117`).
    pub(super) fn prefs_before_exit(&mut self) -> bool {
        self.prefs_note_location();
        if self.prefs_launch_folder.is_none() {
            return true;
        }
        let g = &self.globals;
        let on = g.bool(rp::LOAD_KEY);
        let mode = rp::save_mode(g.bool(rp::HAVE_SELECTION_KEY), g.int(rp::SELECTION_KEY));
        let (text, location) = self.prefs_location();
        let differs = on && self.prefs_saved.as_ref().is_some_and(|saved| rp::differs(saved, &self.prefs_local()));
        let data = on && !rp::is_url(&location) && self.data_files_differ();
        let step = rp::quit_step(on, rp::is_url(&location), differs, mode, self.prefs_saved.is_some());
        // Only the data changed: push just them, or ask with the data-only text.
        if data && !differs && matches!(step, QuitStep::Nothing | QuitStep::ClearUrlPrompt) {
            if matches!(step, QuitStep::ClearUrlPrompt) {
                self.globals.unset(rp::PROMPT_URL_KEY);
                self.save_globals();
            }
            if let Some(folder) = self.data_folder() {
                if mode == rp::SaveMode::Always {
                    self.data_files_push(&folder, false);
                    return true;
                }
                self.ask_scripts(data_quit_ask(&folder.display().to_string()));
                self.prefs_exit_pending = true;
                return false;
            }
            return true;
        }
        match step {
            QuitStep::Nothing => return true,
            QuitStep::RememberUrl => {
                self.globals.set(rp::PROMPT_URL_KEY, Value::String(location));
                self.save_globals();
                return true;
            }
            QuitStep::ClearUrlPrompt => {
                self.globals.unset(rp::PROMPT_URL_KEY);
                self.save_globals();
                return true;
            }
            QuitStep::SaveSilently => self.prefs_save_now(false),
            QuitStep::Ask => self.ask_scripts(quit_ask(&text, data)),
        }
        if self.prefs_quit_question_open() {
            self.prefs_exit_pending = true;
            return false;
        }
        true
    }

    /// Each pass of the event loop: a finished URL load, the automatic save when due, Reveal in
    /// Settings, and the exit that waited for the quit question.
    pub(super) fn prefs_tick(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(rx) = &self.prefs_fetch {
            match rx.try_recv() {
                Ok(result) => {
                    self.prefs_fetch = None;
                    self.prefs_fetched(result);
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                Err(std::sync::mpsc::TryRecvError::Disconnected) => self.prefs_fetch = None,
            }
        }
        if self.prefs_save_due.is_some_and(|t| Instant::now() >= t) {
            self.prefs_save_due = None;
            self.prefs_autosave();
        }
        if std::mem::take(&mut self.prefs_reveal) {
            self.open_settings_page(event_loop, crate::settings_ui::Page::General);
        }
        if self.prefs_exit_pending && !self.prefs_quit_question_open() {
            self.prefs_exit_pending = false;
            self.catalog_flush();
            event_loop.exit();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    fn map(pairs: &[(&str, Value)]) -> std::collections::BTreeMap<String, Value> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.clone())).collect()
    }

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tandem-prefs-ui-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Local settings with the setting on at `location`, a local syncable key, a NoSync key, and
    /// one local profile "Local".
    fn local(dir: &Path, location: &str) -> (Globals, Store) {
        let mut g = Globals::empty(&dir.join("settings.toml"));
        g.set(rp::LOAD_KEY, Value::Boolean(true));
        g.set(rp::FOLDER_KEY, Value::String(location.to_string()));
        g.values.insert("TmuxTitlePrefix".into(), Value::String("local".into()));
        g.values.insert("NoSyncX".into(), Value::Boolean(true));
        let mut store = Store::fresh(&dir.join("profiles.toml"));
        store.profiles[0].set(crate::profile::NAME, Value::String("Local".into()));
        (g, store)
    }

    fn remote() -> Snapshot {
        let profile = map(&[("Guid", Value::String("g1".into())), ("Name", Value::String("Remote".into()))]);
        Snapshot { settings: map(&[("FocusFollowsMouse", Value::Boolean(true))]), default: "g1".into(), profiles: vec![profile] }
    }

    /// 13y2 Task 9b review: a snapshot written by a Tandem before the rename sweep (old
    /// lower-case Advanced keys) lands under the capitalized keys.
    #[test]
    fn an_older_snapshot_with_old_keys_migrates() {
        let dir = temp("older-snapshot");
        let (mut g, mut store) = local(&dir, "x");
        let mut snap = remote();
        snap.settings.insert("disableTopRightIndicators".into(), Value::Boolean(true)); // old key on purpose
        apply_snapshot(&snap, &mut g, &mut store);
        assert_eq!(g.values.get("DisableTopRightIndicators"), Some(&Value::Boolean(true)));
        assert!(!g.values.contains_key("disableTopRightIndicators")); // old key on purpose
    }

    fn names(store: &Store) -> Vec<String> {
        store.profiles.iter().map(|p| p.name()).collect()
    }

    fn buttons(ask: &Ask) -> Vec<&str> {
        ask.spec.labels.iter().map(String::as_str).collect()
    }

    fn never(_: &str, _: Option<f64>) -> Fetched {
        panic!("nothing is fetched")
    }

    fn assert_applied(g: &Globals, store: &Store, location: &str) {
        assert_eq!(g.values.get("FocusFollowsMouse"), Some(&Value::Boolean(true)));
        assert!(!g.values.contains_key("TmuxTitlePrefix"), "a local syncable key goes");
        assert_eq!(g.values.get("NoSyncX"), Some(&Value::Boolean(true)));
        assert!(g.bool(rp::LOAD_KEY));
        assert_eq!(g.string(rp::FOLDER_KEY), location);
        assert_eq!(names(store), ["Remote"]);
        assert_eq!(store.default_guid, "g1");
    }

    fn assert_untouched(g: &Globals, store: &Store) {
        assert_eq!(g.values.get("TmuxTitlePrefix"), Some(&Value::String("local".into())));
        assert!(!g.values.contains_key("FocusFollowsMouse"));
        assert_eq!(names(store), ["Local"]);
    }

    /// Controller ruling I5: the AI consent switches (`ai_settings::guards_consent`) keep their
    /// local value when a custom folder or URL carries them, and the kept value is no local
    /// change to save.
    #[test]
    fn load_keeps_the_ai_consent_switches() {
        use crate::ai_settings as ai;
        let dir = temp("ai");
        let folder = dir.join("T");
        std::fs::create_dir_all(&folder).unwrap();
        let mut snap = remote();
        for key in [ai::ADMIN, ai::ENABLE_AI, ai::AI_COMPLETIONS] {
            snap.settings.insert(key.into(), Value::Boolean(key != ai::ADMIN));
        }
        for (key, _) in ai::PERMISSIONS {
            snap.settings.insert(key.into(), Value::Integer(1));
        }
        std::fs::write(folder.join(rp::PREFS_FILE), snap.to_toml()).unwrap();
        let t = folder.display().to_string();
        let (mut g, mut store) = local(&dir, &t);
        g.values.insert(ai::PERMISSIONS[1].0.into(), Value::Integer(2));
        let (loaded, asks) = load_at_start(&mut g, &mut store, &never);
        assert!(asks.is_empty());
        assert_applied(&g, &store, &t);
        for key in [ai::ADMIN, ai::ENABLE_AI, ai::AI_COMPLETIONS] {
            assert!(!g.values.contains_key(key), "{key} came from the folder");
        }
        for (i, (key, _)) in ai::PERMISSIONS.iter().enumerate() {
            let want = (i == 1).then_some(Value::Integer(2));
            assert_eq!(g.values.get(*key), want.as_ref(), "{key}");
        }
        let now = Snapshot::from_local(&g.values, &store.default_guid, &store.profiles.iter().map(|p| p.values.clone()).collect::<Vec<_>>());
        assert!(!rp::differs(&loaded.unwrap(), &now), "a kept switch is no local change");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_at_start_follows_iterm2() {
        let dir = temp("load");
        let folder = dir.join("T");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join(rp::PREFS_FILE), remote().to_toml()).unwrap();
        let t = folder.display().to_string();

        // Off: nothing read.
        let (mut g, mut store) = local(&dir, &t);
        g.set(rp::LOAD_KEY, Value::Boolean(false));
        let (snap, asks) = load_at_start(&mut g, &mut store, &never);
        assert!(snap.is_none() && asks.is_empty());
        assert_eq!(names(&store), ["Local"]);

        // A folder holding the file: its settings replace the syncable ones.
        let (mut g, mut store) = local(&dir, &t);
        let (snap, asks) = load_at_start(&mut g, &mut store, &never);
        assert!(asks.is_empty());
        assert_eq!(snap, Snapshot::parse(&std::fs::read_to_string(folder.join(rp::PREFS_FILE)).unwrap()));
        assert_eq!(snap, Some(remote()));
        assert_applied(&g, &store, &t);

        // No location.
        let (mut g, mut store) = local(&dir, "");
        let (snap, asks) = load_at_start(&mut g, &mut store, &never);
        assert!(snap.is_none());
        assert_eq!(asks.len(), 1);
        assert_eq!((asks[0].spec.heading.as_str(), asks[0].spec.text.as_str()), (rp::ERROR_LOADING, rp::LOCATION_NOT_SET));
        assert_eq!(buttons(&asks[0]), ["Don\u{2019}t Load Remote Settings", "Cancel"]);
        assert_eq!(asks[0].then, Then::PrefsLocationNotSet);
        assert_untouched(&g, &store);

        // A folder with no file falls back.
        let empty = dir.join("E");
        std::fs::create_dir_all(&empty).unwrap();
        let e = empty.display().to_string();
        let (mut g, mut store) = local(&dir, &e);
        let (snap, asks) = load_at_start(&mut g, &mut store, &never);
        assert!(snap.is_none());
        assert_eq!(asks.len(), 1);
        assert_eq!(asks[0].spec.heading, "Failed to load settings from custom directory. Falling back to local copy.");
        assert_eq!(asks[0].spec.text, format!("Missing or malformed file at \u{201c}{e}\u{201d}"));
        assert_eq!(buttons(&asks[0]), ["OK"]);
        assert_eq!(asks[0].then, Then::Nothing);
        assert_untouched(&g, &store);

        // A URL: fetched with the timeout, then applied.
        let url = "https://h/p";
        let calls = RefCell::new(Vec::new());
        let answer = |u: &str, t: Option<f64>| {
            calls.borrow_mut().push((u.to_string(), t));
            Ok(remote().to_toml())
        };
        let (mut g, mut store) = local(&dir, url);
        let (snap, asks) = load_at_start(&mut g, &mut store, &answer);
        assert!(asks.is_empty());
        assert_eq!(snap, Some(remote()));
        assert_eq!(*calls.borrow(), [(url.to_string(), Some(5.0))]);
        assert_applied(&g, &store, url);

        // A timeout offers to try again.
        let (mut g, mut store) = local(&dir, url);
        let (snap, asks) = load_at_start(&mut g, &mut store, &|_, _| Err((28, "timed out".into())));
        assert!(snap.is_none());
        assert_eq!(asks.len(), 1);
        assert_eq!((asks[0].spec.heading.as_str(), asks[0].spec.text.as_str()), (rp::FAILED_URL, "HTTP request failed: timed out"));
        assert_eq!(asks[0].then, Then::PrefsFailedUrl { url: url.into(), timed_out: true });
        assert_eq!(buttons(&asks[0]), ["OK", "Reveal in Settings", "Try Again Without Timeout"]);
        assert_untouched(&g, &store);

        // Any other failure does not.
        let (mut g, mut store) = local(&dir, url);
        let (_, asks) = load_at_start(&mut g, &mut store, &|_, _| Err((6, "could not resolve".into())));
        assert_eq!(asks[0].then, Then::PrefsFailedUrl { url: url.into(), timed_out: false });
        assert_eq!(buttons(&asks[0]), ["OK", "Reveal in Settings"]);
        assert_untouched(&g, &store);

        // The URL remembered at the last quit asks first.
        let (mut g, mut store) = local(&dir, url);
        g.set(rp::PROMPT_URL_KEY, Value::String(url.into()));
        let (snap, asks) = load_at_start(&mut g, &mut store, &never);
        assert!(snap.is_none());
        assert_eq!(asks.len(), 1);
        assert_eq!(asks[0].spec.text, rp::URL_PROMPT);
        assert_eq!(asks[0].then, Then::PrefsUrlPrompt(url.into()));
        assert_eq!(buttons(&asks[0]), ["Keep Local Changes", "Disable Loading from URL", "Discard Local Changes"]);
        assert!(asks[0].spec.id.is_none());
        assert_untouched(&g, &store);

        // A network folder is never read.
        let (mut g, mut store) = local(&dir, r"\\host\share");
        let (snap, asks) = load_at_start(&mut g, &mut store, &never);
        assert!(snap.is_none());
        assert_eq!(asks[0].spec.heading, rp::FAILED_FOLDER);
        assert_untouched(&g, &store);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn quit_skips_an_edited_location() {
        let launch = Some("C:\\A".to_string());
        assert_eq!(launch_location_after(launch.clone(), "C:\\A"), launch);
        assert_eq!(launch_location_after(launch.clone(), "C:\\B"), None);
        assert_eq!(launch_location_after(None, "C:\\A"), None, "typed back, still changed");
        assert_eq!(launch_location_after(Some(String::new()), ""), Some(String::new()));
    }

    #[test]
    fn exit_waits_for_any_alert() {
        let failed = || ask(rp::FAILED_COPY, &rp::tried_to_copy("f", "f"), &[crate::api_auth::OK], None, Kind::Persistent, Then::Nothing);
        let mut queued = std::collections::VecDeque::new();
        assert!(!exit_waits(None, &queued));
        assert!(exit_waits(Some(&failed()), &queued), "the failure to copy is seen before the exit");
        queued.push_back(failed());
        assert!(exit_waits(None, &queued));
    }

    /// Task 2 review finding 1 (ruling: the source wins): the Save changes popup's Manually
    /// saves the quit question's answer as Lose Changes, which is never remembered, so the
    /// question still shows at quit; When Quitting (Copy) answers it at once.
    #[test]
    fn a_silenced_quit_question_saved_as_lose_changes_is_shown() {
        use crate::warnings::{preempt, Preempt};
        let mut g = Globals::empty(Path::new("settings.toml"));
        let q = quit_ask("C:\\T", false);
        assert_eq!(q.spec.never_remember, [1]);
        g.set(rp::HAVE_SELECTION_KEY, Value::Boolean(true));
        g.set(rp::SELECTION_KEY, Value::Integer(1));
        assert_eq!(preempt(&q.spec, &g, 0.0, false), Preempt::Show, "Manually asks at quit");
        g.set(rp::SELECTION_KEY, Value::Integer(0));
        assert_eq!(preempt(&q.spec, &g, 0.0, false), Preempt::Answer(0), "When Quitting copies without asking");
    }

    #[test]
    fn save_now_follows_iterm2() {
        assert_eq!(save_plan(true, false), SavePlan::CannotCopyToUrl);
        assert_eq!(save_plan(true, true), SavePlan::CannotCopyToUrl);
        assert_eq!(save_plan(false, true), SavePlan::AskOverwrite);
        assert_eq!(save_plan(false, false), SavePlan::Write);
        let ask = overwrite_ask(r"D:\s");
        assert_eq!(ask.spec.text, "Settings at D:\\s changed since Tandem started. Overwrite it?");
        assert_eq!(buttons(&ask), ["Overwrite", "Discard Local Changes"]);
        assert_eq!(ask.then, Then::PrefsOverwrite);
        assert!(ask.spec.id.is_none());
    }

    #[test]
    fn written_snapshots_read_back() {
        let dir = temp("write");
        let file = dir.join(rp::PREFS_FILE);
        write_snapshot(&remote(), &file, &|_| None).unwrap();
        assert_eq!(Snapshot::parse(&std::fs::read_to_string(&file).unwrap()), Some(remote()));
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1, "the temporary file is renamed");
        assert!(write_snapshot(&remote(), Path::new(r"\\host\share\tandem-settings.toml"), &|_| None).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_folder_linked_to_a_network_path_is_never_read_or_written() {
        // The final review's Important 2 (invariant 5): the folder holds a good file, but it is
        // reached through a link to \\h\s, so the launch falls back without reading it and the
        // save writes nothing.
        let dir = temp("linked");
        let folder = dir.join("T");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join(rp::PREFS_FILE), remote().to_toml()).unwrap();
        let t = folder.display().to_string();
        let to_share = |p: &Path| (p == folder.as_path()).then(|| PathBuf::from(r"\\h\s"));
        let (g, _) = local(&dir, &t);
        match start(&g, Path::new(r"C:\Users\u"), &to_share, &never) {
            Start::Ask(ask) => {
                assert_eq!((ask.spec.heading.as_str(), ask.spec.text.as_str()), (rp::FAILED_FOLDER, rp::missing_file(&t).as_str()));
                assert_eq!(buttons(&ask), ["OK"]);
            }
            _ => panic!("a folder linked to a network path is never read"),
        }
        // The same file named directly, and a link on the file itself.
        let named = folder.join(rp::PREFS_FILE);
        let (g, _) = local(&dir, &named.display().to_string());
        assert!(matches!(start(&g, Path::new(r"C:\Users\u"), &to_share, &never), Start::Ask(_)));
        let file_to_share = |p: &Path| (p == named.as_path()).then(|| PathBuf::from(r"\\h\s\f.toml"));
        let (g, _) = local(&dir, &t);
        assert!(matches!(start(&g, Path::new(r"C:\Users\u"), &file_to_share, &never), Start::Ask(_)));
        // With no link the same folder loads.
        assert!(matches!(start(&g, Path::new(r"C:\Users\u"), &|_| None, &never), Start::Loaded(_)));

        let before = std::fs::read_to_string(&named).unwrap();
        let other = Snapshot { default: "g2".into(), ..remote() };
        assert!(write_snapshot(&other, &named, &to_share).is_err());
        assert!(write_snapshot(&other, &named, &file_to_share).is_err());
        assert_eq!(std::fs::read_to_string(&named).unwrap(), before, "nothing written");
        assert_eq!(std::fs::read_dir(&folder).unwrap().count(), 1, "no temporary file either");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A snapshot loaded from a custom folder or URL never brings a network start folder in.
    #[test]
    fn a_loaded_snapshot_loses_network_folders() {
        let dir = std::env::temp_dir().join(format!("tandem-prefs-unc-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (mut g, mut store) = local(&dir, "x");
        let profile = map(&[("Guid", Value::String("g1".into())), ("Name", Value::String("R".into())), ("Working Directory", Value::String(" \\\\evil\\share".into())), ("AWDS Tab Directory", Value::String("D:\\ok".into()))]);
        let snap = Snapshot { settings: map(&[]), default: "g1".into(), profiles: vec![profile] };
        apply_snapshot(&snap, &mut g, &mut store);
        assert_eq!(store.profiles[0].string("Working Directory"), "");
        assert_eq!(store.profiles[0].string("AWDS Tab Directory"), "D:\\ok");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Final review 2: a run-time load writes pending dynamic edits first, applies the snapshot,
    /// then reloads the Dynamic Profiles folder once.
    #[test]
    fn prefs_apply_flushes_then_applies_then_reloads_the_folder() {
        let src = include_str!("prefs_ui.rs");
        let from = src.find("pub(super) fn prefs_apply(").expect("prefs_apply");
        let body = &src[from..from + src[from..].find("\n    }\n").expect("end")];
        let at = |n: &str| body.find(n).unwrap_or_else(|| panic!("prefs_apply calls {n}"));
        assert!(at("self.flush_dynamic_writes();") < at("apply_snapshot(&snap"));
        assert!(at("apply_snapshot(&snap") < at("self.reload_dynamic_profiles();"));
    }

    /// Final review 2: loading remote settings while running keeps the Dynamic Profiles in the
    /// list (the snapshot never holds them); a Guid the snapshot also holds goes to the snapshot.
    #[test]
    fn a_loaded_snapshot_keeps_the_running_dynamic_profiles() {
        let dir = std::env::temp_dir().join(format!("tandem-prefs-keepdyn-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (mut g, mut store) = local(&dir, "x");
        for guid in ["dyn1", "clash"] {
            let mut p = Profile::new("D");
            p.set("Guid", Value::String(guid.into()));
            p.set("Is Dynamic Profile", Value::Boolean(true));
            store.profiles.push(p);
        }
        let snap = Snapshot { settings: map(&[]), default: "s1".into(), profiles: vec![map(&[("Guid", Value::String("s1".into())), ("Name", Value::String("S".into()))]), map(&[("Guid", Value::String("clash".into())), ("Name", Value::String("C".into()))])] };
        apply_snapshot(&snap, &mut g, &mut store);
        assert_eq!(store.dynamic_guids(), ["dyn1"], "the running dynamic profile survives; a clash goes to the snapshot");
        assert!(store.unsaved("dyn1"));
        assert_eq!(store.file_profiles().len(), 2);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Trust ruling: whatever a remote snapshot writes into the workgroups is marked Imported, and
    /// it cannot carry its own marks in (13y1 Task 1 fix round 1).
    #[test]
    fn a_loaded_snapshot_marks_its_workgroups_imported() {
        use crate::workgroup::{self as wg, Source};
        let dir = std::env::temp_dir().join(format!("tandem-prefs-wg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (mut g, mut store) = local(&dir, "x");
        let mut m = wg::Model::default();
        m.add(wg::Workgroup::new_empty("mine"));
        m.save(&mut g);
        let mine = m.workgroups[0].unique_identifier.clone();
        let other = wg::Workgroup::new_empty("remote");
        let other_id = other.unique_identifier.clone();
        let snap = Snapshot {
            settings: map(&[(wg::KEY, Value::String(wg::encode(&[other]))), (wg::KEY_IMPORTED, Value::String(String::new()))]),
            default: "s1".into(),
            profiles: vec![map(&[("Guid", Value::String("s1".into())), ("Name", Value::String("S".into()))])],
        };
        apply_snapshot(&snap, &mut g, &mut store);
        let (back, _) = wg::Model::load(&g);
        assert_eq!(back.workgroups.len(), 1);
        assert_eq!(back.source_of(&other_id), Source::Import);
        assert!(back.get(&mine).is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Task 5 fix r2 item 4: remote settings at launch and Import All never bring the Dynamic
    /// Profiles loader's marks in (they would keep a profile out of profiles.toml).
    #[test]
    fn a_loaded_snapshot_loses_the_dynamic_marks() {
        let dir = std::env::temp_dir().join(format!("tandem-prefs-dyn-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (mut g, mut store) = local(&dir, "x");
        let mut values = vec![("Guid", Value::String("g1".into())), ("Name", Value::String("R".into()))];
        for key in crate::dynamic_profiles::INTERNAL_KEYS {
            values.push((key, if key == "Is Dynamic Profile" || key == "Rewritable" { Value::Boolean(true) } else { Value::String("x".into()) }));
        }
        let snap = Snapshot { settings: map(&[]), default: "g1".into(), profiles: vec![map(&values)] };
        apply_snapshot(&snap, &mut g, &mut store);
        for key in crate::dynamic_profiles::INTERNAL_KEYS {
            assert!(store.profiles[0].get(key).is_none(), "{key} came in");
        }
        assert!(store.dynamic_guids().is_empty());
        assert_eq!(store.file_profiles().len(), 1, "the profile is saved like any other");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A loaded snapshot's custom color presets are bounded and sanitized as Settings writes are.
    #[test]
    fn a_loaded_snapshot_bounds_custom_color_presets() {
        let dir = std::env::temp_dir().join(format!("tandem-prefs-presets-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (mut g, mut store) = local(&dir, "x");
        let color = Value::Table(map(&[("r", Value::Float(0.1)), ("g", Value::Float(0.2)), ("b", Value::Float(0.3))]).into_iter().collect());
        let bad = Value::Table(map(&[("r", Value::Float(f64::NAN)), ("g", Value::Float(0.2)), ("b", Value::Float(0.3))]).into_iter().collect());
        let mut presets = toml::Table::new();
        for i in 0..400 {
            presets.insert(format!("P{i:03}"), Value::Table([("Ansi 0 Color".to_string(), color.clone())].into_iter().collect()));
        }
        presets.insert("A bad".into(), Value::Table([("Ansi 0 Color".to_string(), bad)].into_iter().collect()));
        let profile = map(&[("Guid", Value::String("g1".into())), ("Name", Value::String("R".into()))]);
        let snap = Snapshot { settings: map(&[(crate::color_presets::CUSTOM_KEY, Value::Table(presets))]), default: "g1".into(), profiles: vec![profile] };
        apply_snapshot(&snap, &mut g, &mut store);
        let stored = g.values[crate::color_presets::CUSTOM_KEY].as_table().unwrap();
        assert_eq!(stored.len(), crate::color_presets::MAX_CUSTOM);
        assert!(!stored.contains_key("A bad"));
        assert_eq!(crate::color_presets::custom(&g).len(), crate::color_presets::MAX_CUSTOM);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Task 4 ruling: and no network log, archive, icon or background image path either.
    #[test]
    fn a_loaded_snapshot_loses_every_network_path() {
        let dir = std::env::temp_dir().join(format!("tandem-prefs-unc2-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (mut g, mut store) = local(&dir, "x");
        let keys = ["Log Directory", "Archive Directory", "Custom Icon Path", "Background Image Location", "Background Image Folder Location"];
        let mut pairs = vec![("Guid", Value::String("g1".into())), ("Name", Value::String("R".into()))];
        pairs.extend(keys.iter().map(|k| (*k, Value::String("\t\\\\evil\\share".into()))));
        let snap = Snapshot { settings: map(&[]), default: "g1".into(), profiles: vec![map(&pairs)] };
        apply_snapshot(&snap, &mut g, &mut store);
        for k in keys {
            assert_eq!(store.profiles[0].string(k), "", "{k}");
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
