//! Restoring workgroups on the app side (milestone 13y1, Task 9): the shown member's session value
//! carries a descriptor, a restore registers the anchors it brings back, and `adopt_workgroups`
//! rebuilds each instance once its anchor is in a window. The pure side is `workgroup_restore`.
//!
//! Restoration is data, not execution (P9): nothing here starts a program. A restored Diff or Code
//! Review session with no program to take back is an idle session (its last output stays); a peer that
//! never started comes back waiting or asking; a regular peer runs only the program the session host
//! kept for it. An import's commands run only while the answer the person gave at entry still stands
//! (`needs_question`), otherwise the person is asked again before anything is rebuilt.
//!
//! Citations: `WR` `iTermWorkgroupRestoration.swift`, `WRC` `WorkgroupRestorationCoordinator.swift`,
//! `WC` `iTermWorkgroupController.swift`, `WI` `iTermWorkgroupInstance.swift` under
//! `~/gitrepos/iterm2-ref/sources/Workgroups`.

use super::arrange_ui::Contents;
use super::workgroup_app::Instance;
use super::*;
use crate::arrangement::Value;
use crate::workgroup::{Mode, Workgroup};
use crate::workgroup_restore as wr;
use crate::workgroup_run::{self as run, Approved, Mechanism, SpawnStep};
use std::collections::{HashMap, HashSet};

/// Restored anchors waiting at most (a file cannot queue work without end).
const MAX_PENDING: usize = 16;
/// An anchor not in a window this long stops blocking Enter Workgroup (`pendingReconstructionTimeout`,
/// a soft limit: the entry is kept), and is given up on after `GIVE_UP`.
const RELEASE_AFTER: Duration = Duration::from_secs(30);
const GIVE_UP: Duration = Duration::from_secs(120);
/// How soon the next look comes while anchors wait for their window.
const LOOK: Duration = Duration::from_millis(250);
/// Toggles and declined entries remembered at most.
const MAX_NOTES: usize = 64;

/// One restored anchor, waiting for its window (`WRC.Pending`): what the restore knows of it, and the
/// descriptor itself (taken whole when it is adopted or given up).
pub(super) struct Pending {
    m: Meta,
    descriptor: wr::Descriptor,
}

struct Meta {
    token: u64,
    anchor: PaneId,
    /// The saved arrangement's name (None: window restoration or a file), for the members' own build.
    name: Option<String>,
    /// Launch-time restoration of the saved state: members adopt their saved references.
    launch: bool,
    /// Every session this restore involves (the anchor's, the buried members' and the children's
    /// GUIDs); the Enter Workgroup trigger is a no-op in them until the rebuild is over (`WRC.guids`).
    guids: Vec<String>,
    since: Instant,
    released: bool,
    /// The person was asked again; the answer decides (`restore_answered`).
    asked: bool,
}

/// What a restore holds: the anchors, the GUIDs they block, the toggles the sessions saved and the
/// triggers' declined entries. App-wide, in `Instances` (no per-window field).
#[derive(Default)]
pub(super) struct Restoring {
    pending: Vec<Pending>,
    next_token: u64,
    /// GUID -> how many pending restores claim it (`restoringGUIDCounts`).
    guids: HashMap<String, usize>,
    /// Toggles a restored session saved, by pane, until its adoption takes them or it is gone.
    toggles: HashMap<PaneId, (bool, bool)>,
    /// The next restored session starts with no program (a buried Code Review or Diff member).
    inhibit_next: bool,
    /// An Enter Workgroup question a person cancelled: not asked again for this session and workgroup.
    declined: HashSet<(PaneId, String)>,
}

impl Restoring {
    fn release(&mut self, p: &mut Meta) {
        if std::mem::replace(&mut p.released, true) {
            return;
        }
        for g in &p.guids {
            match self.guids.get_mut(g) {
                Some(n) if *n > 1 => *n -= 1,
                _ => {
                    self.guids.remove(g);
                }
            }
        }
    }

    /// Registers an anchor's restore (`WRC.register`): its GUIDs are claimed, counted, so two restores
    /// that claim one GUID release it only when both are over. False when too many wait.
    fn add(&mut self, anchor: PaneId, own_guid: String, d: wr::Descriptor, name: Option<&str>, launch: bool) -> bool {
        if self.pending.len() >= MAX_PENDING {
            return false;
        }
        let mut guids = vec![own_guid];
        guids.extend(d.member_guids.iter().cloned());
        guids.extend(d.non_peer_children.iter().map(|c| c.guid.clone()));
        for g in &guids {
            *self.guids.entry(g.clone()).or_insert(0) += 1;
        }
        self.next_token += 1;
        let token = self.next_token;
        self.pending.push(Pending { m: Meta { token, anchor, name: name.map(str::to_string), launch, guids, since: Instant::now(), released: false, asked: false }, descriptor: d });
        true
    }

    /// Drops every note about a pane that is gone.
    pub(super) fn forget_pane(&mut self, pane: PaneId) {
        self.toggles.remove(&pane);
        self.declined.retain(|(p, _)| *p != pane);
    }

    /// Remembers that the person said no to this entry.
    pub(super) fn decline(&mut self, pane: PaneId, workgroup: &str) {
        if self.declined.len() < MAX_NOTES {
            self.declined.insert((pane, workgroup.to_string()));
        }
    }

    pub(super) fn declined(&self, pane: PaneId, workgroup: &str) -> bool {
        self.declined.contains(&(pane, workgroup.to_string()))
    }
}

impl App {
    /// Whether `pane` is the anchor of a restore that has not finished (`isRestoringAnchor`): entering
    /// a workgroup there would race the rebuild.
    pub(super) fn is_restoring_anchor(&self, pane: PaneId) -> bool {
        self.workgroups.restoring.pending.iter().any(|p| p.m.anchor == pane && !p.m.released)
    }

    /// Whether `pane`'s session belongs to a workgroup that is being restored (`isRestoring(guid:)`):
    /// the Enter Workgroup trigger fires on replayed output and must not start a second workgroup.
    pub(super) fn is_restoring_session(&self, pane: PaneId) -> bool {
        self.panes.get(&pane).is_some_and(|p| self.workgroups.restoring.guids.contains_key(&p.session_guid))
    }

    // --- Saving ---

    /// The descriptor the shown member of a peer group saves (`WR:encodeState`): the buried members as
    /// their own session values, the non-peer children by config id and GUID. None for any other pane
    /// and for an archive. `contents` is the one the snapshot saves, so a member's text is kept as the
    /// anchor's is.
    pub(super) fn workgroup_descriptor(&self, pane: PaneId, contents: Contents) -> Option<Value> {
        if matches!(contents, Contents::Archive) {
            return None;
        }
        let inst = self.workgroup_of(pane)?;
        if inst.shown_pane() != Some(pane) {
            return None;
        }
        let anchor = inst.config_of(pane)?.to_string();
        let w = &inst.workgroup;
        let area = self.content_area();
        let frame = format!("{{{{0, 0}}, {{{}, {}}}}}", area.w.round() as i64, area.h.round() as i64);
        // A member still on its pre-launch overlay (a Code Review prompt, a Diff waiting for changes).
        let pending = |c: &str| inst.review.overlays.contains_key(c) || inst.git.as_ref().is_some_and(|g| g.pending.contains_key(c));
        let mut members = Vec::new();
        let mut member_guids = Vec::new();
        for (config, p) in &inst.peers {
            let Some(p) = p.filter(|p| self.panes.contains_key(p)) else { continue };
            if *config == anchor {
                continue;
            }
            let waiting = pending(config);
            // A waiting member has no content worth saving: it comes back fresh.
            let arrangement = if waiting { Value::Table(Default::default()) } else { self.session_value_for_restore(p, contents) };
            members.push(wr::Member { config_id: config.clone(), display_name: w.session(config).map(|s| s.display_name.clone()).unwrap_or_default(), frame: frame.clone(), arrangement, pending: waiting });
            member_guids.push(self.panes[&p].session_guid.clone());
        }
        let non_peer_children = inst.non_peers.iter().filter_map(|(c, p)| Some(wr::Child { config_id: c.clone(), guid: self.panes.get(p)?.session_guid.clone() })).collect();
        let digest = inst.approved.digest();
        Some(
            wr::Descriptor {
                workgroup_id: w.unique_identifier.clone(),
                instance_id: inst.id.clone(),
                git_base: inst.git_base.clone(),
                leader_id: w.root()?.unique_identifier.clone(),
                anchor_id: anchor.clone(),
                members,
                member_guids,
                non_peer_children,
                anchor_pending: pending(&anchor),
                approval: (!digest.is_empty()).then(|| digest.to_string()),
            }
            .to_value(),
        )
    }

    // --- Restoring ---

    /// The descriptor a restored session saved, when this restore may use it: window restoration at
    /// launch (our own state file) and a saved arrangement (the person's settings). A file opened from
    /// elsewhere carries a descriptor that is never read: its embedded sessions would start programs the
    /// open question never named.
    pub(super) fn restored_descriptor(&self, session: &Value, launch: bool, named: bool) -> Option<wr::Descriptor> {
        if !(launch || named) {
            return None;
        }
        wr::Descriptor::from_value(session.get(crate::arrangement::WORKGROUP_KEY)?)
    }

    /// Whether the restored anchor starts no program of its own (`shouldInhibitRelaunch`): its workgroup
    /// still exists, its mode is Code Review or Diff and no program came back from the session host.
    /// The next session built for a buried member asks the same through `inhibit_next`.
    pub(super) fn restore_inhibits(&mut self, d: Option<&wr::Descriptor>, had_program: bool) -> bool {
        let member = std::mem::take(&mut self.workgroups.restoring.inhibit_next);
        let anchor = d.and_then(|d| Some((d, self.workgroup_model.get(&d.workgroup_id)?.session(&d.anchor_id)?.mode))).is_some_and(|(d, mode)| wr::inhibit_relaunch(d, mode, had_program));
        (member && !had_program) || anchor
    }

    /// Notes what a restored session saved of the two toolbar toggles until it is adopted (P9).
    pub(super) fn note_saved_toggles(&mut self, pane: PaneId, session: &Value) {
        let on = |k: &str| session.get(k).and_then(Value::as_bool).unwrap_or(false);
        let (a, b) = (on(crate::workgroup::AUTO_SEND_KEY), on(crate::workgroup::AUTO_REQUEST_KEY));
        let r = &mut self.workgroups.restoring;
        if (a || b) && r.toggles.len() < MAX_NOTES {
            r.toggles.insert(pane, (a, b));
        }
    }

    /// Registers a restored anchor (`WRC.register`): its GUIDs block the Enter Workgroup trigger and the
    /// rebuild comes once its window is up.
    pub(super) fn register_restored_anchor(&mut self, pane: PaneId, d: wr::Descriptor, name: Option<&str>, launch: bool) {
        let Some(own) = self.panes.get(&pane).map(|p| p.session_guid.clone()) else { return };
        self.workgroups.restoring.add(pane, own, d, name, launch);
    }

    /// Rebuilds the instances whose anchors are in a window now (`reconstructReadyAnchors`). Idempotent:
    /// an anchor that already belongs to an instance is skipped. When to look again.
    pub(super) fn adopt_workgroups(&mut self) -> Option<Instant> {
        if self.workgroups.restoring.pending.is_empty() {
            return None;
        }
        let now = Instant::now();
        // The launch is still applying saved windows (13x2 seam S5): members are rebuilt after it, so
        // a browser member's page takes the same deferred load as every restored page.
        if self.browser_restoring() {
            return Some(now + LOOK);
        }
        let mut waiting = false;
        for mut p in std::mem::take(&mut self.workgroups.restoring.pending) {
            if !self.panes.contains_key(&p.m.anchor) {
                // The anchor closed before the rebuild: its buried members' shells still end.
                self.workgroups.restoring.release(&mut p.m);
                self.discard_members(p.descriptor, p.m.name.as_deref(), p.m.launch);
                continue;
            }
            if p.m.asked {
                self.workgroups.restoring.pending.push(p);
                continue;
            }
            if self.pane_window_id(p.m.anchor).is_none() {
                let age = now.saturating_duration_since(p.m.since);
                if age >= GIVE_UP {
                    self.workgroups.restoring.release(&mut p.m);
                    self.discard_members(p.descriptor, p.m.name.as_deref(), p.m.launch);
                    continue;
                }
                if age >= RELEASE_AFTER {
                    self.workgroups.restoring.release(&mut p.m);
                }
                waiting = true;
                self.workgroups.restoring.pending.push(p);
                continue;
            }
            if let Some(kept) = self.adopt_one(p, None) {
                self.workgroups.restoring.pending.push(kept);
            }
        }
        waiting.then(|| now + LOOK)
    }

    /// The answer to the restore question (`Then::RestoreWorkgroup`): yes rebuilds with the commands
    /// the question showed, no leaves the anchor a plain session and ends the buried members' shells.
    pub(super) fn restore_answered(&mut self, token: u64, workgroup: Workgroup, approved: Approved, yes: bool) {
        let Some(at) = self.workgroups.restoring.pending.iter().position(|p| p.m.token == token) else { return };
        let mut p = self.workgroups.restoring.pending.remove(at);
        self.workgroups.restoring.release(&mut p.m);
        let current = self.workgroup_model.get(&workgroup.unique_identifier).cloned();
        let ok = yes && self.panes.contains_key(&p.m.anchor) && match run::to_enter(&approved, &workgroup, current.as_ref()) {
            Ok(_) => true,
            Err(why) => {
                self.banners.set("workgroup", why);
                false
            }
        };
        if ok {
            p.m.asked = false;
            self.adopt_one(p, Some(approved));
        } else {
            self.discard_members(p.descriptor, p.m.name.as_deref(), p.m.launch);
        }
    }

    /// Ends the sessions a restore took back for buried members that will not be adopted (a workgroup
    /// deleted since the save, a descriptor that does not fit it, a question answered no): a shell the
    /// session host kept for one would otherwise run on with no one to see it. Members with no shell to
    /// take back were never built, so nothing is. Called with the descriptor consumed.
    fn discard_members(&mut self, mut d: wr::Descriptor, name: Option<&str>, launch: bool) {
        for m in d.members.iter_mut().filter(|m| !m.pending && super::restore_ui::attach_key(&m.arrangement).is_some()) {
            let session = std::mem::replace(&mut m.arrangement, Value::Table(Default::default()));
            if let Some(id) = self.restore_member_pane(session, name, launch, false) {
                self.workgroups.close_later(id);
            }
        }
    }

    /// One buried member's saved session as a pane in no layout: the same build every restored session
    /// takes (`build_tab`), through a tab of its own that is dropped, so its profile, folder, reattach
    /// and contents follow the rules of every restored session.
    fn restore_member_pane(&mut self, session: Value, name: Option<&str>, launch: bool, inhibit: bool) -> Option<PaneId> {
        let area = self.content_area();
        let root = crate::arrangement::node_with_active(&crate::layout::Tree::Leaf(session), area.w as f64, area.h as f64, None);
        let tab = crate::arrangement::tab(root, false, None, false, None);
        self.workgroups.restoring.inhibit_next = inhibit;
        let built = self.build_tab(&tab, name, launch, false);
        self.workgroups.restoring.inhibit_next = false;
        built.ok()?.layout.panes().first().copied()
    }

    /// Rebuilds one instance (`WRC.reconstruct`, `WI.adopt`). Returns the entry when the person is being
    /// asked and the answer decides.
    fn adopt_one(&mut self, p: Pending, approved: Option<Approved>) -> Option<Pending> {
        let Pending { mut m, descriptor } = p;
        let anchor = m.anchor;
        // Idempotent: another pass rebuilt this anchor.
        if self.panes.get(&anchor).is_some_and(|a| a.workgroup.is_some()) {
            self.workgroups.restoring.release(&mut m);
            return None;
        }
        let plan = match wr::adoption(&descriptor, &|id| self.workgroup_model.get(id).cloned()) {
            Ok(plan) => plan,
            Err(why) => {
                // The config is gone or does not fit: the anchor stays a plain session (WRC).
                crate::rlog!("workgroup restore skipped: {}", crate::dlog::name(&format!("{why:?}")));
                self.workgroups.restoring.release(&mut m);
                self.discard_members(descriptor, m.name.as_deref(), m.launch);
                return None;
            }
        };
        let w = plan.workgroup.clone();
        let source = self.workgroup_model.source_of(&w.unique_identifier);
        let approved = match approved {
            Some(a) => a,
            None if wr::needs_question(source, &descriptor, &w) => {
                let Some((heading, body)) = run::ask_text(&w, Mechanism::Restore) else {
                    self.banners.set("workgroup", "A restored workgroup's commands are too long to show in full, so it was not rebuilt. Read them in Settings > Arrangements > Workgroups and trust it there.");
                    self.workgroups.restoring.release(&mut m);
                    self.discard_members(descriptor, m.name.as_deref(), m.launch);
                    return None;
                };
                let token = m.token;
                self.workgroups.restoring.release(&mut m);
                m.asked = true;
                let then = scripts_alerts::Then::RestoreWorkgroup { token, workgroup: Box::new(w.clone()), approved: Approved::of(&w) };
                self.ask_scripts(scripts_alerts::ask(&heading, &body, &run::RESTORE_LABELS, None, crate::warnings::Kind::Persistent, then));
                return Some(Pending { m, descriptor });
            }
            None => Approved::of(&w),
        };
        let d = descriptor;
        let (name, launch) = (m.name.clone(), m.launch);
        let anchor_config = d.anchor_id.clone();
        // The buried members that were running come back from their saved sessions; one that was on its
        // pre-launch overlay, or that cannot be restored, comes back fresh below.
        let mut panes: HashMap<String, PaneId> = HashMap::from([(anchor_config.clone(), anchor)]);
        let mut built: Vec<PaneId> = Vec::new();
        let mut d = d;
        for config in &plan.restored {
            let Some(m) = d.members.iter_mut().find(|m| &m.config_id == config) else { continue };
            let inhibit = w.session(config).is_some_and(|s| wr::mode_inhibits(s.mode));
            let session = std::mem::replace(&mut m.arrangement, Value::Table(Default::default()));
            if let Some(id) = self.restore_member_pane(session, name.as_deref(), launch, inhibit) {
                panes.insert(config.clone(), id);
                built.push(id);
            }
        }
        // Without its leader there is no instance (WRC: corrupt descriptor): what was built ends.
        let Some(&leader) = panes.get(&plan.leader) else {
            crate::rlog!("workgroup restore skipped: {}", crate::dlog::name("the leader could not be restored"));
            for id in built {
                self.workgroups.close_later(id);
            }
            self.discard_members(d, name.as_deref(), launch);
            self.workgroups.restoring.release(&mut m);
            return None;
        };
        let id = wr::resolve_instance_id(&d.instance_id, &|i| self.workgroups.contains(i));
        let mut inst = Instance {
            id: id.clone(),
            workgroup: w.clone(),
            leader,
            peers: plan.peers.iter().map(|c| (c.clone(), panes.get(c).copied())).collect(),
            active: plan.active.clone(),
            non_peers: Vec::new(),
            git_base: plan.git_base.clone(),
            provenance: run::RESTORED_PROVENANCE.to_string(),
            leader_title: None,
            group_before: None,
            torn_down: false,
            usage: HashMap::new(),
            approved,
            git: super::workgroup_git_app::GitSide::for_workgroup(&w),
            review: Default::default(),
        };
        // Splits and tabs the restore brought back as panes of their own, found by GUID; one that already
        // belongs to a live instance is never taken (restored sessions keep their GUIDs).
        let by_guid: HashMap<&str, &str> = plan.children.iter().map(|c| (c.guid.as_str(), c.config_id.as_str())).collect();
        let mut found: HashMap<String, PaneId> = HashMap::new();
        for (pid, pane) in &self.panes {
            if pane.workgroup.is_none() && !panes.values().any(|m| m == pid) {
                if let Some(config) = by_guid.get(pane.session_guid.as_str()) {
                    found.entry(config.to_string()).or_insert(*pid);
                }
            }
        }
        for step in run::spawn_plan(&w) {
            let config = match &step {
                SpawnStep::Split { config, .. } | SpawnStep::Tab { config, .. } => config,
                SpawnStep::Peer { .. } => continue,
            };
            if let Some(pid) = found.get(config) {
                inst.non_peers.push((config.clone(), *pid));
            }
        }
        // Members that came back waiting: a Diff member waits for changes, a Code Review member asks.
        let home = self.current_window();
        let mut waiting: Vec<String> = Vec::new();
        if d.anchor_pending {
            waiting.push(anchor_config.clone());
        }
        for config in &plan.fresh {
            let Some(cfg) = w.session(config).cloned() else { continue };
            if self.enter_pane_window(anchor) {
                if let Some(pid) = self.spawn_restored_peer(&w, &cfg, &id, anchor, &inst.approved) {
                    inst.peers.iter_mut().filter(|(c, _)| c == config).for_each(|(_, slot)| *slot = Some(pid));
                    waiting.push(config.clone());
                }
            }
        }
        if let Some(h) = home.filter(|h| self.current_window() != Some(*h)) {
            self.enter(h);
        }
        let diffs: Vec<String> = waiting.iter().filter(|c| w.session(c).is_some_and(|s| s.mode == Mode::Diff && inst.members_of(&s.unique_identifier))).cloned().collect();
        if let Some(git) = inst.git.as_mut() {
            for c in diffs {
                git.wait_for_changes(&c);
            }
        }
        let reviews: Vec<String> = waiting.iter().filter(|c| w.session(c).is_some_and(|s| s.mode == Mode::CodeReview && inst.members_of(&s.unique_identifier))).cloned().collect();
        if !reviews.is_empty() {
            self.review_entry_overlays(&mut inst, &reviews);
        }
        // Every member points back at the instance and takes the toggles it saved (only adopted ones, P9).
        for pid in inst.members() {
            let saved = self.workgroups.restoring.toggles.remove(&pid).unwrap_or((false, false));
            if let Some(pane) = self.panes.get_mut(&pid) {
                pane.workgroup = Some(id.clone());
                (pane.auto_send_clippings, pane.auto_request_review) = crate::workgroup::restored_toggles(|k| if k == crate::workgroup::AUTO_SEND_KEY { saved.0 } else { k == crate::workgroup::AUTO_REQUEST_KEY && saved.1 }, true);
            }
        }
        let _ = self.api_vars.set_user(crate::api_vars::Scope::Session, &leader.to_string(), "user.gitBase", serde_json::json!(inst.git_base));
        // The leader takes the root's display name (`nameLeader`, no revert snapshot: its saved profile
        // already carries the workgroup's name).
        if let Some(root) = w.root().filter(|r| !r.display_name.is_empty()) {
            let name = crate::inert_files::display_text(&root.display_name, 128, false);
            if self.panes.get(&leader).and_then(|p| self.store.get(&p.profile)).map(|p| p.name()).as_deref() != Some(name.as_str()) {
                self.set_session_values(leader, &[(crate::profile::NAME.to_string(), Some(toml::Value::String(name)))]);
            }
        }
        crate::rlog!("workgroup restored: instance {} members {}", crate::dlog::name(&id), inst.members().len());
        self.workgroups.add(inst);
        self.workgroups.restoring.release(&mut m);
        self.menus_dirty = true;
        self.redraw();
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const THIS: &str = include_str!("workgroup_restore_app.rs");

    /// P9: restoring starts no program of its own: the only spawn is a waiting Diff or Code Review peer,
    /// which starts idle (`spawn_restored_peer`), and every member comes from `build_tab`.
    #[test]
    fn restoring_never_starts_a_command() {
        let production = THIS.split("#[cfg(test)]").next().unwrap();
        for banned in ["start_command_override", "start_pane(", "start_pane_as(", "start_extra_env", "restart_pane", "restart_via", "enter_workgroup_now", "plan_command"] {
            assert!(!production.contains(banned), "{banned}");
        }
        assert!(production.contains("self.spawn_restored_peer("));
        assert!(production.contains("self.build_tab(&tab, name, launch, false)"), "one build for every restored session");
        let wg = include_str!("workgroup_app.rs");
        let spawn = &wg[wg.find("fn spawn_restored_peer").unwrap()..];
        let spawn = &spawn[..spawn.find("\n    }\n").unwrap()];
        assert!(spawn.contains("cfg.mode") && spawn.contains("Mode::Diff | Mode::CodeReview"), "only a member that starts idle is spawned: {spawn}");
    }

    /// The descriptor is read only for launch restoration and a saved arrangement, never a file.
    #[test]
    fn a_file_descriptor_is_never_read() {
        let production = THIS.split("#[cfg(test)]").next().unwrap();
        assert!(production.contains("if !(launch || named)"));
        let arrange = include_str!("arrange_ui.rs");
        assert!(arrange.contains("self.restored_descriptor(&session, launch, name.is_some())"));
    }

    fn descriptor() -> wr::Descriptor {
        wr::Descriptor::from_value(
            &wr::Descriptor { workgroup_id: "W".into(), instance_id: String::new(), git_base: "HEAD".into(), leader_id: "MAIN".into(), anchor_id: "MAIN".into(), members: Vec::new(), member_guids: vec!["B1".into()], non_peer_children: vec![wr::Child { config_id: "SPLIT".into(), guid: "C1".into() }], anchor_pending: false, approval: None }
                .to_value(),
        )
        .unwrap()
    }

    /// `restoringGUIDCounts`: two restores that claim one GUID release it only when both are over, and a
    /// release twice counts once; the Enter Workgroup trigger is blocked in every claimed session.
    #[test]
    fn restoring_guids_are_counted_as_iterm2() {
        let mut r = Restoring::default();
        assert!(r.add(1, "A1".into(), descriptor(), None, true));
        assert!(r.add(2, "A2".into(), descriptor(), Some("saved"), false));
        for g in ["A1", "A2", "B1", "C1"] {
            assert!(r.guids.contains_key(g), "{g}");
        }
        assert_eq!(r.guids["B1"], 2);
        let mut first = r.pending.remove(0);
        r.release(&mut first.m);
        r.release(&mut first.m);
        assert_eq!(r.guids["B1"], 1, "released once, however often it is asked");
        assert!(!r.guids.contains_key("A1") && r.guids.contains_key("A2"));
        let mut second = r.pending.remove(0);
        r.release(&mut second.m);
        assert!(r.guids.is_empty());
        // A file cannot queue work without end.
        let mut many = Restoring::default();
        let kept = (0..100).filter(|i| many.add(*i, format!("G{i}"), descriptor(), None, true)).count();
        assert_eq!(kept, MAX_PENDING);
    }

    /// A trigger's Enter question that was cancelled is not asked again for that session and workgroup,
    /// until the session is gone (the Task 8 review: not every 5 seconds).
    #[test]
    fn a_cancelled_trigger_is_not_asked_again() {
        let mut r = Restoring::default();
        assert!(!r.declined(7, "builtin.claudeCode"));
        r.decline(7, "builtin.claudeCode");
        assert!(r.declined(7, "builtin.claudeCode"));
        assert!(!r.declined(7, "other") && !r.declined(8, "builtin.claudeCode"));
        r.toggles.insert(7, (true, false));
        r.forget_pane(7);
        assert!(!r.declined(7, "builtin.claudeCode") && r.toggles.is_empty());
        for i in 0..200 {
            r.decline(i, "w");
        }
        assert!(r.declined.len() <= MAX_NOTES);
    }

    /// The Enter Workgroup trigger samples the restoring flag when it fires and asks once per match; the
    /// manual entry refuses an anchor a restore is rebuilding.
    #[test]
    fn the_trigger_checks_the_restoring_flag() {
        let wg = include_str!("workgroup_app.rs");
        let production = wg.split("#[cfg(test)]").next().unwrap();
        let queue = &production[production.find("fn run_workgroup_triggers").unwrap()..production.find("/// Ends the members a teardown queued").unwrap()];
        assert!(queue.contains("self.is_restoring_session(pane)"), "{queue}");
        assert!(queue.contains("self.workgroups.restoring.declined(pane, &id)"), "{queue}");
        assert!(production.contains("self.is_restoring_anchor(pane)"), "enter_workgroup passes the real flag, not false");
        let alerts = include_str!("scripts_alerts.rs");
        assert!(alerts.contains("self.workgroups.restoring.decline(pane, &workgroup.unique_identifier)"), "a Cancel is remembered");
    }

    /// Everything a restore remembers about a pane goes with the pane.
    #[test]
    fn the_notes_go_with_the_pane() {
        let wg = include_str!("workgroup_app.rs");
        let ended = &wg[wg.find("pub(super) fn workgroup_member_ended").unwrap()..];
        let ended = &ended[..ended.find("\n    }\n").unwrap()];
        assert!(ended.contains("self.workgroups.restoring.forget_pane(pane)"), "{ended}");
    }
}
