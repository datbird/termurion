//! The adapter protocol: the JSON wire types and process convention iTerm2 defines for its
//! bundled KeePassXC, Bitwarden and Keeper adapters (`~/gitrepos/iterm2-ref/pwmplugin/Sources/
//! PasswordManagerProtocol/PasswordManagerProtocol.swift`, `PR`), and `Adapter<R>`, a `Source`
//! that speaks it over a `pwm_run::Runner`. Every call runs the adapter executable once, with the
//! subcommand name ("handshake", "login", "list-accounts", ...) as its one argument and the
//! request encoded as JSON on stdin; the adapter answers with JSON on stdout, either the expected
//! response or an `ErrorResponse` (`AP:124-177`).
//!
//! Tandem ships all three adapter executables (`src/bin/iterm2-*-adapter.rs`) beside
//! `termurion.exe`; the wire types here derive both `Serialize` and `Deserialize` so the app and the
//! executables share one definition. When an executable is missing, `available()` fails with
//! "Adapter not found." and the window falls back to Credential Manager, as iTerm2 falls back
//! when a source's own availability check fails (`P:600-668`).
//!
//! A master password or session token is a `pwm::Secret` from the moment it is read to the
//! moment it is written to the adapter's stdin and dropped; it is a plain `String` inside the
//! request structs below only because that is what the wire protocol carries (as iTerm2's own
//! `Codable` types do), so no request or response type that can carry one derives `Debug`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::pwm::{Account as PwmAccount, Secret, SEP};
use crate::pwm_run::{Request, Runner};
use crate::pwm_source::{Caps, Field, Kind, Scope, Source, Toggle};

/// The adapter protocol's version this build speaks (`PR:5`, "Protocol Version: 0"). The
/// handshake below accepts only a reply that echoes it back (`AP:195`).
pub const PROTOCOL: i64 = 0;

/// `RequestHeader.mode` of the terminal password manager (`PR:152-155`).
const MODE_TERMINAL: &str = "terminal";

/// `RequestHeader.mode` for a scope: `"terminal"`, or `"browser"` for the browser password
/// manager (`ADS:314-318`).
pub fn mode_for(scope: Scope) -> &'static str {
    match scope {
        Scope::Terminal => MODE_TERMINAL,
        Scope::Browser => "browser",
    }
}

/// A short timeout for one adapter round trip.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

/// The timeout of a subcommand that opens or syncs the password manager's store (KeePassXC
/// derives the key again for every `keepassxc-cli` call, and a listing makes one per entry).
const LONG_TIMEOUT: Duration = Duration::from_secs(180);

/// A single read (Keeper's service answers a get in seconds, polling up to 15 s).
const GET_TIMEOUT: Duration = Duration::from_secs(30);

fn timeout_for(subcommand: &str) -> Duration {
    match subcommand {
        "login" | "list-accounts" | "add-account" | "set-password" | "delete-account" | "sync-down" => LONG_TIMEOUT,
        "get-password" => GET_TIMEOUT,
        _ => DEFAULT_TIMEOUT,
    }
}

// --- Wire types (`PR:12-343`), iTerm2's camelCase JSON names verbatim ---

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PathKind {
    File,
    Url,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomCommand {
    pub name: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
}

/// Names a pre-existing keychain entry an adapter wants migrated into its master-password slot.
/// Tandem never persists adapter credentials (a Decision below), so it is decoded and ignored.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LegacyCredentialMigration {
    pub from_keychain_account: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct UserAccount {
    pub name: String,
    pub identifier: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HandshakeRequest {
    pub i_term_version: String,
    pub min_protocol_version: i64,
    pub max_protocol_version: i64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HandshakeResponse {
    pub protocol_version: i64,
    pub name: String,
    pub requires_master_password: bool,
    pub can_set_passwords: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_accounts: Option<Vec<UserAccount>>,
    pub needs_path_to_database: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub database_extension: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub needs_path_to_executable: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path_to_database_kind: Option<PathKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path_to_database_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path_to_database_placeholder: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub master_password_label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub persists_credentials: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_commands: Option<Vec<CustomCommand>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings_fields: Option<Vec<Field>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub add_account_toggles: Option<Vec<Toggle>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub legacy_credential_migrations: Option<Vec<LegacyCredentialMigration>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub can_edit_in_place: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requires_password_for_add: Option<bool>,
}

/// `PR:146-156`. `settings` carries every settings field's current value, secret or not: iTerm2
/// merges them all into one dictionary (`AP:319-329`) rather than keeping secret ones out of the
/// request, so Tandem does the same.
#[derive(Serialize, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestHeader {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path_to_database: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path_to_executable: Option<String>,
    pub mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub settings: Option<HashMap<String, String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountIdentifier {
    #[serde(rename = "accountID")]
    pub account_id: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub identifier: AccountIdentifier,
    pub user_name: String,
    pub account_name: String,
    #[serde(rename = "hasOTP")]
    pub has_otp: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_label: Option<String>,
}

// -- Login: the only request that ever carries a master password, and the only response that
// ever hands back a fresh session token, so neither derives `Debug`.

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginRequest {
    pub header: RequestHeader,
    #[serde(rename = "userAccountID", skip_serializing_if = "Option::is_none")]
    pub user_account_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub master_password: Option<String>,
}

#[derive(Deserialize, Serialize)]
pub struct LoginResponse {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListAccountsRequest {
    pub header: RequestHeader,
    #[serde(rename = "userAccountID", skip_serializing_if = "Option::is_none")]
    pub user_account_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ListAccountsResponse {
    pub accounts: Vec<Account>,
    /// A non-fatal listing issue (`AP:690-700`): `accounts` keeps it, trimmed, for
    /// `Source::take_warning`, through which the app shows it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetPasswordRequest {
    pub header: RequestHeader,
    #[serde(rename = "userAccountID", skip_serializing_if = "Option::is_none")]
    pub user_account_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    pub account_identifier: AccountIdentifier,
}

#[derive(Deserialize, Serialize)]
pub struct Password {
    pub password: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub otp: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetPasswordRequest {
    pub header: RequestHeader,
    #[serde(rename = "userAccountID", skip_serializing_if = "Option::is_none")]
    pub user_account_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    pub account_identifier: AccountIdentifier,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_password: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_account_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_user_name: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct SetPasswordResponse {}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteAccountRequest {
    pub header: RequestHeader,
    #[serde(rename = "userAccountID", skip_serializing_if = "Option::is_none")]
    pub user_account_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    pub account_identifier: AccountIdentifier,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_label: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct DeleteAccountResponse {}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddAccountRequest {
    pub header: RequestHeader,
    #[serde(rename = "userAccountID", skip_serializing_if = "Option::is_none")]
    pub user_account_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    pub user_name: String,
    pub account_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub flags: Option<HashMap<String, bool>>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AddAccountResponse {
    pub account_identifier: AccountIdentifier,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomCommandRequest {
    pub header: RequestHeader,
    #[serde(rename = "userAccountID", skip_serializing_if = "Option::is_none")]
    pub user_account_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    pub command_name: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct CustomCommandResponse {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ErrorResponse {
    pub error: String,
    /// A response flagged with this clears the session token and retries once instead of
    /// showing an error (`AP:53-62`, `AP:656-664` and the other recipes' matching blocks).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub needs_authentication: Option<bool>,
}

// --- Errors (`AP:14-73`) ---

/// The adapter side's error shapes, each carrying iTerm2's own text (`AP:26-51`, "Tandem" for
/// "iTerm2"). `Source` methods return `String` (the trait's error type), so `text()` is the last
/// step before a `Result` leaves this module; nothing here is ever shown via `Debug`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdapterError {
    /// The adapter rejected the login attempt; the message is its own reported reason.
    Login(String),
    NotAuthenticated,
    InvalidOutput,
    Handshake,
    Incompatible,
    NotFound,
    InvalidToken,
    /// Any other adapter-reported or transport failure, verbatim.
    Other(String),
}

impl AdapterError {
    pub fn text(&self) -> String {
        match self {
            AdapterError::Login(message) => format!("Could not log in: {message}"),
            AdapterError::NotAuthenticated => "Not authenticated.".to_string(),
            AdapterError::InvalidOutput => "Invalid output.".to_string(),
            AdapterError::Handshake => "Handshake failed.".to_string(),
            AdapterError::Incompatible => "Incompatible protocol. Please update Tandem.".to_string(),
            AdapterError::NotFound => "Adapter not found.".to_string(),
            AdapterError::InvalidToken => "Authentication failed. Log in again.".to_string(),
            AdapterError::Other(message) => message.clone(),
        }
    }
}

// --- Locating a bundled adapter ---

/// The executable Tandem looks for next to `termurion.exe` for each adapter-backed source; `None`
/// for a source that is not an adapter.
pub fn adapter_file(kind: Kind) -> Option<&'static str> {
    match kind {
        Kind::KeePassXC => Some("iterm2-keepassxc-adapter.exe"),
        Kind::Bitwarden => Some("iterm2-bitwarden-adapter.exe"),
        Kind::Keeper => Some("iterm2-keeper-adapter.exe"),
        Kind::CredentialManager | Kind::OnePassword | Kind::LastPass => None,
    }
}

/// `exe_dir` joined with `adapter_file(kind)`, or `None` for a non-adapter `Kind`.
pub fn adapter_path(kind: Kind, exe_dir: &Path) -> Option<PathBuf> {
    adapter_file(kind).map(|f| exe_dir.join(f))
}

/// The short name shown for an adapter-backed source (`Source::name`, as `OnePassword::name`
/// returns "1Password" rather than `Kind::menu_title`'s "1Password Integration").
fn short_name(kind: Kind) -> &'static str {
    match kind {
        Kind::KeePassXC => "KeePassXC",
        Kind::Bitwarden => "Bitwarden",
        Kind::Keeper => "Keeper",
        Kind::CredentialManager | Kind::OnePassword | Kind::LastPass => "Adapter",
    }
}

/// `AdapterCapabilities` and the handshake's own flags, turned into a `Caps` (`AP:1006-1122`).
/// `canResetConfiguration` is always true for an adapter (`AP:1010`), unlike a CLI source.
pub fn caps_from_handshake(h: &HandshakeResponse) -> Caps {
    Caps {
        in_place: h.can_edit_in_place.unwrap_or(false),
        can_edit_password: h.can_set_passwords,
        requires_password: h.requires_password_for_add.unwrap_or(false),
        generated_only: false,
        multiple_accounts: h.user_accounts.is_some(),
        can_reset: true,
        add_toggle: h.add_account_toggles.as_ref().and_then(|v| v.first().cloned()),
        settings: h.settings_fields.clone().unwrap_or_default(),
        commands: h.custom_commands.as_ref().map(|v| v.iter().map(|c| (c.name.clone(), c.label.clone())).collect()).unwrap_or_default(),
    }
}

/// `ListAccountsResponse.accounts` turned into `pwm::Account`s (`AP:679-688`): the display
/// string is the account and user name joined by `SEP`, matching `CommandLinePasswordDataSource`
/// (which `AdapterPasswordDataSource` extends, `CL:24-30`) - the source label is shown by
/// `pwm::row_name` at render time, not folded into the sort key here. `send_otp` always equals
/// `has_otp`: the adapter protocol has no per-account opt-out (unlike 1Password's tag).
pub fn map_accounts(resp: ListAccountsResponse) -> Vec<PwmAccount> {
    resp.accounts
        .into_iter()
        .map(|entry| PwmAccount {
            id: entry.identifier.account_id,
            display: format!("{}{SEP}{}", entry.account_name, entry.user_name),
            account_name: entry.account_name,
            user_name: entry.user_name,
            has_otp: entry.has_otp,
            send_otp: entry.has_otp,
            source_label: entry.source_label,
        })
        .collect()
}

/// Asks for an adapter's master password; `None` means the user canceled. The real prompt is a
/// `prompt_ui::PromptWindow` wired in by the app layer that constructs `Adapter`, as with
/// `pwm_cli::MasterPasswordPrompt`.
pub type MasterPasswordPrompt = Box<dyn FnMut(&str) -> Option<Secret> + Send>;

/// The protocol's own subcommands, which a custom command may not name (`AP:1152-1156`).
const BUILT_IN_SUBCOMMANDS: [&str; 7] = ["handshake", "login", "list-accounts", "get-password", "set-password", "add-account", "delete-account"];

/// One of the bundled adapters (KeePassXC, Bitwarden or Keeper) as a `Source`, speaking the
/// protocol above over `runner`. `path_to_database`, `path_to_executable` and `settings` are
/// already-resolved values the caller supplies (as `OnePassword::new` takes an already-resolved
/// `op` path): finding and persisting them is app-layer work, not this module's.
pub struct Adapter<R: Runner> {
    runner: R,
    kind: Kind,
    path: PathBuf,
    path_to_database: Option<String>,
    path_to_executable: Option<String>,
    settings: HashMap<String, String>,
    user_account_id: Option<String>,
    prompt: MasterPasswordPrompt,
    handshake: Option<HandshakeResponse>,
    /// The session token from `login`, kept only for this run (never written to disk).
    token: Option<Secret>,
    /// The last account list's warning, trimmed and non-empty, until `take_warning`.
    warning: Option<String>,
    scope: Scope,
}

impl<R: Runner> Adapter<R> {
    pub fn new(
        runner: R,
        kind: Kind,
        path: PathBuf,
        path_to_database: Option<String>,
        path_to_executable: Option<String>,
        settings: HashMap<String, String>,
        user_account_id: Option<String>,
        prompt: MasterPasswordPrompt,
    ) -> Adapter<R> {
        Adapter { runner, kind, path, path_to_database, path_to_executable, settings, user_account_id, prompt, handshake: None, token: None, warning: None, scope: Scope::Terminal }
    }

    /// Serves one password manager scope: its requests carry that scope's `mode`.
    pub fn with_scope(mut self, scope: Scope) -> Adapter<R> {
        self.scope = scope;
        self
    }

    /// `standardHeader` (`AP:314-329`): only the settings the handshake declared, and none
    /// when that leaves nothing.
    fn header(&self) -> RequestHeader {
        let declared = self.handshake.as_ref().and_then(|h| h.settings_fields.as_deref()).unwrap_or_default();
        let settings: HashMap<String, String> = declared.iter().filter_map(|f| self.settings.get(&f.key).map(|v| (f.key.clone(), v.clone()))).collect();
        RequestHeader {
            path_to_database: self.path_to_database.clone(),
            path_to_executable: self.path_to_executable.clone(),
            mode: mode_for(self.scope).to_string(),
            settings: if settings.is_empty() { None } else { Some(settings) },
        }
    }

    fn token_value(&self) -> Option<String> {
        self.token.as_ref().map(|t| t.expose().to_string())
    }

    /// One process run: `subcommand` is the adapter's one argument, `request` is encoded as JSON
    /// and sent on stdin as a `Secret` (`AP:124-177`). An `ErrorResponse` decoded from stdout
    /// wins over a non-zero exit code, exactly as iTerm2 checks it.
    fn run<Req: Serialize, Resp: for<'de> Deserialize<'de>>(&mut self, subcommand: &str, request: &Req) -> Result<Resp, AdapterError> {
        let body = serde_json::to_string(request).map_err(|_| AdapterError::InvalidOutput)?;
        let req = Request {
            program: self.path.to_string_lossy().into_owned(),
            args: vec![subcommand.to_string()],
            env: Vec::new(),
            stdin: Some(Secret::new(body)),
            timeout: Some(timeout_for(subcommand)),
            interactive: None,
        };
        let out = self.runner.run(req).map_err(AdapterError::Other)?;
        let stdout = out.stdout.expose();
        if let Ok(err) = serde_json::from_str::<ErrorResponse>(stdout) {
            return Err(if err.needs_authentication == Some(true) { AdapterError::NotAuthenticated } else { AdapterError::Other(err.error) });
        }
        if out.code != 0 {
            return Err(AdapterError::Other(format!("Adapter returned code {}", out.code)));
        }
        serde_json::from_str(stdout).map_err(|_| AdapterError::InvalidOutput)
    }

    /// Runs `handshake` once per `Adapter` and keeps the reply; a protocol version other than
    /// `PROTOCOL` is `Incompatible` (`AP:185-204`).
    fn ensure_handshake(&mut self) -> Result<(), AdapterError> {
        if self.handshake.is_some() {
            return Ok(());
        }
        let request = HandshakeRequest { i_term_version: crate::reports::VERSION.to_string(), min_protocol_version: PROTOCOL, max_protocol_version: PROTOCOL };
        let response: HandshakeResponse = self.run("handshake", &request)?;
        if response.protocol_version != PROTOCOL {
            return Err(AdapterError::Incompatible);
        }
        self.handshake = Some(response);
        Ok(())
    }

    /// Handshakes if needed, then logs in if there is no session token yet, prompting for a
    /// master password only when the handshake says one is required (`AP:459-525`). Tandem never
    /// persists a master password or session token across runs (a Decision below), so this asks
    /// again every time the token is cleared.
    fn ensure_authenticated(&mut self) -> Result<(), AdapterError> {
        self.ensure_handshake()?;
        if self.token.is_some() {
            return Ok(());
        }
        let handshake = self.handshake.as_ref().ok_or(AdapterError::Handshake)?;
        let master_password = if handshake.requires_master_password {
            let label = handshake.master_password_label.as_deref().unwrap_or("master password");
            let prompt_text = format!("Enter {label} for {}:", handshake.name);
            let Some(password) = (self.prompt)(&prompt_text) else {
                return Err(AdapterError::Other("Canceled.".to_string()));
            };
            Some(password)
        } else {
            None
        };
        let request = LoginRequest { header: self.header(), user_account_id: self.user_account_id.clone(), master_password: master_password.as_ref().map(|p| p.expose().to_string()) };
        let response: LoginResponse = match self.run("login", &request) {
            Ok(r) => r,
            Err(AdapterError::Other(message)) => return Err(AdapterError::Login(message)),
            Err(e) => return Err(e),
        };
        self.token = response.token.map(Secret::new);
        Ok(())
    }

    /// Ensures authentication, sends one request built by `build`, and decodes `Resp`. A reply
    /// flagged `needsAuthentication` clears the token and retries the whole thing once, as every
    /// recipe's `recovery` closure does in `AP` (e.g. `AP:656-664`).
    fn authed<Req: Serialize, Resp: for<'de> Deserialize<'de>>(&mut self, subcommand: &str, mut build: impl FnMut(&mut Self) -> Req) -> Result<Resp, String> {
        self.ensure_authenticated().map_err(|e| e.text())?;
        let request = build(self);
        match self.run(subcommand, &request) {
            Ok(resp) => Ok(resp),
            Err(AdapterError::NotAuthenticated) => {
                self.token = None;
                self.ensure_authenticated().map_err(|e| e.text())?;
                let request = build(self);
                self.run(subcommand, &request).map_err(|e| e.text())
            }
            Err(e) => Err(e.text()),
        }
    }
}

impl<R: Runner> Source for Adapter<R> {
    fn kind(&self) -> Kind {
        self.kind
    }

    fn name(&self) -> String {
        short_name(self.kind).to_string()
    }

    fn caps(&self) -> Caps {
        match &self.handshake {
            Some(h) => caps_from_handshake(h),
            None => Caps::default(),
        }
    }

    /// `checkAvailability`: whether the adapter executable exists, nothing more (`AP:1028-1030`).
    /// This never runs the adapter, so it never touches `runner`.
    fn available(&mut self) -> Result<(), String> {
        if crate::localfs::is_local_file(&self.path) {
            Ok(())
        } else {
            Err(AdapterError::NotFound.text())
        }
    }

    fn accounts(&mut self) -> Result<Vec<PwmAccount>, String> {
        let response: ListAccountsResponse = self.authed("list-accounts", |a| ListAccountsRequest { header: a.header(), user_account_id: a.user_account_id.clone(), token: a.token_value() })?;
        self.warning = response.warning.as_deref().map(str::trim).filter(|w| !w.is_empty()).map(str::to_string);
        Ok(map_accounts(response))
    }

    fn password(&mut self, acc: &PwmAccount) -> Result<(Secret, Option<Secret>), String> {
        let id = acc.id.clone();
        let response: Password =
            self.authed("get-password", |a| GetPasswordRequest { header: a.header(), user_account_id: a.user_account_id.clone(), token: a.token_value(), account_identifier: AccountIdentifier { account_id: id.clone() } })?;
        Ok((Secret::new(response.password), response.otp.map(Secret::new)))
    }

    fn set_password(&mut self, acc: &PwmAccount, p: Secret) -> Result<(), String> {
        self.edit_in_place(acc, None, None, Some(p))
    }

    fn delete(&mut self, acc: &PwmAccount) -> Result<(), String> {
        let id = acc.id.clone();
        let label = acc.source_label.clone();
        let _resp: DeleteAccountResponse = self.authed("delete-account", |a| DeleteAccountRequest {
            header: a.header(),
            user_account_id: a.user_account_id.clone(),
            token: a.token_value(),
            account_identifier: AccountIdentifier { account_id: id.clone() },
            source_label: label.clone(),
        })?;
        Ok(())
    }

    fn add(&mut self, name: &str, user: &str, p: Secret, flags: &[(String, bool)]) -> Result<PwmAccount, String> {
        let flags_map: Option<HashMap<String, bool>> = if flags.is_empty() { None } else { Some(flags.iter().cloned().collect()) };
        let response: AddAccountResponse = self.authed("add-account", |a| AddAccountRequest {
            header: a.header(),
            user_account_id: a.user_account_id.clone(),
            token: a.token_value(),
            user_name: user.to_string(),
            account_name: name.to_string(),
            password: Some(p.expose().to_string()),
            flags: flags_map.clone(),
        })?;
        Ok(PwmAccount { id: response.account_identifier.account_id, display: format!("{name}{SEP}{user}"), account_name: name.to_string(), user_name: user.to_string(), has_otp: false, send_otp: false, source_label: None })
    }

    fn edit_in_place(&mut self, acc: &PwmAccount, name: Option<&str>, user: Option<&str>, p: Option<Secret>) -> Result<(), String> {
        let id = acc.id.clone();
        let label = acc.source_label.clone();
        let _resp: SetPasswordResponse = self.authed("set-password", |a| SetPasswordRequest {
            header: a.header(),
            user_account_id: a.user_account_id.clone(),
            token: a.token_value(),
            account_identifier: AccountIdentifier { account_id: id.clone() },
            new_password: p.as_ref().map(|s| s.expose().to_string()),
            source_label: label.clone(),
            new_account_name: name.map(str::to_string),
            new_user_name: user.map(str::to_string),
        })?;
        Ok(())
    }

    fn toggle_otp(&mut self, _acc: &PwmAccount) -> Result<PwmAccount, String> {
        // Never called: the adapter protocol has no per-account OTP opt-out, so nothing in
        // `caps()` ever enables the button that would call this (`AP:1103-1107`,
        // `toggleShouldSendOTP` is `it_fatalError()` there; `LastPass::toggle_otp` uses the same
        // text for the same reason).
        Err("One-time codes are not supported.".to_string())
    }

    fn reload(&mut self) {
        // No account list is cached here (unlike iTerm2's 30-minute cache, which the app layer
        // would own): every accounts() call already re-fetches.
    }

    fn reset_configuration(&mut self) {
        // Tandem keeps no adapter credentials or settings storage in this module to clear (a
        // Decision below); resetting drops the in-memory handshake and session only, so the next
        // call re-handshakes and re-logs in.
        self.handshake = None;
        self.token = None;
    }

    fn take_warning(&mut self) -> Option<String> {
        self.warning.take()
    }

    /// `runCustomCommand:` (`AP:1158-1183`): the command's name is the subcommand; a built-in
    /// one is refused.
    fn custom_command(&mut self, name: &str) -> Result<Option<String>, String> {
        if BUILT_IN_SUBCOMMANDS.contains(&name) {
            return Err(format!("Cannot run built-in subcommand \u{201c}{name}\u{201d} as a custom command"));
        }
        let response: CustomCommandResponse = self.authed(name, |a| CustomCommandRequest { header: a.header(), user_account_id: a.user_account_id.clone(), token: a.token_value(), command_name: name.to_string() })?;
        Ok(response.message)
    }

    fn switch_account(&mut self) -> Result<(), String> {
        // Picking among several accounts (`AP:1113-1121`, `AccountPicker`) is UI, wired by the
        // app layer; `caps().multiple_accounts` only enables the menu item, as `OnePassword`'s
        // `switch_account` leaves picking to a later task.
        Ok(())
    }
}

// --- The adapter side: the one framework the three executables share ---

/// The most request JSON an adapter reads from stdin (a request is a few hundred bytes).
pub const MAX_REQUEST: usize = 1 << 20;

/// The most JSON an adapter parses from a password manager CLI (untrusted output is capped).
pub const MAX_CLI_JSON: usize = 16 << 20;

/// What an adapter's `handle` is: the subcommand, its stdin, and the `Runner` it runs its
/// password manager CLI through; it returns the JSON to print.
pub type Handle = fn(&str, &[u8], &mut dyn Runner) -> Vec<u8>;

/// `ErrorResponse` as the JSON an adapter prints.
pub fn error_json(message: &str) -> Vec<u8> {
    let mut v = serde_json::to_vec(&ErrorResponse { error: message.to_string(), needs_authentication: None }).unwrap_or_default();
    v.push(b'\n');
    v
}

/// Like `error_json`, flagged so the app clears its token and logs in again (`AP:53-62`).
pub fn error_json_reauth(message: &str) -> Vec<u8> {
    let mut v = serde_json::to_vec(&ErrorResponse { error: message.to_string(), needs_authentication: Some(true) }).unwrap_or_default();
    v.push(b'\n');
    v
}

/// A response as the JSON an adapter prints.
pub fn ok_json<T: Serialize>(response: &T) -> Vec<u8> {
    match serde_json::to_vec(response) {
        Ok(mut v) => {
            v.push(b'\n');
            v
        }
        Err(_) => error_json("Failed to encode output."),
    }
}

/// Decodes a request, refusing one over `MAX_REQUEST`. The error text names no content.
pub fn decode_request<T: serde::de::DeserializeOwned>(stdin: &[u8], what: &str) -> Result<T, String> {
    if stdin.len() > MAX_REQUEST {
        return Err(format!("Failed to decode {what} request."));
    }
    serde_json::from_slice(stdin).map_err(|_| format!("Failed to decode {what} request."))
}

/// A session token travels as base64 of the session key (`encodeToken` in iTerm2's adapters).
pub fn encode_token(session: &Secret) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(session.expose().as_bytes())
}

/// The session key a token carries, held in a `Secret`; `None` for a missing or malformed token.
pub fn decode_token(token: Option<&str>) -> Option<Secret> {
    use base64::Engine;
    let token = token.filter(|t| !t.is_empty())?;
    let bytes = base64::engine::general_purpose::STANDARD.decode(token).ok()?;
    String::from_utf8(bytes).ok().map(Secret::new)
}

/// Overwrites a buffer with zeros so a request's plaintext does not linger.
pub fn zero_bytes(bytes: &mut [u8]) {
    for b in bytes.iter_mut() {
        // SAFETY: a plain byte write into our own buffer.
        unsafe { std::ptr::write_volatile(b, 0) };
    }
    std::sync::atomic::compiler_fence(std::sync::atomic::Ordering::SeqCst);
}

/// The whole `main` of an adapter executable: the subcommand is argument 1, stdin is read to the
/// end (capped), `call`'s JSON is printed, and the process exits 0 after an error response
/// except for "No input provided", which exits 1 (iTerm2's adapters). `call` gets the subcommand
/// and the request bytes. Returns the exit code.
pub fn run_main_with(args: &[String], stdin: &mut dyn std::io::Read, stdout: &mut dyn std::io::Write, call: impl FnOnce(&str, &[u8]) -> Vec<u8>) -> i32 {
    use std::io::Read;
    let Some(command) = args.get(1) else {
        let _ = stdout.write_all(&error_json("Usage: <adapter> <command>"));
        return 1;
    };
    let mut input = Vec::new();
    let _ = stdin.take(MAX_REQUEST as u64 + 1).read_to_end(&mut input);
    if input.is_empty() {
        let _ = stdout.write_all(&error_json("No input provided"));
        return 1;
    }
    let out = call(command, &input);
    zero_bytes(&mut input);
    let _ = stdout.write_all(&out);
    let _ = stdout.flush();
    0
}

/// `run_main_with` for the adapters that drive a CLI through a `Runner`.
pub fn run_main(args: &[String], stdin: &mut dyn std::io::Read, stdout: &mut dyn std::io::Write, handle: Handle, runner: &mut dyn Runner) -> i32 {
    run_main_with(args, stdin, stdout, |c, i| handle(c, i, runner))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pwm_run::Output;
    use std::sync::{Arc, Mutex};

    /// Records every `Request` (program, args, stdin) and returns the next canned stdout from
    /// its queue, at exit code 0.
    #[derive(Default)]
    struct FakeRunner {
        requests: Arc<Mutex<Vec<(String, Vec<String>, Option<String>)>>>,
        outputs: std::collections::VecDeque<String>,
    }

    impl FakeRunner {
        fn push(&mut self, stdout: &str) {
            self.outputs.push_back(stdout.to_string());
        }
    }

    impl Runner for FakeRunner {
        fn run(&mut self, r: Request) -> Result<Output, String> {
            let stdin = r.stdin.as_ref().map(|s| s.expose().to_string());
            self.requests.lock().unwrap().push((r.program.clone(), r.args.clone(), stdin));
            let stdout = self.outputs.pop_front().ok_or_else(|| "FakeRunner: no more outputs queued".to_string())?;
            Ok(Output { code: 0, stdout: Secret::new(stdout), stderr: String::new() })
        }
    }

    fn adapter(runner: FakeRunner, prompt: MasterPasswordPrompt) -> Adapter<FakeRunner> {
        Adapter::new(runner, Kind::KeePassXC, PathBuf::from("adapter.exe"), None, None, HashMap::new(), None, prompt)
    }

    fn handshake_json(extra: serde_json::Value) -> String {
        let mut base = serde_json::json!({
            "protocolVersion": 0,
            "name": "Test Adapter",
            "requiresMasterPassword": true,
            "canSetPasswords": true,
            "needsPathToDatabase": false,
        });
        base.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        base.to_string()
    }

    #[test]
    fn adapter_follows_iterm2() {
        // A missing adapter file is NotFound, and available() never spawns anything.
        let mut a = adapter(FakeRunner::default(), Box::new(|_| None));
        assert_eq!(a.available().unwrap_err(), "Adapter not found.");
        assert!(a.runner.requests.lock().unwrap().is_empty());

        // The handshake request, argument "handshake", stdin
        // {"iTermVersion": <Tandem version>, "minProtocolVersion": 0, "maxProtocolVersion": 0};
        // then login with the master password only on the login request's stdin, never on the
        // handshake request, and never as an argument.
        let mut runner = FakeRunner::default();
        let requests = runner.requests.clone();
        runner.push(&handshake_json(serde_json::json!({})));
        runner.push(r#"{"token":"tok"}"#);
        let mut a = adapter(runner, Box::new(|_| Some(Secret::new("hunter2".to_string()))));
        a.ensure_authenticated().expect("handshake and login");
        let calls = requests.lock().unwrap().clone();
        assert_eq!(calls.len(), 2);
        let (program, args, handshake_stdin) = &calls[0];
        assert_eq!(program, "adapter.exe");
        assert_eq!(args, &vec!["handshake".to_string()]);
        let body: serde_json::Value = serde_json::from_str(handshake_stdin.as_deref().unwrap()).unwrap();
        assert_eq!(body["iTermVersion"], crate::reports::VERSION);
        assert_eq!(body["minProtocolVersion"].as_i64(), Some(0));
        assert_eq!(body["maxProtocolVersion"].as_i64(), Some(0));
        assert!(!handshake_stdin.as_deref().unwrap().contains("hunter2"), "the master password never reaches the handshake request");
        let (_, login_args, login_stdin) = &calls[1];
        assert_eq!(login_args, &vec!["login".to_string()]);
        assert!(login_args.iter().all(|s| !s.contains("hunter2")), "never an argument");
        assert!(login_stdin.as_deref().unwrap().contains("hunter2"), "only the login request's stdin");

        // An incompatible protocol version.
        let mut runner = FakeRunner::default();
        runner.push(&handshake_json(serde_json::json!({"protocolVersion": 1, "requiresMasterPassword": false})));
        let mut a = adapter(runner, Box::new(|_| None));
        assert_eq!(a.accounts().unwrap_err(), "Incompatible protocol. Please update Tandem.");

        // Invalid JSON from the adapter.
        let mut runner = FakeRunner::default();
        runner.push("not json");
        let mut a = adapter(runner, Box::new(|_| None));
        assert_eq!(a.accounts().unwrap_err(), "Invalid output.");

        assert_eq!(AdapterError::Login("x".to_string()).text(), "Could not log in: x");

        // A login failure gets iTerm2's "Could not log in: <message>" wrapping end to end.
        let mut runner = FakeRunner::default();
        runner.push(&handshake_json(serde_json::json!({})));
        runner.push(r#"{"error":"bad password"}"#);
        let mut a = adapter(runner, Box::new(|_| Some(Secret::new("wrong".to_string()))));
        assert_eq!(a.accounts().unwrap_err(), "Could not log in: bad password");

        // list-accounts maps accounts[].identifier.accountID, userName, accountName, hasOTP and
        // sourceLabel.
        let mut runner = FakeRunner::default();
        runner.push(&handshake_json(serde_json::json!({"requiresMasterPassword": false})));
        runner.push(r#"{"token":"tok"}"#);
        runner.push(r#"{"accounts":[{"identifier":{"accountID":"id-1"},"userName":"bob","accountName":"github","hasOTP":true,"sourceLabel":"Nested"}],"warning":"  Vault is read-only. \n"}"#);
        let mut a = adapter(runner, Box::new(|_| None));
        let accounts = a.accounts().expect("accounts");
        assert_eq!(a.take_warning().as_deref(), Some("Vault is read-only."), "the list's warning, trimmed, once");
        assert_eq!(a.take_warning(), None);
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0].id, "id-1");
        assert_eq!(accounts[0].user_name, "bob");
        assert_eq!(accounts[0].account_name, "github");
        assert!(accounts[0].has_otp && accounts[0].send_otp);
        assert_eq!(accounts[0].source_label.as_deref(), Some("Nested"));
        assert_eq!(accounts[0].display, format!("github{SEP}bob"));

        // Caps from a handshake with canEditInPlace, requiresPasswordForAdd, settingsFields,
        // customCommands and addAccountToggles (only the first toggle is kept).
        let handshake: HandshakeResponse = serde_json::from_str(&handshake_json(serde_json::json!({
            "requiresMasterPassword": false,
            "canEditInPlace": true,
            "requiresPasswordForAdd": true,
            "settingsFields": [{"key": "url", "label": "Server URL", "isSecret": false, "persistInKeychain": false}],
            "customCommands": [{"name": "sync", "label": "Sync Now"}],
            "addAccountToggles": [
                {"key": "a", "label": "A", "defaultValue": true},
                {"key": "b", "label": "B", "defaultValue": false},
            ],
        })))
        .expect("handshake parses");
        let caps = caps_from_handshake(&handshake);
        assert!(caps.in_place);
        assert!(caps.requires_password);
        assert!(caps.can_reset);
        assert_eq!(caps.settings.len(), 1);
        assert_eq!(caps.settings[0].key, "url");
        assert_eq!(caps.settings[0].label, "Server URL");
        assert_eq!(caps.commands, vec![("sync".to_string(), "Sync Now".to_string())]);
        assert_eq!(caps.add_toggle.as_ref().map(|t| t.key.as_str()), Some("a"));
    }

    #[test]
    fn adapter_file_and_path_only_name_the_three_adapters() {
        assert_eq!(adapter_file(Kind::KeePassXC), Some("iterm2-keepassxc-adapter.exe"));
        assert_eq!(adapter_file(Kind::Bitwarden), Some("iterm2-bitwarden-adapter.exe"));
        assert_eq!(adapter_file(Kind::Keeper), Some("iterm2-keeper-adapter.exe"));
        assert_eq!(adapter_file(Kind::CredentialManager), None);
        assert_eq!(adapter_file(Kind::OnePassword), None);
        assert_eq!(adapter_file(Kind::LastPass), None);

        let dir = Path::new(r"C:\Program Files\Tandem");
        assert_eq!(adapter_path(Kind::Keeper, dir), Some(dir.join("iterm2-keeper-adapter.exe")));
        assert_eq!(adapter_path(Kind::OnePassword, dir), None);
    }

    #[test]
    fn set_password_and_delete_send_the_account_id_and_a_secret_only_on_stdin() {
        let mut runner = FakeRunner::default();
        let requests = runner.requests.clone();
        runner.push(&handshake_json(serde_json::json!({"requiresMasterPassword": false})));
        runner.push(r#"{"token":"tok"}"#);
        runner.push(r#"{}"#); // set-password
        runner.push(r#"{}"#); // delete-account
        runner.push(r#"{"accountIdentifier":{"accountID":"id-2"}}"#); // add-account
        let mut a = adapter(runner, Box::new(|_| None));
        let acc = PwmAccount { id: "id-1".to_string(), account_name: "github".to_string(), user_name: "bob".to_string(), display: "github".to_string(), has_otp: false, send_otp: false, source_label: None };
        a.set_password(&acc, Secret::new("s3cr3t".to_string())).expect("set_password");
        a.delete(&acc).expect("delete");
        let added = a.add("gitlab", "amy", Secret::new("n3wpass".to_string()), &[]).expect("add");
        assert_eq!(added.id, "id-2");
        let calls = requests.lock().unwrap();
        let call = |name: &str| calls.iter().find(|(_, args, _)| args == &vec![name.to_string()]).unwrap_or_else(|| panic!("a {name} call")).clone();
        let (_, set_args, set_stdin) = call("set-password");
        assert!(!set_args.iter().any(|a| a.contains("s3cr3t")));
        let stdin = set_stdin.as_deref().unwrap();
        assert!(stdin.contains("s3cr3t"));
        assert!(stdin.contains("\"accountID\":\"id-1\""));
        let (_, delete_args, delete_stdin) = call("delete-account");
        assert_eq!(delete_args, vec!["delete-account".to_string()], "the account id rides on stdin only");
        assert!(delete_stdin.as_deref().is_some_and(|s| s.contains("\"accountID\":\"id-1\"") && !s.contains("s3cr3t")));
        let (_, add_args, add_stdin) = call("add-account");
        assert!(!add_args.iter().any(|a| a.contains("n3wpass")), "the new password is never an argument");
        assert!(add_stdin.as_deref().is_some_and(|s| s.contains("n3wpass") && s.contains("gitlab") && s.contains("amy")));
    }

    #[test]
    fn the_header_sends_only_declared_settings() {
        let mut runner = FakeRunner::default();
        let requests = runner.requests.clone();
        runner.push(&handshake_json(serde_json::json!({"requiresMasterPassword": false, "settingsFields": [{"key": "url", "label": "Server URL", "isSecret": false, "persistInKeychain": false}]})));
        runner.push(r#"{"token":"tok"}"#);
        runner.push(r#"{"accounts":[]}"#);
        let settings = HashMap::from([("url".to_string(), "https://vault".to_string()), ("stray".to_string(), "left-over".to_string())]);
        let mut a = Adapter::new(runner, Kind::KeePassXC, PathBuf::from("adapter.exe"), None, None, settings, None, Box::new(|_| None));
        // The adapter file is not there; the handshake still runs through the fake.
        a.accounts().expect("accounts");
        let calls = requests.lock().unwrap();
        let (_, _, stdin) = calls.iter().find(|(_, args, _)| args == &vec!["list-accounts".to_string()]).expect("a list-accounts call");
        let stdin = stdin.as_deref().unwrap();
        assert!(stdin.contains("\"url\":\"https://vault\""));
        assert!(!stdin.contains("stray") && !stdin.contains("left-over"), "a setting the handshake did not declare is not sent");
    }

    #[test]
    fn custom_commands_follow_iterm2() {
        let mut a = adapter(FakeRunner::default(), Box::new(|_| None));
        assert_eq!(a.custom_command("login").unwrap_err(), "Cannot run built-in subcommand \u{201c}login\u{201d} as a custom command");

        let mut runner = FakeRunner::default();
        let requests = runner.requests.clone();
        runner.push(&handshake_json(serde_json::json!({"requiresMasterPassword": false})));
        runner.push(r#"{"token":"tok"}"#);
        runner.push(r#"{"message":"Synced 3 items."}"#);
        let mut a = adapter(runner, Box::new(|_| None));
        assert_eq!(a.custom_command("sync").unwrap().as_deref(), Some("Synced 3 items."));
        let calls = requests.lock().unwrap().clone();
        let last = calls.last().unwrap();
        assert_eq!(last.1, vec!["sync".to_string()], "the command's name is the subcommand");
        assert!(last.2.as_deref().is_some_and(|s| s.contains("\"commandName\":\"sync\"")));
    }
    #[test]
    fn adapter_mode_follows_iterm2() {
        assert_eq!(mode_for(Scope::Browser), "browser");
        assert_eq!(mode_for(Scope::Terminal), "terminal");
        let mut runner = FakeRunner::default();
        let requests = runner.requests.clone();
        runner.push(&handshake_json(serde_json::json!({"requiresMasterPassword": false})));
        runner.push(r#"{"token":"tok"}"#);
        runner.push(r#"{"accounts":[]}"#);
        let mut a = adapter(runner, Box::new(|_| None)).with_scope(Scope::Browser);
        a.accounts().expect("accounts");
        let calls = requests.lock().unwrap();
        let (_, _, stdin) = calls.iter().find(|(_, args, _)| args == &vec!["list-accounts".to_string()]).expect("a list-accounts call");
        assert!(stdin.as_deref().unwrap().contains("\"mode\":\"browser\""));
    }
}
