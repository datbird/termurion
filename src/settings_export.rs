//! Export All Settings and Data (iTerm2's `ImportExport.swift`, `IE`): the save panel's
//! texts, the entities and their folders in the archive, which entries of `%APPDATA%\Tandem`
//! stay out, `PythonRuntimes.json`, the script zips' names, tar.exe's arguments and the
//! failure texts; Import All's staging and the marker naming it for the next start; Erase
//! All's list and texts. Pure.

use std::path::{Path, PathBuf};

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

/// The file in `%APPDATA%\Tandem` naming the extracted export the next start applies (iTerm2's
/// `iTermUserDefaults.importPath`, `IE:112-120`).
pub const MARKER: &str = "import-pending";

/// The staging folder's name before its UUID (iTerm2's `~/.<UUID>`, `IE:95`).
const STAGING_PREFIX: &str = ".tandem-import-";

/// Where an import is extracted: `<temp>\.tandem-import-<uuid>`.
pub fn staging_dir(temp: &Path, uuid: &str) -> PathBuf {
    temp.join(format!("{STAGING_PREFIX}{uuid}"))
}

/// tar.exe's arguments extracting the archive `file` into `to`.
pub fn extract_args(file: &Path, to: &Path) -> Vec<String> {
    vec!["-xzf".to_string(), file.to_string_lossy().into_owned(), "-C".to_string(), to.to_string_lossy().into_owned()]
}

/// What the marker asks of this start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pending {
    /// No marker: nothing to do.
    None,
    /// Apply the export extracted here, then remove it and the marker.
    Apply(PathBuf),
    /// A marker naming nothing usable: it is removed and nothing is applied.
    Stale,
}

/// The marker's text read (None: no marker). Only a folder named as `staging_dir` names, on a
/// local path, is applied, since it is removed afterwards (iTerm2's precondition that the
/// folder is hidden and named by a UUID, `IE:117`); a folder that is gone is stale.
pub fn pending(marker_text: Option<&str>, is_dir: &dyn Fn(&Path) -> bool) -> Pending {
    let Some(text) = marker_text else { return Pending::None };
    let path = PathBuf::from(text.trim());
    let named = path.file_name().map(|n| n.to_string_lossy()).is_some_and(|n| n.len() > STAGING_PREFIX.len() && n.starts_with(STAGING_PREFIX));
    if !named || crate::scripts::is_network_path(text.trim()) || !is_dir(&path) {
        return Pending::Stale;
    }
    Pending::Apply(path)
}

/// Logged in the app-wide console entry when a script from an imported archive could not be
/// installed (Tandem's; iTerm2 writes the path and the error to its log, `IE:1082`).
pub fn script_import_failed(zip: &Path, error: &str) -> String {
    format!("Import of {} failed: {error}\n", zip.display())
}

/// Import texts (`IE:81-110`, `GP:4089`).
pub const IMPORT_HEADING: &str = "Importing Settings and Data";
pub const IMPORT_BODY: &str = "Any needed Python runtimes will be installed and secure settings will be updated, which may require you to enter your password. Then Tandem will restart and finish importing. This can take several minutes.";
pub const PROBLEM_IMPORTING: &str = "Problem Importing";

/// Erase texts (`IE:122-222`, `GP:4094`).
pub const ERASE_HEADING: &str = "Erase All Settings and Data";
pub const EXPORT_FIRST_BODY: &str = "Would you like to export your settings and data first? You will be able to re-import the exported file later if you change your mind.";
pub const EXPORT_FIRST: &str = "Export First";
pub const SKIP_EXPORT: &str = "Skip Export";
pub const PROBLEM_EXPORTING_ERASE: &str = "Problem Exporting Settings and Data";
pub const DRY_RUN_HEADING: &str = "Dry-Run Erase?";
pub const DRY_RUN_BODY: &str = "Dry-run mode is enabled. Tandem will log to the debug output what it would erase and stay running. Nothing will actually be deleted.";
pub const DRY_RUN_ACTION: &str = "Run Dry Run";
pub const ERASE_CONFIRM_HEADING: &str = "Erase Everything?";
pub const ERASE_ACTION: &str = "Erase Everything and Quit";
pub const DRY_RUN_DONE_HEADING: &str = "Dry Run Complete";
pub const DRY_RUN_DONE_BODY: &str = "Tandem logged what it would have erased to the debug output. Nothing was actually deleted because the \u{201c}Dry-run Erase All Settings and Data\u{201d} advanced setting is enabled.";
pub const PROBLEM_ERASING: &str = "Problem Erasing Settings and Data";

/// "Erase Everything?"'s body (`IE:170-181`): iTerm2's list for what Tandem keeps, and Windows
/// Credential Manager for the Keychain.
pub fn erase_confirm_body() -> String {
    [
        "The following will be erased and Tandem will quit immediately:",
        "",
        "\u{2022} Preferences (profiles, key bindings, arrangements, advanced settings)",
        "\u{2022} Saved windows and sessions",
        "\u{2022} Python API scripts and runtimes",
        "\u{2022} The contents of %APPDATA%\\Tandem",
        "",
        "Items stored in Windows Credential Manager (such as the AI API key and Password Manager entries) are not erased and must be removed manually from Credential Manager if you want them gone too.",
        "",
        "This cannot be undone.",
    ]
    .join("\n")
}

/// The Scripts folder whose entries the erase removes (`Eraser.init`, `IE:246-250`), resolved
/// before anything is removed, since the settings naming a custom folder go with the rest: the
/// custom folder when it is on and named (an empty one is not used, as
/// `scripts::scripts_folder` says), else the default. A network path is never erased (None).
pub fn scripts_to_erase(use_custom: bool, custom: &str, default_folder: &Path) -> Option<PathBuf> {
    if !use_custom || custom.is_empty() {
        return Some(default_folder.to_path_buf());
    }
    (!crate::scripts::is_network_path(custom)).then(|| PathBuf::from(custom))
}

/// What the erase removes, in order (`IE:255-329`): every entry of `%APPDATA%\Tandem`, then
/// every entry of the Scripts folder resolved first. Hidden entries are included and nothing
/// is excluded (a clean slate).
pub fn erase_list(appdata_tandem: &[PathBuf], scripts: Option<(&Path, Vec<PathBuf>)>) -> Vec<PathBuf> {
    appdata_tandem.iter().cloned().chain(scripts.map(|(_, entries)| entries).unwrap_or_default()).collect()
}

/// The dry run's line for one path (`IE:228`).
pub fn dry_run_line(path: &Path) -> String {
    format!("[Erase dry-run] would remove {}", path.display())
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

    #[test]
    fn pending_import_follows_iterm2() {
        let staged = PathBuf::from(r"C:\T\.tandem-import-U");
        let that = |p: &Path| p == Path::new(r"C:\T\.tandem-import-U");
        assert_eq!(pending(None, &that), Pending::None);
        assert_eq!(pending(Some(r"C:\T\.tandem-import-U"), &that), Pending::Apply(staged.clone()));
        assert_eq!(pending(Some(r"C:\T\.tandem-import-U"), &|_| false), Pending::Stale, "the folder is gone");
        assert_eq!(pending(Some(""), &that), Pending::Stale);
        assert_eq!(staging_dir(Path::new(r"C:\T"), "U"), staged);
        assert_eq!(extract_args(Path::new(r"C:\a b.tandemexport"), Path::new(r"C:\T\x")), ["-xzf", r"C:\a b.tandemexport", "-C", r"C:\T\x"]);
        // Beyond the plan: a trailing newline is ignored, and a marker naming a folder that is
        // not a staging folder, or a network path, is never applied (the folder is removed
        // afterwards).
        assert_eq!(pending(Some("C:\\T\\.tandem-import-U\r\n"), &that), Pending::Apply(staged));
        assert_eq!(pending(Some(r"C:\Users\u"), &|_| true), Pending::Stale);
        assert_eq!(pending(Some(r"C:\T\.tandem-import-"), &|_| true), Pending::Stale);
        assert_eq!(pending(Some(r"\\h\s\.tandem-import-U"), &|_| true), Pending::Stale);
    }

    #[test]
    fn erase_plan_follows_iterm2() {
        let d = Path::new(r"C:\A\Scripts");
        assert_eq!(scripts_to_erase(false, "", d), Some(d.to_path_buf()));
        assert_eq!(scripts_to_erase(true, r"E:\s", d), Some(PathBuf::from(r"E:\s")));
        assert_eq!(scripts_to_erase(true, r"\\h\s", d), None);
        assert_eq!(scripts_to_erase(true, "", d), Some(d.to_path_buf()));
        // Beyond the plan: an off custom folder is not used, and `//host` is a network path too.
        assert_eq!(scripts_to_erase(false, r"E:\s", d), Some(d.to_path_buf()));
        assert_eq!(scripts_to_erase(true, "//h/s", d), None);
        let p = |s: &str| PathBuf::from(s);
        assert_eq!(
            erase_list(&[p(r"A\settings.toml"), p(r"A\Scripts")], Some((Path::new(r"E:\s"), vec![p(r"E:\s\a.py")]))),
            [p(r"A\settings.toml"), p(r"A\Scripts"), p(r"E:\s\a.py")]
        );
        assert_eq!(erase_list(&[p(r"A\x")], None), [p(r"A\x")]);
        assert_eq!(dry_run_line(Path::new("A\\x")), "[Erase dry-run] would remove A\\x");
    }

    #[test]
    fn import_erase_texts_follow_iterm2() {
        assert_eq!(MARKER, "import-pending");
        assert_eq!(IMPORT_HEADING, "Importing Settings and Data");
        assert_eq!(
            IMPORT_BODY,
            "Any needed Python runtimes will be installed and secure settings will be updated, which may require you to enter your password. Then Tandem will restart and finish importing. This can take several minutes."
        );
        assert_eq!(PROBLEM_IMPORTING, "Problem Importing");
        assert_eq!(ERASE_HEADING, "Erase All Settings and Data");
        assert_eq!(EXPORT_FIRST_BODY, "Would you like to export your settings and data first? You will be able to re-import the exported file later if you change your mind.");
        assert_eq!((EXPORT_FIRST, SKIP_EXPORT), ("Export First", "Skip Export"));
        assert_eq!(PROBLEM_EXPORTING_ERASE, "Problem Exporting Settings and Data");
        assert_eq!(DRY_RUN_HEADING, "Dry-Run Erase?");
        assert_eq!(DRY_RUN_BODY, "Dry-run mode is enabled. Tandem will log to the debug output what it would erase and stay running. Nothing will actually be deleted.");
        assert_eq!(DRY_RUN_ACTION, "Run Dry Run");
        assert_eq!((ERASE_CONFIRM_HEADING, ERASE_ACTION), ("Erase Everything?", "Erase Everything and Quit"));
        assert_eq!(
            erase_confirm_body(),
            "The following will be erased and Tandem will quit immediately:\n\n\u{2022} Preferences (profiles, key bindings, arrangements, advanced settings)\n\u{2022} Saved windows and sessions\n\u{2022} Python API scripts and runtimes\n\u{2022} The contents of %APPDATA%\\Tandem\n\nItems stored in Windows Credential Manager (such as the AI API key and Password Manager entries) are not erased and must be removed manually from Credential Manager if you want them gone too.\n\nThis cannot be undone."
        );
        assert_eq!(DRY_RUN_DONE_HEADING, "Dry Run Complete");
        assert_eq!(
            DRY_RUN_DONE_BODY,
            "Tandem logged what it would have erased to the debug output. Nothing was actually deleted because the \u{201c}Dry-run Erase All Settings and Data\u{201d} advanced setting is enabled."
        );
        assert_eq!(PROBLEM_ERASING, "Problem Erasing Settings and Data");
        assert_eq!(script_import_failed(Path::new(r"C:\A\script U.zip"), "e"), "Import of C:\\A\\script U.zip failed: e\n");
    }
}
