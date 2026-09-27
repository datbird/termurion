//! Launching scripts (`M:867-977`, `L:725-1029`): a path is classified, its environment made
//! ready (asking before any download), the API started when a person asked while it was
//! off, and the script run hidden with a fresh cookie and key. Its output goes to its Script
//! Console entry; Terminate and Restart end and relaunch it.

use std::io::{BufRead, Read};
use std::path::{Path, PathBuf};

use super::api_ui::{self, Alert};
use super::script_runtime_ui::After;
use super::scripts_alerts::{Ask, Then};
use super::App;
use crate::script_history::{Entry, Relaunch};
use crate::script_launch::{self as sl, Kind, LaunchGate, Request};
use crate::script_proc::ScriptProcess;
use crate::script_provision::Target;
use crate::script_runtime as rt;

/// The most of a first line read for its shebang.
const FIRST_LINE_LIMIT: u64 = 4096;

/// A script's first line (`L:677-723` reads it with `fgetln`).
fn first_line(p: &Path) -> Option<String> {
    let file = std::fs::File::open(p).ok()?;
    let mut line = Vec::new();
    std::io::BufReader::new(file.take(FIRST_LINE_LIMIT)).read_until(b'\n', &mut line).ok()?;
    (!line.is_empty()).then(|| String::from_utf8_lossy(&line).into_owned())
}

/// What a full environment is built from (`M:1009-1016`): the version its marker recorded,
/// else its `setup.cfg` pin, else the default; the dependencies `setup.cfg` lists.
fn container_inputs(container: &Path) -> (String, Vec<String>) {
    let cfg = std::fs::read_to_string(container.join(rt::SETUP_CFG)).ok().and_then(|t| rt::SetupCfg::parse(&t)).unwrap_or_default();
    let recorded = std::fs::read_to_string(container.join(rt::MARKER)).ok().and_then(|t| rt::Marker::parse(&t)).map(|m| m.python);
    let version = recorded.or(cfg.python_version).unwrap_or_else(|| rt::DEFAULT_PYTHON.to_string());
    (version, cfg.dependencies)
}

impl App {
    /// Launches `req.path` (`launchScriptWithAbsolutePath:`).
    pub(super) fn launch_script(&mut self, req: Request) {
        let kind = sl::classify(&req.path, &crate::scripts::RealFs, &first_line);
        self.launch_classified(req, kind);
    }

    /// Launches what `classify` found: a path that is not a script does nothing (it is never
    /// opened); a plain folder is malformed; a lost environment is offered a rebuild (a
    /// person's launch) or logged; a script goes through the API gate, then its environment.
    pub(super) fn launch_classified(&mut self, req: Request, kind: Kind) {
        let environment = match &kind {
            Kind::NotAScript(_) => return,
            Kind::Malformed(p) => {
                let buttons = vec![crate::api_auth::OK.to_string(), sl::REVEAL.to_string()];
                // Temporarily silenceable in iTerm2: no checkbox (Decision); Reveal is never remembered.
                let ask = Ask { heading: sl::MALFORMED_HEADING.to_string(), text: sl::malformed(&sl::entry_name(&kind)), buttons, silence: None, then: Then::Malformed(p.clone()) };
                return self.ask_scripts(ask);
            }
            Kind::NeedsRebuild { .. } => {
                let name = sl::entry_name(&kind);
                if !req.explicit {
                    return self.script_log_app(sl::env_missing_line(&name));
                }
                // `REBUILD_ID` is persistent in iTerm2: never silenced.
                let buttons = vec![sl::REBUILD.to_string(), crate::api_auth::CANCEL.to_string()];
                let ask = Ask { heading: sl::REBUILD_HEADING.to_string(), text: sl::rebuild_text(&name), buttons, silence: None, then: Then::Rebuild(req) };
                return self.ask_scripts(ask);
            }
            Kind::Basic { version, .. } => {
                let requested = version.clone().unwrap_or_else(|| rt::DEFAULT_PYTHON.to_string());
                (Target::Shared(requested.clone()), requested, Vec::new())
            }
            Kind::Full { container, .. } => {
                let (requested, dependencies) = container_inputs(container);
                (Target::Container(container.clone()), requested, dependencies)
            }
        };
        match sl::launch_gate(req.explicit, self.api_on) {
            // iTerm2 only logs this to its debug log.
            LaunchGate::Skip => {}
            LaunchGate::EnableFirst => {
                self.script_after_enable.push(req);
                self.queue_api_alert(Alert::Enable);
            }
            LaunchGate::Launch => {
                let (target, requested, dependencies) = environment;
                let explicit = req.explicit;
                self.ensure_environment(target, &requested, &dependencies, explicit, After::Launch(req));
            }
        }
    }

    /// Rebuild, in "Rebuild Python Environment?" (`M:1006-1042`): the container is built again
    /// from its saved requirements (keeping its `setup.cfg`), then launched.
    pub(super) fn rebuild_script(&mut self, req: Request) {
        let (requested, dependencies) = container_inputs(&req.path);
        self.ensure_environment(Target::Container(req.path.clone()), &requested, &dependencies, true, After::Rebuild(req));
    }

    /// A launch's environment is ready: the script runs when it still is one and the API
    /// still allows it; otherwise it goes through the launch again.
    pub(super) fn script_environment_ready(&mut self, req: Request, python: PathBuf) {
        let kind = sl::classify(&req.path, &crate::scripts::RealFs, &first_line);
        let runnable = matches!(kind, Kind::Basic { .. } | Kind::Full { .. });
        if !runnable || sl::launch_gate(req.explicit, self.api_on) != LaunchGate::Launch {
            return self.launch_classified(req, kind);
        }
        let venv = python.parent().and_then(Path::parent).map(Path::to_path_buf);
        self.spawn_script(&req, &kind, python, venv);
    }

    /// Runs a script (`L:725-853`): its entry (a fresh key as its id) is added and logged with
    /// the command line, then the interpreter starts hidden with a Single cookie and the key.
    /// A start that fails is logged and alerted.
    pub(super) fn spawn_script(&mut self, req: &Request, kind: &Kind, python: PathBuf, venv: Option<PathBuf>) {
        let key = crate::api_auth::fresh_key();
        let cookie = self.api_jar.lock().unwrap().mint(crate::api_auth::CookieUse::Single);
        let main = sl::main_path(kind);
        let mut entry = Entry::new(&key, &sl::entry_name(kind), api_ui::unix_secs());
        entry.full_path = Some(sl::full_path(kind));
        entry.path = Some(main.clone());
        entry.relaunch = Some(Relaunch { path: req.path.clone(), args: req.args.clone() });
        self.script_history.add(entry);
        let stamp = self.history_stamp();
        self.script_history.add_log(&key, &sl::console_line(&python, &main, &req.args), &stamp);
        let base = std::env::vars_os().filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?))).collect();
        let certifi = venv.as_deref().is_some_and(|v| rt::certifi_path(v).is_file());
        let env = sl::launch_env(base, &cookie, &key, venv.as_deref(), certifi);
        let mut args = vec![main.to_string_lossy().into_owned()];
        args.extend(req.args.iter().cloned());
        match ScriptProcess::start(&python, &args, &env, None, self.notify.clone()) {
            Ok(p) => {
                if let Some(e) = self.script_history.find_mut(&key) {
                    e.pids = vec![p.pid()];
                }
                self.script_procs.insert(key, p);
            }
            Err(e) => {
                // The cookie will never be used.
                self.api_jar.lock().unwrap().remove(&cookie);
                let stamp = self.history_stamp();
                self.script_history.add_log(&key, &sl::failed_to_launch(&e), &stamp);
                self.script_history.stop(&key);
                let text = sl::error_running(&main.display().to_string(), &e);
                self.ask_scripts(Ask { heading: sl::ERROR_RUNNING_TITLE.to_string(), text, buttons: vec![crate::api_auth::OK.to_string()], silence: None, then: Then::ErrorRunning });
            }
        }
        self.redraw();
    }

    /// Terminate (`SH:190-196`): the note in the log, the script and everything it started
    /// ended, and its API connections closed.
    // Called by the Scripts menu (Task 6) and the Script Console (Task 8).
    #[allow(dead_code)]
    pub(super) fn terminate_script(&mut self, id: &str) {
        let stamp = self.history_stamp();
        self.script_history.kill(id, &stamp);
        if let Some(p) = self.script_procs.get_mut(id) {
            p.terminate();
        }
        if let Some(s) = &self.api_server {
            for (conn, _) in self.api_keys.iter().filter(|(_, (key, _))| key == id) {
                s.close(*conn);
            }
        }
    }

    /// Restart (`SC:224-232`): Terminate, then the same launch again, as a person's.
    #[allow(dead_code)]
    pub(super) fn relaunch_script(&mut self, id: &str) {
        let relaunch = self.script_history.find(id).and_then(|e| e.relaunch.clone());
        self.terminate_script(id);
        if let Some(r) = relaunch {
            self.launch_script(Request { path: r.path, args: r.args, explicit: true });
        }
    }

    /// Moves each script's output into its entry; an ended script's entry gets the exit text
    /// and, unless the person terminated it, a notification (`L:960-1012`), then stops.
    pub(super) fn scripts_tick(&mut self) {
        if self.script_procs.is_empty() {
            return;
        }
        let ids: Vec<String> = self.script_procs.keys().cloned().collect();
        for id in ids {
            let Some(p) = self.script_procs.get_mut(&id) else { continue };
            let mut text = String::new();
            while let Some(bytes) = p.try_output() {
                text.push_str(&p.decoder.push(&bytes));
            }
            let code = p.exit_code();
            if !text.is_empty() {
                let stamp = self.history_stamp();
                self.script_history.add_log(&id, &text, &stamp);
            }
            if let Some(code) = code {
                self.script_ended(&id, code);
            }
        }
        self.redraw();
    }

    fn script_ended(&mut self, id: &str, code: u32) {
        // Whatever the script left running ends with its job.
        self.script_procs.remove(id);
        if let Some(text) = sl::exit_text(code) {
            let stamp = self.history_stamp();
            self.script_history.add_log(id, &text, &stamp);
            let failed = self.script_history.find(id).filter(|e| !e.terminated_by_user).map(|e| e.name.clone());
            if let Some(name) = failed {
                match crate::notify::show_with(sl::SCRIPT_FAILED, &sl::ended_unexpectedly(&name), self.globals.bool("silentUserNotifications")) {
                    Ok(()) => {
                        self.last_notified_script = Some(id.to_string());
                        self.last_notified = None;
                    }
                    Err(e) => self.banners.set("notify", format!("Could not show a notification: {e}")),
                }
            }
        }
        self.script_history.stop(id);
    }

    /// The API stopped: its scripts are ended (`SH:182-188`, signal 9 in iTerm2).
    pub(super) fn scripts_api_stopped(&mut self) {
        let pids = self.script_history.api_stopped();
        for p in self.script_procs.values_mut().filter(|p| pids.contains(&p.pid())) {
            p.terminate();
        }
    }

    /// A failed script's notification was clicked: its Script Console entry shows its end
    /// (`revealTailOfHistoryEntry:`). The Script Console window is Task 8's; until it exists
    /// nothing opens.
    pub(super) fn reveal_script_entry(&mut self, _id: &str) {}

    /// Selects `path` in Explorer; nothing runs it.
    pub(super) fn reveal_path(&self, path: &Path) {
        use std::os::windows::process::CommandExt;
        let text = path.to_string_lossy().replace('/', "\\");
        let mut cmd = std::process::Command::new("explorer.exe");
        cmd.raw_arg(format!("/select,{}", crate::semantic::cmd_quote(&text)));
        // Explorer shows its own message for a path it cannot select.
        let _ = cmd.spawn();
    }
}
