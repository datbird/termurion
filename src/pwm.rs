//! The password manager window's model (pure): accounts, display strings, filter and sort,
//! button rules, the edit commit rules, random passwords, the busy counter, and `Secret`, a
//! string that zeroes itself when dropped and never prints. Everything here is plain data and
//! functions so it can be unit tested on Linux; the egui window (`pwm_ui.rs`) and the app wiring
//! (`app/pwm_app.rs`) that drive real accounts, sources and sessions come in later tasks.
//!
//! A password, one-time code, master password or session token is a `Secret` from the moment
//! it is read until it is written out and dropped. Never put one in `Debug`, a log, a toast, a
//! panic message or a command line.

use std::fmt;
use std::sync::atomic::{compiler_fence, Ordering};

/// A string that never prints and zeroes its bytes when dropped. Construct with `new`, read
/// with `expose`, and use `duplicate` where a second owned copy is genuinely needed (there is
/// no `Clone`, so a copy is never accidental).
pub struct Secret(String);

impl Secret {
    pub fn new(s: String) -> Secret {
        Secret(s)
    }

    pub fn expose(&self) -> &str {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn duplicate(&self) -> Secret {
        Secret(self.0.clone())
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Secret(<redacted>)")
    }
}

impl Drop for Secret {
    fn drop(&mut self) {
        // SAFETY: every byte is overwritten with 0 before the buffer is freed; nothing reads
        // self.0 as a str again between here and deallocation, so a moment of invalid UTF-8 in
        // a soon-to-be-freed buffer is fine. write_volatile plus the fence stop the optimizer
        // from treating these stores as dead (the value is about to be dropped) and eliding them.
        unsafe {
            let bytes = self.0.as_bytes_mut();
            for b in bytes.iter_mut() {
                std::ptr::write_volatile(b, 0);
            }
        }
        compiler_fence(Ordering::SeqCst);
    }
}

/// iTerm2's separator between an account name and a user name in a combined display or
/// legacy Keychain label (`\u{2002}\u{2014}\u{2002}`, en space, em dash, en space).
pub const SEP: &str = "\u{2002}\u{2014}\u{2002}";

/// One row in the password manager's table, however the active source represents it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Account {
    pub id: String,
    pub account_name: String,
    pub user_name: String,
    /// The source's cached display string, sorted on (iTerm2 sorts `displayString`, not the
    /// account name alone).
    pub display: String,
    pub has_otp: bool,
    pub send_otp: bool,
    /// A vault/source hint ("Classic", "Nested") for the row and readd-flag logic; `None` when
    /// the source has no such concept.
    pub source_label: Option<String>,
}

/// The account name column's text: the account name, with a non-empty source label appended in
/// parentheses (`iTermPasswordManagerAccountFormatting.displayName(accountName:sourceLabel:)`).
pub fn row_name(a: &Account) -> String {
    match a.source_label.as_deref().map(str::trim) {
        Some(label) if !label.is_empty() => format!("{} ({})", a.account_name, label),
        _ => a.account_name.clone(),
    }
}

/// A case-insensitive substring match against the account name or the user name (each source's
/// own `matches(filter:)` also checks its formatted combined name, which never matches anything
/// the account or user name alone would not).
pub fn matches(a: &Account, filter: &str) -> bool {
    let f = filter.to_lowercase();
    a.account_name.to_lowercase().contains(&f) || a.user_name.to_lowercase().contains(&f)
}

/// iTerm2 sorts the fetched account list by `displayString`, case-insensitively.
pub fn sorted(mut v: Vec<Account>) -> Vec<Account> {
    v.sort_by(|a, b| a.display.to_lowercase().cmp(&b.display.to_lowercase()));
    v
}

/// The password to send: the account's password, with a second factor appended. The typed
/// two-factor field wins when non-empty; otherwise the account's own OTP is used if there is
/// one; the chosen code is trimmed of surrounding whitespace before being appended (iTerm2's
/// `combinedPassword:otp:`, no separator between password and code).
pub fn combined(password: &Secret, two_factor: &str, otp: Option<&Secret>) -> Secret {
    let second = if two_factor.is_empty() {
        otp.map(Secret::expose).unwrap_or(two_factor)
    } else {
        two_factor
    };
    Secret::new(format!("{}{}", password.expose(), second.trim()))
}

/// The full random-password alphabet (iTerm2's default, 91 characters).
pub const CHARS: &str =
    "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789!@#$%^&*()_+{}|:<>?,./;'[]-=";
/// The alphanumeric-only alphabet (held Alt/Option), 62 characters.
pub const ALNUM: &str = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";

/// A random password: 16 characters from `CHARS`, or 18 from `ALNUM` when `alnum_only` (holding
/// Alt asks for letters and digits only, and iTerm2 makes it two characters longer to
/// compensate for the smaller alphabet). `pick(bound)` returns a value in `0..bound`, as
/// `arc4random_uniform` does; callers pass a real RNG, tests a scripted one.
pub fn random_password(alnum_only: bool, pick: &mut dyn FnMut(u32) -> u32) -> Secret {
    let (alphabet, length): (&str, usize) = if alnum_only { (ALNUM, 18) } else { (CHARS, 16) };
    let pool: Vec<char> = alphabet.chars().collect();
    let mut s = String::with_capacity(length);
    for _ in 0..length {
        let i = pick(pool.len() as u32) as usize;
        s.push(pool[i]);
    }
    Secret::new(s)
}

/// What the window's buttons and fields depend on (`buttons`' input).
#[derive(Clone, Copy, Debug, Default)]
pub struct ButtonState {
    /// A row is selected.
    pub selected: bool,
    pub user_name_empty: bool,
    /// The delegate's `iTermPasswordManagerCanEnterPassword`: a live, unexited session.
    pub can_enter: bool,
    pub can_broadcast: bool,
    /// The current source's availability (gates the "+" button).
    pub available: bool,
    /// Opened to send a user name by default (iTerm2's `sendUserByDefault`).
    pub send_user_by_default: bool,
    /// Opened wanting both the user name and the password sent (iTerm2's `didSendUserName`
    /// block being set, not its content).
    pub send_both: bool,
    pub account_sends_otp: bool,
}

/// The button titles: iTerm2's `Enter User Name` / `Enter Username & Password` / `Enter
/// Password`.
pub const TITLE_ENTER_USER_NAME: &str = "Enter User Name";
pub const TITLE_ENTER_USERNAME_AND_PASSWORD: &str = "Enter Username & Password";
pub const TITLE_ENTER_PASSWORD: &str = "Enter Password";
/// The Broadcast checkbox's title and the Enter User Name button's tooltip (iTerm2: Cmd+B and
/// Cmd+U; Tandem's Win is iTerm2's Cmd).
pub const TITLE_BROADCAST: &str = "Broadcast (Win+B)";
pub const TOOLTIP_ENTER_USER_NAME: &str = "Win+U";

/// What the window shows: titles, which buttons are enabled, and which are visible.
#[derive(Debug, PartialEq)]
pub struct Buttons {
    pub default_title: &'static str,
    pub secondary_shown: bool,
    pub default_on: bool,
    pub secondary_on: bool,
    pub edit_on: bool,
    pub add_on: bool,
    pub broadcast_on: bool,
    pub two_factor_on: bool,
    pub remove_on: bool,
}

/// iTerm2's `updateKeyEquivalents` (titles and which button shows) and `updateWithAvailability`
/// (enabling), kept as the two independent checks the source runs: the title depends on
/// `send_user_by_default` only when `send_both` is not also set, but enabling checks `send_both`
/// alone, so a caller that somehow sets both sees the send-both enabling rule under the
/// send-user-by-default title. In practice iTerm2 never opens with both set.
pub fn buttons(s: &ButtonState) -> Buttons {
    let should_enable = s.selected && s.can_enter;

    let (default_title, secondary_shown) = if s.send_user_by_default && !s.send_both {
        (TITLE_ENTER_USER_NAME, false)
    } else if s.send_both {
        (TITLE_ENTER_USERNAME_AND_PASSWORD, true)
    } else {
        (TITLE_ENTER_PASSWORD, true)
    };

    let (default_on, secondary_on) = if s.send_both {
        let enable = should_enable && !s.user_name_empty;
        (enable, enable)
    } else {
        (should_enable, should_enable && !s.user_name_empty)
    };

    Buttons {
        default_title,
        secondary_shown,
        default_on,
        secondary_on,
        edit_on: s.selected,
        add_on: s.available,
        broadcast_on: s.can_broadcast,
        two_factor_on: !s.account_sends_otp,
        remove_on: s.selected,
    }
}

/// Tab jumps from the table, or from the search field with exactly one result, to the 2FA
/// field (iTerm2's `tabShouldSelectTwoFactorField`, minus the window-is-key check the app owns).
pub fn tab_to_two_factor(table_focused: bool, search_focused: bool, rows: usize) -> bool {
    if table_focused {
        return true;
    }
    if search_focused {
        return rows == 1;
    }
    false
}

/// What a double-click on a row does.
#[derive(Debug, PartialEq)]
pub enum Double {
    Edit,
    Default,
}

/// Column 1 (User Name) opens Edit; any other column runs the default button's action
/// (iTerm2's `doubleClickOnTableView:`).
pub fn double_click(column: usize) -> Double {
    if column == 1 {
        Double::Edit
    } else {
        Double::Default
    }
}

/// Why Add refused to add an account (iTerm2's `reallyAdd:` guard order, add path only).
#[derive(Debug, PartialEq)]
pub enum AddRefusal {
    NoName,
    NeedsPassword,
    Duplicate,
    Loading,
}

pub const PLACEHOLDER_ENTER_OR_GENERATE_PASSWORD: &str = "Enter or generate a password";

/// Checks the new-account fields in iTerm2's own order: empty name, then a pending fetch
/// (`_passwordPrefetchInFlight`, checked before the source's own requirement), then a source
/// that requires a password for Add, then a duplicate (name, user) pair.
pub fn add_check(
    name: &str,
    user: &str,
    password: &Secret,
    requires_password: bool,
    loading: bool,
    existing: &[Account],
) -> Result<(), AddRefusal> {
    if name.is_empty() {
        return Err(AddRefusal::NoName);
    }
    if loading {
        return Err(AddRefusal::Loading);
    }
    if requires_password && password.is_empty() {
        return Err(AddRefusal::NeedsPassword);
    }
    if existing.iter().any(|e| e.account_name == name && e.user_name == user) {
        return Err(AddRefusal::Duplicate);
    }
    Ok(())
}

/// The new password to commit, or `None` for "leave it unchanged" (iTerm2's
/// `commitEditWithAccountName:userName:`): an empty typed field never changes the password, and
/// a typed field equal to the baseline (the value the panel prefetched) is treated as untouched.
pub fn edit_password(typed: &Secret, baseline: Option<&Secret>) -> Option<Secret> {
    if typed.is_empty() {
        return None;
    }
    match baseline {
        Some(b) if b.expose() == typed.expose() => None,
        _ => Some(typed.duplicate()),
    }
}

/// What committing the Edit panel does.
#[derive(Debug, PartialEq)]
pub enum Commit {
    /// Nothing changed.
    Nothing,
    /// The name and user name are unchanged; only the password changes, in place.
    SetPassword,
    /// The source can edit in place: rename with only the changed fields sent (an unchanged
    /// field stays `None` so it is never rewritten).
    InPlace { name: Option<String>, user: Option<String> },
    /// The source cannot edit in place: delete then re-add, with the flags that keep a vaulted
    /// record (Classic/Nested) in its vault.
    Readd { flags: Vec<(String, bool)> },
}

/// Plans an Edit-panel commit (iTerm2's `commitEditWithAccountName:userName:` and
/// `renameEntry:toAccountName:userName:newPassword:`). `new_password` is whether
/// `edit_password` returned a value; `plan_commit` itself never needs the password.
pub fn plan_commit(a: &Account, name: &str, user: &str, new_password: bool, in_place: bool) -> Commit {
    let name_changed = name != a.account_name;
    let user_changed = user != a.user_name;
    if name_changed || user_changed {
        if in_place {
            Commit::InPlace {
                name: if name_changed { Some(name.to_string()) } else { None },
                user: if user_changed { Some(user.to_string()) } else { None },
            }
        } else {
            let flags = match a.source_label.as_deref() {
                Some(s) if s.eq_ignore_ascii_case("classic") => vec![("useClassicPermission".to_string(), true)],
                Some(s) if s.eq_ignore_ascii_case("nested") => vec![("useClassicPermission".to_string(), false)],
                _ => Vec::new(),
            };
            Commit::Readd { flags }
        }
    } else if new_password {
        Commit::SetPassword
    } else {
        Commit::Nothing
    }
}

/// Only a delete-then-readd rename can clobber another account's (name, user) key (an in-place
/// source keeps distinct records, so it is never checked): true when the new pair already
/// belongs to a different existing account (iTerm2's `reallyAdd:` rename guard).
pub fn rename_refused(a: &Account, name: &str, user: &str, in_place: bool, existing: &[Account]) -> bool {
    if in_place {
        return false;
    }
    if name == a.account_name && user == a.user_name {
        return false;
    }
    existing.iter().any(|e| e.id != a.id && e.account_name == name && e.user_name == user)
}

/// The window's busy overlay and its cancel token (iTerm2's `_busyCount`/`_cancelCount`):
/// `incr` returns the token an async operation should check later with `current`; `cancel`
/// (the overlay's Cancel button) bumps the token, stale-ing every operation already in flight,
/// and drops the count straight to 0 so the overlay hides at once.
#[derive(Default)]
pub struct Busy {
    pub count: u32,
    pub cancels: u32,
}

impl Busy {
    pub fn incr(&mut self) -> u32 {
        self.count += 1;
        self.cancels
    }

    pub fn decr(&mut self) {
        self.count = self.count.saturating_sub(1);
    }

    pub fn cancel(&mut self) {
        self.cancels += 1;
        self.count = 1;
        self.decr();
    }

    pub fn current(&self, token: u32) -> bool {
        self.cancels == token
    }

    pub fn shown(&self) -> bool {
        self.count > 0
    }
}

pub const BUSY_WORKING: &str = "Working\u{2026}";

pub const TITLE_NEW_ACCOUNT: &str = "New Account";
pub const TITLE_EDIT_ACCOUNT: &str = "Edit Account";
pub const COLUMN_ACCOUNT_NAME: &str = "Account Name";
pub const COLUMN_USER_NAME: &str = "User Name";
pub const COLUMN_PASSWORD: &str = "Password";
pub const LABEL_ACCOUNT: &str = "Account:";
pub const PLACEHOLDER_REQUIRED: &str = "Required";
pub const LABEL_USER_NAME: &str = "User name:";
pub const LABEL_PASSWORD: &str = "Password:";
pub const PLACEHOLDER_AUTOGENERATED: &str = "Autogenerated";
pub const PLACEHOLDER_CANNOT_CHANGE_PASSWORD_HERE: &str = "Cannot change password here";
pub const TOOLTIP_GENERATE_PASSWORD: &str =
    "Generate a random password. Hold Alt to use only alphanumerics.";
pub const TOOLTIP_TWO_FACTOR: &str = "This value is appended to the password.";
pub const CONFIRM_DELETE_PASSWORD: &str = "Are you sure you want to delete this password?";
pub const ALERT_COULD_NOT_SAVE_CHANGES: &str = "Could Not Save Changes";
pub const ALERT_GENERIC_EDIT_ERROR: &str = "The password manager reported an error.";

#[cfg(test)]
mod tests {
    use super::*;

    fn account(id: &str, name: &str, user: &str, source: Option<&str>) -> Account {
        Account {
            id: id.to_string(),
            account_name: name.to_string(),
            user_name: user.to_string(),
            display: format!("{}{}{}", name, SEP, user),
            has_otp: false,
            send_otp: false,
            source_label: source.map(|s| s.to_string()),
        }
    }

    #[test]
    fn secret_never_prints() {
        let s = Secret::new("hunter2".to_string());
        assert_eq!(format!("{:?}", s), "Secret(<redacted>)");

        let combo = combined(&Secret::new("pw".to_string()), " 123 ", None);
        assert_eq!(combo.expose(), "pw123");

        let combo = combined(&Secret::new("pw".to_string()), "", Some(&Secret::new("999".to_string())));
        assert_eq!(combo.expose(), "pw999");

        let mut n = 0u32;
        let mut pick = |bound: u32| -> u32 {
            n = (n + 1) % bound.max(1);
            n
        };
        let p = random_password(false, &mut pick);
        assert_eq!(p.expose().chars().count(), 16);
        assert!(p.expose().chars().all(|c| CHARS.contains(c)));

        let p = random_password(true, &mut pick);
        assert_eq!(p.expose().chars().count(), 18);
        assert!(p.expose().chars().all(|c| ALNUM.contains(c)));
    }

    #[test]
    fn window_rules_follow_iterm2() {
        let none_selected = ButtonState::default();
        let b = buttons(&none_selected);
        assert!(!b.default_on);
        assert!(!b.secondary_on);
        assert!(!b.edit_on);
        assert!(!b.remove_on);

        let base = ButtonState {
            selected: true,
            user_name_empty: false,
            can_enter: true,
            can_broadcast: true,
            available: true,
            send_user_by_default: false,
            send_both: false,
            account_sends_otp: false,
        };
        let b = buttons(&base);
        assert!(b.default_on);
        assert!(b.secondary_on);
        assert!(b.edit_on);
        assert!(b.remove_on);
        assert!(b.broadcast_on);
        assert!(b.two_factor_on);
        assert!(b.add_on);
        assert_eq!(b.default_title, TITLE_ENTER_PASSWORD);
        assert!(b.secondary_shown);

        let empty_user = ButtonState { user_name_empty: true, ..base };
        let b = buttons(&empty_user);
        assert!(b.default_on);
        assert!(!b.secondary_on);

        let by_default = ButtonState { send_user_by_default: true, ..base };
        let b = buttons(&by_default);
        assert_eq!(b.default_title, TITLE_ENTER_USER_NAME);
        assert!(!b.secondary_shown);

        let both = ButtonState { send_both: true, ..base };
        let b = buttons(&both);
        assert_eq!(b.default_title, TITLE_ENTER_USERNAME_AND_PASSWORD);
        assert!(b.secondary_shown);
        assert!(b.default_on);
        assert!(b.secondary_on);
        let both_no_user = ButtonState { send_both: true, user_name_empty: true, ..base };
        let b = buttons(&both_no_user);
        assert!(!b.default_on);
        assert!(!b.secondary_on);

        let no_broadcast = ButtonState { can_broadcast: false, ..base };
        assert!(!buttons(&no_broadcast).broadcast_on);

        let unavailable = ButtonState { available: false, ..base };
        assert!(!buttons(&unavailable).add_on);

        let otp = ButtonState { account_sends_otp: true, ..base };
        assert!(!buttons(&otp).two_factor_on);

        assert!(tab_to_two_factor(true, false, 5));
        assert!(tab_to_two_factor(false, true, 1));
        assert!(!tab_to_two_factor(false, true, 2));
        assert!(!tab_to_two_factor(false, false, 1));

        assert_eq!(double_click(1), Double::Edit);
        assert_eq!(double_click(0), Double::Default);
        assert_eq!(double_click(2), Double::Default);

        let a = account("1", "Example", "bob", None);
        assert!(matches(&a, "exam"));
        assert!(matches(&a, "BOB"));
        assert!(!matches(&a, "nope"));

        let x = account("1", "b account", "u", None);
        let y = account("2", "A account", "u", None);
        let sorted_list = sorted(vec![x.clone(), y.clone()]);
        assert_eq!(sorted_list, vec![y, x]);

        let labeled = account("1", "name", "u", Some("Classic"));
        assert_eq!(row_name(&labeled), "name (Classic)");
        let unlabeled = account("1", "name", "u", None);
        assert_eq!(row_name(&unlabeled), "name");
    }

    #[test]
    fn edit_rules_follow_iterm2() {
        let empty = Secret::new(String::new());
        let a_secret = Secret::new("a".to_string());
        assert!(edit_password(&empty, Some(&a_secret)).is_none());

        let a_secret2 = Secret::new("a".to_string());
        assert!(edit_password(&a_secret, Some(&a_secret2)).is_none());

        let b_secret = Secret::new("b".to_string());
        let a_secret3 = Secret::new("a".to_string());
        let changed = edit_password(&b_secret, Some(&a_secret3));
        assert_eq!(changed.unwrap().expose(), "b");

        let b_secret2 = Secret::new("b".to_string());
        let no_baseline = edit_password(&b_secret2, None);
        assert_eq!(no_baseline.unwrap().expose(), "b");

        let acct = account("1", "name", "user", None);
        assert_eq!(plan_commit(&acct, "name", "user", true, true), Commit::SetPassword);
        assert_eq!(plan_commit(&acct, "name", "user", false, true), Commit::Nothing);
        assert_eq!(
            plan_commit(&acct, "newname", "user", false, true),
            Commit::InPlace { name: Some("newname".to_string()), user: None }
        );
        assert_eq!(
            plan_commit(&acct, "name", "newuser", false, true),
            Commit::InPlace { name: None, user: Some("newuser".to_string()) }
        );

        let classic = account("1", "name", "user", Some("Classic"));
        assert_eq!(
            plan_commit(&classic, "newname", "user", false, false),
            Commit::Readd { flags: vec![("useClassicPermission".to_string(), true)] }
        );
        let nested = account("1", "name", "user", Some("Nested"));
        assert_eq!(
            plan_commit(&nested, "newname", "user", false, false),
            Commit::Readd { flags: vec![("useClassicPermission".to_string(), false)] }
        );
        let plain = account("1", "name", "user", None);
        assert_eq!(plan_commit(&plain, "newname", "user", false, false), Commit::Readd { flags: vec![] });

        let a = account("1", "name", "user", None);
        let other = account("2", "taken", "other", None);
        assert!(rename_refused(&a, "taken", "other", false, &[a.clone(), other.clone()]));
        assert!(!rename_refused(&a, "taken", "other", true, &[a.clone(), other.clone()]));
        assert!(!rename_refused(&a, "name", "user", false, &[a.clone(), other.clone()]));
        assert!(!rename_refused(&a, "fresh", "user", false, &[a.clone(), other.clone()]));

        let existing = [account("1", "existing", "u", None)];
        let pw = Secret::new("x".to_string());
        let empty_pw = Secret::new(String::new());
        assert_eq!(add_check("", "u", &pw, false, false, &existing), Err(AddRefusal::NoName));
        assert_eq!(add_check("new", "u", &pw, false, true, &existing), Err(AddRefusal::Loading));
        assert_eq!(add_check("new", "u", &empty_pw, true, false, &existing), Err(AddRefusal::NeedsPassword));
        assert_eq!(add_check("existing", "u", &pw, false, false, &existing), Err(AddRefusal::Duplicate));
        assert_eq!(add_check("new", "u", &pw, true, false, &existing), Ok(()));

        let mut busy = Busy::default();
        assert!(!busy.shown());
        let token = busy.incr();
        assert!(busy.shown());
        assert!(busy.current(token));
        busy.decr();
        assert!(!busy.shown());

        let token2 = busy.incr();
        busy.cancel();
        assert!(!busy.shown());
        assert!(!busy.current(token2));
    }
}
