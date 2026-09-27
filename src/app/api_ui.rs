//! The Python API's app side: the server follows "Enable Python API", the authentication
//! popup mirrors the allow-all file, and the API's alerts ("Enable Python API?", "Allow Python
//! API Usage?" and the allow-all warnings) are shown one at a time. Events from the server are
//! pumped on each pass (`H:629-656`, `H:782-795`, `H:388-560`, `H:1541-1619`).

use std::collections::VecDeque;
use std::sync::Arc;

use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::WindowId;

use super::api_requests::Answer;
use super::App;
use crate::api_auth::{self, AllowAll, AllowAllStep, AskAnswer, LogEntry, Verdict};
use crate::api_dispatch::{Route, Subscriptions, Transactions, Tx};
use crate::api_model;
use crate::api_proto::pb;
use crate::api_server::{Event, Pending, Server};
use crate::api_vars::{self, Objects};
use crate::prompt_ui::{ConfirmResult, ConfirmWindow};

/// The settings this module follows.
pub(super) const ENABLE_KEY: &str = "EnableAPIServer";
pub(super) const AUTH_KEY: &str = "API Authentication Method";
pub(super) const SET_COOKIE_KEY: &str = crate::api_profile::SET_COOKIE;

/// The alert when the server could not start (Tandem's own texts).
pub const START_FAILED_HEADING: &str = "Could Not Start the Python API";

/// "The Python API could not start: ..." and, when another process listens on the port, its
/// name.
pub fn start_failed_text(error: &str, owner: Option<&str>) -> String {
    match owner {
        Some(o) => format!("The Python API could not start: {error}\n\nPort {} is in use by {o}.", crate::api_http::PORT),
        None => format!("The Python API could not start: {error}"),
    }
}

/// Requests waiting to be offered to the transaction logic, in order.
pub(super) type Work = VecDeque<(u64, pb::ClientOriginatedMessage)>;

/// An alert waiting its turn or showing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Alert {
    /// "Enable Python API?" from the Settings checkbox (forced: never silenced).
    Enable,
    /// "Allow Python API Usage?" for a waiting connection.
    Allow { conn: u64 },
    /// Always's "Confirm".
    ConfirmAlways { conn: u64 },
    /// The same "Confirm" for a script's SetPreference of `setCookie` on.
    ConfirmCookie,
    /// "Disable per-app authentication?".
    AllowAll,
    /// "Python API Permissions Reset" (silenceable under `RESET_ID`).
    Reset,
    /// The allow-all file could not be deleted: the text.
    RemoveFailed(String),
    /// The allow-all file could not be written: the error.
    MakeChangeFailed(String),
    /// The server could not start: the text.
    StartFailed(String),
}

/// What an alert shows: heading, text, buttons left to right, and whether "Suppress this
/// message permanently" shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Form {
    pub heading: &'static str,
    pub text: String,
    pub labels: Vec<&'static str>,
    pub silenceable: bool,
}

/// The alert's form; the Allow alert's text and buttons follow `setCookie` as the alert opens.
pub(super) fn form(alert: &Alert, set_cookie: bool) -> Form {
    use api_auth::*;
    let f = |heading, text: String, labels: &[&'static str], silenceable| Form { heading, text, labels: labels.to_vec(), silenceable };
    match alert {
        Alert::Enable => f(ENABLE_HEADING, ENABLE_TEXT.into(), &[OK, CANCEL], false),
        Alert::Allow { .. } => match decide(false, false, false, set_cookie) {
            Verdict::Ask { text, buttons } => f(ALLOW_HEADING, text, &buttons, false),
            _ => unreachable!("an undecided connection is always asked"),
        },
        Alert::ConfirmAlways { .. } | Alert::ConfirmCookie => f(CONFIRM_HEADING, CONFIRM_ALWAYS.into(), &[OK, CANCEL], false),
        Alert::AllowAll => f(ALLOW_ALL_HEADING, ALLOW_ALL_TEXT.into(), &[OK, CANCEL, MORE_INFO], false),
        Alert::Reset => f(RESET_HEADING, RESET_TEXT.into(), &[OK, REVEAL_PREFERENCE], true),
        Alert::RemoveFailed(text) => f(CHANGE_FAILED_HEADING, text.clone(), &[OK, SHOW_IN_EXPLORER], false),
        Alert::MakeChangeFailed(e) => f(MAKE_CHANGE_FAILED_HEADING, change_failed_text(e), &[OK], false),
        Alert::StartFailed(text) => f(START_FAILED_HEADING, text.clone(), &[OK], false),
    }
}

/// What the server should do for "Enable Python API" and the consent marker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ServerStep {
    /// Run (start it if it does not run).
    Start,
    /// The setting is on but "Enable Python API?" was never confirmed: turn the setting back
    /// off and ask.
    Warn,
    /// Stop it, and forget the consent.
    Off,
}

/// The server never starts unless the forced warning was confirmed: the setting alone, however
/// it was written (the Settings checkbox, Import from iTerm2, a Toggle Setting key, an edited
/// file), only asks.
pub(super) fn start_rule(setting: bool, consent: bool) -> ServerStep {
    match (setting, consent) {
        (true, true) => ServerStep::Start,
        (true, false) => ServerStep::Warn,
        (false, _) => ServerStep::Off,
    }
}

/// Which of ("Enable Python API", "Allow all apps to connect") differ from the values last acted
/// on, as their new values.
pub(super) fn changes(seen: (bool, bool), now: (bool, bool)) -> (Option<bool>, Option<bool>) {
    ((now.0 != seen.0).then_some(now.0), (now.1 != seen.1).then_some(now.1))
}

/// The alert holds a waiting connection.
fn holds_connection(a: &Alert) -> bool {
    matches!(a, Alert::Allow { .. } | Alert::ConfirmAlways { .. })
}

/// A stopped server closed every connection: none waits, and their alerts go.
pub(super) fn clear_on_stop<P, W>(pending: &mut std::collections::HashMap<u64, P>, alerts: &mut Vec<(Option<W>, Alert)>) {
    pending.clear();
    alerts.retain(|(_, a)| !holds_connection(a));
}

/// A connection that is gone: it waits no longer, its alerts go (dropping a showing one closes
/// its window), and one still deferred for authorization is dropped. What it waited with.
pub(super) fn forget_waiting<W>(conn: u64, pending: &mut std::collections::HashMap<u64, Pending>, alerts: &mut Vec<(Option<W>, Alert)>, deferred: &mut VecDeque<Event>) -> Option<Pending> {
    alerts.retain(|(_, a)| !matches!(a, Alert::Allow { conn: c } | Alert::ConfirmAlways { conn: c } if *c == conn));
    let mut gone = pending.remove(&conn);
    if let Some(i) = deferred.iter().position(|e| matches!(e, Event::Pending(p) if p.conn == conn))
        && let Some(Event::Pending(p)) = deferred.remove(i)
    {
        gone.get_or_insert(p);
    }
    gone
}

/// Server events handled in one pass at most, so a chatty script cannot starve input and
/// drawing.
pub(super) const EVENTS_PER_PASS: usize = 64;

/// Runs `step` until it has nothing left or `limit` times; true when the limit ended it (more
/// may wait).
pub(super) fn pump(limit: usize, step: &mut dyn FnMut() -> bool) -> bool {
    for _ in 0..limit {
        if !step() {
            return false;
        }
    }
    true
}

/// What answering an alert does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Effect {
    /// Record the consent, turn "Enable Python API" on and start the server.
    Enable,
    Verdict { conn: u64, allow: bool, reason: &'static str },
    OpenUrl(&'static str),
    /// Show Always's confirmation next.
    ConfirmAlways { conn: u64 },
    /// Turn `setCookie` on.
    SetCookie,
    WriteAllowAll,
    /// The popup shows the file again.
    Mirror,
    /// "Suppress this message permanently" for the reset warning.
    SilenceReset,
    /// Settings > General.
    RevealPreference,
    /// The allow-all file selected in Explorer.
    RevealFile,
}

/// The effects of button `index` (a closed window answers `close_index`'s button).
pub(super) fn effects(alert: &Alert, index: usize, suppress: bool) -> Vec<Effect> {
    use Effect::*;
    let verdict = |conn, allow, reason| Verdict { conn, allow, reason };
    match alert {
        // Cancel leaves the setting off (`H:647-649`).
        Alert::Enable => if index == 0 { vec![Enable] } else { vec![] },
        Alert::Allow { conn } => match api_auth::ask_answer(index) {
            AskAnswer::Allow => vec![verdict(*conn, true, api_auth::REASON_ALLOWED)],
            AskAnswer::Deny => vec![verdict(*conn, false, api_auth::REASON_DENIED)],
            AskAnswer::MoreInfo => vec![OpenUrl(api_auth::SECURITY_URL), verdict(*conn, false, api_auth::REASON_DENIED)],
            // The confirmation shows next; the connection keeps waiting.
            AskAnswer::Always => vec![ConfirmAlways { conn: *conn }],
        },
        Alert::ConfirmAlways { conn } => {
            if index == 0 { vec![SetCookie, verdict(*conn, true, api_auth::REASON_ALLOWED)] } else { vec![verdict(*conn, false, api_auth::REASON_DENIED)] }
        }
        // Cancel leaves `setCookie` off; the script was already answered OK.
        Alert::ConfirmCookie => if index == 0 { vec![SetCookie] } else { vec![] },
        Alert::AllowAll => match index {
            0 => vec![WriteAllowAll, Mirror],
            2 => vec![OpenUrl(api_auth::AUTH_URL), Mirror],
            _ => vec![Mirror],
        },
        // The checkbox is remembered only for OK: "Reveal Preference" is a one-time navigation
        // (`H:449-451`).
        Alert::Reset => match index {
            0 if suppress => vec![SilenceReset],
            1 => vec![RevealPreference],
            _ => vec![],
        },
        Alert::RemoveFailed(_) => if index == 1 { vec![RevealFile] } else { vec![] },
        Alert::MakeChangeFailed(_) | Alert::StartFailed(_) => vec![],
    }
}

/// The consent marker: a file beside the settings that only OK in "Enable Python API?" writes,
/// so no writer of the settings (an import, a key action, an edited file) can grant it.
fn consent_path() -> std::path::PathBuf {
    crate::globals::default_path().with_file_name("python-api-consent")
}

/// Writes the consent marker.
fn record_consent() -> std::io::Result<()> {
    let path = consent_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, b"confirmed\n")
}

/// The allow-all file's path, from `USERPROFILE`.
fn noauth_path() -> Option<String> {
    std::env::var("USERPROFILE").ok().filter(|p| !p.is_empty()).map(|p| api_auth::noauth_path(&p))
}

/// What the allow-all file says now, and what was found at its path.
fn noauth_now() -> (AllowAll, Option<api_auth::NoauthFile>) {
    let Some(path) = noauth_path() else { return (AllowAll::None, None) };
    let file = api_auth::read_noauth_file(&path);
    (api_auth::noauth_status(&path, file.as_ref()), file)
}

/// Writes the allow-all file as iTerm2 does (`H:538-560`): the contents go to a new temporary
/// file beside it, then moved over the path, so a link at the path is replaced, never written
/// through. With `set_owner` (an elevated Tandem, whose files are otherwise owned by
/// Administrators, which the check reads as no file) the temporary file is given the user as
/// its owner first.
pub(super) fn write_noauth_file(path: &str, set_owner: bool) -> Result<(), String> {
    use std::io::Write;
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{FILE_GENERIC_WRITE, READ_CONTROL, WRITE_OWNER};
    let target = std::path::Path::new(path);
    if let Some(dir) = target.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let temp = format!("{path}.{}.tmp", api_auth::cookie_text(*uuid::Uuid::new_v4().as_bytes()));
    let written = (|| {
        let access = if set_owner { FILE_GENERIC_WRITE | WRITE_OWNER | READ_CONTROL } else { FILE_GENERIC_WRITE };
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).access_mode(access).open(&temp).map_err(|e| e.to_string())?;
        if set_owner {
            crate::winpipe::set_file_owner_to_user(&f)?;
        }
        f.write_all(api_auth::noauth_contents(path).as_bytes()).map_err(|e| e.to_string())?;
        f.sync_all().map_err(|e| e.to_string())?;
        drop(f);
        std::fs::rename(&temp, target).map_err(|e| e.to_string())
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    written
}

/// What the dispatch loop needs of its host: the app, or a test's stand-in with a real server.
pub(super) trait Host {
    fn subs(&mut self) -> &mut Subscriptions;
    fn tx(&mut self) -> &mut Transactions;
    fn reply(&mut self, conn: u64, m: &pb::ServerOriginatedMessage);
    /// The answer to a request `route` gives the app.
    fn answer(&mut self, conn: u64, req: &pb::ClientOriginatedMessage) -> Answer;
    /// Blocks for the server's next event; None once the server is gone.
    fn wait_event(&mut self) -> Option<Event>;
    /// Keeps a connection to authorize or a refusal for after the freeze.
    fn defer(&mut self, e: Event);
    /// Forgets a closed connection; the requests its transaction's end released.
    fn forget(&mut self, conn: u64) -> Work;
    /// After answers were sent: acts on what the requests changed that may stop the server
    /// ("Enable Python API" off), so a request's own answer goes out first.
    fn settle(&mut self);
}

/// The app as the dispatch loop's host.
struct AppHost<'a> {
    app: &'a mut App,
    event_loop: &'a ActiveEventLoop,
}

impl Host for AppHost<'_> {
    fn subs(&mut self) -> &mut Subscriptions {
        &mut self.app.api_subs
    }

    fn tx(&mut self) -> &mut Transactions {
        &mut self.app.api_tx
    }

    fn reply(&mut self, conn: u64, m: &pb::ServerOriginatedMessage) {
        self.app.api_reply(conn, m);
    }

    fn answer(&mut self, conn: u64, req: &pb::ClientOriginatedMessage) -> Answer {
        self.app.api_answer(conn, req, self.event_loop)
    }

    fn wait_event(&mut self) -> Option<Event> {
        self.app.api_server.as_ref().and_then(Server::wait_event)
    }

    fn defer(&mut self, e: Event) {
        self.app.api_deferred.push_back(e);
    }

    fn forget(&mut self, conn: u64) -> Work {
        self.app.api_forget(conn)
    }

    fn settle(&mut self) {
        self.app.api_settings_changed();
    }
}

/// Offers `work` in order, then freezes while a transaction it began runs.
pub(super) fn serve(h: &mut impl Host, mut work: Work) {
    while let Some((conn, req)) = work.pop_front() {
        offer(h, conn, req, &mut work);
    }
    h.settle();
    if h.tx().owner().is_some() {
        freeze(h);
    }
}

/// One request through the transaction logic. An end's released requests join `work`, so
/// they are offered again in order (`S:703-714`): a held begin starts the next transaction
/// and the requests after it are held behind that one.
fn offer(h: &mut impl Host, conn: u64, req: pb::ClientOriginatedMessage, work: &mut Work) {
    match h.tx().offer(conn, req) {
        Tx::Run(req) => run(h, conn, req),
        Tx::Held => {}
        Tx::Began(m) | Tx::Reply(m) => h.reply(conn, &m),
        Tx::Ended(m) => {
            h.reply(conn, &m);
            work.extend(h.tx().release());
        }
    }
}

/// Dispatches a request: answered here, or by the app.
fn run(h: &mut impl Host, conn: u64, req: pb::ClientOriginatedMessage) {
    match crate::api_dispatch::route(h.subs(), conn, req) {
        Route::Reply(m) => h.reply(conn, &m),
        Route::App(req) => match h.answer(conn, &req) {
            Answer::Now(m) => h.reply(conn, &m),
            Answer::Later => {}
        },
    }
}

/// Blocks the UI thread while a transaction runs, as iTerm2's `drainTransaction` blocks its
/// main queue (`S:675-715`): nothing is drawn and no input is handled. Only the server's
/// events are taken: the owner's requests run, the others' are held, and connections to
/// authorize and refusals are deferred. It ends when the owner ends the transaction or its
/// connection closes (a script that dies mid-transaction always closes it); a released begin
/// from another connection freezes on in the same loop.
fn freeze(h: &mut impl Host) {
    let mut work = Work::new();
    while let Some(owner) = h.tx().owner() {
        match h.wait_event() {
            Some(Event::Message { conn, bytes }) => {
                if let Some(req) = crate::api_proto::decode(&bytes) {
                    work.push_back((conn, req));
                }
            }
            Some(Event::Closed { conn }) => work.extend(h.forget(conn)),
            Some(e @ (Event::Pending(_) | Event::Rejected { .. })) => h.defer(e),
            // The server is gone, and every connection with it.
            None => work.extend(h.forget(owner)),
        }
        while let Some((conn, req)) = work.pop_front() {
            offer(h, conn, req, &mut work);
        }
        h.settle();
    }
}

/// The model's answer to a request `route` gave the app; iTerm2's malformed error for one it
/// does not answer.
pub(super) fn answer_from(req: &pb::ClientOriginatedMessage, model: &api_model::Model) -> Answer {
    Answer::Now(api_model::answer(req, model).unwrap_or_else(|| crate::api_proto::malformed(req)))
}

/// Sends each notification to every connection subscribed to its type, in order.
pub(super) fn post(subs: &Subscriptions, notes: Vec<(pb::NotificationType, pb::Notification)>, send: &mut dyn FnMut(u64, &pb::ServerOriginatedMessage)) {
    for (kind, n) in notes {
        let conns = subs.subscribers(kind);
        if conns.is_empty() {
            continue;
        }
        let m = crate::api_proto::notification(n);
        for conn in conns {
            send(conn, &m);
        }
    }
}

impl App {
    /// At the first window: the popup mirrors the file and the server starts when the setting
    /// is on.
    pub(super) fn api_launch(&mut self) {
        self.api_consent = consent_path().is_file();
        self.api_settings_seen.0 = self.globals.bool(ENABLE_KEY);
        self.api_mirror_auth();
        self.sync_api_server();
    }

    /// The one start and stop point (`start_rule`): the server starts only when "Enable Python
    /// API" is on and its warning was confirmed; on without that turns the setting back off and
    /// asks (a declined warning leaves it off); off stops the server (every connection closed,
    /// `H:782-795`, dropped on another thread so the UI never waits for scripts to close) and
    /// forgets the consent. A failed start shows why and leaves the API off.
    pub(super) fn sync_api_server(&mut self) {
        match start_rule(self.globals.bool(ENABLE_KEY), self.api_consent) {
            ServerStep::Start if self.api_server.is_none() => {
                let jar = self.api_jar.clone();
                let cookie: crate::api_server::CookieCheck = Arc::new(move |c: &str| jar.lock().unwrap().consume(c));
                match Server::start(crate::api_http::PORT, Arc::new(crate::api_peer::check), cookie, self.notify.clone()) {
                    Ok(s) => self.api_server = Some(s),
                    Err(e) => {
                        let owner = port_owner(crate::api_http::PORT);
                        self.queue_api_alert(Alert::StartFailed(start_failed_text(&e, owner.as_deref())));
                    }
                }
            }
            ServerStep::Start => {}
            ServerStep::Warn => {
                // The checkbox shows off until OK, as iTerm2 turns it back off.
                self.set_api_enabled(false);
                self.queue_api_alert(Alert::Enable);
            }
            ServerStep::Off => {
                if let Some(s) = self.api_server.take() {
                    std::thread::spawn(move || drop(s));
                }
                clear_on_stop(&mut self.api_pending, &mut self.api_alerts);
                // No Closed event comes from a dropped server. Connection ids are never reused
                // (`api_server::NEXT_CONN`), but nothing of these connections is kept either.
                self.api_subs = Default::default();
                self.api_var_subs = Default::default();
                self.api_tx = Default::default();
                self.api_deferred.clear();
                if self.api_consent {
                    self.api_consent = false;
                    let _ = std::fs::remove_file(consent_path());
                }
            }
        }
        self.api_on = self.api_server.is_some();
    }

    /// Acts on "Enable Python API" and the authentication popup when either changed, whoever
    /// changed it: called after a Settings edit and on every pass (`GP:3512-3535`,
    /// `GP:2045-2053`).
    pub(super) fn api_settings_changed(&mut self) {
        // Nothing is acted on before the first window's `api_launch`.
        if self.gpu.is_none() {
            return;
        }
        let now = (self.globals.bool(ENABLE_KEY), self.globals.int(AUTH_KEY) == 1);
        let (enable, allow_all) = changes(self.api_settings_seen, now);
        if let Some(on) = enable {
            self.api_settings_seen.0 = on;
            self.sync_api_server();
        }
        if let Some(want_allow_all) = allow_all {
            match api_auth::allow_all_step(self.allow_all_checked(), want_allow_all) {
                AllowAllStep::Warn => self.queue_api_alert(Alert::AllowAll),
                AllowAllStep::Remove => self.remove_allow_all(),
                AllowAllStep::Nothing => {}
            }
            self.api_mirror_auth();
        }
    }

    /// The Settings window opens: a corrupt file is warned about, and the popup shows the file.
    pub(super) fn api_settings_opened(&mut self) {
        self.allow_all_checked();
        self.api_mirror_auth();
    }

    /// What the allow-all file says.
    pub(super) fn allow_all_state(&self) -> AllowAll {
        noauth_now().0
    }

    /// `allow_all_state`, queueing the reset warning for a corrupt file once per distinct
    /// contents, unless it was silenced (`H:432-464`).
    fn allow_all_checked(&mut self) -> AllowAll {
        let (state, file) = noauth_now();
        if state == AllowAll::Corrupt {
            let seen = file.map(|f| f.contents.unwrap_or_else(|| format!("({} bytes, unreadable)", f.size))).unwrap_or_default();
            if self.api_reset_seen.as_deref() != Some(seen.as_str()) {
                self.api_reset_seen = Some(seen);
                if !self.globals.bool(api_auth::RESET_ID) {
                    self.queue_api_alert(Alert::Reset);
                }
            }
        }
        state
    }

    /// Writes (on) or deletes (off) the allow-all file.
    pub(super) fn set_allow_all(&mut self, on: bool) -> Result<(), String> {
        let path = noauth_path().ok_or_else(|| "USERPROFILE is not set".to_string())?;
        if on { write_noauth_file(&path, crate::winpipe::elevated().unwrap_or(false)) } else { std::fs::remove_file(&path).map_err(|e| e.to_string()) }
    }

    /// The popup's value is never trusted: it shows the file, and is saved like any setting.
    fn api_mirror_auth(&mut self) {
        let allow_all = self.allow_all_state() == AllowAll::Valid;
        self.api_settings_seen.1 = allow_all;
        if self.globals.int(AUTH_KEY) != allow_all as i64 {
            self.globals.set(AUTH_KEY, toml::Value::Integer(allow_all as i64));
            self.save_globals();
        }
    }

    /// "Require a cookie" chosen over a valid file: delete it; a failure turns the API off and
    /// says so (`H:489-520`).
    fn remove_allow_all(&mut self) {
        let Err(e) = self.set_allow_all(false) else { return };
        self.set_api_enabled(false);
        self.sync_api_server();
        let path = noauth_path().unwrap_or_default();
        self.queue_api_alert(Alert::RemoveFailed(api_auth::remove_failed_text(&path, &e)));
    }

    /// Writes and saves "Enable Python API".
    fn set_api_enabled(&mut self, on: bool) {
        self.globals.set(ENABLE_KEY, toml::Value::Boolean(on));
        self.api_settings_seen.0 = on;
        self.save_globals();
    }

    /// SetPreference of `setCookie` on (`api_profile::PrefStep::Confirm`): the Confirm alert
    /// asks, and only its OK turns it on.
    pub(super) fn api_confirm_cookie(&mut self) {
        self.queue_api_alert(Alert::ConfirmCookie);
    }

    /// Queues an alert unless the same one already waits.
    fn queue_api_alert(&mut self, alert: Alert) {
        if !self.api_alerts.iter().any(|(_, a)| *a == alert) {
            self.api_alerts.push((None, alert));
        }
    }

    /// Takes the server's events: connections to authorize, messages, closes and refusals.
    pub(super) fn api_tick(&mut self, event_loop: &ActiveEventLoop) {
        // A setting written by any path (an import, a key action) is acted on here.
        self.api_settings_changed();
        // What a transaction deferred comes first. A bounded number per pass: when that many
        // were handled, another pass comes after input and drawing.
        let more = pump(EVENTS_PER_PASS, &mut || {
            let Some(event) = self.api_deferred.pop_front().or_else(|| self.api_server.as_ref().and_then(Server::try_event)) else { return false };
            match event {
                Event::Pending(p) => self.api_authorize(p),
                Event::Message { conn, bytes } => self.api_message(conn, bytes, event_loop),
                Event::Closed { conn } => self.api_closed(conn, event_loop),
                Event::Rejected { key, reason } => self.api_log.push(LogEntry { key, name: api_auth::display_name(None), text: reason }),
            }
            true
        });
        if more {
            (self.notify)();
        }
    }

    /// iTerm2's authorization (`H:1541-1583`): allowed or refused at once, or asked.
    fn api_authorize(&mut self, p: Pending) {
        let allow_all = self.allow_all_checked() == AllowAll::Valid;
        match api_auth::decide(p.upgrade.preauthorized, allow_all, p.upgrade.disable_auth_ui, self.globals.bool(SET_COOKIE_KEY)) {
            Verdict::Allow(reason) => self.api_verdict(&p, true, reason),
            Verdict::Deny(reason) => self.api_verdict(&p, false, reason),
            Verdict::Ask { .. } => {
                let conn = p.conn;
                self.api_pending.insert(conn, p);
                self.api_alerts.push((None, Alert::Allow { conn }));
            }
        }
    }

    /// Answers a connection and logs why.
    fn api_verdict(&mut self, p: &Pending, allow: bool, reason: &str) {
        if let Some(s) = &self.api_server {
            s.verdict(p.conn, allow);
        }
        self.api_log.push(LogEntry { key: Some(p.upgrade.key.clone()), name: api_auth::display_name(p.upgrade.advisory_name.as_deref()), text: reason.to_string() });
    }

    /// Answers a waiting connection, if it still waits.
    fn api_decided(&mut self, conn: u64, allow: bool, reason: &str) {
        if let Some(p) = self.api_pending.remove(&conn) {
            self.api_verdict(&p, allow, reason);
        }
    }

    /// A message from an upgraded connection. Undecodable frames are dropped (`S:1436-1447`);
    /// a request goes through the transaction logic (`S:1397-1411`).
    pub(super) fn api_message(&mut self, conn: u64, bytes: Vec<u8>, event_loop: &ActiveEventLoop) {
        if let Some(req) = crate::api_proto::decode(&bytes) {
            serve(&mut AppHost { app: self, event_loop }, VecDeque::from([(conn, req)]));
        }
    }

    /// An upgraded connection ended: its subscriptions, held requests and wait for a verdict
    /// go, and a transaction it owned ends (`S:1418-1424`, `H:2154-2192`), running what it
    /// held.
    pub(super) fn api_closed(&mut self, conn: u64, event_loop: &ActiveEventLoop) {
        let released = self.api_forget(conn);
        serve(&mut AppHost { app: self, event_loop }, released);
    }

    /// Forgets a closed connection; the requests its transaction's end released. The one
    /// per-connection cleanup: `api_closed` and the freeze (through `Host::forget`) both come
    /// here, so state keyed by connection is dropped here (and in `sync_api_server`'s Off
    /// reset). A connection gone before its verdict takes its Allow alert with it, answered
    /// Deny (logged as an internal error).
    fn api_forget(&mut self, conn: u64) -> Work {
        self.api_subs.drop_conn(conn);
        self.api_var_subs.drop_conn(conn);
        if let Some(p) = forget_waiting(conn, &mut self.api_pending, &mut self.api_alerts, &mut self.api_deferred) {
            self.api_verdict(&p, false, api_auth::REASON_INTERNAL);
        }
        self.api_tx.closed(conn).into()
    }

    /// The app's answer to a request `route` gave it: ListSessions, Focus and
    /// GetBroadcastDomains from a snapshot taken for the request, the rest from `api_request`.
    pub(super) fn api_answer(&mut self, conn: u64, req: &pb::ClientOriginatedMessage, event_loop: &ActiveEventLoop) -> Answer {
        use pb::client_originated_message::Submessage as Req;
        match &req.submessage {
            Some(Req::ListSessionsRequest(_) | Req::FocusRequest(_) | Req::GetBroadcastDomainsRequest(_)) => {
                let model = self.api_model(true);
                answer_from(req, &model)
            }
            _ => self.api_request(conn, req, event_loop),
        }
    }

    /// Sends a message to a connection.
    pub(super) fn api_reply(&self, conn: u64, m: &crate::api_proto::pb::ServerOriginatedMessage) {
        if let Some(s) = &self.api_server {
            s.send(conn, crate::api_proto::encode(m));
        }
    }

    /// Posts each watched variable that changed since the last pass to its subscribers
    /// (`H:1866-1877`). Objects no longer watched lose their baseline; nothing is built while
    /// no connection watches a variable.
    fn api_variable_tick(&mut self) {
        if !self.api_var_subs.any() {
            self.api_vars.keep_seen(&[]);
            return;
        }
        let objects = self.api_objects();
        let watched = self.api_var_subs.watched(&objects);
        let now: Vec<(api_vars::Scope, String, api_vars::Values)> = watched
            .iter()
            .map(|(s, id)| (*s, id.clone(), objects.values(*s, id)))
            .collect();
        self.api_vars.keep_seen(&watched);
        for (scope, id, values) in now {
            for c in self.api_vars.observe(scope, &id, values) {
                for (conn, identifier) in self.api_var_subs.subscribers(scope, &id, &c.name) {
                    let m = crate::api_proto::notification(api_vars::change_notification(scope, &identifier, &c.name, &c.json));
                    self.api_reply(conn, &m);
                }
            }
        }
    }

    /// Posts what changed since the last pass to the subscribers (`H:1000-1133`), variable
    /// changes first (`api_variable_tick`). Nothing is
    /// snapshotted while no connection subscribes; the first pass with a subscriber only takes
    /// the baseline. The snapshots compared carry no titles (no change reads them); titles
    /// are built only for a layout change, whose ListSessions carries them.
    pub(super) fn api_notify_tick(&mut self) {
        self.api_variable_tick();
        if !self.api_subs.any() {
            self.api_last = None;
            return;
        }
        let new = self.api_model(false);
        if let Some(old) = self.api_last.take() {
            let c = api_model::changes(&old, &new);
            let notes = if c.layout { api_model::notifications(&c, &self.api_model(true)) } else { api_model::notifications(&c, &new) };
            post(&self.api_subs, notes, &mut |conn, m| self.api_reply(conn, m));
        }
        self.api_last = Some(new);
    }

    /// The snapshot `async_get_app` reads: each window by number (entered to read its tabs,
    /// the current one restored after, as `snapshot_all` does), the buried sessions, and focus.
    /// Without `titles`, every session's title is left empty.
    pub(super) fn api_model(&mut self, titles: bool) -> api_model::Model {
        let back = self.current_window();
        let mut windows = Vec::new();
        let mut key_window = None;
        for k in self.registry.order() {
            let Some(id) = self.window_id_of_key(k) else { continue };
            if !self.enter(id) {
                continue;
            }
            let w = self.api_window(k, titles);
            if self.focused && self.app_active {
                key_window = Some(w.id.clone());
            }
            windows.push(w);
        }
        if let Some(b) = back {
            self.enter(b);
        }
        let buried = self.buried.items().into_iter().map(|(p, title)| api_model::ApiSession { id: p.to_string(), frame: None, grid: None, title: if titles { title } else { String::new() } }).collect();
        let last = self.last_window.and_then(|w| self.registry.guid(super::windows_ui::key(w))).map(str::to_string);
        let current_window = api_model::live_window(&windows, last);
        api_model::Model { windows, buried, app_active: self.app_active, key_window, current_window }
    }

    /// The current window as the snapshot shows it: `k` is its registry key.
    fn api_window(&self, k: u64, titles: bool) -> api_model::ApiWindow {
        let frame = self.gpu.as_ref().map_or((0, 0, 0, 0), |g| {
            let p = g.window.outer_position().unwrap_or_default();
            let s = g.window.outer_size();
            (p.x, p.y, s.width as i32, s.height as i32)
        });
        let domains = api_model::domains(&self.broadcast, &self.broadcast_view()).into_iter().map(|d| d.into_iter().map(|p| p.to_string()).collect()).collect();
        api_model::ApiWindow {
            id: self.registry.guid(k).unwrap_or_default().to_string(),
            frame,
            number: self.registry.number(k).unwrap_or(0) as i32,
            selected_tab: self.tabs.get(self.active).map(|t| t.id),
            tabs: self.tabs.iter().map(|t| self.api_tab(t, titles)).collect(),
            domains,
        }
    }

    /// A tab of the current window: pane frames in window pixels, grids from each pane's own
    /// screen, names as the tab bar shows them, and the tmux window and client of its active
    /// pane.
    fn api_tab(&self, tab: &super::Tab, titles: bool) -> api_model::ApiTab {
        use alacritty_terminal::grid::Dimensions;
        let rects: std::collections::HashMap<_, _> = self.visible(tab).into_iter().collect();
        let session = |p: crate::layout::PaneId| api_model::ApiSession {
            id: p.to_string(),
            frame: rects.get(&p).map(|r| (r.x.round() as i32, r.y.round() as i32, r.w.round() as i32, r.h.round() as i32)),
            grid: self.panes.get(&p).map(|pane| {
                let g = pane.own().term().grid();
                (g.columns() as i32, g.screen_lines() as i32)
            }),
            title: if titles { self.session_title(p) } else { String::new() },
        };
        let (root, minimized) = api_model::node_of(&tab.layout.tree(), tab.zoomed, &session);
        let tmux = self.tmux_of(tab.focused).and_then(|(gateway, _, window)| Some((self.tmux.get(&gateway)?.client.clone(), window)));
        api_model::ApiTab {
            id: tab.id,
            root,
            active: tab.layout.panes().contains(&tab.focused).then(|| tab.focused.to_string()),
            tmux_window: tmux.as_ref().map_or_else(|| "-1".to_string(), |(_, w)| w.to_string()),
            tmux_connection: tmux.map(|(client, _)| client),
            minimized,
        }
    }

    /// Opens the first queued alert when none shows. An Allow alert whose connection no longer
    /// waits is dropped; one that cannot open refuses its connection.
    pub(super) fn open_api_alerts(&mut self, event_loop: &ActiveEventLoop) {
        if self.api_alerts.iter().any(|(w, _)| w.is_some()) {
            return;
        }
        self.api_alerts.retain(|(_, a)| match a {
            Alert::Allow { conn } | Alert::ConfirmAlways { conn } => self.api_pending.contains_key(conn),
            _ => true,
        });
        let Some((_, alert)) = self.api_alerts.first() else { return };
        let Some(g) = self.gpu.as_ref() else { return };
        let f = form(alert, self.globals.bool(SET_COOKIE_KEY));
        match ConfirmWindow::actions(event_loop, &g.instance, &g.adapter, &g.device, &g.queue, f.heading, &f.text, &f.labels, f.silenceable) {
            Ok(w) => self.api_alerts[0].0 = Some(w),
            Err(e) => {
                let (_, alert) = self.api_alerts.remove(0);
                if let Alert::Allow { conn } | Alert::ConfirmAlways { conn } = alert {
                    self.api_decided(conn, false, api_auth::REASON_INTERNAL);
                }
                self.banners.set("window", format!("Could not open the window: {e}"));
            }
        }
    }

    /// Events for an API alert; a button (or closing it) answers it. False for other windows.
    pub(super) fn api_alert_event(&mut self, event_loop: &ActiveEventLoop, window: WindowId, event: &WindowEvent) -> bool {
        let Some(i) = self.api_alerts.iter().position(|(w, _)| w.as_ref().is_some_and(|w| w.id() == window)) else { return false };
        let Some(w) = self.api_alerts[i].0.as_mut() else { return false };
        let answer = match event {
            WindowEvent::CloseRequested => w.close_answer(),
            WindowEvent::RedrawRequested => w.redraw(),
            other => {
                w.event(other);
                ConfirmResult::Open
            }
        };
        let (index, suppress) = match answer {
            ConfirmResult::Open => return true,
            ConfirmResult::Action { index, suppress } => (index, suppress),
            // An `actions` window answers only Action.
            ConfirmResult::Cancel | ConfirmResult::Ok { .. } => (usize::MAX, false),
        };
        let (_, alert) = self.api_alerts.remove(i);
        self.api_alert_answered(alert, index, suppress, event_loop);
        true
    }

    fn api_alert_answered(&mut self, alert: Alert, index: usize, suppress: bool, event_loop: &ActiveEventLoop) {
        for effect in effects(&alert, index, suppress) {
            match effect {
                Effect::Enable => {
                    // The consent is what lets the server start, now and at later launches.
                    if let Err(e) = record_consent() {
                        self.banners.set("api", format!("Could not record the Python API choice: {e}"));
                    }
                    self.api_consent = true;
                    self.set_api_enabled(true);
                    self.sync_api_server();
                }
                Effect::Verdict { conn, allow, reason } => self.api_decided(conn, allow, reason),
                Effect::OpenUrl(url) => self.open_link(url),
                Effect::ConfirmAlways { conn } => self.api_alerts.insert(0, (None, Alert::ConfirmAlways { conn })),
                Effect::SetCookie => {
                    self.globals.set(SET_COOKIE_KEY, toml::Value::Boolean(true));
                    self.save_globals();
                }
                Effect::WriteAllowAll => {
                    if let Err(e) = self.set_allow_all(true) {
                        self.queue_api_alert(Alert::MakeChangeFailed(e));
                    }
                }
                Effect::Mirror => self.api_mirror_auth(),
                Effect::SilenceReset => {
                    self.globals.set(api_auth::RESET_ID, toml::Value::Boolean(true));
                    self.save_globals();
                }
                Effect::RevealPreference => self.open_settings_page(event_loop, crate::settings_ui::Page::General),
                Effect::RevealFile => self.reveal_allow_all_file(),
            }
        }
    }

    /// A "More Info" page in the browser.
    fn open_link(&mut self, url: &str) {
        if let Err(e) = super::semantic_ui::shell_open(url, None, false) {
            self.banners.set("action", format!("Could not open {url}: {e}"));
        }
    }

    /// "Show in Explorer": the allow-all file selected; nothing runs it.
    fn reveal_allow_all_file(&mut self) {
        let Some(path) = noauth_path() else { return };
        let path = path.replace('/', "\\");
        let mut cmd = std::process::Command::new("explorer.exe");
        {
            use std::os::windows::process::CommandExt;
            cmd.raw_arg(format!("/select,{}", crate::semantic::cmd_quote(&path)));
        }
        if let Err(e) = cmd.spawn() {
            self.banners.set("action", format!("Could not show {path}: {e}"));
        }
    }
}

/// The process listening on `port`, named, when it can be found.
fn port_owner(port: u16) -> Option<String> {
    let pid = crate::api_peer::listener(&crate::api_peer::listen_rows().ok()?, port)?;
    Some(crate::api_peer::process_label(crate::api_peer::process_name(pid).as_deref(), pid))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api_auth::*;

    #[test]
    fn alerts_follow_iterm2() {
        let enable = form(&Alert::Enable, false);
        assert_eq!((enable.heading, enable.text.as_str(), enable.labels.as_slice(), enable.silenceable), (ENABLE_HEADING, ENABLE_TEXT, &[OK, CANCEL][..], false));
        let allow = form(&Alert::Allow { conn: 1 }, false);
        assert_eq!((allow.heading, allow.text.as_str(), allow.labels.as_slice(), allow.silenceable), (ALLOW_HEADING, ALLOW_TEXT, &[OK, CANCEL, MORE_INFO, ALWAYS][..], false));
        let allow = form(&Alert::Allow { conn: 1 }, true);
        assert_eq!((allow.text.as_str(), allow.labels.as_slice()), (format!("{ALLOW_TEXT}{NO_COOKIE_SUFFIX}").as_str(), &[OK, CANCEL, MORE_INFO][..]));
        let confirm = form(&Alert::ConfirmAlways { conn: 1 }, false);
        assert_eq!((confirm.heading, confirm.text.as_str(), confirm.labels.as_slice(), confirm.silenceable), (CONFIRM_HEADING, CONFIRM_ALWAYS, &[OK, CANCEL][..], false));
        let all = form(&Alert::AllowAll, false);
        assert_eq!((all.heading, all.text.as_str(), all.labels.as_slice(), all.silenceable), (ALLOW_ALL_HEADING, ALLOW_ALL_TEXT, &[OK, CANCEL, MORE_INFO][..], false));
        let reset = form(&Alert::Reset, false);
        assert_eq!((reset.heading, reset.text.as_str(), reset.labels.as_slice(), reset.silenceable), (RESET_HEADING, RESET_TEXT, &[OK, REVEAL_PREFERENCE][..], true));
        let removed = form(&Alert::RemoveFailed("t".into()), false);
        assert_eq!((removed.heading, removed.text.as_str(), removed.labels.as_slice(), removed.silenceable), (CHANGE_FAILED_HEADING, "t", &[OK, SHOW_IN_EXPLORER][..], false));
        let made = form(&Alert::MakeChangeFailed("denied".into()), false);
        assert_eq!((made.heading, made.text.as_str(), made.labels.as_slice()), (MAKE_CHANGE_FAILED_HEADING, "The setting could not be changed: denied", &[OK][..]));
        let start = form(&Alert::StartFailed(start_failed_text("[::1] port 1912: in use", Some("python.exe (pid 9)"))), false);
        assert_eq!(start.text, "The Python API could not start: [::1] port 1912: in use\n\nPort 1912 is in use by python.exe (pid 9).");
        assert_eq!(start_failed_text("x", None), "The Python API could not start: x");
        // Closing: Escape's button (Cancel, the Deny answer), else the last one.
        assert_eq!(crate::prompt_ui::action_keys(&allow.labels).1, Some(1));
        assert_eq!(ask_answer(1), AskAnswer::Deny);
    }

    #[test]
    fn start_rule_needs_consent() {
        assert_eq!(start_rule(true, true), ServerStep::Start);
        assert_eq!(start_rule(true, false), ServerStep::Warn);
        assert_eq!(start_rule(false, true), ServerStep::Off);
        assert_eq!(start_rule(false, false), ServerStep::Off);
    }

    #[test]
    fn settings_changes_are_tracked() {
        assert_eq!(changes((false, false), (false, false)), (None, None));
        assert_eq!(changes((false, false), (true, false)), (Some(true), None));
        assert_eq!(changes((true, true), (true, false)), (None, Some(false)));
        assert_eq!(changes((true, false), (false, true)), (Some(false), Some(true)));
    }

    #[test]
    fn stop_clears_waiting_connections() {
        let mut pending: std::collections::HashMap<u64, ()> = [(1, ()), (2, ())].into_iter().collect();
        let mut alerts: Vec<(Option<()>, Alert)> =
            vec![(Some(()), Alert::Allow { conn: 1 }), (None, Alert::Enable), (None, Alert::ConfirmAlways { conn: 2 }), (None, Alert::Reset)];
        clear_on_stop(&mut pending, &mut alerts);
        assert!(pending.is_empty());
        assert_eq!(alerts.into_iter().map(|(_, a)| a).collect::<Vec<_>>(), [Alert::Enable, Alert::Reset]);
    }

    fn waiting(conn: u64) -> Pending {
        let upgrade = crate::api_http::Upgrade {
            key: format!("K{conn}"),
            ws_key: String::new(),
            ws_version: 13,
            preauthorized: false,
            advisory_name: None,
            library_version: "python 2.24".into(),
            disable_auth_ui: false,
        };
        Pending { conn, upgrade, peer_pid: 1 }
    }

    #[test]
    fn a_gone_connection_takes_its_question_with_it() {
        let mut pending: std::collections::HashMap<u64, Pending> = [(1, waiting(1)), (2, waiting(2))].into_iter().collect();
        let mut alerts: Vec<(Option<()>, Alert)> =
            vec![(Some(()), Alert::Allow { conn: 1 }), (None, Alert::Enable), (None, Alert::ConfirmAlways { conn: 1 }), (None, Alert::Allow { conn: 2 })];
        let mut deferred: VecDeque<Event> = VecDeque::from([Event::Pending(waiting(3)), Event::Rejected { key: None, reason: "r".into() }]);
        // The showing Allow alert and the queued confirmation go; the rest stay.
        assert_eq!(forget_waiting(1, &mut pending, &mut alerts, &mut deferred).map(|p| p.conn), Some(1));
        assert_eq!(alerts.iter().map(|(_, a)| a.clone()).collect::<Vec<_>>(), [Alert::Enable, Alert::Allow { conn: 2 }]);
        assert!(pending.contains_key(&2) && !pending.contains_key(&1));
        // One still deferred (it closed during a transaction) is dropped before it is asked.
        assert_eq!(forget_waiting(3, &mut pending, &mut alerts, &mut deferred).map(|p| p.conn), Some(3));
        assert!(matches!(deferred.iter().collect::<Vec<_>>()[..], [Event::Rejected { .. }]));
        // An upgraded connection waits for nothing.
        assert!(forget_waiting(9, &mut pending, &mut alerts, &mut deferred).is_none());
        assert_eq!(alerts.len(), 2);
    }

    #[test]
    fn events_are_handled_in_bounded_passes() {
        let mut left = 150;
        let mut step = || {
            if left == 0 {
                return false;
            }
            left -= 1;
            true
        };
        assert!(pump(EVENTS_PER_PASS, &mut step), "more may wait: another pass");
        assert!(pump(EVENTS_PER_PASS, &mut step));
        assert!(!pump(EVENTS_PER_PASS, &mut step), "drained");
        assert_eq!(left, 0);
        assert!(!pump(EVENTS_PER_PASS, &mut || false));
    }

    #[test]
    fn alert_answers_follow_iterm2() {
        use Effect::*;
        let closed = |a: &Alert| effects(a, crate::prompt_ui::close_index(&form(a, false).labels), false);
        let deny = |conn| Verdict { conn, allow: false, reason: REASON_DENIED };
        // Enable: OK turns the API on; Cancel or closing leaves it off.
        assert_eq!(effects(&Alert::Enable, 0, false), [Enable]);
        assert_eq!(closed(&Alert::Enable), []);
        // Allow: OK, Cancel, More Info, Always; closing denies.
        let allow = Alert::Allow { conn: 4 };
        assert_eq!(effects(&allow, 0, false), [Verdict { conn: 4, allow: true, reason: REASON_ALLOWED }]);
        assert_eq!(effects(&allow, 1, false), [deny(4)]);
        assert_eq!(effects(&allow, 2, false), [OpenUrl(SECURITY_URL), deny(4)]);
        assert_eq!(effects(&allow, 3, false), [ConfirmAlways { conn: 4 }]);
        assert_eq!(closed(&allow), [deny(4)]);
        // Confirm: OK turns setCookie on and allows; Cancel or closing denies.
        let confirm = Alert::ConfirmAlways { conn: 4 };
        assert_eq!(effects(&confirm, 0, false), [SetCookie, Verdict { conn: 4, allow: true, reason: REASON_ALLOWED }]);
        assert_eq!(closed(&confirm), [deny(4)]);
        // A script's SetPreference of setCookie: the same Confirm text; only OK turns it on.
        assert_eq!(form(&Alert::ConfirmCookie, false), form(&confirm, false));
        assert_eq!(effects(&Alert::ConfirmCookie, 0, false), [SetCookie]);
        assert_eq!(effects(&Alert::ConfirmCookie, 1, false), []);
        assert_eq!(closed(&Alert::ConfirmCookie), []);
        // Allow all: OK writes the file, More Info opens the page; the popup mirrors after.
        assert_eq!(effects(&Alert::AllowAll, 0, false), [WriteAllowAll, Mirror]);
        assert_eq!(effects(&Alert::AllowAll, 2, false), [OpenUrl(AUTH_URL), Mirror]);
        assert_eq!(closed(&Alert::AllowAll), [Mirror]);
        // Reset: the checkbox counts only with OK; closing does nothing.
        assert_eq!(effects(&Alert::Reset, 0, true), [SilenceReset]);
        assert_eq!(effects(&Alert::Reset, 0, false), []);
        assert_eq!(effects(&Alert::Reset, 1, true), [RevealPreference]);
        assert_eq!(closed(&Alert::Reset), []);
        // Remove failed: Show in Explorer reveals; closing does nothing.
        assert_eq!(effects(&Alert::RemoveFailed("t".into()), 1, false), [RevealFile]);
        assert_eq!(closed(&Alert::RemoveFailed("t".into())), []);
        assert_eq!(closed(&Alert::MakeChangeFailed("e".into())), []);
        assert_eq!(closed(&Alert::StartFailed("e".into())), []);
    }

    #[test]
    fn allow_all_file_is_written_as_the_package_reads_it() {
        let dir = std::env::temp_dir().join(format!("tandem-noauth-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = format!("{}/Library/Application Support/iTerm2/disable-automation-auth", dir.display());
        // The owner step (an elevated Tandem's) works for a normal one too.
        write_noauth_file(&path, true).unwrap();
        let file = read_noauth_file(&path).unwrap();
        assert!(file.owner_is_user);
        assert_eq!(noauth_status(&path, Some(&file)), AllowAll::Valid);
        // A second write, setting the owner only when elevated as `set_allow_all` does,
        // replaces the file, leaving no temporary file behind.
        std::fs::write(&path, "stale").unwrap();
        write_noauth_file(&path, crate::winpipe::elevated().unwrap()).unwrap();
        assert_eq!(noauth_status(&path, read_noauth_file(&path).as_ref()), AllowAll::Valid);
        let parent = std::path::Path::new(&path).parent().unwrap();
        assert_eq!(std::fs::read_dir(parent).unwrap().count(), 1);
        // A directory at the path is not replaced.
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert!(write_noauth_file(&path, false).is_err());
        assert_eq!(std::fs::read_dir(parent).unwrap().count(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The dispatch loop's host with a real server on an ephemeral loopback port, answering
    /// from the fixture model, and a log of every reply it sent.
    struct TestHost {
        server: Option<Server>,
        subs: Subscriptions,
        tx: Transactions,
        model: api_model::Model,
        deferred: Vec<Event>,
        replies: Vec<(u64, Option<i64>)>,
        /// Told the connection of every message the loop takes, so a client can wait until
        /// its request was taken.
        seen: std::sync::mpsc::Sender<u64>,
        /// The request whose answer turns the API off, as SetPreference of EnableAPIServer
        /// false does; the stop comes when the loop settles.
        stop_on: Option<i64>,
        stop_wanted: bool,
    }

    impl TestHost {
        fn new(server: Server, model: api_model::Model, seen: std::sync::mpsc::Sender<u64>) -> TestHost {
            TestHost { server: Some(server), subs: Default::default(), tx: Default::default(), model, deferred: Vec::new(), replies: Vec::new(), seen, stop_on: None, stop_wanted: false }
        }

        fn srv(&self) -> &Server {
            self.server.as_ref().expect("the server runs")
        }
    }

    impl Host for TestHost {
        fn subs(&mut self) -> &mut Subscriptions {
            &mut self.subs
        }

        fn tx(&mut self) -> &mut Transactions {
            &mut self.tx
        }

        fn reply(&mut self, conn: u64, m: &pb::ServerOriginatedMessage) {
            self.replies.push((conn, m.id));
            if let Some(s) = &self.server {
                s.send(conn, crate::api_proto::encode(m));
            }
        }

        fn answer(&mut self, _conn: u64, req: &pb::ClientOriginatedMessage) -> Answer {
            if req.id.is_some() && req.id == self.stop_on {
                self.stop_wanted = true;
            }
            answer_from(req, &self.model)
        }

        fn wait_event(&mut self) -> Option<Event> {
            let e = self.server.as_ref().and_then(Server::wait_event);
            if let Some(Event::Message { conn, .. }) = &e {
                let _ = self.seen.send(*conn);
            }
            e
        }

        fn defer(&mut self, e: Event) {
            self.deferred.push(e);
        }

        fn forget(&mut self, conn: u64) -> Work {
            self.subs.drop_conn(conn);
            self.tx.closed(conn).into()
        }

        fn settle(&mut self) {
            if std::mem::take(&mut self.stop_wanted) {
                if let Some(s) = self.server.take() {
                    s.stop();
                }
            }
        }
    }

    type Ws = tungstenite::WebSocket<std::net::TcpStream>;

    /// A websocket client with the stock package's headers, upgraded by the host.
    fn connect(h: &TestHost) -> (u64, Ws) {
        use tungstenite::client::IntoClientRequest;
        let addr = h.srv().addrs()[0];
        let mut req = format!("ws://{addr}/").into_client_request().unwrap();
        let hd = req.headers_mut();
        hd.insert("Sec-WebSocket-Protocol", "api.iterm2.com".parse().unwrap());
        hd.insert("origin", "ws://localhost/".parse().unwrap());
        hd.insert("x-iterm2-library-version", "python 2.24".parse().unwrap());
        let stream = std::net::TcpStream::connect(addr).unwrap();
        let hs = std::thread::spawn(move || tungstenite::client::client(req, stream).map_err(|e| e.to_string()));
        let conn = match next_event(h) {
            Event::Pending(p) => p.conn,
            e => panic!("{e:?}"),
        };
        h.srv().verdict(conn, true);
        let (ws, _) = hs.join().unwrap().unwrap();
        ws.get_ref().set_read_timeout(Some(std::time::Duration::from_secs(10))).unwrap();
        (conn, ws)
    }

    fn next_event(h: &TestHost) -> Event {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            if let Some(e) = h.srv().try_event() {
                return e;
            }
            assert!(std::time::Instant::now() < deadline, "no event");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    fn send(ws: &mut Ws, id: i64, sub: pb::client_originated_message::Submessage) {
        let m = pb::ClientOriginatedMessage { id: Some(id), submessage: Some(sub) };
        ws.send(tungstenite::Message::binary(prost::Message::encode_to_vec(&m))).unwrap();
    }

    fn read(ws: &mut Ws) -> pb::ServerOriginatedMessage {
        loop {
            match ws.read().unwrap() {
                tungstenite::Message::Binary(b) => return prost::Message::decode(&b[..]).unwrap(),
                tungstenite::Message::Ping(_) | tungstenite::Message::Pong(_) => {}
                m => panic!("{m:?}"),
            }
        }
    }

    /// The real dispatch loop (`serve`, the freeze included) over a real server: a
    /// transaction holds another connection's request until it ends, and a notification
    /// reaches only the subscribed connection.
    #[test]
    fn dispatch_loop_holds_and_posts() {
        use pb::client_originated_message::Submessage as Req;
        use pb::server_originated_message::Submessage as Resp;
        let server = Server::start(0, Arc::new(|_: std::net::SocketAddr, _: std::net::SocketAddr| Ok::<u32, String>(1)), Arc::new(|_: &str| false), Arc::new(|| {})).unwrap();
        let model = api_model::fixture();
        let (seen, taken) = std::sync::mpsc::channel();
        let mut h = TestHost::new(server, model.clone(), seen);
        let (a, mut wa) = connect(&h);
        let (b, mut wb) = connect(&h);
        let list = || Req::ListSessionsRequest(pb::ListSessionsRequest {});
        let tx = |begin| Req::TransactionRequest(pb::TransactionRequest { begin: Some(begin) });
        let script = std::thread::spawn(move || {
            send(&mut wa, 1, tx(true));
            let r = read(&mut wa);
            assert_eq!((r.id, r.submessage), (Some(1), Some(Resp::TransactionResponse(pb::TransactionResponse { status: Some(0) }))));
            // B's request arrives inside the transaction; A's own request runs.
            send(&mut wb, 10, list());
            send(&mut wa, 2, list());
            assert!(matches!(read(&mut wa), pb::ServerOriginatedMessage { id: Some(2), submessage: Some(Resp::ListSessionsResponse(_)) }));
            // The end goes only once the frozen loop has taken B's request.
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            loop {
                let left = deadline.saturating_duration_since(std::time::Instant::now());
                if taken.recv_timeout(left).expect("the loop never took B's request") == b {
                    break;
                }
            }
            send(&mut wa, 3, tx(false));
            assert_eq!(read(&mut wa).id, Some(3));
            // B's held request is answered after the end.
            assert!(matches!(read(&mut wb), pb::ServerOriginatedMessage { id: Some(10), submessage: Some(Resp::ListSessionsResponse(_)) }));
            // B subscribes; the notification reaches B and not A (A's next message is its
            // own answer).
            let notif = Req::NotificationRequest(pb::NotificationRequest {
                session: Some("all".into()),
                subscribe: Some(true),
                notification_type: Some(pb::NotificationType::NotifyOnNewSession as i32),
                arguments: None,
            });
            send(&mut wb, 11, notif);
            assert_eq!(read(&mut wb).id, Some(11));
            let n = read(&mut wb);
            match n.submessage {
                Some(Resp::Notification(n)) => assert_eq!(n.new_session_notification.and_then(|s| s.session_id).as_deref(), Some("10")),
                other => panic!("{other:?}"),
            }
            assert_eq!(n.id, None);
            send(&mut wa, 4, Req::FocusRequest(pb::FocusRequest {}));
            assert!(matches!(read(&mut wa), pb::ServerOriginatedMessage { id: Some(4), submessage: Some(Resp::FocusResponse(_)) }));
        });
        // The loop the app runs: each message through `serve`; the begin freezes it until
        // the end.
        let message = |h: &mut TestHost| match next_event(h) {
            Event::Message { conn, bytes } => serve(h, VecDeque::from([(conn, crate::api_proto::decode(&bytes).unwrap())])),
            e => panic!("{e:?}"),
        };
        message(&mut h);
        assert_eq!(h.replies, [(a, Some(1)), (a, Some(2)), (a, Some(3)), (b, Some(10))]);
        assert_eq!(h.tx.owner(), None);
        message(&mut h);
        // A new session, posted as the app's notification tick posts it.
        let mut new = model.clone();
        new.windows[1].tabs[0].root = api_model::node_of(
            &crate::layout::Tree::Split { axis: crate::layout::Axis::Row, ratio: 0.5, a: Box::new(crate::layout::Tree::Leaf(7)), b: Box::new(crate::layout::Tree::Leaf(10)) },
            None,
            &api_model::fixture_session,
        )
        .0;
        let notes = api_model::notifications(&api_model::changes(&model, &new), &new);
        assert!(notes.len() > 1, "a layout change as well, to nobody");
        let server = h.srv();
        post(&h.subs, notes, &mut |conn, m| server.send(conn, crate::api_proto::encode(m)));
        message(&mut h);
        script.join().unwrap();
        assert!(h.deferred.is_empty());
    }

    /// A request that turns the API off (SetPreference of EnableAPIServer false) is answered
    /// first; the server stops when the loop settles, the script sees its answer and then
    /// the connection end, and the loop returns. Inside a transaction too: the stop ends the
    /// freeze.
    #[test]
    fn turning_the_api_off_answers_then_stops() {
        use pb::client_originated_message::Submessage as Req;
        for in_transaction in [false, true] {
            let server = Server::start(0, Arc::new(|_: std::net::SocketAddr, _: std::net::SocketAddr| Ok::<u32, String>(1)), Arc::new(|_: &str| false), Arc::new(|| {})).unwrap();
            let (seen, _taken) = std::sync::mpsc::channel();
            let mut h = TestHost::new(server, api_model::fixture(), seen);
            h.stop_on = Some(7);
            let (a, mut wa) = connect(&h);
            let script = std::thread::spawn(move || {
                if in_transaction {
                    send(&mut wa, 1, Req::TransactionRequest(pb::TransactionRequest { begin: Some(true) }));
                    assert_eq!(read(&mut wa).id, Some(1));
                }
                send(&mut wa, 7, Req::FocusRequest(pb::FocusRequest {}));
                assert_eq!(read(&mut wa).id, Some(7));
                // Then the connection ends: a close frame or the socket closing.
                loop {
                    match wa.read() {
                        Ok(tungstenite::Message::Close(_)) | Err(_) => break,
                        Ok(_) => {}
                    }
                }
            });
            let first = match next_event(&h) {
                Event::Message { conn, bytes } => (conn, crate::api_proto::decode(&bytes).unwrap()),
                e => panic!("{e:?}"),
            };
            serve(&mut h, VecDeque::from([first]));
            assert!(h.server.is_none(), "stopped");
            assert_eq!(h.tx.owner(), None);
            assert_eq!(h.replies.last(), Some(&(a, Some(7))));
            script.join().unwrap();
        }
    }
}
