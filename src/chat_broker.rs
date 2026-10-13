//! The chat broker and the service's queue (`AITerm/ChatBroker.swift`, the streaming rewrite
//! of `ChatClient.swift:296-418`, the list model's append rules `ChatListModel.swift:264-345`,
//! the per-chat turn queue of `ChatService.swift`). A pure state machine: `publish` runs the
//! processors, applies the streaming rule, fans the message out to subscriptions and answers
//! the database writes and the service hand-off as `Effect`s the App applies.
//!
//! The broker keeps each chat's newest message id and its rewrite table in memory, so a
//! streamed chunk never reads or decodes the whole chat (minor m6): the App seeds the newest
//! id once per chat and passes the one row a chunk or commit targets.

use std::collections::{HashMap, HashSet, VecDeque};

use base64::Engine;
use serde_json::{json, Value};

use crate::chat_message::{case, new_id, one_key, Content, Message, Participant, Subpart, TurnEvent};

pub type SubId = u64;

/// What a subscription hears (`ChatBroker.Update`).
#[derive(Clone, Debug, PartialEq)]
pub enum Update {
    Delivery { message: Message, partial: bool },
    Typing { on: bool, who: Participant },
    Turn(TurnEvent),
    Removed { ids: Vec<String>, chat: String },
}

/// One step of a publish's writing: a database write, then, once every write succeeded,
/// `Settle`, which makes them final (the App releases its savepoint). A failed step ends the
/// publish before anything is delivered.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Step<'a> {
    Write(&'a Effect),
    Settle,
}

/// What the App does after a publish: the database writes, then the hand-off to the chat's
/// agent.
#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    Store(Message),
    Replace { unique_id: String, content: Content },
    Remove { chat: String, ids: Vec<String> },
    Bump(String),
    Rename { chat: String, title: String },
    ToService(Message),
}

/// Rewrites or squelches (None) a message before it is stored and delivered: (message, chat
/// id, partial).
pub type Processor = Box<dyn FnMut(Message, &str, bool) -> Option<Message>>;

struct Sub {
    id: SubId,
    /// None: every chat.
    chat: Option<String>,
    queue: Vec<(String, Update)>,
}

/// The broker and the service's per-chat queue. Its newest-id test for a streamed chunk is
/// right only while every stored message passes through `publish`, `publish_with` or
/// `preload`; code that writes rows itself (a fork's clones) calls `seed_newest` or `forget`.
pub struct Broker {
    processors: Vec<Processor>,
    subs: Vec<Sub>,
    next_sub: SubId,
    /// Each known chat's newest stored message id (None: the chat has none).
    newest: HashMap<String, Option<String>>,
    /// Per chat, a streamed message's upstream id to the id its chunks now go to, until its
    /// commit (`ChatClient.swift:12,302-320`).
    rewrite: HashMap<String, HashMap<String, String>>,
    typing: HashSet<(String, Participant)>,
    in_turn: HashSet<String>,
    /// Chats whose agent is busy with a turn: `next_turn` hands out nothing until `turn_done`.
    busy: HashSet<String>,
    queues: HashMap<String, VecDeque<Message>>,
    /// Chats whose running message is a watcher event (iTerm2 keeps the in-flight message at
    /// the queue's head, `ChatService.swift:31-39`; the queue here holds only waiting ones).
    running_watcher: HashSet<String>,
}

impl Default for Broker {
    fn default() -> Broker {
        Broker::new()
    }
}

impl Broker {
    pub fn new() -> Broker {
        Broker {
            processors: Vec::new(),
            subs: Vec::new(),
            next_sub: 1,
            newest: HashMap::new(),
            rewrite: HashMap::new(),
            typing: HashSet::new(),
            in_turn: HashSet::new(),
            busy: HashSet::new(),
            queues: HashMap::new(),
            running_watcher: HashSet::new(),
        }
    }

    /// Processors run in the order added; any may rewrite or squelch.
    pub fn add_processor(&mut self, p: Processor) {
        self.processors.push(p);
    }

    /// Subscribes to one chat, or to every chat with None. The queue grows until drained, so
    /// a subscriber that stops draining unsubscribes.
    pub fn subscribe(&mut self, chat: Option<&str>) -> SubId {
        let id = self.next_sub;
        self.next_sub += 1;
        self.subs.push(Sub { id, chat: chat.map(str::to_string), queue: Vec::new() });
        id
    }

    pub fn unsubscribe(&mut self, id: SubId) {
        self.subs.retain(|s| s.id != id);
    }

    /// The updates the subscription heard since its last drain, oldest first, each with its
    /// chat id.
    pub fn drain(&mut self, id: SubId) -> Vec<(String, Update)> {
        self.subs.iter_mut().find(|s| s.id == id).map(|s| std::mem::take(&mut s.queue)).unwrap_or_default()
    }

    /// `fanOut` (`ChatBroker.swift:162-167`): the chat's subscriptions and the wildcard ones.
    fn fan_out(&mut self, chat: &str, update: Update) {
        for sub in self.subs.iter_mut().filter(|s| s.chat.as_deref().is_none_or(|c| c == chat)) {
            sub.queue.push((chat.to_string(), update.clone()));
        }
    }

    /// Whether the chat's newest id is known (seeded, or seen since launch).
    pub fn knows(&self, chat: &str) -> bool {
        self.newest.contains_key(chat)
    }

    /// The newest id of a chat the broker has not seen since launch (`Store::newest_id`).
    pub fn seed_newest(&mut self, chat: &str, id: Option<String>) {
        self.newest.insert(chat.to_string(), id);
    }

    /// The id a chunk or commit for `uuid` goes to after the rewrite table.
    pub fn append_target(&self, chat: &str, uuid: &str) -> String {
        self.rewrite.get(chat).and_then(|t| t.get(uuid)).cloned().unwrap_or_else(|| uuid.to_string())
    }

    /// `publish(message:toChatID:partial:)` (`ChatBroker.swift:117-133`) with the database writes
    /// answered as effects: every effect in order, the writes first, then the hand-off to the
    /// service. `target` is the stored row an `Append`, `AppendAttachment` or `Commit` resolves
    /// to through `append_target`, None for other contents or when that row is missing.
    pub fn publish(&mut self, chat: &str, message: Message, partial: bool, target: Option<&Message>) -> Vec<Effect> {
        let mut written = Vec::new();
        let after = self.publish_with(chat, message, partial, target, &mut |step: Step| {
            if let Step::Write(e) = step {
                written.push(e.clone());
            }
            Ok(())
        });
        written.extend(after.unwrap_or_default());
        written
    }

    /// `publish` storing first, as iTerm2 does (`try listModel.append` before `fanOut`): the
    /// processors and the streaming rule, then `write` for each database write in order. A
    /// failed write ends the publish with its error: the message is neither delivered nor
    /// queued nor handed to the service, and the chat's newest id stays. Otherwise the message
    /// is delivered, and the answer is the hand-off to the service (`Effect::ToService`), if any.
    pub fn publish_with(&mut self, chat: &str, message: Message, partial: bool, target: Option<&Message>, write: &mut dyn FnMut(Step) -> Result<(), String>) -> Result<Vec<Effect>, String> {
        let mut processed = message;
        for p in self.processors.iter_mut() {
            match p(processed, chat, partial) {
                Some(m) => processed = m,
                // Squelched: nothing to write, but the caller's unit is still settled.
                None => return write(Step::Settle).map(|()| Vec::new()),
            }
        }
        // A chunk that restarts the stream records its rewrite first; a failed write takes the
        // record back, so later chunks do not follow a message that was never stored.
        let upstream = match &processed.content {
            Content::Append { uuid, .. } | Content::AppendAttachment { uuid, .. } => Some(uuid.clone()),
            _ => None,
        };
        let before = upstream.as_ref().map(|u| self.rewrite.get(chat).and_then(|t| t.get(u)).cloned());
        let decided = self.append(chat, processed, target);
        // The writes, then settled, all before the broker moves or anything is delivered.
        let steps = decided.writes.iter().map(Step::Write).chain(std::iter::once(Step::Settle));
        for step in steps {
            if let Err(error) = write(step) {
                if let (Some(u), Some(before)) = (upstream, before) {
                    let table = self.rewrite.entry(chat.to_string()).or_default();
                    match before {
                        Some(id) => table.insert(u, id),
                        None => table.remove(&u),
                    };
                }
                return Err(error);
            }
        }
        let mut after = Vec::new();
        if decided.stored {
            self.newest.insert(chat.to_string(), Some(decided.message.unique_id.clone()));
            if routes_to_service(&decided.message) {
                if !bypasses_queue(&decided.message.content) {
                    self.enqueue(chat, decided.message.clone());
                }
                after.push(Effect::ToService(decided.message.clone()));
            }
        }
        self.fan_out(chat, Update::Delivery { message: decided.message, partial });
        Ok(after)
    }

    /// Queues a message: a person's message goes before the queued watcher events, never before
    /// the running message (`ChatService.swift:365-403`, `queue_insertion_index`).
    fn enqueue(&mut self, chat: &str, m: Message) {
        let busy = self.busy.contains(chat);
        let mut flags: Vec<bool> = Vec::new();
        if busy {
            flags.push(self.running_watcher.contains(chat));
        }
        let queue = self.queues.entry(chat.to_string()).or_default();
        flags.extend(queue.iter().map(is_watcher));
        let at = crate::orch_watch::queue_insertion_index(is_watcher(&m), &flags);
        queue.insert(if busy { at.saturating_sub(1) } else { at }, m);
    }

    /// The streaming rule (`ChatClient.processAppend`, `processCommit`) and the list model's
    /// append; answers the message as delivered and its database writes.
    fn append(&mut self, chat: &str, mut m: Message, target: Option<&Message>) -> Decided {
        let unstored = |message: Message, writes: Vec<Effect>| Decided { message, writes, stored: false };
        match m.content.clone() {
            Content::Append { text, uuid } => {
                let Some(t) = target else { return unstored(m, Vec::new()) };
                if let Some(fresh) = self.restart_stream(chat, &m, &uuid, t, Some(&text)) {
                    return stored(chat, fresh);
                }
                let resolved = t.unique_id.clone();
                m.content = Content::Append { text: text.clone(), uuid: resolved.clone() };
                let writes = appended_text(&t.content, &text).map(|content| Effect::Replace { unique_id: resolved, content }).into_iter().collect();
                unstored(m, writes)
            }
            Content::AppendAttachment { attachment, uuid } => {
                let Some(t) = target else { return unstored(m, Vec::new()) };
                if let Some(fresh) = self.restart_stream(chat, &m, &uuid, t, None) {
                    return stored(chat, fresh);
                }
                let resolved = t.unique_id.clone();
                m.content = Content::AppendAttachment { attachment: attachment.clone(), uuid: resolved.clone() };
                let writes = appended_attachment(&t.content, &attachment).map(|content| Effect::Replace { unique_id: resolved, content }).into_iter().collect();
                unstored(m, writes)
            }
            Content::Commit(uuid) => {
                if m.author == Participant::Agent {
                    if let Some(table) = self.rewrite.get_mut(chat) {
                        table.remove(&uuid);
                    }
                }
                // The commit is the settling point: status updates leave the body
                // (`ChatListModel.swift:329-345`). The commit itself is not stored.
                let writes = target
                    .and_then(|t| without_status(&t.content).filter(|c| *c != t.content).map(|content| Effect::Replace { unique_id: t.unique_id.clone(), content }))
                    .into_iter()
                    .collect();
                unstored(m, writes)
            }
            _ => stored(chat, m),
        }
    }

    /// `processAppend`'s first branch (`ChatClient.swift:305-338`): an agent's chunk for a plain
    /// text or Markdown message that is no longer the chat's newest starts a new message with
    /// a fresh id, and later chunks for the same upstream id go to it until the commit. None
    /// when the chunk appends in place.
    fn restart_stream(&mut self, chat: &str, m: &Message, upstream: &str, target: &Message, text: Option<&str>) -> Option<Message> {
        if m.author != Participant::Agent || !matches!(target.content, Content::PlainText { .. } | Content::Markdown(_)) {
            return None;
        }
        if self.newest.get(chat).and_then(Option::as_deref) == Some(target.unique_id.as_str()) {
            return None;
        }
        let content = match (text, &target.content, &m.content) {
            (Some(t), Content::PlainText { .. }, _) => Content::PlainText { text: t.to_string(), context: None },
            (Some(t), _, _) => Content::Markdown(t.to_string()),
            // iTerm2 stops here ("Server-sent attachments are not yet supported"); Termurion starts
            // the new message with the attachment (Decision).
            (None, _, Content::AppendAttachment { attachment, .. }) => Content::Multipart { parts: vec![Subpart::Attachment(attachment.clone())], vector_store_id: None },
            (None, _, _) => return None,
        };
        let fresh = Message { unique_id: new_id(), content, ..m.clone() };
        self.rewrite.entry(chat.to_string()).or_default().insert(upstream.to_string(), fresh.unique_id.clone());
        Some(fresh)
    }

    /// A message stored through the list model alone, without processors, fan-out or the
    /// service: a new chat's preloaded messages (`ChatBroker.swift:102-110`). The chat's newest
    /// id moves only once every write succeeded.
    pub fn preload(&mut self, chat: &str, m: Message, write: &mut dyn FnMut(Step) -> Result<(), String>) -> Result<(), String> {
        let id = m.unique_id.clone();
        for e in &stored(chat, m).writes {
            write(Step::Write(e))?;
        }
        write(Step::Settle)?;
        self.newest.insert(chat.to_string(), Some(id));
        Ok(())
    }

    /// `publish(typingStatus:of:toChatID:)` (`ChatBroker.swift:135-140`).
    pub fn typing(&mut self, chat: &str, on: bool, who: Participant) {
        if on {
            self.typing.insert((chat.to_string(), who));
        } else {
            self.typing.remove(&(chat.to_string(), who));
        }
        self.fan_out(chat, Update::Typing { on, who });
    }

    pub fn is_typing(&self, chat: &str, who: Participant) -> bool {
        self.typing.contains(&(chat.to_string(), who))
    }

    /// `publish(turnEvent:toChatID:)` (`ChatBroker.swift:142-145`). A started turn also holds the
    /// queue until `turn_done`.
    pub fn turn(&mut self, chat: &str, e: TurnEvent) {
        match e {
            TurnEvent::Started => {
                self.in_turn.insert(chat.to_string());
                self.busy.insert(chat.to_string());
            }
            TurnEvent::Ended => {
                self.in_turn.remove(chat);
            }
        }
        self.fan_out(chat, Update::Turn(e));
    }

    pub fn turn_in_progress(&self, chat: &str) -> bool {
        self.in_turn.contains(chat)
    }

    /// `deleteMessages` (`ChatBroker.swift:147-160`): the removed ids are a suffix of the chat.
    /// The chat's newest id is forgotten, so the next publish reads it again.
    pub fn remove(&mut self, chat: &str, ids: Vec<String>) -> Vec<Effect> {
        if ids.is_empty() {
            return Vec::new();
        }
        self.newest.remove(chat);
        self.fan_out(chat, Update::Removed { ids: ids.clone(), chat: chat.to_string() });
        vec![Effect::Remove { chat: chat.to_string(), ids }]
    }

    /// The next queued message to answer when no turn runs; it leaves the queue and holds it
    /// until `turn_done` (iTerm2 keeps the in-flight message at the queue's head,
    /// `ChatService.swift:31-39`).
    pub fn next_turn(&mut self, chat: &str) -> Option<Message> {
        if self.busy.contains(chat) {
            return None;
        }
        let m = self.queues.get_mut(chat)?.pop_front()?;
        self.busy.insert(chat.to_string());
        if is_watcher(&m) {
            self.running_watcher.insert(chat.to_string());
        } else {
            self.running_watcher.remove(chat);
        }
        Some(m)
    }

    /// The running turn finished; the queue moves on.
    pub fn turn_done(&mut self, chat: &str) {
        self.busy.remove(chat);
        self.running_watcher.remove(chat);
    }

    /// The ids of the chat's messages still waiting for their turn (iTerm2's `historyExcluding`
    /// leaves them out of a turn's history, `ChatService.swift:517-525`).
    pub fn queued_ids(&self, chat: &str) -> Vec<String> {
        self.queues.get(chat).map(|q| q.iter().map(|m| m.unique_id.clone()).collect()).unwrap_or_default()
    }

    /// Drops the chat's queued messages; the running turn is its own (Stop,
    /// `ChatService.swift:307-321`).
    pub fn clear_queue(&mut self, chat: &str) {
        self.queues.remove(chat);
    }

    /// Everything the broker holds for a deleted chat.
    pub fn forget(&mut self, chat: &str) {
        self.newest.remove(chat);
        self.rewrite.remove(chat);
        self.typing.retain(|(c, _)| c != chat);
        self.in_turn.remove(chat);
        self.busy.remove(chat);
        self.running_watcher.remove(chat);
        self.queues.remove(chat);
    }
}

/// What the streaming rule and the list model decided for one published message.
struct Decided {
    /// The message as delivered.
    message: Message,
    /// The database writes, in order.
    writes: Vec<Effect>,
    /// A new row: it becomes the chat's newest and a user's goes to the service.
    stored: bool,
}

/// The list model's append of a whole message (`ChatListModel.swift:264-287`): stored, then a
/// rename renames, a vector store changes nothing more, and anything else bumps.
fn stored(chat: &str, m: Message) -> Decided {
    let follow = match &m.content {
        Content::RenameChat(title) => Some(Effect::Rename { chat: chat.to_string(), title: title.clone() }),
        Content::VectorStoreCreated(_) => None,
        _ => Some(Effect::Bump(chat.to_string())),
    };
    let writes = std::iter::once(Effect::Store(m.clone())).chain(follow).collect();
    Decided { message: m, writes, stored: true }
}

/// Messages that never drive a turn and skip the queue (`ChatService.swift:130-178`):
/// client-local state, user commands, permission changes, a tool's response (it resumes the
/// turn that asked for it) and a message this build cannot read.
pub fn bypasses_queue(c: &Content) -> bool {
    matches!(c, Content::ClientLocal(_) | Content::UserCommand(_) | Content::SetPermissions(_) | Content::RemoteCommandResponse { .. } | Content::Unsupported(_))
}

/// A watcher event (`Content::WatcherEvent`), which a person's message does not wait behind.
fn is_watcher(m: &Message) -> bool {
    matches!(m.content, Content::WatcherEvent(_))
}

/// The service handles every user-authored message but client-local ones
/// (`ChatService.swift:116-155`).
pub fn routes_to_service(m: &Message) -> bool {
    m.author == Participant::User && !matches!(m.content, Content::ClientLocal(_))
}

fn is_status(part: &Subpart) -> bool {
    matches!(part, Subpart::Attachment(a) if a.get("type").and_then(one_key).is_some_and(|(k, _)| k == "statusUpdate"))
}

/// `removeReasoningStatusSubparts` (`Message.swift:589-625`): a multipart body without its
/// status updates; None for other bodies. The reasoning is not harvested into
/// `agentReasoning` here: 12b's turn reports it whole as `Out::Reasoning`, which the agent
/// stores (`Engine`), so harvesting too would store it twice (Decision).
fn without_status(c: &Content) -> Option<Content> {
    match c {
        Content::Multipart { parts, vector_store_id } => {
            Some(Content::Multipart { parts: parts.iter().filter(|p| !is_status(p)).cloned().collect(), vector_store_id: vector_store_id.clone() })
        }
        _ => None,
    }
}

/// `Message.append(_:useMarkdownIfAmbiguous: true)` after the status updates leave
/// (`Message.swift:655-691`); None for a body that cannot take text (iTerm2 stops there), so an
/// unsupported row is never rewritten.
fn appended_text(c: &Content, chunk: &str) -> Option<Content> {
    Some(match c {
        Content::PlainText { text, .. } => Content::PlainText { text: format!("{text}{chunk}"), context: None },
        Content::Markdown(text) => Content::Markdown(format!("{text}{chunk}")),
        Content::Multipart { .. } => {
            let Some(Content::Multipart { mut parts, vector_store_id }) = without_status(c) else { return None };
            match parts.last_mut() {
                Some(Subpart::Markdown(s) | Subpart::PlainText(s)) => s.push_str(chunk),
                _ => parts.push(Subpart::Markdown(chunk.to_string())),
            }
            Content::Multipart { parts, vector_store_id }
        }
        _ => return None,
    })
}

/// `Message.append(_ attachment:vectorStoreID:)` (`Message.swift:627-653`); None for a body
/// that cannot take one. Only the agent streams attachments (`ChatService.swift:464-485`), so a
/// text body becomes a multipart without a vector store.
fn appended_attachment(c: &Content, attachment: &Value) -> Option<Content> {
    Some(match c {
        Content::PlainText { text, .. } => Content::Multipart { parts: vec![Subpart::PlainText(text.clone()), Subpart::Attachment(attachment.clone())], vector_store_id: None },
        Content::Markdown(text) => Content::Multipart { parts: vec![Subpart::Markdown(text.clone()), Subpart::Attachment(attachment.clone())], vector_store_id: None },
        Content::Multipart { parts, vector_store_id } => {
            let mut parts = parts.clone();
            match parts.last().and_then(|p| if let Subpart::Attachment(a) = p { attachment_appending(a, attachment) } else { None }) {
                Some(combined) => {
                    let last = parts.len() - 1;
                    parts[last] = Subpart::Attachment(combined);
                }
                None => parts.push(Subpart::Attachment(attachment.clone())),
            }
            Content::Multipart { parts, vector_store_id: vector_store_id.clone() }
        }
        _ => return None,
    })
}

/// A status update's single updates in order (`StatusUpdate.exploded`).
fn exploded(status: &Value) -> Vec<Value> {
    match one_key(status) {
        Some(("multipart", body)) => body.get("_0").and_then(Value::as_array).map(|parts| parts.iter().flat_map(exploded).collect()).unwrap_or_default(),
        _ => vec![status.clone()],
    }
}

/// `Attachment.appending` (`LLM.swift:158-192`) on the stored JSON: status updates always merge;
/// code and files of the same id concatenate; anything else is a new part (None).
fn attachment_appending(lhs: &Value, rhs: &Value) -> Option<Value> {
    let (lkind, lbody) = one_key(lhs.get("type")?)?;
    let (rkind, rbody) = one_key(rhs.get("type")?)?;
    let first = |body: &Value| body.get("_0").cloned();
    if lkind == "statusUpdate" && rkind == "statusUpdate" {
        let parts: Vec<Value> = exploded(&first(lbody)?).into_iter().chain(exploded(&first(rbody)?)).collect();
        let mut result = lhs.clone();
        result["type"] = case("statusUpdate", json!({"_0": case("multipart", json!({"_0": parts}))}));
        return Some(result);
    }
    if lhs.get("id") != rhs.get("id") {
        return None;
    }
    let ty = match (lkind, rkind) {
        ("code", "code") => case("code", json!({"_0": format!("{}{}", first(lbody)?.as_str()?, first(rbody)?.as_str()?)})),
        ("file", "file") => {
            let (l, r) = (first(lbody)?, first(rbody)?);
            let text = |v: &Value, key: &str| v.get(key).and_then(Value::as_str).map(str::to_string);
            let b64 = base64::engine::general_purpose::STANDARD;
            let mut content = b64.decode(text(&l, "content")?).ok()?;
            content.extend(b64.decode(text(&r, "content")?).ok()?);
            let mut file = serde_json::Map::new();
            file.insert("name".into(), Value::String(text(&l, "name")? + &text(&r, "name")?));
            file.insert("content".into(), Value::String(b64.encode(content)));
            file.insert("mimeType".into(), Value::String(text(&l, "mimeType")? + &text(&r, "mimeType")?));
            // `String?.concat` (`String+iTerm.swift:376-387`); nil is left out.
            let local = match (text(&l, "localPath"), text(&r, "localPath")) {
                (Some(a), Some(b)) => Some(a + &b),
                (a, b) => a.or(b),
            };
            if let Some(p) = local {
                file.insert("localPath".into(), Value::String(p));
            }
            case("file", json!({"_0": Value::Object(file)}))
        }
        _ => return None,
    };
    let mut result = serde_json::Map::new();
    result.insert("inline".into(), lhs.get("inline")?.clone());
    result.insert("id".into(), lhs.get("id")?.clone());
    result.insert("type".into(), ty);
    Some(Value::Object(result))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat_message::{new_id, ClientLocal, Content, Message, Participant, Subpart, TurnEvent, UserCommand};
    use serde_json::json;

    fn msg(author: Participant, content: Content) -> Message {
        Message::new("C", author, content, 1000.0)
    }

    fn user(text: &str) -> Message {
        msg(Participant::User, Content::PlainText { text: text.into(), context: None })
    }

    fn agent_md(text: &str) -> Message {
        msg(Participant::Agent, Content::Markdown(text.into()))
    }

    fn append(text: &str, uuid: &str) -> Message {
        msg(Participant::Agent, Content::Append { text: text.into(), uuid: uuid.into() })
    }

    fn stored(effects: &[Effect]) -> Vec<&Message> {
        effects.iter().filter_map(|e| if let Effect::Store(m) = e { Some(m) } else { None }).collect()
    }

    #[test]
    fn publish_follows_iterm2() {
        let mut b = Broker::new();
        let c = b.subscribe(Some("C"));
        let all = b.subscribe(None);
        let d = b.subscribe(Some("D"));
        b.seed_newest("C", None);

        let m = user("hi");
        let effects = b.publish("C", m.clone(), false, None);
        assert_eq!(effects, vec![Effect::Store(m.clone()), Effect::Bump("C".into()), Effect::ToService(m.clone())]);
        let delivery = ("C".to_string(), Update::Delivery { message: m.clone(), partial: false });
        assert_eq!(b.drain(c), vec![delivery.clone()]);
        assert_eq!(b.drain(all), vec![delivery]);
        assert_eq!(b.drain(d), vec![], "another chat's subscription hears nothing");
        assert_eq!(b.drain(c), vec![], "drained once");

        let a = agent_md("x");
        assert_eq!(b.publish("C", a.clone(), true, None), vec![Effect::Store(a.clone()), Effect::Bump("C".into())], "the agent's own message is not the service's");
        assert_eq!(b.drain(c), vec![("C".to_string(), Update::Delivery { message: a, partial: true })]);

        let r = msg(Participant::Agent, Content::RenameChat("t".into()));
        assert_eq!(b.publish("C", r.clone(), false, None), vec![Effect::Store(r), Effect::Rename { chat: "C".into(), title: "t".into() }], "a rename does not bump");
        let v = msg(Participant::Agent, Content::VectorStoreCreated("vs".into()));
        assert_eq!(b.publish("C", v.clone(), false, None), vec![Effect::Store(v)], "a vector store does not bump");

        let mut b = Broker::new();
        let s = b.subscribe(None);
        b.add_processor(Box::new(|m: Message, _: &str, _: bool| match &m.content {
            Content::PlainText { text, .. } if text == "drop" => None,
            Content::PlainText { text, .. } if text == "edit" => Some(Message { content: Content::PlainText { text: "edited".into(), context: None }, ..m }),
            _ => Some(m),
        }));
        assert_eq!(b.publish("C", user("drop"), false, None), vec![], "squelched");
        assert_eq!(b.drain(s), vec![]);
        let e = user("edit");
        let effects = b.publish("C", e.clone(), false, None);
        let want = Message { content: Content::PlainText { text: "edited".into(), context: None }, ..e };
        assert_eq!(effects[0], Effect::Store(want.clone()));
        assert_eq!(b.drain(s), vec![("C".to_string(), Update::Delivery { message: want, partial: false })]);

        b.unsubscribe(s);
        b.publish("C", user("again"), false, None);
        assert_eq!(b.drain(s), vec![], "an ended subscription hears nothing");
    }

    #[test]
    fn append_rewrite_follows_iterm2() {
        let mut b = Broker::new();
        let target = agent_md("a");
        let a = target.unique_id.clone();
        b.seed_newest("C", Some(a.clone()));
        assert!(b.knows("C"));

        let effects = b.publish("C", append("b", &a), true, Some(&target));
        assert_eq!(effects, vec![Effect::Replace { unique_id: a.clone(), content: Content::Markdown("ab".into()) }], "appended in place, never stored or bumped");

        b.publish("C", user("q2"), false, None);
        assert_eq!(b.append_target("C", &a), a, "no rewrite yet");
        let target = agent_md("ab");
        let effects = b.publish("C", append("b", &a), true, Some(&target));
        let fresh = stored(&effects);
        assert_eq!(fresh.len(), 1, "a chunk for a message that is no longer newest starts a new one: {effects:?}");
        let n = fresh[0].unique_id.clone();
        assert_ne!(n, a);
        assert_eq!(fresh[0].content, Content::Markdown("b".into()));
        assert_eq!(effects[1], Effect::Bump("C".into()));
        assert_eq!(b.append_target("C", &a), n);

        let target_n = Message { unique_id: n.clone(), ..agent_md("b") };
        let s = b.subscribe(Some("C"));
        let effects = b.publish("C", append("c", &a), true, Some(&target_n));
        assert_eq!(effects, vec![Effect::Replace { unique_id: n.clone(), content: Content::Markdown("bc".into()) }]);
        let delivered = b.drain(s);
        assert!(
            matches!(&delivered[0].1, Update::Delivery { message: Message { content: Content::Append { uuid, .. }, .. }, partial: true } if *uuid == n),
            "the delivered chunk names the rewritten id: {delivered:?}"
        );

        let commit = msg(Participant::Agent, Content::Commit(a.clone()));
        let target_n = Message { unique_id: n.clone(), ..agent_md("bc") };
        let effects = b.publish("C", commit, false, Some(&target_n));
        assert_eq!(effects, vec![], "the commit is not stored (ChatListModel.swift:329-345) and nothing is stripped");
        assert_eq!(b.append_target("C", &a), a, "the commit ends the rewrite");

        assert_eq!(b.publish("C", append("z", &new_id()), true, None), vec![], "an append for a missing row is dropped");

        let mut b = Broker::new();
        let p = Message { content: Content::PlainText { text: "x".into(), context: Some("ctx".into()) }, ..agent_md("") };
        b.seed_newest("C", Some(p.unique_id.clone()));
        let effects = b.publish("C", append("y", &p.unique_id), true, Some(&p));
        assert_eq!(effects, vec![Effect::Replace { unique_id: p.unique_id.clone(), content: Content::PlainText { text: "xy".into(), context: None } }], "plain text stays plain text");

        let u = Message { content: Content::Unsupported("{}".into()), ..agent_md("") };
        b.seed_newest("C", Some(u.unique_id.clone()));
        assert_eq!(b.publish("C", append("y", &u.unique_id), true, Some(&u)), vec![], "an unsupported row is never rewritten");

        assert!(!b.knows("D"));
        b.seed_newest("D", None);
        assert!(b.knows("D"));
    }

    #[test]
    fn streamed_attachments_follow_iterm2() {
        let status = |s: serde_json::Value| json!({"inline": true, "id": "w1", "type": {"statusUpdate": {"_0": s}}});
        let code = |id: &str, s: &str| json!({"inline": true, "id": id, "type": {"code": {"_0": s}}});
        let mut b = Broker::new();
        let first = Message { content: Content::Multipart { parts: vec![Subpart::Attachment(status(json!({"webSearchStarted": {}})))], vector_store_id: None }, ..agent_md("") };
        let id = first.unique_id.clone();
        b.seed_newest("C", Some(id.clone()));

        let more = msg(Participant::Agent, Content::AppendAttachment { attachment: status(json!({"webSearchFinished": {"_0": "cats"}})), uuid: id.clone() });
        let effects = b.publish("C", more, true, Some(&first));
        let merged = Content::Multipart {
            parts: vec![Subpart::Attachment(status(json!({"multipart": {"_0": [{"webSearchStarted": {}}, {"webSearchFinished": {"_0": "cats"}}]}})))],
            vector_store_id: None,
        };
        assert_eq!(effects, vec![Effect::Replace { unique_id: id.clone(), content: merged.clone() }], "status updates always merge (LLM.swift:158-165)");

        let with_status = Message { content: merged, ..first.clone() };
        let effects = b.publish("C", append("Hi", &id), true, Some(&with_status));
        let text = Content::Multipart { parts: vec![Subpart::Markdown("Hi".into())], vector_store_id: None };
        assert_eq!(effects, vec![Effect::Replace { unique_id: id.clone(), content: text.clone() }], "a text chunk strips the status updates, then adds markdown");

        let with_text = Message { content: text, ..first.clone() };
        let c1 = msg(Participant::Agent, Content::AppendAttachment { attachment: code("k", "a"), uuid: id.clone() });
        let effects = b.publish("C", c1, true, Some(&with_text));
        let one = Content::Multipart { parts: vec![Subpart::Markdown("Hi".into()), Subpart::Attachment(code("k", "a"))], vector_store_id: None };
        assert_eq!(effects, vec![Effect::Replace { unique_id: id.clone(), content: one.clone() }]);
        let with_code = Message { content: one, ..first.clone() };
        let c2 = msg(Participant::Agent, Content::AppendAttachment { attachment: code("k", "b"), uuid: id.clone() });
        let effects = b.publish("C", c2, true, Some(&with_code));
        let two = Content::Multipart { parts: vec![Subpart::Markdown("Hi".into()), Subpart::Attachment(code("k", "ab"))], vector_store_id: None };
        assert_eq!(effects, vec![Effect::Replace { unique_id: id.clone(), content: two }], "code of the same id accumulates");
        let c3 = msg(Participant::Agent, Content::AppendAttachment { attachment: code("j", "c"), uuid: id.clone() });
        let effects = b.publish("C", c3, true, Some(&with_code));
        let apart = Content::Multipart { parts: vec![Subpart::Markdown("Hi".into()), Subpart::Attachment(code("k", "a")), Subpart::Attachment(code("j", "c"))], vector_store_id: None };
        assert_eq!(effects, vec![Effect::Replace { unique_id: id.clone(), content: apart }], "code of another id is a new part");

        let md = agent_md("t");
        b.seed_newest("C", Some(md.unique_id.clone()));
        let a = msg(Participant::Agent, Content::AppendAttachment { attachment: code("k", "a"), uuid: md.unique_id.clone() });
        let effects = b.publish("C", a, true, Some(&md));
        let parts = Content::Multipart { parts: vec![Subpart::Markdown("t".into()), Subpart::Attachment(code("k", "a"))], vector_store_id: None };
        assert_eq!(effects, vec![Effect::Replace { unique_id: md.unique_id.clone(), content: parts }], "markdown becomes multipart");

        let only_status = Message { content: Content::Multipart { parts: vec![Subpart::Attachment(status(json!({"webSearchStarted": {}})))], vector_store_id: None }, ..first };
        let commit = msg(Participant::Agent, Content::Commit(id.clone()));
        let effects = b.publish("C", commit, false, Some(&only_status));
        assert_eq!(effects, vec![Effect::Replace { unique_id: id, content: Content::Multipart { parts: vec![], vector_store_id: None } }], "the commit settles the status updates");

        b.seed_newest("C", Some(new_id()));
        let late = msg(Participant::Agent, Content::AppendAttachment { attachment: code("k", "a"), uuid: md.unique_id.clone() });
        let effects = b.publish("C", late, true, Some(&md));
        let fresh = stored(&effects);
        assert_eq!(fresh.len(), 1, "an attachment for a message that is no longer newest starts a new one: {effects:?}");
        assert_eq!(fresh[0].content, Content::Multipart { parts: vec![Subpart::Attachment(code("k", "a"))], vector_store_id: None });
        assert_eq!(b.append_target("C", &md.unique_id), fresh[0].unique_id);
    }

    #[test]
    fn store_comes_before_delivery() {
        let mut b = Broker::new();
        let s = b.subscribe(None);
        let a = agent_md("a");
        b.seed_newest("C", Some(a.unique_id.clone()));
        let mut refuse = |_: Step| Err("full".to_string());
        assert_eq!(b.publish_with("C", user("q"), false, None, &mut refuse), Err("full".to_string()));
        assert_eq!(b.drain(s), vec![], "an unstored message is not delivered");
        assert_eq!(b.next_turn("C"), None, "nor queued");
        let effects = b.publish("C", append("b", &a.unique_id), true, Some(&a));
        assert_eq!(effects, vec![Effect::Replace { unique_id: a.unique_id.clone(), content: Content::Markdown("ab".into()) }], "the newest id did not move");
        b.drain(s);
        assert!(b.publish_with("C", append("c", &a.unique_id), true, Some(&a), &mut refuse).is_err());
        assert_eq!(b.drain(s), vec![], "a chunk that was not written is not delivered");

        // Written in order, then delivered; the answer is the hand-off.
        let mut seen = Vec::new();
        let q = user("q2");
        let after = b.publish_with("C", q.clone(), false, None, &mut |step: Step| {
            seen.push(match step {
                Step::Write(e) => Some(e.clone()),
                Step::Settle => None,
            });
            Ok(())
        });
        assert_eq!(seen, vec![Some(Effect::Store(q.clone())), Some(Effect::Bump("C".into())), None], "settled after the writes");
        assert_eq!(after, Ok(vec![Effect::ToService(q.clone())]));
        assert_eq!(b.drain(s), vec![("C".to_string(), Update::Delivery { message: q.clone(), partial: false })]);
        assert_eq!(b.next_turn("C"), Some(q));

        // A chunk that would restart the stream but cannot be written leaves no rewrite behind.
        assert!(b.publish_with("C", append("d", &a.unique_id), true, Some(&a), &mut refuse).is_err());
        assert_eq!(b.append_target("C", &a.unique_id), a.unique_id);
        let effects = b.publish("C", append("d", &a.unique_id), true, Some(&a));
        let n = stored(&effects)[0].unique_id.clone();
        assert_eq!(b.append_target("C", &a.unique_id), n);
        // Once restarted, a failed chunk keeps the rewrite it found.
        let target_n = Message { unique_id: n.clone(), ..agent_md("d") };
        assert!(b.publish_with("C", append("e", &a.unique_id), true, Some(&target_n), &mut refuse).is_err());
        assert_eq!(b.append_target("C", &a.unique_id), n);

        // A preload moves the newest id only once written.
        let p = agent_md("p");
        assert!(b.preload("P", p.clone(), &mut refuse).is_err());
        assert!(!b.knows("P"));
        let mut unsettled = |step: Step| if step == Step::Settle { Err("busy".to_string()) } else { Ok(()) };
        assert!(b.preload("P", p.clone(), &mut unsettled).is_err());
        assert!(!b.knows("P"), "nor before its writes are settled");
        assert_eq!(b.preload("P", p.clone(), &mut |_: Step| Ok(())), Ok(()));
        assert_eq!(b.newest.get("P"), Some(&Some(p.unique_id)));

        // Every write stored but not settled: nothing is delivered, queued or moved.
        b.turn_done("C");
        b.drain(s);
        let before = b.newest.get("C").cloned();
        let q3 = user("q3");
        assert_eq!(b.publish_with("C", q3, false, None, &mut unsettled), Err("busy".to_string()));
        assert_eq!(b.drain(s), vec![]);
        assert_eq!(b.next_turn("C"), None);
        assert_eq!(b.newest.get("C").cloned(), before);
        assert!(b.publish_with("C", append("f", &a.unique_id), true, Some(&target_n), &mut unsettled).is_err());
        assert_eq!(b.append_target("C", &a.unique_id), n, "an unsettled chunk keeps the rewrite it found");
    }

    fn watcher(detail: &str) -> Message {
        msg(Participant::User, Content::WatcherEvent(json!({"_0": {"detail": detail}})))
    }

    /// Runs the queue dry: each answered message, in order, as a short label.
    fn drain(b: &mut Broker, label: &dyn Fn(&Message) -> String) -> Vec<String> {
        let mut order = Vec::new();
        loop {
            b.turn_done("C");
            match b.next_turn("C") {
                Some(m) => order.push(label(&m)),
                None => return order,
            }
        }
    }

    fn label_of(m: &Message) -> String {
        match &m.content {
            Content::PlainText { text, .. } => text.clone(),
            Content::WatcherEvent(v) => v["_0"]["detail"].as_str().unwrap_or_default().to_string(),
            _ => "?".into(),
        }
    }

    #[test]
    fn watcher_events_queue_behind_people() {
        // A person's message in flight; two watcher events wait; a person's message P arrives.
        let mut b = Broker::new();
        b.seed_newest("C", None);
        b.publish("C", user("M0"), false, None);
        assert_eq!(b.next_turn("C").map(|m| label_of(&m)), Some("M0".into()));
        b.turn("C", TurnEvent::Started);
        b.publish("C", watcher("A"), false, None);
        b.publish("C", watcher("B"), false, None);
        b.publish("C", user("P"), false, None);
        assert_eq!(drain(&mut b, &label_of), ["P", "A", "B"], "a person's message is answered before the queued watcher events");

        // A person's message waits behind another person's, but before the watcher event.
        let mut b = Broker::new();
        b.seed_newest("C", None);
        b.publish("C", user("M0"), false, None);
        b.next_turn("C");
        b.turn("C", TurnEvent::Started);
        b.publish("C", user("X"), false, None);
        b.publish("C", watcher("A"), false, None);
        b.publish("C", user("P"), false, None);
        b.publish("C", watcher("C"), false, None);
        assert_eq!(drain(&mut b, &label_of), ["X", "P", "A", "C"], "P after X and before A; a watcher event published meanwhile goes last");

        // A watcher event in flight and nothing waiting: P is next.
        let mut b = Broker::new();
        b.seed_newest("C", None);
        b.publish("C", watcher("W0"), false, None);
        assert_eq!(b.next_turn("C").map(|m| label_of(&m)), Some("W0".into()));
        b.turn("C", TurnEvent::Started);
        b.publish("C", user("P"), false, None);
        assert_eq!(drain(&mut b, &label_of), ["P"]);

        // A watcher event in flight, a watcher event waiting: P goes before the waiting one only.
        let mut b = Broker::new();
        b.seed_newest("C", None);
        b.publish("C", watcher("W0"), false, None);
        b.next_turn("C");
        b.turn("C", TurnEvent::Started);
        b.publish("C", watcher("W1"), false, None);
        b.publish("C", user("P"), false, None);
        assert_eq!(drain(&mut b, &label_of), ["P", "W1"]);

        // Nothing running: arrival order, as 12c's queue.
        let mut b = Broker::new();
        b.seed_newest("C", None);
        b.publish("C", watcher("A"), false, None);
        b.publish("C", user("P"), false, None);
        assert_eq!(b.next_turn("C").map(|m| label_of(&m)), Some("A".into()), "the first message starts at once");
        assert_eq!(drain(&mut b, &label_of), ["P"]);
    }

    #[test]
    fn queue_follows_iterm2() {
        let mut b = Broker::new();
        let s = b.subscribe(Some("C"));
        b.seed_newest("C", None);
        assert_eq!(b.next_turn("C"), None);
        b.turn("C", TurnEvent::Started);
        assert!(b.turn_in_progress("C"));
        assert_eq!(b.drain(s), vec![("C".to_string(), Update::Turn(TurnEvent::Started))]);
        let one = user("one");
        let two = user("two");
        b.publish("C", one.clone(), false, None);
        b.publish("C", two.clone(), false, None);
        assert_eq!(b.next_turn("C"), None, "a turn runs");
        b.turn("C", TurnEvent::Ended);
        assert!(!b.turn_in_progress("C"));
        b.turn_done("C");
        assert_eq!(b.next_turn("C"), Some(one));
        assert_eq!(b.next_turn("C"), None, "the first message's turn runs");
        b.turn_done("C");
        assert_eq!(b.next_turn("C"), Some(two));
        b.turn_done("C");
        assert_eq!(b.next_turn("C"), None);

        for c in [
            Content::ClientLocal(ClientLocal::Notice("n".into())),
            Content::UserCommand(UserCommand::Stop),
            Content::SetPermissions(vec![]),
        ] {
            assert!(bypasses_queue(&c), "{c:?}");
            b.publish("C", msg(Participant::User, c), false, None);
        }
        assert_eq!(b.next_turn("C"), None, "never queued");
        assert!(!bypasses_queue(&Content::PlainText { text: "x".into(), context: None }));
        assert!(routes_to_service(&user("x")));
        assert!(!routes_to_service(&agent_md("x")));
        assert!(!routes_to_service(&msg(Participant::User, Content::ClientLocal(ClientLocal::PickingSession))));

        b.drain(s);
        b.typing("C", true, Participant::Agent);
        assert!(b.is_typing("C", Participant::Agent));
        assert!(!b.is_typing("C", Participant::User));
        assert_eq!(b.drain(s), vec![("C".to_string(), Update::Typing { on: true, who: Participant::Agent })]);
        b.typing("C", false, Participant::Agent);
        assert!(!b.is_typing("C", Participant::Agent));
        b.drain(s);

        let ids = vec!["X".to_string(), "Y".to_string()];
        assert_eq!(b.remove("C", ids.clone()), vec![Effect::Remove { chat: "C".into(), ids: ids.clone() }]);
        assert_eq!(b.drain(s), vec![("C".to_string(), Update::Removed { ids, chat: "C".into() })]);
        assert!(!b.knows("C"), "a removal forgets the newest id, so the next publish reads it again");
        assert_eq!(b.remove("C", vec![]), vec![], "nothing to remove");
    }
}
