//! The KeePassXC adapter's logic (`iterm2-keepassxc-adapter`), a port of iTerm2's
//! `pwmplugin/Sources/iterm2-keepassxc-adapter/main.swift` (`KX`) over a `pwm_run::Runner`, so it
//! is testable with a fake. The executable in `src/bin` only wires `ProcessRunner` and stdio to
//! `pwm_adapter::run_main`.
//!
//! Every `keepassxc-cli` call is a typed argument list (never a shell string):
//! `<command> --quiet [options] <database> [entry]`. The database master password is the FIRST
//! line of that call's stdin (`keepassxc-cli` reads it there), followed by the new entry password
//! twice for `add -p` / `edit -p`; it is never an argument, an environment variable, a file or a
//! log line. The password is a `Secret` from the moment it leaves the request or token, and the
//! stdin text is built in a `Secret` too. The session token the protocol requires carries the
//! master password (iTerm2's `encodeToken`; the adapter keeps no state between calls).
//!
//! `keepassxc-cli`'s output is untrusted: it is capped (`MAX_CLI_JSON`, `MAX_ENTRIES`), names go
//! through `inert_files::display_text`, an entry path with a control character is dropped, and
//! its error text is never shown (it can quote what was typed), only a fixed text. A database
//! path or entry path that starts with `-` is refused, so it can never pass for an option. The
//! database path comes only from the request header (the person's own setting). A key file is not
//! supported (the handshake declares no setting for one), so none is ever passed.

use std::time::Duration;

use crate::inert_files::display_text;
use crate::pwm::Secret;
use crate::pwm_adapter::*;
use crate::pwm_run::{Output, Request, Runner};

/// The CLI looked up on PATH when the handshake's `pathToExecutable` is empty.
pub const CLI: &str = "keepassxc-cli";

/// The group terminal-mode entries live in (`modeTag`).
const FOLDER: &str = "iTerm2";

/// KeePassXC's recycle bin group (`main.swift` skips it).
const RECYCLE_BIN: &str = "Recycle Bin";

/// One `keepassxc-cli` call opens the database (a key derivation), so it is given a while.
const TIMEOUT: Duration = Duration::from_secs(60);

/// The most entries one listing reads (each costs one `show` call).
const MAX_ENTRIES: usize = 1000;

/// Names, user names and entry paths are shown in the window, so they are capped.
const MAX_NAME: usize = 200;
const MAX_PATH: usize = 1024;

/// Why a step failed: a fixed text.
struct Failure(String);

fn fail(text: &str) -> Failure {
    Failure(text.to_string())
}

fn bad_request(e: String) -> Failure {
    Failure(e)
}

struct Kx<'a> {
    program: String,
    database: String,
    password: Secret,
    runner: &'a mut dyn Runner,
}

impl<'a> Kx<'a> {
    fn new(header: &RequestHeader, password: Secret, runner: &'a mut dyn Runner) -> Result<Kx<'a>, Failure> {
        let database = header.path_to_database.clone().filter(|p| !p.is_empty()).ok_or_else(|| fail("Database path is required in request header"))?;
        if database.starts_with('-') || database.contains('\0') || database.contains(['\r', '\n']) {
            return Err(fail("Invalid database path."));
        }
        let program = header.path_to_executable.clone().filter(|p| !p.is_empty()).unwrap_or_else(|| CLI.to_string());
        Ok(Kx { program, database, password, runner })
    }

    /// Runs `keepassxc-cli <command> --quiet <options> <database> <args>` with the database
    /// password, then `extra` (each followed by a line break), on stdin. `Err(())` when the
    /// program could not run.
    fn run(&mut self, command: &str, options: &[&str], args: &[&str], extra: &[&Secret]) -> Result<Output, ()> {
        let mut argv = vec![command.to_string(), "--quiet".to_string()];
        argv.extend(options.iter().map(|o| o.to_string()));
        argv.push(self.database.clone());
        argv.extend(args.iter().map(|a| a.to_string()));
        let mut stdin = Secret::new(String::with_capacity(self.password.expose().len() + 2 + extra.iter().map(|e| e.expose().len() + 1).sum::<usize>()));
        stdin.push_str(self.password.expose());
        stdin.push_str("\n");
        for e in extra {
            stdin.push_str(e.expose());
            stdin.push_str("\n");
        }
        let mut r = Request::new(self.program.clone(), argv);
        r.stdin = Some(stdin);
        r.timeout = Some(TIMEOUT);
        self.runner.run(r).map_err(|_| ())
    }

    /// Like `run`, but a non-zero exit, a program that cannot run, or output over the cap is
    /// the fixed `what` text.
    fn ok(&mut self, command: &str, options: &[&str], args: &[&str], extra: &[&Secret], what: &str) -> Result<Secret, Failure> {
        match self.run(command, options, args, extra) {
            Ok(out) if out.code == 0 && out.stdout.expose().len() <= MAX_CLI_JSON => Ok(out.stdout),
            Ok(out) if out.code == 0 => Err(fail(&format!("{what} The output was too large."))),
            Ok(_) => Err(fail(what)),
            Err(()) => Err(fail(&format!("{what} Is keepassxc-cli installed?"))),
        }
    }
}

/// An entry path goes on a command line, so one that could pass for an option, or has a control
/// character, is refused.
fn valid_entry(path: &str) -> bool {
    !path.is_empty() && path.len() <= MAX_PATH && !path.starts_with('-') && !path.chars().any(|c| c.is_control())
}

fn entry_of(id: &AccountIdentifier) -> Result<&str, Failure> {
    if valid_entry(&id.account_id) { Ok(&id.account_id) } else { Err(fail("Invalid account identifier.")) }
}

fn password_of(token: Option<&str>) -> Result<Secret, Failure> {
    decode_token(token).ok_or_else(|| fail("Invalid or missing token. Please login first."))
}

fn is_terminal(header: &RequestHeader) -> bool {
    header.mode != "browser"
}

/// The entry paths `ls --flatten --recursive` printed that the mode shows (`main.swift`
/// `handleListAccounts`): the recycle bin and empty-group markers skipped; terminal mode keeps
/// the entries under `iTerm2/`, browser mode the root-level ones.
fn entry_paths(listing: &str, terminal: bool) -> Vec<String> {
    let prefix = format!("{FOLDER}/");
    listing
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .filter(|l| !l.starts_with(&format!("{RECYCLE_BIN}/")) && *l != RECYCLE_BIN)
        .filter(|l| if terminal { l.starts_with(&prefix) && *l != FOLDER && *l != prefix && !l.ends_with("/[empty]") && !l.ends_with('/') } else { !l.contains('/') })
        .filter(|l| valid_entry(l))
        .map(|l| l.to_string())
        .collect()
}

fn handshake(stdin: &[u8]) -> Result<Vec<u8>, Failure> {
    let request: HandshakeRequest = decode_request(stdin, "handshake").map_err(bad_request)?;
    if request.max_protocol_version < PROTOCOL {
        return Err(fail("Protocol version 0 is required but not supported by client"));
    }
    Ok(ok_json(&HandshakeResponse {
        protocol_version: PROTOCOL,
        name: "KeePassXC".to_string(),
        requires_master_password: true,
        can_set_passwords: true,
        user_accounts: None,
        needs_path_to_database: true,
        database_extension: Some("kdbx".to_string()),
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
    let mut request: LoginRequest = decode_request(stdin, "login").map_err(bad_request)?;
    let password = Secret::new(std::mem::take(request.master_password.get_or_insert_with(String::new)));
    if request.header.path_to_database.as_deref().is_none_or(str::is_empty) {
        return Err(fail("Database path is required in request header"));
    }
    if password.is_empty() {
        return Err(fail("Master password is required"));
    }
    let mut kx = Kx::new(&request.header, password.duplicate(), runner)?;
    // The password is verified by opening the database.
    kx.ok("ls", &[], &[], &[], "Invalid master password or database error.")?;
    Ok(ok_json(&LoginResponse { token: Some(encode_token(&password)) }))
}

/// How long a listing reads entry details (one `show` call each, a key derivation every time;
/// a call itself is cut at 60 s). Termurion kills the adapter at 180 s, so after this the remaining entries are listed by name
/// only (no user name, no OTP flag) with a warning, instead of the whole listing being lost.
const DETAIL_BUDGET: Duration = Duration::from_secs(100);

fn list_accounts(stdin: &[u8], runner: &mut dyn Runner) -> Result<Vec<u8>, Failure> {
    list_accounts_within(stdin, runner, DETAIL_BUDGET)
}

fn list_accounts_within(stdin: &[u8], runner: &mut dyn Runner, budget: Duration) -> Result<Vec<u8>, Failure> {
    let request: ListAccountsRequest = decode_request(stdin, "list-accounts").map_err(bad_request)?;
    let password = password_of(request.token.as_deref())?;
    let mut kx = Kx::new(&request.header, password, runner)?;
    let listing = kx.ok("ls", &["--flatten", "--recursive"], &[], &[], "Failed to list accounts.")?;
    let mut paths = entry_paths(listing.expose(), is_terminal(&request.header));
    let mut warning = (paths.len() > MAX_ENTRIES).then(|| format!("The database has more than {MAX_ENTRIES} entries; only the first {MAX_ENTRIES} are shown."));
    paths.truncate(MAX_ENTRIES);
    let mut accounts = Vec::with_capacity(paths.len());
    let started = std::time::Instant::now();
    let mut unread = 0usize;
    for path in paths {
        let name = path.rsplit('/').next().unwrap_or(&path).to_string();
        let mut user_name = String::new();
        let mut has_otp = false;
        let in_time = started.elapsed() < budget;
        if !in_time {
            unread += 1;
        }
        if let Some(out) = in_time.then(|| kx.run("show", &["-s"], &[&path], &[]).ok()).flatten() {
            if out.code == 0 && out.stdout.expose().len() <= MAX_CLI_JSON {
                for line in out.stdout.expose().lines() {
                    let line = line.trim();
                    if let Some(v) = line.strip_prefix("UserName: ") {
                        user_name = display_text(v, MAX_NAME, false);
                    }
                    if line.starts_with("TOTP") {
                        has_otp = true;
                    }
                }
            }
        }
        accounts.push(Account {
            identifier: AccountIdentifier { account_id: path },
            user_name,
            account_name: display_text(&name, MAX_NAME, false),
            has_otp,
            source_label: None,
        });
    }
    if unread > 0 {
        let note = format!("Reading the database took too long: {unread} entries are listed by name only, without user names.");
        warning = Some(warning.map_or(note.clone(), |w| format!("{w} {note}")));
    }
    Ok(ok_json(&ListAccountsResponse { accounts, warning }))
}

fn get_password(stdin: &[u8], runner: &mut dyn Runner) -> Result<Vec<u8>, Failure> {
    let request: GetPasswordRequest = decode_request(stdin, "get-password").map_err(bad_request)?;
    let password = password_of(request.token.as_deref())?;
    let entry = entry_of(&request.account_identifier)?;
    let mut kx = Kx::new(&request.header, password, runner)?;
    let value = kx.ok("show", &["-s", "-a", "Password"], &[entry], &[], "Failed to get password.")?;
    // The TOTP is optional: none, or a failure, just leaves it out.
    let otp = kx
        .ok("show", &["--totp"], &[entry], &[], "")
        .ok()
        .map(|o| o.expose().trim().to_string())
        .filter(|t| !t.is_empty() && !t.contains("ERROR"));
    Ok(ok_json(&Password { password: value.expose().trim().to_string(), otp }))
}

fn set_password(stdin: &[u8], runner: &mut dyn Runner) -> Result<Vec<u8>, Failure> {
    let mut request: SetPasswordRequest = decode_request(stdin, "set-password").map_err(bad_request)?;
    let password = password_of(request.token.as_deref())?;
    let Some(new_password) = request.new_password.take().map(Secret::new) else {
        return Err(fail("New password is required"));
    };
    let entry = entry_of(&request.account_identifier)?;
    let mut kx = Kx::new(&request.header, password, runner)?;
    // `edit -p` asks for the new password and its confirmation on the same stdin.
    kx.ok("edit", &["-p"], &[entry], &[&new_password, &new_password], "Failed to set password.")?;
    Ok(ok_json(&SetPasswordResponse {}))
}

fn add_account(stdin: &[u8], runner: &mut dyn Runner) -> Result<Vec<u8>, Failure> {
    let mut request: AddAccountRequest = decode_request(stdin, "add-account").map_err(bad_request)?;
    let password = password_of(request.token.as_deref())?;
    let entry_password = request.password.take().map(Secret::new);
    let name = request.account_name.as_str();
    if name.is_empty() || name.contains('/') || !valid_entry(name) {
        return Err(fail("Invalid account name."));
    }
    // The user name is the value of `-u`; one that starts with `-` could pass for an option.
    if request.user_name.starts_with('-') || request.user_name.chars().any(|c| c.is_control()) {
        return Err(fail("Invalid user name."));
    }
    let terminal = is_terminal(&request.header);
    let entry = if terminal { format!("{FOLDER}/{name}") } else { name.to_string() };
    let mut kx = Kx::new(&request.header, password, runner)?;
    if terminal {
        // Ensure the mode folder exists.
        let top = kx.ok("ls", &[], &[], &[], "Failed to list the database.")?;
        if !top.expose().lines().any(|l| l.trim().trim_end_matches('/') == FOLDER) {
            kx.ok("mkdir", &[], &[FOLDER], &[], "Failed to create mode folder.")?;
        }
    }
    // An entry of the same name in the recycle bin is removed for good first.
    let bin = format!("{RECYCLE_BIN}/{entry}");
    if kx.run("show", &["-s", "-a", "Title"], &[&bin], &[]).is_ok_and(|o| o.code == 0) {
        let _ = kx.run("rm", &[], &[&bin], &[]);
    }
    let user = request.user_name.as_str();
    let result = match &entry_password {
        Some(p) => kx.ok("add", &["-p", "-u", user], &[&entry], &[p, p], "Failed to add account."),
        None => kx.ok("add", &["-u", user], &[&entry], &[], "Failed to add account."),
    };
    result?;
    Ok(ok_json(&AddAccountResponse { account_identifier: AccountIdentifier { account_id: entry } }))
}

fn delete_account(stdin: &[u8], runner: &mut dyn Runner) -> Result<Vec<u8>, Failure> {
    let request: DeleteAccountRequest = decode_request(stdin, "delete-account").map_err(bad_request)?;
    let password = password_of(request.token.as_deref())?;
    let entry = entry_of(&request.account_identifier)?;
    let mut kx = Kx::new(&request.header, password, runner)?;
    kx.ok("rm", &[], &[entry], &[], "Failed to delete account.")?;
    Ok(ok_json(&DeleteAccountResponse {}))
}

/// One adapter call: the JSON to print (an `ErrorResponse` for any failure or an unknown
/// subcommand).
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
    result.unwrap_or_else(|f| error_json(&f.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pwm_run::Output;
    use std::collections::VecDeque;

    #[derive(Clone, Debug)]
    struct Recorded {
        program: String,
        args: Vec<String>,
        env_len: usize,
        stdin: Option<String>,
    }

    #[derive(Default)]
    struct Fake {
        log: Vec<Recorded>,
        outputs: VecDeque<Result<(i32, String), String>>,
    }

    impl Fake {
        fn push(&mut self, code: i32, out: &str) {
            self.outputs.push_back(Ok((code, out.to_string())));
        }
    }

    impl Runner for Fake {
        fn run(&mut self, r: Request) -> Result<Output, String> {
            self.log.push(Recorded { program: r.program.clone(), args: r.args.clone(), env_len: r.env.len(), stdin: r.stdin.as_ref().map(|s| s.expose().to_string()) });
            match self.outputs.pop_front() {
                Some(Ok((code, stdout))) => Ok(Output { code, stdout: Secret::new(stdout), stderr: "stderr-quotes-dbpw".to_string() }),
                Some(Err(e)) => Err(e),
                None => Err("Fake: no more outputs".to_string()),
            }
        }
    }

    fn json(bytes: &[u8]) -> serde_json::Value {
        serde_json::from_slice(bytes).unwrap()
    }

    fn token(pw: &str) -> String {
        encode_token(&Secret::new(pw.to_string()))
    }

    fn body(extra: &str, mode: &str) -> String {
        format!(r#"{{"header":{{"pathToDatabase":"C:\\kp\\db.kdbx","pathToExecutable":"C:\\kp\\keepassxc-cli.exe","mode":"{mode}"}},"token":"{}"{extra}}}"#, token("dbpw"))
    }

    fn no_secret_in_args(f: &Fake) {
        for c in &f.log {
            assert!(!c.args.iter().any(|a| a.contains("dbpw") || a.contains("newpw") || a.contains("pw-add")), "{:?}", c.args);
            assert_eq!(c.env_len, 0);
            assert!(c.stdin.as_ref().is_some_and(|s| s.starts_with("dbpw\n")), "{:?}", c.stdin);
        }
    }

    #[test]
    fn keepassxc_adapter_follows_iterm2() {
        let mut f = Fake::default();
        let out = json(&handle("handshake", br#"{"iTermVersion":"3","minProtocolVersion":0,"maxProtocolVersion":0}"#, &mut f));
        assert_eq!(out["protocolVersion"], 0);
        assert_eq!(out["name"], "KeePassXC");
        assert_eq!(out["requiresMasterPassword"], true);
        assert_eq!(out["canSetPasswords"], true);
        assert_eq!(out["needsPathToDatabase"], true);
        assert_eq!(out["databaseExtension"], "kdbx");
        assert_eq!(out["needsPathToExecutable"], "keepassxc-cli");
        assert!(f.log.is_empty());

        // Login: ls with the password on stdin only; the token carries it for later calls.
        let mut f = Fake::default();
        f.push(0, "iTerm2/\n");
        let login = r#"{"header":{"pathToDatabase":"C:\\kp\\db.kdbx","pathToExecutable":"C:\\kp\\keepassxc-cli.exe","mode":"terminal"},"masterPassword":"dbpw"}"#;
        let out = json(&handle("login", login.as_bytes(), &mut f));
        assert_eq!(decode_token(out["token"].as_str()).unwrap().expose(), "dbpw");
        assert_eq!(f.log[0].program, "C:\\kp\\keepassxc-cli.exe");
        assert_eq!(f.log[0].args, ["ls", "--quiet", "C:\\kp\\db.kdbx"]);
        assert_eq!(f.log[0].stdin.as_deref(), Some("dbpw\n"));
        no_secret_in_args(&f);
        // A failure never echoes keepassxc's output, and an empty password is refused.
        let mut f = Fake::default();
        f.push(1, "Error: dbpw wrong");
        let text = String::from_utf8(handle("login", login.as_bytes(), &mut f)).unwrap();
        assert!(text.contains("Invalid master password or database error."));
        assert!(!text.contains("dbpw") && !text.contains("stderr-quotes"));
        let out = json(&handle("login", br#"{"header":{"pathToDatabase":"d.kdbx","mode":"terminal"}}"#, &mut Fake::default()));
        assert_eq!(out["error"], "Master password is required");
        let out = json(&handle("login", br#"{"header":{"mode":"terminal"},"masterPassword":"x"}"#, &mut Fake::default()));
        assert_eq!(out["error"], "Database path is required in request header");

        // List accounts: flatten + recursive, the recycle bin and empty groups skipped, only the
        // iTerm2 folder, then show -s per entry for the user name and the TOTP.
        let mut f = Fake::default();
        f.push(0, "iTerm2/\niTerm2/Mail\niTerm2/Bank\niTerm2/Sub/[empty]\nOther/Thing\nRecycle Bin/\nRecycle Bin/iTerm2/Old\nRoot\n");
        f.push(0, "Title: Mail\nUserName: me@x\nPassword: PROTECTED\nTOTP Seed: abc\n");
        f.push(0, "Title: Bank\nUserName: bob\n");
        let out = json(&handle("list-accounts", body("", "terminal").as_bytes(), &mut f));
        assert_eq!(f.log[0].args, ["ls", "--quiet", "--flatten", "--recursive", "C:\\kp\\db.kdbx"]);
        assert_eq!(f.log[1].args, ["show", "--quiet", "-s", "C:\\kp\\db.kdbx", "iTerm2/Mail"]);
        assert_eq!(f.log.len(), 3);
        no_secret_in_args(&f);
        let accounts = out["accounts"].as_array().unwrap();
        assert_eq!(accounts.len(), 2);
        assert_eq!(accounts[0]["identifier"]["accountID"], "iTerm2/Mail");
        assert_eq!(accounts[0]["accountName"], "Mail");
        assert_eq!(accounts[0]["userName"], "me@x");
        assert_eq!(accounts[0]["hasOTP"], true);
        assert_eq!(accounts[1]["userName"], "bob");
        assert_eq!(accounts[1]["hasOTP"], false);
        // Browser mode: root-level entries only.
        let mut f = Fake::default();
        f.push(0, "iTerm2/Mail\nRoot\nRecycle Bin/X\n");
        f.push(0, "UserName: r\n");
        let out = json(&handle("list-accounts", body("", "browser").as_bytes(), &mut f));
        assert_eq!(out["accounts"].as_array().unwrap().len(), 1);
        assert_eq!(out["accounts"][0]["identifier"]["accountID"], "Root");

        // Get password: the value trimmed, then the TOTP when present.
        let mut f = Fake::default();
        f.push(0, "s3cret \n");
        f.push(0, "123456\n");
        let req = body(r#","accountIdentifier":{"accountID":"iTerm2/Mail"}"#, "terminal");
        let out = json(&handle("get-password", req.as_bytes(), &mut f));
        assert_eq!(out["password"], "s3cret");
        assert_eq!(out["otp"], "123456");
        assert_eq!(f.log[0].args, ["show", "--quiet", "-s", "-a", "Password", "C:\\kp\\db.kdbx", "iTerm2/Mail"]);
        assert_eq!(f.log[1].args, ["show", "--quiet", "--totp", "C:\\kp\\db.kdbx", "iTerm2/Mail"]);
        no_secret_in_args(&f);
        let mut f = Fake::default();
        f.push(0, "s3cret\n");
        f.push(1, "ERROR no totp");
        let out = json(&handle("get-password", req.as_bytes(), &mut f));
        assert!(out.get("otp").is_none());

        // Set password: edit -p with the new password twice after the database password.
        let req = body(r#","accountIdentifier":{"accountID":"iTerm2/Mail"},"newPassword":"newpw-1""#, "terminal");
        let mut f = Fake::default();
        f.push(0, "");
        assert_eq!(json(&handle("set-password", req.as_bytes(), &mut f)), serde_json::json!({}));
        assert_eq!(f.log[0].args, ["edit", "--quiet", "-p", "C:\\kp\\db.kdbx", "iTerm2/Mail"]);
        assert_eq!(f.log[0].stdin.as_deref(), Some("dbpw\nnewpw-1\nnewpw-1\n"));
        no_secret_in_args(&f);

        // Add with a password: folder exists, nothing in the recycle bin, add -p -u.
        let req = body(r#","userName":"u1","accountName":"Site","password":"pw-add""#, "terminal");
        let mut f = Fake::default();
        f.push(0, "iTerm2/\nOther/\n");
        f.push(1, "");
        f.push(0, "");
        let out = json(&handle("add-account", req.as_bytes(), &mut f));
        assert_eq!(out["accountIdentifier"]["accountID"], "iTerm2/Site");
        assert_eq!(f.log[0].args, ["ls", "--quiet", "C:\\kp\\db.kdbx"]);
        assert_eq!(f.log[1].args, ["show", "--quiet", "-s", "-a", "Title", "C:\\kp\\db.kdbx", "Recycle Bin/iTerm2/Site"]);
        assert_eq!(f.log[2].args, ["add", "--quiet", "-p", "-u", "u1", "C:\\kp\\db.kdbx", "iTerm2/Site"]);
        assert_eq!(f.log[2].stdin.as_deref(), Some("dbpw\npw-add\npw-add\n"));
        no_secret_in_args(&f);
        // Add without a password: -u only and the database password alone; a missing folder is
        // made; an entry of the same name in the recycle bin is removed first.
        let req = body(r#","userName":"u2","accountName":"Site""#, "terminal");
        let mut f = Fake::default();
        f.push(0, "Other/\n");
        f.push(0, "");
        f.push(0, "Title: Site");
        f.push(0, "");
        f.push(0, "");
        json(&handle("add-account", req.as_bytes(), &mut f));
        assert_eq!(f.log[1].args, ["mkdir", "--quiet", "C:\\kp\\db.kdbx", "iTerm2"]);
        assert_eq!(f.log[3].args, ["rm", "--quiet", "C:\\kp\\db.kdbx", "Recycle Bin/iTerm2/Site"]);
        assert_eq!(f.log[4].args, ["add", "--quiet", "-u", "u2", "C:\\kp\\db.kdbx", "iTerm2/Site"]);
        assert_eq!(f.log[4].stdin.as_deref(), Some("dbpw\n"));
        // Browser mode adds to the root: no folder commands.
        let req = body(r#","userName":"u3","accountName":"Root""#, "browser");
        let mut f = Fake::default();
        f.push(1, "");
        f.push(0, "");
        let out = json(&handle("add-account", req.as_bytes(), &mut f));
        assert_eq!(out["accountIdentifier"]["accountID"], "Root");
        assert_eq!(f.log.len(), 2);

        // Delete, and iTerm2's failure text when the entry is gone.
        let req = body(r#","accountIdentifier":{"accountID":"iTerm2/Mail"}"#, "terminal");
        let mut f = Fake::default();
        f.push(0, "");
        assert_eq!(json(&handle("delete-account", req.as_bytes(), &mut f)), serde_json::json!({}));
        assert_eq!(f.log[0].args, ["rm", "--quiet", "C:\\kp\\db.kdbx", "iTerm2/Mail"]);
        let mut f = Fake::default();
        f.push(1, "Could not find entry with path iTerm2/Mail. dbpw");
        let text = String::from_utf8(handle("delete-account", req.as_bytes(), &mut f)).unwrap();
        assert!(text.contains("Failed to delete account."));
        assert!(!text.contains("dbpw"));

        // Missing token; unknown subcommand.
        let out = json(&handle("list-accounts", br#"{"header":{"pathToDatabase":"d.kdbx","mode":"terminal"}}"#, &mut Fake::default()));
        assert_eq!(out["error"], "Invalid or missing token. Please login first.");
        assert_eq!(json(&handle("frobnicate", b"{}", &mut Fake::default()))["error"], "Unknown command: frobnicate");
    }

    #[test]
    fn keepassxc_refuses_what_could_pass_for_an_option() {
        let get = |db: &str, id: &str| {
            let mut f = Fake::default();
            let req = format!(r#"{{"header":{{"pathToDatabase":"{db}","mode":"terminal"}},"token":"{}","accountIdentifier":{{"accountID":"{id}"}}}}"#, token("dbpw"));
            let out = json(&handle("get-password", req.as_bytes(), &mut f));
            (out, f.log.len())
        };
        let (out, ran) = get("-x.kdbx", "iTerm2/a");
        assert!(out.get("error").is_some() && ran == 0);
        let (out, ran) = get("d.kdbx", "--help");
        assert!(out.get("error").is_some() && ran == 0);
        let (out, ran) = get("d.kdbx", "iTerm2/a\\nb");
        assert!(out.get("error").is_some() && ran == 0);
        let add = |name: &str, user: &str| {
            let mut f = Fake::default();
            let req = format!(r#"{{"header":{{"pathToDatabase":"d.kdbx","mode":"browser"}},"token":"{}","userName":"{user}","accountName":"{name}"}}"#, token("dbpw"));
            let out = json(&handle("add-account", req.as_bytes(), &mut f));
            (out, f.log.len())
        };
        assert!(add("-x", "u").0.get("error").is_some());
        assert!(add("a/b", "u").0.get("error").is_some());
        let (out, ran) = add("ok", "-u");
        assert!(out.get("error").is_some() && ran == 0);
    }

    #[test]
    fn a_slow_listing_keeps_every_name_and_warns() {
        let req = serde_json::json!({"header": {"pathToDatabase": "C:\\db.kdbx", "mode": "terminal"}, "token": encode_token(&Secret::new("pw".to_string()))});
        let mut fake = Fake::default();
        fake.outputs.push_back(Ok((0, "iTerm2/a\niTerm2/b\n".to_string())));
        let out = list_accounts_within(req.to_string().as_bytes(), &mut fake, Duration::ZERO).ok().unwrap();
        let out: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(out["accounts"].as_array().unwrap().len(), 2, "{out}");
        assert!(out["warning"].as_str().unwrap().contains("2 entries are listed by name only"), "{out}");
        assert_eq!(fake.log.len(), 1, "no show call once the budget is spent");
    }

    #[test]
    fn keepassxc_listing_is_untrusted_and_capped() {
        // Names go through display_text; an entry path with a control character is dropped.
        let mut f = Fake::default();
        f.push(0, "iTerm2/Ma\u{202e}il\niTerm2/bad\u{7}one\n");
        f.push(0, "UserName: us\u{202e}er\n");
        let out = json(&handle("list-accounts", body("", "terminal").as_bytes(), &mut f));
        let accounts = out["accounts"].as_array().unwrap();
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0]["accountName"], "Mail");
        assert_eq!(accounts[0]["userName"], "user");
        // Too many entries: capped, with a warning.
        let mut f = Fake::default();
        let many: String = (0..MAX_ENTRIES + 5).map(|i| format!("iTerm2/e{i}\n")).collect();
        f.push(0, &many);
        let out = json(&handle("list-accounts", body("", "terminal").as_bytes(), &mut f));
        assert_eq!(out["accounts"].as_array().unwrap().len(), MAX_ENTRIES);
        assert!(out["warning"].as_str().unwrap().contains("first"));
        // Output over the cap is refused.
        let mut f = Fake::default();
        f.push(0, &"x".repeat(MAX_CLI_JSON + 1));
        assert!(json(&handle("list-accounts", body("", "terminal").as_bytes(), &mut f)).get("error").is_some());
    }

    #[test]
    fn keepassxc_source_never_logs_or_spawns_a_shell() {
        let src = include_str!("pwm_keepassxc.rs");
        let production = src.split("#[cfg(test)]").next().unwrap();
        for bad in ["rlog!", "println!", "eprintln!", "cmd.exe", "powershell", "sh -c", "std::env::set_var", "File::create", "fs::write"] {
            assert!(!production.contains(bad), "{bad}");
        }
    }
}
