//! Scripts > Manage > Manage Dependencies in the app (iTerm2's
//! `iTermDependencyEditorWindowController`): the editor's state, its `pip show` queries and
//! Python version list off the UI thread, the pip commands asked about and run in a window,
//! what follows each, the Python version change and the basic-to-full upgrade.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender};

use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::WindowId;

use super::script_runtime_ui::{uv_dir, After};
use super::scripts_alerts::{ask, Then};
use crate::warnings::Kind;
use super::App;
use crate::layout::PaneId;
use crate::prompt_ui::{PromptResult, PromptWindow};
use crate::script_deps::{self as sd, VersionChange};
use crate::script_deps_ui::{DepsData, DepsEvent, DepsWindow};
use crate::script_provision::{Job, Outcome, RealRunner, Runner, Step, Target};
use crate::script_runtime::{self as rt, Backend};

/// What finishes when a pip command's window ends (`DE:413-458`, `DE:284-339`): the
/// container and the dependency it was about.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Pending {
    Install { container: PathBuf, dep: String },
    Uninstall { container: PathBuf, dep: String },
    Upgrade { container: PathBuf, dep: String },
}

/// A confirmed pip command waiting for the next loop pass, which has the event loop to open
/// its window with. The variables hold no secret.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct RunPip {
    program: PathBuf,
    args: Vec<String>,
    env: Vec<(String, String)>,
    pending: Pending,
}

/// Why a `pip show` ran.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Purpose {
    /// The table's Installed Version.
    Show,
    /// After an install: recorded in `setup.cfg`, or "Installation Failed".
    Installed,
    /// After an uninstall: still installed is "Removal Failed", else it leaves `setup.cfg`.
    Uninstalled,
    /// After Check for Update: re-pinned to the version installed.
    Upgraded,
}

/// What a worker thread reports.
enum Msg {
    /// The Python versions the backend can build, and the backend (which a version change
    /// builds with).
    Minors { container: PathBuf, backend: Option<Backend>, minors: Vec<String> },
    /// One `pip show`: the version its output named, or its error.
    Version { container: PathBuf, dep: String, purpose: Purpose, result: Result<Option<String>, String> },
}

/// The editor's state, app-wide: it outlives the window, so a pip command or a rebuild that
/// ends after the window closed still finishes.
pub(super) struct Deps {
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    /// The Script popup's items (`sd::script_choices`) and the one selected.
    choices: Vec<(String, PathBuf, bool)>,
    selected: usize,
    /// The selected environment's table, version, and the versions offered.
    packages: Vec<(String, String)>,
    version: String,
    versions: Vec<String>,
    /// What listed `versions`; a version change builds with it.
    backend: Option<Backend>,
    /// The container whose Python version is being changed (`_rebuildInProgress`).
    rebuilding: Option<PathBuf>,
    run: Option<RunPip>,
    /// "Add Dependency"'s question, and the container it is for.
    prompt: Option<(PromptWindow, PathBuf)>,
    /// A script to select once the scan lists it (an upgraded script's new folder).
    want: Option<PathBuf>,
}

impl Deps {
    pub(super) fn new() -> Deps {
        let (tx, rx) = std::sync::mpsc::channel();
        Deps { tx, rx, choices: Vec::new(), selected: 0, packages: Vec::new(), version: String::new(), versions: Vec::new(), backend: None, rebuilding: None, run: None, prompt: None, want: None }
    }

    /// The selected script's path and whether it is a full environment.
    fn current(&self) -> Option<(PathBuf, bool)> {
        self.choices.get(self.selected).map(|(_, p, full)| (p.clone(), *full))
    }
}

/// A runner for `pip show` and the version list: short, output not wanted.
fn probe_runner() -> RealRunner {
    RealRunner { lines: std::sync::mpsc::channel().0, timeout: crate::script_provision::PROBE_TIMEOUT }
}

/// What runs pip for a container, from its marker (no probe); None without one.
fn pip_backend(container: &Path) -> Option<Backend> {
    let marker = rt::Marker::parse(&std::fs::read_to_string(container.join(rt::MARKER)).ok()?)?;
    Some(sd::marker_backend(&marker, &rt::uv_exe(&uv_dir()), &container.join(rt::VENV_DIR)))
}

/// A container's `setup.cfg` rewritten with `f` applied to its dependencies, its version and
/// the rest kept (`writeSetupCfgToFile:`); the version is the environment's marker's when
/// `setup.cfg` pins none.
fn edit_setup_cfg(container: &Path, f: impl FnOnce(Vec<String>) -> Vec<String>) -> Result<(), String> {
    let path = container.join(rt::SETUP_CFG);
    let text = std::fs::read_to_string(&path).map_err(|e| format!("Could not read {}: {e}", path.display()))?;
    let cfg = rt::SetupCfg::parse(&text).ok_or_else(|| format!("Could not read {}", path.display()))?;
    let marker = std::fs::read_to_string(container.join(rt::MARKER)).ok().and_then(|t| rt::Marker::parse(&t));
    let python = cfg.python_version.clone().or(marker.map(|m| m.python)).unwrap_or_else(|| rt::DEFAULT_PYTHON.to_string());
    let out = sd::setup_cfg_with(&text, &f(cfg.dependencies), &python).ok_or_else(|| format!("Could not read {}", path.display()))?;
    std::fs::write(&path, out).map_err(|e| format!("Could not write {}: {e}", path.display()))
}

/// The variables the pip command's window gets: Tandem's own with the command's over them
/// (`open_pending_deps_run`; the session's own variables name no index).
fn window_env(env: &[(String, String)]) -> Vec<(String, String)> {
    let inherited = std::env::vars_os().filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?))).collect();
    let extra: Vec<String> = env.iter().map(|(k, v)| format!("{k}={v}")).collect();
    crate::session_env::merge(inherited, &extra)
}

impl App {
    fn deps_alert(&mut self, heading: &str, text: String) {
        self.ask_scripts(ask(heading, &text, &[crate::api_auth::OK], None, Kind::Persistent, Then::Nothing));
    }

    /// Scripts > Manage > Manage Dependencies (`open`, `DE:79-90`): the window lists the
    /// scripts again, keeping the one selected (the first when it is gone).
    pub(super) fn open_dependency_editor(&mut self, event_loop: &ActiveEventLoop) {
        if self.script_deps.is_none() {
            let Some(g) = self.gpu.as_ref() else { return };
            match DepsWindow::open(event_loop, &g.instance, &g.adapter, &g.device, &g.queue) {
                Ok(w) => self.script_deps = Some(w),
                Err(e) => return self.banners.set("window", format!("Could not open the window: {e}")),
            }
        } else if let Some(w) = &self.script_deps {
            w.focus();
        }
        let keep = self.script_deps_state.current().map(|(p, _)| p);
        self.deps_reload(keep);
    }

    /// `populateScripts` then `didSelectScriptAtIndex:`: the scripts from the last scan, the
    /// one at `keep` selected when listed, else the first.
    fn deps_reload(&mut self, keep: Option<PathBuf>) {
        let choices = sd::script_choices(&self.scripts_tree);
        let at = keep.and_then(|k| choices.iter().position(|(_, p, _)| *p == k)).unwrap_or(0);
        self.script_deps_state.choices = choices;
        self.deps_select(at);
    }

    /// `didSelectScriptAtIndex:` (`DE:181-205`): a full environment's dependencies sorted,
    /// its version at once and the versions offered when they arrive, each package's version
    /// from `pip show` (both off the UI thread).
    fn deps_select(&mut self, index: usize) {
        let s = &mut self.script_deps_state;
        s.selected = index;
        s.packages.clear();
        s.versions.clear();
        s.version.clear();
        s.backend = None;
        let Some((container, true)) = s.current() else { return self.deps_push() };
        let cfg = std::fs::read_to_string(container.join(rt::SETUP_CFG)).ok().and_then(|t| rt::SetupCfg::parse(&t)).unwrap_or_default();
        let marker = std::fs::read_to_string(container.join(rt::MARKER)).ok().and_then(|t| rt::Marker::parse(&t));
        let deps = sd::table_dependencies(&cfg);
        s.packages = deps.iter().map(|d| (d.clone(), String::new())).collect();
        s.version = sd::shown_version(marker.as_ref(), &cfg);
        s.versions = sd::version_choices(&[], &s.version);
        let (tx, notify, c) = (s.tx.clone(), self.notify.clone(), container.clone());
        std::thread::spawn(move || {
            // Finding an installed Python runs it, so this is off the UI thread.
            let backend = super::script_runtime_ui::find_backend();
            let minors = backend.clone().map_or_else(Vec::new, |b| match crate::script_provision::run_job(&Job::Minors { backend: b }, &uv_dir(), &mut probe_runner()) {
                Ok(Outcome::Minors(v)) => v,
                _ => Vec::new(),
            });
            let _ = tx.send(Msg::Minors { container: c, backend, minors });
            notify();
        });
        self.deps_pip_show(&container, deps, Purpose::Show);
        self.deps_push();
    }

    /// `fetchVersionOfPackage:` for each dependency, by its bare name (`DE:94-117`), on a
    /// thread of its own. A dependency naming no package is never passed to pip: its row
    /// reads "[Error]".
    fn deps_pip_show(&mut self, container: &Path, deps: Vec<String>, purpose: Purpose) {
        let (tx, notify, container) = (self.script_deps_state.tx.clone(), self.notify.clone(), container.to_path_buf());
        let Some(backend) = pip_backend(&container) else {
            for dep in deps {
                let _ = tx.send(Msg::Version { container: container.clone(), dep, purpose, result: Err("No Python environment.".to_string()) });
            }
            return notify();
        };
        std::thread::spawn(move || {
            let venv = container.join(rt::VENV_DIR);
            for dep in deps {
                let result = match sd::package_args("show", &dep) {
                    Some(show) => {
                        let (program, args, env) = sd::pip_command(&backend, &venv, &show);
                        probe_runner().run(&Step { program, args, env }).map(|out| sd::version_in_pip_show(&out))
                    }
                    None => Err(rt::refused_dependency_line(&dep)),
                };
                let _ = tx.send(Msg::Version { container: container.clone(), dep, purpose, result });
                notify();
            }
        });
    }

    /// The window shows the state; its controls are off while a rebuild or a pip command's
    /// window runs.
    pub(super) fn deps_push(&mut self) {
        let busy = self.script_deps_state.rebuilding.is_some() || self.script_deps_pending.is_some();
        let s = &self.script_deps_state;
        let Some(w) = self.script_deps.as_mut() else { return };
        w.set_data(DepsData {
            scripts: s.choices.iter().map(|c| c.0.clone()).collect(),
            selected: s.selected,
            full: s.current().is_some_and(|c| c.1),
            versions: s.versions.clone(),
            version: s.version.clone(),
            packages: s.packages.clone(),
            busy,
        });
    }

    /// The workers' reports.
    pub(super) fn deps_tick(&mut self) {
        let msgs: Vec<Msg> = std::iter::from_fn(|| self.script_deps_state.rx.try_recv().ok()).collect();
        if msgs.is_empty() {
            return;
        }
        for m in msgs {
            let selected = self.script_deps_state.current().map(|c| c.0);
            match m {
                Msg::Minors { container, backend, minors } => {
                    if selected.as_ref() == Some(&container) {
                        let s = &mut self.script_deps_state;
                        s.versions = sd::version_choices(&minors, &s.version);
                        s.backend = backend;
                    }
                }
                Msg::Version { container, dep, purpose, result } => self.deps_version_known(&container, &dep, purpose, result, selected.as_ref() == Some(&container)),
            }
        }
        self.deps_push();
    }

    /// One `pip show` came back: the table's row, or what follows a command.
    fn deps_version_known(&mut self, container: &Path, dep: &str, purpose: Purpose, result: Result<Option<String>, String>, shown: bool) {
        if shown {
            if let Some(row) = self.script_deps_state.packages.iter_mut().find(|(d, _)| d == dep) {
                row.1 = match &result {
                    Ok(v) => v.clone().unwrap_or_default(),
                    Err(_) => sd::ERROR_VERSION.to_string(),
                };
            }
        }
        // Installed means `pip show` succeeded and named a version (uv and pip differ in the
        // exit status for a missing package; both print no "Version: " line).
        let installed = result.ok().flatten();
        let edited = match (purpose, installed) {
            (Purpose::Show, _) => return,
            (Purpose::Installed, None) => return self.deps_alert(sd::INSTALLATION_FAILED, sd::INSTALL_CHECK_PIP.to_string()),
            (Purpose::Installed, Some(_)) => edit_setup_cfg(container, |mut deps| {
                deps.push(dep.to_string());
                deps
            }),
            (Purpose::Uninstalled, Some(_)) => return self.deps_alert(sd::REMOVAL_FAILED, sd::uninstall_failed(dep)),
            (Purpose::Uninstalled, None) => edit_setup_cfg(container, |deps| deps.into_iter().filter(|d| d != dep).collect()),
            (Purpose::Upgraded, Some(v)) => edit_setup_cfg(container, |deps| rt::set_dependency(&deps, &sd::after_upgrade(dep, &v))),
            (Purpose::Upgraded, None) => return,
        };
        if let Err(e) = edited {
            self.banners.set("scripts", e);
        }
        if shown {
            let at = self.script_deps_state.selected;
            self.deps_select(at);
        }
    }

    /// Events for the editor and its Add question. False for other windows.
    pub(super) fn script_deps_event(&mut self, window: WindowId, event: &WindowEvent, event_loop: &ActiveEventLoop) -> bool {
        if let Some((p, _)) = self.script_deps_state.prompt.as_mut().filter(|(p, _)| p.id() == window) {
            let answer = match event {
                WindowEvent::CloseRequested => PromptResult::Cancel,
                WindowEvent::RedrawRequested => p.redraw(),
                other => {
                    p.event(other);
                    PromptResult::Open
                }
            };
            self.deps_prompt_answer(answer);
            return true;
        }
        let Some(w) = self.script_deps.as_mut().filter(|w| w.id() == window) else { return false };
        let answer = match event {
            WindowEvent::CloseRequested => DepsEvent::Close,
            WindowEvent::RedrawRequested => w.redraw(),
            other => {
                w.event(other);
                DepsEvent::None
            }
        };
        self.deps_answer(answer, event_loop);
        true
    }

    fn deps_answer(&mut self, answer: DepsEvent, event_loop: &ActiveEventLoop) {
        let current = self.script_deps_state.current();
        let remove = matches!(answer, DepsEvent::Remove(_));
        // Edits wait while a rebuild or a pip command's window runs (`DE:477`, `DE:606`).
        let busy = self.script_deps_state.rebuilding.is_some() || self.script_deps_pending.is_some();
        match answer {
            DepsEvent::None => {}
            DepsEvent::Close => {
                self.script_deps = None;
                self.script_deps_state.prompt = None;
            }
            _ if busy => {}
            DepsEvent::Script(i) => self.deps_select(i),
            DepsEvent::Add => {
                let Some((container, true)) = current else { return };
                if self.script_deps_state.prompt.is_some() {
                    return;
                }
                let Some(g) = self.gpu.as_ref() else { return };
                match PromptWindow::open(event_loop, &g.instance, &g.adapter, &g.device, &g.queue, sd::ADD_HEADING, sd::ADD_TEXT, "") {
                    Ok(p) => self.script_deps_state.prompt = Some((p, container)),
                    Err(e) => self.banners.set("window", format!("Could not open the window: {e}")),
                }
            }
            DepsEvent::Remove(row) | DepsEvent::CheckForUpdate(row) => {
                let Some((container, true)) = current else { return };
                let Some(dep) = self.script_deps_state.packages.get(row).map(|r| r.0.clone()) else { return };
                if remove {
                    // By its package name only (iTerm2 passes the whole dependency,
                    // `DE:623-638`); a dependency naming none is refused, never passed to pip.
                    match sd::package_args("uninstall", &dep) {
                        Some(args) => self.deps_run(&container, args, Pending::Uninstall { container: container.clone(), dep }),
                        None => self.script_log_app(rt::refused_dependency_line(&dep)),
                    }
                } else if self.deps_plain(&dep) {
                    self.deps_run(&container, vec!["install".to_string(), dep.clone(), "--upgrade".to_string()], Pending::Upgrade { container: container.clone(), dep });
                }
            }
            DepsEvent::Version(v) => {
                let Some((container, true)) = current else { return };
                let (heading, text) = match sd::version_change(&self.script_deps_state.version, &v) {
                    VersionChange::Same => return,
                    VersionChange::Downgrade => (sd::DOWNGRADE_HEADING, sd::DOWNGRADE_TEXT),
                    VersionChange::Upgrade => (sd::UPGRADE_HEADING, sd::UPGRADE_TEXT),
                };
                // Persistent in iTerm2: never silenced.
                let buttons = [crate::api_auth::OK, crate::api_auth::CANCEL];
                self.ask_scripts(ask(heading, text, &buttons, None, Kind::Persistent, Then::VersionChange(container, v)));
            }
            DepsEvent::Upgrade => {
                if let Some((script, false)) = current {
                    self.deps_upgrade(&script);
                }
            }
        }
    }

    /// Whether a dependency may go to pip install: a plain PyPI name, so the confirmation's
    /// "Packages come from pypi.org." holds; else it is logged and refused.
    fn deps_plain(&mut self, dep: &str) -> bool {
        let plain = rt::plain_dependency(dep);
        if !plain {
            self.script_log_app(rt::refused_dependency_line(dep));
        }
        plain
    }

    /// "Add Dependency" answered (`requestDependencyName:`, `DE:341-367`): an empty or
    /// refused answer asks again (the question stays open).
    fn deps_prompt_answer(&mut self, answer: PromptResult) {
        let text = match answer {
            PromptResult::Open => return,
            PromptResult::Cancel => {
                self.script_deps_state.prompt = None;
                return;
            }
            PromptResult::Ok(t) | PromptResult::Extra(t) => t.trim().to_string(),
        };
        if text.is_empty() || !self.deps_plain(&text) {
            return;
        }
        let Some((_, container)) = self.script_deps_state.prompt.take() else { return };
        self.deps_run(&container, vec!["install".to_string(), text.clone()], Pending::Install { container: container.clone(), dep: text });
    }

    /// `runPip3WithArguments:` (`DE:369-411`): "Run this Command?" with the command (and the
    /// download's source: pypi.org, or the index the window's variables or pip's
    /// configuration name); OK runs it.
    fn deps_run(&mut self, container: &Path, pip_args: Vec<String>, pending: Pending) {
        let Some(backend) = pip_backend(container) else { return };
        let venv = container.join(rt::VENV_DIR);
        let (program, args, env) = sd::pip_command(&backend, &venv, &pip_args);
        let exists = |p: &Path| !crate::scripts::ancestors_stay_local(p, &crate::netpath::read_reparse) || crate::localfs::is_local_file(p);
        let index = sd::configured_index(&backend, &venv, &window_env(&env), &exists);
        let text = sd::confirm_text(&program, &args, index.as_deref());
        let buttons = [crate::api_auth::OK, crate::api_auth::CANCEL];
        // `RUN_ID` is persistent in iTerm2: never silenced.
        let then = Then::RunPip(Box::new(RunPip { program, args, env, pending }));
        self.ask_scripts(ask(sd::RUN_HEADING, &text, &buttons, None, Kind::Persistent, then));
    }

    /// "Run this Command?" answered OK: the window opens on the next loop pass.
    pub(super) fn deps_run_confirmed(&mut self, run: RunPip) {
        self.script_deps_state.run = Some(run);
    }

    /// `openSingleUseWindowWithCommand:` for the confirmed command: the default profile
    /// running it with the provisioning variables; the window stays open with pip's output
    /// when it ends, and its end finishes the command (`deps_command_ended`).
    pub(super) fn open_pending_deps_run(&mut self, event_loop: &ActiveEventLoop) {
        let Some(run) = self.script_deps_state.run.take() else { return };
        self.start_command_override = Some(sd::command_text(&run.program, &run.args));
        self.start_env_override = Some(run.env.iter().map(|(k, v)| format!("{k}={v}")).collect());
        let made = self.open_window(event_loop, None);
        self.start_command_override = None;
        self.start_env_override = None;
        let pane = made.ok().and_then(|_| self.tabs.get(self.active).map(|t| t.focused));
        if let Some(p) = pane {
            if let Some(pane) = self.panes.get_mut(&p) {
                pane.hold_open = true;
            }
            self.script_deps_pending = Some((p, run.pending));
        }
        self.deps_push();
        self.redraw_all();
    }

    /// A pane ended or closed: when it ran the editor's pip command, what follows it runs (a
    /// `pip show` of the package, then `deps_version_known`). Once per pane.
    pub(super) fn deps_command_ended(&mut self, pane: PaneId) {
        if self.script_deps_pending.as_ref().is_none_or(|(p, _)| *p != pane) {
            return;
        }
        let Some((_, pending)) = self.script_deps_pending.take() else { return };
        let (container, dep, purpose) = match pending {
            Pending::Install { container, dep } => (container, dep, Purpose::Installed),
            Pending::Uninstall { container, dep } => (container, dep, Purpose::Uninstalled),
            Pending::Upgrade { container, dep } => (container, dep, Purpose::Upgraded),
        };
        self.deps_pip_show(&container, vec![dep], purpose);
        self.deps_push();
    }

    /// A version change confirmed (`pythonVersionChanged:`, `DE:689-744`): the environment is
    /// rebuilt for it with its dependencies, after the download consent. Edits wait until it
    /// ends.
    pub(super) fn deps_version_confirmed(&mut self, container: PathBuf, version: String) {
        let Some(backend) = self.script_deps_state.backend.clone() else { return self.deps_push() };
        let (_, dependencies) = super::scripts_app::container_inputs(&container);
        let interpreter = matches!(backend, Backend::Uv(_)).then(|| rt::two_part(&version));
        let text = rt::packages_consent(&rt::packages_for(&dependencies), interpreter.as_deref());
        self.script_deps_state.rebuilding = Some(container.clone());
        let job = Job::Provision { backend, target: Target::Container(container.clone()), requested: version, dependencies, name: None };
        self.consent(text, vec![(job, After::DepsVersion(container))]);
        self.deps_push();
    }

    /// The rebuilt environment is ready: `setup.cfg` takes the version built (its marker's).
    pub(super) fn deps_version_ready(&mut self, container: &Path) {
        self.script_deps_state.rebuilding = None;
        let built = std::fs::read_to_string(container.join(rt::MARKER)).ok().and_then(|t| rt::Marker::parse(&t)).map(|m| m.python);
        if let Some(built) = built {
            let path = container.join(rt::SETUP_CFG);
            let written = std::fs::read_to_string(&path).ok().and_then(|t| {
                let cfg = rt::SetupCfg::parse(&t)?;
                sd::setup_cfg_with(&t, &cfg.dependencies, &built)
            });
            if let Some(text) = written {
                if let Err(e) = std::fs::write(&path, text) {
                    self.banners.set("scripts", format!("Could not write {}: {e}", path.display()));
                }
            }
        }
        self.deps_refresh(container);
    }

    /// The rebuild failed: "Could Not Change Python Version"; the old environment stays.
    pub(super) fn deps_version_failed(&mut self, container: &Path, e: &str) {
        self.script_deps_state.rebuilding = None;
        self.deps_alert(sd::COULD_NOT_CHANGE, e.to_string());
        self.deps_refresh(container);
    }

    /// The download was declined: nothing changes.
    pub(super) fn deps_version_declined(&mut self) {
        self.script_deps_state.rebuilding = None;
        self.deps_push();
    }

    /// The editor shows `container` again when it is the one selected.
    fn deps_refresh(&mut self, container: &Path) {
        if self.script_deps_state.current().is_some_and(|(p, _)| p == container) {
            let at = self.script_deps_state.selected;
            self.deps_select(at);
        } else {
            self.deps_push();
        }
    }

    /// Upgrade (`upgrade:`, `DE:474-520`): refused when `<parent>\<stem>` exists; else that
    /// folder is built as a full environment for the script's shebang version with no
    /// dependencies (asking before any download), then the script moves in.
    fn deps_upgrade(&mut self, script: &Path) {
        let folder = sd::upgrade_folder(script);
        if crate::localfs::local_exists(&folder) {
            // Persistent in iTerm2: never silenced.
            return self.deps_alert(sd::ERROR, sd::folder_exists(&folder));
        }
        let version = match crate::script_launch::classify(script, &crate::scripts::RealFs, &super::scripts_app::first_line) {
            crate::script_launch::Kind::Basic { version: Some(v), .. } => rt::two_part(&v),
            _ => rt::DEFAULT_PYTHON.to_string(),
        };
        self.ensure_environment(Target::Container(folder.clone()), &version, &[], true, After::DepsUpgrade { script: script.to_path_buf(), folder });
    }

    /// The new environment is built (`finishUpgradingScriptItem:`, `DE:522-564`): its
    /// `setup.cfg` (the one writer, the version built), `<folder>\<name>`, and the script moved
    /// in as its main; then the scan lists it and the editor selects it.
    pub(super) fn deps_upgrade_ready(&mut self, script: &Path, folder: &Path) {
        let name = folder.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let marker = std::fs::read_to_string(folder.join(rt::MARKER)).ok();
        let built = crate::script_new::built_python(marker.as_deref(), rt::DEFAULT_PYTHON);
        let cfg = folder.join(rt::SETUP_CFG);
        if let Err(e) = std::fs::write(&cfg, rt::setup_cfg_after_build(None, &name, &[], &built)) {
            return self.deps_alert(sd::INSTALLATION_FAILED, sd::error_creating(&cfg, &e.to_string()));
        }
        let inner = folder.join(&name);
        if let Err(e) = std::fs::create_dir_all(&inner) {
            return self.deps_alert(sd::INSTALLATION_FAILED, sd::error_creating(&inner, &e.to_string()));
        }
        let dest = crate::script_new::full_main(folder, &name);
        if let Err(e) = std::fs::rename(script, &dest) {
            return self.deps_alert(sd::INSTALLATION_FAILED, sd::error_moving(script, &dest, &e.to_string()));
        }
        self.script_deps_state.want = Some(folder.to_path_buf());
        if let Some(scanner) = &self.scripts_scan {
            scanner.rescan();
        }
    }

    /// The scan's tree changed: an open editor lists the scripts again, keeping its selection
    /// or selecting a script it waits for.
    pub(super) fn deps_tree_changed(&mut self) {
        if self.script_deps.is_none() {
            return;
        }
        let want = self.script_deps_state.want.clone().filter(|w| sd::script_choices(&self.scripts_tree).iter().any(|(_, p, _)| p == w));
        if want.is_some() {
            self.script_deps_state.want = None;
        }
        let keep = want.or_else(|| self.script_deps_state.current().map(|(p, _)| p));
        self.deps_reload(keep);
    }
}
