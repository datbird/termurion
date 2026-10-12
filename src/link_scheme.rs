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

/// Programs that read their argument as script: a scheme whose handler is one is refused.
pub const BAD_HANDLERS: &[&str] = &["cmd", "powershell", "pwsh", "wscript", "cscript", "mshta", "rundll32", "regsvr32", "wsl", "bash", "msiexec", "forfiles", "msbuild"];

/// The longest URL handed to an app.
pub const MAX_URL: usize = 2048;

/// What Windows has registered for a scheme.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Handler {
    /// The app's friendly name (`AssocQueryStringW`).
    pub name: Option<String>,
    /// Its `shell\open\command` line; None when no app is registered.
    pub command: Option<String>,
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

/// The file stem of a registered command's program (`"C:\App\app.exe" "%1"` gives `app`), lowercase.
pub fn program_stem(command: &str) -> Option<String> {
    let c = command.trim();
    let program = match c.strip_prefix('"') {
        Some(rest) => rest.split('"').next()?,
        None => c.split_whitespace().next()?,
    };
    let file = program.rsplit(['\\', '/']).next()?;
    let stem = file.rsplit_once('.').map_or(file, |(s, _)| s);
    (!stem.is_empty()).then(|| stem.to_ascii_lowercase())
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
    let Some(stem) = handler.command.as_deref().and_then(program_stem) else {
        return Plan::Refuse(format!("No app is registered for {scheme}: links."));
    };
    if BAD_HANDLERS.contains(&stem.as_str()) {
        return Plan::Refuse(format!("The app registered for {scheme}: links runs commands, so Tandem does not open them."));
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

    fn app(name: &str, cmd: &str) -> Handler {
        Handler { name: Some(name.into()), command: Some(cmd.into()) }
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
            assert!(matches!(decide(&format!("{s}:x"), &app("A", "\"C:\\a.exe\" \"%1\""), s, ""), Plan::Refuse(_)), "{s}");
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
            assert!(matches!(decide(url, &app("A", "\"C:\\a.exe\" \"%1\""), &s, ""), Plan::Refuse(_)), "{url} even with Always");
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
        for cmd in ["cmd.exe /c %1", "\"C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe\" -c %1", "C:\\Windows\\System32\\mshta.exe \"%1\"", "\"C:\\Windows\\system32\\RUNDLL32.EXE\" x,y %1", "wscript.exe %1"] {
            assert!(matches!(decide("foo:x", &app("X", cmd), "foo", ""), Plan::Refuse(_)), "{cmd}");
        }
        assert_eq!(program_stem("\"C:\\Program Files\\Code\\Code.exe\" --open-url -- \"%1\"").as_deref(), Some("code"));
        assert_eq!(program_stem("C:\\x\\app.exe %1").as_deref(), Some("app"));
    }

    #[test]
    fn ask_once_per_scheme_and_remember() {
        let code = app("Visual Studio Code", "\"C:\\Code.exe\" \"%1\"");
        assert_eq!(decide("vscode://file/x", &code, "", ""), Plan::Ask { app: "Visual Studio Code".into() });
        assert_eq!(decide("vscode://file/x", &code, "slack vscode", ""), Plan::Open);
        assert!(matches!(decide("vscode://file/x", &code, "", "vscode"), Plan::Refuse(_)));
        assert!(matches!(decide("vscode://file/x", &code, "vscode", "vscode"), Plan::Refuse(_)), "Never wins");
        assert_eq!(decide("vscode://file/x", &Handler { name: None, command: Some("\"C:\\Code.exe\" \"%1\"".into()) }, "", ""), Plan::Ask { app: "code".into() }, "the program, never link text");
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
