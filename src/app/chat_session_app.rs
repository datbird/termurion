//! Chat buttons, consent answers and the session picker in the app (`ChatViewController`'s
//! `buttonClicked` handlers, `linkSession`, `SessionSelector`). A press becomes one of 12c's
//! public answers or one of 12d's own actions; nothing here writes an answer to the model. The
//! chat's session reference and its live pane are `App::{session_ref, ref_pane}` (12c's file,
//! the only conversions).

use super::*;
use crate::ai_permissions::{Category, Permission};
use crate::chat_agent::Agent;
use crate::chat_buttons::{self, Answer, Button, EnableFacts, Press};
use crate::chat_message::{ClientLocal, Content, Message};
use crate::chat_store::Chat;
use crate::chat_view;
use super::chat_engine::Choice;

/// A pick in progress: which chat asked, for which kind of session, and what follows it.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Pick {
    pub chat: String,
    pub terminal: bool,
    pub then: PickThen,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum PickThen {
    /// Link the chosen session (the Chat Info menu's "Link Terminal Session").
    #[allow(dead_code)] // Made by Task 6's "Link Terminal Session".
    Link,
    /// Answer a parked select-session request (its original request id).
    Request(String),
}

/// The button's answer as 12c's `Choice`, one to one.
pub(super) fn choice_of(a: Answer) -> Choice {
    match a {
        Answer::AllowOnce => Choice::AllowOnce,
        Answer::AlwaysAllow => Choice::AlwaysAllow,
        Answer::DenyOnce => Choice::DenyOnce,
        Answer::AlwaysDeny => Choice::AlwaysDeny,
    }
}

/// A pick for a request the chat's agent no longer holds (Stop, a turn that ended) has nobody
/// to answer: it is dropped (ruling F29). A pick that only links never goes stale.
pub(super) fn pick_stale(pick: &Pick, agent: Option<&Agent>) -> bool {
    match &pick.then {
        PickThen::Link => false,
        PickThen::Request(id) => !agent.is_some_and(|a| a.parks.iter().any(|p| p.request_id == *id)),
    }
}

/// What the person sees when a link cannot be made.
const NOT_LIVE: &str = "That session is no longer open.";
const NOT_LINKED: &str = "The chat could not be linked.";

impl App {
    /// `linkSession`'s tail (`CVC:1277-1306`): the chat stores the pane's reference, the person
    /// is told, the permissions bubble shows and the allowed set is published. The one link path:
    /// the picker, the offer's Link button and 12c's select-session answer all come here. A pane
    /// that is not live (or every pane, while links are off for this run) links nothing.
    pub(super) fn link_chat(&mut self, chat: &str, pane: PaneId) -> Result<(), String> {
        let Some(reference) = self.session_ref(pane) else { return Err(NOT_LIVE.to_string()) };
        if self.chat_db().is_none() {
            return Err(NOT_LINKED.to_string());
        }
        let stored = reference.clone();
        if !self.chat_update(chat, |c| c.terminal_session_guid = Some(stored)) {
            return Err(NOT_LINKED.to_string());
        }
        // A new session starts from no watermark: "the screen is unchanged" must not carry over
        // from the old one (ruling F34).
        self.ai_watermarks.remove(chat);
        let name = self.window_title(pane);
        self.notice(chat, &chat_buttons::linked_notice(true, Some(name.as_str()).filter(|n| !n.trim().is_empty())));
        self.client_local(chat, ClientLocal::Permissions { terminal: true, guid: reference });
        self.ai_publish_permissions(chat);
        self.chat_state_changed();
        Ok(())
    }

    /// Something the buttons' rules read changed (a link, a pick, a permission, a pane): the
    /// window shows the buttons as they now stand, and every terminal window draws the picker
    /// or forgets it.
    pub(super) fn chat_state_changed(&mut self) {
        self.chat_window_refresh();
        self.redraw();
        self.redraw_all();
    }

    /// Starts a pick (`SessionSelector.select`, `CVC:1222-1275`): an earlier pick is replaced,
    /// the chat shows "Waiting for a session", and every terminal pane of every window shows
    /// the picker's button. A browser pick finds no browser pane before milestone 13, so it
    /// shows none, and Cancel or Escape ends it.
    pub(super) fn start_pick(&mut self, chat: &str, terminal: bool, then: PickThen) {
        self.chat_picker = Some(Pick { chat: chat.to_string(), terminal, then });
        self.client_local(chat, ClientLocal::PickingSession);
        self.chat_state_changed();
    }

    /// The pick's end: a pane (a press on its button) or None (Cancel, Escape, no pick). A pane
    /// that closed meanwhile counts as no pane. Linking is `link_chat`; a select-session request
    /// goes to 12c's `ai_session_selected`, which links and publishes the request again, or
    /// answers the decline.
    pub(super) fn picked(&mut self, pane: Option<PaneId>) {
        let Some(pick) = self.chat_picker.take() else { return };
        let pane = pane.filter(|p| self.session_ref(*p).is_some());
        match pick.then {
            PickThen::Link => {
                if let Some(pane) = pane {
                    if let Err(e) = self.link_chat(&pick.chat, pane) {
                        self.banners.set("chat_link", e);
                    }
                }
            }
            PickThen::Request(id) => self.ai_session_selected(&pick.chat, &id, pane),
        }
        self.chat_state_changed();
    }

    /// A pick nobody can answer any more is dropped (ruling F29): its request was stopped or
    /// answered, so the picker's buttons must not stay on every pane.
    pub(super) fn pick_tick(&mut self) {
        let stale = self.chat_picker.as_ref().is_some_and(|p| pick_stale(p, self.chat_agents.get(&p.chat)));
        if stale {
            self.chat_picker = None;
            self.chat_state_changed();
        }
    }

    /// The window closed: a pick for a request still parked is declined (F29). A pick that only
    /// links stays, for Escape.
    pub(super) fn pick_window_closed(&mut self) {
        let parked = self.chat_picker.as_ref().is_some_and(|p| matches!(p.then, PickThen::Request(_)) && !pick_stale(p, self.chat_agents.get(&p.chat)));
        if parked {
            self.picked(None);
        }
    }

    /// The picker's button on a terminal pane while a pick for a terminal is open; answers the
    /// button's rectangle, which the frame keeps as this window's hits (`picker_hits`).
    pub(super) fn draw_session_picker(&self, frame: &mut Frame, id: PaneId, rect: Rect, cell: (f32, f32)) -> Option<(PaneId, Rect)> {
        if !self.chat_picker.as_ref().is_some_and(|p| p.terminal) {
            return None;
        }
        let p = self.panes.get(&id)?;
        let (fg, bg) = (p.own().palette.fg, p.own().palette.bg);
        Some((id, scene::session_picker_button(frame, rect, chat_buttons::PICK_REASON, fg, bg, cell, self.scale())))
    }

    /// A left press: on a picker's button it picks that pane; anywhere else the pick keeps
    /// waiting and the press goes on. The hits are this window's, from its last frame.
    pub(super) fn picker_press(&mut self, x: f32, y: f32) -> bool {
        if self.chat_picker.is_none() {
            return false;
        }
        let Some(&(pane, _)) = self.picker_hits.iter().find(|(_, r)| r.contains(x, y)) else { return false };
        self.picked(Some(pane));
        true
    }

    /// Escape in a terminal window ends a pick (`picked(None)`).
    pub(super) fn picker_key(&mut self, ev: &KeyEvent) -> bool {
        use winit::keyboard::{Key, NamedKey};
        if self.chat_picker.is_none() || ev.state != ElementState::Pressed || !matches!(ev.logical_key, Key::Named(NamedKey::Escape)) {
            return false;
        }
        self.picked(None);
        true
    }

    /// The chat's permission for a category as its Permissions bubble shows it: the chat's own
    /// override in the bubble's session, else the global default.
    fn chat_permission_in(&self, chat: &str, content: &Content, c: Category) -> Permission {
        let Content::ClientLocal(ClientLocal::Permissions { guid, .. }) = content else { return Permission::Ask };
        let global = |c: Category| self.ai_global_permission(c);
        self.ai_overrides.get(chat, guid, c, &global)
    }

    /// Whether a command of the chat is running (ruling F30): the agent's running tool, or a
    /// Get-Help on the chat's linked pane.
    fn chat_executing(&self, chat: &str, row: Option<&Chat>) -> bool {
        let pane = row.and_then(|r| r.terminal_session_guid.as_deref()).and_then(|g| self.ref_pane(g));
        self.chat_agents.get(chat).is_some_and(|a| a.running.is_some()) || pane.and_then(|p| self.ai_pending_tools.get(&p)).is_some_and(|(c, _)| c == chat)
    }

    /// Whether the chat is streaming finished commands (Task 6 keeps the state).
    fn chat_streaming(&self, _chat: &str) -> bool {
        // Filled by Task 6.
        false
    }

    /// The live facts for one message's rules (`CVC:1962-2050`).
    fn chat_enable_facts(&self, chat: &str, row: Option<&Chat>, msgs: &[Message], m: &Message, is_last: bool) -> EnableFacts {
        let live = |g: &str| self.ref_pane(g).is_some();
        let mut f = EnableFacts {
            is_last,
            picking: self.chat_picker.as_ref().is_some_and(|p| p.chat == chat),
            executing: self.chat_executing(chat, row),
            streaming: self.chat_streaming(chat),
            linked: row.is_some_and(|r| r.terminal_session_guid.is_some() || r.browser_session_guid.is_some()),
            orchestrating: row.is_some_and(|r| r.orchestration_enabled),
            ..EnableFacts::default()
        };
        match &m.content {
            Content::ClientLocal(ClientLocal::OfferLink { guid, .. }) => f.offer_session_live = live(guid),
            Content::ClientLocal(ClientLocal::Permissions { guid, .. }) => f.permissions_session_live = live(guid),
            Content::ClientLocal(ClientLocal::Other(v)) => {
                let scope = crate::chat_message::one_key(v).filter(|(k, _)| *k == "orchestrationPermissionGranted").and_then(|(_, b)| b.get("scope")?.as_str());
                f.claimed = scope.is_some_and(|s| row.is_some_and(|r| r.claimed_scopes.iter().any(|c| c == s)));
            }
            Content::RemoteCommandRequest { .. } => f.answered = chat_buttons::answered(msgs, &m.unique_id),
            _ => {}
        }
        f
    }

    /// One message's buttons and whether they can be pressed now, for the shown chat's rows.
    pub(super) fn chat_row_buttons(&self, chat: &str, row: Option<&Chat>, msgs: &[Message], m: &Message, is_last: bool) -> (Vec<Button>, bool) {
        let buttons = chat_buttons::buttons(&m.content, &|c| self.chat_permission_in(chat, &m.content, c));
        if buttons.is_empty() {
            return (buttons, false);
        }
        let facts = self.chat_enable_facts(chat, row, msgs, m, is_last);
        (buttons, chat_buttons::enabled(&m.content, &facts))
    }

    /// The rows' buttons again from the live state, when some differ (a pick began, a pane
    /// closed, a permission changed): only the rows that have buttons are asked.
    pub(super) fn chat_buttons_refresh(&mut self) {
        let Some(chat) = self.chat_shown.clone() else { return };
        if !self.chat_convo.iter().any(|r| !r.buttons.is_empty()) {
            return;
        }
        let row = self.chat_db.as_ref().and_then(|db| db.chat(&chat).ok().flatten());
        let visible = chat_view::visible(&self.chat_msgs);
        let last = visible.last().map(|m| m.unique_id.clone());
        let now: Vec<Option<(Vec<Button>, bool)>> = self
            .chat_convo
            .iter()
            .map(|r| {
                if r.buttons.is_empty() {
                    return None;
                }
                let m = visible.iter().find(|m| m.unique_id == r.id)?;
                Some(self.chat_row_buttons(&chat, row.as_ref(), &self.chat_msgs, m, last.as_deref() == Some(r.id.as_str())))
            })
            .collect();
        let differs = self.chat_convo.iter().zip(&now).any(|(r, n)| n.as_ref().is_some_and(|(b, e)| *b != r.buttons || *e != r.enabled));
        if differs {
            let mut rows = (*self.chat_convo).clone();
            for (r, n) in rows.iter_mut().zip(now) {
                if let Some((b, e)) = n {
                    r.buttons = b;
                    r.enabled = e;
                }
            }
            self.chat_convo = std::sync::Arc::new(rows);
        }
    }

    /// A button under a message was pressed (`buttonClicked`, `CVC:1740-1905`). The click names
    /// a chat, a message and a button; everything else is read from the store now: the message
    /// must be in that chat, the button one it shows, and its buttons enabled at this moment,
    /// so a stale click, a superseded request or a forged event does nothing. Then the press
    /// becomes one of 12c's answers or one of 12d's own actions; no message to the model is
    /// written here.
    pub(super) fn chat_button_pressed(&mut self, chat: &str, message: &str, button: &str, _event_loop: &ActiveEventLoop) {
        let Some(db) = self.chat_db.as_ref() else { return };
        let (Ok(msgs), Ok(row)) = (db.messages(chat), db.chat(chat)) else { return };
        let Some(m) = msgs.iter().find(|m| m.unique_id == message) else { return };
        let is_last = chat_view::visible(&msgs).last().is_some_and(|v| v.unique_id == message);
        let facts = self.chat_enable_facts(chat, row.as_ref(), &msgs, m, is_last);
        let on = chat_buttons::enabled(&m.content, &facts);
        let Some(p) = chat_buttons::press(&m.content, &|c| self.chat_permission_in(chat, &m.content, c), on, button) else { return };
        match p {
            Press::CancelPick => self.picked(None),
            Press::CancelTool => self.ai_cancel_tool(chat),
            Press::StopStreaming => self.stop_streaming(chat),
            Press::Link { terminal, guid } => {
                // The offered session, if it is still open; a browser offer has none before 13.
                if let (true, Some(pane)) = (terminal, self.ref_pane(&guid)) {
                    if let Err(e) = self.link_chat(chat, pane) {
                        self.banners.set("chat_link", e);
                    }
                }
            }
            Press::Permission(c) => self.toggle_permission(chat, c),
            Press::Decline { request } => self.ai_session_selected(chat, &request, None),
            Press::PickSession { terminal, request } => self.start_pick(chat, terminal, PickThen::Request(request)),
            Press::Answer(a) => self.ai_answer_request(chat, message, choice_of(a)),
        }
        self.chat_state_changed();
    }

    /// A permissions button: the category cycles for the linked session and Confirm Change asks
    /// where iTerm2 asks (filled by Task 6, ruling F24).
    pub(super) fn toggle_permission(&mut self, _chat: &str, _category: Category) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat_agent::Park;

    fn park(request: &str) -> Park {
        Park { call_id: "c".into(), request_id: request.into(), call: crate::ai_tools::Call::IsAtPrompt, safe: None, cleared_typing: false, guid: String::new(), send: None }
    }

    #[test]
    fn answers_map_one_to_one_onto_12c_choices() {
        assert_eq!(choice_of(Answer::AllowOnce), Choice::AllowOnce);
        assert_eq!(choice_of(Answer::AlwaysAllow), Choice::AlwaysAllow);
        assert_eq!(choice_of(Answer::DenyOnce), Choice::DenyOnce);
        assert_eq!(choice_of(Answer::AlwaysDeny), Choice::AlwaysDeny);
    }

    #[test]
    fn a_pick_for_a_request_that_is_gone_is_stale() {
        let request = Pick { chat: "c".into(), terminal: true, then: PickThen::Request("R".into()) };
        let mut agent = Agent::default();
        assert!(pick_stale(&request, Some(&agent)), "no park");
        assert!(pick_stale(&request, None), "no agent");
        agent.parks.push(park("R"));
        assert!(!pick_stale(&request, Some(&agent)), "still parked");
        agent.parks = vec![park("OTHER")];
        assert!(pick_stale(&request, Some(&agent)), "another request is parked");
        let link = Pick { chat: "c".into(), terminal: true, then: PickThen::Link };
        assert!(!pick_stale(&link, None), "a link pick has no request to lose");
    }
}
