//! The password manager in the app: opening it over a terminal window, the authentication gate,
//! the source worker thread, what the window's acts do, and entering text into sessions with
//! the echo probe (`PseudoTerminal.m` openPasswordManagerToAccountName:, the
//! `iTermPasswordManagerWindowController.m` actions, `PTYSession.m` enterPassword:,
//! `iTermBroadcastPasswordHelper.m`).
//!
//! One manager per terminal window (`pwm_window`, swapped with the window); the authentication,
//! the source and its worker thread are app-wide, as iTerm2's data source provider is. Every
//! source call runs on the worker thread; a reply names the window and the busy token it was
//! asked with, and a reply to a canceled request is dropped (`ifCancelCountUnchanged:`).
//!
//! Secrets: a fetched password stays a `Secret` from the worker's reply until it is written to
//! the pty (`user_write` with `Write::SendTextLocal`, never broadcast), put on the clipboard (Copy
//! Password, Reveal's Copy) or dropped. Nothing here logs, prints or toasts one.

use std::collections::{HashMap, VecDeque};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};

use super::*;
use crate::broadcast::Write;
use crate::browser_basic_auth::SpaceKey;
use crate::browser_password::{self as bp, PwPage};
use crate::echo_probe::{self, Finish, Out, Probe, Round};
use crate::prompt_ui::{ConfirmResult, ConfirmWindow};
use crate::pwm::{self, Account, AccountPanel, Alert, Auth, Busy, GearItem, Secret};
use crate::pwm_cli::{self, OpNotUsable};
use crate::pwm_prompt;
use crate::pwm_source::{self, Caps, Field, Kind, Scope, Source};
use crate::pwm_ui::{PwmAct, PwmView, PwmWindow};
use crate::warnings::Preempt;

/// Where the CLIs picked with Locate are remembered (menu-only globals, no settings rows; iTerm2
/// keeps the picked path for the run only).
const OP_PATH_KEY: &str = "NoSyncPasswordManagerOnePasswordCLI";
const LP_PATH_KEY: &str = "NoSyncPasswordManagerLastPassCLI";
/// `iTermAdvancedSettingsModel.onePasswordAccount`, which the account picker sets.
const OP_ACCOUNT_KEY: &str = "onePasswordAccount";
/// `iTermAdvancedSettingsModel.lastpassGroups`.
const LP_GROUPS_KEY: &str = "lastpassGroups";
/// The LastPass help page the Can't Find alert's Help opens (`LP:579`).
const LP_HELP_URL: &str = "https://iterm2.com/lastpass-cli";
/// The heading of an adapter's errors (`AP:999`) and of its list warning (`AP:697`).
const ADAPTER_ERROR_HEADING: &str = "Password Manager Error";
const ADAPTER_WARNING_HEADING: &str = "Password Manager";
/// iTerm2's authentication failure alert (`P:361-362`).
const AUTH_FAILED_TITLE: &str = "Authentication Failed";
/// The fields of a master password or login prompt.
const PROMPT_USER: &str = "user";
const PROMPT_PASSWORD: &str = "password";

impl PwmWin {
    /// Follows a forced appearance (`tab_theme::forced_dark`), or Windows' when `None`.
    pub(super) fn follow_theme(&self, forced: Option<bool>) {
        self.window.follow_theme(forced);
    }
}

/// A password manager window and what it shows.
pub(super) struct PwmWin {
    window: PwmWindow,
    /// This manager's instance, unique for the run: the worker's replies carry it, so a closed
    /// manager's replies never act in one reopened in its place.
    instance: u64,
    /// The session whose focus reports are held while it is open (iTerm2's effective session).
    hold: Option<PaneId>,
    /// Every account, sorted by display string.
    accounts: Vec<Account>,
    filter: String,
    selected: Option<usize>,
    /// The source's availability: "+" follows it.
    available: bool,
    busy: Busy,
    panel: Option<AccountPanel>,
    /// The account the Edit panel edits, and the password it prefetched.
    editing: Option<Account>,
    baseline: Option<Secret>,
    alerts: VecDeque<(Alert, AlertFor)>,
    two_factor: String,
    broadcast: bool,
    /// The account name it opened to (`defaultAccountName`): Add's starting name.
    default_name: String,
    /// Selected once the next list arrives: by display string, or by (account, user).
    select_name: Option<String>,
    select_pair: Option<(String, String)>,
    /// Which store this manager serves: the terminal's or the browser's own (iTerm2's two providers).
    scope: Scope,
    /// The browser pane it was opened over; the page every entry is bound to (Session target).
    page_pane: Option<PaneId>,
    /// Opened to send the user name by default (`sendUserByDefault`), and to send both
    /// (`didSendUserName` set).
    send_user_by_default: bool,
    send_both: bool,
    target: PwmTarget,
    /// Enter Username & Password's focus step: the password field's id and the navigation count it
    /// was asked at.
    focus_after_user: Option<(String, u64)>,
    /// A basic-auth picker's user name, which arrives before the password (`pendingUserName`).
    pending_user: Option<String>,
    /// The challenge origin a picker lists for and picks within.
    pick_page: Option<PwPage>,
}

impl PwmWin {
    pub(super) fn id(&self) -> WindowId {
        self.window.id()
    }

    pub(super) fn hwnd(&self) -> Option<isize> {
        self.window.hwnd()
    }

    fn selected_account(&self) -> Option<&Account> {
        self.selected.and_then(|i| self.accounts.get(i))
    }

    fn push_alert(&mut self, a: Alert, f: AlertFor) {
        self.alerts.push_back((a, f));
        self.window.request_redraw();
    }
}

impl Drop for PwmWin {
    /// However it goes (closed, or with its terminal window), the key hook forgets it.
    fn drop(&mut self) {
        if let Some(h) = self.window.hwnd() {
            crate::winkeys::remove_pwm_window(h);
        }
        // A picker closed any way at all (or with its window) without a choice ends its challenge once.
        if let Some(n) = take_challenge(&mut self.target) {
            ORPHANED.lock().unwrap().push(n);
        }
    }
}

/// What answering an alert does.
enum AlertFor {
    Plain,
    Delete(Account),
    Reveal,
    Reset,
    Settings,
    /// A master password or login prompt from the worker.
    Prompt(Sender<Option<Vec<(String, Secret)>>>),
    /// The 1Password account picker: the account ids, in button order.
    Pick(Vec<String>, Sender<Option<String>>),
    /// "OnePassword Unavailable" for a CLI too old; Can't Find follows. True while choosing
    /// the source.
    OpTooOld(bool),
    CannotFind(Kind, bool),
    LpAuthFailed(String),
}

/// The password manager's app-wide state.
#[derive(Default)]
pub(super) struct PwmState {
    auth: Auth,
    worker: Option<Worker>,
    /// The source as the worker last reported it: kind, name, capabilities.
    info: Option<(Kind, String, Caps)>,
    /// Passwords waiting for a session's echo probe (`iTermEchoProbe`'s `_password`).
    /// Keyed by session, with the entry's own number so a question answers only its entry.
    entries: HashMap<PaneId, (u64, Secret)>,
    /// The last entry number handed out.
    entry_ids: u64,
    rounds: Vec<BroadcastRound>,
    confirms: Vec<Confirm>,
    /// The browser manager's own authentication, worker and source info (iTerm2 keeps `authenticated`
    /// and the data source per provider, `PDS:15`): unlocking one never unlocks the other.
    browser_auth: Auth,
    browser_worker: Option<Worker>,
    browser_info: Option<(Kind, String, Caps)>,
    /// Basic-auth store answers waiting for the coordinator's owner (Task 10).
    store_replies: Vec<StoreReply>,
    /// Opens asked for, done on the next pass (a window needs the event loop).
    requests: Vec<PwmOpen>,
    /// The window whose authentication gate failed on the last pass: opens for it queued
    /// meanwhile (events held during the modal prompt) are dropped, so one cancel never pops
    /// a second prompt at once.
    refused: Option<WindowId>,
    /// Sessions whose focus reports stay held until their window next gains focus.
    holds: Vec<PaneId>,
    serial: u64,
    /// The last manager instance handed out.
    instances: u64,
    /// An adapter's secret settings, for the run only.
    secret_settings: HashMap<String, Secret>,
    /// LastPass's Open Terminal Window: the email to log in, then the session watched for
    /// "Success: Logged in as".
    lp_terminal: Option<String>,
    lp_watch: Option<PaneId>,
}

/// How a manager is asked for (`openPasswordManagerToAccountName:inSession:forUser:didSendUserName:`).
#[derive(Clone, Debug, Default)]
pub(super) struct PwmOpen {
    pub(super) account: Option<String>,
    pub(super) pane: Option<PaneId>,
    pub(super) scope: Scope,
    /// `forUser`: the default button sends the user name.
    pub(super) for_user: bool,
    /// `didSendUserName` set: the default button sends the user name and the password.
    pub(super) send_both: bool,
    pub(super) target: PwmTarget,
    /// The password field's id and the page's navigation count, for send-both's focus step.
    pub(super) focus_after_user: Option<(String, u64)>,
    /// A web page's button asked for it: the window carries the one input guard (`PwmWindow::guard_input`).
    pub(super) from_page: bool,
    /// A basic-auth picker's challenge origin (https): the list shows only its accounts and a pick
    /// must match it (`account_matches`). None with a picker target lists nothing.
    pub(super) pick_page: Option<PwPage>,
}

/// What the manager's entries go to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum PwmTarget {
    /// The session (a terminal's pty, or a browser page).
    #[default]
    Session,
    /// A basic-auth challenge's picker: the entries finish this challenge (`BAP`).
    BasicAuth(u64),
}

/// A basic-auth picker's outcome, the way Task 10's coordinator takes it.
#[allow(dead_code)]
pub(super) struct PickerResult {
    pub(super) chosen: Option<(String, Secret)>,
    /// With nothing chosen the prompt comes back with what was typed.
    pub(super) typed_fallback: bool,
}

/// Challenges of pickers that closed without a choice, ended on the next pass.
static ORPHANED: Mutex<Vec<u64>> = Mutex::new(Vec::new());

/// The challenge a picker still owes an answer, once: taking it leaves the target a plain session.
fn take_challenge(target: &mut PwmTarget) -> Option<u64> {
    match std::mem::take(target) {
        PwmTarget::BasicAuth(n) => Some(n),
        PwmTarget::Session => None,
    }
}

/// The request `open_password_manager` queues: the browser's store when the pane (or the focused pane
/// when none is named) is a browser pane, bound to that pane (`PT:10319-10323`).
fn open_request(account: Option<String>, pane: Option<PaneId>, focused: Option<PaneId>, browser: bool) -> PwmOpen {
    if browser {
        PwmOpen { account, pane: pane.or(focused), scope: Scope::Browser, ..PwmOpen::default() }
    } else {
        PwmOpen { account, pane, ..PwmOpen::default() }
    }
}

/// What opening does with the window's one `pwm_window` slot. iTerm2's refuse-if-open rule is the
/// session path's; a picker gets its own window in iTerm2 (`BAP:32-38`), so here a taken slot ends the
/// challenge at once with no choice (the open manager is left as it is).
#[derive(Debug, PartialEq, Eq)]
enum OpenDecision {
    Open,
    Refuse,
    AnswerNone,
}

fn open_decision(target: PwmTarget, slot_taken: bool) -> OpenDecision {
    match (target, slot_taken) {
        (_, false) => OpenDecision::Open,
        (PwmTarget::Session, true) => OpenDecision::Refuse,
        (PwmTarget::BasicAuth(_), true) => OpenDecision::AnswerNone,
    }
}

/// What a manager window's buttons depend on, before `pwm::buttons`.
struct ButtonInputs {
    target: PwmTarget,
    for_user: bool,
    send_both: bool,
    selected: bool,
    user_name_empty: bool,
    /// The session (or page) can take an entry for the selection.
    session_can_enter: bool,
    can_broadcast: bool,
    available: bool,
    sends_otp: bool,
}

/// A picker can always enter and never broadcast (`BAP:54-58`).
fn button_state(i: &ButtonInputs) -> pwm::ButtonState {
    let picker = matches!(i.target, PwmTarget::BasicAuth(_));
    pwm::ButtonState {
        selected: i.selected,
        user_name_empty: i.user_name_empty,
        can_enter: picker || i.session_can_enter,
        can_broadcast: !picker && i.can_broadcast,
        available: i.available,
        send_user_by_default: i.for_user,
        send_both: i.send_both,
        account_sends_otp: i.sends_otp,
    }
}

/// What a browser manager lists: over a page, only the accounts for that page's exact https origin
/// (Task 5's `account_matches`; the others are hidden, as iTerm2 has no "use anyway"); no https page,
/// nothing. A picker and the terminal manager list everything.
fn browser_listing(scope: Scope, target: PwmTarget, page: Option<&PwPage>, accounts: Vec<Account>) -> Vec<Account> {
    match (scope, target) {
        (Scope::Browser, PwmTarget::Session | PwmTarget::BasicAuth(_)) => match page {
            Some(p) => accounts.into_iter().filter(|a| bp::account_matches(&a.account_name, p)).collect(),
            None => Vec::new(),
        },
        _ => accounts,
    }
}

/// One thing the browser manager types into a page.
#[derive(Debug, PartialEq, Eq)]
enum PageStep {
    User(String),
    Focus(String),
    Password,
}

/// Enter Username & Password (`PMW:1282-1296`): the user name (when it has one), the focus on the next
/// password field (only while the page's navigation count is still the one it was asked at), then the
/// password.
fn page_steps(send_both: bool, user: &str, focus: Option<&(String, u64)>, epoch: u64) -> Vec<PageStep> {
    let mut v = Vec::new();
    if send_both {
        if !user.is_empty() {
            v.push(PageStep::User(user.to_string()));
        }
        if let Some((id, _)) = focus.filter(|(_, at)| *at == epoch) {
            v.push(PageStep::Focus(id.clone()));
        }
    }
    v.push(PageStep::Password);
    v
}

impl PwmState {
    pub(super) fn auth_mut(&mut self, scope: Scope) -> &mut Auth {
        match scope {
            Scope::Terminal => &mut self.auth,
            Scope::Browser => &mut self.browser_auth,
        }
    }

    pub(super) fn auth_of(&self, scope: Scope) -> &Auth {
        match scope {
            Scope::Terminal => &self.auth,
            Scope::Browser => &self.browser_auth,
        }
    }

    fn info_of(&self, scope: Scope) -> &Option<(Kind, String, Caps)> {
        match scope {
            Scope::Terminal => &self.info,
            Scope::Browser => &self.browser_info,
        }
    }

    /// The source info a store's worker reported; only that store's.
    fn store_info(&mut self, scope: Scope, info: (Kind, String, Caps)) {
        match scope {
            Scope::Terminal => self.info = Some(info),
            Scope::Browser => self.browser_info = Some(info),
        }
    }

    /// The authentication prompt's answer for one store: success unlocks that store only, failure
    /// revokes it. Err is the reason.
    fn apply_answer(&mut self, scope: Scope, answer: win_auth::Answer) -> Result<(), String> {
        let reason = match answer {
            win_auth::Answer::Granted => {
                self.auth_mut(scope).granted(true);
                return Ok(());
            }
            win_auth::Answer::Canceled => "password entry was cancelled.".to_string(),
            win_auth::Answer::Failed(reason) => reason,
        };
        self.auth_mut(scope).granted(false);
        Err(reason)
    }

    /// The first steps of `pwm_reply`, which need no window (review I5), in this order: (1) a basic-auth
    /// store reply goes to the coordinator's owner and ends there; (2) the source info is stored per
    /// scope, a browser reply never overwriting the terminal's; (3) a prompt or account pick with no
    /// manager window to show it drops its sender (Cancel: a locked 1Password save fails, with the
    /// toast). The rest is the window path, handed back.
    fn route_reply(&mut self, r: Reply) -> Option<Reply> {
        let Reply { scope, owner, instance, token, busy, info, done } = r;
        if let Some(i) = info {
            self.store_info(scope, i);
        }
        match done {
            Done::AuthLooked { pane, id, found } => {
                self.basic_auth_store_reply(StoreReply::Looked { pane, id, found });
                None
            }
            Done::AuthSaved { pane, space, host, success } => {
                self.basic_auth_store_reply(StoreReply::Saved { pane, space, host, success });
                None
            }
            Done::Prompt { .. } | Done::Pick { .. } if owner.is_none() => None,
            done => Some(Reply { scope, owner, instance, token, busy, info: None, done }),
        }
    }

    /// The store answers the coordinator's owner has not taken yet.
    pub(super) fn take_store_replies(&mut self) -> Vec<StoreReply> {
        std::mem::take(&mut self.store_replies)
    }

    /// A basic-auth store job's answer, held for the coordinator's owner (Task 10) to take.
    fn basic_auth_store_reply(&mut self, r: StoreReply) {
        self.store_replies.push(r);
    }

    /// The screen locked: both stores re-lock when "after lock" is on.
    fn screen_locked(&mut self, after: bool) {
        self.auth.screen_locked(after);
        self.browser_auth.screen_locked(after);
    }
}

/// One broadcast password round (`iTermBroadcastPasswordHelper`).
struct BroadcastRound {
    round: Round<PaneId>,
    panes: Vec<PaneId>,
    password: Secret,
}

/// A question in its own window, app level (iTerm2's `iTermWarning`).
struct Confirm {
    window: Option<ConfirmWindow>,
    heading: String,
    text: String,
    kind: ConfirmKind,
    /// A notice shown in the remembered mode: the saved answer's label.
    remembered: Option<String>,
}

enum ConfirmKind {
    /// "Are you really at a password prompt?" for a session whose probe failed.
    Check(PaneId, u64),
    /// "Not all sessions at password prompt": Cancel, or enter it in the sessions at a prompt.
    Broadcast(Vec<PaneId>, Secret),
    /// OK only.
    Warning,
    /// An adapter's notice, OK with iTerm2's checkbox (`adapter_warning_spec`).
    Notice(crate::warnings::Spec),
    /// A browser page's autofill press, asked in Tandem's own window (13x7): only Fill, pressed by
    /// the person, runs it, and only on the page it was asked on.
    Autofill(PaneId, super::browser_autofill_app::AutofillAsk),
}

impl App {
    /// Opens the password manager over the window of `pane` (or the current window) to
    /// `account`, on the next pass (`openPasswordManagerToAccountName:inSession:`).
    pub(super) fn open_password_manager(&mut self, account: Option<String>, pane: Option<PaneId>) {
        let focused = self.focused_id();
        let browser = pane.or(focused).is_some_and(|p| self.is_browser_pane(p));
        self.open_password_manager_with(open_request(account, pane, focused, browser));
    }

    /// Opens a manager as `o` describes, on the next pass.
    pub(super) fn open_password_manager_with(&mut self, o: PwmOpen) {
        self.pwm.requests.push(o);
        self.redraw();
    }

    /// Opens what waited for the event loop: managers, questions, LastPass's login window.
    pub(super) fn open_pwm_windows(&mut self, event_loop: &ActiveEventLoop) {
        // One open per window a pass: the first; the rest (a trigger and automatic opening at
        // the same prompt, say) would only find it open, or ask to authenticate again.
        let mut handled: Vec<WindowId> = self.pwm.refused.take().into_iter().collect();
        for o in std::mem::take(&mut self.pwm.requests) {
            let challenge = match o.target {
                PwmTarget::BasicAuth(n) => Some(n),
                PwmTarget::Session => None,
            };
            // A picker that cannot open ends its challenge (the prompt comes back with what was typed).
            if !self.open_pwm_now(event_loop, o, &mut handled) {
                if let Some(n) = challenge {
                    self.basic_auth_picked(n, None);
                }
            }
        }
        if let Some(email) = self.pwm.lp_terminal.take() {
            self.open_lp_login(event_loop, &email);
        }
        self.watch_lp_login(event_loop);
        let Some(g) = self.gpu.as_ref() else { return };
        let underline = self.globals.bool("AlertsIndicateShortcuts");
        let mut failed = None;
        for c in self.pwm.confirms.iter_mut().filter(|c| c.window.is_none()) {
            let opened = match &c.kind {
                ConfirmKind::Check(..) => ConfirmWindow::choice(event_loop, &g.instance, &g.adapter, &g.device, &g.queue, &c.heading, &c.text, pwm::TITLE_CANCEL, pwm::TITLE_ENTER_PASSWORD),
                ConfirmKind::Broadcast(..) => ConfirmWindow::choice(event_loop, &g.instance, &g.adapter, &g.device, &g.queue, &c.heading, &c.text, pwm::TITLE_CANCEL, BROADCAST_ENTER),
                ConfirmKind::Warning => ConfirmWindow::warning(event_loop, &g.instance, &g.adapter, &g.device, &g.queue, &c.heading, &c.text),
                ConfirmKind::Autofill(..) => ConfirmWindow::choice(event_loop, &g.instance, &g.adapter, &g.device, &g.queue, &c.heading, &c.text, pwm::TITLE_CANCEL, crate::browser_autofill::CONFIRM_FILL),
                ConfirmKind::Notice(spec) => super::warnings_app::open_warning(event_loop, (&g.instance, &g.adapter, &g.device, &g.queue), spec, c.remembered.as_deref(), underline),
            };
            match opened {
                Ok(mut w) => {
                    // A question a page can raise: the one shared input guard (prompt_ui::InputGuard,
                    // 1 s, restarted on refocus, keys, clicks and touch), no default button.
                    if matches!(c.kind, ConfirmKind::Autofill(..)) {
                        w.guard_input();
                    }
                    c.window = Some(w);
                }
                Err(e) => failed = Some(e),
            }
        }
        if let Some(e) = failed {
            // A question that cannot be asked is answered Cancel.
            let lost: Vec<Confirm> = {
                let (open, lost) = std::mem::take(&mut self.pwm.confirms).into_iter().partition(|c| c.window.is_some());
                self.pwm.confirms = open;
                lost
            };
            for c in lost {
                self.pwm_confirmed(c.kind, ConfirmResult::Cancel);
            }
            self.banners.set("window", format!("Could not open the window: {e}"));
        }
    }

    /// `openPasswordManagerToAccountName:inSession:forUser:didSendUserName:` (`PT:10302-10346`).
    /// True when a manager opened (or the open was refused as iTerm2 refuses it); false when a picker's
    /// challenge must end because nothing opened.
    fn open_pwm_now(&mut self, event_loop: &ActiveEventLoop, o: PwmOpen, handled: &mut Vec<WindowId>) -> bool {
        let PwmOpen { account, pane, scope, for_user, send_both, target, focus_after_user, from_page, pick_page } = o;
        let session = target == PwmTarget::Session;
        if let Some(p) = pane {
            // Unburied first, else it has no window (`AD:650-653`).
            self.disinter(p);
            if !self.enter_pane_window(p) {
                return session;
            }
            self.reveal_pane(p);
        } else if let Some(w) = self.last_window {
            self.enter(w);
        }
        let Some(here) = self.current_window() else { return session };
        // The refuse-if-open rules are the session path's; a picker's one slot is decided below.
        if session && !first_for(handled, here) {
            return true;
        }
        match open_decision(target, self.pwm_window.is_some()) {
            OpenDecision::Open => {}
            OpenDecision::Refuse => return true, // "Password manager sheet already open"
            OpenDecision::AnswerNone => return false, // the challenge ends with None; the open manager stays
        }
        // `canOpenPasswordManager`: not while that session's probe runs (a terminal session's probe).
        if session && pane.and_then(|p| self.panes.get(&p)).and_then(|p| p.echo_probe.as_ref()).is_some_and(Probe::active) {
            return true;
        }
        // It would be a shame to send a focus report to a password prompt.
        let hold = pane.or_else(|| self.focused_id());
        if let Some(p) = hold.and_then(|id| self.panes.get_mut(&id)) {
            p.no_focus_reports += 1;
        }
        let required = self.pwm_setting(pwm::REQUIRE_OPEN_KEY);
        let every = self.pwm_setting(pwm::REQUIRE_EVERY_KEY);
        let prompted = self.pwm_auth(scope).needs_prompt(required, every);
        if prompted {
            if !self.authenticate(scope) {
                // The prompt took the focus: the hold ends when the window gets it back.
                if let Some(p) = hold {
                    self.pwm.holds.push(p);
                }
                if session {
                    self.pwm.refused = Some(here);
                }
                return session;
            }
        } else if !required {
            self.pwm_auth(scope).open_unprompted();
        }
        let opened = match (self.gpu.as_ref(), self.hwnd()) {
            (Some(g), Some(h)) => PwmWindow::open(event_loop, &g.instance, &g.adapter, &g.device, &g.queue, h, crate::tab_theme::forced_dark(self.tab_style())).map(|w| (w, h)),
            _ => Err("no terminal window".to_string()),
        };
        let (window, owner_hwnd) = match opened {
            Ok((mut win, h)) => {
                if from_page {
                    win.guard_input();
                }
                (win, h)
            }
            Err(e) => {
                self.banners.set("window", format!("Could not open the window: {e}"));
                match hold {
                    Some(p) if prompted => self.pwm.holds.push(p),
                    Some(p) => {
                        if let Some(p) = self.panes.get_mut(&p) {
                            p.no_focus_reports = p.no_focus_reports.saturating_sub(1);
                        }
                    }
                    None => {}
                }
                return session;
            }
        };
        if let Some(h) = window.hwnd() {
            crate::winkeys::add_pwm_window(h);
        }
        lock::watch(owner_hwnd, self.notify.clone());
        self.pwm.instances += 1;
        self.pwm_window = Some(PwmWin {
            window,
            instance: self.pwm.instances,
            hold,
            accounts: Vec::new(),
            filter: String::new(),
            selected: None,
            available: true,
            busy: Busy::default(),
            panel: None,
            editing: None,
            baseline: None,
            alerts: VecDeque::new(),
            two_factor: String::new(),
            broadcast: false,
            default_name: account.clone().unwrap_or_default(),
            select_name: account,
            select_pair: None,
            scope,
            page_pane: if scope == Scope::Browser { pane } else { None },
            send_user_by_default: for_user,
            send_both,
            target,
            focus_after_user,
            pending_user: None,
            pick_page,
        });
        self.pwm_job(Work::Load, true);
        true
    }

    /// Windows Hello for the signed-in user when it is available (iTerm2's `LAContext`
    /// `deviceOwnerAuthentication`), else the Windows credential prompt checked with `LogonUserW`
    /// (`requestAuthenticationIfNeeded`). A failure or a cancel shows iTerm2's alert.
    pub(super) fn authenticate(&mut self, scope: Scope) -> bool {
        let owner = self.hwnd().unwrap_or(0);
        // Windows Hello (PIN, fingerprint, face) where it is set up, else the password prompt.
        let answer = win_auth::hello(owner).unwrap_or_else(|| win_auth::prompt(owner));
        match self.pwm.apply_answer(scope, answer) {
            Ok(()) => true,
            Err(reason) => {
                self.pwm_warning(AUTH_FAILED_TITLE, &format!("Authentication failed because {reason}"));
                false
            }
        }
    }

    /// The authentication of one store (iTerm2 keeps `authenticated` per provider, `PDS:15`).
    pub(super) fn pwm_auth(&mut self, scope: Scope) -> &mut Auth {
        self.pwm.auth_mut(scope)
    }

    fn pwm_warning(&mut self, heading: &str, text: &str) {
        self.pwm.confirms.push(Confirm { window: None, heading: heading.to_string(), text: text.to_string(), kind: ConfirmKind::Warning, remembered: None });
    }

    /// A gear setting's value (stored, else iTerm2's default).
    fn pwm_setting(&self, key: &str) -> bool {
        pwm::setting(&|k| self.globals.get(k).and_then(|v| v.as_bool()), key)
    }

    fn probe_duration(&self) -> f64 {
        self.globals.number(echo_probe::DURATION_KEY).unwrap_or(echo_probe::DEFAULT_DURATION)
    }

    /// The store the manager in front serves (the terminal's when none is open).
    fn cur_scope(&self) -> Scope {
        self.pwm_window.as_ref().map_or(Scope::Terminal, |w| w.scope)
    }

    fn pwm_kind(&self) -> Kind {
        self.pwm_kind_for(self.cur_scope())
    }

    fn pwm_kind_for(&self, scope: Scope) -> Kind {
        Kind::from_stored(&self.globals.string(scope.source_key()))
    }

    /// The current source's name and capabilities, once the worker has built it.
    fn pwm_info(&self) -> (String, Caps) {
        let kind = self.pwm_kind();
        match self.pwm.info_of(self.cur_scope()) {
            Some((k, name, caps)) if *k == kind => (name.clone(), caps.clone()),
            _ => (String::new(), Caps::default()),
        }
    }

    /// Events for a password manager window. False for other windows.
    pub(super) fn pwm_window_event(&mut self, id: WindowId, event: &WindowEvent) -> bool {
        if !self.pwm_window.as_ref().is_some_and(|w| w.id() == id) {
            match self.window_of_pwm(id) {
                Some(owner) if self.enter(owner) => {}
                _ => return false,
            }
        }
        match event {
            WindowEvent::CloseRequested => self.close_pwm(),
            WindowEvent::RedrawRequested => {
                for act in self.pwm_redraw() {
                    self.pwm_act(act);
                }
            }
            other => {
                if let Some(w) = self.pwm_window.as_mut() {
                    if w.window.event(other) {
                        w.window.request_redraw();
                    }
                }
            }
        }
        true
    }

    /// Draws the manager.
    fn pwm_redraw(&mut self) -> Vec<PwmAct> {
        let buttons = self.pwm_buttons();
        let gear = self.pwm_gear();
        let Some(w) = self.pwm_window.as_mut() else { return Vec::new() };
        let asking = matches!(w.alerts.front(), Some((_, AlertFor::Prompt(_) | AlertFor::Pick(..))));
        let PwmWin { window, accounts, filter, selected, busy, panel, alerts, .. } = w;
        let view = PwmView {
            accounts,
            filter,
            selected: *selected,
            buttons,
            // A prompt from the worker shows over the scrim of the request that asked.
            busy: busy.shown() && !asking,
            gear,
            panel: panel.as_ref(),
            alert: alerts.front().map(|(a, _)| a),
        };
        window.redraw(&view)
    }

    /// `updateWithAvailability:`'s inputs.
    fn pwm_buttons(&self) -> pwm::Buttons {
        let w = self.pwm_window.as_ref();
        let sel = w.and_then(PwmWin::selected_account);
        let focused = self.focused_id();
        let scope = self.cur_scope();
        let session_can_enter = match (scope, w.and_then(|w| w.page_pane)) {
            // A browser page takes an entry only from an account for its exact https origin.
            (Scope::Browser, pane) => pane.zip(sel).is_some_and(|(pane, a)| self.page_accepts(pane, &a.account_name)),
            (Scope::Terminal, _) => focused.and_then(|id| self.panes.get(&id)).is_some_and(|p| p.exited.is_none()),
        };
        let can_broadcast = scope == Scope::Terminal && focused.is_some_and(|id| self.broadcast.targets(&self.broadcast_view(), id).len() > 1);
        pwm::buttons(&button_state(&ButtonInputs {
            target: w.map_or(PwmTarget::Session, |w| w.target),
            for_user: w.is_some_and(|w| w.send_user_by_default),
            send_both: w.is_some_and(|w| w.send_both),
            selected: sel.is_some(),
            user_name_empty: sel.is_none_or(|a| a.user_name.is_empty()),
            session_can_enter,
            can_broadcast,
            available: w.is_some_and(|w| w.available),
            sends_otp: sel.is_some_and(|a| a.send_otp),
        }))
    }

    /// Whether `pane` shows an https page that the stored `account` is for, exactly (Task 5's one
    /// matcher, `account_matches`).
    fn page_accepts(&self, pane: PaneId, account: &str) -> bool {
        self.page_bound_now(pane).and_then(|(url, epoch)| PwPage::new(&url, epoch)).is_some_and(|p| bp::account_matches(account, &p))
    }

    fn pwm_gear(&self) -> pwm::Gear {
        let (name, caps) = self.pwm_info();
        let scope = self.cur_scope();
        let authenticated = self.pwm.auth_of(scope).authenticated;
        pwm::Gear {
            authenticated,
            require_open: self.pwm_setting(pwm::REQUIRE_OPEN_KEY),
            require_lock: self.pwm_setting(pwm::REQUIRE_LOCK_KEY),
            require_every: self.pwm_setting(pwm::REQUIRE_EVERY_KEY),
            probe: pwm::should_probe(self.pwm_setting(pwm::PROBE_KEY), self.probe_duration()),
            send_return: self.pwm_setting(pwm::SEND_RETURN_KEY),
            source: self.pwm_kind(),
            reset_title: pwm::reset_title(caps.can_reset, &name),
            reset_on: caps.can_reset,
            switch_on: authenticated && caps.multiple_accounts,
            adapter_settings: !caps.settings.is_empty(),
            commands: caps.commands.clone(),
            browser: scope == Scope::Browser,
        }
    }

    /// Closes this window's manager (`closeOrEndSheet`). Its session's focus reports stay held
    /// until the terminal window has the focus back.
    pub(super) fn close_pwm(&mut self) {
        let Some(w) = crate::transparency::drop_owned(&mut self.pwm_window) else { return };
        if let Some(p) = w.hold {
            self.pwm.holds.push(p);
        }
        // Dropping the window answers any prompt it showed with Cancel, and a picker's challenge with
        // nothing (its `Drop` queues it; this ends it now).
        drop(w);
        for n in std::mem::take(&mut *ORPHANED.lock().unwrap()) {
            self.basic_auth_picked(n, None);
        }
    }

    /// The current window gained the focus: holds left by a closed manager on its sessions end
    /// (a session moved here since counts too).
    pub(super) fn pwm_focus_gained(&mut self) {
        let here: Vec<PaneId> = self.tabs.iter().flat_map(|t| t.layout.panes()).collect();
        let (mine, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut self.pwm.holds).into_iter().partition(|p| here.contains(p) || !self.panes.contains_key(p));
        self.pwm.holds = rest;
        for pane in mine {
            if let Some(p) = self.panes.get_mut(&pane) {
                p.no_focus_reports = p.no_focus_reports.saturating_sub(1);
            }
        }
    }

    /// A pane back from Undo Close: its focus reports are held once for each hold still on it
    /// (a manager open over it, a closed manager's hold waiting for the focus, its waiting
    /// password), never more, since holds dropped while it was stashed took nothing off; and a
    /// probe nothing waits on any more ends.
    pub(super) fn pwm_restored(&self, pane: &mut Pane) {
        let windows: Vec<Option<PaneId>> = self.pwm_windows().into_iter().map(|w| w.hold).collect();
        pane.no_focus_reports = true_holds(pane.id, self.pwm.entries.contains_key(&pane.id), &self.pwm.holds, windows);
        let waited = self.pwm.entries.contains_key(&pane.id) || self.pwm.rounds.iter().any(|r| r.panes.contains(&pane.id));
        if !waited {
            pane.echo_probe = None;
        }
    }

    /// Win+B, Win+U or Win+C from the key hook with a manager in front. True when taken.
    pub(super) fn pwm_take_chord(&mut self, c: &crate::menus::Chord) -> bool {
        let crate::menus::ChordKey::Char(key) = c.key else { return false };
        if !crate::winkeys::pwm_chord(c) {
            return false;
        }
        // The window in front when the hook took the chord, not now.
        let Some(owner) = self.pwm_of_hwnd(crate::winkeys::chord_foreground()) else { return false };
        if !self.enter(owner) {
            return false;
        }
        if let Some(w) = self.pwm_window.as_mut() {
            w.window.win_chord(key);
        }
        true
    }

    /// What the user did in the manager.
    pub(super) fn pwm_act(&mut self, act: PwmAct) {
        let authenticated = self.pwm.auth_of(self.cur_scope()).authenticated;
        let (_, caps) = self.pwm_info();
        let Some(w) = self.pwm_window.as_mut() else { return };
        match act {
            PwmAct::Filter(f) => {
                w.filter = f;
                let rows: Vec<usize> = (0..w.accounts.len()).filter(|&i| pwm::matches(&w.accounts[i], &w.filter)).collect();
                w.selected = pwm::select_after_filter(&rows, w.selected);
            }
            PwmAct::Select(i) => w.selected = i.filter(|&i| i < w.accounts.len()),
            PwmAct::TwoFactor(s) => w.two_factor = s,
            PwmAct::Broadcast(on) => w.broadcast = on,
            PwmAct::Close => self.close_pwm(),
            PwmAct::EnterPassword => {
                let can_enter = self.pwm_buttons().default_on;
                let Some(w) = self.pwm_window.as_ref() else { return };
                if let (true, true, Some(a)) = (authenticated, can_enter, w.selected_account().cloned()) {
                    // The pick binds the page now, before the secret is fetched.
                    let Some(why) = self.pick_why(&a) else { return };
                    self.pwm_job(Work::Fetch(a, why), true);
                }
            }
            PwmAct::EnterUserName => {
                let user = w.selected_account().map(|a| a.user_name.clone()).unwrap_or_default();
                let broadcast = w.broadcast;
                let account = w.selected_account().map(|a| a.account_name.clone()).unwrap_or_default();
                if authenticated && !user.is_empty() && self.pwm_buttons().secondary_on {
                    self.pwm_enter_user(&account, &user, broadcast);
                }
            }
            PwmAct::Add => {
                if authenticated {
                    self.pwm.serial += 1;
                    w.panel = Some(AccountPanel::add(self.pwm.serial, &w.default_name, &caps));
                    w.editing = None;
                    w.baseline = None;
                }
            }
            PwmAct::Remove => {
                if let (true, Some(a)) = (authenticated, w.selected_account().cloned()) {
                    w.push_alert(Alert::confirm_delete(), AlertFor::Delete(a));
                }
            }
            PwmAct::Edit(i) => {
                let Some(a) = w.accounts.get(i).cloned() else { return };
                w.selected = Some(i);
                self.pwm.serial += 1;
                let serial = self.pwm.serial;
                w.panel = Some(AccountPanel::edit(serial, &a, &caps));
                w.editing = Some(a.clone());
                w.baseline = None;
                if caps.can_edit_password {
                    self.pwm_job(Work::Fetch(a, Why::Prefill(serial)), false);
                }
            }
            PwmAct::Gear(item) => self.pwm_gear_item(item),
            PwmAct::AppendOtp(i) => {
                if let (true, Some(a)) = (authenticated, w.accounts.get(i).cloned()) {
                    self.pwm_job(Work::ToggleOtp(a), true);
                }
            }
            PwmAct::Reveal(i) => {
                if let (true, Some(a)) = (authenticated, w.accounts.get(i).cloned()) {
                    self.pwm_job(Work::Fetch(a, Why::Reveal), true);
                }
            }
            PwmAct::CopyPassword(i) => {
                if let (true, Some(a)) = (authenticated, w.accounts.get(i).cloned()) {
                    self.pwm_job(Work::Fetch(a, Why::Copy), true);
                }
            }
            PwmAct::PanelOk { name, user, password, toggle } => self.pwm_panel_ok(name, user, password, toggle, &caps),
            PwmAct::PanelCancel => {
                w.panel = None;
                w.editing = None;
                w.baseline = None;
            }
            PwmAct::Generate { alnum } => {
                if let Some(serial) = w.panel.as_ref().map(|p| p.serial) {
                    let p = pwm::random_password(alnum, &mut random_below);
                    w.window.fill_password(serial, p);
                }
            }
            PwmAct::AlertButton(n) => self.pwm_alert_answer(Some(n), None),
            PwmAct::Settings(values) => self.pwm_alert_answer(None, Some(values)),
            PwmAct::CancelBusy => w.busy.cancel(),
        }
        if let Some(w) = self.pwm_window.as_ref() {
            w.window.request_redraw();
        }
    }

    /// OK on the New Account / Edit Account panel (`reallyAdd:`, `commitEditWithAccountName:`).
    fn pwm_panel_ok(&mut self, name: String, user: String, password: Secret, toggle: Option<bool>, caps: &Caps) {
        let Some(w) = self.pwm_window.as_mut() else { return };
        let Some(panel) = w.panel.as_mut() else { return };
        if name.is_empty() || panel.fetching {
            w.window.shake(false);
            return;
        }
        if let Some(a) = w.editing.clone() {
            if pwm::rename_refused(&a, &name, &user, caps.in_place, &w.accounts) {
                w.window.shake(false);
                return;
            }
            let new_password = pwm::edit_password(&password, w.baseline.as_ref());
            w.panel = None;
            w.editing = None;
            w.baseline = None;
            let work = match pwm::plan_commit(&a, &name, &user, new_password.is_some(), caps.in_place) {
                pwm::Commit::Nothing => return,
                pwm::Commit::SetPassword => match new_password {
                    Some(p) => Work::SetPassword(a, p),
                    None => return,
                },
                pwm::Commit::InPlace { name: n, user: u } => Work::EditInPlace { account: a, name: n, user: u, password: new_password, pair: (name, user) },
                pwm::Commit::Readd { flags } => Work::Readd { account: a, name, user, password: new_password, flags },
            };
            self.pwm_job(work, true);
            return;
        }
        match pwm::add_check(&name, &user, &password, caps.requires_password, false, &w.accounts) {
            Ok(()) => {}
            Err(pwm::AddRefusal::NeedsPassword) => {
                panel.needs_password();
                w.window.shake(true);
                return;
            }
            Err(_) => {
                w.window.shake(false);
                return;
            }
        }
        let flags: Vec<(String, bool)> = match (&panel.toggle, toggle) {
            (Some(t), Some(on)) if !t.key.is_empty() => vec![(t.key.clone(), on)],
            _ => Vec::new(),
        };
        w.panel = None;
        self.pwm_job(Work::Add { name, user, password, flags }, true);
    }

    /// The gear menu's items (`W:555-668`, `W:1815-1830`). Every item needs authentication.
    fn pwm_gear_item(&mut self, item: GearItem) {
        if !self.pwm.auth_of(self.cur_scope()).authenticated {
            return;
        }
        let (name, caps) = self.pwm_info();
        match item {
            GearItem::RequireOpen => self.pwm_toggle(pwm::REQUIRE_OPEN_KEY),
            GearItem::RequireLock => self.pwm_toggle(pwm::REQUIRE_LOCK_KEY),
            GearItem::RequireEvery => self.pwm_toggle(pwm::REQUIRE_EVERY_KEY),
            GearItem::SendReturn => self.pwm_toggle(pwm::SEND_RETURN_KEY),
            GearItem::Probe => {
                // `setProbeForPassword:`: turning it on with the probe's time at 0 reverts the
                // legacy setting to 0.5 s.
                let on = !pwm::should_probe(self.pwm_setting(pwm::PROBE_KEY), self.probe_duration());
                if on && self.probe_duration() == 0.0 {
                    self.globals.set(echo_probe::DURATION_KEY, toml::Value::Float(echo_probe::DEFAULT_DURATION));
                }
                self.globals.set(pwm::PROBE_KEY, toml::Value::Boolean(on));
                self.save_globals();
            }
            GearItem::Source(k) => self.pwm_choose(k),
            GearItem::Reset => {
                if caps.can_reset {
                    if let Some(w) = self.pwm_window.as_mut() {
                        w.push_alert(Alert::reset(), AlertFor::Reset);
                    }
                }
            }
            GearItem::SwitchAccount => {
                if caps.multiple_accounts {
                    self.pwm_job(Work::Switch, true);
                }
            }
            GearItem::Settings => {
                let fields: Vec<(Field, Secret)> = caps.settings.iter().map(|f| (f.clone(), self.adapter_setting(f))).collect();
                if !fields.is_empty() {
                    if let Some(w) = self.pwm_window.as_mut() {
                        w.push_alert(Alert::settings(&name, fields), AlertFor::Settings);
                    }
                }
            }
            GearItem::Command(c) => self.pwm_job(Work::Command(c), true),
            GearItem::Reload => self.pwm_job(Work::Reload, true),
        }
    }

    fn pwm_toggle(&mut self, key: &str) {
        let now = self.pwm_setting(key);
        self.globals.set(key, toml::Value::Boolean(!now));
        self.save_globals();
    }

    /// Choosing a source (`useKeychain:`, `use1Password:` and the like): stored at once; a
    /// source whose check fails shows its alert and falls back to Credential Manager (kept).
    fn pwm_choose(&mut self, k: Kind) {
        // Each store keeps its own choice (`NoSyncPasswordManagerDataSourceName` + "Browser").
        self.globals.set(self.cur_scope().source_key(), toml::Value::String(k.stored().to_string()));
        self.save_globals();
        if k == Kind::CredentialManager {
            self.pwm_job(Work::Load, true);
        } else {
            self.pwm_job(Work::Choose, true);
        }
    }

    fn pwm_fall_back(&mut self) {
        self.pwm_choose(Kind::CredentialManager);
    }

    /// The answer to the alert in front: a button, or OK on a sheet with fields.
    fn pwm_alert_answer(&mut self, button: Option<usize>, values: Option<Vec<(String, Secret)>>) {
        let Some(w) = self.pwm_window.as_mut() else { return };
        let Some((alert, what)) = w.alerts.pop_front() else { return };
        let ok = values.is_some() || button == Some(0);
        match what {
            AlertFor::Plain => {}
            AlertFor::Delete(a) => {
                if ok {
                    self.pwm_job(Work::Delete(a), true);
                }
            }
            AlertFor::Reveal => {
                if button == Some(1) {
                    if let Some(s) = alert.secret.as_ref() {
                        self.pwm_copy(s);
                    }
                }
            }
            AlertFor::Reset => {
                if ok {
                    self.pwm_reset();
                }
            }
            AlertFor::Settings => {
                if let Some(values) = values {
                    self.save_adapter_settings(values);
                    self.pwm_job(Work::Reload, true);
                }
            }
            AlertFor::Prompt(tx) => {
                let _ = tx.send(values);
            }
            AlertFor::Pick(ids, tx) => {
                let id = button.and_then(|n| ids.get(n).cloned());
                if let Some(id) = &id {
                    self.globals.set(OP_ACCOUNT_KEY, toml::Value::String(id.clone()));
                    self.save_globals();
                }
                let _ = tx.send(id);
            }
            AlertFor::OpTooOld(choosing) => {
                if let Some(w) = self.pwm_window.as_mut() {
                    w.push_alert(cannot_find_alert(Kind::OnePassword), AlertFor::CannotFind(Kind::OnePassword, choosing));
                }
            }
            AlertFor::CannotFind(kind, choosing) => {
                if button == Some(2) && kind == Kind::LastPass {
                    if let Some(p) = self.focused_id() {
                        self.open_url(p, LP_HELP_URL);
                    }
                }
                let picked = if button == Some(0) { locate_cli(kind) } else { None };
                match picked {
                    Some(path) => {
                        let key = if kind == Kind::OnePassword { OP_PATH_KEY } else { LP_PATH_KEY };
                        self.globals.set(key, toml::Value::String(path));
                        self.save_globals();
                        self.pwm_job(if choosing { Work::Choose } else { Work::Load }, true);
                    }
                    None if choosing => self.pwm_fall_back(),
                    None => self.pwm_unavailable(),
                }
            }
            AlertFor::LpAuthFailed(email) => match button {
                Some(0) => self.pwm.lp_terminal = Some(email),
                Some(1) => {
                    if let Some(cb) = &mut self.clipboard {
                        let _ = cb.set_text(pwm_cli::lp_login_command(&email));
                    }
                }
                _ => {}
            },
        }
    }

    /// `dataSourceDidBecomeUnavailable`: an empty list and "+" off.
    fn pwm_unavailable(&mut self) {
        if let Some(w) = self.pwm_window.as_mut() {
            w.accounts.clear();
            w.selected = None;
            w.available = false;
        }
    }

    /// Copy Password, Reveal's Copy, Ctrl+C and Win+C: kept out of Windows clipboard history
    /// (Win+V) and cloud clipboard. A Windows addition (m10b ruling): iTerm2 copies a plain
    /// string (`copyPasswordToClipboard:`), and macOS keeps no history to leak it into.
    fn pwm_copy(&mut self, s: &Secret) {
        use arboard::SetExtWindows;
        if let Some(cb) = &mut self.clipboard {
            if let Err(e) = cb.set().exclude_from_history().exclude_from_cloud().text(s.expose()) {
                self.banners.set("clipboard", format!("Copy failed: {e}"));
            }
        }
    }

    /// Reset <name> Configuration: an adapter forgets its paths and settings.
    fn pwm_reset(&mut self) {
        let kind = self.pwm_kind();
        if let Some(id) = adapter_identifier(kind) {
            let prefix = format!("AdapterSetting_{id}_");
            let keys: Vec<String> = self.globals.values.keys().filter(|k| k.starts_with(&prefix) || **k == format!("PathToDatabase_{id}") || **k == format!("PathToExecutable_{id}")).cloned().collect();
            for k in keys {
                self.globals.values.remove(&k);
            }
            self.save_globals();
            self.pwm.secret_settings.retain(|k, _| !k.starts_with(&prefix));
        }
        self.pwm_job(Work::Reset, true);
    }

    /// An adapter Settings field's value: a secret one from this run's memory, the rest from
    /// the globals (`AdapterSetting_<name>_<key>`, iTerm2's key).
    fn adapter_setting(&self, f: &Field) -> Secret {
        let Some(id) = adapter_identifier(self.pwm_kind()) else { return Secret::new(String::new()) };
        let key = format!("AdapterSetting_{id}_{}", f.key);
        if f.persist_in_keychain {
            self.pwm.secret_settings.get(&key).map(Secret::duplicate).unwrap_or_else(|| Secret::new(String::new()))
        } else {
            Secret::new(self.globals.string(&key))
        }
    }

    fn save_adapter_settings(&mut self, values: Vec<(String, Secret)>) {
        let kind = self.pwm_kind();
        let Some(id) = adapter_identifier(kind) else { return };
        let (_, caps) = self.pwm_info();
        for (k, v) in values {
            let Some(f) = caps.settings.iter().find(|f| f.key == k) else { continue };
            let key = format!("AdapterSetting_{id}_{k}");
            let value = v.expose().trim().to_string();
            if f.persist_in_keychain {
                self.pwm.secret_settings.insert(key, Secret::new(value));
            } else if value.is_empty() {
                self.globals.values.remove(&key);
            } else {
                self.globals.set(&key, toml::Value::String(value));
            }
        }
        self.save_globals();
    }

    /// What the worker builds the source from.
    fn pwm_config(&self) -> Config {
        self.pwm_config_for(self.cur_scope())
    }

    /// What the worker of `scope`'s store builds its source from (a sign-in prompt's store jobs have
    /// no manager window, so the scope is named, not read from the window in front).
    fn pwm_config_for(&self, scope: Scope) -> Config {
        let kind = self.pwm_kind_for(scope);
        let cli = match kind {
            Kind::OnePassword => self.globals.string(OP_PATH_KEY),
            Kind::LastPass => self.globals.string(LP_PATH_KEY),
            _ => String::new(),
        };
        let mut adapter = AdapterConfig::default();
        if let Some(id) = adapter_identifier(kind) {
            let get = |k: String| Some(self.globals.string(&k)).filter(|v| !v.is_empty());
            adapter.db = get(format!("PathToDatabase_{id}"));
            adapter.exe = get(format!("PathToExecutable_{id}"));
            let prefix = format!("AdapterSetting_{id}_");
            let plain = self.globals.values.iter().filter_map(|(k, v)| Some((k.strip_prefix(&prefix)?.to_string(), v.as_str()?.to_string())));
            let secret = self.pwm.secret_settings.iter().filter_map(|(k, v)| Some((k.strip_prefix(&prefix)?.to_string(), v.expose().to_string())));
            adapter.settings = plain.chain(secret).collect();
            adapter.settings.sort();
        }
        Config { kind, cli, groups: self.globals.string(LP_GROUPS_KEY), account: self.globals.string(OP_ACCOUNT_KEY), adapter, scope }
    }

    /// Sends a request for the current window's manager to the worker; `busy` shows the scrim
    /// and makes Cancel drop its reply.
    fn pwm_job(&mut self, work: Work, busy: bool) {
        let Some(owner) = self.current_window() else { return };
        let config = self.pwm_config();
        let Some(w) = self.pwm_window.as_mut() else { return };
        let token = if busy { w.busy.incr() } else { w.busy.cancels };
        let instance = w.instance;
        w.window.request_redraw();
        let notify = self.notify.clone();
        let scope = config.scope;
        let slot = match scope {
            Scope::Terminal => &mut self.pwm.worker,
            Scope::Browser => &mut self.pwm.browser_worker,
        };
        let worker = slot.get_or_insert_with(|| Worker::start(notify, scope));
        if worker.tx.send(Job::Window(WindowJob { owner, instance, token, busy, config, work })).is_err() {
            *slot = None;
            if busy {
                w.busy.decr();
            }
            self.banners.set("pwm", "The password manager could not start its worker thread.");
        }
    }

    /// The browser store can be read now with no prompt (REQUIRE_OPEN and REQUIRE_EVERY honored).
    pub(super) fn basic_auth_unlocked(&self) -> bool {
        let required = self.pwm_setting(pwm::REQUIRE_OPEN_KEY);
        let every = self.pwm_setting(pwm::REQUIRE_EVERY_KEY);
        crate::browser_basic_auth::unlocked_without_prompt(self.pwm.browser_auth.authenticated, required, every)
    }

    /// Saving a remembered sign-in needs the browser store open: whether that is a prompt now. The
    /// answer is for the BROWSER store only; the terminal's authentication is never touched.
    pub(super) fn basic_auth_save_needs_prompt(&mut self) -> bool {
        let required = self.pwm_setting(pwm::REQUIRE_OPEN_KEY);
        let every = self.pwm_setting(pwm::REQUIRE_EVERY_KEY);
        let needs = self.pwm_auth(Scope::Browser).needs_prompt(required, every);
        if !needs && !required {
            self.pwm_auth(Scope::Browser).open_unprompted();
        }
        needs
    }

    /// A challenge ended elsewhere (answered, cancelled, superseded): its picker goes, queued opens
    /// included; an open picker window closes.
    pub(super) fn basic_auth_picker_gone(&mut self, id: u64) {
        self.pwm.requests.retain(|o| o.target != PwmTarget::BasicAuth(id));
        if self.pwm_window.as_ref().is_some_and(|w| w.target == PwmTarget::BasicAuth(id)) {
            self.close_pwm();
        }
    }

    /// Sends a basic-auth store job to the browser store's worker (13x7 Task 9). The store is read only
    /// when it is unlocked without any prompt (`BAS:87-89`); a locked one answers a miss or a failed save
    /// at once, and never asks here. Returns false when it could not be sent.
    pub(super) fn basic_auth_store_job(&mut self, work: AuthWork) -> bool {
        let required = self.pwm_setting(pwm::REQUIRE_OPEN_KEY);
        let every = self.pwm_setting(pwm::REQUIRE_EVERY_KEY);
        let authenticated = self.pwm.browser_auth.authenticated;
        // A read is silent only when no prompt is due (REQUIRE_EVERY asks each time, so a lookup is a
        // miss). A save follows the authentication the caller just did, which `every` would not reuse.
        let open = match work {
            AuthWork::Lookup { .. } => crate::browser_basic_auth::unlocked_without_prompt(authenticated, required, every),
            AuthWork::Save { .. } => authenticated || !required,
        };
        if !open {
            self.pwm.store_replies.push(match auth_failed(work) {
                Done::AuthLooked { pane, id, found } => StoreReply::Looked { pane, id, found },
                Done::AuthSaved { pane, space, host, success } => StoreReply::Saved { pane, space, host, success },
                _ => return false,
            });
            return true;
        }
        let config = self.pwm_config_for(Scope::Browser);
        let notify = self.notify.clone();
        let slot = &mut self.pwm.browser_worker;
        let worker = slot.get_or_insert_with(|| Worker::start(notify, Scope::Browser));
        if worker.tx.send(Job::Auth(AuthJob { config, work })).is_err() {
            *slot = None;
            return false;
        }
        true
    }

    /// Every other window's manager lists the accounts again (`kPasswordManagersShouldReloadData`);
    /// with `this` too, this window's also.
    fn pwm_passwords_changed(&mut self, this: bool) {
        let here = self.current_window();
        for owner in self.pwm_owners() {
            if (Some(owner) == here && !this) || !self.enter(owner) {
                continue;
            }
            self.pwm_job(Work::List, true);
        }
        if let Some(h) = here {
            self.enter(h);
        }
    }

    /// Handles the worker's replies, the probes' deadlines, broadcast rounds and a screen lock.
    /// Returns when the next probe deadline is due.
    pub(super) fn tick_pwm(&mut self) -> Option<Instant> {
        if lock::take() {
            self.pwm_screen_locked();
        }
        if lock::watched() == 0 && (self.pwm.auth.authenticated || self.pwm.browser_auth.authenticated) {
            if let Some(h) = self.hwnd() {
                lock::watch(h, self.notify.clone());
            }
        }
        let replies: Vec<Reply> = [&self.pwm.worker, &self.pwm.browser_worker].into_iter().flatten().flat_map(|w| w.rx.try_iter()).collect();
        // Pickers that closed without a choice end their challenges.
        for n in std::mem::take(&mut *ORPHANED.lock().unwrap()) {
            self.basic_auth_picked(n, None);
        }
        for r in replies {
            self.pwm_reply(r);
        }
        let now = Instant::now();
        let due: Vec<PaneId> = self.panes.iter().filter(|(_, p)| p.echo_probe.as_ref().and_then(Probe::due).is_some_and(|d| now >= d)).map(|(id, _)| *id).collect();
        for id in due {
            let outs = self.panes.get_mut(&id).and_then(|p| p.echo_probe.as_mut()).map(|p| p.tick(now)).unwrap_or_default();
            self.probe_outs(id, outs);
        }
        // A session that went away takes its waiting password with it.
        let gone: Vec<PaneId> = self.pwm.entries.keys().filter(|id| !self.panes.contains_key(id)).copied().collect();
        for id in gone {
            self.pwm.entries.remove(&id);
        }
        self.check_rounds();
        self.tick_password_input();
        self.panes.values().filter_map(|p| p.echo_probe.as_ref().and_then(Probe::due)).min()
    }

    /// Password prompt detection (`PTYSession.m:7657-7682`): the job watcher (local panes)
    /// and tandem-host (hosted panes) debounce real samples of each console; a session whose
    /// state changed goes to `password_input_changed`. A new sample wakes the window, so
    /// nothing waits here.
    fn tick_password_input(&mut self) {
        // The samplers run whatever the "Show key at cursor" setting says: the screen reader's
        // privacy gate (`Pane::secure_input`) needs the state, the setting only gates what the
        // key cursor and the manager do with it (`pwm_prompt::states`).
        let detect_on = self.globals.bool(pwm_prompt::DETECT_KEY);
        let debounce = pwm_prompt::sampling_debounce(detect_on, self.globals.number(pwm_prompt::DEBOUNCE_KEY));
        self.jobs.set_read_modes(debounce);
        if let Some(h) = &self.host {
            h.watch_prompts(debounce);
        }
        let mut changes = Vec::new();
        for (id, p) in self.panes.iter_mut() {
            let local = || p.owns_console() && p.shell_pid().is_some_and(|pid| self.jobs.password_input(pid));
            let raw = p.exited.is_none() && pwm_prompt::with_conductor(p.hosted_password_input().unwrap_or_else(local), p.ssh_password_prompt());
            let (now, secure) = pwm_prompt::states(raw, detect_on);
            p.secure_input = secure;
            if now != p.password_input {
                changes.push((*id, p.password_input, now));
                p.password_input = now;
            }
        }
        for (id, was, now) in changes {
            self.password_input_changed(id, was, now);
        }
    }

    /// A session reached or left a password prompt: the key cursor follows, and reaching one
    /// opens the manager by `pwm_prompt::opens_automatically` (`didBeginPasswordInput`).
    pub(super) fn password_input_changed(&mut self, pane: PaneId, was: bool, now: bool) {
        self.redraw();
        let Some(p) = self.panes.get(&pane) else { return };
        let profile_on = self.store.get(&p.profile).is_some_and(|pr| pr.bool(pwm_prompt::AUTO_OPEN_KEY));
        if pwm_prompt::opens_automatically(was, now, profile_on, p.replay.is_some()) {
            self.open_password_manager(None, Some(pane));
        }
    }

    /// The pointer came up on `pane`'s cursor cell while it is at a password prompt, with
    /// neither Alt nor Win down (`mouseHandlerAtPasswordPrompt:`, `mouseHandlerCursorCoord:`).
    pub(super) fn released_on_key_cursor(&self, pane: PaneId) -> bool {
        let (Some(p), Some(rect)) = (self.panes.get(&pane), self.pane_rect(pane)) else { return false };
        let (x, y) = self.cursor;
        if !rect.contains(x, y) {
            return false;
        }
        let (col, row, _) = self.cell_at(pane, rect, x, y);
        let point = self.grid_point(pane, col, row);
        let Ok(line) = usize::try_from(point.line.0) else { return false };
        let cursor = p.screen.term().grid().cursor.point;
        let cursor = (cursor.column.0, cursor.line.0.max(0) as usize);
        pwm_prompt::opens_on_click(p.at_password_prompt(), self.mods.alt, self.mods.cmd, (point.column.0, line), cursor)
    }

    /// The screen locked (`staticScreenDidLock:`, `instanceScreenDidLock:`).
    pub(super) fn pwm_screen_locked(&mut self) {
        let after = self.pwm_setting(pwm::REQUIRE_LOCK_KEY);
        self.pwm.screen_locked(after);
        if self.pwm_setting(pwm::REQUIRE_OPEN_KEY) && after {
            let here = self.current_window();
            for owner in self.pwm_owners() {
                if self.enter(owner) {
                    self.close_pwm();
                }
            }
            if let Some(h) = here {
                self.enter(h);
            }
        }
    }

    fn pwm_reply(&mut self, r: Reply) {
        // Store replies and ownerless prompts go first, never needing a manager window (review I5).
        let Some(Reply { owner: Some(owner), instance, token, busy, done, scope, .. }) = self.pwm.route_reply(r) else { return };
        let here = self.current_window();
        if here != Some(owner) && !self.enter(owner) {
            return; // The window closed: a prompt's sender drops, which answers Cancel.
        }
        let Some(w) = self.pwm_window.as_mut().filter(|w| w.scope == scope) else { return self.restore_window(here) };
        // A prompt is never under a scrim of its own; a stale one drops its sender (Cancel).
        let scrim = busy && !matches!(done, Done::Prompt { .. } | Done::Pick { .. });
        if !pwm::reply_current(Some(w.instance), instance, scrim, token, &w.busy) {
            return self.restore_window(here);
        }
        match done {
            Done::Prompt { heading, user, answer } => {
                let mut fields = Vec::new();
                if let Some(u) = user {
                    fields.push((field(PROMPT_USER, pwm::LABEL_USER_NAME, false), Secret::new(u)));
                }
                fields.push((field(PROMPT_PASSWORD, pwm::LABEL_PASSWORD, true), Secret::new(String::new())));
                let alert = Alert { serial: pwm::next_alert_serial(), heading, text: String::new(), secret: None, fields, buttons: vec![pwm::TITLE_OK.to_string(), pwm::TITLE_CANCEL.to_string()] };
                w.push_alert(alert, AlertFor::Prompt(answer));
            }
            Done::Pick { choices, answer } => {
                let alert = Alert {
                    serial: pwm::next_alert_serial(),
                    heading: pwm_cli::PICK_ACCOUNT_TITLE.to_string(),
                    text: pwm_cli::PICK_ACCOUNT_MESSAGE.to_string(),
                    secret: None,
                    fields: Vec::new(),
                    buttons: choices.iter().map(|(email, _)| email.clone()).collect(),
                };
                w.push_alert(alert, AlertFor::Pick(choices.into_iter().map(|(_, id)| id).collect(), answer));
            }
            done => {
                if busy {
                    w.busy.decr();
                }
                self.pwm_done(done);
            }
        }
        if let Some(w) = self.pwm_window.as_ref() {
            w.window.request_redraw();
        }
        self.restore_window(here);
    }

    fn restore_window(&mut self, here: Option<WindowId>) {
        if let Some(h) = here {
            if self.current_window() != Some(h) {
                self.enter(h);
            }
        }
    }

    fn pwm_done(&mut self, done: Done) {
        let kind = self.pwm_kind();
        match done {
            Done::Prompt { .. } | Done::Pick { .. } | Done::AuthLooked { .. } | Done::AuthSaved { .. } => {}
            Done::Listed { accounts, warning } => {
                // A browser manager over a page lists only the accounts for that page's exact origin.
                let (scope, target) = self.pwm_window.as_ref().map_or((Scope::Terminal, PwmTarget::Session), |w| (w.scope, w.target));
                let page = match target {
                    PwmTarget::BasicAuth(_) => self.pwm_window.as_ref().and_then(|w| w.pick_page.clone()),
                    PwmTarget::Session => self.pwm_window.as_ref().and_then(|w| w.page_pane).and_then(|p| self.page_bound_now(p)).and_then(|(url, epoch)| PwPage::new(&url, epoch)),
                };
                let result = match accounts {
                    Ok(v) => browser_listing(scope, target, page.as_ref(), pwm::sorted(v)),
                    Err(e) => {
                        self.pwm_source_alert(kind, &e);
                        Vec::new()
                    }
                };
                // As a side effect, the trigger's pop-up gets the names (`setCachedCombinedAccountNames:`).
                // (The terminal's only: the browser's names never reach a terminal trigger.)
                if scope == Scope::Terminal {
                    pwm_prompt::set_cached_names(result.iter().map(|a| a.display.clone()).collect());
                }
                let Some(w) = self.pwm_window.as_mut() else { return };
                let kept = w.selected_account().map(|a| a.id.clone());
                w.accounts = result;
                w.available = true;
                w.selected = kept.and_then(|id| w.accounts.iter().position(|a| a.id == id));
                if let Some(name) = w.select_name.take() {
                    // `reallySelectAccountName:`.
                    if let Some(i) = w.accounts.iter().position(|a| a.display == name) {
                        w.selected = Some(i);
                    }
                }
                if let Some((name, user)) = w.select_pair.take() {
                    if let Some(i) = w.accounts.iter().position(|a| a.account_name == name && a.user_name == user) {
                        w.selected = Some(i);
                    }
                }
                if let (Some(text), Some(id)) = (warning, adapter_identifier(kind)) {
                    let spec = adapter_warning_spec(id, &text);
                    let remembered = match self.warning_preempt(&spec) {
                        // OK, the only button, does nothing.
                        Preempt::Answer(_) => None,
                        Preempt::Show => Some(None),
                        Preempt::ShowRemembered { saved_label } => Some(Some(saved_label)),
                    };
                    if let Some(remembered) = remembered {
                        self.pwm.confirms.push(Confirm { window: None, heading: spec.heading.clone(), text, kind: ConfirmKind::Notice(spec), remembered });
                    }
                }
            }
            Done::Unavailable { error, choosing } => {
                self.forget_unusable_cli(kind);
                if let Some((a, f)) = unavailable_alert(kind, &error) {
                    if let Some(w) = self.pwm_window.as_mut() {
                        w.push_alert(a, f);
                    }
                }
                if choosing {
                    self.pwm_fall_back();
                } else {
                    self.pwm_unavailable();
                }
            }
            Done::Chosen => {
                if let Some(w) = self.pwm_window.as_mut() {
                    w.available = true;
                }
                self.pwm_job(Work::List, true);
            }
            Done::NeedCli { need, choosing } => {
                self.pwm_unavailable();
                let Some(w) = self.pwm_window.as_mut() else { return };
                match need {
                    NeedCli::Op(OpNotUsable::TooOld { path, .. }) => {
                        w.push_alert(Alert::message(pwm_cli::OP_UNAVAILABLE_TITLE, &pwm_cli::op_too_old_message(&path)), AlertFor::OpTooOld(choosing));
                    }
                    NeedCli::Op(OpNotUsable::NotFound) => w.push_alert(cannot_find_alert(Kind::OnePassword), AlertFor::CannotFind(Kind::OnePassword, choosing)),
                    NeedCli::Lp => w.push_alert(cannot_find_alert(Kind::LastPass), AlertFor::CannotFind(Kind::LastPass, choosing)),
                }
            }
            Done::Fetched(why, a, result) => self.pwm_fetched(kind, why, a, result),
            Done::Saved(result) => {
                self.pwm_passwords_changed(true);
                if let Err(e) = result {
                    self.pwm_save_failed(kind, &e);
                }
            }
            Done::Deleted(result) => {
                if result.is_ok() {
                    self.pwm_passwords_changed(true);
                }
            }
            Done::Added(result) => {
                if let Ok(a) = result {
                    if let Some(w) = self.pwm_window.as_mut() {
                        w.select_pair = Some((a.account_name, a.user_name));
                    }
                    self.pwm_job(Work::List, true);
                    self.pwm_passwords_changed(false);
                }
            }
            Done::Edited(result, pair) => {
                match &result {
                    Ok(Some(_)) => {
                        if let Some(w) = self.pwm_window.as_mut() {
                            w.select_pair = Some(pair);
                        }
                    }
                    Ok(None) => {}
                    Err(e) => self.pwm_save_failed(kind, e),
                }
                self.pwm_job(Work::List, true);
                self.pwm_passwords_changed(false);
            }
            Done::Toggled(result) => {
                if let (Ok(a), Some(w)) = (&result, self.pwm_window.as_mut()) {
                    w.select_pair = Some((a.account_name.clone(), a.user_name.clone()));
                }
                self.pwm_job(Work::Reload, true);
            }
            Done::Reset | Done::Switched => self.pwm_job(Work::Reload, true),
            Done::Command(name, result) => match result {
                Ok(message) => {
                    self.pwm_job(Work::Reload, true);
                    if let (Some(m), Some(w)) = (message.filter(|m| !m.is_empty()), self.pwm_window.as_mut()) {
                        w.push_alert(Alert::command_result(&name, &m), AlertFor::Plain);
                    }
                }
                Err(e) => {
                    if let Some(w) = self.pwm_window.as_mut() {
                        w.push_alert(Alert::command_failed(&e, ""), AlertFor::Plain);
                    }
                }
            },
        }
    }

    /// A remembered CLI that failed its check is forgotten, so the next choice searches (and
    /// offers Locate) again (`resetErrors`).
    fn forget_unusable_cli(&mut self, kind: Kind) {
        let key = match kind {
            Kind::OnePassword => OP_PATH_KEY,
            Kind::LastPass => LP_PATH_KEY,
            _ => return,
        };
        let path = self.globals.string(key);
        if !path.is_empty() && (kind == Kind::OnePassword || !crate::localfs::is_local_file(std::path::Path::new(&path))) {
            self.globals.values.remove(key);
            self.save_globals();
        }
    }

    /// The source's own alert for a failure, if it shows one.
    fn pwm_source_alert(&mut self, kind: Kind, error: &str) {
        if let (Some((a, f)), Some(w)) = (source_alert(kind, error), self.pwm_window.as_mut()) {
            w.push_alert(a, f);
        }
    }

    /// `presentEditError:`.
    fn pwm_save_failed(&mut self, kind: Kind, error: &str) {
        self.pwm_source_alert(kind, error);
        if let Some(w) = self.pwm_window.as_mut() {
            w.push_alert(Alert::save_failed(error), AlertFor::Plain);
        }
    }

    fn pwm_fetched(&mut self, kind: Kind, why: Why, a: Account, result: Result<(Secret, Option<Secret>), String>) {
        let (password, otp) = match result {
            Ok(v) => v,
            Err(e) => {
                if let Why::Prefill(serial) = why {
                    // Leave the field empty and editable: an untouched empty field changes nothing.
                    if let Some(p) = self.pwm_window.as_mut().and_then(|w| w.panel.as_mut()).filter(|p| p.serial == serial) {
                        p.fetched();
                    }
                    return;
                }
                self.pwm_source_alert(kind, &e);
                if let Some(w) = self.pwm_window.as_mut() {
                    w.push_alert(Alert::message(&pwm_source::password_failed(&e), ""), AlertFor::Plain);
                }
                return;
            }
        };
        // `fetchPasswordForRow:` hands on the code only for an account that sends it.
        let otp = otp.filter(|_| a.send_otp);
        match why {
            Why::Prefill(serial) => {
                let Some(w) = self.pwm_window.as_mut() else { return };
                let Some(p) = w.panel.as_mut().filter(|p| p.serial == serial && w.editing.is_some()) else { return };
                p.fetched();
                w.baseline = Some(password.duplicate());
                w.window.fill_password(serial, password);
            }
            Why::Reveal => {
                if let Some(w) = self.pwm_window.as_mut() {
                    w.push_alert(Alert::reveal(&pwm::row_name(&a), &password, otp.as_ref()), AlertFor::Reveal);
                }
            }
            Why::Copy => {
                let mut s = password.duplicate();
                if let Some(o) = &otp {
                    s.push_str(o.expose());
                }
                self.pwm_copy(&s);
            }
            Why::Enter(page) => {
                // `didFetchPasswordToEnter:otp:`: to the window's current session (or its
                // broadcast sessions), then the manager closes.
                let Some(w) = self.pwm_window.as_ref() else { return };
                let combined = pwm::combined(&password, &w.two_factor, otp.as_ref());
                if w.scope == Scope::Browser {
                    return self.enter_browser(page, &a, combined);
                }
                let broadcast = w.broadcast;
                if let Some(pane) = self.focused_id() {
                    if broadcast {
                        self.broadcast_password(pane, combined);
                    } else {
                        self.enter_password(pane, combined);
                    }
                }
                self.close_pwm();
            }
        }
    }

    /// What a pick of `a` is bound to: in the browser, the page showing now, which must be https and
    /// the one `a` is for (Task 5's `account_matches`); otherwise nothing. None (with the reason
    /// toasted) refuses the pick.
    fn pick_why(&mut self, a: &Account) -> Option<Why> {
        let w = self.pwm_window.as_ref()?;
        if w.scope == Scope::Terminal {
            return Some(Why::Enter(None));
        }
        // A picker's pick must be for the challenge's own origin (the list already hides the rest).
        if let PwmTarget::BasicAuth(_) = w.target {
            let page = w.pick_page.clone()?;
            if !bp::account_matches(&a.account_name, &page) {
                self.show_toast(bp::WRONG_SITE);
                return None;
            }
            return Some(Why::Enter(Some(page)));
        }
        let pane = w.page_pane?;
        let page = self.browser_pw_page(pane)?;
        if !bp::account_matches(&a.account_name, &page) {
            self.show_toast(bp::WRONG_SITE);
            return None;
        }
        Some(Why::Enter(Some(page)))
    }

    /// Enter User Name in the manager in front: a terminal session as before, a basic-auth picker records
    /// it for the password to follow, a page gets it through the bound writer.
    fn pwm_enter_user(&mut self, account: &str, user: &str, broadcast: bool) {
        let Some(w) = self.pwm_window.as_mut() else { return };
        let (scope, target, pane, close) = (w.scope, w.target, w.page_pane, w.send_user_by_default && !w.send_both);
        if scope == Scope::Terminal {
            if let Some(pane) = self.focused_id() {
                self.enter_username(pane, user, broadcast);
            }
            return;
        }
        match target {
            PwmTarget::BasicAuth(_) => w.pending_user = Some(user.to_string()),
            PwmTarget::Session => {
                let Some(pane) = pane else { return };
                let Some(page) = self.browser_pw_page(pane) else { return };
                if !bp::account_matches(account, &page) {
                    self.show_toast(bp::WRONG_SITE);
                    return;
                }
                self.browser_fill_username(pane, page, Secret::new(user.to_string()));
            }
        }
        if close {
            self.close_pwm();
        }
    }

    /// Enter Password in a browser manager (`didFetchPasswordToEnter:`): in send-both, the user name,
    /// then the password field's focus (only when the page has not navigated since), then the
    /// password, and the manager closes. A basic-auth picker finishes its challenge with (the user
    /// name that came first, or none) and the password.
    fn enter_browser(&mut self, page: Option<PwPage>, a: &Account, password: Secret) {
        let Some(w) = self.pwm_window.as_mut() else { return };
        match w.target {
            PwmTarget::BasicAuth(_) => {
                if w.send_both && !a.user_name.is_empty() {
                    w.pending_user = Some(a.user_name.clone());
                }
                let user = w.pending_user.take().unwrap_or_default();
                let challenge = take_challenge(&mut w.target);
                self.close_pwm();
                if let Some(n) = challenge {
                    self.basic_auth_picked(n, Some((user, password)));
                }
            }
            PwmTarget::Session => {
                let (Some(pane), Some(page)) = (w.page_pane, page) else { return };
                let steps = page_steps(w.send_both, &a.user_name, w.focus_after_user.as_ref(), page.epoch);
                let mut password = Some(password);
                for step in steps {
                    match step {
                        PageStep::User(u) => self.browser_fill_username(pane, page.clone(), Secret::new(u)),
                        PageStep::Focus(id) => self.browser_focus_password(pane, page.clone(), &id),
                        PageStep::Password => {
                            if let Some(p) = password.take() {
                                self.browser_fill_password(pane, page.clone(), p);
                            }
                        }
                    }
                }
                self.close_pwm();
            }
        }
    }

    /// `-[PTYSession enterPassword:]`: the echo probe first, with the profile's Backspace; focus
    /// reports held until it is done. A tmux gateway gets nothing. A browser pane never gets a
    /// password here: the page path binds the pick's page (`enter_browser`).
    pub(super) fn enter_password(&mut self, pane: PaneId, password: Secret) {
        if self.is_browser_pane(pane) {
            self.show_toast("This manager types into a terminal; open the browser's password manager for a page.");
            return;
        }
        if self.is_tmux_gateway(pane) || !self.panes.contains_key(&pane) {
            return;
        }
        self.end_probe_of(pane);
        let bs = self.backspace_data(pane);
        let (probe, outs) = Probe::begin(self.pwm_setting(pwm::PROBE_KEY), self.probe_duration(), &bs, Instant::now());
        if let Some(p) = self.panes.get_mut(&pane) {
            p.no_focus_reports += 1;
            p.echo_probe = Some(probe);
        }
        self.pwm.entry_ids += 1;
        self.pwm.entries.insert(pane, (self.pwm.entry_ids, password));
        self.probe_outs(pane, outs);
    }

    /// `broadcastPassword:` (`PT:15379-15420`, `BH:51-131`): every broadcast session that can
    /// backspace probes, with no password and never sending it; none probing enters it in each.
    pub(super) fn broadcast_password(&mut self, sender: PaneId, password: Secret) {
        let targets: Vec<PaneId> = self.broadcast.targets(&self.broadcast_view(), sender).into_iter().filter(|t| !self.is_tmux_gateway(*t) && !self.is_browser_pane(*t)).collect();
        let probing: Vec<(PaneId, Vec<u8>)> = targets.iter().map(|t| (*t, self.backspace_data(*t))).filter(|(_, bs)| !bs.is_empty()).collect();
        if probing.is_empty() {
            for t in targets {
                self.enter_password(t, password.duplicate());
            }
            return;
        }
        for (t, _) in &probing {
            self.end_probe_of(*t);
        }
        let panes: Vec<PaneId> = probing.iter().map(|(t, _)| *t).collect();
        self.pwm.rounds.push(BroadcastRound { round: Round::new(panes.clone()), panes, password });
        let (on, duration, now) = (self.pwm_setting(pwm::PROBE_KEY), self.probe_duration(), Instant::now());
        for (t, bs) in probing {
            let (probe, outs) = Probe::begin(on, duration, &bs, now);
            if let Some(p) = self.panes.get_mut(&t) {
                p.echo_probe = Some(probe);
            }
            self.probe_outs(t, outs);
        }
        self.check_rounds();
    }

    /// `-[PTYSession enterUsername:]`: the name and a newline, written as typing, so it is
    /// broadcast; with Broadcast on, once per broadcast session (kept: each write broadcasts
    /// again).
    pub(super) fn enter_username(&mut self, pane: PaneId, user: &str, broadcast: bool) {
        if self.is_browser_pane(pane) {
            self.show_toast("This manager types into a terminal; open the browser's password manager for a page.");
            return; // a page takes its user name through the bound writer
        }
        let text = format!("{user}\n");
        let targets = if broadcast { self.broadcast.targets(&self.broadcast_view(), pane) } else { vec![pane] };
        for t in targets {
            if !self.is_tmux_gateway(t) && !self.is_browser_pane(t) {
                self.user_write(t, text.as_bytes(), Write::SendText);
            }
        }
    }

    /// A new probe replaces a session's old one: its password is dropped and its round, whose
    /// delegate changed, ends without sending (`echoProbeDelegateWillChange:`).
    fn end_probe_of(&mut self, pane: PaneId) {
        if self.pwm.entries.remove(&pane).is_some() {
            if let Some(p) = self.panes.get_mut(&pane) {
                p.no_focus_reports = p.no_focus_reports.saturating_sub(1);
            }
        }
        self.pwm.rounds.retain(|r| !r.panes.contains(&pane));
        if let Some(p) = self.panes.get_mut(&pane).and_then(|p| p.echo_probe.as_mut()) {
            p.reset();
        }
    }

    /// What a probe asked for. A session entering its own password writes with the toast
    /// (`PS:22854-22866`: the probe's space too, kept; its backspace is data and shows none); a
    /// broadcast round's probe writes silently and reports instead of sending.
    fn probe_outs(&mut self, pane: PaneId, outs: Vec<Out>) {
        let single = self.pwm.entries.contains_key(&pane);
        for o in outs {
            match o {
                Out::Write(bytes) => {
                    if self.is_tmux_gateway(pane) {
                        continue;
                    }
                    self.user_write(pane, &bytes, Write::SendTextLocal);
                    if single && bytes == b" " {
                        self.password_toast(pane);
                    }
                }
                Out::SendPassword if single => self.send_entry(pane),
                Out::Failed if single => {
                    let id = self.pwm.entries.get(&pane).map_or(0, |(id, _)| *id);
                    self.pwm.confirms.push(Confirm { window: None, heading: WARNING_HEADING.to_string(), text: echo_probe::CHECK_TEXT.to_string(), kind: ConfirmKind::Check(pane, id), remembered: None });
                }
                Out::SendPassword | Out::Failed => {
                    let ok = matches!(o, Out::SendPassword);
                    for r in self.pwm.rounds.iter_mut().filter(|r| r.panes.contains(&pane)) {
                        r.round.report(pane, ok);
                    }
                }
            }
        }
    }

    /// "Password sent." (or "...press Return"), unless the session dropped the write: a
    /// replaying pane takes no input (`Pane::input`), so nothing is said.
    fn password_toast(&mut self, pane: PaneId) {
        if self.panes.get(&pane).is_none_or(|p| p.replay.is_some()) {
            return;
        }
        let text = if self.pwm_setting(pwm::SEND_RETURN_KEY) { echo_probe::SENT } else { echo_probe::SENT_PRESS_RETURN };
        self.show_toast(text);
    }

    /// `-[iTermEchoProbe enterPassword]` for a session's own entry.
    fn send_entry(&mut self, pane: PaneId) {
        let Some((_, password)) = self.pwm.entries.remove(&pane) else { return };
        self.write_password(pane, &password);
        if let Some(p) = self.panes.get_mut(&pane) {
            p.no_focus_reports = p.no_focus_reports.saturating_sub(1);
        }
    }

    /// The password (and a newline with Automatically Send Return) as the session's own write,
    /// never broadcast, with the toast. Nothing to a tmux gateway.
    fn write_password(&mut self, pane: PaneId, password: &Secret) {
        if self.is_tmux_gateway(pane) {
            return;
        }
        let bytes = echo_probe::password_bytes(password, self.pwm_setting(pwm::SEND_RETURN_KEY));
        self.user_write(pane, bytes.expose().as_bytes(), Write::SendTextLocal);
        self.password_toast(pane);
    }

    /// Finished rounds decide (`checkIfFinished`): a session gone counts as indeterminate.
    fn check_rounds(&mut self) {
        let mut i = 0;
        while i < self.pwm.rounds.len() {
            let r = &mut self.pwm.rounds[i];
            for p in r.panes.clone() {
                if !self.panes.contains_key(&p) {
                    r.round.vanished(p);
                }
            }
            if !r.round.done() {
                i += 1;
                continue;
            }
            let r = self.pwm.rounds.remove(i);
            for p in &r.panes {
                if let Some(pane) = self.panes.get_mut(p) {
                    pane.echo_probe = None;
                }
            }
            let names: Vec<String> = r.round.failed.iter().map(|p| self.session_name(*p)).collect();
            match echo_probe::finish(r.round.ok.clone(), names) {
                Finish::SendTo(ok) => {
                    for p in ok {
                        self.write_password(p, &r.password);
                    }
                }
                Finish::Ask { heading, text, ok, .. } => {
                    let kind = if ok.is_empty() { ConfirmKind::Warning } else { ConfirmKind::Broadcast(ok, r.password) };
                    self.pwm.confirms.push(Confirm { window: None, heading: heading.to_string(), text, kind, remembered: None });
                }
            }
        }
    }

    /// A session's name as the broadcast warning shows it (`presentationSessionTitle`).
    pub(super) fn session_name(&self, pane: PaneId) -> String {
        let Some(p) = self.panes.get(&pane) else { return String::new() };
        p.own().title().map(str::to_string).unwrap_or_else(|| self.store.get(&p.profile).map(|pr| pr.string("Name")).unwrap_or_default())
    }

    /// Events for the password manager's questions. False for other windows.
    pub(super) fn pwm_confirm_event(&mut self, id: WindowId, event: &WindowEvent) -> bool {
        let Some(i) = self.pwm.confirms.iter().position(|c| c.window.as_ref().is_some_and(|w| w.id() == id)) else { return false };
        // The autofill question: its window carries the one shared input guard (`guard_input` at
        // open; `ConfirmWindow::event` drops what it guards). Only a pointer click answers Fill;
        // the keyboard's one key is Escape (Cancel), itself held back while the guard waits.
        let autofill = matches!(self.pwm.confirms[i].kind, ConfirmKind::Autofill(..));
        let Some(w) = self.pwm.confirms[i].window.as_mut() else { return false };
        let answer = match event {
            WindowEvent::CloseRequested => ConfirmResult::Cancel,
            WindowEvent::RedrawRequested => w.redraw(),
            WindowEvent::KeyboardInput { event: k, .. } if autofill && autofill_escape(k) && !w.input_guarded(event) => ConfirmResult::Cancel,
            other => {
                if !(autofill && autofill_key_dropped(other)) {
                    w.event(other);
                }
                ConfirmResult::Open
            }
        };
        if !matches!(answer, ConfirmResult::Open) {
            let c = self.pwm.confirms.remove(i);
            if let (ConfirmKind::Notice(spec), ConfirmResult::Action { index, suppress }) = (&c.kind, &answer) {
                let spec = spec.clone();
                if self.warning_window_answered(&spec, c.remembered.is_some(), *index, *suppress) {
                    // Forgotten: the notice again, without the remembered mode.
                    self.pwm.confirms.push(Confirm { window: None, remembered: None, ..c });
                }
                return true;
            }
            self.pwm_confirmed(c.kind, answer);
        }
        true
    }

    /// Opens a page's autofill question for `pane` on the next pass. One at a time per pane (the
    /// caller checks `autofill_ask_open`).
    pub(super) fn pwm_ask_autofill(&mut self, pane: PaneId, text: String, ask: super::browser_autofill_app::AutofillAsk) {
        self.pwm.confirms.push(Confirm { window: None, heading: crate::browser_autofill::CONFIRM_HEADING.to_string(), text, kind: ConfirmKind::Autofill(pane, ask), remembered: None });
        self.redraw();
    }

    /// Whether `pane` has an autofill question open or waiting.
    pub(super) fn autofill_ask_open(&self, pane: PaneId) -> bool {
        self.pwm.confirms.iter().any(|c| matches!(&c.kind, ConfirmKind::Autofill(p, _) if *p == pane))
    }

    /// Drops `pane`'s autofill questions (closing their windows) whose page is gone: all of them with
    /// `all`, else those `still` (origin, epoch) does not accept. Nothing fills.
    pub(super) fn autofill_ask_drop(&mut self, pane: PaneId, all: bool, still: impl Fn(&str, u64) -> bool) {
        self.pwm.confirms.retain(|c| !matches!(&c.kind, ConfirmKind::Autofill(p, a) if *p == pane && (all || !still(&a.origin, a.epoch))));
    }

    /// The panes with an autofill question, to sweep.
    pub(super) fn autofill_ask_panes(&self) -> Vec<PaneId> {
        self.pwm.confirms.iter().filter_map(|c| match &c.kind { ConfirmKind::Autofill(p, _) => Some(*p), _ => None }).collect()
    }

    fn pwm_confirmed(&mut self, kind: ConfirmKind, answer: ConfirmResult) {
        let ok = matches!(answer, ConfirmResult::Ok { .. });
        match kind {
            // `sendPasswordAfterGettingPermission`.
            // A newer entry on the session since the question is not its to answer.
            ConfirmKind::Check(pane, id) if self.pwm.entries.get(&pane).is_none_or(|(e, _)| *e != id) => {}
            ConfirmKind::Check(pane, _) => {
                if ok {
                    self.send_entry(pane);
                } else {
                    self.end_probe_of(pane);
                }
            }
            ConfirmKind::Broadcast(sessions, password) => {
                if ok {
                    for p in sessions {
                        self.write_password(p, &password);
                    }
                }
            }
            ConfirmKind::Autofill(pane, ask) => {
                if ok {
                    self.autofill_confirmed(pane, ask);
                }
            }
            // A notice's answer is remembered in `pwm_confirm_event`; closed, nothing.
            ConfirmKind::Warning | ConfirmKind::Notice(_) => {}
        }
    }

    /// Open Terminal Window (`openSingleUseLoginWindowAndWrite:`): a new window with the login
    /// command typed and not sent; "Login Successful" when it reports success.
    fn open_lp_login(&mut self, event_loop: &ActiveEventLoop, email: &str) {
        if self.open_window(event_loop, None).is_err() {
            return;
        }
        let Some(pane) = self.tabs.get(self.active).map(|t| t.focused) else { return };
        self.user_write(pane, pwm_cli::lp_login_command(email).as_bytes(), Write::SendText);
        self.pwm.lp_watch = Some(pane);
    }

    fn watch_lp_login(&mut self, event_loop: &ActiveEventLoop) {
        let Some(pane) = self.pwm.lp_watch else { return };
        let Some(p) = self.panes.get(&pane) else {
            self.pwm.lp_watch = None;
            return;
        };
        let s = p.own();
        let rows = s.term().screen_lines() as i32;
        if !(0..rows).any(|l| pwm_cli::lp_logged_in(&s.line_text(l))) {
            return;
        }
        self.pwm.lp_watch = None;
        self.pwm_warning(pwm_cli::LP_LOGIN_SUCCESSFUL_TITLE, pwm_cli::LP_LOGIN_SUCCESSFUL_MESSAGE);
        self.close_pane(pane, event_loop);
    }
}

/// The autofill question's Escape press: its Cancel (a guarded window ignores Escape itself).
fn autofill_escape(k: &winit::event::KeyEvent) -> bool {
    k.state == winit::event::ElementState::Pressed && autofill_key_allowed(&k.logical_key)
}

/// Whether the autofill question lets a key through: Escape (Cancel) only. Tab never moves focus
/// to Fill and Space or Enter never press it, so only a pointer click answers Fill.
fn autofill_key_allowed(key: &winit::keyboard::Key) -> bool {
    matches!(key, winit::keyboard::Key::Named(winit::keyboard::NamedKey::Escape))
}

/// Whether the autofill question drops a keyboard or input-method event.
fn autofill_key_dropped(event: &WindowEvent) -> bool {
    match event {
        WindowEvent::KeyboardInput { event, .. } => !autofill_key_allowed(&event.logical_key),
        WindowEvent::Ime(_) => true,
        _ => false,
    }
}

/// An `iTermWarning` with no heading of its own (`iTermWarning.m:329`).
const WARNING_HEADING: &str = "Warning";

/// The broadcast warning's second button (`PT:15407`).
const BROADCAST_ENTER: &str = "Enter Password in Sessions at Prompt";

/// A field of a prompt from the worker.
fn field(key: &str, label: &str, secret: bool) -> Field {
    Field { key: key.to_string(), label: label.to_string(), placeholder: None, is_secret: secret, note: None, persist_in_keychain: false }
}

/// An adapter's warning from its account list (`AdapterPasswordDataSource.swift:622`,
/// `:688-696`): per adapter, OK only, permanently silenceable.
pub(super) fn adapter_warning_spec(id: &str, text: &str) -> crate::warnings::Spec {
    crate::warnings::Spec {
        id: Some(format!("NoSyncAdapterListWarning_{id}")),
        kind: crate::warnings::Kind::Permanent,
        heading: ADAPTER_WARNING_HEADING.to_string(),
        text: text.to_string(),
        labels: vec![crate::api_auth::OK.to_string()],
        cancel: None,
        never_remember: Vec::new(),
    }
}

/// The name an adapter keeps its settings under (iTerm2's adapter identifiers, `P:52-66`).
fn adapter_identifier(kind: Kind) -> Option<&'static str> {
    match kind {
        Kind::KeePassXC => Some("KeePassXC"),
        Kind::Bitwarden => Some("Bitwarden"),
        Kind::Keeper => Some("Keeper Security"),
        _ => None,
    }
}

/// Can't Find 1Password CLI (Locate, Cancel) or LastPass CLI (Locate, Cancel, Help).
fn cannot_find_alert(kind: Kind) -> Alert {
    let (heading, text, buttons) = if kind == Kind::OnePassword {
        (pwm_cli::OP_CANNOT_FIND_TITLE, pwm_cli::OP_CANNOT_FIND_MESSAGE, vec![pwm_cli::OP_LOCATE, pwm::TITLE_CANCEL])
    } else {
        (pwm_cli::LP_CANNOT_FIND_TITLE, pwm_cli::LP_CANNOT_FIND_MESSAGE, vec![pwm_cli::LP_LOCATE, pwm::TITLE_CANCEL, pwm_cli::LP_HELP])
    };
    Alert { buttons: buttons.into_iter().map(str::to_string).collect(), ..Alert::message(heading, text) }
}

/// LastPass's Authentication Failed (`LP:531-564`).
fn lp_auth_failed_alert(email: &str) -> (Alert, AlertFor) {
    let buttons = [pwm_cli::LP_OPEN_TERMINAL_WINDOW, pwm_cli::LP_COPY_COMMAND, pwm::TITLE_CANCEL].map(str::to_string).to_vec();
    (Alert { buttons, ..Alert::message(pwm_cli::LP_AUTH_FAILED_TITLE, &pwm_cli::lp_auth_failed_message(email)) }, AlertFor::LpAuthFailed(email.to_string()))
}

/// The alert a source shows for a failed call, if it shows one: 1Password's Timeout and
/// Authentication Error, LastPass's Authentication Failed and Timeout, an adapter's error.
/// Credential Manager shows none, and a canceled prompt never does.
fn source_alert(kind: Kind, error: &str) -> Option<(Alert, AlertFor)> {
    if error == CANCELED {
        return None;
    }
    let plain = |heading: &str| Some((Alert::message(heading, error), AlertFor::Plain));
    match kind {
        Kind::CredentialManager => None,
        Kind::OnePassword if error == pwm_cli::OP_TIMEOUT_MESSAGE => plain(pwm_cli::OP_TIMEOUT_TITLE),
        Kind::OnePassword if error == pwm_cli::OP_SIGNIN_FAILED_MESSAGE || error == pwm_cli::OP_GARBLED_TOKEN_MESSAGE => plain(pwm_cli::OP_AUTH_ERROR_TITLE),
        Kind::OnePassword => None,
        Kind::LastPass => match pwm_cli::lp_auth_failed_email(error) {
            Some(email) => Some(lp_auth_failed_alert(email)),
            None if error.starts_with(&pwm_cli::lp_timeout_message("")) => plain(pwm_cli::LP_TIMEOUT_TITLE),
            None => None,
        },
        Kind::KeePassXC | Kind::Bitwarden | Kind::Keeper => plain(ADAPTER_ERROR_HEADING),
    }
}

/// The alert for a source that failed its availability check: 1Password's "OnePassword
/// Unavailable", LastPass's own alerts, an adapter's "Adapter not found." (m10b ruling: the
/// adapters are not shipped, so choosing one says so before falling back).
fn unavailable_alert(kind: Kind, error: &str) -> Option<(Alert, AlertFor)> {
    match kind {
        Kind::OnePassword if error != CANCELED => Some((Alert::message(pwm_cli::OP_UNAVAILABLE_TITLE, error), AlertFor::Plain)),
        _ => source_alert(kind, error),
    }
}

/// What the sources answer for a prompt the user canceled.
const CANCELED: &str = "Canceled.";

/// Locate: a file picker for `op.exe` or `lpass.exe`; a file with another name is refused
/// (`OT:160-176`, `LP:584-600`).
fn locate_cli(kind: Kind) -> Option<String> {
    let exe = if kind == Kind::OnePassword { "op" } else { "lpass" };
    let path = rfd::FileDialog::new().add_filter(exe, &["exe"]).pick_file()?;
    let stem_ok = path.file_stem().is_some_and(|s| s.eq_ignore_ascii_case(exe));
    (stem_ok && crate::localfs::is_local_file(&path)).then(|| path.to_string_lossy().into_owned())
}

/// A uniform random number below `bound` (`arc4random_uniform`), from the system's random
/// source (a version 4 UUID's first four bytes are random), without modulo bias.
fn random_below(bound: u32) -> u32 {
    if bound <= 1 {
        return 0;
    }
    let limit = u32::MAX - u32::MAX % bound;
    loop {
        let b = uuid::Uuid::new_v4().into_bytes();
        let r = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
        if r < limit {
            return r % bound;
        }
    }
}

// --- The worker thread ---

/// What the worker builds the source from; a change rebuilds it (a new sign-in).
#[derive(Clone)]
struct Config {
    kind: Kind,
    /// The remembered CLI for 1Password or LastPass, else empty (searched for).
    cli: String,
    groups: String,
    /// `onePasswordAccount`: a change in Settings rebuilds the source; the picker's own choice
    /// is written back into the built source's config, so it does not.
    account: String,
    adapter: AdapterConfig,
    /// Which password manager the source serves: the terminal's, or the browser's own store.
    scope: Scope,
}

#[derive(Clone, Default, PartialEq)]
struct AdapterConfig {
    db: Option<String>,
    exe: Option<String>,
    /// Settings as (key, value); secret ones included, for the run only.
    settings: Vec<(String, String)>,
}

impl Config {
    fn same_source(&self, o: &Config) -> bool {
        self.kind == o.kind && self.cli == o.cli && self.groups == o.groups && self.account == o.account && self.adapter == o.adapter && self.scope == o.scope
    }
}

/// A request for the worker: a manager window's, or a sign-in prompt's store job, which has no window.
enum Job {
    Window(WindowJob),
    #[allow(dead_code)]
    Auth(AuthJob),
}

struct WindowJob {
    owner: WindowId,
    instance: u64,
    token: u32,
    busy: bool,
    config: Config,
    work: Work,
}

/// A basic-auth store job (13x7 Task 9): owner-less, no instance and no token; its reply is routed by
/// the challenge, never by a manager window (review I5).
struct AuthJob {
    config: Config,
    work: AuthWork,
}

/// What the sign-in prompt asks of the store (`BAS:100-190`).
pub(super) enum AuthWork {
    /// The saved credential for the account name: it only prefills the prompt (R13x78-1).
    Lookup { pane: PaneId, id: u64, name: String, preferred_user: Option<String> },
    /// Remember a credential the server accepted.
    Save { pane: PaneId, space: SpaceKey, name: String, user: String, password: Secret },
}

/// A store job's answer, queued for the coordinator's owner (Task 10).
pub(super) enum StoreReply {
    Looked { pane: PaneId, id: u64, found: Option<(String, Secret)> },
    Saved { pane: PaneId, space: SpaceKey, host: String, success: bool },
}

enum Why {
    /// Enter Password; in the browser, the page the pick was made on (bound before the fetch).
    Enter(Option<PwPage>),
    Reveal,
    Copy,
    /// The Edit panel with this serial.
    Prefill(u64),
}

enum Work {
    /// The availability check, then the accounts (opening, `update` and `updateConfiguration`).
    Load,
    /// A newly chosen source's availability check.
    Choose,
    List,
    /// `reloadItems:`: the source drops its caches, then the accounts.
    Reload,
    Fetch(Account, Why),
    SetPassword(Account, Secret),
    Delete(Account),
    Add { name: String, user: String, password: Secret, flags: Vec<(String, bool)> },
    EditInPlace { account: Account, name: Option<String>, user: Option<String>, password: Option<Secret>, pair: (String, String) },
    /// Rename by delete then add, with the current password unless a new one was typed.
    Readd { account: Account, name: String, user: String, password: Option<Secret>, flags: Vec<(String, bool)> },
    ToggleOtp(Account),
    Reset,
    Switch,
    Command(String),
}

enum NeedCli {
    Op(OpNotUsable),
    Lp,
}

enum Done {
    Listed { accounts: Result<Vec<Account>, String>, warning: Option<String> },
    Unavailable { error: String, choosing: bool },
    Chosen,
    NeedCli { need: NeedCli, choosing: bool },
    Fetched(Why, Account, Result<(Secret, Option<Secret>), String>),
    Saved(Result<(), String>),
    Deleted(Result<(), String>),
    Added(Result<Account, String>),
    /// An edit: the renamed account when it was re-added, and the (account, user) it now has.
    Edited(Result<Option<Account>, String>, (String, String)),
    Toggled(Result<Account, String>),
    Reset,
    Switched,
    Command(String, Result<Option<String>, String>),
    /// A master password or login prompt; the worker waits for the answer.
    Prompt { heading: String, user: Option<String>, answer: Sender<Option<Vec<(String, Secret)>>> },
    /// The 1Password account picker, as (email, account id); the worker waits.
    Pick { choices: Vec<(String, String)>, answer: Sender<Option<String>> },
    /// A basic-auth lookup's result (no window).
    AuthLooked { pane: PaneId, id: u64, found: Option<(String, Secret)> },
    /// A basic-auth save's result (no window).
    AuthSaved { pane: PaneId, space: SpaceKey, host: String, success: bool },
}

struct Reply {
    /// The store whose worker sent it.
    scope: Scope,
    /// None for a basic-auth store reply: there is no manager window to show it in.
    owner: Option<WindowId>,
    instance: u64,
    token: u32,
    busy: bool,
    info: Option<(Kind, String, Caps)>,
    done: Done,
}

struct Worker {
    tx: Sender<Job>,
    rx: Receiver<Reply>,
}

/// What a prompt from inside a source needs to reach the window that asked.
#[derive(Clone)]
struct Ctx {
    scope: Scope,
    out: Sender<Reply>,
    notify: Notify,
    /// The window and manager instance of the request being run.
    current: Arc<Mutex<Option<(WindowId, u64)>>>,
    /// An account the 1Password picker chose during the request.
    picked: Arc<Mutex<Option<String>>>,
}

impl Ctx {
    fn send(&self, done: Done) -> bool {
        let Some((owner, instance)) = *self.current.lock().unwrap() else { return false };
        let sent = self.out.send(Reply { scope: self.scope, owner: Some(owner), instance, token: 0, busy: false, info: None, done }).is_ok();
        (self.notify)();
        sent
    }

    fn ask(&self, heading: &str, user: Option<&str>) -> Option<Vec<(String, Secret)>> {
        let (tx, rx) = mpsc::channel();
        if !self.send(Done::Prompt { heading: heading.to_string(), user: user.map(str::to_string), answer: tx }) {
            return None;
        }
        rx.recv().ok().flatten()
    }

    fn master_prompt(&self) -> Box<dyn FnMut(&str) -> Option<Secret> + Send> {
        let ctx = self.clone();
        Box::new(move |text| ctx.ask(text, None)?.into_iter().find(|(k, _)| k == PROMPT_PASSWORD).map(|(_, v)| v))
    }

    /// Answers `lpass`'s mid-command password asks one at a time (a second never opens a second
    /// window) through the same secret prompt as every master password, naming LastPass. The
    /// thread ends when the source (the only sender) is dropped.
    fn lastpass_asker(&self) -> Sender<crate::pwm_run::Ask> {
        let (ask, asks) = mpsc::channel::<crate::pwm_run::Ask>();
        let mut prompt = self.master_prompt();
        let _ = std::thread::Builder::new().name("tandem-lpass-ask".into()).spawn(move || {
            for ask in asks {
                // An ask whose request already ended (timed out, canceled or the command
                // failed) is skipped: nobody is waiting, and a stale prompt would teach the
                // person to type the master password into dialogs that lead nowhere.
                if ask.ended.load(std::sync::atomic::Ordering::SeqCst) {
                    let _ = ask.reply.send(None);
                    continue;
                }
                let _ = ask.reply.send(prompt(pwm_cli::LP_ASK));
            }
        });
        ask
    }

    fn login_prompt(&self) -> pwm_cli::LoginPrompt {
        let ctx = self.clone();
        Box::new(move |text, user| {
            let values = ctx.ask(text, Some(user))?;
            let user = values.iter().find(|(k, _)| k == PROMPT_USER).map(|(_, v)| v.expose().to_string()).unwrap_or_default();
            let password = values.into_iter().find(|(k, _)| k == PROMPT_PASSWORD).map(|(_, v)| v)?;
            Some((user, password))
        })
    }

    fn picker(&self) -> pwm_cli::AccountPicker {
        let ctx = self.clone();
        Box::new(move |choices| {
            let (tx, rx) = mpsc::channel();
            if !ctx.send(Done::Pick { choices: choices.to_vec(), answer: tx }) {
                return None;
            }
            let id = rx.recv().ok().flatten();
            if let Some(id) = &id {
                *ctx.picked.lock().unwrap() = Some(id.clone());
            }
            id
        })
    }
}

impl Worker {
    /// The worker thread: it owns the source and runs one request at a time, in order.
    fn start(notify: Notify, scope: Scope) -> Worker {
        let (tx, jobs) = mpsc::channel::<Job>();
        let (out, rx) = mpsc::channel::<Reply>();
        let _ = std::thread::Builder::new().name("tandem-pwm".into()).spawn(move || {
            let ctx = Ctx { scope, out: out.clone(), notify: notify.clone(), current: Arc::new(Mutex::new(None)), picked: Arc::new(Mutex::new(None)) };
            let mut slot: Option<(Config, Box<dyn Source>)> = None;
            for job in jobs {
                let job = match job {
                    Job::Window(j) => j,
                    Job::Auth(j) => {
                        // No window: a source that needs a master password cannot ask (its prompt
                        // finds no window and answers Cancel), so a locked store fails the job.
                        *ctx.current.lock().unwrap() = None;
                        let done = run_auth(&mut slot, j, &ctx);
                        let info = slot.as_ref().map(|(_, s)| (s.kind(), s.name(), s.caps()));
                        if out.send(Reply { scope, owner: None, instance: 0, token: 0, busy: false, info, done }).is_err() {
                            break;
                        }
                        notify();
                        continue;
                    }
                };
                *ctx.current.lock().unwrap() = Some((job.owner, job.instance));
                let (owner, instance, token, busy) = (job.owner, job.instance, job.token, job.busy);
                let done = run(&mut slot, job, &ctx);
                let info = slot.as_ref().map(|(_, s)| (s.kind(), s.name(), s.caps()));
                // The picked account is the one the built source now uses.
                if let (Some(a), Some((c, _))) = (ctx.picked.lock().unwrap().take(), slot.as_mut()) {
                    c.account = a;
                }
                if out.send(Reply { scope, owner: Some(owner), instance, token, busy, info, done }).is_err() {
                    break;
                }
                notify();
            }
        });
        Worker { tx, rx }
    }
}

/// Builds the source for `config`. A CLI that cannot be found (or is too old) comes back for
/// the window to offer Locate.
fn build(config: &Config, ctx: &Ctx) -> Result<Box<dyn Source>, NeedCli> {
    use crate::pwm_run::ProcessRunner;
    match config.kind {
        Kind::CredentialManager => Ok(Box::new(crate::pwm_credman::CredentialManager::for_scope(config.scope))),
        Kind::OnePassword => {
            let mut runner = ProcessRunner::new();
            let path = if config.cli.is_empty() {
                let env = |k: &str| std::env::var(k).unwrap_or_default();
                let candidates = pwm_cli::op_paths(&env("PATH"), &env("LOCALAPPDATA"));
                pwm_cli::resolve_op(&mut runner, &candidates, &mut |p| crate::localfs::is_local_file(std::path::Path::new(p))).map_err(NeedCli::Op)?
            } else {
                config.cli.clone()
            };
            let account = Some(config.account.clone()).filter(|a| !a.is_empty());
            Ok(Box::new(pwm_cli::OnePassword::new(runner, path, account, ctx.master_prompt()).with_picker(ctx.picker()).with_scope(config.scope)))
        }
        Kind::LastPass => {
            let path = if config.cli.is_empty() { pwm_cli::resolve_lpass().map_err(|_| NeedCli::Lp)? } else { config.cli.clone() };
            Ok(Box::new(pwm_cli::LastPass::new(ProcessRunner::new(), path, pwm_cli::lpass_groups(&config.groups), ctx.login_prompt()).with_asker(ctx.lastpass_asker()).with_scope(config.scope)))
        }
        Kind::KeePassXC | Kind::Bitwarden | Kind::Keeper => {
            let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(std::path::Path::to_path_buf)).unwrap_or_default();
            let path = crate::pwm_adapter::adapter_path(config.kind, &exe_dir).unwrap_or_default();
            let a = &config.adapter;
            let settings = a.settings.iter().cloned().collect();
            Ok(Box::new(crate::pwm_adapter::Adapter::new(ProcessRunner::new(), config.kind, path, a.db.clone(), a.exe.clone(), settings, None, ctx.master_prompt()).with_scope(config.scope)))
        }
    }
}

fn list(src: &mut dyn Source) -> Done {
    let accounts = src.accounts();
    Done::Listed { accounts, warning: src.take_warning() }
}

/// A basic-auth store job: the source built (or reused), then `auth_job` on it. A source that cannot be
/// built answers as a miss or a failed save.
fn run_auth(slot: &mut Option<(Config, Box<dyn Source>)>, job: AuthJob, ctx: &Ctx) -> Done {
    if slot.as_ref().is_none_or(|(c, _)| !c.same_source(&job.config)) {
        *slot = None;
        if let Ok(s) = build(&job.config, ctx) {
            *slot = Some((job.config.clone(), s));
        }
    }
    match slot.as_mut() {
        Some((_, src)) => auth_job(src.as_mut(), job.work),
        None => auth_failed(job.work),
    }
}

fn auth_failed(work: AuthWork) -> Done {
    match work {
        AuthWork::Lookup { pane, id, .. } => Done::AuthLooked { pane, id, found: None },
        AuthWork::Save { pane, space, .. } => {
            let host = space.host.clone();
            Done::AuthSaved { pane, space, host, success: false }
        }
    }
}

/// `BAS:100-190` over a source: the first account named for the space (the proposed user preferred) and
/// its password; a save sets the password of an existing (name, user) account, else adds with no flags.
/// Any store error is a miss or a failed save, never a thrown prompt.
fn auth_job(src: &mut dyn Source, work: AuthWork) -> Done {
    use crate::browser_basic_auth::{pick_saved, save_plan, SavePlan};
    match work {
        AuthWork::Lookup { pane, id, name, preferred_user } => {
            let found = src.accounts().ok().and_then(|accounts| {
                let a = pick_saved(&accounts, &name, preferred_user.as_deref())?;
                let (password, _) = src.password(a).ok()?;
                Some((a.user_name.clone(), password))
            });
            Done::AuthLooked { pane, id, found }
        }
        AuthWork::Save { pane, space, name, user, password } => {
            let host = space.host.clone();
            let success = match src.accounts() {
                Err(_) => false,
                Ok(accounts) => match save_plan(&accounts, &name, &user) {
                    SavePlan::SetPassword(a) => src.set_password(&a, password).is_ok(),
                    SavePlan::Add => src.add(&name, &user, password, &[]).is_ok(),
                },
            };
            Done::AuthSaved { pane, space, host, success }
        }
    }
}

/// Runs one request against the source, building it first when the configuration changed.
fn run(slot: &mut Option<(Config, Box<dyn Source>)>, job: WindowJob, ctx: &Ctx) -> Done {
    let choosing = matches!(job.work, Work::Choose);
    if slot.as_ref().is_none_or(|(c, _)| !c.same_source(&job.config)) {
        *slot = None;
        match build(&job.config, ctx) {
            Ok(s) => *slot = Some((job.config.clone(), s)),
            Err(need) => return Done::NeedCli { need, choosing },
        }
    }
    let Some((_, src)) = slot.as_mut() else { return Done::Unavailable { error: String::new(), choosing } };
    let src = src.as_mut();
    match job.work {
        Work::Load => match src.available() {
            Ok(()) => list(src),
            Err(error) => Done::Unavailable { error, choosing: false },
        },
        Work::Choose => match src.available() {
            Ok(()) => Done::Chosen,
            Err(error) => Done::Unavailable { error, choosing: true },
        },
        Work::List => list(src),
        Work::Reload => {
            src.reload();
            list(src)
        }
        Work::Fetch(a, why) => {
            let r = src.password(&a);
            Done::Fetched(why, a, r)
        }
        Work::SetPassword(a, p) => Done::Saved(src.set_password(&a, p)),
        Work::Delete(a) => Done::Deleted(src.delete(&a)),
        Work::Add { name, user, password, flags } => Done::Added(src.add(&name, &user, password, &flags)),
        Work::EditInPlace { account, name, user, password, pair } => Done::Edited(src.edit_in_place(&account, name.as_deref(), user.as_deref(), password).map(|_| None), pair),
        Work::Readd { account, name, user, password, flags } => {
            let pair = (name.clone(), user.clone());
            // `renameEntry:`'s fallback: the current password unless one was typed (a fetch
            // error then does not matter), delete, then add.
            let password = match password {
                Some(p) => p,
                None => match src.password(&account) {
                    Ok((p, _)) => p,
                    Err(e) => return Done::Edited(Err(e), pair),
                },
            };
            if let Err(e) = src.delete(&account) {
                return Done::Edited(Err(e), pair);
            }
            Done::Edited(src.add(&name, &user, password, &flags).map(Some), pair)
        }
        Work::ToggleOtp(a) => Done::Toggled(src.toggle_otp(&a)),
        Work::Reset => {
            src.reset_configuration();
            Done::Reset
        }
        Work::Switch => {
            let _ = src.switch_account();
            Done::Switched
        }
        Work::Command(name) => {
            let r = src.custom_command(&name);
            Done::Command(name, r)
        }
    }
}

// --- The screen lock ---

/// `WTS_SESSION_LOCK` through `WTSRegisterSessionNotification` on a terminal window (iTerm2's
/// `com.apple.screenIsLocked`), seen by a window subclass and handled on the next pass.
mod lock {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Mutex, OnceLock};

    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows_sys::Win32::System::RemoteDesktop::{WTSRegisterSessionNotification, WTSUnRegisterSessionNotification, NOTIFY_FOR_THIS_SESSION};
    use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
    use windows_sys::Win32::UI::WindowsAndMessaging::{WM_NCDESTROY, WM_WTSSESSION_CHANGE};

    use crate::pty::Notify;

    const SUBCLASS_ID: usize = 0x7077_6D6C;
    const WTS_SESSION_LOCK: usize = 7;

    static LOCKED: AtomicBool = AtomicBool::new(false);
    static WAKE: OnceLock<Notify> = OnceLock::new();
    static WINDOWS: Mutex<Vec<isize>> = Mutex::new(Vec::new());

    /// Watches `hwnd` for the screen locking, once per window.
    pub(super) fn watch(hwnd: isize, wake: Notify) {
        let _ = WAKE.set(wake);
        let Ok(mut w) = WINDOWS.lock() else { return };
        if w.contains(&hwnd) {
            return;
        }
        let ok = unsafe { WTSRegisterSessionNotification(hwnd as HWND, NOTIFY_FOR_THIS_SESSION) } != 0;
        if ok && unsafe { SetWindowSubclass(hwnd as HWND, Some(proc), SUBCLASS_ID, 0) } != 0 {
            w.push(hwnd);
        }
    }

    /// How many windows are watched.
    pub(super) fn watched() -> usize {
        WINDOWS.lock().map_or(0, |w| w.len())
    }

    /// Whether the screen locked since the last call.
    pub(super) fn take() -> bool {
        LOCKED.swap(false, Ordering::AcqRel)
    }

    unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM, _id: usize, _data: usize) -> LRESULT {
        if msg == WM_WTSSESSION_CHANGE && wparam == WTS_SESSION_LOCK {
            LOCKED.store(true, Ordering::Release);
            if let Some(w) = WAKE.get() {
                w();
            }
        } else if msg == WM_NCDESTROY {
            unsafe {
                WTSUnRegisterSessionNotification(hwnd);
                RemoveWindowSubclass(hwnd, Some(proc), SUBCLASS_ID);
            }
            if let Ok(mut w) = WINDOWS.lock() {
                w.retain(|h| *h != hwnd as isize);
            }
        }
        unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
    }
}

// --- Authentication ---

/// Windows Hello through `UserConsentVerifier` (for this window, `IUserConsentVerifierInterop`),
/// or the Windows credential prompt for the signed-in user, checked with `LogonUserW`. The typed
/// password is unpacked into a stack buffer, used once and wiped with the packed buffer.
mod win_auth {
    use std::sync::atomic::{compiler_fence, Ordering};

    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, HANDLE};
    use windows_sys::Win32::Security::Credentials::{CredUIPromptForWindowsCredentialsW, CredUnPackAuthenticationBufferW, CREDUIWIN_ENUMERATE_CURRENT_USER, CREDUI_INFOW, CRED_PACK_PROTECTED_CREDENTIALS};
    use windows_sys::Win32::Security::{LogonUserW, LOGON32_LOGON_INTERACTIVE, LOGON32_PROVIDER_DEFAULT};
    use windows_sys::Win32::System::Com::CoTaskMemFree;

    pub(super) const CAPTION: &str = "Tandem";
    pub(super) const MESSAGE: &str = "Tandem wants to open the password manager.";
    const ERROR_CANCELLED: u32 = 1223;
    const ERROR_LOGON_FAILURE: u32 = 1326;

    pub(super) enum Answer {
        Granted,
        Canceled,
        /// The reason, finishing "Authentication failed because ".
        Failed(String),
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    fn wipe(b: &mut [u16]) {
        for c in b.iter_mut() {
            unsafe { std::ptr::write_volatile(c, 0) };
        }
        compiler_fence(Ordering::SeqCst);
    }

    /// iTerm2's reasons (`P:320-359`) where Windows has the same case, else Windows' own text.
    pub(super) fn reason(code: u32) -> String {
        match code {
            ERROR_LOGON_FAILURE => "valid credentials weren't supplied.".to_string(),
            ERROR_CANCELLED => "password entry was cancelled.".to_string(),
            _ => format!("{}.", std::io::Error::from_raw_os_error(code as i32).to_string().trim_end_matches(['.', ' ', '\r', '\n'])),
        }
    }

    /// "DOMAIN\user" in the user field splits into its parts when the domain came back empty;
    /// "user@domain" stays whole for `LogonUserW` with no domain.
    pub(super) fn split_user(user: &[u16], domain: &[u16]) -> (Vec<u16>, Option<Vec<u16>>) {
        let user = &user[..user.iter().position(|c| *c == 0).unwrap_or(user.len())];
        let domain = &domain[..domain.iter().position(|c| *c == 0).unwrap_or(domain.len())];
        let z = |s: &[u16]| s.iter().copied().chain(Some(0)).collect::<Vec<u16>>();
        if !domain.is_empty() {
            return (z(user), Some(z(domain)));
        }
        match user.iter().position(|c| *c == u16::from(b'\\')) {
            Some(i) => (z(&user[i + 1..]), Some(z(&user[..i]))),
            None => (z(user), None),
        }
    }

    /// Windows Hello's answer (`UserConsentVerificationResult`): None where the device or its
    /// setup is missing, so the password prompt is used instead.
    pub(super) fn hello_answer(code: i32) -> Option<Answer> {
        Some(match code {
            0 => Answer::Granted,
            6 => Answer::Canceled,
            1 | 2 => return None,
            3 => Answer::Failed("authentication is disabled by policy.".to_string()),
            4 => Answer::Failed("the authentication device is busy.".to_string()),
            5 => Answer::Failed("there were too many failed attempts.".to_string()),
            _ => Answer::Failed("Windows Hello reported an error.".to_string()),
        })
    }

    /// Waits for a Windows Runtime operation, handling this thread's messages meanwhile, so
    /// the Hello dialog works. Tandem's own windows are frozen until it returns: winit holds
    /// every event while its handler is on the stack, so they neither repaint nor take input
    /// (as with the credential prompt and the file pickers; accepted, checklist "modal calls").
    fn wait<T: windows::core::RuntimeType + 'static>(op: &windows_future::IAsyncOperation<T>) -> Option<T> {
        use windows_sys::Win32::UI::WindowsAndMessaging::{DispatchMessageW, MsgWaitForMultipleObjects, PeekMessageW, TranslateMessage, MSG, PM_REMOVE, QS_ALLINPUT};
        while op.Status().ok()? == windows_future::AsyncStatus::Started {
            let mut msg: MSG = unsafe { std::mem::zeroed() };
            if unsafe { PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) } != 0 {
                unsafe {
                    TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            } else {
                unsafe { MsgWaitForMultipleObjects(0, std::ptr::null(), 0, 50, QS_ALLINPUT) };
            }
        }
        op.GetResults().ok()
    }

    /// Windows Hello over `owner`; None when it is not available (or could not be asked).
    pub(super) fn hello(owner: isize) -> Option<Answer> {
        use windows::Security::Credentials::UI::{UserConsentVerificationResult, UserConsentVerifier, UserConsentVerifierAvailability};
        use windows::Win32::System::WinRT::IUserConsentVerifierInterop;
        let available = wait(&UserConsentVerifier::CheckAvailabilityAsync().ok()?)?;
        if available != UserConsentVerifierAvailability::Available {
            return None;
        }
        let interop = windows::core::factory::<UserConsentVerifier, IUserConsentVerifierInterop>().ok()?;
        let hwnd = windows::Win32::Foundation::HWND(owner as _);
        let op: windows_future::IAsyncOperation<UserConsentVerificationResult> = unsafe { interop.RequestVerificationForWindowAsync(hwnd, &windows::core::HSTRING::from(MESSAGE)) }.ok()?;
        hello_answer(wait(&op)?.0)
    }

    pub(super) fn prompt(owner: isize) -> Answer {
        let caption = wide(CAPTION);
        let message = wide(MESSAGE);
        let info = CREDUI_INFOW {
            cbSize: std::mem::size_of::<CREDUI_INFOW>() as u32,
            hwndParent: owner as _,
            pszMessageText: message.as_ptr(),
            pszCaptionText: caption.as_ptr(),
            hbmBanner: std::ptr::null_mut(),
        };
        let mut package = 0u32;
        let mut out: *mut core::ffi::c_void = std::ptr::null_mut();
        let mut out_len = 0u32;
        let mut save = 0;
        let r = unsafe { CredUIPromptForWindowsCredentialsW(&info, 0, &mut package, std::ptr::null(), 0, &mut out, &mut out_len, &mut save, CREDUIWIN_ENUMERATE_CURRENT_USER) };
        if r == ERROR_CANCELLED {
            return Answer::Canceled;
        }
        if r != 0 {
            return Answer::Failed(reason(r));
        }
        let mut user = [0u16; 514];
        let mut domain = [0u16; 338];
        let mut password = [0u16; 514];
        let (mut ul, mut dl, mut pl) = (user.len() as u32, domain.len() as u32, password.len() as u32);
        let unpacked = unsafe { CredUnPackAuthenticationBufferW(CRED_PACK_PROTECTED_CREDENTIALS, out, out_len, user.as_mut_ptr(), &mut ul, domain.as_mut_ptr(), &mut dl, password.as_mut_ptr(), &mut pl) } != 0;
        let unpack_error = unsafe { GetLastError() };
        unsafe {
            let bytes = std::slice::from_raw_parts_mut(out as *mut u8, out_len as usize);
            for b in bytes.iter_mut() {
                std::ptr::write_volatile(b, 0);
            }
            compiler_fence(Ordering::SeqCst);
            CoTaskMemFree(out);
        }
        let answer = if !unpacked {
            Answer::Failed(reason(unpack_error))
        } else {
            let (mut u, mut d) = split_user(&user, &domain);
            let mut token: HANDLE = std::ptr::null_mut();
            let domain_ptr = d.as_ref().map_or(std::ptr::null(), |d| d.as_ptr());
            let ok = unsafe { LogonUserW(u.as_ptr(), domain_ptr, password.as_ptr(), LOGON32_LOGON_INTERACTIVE, LOGON32_PROVIDER_DEFAULT, &mut token) } != 0;
            let error = unsafe { GetLastError() };
            wipe(&mut u);
            if let Some(d) = d.as_mut() {
                wipe(d);
            }
            if ok {
                unsafe { CloseHandle(token) };
                Answer::Granted
            } else {
                Answer::Failed(reason(error))
            }
        };
        wipe(&mut password);
        wipe(&mut user);
        wipe(&mut domain);
        answer
    }
}

/// How many holds on its focus reports `pane` has: its waiting password, closed managers'
/// holds waiting for the focus, and managers open over it.
fn true_holds(pane: PaneId, entry: bool, holds: &[PaneId], windows: impl IntoIterator<Item = Option<PaneId>>) -> u32 {
    let waiting = holds.iter().filter(|p| **p == pane).count();
    let open = windows.into_iter().filter(|w| *w == Some(pane)).count();
    entry as u32 + waiting as u32 + open as u32
}

/// True the first time `window` asks in a pass (it is then noted in `seen`).
fn first_for<W: PartialEq>(seen: &mut Vec<W>, window: W) -> bool {
    if seen.contains(&window) {
        return false;
    }
    seen.push(window);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_restored_pane_counts_only_its_live_holds() {
        assert_eq!(true_holds(3, false, &[], [None, Some(4)]), 0, "nothing left on it: reports flow");
        assert_eq!(true_holds(3, true, &[3, 5, 3], [Some(3), None]), 4);
        assert_eq!(true_holds(3, false, &[5], [Some(3)]), 1, "a manager still open over it keeps one");
    }

    #[test]
    fn opens_for_one_window_coalesce() {
        let mut seen = Vec::new();
        assert!(first_for(&mut seen, 1));
        assert!(!first_for(&mut seen, 1), "a second open for the same window in a pass is dropped");
        assert!(first_for(&mut seen, 2), "another window opens its own");
        // A window whose gate was refused on the last pass starts the next one seen.
        let mut seen = vec![7];
        assert!(!first_for(&mut seen, 7));
        assert!(first_for(&mut seen, 8));
    }

    #[test]
    fn source_alerts_follow_iterm2() {
        // Credential Manager shows none; a canceled prompt never shows one.
        assert!(source_alert(Kind::CredentialManager, "Element not found.").is_none());
        assert!(source_alert(Kind::KeePassXC, CANCELED).is_none());
        let heading = |k: Kind, e: &str| source_alert(k, e).map(|(a, _)| a.heading);
        assert_eq!(heading(Kind::OnePassword, pwm_cli::OP_TIMEOUT_MESSAGE).as_deref(), Some("Timeout"));
        assert_eq!(heading(Kind::OnePassword, pwm_cli::OP_SIGNIN_FAILED_MESSAGE).as_deref(), Some("Authentication Error"));
        assert!(heading(Kind::OnePassword, pwm_cli::OP_RUNTIME_ERROR).is_none());
        assert_eq!(heading(Kind::LastPass, &pwm_cli::lp_timeout_message(pwm_cli::LP_ACCOUNT_LIST_FETCH_FAILED)).as_deref(), Some("Timeout"));
        assert_eq!(heading(Kind::Bitwarden, "Adapter not found.").as_deref(), Some("Password Manager Error"));
        let (a, f) = source_alert(Kind::LastPass, &pwm_cli::lp_auth_failed_message("me@x")).unwrap();
        assert_eq!(a.heading, "Authentication Failed");
        assert_eq!(a.buttons, vec!["Open Terminal Window", "Copy Command", "Cancel"]);
        assert!(matches!(f, AlertFor::LpAuthFailed(e) if e == "me@x"));
        assert_eq!(unavailable_alert(Kind::OnePassword, pwm_cli::OP_NOT_FOUND_MESSAGE).map(|(a, _)| a.heading).as_deref(), Some("OnePassword Unavailable"));
        assert!(unavailable_alert(Kind::CredentialManager, "x").is_none());

        let op = cannot_find_alert(Kind::OnePassword);
        assert_eq!((op.heading.as_str(), op.buttons.clone()), ("Can\u{2019}t Find 1Password CLI", vec!["Locate".to_string(), "Cancel".to_string()]));
        let lp = cannot_find_alert(Kind::LastPass);
        assert_eq!(lp.buttons, vec!["Locate", "Cancel", "Help"]);
        assert_eq!(adapter_identifier(Kind::Keeper), Some("Keeper Security"));
        assert_eq!(adapter_identifier(Kind::OnePassword), None);
    }

    #[test]
    fn authentication_texts_follow_iterm2() {
        assert_eq!(win_auth::CAPTION, "Tandem");
        assert_eq!(win_auth::MESSAGE, "Tandem wants to open the password manager.");
        assert_eq!(win_auth::reason(1326), "valid credentials weren't supplied.");
        assert_eq!(win_auth::reason(1223), "password entry was cancelled.");
        assert!(win_auth::reason(5).ends_with('.'));
        let w = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
        assert_eq!(win_auth::split_user(&w("BOX\\bob"), &w("")), (w("bob"), Some(w("BOX"))));
        assert_eq!(win_auth::split_user(&w("bob@example.com"), &w("")), (w("bob@example.com"), None));
        assert_eq!(win_auth::split_user(&w("bob"), &w("CORP")), (w("bob"), Some(w("CORP"))));
        // Windows Hello: verified, canceled, and a missing device or setup falls back.
        assert!(matches!(win_auth::hello_answer(0), Some(win_auth::Answer::Granted)));
        assert!(matches!(win_auth::hello_answer(6), Some(win_auth::Answer::Canceled)));
        assert!(win_auth::hello_answer(1).is_none() && win_auth::hello_answer(2).is_none());
        assert!(matches!(win_auth::hello_answer(5), Some(win_auth::Answer::Failed(r)) if r == "there were too many failed attempts."));
    }

    #[test]
    fn random_picks_stay_in_range() {
        for bound in [1u32, 2, 62, 91] {
            for _ in 0..200 {
                assert!(random_below(bound) < bound);
            }
        }
        let p = pwm::random_password(false, &mut random_below);
        assert_eq!(p.expose().chars().count(), 16);
        assert!(p.expose().chars().all(|c| pwm::CHARS.contains(c)));
    }

    #[test]
    fn a_changed_source_is_rebuilt() {
        let c = Config { kind: Kind::OnePassword, cli: String::new(), groups: String::new(), account: String::new(), adapter: AdapterConfig::default(), scope: Scope::Terminal };
        let picked = Config { account: "AB".into(), ..c.clone() };
        assert!(!c.same_source(&picked), "an account changed in Settings rebuilds the 1Password source");
        assert!(!c.same_source(&Config { cli: "C:\\op.exe".into(), ..c.clone() }));
        assert!(!c.same_source(&Config { kind: Kind::LastPass, ..c.clone() }));
    }

    #[test]
    fn autofill_fill_answers_only_a_pointer_click() {
        use winit::keyboard::{Key, NamedKey};
        assert!(autofill_key_allowed(&Key::Named(NamedKey::Escape)));
        for k in [Key::Named(NamedKey::Tab), Key::Named(NamedKey::Space), Key::Named(NamedKey::Enter), Key::Character(" ".into()), Key::Character("f".into())] {
            assert!(!autofill_key_allowed(&k), "{k:?} must not reach the question");
        }
        assert!(!autofill_key_dropped(&WindowEvent::Focused(true)), "pointer and focus events pass");
        assert!(autofill_key_dropped(&WindowEvent::Ime(winit::event::Ime::Enabled)));
        // The call site routes every keyboard event of an Autofill question through the rule, and the
        // window carries the one shared guard (no second delay of its own).
        let src = include_str!("pwm_app.rs");
        let prod = &src[..src.find("#[cfg(test)]").unwrap()];
        let ev = &prod[prod.find("pub(super) fn pwm_confirm_event").unwrap()..prod.find("pub(super) fn pwm_ask_autofill").unwrap()];
        assert!(ev.contains("if autofill && autofill_escape(k) && !w.input_guarded(event) => ConfirmResult::Cancel"), "Escape cancels only once the guard is armed");
        assert!(ev.contains("if !(autofill && autofill_key_dropped(other)) {\n                    w.event(other);"), "every other key is dropped; the window's guard drops early clicks and touches");
        assert!(prod.contains("if matches!(c.kind, ConfirmKind::Autofill(..)) {\n                        w.guard_input();"));
        assert!(!prod.contains("input_ignored(") && !prod.contains("opened: Option<Instant>"), "no second guard");
    }

    #[test]
    fn a_changed_scope_is_rebuilt() {
        let c = Config { kind: Kind::CredentialManager, cli: String::new(), groups: String::new(), account: String::new(), adapter: AdapterConfig::default(), scope: Scope::Terminal };
        assert!(c.same_source(&c.clone()));
        assert!(!c.same_source(&Config { scope: Scope::Browser, ..c.clone() }));
    }

    fn acct(name: &str, user: &str) -> Account {
        Account { id: format!("{name}|{user}"), account_name: name.into(), user_name: user.into(), display: name.into(), has_otp: false, send_otp: false, source_label: None }
    }

    fn inputs() -> ButtonInputs {
        ButtonInputs { target: PwmTarget::Session, for_user: false, send_both: false, selected: true, user_name_empty: false, session_can_enter: true, can_broadcast: true, available: true, sends_otp: false }
    }

    #[test]
    fn browser_manager_follows_iterm2() {
        // A browser pane (named, or the focused one) opens the browser store, bound to the pane; a
        // terminal pane the terminal's. Edit > Password Manager from a settings page names no account.
        let b = open_request(None, Some(7), Some(3), true);
        assert_eq!((b.scope, b.pane, b.for_user, b.send_both, b.target), (Scope::Browser, Some(7), false, false, PwmTarget::Session));
        assert_eq!(open_request(None, None, Some(3), true).pane, Some(3), "no pane named: the focused browser pane");
        let t = open_request(Some("a".into()), Some(7), Some(3), false);
        assert_eq!((t.scope, t.pane, t.account.as_deref()), (Scope::Terminal, Some(7), Some("a")));
        assert_eq!(open_request(None, None, Some(3), false).pane, None, "a terminal request keeps its pane as asked");
        // The titles (`PMW:361-371`).
        let user = pwm::buttons(&button_state(&ButtonInputs { for_user: true, ..inputs() }));
        assert_eq!((user.default_title, user.secondary_shown), (pwm::TITLE_ENTER_USER_NAME, false));
        let both = pwm::buttons(&button_state(&ButtonInputs { for_user: true, send_both: true, ..inputs() }));
        assert_eq!((both.default_title, both.default_on), (pwm::TITLE_ENTER_USERNAME_AND_PASSWORD, true));
        let no_user = pwm::buttons(&button_state(&ButtonInputs { for_user: true, send_both: true, user_name_empty: true, ..inputs() }));
        assert!(!no_user.default_on && !no_user.secondary_on, "send-both needs a user name");
        // A picker can always enter and never broadcast (`BAP:54-58`).
        let picker = button_state(&ButtonInputs { target: PwmTarget::BasicAuth(7), session_can_enter: false, ..inputs() });
        assert!(picker.can_enter && !picker.can_broadcast);
        let session = button_state(&ButtonInputs { session_can_enter: false, ..inputs() });
        assert!(!session.can_enter, "a page that does not take the account enables nothing");
        // The slot: a picker never displaces an open manager, it ends its challenge; the session path refuses.
        assert_eq!(open_decision(PwmTarget::BasicAuth(7), true), OpenDecision::AnswerNone);
        assert_eq!(open_decision(PwmTarget::BasicAuth(7), false), OpenDecision::Open);
        assert_eq!(open_decision(PwmTarget::Session, true), OpenDecision::Refuse);
        // The source: its own key, and the choice goes there.
        assert_eq!(Scope::Browser.source_key(), pwm_source::SOURCE_KEY_BROWSER);
        assert_ne!(Scope::Browser.source_key(), Scope::Terminal.source_key());
        // Replies and info are kept apart.
        let mut st = PwmState::default();
        st.store_info(Scope::Browser, (Kind::CredentialManager, "b".into(), Caps::default()));
        assert!(st.info.is_none() && st.browser_info.is_some());
        assert_eq!(st.info_of(Scope::Browser).as_ref().map(|i| i.1.as_str()), Some("b"));
        assert!(st.info_of(Scope::Terminal).is_none());
        let src = include_str!("pwm_app.rs");
        let prod = &src[..src.find("#[cfg(test)]\nmod tests").unwrap()];
        assert!(prod.contains("Scope::Browser => &mut self.pwm.browser_worker"), "the browser's jobs go to its own worker");
        assert!(prod.contains("Worker::start(notify, scope)") && prod.contains("self.out.send(Reply { scope: self.scope,"), "replies carry the scope");
        assert!(prod.contains("self.store_info(scope, i)"), "a reply updates its own store's info");
        assert!(prod.contains("self.globals.set(self.cur_scope().source_key(),"), "choosing a source writes the scope's key");
        assert!(prod.contains("browser: scope == Scope::Browser"));
        assert!(prod.contains("if scope == Scope::Terminal {\n                    pwm_prompt::set_cached_names"), "a browser list never reaches terminal triggers");
    }

    #[test]
    fn browser_unlock_leaves_terminal_locked() {
        let (required, every) = (true, false);
        let mut st = PwmState::default();
        assert!(st.auth_of(Scope::Browser).needs_prompt(required, every) && st.auth_of(Scope::Terminal).needs_prompt(required, every));
        assert!(st.apply_answer(Scope::Browser, win_auth::Answer::Granted).is_ok());
        assert!(!st.auth_of(Scope::Browser).needs_prompt(required, every));
        assert!(st.auth_of(Scope::Terminal).needs_prompt(required, every), "the terminal store stays locked");
        assert!(st.apply_answer(Scope::Terminal, win_auth::Answer::Granted).is_ok());
        assert!(st.apply_answer(Scope::Browser, win_auth::Answer::Canceled).is_err());
        assert!(st.auth_of(Scope::Browser).needs_prompt(required, every) && !st.auth_of(Scope::Terminal).needs_prompt(required, every), "a refusal revokes only its own store");
        // The reverse.
        let mut st = PwmState::default();
        assert!(st.apply_answer(Scope::Terminal, win_auth::Answer::Granted).is_ok());
        assert!(st.auth_of(Scope::Browser).needs_prompt(required, every));
        // A screen lock revokes both.
        assert!(st.apply_answer(Scope::Browser, win_auth::Answer::Granted).is_ok());
        st.screen_locked(false);
        assert!(st.auth.authenticated && st.browser_auth.authenticated, "after-lock off keeps both");
        st.screen_locked(true);
        assert!(!st.auth.authenticated && !st.browser_auth.authenticated);
        // Every caller names the store it unlocks.
        let src = include_str!("pwm_app.rs");
        let prod = &src[..src.find("#[cfg(test)]\nmod tests").unwrap()];
        assert!(!prod.contains("self.authenticate()") && !prod.contains("self.pwm.auth.needs_prompt"), "no caller unlocks the terminal store by default");
        assert!(prod.contains("self.authenticate(scope)"));
    }

    #[test]
    fn enter_routes_to_the_page() {
        // Send-both: the user name, the focus step, then the password.
        let focus = ("pw".to_string(), 4u64);
        assert_eq!(page_steps(true, "me", Some(&focus), 4), vec![PageStep::User("me".into()), PageStep::Focus("pw".into()), PageStep::Password]);
        assert_eq!(page_steps(true, "me", Some(&focus), 5), vec![PageStep::User("me".into()), PageStep::Password], "a page that navigated since gets no focus step");
        assert_eq!(page_steps(true, "", Some(&focus), 4), vec![PageStep::Focus("pw".into()), PageStep::Password], "no user name, none typed");
        assert_eq!(page_steps(false, "me", Some(&focus), 4), vec![PageStep::Password], "plain Enter Password types only the password");
        // A picker: the user name first, then the password finishes the challenge; closing finishes with none, once.
        let mut t = PwmTarget::BasicAuth(7);
        assert_eq!(take_challenge(&mut t), Some(7));
        assert_eq!(take_challenge(&mut t), None, "a challenge ends once");
        let mut s = PwmTarget::Session;
        assert_eq!(take_challenge(&mut s), None);
        // The source: a browser pane gets nothing through the pty paths; the page path is bound.
        let src = include_str!("pwm_app.rs");
        let prod = &src[..src.find("#[cfg(test)]\nmod tests").unwrap()];
        let pw = between(prod, "pub(super) fn enter_password", "pub(super) fn broadcast_password");
        assert!(pw.contains("self.is_browser_pane(pane)") && pw.find("is_browser_pane").unwrap() < pw.find("Probe::begin").unwrap(), "no echo probe for a browser pane");
        let un = between(prod, "pub(super) fn enter_username", "fn end_probe_of");
        assert!(un.contains("self.is_browser_pane(pane)") && un.find("is_browser_pane").unwrap() < un.find("user_write").unwrap(), "no pty write for a browser pane");
        assert!(prod.contains(".filter(|t| !self.is_tmux_gateway(*t) && !self.is_browser_pane(*t)).collect();"), "broadcast skips pages");
        let eb = between(prod, "fn enter_browser", "/// `-[PTYSession enterPassword:]`");
        assert!(eb.contains("self.browser_fill_username(pane, page.clone(),") && eb.contains("self.browser_focus_password(pane, page.clone(),") && eb.contains("self.browser_fill_password(pane, page.clone(), p)"));
        assert!(eb.contains("page_steps(w.send_both, &a.user_name, w.focus_after_user.as_ref(), page.epoch)"));
        assert!(eb.contains("take_challenge(&mut w.target)") && eb.contains("self.basic_auth_picked(n, Some((user, password)))"));
        assert!(prod.contains("ORPHANED.lock().unwrap().push(n)"), "a picker that goes with its window ends its challenge");
    }

    #[test]
    fn a_page_takes_only_its_own_accounts() {
        let page = PwPage::new("https://shop.example/login", 3).unwrap();
        let all = vec![
            acct("https://shop.example", "me"),
            acct("https://shop.example (Realm)", "me"),
            acct("https://evil.example", "me"),
            acct("https://shop.example.evil.test", "me"),
            acct("https://evil.test/https://shop.example", "me"),
            acct("http://shop.example", "me"),
            acct("https://shop.example:8443", "me"),
            acct("shop.example", "me"),
            acct("terminal-login", "root"),
        ];
        let names = |v: Vec<Account>| v.into_iter().map(|a| a.account_name).collect::<Vec<_>>();
        assert_eq!(names(browser_listing(Scope::Browser, PwmTarget::Session, Some(&page), all.clone())), vec!["https://shop.example", "https://shop.example (Realm)"]);
        assert!(browser_listing(Scope::Browser, PwmTarget::Session, None, all.clone()).is_empty(), "an http (or no) page lists nothing");
        assert_eq!(browser_listing(Scope::Terminal, PwmTarget::Session, None, all.clone()).len(), all.len(), "the terminal manager lists its own store");
        assert_eq!(names(browser_listing(Scope::Browser, PwmTarget::BasicAuth(1), Some(&page), all.clone())), vec!["https://shop.example", "https://shop.example (Realm)"], "a picker lists only the challenge's own origin");
        assert!(browser_listing(Scope::Browser, PwmTarget::BasicAuth(1), None, all.clone()).is_empty(), "a picker with no https origin lists nothing");
        // The pick is checked again against the page showing then.
        let src = include_str!("pwm_app.rs");
        let prod = &src[..src.find("#[cfg(test)]\nmod tests").unwrap()];
        let pick = between(prod, "fn pick_why", "fn pwm_enter_user");
        assert!(pick.contains("self.browser_pw_page(pane)?") && pick.contains("bp::account_matches(&a.account_name, &page)"), "the pick binds the page, then the account");
        assert!(pick.contains("w.pick_page.clone()?") && pick.contains("PwmTarget::BasicAuth(_)"), "a picker's pick binds the challenge's origin first");
        let user = between(prod, "fn pwm_enter_user", "fn enter_browser");
        assert!(user.contains("bp::account_matches(account, &page)"));
        assert!(prod.contains("fn page_accepts") && prod.contains("bp::account_matches(account, &p)"));
        assert!(!prod.contains("fn account_matches") && !prod.contains("fn origin_matches"), "one matcher");
        assert!(between(prod, "PwmAct::EnterPassword => {", "PwmAct::EnterUserName").find("pick_why").unwrap() < between(prod, "PwmAct::EnterPassword => {", "PwmAct::EnterUserName").find("Work::Fetch").unwrap(), "bound before the fetch");
    }

    #[test]
    fn the_browser_manager_never_logs_or_derives_debug_over_secrets() {
        let src = include_str!("pwm_app.rs");
        let prod = &src[..src.find("#[cfg(test)]\nmod tests").unwrap()];
        for line in ["pub(super) struct PickerResult", "pub(super) struct PwmOpen", "enum PwmTarget"] {
            let at = prod.find(line).unwrap();
            let before = &prod[at.saturating_sub(160)..at];
            if line.contains("PickerResult") {
                assert!(!before.contains("derive(") || !before.contains("Debug"), "{line}: a secret field has no derived Debug");
            }
        }
        for banned in ["log::", "println!", "eprintln!", "dbg!"] {
            let browser = between(prod, "fn pick_why", "pub(super) fn enter_password");
            assert!(!browser.contains(banned), "{banned}");
        }
    }

    /// Task 6 review I1: a picker whose window cannot open must report false so its challenge is
    /// answered; only a session may report true on a failure. Task 6 review M1 (class rule): the
    /// manager a page's button opens carries the one input guard.
    #[test]
    fn a_failed_open_answers_a_picker_and_a_page_opened_manager_is_guarded() {
        let src = include_str!("pwm_app.rs");
        let prod = &src[..src.find("#[cfg(test)]\nmod tests").unwrap()];
        assert!(!prod.contains("return !session") && !prod.contains("{ return !session }"), "no inverted failure return");
        let open = between(prod, "fn open_pwm_now", "pub(super) fn authenticate");
        assert_eq!(open.matches("return session").count(), 4, "all four failure paths answer a picker");
        let guard = between(open, "Ok((mut win, h)) => {", "Err(e) =>");
        assert!(guard.contains("if from_page {") && guard.contains("win.guard_input();"));
        assert_eq!(prod.matches("guard_input()").count(), 2, "the autofill question and this window");
        let ui = include_str!("../pwm_ui.rs");
        assert!(ui.contains("g.drops(e, now)") && ui.contains("g.window_event(e, now)"), "the window drops guarded input before egui");
        assert!(include_str!("browser_password_app.rs").contains("from_page: true,"));
        // The toast tells the person why nothing was entered (M5).
        assert!(between(prod, "pub(super) fn enter_password", "let bs =").contains("show_toast"));
    }

    fn between<'a>(s: &'a str, a: &str, b: &str) -> &'a str {
        let i = s.find(a).unwrap_or_else(|| panic!("{a}"));
        let j = s[i..].find(b).unwrap_or_else(|| panic!("{b}")) + i;
        &s[i..j]
    }

    /// A store whose accounts are a list; it records what was written.
    struct FakeStore {
        accounts: Vec<Account>,
        calls: Vec<String>,
        fail: bool,
    }

    impl Source for FakeStore {
        fn kind(&self) -> Kind {
            Kind::CredentialManager
        }
        fn name(&self) -> String {
            "fake".into()
        }
        fn caps(&self) -> Caps {
            Caps::default()
        }
        fn available(&mut self) -> Result<(), String> {
            Ok(())
        }
        fn accounts(&mut self) -> Result<Vec<Account>, String> {
            if self.fail { Err("locked".into()) } else { Ok(self.accounts.clone()) }
        }
        fn password(&mut self, a: &Account) -> Result<(Secret, Option<Secret>), String> {
            Ok((Secret::new(format!("pw-of-{}", a.user_name)), None))
        }
        fn set_password(&mut self, a: &Account, _p: Secret) -> Result<(), String> {
            self.calls.push(format!("set {} {}", a.account_name, a.user_name));
            if self.fail { Err("no".into()) } else { Ok(()) }
        }
        fn delete(&mut self, _a: &Account) -> Result<(), String> {
            Ok(())
        }
        fn add(&mut self, name: &str, user: &str, _p: Secret, flags: &[(String, bool)]) -> Result<Account, String> {
            self.calls.push(format!("add {name} {user} {}", flags.len()));
            if self.fail { Err("no".into()) } else { Ok(acct(name, user)) }
        }
        fn edit_in_place(&mut self, _a: &Account, _n: Option<&str>, _u: Option<&str>, _p: Option<Secret>) -> Result<(), String> {
            Ok(())
        }
        fn toggle_otp(&mut self, a: &Account) -> Result<Account, String> {
            Ok(a.clone())
        }
        fn reload(&mut self) {}
        fn reset_configuration(&mut self) {}
        fn switch_account(&mut self) -> Result<(), String> {
            Ok(())
        }
    }

    fn space() -> SpaceKey {
        SpaceKey { scheme: "https".into(), host: "a.b".into(), port: 443, realm: String::new() }
    }

    #[test]
    fn auth_jobs_follow_iterm2() {
        let mut st = FakeStore { accounts: vec![acct("https://a.b", "u")], calls: Vec::new(), fail: false };
        let look = |st: &mut FakeStore, name: &str| auth_job(st, AuthWork::Lookup { pane: 1, id: 3, name: name.into(), preferred_user: None });
        match look(&mut st, "https://a.b") {
            Done::AuthLooked { pane: 1, id: 3, found: Some((user, pw)) } => assert_eq!((user.as_str(), pw.expose()), ("u", "pw-of-u")),
            _ => panic!("a saved account is found"),
        }
        assert!(matches!(look(&mut st, "https://c.d"), Done::AuthLooked { found: None, .. }));
        let save = |st: &mut FakeStore, name: &str, user: &str| auth_job(st, AuthWork::Save { pane: 1, space: space(), name: name.into(), user: user.into(), password: Secret::new("p".into()) });
        assert!(matches!(save(&mut st, "https://a.b", "u"), Done::AuthSaved { success: true, .. }));
        assert!(matches!(save(&mut st, "https://a.b", "v"), Done::AuthSaved { success: true, .. }));
        assert_eq!(st.calls, vec!["set https://a.b u", "add https://a.b v 0"], "an existing pair is set, else added with no flags");
        st.fail = true;
        assert!(matches!(save(&mut st, "https://a.b", "u"), Done::AuthSaved { success: false, .. }), "the result comes from the store");
        assert!(matches!(look(&mut st, "https://a.b"), Done::AuthLooked { found: None, .. }), "a store error is a miss");
    }

    #[test]
    fn auth_replies_route_before_windows() {
        let info = || Some((Kind::CredentialManager, "b".to_string(), Caps::default()));
        let mut st = PwmState::default();
        st.info = Some((Kind::OnePassword, "t".into(), Caps::default()));
        let r = Reply { scope: Scope::Browser, owner: None, instance: 0, token: 0, busy: false, info: info(), done: Done::AuthLooked { pane: 5, id: 3, found: None } };
        assert!(st.route_reply(r).is_none(), "a store reply needs no window");
        assert_eq!(st.store_replies.len(), 1);
        assert!(matches!(st.store_replies[0], StoreReply::Looked { pane: 5, id: 3, found: None }));
        assert_eq!(st.browser_info.as_ref().map(|i| i.1.as_str()), Some("b"));
        assert_eq!(st.info.as_ref().map(|i| i.1.as_str()), Some("t"), "the terminal's info is untouched");
        let (tx, rx) = mpsc::channel::<Option<Vec<(String, Secret)>>>();
        let r = Reply { scope: Scope::Browser, owner: None, instance: 0, token: 0, busy: false, info: None, done: Done::Prompt { heading: String::new(), user: None, answer: tx } };
        assert!(st.route_reply(r).is_none());
        assert!(rx.recv().is_err(), "an ownerless prompt drops its sender");
        // A terminal window's reply still updates the terminal's info and goes on to its window.
        let w = WindowId::dummy();
        let r = Reply { scope: Scope::Terminal, owner: Some(w), instance: 1, token: 0, busy: false, info: Some((Kind::LastPass, "lp".into(), Caps::default())), done: Done::Reset };
        assert!(st.route_reply(r).is_some());
        assert_eq!(st.info.as_ref().map(|i| i.1.as_str()), Some("lp"));
        assert_eq!(st.browser_info.as_ref().map(|i| i.1.as_str()), Some("b"));
        let src = include_str!("pwm_app.rs");
        let prod = &src[..src.find("#[cfg(test)]\nmod tests").unwrap()];
        let start = prod.find("fn pwm_reply").unwrap();
        assert!(prod[start..start + 400].contains("route_reply"), "routing is the first step of pwm_reply");
        assert!(!prod.contains("self.pwm.info = Some"), "no reply overwrites the terminal info directly");
    }
}
