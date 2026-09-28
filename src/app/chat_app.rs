//! The AI chats' app side: the chat database (opened on first use), the broker, and applying
//! what a publish decided to the database (`ChatBroker.swift`, `ChatListModel.swift`). The
//! chat agents that answer `Effect::ToService` and the tick that drives them are Task 11's.

use std::time::Instant;

use super::App;
use crate::ai_gate::{Alert, Then};
use crate::chat_broker::Effect;
use crate::chat_message::{Content, Message, Participant};
use crate::chat_store::{self, Chat, Store, OPEN_FAILED, OPEN_FAILED_HEADING};

fn unix_now() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64())
}

impl App {
    /// The chat database, opened on first use at `chat_store::default_path()` with its lock
    /// beside it. A failure leaves no store for the rest of the run and says so once
    /// (`ChatWindowController.swift:169-185`); the store's own error text, which can name the
    /// path, is never shown.
    #[allow(dead_code)] // Called by the agent (Task 11) and the chat window (12d).
    pub(crate) fn chat_db(&mut self) -> Option<&mut Store> {
        if self.chat_db.is_none() && !self.chat_db_failed {
            let path = chat_store::default_path();
            match Store::open(&path, &path.with_file_name(chat_store::LOCK_NAME)) {
                Ok(store) => self.chat_db = Some(store),
                Err(_) => {
                    self.chat_db_failed = true;
                    self.ask_ai(Alert { heading: OPEN_FAILED_HEADING.to_string(), text: OPEN_FAILED.to_string(), buttons: vec!["OK"], silence: None, then: Then::Nothing, gate: false });
                }
            }
        }
        self.chat_db.as_mut()
    }

    /// `ChatBroker.publish` with the list model behind it: the broker decides, then each
    /// effect is written to the database. A chunk or commit reads only the one row it targets
    /// (minor m6).
    #[allow(dead_code)] // Called by the agent (Task 11) and the chat window (12d).
    pub(crate) fn chat_publish(&mut self, chat: &str, message: Message, partial: bool) {
        if self.chat_db().is_none() {
            return;
        }
        let Some(db) = self.chat_db.as_ref() else { return };
        // Read once per chat; a failed read leaves the chat unknown, to be read again.
        let newest = if self.chat_broker.knows(chat) { None } else { db.newest_id(chat).ok() };
        let target = match &message.content {
            Content::Append { uuid, .. } | Content::AppendAttachment { uuid, .. } | Content::Commit(uuid) => db.message(&self.chat_broker.append_target(chat, uuid)).ok().flatten(),
            _ => None,
        };
        if let Some(id) = newest {
            self.chat_broker.seed_newest(chat, id);
        }
        let effects = self.chat_broker.publish(chat, message, partial, target.as_ref());
        self.chat_apply_effects(effects);
    }

    /// Writes what the broker decided. A write that fails leaves the row as it was; the
    /// message was already delivered, as iTerm2's model is updated before its database.
    fn chat_apply_effects(&mut self, effects: Vec<Effect>) {
        let Some(db) = self.chat_db.as_mut() else { return };
        for effect in effects {
            let _ = match effect {
                Effect::Store(m) => db.add_message(&m).map(|_| ()).and_then(|()| match &m.content {
                    // `setVectorStore` (`ChatListModel.swift:283-287`).
                    Content::VectorStoreCreated(id) => match db.chat(&m.chat_id) {
                        Ok(Some(mut c)) => {
                            c.vector_store = Some(id.clone());
                            db.update_chat(&c)
                        }
                        other => other.map(|_| ()),
                    },
                    _ => Ok(()),
                }),
                Effect::Replace { unique_id, content } => db.replace_content(&unique_id, &content).map(|_| ()),
                // A removal also drops the chat's blobs (`ChatListModel.swift:245-264`).
                Effect::Remove { chat, ids } => db.remove_messages(&chat, &ids).and_then(|()| db.clear_blobs(&chat)),
                Effect::Bump(chat) => db.bump(&chat, unix_now()),
                Effect::Rename { chat, title } => db.rename(&chat, &title).map(|_| ()),
                // The chat's agent takes it (Task 11).
                Effect::ToService(_) => Ok(()),
            };
        }
    }

    /// `create(chatWithTitle:terminalSessionGuid:browserSessionGuid:permissions:initialMessages:)`
    /// (`ChatBroker.swift:78-112`): a new chat pinned to the configured model, its permissions
    /// published first unless `initial` carries them, then `initial` stored in order. Answers
    /// the new chat's id; None when the database cannot open.
    #[allow(dead_code)] // Called by the agent (Task 11) and the chat window (12d).
    pub(crate) fn chat_create(&mut self, title: &str, terminal_guid: Option<&str>, browser_guid: Option<&str>, permissions: &str, initial: Vec<Message>) -> Option<String> {
        let found = self.ai_found();
        let model_name = crate::ai_model::configured(&self.globals, crate::ai_catalog::bundled(), &found).map(|m| m.name);
        let now = unix_now();
        let chat = Chat {
            terminal_session_guid: terminal_guid.map(str::to_string),
            browser_session_guid: browser_guid.map(str::to_string),
            permissions: permissions.to_string(),
            model_name,
            ..Chat::new(title, now)
        };
        let id = chat.id.clone();
        self.chat_db()?.add_chat(&chat).ok()?;
        self.chat_broker.seed_newest(&id, None);
        if !initial.iter().any(|m| matches!(m.content, Content::SetPermissions(_))) {
            // The allowed categories of the chat's permissions (Task 3 fills the rule).
            let allowed = Vec::new();
            self.chat_publish(&id, Message::new(&id, Participant::User, Content::SetPermissions(allowed), now), false);
        }
        // Preloaded straight into the list model, not published (`ChatBroker.swift:102-110`).
        for m in initial {
            let effects = self.chat_broker.preload(&id, Message { chat_id: id.clone(), ..m });
            self.chat_apply_effects(effects);
        }
        Some(id)
    }

    /// `ChatBroker.delete(chatID:)`: the chat's rows go, and the broker forgets it.
    #[allow(dead_code)] // Called by the agent (Task 11) and the chat window (12d).
    pub(crate) fn chat_delete(&mut self, chat: &str) {
        if let Some(db) = self.chat_db() {
            let _ = db.delete_chat(chat);
        }
        self.chat_broker.forget(chat);
    }

    /// The chats' part of the loop; answers when it next needs to run. The agents, their
    /// commands and checks join it in later tasks.
    pub(super) fn chat_tick(&mut self) -> Option<Instant> {
        None
    }
}
