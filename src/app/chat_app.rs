//! The AI chats' app side: the chat database (opened on first use), the broker, the chats'
//! permissions, and applying what a publish decided to the database (`ChatBroker.swift`,
//! `ChatListModel.swift`). The chat agents that answer `Effect::ToService` and the tick that
//! drives them are Task 11's.

use std::collections::BTreeSet;
use std::time::Instant;

use super::App;
use crate::ai_gate::{Alert, Then};
use crate::ai_permissions::{self, Category, Key, Overrides, Permission};
use crate::chat_broker::{Broker, Effect};
use crate::chat_message::{Content, Message, Participant};
use crate::chat_store::{self, Chat, Store, OPEN_FAILED, OPEN_FAILED_HEADING};

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
        // The chat's agent takes it (Task 11).
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
    broker.publish_with(chat, message, partial, target.as_ref(), &mut |e: &Effect| write_effect(db, e))
}

/// `create(chatWithTitle:terminalSessionGuid:browserSessionGuid:permissions:initialMessages:)`
/// (`ChatBroker.swift:78-112`) without the App: the chat row (keeping `permissions` as given),
/// the given text merged into the overrides when it decodes (the load `ChatListModel.chat(id:)`
/// does on the chat's first use), `SetPermissions` of the seeded set published first unless
/// `initial` carries one, then `initial` stored in order. Answers the new chat's id.
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
    let chat = Chat {
        terminal_session_guid: terminal.map(str::to_string),
        browser_session_guid: browser.map(str::to_string),
        permissions: permissions.to_string(),
        model_name,
        ..Chat::new(title, now)
    };
    let id = chat.id.clone();
    let seeded = ai_permissions::seed(&id, terminal, browser, permissions, global);
    store.add_chat(&chat)?;
    overrides.merge_encoded(permissions);
    broker.seed_newest(&id, None);
    if !initial.iter().any(|m| matches!(m.content, Content::SetPermissions(_))) {
        let allowed = ai_permissions::allowed_of(&seeded).into_iter().collect();
        publish_in(store, broker, &id, Message::new(&id, Participant::User, Content::SetPermissions(allowed), now), false)?;
    }
    // Preloaded straight into the list model, not published; a failure skips that message
    // (`ChatBroker.swift:102-110`).
    for m in initial {
        for effect in broker.preload(&id, Message { chat_id: id.clone(), ..m }) {
            let _ = write_effect(store, &effect);
        }
    }
    Ok(id)
}

impl App {
    /// The chat database, opened on first use at `chat_store::default_path()` with its lock
    /// beside it. Opening merges every session-bound chat's stored permissions into the
    /// overrides (`ChatListModel.chat(id:)` loads them; an orchestration chat's are kept but
    /// not loaded, `ChatListModel.swift:45-63`). A failure leaves no store for the rest of the
    /// run and says so once (`ChatWindowController.swift:169-185`); the store's own error
    /// text, which can name the path, is never shown.
    #[allow(dead_code)] // Called by the agent (Task 11) and the chat window (12d).
    pub(crate) fn chat_db(&mut self) -> Option<&mut Store> {
        if self.chat_db.is_none() && !self.chat_db_failed {
            let path = chat_store::default_path();
            match Store::open(&path, &path.with_file_name(chat_store::LOCK_NAME)) {
                Ok(store) => {
                    for chat in store.chats().unwrap_or_default() {
                        if !chat.permissions.is_empty() && !chat.orchestration_enabled {
                            self.ai_overrides.merge_encoded(&chat.permissions);
                        }
                    }
                    self.chat_db = Some(store);
                }
                Err(_) => {
                    self.chat_db_failed = true;
                    self.ask_ai(Alert { heading: OPEN_FAILED_HEADING.to_string(), text: OPEN_FAILED.to_string(), buttons: vec!["OK"], silence: None, then: Then::Nothing, gate: false });
                }
            }
        }
        self.chat_db.as_mut()
    }

    /// `ChatBroker.publish` with the list model behind it (`publish_in`). The error is a failed
    /// open or write; the message was then not delivered.
    #[allow(dead_code)] // Called by the agent (Task 11) and the chat window (12d).
    pub(crate) fn chat_publish(&mut self, chat: &str, message: Message, partial: bool) -> Result<(), String> {
        if self.chat_db().is_none() {
            return Err(OPEN_FAILED.to_string());
        }
        let Some(db) = self.chat_db.as_mut() else { return Err(OPEN_FAILED.to_string()) };
        // The hand-off to the chat's agent is Task 11's.
        publish_in(db, &mut self.chat_broker, chat, message, partial).map(|_| ())
    }

    /// A new chat pinned to the configured model (`ChatBroker.swift:78-112`, through
    /// `create_in`). Answers the new chat's id; None when the database cannot open.
    #[allow(dead_code)] // Called by the agent (Task 11) and the chat window (12d).
    pub(crate) fn chat_create(&mut self, title: &str, terminal_guid: Option<&str>, browser_guid: Option<&str>, permissions: &str, initial: Vec<Message>) -> Option<String> {
        let found = self.ai_found();
        let model_name = crate::ai_model::configured(&self.globals, crate::ai_catalog::bundled(), &found).map(|m| m.name);
        self.chat_db()?;
        let db = self.chat_db.as_mut()?;
        let globals = &self.globals;
        let global = |c: Category| globals.int(c.global_key());
        create_in(db, &mut self.chat_broker, &mut self.ai_overrides, title, terminal_guid, browser_guid, permissions, initial, model_name, &global, unix_now()).ok()
    }

    /// `ChatBroker.delete(chatID:)`: the chat's rows go, and the broker forgets it.
    #[allow(dead_code)] // Called by the agent (Task 11) and the chat window (12d).
    pub(crate) fn chat_delete(&mut self, chat: &str) {
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
    #[allow(dead_code)] // Called by the agent (Task 11) and the chat window (12d).
    pub(crate) fn ai_allowed(&self, chat: &Chat) -> BTreeSet<Category> {
        let global = |c: Category| self.ai_global_permission(c);
        self.ai_overrides.allowed(&chat.id, chat.terminal_session_guid.as_deref(), chat.browser_session_guid.as_deref(), &global)
    }

    /// Sets the chat's permission in a session and stores the chat's overrides in its row
    /// (`ChatListModel.setPermission`, `ChatListModel.swift:432-446`), then publishes the new
    /// allowed set as a user message (`publishUpdatedPermissions`,
    /// `ChatViewController.swift:1204-1220`). The Always confirmation is the caller's (12d's
    /// Info menu asks before calling this).
    #[allow(dead_code)] // Called by the agent (Task 11) and the chat window (12d).
    pub(crate) fn ai_set_permission(&mut self, chat: &str, guid: &str, c: Category, p: Permission) {
        self.ai_overrides.set(Key { chat: chat.to_string(), guid: guid.to_string(), category: c }, p);
        let encoded = self.ai_overrides.encoded(chat);
        let Some(db) = self.chat_db() else { return };
        let Ok(Some(mut row)) = db.chat(chat) else { return };
        row.permissions = encoded;
        let _ = db.update_chat(&row);
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

    /// The chats' part of the loop; answers when it next needs to run. The agents, their
    /// commands and checks join it in later tasks.
    pub(super) fn chat_tick(&mut self) -> Option<Instant> {
        None
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

        // A readable text is merged and decides the published set.
        let mut given = Overrides::default();
        given.set(Key { chat: "C".into(), guid: "7".into(), category: Category::RunCommands }, Permission::Always);
        let text = given.encoded("C");
        let id = create_in(&mut store, &mut broker, &mut overrides, "T", Some("7"), None, &text, vec![], None, &ask, 100.0).unwrap();
        assert_eq!(store.chat(&id).unwrap().unwrap().permissions, text);
        assert_eq!(overrides.get("C", "7", Category::RunCommands, &ask), Permission::Always);
        assert_eq!(store.messages(&id).unwrap()[0].content, Content::SetPermissions(vec![Category::RunCommands]));

        // A browser session is stored; no session at all still publishes the (empty) set.
        let id = create_in(&mut store, &mut broker, &mut overrides, "T", None, Some("9"), "", vec![], Some("gpt-5".into()), &ask, 100.0).unwrap();
        let chat = store.chat(&id).unwrap().unwrap();
        assert_eq!(chat.browser_session_guid.as_deref(), Some("9"));
        assert_eq!(chat.model_name.as_deref(), Some("gpt-5"));
        assert_eq!(store.messages(&id).unwrap()[0].content, Content::SetPermissions(vec![Category::ActInWebBrowser]));
        let id = create_in(&mut store, &mut broker, &mut overrides, "T", None, None, "", vec![], None, &ask, 100.0).unwrap();
        assert_eq!(store.messages(&id).unwrap()[0].content, Content::SetPermissions(vec![]));
        assert_eq!(store.chats().unwrap().len(), 5);
    }
}
