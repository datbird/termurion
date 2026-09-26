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

/// Wraps `op item get -`'s "phony json" (one bare object per item, separated by `"}\n{"`, never
/// a real array) into one valid JSON array (`OP:283`).
pub fn op_split_objects(stdout: &str) -> String {
    format!("[{}]", stdout.replace("}\n{", "},\n{"))
}

#[derive(Deserialize)]
struct OpField {
    id: String,
    #[serde(rename = "type")]
    kind: Option<String>,
    value: Option<String>,
    totp: Option<String>,
}

#[derive(Deserialize)]
struct OpItem {
    id: String,
    title: String,
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
/// is false when the item carries `NO_OTP_TAG`.
pub fn op_accounts(json: &str) -> Result<Vec<Account>, String> {
    let items: Vec<OpItem> = serde_json::from_str(&op_split_objects(json)).map_err(|e| e.to_string())?;
    Ok(items
        .into_iter()
        .map(|item| {
            let user = op_field(&item.fields, "username").and_then(|f| f.value.clone()).unwrap_or_default();
            let has_otp = item.fields.iter().any(|f| f.kind.as_deref() == Some("OTP"));
            let no_otp = item.tags.as_ref().is_some_and(|t| t.iter().any(|s| s == NO_OTP_TAG));
            Account {
                id: item.id,
                account_name: item.title.clone(),
                user_name: user.clone(),
                display: format!("{}{SEP}{}", item.title, user),
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

/// Each `PATH` entry's `op.exe`, then WinGet's link folder, newest-tried-last-installed-first is
/// not attempted here (this milestone does not shell out to compare versions across candidates;
/// the caller resolves and keeps one path for the run, as iTerm2 keeps `_customPathToCLI`).
pub fn op_paths(path_env: &str, local_app_data: &str) -> Vec<String> {
    let mut out: Vec<String> = path_env.split(';').filter(|s| !s.is_empty()).map(|dir| format!("{}\\op.exe", dir.trim_end_matches('\\'))).collect();
    out.push(format!("{}\\Microsoft\\WinGet\\Links\\op.exe", local_app_data.trim_end_matches('\\')));
    out
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

/// Parses `lpass ls`'s tab-separated lines (group, id, name, user): an id of "0" is an unsynced
/// account with no stable identifier and is skipped, and only a listed group is kept
/// (`LP:150-176`).
pub fn lpass_accounts(stdout: &str, groups: &[String]) -> Vec<Account> {
    stdout
        .split('\n')
        .filter_map(|line| {
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

// --- Alert texts (Resume notes; "iTerm2" replaced by "Tandem" in user-facing text, exact
// wording and punctuation otherwise, non-ASCII as \u{...} escapes) ---

pub const OP_UNAVAILABLE_TITLE: &str = "OnePassword Unavailable";
pub const OP_CANNOT_FIND_TITLE: &str = "Can\u{2019}t Find 1Password CLI";
pub const OP_CANNOT_FIND_MESSAGE: &str =
    "In order to use the 1Password integration, Tandem needs to know where to find the CLI app named \u{201c}op\u{201d}. It\u{2019}s normally in /usr/local/bin. If you have installed it elsewhere, please select Locate to provide its location.";
pub const OP_LOCATE: &str = "Locate";
pub const OP_NOT_FOUND_MESSAGE: &str = "The 1Password CLI could not be found, or is older than the required version 2.23. Check that a current op is installed.";
pub const OP_MASTER_PASSWORD_PROMPT: &str = "Enter your 1Password master password:";

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

/// "The LastPass service took too long to respond. <message>" (`LP:313`).
pub fn lp_timeout_message(context: &str) -> String {
    format!("The LastPass service took too long to respond. {context}")
}

// --- OnePassword and LastPass as `Source`s over a `Runner` ---

/// A short timeout for a normal `op`/`lpass` call; the LastPass account list gets its own
/// 5-second one, matching iTerm2 (`LP:150`).
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);
const LASTPASS_LIST_TIMEOUT: Duration = Duration::from_secs(5);

fn secret_env(key: &str, value: &str) -> (String, Secret) {
    (key.to_string(), Secret::new(value.to_string()))
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
    /// `path` is `op`'s already-resolved executable path (the caller finds it via `op_paths`
    /// and, failing that, a Locate file picker, as iTerm2's `OnePasswordUtils.pathToCLI` does;
    /// that search is not pure and lives with the app wiring, not here). `account` is the
    /// 1Password account to use when more than one is signed in (`--account`), or `None`.
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

    fn request(&self, args: Vec<String>) -> Request {
        Request { program: self.path.clone(), args, env: self.env(), stdin: None, timeout: Some(DEFAULT_TIMEOUT) }
    }

    /// `op user get --me` succeeding means the desktop app already unlocked 1Password, so no
    /// token is needed; otherwise the master password is asked for and traded for a session
    /// token with `op signin --raw` (`OT:328-408`). A no-op once authenticated this run.
    fn ensure_auth(&mut self) -> Result<(), String> {
        if !matches!(self.auth, Auth::None) {
            return Ok(());
        }
        let mut args = vec!["user".to_string(), "get".to_string(), "--me".to_string()];
        self.account_args(&mut args);
        let out = self.runner.run(Request { program: self.path.clone(), args, env: self.base_env(), stdin: None, timeout: Some(DEFAULT_TIMEOUT) })?;
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
        let out = self.runner.run(Request { program: self.path.clone(), args, env: self.base_env(), stdin: Some(stdin), timeout: Some(DEFAULT_TIMEOUT) })?;
        if out.code != 0 {
            return Err(out.stderr);
        }
        self.auth = Auth::Token(Secret::new(out.stdout.expose().trim().to_string()));
        Ok(())
    }

    fn item_tags(&mut self, id: &str) -> Result<Vec<String>, String> {
        let out = self.runner.run(self.request(op_get(id)))?;
        if out.code != 0 {
            return Err(out.stderr);
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

    fn available(&mut self) -> Result<(), String> {
        self.ensure_auth()
    }

    fn accounts(&mut self) -> Result<Vec<Account>, String> {
        self.ensure_auth()?;
        let (list_args, get_args) = op_list();
        let listed = self.runner.run(self.request(list_args))?;
        if listed.code != 0 {
            return Err(listed.stderr);
        }
        let mut piped = self.request(get_args);
        piped.stdin = Some(listed.stdout);
        let fetched = self.runner.run(piped)?;
        if fetched.code != 0 {
            return Err(fetched.stderr);
        }
        op_accounts(fetched.stdout.expose())
    }

    fn password(&mut self, a: &Account) -> Result<(Secret, Option<Secret>), String> {
        self.ensure_auth()?;
        let out = self.runner.run(self.request(op_get(&a.id)))?;
        if out.code != 0 {
            return Err(out.stderr);
        }
        op_password(out.stdout.expose())
    }

    fn set_password(&mut self, a: &Account, p: Secret) -> Result<(), String> {
        self.edit_in_place(a, None, None, Some(p))
    }

    fn delete(&mut self, a: &Account) -> Result<(), String> {
        self.ensure_auth()?;
        let out = self.runner.run(self.request(vec!["item".to_string(), "delete".to_string(), a.id.clone()]))?;
        if out.code != 0 {
            return Err(out.stderr);
        }
        Ok(())
    }

    fn add(&mut self, name: &str, user: &str, p: Secret, _flags: &[(String, bool)]) -> Result<Account, String> {
        // 1Password has no Add Account toggles, so flags is ignored (as iTerm2's `OP:527-528`).
        self.ensure_auth()?;
        let template = op_create_template(name, user, &p);
        let args = vec!["item".to_string(), "create".to_string(), "--format=json".to_string(), format!("--tags={OP_TAG}"), "-".to_string()];
        let mut request = self.request(args);
        request.stdin = Some(template);
        let out = self.runner.run(request)?;
        if out.code != 0 {
            return Err(out.stderr);
        }
        #[derive(Deserialize)]
        struct Created {
            id: String,
        }
        let created: Created = serde_json::from_str(out.stdout.expose()).map_err(|e| e.to_string())?;
        Ok(Account { id: created.id, account_name: name.to_string(), user_name: user.to_string(), display: format!("{name}{SEP}{user}"), has_otp: false, send_otp: false, source_label: None })
    }

    fn edit_in_place(&mut self, a: &Account, name: Option<&str>, user: Option<&str>, p: Option<Secret>) -> Result<(), String> {
        self.ensure_auth()?;
        // `--reveal` so the concealed password field carries its real value: the whole
        // template is resubmitted, so an unchanged password must round-trip as itself
        // (`OP:395-400`).
        let get_args = vec!["item".to_string(), "get".to_string(), a.id.clone(), "--reveal".to_string(), "--format=json".to_string()];
        let got = self.runner.run(self.request(get_args))?;
        if got.code != 0 {
            return Err(got.stderr);
        }
        let patched = op_patch_item(got.stdout.expose(), name, user, p.as_ref())?;
        let mut request = self.request(vec!["item".to_string(), "edit".to_string(), a.id.clone()]);
        request.stdin = Some(patched);
        let out = self.runner.run(request)?;
        if out.code != 0 {
            return Err(out.stderr);
        }
        Ok(())
    }

    fn toggle_otp(&mut self, a: &Account) -> Result<Account, String> {
        self.ensure_auth()?;
        let mut tags = self.item_tags(&a.id)?;
        if a.send_otp {
            if !tags.iter().any(|t| t == NO_OTP_TAG) {
                tags.push(NO_OTP_TAG.to_string());
            }
        } else {
            tags.retain(|t| t != NO_OTP_TAG);
        }
        let args = vec!["item".to_string(), "edit".to_string(), a.id.clone(), "--tags".to_string(), tags.join(",")];
        let out = self.runner.run(self.request(args))?;
        if out.code != 0 {
            return Err(out.stderr);
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

    fn request(&self, args: Vec<String>, timeout: Duration) -> Request {
        Request { program: self.path.clone(), args, env: Vec::new(), stdin: None, timeout: Some(timeout) }
    }

    /// Prompts for LastPass credentials and runs `lpass login` with the password on stdin
    /// (`LP:504-529`, `LastPassUtils.showLoginUI`).
    fn login(&mut self) -> Result<(), String> {
        let Some((user, password)) = (self.prompt)(LP_LOGIN_PROMPT, &self.last_username) else {
            return Err("Canceled.".to_string());
        };
        self.last_username = user.clone();
        let mut request = self.request(vec!["login".to_string(), "--color=never".to_string(), user], DEFAULT_TIMEOUT);
        request.stdin = Some(password);
        let out = self.runner.run(request)?;
        if out.code != 0 {
            return Err(out.stderr);
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
        let out = self.runner.run(self.request(vec!["status".to_string(), "--color=never".to_string()], DEFAULT_TIMEOUT))?;
        if out.code == 0 {
            return Ok(());
        }
        if out.stdout.expose().starts_with("Not logged in") {
            return self.login();
        }
        Err("Not logged in.".to_string())
    }

    fn accounts(&mut self) -> Result<Vec<Account>, String> {
        let out = self.runner.run(self.request(lpass_list(&self.groups), LASTPASS_LIST_TIMEOUT))?;
        if out.code != 0 {
            return Err(LP_ACCOUNT_LIST_FETCH_FAILED.to_string());
        }
        Ok(lpass_accounts(out.stdout.expose(), &self.groups))
    }

    fn password(&mut self, a: &Account) -> Result<(Secret, Option<Secret>), String> {
        let out = self.runner.run(self.request(vec!["show".to_string(), "--password".to_string(), a.id.clone()], DEFAULT_TIMEOUT))?;
        if out.code != 0 {
            return Err(LP_PASSWORD_FETCH_FAILED.to_string());
        }
        Ok((Secret::new(out.stdout.expose().trim().to_string()), None))
    }

    fn set_password(&mut self, a: &Account, p: Secret) -> Result<(), String> {
        let mut request = self.request(vec!["edit".to_string(), "--non-interactive".to_string(), "--password".to_string(), a.id.clone()], DEFAULT_TIMEOUT);
        request.stdin = Some(Secret::new(format!("{}\n", p.expose())));
        let out = self.runner.run(request)?;
        if out.code != 0 {
            return Err(LP_PASSWORD_SET_FAILED.to_string());
        }
        Ok(())
    }

    fn delete(&mut self, a: &Account) -> Result<(), String> {
        let out = self.runner.run(self.request(vec!["rm".to_string(), a.id.clone()], DEFAULT_TIMEOUT))?;
        if out.code != 0 {
            return Err(LP_ACCOUNT_DELETE_FAILED.to_string());
        }
        Ok(())
    }

    fn add(&mut self, name: &str, user: &str, p: Secret, _flags: &[(String, bool)]) -> Result<Account, String> {
        // LastPass has no Add Account toggles, so flags is ignored (as iTerm2's `LP:368`).
        let mut request = self.request(lpass_add(name), DEFAULT_TIMEOUT);
        request.stdin = Some(lpass_add_input(user, &p));
        let added = self.runner.run(request)?;
        if added.code != 0 {
            return Err(LP_ACCOUNT_ADD_FAILED.to_string());
        }
        let _ = self.runner.run(self.request(vec!["sync".to_string(), "now".to_string()], LASTPASS_LIST_TIMEOUT));
        let shown = self.runner.run(self.request(vec!["show".to_string(), "--id".to_string(), format!("iTerm2/{name}")], DEFAULT_TIMEOUT))?;
        if shown.code != 0 {
            return Err(LP_ACCOUNT_ADD_FAILED.to_string());
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
    use crate::pwm_run::Output;
    use std::sync::{Arc, Mutex};

    /// Records every `Request` it is given (program, args and env keys, never a secret's value
    /// unless the test itself reads one back through `Secret::expose`) and returns the next
    /// canned `Output` from its queue.
    #[derive(Default)]
    struct FakeRunner {
        requests: Arc<Mutex<Vec<(String, Vec<String>, Vec<(String, String)>, Option<String>)>>>,
        outputs: std::collections::VecDeque<(i32, String, String)>,
    }

    impl FakeRunner {
        fn push(&mut self, code: i32, stdout: &str, stderr: &str) {
            self.outputs.push_back((code, stdout.to_string(), stderr.to_string()));
        }

        fn calls(&self) -> Vec<(String, Vec<String>, Vec<(String, String)>, Option<String>)> {
            self.requests.lock().unwrap().clone()
        }
    }

    impl Runner for FakeRunner {
        fn run(&mut self, r: Request) -> Result<Output, String> {
            let env: Vec<(String, String)> = r.env.iter().map(|(k, v)| (k.clone(), v.expose().to_string())).collect();
            let stdin = r.stdin.as_ref().map(|s| s.expose().to_string());
            self.requests.lock().unwrap().push((r.program.clone(), r.args.clone(), env, stdin));
            let (code, stdout, stderr) = self.outputs.pop_front().ok_or_else(|| "FakeRunner: no more outputs queued".to_string())?;
            Ok(Output { code, stdout: Secret::new(stdout), stderr })
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

        // Adding an account puts the password only in the stdin template; no argument, and no
        // program name, ever contains it. A fake Runner captures the real Request the Source
        // impl builds.
        let secret_marker = "s3cr3t-marker";
        let mut runner = FakeRunner::default();
        let requests = runner.requests.clone();
        runner.push(0, "", ""); // op user get --me: biometric unlock, no signin needed
        runner.push(0, r#"{"id":"new-id"}"#, "");
        let mut op = OnePassword::new(runner, "op".to_string(), None, Box::new(|_| None));
        op.add("Example", "bob", Secret::new(secret_marker.to_string()), &[]).expect("add");
        for (program, args, _env, stdin) in requests.lock().unwrap().iter() {
            assert!(!program.contains(secret_marker));
            assert!(!args.iter().any(|a| a.contains(secret_marker)), "{args:?}");
            if args.first().map(String::as_str) == Some("item") && args.get(1).map(String::as_str) == Some("create") {
                assert_eq!(args, &vec!["item".to_string(), "create".to_string(), "--format=json".to_string(), "--tags=iTerm2".to_string(), "-".to_string()]);
                assert!(stdin.as_deref().is_some_and(|s| s.contains(secret_marker)), "the template carries the password");
            }
        }

        // The session token travels as the env pair OP_SESSION_my, never as an argument, once
        // signing in (no desktop app unlock) hands one back.
        let mut runner = FakeRunner::default();
        let requests = runner.requests.clone();
        runner.push(1, "", "not signed in"); // op user get --me fails: no desktop unlock
        runner.push(0, "the-token\n", ""); // op signin --raw
        runner.push(0, "[]", ""); // item list
        runner.push(0, "", ""); // item get -
        let mut op = OnePassword::new(runner, "op".to_string(), None, Box::new(|_| Some(Secret::new("master".to_string()))));
        op.accounts().expect("accounts");
        let calls = requests.lock().unwrap();
        let list_call = calls.iter().find(|(_, args, ..)| args.first().map(String::as_str) == Some("item") && args.get(1).map(String::as_str) == Some("list")).expect("an item list call");
        assert!(list_call.2.iter().any(|(k, v)| k == "OP_SESSION_my" && v == "the-token"), "{:?}", list_call.2);
        assert!(!list_call.1.iter().any(|a| a.contains("the-token")));
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
        let (_, _, _, stdin) = calls.last().expect("edit call");
        let patched = stdin.clone().expect("patched template on stdin");
        assert!(patched.contains("\"title\":\"new\""));
        assert!(patched.contains("\"value\":\"newpw\""));
        assert!(patched.contains("\"value\":\"u\""), "the untouched user name field survives");
        assert!(patched.contains("kept"), "an unrelated field is never dropped");
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
        assert!(!calls.iter().any(|(program, args, _, _)| program.contains("hunter2") || args.iter().any(|a| a.contains("hunter2"))));
        let add_call = &calls[0];
        assert_eq!(add_call.1, vec!["add".to_string(), "iTerm2/site".to_string(), "--non-interactive".to_string()]);
        assert_eq!(add_call.3.as_deref(), Some("Username: bob\nPassword: hunter2"));
    }
}
