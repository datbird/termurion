//! Launching scripts (`M:867-977`, `L:725-1029`): a path is classified, its environment made
//! ready (asking before any download), the API started when a person asked while it was
//! off, and the script run hidden with a fresh cookie and key. Its output goes to its Script
//! Console entry; Terminate and Restart end and relaunch it. The Scripts menu lists the
//! scripts after its fixed items (`M:1559-1577`), AutoLaunch starts its folder's scripts
//! once (`M:1587-1650`), and Open Quickly and MenuItemRequest reach the same items. Scripts >
//! Manage also opens the Python REPL (`AD:3528-3595`), imports scripts and archives (`IM`,
//! `AR`, `M:547-607`, `M:733-789`) and exports them (`EX`, `M:683-714`).

use std::io::{BufRead, Read};
use std::path::{Path, PathBuf};

use super::api_ui::{self, Alert};
use super::script_runtime_ui::After;
use super::scripts_alerts::{ask, Then};
use crate::warnings;
use super::App;
use crate::script_archive as sa;
use crate::script_export_ui::{ExportEvent, ExportWindow};
use crate::script_history::{Entry, Relaunch};
use crate::script_launch::{self as sl, Kind, LaunchGate, Request};
use crate::script_proc::ScriptProcess;
use crate::script_new::{self as sn, Env, Plan};
use crate::script_new_ui::{PickerEvent, PickerWindow};
use crate::script_provision::{Job, Outcome, RealRunner, Target};
use crate::script_runtime as rt;
use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::WindowId;

/// The most of a first line read for its shebang.
const FIRST_LINE_LIMIT: u64 = 4096;

/// The most output taken from one script in one pass of the loop; the rest waits for the
/// next pass.
const OUTPUT_PER_PASS: usize = 64 * 1024;

/// A script's first line (`L:677-723` reads it with `fgetln`).
pub(super) fn first_line(p: &Path) -> Option<String> {
    let file = crate::localfs::open(&crate::localfs::LocalPath::judge(p)?).ok()?;
    let mut line = Vec::new();
    std::io::BufReader::new(file.take(FIRST_LINE_LIMIT)).read_until(b'\n', &mut line).ok()?;
    (!line.is_empty()).then(|| String::from_utf8_lossy(&line).into_owned())
}

/// Menu ids of the Scripts menu's scripts: the prefix, then the script's full path (iTerm2's
/// item identifier, `M:1559-1566`).
pub(super) const SCRIPT_ID: &str = "script:";

/// iTerm2's prefix for a script's Option alternate "Reveal %@" (`M:1568-1576`).
const REVEAL_PREFIX: &str = "/Reveal/";

/// One entry of the Scripts menu after its fixed items.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum MenuEntry {
    Script { id: String, label: String, path: PathBuf, checked: bool },
    Folder { label: String, children: Vec<MenuEntry> },
}

/// The Scripts menu's entries for the scanned tree (`M:476-545`, `M:1559-1566`): folders as
/// submenus, scripts titled by their file names. A script is checked while `running` says an
/// entry runs with its path (`M:180-185`); a full environment's entry runs its main .py, so
/// its item (the folder) is never checked, as in iTerm2 (kept).
pub(super) fn menu_entries(nodes: &[crate::scripts::Node], running: &dyn Fn(&Path) -> bool) -> Vec<MenuEntry> {
    use crate::scripts::Node;
    nodes
        .iter()
        .map(|n| match n {
            Node::Script { name, path, .. } => MenuEntry::Script { id: format!("{SCRIPT_ID}{}", path.display()), label: name.clone(), path: path.clone(), checked: running(path) },
            Node::Folder { name, children, .. } => MenuEntry::Folder { label: name.clone(), children: menu_entries(children, running) },
        })
        .collect()
}

/// iTerm2's title for a script's Option alternate (`M:1568-1571`).
pub(super) fn reveal_label(name: &str) -> String {
    format!("Reveal {name}")
}

/// Every script item of the Scripts menu and its Option alternate, in menu order, as iTerm2
/// adds them (`addFile:withFullPath:toScriptMenu:`, `M:1559-1576`): the script's path and the
/// menu item, whose path runs from `parents` (the Scripts menu, then the folders) to its title
/// and whose identifier is the full path, or "/Reveal/" and the path for the alternate.
pub(super) fn script_items(nodes: &[crate::scripts::Node], parents: &[String]) -> Vec<(PathBuf, crate::menus::Item)> {
    use crate::scripts::Node;
    let item = |title: String, selector: &str, identifier: String, alternate: bool| {
        let mut path = parents.to_vec();
        path.push(title);
        crate::menus::Item { path, selector: selector.to_string(), identifier, key: String::new(), mods: Vec::new(), separator: false, submenu: false, alternate }
    };
    let mut out = Vec::new();
    for n in nodes {
        match n {
            Node::Script { name, path, .. } => {
                let full = path.display().to_string();
                out.push((path.clone(), item(name.clone(), "launchOrTerminateScript:", full.clone(), false)));
                out.push((path.clone(), item(reveal_label(name), "revealScript:", format!("{REVEAL_PREFIX}{full}"), true)));
            }
            Node::Folder { name, children, .. } => {
                let mut inner = parents.to_vec();
                inner.push(name.clone());
                out.extend(script_items(children, &inner));
            }
        }
    }
    out
}

/// The Scripts menu's own title, the first part of every script item's menu path.
const SCRIPTS_MENU: &str = "Scripts";

/// What choosing a script item does (`M:833-851`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Pick {
    Launch,
    Terminate,
    Reveal,
}

/// The Option alternate (Alt held, Decision) reveals; a running script is terminated; any
/// other is launched.
pub(super) fn pick(running: bool, alt: bool) -> Pick {
    match (running, alt) {
        (_, true) => Pick::Reveal,
        (true, false) => Pick::Terminate,
        (false, false) => Pick::Launch,
    }
}

/// What AutoLaunch starts (`M:1620-1650`): every entry of its folder but hidden ones and
/// `__pycache__`, folders first and then names ignoring case (iTerm2 takes the file system's
/// order; Decision). Nobody asked for these, so an entry `local` refuses (a link reaching a
/// network path, `scripts::stays_local`) is skipped before anything probes it.
pub(super) fn autolaunch_targets(listing: &[(String, bool)], folder: &Path, local: &dyn Fn(&Path) -> bool) -> Vec<PathBuf> {
    let mut kept: Vec<&(String, bool)> = listing.iter().filter(|(name, _)| !name.starts_with('.') && name != "__pycache__").collect();
    kept.sort_by(|(a, a_dir), (b, b_dir)| b_dir.cmp(a_dir).then_with(|| a.to_lowercase().cmp(&b.to_lowercase())).then_with(|| a.cmp(b)));
    kept.into_iter().map(|(name, _)| folder.join(name)).filter(|p| local(p)).collect()
}

/// `shouldRunAutoLaunchScripts` (`M:1600-1606`): once, and only when the folder exists.
pub(super) fn should_autolaunch(ran: bool, folder_exists: bool) -> bool {
    !ran && folder_exists
}

/// A MenuItemRequest's identifier as one of the Scripts menu's items (`H:1526-1539` searches
/// every item, the scripts' and their "/Reveal/" alternates too): the script's path and its
/// item (the alternate reveals).
pub(super) fn script_menu_answer<'a>(identifier: &str, items: &'a [(PathBuf, crate::menus::Item)]) -> Option<&'a (PathBuf, crate::menus::Item)> {
    items.iter().find(|(_, item)| item.identifier == identifier)
}

/// The menu id Tandem gives a script item (`SCRIPT_ID` and the item's identifier), which
/// `script_picked` answers.
pub(super) fn script_item_id(item: &crate::menus::Item) -> String {
    format!("{SCRIPT_ID}{}", item.identifier)
}

/// What a full environment is built from (`M:1009-1016`): the version its marker recorded,
/// else its `setup.cfg` pin, else the default; the dependencies `setup.cfg` lists.
pub(super) fn container_inputs(container: &Path) -> (String, Vec<String>) {
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
                let ask = malformed_ask(p.clone(), &sl::entry_name(&kind));
                return self.ask_scripts(ask);
            }
            Kind::NeedsRebuild { .. } => {
                let name = sl::entry_name(&kind);
                if !req.explicit {
                    return self.script_log_app(sl::env_missing_line(&name));
                }
                return self.ask_scripts(rebuild_ask(req, &name));
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
                // iTerm2's alert is modal, so picking the script again cannot queue a second copy.
                if !self.script_after_enable.iter().any(|r| r.path == req.path) {
                    self.script_after_enable.push(req);
                }
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
            Ok(mut p) => {
                if let Some(e) = self.script_history.find_mut(&key) {
                    e.pids = vec![p.pid()];
                }
                p.cookie = cookie;
                self.script_procs.insert(key, p);
            }
            Err(e) => {
                // The cookie will never be used.
                self.api_jar.lock().unwrap().remove(&cookie);
                let stamp = self.history_stamp();
                self.script_history.add_log(&key, &sl::failed_to_launch(&e), &stamp);
                self.script_history.stop(&key);
                let text = sl::error_running(&main.display().to_string(), &e);
                self.ask_scripts(ask(sl::ERROR_RUNNING_TITLE, &text, &[crate::api_auth::OK], None, warnings::Kind::Persistent, Then::ErrorRunning));
            }
        }
        self.refresh_scripts_menu();
        self.redraw();
    }

    /// Terminate (`SH:190-196`): the note in the log, the script and everything it started
    /// ended, and its API connections closed.
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

    /// Restart (`SC:224-232`): Terminate (only a running entry with one process,
    /// `terminateScriptOnRow:`), then the same launch again, as a person's.
    pub(super) fn relaunch_script(&mut self, id: &str) {
        let Some(e) = self.script_history.find(id) else { return };
        let (relaunch, ends) = (e.relaunch.clone(), e.running && e.only_pid().is_some());
        if ends {
            self.terminate_script(id);
        }
        if let Some(r) = relaunch {
            self.launch_script(Request { path: r.path, args: r.args, explicit: true });
        }
    }

    /// Moves each script's output into its entry (a piece per pass); an ended script's entry
    /// gets the exit text and, unless the person terminated it, a notification
    /// (`L:960-1012`), then stops. Redraws only when something changed.
    pub(super) fn scripts_tick(&mut self) {
        // What ended scripts left running is let go once it has ended too.
        self.script_jobs_left.retain(|j| j.active_processes() > 0);
        if self.script_procs.is_empty() {
            return;
        }
        let mut changed = false;
        let ids: Vec<String> = self.script_procs.keys().cloned().collect();
        for id in ids {
            let Some(p) = self.script_procs.get_mut(&id) else { continue };
            let bytes = p.take_output(OUTPUT_PER_PASS);
            let text = p.decoder.push(&bytes);
            let code = p.exit_code();
            if !text.is_empty() {
                let stamp = self.history_stamp();
                self.script_history.add_log(&id, &text, &stamp);
                changed = true;
            }
            if let Some(code) = code {
                self.script_ended(&id, code);
                changed = true;
            }
        }
        if changed {
            // A script that ended loses its check mark.
            self.refresh_scripts_menu();
            self.redraw();
        }
    }

    fn script_ended(&mut self, id: &str, code: u32) {
        if let Some(p) = self.script_procs.remove(id) {
            // A one-time cookie the script never used goes (using it removed it already).
            self.api_jar.lock().unwrap().remove(&p.cookie);
            // What the script started keeps running, as in iTerm2 (`L:960-1010` signals
            // nothing); its job is kept until that ends, or Tandem quits.
            let job = p.into_job();
            if job.active_processes() > 0 {
                self.script_jobs_left.push(job);
            }
        }
        if let Some(text) = sl::exit_text(code) {
            let stamp = self.history_stamp();
            self.script_history.add_log(id, &text, &stamp);
            let failed = self.script_history.find(id).filter(|e| sl::notifies(code, e.terminated_by_user)).map(|e| e.name.clone());
            if let Some(name) = failed {
                match crate::notify::show_with(sl::SCRIPT_FAILED, &sl::ended_unexpectedly(&name), self.globals.bool("silentUserNotifications")) {
                    Ok(()) => {
                        self.last_notified_script = Some(id.to_string());
                        self.last_notified = None;
                        self.last_notified_debug = None;
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
        self.refresh_scripts_menu();
    }

    /// A failed script's notification was clicked, or the function error alert's "Reveal in
    /// Script Console": its Script Console entry shows its end (`revealTailOfHistoryEntry:`,
    /// `reveal_in_console`) on the next loop pass, which has the event loop to open the
    /// console with.
    pub(super) fn reveal_script_entry(&mut self, id: &str) {
        self.script_console_reveal = Some(id.to_string());
    }

    /// `couldLaunchScriptWithAbsolutePath:` (`M:1046-1079`): a basic script, a full
    /// environment, or one that needs rebuilding. A path reached through a link to a network
    /// path is never probed.
    pub(super) fn could_launch(path: &Path) -> bool {
        crate::scripts::ancestors_stay_local(path, &crate::netpath::read_reparse) && matches!(sl::classify(path, &crate::scripts::RealFs, &first_line), Kind::Basic { .. } | Kind::Full { .. } | Kind::NeedsRebuild { .. })
    }

    /// Selects `path` in Explorer; nothing runs it.
    pub(super) fn reveal_path(&self, path: &Path) {
        use std::os::windows::process::CommandExt;
        let text = path.to_string_lossy().replace('/', "\\");
        let Some(arg) = crate::semantic::explorer_select_arg(&text) else { return };
        let mut cmd = std::process::Command::new("explorer.exe");
        cmd.raw_arg(arg);
        // Explorer shows its own message for a path it cannot select.
        let _ = cmd.spawn();
    }

    /// Rebuilds the Scripts menu after its fixed items (Manage and the line) when what it
    /// lists or checks changed: after a scan, and when a script starts or stops.
    pub(super) fn refresh_scripts_menu(&mut self) {
        let history = &self.script_history;
        let entries = menu_entries(&self.scripts_tree, &|p| history.running_with_path(p).is_some());
        let Some(bar) = &mut self.menu_bar else { return };
        if bar.script_entries == entries {
            return;
        }
        let Some((sub, fixed)) = &bar.scripts else { return };
        while sub.remove_at(*fixed).is_some() {}
        let mut errors = Vec::new();
        fill_scripts(sub, &entries, &mut errors);
        bar.script_entries = entries;
        if let Some(e) = errors.first() {
            self.banners.set("menu", format!("Scripts menu: {e}"));
        }
    }

    /// A pick from the Scripts menu's scripts, or from their items in Open Quickly (where the
    /// "/Reveal/" alternate is an item of its own). False for other ids.
    pub(super) fn script_picked(&mut self, id: &str) -> bool {
        let Some(rest) = id.strip_prefix(SCRIPT_ID) else { return false };
        let (path, reveal) = match rest.strip_prefix(REVEAL_PREFIX) {
            Some(path) => (path, true),
            None => (rest, false),
        };
        // Alt held at the pick stands in for the Option alternate (m7a's precedent).
        let alt = reveal || unsafe { windows_sys::Win32::UI::Input::KeyboardAndMouse::GetKeyState(0x12) } < 0;
        self.pick_script(Path::new(path), alt);
        // The menu toggled the item's check itself: it is rebuilt from what runs.
        if let Some(bar) = &mut self.menu_bar {
            bar.script_entries.clear();
        }
        self.refresh_scripts_menu();
        true
    }

    /// Reveals, terminates or launches a script as its menu item does
    /// (`launchOrTerminateScript:`, `revealScript:`).
    fn pick_script(&mut self, path: &Path, reveal: bool) {
        let running = self.script_history.running_with_path(path).map(|e| e.id.clone());
        match pick(running.is_some(), reveal) {
            Pick::Reveal => self.reveal_path(path),
            Pick::Terminate => {
                if let Some(id) = running {
                    self.terminate_script(&id);
                }
            }
            Pick::Launch => self.launch_script(Request { path: path.to_path_buf(), args: Vec::new(), explicit: true }),
        }
    }

    /// Scripts > Manage > Reveal Scripts in Explorer (`M:642-650`): the folder is made first;
    /// as in iTerm2 a failure to make it is ignored and the folder revealed anyway.
    pub(super) fn reveal_scripts_folder(&mut self) {
        let folder = self.scripts_folder();
        let _ = std::fs::create_dir_all(&folder);
        self.reveal_path(&folder);
    }

    /// The script items and their alternates for Open Quickly's menu items, which in iTerm2
    /// walk the live menu and so list every script a second time (`OQM:369-410`): menu id,
    /// title, menu path and whether it is an alternate.
    pub(super) fn quickly_script_menu_items(&self) -> Vec<(String, String, Vec<String>, bool)> {
        script_items(&self.scripts_tree, &[SCRIPTS_MENU.to_string()])
            .into_iter()
            .map(|(_, item)| (script_item_id(&item), item.label().to_string(), item.path[..item.path.len() - 1].to_vec(), item.alternate))
            .collect()
    }

    /// AutoLaunch (`runAutoLaunchScriptsIfNeeded`, `M:631-641`, `M:1587-1650`), once: every
    /// entry of the AutoLaunch folder is launched as nobody's choice, so nothing runs while
    /// the API is off and nothing is downloaded.
    pub(super) fn run_autolaunch(&mut self) {
        let ran = std::mem::replace(&mut self.autolaunch_ran, true);
        if ran {
            return;
        }
        let folder = crate::scripts::autolaunch_folder(&self.scripts_folder());
        // The folder, or one above it, may be a link to a share: nothing there is probed.
        if !crate::scripts::ancestors_stay_local(&folder, &crate::netpath::read_reparse) {
            return self.script_log_app(crate::scripts::autolaunch_not_local(&folder));
        }
        if !should_autolaunch(ran, folder.is_dir()) {
            return;
        }
        use crate::scripts::Fs;
        for path in autolaunch_targets(&crate::scripts::RealFs.list(&folder), &folder, &crate::scripts::stays_local) {
            self.launch_script(Request { path, args: Vec::new(), explicit: false });
        }
    }

    /// Open Quickly's script item (`OQW:444-448`): the path relative to the Scripts folder,
    /// launched as a person's choice.
    pub(super) fn launch_relative_script(&mut self, name: &str) {
        let path = name.split('/').fold(self.scripts_folder(), |p, part| p.join(part));
        self.launch_script(Request { path, args: Vec::new(), explicit: true });
    }

    /// MenuItemRequest for a script item (`H:3524-3547`): None when `identifier` is none of
    /// them; otherwise the answer (a script item is always enabled; checked while it runs, a
    /// reveal item never) and, unless only asked, the pick the menu would make.
    pub(super) fn script_menu_request(&mut self, identifier: &str, query_only: bool) -> Option<crate::api_proto::pb::MenuItemResponse> {
        let items = script_items(&self.scripts_tree, &[SCRIPTS_MENU.to_string()]);
        let (path, item) = script_menu_answer(identifier, &items)?;
        let reveal = item.alternate;
        let checked = !reveal && self.script_history.running_with_path(path).is_some();
        let (answer, run) = crate::menus::menu_answer(Some(item), true, checked, query_only);
        if run {
            let path = path.clone();
            self.pick_script(&path, reveal);
            self.refresh_scripts_menu();
        }
        Some(answer)
    }
}

/// The display name of the app that edits `.py` files (the "edit" verb's,
/// `AssocQueryStringW(ASSOCSTR_FRIENDLYAPPNAME)`); None when there is no such verb.
fn edit_app_name() -> Option<String> {
    use windows_sys::Win32::UI::Shell::{AssocQueryStringW, ASSOCF_INIT_IGNOREUNKNOWN, ASSOCF_NOTRUNCATE, ASSOCSTR_FRIENDLYAPPNAME};
    let wide = |s: &str| s.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
    let (ext, verb) = (wide(".py"), wide("edit"));
    let mut buf = vec![0u16; 1024];
    let mut len = buf.len() as u32;
    let hr = unsafe { AssocQueryStringW(ASSOCF_INIT_IGNOREUNKNOWN | ASSOCF_NOTRUNCATE, ASSOCSTR_FRIENDLYAPPNAME, ext.as_ptr(), verb.as_ptr(), buf.as_mut_ptr(), &mut len) };
    if hr != 0 {
        return None;
    }
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    let name = String::from_utf16_lossy(&buf[..end]);
    (!name.trim().is_empty()).then_some(name)
}

impl App {
    /// Scripts > Manage > New Python Script (`M:1091-1127`): the picker. Its Python version
    /// popup shows the default until the versions the backend can build arrive (a `Minors`
    /// job off the UI thread; `M:1350-1375`).
    pub(super) fn new_python_script(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(w) = &self.script_picker {
            return w.focus();
        }
        let Some(g) = self.gpu.as_ref() else { return };
        match PickerWindow::open(event_loop, &g.instance, &g.adapter, &g.device, &g.queue, Vec::new(), rt::DEFAULT_PYTHON) {
            Ok(w) => self.script_picker = Some(w),
            Err(e) => return self.banners.set("window", format!("Could not open the window: {e}")),
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let notify = self.notify.clone();
        std::thread::spawn(move || {
            // Finding an installed Python runs it, so this is off the UI thread as well.
            let minors = super::script_runtime_ui::find_backend().map_or_else(Vec::new, |backend| {
                let mut runner = RealRunner { lines: std::sync::mpsc::channel().0, timeout: crate::script_provision::PROBE_TIMEOUT };
                match crate::script_provision::run_job(&Job::Minors { backend }, &super::script_runtime_ui::uv_dir(), &mut runner) {
                    Ok(Outcome::Minors(v)) => v,
                    _ => Vec::new(),
                }
            });
            let _ = tx.send(minors);
            notify();
        });
        self.script_picker_minors = Some(rx);
    }

    /// The picker's versions arrived.
    pub(super) fn script_picker_tick(&mut self) {
        let Some(rx) = &self.script_picker_minors else { return };
        match rx.try_recv() {
            Ok(versions) => {
                self.script_picker_minors = None;
                if let Some(w) = self.script_picker.as_mut() {
                    w.set_versions(versions);
                }
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
            Err(std::sync::mpsc::TryRecvError::Disconnected) => self.script_picker_minors = None,
        }
    }

    /// Events for the picker. False for other windows. Closing it is Cancel.
    pub(super) fn script_picker_event(&mut self, window: WindowId, event: &WindowEvent) -> bool {
        let Some(w) = self.script_picker.as_mut().filter(|w| w.id() == window) else { return false };
        let answer = match event {
            WindowEvent::CloseRequested => PickerEvent::Cancel,
            WindowEvent::RedrawRequested => w.redraw(),
            other => {
                w.event(other);
                PickerEvent::None
            }
        };
        match answer {
            PickerEvent::None => {}
            PickerEvent::Cancel => {
                self.script_picker = None;
                self.script_picker_minors = None;
            }
            PickerEvent::Chosen { env, template, dependencies, python } => {
                // The picker closes before the save dialog opens (`M:1129-1131`).
                self.script_picker = None;
                self.script_picker_minors = None;
                self.new_script_save(Plan { env, template, dependencies, python, chosen: PathBuf::new() });
            }
        }
        true
    }

    /// The save dialog (`runSavePanelForNewScriptWithPicker:`, `M:1439-1477`), opening in the
    /// Scripts folder, a basic script's filtered to `.py`. A place outside the Scripts folder
    /// is refused ("Invalid Folder", `M:1693-1709`), then a name with a space ("Spaces Not
    /// Allowed"); each asks, then the dialog opens again or the script is made.
    pub(super) fn new_script_save(&mut self, mut plan: Plan) {
        let scripts = self.scripts_folder();
        let _ = std::fs::create_dir_all(&scripts);
        let mut dialog = rfd::FileDialog::new().set_directory(&scripts);
        if plan.env == Env::Basic {
            dialog = dialog.add_filter("Python script", &["py"]);
        }
        let Some(chosen) = dialog.save_file() else { return };
        plan.chosen = chosen;
        let ok = [crate::api_auth::OK];
        if !sn::under_scripts(&plan.chosen, &scripts) {
            // Persistent in iTerm2 (`LOCATION_ID`): never silenced.
            let ask = ask(sn::INVALID_FOLDER, &sn::location_restricted(&scripts), &ok, None, warnings::Kind::Persistent, Then::InvalidFolder(plan));
            return self.ask_scripts(ask);
        }
        let name = plan.chosen.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if sn::spaceless(&name).is_some() {
            let buttons = [sn::USE_UNDERSCORE, sn::CHANGE_NAME];
            let ask = ask(sn::SPACES_HEADING, sn::SPACES_TEXT, &buttons, None, warnings::Kind::Persistent, Then::Spaces(plan));
            return self.ask_scripts(ask);
        }
        self.create_new_script(plan);
    }

    /// "Use _ Instead of Space": the same place with every space in the name as "_".
    pub(super) fn spaces_replaced(&mut self, mut plan: Plan) {
        let name = plan.chosen.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if let Some(safe) = sn::spaceless(&name) {
            plan.chosen.set_file_name(safe);
        }
        self.create_new_script(plan);
    }

    /// `reallyCreateNewPythonScriptAtURL:` (`M:1173-1226`): a basic script is written at once;
    /// a full environment is built first (asking before any download), then written.
    fn create_new_script(&mut self, plan: Plan) {
        match plan.env {
            Env::Basic => self.finish_new_script(plan),
            Env::Full => {
                let container = sn::container(&plan.chosen, &self.scripts_folder());
                let (python, dependencies) = (plan.python.clone(), plan.dependencies.clone());
                self.ensure_environment(Target::Container(container), &python, &dependencies, true, After::NewScript(plan));
            }
        }
    }

    /// `finishInstallingNewPythonScriptForPicker:url:` (`M:1228-1264`): the template is
    /// written (a full environment's main with its `setup.cfg`), the Scripts folder scanned,
    /// then "Open new script in %@?" names the `.py` edit verb's app; with no edit verb the
    /// script is revealed.
    pub(super) fn finish_new_script(&mut self, plan: Plan) {
        let scripts = self.scripts_folder();
        let dest = sn::destination(plan.env, &plan.chosen, &scripts);
        // Under uv iTerm2 finds no runtime version, so the shebang names plain python3
        // (`M:1542-1558`).
        let text = sn::template(plan.env, plan.template, None);
        let files = match plan.env {
            Env::Basic => vec![(dest.clone(), text)],
            Env::Full => {
                let name = plan.chosen.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let container = sn::container(&plan.chosen, &scripts);
                let read = |f: &str| std::fs::read_to_string(container.join(f)).ok();
                // The version the build made, which the launcher may have resolved to another.
                let built = sn::built_python(read(rt::MARKER).as_deref(), &plan.python);
                sn::full_environment_files(&container, &name, &plan.dependencies, &built, &text, read(rt::SETUP_CFG).as_deref())
            }
        };
        if let Err(e) = sn::write_files(&files) {
            return self.banners.set("scripts", e);
        }
        if let Some(scanner) = &self.scripts_scan {
            scanner.rescan();
        }
        match edit_app_name() {
            Some(app) => {
                let buttons = [crate::api_auth::OK, sn::SHOW_IN_EXPLORER];
                let ask = ask(sn::WARNING, &sn::open_in(&app), &buttons, Some(sn::OPEN_IN_ID), warnings::Kind::Permanent, Then::OpenNewScript(dest));
                self.ask_scripts(ask);
            }
            None => self.reveal_path(&dest),
        }
    }

    /// Opens a new script in its editor with the "edit" verb, never "open" (which runs a
    /// `.py`); when that fails the script is revealed.
    pub(super) fn edit_new_script(&self, path: &Path) {
        // ShellExecute takes a file only as a LocalPath (round 5 item 4).
        let edited = crate::localfs::LocalPath::judge(path).is_some_and(|p| crate::localfs::shell_verb("edit", &p, None, false).is_ok());
        if !edited {
            self.reveal_path(path);
        }
    }
}

/// Why an import's install did not finish.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Failure {
    Error(String),
    /// The person declined a download (iTerm2's cancelation error).
    Canceled,
}

/// The import under way (`IM:69-155`).
pub(super) struct Import {
    /// The file picked (Try Again imports it again).
    source: PathBuf,
    /// An archive the scan found: moved to the Recycle Bin once installed.
    trash: bool,
    /// Imported with no questions and no result alert, into AutoLaunch when true (iTerm2's
    /// `avoidUI` with `offerAutoLaunch`, Import All Settings and Data); None for a person's
    /// import.
    quiet: Option<bool>,
    /// `<Scripts>\.installing-<UUID>`: the archive is extracted here, on the Scripts folder's
    /// volume, so the install is a rename; removed at the end, and at the next start if Tandem
    /// ended first.
    staging: PathBuf,
    /// The extraction's layout while tar.exe runs off the UI thread.
    extracting: Option<std::sync::mpsc::Receiver<Result<sa::Layout, String>>>,
    layout: Option<sa::Layout>,
    /// The script's name, where it is installed, and where a replaced one was moved aside.
    name: String,
    dest: PathBuf,
    backup: Option<PathBuf>,
    /// Replace Script right after a Terminate: the backup path and until when moving the old
    /// script aside is tried again (its files may be held for a moment), from `import_tick`.
    moving: Option<(PathBuf, std::time::Instant)>,
}

/// A file or folder removed, a link without touching what it names; errors ignored (iTerm2
/// passes `error:nil`).
pub(super) fn remove_path(p: &Path) {
    let Ok(m) = p.symlink_metadata() else { return };
    if m.file_type().is_symlink() {
        let _ = std::fs::remove_file(p).or_else(|_| std::fs::remove_dir(p));
    } else if m.is_dir() {
        let _ = std::fs::remove_dir_all(p);
    } else {
        let _ = std::fs::remove_file(p);
    }
}

/// How long moving a just-terminated script aside is tried, and how often.
const MOVE_ASIDE_FOR: std::time::Duration = std::time::Duration::from_secs(1);
const MOVE_ASIDE_EVERY: std::time::Duration = std::time::Duration::from_millis(50);

/// The Recycle Bin (`trashItemAtURL:`): `SHFileOperationW` with undo allowed, no questions
/// and no error window; a failure is ignored, as iTerm2 ignores it.
pub(super) fn recycle(path: &Path) {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Shell::{SHFileOperationW, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT, FO_DELETE, SHFILEOPSTRUCTW};
    // The list of paths ends with two NULs.
    let mut from: Vec<u16> = path.as_os_str().encode_wide().collect();
    from.extend([0, 0]);
    let mut op: SHFILEOPSTRUCTW = unsafe { std::mem::zeroed() };
    op.wFunc = FO_DELETE;
    op.pFrom = from.as_ptr();
    op.fFlags = (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_NOERRORUI | FOF_SILENT) as u16;
    unsafe { SHFileOperationW(&mut op) };
}

/// Runs tar.exe from System32 with `args` (never one from PATH).
pub(super) fn run_tar(args: Vec<String>) -> Result<(), String> {
    let mut runner = RealRunner::new(std::sync::mpsc::channel().0);
    use crate::script_provision::Runner;
    runner.run(&crate::script_provision::Step { program: RealRunner::system_tool("tar.exe")?, args, env: Vec::new() }).map(|_| ())
}

pub(super) use sa::stage_entries;

/// Extracts `zip` into `to` (`unzipURL:`) and reads its layout (off the UI thread). Nothing
/// that could reach outside `to` gets through: the file must be a zip whose every entry is a
/// plain file or folder with a safe name (`zip_check`) before tar.exe sees it; after, the
/// extracted tree must hold no link, junction or other reparse point and no file with
/// another hard link; the script's name must be safe; and a full environment's hidden
/// entries (`.venv`, its build and swap folders) are removed, so provisioning starts clean.
fn extract_archive(zip: &Path, to: &Path) -> Result<sa::Layout, String> {
    // The archive came from outside: judged before it is read.
    let bytes = crate::localfs::judge_io(zip).and_then(|z| crate::localfs::read(&z)).map_err(|e| sa::unzip_failed(&e.to_string()))?;
    crate::zip_check::check(&bytes).map_err(|e| sa::unzip_failed(&e))?;
    std::fs::create_dir_all(to).map_err(|e| sa::unzip_failed(&e.to_string()))?;
    run_tar(sa::extract_tar_args(zip, to)).map_err(|e| sa::unzip_failed(&e))?;
    if let Some(entry) = sa::unsafe_entry(to, &stage_entries) {
        return Err(sa::unzip_failed(&sa::unsafe_entry_text(entry.strip_prefix(to).unwrap_or(&entry))));
    }
    let layout = sa::layout(to, &crate::scripts::RealFs, &|p: &Path| std::fs::read_to_string(p).ok()).map_err(|e| match e {
        sa::LayoutError::Legacy => sa::LEGACY_ARCHIVE.to_string(),
        sa::LayoutError::NoValidScript => sa::NO_VALID_SCRIPT.to_string(),
    })?;
    let name = match &layout {
        sa::Layout::Basic { file } => file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        sa::Layout::Full { name, .. } => name.clone(),
    };
    if !sa::safe_script_name(&name) {
        return Err(sa::NO_VALID_SCRIPT.to_string());
    }
    if let sa::Layout::Full { folder, .. } = &layout {
        for e in std::fs::read_dir(folder).map_err(|e| sa::unzip_failed(&e.to_string()))?.flatten() {
            if e.file_name().to_string_lossy().starts_with('.') {
                remove_path(&e.path());
            }
        }
    }
    Ok(layout)
}

/// `urlContainsScript:fullEnvironment:` (`EX:207-240`): a `.py` file (basic), or a folder
/// with `setup.cfg`, an environment (the marker, or iTerm2's legacy `iterm2env`) and its
/// `<name>\<name>.py` (full). None when neither.
fn export_kind(path: &Path) -> Option<bool> {
    if crate::localfs::is_local_file(path) && path.extension().is_some_and(|e| e == "py") {
        return Some(false);
    }
    let name = path.file_name()?.to_string_lossy().into_owned();
    let runtime = crate::localfs::local_exists(&path.join(rt::MARKER)) || crate::localfs::local_exists(&path.join("iterm2env"));
    (crate::localfs::is_local_dir(path) && crate::localfs::local_exists(&path.join(sa::SETUP_CFG)) && runtime && crate::localfs::local_exists(&path.join(&name).join(format!("{name}.py")))).then_some(true)
}

/// Writes `zip` from `<parent>\<name>`'s `setup.cfg` and inner folder (`EX:102-150`); a
/// partial zip is removed.
fn zip_to(zip: &Path, parent: &Path, name: &str) -> Result<(), String> {
    run_tar(sa::export_tar_args(zip, parent, name)).map_err(|_| {
        remove_path(zip);
        sa::ZIP_FAILED.to_string()
    })
}

/// The name a script's zip takes: a full environment's folder name, a basic script's stem.
fn export_zip_name(path: &Path, full: bool) -> String {
    let name = if full { path.file_name() } else { path.file_stem() };
    name.map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Exports a script on the Desktop, as `<name>.zip` or the first free `<name> (n).zip`. The
/// zip's path, or the error's text.
fn export_one(path: &Path, autolaunch: bool, desktop: &Path) -> Result<PathBuf, String> {
    let full = export_kind(path).ok_or_else(|| sa::NO_SCRIPT_FOUND.to_string())?;
    let out = sa::export_name(desktop, &export_zip_name(path, full), &|p: &Path| p.exists());
    export_script_to(path, &out, autolaunch)?;
    Ok(out)
}

/// `exportScriptAtURL:` (`EX:37-90`), unsigned, into `zip`: a basic script is staged as a
/// full environment in a temporary folder (`setup.cfg` with the default version, the script
/// in `<name>\<name>`, `metadata.json`); a full one gets `metadata.json` written into its own
/// folder; then the zip. The error's text when it fails. Scripts > Manage > Export and Export
/// All Settings and Data both export through it.
pub(super) fn export_script_to(path: &Path, zip: &Path, autolaunch: bool) -> Result<(), String> {
    let full = export_kind(path).ok_or_else(|| sa::NO_SCRIPT_FOUND.to_string())?;
    let name = export_zip_name(path, full);
    if full {
        // iTerm2 ignores a failure to write it.
        let _ = std::fs::write(path.join(sa::METADATA), sa::metadata_json(autolaunch));
        let parent = path.parent().unwrap_or(path);
        return zip_to(zip, parent, &name);
    }
    let temp = std::env::temp_dir().join(format!("tandem-export-{}", crate::api_auth::fresh_key()));
    let staged = temp.join(&name);
    // The one helper for a full environment's files: its `setup.cfg`, then the script copied
    // over the main it names (`<name>\<name>\<name>.py`, the script's own file name) byte for
    // byte.
    let files = sn::full_environment_files(&staged, &name, &[], rt::DEFAULT_PYTHON, "", None);
    let result = sn::write_files(&files)
        .and_then(|_| {
            let main = &files[1].0;
            std::fs::copy(path, main).map(|_| ()).map_err(|e| format!("Could not copy {}: {e}", path.display()))
        })
        .and_then(|_| {
            let _ = std::fs::write(staged.join(sa::METADATA), sa::metadata_json(autolaunch));
            zip_to(zip, &temp, &name)
        });
    remove_path(&temp);
    result
}

/// `urlIsScript:` (`EX:245-247`): what Export All Settings and Data exports.
pub(super) fn is_exportable_script(path: &Path) -> bool {
    export_kind(path).is_some()
}

/// Puts right what an import that did not finish left in the Scripts folder
/// (`recoverStaleReplaceBackupsInDirectory:`, `IM:476-525`): a staging folder is removed; a
/// replaced script's backup is removed when its script is back as a real item (not a link),
/// else it is restored.
fn recover_in(folder: &Path, read_link: &dyn Fn(&Path) -> Option<PathBuf>) {
    // A folder reached through a link to a network path is not touched.
    if !crate::scripts::ancestors_stay_local(folder, read_link) {
        return;
    }
    let Ok(entries) = std::fs::read_dir(folder) else { return };
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let path = e.path();
        if name.starts_with(sa::STAGING_PREFIX) {
            remove_path(&path);
            continue;
        }
        let Some(script) = sa::stale_backup(&name) else { continue };
        let target = folder.join(&script);
        if target.symlink_metadata().is_ok_and(|m| !m.file_type().is_symlink()) {
            remove_path(&path);
        } else {
            remove_path(&target);
            let _ = std::fs::rename(&path, &target);
        }
    }
}

impl App {
    /// Scripts > Manage > Open Python REPL (`openREPL:`, `AD:3553-3595`): the default shared
    /// environment is made ready (asking before any download), then the API is started as a
    /// person's choice, then the window opens.
    pub(super) fn open_repl(&mut self) {
        self.ensure_environment(Target::Shared(rt::DEFAULT_PYTHON.to_string()), rt::DEFAULT_PYTHON, &[], true, After::Repl);
    }

    /// The REPL's interpreter is ready: with the API off, "Enable Python API?" first
    /// (`sharedInstanceFromExplicitUserAction`); the window opens on the next loop pass.
    pub(super) fn repl_ready(&mut self, python: PathBuf) {
        if self.api_on {
            self.repl_pending = Some(python);
        } else {
            self.repl_after_enable = Some(python);
            self.queue_api_alert(Alert::Enable);
        }
    }

    /// The default environment could not be made: the newest provisioned shared one is used
    /// (they all carry iterm2), else "Python Environment Unavailable" (`AD:3562-3577`).
    pub(super) fn repl_environment_failed(&mut self, e: &str) {
        let uv = super::script_runtime_ui::uv_dir();
        let names: Vec<String> = std::fs::read_dir(rt::venvs_dir(&uv)).map(|rd| rd.flatten().map(|d| d.file_name().to_string_lossy().into_owned()).collect()).unwrap_or_default();
        match crate::script_provision::newest_shared_python(&uv, &names, &|p: &Path| p.is_file()) {
            Some(python) => self.repl_ready(python),
            None => {
                let ask = ask(rt::ENV_UNAVAILABLE_HEADING, &rt::repl_prepare_failed(e), &[crate::api_auth::OK], None, warnings::Kind::Persistent, Then::Nothing);
                self.ask_scripts(ask);
            }
        }
    }

    /// `launchREPLWithCommand:arguments:` (`AD:3528-3551`): a Single cookie and a fresh key,
    /// the entry "REPL" (no path, no relaunch, no process: Terminate stays off and the
    /// connection's close stops it), a window with the default profile running the
    /// interpreter with `-m asyncio` and the cookie and key, and the banner shown first. The
    /// window is not short-lived and stays open when the REPL ends. The cookie is the pane's,
    /// so it leaves the jar with the pane.
    pub(super) fn open_pending_repl(&mut self, event_loop: &ActiveEventLoop) {
        let Some(python) = self.repl_pending.take() else { return };
        let key = crate::api_auth::fresh_key();
        let cookie = self.api_jar.lock().unwrap().mint(crate::api_auth::CookieUse::Single);
        self.script_history.add(Entry::new(&key, sa::REPL_NAME, api_ui::unix_secs()));
        let [a, b] = sa::repl_args();
        self.start_command_override = Some(format!("\"{}\" {a} {b}", python.display()));
        self.start_env_override = Some(vec![format!("ITERM2_COOKIE={cookie}"), format!("ITERM2_KEY={key}")]);
        let made = self.open_window(event_loop, None);
        self.start_command_override = None;
        self.start_env_override = None;
        let pane = made.ok().and_then(|_| self.tabs.get(self.active).map(|t| t.focused));
        if let Some(p) = pane {
            // Held open when the interpreter ends (no CloseOnTermination,
            // `iTermController.m:2189-2191`): it ends as No Action and is never restarted.
            if let Some(pane) = self.panes.get_mut(&p) {
                pane.hold_open = true;
            }
        }
        match pane.and_then(|p| self.panes.get_mut(&p)) {
            Some(p) => p.feed_output(sa::repl_banner().as_bytes()),
            None => {
                self.api_jar.lock().unwrap().remove(&cookie);
                self.script_history.stop(&key);
            }
        }
        self.redraw_all();
    }

    /// Scripts > Manage > Import (`chooseAndImportScript`, `M:733-751`): zip, its and py files,
    /// several at once, each imported in turn.
    pub(super) fn import_scripts(&mut self) {
        let Some(files) = rfd::FileDialog::new().add_filter("Script archives and Python scripts", &["zip", "its", "py"]).pick_files() else { return };
        for f in files {
            self.import_path(f, false);
        }
    }

    /// `importScriptFromURL:` (`IM:37-155`), one at a time: another waits behind the one under
    /// way (iTerm2's serial install queue). A `.py` is copied into the Scripts folder; a
    /// signed `.its` is refused; anything else is extracted off the UI thread.
    pub(super) fn import_path(&mut self, path: PathBuf, trash: bool) {
        self.import_start(path, trash, None);
    }

    /// An import with no UI (`avoidUI`, `IE:1069-1080`): no "Script Already Exists" (the old
    /// script is replaced), no result alert (a failure is logged in the app-wide console
    /// entry), and with `autolaunch` the script goes into the AutoLaunch folder
    /// (`shouldAutoLaunchWhenTrusted`, `iTermScriptArchive.m:216-230`). A download is still
    /// asked for.
    pub(super) fn import_quietly(&mut self, path: PathBuf, autolaunch: bool) {
        self.import_start(path, false, Some(autolaunch));
    }

    fn import_start(&mut self, path: PathBuf, trash: bool, quiet: Option<bool>) {
        if self.importing.is_some() {
            return self.import_queue.push_back((path, trash, quiet));
        }
        let scripts = self.scripts_folder();
        let _ = std::fs::create_dir_all(&scripts);
        match path.extension().and_then(|e| e.to_str()) {
            Some("py") => {
                let to = scripts.join(path.file_name().unwrap_or_default());
                // `copyItemAtURL:` never overwrites: an existing script is an error.
                let copied = crate::localfs::judge_io(&path).and_then(|p| crate::localfs::open(&p)).and_then(|mut from| {
                    let mut out = std::fs::OpenOptions::new().write(true).create_new(true).open(&to)?;
                    std::io::copy(&mut from, &mut out).map(|_| ())
                });
                self.import_result(path, trash, quiet, copied.map(|_| Some(to)).map_err(|e| e.to_string()));
            }
            Some("its") => self.import_result(path, trash, quiet, Err(sa::its_refused())),
            _ => {
                let staging = scripts.join(format!("{}{}", sa::STAGING_PREFIX, crate::api_auth::fresh_key()));
                let (tx, rx) = std::sync::mpsc::channel();
                let notify = self.notify.clone();
                let (zip, to) = (path.clone(), staging.clone());
                std::thread::spawn(move || {
                    let _ = tx.send(extract_archive(&zip, &to));
                    notify();
                });
                self.importing = Some(Import { source: path, trash, quiet, staging, extracting: Some(rx), layout: None, name: String::new(), dest: PathBuf::new(), backup: None, moving: None });
            }
        }
    }

    /// The extraction finished: its layout goes on, or its error ends the import. A Replace
    /// waiting for a terminated script's files is tried again; when to come back for it.
    pub(super) fn import_tick(&mut self) -> Option<std::time::Instant> {
        if let Some((backup, until)) = self.importing.as_ref().and_then(|i| i.moving.clone()) {
            return self.move_aside_again(backup, until);
        }
        let rx = self.importing.as_ref().and_then(|i| i.extracting.as_ref())?;
        let result = match rx.try_recv() {
            Ok(r) => r,
            Err(std::sync::mpsc::TryRecvError::Empty) => return None,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Err(sa::unzip_failed("the extraction stopped")),
        };
        if let Some(i) = self.importing.as_mut() {
            i.extracting = None;
        }
        match result {
            Err(e) => self.import_finish(Err(e)),
            Ok(layout) => self.import_layout(layout),
        }
        None
    }

    /// Moves the replaced script aside again (never sleeping on the UI thread); past the
    /// deadline it is left as it was and the import ends.
    fn move_aside_again(&mut self, backup: PathBuf, until: std::time::Instant) -> Option<std::time::Instant> {
        let (dest, name) = self.importing.as_ref().map(|i| (i.dest.clone(), i.name.clone()))?;
        if std::fs::rename(&dest, &backup).is_ok() {
            if let Some(i) = self.importing.as_mut() {
                i.moving = None;
                i.backup = Some(backup);
            }
            self.import_install();
            return None;
        }
        let now = std::time::Instant::now();
        if now >= until {
            if let Some(i) = self.importing.as_mut() {
                i.moving = None;
            }
            self.import_finish(Err(sa::replace_failed(&name)));
            return None;
        }
        Some(now + MOVE_ASIDE_EVERY)
    }

    /// `didUnzipSuccessfullyTo:` (`IM:326-380`): a script of that name asks "Script Already
    /// Exists" (persistent: never silenced); else it is installed.
    fn import_layout(&mut self, layout: sa::Layout) {
        let name = match &layout {
            sa::Layout::Basic { file } => file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            sa::Layout::Full { name, .. } => name.clone(),
        };
        let quiet = self.importing.as_ref().and_then(|i| i.quiet);
        let folder = match quiet {
            Some(true) => crate::scripts::autolaunch_folder(&self.scripts_folder()),
            _ => self.scripts_folder(),
        };
        // An AutoLaunch folder reached through a link to a network path is never probed.
        if quiet == Some(true) && !crate::scripts::ancestors_stay_local(&folder, &crate::netpath::read_reparse) {
            return self.import_finish(Err(crate::scripts::autolaunch_not_local(&folder).trim_end().to_string()));
        }
        let _ = std::fs::create_dir_all(&folder);
        let dest = folder.join(&name);
        let Some(i) = self.importing.as_mut() else { return };
        i.layout = Some(layout);
        i.name = name.clone();
        i.dest = dest.clone();
        if dest.exists() && quiet.is_some() {
            return self.import_replace(&name);
        }
        if dest.exists() {
            let buttons = [sa::REPLACE, crate::api_auth::CANCEL];
            let ask = ask(sa::ALREADY_HEADING, &sa::already_installed(&name), &buttons, None, warnings::Kind::Persistent, Then::AlreadyInstalled(name));
            return self.ask_scripts(ask);
        }
        self.import_install();
    }

    /// Replace Script (`moveAsideScriptNamed:`, `IM:442-458`): a running copy is terminated,
    /// then the old script moved aside to `.replacing-<name>-<UUID>`; when it cannot be moved
    /// it is left as it was and the import ends. (What a full environment's script started
    /// and left running keeps its files; the move then fails the same way.)
    pub(super) fn import_replace(&mut self, name: &str) {
        let Some(dest) = self.importing.as_ref().map(|i| i.dest.clone()) else { return };
        let running = self.script_history.running_with_full_path(&dest).map(|e| e.id.clone());
        if let Some(id) = &running {
            self.terminate_script(id);
        }
        let backup = dest.with_file_name(sa::backup_name(name, &crate::api_auth::fresh_key()));
        if std::fs::rename(&dest, &backup).is_err() {
            // Right after a Terminate the files may still be held: `import_tick` tries again.
            if running.is_some() {
                if let Some(i) = self.importing.as_mut() {
                    i.moving = Some((backup, std::time::Instant::now() + MOVE_ASIDE_FOR));
                }
                return;
            }
            return self.import_finish(Err(sa::replace_failed(name)));
        }
        if let Some(i) = self.importing.as_mut() {
            i.backup = Some(backup);
        }
        self.import_install();
    }

    /// `installTrusted:` (`AR:176-196`): a basic script is moved into the Scripts folder; a
    /// full environment's `.venv` is built from its `setup.cfg` where it was extracted (asking
    /// before any download), then moved into place.
    fn import_install(&mut self) {
        let Some(i) = self.importing.as_ref() else { return };
        let dest = i.dest.clone();
        match i.layout.clone() {
            Some(sa::Layout::Basic { file }) => {
                let moved = std::fs::rename(&file, &dest).map(|_| dest).map_err(|e| Failure::Error(e.to_string()));
                self.import_install_done(moved);
            }
            Some(sa::Layout::Full { folder, .. }) => {
                if std::fs::read_to_string(folder.join(sa::SETUP_CFG)).ok().and_then(|t| rt::SetupCfg::parse(&t)).is_none() {
                    return self.import_install_done(Err(Failure::Error(sa::MISSING_SETUP_CFG.to_string())));
                }
                let (requested, mut dependencies) = container_inputs(&folder);
                // The iterm2 package always comes; it is not asked for again (`AR:256-257`).
                dependencies.retain(|d| d != "iterm2");
                self.ensure_environment(Target::Container(folder.clone()), &requested, &dependencies, true, After::Import(folder));
            }
            None => {}
        }
    }

    /// The imported full environment is built: it moves into place.
    pub(super) fn import_environment_ready(&mut self, folder: &Path) {
        let Some(dest) = self.importing.as_ref().filter(|i| matches!(&i.layout, Some(sa::Layout::Full { folder: f, .. }) if f == folder)).map(|i| i.dest.clone()) else { return };
        let moved = std::fs::rename(folder, &dest).map(|_| dest).map_err(|e| Failure::Error(e.to_string()));
        self.import_install_done(moved);
    }

    /// The install's end (`IM:381-415`): with a replaced script, a failure or a declined
    /// download puts it back and says so; success removes its backup. A declined fresh
    /// install ends quietly.
    pub(super) fn import_install_done(&mut self, result: Result<PathBuf, Failure>) {
        let Some(i) = self.importing.as_ref() else { return };
        let (name, dest, backup) = (i.name.clone(), i.dest.clone(), i.backup.clone());
        let outcome = match (result, backup) {
            (Ok(location), backup) => {
                if let Some(b) = backup {
                    remove_path(&b);
                }
                Ok(Some(location))
            }
            (Err(f), Some(b)) => {
                remove_path(&dest);
                let _ = std::fs::rename(&b, &dest);
                Err(match f {
                    Failure::Canceled => sa::replace_canceled(&name),
                    Failure::Error(e) if e.is_empty() => sa::COULD_NOT_INSTALL.to_string(),
                    Failure::Error(e) => e,
                })
            }
            (Err(Failure::Canceled), None) => Ok(None),
            (Err(Failure::Error(e)), None) => Err(e),
        };
        self.import_finish(outcome);
    }

    /// The import under way ends: its staging folder goes (off the UI thread), then the
    /// result shows.
    pub(super) fn import_finish(&mut self, result: Result<Option<PathBuf>, String>) {
        let Some(i) = self.importing.take() else { return };
        let staging = i.staging;
        std::thread::spawn(move || remove_path(&staging));
        self.import_result(i.source, i.trash, i.quiet, result);
    }

    /// `importDidFinishWithErrorMessage:` (`M:761-789`): the Scripts folder is scanned again;
    /// a failure asks "Could Not Install Script" (OK, Try Again), a success recycles a found
    /// archive and asks "Script Imported Successfully" (OK, Launch); a quiet end shows
    /// nothing. Then the next waiting import starts.
    fn import_result(&mut self, source: PathBuf, trash: bool, quiet: Option<bool>, result: Result<Option<PathBuf>, String>) {
        if let Some(scanner) = &self.scripts_scan {
            scanner.rescan();
        }
        let ok = crate::api_auth::OK;
        match result {
            Err(message) if quiet.is_some() => self.script_log_app(crate::settings_export::script_import_failed(&source, &message)),
            _ if quiet.is_some() => {}
            Ok(None) => {}
            Ok(Some(location)) => {
                if trash {
                    recycle(&source);
                }
                let ask = ask(sa::IMPORTED_TITLE, "", &[ok, sa::LAUNCH], None, warnings::Kind::Persistent, Then::ImportDone(location));
                self.ask_scripts(ask);
            }
            Err(message) => {
                let ask = ask(sa::COULD_NOT_INSTALL_TITLE, &message, &[ok, sa::TRY_AGAIN], None, warnings::Kind::Persistent, Then::ImportFailed(source));
                self.ask_scripts(ask);
            }
        }
        if self.importing.is_none() {
            match self.import_queue.pop_front() {
                Some((path, trash, quiet)) => self.import_start(path, trash, quiet),
                None => self.restored_scripts_done(),
            }
        }
    }

    /// `didFindScriptArchive:autolaunch:` (`M:547-607`): each archive the scan finds asks
    /// "Install Script Archive?" once per run (temporarily silenceable; only OK is remembered).
    pub(super) fn archives_found(&mut self, archives: &[(PathBuf, bool)]) {
        for (path, _) in archives {
            if !self.archives_asked.insert(path.clone()) {
                continue;
            }
            self.ask_scripts(archive_ask(path));
        }
    }

    /// At start, before the first scan and AutoLaunch: what an earlier run's import left is
    /// put right (iTerm2 runs `recoverStaleReplaceBackups` at launch, `IM:476-527`); one
    /// listing and a few renames.
    pub(super) fn recover_imports(&mut self) {
        recover_in(&self.scripts_folder(), &crate::netpath::read_reparse);
    }

    /// Scripts > Manage > Export (`chooseAndExportScript`, `M:683-714`): the chooser.
    pub(super) fn export_scripts(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(w) = &self.script_export {
            return w.focus();
        }
        let Some(g) = self.gpu.as_ref() else { return };
        match ExportWindow::open(event_loop, &g.instance, &g.adapter, &g.device, &g.queue, &self.scripts_tree) {
            Ok(w) => self.script_export = Some(w),
            Err(e) => self.banners.set("window", format!("Could not open the window: {e}")),
        }
    }

    /// Events for the chooser. False for other windows. Closing it is Cancel; OK exports each
    /// chosen script off the UI thread.
    pub(super) fn script_export_event(&mut self, window: WindowId, event: &WindowEvent) -> bool {
        let Some(w) = self.script_export.as_mut().filter(|w| w.id() == window) else { return false };
        let answer = match event {
            WindowEvent::CloseRequested => ExportEvent::Cancel,
            WindowEvent::RedrawRequested => w.redraw(),
            other => {
                w.event(other);
                ExportEvent::None
            }
        };
        match answer {
            ExportEvent::None => {}
            ExportEvent::Cancel => self.script_export = None,
            ExportEvent::Export { paths, autolaunch } => {
                self.script_export = None;
                let home = std::env::var_os("USERPROFILE").map(PathBuf::from).unwrap_or_default();
                let desktop = super::screenshot_ui::desktop().unwrap_or_else(|| home.join("Desktop"));
                let (tx, rx) = std::sync::mpsc::channel();
                let notify = self.notify.clone();
                std::thread::spawn(move || {
                    for p in paths {
                        let _ = tx.send(export_one(&p, autolaunch, &desktop));
                        notify();
                    }
                });
                self.script_exports.push(rx);
            }
        }
        true
    }

    /// Finished exports: a zip is revealed in Explorer; a failure asks "Export Failed".
    pub(super) fn script_export_tick(&mut self) {
        let mut results = Vec::new();
        self.script_exports.retain(|rx| loop {
            match rx.try_recv() {
                Ok(r) => results.push(r),
                Err(std::sync::mpsc::TryRecvError::Empty) => break true,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => break false,
            }
        });
        for r in results {
            match r {
                Ok(zip) => self.reveal_path(&zip),
                Err(message) => {
                    let text = if message.is_empty() { sa::FAILED_TO_CREATE.to_string() } else { message };
                    let ask = ask(sa::EXPORT_FAILED, &text, &[crate::api_auth::OK], None, warnings::Kind::Persistent, Then::Nothing);
                    self.ask_scripts(ask);
                }
            }
        }
    }
}

/// Appends `entries` to `sub`: folders as submenus, scripts as check items.
fn fill_scripts(sub: &muda::Submenu, entries: &[MenuEntry], errors: &mut Vec<String>) {
    for e in entries {
        let appended = match e {
            MenuEntry::Script { id, label, checked, .. } => sub.append(&muda::CheckMenuItem::with_id(id.clone(), label.replace('&', "&&"), true, *checked, None)),
            MenuEntry::Folder { label, children } => {
                let folder = muda::Submenu::new(label.replace('&', "&&"), true);
                fill_scripts(&folder, children, errors);
                sub.append(&folder)
            }
        };
        if let Err(e) = appended {
            errors.push(e.to_string());
        }
    }
}

/// "Cannot Run Script" for the malformed script `name` at `path`: OK or Reveal, temporarily
/// silenceable; Reveal is never remembered.
pub(super) fn malformed_ask(path: PathBuf, name: &str) -> super::scripts_alerts::Ask {
    let buttons = [crate::api_auth::OK, sl::REVEAL];
    ask(sl::MALFORMED_HEADING, &sl::malformed(name), &buttons, Some(sl::MALFORMED_ID), warnings::Kind::Temporary, Then::Malformed(path))
}

/// "Rebuild Python Environment?" for the lost environment `name` of the launch `req`:
/// Rebuild or Cancel; `REBUILD_ID` is persistent in iTerm2: never silenced.
pub(super) fn rebuild_ask(req: Request, name: &str) -> super::scripts_alerts::Ask {
    let buttons = [sl::REBUILD, crate::api_auth::CANCEL];
    ask(sl::REBUILD_HEADING, &sl::rebuild_text(name), &buttons, Some(sl::REBUILD_ID), warnings::Kind::Persistent, Then::Rebuild(req))
}

/// "Install Script Archive?" for the archive the scan found at `path`: OK, Cancel or Move to
/// Trash, temporarily silenceable; only OK is remembered.
pub(super) fn archive_ask(path: &Path) -> super::scripts_alerts::Ask {
    let file = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let buttons = [crate::api_auth::OK, crate::api_auth::CANCEL, crate::scripts::MOVE_TO_TRASH];
    ask(crate::scripts::ARCHIVE_HEADING, &crate::scripts::archive_text(&file), &buttons, Some(crate::scripts::ARCHIVE_ID), warnings::Kind::Temporary, Then::Archive(path.to_path_buf()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scripts::Node;

    fn temp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("tandem-{tag}-{}", crate::api_auth::fresh_key()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(dir).map(|rd| rd.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect()).unwrap_or_default();
        v.sort();
        v
    }

    #[test]
    fn import_refuses_unsafe_archives() {
        use crate::zip_check::tests::{file, valid, zip, Entry};
        let root = temp_dir("zipcheck");
        let run = |entries: &[Entry]| {
            let archive = root.join("a.zip");
            std::fs::write(&archive, zip(entries)).unwrap();
            let staging = root.join(format!(".installing-{}", crate::api_auth::fresh_key()));
            let r = extract_archive(&archive, &staging);
            (r, staging)
        };
        // A valid export extracts; its hidden entries at the top of the full folder go.
        let mut entries = valid();
        entries.push(file("tool/.venv/Scripts/python.exe", "x"));
        let (r, staging) = run(&entries[..4]);
        assert!(matches!(r, Ok(sa::Layout::Full { ref name, .. }) if name == "tool"), "{r:?}");
        let _ = std::fs::remove_dir_all(&staging);
        let (r, staging) = run(&entries[..5]);
        assert!(r.is_ok(), "{r:?}");
        assert_eq!(names(&staging.join("tool")), ["setup.cfg", "tool"]);
        let _ = std::fs::remove_dir_all(&staging);

        // Hostile names and links are refused before tar.exe runs: nothing is extracted and
        // nothing lands beside the staging folder.
        for bad in ["../x.py", "C:/x.py", "/x.py", "tool/tool/a:b", "tool/CON"] {
            let mut entries = valid();
            entries.push(file(bad, "evil"));
            let (r, staging) = run(&entries);
            assert!(r.as_ref().is_err_and(|e| e.contains("not safe to extract")), "{bad}: {r:?}");
            assert!(!staging.exists(), "{bad}");
            assert_eq!(names(&root), ["a.zip"], "{bad}");
        }
        let mut entries = valid();
        entries.push(Entry { external: (0o120_777) << 16, ..file("tool/.venv.building", "C:/Windows") });
        assert!(run(&entries).0.is_err());
        // Not a zip at all (a tar stream).
        std::fs::write(root.join("a.zip"), b"x.py\0\0\0ustar").unwrap();
        assert!(extract_archive(&root.join("a.zip"), &root.join("s")).is_err());
        assert!(!root.join("s").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn staging_walk_refuses_links() {
        let root = temp_dir("walk");
        let staging = root.join("s");
        std::fs::create_dir_all(staging.join(r"tool\tool")).unwrap();
        std::fs::write(staging.join(r"tool\setup.cfg"), "x").unwrap();
        std::fs::write(staging.join(r"tool\tool\tool.py"), "x").unwrap();
        assert_eq!(sa::unsafe_entry(&staging, &stage_entries), None);
        // A junction made in staging (no rights needed), pointing outside it.
        let outside = root.join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        let junction = staging.join(r"tool\.venv");
        let made = std::process::Command::new("cmd.exe").args(["/d", "/c", "mklink", "/J"]).arg(&junction).arg(&outside).output().unwrap();
        assert!(made.status.success(), "{made:?}");
        assert_eq!(sa::unsafe_entry(&staging, &stage_entries), Some(junction.clone()));
        std::fs::remove_dir(&junction).unwrap();
        assert!(outside.exists(), "removing the junction leaves its target");
        // A file with a second hard link.
        std::fs::hard_link(staging.join(r"tool\tool\tool.py"), outside.join("other.py")).unwrap();
        assert_eq!(sa::unsafe_entry(&staging, &stage_entries), Some(staging.join(r"tool\tool\tool.py")));
        let _ = std::fs::remove_dir_all(&root);
    }

    fn script(path: &str, full: bool) -> Node {
        let path = PathBuf::from(path);
        Node::Script { name: path.file_name().unwrap().to_string_lossy().into_owned(), path, full_environment: full }
    }

    fn folder(path: &str, children: Vec<Node>) -> Node {
        let path = PathBuf::from(path);
        Node::Folder { name: path.file_name().unwrap().to_string_lossy().into_owned(), path, children }
    }

    fn item(path: &str, checked: bool) -> MenuEntry {
        let path = PathBuf::from(path);
        MenuEntry::Script { id: format!("script:{}", path.display()), label: path.file_name().unwrap().to_string_lossy().into_owned(), path, checked }
    }

    #[test]
    fn menu_entries_follow_iterm2() {
        // Task 1's tree.
        let tree = vec![
            folder(r"C:\S\AutoLaunch", vec![script(r"C:\S\AutoLaunch\run.py", false)]),
            folder(r"C:\S\zeta", vec![script(r"C:\S\zeta\z.py", false)]),
            script(r"C:\S\A.py", false),
            script(r"C:\S\b.py", false),
            script(r"C:\S\Full", true),
        ];
        // b.py runs, and so does Full's main: a full environment's entry has the main .py as
        // its path, so its item (the container) is never checked (`M:180-185`, kept).
        let running = |p: &Path| p == Path::new(r"C:\S\b.py") || p == Path::new(r"C:\S\Full\Full\Full.py");
        let entries = menu_entries(&tree, &running);
        assert_eq!(
            entries,
            vec![
                MenuEntry::Folder { label: "AutoLaunch".into(), children: vec![item(r"C:\S\AutoLaunch\run.py", false)] },
                MenuEntry::Folder { label: "zeta".into(), children: vec![item(r"C:\S\zeta\z.py", false)] },
                item(r"C:\S\A.py", false),
                item(r"C:\S\b.py", true),
                item(r"C:\S\Full", false),
            ]
        );
        let MenuEntry::Script { id, .. } = &entries[2] else { panic!() };
        assert_eq!(id, r"script:C:\S\A.py");
        let MenuEntry::Script { id, label, .. } = &entries[4] else { panic!() };
        assert_eq!((id.as_str(), label.as_str()), (r"script:C:\S\Full", "Full"));
        assert_eq!(SCRIPT_ID, "script:");
    }

    #[test]
    fn picks_follow_iterm2() {
        assert_eq!(pick(false, false), Pick::Launch);
        assert_eq!(pick(true, false), Pick::Terminate);
        assert_eq!(pick(true, true), Pick::Reveal);
        assert_eq!(pick(false, true), Pick::Reveal);
    }

    #[test]
    fn autolaunch_follows_iterm2() {
        let listing: Vec<(String, bool)> = [(".DS_Store", false), ("__pycache__", true), ("run.py", false), ("notes.txt", false), ("Tool", true)].into_iter().map(|(n, d)| (n.to_string(), d)).collect();
        let folder = Path::new(r"C:\S\AutoLaunch");
        assert_eq!(autolaunch_targets(&listing, folder, &|_| true), vec![PathBuf::from(r"C:\S\AutoLaunch\Tool"), PathBuf::from(r"C:\S\AutoLaunch\notes.txt"), PathBuf::from(r"C:\S\AutoLaunch\run.py")]);
        assert!(should_autolaunch(false, true));
        assert!(!should_autolaunch(true, true));
        assert!(!should_autolaunch(false, false));
    }

    #[test]
    fn menu_identifiers_reach_scripts() {
        let tree = vec![script(r"C:\S\a.py", false)];
        let items = script_items(&tree, &["Scripts".to_string()]);
        let answer = |id: &str| script_menu_answer(id, &items).map(|(p, item)| (p.clone(), item.alternate));
        assert_eq!(answer(r"C:\S\a.py"), Some((PathBuf::from(r"C:\S\a.py"), false)));
        assert_eq!(answer(r"/Reveal/C:\S\a.py"), Some((PathBuf::from(r"C:\S\a.py"), true)));
        assert_eq!(answer("New Tab"), None);
        assert_eq!(answer(r"/Reveal/C:\S\b.py"), None);
    }

    #[test]
    fn script_items_follow_iterm2() {
        // `M:1559-1576`: each script item, then its Option alternate "Reveal %@" whose
        // identifier is "/Reveal/" and the path; folders nest in the menu path.
        let tree = vec![folder(r"C:\S\zeta", vec![script(r"C:\S\zeta\z.py", false)]), script(r"C:\S\Full", true)];
        let items = script_items(&tree, &["Scripts".to_string()]);
        let shown: Vec<(Vec<String>, &str, &str, bool)> = items.iter().map(|(_, i)| (i.path.clone(), i.identifier.as_str(), i.selector.as_str(), i.alternate)).collect();
        let v = |parts: &[&str]| parts.iter().map(|p| p.to_string()).collect::<Vec<String>>();
        assert_eq!(
            shown,
            vec![
                (v(&["Scripts", "zeta", "z.py"]), r"C:\S\zeta\z.py", "launchOrTerminateScript:", false),
                (v(&["Scripts", "zeta", "Reveal z.py"]), r"/Reveal/C:\S\zeta\z.py", "revealScript:", true),
                (v(&["Scripts", "Full"]), r"C:\S\Full", "launchOrTerminateScript:", false),
                (v(&["Scripts", "Reveal Full"]), r"/Reveal/C:\S\Full", "revealScript:", true),
            ]
        );
        assert_eq!(script_item_id(&items[1].1), r"script:/Reveal/C:\S\zeta\z.py");
        assert_eq!(reveal_label("a.py"), "Reveal a.py");
    }

    /// A real round trip through tar.exe (nothing is downloaded): a basic script's export is
    /// a full environment's zip without metadata, and importing it finds that layout; a full
    /// environment's export writes metadata into its folder but not into the zip.
    #[test]
    fn export_round_trips_through_tar() {
        let root = std::env::temp_dir().join(format!("tandem-archive-test-{}", crate::api_auth::fresh_key()));
        let (scripts, desktop) = (root.join("Scripts"), root.join("Desktop"));
        std::fs::create_dir_all(&scripts).unwrap();
        std::fs::create_dir_all(&desktop).unwrap();
        let body = b"#!/usr/bin/env python3\r\nprint('x')\n\xff\n";
        std::fs::write(scripts.join("tool.py"), body).unwrap();
        let zip = export_one(&scripts.join("tool.py"), true, &desktop).unwrap();
        assert_eq!(zip, desktop.join("tool.zip"));
        let again = export_one(&scripts.join("tool.py"), false, &desktop).unwrap();
        assert_eq!(again, desktop.join("tool (2).zip"));
        let out = root.join("out");
        let layout = extract_archive(&zip, &out).unwrap();
        assert_eq!(layout, sa::Layout::Full { folder: out.join("tool"), name: "tool".into(), autolaunch: false });
        assert_eq!(std::fs::read(out.join(r"tool\tool\tool.py")).unwrap(), body, "copied byte for byte");
        let cfg = rt::SetupCfg::parse(&std::fs::read_to_string(out.join(r"tool\setup.cfg")).unwrap()).unwrap();
        assert_eq!((cfg.name.as_str(), cfg.dependencies.clone(), cfg.python_version.clone()), ("tool", vec!["iterm2".to_string()], Some(rt::DEFAULT_PYTHON.to_string())));
        assert!(!out.join(r"tool\metadata.json").exists(), "an unsigned zip holds no metadata");

        let full = scripts.join("Full");
        std::fs::create_dir_all(full.join("Full")).unwrap();
        std::fs::create_dir_all(full.join(".venv")).unwrap();
        std::fs::write(full.join("setup.cfg"), rt::setup_cfg_text("Full", &[], true, "3.12", 0)).unwrap();
        std::fs::write(full.join(rt::MARKER), "{}").unwrap();
        std::fs::write(full.join(r"Full\Full.py"), "print(1)\n").unwrap();
        let zip = export_one(&full, true, &desktop).unwrap();
        assert_eq!(std::fs::read_to_string(full.join("metadata.json")).unwrap(), "{\"AutoLaunch\":true}");
        let out2 = root.join("out2");
        assert_eq!(extract_archive(&zip, &out2).unwrap(), sa::Layout::Full { folder: out2.join("Full"), name: "Full".into(), autolaunch: false });
        assert!(!out2.join(r"Full\.venv").exists() && !out2.join(r"Full\python-runtime.json").exists(), "only setup.cfg and the inner folder");

        // Not a script: a plain folder, and a missing path.
        std::fs::create_dir_all(scripts.join("plain")).unwrap();
        assert_eq!(export_one(&scripts.join("plain"), false, &desktop), Err(sa::NO_SCRIPT_FOUND.to_string()));
        // An archive that is not a zip.
        std::fs::write(root.join("bad.zip"), "not a zip").unwrap();
        assert!(extract_archive(&root.join("bad.zip"), &root.join("out3")).unwrap_err().starts_with("Could not unzip archive: "));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn recovery_follows_iterm2() {
        let root = std::env::temp_dir().join(format!("tandem-recover-test-{}", crate::api_auth::fresh_key()));
        std::fs::create_dir_all(root.join(".installing-X").join("tool")).unwrap();
        let uuid = crate::api_auth::fresh_key();
        // A backup whose script is back: removed. One whose script is gone: restored.
        std::fs::write(root.join("done.py"), "new").unwrap();
        std::fs::write(root.join(sa::backup_name("done.py", &uuid)), "old").unwrap();
        std::fs::create_dir_all(root.join(sa::backup_name("lost", &uuid))).unwrap();
        std::fs::write(root.join(sa::backup_name("lost", &uuid)).join("x"), "kept").unwrap();
        // A user's file that only looks like one is left alone.
        std::fs::write(root.join(".replacing-mine"), "mine").unwrap();
        // Reached through a link to a network path: nothing is listed, removed or renamed.
        let above = root.parent().unwrap().to_path_buf();
        recover_in(&root, &|p: &Path| (p == above).then(|| PathBuf::from(r"\\h\share")));
        assert!(root.join(".installing-X").exists() && root.join(sa::backup_name("lost", &uuid)).exists());
        recover_in(&root, &|_: &Path| None);
        let mut names: Vec<String> = std::fs::read_dir(&root).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        assert_eq!(names, [".replacing-mine", "done.py", "lost"]);
        assert_eq!(std::fs::read_to_string(root.join("done.py")).unwrap(), "new");
        assert_eq!(std::fs::read_to_string(root.join(r"lost\x")).unwrap(), "kept");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn autolaunch_skips_network_links() {
        use std::collections::HashMap;
        // A link straight to a share, one reaching a share on its second hop, a loop, a link
        // to a local folder, and a plain file.
        let links: HashMap<PathBuf, PathBuf> = [
            (r"C:\S\AutoLaunch\share", r"\\server\x"),
            (r"C:\S\AutoLaunch\hop", r"C:\S\AutoLaunch\hop2"),
            (r"C:\S\AutoLaunch\hop2", r"\\?\UNC\srv\y"),
            (r"C:\S\AutoLaunch\loop", r"C:\S\AutoLaunch\loop2"),
            (r"C:\S\AutoLaunch\loop2", r"C:\S\AutoLaunch\loop"),
            (r"C:\S\AutoLaunch\local", r"D:\tools"),
        ]
        .into_iter()
        .map(|(a, b)| (PathBuf::from(a), PathBuf::from(b)))
        .collect();
        let read_link = |p: &Path| links.get(p).cloned();
        let local = |p: &Path| crate::scripts::stays_local_with(p, &read_link, &crate::netpath::LocalDrives);
        let listing: Vec<(String, bool)> = [("share", false), ("hop", false), ("loop", false), ("local", true), ("run.py", false)].into_iter().map(|(n, d)| (n.to_string(), d)).collect();
        let folder = Path::new(r"C:\S\AutoLaunch");
        assert_eq!(autolaunch_targets(&listing, folder, &local), vec![PathBuf::from(r"C:\S\AutoLaunch\local"), PathBuf::from(r"C:\S\AutoLaunch\run.py")]);
        // A network path itself is refused without being touched.
        assert!(!crate::scripts::stays_local(Path::new(r"\\server\share\x.py")));
    }
}
