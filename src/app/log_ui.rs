//! Session logging in the app (milestone 13q): automatic logging, Log to File, compact logging
//! and the menu item (`PT:13813-13822`, `PT:13932-13939`, `PSA:116-158`). The file is opened and
//! written only on the log's own thread (`session_log::FileSink`); nothing here stats a network
//! folder.

use super::scripts_alerts::{ask, Then};
use super::*;
use crate::session_log::{self as sl, AsciicastHeader, FolderAction, Logger, Style};
use crate::warnings::Kind;

/// Log to File's state from the focused pane's (ended, logging): (enabled, checked). The item is
/// disabled with no session or an exited one and checked while logging.
pub(super) fn log_menu_state(pane: Option<(bool, bool)>) -> (bool, bool) {
    match pane {
        Some((ended, logging)) => (!ended, logging && !ended),
        None => (false, false),
    }
}

/// The program of a command line: the first word, or the quoted first word.
fn shell_program(command: &str) -> String {
    let c = command.trim();
    match c.strip_prefix('"') {
        Some(rest) => rest.split('"').next().unwrap_or("").to_string(),
        None => c.split_whitespace().next().unwrap_or("").to_string(),
    }
}

/// The tab and pane numbers of the session whose log is named: where it already is, else the
/// numbers a new session gets (`session_env::numbers`: a split's tab and its next place, or a new
/// tab after the last), since a pane being installed is not in its tab yet (B5).
fn log_numbers(counts: &[usize], at: Option<(usize, usize)>, split: Option<usize>) -> (usize, usize) {
    at.unwrap_or_else(|| crate::session_env::numbers(counts, None, split))
}

impl App {
    /// The session's id as the log name's `termid` reads it, for a pane that may not be in its
    /// tab yet (B5: the same numbers a new session's environment gets).
    fn log_termid(&self, id: PaneId) -> String {
        let counts: Vec<usize> = self.tabs.iter().map(|t| t.layout.panes().len()).collect();
        let at = self.tabs.iter().enumerate().find_map(|(t, tab)| tab.layout.panes().iter().position(|p| *p == id).map(|i| (t, i)));
        let (tab, pane) = log_numbers(&counts, at, self.start_tab);
        crate::session_env::termid(self.window_number(), tab, pane, &id.to_string())
    }

    /// A problem starting the automatic log (`PSA:116-129`).
    fn log_problem(&mut self, profile: &str, error: &str) {
        let text = sl::cannot_start_text(profile, error);
        self.ask_scripts(ask(sl::LOGGING_PROBLEM_HEADING, &text, &["OK"], Some("NoSyncCannotStartLogging"), Kind::Persistent, Then::LogProblem));
    }

    /// Automatic logging at a session's start, at its restart and for a restored session
    /// (`PS:3549-3560`): nothing unless the profile says "Automatically Log". The name is the
    /// profile's "Log Filename Format" when it has one, else `AutoLogFormat`; a restart gets the
    /// same name (the creation time and `autoLogId` are kept, B6) and appends.
    pub(super) fn start_auto_log(&mut self, id: PaneId) {
        let Some(p) = self.panes.get(&id) else { return };
        if p.dvr.read_only || p.exited.is_some() {
            return;
        }
        let profile = self.store.get(&p.profile).unwrap_or_else(|| self.store.default_profile()).clone();
        if !profile.bool("Automatically Log") {
            return;
        }
        let own = profile.values.get("Log Filename Format").and_then(|v| v.as_str()).filter(|t| !t.is_empty()).map(str::to_string);
        let name_template = own.unwrap_or_else(|| self.globals.string("AutoLogFormat"));
        let dir_template = profile.string("Log Directory");
        let vars = self.variables_with(id, false);
        let termid = self.log_termid(id);
        let lookup = |n: &str| {
            let n = n.strip_prefix("session.").unwrap_or(n);
            if n == "termid" {
                return Some(termid.clone());
            }
            crate::prompted::variable(n, &vars)
        };
        let path = match sl::auto_log_path(&dir_template, &name_template, &lookup, &profile::home()) {
            Ok(path) => path,
            Err(e) => return self.log_problem(&profile.name(), &e),
        };
        let append = self.globals.bool("AutologAppends");
        let style = Style::from_int(profile.int("Plain Text Logging"));
        self.begin_log(id, path, append, style, Some(&profile.name()));
    }

    /// A log at `path`: a local folder that is not there asks first; a network folder is never
    /// probed or created (it opens on the writer thread and a failure comes back as
    /// `CANNOT_WRITE`).
    fn begin_log(&mut self, id: PaneId, path: PathBuf, append: bool, style: Style, profile: Option<&str>) {
        let dir = path.parent().map(PathBuf::from).unwrap_or_default();
        match sl::folder_action(&dir, sl::is_network_log_path(&path), &|d| crate::localfs::is_local_dir(d)) {
            FolderAction::AskCreate => {
                let text = sl::create_folder_text(&dir.display().to_string());
                self.ask_scripts(ask(sl::FOLDER_HEADING, &text, &["OK", "Cancel"], Some("CreateDirectory_LoggingFolder"), Kind::Permanent, Then::CreateLogFolder { pane: id, path, append, style }));
            }
            FolderAction::Open => {
                if let Err(e) = self.start_log(id, path, append, style) {
                    match profile {
                        Some(name) => self.log_problem(name, &e),
                        None => self.banners.set("log", format!("{}: {e}", sl::CANNOT_WRITE)),
                    }
                }
            }
        }
    }

    /// Starts the pane's log. The file opens on the writer thread; a failure to open comes back
    /// through `log_tick`.
    pub(super) fn start_log(&mut self, id: PaneId, path: PathBuf, append: bool, style: Style) -> Result<(), String> {
        let Some(p) = self.panes.get(&id) else { return Err("the session is gone".into()) };
        if p.exited.is_some() || p.ended {
            return Err("the session has ended".into());
        }
        let profile = self.store.get(&p.profile).unwrap_or_else(|| self.store.default_profile()).clone();
        let palette = p.own().palette.clone();
        let timestamps = (style == Style::PlainText && self.globals.bool("LogTimestampsWithPlainText")).then(|| self.globals.string("LogTimestampFormat"));
        let stylesheet = (style == Style::Html).then(|| {
            let font = profile::parse_font(&profile.string("Normal Font"));
            sl::html_stylesheet(&font.family, font.size_pt, palette.bg, palette.fg)
        });
        let header = (style == Style::Asciicast).then(|| {
            let term = p.own().term();
            AsciicastHeader {
                width: term.columns() as u16,
                height: term.screen_lines() as u16,
                timestamp: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs_f64().round() as u64),
                command: p.command.clone(),
                title: p.own().title().unwrap_or("").trim().to_string(),
                term: profile.terminal_type(),
                shell: shell_program(&p.command),
                fg: palette.fg,
                bg: palette.bg,
                ansi: palette.ansi,
            }
        });
        let proxy = self.proxy.clone();
        let wake: sl::Wake = std::sync::Arc::new(move || {
            let _ = proxy.lock().map(|p| p.send_event(super::UserEvent::Wake));
        });
        let sink = sl::FileSink::open_with_wake(&path, append, Some(wake));
        let logger = Logger::new(style, Box::new(sink), timestamps, header, stylesheet);
        if let Some(p) = self.panes.get_mut(&id) {
            p.stop_log();
            p.log = Some(logger);
            p.log_started = Some(Instant::now());
        }
        self.menus_dirty = true;
        Ok(())
    }

    /// Stops the pane's log (`PS:4728`, the close).
    pub(super) fn stop_log(&mut self, id: PaneId) {
        if let Some(p) = self.panes.get_mut(&id) {
            p.stop_log();
        }
        self.menus_dirty = true;
    }

    /// Session > Log > Log to File (`PT:13813-13822`): logging stops it; otherwise the save
    /// dialog, then Replace or Append for a file that is there, then the style.
    pub(super) fn start_stop_logging(&mut self) {
        let Some(id) = self.focused_id() else { return };
        let Some(p) = self.panes.get(&id) else { return };
        if p.ended || p.exited.is_some() {
            return;
        }
        if p.log.is_some() {
            return self.stop_log(id);
        }
        let Some(path) = rfd::FileDialog::new().set_title("Log to File").set_directory(profile::home()).save_file() else { return };
        // A network file is replaced with no question: it is never probed (13p UNC invariant).
        if sl::is_network_log_path(&path) || !crate::localfs::local_exists(&path) {
            return self.ask_log_style(id, path, false);
        }
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        self.ask_scripts(ask(&sl::exists_heading(&name), sl::EXISTS_BODY, &["Cancel", sl::REPLACE, sl::APPEND], None, Kind::Persistent, Then::LogExists { pane: id, path }));
    }

    /// Replace (1) or Append (2) for a file that exists; Cancel (0) asks nothing more.
    pub(super) fn log_exists_answered(&mut self, pane: PaneId, path: PathBuf, index: usize) {
        if index != 0 {
            self.ask_log_style(pane, path, index == 2);
        }
    }

    /// The style window: the last used style is named in its text and stored under
    /// `NoSyncLoggingStyle` (Decision).
    fn ask_log_style(&mut self, pane: PaneId, path: PathBuf, append: bool) {
        let remembered = Style::from_int(self.globals.int(sl::LOGGING_STYLE_KEY));
        self.ask_scripts(ask("Logging Style", &sl::style_prompt(remembered), &sl::STYLE_LABELS, None, Kind::Persistent, Then::LogStyle { pane, path, append }));
    }

    pub(super) fn log_style_answered(&mut self, pane: PaneId, path: PathBuf, append: bool, index: usize) {
        let style = Style::from_int(index as i64);
        self.globals.set(sl::LOGGING_STYLE_KEY, toml::Value::Integer(index as i64));
        self.save_globals();
        self.begin_log(pane, path, append, style, None);
    }

    /// "Would you like to create the directory at ...?": OK (0) makes it and starts the log. The
    /// question is asked only for a local folder; a network path is refused again here.
    pub(super) fn log_folder_answered(&mut self, pane: PaneId, path: PathBuf, append: bool, style: Style, index: usize) {
        if index != 0 || sl::is_network_log_path(&path) {
            return;
        }
        let Some(dir) = path.parent() else { return };
        if let Err(e) = crate::localfs::judge_io(dir).and_then(|d| crate::localfs::create_dir_all(&d)) {
            return self.banners.set("log", format!("Failed to create {}: {e}", dir.display()));
        }
        if let Err(e) = self.start_log(pane, path, append, style) {
            self.banners.set("log", format!("{}: {e}", sl::CANNOT_WRITE));
        }
    }

    /// A log that failed (it never opened, or a write failed) goes off and the person is told
    /// once, with the path (`LH:304-316`). The writer wakes the loop when it records a failure;
    /// no timer looks (an idle wake while any pane logs).
    pub(super) fn log_tick(&mut self) {
        let mut failed = Vec::new();
        for (id, p) in self.panes.iter() {
            if let Some(log) = p.log.as_ref().filter(|l| l.failed().is_some()) {
                failed.push((*id, log.path().map(|x| x.display().to_string()).unwrap_or_default(), log.failed().unwrap_or_default()));
            }
        }
        for (id, path, error) in failed {
            self.stop_log(id);
            let _ = crate::notify::show_with(sl::CANNOT_WRITE, &format!("{path}\n{error}"), false);
        }
    }

    /// Quit and the last window's close: every log stops and what is queued reaches its file, in
    /// one wait for all of them (a hung share costs the wait once, not per pane).
    pub(super) fn close_all_logs(&mut self) {
        for p in self.panes.values_mut() {
            p.stop_log();
        }
        crate::session_log::wait_for_writers(Duration::from_millis(500));
    }

    /// Compact logging's prompt (`PS:16729-16740`): each line of the revealed prompt's text, then a
    /// space. Only with `smartLoggingWithAutoComposer`.
    pub(super) fn log_compact_prompt(&mut self, pane: PaneId, lines: &[String]) {
        if !self.globals.bool("SmartLoggingWithAutoComposer") {
            return;
        }
        let Some(p) = self.panes.get_mut(&pane) else { return };
        let last = lines.len().saturating_sub(1);
        for (i, line) in lines.iter().enumerate() {
            p.log_text(&format!("{line}{}", if i == last { " " } else { "\n " }));
        }
    }

    /// Compact logging's sent command (`PS:24561-24566`), trailing newlines trimmed.
    pub(super) fn log_compact_command(&mut self, pane: PaneId, command: &str) {
        if !self.globals.bool("SmartLoggingWithAutoComposer") {
            return;
        }
        if let Some(p) = self.panes.get_mut(&pane) {
            p.log_text(command.trim_end_matches(['\n', '\r']));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_menu_follows_iterm2() {
        assert_eq!(log_menu_state(None), (false, false));
        assert_eq!(log_menu_state(Some((true, true))), (false, false));
        assert_eq!(log_menu_state(Some((false, true))), (true, true));
        assert_eq!(log_menu_state(Some((false, false))), (true, false));
    }

    #[test]
    fn auto_log_path_for_a_pane_not_yet_in_a_tab() {
        // Two tabs of 2 and 1 panes. A new tab: tab 2, pane 0. A split of tab 1: its next pane.
        assert_eq!(log_numbers(&[2, 1], None, None), (2, 0));
        assert_eq!(log_numbers(&[2, 1], None, Some(1)), (1, 1));
        assert_eq!(log_numbers(&[], None, None), (0, 0));
        // A pane that is in its tab (a restart) keeps its own place.
        assert_eq!(log_numbers(&[3], Some((0, 1)), None), (0, 1));
        // So the default name of two new sessions differs.
        assert_ne!(crate::session_env::termid(0, 2, 0, "5"), crate::session_env::termid(0, 1, 1, "6"));
        // And the name's variables are read from the pane, not the app.
        let src = include_str!("log_ui.rs");
        assert!(src.contains("if n == \"termid\""), "termid comes from the install-time numbers");
        let prompted = include_str!("prompted_ui.rs");
        for name in ["creationTimeString", "autoLogId", "logFilename"] {
            assert!(prompted.contains(&format!("insert(\"{name}\"")), "{name}");
        }
        assert!(prompted.contains("crate::session_env::termid(number, t, index"));
    }

    fn body<'a>(src: &'a str, name: &str) -> &'a str {
        let b = &src[src.find(name).unwrap_or_else(|| panic!("{name}"))..];
        &b[..b.find("\n    }\n").unwrap()]
    }

    /// Logging starts where iTerm2 starts it (`PS:3549-3560`) and stops where it stops it
    /// (`PS:4728`, the close): nothing is written after the person closed the pane.
    #[test]
    fn logging_starts_and_stops_where_iterm2_does() {
        let m = include_str!("mod.rs");
        assert!(body(m, "    fn install_pane(").contains("self.start_auto_log(id);"), "start, restart and restore all install the pane");
        let close = body(m, "    fn close_pane(");
        assert!(close.contains("p.stop_log();"), "a close stops the log");
        assert!(close.find("p.stop_log();").unwrap() < close.find("match self.undo_capture").unwrap());
        assert!(body(include_str!("session_end_ui.rs"), "pub(super) fn session_exited(").contains("p.stop_log();"), "the end stops it");
        let menu = include_str!("menu.rs");
        assert!(menu.contains("\"startStopLogging:\" => self.start_stop_logging()"));
        let quit = body(menu, "pub(super) fn quit_confirmed(");
        assert!(quit.find("self.close_all_logs();").unwrap() < quit.find("self.leave_host(keep);").unwrap(), "quit writes the queue out BEFORE leave_host empties the panes");
        let last = body(m, "    fn close_pane(");
        assert!(last.find("self.close_all_logs();").unwrap() < last.find("self.leave_host(false);").unwrap(), "the last window's close does too");
        let all = body(include_str!("log_ui.rs"), "    pub(super) fn close_all_logs(");
        assert!(all.find("stop_log()").unwrap() < all.find("wait_for_writers(").unwrap(), "every log stops, then one shared wait");
        let restart = body(m, "    fn restart_confirmed(");
        assert!(restart.find("wait_for_writers(").unwrap() < restart.find("Pane::relaunch(").unwrap(), "the old log is written out before the new one opens");
        assert!(restart.find("stop_log()").unwrap() < restart.find("wait_for_writers(").unwrap());
        // Log to File is a check item and a command that works.
        assert!(menu.contains("| \"startStopLogging:\")"));
        assert!(crate::commands::DONE.contains(&"startStopLogging:"));
    }

    /// The questions the alert queue asks: a closed window never starts a log or picks a style.
    #[test]
    fn a_closed_logging_question_starts_nothing() {
        use super::super::scripts_alerts::{on_close, Answer};
        let path = std::path::PathBuf::from("C:\\logs\\a.log");
        assert_eq!(on_close(&Then::LogStyle { pane: 1, path: path.clone(), append: false }, 0), Answer::Closed);
        assert_eq!(on_close(&Then::LogExists { pane: 1, path: path.clone() }, 0), Answer::Closed);
        assert_eq!(on_close(&Then::CreateLogFolder { pane: 1, path, append: false, style: Style::Raw }, 0), Answer::Button(1), "Cancel");
    }

    #[test]
    fn the_program_of_a_command_line() {
        assert_eq!(shell_program("pwsh.exe -NoLogo"), "pwsh.exe");
        assert_eq!(shell_program("\"C:\\Program Files\\PowerShell\\7\\pwsh.exe\" -l"), "C:\\Program Files\\PowerShell\\7\\pwsh.exe");
        assert_eq!(shell_program(""), "");
    }

    /// A logging pane adds no idle wake: the writer wakes the loop when a write fails, and the
    /// wake array has no slot of the log's own (13q final fix 3).
    #[test]
    fn a_logging_pane_adds_no_idle_wake() {
        let m = include_str!("mod.rs");
        assert!(!m.contains("self.log_due()") && !m.contains("[ended_due, log_due"), "no log timer in the wake array");
        let prod = include_str!("log_ui.rs").split("#[cfg(test)]").next().unwrap();
        assert!(!prod.contains("fn log_due"));
        assert!(body(prod, "    pub(super) fn start_log(").contains("open_with_wake(") && prod.contains("UserEvent::Wake"), "the writer's failure wakes the loop");
    }
}
