//! The Python API's dispatcher: the five notification subscriptions `async_get_app` makes
//! (`H:1898-1978`, `H:2121-2150`), iTerm2's transactions (`S:628-737`, `S:1386-1411`,
//! `S:1418-1424`), and the route of a request to an immediate reply or to the app.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use crate::api_proto::{self, pb};
use pb::client_originated_message::Submessage as Req;
use pb::server_originated_message::Submessage as Resp;

/// The notification types 11a handles, all of which ignore the request's `session`
/// (`H:2128-2135`).
pub const HANDLED: [pb::NotificationType; 5] = [
    pb::NotificationType::NotifyOnNewSession,
    pb::NotificationType::NotifyOnTerminateSession,
    pb::NotificationType::NotifyOnLayoutChange,
    pb::NotificationType::NotifyOnFocusChange,
    pb::NotificationType::NotifyOnBroadcastChange,
];

/// Each handled notification type's subscribed connections. iTerm2 keys them by the
/// connection, not by `x-iterm2-key` (`S:797-798`).
#[derive(Clone, Debug, Default)]
pub struct Subscriptions {
    subs: BTreeMap<i32, BTreeSet<u64>>,
}

impl Subscriptions {
    /// iTerm2's answer to a subscribe or unsubscribe (`H:1898-1978`): no `subscribe` field is
    /// REQUEST_MALFORMED, a repeat ALREADY_SUBSCRIBED, an unsubscribe without one
    /// NOT_SUBSCRIBED, else OK. None for a type 11a does not handle.
    pub fn request(&mut self, conn: u64, r: &pb::NotificationRequest) -> Option<pb::notification_response::Status> {
        use pb::notification_response::Status;
        let kind = r.notification_type.and_then(|t| pb::NotificationType::try_from(t).ok()).unwrap_or(pb::NotificationType::NotifyOnKeystroke);
        if !HANDLED.contains(&kind) {
            return None;
        }
        let Some(subscribe) = r.subscribe else { return Some(Status::RequestMalformed) };
        let set = self.subs.entry(kind as i32).or_default();
        let status = match (subscribe, set.contains(&conn)) {
            (true, true) => Status::AlreadySubscribed,
            (true, false) => {
                set.insert(conn);
                Status::Ok
            }
            (false, false) => Status::NotSubscribed,
            (false, true) => {
                set.remove(&conn);
                Status::Ok
            }
        };
        if set.is_empty() {
            self.subs.remove(&(kind as i32));
        }
        Some(status)
    }

    /// The connections subscribed to `kind`, in connection order.
    pub fn subscribers(&self, kind: pb::NotificationType) -> Vec<u64> {
        self.subs.get(&(kind as i32)).map(|s| s.iter().copied().collect()).unwrap_or_default()
    }

    /// Any connection subscribes to anything.
    pub fn any(&self) -> bool {
        self.subs.values().any(|s| !s.is_empty())
    }

    /// A closed connection's subscriptions go (`H:2154-2192`).
    pub fn drop_conn(&mut self, conn: u64) {
        self.subs.retain(|_, s| {
            s.remove(&conn);
            !s.is_empty()
        });
    }
}

/// What to do with a request, given the transaction state.
#[derive(Debug)]
pub enum Tx {
    /// Dispatch it now (no transaction, or the owner's request inside one).
    Run(pb::ClientOriginatedMessage),
    /// Another connection's request during a transaction: it waits for the end.
    Held,
    /// A transaction began: send this OK, then freeze until it ends.
    Began(pb::ServerOriginatedMessage),
    /// The owner ended its transaction: send this OK, then run the released requests.
    Ended(pb::ServerOriginatedMessage),
    /// Answered at once (NO_TRANSACTION, ALREADY_IN_TRANSACTION).
    Reply(pb::ServerOriginatedMessage),
}

/// iTerm2's single server-wide transaction: its owner, and the requests that arrived from other
/// connections while it ran, in arrival order.
#[derive(Debug, Default)]
pub struct Transactions {
    owner: Option<u64>,
    held: VecDeque<(u64, pb::ClientOriginatedMessage)>,
}

/// A TransactionResponse with `status`, carrying the request's id.
fn transaction_reply(req: &pb::ClientOriginatedMessage, status: pb::transaction_response::Status) -> pb::ServerOriginatedMessage {
    let mut m = api_proto::reply_to(req);
    m.submessage = Some(Resp::TransactionResponse(pb::TransactionResponse { status: Some(status as i32) }));
    m
}

impl Transactions {
    /// iTerm2's `enqueueOrDispatchRequest` (`S:1397-1411`), `dispatchRequestWhileNotInTransaction`
    /// (`S:628-670`) and `drainTransaction`'s end (`S:683-698`): outside a transaction an end
    /// answers NO_TRANSACTION and a begin makes `conn` the owner; inside, another connection's
    /// request (a transaction request too) is held, the owner's end ends it, and the owner's
    /// begin answers ALREADY_IN_TRANSACTION (`S:732-737`).
    pub fn offer(&mut self, conn: u64, req: pb::ClientOriginatedMessage) -> Tx {
        use pb::transaction_response::Status;
        let begin = match &req.submessage {
            Some(Req::TransactionRequest(t)) => Some(t.begin()),
            _ => None,
        };
        match (self.owner, begin) {
            (None, Some(false)) => Tx::Reply(transaction_reply(&req, Status::NoTransaction)),
            (None, Some(true)) => {
                self.owner = Some(conn);
                Tx::Began(transaction_reply(&req, Status::Ok))
            }
            (None, None) => Tx::Run(req),
            (Some(owner), _) if owner != conn => {
                self.held.push_back((conn, req));
                Tx::Held
            }
            (Some(_), Some(false)) => {
                self.owner = None;
                Tx::Ended(transaction_reply(&req, Status::Ok))
            }
            (Some(_), Some(true)) => Tx::Reply(transaction_reply(&req, Status::AlreadyInTransaction)),
            (Some(_), None) => Tx::Run(req),
        }
    }

    pub fn owner(&self) -> Option<u64> {
        self.owner
    }

    /// Once no transaction runs, the held requests in arrival order, to be offered again as
    /// iTerm2 re-enqueues them (`S:703-714`); nothing while one still runs.
    pub fn release(&mut self) -> Vec<(u64, pb::ClientOriginatedMessage)> {
        if self.owner.is_some() {
            return Vec::new();
        }
        self.held.drain(..).collect()
    }

    /// A connection closed: its held requests go (iTerm2 skips a request whose connection is
    /// gone, `S:709`), and when it owned the transaction, the transaction ends (`S:1418-1424`)
    /// and the others' held requests are released.
    pub fn closed(&mut self, conn: u64) -> Vec<(u64, pb::ClientOriginatedMessage)> {
        self.held.retain(|(c, _)| *c != conn);
        if self.owner != Some(conn) {
            return Vec::new();
        }
        self.owner = None;
        self.release()
    }
}

/// Where a request goes.
#[derive(Debug)]
#[allow(clippy::large_enum_variant)] // built once and moved rarely; boxing the variant adds an indirection for no gain
pub enum Route {
    /// Answered here.
    Reply(pb::ServerOriginatedMessage),
    /// Answered by the app, which reads its state.
    App(pb::ClientOriginatedMessage),
}

/// Routes a request outside of the transaction logic: NotificationRequests of the five handled
/// types are answered from `subs`, VARIABLE_CHANGE ones and those routed by session (the five
/// session kinds, LOCATION_CHANGE and PROFILE_CHANGE) go to the app; ListSessions, Focus, GetBroadcastDomains and every request
/// of milestone 11b go to the app, as do NOTIFY_ON_SERVER_ORIGINATED_RPC registrations,
/// ServerOriginatedRPCResult, InvokeFunction and StatusBarComponent; a TransactionRequest
/// reaching dispatch (a begin inside a transaction) answers ALREADY_IN_TRANSACTION
/// (`S:732-737`); everything else (tools, an empty oneof) answers iTerm2's malformed error
/// (`S:1040-1044`) until a later milestone handles it.
pub fn route(subs: &mut Subscriptions, conn: u64, req: pb::ClientOriginatedMessage) -> Route {
    match &req.submessage {
        // VARIABLE_CHANGE is routed first (`H:2124-2128`); its objects are the app's.
        Some(Req::NotificationRequest(r)) if r.notification_type == Some(pb::NotificationType::NotifyOnVariableChange as i32) => Route::App(req),
        // Every type iTerm2 routes by `session` (`H:2137-2150`) is the app's: it finds the
        // sessions.
        Some(Req::NotificationRequest(r)) if crate::api_notify::by_session(r) => Route::App(req),
        // Function registrations are the app's (`H:1919-1960`), as are the scripts' results.
        Some(Req::NotificationRequest(r)) if r.notification_type == Some(pb::NotificationType::NotifyOnServerOriginatedRpc as i32) => Route::App(req),
        Some(Req::ServerOriginatedRpcResultRequest(_)) => Route::App(req),
        // Invocations run in the app (`H:4257-4394`).
        Some(Req::InvokeFunctionRequest(_)) => Route::App(req),
        // A script component's popover opens in its session's status bar (`H:4101-4140`).
        Some(Req::StatusBarComponentRequest(_)) => Route::App(req),
        // A dynamic tool joins the Toolbelt menu (`H:2216-2243`).
        Some(Req::RegisterToolRequest(_)) => Route::App(req),
        Some(Req::NotificationRequest(r)) => match subs.request(conn, r) {
            Some(status) => {
                let mut m = api_proto::reply_to(&req);
                m.submessage = Some(Resp::NotificationResponse(pb::NotificationResponse { status: Some(status as i32) }));
                Route::Reply(m)
            }
            None => Route::Reply(api_proto::malformed(&req)),
        },
        Some(
            Req::ListSessionsRequest(_)
            | Req::FocusRequest(_)
            | Req::GetBroadcastDomainsRequest(_)
            | Req::SendTextRequest(_)
            | Req::CreateTabRequest(_)
            | Req::GetBufferRequest(_)
            | Req::GetPromptRequest(_)
            | Req::SetProfilePropertyRequest(_)
            | Req::SplitPaneRequest(_)
            | Req::GetProfilePropertyRequest(_)
            | Req::SetPropertyRequest(_)
            | Req::GetPropertyRequest(_)
            | Req::InjectRequest(_)
            | Req::ActivateRequest(_)
            | Req::VariableRequest(_)
            | Req::SavedArrangementRequest(_)
            | Req::ListProfilesRequest(_)
            | Req::RestartSessionRequest(_)
            | Req::MenuItemRequest(_)
            | Req::SetTabLayoutRequest(_)
            | Req::TmuxRequest(_)
            | Req::ReorderTabsRequest(_)
            | Req::PreferencesRequest(_)
            | Req::ColorPresetRequest(_)
            | Req::SelectionRequest(_)
            | Req::SetBroadcastDomainsRequest(_)
            | Req::CloseRequest(_)
            | Req::ListPromptsRequest(_)
            | Req::ScreenshotRequest(_),
        ) => Route::App(req),
        Some(Req::TransactionRequest(_)) => Route::Reply(transaction_reply(&req, pb::transaction_response::Status::AlreadyInTransaction)),
        _ => Route::Reply(api_proto::malformed(&req)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pb::notification_response::Status as NStatus;
    use pb::transaction_response::Status as TStatus;
    use pb::NotificationType as N;

    fn msg(id: i64, sub: Req) -> pb::ClientOriginatedMessage {
        pb::ClientOriginatedMessage { id: Some(id), submessage: Some(sub) }
    }

    fn tx(id: i64, begin: bool) -> pb::ClientOriginatedMessage {
        msg(id, Req::TransactionRequest(pb::TransactionRequest { begin: Some(begin) }))
    }

    fn list(id: i64) -> pb::ClientOriginatedMessage {
        msg(id, Req::ListSessionsRequest(pb::ListSessionsRequest {}))
    }

    fn notif(kind: N, subscribe: Option<bool>, session: Option<&str>) -> pb::NotificationRequest {
        pb::NotificationRequest { session: session.map(str::to_string), subscribe, notification_type: Some(kind as i32), arguments: None }
    }

    /// The transaction status and id of a reply.
    fn tstatus(m: &pb::ServerOriginatedMessage) -> (Option<i64>, TStatus) {
        match &m.submessage {
            Some(Resp::TransactionResponse(r)) => (m.id, TStatus::try_from(r.status.expect("status is always set")).unwrap()),
            other => panic!("not a transaction response: {other:?}"),
        }
    }

    fn ids(v: &[(u64, pb::ClientOriginatedMessage)]) -> Vec<(u64, Option<i64>)> {
        v.iter().map(|(c, r)| (*c, r.id)).collect()
    }

    #[test]
    fn transactions_follow_iterm2() {
        let mut t = Transactions::default();
        // No transaction: an end answers NO_TRANSACTION with the request's id.
        match t.offer(1, tx(10, false)) {
            Tx::Reply(m) => assert_eq!(tstatus(&m), (Some(10), TStatus::NoTransaction)),
            other => panic!("{other:?}"),
        }
        assert_eq!(t.owner(), None);
        // A begin answers OK and makes 1 the owner.
        match t.offer(1, tx(11, true)) {
            Tx::Began(m) => assert_eq!(tstatus(&m), (Some(11), TStatus::Ok)),
            other => panic!("{other:?}"),
        }
        assert_eq!(t.owner(), Some(1));
        // Another connection's request waits; the owner's runs.
        assert!(matches!(t.offer(2, list(20)), Tx::Held));
        assert!(matches!(t.offer(1, list(12)), Tx::Run(r) if r.id == Some(12)));
        // Nothing is released while the transaction runs.
        assert!(t.release().is_empty());
        // A second begin from the owner.
        match t.offer(1, tx(13, true)) {
            Tx::Reply(m) => assert_eq!(tstatus(&m), (Some(13), TStatus::AlreadyInTransaction)),
            other => panic!("{other:?}"),
        }
        // The owner's end.
        match t.offer(1, tx(14, false)) {
            Tx::Ended(m) => assert_eq!(tstatus(&m), (Some(14), TStatus::Ok)),
            other => panic!("{other:?}"),
        }
        assert_eq!(t.owner(), None);
        assert_eq!(ids(&t.release()), [(2, Some(20))]);
        assert!(t.release().is_empty());
        // The owner closing ends the transaction and releases the others' requests in order;
        // a transaction request from another connection is held like any other.
        assert!(matches!(t.offer(1, tx(15, true)), Tx::Began(_)));
        assert!(matches!(t.offer(2, list(21)), Tx::Held));
        assert!(matches!(t.offer(2, tx(22, true)), Tx::Held));
        assert!(matches!(t.offer(3, list(30)), Tx::Held));
        assert!(t.closed(3).is_empty(), "a closed non-owner's held requests go");
        assert_eq!(t.owner(), Some(1));
        assert_eq!(ids(&t.closed(1)), [(2, Some(21)), (2, Some(22))]);
        assert_eq!(t.owner(), None);
        // No transaction: a close returns nothing.
        assert!(t.closed(3).is_empty());
        assert!(t.closed(1).is_empty());
    }

    /// Released requests are offered again in order, as iTerm2 re-enqueues them: a held
    /// begin starts the next transaction at once, and that connection's own later requests,
    /// its end included, run in it before anything waits on the socket.
    #[test]
    fn released_requests_are_offered_again() {
        let mut t = Transactions::default();
        assert!(matches!(t.offer(1, tx(1, true)), Tx::Began(_)));
        for (conn, req) in [(2, tx(20, true)), (3, list(30)), (2, list(21)), (2, tx(22, false))] {
            assert!(matches!(t.offer(conn, req), Tx::Held));
        }
        assert!(matches!(t.offer(1, tx(2, false)), Tx::Ended(_)));
        let mut work: VecDeque<_> = t.release().into();
        let mut seen = Vec::new();
        while let Some((conn, req)) = work.pop_front() {
            let id = req.id;
            let step = match t.offer(conn, req) {
                Tx::Run(_) => "run",
                Tx::Held => "held",
                Tx::Began(_) => "began",
                Tx::Reply(_) => "reply",
                Tx::Ended(_) => {
                    work.extend(t.release());
                    "ended"
                }
            };
            seen.push((conn, id, step));
        }
        assert_eq!(
            seen,
            [(2, Some(20), "began"), (3, Some(30), "held"), (2, Some(21), "run"), (2, Some(22), "ended"), (3, Some(30), "run")]
        );
        assert_eq!(t.owner(), None);
    }

    #[test]
    fn subscriptions_follow_iterm2() {
        let mut s = Subscriptions::default();
        assert!(!s.any());
        let sub = notif(N::NotifyOnNewSession, Some(true), Some("all"));
        let unsub = notif(N::NotifyOnNewSession, Some(false), Some("all"));
        assert_eq!(s.request(1, &sub), Some(NStatus::Ok));
        assert!(s.any());
        assert_eq!(s.request(1, &sub), Some(NStatus::AlreadySubscribed));
        assert_eq!(s.request(1, &unsub), Some(NStatus::Ok));
        assert_eq!(s.request(1, &unsub), Some(NStatus::NotSubscribed));
        assert!(!s.any());
        // No `subscribe` field.
        assert_eq!(s.request(1, &notif(N::NotifyOnFocusChange, None, None)), Some(NStatus::RequestMalformed));
        assert!(!s.any());
        // Connections are separate; subscribers come in connection order.
        assert_eq!(s.request(2, &sub), Some(NStatus::Ok));
        assert_eq!(s.request(1, &sub), Some(NStatus::Ok));
        assert_eq!(s.subscribers(N::NotifyOnNewSession), [1, 2]);
        assert_eq!(s.subscribers(N::NotifyOnFocusChange), Vec::<u64>::new());
        // The session field is ignored for the five: any value, or none, is one subscription.
        for (i, kind) in HANDLED.iter().enumerate() {
            let session = [None, Some("all"), Some("99"), Some(""), Some("x")][i];
            assert_eq!(s.request(1, &notif(*kind, Some(true), session)), Some(if *kind == N::NotifyOnNewSession { NStatus::AlreadySubscribed } else { NStatus::Ok }));
            assert_eq!(s.request(1, &notif(*kind, Some(true), Some("other"))), Some(NStatus::AlreadySubscribed));
        }
        // A closed connection's subscriptions all go; the other's stay.
        s.drop_conn(1);
        for kind in HANDLED {
            assert!(!s.subscribers(kind).contains(&1));
        }
        assert_eq!(s.subscribers(N::NotifyOnNewSession), [2]);
        assert!(s.any());
        s.drop_conn(2);
        assert!(!s.any());
        // A type 11a does not handle, subscribed or not; a missing type is iTerm2's default,
        // NOTIFY_ON_KEYSTROKE.
        assert_eq!(s.request(1, &notif(N::NotifyOnKeystroke, Some(true), Some("all"))), None);
        assert_eq!(s.request(1, &notif(N::NotifyOnKeystroke, None, Some("all"))), None);
        let untyped = pb::NotificationRequest { notification_type: None, subscribe: Some(true), ..Default::default() };
        assert_eq!(s.request(1, &untyped), None);
        assert!(!s.any());
    }

    #[test]
    #[allow(deprecated)]
    fn routes_follow_iterm2() {
        let mut s = Subscriptions::default();
        let is_malformed = |r: &Route, id: i64| matches!(r, Route::Reply(m) if *m == api_proto::malformed(&pb::ClientOriginatedMessage { id: Some(id), submessage: None }));
        // An empty message.
        let r = route(&mut s, 1, pb::ClientOriginatedMessage { id: Some(1), submessage: None });
        assert!(is_malformed(&r, 1));
        // A handled NotificationRequest answers its status.
        match route(&mut s, 1, msg(2, Req::NotificationRequest(notif(N::NotifyOnFocusChange, Some(true), Some("all"))))) {
            Route::Reply(m) => {
                assert_eq!(m.id, Some(2));
                match m.submessage {
                    Some(Resp::NotificationResponse(r)) => assert_eq!(r.status, Some(NStatus::Ok as i32)),
                    other => panic!("{other:?}"),
                }
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(s.subscribers(N::NotifyOnFocusChange), [1]);
        // The types routed by session go to the app, a missing type (iTerm2's default,
        // NOTIFY_ON_KEYSTROKE) too.
        for kind in [N::NotifyOnKeystroke, N::NotifyOnScreenUpdate, N::NotifyOnPrompt, N::NotifyOnCustomEscapeSequence, N::KeystrokeFilter, N::NotifyOnLocationChange, N::NotifyOnProfileChange] {
            assert!(matches!(route(&mut s, 1, msg(3, Req::NotificationRequest(notif(kind, Some(true), Some("all"))))), Route::App(r) if r.id == Some(3)), "{kind:?}");
        }
        let untyped = pb::NotificationRequest { notification_type: None, subscribe: Some(true), ..Default::default() };
        assert!(matches!(route(&mut s, 1, msg(3, Req::NotificationRequest(untyped))), Route::App(_)));
        // Server-originated RPC registrations and results are the app's (`H:1919-1960`,
        // `H:3490-3496`), subscribing or not.
        for subscribe in [Some(true), Some(false), None] {
            assert!(matches!(route(&mut s, 1, msg(3, Req::NotificationRequest(notif(N::NotifyOnServerOriginatedRpc, subscribe, Some("all"))))), Route::App(r) if r.id == Some(3)));
        }
        assert!(matches!(route(&mut s, 1, msg(8, Req::ServerOriginatedRpcResultRequest(Default::default()))), Route::App(r) if r.id == Some(8)));
        assert!(matches!(route(&mut s, 1, msg(9, Req::InvokeFunctionRequest(Default::default()))), Route::App(r) if r.id == Some(9)));
        assert!(matches!(route(&mut s, 1, msg(10, Req::StatusBarComponentRequest(Default::default()))), Route::App(r) if r.id == Some(10)));
        assert!(matches!(route(&mut s, 1, msg(11, Req::RegisterToolRequest(Default::default()))), Route::App(r) if r.id == Some(11)));
        // VARIABLE_CHANGE goes to the app, subscribing or not.
        for subscribe in [Some(true), Some(false), None] {
            assert!(matches!(route(&mut s, 1, msg(7, Req::NotificationRequest(notif(N::NotifyOnVariableChange, subscribe, None)))), Route::App(r) if r.id == Some(7)));
        }
        assert!(s.subscribers(N::NotifyOnVariableChange).is_empty());
        // The three `async_get_app` requests go to the app.
        assert!(matches!(route(&mut s, 1, list(4)), Route::App(r) if r.id == Some(4)));
        assert!(matches!(route(&mut s, 1, msg(5, Req::FocusRequest(pb::FocusRequest {}))), Route::App(r) if r.id == Some(5)));
        assert!(matches!(route(&mut s, 1, msg(6, Req::GetBroadcastDomainsRequest(pb::GetBroadcastDomainsRequest {}))), Route::App(r) if r.id == Some(6)));
        // Every 11b request goes to the app.
        let app = [
            Req::SendTextRequest(pb::SendTextRequest { session: Some("1".into()), text: Some("ls".into()), suppress_broadcast: None }),
            Req::CreateTabRequest(Default::default()),
            Req::GetBufferRequest(Default::default()),
            Req::GetPromptRequest(Default::default()),
            Req::SetProfilePropertyRequest(Default::default()),
            Req::SplitPaneRequest(Default::default()),
            Req::GetProfilePropertyRequest(Default::default()),
            Req::SetPropertyRequest(Default::default()),
            Req::GetPropertyRequest(Default::default()),
            Req::InjectRequest(Default::default()),
            Req::ActivateRequest(Default::default()),
            Req::VariableRequest(Default::default()),
            Req::SavedArrangementRequest(Default::default()),
            Req::ListProfilesRequest(Default::default()),
            Req::RestartSessionRequest(Default::default()),
            Req::MenuItemRequest(Default::default()),
            Req::SetTabLayoutRequest(Default::default()),
            Req::TmuxRequest(Default::default()),
            Req::ReorderTabsRequest(Default::default()),
            Req::PreferencesRequest(Default::default()),
            Req::ColorPresetRequest(Default::default()),
            Req::SelectionRequest(Default::default()),
            Req::SetBroadcastDomainsRequest(Default::default()),
            Req::CloseRequest(Default::default()),
            Req::ListPromptsRequest(Default::default()),
            Req::ScreenshotRequest(Default::default()),
        ];
        for (i, sub) in app.into_iter().enumerate() {
            let id = 100 + i as i64;
            assert!(matches!(route(&mut s, 1, msg(id, sub)), Route::App(r) if r.id == Some(id)), "request {i}");
        }
    }
}
