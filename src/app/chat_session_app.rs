//! Chat buttons, consent answers and the session picker in the app (`ChatViewController`'s
//! `buttonClicked` handlers, `linkSession`, `SessionSelector`). A press becomes one of 12c's
//! public answers or one of 12d's own actions; nothing here writes an answer to the model. The
//! chat's session reference and its live pane are `App::{session_ref, ref_pane}` (12c's file,
//! the only conversions).

use super::*;
use crate::ai_permissions::{Category, Permission};
use crate::chat_agent::Agent;
use crate::chat_buttons::{self, Answer, Button, EnableFacts, Press};
use crate::chat_menu::{self, Entry, MenuFacts, Tri};
use crate::chat_message::{ClientLocal, Content, Message, Streaming, TerminalCommand};
use crate::chat_routes::{self, Route};
use crate::chat_store::Chat;
use crate::chat_view;
use super::chat_engine::Choice;
use crate::scene::IndicatorKind;

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

/// What ending a pick does.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Outcome {
    Nothing,
    /// Link the chat to the chosen pane.
    Link { chat: String, pane: PaneId },
    /// Answer a parked select-session request: with the pane, or None to decline it.
    Answer { chat: String, request: String, pane: Option<PaneId> },
}

/// The end of `pick` with `pane` chosen, or None for Cancel, Escape and no pane.
pub(super) fn pick_outcome(pick: &Pick, pane: Option<PaneId>) -> Outcome {
    match (&pick.then, pane) {
        (PickThen::Link, Some(pane)) => Outcome::Link { chat: pick.chat.clone(), pane },
        (PickThen::Link, None) => Outcome::Nothing,
        (PickThen::Request(id), pane) => Outcome::Answer { chat: pick.chat.clone(), request: id.clone(), pane },
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
        self.chat_refresh();
        self.chat_state_changed();
        Ok(())
    }

    /// Something the buttons' rules read changed (a link, a pick, a permission, a pane): the
    /// window shows the buttons as they now stand, and every terminal window draws the picker
    /// or forgets it.
    pub(super) fn chat_state_changed(&mut self) {
        // The indicators read the links again on this very redraw (F33).
        self.chat_links_stale();
        self.chat_window_refresh();
        self.redraw();
        self.redraw_all();
    }

    /// Starts a pick (`SessionSelector.select`, `CVC:1222-1275`): an earlier pick is replaced,
    /// the chat shows "Waiting for a session", and every terminal pane of every window shows
    /// the picker's button. A browser pick finds no browser pane before milestone 13, so it
    /// shows none, and Cancel or Escape ends it.
    pub(super) fn start_pick(&mut self, chat: &str, terminal: bool, then: PickThen) {
        // An open pick ends first, with no pane: its request is declined and its turn goes on.
        if self.chat_picker.is_some() {
            self.picked(None);
        }
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
        match pick_outcome(&pick, pane) {
            Outcome::Nothing => {}
            Outcome::Link { chat, pane } => {
                if let Err(e) = self.link_chat(&chat, pane) {
                    self.banners.set("chat_link", e);
                }
            }
            Outcome::Answer { chat, request, pane } => self.ai_session_selected(&chat, &request, pane),
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

    /// Whether the chat is streaming finished commands (`streaming`, per chat and app-wide).
    fn chat_streaming(&self, chat: &str) -> bool {
        self.streaming_chats.contains(chat)
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

    /// The chat's stored row now (never a copy read earlier).
    fn chat_row(&self, chat: &str) -> Option<Chat> {
        self.chat_db.as_ref()?.chat(chat).ok().flatten()
    }

    /// The chat's linked terminal, as its reference and its live pane: None when it is not linked
    /// or the session is not open.
    fn chat_live_link(&self, chat: &str) -> Option<(String, PaneId)> {
        let guid = self.chat_row(chat)?.terminal_session_guid?;
        let pane = self.ref_pane(&guid)?;
        Some((guid, pane))
    }

    /// A permissions button or menu item (`toggle(permissionCategory:)`, `CVC:1150-1204`): the
    /// category cycles Always, Never, Ask for the linked live session, and the two autopopulated
    /// categories ask Confirm Change before Always. Nothing happens for a chat with no live link.
    pub(super) fn toggle_permission(&mut self, chat: &str, category: Category) {
        if category.is_browser() {
            return; // No browser pane exists before milestone 13.
        }
        let Some((guid, _)) = self.chat_live_link(chat) else { return };
        let global = |c: Category| self.ai_global_permission(c);
        let current = self.ai_overrides.get(chat, &guid, category, &global);
        match chat_menu::toggle(category, current) {
            chat_menu::Toggle::Set(p) => self.ai_set_permission(chat, &guid, category, p),
            chat_menu::Toggle::Confirm(warning) => self.ask_ai(chat_menu::confirm_alert(chat, &guid, category, warning)),
        }
        self.chat_state_changed();
    }

    /// Confirm Change's answer. It applies only while the chat is still linked to the live
    /// session the question named (a stale answer changes and grants nothing). Send Automatically
    /// is the informed consent, recorded globally, and it clears the queued one-time question
    /// that asked the same (ruling F31).
    pub(super) fn confirm_change_answered(&mut self, chat: &str, guid: &str, category: Category, index: usize) {
        let linked = self.chat_row(chat).and_then(|r| r.terminal_session_guid);
        let live = self.ref_pane(guid).is_some();
        let Some((permission, grants)) = chat_menu::confirm_outcome(index, linked.as_deref(), guid, live) else { return };
        if let Some(consent) = chat_menu::consent_write(grants) {
            self.globals.set(crate::ai_context::CONSENT_KEY, toml::Value::Integer(consent.raw()));
            self.save_globals();
            chat_menu::drop_queued_consent(&mut self.ai_asks);
        }
        self.ai_set_permission(chat, guid, category, permission);
        self.chat_state_changed();
    }

    /// Unlinks the chat's terminal (or browser) session (`unlinkTerminalSession`, `CVC:1403-1435`):
    /// streaming ends, the notice shows and the allowed set is published again. Watchers are
    /// 12f's.
    pub(super) fn unlink_chat(&mut self, chat: &str, terminal: bool) {
        if terminal {
            self.stop_streaming(chat);
        }
        let changed = self.chat_update(chat, |c| {
            if terminal {
                c.terminal_session_guid = None;
            } else {
                c.browser_session_guid = None;
            }
        });
        if !changed {
            return;
        }
        // The next link starts from no watermark (ruling F34).
        self.ai_watermarks.remove(chat);
        self.notice(chat, if terminal { chat_menu::UNLINKED_TERMINAL } else { chat_menu::UNLINKED_BROWSER });
        self.ai_publish_permissions(chat);
        self.chat_refresh();
        self.chat_state_changed();
    }

    /// Reveal Linked Terminal Session.
    fn reveal_chat_session(&mut self, chat: &str) {
        if let Some((_, pane)) = self.chat_live_link(chat) {
            self.reveal_pane(pane);
        }
    }

    /// Put Chat in Linked Terminal Session (`CVC:1379-1401`): the panel of the linked pane
    /// shows this chat. Nothing when the chat is not linked to a live session.
    pub(super) fn put_chat_in_session(&mut self, chat: &str) {
        if let Some((_, pane)) = self.chat_live_link(chat) {
            self.bind_inline(pane, chat, true);
        }
    }

    /// Send Commands & Output to AI Automatically (`toggleStream`, `CVC:1335-1357`): only for a
    /// linked live terminal. Turning it on asks the Privacy Warning first; turning it off is
    /// `stop_streaming`.
    pub(super) fn toggle_stream(&mut self, chat: &str) {
        let Some((guid, _)) = self.chat_live_link(chat) else { return };
        if self.streaming_chats.contains(chat) {
            self.stop_streaming(chat);
        } else {
            self.ask_ai(chat_menu::privacy_alert(chat, &guid));
        }
    }

    /// The Privacy Warning's OK: streaming starts for the chat's linked pane, if it is still
    /// linked to the session the question named and that session is open.
    pub(super) fn start_streaming(&mut self, chat: &str, guid: &str) {
        let link = self.chat_live_link(chat);
        match chat_menu::start_streaming_decision(link.as_ref().map(|(g, _)| g.as_str()), guid, link.is_some(), self.streaming_chats.contains(chat)) {
            chat_menu::StartStreaming::Already => return,
            // The answer belongs to a link that is gone: nothing starts, and the chat says so.
            chat_menu::StartStreaming::Stale => return self.notice(chat, chat_menu::STREAM_STALE_NOTICE),
            chat_menu::StartStreaming::Start => {}
        }
        self.streaming_chats.insert(chat.to_string());
        self.chat_refresh();
        self.client_local(chat, ClientLocal::StreamingChanged(Streaming::Active));
        self.chat_state_changed();
    }

    /// Ends the chat's streaming (`stopStreaming`, `CVC:1320-1330`): the stop is published when
    /// it was on, and the pane stops queueing command ends unless another chat streams it.
    pub(super) fn stop_streaming(&mut self, chat: &str) {
        if !self.streaming_chats.remove(chat) {
            return;
        }
        self.chat_refresh();
        self.client_local(chat, ClientLocal::StreamingChanged(Streaming::Stopped));
        self.chat_window_refresh();
        self.redraw_all();
    }

    /// What a change of links, streams or panes refreshes: the panes' watch flags and the
    /// indicators' link cache. Every such site calls this (a restart or a relink moves the watch,
    /// `CVC:144-165`, and the glyph never lies for a second).
    pub(super) fn chat_refresh(&mut self) {
        self.sync_chat_watch();
        self.chat_links_stale();
    }

    /// Every pane's `chat_watch` follows the streaming chats' current links.
    pub(super) fn sync_chat_watch(&mut self) {
        let links: Vec<(String, Option<PaneId>)> = self.streaming_chats.iter().map(|c| (c.clone(), self.chat_live_link(c).map(|(_, pane)| pane))).collect();
        let wanted = chat_menu::watched_panes(links.iter().map(|(c, p)| (c.as_str(), *p)), &self.streaming_chats);
        for (id, p) in self.panes.iter_mut() {
            let on = wanted.contains(id);
            if p.own().chat_watch != on {
                p.own_mut().chat_watch = on;
            }
        }
    }

    /// A finished command of a streamed session goes to every chat streaming it
    /// (`streamLastCommand`, `CVC:2401-2455`): its command, directory, host and user, the code,
    /// its own output while it is still in the buffer (else a sentence saying so, ruling F32), and
    /// a reveal-mark link. A command with no text sends nothing.
    pub(super) fn stream_command(&mut self, pane: PaneId, code: i32, mark: Option<u32>) {
        if self.streaming_chats.is_empty() {
            return;
        }
        let Some(reference) = self.session_ref(pane) else { return };
        let links: Vec<(String, String)> = self.streaming_chats.iter().filter_map(|c| Some((c.clone(), self.chat_row(c)?.terminal_session_guid?))).collect();
        let targets = chat_menu::stream_targets(links.iter().map(|(c, g)| (c.as_str(), Some(g.as_str()))), &reference);
        if targets.is_empty() {
            return;
        }
        let Some(f) = self.panes.get(&pane).and_then(|p| p.own().finished_command(mark)) else { return };
        let command = TerminalCommand {
            username: f.user,
            hostname: f.host,
            directory: f.directory,
            command: f.command,
            output: f.output.unwrap_or_else(|| chat_menu::OUTPUT_GONE.to_string()),
            exit_code: code,
            url: chat_routes::mark_url(&reference, f.mark),
        };
        for chat in targets {
            self.ai_send_terminal_command(&chat, command.clone());
        }
    }

    /// A pane is closing (`sessionWillTerminate`, `CVC:241-256`): every chat linked to it unlinks
    /// with the notice and stops streaming, and a picker button on it is dropped. Called before
    /// the pane leaves, while its reference still names it.
    pub(super) fn chat_pane_gone(&mut self, pane: PaneId) {
        let Some(reference) = self.session_ref(pane) else { return };
        self.chat_reference_gone(pane, &reference);
    }

    /// The session `reference` (pane `pane`) is gone, for a close or for a restart that could not
    /// start its shell: its chats unlink with the notice and stop streaming.
    pub(super) fn chat_reference_gone(&mut self, pane: PaneId, reference: &str) {
        self.picker_hits.retain(|(p, _)| *p != pane);
        let linked = self.chat_db.as_ref().and_then(|db| db.linked_sessions().ok()).unwrap_or_default();
        for chat in chat_menu::chats_linked_to(&linked, reference) {
            self.unlink_chat(&chat, true);
        }
        self.chat_refresh();
    }

    /// The links the indicators read, refreshed at most once a second unless something changed
    /// them (a draw asks every pane every frame).
    fn chat_links(&self) -> Vec<(String, String)> {
        let mut cache = self.chat_links.borrow_mut();
        if let Some((at, links)) = cache.as_ref() {
            if at.elapsed() < Duration::from_secs(1) {
                return links.clone();
            }
        }
        let links = self.chat_db.as_ref().and_then(|db| db.linked_sessions().ok()).unwrap_or_default();
        *cache = Some((Instant::now(), links.clone()));
        links
    }

    /// The indicators read the links again on the next frame.
    pub(super) fn chat_links_stale(&self) {
        *self.chat_links.borrow_mut() = None;
    }

    /// Whether a chat links the pane, and whether a chat streaming it does.
    pub(super) fn chat_pane_state(&self, pane: PaneId) -> (bool, bool) {
        let Some(reference) = self.session_ref(pane) else { return (false, false) };
        let links = self.chat_links();
        let linked = links.iter().any(|(_, g)| *g == reference);
        let streaming = links.iter().any(|(c, g)| *g == reference && self.streaming_chats.contains(c));
        (linked, streaming)
    }

    /// The Chat Info menu at `at` (screen pixels) for the chat, on the chat window or, for a
    /// panel (`inline` names its pane), the terminal window. Ids are `aichat:menu:<verb>:<chat>`;
    /// the menu lives until its pick arrives (`popup_menu`).
    pub(super) fn show_session_menu(&mut self, chat: &str, inline: Option<PaneId>, at: (f32, f32)) {
        use muda::ContextMenu;
        let hwnd = match inline {
            Some(pane) => {
                self.enter_pane_window(pane);
                self.hwnd()
            }
            None => self.chat_window.as_ref().and_then(|w| w.hwnd()),
        };
        let Some(hwnd) = hwnd else { return };
        let Some(row) = self.chat_row(chat) else { return };
        let live = row.terminal_session_guid.as_deref().and_then(|g| self.ref_pane(g).map(|_| g.to_string()));
        let facts = MenuFacts { orchestrating: row.orchestration_enabled, terminal_live: live.is_some(), browser_live: false, inline: inline.is_some(), streaming: self.streaming_chats.contains(chat) };
        let global = |c: Category| self.ai_global_permission(c);
        let perm = |c: Category| match &live {
            Some(g) => self.ai_overrides.get(chat, g, c, &global),
            None => Permission::Ask,
        };
        let entries = chat_menu::session_menu(&facts, &perm);
        let menu = muda::Menu::new();
        let mut results = Vec::new();
        for e in &entries {
            results.push(match e {
                Entry::Separator => menu.append(&muda::PredefinedMenuItem::separator()),
                Entry::Item { title, action, state, enabled } => {
                    let id = chat_menu::menu_id(action, chat);
                    let text = title.replace('&', "&&");
                    match state {
                        None => menu.append(&muda::MenuItem::with_id(id, text, *enabled, None)),
                        Some(Tri::On) => menu.append(&muda::CheckMenuItem::with_id(id, text, *enabled, true, None)),
                        Some(Tri::Off) => menu.append(&muda::CheckMenuItem::with_id(id, text, *enabled, false, None)),
                        Some(Tri::Mixed) => menu.append(&muda::IconMenuItem::with_id(id, text, *enabled, dash_icon(), None)),
                    }
                }
            });
        }
        if let Some(Err(e)) = results.into_iter().find(Result::is_err) {
            self.banners.set("menu", format!("Chat menu: {e}"));
            return;
        }
        let at = muda::dpi::Position::Physical(muda::dpi::PhysicalPosition::new(at.0 as i32, at.1 as i32));
        unsafe { menu.show_context_menu_for_hwnd(hwnd, Some(at)) };
        self.popup_menu = Some(menu);
    }

    /// A pick from the Chat Info menu or the indicator menu. False for other ids.
    pub(super) fn chat_menu_picked(&mut self, id: &str, event_loop: &ActiveEventLoop) -> bool {
        if let Some(n) = id.strip_prefix(INDICATOR_ID) {
            if let (Some((_, routes)), Ok(n)) = (self.chat_indicator_menu.take(), n.parse::<usize>()) {
                if let Some(route) = routes.get(n) {
                    self.run_chat_route(route, event_loop);
                }
            }
            return true;
        }
        if !id.starts_with(chat_menu::MENU_PREFIX) {
            return false;
        }
        let Some((action, chat)) = chat_menu::parse_menu_id(id) else { return true };
        match action {
            chat_menu::Action::Reveal => self.reveal_chat_session(&chat),
            chat_menu::Action::Unlink => self.unlink_chat(&chat, true),
            chat_menu::Action::Put => self.put_chat_in_session(&chat),
            chat_menu::Action::Toggle(c) => self.toggle_permission(&chat, c),
            chat_menu::Action::Stream => self.toggle_stream(&chat),
            chat_menu::Action::Link => self.start_pick(&chat, true, PickThen::Link),
            chat_menu::Action::Help => self.ask_ai(chat_menu::help_alert()),
            // Disabled until 12f and milestone 13; never a pick.
            chat_menu::Action::Orchestration | chat_menu::Action::LinkBrowser => {}
        }
        true
    }

    /// A press on an AI glyph among a pane's indicators: a click menu of the popover's links
    /// (Decision). False for a press anywhere else. The hits are this window's last frame's.
    pub(super) fn chat_indicator_press(&mut self, x: f32, y: f32) -> bool {
        use muda::ContextMenu;
        let Some(&(pane, _, kind)) = self.chat_indicator_hits.iter().find(|(_, r, _)| r.contains(x, y)) else { return false };
        let Some(reference) = self.session_ref(pane) else { return true };
        let routes = match kind {
            IndicatorKind::AiLinked => chat_routes::indicator_routes(true, false, &reference),
            IndicatorKind::AiStreaming => chat_routes::indicator_routes(false, true, &reference),
            IndicatorKind::Other => return false,
        };
        let Some(hwnd) = self.hwnd() else { return true };
        let menu = muda::Menu::new();
        for (i, (label, _)) in routes.iter().enumerate() {
            if let Err(e) = menu.append(&muda::MenuItem::with_id(format!("{INDICATOR_ID}{i}"), label.replace('&', "&&"), true, None)) {
                self.banners.set("menu", format!("Chat menu: {e}"));
                return true;
            }
        }
        self.chat_indicator_menu = Some((pane, routes.into_iter().map(|(_, r)| r).collect()));
        unsafe { menu.show_context_menu_for_hwnd(hwnd, None) };
        self.popup_menu = Some(menu);
        true
    }

    /// Runs a route (minor m10). Only three inputs reach it: the indicator menu's items, a
    /// terminal command cell's reveal-mark link (`from_command_cell` refuses every other route
    /// there) and a link inside an explanation response (`from_explanation_link`: only its
    /// annotation reveal); any other markdown link is never parsed into one. A session that is
    /// not a live pane exactly, by its reference, is nothing.
    pub(super) fn run_chat_route(&mut self, r: &Route, event_loop: &ActiveEventLoop) {
        match r {
            Route::UnlinkSessionChat(session) => {
                for (chat, guid) in self.chat_db.as_ref().and_then(|db| db.linked_sessions().ok()).unwrap_or_default() {
                    if guid == *session {
                        self.unlink_chat(&chat, true);
                    }
                }
            }
            Route::RevealChatForSession(session) => {
                // The most recent chat linked to it, in the chat window.
                let chat = self.chat_db.as_ref().and_then(|db| db.chats().ok()).and_then(|chats| chats.into_iter().find(|c| c.terminal_session_guid.as_deref() == Some(session.as_str())));
                if let Some(c) = chat {
                    if self.chat_window_enter(event_loop) {
                        self.show_chat(Some(c.id));
                    }
                }
            }
            Route::DisableStreaming(session) => {
                let streaming: Vec<String> = self.streaming_chats.iter().filter(|c| self.chat_row(c).and_then(|r| r.terminal_session_guid).as_deref() == Some(session.as_str())).cloned().collect();
                for chat in streaming {
                    self.stop_streaming(&chat);
                }
            }
            Route::RevealAnnotation { session, id } => self.reveal_ai_annotation(session, id, event_loop),
            Route::RevealMark { session, mark } => {
                let (Some(pane), Ok(mark)) = (self.ref_pane(session), mark.parse::<u32>()) else { return };
                self.reveal_pane(pane);
                let line = self.panes.get_mut(&pane).and_then(|p| p.own_mut().marks_in_order().into_iter().find(|(_, m)| m.id == mark).map(|(l, _)| l));
                if let Some(line) = line {
                    self.show_line(pane, line, false);
                }
                self.redraw();
            }
        }
    }

    /// A terminal selection ended (`textViewLiveSelectionDidEnd`, `PTYSession.m:14912-14916`): a
    /// short one in the pane the shown chat links goes to the chat's input when it is still
    /// eligible (`CVC:1060-1067`, `offer_text`).
    pub(super) fn chat_offer_selection(&mut self, pane: PaneId) {
        let Some(chat) = self.chat_shown.clone() else { return };
        let Some(reference) = self.session_ref(pane) else { return };
        if self.chat_row(&chat).and_then(|r| r.terminal_session_guid).as_deref() != Some(reference.as_str()) {
            return;
        }
        let Some(p) = self.panes.get(&pane) else { return };
        let Some(text) = p.own().term().selection_to_string().filter(|t| !t.is_empty()) else { return };
        if chat_menu::short_selection(text.chars().count(), p.own().term().grid().columns()) {
            self.offer_text(&chat, &text);
        }
    }
}

/// The indicator menu's ids are `aichat:ind:<index>` into the routes it was built with.
const INDICATOR_ID: &str = "aichat:ind:";

/// The mixed state's dash (muda has no third check state): a 16 by 16 bar in the menu's ink.
fn dash_icon() -> Option<muda::Icon> {
    let mut px = vec![0u8; 16 * 16 * 4];
    for y in 7..9 {
        for x in 3..13 {
            let i = (y * 16 + x) * 4;
            px[i..i + 4].copy_from_slice(&[0x60, 0x60, 0x60, 0xff]);
        }
    }
    muda::Icon::from_rgba(px, 16, 16).ok()
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
    fn ending_a_pick_with_no_pane_declines_its_request() {
        let request = Pick { chat: "a".into(), terminal: true, then: PickThen::Request("R".into()) };
        let link = Pick { chat: "b".into(), terminal: true, then: PickThen::Link };
        // start_pick ends the open pick with no pane: its request is declined, not left parked.
        assert_eq!(pick_outcome(&request, None), Outcome::Answer { chat: "a".into(), request: "R".into(), pane: None });
        assert_eq!(pick_outcome(&link, None), Outcome::Nothing);
        assert_eq!(pick_outcome(&request, Some(3)), Outcome::Answer { chat: "a".into(), request: "R".into(), pane: Some(3) });
        assert_eq!(pick_outcome(&link, Some(3)), Outcome::Link { chat: "b".into(), pane: 3 });
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
