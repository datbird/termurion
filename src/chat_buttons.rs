//! The buttons under a chat bubble, when they are enabled, and what a press does (iTerm2's
//! `ChatViewController` buttons, `CVC:2779-2885`, and its enabled rules, `CVC:1962-2050`).
//! Pure: the app reads the live facts, this module decides. Buttons never answer the model
//! themselves: a press becomes a `Press`, which the app hands to 12c's public answers.

use crate::ai_permissions::{summary_line, Category, Permission};
use crate::ai_tools::escape_code;
use crate::chat_message::{one_key, ClientLocal, Content, Message, Payload, Streaming};
use serde_json::Value;

/// One button of a bubble: its title, whether it is drawn in the error color, and its id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Button {
    pub title: String,
    pub destructive: bool,
    pub id: String,
}

pub const ENABLE_ORCHESTRATION: &str = "Enable Orchestration";
/// The picker's button (`iTermSessionSelector`).
pub const PICK_REASON: &str = "Link this session to AI chat?";

const CANCEL: &str = "Cancel";

fn button(title: &str, destructive: bool, id: &str) -> Button {
    Button { title: title.to_string(), destructive, id: id.to_string() }
}

/// The buttons a message shows (`CVC:2779-2885`). `perm` answers the permissions bubble's
/// current state for one category.
pub fn buttons(content: &Content, perm: &dyn Fn(Category) -> Permission) -> Vec<Button> {
    match content {
        Content::ClientLocal(local) => match local {
            ClientLocal::PickingSession | ClientLocal::ExecutingCommand(_) => vec![button(CANCEL, true, "")],
            ClientLocal::Notice(_) => vec![],
            ClientLocal::StreamingChanged(Streaming::Active) => vec![button("Stop", true, "")],
            ClientLocal::StreamingChanged(_) => vec![],
            ClientLocal::OfferLink { .. } => vec![button("Link", false, "link"), button(ENABLE_ORCHESTRATION, false, ORCHESTRATE)],
            ClientLocal::OfferOrchestration => vec![button(ENABLE_ORCHESTRATION, false, ORCHESTRATE)],
            ClientLocal::Permissions { terminal, .. } => Category::ALL
                .into_iter()
                .filter(|c| c.is_browser() != *terminal)
                .map(|c| button(&summary_line(c, perm(c)), false, c.raw()))
                .collect(),
            ClientLocal::Other(v) => orchestration_buttons(v),
        },
        Content::SelectSessionRequest { .. } => vec![button("Select a Session", false, PICK_SESSION), button(CANCEL, true, CANCEL_ID)],
        Content::RemoteCommandRequest { payload: Payload::Classic(_), .. } => {
            vec![button("Allow Once", false, "allowOnce"), button("Always Allow", false, "allowAlways"), button("Deny this Time", true, "denyOnce"), button("Always Deny", true, "denyAlways")]
        }
        _ => vec![],
    }
}

const ORCHESTRATE: &str = "orchestrate";
const PICK_SESSION: &str = "pickSession";
const CANCEL_ID: &str = "cancel";

/// The orchestration actions' buttons by their JSON discriminator (`CVC:2830-2870`); the ids
/// carry the discriminator so no other button can share one.
fn orchestration_buttons(v: &Value) -> Vec<Button> {
    let Some((kind, body)) = one_key(v) else { return vec![] };
    let text = |k: &str| body.get(k).and_then(Value::as_str).unwrap_or_default();
    match kind {
        "workgroupPermissionRequest" => {
            let id = text("requestID");
            vec![button("Approve", false, &format!("workgroupPermission:approve:{id}")), button("Deny", true, &format!("workgroupPermission:deny:{id}"))]
        }
        "enableOrchestrationRequest" => {
            let id = text("requestID");
            vec![button(ENABLE_ORCHESTRATION, false, &format!("enableOrchestration:approve:{id}")), button("Not Now", true, &format!("enableOrchestration:deny:{id}"))]
        }
        "orchestrationPermissionGranted" => vec![button("Revoke", true, &format!("revokeOrchestrationPermission:{}", text("scope")))],
        _ => vec![],
    }
}

/// What the app knows about the live state, for one message.
#[derive(Clone, Debug, Default)]
pub struct EnableFacts {
    pub is_last: bool,
    pub picking: bool,
    pub executing: bool,
    pub streaming: bool,
    pub offer_session_live: bool,
    pub linked: bool,
    pub orchestrating: bool,
    pub permissions_session_live: bool,
    pub claimed: bool,
    pub answered: bool,
    /// An orchestration request that is still parked on the person's answer.
    pub open: bool,
}

/// Whether the message's buttons can be pressed (`CVC:1962-2050`): the last message only,
/// unless the message's own rule says otherwise.
pub fn enabled(content: &Content, f: &EnableFacts) -> bool {
    match content {
        Content::ClientLocal(local) => match local {
            ClientLocal::PickingSession => f.is_last && f.picking,
            ClientLocal::ExecutingCommand(_) => f.is_last && f.executing,
            ClientLocal::StreamingChanged(_) => f.streaming,
            // Moot once either choice is made, last or not (`CVC:1996-2012`).
            ClientLocal::OfferLink { .. } => f.offer_session_live && !f.linked && !f.orchestrating,
            ClientLocal::OfferOrchestration => !f.orchestrating,
            ClientLocal::Permissions { .. } => f.permissions_session_live,
            ClientLocal::Other(v) => match one_key(v) {
                Some(("orchestrationPermissionGranted", _)) => f.claimed,
                // A request answers only while it is still parked on the person (ruling P7).
                Some(("workgroupPermissionRequest" | "enableOrchestrationRequest", _)) => f.is_last && f.open,
                _ => f.is_last,
            },
            ClientLocal::Notice(_) => f.is_last,
        },
        Content::RemoteCommandRequest { .. } => f.is_last && !f.answered,
        _ => f.is_last,
    }
}

/// Whether a `RemoteCommandResponse` names `request_id`, in the whole chat (hidden messages
/// included: the answer to a request is hidden from the client).
pub fn answered(visible_and_hidden: &[Message], request_id: &str) -> bool {
    visible_and_hidden.iter().any(|m| matches!(&m.content, Content::RemoteCommandResponse { request_id: r, .. } if r == request_id))
}

/// The person's answer to a request, before the app maps it onto 12c's `Choice`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    AllowOnce,
    AlwaysAllow,
    DenyOnce,
    AlwaysDeny,
}

pub fn answer(button_id: &str) -> Option<Answer> {
    match button_id {
        "allowOnce" => Some(Answer::AllowOnce),
        "allowAlways" => Some(Answer::AlwaysAllow),
        "denyOnce" => Some(Answer::DenyOnce),
        "denyAlways" => Some(Answer::AlwaysDeny),
        _ => None,
    }
}

/// What a press asks the app to do. The app maps each onto an existing function; none writes
/// a message to the model.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Press {
    /// Cancel on "Waiting for a session".
    CancelPick,
    /// Cancel on an executing command.
    CancelTool,
    /// Stop on the streaming bubble.
    StopStreaming,
    /// Link on the offer.
    Link { terminal: bool, guid: String },
    /// A permissions button: cycle this category.
    Permission(Category),
    /// Cancel on a select-session request: decline the original request.
    Decline { request: String },
    /// Select a Session on a select-session request.
    PickSession { terminal: bool, request: String },
    /// One of the four buttons of a request.
    Answer(Answer),
    /// "Enable Orchestration" on an offer: the person's own switch.
    Orchestrate,
    /// Approve or Deny on a permission request.
    OrchPermission { request_id: String, approved: bool },
    /// Enable Orchestration or Not Now on the model's enable request.
    OrchEnable { request_id: String, approved: bool },
    /// Revoke on a grant notice.
    OrchRevoke { scope: String },
}

/// What pressing `button` on a message does; None when the press must be ignored: the button is
/// not one this message shows, or the message's buttons are not enabled now (a stale click).
/// `enabled` is `enabled(content, facts)`
/// computed from the live state at the moment of the press.
pub fn press(content: &Content, perm: &dyn Fn(Category) -> Permission, enabled: bool, button: &str) -> Option<Press> {
    if !enabled || !buttons(content, perm).iter().any(|b| b.id == button) {
        return None;
    }
    match content {
        Content::ClientLocal(local) => match local {
            ClientLocal::PickingSession => Some(Press::CancelPick),
            ClientLocal::ExecutingCommand(_) => Some(Press::CancelTool),
            ClientLocal::StreamingChanged(_) => Some(Press::StopStreaming),
            ClientLocal::OfferLink { .. } | ClientLocal::OfferOrchestration if button == ORCHESTRATE => Some(Press::Orchestrate),
            ClientLocal::OfferLink { terminal, guid, .. } => Some(Press::Link { terminal: *terminal, guid: guid.clone() }),
            ClientLocal::Permissions { .. } => Category::parse(button).map(Press::Permission),
            ClientLocal::Other(_) => orchestration_press(button),
            _ => None,
        },
        Content::SelectSessionRequest { original, terminal } => match button {
            CANCEL_ID => Some(Press::Decline { request: original.unique_id.clone() }),
            PICK_SESSION => Some(Press::PickSession { terminal: *terminal, request: original.unique_id.clone() }),
            _ => None,
        },
        Content::RemoteCommandRequest { payload: Payload::Classic(_), .. } => answer(button).map(Press::Answer),
        _ => None,
    }
}

/// What an orchestration button id asks (the ids `orchestration_buttons` made; the caller has
/// checked the message shows the id, so the request id is the message's own).
fn orchestration_press(button: &str) -> Option<Press> {
    let pair = |prefix: &str| -> Option<(String, bool)> {
        let (verb, id) = button.strip_prefix(prefix)?.split_once(':')?;
        Some((id.to_string(), match verb { "approve" => true, "deny" => false, _ => return None }))
    };
    if let Some((request_id, approved)) = pair("workgroupPermission:") {
        return Some(Press::OrchPermission { request_id, approved });
    }
    if let Some((request_id, approved)) = pair("enableOrchestration:") {
        return Some(Press::OrchEnable { request_id, approved });
    }
    button.strip_prefix("revokeOrchestrationPermission:").map(|scope| Press::OrchRevoke { scope: scope.to_string() })
}

/// The notice after a link (`CVC:1277-1306`): the session's name is a terminal program's to
/// set, so a backtick is escaped as 12c's `escape_code` does and a control character or line
/// break becomes a space.
pub fn linked_notice(terminal: bool, name: Option<&str>) -> String {
    let kind = if terminal { "terminal" } else { "web browser" };
    let name = match name {
        Some(n) => escape_code(&n.chars().map(|c| if c.is_control() || c == '\u{2028}' || c == '\u{2029}' { ' ' } else { c }).collect::<String>()),
        None => "(Unnamed session)".to_string(),
    };
    format!("This chat has been linked to {kind} session \u{201C}{name}\u{201D}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use crate::ai_tools::Call;
    use crate::chat_message::{AiError, Participant, RemoteCommand};
    use serde_json::json;

    fn map(pairs: &[(Category, Permission)]) -> BTreeMap<Category, Permission> {
        pairs.iter().copied().collect()
    }

    fn local(l: ClientLocal) -> Content {
        Content::ClientLocal(l)
    }

    fn b(title: &str, destructive: bool, id: &str) -> Button {
        button(title, destructive, id)
    }

    fn request(safe: Option<bool>) -> Content {
        Content::RemoteCommandRequest { payload: Payload::Classic(RemoteCommand { llm_message: json!({}), call: Call::IsAtPrompt }), safe }
    }

    fn none(_: Category) -> Permission {
        Permission::Never
    }

    #[test]
    fn button_sets_follow_iterm2() {
        assert_eq!(buttons(&local(ClientLocal::PickingSession), &none), vec![b("Cancel", true, "")]);
        let running = RemoteCommand { llm_message: json!({}), call: Call::IsAtPrompt };
        assert_eq!(buttons(&local(ClientLocal::ExecutingCommand(running)), &none), vec![b("Cancel", true, "")]);
        assert_eq!(buttons(&local(ClientLocal::StreamingChanged(Streaming::Active)), &none), vec![b("Stop", true, "")]);
        assert_eq!(buttons(&local(ClientLocal::StreamingChanged(Streaming::Stopped)), &none), vec![]);
        assert_eq!(buttons(&local(ClientLocal::StreamingChanged(Streaming::StoppedAutomatically)), &none), vec![]);
        assert_eq!(buttons(&local(ClientLocal::Notice("n".into())), &none), vec![]);
        let offer = ClientLocal::OfferLink { terminal: true, guid: "g".into(), name: None };
        assert_eq!(buttons(&local(offer), &none), vec![b("Link", false, "link"), b("Enable Orchestration", false, "orchestrate")]);
        assert_eq!(buttons(&local(ClientLocal::OfferOrchestration), &none), vec![b("Enable Orchestration", false, "orchestrate")]);

        let states = map(&[(Category::RunCommands, Permission::Always), (Category::CheckTerminalState, Permission::Always), (Category::ViewContents, Permission::Ask)]);
        let perm = |c: Category| states.get(&c).copied().unwrap_or(Permission::Never);
        let terminal = buttons(&local(ClientLocal::Permissions { terminal: true, guid: "g".into() }), &perm);
        let titles: Vec<&str> = terminal.iter().map(|x| x.title.as_str()).collect();
        assert_eq!(
            titles,
            [
                "AI can Check Terminal State: Provided automatically",
                "AI can Run Commands: Always",
                "AI can View Contents: Ask",
                "AI can Write to the Clipboard: Never",
                "AI can Control Terminal: Never",
                "AI can View Manpages: Never",
                "AI can Write to the File System: Never",
            ]
        );
        let ids: Vec<&str> = terminal.iter().map(|x| x.id.as_str()).collect();
        assert_eq!(ids, ["Check Terminal State", "Run Commands", "View Contents", "Write to the Clipboard", "Control Terminal", "View Manpages", "Write to the File System"]);
        assert!(terminal.iter().all(|x| !x.destructive));
        let web = buttons(&local(ClientLocal::Permissions { terminal: false, guid: "g".into() }), &|_| Permission::Ask);
        assert_eq!(web, vec![b("AI can Act in Web Browser: Ask", false, "Act in Web Browser")]);

        assert_eq!(
            buttons(&request(None), &none),
            vec![b("Allow Once", false, "allowOnce"), b("Always Allow", false, "allowAlways"), b("Deny this Time", true, "denyOnce"), b("Always Deny", true, "denyAlways")]
        );
        assert_eq!(buttons(&Content::RemoteCommandRequest { payload: Payload::External(json!({})), safe: None }, &none), vec![]);
        let select = Content::SelectSessionRequest { original: Box::new(Message::new("c", Participant::Agent, request(None), 1.0)), terminal: true };
        assert_eq!(buttons(&select, &none), vec![b("Select a Session", false, "pickSession"), b("Cancel", true, "cancel")]);

        let other = |v| buttons(&local(ClientLocal::Other(v)), &none);
        assert_eq!(other(json!({"workgroupPermissionRequest": {"requestID": "R"}})).iter().map(|x| (x.title.as_str(), x.destructive)).collect::<Vec<_>>(), [("Approve", false), ("Deny", true)]);
        assert_eq!(other(json!({"enableOrchestrationRequest": {"requestID": "R"}})).iter().map(|x| (x.title.as_str(), x.destructive)).collect::<Vec<_>>(), [("Enable Orchestration", false), ("Not Now", true)]);
        assert_eq!(other(json!({"orchestrationPermissionGranted": {"scope": "s"}})).iter().map(|x| (x.title.as_str(), x.destructive)).collect::<Vec<_>>(), [("Revoke", true)]);
        assert_eq!(other(json!({"unknown": {}})), vec![]);

        for c in [
            Content::PlainText { text: "t".into(), context: None },
            Content::Markdown("m".into()),
            Content::RemoteCommandResponse { result: Err(AiError { message: "e".into(), kind: 0 }), request_id: "r".into(), function_name: "f".into(), function_call_id: None },
        ] {
            assert_eq!(buttons(&c, &none), vec![]);
        }
    }

    fn last() -> EnableFacts {
        EnableFacts { is_last: true, ..EnableFacts::default() }
    }

    #[test]
    fn enabled_rules_follow_iterm2() {
        let plain = Content::PlainText { text: "t".into(), context: None };
        assert!(enabled(&plain, &last()));
        assert!(!enabled(&plain, &EnableFacts::default()));

        let picking = local(ClientLocal::PickingSession);
        assert!(!enabled(&picking, &last()), "no pick is open");
        assert!(enabled(&picking, &EnableFacts { picking: true, ..last() }));
        assert!(!enabled(&picking, &EnableFacts { picking: true, is_last: false, ..EnableFacts::default() }));

        let exec = local(ClientLocal::ExecutingCommand(RemoteCommand { llm_message: json!({}), call: Call::IsAtPrompt }));
        assert!(!enabled(&exec, &last()), "the command is no longer executing");
        assert!(enabled(&exec, &EnableFacts { executing: true, ..last() }));

        let stream = local(ClientLocal::StreamingChanged(Streaming::Active));
        assert!(enabled(&stream, &EnableFacts { streaming: true, ..EnableFacts::default() }), "last or not");
        assert!(!enabled(&stream, &last()));

        let offer = local(ClientLocal::OfferLink { terminal: true, guid: "g".into(), name: None });
        let live = EnableFacts { offer_session_live: true, ..EnableFacts::default() };
        assert!(enabled(&offer, &live), "last or not");
        assert!(!enabled(&offer, &EnableFacts { offer_session_live: false, ..last() }));
        assert!(!enabled(&offer, &EnableFacts { linked: true, ..live.clone() }));
        assert!(!enabled(&offer, &EnableFacts { orchestrating: true, ..live }));

        let orch = local(ClientLocal::OfferOrchestration);
        assert!(enabled(&orch, &EnableFacts::default()));
        assert!(!enabled(&orch, &EnableFacts { orchestrating: true, ..last() }));

        let perms = local(ClientLocal::Permissions { terminal: true, guid: "g".into() });
        assert!(enabled(&perms, &EnableFacts { permissions_session_live: true, ..EnableFacts::default() }), "even when not last");
        assert!(!enabled(&perms, &last()));

        let revoke = local(ClientLocal::Other(json!({"orchestrationPermissionGranted": {"scope": "s"}})));
        assert!(enabled(&revoke, &EnableFacts { claimed: true, ..EnableFacts::default() }));
        assert!(!enabled(&revoke, &last()));
        let approve = local(ClientLocal::Other(json!({"workgroupPermissionRequest": {}})));
        assert!(enabled(&approve, &EnableFacts { open: true, ..last() }));
        assert!(!enabled(&approve, &last()), "a request no longer parked is moot");
        assert!(!enabled(&approve, &EnableFacts { open: true, ..EnableFacts::default() }), "and only the last message answers");
        let enable = local(ClientLocal::Other(json!({"enableOrchestrationRequest": {"requestID": "R"}})));
        assert!(enabled(&enable, &EnableFacts { open: true, ..last() }) && !enabled(&enable, &last()));

        assert!(enabled(&request(None), &last()));
        assert!(!enabled(&request(None), &EnableFacts { answered: true, ..last() }));
        assert!(!enabled(&request(None), &EnableFacts::default()), "not the last message");

        let response = Message::new("c", Participant::User, Content::RemoteCommandResponse { result: Ok("ok".into()), request_id: "R".into(), function_name: "f".into(), function_call_id: None }, 2.0);
        let mut asked = Message::new("c", Participant::Agent, request(None), 1.0);
        asked.unique_id = "R".into();
        assert!(answered(&[asked.clone(), response], "R"));
        assert!(!answered(&[asked], "R"));
        assert!(!answered(&[], "R"));
    }

    #[test]
    fn consent_texts_follow_iterm2() {
        assert_eq!(answer("allowOnce"), Some(Answer::AllowOnce));
        assert_eq!(answer("allowAlways"), Some(Answer::AlwaysAllow));
        assert_eq!(answer("denyOnce"), Some(Answer::DenyOnce));
        assert_eq!(answer("denyAlways"), Some(Answer::AlwaysDeny));
        assert_eq!(answer("x"), None);
        assert_eq!(answer(""), None);
        assert_eq!(ENABLE_ORCHESTRATION, "Enable Orchestration");
                assert_eq!(PICK_REASON, "Link this session to AI chat?");
        assert_eq!(linked_notice(true, Some("zsh")), "This chat has been linked to terminal session \u{201C}zsh\u{201D}");
        assert_eq!(linked_notice(false, Some("x")), "This chat has been linked to web browser session \u{201C}x\u{201D}");
        assert_eq!(linked_notice(true, None), "This chat has been linked to terminal session \u{201C}(Unnamed session)\u{201D}");
        assert_eq!(linked_notice(true, Some("a`b")), "This chat has been linked to terminal session \u{201C}a\\`b\u{201D}");
        assert_eq!(linked_notice(true, Some("a\nb\u{7}c")), "This chat has been linked to terminal session \u{201C}a b c\u{201D}", "a title cannot add a line to the notice");
    }

    #[test]
    fn presses_do_only_what_the_button_says() {
        let all = |c: &Content, id: &str| press(c, &none, true, id);
        assert_eq!(all(&request(None), "allowAlways"), Some(Press::Answer(Answer::AlwaysAllow)));
        assert_eq!(all(&request(Some(false)), "denyOnce"), Some(Press::Answer(Answer::DenyOnce)));
        // A stale click: the buttons are not enabled now.
        for id in ["allowOnce", "allowAlways", "denyOnce", "denyAlways"] {
            assert_eq!(press(&request(None), &none, false, id), None, "{id} on an answered or superseded request");
        }
        // An id the message does not show, model-written or forged.
        assert_eq!(all(&request(None), "pickSession"), None);
        assert_eq!(all(&request(None), ""), None);
        assert_eq!(all(&request(None), "Allow Once"), None, "the title is not the id");
        // An orchestration call is never gated by these buttons.
        assert_eq!(all(&Content::RemoteCommandRequest { payload: Payload::External(json!({})), safe: None }, "allowOnce"), None);
        // A plain message has no buttons.
        assert_eq!(all(&Content::PlainText { text: "allowOnce".into(), context: None }, "allowOnce"), None);

        assert_eq!(all(&local(ClientLocal::PickingSession), ""), Some(Press::CancelPick));
        let running = ClientLocal::ExecutingCommand(RemoteCommand { llm_message: json!({}), call: Call::IsAtPrompt });
        assert_eq!(all(&local(running), ""), Some(Press::CancelTool));
        assert_eq!(all(&local(ClientLocal::StreamingChanged(Streaming::Active)), ""), Some(Press::StopStreaming));
        assert_eq!(all(&local(ClientLocal::StreamingChanged(Streaming::Stopped)), ""), None);
        let offer = ClientLocal::OfferLink { terminal: true, guid: "G".into(), name: None };
        assert_eq!(all(&local(offer.clone()), "link"), Some(Press::Link { terminal: true, guid: "G".into() }));
        assert_eq!(all(&local(offer), "orchestrate"), Some(Press::Orchestrate));
        assert_eq!(all(&local(ClientLocal::OfferOrchestration), "orchestrate"), Some(Press::Orchestrate));
        let perms = local(ClientLocal::Permissions { terminal: true, guid: "G".into() });
        assert_eq!(all(&perms, "Run Commands"), Some(Press::Permission(Category::RunCommands)));
        assert_eq!(all(&perms, "Act in Web Browser"), None, "not a terminal category");
        assert_eq!(all(&perms, "Nonsense"), None);
        let web = local(ClientLocal::Permissions { terminal: false, guid: "G".into() });
        assert_eq!(all(&web, "Act in Web Browser"), Some(Press::Permission(Category::ActInWebBrowser)));
        let other = |v: Value, id: &str| all(&local(ClientLocal::Other(v)), id);
        let ask = json!({"workgroupPermissionRequest": {"requestID": "R"}});
        assert_eq!(other(ask.clone(), "workgroupPermission:approve:R"), Some(Press::OrchPermission { request_id: "R".into(), approved: true }));
        assert_eq!(other(ask.clone(), "workgroupPermission:deny:R"), Some(Press::OrchPermission { request_id: "R".into(), approved: false }));
        let enable = json!({"enableOrchestrationRequest": {"requestID": "R"}});
        assert_eq!(other(enable.clone(), "enableOrchestration:approve:R"), Some(Press::OrchEnable { request_id: "R".into(), approved: true }));
        assert_eq!(other(enable.clone(), "enableOrchestration:deny:R"), Some(Press::OrchEnable { request_id: "R".into(), approved: false }));
        assert_eq!(other(json!({"orchestrationPermissionGranted": {"scope": "s"}}), "revokeOrchestrationPermission:s"), Some(Press::OrchRevoke { scope: "s".into() }));
        // A button this message does not show, a forged request id, or another bubble's: nothing.
        assert_eq!(other(enable.clone(), "enableOrchestration:approve:OTHER"), None);
        assert_eq!(other(enable, "workgroupPermission:approve:R"), None);
        assert_eq!(other(ask, "revokeOrchestrationPermission:s"), None);

        let mut original = Message::new("c", Participant::Agent, request(None), 1.0);
        original.unique_id = "REQ".into();
        let select = Content::SelectSessionRequest { original: Box::new(original), terminal: true };
        assert_eq!(all(&select, "cancel"), Some(Press::Decline { request: "REQ".into() }));
        assert_eq!(all(&select, "pickSession"), Some(Press::PickSession { terminal: true, request: "REQ".into() }));
        assert_eq!(all(&select, "allowOnce"), None, "a select-session bubble never allows a command");
    }
}
