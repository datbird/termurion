//! The AI chat list (`ChatListViewController`, `ChatCellView`, `ChatListModel`): the rows, the
//! selection, the context menu, the rename and delete rules and their texts, and the minted
//! chat titles (`ChatAgent.requestRenaming`). Pure; the window draws it and the App acts on it.

use crate::ai_catalog::Model;
use crate::ai_gate::{Alert, Then};
use crate::ai_turn::Out;
use crate::ai_models::Endpoint;
use crate::ai_msg::Message as AiMessage;
use crate::ai_msg::Role;
use crate::chat_agent::{history, user_text};
use crate::chat_gate::{endpoint_of, gate_open};
use crate::chat_message::{snippet, Content, Message, Participant, SNIPPET_MAX};
use crate::chat_store::Chat;
use crate::globals::Globals;

/// The row's snippet while the agent types (`CCV:88-93`).
pub const AI_TYPING: &str = "AI is typing\u{2026}";
/// The round icon's diameter in points (`CCV:16`).
pub const ICON_SIZE: f32 = 32.0;
/// The default icon: a speech bubble (Segoe Fluent Icons), white on gray at 45 percent.
pub const DEFAULT_ICON_GLYPH: char = '\u{E8BD}';
/// Before the title of an orchestration chat.
pub const ORCHESTRATOR_GLYPH: char = '\u{E99A}';
/// The header's new chat button (`CWC:739-741`).
pub const NEW_CHAT_GLYPH: char = '\u{E710}';

pub const RENAME_TITLE: &str = "Rename Chat";
pub const RENAME_PROMPT: &str = "Choose a new name for this chat.";
pub const RENAME_BUTTON: &str = "Rename";
pub const RENAME_FAILED_TEXT: &str = "The chat could not be renamed.";
pub const RENAME_FAILED_HEADING: &str = "Rename Failed";
pub const DELETE: &str = "Delete";

/// The naming request, verbatim (`ChatAgent.swift:1872`).
pub const TITLE_PROMPT: &str = "Please assign a short, specific name to this chat, less than 30 characters in length, but descriptive. It will be shown in a chat list UI. Respond with only the name of the chat.";

/// One row of the list.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub id: String,
    pub title: String,
    pub snippet: String,
    pub age: String,
    pub orchestrator: bool,
    pub icon: Option<Vec<u8>>,
}

/// The last message whose content has a snippet, newlines as spaces; "" when none
/// (`ChatListModel.swift:228-243`).
pub fn row_snippet(messages: &[Message]) -> String {
    messages.iter().rev().map(|m| snippet(&m.content, SNIPPET_MAX)).find(|s| !s.is_empty()).map(|s| s.replace(['\r', '\n'], " ")).unwrap_or_default()
}

/// The most recent chat linked to `session` (`lastChat(guid:)`); `chats` is most recent first.
pub fn last_linked<'a>(chats: &'a [Chat], session: &str) -> Option<&'a Chat> {
    chats.iter().find(|c| c.terminal_session_guid.as_deref() == Some(session))
}

/// The rows, in the order the store gives the chats. `snippet` is a chat's row snippet (its
/// stored messages), `typing` whether its agent types now, `now` seconds since 1970.
pub fn rows(chats: &[Chat], snippet: &dyn Fn(&str) -> String, typing: &dyn Fn(&str) -> bool, now: f64) -> Vec<Row> {
    chats
        .iter()
        .map(|c| Row {
            id: c.id.clone(),
            title: c.title.clone(),
            snippet: if typing(&c.id) { AI_TYPING.to_string() } else { snippet(&c.id) },
            age: crate::ai_terminal::compact_age(now - c.last_modified_date),
            orchestrator: c.orchestration_enabled,
            icon: c.icon.clone(),
        })
        .collect()
}

/// The rows a person has selected (`CL:480-506`), and the row a Shift+click extends from.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Selection {
    pub ids: Vec<String>,
    anchor: Option<usize>,
}

impl Selection {
    /// A click: alone selects the row; with Ctrl toggles it; with Shift selects the rows from
    /// the anchor to it (the anchor stays).
    pub fn click(&mut self, rows: &[Row], index: usize, ctrl: bool, shift: bool) {
        let Some(row) = rows.get(index) else { return };
        if shift {
            let from = self.anchor.filter(|a| *a < rows.len()).unwrap_or(index);
            let (lo, hi) = (from.min(index), from.max(index));
            self.ids = rows[lo..=hi].iter().map(|r| r.id.clone()).collect();
            self.anchor = Some(from);
        } else if ctrl {
            if let Some(at) = self.ids.iter().position(|i| *i == row.id) {
                self.ids.remove(at);
                // The anchor moves to the last row still selected.
                self.anchor = rows.iter().rposition(|r| self.ids.contains(&r.id));
            } else {
                self.ids.push(row.id.clone());
                self.anchor = Some(index);
            }
        } else {
            self.ids = vec![row.id.clone()];
            self.anchor = Some(index);
        }
    }

    /// Selects only the chat with this id (the window shows it from outside the list).
    pub fn set(&mut self, rows: &[Row], id: &str) {
        match rows.iter().position(|r| r.id == id) {
            Some(i) => {
                self.ids = vec![id.to_string()];
                self.anchor = Some(i);
            }
            None => *self = Selection::default(),
        }
    }

    /// A right click selects the row unless it is already selected (`CL:393-414`).
    pub fn right_click(&mut self, rows: &[Row], index: usize) {
        let Some(row) = rows.get(index) else { return };
        if !self.ids.contains(&row.id) {
            self.ids = vec![row.id.clone()];
            self.anchor = Some(index);
        }
    }

    /// Drops the ids no row has (a deleted chat).
    pub fn retain(&mut self, rows: &[Row]) {
        self.ids.retain(|i| rows.iter().any(|r| r.id == *i));
        self.anchor = self.anchor.filter(|a| *a < rows.len());
    }

    /// The chat the window shows for this selection (`CL:490-506`).
    pub fn first(&self) -> Option<&str> {
        self.ids.first().map(String::as_str)
    }
}

/// The right-click menu's entries for `count` selected chats; None is a separator. The
/// "1 Chats" is iTerm2's (`CL:393-414`).
pub fn context_menu(count: usize) -> Vec<Option<String>> {
    match count {
        0 => Vec::new(),
        1 => vec![Some(RENAME_TITLE.to_string()), None, Some(format!("Delete {count} Chats"))],
        n => vec![Some(format!("Delete {n} Chats"))],
    }
}

/// The new title a Rename answered with, None when it changes nothing (`CWC:774-817`).
pub fn rename_result(old: &str, typed: &str) -> Option<String> {
    let t = typed.trim();
    (!t.is_empty() && t != old).then(|| t.to_string())
}

/// What shows after a delete: Some(the new shown chat) when the shown chat went, None when it
/// stays (`CWC:757-778`).
pub fn after_delete(shown: Option<&str>, deleted: &[String], first_remaining: Option<&str>) -> Option<Option<String>> {
    let shown = shown?;
    deleted.iter().any(|d| d == shown).then(|| first_remaining.map(str::to_string))
}

pub fn delete_heading(n: usize) -> String {
    format!("Delete {n} Chats?")
}

pub fn delete_text(n: usize) -> String {
    format!("Are you sure you want to delete {n} chats? This action cannot be undone.")
}

/// Whether the chat's first turn just finished and it has no name yet (`ChatAgent.swift:1476-1483`):
/// exactly one message the model reads from the person, and an answer from the agent after it.
pub fn needs_title(log: &[Message]) -> bool {
    let users: Vec<usize> = log.iter().enumerate().filter(|(_, m)| m.author == Participant::User && user_text(&m.content).is_some()).map(|(i, _)| i).collect();
    let [first] = users[..] else { return false };
    log[first + 1..].iter().any(|m| m.author == Participant::Agent && matches!(m.content, Content::PlainText { .. } | Content::Markdown(_) | Content::Multipart { .. }))
}

/// The title a model's reply gives: trimmed, None when blank (`ChatAgent.swift:1895-1900`).
pub fn minted_title(reply: &str) -> Option<String> {
    let t = reply.trim();
    (!t.is_empty()).then(|| t.to_string())
}

/// The delete question (`CWC:844-872`): Cancel, then the destructive Delete, persistent. Its
/// own gate flag is off and a close answers Cancel (ruling F2), so closing never deletes.
pub fn delete_alert(ids: Vec<String>) -> Alert {
    let n = ids.len();
    Alert { heading: delete_heading(n), text: delete_text(n), buttons: vec!["Cancel", DELETE], silence: None, then: Then::DeleteChats(ids), gate: false, close: Some("Cancel") }
}

/// The alert a failed rename shows (OK only).
pub fn rename_failed_alert() -> Alert {
    Alert { heading: RENAME_FAILED_HEADING.to_string(), text: RENAME_FAILED_TEXT.to_string(), buttons: vec!["OK"], silence: None, then: Then::Nothing, gate: false, close: None }
}

/// What a title run's output says: None while it is still going; Some(None) when it ended
/// with no usable title (a failure, a cancel, a missing key, a blank reply: the chat keeps its
/// name, `ChatAgent.swift:1886-1906`); Some(Some(title)) when it gave one.
pub fn title_outcome(outs: &[Out]) -> Option<Option<String>> {
    for o in outs {
        match o {
            Out::Choice(reply) => return Some(minted_title(reply)),
            Out::Failed(_) | Out::Canceled | Out::NeedKey(_) => return Some(None),
            _ => {}
        }
    }
    None
}

/// Drops the runs of these chats, handing each to `cancel` first (ruling F16: a deleted chat's
/// run must not publish a title to a chat with no row).
pub fn cancel_runs<T>(runs: &mut std::collections::HashMap<String, T>, chats: &[String], cancel: &mut dyn FnMut(T)) {
    for c in chats {
        if let Some(run) = runs.remove(c) {
            cancel(run);
        }
    }
}

/// The naming request (ruling R12cd-1): None when the chat needs no title or the AI gate is
/// closed for the model's own endpoint, so nothing may be sent; else that endpoint and the
/// messages, the history with every response id removed and the naming prompt last.
pub fn title_request(g: &Globals, model: &Model, log: &[Message]) -> Option<(Endpoint, Vec<AiMessage>)> {
    if !needs_title(log) {
        return None;
    }
    let endpoint = endpoint_of(model);
    if !gate_open(g, &endpoint) {
        return None;
    }
    let mut messages = history(log);
    for m in &mut messages {
        m.response_id = None;
    }
    messages.push(AiMessage::text(Role::User, TITLE_PROMPT));
    Some((endpoint, messages))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai_settings::{Api, ADMIN, ENABLE_AI};
    use crate::chat_message::ClientLocal;
    use std::path::Path;
    use toml::Value;

    fn msg(author: Participant, c: Content) -> Message {
        Message::new("chat", author, c, 1.0)
    }

    fn text(author: Participant, t: &str) -> Message {
        msg(author, Content::PlainText { text: t.into(), context: None })
    }

    fn markdown(t: &str) -> Message {
        msg(Participant::Agent, Content::Markdown(t.into()))
    }

    fn chat(id: &str, session: Option<&str>) -> Chat {
        let mut c = Chat::new(id, 100.0);
        c.id = id.into();
        c.terminal_session_guid = session.map(str::to_string);
        c
    }

    fn row(id: &str) -> Row {
        Row { id: id.into(), title: id.into(), snippet: String::new(), age: String::new(), orchestrator: false, icon: None }
    }

    #[test]
    fn list_rows_follow_iterm2() {
        let mut a = chat("a", None);
        a.last_modified_date = 1000.0;
        let mut b = chat("b", None);
        b.last_modified_date = 400.0;
        b.orchestration_enabled = true;
        b.icon = Some(vec![1, 2]);
        let logs = |id: &str| -> Vec<Message> {
            match id {
                "a" => vec![text(Participant::User, "hello\nthere"), msg(Participant::Agent, Content::SetPermissions(vec![]))],
                _ => vec![text(Participant::User, &"x".repeat(50))],
            }
        };
        let snip = |id: &str| row_snippet(&logs(id));
        let typing = |id: &str| id == "b";
        let r = rows(&[a.clone(), b.clone()], &snip, &typing, 1060.0);
        assert_eq!(r[0].snippet, "hello there", "setPermissions has no snippet: the last message with one wins");
        assert_eq!(r[0].age, crate::ai_terminal::compact_age(60.0));
        assert!(!r[0].orchestrator && r[0].icon.is_none());
        assert_eq!(r[1].snippet, AI_TYPING);
        assert!(r[1].orchestrator);
        assert_eq!(r[1].icon, Some(vec![1, 2]));
        assert_eq!(r[1].age, crate::ai_terminal::compact_age(660.0));
        assert_eq!((r[0].id.as_str(), r[1].id.as_str()), ("a", "b"));
        let long = row_snippet(&[text(Participant::User, &"y".repeat(50))]);
        assert_eq!(long, format!("{}\u{2026}", "y".repeat(39)));
        assert_eq!(row_snippet(&[msg(Participant::Agent, Content::RenameChat("t".into()))]), "");
        assert_eq!(row_snippet(&[]), "");

        let chats = [chat("c1", Some("7")), chat("c2", Some("7")), chat("c3", Some("8"))];
        assert_eq!(last_linked(&chats, "7").map(|c| c.id.as_str()), Some("c1"));
        assert_eq!(last_linked(&chats, "9").map(|c| c.id.as_str()), None);
    }

    #[test]
    fn selection_follows_iterm2() {
        let rows = [row("a"), row("b"), row("c")];
        let mut s = Selection::default();
        s.click(&rows, 1, false, false);
        assert_eq!(s.ids, ["b"]);
        s.click(&rows, 2, true, false);
        assert_eq!(s.ids, ["b", "c"]);
        s.click(&rows, 1, true, false);
        assert_eq!(s.ids, ["c"]);
        assert_eq!(s.first(), Some("c"));
        s.click(&rows, 0, false, true);
        assert_eq!(s.ids, ["a", "b", "c"], "Shift+click from anchor 2 selects rows 0 to 2");
        s.click(&rows, 0, false, false);
        s.right_click(&rows, 2);
        assert_eq!(s.ids, ["c"], "an unselected row is selected alone");
        s.click(&rows, 0, true, false);
        s.right_click(&rows, 2);
        assert_eq!(s.ids, ["c", "a"], "a selected row keeps the selection");
        s.retain(&rows[..2]);
        assert_eq!(s.ids, ["a"]);
        s.click(&rows, 9, false, false);
        assert_eq!(s.ids, ["a"], "a click past the last row does nothing");
        s.set(&rows, "c");
        assert_eq!(s.ids, ["c"]);
        s.click(&rows, 0, false, true);
        assert_eq!(s.ids, ["a", "b", "c"], "the shown chat is the anchor");
        s.set(&rows, "gone");
        assert!(s.ids.is_empty());
    }

    #[test]
    fn list_menu_follows_iterm2() {
        assert_eq!(context_menu(1), [Some("Rename Chat".to_string()), None, Some("Delete 1 Chats".to_string())]);
        assert_eq!(context_menu(3), [Some("Delete 3 Chats".to_string())]);
    }

    #[test]
    fn rename_and_delete_follow_iterm2() {
        assert_eq!(rename_result("Old", "  New  "), Some("New".to_string()));
        assert_eq!(rename_result("Old", "Old"), None);
        assert_eq!(rename_result("Old", " \n"), None);
        assert_eq!(
            (RENAME_TITLE, RENAME_PROMPT, RENAME_BUTTON, RENAME_FAILED_TEXT, RENAME_FAILED_HEADING, DELETE),
            ("Rename Chat", "Choose a new name for this chat.", "Rename", "The chat could not be renamed.", "Rename Failed", "Delete")
        );
        assert_eq!(delete_heading(2), "Delete 2 Chats?");
        assert_eq!(delete_text(2), "Are you sure you want to delete 2 chats? This action cannot be undone.");
        let a = ["a".to_string()];
        assert_eq!(after_delete(Some("a"), &a, Some("b")), Some(Some("b".to_string())));
        assert_eq!(after_delete(Some("a"), &a, None), Some(None));
        assert_eq!(after_delete(Some("a"), &["x".to_string()], Some("b")), None);
        assert_eq!(after_delete(None, &a, Some("b")), None);
    }

    #[test]
    fn a_close_never_deletes_and_alerts_are_not_gate_alerts() {
        let a = delete_alert(vec!["a".into(), "b".into()]);
        assert_eq!((a.heading.as_str(), a.buttons.clone()), ("Delete 2 Chats?", vec!["Cancel", "Delete"]));
        assert_eq!(a.then, Then::DeleteChats(vec!["a".into(), "b".into()]));
        assert!(!a.gate && a.silence.is_none());
        // Closing, Escape and Enter all press Cancel, never Delete (ruling F2).
        assert_eq!(crate::prompt_ui::close_index_with(&a.buttons, a.close), 0);
        assert_eq!(crate::prompt_ui::escape_index(&a.buttons, a.close), Some(0));
        assert_eq!(crate::prompt_ui::action_keys(&a.buttons).0, 0);
        let r = rename_failed_alert();
        assert_eq!((r.heading.as_str(), r.text.as_str(), r.buttons.clone(), r.then.clone(), r.gate), ("Rename Failed", "The chat could not be renamed.", vec!["OK"], Then::Nothing, false));
        assert!(!crate::ai_gate::is_gate(&a) && !crate::ai_gate::is_gate(&r));
    }

    #[test]
    fn title_runs_end_and_cancel_as_expected() {
        use crate::ai_settings::Vendor;
        assert_eq!(title_outcome(&[]), None);
        assert_eq!(title_outcome(&[Out::WillSend, Out::Delta("x".into())]), None);
        assert_eq!(title_outcome(&[Out::Choice(" Rust help ".into())]), Some(Some("Rust help".to_string())));
        assert_eq!(title_outcome(&[Out::Choice("  ".into())]), Some(None));
        assert_eq!(title_outcome(&[Out::Failed("boom".into())]), Some(None));
        assert_eq!(title_outcome(&[Out::Canceled]), Some(None));
        assert_eq!(title_outcome(&[Out::NeedKey(Vendor::OpenAI)]), Some(None), "a missing key cancels silently");
        let mut runs = std::collections::HashMap::from([("a".to_string(), 1), ("b".to_string(), 2)]);
        let mut cancelled = Vec::new();
        cancel_runs(&mut runs, &["a".to_string(), "gone".to_string()], &mut |r| cancelled.push(r));
        assert_eq!((cancelled, runs.len(), runs.contains_key("b")), (vec![1], 1, true), "a deleted chat's run is cancelled and dropped");
    }

    #[test]
    fn titles_follow_iterm2() {
        assert_eq!(
            TITLE_PROMPT,
            "Please assign a short, specific name to this chat, less than 30 characters in length, but descriptive. It will be shown in a chat list UI. Respond with only the name of the chat."
        );
        let perms = msg(Participant::Agent, Content::SetPermissions(vec![]));
        let (q, a) = (text(Participant::User, "q"), markdown("a"));
        assert!(needs_title(&[perms.clone(), q.clone(), a.clone()]));
        assert!(!needs_title(&[q.clone()]), "no reply yet");
        assert!(!needs_title(&[q.clone(), a.clone(), text(Participant::User, "q2"), markdown("b")]), "not the first turn");
        let notice = msg(Participant::User, Content::ClientLocal(ClientLocal::Notice("n".into())));
        assert!(needs_title(&[notice, q, a]), "a notice is not a message the model reads");
        assert_eq!(minted_title("  Rust help \n"), Some("Rust help".to_string()));
        assert_eq!(minted_title("  "), None);
    }

    #[test]
    fn minted_title_needs_the_gate() {
        let m = crate::ai_model::fixture("t", "https://titles.test/v1/chat/completions", Api::ChatCompletions);
        let mut answered = markdown("a");
        answered.response_id = Some("resp_1".into());
        let log = [text(Participant::User, "q"), answered];
        let mut g = Globals::empty(Path::new("chat-list-test.toml"));
        g.set(ENABLE_AI, Value::Boolean(false));
        assert_eq!(title_request(&g, &m, &log), None, "Enable generative AI features is off");
        g.set(ENABLE_AI, Value::Boolean(true));
        crate::ai_gate::grant(&mut g, "api.openai.com");
        assert_eq!(title_request(&g, &m, &log), None, "the title model's own host decides");
        crate::ai_gate::grant(&mut g, "titles.test");
        let (endpoint, messages) = title_request(&g, &m, &log).expect("the gate is open");
        assert_eq!(endpoint, endpoint_of(&m));
        let last = messages.last().unwrap();
        assert_eq!((last.role, last.body.clone()), (Some(Role::User), crate::ai_msg::Body::Text(TITLE_PROMPT.to_string())));
        assert!(messages.iter().all(|m| m.response_id.is_none()), "no response id reaches the request");
        assert_eq!(messages.len(), 3);
        assert_eq!(title_request(&g, &m, &log[..1]), None, "needs_title is false with the gate open");
        g.set(ADMIN, Value::Boolean(false));
        assert_eq!(title_request(&g, &m, &log), None, "the admin switch wins");
    }
}
