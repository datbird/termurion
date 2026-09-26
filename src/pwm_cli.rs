//! 1Password and LastPass through their CLIs (`op`, `lpass`). The pure pieces (command builders
//! and output parsers) are plain functions so they are unit tested on Linux; `OnePassword<R>`
//! and `LastPass<R>` are generic over `pwm_run::Runner` so their whole `Source` behavior is
//! tested too, against a fake runner, with no real `op` or `lpass` ever spawned by a test. Only
//! `pwm_run`'s process runner (its own module) is `#[cfg(windows)]`.
//!
//! A password, master password or session token is a `Secret` from the moment it is read to the
//! moment it is written to the child's stdin or environment. It never appears in an argument: a
//! command line is visible to every other process on the machine, so `op item create` and
//! `lpass add` take the new password on stdin, and a 1Password session token travels as the
//! environment variable `OP_SESSION_my` (`OT:176-180`), never `--session`.
//!
//! Nothing here ever shows a CLI's own stderr to the window: only the fixed texts below (a rule
//! stricter than iTerm2's own `showErrorMessage(reason)` for a failed `op signin`, kept because a
//! command that took a secret on stdin could in principle echo part of it back in an error).

use std::time::Duration;

use serde::Deserialize;

use crate::pwm::{Account, Secret, SEP};
use crate::pwm_run::{Request, Runner};
use crate::pwm_source::{Caps, Kind, Source};

/// `op`'s minimum usable version (`OT:103`): older CLIs accept a piped `item edit` template,
/// report success, and silently change nothing, which would break secure in-place editing.
pub const OP_MIN: [u32; 3] = [2, 23, 0];
/// The tag every account Tandem's 1Password integration lists or creates carries (`OP:228`).
pub const OP_TAG: &str = "iTerm2";
/// The tag that turns an OTP-bearing item's automatic code off (`OP:263`).
pub const NO_OTP_TAG: &str = "iTerm2-no-otp";

/// Whether an `op -v` version string meets `OP_MIN`. Missing trailing components are padded
/// with 0 so a two-component "2.23" is not ranked below the three-component minimum
/// (`OT:114-121`).
pub fn op_version_ok(v: &str) -> bool {
    let mut parts: Vec<u32> = v.trim().split('.').filter_map(|s| s.parse().ok()).collect();
    while parts.len() < OP_MIN.len() {
        parts.push(0);
    }
    parts >= OP_MIN.to_vec()
}

/// `op`'s two-stage account list pipeline (`OP:228-246`): `item list --tags iTerm2` piped into
/// `item get -`, which fetches every field for every listed item in one call.
pub fn op_list() -> (Vec<String>, Vec<String>) {
    (
        vec!["item".to_string(), "list".to_string(), "--format=json".to_string(), "--no-color".to_string(), "--tags".to_string(), OP_TAG.to_string()],
        vec!["item".to_string(), "get".to_string(), "--format=json".to_string(), "--no-color".to_string(), "-".to_string()],
    )
}

/// Fetches one item's fields, for the password read (`OP:296-300`).
pub fn op_get(id: &str) -> Vec<String> {
    vec!["item".to_string(), "get".to_string(), "--format=json".to_string(), "--no-color".to_string(), id.to_string()]
}

/// The JSON template piped to `op item create`'s stdin: the password goes only here, never on
/// the command line (`OP:440-450`). Returned as a `Secret` since it carries the password.
pub fn op_create_template(name: &str, user: &str, p: &Secret) -> Secret {
    let value = serde_json::json!({
        "title": name,
        "category": "LOGIN",
        "fields": [
            {"id": "username", "type": "STRING", "purpose": "USERNAME", "label": "username", "value": user},
            {"id": "password", "type": "CONCEALED", "purpose": "PASSWORD", "label": "password", "value": p.expose()},
        ],
    });
    Secret::new(value.to_string())
}

/// Wraps `op item get -`'s "phony json" (one bare object per item, separated by `"}\n{"` or, on
/// a CRLF pipe, `"}\r\n{"`, never a real array) into one valid JSON array (`OP:283`). A comma
/// inserted before a line ending is still valid JSON: RFC 8259 counts CR as whitespace.
pub fn op_split_objects(stdout: &str) -> String {
    let normalized = stdout.replace("}\r\n{", "},\r\n{").replace("}\n{", "},\n{");
    format!("[{normalized}]")
}

#[derive(Deserialize)]
struct OpField {
    id: String,
    #[serde(rename = "type")]
    kind: Option<String>,
    value: Option<String>,
    totp: Option<String>,
}

/// Only what `op_password` and `item_tags` need from one item (the account list, `op_accounts`,
/// reads a raw `serde_json::Value` instead so a vault password's value is never copied into a
/// typed field it does not need).
#[derive(Deserialize)]
struct OpItem {
    #[serde(default)]
    fields: Vec<OpField>,
    #[serde(default)]
    tags: Option<Vec<String>>,
}

fn op_field<'a>(fields: &'a [OpField], id: &str) -> Option<&'a OpField> {
    fields.iter().find(|f| f.id == id)
}

/// Turns `op item get -`'s concatenated-objects stdout into the account list (`OP:283-322`):
/// the user name is field id "username", `has_otp` is any field of type "OTP", and `send_otp`
/// is false when the item carries `NO_OTP_TAG`. Reads each item as a plain `serde_json::Value`
/// rather than a typed struct so that every other field's value (every vault password in the
/// listed items, since `item get` includes concealed values) is never copied into a Rust
/// `String` that would outlive the parse; only "username"'s value, which is not secret, is
/// pulled out. Unknown keys (`section`, `reference`, `label`, and anything else a real item
/// carries) are simply never looked at.
pub fn op_accounts(json: &str) -> Result<Vec<Account>, String> {
    let items: Vec<serde_json::Value> = serde_json::from_str(&op_split_objects(json)).map_err(|e| e.to_string())?;
    Ok(items
        .into_iter()
        .map(|item| {
            let id = item.get("id").and_then(|v| v.as_str()).unwrap_or_default().to_string();
            let title = item.get("title").and_then(|v| v.as_str()).unwrap_or_default().to_string();
            let empty = Vec::new();
            let fields = item.get("fields").and_then(|v| v.as_array()).unwrap_or(&empty);
            let user = fields
                .iter()
                .find(|f| f.get("id").and_then(|v| v.as_str()) == Some("username"))
                .and_then(|f| f.get("value"))
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let has_otp = fields.iter().any(|f| f.get("type").and_then(|v| v.as_str()) == Some("OTP"));
            let no_otp = item.get("tags").and_then(|v| v.as_array()).is_some_and(|tags| tags.iter().any(|t| t.as_str() == Some(NO_OTP_TAG)));
            Account {
                id,
                account_name: title.clone(),
                user_name: user.clone(),
                display: format!("{title}{SEP}{user}"),
                has_otp,
                send_otp: has_otp && !no_otp,
                source_label: None,
            }
        })
        .collect())
}

/// A trailing "\r" a field's value can carry, dropped (`OP:326-330`).
fn drop_trailing_cr(v: String) -> String {
    v.strip_suffix('\r').map(str::to_string).unwrap_or(v)
}

/// Parses one `op item get <id>`'s JSON into its password and, if it has one, its current
/// one-time code (`OP:296-350`): the password is field id "password", else "credential" (a
/// manually tagged API credential); the OTP is the "OTP"-typed field's `totp`.
pub fn op_password(json: &str) -> Result<(Secret, Option<Secret>), String> {
    let item: OpItem = serde_json::from_str(json).map_err(|e| e.to_string())?;
    let value = op_field(&item.fields, "password").or_else(|| op_field(&item.fields, "credential")).and_then(|f| f.value.clone());
    let Some(password) = value.map(drop_trailing_cr) else {
        return Err("op item has no password or credential field.".to_string());
    };
    let otp = item.fields.iter().find(|f| f.kind.as_deref() == Some("OTP")).and_then(|f| f.totp.clone());
    Ok((Secret::new(password), otp.map(Secret::new)))
}

/// Patches a full `op item get --reveal --format=json` payload with the changed fields, leaving
/// every other field (notes, OTP, custom) exactly as `op` returned it: `op item edit` applies
/// the whole template back, so the round-trip must be lossless (`OP:395-427`). `None` leaves
/// that field unchanged. Returned as a `Secret` since a changed password lives in it.
pub fn op_patch_item(original: &str, name: Option<&str>, user: Option<&str>, p: Option<&Secret>) -> Result<Secret, String> {
    let mut obj: serde_json::Value = serde_json::from_str(original).map_err(|e| e.to_string())?;
    let Some(map) = obj.as_object_mut() else { return Err("op item is not an object.".to_string()) };
    if let Some(name) = name {
        map.insert("title".to_string(), serde_json::Value::String(name.to_string()));
    }
    let fields = map.entry("fields").or_insert_with(|| serde_json::Value::Array(Vec::new()));
    let Some(fields) = fields.as_array_mut() else { return Err("op item's fields is not an array.".to_string()) };
    let mut patched_user = false;
    let mut patched_password = false;
    for f in fields.iter_mut() {
        let purpose = f.get("purpose").and_then(|v| v.as_str()).map(str::to_uppercase);
        let id = f.get("id").and_then(|v| v.as_str()).map(str::to_lowercase);
        let is_username = purpose.as_deref() == Some("USERNAME") || id.as_deref() == Some("username");
        let is_password = purpose.as_deref() == Some("PASSWORD") || id.as_deref() == Some("password");
        if is_username {
            if let Some(user) = user {
                f["value"] = serde_json::Value::String(user.to_string());
                patched_user = true;
            }
        } else if is_password {
            if let Some(p) = p {
                f["value"] = serde_json::Value::String(p.expose().to_string());
                patched_password = true;
            }
        }
    }
    if let Some(user) = user.filter(|_| !patched_user) {
        fields.push(serde_json::json!({"id": "username", "type": "STRING", "purpose": "USERNAME", "label": "username", "value": user}));
    }
    if let Some(p) = p.filter(|_| !patched_password) {
        fields.push(serde_json::json!({"id": "password", "type": "CONCEALED", "purpose": "PASSWORD", "label": "password", "value": p.expose()}));
    }
    Ok(Secret::new(obj.to_string()))
}

/// Each `PATH` entry's `op.exe` (a quoted entry has its quotes stripped first), then WinGet's
/// link folder, unless `local_app_data` is empty (in which case that candidate, which would
/// otherwise be a bare rooted path, is left out entirely). Existence and version are checked by
/// `resolve_op`, not here: this only builds the candidate list, as iTerm2's `normalPaths` does.
pub fn op_paths(path_env: &str, local_app_data: &str) -> Vec<String> {
    let mut out: Vec<String> = path_env
        .split(';')
        .map(|s| s.trim().trim_matches('"'))
        .filter(|s| !s.is_empty())
        .map(|dir| format!("{}\\op.exe", dir.trim_end_matches('\\')))
        .collect();
    if !local_app_data.is_empty() {
        out.push(format!("{}\\Microsoft\\WinGet\\Links\\op.exe", local_app_data.trim_end_matches('\\')));
    }
    out
}

/// LastPass has no standard Windows install location (`LP:453` lists only macOS Homebrew and
/// MacPorts paths), so there are no automatic candidates to try; the window goes straight to
/// Locate for it (`resolve_lpass`).
pub fn lpass_paths() -> Vec<String> {
    Vec::new()
}

/// `iTermAdvancedSettingsModel.lastpassGroups()`'s groups (`LP:124-134`): the default when
/// unset is `["iTerm2"]`; otherwise each comma-separated name is kept only if every character is
/// alphanumeric or a space (a name with any other character, such as a stray "!", is dropped
/// whole, not trimmed of just the bad character).
pub fn lpass_groups(setting: &str) -> Vec<String> {
    if setting.is_empty() {
        return vec!["iTerm2".to_string()];
    }
    setting.split(',').filter(|name| name.chars().all(|c| c.is_alphanumeric() || c == ' ')).map(str::to_string).collect()
}

/// `lpass ls`'s args (`LP:136-149`): the tab-separated format iTerm2 parses, plus the single
/// group name when there is exactly one (more than one is filtered after listing, by
/// `lpass_accounts`, since `lpass ls` takes at most one group argument).
pub fn lpass_list(groups: &[String]) -> Vec<String> {
    let mut args = vec!["ls".to_string(), "--format=%ag\t%ai\t%an\t%au".to_string()];
    if let [only] = groups {
        args.push(only.clone());
    }
    args
}

/// Parses `lpass ls`'s tab-separated lines (group, id, name, user): a trailing "\r" (a CRLF
/// pipe) is trimmed from each line first so it never ends up inside the user name; an id of "0"
/// is an unsynced account with no stable identifier and is skipped; only a listed group is kept
/// (`LP:150-176`).
pub fn lpass_accounts(stdout: &str, groups: &[String]) -> Vec<Account> {
    stdout
        .split('\n')
        .filter_map(|line| {
            let line = line.strip_suffix('\r').unwrap_or(line);
            let parts: Vec<&str> = line.split('\t').collect();
            if parts.len() != 4 {
                return None;
            }
            let (group, id, name, user) = (parts[0], parts[1], parts[2], parts[3]);
            if id == "0" || !groups.iter().any(|g| g == group) {
                return None;
            }
            Some(Account {
                id: id.to_string(),
                account_name: name.to_string(),
                user_name: user.to_string(),
                display: format!("{name}{SEP}{user}"),
                has_otp: false,
                send_otp: false,
                source_label: None,
            })
        })
        .collect()
}

/// `lpass add`'s args (`LP:247-250`): iTerm2 always adds under the "iTerm2/" group prefix.
pub fn lpass_add(name: &str) -> Vec<String> {
    vec!["add".to_string(), format!("iTerm2/{name}"), "--non-interactive".to_string()]
}

/// `lpass add`'s stdin (`LP:251`): no trailing newline (the process's stdin is closed instead).
pub fn lpass_add_input(user: &str, p: &Secret) -> Secret {
    Secret::new(format!("Username: {user}\nPassword: {}", p.expose()))
}

// --- Alert texts (Resume notes and the source; "iTerm2" replaced by "Tandem" in user-facing
// text, exact wording and punctuation otherwise, non-ASCII as \u{...} escapes) ---

pub const OP_UNAVAILABLE_TITLE: &str = "OnePassword Unavailable";
pub const OP_CANNOT_FIND_TITLE: &str = "Can\u{2019}t Find 1Password CLI";
/// Names where Tandem actually looks (PATH, then WinGet's link folder; `op_paths`) in place of
/// iTerm2's macOS-only "/usr/local/bin", which would mislead a Windows user (m10b ruling; the
/// source's wording and structure otherwise kept verbatim).
pub const OP_CANNOT_FIND_MESSAGE: &str =
    "In order to use the 1Password integration, Tandem needs to know where to find the CLI app named \u{201c}op\u{201d}. Tandem looks on your PATH and in WinGet\u{2019}s install folder. If you have installed it elsewhere, please select Locate to provide its location.";
pub const OP_LOCATE: &str = "Locate";
pub const OP_NOT_FOUND_MESSAGE: &str = "The 1Password CLI could not be found, or is older than the required version 2.23. Check that a current op is installed.";
pub const OP_MASTER_PASSWORD_PROMPT: &str = "Enter your 1Password master password:";
pub const OP_AUTH_ERROR_TITLE: &str = "Authentication Error";
pub const OP_TIMEOUT_TITLE: &str = "Timeout";
pub const OP_TIMEOUT_MESSAGE: &str = "1Password took too long to respond.";
pub const OP_GARBLED_TOKEN_MESSAGE: &str = "The 1Password CLI app produced garbled output instead of an auth token.";
/// The safe message returned for any failed `op` call other than sign-in (iTerm2's
/// `OPError.runtime` carries no message of its own; the window wraps this via
/// `pwm_source::password_failed` for a read or `pwm::ALERT_GENERIC_EDIT_ERROR` for a save, as it
/// does for every source). Never `op`'s own stderr: a command that took a secret on stdin (an
/// edit or create template, a master password) could in principle echo part of it back.
pub const OP_RUNTIME_ERROR: &str = "1Password reported an error.";
/// The safe message for a failed `op signin --raw` (the master password went in on stdin, so its
/// stderr is never shown; pair with `OP_AUTH_ERROR_TITLE` for the alert).
pub const OP_SIGNIN_FAILED_MESSAGE: &str = "1Password sign-in failed.";

/// "The 1Password CLI at <path> is too old..." (`OT:128`).
pub fn op_too_old_message(path: &str) -> String {
    format!("The 1Password CLI at {path} is too old. The Tandem integration requires version 2.23 or later.")
}

/// "Enter the 1Password master password for account \u{201c}<account>\u{201d}:" (`OT:325`).
pub fn op_master_password_prompt_for_account(account: &str) -> String {
    format!("Enter the 1Password master password for account \u{201c}{account}\u{201d}:")
}

pub const LP_CANNOT_FIND_TITLE: &str = "Can\u{2019}t Find LastPass CLI";
pub const LP_CANNOT_FIND_MESSAGE: &str =
    "In order to use the LastPass integration, Tandem needs to know where to find the CLI app named \u{201c}lpass\u{201d}. Select Locate to provide its location.";
pub const LP_LOCATE: &str = "Locate";
pub const LP_HELP: &str = "Help";
pub const LP_ACCOUNT_LIST_FETCH_FAILED: &str = "The account list could not be fetched.";
pub const LP_PASSWORD_FETCH_FAILED: &str = "The password could not be fetched.";
pub const LP_PASSWORD_SET_FAILED: &str = "The password could not be set.";
pub const LP_ACCOUNT_ADD_FAILED: &str = "The account could not be added.";
pub const LP_ACCOUNT_DELETE_FAILED: &str = "The account could not be deleted";
pub const LP_TIMEOUT_TITLE: &str = "Timeout";
pub const LP_LOGIN_SUCCESSFUL_TITLE: &str = "Login Successful";
pub const LP_LOGIN_SUCCESSFUL_MESSAGE: &str = "Please retry your action in the password manager.";
pub const LP_LOGIN_PROMPT: &str = "Please log in to LastPass";
pub const LP_MASTER_PASSWORD_PROMPT: &str = "Enter your LastPass master password:";
pub const LP_AUTH_FAILED_TITLE: &str = "Authentication Failed";
pub const LP_OPEN_TERMINAL_WINDOW: &str = "Open Terminal Window";
pub const LP_COPY_COMMAND: &str = "Copy Command";
/// The condition an authenticated `lpass` call ends in when its own stderr says the session
/// needs a fresh login (`lpass_needs_login`), distinguished from every other failure so a later
/// task can drive iTerm2's "Authentication Failed" alert and terminal-login flow instead of
/// showing a plain fetch/set/add/delete-failed text.
pub const LP_NEEDS_LOGIN: &str = "LastPass needs a fresh login.";

/// "The LastPass service took too long to respond. <message>" (`LP:313`).
pub fn lp_timeout_message(context: &str) -> String {
    format!("The LastPass service took too long to respond. {context}")
}

/// "You can also try opening a terminal window and running `lpass login <email>`." (`LP:536`),
/// LastPass's "Authentication Failed" alert body.
pub fn lp_auth_failed_message(email: &str) -> String {
    format!("You can also try opening a terminal window and running `lpass login {email}`.")
}

// --- CLI location (pure and process parts only; Locate's file picker and remembering the
// chosen path for the run are UI and belong to the task that wires PromptWindow/ConfirmWindow) ---

/// Why no usable `op` was found automatically (`OT:41-79`): distinguished so the window can show
/// the right alert, `OP_NOT_FOUND_MESSAGE` or `op_too_old_message`, before offering Locate.
#[derive(Debug, PartialEq)]
pub enum OpNotUsable {
    NotFound,
    TooOld { path: String, version: String },
}

/// The alert text for a `resolve_op` failure, ready for the window.
pub fn op_not_usable_message(e: &OpNotUsable) -> String {
    match e {
        OpNotUsable::NotFound => OP_NOT_FOUND_MESSAGE.to_string(),
        OpNotUsable::TooOld { path, .. } => op_too_old_message(path),
    }
}

fn op_probe_version<R: Runner>(runner: &mut R, path: &str) -> Option<String> {
    let out = runner.run(Request::new(path.to_string(), vec!["-v".to_string()])).ok()?;
    (out.code == 0).then(|| out.stdout.expose().trim().to_string())
}

/// Tries each of `op_paths`' candidates that `exists` reports present, checking each one's
/// version by running `-v` through `runner`, and returns the first that meets `OP_MIN`
/// (iTerm2's `pathToCLI`/`checkUsability`; unlike it, this does not also pick the newest among
/// several usable ones, since a Windows machine is not expected to have more than one `op` on
/// PATH). `exists` lets a test avoid touching the real filesystem; the real caller passes
/// `|p| std::path::Path::new(p).is_file()`.
pub fn resolve_op<R: Runner>(runner: &mut R, candidates: &[String], exists: &mut dyn FnMut(&str) -> bool) -> Result<String, OpNotUsable> {
    let mut too_old = None;
    for path in candidates.iter().filter(|c| exists(c)) {
        match op_probe_version(runner, path) {
            Some(v) if op_version_ok(&v) => return Ok(path.clone()),
            Some(v) if too_old.is_none() => too_old = Some((path.clone(), v)),
            _ => {}
        }
    }
    match too_old {
        Some((path, version)) => Err(OpNotUsable::TooOld { path, version }),
        None => Err(OpNotUsable::NotFound),
    }
}

/// LastPass's counterpart to `resolve_op`: since `lpass_paths` is always empty on Windows, there
/// is nothing to probe, so this is a placeholder for symmetry with `resolve_op` (the window's
/// wiring calls one or the other by kind); the real search is entirely Locate.
pub fn resolve_lpass() -> Result<String, ()> {
    Err(())
}

// --- OnePassword and LastPass as `Source`s over a `Runner` ---

/// `op item list`'s own timeout (`OT:143`, `OnePasswordBasicCommandRecipe`'s fixed 10 s
/// deadline); every other `op` call (`user get --me`, `signin`, `item get`, `item get -`,
/// `item create`, `item edit`, `item delete`) has none, matching iTerm2 exactly.
const OP_LIST_TIMEOUT: Duration = Duration::from_secs(10);
/// `lpass ls` and `lpass sync now`'s timeout (`LP:150`, `LP:275`); every other `lpass` call has
/// none.
const LASTPASS_LIST_TIMEOUT: Duration = Duration::from_secs(5);

fn secret_env(key: &str, value: &str) -> (String, Secret) {
    (key.to_string(), Secret::new(value.to_string()))
}

/// Whether `op`'s stderr shows the session is no longer signed in
/// (`Data.smellsLike1PasswordAuthenticationError`): an `op` line of the shape
/// `"[ERROR] ... You are not currently signed in ..."`.
fn smells_like_op_auth_error(stderr: &str) -> bool {
    stderr.starts_with("[ERROR] ") && stderr.contains("You are not currently signed in")
}

/// Asks for the 1Password master password; `None` means the user canceled (`OT:418-421`,
/// `ModalPasswordAlert`). The real prompt is a `prompt_ui::PromptWindow`, wired in by the app
/// layer that constructs `OnePassword`; this module only calls the closure it is given, so it
/// is testable with a canned answer and never itself touches egui.
pub type MasterPasswordPrompt = Box<dyn FnMut(&str) -> Option<Secret> + Send>;

/// 1Password's authentication state for this run: unauthenticated, unlocked by the desktop app
/// (no session token needed), or holding a session token from `op signin --raw` (`OT:306-309`).
enum Auth {
    None,
    Biometric,
    Token(Secret),
}

/// 1Password through `op` (`OnePasswordDataSource`). Supports in-place edit, multiple accounts,
/// and requires a password to add (`OP:500-575`).
pub struct OnePassword<R: Runner> {
    runner: R,
    path: String,
    account: Option<String>,
    auth: Auth,
    prompt: MasterPasswordPrompt,
}

impl<R: Runner> OnePassword<R> {
    /// `path` is `op`'s already-resolved executable path (found by `resolve_op` and, failing
    /// that, a Locate file picker the app layer wires up; that UI is not pure and lives outside
    /// this module). `account` is the 1Password account to use when more than one is signed in
    /// (`--account`), or `None`.
    pub fn new(runner: R, path: String, account: Option<String>, prompt: MasterPasswordPrompt) -> OnePassword<R> {
        OnePassword { runner, path, account, auth: Auth::None, prompt }
    }

    fn account_args(&self, args: &mut Vec<String>) {
        if let Some(a) = &self.account {
            args.push("--account".to_string());
            args.push(a.clone());
        }
    }

    fn base_env(&self) -> Vec<(String, Secret)> {
        match &self.account {
            Some(a) => vec![secret_env("OP_ACCOUNT", a)],
            None => Vec::new(),
        }
    }

    /// The environment for an authenticated call: the session token as `OP_SESSION_my`
    /// (`OT:176-180`) when signed in without the desktop app; nothing extra for biometric
    /// unlock, which needs no token.
    fn env(&self) -> Vec<(String, Secret)> {
        let mut env = self.base_env();
        if let Auth::Token(t) = &self.auth {
            env.push(secret_env("OP_SESSION_my", t.expose()));
        }
        env
    }

    /// `op user get --me` succeeding means the desktop app already unlocked 1Password, so no
    /// token is needed; otherwise the master password is asked for and traded for a session
    /// token with `op signin --raw` (`OT:328-408`). A no-op once authenticated this run. Neither
    /// call has a timeout: both can wait on the desktop app's own unlock prompt (`OT:436`,
    /// `useTTY` with no deadline; `OT:371`'s `signin` has none either).
    fn ensure_auth(&mut self) -> Result<(), String> {
        if !matches!(self.auth, Auth::None) {
            return Ok(());
        }
        let mut args = vec!["user".to_string(), "get".to_string(), "--me".to_string()];
        self.account_args(&mut args);
        let out = self.runner.run(Request { program: self.path.clone(), args, env: self.base_env(), stdin: None, timeout: None })?;
        if out.code == 0 {
            self.auth = Auth::Biometric;
            return Ok(());
        }
        let prompt_text = match &self.account {
            Some(a) => op_master_password_prompt_for_account(a),
            None => OP_MASTER_PASSWORD_PROMPT.to_string(),
        };
        let Some(password) = (self.prompt)(&prompt_text) else {
            return Err("Canceled.".to_string());
        };
        let mut args = vec!["signin".to_string(), "--raw".to_string()];
        self.account_args(&mut args);
        let stdin = Secret::new(format!("{}\n", password.expose()));
        let out = self.runner.run(Request { program: self.path.clone(), args, env: self.base_env(), stdin: Some(stdin), timeout: None })?;
        if out.code != 0 {
            return Err(OP_SIGNIN_FAILED_MESSAGE.to_string());
        }
        self.auth = Auth::Token(Secret::new(out.stdout.expose().trim().to_string()));
        Ok(())
    }

    /// Runs one authenticated `op` call, retrying once if `op` reports the session was not
    /// signed in (`OT:176-190`, `smells_like_op_auth_error`): the cached auth is cleared, `op`
    /// is signed into again (prompting again if needed), and the same call is repeated with
    /// fresh auth. Never used for `user get --me` or `signin` themselves, which `ensure_auth`
    /// runs directly. `args` is cloned and `stdin` duplicated so the same call can be replayed.
    fn call(&mut self, args: Vec<String>, stdin: Option<Secret>, timeout: Option<Duration>) -> Result<crate::pwm_run::Output, String> {
        self.ensure_auth()?;
        let retry_stdin = stdin.as_ref().map(Secret::duplicate);
        let out = self.runner.run(Request { program: self.path.clone(), args: args.clone(), env: self.env(), stdin, timeout })?;
        if out.code != 0 && smells_like_op_auth_error(&out.stderr) {
            self.auth = Auth::None;
            self.ensure_auth()?;
            return self.runner.run(Request { program: self.path.clone(), args, env: self.env(), stdin: retry_stdin, timeout });
        }
        Ok(out)
    }

    fn item_tags(&mut self, id: &str) -> Result<Vec<String>, String> {
        let out = self.call(op_get(id), None, None)?;
        if out.code != 0 {
            return Err(OP_RUNTIME_ERROR.to_string());
        }
        let item: OpItem = serde_json::from_str(out.stdout.expose()).map_err(|e| e.to_string())?;
        Ok(item.tags.unwrap_or_default())
    }
}

impl<R: Runner> Source for OnePassword<R> {
    fn kind(&self) -> Kind {
        Kind::OnePassword
    }

    fn name(&self) -> String {
        "1Password".to_string()
    }

    fn caps(&self) -> Caps {
        Caps { in_place: true, can_edit_password: true, requires_password: true, multiple_accounts: true, ..Caps::default() }
    }

    /// Checks only that a usable `op` is at `self.path` (found + new enough), as iTerm2's
    /// `checkAvailability`/`throwIfUnusable` do; authentication happens lazily on the first real
    /// operation, not here.
    fn available(&mut self) -> Result<(), String> {
        match op_probe_version(&mut self.runner, &self.path) {
            Some(v) if op_version_ok(&v) => Ok(()),
            Some(_) => Err(op_too_old_message(&self.path)),
            None => Err(OP_NOT_FOUND_MESSAGE.to_string()),
        }
    }

    fn accounts(&mut self) -> Result<Vec<Account>, String> {
        let (list_args, get_args) = op_list();
        let listed = match self.call(list_args, None, Some(OP_LIST_TIMEOUT)) {
            Ok(o) => o,
            Err(e) if e == crate::pwm_run::TIMED_OUT => return Err(OP_TIMEOUT_MESSAGE.to_string()),
            Err(e) => return Err(e),
        };
        if listed.code != 0 {
            return Err(OP_RUNTIME_ERROR.to_string());
        }
        let fetched = self.call(get_args, Some(listed.stdout), None)?;
        if fetched.code != 0 {
            return Err(OP_RUNTIME_ERROR.to_string());
        }
        op_accounts(fetched.stdout.expose())
    }

    fn password(&mut self, a: &Account) -> Result<(Secret, Option<Secret>), String> {
        let out = self.call(op_get(&a.id), None, None)?;
        if out.code != 0 {
            return Err(OP_RUNTIME_ERROR.to_string());
        }
        op_password(out.stdout.expose())
    }

    fn set_password(&mut self, a: &Account, p: Secret) -> Result<(), String> {
        self.edit_in_place(a, None, None, Some(p))
    }

    fn delete(&mut self, a: &Account) -> Result<(), String> {
        let out = self.call(vec!["item".to_string(), "delete".to_string(), a.id.clone()], None, None)?;
        if out.code != 0 {
            return Err(OP_RUNTIME_ERROR.to_string());
        }
        Ok(())
    }

    fn add(&mut self, name: &str, user: &str, p: Secret, _flags: &[(String, bool)]) -> Result<Account, String> {
        // 1Password has no Add Account toggles, so flags is ignored (as iTerm2's `OP:527-528`).
        let template = op_create_template(name, user, &p);
        let args = vec!["item".to_string(), "create".to_string(), "--format=json".to_string(), format!("--tags={OP_TAG}"), "-".to_string()];
        let out = self.call(args, Some(template), None)?;
        if out.code != 0 {
            return Err(OP_RUNTIME_ERROR.to_string());
        }
        #[derive(Deserialize)]
        struct Created {
            id: String,
        }
        let created: Created = serde_json::from_str(out.stdout.expose()).map_err(|e| e.to_string())?;
        Ok(Account { id: created.id, account_name: name.to_string(), user_name: user.to_string(), display: format!("{name}{SEP}{user}"), has_otp: false, send_otp: false, source_label: None })
    }

    fn edit_in_place(&mut self, a: &Account, name: Option<&str>, user: Option<&str>, p: Option<Secret>) -> Result<(), String> {
        // `--reveal` so the concealed password field carries its real value: the whole
        // template is resubmitted, so an unchanged password must round-trip as itself
        // (`OP:395-400`).
        let get_args = vec!["item".to_string(), "get".to_string(), a.id.clone(), "--reveal".to_string(), "--format=json".to_string()];
        let got = self.call(get_args, None, None)?;
        if got.code != 0 {
            return Err(OP_RUNTIME_ERROR.to_string());
        }
        let patched = op_patch_item(got.stdout.expose(), name, user, p.as_ref())?;
        let out = self.call(vec!["item".to_string(), "edit".to_string(), a.id.clone()], Some(patched), None)?;
        if out.code != 0 {
            return Err(OP_RUNTIME_ERROR.to_string());
        }
        Ok(())
    }

    fn toggle_otp(&mut self, a: &Account) -> Result<Account, String> {
        let mut tags = self.item_tags(&a.id)?;
        if a.send_otp {
            if !tags.iter().any(|t| t == NO_OTP_TAG) {
                tags.push(NO_OTP_TAG.to_string());
            }
        } else {
            tags.retain(|t| t != NO_OTP_TAG);
        }
        let args = vec!["item".to_string(), "edit".to_string(), a.id.clone(), "--tags".to_string(), tags.join(",")];
        let out = self.call(args, None, None)?;
        if out.code != 0 {
            return Err(OP_RUNTIME_ERROR.to_string());
        }
        Ok(Account { send_otp: !a.send_otp, ..a.clone() })
    }

    fn reload(&mut self) {
        // No account list is cached here (unlike iTerm2's 30-minute cache): every `accounts()`
        // call already re-fetches, so there is nothing to invalidate.
    }

    fn reset_configuration(&mut self) {
        // `canResetConfiguration` is false (`OP:493`): never called.
    }

    fn switch_account(&mut self) -> Result<(), String> {
        // Picking among several signed-in 1Password accounts (`OP:664-668`,
        // `OnePasswordAccountPicker`) is not built in this milestone; `caps().multiple_accounts`
        // enables the menu item, but choosing it does nothing yet.
        Ok(())
    }
}

/// Asks for a LastPass login: the prompt text, and a default username to prefill (iTerm2 keeps
/// the last one in `UserDefaults`; here the caller tracks it, since this module has no settings
/// store). Returns `None` if the user canceled.
pub type LoginPrompt = Box<dyn FnMut(&str, &str) -> Option<(String, Secret)> + Send>;

/// LastPass through `lpass` (`LastPassDataSource`). No in-place edit, one account group set,
/// an empty password is allowed on Add (`lpass add` fills a blank one) (`LP:335-437`).
pub struct LastPass<R: Runner> {
    runner: R,
    path: String,
    groups: Vec<String>,
    last_username: String,
    prompt: LoginPrompt,
}

impl<R: Runner> LastPass<R> {
    /// `path` is `lpass`'s already-resolved path (there is no Windows installer, so it is
    /// always found by Locate; that search lives with the app wiring, not here). `groups` is
    /// `lpass_groups` applied to the `lastpassGroups` global.
    pub fn new(runner: R, path: String, groups: Vec<String>, prompt: LoginPrompt) -> LastPass<R> {
        LastPass { runner, path, groups, last_username: String::new(), prompt }
    }

    /// Every `lpass` call runs with `LPASS_DISABLE_PINENTRY=1` and no `LPASS_ASKPASS`, so a
    /// password given on stdin is actually read: without it, lastpass-cli tries a system
    /// pinentry program first (Gpg4win, Cygwin) and only falls back to stdin when none is found,
    /// which would otherwise hang a real sign-in until the timeout (m10b ruling; not itself a
    /// secret, so it travels as a plain env pair like any other).
    fn env(&self) -> Vec<(String, Secret)> {
        vec![secret_env("LPASS_DISABLE_PINENTRY", "1")]
    }

    fn request(&self, args: Vec<String>, timeout: Option<Duration>) -> Request {
        Request { program: self.path.clone(), args, env: self.env(), stdin: None, timeout }
    }

    /// Whether `lpass`'s stderr shows the session needs a fresh login (`LP:43`, the Swift
    /// `ErrorHandler`'s check): the CLI's own text mentions running `lpass login`.
    fn needs_login(out: &crate::pwm_run::Output) -> bool {
        out.stderr.contains("lpass login")
    }

    /// Maps a failed, non-secret-bearing `lpass` call to `LP_NEEDS_LOGIN` when its stderr shows
    /// an expired session, else to `fallback` (never `out.stderr` itself).
    fn mapped_error(out: &crate::pwm_run::Output, fallback: &str) -> String {
        if Self::needs_login(out) { LP_NEEDS_LOGIN.to_string() } else { fallback.to_string() }
    }

    /// Prompts for LastPass credentials and runs `lpass login` with the password on stdin
    /// (`LP:504-529`, `LastPassUtils.showLoginUI`). A failure never shows raw stderr (the
    /// password went in on stdin): iTerm2's own "Authentication Failed" body, naming the
    /// username just entered.
    fn login(&mut self) -> Result<(), String> {
        let Some((user, password)) = (self.prompt)(LP_LOGIN_PROMPT, &self.last_username) else {
            return Err("Canceled.".to_string());
        };
        self.last_username = user.clone();
        let mut request = self.request(vec!["login".to_string(), "--color=never".to_string(), user.clone()], None);
        request.stdin = Some(password);
        let out = self.runner.run(request)?;
        if out.code != 0 {
            return Err(lp_auth_failed_message(&user));
        }
        Ok(())
    }
}

impl<R: Runner> Source for LastPass<R> {
    fn kind(&self) -> Kind {
        Kind::LastPass
    }

    fn name(&self) -> String {
        "LastPass".to_string()
    }

    fn caps(&self) -> Caps {
        Caps { can_edit_password: true, ..Caps::default() }
    }

    fn available(&mut self) -> Result<(), String> {
        let out = self.runner.run(self.request(vec!["status".to_string(), "--color=never".to_string()], None))?;
        if out.code == 0 {
            return Ok(());
        }
        if out.stdout.expose().starts_with("Not logged in") {
            return self.login();
        }
        Err("LastPass is not available.".to_string())
    }

    fn accounts(&mut self) -> Result<Vec<Account>, String> {
        let out = match self.runner.run(self.request(lpass_list(&self.groups), Some(LASTPASS_LIST_TIMEOUT))) {
            Ok(o) => o,
            Err(e) if e == crate::pwm_run::TIMED_OUT => return Err(lp_timeout_message(LP_ACCOUNT_LIST_FETCH_FAILED)),
            Err(e) => return Err(e),
        };
        if out.code != 0 {
            return Err(Self::mapped_error(&out, LP_ACCOUNT_LIST_FETCH_FAILED));
        }
        Ok(lpass_accounts(out.stdout.expose(), &self.groups))
    }

    fn password(&mut self, a: &Account) -> Result<(Secret, Option<Secret>), String> {
        let out = self.runner.run(self.request(vec!["show".to_string(), "--password".to_string(), a.id.clone()], None))?;
        if out.code != 0 {
            return Err(Self::mapped_error(&out, LP_PASSWORD_FETCH_FAILED));
        }
        Ok((Secret::new(out.stdout.expose().trim().to_string()), None))
    }

    fn set_password(&mut self, a: &Account, p: Secret) -> Result<(), String> {
        let mut request = self.request(vec!["edit".to_string(), "--non-interactive".to_string(), "--password".to_string(), a.id.clone()], None);
        request.stdin = Some(Secret::new(format!("{}\n", p.expose())));
        let out = self.runner.run(request)?;
        if out.code != 0 {
            return Err(Self::mapped_error(&out, LP_PASSWORD_SET_FAILED));
        }
        Ok(())
    }

    fn delete(&mut self, a: &Account) -> Result<(), String> {
        let out = self.runner.run(self.request(vec!["rm".to_string(), a.id.clone()], None))?;
        if out.code != 0 {
            return Err(Self::mapped_error(&out, LP_ACCOUNT_DELETE_FAILED));
        }
        Ok(())
    }

    fn add(&mut self, name: &str, user: &str, p: Secret, _flags: &[(String, bool)]) -> Result<Account, String> {
        // LastPass has no Add Account toggles, so flags is ignored (as iTerm2's `LP:368`).
        let mut request = self.request(lpass_add(name), None);
        request.stdin = Some(lpass_add_input(user, &p));
        let added = self.runner.run(request)?;
        if added.code != 0 {
            return Err(Self::mapped_error(&added, LP_ACCOUNT_ADD_FAILED));
        }
        match self.runner.run(self.request(vec!["sync".to_string(), "now".to_string()], Some(LASTPASS_LIST_TIMEOUT))) {
            Ok(out) if out.code == 0 => {}
            Ok(out) => return Err(Self::mapped_error(&out, LP_ACCOUNT_ADD_FAILED)),
            Err(e) if e == crate::pwm_run::TIMED_OUT => return Err(lp_timeout_message(LP_ACCOUNT_ADD_FAILED)),
            Err(e) => return Err(e),
        }
        let shown = self.runner.run(self.request(vec!["show".to_string(), "--id".to_string(), format!("iTerm2/{name}")], None))?;
        if shown.code != 0 {
            return Err(Self::mapped_error(&shown, LP_ACCOUNT_ADD_FAILED));
        }
        let id = shown.stdout.expose().trim().lines().last().unwrap_or_default().to_string();
        if id.is_empty() || id == "0" {
            return Err(LP_ACCOUNT_ADD_FAILED.to_string());
        }
        Ok(Account { id, account_name: name.to_string(), user_name: user.to_string(), display: format!("{name}{SEP}{user}"), has_otp: false, send_otp: false, source_label: None })
    }

    fn edit_in_place(&mut self, _a: &Account, _name: Option<&str>, _user: Option<&str>, _p: Option<Secret>) -> Result<(), String> {
        // Never called: `caps().in_place` is false (`LP:356`), so the window deletes and re-adds.
        Err("In-place edit is not supported.".to_string())
    }

    fn toggle_otp(&mut self, _a: &Account) -> Result<Account, String> {
        // Never called: LastPass accounts never have a one-time code (`LP:340-341`).
        Err("One-time codes are not supported.".to_string())
    }

    fn reload(&mut self) {}

    fn reset_configuration(&mut self) {
        // `canResetConfiguration` is false (`LP:337`): never called.
    }

    fn switch_account(&mut self) -> Result<(), String> {
        // `supportsMultipleAccounts` is false (`LP:430-432`): never called.
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pwm_run::{Output, TIMED_OUT};
    use std::sync::{Arc, Mutex};

    /// One `Request` a `FakeRunner` was given, captured for assertions (never a secret's value
    /// unless the test itself reads one back through `Secret::expose`).
    #[derive(Clone, Debug)]
    struct Recorded {
        program: String,
        args: Vec<String>,
        env: Vec<(String, String)>,
        stdin: Option<String>,
        timeout: Option<Duration>,
    }

    /// Records every `Request` it is given and returns the next canned result from its queue
    /// (an `Ok(Output)` or an `Err(String)`, so a test can simulate a timeout via `TIMED_OUT` or
    /// a spawn failure).
    #[derive(Default)]
    struct FakeRunner {
        requests: Arc<Mutex<Vec<Recorded>>>,
        outputs: std::collections::VecDeque<Result<(i32, String, String), String>>,
    }

    impl FakeRunner {
        fn push(&mut self, code: i32, stdout: &str, stderr: &str) {
            self.outputs.push_back(Ok((code, stdout.to_string(), stderr.to_string())));
        }

        fn push_err(&mut self, err: &str) {
            self.outputs.push_back(Err(err.to_string()));
        }

        fn calls(&self) -> Vec<Recorded> {
            self.requests.lock().unwrap().clone()
        }
    }

    impl Runner for FakeRunner {
        fn run(&mut self, r: Request) -> Result<Output, String> {
            let env: Vec<(String, String)> = r.env.iter().map(|(k, v)| (k.clone(), v.expose().to_string())).collect();
            let stdin = r.stdin.as_ref().map(|s| s.expose().to_string());
            self.requests.lock().unwrap().push(Recorded { program: r.program.clone(), args: r.args.clone(), env, stdin, timeout: r.timeout });
            match self.outputs.pop_front() {
                Some(Ok((code, stdout, stderr))) => Ok(Output { code, stdout: Secret::new(stdout), stderr }),
                Some(Err(e)) => Err(e),
                None => Err("FakeRunner: no more outputs queued".to_string()),
            }
        }
    }

    /// Asserts that no recorded request's program, arguments or environment values contain
    /// `secret` (item 13: a single helper covering every call a test made, not just one).
    fn assert_secret_only_on_stdin(calls: &[Recorded], secret: &str) {
        for c in calls {
            assert!(!c.program.contains(secret), "program: {}", c.program);
            assert!(!c.args.iter().any(|a| a.contains(secret)), "args: {:?}", c.args);
            assert!(!c.env.iter().any(|(_, v)| v.contains(secret)), "env: {:?}", c.env);
        }
    }

    fn account(id: &str, name: &str, user: &str) -> Account {
        Account { id: id.to_string(), account_name: name.to_string(), user_name: user.to_string(), display: format!("{name}{SEP}{user}"), has_otp: false, send_otp: false, source_label: None }
    }

    #[test]
    fn cli_commands_follow_iterm2() {
        assert_eq!(
            op_list(),
            (
                vec!["item".to_string(), "list".to_string(), "--format=json".to_string(), "--no-color".to_string(), "--tags".to_string(), "iTerm2".to_string()],
                vec!["item".to_string(), "get".to_string(), "--format=json".to_string(), "--no-color".to_string(), "-".to_string()],
            )
        );
        assert_eq!(op_get("abc"), vec!["item".to_string(), "get".to_string(), "--format=json".to_string(), "--no-color".to_string(), "abc".to_string()]);

        assert_eq!(lpass_add("x"), vec!["add".to_string(), "iTerm2/x".to_string(), "--non-interactive".to_string()]);
        let stdin = lpass_add_input("u", &Secret::new("p".to_string()));
        assert_eq!(stdin.expose(), "Username: u\nPassword: p");

        // Adding an account puts the password only in the stdin template; no argument, program
        // name or env value, in any recorded request, ever contains it.
        let secret_marker = "s3cr3t-marker";
        let mut runner = FakeRunner::default();
        let requests = runner.requests.clone();
        runner.push(0, "", ""); // op user get --me: biometric unlock, no signin needed
        runner.push(0, r#"{"id":"new-id"}"#, "");
        let mut op = OnePassword::new(runner, "op".to_string(), None, Box::new(|_| None));
        op.add("Example", "bob", Secret::new(secret_marker.to_string()), &[]).expect("add");
        let calls = requests.lock().unwrap().clone();
        assert_secret_only_on_stdin(&calls, secret_marker);
        let create_call = calls.iter().find(|c| c.args.first().map(String::as_str) == Some("item") && c.args.get(1).map(String::as_str) == Some("create"));
        let create_call = create_call.expect("an item create call was made");
        assert_eq!(create_call.args, vec!["item".to_string(), "create".to_string(), "--format=json".to_string(), "--tags=iTerm2".to_string(), "-".to_string()]);
        assert!(create_call.stdin.as_deref().is_some_and(|s| s.contains(secret_marker)), "the template carries the password");
        assert!(create_call.timeout.is_none(), "op item create has no timeout");

        // The session token travels as the env pair OP_SESSION_my, never as an argument, once
        // signing in (no desktop app unlock) hands one back; the master password itself never
        // appears in any argument or env value either.
        let master_marker = "m4st3r-marker";
        let mut runner = FakeRunner::default();
        let requests = runner.requests.clone();
        runner.push(1, "", "not signed in"); // op user get --me fails: no desktop unlock
        runner.push(0, "the-token\n", ""); // op signin --raw
        runner.push(0, "[]", ""); // item list
        runner.push(0, "", ""); // item get -
        let mut op = OnePassword::new(runner, "op".to_string(), None, Box::new(move |_| Some(Secret::new(master_marker.to_string()))));
        op.accounts().expect("accounts");
        let calls = requests.lock().unwrap().clone();
        assert_secret_only_on_stdin(&calls, master_marker);
        let signin_call = calls.iter().find(|c| c.args.first().map(String::as_str) == Some("signin")).expect("a signin call was made");
        assert!(signin_call.stdin.as_deref().is_some_and(|s| s.contains(master_marker)), "the master password carries on stdin");
        assert!(signin_call.timeout.is_none(), "op signin has no timeout");
        let list_call = calls.iter().find(|c| c.args.first().map(String::as_str) == Some("item") && c.args.get(1).map(String::as_str) == Some("list")).expect("an item list call");
        assert!(list_call.env.iter().any(|(k, v)| k == "OP_SESSION_my" && v == "the-token"), "{:?}", list_call.env);
        assert!(!list_call.args.iter().any(|a| a.contains("the-token")));
        assert_eq!(list_call.timeout, Some(OP_LIST_TIMEOUT));
        let get_call = calls.iter().find(|c| c.args.first().map(String::as_str) == Some("item") && c.args.get(1).map(String::as_str) == Some("get") && c.args.contains(&"-".to_string()));
        assert!(get_call.expect("an item get - call").timeout.is_none(), "item get - has no timeout");

        // Editing in place also keeps the new password off every argument and env value.
        let edit_marker = "3d1t-marker";
        let original = r#"{"id":"1","title":"old","fields":[{"id":"password","value":"old-pw"}]}"#;
        let mut runner = FakeRunner::default();
        let requests = runner.requests.clone();
        runner.push(0, "", ""); // op user get --me
        runner.push(0, original, ""); // item get --reveal
        runner.push(0, "", ""); // item edit
        let mut op = OnePassword::new(runner, "op".to_string(), None, Box::new(|_| None));
        let a = account("1", "old", "u");
        op.edit_in_place(&a, None, None, Some(Secret::new(edit_marker.to_string()))).expect("edit");
        let calls = requests.lock().unwrap().clone();
        assert_secret_only_on_stdin(&calls, edit_marker);
        let edit_call = calls.iter().find(|c| c.args.first().map(String::as_str) == Some("item") && c.args.get(1).map(String::as_str) == Some("edit"));
        let edit_call = edit_call.expect("an item edit call was made");
        assert!(edit_call.stdin.as_deref().is_some_and(|s| s.contains(edit_marker)), "the patched template carries the new password");
        assert!(edit_call.timeout.is_none(), "op item edit has no timeout");

        // LastPass: the LPASS_DISABLE_PINENTRY env pair rides along on every call, and no
        // account/add/login call ever times out except ls and sync now.
        let mut runner = FakeRunner::default();
        runner.push(0, "iTerm2\t7\tsite\tbob", "");
        let mut lp = LastPass::new(runner, "lpass".to_string(), vec!["iTerm2".to_string()], Box::new(|_, _| None));
        lp.accounts().expect("accounts");
        let calls = lp.runner.calls();
        let ls_call = &calls[0];
        assert!(ls_call.env.iter().any(|(k, v)| k == "LPASS_DISABLE_PINENTRY" && v == "1"));
        assert_eq!(ls_call.timeout, Some(LASTPASS_LIST_TIMEOUT));
    }

    #[test]
    fn cli_parsing_follows_iterm2() {
        let two = "{\"id\":\"1\",\"title\":\"a\",\"fields\":[{\"id\":\"username\",\"value\":\"u1\"}]}\n{\"id\":\"2\",\"title\":\"b\",\"fields\":[]}";
        let accounts = op_accounts(two).expect("parses");
        assert_eq!(accounts.len(), 2);
        assert_eq!(accounts[0].id, "1");
        assert_eq!(accounts[0].account_name, "a");
        assert_eq!(accounts[0].user_name, "u1");
        assert_eq!(accounts[0].display, format!("a{SEP}u1"));
        assert_eq!(accounts[1].id, "2");
        assert_eq!(accounts[1].user_name, "");

        // A real op vault entry: pretty-printed, CRLF-joined between items, with unrelated keys
        // (section, reference, label, notesPlain) op's own output carries, none of which should
        // trip the parser or the item-boundary split.
        let realistic = "{\r\n  \"id\": \"abc123\",\r\n  \"title\": \"GitHub\",\r\n  \"category\": \"LOGIN\",\r\n  \"tags\": [\"iTerm2\"],\r\n  \"section\": {\"id\": \"linked items\"},\r\n  \"fields\": [\r\n    {\"id\": \"username\", \"type\": \"STRING\", \"purpose\": \"USERNAME\", \"label\": \"username\", \"value\": \"octocat\", \"reference\": \"op://x/y/username\"},\r\n    {\"id\": \"password\", \"type\": \"CONCEALED\", \"purpose\": \"PASSWORD\", \"label\": \"password\", \"value\": \"hunter2\", \"reference\": \"op://x/y/password\"},\r\n    {\"id\": \"notesPlain\", \"type\": \"STRING\", \"purpose\": \"NOTES\", \"label\": \"notesPlain\", \"value\": \"\"}\r\n  ]\r\n}\r\n{\r\n  \"id\": \"def456\",\r\n  \"title\": \"Second\",\r\n  \"fields\": []\r\n}";
        let accounts = op_accounts(realistic).expect("parses a realistic, CRLF-joined fixture");
        assert_eq!(accounts.len(), 2);
        assert_eq!(accounts[0].id, "abc123");
        assert_eq!(accounts[0].account_name, "GitHub");
        assert_eq!(accounts[0].user_name, "octocat");
        assert_eq!(accounts[1].id, "def456");

        let no_otp = r#"{"id":"3","title":"c","tags":["iTerm2-no-otp"],"fields":[{"id":"secret","type":"OTP","totp":"123456"}]}"#;
        let a = &op_accounts(no_otp).unwrap()[0];
        assert!(a.has_otp);
        assert!(!a.send_otp);

        let with_otp = r#"{"id":"4","title":"d","fields":[{"id":"secret","type":"OTP","totp":"999999"}]}"#;
        let a = &op_accounts(with_otp).unwrap()[0];
        assert!(a.has_otp);
        assert!(a.send_otp);

        let password_item = r#"{"id":"5","title":"e","fields":[{"id":"password","value":"hunter2\r"},{"id":"secret","type":"OTP","totp":"246810"}]}"#;
        let (pw, otp) = op_password(password_item).unwrap();
        assert_eq!(pw.expose(), "hunter2");
        assert_eq!(otp.unwrap().expose(), "246810");

        let credential_item = r#"{"id":"6","title":"f","fields":[{"id":"credential","value":"apikey"}]}"#;
        let (pw, otp) = op_password(credential_item).unwrap();
        assert_eq!(pw.expose(), "apikey");
        assert!(otp.is_none());

        assert!(op_version_ok("2.23.0"));
        assert!(!op_version_ok("2.22.9"));
        assert!(op_version_ok("2.30"));

        assert_eq!(lpass_groups(""), vec!["iTerm2".to_string()]);
        assert_eq!(lpass_groups("Work, bad!,Home"), vec!["Work".to_string(), "Home".to_string()]);

        let groups = vec!["iTerm2".to_string()];
        let ls = "iTerm2\t0\tskipped\tu\niTerm2\t9\tkept\tuser\nOther\t8\tother-group\tu2";
        let accounts = lpass_accounts(ls, &groups);
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0].id, "9");
        assert_eq!(accounts[0].account_name, "kept");
        assert_eq!(accounts[0].user_name, "user");
        assert_eq!(accounts[0].display, format!("kept{SEP}user"));

        // A CRLF `lpass ls` line never leaves a trailing "\r" in the user name.
        let crlf_ls = "iTerm2\t9\tkept\tuser\r\n";
        let accounts = lpass_accounts(crlf_ls, &groups);
        assert_eq!(accounts[0].user_name, "user");
    }

    #[test]
    fn one_password_edit_in_place_round_trips() {
        let original = r#"{"id":"1","title":"old","fields":[{"id":"username","value":"u"},{"id":"password","value":"p"},{"id":"notes","value":"kept"}]}"#;
        let mut runner = FakeRunner::default();
        runner.push(0, "", ""); // op user get --me
        runner.push(0, original, "");
        runner.push(0, "", ""); // op item edit
        let mut op = OnePassword::new(runner, "op".to_string(), None, Box::new(|_| None));
        let a = account("1", "old", "u");
        op.edit_in_place(&a, Some("new"), None, Some(Secret::new("newpw".to_string()))).expect("edit");
        let calls = op.runner.calls();
        let last = calls.last().expect("edit call");
        let patched = last.stdin.clone().expect("patched template on stdin");
        assert!(patched.contains("\"title\":\"new\""));
        assert!(patched.contains("\"value\":\"newpw\""));
        assert!(patched.contains("\"value\":\"u\""), "the untouched user name field survives");
        assert!(patched.contains("kept"), "an unrelated field is never dropped");
    }

    #[test]
    fn one_password_retries_once_after_an_expired_session() {
        let mut runner = FakeRunner::default();
        runner.push(0, "", ""); // op user get --me: biometric unlock
        runner.push(1, "", "[ERROR] 2024/01/01 00:00:00 You are not currently signed in. Please run `op signin --raw`."); // item get <id>: session expired
        runner.push(1, "", "no longer unlocked"); // op user get --me (retry): biometric no longer available
        runner.push(0, "fresh-token\n", ""); // op signin --raw
        runner.push(0, r#"{"id":"1","title":"a","fields":[{"id":"password","value":"pw"}]}"#, ""); // item get <id>: retried, succeeds
        let mut op = OnePassword::new(runner, "op".to_string(), None, Box::new(|_| Some(Secret::new("master".to_string()))));
        let a = account("1", "a", "u");
        let (pw, _) = op.password(&a).expect("recovers after one retry");
        assert_eq!(pw.expose(), "pw");
        let calls = op.runner.calls();
        assert_eq!(calls.len(), 5, "get-me, failed get, get-me retry, signin, retried get: {calls:?}");
    }

    #[test]
    fn one_password_available_checks_version_without_authenticating() {
        let mut runner = FakeRunner::default();
        runner.push(0, "2.23.0\n", "");
        let mut op = OnePassword::new(runner, "op".to_string(), None, Box::new(|_| panic!("available must never prompt")));
        op.available().expect("2.23.0 meets OP_MIN");
        assert_eq!(op.runner.calls().len(), 1, "only the version probe, no user get --me");

        let mut runner = FakeRunner::default();
        runner.push(0, "2.5.0\n", "");
        let mut op = OnePassword::new(runner, "op".to_string(), None, Box::new(|_| panic!("available must never prompt")));
        assert_eq!(op.available().unwrap_err(), op_too_old_message("op"));

        let mut runner = FakeRunner::default();
        runner.push_err("could not start: not found");
        let mut op = OnePassword::new(runner, "missing-op".to_string(), None, Box::new(|_| panic!("available must never prompt")));
        assert_eq!(op.available().unwrap_err(), OP_NOT_FOUND_MESSAGE);
    }

    #[test]
    fn one_password_timeout_maps_to_the_fixed_text() {
        let mut runner = FakeRunner::default();
        runner.push(0, "", ""); // op user get --me
        runner.push_err(TIMED_OUT); // item list
        let mut op = OnePassword::new(runner, "op".to_string(), None, Box::new(|_| None));
        assert_eq!(op.accounts().unwrap_err(), OP_TIMEOUT_MESSAGE);
    }

    #[test]
    fn one_password_never_shows_raw_stderr() {
        let secret_stderr = "leaked-master-password-in-stderr";
        let mut runner = FakeRunner::default();
        runner.push(1, "", "no desktop unlock"); // op user get --me fails
        runner.push(1, "", secret_stderr); // op signin --raw fails, stderr must never surface
        let mut op = OnePassword::new(runner, "op".to_string(), None, Box::new(|_| Some(Secret::new("master".to_string()))));
        let err = op.password(&account("1", "a", "u")).unwrap_err();
        assert_eq!(err, OP_SIGNIN_FAILED_MESSAGE);
        assert!(!err.contains(secret_stderr));

        let mut runner = FakeRunner::default();
        runner.push(0, "", ""); // op user get --me
        runner.push(1, "", secret_stderr); // item get fails
        let mut op = OnePassword::new(runner, "op".to_string(), None, Box::new(|_| None));
        let err = op.password(&account("1", "a", "u")).unwrap_err();
        assert_eq!(err, OP_RUNTIME_ERROR);
        assert!(!err.contains(secret_stderr));
    }

    #[test]
    fn last_pass_accounts_and_add() {
        let mut runner = FakeRunner::default();
        runner.push(0, "iTerm2\t7\tsite\tbob", "");
        let groups = vec!["iTerm2".to_string()];
        let mut lp = LastPass::new(runner, "lpass".to_string(), groups, Box::new(|_, _| None));
        let accounts = lp.accounts().expect("accounts");
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0].account_name, "site");

        let mut runner = FakeRunner::default();
        runner.push(0, "", ""); // add
        runner.push(0, "", ""); // sync now
        runner.push(0, "OK\n7\n", ""); // show --id
        let groups = vec!["iTerm2".to_string()];
        let mut lp = LastPass::new(runner, "lpass".to_string(), groups, Box::new(|_, _| None));
        let a = lp.add("site", "bob", Secret::new("hunter2".to_string()), &[]).expect("add");
        assert_eq!(a.id, "7");
        let calls = lp.runner.calls();
        assert_secret_only_on_stdin(&calls, "hunter2");
        let add_call = &calls[0];
        assert_eq!(add_call.args, vec!["add".to_string(), "iTerm2/site".to_string(), "--non-interactive".to_string()]);
        assert_eq!(add_call.stdin.as_deref(), Some("Username: bob\nPassword: hunter2"));
        assert!(add_call.timeout.is_none(), "lpass add has no timeout");
        let sync_call = calls.iter().find(|c| c.args.first().map(String::as_str) == Some("sync")).expect("a sync now call");
        assert_eq!(sync_call.timeout, Some(LASTPASS_LIST_TIMEOUT));
    }

    #[test]
    fn last_pass_login_never_shows_raw_stderr_and_needs_login_is_distinguished() {
        let secret_stderr = "leaked-lastpass-password";
        let mut runner = FakeRunner::default();
        runner.push(1, "Not logged in as anyone", ""); // status: nonzero when not logged in
        runner.push(1, "", secret_stderr); // login fails
        let mut lp = LastPass::new(runner, "lpass".to_string(), vec!["iTerm2".to_string()], Box::new(|_, _| Some(("me@example.com".to_string(), Secret::new("pw".to_string())))));
        let err = lp.available().unwrap_err();
        assert_eq!(err, lp_auth_failed_message("me@example.com"));
        assert!(!err.contains(secret_stderr));

        let mut runner = FakeRunner::default();
        runner.push(1, "", "Could not find decryption key. Perhaps you need to login with `lpass login`?");
        let mut lp = LastPass::new(runner, "lpass".to_string(), vec!["iTerm2".to_string()], Box::new(|_, _| None));
        assert_eq!(lp.password(&account("1", "a", "u")).unwrap_err(), LP_NEEDS_LOGIN);
    }

    #[test]
    fn last_pass_timeout_maps_to_the_fixed_text() {
        let mut runner = FakeRunner::default();
        runner.push_err(TIMED_OUT);
        let mut lp = LastPass::new(runner, "lpass".to_string(), vec!["iTerm2".to_string()], Box::new(|_, _| None));
        assert_eq!(lp.accounts().unwrap_err(), lp_timeout_message(LP_ACCOUNT_LIST_FETCH_FAILED));
    }

    #[test]
    fn op_paths_edge_cases() {
        let quoted = op_paths(r#""C:\Program Files\op"; C:\tools "#, r"C:\Users\me\AppData\Local");
        assert!(quoted.contains(&r"C:\Program Files\op\op.exe".to_string()), "{quoted:?}");
        assert!(quoted.contains(&r"C:\tools\op.exe".to_string()), "{quoted:?}");
        assert!(quoted.last().unwrap().ends_with(r"\Microsoft\WinGet\Links\op.exe"));

        let no_local_app_data = op_paths(r"C:\tools", "");
        assert!(!no_local_app_data.iter().any(|p| p.starts_with('\\')), "an empty LOCALAPPDATA never yields a rooted candidate: {no_local_app_data:?}");
        assert_eq!(no_local_app_data, vec![r"C:\tools\op.exe".to_string()]);
    }

    #[test]
    fn resolve_op_follows_iterm2() {
        // Found and new enough: the first existing, usable candidate wins.
        let mut runner = FakeRunner::default();
        runner.push(0, "2.23.0\n", "");
        let candidates = vec![r"C:\a\op.exe".to_string(), r"C:\b\op.exe".to_string()];
        let mut exists = |p: &str| p == r"C:\a\op.exe";
        assert_eq!(resolve_op(&mut runner, &candidates, &mut exists), Ok(r"C:\a\op.exe".to_string()));

        // Present but too old: distinguished from not found at all.
        let mut runner = FakeRunner::default();
        runner.push(0, "2.5.0\n", "");
        let mut exists = |p: &str| p == r"C:\a\op.exe";
        let err = resolve_op(&mut runner, &candidates, &mut exists).unwrap_err();
        assert_eq!(err, OpNotUsable::TooOld { path: r"C:\a\op.exe".to_string(), version: "2.5.0".to_string() });
        assert_eq!(op_not_usable_message(&err), op_too_old_message(r"C:\a\op.exe"));

        // Nothing exists at all.
        let mut runner = FakeRunner::default();
        let mut exists = |_: &str| false;
        let err = resolve_op(&mut runner, &candidates, &mut exists).unwrap_err();
        assert_eq!(err, OpNotUsable::NotFound);
        assert_eq!(op_not_usable_message(&err), OP_NOT_FOUND_MESSAGE);

        assert_eq!(lpass_paths(), Vec::<String>::new());
        assert!(resolve_lpass().is_err());
    }
}
