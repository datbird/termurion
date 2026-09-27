//! Launching scripts (`M:867-977`, `L:725-1029`): a path is classified, its environment made
//! ready (asking before any download), the API started when a person asked while it was
//! off, and the script run hidden with a fresh cookie and key. Its output goes to its Script
//! Console entry; Terminate and Restart end and relaunch it. The Scripts menu lists the
//! scripts after its fixed items (`M:1559-1577`), AutoLaunch starts its folder's scripts
//! once (`M:1587-1650`), and Open Quickly and MenuItemRequest reach the same items.

use std::io::{BufRead, Read};
use std::path::{Path, PathBuf};

use super::api_ui::{self, Alert};
use super::script_runtime_ui::After;
use super::scripts_alerts::{Ask, Then};
use super::App;
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
fn first_line(p: &Path) -> Option<String> {
    let file = std::fs::File::open(p).ok()?;
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
                self.ask_scripts(Ask { heading: sl::ERROR_RUNNING_TITLE.to_string(), text, buttons: vec![crate::api_auth::OK.to_string()], silence: None, then: Then::ErrorRunning });
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

    /// Restart (`SC:224-232`): Terminate, then the same launch again, as a person's.
    // Called by the Script Console (Task 8).
    #[allow(dead_code)]
    pub(super) fn relaunch_script(&mut self, id: &str) {
        let relaunch = self.script_history.find(id).and_then(|e| e.relaunch.clone());
        self.terminate_script(id);
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
        if !crate::scripts::ancestors_stay_local(&folder, &crate::scripts::read_link) {
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
        let ok = vec![crate::api_auth::OK.to_string()];
        if !sn::under_scripts(&plan.chosen, &scripts) {
            // Persistent in iTerm2 (`LOCATION_ID`): never silenced.
            let ask = Ask { heading: sn::INVALID_FOLDER.to_string(), text: sn::location_restricted(&scripts), buttons: ok, silence: None, then: Then::InvalidFolder(plan) };
            return self.ask_scripts(ask);
        }
        let name = plan.chosen.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if sn::spaceless(&name).is_some() {
            let buttons = vec![sn::USE_UNDERSCORE.to_string(), sn::CHANGE_NAME.to_string()];
            let ask = Ask { heading: sn::SPACES_HEADING.to_string(), text: sn::SPACES_TEXT.to_string(), buttons, silence: None, then: Then::Spaces(plan) };
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
                sn::full_environment_files(&sn::container(&plan.chosen, &scripts), &name, &plan.dependencies, &plan.python, &text)
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
                let buttons = vec![crate::api_auth::OK.to_string(), sn::SHOW_IN_EXPLORER.to_string()];
                let ask = Ask { heading: sn::WARNING.to_string(), text: sn::open_in(&app), buttons, silence: Some(sn::OPEN_IN_ID), then: Then::OpenNewScript(dest) };
                self.ask_scripts(ask);
            }
            None => self.reveal_path(&dest),
        }
    }

    /// Opens a new script in its editor with the "edit" verb, never "open" (which runs a
    /// `.py`); when that fails the script is revealed.
    pub(super) fn edit_new_script(&self, path: &Path) {
        use windows_sys::Win32::UI::Shell::ShellExecuteW;
        use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
        let wide = |s: &str| s.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
        let (verb, file) = (wide("edit"), wide(&path.to_string_lossy()));
        let r = unsafe { ShellExecuteW(std::ptr::null_mut(), verb.as_ptr(), file.as_ptr(), std::ptr::null(), std::ptr::null(), SW_SHOWNORMAL) };
        // Values above 32 mean success.
        if (r as isize) <= 32 {
            self.reveal_path(path);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scripts::Node;

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
        let local = |p: &Path| crate::scripts::link_chain_end(p, &read_link).is_some();
        let listing: Vec<(String, bool)> = [("share", false), ("hop", false), ("loop", false), ("local", true), ("run.py", false)].into_iter().map(|(n, d)| (n.to_string(), d)).collect();
        let folder = Path::new(r"C:\S\AutoLaunch");
        assert_eq!(autolaunch_targets(&listing, folder, &local), vec![PathBuf::from(r"C:\S\AutoLaunch\local"), PathBuf::from(r"C:\S\AutoLaunch\run.py")]);
        // A network path itself is refused without being touched.
        assert!(!crate::scripts::stays_local(Path::new(r"\\server\share\x.py")));
    }
}
