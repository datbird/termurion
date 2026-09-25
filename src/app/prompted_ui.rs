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

/// RequestUpload sends this much every tick, as iTerm2's fast paste does.
const UPLOAD_CHUNK: usize = 768;
const UPLOAD_TICK: std::time::Duration = std::time::Duration::from_millis(10);

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
        let prevent = self.globals.values.get("PreventEscapeSequenceFromChangingProfile").and_then(|v| v.as_bool());
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

    fn open_url(&mut self, pane: PaneId, url: &str) {
        // The same guard as a Win-click: web and mail links open; others are refused.
        let run = crate::semantic::guard_open(url, false);
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
        let mut v = prompted::Vars::default();
        let tab_index = self.tabs.iter().position(|t| t.layout.panes().contains(&pane));
        let Some(p) = self.panes.get(&pane) else { return v };
        let s = p.own();
        let title = s.title().unwrap_or("Shell").to_string();
        let grid = s.term().grid();
        use alacritty_terminal::grid::Dimensions;
        v.session.insert("name", title.clone());
        v.session.insert("presentationName", title.clone());
        v.session.insert("id", pane.to_string());
        if let Some(t) = tab_index {
            let index = self.tabs[t].layout.panes().iter().position(|x| *x == pane).unwrap_or(0);
            v.session.insert("termid", format!("w0t{t}p{index}"));
            v.tab.insert("id", t.to_string());
            let focused = self.tabs[t].focused;
            v.tab.insert("title", self.panes.get(&focused).and_then(|f| f.own().title()).unwrap_or("Shell").to_string());
        }
        v.session.insert("columns", grid.columns().to_string());
        v.session.insert("rows", grid.screen_lines().to_string());
        v.session.insert("showingAlternateScreen", if s.term().mode().contains(TermMode::ALT_SCREEN) { "1" } else { "0" }.to_string());
        if let Some(c) = &s.shell.cwd {
            v.session.insert("path", c.clone());
        }
        if let Some(h) = &s.shell.host {
            v.session.insert("hostname", h.clone());
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
        if let Some(pid) = p.shell_pid() {
            v.session.insert("pid", pid.to_string());
            v.session.insert("effective_root_pid", pid.to_string());
        }
        if let Some(c) = s.shell.history.last() {
            v.session.insert("lastCommand", c.command.clone());
        }
        if let Some(sel) = s.term().selection_to_string() {
            v.session.insert("selectionLength", sel.chars().count().to_string());
            v.session.insert("selection", sel);
        }
        if let Some(pr) = self.store.get(&p.profile) {
            v.session.insert("profileName", pr.name());
        }
        for (k, val) in &s.shell.vars {
            v.user.insert(k.clone(), val.clone());
        }
        v.window.insert("id", "0".into());
        v.window.insert("title", title);
        v.app.insert("pid", std::process::id().to_string());
        v.app.insert("effectiveTheme", if self.dark() { "dark" } else { "light" }.to_string());
        v
    }

    /// RequestUpload: pick files, answer, and send the archive.
    pub(super) fn upload_request(&mut self, pane: PaneId, params: String) {
        if !prompted::upload_format_ok(&params) {
            self.ask(pane, UPLOAD_FORMAT, "An upload with an unsupported archive format was requested. You may need a newer version of iTerm2.", &[], false);
            return;
        }
        let picked = rfd::FileDialog::new().set_title("Choose files to upload").pick_files();
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
        let mut done = Vec::new();
        let mut progress = None;
        for (id, up) in self.uploads.iter_mut() {
            let end = (up.sent + UPLOAD_CHUNK).min(up.data.len());
            if let Some(p) = self.panes.get_mut(id) {
                // An upload is pasted text: unbracketed, and held by Buffer Input like typing.
                p.input(&up.data[up.sent..end]);
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
        self.upload_due = (!self.uploads.is_empty()).then(|| now + UPLOAD_TICK);
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
