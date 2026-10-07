//! Entering and exiting a workgroup (milestone 13y1, Task 3): the live instances, spawning the
//! members, tearing them down, Shell > Workgroups and the close warning. The pure side is
//! `workgroup_run`.
//!
//! Citations: `WC` `iTermWorkgroupController.swift`, `WI` `iTermWorkgroupInstance.swift`, `WSS`
//! `WorkgroupSessionSpawner.swift` under `~/gitrepos/iterm2-ref/sources/Workgroups`.

use super::*;
use crate::workgroup::{Mode, SessionConfig, Workgroup};
use crate::workgroup_run::{self as run, Mechanism, Plan, SpawnStep};
use std::collections::BTreeMap;

/// Live instances at most (a hostile host printing a trigger's text after every exit cannot fan
/// out sessions without end, P3).
const MAX_INSTANCES: usize = 8;
/// A trigger enters one workgroup per session this often at most (P3).
const TRIGGER_GAP: Duration = Duration::from_secs(5);

/// One entered workgroup.
pub(super) struct Instance {
    pub id: String,
    pub workgroup: Workgroup,
    pub leader: PaneId,
    /// Config id and its session: the root first (the leader), then its peers; None is a peer that
    /// has not started (a Diff or Code Review peer waits, Tasks 6 and 7). A peer lives in
    /// `App.panes` with `Pane.workgroup` set and in no layout; it is never in `App.buried` (B2).
    pub peers: Vec<(String, Option<PaneId>)>,
    /// The config id of the member shown in the leader's place (Task 4 swaps it).
    #[allow(dead_code)]
    pub active: String,
    /// Splits and tabs, in the order they started.
    pub non_peers: Vec<(String, PaneId)>,
    /// The workgroup-wide base ref (Task 6) and who started the entry (the orchestration snapshot, Task 10).
    #[allow(dead_code)]
    pub git_base: String,
    #[allow(dead_code)]
    pub provenance: String,
    /// The leader's name before the root's display name replaced it.
    pub leader_title: Option<String>,
    /// The entry tab's group before the workgroup's: Some(None) was no group.
    pub group_before: Option<Option<crate::tab_groups::TabGroup>>,
    pub torn_down: bool,
}

impl Instance {
    /// Every live member's pane, the leader first, each once.
    pub fn members(&self) -> Vec<PaneId> {
        let mut out = vec![self.leader];
        for p in self.peers.iter().filter_map(|(_, p)| *p).chain(self.non_peers.iter().map(|(_, p)| *p)) {
            if !out.contains(&p) {
                out.push(p);
            }
        }
        out
    }
}

/// The live instances by id, and what the next tick closes.
#[derive(Default)]
pub(super) struct Instances {
    map: BTreeMap<String, Instance>,
    /// Members a teardown ends; closed on the next tick (the leader's own close must finish first,
    /// `WI:deferredClose`), by `tick_workgroups`.
    to_close: Vec<PaneId>,
    /// When a trigger last entered a workgroup on a session.
    recent: HashMap<PaneId, Instant>,
}

impl Instances {
    pub fn get(&self, id: &str) -> Option<&Instance> {
        self.map.get(id)
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn values(&self) -> impl Iterator<Item = &Instance> {
        self.map.values()
    }
}

/// What starting one member did.
enum Made {
    Pane(PaneId),
    /// Not started, with its reason already shown (a refused profile or command).
    Skipped,
    /// Could not start: the entry is abandoned.
    Failed,
}

/// Where a member goes.
enum Place {
    Peer,
    Split { parent: PaneId, vertical: bool, before: bool, fraction: f64 },
    Tab,
}

impl App {
    /// The instance a session belongs to.
    pub(super) fn workgroup_of(&self, pane: PaneId) -> Option<&Instance> {
        self.workgroups.get(self.panes.get(&pane)?.workgroup.as_deref()?)
    }

    /// (instance id, its workgroup id, `pane` is its leader), the form `run::disposition` takes.
    fn workgroup_facts(&self, pane: PaneId) -> Option<(String, String, bool)> {
        self.workgroup_of(pane).map(|i| (i.id.clone(), i.workgroup.unique_identifier.clone(), i.leader == pane))
    }

    /// Enters the workgroup `id` on `pane` (`WC:enter`). An import or a trigger asks first, and the
    /// answer enters; true means the session is in the workgroup now.
    pub(super) fn enter_workgroup(&mut self, id: &str, pane: PaneId, how: Mechanism) -> bool {
        let Some(w) = self.workgroup_model.get(id).cloned() else {
            self.banners.set("workgroup", "That workgroup no longer exists.");
            return false;
        };
        let source = self.workgroup_model.source_of(id);
        let facts = self.workgroup_facts(pane);
        let d = run::disposition(Some(&w), facts.as_ref().map(|(i, g, l)| (i.as_str(), g.as_str(), *l)), false);
        match d {
            run::Disposition::AlreadyEntered => return true,
            run::Disposition::Refused(r) => {
                self.banners.set("workgroup", match r {
                    run::Refusal::Restoring => "This session is being restored into a workgroup.".to_string(),
                    run::Refusal::Unusable => "That workgroup has no main session.".to_string(),
                    run::Refusal::NonLeader(_) => "Exit the workgroup from its main session before entering another.".to_string(),
                });
                return false;
            }
            run::Disposition::Proceed { .. } => {}
        }
        if let Some(why) = self.entry_refusal(pane, how) {
            self.banners.set("workgroup", why);
            return false;
        }
        if run::needs_ask(source, how, w.sessions.len()) {
            let (heading, body) = run::ask_text(&w, how);
            let then = scripts_alerts::Then::EnterWorkgroup { workgroup: Box::new(w), pane, how };
            let ask = scripts_alerts::ask(&heading, &body, &["Enter", crate::api_auth::CANCEL], None, crate::warnings::Kind::Persistent, then);
            self.ask_scripts(ask);
            return false;
        }
        self.enter_workgroup_now(w, pane, how)
    }

    /// Why this session cannot enter a workgroup now (P6, P3).
    fn entry_refusal(&mut self, pane: PaneId, how: Mechanism) -> Option<String> {
        let Some(p) = self.panes.get(&pane) else { return Some("That session has closed.".into()) };
        if p.tmux_io().is_some() || self.tmux_of(pane).is_some() {
            return Some("A tmux session cannot be the main session of a workgroup.".into());
        }
        if self.in_instant_replay() || p.archived || p.dvr.read_only {
            return Some("A workgroup cannot start in a replay or an archive.".into());
        }
        if !self.layout_allowed_quiet(crate::winmodel::LayoutOp::Split) {
            return Some("The layout is locked, so the workgroup cannot add its sessions.".into());
        }
        if self.workgroups.len() >= MAX_INSTANCES {
            return Some(format!("{MAX_INSTANCES} workgroups are running; exit one first."));
        }
        if matches!(how, Mechanism::Trigger | Mechanism::BrowserTrigger) {
            let now = Instant::now();
            if self.workgroups.recent.get(&pane).is_some_and(|t| now.saturating_duration_since(*t) < TRIGGER_GAP) {
                return Some("A trigger entered a workgroup in this session a moment ago.".into());
            }
            self.workgroups.recent.retain(|_, t| now.saturating_duration_since(*t) < TRIGGER_GAP);
            self.workgroups.recent.insert(pane, now);
        }
        None
    }

    /// Enters `w` on `pane` without asking: the menu's own entry, or an answered question. The
    /// question showed this very workgroup, so it is carried, not looked up again.
    pub(super) fn enter_workgroup_now(&mut self, w: Workgroup, pane: PaneId, how: Mechanism) -> bool {
        let facts = self.workgroup_facts(pane);
        let d = run::disposition(Some(&w), facts.as_ref().map(|(i, g, l)| (i.as_str(), g.as_str(), *l)), false);
        let exit_first = match d {
            run::Disposition::AlreadyEntered => return true,
            run::Disposition::Refused(_) => return false,
            run::Disposition::Proceed { exit_first } => exit_first,
        };
        if let Some(why) = self.entry_refusal_quiet(pane) {
            self.banners.set("workgroup", why);
            return false;
        }
        if let Some(old) = exit_first {
            self.teardown_workgroup(&old, None);
        }
        if !self.enter_pane_window(pane) {
            return false;
        }
        let Some(root) = w.root().cloned() else { return false };
        let Some(entry_tab) = self.tabs.iter().position(|t| t.layout.panes().contains(&pane)) else { return false };
        let instance_id = run::mint_instance_id();
        let _ = self.api_vars.set_user(crate::api_vars::Scope::Session, &pane.to_string(), "user.gitBase", serde_json::json!("HEAD"));
        let mut inst = Instance {
            id: instance_id.clone(),
            workgroup: w.clone(),
            leader: pane,
            peers: vec![(root.unique_identifier.clone(), Some(pane))],
            active: root.unique_identifier.clone(),
            non_peers: Vec::new(),
            git_base: "HEAD".into(),
            provenance: how.provenance(&self.session_guid_of(pane)),
            leader_title: None,
            group_before: None,
            torn_down: false,
        };
        // config id -> the pane that hosts its children
        let mut hosts: HashMap<String, PaneId> = HashMap::from([(root.unique_identifier.clone(), pane)]);
        let mut spawned_tabs: Vec<u64> = Vec::new();
        let mut failed = false;
        for step in run::spawn_plan(&w) {
            let (config, place) = match &step {
                SpawnStep::Peer { config } => (config, Place::Peer),
                SpawnStep::Split { config, parent, vertical, before, fraction } => {
                    let Some(&host) = hosts.get(parent) else { continue };
                    (config, Place::Split { parent: host, vertical: *vertical, before: *before, fraction: *fraction })
                }
                SpawnStep::Tab { config, parent } => {
                    if !hosts.contains_key(parent) {
                        continue;
                    }
                    (config, Place::Tab)
                }
            };
            let Some(cfg) = w.session(config).cloned() else { continue };
            // A Diff peer waits for changes (Task 6) and a Code Review peer for its prompt (Task 7).
            if matches!(place, Place::Peer) && cfg.mode != Mode::Regular {
                inst.peers.push((config.clone(), None));
                continue;
            }
            match self.spawn_member(&w, &cfg, &instance_id, pane, place, entry_tab, spawned_tabs.len()) {
                Made::Pane(id) => {
                    match step {
                        SpawnStep::Peer { .. } => inst.peers.push((config.clone(), Some(id))),
                        SpawnStep::Split { .. } => {
                            inst.non_peers.push((config.clone(), id));
                            hosts.insert(config.clone(), id);
                        }
                        SpawnStep::Tab { .. } => {
                            inst.non_peers.push((config.clone(), id));
                            hosts.insert(config.clone(), id);
                            if let Some(t) = self.tabs.iter().find(|t| t.layout.panes().contains(&id)) {
                                spawned_tabs.push(t.id);
                            }
                        }
                    }
                }
                Made::Skipped => {}
                Made::Failed => {
                    failed = true;
                    break;
                }
            }
        }
        if failed {
            // A member that could not start abandons the entry; it is not registered (`WC:61-70`).
            for p in inst.members().into_iter().filter(|p| *p != pane) {
                if let Some(m) = self.panes.get_mut(&p) {
                    m.workgroup = None;
                }
                self.workgroups.to_close.push(p);
            }
            return false;
        }
        // Gather the entry tab and the spawned tabs into one group named for the workgroup.
        if !spawned_tabs.is_empty() {
            let entry_id = self.tabs[entry_tab].id;
            inst.group_before = Some(self.tabs[entry_tab].group.clone());
            let count = crate::tab_groups::groups_in_window(self.tabs.iter().map(|t| t.group.as_ref())).len();
            let group = crate::tab_groups::TabGroup { id: crate::tab_groups::fresh_id(), name: crate::inert_files::display_text(crate::workgroup::display_name(&w), 128, false), color: crate::tab_groups::next_color(count), collapsed: false };
            for tab_id in std::iter::once(entry_id).chain(spawned_tabs) {
                if let Some(at) = self.tabs.iter().position(|t| t.id == tab_id) {
                    self.tabs[at].group = Some(group.clone());
                    self.set_tab_group(at, Some(group.id.clone()));
                }
            }
            self.group_changed();
        }
        // The leader takes the root's display name; its own is put back at exit.
        if !root.display_name.is_empty() {
            let name = crate::inert_files::display_text(&root.display_name, 128, false);
            inst.leader_title = self.panes.get(&pane).and_then(|p| self.store.get(&p.profile)).map(|p| p.name());
            self.set_session_values(pane, &[(crate::profile::NAME.to_string(), Some(toml::Value::String(name)))]);
        }
        if let Some(p) = self.panes.get_mut(&pane) {
            p.workgroup = Some(instance_id.clone());
        }
        crate::rlog!("workgroup entered: instance {} members {}", crate::dlog::name(&instance_id), inst.members().len());
        self.workgroups.map.insert(instance_id, inst);
        self.menus_dirty = true;
        self.relayout();
        true
    }

    /// The refusals that can change between a question and its answer (the entry's first look has
    /// the rest, and a trigger's rate limit counted that first look).
    fn entry_refusal_quiet(&self, pane: PaneId) -> Option<String> {
        let p = self.panes.get(&pane).map_or(Some("That session has closed.".to_string()), |_| None);
        p.or_else(|| (self.workgroups.len() >= MAX_INSTANCES).then(|| format!("{MAX_INSTANCES} workgroups are running; exit one first.")))
    }

    fn session_guid_of(&self, pane: PaneId) -> String {
        self.panes.get(&pane).map(|p| p.session_guid.clone()).unwrap_or_default()
    }

    /// Starts one member beside `leader`. A peer joins no layout; a split goes beside its parent at
    /// its fraction; a tab goes after the entry tab (and the tabs this entry already added).
    #[allow(clippy::too_many_arguments)]
    fn spawn_member(&mut self, w: &Workgroup, cfg: &SessionConfig, instance_id: &str, leader: PaneId, place: Place, entry_tab: usize, added_tabs: usize) -> Made {
        if !run::valid_instance_id(instance_id) {
            return Made::Failed;
        }
        // A dynamic or Bonjour profile never takes part in a crowd (P4); a missing one is the default.
        let guid = cfg.profile_guid.clone().filter(|g| self.store.get(g).is_some());
        if w.sessions.len() > 1 {
            if let Some(p) = guid.as_deref().and_then(|g| self.store.get(g)) {
                if p.bool(crate::dynamic_profiles::DYNAMIC_KEY) || self.is_bonjour_profile(&p) {
                    self.banners.set("workgroup", format!("A member was left out: its profile {} is not a saved one.", crate::inert_files::display_text(&p.name(), 64, false)));
                    return Made::Skipped;
                }
            }
        }
        let plan = match run::command_plan(&cfg.command, "HEAD") {
            Ok(p) => p,
            Err(why) => {
                self.banners.set("workgroup", why);
                return Made::Skipped;
            }
        };
        let command = match plan {
            Plan::Profile => None,
            Plan::Line(l) => Some(l),
            Plan::Argv(argv) => {
                let Some(program) = crate::semantic::resolve_on_path(std::path::Path::new(&argv[0]), std::env::var_os("PATH").as_deref()) else {
                    self.banners.set("workgroup", "A member's program was not found, so it did not start.");
                    return Made::Skipped;
                };
                match run::argv_line(&program, &argv[1..]) {
                    Ok(l) => Some(l),
                    Err(why) => {
                        self.banners.set("workgroup", why);
                        return Made::Skipped;
                    }
                }
            }
        };
        let Some(t) = self.tabs.iter().position(|t| t.layout.panes().contains(&leader)) else { return Made::Failed };
        let area = self.content_area();
        let leader_rect = self.tabs[t].layout.rect_of(leader, area, self.gap()).unwrap_or(area);
        let rect = match &place {
            Place::Peer => leader_rect,
            Place::Tab => area,
            Place::Split { parent, vertical, .. } => {
                let r = self.tabs.iter().find_map(|t| t.layout.rect_of(*parent, area, self.gap())).unwrap_or(leader_rect);
                if *vertical { Rect { w: r.w / 2.0, ..r } } else { Rect { h: r.h / 2.0, ..r } }
            }
        };
        let profile = self.profile(guid.as_deref());
        self.start_extra_env = vec![format!("ITERM_WORKGROUP_ID={instance_id}")];
        self.start_command_override = command.clone();
        (self.start_object, self.start_from) = (crate::start_dir::Object::Pane, Some(leader));
        let started = if command.is_none() && self.is_browser_profile(&profile) {
            let id = self.next_id;
            let ok = self.start_browser_pane(id, rect, &run::member_profile(&profile, &cfg.url_string));
            if ok {
                self.next_id += 1;
            }
            ok.then_some(id)
        } else {
            self.start_tab = Some(t);
            let id = self.start_pane(rect, guid.as_deref());
            self.start_tab = None;
            id
        };
        (self.start_object, self.start_from) = (crate::start_dir::Object::Tab, None);
        self.start_command_override = None;
        self.start_extra_env.clear();
        let Some(id) = started else { return Made::Failed };
        if let Some(p) = self.panes.get_mut(&id) {
            p.workgroup = Some(instance_id.to_string());
        }
        match place {
            Place::Peer => {}
            Place::Split { parent, vertical, before, fraction } => {
                let axis = if vertical { Axis::Row } else { Axis::Column };
                let at = self.tabs.iter().position(|t| t.layout.panes().contains(&parent)).unwrap_or(t);
                if !self.tabs[at].layout.split_at(parent, axis, id, before, fraction as f32) {
                    self.workgroups.to_close.push(id);
                    return Made::Failed;
                }
                self.tabs[at].zoomed = None;
            }
            Place::Tab => {
                let active = self.tabs.get(self.active).map(|t| t.id);
                let at = (entry_tab + 1 + added_tabs).min(self.tabs.len());
                self.tabs.insert(at, Tab::new(self.next_tab_id, Layout::new(id), id));
                self.next_tab_id += 1;
                if let Some(a) = active.and_then(|a| self.tabs.iter().position(|t| t.id == a)) {
                    self.active = a;
                }
            }
        }
        Made::Pane(id)
    }

    fn is_bonjour_profile(&self, p: &Profile) -> bool {
        !super::bonjour_ui::without_bonjour(&self.store, vec![p.guid()]).contains(&p.guid())
    }

    /// Exit Workgroup (`WC:exit(on:)`): every peer and child ends, the leader stays in its place as a
    /// plain session. The ends happen on the next tick.
    pub(super) fn exit_workgroup(&mut self, pane: PaneId) {
        let Some(id) = self.workgroup_of(pane).map(|i| i.id.clone()) else {
            self.banners.set("workgroup", "This session is not in a workgroup.");
            return;
        };
        self.teardown_workgroup(&id, None);
        self.menus_dirty = true;
        self.relayout();
    }

    /// Any member ending tears the instance down (`WI:257-296`); the closing pane itself is left to
    /// close. A leader that ends takes the rest with it; any other member ends and the leader
    /// stays as a plain session.
    pub(super) fn workgroup_member_ended(&mut self, pane: PaneId) {
        if let Some(id) = self.panes.get(&pane).and_then(|p| p.workgroup.clone()) {
            self.teardown_workgroup(&id, Some(pane));
        }
    }

    /// A member's program ended: the same teardown, and a member that is not the leader goes too.
    pub(super) fn workgroup_member_exited(&mut self, pane: PaneId) {
        let leader = self.workgroup_of(pane).map(|i| i.leader);
        self.workgroup_member_ended(pane);
        if leader.is_some_and(|l| l != pane) {
            self.workgroups.to_close.push(pane);
        }
    }

    /// The synchronous half of a teardown (`WI:teardown`): unregister, clear every back-pointer,
    /// put the leader's name and the entry tab's group back, and queue the members to end, in spawn
    /// order. `skip` is the member whose close drove it.
    fn teardown_workgroup(&mut self, id: &str, skip: Option<PaneId>) {
        let Some(mut inst) = self.workgroups.map.remove(id) else { return };
        if inst.torn_down {
            return;
        }
        inst.torn_down = true;
        // (Task 4: a peer shown in the leader's place is swapped back out before the peers end.)
        let non_peers: Vec<String> = inst.non_peers.iter().map(|(c, _)| c.clone()).collect();
        let by_config: HashMap<&str, PaneId> = inst.non_peers.iter().map(|(c, p)| (c.as_str(), *p)).collect();
        let mut ending: Vec<PaneId> = inst.peers.iter().filter_map(|(_, p)| *p).collect();
        ending.extend(run::teardown_order(&non_peers).iter().filter_map(|c| by_config.get(c.as_str()).copied()));
        for p in inst.members() {
            if let Some(m) = self.panes.get_mut(&p) {
                m.workgroup = None;
            }
        }
        for p in ending {
            if p != inst.leader && Some(p) != skip && !self.workgroups.to_close.contains(&p) {
                self.workgroups.to_close.push(p);
            }
        }
        // The leader's own name, and the entry tab's own group.
        if skip != Some(inst.leader) && self.panes.contains_key(&inst.leader) {
            if let Some(name) = inst.leader_title.take() {
                self.set_session_values(inst.leader, &[(crate::profile::NAME.to_string(), Some(toml::Value::String(name)))]);
            }
            if let Some(before) = inst.group_before.take() {
                if let Some(at) = self.tabs.iter().position(|t| t.layout.panes().contains(&inst.leader)) {
                    let gid = before.as_ref().map(|g| g.id.clone());
                    self.tabs[at].group = before;
                    self.set_tab_group(at, gid);
                    self.group_changed();
                }
            }
        }
        self.workgroups.recent.remove(&inst.leader);
        self.menus_dirty = true;
        crate::rlog!("workgroup exited: instance {}", crate::dlog::name(id));
    }

    /// Ends the members a teardown queued (once the closing pane's own close has finished).
    pub(super) fn tick_workgroups(&mut self, event_loop: &ActiveEventLoop) {
        if self.workgroups.to_close.is_empty() {
            return;
        }
        for id in std::mem::take(&mut self.workgroups.to_close) {
            if self.panes.contains_key(&id) {
                self.close_pane(id, event_loop);
            }
        }
        self.redraw();
    }

    /// The close confirmation's extra line when closing `panes` would take other members of a
    /// workgroup with them (`WI:closeCascadeWarning`).
    pub(super) fn workgroup_close_warning(&self, panes: &[PaneId]) -> Option<String> {
        let groups: Vec<(String, usize)> = self
            .workgroups
            .values()
            .filter(|i| i.members().iter().any(|m| panes.contains(m)))
            .map(|i| {
                let extra = i.members().into_iter().filter(|m| !panes.contains(m) && self.panes.get(m).is_some_and(|p| p.exited.is_none())).count();
                (crate::workgroup::display_name(&i.workgroup).to_string(), extra)
            })
            .collect();
        run::close_warning(&groups)
    }

    /// Shell > Workgroups: the workgroups above the separator, checked for the current one, and
    /// Exit Workgroup enabled in a workgroup. The separator is out while there are none.
    pub(super) fn refresh_workgroups_menu(&mut self) {
        let focused = self.tabs.get(self.active).map(|t| t.focused).filter(|p| self.panes.contains_key(p));
        let facts = focused.and_then(|p| self.workgroup_facts(p));
        let entries = run::menu_entries(&self.workgroup_model.workgroups, facts.as_ref().map(|(i, g, l)| (i.as_str(), g.as_str(), *l)), focused.is_some(), false);
        let in_workgroup = facts.is_some();
        let Some(bar) = &mut self.menu_bar else { return };
        let Some(menu) = &mut bar.workgroups else { return };
        if let Some(exit) = &menu.exit {
            exit.set_enabled(in_workgroup);
        }
        let shown: Vec<(String, String, bool, bool)> = entries.iter().map(|e| (e.id.clone(), e.label.clone(), e.enabled, e.checked)).collect();
        if menu.shown == shown {
            return;
        }
        for _ in 0..menu.shown.len() {
            let _ = menu.sub.remove_at(0);
        }
        for (i, e) in entries.iter().enumerate() {
            let _ = menu.sub.insert(&muda::CheckMenuItem::with_id(e.id.clone(), e.label.clone(), e.enabled, e.checked, None), i);
        }
        // The separator sits between the entries and Exit Workgroup, and only with entries.
        if let Some(sep) = &menu.separator {
            if menu.sep_shown && entries.is_empty() {
                let _ = menu.sub.remove(sep);
                menu.sep_shown = false;
            } else if !menu.sep_shown && !entries.is_empty() {
                let _ = menu.sub.insert(sep, entries.len());
                menu.sep_shown = true;
            }
        }
        menu.shown = shown;
    }

    /// A pick from Shell > Workgroups. False for other ids.
    pub(super) fn workgroup_picked(&mut self, id: &str) -> bool {
        let Some(wid) = id.strip_prefix(run::MENU_ID) else { return false };
        if let Some(pane) = self.tabs.get(self.active).map(|t| t.focused) {
            self.enter_workgroup(wid, pane, Mechanism::Menu);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    const MOD: &str = include_str!("mod.rs");
    const THIS: &str = include_str!("workgroup_app.rs");

    /// B1: the new one-shot is read once and emptied; the REPL's override and its four guards are untouched.
    #[test]
    fn workgroup_env_keeps_session_guards() {
        assert_eq!(MOD.matches("self.start_command_override.is_none() && self.start_env_override.is_none()").count(), 4, "the guards: the restart, the locale ask, start_pane_as and the injection");
        assert_eq!(MOD.matches("std::mem::take(&mut self.start_extra_env)").count(), 1, "read once and emptied");
        assert!(THIS.contains("self.start_extra_env = vec![format!(\"ITERM_WORKGROUP_ID={instance_id}\")]"));
        assert!(THIS.contains("run::valid_instance_id(instance_id)"), "only a minted id reaches the environment");
    }

    /// B2: a peer never enters the buried store.
    #[test]
    fn peers_are_never_buried() {
        let production = THIS.split("#[cfg(test)]").next().unwrap();
        for banned in ["bury_pane", "self.buried", "Burial", "disinter"] {
            assert!(!production.contains(banned), "{banned}");
        }
    }

    /// P18: nothing a program or a file supplies is logged.
    #[test]
    fn workgroup_logs_carry_only_names() {
        for line in THIS.lines().filter(|l| l.trim_start().starts_with("crate::rlog!(")) {
            assert!(line.contains("dlog::name("), "{line}");
        }
    }

    /// Every pane close and every program exit reaches the teardown; the menu pick, the exit command and the close warning are wired.
    #[test]
    fn the_hooks_are_wired() {
        assert!(MOD.contains("self.workgroup_member_ended(id);"));
        assert!(include_str!("session_end_ui.rs").contains("self.workgroup_member_exited(id);"));
        assert!(include_str!("menu.rs").contains("\"exitWorkgroup:\" =>"));
        assert!(include_str!("menu.rs").contains("self.workgroup_picked(id)"));
        assert!(include_str!("close_ui.rs").contains("self.workgroup_close_warning("));
        assert!(include_str!("scripts_alerts.rs").contains("Then::EnterWorkgroup { workgroup, pane, how } =>"));
    }
}
