//! The Python API's session notifications: the subscriptions a session keeps by connection
//! (`PS:23760-23821`) and the "all" subscriptions kept for future sessions (`H:2053-2106`,
//! `H:909-925`); keystroke notifications and the keystroke filter (`PS:11251-11326`,
//! `PS:11515-11626`); screen updates (`PS:13241-13254`); prompt, command start and command
//! end (`PS:16709-16726`, `PS:18645-18656`, `PS:18711-18722`); custom escape sequences
//! (`PS:18266-18284`).

use std::collections::BTreeMap;

use crate::api_proto::pb;
use crate::keystroke::{self, Keystroke};
use pb::notification_request::Arguments;
use pb::notification_response::Status;
use pb::NotificationType as N;

/// The types a session keeps a subscription dictionary for (`PS:23766-23782`).
pub const SESSION_KINDS: [pb::NotificationType; 5] = [N::NotifyOnKeystroke, N::NotifyOnScreenUpdate, N::NotifyOnPrompt, N::NotifyOnCustomEscapeSequence, N::KeystrokeFilter];

/// A request's type; unset (or a value the proto does not know) reads as iTerm2's proto2
/// default, NOTIFY_ON_KEYSTROKE.
pub fn kind(r: &pb::NotificationRequest) -> pb::NotificationType {
    r.notification_type.and_then(|t| pb::NotificationType::try_from(t).ok()).unwrap_or(N::NotifyOnKeystroke)
}

/// The types iTerm2 routes by the request's `session` (`H:2121-2156`): the five session kinds,
/// and NOTIFY_ON_LOCATION_CHANGE and NOTIFY_ON_PROFILE_CHANGE, which no session accepts.
#[allow(deprecated)]
pub fn by_session(r: &pb::NotificationRequest) -> bool {
    let k = kind(r);
    SESSION_KINDS.contains(&k) || matches!(k, N::NotifyOnLocationChange | N::NotifyOnProfileChange)
}

/// Each session's subscriptions by type and pane, one request per connection, and the "all"
/// requests applied to every session made later.
#[derive(Clone, Debug, Default)]
pub struct SessionSubs {
    per: BTreeMap<(i32, u64), BTreeMap<u64, pb::NotificationRequest>>,
    all: Vec<(u64, pb::NotificationRequest)>,
}

impl SessionSubs {
    /// iTerm2's answer (`H:2137-2150`): "all" asks every session in `all_panes` (any status but
    /// OK, ALREADY_SUBSCRIBED and NOT_SUBSCRIBED is returned) and then keeps the request for
    /// future sessions or removes every equal one (NOT_SUBSCRIBED when none was kept); else
    /// the session `target` names (None: SESSION_NOT_FOUND).
    pub fn request(&mut self, conn: u64, r: &pb::NotificationRequest, target: Option<u64>, all_panes: &[u64]) -> Status {
        if r.session.as_deref() == Some("all") {
            for &pane in all_panes {
                let s = self.session_request(conn, r, pane);
                if !matches!(s, Status::Ok | Status::AlreadySubscribed | Status::NotSubscribed) {
                    return s;
                }
            }
            if r.subscribe == Some(true) {
                self.all.push((conn, r.clone()));
                return Status::Ok;
            }
            // iTerm2 removes every kept request equal to this one with subscribe set, whatever
            // its connection (`H:2080-2090`).
            let mut key = r.clone();
            key.subscribe = Some(true);
            let before = self.all.len();
            self.all.retain(|(_, q)| *q != key);
            return if self.all.len() == before { Status::NotSubscribed } else { Status::Ok };
        }
        match target {
            Some(pane) => self.session_request(conn, r, pane),
            None => Status::SessionNotFound,
        }
    }

    /// A session's own handler (`PS:23760-23821`): no `subscribe` REQUEST_MALFORMED, a type
    /// with no dictionary REQUEST_MALFORMED, then keyed by the connection.
    fn session_request(&mut self, conn: u64, r: &pb::NotificationRequest, pane: u64) -> Status {
        let Some(subscribe) = r.subscribe else { return Status::RequestMalformed };
        let k = kind(r);
        if !SESSION_KINDS.contains(&k) {
            return Status::RequestMalformed;
        }
        let key = (k as i32, pane);
        let map = self.per.entry(key).or_default();
        let status = match (subscribe, map.contains_key(&conn)) {
            (true, true) => Status::AlreadySubscribed,
            (true, false) => {
                map.insert(conn, r.clone());
                Status::Ok
            }
            (false, false) => Status::NotSubscribed,
            (false, true) => {
                map.remove(&conn);
                Status::Ok
            }
        };
        if map.is_empty() {
            self.per.remove(&key);
        }
        status
    }

    /// A new session takes every "all" request, in the order they were made (`H:909-925`).
    pub fn new_session(&mut self, pane: u64) {
        for (conn, r) in self.all.clone() {
            self.session_request(conn, &r, pane);
        }
    }

    /// An ended session's subscriptions go with it.
    pub fn forget_session(&mut self, pane: u64) {
        self.per.retain(|(_, p), _| *p != pane);
    }

    /// A closed connection's subscriptions all go, "all" ones included (`H:2154-2192`,
    /// `PS:8120`).
    pub fn drop_conn(&mut self, conn: u64) {
        self.per.retain(|_, m| {
            m.remove(&conn);
            !m.is_empty()
        });
        self.all.retain(|(c, _)| *c != conn);
    }

    /// The connections subscribed to `kind` on `pane`, in connection order, with their
    /// requests.
    pub fn subscribers(&self, kind: pb::NotificationType, pane: u64) -> Vec<(u64, &pb::NotificationRequest)> {
        self.per.get(&(kind as i32, pane)).map(|m| m.iter().map(|(c, r)| (*c, r)).collect()).unwrap_or_default()
    }

    /// The panes with a subscriber to `kind`.
    pub fn watching(&self, kind: pb::NotificationType) -> Vec<u64> {
        self.per.keys().filter(|(k, _)| *k == kind as i32).map(|(_, p)| *p).collect()
    }

    /// Any subscription is kept, for a session or for all of them.
    pub fn any(&self) -> bool {
        !self.per.is_empty() || !self.all.is_empty()
    }
}

/// A key event as the API reports it: `NSEvent`'s characters, characters ignoring modifiers,
/// modifiers, key code and type.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Key {
    pub characters: Option<String>,
    pub ignoring: Option<String>,
    pub modifiers: Vec<i32>,
    pub keycode: i32,
    pub action: i32,
}

fn text_of(c: u32) -> Option<String> {
    char::from_u32(c).map(String::from)
}

/// iTerm2's modifier order (`PS:11568-11589`): Control, Option, Command, Shift, Numpad,
/// Function.
fn modifiers(ctrl: bool, alt: bool, cmd: bool, shift: bool, numpad: bool, function: bool) -> Vec<i32> {
    use pb::Modifiers as M;
    [(ctrl, M::Control), (alt, M::Option), (cmd, M::Command), (shift, M::Shift), (numpad, M::Numpad), (function, M::Function)]
        .into_iter()
        .filter(|(on, _)| *on)
        .map(|(_, m)| m as i32)
        .collect()
}

/// A key down or up: the typed character, the character without modifiers but Shift, the
/// modifiers (Function for a macOS function-key character, U+F700 to U+F8FF, as AppKit sets
/// it), and the macOS key code (0 when the key has none).
pub fn key_from(k: &Keystroke, action: pb::keystroke_notification::Action) -> Key {
    let f = k.flags;
    let function = (0xF700..=0xF8FF).contains(&k.ch);
    Key {
        characters: text_of(k.modified),
        ignoring: text_of(k.ch),
        modifiers: modifiers(f & keystroke::CONTROL != 0, f & keystroke::OPTION != 0, f & keystroke::COMMAND != 0, f & keystroke::SHIFT != 0, f & keystroke::NUMPAD != 0, function),
        keycode: k.keycode.map_or(0, i32::from),
        action: action as i32,
    }
}

/// A modifier key going down or up: no characters (`PS:11593-11596`), the modifiers held
/// after it, its key code.
pub fn flags_key(mods: crate::mouse::Mods, keycode: u16) -> Key {
    Key {
        characters: None,
        ignoring: None,
        modifiers: modifiers(mods.ctrl, mods.alt, mods.cmd, mods.shift, false, false),
        keycode: i32::from(keycode),
        action: pb::keystroke_notification::Action::FlagsChanged as i32,
    }
}

/// iTerm2's `event:matchesPattern:` (`PS:11251-11304`): key downs only; every required and no
/// forbidden modifier, then any key code, characters or characters ignoring modifiers.
pub fn matches(p: &pb::KeystrokePattern, k: &Key) -> bool {
    if k.action != pb::keystroke_notification::Action::KeyDown as i32 {
        return false;
    }
    if !p.required_modifiers.iter().all(|m| k.modifiers.contains(m)) || p.forbidden_modifiers.iter().any(|m| k.modifiers.contains(m)) {
        return false;
    }
    p.keycodes.contains(&k.keycode)
        || k.characters.as_ref().is_some_and(|c| p.characters.contains(c))
        || k.ignoring.as_ref().is_some_and(|c| p.characters_ignoring_modifiers.contains(c))
}

/// iTerm2's `keystrokeIsFilteredByMonitor:` (`PS:11306-11326`) over one session's
/// KEYSTROKE_FILTER requests: any filter pattern, or any deprecated monitor pattern.
pub fn filtered<'a>(subs: impl Iterator<Item = &'a pb::NotificationRequest>, k: &Key) -> bool {
    subs.into_iter().any(|r| match &r.arguments {
        Some(Arguments::KeystrokeFilterRequest(f)) => f.patterns_to_ignore.iter().any(|p| matches(p, k)),
        #[allow(deprecated)]
        Some(Arguments::KeystrokeMonitorRequest(m)) => m.patterns_to_ignore.iter().any(|p| matches(p, k)),
        _ => false,
    })
}

/// A key down goes to every keystroke subscriber; key up and flags changed only to those
/// asking for `advanced` (`PS:11617-11619`).
pub fn wants_key(r: &pb::NotificationRequest, advanced_event: bool) -> bool {
    !advanced_event || matches!(&r.arguments, Some(Arguments::KeystrokeMonitorRequest(m)) if m.advanced == Some(true))
}

/// The KeystrokeNotification (`PS:11591-11626`).
pub fn keystroke(k: &Key, session: &str) -> pb::Notification {
    pb::Notification {
        keystroke_notification: Some(pb::KeystrokeNotification {
            characters: k.characters.clone(),
            characters_ignoring_modifiers: k.ignoring.clone(),
            modifiers: k.modifiers.clone(),
            key_code: Some(k.keycode),
            session: Some(session.to_string()),
            action: Some(k.action),
        }),
        ..Default::default()
    }
}

/// Whether a PROMPT subscription wants events of `mode`: PROMPT with no arguments or PROMPT
/// among its modes (`PS:16713-16714`); COMMAND_START and COMMAND_END only when among its
/// modes (`PS:18646`, `PS:18712`).
pub fn wants_prompt(r: &pb::NotificationRequest, mode: pb::PromptMonitorMode) -> bool {
    let listed = matches!(&r.arguments, Some(Arguments::PromptMonitorRequest(p)) if p.modes.contains(&(mode as i32)));
    listed || (mode == pb::PromptMonitorMode::Prompt && r.arguments.is_none())
}

/// What a watched screen saw for the API, in order: the prompt's end (B) with the current
/// prompt mark, a command's start (C) with its text, its end (D) with the code, and a custom
/// escape sequence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ApiEvent {
    Prompt(Option<u32>),
    CommandStart { command: Option<String>, mark: Option<u32> },
    CommandEnd { code: i32, mark: Option<u32> },
    Custom { id: String, payload: String },
}

/// The ScreenUpdateNotification (`PS:13244-13246`).
pub fn screen_update(session: &str) -> pb::Notification {
    pb::Notification { screen_update_notification: Some(pb::ScreenUpdateNotification { session: Some(session.to_string()) }), ..Default::default() }
}

/// The CustomEscapeSequenceNotification (`PS:18274-18278`).
pub fn custom(session: &str, id: &str, payload: &str) -> pb::Notification {
    pb::Notification {
        custom_escape_sequence_notification: Some(pb::CustomEscapeSequenceNotification {
            session: Some(session.to_string()),
            sender_identity: Some(id.to_string()),
            payload: Some(payload.to_string()),
        }),
        ..Default::default()
    }
}

/// The PromptNotification for a prompt event and the mode it belongs to; `prompt` is the
/// GetPrompt answer for a Prompt event's mark. None for a custom escape sequence.
pub fn prompt_event(session: &str, ev: &ApiEvent, prompt: Option<pb::GetPromptResponse>) -> Option<(pb::PromptMonitorMode, pb::Notification)> {
    use pb::prompt_notification::Event;
    use pb::PromptMonitorMode as Mode;
    let (mode, event, mark) = match ev {
        ApiEvent::Prompt(mark) => (Mode::Prompt, Event::Prompt(pb::PromptNotificationPrompt { placeholder: Some(String::new()), prompt }), *mark),
        ApiEvent::CommandStart { command, mark } => (Mode::CommandStart, Event::CommandStart(pb::PromptNotificationCommandStart { command: command.clone() }), *mark),
        ApiEvent::CommandEnd { code, mark } => (Mode::CommandEnd, Event::CommandEnd(pb::PromptNotificationCommandEnd { status: Some(*code) }), *mark),
        ApiEvent::Custom { .. } => return None,
    };
    let n = pb::PromptNotification { session: Some(session.to_string()), unique_prompt_id: mark.map(|m| m.to_string()), event: Some(event) };
    Some((mode, pb::Notification { prompt_notification: Some(n), ..Default::default() }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keystroke::{CONTROL, SHIFT};
    use pb::keystroke_notification::Action;
    use pb::Modifiers as M;

    fn req(kind: N, subscribe: Option<bool>, session: Option<&str>) -> pb::NotificationRequest {
        pb::NotificationRequest { session: session.map(str::to_string), subscribe, notification_type: Some(kind as i32), arguments: None }
    }

    #[test]
    #[allow(deprecated)]
    fn session_subscriptions_follow_iterm2() {
        let mut s = SessionSubs::default();
        let sub = req(N::NotifyOnKeystroke, Some(true), Some("1"));
        let unsub = req(N::NotifyOnKeystroke, Some(false), Some("1"));
        assert_eq!(s.request(1, &sub, Some(1), &[]), Status::Ok);
        assert_eq!(s.request(1, &sub, Some(1), &[]), Status::AlreadySubscribed);
        assert_eq!(s.request(1, &unsub, Some(1), &[]), Status::Ok);
        assert_eq!(s.request(1, &unsub, Some(1), &[]), Status::NotSubscribed);
        assert!(!s.any());
        // No `subscribe` field.
        assert_eq!(s.request(1, &req(N::NotifyOnKeystroke, None, Some("1")), Some(1), &[]), Status::RequestMalformed);
        // A session not found (not "all").
        assert_eq!(s.request(1, &req(N::NotifyOnKeystroke, Some(true), Some("99")), None, &[1, 2]), Status::SessionNotFound);
        // No session has a dictionary for these two.
        assert_eq!(s.request(1, &req(N::NotifyOnLocationChange, Some(true), Some("1")), Some(1), &[]), Status::RequestMalformed);
        assert_eq!(s.request(1, &req(N::NotifyOnProfileChange, Some(true), Some("1")), Some(1), &[]), Status::RequestMalformed);
        // PROFILE_CHANGE is not app-wide: session "" is not found.
        assert_eq!(s.request(1, &req(N::NotifyOnProfileChange, Some(true), Some("")), None, &[1, 2]), Status::SessionNotFound);
        assert_eq!(s.request(1, &req(N::NotifyOnProfileChange, Some(true), Some("all")), None, &[1, 2]), Status::RequestMalformed);
        assert!(!s.any());
        // "all": every session, and future ones.
        let all = req(N::NotifyOnKeystroke, Some(true), Some("all"));
        let unall = req(N::NotifyOnKeystroke, Some(false), Some("all"));
        assert_eq!(s.request(1, &all, None, &[1, 2]), Status::Ok);
        assert_eq!(s.subscribers(N::NotifyOnKeystroke, 2).iter().map(|(c, _)| *c).collect::<Vec<_>>(), [1]);
        assert_eq!(s.watching(N::NotifyOnKeystroke), [1, 2]);
        s.new_session(9);
        assert_eq!(s.subscribers(N::NotifyOnKeystroke, 9).iter().map(|(c, _)| *c).collect::<Vec<_>>(), [1]);
        assert_eq!(s.request(1, &req(N::NotifyOnKeystroke, Some(true), Some("9")), Some(9), &[]), Status::AlreadySubscribed);
        assert_eq!(s.request(1, &unall, None, &[1, 2, 9]), Status::Ok);
        assert_eq!(s.request(1, &unall, None, &[1, 2, 9]), Status::NotSubscribed);
        // Unsubscribing "all" left no kept request: a new session takes nothing.
        s.new_session(10);
        assert!(s.subscribers(N::NotifyOnKeystroke, 10).is_empty());
        // An ended session's subscriptions go; a closed connection's all go.
        assert_eq!(s.request(1, &all, None, &[1, 2]), Status::Ok);
        assert_eq!(s.request(2, &req(N::KeystrokeFilter, Some(true), Some("1")), Some(1), &[]), Status::Ok);
        s.forget_session(1);
        assert!(s.subscribers(N::NotifyOnKeystroke, 1).is_empty());
        assert!(s.subscribers(N::KeystrokeFilter, 1).is_empty());
        assert_eq!(s.watching(N::NotifyOnKeystroke), [2]);
        s.drop_conn(1);
        assert!(s.watching(N::NotifyOnKeystroke).is_empty());
        s.new_session(11);
        assert!(s.subscribers(N::NotifyOnKeystroke, 11).is_empty(), "the closed connection's \"all\" request went too");
        assert!(!s.any());
    }

    #[test]
    fn keystrokes_follow_iterm2() {
        let k = key_from(&Keystroke::new('a' as u32, CONTROL | SHIFT, Some(0), 'A' as u32), Action::KeyDown);
        assert_eq!(k, Key { characters: Some("A".into()), ignoring: Some("a".into()), modifiers: vec![M::Control as i32, M::Shift as i32], keycode: 0, action: Action::KeyDown as i32 });
        // An arrow: Numpad (Tandem sets it as iTerm2 does) then Function, in iTerm2's order.
        let k = key_from(&Keystroke::new(0xF702, 0, Some(0x7B), 0xF702), Action::KeyDown);
        assert_eq!(k.modifiers, [M::Numpad as i32, M::Function as i32]);
        assert_eq!(k.keycode, 0x7B);
        let f = flags_key(crate::mouse::Mods { shift: true, ..Default::default() }, 0x38);
        assert_eq!(f, Key { characters: None, ignoring: None, modifiers: vec![M::Shift as i32], keycode: 0x38, action: Action::FlagsChanged as i32 });
        // Advanced events only reach `advanced` subscribers.
        let plain = req(N::NotifyOnKeystroke, Some(true), Some("1"));
        assert!(wants_key(&plain, false));
        assert!(!wants_key(&plain, true));
        let advanced = pb::NotificationRequest { arguments: Some(Arguments::KeystrokeMonitorRequest(pb::KeystrokeMonitorRequest { advanced: Some(true), ..Default::default() })), ..plain.clone() };
        assert!(wants_key(&advanced, false));
        assert!(wants_key(&advanced, true));
        let n = keystroke(&f, "1").keystroke_notification.unwrap();
        assert_eq!(n.session.as_deref(), Some("1"));
        assert_eq!((n.characters, n.characters_ignoring_modifiers, n.key_code, n.action), (None, None, Some(0x38), Some(Action::FlagsChanged as i32)));
    }

    #[test]
    fn filters_follow_iterm2() {
        let ctrl_c = key_from(&Keystroke::new('c' as u32, CONTROL, Some(0x08), 3), Action::KeyDown);
        let c = key_from(&Keystroke::new('c' as u32, 0, Some(0x08), 'c' as u32), Action::KeyDown);
        let ctrl_shift_c = key_from(&Keystroke::new('C' as u32, CONTROL | SHIFT, Some(0x08), 3), Action::KeyDown);
        let p = pb::KeystrokePattern { required_modifiers: vec![M::Control as i32], characters_ignoring_modifiers: vec!["c".into()], ..Default::default() };
        assert!(matches(&p, &ctrl_c));
        assert!(!matches(&p, &c));
        let no_shift = pb::KeystrokePattern { forbidden_modifiers: vec![M::Shift as i32], characters_ignoring_modifiers: vec!["c".into(), "C".into()], ..p.clone() };
        assert!(matches(&no_shift, &ctrl_c));
        assert!(!matches(&no_shift, &ctrl_shift_c));
        let a = key_from(&Keystroke::new('a' as u32, SHIFT, Some(0), 'A' as u32), Action::KeyDown);
        assert!(matches(&pb::KeystrokePattern { keycodes: vec![0], ..Default::default() }, &a));
        assert!(matches(&pb::KeystrokePattern { characters: vec!["A".into()], ..Default::default() }, &a));
        // Modifiers alone are necessary, never sufficient.
        assert!(!matches(&pb::KeystrokePattern { required_modifiers: vec![M::Shift as i32], ..Default::default() }, &a));
        // Only key downs match.
        let up = Key { action: Action::KeyUp as i32, ..a.clone() };
        assert!(!matches(&pb::KeystrokePattern { keycodes: vec![0], ..Default::default() }, &up));
        // The filter reads KEYSTROKE_FILTER's patterns and the deprecated monitor patterns.
        let filter = pb::NotificationRequest { arguments: Some(Arguments::KeystrokeFilterRequest(pb::KeystrokeFilterRequest { patterns_to_ignore: vec![p.clone()] })), ..req(N::KeystrokeFilter, Some(true), Some("1")) };
        assert!(filtered([&filter].into_iter(), &ctrl_c));
        assert!(!filtered([&filter].into_iter(), &c));
        #[allow(deprecated)]
        let old = pb::NotificationRequest { arguments: Some(Arguments::KeystrokeMonitorRequest(pb::KeystrokeMonitorRequest { patterns_to_ignore: vec![p.clone()], advanced: None })), ..req(N::KeystrokeFilter, Some(true), Some("1")) };
        assert!(filtered([&old].into_iter(), &ctrl_c));
        assert!(!filtered([&req(N::KeystrokeFilter, Some(true), Some("1"))].into_iter(), &ctrl_c));
        assert!(!filtered(std::iter::empty(), &ctrl_c));
    }

    #[test]
    fn prompt_modes_follow_iterm2() {
        use pb::PromptMonitorMode as Mode;
        let bare = req(N::NotifyOnPrompt, Some(true), Some("1"));
        assert!(wants_prompt(&bare, Mode::Prompt));
        assert!(!wants_prompt(&bare, Mode::CommandStart));
        assert!(!wants_prompt(&bare, Mode::CommandEnd));
        let end = pb::NotificationRequest { arguments: Some(Arguments::PromptMonitorRequest(pb::PromptMonitorRequest { modes: vec![Mode::CommandEnd as i32] })), ..bare.clone() };
        assert!(!wants_prompt(&end, Mode::Prompt));
        assert!(!wants_prompt(&end, Mode::CommandStart));
        assert!(wants_prompt(&end, Mode::CommandEnd));
        let keys = pb::NotificationRequest { arguments: Some(Arguments::KeystrokeMonitorRequest(Default::default())), ..bare.clone() };
        for mode in [Mode::Prompt, Mode::CommandStart, Mode::CommandEnd] {
            assert!(!wants_prompt(&keys, mode));
        }
        use pb::prompt_notification::Event;
        let resp = pb::GetPromptResponse { status: Some(pb::get_prompt_response::Status::Ok as i32), unique_prompt_id: Some("4".into()), ..Default::default() };
        let (mode, n) = prompt_event("1", &ApiEvent::Prompt(Some(4)), Some(resp.clone())).unwrap();
        assert_eq!(mode, Mode::Prompt);
        assert_eq!(
            n.prompt_notification.unwrap(),
            pb::PromptNotification { session: Some("1".into()), event: Some(Event::Prompt(pb::PromptNotificationPrompt { placeholder: Some(String::new()), prompt: Some(resp) })), unique_prompt_id: Some("4".into()) }
        );
        let (mode, n) = prompt_event("1", &ApiEvent::CommandStart { command: Some("ls".into()), mark: Some(4) }, None).unwrap();
        assert_eq!(mode, Mode::CommandStart);
        let n = n.prompt_notification.unwrap();
        assert_eq!(n.event, Some(Event::CommandStart(pb::PromptNotificationCommandStart { command: Some("ls".into()) })));
        assert_eq!(n.unique_prompt_id.as_deref(), Some("4"));
        let (mode, n) = prompt_event("1", &ApiEvent::CommandEnd { code: 2, mark: None }, None).unwrap();
        assert_eq!(mode, Mode::CommandEnd);
        let n = n.prompt_notification.unwrap();
        assert_eq!(n.event, Some(Event::CommandEnd(pb::PromptNotificationCommandEnd { status: Some(2) })));
        assert_eq!(n.unique_prompt_id, None);
        assert!(prompt_event("1", &ApiEvent::Custom { id: "me".into(), payload: "hi".into() }, None).is_none());
        // The other two notifications.
        assert_eq!(screen_update("1").screen_update_notification.unwrap().session.as_deref(), Some("1"));
        let c = custom("1", "me", "hi").custom_escape_sequence_notification.unwrap();
        assert_eq!((c.session.as_deref(), c.sender_identity.as_deref(), c.payload.as_deref()), (Some("1"), Some("me"), Some("hi")));
    }
}
