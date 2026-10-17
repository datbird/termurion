//! Watchers and timers on the App side (12f Task 11; `OrchestratorDispatcher.swift` watcher
//! half, `ScreenWatchPoller.swift`). The rules are `orch_watch`'s.
//!
//! A watcher's event is data: it is queued in the chat's outbox as an already cleaned
//! `StatusUpdate`, and goes out only while the chat's model passes 12a's gate and the rate
//! allows (ruling P9). Nothing here reads the stored `watchers` column back (ruling P2: the
//! first open of a run empties it); everything dies with Stop, AI off, the mode off, a deleted
//! chat, a closed pane (ruling P7).

use std::time::{Duration, Instant};

use serde_json::Value;

use super::orch_dispatch::Wait;
use super::App;
use crate::ai_catalog::Model;
use crate::ai_gate::Check;
use crate::ai_msg::{Message as WireMessage, Role};
use crate::ai_permissions::{Category, Permission};
use crate::ai_turn::{Hosted, Runner, Turn, TurnInput, Wire};
use crate::chat_gate::{endpoint_of, gate_open, pass};
use crate::chat_message::{Content, Message, Participant};
use crate::chat_store::Chat;
use crate::layout::PaneId;
use crate::orch::{self, Outcome};
use crate::orch_tools::{self as ot, OrchError};
use crate::orch_watch::{self as ow, Flight, Mode, Poll, Reason, Registered, Verdict, Watcher};
use crate::session_state::{self, State};

/// `TD`'s advice when a session-bound tab-status watch has no screen backstop.
const NO_BACKSTOP: &str = "No screen backstop: View Contents isn't granted for this session, so this watch fires only on the program's own status updates and has no timeout. If that status source stops (the program crashes or detaches) the watch will not fire. Grant View Contents (Always) for a screen-based fallback, or plan to check the session yourself if it goes quiet.";

/// How often a tick looks at the session watchers when nothing sooner is due.
const TICK: Duration = Duration::from_secs(10);

fn now_secs() -> f64 {
    super::warnings_app::unix_now()
}

/// What one look at a watcher decided.
enum Step {
    Wait,
    /// Ends without a word (its link changed).
    End,
    Fire(Reason, String, String),
    Drop(String),
}

/// The reads a watcher's shape needs (`WorkgroupWatcher.readRequirement`): a screen poll or a
/// condition reads the screen, a target state reads the state; a timer reads nothing.
fn needs(w: &Watcher) -> (bool, bool) {
    (w.condition.is_some() || w.effective_mode() == Mode::ScreenPoll, w.target_state.is_some())
}

fn gone() -> OrchError {
    OrchError::Unsupported("The chat is gone.".to_string())
}

impl App {
    /// One of the four watch tools, in an orchestration chat or a session-bound one.
    pub(super) fn orch_watch_call(&mut self, chat: &str, call_id: &str, name: &str, args: &Value) -> Outcome {
        let answered = match name {
            "register_watch" => return self.orch_register_watch(chat, call_id, args),
            "register_timer" => self.orch_register_timer(chat, args),
            "unregister_watch" => self.orch_unregister_watch(chat, args),
            "list_watches" => self.orch_list_watches(chat),
            other => Err(OrchError::UnknownTool(other.to_string())),
        };
        Outcome::Done(answered.unwrap_or_else(|e| ot::error_json(&e)))
    }

    pub(super) fn orch_register_timer(&mut self, chat: &str, args: &Value) -> Result<String, OrchError> {
        let row = self.chat_row(chat).ok_or_else(gone)?;
        let a = ot::parse_register_timer(args)?;
        let now = now_secs();
        let fire = ow::resolve_timer(a.delay, a.fire_at.as_deref(), now)?;
        let mut w = Watcher::timer(&crate::chat_message::new_id(), now, fire, a.note);
        w.linked = row.terminal_session_guid;
        let d = ow::description(&w, None);
        self.orch_watch_add(chat, w)?;
        Ok(ot::result_json("watcher_registered", d))
    }

    pub(super) fn orch_unregister_watch(&mut self, chat: &str, args: &Value) -> Result<String, OrchError> {
        let id = ot::parse_unregister_watch(args)?;
        self.orch_watch_remove(chat, &id);
        Ok(ot::ack_json())
    }

    pub(super) fn orch_list_watches(&mut self, chat: &str) -> Result<String, OrchError> {
        let list: Vec<Value> = self.orch_watchers.get(chat).map(|l| l.iter().map(|w| ow::description(w, None)).collect()).unwrap_or_default();
        Ok(ot::result_json("watcher_list", Value::Array(list)))
    }

    /// register_watch: the answer, or parked on the one-time screen-read consent.
    pub(super) fn orch_register_watch(&mut self, chat: &str, call_id: &str, args: &Value) -> Outcome {
        match self.orch_register_watch_in(chat, call_id, args, false) {
            Ok(o) => o,
            Err(e) => Outcome::Done(ot::error_json(&e)),
        }
    }

    /// The person's answer to the watch consent: registers with it, or says it was declined.
    pub(super) fn orch_watch_consent_answered(&mut self, chat: &str, call_id: &str, args: &Value, approved: bool) -> Outcome {
        if !approved {
            return Outcome::Done(ot::error_json(&OrchError::Unsupported(ow::SCREEN_DECLINED.to_string())));
        }
        match self.orch_register_watch_in(chat, call_id, args, true) {
            Ok(o) => o,
            Err(e) => Outcome::Done(ot::error_json(&e)),
        }
    }

    fn orch_watch_perm(&self, chat: &str, row: &Chat, c: Category) -> Permission {
        match row.terminal_session_guid.as_deref() {
            Some(g) => self.ai_overrides.get(chat, g, c, &|k: Category| self.ai_global_permission(k)),
            None => Permission::Never,
        }
    }

    /// `consented` is true on the re-run after the person approved: every permission is read
    /// again then, so a change during the prompt fails closed (`DI:2254-2272`).
    fn orch_register_watch_in(&mut self, chat: &str, call_id: &str, args: &Value, consented: bool) -> Result<Outcome, OrchError> {
        let row = self.chat_row(chat).ok_or_else(gone)?;
        let a = ot::parse_register_watch(args)?;
        let orchestrating = row.orchestration_enabled;
        let linked = if orchestrating { None } else { row.terminal_session_guid.as_deref().and_then(|g| self.ref_pane(g)).map(|p| self.orch_stable(p)) };
        let target = ow::resolve_target(a.guid.as_deref(), orchestrating, linked.as_deref())?;
        let pane = self.orch_resolve(&target)?;
        let status = self.panes.get(&pane).map(|p| p.own().tab_status.clone()).ok_or_else(|| OrchError::TargetGone(target.clone()))?;
        if self.orch_pane_state(pane).exited {
            return Err(OrchError::TargetGone(target));
        }
        let reports = session_state::reported_state(&status) != State::Unknown;
        let screen_poll = ow::is_screen_poll(a.condition.is_some(), reports);
        if screen_poll && self.orch_watch_model().is_none() {
            return Err(OrchError::Unsupported(ow::NO_WATCH_MODEL.to_string()));
        }
        let name = self.orch_name(pane);
        // A member of a workgroup is watched under its instance and role (`doRegisterWatch`).
        let (workgroup_id, workgroup_name, role_id, role_name) = self.orch_identity(&target).unwrap_or_else(|| (format!("{}{target}", orch::SYNTHETIC_PREFIX), name.clone(), "default".to_string(), name));
        let now = now_secs();
        let mut w = Watcher {
            watcher_id: crate::chat_message::new_id(),
            session_guid: target.clone(),
            workgroup_id,
            workgroup_name,
            role_id,
            role_name,
            target_state: a.target_state,
            registered_at: now,
            mode: Some(if screen_poll { Mode::ScreenPoll } else { Mode::TabStatus }),
            condition: a.condition.clone(),
            notify_user: None,
            fire_date: None,
            note: None,
            screen_read_consented: None,
            linked: row.terminal_session_guid.clone(),
        };
        // The same watch is registered once, before any prompt.
        if let Some(e) = self.orch_watchers.get(chat).and_then(|l| l.iter().find(|e| e.effective_mode() != Mode::Timer && e.session_guid == w.session_guid && e.target_state == w.target_state && e.condition == w.condition)) {
            return Ok(Outcome::Done(ot::result_json("watcher_registered", ow::description(e, None))));
        }
        let mut screen_consent = false;
        if !orchestrating {
            let req = ow::read_requirement(a.condition.is_some(), reports);
            let (view, state) = (self.orch_watch_perm(chat, &row, Category::ViewContents), self.orch_watch_perm(chat, &row, Category::CheckTerminalState));
            match ow::registration_reads(req, view, state) {
                ow::ReadCheck::Denied(t) => return Err(OrchError::Unsupported(t)),
                ow::ReadCheck::Ok { ask_consent: true } if !consented => {
                    let summary = ow::consent_summary(&ow::label(&w));
                    return Ok(self.orch_ask(chat, call_id, orch::WATCH_APPROVAL_ID, &w.workgroup_name, &summary, Wait::WatchConsent { args: args.clone() }));
                }
                // Stored only while View Contents is still Ask: under Always no standing
                // consent is held, so a later downgrade to Ask stops the reads.
                ow::ReadCheck::Ok { ask_consent } => screen_consent = ask_consent,
            }
        }
        w.screen_read_consented = screen_consent.then_some(true);
        if screen_poll {
            let d = ow::description(&w, None);
            if let Registered::New = self.orch_watch_add(chat, w.clone())? {
                self.orch_polls.insert(w.watcher_id.clone(), Poll::new(Instant::now(), false));
            }
            return Ok(Outcome::Done(ot::result_json("watcher_registered", d)));
        }
        let target_state = a.target_state.unwrap_or(State::Idle);
        // Already there: fires at once (the next pass, after this call is answered).
        if session_state::state(&status) == target_state {
            let d = ow::description(&w, None);
            self.orch_event(chat, Reason::StateReached, &w, target_state.word(), ow::already(&ow::label(&w), target_state.word()));
            return Ok(Outcome::Done(ot::result_json("watcher_registered", d)));
        }
        let view = self.orch_watch_perm(chat, &row, Category::ViewContents);
        let note = (!orchestrating && target_state != State::Working && !ow::may_read_screen(false, true, view, w.screen_read_consented)).then_some(NO_BACKSTOP);
        let d = ow::description(&w, note);
        self.orch_watch_add(chat, w)?;
        Ok(Outcome::Done(ot::result_json("watcher_registered", d)))
    }

    fn orch_watch_add(&mut self, chat: &str, w: Watcher) -> Result<Registered, OrchError> {
        let list = self.orch_watchers.entry(chat.to_string()).or_default();
        let r = ow::register(list, w)?;
        self.orch_watch_persist(chat);
        (self.notify)();
        Ok(r)
    }

    /// The chat's `watchers` column is written for the record only (ruling P2).
    fn orch_watch_persist(&mut self, chat: &str) {
        let text = self.orch_watchers.get(chat).filter(|l| !l.is_empty()).map(|l| ow::records_json(l));
        self.chat_update(chat, |c| c.watchers = text);
    }

    fn orch_watch_remove(&mut self, chat: &str, id: &str) {
        let before = self.orch_watchers.get(chat).map_or(0, Vec::len);
        if let Some(l) = self.orch_watchers.get_mut(chat) {
            l.retain(|w| w.watcher_id != id);
        }
        if let Some(mut p) = self.orch_polls.remove(id)
            && let Some(f) = p.flight.as_mut() {
                f.runner.cancel();
            }
        if self.orch_watchers.get(chat).map_or(0, Vec::len) != before {
            self.orch_watch_persist(chat);
        }
    }

    /// Queues an event for the chat (cleaned and capped on the way in); it goes out from the
    /// outbox when the gate and the rate allow.
    fn orch_event(&mut self, chat: &str, reason: Reason, w: &Watcher, reached: &str, detail: String) {
        let now = now_secs();
        let changed = self.orch_resolve(&w.session_guid).ok().and_then(|p| self.panes.get(&p)).and_then(|p| p.own().newest_visible_change()).map(|t| format!("{} ago", crate::ai_terminal::compact_age((now - f64::from(t)).max(0.0))));
        let v = ow::status_update_value(reason, w, now, reached, &detail, changed.as_deref());
        self.orch_outbox.entry(chat.to_string()).or_default().hold(v);
        (self.notify)();
    }

    /// Watchers whose pane's state is now their target fire (`orch_status_changed`); a read
    /// the chat may no longer make drops them instead.
    pub(super) fn orch_status_changed(&mut self, changes: &[(PaneId, Option<String>, Option<String>)]) {
        if self.orch_watchers.is_empty() || changes.is_empty() {
            return;
        }
        let chats: Vec<String> = self.orch_watchers.keys().cloned().collect();
        for chat in chats {
            let list = self.orch_watchers.get(&chat).cloned().unwrap_or_default();
            let Some(row) = self.chat_row(&chat) else { continue };
            for w in list.into_iter().filter(|w| w.effective_mode() == Mode::TabStatus) {
                let Ok(pane) = self.orch_resolve(&w.session_guid) else { continue };
                if !changes.iter().any(|(p, _, _)| *p == pane) {
                    continue;
                }
                // Made under another link: it reads nothing and ends (Task 11 fix round 1).
                if !ow::link_holds(&w, row.terminal_session_guid.as_deref()) {
                    self.orch_watch_remove(&chat, &w.watcher_id);
                    continue;
                }
                let Some(now_state) = self.panes.get(&pane).map(|p| session_state::state(&p.own().tab_status)) else { continue };
                if Some(now_state) != w.target_state {
                    continue;
                }
                let state_perm = self.orch_watch_perm(&chat, &row, Category::CheckTerminalState);
                self.orch_watch_remove(&chat, &w.watcher_id);
                if ow::may_read_state(row.orchestration_enabled, row.terminal_session_guid.is_some(), state_perm) {
                    self.orch_event(&chat, Reason::StateReached, &w, now_state.word(), ow::reached(&ow::label(&w), now_state.word()));
                } else {
                    self.orch_event(&chat, Reason::WatcherDropped, &w, "", ow::revoked(&ow::label(&w), "Check Terminal State", false));
                }
            }
        }
    }

    /// Chat load: the in-memory watchers are armed again (a screen watch gets its poll back).
    /// Never reads the stored column (ruling P2).
    pub(super) fn orch_rearm(&mut self, chat: &str) {
        let missing: Vec<String> = self.orch_watchers.get(chat).map(|l| l.iter().filter(|w| w.effective_mode() == Mode::ScreenPoll && !self.orch_polls.contains_key(&w.watcher_id)).map(|w| w.watcher_id.clone()).collect()).unwrap_or_default();
        for id in missing {
            self.orch_polls.insert(id, Poll::new(Instant::now(), false));
        }
    }

    /// The model that judges a screen: the economy model, else the configured one
    /// (`LLMMetadata.economyModel()` nil leaves the chat model).
    fn orch_watch_model(&mut self) -> Option<Model> {
        let found = self.ai_found();
        let catalog = crate::ai_catalog::bundled();
        crate::ai_model::economy(&self.globals, catalog, &found).or_else(|| crate::ai_model::configured(&self.globals, catalog, &found))
    }

    /// Whether the gate is open for the model the chat's next turn would use.
    fn orch_chat_gate(&mut self, chat: &str) -> bool {
        let Some(row) = self.chat_row(chat) else { return false };
        let found = self.ai_found();
        let catalog = crate::ai_catalog::bundled();
        let model = row.model_name.as_deref().and_then(|n| crate::ai_model::named(n, &self.globals, catalog, &found)).or_else(|| crate::ai_model::configured(&self.globals, catalog, &found));
        model.is_some_and(|m| gate_open(&self.globals, &endpoint_of(&m)))
    }

    /// Starts one judge request behind the gate for its own host. None: no model or the gate
    /// is closed (the poll waits).
    fn orch_judge_start(&mut self, messages: Vec<WireMessage>) -> Option<(Runner, crate::ai_models::Endpoint, Option<Result<String, String>>)> {
        let model = self.orch_watch_model()?;
        let endpoint = endpoint_of(&model);
        let _proof = pass(&self.globals, &endpoint)?;
        let wire = Wire { response_limit: 64, ..Wire::from_globals(&self.globals) };
        let turn = Turn::new(TurnInput {
            model: model.clone(),
            messages,
            tools: Vec::new(),
            stream: false,
            hosted: Hosted::default(),
            previous_response_id: None,
            // SWP asks for thinking off.
            should_think: Some(false),
            effort: None,
            tier: None,
            volatile: Box::new(|| None),
            frozen: None,
            wire,
            legacy_url_pref: String::new(),
        });
        let key = crate::ai_keys::shared().lock().unwrap_or_else(|e| e.into_inner()).get(model.vendor, &mut *crate::ai_keys::system_store());
        let mut runner = Runner::new(turn, self.ai_transport.clone(), crate::ai_transport::options_from(&self.globals), self.notify.clone());
        let outs = runner.start(key.as_ref());
        Some((runner, endpoint, super::ai_safety_app::reply_of(&outs)))
    }

    /// The screen a poll judges: its last 150 lines, and whether it is a full-screen program.
    fn orch_watch_capture(&self, pane: PaneId) -> (String, bool) {
        let v = self.orch_screen_value(pane, Some(ow::POLL_LINES as i64));
        let sc = &v["screen_contents"];
        (sc["text"].as_str().unwrap_or_default().to_string(), sc["kind"] == "tui")
    }

    /// One look at a session watcher: reads re-checked, then the poll or the escalation.
    fn orch_watch_step(&mut self, chat: &str, row: &Chat, w: &Watcher, pane: PaneId, now_i: Instant) -> Step {
        // A watcher made under another link reads nothing: the permissions it was granted
        // under are not this session's (Task 11 fix round 1).
        if !ow::link_holds(w, row.terminal_session_guid.as_deref()) {
            return Step::End;
        }
        let (orch_mode, linked) = (row.orchestration_enabled, row.terminal_session_guid.is_some());
        let (view, state_perm) = (self.orch_watch_perm(chat, row, Category::ViewContents), self.orch_watch_perm(chat, row, Category::CheckTerminalState));
        let may_screen = ow::may_read_screen(orch_mode, linked, view, w.screen_read_consented);
        let (need_screen, need_state) = needs(w);
        let label = ow::label(w);
        let single = w.effective_mode() == Mode::TabStatus;
        if need_screen && !may_screen {
            return Step::Drop(ow::revoked(&label, "View Contents", true));
        }
        if need_state && !ow::may_read_state(orch_mode, linked, state_perm) && !single {
            return Step::Drop(ow::revoked(&label, "Check Terminal State", false));
        }
        let goal = w.goal();
        if single {
            // The backstop only reads while the screen reads are allowed, and never for a
            // working target (a quiet screen cannot confirm activity).
            if w.target_state == Some(State::Working) || !may_screen {
                self.orch_drop_poll(&w.watcher_id);
                return Step::Wait;
            }
            let changed = self.panes.get(&pane).and_then(|p| p.own().newest_visible_change());
            let quiet = changed.map_or(f64::MAX, |t| now_secs() - f64::from(t));
            if quiet < ow::ESCALATE_AFTER.as_secs_f64() && !self.orch_polls.get(&w.watcher_id).is_some_and(|p| p.flight.is_some()) {
                return Step::Wait;
            }
        }
        let mut poll = self.orch_polls.remove(&w.watcher_id).unwrap_or_else(|| Poll::new(now_i, single));
        let stamp = self.panes.get(&pane).and_then(|p| p.own().newest_visible_change());
        let mut verdict = None;
        if let Some(mut f) = poll.flight.take() {
            let capture = f.capture.clone();
            let reply = match f.early.take() {
                Some(r) => Some(r),
                None => {
                    if pass(&self.globals, &f.endpoint).is_none() {
                        // A request in flight when the gate closes is cancelled; the poll waits.
                        f.runner.cancel();
                        self.orch_poll_round(&mut poll, now_i, None, stamp, single);
                        self.orch_polls.insert(w.watcher_id.clone(), poll);
                        return Step::Wait;
                    }
                    let (globals, endpoint) = (&self.globals, f.endpoint.clone());
                    super::ai_safety_app::reply_of(&f.runner.poll_with(&mut || pass(globals, &endpoint)))
                }
            };
            match reply {
                None => poll.flight = Some(f),
                Some(Ok(text)) => match ow::verdict(&text) {
                    Verdict::Reached => verdict = Some(true),
                    Verdict::NotYet => verdict = Some(false),
                    Verdict::Unknown if !f.retried => {
                        let msgs = vec![WireMessage::text(Role::System, &f.system), WireMessage::text(Role::User, &f.user), WireMessage::text(Role::Assistant, &text), WireMessage::text(Role::User, ow::RETRY_PROMPT)];
                        if let Some((runner, endpoint, early)) = self.orch_judge_start(msgs) {
                            poll.flight = Some(Flight { runner, endpoint, capture: f.capture.clone(), retried: true, system: f.system.clone(), user: f.user.clone(), early });
                        } else {
                            verdict = Some(false);
                        }
                    }
                    Verdict::Unknown => verdict = Some(false),
                },
                // A failed request is unknown: keep watching.
                Some(Err(_)) => verdict = Some(false),
            }
            if let Some(false) = verdict {
                self.orch_poll_round(&mut poll, now_i, Some(capture), stamp, single);
            }
        } else if now_i >= poll.next_at {
            if !single && now_i.duration_since(poll.started) >= ow::POLL_DEADLINE {
                return Step::Fire(Reason::WatchTimedOut, String::new(), ow::timed_out(&label, &goal));
            }
            if single && poll.judged_stamp.is_some() && poll.judged_stamp == stamp {
                poll.next_at = now_i + ow::ESCALATE_AFTER;
            } else {
                let (text, tui) = self.orch_watch_capture(pane);
                let cap = (now_i.duration_since(poll.started).as_secs(), text);
                if poll.first.is_none() {
                    poll.first = Some(cap.clone());
                }
                let win = ow::window(poll.first.as_ref(), poll.previous.as_ref(), &cap);
                let (system, user) = (ow::system_prompt(w), ow::user_prompt(&win, tui));
                let msgs = vec![WireMessage::text(Role::System, &system), WireMessage::text(Role::User, &user)];
                match self.orch_judge_start(msgs) {
                    Some((runner, endpoint, early)) => poll.flight = Some(Flight { runner, endpoint, capture: cap, retried: false, system, user, early }),
                    // No model or a closed gate: nothing is sent; the next step is later.
                    None => self.orch_poll_round(&mut poll, now_i, None, stamp, single),
                }
            }
        }
        if verdict == Some(true) {
            return self.orch_watch_reached(w, &label, now_i, &poll);
        }
        self.orch_polls.insert(w.watcher_id.clone(), poll);
        Step::Wait
    }

    /// A round ended without the target: the capture becomes the previous one and the next
    /// poll is scheduled (`backoff`; an escalated watch waits a full interval).
    fn orch_poll_round(&self, poll: &mut Poll, now_i: Instant, capture: Option<(u64, String)>, stamp: Option<u32>, single: bool) {
        if let Some(c) = capture {
            poll.previous = Some(c);
        }
        let wait = if single { ow::ESCALATE_AFTER } else { ow::backoff(poll.n, now_i.duration_since(poll.started)) };
        poll.next_at = now_i + wait;
        poll.n = poll.n.saturating_add(1);
        if single {
            poll.judged_stamp = stamp;
        }
    }

    /// The judge said REACHED: the event for this watcher's goal (`screenPollFinished`).
    fn orch_watch_reached(&self, w: &Watcher, label: &str, _now_i: Instant, _poll: &Poll) -> Step {
        if let Some(c) = &w.condition {
            return Step::Fire(Reason::ConditionMet, String::new(), ow::condition_met(label, c));
        }
        let target = w.target_state.unwrap_or(State::Idle).word();
        let how = if w.effective_mode() == Mode::TabStatus { ow::HOW_STALE } else { ow::HOW_NO_STATUS };
        Step::Fire(Reason::StateReached, target.to_string(), ow::reached_by(label, target, how))
    }

    fn orch_drop_poll(&mut self, id: &str) {
        if let Some(mut p) = self.orch_polls.remove(id)
            && let Some(f) = p.flight.as_mut() {
                f.runner.cancel();
            }
    }

    /// Everything the chat's watchers and timers hold ends, nothing said (ruling P7: Stop, AI
    /// off, the mode off, a deleted chat).
    pub(super) fn orch_watch_drop_chat(&mut self, chat: &str) {
        let ids: Vec<String> = self.orch_watchers.remove(chat).unwrap_or_default().into_iter().map(|w| w.watcher_id).collect();
        for id in &ids {
            self.orch_drop_poll(id);
        }
        if let Some(o) = self.orch_outbox.get_mut(chat) {
            o.clear();
        }
        self.orch_outbox.remove(chat);
        if !ids.is_empty() {
            self.chat_update(chat, |c| c.watchers = None);
        }
    }

    fn orch_watch_drop_all(&mut self) {
        let chats: Vec<String> = self.orch_watchers.keys().chain(self.orch_outbox.keys()).cloned().collect();
        for c in chats {
            self.orch_watch_drop_chat(&c);
        }
    }

    /// Timers, session watchers, then the outboxes; when to come back.
    pub(super) fn orch_watch_tick(&mut self) -> Option<Instant> {
        self.orch_watchers.retain(|_, l| !l.is_empty());
        if self.orch_watchers.is_empty() && self.orch_outbox.values().all(ow::Outbox::is_empty) {
            self.orch_outbox.clear();
            return None;
        }
        // AI features turned off: nothing keeps running (ruling P7).
        if matches!(crate::ai_gate::check_endpoint(&self.globals, None), Check::AdminOff | Check::AiOff) {
            self.orch_watch_drop_all();
            return None;
        }
        let (now_i, now) = (Instant::now(), now_secs());
        let mut wake: Option<Instant> = None;
        let soonest = |d: Duration, wake: &mut Option<Instant>| {
            let at = now_i + d;
            *wake = Some(wake.map_or(at, |w| w.min(at)));
        };
        let chats: Vec<String> = self.orch_watchers.keys().cloned().collect();
        for chat in chats {
            let Some(row) = self.chat_row(&chat) else {
                self.orch_watch_drop_chat(&chat);
                continue;
            };
            let due = self.orch_watchers.get_mut(&chat).map(|l| ow::take_due_timers(l, now)).unwrap_or_default();
            if !due.is_empty() {
                self.orch_watch_persist(&chat);
            }
            for w in due {
                let iso = ow::iso8601(w.fire_date.unwrap_or(now));
                self.orch_event(&chat, Reason::TimerFired, &w, "", ow::timer_fired(&iso, w.note.as_deref()));
            }
            let list = self.orch_watchers.get(&chat).cloned().unwrap_or_default();
            if let Some(d) = ow::next_timer_wake(&list, now) {
                soonest(d, &mut wake);
            }
            let sessions: Vec<Watcher> = list.into_iter().filter(|w| w.effective_mode() != Mode::Timer).collect();
            if sessions.is_empty() {
                continue;
            }
            soonest(TICK, &mut wake);
            // A session-bound chat that lost its link watches nothing (iTerm2 cancels on unlink).
            if !row.orchestration_enabled && row.terminal_session_guid.is_none() {
                for w in &sessions {
                    self.orch_watch_remove(&chat, &w.watcher_id);
                }
                continue;
            }
            for w in sessions {
                let live = self.orch_resolve(&w.session_guid).ok().filter(|p| self.panes.get(p).is_some_and(|p| p.exited.is_none()));
                let step = match live {
                    None if !ow::link_holds(&w, row.terminal_session_guid.as_deref()) => Step::End,
                    // The pane closed: the watcher dies with it and the chat is told.
                    None => Step::Drop(ow::dropped(&ow::label(&w), &w.goal())),
                    Some(pane) => self.orch_watch_step(&chat, &row, &w, pane, now_i),
                };
                match step {
                    Step::End => self.orch_watch_remove(&chat, &w.watcher_id),
                    Step::Wait => {
                        if let Some(p) = self.orch_polls.get(&w.watcher_id) {
                            soonest(p.next_at.saturating_duration_since(now_i), &mut wake);
                        }
                    }
                    Step::Fire(reason, reached, detail) => {
                        self.orch_watch_remove(&chat, &w.watcher_id);
                        self.orch_event(&chat, reason, &w, &reached, detail);
                    }
                    Step::Drop(detail) => {
                        self.orch_watch_remove(&chat, &w.watcher_id);
                        self.orch_event(&chat, Reason::WatcherDropped, &w, "", detail);
                    }
                }
            }
        }
        // The outboxes: an event goes out only through an open gate, inside the rate, and not
        // while the chat awaits the person's approval (the answer wakes the loop again).
        let held: Vec<String> = self.orch_outbox.iter().filter(|(_, o)| !o.is_empty()).map(|(c, _)| c.clone()).collect();
        for chat in held {
            let open = ow::may_release(self.orch_chat_gate(&chat), self.chat_agents.get(&chat).is_some_and(|a| a.is_awaiting_approval()));
            let events = self.orch_outbox.get_mut(&chat).map(|o| o.release(now, open)).unwrap_or_default();
            for v in events {
                let m = Message::new(&chat, Participant::User, Content::WatcherEvent(v), now);
                // Data, never a mention: `orch_grant_mentions` is not run for it (ruling P9).
                let _ = self.chat_publish(&chat, m, false);
            }
            if let Some(o) = self.orch_outbox.get(&chat).filter(|o| !o.is_empty()) {
                soonest(if open { o.next_release(now_secs()).unwrap_or(TICK) } else { ow::TIMER_WAKE }, &mut wake);
            }
        }
        wake
    }
}

#[cfg(test)]
mod tests {
    /// The code of one function, up to the next top-level method.
    fn body<'a>(src: &'a str, name: &str) -> &'a str {
        let code = src.split("#[cfg(test)]").next().unwrap_or(src);
        let start = code.find(&format!("fn {name}(")).unwrap_or_else(|| panic!("fn {name} is missing"));
        let rest = &code[start..];
        let end = ["\n    pub(super) fn ", "\n    fn ", "\n    /// "].iter().filter_map(|m| rest[1..].find(m)).min().map_or(rest.len(), |e| e + 1);
        &rest[..end]
    }

    #[test]
    fn a_watcher_surviving_a_restart_is_refused() {
        // Ruling P2: the column is emptied at the first open of a run and is never read back.
        let src = include_str!("orch_watch_app.rs");
        let rearm = body(src, "orch_rearm");
        assert!(!rearm.contains("records(") && !rearm.contains(".watchers") && !rearm.contains("chat_row"), "orch_rearm reads only the in-memory lists:\n{rearm}");
        let code = src.split("#[cfg(test)]").next().unwrap();
        assert!(!code.contains("orch_watch::records("), "no code path parses the stored column");
    }

    #[test]
    fn watchers_die_with_the_chat_and_events_are_data() {
        // Ruling P7: every halt reaches orch_chat_ended, which ends the chat's watchers.
        let app = include_str!("orch_app.rs");
        assert!(body(app, "orch_chat_ended").contains("orch_watch_drop_chat"));
        assert!(include_str!("chat_app.rs").contains("self.orch_chat_ended(&chat)") && include_str!("chat_app.rs").contains("self.orch_chat_ended(chat)"), "a halt and a delete both end the chat's watchers");
        assert!(body(app, "set_orchestration").contains("orch_chat_ended"), "the mode off");
        // Ruling P9: a watcher event is never a mention grant, and goes out only through the outbox.
        let code = include_str!("orch_watch_app.rs").split("#[cfg(test)]").next().unwrap();
        assert!(!code.contains("self.orch_grant_mentions("));
        assert_eq!(code.matches("chat_publish(").count(), 1, "one publish, in the tick's outbox loop");
        let tick = body(code, "orch_watch_tick");
        assert!(tick.contains("release(now, open)") && tick.contains("AdminOff"), "gate-checked release and the AI-off sweep");
    }

    /// Final review fix, ruling 1: the outbox releases only through the gate AND while the
    /// chat does not await the person's approval, so no event lands under a parked bubble.
    #[test]
    fn an_approval_holds_the_watch_outbox() {
        let code = include_str!("orch_watch_app.rs").split("#[cfg(test)]").next().unwrap();
        let tick = body(code, "orch_watch_tick");
        assert!(tick.contains("ow::may_release(self.orch_chat_gate(&chat), self.chat_agents.get(&chat).is_some_and(|a| a.is_awaiting_approval()))"), "{tick}");
    }

    /// Task 11 fix round 1, ruling 3: an unlink or a relink ends the chat's watchers and
    /// timers at once, and every read checks the link the watcher was made under.
    #[test]
    fn a_link_change_ends_the_watchers() {
        let session = include_str!("chat_session_app.rs");
        assert!(body(session, "unlink_chat").contains("self.orch_watch_drop_chat(chat);"), "unlink");
        assert!(body(session, "link_chat").contains("self.orch_watch_drop_chat(chat);"), "relink");
        let code = include_str!("orch_watch_app.rs");
        assert!(body(code, "orch_register_watch_in").contains("linked: row.terminal_session_guid.clone(),"), "a watcher keeps its link");
        assert!(body(code, "orch_register_timer").contains("linked = row.terminal_session_guid"), "so does a timer");
        assert!(body(code, "orch_watch_step").contains("ow::link_holds(w, row.terminal_session_guid.as_deref())"), "each poll or escalation");
        assert!(body(code, "orch_status_changed").contains("ow::link_holds(&w, row.terminal_session_guid.as_deref())"), "each state read");
    }
}
