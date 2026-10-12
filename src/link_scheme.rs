//! Other apps' link schemes in terminal links (`vscode:`, `slack:`; TWEAKS 6, Robert's ruling of
//! 2026-10-12). iTerm2 opens any scheme; Tandem asks once per scheme: Always, Once or Never, with the
//! name of the app Windows has registered for it (never text from the link). A fixed list of schemes
//! is refused whatever the person chose, as is a scheme with no registered app or one whose app is a
//! command interpreter. Pure: the registry lookup, the question and the launch are the App's.

use crate::warnings::{Kind, Spec};

/// Settings: schemes the person chose Always / Never for, lowercase, separated by spaces.
pub const ALWAYS_KEY: &str = "LinkSchemesAlways";
pub const NEVER_KEY: &str = "LinkSchemesNever";

/// Schemes no choice can open: each runs code, reads a local or remote file, or reaches a shell
/// namespace through the handler Windows registers for it. `ms-settings` is here too: it is only
/// reachable from a terminal link as a lure to a security page, and the old guard refused it.
pub const BLOCKED: &[&str] = &[
    "ms-msdt", "msdt", "search-ms", "search", "ms-officecmd", "ms-appinstaller", "ms-cxh", "ms-cxh-full", "ms-settings", "file", "javascript", "vbscript", "data", "shell", "mk",
    "its", "ms-its", "hcp", "ldap", "ldaps", "res", "jar", "smb", "dav", "davs", "webdav", "webdavs", "about", "blob", "filesystem", "view-source", "ms-help", "mhtml",
];

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
    if BLOCKED.contains(&scheme) {
        return refuse();
    }
    // The Office schemes open a document from a URL with the ofe and ofv commands.
    if matches!(scheme, "ms-word" | "ms-excel" | "ms-powerpoint") {
        let lower = url.to_ascii_lowercase().replace("%7c", "|");
        if lower.contains("ofe|") || lower.contains("ofv|") {
            return refuse();
        }
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
        assert!(screen("ms-word", "ms-word:nft|u|https://h/x").is_ok(), "only the open commands are refused");
        assert!(screen("vscode", &format!("vscode:{}", "a".repeat(MAX_URL))).is_err(), "too long");
        assert!(screen("vscode", "vscode:a\nb").is_err(), "control characters");
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
