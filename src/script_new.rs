//! New Python Script (`M:1081-1264`, `M:1435-1540`, `M:1682-1712`): the four templates,
//! where a new script goes for each environment, the name and folder rules, and the texts of
//! the picker and the questions after it.

use std::path::{Path, PathBuf};

use crate::script_runtime as rt;

/// The picker's first choice (`iTermScriptEnvironment`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Env {
    Basic,
    Full,
}

/// The picker's second choice (`iTermScriptTemplate`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Template {
    Simple,
    Daemon,
}

/// `pathToTemplateForPicker:` (`M:1081-1089`): `template_{basic,pyenv}_{simple,daemon}.py`.
pub fn template_name(env: Env, t: Template) -> &'static str {
    match (env, t) {
        (Env::Basic, Template::Simple) => "template_basic_simple.py",
        (Env::Basic, Template::Daemon) => "template_basic_daemon.py",
        (Env::Full, Template::Simple) => "template_pyenv_simple.py",
        (Env::Full, Template::Daemon) => "template_pyenv_daemon.py",
    }
}

/// iTerm2's template as vendored, byte for byte (`sources/template_*.py` at `64791fc`).
pub fn vendored(env: Env, t: Template) -> &'static str {
    match (env, t) {
        (Env::Basic, Template::Simple) => include_str!("../resources/scripts/template_basic_simple.py"),
        (Env::Basic, Template::Daemon) => include_str!("../resources/scripts/template_basic_daemon.py"),
        (Env::Full, Template::Simple) => include_str!("../resources/scripts/template_pyenv_simple.py"),
        (Env::Full, Template::Daemon) => include_str!("../resources/scripts/template_pyenv_daemon.py"),
    }
}

/// The SHA-256 of each vendored template: iTerm2's files at `64791fc`.
pub const TEMPLATE_SHA256: [(&str, &str); 4] = [
    ("template_basic_simple.py", "4ccca42b5cbe7527d5b0ef065daca25117457f128f67ea485631adc07d1650da"),
    ("template_basic_daemon.py", "02db43f30c5253b65b29a3fd52e96961e8c07799b75090be5d7b4a2af772588a"),
    ("template_pyenv_simple.py", "5eaf612437204bf703f5e03ef2ed0e1a699c62576a5fd8aa552fb120e3f0850f"),
    ("template_pyenv_daemon.py", "d02aa2a592dcbdfe6334d8fe2d02d911fb7855494dcce2d7aa2acd11b2a24388"),
];

/// `templateForPicker:url:` (`M:1479-1491`): `$$PYTHON_VERSION$$` becomes the two-part
/// version, else "3"; the comments name Tandem.
pub fn template(env: Env, t: Template, python: Option<&str>) -> String {
    let version = python.map(rt::two_part).unwrap_or_else(|| "3".to_string());
    vendored(env, t).replace("iTerm2", "Tandem").replace("$$PYTHON_VERSION$$", &version)
}

/// What the picker and the save dialog chose.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    pub env: Env,
    pub template: Template,
    /// A full environment's PyPI dependencies (empty for a basic script).
    pub dependencies: Vec<String>,
    /// A full environment's Python version (the popup's).
    pub python: String,
    /// The path the save dialog returned.
    pub chosen: PathBuf,
}

/// A path's components as `under_scripts` and `container` compare them: either separator, the
/// local long-path prefix `\\?\` dropped (`\\?\D:\S` is `D:\S`, and the save dialog
/// returns paths without it), `.` dropped and `..` taking back the component before it (never
/// the drive). A network path keeps a leading `\\` of its own.
fn parts(p: &Path) -> Vec<String> {
    let text = p.to_string_lossy().replace('/', "\\");
    let text = match text.strip_prefix(r"\\?\") {
        Some(rest) if rest.len() >= 2 && rest.as_bytes()[0].is_ascii_alphabetic() && rest.as_bytes()[1] == b':' => rest.to_string(),
        _ => text,
    };
    let mut out: Vec<String> = Vec::new();
    if text.starts_with(r"\\") {
        out.push(r"\\".to_string());
    }
    for part in text.split('\\') {
        match part {
            "" | "." => {}
            ".." => {
                if out.len() > 1 {
                    out.pop();
                }
            }
            _ => out.push(part.to_string()),
        }
    }
    out
}

/// `parts` with letters folded (Windows paths ignore case).
fn comparable(p: &Path) -> Vec<String> {
    parts(p).into_iter().map(|c| c.to_lowercase()).collect()
}

/// `urlIsUnderScripts:` (`M:1682-1686`): the Scripts folder itself or anything inside it.
pub fn under_scripts(path: &Path, scripts: &Path) -> bool {
    let (p, s) = (comparable(path), comparable(scripts));
    !s.is_empty() && p.len() >= s.len() && p[..s.len()] == s[..]
}

/// `folderForFullEnvironmentSavePanelURL:` (`M:1509-1524`): the chosen path when it is under
/// the Scripts folder (spelled from the Scripts folder), else the Scripts folder and the
/// chosen name.
pub fn container(chosen: &Path, scripts: &Path) -> PathBuf {
    let name = chosen.file_name().map(PathBuf::from).unwrap_or_default();
    if !under_scripts(chosen, scripts) {
        return scripts.join(name);
    }
    // The components past the Scripts folder's own.
    let depth = parts(scripts).len();
    parts(chosen).into_iter().skip(depth).fold(scripts.to_path_buf(), |p, c| p.join(c))
}

/// A full environment's main script, `<container>\<name>\<name>.py`.
fn full_main(container: &Path, name: &str) -> PathBuf {
    container.join(name).join(format!("{name}.py"))
}

/// `destinationTemplatePathForPicker:url:` (`M:1526-1540`): a basic script is the chosen file;
/// a full environment's main is `<container>\<name>\<name>.py`.
pub fn destination(env: Env, chosen: &Path, scripts: &Path) -> PathBuf {
    match env {
        Env::Basic => chosen.to_path_buf(),
        Env::Full => {
            let name = chosen.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            full_main(&container(chosen, scripts), &name)
        }
    }
}

/// The name with every space as "_", when it has a space (`M:1455-1458`).
pub fn spaceless(file_name: &str) -> Option<String> {
    file_name.contains(' ').then(|| file_name.replace(' ', "_"))
}

/// The dependencies field's packages: iTerm2's token field splits on whitespace (`M:1318-1323`).
pub fn parse_dependencies(text: &str) -> Vec<String> {
    text.split_whitespace().map(str::to_string).collect()
}

/// A full environment's files besides its `.venv`: `setup.cfg` (`rt::setup_cfg_after_build`:
/// an `existing` one's name and environment version kept, the version built, the accepted
/// dependencies) and the main `<container>\<name>\<name>.py` holding `main`
/// (`M:1526-1540`). The one helper for New Python Script, an export's staged basic script and
/// the basic-to-full upgrade.
pub fn full_environment_files(container: &Path, name: &str, dependencies: &[String], built: &str, main: &str, existing: Option<&str>) -> Vec<(PathBuf, String)> {
    vec![(container.join(rt::SETUP_CFG), rt::setup_cfg_after_build(existing, name, dependencies, built)), (full_main(container, name), main.to_string())]
}

/// The Python version a built environment has: its marker's (`UV:683-689` records
/// `marker.python`); the version asked for only when the marker cannot be read.
pub fn built_python(marker: Option<&str>, requested: &str) -> String {
    marker.and_then(rt::Marker::parse).map_or_else(|| requested.to_string(), |m| m.python)
}

/// Writes each file, making its folder first.
pub fn write_files(files: &[(PathBuf, String)]) -> Result<(), String> {
    for (path, text) in files {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("Could not create {}: {e}", parent.display()))?;
        }
        std::fs::write(path, text).map_err(|e| format!("Could not write {}: {e}", path.display()))?;
    }
    Ok(())
}

/// The picker window's title.
pub const PICKER_TITLE: &str = "New Python Script";
/// The picker's texts (`iTermScriptTemplatePickerWindowController.xib`).
pub const ENV_PROMPT: &str = "Choose a Python environment:";
pub const BASIC: &str = "Basic";
pub const BASIC_TEXT: &str = "A simple script that doesn't have any dependencies besides the iterm2 library.";
pub const FULL: &str = "Full Environment";
pub const FULL_TEXT: &str = "A script that depends on PyPI packages other than the iterm2 library. A private pyenv will be created for this script.";
pub const TEMPLATE_PROMPT: &str = "Choose a script template:";
pub const SIMPLE: &str = "Simple";
pub const SIMPLE_TEXT: &str = "Choose this template for a simple script that performs actions and terminates.";
pub const DAEMON: &str = "Long-Running Daemon";
pub const DAEMON_TEXT: &str = "Choose this template for a script that stays running and responds to notifications.";
/// The save panel's accessory fields, moved into the picker (`M:1318-1345`, `M:1379-1395`).
pub const DEPS_LABEL: &str = "PyPI Dependencies:";
pub const DEPS_PLACEHOLDER: &str = "Package names";
pub const VERSION_LABEL: &str = "Python Version:";

/// A name with spaces (`M:1460-1465`).
pub const SPACES_HEADING: &str = "Spaces Not Allowed";
pub const SPACES_TEXT: &str = "Scripts can't have space characters in their filenames.";
pub const USE_UNDERSCORE: &str = "Use _ Instead of Space";
pub const CHANGE_NAME: &str = "Change Name";

/// After the script is written (`M:1243-1260`).
pub fn open_in(app: &str) -> String {
    format!("Open new script in {app}?")
}
pub const SHOW_IN_EXPLORER: &str = "Show in Explorer";
pub const OPEN_IN_ID: &str = "NoSyncOpenNewPythonScriptInDefaultEditor";
/// iTermWarning's heading when none is given (`iTermWarning.m:330`).
pub const WARNING: &str = "Warning";

/// A place outside the Scripts folder (`M:1693-1709`).
pub const INVALID_FOLDER: &str = "Invalid Folder";
pub fn location_restricted(scripts: &Path) -> String {
    format!("Full-environment scripts must be located under in your Tandem Scripts directory:\n{}", scripts.display())
}
pub const LOCATION_ID: &str = "FullEnvironmentScriptsLocationRestricted";

/// A full environment that could not be built (`M:1186-1193`).
pub const INSTALLATION_FAILED: &str = rt::INSTALL_FAILED_HEADING;
pub fn env_create_error(e: &str) -> String {
    format!("An error occurred while creating the Python environment. The error was: {e}")
}
pub use crate::script_runtime::SETTING_UP;

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn templates_follow_iterm2() {
        use sha2::{Digest, Sha256};
        for env in [Env::Basic, Env::Full] {
            for t in [Template::Simple, Template::Daemon] {
                let name = template_name(env, t);
                let want = TEMPLATE_SHA256.iter().find(|(n, _)| *n == name).map(|(_, h)| *h).unwrap();
                let got: String = Sha256::digest(vendored(env, t).as_bytes()).iter().map(|b| format!("{b:02x}")).collect();
                assert_eq!(got, want, "{name}");
                // Comments name Tandem; nothing else changes but the version.
                assert_eq!(template(env, t, Some("3.12")), vendored(env, t).replace("iTerm2", "Tandem").replace("$$PYTHON_VERSION$$", "3.12"));
                assert!(!template(env, t, None).contains("iTerm2"));
            }
        }
        assert_eq!(template_name(Env::Basic, Template::Daemon), "template_basic_daemon.py");
        assert_eq!(template_name(Env::Full, Template::Simple), "template_pyenv_simple.py");
        assert!(template(Env::Basic, Template::Simple, Some("3.12")).starts_with("#!/usr/bin/env python3.12\n\nimport iterm2\n"));
        assert!(template(Env::Basic, Template::Simple, None).starts_with("#!/usr/bin/env python3\n"));
        // A three-part version is cut to two (`M:1553`).
        assert!(template(Env::Basic, Template::Simple, Some("3.12.4")).starts_with("#!/usr/bin/env python3.12\n"));
        let daemon = template(Env::Full, Template::Daemon, None);
        // The brief's "a terminal session in Tandem ..." spans a line break in iTerm2's file:
        // "in a\n    # terminal session in iTerm2 while this script is running:".
        assert!(daemon.contains("issuing this command in a\n    # terminal session in Tandem while this script is running"));
        assert!(!daemon.contains("iTerm2"));
    }

    #[test]
    fn new_script_paths_follow_iterm2() {
        let s = p(r"C:\S");
        assert_eq!(destination(Env::Basic, &p(r"C:\S\tool.py"), &s), p(r"C:\S\tool.py"));
        assert_eq!(container(&p(r"C:\S\sub\tool"), &s), p(r"C:\S\sub\tool"));
        assert_eq!(destination(Env::Full, &p(r"C:\S\sub\tool"), &s), p(r"C:\S\sub\tool\tool\tool.py"));
        // A place outside Scripts falls back to Scripts\name.
        assert_eq!(container(&p(r"D:\x\tool"), &s), p(r"C:\S\tool"));
        assert_eq!(destination(Env::Full, &p(r"D:\x\tool"), &s), p(r"C:\S\tool\tool\tool.py"));
        assert!(under_scripts(&p(r"C:\S"), &s));
        assert!(under_scripts(&p(r"C:\S\a"), &s));
        assert!(!under_scripts(&p(r"C:\Sx"), &s));
        // Windows paths ignore case and take either separator.
        assert!(under_scripts(&p(r"c:\s\a"), &s));
        assert!(under_scripts(&p("C:/S/a"), &s));
        assert!(under_scripts(&p(r"C:\S\"), &s));
        assert_eq!(container(&p(r"c:\s\Tool"), &s), p(r"C:\S\Tool"));
        // A Scripts folder in the local long-path form matches the dialog's plain path, and
        // the container is spelled from the Scripts folder.
        let long = p(r"\\?\D:\S");
        assert!(under_scripts(&p(r"D:\S\a"), &long));
        assert!(under_scripts(&p(r"\\?\D:\S\a"), &p(r"D:\S")));
        assert_eq!(container(&p(r"D:\S\sub\a"), &long), p(r"\\?\D:\S\sub\a"));
        // `..` is resolved before comparing: out of Scripts is out, back in is in.
        assert!(!under_scripts(&p(r"C:\S\..\x\a"), &s));
        assert!(under_scripts(&p(r"C:\S\x\..\a"), &s));
        assert_eq!(container(&p(r"C:\S\x\..\a"), &s), p(r"C:\S\a"));
        assert!(!under_scripts(&p(r"C:\..\..\S\a"), &p(r"D:\S")));
        // A share is never the local folder of the same name.
        assert!(!under_scripts(&p(r"\\srv\S\a"), &p(r"\srv\S")));

        let deps = ["requests".to_string(), "--index-url=https://h/simple".to_string()];
        let files = full_environment_files(&p(r"C:\S\tool"), "tool", &deps, "3.12", "print(1)\n", None);
        assert_eq!(
            files,
            vec![(p(r"C:\S\tool\setup.cfg"), rt::setup_cfg_text("tool", &["requests".to_string()], true, "3.12", 72)), (p(r"C:\S\tool\tool\tool.py"), "print(1)\n".to_string())]
        );
        let old = rt::setup_cfg_text("kept", &[], true, "3.9", 3);
        assert_eq!(full_environment_files(&p(r"C:\S\tool"), "tool", &[], "3.12", "", Some(&old))[0].1, rt::setup_cfg_text("kept", &[], true, "3.12", 3));
        // setup.cfg records the version built (the marker's), not the one picked.
        let marker = rt::Marker { schema: 1, backend: "venv".into(), uv_version: String::new(), python: "3.12".into(), remapped_from: Some("3.11".into()) };
        assert_eq!(built_python(Some(&marker.json()), "3.11"), "3.12");
        assert_eq!(built_python(None, "3.11"), "3.11");
        assert_eq!(built_python(Some("{}"), "3.11"), "3.11");
    }

    #[test]
    fn new_script_names_follow_iterm2() {
        assert_eq!(spaceless("my tool.py"), Some("my_tool.py".to_string()));
        assert_eq!(spaceless("tool.py"), None);
        assert_eq!(parse_dependencies(" requests  aiohttp>=3 "), vec!["requests".to_string(), "aiohttp>=3".to_string()]);
        assert!(parse_dependencies("  ").is_empty());
        assert_eq!(SPACES_HEADING, "Spaces Not Allowed");
        assert_eq!(SPACES_TEXT, "Scripts can't have space characters in their filenames.");
        assert_eq!(USE_UNDERSCORE, "Use _ Instead of Space");
        assert_eq!(CHANGE_NAME, "Change Name");
        assert_eq!(open_in("Notepad"), "Open new script in Notepad?");
        assert_eq!(SHOW_IN_EXPLORER, "Show in Explorer");
        assert_eq!(OPEN_IN_ID, "NoSyncOpenNewPythonScriptInDefaultEditor");
        assert_eq!(location_restricted(Path::new(r"C:\S")), "Full-environment scripts must be located under in your Tandem Scripts directory:\nC:\\S");
        assert_eq!(LOCATION_ID, "FullEnvironmentScriptsLocationRestricted");
        assert_eq!(INVALID_FOLDER, "Invalid Folder");
        assert_eq!(INSTALLATION_FAILED, "Installation Failed");
        assert_eq!(env_create_error("boom"), "An error occurred while creating the Python environment. The error was: boom");
        assert_eq!(SETTING_UP, "Setting up the Python environment\u{2026}");
        // The picker (the xib's texts, the accessory view's labels).
        assert_eq!(PICKER_TITLE, "New Python Script");
        assert_eq!(ENV_PROMPT, "Choose a Python environment:");
        assert_eq!((BASIC, BASIC_TEXT), ("Basic", "A simple script that doesn't have any dependencies besides the iterm2 library."));
        assert_eq!((FULL, FULL_TEXT), ("Full Environment", "A script that depends on PyPI packages other than the iterm2 library. A private pyenv will be created for this script."));
        assert_eq!(TEMPLATE_PROMPT, "Choose a script template:");
        assert_eq!((SIMPLE, SIMPLE_TEXT), ("Simple", "Choose this template for a simple script that performs actions and terminates."));
        assert_eq!((DAEMON, DAEMON_TEXT), ("Long-Running Daemon", "Choose this template for a script that stays running and responds to notifications."));
        assert_eq!((DEPS_LABEL, DEPS_PLACEHOLDER, VERSION_LABEL), ("PyPI Dependencies:", "Package names", "Python Version:"));
    }
}
