//! Orchestration in the app (12f Task 9): the mode switch, the enable request, the snapshot the
//! model reads and the cleanup of what a chat parked. The tools themselves are Task 10's.

use super::chat_engine::Engine;
use super::*;
use crate::ai_turn::ToolResult;
use crate::chat_message::{ClientLocal, Content, Message, Participant};
use crate::chat_store::Chat;
use crate::orch::{self, OrchCommand, OrchLocal, Outcome};

impl App {
    /// Every live and buried pane, in window, tab and layout order, as the snapshot names it:
    /// by stable id only (ruling P3).
    pub(super) fn orch_facts(&self) -> Vec<orch::PaneFacts> {
        let world = self.cockpit_world();
        let now = super::warnings_app::unix_now();
        world
            .windows
            .iter()
            .flat_map(|w| &w.tabs)
            .flat_map(|t| &t.sessions)
            .chain(&world.buried)
            .chain(&world.hidden)
            .filter_map(|s| {
                let screen = self.panes.get(&s.pane)?.own();
                let changed = screen.newest_visible_change().map(|t| now - f64::from(t)).filter(|age| *age >= 0.0);
                Some(orch::PaneFacts {
                    stable_id: s.stable_id.clone(),
                    name: s.name.clone(),
                    status: s.status.clone(),
                    ancestors: s.ancestors.clone(),
                    alt_screen: s.alt_screen,
                    job: screen.shell.job.as_ref().map(|j| j.name.clone()),
                    changed_secs_ago: changed,
                    provenance: self.orch_provenance.get(&s.pane).cloned(),
                })
            })
            .collect()
    }

    /// Every workgroup the model can address (`allWorkgroups`): the live instances with their
    /// members' roles, then a synthetic one for each session in none.
    pub(super) fn orch_groups(&self) -> Vec<orch::WorkgroupSummary> {
        orch::all_workgroups(&self.orch_live_groups(), &self.orch_facts())
    }

    /// The live instances with who started each entry.
    pub(super) fn orch_live_groups(&self) -> Vec<(crate::cockpit_rows::WorkgroupFact, Option<String>)> {
        self.workgroup_world().into_iter().map(|w| { let p = self.workgroups.get(&w.id).map(|i| i.provenance.clone()); (w, p) }).collect()
    }

    /// The workgroup and role a session answers to: (workgroup id, workgroup name, role id, role
    /// name), the instance's for a member, else its synthetic one.
    pub(super) fn orch_identity(&self, stable_id: &str) -> Option<(String, String, String, String)> {
        let groups = self.orch_groups();
        groups.into_iter().find_map(|g| g.sessions.iter().find(|s| s.session_guid == stable_id).map(|s| (g.workgroup_id.clone(), g.workgroup_name.clone(), s.role_id.clone(), s.role_name.clone())))
    }

    /// The trailing message of an orchestration chat's every request: the workgroups snapshot,
    /// said to be data and cut to a size (it holds text programs wrote). None for any other chat.
    pub(super) fn orch_volatile(&self, chat: &Chat) -> Option<String> {
        orch::volatile_for(chat.orchestration_enabled, || self.orch_groups())
    }

    /// The engine returned an orchestration call: run it, and answer the model at once unless it
    /// waits (on the person, a safety check or a mirror).
    pub(super) fn orch_effect(&mut self, chat: &str, call_id: &str, name: &str, args: &serde_json::Value) {
        let o = self.orch_call(chat, call_id, name, args);
        self.orch_settle(chat, call_id, o);
    }

    /// One orchestration call: the enable tool here, every other tool in `orch_dispatch`.
    pub(super) fn orch_call(&mut self, chat_id: &str, call_id: &str, name: &str, args: &serde_json::Value) -> Outcome {
        // The watch tools work in every mode (a session-bound chat has them too).
        if orch::is_watch_tool(name) {
            return self.orch_watch_call(chat_id, call_id, name, args);
        }
        if name != orch::ENABLE_TOOL {
            return self.orch_dispatch(chat_id, call_id, name, args);
        }
        // Asked in a chat that already orchestrates: nothing to ask.
        if self.chat_row(chat_id).is_some_and(|c| c.orchestration_enabled) {
            return Outcome::Done(orch::APPROVED.to_string());
        }
        let request_id = crate::chat_message::new_id();
        orch::park(&mut self.orch_parks, &request_id, chat_id, call_id);
        let local = ClientLocal::Other(OrchLocal::EnableRequest { request_id: request_id.clone() }.to_value());
        let m = Message::new(chat_id, Participant::Agent, Content::ClientLocal(local), super::warnings_app::unix_now());
        match self.chat_publish(chat_id, m, false) {
            Ok(()) => Outcome::Parked,
            Err(e) => {
                orch::take_park(&mut self.orch_parks, &request_id, chat_id);
                Outcome::Done(orch::surface_failed(&e))
            }
        }
    }

    /// The call's answer goes to the model, after the turn's gate (the engine checks it).
    pub(super) fn orch_resume(&mut self, chat_id: &str, call_id: &str, result: ToolResult) {
        self.ai_refresh_volatile(chat_id);
        let fx = self.engine(|cx| Engine::orch_done(cx, chat_id, call_id, result)).unwrap_or_default();
        self.chat_apply(fx);
    }

    /// Switches a chat's orchestration mode, only on the person's own act (the Enable button, the
    /// Info menu, an approved enable request). Turning it on needs the AI gate open; the chat's
    /// linked terminal and browser sessions are detached, its command streaming stops and the flag
    /// is stored before anything else changes. Turning it off stops the chat's turn first.
    pub(super) fn set_orchestration(&mut self, chat_id: &str, on: bool) -> Result<(), String> {
        let Some(row) = self.chat_row(chat_id) else { return Err("The chat is gone.".to_string()) };
        if row.orchestration_enabled == on {
            return Ok(());
        }
        if on {
            let check = crate::ai_gate::check_endpoint(&self.globals, None);
            if !orch::enable_allowed(&check) {
                // The gate's own alert says what to switch on.
                self.ai_check_endpoint(None, false);
                return Err("AI features are turned off.".to_string());
            }
            // A turn running under the old mode ends, unless it is the one asking to switch.
            if self.chat_agents.get(chat_id).is_some_and(|a| a.in_turn && a.orch_pending.is_none()) {
                self.chat_stop(chat_id);
            }
            if row.terminal_session_guid.is_some() {
                self.unlink_chat(chat_id, true);
            }
            if row.browser_session_guid.is_some() {
                self.unlink_chat(chat_id, false);
            }
            self.stop_streaming(chat_id);
            self.ai_watermarks.remove(chat_id);
        } else {
            self.chat_stop(chat_id);
            self.orch_chat_ended(chat_id);
        }
        if !self.chat_update(chat_id, |c| c.orchestration_enabled = on) {
            return Err("The chat could not be saved.".to_string());
        }
        self.chat_refresh();
        self.chat_state_changed();
        Ok(())
    }

    /// The person's answer to an orchestration bubble. An answer for a request that is not
    /// parked for this chat (answered already, stopped, forged, another chat's) does nothing.
    pub(super) fn orch_command(&mut self, chat_id: &str, command: OrchCommand) {
        match command {
            OrchCommand::EnableResponse { request_id, approved } => {
                let Some(call_id) = orch::take_park(&mut self.orch_parks, &request_id, chat_id) else { return };
                if let Some(p) = self.chat_agents.get_mut(chat_id).and_then(|a| a.orch_pending.as_mut()) {
                    p.1 = false;
                }
                let text = match (approved, if approved { self.set_orchestration(chat_id, true) } else { Ok(()) }) {
                    (true, Ok(())) => orch::APPROVED.to_string(),
                    (true, Err(e)) => orch::persist_failed(&e),
                    (false, _) => orch::DECLINED.to_string(),
                };
                self.orch_resume(chat_id, &call_id, ToolResult::Ok(text));
            }
            OrchCommand::PermissionResponse { request_id, approved } => self.orch_claim_answered(chat_id, &request_id, approved),
            // Revoke drops the claim; the next write there asks again (`revokeClaim`, `DI:1796-1814`).
            OrchCommand::Revoke { scope } => {
                let mut removed = false;
                self.chat_update(chat_id, |c| {
                    let before = c.claimed_scopes.len();
                    c.claimed_scopes.retain(|s| *s != scope);
                    removed = c.claimed_scopes.len() != before;
                });
                if removed {
                    let name = self.orch_scope_name(&scope);
                    self.notice(chat_id, &crate::orch_tools::revoked_text(&name));
                }
                self.chat_buttons_refresh();
            }
        }
    }

    /// Everything orchestration parked for `chat` ends with its turn (ruling P7): Stop, Enable
    /// generative AI features off, the mode off and a deleted chat all come here (the engine's
    /// halt answers `CancelChecks`). The checks, delayed CRs, mirrors and spawns go too.
    pub(super) fn orch_chat_ended(&mut self, chat: &str) {
        self.orch_drop_chat(chat);
        self.orch_watch_drop_chat(chat);
        orch::drop_chat_parks(&mut self.orch_parks, chat);
        self.chat_buttons_refresh();
    }
}
