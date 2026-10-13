//! Scripts > Manage > Manage Dependencies (iTerm2's `iTermDependencyEditorWindowController`,
//! `DE`): the scripts it lists, the pip commands it runs and asks about, the re-pin after an
//! upgrade, the Python version question, the basic-to-full upgrade's folder, and its texts.

use std::cmp::Ordering;
use std::path::{Path, PathBuf};

use crate::script_runtime::{self as rt, Backend};
use crate::scripts::Node;

/// The window's title and labels (`API/Base.lproj/iTermDependencyEditorWindowController.xib`).
pub const TITLE: &str = "Script Dependency Editor";
pub const SCRIPT_LABEL: &str = "Script:";
pub const PYTHON_LABEL: &str = "Python Version:";
pub const PACKAGE_COLUMN: &str = "Package Name";
pub const VERSION_COLUMN: &str = "Installed Version";
pub const ADD: &str = "Add";
pub const REMOVE: &str = "Remove";
pub const CHECK_FOR_UPDATE: &str = "Check for Update";
pub const UPGRADE: &str = "Upgrade";
/// A package whose `pip show` failed (`DE:155`).
pub const ERROR_VERSION: &str = "[Error]";

/// "Add Dependency" (`DE:341-367`).
pub const ADD_HEADING: &str = "Add Dependency";
pub const ADD_TEXT: &str = "What dependency would you like to add?";
/// The pip command question (`DE:369-411`); persistent in iTerm2, so never silenced.
pub const RUN_HEADING: &str = "Run this Command?";
pub const RUN_ID: &str = "DependencyEditorPip3Confirmation";
/// What the question adds for a command that downloads (Termurion's: every download names its
/// source): pypi.org, or the index the command's environment names (`configured_index`).
pub fn source_note(index: Option<&str>) -> String {
    format!("\n\nPackages come from {}.", index.unwrap_or("pypi.org"))
}
/// The source named when a pip configuration file may set the index (its text is not read).
pub const PIP_CONFIG_INDEX: &str = "the index your pip configuration names";
/// After a failed install (`DE:440-452`), headed "Installation Failed".
pub const INSTALL_CHECK_PIP: &str = "Check the pip output for errors.";
pub const INSTALLATION_FAILED: &str = rt::INSTALL_FAILED_HEADING;
pub fn uninstall_failed(package: &str) -> String {
    format!("Uninstall of {package} failed. Check the pip output for errors.")
}
pub const REMOVAL_FAILED: &str = "Removal Failed";
/// A basic script's pane (the xib's text, iTerm2's mismatched closing quote kept).
pub const UPGRADE_TEXT_BASIC: &str = "This script must be upgraded to a \u{201c}full environment\u{201c} to add dependencies. Doing so makes a copy of the Python runtime but will not use any additional disk space.";
/// The Python version questions (`DE:653-744`), persistent in iTerm2.
pub const DOWNGRADE_HEADING: &str = "Confirm Python Downgrade";
pub const DOWNGRADE_TEXT: &str = "You have asked to downgrade to an older Python version. Dependencies will need to be reinstalled. This may go badly. Are you sure you want to do this?";
pub const UPGRADE_HEADING: &str = "Confirm Python Upgrade";
pub const UPGRADE_TEXT: &str = "You have asked to upgrade to a newer Python version. Dependencies will need to be reinstalled. OK to continue?";
pub use crate::script_launch::REBUILDING;
pub const COULD_NOT_CHANGE: &str = "Could Not Change Python Version";
/// Upgrade (`DE:474-579`).
pub const ERROR: &str = "Error";
pub fn folder_exists(path: &Path) -> String {
    format!("Can\u{2019}t upgrade because {} already exists", path.display())
}
pub fn upgrade_error(e: &str) -> String {
    format!("Please file a bug report at https://github.com/datbird/termurion/issues. The following error occurred while upgrading a dependency: {e}")
}
pub fn error_creating(folder: &Path, e: &str) -> String {
    format!("Error creating {}: {e}", folder.display())
}
pub fn error_moving(from: &Path, to: &Path, e: &str) -> String {
    format!("Error moving {} to {}: {e}", from.display(), to.display())
}

/// The Script popup's items (`DE:258-282`): every script, folders flattened into
/// "folder/name" breadcrumbs, each level sorted by name with `compare:` (case-sensitive, so
/// uppercase first); each with its path and whether it is a full environment.
pub fn script_choices(nodes: &[Node]) -> Vec<(String, PathBuf, bool)> {
    fn add(nodes: &[Node], crumbs: &str, out: &mut Vec<(String, PathBuf, bool)>) {
        let mut sorted: Vec<&Node> = nodes.iter().collect();
        sorted.sort_by(|a, b| a.name().cmp(b.name()));
        for n in sorted {
            match n {
                Node::Folder { name, children, .. } => add(children, &format!("{crumbs}{name}/"), out),
                Node::Script { name, path, full_environment } => out.push((format!("{crumbs}{name}"), path.clone(), *full_environment)),
            }
        }
    }
    let mut out = Vec::new();
    add(nodes, "", &mut out);
    out
}

/// The version `pip show` printed: its first "Version: " line (`DE:171-179`).
pub fn version_in_pip_show(output: &str) -> Option<String> {
    output.split('\n').find_map(|l| l.strip_prefix("Version: ")).map(|v| v.trim_end_matches('\r').to_string())
}

/// The program, arguments and added variables of a pip command against the environment
/// `venv` (`DE:369-392`): through Termurion's uv with the provisioning variables, or with the
/// environment's own Python.
pub fn pip_command(backend: &Backend, venv: &Path, pip_args: &[String]) -> (PathBuf, Vec<String>, Vec<(String, String)>) {
    let python = rt::venv_python(venv);
    match backend {
        Backend::Uv(uv) => {
            // `<uv dir>\bin\uv.exe`.
            let uv_dir = uv.parent().and_then(Path::parent).unwrap_or(Path::new(""));
            (uv.clone(), rt::uv_pip_passthrough(pip_args, &python), rt::uv_env(uv_dir, true))
        }
        Backend::Python { .. } => (python, rt::pip_passthrough(pip_args), Vec::new()),
    }
}

/// The command as the window runs it and the question shows it: a Windows command line.
pub fn command_text(program: &Path, args: &[String]) -> String {
    crate::git_wrapper::program_line(program, args)
}

/// "Run this Command?"'s text: the command, and for an install where the packages come from
/// (`index`, from `configured_index`; None is pypi.org).
pub fn confirm_text(program: &Path, args: &[String], index: Option<&str>) -> String {
    let command = command_text(program, args);
    let subcommand = args.iter().position(|a| a == "pip").and_then(|i| args.get(i + 1));
    if subcommand.map(String::as_str) == Some("install") { format!("{command}{}", source_note(index)) } else { command }
}

/// Where an install through `backend` gets its packages when not from pypi.org alone: the
/// variables of the window it runs in (`env`) name another index, or for an installed
/// Python's pip a configuration file may (`exists` looks for one; a network path counts as
/// one without being probed). The index replacing pypi.org comes first, then any searched
/// besides it, joined with " or "; None for pypi.org alone. uv reads no configuration file
/// (`UV_NO_CONFIG`).
pub fn configured_index(backend: &Backend, venv: &Path, env: &[(String, String)], exists: &dyn Fn(&Path) -> bool) -> Option<String> {
    let var = |name: &str| env_var(env, name);
    let (replacing, besides): (&[&str], &[&str]) = match backend {
        Backend::Uv(_) => (&["UV_DEFAULT_INDEX", "UV_INDEX_URL"], &["UV_INDEX", "UV_EXTRA_INDEX_URL"]),
        Backend::Python { .. } => (&["PIP_INDEX_URL"], &["PIP_EXTRA_INDEX_URL"]),
    };
    let mut first = replacing.iter().find_map(|n| var(n)).map(str::to_string);
    if first.is_none() && matches!(backend, Backend::Python { .. }) && pip_config_file(venv, env, exists) {
        first = Some(PIP_CONFIG_INDEX.to_string());
    }
    let extra: Vec<&str> = besides.iter().filter_map(|n| var(n)).collect();
    if first.is_none() && extra.is_empty() {
        return None;
    }
    let mut sources = vec![first.unwrap_or_else(|| "pypi.org".to_string())];
    sources.extend(extra.into_iter().map(str::to_string));
    Some(sources.join(" or "))
}

/// A variable's value in `env`, its name compared ignoring case as Windows does; the last of
/// a name given twice, and None when empty.
fn env_var<'a>(env: &'a [(String, String)], name: &str) -> Option<&'a str> {
    env.iter().rev().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.trim()).filter(|v| !v.is_empty())
}

/// Whether pip may read a configuration file (pip's `_iter_config_files`):
/// `PIP_CONFIG_FILE`, `%ProgramData%\pip\pip.ini`, `%APPDATA%\pip\pip.ini` or the
/// environment's `pip.ini`; `PIP_CONFIG_FILE=nul` (`os.devnull`) turns them all off. A file
/// on a network path counts without being probed.
fn pip_config_file(venv: &Path, env: &[(String, String)], exists: &dyn Fn(&Path) -> bool) -> bool {
    let var = |name: &str| env_var(env, name);
    let named = var("PIP_CONFIG_FILE");
    if named.is_some_and(|f| f.eq_ignore_ascii_case("nul")) {
        return false;
    }
    let mut files: Vec<PathBuf> = named.map(PathBuf::from).into_iter().collect();
    files.extend(["ProgramData", "APPDATA"].iter().filter_map(|d| var(d)).map(|d| Path::new(d).join("pip").join("pip.ini")));
    files.push(venv.join("pip.ini"));
    files.iter().any(|f| crate::scripts::is_network_path(&f.to_string_lossy()) || exists(f))
}

/// The arguments of a pip command about one installed dependency (`show`, `uninstall`): the
/// subcommand and the dependency's package name. None when it names no package
/// (`rt::python_package`): `setup.cfg` is document text, and one such as
/// `--log=\\host\share\x` must never reach pip as an option.
pub fn package_args(subcommand: &str, dep: &str) -> Option<Vec<String>> {
    Some(vec![subcommand.to_string(), rt::python_package(dep)?])
}

/// The dependency re-pinned to the version an upgrade installed, by its bare name
/// (`DE:296-300`): "aiohttp>=3.1" and "3.14.3" make "aiohttp>=3.14.3".
pub fn after_upgrade(dep: &str, installed: &str) -> String {
    let name = rt::python_package(dep).unwrap_or_else(|| dep.to_string());
    format!("{name}>={installed}")
}

/// Which way a Python version change goes (`DE:661`, Sparkle's version comparator).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VersionChange {
    Same,
    Upgrade,
    Downgrade,
}

pub fn version_change(from: &str, to: &str) -> VersionChange {
    match rt::dotted(to, from) {
        Ordering::Equal => VersionChange::Same,
        Ordering::Greater => VersionChange::Upgrade,
        Ordering::Less => VersionChange::Downgrade,
    }
}

/// Where Upgrade makes a basic script's full environment: `<parent>\<stem>` (`DE:481-482`).
pub fn upgrade_folder(basic: &Path) -> PathBuf {
    let stem = basic.file_stem().map(PathBuf::from).unwrap_or_default();
    basic.parent().map_or_else(|| stem.clone(), |p| p.join(&stem))
}

/// The table's rows: the `setup.cfg` dependencies sorted with `compare:` (`DE:195-199`).
pub fn table_dependencies(cfg: &rt::SetupCfg) -> Vec<String> {
    let mut deps = cfg.dependencies.clone();
    deps.sort();
    deps
}

/// The Python Version popup's items (`DE:210-239`): the versions the backend can build, plus
/// the environment's own when missing, sorted by number.
pub fn version_choices(minors: &[String], current: &str) -> Vec<String> {
    let mut out = minors.to_vec();
    if !current.is_empty() && !out.iter().any(|m| m == current) {
        out.push(current.to_string());
    }
    out.sort_by(|a, b| rt::dotted(a, b));
    out
}

/// What runs pip for an environment, from its marker (no probe): Termurion's uv for one uv
/// built, else the environment's own Python.
pub fn marker_backend(marker: &rt::Marker, uv_exe: &Path, venv: &Path) -> Backend {
    if marker.backend == "uv" {
        Backend::Uv(uv_exe.to_path_buf())
    } else {
        Backend::Python { argv: vec![rt::venv_python(venv).to_string_lossy().into_owned()], minor: marker.python.clone() }
    }
}

/// `setup.cfg` rewritten with `deps` and `python`, its name and environment version kept
/// (`writeSetupCfgToFile:` with `ensureiTerm2Present:NO`, `DE:326-333`); None when it cannot
/// be read.
pub fn setup_cfg_with(existing: &str, deps: &[String], python: &str) -> Option<String> {
    let cfg = rt::SetupCfg::parse(existing)?;
    Some(rt::setup_cfg_text(&cfg.name, deps, false, python, cfg.environment_version))
}

/// The version an environment shows: its marker's, else its `setup.cfg` pin, cut to two
/// parts (`DE:212`, `DE:200`).
pub fn shown_version(marker: Option<&rt::Marker>, cfg: &rt::SetupCfg) -> String {
    marker.map(|m| m.python.clone()).or_else(|| cfg.python_version.clone()).map(|v| rt::two_part(&v)).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    fn script(path: &Path, full: bool) -> Node {
        Node::Script { name: path.file_name().unwrap().to_string_lossy().into_owned(), path: path.to_path_buf(), full_environment: full }
    }

    fn folder(path: &Path, children: Vec<Node>) -> Node {
        Node::Folder { name: path.file_name().unwrap().to_string_lossy().into_owned(), path: path.to_path_buf(), children }
    }

    #[test]
    fn dependency_editor_follows_iterm2() {
        // Task 1's tree as the scan gives it (folders first, then names ignoring case).
        let root = Path::new(r"C:\S");
        let tree = vec![
            folder(&root.join("AutoLaunch"), vec![script(&root.join(r"AutoLaunch\run.py"), false)]),
            folder(&root.join("zeta"), vec![script(&root.join(r"zeta\z.py"), false)]),
            script(&root.join("A.py"), false),
            script(&root.join("b.py"), false),
            script(&root.join("Full"), true),
        ];
        let choices = script_choices(&tree);
        let titles: Vec<&str> = choices.iter().map(|c| c.0.as_str()).collect();
        assert_eq!(titles, ["A.py", "AutoLaunch/run.py", "Full", "b.py", "zeta/z.py"]);
        assert_eq!(choices[1].1, root.join(r"AutoLaunch\run.py"));
        let full: Vec<bool> = choices.iter().map(|c| c.2).collect();
        assert_eq!(full, [false, false, true, false, false]);

        assert_eq!(version_in_pip_show("Name: requests\nVersion: 2.32.3\nSummary: x\n").as_deref(), Some("2.32.3"));
        // Windows pip ends its lines in CRLF.
        assert_eq!(version_in_pip_show("Name: requests\r\nVersion: 2.32.3\r\n").as_deref(), Some("2.32.3"));
        assert_eq!(version_in_pip_show("Name: requests\nSummary: x\n"), None);

        assert_eq!(after_upgrade("aiohttp>=3.1", "3.14.3"), "aiohttp>=3.14.3");
        assert_eq!(after_upgrade("requests", "2.32.3"), "requests>=2.32.3");
        assert_eq!(upgrade_folder(Path::new(r"C:\S\tool.py")), PathBuf::from(r"C:\S\tool"));

        assert_eq!(uninstall_failed("x"), "Uninstall of x failed. Check the pip output for errors.");
        assert_eq!(folder_exists(Path::new(r"C:\S\tool")), "Can\u{2019}t upgrade because C:\\S\\tool already exists");
        assert_eq!(error_creating(Path::new(r"C:\a"), "e"), "Error creating C:\\a: e");
        assert_eq!(error_moving(Path::new(r"C:\a"), Path::new(r"C:\b"), "e"), "Error moving C:\\a to C:\\b: e");
        assert_eq!(upgrade_error("e"), "Please file a bug report at https://github.com/datbird/termurion/issues. The following error occurred while upgrading a dependency: e");
        assert_eq!((ADD_HEADING, ADD_TEXT, RUN_HEADING, RUN_ID), ("Add Dependency", "What dependency would you like to add?", "Run this Command?", "DependencyEditorPip3Confirmation"));
        assert_eq!((INSTALL_CHECK_PIP, INSTALLATION_FAILED, REMOVAL_FAILED, COULD_NOT_CHANGE, ERROR), ("Check the pip output for errors.", "Installation Failed", "Removal Failed", "Could Not Change Python Version", "Error"));
        assert_eq!(REBUILDING, "Rebuilding the Python environment\u{2026}");
        assert!(UPGRADE_TEXT_BASIC.starts_with("This script must be upgraded to a \u{201c}full environment\u{201c} to add"));
        assert_eq!((TITLE, SCRIPT_LABEL, PYTHON_LABEL, PACKAGE_COLUMN, VERSION_COLUMN), ("Script Dependency Editor", "Script:", "Python Version:", "Package Name", "Installed Version"));
    }

    #[test]
    fn pip_commands_follow_iterm2() {
        let venv = Path::new(r"C:\S\t\.venv");
        let python = r"C:\S\t\.venv\Scripts\python.exe";
        let uv = Backend::Uv(PathBuf::from(r"C:\u\bin\uv.exe"));
        let (program, args, env) = pip_command(&uv, venv, &s(&["install", "x"]));
        assert_eq!(program, PathBuf::from(r"C:\u\bin\uv.exe"));
        assert_eq!(args, s(&["pip", "install", "x", "--only-binary", ":all:", "--python", python]));
        assert_eq!(env, rt::uv_env(Path::new(r"C:\u"), true));

        let py = Backend::Python { argv: s(&["py.exe", "-3.12"]), minor: "3.12".into() };
        let (program, args, env) = pip_command(&py, venv, &s(&["install", "x"]));
        assert_eq!(program, PathBuf::from(python));
        assert_eq!(args, s(&["-m", "pip", "install", "x", "--only-binary", ":all:"]));
        assert!(env.is_empty());

        let install = confirm_text(&program, &args, None);
        assert!(install.ends_with("\n\nPackages come from pypi.org."), "{install}");
        assert!(install.starts_with(&command_text(&program, &args)));
        let (program, args, _) = pip_command(&uv, venv, &s(&["uninstall", "x"]));
        let uninstall = confirm_text(&program, &args, None);
        assert!(!uninstall.contains("pypi.org"), "{uninstall}");
        assert_eq!(uninstall, command_text(&program, &args));
        let (program, args, _) = pip_command(&py, venv, &s(&["show", "x"]));
        assert!(!confirm_text(&program, &args, None).contains("pypi.org"));
        assert!(!confirm_text(&program, &args, Some("https://a/simple")).contains("https://a"));

        assert_eq!(command_text(Path::new(r"C:\u\bin\uv.exe"), &s(&["pip", "install", "x"])), r"C:\u\bin\uv.exe pip install x");
        assert_eq!(command_text(Path::new(r"C:\My Tools\uv.exe"), &s(&["--python", r"C:\a b\python.exe"])), r#""C:\My Tools\uv.exe" --python "C:\a b\python.exe""#);
    }

    #[test]
    fn only_package_names_reach_pip() {
        assert_eq!(package_args("show", "aiohttp>=3.1"), Some(s(&["show", "aiohttp"])));
        assert_eq!(package_args("uninstall", "requests"), Some(s(&["uninstall", "requests"])));
        // Options, direct references and paths from a document's setup.cfg never reach pip.
        for bad in [r"--log=\\h\s\x", "--cert=x", "-r x", "pkg @ https://h/x.whl", r"\\h\s\x", "", "a  b"] {
            assert_eq!(package_args("show", bad), None, "{bad}");
            assert_eq!(package_args("uninstall", bad), None, "{bad}");
        }
    }

    #[test]
    fn confirmation_names_the_configured_index() {
        let venv = Path::new(r"C:\S\t\.venv");
        let uv = Backend::Uv(PathBuf::from(r"C:\u\bin\uv.exe"));
        let py = Backend::Python { argv: s(&["py.exe", "-3.12"]), minor: "3.12".into() };
        let env = |v: &[(&str, &str)]| -> Vec<(String, String)> { v.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect() };
        let none = |p: &Path| {
            assert!(!crate::scripts::is_network_path(&p.to_string_lossy()), "probed {}", p.display());
            false
        };
        // Nothing set: pypi.org.
        assert_eq!(configured_index(&uv, venv, &env(&[("PATH", "x")]), &none), None);
        assert_eq!(configured_index(&py, venv, &env(&[("APPDATA", r"C:\A"), ("ProgramData", r"C:\P")]), &none), None);
        // uv's variables, names compared ignoring case; the default index replaces pypi.org.
        assert_eq!(configured_index(&uv, venv, &env(&[("UV_DEFAULT_INDEX", "https://a/simple")]), &none).as_deref(), Some("https://a/simple"));
        assert_eq!(configured_index(&uv, venv, &env(&[("uv_index_url", "https://b")]), &none).as_deref(), Some("https://b"));
        assert_eq!(configured_index(&uv, venv, &env(&[("UV_INDEX_URL", "https://b"), ("UV_DEFAULT_INDEX", "https://a")]), &none).as_deref(), Some("https://a"));
        assert_eq!(configured_index(&uv, venv, &env(&[("UV_INDEX", "https://c")]), &none).as_deref(), Some("pypi.org or https://c"));
        assert_eq!(configured_index(&uv, venv, &env(&[("UV_EXTRA_INDEX_URL", "https://d"), ("UV_DEFAULT_INDEX", "https://a")]), &none).as_deref(), Some("https://a or https://d"));
        // uv reads neither pip's variables nor its files.
        assert_eq!(configured_index(&uv, venv, &env(&[("PIP_INDEX_URL", "https://p"), ("APPDATA", r"C:\A")]), &|_| true), None);
        // pip's variables.
        assert_eq!(configured_index(&py, venv, &env(&[("PIP_INDEX_URL", "https://p")]), &none).as_deref(), Some("https://p"));
        assert_eq!(configured_index(&py, venv, &env(&[("PIP_EXTRA_INDEX_URL", "https://e")]), &none).as_deref(), Some("pypi.org or https://e"));
        assert_eq!(configured_index(&py, venv, &env(&[("UV_DEFAULT_INDEX", "https://a")]), &none), None);
        // pip's configuration files: the user's, the machine's, the environment's, and
        // PIP_CONFIG_FILE's.
        let only = |want: &'static str| move |p: &Path| p == Path::new(want);
        let dirs = env(&[("APPDATA", r"C:\A"), ("ProgramData", r"C:\P")]);
        for file in [r"C:\A\pip\pip.ini", r"C:\P\pip\pip.ini", r"C:\S\t\.venv\pip.ini"] {
            assert_eq!(configured_index(&py, venv, &dirs, &only(file)).as_deref(), Some(PIP_CONFIG_INDEX), "{file}");
        }
        assert_eq!(configured_index(&py, venv, &env(&[("PIP_CONFIG_FILE", r"C:\c.ini")]), &only(r"C:\c.ini")).as_deref(), Some(PIP_CONFIG_INDEX));
        assert_eq!(configured_index(&py, venv, &env(&[("PIP_CONFIG_FILE", r"C:\c.ini")]), &none), None);
        // PIP_CONFIG_FILE=nul (os.devnull) turns every file off.
        let mut off = dirs.clone();
        off.push(("PIP_CONFIG_FILE".into(), "nul".into()));
        assert_eq!(configured_index(&py, venv, &off, &|_| true), None);
        // A variable wins over the files, as in pip; extras follow.
        let mut both = dirs.clone();
        both.push(("PIP_INDEX_URL".into(), "https://p".into()));
        assert_eq!(configured_index(&py, venv, &both, &|_| true).as_deref(), Some("https://p"));
        let mut extra = dirs.clone();
        extra.push(("PIP_EXTRA_INDEX_URL".into(), "https://e".into()));
        assert_eq!(configured_index(&py, venv, &extra, &only(r"C:\A\pip\pip.ini")).as_deref(), Some("the index your pip configuration names or https://e"));
        // A file on a network path counts, never probed.
        assert_eq!(configured_index(&py, venv, &env(&[("PIP_CONFIG_FILE", r"\\h\s\pip.ini")]), &none).as_deref(), Some(PIP_CONFIG_INDEX));
        assert_eq!(configured_index(&py, venv, &env(&[("APPDATA", r"\\h\home")]), &none).as_deref(), Some(PIP_CONFIG_INDEX));

        // The confirmation names it.
        let (program, args, _) = pip_command(&py, venv, &s(&["install", "x"]));
        let text = confirm_text(&program, &args, Some("https://a/simple"));
        assert!(text.ends_with("\n\nPackages come from https://a/simple."), "{text}");
        assert!(!text.contains("pypi.org"), "{text}");
        assert!(confirm_text(&program, &args, None).ends_with("\n\nPackages come from pypi.org."));
    }

    #[test]
    fn editor_state_follows_iterm2() {
        let cfg = rt::SetupCfg { name: "t".into(), dependencies: s(&["zlib", "Numpy", "aiohttp>=3.1"]), python_version: Some("3.10.2".into()), environment_version: 80 };
        assert_eq!(table_dependencies(&cfg), s(&["Numpy", "aiohttp>=3.1", "zlib"]));
        assert_eq!(version_choices(&s(&["3.13", "3.9", "3.12"]), "3.10"), s(&["3.9", "3.10", "3.12", "3.13"]));
        assert_eq!(version_choices(&[], "3.12"), s(&["3.12"]));
        let marker = rt::Marker { schema: 1, backend: "uv".into(), uv_version: "0.1".into(), python: "3.12".into(), remapped_from: None };
        assert_eq!(shown_version(Some(&marker), &cfg), "3.12");
        assert_eq!(shown_version(None, &cfg), "3.10");
        let (uv, venv) = (Path::new(r"C:\u\bin\uv.exe"), Path::new(r"C:\S\t\.venv"));
        assert_eq!(marker_backend(&marker, uv, venv), Backend::Uv(uv.to_path_buf()));
        let venv_marker = rt::Marker { backend: "venv".into(), ..marker };
        assert_eq!(marker_backend(&venv_marker, uv, venv), Backend::Python { argv: s(&[r"C:\S\t\.venv\Scripts\python.exe"]), minor: "3.12".into() });
        // The name and environment version stay; the version and dependencies are the new ones.
        let text = rt::setup_cfg_text("t", &s(&["iterm2", "x"]), true, "3.10", 80);
        let out = setup_cfg_with(&text, &s(&["iterm2", "x", "y"]), "3.12").unwrap();
        let back = rt::SetupCfg::parse(&out).unwrap();
        assert_eq!(back, rt::SetupCfg { name: "t".into(), dependencies: s(&["iterm2", "x", "y"]), python_version: Some("3.12".into()), environment_version: 80 });
        assert_eq!(setup_cfg_with("[bad", &[], "3.12"), None);
    }

    #[test]
    fn version_change_follows_iterm2() {
        assert_eq!(version_change("3.10", "3.12"), VersionChange::Upgrade);
        // Dotted numbers, not text: "3.9" is older than "3.12".
        assert_eq!(version_change("3.12", "3.9"), VersionChange::Downgrade);
        assert_eq!(version_change("3.12", "3.12"), VersionChange::Same);
        assert_eq!(DOWNGRADE_HEADING, "Confirm Python Downgrade");
        assert_eq!(DOWNGRADE_TEXT, "You have asked to downgrade to an older Python version. Dependencies will need to be reinstalled. This may go badly. Are you sure you want to do this?");
        assert_eq!(UPGRADE_HEADING, "Confirm Python Upgrade");
        assert_eq!(UPGRADE_TEXT, "You have asked to upgrade to a newer Python version. Dependencies will need to be reinstalled. OK to continue?");
    }
}
