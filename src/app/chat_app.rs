//! The AI chats' app side: the chat database (opened on first use), the broker, the chats'
//! permissions, applying what a publish decided to the database (`ChatBroker.swift`,
//! `ChatListModel.swift`), and the App's side of the chat engine (`chat_engine.rs`, ruling
//! R12cd-4): applying its effects, among them `ai_run_tool`, the one dispatcher of every tool
//! call, and the live drivers over 12b's runners.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Instant;

use super::chat_engine::{Choice, Ctx, Engine, EngineEffect};
use super::App;
use crate::ai_catalog::Model;
use crate::ai_files::{FileOut, FileRunner, Job};
use crate::ai_gate::{Alert, Then};
use crate::ai_permissions::{self, Category, Key, Overrides, Permission};
use crate::ai_settings::Vendor;
use crate::ai_terminal::Outcome;
use crate::ai_tools::Call;
use crate::ai_transport::{Notify, Options, Transport};
use crate::ai_turn::{Out, Runner, ToolResult, Turn, TurnInput};
use crate::chat_agent::{self, Agent, FileDriver, TurnDriver};
use crate::chat_broker::{Broker, Effect, Step};
use crate::chat_gate::Passed;
use crate::chat_message::{Content, Message, Participant, TerminalCommand};
use crate::chat_store::{self, Chat, Store, OPEN_FAILED, OPEN_FAILED_HEADING};
use crate::layout::PaneId;
use crate::pwm::Secret;

fn unix_now() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64())
}

/// One database write the broker decided (`ChatListModel.append` and its mutations).
fn write_effect(db: &mut Store, effect: &Effect) -> Result<(), String> {
    match effect {
        Effect::Store(m) => {
            db.add_message(m)?;
            // `setVectorStore` (`ChatListModel.swift:283-287`).
            match &m.content {
                Content::VectorStoreCreated(id) => match db.chat(&m.chat_id)? {
                    Some(mut c) => {
                        c.vector_store = Some(id.clone());
                        db.update_chat(&c)
                    }
                    None => Ok(()),
                },
                _ => Ok(()),
            }
        }
        Effect::Replace { unique_id, content } => db.replace_content(unique_id, content).map(|_| ()),
        // A removal also drops the chat's blobs (`ChatListModel.swift:245-264`).
        Effect::Remove { chat, ids } => db.remove_messages(chat, ids).and_then(|()| db.clear_blobs(chat)),
        Effect::Bump(chat) => db.bump(chat, unix_now()),
        Effect::Rename { chat, title } => db.rename(chat, title).map(|_| ()),
        // The chat's agent takes it (`Engine::publish`).
        Effect::ToService(_) => Ok(()),
    }
}

/// `ChatBroker.publish` over the database without the App: the chat's newest id read once,
/// the one row a chunk or commit targets (minor m6), then the broker, which stores before it
/// delivers. A failed write is the error, and the message goes nowhere (`ChatBroker.swift:124-133`).
/// Answers the hand-off to the chat's agent.
pub(super) fn publish_in(db: &mut Store, broker: &mut Broker, chat: &str, message: Message, partial: bool) -> Result<Vec<Effect>, String> {
    // Read once per chat; a failed read leaves the chat unknown, to be read again.
    if !broker.knows(chat) {
        if let Ok(id) = db.newest_id(chat) {
            broker.seed_newest(chat, id);
        }
    }
    let target = match &message.content {
        // A row of another chat is never a target.
        Content::Append { uuid, .. } | Content::AppendAttachment { uuid, .. } | Content::Commit(uuid) => db.message(&broker.append_target(chat, uuid)).ok().flatten().filter(|t| t.chat_id == chat),
        _ => None,
    };
    // One publish's writes land together or not at all, and are settled before the broker
    // delivers the message.
    db.unit_begin()?;
    let result = broker.publish_with(chat, message, partial, target.as_ref(), &mut |step: Step| match step {
        Step::Write(e) => write_effect(db, e),
        Step::Settle => db.unit_settle(),
    });
    if result.is_err() {
        db.unit_abort();
    }
    result
}

/// Merges each chat row's stored permissions into the overrides, each for its own chat only
/// (`ChatListModel.chat(id:)` loads them; an orchestration chat's are kept but not loaded,
/// `ChatListModel.swift:45-63`).
fn load_overrides(store: &Store, overrides: &mut Overrides) {
    for chat in store.chats().unwrap_or_default() {
        if !chat.permissions.is_empty() && !chat.orchestration_enabled {
            // An entry keyed by a pane id belongs to a session of an earlier run (ruling F1).
            let kept = ai_permissions::without_pane_guids(&chat.permissions).unwrap_or_else(|| chat.permissions.clone());
            overrides.merge_encoded(&chat.id, &kept);
        }
    }
}

/// The first open of a run (ruling F1): pane ids restart at 1 every launch, so a chat linked to
/// a pane id, and any grant stored under one, names a session of an earlier run and would
/// control an unrelated new pane. Each such chat is unlinked, and its stored entries keyed by a
/// pane id are dropped (a fork or the overrides cannot carry them). Imported UUID guids stay.
/// A row that cannot be written stays as it was; `load_overrides` skips its entries anyway.
fn unlink_stale_panes(store: &mut Store) {
    for chat in store.chats().unwrap_or_default() {
        let linked = chat.terminal_session_guid.as_deref().is_some_and(ai_permissions::is_pane_guid);
        let scrubbed = ai_permissions::without_pane_guids(&chat.permissions);
        if !linked && scrubbed.is_none() {
            continue;
        }
        // Re-read, so only this row's own fields change (ruling F6).
        let Ok(Some(mut row)) = store.chat(&chat.id) else { continue };
        if row.terminal_session_guid.as_deref().is_some_and(ai_permissions::is_pane_guid) {
            row.terminal_session_guid = None;
        }
        if let Some(text) = ai_permissions::without_pane_guids(&row.permissions) {
            row.permissions = text;
        }
        let _ = store.update_chat(&row);
    }
}

/// `ChatListModel.setPermission` (`ChatListModel.swift:432-446`) over an open database: the
/// override, then the chat's entries written to its row. Taking the open store means the
/// open-time merge has already run and cannot overwrite the change. Answers the chat's row
/// and whether it was saved; an unsaved change still holds for this run (a revoke keeps
/// working), and the caller tells the person (iTerm2 only logs it).
pub(super) fn set_permission_in(store: &mut Store, overrides: &mut Overrides, key: Key, p: Permission) -> Option<(Chat, bool)> {
    let chat = key.chat.clone();
    overrides.set(key, p);
    let mut row = store.chat(&chat).ok().flatten()?;
    row.permissions = overrides.encoded(&chat);
    let saved = store.update_chat(&row).is_ok();
    Some((row, saved))
}

/// `create(chatWithTitle:terminalSessionGuid:browserSessionGuid:permissions:initialMessages:)`
/// (`ChatBroker.swift:78-112`) without the App: the chat row, `SetPermissions` of the seeded set
/// published first unless `initial` carries one, then `initial` stored in order. A given text
/// that decodes (a fork's source permissions) becomes the new chat's own: its entries move to
/// the new id in the row and the overrides (ruling; iTerm2 keeps the text as given, keyed to
/// the source). Any other text is kept as given. Answers the new chat's id.
#[allow(clippy::too_many_arguments)]
pub(super) fn create_in(
    store: &mut Store,
    broker: &mut Broker,
    overrides: &mut Overrides,
    title: &str,
    terminal: Option<&str>,
    browser: Option<&str>,
    permissions: &str,
    initial: Vec<Message>,
    model_name: Option<String>,
    global: &dyn Fn(Category) -> i64,
    now: f64,
) -> Result<String, String> {
    let mut chat = Chat { terminal_session_guid: terminal.map(str::to_string), browser_session_guid: browser.map(str::to_string), model_name, ..Chat::new(title, now) };
    let id = chat.id.clone();
    chat.permissions = ai_permissions::rekey(permissions, &id).unwrap_or_else(|| permissions.to_string());
    let seeded = ai_permissions::seed(&id, terminal, browser, permissions, global);
    store.add_chat(&chat)?;
    overrides.merge_encoded(&id, &chat.permissions);
    broker.seed_newest(&id, None);
    if !initial.iter().any(|m| matches!(m.content, Content::SetPermissions(_))) {
        let allowed = ai_permissions::allowed_of(&seeded).into_iter().collect();
        publish_in(store, broker, &id, Message::new(&id, Participant::User, Content::SetPermissions(allowed), now), false)?;
    }
    // Preloaded straight into the list model, not published; a failure skips that message
    // (`ChatBroker.swift:102-110`).
    for m in initial {
        if store.unit_begin().is_err() {
            continue;
        }
        let preloaded = broker.preload(&id, Message { chat_id: id.clone(), ..m }, &mut |step: Step| match step {
            Step::Write(e) => write_effect(store, e),
            Step::Settle => store.unit_settle(),
        });
        if preloaded.is_err() {
            store.unit_abort();
        }
    }
    Ok(id)
}

impl App {
    /// The chat database, opened on first use at `chat_store::default_path()` with its lock
    /// beside it. Opening merges the chats' stored permissions into the overrides
    /// (`load_overrides`), after every chat linked to a pane id of an earlier run is unlinked
    /// (`unlink_stale_panes`, ruling F1). A failure leaves no store for the rest of the
    /// run and says so once (`ChatWindowController.swift:169-185`); the store's own error
    /// text, which can name the path, is never shown.
    pub(crate) fn chat_db(&mut self) -> Option<&mut Store> {
        if self.chat_db.is_none() && !self.chat_db_failed {
            let path = chat_store::default_path();
            match Store::open(&path, &path.with_file_name(chat_store::LOCK_NAME)) {
                Ok(mut store) => {
                    unlink_stale_panes(&mut store);
                    load_overrides(&store, &mut self.ai_overrides);
                    self.chat_db = Some(store);
                }
                Err(_) => {
                    self.chat_db_failed = true;
                    self.ask_ai(Alert { heading: OPEN_FAILED_HEADING.to_string(), text: OPEN_FAILED.to_string(), buttons: vec!["OK"], silence: None, then: Then::Nothing, gate: false, close: None });
                }
            }
        }
        self.chat_db.as_mut()
    }

    /// `ChatBroker.publish` with the list model behind it, and the hand-off to the chat's
    /// agent (`Engine::publish`). The error is a failed open or write; the message was then
    /// not delivered.
    #[allow(dead_code)] // Called by the chat window (12d).
    pub(crate) fn chat_publish(&mut self, chat: &str, message: Message, partial: bool) -> Result<(), String> {
        if self.chat_db().is_none() {
            return Err(OPEN_FAILED.to_string());
        }
        self.ai_refresh_volatile(chat);
        let fx = self.engine(|cx| Engine::publish(cx, chat, message, partial)).ok_or_else(|| OPEN_FAILED.to_string())??;
        self.chat_apply(fx);
        Ok(())
    }

    /// A new chat pinned to the configured model (`ChatBroker.swift:78-112`, through
    /// `create_in`). Answers the new chat's id; None when the database cannot open.
    #[allow(dead_code)] // Called by the chat window (12d).
    pub(crate) fn chat_create(&mut self, title: &str, terminal_guid: Option<&str>, browser_guid: Option<&str>, permissions: &str, initial: Vec<Message>) -> Option<String> {
        let found = self.ai_found();
        let model_name = crate::ai_model::configured(&self.globals, crate::ai_catalog::bundled(), &found).map(|m| m.name);
        self.chat_db()?;
        let db = self.chat_db.as_mut()?;
        let globals = &self.globals;
        let global = |c: Category| globals.int(c.global_key());
        create_in(db, &mut self.chat_broker, &mut self.ai_overrides, title, terminal_guid, browser_guid, permissions, initial, model_name, &global, unix_now()).ok()
    }

    /// `ChatBroker.delete(chatID:)`: the chat's turn stops and its agent goes before it can
    /// publish again, then its rows go and the broker forgets it.
    #[allow(dead_code)] // Called by the chat window (12d).
    pub(crate) fn chat_delete(&mut self, chat: &str) {
        if self.chat_agents.contains_key(chat) {
            let fx = self.engine(|cx| Engine::drop_chat(cx, chat)).unwrap_or_default();
            self.chat_apply(fx);
            self.chat_agents.remove(chat);
        }
        if let Some(db) = self.chat_db() {
            let _ = db.delete_chat(chat);
        }
        self.chat_broker.forget(chat);
    }

    /// A category's global default as stored (0 Ask, 1 Allow, 2 Never; `P:276-285`).
    pub(crate) fn ai_global_permission(&self, c: Category) -> i64 {
        self.globals.int(c.global_key())
    }

    /// The categories the chat's model is told about, from its linked sessions.
    #[allow(dead_code)] // Called by the chat window (12d).
    pub(crate) fn ai_allowed(&self, chat: &Chat) -> BTreeSet<Category> {
        let global = |c: Category| self.ai_global_permission(c);
        self.ai_overrides.allowed(&chat.id, chat.terminal_session_guid.as_deref(), chat.browser_session_guid.as_deref(), &global)
    }

    /// Sets the chat's permission in a session and stores the chat's overrides in its row
    /// (`set_permission_in`, after the database is open), then publishes the new
    /// allowed set as a user message (`publishUpdatedPermissions`,
    /// `ChatViewController.swift:1204-1220`). The Always confirmation is the caller's (12d's
    /// Info menu asks before calling this).
    #[allow(dead_code)] // Called by the chat window (12d).
    pub(crate) fn ai_set_permission(&mut self, chat: &str, guid: &str, c: Category, p: Permission) {
        if self.chat_db().is_none() {
            return;
        }
        let Some(db) = self.chat_db.as_mut() else { return };
        let Some((row, saved)) = set_permission_in(db, &mut self.ai_overrides, Key { chat: chat.to_string(), guid: guid.to_string(), category: c }, p) else { return };
        if !saved {
            self.ai_permission_not_saved();
        }
        let allowed = self.ai_allowed(&row);
        // `shouldShareTerminalStateAutomatically`: a live linked terminal at Always.
        let global = |c: Category| self.ai_global_permission(c);
        let check_state_always = row
            .terminal_session_guid
            .as_deref()
            .filter(|g| g.parse::<crate::layout::PaneId>().is_ok_and(|id| id.to_string() == *g && self.panes.contains_key(&id)))
            .is_some_and(|g| self.ai_overrides.get(chat, g, Category::CheckTerminalState, &global) == Permission::Always);
        let set = ai_permissions::published_set(&allowed, check_state_always);
        // iTerm2 logs a failure here (`ChatViewController.swift:1195-1203`).
        let _ = self.chat_publish(chat, Message::new(chat, Participant::User, Content::SetPermissions(set), unix_now()), false);
    }

    /// "The permission change could not be saved..." (Task 3 ruling; iTerm2 only logs it).
    fn ai_permission_not_saved(&mut self) {
        self.ask_ai(Alert { heading: OPEN_FAILED_HEADING.to_string(), text: ai_permissions::NOT_SAVED.to_string(), buttons: vec!["OK"], silence: None, then: Then::Nothing, gate: false, close: None });
    }

    /// The volatile context block for `chat`'s next request (`autoProvidedContext`,
    /// `ChatAgent.swift:1051-1112`): its linked terminal's state and visible screen when the
    /// chat's permissions are Always and the person consented. Only a session-bound chat whose
    /// linked pane is alive gets one. Never stored or published. With `commit` false it is a
    /// budgeting peek: the watermark stays and no question shows; with `commit` true a turn that
    /// wanted the block without consent asks once per launch and goes without it.
    pub(crate) fn ai_volatile_context(&mut self, chat: &Chat, commit: bool) -> Option<String> {
        // Only a session-bound chat that is not orchestrating (`ChatAgent.swift:1058-1060`).
        if !crate::ai_context::shares_context(chat.orchestration_enabled, chat.terminal_session_guid.as_deref()) {
            return None;
        }
        let guid = chat.terminal_session_guid.as_deref()?;
        // A guid that is not a live pane's id is no linked terminal (`ai_set_permission`).
        let pane = guid.parse::<crate::layout::PaneId>().ok().filter(|id| id.to_string() == guid && self.panes.contains_key(id))?;
        let global = |c: Category| self.ai_global_permission(c);
        let wants = crate::ai_context::wants(self.ai_overrides.get(&chat.id, guid, Category::CheckTerminalState, &global), self.ai_overrides.get(&chat.id, guid, Category::ViewContents, &global), true);
        let consent = crate::ai_context::Consent::from_global(self.globals.int(crate::ai_context::CONSENT_KEY));
        if crate::ai_context::should_ask(wants, consent, self.ai_consent_asked) && commit {
            self.ai_consent_asked = true;
            self.ask_ai(crate::ai_gate::auto_provide_alert());
        }
        if !(wants.state || wants.screen) || consent != crate::ai_context::Consent::Granted {
            return None;
        }
        let facts = self.ai_facts(pane, &crate::ai_tools::Call::GetTerminalSize);
        let state = crate::ai_context::ai_state(&facts, facts.last_exit.flatten());
        let screen = {
            use alacritty_terminal::grid::Dimensions;
            let s = self.panes.get(&pane)?.own();
            let rows = s.term().grid().screen_lines() as i32;
            s.marked_text(0, rows - 1, true)
        };
        let mark = self.ai_watermarks.entry(chat.id.clone()).or_default();
        crate::ai_context::block(wants, consent, &state, &screen, mark, commit)
    }

    /// The chats' part of the loop: closed gates stop their chats, the turns and file jobs
    /// read what arrived, finished checks, commands and Get-Helps go back to their requests.
    /// Answers when it next needs to run (a file job's next poll).
    pub(super) fn chat_tick(&mut self) -> Option<Instant> {
        let busy = |a: &Agent| a.in_turn || a.driver.is_some() || a.files.is_some();
        if self.chat_db.is_some() && self.chat_agents.values().any(busy) {
            let fx = self.engine(|cx| Engine::gate_tick(cx)).unwrap_or_default();
            self.chat_apply(fx);
            let reading: Vec<String> = self.chat_agents.iter().filter(|(_, a)| a.driver.is_some()).map(|(c, _)| c.clone()).collect();
            for chat in reading {
                self.ai_refresh_volatile(&chat);
            }
            let fx = self.engine(|cx| Engine::poll(cx)).unwrap_or_default();
            self.chat_apply(fx);
        }
        for (chat, request, send, safe) in self.ai_check_tick() {
            self.ai_refresh_volatile(&chat);
            let fx = self.engine(|cx| Engine::checked(cx, &chat, request, send, safe)).unwrap_or_default();
            self.chat_apply(fx);
        }
        for (chat, request_id, o) in self.ai_exec_tick() {
            self.ai_tool_finished(&chat, &request_id, o);
        }
        for (pane, o) in self.ai_help_tick() {
            if let Some((chat, request_id)) = self.ai_pending_tools.remove(&pane) {
                self.ai_tool_finished(&chat, &request_id, o);
            }
        }
        self.chat_agents.values().any(|a| a.files.is_some()).then(|| Instant::now() + crate::ai_files::POLL_EVERY)
    }

    /// Lends the engine its `Ctx` over the App's own fields for one call (ruling R12cd-4).
    /// None when the database cannot open.
    fn engine<T>(&mut self, f: impl FnOnce(&mut Ctx) -> T) -> Option<T> {
        self.chat_db()?;
        let found = self.ai_found();
        let catalog = crate::ai_catalog::bundled();
        let transport = self.ai_transport.clone();
        let options = crate::ai_transport::options_from(&self.globals);
        let notify = self.notify.clone();
        let verbose = self.globals.bool(VERBOSE_KEY);
        #[cfg(windows)]
        let log = (self.ai_wire_log.clone(), self.globals.bool(WIRE_LOG_KEY));
        let globals = &self.globals;
        let panes = &self.panes;
        let model_for = |c: &Chat| c.model_name.as_deref().and_then(|n| crate::ai_model::named(n, globals, catalog, &found)).or_else(|| crate::ai_model::configured(globals, catalog, &found));
        let pane_of = |g: &str| g.parse::<PaneId>().ok().filter(|id| id.to_string() == g && panes.contains_key(id));
        let (t, o, n) = (transport.clone(), options.clone(), notify.clone());
        let mut new_turn = move |_: &Model| -> Box<dyn TurnDriver> {
            Box::new(LiveTurn {
                runner: None,
                transport: t.clone(),
                options: o.clone(),
                notify: n.clone(),
                #[cfg(windows)]
                log: log.clone(),
                verbose,
            })
        };
        let mut new_files = move |m: &Model, job: Job| -> Box<dyn FileDriver> { Box::new(LiveFiles { runner: FileRunner::new(m.clone(), job, transport.clone(), options.clone(), notify.clone()), vendor: m.vendor }) };
        let mut cx = Ctx {
            store: self.chat_db.as_mut()?,
            broker: &mut self.chat_broker,
            overrides: &mut self.ai_overrides,
            agents: &mut self.chat_agents,
            globals,
            model_for: &model_for,
            pane_of: &pane_of,
            new_turn: &mut new_turn,
            new_files: &mut new_files,
            now: unix_now(),
        };
        Some(f(&mut cx))
    }

    /// Refreshes the terminal context snapshot the chat's next request reads
    /// (`ai_volatile_context` as a peek, minor m2); called before every engine call that can
    /// send. The request reads it only after its own gate passed.
    fn ai_refresh_volatile(&mut self, chat: &str) {
        let Some(row) = self.chat_db.as_ref().and_then(|db| db.chat(chat).ok().flatten()) else { return };
        let block = self.ai_volatile_context(&row, false);
        if let Ok(mut slot) = self.chat_agents.entry(chat.to_string()).or_default().volatile.lock() {
            *slot = block;
        }
    }

    /// Applies the engine's effects in order; one that re-enters the engine lends it a fresh
    /// `Ctx` and applies what that answers.
    pub(super) fn chat_apply(&mut self, effects: Vec<EngineEffect>) {
        for e in effects {
            match e {
                EngineEffect::RunTool { chat, request_id, pane, call, send } => self.ai_run_tool(&chat, &request_id, pane, &call, send),
                // Stop and Cancel: forgotten, not answered (the engine answered or dropped it).
                EngineEffect::CancelExec(pane) => {
                    self.ai_exec_cancel(pane);
                    self.ai_help_cancel(pane);
                }
                EngineEffect::CancelChecks(chat) => self.ai_cancel_checks(&chat),
                EngineEffect::Check { chat, pane, command, request } => self.ai_check_command(&chat, pane, &command, request),
                EngineEffect::GateRefused { endpoint, silent } => {
                    self.ai_check_endpoint(Some(&endpoint), silent);
                }
                EngineEffect::AskKey { vendor, endpoint, then } => self.ask_ai_key(vendor, Some(&endpoint), then),
                // 12d Task 5 makes this its `link_chat` (minor m14).
                EngineEffect::Link { chat, pane } => {
                    if let Some(db) = self.chat_db.as_mut() {
                        if let Ok(Some(row)) = db.chat(&chat) {
                            let _ = db.update_chat(&Chat { terminal_session_guid: Some(pane.to_string()), ..row });
                        }
                    }
                }
                EngineEffect::Republish { chat, request_id } => {
                    self.ai_refresh_volatile(&chat);
                    let fx = self.engine(|cx| Engine::republish(cx, &chat, &request_id)).unwrap_or_default();
                    self.chat_apply(fx);
                }
                EngineEffect::PermissionNotSaved => self.ai_permission_not_saved(),
                EngineEffect::VolatileSent(chat) => {
                    if let Some(row) = self.chat_db.as_ref().and_then(|db| db.chat(&chat).ok().flatten()) {
                        self.ai_volatile_context(&row, true);
                    }
                }
            }
        }
    }

    /// The text already on `pane`'s prompt line: shell integration's typed command while at a
    /// prompt, and the agent's own staged typing (ruling R12cd-2).
    fn ai_prompt_text(&self, pane: PaneId) -> (Option<String>, String) {
        let Some(p) = self.panes.get(&pane) else { return (None, String::new()) };
        let s = p.own();
        let typed = if s.at_prompt() == Some(true) { s.typed_command() } else { None };
        let own = self.ai_staged.get(&pane).map(|st| st.current(s.prompts_seen()).to_string()).unwrap_or_default();
        (typed, own)
    }

    /// An `execute_command` on its pane: the whole line is checked and shown (ruling R12cd-2),
    /// the model's own command kept as `send`; no check due publishes it at once.
    fn ai_check_command(&mut self, chat: &str, pane: PaneId, command: &str, request: Message) {
        let (typed, own) = self.ai_prompt_text(pane);
        let line = super::ai_safety_app::check_line(typed.as_deref(), &own, command);
        let (request, send) = if line == command { (request, None) } else { (Message { content: chat_agent::with_line(&request.content, &line), ..request }, Some(command.to_string())) };
        if self.ai_start_check(chat, pane, &line, send.clone(), request.clone()).is_some() {
            return;
        }
        self.ai_refresh_volatile(chat);
        let fx = self.engine(|cx| Engine::checked(cx, chat, request, send, None)).unwrap_or_default();
        self.chat_apply(fx);
    }

    /// Forgets the Get-Help running for a request on `pane`; its (chat, request id).
    fn ai_help_cancel(&mut self, pane: PaneId) -> Option<(String, String)> {
        self.ai_helps.remove(&pane);
        self.ai_pending_tools.remove(&pane)
    }

    /// The one dispatcher of every tool call (the `RunTool` arm; 12f Task 10's `session_*`
    /// mirrors pass their call id as `request_id`). A newer tool on the pane cancels the one
    /// running there, answered `CANCELED_MODEL` (`PS:342`). `execute_command` types `send` (or
    /// the call's command) only while the prompt still holds what was checked; every other tool
    /// starts in `ai_tool_start`.
    pub(super) fn ai_run_tool(&mut self, chat: &str, request_id: &str, pane: PaneId, call: &Call, send: Option<String>) {
        if let Some((c, r)) = self.ai_exec_cancel(pane) {
            self.ai_tool_finished(&c, &r, crate::ai_exec::canceled_outcome());
        }
        if let Some((c, r)) = self.ai_help_cancel(pane) {
            self.ai_tool_finished(&c, &r, crate::ai_exec::canceled_outcome());
        }
        if let Call::ExecuteCommand { command } = call {
            let typed = send.unwrap_or_else(|| command.clone());
            let (si, own) = self.ai_prompt_text(pane);
            let prefix = crate::ai_actions::staged_line(si.as_deref(), &own);
            if !chat_agent::still_staged(command, &prefix, &typed) {
                let o = Outcome { model: chat_agent::STAGED_CHANGED_MODEL.to_string(), notice: Some(chat_agent::STAGED_CHANGED_NOTICE.to_string()) };
                return self.ai_tool_finished(chat, request_id, o);
            }
            if let Some(o) = self.ai_exec_start(pane, chat, request_id, &typed) {
                self.ai_tool_finished(chat, request_id, o);
            }
            return;
        }
        match self.ai_tool_start(pane, call) {
            super::ai_tools_app::ToolStart::Done(o) => self.ai_tool_finished(chat, request_id, o),
            super::ai_tools_app::ToolStart::Pending => {
                self.ai_pending_tools.insert(pane, (chat.to_string(), request_id.to_string()));
            }
            // The 13x browser tools plan records it by token in its own `ai_browser_calls`.
            super::ai_tools_app::ToolStart::Waiting(_) => {}
        }
    }

    /// The one App-side return of every tool result (`ai_run_tool`'s answer, each finished
    /// command and Get-Help, each cancel answer): the engine stores it and answers the model.
    pub(super) fn ai_tool_finished(&mut self, chat: &str, request_id: &str, o: Outcome) {
        self.ai_refresh_volatile(chat);
        let fx = self.engine(|cx| Engine::tool_finished(cx, chat, request_id, o)).unwrap_or_default();
        self.chat_apply(fx);
    }

    /// A button on a parked request (12d).
    #[allow(dead_code)] // Called by the chat window (12d).
    pub(crate) fn ai_answer_request(&mut self, chat: &str, request_id: &str, choice: Choice) {
        self.ai_refresh_volatile(chat);
        let fx = self.engine(|cx| Engine::answer_request(cx, chat, request_id, choice)).unwrap_or_default();
        self.chat_apply(fx);
    }

    /// The session picker's answer (12d).
    #[allow(dead_code)] // Called by the chat window (12d).
    pub(crate) fn ai_session_selected(&mut self, chat: &str, request_id: &str, pane: Option<PaneId>) {
        self.ai_refresh_volatile(chat);
        let fx = self.engine(|cx| Engine::session_selected(cx, chat, request_id, pane)).unwrap_or_default();
        self.chat_apply(fx);
    }

    /// The executing bubble's Cancel (12d).
    #[allow(dead_code)] // Called by the chat window (12d).
    pub(crate) fn ai_cancel_tool(&mut self, chat: &str) {
        self.ai_refresh_volatile(chat);
        let fx = self.engine(|cx| Engine::cancel_tool(cx, chat)).unwrap_or_default();
        self.chat_apply(fx);
    }

    /// Stop (12d).
    #[allow(dead_code)] // Called by the chat window (12d).
    pub(crate) fn chat_stop(&mut self, chat: &str) {
        let fx = self.engine(|cx| Engine::stop(cx, chat)).unwrap_or_default();
        self.chat_apply(fx);
    }

    /// 12d's "Send Commands & Output to AI Automatically".
    #[allow(dead_code)] // Called by the chat window (12d).
    pub(crate) fn ai_send_terminal_command(&mut self, chat: &str, t: TerminalCommand) {
        self.ai_refresh_volatile(chat);
        let fx = self.engine(|cx| Engine::send_terminal_command(cx, chat, t)).unwrap_or_default();
        self.chat_apply(fx);
    }

    /// Starts the chat's vector-store flow for files (ruling I-3, piece 4); a turn with files
    /// starts it itself.
    #[allow(dead_code)] // Called by the chat window (12d).
    pub(crate) fn ai_files_start(&mut self, chat: &str, files: Vec<(String, Vec<u8>)>) {
        let fx = self.engine(|cx| Engine::files_start(cx, chat, files)).unwrap_or_default();
        self.chat_apply(fx);
    }

    /// Polls every chat's turn and file job (`chat_tick` runs it with the gate first).
    #[allow(dead_code)] // `chat_tick` does this; kept for 12d.
    pub(crate) fn ai_files_tick(&mut self) {
        // The gate first, as `chat_tick` does: a closed gate stops the chat before it is polled.
        let fx = self.engine(|cx| {
            let mut fx = Engine::gate_tick(cx);
            fx.extend(Engine::poll(cx));
            fx
        });
        let fx = fx.unwrap_or_default();
        self.chat_apply(fx);
    }

    /// `KeyThen::Turn`: the key prompt's answer for a chat turn.
    pub(super) fn ai_turn_key(&mut self, chat: &str, key: Option<Secret>) {
        self.ai_refresh_volatile(chat);
        let fx = self.engine(|cx| Engine::key_given(cx, chat, key)).unwrap_or_default();
        self.chat_apply(fx);
    }

    /// `KeyThen::Files`: the key prompt's answer for a file job.
    pub(super) fn ai_files_key(&mut self, chat: &str, key: Option<Secret>) {
        let fx = self.engine(|cx| Engine::files_key(cx, chat, key)).unwrap_or_default();
        self.chat_apply(fx);
    }
}

/// `aiChatRawWireLogging` and `aiChatVerboseConsoleLogging` (12b's wire log and DebugView lines).
#[cfg(windows)]
const WIRE_LOG_KEY: &str = "aiChatRawWireLogging";
const VERBOSE_KEY: &str = "aiChatVerboseConsoleLogging";

/// A chat turn's requests: 12b's `Runner` over the App's transport, with the vendor's stored
/// key read at the start.
struct LiveTurn {
    runner: Option<Runner>,
    transport: Arc<dyn Transport>,
    options: Options,
    notify: Notify,
    #[cfg(windows)]
    log: (Arc<std::sync::Mutex<crate::ai_wire_log::WireLog>>, bool),
    verbose: bool,
}

impl TurnDriver for LiveTurn {
    fn start(&mut self, input: TurnInput) -> Vec<Out> {
        let key = crate::ai_keys::shared().lock().unwrap_or_else(|e| e.into_inner()).get(input.model.vendor, &mut *crate::ai_keys::system_store());
        crate::ai_wire_log::console(self.verbose, &format!("chat turn started with {} ({} messages, {} tools)", input.model.name, input.messages.len(), input.tools.len()));
        let runner = Runner::new(Turn::new(input), self.transport.clone(), self.options.clone(), self.notify.clone());
        #[cfg(windows)]
        let runner = runner.with_log(self.log.0.clone(), self.log.1);
        self.runner.insert(runner).start(key.as_ref())
    }

    fn poll(&mut self, pass: &mut dyn FnMut() -> Option<Passed>) -> Vec<Out> {
        self.runner.as_mut().map(|r| r.poll_with(pass)).unwrap_or_default()
    }

    fn key_given(&mut self, key: Option<Secret>) -> Vec<Out> {
        self.runner.as_mut().map(|r| r.key_given(key)).unwrap_or_default()
    }

    fn tool_done(&mut self, r: ToolResult) -> Vec<Out> {
        crate::ai_wire_log::console(self.verbose, "chat tool result sent");
        self.runner.as_mut().map(|run| run.tool_done(r)).unwrap_or_default()
    }

    fn cancel(&mut self) -> Vec<Out> {
        crate::ai_wire_log::console(self.verbose, "chat turn canceled");
        self.runner.as_mut().map(Runner::cancel).unwrap_or_default()
    }
}

/// One vector-store job: 12b's `FileRunner`, with the vendor's stored key when none is given.
struct LiveFiles {
    runner: FileRunner,
    vendor: Vendor,
}

impl FileDriver for LiveFiles {
    fn start(&mut self, key: Option<&Secret>) -> Vec<FileOut> {
        let stored = match key {
            Some(_) => None,
            None => crate::ai_keys::shared().lock().unwrap_or_else(|e| e.into_inner()).get(self.vendor, &mut *crate::ai_keys::system_store()),
        };
        self.runner.start(key.or(stored.as_ref()))
    }

    fn poll(&mut self, now: Instant) -> Vec<FileOut> {
        self.runner.poll(now)
    }

    fn cancel(&mut self) {
        self.runner.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai_permissions::{allowed_of, defaults, Category, Key, Overrides, Permission};
    use crate::chat_broker::Broker;

    fn ask(_: Category) -> i64 {
        0
    }

    const TERMINAL: [Category; 7] = [Category::CheckTerminalState, Category::RunCommands, Category::ViewContents, Category::WriteToClipboard, Category::ControlTerminal, Category::ViewManpages, Category::WriteToFilesystem];

    #[test]
    fn failed_writes_deliver_nothing() {
        let (mut store, mut broker, mut overrides) = (Store::open_in_memory().unwrap(), Broker::new(), Overrides::default());
        let id = create_in(&mut store, &mut broker, &mut overrides, "T", Some("7"), None, "", vec![], None, &ask, 1.0).unwrap();
        let sub = broker.subscribe(Some(&id));
        let q = Message::new(&id, Participant::User, Content::PlainText { text: "q".into(), context: None }, 2.0);
        assert_eq!(publish_in(&mut store, &mut broker, &id, q.clone(), false).unwrap(), vec![Effect::ToService(q.clone())]);
        assert_eq!(broker.drain(sub).len(), 1);
        assert_eq!(broker.next_turn(&id), Some(q));
        broker.turn_done(&id);

        store.refuse_message_writes();
        let r = Message::new(&id, Participant::User, Content::PlainText { text: "r".into(), context: None }, 3.0);
        assert!(publish_in(&mut store, &mut broker, &id, r, false).is_err());
        assert_eq!(broker.drain(sub), vec![], "not delivered");
        assert_eq!(broker.next_turn(&id), None, "not queued for the agent");
        assert_eq!(store.messages(&id).unwrap().len(), 2);
        // A new chat whose permissions cannot be stored fails as iTerm2's `create` throws.
        assert!(create_in(&mut store, &mut broker, &mut overrides, "U", Some("7"), None, "", vec![], None, &ask, 4.0).is_err());
    }

    #[test]
    fn a_squelched_publish_settles_its_unit() {
        let dir = crate::util_cli::TempDir::new("tandem-chatdb-squelch");
        let (path, lock) = (dir.join(crate::chat_store::FILE_NAME), dir.join(crate::chat_store::LOCK_NAME));
        let (mut broker, mut overrides) = (Broker::new(), Overrides::default());
        broker.add_processor(Box::new(|m: Message, _: &str, _: bool| match &m.content {
            Content::PlainText { text, .. } if text == "drop" => None,
            _ => Some(m),
        }));
        let text = |t: &str, at: f64| Message::new("", Participant::User, Content::PlainText { text: t.into(), context: None }, at);
        let id = {
            let mut store = Store::open(&path, &lock).unwrap();
            let id = create_in(&mut store, &mut broker, &mut overrides, "T", Some("7"), None, "", vec![], None, &ask, 1.0).unwrap();
            let mut drop_me = text("drop", 2.0);
            drop_me.chat_id = id.clone();
            assert_eq!(publish_in(&mut store, &mut broker, &id, drop_me, false).unwrap(), vec![]);
            let mut keep = text("keep", 3.0);
            keep.chat_id = id.clone();
            publish_in(&mut store, &mut broker, &id, keep, false).unwrap();
            id
        };
        // Reopened: a write that was never committed is gone.
        let store = Store::open(&path, &lock).unwrap();
        let kept: Vec<String> = store.messages(&id).unwrap().into_iter().filter_map(|m| if let Content::PlainText { text, .. } = m.content { Some(text) } else { None }).collect();
        assert_eq!(kept, vec!["keep".to_string()], "the squelched message is not stored and the next one is committed");
    }

    #[test]
    fn chat_create_seeds_permissions() {
        let (mut store, mut broker, mut overrides) = (Store::open_in_memory().unwrap(), Broker::new(), Overrides::default());
        let id = create_in(&mut store, &mut broker, &mut overrides, "T", Some("7"), None, "", vec![], None, &ask, 100.0).unwrap();
        let chat = store.chat(&id).unwrap().unwrap();
        // The row keeps the text it was given (`ChatBroker.swift:89-92`); lookups fall back to
        // the globals, so the defaults are not written.
        assert_eq!(chat.permissions, "");
        assert_eq!(chat.terminal_session_guid.as_deref(), Some("7"));
        assert_eq!(chat.browser_session_guid, None);
        assert_eq!(chat.title, "T");
        let messages = store.messages(&id).unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].author, Participant::User);
        assert_eq!(messages[0].content, Content::SetPermissions(TERMINAL.to_vec()));
        assert_eq!(allowed_of(&defaults(&id, Some("7"), None, &ask)).into_iter().collect::<Vec<_>>(), TERMINAL);
        assert_eq!(overrides.get(&id, "7", Category::RunCommands, &ask), Permission::Ask);

        // Initial messages that carry permissions: none published, the rest kept in order.
        let initial = vec![
            Message::new("X", Participant::User, Content::SetPermissions(vec![Category::RunCommands]), 1.0),
            Message::new("X", Participant::User, Content::PlainText { text: "a".into(), context: None }, 2.0),
            Message::new("X", Participant::Agent, Content::Markdown("b".into()), 3.0),
        ];
        let id = create_in(&mut store, &mut broker, &mut overrides, "T", Some("7"), None, "", initial.clone(), None, &ask, 100.0).unwrap();
        let messages = store.messages(&id).unwrap();
        assert_eq!(messages.len(), 3);
        for (m, i) in messages.iter().zip(&initial) {
            assert_eq!((&m.unique_id, &m.content, &m.chat_id), (&i.unique_id, &i.content, &id));
        }

        // A readable text becomes the new chat's own (a fork's copy) and decides the published set.
        let mut given = Overrides::default();
        given.set(Key { chat: "C".into(), guid: "7".into(), category: Category::RunCommands }, Permission::Always);
        let text = given.encoded("C");
        let id = create_in(&mut store, &mut broker, &mut overrides, "T", Some("7"), None, &text, vec![], None, &ask, 100.0).unwrap();
        assert_eq!(store.chat(&id).unwrap().unwrap().permissions, ai_permissions::rekey(&text, &id).unwrap());
        assert_eq!(overrides.get(&id, "7", Category::RunCommands, &ask), Permission::Always);
        assert_eq!(overrides.get("C", "7", Category::RunCommands, &ask), Permission::Ask, "nothing granted to the source");
        assert_eq!(store.messages(&id).unwrap()[0].content, Content::SetPermissions(vec![Category::RunCommands]));
        // A text that does not decode is kept as given.
        let id = create_in(&mut store, &mut broker, &mut overrides, "T", Some("7"), None, "x", vec![], None, &ask, 100.0).unwrap();
        assert_eq!(store.chat(&id).unwrap().unwrap().permissions, "x");

        // A browser session is stored; no session at all still publishes the (empty) set.
        let id = create_in(&mut store, &mut broker, &mut overrides, "T", None, Some("9"), "", vec![], Some("gpt-5".into()), &ask, 100.0).unwrap();
        let chat = store.chat(&id).unwrap().unwrap();
        assert_eq!(chat.browser_session_guid.as_deref(), Some("9"));
        assert_eq!(chat.model_name.as_deref(), Some("gpt-5"));
        assert_eq!(store.messages(&id).unwrap()[0].content, Content::SetPermissions(vec![Category::ActInWebBrowser]));
        let id = create_in(&mut store, &mut broker, &mut overrides, "T", None, None, "", vec![], None, &ask, 100.0).unwrap();
        assert_eq!(store.messages(&id).unwrap()[0].content, Content::SetPermissions(vec![]));
        assert_eq!(store.chats().unwrap().len(), 6);
    }

    /// A session guid that is not a pane id: a pane id stored in a row never loads (ruling F1).
    const SESSION: &str = "S-7";

    fn key(chat: &str, c: Category) -> Key {
        Key { chat: chat.to_string(), guid: SESSION.to_string(), category: c }
    }

    #[test]
    fn stored_permissions_stay_with_their_chat() {
        let (mut store, mut broker, mut overrides) = (Store::open_in_memory().unwrap(), Broker::new(), Overrides::default());
        let source = create_in(&mut store, &mut broker, &mut overrides, "S", Some(SESSION), None, "", vec![], None, &ask, 1.0).unwrap();
        assert!(set_permission_in(&mut store, &mut overrides, key(&source, Category::RunCommands), Permission::Always).unwrap().1);
        let text = store.chat(&source).unwrap().unwrap().permissions;
        let fork = create_in(&mut store, &mut broker, &mut overrides, "F", Some(SESSION), None, &text, vec![], None, &ask, 2.0).unwrap();
        // The source revokes and is used again, so it is newer than the fork.
        assert!(set_permission_in(&mut store, &mut overrides, key(&source, Category::RunCommands), Permission::Never).unwrap().1);
        store.bump(&source, 3.0).unwrap();
        // A hostile or imported row naming the source.
        let mut hostile = Chat::new("H", 0.5);
        hostile.permissions = text.clone();
        store.add_chat(&hostile).unwrap();

        // Relaunch: the source stays revoked, the fork keeps its own copy.
        let mut relaunched = Overrides::default();
        load_overrides(&store, &mut relaunched);
        assert_eq!(relaunched.get(&source, SESSION, Category::RunCommands, &ask), Permission::Never);
        assert_eq!(relaunched.get(&fork, SESSION, Category::RunCommands, &ask), Permission::Always);
        assert_eq!(relaunched.encoded(&hostile.id), "[]", "a row grants nothing to another chat, itself included");
    }

    #[test]
    fn a_restarted_pane_id_never_inherits_a_chat() {
        for pane in ["0", "3", "18446744073709551615"] {
            assert!(ai_permissions::is_pane_guid(pane), "{pane}");
        }
        for other in ["", "03", "+3", "-3", " 3", "3 ", "6F1C2A3B-0000-4000-8000-000000000001", "18446744073709551616"] {
            assert!(!ai_permissions::is_pane_guid(other), "{other:?}");
        }
        let mut store = Store::open_in_memory().unwrap();
        let mut stored = Overrides::default();
        // A chat of the previous run, linked to pane 3 with Always Allow; one imported with a UUID guid.
        let (mut old, mut imported) = (Chat::new("old", 1.0), Chat::new("imported", 2.0));
        old.terminal_session_guid = Some("3".into());
        imported.terminal_session_guid = Some("6F1C2A3B-0000-4000-8000-000000000001".into());
        for c in [&mut old, &mut imported] {
            let guid = c.terminal_session_guid.clone().unwrap();
            stored.set(Key { chat: c.id.clone(), guid, category: Category::RunCommands }, Permission::Always);
            c.permissions = stored.encoded(&c.id);
        }
        store.add_chat(&old).unwrap();
        store.add_chat(&imported).unwrap();

        // The next launch: the first open.
        unlink_stale_panes(&mut store);
        let mut overrides = Overrides::default();
        load_overrides(&store, &mut overrides);

        let row = store.chat(&old.id).unwrap().unwrap();
        assert_eq!(row.terminal_session_guid, None, "the chat is unlinked");
        // A new pane 3 owns nothing of it.
        assert_eq!(overrides.get(&old.id, "3", Category::RunCommands, &ask), Permission::Ask);
        assert_eq!(row.permissions, "[]", "the stored grant is gone too, so a fork cannot carry it");
        let (mut broker, mut later) = (Broker::new(), Overrides::default());
        let fork = create_in(&mut store, &mut broker, &mut later, "F", Some("3"), None, &row.permissions, vec![], None, &ask, 5.0).unwrap();
        assert_eq!(later.get(&fork, "3", Category::RunCommands, &ask), Permission::Ask);
        // An entry keyed by a decimal guid is skipped even when a row still holds it.
        let mut held = Chat::new("held", 3.0);
        held.permissions = stored.encoded(&old.id).replace(&old.id, &held.id);
        assert!(held.permissions.contains("\"guid\":\"3\""));
        store.add_chat(&held).unwrap();
        let mut skipped = Overrides::default();
        load_overrides(&store, &mut skipped);
        assert_eq!(skipped.get(&held.id, "3", Category::RunCommands, &ask), Permission::Ask);
        // The imported chat keeps its link and its grant.
        let kept = store.chat(&imported.id).unwrap().unwrap();
        assert_eq!(kept.terminal_session_guid, imported.terminal_session_guid);
        assert_eq!(overrides.get(&imported.id, "6F1C2A3B-0000-4000-8000-000000000001", Category::RunCommands, &ask), Permission::Always);
    }

    #[test]
    fn set_permission_follows_the_open() {
        // The row as a previous run left it: two entries.
        let mut store = Store::open_in_memory().unwrap();
        let mut stored = Overrides::default();
        let mut chat = Chat::new("T", 1.0);
        stored.set(key(&chat.id, Category::RunCommands), Permission::Always);
        stored.set(key(&chat.id, Category::ViewContents), Permission::Always);
        chat.permissions = stored.encoded(&chat.id);
        store.add_chat(&chat).unwrap();

        // `ai_set_permission`'s order: the open (and its merge), then the change.
        let mut overrides = Overrides::default();
        load_overrides(&store, &mut overrides);
        let (row, saved) = set_permission_in(&mut store, &mut overrides, key(&chat.id, Category::RunCommands), Permission::Never).unwrap();
        assert!(saved);
        assert_eq!(overrides.get(&chat.id, SESSION, Category::RunCommands, &ask), Permission::Never, "the change is not overwritten");
        let mut back = Overrides::default();
        assert!(back.merge_encoded(&chat.id, &store.chat(&chat.id).unwrap().unwrap().permissions));
        assert_eq!(back.get(&chat.id, SESSION, Category::RunCommands, &ask), Permission::Never);
        assert_eq!(back.get(&chat.id, SESSION, Category::ViewContents, &ask), Permission::Always, "the chat's other entries are kept");
        assert_eq!(row.permissions, overrides.encoded(&chat.id));
        assert_eq!(set_permission_in(&mut store, &mut overrides, key("gone", Category::RunCommands), Permission::Never), None);

        // A row that cannot be written: the revoke holds for this run and is reported unsaved.
        store.refuse_chat_updates();
        let (_, saved) = set_permission_in(&mut store, &mut overrides, key(&chat.id, Category::ViewContents), Permission::Never).unwrap();
        assert!(!saved);
        assert_eq!(overrides.get(&chat.id, SESSION, Category::ViewContents, &ask), Permission::Never);
        assert_eq!(ai_permissions::NOT_SAVED, "The permission change could not be saved. It applies until Tandem quits.");
    }

    #[test]
    fn one_publish_writes_together() {
        let (mut store, mut broker, mut overrides) = (Store::open_in_memory().unwrap(), Broker::new(), Overrides::default());
        let id = create_in(&mut store, &mut broker, &mut overrides, "T", Some("7"), None, "", vec![], None, &ask, 1.0).unwrap();
        let sub = broker.subscribe(Some(&id));
        store.refuse_chat_updates();
        // The message row stores, the bump fails: neither lands, nothing is delivered.
        let q = Message::new(&id, Participant::User, Content::PlainText { text: "q".into(), context: None }, 2.0);
        assert!(publish_in(&mut store, &mut broker, &id, q, false).is_err());
        assert_eq!(store.messages(&id).unwrap().len(), 1);
        assert_eq!(broker.drain(sub), vec![]);
    }

}
