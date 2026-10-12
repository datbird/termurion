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
use std::collections::{HashMap, VecDeque};

/// Restored anchors waiting at most (a file cannot queue work without end).
const MAX_PENDING: usize = 16;
/// How soon the next look comes while anchors wait for their window.
const LOOK: Duration = Duration::from_millis(250);
/// Toggles and declined entries remembered at most.
const MAX_NOTES: usize = 64;

/// This install's approval key (`wr::key_in`), read or made once per run. None: every import asks.
#[cfg(windows)]
fn approval_key() -> Option<&'static [u8; wr::KEY_LEN]> {
    static KEY: std::sync::OnceLock<Option<[u8; wr::KEY_LEN]>> = std::sync::OnceLock::new();
    KEY.get_or_init(|| crate::globals::default_path().parent().and_then(wr::key_in)).as_ref()
}

#[cfg(not(windows))]
fn approval_key() -> Option<&'static [u8; wr::KEY_LEN]> {
    None
}

/// One look at a waiting restore: the step, and what Ask and Adopt need.
struct Look {
    step: wr::Step,
    plan: Option<wr::Adoption>,
    ask: Option<(String, String)>,
}

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
    /// A yes given while the anchor was in no window: kept here, with the workgroup the question showed,
    /// until the anchor is back in a window and the children can be found (the wait counts from it).
    answer: Option<(Workgroup, Approved)>,
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
    /// Oldest first; past `MAX_NOTES` the oldest is forgotten.
    declined: VecDeque<(PaneId, String)>,
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
        self.pending.push(Pending { m: Meta { token, anchor, name: name.map(str::to_string), launch, guids, since: Instant::now(), released: false, asked: false, answer: None }, descriptor: d });
        true
    }

    /// Drops every note about a pane that is gone.
    pub(super) fn forget_pane(&mut self, pane: PaneId) {
        self.toggles.remove(&pane);
        self.declined.retain(|(p, _)| *p != pane);
    }

    /// Remembers that the person said no to this entry.
    pub(super) fn decline(&mut self, pane: PaneId, workgroup: &str) {
        if self.declined(pane, workgroup) {
            return;
        }
        if self.declined.len() >= MAX_NOTES {
            self.declined.pop_front();
        }
        self.declined.push_back((pane, workgroup.to_string()));
    }

    pub(super) fn declined(&self, pane: PaneId, workgroup: &str) -> bool {
        self.declined.iter().any(|(p, w)| *p == pane && w == workgroup)
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
        // The answer the person gave is kept only in the launch state (the contents session restoration
        // saves, never an arrangement or a file), sealed to this install (Task 9 review C1).
        let digest = inst.approved.digest();
        let state = matches!(contents, Contents::Styled(_) | Contents::KeysOnly);
        let approval = approval_key().filter(|_| state && !digest.is_empty()).map(|k| wr::seal(k, &w.unique_identifier, digest));
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
                approval,
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

    /// Whether the restored session starts no program of its own (`shouldInhibitRelaunch`): the anchor
    /// when its workgroup still exists, its mode is Code Review or Diff and no program came back from the
    /// session host; a buried member through `inhibit_next`. Asked with or without a program, so a
    /// failed attach is answered too.
    pub(super) fn restore_inhibits(&mut self, d: Option<&wr::Descriptor>) -> wr::Inhibit {
        let member = std::mem::take(&mut self.workgroups.restoring.inhibit_next);
        let mode = d.and_then(|d| Some(self.workgroup_model.get(&d.workgroup_id)?.session(&d.anchor_id)?.mode));
        wr::Inhibit::of(d, mode, member)
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
    /// an anchor that already belongs to an instance is not adopted again. When to look again.
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
            // A kept yes is checked again against the workgroup as it is now (as `restore_answered` does).
            let kept = p.m.answer.take();
            let profiles = |g: Option<&str>| self.member_profile_of(g);
            let answer = kept.as_ref().map(|(w, approved)| run::to_enter(approved, w, self.workgroup_model.get(&w.unique_identifier), &profiles).is_ok());
            let look = self.look(&p, now, answer);
            waiting |= matches!(look.step, wr::Step::Wait { .. });
            p.m.answer = kept.clone().filter(|_| matches!(look.step, wr::Step::Wait { .. }));
            self.take_step(p, look, kept.map(|(_, a)| a));
        }
        waiting.then(|| now + LOOK)
    }

    /// The answer to the restore question (`Then::RestoreWorkgroup`): yes rebuilds with the commands
    /// the question showed, no leaves the anchor a plain session and ends the buried members' shells.
    /// The restore kept its claims while the question was up (Task 9 review I2).
    pub(super) fn restore_answered(&mut self, token: u64, workgroup: Workgroup, approved: Approved, yes: bool) {
        let Some(at) = self.workgroups.restoring.pending.iter().position(|p| p.m.token == token) else { return };
        let p = self.workgroups.restoring.pending.remove(at);
        let current = self.workgroup_model.get(&workgroup.unique_identifier).cloned();
        let ok = yes && match run::to_enter(&approved, &workgroup, current.as_ref(), &|g| self.member_profile_of(g)) {
            Ok(_) => true,
            Err(why) => {
                self.banners.set("workgroup", why);
                false
            }
        };
        let mut p = p;
        let now = Instant::now();
        if ok {
            p.m.since = now;
        }
        let look = self.look(&p, now, Some(ok));
        if ok && matches!(look.step, wr::Step::Wait { .. }) {
            p.m.answer = Some((workgroup, approved.clone()));
        }
        self.take_step(p, look, Some(approved));
    }

    /// What one waiting restore does next (`wr::next_step`), with the plan and the question text it
    /// needs. The plan and the trust check are worked out only when the step can depend on them.
    fn look(&self, p: &Pending, now: Instant, answer: Option<bool>) -> Look {
        let anchor = p.m.anchor;
        let anchor_alive = self.panes.contains_key(&anchor);
        let in_window = self.pane_window_id(anchor).is_some();
        let mut look = Look { step: wr::Step::Hold, plan: None, ask: None };
        let mut question = wr::Question::None;
        if anchor_alive && (answer.is_some() || (!p.m.asked && in_window)) {
            match wr::adoption(&p.descriptor, &|id| self.workgroup_model.get(id).cloned()) {
                Ok(plan) => {
                    let w = &plan.workgroup;
                    let origin = if p.m.launch { wr::Origin::Launch } else { wr::Origin::Named };
                    let profiles = |g: Option<&str>| self.member_profile_of(g);
                    if answer.is_none() && wr::needs_question(self.workgroup_model.source_of(&w.unique_identifier), &p.descriptor, w, origin, approval_key().map(|k| &k[..]), &profiles) {
                        look.ask = run::ask_text(w, Mechanism::Restore, &profiles);
                        question = if look.ask.is_some() { wr::Question::Showable } else { wr::Question::TooLong };
                    }
                    look.plan = Some(plan);
                }
                // The config is gone or does not fit: the anchor stays a plain session (WRC).
                Err(why) => crate::rlog!("workgroup restore skipped: {}", crate::dlog::name(&format!("{why:?}"))),
            }
        }
        let facts = wr::Facts {
            anchor_alive,
            in_window,
            in_workgroup: self.panes.get(&anchor).is_some_and(|a| a.workgroup.is_some()),
            asked: p.m.asked,
            age: now.saturating_duration_since(p.m.since),
            answer,
            fits: look.plan.is_some(),
            question,
        };
        look.step = wr::next_step(&facts);
        look
    }

    /// Takes the step `look` decided for `p`. Only Adopt and Discard end a restore; the claims are
    /// released where the step says (`Step::keeps_claims`).
    fn take_step(&mut self, mut p: Pending, look: Look, answered: Option<Approved>) {
        match look.step {
            wr::Step::Wait { release } => {
                if release {
                    self.workgroups.restoring.release(&mut p.m);
                }
                self.workgroups.restoring.pending.push(p);
            }
            wr::Step::Hold => self.workgroups.restoring.pending.push(p),
            wr::Step::Ask => {
                let (Some(plan), Some((heading, body))) = (look.plan, look.ask) else { return self.end_restore(p, wr::Why::Unfit) };
                let w = plan.workgroup;
                p.m.asked = true;
                let then = scripts_alerts::Then::RestoreWorkgroup { token: p.m.token, approved: Approved::of(&w, &|g| self.member_profile_of(g)), workgroup: Box::new(w) };
                self.workgroups.restoring.pending.push(p);
                self.ask_scripts(scripts_alerts::ask(&heading, &body, &run::RESTORE_LABELS, None, crate::warnings::Kind::Persistent, then));
            }
            wr::Step::Adopt => match look.plan {
                Some(plan) => {
                    let approved = answered.unwrap_or_else(|| Approved::of(&plan.workgroup, &|g| self.member_profile_of(g)));
                    self.adopt_one(p, plan, approved);
                }
                None => self.end_restore(p, wr::Why::Unfit),
            },
            wr::Step::Discard(why) => self.end_restore(p, why),
        }
    }

    /// A restore that is not adopted: its claims go and the buried members' shells end.
    fn end_restore(&mut self, mut p: Pending, why: wr::Why) {
        if why == wr::Why::TooLong {
            self.banners.set("workgroup", "A restored workgroup's commands are too long to show in full, so it was not rebuilt. Read them in Settings > Arrangements > Workgroups and trust it there.");
        }
        self.workgroups.restoring.release(&mut p.m);
        self.discard_members(p.descriptor, p.m.name.as_deref(), p.m.launch);
    }

    /// Ends the sessions a restore took back for buried members that will not be adopted (a workgroup
    /// deleted since the save, a descriptor that does not fit it, a question answered no): a shell the
    /// session host kept for one would otherwise run on with no one to see it. Members with no shell to
    /// take back were never built, so nothing is. Called with the descriptor consumed.
    fn discard_members(&mut self, mut d: wr::Descriptor, name: Option<&str>, launch: bool) {
        for m in d.members.iter_mut().filter(|m| !m.pending && super::restore_ui::attach_key(&m.arrangement).is_some()) {
            let session = std::mem::replace(&mut m.arrangement, Value::Table(Default::default()));
            if let Some(id) = self.restore_member_pane(session, name, launch, true) {
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

    /// Rebuilds one instance (`WRC.reconstruct`, `WI.adopt`) from its plan, with the commands the
    /// person's answer (or the person's own workgroup, or an approval that stands) allows.
    fn adopt_one(&mut self, p: Pending, plan: wr::Adoption, approved: Approved) {
        let Pending { mut m, descriptor } = p;
        // The cap on running workgroups holds for a restore as for an entry: past it the anchor stays a
        // plain session and the buried members' shells end.
        if self.workgroups.len() >= super::workgroup_app::MAX_INSTANCES {
            self.banners.set("workgroup", format!("{} workgroups are running, so a restored one was not rebuilt.", super::workgroup_app::MAX_INSTANCES));
            self.workgroups.restoring.release(&mut m);
            self.discard_members(descriptor, m.name.as_deref(), m.launch);
            return;
        }
        let anchor = m.anchor;
        let w = plan.workgroup.clone();
        let mut d = descriptor;
        let (name, launch) = (m.name.clone(), m.launch);
        let anchor_config = d.anchor_id.clone();
        // The buried members that were running come back from their saved sessions; one that was on its
        // pre-launch overlay, or that cannot be restored, comes back fresh below.
        let mut panes: HashMap<String, PaneId> = HashMap::from([(anchor_config.clone(), anchor)]);
        let mut built: Vec<PaneId> = Vec::new();
        for config in &plan.restored {
            let Some(m) = d.members.iter_mut().find(|m| &m.config_id == config) else { continue };
            let inhibit = w.session(config).is_some_and(|s| wr::member_inhibits(s.mode, wr::foreign_restore(name.is_some(), launch, self.state_imported)));
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
            return;
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
            seq: 0,
            usage: HashMap::new(),
            approved,
            git: super::workgroup_git_app::GitSide::for_workgroup(&w),
            review: Default::default(),
        };
        // Splits and tabs the restore brought back as panes of their own, found by GUID in the anchor's
        // window; one that already belongs to a live instance is never taken (restored sessions keep their
        // GUIDs), nor a pane in another window whose GUID collides.
        let by_guid: HashMap<&str, &str> = plan.children.iter().map(|c| (c.guid.as_str(), c.config_id.as_str())).collect();
        let anchor_window = self.pane_window_id(anchor);
        let mut found: HashMap<String, PaneId> = HashMap::new();
        for (pid, pane) in &self.panes {
            if pane.workgroup.is_none() && !panes.values().any(|m| m == pid) {
                if let Some(config) = by_guid.get(pane.session_guid.as_str()).filter(|_| anchor_window.is_some() && self.pane_window_id(*pid) == anchor_window) {
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

    /// The text of `f`'s body in `src` (from its signature to the first closing brace at its indent).
    fn body<'a>(src: &'a str, sig: &str) -> &'a str {
        let from = &src[src.find(sig).unwrap_or_else(|| panic!("{sig}"))..];
        &from[..from.find("\n    }\n").unwrap()]
    }

    /// Task 9 review C1, path by path: an imported workgroup restored by any path but Tandem's own launch
    /// state asks before anything runs, and the launch state's approval counts only as this install's
    /// seal. Each path is pinned to the origin it builds with, then the decision is checked for what
    /// that path can deliver: the plain digest (computable by whoever holds the workgroup) or another
    /// install's seal.
    #[test]
    fn every_restore_path_asks_for_an_import_until_answered() {
        let arrange = include_str!("arrange_ui.rs");
        let mod_rs = include_str!("mod.rs");
        let windows = include_str!("windows_ui.rs");
        // OpenArrangementAtStartup: the default arrangement opens by name, never as the launch state.
        assert!(mod_rs.contains(".or(default_arrangement.map(|(n, w)| (n, w, false)))"));
        assert!(mod_rs.contains("if launch { windows_ui::NewWindow::Restored(w) } else { windows_ui::NewWindow::Arrangement(w, name.clone()) }"));
        // Remote prefs and imports deliver Window Arrangements; Window > Arrangements, Open Quickly and
        // the it2/API restore all open one by name: built with launch false.
        let restore = body(arrange, "pub(super) fn restore_arrangement(");
        assert!(restore.contains("self.build_tab(&t, Some(name), false,") && restore.contains("NewWindow::Arrangement(w, Some(name.to_string()))"), "{restore}");
        assert!(windows.contains("NewWindow::Arrangement(w, name) => {\n                if let Err(e) = self.apply_arrangement_window(&w, name.as_deref(), false)"));
        assert!(include_str!("api_requests.rs").contains("self.restore_arrangement(&name, w.is_some())"));
        // An arrangement file (Open Arrangement, archives) is never read for a descriptor at all.
        assert!(body(arrange, "pub(super) fn open_arrangement_windows(").contains("NewWindow::Arrangement(w, None)"));
        // Only the saved state restores with launch true (the startup windows and hotkey windows).
        assert_eq!(windows.matches("apply_arrangement_window(&w, None, true)").count(), 2);
        assert_eq!(mod_rs.matches("apply_arrangement_window(&w, None, true)").count(), 1);
        // Import All may bring a state file, never the key that seals its approvals.
        assert!(crate::settings_export::excluded(wr::KEY_FILE));

        let w = crate::workgroup::presets::coding_agent_plus_diff_plus_code_review(Some(("W", "MAIN", "DIFF", "REVIEW")), Some("Claude Code"));
        let ours = [3u8; wr::KEY_LEN];
        let digest = Approved::of(&w, &|_| run::MemberProfile::default()).digest().to_string();
        let mut d = descriptor();
        d.workgroup_id = w.unique_identifier.clone();
        let delivered = [digest.clone(), wr::seal(&[4u8; wr::KEY_LEN], &w.unique_identifier, &digest)];
        for (path, launch) in [("OpenArrangementAtStartup", false), ("remote prefs arrangement", false), ("Import All state file", true), ("it2/API restore", false)] {
            for source in [crate::workgroup::Source::Import, crate::workgroup::Source::RemotePrefs] {
                for a in &delivered {
                    d.approval = Some(a.clone());
                    let origin = if launch { wr::Origin::Launch } else { wr::Origin::Named };
                    assert!(wr::needs_question(source, &d, &w, origin, Some(&ours), &|_| run::MemberProfile::default()), "{path} {source:?}");
                }
                // Even this install's own seal, carried in a named arrangement, asks.
                d.approval = Some(wr::seal(&ours, &w.unique_identifier, &digest));
                assert_eq!(wr::needs_question(source, &d, &w, if launch { wr::Origin::Launch } else { wr::Origin::Named }, Some(&ours), &|_| run::MemberProfile::default()), !launch, "{path}");
            }
        }
    }

    /// Task 9 review I2: the steps are taken as `wr::next_step` decides them. Ask keeps the claims (no
    /// release on that path), every Discard ends the buried members' shells, and the answer is looked at
    /// again, so an anchor that entered a workgroup during the question is discarded, not dropped.
    #[test]
    fn the_steps_are_taken_as_decided() {
        let production = THIS.split("#[cfg(test)]").next().unwrap();
        let take = body(production, "    fn take_step(");
        let ask = &take[take.find("wr::Step::Ask =>").unwrap()..take.find("wr::Step::Adopt =>").unwrap()];
        assert!(!ask.contains("release"), "the question keeps the claims: {ask}");
        assert!(take.contains("wr::Step::Discard(why) => self.end_restore(p, why)"));
        let end = body(production, "    fn end_restore(");
        assert!(end.contains("self.workgroups.restoring.release(&mut p.m)") && end.contains("self.discard_members(p.descriptor"), "{end}");
        let answered = body(production, "    pub(super) fn restore_answered(");
        assert!(!answered.contains("release") && answered.contains("self.look(&p, now, Some(ok))"), "{answered}");
        // A yes while the anchor is in no window stays on the restore (wr::next_step waits), and every
        // later look takes it, checked again against the workgroup as it is now (final wave).
        assert!(answered.contains("p.m.answer = Some((workgroup, approved.clone()))"), "{answered}");
        let adopt = body(production, "    pub(super) fn adopt_workgroups(");
        assert!(adopt.contains("p.m.answer.take()") && adopt.contains("run::to_enter(approved, w,") && adopt.contains("self.look(&p, now, answer)"), "{adopt}");
        assert!(body(production, "    fn look(").contains("wr::next_step(&facts)"));
        // A Restoring entry that is being asked still claims its sessions.
        let mut r = Restoring::default();
        assert!(r.add(1, "A1".into(), descriptor(), None, true));
        r.pending[0].m.asked = true;
        assert!(r.guids.contains_key("A1") && r.guids.contains_key("B1") && !r.pending[0].m.released);
    }

    /// Final review minor 12: a restore counts against the cap on running workgroups as an entry does;
    /// past it the anchor stays a plain session and the buried members' shells end.
    #[test]
    fn a_restore_counts_against_the_cap() {
        let production = THIS.split("#[cfg(test)]").next().unwrap();
        let adopt = body(production, "    fn adopt_one(");
        let cap = adopt.find("self.workgroups.len() >= super::workgroup_app::MAX_INSTANCES").expect("the cap is checked");
        assert!(cap < adopt.find("self.restore_member_pane(").unwrap(), "before anything is built");
        let past = &adopt[cap..];
        assert!(past[..past.find("return;").unwrap()].contains("self.discard_members("), "{past}");
    }

    /// Task 9 review minor 3: a Code Review or Diff session whose shell could not be taken back comes
    /// back idle, not with a stray shell: the failed attach asks again with no program.
    #[test]
    fn a_failed_reattach_of_a_review_or_diff_member_is_idle() {
        let arrange = include_str!("arrange_ui.rs");
        let tab = body(arrange, "    pub(super) fn build_tab(");
        let failed = &tab[tab.find("Err(e) => {").unwrap()..];
        let failed = &failed[..failed.find("None if").unwrap()];
        assert!(failed.contains("if inhibit.with(false)"), "{failed}");
        let d = descriptor();
        let anchor = wr::Inhibit::of(Some(&d), Some(crate::workgroup::Mode::Diff), false);
        assert!(!anchor.with(true) && anchor.with(false), "a Diff anchor: idle unless a program came back");
        let member = wr::Inhibit::of(None, None, true);
        assert!(!member.with(true) && member.with(false), "a buried Code Review or Diff member: the same");
        let plain = wr::Inhibit::of(Some(&d), Some(crate::workgroup::Mode::Regular), false);
        assert!(!plain.with(false) && !plain.with(true));
    }

    /// 13y1 final review minor 6: a named arrangement's buried members are never started hidden.
    #[test]
    fn buried_members_of_a_named_arrangement_are_built_idle() {
        let production = THIS.split("#[cfg(test)]").next().unwrap();
        let adopt = body(production, "    fn adopt_one(");
        assert!(adopt.contains("wr::member_inhibits(s.mode, wr::foreign_restore(name.is_some(), launch, self.state_imported))"), "{adopt}");
    }

    /// Carried pass I1: a member taken back only to be ended is built idle too, so a forged attach table
    /// cannot start its embedded command when a restore is declined, unfit, too long or over the cap.
    #[test]
    fn discarded_members_are_built_idle() {
        let production = THIS.split("#[cfg(test)]").next().unwrap();
        let discard = body(production, "    fn discard_members(");
        assert!(discard.contains("self.restore_member_pane(session, name, launch, true)"), "{discard}");
        assert!(!discard.contains("launch, false)"), "{discard}");
    }

    /// Carried pass I2: an imported state file restores like a named arrangement.
    #[test]
    fn an_imported_state_starts_no_hidden_member() {
        assert!(wr::foreign_restore(true, false, false), "a named arrangement");
        assert!(wr::foreign_restore(false, true, true), "the launch state an Import All brought");
        assert!(!wr::foreign_restore(false, true, false), "the person's own launch state keeps its shells");
        let restore = include_str!("restore_ui.rs");
        let prod = restore.split("#[cfg(test)]").next().unwrap();
        assert!(prod.contains("self.state_imported = path.with_file_name(crate::settings_export::STATE_IMPORTED).is_file()"), "the launch read notes the marker");
        assert!(prod.contains("remove_file(Self::state_path().with_file_name(crate::settings_export::STATE_IMPORTED))"), "Tandem's own state replaces it");
    }

    /// Task 9 review minor 6: a split or tab child is found only in the anchor's own window, so a plain
    /// pane elsewhere whose GUID collides is never taken in (and torn down at Exit).
    #[test]
    fn children_are_found_in_the_anchor_window_only() {
        let production = THIS.split("#[cfg(test)]").next().unwrap();
        let adopt = body(production, "    fn adopt_one(");
        let search = &adopt[adopt.find("let by_guid").unwrap()..adopt.find("for step in run::spawn_plan").unwrap()];
        assert!(search.contains("self.pane_window_id(*pid) == anchor_window"), "{search}");
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
        // Past the limit the oldest is forgotten, never the newest (Task 9 review minor 4).
        assert!(r.declined(199, "w") && r.declined(200 - MAX_NOTES as u64, "w"), "the latest Cancel is remembered");
        assert!(!r.declined(0, "w"));
        r.decline(199, "w");
        assert_eq!(r.declined.len(), MAX_NOTES, "a repeat is not counted twice");
    }

    /// The Enter Workgroup trigger samples the restoring flag when it fires and asks once per match; the
    /// manual entry refuses an anchor a restore is rebuilding.
    #[test]
    fn the_trigger_checks_the_restoring_flag() {
        let wg = include_str!("workgroup_app.rs");
        let production = wg.split("#[cfg(test)]").next().unwrap();
        let queue = &production[production.find("fn run_workgroup_triggers").unwrap()..production.find("/// Ends the members a teardown queued").unwrap()];
        // The guards live in one helper that both the terminal and the browser Enter call.
        let guard = &production[production.find("fn trigger_enter_id").unwrap()..production.find("/// The queued triggers, after the pass").unwrap()];
        assert!(guard.contains("self.is_restoring_session(pane)"), "{guard}");
        assert!(guard.contains("self.workgroups.restoring.declined(pane, &id)"), "{guard}");
        assert_eq!(queue.matches("self.trigger_enter_id(pane,").count(), 2, "terminal and browser Enter both use it");
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
