//! Export All Settings and Data (iTerm2's `ImportExport.swift`, `IE`): the save panel's
//! texts, the entities and their folders in the archive, which entries of `%APPDATA%\Tandem`
//! stay out, `PythonRuntimes.json`, the script zips' names, tar.exe's arguments and the
//! failure texts. Pure.

use std::path::Path;

use serde::Serialize;

/// The archive's extension (`IE:22`, iTerm2's `itermexport`).
pub const EXTENSION: &str = "tandemexport";
/// The name the save panel suggests (`IE:23`).
pub const DEFAULT_NAME: &str = "Tandem State.tandemexport";
/// The save panel's title (`IE:24`).
pub const PANEL_TITLE: &str = "Export Tandem Settings and Data";
/// The save panel's file type (Tandem's; macOS names the content type itself).
pub const FILTER_NAME: &str = "Tandem Settings and Data";
/// The failure alert's heading (`GP:4085`).
pub const PROBLEM_EXPORTING: &str = "Problem Exporting";

/// What the archive holds, one folder each (`IE:393-429`). iTerm2's secure settings, the
/// automation authorization and `~/.iterm2` have no Tandem counterpart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Entity {
    PythonRuntimes,
    UserDefaults,
    AppSupport,
    Scripts,
}

impl Entity {
    /// The entity's folder in the archive.
    pub fn key(self) -> &'static str {
        match self {
            Entity::PythonRuntimes => "python-runtimes",
            Entity::UserDefaults => "user-defaults",
            Entity::AppSupport => "app-support",
            Entity::Scripts => "scripts",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Entity::PythonRuntimes => "Python Runtimes",
            Entity::UserDefaults => "User Defaults",
            Entity::AppSupport => "Application Support",
            Entity::Scripts => "Python API Scripts",
        }
    }
}

/// The entities in iTerm2's order.
pub const ENTITIES: [Entity; 4] = [Entity::PythonRuntimes, Entity::UserDefaults, Entity::AppSupport, Entity::Scripts];

/// Entries of `%APPDATA%\Tandem` the Application Support entity leaves out (iTerm2's app-support
/// exclusions, `IE:410-424`, for Tandem's files): the settings and profiles (the User Defaults
/// entity carries them), their temporary and recovered copies, the Scripts folder (the Scripts
/// entity), uv's runtimes (provisioned again on first launch), the Python API consent marker
/// (only OK in "Enable Python API?" grants it), the host's log and an import waiting for the
/// next start. Glob patterns on one name, at the top level only (`IE:1181`, `:1199`).
pub const APP_SUPPORT_EXCLUDE: [&str; 9] = ["settings.toml", "profiles.toml", "*.tmp", "*.recovered", "Scripts", "uv", "python-api-consent", "host.log", "import-pending"];

/// `fnmatch` on one name (`IE:1235-1241`): `*` any run, `?` one character; ASCII case is
/// ignored, as Windows file names ignore it.
pub fn glob_match(name: &str, pattern: &str) -> bool {
    let (n, p): (Vec<char>, Vec<char>) = (name.chars().collect(), pattern.chars().collect());
    // The classic backtracking match: on a mismatch, the last `*` takes one more character.
    let (mut i, mut j) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while i < n.len() {
        if j < p.len() && p[j] == '*' {
            star = Some((j, i));
            j += 1;
        } else if j < p.len() && (p[j] == '?' || p[j].eq_ignore_ascii_case(&n[i])) {
            i += 1;
            j += 1;
        } else if let Some((sj, si)) = star {
            star = Some((sj, si + 1));
            j = sj + 1;
            i = si + 1;
        } else {
            return false;
        }
    }
    p[j..].iter().all(|&c| c == '*')
}

/// Whether the Application Support entity leaves out the entry `name`.
pub fn excluded(name: &str) -> bool {
    APP_SUPPORT_EXCLUDE.iter().any(|p| glob_match(name, p))
}

/// The User Defaults entity's file: the syncable settings and the profiles (iTerm2's
/// `UserDefaults.plist`, `IE:829-835`).
pub const USER_DEFAULTS_FILE: &str = "UserDefaults.toml";
/// The Python Runtimes entity's file (`IE:694`).
pub const RUNTIMES_FILE: &str = "PythonRuntimes.json";

/// One runtime a full-environment script needs (`IE:687-692`).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Requirement {
    pub minimum_runtime: i64,
    pub python_version: String,
}

/// `PythonRuntimes.json` (`IE:733-735`): the requirements, then whether any basic script exists,
/// with iTerm2's key names.
pub fn runtimes_json(requirements: &[Requirement], have_basic: bool) -> String {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Req<'a> {
        minimum_runtime: i64,
        python_version: &'a str,
    }
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Info<'a> {
        requirements: Vec<Req<'a>>,
        have_any_basic_scripts: bool,
    }
    let requirements = requirements.iter().map(|r| Req { minimum_runtime: r.minimum_runtime, python_version: &r.python_version }).collect();
    serde_json::to_string(&Info { requirements, have_any_basic_scripts: have_basic }).unwrap_or_default()
}

/// A script's zip in the Scripts entity (`IE:1033-1038`).
pub fn script_zip_name(autolaunch: bool, uuid: &str) -> String {
    if autolaunch { format!("autolaunch {uuid}.zip") } else { format!("script {uuid}.zip") }
}

/// tar.exe's arguments writing the gzip tar of the entities' folders in `temp` to `out`
/// (`IE:599-616`).
pub fn archive_args(out: &Path, temp: &Path) -> Vec<String> {
    let s = |p: &Path| p.to_string_lossy().into_owned();
    let mut args = vec!["-czf".to_string(), s(out), "-C".to_string(), s(temp)];
    args.extend(ENTITIES.iter().map(|e| e.key().to_string()));
    args
}

/// Why an export (or, later, an import) failed (`IE:44-65`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExportError {
    TempDir(String),
    Folder(String),
    Copy(String),
    Save(String),
    Bug(String),
    Archive(String),
    Load(String),
    Malformed(String),
    Script(String),
    Runtime,
}

impl ExportError {
    /// The alert's text under "Problem Exporting".
    pub fn message(&self) -> String {
        match self {
            ExportError::TempDir(r) => format!("Failed to create temporary directory: {r}"),
            ExportError::Folder(r) => format!("Failed to create folder: {r}"),
            ExportError::Copy(r) => format!("Failed to copy file: {r}"),
            ExportError::Save(r) => format!("Failed to save file: {r}"),
            ExportError::Bug(r) => format!("A bug was encountered: {r}. Please report this at https://iterm2.com/bugs"),
            ExportError::Archive(r) => format!("Failed to create archive: {r}"),
            ExportError::Load(r) => format!("Failed to load file: {r}"),
            ExportError::Malformed(r) => format!("Malformed data found: {r}"),
            ExportError::Script(r) => format!("Script could not be exported: {r}"),
            ExportError::Runtime => "Failed to install Python runtime".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entities_follow_iterm2() {
        let keys: Vec<&str> = ENTITIES.iter().map(|e| e.key()).collect();
        assert_eq!(keys, ["python-runtimes", "user-defaults", "app-support", "scripts"]);
        let names: Vec<&str> = ENTITIES.iter().map(|e| e.display_name()).collect();
        assert_eq!(names, ["Python Runtimes", "User Defaults", "Application Support", "Python API Scripts"]);
        for name in ["settings.toml", "profiles.toml", "settings.toml.tmp", "profiles.toml.recovered", "Scripts", "uv", "python-api-consent", "host.log", "import-pending"] {
            assert!(excluded(name), "{name}");
        }
        for name in ["snippets.json", "notes.txt", "state", "shell_integration"] {
            assert!(!excluded(name), "{name}");
        }
        assert!(glob_match("iTermServer-12", "iTermServer-*"));
        assert!(glob_match("a.toml", "?.toml"));
        assert!(!glob_match("ab.toml", "?.toml"));
        // Beyond the plan's cases: case is ignored, `*` may match nothing, and the whole name
        // must match.
        assert!(glob_match("SETTINGS.TOML", "settings.toml"));
        assert!(glob_match("scripts", "Scripts"));
        assert!(glob_match(".tmp", "*.tmp"));
        assert!(!glob_match("settings.toml.bak", "settings.toml"));
        assert!(!glob_match("a.tmpx", "*.tmp"));
        assert!(glob_match("a.b.c", "*.*.*"));
        assert!(!excluded("Scripts2"));
    }

    #[test]
    fn runtimes_json_follows_iterm2() {
        assert_eq!(
            runtimes_json(&[Requirement { minimum_runtime: 0, python_version: "3.12".into() }], true),
            r#"{"requirements":[{"minimumRuntime":0,"pythonVersion":"3.12"}],"haveAnyBasicScripts":true}"#
        );
        assert_eq!(runtimes_json(&[], false), r#"{"requirements":[],"haveAnyBasicScripts":false}"#);
    }

    #[test]
    fn archive_follows_the_rule() {
        let (out, temp) = (Path::new(r"C:\Users\u\Tandem State.tandemexport"), Path::new(r"C:\T\abc"));
        assert_eq!(
            archive_args(out, temp),
            ["-czf", r"C:\Users\u\Tandem State.tandemexport", "-C", r"C:\T\abc", "python-runtimes", "user-defaults", "app-support", "scripts"]
        );
        assert_eq!(script_zip_name(false, "U"), "script U.zip");
        assert_eq!(script_zip_name(true, "U"), "autolaunch U.zip");
    }

    #[test]
    fn export_texts_follow_iterm2() {
        let x = || "x".to_string();
        for (e, text) in [
            (ExportError::TempDir(x()), "Failed to create temporary directory: x"),
            (ExportError::Folder(x()), "Failed to create folder: x"),
            (ExportError::Copy(x()), "Failed to copy file: x"),
            (ExportError::Save(x()), "Failed to save file: x"),
            (ExportError::Bug(x()), "A bug was encountered: x. Please report this at https://iterm2.com/bugs"),
            (ExportError::Archive(x()), "Failed to create archive: x"),
            (ExportError::Load(x()), "Failed to load file: x"),
            (ExportError::Malformed(x()), "Malformed data found: x"),
            (ExportError::Script(x()), "Script could not be exported: x"),
            (ExportError::Runtime, "Failed to install Python runtime"),
        ] {
            assert_eq!(e.message(), text);
        }
        assert_eq!(PROBLEM_EXPORTING, "Problem Exporting");
        assert_eq!(DEFAULT_NAME, "Tandem State.tandemexport");
        assert_eq!(PANEL_TITLE, "Export Tandem Settings and Data");
        assert_eq!(EXTENSION, "tandemexport");
        assert_eq!(FILTER_NAME, "Tandem Settings and Data");
    }
}
