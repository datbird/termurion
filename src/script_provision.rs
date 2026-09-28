//! Provisioning Python environments for API scripts (iTerm2's `iTermUvProvisioner`): installing
//! Tandem's pinned uv (its SHA-256 checked before it is unpacked), building a shared or a
//! script's own environment aside and swapping it into place, the update check, and the gate
//! that keeps a download from happening without a person asking. Every step goes through a
//! `Runner`, so the order is tested without running or downloading anything; `RealRunner`
//! (curl.exe, tar.exe, uv, python) and the `Provisioner` worker are Windows-only.

use std::cmp::Ordering;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::script_runtime::{
    dotted, numeric_version, packages_for, pip_install_args, py_list, refused_dependencies, refused_dependency_line, resolve, setup_cfg_after_build, shared_venv, two_part, upgraded, up_to_date, uv_env, uv_exe, uv_minors, uv_pip_install_args, uv_python_list_args,
    uv_sha256, uv_venv_args, uv_version, uv_zip_url, venv_args, venv_python, venvs_dir, Arch, Backend, Marker, DEFAULT_PYTHON, MARKER, NOT_INSTALLED, PACKAGES, SETUP_CFG, UPDATE_DOWNLOAD_FAILED, UV_VERSION, VENV_DIR,
};

/// The text when the downloaded uv zip's SHA-256 is not the pinned one.
pub use crate::script_runtime::CHECKSUM_MISMATCH;
/// How long one uv or pip run may take before it is stopped (`UV:554`, iTerm2's
/// `uvBuildTimeout`): a safety net for a hung process, not a slow-link cap.
pub const BUILD_TIMEOUT: Duration = Duration::from_secs(3600);

/// Where shared-environment remaps are recorded: `<venvs>\.remaps\<requested minor>` holds the
/// minor it was built as, so a later launch finds it without asking uv (`UV:835-866`).
const REMAPS: &str = ".remaps";

/// One program run: the program, its arguments, and variables added to Tandem's environment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

/// What provisioning does to the world. `run` answers the program's output (stdout and stderr
/// together) or an error holding it; `log` writes a line to the Script Console.
pub trait Runner {
    fn run(&mut self, step: &Step) -> Result<String, String>;
    fn download(&mut self, url: &str, to: &Path) -> Result<(), String>;
    fn sha256(&mut self, file: &Path) -> Result<String, String>;
    fn extract(&mut self, zip: &Path, to: &Path) -> Result<(), String>;
    fn exists(&self, p: &Path) -> bool;
    /// The file `dir\name`, when it is a file (not a folder).
    fn find_file(&self, dir: &Path, name: &str) -> Option<PathBuf>;
    /// The folders directly in `dir`, sorted.
    fn list_dirs(&self, dir: &Path) -> Vec<PathBuf>;
    fn rename(&mut self, from: &Path, to: &Path) -> Result<(), String>;
    /// Removes a file or a folder and everything in it; a missing one is fine.
    fn remove(&mut self, p: &Path);
    fn write(&mut self, p: &Path, text: &str) -> Result<(), String>;
    fn log(&mut self, line: &str);
}

/// The environment a job builds: a shared one for a Python version (basic scripts and the
/// REPL, under `<uv>\venvs`), or a script folder's own `.venv`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Shared(String),
    Container(PathBuf),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Job {
    /// Downloads the pinned uv and builds the default shared environment (iTerm2's
    /// `downloadAndProvisionSharedVenv` for the default version, `AD:3443-3471`).
    InstallUv { arch: Arch },
    /// Builds `target` for the `requested` version with the dependencies; with a `name` a
    /// script folder also gets its `setup.cfg`.
    Provision { backend: Backend, target: Target, requested: String, dependencies: Vec<String>, name: Option<String> },
    /// Check for Updated Runtime (`UV:1084-1131`).
    CheckForUpdate { arch: Arch },
    /// The Python minors a backend can build.
    Minors { backend: Backend },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    Installed(PathBuf),
    Provisioned { python: PathBuf, marker: Marker },
    Updated(String),
    Minors(Vec<String>),
}

/// Whether provisioning may go on: at once, after asking, or not at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gate {
    Go,
    Ask,
    Refuse,
}

/// Nothing is downloaded without a person asking: with nothing to download provisioning goes
/// on; a download is asked about when a person started it (`explicit`) and refused otherwise
/// (AutoLaunch and other paths no one chose).
pub fn gate(explicit: bool, needs_download: bool) -> Gate {
    match (explicit, needs_download) {
        (_, false) => Gate::Go,
        (true, true) => Gate::Ask,
        (false, true) => Gate::Refuse,
    }
}

/// uv.exe in an unpacked zip: at its root, else one folder down (`UV:170-190`).
pub fn locate_uv(dir: &Path, r: &dyn Runner) -> Option<PathBuf> {
    r.find_file(dir, "uv.exe").or_else(|| r.list_dirs(dir).iter().find_map(|d| r.find_file(d, "uv.exe")))
}

/// A target's interpreter when it is ready to run: the interpreter and the marker both exist
/// (a half-built environment has only the first). A shared version that was remapped is
/// found through its remap record (`UV:843-866`).
pub fn ready_python(target: &Target, uv_dir: &Path, exists: &dyn Fn(&Path) -> bool, read: &dyn Fn(&Path) -> Option<String>) -> Option<PathBuf> {
    let ready = |venv: &Path, marker: &Path| (exists(&venv_python(venv)) && exists(marker)).then(|| venv_python(venv));
    match target {
        Target::Container(c) => ready(&c.join(VENV_DIR), &c.join(MARKER)),
        Target::Shared(requested) => {
            if !numeric_version(requested) {
                return None;
            }
            let minor = two_part(requested);
            let venv = shared_venv(uv_dir, &minor);
            ready(&venv, &venv.join(MARKER)).or_else(|| {
                let to = read(&venvs_dir(uv_dir).join(REMAPS).join(&minor))?;
                let to = to.trim();
                if to.is_empty() || !numeric_version(to) {
                    return None;
                }
                let venv = shared_venv(uv_dir, to);
                ready(&venv, &venv.join(MARKER))
            })
        }
    }
}

/// The newest provisioned shared environment's interpreter
/// (`newestProvisionedSharedVenvInterpreter`, `UV:1278-1293`, `UV:896-902`): among the venvs
/// folder's entries (`names`), only `<minor>` folders holding an interpreter and a marker
/// count (a `.building` or `.old` folder or `.remaps` never does), the highest version wins.
pub fn newest_shared_python(uv_dir: &Path, names: &[String], exists: &dyn Fn(&Path) -> bool) -> Option<PathBuf> {
    names
        .iter()
        .filter(|n| numeric_version(n))
        .filter(|n| {
            let venv = shared_venv(uv_dir, n);
            exists(&venv_python(&venv)) && exists(&venv.join(MARKER))
        })
        .max_by(|a, b| dotted(a, b))
        .map(|n| venv_python(&shared_venv(uv_dir, n)))
}

/// Runs one job to its end.
pub fn run_job(job: &Job, uv_dir: &Path, r: &mut dyn Runner) -> Result<Outcome, String> {
    match job {
        Job::InstallUv { arch } => {
            let uv = install_uv(*arch, uv_dir, false, r)?;
            let target = Target::Shared(DEFAULT_PYTHON.to_string());
            provision(&Backend::Uv(uv.clone()), &target, DEFAULT_PYTHON, &[], None, uv_dir, r)?;
            Ok(Outcome::Installed(uv))
        }
        Job::Provision { backend, target, requested, dependencies, name } => {
            provision(backend, target, requested, dependencies, name.as_deref(), uv_dir, r).map(|(python, marker)| Outcome::Provisioned { python, marker })
        }
        Job::CheckForUpdate { arch } => check_for_update(*arch, uv_dir, r).map(Outcome::Updated),
        Job::Minors { backend } => Ok(Outcome::Minors(minors(backend, uv_dir, r))),
    }
}

/// `path` with `suffix` added to its last part: `.venv` and ".building" make `.venv.building`.
fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut s = OsString::from(path.as_os_str());
    s.push(suffix);
    PathBuf::from(s)
}

fn uv_step(uv: &Path, args: Vec<String>, uv_dir: &Path, downloads: bool) -> Step {
    Step { program: uv.to_path_buf(), args, env: uv_env(uv_dir, downloads) }
}

/// The installed uv's version for the marker, "unknown" when it cannot be read (`UV:316-333`).
fn installed_uv_version(uv: &Path, uv_dir: &Path, r: &mut dyn Runner) -> String {
    r.run(&uv_step(uv, vec!["--version".to_string()], uv_dir, false)).ok().and_then(|o| uv_version(&o)).unwrap_or_else(|| "unknown".to_string())
}

/// Downloads the pinned uv zip, checks its SHA-256 before anything reads it, unpacks it aside
/// and moves uv.exe into place; the zip and the unpacked folder are removed whatever happens.
/// For the update check a failed download reads as iTerm2's update download failure.
fn install_uv(arch: Arch, uv_dir: &Path, update: bool, r: &mut dyn Runner) -> Result<PathBuf, String> {
    let bin = uv_dir.join("bin");
    let zip = bin.join(".uv-download.zip");
    let staging = bin.join(".uv-staging");
    let dest = uv_exe(uv_dir);
    let url = uv_zip_url(arch);
    r.log(&format!("Downloading uv {UV_VERSION} from {url}"));
    if let Err(e) = r.download(&url, &zip) {
        r.remove(&zip);
        if update {
            r.log(&e);
            return Err(UPDATE_DOWNLOAD_FAILED.to_string());
        }
        return Err(e);
    }
    let digest = r.sha256(&zip);
    if !digest.as_deref().is_ok_and(|d| d.eq_ignore_ascii_case(uv_sha256(arch))) {
        r.remove(&zip);
        return Err(digest.err().unwrap_or_else(|| CHECKSUM_MISMATCH.to_string()));
    }
    r.remove(&staging);
    let placed = r.extract(&zip, &staging).and_then(|()| {
        let found = locate_uv(&staging, r).ok_or_else(|| "The uv download did not contain a uv binary.".to_string())?;
        r.rename(&found, &dest)
    });
    r.remove(&zip);
    r.remove(&staging);
    placed.map(|()| dest)
}

/// Builds `target` aside (`.building`), installs its packages, swaps it into place (an old
/// environment moves to `.old` and is removed after), then writes the marker and, with a
/// name, the script's `setup.cfg` (`UV:349-455`). A failure removes only the new build, so a
/// working environment survives a failed rebuild.
fn provision(backend: &Backend, target: &Target, requested: &str, deps: &[String], name: Option<&str>, uv_dir: &Path, r: &mut dyn Runner) -> Result<(PathBuf, Marker), String> {
    for d in refused_dependencies(deps) {
        r.log(refused_dependency_line(&d).trim_end());
    }
    let packages = packages_for(deps);
    let (minor, remapped_from) = match backend {
        Backend::Uv(uv) => {
            let listed = r.run(&uv_step(uv, uv_python_list_args(), uv_dir, false));
            let available = listed.map(|o| uv_minors(&o)).unwrap_or_default();
            resolve(requested, &available).ok_or_else(|| format!("\u{201c}{requested}\u{201d} is not a Python version."))?
        }
        Backend::Python { argv, minor } => {
            let asked = numeric_version(requested).then(|| two_part(requested));
            match asked {
                // The py launcher builds the version asked for when it is installed, else the
                // nearest by `resolve` (the version popup lists the launcher's minors).
                Some(a) if a != *minor && argv.first().is_some_and(|p| is_py_launcher(p)) => {
                    let installed = py_minors(&argv[0], r);
                    match resolve(requested, &installed) {
                        Some(found) if !installed.is_empty() => found,
                        _ => (minor.clone(), Some(requested.to_string())),
                    }
                }
                _ => (minor.clone(), asked.filter(|a| a != minor).map(|_| requested.to_string())),
            }
        }
    };
    let (venv, marker_path) = match target {
        Target::Shared(_) => {
            let venv = shared_venv(uv_dir, &minor);
            let marker = venv.join(MARKER);
            (venv, marker)
        }
        Target::Container(c) => (c.join(VENV_DIR), c.join(MARKER)),
    };
    r.log(&format!("Setting up the Python {minor} environment in {}", venv.display()));
    recover_swap(&venv, r);
    let build = with_suffix(&venv, ".building");
    r.remove(&build);
    let (make, install) = match backend {
        Backend::Uv(uv) => {
            let args = uv_venv_args(&minor, &build).ok_or_else(|| format!("\u{201c}{minor}\u{201d} is not a Python version."))?;
            (uv_step(uv, args, uv_dir, true), uv_step(uv, uv_pip_install_args(&venv_python(&build), &packages, false), uv_dir, true))
        }
        Backend::Python { argv, .. } => {
            let program = PathBuf::from(argv.first().cloned().unwrap_or_default());
            // The launcher is told the minor chosen; a lone Python is itself.
            let mut args: Vec<String> = if is_py_launcher(&argv[0]) { vec![format!("-{minor}")] } else { argv.iter().skip(1).cloned().collect() };
            args.extend(venv_args(&build));
            (Step { program, args, env: Vec::new() }, Step { program: venv_python(&build), args: pip_install_args(&packages, false), env: Vec::new() })
        }
    };
    for step in [make, install] {
        if let Err(e) = r.run(&step) {
            r.remove(&build);
            return Err(e);
        }
    }
    if r.exists(&venv) {
        let aside = with_suffix(&venv, ".old");
        r.remove(&aside);
        if let Err(e) = r.rename(&venv, &aside) {
            r.remove(&build);
            return Err(e);
        }
        if let Err(e) = r.rename(&build, &venv) {
            // Best effort to put the old one back (`UV:414-420`).
            let _ = r.rename(&aside, &venv);
            r.remove(&build);
            return Err(e);
        }
        r.remove(&aside);
    } else if let Err(e) = r.rename(&build, &venv) {
        r.remove(&build);
        return Err(e);
    }
    let (backend_name, uv_version) = match backend {
        Backend::Uv(uv) => ("uv", installed_uv_version(uv, uv_dir, r)),
        Backend::Python { .. } => ("venv", String::new()),
    };
    let marker = Marker { schema: 1, backend: backend_name.to_string(), uv_version, python: minor.clone(), remapped_from };
    r.write(&marker_path, &marker.json())?;
    match (target, name) {
        (Target::Shared(_), _) => {
            if let Some(from) = &marker.remapped_from {
                // Recorded only once the environment really exists (`UV:944-956`).
                r.write(&venvs_dir(uv_dir).join(REMAPS).join(two_part(from)), &minor)?;
            }
        }
        // The version built, not the one asked for (`UV:683-689`).
        (Target::Container(c), Some(name)) => r.write(&c.join(SETUP_CFG), &setup_cfg_after_build(None, name, deps, &minor))?,
        (Target::Container(_), None) => {}
    }
    Ok((venv_python(&venv), marker))
}

/// Tidies what an interrupted build or swap left: a `.old` with no environment beside it is
/// the working one moved aside and is put back; a `.old` beside an environment is removed.
/// (`.building` is removed by every build before it starts.)
fn recover_swap(venv: &Path, r: &mut dyn Runner) {
    let aside = with_suffix(venv, ".old");
    if !r.exists(&aside) {
        return;
    }
    if r.exists(venv) {
        r.remove(&aside);
    } else if let Err(e) = r.rename(&aside, venv) {
        r.log(&e);
    }
}

/// Check for Updated Runtime (`UV:1084-1131`, `UV:1141-1180`): a uv older than the pinned one
/// (or unreadable) is replaced and the remap records dropped (a newer uv may provide a minor
/// that was remapped); then, whatever happened, the packages of every shared environment with
/// a marker are upgraded.
fn check_for_update(arch: Arch, uv_dir: &Path, r: &mut dyn Runner) -> Result<String, String> {
    let uv = uv_exe(uv_dir);
    if !r.exists(&uv) {
        return Err(NOT_INSTALLED.to_string());
    }
    let installed = installed_uv_version(&uv, uv_dir, r);
    let older = installed == "unknown" || dotted(&installed, UV_VERSION) == Ordering::Less;
    let replaced = older.then(|| install_uv(arch, uv_dir, true, r));
    if matches!(replaced, Some(Ok(_))) {
        r.remove(&venvs_dir(uv_dir).join(REMAPS));
    }
    let packages: Vec<String> = PACKAGES.iter().map(|p| p.to_string()).collect();
    for venv in r.list_dirs(&venvs_dir(uv_dir)) {
        // Only `<minor>` folders: a `.old` left by an interrupted swap holds a marker too.
        let minor = venv.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if !numeric_version(&minor) || !r.exists(&venv.join(MARKER)) {
            continue;
        }
        if let Err(e) = r.run(&uv_step(&uv, uv_pip_install_args(&venv_python(&venv), &packages, true), uv_dir, true)) {
            r.log(&e);
        }
    }
    match replaced {
        None => Ok(up_to_date(&installed)),
        Some(Ok(_)) => Ok(upgraded(&installed, UV_VERSION)),
        Some(Err(e)) => Err(e),
    }
}

/// The minors a backend can build: uv's downloadable ones, the py launcher's installed ones,
/// or a lone Python's own.
fn minors(backend: &Backend, uv_dir: &Path, r: &mut dyn Runner) -> Vec<String> {
    match backend {
        Backend::Uv(uv) => r.run(&uv_step(uv, uv_python_list_args(), uv_dir, false)).map(|o| uv_minors(&o)).unwrap_or_default(),
        Backend::Python { argv, minor } if argv.len() > 1 && is_py_launcher(&argv[0]) => {
            let out = py_minors(&argv[0], r);
            if out.is_empty() { vec![minor.clone()] } else { out }
        }
        Backend::Python { minor, .. } => vec![minor.clone()],
    }
}

/// The minors the py launcher has installed (`py -0p`), sorted by number; none when it
/// cannot be asked.
fn py_minors(launcher: &str, r: &mut dyn Runner) -> Vec<String> {
    let step = Step { program: PathBuf::from(launcher), args: vec!["-0p".to_string()], env: Vec::new() };
    let mut out: Vec<String> = r.run(&step).map(|o| py_list(&o).into_iter().map(|(v, _)| v).collect()).unwrap_or_default();
    out.sort_by(|a, b| dotted(a, b));
    out.dedup();
    out
}

/// Whether a program is the py launcher (`py.exe`).
pub fn is_py_launcher(program: &str) -> bool {
    Path::new(program).file_name().is_some_and(|n| n.eq_ignore_ascii_case("py.exe"))
}

/// A Windows tool (curl.exe, tar.exe) in the System32 folder `system`: never a program of
/// the same name from PATH. An error naming it when System32 is unknown or lacks it.
pub fn system_tool_in(system: Option<&Path>, name: &str, is_file: &dyn Fn(&Path) -> bool) -> Result<PathBuf, String> {
    let system = system.ok_or_else(|| format!("{name} was not found: the System32 folder is unknown."))?;
    let tool = system.join(name);
    if is_file(&tool) { Ok(tool) } else { Err(format!("{name} was not found in {}.", system.display())) }
}

/// How long a probe of an installed Python (`py -0p`, `python --version`) may take.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// Runs provisioning for real: every program hidden (`CREATE_NO_WINDOW`), stdin empty, stdout
/// and stderr on one pipe, Tandem's environment cleaned by `child_env` plus the step's
/// variables. Each run is in its own Job Object that ends every process it started: when the
/// program ends (so nothing it left behind holds the output pipe), when it runs past
/// `timeout`, and when Tandem quits (the job closes with it). Each output line goes to `lines`.
#[cfg(windows)]
pub struct RealRunner {
    pub lines: std::sync::mpsc::Sender<String>,
    pub timeout: Duration,
}

/// A Job Object whose processes all end when it is closed (provisioning runs and launched
/// scripts).
#[cfg(windows)]
pub(crate) struct KillJob(windows_sys::Win32::Foundation::HANDLE);

#[cfg(windows)]
impl KillJob {
    pub(crate) fn new() -> Result<KillJob, String> {
        use windows_sys::Win32::System::JobObjects::{CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE};
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return Err(format!("CreateJobObjectW failed: {}", std::io::Error::last_os_error()));
            }
            let job = KillJob(job);
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let size = std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32;
            if SetInformationJobObject(job.0, JobObjectExtendedLimitInformation, (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(), size) == 0 {
                return Err(format!("SetInformationJobObject failed: {}", std::io::Error::last_os_error()));
            }
            Ok(job)
        }
    }

    /// Puts a process started suspended in the job, then lets it run, so nothing it starts can
    /// get out of the job first. A process none of whose threads could be resumed would sit
    /// suspended for ever: that is a failed start.
    pub(crate) fn adopt(&self, child: &std::process::Child) -> Result<(), String> {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::System::JobObjects::AssignProcessToJobObject;
        if unsafe { AssignProcessToJobObject(self.0, child.as_raw_handle()) } == 0 {
            return Err(format!("AssignProcessToJobObject failed: {}", std::io::Error::last_os_error()));
        }
        if resume_threads(child.id()) == 0 {
            return Err(format!("Could not resume the new process: {}", std::io::Error::last_os_error()));
        }
        Ok(())
    }

    /// Ends every process in the job with exit code 1.
    pub(crate) fn terminate(&self) {
        self.terminate_with(1);
    }

    /// Ends every process in the job with exit code `code`.
    pub(crate) fn terminate_with(&self, code: u32) {
        unsafe { windows_sys::Win32::System::JobObjects::TerminateJobObject(self.0, code) };
    }

    /// How many processes in the job are still running (0 when that cannot be read).
    pub(crate) fn active_processes(&self) -> u32 {
        use windows_sys::Win32::System::JobObjects::{JobObjectBasicAccountingInformation, QueryInformationJobObject, JOBOBJECT_BASIC_ACCOUNTING_INFORMATION};
        unsafe {
            let mut info: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = std::mem::zeroed();
            let size = std::mem::size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32;
            if QueryInformationJobObject(self.0, JobObjectBasicAccountingInformation, (&mut info as *mut JOBOBJECT_BASIC_ACCOUNTING_INFORMATION).cast(), size, std::ptr::null_mut()) == 0 {
                return 0;
            }
            info.ActiveProcesses
        }
    }
}

#[cfg(windows)]
impl Drop for KillJob {
    fn drop(&mut self) {
        unsafe { windows_sys::Win32::Foundation::CloseHandle(self.0) };
    }
}

/// Resumes the threads of a process started suspended (it has only its first one); how many
/// were resumed.
#[cfg(windows)]
fn resume_threads(pid: u32) -> u32 {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32};
    use windows_sys::Win32::System::Threading::{OpenThread, ResumeThread, THREAD_SUSPEND_RESUME};
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
        if snap == INVALID_HANDLE_VALUE {
            return 0;
        }
        let mut resumed = 0;
        let mut entry: THREADENTRY32 = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
        let mut more = Thread32First(snap, &mut entry) != 0;
        while more {
            if entry.th32OwnerProcessID == pid {
                let thread = OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID);
                if !thread.is_null() {
                    if ResumeThread(thread) != u32::MAX {
                        resumed += 1;
                    }
                    CloseHandle(thread);
                }
            }
            more = Thread32Next(snap, &mut entry) != 0;
        }
        CloseHandle(snap);
        resumed
    }
}

/// "an hour" for `BUILD_TIMEOUT`, else the seconds.
#[cfg(windows)]
fn duration_text(d: Duration) -> String {
    if d == BUILD_TIMEOUT { "an hour".to_string() } else { format!("{} seconds", d.as_secs()) }
}

#[cfg(windows)]
impl RealRunner {
    /// A runner with `BUILD_TIMEOUT`.
    pub fn new(lines: std::sync::mpsc::Sender<String>) -> RealRunner {
        RealRunner { lines, timeout: BUILD_TIMEOUT }
    }

    /// A Windows tool from System32 (curl.exe, tar.exe), found with `GetSystemDirectoryW`
    /// (not Tandem's environment), so a program of the same name on PATH is never run
    /// instead; an error naming it when System32 lacks it (`system_tool_in`).
    pub(crate) fn system_tool(name: &str) -> Result<PathBuf, String> {
        use std::os::windows::ffi::OsStringExt;
        let mut buf = [0u16; 520];
        let n = unsafe { windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW(buf.as_mut_ptr(), buf.len() as u32) } as usize;
        let system = (n > 0 && n < buf.len()).then(|| PathBuf::from(std::ffi::OsString::from_wide(&buf[..n])));
        system_tool_in(system.as_deref(), name, &|p: &Path| p.is_file())
    }

    /// How a failure names the run: the program's name and the arguments up to the first that
    /// is not an option ("uv venv", "python -m venv").
    fn label(step: &Step) -> String {
        let mut words = vec![step.program.file_stem().map_or_else(|| step.program.display().to_string(), |s| s.to_string_lossy().into_owned())];
        for a in &step.args {
            words.push(a.clone());
            if !a.starts_with('-') {
                break;
            }
        }
        words.join(" ")
    }
}

#[cfg(windows)]
impl Runner for RealRunner {
    fn run(&mut self, step: &Step) -> Result<String, String> {
        use std::io::Read;
        use std::os::windows::process::CommandExt;
        use std::process::{Command, Stdio};
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        const CREATE_SUSPENDED: u32 = 0x0000_0004;
        let label = Self::label(step);
        let job = KillJob::new().map_err(|e| format!("Could not run {label}: {e}"))?;
        let (mut reader, writer) = std::io::pipe().map_err(|e| format!("Could not run {label}: {e}"))?;
        let err_writer = writer.try_clone().map_err(|e| format!("Could not run {label}: {e}"))?;
        let inherited = std::env::vars_os().filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?)));
        let mut cmd = Command::new(&step.program);
        cmd.args(&step.args)
            .env_clear()
            .envs(crate::script_runtime::child_env(inherited, &step.env))
            .stdin(Stdio::null())
            .stdout(writer)
            .stderr(err_writer)
            .creation_flags(CREATE_NO_WINDOW | CREATE_SUSPENDED);
        let spawned = cmd.spawn();
        // The pipe's write ends go with the command, so the read ends when the program does.
        drop(cmd);
        let mut child = spawned.map_err(|e| format!("Could not run {label}: {e}"))?;
        if let Err(e) = job.adopt(&child) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("Could not run {label}: {e}"));
        }
        let lines = self.lines.clone();
        let (read_tx, read_rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let (mut all, mut line, mut buf) = (Vec::new(), Vec::new(), [0u8; 4096]);
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 {
                    break;
                }
                all.extend_from_slice(&buf[..n]);
                for &b in &buf[..n] {
                    if b == b'\n' {
                        let text = String::from_utf8_lossy(&line);
                        let _ = lines.send(text.trim_end_matches('\r').to_string());
                        line.clear();
                    } else {
                        line.push(b);
                    }
                }
            }
            if !line.is_empty() {
                let _ = lines.send(String::from_utf8_lossy(&line).trim_end_matches('\r').to_string());
            }
            let _ = read_tx.send(String::from_utf8_lossy(&all).into_owned());
        });
        let deadline = std::time::Instant::now() + self.timeout;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Ok(status),
                Ok(None) if std::time::Instant::now() >= deadline => {
                    job.terminate();
                    let _ = child.wait();
                    break Err(format!("{label} did not finish within {} and was stopped.", duration_text(self.timeout)));
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(50)),
                Err(e) => {
                    job.terminate();
                    break Err(format!("Could not wait for {label}: {e}"));
                }
            }
        };
        // The program ended: whatever it left running would hold the pipe, so it ends too;
        // the reader then sees the end of the output. It is not waited on for ever.
        job.terminate();
        let output = read_rx.recv_timeout(Duration::from_secs(5)).unwrap_or_default();
        let status = status?;
        if status.success() {
            return Ok(output);
        }
        // uv's own output holds the reason (a missing wheel, an unknown package), so the
        // error carries its end (`UV:559-571`).
        let code = status.code().map_or_else(|| "unknown".to_string(), |c| (c as u32).to_string());
        let tail: String = {
            let chars: Vec<char> = output.chars().collect();
            chars[chars.len().saturating_sub(2000)..].iter().collect()
        };
        let detail = if tail.is_empty() { String::new() } else { format!("\n{tail}") };
        Err(format!("{label} failed with status {code}.{detail}"))
    }

    fn download(&mut self, url: &str, to: &Path) -> Result<(), String> {
        if let Some(dir) = to.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("Could not create {}: {e}", dir.display()))?;
        }
        let args = ["--fail", "--location", "--silent", "--show-error", "--proto", "=https", "--output"].iter().map(|a| a.to_string()).chain([to.to_string_lossy().into_owned(), url.to_string()]).collect();
        self.run(&Step { program: Self::system_tool("curl.exe")?, args, env: Vec::new() }).map(|_| ())
    }

    fn sha256(&mut self, file: &Path) -> Result<String, String> {
        use sha2::{Digest, Sha256};
        use std::io::Read;
        let mut f = std::fs::File::open(file).map_err(|e| format!("Could not read {}: {e}", file.display()))?;
        let (mut hasher, mut buf) = (Sha256::new(), vec![0u8; 64 * 1024]);
        loop {
            let n = f.read(&mut buf).map_err(|e| format!("Could not read {}: {e}", file.display()))?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
        }
        Ok(hasher.finalize().iter().map(|b| format!("{b:02x}")).collect())
    }

    fn extract(&mut self, zip: &Path, to: &Path) -> Result<(), String> {
        std::fs::create_dir_all(to).map_err(|e| format!("Could not create {}: {e}", to.display()))?;
        // bsdtar refuses absolute paths and ".." in an archive unless -P is passed; it is not.
        let args = vec!["-xf".to_string(), zip.to_string_lossy().into_owned(), "-C".to_string(), to.to_string_lossy().into_owned()];
        self.run(&Step { program: Self::system_tool("tar.exe")?, args, env: Vec::new() }).map(|_| ())
    }

    fn exists(&self, p: &Path) -> bool {
        p.exists()
    }

    fn find_file(&self, dir: &Path, name: &str) -> Option<PathBuf> {
        let p = dir.join(name);
        p.is_file().then_some(p)
    }

    fn list_dirs(&self, dir: &Path) -> Vec<PathBuf> {
        let mut out: Vec<PathBuf> = std::fs::read_dir(dir).map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect()).unwrap_or_default();
        out.sort();
        out
    }

    fn rename(&mut self, from: &Path, to: &Path) -> Result<(), String> {
        if let Some(dir) = to.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        std::fs::rename(from, to).map_err(|e| format!("Could not move {} to {}: {e}", from.display(), to.display()))
    }

    fn remove(&mut self, p: &Path) {
        let _ = if p.is_dir() { std::fs::remove_dir_all(p) } else { std::fs::remove_file(p) };
    }

    fn write(&mut self, p: &Path, text: &str) -> Result<(), String> {
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        std::fs::write(p, text).map_err(|e| format!("Could not write {}: {e}", p.display()))
    }

    fn log(&mut self, line: &str) {
        let _ = self.lines.send(line.to_string());
    }
}

/// Runs jobs on a worker thread, one at a time in the order submitted (iTerm2's serial
/// `provisionQueue`). The app is woken when a job ends or a line of output arrives.
#[cfg(windows)]
pub struct Provisioner {
    jobs: std::sync::mpsc::Sender<(u64, Job)>,
    done: std::sync::mpsc::Receiver<(u64, Result<Outcome, String>)>,
    lines: std::sync::mpsc::Receiver<String>,
    next: std::cell::Cell<u64>,
    /// Jobs submitted and not yet ended.
    pending: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

#[cfg(windows)]
impl Provisioner {
    pub fn start(uv_dir: PathBuf, notify: crate::pty::Notify) -> Provisioner {
        use std::sync::atomic::Ordering::SeqCst;
        use std::sync::mpsc::channel;
        let (jobs, job_rx) = channel::<(u64, Job)>();
        let (done_tx, done) = channel();
        let (line_tx, line_rx) = channel::<String>();
        let (app_lines, lines) = channel();
        let pending = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        // Lines pass through here so each one wakes the app.
        let wake = notify.clone();
        std::thread::spawn(move || {
            for line in line_rx {
                if app_lines.send(line).is_err() {
                    break;
                }
                wake();
            }
        });
        let left = pending.clone();
        std::thread::spawn(move || {
            let mut runner = RealRunner::new(line_tx);
            for (id, job) in job_rx {
                let result = run_job(&job, &uv_dir, &mut runner);
                left.fetch_sub(1, SeqCst);
                if done_tx.send((id, result)).is_err() {
                    break;
                }
                notify();
            }
        });
        Provisioner { jobs, done, lines, next: std::cell::Cell::new(1), pending }
    }

    /// Queues a job; its result comes back from `try_done` with this id.
    pub fn submit(&self, job: Job) -> u64 {
        let id = self.next.get();
        self.next.set(id + 1);
        self.pending.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if self.jobs.send((id, job)).is_err() {
            self.pending.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
        }
        id
    }

    pub fn try_done(&self) -> Option<(u64, Result<Outcome, String>)> {
        self.done.try_recv().ok()
    }

    pub fn try_line(&self) -> Option<String> {
        self.lines.try_recv().ok()
    }

    /// Whether a job is running or waiting.
    pub fn busy(&self) -> bool {
        self.pending.load(std::sync::atomic::Ordering::SeqCst) > 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script_runtime::*;
    use std::collections::BTreeSet;
    use std::path::{Path, PathBuf};

    #[test]
    fn system_tools_never_come_from_path() {
        let system = Path::new(r"C:\Windows\System32");
        let there = |p: &Path| p == Path::new(r"C:\Windows\System32\tar.exe");
        assert_eq!(system_tool_in(Some(system), "tar.exe", &there), Ok(PathBuf::from(r"C:\Windows\System32\tar.exe")));
        // Missing from System32, or System32 unknown: an error naming the tool, never the bare
        // name (which would search PATH).
        let missing = system_tool_in(Some(system), "curl.exe", &there).unwrap_err();
        assert!(missing.contains("curl.exe") && missing.contains(r"C:\Windows\System32"), "{missing}");
        assert!(system_tool_in(None, "tar.exe", &there).unwrap_err().contains("tar.exe"));
    }

    /// One thing a `FakeRunner` was asked to do (queries such as `exists` are not recorded).
    #[derive(Clone, Debug, PartialEq, Eq)]
    enum Call {
        Run(Step),
        Download(String, PathBuf),
        Sha(PathBuf),
        Extract(PathBuf, PathBuf),
        Rename(PathBuf, PathBuf),
        Remove(PathBuf),
        Write(PathBuf, String),
    }

    /// Records every call, answers runs from a script (the first answer whose key the joined
    /// arguments contain; else empty output), and keeps a set of files: a path exists when it
    /// is in the set or a file is under it.
    struct FakeRunner {
        calls: Vec<Call>,
        logs: Vec<String>,
        answers: Vec<(&'static str, Result<String, String>)>,
        files: BTreeSet<PathBuf>,
        digest: String,
        /// What an extract puts under its folder.
        zip_holds: Vec<&'static str>,
    }

    impl FakeRunner {
        fn new(files: &[&str]) -> FakeRunner {
            FakeRunner { calls: Vec::new(), logs: Vec::new(), answers: Vec::new(), files: files.iter().map(PathBuf::from).collect(), digest: UV_SHA256_X64.to_string(), zip_holds: vec!["uv.exe", "uvx.exe"] }
        }

        fn answer(mut self, key: &'static str, result: Result<&str, &str>) -> FakeRunner {
            self.answers.push((key, result.map(str::to_string).map_err(str::to_string)));
            self
        }

        fn has(&self, p: &str) -> bool {
            self.exists(Path::new(p))
        }
    }

    impl Runner for FakeRunner {
        fn run(&mut self, step: &Step) -> Result<String, String> {
            self.calls.push(Call::Run(step.clone()));
            let joined = step.args.join(" ");
            self.answers.iter().find(|(k, _)| joined.contains(k)).map_or(Ok(String::new()), |(_, r)| r.clone())
        }
        fn download(&mut self, url: &str, to: &Path) -> Result<(), String> {
            self.calls.push(Call::Download(url.to_string(), to.to_path_buf()));
            self.files.insert(to.to_path_buf());
            Ok(())
        }
        fn sha256(&mut self, file: &Path) -> Result<String, String> {
            self.calls.push(Call::Sha(file.to_path_buf()));
            Ok(self.digest.clone())
        }
        fn extract(&mut self, zip: &Path, to: &Path) -> Result<(), String> {
            self.calls.push(Call::Extract(zip.to_path_buf(), to.to_path_buf()));
            for f in &self.zip_holds {
                self.files.insert(to.join(f));
            }
            Ok(())
        }
        fn exists(&self, p: &Path) -> bool {
            self.files.iter().any(|f| f.starts_with(p))
        }
        fn find_file(&self, dir: &Path, name: &str) -> Option<PathBuf> {
            let p = dir.join(name);
            self.files.contains(&p).then_some(p)
        }
        fn list_dirs(&self, dir: &Path) -> Vec<PathBuf> {
            let mut out: Vec<PathBuf> = self
                .files
                .iter()
                .filter_map(|f| f.strip_prefix(dir).ok().filter(|rest| rest.components().count() > 1).and_then(|rest| rest.components().next()).map(|c| dir.join(c)))
                .collect();
            out.dedup();
            out
        }
        fn rename(&mut self, from: &Path, to: &Path) -> Result<(), String> {
            self.calls.push(Call::Rename(from.to_path_buf(), to.to_path_buf()));
            let moved: Vec<PathBuf> = self.files.iter().filter(|f| f.starts_with(from)).cloned().collect();
            for f in moved {
                self.files.remove(&f);
                self.files.insert(to.join(f.strip_prefix(from).unwrap()));
            }
            Ok(())
        }
        fn remove(&mut self, p: &Path) {
            self.calls.push(Call::Remove(p.to_path_buf()));
            self.files.retain(|f| !f.starts_with(p));
        }
        fn write(&mut self, p: &Path, text: &str) -> Result<(), String> {
            self.calls.push(Call::Write(p.to_path_buf(), text.to_string()));
            self.files.insert(p.to_path_buf());
            Ok(())
        }
        fn log(&mut self, line: &str) {
            self.logs.push(line.to_string());
        }
    }

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    fn p(v: &str) -> PathBuf {
        PathBuf::from(v)
    }

    fn uv_step(args: Vec<String>, downloads: bool) -> Step {
        Step { program: p(r"C:\u\bin\uv.exe"), args, env: uv_env(Path::new(r"C:\u"), downloads) }
    }

    /// The runs, renames, removes and writes a uv provision of shared 3.12 makes.
    fn shared_312_calls() -> Vec<Call> {
        let build = p(r"C:\u\venvs\3.12.building");
        vec![
            Call::Run(uv_step(uv_python_list_args(), false)),
            Call::Remove(build.clone()),
            Call::Run(uv_step(uv_venv_args("3.12", &build).unwrap(), true)),
            Call::Run(uv_step(uv_pip_install_args(&p(r"C:\u\venvs\3.12.building\Scripts\python.exe"), &s(&["iterm2", "certifi"]), false), true)),
            Call::Rename(build, p(r"C:\u\venvs\3.12")),
            Call::Run(uv_step(s(&["--version"]), false)),
            Call::Write(p(r"C:\u\venvs\3.12\python-runtime.json"), Marker { schema: 1, backend: "uv".into(), uv_version: UV_VERSION.into(), python: "3.12".into(), remapped_from: None }.json()),
        ]
    }

    const LIST: &str = r#"[{"version":"3.12.5","implementation":"cpython","variant":"default"},{"version":"3.9.20"}]"#;

    #[test]
    fn newest_shared_python_follows_iterm2() {
        let u = Path::new(r"C:\u");
        let ready: BTreeSet<PathBuf> = ["3.9", "3.12", "3.10", "3.13.building", "3.14"]
            .iter()
            .flat_map(|m| {
                let venv = shared_venv(u, m);
                [venv_python(&venv), venv.join(MARKER)]
            })
            .filter(|p| !p.starts_with(shared_venv(u, "3.14")) || p.ends_with(MARKER))
            .collect();
        let names: Vec<String> = ["3.9", "3.12", "3.10", "3.13.building", "3.14", ".remaps", "3.12.old"].iter().map(|s| s.to_string()).collect();
        // 3.14 has a marker but no interpreter; 3.12 beats 3.10 by number, not by text.
        assert_eq!(newest_shared_python(u, &names, &|p| ready.contains(p)), Some(venv_python(&shared_venv(u, "3.12"))));
        assert_eq!(newest_shared_python(u, &[], &|_| true), None);
    }

    #[test]
    fn install_uv_follows_iterm2() {
        let version = format!("uv {UV_VERSION} (abc 2026-09-25)");
        let mut r = FakeRunner::new(&[]).answer("python list", Ok(LIST)).answer("--version", Ok(version.as_str()));
        // The uv folder does not exist yet; the venv build makes the interpreter.
        let out = run_job(&Job::InstallUv { arch: Arch::X64 }, Path::new(r"C:\u"), &mut r);
        assert_eq!(out, Ok(Outcome::Installed(p(r"C:\u\bin\uv.exe"))));
        let zip = p(r"C:\u\bin\.uv-download.zip");
        let staging = p(r"C:\u\bin\.uv-staging");
        let mut want = vec![
            Call::Download(uv_zip_url(Arch::X64), zip.clone()),
            Call::Sha(zip.clone()),
            Call::Remove(staging.clone()),
            Call::Extract(zip.clone(), staging.clone()),
            Call::Rename(staging.join("uv.exe"), p(r"C:\u\bin\uv.exe")),
            Call::Remove(zip.clone()),
            Call::Remove(staging.clone()),
        ];
        want.extend(shared_312_calls());
        assert_eq!(r.calls, want);
        assert!(r.has(r"C:\u\bin\uv.exe") && !r.has(r"C:\u\bin\.uv-download.zip") && !r.has(r"C:\u\bin\.uv-staging"));
        assert!(r.logs.iter().any(|l| l.contains(&uv_zip_url(Arch::X64))), "{:?}", r.logs);

        // uv.exe one folder down in the zip is found there (`locate_uv`); a folder named
        // uv.exe does not count.
        let mut r = FakeRunner::new(&[]).answer("python list", Ok(LIST));
        r.zip_holds = vec![r"uv-x86_64-pc-windows-msvc\uv.exe"];
        assert_eq!(run_job(&Job::InstallUv { arch: Arch::X64 }, Path::new(r"C:\u"), &mut r), Ok(Outcome::Installed(p(r"C:\u\bin\uv.exe"))));
        assert!(r.calls.contains(&Call::Rename(staging.join(r"uv-x86_64-pc-windows-msvc\uv.exe"), p(r"C:\u\bin\uv.exe"))));
        let tree = FakeRunner::new(&[r"C:\x\uv.exe\inner.txt", r"C:\x\a\b\uv.exe"]);
        assert_eq!(locate_uv(Path::new(r"C:\x"), &tree), None);
        let tree = FakeRunner::new(&[r"C:\x\a\uv.exe"]);
        assert_eq!(locate_uv(Path::new(r"C:\x"), &tree), Some(p(r"C:\x\a\uv.exe")));

        // A zip with no uv.exe fails and leaves nothing behind.
        let mut r = FakeRunner::new(&[]);
        r.zip_holds = vec!["README.md"];
        assert!(run_job(&Job::InstallUv { arch: Arch::X64 }, Path::new(r"C:\u"), &mut r).is_err());
        assert!(!r.has(r"C:\u\bin"));

        // A digest mismatch: nothing is unpacked, no uv.exe, the zip removed.
        let mut r = FakeRunner::new(&[]);
        r.digest = UV_SHA256_ARM64.to_string();
        assert_eq!(run_job(&Job::InstallUv { arch: Arch::X64 }, Path::new(r"C:\u"), &mut r), Err(CHECKSUM_MISMATCH.to_string()));
        assert_eq!(r.calls, vec![Call::Download(uv_zip_url(Arch::X64), zip.clone()), Call::Sha(zip.clone()), Call::Remove(zip.clone())]);
        assert!(!r.has(r"C:\u\bin\uv.exe") && !r.has(r"C:\u\bin\.uv-download.zip"));
        assert_eq!(CHECKSUM_MISMATCH, "The downloaded uv did not match its checksum.");
        // The digest check ignores case (sha256sum files are lowercase; a runner may not be).
        let mut r = FakeRunner::new(&[]).answer("python list", Ok(LIST));
        r.digest = UV_SHA256_X64.to_ascii_uppercase();
        assert!(run_job(&Job::InstallUv { arch: Arch::X64 }, Path::new(r"C:\u"), &mut r).is_ok());
    }

    #[test]
    fn provision_follows_iterm2() {
        let uv = Backend::Uv(p(r"C:\u\bin\uv.exe"));
        let version = format!("uv {UV_VERSION} (abc 2026-09-25)");
        let mut r = FakeRunner::new(&[]).answer("python list", Ok(LIST)).answer("--version", Ok(version.as_str()));
        let job = Job::Provision { backend: uv.clone(), target: Target::Shared("3.12".into()), requested: "3.12".into(), dependencies: vec![], name: None };
        let marker = Marker { schema: 1, backend: "uv".into(), uv_version: UV_VERSION.into(), python: "3.12".into(), remapped_from: None };
        assert_eq!(run_job(&job, Path::new(r"C:\u"), &mut r), Ok(Outcome::Provisioned { python: p(r"C:\u\venvs\3.12\Scripts\python.exe"), marker }));
        assert_eq!(r.calls, shared_312_calls());

        // An installed Python builds a script's own environment with -m venv and -m pip.
        let py = Backend::Python { argv: s(&[r"C:\Windows\py.exe", "-3.12"]), minor: "3.12".into() };
        let mut r = FakeRunner::new(&[]);
        let deps = s(&["requests"]);
        let job = Job::Provision { backend: py.clone(), target: Target::Container(p(r"C:\S\tool")), requested: "3.12".into(), dependencies: deps.clone(), name: Some("tool".into()) };
        let marker = Marker { schema: 1, backend: "venv".into(), uv_version: String::new(), python: "3.12".into(), remapped_from: None };
        assert_eq!(run_job(&job, Path::new(r"C:\u"), &mut r), Ok(Outcome::Provisioned { python: p(r"C:\S\tool\.venv\Scripts\python.exe"), marker: marker.clone() }));
        let build = p(r"C:\S\tool\.venv.building");
        assert_eq!(
            r.calls,
            vec![
                Call::Remove(build.clone()),
                Call::Run(Step { program: p(r"C:\Windows\py.exe"), args: s(&["-3.12", "-m", "venv", r"C:\S\tool\.venv.building"]), env: vec![] }),
                Call::Run(Step {
                    program: p(r"C:\S\tool\.venv.building\Scripts\python.exe"),
                    args: s(&["-m", "pip", "install", "--only-binary", ":all:", "requests", "iterm2", "certifi"]),
                    env: vec![]
                }),
                Call::Rename(build, p(r"C:\S\tool\.venv")),
                Call::Write(p(r"C:\S\tool\python-runtime.json"), marker.json()),
                Call::Write(p(r"C:\S\tool\setup.cfg"), crate::script_runtime::setup_cfg_text("tool", &deps, true, "3.12", 72)),
            ]
        );
        // With the py launcher a different pick is built with `py -X.Y` when installed, and
        // setup.cfg records the version built; one not installed is resolved (3.11 to 3.12).
        let listing = "-V:3.12 *  C:\\Py312\\python.exe\n -V:3.10  C:\\Py310\\python.exe\n";
        for (asked, built, remapped) in [("3.10", "3.10", None), ("3.11", "3.12", Some("3.11"))] {
            let mut r = FakeRunner::new(&[]).answer("-0p", Ok(listing));
            let job = Job::Provision { backend: py.clone(), target: Target::Container(p(r"C:\S\tool")), requested: asked.into(), dependencies: deps.clone(), name: Some("tool".into()) };
            let Ok(Outcome::Provisioned { marker, .. }) = run_job(&job, Path::new(r"C:\u"), &mut r) else { panic!("{:?}", r.calls) };
            assert_eq!((marker.python.as_str(), marker.remapped_from.as_deref()), (built, remapped), "{asked}");
            let venv = Step { program: p(r"C:\Windows\py.exe"), args: s(&[format!("-{built}").as_str(), "-m", "venv", r"C:\S\tool\.venv.building"]), env: vec![] };
            assert!(r.calls.contains(&Call::Run(venv)), "{asked}: {:?}", r.calls);
            assert!(r.calls.contains(&Call::Write(p(r"C:\S\tool\setup.cfg"), crate::script_runtime::setup_cfg_text("tool", &deps, true, built, 72))), "{asked}");
        }
        // The same pick as the backend's asks the launcher nothing.
        let mut r = FakeRunner::new(&[]);
        let job = Job::Provision { backend: py.clone(), target: Target::Container(p(r"C:\S\tool")), requested: "3.12".into(), dependencies: vec![], name: None };
        assert!(run_job(&job, Path::new(r"C:\u"), &mut r).is_ok());
        assert!(!r.calls.iter().any(|c| matches!(c, Call::Run(st) if st.args == s(&["-0p"]))));
        // With no name no setup.cfg is written.
        let mut r = FakeRunner::new(&[]);
        let job = Job::Provision { backend: py.clone(), target: Target::Container(p(r"C:\S\tool")), requested: "3.12".into(), dependencies: deps.clone(), name: None };
        assert!(run_job(&job, Path::new(r"C:\u"), &mut r).is_ok());
        assert!(!r.calls.iter().any(|c| matches!(c, Call::Write(f, _) if f.ends_with("setup.cfg"))));

        // A pin uv cannot provide is remapped (3.7 to 3.9); the marker and a remap record say so.
        let mut r = FakeRunner::new(&[]).answer("python list", Ok(LIST)).answer("--version", Ok(version.as_str()));
        let job = Job::Provision { backend: uv.clone(), target: Target::Shared("3.7".into()), requested: "3.7".into(), dependencies: vec![], name: None };
        let Ok(Outcome::Provisioned { python, marker }) = run_job(&job, Path::new(r"C:\u"), &mut r) else { panic!("{:?}", r.calls) };
        assert_eq!(python, p(r"C:\u\venvs\3.9\Scripts\python.exe"));
        assert_eq!((marker.python.as_str(), marker.remapped_from.as_deref()), ("3.9", Some("3.7")));
        assert!(r.calls.contains(&Call::Run(uv_step(uv_venv_args("3.9", &p(r"C:\u\venvs\3.9.building")).unwrap(), true))));
        assert!(r.calls.contains(&Call::Write(p(r"C:\u\venvs\.remaps\3.7"), "3.9".into())));
        let mut container = FakeRunner::new(&[]).answer("python list", Ok(LIST));
        let job = Job::Provision { backend: uv.clone(), target: Target::Container(p(r"C:\S\f")), requested: "3.7".into(), dependencies: vec![], name: None };
        let Ok(Outcome::Provisioned { marker, .. }) = run_job(&job, Path::new(r"C:\u"), &mut container) else { panic!() };
        assert_eq!((marker.python.as_str(), marker.remapped_from.as_deref()), ("3.9", Some("3.7")));
        assert!(container.calls.contains(&Call::Write(p(r"C:\S\f\python-runtime.json"), marker.json())));

        // A version that is not digits and dots never reaches uv.
        let mut r = FakeRunner::new(&[]).answer("python list", Ok(LIST));
        let job = Job::Provision { backend: uv.clone(), target: Target::Shared("pypy".into()), requested: "pypy".into(), dependencies: vec![], name: None };
        assert!(run_job(&job, Path::new(r"C:\u"), &mut r).is_err());
        assert!(!r.calls.iter().any(|c| matches!(c, Call::Run(st) if st.args.first().is_some_and(|a| a == "venv"))));

        // A dependency that could download from elsewhere is left out and logged.
        let mut r = FakeRunner::new(&[]);
        let hostile = s(&["--index-url=https://h/simple", "requests"]);
        let job = Job::Provision { backend: py.clone(), target: Target::Container(p(r"C:\S\tool")), requested: "3.12".into(), dependencies: hostile, name: None };
        assert!(run_job(&job, Path::new(r"C:\u"), &mut r).is_ok());
        assert!(r.calls.iter().all(|c| !matches!(c, Call::Run(st) if st.args.iter().any(|a| a.contains("index-url")))));
        assert!(r.logs.iter().any(|l| l.contains("--index-url=https://h/simple")), "{:?}", r.logs);

        // Ready environments: interpreter and marker, and a recorded remap for a shared pin.
        let files = [r"C:\u\venvs\3.9\Scripts\python.exe", r"C:\u\venvs\3.9\python-runtime.json", r"C:\S\t\.venv\Scripts\python.exe", r"C:\S\t\python-runtime.json", r"C:\S\half\.venv\Scripts\python.exe"];
        let exists = |q: &Path| files.iter().any(|f| Path::new(f) == q);
        let read = |q: &Path| (q == Path::new(r"C:\u\venvs\.remaps\3.7")).then(|| "3.9\n".to_string());
        let uv_dir = Path::new(r"C:\u");
        assert_eq!(ready_python(&Target::Shared("3.9".into()), uv_dir, &exists, &read), Some(p(r"C:\u\venvs\3.9\Scripts\python.exe")));
        assert_eq!(ready_python(&Target::Shared("3.7".into()), uv_dir, &exists, &read), Some(p(r"C:\u\venvs\3.9\Scripts\python.exe")));
        assert_eq!(ready_python(&Target::Shared("3.12".into()), uv_dir, &exists, &read), None);
        assert_eq!(ready_python(&Target::Container(p(r"C:\S\t")), uv_dir, &exists, &read), Some(p(r"C:\S\t\.venv\Scripts\python.exe")));
        assert_eq!(ready_python(&Target::Container(p(r"C:\S\half")), uv_dir, &exists, &read), None);
    }

    #[test]
    fn failed_rebuild_keeps_the_old_venv() {
        let py = Backend::Python { argv: s(&[r"C:\Windows\py.exe", "-3.12"]), minor: "3.12".into() };
        let job = Job::Provision { backend: py, target: Target::Container(p(r"C:\S\tool")), requested: "3.12".into(), dependencies: vec![], name: None };
        let old = [r"C:\S\tool\.venv\Scripts\python.exe", r"C:\S\tool\python-runtime.json"];
        let mut r = FakeRunner::new(&old).answer("pip install", Err("no wheel"));
        assert_eq!(run_job(&job, Path::new(r"C:\u"), &mut r), Err("no wheel".to_string()));
        assert!(!r.calls.iter().any(|c| matches!(c, Call::Rename(..) | Call::Write(..))), "{:?}", r.calls);
        assert!(!r.calls.contains(&Call::Remove(p(r"C:\S\tool\.venv"))));
        assert_eq!(r.calls.last(), Some(&Call::Remove(p(r"C:\S\tool\.venv.building"))));
        assert!(r.has(r"C:\S\tool\.venv\Scripts\python.exe"));
        // The venv step failing is the same.
        let mut r = FakeRunner::new(&old).answer("-m venv", Err("no python"));
        assert_eq!(run_job(&job, Path::new(r"C:\u"), &mut r), Err("no python".to_string()));
        assert!(r.has(r"C:\S\tool\.venv\Scripts\python.exe"));

        // Success swaps through .venv.old: the old aside, the new in, the old removed.
        let mut r = FakeRunner::new(&old);
        assert!(run_job(&job, Path::new(r"C:\u"), &mut r).is_ok());
        let (venv, build, aside) = (p(r"C:\S\tool\.venv"), p(r"C:\S\tool\.venv.building"), p(r"C:\S\tool\.venv.old"));
        let swaps: Vec<&Call> = r.calls.iter().filter(|c| matches!(c, Call::Rename(..)) || matches!(c, Call::Remove(q) if *q == aside)).collect();
        assert_eq!(swaps, vec![&Call::Remove(aside.clone()), &Call::Rename(venv.clone(), aside.clone()), &Call::Rename(build, venv), &Call::Remove(aside)]);
    }

    #[test]
    fn update_check_follows_iterm2() {
        let uv_dir = Path::new(r"C:\u");
        let installed = [
            r"C:\u\bin\uv.exe",
            r"C:\u\venvs\3.12\Scripts\python.exe",
            r"C:\u\venvs\3.12\python-runtime.json",
            r"C:\u\venvs\3.9\Scripts\python.exe",
            r"C:\u\venvs\3.9\python-runtime.json",
            r"C:\u\venvs\3.10.building\Scripts\python.exe",
            r"C:\u\venvs\.remaps\3.7",
        ];
        let upgrade = |minor: &str| {
            let py = p(&format!(r"C:\u\venvs\{minor}\Scripts\python.exe"));
            Call::Run(uv_step(uv_pip_install_args(&py, &s(&["iterm2", "certifi"]), true), true))
        };
        let zip = p(r"C:\u\bin\.uv-download.zip");
        let staging = p(r"C:\u\bin\.uv-staging");

        // An older uv: the pinned one is installed (no shared provisioning), the remap records
        // are dropped, then every shared environment with a marker is upgraded.
        let mut r = FakeRunner::new(&installed).answer("--version", Ok("uv 0.1.0 (x)"));
        assert_eq!(run_job(&Job::CheckForUpdate { arch: Arch::X64 }, uv_dir, &mut r), Ok(Outcome::Updated(upgraded("0.1.0", UV_VERSION))));
        assert_eq!(
            r.calls,
            vec![
                Call::Run(uv_step(s(&["--version"]), false)),
                Call::Download(uv_zip_url(Arch::X64), zip.clone()),
                Call::Sha(zip.clone()),
                Call::Remove(staging.clone()),
                Call::Extract(zip.clone(), staging.clone()),
                Call::Rename(staging.join("uv.exe"), p(r"C:\u\bin\uv.exe")),
                Call::Remove(zip.clone()),
                Call::Remove(staging.clone()),
                Call::Remove(p(r"C:\u\venvs\.remaps")),
                upgrade("3.12"),
                upgrade("3.9"),
            ]
        );

        // The pinned uv: only the packages are upgraded.
        let current = format!("uv {UV_VERSION} (x)");
        let mut r = FakeRunner::new(&installed).answer("--version", Ok(current.as_str()));
        assert_eq!(run_job(&Job::CheckForUpdate { arch: Arch::X64 }, uv_dir, &mut r), Ok(Outcome::Updated(up_to_date(UV_VERSION))));
        assert_eq!(r.calls, vec![Call::Run(uv_step(s(&["--version"]), false)), upgrade("3.12"), upgrade("3.9")]);

        // A failed download still upgrades the packages, then reports the failure.
        let mut r = FakeRunner::new(&installed).answer("--version", Ok("uv 0.1.0 (x)"));
        r.digest = "0".repeat(64);
        assert_eq!(run_job(&Job::CheckForUpdate { arch: Arch::X64 }, uv_dir, &mut r), Err(CHECKSUM_MISMATCH.to_string()));
        assert_eq!(r.calls.last(), Some(&upgrade("3.9")));
        assert!(r.has(r"C:\u\venvs\.remaps\3.7"));

        // No uv: nothing runs.
        let mut r = FakeRunner::new(&[]);
        assert_eq!(run_job(&Job::CheckForUpdate { arch: Arch::X64 }, uv_dir, &mut r), Err(NOT_INSTALLED.to_string()));
        assert!(r.calls.is_empty());

        // The minors a backend offers.
        let mut r = FakeRunner::new(&[]).answer("python list", Ok(LIST));
        assert_eq!(run_job(&Job::Minors { backend: Backend::Uv(p(r"C:\u\bin\uv.exe")) }, uv_dir, &mut r), Ok(Outcome::Minors(s(&["3.9", "3.12"]))));
        let mut r = FakeRunner::new(&[]).answer("-0p", Ok(" -V:3.12 *  C:\\Py312\\python.exe\n -V:3.10   C:\\Py310\\python.exe\n"));
        let py = Backend::Python { argv: s(&[r"C:\Windows\py.exe", "-3.12"]), minor: "3.12".into() };
        assert_eq!(run_job(&Job::Minors { backend: py }, uv_dir, &mut r), Ok(Outcome::Minors(s(&["3.10", "3.12"]))));
        let plain = Backend::Python { argv: s(&[r"C:\Py311\python.exe"]), minor: "3.11".into() };
        assert_eq!(run_job(&Job::Minors { backend: plain }, uv_dir, &mut FakeRunner::new(&[])), Ok(Outcome::Minors(s(&["3.11"]))));
    }

    #[test]
    fn gate_follows_the_rule() {
        assert_eq!(gate(true, false), Gate::Go);
        assert_eq!(gate(true, true), Gate::Ask);
        assert_eq!(gate(false, false), Gate::Go);
        assert_eq!(gate(false, true), Gate::Refuse);
        assert_eq!(BUILD_TIMEOUT, std::time::Duration::from_secs(3600));
    }

    #[cfg(windows)]
    #[test]
    fn runner_runs_hidden() {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut r = RealRunner::new(tx);
        let echo = Step { program: p("cmd.exe"), args: s(&["/c", "echo tandem"]), env: vec![] };
        assert_eq!(r.run(&echo), Ok("tandem\r\n".to_string()));
        assert_eq!(rx.try_iter().collect::<Vec<_>>(), s(&["tandem"]));
        let fail = Step { program: p("cmd.exe"), args: s(&["/c", "exit 3"]), env: vec![] };
        let err = r.run(&fail).unwrap_err();
        assert!(err.contains("status 3"), "{err}");
        // The step's variables reach the program.
        let env = Step { program: p("cmd.exe"), args: s(&["/c", "echo %TANDEM_T%"]), env: vec![("TANDEM_T".into(), "yes".into())] };
        assert_eq!(r.run(&env), Ok("yes\r\n".to_string()));
        let tmp = std::env::temp_dir().join(format!("tandem-sha-{}.txt", std::process::id()));
        std::fs::write(&tmp, "abc").unwrap();
        let sum = r.sha256(&tmp);
        let _ = std::fs::remove_file(&tmp);
        assert_eq!(sum, Ok("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad".to_string()));
    }

    #[cfg(windows)]
    #[test]
    fn runner_ends_the_process_tree() {
        let started = std::time::Instant::now();
        let mut r = RealRunner { lines: std::sync::mpsc::channel().0, timeout: Duration::from_secs(2) };
        // A grandchild that outlives its parent and holds the output pipe: the run still ends
        // when cmd.exe does, and the grandchild with it.
        let orphan = Step { program: p("cmd.exe"), args: s(&["/c", "start /b ping -n 30 127.0.0.1 & echo left"]), env: vec![] };
        let out = r.run(&orphan);
        assert!(out.as_deref().is_ok_and(|o| o.contains("left")), "{out:?}");
        assert!(started.elapsed() < Duration::from_secs(10), "{:?}", started.elapsed());
        // A program that runs past the timeout is stopped with everything it started.
        let started = std::time::Instant::now();
        let hung = Step { program: p("cmd.exe"), args: s(&["/c", "ping -n 30 127.0.0.1"]), env: vec![] };
        let err = r.run(&hung).unwrap_err();
        assert!(err.contains("did not finish within 2 seconds and was stopped."), "{err}");
        assert!(started.elapsed() < Duration::from_secs(10), "{:?}", started.elapsed());
        assert_eq!(RealRunner::new(std::sync::mpsc::channel().0).timeout, BUILD_TIMEOUT);
    }

    #[test]
    fn interrupted_swap_is_recovered() {
        let py = Backend::Python { argv: s(&[r"C:\Windows\py.exe", "-3.12"]), minor: "3.12".into() };
        let job = Job::Provision { backend: py, target: Target::Container(p(r"C:\S\tool")), requested: "3.12".into(), dependencies: vec![], name: None };
        let (venv, aside) = (p(r"C:\S\tool\.venv"), p(r"C:\S\tool\.venv.old"));
        // The working environment was moved aside and the new one never moved in: it is put
        // back before the build, so a failed build leaves it in place.
        let mut r = FakeRunner::new(&[r"C:\S\tool\.venv.old\Scripts\python.exe", r"C:\S\tool\python-runtime.json"]).answer("pip install", Err("no wheel"));
        assert!(run_job(&job, Path::new(r"C:\u"), &mut r).is_err());
        assert_eq!(r.calls[0], Call::Rename(aside.clone(), venv.clone()));
        assert!(r.has(r"C:\S\tool\.venv\Scripts\python.exe") && !r.has(r"C:\S\tool\.venv.old"));
        // A .old beside a working environment is removed.
        let mut r = FakeRunner::new(&[r"C:\S\tool\.venv\Scripts\python.exe", r"C:\S\tool\.venv.old\Scripts\python.exe"]).answer("pip install", Err("no wheel"));
        assert!(run_job(&job, Path::new(r"C:\u"), &mut r).is_err());
        assert_eq!(r.calls[0], Call::Remove(aside));
        assert!(r.has(r"C:\S\tool\.venv\Scripts\python.exe") && !r.has(r"C:\S\tool\.venv.old"));

        // The update check upgrades only `<minor>` folders, not a shared `.old` with a marker.
        let current = format!("uv {UV_VERSION} (x)");
        let files = [r"C:\u\bin\uv.exe", r"C:\u\venvs\3.12\python-runtime.json", r"C:\u\venvs\3.12.old\python-runtime.json"];
        let mut r = FakeRunner::new(&files).answer("--version", Ok(current.as_str()));
        assert!(run_job(&Job::CheckForUpdate { arch: Arch::X64 }, Path::new(r"C:\u"), &mut r).is_ok());
        let upgrades: Vec<&Call> = r.calls.iter().filter(|c| matches!(c, Call::Run(st) if st.args.contains(&"install".to_string()))).collect();
        assert_eq!(upgrades.len(), 1, "{:?}", r.calls);
        assert!(matches!(upgrades[0], Call::Run(st) if st.args.iter().any(|a| a.contains(r"venvs\3.12\Scripts"))), "{upgrades:?}");
    }
}
