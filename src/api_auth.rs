//! Authorization for the Python API: the cookie jar, the connection key, the allow-all file the
//! `iterm2` package also reads, iTerm2's authorization decision and every alert text. Pure; the
//! app side (`app/api_ui.rs`) reads the file, shows the alerts and applies the steps.

use std::collections::{HashMap, VecDeque};

/// How long a cookie lasts: a Single cookie authorizes one connection (`J:50-52`); a Session
/// cookie authorizes any number until removed (Tandem's session cookies, a Decision; iTerm2's
/// reusable cookies expire instead, `J:40-48`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CookieUse {
    Single,
    Session,
}

/// The cookies Tandem has handed out (`iTermWebSocketCookieJar`). Cookies are secrets: `Debug`
/// shows only how many there are, and a lookup compares every cookie in constant time.
#[derive(Clone, Default)]
pub struct Jar {
    cookies: HashMap<String, CookieUse>,
}

impl std::fmt::Debug for Jar {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Jar").field("cookies", &self.cookies.len()).finish()
    }
}

impl Jar {
    /// A new cookie from 16 random bytes (`J:56-72`, `J:83-101`).
    pub fn mint(&mut self, kind: CookieUse) -> String {
        let mut bytes = [0u8; 16];
        getrandom::fill(&mut bytes).expect("the system random source failed");
        self.mint_from(kind, bytes)
    }

    /// `mint` with the bytes given (tests).
    pub fn mint_from(&mut self, kind: CookieUse, bytes: [u8; 16]) -> String {
        let cookie = cookie_text(bytes);
        self.add(&cookie, kind);
        cookie
    }

    /// Adds a cookie made elsewhere (a restored session's "Reusable Cookie").
    pub fn add(&mut self, cookie: &str, kind: CookieUse) {
        self.cookies.insert(cookie.to_string(), kind);
    }

    /// Whether `cookie` is in the jar; a Single cookie is removed by its use (`J:35-54`). Every
    /// stored cookie is compared, each in constant time, so the time taken says nothing about how
    /// close a guess came.
    pub fn consume(&mut self, cookie: &str) -> bool {
        let mut found: Option<(&String, CookieUse)> = None;
        for (k, &kind) in &self.cookies {
            if same_secret(k.as_bytes(), cookie.as_bytes()) {
                found = Some((k, kind));
            }
        }
        match found {
            Some((k, CookieUse::Single)) => {
                let k = k.clone();
                self.cookies.remove(&k);
                true
            }
            Some((_, CookieUse::Session)) => true,
            None => false,
        }
    }

    /// Forgets a cookie (`J:104-109`).
    pub fn remove(&mut self, cookie: &str) {
        self.cookies.remove(cookie);
    }

    pub fn len(&self) -> usize {
        self.cookies.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cookies.is_empty()
    }
}

/// Byte equality whose time depends only on the lengths (cookies all have 32 characters, so the
/// length gives nothing away).
fn same_secret(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let diff = a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y));
    std::hint::black_box(diff) == 0
}

/// A cookie's text: each byte as two lowercase hex digits (`%02x`, `J:68`).
pub fn cookie_text(bytes: [u8; 16]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A fresh connection key for a client that sent no `x-iterm2-key`: an uppercase UUID, as
/// `[[NSUUID UUID] UUIDString]` gives (`W:171`).
pub fn fresh_key() -> String {
    uuid::Uuid::new_v4().hyphenated().to_string().to_uppercase()
}

/// The allow-all file's magic (`H:68`, `PY/auth.py:143`).
pub const NOAUTH_MAGIC: &str = "61DF88DC-3423-4823-B725-22570E01C027";

/// The allow-all file's path exactly as the `iterm2` package builds it on Windows:
/// `os.path.expanduser("~/Library/Application Support/iTerm2/disable-automation-auth")`, which is
/// `USERPROFILE` followed by the rest with its forward slashes (`PY/auth.py:139-141`). The
/// package's `IT2_SUITE` override is not followed: Tandem is always "iTerm2" to the package.
pub fn noauth_path(userprofile: &str) -> String {
    format!("{userprofile}/Library/Application Support/iTerm2/disable-automation-auth")
}

/// The contents that make the file valid: the lowercase hex of the path's UTF-8, a space, then
/// the magic (`H:392-396`, `PY/auth.py:144`), with no line end.
pub fn noauth_contents(path: &str) -> String {
    let hex: String = path.bytes().map(|b| format!("{b:02x}")).collect();
    format!("{hex} {NOAUTH_MAGIC}")
}

/// What the allow-all file says (`iTermNoAuthStatus`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AllowAll {
    /// No file (or unreadable): a cookie is required.
    None,
    /// Exactly the expected contents: every program running as the user, at Tandem's elevation,
    /// may connect without a cookie.
    Valid,
    /// Any other contents: a cookie is required, and the reset warning is due.
    Corrupt,
}

/// The file's status from its contents (`None` when it could not be read), compared exactly as
/// iTerm2 and the package do (`H:411-432`, `PY/auth.py:145-151`). A Valid file is a user-level
/// trust grant: any program running as the user can write it (on macOS it must be root-owned;
/// Windows has no such owner check, a Decision), so it is only ever read, never inferred.
pub fn noauth_status(path: &str, contents: Option<&str>) -> AllowAll {
    match contents {
        None => AllowAll::None,
        Some(c) if c == noauth_contents(path) => AllowAll::Valid,
        Some(_) => AllowAll::Corrupt,
    }
}

/// The name a connection shows in alerts and the Log: "\u{2248}" and the advisory name, or
/// "Unknown" (`H:1547`).
pub fn display_name(advisory: Option<&str>) -> String {
    match advisory {
        Some(n) => format!("\u{2248}{n}"),
        None => UNKNOWN_NAME.to_string(),
    }
}

/// The authorization decision: allow or deny with the reason to log, or ask the user.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    Allow(&'static str),
    Deny(&'static str),
    /// The "Allow Python API Usage?" alert (heading `ALLOW_HEADING`, identifier
    /// `NoSyncAllowPythonAPI`, never silenced) with this text and these buttons.
    Ask { text: String, buttons: Vec<&'static str> },
}

/// iTerm2's decision order (`H:1541-1583`): a valid cookie ("preauthorized") allows; then a
/// valid allow-all file; then a client that disabled the UI (`x-iterm2-disable-auth-ui` present)
/// is refused; otherwise the user is asked, with Always offered only while `setCookie` is off and
/// the no-cookie suffix added while it is on.
pub fn decide(preauthorized: bool, allow_all: bool, disable_auth_ui: bool, set_cookie: bool) -> Verdict {
    if preauthorized {
        return Verdict::Allow(REASON_USER_LAUNCHED);
    }
    if allow_all {
        return Verdict::Allow(REASON_ALL_APPS);
    }
    if disable_auth_ui {
        return Verdict::Deny(REASON_NO_COOKIE);
    }
    if set_cookie {
        Verdict::Ask { text: format!("{ALLOW_TEXT}{NO_COOKIE_SUFFIX}"), buttons: vec![OK, CANCEL, MORE_INFO] }
    } else {
        Verdict::Ask { text: ALLOW_TEXT.to_string(), buttons: vec![OK, CANCEL, MORE_INFO, ALWAYS] }
    }
}

/// The alert's answer (`H:1584-1617`): Allow logs `REASON_ALLOWED`; Deny and MoreInfo log
/// `REASON_DENIED` (MoreInfo also opens `SECURITY_URL`); Always asks `CONFIRM_ALWAYS` (heading
/// `CONFIRM_HEADING`, identifier `NoSyncConfirmAlways`, OK and Cancel), whose OK turns
/// `setCookie` on and allows, and whose Cancel denies.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AskAnswer {
    Allow,
    Deny,
    MoreInfo,
    Always,
}

/// The answer for a button index; a closed alert (any other index) is Deny.
pub fn ask_answer(index: usize) -> AskAnswer {
    match index {
        0 => AskAnswer::Allow,
        2 => AskAnswer::MoreInfo,
        3 => AskAnswer::Always,
        _ => AskAnswer::Deny,
    }
}

/// What a change of "Enable Python API" asks for (`H:629-656`, `H:782-795`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EnableStep {
    Nothing,
    /// Show the Enable warning (`ENABLE_HEADING`, `ENABLE_TEXT`, `ENABLE_ID`, OK and Cancel,
    /// persistent); OK starts the server, Cancel turns the setting back off.
    Confirm,
    /// Stop the server, disconnecting every script.
    Stop,
}

/// The step for the setting's value and whether the server runs.
pub fn enable_step(setting: bool, running: bool) -> EnableStep {
    match (setting, running) {
        (true, false) => EnableStep::Confirm,
        (false, true) => EnableStep::Stop,
        _ => EnableStep::Nothing,
    }
}

/// What a choice in the authentication popup asks for (`H:466-560`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AllowAllStep {
    Nothing,
    /// Show the allow-all warning (`ALLOW_ALL_HEADING`, `ALLOW_ALL_TEXT`, OK, Cancel and More
    /// Info opening `AUTH_URL`, identifier `NoSyncRequireApplescriptAuth`); OK writes the file
    /// through a temporary file and a move.
    Warn,
    /// Delete the file; a failure turns the API off and shows `remove_failed_text`.
    Remove,
}

/// The step for the file's status and the chosen method. A corrupt file already means "require a
/// cookie", so choosing that changes nothing, as in iTerm2 (`H:522-526`).
pub fn allow_all_step(now: AllowAll, want_allow_all: bool) -> AllowAllStep {
    match (now == AllowAll::Valid, want_allow_all) {
        (false, true) => AllowAllStep::Warn,
        (true, false) => AllowAllStep::Remove,
        _ => AllowAllStep::Nothing,
    }
}

// The alert texts (iTerm2's, with "iTerm2" replaced by "Tandem" and Mac words by Windows words).
pub const ENABLE_HEADING: &str = "Enable Python API?";
pub const ENABLE_TEXT: &str = "The Python API allows scripts you run to control Tandem and access all its data.";
pub const ENABLE_ID: &str = "NoSyncEnableAPIServer";
pub const ALLOW_HEADING: &str = "Allow Python API Usage?";
pub const ALLOW_TEXT: &str = "Another process is trying to use the Tandem API. The API allows a script to control Tandem and view and modify its contents. Allow the connection?";
pub const NO_COOKIE_SUFFIX: &str =
    "\n\nAlthough you have chosen to allow connections automatically, this script has not presented a valid cookie.";
pub const OK: &str = "OK";
pub const CANCEL: &str = "Cancel";
pub const MORE_INFO: &str = "More Info";
pub const ALWAYS: &str = "Always";
pub const SECURITY_URL: &str = "https://iterm2.com/python-api-security-model";
pub const CONFIRM_HEADING: &str = "Confirm";
pub const CONFIRM_ALWAYS: &str = "New sessions will contain an environment variable that allows scripts to run without confirmation. Are you sure you want to enable this?";

// The reasons the Log keeps.
pub const REASON_USER_LAUNCHED: &str = "Script launched by user action";
pub const REASON_ALL_APPS: &str =
    "All apps are allowed to use the API, per \u{201c}Settings > General > Python API > Allow all apps to connect\u{201d}.";
pub const REASON_NO_COOKIE: &str = "UI authorization disabled and no valid cookie was presented.";
pub const REASON_ALLOWED: &str = "Allowed by user";
pub const REASON_DENIED: &str = "Denied by user";
pub const REASON_INTERNAL: &str = "Internal error";
pub const UNKNOWN_NAME: &str = "Unknown";

// The authentication popup and its warnings.
pub const AUTH_METHOD_LABEL: &str = "Authentication method for Python API";
pub const REQUIRE_COOKIE: &str = "Require a cookie";
pub const ALLOW_ALL_APPS: &str = "Allow all apps to connect";
pub const ALLOW_ALL_HEADING: &str = "Disable per-app authentication?";
pub const ALLOW_ALL_TEXT: &str = "Do you want to allow all apps running on this machine to use the Python API?\n\nThis will disable the check for a cookie. Only programs running as you, at the same elevation, can connect.";
pub const AUTH_URL: &str = "https://iterm2.com/python-api-auth.html";
pub const RESET_HEADING: &str = "Python API Permissions Reset";
pub const RESET_TEXT: &str = "The location of your user profile folder appears to have moved or the allow-all file's contents have changed unexpectedly. As a precaution, the authentication mechanism for Python API scripts for Tandem has been reverted to always require a cookie.";
pub const REVEAL_PREFERENCE: &str = "Reveal Preference";
pub const RESET_ID: &str = "NoSyncAppSupportMoved";
pub const CHANGE_FAILED_HEADING: &str = "Error changing API permissions setting";
pub const SHOW_IN_EXPLORER: &str = "Show in Explorer";
pub const MAKE_CHANGE_FAILED_HEADING: &str = "Failed to make change";

/// The text when the allow-all file could not be deleted (`H:503`).
pub fn remove_failed_text(path: &str, error: &str) -> String {
    format!(
        "Failed to remove the file \u{201c}{path}\u{201d}: {error}\n\nPlease remove this file manually to require a cookie for the Python API.\n\nThe Python API has been disabled for your security."
    )
}

/// The text when the allow-all file could not be written (`H:556`).
pub fn change_failed_text(error: &str) -> String {
    format!("The setting could not be changed: {error}")
}

/// One accepted or refused connection, for the Script Console (11d): its key, display name and
/// reason. Never a cookie.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogEntry {
    pub key: Option<String>,
    pub name: String,
    pub text: String,
}

const LOG_LIMIT: usize = 500;

/// The newest `LOG_LIMIT` entries, oldest first.
#[derive(Clone, Debug, Default)]
pub struct Log {
    entries: VecDeque<LogEntry>,
}

impl Log {
    pub fn push(&mut self, e: LogEntry) {
        if self.entries.len() == LOG_LIMIT {
            self.entries.pop_front();
        }
        self.entries.push_back(e);
    }

    pub fn entries(&self) -> impl Iterator<Item = &LogEntry> {
        self.entries.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_key_is_an_uppercase_uuid() {
        let k = fresh_key();
        assert_eq!(k.len(), 36);
        for (i, c) in k.chars().enumerate() {
            if [8, 13, 18, 23].contains(&i) {
                assert_eq!(c, '-', "{k}");
            } else {
                assert!(c.is_ascii_digit() || ('A'..='F').contains(&c), "{k}");
            }
        }
        assert_eq!(&k[14..15], "4");
        assert_ne!(fresh_key(), k);
    }

    #[test]
    fn jar_follows_iterm2() {
        assert_eq!(cookie_text([0xab; 16]), "ab".repeat(16));
        assert_eq!(cookie_text([0x0f; 16]), "0f".repeat(16));
        let mut jar = Jar::default();
        let a = jar.mint(CookieUse::Single);
        let b = jar.mint(CookieUse::Single);
        assert_eq!(a.len(), 32);
        assert!(a.chars().all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)));
        assert_ne!(a, b);
        assert_eq!(jar.len(), 2);
        assert!(jar.consume(&a));
        assert!(!jar.consume(&a));
        assert_eq!(jar.len(), 1);
        let s = jar.mint_from(CookieUse::Session, [0x11; 16]);
        assert_eq!(s, "11".repeat(16));
        for _ in 0..3 {
            assert!(jar.consume(&s));
        }
        jar.remove(&s);
        assert!(!jar.consume(&s));
        assert!(!jar.consume("00000000000000000000000000000000"));
        assert!(!jar.consume(""));
        assert!(!jar.consume(&b[..31]));
        jar.add("restored", CookieUse::Session);
        assert!(jar.consume("restored"));
        assert!(jar.consume("restored"));
        // Debug never shows a cookie.
        assert!(!format!("{jar:?}").contains(&b));
        let k = fresh_key();
        assert_eq!(k.len(), 36);
        assert_eq!(k, k.to_uppercase());
        assert_eq!(k.matches('-').count(), 4);
    }

    #[test]
    fn allow_all_file_follows_python() {
        let p = noauth_path("C:\\Users\\datbi");
        assert_eq!(p, "C:\\Users\\datbi/Library/Application Support/iTerm2/disable-automation-auth");
        let c = noauth_contents(&p);
        let hex: String = p.bytes().map(|b| format!("{b:02x}")).collect();
        assert_eq!(c, format!("{hex} {NOAUTH_MAGIC}"));
        assert!(c.starts_with("433a5c55736572735c"), "{c}");
        assert_eq!(NOAUTH_MAGIC, "61DF88DC-3423-4823-B725-22570E01C027");
        // A path outside ASCII is hexed as its UTF-8 bytes.
        let u = noauth_path("C:\\Users\\J\u{f6}rg");
        assert!(noauth_contents(&u).contains("c3b6"));
        assert_eq!(noauth_status(&p, None), AllowAll::None);
        assert_eq!(noauth_status(&p, Some(&c)), AllowAll::Valid);
        assert_eq!(noauth_status(&p, Some(&format!("{c}\n"))), AllowAll::Corrupt);
        assert_eq!(noauth_status(&p, Some(&format!("{} {NOAUTH_MAGIC}", hex.to_uppercase()))), AllowAll::Corrupt);
        assert_eq!(noauth_status(&p, Some(&noauth_contents(&u))), AllowAll::Corrupt);
        assert_eq!(noauth_status(&p, Some("")), AllowAll::Corrupt);
    }

    #[test]
    fn authorization_follows_iterm2() {
        for &(a, d, s) in &[(false, false, false), (true, true, true), (false, true, false), (true, false, true)] {
            assert_eq!(decide(true, a, d, s), Verdict::Allow(REASON_USER_LAUNCHED));
        }
        for &(d, s) in &[(false, false), (true, true), (true, false), (false, true)] {
            assert_eq!(decide(false, true, d, s), Verdict::Allow(REASON_ALL_APPS));
        }
        assert_eq!(decide(false, false, true, false), Verdict::Deny(REASON_NO_COOKIE));
        assert_eq!(decide(false, false, true, true), Verdict::Deny(REASON_NO_COOKIE));
        assert_eq!(
            decide(false, false, false, false),
            Verdict::Ask { text: ALLOW_TEXT.to_string(), buttons: vec![OK, CANCEL, MORE_INFO, ALWAYS] }
        );
        assert_eq!(
            decide(false, false, false, true),
            Verdict::Ask { text: format!("{ALLOW_TEXT}{NO_COOKIE_SUFFIX}"), buttons: vec![OK, CANCEL, MORE_INFO] }
        );
        assert_eq!(ask_answer(0), AskAnswer::Allow);
        assert_eq!(ask_answer(1), AskAnswer::Deny);
        assert_eq!(ask_answer(2), AskAnswer::MoreInfo);
        assert_eq!(ask_answer(3), AskAnswer::Always);
        assert_eq!(ask_answer(9), AskAnswer::Deny);
        assert_eq!(display_name(Some("demo.py")), "\u{2248}demo.py");
        assert_eq!(display_name(None), "Unknown");
        assert_eq!(display_name(None), UNKNOWN_NAME);
    }

    #[test]
    fn steps_follow_iterm2() {
        assert_eq!(enable_step(true, false), EnableStep::Confirm);
        assert_eq!(enable_step(false, true), EnableStep::Stop);
        assert_eq!(enable_step(true, true), EnableStep::Nothing);
        assert_eq!(enable_step(false, false), EnableStep::Nothing);
        assert_eq!(allow_all_step(AllowAll::None, true), AllowAllStep::Warn);
        assert_eq!(allow_all_step(AllowAll::Valid, false), AllowAllStep::Remove);
        // iTerm2 reads a corrupt file as "require": choosing require again changes nothing
        // (H:522-526); the brief's Remove is overruled by the source.
        assert_eq!(allow_all_step(AllowAll::Corrupt, false), AllowAllStep::Nothing);
        assert_eq!(allow_all_step(AllowAll::Valid, true), AllowAllStep::Nothing);
        assert_eq!(allow_all_step(AllowAll::None, false), AllowAllStep::Nothing);
        assert_eq!(allow_all_step(AllowAll::Corrupt, true), AllowAllStep::Warn);
    }

    #[test]
    fn texts_follow_iterm2() {
        assert_eq!(ENABLE_HEADING, "Enable Python API?");
        assert_eq!(ENABLE_TEXT, "The Python API allows scripts you run to control Tandem and access all its data.");
        assert_eq!(ENABLE_ID, "NoSyncEnableAPIServer");
        assert_eq!(ALLOW_HEADING, "Allow Python API Usage?");
        assert_eq!(
            ALLOW_TEXT,
            "Another process is trying to use the Tandem API. The API allows a script to control Tandem and view and modify its contents. Allow the connection?"
        );
        assert_eq!(
            NO_COOKIE_SUFFIX,
            "\n\nAlthough you have chosen to allow connections automatically, this script has not presented a valid cookie."
        );
        assert_eq!((OK, CANCEL, MORE_INFO, ALWAYS), ("OK", "Cancel", "More Info", "Always"));
        assert_eq!(SECURITY_URL, "https://iterm2.com/python-api-security-model");
        assert_eq!(CONFIRM_HEADING, "Confirm");
        assert_eq!(
            CONFIRM_ALWAYS,
            "New sessions will contain an environment variable that allows scripts to run without confirmation. Are you sure you want to enable this?"
        );
        assert_eq!(REASON_USER_LAUNCHED, "Script launched by user action");
        assert_eq!(
            REASON_ALL_APPS,
            "All apps are allowed to use the API, per \u{201c}Settings > General > Python API > Allow all apps to connect\u{201d}."
        );
        assert_eq!(REASON_NO_COOKIE, "UI authorization disabled and no valid cookie was presented.");
        assert_eq!(REASON_ALLOWED, "Allowed by user");
        assert_eq!(REASON_DENIED, "Denied by user");
        assert_eq!(REASON_INTERNAL, "Internal error");
        assert_eq!(UNKNOWN_NAME, "Unknown");
        assert_eq!(AUTH_METHOD_LABEL, "Authentication method for Python API");
        assert_eq!(REQUIRE_COOKIE, "Require a cookie");
        assert_eq!(ALLOW_ALL_APPS, "Allow all apps to connect");
        assert_eq!(ALLOW_ALL_HEADING, "Disable per-app authentication?");
        assert_eq!(
            ALLOW_ALL_TEXT,
            "Do you want to allow all apps running on this machine to use the Python API?\n\nThis will disable the check for a cookie. Only programs running as you, at the same elevation, can connect."
        );
        assert_eq!(AUTH_URL, "https://iterm2.com/python-api-auth.html");
        assert_eq!(RESET_HEADING, "Python API Permissions Reset");
        assert_eq!(
            RESET_TEXT,
            "The location of your user profile folder appears to have moved or the allow-all file's contents have changed unexpectedly. As a precaution, the authentication mechanism for Python API scripts for Tandem has been reverted to always require a cookie."
        );
        assert_eq!(REVEAL_PREFERENCE, "Reveal Preference");
        assert_eq!(RESET_ID, "NoSyncAppSupportMoved");
        assert_eq!(CHANGE_FAILED_HEADING, "Error changing API permissions setting");
        assert_eq!(
            remove_failed_text("p", "e"),
            "Failed to remove the file \u{201c}p\u{201d}: e\n\nPlease remove this file manually to require a cookie for the Python API.\n\nThe Python API has been disabled for your security."
        );
        assert_eq!(SHOW_IN_EXPLORER, "Show in Explorer");
        assert_eq!(MAKE_CHANGE_FAILED_HEADING, "Failed to make change");
        assert_eq!(change_failed_text("e"), "The setting could not be changed: e");

        let user = [
            ENABLE_HEADING, ENABLE_TEXT, ENABLE_ID, ALLOW_HEADING, ALLOW_TEXT, NO_COOKIE_SUFFIX, OK, CANCEL,
            MORE_INFO, ALWAYS, CONFIRM_HEADING, CONFIRM_ALWAYS, REASON_USER_LAUNCHED, REASON_ALL_APPS,
            REASON_NO_COOKIE, REASON_ALLOWED, REASON_DENIED, REASON_INTERNAL, UNKNOWN_NAME, AUTH_METHOD_LABEL,
            REQUIRE_COOKIE, ALLOW_ALL_APPS, ALLOW_ALL_HEADING, ALLOW_ALL_TEXT, RESET_HEADING, RESET_TEXT,
            REVEAL_PREFERENCE, RESET_ID, CHANGE_FAILED_HEADING, SHOW_IN_EXPLORER, MAKE_CHANGE_FAILED_HEADING,
        ];
        for t in user.iter().copied().chain([remove_failed_text("p", "e").as_str(), change_failed_text("e").as_str()]) {
            assert!(!t.contains("iTerm2"), "{t}");
        }
        assert!(AUTH_URL.contains("iterm2.com") && SECURITY_URL.contains("iterm2.com"));

        let mut log = Log::default();
        for i in 0..501 {
            log.push(LogEntry { key: None, name: display_name(None), text: format!("{i}") });
        }
        assert_eq!(log.entries().count(), 500);
        assert_eq!(log.entries().next().unwrap().text, "1");
        assert_eq!(log.entries().last().unwrap().text, "500");
    }
}
