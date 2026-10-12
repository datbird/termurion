//! Other apps' link schemes in terminal links (`vscode:`, `slack:`; TWEAKS 6, Robert's ruling of
//! 2026-10-12). iTerm2 opens any scheme; Tandem asks once per scheme: Always, Once or Never, with the
//! name of the app Windows has registered for it (never text from the link). A fixed list of schemes
//! is refused whatever the person chose, as is a scheme with no registered app or one whose app is a
//! command interpreter. Pure: the registry lookup, the question and the launch are the App's.

use crate::warnings::{Kind, Spec};

/// Settings: schemes the person chose Always / Never for, lowercase, separated by spaces.
pub const ALWAYS_KEY: &str = "LinkSchemesAlways";
pub const NEVER_KEY: &str = "LinkSchemesNever";

/// Whether `key` is one of the two lists a person's answers fill. They are consent, so they stay on
/// this machine: no custom folder, URL, import or Toggle Setting carries them (TWEAKS 6 review I2).
pub fn stays_local(key: &str) -> bool {
    key == ALWAYS_KEY || key == NEVER_KEY
}

/// Schemes no choice can open, each with its reason: they run code, read a local or remote file, or
/// reach a shell namespace through the handler Windows registers for them. One table (TWEAKS 6 review I3).
pub const BLOCKED: &[(&str, &str)] = &[
    ("ms-msdt", "Follina (CVE-2022-30190): runs a diagnostic tool with a remote payload"),
    ("msdt", "the same diagnostic tool as ms-msdt"),
    ("search-ms", "opens a remote search folder (2022 malware campaigns)"),
    ("search", "the same search folder as search-ms"),
    ("ms-officecmd", "Office argument injection to code execution (2021)"),
    ("ms-appinstaller", "installs a package from a URL (Emotet; disabled by Microsoft)"),
    ("ms-cxh", "Cloud Experience Host: a shell namespace"),
    ("ms-cxh-full", "Cloud Experience Host: a shell namespace"),
    ("ms-settings", "only a lure to a security page from a terminal link"),
    ("file", "a local or network file: Tandem's own file rules apply"),
    ("javascript", "runs script"),
    ("vbscript", "runs script"),
    ("data", "carries its own content"),
    ("shell", "a shell namespace"),
    ("mk", "CHM help content (script)"),
    ("its", "CHM help content (script)"),
    ("ms-its", "CHM help content (script)"),
    ("hcp", "Help and Support Center command execution (CVE-2010-1885)"),
    ("ldap", "a directory lookup that leaves the machine"),
    ("ldaps", "a directory lookup that leaves the machine"),
    ("res", "reads a resource out of a program file"),
    ("jar", "fetches and reads a Java archive"),
    ("smb", "a network share (NTLM leak)"),
    ("dav", "a network share (NTLM leak)"),
    ("davs", "a network share (NTLM leak)"),
    ("webdav", "a network share (NTLM leak)"),
    ("webdavs", "a network share (NTLM leak)"),
    ("about", "a browser internal page"),
    ("blob", "a browser internal object"),
    ("filesystem", "a browser internal file"),
    ("view-source", "a browser internal page"),
    ("ms-help", "help content (script)"),
    ("mhtml", "an archived page that can hold script"),
    // The whole Office family: the abbreviated form (ms-word:https://h/x.docx) and nft| open a remote
    // document or template, and a UNC target leaks NTLM; onenote: opens remote .one files.
    ("ms-word", "Office opens a remote document or template; a UNC target leaks NTLM"),
    ("ms-excel", "Office opens a remote document or template; a UNC target leaks NTLM"),
    ("ms-powerpoint", "Office opens a remote document or template; a UNC target leaks NTLM"),
    ("ms-visio", "Office opens a remote document or template; a UNC target leaks NTLM"),
    ("ms-access", "Office opens a remote document or template; a UNC target leaks NTLM"),
    ("ms-project", "Office opens a remote document or template; a UNC target leaks NTLM"),
    ("ms-publisher", "Office opens a remote document or template; a UNC target leaks NTLM"),
    ("ms-spd", "SharePoint Designer opens a remote site"),
    ("ms-infopath", "InfoPath opens a remote form"),
    ("onenote", "OneNote opens remote .one files (2023 malware wave)"),
    ("ms-officeapp", "the Office app launcher"),
    ("ms-outlook", "Office opens a remote item"),
    ("jnlp", "Java Web Start runs remote code"),
    ("jnlps", "Java Web Start runs remote code"),
    ("microsoft-edge", "hands any URL, file: included, to Edge past Tandem's own browser gates"),
];

/// Schemes Tandem owns or deliberately does not offer: never handed to whatever Windows registered
/// (TWEAKS 6 review I4). ftp, ssh and telnet are `url_schemes::SCHEMES` (Tandem's own, bound per
/// profile); `x-man-page` is macOS only; `iterm2` is the app's internal link scheme; `tandem` is Tandem's own.
pub const OWN: &[&str] = &["ftp", "ssh", "telnet", "x-man-page", "iterm2", "tandem"];

/// Programs that read their argument as script, or that run what they are given: a scheme whose handler
/// is one is refused. Compared by file name stem, lowercase, after the path is resolved.
pub const BAD_HANDLERS: &[&str] = &[
    "cmd", "powershell", "powershell_ise", "pwsh", "wscript", "cscript", "mshta", "rundll32", "regsvr32", "conhost", "wsl", "bash", "sh", "msiexec", "installutil", "msbuild", "forfiles", "hh", "wt",
    "explorer", "msdt", "control", "mmc", "cmstp", "regasm", "regsvcs", "odbcconf", "pcalua", "msxsl", "wmic", "javaws",
    // Language runtimes (a handler like `node.exe -e "%1"` would run the URL as code) and living-off-the-land
    // binaries that fetch, schedule or run what they are given (TWEAKS 6 re-review residuals).
    "node", "python", "pythonw", "py", "pyw", "java", "javaw", "dotnet", "ruby", "perl", "php", "scriptrunner", "syncappvpublishingserver", "bitsadmin", "certutil",
    "presentationhost", "dfsvc", "ieexec", "schtasks", "at", "mavinject", "pcwrun", "runas", "ftp",
];

/// Folders only an administrator can write to, as the path after the drive letter and colon. A program
/// there with no readable version info is trusted; anywhere else it is refused.
const SYSTEM_ROOTS: &[&str] = &["\\program files\\", "\\program files (x86)\\", "\\windows\\"];

/// The longest URL handed to an app.
pub const MAX_URL: usize = 2048;

/// What Windows has registered for a scheme.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Handler {
    /// The app's friendly name (`AssocQueryStringW`).
    pub name: Option<String>,
    /// Its `shell\open\command` line; None when no app is registered.
    pub command: Option<String>,
    /// The program's full path (`ASSOCSTR_EXECUTABLE`, quotes and spaces already handled); None when Windows
    /// cannot name one. The command line is never parsed for the program (TWEAKS 6 review I1).
    pub exe: Option<String>,
    /// OriginalFilename and InternalName from the program's version info; None when unreadable.
    pub version_names: Option<Vec<String>>,
}

/// What to do with a link.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Plan {
    Refuse(String),
    Open,
    Ask { app: String },
}

/// The scheme of `url`, lowercase, when it has the shape of one (a letter, then letters, digits, `+`, `-`, `.`; at most 32).
pub fn scheme_of(url: &str) -> Option<String> {
    let (s, _) = url.split_once(':')?;
    let ok = s.len() > 1 && s.len() <= 32 && s.starts_with(|c: char| c.is_ascii_alphabetic()) && s.chars().all(|c| c.is_ascii_alphanumeric() || "+.-".contains(c));
    ok.then(|| s.to_ascii_lowercase())
}

/// Whether `scheme` (and `url`) can never open.
pub fn screen(scheme: &str, url: &str) -> Result<(), String> {
    let refuse = || Err(format!("Tandem never opens {scheme}: links."));
    if BLOCKED.iter().any(|(s, _)| *s == scheme) {
        return refuse();
    }
    if OWN.contains(&scheme) {
        return Err(format!("Tandem handles {scheme}: links itself and never hands them to another app."));
    }
    if url.len() > MAX_URL || url.chars().any(char::is_control) {
        return Err(format!("Tandem did not open this {scheme}: link (too long, or it holds a control character)."));
    }
    Ok(())
}

/// The lowercase file stem of a resolved executable path (`"C:\App\app.exe"` gives `app`), or why it is
/// refused: not an absolute path (an unexpanded `%ComSpec%`, a bare name), not a `.exe`, or an interpreter.
pub fn exe_stem(exe: &str) -> Result<String, String> {
    let path = exe.trim().trim_matches('"');
    let bytes = path.as_bytes();
    let absolute = bytes.len() > 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && (bytes[2] == b'\\' || bytes[2] == b'/');
    if !absolute || path.contains('%') {
        return Err("Windows did not name the app's program".into());
    }
    let file = path.rsplit(['\\', '/']).next().unwrap_or("").to_ascii_lowercase();
    let Some(stem) = file.strip_suffix(".exe").filter(|s| !s.is_empty()) else {
        return Err("the app is not an .exe program".into());
    };
    if BAD_HANDLERS.contains(&stem) {
        return Err("the app runs commands".into());
    }
    // An 8.3 alias (POWERS~1) that was not resolved to the long name hides the real program from the list.
    if is_short_alias(stem) {
        return Err("the app is named by a short 8.3 alias".into());
    }
    Ok(stem.to_string())
}

/// Whether `stem` has the shape of an 8.3 alias: up to six characters, `~`, digits.
fn is_short_alias(stem: &str) -> bool {
    stem.rsplit_once('~').is_some_and(|(head, tail)| !head.is_empty() && head.len() <= 6 && !tail.is_empty() && tail.chars().all(|c| c.is_ascii_digit()))
}

/// Whether the path (already known to be `X:\...`) is inside an administrator-only folder.
fn in_system_root(exe: &str) -> bool {
    let rest = exe.trim().trim_matches('"').get(2..).unwrap_or("").replace('/', "\\").to_ascii_lowercase();
    SYSTEM_ROOTS.iter().any(|r| rest.starts_with(r))
}

/// The program a version-info name stands for: `Python.EXE` gives `python`, `pwsh.dll` gives `pwsh`.
fn version_stem(name: &str) -> String {
    let name = name.trim().to_ascii_lowercase();
    name.rsplit_once('.').map_or(name.clone(), |(s, _)| s.to_string())
}

/// Judges the program's own version info (OriginalFilename, InternalName), which a rename cannot change.
/// Unreadable info is refused outside the administrator-only folders: every real app ships it, and
/// anything a person could drop into their profile without it is not worth a link.
fn check_version(exe: &str, names: Option<&[String]>) -> Result<(), String> {
    match names {
        Some(names) if names.iter().any(|n| BAD_HANDLERS.contains(&version_stem(n).as_str())) => Err("the app is a renamed command interpreter".into()),
        Some(_) => Ok(()),
        None if in_system_root(exe) => Ok(()),
        None => Err("the app has no readable version information".into()),
    }
}

/// The plan for `url` given the registered app and the person's two lists.
pub fn decide(url: &str, handler: &Handler, always: &str, never: &str) -> Plan {
    let Some(scheme) = scheme_of(url) else { return Plan::Refuse("Tandem opens only links here, not paths.".into()) };
    if let Err(why) = screen(&scheme, url) {
        return Plan::Refuse(why);
    }
    if listed(never, &scheme) {
        return Plan::Refuse(format!("{scheme}: links are set to Never open (Settings > Advanced > Link schemes never opened)."));
    }
    if handler.command.is_none() {
        return Plan::Refuse(format!("No app is registered for {scheme}: links."));
    }
    let stem = match handler.exe.as_deref().map(exe_stem) {
        Some(Ok(stem)) => stem,
        Some(Err(why)) => return Plan::Refuse(format!("Tandem does not open {scheme}: links: {why}.")),
        None => return Plan::Refuse(format!("Tandem does not open {scheme}: links: Windows did not name the app's program.")),
    };
    if let Err(why) = check_version(handler.exe.as_deref().unwrap_or(""), handler.version_names.as_deref()) {
        return Plan::Refuse(format!("Tandem does not open {scheme}: links: {why}."));
    }
    if listed(always, &scheme) {
        return Plan::Open;
    }
    let app = handler.name.as_deref().map(str::trim).filter(|n| !n.is_empty()).map_or(stem, str::to_string);
    Plan::Ask { app }
}

/// Whether the space separated list holds `scheme`.
pub fn listed(list: &str, scheme: &str) -> bool {
    list.split_whitespace().any(|s| s.eq_ignore_ascii_case(scheme))
}

/// `list` with `scheme` added (lowercase, once, sorted).
pub fn added(list: &str, scheme: &str) -> String {
    let mut all: Vec<String> = list.split_whitespace().map(str::to_ascii_lowercase).collect();
    all.push(scheme.to_ascii_lowercase());
    all.sort();
    all.dedup();
    all.join(" ")
}

/// The question: Always, Once, Never, naming the app. Cancel is the closed window; nothing is remembered by the warnings store.
pub fn question(scheme: &str, app: &str) -> Spec {
    Spec {
        id: None,
        kind: Kind::Persistent,
        heading: "Open link?".to_string(),
        text: format!("Open this {scheme}: link with {}?\n\nAlways and Never are remembered for {scheme}: links (Settings > Advanced).", crate::inert_files::shown_name(app)),
        labels: vec!["Always".into(), "Once".into(), "Never".into()],
        cancel: None,
        never_remember: vec![0, 1, 2],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A registered app whose executable (ASSOCSTR_EXECUTABLE) is `exe` and whose command line is `cmd`.
    fn app_cmd(name: &str, exe: &str, cmd: &str) -> Handler {
        Handler { name: Some(name.into()), command: Some(cmd.into()), exe: Some(exe.into()), version_names: Some(vec![]) }
    }

    /// The same app with these OriginalFilename / InternalName values (None = version info unreadable).
    fn with_version(mut h: Handler, names: Option<&[&str]>) -> Handler {
        h.version_names = names.map(|n| n.iter().map(|s| s.to_string()).collect());
        h
    }

    fn app(name: &str, exe: &str) -> Handler {
        app_cmd(name, exe, &format!("\"{exe}\" \"%1\""))
    }

    #[test]
    fn scheme_shape() {
        assert_eq!(scheme_of("VSCode://file/x").as_deref(), Some("vscode"));
        assert_eq!(scheme_of("slack:open").as_deref(), Some("slack"));
        assert_eq!(scheme_of("C:\\x"), None, "a drive letter is a path");
        assert_eq!(scheme_of("1abc:x"), None);
        assert_eq!(scheme_of("no scheme"), None);
        assert_eq!(scheme_of(&format!("{}:x", "a".repeat(40))), None);
    }

    #[test]
    fn the_blocked_list_is_final() {
        for s in ["ms-msdt", "search-ms", "search", "ms-officecmd", "file", "javascript", "vbscript", "data", "shell", "mk", "its", "ms-its", "hcp", "ldap", "res", "jar", "ms-settings", "ms-appinstaller"] {
            assert!(screen(s, &format!("{s}:x")).is_err(), "{s}");
            // Not even Always opens it.
            assert!(matches!(decide(&format!("{s}:x"), &app("A", "C:\\a.exe"), s, ""), Plan::Refuse(_)), "{s}");
        }
        for url in ["ms-word:ofe|u|https://h/x.docx", "MS-EXCEL:ofv|u|https://h/x", "ms-powerpoint:ofe%7Cu%7Chttps://h/x"] {
            assert!(screen(&scheme_of(url).unwrap(), url).is_err(), "{url}");
        }
        assert!(screen("vscode", &format!("vscode:{}", "a".repeat(MAX_URL))).is_err(), "too long");
        assert!(screen("vscode", "vscode:a\nb").is_err(), "control characters");
    }

    #[test]
    fn the_whole_office_family_and_java_web_start_are_blocked() {
        for url in ["ms-word:https://h/x.docx", "ms-word:nft|u|https://h/t.dotm", "ms-excel:ofv|u|https://h/x", "ms-powerpoint:ofe%7Cu%7Chttps://h/x", "ms-visio:https://h/x", "ms-access:https://h/x", "ms-project:https://h/x", "ms-publisher:https://h/x", "ms-spd:https://h/x", "ms-infopath:https://h/x", "onenote:https://h/x.one", "ms-officecmd:x", "ms-officeapp:x", "jnlp://h/x.jnlp", "jnlps://h/x.jnlp", "microsoft-edge:file:///c:/x"] {
            let s = scheme_of(url).unwrap();
            assert!(screen(&s, url).is_err(), "{url}");
            assert!(matches!(decide(url, &app("A", "C:\\a.exe"), &s, ""), Plan::Refuse(_)), "{url} even with Always");
        }
        assert!(BLOCKED.iter().all(|(s, why)| !why.is_empty() && *s == s.to_ascii_lowercase()), "one table, a reason each");
    }

    #[test]
    fn the_choice_lists_stay_local() {
        for key in [ALWAYS_KEY, NEVER_KEY] {
            assert!(!crate::remote_prefs::syncable(key), "{key} syncs");
            assert!(!crate::ai_settings::toggle_allowed(key), "{key} toggles");
        }
        let dir = std::env::temp_dir().join(format!("tandem-linkscheme-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("p.plist");
        std::fs::write(&path, r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict><key>LinkSchemesAlways</key><string>vscode</string><key>LinkSchemesNever</key><string>slack</string><key>FocusFollowsMouse</key><true/></dict></plist>"#).unwrap();
        let got = crate::import::globals_from_file(&path).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        let keys: Vec<&str> = got.iter().map(|(k, _)| k.as_str()).collect();
        assert!(keys.contains(&"FocusFollowsMouse") && !keys.contains(&ALWAYS_KEY) && !keys.contains(&NEVER_KEY), "{keys:?}");
    }

    #[test]
    fn no_handler_and_interpreters_are_refused() {
        let none = Handler::default();
        assert!(matches!(decide("foo:x", &none, "foo", ""), Plan::Refuse(m) if m.contains("foo")));
        // A command with no resolved executable is refused too.
        let unresolved = Handler { name: None, command: Some("\"C:\\a.exe\" \"%1\"".into()), exe: None, version_names: None };
        assert!(matches!(decide("foo:x", &unresolved, "foo", ""), Plan::Refuse(_)));
        for exe in [
            "C:\\Windows\\System32\\cmd.exe",
            "C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe",
            "C:\\Program Files\\PowerShell\\7\\pwsh.exe",
            "C:\\Windows\\System32\\mshta.exe",
            "C:\\Windows\\system32\\RUNDLL32.EXE",
            "C:\\Windows\\System32\\wscript.exe",
            "C:\\Windows\\System32\\cscript.exe",
            "C:\\Windows\\System32\\regsvr32.exe",
            "C:\\Windows\\System32\\conhost.exe",
            "C:\\Windows\\System32\\wsl.exe",
            "C:\\Windows\\System32\\bash.exe",
            "C:\\Windows\\System32\\msiexec.exe",
            "C:\\Windows\\Microsoft.NET\\Framework64\\v4.0.30319\\InstallUtil.exe",
            "C:\\Windows\\Microsoft.NET\\Framework64\\v4.0.30319\\MSBuild.exe",
            "C:\\Windows\\System32\\forfiles.exe",
            "C:\\Windows\\hh.exe",
        ] {
            assert!(matches!(decide("foo:x", &app("X", exe), "foo", ""), Plan::Refuse(_)), "{exe}");
        }
        // Only a real .exe: scripts, shortcuts and the old executable types are refused.
        for exe in ["C:\\x\\h.bat", "C:\\x\\h.cmd", "C:\\x\\h.com", "C:\\x\\h.pif", "C:\\x\\h.scr", "C:\\x\\h.vbs", "C:\\x\\h.js", "C:\\x\\h.ps1", "C:\\x\\h.lnk", "C:\\x\\h", "C:\\x\\h.exe.bat"] {
            assert!(matches!(decide("foo:x", &app("X", exe), "foo", ""), Plan::Refuse(_)), "{exe}");
        }
        // The cases the command line gets wrong: a .bat run through cmd, an unquoted path with spaces,
        // an unexpanded %ComSpec%.
        assert!(matches!(decide("foo:x", &app_cmd("X", "C:\\x\\h.bat", "\"C:\\x\\h.bat\" \"%1\""), "foo", ""), Plan::Refuse(_)));
        assert!(matches!(decide("foo:x", &app_cmd("X", "C:\\Program Files\\PowerShell\\7\\pwsh.exe", "C:\\Program Files\\PowerShell\\7\\pwsh.exe -c %1"), "foo", ""), Plan::Refuse(_)));
        assert!(matches!(decide("foo:x", &app_cmd("X", "%ComSpec%", "%ComSpec% /c %1"), "foo", ""), Plan::Refuse(_)));
        assert!(matches!(decide("foo:x", &app("X", "cmd.exe"), "foo", ""), Plan::Refuse(_)), "a bare name is not a resolved path");
        // A normal app, in a folder with spaces and a quoted path, is asked about.
        assert_eq!(exe_stem("\"C:\\Program Files\\Code\\Code.exe\"").as_deref(), Ok("code"));
        assert_eq!(exe_stem("C:\\x\\App.EXE").as_deref(), Ok("app"));
    }

    #[test]
    fn runtimes_and_lolbins_are_refused() {
        for stem in [
            "node", "python", "pythonw", "py", "pyw", "java", "javaw", "dotnet", "ruby", "perl", "php", "scriptrunner", "syncappvpublishingserver", "pcalua", "bitsadmin", "certutil", "cmstp", "odbcconf", "regasm", "regsvcs",
            "installutil", "presentationhost", "dfsvc", "ieexec", "wmic", "schtasks", "at", "mavinject", "control", "pcwrun", "runas", "ftp",
        ] {
            let exe = format!("C:\\Tools\\{stem}.exe");
            assert!(matches!(decide("foo:x", &app("X", &exe), "foo", ""), Plan::Refuse(_)), "{exe}");
        }
    }

    #[test]
    fn a_short_8_3_name_is_refused_when_it_was_not_resolved() {
        for exe in ["C:\\PROGRA~1\\POWERS~1\\7\\POWERS~1.EXE", "C:\\x\\PYTHON~1.EXE", "C:\\x\\NODE~2.exe"] {
            assert!(matches!(decide("foo:x", &app("X", exe), "foo", ""), Plan::Refuse(_)), "{exe}");
        }
        // A tilde that is not an 8.3 alias is an ordinary name.
        assert_eq!(exe_stem("C:\\x\\my~app.exe").as_deref(), Ok("my~app"));
    }

    #[test]
    fn a_renamed_interpreter_is_caught_by_its_version_info() {
        let renamed = |names: &[&str]| with_version(app("X", "C:\\Users\\u\\AppData\\Local\\Tool\\helper.exe"), Some(names));
        for names in [&["python.exe", "python"][..], &["POWERSHELL.EXE"], &["", "cmd"], &["pwsh.dll"], &["Node.exe"], &["wscript.exe", "x"]] {
            assert!(matches!(decide("foo:x", &renamed(names), "foo", ""), Plan::Refuse(_)), "{names:?}");
        }
        // Real apps keep working: Electron apps and their ordinary version strings.
        for names in [&["Code.exe", "Code"][..], &["slack.exe", "Slack"], &["ms-teams.exe", "Teams"], &[]] {
            assert!(matches!(decide("foo:x", &renamed(names), "", ""), Plan::Ask { .. }), "{names:?}");
        }
    }

    #[test]
    fn unreadable_version_info_is_refused_outside_the_system_folders() {
        let unreadable = |exe: &str| with_version(app("X", exe), None);
        for exe in ["C:\\Users\\u\\AppData\\Local\\Tool\\t.exe", "D:\\tools\\t.exe", "C:\\ProgramData\\t\\t.exe", "C:\\Program Files Evil\\t.exe", "C:\\Windows.old\\t.exe"] {
            assert!(matches!(decide("foo:x", &unreadable(exe), "foo", ""), Plan::Refuse(_)), "{exe}");
        }
        for exe in ["C:\\Program Files\\T\\t.exe", "c:\\program files (x86)\\T\\t.exe", "C:\\Windows\\System32\\t.exe"] {
            assert!(matches!(decide("foo:x", &unreadable(exe), "", ""), Plan::Ask { .. }), "{exe}");
        }
    }

    #[test]
    fn ask_once_per_scheme_and_remember() {
        let code = app("Visual Studio Code", "C:\\Code.exe");
        assert_eq!(decide("vscode://file/x", &code, "", ""), Plan::Ask { app: "Visual Studio Code".into() });
        assert_eq!(decide("vscode://file/x", &code, "slack vscode", ""), Plan::Open);
        assert!(matches!(decide("vscode://file/x", &code, "", "vscode"), Plan::Refuse(_)));
        assert!(matches!(decide("vscode://file/x", &code, "vscode", "vscode"), Plan::Refuse(_)), "Never wins");
        assert_eq!(decide("vscode://file/x", &Handler { name: None, command: Some("\"C:\\Code.exe\" \"%1\"".into()), exe: Some("C:\\Code.exe".into()), version_names: Some(vec![]) }, "", ""), Plan::Ask { app: "code".into() }, "the program, never link text");
        assert!(listed("a  VSCode b", "vscode"));
        assert!(!listed("vscode2", "vscode"));
        assert_eq!(added("slack", "VSCode"), "slack vscode");
        assert_eq!(added("slack vscode", "vscode"), "slack vscode");
    }

    #[test]
    fn the_question_names_the_app_not_the_link() {
        let q = question("vscode", "Visual\u{202e} Studio\u{200b} Code");
        assert_eq!(q.labels, ["Always", "Once", "Never"]);
        assert!(q.text.contains("Visual Studio Code") && !q.text.contains('\u{202e}') && !q.text.contains('\u{200b}'));
        assert_eq!(q.id, None);
        assert_eq!(q.cancel, None);
    }
}
