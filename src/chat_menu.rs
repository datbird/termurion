//! The Chat Info menu and the permission cycle (`ChatViewController.showSessionButtonMenu`,
//! `toggle(permissionCategory:)`, `RemoteCommandExecutor.controlState`). Pure: the entries the
//! menu shows for the chat's state, the tri-state of a permission, the cycle and the two
//! confirmation alerts. The menu itself is built by `app::chat_session_app`.

use crate::ai_context::Consent;
use crate::ai_gate::{Alert, Then};
use crate::ai_permissions::{Category, Permission};

pub const ENABLE_ORCHESTRATION: &str = "Enable Orchestration";
pub const DISABLE_ORCHESTRATION: &str = "Disable Orchestration";
pub const REVEAL_TERMINAL: &str = "Reveal Linked Terminal Session";
pub const UNLINK_TERMINAL: &str = "Unlink Terminal Session";
pub const PUT_TERMINAL: &str = "Put Chat in Linked Terminal Session";
pub const SEND_AUTOMATICALLY: &str = "Send Commands & Output to AI Automatically";
pub const LINK_TERMINAL: &str = "Link Terminal Session";
pub const LINK_BROWSER: &str = "Link Browser Session";
pub const HELP: &str = "Help";
pub const HELP_TEXT: &str = "When a terminal session is linked to this chat, the AI may view terminal contents and run commands in that session. You will be prompted to grant permission before it is able to view, type to, or modify a terminal session.";
pub const CONFIRM_HEADING: &str = "Confirm Change";
pub const CONFIRM_BUTTONS: [&str; 3] = ["Send Automatically", "Ask Each Time", "Never Allow"];
pub const PRIVACY_HEADING: &str = "Privacy Warning";
pub const PRIVACY_TEXT: &str = "All terminal content will be sent to AI, which may go to a third party. Ensure this is safe to do before proceeding.";
pub const UNLINKED_TERMINAL: &str = "This chat is no longer linked to a terminal session.";
pub const UNLINKED_BROWSER: &str = "This chat is no longer linked to a web browser session.";

/// A menu item's check state: On checked, Off unchecked, Mixed a dash (`NSControl.StateValue`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tri {
    On,
    Off,
    Mixed,
}

/// `controlState` (`RCE:184-195`): Always is on, Never off, Ask mixed.
pub fn tri(p: Permission) -> Tri {
    match p {
        Permission::Always => Tri::On,
        Permission::Never => Tri::Off,
        Permission::Ask => Tri::Mixed,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Orchestration,
    Reveal,
    Unlink,
    Put,
    Toggle(Category),
    Stream,
    Link,
    LinkBrowser,
    Help,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Entry {
    Item { title: String, action: Action, state: Option<Tri>, enabled: bool },
    Separator,
}

/// What the menu's entries depend on.
#[derive(Clone, Debug, Default)]
pub struct MenuFacts {
    pub orchestrating: bool,
    /// The chat's linked terminal is a live pane.
    pub terminal_live: bool,
    /// Always false before milestone 13 (no browser pane exists); kept for the 13x plan.
    pub browser_live: bool,
    /// The menu is the inline panel's: it is already in its session, so no Put item.
    pub inline: bool,
    pub streaming: bool,
}

fn item(title: &str, action: Action, state: Option<Tri>, enabled: bool) -> Entry {
    Entry::Item { title: title.to_string(), action, state, enabled }
}

/// A permission item's title: the autopopulated categories read "Provide ... Automatically"
/// at Always (`CVC:2896-2909`), else the regular title.
pub fn item_title(c: Category, p: Permission) -> String {
    match (p, c.autopopulation_title()) {
        (Permission::Always, Some(t)) => t.to_string(),
        _ => c.title().to_string(),
    }
}

/// The menu for the chat's state (`CVC:3369-3483`). Orchestration and the browser items are
/// shown disabled until 12f and milestone 13.
pub fn session_menu(f: &MenuFacts, perm: &dyn Fn(Category) -> Permission) -> Vec<Entry> {
    if f.orchestrating {
        return vec![item(DISABLE_ORCHESTRATION, Action::Orchestration, None, false), Entry::Separator, item(HELP, Action::Help, None, true)];
    }
    let mut out = vec![item(ENABLE_ORCHESTRATION, Action::Orchestration, None, false), Entry::Separator];
    if f.terminal_live {
        out.push(item(REVEAL_TERMINAL, Action::Reveal, None, true));
        out.push(item(UNLINK_TERMINAL, Action::Unlink, None, true));
        if !f.inline {
            out.push(item(PUT_TERMINAL, Action::Put, None, true));
        }
        out.push(Entry::Separator);
        for c in Category::ALL.into_iter().filter(|c| !c.is_browser()) {
            let p = perm(c);
            out.push(item(&item_title(c, p), Action::Toggle(c), Some(tri(p)), true));
        }
        out.push(Entry::Separator);
        out.push(item(SEND_AUTOMATICALLY, Action::Stream, Some(if f.streaming { Tri::On } else { Tri::Off }), true));
        out.push(Entry::Separator);
    } else {
        out.push(item(LINK_TERMINAL, Action::Link, None, true));
        out.push(Entry::Separator);
    }
    out.push(item(LINK_BROWSER, Action::LinkBrowser, None, false));
    out.push(Entry::Separator);
    out.push(item(HELP, Action::Help, None, true));
    out
}

/// What a Confirm Change button answers: the permission and whether consent to automatic
/// sending is granted. Only the first button grants; anything unknown asks each time.
pub fn confirm_answer(index: usize) -> (Permission, bool) {
    match index {
        0 => (Permission::Always, true),
        2 => (Permission::Never, false),
        _ => (Permission::Ask, false),
    }
}

/// What one press of a permission item does (`CVC:1150-1204`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Toggle {
    /// Set the permission now.
    Set(Permission),
    /// Ask first: the category's warning is the alert's text.
    Confirm(&'static str),
}

pub fn toggle(c: Category, current: Permission) -> Toggle {
    let next = current.next();
    match (next, c.autopopulation_warning()) {
        (Permission::Always, Some(w)) => Toggle::Confirm(w),
        _ => Toggle::Set(next),
    }
}

/// The Confirm Change alert. Its buttons have no "Cancel", so it names its refusing button:
/// a close and Escape answer "Ask Each Time", never Send Automatically (ruling F2).
pub fn confirm_alert(chat: &str, guid: &str, category: Category, warning: &str) -> Alert {
    Alert {
        heading: CONFIRM_HEADING.to_string(),
        text: warning.to_string(),
        buttons: CONFIRM_BUTTONS.to_vec(),
        silence: None,
        then: Then::ConfirmChange { chat: chat.to_string(), guid: guid.to_string(), category },
        gate: false,
        close: Some(CONFIRM_BUTTONS[1]),
    }
}

/// The Privacy Warning before streaming starts; its Cancel is what a close answers.
pub fn privacy_alert(chat: &str, guid: &str) -> Alert {
    Alert { heading: PRIVACY_HEADING.to_string(), text: PRIVACY_TEXT.to_string(), buttons: vec!["OK", "Cancel"], silence: None, then: Then::PrivacyWarning { chat: chat.to_string(), guid: guid.to_string() }, gate: false, close: None }
}

/// Whether the Privacy Warning's OK may start streaming: the chat must still be linked to the
/// session the question named, and it must be open. A chat unlinked and relinked elsewhere
/// since the question was asked starts nothing.
pub fn streaming_allowed(linked: Option<&str>, guid: &str, live: bool) -> bool {
    live && linked == Some(guid)
}

/// What the Privacy Warning's OK does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StartStreaming {
    Start,
    /// The chat was relinked, unlinked or its session closed since the question: nothing starts,
    /// and the chat says so.
    Stale,
    /// Already streaming: nothing to do, nothing to say.
    Already,
}

pub const STREAM_STALE_NOTICE: &str = "Sending commands automatically was not started: this chat is no longer linked to the session the question was about.";

pub fn start_streaming_decision(linked: Option<&str>, guid: &str, live: bool, already: bool) -> StartStreaming {
    if !streaming_allowed(linked, guid, live) {
        StartStreaming::Stale
    } else if already {
        StartStreaming::Already
    } else {
        StartStreaming::Start
    }
}

/// The chats linked to the session `reference` (from `linked_sessions`' (chat, guid) pairs).
pub fn chats_linked_to(linked: &[(String, String)], reference: &str) -> Vec<String> {
    linked.iter().filter(|(_, guid)| guid == reference).map(|(chat, _)| chat.clone()).collect()
}

/// Help (the warning bubble on the button in iTerm2): OK only, nothing follows.
pub fn help_alert() -> Alert {
    Alert { heading: String::new(), text: HELP_TEXT.to_string(), buttons: vec!["OK"], silence: None, then: Then::Nothing, gate: false, close: None }
}

/// What a Confirm Change answer may do now: the alert named a chat, a session and a category,
/// and applies only while the chat is still linked to that live session (a stale answer
/// changes nothing and grants nothing).
pub fn confirm_outcome(index: usize, linked: Option<&str>, guid: &str, live: bool) -> Option<(Permission, bool)> {
    if !live || linked != Some(guid) {
        return None;
    }
    Some(confirm_answer(index))
}

/// The consent an answer writes: Granted only when the answer grants (ruling F31,
/// `ChatViewController.swift:1187`); the guard against writing over an existing Granted is
/// `queued_consent_write`, for the one-time question.
pub fn consent_write(grants: bool) -> Option<Consent> {
    grants.then_some(Consent::Granted)
}

/// Drops the queued one-time "Share Terminal Contents Automatically?" questions: Confirm
/// Change's Send Automatically answered the same question (ruling F31).
pub fn drop_queued_consent(asks: &mut std::collections::VecDeque<Alert>) {
    asks.retain(|a| a.then != Then::AutoProvide);
}

/// The one-time question's write once answered: a question that was overtaken by a grant writes
/// nothing (Not Now would otherwise turn Granted into Denied, ruling F31).
pub fn queued_consent_write(current: Consent, index: usize) -> Option<Consent> {
    if current == Consent::Granted {
        return None;
    }
    Some(crate::ai_context::consent_answer(index))
}

pub const MENU_PREFIX: &str = "aichat:menu:";

/// A menu item's id: `aichat:menu:<verb>:<chat>`.
pub fn menu_id(a: &Action, chat: &str) -> String {
    let verb = match a {
        Action::Orchestration => "orchestration".to_string(),
        Action::Reveal => "reveal".to_string(),
        Action::Unlink => "unlink".to_string(),
        Action::Put => "put".to_string(),
        Action::Toggle(c) => format!("toggle-{}", Category::ALL.iter().position(|x| x == c).unwrap_or(0)),
        Action::Stream => "stream".to_string(),
        Action::Link => "link".to_string(),
        Action::LinkBrowser => "link-browser".to_string(),
        Action::Help => "help".to_string(),
    };
    format!("{MENU_PREFIX}{verb}:{chat}")
}

/// The action and chat of a pick. The disabled items (orchestration, the browser link) and a
/// browser category are never a pick.
pub fn parse_menu_id(id: &str) -> Option<(Action, String)> {
    let (verb, chat) = id.strip_prefix(MENU_PREFIX)?.split_once(':')?;
    if chat.is_empty() {
        return None;
    }
    let action = match verb {
        "reveal" => Action::Reveal,
        "unlink" => Action::Unlink,
        "put" => Action::Put,
        "stream" => Action::Stream,
        "link" => Action::Link,
        "help" => Action::Help,
        v => {
            let c = *Category::ALL.get(v.strip_prefix("toggle-")?.parse::<usize>().ok()?)?;
            if c.is_browser() {
                return None;
            }
            Action::Toggle(c)
        }
    };
    Some((action, chat.to_string()))
}

/// The last streaming state a chat's messages hold (`lastStreamingState`).
pub fn last_streaming(msgs: &[crate::chat_message::Message]) -> Option<crate::chat_message::Streaming> {
    use crate::chat_message::{ClientLocal, Content};
    msgs.iter().rev().find_map(|m| match &m.content {
        Content::ClientLocal(ClientLocal::StreamingChanged(s)) => Some(*s),
        _ => None,
    })
}

/// The most characters a selection may hold and still be offered to the chat: at least 80, or
/// the session's width (`_haveShortSelection`, `PTYTextView.m:3893-3896`).
pub fn short_selection(chars: usize, columns: usize) -> bool {
    chars <= columns.max(80)
}

/// The text a finished command reports when its output scrolled away (ruling F32).
pub const OUTPUT_GONE: &str = "The output was no longer available.";

/// The streaming chats linked to the session named `reference`: `chats` is each streaming
/// chat with the guid its row links.
pub fn stream_targets<'a>(chats: impl IntoIterator<Item = (&'a str, Option<&'a str>)>, reference: &str) -> Vec<String> {
    chats.into_iter().filter(|(_, g)| *g == Some(reference)).map(|(c, _)| c.to_string()).collect()
}

/// Whether a chat opened now was streaming when the last run ended: its last streaming state
/// is Active but nothing streams it (`CVC:1006-1011`).
pub fn stale_streaming(last_state: Option<crate::chat_message::Streaming>, streaming_now: bool) -> bool {
    last_state == Some(crate::chat_message::Streaming::Active) && !streaming_now
}

/// The panes that must queue their finished commands: the panes the streaming chats link to.
/// `links` is each chat with its live linked pane (None when unlinked or the pane is gone).
pub fn watched_panes<'a>(links: impl IntoIterator<Item = (&'a str, Option<crate::layout::PaneId>)>, streaming: &std::collections::HashSet<String>) -> std::collections::HashSet<crate::layout::PaneId> {
    links.into_iter().filter(|(chat, _)| streaming.contains(*chat)).filter_map(|(_, pane)| pane).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn titles(e: &[Entry]) -> Vec<String> {
        e.iter()
            .map(|e| match e {
                Entry::Item { title, .. } => title.clone(),
                Entry::Separator => "-".to_string(),
            })
            .collect()
    }

    fn enabled(e: &[Entry], t: &str) -> bool {
        e.iter().any(|e| matches!(e, Entry::Item { title, enabled: true, .. } if title == t))
    }

    fn state(e: &[Entry], t: &str) -> Option<Tri> {
        e.iter().find_map(|e| match e {
            Entry::Item { title, state, .. } if title == t => *state,
            _ => None,
        })
    }

    fn run_always(c: Category) -> Permission {
        if c == Category::RunCommands { Permission::Always } else { Permission::Ask }
    }

    #[test]
    fn session_menu_follows_iterm2() {
        let live = MenuFacts { terminal_live: true, ..MenuFacts::default() };
        let m = session_menu(&live, &run_always);
        assert_eq!(
            titles(&m),
            vec![
                "Enable Orchestration", "-", "Reveal Linked Terminal Session", "Unlink Terminal Session", "Put Chat in Linked Terminal Session", "-",
                "AI can Check Terminal State", "AI can Run Commands", "AI can View Contents", "AI can Write to the Clipboard", "AI can Control Terminal", "AI can View Manpages", "AI can Write to the File System", "-",
                "Send Commands & Output to AI Automatically", "-", "Link Browser Session", "-", "Help",
            ]
        );
        assert!(!enabled(&m, "Enable Orchestration"), "orchestration waits for 12f");
        assert!(!enabled(&m, "Link Browser Session"), "no browser pane before milestone 13");
        assert!(enabled(&m, "Help") && enabled(&m, "Unlink Terminal Session"));
        assert_eq!(state(&m, "AI can Check Terminal State"), Some(Tri::Mixed));
        assert_eq!(state(&m, "AI can Run Commands"), Some(Tri::On));
        assert_eq!(state(&m, "AI can View Manpages"), Some(Tri::Mixed));
        assert_eq!(state(&m, "Send Commands & Output to AI Automatically"), Some(Tri::Off));
        // The inline panel is already in its session: no Put item.
        let inline = session_menu(&MenuFacts { inline: true, ..live.clone() }, &run_always);
        assert!(!titles(&inline).contains(&"Put Chat in Linked Terminal Session".to_string()));
        assert_eq!(titles(&inline).len(), titles(&m).len() - 1);
        // Not linked.
        let none = session_menu(&MenuFacts::default(), &run_always);
        assert_eq!(titles(&none), vec!["Enable Orchestration", "-", "Link Terminal Session", "-", "Link Browser Session", "-", "Help"]);
        assert!(enabled(&none, "Link Terminal Session"));
        // Orchestrating.
        let orch = session_menu(&MenuFacts { orchestrating: true, terminal_live: true, ..MenuFacts::default() }, &run_always);
        assert_eq!(titles(&orch), vec!["Disable Orchestration", "-", "Help"]);
        assert!(!enabled(&orch, "Disable Orchestration"));
        // Streaming shows the send item On.
        let streaming = session_menu(&MenuFacts { streaming: true, ..live }, &run_always);
        assert_eq!(state(&streaming, "Send Commands & Output to AI Automatically"), Some(Tri::On));
        // The autopopulated categories retitle at Always.
        let all = session_menu(&MenuFacts { terminal_live: true, ..MenuFacts::default() }, &|_| Permission::Always);
        assert!(titles(&all).contains(&"Provide Terminal State Automatically".to_string()));
        assert!(titles(&all).contains(&"Provide Screen Contents Automatically".to_string()));
        assert_eq!(item_title(Category::CheckTerminalState, Permission::Always), "Provide Terminal State Automatically");
        assert_eq!(item_title(Category::ViewContents, Permission::Always), "Provide Screen Contents Automatically");
        assert_eq!(item_title(Category::RunCommands, Permission::Always), "AI can Run Commands");
        assert_eq!(item_title(Category::ViewContents, Permission::Never), "AI can View Contents");
    }

    #[test]
    fn permission_cycle_follows_iterm2() {
        assert_eq!(tri(Permission::Always), Tri::On);
        assert_eq!(tri(Permission::Never), Tri::Off);
        assert_eq!(tri(Permission::Ask), Tri::Mixed);
        assert_eq!(confirm_answer(0), (Permission::Always, true));
        assert_eq!(confirm_answer(1), (Permission::Ask, false));
        assert_eq!(confirm_answer(2), (Permission::Never, false));
        assert_eq!(CONFIRM_HEADING, "Confirm Change");
        assert_eq!(CONFIRM_BUTTONS, ["Send Automatically", "Ask Each Time", "Never Allow"]);
        assert_eq!(PRIVACY_HEADING, "Privacy Warning");
        assert_eq!(PRIVACY_TEXT, "All terminal content will be sent to AI, which may go to a third party. Ensure this is safe to do before proceeding.");
        assert_eq!(HELP_TEXT, "When a terminal session is linked to this chat, the AI may view terminal contents and run commands in that session. You will be prompted to grant permission before it is able to view, type to, or modify a terminal session.");
        // Always to Never to Ask to Always; the two autopopulated categories confirm the move to Always.
        assert_eq!(toggle(Category::RunCommands, Permission::Always), Toggle::Set(Permission::Never));
        assert_eq!(toggle(Category::RunCommands, Permission::Never), Toggle::Set(Permission::Ask));
        assert_eq!(toggle(Category::RunCommands, Permission::Ask), Toggle::Set(Permission::Always));
        assert_eq!(toggle(Category::CheckTerminalState, Permission::Ask), Toggle::Confirm(Category::CheckTerminalState.autopopulation_warning().unwrap()));
        assert_eq!(toggle(Category::ViewContents, Permission::Ask), Toggle::Confirm(Category::ViewContents.autopopulation_warning().unwrap()));
        assert_eq!(toggle(Category::ViewContents, Permission::Always), Toggle::Set(Permission::Never));
    }

    #[test]
    fn linking_texts_follow_iterm2() {
        assert_eq!(UNLINKED_TERMINAL, "This chat is no longer linked to a terminal session.");
        assert_eq!(UNLINKED_BROWSER, "This chat is no longer linked to a web browser session.");
        assert_eq!(SEND_AUTOMATICALLY, "Send Commands & Output to AI Automatically");
        assert_eq!(REVEAL_TERMINAL, "Reveal Linked Terminal Session");
        assert_eq!(LINK_TERMINAL, "Link Terminal Session");
        assert_eq!(LINK_BROWSER, "Link Browser Session");
    }

    #[test]
    fn closing_an_alert_never_grants_or_starts_anything() {
        // The close answer is the button the window's close and Escape press (ruling F2).
        let confirm = confirm_alert("c", "g", Category::ViewContents, "w");
        assert_eq!(confirm.buttons, vec!["Send Automatically", "Ask Each Time", "Never Allow"]);
        assert!(!confirm.gate);
        let closed = crate::prompt_ui::close_index_with(&confirm.buttons, confirm.close);
        assert_eq!(confirm_answer(closed), (Permission::Ask, false), "a close asks each time and grants no consent");
        let escaped = crate::prompt_ui::escape_index(&confirm.buttons, confirm.close);
        assert_eq!(escaped, Some(1));
        let privacy = privacy_alert("c", "g");
        assert!(!privacy.gate);
        let closed = crate::prompt_ui::close_index_with(&privacy.buttons, privacy.close);
        assert_eq!(privacy.buttons[closed], "Cancel", "a close never starts streaming");
        let help = help_alert();
        assert_eq!((help.buttons.clone(), help.then.clone(), help.gate), (vec!["OK"], Then::Nothing, false));
    }

    #[test]
    fn a_stale_confirm_answer_changes_nothing() {
        assert_eq!(confirm_outcome(0, Some("g"), "g", true), Some((Permission::Always, true)));
        assert_eq!(confirm_outcome(0, Some("other"), "g", true), None, "relinked meanwhile");
        assert_eq!(confirm_outcome(0, None, "g", true), None, "unlinked meanwhile");
        assert_eq!(confirm_outcome(0, Some("g"), "g", false), None, "the session closed");
        assert_eq!(consent_write(true), Some(Consent::Granted));
        assert_eq!(consent_write(false), None);
    }

    #[test]
    fn a_grant_clears_the_queued_question_and_the_question_never_overwrites_it() {
        let mk = |then| Alert { heading: String::new(), text: String::new(), buttons: vec!["OK"], silence: None, then, gate: false, close: None };
        let mut asks: std::collections::VecDeque<Alert> = vec![mk(Then::AutoProvide), mk(Then::Nothing), mk(Then::AutoProvide)].into();
        drop_queued_consent(&mut asks);
        assert_eq!(asks.len(), 1);
        assert_eq!(asks[0].then, Then::Nothing);
        assert_eq!(queued_consent_write(Consent::Granted, 1), None, "Not Now over Granted writes nothing");
        assert_eq!(queued_consent_write(Consent::Granted, 0), None);
        assert_eq!(queued_consent_write(Consent::Unknown, 0), Some(Consent::Granted));
        assert_eq!(queued_consent_write(Consent::Unknown, 1), Some(Consent::Denied));
    }

    #[test]
    fn menu_ids_round_trip_and_refuse_the_rest() {
        for a in [Action::Reveal, Action::Unlink, Action::Put, Action::Stream, Action::Link, Action::Help, Action::Toggle(Category::RunCommands), Action::Toggle(Category::ViewContents)] {
            let id = menu_id(&a, "chat-1");
            assert!(id.starts_with("aichat:menu:"));
            assert_eq!(parse_menu_id(&id), Some((a, "chat-1".to_string())));
        }
        assert_eq!(menu_id(&Action::Unlink, "c"), "aichat:menu:unlink:c");
        assert_eq!(menu_id(&Action::Toggle(Category::RunCommands), "c"), "aichat:menu:toggle-1:c");
        // The disabled items are never a pick, nor is a browser toggle, an unknown verb or no chat.
        assert_eq!(parse_menu_id("aichat:menu:orchestration:c"), None);
        assert_eq!(parse_menu_id("aichat:menu:link-browser:c"), None);
        assert_eq!(parse_menu_id("aichat:menu:toggle-7:c"), None, "Act in Web Browser has no item");
        assert_eq!(parse_menu_id("aichat:menu:toggle-99:c"), None);
        assert_eq!(parse_menu_id("aichat:menu:unlink:"), None);
        assert_eq!(parse_menu_id("aichat:menu:nope:c"), None);
        assert_eq!(parse_menu_id("aichat:msg:edit:c"), None);
    }

    #[test]
    fn the_last_streaming_state_is_read_from_the_messages() {
        use crate::chat_message::{ClientLocal, Content, Message, Participant, Streaming};
        let m = |c| Message::new("c", Participant::Agent, c, 1.0);
        let s = |x| m(Content::ClientLocal(ClientLocal::StreamingChanged(x)));
        assert_eq!(last_streaming(&[]), None);
        assert_eq!(last_streaming(&[m(Content::ClientLocal(ClientLocal::Notice("n".into())))]), None);
        assert_eq!(last_streaming(&[s(Streaming::Active), m(Content::ClientLocal(ClientLocal::Notice("n".into())))]), Some(Streaming::Active));
        assert_eq!(last_streaming(&[s(Streaming::Active), s(Streaming::Stopped)]), Some(Streaming::Stopped));
    }

    #[test]
    fn a_short_selection_is_at_most_80_or_the_width() {
        assert!(short_selection(1, 40));
        assert!(short_selection(80, 40));
        assert!(!short_selection(81, 40));
        assert!(short_selection(120, 120));
        assert!(!short_selection(121, 120));
    }

    #[test]
    fn streaming_belongs_to_the_linked_session() {
        let chats = [("a", Some("s1")), ("b", Some("s2")), ("c", None), ("d", Some("s1"))];
        assert_eq!(stream_targets(chats, "s1"), vec!["a".to_string(), "d".to_string()]);
        assert!(stream_targets(chats, "s9").is_empty());
        use crate::chat_message::Streaming;
        assert!(stale_streaming(Some(Streaming::Active), false), "a restart left Active behind");
        assert!(!stale_streaming(Some(Streaming::Active), true));
        assert!(!stale_streaming(Some(Streaming::Stopped), false));
        assert!(!stale_streaming(None, false));
    }

    #[test]
    fn streaming_starts_only_for_the_session_the_warning_named() {
        assert!(streaming_allowed(Some("g1"), "g1", true));
        assert!(!streaming_allowed(Some("g2"), "g1", true), "relinked elsewhere since the question");
        assert!(!streaming_allowed(None, "g1", true), "unlinked since");
        assert!(!streaming_allowed(Some("g1"), "g1", false), "the session closed");
    }

    #[test]
    fn the_watch_follows_every_change_of_link_stream_and_pane() {
        use std::collections::{HashMap, HashSet};
        // The chats' live links and the streaming set as the app keeps them; after each change
        // the watch is what `watched_panes` says of them.
        let mut links: HashMap<&str, Option<crate::layout::PaneId>> = HashMap::from([("a", Some(1)), ("b", Some(2))]);
        let mut streaming: HashSet<String> = HashSet::new();
        let watch = |links: &HashMap<&str, Option<crate::layout::PaneId>>, streaming: &HashSet<String>| watched_panes(links.iter().map(|(c, p)| (*c, *p)), streaming);
        assert!(watch(&links, &streaming).is_empty(), "nothing streams");
        streaming.insert("a".into());
        assert_eq!(watch(&links, &streaming), HashSet::from([1]), "started");
        // Restart: the pane keeps its number, so the same pane is watched again.
        assert_eq!(watch(&links, &streaming), HashSet::from([1]), "restarted");
        links.insert("a", Some(2));
        assert_eq!(watch(&links, &streaming), HashSet::from([2]), "relinked: the watch moves");
        streaming.insert("b".into());
        assert_eq!(watch(&links, &streaming), HashSet::from([2]), "two chats, one pane");
        links.insert("a", None);
        assert_eq!(watch(&links, &streaming), HashSet::from([2]), "one unlinked: the other still watches");
        links.insert("b", None);
        assert!(watch(&links, &streaming).is_empty(), "pane closed, both unlinked");
        links.insert("a", Some(1));
        streaming.remove("a");
        assert!(watch(&links, &streaming).is_empty(), "stopped or deleted");
    }

    #[test]
    fn a_stale_privacy_warning_answer_says_so() {
        use StartStreaming::*;
        assert_eq!(start_streaming_decision(Some("g1"), "g1", true, false), Start);
        assert_eq!(start_streaming_decision(Some("g2"), "g1", true, false), Stale, "relinked elsewhere");
        assert_eq!(start_streaming_decision(None, "g1", true, false), Stale, "unlinked");
        assert_eq!(start_streaming_decision(Some("g1"), "g1", false, false), Stale, "session closed");
        assert_eq!(start_streaming_decision(Some("g1"), "g1", true, true), Already);
        assert_eq!(start_streaming_decision(Some("g2"), "g1", true, true), Stale, "stale wins over already");
        assert!(STREAM_STALE_NOTICE.contains("no longer linked"));
    }

    #[test]
    fn a_session_that_is_gone_unlinks_every_chat_of_it() {
        let linked = vec![("c1".to_string(), "r1".to_string()), ("c2".to_string(), "r2".to_string()), ("c3".to_string(), "r1".to_string())];
        assert_eq!(chats_linked_to(&linked, "r1"), ["c1", "c3"]);
        assert!(chats_linked_to(&linked, "r9").is_empty());
    }

    #[test]
    fn the_watch_follows_the_streaming_chats_current_link() {
        use std::collections::HashSet;
        let streaming: HashSet<String> = ["a", "b"].iter().map(|s| s.to_string()).collect();
        let links = [("a", Some(3)), ("b", None), ("c", Some(9))];
        assert_eq!(watched_panes(links, &streaming), HashSet::from([3]), "b is unlinked, c is not streaming");
        // A streaming chat relinked to another pane moves the watch with it.
        assert_eq!(watched_panes([("a", Some(4)), ("b", Some(4))], &streaming), HashSet::from([4]));
        assert!(watched_panes(links, &HashSet::new()).is_empty());
    }
}
