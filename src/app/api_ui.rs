//! The Python API's app side: the server follows "Enable Python API", the authentication
//! popup mirrors the allow-all file, and the API's alerts ("Enable Python API?", "Allow Python
//! API Usage?" and the allow-all warnings) are shown one at a time. Events from the server are
//! pumped on each pass (`H:629-656`, `H:782-795`, `H:388-560`, `H:1541-1619`).

use std::sync::Arc;

use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::WindowId;

use super::App;
use crate::api_auth::{self, AllowAll, AllowAllStep, AskAnswer, EnableStep, LogEntry, Verdict};
use crate::api_server::{Event, Pending, Server};
use crate::prompt_ui::{ConfirmResult, ConfirmWindow};

/// The settings this module follows.
pub(super) const ENABLE_KEY: &str = "EnableAPIServer";
pub(super) const AUTH_KEY: &str = "API Authentication Method";
pub(super) const SET_COOKIE_KEY: &str = "setCookie";

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

/// An alert waiting its turn or showing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Alert {
    /// "Enable Python API?" from the Settings checkbox (forced: never silenced).
    Enable,
    /// "Allow Python API Usage?" for a waiting connection.
    Allow { conn: u64 },
    /// Always's "Confirm".
    ConfirmAlways { conn: u64 },
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
        Alert::ConfirmAlways { .. } => f(CONFIRM_HEADING, CONFIRM_ALWAYS.into(), &[OK, CANCEL], false),
        Alert::AllowAll => f(ALLOW_ALL_HEADING, ALLOW_ALL_TEXT.into(), &[OK, CANCEL, MORE_INFO], false),
        Alert::Reset => f(RESET_HEADING, RESET_TEXT.into(), &[OK, REVEAL_PREFERENCE], true),
        Alert::RemoveFailed(text) => f(CHANGE_FAILED_HEADING, text.clone(), &[OK, SHOW_IN_EXPLORER], false),
        Alert::MakeChangeFailed(e) => f(MAKE_CHANGE_FAILED_HEADING, change_failed_text(e), &[OK], false),
        Alert::StartFailed(text) => f(START_FAILED_HEADING, text.clone(), &[OK], false),
    }
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
/// file beside it, owned by the user (an elevated Tandem's files are otherwise owned by
/// Administrators, which the check reads as no file), then moved over the path, so a link at
/// the path is replaced, never written through.
pub(super) fn write_noauth_file(path: &str) -> Result<(), String> {
    use std::io::Write;
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{FILE_GENERIC_WRITE, READ_CONTROL, WRITE_OWNER};
    let target = std::path::Path::new(path);
    if let Some(dir) = target.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let temp = format!("{path}.{}.tmp", api_auth::cookie_text(*uuid::Uuid::new_v4().as_bytes()));
    let written = (|| {
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).access_mode(FILE_GENERIC_WRITE | WRITE_OWNER | READ_CONTROL).open(&temp).map_err(|e| e.to_string())?;
        crate::winpipe::set_file_owner_to_user(f.as_raw_handle())?;
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

impl App {
    /// At the first window: the popup mirrors the file and the server starts when the setting
    /// is on.
    pub(super) fn api_launch(&mut self) {
        self.api_settings_seen.0 = self.globals.bool(ENABLE_KEY);
        self.api_mirror_auth();
        self.sync_api_server();
    }

    /// Starts the server when "Enable Python API" is on and it does not run; stops it (every
    /// connection closed, `H:782-795`) when the setting is off. A failed start shows why and
    /// leaves the API off.
    pub(super) fn sync_api_server(&mut self) {
        let want = self.globals.bool(ENABLE_KEY);
        if want && self.api_server.is_none() {
            let jar = self.api_jar.clone();
            let cookie: crate::api_server::CookieCheck = Arc::new(move |c: &str| jar.lock().unwrap().consume(c));
            match Server::start(crate::api_http::PORT, Arc::new(crate::api_peer::check), cookie, self.notify.clone()) {
                Ok(s) => self.api_server = Some(s),
                Err(e) => {
                    let owner = port_owner(crate::api_http::PORT);
                    self.queue_api_alert(Alert::StartFailed(start_failed_text(&e, owner.as_deref())));
                }
            }
        } else if !want && self.api_server.is_some() {
            self.api_server = None;
            self.api_pending.clear();
            self.api_alerts.retain(|(_, a)| !matches!(a, Alert::Allow { .. } | Alert::ConfirmAlways { .. }));
        }
        self.api_on = self.api_server.is_some();
    }

    /// A global setting changed in the Settings window: acts on "Enable Python API" and the
    /// authentication popup when they are what changed (`GP:3512-3535`, `GP:2045-2053`).
    pub(super) fn api_settings_changed(&mut self) {
        let enable = self.globals.bool(ENABLE_KEY);
        if enable != self.api_settings_seen.0 {
            self.api_settings_seen.0 = enable;
            match api_auth::enable_step(enable, self.api_on) {
                EnableStep::Confirm => {
                    // The checkbox shows off until OK, as iTerm2 turns it back off.
                    self.set_api_enabled(false);
                    self.queue_api_alert(Alert::Enable);
                }
                EnableStep::Stop => self.sync_api_server(),
                EnableStep::Nothing => {}
            }
        }
        let want_allow_all = self.globals.int(AUTH_KEY) == 1;
        if want_allow_all != self.api_settings_seen.1 {
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
        if on { write_noauth_file(&path) } else { std::fs::remove_file(&path).map_err(|e| e.to_string()) }
    }

    /// The popup's value is never trusted: it shows the file.
    fn api_mirror_auth(&mut self) {
        let allow_all = self.allow_all_state() == AllowAll::Valid;
        self.api_settings_seen.1 = allow_all;
        if self.globals.int(AUTH_KEY) != allow_all as i64 {
            self.globals.set(AUTH_KEY, toml::Value::Integer(allow_all as i64));
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

    /// Queues an alert unless the same one already waits.
    fn queue_api_alert(&mut self, alert: Alert) {
        if !self.api_alerts.iter().any(|(_, a)| *a == alert) {
            self.api_alerts.push((None, alert));
        }
    }

    /// Takes the server's events: connections to authorize, messages, closes and refusals.
    pub(super) fn api_tick(&mut self, event_loop: &ActiveEventLoop) {
        while let Some(event) = self.api_server.as_ref().and_then(Server::try_event) {
            match event {
                Event::Pending(p) => self.api_authorize(p),
                Event::Message { conn, bytes } => self.api_message(conn, bytes, event_loop),
                Event::Closed { conn } => self.api_closed(conn),
                Event::Rejected { key, reason } => self.api_log.push(LogEntry { key, name: api_auth::display_name(None), text: reason }),
            }
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
    /// every request answers iTerm2's malformed error until the dispatcher (Task 7).
    pub(super) fn api_message(&mut self, conn: u64, bytes: Vec<u8>, _event_loop: &ActiveEventLoop) {
        if let Some(req) = crate::api_proto::decode(&bytes) {
            self.api_reply(conn, &crate::api_proto::malformed(&req));
        }
    }

    /// An upgraded connection ended.
    pub(super) fn api_closed(&mut self, conn: u64) {
        self.api_pending.remove(&conn);
    }

    /// Sends a message to a connection.
    pub(super) fn api_reply(&self, conn: u64, m: &crate::api_proto::pb::ServerOriginatedMessage) {
        if let Some(s) = &self.api_server {
            s.send(conn, crate::api_proto::encode(m));
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
        match alert {
            Alert::Enable => {
                // Cancel leaves the setting off (`H:647-649`).
                if index == 0 {
                    self.set_api_enabled(true);
                    self.sync_api_server();
                }
            }
            Alert::Allow { conn } => match api_auth::ask_answer(index) {
                AskAnswer::Allow => self.api_decided(conn, true, api_auth::REASON_ALLOWED),
                AskAnswer::Deny => self.api_decided(conn, false, api_auth::REASON_DENIED),
                AskAnswer::MoreInfo => {
                    self.open_link(api_auth::SECURITY_URL);
                    self.api_decided(conn, false, api_auth::REASON_DENIED);
                }
                // The confirmation shows next; the connection keeps waiting.
                AskAnswer::Always => self.api_alerts.insert(0, (None, Alert::ConfirmAlways { conn })),
            },
            Alert::ConfirmAlways { conn } => {
                if index == 0 {
                    self.globals.set(SET_COOKIE_KEY, toml::Value::Boolean(true));
                    self.save_globals();
                    self.api_decided(conn, true, api_auth::REASON_ALLOWED);
                } else {
                    self.api_decided(conn, false, api_auth::REASON_DENIED);
                }
            }
            Alert::AllowAll => {
                match index {
                    0 => {
                        if let Err(e) = self.set_allow_all(true) {
                            self.queue_api_alert(Alert::MakeChangeFailed(e));
                        }
                    }
                    2 => self.open_link(api_auth::AUTH_URL),
                    _ => {}
                }
                self.api_mirror_auth();
            }
            Alert::Reset => match index {
                // The checkbox is remembered only for OK: "Reveal Preference" is a one-time
                // navigation (`H:449-451`).
                0 if suppress => {
                    self.globals.set(api_auth::RESET_ID, toml::Value::Boolean(true));
                    self.save_globals();
                }
                1 => self.open_settings_page(event_loop, crate::settings_ui::Page::General),
                _ => {}
            },
            Alert::RemoveFailed(_) => {
                if index == 1 {
                    self.reveal_allow_all_file();
                }
            }
            Alert::MakeChangeFailed(_) | Alert::StartFailed(_) => {}
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
    fn allow_all_file_is_written_as_the_package_reads_it() {
        let dir = std::env::temp_dir().join(format!("tandem-noauth-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = format!("{}/Library/Application Support/iTerm2/disable-automation-auth", dir.display());
        write_noauth_file(&path).unwrap();
        let file = read_noauth_file(&path).unwrap();
        assert!(file.owner_is_user);
        assert_eq!(noauth_status(&path, Some(&file)), AllowAll::Valid);
        // A second write replaces the file, leaving no temporary file behind.
        std::fs::write(&path, "stale").unwrap();
        write_noauth_file(&path).unwrap();
        assert_eq!(noauth_status(&path, read_noauth_file(&path).as_ref()), AllowAll::Valid);
        let parent = std::path::Path::new(&path).parent().unwrap();
        assert_eq!(std::fs::read_dir(parent).unwrap().count(), 1);
        // A directory at the path is not replaced.
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert!(write_noauth_file(&path).is_err());
        assert_eq!(std::fs::read_dir(parent).unwrap().count(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
