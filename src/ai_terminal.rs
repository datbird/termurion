//! The read-only terminal tools a chat's model can call (`PTYSession.swift`
//! `*RemoteCommand`, `PS:502-783`), answered from a `Facts` snapshot the App takes of the
//! linked session. Pure: the texts, the history JSON, the screen kind, the line clamp, the
//! compact age and the one detector of the session's shell.

use crate::ai_tools::Call;

/// `getCommandOutputRemoteCommand`'s `cappedAtSize` (`PS:717`): the output's last characters.
pub const OUTPUT_CAP: usize = 16384;
/// `get_screen_contents`' lines when the model asks for none (`WI:329`).
pub const DEFAULT_SCREEN_LINES: i64 = 100;
/// The most lines `get_screen_contents` returns (`trailingTranscript`, `WI:648-652`).
pub const MAX_SCREEN_LINES: i64 = 2000;

pub const SI_MISSING: &str = "Tell the user they need to install Shell Integration to access this information.";
pub const NOT_AT_PROMPT: &str = "The user is not at the prompt.";
/// iTerm2's text as written ("command", sic).
pub const NO_PREVIOUS: &str = "There are no previous command in history";
pub const STILL_RUNNING: &str = "The last command is still running and has not exited yet.";
pub const OUTPUT_GONE: &str = "The output is no longer available.";
pub const UNKNOWN_HOST: &str = "Unknown";

/// What the read-only tools read from the linked session, taken by the App (`ai_facts`)
/// with only what the call needs.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Facts {
    /// Shell integration has ever fed the command history (iTerm2's flag is global too).
    pub ever_used: bool,
    /// At a shell prompt (`currentCommand != nil`).
    pub at_prompt: bool,
    /// The command being typed (`currentCommand`).
    pub typed: Option<String>,
    /// The command typed up to the cursor (`currentCommandUpToCursor`).
    pub typed_to_cursor: Option<String>,
    /// The last command mark's command.
    pub last_command: Option<String>,
    /// `Screen::last_exit`.
    pub last_exit: Option<Option<i32>>,
    /// The prompt marks with a command, oldest first.
    pub history: Vec<(u32, String)>,
    /// The output `get_command_output` asked for.
    pub output: Option<String>,
    /// The screen JSON `get_screen_contents` asked for.
    pub screen: Option<serde_json::Value>,
    pub columns: usize,
    pub rows: usize,
    /// The `shell` session variable ("" when unknown).
    pub shell: String,
    /// The host shell integration reported.
    pub host: Option<String>,
    /// The session is on this computer (`isLocalhost`).
    pub local: bool,
    /// The user shell integration reported.
    pub user: Option<String>,
    /// This computer's user (`NSUserName`: `%USERNAME%`).
    pub local_user: String,
    /// The `path` session variable.
    pub path: String,
}

/// A tool's answer: the text the model reads and the notice the chat shows.
#[derive(Clone, Debug, PartialEq)]
pub struct Outcome {
    pub model: String,
    pub notice: Option<String>,
}

fn outcome(model: impl Into<String>, notice: &str) -> Option<Outcome> {
    Some(Outcome { model: model.into(), notice: Some(notice.to_string()) })
}

/// The fourteen read-only tools (`PS:502-783`); None for any other call. Six need shell
/// integration to have been used; `get_command_output` does not check (iTerm2).
pub fn answer(call: &Call, f: &Facts) -> Option<Outcome> {
    let si_missing = |notice: &str| if f.ever_used { None } else { outcome(SI_MISSING, notice) };
    match call {
        Call::IsAtPrompt => si_missing("Failed to check if you\u{2019}re at the prompt because Shell Integration is not installed.").or_else(|| {
            let (answer, word) = if f.at_prompt { ("true", "at") } else { ("false", "not at") };
            outcome(answer, &format!("Notified AI that you are {word} the a prompt."))
        }),
        Call::GetLastExitStatus => si_missing("Failed to get the last command\u{2019}s exit status because shell integration is not installed.").or_else(|| match f.last_exit {
            Some(Some(code)) => outcome(code.to_string(), "The exit status of the last command provided to AI."),
            // No mark reads as no code (`PS:607-611`).
            _ => outcome(STILL_RUNNING, "The last command seems to still be running so its exit status cannot be retrieved."),
        }),
        Call::GetCommandHistory { limit } => si_missing("Command history isn\u{2019}t available because Shell Integration isn\u{2019}t installed.").or_else(|| {
            // Any whole number reaches here; below 0 stops at once, as `entries.count >= limit`.
            let limit = usize::try_from((*limit).max(0)).unwrap_or(usize::MAX);
            outcome(history_json(&f.history, Some(limit), None), "Command history provided to AI.")
        }),
        Call::GetLastCommand => si_missing("The last command execute can\u{2019}t be provided because Shell Integration isn\u{2019}t installed.").or_else(|| match &f.last_command {
            Some(c) => outcome(c.clone(), "Last command provided to AI."),
            None => outcome(NO_PREVIOUS, "No previous command is available in this session}s history."),
        }),
        Call::GetCommandBeforeCursor => si_missing("The contents of the shell prompt could not be determined because Shell Integration isn\u{2019}t installed.").or_else(|| match &f.typed_to_cursor {
            Some(t) => outcome(t.clone(), "Current command provided to AI."),
            None => outcome(NOT_AT_PROMPT, "The contents of the shell prompt could not be provided because it appears the session is not currently at a prompt."),
        }),
        Call::SearchCommandHistory { query } => si_missing("Command history is not available because Shell Integration isn\u{2019}t installed.")
            .or_else(|| outcome(history_json(&f.history, None, Some(query)), "Command history provided to AI.")),
        Call::GetCommandOutput { .. } => match &f.output {
            Some(o) => outcome(o.clone(), "Command output provided to AI."),
            None => outcome(OUTPUT_GONE, "The command output wasn\u{2019}t provided because it is no longer available."),
        },
        Call::GetScreenContents { .. } => outcome(f.screen.as_ref().map_or_else(|| "{}".to_string(), |v| v.to_string()), "Screen contents provided to AI."),
        Call::GetTerminalSize => outcome(format!("{} columns wide by {} rows tall", f.columns, f.rows), "Session size provided to AI."),
        Call::GetShellType => outcome(f.shell.clone(), "Shell type provided to AI"),
        // Ruling 10: true for a remote host (iTerm2 answers the inverse, `PS:761-765`).
        Call::DetectSshSession => outcome((!f.local).to_string(), "SSH status provided to AI."),
        Call::GetRemoteHostname => outcome(f.host.clone().unwrap_or_else(|| UNKNOWN_HOST.to_string()), "Hostname provided to AI"),
        Call::GetUserIdentity => outcome(f.user.clone().unwrap_or_else(|| f.local_user.clone()), "User name provided to AI."),
        Call::GetCurrentDirectory => outcome(f.path.clone(), "Current directory provided to AI."),
        _ => None,
    }
}

/// The history as the model reads it (`PS:613-676`): `[{"command", "id"}]`, oldest first.
/// A search keeps the commands holding `query` (case-sensitive) with no limit; otherwise the
/// list stops at `limit`, keeping the oldest.
pub fn history_json(entries: &[(u32, String)], limit: Option<usize>, query: Option<&str>) -> String {
    let list: Vec<serde_json::Value> = entries
        .iter()
        .filter(|(_, c)| query.is_none_or(|q| c.contains(q)))
        .take(if query.is_some() { usize::MAX } else { limit.unwrap_or(usize::MAX) })
        .map(|(id, c)| serde_json::json!({"command": c, "id": id.to_string()}))
        .collect();
    serde_json::Value::Array(list).to_string()
}

/// The screen's kind (`WorkgroupIntrospection.kind`): "claude-code" while claude runs in the
/// session, else "tui" on the alternate screen, else "other" (iTerm2 never answers "shell").
pub fn kind_of(foreground: Option<&str>, alternate: bool) -> &'static str {
    if foreground.is_some_and(|f| f.eq_ignore_ascii_case("claude")) {
        "claude-code"
    } else if alternate {
        "tui"
    } else {
        "other"
    }
}

/// The lines `get_screen_contents` reads: below 1 means the default (`lines > 0 ? lines :
/// nil`, `PS:745`), at most `MAX_SCREEN_LINES`.
pub fn clamp_lines(requested: i64) -> i64 {
    if requested > 0 { requested.min(MAX_SCREEN_LINES) } else { DEFAULT_SCREEN_LINES }
}

/// `compactDateDifferenceStringFromTimeDelta` (`NDF:230-292`, the English path): an age in
/// seconds as "< 1 min", "N min", "1 hour", "N hrs", "1 day", "N days", "1 week", "N wks",
/// rounding half away from zero. A value that is not finite is "" (iTerm2's localized path;
/// its English path would convert it to an integer).
pub fn compact_age(secs: f64) -> String {
    if !secs.is_finite() {
        return String::new();
    }
    let round = |unit: f64| (secs / unit).round() as i64;
    if secs < 60.0 {
        "< 1 min".to_string()
    } else if secs < 3600.0 {
        match round(60.0) {
            1 => "1 min".to_string(),
            n => format!("{n} min"),
        }
    } else if secs < 86400.0 {
        match round(3600.0) {
            1 => "1 hour".to_string(),
            n => format!("{n} hrs"),
        }
    } else if secs < 604800.0 {
        match round(86400.0) {
            1 => "1 day".to_string(),
            7 => "1 week".to_string(),
            n => format!("{n} days"),
        }
    } else {
        match round(604800.0) {
            1 => "1 week".to_string(),
            n => format!("{n} wks"),
        }
    }
}

/// The `shell` session variable, the one answer to which shell a session runs (ruling I-1;
/// `ai_shell` classifies this answer, never a second detection). iTerm2's precedence
/// (`screenDidDetectShell`, `bestGuessAtUserShellWithPath:`, `PS:19408-19425`): the shell
/// shell integration reported, else the program the session launched (iTerm2's custom shell or
/// login shell), its file name without ".exe". A remote session with no reported shell has
/// none: the local program says nothing about the remote shell (iTerm2 keeps its local guess).
pub fn detected_shell(shell: Option<&str>, launch_program: Option<&str>, local: bool) -> Option<String> {
    if let Some(s) = shell.map(str::trim).filter(|s| !s.is_empty()) {
        return Some(s.to_string());
    }
    if !local {
        return None;
    }
    let program = launch_program?.trim();
    let name = crate::proctree::strip_exe(program.rsplit(['\\', '/']).next().unwrap_or(program));
    (!name.is_empty()).then(|| name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts() -> Facts {
        Facts { columns: 80, rows: 24, local_user: "me".into(), ..Default::default() }
    }

    fn ask(call: Call, f: &Facts) -> (String, Option<String>) {
        let o = answer(&call, f).expect("a read-only tool");
        (o.model, o.notice)
    }

    fn pair(model: &str, notice: &str) -> (String, Option<String>) {
        (model.to_string(), Some(notice.to_string()))
    }

    #[test]
    fn read_tools_follow_iterm2() {
        // Without Shell Integration ever used, six tools answer the install text.
        let f = facts();
        assert_eq!(SI_MISSING, "Tell the user they need to install Shell Integration to access this information.");
        assert_eq!(ask(Call::IsAtPrompt, &f), pair(SI_MISSING, "Failed to check if you\u{2019}re at the prompt because Shell Integration is not installed."));
        assert_eq!(ask(Call::GetLastExitStatus, &f), pair(SI_MISSING, "Failed to get the last command\u{2019}s exit status because shell integration is not installed."));
        assert_eq!(ask(Call::GetCommandHistory { limit: 5 }, &f), pair(SI_MISSING, "Command history isn\u{2019}t available because Shell Integration isn\u{2019}t installed."));
        assert_eq!(ask(Call::GetLastCommand, &f), pair(SI_MISSING, "The last command execute can\u{2019}t be provided because Shell Integration isn\u{2019}t installed."));
        assert_eq!(ask(Call::GetCommandBeforeCursor, &f), pair(SI_MISSING, "The contents of the shell prompt could not be determined because Shell Integration isn\u{2019}t installed."));
        assert_eq!(ask(Call::SearchCommandHistory { query: "x".into() }, &f), pair(SI_MISSING, "Command history is not available because Shell Integration isn\u{2019}t installed."));
        // get_command_output does not check (iTerm2).
        let with_output = Facts { output: Some("out".into()), ..facts() };
        assert_eq!(ask(Call::GetCommandOutput { id: String::new() }, &with_output), pair("out", "Command output provided to AI."));

        let si = Facts { ever_used: true, at_prompt: true, ..facts() };
        assert_eq!(ask(Call::IsAtPrompt, &si), pair("true", "Notified AI that you are at the a prompt."));
        assert_eq!(ask(Call::IsAtPrompt, &Facts { at_prompt: false, ..si.clone() }), pair("false", "Notified AI that you are not at the a prompt."));
        assert_eq!(ask(Call::GetLastExitStatus, &Facts { last_exit: Some(Some(2)), ..si.clone() }), pair("2", "The exit status of the last command provided to AI."));
        assert_eq!(STILL_RUNNING, "The last command is still running and has not exited yet.");
        let running = pair(STILL_RUNNING, "The last command seems to still be running so its exit status cannot be retrieved.");
        assert_eq!(ask(Call::GetLastExitStatus, &Facts { last_exit: Some(None), ..si.clone() }), running);
        assert_eq!(ask(Call::GetLastExitStatus, &Facts { last_exit: None, ..si.clone() }), running, "no mark reads as no code");
        assert_eq!(NO_PREVIOUS, "There are no previous command in history");
        assert_eq!(ask(Call::GetLastCommand, &si), pair(NO_PREVIOUS, "No previous command is available in this session}s history."));
        assert_eq!(ask(Call::GetLastCommand, &Facts { last_command: Some("ls".into()), ..si.clone() }), pair("ls", "Last command provided to AI."));
        assert_eq!(NOT_AT_PROMPT, "The user is not at the prompt.");
        assert_eq!(
            ask(Call::GetCommandBeforeCursor, &si),
            pair(NOT_AT_PROMPT, "The contents of the shell prompt could not be provided because it appears the session is not currently at a prompt.")
        );
        assert_eq!(ask(Call::GetCommandBeforeCursor, &Facts { typed_to_cursor: Some("git st".into()), ..si.clone() }), pair("git st", "Current command provided to AI."));
        let history = Facts { history: vec![(1, "ls".into()), (2, "cd x".into())], ..si.clone() };
        assert_eq!(ask(Call::GetCommandHistory { limit: 1 }, &history), pair(r#"[{"command":"ls","id":"1"}]"#, "Command history provided to AI."));
        assert_eq!(ask(Call::SearchCommandHistory { query: "x".into() }, &history), pair(r#"[{"command":"cd x","id":"2"}]"#, "Command history provided to AI."));
        // A model-chosen limit is any whole number (Task 4 review Minor 2).
        assert_eq!(ask(Call::GetCommandHistory { limit: -1 }, &history).0, "[]");
        assert_eq!(ask(Call::GetCommandHistory { limit: i64::MAX }, &history).0, r#"[{"command":"ls","id":"1"},{"command":"cd x","id":"2"}]"#);
        assert_eq!(ask(Call::GetTerminalSize, &si), pair("80 columns wide by 24 rows tall", "Session size provided to AI."));
        assert_eq!(ask(Call::GetShellType, &si), pair("", "Shell type provided to AI"));
        assert_eq!(ask(Call::GetShellType, &Facts { shell: "pwsh".into(), ..si.clone() }), pair("pwsh", "Shell type provided to AI"));
        assert_eq!(UNKNOWN_HOST, "Unknown");
        assert_eq!(ask(Call::GetRemoteHostname, &si), pair(UNKNOWN_HOST, "Hostname provided to AI"));
        assert_eq!(ask(Call::GetRemoteHostname, &Facts { host: Some("box".into()), ..si.clone() }), pair("box", "Hostname provided to AI"));
        assert_eq!(ask(Call::GetUserIdentity, &si), pair("me", "User name provided to AI."));
        assert_eq!(ask(Call::GetUserIdentity, &Facts { user: Some("root".into()), ..si.clone() }), pair("root", "User name provided to AI."));
        assert_eq!(ask(Call::GetCurrentDirectory, &Facts { path: r"C:\w".into(), ..si.clone() }), pair(r"C:\w", "Current directory provided to AI."));
        assert_eq!(OUTPUT_GONE, "The output is no longer available.");
        assert_eq!(ask(Call::GetCommandOutput { id: "3".into() }, &si), pair(OUTPUT_GONE, "The command output wasn\u{2019}t provided because it is no longer available."));
        let screen = serde_json::json!({"screen_contents": {"text": "a"}});
        assert_eq!(ask(Call::GetScreenContents { lines: 0 }, &Facts { screen: Some(screen), ..si.clone() }), pair(r#"{"screen_contents":{"text":"a"}}"#, "Screen contents provided to AI."));
        // Not a read-only tool.
        assert_eq!(answer(&Call::SetClipboard { text: "x".into() }, &si), None);
        assert_eq!(answer(&Call::ExecuteCommand { command: "ls".into() }, &si), None);
        assert_eq!(answer(&Call::RestartSession, &si), None);
    }

    #[test]
    fn shell_variable_follows_iterm2() {
        assert_eq!(detected_shell(Some("pwsh"), Some("C:\\Windows\\System32\\cmd.exe"), true), Some("pwsh".to_string()));
        assert_eq!(detected_shell(None, Some("C:\\Program Files\\PowerShell\\7\\pwsh.exe"), true), Some("pwsh".to_string()));
        assert_eq!(detected_shell(None, Some("cmd"), true), Some("cmd".to_string()));
        assert_eq!(detected_shell(None, None, true), None);
        assert_eq!(detected_shell(Some("bash"), None, false), Some("bash".to_string()));
        // A remote session never takes a shell name from the local launch program.
        assert_eq!(detected_shell(None, Some("ssh.exe"), false), None);
        // An empty report is no report.
        assert_eq!(detected_shell(Some(""), Some("cmd.exe"), true), Some("cmd".to_string()));
    }

    #[test]
    fn ssh_answer_is_fixed() {
        // iTerm2 answers the inverse (`PS:761-765`); ruling 10 fixes it.
        let remote = Facts { local: false, ..facts() };
        assert_eq!(ask(Call::DetectSshSession, &remote), pair("true", "SSH status provided to AI."));
        let local = Facts { local: true, ..facts() };
        assert_eq!(ask(Call::DetectSshSession, &local), pair("false", "SSH status provided to AI."));
    }

    #[test]
    fn history_json_follows_iterm2() {
        let entries = [(1, "ls".to_string()), (2, "cd x".to_string())];
        assert_eq!(history_json(&entries, Some(1), None), r#"[{"command":"ls","id":"1"}]"#, "the oldest entries, up to the limit");
        assert_eq!(history_json(&entries, None, None), r#"[{"command":"ls","id":"1"},{"command":"cd x","id":"2"}]"#);
        assert_eq!(history_json(&entries, Some(0), None), "[]");
        assert_eq!(history_json(&entries, Some(1), Some("x")), r#"[{"command":"cd x","id":"2"}]"#, "a search has no limit");
        assert_eq!(history_json(&entries, None, Some("X")), "[]", "case-sensitive");
        assert_eq!(history_json(&[(7, "echo \"hi\"".to_string())], None, None), r#"[{"command":"echo \"hi\"","id":"7"}]"#);
        assert_eq!(history_json(&[], None, None), "[]");
    }

    #[test]
    fn compact_age_follows_iterm2() {
        for (secs, text) in [
            (0.0, "< 1 min"),
            (30.0, "< 1 min"),
            (59.9, "< 1 min"),
            (-5.0, "< 1 min"),
            (60.0, "1 min"),
            (89.0, "1 min"),
            (90.0, "2 min"),
            (3599.0, "60 min"),
            (3600.0, "1 hour"),
            (5399.0, "1 hour"),
            (5400.0, "2 hrs"),
            (86399.0, "24 hrs"),
            (86400.0, "1 day"),
            (129600.0, "2 days"),
            (561600.0, "1 week"),
            (604800.0, "1 week"),
            (907200.0, "2 wks"),
            (1814400.0, "3 wks"),
            (f64::NAN, ""),
            (f64::INFINITY, ""),
            (f64::NEG_INFINITY, ""),
        ] {
            assert_eq!(compact_age(secs), text, "{secs}");
        }
    }

    #[test]
    fn kinds_follow_iterm2() {
        assert_eq!(kind_of(Some("claude"), false), "claude-code");
        assert_eq!(kind_of(Some("claude"), true), "claude-code");
        assert_eq!(kind_of(Some("Claude"), false), "claude-code");
        assert_eq!(kind_of(None, true), "tui");
        assert_eq!(kind_of(Some("bash"), false), "other");
        assert_eq!(kind_of(None, false), "other");
        assert_eq!(clamp_lines(0), 100);
        assert_eq!(clamp_lines(5), 5);
        // iTerm2 reads any lines below 1 as the default (`lines > 0 ? lines : nil`, `PS:745`).
        assert_eq!(clamp_lines(-3), 100);
        assert_eq!(clamp_lines(9999), 2000);
        assert_eq!(clamp_lines(i64::MAX), 2000);
        assert_eq!((DEFAULT_SCREEN_LINES, MAX_SCREEN_LINES, OUTPUT_CAP), (100, 2000, 16384));
    }
}
