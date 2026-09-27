//! The Python runtime in the app: which backend builds environments, making sure one is ready
//! before something needs it (asking before any download), Install Python Runtime and Check
//! for Updated Runtime (`AD:3410-3471`), and the provisioner's results.

use std::path::{Path, PathBuf};
use std::time::Instant;

use super::scripts_alerts::{Ask, Then};
use super::App;
use crate::script_provision::{self, Gate, Job, Outcome, Provisioner, RealRunner, Runner, Step, Target};
use crate::script_runtime::{self as rt, Arch, Backend};

/// The Script Console's app-wide entry (`SH:32-43`, "iTerm2 App").
const APP_ENTRY: &str = "Tandem App";

/// What happens once an environment is ready (or what a runtime job reports).
#[derive(Clone, Debug, PartialEq)]
pub(super) enum After {
    /// Nothing waits (Task 5's AutoLaunch rebuild and later callers pass it).
    #[allow(dead_code)]
    Nothing,
    /// Install Python Runtime: a failure shows "Installation Failed".
    InstallRuntime,
    /// Check for Updated Runtime: the result shows as an alert and a console line.
    CheckRuntime,
    /// The REPL, an import and the dependency editor continue here (Tasks 9 and 10).
    #[allow(dead_code)]
    Repl,
    #[allow(dead_code)]
    Import(PathBuf),
    #[allow(dead_code)]
    Deps(PathBuf),
}

/// Tandem's uv folder, `%APPDATA%\Tandem\uv`.
pub(super) fn uv_dir() -> PathBuf {
    let appdata = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    rt::uv_dir(&appdata)
}

/// The processor uv is downloaded for.
fn arch() -> Arch {
    if std::env::consts::ARCH == "aarch64" { Arch::Arm64 } else { Arch::X64 }
}

/// The name an environment-missing line gives: the script or folder it is for.
fn env_name(target: &Target, after: &After) -> String {
    let stem = |p: &Path| p.file_stem().map(|s| s.to_string_lossy().into_owned());
    match (after, target) {
        (After::Import(p) | After::Deps(p), _) => stem(p),
        (_, Target::Container(c)) => c.file_name().map(|s| s.to_string_lossy().into_owned()),
        (_, Target::Shared(minor)) => Some(format!("Python {minor}")),
    }
    .unwrap_or_default()
}

/// An installed Python as a backend: the py launcher with the version `resolve` picks from
/// `py -0p` (never simply its first line), or a `python.exe` with the minor `--version` names.
fn python_backend(argv: &[String]) -> Option<Backend> {
    let program = argv.first()?;
    // Lines are not wanted here; the receiver goes at once.
    let mut runner = RealRunner { lines: std::sync::mpsc::channel().0 };
    if script_provision::is_py_launcher(program) {
        let out = runner.run(&Step { program: PathBuf::from(program), args: vec!["-0p".to_string()], env: Vec::new() }).ok()?;
        let minors: Vec<String> = rt::py_list(&out).into_iter().map(|(v, _)| v).collect();
        if minors.is_empty() {
            return None;
        }
        let (minor, _) = rt::resolve(rt::DEFAULT_PYTHON, &minors)?;
        return Some(Backend::Python { argv: vec![program.clone(), format!("-{minor}")], minor });
    }
    let out = runner.run(&Step { program: PathBuf::from(program), args: vec!["--version".to_string()], env: Vec::new() }).ok()?;
    let minor = rt::two_part(out.split_whitespace().nth(1)?);
    (minor.contains('.') && rt::numeric_version(&minor)).then(|| Backend::Python { argv: vec![program.clone()], minor })
}

impl App {
    /// What builds environments: Tandem's uv when installed, else the first installed Python
    /// by it2api's candidates (11e's `python_candidates`).
    // Launching, New Python Script, the REPL, import and the dependency editor (Tasks 5, 7, 9
    // and 10) call this and `ensure_environment`.
    #[allow(dead_code)]
    pub(super) fn backend(&self) -> Option<Backend> {
        let uv = rt::uv_exe(&uv_dir());
        if uv.is_file() {
            return Some(Backend::Uv(uv));
        }
        let path = std::env::var("PATH").unwrap_or_default();
        crate::util_api::python_candidates(&path, &|p: &Path| p.is_file()).iter().find_map(|argv| python_backend(argv))
    }

    /// A line for the Script Console's app-wide entry.
    pub(super) fn script_log_app(&mut self, text: String) {
        self.api_log.push(crate::api_auth::LogEntry { key: None, name: APP_ENTRY.to_string(), text });
    }

    fn runtime_alert(&mut self, heading: &str, text: String) {
        self.ask_scripts(Ask { heading: heading.to_string(), text, buttons: vec![crate::api_auth::OK.to_string()], silence: None, then: Then::Nothing });
    }

    fn consent(&mut self, (heading, text): (&str, String), jobs: Vec<(Job, After)>) {
        let buttons = vec![crate::api_auth::OK.to_string(), "Cancel".to_string()];
        self.ask_scripts(Ask { heading: heading.to_string(), text, buttons, silence: None, then: Then::Consent(jobs) });
    }

    /// Makes sure `target` is ready, then continues with `after`. Ready now: at once. Else a
    /// download is needed: a person's action asks first, naming what will download; any
    /// other path logs the environment-missing line and stops.
    #[allow(dead_code)]
    pub(super) fn ensure_environment(&mut self, target: Target, requested: &str, dependencies: &[String], explicit: bool, after: After) {
        let dir = uv_dir();
        if let Some(python) = script_provision::ready_python(&target, &dir, &|p: &Path| p.exists(), &|p: &Path| std::fs::read_to_string(p).ok()) {
            return self.environment_ready(after, python);
        }
        if script_provision::gate(explicit, true) == Gate::Refuse {
            let line = rt::env_missing_line(&env_name(&target, &after));
            return self.script_log_app(line);
        }
        match self.backend() {
            // No Python at all: uv first. Installing uv builds the default shared environment,
            // so that target needs nothing more.
            None => {
                let install = Job::InstallUv { arch: arch() };
                let jobs = if target == Target::Shared(rt::DEFAULT_PYTHON.to_string()) {
                    vec![(install, after)]
                } else {
                    let build = Job::Provision { backend: Backend::Uv(rt::uv_exe(&dir)), target, requested: requested.to_string(), dependencies: dependencies.to_vec(), name: None };
                    vec![(install, After::InstallRuntime), (build, after)]
                };
                self.consent(rt::uv_consent(rt::UV_MEGABYTES), jobs);
            }
            Some(backend) => {
                let packages = rt::packages_for(dependencies);
                // Only uv may fetch a Python as well.
                let interpreter = matches!(backend, Backend::Uv(_)).then(|| rt::two_part(requested));
                let text = rt::packages_consent(&packages, interpreter.as_deref());
                let build = Job::Provision { backend, target, requested: requested.to_string(), dependencies: dependencies.to_vec(), name: None };
                self.consent(text, vec![(build, after)]);
            }
        }
    }

    /// Scripts > Manage > Install Python Runtime, or Check for Updated Runtime once uv is
    /// installed (`AD:3443-3471`), each after asking.
    pub(super) fn install_python_runtime(&mut self) {
        self.uv_installed = rt::uv_exe(&uv_dir()).is_file();
        self.runtime_menu_title();
        if self.uv_installed {
            self.consent(rt::update_consent(), vec![(Job::CheckForUpdate { arch: arch() }, After::CheckRuntime)]);
        } else {
            self.consent(rt::uv_consent(rt::UV_MEGABYTES), vec![(Job::InstallUv { arch: arch() }, After::InstallRuntime)]);
        }
    }

    /// Submits consented jobs, in order, to the provisioner (started on first use).
    pub(super) fn provision_jobs(&mut self, jobs: Vec<(Job, After)>) {
        let notify = self.notify.clone();
        let provisioner = self.provisioner.get_or_insert_with(|| Provisioner::start(uv_dir(), notify));
        let ids: Vec<(u64, After)> = jobs.into_iter().map(|(job, after)| (provisioner.submit(job), after)).collect();
        self.provision_after.extend(ids);
        self.provision_busy(true);
    }

    /// While a job runs the "scripts" banner says so and the Scripts folder is not rescanned
    /// (a build makes many file events; iTerm2 skips them), then it is scanned once.
    fn provision_busy(&mut self, busy: bool) {
        if busy {
            self.banners.set("scripts", rt::SETTING_UP);
        } else {
            self.banners.clear("scripts");
        }
        if let Some(scanner) = self.scripts_scan.as_mut() {
            scanner.hold(busy);
        }
        self.redraw();
    }

    /// Takes the provisioner's output lines (to the app-wide console entry) and results.
    pub(super) fn provision_tick(&mut self) -> Option<Instant> {
        let p = self.provisioner.as_ref()?;
        let lines: Vec<String> = std::iter::from_fn(|| p.try_line()).collect();
        let done: Vec<(u64, Result<Outcome, String>)> = std::iter::from_fn(|| p.try_done()).collect();
        let busy = p.busy();
        for line in lines {
            self.script_log_app(format!("{line}\n"));
        }
        if done.is_empty() {
            return None;
        }
        for (id, result) in done {
            if let Some(after) = self.provision_after.remove(&id) {
                self.provision_done(id, after, result);
            }
        }
        self.provision_busy(busy);
        None
    }

    fn provision_done(&mut self, id: u64, after: After, result: Result<Outcome, String>) {
        let dir = uv_dir();
        let had_uv = self.uv_installed;
        self.uv_installed = rt::uv_exe(&dir).is_file();
        if self.uv_installed != had_uv {
            self.runtime_menu_title();
        }
        match result {
            Ok(Outcome::Installed(_)) => {
                if after != After::InstallRuntime {
                    self.environment_ready(after, rt::venv_python(&rt::shared_venv(&dir, rt::DEFAULT_PYTHON)));
                }
            }
            Ok(Outcome::Provisioned { python, .. }) => self.environment_ready(after, python),
            Ok(Outcome::Updated(message)) => {
                self.script_log_app(format!("{message}\n"));
                self.runtime_alert(rt::RUNTIME_HEADING, message);
            }
            Ok(Outcome::Minors(_)) => {}
            Err(e) => {
                self.script_log_app(format!("{e}\n"));
                match after {
                    After::InstallRuntime => {
                        // What was queued behind the install needs uv; without it each would
                        // only fail again, so one alert says why.
                        if !self.uv_installed {
                            self.provision_after.retain(|k, _| *k < id);
                        }
                        self.runtime_alert(rt::INSTALL_FAILED_HEADING, rt::install_failed(&e));
                    }
                    After::CheckRuntime => self.runtime_alert(rt::UPDATE_FAILED_HEADING, e),
                    _ => self.runtime_alert(rt::ENV_UNAVAILABLE_HEADING, rt::env_unavailable(&e)),
                }
            }
        }
    }

    /// The environment is ready: what waited on it goes on.
    fn environment_ready(&mut self, after: After, _python: PathBuf) {
        match after {
            After::Nothing | After::InstallRuntime | After::CheckRuntime => {}
            // Their tasks (9 and 10) continue these with the interpreter.
            After::Repl | After::Import(_) | After::Deps(_) => {}
        }
    }

    /// The Scripts > Manage runtime item's title follows whether uv is installed (`SR:62-79`).
    pub(super) fn runtime_menu_title(&mut self) {
        if let Some(item) = self.menu_bar.as_ref().and_then(|b| b.install_runtime.as_ref()) {
            item.set_text(rt::menu_title(self.uv_installed));
        }
    }
}
