//! Opening files and folders handed over by another `tandem.exe` (m13v1 Task 2), after iTerm2's
//! `application:openFile:` (`iTermApplicationDelegate.m:785-913`). Every path is another program's:
//! it is checked on a worker (`open_file::inspect`: the one network rule, the link check, the
//! stat, Mark of the Web), never on the event loop, and only then does a kind's arm run. Many
//! files arrive as many processes: they are batched (`open_file::Batch`), never dropped silently.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};
use std::time::Instant;

use winit::event_loop::ActiveEventLoop;

use super::scripts_alerts::{ask, Then};
use super::App;
use crate::globals::Globals;
use crate::open_file::{self, Facts, Kind, Launch, Style};
use crate::warnings::Kind as Warn;

/// Banner key for what opening a file said.
const BANNER: &str = "open-file";
/// Workers (one per batch) that may be inspecting at once; a share that never answers cannot
/// pile up threads.
const MAX_WORKERS: usize = 4;

/// What a worker found.
pub(super) enum Done {
    /// The facts of each path of a batch, in order.
    Inspected(Vec<Result<Facts, String>>),
    /// The file of a "Add it anyway", checked again before it is read.
    Recheck(Result<Facts, String>),
}

/// The inspecting workers' channel (app-wide).
pub(super) struct OpenWork {
    tx: mpsc::Sender<Done>,
    rx: mpsc::Receiver<Done>,
    notify: crate::pty::Notify,
    inflight: Arc<AtomicUsize>,
}

impl OpenWork {
    pub(super) fn new(notify: crate::pty::Notify) -> OpenWork {
        let (tx, rx) = mpsc::channel();
        OpenWork { tx, rx, notify, inflight: Arc::new(AtomicUsize::new(0)) }
    }
}

/// What importing a `.itermcolors` file came to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum ColorImport {
    Failed,
    Duplicate(String),
    Added(String),
}

/// `iTermColorPresets.m:31-68` through 13t: a file that is no preset is `Failed`; a custom preset
/// equal to it is `Duplicate(its name)` unless `add_duplicate`; otherwise it is added under the
/// file's name (`Added(the name used)`).
pub(super) fn import_color_preset(g: &mut Globals, path: &Path, bytes: &[u8], add_duplicate: bool) -> ColorImport {
    let Some(preset) = crate::color_presets::read_itermcolors(bytes) else { return ColorImport::Failed };
    if !add_duplicate {
        if let Some(name) = crate::color_presets::duplicate_of(g, &preset) {
            return ColorImport::Duplicate(name);
        }
    }
    match crate::color_presets::add_custom(g, &crate::color_presets::name_from_file(path), preset) {
        Ok(name) => ColorImport::Added(name),
        Err(_) => ColorImport::Failed,
    }
}

/// A color file's bytes: at most one byte past the preset limit is read.
fn read_color_file(path: &Path) -> Option<Vec<u8>> {
    use std::io::Read;
    let mut bytes = Vec::new();
    crate::localfs::open(&crate::localfs::LocalPath::judge(path)?).ok()?.take(crate::color_presets::MAX_ITERMCOLORS as u64 + 1).read_to_end(&mut bytes).ok()?;
    Some(bytes)
}

impl App {
    /// Files that were not opened, from any layer: ONE banner whose count adds up.
    pub(super) fn open_files_ignored(&mut self, n: usize) {
        let total = self.open_ignored.add(Instant::now(), n);
        self.banners.set("open-ignored", open_file::ignored_banner(total));
        self.redraw();
    }

    fn open_file_banner(&mut self, text: String) {
        self.banners.set(BANNER, text);
        self.redraw();
    }

    fn open_keys(&self) -> open_file::OpenKeys {
        let g = &self.globals;
        open_file::OpenKeys {
            overrides_send_text: g.bool("OpenFileOverridesSendText"),
            new_instance_style: g.int("NewInstanceOpenStyle"),
            in_new_windows: g.bool("OpenFileInNewWindows"),
            in_splits: g.bool("OpenFileInSplitPanes"),
            vertical: g.bool("OpenFileInVerticalSplitPane"),
        }
    }

    /// One turn of the open machinery: a batch whose window ended goes to a worker, the workers'
    /// answers act, and the scripts the person said OK to run. Returns when to come back. No file
    /// system call here: they are all on the worker.
    pub(super) fn open_tick(&mut self, event_loop: &ActiveEventLoop) -> Option<Instant> {
        if self.gpu.is_some() {
            if let Some((paths, ignored)) = self.open_batch.take(Instant::now()) {
                if ignored > 0 {
                    self.open_files_ignored(ignored);
                }
                if !paths.is_empty() {
                    self.open_paths(paths, event_loop);
                }
            }
        }
        while let Ok(done) = self.open_work.rx.try_recv() {
            match done {
                Done::Inspected(list) => {
                    for r in list {
                        self.opened(r, event_loop);
                    }
                }
                Done::Recheck(Ok(f)) => self.add_duplicate_color(&f.path),
                Done::Recheck(Err(e)) => self.open_file_banner(e),
            }
        }
        for launch in std::mem::take(&mut self.open_runs) {
            self.run_opened(launch, event_loop);
        }
        if self.gpu.is_some() { self.open_batch.due() } else { None }
    }

    /// Hands the paths of one batch to a worker; each is checked there, in order.
    pub(super) fn open_paths(&mut self, paths: Vec<PathBuf>, _event_loop: &ActiveEventLoop) {
        self.spawn_inspect(paths, false);
    }

    /// An "Add it anyway": the file is checked again (it may have changed since) before it is read.
    pub(super) fn open_recheck(&mut self, path: PathBuf) {
        self.spawn_inspect(vec![path], true);
    }

    fn spawn_inspect(&mut self, paths: Vec<PathBuf>, recheck: bool) {
        if self.open_work.inflight.load(Ordering::SeqCst) >= MAX_WORKERS {
            return self.open_files_ignored(paths.len());
        }
        self.open_work.inflight.fetch_add(1, Ordering::SeqCst);
        let (tx, notify, inflight) = (self.open_work.tx.clone(), self.open_work.notify.clone(), self.open_work.inflight.clone());
        let started = std::thread::Builder::new().name("open-file".into()).spawn({
            let inflight = inflight.clone();
            move || {
                let list: Vec<Result<Facts, String>> = paths.iter().map(|p| open_file::inspect(p, &open_file::RealProbe)).collect();
                inflight.fetch_sub(1, Ordering::SeqCst);
                let done = if recheck { Done::Recheck(list.into_iter().next().unwrap_or_else(|| Err(open_file::NETWORK_BANNER.to_string()))) } else { Done::Inspected(list) };
                if tx.send(done).is_ok() {
                    notify();
                }
            }
        });
        if started.is_err() {
            inflight.fetch_sub(1, Ordering::SeqCst);
            self.open_file_banner("Tandem could not start a worker to open this file.".to_string());
        }
    }

    /// One inspected path: its kind's arm (`AD:785-913`).
    fn opened(&mut self, r: Result<Facts, String>, event_loop: &ActiveEventLoop) {
        let f = match r {
            Ok(f) => f,
            Err(e) => return self.open_file_banner(e),
        };
        if let Some(w) = self.last_window {
            self.enter(w);
        }
        let shown = crate::inert_files::shown_name(&f.path.to_string_lossy());
        let data = !matches!(f.kind, Kind::Folder | Kind::Script);
        if data && (!f.regular || f.len > open_file::MAX_DATA_LEN) {
            return self.open_file_banner(format!("Tandem did not open \u{201c}{shown}\u{201d}: it is not a file of a size it reads."));
        }
        match f.kind {
            // Not user-initiated: no AutoLaunch offer. A signed archive is refused by the importer.
            Kind::ScriptArchive => self.import_path(f.path, false),
            Kind::Recording => {
                self.import_recording_at(&f.path);
                self.bring_forward();
            }
            Kind::Arrangement => {
                self.load_arrangement_path(&f.path);
                self.bring_forward();
            }
            Kind::ColorPresets => self.open_color_file(&f.path),
            Kind::ProfileTab => self.open_profile_tab(&f.path),
            Kind::Triggers => self.open_file_banner(format!("Tandem cannot open \u{201c}{shown}\u{201d} yet.")),
            Kind::Folder => self.open_folder_or_script(&f, event_loop),
            Kind::Script => self.open_folder_or_script(&f, event_loop),
        }
    }

    /// A folder opens a session there with no question; a script asks "OK to run" (`AD:826-860`).
    fn open_folder_or_script(&mut self, f: &Facts, event_loop: &ActiveEventLoop) {
        let profile = self.store.default_profile().clone();
        let command = crate::profile::command_line(&profile, &self.shell, None);
        let shown = crate::inert_files::shown_name(&f.path.to_string_lossy());
        let launch = match open_file::plan(&f.path, f.is_dir, &profile.guid(), &profile.string("Initial Text"), &command, self.open_keys()) {
            Ok(l) => l,
            Err(_) => return self.open_file_banner(open_file::refused_banner(&shown)),
        };
        if f.is_dir {
            return self.run_opened(launch, event_loop);
        }
        let Ok(quoted) = open_file::quoted_path(&f.path, open_file::shell_of(&command)) else { return self.open_file_banner(open_file::refused_banner(&shown)) };
        let (id, kind, text) = open_file::run_question(f, &open_file::confirm_run_title(&quoted));
        self.ask_scripts(ask("", &text, &[crate::api_auth::OK, crate::api_auth::CANCEL], id, kind, Then::RunOpenedFile(launch)));
    }

    /// The session for an opened folder or script: a session copy of the default profile (`AD:834`)
    /// whose "Custom Directory" is the folder, or whose "Initial Text" is the run text; 13p's
    /// `send_initial_text` types it once the pane is installed. Opens as `launch.style` says, in a
    /// new window when asked or when there is none, and comes to the front.
    pub(super) fn run_opened(&mut self, launch: Launch, event_loop: &ActiveEventLoop) {
        if self.gpu.is_none() {
            return;
        }
        if let Some(w) = self.last_window {
            self.enter(w);
        }
        let guid = if self.store.get(&launch.guid).is_some() { launch.guid.clone() } else { self.store.default_profile().guid() };
        let mut values: Vec<(String, Option<toml::Value>)> = Vec::new();
        if let Some(dir) = &launch.dir {
            values.push(("Custom Directory".to_string(), Some(toml::Value::String("Yes".into()))));
            values.push(("Working Directory".to_string(), Some(toml::Value::String(dir.to_string_lossy().into_owned()))));
        }
        if let Some(text) = &launch.text {
            values.push(("Initial Text".to_string(), Some(toml::Value::String(text.clone()))));
        }
        let saved = self.store.shared_guid(&guid);
        let shared = self.store.profiles.iter().find(|p| p.guid() == saved).unwrap_or_else(|| self.store.default_profile()).clone();
        let fresh = uuid::Uuid::new_v4().to_string().to_uppercase();
        let divorced = crate::session_profile::divorce(&shared, fresh.clone());
        let copy = self.add_session_copy(divorced, &shared, &values, fresh);
        let before = (self.tabs.len(), self.pending_windows.len(), self.panes.len());
        let in_window = !(launch.new_window || self.tabs.is_empty());
        let started = match (in_window, launch.style) {
            (true, Style::Tab) => {
                self.add_tab(Some(&copy));
                true
            }
            (true, Style::VerticalSplit) => {
                self.split(crate::layout::Axis::Row, Some(&copy));
                true
            }
            (true, Style::HorizontalSplit) => {
                self.split(crate::layout::Axis::Column, Some(&copy));
                true
            }
            _ => match self.open_window(event_loop, Some(&copy)) {
                Ok(_) => true,
                Err(e) => {
                    self.banners.set(BANNER, format!("Could not open a window: {e}"));
                    false
                }
            },
        };
        if !started || (self.tabs.len(), self.pending_windows.len(), self.panes.len()) == before {
            self.forget_session_profile(&copy);
        }
        self.bring_forward();
        self.redraw();
    }

    /// An opened `.itermcolors` (`AD:802-810`, `iTermColorPresets.m:31-68`).
    pub(super) fn open_color_file(&mut self, path: &Path) {
        let Some(bytes) = read_color_file(path) else { return self.color_import_done(ColorImport::Failed, path) };
        let result = import_color_preset(&mut self.globals, path, &bytes, false);
        self.color_import_done(result, path);
    }

    fn add_duplicate_color(&mut self, path: &Path) {
        let Some(bytes) = read_color_file(path) else { return self.color_import_done(ColorImport::Failed, path) };
        let result = import_color_preset(&mut self.globals, path, &bytes, true);
        self.color_import_done(result, path);
    }

    fn color_import_done(&mut self, result: ColorImport, path: &Path) {
        let ok = [crate::api_auth::OK];
        match result {
            ColorImport::Failed => self.ask_scripts(ask(crate::settings_presets::IMPORT_FAILED_HEADING, crate::settings_presets::IMPORT_FAILED_TEXT, &ok, None, Warn::Persistent, Then::Nothing)),
            ColorImport::Duplicate(name) => {
                let text = crate::settings_presets::duplicate_text(&crate::inert_files::shown_name(&name));
                let labels = [crate::api_auth::CANCEL, crate::settings_presets::ADD_ANYWAY];
                self.ask_scripts(ask(crate::settings_presets::DUPLICATE_HEADING, &text, &labels, None, Warn::Persistent, Then::AddDuplicateColorPreset(path.to_path_buf())));
            }
            ColorImport::Added(_) => {
                self.save_globals();
                self.ask_scripts(ask(open_file::COLORS_IMPORTED_HEADING, open_file::COLORS_IMPORTED_TEXT, &ok, None, Warn::Persistent, Then::Nothing));
            }
        }
    }

    /// Adds Tandem to Explorer's "Open with" list, once per run, only when its entry is absent or
    /// names another program (P10): per user, never a default association.
    pub(super) fn register_open_with(&mut self) {
        if std::mem::replace(&mut self.open_with_done, true) {
            return;
        }
        #[cfg(windows)]
        {
            use crate::reg::Registry;
            let Ok(exe) = std::env::current_exe() else { return };
            let entries = open_file::open_with_entries(&exe);
            let mut reg = crate::reg::HkcuRegistry;
            if reg.get(&entries[1].0, &entries[1].1).as_deref() == Some(entries[1].2.as_str()) {
                return;
            }
            if let Err(e) = crate::reg::apply(&mut reg, &entries) {
                crate::host::log(&format!("open with: {e}"));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color_presets::{self, Preset, PresetColor};
    use crate::globals::Globals;
    use std::path::Path;

    fn two_colors() -> Vec<u8> {
        let mut p = Preset::new();
        p.insert("Ansi 0 Color".into(), PresetColor { r: 0.1, g: 0.2, b: 0.3, a: None, space: None });
        p.insert("Foreground Color".into(), PresetColor { r: 0.9, g: 0.8, b: 0.7, a: None, space: None });
        color_presets::write_itermcolors(&p)
    }

    /// R13vw-5: `.itermcolors` goes through 13t's reader, duplicate check and `add_custom`.
    #[test]
    fn color_files_import_through_13t() {
        let dir = std::env::temp_dir().join(format!("tandem-open-colors-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut g = Globals::empty(&dir.join("globals.toml"));
        let bytes = two_colors();
        let path = Path::new(r"C:\x\My Theme.itermcolors");
        assert_eq!(import_color_preset(&mut g, path, &bytes, false), ColorImport::Added("My Theme".into()));
        assert!(color_presets::custom(&g).iter().any(|(n, _)| n == "My Theme"));
        assert_eq!(import_color_preset(&mut g, path, &bytes, false), ColorImport::Duplicate("My Theme".into()));
        assert_eq!(color_presets::custom(&g).len(), 1, "a duplicate adds nothing");
        assert_eq!(import_color_preset(&mut g, path, &bytes, true), ColorImport::Added("My Theme (2)".into()));
        assert_eq!(import_color_preset(&mut g, path, b"not a plist", false), ColorImport::Failed);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// B6: a Mark-of-the-Web or oversized script asks every time; the question closes as Cancel.
    #[test]
    fn open_requests_are_bounded_and_never_probe_on_the_event_loop() {
        let src = include_str!("open_file_ui.rs").split("#[cfg(test)]").next().unwrap();
        let tick = &src[src.find("fn open_tick").unwrap()..];
        assert!(!tick.contains("std::fs::"), "the tick never touches the file system");
        assert!(src.contains("open_file::inspect"), "the checks run through inspect");
        assert!(src.contains("std::thread::Builder"), "on a worker");
    }

    /// 13p's `send_initial_text` is the only sender: an opened script's text rides the session copy.
    #[test]
    fn opened_scripts_type_through_13p_only() {
        let src = include_str!("open_file_ui.rs").split("#[cfg(test)]").next().unwrap();
        assert!(!src.contains("send_initial_text("));
        assert!(!src.contains("start_command_override") && !src.contains("start_env_override"), "an override launch would skip the initial text");
        assert!(src.contains("add_session_copy"));
    }
}
