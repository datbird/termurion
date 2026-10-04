//! The Claude Code integration's pure half (`OB`, `MC`, `HM`, `CWT`): the settings.json hook
//! merge and strip, the health checks, the menu rule, the steps and every text. The person's
//! `settings.json` is theirs: a file that cannot be read or parsed is never written, only
//! Tandem's own cc-status entries are touched, a backup is kept, and the write is a temp file
//! renamed over the old one. Every path is passed in, so a test never reaches the real
//! `%USERPROFILE%\.claude`.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

/// The hook events, in iTerm2's order (`OB:1595`).
pub const HOOK_EVENTS: [&str; 10] = ["UserPromptSubmit", "Stop", "StopFailure", "Notification", "PermissionRequest", "SessionEnd", "PreToolUse", "PostToolUse", "SessionStart", "SubagentStop"];
/// The hook program, beside tandem.exe (`src/bin/cc-status.rs`).
pub const EXE: &str = "cc-status.exe";
/// iTerm2's `NoSyncClaudeCodeIntegrationCompleted`.
pub const COMPLETED_KEY: &str = "NoSyncClaudeCodeIntegrationCompleted";
/// The folder the hook was last installed in (iTerm2's config dir path).
pub const DIR_KEY: &str = "NoSyncClaudeCodeConfigDirPath";
/// The hooks cache: set by a successful install, cleared by an uninstall.
pub const HOOKS_KEY: &str = "NoSyncClaudeCodeHooksInstalled";
/// Set by "_Yes" and "Never" on the announcement.
pub const WATCHER_OFF_KEY: &str = "NoSyncDisableClaudeWatcher";
/// The repair prompt's silence identifier.
pub const REPAIR_ID: &str = "NoSyncSuppressClaudeCodeIntegrationRepairPrompt";
/// The announcement bar's identifier (`ClaudeCodeStatusTool`).
pub const WATCHER_BAR: &str = "ClaudeCodeStatusTool";
/// The announcement shows when this many sessions run claude.
pub const WATCHER_THRESHOLD: usize = 3;
pub const HELP_URL: &str = "https://iterm2.com/claude-code-integration.html";

// ---- texts ("Claude Code texts")
pub const INTRO_TITLE: &str = "Before You Start";
pub const INTRO_LEAD: &str = "Don\u{2019}t panic! All of this can be undone later via Tandem > Uninstall Claude Code Integration.";
pub const WHAT_CHANGES: &str = "What gets changed";
pub const LEARN_MORE: &str = "Learn more about the Claude Code integration";
pub const CONTINUE: &str = "Continue";
pub const PANEL_TITLE: &str = "Claude Code Integration Setup";
pub const NEXT: &str = "Next";
pub const DO_IT: &str = "Do It";
pub const BACK: &str = "Back";
pub const CLOSE: &str = "Close";
pub const HOOK_FAILED: &str = "Failed to install hook";
pub const UNINSTALL_HEADING: &str = "Uninstall Claude Code Integration?";
pub const UNINSTALL: &str = "Uninstall";
pub const REMOVE_FAILED_HEADING: &str = "Couldn\u{2019}t Remove Hooks";
pub const CONTINUE_BUTTON: &str = "Continue";
pub const DISABLE_API_HEADING: &str = "Disable the Python API?";
pub const DISABLE_API_TEXT: &str = "The installer enabled Tandem\u{2019}s Python API. Other scripts or integrations may be using it now. Leave it enabled, or turn it off?";
pub const LEAVE_ENABLED: &str = "Leave Enabled";
pub const DISABLE: &str = "Disable";
pub const BROKEN_HEADING: &str = "Claude Code Integration Looks Broken";
pub const REINSTALL: &str = "Reinstall";
pub const NOT_NOW: &str = "Not Now";
pub const WATCHER_TEXT: &str = "Want to try Tandem\u{2019}s Claude Code integration?";
pub const WATCHER_BUTTONS: [&str; 3] = ["_Yes", "Never", "Ask Later"];

/// The settings the person is asked about: "\u{2022} ..." lines.
pub fn bullets(settings: &Path) -> String {
    format!("\u{2022} Tandem\u{2019}s Python API is enabled\n\u{2022} A cc-status hook is added to {}\n\u{2022} The toolbelt is shown", settings.display())
}

pub fn write_failed(path: &Path, e: impl std::fmt::Display) -> String {
    format!("Could not write to {}: {e}", path.display())
}

pub fn read_failed(path: &Path, e: impl std::fmt::Display) -> String {
    format!("Could not read {}: {e}", path.display())
}

pub fn not_json(path: &Path) -> String {
    format!("{} couldn\u{2019}t be parsed as JSON. Open it in a text editor and check it for syntax errors.", path.display())
}

/// `%USERPROFILE%` is not set, so Claude Code's folder cannot be found.
pub const NO_PROFILE: &str = "Could not find your user folder (USERPROFILE is not set).";

/// The hook program is not beside tandem.exe: a hook pointing at nothing would fail at every
/// Claude Code event.
pub fn missing_exe(exe: &Path) -> String {
    format!("{} was not found. Reinstall Tandem, then try again.", exe.display())
}

pub fn uninstall_text(path: &Path) -> String {
    format!("This removes the cc-status hook from {}. You can reinstall any time using Tandem > Install Claude Code Integration.", path.display())
}

pub fn remove_failed(detail: &str) -> String {
    format!("{detail} Continue anyway? cc-status will keep running until you fix the underlying issue and try again.")
}

pub fn broken_text(path: &Path) -> String {
    format!("Tandem\u{2019}s cc-status hook is no longer in {}. This usually means Claude Code rewrote that file. Reinstall the hook so per-tab status indicators (\u{201C}Working\u{2026},\u{201D} \u{201C}Waiting\u{2026}\u{201D}) work again?", path.display())
}

/// The hook command for the exe: its path with forward slashes in double quotes, a form cmd
/// and Git Bash both run as a program.
pub fn hook_command(exe: &Path) -> String {
    format!("\"{}\"", exe.to_string_lossy().replace('\\', "/"))
}

/// What the three install steps are, and what each says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    EnablePythonApi,
    /// Session cookies, which cc-status needs to be heard (its own question).
    EnableCookie,
    InstallHook,
    ShowToolbelt,
}

impl Step {
    pub fn title(self) -> &'static str {
        match self {
            Step::EnablePythonApi => "Enable Python API",
            Step::EnableCookie => "Enable Session Cookies",
            Step::InstallHook => "Install Hook",
            Step::ShowToolbelt => "Show Toolbelt",
        }
    }

    pub fn button(self) -> &'static str {
        match self {
            Step::EnablePythonApi | Step::EnableCookie => "Enable",
            Step::InstallHook => "Install",
            Step::ShowToolbelt => "Show",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Step::EnablePythonApi => "The Claude Code integration relies on Tandem\u{2019}s Python API to find sessions running Claude and track their status.\n\nThe Python API is currently disabled. Setup can\u{2019}t continue until it is enabled. Click Enable to turn it on.",
            Step::EnableCookie => "The hook runs in a new process that has to prove it belongs to a Tandem session. Session cookies let it do that.\n\nWith cookies on, every new session gets a secret that lets a program running in it use Tandem\u{2019}s Python API without a prompt. Click Enable and confirm to turn them on. Setup cannot go on without them: Cancel leaves them as they are, and you can close setup.",
            Step::InstallHook => "Install a Claude Code hook that lets Tandem detect Claude\u{2019}s state (working, waiting, idle) and display it in the Session Status tool.\n\nThis adds a hook to your Claude Code settings that runs automatically as Claude works.",
            Step::ShowToolbelt => "Show the toolbelt and enable the Session Status tool. The toolbelt appears on the right side of your terminal window.\n\nYou can toggle the toolbelt from View > Toolbelt > Show Toolbelt, or with the shortcut Win+Shift+B.",
        }
    }
}

// ---- impl begin
/// "Enable Session Cookies?": the step's own question (a closed window is Cancel).
pub const COOKIE_HEADING: &str = "Enable Session Cookies?";
pub const COOKIE_TEXT: &str = "New Tandem sessions will get a secret cookie that lets any program running in them use Tandem\u{2019}s Python API without asking you. The Claude Code hook needs it to report status. Cancel leaves cookies as they are.";
pub const COOKIE_BUTTONS: [&str; 2] = ["Enable", "Cancel"];
/// Set while the cookie setting is on because Tandem turned it on (with the person's OK) and
/// nobody changed it since: any other change of the setting clears it (`Globals::set`).
pub const COOKIE_BY_KEY: &str = "NoSyncClaudeCodeTurnedOnCookie";

/// The cookie setting and whether Tandem is the one who turned it on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CookieState {
    pub on: bool,
    pub by_tandem: bool,
}

impl CookieState {
    /// The person said Enable: on, and Tandem's own only when it was off.
    pub fn enabled(self) -> CookieState {
        if self.on { self } else { CookieState { on: true, by_tandem: true } }
    }

    /// Undo what Tandem did, and only that: a setting the person made is left as it is.
    pub fn reverted(self) -> CookieState {
        if self.by_tandem { CookieState { on: false, by_tandem: false } } else { self }
    }
}

/// Whether two paths name the same folder: by final path when both exist (a junction, a
/// substitute drive or a short name included), else by absolute path with the same separators,
/// no trailing separator and no case (Windows ignores case).
pub fn same_path(a: &Path, b: &Path) -> bool {
    if let (Ok(x), Ok(y)) = (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        return lexical(&x) == lexical(&y);
    }
    lexical(a) == lexical(b)
}

fn lexical(p: &Path) -> Vec<String> {
    let abs = std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
    abs.components().map(|c| c.as_os_str().to_string_lossy().to_lowercase()).collect()
}

/// Where Claude Code keeps its settings: `CLAUDE_CONFIG_DIR` (a leading `~` is the profile
/// folder), else `<profile>\.claude`. Tandem has no login shell to ask, so only its own
/// environment counts. A relative value or a network folder means nothing safe here and
/// falls back to the default.
pub fn config_dir(env_value: Option<&str>, userprofile: &Path) -> PathBuf {
    let default = userprofile.join(".claude");
    let Some(v) = env_value.map(str::trim).filter(|v| !v.is_empty()) else { return default };
    if v == "~" {
        return userprofile.to_path_buf();
    }
    if let Some(rest) = v.strip_prefix("~\\").or_else(|| v.strip_prefix("~/")) {
        return userprofile.join(rest.trim_start_matches(['\\', '/']));
    }
    let p = PathBuf::from(v);
    if p.is_absolute() && !is_network(v) { p } else { default }
}

/// A `\\host` or `//host` path: never probed, never written to.
fn is_network(path: &str) -> bool {
    path.starts_with("\\\\") || path.starts_with("//")
}

pub fn settings_path(dir: &Path) -> PathBuf {
    dir.join("settings.json")
}

/// The program a hook command runs: the text in double quotes, else the whole command.
pub fn command_path(command: &str) -> String {
    let c = command.trim();
    match c.strip_prefix('"') {
        Some(rest) => rest.split('"').next().unwrap_or_default().to_string(),
        None => c.to_string(),
    }
}

/// Whether a hook command runs cc-status (iTerm2's ends in "/cc-status"; Tandem's is the exe).
pub fn is_cc_status(command: &str) -> bool {
    let path = command_path(command).to_lowercase();
    matches!(path.rsplit(['/', '\\']).next(), Some("cc-status.exe" | "cc-status"))
}

/// The settings file's text from what reading it returned. Only a missing file is `None`;
/// every other error refuses (the file is left alone). A byte order mark is dropped and an
/// empty file reads as `{}`.
pub fn read_settings(r: std::io::Result<Vec<u8>>, path: &Path) -> Result<Option<String>, String> {
    let bytes = match r {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(read_failed(path, e)),
    };
    let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF][..]).unwrap_or(&bytes);
    let text = std::str::from_utf8(bytes).map_err(|_| read_failed(path, "the file is not UTF-8"))?;
    Ok(Some(if text.trim().is_empty() { "{}".to_string() } else { text.to_string() }))
}

fn pretty(v: &Value) -> String {
    let mut text = serde_json::to_string_pretty(v).unwrap_or_default();
    text.push('\n');
    text
}

fn parse_object(text: &str) -> Option<Value> {
    serde_json::from_str::<Value>(text).ok().filter(Value::is_object)
}

/// An object with its keys in order (the objects Tandem creates; the person's keep theirs).
fn sorted_object(entries: Vec<(String, Value)>) -> Value {
    let mut entries = entries;
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    Value::Object(entries.into_iter().collect())
}

fn fresh_group(command: &str) -> Value {
    let entry = sorted_object(vec![("command".into(), command.into()), ("type".into(), "command".into())]);
    sorted_object(vec![("hooks".into(), Value::Array(vec![entry]))])
}

/// Points every cc-status entry of an event's groups at `command`; whether there was one.
fn rewrite_entries(groups: &mut [Value], command: &str) -> bool {
    let mut found = false;
    for group in groups {
        let Some(Value::Array(entries)) = group.get_mut("hooks") else { continue };
        for entry in entries {
            if entry.get("command").and_then(Value::as_str).is_some_and(is_cc_status) {
                found = true;
                if entry["command"] != command {
                    entry["command"] = command.into();
                }
            }
        }
    }
    found
}

/// The settings with Tandem's hook on all ten events (`OB:1581-1726`): an old cc-status entry is
/// rewritten in place, any other hook stays, an event with none gets a group of its own.
/// Nothing of the person's is removed, and a file that is already right comes back byte for
/// byte. Text that is not a JSON object is an `Err`: it is refused, never replaced (iTerm2
/// starts from `{}`); the caller names the file.
pub fn merge(existing: Option<&str>, command: &str) -> Result<String, String> {
    let original = match existing {
        None => Value::Object(Map::new()),
        Some(t) => parse_object(t).ok_or_else(|| "settings.json is not a JSON object".to_string())?,
    };
    let mut root = original.clone();
    let obj = root.as_object_mut().expect("an object");
    if !obj.get("hooks").is_some_and(Value::is_object) {
        obj.insert("hooks".into(), Value::Object(Map::new()));
    }
    let hooks = obj.get_mut("hooks").and_then(Value::as_object_mut).expect("an object");
    let mut missing = Vec::new();
    for event in HOOK_EVENTS {
        let present = hooks.contains_key(event);
        match hooks.get_mut(event) {
            Some(Value::Array(groups)) => {
                if !rewrite_entries(groups, command) {
                    groups.push(fresh_group(command));
                }
            }
            _ if present => {
                hooks.insert(event.into(), Value::Array(vec![fresh_group(command)]));
            }
            _ => missing.push(event),
        }
    }
    // New events go in sorted order after the person's own.
    missing.sort_unstable();
    for event in missing {
        hooks.insert(event.into(), Value::Array(vec![fresh_group(command)]));
    }
    if root == original {
        if let Some(t) = existing {
            return Ok(t.to_string());
        }
    }
    Ok(pretty(&root))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StripError {
    Unreadable(String),
    Malformed,
    WriteFailed(String),
}

impl StripError {
    /// iTerm2's text for each cause (`OB:158-237`).
    pub fn text(&self, path: &Path) -> String {
        match self {
            StripError::Malformed => not_json(path),
            StripError::Unreadable(_) => format!("Could not read {} \u{2014} check the file\u{2019}s permissions.", path.display()),
            StripError::WriteFailed(_) => format!("Could not write to {} \u{2014} check the file\u{2019}s permissions.", path.display()),
        }
    }
}

/// Removes Tandem's cc-status entries, then the groups, events and `hooks` that this left
/// empty, and nothing else; text with none of ours comes back as it was.
pub fn strip(text: &str) -> Result<String, StripError> {
    let original = parse_object(text).ok_or(StripError::Malformed)?;
    let mut root = original.clone();
    if let Some(Value::Object(hooks)) = root.get_mut("hooks") {
        let mut kept = Map::new();
        let mut removed_any = false;
        for (event, value) in std::mem::take(hooks) {
            let Value::Array(groups) = value else {
                kept.insert(event, value);
                continue;
            };
            let mut event_removed = false;
            let mut left = Vec::new();
            for mut group in groups {
                let mut emptied = false;
                if let Some(Value::Array(entries)) = group.get_mut("hooks") {
                    let before = entries.len();
                    entries.retain(|e| !e.get("command").and_then(Value::as_str).is_some_and(is_cc_status));
                    emptied = entries.len() != before && entries.is_empty();
                    event_removed |= entries.len() != before;
                }
                if !emptied {
                    left.push(group);
                }
            }
            removed_any |= event_removed;
            if !(event_removed && left.is_empty()) {
                kept.insert(event, Value::Array(left));
            }
        }
        *hooks = kept;
        if removed_any && hooks.is_empty() {
            root.as_object_mut().expect("an object").retain(|k, _| k != "hooks");
        }
    }
    if root == original {
        return Ok(text.to_string());
    }
    Ok(pretty(&root))
}

/// Whether an event's groups hold a cc-status entry whose program passes `ok`.
fn has_entry(groups: &Value, ok: &dyn Fn(&str) -> bool) -> bool {
    groups.as_array().into_iter().flatten().filter_map(|g| g.get("hooks")?.as_array()).flatten().filter_map(|e| e.get("command")?.as_str()).any(|c| is_cc_status(c) && ok(&command_path(c)))
}

/// The strict check (`OB:461-495`): all ten events have a cc-status entry whose program
/// `runnable` accepts.
pub fn healthy(text: Option<&str>, runnable: &dyn Fn(&str) -> bool) -> bool {
    let Some(root) = text.and_then(parse_object) else { return false };
    let Some(hooks) = root.get("hooks") else { return false };
    HOOK_EVENTS.iter().all(|e| hooks.get(e).is_some_and(|g| has_entry(g, runnable)))
}

/// The loose check (`OB:505-525`): any cc-status entry on any event.
pub fn any_hook(text: Option<&str>) -> bool {
    let Some(root) = text.and_then(parse_object) else { return false };
    root.get("hooks").and_then(Value::as_object).is_some_and(|h| h.values().any(|g| has_entry(g, &|_| true)))
}

/// Whether a hook's program exists as a file. A network path is never probed.
pub fn runnable_path(path: &str) -> bool {
    !is_network(path) && Path::new(path).is_file()
}

/// "Known" (`MC:24-39`): the hooks cache or the completed flag says the integration is in.
pub fn known(hooks_cache: bool, completed: bool) -> bool {
    hooks_cache || completed
}

/// Which of Install, Reinstall and Uninstall are enabled (muda cannot hide an item).
pub fn menu_enabled(known: bool) -> [bool; 3] {
    [!known, known, known]
}

/// The setup's steps: the API step only while the API is off, the cookie step only while
/// cookies are off.
pub fn steps(api_enabled: bool, cookie_on: bool) -> Vec<Step> {
    let mut v = Vec::new();
    if !api_enabled {
        v.push(Step::EnablePythonApi);
    }
    // The hook reports only with a cookie, so a setup with it off always asks.
    if !cookie_on {
        v.push(Step::EnableCookie);
    }
    v.extend([Step::InstallHook, Step::ShowToolbelt]);
    v
}

/// Whether the announcement may show (`CWT:13-213`).
pub fn watcher_enabled(off: bool, status_tool_last_use: f64, status_tool_visible: bool) -> bool {
    !off && status_tool_last_use <= 0.0 && !status_tool_visible
}

/// The backup kept beside a settings file before a change.
fn backup_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    name.push(".tandem-backup");
    path.with_file_name(name)
}

/// Writes `text` to a temp file in `path`'s folder, then renames it over `path`; with
/// `backup`, the file being replaced is copied aside first. The temp file never outlives a
/// failure.
fn write_replacing(path: &Path, text: &str, backup: bool) -> std::io::Result<()> {
    use std::io::Write;
    if backup {
        std::fs::copy(path, backup_path(path))?;
    }
    let mut name = path.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    name.push(format!(".tandem-{}.tmp", std::process::id()));
    let temp = path.with_file_name(name);
    let written = (|| {
        let mut f = std::fs::File::create(&temp)?;
        f.write_all(text.as_bytes())?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&temp, path)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    written
}

/// Adds the hook to the settings file at `path`, creating the folder and file when missing.
/// Returns whether the file changed. Every failure is one text and leaves the file as it was.
pub fn install_to(path: &Path, command: &str) -> Result<bool, String> {
    let existing = read_settings(std::fs::read(path), path)?;
    let merged = merge(existing.as_deref(), command).map_err(|_| not_json(path))?;
    if existing.as_deref() == Some(merged.as_str()) {
        return Ok(false);
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| write_failed(path, e))?;
    }
    write_replacing(path, &merged, existing.is_some()).map_err(|e| write_failed(path, e))?;
    Ok(true)
}

/// Removes the hook from the settings file at `path`; a missing file is success.
pub fn uninstall_from(path: &Path) -> Result<(), StripError> {
    let text = match read_settings(std::fs::read(path), path) {
        Ok(None) => return Ok(()),
        Ok(Some(t)) => t,
        Err(e) => return Err(StripError::Unreadable(e)),
    };
    let stripped = strip(&text)?;
    if stripped == text {
        return Ok(());
    }
    write_replacing(path, &stripped, true).map_err(|e| StripError::WriteFailed(e.to_string()))
}

/// The settings file of the folder the hook was installed in before, when it is not the one in
/// use. The stored folder is untrusted (P17): it must be an absolute local path with no parent
/// steps and name a folder that exists; only that folder's `settings.json` is returned.
pub fn old_folder_settings(stored: &str, current: &Path) -> Option<PathBuf> {
    let s = stored.trim();
    if s.is_empty() || is_network(s) {
        return None;
    }
    let dir = PathBuf::from(s);
    if !dir.is_absolute() || dir.components().any(|c| matches!(c, std::path::Component::ParentDir)) || !dir.is_dir() {
        return None;
    }
    // The folders are compared, never the raw text: a stored "C:/cc" is the folder "C:\cc\settings.json" is in.
    let same = current.parent().is_some_and(|p| same_path(&dir, p));
    (!same).then(|| settings_path(&dir))
}
// ---- impl end

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::ErrorKind;

    const CMD: &str = "\"C:/Program Files/Tandem/cc-status.exe\"";

    fn parse(t: &str) -> Value {
        serde_json::from_str(t).expect("valid JSON")
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tandem-cc-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Every object's keys are in sorted order, at every level.
    fn sorted(v: &Value) -> bool {
        match v {
            Value::Object(m) => m.keys().zip(m.keys().skip(1)).all(|(a, b)| a < b) && m.values().all(sorted),
            Value::Array(a) => a.iter().all(sorted),
            _ => true,
        }
    }

    fn entries(v: &Value, event: &str) -> Vec<String> {
        v["hooks"][event].as_array().into_iter().flatten().flat_map(|g| g["hooks"].as_array().into_iter().flatten()).filter_map(|e| e["command"].as_str().map(str::to_string)).collect()
    }

    #[test]
    fn config_dir_follows_windows() {
        let home = Path::new(r"C:\Users\u");
        assert_eq!(config_dir(None, home), PathBuf::from(r"C:\Users\u\.claude"));
        assert_eq!(config_dir(Some(""), home), PathBuf::from(r"C:\Users\u\.claude"));
        assert_eq!(config_dir(Some(r"D:\cc"), home), PathBuf::from(r"D:\cc"));
        assert_eq!(config_dir(Some(r"~\cc"), home), PathBuf::from(r"C:\Users\u\cc"));
        // A network folder is never written to or probed; a relative one means nothing here.
        assert_eq!(config_dir(Some(r"\\host\share\cc"), home), PathBuf::from(r"C:\Users\u\.claude"));
        assert_eq!(config_dir(Some("cc"), home), PathBuf::from(r"C:\Users\u\.claude"));
        assert_eq!(settings_path(Path::new(r"D:\cc")), PathBuf::from(r"D:\cc\settings.json"));
        let exe = Path::new(r"C:\Program Files\Tandem\cc-status.exe");
        assert_eq!(hook_command(exe), CMD);
        assert_eq!(command_path(CMD), "C:/Program Files/Tandem/cc-status.exe");
        for yes in [CMD, r"C:\T\cc-status.exe", "/Users/u/.iterm2/cc-status", "\"x/CC-STATUS.EXE\""] {
            assert!(is_cc_status(yes), "{yes}");
        }
        for no in ["cc-status-old", "echo cc-status", ""] {
            assert!(!is_cc_status(no), "{no}");
        }
    }

    #[test]
    fn merge_follows_iterm2() {
        let fresh = merge(None, CMD).unwrap();
        let v = parse(&fresh);
        assert!(sorted(&v), "{fresh}");
        assert!(fresh.contains("\n  \"hooks\""), "two-space indentation");
        for e in HOOK_EVENTS {
            assert_eq!(v["hooks"][e], serde_json::json!([{"hooks": [{"command": CMD, "type": "command"}]}]), "{e}");
        }
        assert_eq!(v["hooks"].as_object().unwrap().len(), 10);

        let old = r#"{"model": "opus", "hooks": {"Stop": [{"matcher": "", "hooks": [{"type": "command", "command": "\"C:/old/cc-status.exe\""}, {"type": "command", "command": "other"}]}]}}"#;
        let v = parse(&merge(Some(old), CMD).unwrap());
        assert_eq!(v["model"], "opus");
        assert_eq!(v["hooks"]["Stop"].as_array().unwrap().len(), 1, "no second Stop group");
        assert_eq!(v["hooks"]["Stop"][0]["matcher"], "");
        assert_eq!(entries(&v, "Stop"), [CMD, "other"]);
        for e in HOOK_EVENTS.iter().filter(|e| **e != "Stop") {
            assert_eq!(entries(&v, e), [CMD], "{e}");
        }
        // A current file is left byte for byte.
        let current = merge(None, CMD).unwrap();
        assert_eq!(merge(Some(&current), CMD).unwrap(), current);
        let odd = format!("{{ \"hooks\":{{}},\n\"b\":1, \"a\":2 }}");
        let merged = merge(Some(&odd), CMD).unwrap();
        let keys: Vec<_> = parse(&merged).as_object().unwrap().keys().cloned().collect();
        assert_eq!(keys, ["hooks", "b", "a"], "the person's key order stays");
        // An event whose value is not an array is replaced by a fresh one.
        let v = parse(&merge(Some(r#"{"hooks": {"Stop": "nope"}}"#), CMD).unwrap());
        assert_eq!(entries(&v, "Stop"), [CMD]);
        // Unparsable text or a root that is not an object is refused, not replaced.
        for bad in ["{oops", "[1]", "3", "\"x\""] {
            assert!(merge(Some(bad), CMD).is_err(), "{bad}");
        }
    }

    #[test]
    fn settings_read_follows_windows() {
        let p = Path::new(r"C:\Users\u\.claude\settings.json");
        assert_eq!(read_settings(Err(std::io::Error::from(ErrorKind::NotFound)), p), Ok(None));
        let denied = read_settings(Err(std::io::Error::from(ErrorKind::PermissionDenied)), p).unwrap_err();
        assert_eq!(denied, read_failed(p, std::io::Error::from(ErrorKind::PermissionDenied)));
        assert!(denied.starts_with(r"Could not read C:\Users\u\.claude\settings.json: "));
        let locked = read_settings(Err(std::io::Error::from_raw_os_error(32)), p).unwrap_err();
        assert!(locked.starts_with("Could not read "), "{locked}");
        assert_eq!(read_settings(Ok(vec![0xff, 0xfe, b'{', b'}']), p), Err(read_failed(p, "the file is not UTF-8")));
        assert_eq!(read_settings(Ok(b"\xef\xbb\xbf{}".to_vec()), p), Ok(Some("{}".to_string())));
        assert_eq!(read_settings(Ok(vec![]), p), Ok(Some("{}".to_string())));
        assert_eq!(read_settings(Ok(b" \n".to_vec()), p), Ok(Some("{}".to_string())));
        assert_eq!(read_settings(Ok(b"{\"a\":1}".to_vec()), p), Ok(Some("{\"a\":1}".to_string())));
        assert_eq!(not_json(p), r"C:\Users\u\.claude\settings.json couldn’t be parsed as JSON. Open it in a text editor and check it for syntax errors.");
        assert_eq!(write_failed(p, "disk full"), r"Could not write to C:\Users\u\.claude\settings.json: disk full");
    }

    /// Every refusal leaves the file's bytes alone: unparsable text, a root that is not an
    /// object, and a path that cannot be read as a file.
    #[test]
    fn an_unparsable_settings_file_is_never_overwritten() {
        let dir = scratch("unparsable");
        let path = dir.join("settings.json");
        for bad in [&b"{oops"[..], b"[1, 2]", b"\xff\xfe{}", b"{\"hooks\": ", b"// a comment\n{}"] {
            std::fs::write(&path, bad).unwrap();
            let e = install_to(&path, CMD).unwrap_err();
            assert!(e.contains("settings.json"), "{e}");
            assert_eq!(std::fs::read(&path).unwrap(), bad, "install left it alone");
            assert!(uninstall_from(&path).is_err());
            assert_eq!(std::fs::read(&path).unwrap(), bad, "uninstall left it alone");
            let names: Vec<_> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name()).collect();
            assert_eq!(names.len(), 1, "no temp or backup left: {names:?}");
        }
        std::fs::write(&path, b"{oops").unwrap();
        assert_eq!(install_to(&path, CMD), Err(not_json(&path)));
        assert_eq!(uninstall_from(&path), Err(StripError::Malformed));
        // A folder where the file should be is a read error, never an empty settings.json.
        let odd = dir.join("dir");
        std::fs::create_dir_all(odd.join("settings.json")).unwrap();
        assert!(install_to(&odd.join("settings.json"), CMD).unwrap_err().starts_with("Could not read "));
        assert!(odd.join("settings.json").is_dir());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn other_hooks_survive_install_and_uninstall() {
        let dir = scratch("survive");
        let path = dir.join("settings.json");
        let original = "{\n  \"model\": \"opus\",\n  \"permissions\": {\"allow\": [\"Bash(ls)\"]},\n  \"hooks\": {\n    \"Stop\": [{\"matcher\": \"*\", \"hooks\": [{\"type\": \"command\", \"command\": \"notify-me\", \"timeout\": 5}]}],\n    \"Custom\": [{\"hooks\": [{\"type\": \"command\", \"command\": \"mine\"}]}]\n  }\n}\n";
        std::fs::write(&path, original).unwrap();
        assert_eq!(install_to(&path, CMD), Ok(true));
        let v = parse(&std::fs::read_to_string(&path).unwrap());
        assert_eq!(v["model"], "opus");
        assert_eq!(v["permissions"]["allow"][0], "Bash(ls)");
        assert_eq!(entries(&v, "Custom"), ["mine"]);
        assert_eq!(entries(&v, "Stop"), ["notify-me", CMD]);
        assert_eq!(v["hooks"]["Stop"][0]["hooks"][0]["timeout"], 5);
        assert_eq!(entries(&v, "SessionEnd"), [CMD]);
        // The original is kept beside it.
        assert_eq!(std::fs::read_to_string(dir.join("settings.json.tandem-backup")).unwrap(), original);
        // A second install changes nothing and writes nothing.
        let after = std::fs::read(&path).unwrap();
        assert_eq!(install_to(&path, CMD), Ok(false));
        assert_eq!(std::fs::read(&path).unwrap(), after);

        assert_eq!(uninstall_from(&path), Ok(()));
        let v = parse(&std::fs::read_to_string(&path).unwrap());
        assert_eq!(v["model"], "opus");
        assert_eq!(v["permissions"]["allow"][0], "Bash(ls)");
        assert_eq!(entries(&v, "Custom"), ["mine"]);
        assert_eq!(entries(&v, "Stop"), ["notify-me"]);
        assert_eq!(v["hooks"]["Stop"][0]["matcher"], "*");
        assert_eq!(v["hooks"].as_object().unwrap().len(), 2, "only the events with other hooks stay: {v}");
        // Nothing of Tandem's is left behind but the backup.
        let mut names: Vec<_> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().to_string()).collect();
        names.sort();
        assert_eq!(names, ["settings.json", "settings.json.tandem-backup"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn install_creates_the_folder_and_a_missing_file_uninstalls_clean() {
        let dir = scratch("create");
        let path = dir.join("nested").join(".claude").join("settings.json");
        assert_eq!(uninstall_from(&path), Ok(()), "a missing file is success");
        assert!(!path.exists());
        assert_eq!(install_to(&path, CMD), Ok(true));
        assert!(sorted(&parse(&std::fs::read_to_string(&path).unwrap())));
        assert!(!path.with_file_name("settings.json.tandem-backup").exists(), "nothing to back up");
        // Stripping what Tandem alone put there leaves a file with no hooks.
        assert_eq!(uninstall_from(&path), Ok(()));
        assert_eq!(parse(&std::fs::read_to_string(&path).unwrap()), serde_json::json!({}));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn strip_follows_iterm2() {
        let merged = merge(Some(r#"{"model": "opus"}"#), CMD).unwrap();
        let v = parse(&strip(&merged).unwrap());
        assert_eq!(v, serde_json::json!({"model": "opus"}));
        let with_other = merge(Some(r#"{"hooks": {"Stop": [{"hooks": [{"type": "command", "command": "other"}]}]}}"#), CMD).unwrap();
        let v = parse(&strip(&with_other).unwrap());
        assert_eq!(v, serde_json::json!({"hooks": {"Stop": [{"hooks": [{"type": "command", "command": "other"}]}]}}));
        // Text with nothing of ours comes back as it was.
        assert_eq!(strip("{ \"a\" :1 }").unwrap(), "{ \"a\" :1 }");
        // A group, event or hooks object that was already empty is not Tandem's to remove.
        let kept = r#"{"hooks": {"Empty": [], "G": [{"hooks": []}]}}"#;
        assert_eq!(strip(kept).unwrap(), kept);
        assert_eq!(strip("{oops"), Err(StripError::Malformed));
        assert_eq!(strip("[]"), Err(StripError::Malformed));
        let p = Path::new(r"C:\c\settings.json");
        assert_eq!(StripError::Malformed.text(p), r"C:\c\settings.json couldn’t be parsed as JSON. Open it in a text editor and check it for syntax errors.");
        assert_eq!(StripError::Unreadable("x".into()).text(p), "Could not read C:\\c\\settings.json \u{2014} check the file\u{2019}s permissions.");
        assert_eq!(StripError::WriteFailed("x".into()).text(p), "Could not write to C:\\c\\settings.json \u{2014} check the file\u{2019}s permissions.");
    }

    #[test]
    fn health_follows_iterm2() {
        let merged = merge(None, CMD).unwrap();
        assert!(healthy(Some(&merged), &|_| true));
        let mut v = parse(&merged);
        v["hooks"].as_object_mut().unwrap().remove("SessionEnd");
        assert!(!healthy(Some(&v.to_string()), &|_| true), "one event missing");
        assert!(!healthy(Some(&merged), &|_| false), "the program is not runnable");
        assert!(!healthy(None, &|_| true));
        assert!(!healthy(Some("{oops"), &|_| true));
        let seen = std::cell::RefCell::new(Vec::new());
        assert!(healthy(Some(&merged), &|p| {
            seen.borrow_mut().push(p.to_string());
            true
        }));
        assert!(seen.borrow().iter().all(|p| p == "C:/Program Files/Tandem/cc-status.exe"), "the check gets the path, not the quoted command");
        let only_stop = r#"{"hooks": {"Stop": [{"hooks": [{"type": "command", "command": "\"C:/T/cc-status.exe\""}]}]}}"#;
        assert!(any_hook(Some(only_stop)));
        assert!(!any_hook(Some(r#"{"hooks": {"Stop": [{"hooks": [{"command": "other"}]}]}}"#)));
        assert!(!any_hook(None));
        // A network path is never probed, and a folder is not a program.
        assert!(!runnable_path(r"\\host\share\cc-status.exe"));
        assert!(!runnable_path("//host/share/cc-status.exe"));
        assert!(!runnable_path(&std::env::temp_dir().to_string_lossy()));
        let dir = scratch("runnable");
        let exe = dir.join("cc-status.exe");
        std::fs::write(&exe, b"x").unwrap();
        assert!(runnable_path(&exe.to_string_lossy()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn menu_rule_follows_iterm2() {
        assert!(!known(false, false));
        assert_eq!(menu_enabled(false), [true, false, false]);
        assert!(known(true, false) && known(false, true));
        assert_eq!(menu_enabled(true), [false, true, true]);
        let titles: Vec<_> = steps(false, true).iter().map(|s| (s.title(), s.button())).collect();
        assert_eq!(titles, [("Enable Python API", "Enable"), ("Install Hook", "Install"), ("Show Toolbelt", "Show")]);
        assert!(!Step::EnablePythonApi.description().contains("cookie"), "the API step turns nothing else on");
        assert_eq!(bullets(Path::new(r"C:\c\settings.json")), "\u{2022} Tandem\u{2019}s Python API is enabled\n\u{2022} A cc-status hook is added to C:\\c\\settings.json\n\u{2022} The toolbelt is shown");
    }

    #[test]
    fn watcher_follows_iterm2() {
        assert!(watcher_enabled(false, 0.0, false));
        assert!(!watcher_enabled(true, 0.0, false));
        assert!(!watcher_enabled(false, 1.0, false));
        assert!(!watcher_enabled(false, 0.0, true));
        assert_eq!(WATCHER_THRESHOLD, 3);
        assert_eq!(WATCHER_TEXT, "Want to try Tandem\u{2019}s Claude Code integration?");
        assert_eq!(WATCHER_BUTTONS, ["_Yes", "Never", "Ask Later"]);
    }

    /// The recorded folder is untrusted (P17): only an absolute, local, existing folder other
    /// than the current one is stripped, and only its settings.json.
    #[test]
    fn the_old_folder_is_untrusted() {
        let dir = scratch("old");
        let (a, b) = (dir.join("a"), dir.join("b"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        let current = settings_path(&a);
        assert_eq!(old_folder_settings(&b.to_string_lossy(), &current), Some(settings_path(&b)));
        assert_eq!(old_folder_settings(&a.to_string_lossy(), &current), None, "the folder in use");
        assert_eq!(old_folder_settings(&a.to_string_lossy().to_uppercase(), &current), None, "Windows paths ignore case");
        assert_eq!(old_folder_settings("", &current), None);
        assert_eq!(old_folder_settings("relative\\dir", &current), None);
        assert_eq!(old_folder_settings(&dir.join("missing").to_string_lossy(), &current), None);
        assert_eq!(old_folder_settings(r"\\host\share\x", &current), None, "never probe a network path");
        assert_eq!(old_folder_settings("//host/share/x", &current), None);
        assert_eq!(old_folder_settings(&format!("{}\\..\\b", a.display()), &current), None, "no parent steps");
        // A file where a folder should be is not a folder.
        let f = dir.join("file");
        std::fs::write(&f, b"x").unwrap();
        assert_eq!(old_folder_settings(&f.to_string_lossy(), &current), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn texts_follow_iterm2() {
        let p = Path::new(r"C:\c\settings.json");
        assert_eq!(uninstall_text(p), "This removes the cc-status hook from C:\\c\\settings.json. You can reinstall any time using Tandem > Install Claude Code Integration.");
        assert!(remove_failed("Boom.").starts_with("Boom. Continue anyway? cc-status will keep running"));
        assert!(broken_text(p).starts_with("Tandem\u{2019}s cc-status hook is no longer in C:\\c\\settings.json. This usually means"));
        assert_eq!(REMOVE_FAILED_HEADING, "Couldn\u{2019}t Remove Hooks");
        assert_eq!(HELP_URL, "https://iterm2.com/claude-code-integration.html");
        assert_eq!(EXE, "cc-status.exe");
        assert!(missing_exe(Path::new(r"C:\T\cc-status.exe")).contains(r"C:\T\cc-status.exe"));
    }

    /// Fix round 1: the cookie is its own step with its own question; the hook cannot report
    /// without it, so setup never shows every step done while it is off.
    #[test]
    fn steps_follow_the_api_and_the_cookie() {
        assert_eq!(steps(false, false), [Step::EnablePythonApi, Step::EnableCookie, Step::InstallHook, Step::ShowToolbelt]);
        assert_eq!(steps(true, false), [Step::EnableCookie, Step::InstallHook, Step::ShowToolbelt], "API on and cookie off: the cookie is asked for");
        assert_eq!(steps(false, true), [Step::EnablePythonApi, Step::InstallHook, Step::ShowToolbelt]);
        assert_eq!(steps(true, true), [Step::InstallHook, Step::ShowToolbelt]);
        assert_eq!((Step::EnableCookie.title(), Step::EnableCookie.button()), ("Enable Session Cookies", "Enable"));
        assert!(Step::EnableCookie.description().contains("cookie") && Step::EnableCookie.description().contains("Cancel"));
        assert_eq!((COOKIE_HEADING, COOKIE_BUTTONS), ("Enable Session Cookies?", ["Enable", "Cancel"]));
        assert!(COOKIE_TEXT.contains("without asking") || COOKIE_TEXT.contains("any program"));
    }

    /// Fix round 1: Tandem turns the cookie on only on the person's OK, remembers that it did,
    /// and turns it back off only if it did.
    #[test]
    fn the_cookie_is_turned_off_only_if_tandem_turned_it_on() {
        let off = CookieState { on: false, by_tandem: false };
        // Cancel does nothing: the state is simply never changed.
        let on = off.enabled();
        assert_eq!(on, CookieState { on: true, by_tandem: true });
        assert_eq!(on.reverted(), off);
        // The person's own setting is never claimed or reverted.
        let theirs = CookieState { on: true, by_tandem: false };
        assert_eq!(theirs.enabled(), theirs);
        assert_eq!(theirs.reverted(), theirs);
        // The person switched it off themselves meanwhile: the marker goes, nothing turns on.
        assert_eq!(CookieState { on: false, by_tandem: true }.reverted(), off);
        assert_eq!(on.enabled(), on);
    }

    /// Task 8 fix round 2: the marker holds only while nobody else changed the setting since
    /// Tandem set it. Switching it off and on again elsewhere makes it the person's own.
    #[test]
    fn a_cookie_changed_elsewhere_is_the_persons_own() {
        let mut g = crate::globals::Globals::empty(std::path::Path::new("cookie-test.toml"));
        let key = crate::api_profile::SET_COOKIE;
        g.set(key, toml::Value::Boolean(true));
        g.set(COOKIE_BY_KEY, toml::Value::Boolean(true));
        g.set(key, toml::Value::Boolean(true));
        assert!(g.bool(COOKIE_BY_KEY), "writing the same value changes nothing");
        g.set(key, toml::Value::Boolean(false));
        g.set(key, toml::Value::Boolean(true));
        assert!(!g.bool(COOKIE_BY_KEY), "off and on again by someone else: no longer Tandem's");
        let state = CookieState { on: g.bool(key), by_tandem: g.bool(COOKIE_BY_KEY) };
        assert_eq!(state.reverted(), state, "Uninstall leaves the person's setting on");
        assert!(!Step::EnableCookie.description().contains("the hook cannot report"), "Cancel does not let setup go on");
    }

    /// Fix round 1: the same folder is found by normalized path, never by raw text, so an install
    /// can never strip the hook it just wrote.
    #[test]
    fn the_same_folder_is_found_by_normalized_path() {
        let dir = scratch("same");
        let a = dir.join("a");
        std::fs::create_dir_all(&a).unwrap();
        let current = settings_path(&a);
        let text = a.to_string_lossy().into_owned();
        for spelling in [text.replace('\\', "/"), format!("{text}\\"), format!("{text}/"), text.to_uppercase(), text.to_lowercase(), format!("{text}\\.")] {
            assert_eq!(old_folder_settings(&spelling, &current), None, "{spelling} is the folder in use");
            assert!(same_path(Path::new(&spelling), &a), "{spelling}");
        }
        // And the current file spelled the other way round.
        let slashed = PathBuf::from(current.to_string_lossy().replace('\\', "/"));
        assert_eq!(old_folder_settings(&text, &slashed), None);
        // A genuinely different folder is still stripped.
        let b = dir.join("b");
        std::fs::create_dir_all(&b).unwrap();
        assert_eq!(old_folder_settings(&b.to_string_lossy(), &current), Some(settings_path(&b)));
        assert!(!same_path(&a, &b));
        assert!(!same_path(&dir.join("a2"), &a), "a longer name is not the same folder");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A junction to the folder in use is the folder in use (the final path is compared when
    /// both exist).
    #[cfg(windows)]
    #[test]
    fn a_junction_is_the_same_folder() {
        let dir = scratch("junction");
        let (real, link) = (dir.join("real"), dir.join("link"));
        std::fs::create_dir_all(&real).unwrap();
        let made = std::process::Command::new("cmd").args(["/C", "mklink", "/J"]).arg(&link).arg(&real).output().map(|o| o.status.success()).unwrap_or(false);
        assert!(made, "mklink /J");
        assert!(same_path(&link, &real));
        assert_eq!(old_folder_settings(&link.to_string_lossy(), &settings_path(&real)), None);
        let _ = std::fs::remove_dir(&link);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
