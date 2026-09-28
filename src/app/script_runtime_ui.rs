//! The Python runtime in the app: which backend builds environments, making sure one is ready
//! before something needs it (asking before any download), Install Python Runtime and Check
//! for Updated Runtime (`AD:3410-3471`), and the provisioner's results.

use std::path::{Path, PathBuf};
use std::time::Instant;

use super::scripts_alerts::{ask, Then};
use crate::warnings::Kind;
use super::App;
use crate::script_provision::{self, Gate, Job, Outcome, Provisioner, RealRunner, Runner, Step, Target};
use crate::script_runtime::{self as rt, Arch, Backend};

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
    /// A script launch runs the script (Task 5).
    Launch(crate::script_launch::Request),
    /// "Rebuild Python Environment?" answered Rebuild: the script launches; a failure shows
    /// "Could Not Rebuild Environment".
    Rebuild(crate::script_launch::Request),
    /// Open Python REPL: its window opens with the interpreter; a failure falls back to the
    /// newest shared environment (`AD:3553-3595`); a declined download is silent.
    Repl,
    /// An imported full environment (the folder being built) is moved into place; a failure
    /// or a declined download ends the import (restoring what it replaced).
    Import(PathBuf),
    /// Manage Dependencies changed a full environment's Python version (the container): its
    /// `setup.cfg` takes the version built; a failure shows "Could Not Change Python Version";
    /// a declined download changes nothing.
    DepsVersion(PathBuf),
    /// Manage Dependencies' Upgrade: the basic script moves into the full environment built
    /// in `folder`; a failure shows "Installation Failed" (`DE:492-500`).
    DepsUpgrade { script: PathBuf, folder: PathBuf },
    /// New Python Script's full environment is built: its files are written; a failure shows
    /// "Installation Failed" (`M:1180-1197`).
    NewScript(crate::script_new::Plan),
}

/// The "scripts" banner while jobs run: "Rebuilding the Python environment\u{2026}" while a
/// rebuild (Rebuild Python Environment? or a Python version change) is among them
/// (`DE:713`), else "Setting up the Python environment\u{2026}".
fn busy_banner<'a>(mut pending: impl Iterator<Item = &'a After>) -> &'static str {
    if pending.any(|a| matches!(a, After::Rebuild(_) | After::DepsVersion(_))) { crate::script_launch::REBUILDING } else { rt::SETTING_UP }
}

/// The environment a consented job makes: a build's target; installing uv (for anything but
/// Install Python Runtime itself) builds the default shared environment.
fn job_target(job: &Job, after: &After) -> Option<Target> {
    match job {
        Job::Provision { target, .. } => Some(target.clone()),
        Job::InstallUv { .. } if *after != After::InstallRuntime => Some(Target::Shared(rt::DEFAULT_PYTHON.to_string())),
        _ => None,
    }
}

/// Whether `after` repeats a launch already waiting (`pending`) for the same environment:
/// the same script launched or rebuilt again before its environment is ready, as
/// `script_after_enable` drops a repeat by path.
fn repeats(pending: &[&After], after: &After) -> bool {
    let path = |a: &After| match a {
        After::Launch(r) | After::Rebuild(r) => Some(r.path.clone()),
        _ => None,
    };
    path(after).is_some_and(|p| pending.iter().any(|a| path(a).as_ref() == Some(&p)))
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
        (After::Import(p) | After::DepsVersion(p) | After::DepsUpgrade { folder: p, .. }, _) => stem(p),
        // iTerm2 names the launched path's last component (`M:915-917`).
        (After::Launch(r) | After::Rebuild(r), _) => r.path.file_name().map(|s| s.to_string_lossy().into_owned()),
        (_, Target::Container(c)) => c.file_name().map(|s| s.to_string_lossy().into_owned()),
        (_, Target::Shared(minor)) => Some(format!("Python {minor}")),
    }
    .unwrap_or_default()
}

/// An installed Python as a backend: the py launcher with the version `resolve` picks from
/// `py -0p` (never simply its first line), or a `python.exe` with the minor `--version` names.
/// Each probe may take `PROBE_TIMEOUT`.
fn python_backend(argv: &[String]) -> Option<Backend> {
    let program = argv.first()?;
    // Lines are not wanted here; the receiver goes at once.
    let mut runner = RealRunner { lines: std::sync::mpsc::channel().0, timeout: script_provision::PROBE_TIMEOUT };
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

/// What builds environments: Tandem's uv when installed, else the first installed Python by
/// it2api's candidates (11e's `python_candidates`). Probing a Python runs it, so off the UI
/// thread only (`ensure_environment` probes on a thread of its own).
pub(super) fn find_backend() -> Option<Backend> {
    let uv = rt::uv_exe(&uv_dir());
    if uv.is_file() {
        return Some(Backend::Uv(uv));
    }
    let path = std::env::var("PATH").unwrap_or_default();
    crate::util_api::python_candidates(&path, &|p: &Path| p.is_file()).iter().find_map(|argv| python_backend(argv))
}

/// A launch, rebuild or REPL for an environment already being asked about or built: it runs
/// again once that is built, and goes quietly when it fails or is declined.
pub(super) struct Waiting {
    target: Target,
    requested: String,
    dependencies: Vec<String>,
    after: After,
}

/// An `ensure_environment` waiting for its backend probe: the answer, then what it was asked.
pub(super) struct Probe {
    answer: std::sync::mpsc::Receiver<Option<Backend>>,
    target: Target,
    requested: String,
    dependencies: Vec<String>,
    after: After,
}

impl App {

    /// A line for the Script Console's app-wide entry.
    pub(super) fn script_log_app(&mut self, text: String) {
        let stamp = self.history_stamp();
        self.script_history.add_log(crate::script_history::GLOBAL_ID, &text, &stamp);
    }

    fn runtime_alert(&mut self, heading: &str, text: String) {
        self.ask_scripts(ask(heading, &text, &[crate::api_auth::OK], None, Kind::Persistent, Then::Nothing));
    }

    pub(super) fn consent(&mut self, (heading, text): (&str, String), jobs: Vec<(Job, After)>) {
        let buttons = [crate::api_auth::OK, "Cancel"];
        self.ask_scripts(ask(heading, &text, &buttons, None, Kind::Persistent, Then::Consent(jobs)));
    }

    /// Makes sure `target` is ready, then continues with `after`. Ready now: at once. Else a
    /// download is needed: a person's action asks first, naming what will download; any
    /// other path logs the environment-missing line and stops.
    pub(super) fn ensure_environment(&mut self, target: Target, requested: &str, dependencies: &[String], explicit: bool, after: After) {
        let dir = uv_dir();
        if let Some(python) = script_provision::ready_python(&target, &dir, &|p: &Path| p.exists(), &|p: &Path| std::fs::read_to_string(p).ok()) {
            return self.environment_ready(after, python);
        }
        if script_provision::gate(explicit, true) == Gate::Refuse {
            let line = rt::env_missing_line(&env_name(&target, &after));
            return self.script_log_app(line);
        }
        // One question and one build per environment (iTerm2's modal alert allows no second
        // ask): a launch, rebuild or REPL for one already probed, asked about or building
        // waits for it, and the same script's launch again is dropped. The rest (imports, new
        // scripts, upgrades) build a folder of their own.
        let folds = matches!(after, After::Launch(_) | After::Rebuild(_) | After::Repl);
        if let Some(pending) = self.pending_for(&target).filter(|_| folds) {
            if !repeats(&pending, &after) {
                self.provision_waiting.push(Waiting { target, requested: requested.to_string(), dependencies: dependencies.to_vec(), after });
            }
            return;
        }
        // Tandem's uv answers at once; an installed Python is probed off the UI thread.
        let uv = rt::uv_exe(&dir);
        if uv.is_file() {
            return self.ask_for_environment(Some(Backend::Uv(uv)), target, requested, dependencies, after);
        }
        let (tx, answer) = std::sync::mpsc::channel();
        let notify = self.notify.clone();
        std::thread::spawn(move || {
            let _ = tx.send(find_backend());
            notify();
        });
        self.backend_probes.push(Probe { answer, target, requested: requested.to_string(), dependencies: dependencies.to_vec(), after });
    }

    /// What waits on `target` when it is being probed for, asked about or built (and what
    /// already waits behind that); None when it is not.
    fn pending_for(&self, target: &Target) -> Option<Vec<&After>> {
        let mut pending: Vec<&After> = self.backend_probes.iter().filter(|p| p.target == *target).map(|p| &p.after).collect();
        let asks = self.scripts_alerts.iter().chain(self.scripts_alert.as_ref().map(|(_, a)| a));
        for ask in asks {
            if let Then::Consent(jobs) = &ask.then {
                pending.extend(jobs.iter().filter(|(j, a)| job_target(j, a).as_ref() == Some(target)).map(|(_, a)| a));
            }
        }
        pending.extend(self.provision_targets.iter().filter(|(_, t)| *t == target).filter_map(|(id, _)| self.provision_after.get(id)));
        if pending.is_empty() {
            return None;
        }
        pending.extend(self.provision_waiting.iter().filter(|w| w.target == *target).map(|w| &w.after));
        Some(pending)
    }

    /// The build of `target` ended, was declined or dropped: what waited on it asks again
    /// when it is built (finding it ready), else it goes (the build's own alert, or the
    /// declined question, said why) unless another build of it is still under way.
    fn release_waiting(&mut self, target: &Target, built: bool) {
        // Another question or build for it still under way keeps them waiting for that.
        if !built && self.pending_for(target).is_some() {
            return;
        }
        let (now, rest): (Vec<Waiting>, Vec<Waiting>) = std::mem::take(&mut self.provision_waiting).into_iter().partition(|w| w.target == *target);
        self.provision_waiting = rest;
        if built {
            for w in now {
                self.ensure_environment(w.target, &w.requested, &w.dependencies, true, w.after);
            }
        }
    }

    /// Asks before building `target` with `backend` (None: no Python at all).
    fn ask_for_environment(&mut self, backend: Option<Backend>, target: Target, requested: &str, dependencies: &[String], after: After) {
        match backend {
            // No Python at all: uv first. Installing uv builds the default shared environment,
            // so that target needs nothing more. One question names everything downloaded.
            None => {
                let install = Job::InstallUv { arch: arch() };
                let (jobs, packages) = if target == Target::Shared(rt::DEFAULT_PYTHON.to_string()) {
                    (vec![(install, after)], rt::packages_for(&[]))
                } else {
                    let build = Job::Provision { backend: Backend::Uv(rt::uv_exe(&uv_dir())), target, requested: requested.to_string(), dependencies: dependencies.to_vec(), name: None };
                    (vec![(install, After::InstallRuntime), (build, after)], rt::packages_for(dependencies))
                };
                self.consent(rt::uv_consent(rt::UV_MEGABYTES, &packages), jobs);
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
            self.consent(rt::uv_consent(rt::UV_MEGABYTES, &rt::packages_for(&[])), vec![(Job::InstallUv { arch: arch() }, After::InstallRuntime)]);
        }
    }

    /// Submits consented jobs, in order, to the provisioner (started on first use).
    pub(super) fn provision_jobs(&mut self, jobs: Vec<(Job, After)>) {
        let notify = self.notify.clone();
        let provisioner = self.provisioner.get_or_insert_with(|| Provisioner::start(uv_dir(), notify));
        // What follows an install in the same group needs its uv.
        let mut install = None;
        for (job, after) in jobs {
            let is_install = matches!(job, Job::InstallUv { .. });
            let target = job_target(&job, &after);
            let id = provisioner.submit(job);
            if let Some(t) = target {
                self.provision_targets.insert(id, t);
            }
            if is_install {
                install = Some(id);
            } else if let Some(i) = install {
                self.provision_needs.insert(id, i);
            }
            self.provision_after.insert(id, after);
        }
        self.provision_busy(true);
    }

    /// While a job runs the "scripts" banner says so and the Scripts folder is not rescanned
    /// (a build makes many file events; iTerm2 skips them), then it is scanned once.
    fn provision_busy(&mut self, busy: bool) {
        if busy {
            self.banners.set("scripts", busy_banner(self.provision_after.values()));
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
        self.probes_tick();
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
            self.provision_needs.remove(&id);
            let target = self.provision_targets.remove(&id);
            let built = result.is_ok();
            if let Some(after) = self.provision_after.remove(&id) {
                self.provision_done(id, after, result);
            }
            if let Some(t) = target {
                self.release_waiting(&t, built);
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
                        // What was queued behind this install needs its uv; without it each
                        // would only fail again, so one alert says why. Other jobs report.
                        let mut dropped = Vec::new();
                        if !self.uv_installed {
                            let needing: Vec<u64> = self.provision_needs.iter().filter(|(_, i)| **i == id).map(|(k, _)| *k).collect();
                            for k in needing {
                                self.provision_needs.remove(&k);
                                dropped.extend(self.provision_after.remove(&k));
                                if let Some(t) = self.provision_targets.remove(&k) {
                                    self.release_waiting(&t, false);
                                }
                            }
                        }
                        self.runtime_alert(rt::INSTALL_FAILED_HEADING, rt::install_failed(&e));
                        // An import waiting on it ends (its replaced script comes back).
                        if dropped.iter().any(|a| matches!(a, After::Import(_))) {
                            self.import_install_done(Err(super::scripts_app::Failure::Error(crate::script_archive::runtime_install_failed(&e))));
                        }
                    }
                    After::CheckRuntime => self.runtime_alert(rt::UPDATE_FAILED_HEADING, e),
                    After::Rebuild(_) => self.runtime_alert(crate::script_launch::COULD_NOT_REBUILD, e),
                    After::NewScript(_) => self.runtime_alert(crate::script_new::INSTALLATION_FAILED, crate::script_new::env_create_error(&e)),
                    After::Repl => self.repl_environment_failed(&e),
                    After::Import(_) => self.import_install_done(Err(super::scripts_app::Failure::Error(crate::script_archive::runtime_install_failed(&e)))),
                    After::DepsVersion(container) => self.deps_version_failed(&container, &e),
                    After::DepsUpgrade { .. } => self.runtime_alert(crate::script_deps::INSTALLATION_FAILED, crate::script_deps::upgrade_error(&e)),
                    _ => self.runtime_alert(rt::ENV_UNAVAILABLE_HEADING, rt::env_unavailable(&e)),
                }
            }
        }
    }

    /// Finished backend probes go on to their question.
    fn probes_tick(&mut self) {
        let mut i = 0;
        while i < self.backend_probes.len() {
            let answer = match self.backend_probes[i].answer.try_recv() {
                Ok(b) => b,
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    i += 1;
                    continue;
                }
                // The probe thread died: treated as no Python found.
                Err(std::sync::mpsc::TryRecvError::Disconnected) => None,
            };
            let p = self.backend_probes.remove(i);
            self.ask_for_environment(answer, p.target, &p.requested, &p.dependencies, p.after);
        }
    }

    /// The environment is ready: what waited on it goes on.
    fn environment_ready(&mut self, after: After, python: PathBuf) {
        match after {
            After::Nothing | After::InstallRuntime | After::CheckRuntime => {}
            After::Launch(req) => self.script_environment_ready(req, python),
            After::Rebuild(req) => self.launch_script(req),
            After::NewScript(plan) => self.finish_new_script(plan),
            After::Repl => self.repl_ready(python),
            After::Import(folder) => self.import_environment_ready(&folder),
            After::DepsVersion(container) => self.deps_version_ready(&container),
            After::DepsUpgrade { script, folder } => self.deps_upgrade_ready(&script, &folder),
        }
    }

    /// A download consent was declined (or never shown): nothing is built; an import waiting
    /// on it ends, restoring what it replaced (`IM:387-406`). The REPL stays silent
    /// (`AD:3558-3561`).
    pub(super) fn consent_declined(&mut self, jobs: Vec<(Job, After)>) {
        for t in jobs.iter().filter_map(|(j, a)| job_target(j, a)) {
            self.release_waiting(&t, false);
        }
        if jobs.iter().any(|(_, a)| matches!(a, After::Import(_))) {
            self.import_install_done(Err(super::scripts_app::Failure::Canceled));
        }
        if jobs.iter().any(|(_, a)| matches!(a, After::DepsVersion(_))) {
            self.deps_version_declined();
        }
    }

    /// The Scripts > Manage runtime item's title follows whether uv is installed (`SR:62-79`).
    pub(super) fn runtime_menu_title(&mut self) {
        if let Some(item) = self.menu_bar.as_ref().and_then(|b| b.install_runtime.as_ref()) {
            item.set_text(rt::menu_title(self.uv_installed));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn busy_banner_follows_iterm2() {
        let req = crate::script_launch::Request { path: PathBuf::from(r"C:\S\a.py"), args: Vec::new(), explicit: true };
        assert_eq!(busy_banner([After::Repl].iter()), rt::SETTING_UP);
        assert_eq!(busy_banner([After::Repl, After::Rebuild(req)].iter()), crate::script_launch::REBUILDING);
        // A Python version change from Manage Dependencies rebuilds too (`DE:713`).
        assert_eq!(busy_banner([After::DepsVersion(PathBuf::from(r"C:\S\t"))].iter()), crate::script_launch::REBUILDING);
        assert_eq!(busy_banner([After::DepsUpgrade { script: PathBuf::from(r"C:\S\t.py"), folder: PathBuf::from(r"C:\S\t") }].iter()), rt::SETTING_UP);
    }

    #[test]
    fn one_build_per_environment() {
        let shared = Target::Shared("3.12".into());
        let build = Job::Provision { backend: Backend::Uv(PathBuf::from(r"C:\u\bin\uv.exe")), target: shared.clone(), requested: "3.12".into(), dependencies: Vec::new(), name: None };
        assert_eq!(job_target(&build, &After::Repl), Some(shared));
        let install = Job::InstallUv { arch: Arch::X64 };
        assert_eq!(job_target(&install, &After::Repl), Some(Target::Shared(rt::DEFAULT_PYTHON.to_string())));
        // Install Python Runtime is not a request for an environment.
        assert_eq!(job_target(&install, &After::InstallRuntime), None);
        assert_eq!(job_target(&Job::CheckForUpdate { arch: Arch::X64 }, &After::CheckRuntime), None);

        let req = |p: &str| crate::script_launch::Request { path: PathBuf::from(p), args: Vec::new(), explicit: true };
        let a = After::Launch(req(r"C:\S\a.py"));
        let rebuild = After::Rebuild(req(r"C:\S\a.py"));
        // The same script again: a repeat, whether it waits to launch or to rebuild.
        assert!(repeats(&[&a], &After::Launch(req(r"C:\S\a.py"))));
        assert!(repeats(&[&After::Repl, &rebuild], &After::Launch(req(r"C:\S\a.py"))));
        assert!(repeats(&[&a], &After::Rebuild(req(r"C:\S\a.py"))));
        // Another script on the same shared environment, or the REPL, waits its turn.
        assert!(!repeats(&[&a], &After::Launch(req(r"C:\S\b.py"))));
        assert!(!repeats(&[&a], &After::Repl));
        assert!(!repeats(&[], &a));
    }
}
