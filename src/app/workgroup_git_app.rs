//! Git items and Diff mode on the app side (milestone 13y1, Task 6): one poller per instance, the
//! changed-file lists, the Diff members' deferred launches and the waiting overlay, the base field and
//! navigation. The pure side is `workgroup_git`, the git runs are `git_state`'s.
//!
//! Every Diff launch, per-file pick, All Files, Reload and base change is a RESTART of the member with
//! an argument vector made from the person's own template (B4): nothing is ever typed into a pane, a
//! value is exactly one argument, and `git` runs through `guarded_args` after `vet_repo` passed for
//! the leader's folder. Citations: `WI` `iTermWorkgroupInstance.swift`, `WPP`
//! `iTermWorkgroupPeerPort.swift`, `DWV` `DiffWaitingPromptView.swift` under
//! `~/gitrepos/iterm2-ref/sources/Workgroups`.

use super::statusbar_content::is_this_machine;
use super::*;
use crate::git_state::{self as gs, Fetched, GitPoller, PollGate};
use crate::workgroup::{Mode, ToolbarItem};
use crate::text_edit::EditKey;
use crate::workgroup_git::{self as wgit, Selector};
use std::collections::BTreeMap;

/// A Diff member's deferred launch (`pendingDiffLaunch`): `queued` is the reload variant, whose
/// overlay has a Cancel because the previous output is still behind it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Pending {
    pub queued: bool,
}

/// The git side of one instance.
pub(super) struct GitSide {
    pub poller: GitPoller,
    /// The changed-file selector of each member that has the item, by config id.
    pub selectors: BTreeMap<String, Selector>,
    /// The Diff members waiting to launch, by config id.
    pub pending: BTreeMap<String, Pending>,
    /// An answer for the current directory and base has come in since the base last changed.
    pub fresh: bool,
}

impl GitSide {
    /// The git side a workgroup needs: any Diff member, or any item that needs the poller. None when
    /// nothing does (`WI:930-960`).
    pub fn for_workgroup(w: &crate::workgroup::Workgroup) -> Option<GitSide> {
        let needs = w.sessions.iter().any(|s| s.mode == Mode::Diff || s.toolbar_items.iter().any(ToolbarItem::needs_git_poller));
        needs.then(|| {
            let mut poller = GitPoller::new(wgit::CADENCE.as_secs_f64(), Instant::now());
            poller.set_enabled(true);
            let selectors = w.sessions.iter().filter(|s| s.toolbar_items.contains(&ToolbarItem::ChangedFileSelector)).map(|s| (s.unique_identifier.clone(), Selector::default())).collect();
            GitSide { poller, selectors, pending: BTreeMap::new(), fresh: false }
        })
    }

    /// The member starts waiting for changes (State A).
    pub fn wait_for_changes(&mut self, config: &str) {
        self.pending.insert(config.to_string(), Pending { queued: false });
    }

    /// The statuses the poller last reported.
    pub fn statuses(&self) -> &[gs::FileStatus] {
        self.poller.state.as_ref().and_then(|s| s.file_statuses.as_deref()).unwrap_or(&[])
    }

    /// `diffLaunchReady`: git reports a change `git difftool` can show.
    pub fn ready(&self) -> bool {
        self.fresh && wgit::diffable(self.statuses())
    }
}

/// The open pulldown: the changed files of a member, or the recent bases of an instance.
pub(super) struct OpenMenu {
    pub instance: String,
    pub owner: String,
    pub rows: Vec<Option<String>>,
    pub recents: bool,
}

/// The base field with the keyboard.
pub(super) struct BaseEdit {
    pub instance: String,
    pub pane: PaneId,
    pub edit: crate::text_edit::TextEdit,
}

/// The navigation item's three buttons.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Nav {
    Back,
    Forward,
    Reload,
}

/// The id prefixes of the two pulldowns.
const FILE_MENU: &str = "workgroup-file:";
const RECENT_MENU: &str = "workgroup-base:";

impl App {
    /// The instance id and config id of the member `pane` is.
    pub(super) fn member_config(&self, pane: PaneId) -> Option<(String, String)> {
        let i = self.workgroup_of(pane)?;
        let config = i.config_of(pane).map(str::to_string).or_else(|| i.non_peers.iter().find(|(_, p)| *p == pane).map(|(c, _)| c.clone()))?;
        Some((i.id.clone(), config))
    }

    /// The member's pane is on screen: in the front tab of its window (`isVisibleForDeferredDiff`).
    pub(super) fn member_visible(&self, pane: PaneId) -> bool {
        self.tabs.get(self.active).is_some_and(|t| t.layout.panes().contains(&pane)) || self.parked_front_tabs_all().iter().any(|t| t.layout.panes().contains(&pane))
    }

    /// An idle pane for a Diff member: no program, no process, restarted into its command.
    pub(super) fn start_idle_pane(&mut self, rect: Rect, guid: Option<&str>) -> Option<PaneId> {
        let p = self.profile(guid);
        let cell = self.profile_cell(&p);
        let id = self.next_id;
        let (cols, rows) = self.grid_size_in(rect, cell, self.pane_room_for(id, &p));
        self.install_pane(id, Pane::idle(id, cols, rows), &p, false);
        self.next_id += 1;
        Some(id)
    }

    /// The git that a Diff launch runs: the configured one, found without a PATH search of the
    /// current folder, never a link to a network path.
    fn git_for_launch(&self) -> Option<std::path::PathBuf> {
        let named = gs::git_program_named(&self.git_search_path())?;
        if named == std::path::Path::new("git") {
            gs::find_git(&std::env::var("PATH").unwrap_or_default(), &crate::localfs::is_file)
        } else {
            gs::program_links_local(&named).then_some(named)
        }
    }

    /// The directory the poller watches: the leader's current folder (Windows form), local only.
    fn workgroup_dir(&self, leader: PaneId) -> Option<String> {
        let shell = &self.panes.get(&leader)?.own().shell;
        is_this_machine(shell.host.as_deref(), shell.machine_id.as_deref()).then(|| self.git_directory(leader)).flatten()
    }

    /// The base the poller is asked for: None is HEAD.
    fn base_of(inst: &super::workgroup_app::Instance) -> Option<String> {
        (inst.git_base != wgit::DEFAULT_BASE && wgit::base_ok(&inst.git_base)).then(|| inst.git_base.clone())
    }

    /// The poller of each instance (`WI:942`): the leader's folder, every 2 seconds, with the files and
    /// the stats, against the instance's base. Deferred Diff launches fire from here. When to come back.
    pub(super) fn workgroup_git_tick(&mut self) -> Option<Instant> {
        if self.workgroups.is_empty() {
            return None;
        }
        let now = Instant::now();
        let (search, timeout) = (self.git_search_path(), self.git_timeout());
        let gate = PollGate { visible: true, last_newline: Some(now), last_activity: None };
        let ids: Vec<String> = self.workgroups.values().filter(|i| i.git.is_some()).map(|i| i.id.clone()).collect();
        let mut due: Option<Instant> = None;
        for id in ids {
            let Some(inst) = self.workgroups.get(&id) else { continue };
            let dir = self.workgroup_dir(inst.leader);
            let base = Self::base_of(inst);
            let Some(git) = self.workgroups.get_mut(&id).and_then(|i| i.git.as_mut()) else { continue };
            let first = git.poller.last_poll.is_none();
            let mut polls = vec![git.poller.set_directory(dir, &gate, now)];
            if first {
                polls.push(git.poller.bump(&gate, now));
            }
            polls.push(git.poller.tick(&gate, now));
            due = [due, git.poller.next_due()].into_iter().flatten().min();
            for p in polls {
                if let Some(path) = p.invalidate {
                    self.git_worker.invalidate(&path);
                }
                if let Some(path) = p.request {
                    self.git_worker.request_workgroup(path, base.clone(), search.clone(), timeout);
                }
            }
            self.fire_deferred(&id);
        }
        due
    }

    /// A worker answer: the instance whose directory and base made `key` takes it, its selectors are
    /// refreshed and a launch that waited for it may fire. True when something changed.
    pub(super) fn workgroup_git_answer(&mut self, key: &str, path: &str, fetched: Fetched) -> bool {
        let mut changed = false;
        let ids: Vec<String> = self.workgroups.values().filter(|i| i.git.as_ref().is_some_and(|g| g.poller.directory.as_deref() == Some(path)) && gs::cache_key(path, Self::base_of(i).as_deref(), true) == key).map(|i| i.id.clone()).collect();
        for id in ids {
            let Some(git) = self.workgroups.get_mut(&id).and_then(|i| i.git.as_mut()) else { continue };
            let before = (git.poller.state.clone(), git.fresh);
            if git.poller.did_poll(path, fetched.clone()) {
                git.fresh = true;
                let list: Vec<gs::FileStatus> = git.statuses().to_vec();
                for s in git.selectors.values_mut() {
                    s.set(&list);
                }
            }
            changed |= before != (git.poller.state.clone(), git.fresh);
            self.fire_deferred(&id);
        }
        changed
    }

    /// Fires every Diff member's deferred launch that may (`fireDeferredDiffLaunches`): git reports a
    /// change and the member is on screen (`diff_should_launch`).
    pub(super) fn fire_deferred(&mut self, instance: &str) {
        let Some(inst) = self.workgroups.get(instance) else { return };
        let Some(git) = inst.git.as_ref() else { return };
        let ready = git.ready();
        let due: Vec<String> = git.pending.keys().filter(|c| inst.pane_of(c).or_else(|| inst.non_peers.iter().find(|(k, _)| k == *c).map(|(_, p)| *p)).is_some_and(|p| wgit::diff_should_launch(self.member_visible(p), ready))).cloned().collect();
        for config in due {
            self.launch_diff(instance, &config);
        }
    }

    /// The pane of member `config`.
    fn member_pane(&self, instance: &str, config: &str) -> Option<PaneId> {
        let i = self.workgroups.get(instance)?;
        i.pane_of(config).or_else(|| i.non_peers.iter().find(|(k, _)| k == config).map(|(_, p)| *p))
    }

    /// Runs `f` with the window that shows the instance current (a hidden peer's group is where its
    /// shown member is), then puts the current window back.
    fn in_instance_window(&mut self, instance: &str, f: impl FnOnce(&mut App)) {
        let Some(at) = self.workgroups.get(instance).map(|i| i.shown_pane().unwrap_or(i.leader)) else { return };
        let home = self.current_window();
        if !self.enter_pane_window(at) {
            return;
        }
        f(self);
        if let Some(h) = home.filter(|h| self.current_window() != Some(*h)) {
            self.enter(h);
        }
    }

    /// Starts member `config`'s command now: its pending launch is spent and the member restarts.
    pub(super) fn launch_diff(&mut self, instance: &str, config: &str) {
        let Some(pane) = self.member_pane(instance, config) else { return };
        if let Some(git) = self.workgroups.get_mut(instance).and_then(|i| i.git.as_mut()) {
            git.pending.remove(config);
        }
        self.in_instance_window(instance, |a| a.restart_launching(pane));
        self.redraw();
    }

    /// The command line of a member that is a Diff member or has a changed-file selector (`WPP:812-880`):
    /// the picked file's command when a file is picked and the member has one, else the member's own,
    /// as arguments (B4). `git` runs through `guarded_args` after `vet_repo` passed for the leader's
    /// folder, in the working tree's top (porcelain paths are relative to it); another program runs as
    /// written with the same values as arguments. An error is the banner text and nothing starts.
    pub(super) fn diff_launch_line(&mut self, instance: &str, config: &str) -> Result<(Option<String>, Vec<String>), String> {
        let Some(inst) = self.workgroups.get(instance) else { return Err("That workgroup has ended.".into()) };
        let Some(cfg) = inst.workgroup.session(config) else { return Err("That member no longer exists.".into()) };
        let picked = inst.git.as_ref().and_then(|g| g.selectors.get(config)).and_then(|s| s.picked()).map(str::to_string);
        let file_command = !cfg.per_file_command.trim().is_empty();
        let (template, file) = match picked.filter(|_| file_command) {
            Some(f) => (cfg.per_file_command.clone(), Some(f)),
            None => (cfg.command.clone(), None),
        };
        if !inst.approved.allows(&template) {
            return Err("That command was not in the list the workgroup was entered with.".into());
        }
        let base = inst.git_base.clone();
        let (leader, pane) = (inst.leader, self.member_pane(instance, config));
        let tokens = match &file {
            Some(f) => wgit::per_file_argv(&template, f, &base),
            None => wgit::all_files_argv(&template, &base),
        }
        .map_err(|r| r.text().to_string())?;
        let dir = self.workgroup_dir(leader).ok_or("This session is not in a folder on this computer, so git will not run.")?;
        let vet = gs::vet_repo(std::path::Path::new(&dir)).ok_or("git will not run in this folder (it is not a local repository, or its configuration is not safe to read).")?;
        // Whatever program runs, git runs under it: the repository's own config is judged first.
        if let Some(why) = vet.launch_refusal() {
            return Err(why);
        }
        let program = tokens[0].clone();
        let stem = std::path::Path::new(&program).file_stem().map(|s| s.to_string_lossy().to_ascii_lowercase());
        let (line, env) = if stem.as_deref() == Some("git") {
            let git = self.git_for_launch().ok_or("git was not found, so the diff did not start.")?;
            let rest: Vec<&str> = tokens[1..].iter().map(String::as_str).collect();
            // The one builder: the repository's own config is judged first, then the overrides and the environment.
            gs::launch(&git, &vet, &rest)?
        } else {
            let found = crate::semantic::resolve_on_path(std::path::Path::new(&program), std::env::var_os("PATH").as_deref()).ok_or("The diff program was not found, so it did not start.")?;
            (crate::workgroup_run::argv_line(&found, &tokens[1..])?, Vec::new())
        };
        // The porcelain paths are relative to the working tree's top: that is where it runs.
        if let (Some(root), Some(p)) = (vet.root, pane.and_then(|p| self.panes.get_mut(&p))) {
            p.start_dir = Some(root);
        }
        Ok((Some(line), env))
    }

    /// A pick in a changed-file selector (`diffDidSelect`, `diffDidSelectAllFiles`): the member restarts
    /// with the file's command or its own. A Diff member that has not run yet only remembers the pick.
    pub(super) fn diff_selected(&mut self, instance: &str, owner: &str, file: Option<&str>) {
        let Some(git) = self.workgroups.get_mut(instance).and_then(|i| i.git.as_mut()) else { return };
        if let Some(s) = git.selectors.get_mut(owner) {
            s.pick(file);
        }
        if git.pending.contains_key(owner) {
            self.redraw();
            return;
        }
        if let Some(pane) = self.member_pane(instance, owner) {
            self.in_instance_window(instance, |a| a.restart_launching(pane));
        }
        self.redraw();
    }

    /// Back, Forward and Reload of a navigation item (`WPP:742-790`).
    pub(super) fn navigation(&mut self, instance: &str, owner: &str, action: Nav) {
        match action {
            Nav::Back | Nav::Forward => {
                let picked = self.workgroups.get_mut(instance).and_then(|i| i.git.as_mut()).and_then(|g| g.selectors.get_mut(owner)).and_then(|s| s.advance(action == Nav::Forward));
                if let Some(file) = picked {
                    self.diff_selected(instance, owner, Some(&file));
                }
            }
            Nav::Reload => {
                if let Some(pane) = self.member_pane(instance, owner) {
                    self.workgroup_reload(pane);
                }
            }
        }
    }

    /// Reload of a Diff member (`reloadDiffWithDeferralIfNeeded`): a waiting launch fires when git has
    /// changes; a running one restarts now with changes, or queues (with the Cancel overlay) without.
    pub(super) fn diff_reload(&mut self, pane: PaneId) {
        let Some((instance, config)) = self.member_config(pane) else { return };
        let Some(git) = self.workgroups.get(&instance).and_then(|i| i.git.as_ref()) else { return };
        let (ready, waiting) = (git.ready(), git.pending.contains_key(&config));
        if waiting {
            if ready {
                self.launch_diff(&instance, &config);
            }
            return;
        }
        if ready {
            self.launch_diff(&instance, &config);
        } else if let Some(git) = self.workgroups.get_mut(&instance).and_then(|i| i.git.as_mut()) {
            git.pending.insert(config, Pending { queued: true });
            self.redraw();
        }
    }

    /// A committed base (`gitBaseChanged`, `WI:1562-1660`): the instance's base, the leader's `gitBase`
    /// variable and the poller follow, and every member with a changed-file selector runs against it.
    pub(super) fn git_base_committed(&mut self, instance: &str, base: &str) {
        let Ok(base) = wgit::typed_base(base) else {
            self.banners.set("workgroup", "That is not a usable base: it may not start with a dash, hold a control character or be over 256 characters.");
            return;
        };
        let Some(inst) = self.workgroups.get_mut(instance) else { return };
        if inst.git_base == base {
            return;
        }
        inst.git_base = base.clone();
        let leader = inst.leader;
        let _ = self.api_vars.set_user(crate::api_vars::Scope::Session, &leader.to_string(), "user.gitBase", serde_json::json!(base));
        // The recents are the person's own list in the stored globals.
        let saved: Vec<String> = self.globals.get(wgit::RECENTS_KEY).and_then(|v| v.as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())).unwrap_or_default();
        let list = wgit::recents(&saved, &base, wgit::MAX_RECENTS);
        self.globals.set(wgit::RECENTS_KEY, toml::Value::Array(list.into_iter().map(toml::Value::String).collect()));
        let now = Instant::now();
        let gate = PollGate { visible: true, last_newline: Some(now), last_activity: None };
        let (search, timeout) = (self.git_search_path(), self.git_timeout());
        let request_base = self.workgroups.get(instance).and_then(|i| Self::base_of(i));
        let owners: Vec<(String, bool)> = {
            let Some(inst) = self.workgroups.get_mut(instance) else { return };
            let Some(git) = inst.git.as_mut() else { return };
            git.fresh = false;
            let polls = git.poller.bump(&gate, now);
            if let Some(path) = polls.request {
                self.git_worker.request_workgroup(path, request_base.clone(), search.clone(), timeout);
            }
            let Some(inst) = self.workgroups.get(instance) else { return };
            inst.git.as_ref().map(|g| g.selectors.keys().map(|k| (k.clone(), inst.workgroup.session(k).is_some_and(|c| c.mode == Mode::Diff))).collect()).unwrap_or_default()
        };
        for (owner, diff) in owners {
            let waiting = self.workgroups.get(instance).and_then(|i| i.git.as_ref()).is_some_and(|g| g.pending.contains_key(&owner));
            if diff && waiting {
                // State A re-resolves the base when it fires.
                continue;
            }
            if diff {
                // Restart against the new base once the new list is in (`installDeferredDiffRestart`).
                if let Some(git) = self.workgroups.get_mut(instance).and_then(|i| i.git.as_mut()) {
                    git.pending.insert(owner, Pending { queued: true });
                }
            } else if let Some(pane) = self.member_pane(instance, &owner) {
                self.in_instance_window(instance, |a| a.restart_launching(pane));
            }
        }
        self.menus_dirty = true;
        self.redraw();
    }

    /// The waiting overlay a shown member wears: Some(queued) while its launch waits.
    pub(super) fn diff_overlay_for(&self, pane: PaneId) -> Option<bool> {
        let (instance, config) = self.member_config(pane)?;
        self.workgroups.get(&instance)?.git.as_ref()?.pending.get(&config).map(|p| p.queued)
    }

    /// Why Run Anyway cannot run: the folder fails the same vetting the poller needs.
    pub(super) fn run_anyway_refusal(&self, pane: PaneId) -> Option<&'static str> {
        let (instance, _) = self.member_config(pane)?;
        let leader = self.workgroups.get(&instance)?.leader;
        let dir = self.workgroup_dir(leader);
        dir.is_none_or(|d| gs::vet_repo(std::path::Path::new(&d)).is_none()).then_some("git will not run in this folder, so the command cannot start.")
    }

    /// Run Anyway: the deferred launch starts at once, after the same vetting.
    pub(super) fn diff_run_anyway(&mut self, pane: PaneId) {
        if let Some(why) = self.run_anyway_refusal(pane) {
            self.banners.set("workgroup", why);
            return;
        }
        if let Some((instance, config)) = self.member_config(pane) {
            self.launch_diff(&instance, &config);
        }
    }

    /// Cancel on the queued-reload overlay: the reload is abandoned and the old output stays.
    pub(super) fn diff_cancel_reload(&mut self, pane: PaneId) {
        if let Some((instance, config)) = self.member_config(pane) {
            if let Some(git) = self.workgroups.get_mut(&instance).and_then(|i| i.git.as_mut()) {
                if git.pending.get(&config).is_some_and(|p| p.queued) {
                    git.pending.remove(&config);
                }
            }
        }
        self.redraw();
    }

    // --- The pulldowns ---

    /// The changed-file selector's rows as a native menu at `at` (`STV:421-470`).
    pub(super) fn open_file_menu(&mut self, pane: PaneId, at: super::statusbar_content::MenuAt) {
        use muda::{Menu, MenuItem, PredefinedMenuItem};
        let Some((instance, owner)) = self.member_config(pane) else { return };
        let Some(sel) = self.workgroups.get(&instance).and_then(|i| i.git.as_ref()).and_then(|g| g.selectors.get(&owner)) else { return };
        let menu = Menu::new();
        let mut rows: Vec<Option<String>> = Vec::new();
        let picked = sel.picked().map(str::to_string);
        for row in sel.rows() {
            let n = rows.len();
            match row {
                wgit::Row::AllFiles(title) => {
                    let _ = menu.append(&MenuItem::with_id(format!("{FILE_MENU}{n}"), title.replace('&', "&&"), sel.count() > 0, None));
                    rows.push(None);
                }
                wgit::Row::Header(title) => {
                    let _ = menu.append(&PredefinedMenuItem::separator());
                    let _ = menu.append(&MenuItem::with_id(format!("{FILE_MENU}h{n}"), title, false, None));
                    rows.push(None);
                }
                wgit::Row::File { letter, text, path } => {
                    let mark = if picked.as_deref() == Some(path.as_str()) { "\u{2713} " } else { "" };
                    let _ = menu.append(&MenuItem::with_id(format!("{FILE_MENU}{n}"), format!("{mark}{letter}  {}", text.replace('&', "&&")), true, None));
                    rows.push(Some(path));
                }
            }
        }
        self.workgroups.menu = Some(OpenMenu { instance, owner, rows, recents: false });
        self.show_menu_at(menu, at);
    }

    /// The recent bases (HEAD first) as a native menu (`GBS:reloadDropdownItems`).
    pub(super) fn open_recents_menu(&mut self, pane: PaneId, at: super::statusbar_content::MenuAt) {
        use muda::{Menu, MenuItem};
        let Some((instance, owner)) = self.member_config(pane) else { return };
        let saved: Vec<String> = self.globals.get(wgit::RECENTS_KEY).and_then(|v| v.as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())).unwrap_or_default();
        let mut list = vec![wgit::DEFAULT_BASE.to_string()];
        list.extend(wgit::recents(&saved, "", wgit::MAX_RECENTS).into_iter().filter(|b| b != wgit::DEFAULT_BASE));
        let menu = Menu::new();
        for (n, b) in list.iter().enumerate() {
            let _ = menu.append(&MenuItem::with_id(format!("{RECENT_MENU}{n}"), crate::inert_files::display_text(b, 64, false).replace('&', "&&"), true, None));
        }
        self.workgroups.menu = Some(OpenMenu { instance, owner, rows: list.into_iter().map(Some).collect(), recents: true });
        self.show_menu_at(menu, at);
    }

    /// A pick from either pulldown. False for other ids.
    pub(super) fn workgroup_file_picked(&mut self, id: &str) -> bool {
        let (rest, recents) = match (id.strip_prefix(FILE_MENU), id.strip_prefix(RECENT_MENU)) {
            (Some(r), _) => (r, false),
            (_, Some(r)) => (r, true),
            _ => return false,
        };
        let Some(menu) = self.workgroups.menu.take().filter(|m| m.recents == recents) else { return true };
        let Some(n) = rest.parse::<usize>().ok() else { return true };
        let Some(row) = menu.rows.get(n).cloned() else { return true };
        if recents {
            if let Some(b) = row {
                self.git_base_committed(&menu.instance, &b);
            }
        } else {
            self.diff_selected(&menu.instance, &menu.owner, row.as_deref());
        }
        true
    }

    // --- The base field ---

    /// The base field takes the keyboard with its text selected.
    pub(super) fn base_field_focus(&mut self, pane: PaneId) {
        let Some((instance, _)) = self.member_config(pane) else { return };
        let Some(base) = self.workgroups.get(&instance).map(|i| i.git_base.clone()) else { return };
        if let Some(t) = self.tabs.get_mut(self.active) {
            t.focused = pane;
        }
        let mut edit = crate::text_edit::TextEdit::default();
        edit.load(&base);
        edit.anchor = Some(0);
        self.workgroups.base_edit = Some(BaseEdit { instance, pane, edit });
        self.redraw();
    }

    /// The field lets go; its text commits (`controlTextDidEndEditing`) unless `cancel`.
    pub(super) fn base_field_blur(&mut self, cancel: bool) {
        let Some(e) = self.workgroups.base_edit.take() else { return };
        if !cancel {
            self.git_base_committed(&e.instance, &e.edit.text.clone());
        }
        self.redraw();
    }

    /// The field's keys while it has the keyboard, releases too. True when taken.
    pub(super) fn base_field_key(&mut self, ev: &KeyEvent) -> bool {
        use winit::keyboard::{Key as WKey, NamedKey};
        let Some(e) = self.workgroups.base_edit.as_ref() else { return false };
        if self.focused_id() != Some(e.pane) {
            return false;
        }
        if ev.state != ElementState::Pressed || super::toolbelt_tools_ui::is_modifier(&ev.logical_key) {
            return true;
        }
        let mods = self.mods;
        let plain = !(mods.ctrl || mods.alt || mods.cmd);
        match &ev.logical_key {
            WKey::Named(NamedKey::Escape) if plain => {
                self.base_field_blur(true);
                return true;
            }
            WKey::Named(NamedKey::Enter | NamedKey::Tab) if plain => {
                self.base_field_blur(false);
                return true;
            }
            _ => {}
        }
        let Some((key, ctrl, alt)) = super::toolbelt_more_ui::edit_key(&ev.logical_key, ev.text.as_deref(), mods) else { return plain };
        if matches!(key, EditKey::Enter | EditKey::Up | EditKey::Down | EditKey::PageUp | EditKey::PageDown) || !crate::text_edit::takes(ctrl, alt, mods.cmd, &key) {
            return false;
        }
        let one = |t: &str| crate::inert_files::display_text(t, wgit::MAX_BASE, false);
        let key = match self.with_clipboard_text(key) {
            EditKey::Paste(t) => EditKey::Paste(one(&t)),
            EditKey::Text(t) => EditKey::Text(one(&t)),
            k => k,
        };
        let cw = self.cell().0;
        let shift = mods.shift;
        let measure = move |t: &str| unicode_width::UnicodeWidthStr::width(t) as f32 * cw;
        if let Some(e) = self.workgroups.base_edit.as_mut() {
            let lines = crate::text_edit::wrap(&e.edit.text, 1.0e9, &measure);
            if let crate::text_edit::EditOut::Copied(t) = e.edit.key(key, shift, &lines, &measure) {
                self.set_clipboard_text(t);
            }
        }
        self.redraw();
        true
    }
}

// --- The waiting overlay (`DWV`, `DWO`) ---

/// Where the overlay's parts sit in its panel (`DWV:layout`): the title at the top, the body under it,
/// the buttons at the bottom right.
struct OverlayGeom {
    title: Rect,
    body: Rect,
    run: Rect,
    cancel: Option<Rect>,
}

fn overlay_geom(panel: Rect, cell: (f32, f32), s: f32, queued: bool) -> OverlayGeom {
    let (inset, gap, button_h) = (16.0 * s, 12.0 * s, 28.0 * s);
    let w = |t: &str, min: f32| (unicode_width::UnicodeWidthStr::width(t) as f32 * cell.0 + 24.0 * s).max(min * s);
    let run_w = w(wgit::RUN_ANYWAY, 100.0);
    let by = panel.y + panel.h - inset - button_h;
    let run = Rect { x: panel.x + panel.w - inset - run_w, y: by, w: run_w, h: button_h };
    let cancel = queued.then(|| {
        let cw = w(crate::api_auth::CANCEL, 80.0);
        Rect { x: run.x - gap - cw, y: by, w: cw, h: button_h }
    });
    let title = Rect { x: panel.x + inset, y: panel.y + inset, w: (panel.w - 2.0 * inset).max(0.0), h: cell.1 * 1.4 };
    let top = title.y + title.h + 8.0 * s;
    let body = Rect { x: title.x, y: top, w: title.w, h: (by - gap - top).max(0.0) };
    OverlayGeom { title, body, run, cancel }
}

/// `text` broken into lines of at most `width` pixels, at blanks.
fn wrap_words(text: &str, width: f32, cw: f32) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        let w = |t: &str| unicode_width::UnicodeWidthStr::width(t) as f32 * cw;
        if !line.is_empty() && w(&line) + cw + w(word) > width {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

impl App {
    /// The shown members of the front tab that wear the waiting overlay: (pane, terminal rectangle,
    /// queued-reload variant).
    fn overlay_panels(&self) -> Vec<(PaneId, Rect, bool)> {
        let Some(tab) = self.tabs.get(self.active) else { return Vec::new() };
        if self.workgroups.is_empty() {
            return Vec::new();
        }
        self.visible(tab).into_iter().filter_map(|(id, r)| self.diff_overlay_for(id).map(|q| (id, r, q))).collect()
    }

    /// Draws each panel: the pane's text stops under it (`cut_out`), its box is a popup and its words
    /// are rows over it (P22).
    pub(super) fn draw_diff_overlays(&mut self, frame: &mut scene::Frame, cell: (f32, f32)) {
        use scene::{Quad, Span, TextRow};
        let panels = self.overlay_panels();
        if panels.is_empty() {
            return;
        }
        let s = self.scale();
        let span = |text: String, color: crate::palette::Rgb, bold: bool| Span { text, color, bold, italic: false, alt_font: false, icon: false, prose: false, badge: false, toolbelt: false, fake_bold: false, ligatures: 0, mask: 0, font: 0 };
        let mix = |a: crate::palette::Rgb, b: crate::palette::Rgb, t: f32| {
            let m = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round().clamp(0.0, 255.0) as u8;
            crate::palette::Rgb { r: m(a.r, b.r), g: m(a.g, b.g), b: m(a.b, b.b) }
        };
        for (pane, panel, queued) in panels {
            let Some(p) = self.panes.get(&pane) else { continue };
            let (bg, fg, selected) = (p.screen.palette.bg, p.screen.palette.fg, p.screen.palette.selection);
            let (back, dim, line) = (mix(bg, fg, 0.04), mix(bg, fg, 0.6), mix(bg, fg, 0.3));
            let refusal = self.run_anyway_refusal(pane).is_some();
            let g = overlay_geom(panel, cell, s, queued);
            scene::cut_out(frame, panel, cell.1);
            frame.popups.push(Quad { rect: panel, color: scene::rgba(back, 1.0) });
            let row = |x: f32, y: f32, spans: Vec<Span>, clip: Rect| TextRow { x, y, spans, clip, scale: 1.0, face: crate::render::DEFAULT_FACE };
            let (title, body) = if queued { (wgit::RELOAD_TITLE, wgit::RELOAD_BODY) } else { (wgit::WAITING_TITLE, wgit::WAITING_BODY) };
            frame.rows.push(row(g.title.x, g.title.y, vec![span(title.to_string(), fg, true)], g.title));
            for (i, l) in wrap_words(body, g.body.w, cell.0).into_iter().enumerate() {
                let y = g.body.y + i as f32 * cell.1;
                if y + cell.1 > g.body.y + g.body.h {
                    break;
                }
                frame.rows.push(row(g.body.x, y, vec![span(l, dim, false)], g.body));
            }
            let button = |frame: &mut scene::Frame, r: Rect, label: &str, on: bool| {
                frame.popups.push(Quad { rect: r, color: scene::rgba(mix(back, fg, 0.12), 1.0) });
                for q in [Rect { h: s.max(1.0), ..r }, Rect { y: r.y + r.h - s.max(1.0), h: s.max(1.0), ..r }, Rect { w: s.max(1.0), ..r }, Rect { x: r.x + r.w - s.max(1.0), w: s.max(1.0), ..r }] {
                    frame.popups.push(Quad { rect: q, color: scene::rgba(if on { selected } else { line }, 1.0) });
                }
                let tw = unicode_width::UnicodeWidthStr::width(label) as f32 * cell.0;
                frame.rows.push(row(r.x + (r.w - tw) / 2.0, r.y + (r.h - cell.1) / 2.0, vec![span(label.to_string(), if on { fg } else { dim }, false)], r));
            };
            button(frame, g.run, wgit::RUN_ANYWAY, !refusal);
            if let Some(c) = g.cancel {
                button(frame, c, crate::api_auth::CANCEL, true);
            }
        }
    }

    /// A press on a waiting panel: Run Anyway and Cancel act on the left button; the rest of the panel
    /// takes the press and does nothing (the terminal is not under it). True when it was on a panel.
    pub(super) fn diff_overlay_press(&mut self, button: MouseButton, x: f32, y: f32) -> bool {
        let s = self.scale();
        let cell = self.cell();
        let Some((pane, panel, queued)) = self.overlay_panels().into_iter().find(|(_, r, _)| r.contains(x, y)) else { return false };
        if button == MouseButton::Left {
            let g = overlay_geom(panel, cell, s, queued);
            if g.run.contains(x, y) {
                self.diff_run_anyway(pane);
            } else if g.cancel.is_some_and(|c| c.contains(x, y)) {
                self.diff_cancel_reload(pane);
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    const THIS: &str = include_str!("workgroup_git_app.rs");

    /// Nothing is ever typed into a pane here: every start is a restart with an argument vector (B4).
    #[test]
    fn restart_never_types_into_the_pane() {
        let live = THIS.split("\n#[cfg(test)]").next().unwrap();
        for sink in ["program_write", "cmdinfo_write", "user_write", "write_pane", ".write(", "paste_text"] {
            assert!(!live.contains(sink), "{sink}");
        }
        assert!(live.contains("restart_launching(pane)"));
    }

    /// Git runs through `guarded_args` after `vet_repo` passed, and Run Anyway needs the same vet.
    #[test]
    fn diff_runs_only_in_a_vetted_repo() {
        let live = THIS.split("\n#[cfg(test)]").next().unwrap();
        let line = &live[live.find("pub(super) fn diff_launch_line").unwrap()..live.find("pub(super) fn diff_selected").unwrap()];
        let vet = line.find("gs::vet_repo(").expect("vetted");
        let guarded = line.find("gs::launch(").expect("launched through the one builder");
        assert!(vet < guarded, "the vet comes first");
        assert!(line.contains("inst.approved.allows(&template)"), "the command comes from the approved list");
        let any = &live[live.find("pub(super) fn run_anyway_refusal").unwrap()..live.find("pub(super) fn diff_run_anyway").unwrap()];
        assert!(any.contains("gs::vet_repo("));
        let run = &live[live.find("pub(super) fn diff_run_anyway").unwrap()..live.find("pub(super) fn diff_cancel_reload").unwrap()];
        assert!(run.find("run_anyway_refusal").unwrap() < run.find("launch_diff").unwrap());
    }

    /// A workgroup gets a git side only when a member is in Diff mode or an item needs the poller; a
    /// launch is ready only after an answer for the current base, and only with a diffable change.
    #[test]
    fn the_git_side_follows_iterm2() {
        use super::*;
        use crate::git_state::{Change, FileStatus, GitState};
        let claude = crate::workgroup::claude_code::config();
        let mut side = GitSide::for_workgroup(&claude).expect("a Diff member needs git");
        assert_eq!(side.selectors.keys().map(String::as_str).collect::<Vec<_>>(), [crate::workgroup::claude_code::DIFF]);
        assert!(!side.ready(), "no answer yet");
        side.poller.state = Some(GitState { file_statuses: Some(vec![FileStatus { path: "a".into(), index: Change::None, workdir: Change::Modified }]), ..Default::default() });
        assert!(!side.ready(), "an answer for an older base does not count");
        side.fresh = true;
        assert!(side.ready());
        side.poller.state = Some(GitState { file_statuses: Some(vec![FileStatus { path: "u".into(), index: Change::None, workdir: Change::Untracked }]), ..Default::default() });
        assert!(!side.ready(), "untracked files are not diffable");
        side.wait_for_changes(crate::workgroup::claude_code::DIFF);
        assert_eq!(side.pending.get(crate::workgroup::claude_code::DIFF), Some(&Pending { queued: false }));
        let mut plain = claude.clone();
        for s in &mut plain.sessions {
            s.mode = Mode::Regular;
            s.toolbar_items.retain(|i| !i.needs_git_poller());
        }
        assert!(GitSide::for_workgroup(&plain).is_none());
    }

    /// Class rule: ONE function builds every git invocation of workgroup code. No workgroup source builds
    /// a guarded line or runs git itself; the launch takes the vet's verdict and the launch environment.
    #[test]
    fn git_is_built_in_one_place() {
        let sources = [
            include_str!("workgroup_git_app.rs"),
            include_str!("workgroup_app.rs"),
            include_str!("workgroup_toolbar_ui.rs"),
            include_str!("../workgroup_git.rs"),
            include_str!("../workgroup_run.rs"),
            include_str!("../workgroup_toolbar.rs"),
            include_str!("../workgroup_peers.rs"),
        ];
        for src in sources {
            let live = src.split("\n#[cfg(test)]").next().unwrap();
            for banned in ["guarded_args(", "HARDENED", "Command::new(git", "Command::new(\"git"] {
                assert!(!live.contains(banned), "{banned}");
            }
        }
        let live = THIS.split("\n#[cfg(test)]").next().unwrap();
        let line = &live[live.find("pub(super) fn diff_launch_line").unwrap()..live.find("pub(super) fn diff_selected").unwrap()];
        assert!(line.contains("gs::launch(") && line.contains("launch_refusal()"), "the one builder, after the verdict");
    }

    /// The panel's Cancel exists only on the queued-reload variant; every part sits inside the panel.
    #[test]
    fn the_overlay_has_cancel_only_when_queued() {
        use super::*;
        let panel = Rect { x: 100.0, y: 50.0, w: 600.0, h: 300.0 };
        let (a, b) = (overlay_geom(panel, (8.0, 16.0), 1.0, false), overlay_geom(panel, (8.0, 16.0), 1.0, true));
        assert!(a.cancel.is_none() && b.cancel.is_some());
        for r in [a.title, a.body, a.run, b.cancel.unwrap()] {
            assert!(r.x >= panel.x && r.y >= panel.y && r.x + r.w <= panel.x + panel.w && r.y + r.h <= panel.y + panel.h, "{r:?}");
        }
        assert!(b.cancel.unwrap().x + b.cancel.unwrap().w < b.run.x, "Cancel is left of Run Anyway");
        assert_eq!(wrap_words("one two three", 8.0 * 7.0, 8.0), ["one two", "three"]);
    }
}
