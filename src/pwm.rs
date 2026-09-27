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

    /// Appends `s` (a password field's typing). When the buffer must grow, the new one is
    /// allocated first and the old one wiped, so no unzeroed copy is left behind.
    pub fn push_str(&mut self, s: &str) {
        let need = self.0.len() + s.len();
        if need > self.0.capacity() {
            let mut grown = String::with_capacity(need.max(self.0.capacity() * 2).max(64));
            grown.push_str(&self.0);
            drop(Secret(std::mem::replace(&mut self.0, grown)));
        }
        self.0.push_str(s);
    }

    /// Appends typed or pasted text, dropping line breaks (a single-line secure field does).
    pub fn push_line(&mut self, s: &str) {
        for part in s.split(['\r', '\n']) {
            self.push_str(part);
        }
    }

    /// Removes the last character (Backspace), wiping its bytes.
    pub fn pop(&mut self) {
        if let Some(c) = self.0.chars().next_back() {
            let keep = self.0.len() - c.len_utf8();
            self.wipe_from(keep);
        }
    }

    /// Empties the secret, wiping its bytes; the buffer is kept for reuse.
    pub fn clear(&mut self) {
        self.wipe_from(0);
    }

    fn wipe_from(&mut self, at: usize) {
        // SAFETY: the bytes from `at` (a char boundary) are overwritten with 0, which is valid
        // UTF-8, and then truncated away.
        unsafe {
            for b in self.0.as_bytes_mut()[at..].iter_mut() {
                std::ptr::write_volatile(b, 0);
            }
        }
        compiler_fence(Ordering::SeqCst);
        self.0.truncate(at);
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

/// The default button's action (`performDefaultAction:`): Enter User Name when opened to send
/// the user name by default, else Enter Password (which enters the user name first in
/// send-both mode).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DefaultAct {
    EnterUserName,
    EnterPassword,
}

/// What the window shows: titles, which buttons are enabled, and which are visible.
#[derive(Clone, Debug, PartialEq)]
pub struct Buttons {
    pub default_title: &'static str,
    /// What the default button (Return, a double-click) does.
    pub default_act: DefaultAct,
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
        default_act: if default_title == TITLE_ENTER_USER_NAME { DefaultAct::EnterUserName } else { DefaultAct::EnterPassword },
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

/// The selection after the filter changes, over the rows it leaves (indexes into the full
/// list): the one remaining row is selected (`W:2370-2372`); otherwise a selection still shown
/// stays and a hidden one clears (iTerm2's table only ever holds shown rows, so Return can
/// never enter an account the user cannot see).
pub fn select_after_filter(rows: &[usize], selected: Option<usize>) -> Option<usize> {
    if rows.len() == 1 {
        return Some(rows[0]);
    }
    selected.filter(|s| rows.contains(s))
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

pub const TITLE_WINDOW: &str = "Password Manager";
/// The window's content size, which is also its minimum (`WX:36-39`).
pub const WINDOW_SIZE: (f32, f32) = (860.0, 360.0);
/// The password column's cell (`W:2157-2159`).
pub const PASSWORD_CELL: &str = "\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}";
pub const PLACEHOLDER_SEARCH: &str = "Account Name";
pub const PLACEHOLDER_TWO_FACTOR: &str = "2-Factor Code";
pub const TITLE_ADD: &str = "+";
pub const TITLE_REMOVE: &str = "-";
pub const TITLE_EDIT: &str = "Edit";
pub const TITLE_CLOSE: &str = "Close";
pub const TITLE_OK: &str = "OK";
pub const TITLE_CANCEL: &str = "Cancel";
pub const TITLE_COPY: &str = "Copy";
/// The adapter Settings sheet's reveal button (iTerm2's eye button, accessibility "Show").
pub const TITLE_SHOW: &str = "Show";

/// One entry of the gear menu or the row menu.
#[derive(Clone, Debug, PartialEq)]
pub enum Entry<T> {
    Separator,
    Item { title: String, checked: bool, enabled: bool, act: T },
}

fn item<T>(title: &str, checked: bool, enabled: bool, act: T) -> Entry<T> {
    Entry::Item { title: title.to_string(), checked, enabled, act }
}

/// What a gear menu item does.
#[derive(Clone, Debug, PartialEq)]
pub enum GearItem {
    RequireOpen,
    RequireLock,
    RequireEvery,
    Probe,
    SendReturn,
    Source(crate::pwm_source::Kind),
    Reset,
    SwitchAccount,
    /// An adapter's "Settings\u{2026}".
    Settings,
    /// An adapter's custom command, by name.
    Command(String),
    Reload,
}

/// What the gear menu shows: the stored settings as they are (the menu applies iTerm2's
/// validation to them) and what the current source can do.
#[derive(Clone, Debug)]
pub struct Gear {
    /// Every item is disabled while not authenticated (`W:1740-1742`).
    pub authenticated: bool,
    pub require_open: bool,
    pub require_lock: bool,
    pub require_every: bool,
    /// `should_probe`.
    pub probe: bool,
    pub send_return: bool,
    pub source: crate::pwm_source::Kind,
    /// `reset_title`.
    pub reset_title: String,
    /// The source's `canResetConfiguration`.
    pub reset_on: bool,
    /// The source's `supportsMultipleAccounts`.
    pub switch_on: bool,
    /// The source is an adapter with Settings fields.
    pub adapter_settings: bool,
    /// An adapter's custom commands as (name, label).
    pub commands: Vec<(String, String)>,
}

pub const GEAR_REQUIRE_OPEN: &str = "Require Authentication to Open Password Manager";
pub const GEAR_REQUIRE_LOCK: &str = "Require Authentication after Screen Locks";
pub const GEAR_REQUIRE_EVERY: &str = "Require Authentication on Every Open";
pub const GEAR_PROBE: &str = "Probe Before Sending Password";
pub const GEAR_SEND_RETURN: &str = "Automatically Send Return Key after Password";
pub const GEAR_RESET_INTEGRATION: &str = "Reset Integration Configuration";
pub const GEAR_SWITCH_ACCOUNT: &str = "Switch Account";
pub const GEAR_SETTINGS: &str = "Settings\u{2026}";
pub const GEAR_RELOAD: &str = "Reload Items";

/// Probe Before Sending Password is checked only when it is on and `echoProbeDuration` is
/// positive (`shouldProbe`).
pub fn should_probe(on: bool, duration: f64) -> bool {
    on && duration > 0.0
}

/// The Reset item names the source when it can reset (`W:1787-1790`).
pub fn reset_title(can_reset: bool, name: &str) -> String {
    if can_reset {
        format!("Reset {name} Configuration")
    } else {
        GEAR_RESET_INTEGRATION.to_string()
    }
}

/// The gear menu in the xib's order (`WX:243-314`) with iTerm2's `validateMenuItem:`
/// (`W:1739-1813`): nothing while unauthenticated; the screen-lock and every-open items only
/// with authentication required, and the screen-lock item checked and disabled while every-open
/// is on (a kept bug: its stored value is not shown). An adapter's Settings and custom commands
/// go after Switch Account with a separator before them (`W:1670-1729`); the xib's separator
/// before Reload Items stays.
pub fn gear_menu(g: &Gear) -> Vec<Entry<GearItem>> {
    use crate::pwm_source::Kind;
    let mut v = vec![
        item(GEAR_REQUIRE_OPEN, g.require_open, true, GearItem::RequireOpen),
        item(GEAR_REQUIRE_LOCK, g.require_lock || (g.require_open && g.require_every), g.require_open && !g.require_every, GearItem::RequireLock),
        item(GEAR_REQUIRE_EVERY, g.require_every, g.require_open, GearItem::RequireEvery),
        Entry::Separator,
        item(GEAR_PROBE, g.probe, true, GearItem::Probe),
        item(GEAR_SEND_RETURN, g.send_return, true, GearItem::SendReturn),
        Entry::Separator,
    ];
    v.extend(Kind::ALL.into_iter().map(|k| item(k.menu_title(), k == g.source, true, GearItem::Source(k))));
    v.push(Entry::Separator);
    v.push(item(&g.reset_title, false, g.reset_on, GearItem::Reset));
    v.push(item(GEAR_SWITCH_ACCOUNT, false, g.switch_on, GearItem::SwitchAccount));
    if g.adapter_settings || !g.commands.is_empty() {
        v.push(Entry::Separator);
        if g.adapter_settings {
            v.push(item(GEAR_SETTINGS, false, true, GearItem::Settings));
        }
        for (name, label) in &g.commands {
            v.push(item(label, false, true, GearItem::Command(name.clone())));
        }
    }
    v.push(Entry::Separator);
    v.push(item(GEAR_RELOAD, false, true, GearItem::Reload));
    if !g.authenticated {
        for e in &mut v {
            if let Entry::Item { enabled, .. } = e {
                *enabled = false;
            }
        }
    }
    v
}

/// What a row menu item does.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RowItem {
    AppendOtp,
    Reveal,
    Copy,
}

pub const ROW_APPEND_OTP: &str = "Append OTP";
pub const ROW_REVEAL_PASSWORD: &str = "Reveal Password";
pub const ROW_COPY_PASSWORD: &str = "Copy Password";

/// The row's context menu (`WX:378-394`): Append OTP is checked when the account sends its
/// code and enabled only when it has one (`W:1798-1808`).
pub fn row_menu(a: &Account) -> Vec<Entry<RowItem>> {
    vec![
        item(ROW_APPEND_OTP, a.has_otp && a.send_otp, a.has_otp, RowItem::AppendOtp),
        Entry::Separator,
        item(ROW_REVEAL_PASSWORD, false, true, RowItem::Reveal),
        item(ROW_COPY_PASSWORD, false, true, RowItem::Copy),
    ]
}

/// The New Account / Edit Account panel as the app opens it (`W:710-837`, `W:1102-1137`). The
/// window keeps the typed fields; a new `serial` means a new panel and reseeds them from here.
#[derive(Clone, Debug)]
pub struct AccountPanel {
    pub serial: u64,
    pub editing: bool,
    /// The fields' starting text.
    pub name: String,
    pub user: String,
    /// The Edit panel is fetching the current password: a spinner shows in the field's place
    /// and OK is off until `fetched`.
    pub fetching: bool,
    /// The password field (and the dice) can be used at all.
    pub password_on: bool,
    pub placeholder: Option<&'static str>,
    /// The source's first Add Account toggle; never when editing (the vault is fixed).
    pub toggle: Option<crate::pwm_source::Toggle>,
}

pub const PANEL_WIDTH: f32 = 330.0;
/// The panel's content heights without and with the toggle row.
pub const PANEL_HEIGHTS: (f32, f32) = (136.0, 196.0);

impl AccountPanel {
    /// Add: the name starts as the window's default account name; a source that generates its
    /// own passwords shows "Autogenerated" in a disabled field.
    pub fn add(serial: u64, default_name: &str, caps: &crate::pwm_source::Caps) -> AccountPanel {
        AccountPanel {
            serial,
            editing: false,
            name: default_name.to_string(),
            user: String::new(),
            fetching: false,
            password_on: !caps.generated_only,
            placeholder: caps.generated_only.then_some(PLACEHOLDER_AUTOGENERATED),
            toggle: caps.add_toggle.clone(),
        }
    }

    /// Edit: opens at once with the account's names; a source that can change passwords fetches
    /// the current one, one that cannot says so in a disabled field.
    pub fn edit(serial: u64, a: &Account, caps: &crate::pwm_source::Caps) -> AccountPanel {
        AccountPanel {
            serial,
            editing: true,
            name: a.account_name.clone(),
            user: a.user_name.clone(),
            fetching: caps.can_edit_password,
            password_on: caps.can_edit_password,
            placeholder: (!caps.can_edit_password).then_some(PLACEHOLDER_CANNOT_CHANGE_PASSWORD_HERE),
            toggle: None,
        }
    }

    pub fn title(&self) -> &'static str {
        if self.editing {
            TITLE_EDIT_ACCOUNT
        } else {
            TITLE_NEW_ACCOUNT
        }
    }

    pub fn width(&self) -> f32 {
        PANEL_WIDTH
    }

    pub fn height(&self) -> f32 {
        if self.toggle.is_some() {
            PANEL_HEIGHTS.1
        } else {
            PANEL_HEIGHTS.0
        }
    }

    /// OK needs a name and no fetch in flight (`updateNewAccountOkButtonEnabled`).
    pub fn ok_on(&self, typed_name: &str) -> bool {
        !typed_name.is_empty() && !self.fetching
    }

    pub fn dice_on(&self) -> bool {
        self.password_on && !self.fetching
    }

    /// The Edit panel's fetch finished, with or without a password (`endPasswordPrefetchUI`).
    pub fn fetched(&mut self) {
        self.fetching = false;
    }

    /// Add refused an empty password: the field says why (`W:973-980`).
    pub fn needs_password(&mut self) {
        self.placeholder = Some(PLACEHOLDER_ENTER_OR_GENERATE_PASSWORD);
    }
}

pub const HEADING_ARE_YOU_SURE: &str = "Are you sure?";
pub const RESET_WARNING: &str = "This will erase Tandem\u{2019}s configuration settings for this password manager. The actual passwords will remain unaffected. You\u{2019}ll have to go through some setup steps to use it again. This action cannot be undone.";
pub const ALERT_COMMAND_FAILED: &str = "Command Failed";
pub const ALERT_AN_ERROR_OCCURRED: &str = "An error occurred.";

/// An alert shown modally inside the window. `buttons[0]` is the default (Return); a button
/// titled "Cancel" also answers Escape, as an NSAlert's does.
#[derive(Debug)]
pub struct Alert {
    /// Unique per alert (`next_alert_serial`): the window keeps a Settings sheet's typed values
    /// for this serial only, so an alert that replaces another never inherits its text.
    pub serial: u64,
    pub heading: String,
    pub text: String,
    /// Reveal's password (and one-time code on a second line). `Secret`, so `Debug` of an
    /// alert never shows it.
    pub secret: Option<Secret>,
    /// The adapter Settings sheet's rows with their current values.
    pub fields: Vec<(crate::pwm_source::Field, Secret)>,
    pub buttons: Vec<String>,
}

/// A new alert serial, never repeated in a run.
pub fn next_alert_serial() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

impl Alert {
    /// A plain alert with OK (the sources' alerts, "Authentication Error" and the like).
    pub fn message(heading: &str, text: &str) -> Alert {
        Alert { serial: next_alert_serial(), heading: heading.to_string(), text: text.to_string(), secret: None, fields: Vec::new(), buttons: vec![TITLE_OK.to_string()] }
    }

    /// The Remove confirmation (`W:1082-1088`).
    pub fn confirm_delete() -> Alert {
        Alert { buttons: vec![TITLE_OK.to_string(), TITLE_CANCEL.to_string()], ..Alert::message(CONFIRM_DELETE_PASSWORD, "") }
    }

    /// Reveal Password (`W:1582-1621`): OK and Copy.
    pub fn reveal(account: &str, password: &Secret, otp: Option<&Secret>) -> Alert {
        let shown = match otp {
            Some(o) => {
                let mut s = password.duplicate();
                s.push_str("\n");
                s.push_str(o.expose());
                s
            }
            None => password.duplicate(),
        };
        Alert { secret: Some(shown), buttons: vec![TITLE_OK.to_string(), TITLE_COPY.to_string()], ..Alert::message(&format!("Password for {account}"), "") }
    }

    /// A failed edit (`W:1015-1025`); an empty error reads as the generic one.
    pub fn save_failed(error: &str) -> Alert {
        Alert::message(ALERT_COULD_NOT_SAVE_CHANGES, if error.is_empty() { ALERT_GENERIC_EDIT_ERROR } else { error })
    }

    /// Reset configuration (`W:626-638`).
    pub fn reset() -> Alert {
        Alert { buttons: vec![TITLE_OK.to_string(), TITLE_CANCEL.to_string()], ..Alert::message(HEADING_ARE_YOU_SURE, RESET_WARNING) }
    }

    /// An adapter's Settings sheet (`W:1347-1492`).
    pub fn settings(name: &str, fields: Vec<(crate::pwm_source::Field, Secret)>) -> Alert {
        Alert { fields, buttons: vec![TITLE_OK.to_string(), TITLE_CANCEL.to_string()], ..Alert::message(&format!("{name} Settings"), "") }
    }

    /// A custom command that failed (`W:1509-1516`): the error (or the generic text), then the
    /// command's message after a blank line.
    pub fn command_failed(error: &str, message: &str) -> Alert {
        let mut text = if error.is_empty() { ALERT_AN_ERROR_OCCURRED.to_string() } else { error.to_string() };
        if !message.is_empty() {
            text.push_str("\n\n");
            text.push_str(message);
        }
        Alert::message(ALERT_COMMAND_FAILED, &text)
    }

    /// A custom command that succeeded with a message: headed by the command's name.
    pub fn command_result(name: &str, message: &str) -> Alert {
        Alert::message(name, message)
    }
}

/// The gear menu's settings, as iTerm2 stores them (menu-only globals: no settings rows). The
/// first two are iTerm2's "secure" defaults; on Windows they are ordinary globals (m10b ruling).
pub const REQUIRE_OPEN_KEY: &str = "RequireAuthenticationToOpenPasswordManager";
pub const REQUIRE_LOCK_KEY: &str = "RequireAuthenticationAfterScreenLocks";
pub const REQUIRE_EVERY_KEY: &str = "RequireAuthenticationOnEveryPasswordManagerOpen";
pub const PROBE_KEY: &str = "ProbeForPassword";
pub const SEND_RETURN_KEY: &str = "ShouldSendReturnAfterPassword";

/// A gear setting's value: the stored one, else iTerm2's default (`SecureUserDefaults.swift:86-87`,
/// `iTermUserDefaults.m:110,210,366`): authentication to open and the probe on, Return after the
/// password on (a registered default), the other two off.
pub fn setting(stored: &dyn Fn(&str) -> Option<bool>, key: &str) -> bool {
    stored(key).unwrap_or(matches!(key, REQUIRE_OPEN_KEY | PROBE_KEY | SEND_RETURN_KEY))
}

/// The password manager's authentication for this run, shared by every window
/// (`PasswordManagerDataSourceProvider.authenticated`).
#[derive(Debug, Default)]
pub struct Auth {
    pub authenticated: bool,
}

impl Auth {
    /// `mayReuseAuthentication(authRequired:requireEveryOpen:)`: a cached authentication is
    /// reused unless every open must prompt, which only counts while authentication is required.
    pub fn may_reuse(required: bool, every_open: bool) -> bool {
        !required || !every_open
    }

    /// Whether opening shows the prompt (`requestAuthenticationIfNeeded`): not when the cached
    /// authentication may be reused, and never when authentication is not required (the caller
    /// then calls `open_unprompted`).
    pub fn needs_prompt(&self, required: bool, every_open: bool) -> bool {
        if self.authenticated && Auth::may_reuse(required, every_open) {
            return false;
        }
        required
    }

    /// An open that needed no prompt: authenticated from here on.
    pub fn open_unprompted(&mut self) {
        self.authenticated = true;
    }

    /// The prompt's answer: success authenticates, failure revokes.
    pub fn granted(&mut self, ok: bool) {
        self.authenticated = ok;
    }

    /// The screen locked (`staticScreenDidLock:`): revoked when `after_lock`
    /// (RequireAuthenticationAfterScreenLocks) is on.
    pub fn screen_locked(&mut self, after_lock: bool) {
        if after_lock {
            self.authenticated = false;
        }
    }
}

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
    fn window_texts_follow_iterm2() {
        use crate::pwm_source::{Caps, Field, Kind, Toggle};

        assert_eq!((COLUMN_ACCOUNT_NAME, COLUMN_USER_NAME, COLUMN_PASSWORD), ("Account Name", "User Name", "Password"));
        assert_eq!(PASSWORD_CELL, "\u{2022}".repeat(8));
        assert_eq!(PLACEHOLDER_SEARCH, "Account Name");
        assert_eq!(PLACEHOLDER_TWO_FACTOR, "2-Factor Code");
        assert_eq!(TITLE_WINDOW, "Password Manager");
        assert_eq!(WINDOW_SIZE, (860.0, 360.0));
        assert_eq!(BUSY_WORKING, "Working\u{2026}");

        let titles = |v: &[Entry<GearItem>]| -> Vec<String> {
            v.iter()
                .map(|e| match e {
                    Entry::Separator => "-".to_string(),
                    Entry::Item { title, .. } => title.clone(),
                })
                .collect()
        };
        let gear = Gear {
            authenticated: true,
            require_open: true,
            require_lock: false,
            require_every: false,
            probe: should_probe(true, 0.5),
            send_return: false,
            source: Kind::CredentialManager,
            reset_title: reset_title(false, "Windows Credential Manager"),
            reset_on: false,
            switch_on: false,
            adapter_settings: false,
            commands: Vec::new(),
        };
        let menu = gear_menu(&gear);
        assert_eq!(
            titles(&menu),
            vec![
                "Require Authentication to Open Password Manager",
                "Require Authentication after Screen Locks",
                "Require Authentication on Every Open",
                "-",
                "Probe Before Sending Password",
                "Automatically Send Return Key after Password",
                "-",
                "Windows Credential Manager",
                "1Password Integration",
                "LastPass Integration",
                "KeePassXC Integration",
                "Bitwarden Integration",
                "Keeper Security",
                "-",
                "Reset Integration Configuration",
                "Switch Account",
                "-",
                "Reload Items",
            ]
        );
        let item = |v: &[Entry<GearItem>], want: GearItem| -> (bool, bool) {
            v.iter()
                .find_map(|e| match e {
                    Entry::Item { checked, enabled, act, .. } if *act == want => Some((*checked, *enabled)),
                    _ => None,
                })
                .expect("the item is in the menu")
        };
        assert_eq!(item(&menu, GearItem::RequireOpen), (true, true));
        assert_eq!(item(&menu, GearItem::RequireLock), (false, true));
        assert_eq!(item(&menu, GearItem::Probe), (true, true));
        assert_eq!(item(&menu, GearItem::Source(Kind::CredentialManager)), (true, true));
        assert_eq!(item(&menu, GearItem::Source(Kind::OnePassword)), (false, true));
        assert_eq!(item(&menu, GearItem::Reset), (false, false));
        assert_eq!(item(&menu, GearItem::SwitchAccount), (false, false));
        assert!(!should_probe(true, 0.0) && !should_probe(false, 0.5));

        // The kept bug: after-screen-locks shows checked and disabled while every-open is on.
        let every = Gear { require_every: true, ..gear.clone() };
        assert_eq!(item(&gear_menu(&every), GearItem::RequireLock), (true, false));
        assert_eq!(item(&gear_menu(&every), GearItem::RequireEvery), (true, true));
        let open_off = Gear { require_open: false, ..gear.clone() };
        assert_eq!(item(&gear_menu(&open_off), GearItem::RequireLock).1, false);
        assert_eq!(item(&gear_menu(&open_off), GearItem::RequireEvery).1, false);
        let locked = Gear { authenticated: false, ..gear.clone() };
        assert!(gear_menu(&locked).iter().all(|e| !matches!(e, Entry::Item { enabled: true, .. })));

        // An adapter's Settings and commands go after Switch Account, a separator before them.
        let adapter = Gear {
            source: Kind::KeePassXC,
            reset_title: reset_title(true, "KeePassXC"),
            reset_on: true,
            switch_on: true,
            adapter_settings: true,
            commands: vec![("sync".to_string(), "Sync Now".to_string())],
            ..gear.clone()
        };
        let menu = gear_menu(&adapter);
        assert_eq!(
            titles(&menu)[14..].to_vec(),
            vec!["Reset KeePassXC Configuration", "Switch Account", "-", "Settings\u{2026}", "Sync Now", "-", "Reload Items"]
        );
        assert_eq!(item(&menu, GearItem::Command("sync".to_string())), (false, true));
        assert_eq!(item(&menu, GearItem::Reset), (false, true));

        let mut a = account("1", "name", "user", None);
        let row = |a: &Account| -> Vec<(String, bool, bool)> {
            row_menu(a)
                .into_iter()
                .map(|e| match e {
                    Entry::Separator => ("-".to_string(), false, false),
                    Entry::Item { title, checked, enabled, .. } => (title, checked, enabled),
                })
                .collect()
        };
        assert_eq!(
            row(&a),
            vec![
                ("Append OTP".to_string(), false, false),
                ("-".to_string(), false, false),
                ("Reveal Password".to_string(), false, true),
                ("Copy Password".to_string(), false, true),
            ]
        );
        a.has_otp = true;
        a.send_otp = true;
        assert_eq!(row(&a)[0], ("Append OTP".to_string(), true, true));

        assert_eq!((LABEL_ACCOUNT, LABEL_USER_NAME, LABEL_PASSWORD, PLACEHOLDER_REQUIRED), ("Account:", "User name:", "Password:", "Required"));
        assert_eq!(TOOLTIP_GENERATE_PASSWORD, "Generate a random password. Hold Alt to use only alphanumerics.");
        assert_eq!(TOOLTIP_TWO_FACTOR, "This value is appended to the password.");
        let toggle = Toggle { key: "classic".to_string(), label: "Classic".to_string(), note: Some("A note".to_string()), default_value: true };
        let caps = Caps { add_toggle: Some(toggle.clone()), can_edit_password: true, ..Caps::default() };
        let add = AccountPanel::add(1, "github", &caps);
        assert_eq!(add.title(), "New Account");
        assert_eq!((add.name.as_str(), add.user.as_str()), ("github", ""));
        assert_eq!(add.toggle, Some(toggle));
        assert_eq!(add.width(), 330.0);
        assert_eq!(add.height(), 196.0);
        assert!(add.password_on && add.dice_on() && !add.fetching && add.placeholder.is_none());
        assert!(add.ok_on("x") && !add.ok_on(""));
        let generated = AccountPanel::add(2, "", &Caps { generated_only: true, ..Caps::default() });
        assert_eq!(generated.placeholder, Some("Autogenerated"));
        assert!(!generated.password_on && !generated.dice_on());
        assert_eq!(generated.height(), 136.0);

        let mut edit = AccountPanel::edit(3, &account("1", "name", "user", None), &caps);
        assert_eq!(edit.title(), "Edit Account");
        assert_eq!((edit.name.as_str(), edit.user.as_str()), ("name", "user"));
        assert!(edit.toggle.is_none(), "no toggle when editing");
        assert!(edit.fetching && !edit.ok_on("name") && !edit.dice_on());
        edit.fetched();
        assert!(edit.ok_on("name") && edit.dice_on());
        let fixed = AccountPanel::edit(4, &account("1", "name", "user", None), &Caps::default());
        assert_eq!(fixed.placeholder, Some("Cannot change password here"));
        assert!(!fixed.fetching && !fixed.password_on && fixed.ok_on("name"));
        let mut needs = AccountPanel::add(5, "", &Caps { requires_password: true, ..Caps::default() });
        needs.needs_password();
        assert_eq!(needs.placeholder, Some("Enter or generate a password"));

        let del = Alert::confirm_delete();
        assert_eq!((del.heading.as_str(), del.buttons.clone()), ("Are you sure you want to delete this password?", vec!["OK".to_string(), "Cancel".to_string()]));
        let pw = Secret::new("hunter2".to_string());
        let otp = Secret::new("123456".to_string());
        let reveal = Alert::reveal("github", &pw, Some(&otp));
        assert_eq!(reveal.heading, "Password for github");
        assert_eq!(reveal.secret.as_ref().map(Secret::expose), Some("hunter2\n123456"));
        assert_eq!(reveal.buttons, vec!["OK", "Copy"]);
        assert!(!format!("{:?}", reveal).contains("hunter2"), "an alert never prints its secret");
        assert_eq!(Alert::reveal("x", &pw, None).secret.as_ref().map(Secret::expose), Some("hunter2"));
        let failed = Alert::save_failed("");
        assert_eq!((failed.heading.as_str(), failed.text.as_str()), ("Could Not Save Changes", "The password manager reported an error."));
        assert_eq!(Alert::save_failed("Denied.").text, "Denied.");
        let reset = Alert::reset();
        assert_eq!(reset.heading, "Are you sure?");
        assert_eq!(reset.text, "This will erase Tandem\u{2019}s configuration settings for this password manager. The actual passwords will remain unaffected. You\u{2019}ll have to go through some setup steps to use it again. This action cannot be undone.");
        assert_eq!(reset.buttons, vec!["OK", "Cancel"]);
        let field = Field { key: "master".to_string(), label: "Password".to_string(), placeholder: None, is_secret: true, note: None, persist_in_keychain: false };
        let settings = Alert::settings("KeePassXC", vec![(field, Secret::new("s3cret".to_string()))]);
        assert_eq!(settings.heading, "KeePassXC Settings");
        assert_eq!(settings.buttons, vec!["OK", "Cancel"]);
        assert!(!format!("{:?}", settings).contains("s3cret"));
        let cmd = Alert::command_failed("", "details");
        assert_eq!((cmd.heading.as_str(), cmd.text.as_str()), ("Command Failed", "An error occurred.\n\ndetails"));
        assert_eq!(Alert::command_failed("Boom.", "").text, "Boom.");
        let done = Alert::command_result("sync", "Synced 3 items.");
        assert_eq!((done.heading.as_str(), done.text.as_str(), done.buttons.clone()), ("sync", "Synced 3 items.", vec!["OK".to_string()]));

        // A password field edits its Secret in place and wipes what it drops.
        let mut s = Secret::new(String::new());
        for c in "p\u{e9}ss".chars() {
            s.push_str(&c.to_string());
        }
        s.push_str(&"x".repeat(100));
        assert_eq!(s.expose(), format!("p\u{e9}ss{}", "x".repeat(100)));
        s.clear();
        s.push_str("ab\u{e9}");
        s.pop();
        assert_eq!(s.expose(), "ab");
        s.clear();
        assert!(s.is_empty());
        s.pop();
        assert!(s.is_empty());
        s.push_line("a\r\nb\nc");
        assert_eq!(s.expose(), "abc", "a pasted line break is dropped");
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

    #[test]
    fn filter_and_default_follow_iterm2() {
        // One remaining row is selected; a hidden selection clears; a visible one stays.
        assert_eq!(select_after_filter(&[4], None), Some(4));
        assert_eq!(select_after_filter(&[4], Some(2)), Some(4));
        assert_eq!(select_after_filter(&[1, 2, 3], Some(2)), Some(2));
        assert_eq!(select_after_filter(&[1, 3], Some(2)), None);
        assert_eq!(select_after_filter(&[], Some(2)), None);
        assert_eq!(select_after_filter(&[1, 3], None), None);

        let base = ButtonState { selected: true, can_enter: true, ..ButtonState::default() };
        assert_eq!(buttons(&base).default_act, DefaultAct::EnterPassword);
        assert_eq!(buttons(&ButtonState { send_both: true, ..base }).default_act, DefaultAct::EnterPassword);
        assert_eq!(buttons(&ButtonState { send_user_by_default: true, ..base }).default_act, DefaultAct::EnterUserName);

        // Every alert is its own: a replacement never shares a serial with what it replaces.
        let a = Alert::message("a", "");
        let b = Alert::message("a", "");
        let c = Alert { buttons: vec![TITLE_OK.to_string()], ..Alert::settings("x", Vec::new()) };
        assert!(a.serial != b.serial && b.serial != c.serial && a.serial != c.serial);
    }

    #[test]
    fn auth_follows_iterm2() {
        assert!(Auth::may_reuse(false, false) && Auth::may_reuse(false, true));
        assert!(Auth::may_reuse(true, false));
        assert!(!Auth::may_reuse(true, true));

        let mut a = Auth::default();
        assert!(a.needs_prompt(true, false), "a first open with authentication required prompts");
        a.granted(true);
        assert!(a.authenticated);
        assert!(!a.needs_prompt(true, false), "reused");
        assert!(a.needs_prompt(true, true), "every open prompts again");
        a.granted(false);
        assert!(!a.authenticated, "a failed prompt revokes");

        let mut a = Auth::default();
        assert!(!a.needs_prompt(false, true), "not required never prompts");
        a.open_unprompted();
        assert!(a.authenticated, "and becomes authenticated");

        a.screen_locked(false);
        assert!(a.authenticated, "a lock without after-screen-locks keeps it");
        a.screen_locked(true);
        assert!(!a.authenticated, "a lock with after-screen-locks revokes");

        let g = |_: &str| None;
        assert!(setting(&g, REQUIRE_OPEN_KEY) && setting(&g, PROBE_KEY) && setting(&g, SEND_RETURN_KEY));
        assert!(!setting(&g, REQUIRE_LOCK_KEY) && !setting(&g, REQUIRE_EVERY_KEY));
        let stored = |k: &str| (k == REQUIRE_OPEN_KEY).then_some(false);
        assert!(!setting(&stored, REQUIRE_OPEN_KEY));
        assert_eq!(REQUIRE_OPEN_KEY, "RequireAuthenticationToOpenPasswordManager");
        assert_eq!(REQUIRE_LOCK_KEY, "RequireAuthenticationAfterScreenLocks");
        assert_eq!(REQUIRE_EVERY_KEY, "RequireAuthenticationOnEveryPasswordManagerOpen");
        assert_eq!(PROBE_KEY, "ProbeForPassword");
        assert_eq!(SEND_RETURN_KEY, "ShouldSendReturnAfterPassword");
    }
}
