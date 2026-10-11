//! The Bitwarden adapter's logic (`iterm2-bitwarden-adapter`), a port of iTerm2's
//! `pwmplugin/Sources/iterm2-bitwarden-adapter/main.swift` (`BW`) over a `pwm_run::Runner`, so it
//! is testable with a fake. The executable in `src/bin` only wires `ProcessRunner` and stdio to
//! `pwm_adapter::run_main`.
//!
//! Secrets: the master password is read out of the request into a `Secret` at once and reaches
//! `bw` only through the environment of the one `unlock` invocation (`--passwordenv`); the session
//! key `bw unlock --raw` prints is a `Secret` too, and every later call passes it only in that
//! call's `BW_SESSION` environment variable (iTerm2 passes `--session <key>`, which shows in a
//! process listing; `bw` reads the variable identically). Neither is ever an argument, written to
//! disk or logged. `bw`'s own output is untrusted: JSON is parsed under `MAX_CLI_JSON`, item
//! names and user names pass `inert_files::display_text`, and `bw`'s error text is never shown
//! (it can quote a password back), only a fixed text. The Bitwarden folder keeps iTerm2's name,
//! "iTerm2", so a vault shared with a Mac stays in one folder (a Decision).

use std::time::Duration;

use serde::Deserialize;

use crate::inert_files::display_text;
use crate::pwm::Secret;
use crate::pwm_adapter::*;
use crate::pwm_run::{Request, Runner};

/// The CLI looked up on PATH when the handshake's `pathToExecutable` is empty.
pub const CLI: &str = "bw";

/// The folder terminal-mode items live in (`modeTag`).
const FOLDER: &str = "iTerm2";

/// The child-only variable `bw unlock --passwordenv` reads the master password from.
const PASSWORD_ENV: &str = "BW_MASTER_PASSWORD";

/// `bw` reads its session key from this variable.
const SESSION_ENV: &str = "BW_SESSION";

/// A `sync` can take a while on a big vault; everything else is quick.
const TIMEOUT: Duration = Duration::from_secs(60);

/// Names and user names are shown in the window, so they are capped as well as made inert.
const MAX_NAME: usize = 200;

/// The most items one listing returns.
const MAX_ITEMS: usize = 50_000;

#[derive(Deserialize)]
struct Status {
    status: String,
}

#[derive(Deserialize)]
struct Folder {
    id: Option<String>,
    name: String,
}

#[derive(Deserialize)]
struct Item {
    id: String,
    #[serde(default)]
    r#type: i64,
    #[serde(default)]
    name: String,
    #[serde(default, rename = "deletedDate")]
    deleted_date: Option<String>,
    #[serde(default)]
    login: Option<Login>,
}

#[derive(Deserialize)]
struct Login {
    #[serde(default)]
    username: Option<String>,
    #[serde(default)]
    totp: Option<String>,
}

/// Why a step failed: a fixed text, flagged when the app should log in again.
struct Failure {
    text: String,
    reauth: bool,
}

fn fail(text: &str) -> Failure {
    Failure { text: text.to_string(), reauth: false }
}

impl Failure {
    fn json(&self) -> Vec<u8> {
        if self.reauth { error_json_reauth(&self.text) } else { error_json(&self.text) }
    }
}

/// `bw` says one of these when the session key is missing, wrong or expired.
fn session_gone(stderr: &str) -> bool {
    let lower = stderr.to_lowercase();
    lower.contains("vault is locked") || lower.contains("you are not logged in") || lower.contains("invalid session")
}

struct Bw<'a> {
    program: String,
    runner: &'a mut dyn Runner,
}

impl<'a> Bw<'a> {
    fn new(path: Option<&str>, runner: &'a mut dyn Runner) -> Bw<'a> {
        let program = path.filter(|p| !p.is_empty()).unwrap_or(CLI).to_string();
        Bw { program, runner }
    }

    /// Runs `bw <args>`; the session key and the stdin payload travel only as the request's
    /// environment and stdin. A non-zero exit is a `Failure` with `what` as its fixed text.
    fn run(&mut self, args: &[&str], session: Option<&Secret>, stdin: Option<Secret>, extra_env: Vec<(String, Secret)>, what: &str) -> Result<Secret, Failure> {
        let mut r = Request::new(self.program.clone(), args.iter().map(|a| a.to_string()).collect());
        // `bw` must never stop to ask a question on a pipe nobody reads.
        r.env.push(("BW_NOINTERACTION".to_string(), Secret::new("true".to_string())));
        if let Some(s) = session {
            r.env.push((SESSION_ENV.to_string(), s.duplicate()));
        }
        r.env.extend(extra_env);
        r.stdin = stdin;
        r.timeout = Some(TIMEOUT);
        let out = self.runner.run(r).map_err(|_| fail(&format!("{what} Is the bw CLI installed?")))?;
        if out.code != 0 {
            return Err(Failure { text: what.to_string(), reauth: session.is_some() && session_gone(&out.stderr) });
        }
        Ok(out.stdout)
    }

    /// Runs `bw` and parses its stdout as JSON of type `T`, refusing output over `MAX_CLI_JSON`.
    fn json<T: for<'d> Deserialize<'d>>(&mut self, args: &[&str], session: Option<&Secret>, what: &str) -> Result<T, Failure> {
        let out = self.run(args, session, None, Vec::new(), what)?;
        parse(&out, what)
    }
}

fn parse<T: for<'d> Deserialize<'d>>(out: &Secret, what: &str) -> Result<T, Failure> {
    if out.expose().len() > MAX_CLI_JSON {
        return Err(fail(&format!("{what} The output was too large.")));
    }
    serde_json::from_str(out.expose()).map_err(|_| fail(&format!("{what} The output was not valid.")))
}

/// An id goes on a command line, so only what Bitwarden ids are made of (UUIDs) is accepted: it
/// can never pass for an option.
fn valid_id(id: &str) -> bool {
    id.chars().next().is_some_and(|c| c.is_ascii_alphanumeric()) && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn session_of(token: Option<&str>) -> Result<Secret, Failure> {
    decode_token(token).ok_or_else(|| fail("Invalid or missing token. Please login first."))
}

fn account_id(id: &AccountIdentifier) -> Result<&str, Failure> {
    if valid_id(&id.account_id) { Ok(&id.account_id) } else { Err(fail("Invalid account identifier.")) }
}

/// `base64(JSON)` as the stdin `bw create`/`bw edit` take, so an item's contents (a password)
/// never appear on a command line.
fn encoded(value: &serde_json::Value) -> Secret {
    use base64::Engine;
    let mut bytes = serde_json::to_vec(value).unwrap_or_default();
    let text = base64::engine::general_purpose::STANDARD.encode(&bytes);
    zero_bytes(&mut bytes);
    Secret::new(text)
}

fn folder_id(bw: &mut Bw, session: &Secret) -> Result<Option<String>, Failure> {
    let folders: Vec<Folder> = bw.json(&["list", "folders"], Some(session), "Failed to list folders.")?;
    Ok(folders.into_iter().find(|f| f.name == FOLDER).and_then(|f| f.id))
}

fn folder_id_creating(bw: &mut Bw, session: &Secret) -> Result<String, Failure> {
    if let Some(id) = folder_id(bw, session)? {
        return Ok(id);
    }
    let out = bw.run(&["create", "folder"], Some(session), Some(encoded(&serde_json::json!({ "name": FOLDER }))), Vec::new(), "Failed to create folder.")?;
    let folder: Folder = parse(&out, "Failed to create folder.")?;
    folder.id.filter(|id| valid_id(id)).ok_or_else(|| fail("Failed to create folder."))
}

fn is_terminal(header: &RequestHeader) -> bool {
    header.mode != "browser"
}

fn handshake(stdin: &[u8]) -> Result<Vec<u8>, Failure> {
    let request: HandshakeRequest = decode_request(stdin, "handshake").map_err(|e| Failure { text: e, reauth: false })?;
    if request.max_protocol_version < PROTOCOL {
        return Err(fail("Protocol version 0 is required but not supported by client"));
    }
    Ok(ok_json(&HandshakeResponse {
        protocol_version: PROTOCOL,
        name: "Bitwarden".to_string(),
        requires_master_password: true,
        can_set_passwords: true,
        user_accounts: None,
        needs_path_to_database: false,
        database_extension: None,
        needs_path_to_executable: Some(CLI.to_string()),
        path_to_database_kind: None,
        path_to_database_prompt: None,
        path_to_database_placeholder: None,
        master_password_label: None,
        persists_credentials: None,
        custom_commands: None,
        settings_fields: None,
        add_account_toggles: None,
        legacy_credential_migrations: None,
        can_edit_in_place: None,
        requires_password_for_add: None,
    }))
}

fn login(stdin: &[u8], runner: &mut dyn Runner) -> Result<Vec<u8>, Failure> {
    let mut request: LoginRequest = decode_request(stdin, "login").map_err(|e| Failure { text: e, reauth: false })?;
    let password = Secret::new(std::mem::take(request.master_password.get_or_insert_with(String::new)));
    if password.is_empty() {
        return Err(fail("Master password is required"));
    }
    let mut bw = Bw::new(request.header.path_to_executable.as_deref(), runner);
    let status: Status = bw.json(&["status"], None, "Failed to get Bitwarden status.")?;
    match status.status.as_str() {
        "unauthenticated" => return Err(fail("Not logged in to Bitwarden. Please run 'bw login' first to authenticate, then try again.")),
        // An unlocked vault gives no way to read its session key, so it is unlocked again.
        "locked" | "unlocked" => {}
        _ => return Err(fail("Unknown Bitwarden status.")),
    }
    let out = bw
        .run(&["unlock", "--passwordenv", PASSWORD_ENV, "--raw"], None, None, vec![(PASSWORD_ENV.to_string(), password)], "Invalid master password")
        .map_err(|f| if f.text == "Invalid master password" { f } else { fail("Failed to unlock vault.") })?;
    let session = Secret::new(out.expose().trim().to_string());
    if session.is_empty() {
        return Err(fail("Failed to unlock vault."));
    }
    // A failed sync is not fatal: the vault is still usable from its cache.
    let _ = bw.run(&["sync"], Some(&session), None, Vec::new(), "Failed to sync.");
    Ok(ok_json(&LoginResponse { token: Some(encode_token(&session)) }))
}

fn list_accounts(stdin: &[u8], runner: &mut dyn Runner) -> Result<Vec<u8>, Failure> {
    let request: ListAccountsRequest = decode_request(stdin, "list-accounts").map_err(|e| Failure { text: e, reauth: false })?;
    let session = session_of(request.token.as_deref())?;
    let mut bw = Bw::new(request.header.path_to_executable.as_deref(), runner);
    // Terminal mode lists the "iTerm2" folder; browser mode lists the items with no folder.
    let folder = if is_terminal(&request.header) {
        match folder_id(&mut bw, &session)? {
            Some(id) if valid_id(&id) => id,
            _ => return Ok(ok_json(&ListAccountsResponse { accounts: Vec::new(), warning: None })),
        }
    } else {
        "null".to_string()
    };
    let items: Vec<Item> = bw.json(&["list", "items", "--folderid", &folder], Some(&session), "Failed to list accounts.")?;
    let accounts = items
        .into_iter()
        .take(MAX_ITEMS)
        .filter(|i| i.r#type == 1 && i.deleted_date.is_none() && valid_id(&i.id))
        .map(|i| Account {
            identifier: AccountIdentifier { account_id: i.id },
            user_name: display_text(i.login.as_ref().and_then(|l| l.username.as_deref()).unwrap_or(""), MAX_NAME, false),
            account_name: display_text(&i.name, MAX_NAME, false),
            has_otp: i.login.as_ref().and_then(|l| l.totp.as_deref()).is_some_and(|t| !t.is_empty()),
            source_label: None,
        })
        .collect();
    Ok(ok_json(&ListAccountsResponse { accounts, warning: None }))
}

fn get_password(stdin: &[u8], runner: &mut dyn Runner) -> Result<Vec<u8>, Failure> {
    let request: GetPasswordRequest = decode_request(stdin, "get-password").map_err(|e| Failure { text: e, reauth: false })?;
    let session = session_of(request.token.as_deref())?;
    let id = account_id(&request.account_identifier)?;
    let mut bw = Bw::new(request.header.path_to_executable.as_deref(), runner);
    let password = bw.run(&["get", "password", id], Some(&session), None, Vec::new(), "Failed to get password.")?;
    // The TOTP is optional: no TOTP, or a failure, just leaves it out.
    let otp = bw.run(&["get", "totp", id], Some(&session), None, Vec::new(), "").ok().map(|o| o.expose().trim().to_string()).filter(|t| !t.is_empty());
    Ok(ok_json(&Password { password: password.expose().trim().to_string(), otp }))
}

fn set_password(stdin: &[u8], runner: &mut dyn Runner) -> Result<Vec<u8>, Failure> {
    let mut request: SetPasswordRequest = decode_request(stdin, "set-password").map_err(|e| Failure { text: e, reauth: false })?;
    let session = session_of(request.token.as_deref())?;
    let Some(new_password) = request.new_password.take().map(Secret::new) else {
        return Err(fail("New password is required"));
    };
    let id = account_id(&request.account_identifier)?;
    let mut bw = Bw::new(request.header.path_to_executable.as_deref(), runner);
    let mut item: serde_json::Value = bw.json(&["get", "item", id], Some(&session), "Failed to get item.")?;
    let Some(login) = item.get_mut("login").and_then(|l| l.as_object_mut()) else {
        return Err(fail("Failed to parse item data"));
    };
    login.insert("password".to_string(), serde_json::Value::String(new_password.expose().to_string()));
    bw.run(&["edit", "item", id], Some(&session), Some(encoded(&item)), Vec::new(), "Failed to set password.")?;
    Ok(ok_json(&SetPasswordResponse {}))
}

fn add_account(stdin: &[u8], runner: &mut dyn Runner) -> Result<Vec<u8>, Failure> {
    let mut request: AddAccountRequest = decode_request(stdin, "add-account").map_err(|e| Failure { text: e, reauth: false })?;
    let session = session_of(request.token.as_deref())?;
    let password = Secret::new(request.password.take().unwrap_or_default());
    let mut bw = Bw::new(request.header.path_to_executable.as_deref(), runner);
    let mut item = serde_json::json!({
        "type": 1,
        "name": request.account_name,
        "login": { "username": request.user_name, "password": password.expose() },
    });
    if is_terminal(&request.header) {
        let folder = folder_id_creating(&mut bw, &session).map_err(|_| fail(&format!("Failed to create or find folder: {FOLDER}")))?;
        item["folderId"] = serde_json::Value::String(folder);
    }
    let out = bw.run(&["create", "item"], Some(&session), Some(encoded(&item)), Vec::new(), "Failed to add account.")?;
    let created: Item = parse(&out, "Failed to parse created item response")?;
    if !valid_id(&created.id) {
        return Err(fail("Failed to parse created item response"));
    }
    Ok(ok_json(&AddAccountResponse { account_identifier: AccountIdentifier { account_id: created.id } }))
}

fn delete_account(stdin: &[u8], runner: &mut dyn Runner) -> Result<Vec<u8>, Failure> {
    let request: DeleteAccountRequest = decode_request(stdin, "delete-account").map_err(|e| Failure { text: e, reauth: false })?;
    let session = session_of(request.token.as_deref())?;
    let id = account_id(&request.account_identifier)?;
    let mut bw = Bw::new(request.header.path_to_executable.as_deref(), runner);
    // Bitwarden's default delete moves the item to the trash.
    bw.run(&["delete", "item", id], Some(&session), None, Vec::new(), "Failed to delete account.")?;
    Ok(ok_json(&DeleteAccountResponse {}))
}

/// One adapter call: the JSON to print (an `ErrorResponse` for any failure or an unknown
/// subcommand, `BW:700-726`).
pub fn handle(command: &str, stdin: &[u8], runner: &mut dyn Runner) -> Vec<u8> {
    let result = match command {
        "handshake" => handshake(stdin),
        "login" => login(stdin, runner),
        "list-accounts" => list_accounts(stdin, runner),
        "get-password" => get_password(stdin, runner),
        "set-password" => set_password(stdin, runner),
        "add-account" => add_account(stdin, runner),
        "delete-account" => delete_account(stdin, runner),
        other => return error_json(&format!("Unknown command: {}", display_text(other, 64, false))),
    };
    result.unwrap_or_else(|f| f.json())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pwm_run::Output;
    use base64::Engine;
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Debug)]
    struct Recorded {
        program: String,
        args: Vec<String>,
        env: Vec<(String, String)>,
        stdin: Option<String>,
    }

    #[derive(Default)]
    struct Fake {
        log: Arc<Mutex<Vec<Recorded>>>,
        outputs: VecDeque<Result<(i32, String, String), String>>,
    }

    impl Fake {
        fn push(&mut self, code: i32, out: &str) {
            self.outputs.push_back(Ok((code, out.to_string(), String::new())));
        }
        fn calls(&self) -> Vec<Recorded> {
            self.log.lock().unwrap().clone()
        }
    }

    impl Runner for Fake {
        fn run(&mut self, r: Request) -> Result<Output, String> {
            self.log.lock().unwrap().push(Recorded {
                program: r.program.clone(),
                args: r.args.clone(),
                env: r.env.iter().map(|(k, v)| (k.clone(), v.expose().to_string())).collect(),
                stdin: r.stdin.as_ref().map(|s| s.expose().to_string()),
            });
            match self.outputs.pop_front() {
                Some(Ok((code, stdout, stderr))) => Ok(Output { code, stdout: Secret::new(stdout), stderr }),
                Some(Err(e)) => Err(e),
                None => Err("Fake: no more outputs".to_string()),
            }
        }
    }

    fn json(bytes: &[u8]) -> serde_json::Value {
        serde_json::from_slice(bytes).unwrap()
    }

    fn token(session: &str) -> String {
        encode_token(&Secret::new(session.to_string()))
    }

    fn env_of<'a>(r: &'a Recorded, key: &str) -> Option<&'a str> {
        r.env.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    #[test]
    fn bitwarden_adapter_follows_iterm2() {
        // Handshake: protocol 0, needs the bw executable, no master password stored.
        let mut f = Fake::default();
        let out = json(&handle("handshake", br#"{"iTermVersion":"3","minProtocolVersion":0,"maxProtocolVersion":0}"#, &mut f));
        assert_eq!(out["protocolVersion"], 0);
        assert_eq!(out["name"], "Bitwarden");
        assert_eq!(out["requiresMasterPassword"], true);
        assert_eq!(out["canSetPasswords"], true);
        assert_eq!(out["needsPathToDatabase"], false);
        assert_eq!(out["needsPathToExecutable"], "bw");
        assert!(f.calls().is_empty());

        // Login of a locked vault: status, unlock with the password only in the environment, sync.
        let mut f = Fake::default();
        f.push(0, r#"{"status":"locked"}"#);
        f.push(0, "SESSIONKEY\n");
        f.push(0, "Syncing complete.");
        let out = json(&handle("login", br#"{"header":{"pathToExecutable":"C:\\bw\\bw.exe","mode":"terminal"},"masterPassword":"hunter2-master"}"#, &mut f));
        let calls = f.calls();
        assert_eq!(calls.len(), 3);
        assert_eq!(calls[0].program, "C:\\bw\\bw.exe");
        assert_eq!(calls[0].args, ["status"]);
        assert_eq!(calls[1].args, ["unlock", "--passwordenv", "BW_MASTER_PASSWORD", "--raw"]);
        assert_eq!(env_of(&calls[1], "BW_MASTER_PASSWORD"), Some("hunter2-master"));
        assert_eq!(calls[2].args, ["sync"]);
        assert_eq!(env_of(&calls[2], "BW_SESSION"), Some("SESSIONKEY"));
        assert_eq!(decode_token(out["token"].as_str()).unwrap().expose(), "SESSIONKEY");
        for c in &calls {
            assert!(!c.args.iter().any(|a| a.contains("hunter2") || a.contains("SESSIONKEY")), "{:?}", c.args);
            assert!(c.stdin.is_none());
            if c.args[0] != "unlock" {
                assert_eq!(env_of(c, "BW_MASTER_PASSWORD"), None);
            }
        }
        assert!(!String::from_utf8_lossy(&handle("login", br#"{"header":{"mode":"terminal"},"masterPassword":"hunter2-master"}"#, &mut Fake::default())).contains("hunter2"));

        // Already unlocked still unlocks again for a fresh session key; unauthenticated stops.
        let mut f = Fake::default();
        f.push(0, r#"{"status":"unlocked"}"#);
        f.push(0, "S2");
        f.push(1, "");
        assert!(json(&handle("login", br#"{"header":{"mode":"terminal"},"masterPassword":"pw"}"#, &mut f)).get("token").is_some());
        let mut f = Fake::default();
        f.push(0, r#"{"status":"unauthenticated"}"#);
        let out = json(&handle("login", br#"{"header":{"mode":"terminal"},"masterPassword":"pw"}"#, &mut f));
        assert!(out["error"].as_str().unwrap().contains("bw login"));
        assert_eq!(f.calls().len(), 1);
        // A wrong password: the bw output is never echoed (it can quote the password back).
        let mut f = Fake::default();
        f.push(0, r#"{"status":"locked"}"#);
        f.outputs.push_back(Ok((1, String::new(), "Invalid master password. pw-quoted".to_string())));
        let out = json(&handle("login", br#"{"header":{"mode":"terminal"},"masterPassword":"pw-quoted"}"#, &mut f));
        assert!(!out["error"].as_str().unwrap().contains("pw-quoted"));
        // Missing password.
        let out = json(&handle("login", br#"{"header":{"mode":"terminal"}}"#, &mut Fake::default()));
        assert_eq!(out["error"], "Master password is required");

        // List accounts: the iTerm2 folder, the session in the environment, login items only,
        // names through display_text.
        let mut f = Fake::default();
        f.push(0, r#"[{"id":"f0","name":"Other"},{"id":"f1","name":"iTerm2"}]"#);
        f.push(0, "[{\"id\":\"a1\",\"type\":1,\"name\":\"Mail\\u202e\\u0007x\",\"login\":{\"username\":\"me\",\"totp\":\"otpauth://x\"}},{\"id\":\"a2\",\"type\":2,\"name\":\"Note\"},{\"id\":\"a3\",\"type\":1,\"name\":\"Gone\",\"deletedDate\":\"2026\"},{\"id\":\"a4\",\"type\":1,\"name\":\"Plain\",\"login\":{}}]");
        let body = format!(r#"{{"header":{{"mode":"terminal"}},"token":"{}"}}"#, token("SK"));
        let out = json(&handle("list-accounts", body.as_bytes(), &mut f));
        let calls = f.calls();
        assert_eq!(calls[0].args, ["list", "folders"]);
        assert_eq!(calls[1].args, ["list", "items", "--folderid", "f1"]);
        assert!(calls.iter().all(|c| env_of(c, "BW_SESSION") == Some("SK") && !c.args.iter().any(|a| a.contains("SK") && a != "--folderid")));
        let accounts = out["accounts"].as_array().unwrap();
        assert_eq!(accounts.len(), 2);
        assert_eq!(accounts[0]["identifier"]["accountID"], "a1");
        assert_eq!(accounts[0]["accountName"], "Mailx");
        assert_eq!(accounts[0]["userName"], "me");
        assert_eq!(accounts[0]["hasOTP"], true);
        assert_eq!(accounts[1]["userName"], "");
        assert_eq!(accounts[1]["hasOTP"], false);
        // No iTerm2 folder yet: an empty list without a second command.
        let mut f = Fake::default();
        f.push(0, "[]");
        let out = json(&handle("list-accounts", body.as_bytes(), &mut f));
        assert_eq!(out["accounts"].as_array().unwrap().len(), 0);
        assert_eq!(f.calls().len(), 1);
        // Browser mode lists the items with no folder.
        let mut f = Fake::default();
        f.push(0, "[]");
        let body = format!(r#"{{"header":{{"mode":"browser"}},"token":"{}"}}"#, token("SK"));
        handle("list-accounts", body.as_bytes(), &mut f);
        assert_eq!(f.calls()[0].args, ["list", "items", "--folderid", "null"]);
        // No token.
        let out = json(&handle("list-accounts", br#"{"header":{"mode":"terminal"}}"#, &mut Fake::default()));
        assert_eq!(out["error"], "Invalid or missing token. Please login first.");

        // Get password with the TOTP when there is one.
        let body = format!(r#"{{"header":{{"mode":"terminal"}},"token":"{}","accountIdentifier":{{"accountID":"a1"}}}}"#, token("SK"));
        let mut f = Fake::default();
        f.push(0, "s3cret\n");
        f.push(0, "123456\n");
        let out = json(&handle("get-password", body.as_bytes(), &mut f));
        assert_eq!(out["password"], "s3cret");
        assert_eq!(out["otp"], "123456");
        assert_eq!(f.calls()[0].args, ["get", "password", "a1"]);
        assert_eq!(f.calls()[1].args, ["get", "totp", "a1"]);
        let mut f = Fake::default();
        f.push(0, "s3cret");
        f.push(1, "");
        let out = json(&handle("get-password", body.as_bytes(), &mut f));
        assert_eq!(out["password"], "s3cret");
        assert!(out.get("otp").is_none());

        // Set password: get item, then edit item with base64 JSON on stdin.
        let body = format!(r#"{{"header":{{"mode":"terminal"}},"token":"{}","accountIdentifier":{{"accountID":"a1"}},"newPassword":"newpw-1"}}"#, token("SK"));
        let mut f = Fake::default();
        f.push(0, r#"{"id":"a1","type":1,"name":"Mail","login":{"username":"me","password":"old"}}"#);
        f.push(0, "{}");
        let out = json(&handle("set-password", body.as_bytes(), &mut f));
        assert_eq!(out, serde_json::json!({}));
        let calls = f.calls();
        assert_eq!(calls[0].args, ["get", "item", "a1"]);
        assert_eq!(calls[1].args, ["edit", "item", "a1"]);
        let sent = base64::engine::general_purpose::STANDARD.decode(calls[1].stdin.as_ref().unwrap()).unwrap();
        let item = json(&sent);
        assert_eq!(item["login"]["password"], "newpw-1");
        assert_eq!(item["login"]["username"], "me");
        assert_eq!(item["name"], "Mail");
        assert!(calls.iter().all(|c| !c.args.iter().any(|a| a.contains("newpw-1"))));

        // Add account (terminal): find or create the folder, then create the item.
        let body = format!(r#"{{"header":{{"mode":"terminal"}},"token":"{}","userName":"u","accountName":"Site","password":"pw-add"}}"#, token("SK"));
        let mut f = Fake::default();
        f.push(0, "[]");
        f.push(0, r#"{"id":"nf","name":"iTerm2"}"#);
        f.push(0, r#"{"id":"new1","type":1,"name":"Site"}"#);
        let out = json(&handle("add-account", body.as_bytes(), &mut f));
        assert_eq!(out["accountIdentifier"]["accountID"], "new1");
        let calls = f.calls();
        assert_eq!(calls[1].args, ["create", "folder"]);
        assert_eq!(json(&base64::engine::general_purpose::STANDARD.decode(calls[1].stdin.as_ref().unwrap()).unwrap()), serde_json::json!({"name": "iTerm2"}));
        assert_eq!(calls[2].args, ["create", "item"]);
        let item = json(&base64::engine::general_purpose::STANDARD.decode(calls[2].stdin.as_ref().unwrap()).unwrap());
        assert_eq!(item["type"], 1);
        assert_eq!(item["folderId"], "nf");
        assert_eq!(item["login"]["password"], "pw-add");
        assert!(calls.iter().all(|c| !c.args.iter().any(|a| a.contains("pw-add"))));
        // Browser mode adds to the root: no folder commands, no folderId.
        let body = format!(r#"{{"header":{{"mode":"browser"}},"token":"{}","userName":"u","accountName":"Site"}}"#, token("SK"));
        let mut f = Fake::default();
        f.push(0, r#"{"id":"new2"}"#);
        handle("add-account", body.as_bytes(), &mut f);
        assert_eq!(f.calls().len(), 1);
        let item = json(&base64::engine::general_purpose::STANDARD.decode(f.calls()[0].stdin.as_ref().unwrap()).unwrap());
        assert!(item.get("folderId").is_none());
        assert_eq!(item["login"]["password"], "");

        // Delete.
        let body = format!(r#"{{"header":{{"mode":"terminal"}},"token":"{}","accountIdentifier":{{"accountID":"a1"}}}}"#, token("SK"));
        let mut f = Fake::default();
        f.push(0, "");
        assert_eq!(json(&handle("delete-account", body.as_bytes(), &mut f)), serde_json::json!({}));
        assert_eq!(f.calls()[0].args, ["delete", "item", "a1"]);

        // An id that could pass for an option is refused before any command runs.
        let body = format!(r#"{{"header":{{"mode":"terminal"}},"token":"{}","accountIdentifier":{{"accountID":"--help"}}}}"#, token("SK"));
        let mut f = Fake::default();
        assert!(json(&handle("delete-account", body.as_bytes(), &mut f)).get("error").is_some());
        assert!(f.calls().is_empty());

        // A locked vault asks the app to log in again.
        let mut f = Fake::default();
        f.outputs.push_back(Ok((1, String::new(), "Vault is locked.".to_string())));
        let out = json(&handle("get-password", format!(r#"{{"header":{{"mode":"terminal"}},"token":"{}","accountIdentifier":{{"accountID":"a1"}}}}"#, token("SK")).as_bytes(), &mut f));
        assert_eq!(out["needsAuthentication"], true);

        // Unknown subcommand.
        let out = json(&handle("frobnicate", b"{}", &mut Fake::default()));
        assert_eq!(out["error"], "Unknown command: frobnicate");
    }

    #[test]
    fn bitwarden_output_is_capped() {
        let mut f = Fake::default();
        f.push(0, &format!("[{}]", "1,".repeat(MAX_CLI_JSON / 2 + 1)));
        let body = format!(r#"{{"header":{{"mode":"browser"}},"token":"{}"}}"#, token("SK"));
        let out = json(&handle("list-accounts", body.as_bytes(), &mut f));
        assert!(out.get("error").is_some());
    }

    #[test]
    fn adapter_main_exit_codes() {
        let args: Vec<String> = vec!["x".into(), "handshake".into()];
        let mut out = Vec::new();
        let mut empty: &[u8] = b"";
        assert_eq!(run_main(&args, &mut empty, &mut out, handle, &mut Fake::default()), 1);
        assert!(String::from_utf8(out).unwrap().contains("No input provided"));
        let mut out = Vec::new();
        let mut input: &[u8] = br#"{"iTermVersion":"3","minProtocolVersion":0,"maxProtocolVersion":0}"#;
        assert_eq!(run_main(&args, &mut input, &mut out, handle, &mut Fake::default()), 0);
        assert!(String::from_utf8(out).unwrap().contains("Bitwarden"));
    }
}
