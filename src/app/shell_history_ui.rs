//! Shell history: each pane's commands and directories (from shell integration) go into
//! the store, and Session > Open Command History... and Open Recent Directories... pick
//! from it.

use super::*;
use crate::broadcast::Write;
use crate::list_popup_ui::{ListPopup, PopupEvent};
use crate::shell_history::{self, Powers, Store};

/// What a list popup picks for.
#[derive(Clone, Debug)]
pub(super) enum PopupFor {
    /// Commands starting with what was typed.
    Commands(String),
    Dirs,
}

/// What each pane has fed already: command records, the codes seen, the directory.
#[derive(Default)]
pub(super) struct Fed {
    records: usize,
    coded: usize,
    cwd: Option<String>,
}

const ABOUT: &str = "shell-integration-about";

fn store_path() -> std::path::PathBuf {
    std::env::var_os("APPDATA").map(std::path::PathBuf::from).unwrap_or_else(std::env::temp_dir).join("Tandem").join("shell_history.json")
}

fn unix(t: std::time::SystemTime) -> f64 {
    t.duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64())
}

impl App {
    /// The store, read (and pruned, as iTerm2 does at launch) the first time.
    fn shell_history(&mut self) -> &mut Store {
        self.shell_history.get_or_insert_with(|| {
            let mut s = std::fs::read_to_string(store_path()).map(|t| Store::from_json(&t)).unwrap_or_default();
            s.prune(unix(std::time::SystemTime::now()));
            s
        })
    }

    fn powers(&self) -> Powers {
        let f = |k: &str, d: f64| self.globals.number(k).unwrap_or(d);
        Powers { uses: f("CommandHistoryUsePower", 3.0), age: f("CommandHistoryAgePower", 1.0) }
    }

    /// New commands, exit codes, and directories from every pane go into the store.
    pub(super) fn feed_shell_history(&mut self) {
        let mut adds: Vec<(String, Option<String>, String, Option<String>, f64)> = Vec::new();
        let mut codes: Vec<(String, String, i32)> = Vec::new();
        let mut dirs: Vec<(String, String)> = Vec::new();
        for (id, p) in &self.panes {
            let s = p.own();
            let fed = self.shell_fed.entry(*id).or_default();
            let h = &s.shell.history;
            let key = |host: Option<&str>| shell_history::host_key(s.shell.user.as_deref(), host.or(s.shell.host.as_deref()));
            for r in h.iter().skip(fed.records) {
                adds.push((key(r.host.as_deref()), None, r.command.clone(), r.cwd.clone(), unix(r.start)));
            }
            fed.records = h.len();
            while fed.coded < h.len() {
                let r = &h[fed.coded];
                // A running command's code comes later; the ones before it are done.
                match r.code {
                    Some(c) => codes.push((key(r.host.as_deref()), r.command.clone(), c)),
                    None if fed.coded + 1 == h.len() => break,
                    None => {}
                }
                fed.coded += 1;
            }
            if s.shell.cwd.is_some() && s.shell.cwd != fed.cwd {
                fed.cwd = s.shell.cwd.clone();
                dirs.push((key(None), fed.cwd.clone().unwrap_or_default()));
            }
        }
        if adds.is_empty() && codes.is_empty() && dirs.is_empty() {
            return;
        }
        let now = unix(std::time::SystemTime::now());
        self.tools_changed();
        let store = self.shell_history();
        for (host, _, command, cwd, time) in adds {
            store.record_command(&host, &command, cwd.as_deref(), time);
        }
        for (host, command, code) in codes {
            store.record_code(&host, &command, code);
        }
        for (host, dir) in dirs {
            store.record_dir(&host, &dir, now);
        }
        self.save_shell_history();
    }

    pub(super) fn save_shell_history(&mut self) {
        if !self.globals.bool("SavePasteHistory") {
            return;
        }
        let json = self.shell_history().to_json();
        let path = store_path();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Err(e) = shell_history::write_whole(&path, &json) {
            self.banners.set("history", format!("Could not save the command history: {e}"));
        }
    }

    pub(super) fn shell_history_pub(&mut self) -> &mut Store {
        self.shell_history()
    }

    pub(super) fn powers_pub(&self) -> Powers {
        self.powers()
    }

    pub(super) fn history_host_pub(&self, pane: PaneId) -> String {
        self.history_host(pane)
    }

    /// The focused session's host record key.
    fn history_host(&self, pane: PaneId) -> String {
        let s = self.panes.get(&pane).map(|p| p.own());
        shell_history::host_key(s.and_then(|s| s.shell.user.as_deref()), s.and_then(|s| s.shell.host.as_deref()))
    }

    /// iTerm2's "About Shell Integration" before shell integration was ever used. True
    /// when it showed.
    fn shell_integration_needed(&mut self, pane: PaneId) -> bool {
        if self.shell_history().ever_used() || self.globals.bool("NoSyncCommandHistoryHasEverBeenUsed") {
            self.globals.set("NoSyncCommandHistoryHasEverBeenUsed", toml::Value::Boolean(true));
            return false;
        }
        self.ask(pane, ABOUT, "About Shell Integration: To use shell integration features such as Command History, Recent Directories, Select Output of Last Command, and Automatic Profile Switching, your shell must be properly configured.", &["Learn More...", "OK", "Install Now"], true);
        true
    }

    /// Session > Open Command History....
    pub(super) fn open_command_history(&mut self, event_loop: &ActiveEventLoop) {
        let Some(pane) = self.focused_id() else { return };
        let typed = self.panes.get(&pane).and_then(|p| p.own().typed_command()).unwrap_or_default();
        self.open_command_history_with(event_loop, pane, typed.trim_start(), false, false);
    }

    /// Command History for `prefix` (`openCommandHistoryWithPrefix:sortChronologically:
    /// currentSessionOnly:`): newest first when `chronological` (else by frecency), and with
    /// `session_only` only commands this session ran (the composer's Up and Down at its edges,
    /// Ctrl+R and Win+Shift+;, `PS:24360-24364`).
    pub(super) fn open_command_history_with(&mut self, event_loop: &ActiveEventLoop, pane: PaneId, prefix: &str, chronological: bool, session_only: bool) {
        if self.shell_integration_needed(pane) {
            return;
        }
        let host = self.history_host(pane);
        let (now, powers) = (unix(std::time::SystemTime::now()), self.powers());
        let mut found = self.shell_history().commands(&host, prefix, now, powers);
        if session_only {
            let ran: std::collections::HashSet<String> = self.panes.get(&pane).map(|p| p.own().shell.history.iter().map(|r| r.command.trim().to_string()).collect()).unwrap_or_default();
            found.retain(|(c, _)| ran.contains(c.trim()));
        }
        if chronological {
            found.sort_by(|a, b| b.1.total_cmp(&a.1));
        }
        let rows: Vec<(String, String)> = found.into_iter().map(|(c, t)| (c, crate::paste_history::ago(t, now))).collect();
        self.open_list_popup(event_loop, pane, "Command History", ["Command", "Last Used"], rows, PopupFor::Commands(prefix.to_string()));
    }

    /// Session > Open Recent Directories....
    pub(super) fn open_recent_dirs(&mut self, event_loop: &ActiveEventLoop) {
        let Some(pane) = self.focused_id() else { return };
        if self.shell_integration_needed(pane) {
            return;
        }
        let host = self.history_host(pane);
        let now = unix(std::time::SystemTime::now());
        let rows: Vec<(String, String)> = self.shell_history().dirs(&host).into_iter().map(|d| (d.path, crate::paste_history::ago(d.last, now))).collect();
        self.open_list_popup(event_loop, pane, "Recent Directories", ["Directory", "Last Used"], rows, PopupFor::Dirs);
    }

    fn open_list_popup(&mut self, event_loop: &ActiveEventLoop, pane: PaneId, title: &str, headers: [&str; 2], rows: Vec<(String, String)>, what: PopupFor) {
        self.list_popup = None;
        let Some(g) = self.gpu.as_ref() else { return };
        let values = rows.iter().map(|(v, _)| v.clone()).collect();
        match ListPopup::open(event_loop, &g.instance, &g.adapter, &g.device, &g.queue, title, headers, rows, None) {
            Ok(w) => self.list_popup = Some((w, pane, what, values)),
            Err(e) => self.banners.set("window", format!("Could not open the window: {e}")),
        }
    }

    pub(super) fn list_popup_event(&mut self, window: WindowId, event: &WindowEvent) -> bool {
        let Some((w, pane, what, values)) = self.list_popup.as_mut() else { return false };
        if w.id() != window {
            return false;
        }
        let (pane, what) = (*pane, what.clone());
        match event {
            WindowEvent::CloseRequested => self.list_popup = None,
            WindowEvent::RedrawRequested => match w.redraw() {
                PopupEvent::Pick(i) => {
                    let value = values.get(i).cloned();
                    self.list_popup = None;
                    if let Some(v) = value {
                        self.list_popup_pick(pane, &what, &v);
                    }
                }
                PopupEvent::Close => self.list_popup = None,
                PopupEvent::None => {}
            },
            other => {
                w.event(other);
            }
        }
        true
    }

    fn list_popup_pick(&mut self, pane: PaneId, what: &PopupFor, value: &str) {
        match what {
            // Only what was not typed yet goes (iTerm2's CommandHistoryPopup).
            PopupFor::Commands(typed) => {
                let rest = shell_history::remainder(value, typed);
                // A shown dropdown takes the pick (`PS:20422-20425`).
                if self.composer_shown(pane) {
                    return self.composer_insert(pane, &rest);
                }
                self.user_write(pane, rest.as_bytes(), Write::SendText);
            }
            // At a prompt, the path shell-escaped (newlines too).
            PopupFor::Dirs => {
                let Some(s) = self.panes.get(&pane).map(|p| p.own()) else { return };
                if self.composer_shown(pane) {
                    let text = shell_history::insertable_dir(value, s.shell.shell.as_deref());
                    return self.composer_insert(pane, &text);
                }
                if s.at_prompt() == Some(true) {
                    let text = shell_history::insertable_dir(value, s.shell.shell.as_deref());
                    self.user_write(pane, text.as_bytes(), Write::SendText);
                }
            }
        }
    }

    /// The "About Shell Integration" bar's answers. True when it was this bar.
    pub(super) fn shell_history_bar(&mut self, pane: PaneId, id: &str, button: Option<&str>) -> bool {
        if id != ABOUT {
            return false;
        }
        match button {
            Some("Learn More...") => self.open_url(pane, "https://iterm2.com/documentation-shell-integration.html"),
            Some("Install Now") => self.install_shell_integration(),
            _ => {}
        }
        true
    }
}
