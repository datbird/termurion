//! "Load settings from a custom folder or URL", the app side (iTerm2's `iTermRemotePreferences`,
//! `RP`, and `GP:4262-4292`): the load at launch, the automatic save, Save Now, and the question
//! at quit. The model is `crate::remote_prefs`; the rows are `settings_keys::prefs_rows`.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use toml::Value;
use winit::event_loop::ActiveEventLoop;

use super::App;
use super::scripts_alerts::{Ask, Then};
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

fn ok() -> Vec<String> {
    vec![crate::api_auth::OK.to_string()]
}

/// "Failed to load settings from custom directory. ..." (`RP:304-310`): the detail names the
/// location as typed.
fn failed_folder(text: &str) -> Ask {
    Ask { heading: rp::FAILED_FOLDER.to_string(), text: rp::missing_file(text), buttons: ok(), silence: None, then: Then::Nothing }
}

/// "Error Loading Settings" (`RP:295-303`).
fn location_not_set() -> Ask {
    let buttons = vec![rp::DONT_LOAD.to_string(), crate::api_auth::CANCEL.to_string()];
    Ask { heading: rp::ERROR_LOADING.to_string(), text: rp::LOCATION_NOT_SET.to_string(), buttons, silence: None, then: Then::PrefsLocationNotSet }
}

/// The remembered URL's question (`RP:241-272`), an iTerm2 warning that cannot be silenced.
fn url_prompt(url: &str) -> Ask {
    let buttons = [rp::KEEP_LOCAL, rp::DISABLE_URL, rp::DISCARD_LOCAL].map(String::from).to_vec();
    Ask { heading: crate::script_new::WARNING.to_string(), text: rp::URL_PROMPT.to_string(), buttons, silence: None, then: Then::PrefsUrlPrompt(url.to_string()) }
}

/// `didFailToLoadFromURL:withError:` (`RP:209-232`): curl's exit code 28 is the timeout.
fn failed_url(url: &str, code: i32, error: &str) -> Ask {
    let timed_out = code == rp::CURL_TIMED_OUT;
    let mut buttons = vec![crate::api_auth::OK.to_string(), rp::REVEAL_IN_SETTINGS.to_string()];
    if timed_out {
        buttons.push(rp::TRY_AGAIN.to_string());
    }
    Ask { heading: rp::FAILED_URL.to_string(), text: rp::http_failed(error), buttons, silence: None, then: Then::PrefsFailedUrl { url: url.to_string(), timed_out } }
}

/// "Settings at ... changed since Tandem started. Overwrite it?" (`RP:338-347`), naming the
/// location as typed; it cannot be silenced.
pub(super) fn overwrite_ask(text: &str) -> Ask {
    let buttons = vec![rp::OVERWRITE.to_string(), rp::DISCARD_LOCAL.to_string()];
    Ask { heading: crate::script_new::WARNING.to_string(), text: rp::remote_changed(text), buttons, silence: None, then: Then::PrefsOverwrite }
}

/// The URL's text as loaded: the snapshot, or the failure (an empty or malformed file falls
/// back with the folder's alert, as iTerm2's empty dictionary does, `RP:293-311`).
fn fetched(text: &str, url: &str, result: Fetched) -> Start {
    match result {
        Ok(body) => Snapshot::parse(&body).map_or_else(|| Start::Ask(failed_folder(text)), Start::Loaded),
        Err((code, error)) => Start::Ask(failed_url(url, code, &error)),
    }
}

/// What the launch finds at the location (`freshCopyOfRemotePreferences`). A network path is
/// never read; an empty location reads nothing.
fn start(g: &Globals, home: &Path, fetch: &dyn Fn(&str, Option<f64>) -> Fetched) -> Start {
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
    if rp::is_unc(&text) || rp::is_unc(&location) {
        return Start::Ask(failed_folder(&text));
    }
    let file = rp::prefs_file(Path::new(&location), &|p| p.is_file());
    match std::fs::read_to_string(&file).ok().and_then(|t| Snapshot::parse(&t)) {
        Some(snap) => Start::Loaded(snap),
        None => Start::Ask(failed_folder(&text)),
    }
}

/// `copyRemotePrefsToLocalUserDefaultsPreserving:` (`RP:1531-1551`): every local syncable
/// setting goes and the snapshot's come; the profiles become the snapshot's (one new "Default"
/// when it has none, as Tandem always has a profile).
pub(super) fn apply_snapshot(snap: &Snapshot, g: &mut Globals, store: &mut Store) {
    rp::apply(snap, &mut g.values);
    let mut profiles: Vec<Profile> = snap.profiles.iter().cloned().map(|values| Profile { values }).collect();
    if profiles.is_empty() {
        profiles.push(Profile::new("Default"));
    }
    crate::store::ensure_unique_guids(&mut profiles);
    store.default_guid = if profiles.iter().any(|p| p.guid() == snap.default) { snap.default.clone() } else { profiles[0].guid() };
    store.profiles = profiles;
}

/// The launch's load (`RP:1452-1535`): with the setting on, the location's settings replace
/// the local ones. Returns the snapshot loaded (iTerm2's `savedRemotePrefs`) and the questions
/// or failures to show after the first window opens.
pub(super) fn load_at_start(g: &mut Globals, store: &mut Store, fetch: &dyn Fn(&str, Option<f64>) -> Fetched) -> (Option<Snapshot>, Vec<Ask>) {
    match start(g, &home(), fetch) {
        Start::Off => (None, Vec::new()),
        Start::Ask(ask) => (None, vec![ask]),
        Start::Loaded(snap) => {
            apply_snapshot(&snap, g, store);
            (Some(snap), Vec::new())
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

/// Writes the snapshot through a temporary file and a rename. A network path is never written.
fn write_snapshot(snap: &Snapshot, file: &Path) -> Result<(), String> {
    if rp::is_unc(&file.to_string_lossy()) {
        return Err(format!("{}: network paths are not supported", file.display()));
    }
    let mut tmp = file.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, snap.to_toml()).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, file).map_err(|e| format!("{}: {e}", file.display()))
}

impl App {
    /// The location as typed and as used (tilde-expanded, a URL as is).
    fn prefs_location(&self) -> (String, String) {
        let text = self.globals.string(rp::FOLDER_KEY);
        let location = rp::expanded(&text, &home());
        (text, location)
    }

    /// The settings file in the folder; None for a URL, a blank location or a network path,
    /// which are never touched as files.
    fn prefs_file(&self) -> Option<PathBuf> {
        let (text, location) = self.prefs_location();
        if rp::is_url(&location) || text.trim().is_empty() || rp::is_unc(&text) || rp::is_unc(&location) {
            return None;
        }
        Some(rp::prefs_file(Path::new(&location), &|p| p.is_file()))
    }

    /// The local settings as the folder would hold them.
    fn prefs_local(&self) -> Snapshot {
        Snapshot::from_local(&self.globals.values, &self.store.default_guid, &self.store.file_profiles())
    }

    /// A setting or profile was saved (`RP:1460-1478`): with "Automatically", a folder, and a
    /// difference from what the folder held (or nothing held), the folder is written half a
    /// second later.
    pub(super) fn prefs_changed(&mut self) {
        self.prefs_note_location();
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
        if write_snapshot(&snap, &file).is_ok() {
            self.prefs_saved = Some(snap);
        }
    }

    /// Writes the folder, or says it could not (`RP:376-386`). iTerm2 names the file twice.
    fn prefs_write(&mut self) {
        let snap = self.prefs_local();
        let file = self.prefs_file();
        if let Some(f) = &file {
            if write_snapshot(&snap, f).is_ok() {
                self.prefs_saved = Some(snap);
                return;
            }
        }
        let shown = file.map_or_else(|| self.prefs_location().1, |f| f.display().to_string());
        self.ask_scripts(Ask { heading: rp::FAILED_COPY.to_string(), text: rp::tried_to_copy(&shown, &shown), buttons: ok(), silence: None, then: Then::Nothing });
    }

    /// `remotePrefsHaveChanged` (`RP:1625-1644`): the folder's file no longer holds what was
    /// loaded or last written. False when nothing was.
    fn prefs_remote_changed(&self) -> bool {
        let (Some(saved), Some(file)) = (&self.prefs_saved, self.prefs_file()) else { return false };
        let fresh = std::fs::read_to_string(file).ok().and_then(|t| Snapshot::parse(&t));
        fresh.as_ref() != Some(saved)
    }

    /// Save Now, the Copy answers and the save at quit (`saveLocalUserDefaultsToRemotePrefs`,
    /// `RP:335-392`). iTerm2's `interactive` only chooses how the Data folder reconciles, which
    /// Tandem does not sync.
    pub(super) fn prefs_save_now(&mut self, _interactive: bool) {
        let (text, location) = self.prefs_location();
        let url = rp::is_url(&location);
        match save_plan(url, !url && self.prefs_remote_changed()) {
            SavePlan::CannotCopyToUrl => {
                let ask = Ask { heading: rp::CANNOT_COPY_URL.to_string(), text: rp::CANNOT_COPY_URL_DETAIL.to_string(), buttons: ok(), silence: None, then: Then::Nothing };
                self.ask_scripts(ask);
            }
            SavePlan::AskOverwrite => self.ask_scripts(overwrite_ask(&text)),
            SavePlan::Write => self.prefs_write(),
        }
    }

    /// "Copy local settings to custom folder now?" (`GP:4276-4284`), after a usable folder was
    /// picked with the checkbox.
    pub(super) fn prefs_offer_copy(&mut self) {
        let buttons = vec![rp::COPY.to_string(), rp::DONT_COPY.to_string()];
        self.ask_scripts(Ask { heading: rp::COPY_NOW.to_string(), text: String::new(), buttons, silence: None, then: Then::PrefsCopyNow });
    }

    /// The location's settings replace the running ones, as an import does.
    pub(super) fn prefs_apply(&mut self, snap: Snapshot) {
        apply_snapshot(&snap, &mut self.globals, &mut self.store);
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
            (Then::PrefsOverwrite, 0) => self.prefs_write(),
            (Then::PrefsQuit, 0) => self.prefs_save_now(false),
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
        match rp::quit_step(on, rp::is_url(&location), differs, mode, self.prefs_saved.is_some()) {
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
            QuitStep::Ask => {
                let buttons = vec![rp::COPY.to_string(), rp::LOSE_CHANGES.to_string()];
                let ask = Ask { heading: crate::script_new::WARNING.to_string(), text: rp::settings_changed(&text), buttons, silence: Some(rp::HAVE_SELECTION_KEY), then: Then::PrefsQuit };
                self.ask_scripts(ask);
            }
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
        g.values.insert("tmuxTitlePrefix".into(), Value::String("local".into()));
        g.values.insert("NoSyncX".into(), Value::Boolean(true));
        let mut store = Store::fresh(&dir.join("profiles.toml"));
        store.profiles[0].set(crate::profile::NAME, Value::String("Local".into()));
        (g, store)
    }

    fn remote() -> Snapshot {
        let profile = map(&[("Guid", Value::String("g1".into())), ("Name", Value::String("Remote".into()))]);
        Snapshot { settings: map(&[("FocusFollowsMouse", Value::Boolean(true))]), default: "g1".into(), profiles: vec![profile] }
    }

    fn names(store: &Store) -> Vec<String> {
        store.profiles.iter().map(|p| p.name()).collect()
    }

    fn buttons(ask: &Ask) -> Vec<&str> {
        ask.buttons.iter().map(String::as_str).collect()
    }

    fn never(_: &str, _: Option<f64>) -> Fetched {
        panic!("nothing is fetched")
    }

    fn assert_applied(g: &Globals, store: &Store, location: &str) {
        assert_eq!(g.values.get("FocusFollowsMouse"), Some(&Value::Boolean(true)));
        assert!(!g.values.contains_key("tmuxTitlePrefix"), "a local syncable key goes");
        assert_eq!(g.values.get("NoSyncX"), Some(&Value::Boolean(true)));
        assert!(g.bool(rp::LOAD_KEY));
        assert_eq!(g.string(rp::FOLDER_KEY), location);
        assert_eq!(names(store), ["Remote"]);
        assert_eq!(store.default_guid, "g1");
    }

    fn assert_untouched(g: &Globals, store: &Store) {
        assert_eq!(g.values.get("tmuxTitlePrefix"), Some(&Value::String("local".into())));
        assert!(!g.values.contains_key("FocusFollowsMouse"));
        assert_eq!(names(store), ["Local"]);
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
        assert_eq!((asks[0].heading.as_str(), asks[0].text.as_str()), (rp::ERROR_LOADING, rp::LOCATION_NOT_SET));
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
        assert_eq!(asks[0].heading, "Failed to load settings from custom directory. Falling back to local copy.");
        assert_eq!(asks[0].text, format!("Missing or malformed file at \u{201c}{e}\u{201d}"));
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
        assert_eq!((asks[0].heading.as_str(), asks[0].text.as_str()), (rp::FAILED_URL, "HTTP request failed: timed out"));
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
        assert_eq!(asks[0].text, rp::URL_PROMPT);
        assert_eq!(asks[0].then, Then::PrefsUrlPrompt(url.into()));
        assert_eq!(buttons(&asks[0]), ["Keep Local Changes", "Disable Loading from URL", "Discard Local Changes"]);
        assert!(asks[0].silence.is_none());
        assert_untouched(&g, &store);

        // A network folder is never read.
        let (mut g, mut store) = local(&dir, r"\\host\share");
        let (snap, asks) = load_at_start(&mut g, &mut store, &never);
        assert!(snap.is_none());
        assert_eq!(asks[0].heading, rp::FAILED_FOLDER);
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
        let failed = || Ask { heading: rp::FAILED_COPY.to_string(), text: rp::tried_to_copy("f", "f"), buttons: ok(), silence: None, then: Then::Nothing };
        let mut queued = std::collections::VecDeque::new();
        assert!(!exit_waits(None, &queued));
        assert!(exit_waits(Some(&failed()), &queued), "the failure to copy is seen before the exit");
        queued.push_back(failed());
        assert!(exit_waits(None, &queued));
    }

    #[test]
    fn save_now_follows_iterm2() {
        assert_eq!(save_plan(true, false), SavePlan::CannotCopyToUrl);
        assert_eq!(save_plan(true, true), SavePlan::CannotCopyToUrl);
        assert_eq!(save_plan(false, true), SavePlan::AskOverwrite);
        assert_eq!(save_plan(false, false), SavePlan::Write);
        let ask = overwrite_ask(r"D:\s");
        assert_eq!(ask.text, "Settings at D:\\s changed since Tandem started. Overwrite it?");
        assert_eq!(buttons(&ask), ["Overwrite", "Discard Local Changes"]);
        assert_eq!(ask.then, Then::PrefsOverwrite);
        assert!(ask.silence.is_none());
    }

    #[test]
    fn written_snapshots_read_back() {
        let dir = temp("write");
        let file = dir.join(rp::PREFS_FILE);
        write_snapshot(&remote(), &file).unwrap();
        assert_eq!(Snapshot::parse(&std::fs::read_to_string(&file).unwrap()), Some(remote()));
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1, "the temporary file is renamed");
        assert!(write_snapshot(&remote(), Path::new(r"\\host\share\tandem-settings.toml")).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
