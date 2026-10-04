//! The OSC 1337 keys that ask first, in the window: SetProfile, SetProfileProperty, and
//! SetBackgroundImageFile, with iTerm2's bars and stored answers.

use base64::Engine;

use super::*;
use crate::prompted::{self, Decision};

pub(super) const ASK_PROFILE: &str = "AskAboutChangingProfile";
pub(super) const ASK_PROPERTY: &str = "SetProfileProperty";
pub(super) const ASK_BACKGROUND: &str = "SetBackgroundImageFile";
pub(super) const ASK_VARIABLE: &str = "ShouldReportVariable";
pub(super) const ASK_URL: &str = "NoSyncAllowOpenURL_host:";
pub(super) const UPLOAD_FORMAT: &str = "UploadInUnsupportedFormatRequested";
pub(super) const ASK_ABORT_UPLOAD: &str = "AbortUploadOnKeyPressAnnouncement";
/// Windows cannot choose files and folders in one dialog, so RequestUpload asks which first.
pub(super) const UPLOAD_KIND: &str = "UploadChooseFilesOrFolders";


/// An upload being sent to a pane.
pub(super) struct Upload {
    data: Vec<u8>,
    sent: usize,
    name: String,
    shown_pct: usize,
}

fn b64_text(s: &str) -> Option<String> {
    let bytes = base64::engine::general_purpose::STANDARD.decode(s.trim()).ok()?;
    String::from_utf8(bytes).ok()
}

impl App {
    fn global_list(&self, key: &str) -> Vec<String> {
        self.globals.get(key).and_then(|v| v.as_array().cloned()).unwrap_or_default().iter().filter_map(|v| v.as_str().map(str::to_string)).collect()
    }

    fn push_global_list(&mut self, key: &str, item: &str) {
        let mut list = self.global_list(key);
        if !list.iter().any(|x| x == item) {
            list.push(item.to_string());
        }
        self.globals.set(key, toml::Value::Array(list.into_iter().map(toml::Value::String).collect()));
        self.save_globals();
    }

    /// SetProfile: the named saved profile (exact; empty is the default) replaces the pane's.
    pub(super) fn set_profile_request(&mut self, pane: PaneId, name: String) {
        let prevent = self.globals.optional_bool("PreventEscapeSequenceFromChangingProfile");
        match prompted::set_profile_decision(prevent) {
            Decision::Deny => {}
            Decision::Ask => {
                self.ask(
                    pane,
                    ASK_PROFILE,
                    "A control sequence attempted to change the current profile. Allow this in the future?",
                    &["Always _Allow", "Always _Deny"],
                    false,
                );
            }
            Decision::Apply => {
                let guid = if name.is_empty() {
                    Some(self.store.default_guid.clone())
                } else {
                    self.store.profiles.iter().find(|p| p.name() == name).map(|p| p.guid())
                };
                if let Some(guid) = guid {
                    self.remarry(pane, &guid);
                }
            }
        }
    }

    /// SetProfileProperty: every pair must be legal, or none applies.
    pub(super) fn set_property_request(&mut self, pane: PaneId, pairs: Vec<(String, String)>) {
        let decoded: Vec<(String, serde_json::Value)> =
            pairs.into_iter().filter_map(|(k, v)| Some((k, serde_json::from_str(&b64_text(&v)?).ok()?))).collect();
        if decoded.is_empty() {
            return;
        }
        let Some(values): Option<Vec<(String, toml::Value)>> = decoded.iter().map(|(k, j)| Some((k.clone(), prompted::valid(k, j)?))).collect() else { return };
        let keys: Vec<String> = values.iter().map(|(k, _)| k.clone()).collect();
        let stored = |k: &str| self.globals.values.get(&format!("NoSyncSetProfileProperty_{k}")).and_then(|v| v.as_integer());
        match prompted::property_decision(&keys, stored) {
            Decision::Deny => {}
            Decision::Apply => self.apply_properties(pane, values),
            Decision::Ask => {
                if self.ask(pane, ASK_PROPERTY, &prompted::property_text(&keys), &["_Allow Once", "Allow Always", "Deny Always"], false) {
                    self.pending_properties.insert(pane, values);
                }
            }
        }
    }

    fn apply_properties(&mut self, pane: PaneId, values: Vec<(String, toml::Value)>) {
        let values: Vec<(String, Option<toml::Value>)> = values.into_iter().map(|(k, v)| (k, Some(v))).collect();
        self.set_session_values(pane, &values);
    }

    /// SetBackgroundImageFile: a path (empty removes the image) as the pane's own setting.
    pub(super) fn set_background_request(&mut self, pane: PaneId, value: String) {
        let Some(path) = b64_text(&value).map(|p| p.trim_end().to_string()) else { return };
        // Network paths are refused before anything touches them (a lookup alone would
        // send the user's credentials to the server).
        if !path.is_empty() && (!prompted::local_path_ok(&path) || !std::path::Path::new(&path).exists()) {
            return;
        }
        match prompted::background_decision(&path, &self.global_list("AlwaysAllowBackgroundImage"), &self.global_list("AlwaysDenyBackgroundImage")) {
            Decision::Deny => {}
            Decision::Apply => self.apply_background(pane, &path),
            Decision::Ask => {
                if self.ask(pane, ASK_BACKGROUND, &prompted::background_text(&path), &["Yes", "Always", "Never"], false) {
                    self.pending_background.insert(pane, path.clone());
                }
            }
        }
    }

    fn apply_background(&mut self, pane: PaneId, path: &str) {
        // Folder rotation ignores a single image, as iTerm2 does.
        if self.profile_of(pane).is_some_and(|p| p.int("Background Image Source Mode") == 1) {
            return;
        }
        let value = (!path.is_empty()).then(|| toml::Value::String(path.to_string()));
        self.set_session_values(pane, &[("Background Image Location".to_string(), value)]);
    }

    /// The answers to these bars. True when the bar was one of them.
    pub(super) fn prompted_bar(&mut self, pane: PaneId, id: &str, button: Option<&str>) -> bool {
        match (id, button) {
            (ASK_PROFILE, Some(b)) => {
                self.globals.set("PreventEscapeSequenceFromChangingProfile", toml::Value::Boolean(b.contains("Deny")));
                self.save_globals();
            }
            (ASK_PROPERTY, answer) => {
                let Some(values) = self.pending_properties.remove(&pane) else { return true };
                match answer {
                    Some("_Allow Once") => self.apply_properties(pane, values),
                    Some(b @ ("Allow Always" | "Deny Always")) => {
                        let n = if b == "Allow Always" { 0 } else { 1 };
                        for (k, _) in &values {
                            self.globals.set(&format!("NoSyncSetProfileProperty_{k}"), toml::Value::Integer(n));
                        }
                        self.save_globals();
                        if n == 0 {
                            self.apply_properties(pane, values);
                        }
                    }
                    _ => {}
                }
            }
            (ASK_BACKGROUND, answer) => {
                let Some(path) = self.pending_background.remove(&pane) else { return true };
                match answer {
                    Some("Yes") => self.apply_background(pane, &path),
                    Some("Always") => {
                        self.push_global_list("AlwaysAllowBackgroundImage", &path);
                        self.apply_background(pane, &path);
                    }
                    Some("Never") => self.push_global_list("AlwaysDenyBackgroundImage", &path),
                    _ => {}
                }
            }
            (ASK_PROFILE, None) => {}
            (UPLOAD_FORMAT, _) => {}
            (UPLOAD_KIND, Some(b @ ("Files" | "Folders"))) => self.upload_pick(pane, b == "Folders"),
            (UPLOAD_KIND, _) => {
                // Dismissed: the program waits for an answer, so it is cancelled.
                if let Some(p) = self.panes.get_mut(&pane) {
                    p.reply(prompted::UPLOAD_CANCEL);
                }
            }
            (ASK_ABORT_UPLOAD, answer) => {
                if answer == Some("OK") {
                    self.uploads.remove(&pane);
                    self.show_toast("Upload aborted");
                }
            }
            (id, answer) if id.starts_with(ASK_VARIABLE) => {
                let name = &id[ASK_VARIABLE.len()..];
                if let Some(b) = answer {
                    let word = if b == "Always Allow" { "allow" } else { "deny" };
                    let list = self.globals.string("NoSyncVariablesToReport");
                    let joined = if list.is_empty() { format!("{word}:{name}") } else { format!("{list},{word}:{name}") };
                    self.globals.set("NoSyncVariablesToReport", toml::Value::String(joined));
                    self.save_globals();
                }
            }
            (id, answer) if id.starts_with(ASK_URL) => {
                let host = id[ASK_URL.len()..].to_string();
                let Some(url) = self.pending_urls.remove(&(pane, host.clone())) else { return true };
                match answer {
                    Some("Allow") => self.open_url(pane, &url),
                    Some("Always allow for this host") => {
                        self.globals.set(&prompted::host_key(&host), toml::Value::Boolean(true));
                        self.save_globals();
                        self.open_url(pane, &url);
                    }
                    Some("Never allow") => {
                        // iTerm2's Never turns the sequence off for every host.
                        self.globals.set("NoSyncDisableOpenURL", toml::Value::Boolean(true));
                        self.save_globals();
                    }
                    _ => {}
                }
            }
            _ => return false,
        }
        true
    }

    /// A key press while the focused pane shows a bar: Alt+letter presses that button, and
    /// any other key dismisses a bar that is not important. True when the key was used.
    pub(super) fn bar_key(&mut self, ev: &KeyEvent, event_loop: &ActiveEventLoop) -> bool {
        let Some(pane) = self.focused_id() else { return false };
        let Some(bar) = self.bars.get(&pane).and_then(|b| b.front()).cloned() else { return false };
        // Modifier keys alone do nothing.
        if keystroke::key_of(&ev.logical_key, ev.text.as_deref()).is_none() {
            return false;
        }
        let pressed = if crate::infobar::shortcut_mods(self.mods.alt, self.mods.ctrl, self.mods.shift, self.mods.cmd) {
            let letter = match &ev.key_without_modifiers() {
                winit::keyboard::Key::Character(c) => c.chars().next(),
                _ => None,
            };
            letter.and_then(|l| bar.shortcut(l))
        } else {
            None
        };
        if pressed.is_none() && bar.important {
            return false;
        }
        if let Some(b) = self.bars.get_mut(&pane) {
            b.pop();
        }
        self.bar_answered(pane, &bar.id, pressed.map(|i| bar.buttons[i].as_str()), event_loop);
        self.redraw();
        true
    }

    /// OpenURL: opens at once for an allowed host, else asks.
    pub(super) fn open_url_request(&mut self, pane: PaneId, value: String) {
        if self.globals.bool("NoSyncDisableOpenURL") {
            return;
        }
        let Some(url) = prompted::open_url(&value) else { return };
        let host = prompted::url_host(&url).unwrap_or_default();
        if !host.is_empty() && self.globals.bool(&prompted::host_key(&host)) {
            return self.open_url(pane, &url);
        }
        if self.ask(pane, &format!("{ASK_URL}{host}"), &format!("Open this URL? {url}"), &["Allow", "Always allow for this host", "Never allow"], true) {
            self.pending_urls.insert((pane, host.clone()), url.clone());
        }
    }

    pub(super) fn open_url(&mut self, pane: PaneId, url: &str) {
        // The same guard as a Win-click: web and mail links open; others are refused. A remote
        // pane's text never opens a local file (P10).
        let run = if self.remote_text(pane) && !crate::semantic::web_link(url) {
            crate::semantic::Run::Blocked(format!("Tandem does not open {url} for a remote host."))
        } else {
            crate::semantic::guard_open(url, false)
        };
        self.execute(pane, run, None, false);
    }

    /// ReportVariable: the value if the name is allowed; empty now and a question if not.
    pub(super) fn report_variable_request(&mut self, pane: PaneId, value: String) {
        let Some(name) = b64_text(&value) else { return };
        // Names go into the allow and deny list, so only plain ones are looked up or asked about.
        if !prompted::variable_name_ok(&name) {
            if let Some(p) = self.panes.get_mut(&pane) {
                p.reply(&prompted::variable_reply(None));
            }
            return;
        }
        let decision = prompted::variable_decision(&name, &self.globals.string("NoSyncVariablesToReport"));
        let reply = match decision {
            Decision::Apply => {
                let vars = self.variables(pane);
                prompted::variable_reply(prompted::variable(&name, &vars).as_deref())
            }
            _ => prompted::variable_reply(None),
        };
        if let Some(p) = self.panes.get_mut(&pane) {
            p.reply(&reply);
        }
        if decision == Decision::Ask {
            self.ask(
                pane,
                &format!("{ASK_VARIABLE}{name}"),
                &format!("A request to report variable \u{201c}{name}\u{201d} was denied. Allow it in the future?"),
                &["Always Allow", "Always Deny"],
                true,
            );
        }
    }

    /// The session's variables, as ReportVariable names them.
    pub(super) fn variables(&self, pane: PaneId) -> prompted::Vars {
        self.variables_with(pane, true)
    }

    /// The tmux status bar's lines of a tmux pane's window (iTermTmuxStatusBarMonitor):
    /// status-left and status-right, empty before the first reply. None off tmux.
    pub(super) fn tmux_status_of(&self, pane: PaneId) -> Option<(String, String)> {
        let io = self.panes.get(&pane)?.tmux_io()?;
        let window = self.tmux.get(&io.gateway).and_then(|t| t.panes.get(&io.pane)).map(|(_, w)| *w);
        Some(window.and_then(|w| self.tmux_status.get(&(io.gateway, w))).cloned().unwrap_or_default())
    }

    /// The session's variables; `selection` and `selectionLength` only with `selection`,
    /// since building them copies the whole selection.
    pub(super) fn variables_with(&self, pane: PaneId, selection: bool) -> prompted::Vars {
        self.session_variables(pane, selection, true)
    }

    /// The session's variables; with `titles` off the tab's and window's title overrides are
    /// their formats, unevaluated (what a title's own evaluation reads, so it never recurses).
    fn session_variables(&self, pane: PaneId, selection: bool, titles: bool) -> prompted::Vars {
        let mut v = prompted::Vars::default();
        let Some(p) = self.panes.get(&pane) else { return v };
        let s = p.own();
        let title = s.title().unwrap_or("Shell").to_string();
        let grid = s.term().grid();
        use alacritty_terminal::grid::Dimensions;
        v.session.insert("name", title.clone());
        v.session.insert("presentationName", title);
        v.session.insert("id", pane.to_string());
        self.scope_variables(pane, &mut v, titles);
        v.session.insert("columns", grid.columns().to_string());
        v.session.insert("rows", grid.screen_lines().to_string());
        v.session.insert("showingAlternateScreen", if s.term().mode().contains(TermMode::ALT_SCREEN) { "1" } else { "0" }.to_string());
        if let Some(c) = &s.shell.cwd {
            v.session.insert("path", c.clone());
        }
        if let Some(h) = &s.shell.host {
            v.session.insert("hostname", h.clone());
        }
        // Whether the session is on this computer (no reported host yet counts).
        let local = super::statusbar_content::is_localhost(s.shell.host.as_deref());
        v.session.insert("isLocalhost", if local { "1" } else { "0" }.to_string());
        // Ruling I-1: the one detector of the session's shell. The launch program is the
        // shell's own process once the window has seen it, else the pane's command line.
        let launch = s.shell.job_root.clone().or_else(|| Some(crate::proctree::argv0_title(&p.command).to_string()).filter(|n| !n.is_empty()));
        let local_shell = crate::ai_terminal::detected_shell(s.shell.shell.as_deref(), launch.as_deref(), local).unwrap_or_default();
        // `PS:25624-25660`: the SSH level, and the home directory, shell and uname of the local
        // computer, none (level 1) or the remote's (level 2, made safe there).
        let local_home = std::env::var("USERPROFILE").unwrap_or_default();
        for (name, value) in crate::conductor_framer::session_values(p.conductor.as_deref(), &local_home, &local_shell) {
            v.session.insert(name, value);
        }
        if let Some(u) = &s.shell.user {
            v.session.insert("username", u.clone());
        }
        if let Some(j) = &s.shell.job {
            v.session.insert("jobName", j.name.clone());
            v.session.insert("deepestJob", j.name.clone());
            v.session.insert("processTitle", j.title.clone());
            v.session.insert("commandLine", j.cmdline.clone());
            v.session.insert("jobPid", j.pid.to_string());
        }
        v.session.insert("foregroundJobAncestors", s.shell.job_ancestors.join("\n"));
        // A tmux pane: tmux's monitors (iTerm2's tmuxPaneTitle, jobName, and path).
        if let Some(io) = p.tmux_io() {
            let info = io.info.borrow();
            v.session.insert("tmuxPaneTitle", info.title.clone());
            if !info.job.is_empty() {
                v.session.insert("jobName", info.job.clone());
            }
            if !info.cwd.is_empty() && s.shell.cwd.is_none() {
                v.session.insert("path", info.cwd.clone());
            }
            // The tmux status bar's lines of the pane's window (iTermTmuxStatusBarMonitor).
            let (left, right) = self.tmux_status_of(pane).unwrap_or_default();
            v.session.insert("tmuxStatusLeft", left);
            v.session.insert("tmuxStatusRight", right);
        }
        if let Some(pid) = p.shell_pid() {
            v.session.insert("pid", pid.to_string());
            v.session.insert("effective_root_pid", pid.to_string());
        }
        if let Some(c) = s.shell.history.last() {
            v.session.insert("lastCommand", c.command.clone());
        }
        if let Some(sel) = selection.then(|| s.term().selection_to_string()).flatten() {
            let (capped, length) = capped_selection(&sel, self.globals.int("maximumBytesToProvideToPythonAPI"));
            v.session.insert("selectionLength", length.to_string());
            v.session.insert("selection", capped);
        }
        if let Some(pr) = self.store.get(&p.profile) {
            v.session.insert("profileName", pr.name());
        }
        for (k, val) in &s.shell.vars {
            v.user.insert(k.clone(), val.clone());
        }
        // Values the API set win over SetUserVar's until the script sets them null.
        v.user.extend(user_strings(self.api_vars.user(crate::api_vars::Scope::Session, &pane.to_string())));
        v.app.insert("pid", std::process::id().to_string());
        v.app.insert("localhostName", local_host_name().to_string());
        v.app.insert("effectiveTheme", if self.dark() { "dark" } else { "light" }.to_string());
        v.app_user = user_strings(self.api_vars.user(crate::api_vars::Scope::App, ""));
        v
    }

    /// The tab and window variables of the pane's window (iTerm2's tab and window scopes),
    /// and the session's `termid`. A buried pane has neither. `titleOverrideFormat` is Edit
    /// Tab Title's or Edit Window Title's format, `titleOverride` and `title` its evaluated
    /// text (`titles`; else the format).
    fn scope_variables(&self, pane: PaneId, v: &mut prompted::Vars, titles: bool) {
        use crate::api_vars::Scope;
        let Some(w) = self.pane_window(pane) else { return };
        let number = self.registry.number(w.key).unwrap_or(0);
        let guid = self.registry.guid(w.key).unwrap_or_default().to_string();
        let t = w.tab;
        if let Some(tab) = w.tabs.get(t) {
            let index = tab.layout.panes().iter().position(|x| *x == pane).unwrap_or(0);
            v.session.insert("termid", format!("w{number}t{t}p{index}"));
            v.tab.insert("id", tab.id.to_string());
            let own = if titles { self.tab_title_text(tab) } else { tab.title.clone() };
            v.tab.insert("title", self.tmux_tab_label(tab.focused).or_else(|| own.clone().filter(|t| !t.is_empty())).unwrap_or_else(|| self.session_title(tab.focused)));
            // A tmux tab's Edit Tab Title text is kept by the gateway (its `title` holds the
            // tmux window's name).
            let tmux = self.tmux_window_of(tab.focused);
            // For a tmux tab the gateway keeps the evaluated text, so the "format" is that text.
            let (over, format) = match tmux {
                Some((gw, win)) => {
                    let text = self.tmux.get(&gw).and_then(|g| g.tab_overrides.get(&win)).cloned();
                    (text.clone(), text)
                }
                None => (own, tab.title.clone()),
            };
            v.tab.insert("titleOverride", over.unwrap_or_default());
            v.tab.insert("titleOverrideFormat", format.unwrap_or_default());
            v.tab.insert("tmuxWindow", tmux.map_or_else(|| "-1".to_string(), |(_, win)| win.to_string()));
            v.tab.insert("window", guid.clone());
            v.tab_user = user_strings(self.api_vars.user(Scope::Tab, &tab.id.to_string()));
        }
        let front = w.front.map(|t| t.focused);
        let over = match w.title {
            Some(f) if titles => Some(self.window_title_text(f, front)),
            other => other.map(str::to_string),
        };
        let title = match over.as_deref().filter(|t| !t.is_empty()) {
            Some(t) => t.to_string(),
            None => front.map_or_else(|| "Tandem".to_string(), |f| self.window_title(f)),
        };
        v.window.insert("id", guid.clone());
        // iTerm2's window number variable counts from 1 (`PseudoTerminal.m:866`).
        v.window.insert("number", (number + 1).to_string());
        v.window.insert("title", title);
        v.window.insert("titleOverride", over.unwrap_or_default());
        v.window.insert("titleOverrideFormat", w.title.unwrap_or_default().to_string());
        v.window.insert("isHotkeyWindow", if w.hotkey { "1" } else { "0" }.to_string());
        v.window_user = user_strings(self.api_vars.user(Scope::Window, &guid));
    }

    /// The names a window's title format reads, for the window whose current tab's focused
    /// session is `pane`: the window scope, and that session's names under
    /// `currentTab.currentSession.` (`iTermVariableScope+Window.m`). Gathered on the first
    /// lookup, with the titles unevaluated.
    pub(super) fn window_title_lookup(&self, pane: PaneId) -> impl Fn(&str) -> Option<String> + '_ {
        self.title_lookup(pane, crate::api_vars::Scope::Window, crate::wintitle::WINDOW_SESSION)
    }

    /// The names a tab's title format reads, for the tab whose focused session is `pane`: the
    /// tab scope, and that session's names under `currentSession.` (`iTermVariableScope+Tab.m`).
    pub(super) fn tab_title_lookup(&self, pane: PaneId) -> impl Fn(&str) -> Option<String> + '_ {
        self.title_lookup(pane, crate::api_vars::Scope::Tab, crate::wintitle::TAB_SESSION)
    }

    fn title_lookup(&self, pane: PaneId, scope: crate::api_vars::Scope, session_prefix: &'static str) -> impl Fn(&str) -> Option<String> + '_ {
        use crate::api_vars::{Scope, scope_values};
        let values = std::cell::OnceCell::new();
        move |name: &str| {
            let (own, session) = values.get_or_init(|| {
                let v = self.session_variables(pane, false, false);
                (scope_values(&v, scope), scope_values(&v, Scope::Session))
            });
            crate::wintitle::scoped_value(name, own, session, session_prefix)
        }
    }

    /// A window title format (Edit Window Title's or the profile's) evaluated in the window
    /// whose current tab's focused session is `front`.
    pub(super) fn window_title_text(&self, format: &str, front: Option<PaneId>) -> String {
        let lax = self.globals.bool("laxNilPolicyInInterpolatedStrings");
        match front {
            Some(pane) => crate::wintitle::evaluate(format, &self.window_title_lookup(pane), lax),
            None => crate::wintitle::evaluate(format, &|_: &str| None, lax),
        }
    }

    /// A tab's title override (Edit Tab Title's or the profile's) as it shows: a format
    /// evaluated in the tab's scope; a tmux tab's (the tmux window's name) as it is.
    pub(super) fn tab_title_text(&self, t: &super::Tab) -> Option<String> {
        let format = t.title.as_deref()?;
        if self.tmux_window_of(t.focused).is_some() {
            return Some(format.to_string());
        }
        let lax = self.globals.bool("laxNilPolicyInInterpolatedStrings");
        Some(crate::wintitle::evaluate(format, &self.tab_title_lookup(t.focused), lax))
    }

    /// The tmux gateway and tmux window id of a tmux pane.
    fn tmux_window_of(&self, pane: PaneId) -> Option<(PaneId, u32)> {
        self.tmux_of(pane).map(|(gateway, _, window)| (gateway, window))
    }

    /// RequestUpload: ask whether files or folders are chosen (`upload_pick` goes on).
    pub(super) fn upload_request(&mut self, pane: PaneId, params: String) {
        if !prompted::upload_format_ok(&params) {
            self.ask(pane, UPLOAD_FORMAT, "An upload with an unsupported archive format was requested. You may need a newer version of iTerm2.", &[], false);
            return;
        }
        // A bar refused (too many waiting) cancels, so the program never waits for nothing.
        if !self.ask(pane, UPLOAD_KIND, "Upload files or folders?", &["Files", "Folders"], false) {
            if let Some(p) = self.panes.get_mut(&pane) {
                p.reply(prompted::UPLOAD_CANCEL);
            }
        }
    }

    /// The Files/Folders answer: pick them, answer, and send the archive.
    pub(super) fn upload_pick(&mut self, pane: PaneId, folders: bool) {
        let dialog = rfd::FileDialog::new();
        let picked = if folders { dialog.set_title("Choose folders to upload").pick_folders() } else { dialog.set_title("Choose files to upload").pick_files() };
        let Some(paths) = picked.filter(|p| !p.is_empty()) else {
            if let Some(p) = self.panes.get_mut(&pane) {
                p.reply(prompted::UPLOAD_CANCEL);
            }
            return;
        };
        if let Some(p) = self.panes.get_mut(&pane) {
            p.reply(prompted::UPLOAD_OK);
        }
        match prompted::upload_archive(&paths) {
            Ok(archive) => {
                let first = paths[0].file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let name = if paths.len() > 1 { format!("{first} plus {} more", paths.len() - 1) } else { first };
                self.uploads.insert(pane, Upload { data: prompted::upload_lines(&archive), sent: 0, name, shown_pct: 0 });
                self.upload_due = Some(Instant::now());
            }
            Err(e) => {
                // The program already has "ok"; as in iTerm2 it waits until the user presses Ctrl+C.
                files_ui::message(&format!("Error Preparing Upload\n\nThe archive could not be made: {e}"));
            }
        }
    }

    /// Sends the next piece of each upload; says when the next piece is due.
    pub(super) fn feed_uploads(&mut self) -> Option<Instant> {
        if self.uploads.is_empty() {
            return None;
        }
        let now = Instant::now();
        if self.upload_due.is_some_and(|t| now < t) {
            return self.upload_due;
        }
        let pace = prompted::upload_pace(self.globals.bool("accelerateUploads"), self.paste_speed(false));
        let chunk = pace.units;
        let mut done = Vec::new();
        let mut progress = None;
        for (id, up) in self.uploads.iter_mut() {
            let end = (up.sent + chunk).min(up.data.len());
            if let Some(p) = self.panes.get_mut(id) {
                // An upload is pasted text: unbracketed, and held by Buffer Input like typing.
                // Its program asked for it, so a replay view does not stop it.
                p.live_input(&up.data[up.sent..end]);
            }
            up.sent = end;
            let pct = up.sent * 100 / up.data.len().max(1);
            if pct / 10 != up.shown_pct / 10 || up.sent == up.data.len() {
                up.shown_pct = pct;
                progress = Some(format!("Uploading {}: {pct}%", up.name));
            }
            if up.sent >= up.data.len() {
                done.push(*id);
            }
        }
        for id in done {
            self.uploads.remove(&id);
        }
        if let Some(text) = progress {
            self.show_toast(&text);
        }
        self.upload_due = (!self.uploads.is_empty()).then(|| now + std::time::Duration::from_secs_f64(pace.delay));
        self.upload_due
    }

    /// Ctrl+C while the focused pane is uploading asks before stopping it. True when the key
    /// was used.
    pub(super) fn upload_key(&mut self, ev: &KeyEvent) -> bool {
        let Some(pane) = self.focused_id().filter(|id| self.uploads.contains_key(id)) else { return false };
        let is_c = matches!(&ev.key_without_modifiers(), winit::keyboard::Key::Character(c) if c.eq_ignore_ascii_case("c"));
        if !(self.mods.ctrl && is_c) {
            return false;
        }
        self.ask(pane, ASK_ABORT_UPLOAD, "A file is being uploaded. Abort the upload?", &["OK", "Cancel"], true);
        true
    }
}

/// iTerm2's `selection` variable: at most `max` characters of the selection, with the whole
/// selection's length in characters (`PS:12518-12529`). A negative `max` caps nothing, as
/// iTerm2's unsigned `MIN` does.
pub(super) fn capped_selection(sel: &str, max: i64) -> (String, usize) {
    let max = usize::try_from(max).unwrap_or(usize::MAX);
    (sel.chars().take(max).collect(), sel.chars().count())
}

/// User variables as text: a string as it is, anything else as its JSON.
fn user_strings(values: crate::api_vars::Values) -> std::collections::BTreeMap<String, String> {
    values.into_iter().map(|(k, v)| (k, v.as_str().map_or_else(|| v.to_string(), str::to_string))).collect()
}

/// This computer's name (iTerm2's `localhostName`, its fully qualified name), read once.
fn local_host_name() -> &'static str {
    static NAME: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    NAME.get_or_init(super::aps_ui::local_host)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uname_has_one_writer() {
        // `conductor_framer::session_values` owns `uname` (`PS:25624-25660`: cleared at SSH
        // level 1), so nothing else in this file may set it.
        let production = include_str!("prompted_ui.rs").split("\n#[cfg(test)]\nmod tests").next().unwrap();
        assert!(!production.contains(concat!("insert(\"un", "ame\"")));
        assert!(production.contains("conductor_framer::session_values"));
    }

    #[test]
    fn selection_cap_follows_iterm2() {
        assert_eq!(capped_selection("abcdef", 3), ("abc".to_string(), 6));
        assert_eq!(capped_selection("abcdef", 100), ("abcdef".to_string(), 6));
        assert_eq!(capped_selection("abcdef", 0), (String::new(), 6));
        assert_eq!(capped_selection("\u{e9}\u{e9}\u{e9}", 2), ("\u{e9}\u{e9}".to_string(), 3));
        assert_eq!(capped_selection("", 100), (String::new(), 0));
    }
}
