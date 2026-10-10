//! Entering and exiting a workgroup (milestone 13y1, Task 3): the live instances, spawning the
//! members, tearing them down, Shell > Workgroups and the close warning. The pure side is
//! `workgroup_run`.
//!
//! Citations: `WC` `iTermWorkgroupController.swift`, `WI` `iTermWorkgroupInstance.swift`, `WSS`
//! `WorkgroupSessionSpawner.swift` under `~/gitrepos/iterm2-ref/sources/Workgroups`.

use super::*;
use crate::workgroup::{Mode, SessionConfig, Workgroup};
use crate::workgroup_peers as peers;
use crate::workgroup_run::{self as run, Mechanism, SpawnStep};
use std::collections::BTreeMap;

/// Live instances at most (a hostile host printing a trigger's text after every exit cannot fan
/// out sessions without end, P3).
pub(super) const MAX_INSTANCES: usize = 8;
/// A trigger enters one workgroup per session this often at most (P3).
const TRIGGER_GAP: Duration = Duration::from_secs(5);

/// A trigger's request, held until the trigger pass is over (iTerm2's callback scheduler: entering
/// spawns sessions, which must not happen inside the pass that matched the output).
#[derive(Clone, Debug, PartialEq)]
pub(super) enum TriggerWork {
    /// Enter Workgroup: the stored parameter (empty: the first workgroup by name).
    Enter(String),
    /// Exit Workgroup, leader-only or from any member.
    Exit(bool),
    /// A browser trigger's Enter Workgroup (page text, never the person's consent): the same queue,
    /// dedupe, declined and restoring guards as the terminal trigger, entered as `BrowserTrigger`.
    EnterBrowser(String),
    /// A browser trigger's Exit Workgroup (`XWB`: no leader-only option).
    ExitBrowser,
}

/// Requests held at most (a trigger's text printed in a loop cannot grow the queue).
const MAX_TRIGGER_QUEUE: usize = 16;

/// One entered workgroup.
pub(super) struct Instance {
    pub id: String,
    pub workgroup: Workgroup,
    pub leader: PaneId,
    /// Config id and its session: the root first (the leader), then its peers; None is a peer that
    /// has not started (a Diff or Code Review peer waits, Tasks 6 and 7). A peer lives in
    /// `App.panes` with `Pane.workgroup` set and in no layout; it is never in `App.buried` (B2).
    pub peers: Vec<(String, Option<PaneId>)>,
    /// The config id of the member shown in the leader's place (`activate_peer` swaps it).
    pub active: String,
    /// Splits and tabs, in the order they started.
    pub non_peers: Vec<(String, PaneId)>,
    /// The workgroup-wide base ref (Task 6) and who started the entry (the orchestration snapshot, Task 10).
    pub git_base: String,
    pub provenance: String,
    /// The leader's name before the root's display name replaced it.
    pub leader_title: Option<String>,
    /// The entry tab's group before the workgroup's: Some(None) was no group.
    pub group_before: Option<Option<crate::tab_groups::TabGroup>>,
    pub torn_down: bool,
    /// When it was entered, counting from 1 (`Instances::add` sets it): the Cockpit lists instances in this order.
    pub seq: u64,
    /// AI Usage items' reports and runs, by item key (Task 5); dropped with the instance.
    pub usage: HashMap<String, super::workgroup_toolbar_ui::UsageSlot>,
    /// Every command the person answered for or owns (`run::approved`): the only commands a runner starts.
    pub approved: run::Approved,
    /// The git poller, the changed-file lists and the Diff members' deferred launches (Task 6); None
    /// when no member is in Diff mode and no item needs git (`WI:930-960`).
    pub git: Option<super::workgroup_git_app::GitSide>,
    /// Code Review members' overlays, one-shot prompts and temp files (Task 7).
    pub review: super::code_review_app::ReviewSide,
}

impl Instance {
    /// The pane of the member `config` (None for a peer that has not started).
    pub fn pane_of(&self, config: &str) -> Option<PaneId> {
        self.peers.iter().find(|(c, _)| c == config).and_then(|(_, p)| *p)
    }

    /// The config id `pane` is a peer-group member as.
    pub fn config_of(&self, pane: PaneId) -> Option<&str> {
        self.peers.iter().find(|(_, p)| *p == Some(pane)).map(|(c, _)| c.as_str())
    }

    /// Whether member `config` started (a peer with a pane, or a split or tab).
    pub fn members_of(&self, config: &str) -> bool {
        self.peers.iter().any(|(k, p)| k == config && p.is_some()) || self.non_peers.iter().any(|(k, _)| k == config)
    }

    /// The peer-switch shortcut of the peer `pane` is (Session Status, `ToolStatus.peerSwitchShortcutLabel`);
    /// None for a pane that is no peer of this group, or a peer with no shortcut.
    pub fn switch_label_of(&self, pane: PaneId) -> Option<String> {
        peers::switch_label(&self.workgroup, self.config_of(pane)?)
    }

    /// The pane shown in the group's slot now.
    pub fn shown_pane(&self) -> Option<PaneId> {
        self.pane_of(&self.active)
    }

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
    /// Instances entered so far (`Instance::seq`).
    entered: u64,
    /// Members a teardown ends; closed on the next tick (the leader's own close must finish first,
    /// `WI:deferredClose`), by `tick_workgroups`.
    to_close: Vec<PaneId>,
    /// When a trigger last entered a workgroup on a session.
    recent: HashMap<PaneId, Instant>,
    /// Trigger requests to run after the pass (Task 8), each session and request once.
    trigger_due: Vec<(PaneId, TriggerWork)>,
    /// The one open pulldown (changed files, recent bases) and the one base field with the keyboard.
    pub(super) menu: Option<super::workgroup_git_app::OpenMenu>,
    pub(super) base_edit: Option<super::workgroup_git_app::BaseEdit>,
    /// The Code Review prompt manager and name sheet (egui windows) and the open Prompts pulldown.
    pub(super) review: super::code_review_app::ReviewWindows,
    /// The anchors a restore brought back, until each is adopted (Task 9).
    pub(super) restoring: super::workgroup_restore_app::Restoring,
}

impl Instances {
    pub fn get(&self, id: &str) -> Option<&Instance> {
        self.map.get(id)
    }

    pub fn get_mut(&mut self, id: &str) -> Option<&mut Instance> {
        self.map.get_mut(id)
    }

    pub fn values_mut(&mut self) -> impl Iterator<Item = &mut Instance> {
        self.map.values_mut()
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn values(&self) -> impl Iterator<Item = &Instance> {
        self.map.values()
    }

    pub fn contains(&self, id: &str) -> bool {
        self.map.contains_key(id)
    }

    /// The base of the instance `pane` leads: iTerm2's session variable gitBase, on the leader only.
    pub fn git_base_of_leader(&self, pane: PaneId) -> Option<&str> {
        self.map.values().find(|i| i.leader == pane).map(|i| i.git_base.as_str())
    }

    /// Registers an instance a restore rebuilt (its id was resolved against the live ones).
    pub fn add(&mut self, mut inst: Instance) {
        self.entered += 1;
        inst.seq = self.entered;
        self.map.insert(inst.id.clone(), inst);
    }

    /// The live instances in the order they were entered (`allInstances`).
    pub fn in_order(&self) -> Vec<&Instance> {
        let mut all: Vec<&Instance> = self.map.values().collect();
        all.sort_by_key(|i| i.seq);
        all
    }

    /// Queues a pane the next tick closes.
    pub fn close_later(&mut self, pane: PaneId) {
        if !self.to_close.contains(&pane) {
            self.to_close.push(pane);
        }
    }
}

/// What restarting a member starts.
pub(super) enum MemberLaunch {
    NotMember,
    Refused,
    Run { command: Option<String>, env: Vec<String> },
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
        let d = run::disposition(Some(&w), facts.as_ref().map(|(i, g, l)| (i.as_str(), g.as_str(), *l)), self.is_restoring_anchor(pane));
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
        let profiles = |g: Option<&str>| self.member_profile_of(g);
        let approved = run::Approved::of(&w, &profiles);
        if run::needs_ask(source, how, w.sessions.len()) {
            let Some((heading, body)) = run::ask_text(&w, how, &profiles) else {
                self.banners.set("workgroup", "This workgroup's commands are too long to show in full, so it is not started. Read them in Settings > Arrangements > Workgroups and trust it there.");
                return false;
            };
            let then = scripts_alerts::Then::EnterWorkgroup { workgroup: Box::new(w), pane, how, approved };
            let ask = scripts_alerts::ask(&heading, &body, &run::ASK_LABELS, None, crate::warnings::Kind::Persistent, then);
            self.ask_scripts(ask);
            return false;
        }
        self.enter_workgroup_now(w, pane, how, approved)
    }

    /// Why this session cannot enter a workgroup now (P6, P3): the standing refusals, then a trigger's
    /// own gap.
    fn entry_refusal(&mut self, pane: PaneId, how: Mechanism) -> Option<String> {
        if let Some(why) = self.standing_refusal(pane) {
            return Some(why);
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
    /// question carried the workgroup it showed; the saved workgroup runs only while its commands are
    /// the answered ones (`run::to_enter`).
    pub(super) fn enter_workgroup_now(&mut self, w: Workgroup, pane: PaneId, how: Mechanism, approved: run::Approved) -> bool {
        // What runs is the saved workgroup as it is now, and only when its commands are the answered ones.
        let profiles = |g: Option<&str>| self.member_profile_of(g);
        let w = match run::to_enter(&approved, &w, self.workgroup_model.get(&w.unique_identifier), &profiles) {
            Ok(w) => w,
            Err(why) => {
                self.banners.set("workgroup", why);
                return false;
            }
        };
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
            seq: 0,
            usage: HashMap::new(),
            approved,
            git: super::workgroup_git_app::GitSide::for_workgroup(&w),
            review: Default::default(),
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
            // A Diff member starts idle and waits for changes; a Code Review member starts idle and waits for Start.
            match self.spawn_member(&w, &cfg, &instance_id, pane, place, entry_tab, spawned_tabs.len(), &inst.approved) {
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
        // Every Diff member waits for changes before it runs (State A).
        let diffs: Vec<String> = w.sessions.iter().filter(|c| c.mode == Mode::Diff && inst.members_of(&c.unique_identifier)).map(|c| c.unique_identifier.clone()).collect();
        if let Some(git) = inst.git.as_mut() {
            for d in diffs {
                git.wait_for_changes(&d);
            }
        }
        // Every Code Review member waits behind its prompt overlay until Start (`PCR`).
        let reviews: Vec<String> = w.sessions.iter().filter(|c| c.mode == Mode::CodeReview && inst.members_of(&c.unique_identifier)).map(|c| c.unique_identifier.clone()).collect();
        if !reviews.is_empty() {
            self.review_entry_overlays(&mut inst, &reviews);
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
        self.workgroups.add(inst);
        self.menus_dirty = true;
        self.relayout();
        true
    }

    /// The refusals checked again at the answer: every standing one (each can change while the question
    /// is up); only a trigger's rate limit is not, since the first look counted it.
    fn entry_refusal_quiet(&self, pane: PaneId) -> Option<String> {
        self.standing_refusal(pane)
    }

    /// The refusals that hold whenever an entry is asked or answered (any can change while a question is
    /// up): a closed session, tmux, a replay or archive, a locked layout, the cap.
    fn standing_refusal(&self, pane: PaneId) -> Option<String> {
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
        None
    }

    /// The profile a member with `guid` starts with, as `spawn_member` resolves it (a missing one is
    /// the default), for the question and the approval (`run::Profiles`, final review minor 4).
    pub(super) fn member_profile_of(&self, guid: Option<&str>) -> run::MemberProfile {
        run::MemberProfile::of(&self.profile(guid.filter(|g| self.store.get(g).is_some())))
    }

    fn session_guid_of(&self, pane: PaneId) -> String {
        self.panes.get(&pane).map(|p| p.session_guid.clone()).unwrap_or_default()
    }

    /// Starts one member beside `leader`. A peer joins no layout; a split goes beside its parent at
    /// its fraction; a tab goes after the entry tab (and the tabs this entry already added).
    #[allow(clippy::too_many_arguments)]
    fn spawn_member(&mut self, w: &Workgroup, cfg: &SessionConfig, instance_id: &str, leader: PaneId, place: Place, entry_tab: usize, added_tabs: usize, approved: &run::Approved) -> Made {
        if !approved.allows(&cfg.command) {
            self.banners.set("workgroup", "A member's command was not in the list that was shown, so it did not start.");
            return Made::Skipped;
        }
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
        // A Diff member has no program until git reports changes (`WI:1316-1385`).
        let diff = matches!(cfg.mode, Mode::Diff | Mode::CodeReview);
        let command = match (diff, self.plan_command(&cfg.command, "HEAD")) {
            (true, _) => None,
            (_, Ok(c)) => c,
            (_, Err(why)) => {
                self.banners.set("workgroup", why);
                return Made::Skipped;
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
        let started = if diff {
            self.start_idle_pane(rect, guid.as_deref())
        } else if command.is_none() && self.is_browser_profile(&profile) {
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

    /// Starts a Diff or Code Review peer that was waiting when the window was saved, as entering starts
    /// it: idle, with no program, under its overlay or waiting for changes (`WRC`: a pending peer is
    /// spawned fresh). Any other member is never started by a restore.
    pub(super) fn spawn_restored_peer(&mut self, w: &Workgroup, cfg: &SessionConfig, instance_id: &str, leader: PaneId, approved: &run::Approved) -> Option<PaneId> {
        if !matches!(cfg.mode, Mode::Diff | Mode::CodeReview) {
            return None;
        }
        match self.spawn_member(w, cfg, instance_id, leader, Place::Peer, 0, 0, approved) {
            Made::Pane(id) => Some(id),
            _ => None,
        }
    }

    /// The command line a member's `template` starts with `git_base` for `\(gitBase)`: None is the
    /// profile's own shell. An error is the banner text and nothing starts (B4: values are arguments).
    pub(super) fn plan_command(&self, template: &str, git_base: &str) -> Result<Option<String>, String> {
        let path = std::env::var_os("PATH");
        run::member_line(run::command_plan(template, git_base)?, &|p| crate::semantic::resolve_on_path(p, path.as_deref()))
    }

    /// What a restart of `pane` starts when it is a workgroup member: its own command through
    /// `plan_command` (a Diff member through the Diff launcher), with the instance id for its
    /// environment. The one place a restart takes a member's command from.
    pub(super) fn workgroup_launch(&mut self, pane: PaneId) -> MemberLaunch {
        let Some((instance, config, base, template, diff, mode)) = self.workgroup_of(pane).and_then(|i| {
            let config = i.config_of(pane).map(str::to_string).or_else(|| i.non_peers.iter().find(|(_, p)| *p == pane).map(|(c, _)| c.clone()))?;
            let cfg = i.workgroup.session(&config)?;
            // A Diff member, or one with a changed-file selector, runs as the Diff launcher makes it.
            let diff = !cfg.command.trim().is_empty() && (cfg.mode == Mode::Diff || i.git.as_ref().is_some_and(|g| g.selectors.contains_key(&config)));
            Some((i.id.clone(), config, i.git_base.clone(), cfg.command.clone(), diff, cfg.mode))
        }) else {
            return MemberLaunch::NotMember;
        };
        // The command comes from the approval stored with the answer (the one list the person saw).
        if !self.workgroups.get(&instance).is_some_and(|i| i.approved.allows(&template)) {
            self.banners.set("workgroup", "That command was not in the list the workgroup was entered with, so it did not start.");
            return MemberLaunch::Refused;
        }
        if mode == Mode::CodeReview {
            return self.review_launch(&instance, &config, &template);
        }
        let line = if diff { self.diff_launch_line(&instance, &config) } else { self.plan_command(&template, &base).map(|c| (c, Vec::new())) };
        match line {
            Ok((command, mut env)) => {
                env.insert(0, format!("ITERM_WORKGROUP_ID={instance}"));
                MemberLaunch::Run { command, env }
            }
            Err(why) => {
                self.banners.set("workgroup", why);
                MemberLaunch::Refused
            }
        }
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
        self.workgroups.restoring.forget_pane(pane);
        if let Some(id) = self.panes.get(&pane).and_then(|p| p.workgroup.clone()) {
            self.teardown_workgroup(&id, Some(pane));
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
        let mut review = std::mem::take(&mut inst.review);
        self.code_review_teardown(id, &mut review);
        // A peer shown in the leader's place goes back out first, so the leader is left in its pane
        // (`WI:666-790`); a leader that is itself ending is not put back, and when the closing member
        // is the shown one its slot is closing too, so the leader goes with the group.
        let shown = inst.shown_pane().filter(|s| self.panes.contains_key(s));
        let mut leader_closes = false;
        if self.panes.contains_key(&inst.leader) {
            match run::leader_fate(skip, inst.leader, shown) {
                run::LeaderFate::SwapBack => {
                    let swapped = shown.is_some_and(|shown| self.swap_slot(shown, inst.leader));
                    // A swap that failed leaves the leader in no layout of any window: it is never left hidden.
                    // The tab list is the current window's only, so a group in a background
                    // window is looked up across windows (the leader is never closed for that).
                    leader_closes = !swapped && self.pane_window_id(inst.leader).is_none();
                }
                run::LeaderFate::Close => leader_closes = true,
                run::LeaderFate::Stay => {}
            }
        }
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
            if (p != inst.leader || leader_closes) && Some(p) != skip && !self.workgroups.to_close.contains(&p) {
                self.workgroups.to_close.push(p);
            }
        }
        // The leader's own name, and the entry tab's own group.
        if skip != Some(inst.leader) && self.panes.contains_key(&inst.leader) {
            if let Some(name) = inst.leader_title.take() {
                self.set_session_values(inst.leader, &[(crate::profile::NAME.to_string(), Some(toml::Value::String(name)))]);
            }
            if let Some(before) = inst.group_before.take() {
                // The leader's tab may sit in another window now (the shown member was moved, or its
                // tab torn off): the group goes back where the leader is.
                let home = self.current_window();
                if self.enter_pane_window(inst.leader) {
                    if let Some(at) = self.tabs.iter().position(|t| t.layout.panes().contains(&inst.leader)) {
                        let gid = before.as_ref().map(|g| g.id.clone());
                        self.tabs[at].group = before;
                        self.set_tab_group(at, gid);
                        self.group_changed();
                    }
                }
                if let Some(h) = home.filter(|h| self.current_window() != Some(*h)) {
                    self.enter(h);
                }
            }
        }
        self.workgroups.recent.remove(&inst.leader);
        self.menus_dirty = true;
        crate::rlog!("workgroup exited: instance {}", crate::dlog::name(id));
    }

    /// Holds a trigger's Enter or Exit Workgroup until the pass is over (`EWT:performAction`, the
    /// trigger callback scheduler). One of each per session, and a short queue.
    pub(super) fn queue_workgroup_trigger(&mut self, pane: PaneId, work: TriggerWork) {
        let due = &mut self.workgroups.trigger_due;
        if due.len() < MAX_TRIGGER_QUEUE && !due.iter().any(|(p, w)| *p == pane && *w == work) {
            due.push((pane, work));
        }
    }

    /// The workgroup a queued Enter may ask about, or None. Enter is a no-op in a session a restore is
    /// rebuilding (the flag is read when the trigger fires, `PS:25381-25390`), refused (and logged) in
    /// a session already in a workgroup or where entering would not go ahead (`BVC:1666-1673`), not
    /// asked again after a Cancel for this session (not every 5 s), and not asked while a question for
    /// this very entry is up or waiting.
    fn trigger_enter_id(&mut self, pane: PaneId, param: &str, what: &'static str) -> Option<String> {
        if self.is_restoring_session(pane) {
            return None;
        }
        let list: Vec<(String, String)> = self.workgroup_model.workgroups.iter().map(|w| (w.unique_identifier.clone(), w.name.clone())).collect();
        let id = crate::triggers::effective_workgroup(param, &list)?;
        let in_group = self.workgroup_of(pane).is_some();
        let d = run::disposition(self.workgroup_model.get(&id), None, self.is_restoring_anchor(pane));
        if !run::can_enter_from_ui(in_group, &d) {
            let guid = self.panes.get(&pane).map(|p| p.session_guid.clone()).unwrap_or_default();
            crate::rlog!("{}: refused for workgroup {} on session {}", what, crate::dlog::name(&id), crate::dlog::name(&guid));
            return None;
        }
        if self.workgroups.restoring.declined(pane, &id) {
            return None;
        }
        let pending = scripts_alerts::waits(self.scripts_alert.as_ref().map(|(_, a)| a), &self.scripts_alerts, |t| {
            matches!(t, scripts_alerts::Then::EnterWorkgroup { workgroup, pane: p, .. } if *p == pane && workgroup.unique_identifier == id)
        });
        (!pending).then_some(id)
    }

    /// The queued triggers, after the pass. Enter is a no-op in a session that is already in a
    /// workgroup (iTerm2: the menu disables itself there too) and never runs a command unasked:
    /// `enter_workgroup` asks for every trigger (`needs_ask`) and the answer enters. Exit leaves
    /// from any member, or, leader-only, from the leader alone (`PS:triggerSideEffectExitWorkgroupLeaderOnly:`).
    fn run_workgroup_triggers(&mut self) {
        for (pane, work) in std::mem::take(&mut self.workgroups.trigger_due) {
            if !self.panes.contains_key(&pane) {
                continue;
            }
            match work {
                TriggerWork::Enter(param) => {
                    let Some(id) = self.trigger_enter_id(pane, &param, "enterWorkgroupTrigger") else { continue };
                    self.enter_workgroup(&id, pane, Mechanism::Trigger);
                }
                TriggerWork::EnterBrowser(param) => {
                    let Some(id) = self.trigger_enter_id(pane, &param, "browserTriggerEnterWorkgroup") else { continue };
                    self.enter_workgroup(&id, pane, Mechanism::BrowserTrigger);
                }
                TriggerWork::ExitBrowser => {
                    if self.workgroup_of(pane).is_some() {
                        self.exit_workgroup(pane);
                    }
                }
                TriggerWork::Exit(leader_only) => {
                    let Some(inst) = self.workgroup_of(pane) else { continue };
                    if leader_only && inst.leader != pane {
                        continue;
                    }
                    self.exit_workgroup(pane);
                }
            }
        }
    }

    /// Ends the members a teardown queued (once the closing pane's own close has finished).
    pub(super) fn tick_workgroups(&mut self, event_loop: &ActiveEventLoop) -> Option<Instant> {
        self.run_workgroup_triggers();
        self.usage_tick();
        let restore_due = self.adopt_workgroups();
        let due = [self.workgroup_git_tick(), restore_due].into_iter().flatten().min();
        if self.workgroups.to_close.is_empty() {
            return due;
        }
        for id in std::mem::take(&mut self.workgroups.to_close) {
            if self.panes.contains_key(&id) {
                self.close_pane(id, event_loop);
            }
        }
        self.redraw();
        due
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

    // --- Peers (Task 4): `PSPP` `PTYSessionPeerPort.swift`, `WPP` `iTermWorkgroupPeerPort.swift` ---

    /// The instance `pane` is the shown member of: the only pane the peer commands act on.
    pub(super) fn peer_group_of(&self, pane: PaneId) -> Option<&Instance> {
        self.workgroup_of(pane).filter(|i| i.shown_pane() == Some(pane))
    }

    /// Whether Previous Peer and Next Peer are enabled for `pane` (`PTYTextView.m:555-560`).
    pub(super) fn peer_menu_enabled(&self, pane: Option<PaneId>) -> bool {
        pane.and_then(|p| self.peer_group_of(p)).is_some_and(|i| peers::menu_enabled(i.peers.len()))
    }

    /// Puts `to` in the layout slot `from` holds (the window that shows `from` is entered for the
    /// change and the window that was current is current again after). Focus and a zoom follow only
    /// when `from` had them, and the group's clippings go with the slot (they are the leader's, and
    /// whichever member is shown carries them, `PSPP:30-55`). The pty is resized by `relayout`.
    fn swap_slot(&mut self, from: PaneId, to: PaneId) -> bool {
        let home = self.current_window();
        if !self.enter_pane_window(from) {
            return false;
        }
        let swapped = match self.tabs.iter().position(|t| t.layout.panes().contains(&from)) {
            Some(t) if self.tabs[t].layout.replace(from, to) => {
                let tab = &mut self.tabs[t];
                if tab.focused == from {
                    tab.focused = to;
                }
                if tab.zoomed == Some(from) {
                    tab.zoomed = Some(to);
                }
                true
            }
            _ => false,
        };
        if swapped {
            if let Some(state) = self.clips.remove(&from) {
                self.clips.insert(to, state);
            } else {
                self.clips.remove(&to);
            }
            self.relayout();
        }
        if let Some(h) = home.filter(|h| self.current_window() != Some(*h)) {
            self.enter(h);
        }
        swapped
    }

    /// Whether member `config` of `inst` can be shown now: its session exists (a peer that has not
    /// started is not yet, Tasks 6 and 7 start theirs on activation).
    fn peer_activatable(&self, inst: &Instance, config: &str) -> bool {
        inst.pane_of(config).is_some_and(|p| self.panes.contains_key(&p))
    }

    /// Shows member `id` of `instance` in the group's slot: the shown member goes out of the layout
    /// alive and hidden, the target takes its place (`Layout::replace`), and the toolbar follows
    /// with the slot. Never activates another window or changes the selected tab. False when the
    /// member cannot be shown; the shown member stays.
    pub(super) fn activate_peer(&mut self, instance: &str, id: &str) -> bool {
        let Some(inst) = self.workgroups.get(instance) else { return false };
        if inst.active == id {
            return inst.shown_pane().is_some();
        }
        if !self.peer_activatable(inst, id) {
            return false;
        }
        let (Some(target), Some(shown)) = (inst.pane_of(id), inst.shown_pane()) else { return false };
        if !self.panes.contains_key(&shown) || !self.swap_slot(shown, target) {
            return false;
        }
        if let Some(i) = self.workgroups.map.get_mut(instance) {
            i.active = id.to_string();
        }
        self.menus_dirty = true;
        self.redraw();
        true
    }

    /// Previous (-1) and Next (1) Peer for the group `pane` shows (`WPP:543-575`).
    pub(super) fn peer_offset(&mut self, pane: PaneId, offset: i32) -> bool {
        let Some(inst) = self.peer_group_of(pane) else { return false };
        let ids: Vec<String> = inst.peers.iter().map(|(c, _)| c.clone()).collect();
        let (instance, active) = (inst.id.clone(), inst.active.clone());
        let Some(target) = peers::by_offset(&ids, &active, offset, &|c| self.peer_activatable(inst, c)) else { return false };
        self.activate_peer(&instance, &target)
    }

    /// A peer's own shortcut pressed in the group `pane` shows. True when the key is the group's
    /// (consumed even when that member cannot be shown: it must not reach the shell, `WPP:466-480`).
    pub(super) fn peer_shortcut(&mut self, pane: PaneId, key: &crate::keystroke::Keystroke) -> bool {
        let Some(inst) = self.peer_group_of(pane) else { return false };
        let hit = peers::members(&inst.workgroup).into_iter().find(|(_, _, s)| s.as_ref().is_some_and(|s| peers::shortcut_matches(s, key)));
        let Some((config, _, _)) = hit else { return false };
        let instance = inst.id.clone();
        self.activate_peer(&instance, &config);
        true
    }

    /// Alt+Shift+Win+`digit` pressed in the group `pane` shows (`WPP:461-485`). True when the digit
    /// names a member (a member that cannot be shown consumes it).
    pub(super) fn peer_digit(&mut self, pane: PaneId, digit: u32) -> bool {
        let Some(inst) = self.peer_group_of(pane) else { return false };
        let Some(at) = peers::by_digit(inst.peers.len(), digit) else { return false };
        let (instance, config) = (inst.id.clone(), inst.peers[at].0.clone());
        self.activate_peer(&instance, &config);
        true
    }

    /// The keys a workgroup takes after global bindings and before the toolbar and the menu: a
    /// peer's custom shortcut, then Alt+Shift+Win+digit (`WI:1689-1790`). True when consumed.
    pub(super) fn peer_key(&mut self, stroke: &crate::keystroke::Keystroke, chord: Option<&crate::menus::Chord>) -> bool {
        let Some(pane) = self.focused_id() else { return false };
        if self.peer_group_of(pane).is_none() {
            return false;
        }
        if self.peer_shortcut(pane, stroke) {
            return true;
        }
        chord.and_then(peers::chord_digit).is_some_and(|d| self.peer_digit(pane, d))
    }

    /// Shows `pane` in its group's slot when it is a hidden peer, without ordering its window front
    /// or changing the selected tab (`PS:16451-16480`).
    pub(super) fn reveal_as_peer(&mut self, pane: PaneId) {
        let Some((instance, config)) = self.workgroup_of(pane).filter(|i| i.shown_pane() != Some(pane)).and_then(|i| i.config_of(pane).map(|c| (i.id.clone(), c.to_string()))) else { return };
        self.activate_peer(&instance, &config);
    }

    /// The pane whose `ClipState` is the group's clippings now: the member shown in the slot.
    pub(super) fn clip_home(&self, pane: PaneId) -> PaneId {
        let inst = self.workgroup_of(pane);
        let shown = inst.and_then(|i| i.shown_pane()).filter(|p| self.panes.contains_key(p));
        peers::clip_home(inst.is_some_and(|i| i.config_of(pane).is_some()), shown, pane)
    }

    /// What the keyboard hook must take for peers: whether any group has more than one peer, and the
    /// forms of every custom shortcut that has Win in it.
    pub(super) fn peer_route_keys(&self) -> (bool, std::collections::HashSet<String>) {
        let mut keys = std::collections::HashSet::new();
        let mut digits = false;
        for inst in self.workgroups.values().filter(|i| peers::menu_enabled(i.peers.len())) {
            digits = true;
            for (_, _, s) in peers::members(&inst.workgroup) {
                if let Some(k) = s.and_then(|s| crate::keystroke::parse(&s.serialized)).filter(|k| k.flags & crate::keystroke::COMMAND != 0) {
                    keys.insert(k.legacy());
                    keys.insert(k.modified_key());
                }
            }
        }
        (digits, keys)
    }

    /// Shell > Workgroups: the workgroups above the separator, checked for the current one, and
    /// Exit Workgroup enabled in a workgroup. The separator is out while there are none.
    pub(super) fn refresh_workgroups_menu(&mut self) {
        let focused = self.tabs.get(self.active).map(|t| t.focused).filter(|p| self.panes.contains_key(p));
        let facts = focused.and_then(|p| self.workgroup_facts(p));
        let entries = run::menu_entries(&self.workgroup_model.workgroups, facts.as_ref().map(|(i, g, l)| (i.as_str(), g.as_str(), *l)), focused.is_some(), false);
        let in_workgroup = facts.is_some();
        let peers_on = self.peer_menu_enabled(focused);
        let Some(bar) = &mut self.menu_bar else { return };
        for item in &bar.peer_items {
            item.set_enabled(peers_on);
        }
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

// --- The Cockpit, orchestration and status (Task 10): the facts they read ---

impl Instance {
    /// Every live member: (config id, pane), the leader first, then the peers that started, then
    /// the splits and tabs in the order they started.
    pub fn member_panes(&self) -> Vec<(String, PaneId)> {
        let mut out: Vec<(String, PaneId)> = Vec::new();
        for (c, p) in self.peers.iter().filter_map(|(c, p)| p.map(|p| (c.clone(), p))).chain(self.non_peers.iter().cloned()) {
            if !out.iter().any(|(_, q)| *q == p) {
                out.push((c, p));
            }
        }
        out
    }
}

impl App {
    /// The instance and the config of the member `pane` is.
    pub(super) fn member_of(&self, pane: PaneId) -> Option<(&Instance, &SessionConfig)> {
        let (instance, config) = self.member_config(pane)?;
        let inst = self.workgroups.get(&instance)?;
        Some((inst, inst.workgroup.session(&config)?))
    }

    /// The role a member's title carries (`cockpitSessionRolePrefix`).
    pub(super) fn member_role_prefix(&self, pane: PaneId) -> Option<String> {
        let (_, cfg) = self.member_of(pane)?;
        crate::cockpit_rows::role_prefix(&cfg.kind, cfg.mode, &cfg.display_name)
    }

    /// The scope a write to `pane` claims under: its instance for a member (approving a workgroup
    /// once approves all its roles, `claimScope(forSessionGuid:)`), else its own `session:` scope.
    pub(super) fn claim_scope_of(&self, pane: PaneId) -> String {
        match self.workgroup_of(pane) {
            Some(i) => i.id.clone(),
            None => crate::orch_tools::session_scope(&self.panes.get(&pane).map(|p| p.stable_id.clone()).unwrap_or_default()),
        }
    }

    /// The live instances, in the order they were entered, with their live members (a member whose
    /// pane is gone is left out).
    pub(super) fn workgroup_world(&self) -> Vec<crate::cockpit_rows::WorkgroupFact> {
        use crate::cockpit_rows::{MemberFact, WorkgroupFact};
        self.workgroups
            .in_order()
            .into_iter()
            .map(|i| WorkgroupFact {
                id: i.id.clone(),
                name: i.workgroup.name.clone(),
                members: i
                    .member_panes()
                    .into_iter()
                    .filter_map(|(config, pane)| {
                        let p = self.panes.get(&pane)?;
                        let cfg = i.workgroup.session(&config)?;
                        Some(MemberFact { pane, stable_id: p.stable_id.clone(), role: cfg.display_name.clone(), mode: cfg.mode, config })
                    })
                    .collect(),
            })
            .collect()
    }

    /// The pane whose `ClipState` holds the clippings of the workgroup `id` (an instance id, `wg-`):
    /// the leader's group, which the shown member carries (`WorkgroupIntrospection.swift:366-389`).
    /// None for an id no live instance has.
    pub(super) fn workgroup_clippings(&self, id: &str) -> Option<PaneId> {
        if !id.starts_with(run::INSTANCE_PREFIX) {
            return None;
        }
        let inst = self.workgroups.get(id).filter(|i| self.panes.contains_key(&i.leader))?;
        Some(self.clip_home(inst.leader))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inst(id: &str, leader: PaneId) -> Instance {
        let w = crate::workgroup::presets::coding_agent_plus_diff_plus_code_review(Some(("builtin.claudeCode", "MAIN", "DIFF", "REVIEW")), Some("Claude Code"));
        Instance {
            id: id.into(),
            leader,
            peers: vec![("MAIN".into(), Some(leader)), ("DIFF".into(), Some(leader + 1)), ("REVIEW".into(), None)],
            active: "MAIN".into(),
            non_peers: vec![("TAB".into(), leader + 2), ("DUP".into(), leader)],
            git_base: "HEAD".into(),
            provenance: String::new(),
            leader_title: None,
            group_before: None,
            torn_down: false,
            seq: 0,
            usage: HashMap::new(),
            approved: run::Approved::of(&w, &|_| run::MemberProfile::default()),
            git: None,
            review: Default::default(),
            workgroup: w,
        }
    }

    /// The Cockpit and the snapshot list instances in the order they were entered, and a member once.
    #[test]
    fn instances_list_in_entry_order() {
        let mut all = Instances::default();
        for (id, leader) in [("wg-Z", 10), ("wg-A", 20), ("wg-M", 30)] {
            all.add(inst(id, leader));
        }
        assert_eq!(all.in_order().iter().map(|i| i.id.as_str()).collect::<Vec<_>>(), ["wg-Z", "wg-A", "wg-M"], "entry order, not id order");
        let first = all.get("wg-Z").unwrap();
        assert_eq!(first.member_panes(), vec![("MAIN".to_string(), 10), ("DIFF".to_string(), 11), ("TAB".to_string(), 12)], "the leader first, a peer that has not started left out, a pane once");
        // Restoring an instance again gives it a later place.
        all.add(inst("wg-Z", 40));
        assert_eq!(all.in_order().last().map(|i| i.id.as_str()), Some("wg-Z"));
    }

    /// Final review minor 8: the answer to an entry question meets the same refusals as the question did
    /// (tmux, a replay or archive, the layout lock, the cap), any of which can change while it is up.
    #[test]
    fn the_answer_rechecks_every_entry_refusal() {
        let live = include_str!("workgroup_app.rs").split("#[cfg(test)]").next().unwrap();
        let quiet = &live[live.find("fn entry_refusal_quiet(").unwrap()..];
        let quiet = &quiet[..quiet.find("\n    }\n").unwrap()];
        for needed in ["tmux_io()", "in_instant_replay()", "layout_allowed_quiet(", "MAX_INSTANCES"] {
            assert!(quiet.contains(needed) || (quiet.contains("self.standing_refusal(pane)") && live.contains(needed)), "{needed}: {quiet}");
        }
        assert!(quiet.contains("self.standing_refusal(pane)"), "{quiet}");
        let asked = &live[live.find("fn entry_refusal(").unwrap()..];
        assert!(asked[..asked.find("\n    }\n").unwrap()].contains("self.standing_refusal(pane)"), "one list of refusals");
    }

    /// Final review 3a: Session Status labels a peer of the focused session's group with its peer-switch
    /// shortcut (`ToolStatus.shortcutString`, `peerSwitchShortcutLabel`), and any other hidden peer with the
    /// shortcut of the member shown in its slot, never with a tab or pane of its own.
    #[test]
    fn a_hidden_peers_status_row_names_its_own_shortcut() {
        let i = inst("wg-A", 10);
        assert_eq!(i.switch_label_of(10).as_deref(), peers::digit_label(1).as_deref(), "the leader is peer 1");
        assert_eq!(i.switch_label_of(11).as_deref(), peers::digit_label(2).as_deref(), "Diff is peer 2");
        assert_eq!(i.switch_label_of(12), None, "a tab is no peer");
        let ui = include_str!("toolbelt_more_ui.rs");
        let rows = &ui[ui.find("pub(super) fn status_rows(").unwrap()..ui.find("/// What Session Status shows").unwrap()];
        assert!(rows.contains("focus_group.and_then(|g| g.switch_label_of(id))"), "the focused group's peers show their peer label: {rows}");
        assert!(rows.contains("ordinal_of(&ordered, visible)"), "a hidden peer takes the shown member's place: {rows}");
    }

    /// Final review 3b: the base is iTerm2's built-in session variable gitBase on the leader, read from
    /// the live instance (so it is gone when the instance is, and a failed entry leaves none); no user
    /// variable is written for it.
    #[test]
    fn the_base_is_the_leaders_git_base_variable() {
        let mut all = Instances::default();
        let mut i = inst("wg-A", 10);
        i.git_base = "main".into();
        all.add(i);
        assert_eq!(all.git_base_of_leader(10), Some("main"));
        assert_eq!(all.git_base_of_leader(11), None, "only the leader carries it");
        assert_eq!(all.git_base_of_leader(99), None);
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        for dir in [root.clone(), root.join("app")] {
            for e in std::fs::read_dir(&dir).unwrap().flatten() {
                let text = std::fs::read_to_string(e.path()).unwrap_or_default();
                let live = text.split("#[cfg(test)]").next().unwrap();
                assert!(!live.contains("\"user.gitBase\""), "{}", e.path().display());
            }
        }
        let vars = include_str!("prompted_ui.rs");
        assert!(vars.contains("if let Some(base) = self.workgroups.git_base_of_leader(pane) {\n            v.session.insert(\"gitBase\", base.to_string());"), "the session variable comes from the live instance");
    }

    /// The facts the Cockpit, the snapshot and the clipping tools read come from one place.
    #[test]
    fn the_facts_are_wired() {
        let app = include_str!("workgroup_app.rs");
        let world = &app[app.find("pub(super) fn workgroup_world(").unwrap()..];
        assert!(world.contains("self.workgroups.in_order()") && world.contains("member_panes()"));
        let cockpit = include_str!("cockpit_app.rs");
        assert!(cockpit.contains("world.workgroups = self.workgroup_world()") && cockpit.contains("role: self.member_role_prefix(pane)"));
        let watch = include_str!("orch_watch_app.rs");
        assert!(watch.contains("self.orch_identity(&target)"), "watchers carry the real workgroup id and role");
    }

    /// A swap-back that worked never closes the leader, even when the group is in a background window
    /// (`self.tabs` is the current window's tabs only).
    #[test]
    fn swap_back_never_reads_the_current_windows_tabs() {
        let src = include_str!("workgroup_app.rs");
        let arm = &src[src.find("run::LeaderFate::SwapBack =>").unwrap()..src.find("run::LeaderFate::Close =>").unwrap()];
        assert!(!arm.contains("self.tabs"), "{arm}");
        assert!(arm.contains("leader_closes = !swapped && self.pane_window_id(inst.leader).is_none();"), "{arm}");
    }
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

    /// Every pane close reaches the teardown; the menu pick, the exit command and the close warning are wired.
    #[test]
    fn the_hooks_are_wired() {
        assert!(MOD.contains("self.workgroup_member_ended(id);"));
        // A program ending is not a termination (`iTermSessionWillTerminate` is the close): a Diff member's
        // command ends all the time and the group goes on.
        assert!(!include_str!("session_end_ui.rs").contains("workgroup_member_exited"));
        assert!(include_str!("menu.rs").contains("\"exitWorkgroup:\" =>"));
        assert!(include_str!("menu.rs").contains("self.workgroup_picked(id)"));
        assert!(include_str!("close_ui.rs").contains("self.workgroup_close_warning("));
        assert!(include_str!("scripts_alerts.rs").contains("Then::EnterWorkgroup { workgroup, pane, how, approved } =>"));
    }

    /// Task 8: a trigger only queues; the queue runs after the pass, an Enter never skips the question
    /// (`enter_workgroup` asks for every trigger), and an Exit never runs from outside a workgroup.
    #[test]
    fn workgroup_triggers_queue_and_ask() {
        let production = THIS.split("#[cfg(test)]").next().unwrap();
        let queue = &production[production.find("fn run_workgroup_triggers").unwrap()..production.find("/// Ends the members a teardown queued").unwrap()];
        assert!(queue.contains("self.enter_workgroup(&id, pane, Mechanism::Trigger)"), "{queue}");
        assert!(!queue.contains("enter_workgroup_now"), "a trigger never enters unasked");
        assert!(queue.contains("inst.leader != pane"), "leader-only exits only from the leader");
        assert!(queue.contains("self.workgroup_of(pane).is_some()"), "Enter is a no-op in a session already in a workgroup");
        let ui = include_str!("triggers_ui.rs");
        assert!(ui.contains("self.queue_workgroup_trigger(pane, super::workgroup_app::TriggerWork::Enter(param))"));
        assert!(!ui.contains("enter_workgroup("), "the pass itself never enters");
        assert!(production.contains("run_workgroup_triggers();\n        self.usage_tick();"), "after the pass, in the workgroup tick");
        // An imported or dynamic profile's trigger of these classes arrives off (13w).
        assert!(crate::triggers::IMPORT_OFF.contains(&"iTermEnterWorkgroupTrigger") && crate::triggers::IMPORT_OFF.contains(&"iTermExitWorkgroupTrigger"));
        // Any entry from a trigger asks: the trust ruling.
        assert!(crate::workgroup_run::needs_ask(crate::workgroup::Source::Person, crate::workgroup_run::Mechanism::Trigger, 1));
    }

    /// The peer swap: only `Layout::replace`, never a close or a reopen of layout space, never the
    /// buried store; the shown member's slot, focus and clippings travel; the teardown puts the leader back.
    #[test]
    fn peers_swap_in_place() {
        let production = THIS.split("#[cfg(test)]").next().unwrap();
        assert!(production.contains("layout.replace(from, to)"));
        assert!(production.contains("self.swap_slot(shown, inst.leader)"), "the teardown puts the leader back before the peers end");
        assert!(production.contains("self.clips.insert(to, state)"), "the group's clippings follow the slot");
        assert!(!production.contains("select_tab") && !production.contains("bring_forward"), "a swap never changes the selected tab or the window");
        assert!(include_str!("alerts_ui.rs").contains("self.reveal_as_peer(pane);"), "reveal shows a hidden peer first");
        assert!(MOD.contains("if self.peer_key(&press.stroke, chord) {"), "peer keys come after the global bindings");
        assert!(MOD.contains("(snapshot.peer_digits, snapshot.peer_keys) = self.peer_route_keys();"), "the hook takes the peers' chords");
    }

    /// `previousWorkgroupPeer:` and `nextWorkgroupPeer:` start disabled and the menu refresh enables them.
    #[test]
    fn peer_menu_follows_iterm2() {
        use crate::workgroup_peers::menu_enabled;
        // A pane in no workgroup has no peer group, a workgroup with one peer cannot cycle, two can.
        assert!(!menu_enabled(0) && !menu_enabled(1) && menu_enabled(2));
        let menu = include_str!("menu.rs");
        assert!(menu.contains("&& !matches!(item.selector.as_str(), \"previousWorkgroupPeer:\" | \"nextWorkgroupPeer:\")"), "disabled until a group is focused");
        assert!(THIS.contains("self.peer_menu_enabled(focused)"));
        assert!(THIS.contains("peers::menu_enabled(i.peers.len())"));
        assert!(menu.contains("self.peer_offset(id, if item.selector == \"nextWorkgroupPeer:\" { 1 } else { -1 })"));
        // The shortcuts stay Alt+Win+[ and ].
        for (selector, key) in [("previousWorkgroupPeer:", "["), ("nextWorkgroupPeer:", "]")] {
            let item = crate::menus::items().iter().find(|i| i.selector == selector).expect(selector);
            assert_eq!((item.key.as_str(), item.mods.clone()), (key, vec!["opt".to_string(), "cmd".to_string()]));
            assert!(!crate::commands::status(item).unavailable(), "{selector} is built");
        }
    }
}
