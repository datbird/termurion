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
use crate::echo_probe::{self, Finish, Out, Probe, Round};
use crate::prompt_ui::{ConfirmResult, ConfirmWindow};
use crate::pwm::{self, Account, AccountPanel, Alert, Auth, Busy, GearItem, Secret};
use crate::pwm_cli::{self, OpNotUsable};
use crate::pwm_prompt;
use crate::pwm_source::{self, Caps, Field, Kind, Source};
use crate::pwm_ui::{PwmAct, PwmView, PwmWindow};

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
    /// Opens asked for, done on the next pass (a window needs the event loop).
    requests: Vec<(Option<String>, Option<PaneId>)>,
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
}

enum ConfirmKind {
    /// "Are you really at a password prompt?" for a session whose probe failed.
    Check(PaneId, u64),
    /// "Not all sessions at password prompt": Cancel, or enter it in the sessions at a prompt.
    Broadcast(Vec<PaneId>, Secret),
    /// OK only.
    Warning,
    /// OK with "Suppress this message permanently", remembered under the key.
    Notice(String),
}

impl App {
    /// Opens the password manager over the window of `pane` (or the current window) to
    /// `account`, on the next pass (`openPasswordManagerToAccountName:inSession:`).
    pub(super) fn open_password_manager(&mut self, account: Option<String>, pane: Option<PaneId>) {
        self.pwm.requests.push((account, pane));
        self.redraw();
    }

    /// Opens what waited for the event loop: managers, questions, LastPass's login window.
    pub(super) fn open_pwm_windows(&mut self, event_loop: &ActiveEventLoop) {
        for (account, pane) in std::mem::take(&mut self.pwm.requests) {
            self.open_pwm_now(event_loop, account, pane);
        }
        if let Some(email) = self.pwm.lp_terminal.take() {
            self.open_lp_login(event_loop, &email);
        }
        self.watch_lp_login(event_loop);
        let Some(g) = self.gpu.as_ref() else { return };
        let mut failed = None;
        for c in self.pwm.confirms.iter_mut().filter(|c| c.window.is_none()) {
            let opened = match &c.kind {
                ConfirmKind::Check(..) => ConfirmWindow::choice(event_loop, &g.instance, &g.adapter, &g.device, &g.queue, &c.heading, &c.text, pwm::TITLE_CANCEL, pwm::TITLE_ENTER_PASSWORD),
                ConfirmKind::Broadcast(..) => ConfirmWindow::choice(event_loop, &g.instance, &g.adapter, &g.device, &g.queue, &c.heading, &c.text, pwm::TITLE_CANCEL, BROADCAST_ENTER),
                ConfirmKind::Warning => ConfirmWindow::warning(event_loop, &g.instance, &g.adapter, &g.device, &g.queue, &c.heading, &c.text),
                ConfirmKind::Notice(_) => ConfirmWindow::notice(event_loop, &g.instance, &g.adapter, &g.device, &g.queue, &c.heading, &c.text),
            };
            match opened {
                Ok(w) => c.window = Some(w),
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
    fn open_pwm_now(&mut self, event_loop: &ActiveEventLoop, account: Option<String>, pane: Option<PaneId>) {
        if let Some(p) = pane {
            // Unburied first, else it has no window (`AD:650-653`).
            self.disinter(p);
            if !self.enter_pane_window(p) {
                return;
            }
            self.reveal_pane(p);
        } else if let Some(w) = self.last_window {
            self.enter(w);
        }
        if self.pwm_window.is_some() {
            return; // "Password manager sheet already open"
        }
        // `canOpenPasswordManager`: not while that session's probe runs.
        if pane.and_then(|p| self.panes.get(&p)).and_then(|p| p.echo_probe.as_ref()).is_some_and(Probe::active) {
            return;
        }
        // It would be a shame to send a focus report to a password prompt.
        let hold = pane.or_else(|| self.focused_id());
        if let Some(p) = hold.and_then(|id| self.panes.get_mut(&id)) {
            p.no_focus_reports += 1;
        }
        let required = self.pwm_setting(pwm::REQUIRE_OPEN_KEY);
        let every = self.pwm_setting(pwm::REQUIRE_EVERY_KEY);
        let prompted = self.pwm.auth.needs_prompt(required, every);
        if prompted {
            if !self.authenticate() {
                // The prompt took the focus: the hold ends when the window gets it back.
                if let Some(p) = hold {
                    self.pwm.holds.push(p);
                }
                return;
            }
        } else if !required {
            self.pwm.auth.open_unprompted();
        }
        let opened = match (self.gpu.as_ref(), self.hwnd()) {
            (Some(g), Some(h)) => PwmWindow::open(event_loop, &g.instance, &g.adapter, &g.device, &g.queue, h).map(|w| (w, h)),
            _ => Err("no terminal window".to_string()),
        };
        let (window, owner_hwnd) = match opened {
            Ok(w) => w,
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
                return;
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
        });
        self.pwm_job(Work::Load, true);
    }

    /// Windows Hello for the signed-in user when it is available (iTerm2's `LAContext`
    /// `deviceOwnerAuthentication`), else the Windows credential prompt checked with `LogonUserW`
    /// (`requestAuthenticationIfNeeded`). A failure or a cancel shows iTerm2's alert.
    pub(super) fn authenticate(&mut self) -> bool {
        let owner = self.hwnd().unwrap_or(0);
        // Windows Hello (PIN, fingerprint, face) where it is set up, else the password prompt.
        let answer = win_auth::hello(owner).unwrap_or_else(|| win_auth::prompt(owner));
        let reason = match answer {
            win_auth::Answer::Granted => {
                self.pwm.auth.granted(true);
                return true;
            }
            win_auth::Answer::Canceled => "password entry was cancelled.".to_string(),
            win_auth::Answer::Failed(reason) => reason,
        };
        self.pwm.auth.granted(false);
        self.pwm_warning(AUTH_FAILED_TITLE, &format!("Authentication failed because {reason}"));
        false
    }

    fn pwm_warning(&mut self, heading: &str, text: &str) {
        self.pwm.confirms.push(Confirm { window: None, heading: heading.to_string(), text: text.to_string(), kind: ConfirmKind::Warning });
    }

    /// A gear setting's value (stored, else iTerm2's default).
    fn pwm_setting(&self, key: &str) -> bool {
        pwm::setting(&|k| self.globals.get(k).and_then(|v| v.as_bool()), key)
    }

    fn probe_duration(&self) -> f64 {
        self.globals.number(echo_probe::DURATION_KEY).unwrap_or(echo_probe::DEFAULT_DURATION)
    }

    fn pwm_kind(&self) -> Kind {
        Kind::from_stored(&self.globals.string(pwm_source::SOURCE_KEY))
    }

    /// The current source's name and capabilities, once the worker has built it.
    fn pwm_info(&self) -> (String, Caps) {
        let kind = self.pwm_kind();
        match &self.pwm.info {
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
        let can_enter = focused.and_then(|id| self.panes.get(&id)).is_some_and(|p| p.exited.is_none());
        let can_broadcast = focused.is_some_and(|id| self.broadcast.targets(&self.broadcast_view(), id).len() > 1);
        pwm::buttons(&pwm::ButtonState {
            selected: sel.is_some(),
            user_name_empty: sel.is_none_or(|a| a.user_name.is_empty()),
            can_enter,
            can_broadcast,
            available: w.is_some_and(|w| w.available),
            send_user_by_default: false,
            send_both: false,
            account_sends_otp: sel.is_some_and(|a| a.send_otp),
        })
    }

    fn pwm_gear(&self) -> pwm::Gear {
        let (name, caps) = self.pwm_info();
        let authenticated = self.pwm.auth.authenticated;
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
        }
    }

    /// Closes this window's manager (`closeOrEndSheet`). Its session's focus reports stay held
    /// until the terminal window has the focus back.
    fn close_pwm(&mut self) {
        let Some(w) = self.pwm_window.take() else { return };
        if let Some(p) = w.hold {
            self.pwm.holds.push(p);
        }
        // Dropping the window answers any prompt it showed with Cancel.
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
        let authenticated = self.pwm.auth.authenticated;
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
                    self.pwm_job(Work::Fetch(a, Why::Enter), true);
                }
            }
            PwmAct::EnterUserName => {
                let user = w.selected_account().map(|a| a.user_name.clone()).unwrap_or_default();
                let broadcast = w.broadcast;
                if authenticated && !user.is_empty() && self.pwm_buttons().secondary_on {
                    if let Some(pane) = self.focused_id() {
                        self.enter_username(pane, &user, broadcast);
                    }
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
        if !self.pwm.auth.authenticated {
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
        self.globals.set(pwm_source::SOURCE_KEY, toml::Value::String(k.stored().to_string()));
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

    fn pwm_copy(&mut self, s: &Secret) {
        if let Some(cb) = &mut self.clipboard {
            if let Err(e) = cb.set_text(s.expose().to_string()) {
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
        let kind = self.pwm_kind();
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
        Config { kind, cli, groups: self.globals.string(LP_GROUPS_KEY), account: self.globals.string(OP_ACCOUNT_KEY), adapter }
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
        let worker = self.pwm.worker.get_or_insert_with(|| Worker::start(notify));
        if worker.tx.send(Job { owner, instance, token, busy, config, work }).is_err() {
            self.pwm.worker = None;
            if busy {
                w.busy.decr();
            }
            self.banners.set("pwm", "The password manager could not start its worker thread.");
        }
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
        if lock::watched() == 0 && self.pwm.auth.authenticated {
            if let Some(h) = self.hwnd() {
                lock::watch(h, self.notify.clone());
            }
        }
        let replies: Vec<Reply> = match &self.pwm.worker {
            Some(w) => w.rx.try_iter().collect(),
            None => Vec::new(),
        };
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
        let debounce = self.globals.bool(pwm_prompt::DETECT_KEY).then(|| self.globals.number(pwm_prompt::DEBOUNCE_KEY).unwrap_or(pwm_prompt::DEFAULT_DEBOUNCE));
        self.jobs.set_read_modes(debounce);
        if let Some(h) = &self.host {
            h.watch_prompts(debounce);
        }
        let mut changes = Vec::new();
        for (id, p) in self.panes.iter_mut() {
            let local = || p.owns_console() && p.shell_pid().is_some_and(|pid| self.jobs.password_input(pid));
            let now = debounce.is_some() && p.exited.is_none() && p.hosted_password_input().unwrap_or_else(local);
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
        self.pwm.auth.screen_locked(after);
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
        let Reply { owner, instance, token, busy, info, done } = r;
        if let Some(i) = info {
            self.pwm.info = Some(i);
        }
        let here = self.current_window();
        if here != Some(owner) && !self.enter(owner) {
            return; // The window closed: a prompt's sender drops, which answers Cancel.
        }
        let Some(w) = self.pwm_window.as_mut() else { return self.restore_window(here) };
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
            Done::Prompt { .. } | Done::Pick { .. } => {}
            Done::Listed { accounts, warning } => {
                let result = match accounts {
                    Ok(v) => pwm::sorted(v),
                    Err(e) => {
                        self.pwm_source_alert(kind, &e);
                        Vec::new()
                    }
                };
                // As a side effect, the trigger's pop-up gets the names (`setCachedCombinedAccountNames:`).
                pwm_prompt::set_cached_names(result.iter().map(|a| a.display.clone()).collect());
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
                    let key = format!("NoSyncAdapterListWarning_{id}");
                    if !self.globals.bool(&key) {
                        self.pwm.confirms.push(Confirm { window: None, heading: ADAPTER_WARNING_HEADING.to_string(), text, kind: ConfirmKind::Notice(key) });
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
        if !path.is_empty() && (kind == Kind::OnePassword || !std::path::Path::new(&path).is_file()) {
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
            Why::Enter => {
                // `didFetchPasswordToEnter:otp:`: to the window's current session (or its
                // broadcast sessions), then the manager closes.
                let Some(w) = self.pwm_window.as_ref() else { return };
                let combined = pwm::combined(&password, &w.two_factor, otp.as_ref());
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

    /// `-[PTYSession enterPassword:]`: the echo probe first, with the profile's Backspace; focus
    /// reports held until it is done. A tmux gateway gets nothing.
    pub(super) fn enter_password(&mut self, pane: PaneId, password: Secret) {
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
        let targets: Vec<PaneId> = self.broadcast.targets(&self.broadcast_view(), sender).into_iter().filter(|t| !self.is_tmux_gateway(*t)).collect();
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
        let text = format!("{user}\n");
        let targets = if broadcast { self.broadcast.targets(&self.broadcast_view(), pane) } else { vec![pane] };
        for t in targets {
            if !self.is_tmux_gateway(t) {
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
                    self.pwm.confirms.push(Confirm { window: None, heading: WARNING_HEADING.to_string(), text: echo_probe::CHECK_TEXT.to_string(), kind: ConfirmKind::Check(pane, id) });
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
                    self.pwm.confirms.push(Confirm { window: None, heading: heading.to_string(), text, kind });
                }
            }
        }
    }

    /// A session's name as the broadcast warning shows it (`presentationSessionTitle`).
    fn session_name(&self, pane: PaneId) -> String {
        let Some(p) = self.panes.get(&pane) else { return String::new() };
        p.own().title().map(str::to_string).unwrap_or_else(|| self.store.get(&p.profile).map(|pr| pr.string("Name")).unwrap_or_default())
    }

    /// Events for the password manager's questions. False for other windows.
    pub(super) fn pwm_confirm_event(&mut self, id: WindowId, event: &WindowEvent) -> bool {
        let Some(i) = self.pwm.confirms.iter().position(|c| c.window.as_ref().is_some_and(|w| w.id() == id)) else { return false };
        let Some(w) = self.pwm.confirms[i].window.as_mut() else { return false };
        let answer = match event {
            WindowEvent::CloseRequested => ConfirmResult::Cancel,
            WindowEvent::RedrawRequested => w.redraw(),
            other => {
                w.event(other);
                ConfirmResult::Open
            }
        };
        if !matches!(answer, ConfirmResult::Open) {
            let c = self.pwm.confirms.remove(i);
            self.pwm_confirmed(c.kind, answer);
        }
        true
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
            ConfirmKind::Warning => {}
            ConfirmKind::Notice(key) => {
                if let ConfirmResult::Ok { suppress: true } = answer {
                    self.globals.set(&key, toml::Value::Boolean(true));
                    self.save_globals();
                }
            }
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

/// An `iTermWarning` with no heading of its own (`iTermWarning.m:329`).
const WARNING_HEADING: &str = "Warning";

/// The broadcast warning's second button (`PT:15407`).
const BROADCAST_ENTER: &str = "Enter Password in Sessions at Prompt";

/// A field of a prompt from the worker.
fn field(key: &str, label: &str, secret: bool) -> Field {
    Field { key: key.to_string(), label: label.to_string(), placeholder: None, is_secret: secret, note: None, persist_in_keychain: false }
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
    (stem_ok && path.is_file()).then(|| path.to_string_lossy().into_owned())
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
        self.kind == o.kind && self.cli == o.cli && self.groups == o.groups && self.account == o.account && self.adapter == o.adapter
    }
}

struct Job {
    owner: WindowId,
    instance: u64,
    token: u32,
    busy: bool,
    config: Config,
    work: Work,
}

enum Why {
    Enter,
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
}

struct Reply {
    owner: WindowId,
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
        let sent = self.out.send(Reply { owner, instance, token: 0, busy: false, info: None, done }).is_ok();
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
    fn start(notify: Notify) -> Worker {
        let (tx, jobs) = mpsc::channel::<Job>();
        let (out, rx) = mpsc::channel::<Reply>();
        let _ = std::thread::Builder::new().name("tandem-pwm".into()).spawn(move || {
            let ctx = Ctx { out: out.clone(), notify: notify.clone(), current: Arc::new(Mutex::new(None)), picked: Arc::new(Mutex::new(None)) };
            let mut slot: Option<(Config, Box<dyn Source>)> = None;
            for job in jobs {
                *ctx.current.lock().unwrap() = Some((job.owner, job.instance));
                let (owner, instance, token, busy) = (job.owner, job.instance, job.token, job.busy);
                let done = run(&mut slot, job, &ctx);
                let info = slot.as_ref().map(|(_, s)| (s.kind(), s.name(), s.caps()));
                // The picked account is the one the built source now uses.
                if let (Some(a), Some((c, _))) = (ctx.picked.lock().unwrap().take(), slot.as_mut()) {
                    c.account = a;
                }
                if out.send(Reply { owner, instance, token, busy, info, done }).is_err() {
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
        Kind::CredentialManager => Ok(Box::new(crate::pwm_credman::CredentialManager::new())),
        Kind::OnePassword => {
            let mut runner = ProcessRunner::new();
            let path = if config.cli.is_empty() {
                let env = |k: &str| std::env::var(k).unwrap_or_default();
                let candidates = pwm_cli::op_paths(&env("PATH"), &env("LOCALAPPDATA"));
                pwm_cli::resolve_op(&mut runner, &candidates, &mut |p| std::path::Path::new(p).is_file()).map_err(NeedCli::Op)?
            } else {
                config.cli.clone()
            };
            let account = Some(config.account.clone()).filter(|a| !a.is_empty());
            Ok(Box::new(pwm_cli::OnePassword::new(runner, path, account, ctx.master_prompt()).with_picker(ctx.picker())))
        }
        Kind::LastPass => {
            let path = if config.cli.is_empty() { pwm_cli::resolve_lpass().map_err(|_| NeedCli::Lp)? } else { config.cli.clone() };
            Ok(Box::new(pwm_cli::LastPass::new(ProcessRunner::new(), path, pwm_cli::lpass_groups(&config.groups), ctx.login_prompt())))
        }
        Kind::KeePassXC | Kind::Bitwarden | Kind::Keeper => {
            let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(std::path::Path::to_path_buf)).unwrap_or_default();
            let path = crate::pwm_adapter::adapter_path(config.kind, &exe_dir).unwrap_or_default();
            let a = &config.adapter;
            let settings = a.settings.iter().cloned().collect();
            Ok(Box::new(crate::pwm_adapter::Adapter::new(ProcessRunner::new(), config.kind, path, a.db.clone(), a.exe.clone(), settings, None, ctx.master_prompt())))
        }
    }
}

fn list(src: &mut dyn Source) -> Done {
    let accounts = src.accounts();
    Done::Listed { accounts, warning: src.take_warning() }
}

/// Runs one request against the source, building it first when the configuration changed.
fn run(slot: &mut Option<(Config, Box<dyn Source>)>, job: Job, ctx: &Ctx) -> Done {
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

    /// Waits for a Windows Runtime operation, handling this thread's messages meanwhile (the
    /// Hello dialog and Tandem's windows keep working, as in any modal loop).
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

#[cfg(test)]
mod tests {
    use super::*;

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
        let c = Config { kind: Kind::OnePassword, cli: String::new(), groups: String::new(), account: String::new(), adapter: AdapterConfig::default() };
        let picked = Config { account: "AB".into(), ..c.clone() };
        assert!(!c.same_source(&picked), "an account changed in Settings rebuilds the 1Password source");
        assert!(!c.same_source(&Config { cli: "C:\\op.exe".into(), ..c.clone() }));
        assert!(!c.same_source(&Config { kind: Kind::LastPass, ..c.clone() }));
    }
}
