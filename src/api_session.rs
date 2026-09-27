//! The Python API's session control: how ids name windows, tabs and sessions
//! (`H:1615-1644`), and the plans for SendText (`H:2435-2463`), Inject (`H:2986-3009`),
//! Activate (`H:3011-3104`) and Close (`H:4189-4255`), made from a snapshot of the app's
//! windows so the app only carries them out.

use std::collections::BTreeSet;

use crate::api_proto::pb;

/// The session id naming the current window's focused session.
pub const ACTIVE: &str = "active";
/// The session id naming every session in a window.
pub const ALL: &str = "all";

/// What ids resolve against: each window by number (its Registry key, its `pty-<UUID>` id,
/// its tabs as (tab id, panes)), the buried sessions, the sessions whose program exited, and
/// the current window's focused session.
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub windows: Vec<(u64, String, Vec<(u64, Vec<u64>)>)>,
    pub buried: Vec<u64>,
    pub exited: BTreeSet<u64>,
    pub active: Option<u64>,
}

impl Snapshot {
    /// The Registry key of the window whose id is `guid` (iTerm2's `terminalGuid`).
    pub fn window_key(&self, guid: &str) -> Option<u64> {
        self.windows.iter().find(|(_, id, _)| id == guid).map(|(k, _, _)| *k)
    }

    /// The Registry key of the window holding tab `tab`.
    pub fn tab_window(&self, tab: u64) -> Option<u64> {
        self.windows.iter().find(|(_, _, tabs)| tabs.iter().any(|(t, _)| *t == tab)).map(|(k, _, _)| *k)
    }

    /// iTerm2's `sessionForAPIIdentifier:includeBuriedSessions:YES` (`H:1615-1634`): "active"
    /// is the current window's focused session; any other id is compared as the session's
    /// GUID string, which in Tandem is the pane id in decimal, so "07" or "+7" name nothing.
    /// Buried sessions are found.
    pub fn pane(&self, id: &str) -> Option<u64> {
        if id == ACTIVE {
            return self.active;
        }
        let p = id.parse::<u64>().ok().filter(|p| p.to_string() == id)?;
        (self.pane_tab(p).is_some() || self.buried(p)).then_some(p)
    }

    /// The tab showing `pane`; None for a buried one.
    pub fn pane_tab(&self, pane: u64) -> Option<u64> {
        self.windows.iter().flat_map(|(_, _, tabs)| tabs).find(|(_, panes)| panes.contains(&pane)).map(|(t, _)| *t)
    }

    pub fn buried(&self, pane: u64) -> bool {
        self.buried.contains(&pane)
    }

    pub fn exited(&self, pane: u64) -> bool {
        self.exited.contains(&pane)
    }

    /// iTerm2's `allSessions` (`H:1981-1985`): every window's sessions by window number, then
    /// tab and pane order; buried sessions are in no window.
    pub fn all_panes(&self) -> Vec<u64> {
        self.windows.iter().flat_map(|(_, _, tabs)| tabs).flat_map(|(_, panes)| panes.iter().copied()).collect()
    }

    /// iTerm2's `allTabs` (`H:1987-1991`).
    pub fn all_tabs(&self) -> Vec<u64> {
        self.windows.iter().flat_map(|(_, _, tabs)| tabs.iter().map(|(t, _)| *t)).collect()
    }

    pub fn window_guids(&self) -> Vec<String> {
        self.windows.iter().map(|(_, id, _)| id.clone()).collect()
    }

    /// iTerm2's `tabWithID:` (`IC:1293-1311`): decimal digits only, then the tab with that
    /// number.
    fn tab_by_id(&self, id: &str) -> Option<u64> {
        if id.is_empty() || !id.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let t = int_value(id);
        u64::try_from(t).ok().filter(|t| self.tab_window(*t).is_some())
    }
}

/// NSString's `intValue`: leading white space, an optional sign, then decimal digits, 0 when
/// there are none, clamped to 32 bits.
fn int_value(s: &str) -> i64 {
    let t = s.trim_start_matches([' ', '\t', '\n', '\r']);
    let (negative, digits) = match t.as_bytes().first() {
        Some(b'-') => (true, &t[1..]),
        Some(b'+') => (false, &t[1..]),
        _ => (false, t),
    };
    let mut n: i64 = 0;
    for b in digits.bytes().take_while(u8::is_ascii_digit) {
        n = (n * 10 + i64::from(b - b'0')).min(i64::from(i32::MAX) + 1);
    }
    if negative { (-n).max(i64::from(i32::MIN)) } else { n.min(i64::from(i32::MAX)) }
}

/// SendText (`H:2435-2463`): "all" writes to every window's sessions (exited ones too; the
/// app skips them at the write); any other id must name a session that has not exited, else
/// SESSION_NOT_FOUND and nothing is written. Each write is (pane, bytes, broadcast):
/// `suppress_broadcast` writes without broadcasting (`writeTaskNoBroadcast`).
pub fn send_text(r: &pb::SendTextRequest, s: &Snapshot) -> (pb::SendTextResponse, Vec<(u64, Vec<u8>, bool)>) {
    use pb::send_text_response::Status;
    let session = r.session.as_deref();
    let panes = if session == Some(ALL) {
        s.all_panes()
    } else {
        match session.and_then(|id| s.pane(id)).filter(|p| !s.exited(*p)) {
            Some(p) => vec![p],
            None => return (pb::SendTextResponse { status: Some(Status::SessionNotFound as i32) }, Vec::new()),
        }
    };
    let bytes = r.text.clone().unwrap_or_default().into_bytes();
    let broadcast = !r.suppress_broadcast.unwrap_or(false);
    let writes = panes.into_iter().map(|p| (p, bytes.clone(), broadcast)).collect();
    (pb::SendTextResponse { status: Some(Status::Ok as i32) }, writes)
}

/// Inject (`H:2986-3009`): one status per id, "all" one OK for every window's sessions; the
/// panes to feed, in order, exited and buried ones included.
pub fn inject(r: &pb::InjectRequest, s: &Snapshot) -> (pb::InjectResponse, Vec<u64>) {
    use pb::inject_response::Status;
    let (mut status, mut panes) = (Vec::new(), Vec::new());
    for id in &r.session_id {
        if id == ALL {
            panes.extend(s.all_panes());
            status.push(Status::Ok as i32);
        } else if let Some(p) = s.pane(id) {
            panes.push(p);
            status.push(Status::Ok as i32);
        } else {
            status.push(Status::SessionNotFound as i32);
        }
    }
    (pb::InjectResponse { status }, panes)
}

/// What Activate does, in iTerm2's order (`H:3011-3104`): the target (a window and tab by
/// Registry key and tab id; a session's pane, `unbury` when it must be restored first, its
/// tab then found by the app), then app activation, raising the window, selecting the tab,
/// selecting the session. Each flag is a step that runs; `error` is the answer when a check
/// failed, after the steps before it ran.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ActivatePlan {
    pub window: Option<u64>,
    pub tab: Option<u64>,
    pub pane: Option<u64>,
    pub unbury: bool,
    pub activate_app: bool,
    pub raise: bool,
    pub select_tab: bool,
    pub select_session: bool,
    pub error: Option<pb::activate_response::Status>,
}

/// iTerm2's Activate checks: an identifier that names nothing is BAD_IDENTIFIER before
/// anything happens; the app is activated; then raising needs a window, selecting a tab a
/// tab, selecting a session a session, each INVALID_OPTION. A tab id is read with `intValue`
/// (`tabWithUniqueId:`), unlike Close's digits-only `tabWithID:`.
pub fn activate(r: &pb::ActivateRequest, s: &Snapshot) -> ActivatePlan {
    use pb::activate_request::Identifier;
    use pb::activate_response::Status;
    let mut plan = ActivatePlan::default();
    let bad = ActivatePlan { error: Some(Status::BadIdentifier), ..Default::default() };
    match &r.identifier {
        Some(Identifier::TabId(id)) => {
            let Some(tab) = u64::try_from(int_value(id)).ok().filter(|t| s.tab_window(*t).is_some()) else { return bad };
            plan.tab = Some(tab);
            plan.window = s.tab_window(tab);
        }
        Some(Identifier::WindowId(id)) => {
            let Some(k) = s.window_key(id) else { return bad };
            plan.window = Some(k);
        }
        Some(Identifier::SessionId(id)) => {
            let Some(p) = s.pane(id) else { return bad };
            plan.pane = Some(p);
            if s.buried(p) {
                plan.unbury = true;
            } else {
                plan.tab = s.pane_tab(p);
                plan.window = plan.tab.and_then(|t| s.tab_window(t));
            }
        }
        None => {}
    }
    // A buried session's window and tab exist once it is restored.
    let restored = plan.unbury;
    plan.activate_app = r.activate_app.is_some();
    if r.order_window_front.unwrap_or(false) {
        if plan.window.is_none() && !restored {
            plan.error = Some(Status::InvalidOption);
            return plan;
        }
        plan.raise = true;
    }
    if r.select_tab.unwrap_or(false) {
        if plan.tab.is_none() && !restored {
            plan.error = Some(Status::InvalidOption);
            return plan;
        }
        plan.select_tab = true;
    }
    if r.select_session.unwrap_or(false) {
        if plan.pane.is_none() {
            plan.error = Some(Status::InvalidOption);
            return plan;
        }
        plan.select_session = true;
    }
    plan
}

/// One id of a Close request, in iTerm2's order (`H:4189-4255`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloseStep {
    Window(u64),
    Tab(u64),
    /// A session; `unbury` when it is buried and is restored before it closes.
    Session { pane: u64, unbury: bool },
    NotFound,
}

/// The steps of a Close request: windows, then tabs (digits only, `tabWithID:`), then
/// sessions, one per id; an id that names nothing is NOT_FOUND.
pub fn close_steps(r: &pb::CloseRequest, s: &Snapshot) -> Vec<CloseStep> {
    use pb::close_request::Target;
    match &r.target {
        Some(Target::Windows(w)) => w.window_ids.iter().map(|id| s.window_key(id).map_or(CloseStep::NotFound, CloseStep::Window)).collect(),
        Some(Target::Tabs(t)) => t.tab_ids.iter().map(|id| s.tab_by_id(id).map_or(CloseStep::NotFound, CloseStep::Tab)).collect(),
        Some(Target::Sessions(v)) => v.session_ids.iter().map(|id| s.pane(id).map_or(CloseStep::NotFound, |pane| CloseStep::Session { pane, unbury: s.buried(pane) })).collect(),
        None => Vec::new(),
    }
}

/// Whether Lock Layout refuses a close step, which then answers USER_DECLINED: a tab or
/// session closed without `force` in a window whose layout is locked (`_layoutLocked` in
/// `tabView:shouldCloseTabViewItem:`, `PT:7597-7600`, and `closeSessionWithConfirmation:`,
/// `PT:2738-2742`). Windows and forced closes ignore the lock (`windowShouldClose:`,
/// `removeTab:`, `closeSessionWithoutConfirmation:`). `locks` are the step's window's.
pub fn lock_refuses(step: CloseStep, force: bool, locks: &crate::winmodel::Locks) -> bool {
    use crate::winmodel::LayoutOp;
    match step {
        _ if force => false,
        CloseStep::Tab(_) => locks.forbids(LayoutOp::CloseTab),
        CloseStep::Session { .. } => locks.forbids(LayoutOp::ClosePane),
        CloseStep::Window(_) | CloseStep::NotFound => false,
    }
}

/// The CloseResponse carrying one status per step.
pub fn close_response(results: &[pb::close_response::Status]) -> pb::CloseResponse {
    pb::CloseResponse { statuses: results.iter().map(|s| *s as i32).collect() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pb::activate_request::Identifier;
    use pb::activate_response::Status as AStatus;
    use pb::close_request::{CloseSessions, CloseTabs, CloseWindows, Target};
    use pb::close_response::Status as CStatus;
    use pb::inject_response::Status as IStatus;
    use pb::send_text_response::Status as SStatus;

    /// Window key 1 "pty-A" with tabs 5 (panes 1, 2) and 6 (pane 3), window key 2 "pty-B"
    /// with tab 9 (pane 7); pane 8 buried; pane 3 exited; pane 2 active.
    fn fixture() -> Snapshot {
        Snapshot {
            windows: vec![(1, "pty-A".into(), vec![(5, vec![1, 2]), (6, vec![3])]), (2, "pty-B".into(), vec![(9, vec![7])])],
            buried: vec![8],
            exited: BTreeSet::from([3]),
            active: Some(2),
        }
    }

    #[test]
    fn targets_follow_iterm2() {
        let s = fixture();
        assert_eq!(s.pane("active"), Some(2));
        assert_eq!(s.pane("7"), Some(7));
        // A buried session is found (`includeBuriedSessions:YES`, `H:1615-1634`).
        assert_eq!(s.pane("8"), Some(8));
        for id in ["99", "x", "", "all", "07", "+7", " 7"] {
            assert_eq!(s.pane(id), None, "{id:?}");
        }
        assert_eq!(Snapshot { active: None, ..fixture() }.pane("active"), None);
        assert_eq!(s.tab_window(9), Some(2));
        assert_eq!(s.tab_window(5), Some(1));
        assert_eq!(s.tab_window(77), None);
        assert_eq!(s.window_key("pty-B"), Some(2));
        assert_eq!(s.window_key("pty-Z"), None);
        assert_eq!(s.pane_tab(3), Some(6));
        assert_eq!(s.pane_tab(8), None);
        assert!(s.buried(8) && !s.buried(1));
        assert!(s.exited(3) && !s.exited(1));
        assert_eq!(s.all_panes(), [1, 2, 3, 7]);
        assert_eq!(s.all_tabs(), [5, 6, 9]);
        assert_eq!(s.window_guids(), ["pty-A", "pty-B"]);
    }

    fn send(session: &str, suppress: Option<bool>) -> (Option<i32>, Vec<(u64, Vec<u8>, bool)>) {
        let r = pb::SendTextRequest { session: Some(session.into()), text: Some("ls\r".into()), suppress_broadcast: suppress };
        let (resp, writes) = send_text(&r, &fixture());
        (resp.status, writes)
    }

    #[test]
    fn send_text_follows_iterm2() {
        let ok = Some(SStatus::Ok as i32);
        let not_found = Some(SStatus::SessionNotFound as i32);
        assert_eq!(send("1", None), (ok, vec![(1, b"ls\r".to_vec(), true)]));
        assert_eq!(send("1", Some(true)), (ok, vec![(1, b"ls\r".to_vec(), false)]));
        assert_eq!(send("1", Some(false)), (ok, vec![(1, b"ls\r".to_vec(), true)]));
        // An exited session is not found, for SendText only.
        assert_eq!(send("3", None), (not_found, vec![]));
        assert_eq!(send("99", None), (not_found, vec![]));
        assert_eq!(send("active", None), (ok, vec![(2, b"ls\r".to_vec(), true)]));
        // "all" is every window's sessions, exited ones included (`H:2435-2463`).
        let all: Vec<_> = [1, 2, 3, 7].into_iter().map(|p| (p, b"ls\r".to_vec(), true)).collect();
        assert_eq!(send("all", None), (ok, all));
        assert_eq!(send("8", None), (ok, vec![(8, b"ls\r".to_vec(), true)]));
        // No session at all.
        let (resp, writes) = send_text(&pb::SendTextRequest { session: None, text: Some("x".into()), suppress_broadcast: None }, &fixture());
        assert_eq!((resp.status, writes), (not_found, vec![]));
    }

    #[test]
    fn inject_follows_iterm2() {
        let r = pb::InjectRequest { session_id: vec!["1".into(), "99".into(), "all".into(), "8".into()], data: Some(b"x".to_vec()) };
        let (resp, panes) = inject(&r, &fixture());
        assert_eq!(resp.status, [IStatus::Ok as i32, IStatus::SessionNotFound as i32, IStatus::Ok as i32, IStatus::Ok as i32]);
        assert_eq!(panes, [1, 1, 2, 3, 7, 8]);
        let (resp, panes) = inject(&pb::InjectRequest { session_id: vec![], data: Some(b"x".to_vec()) }, &fixture());
        assert!(resp.status.is_empty());
        assert!(panes.is_empty());
    }

    fn act(id: Option<Identifier>, front: bool, tab: bool, session: bool, app: bool) -> ActivatePlan {
        let r = pb::ActivateRequest {
            order_window_front: front.then_some(true),
            select_tab: tab.then_some(true),
            select_session: session.then_some(true),
            activate_app: app.then(pb::activate_request::App::default),
            identifier: id,
        };
        activate(&r, &fixture())
    }

    #[test]
    fn activate_follows_iterm2() {
        let tab = |s: &str| Some(Identifier::TabId(s.into()));
        let window = |s: &str| Some(Identifier::WindowId(s.into()));
        let session = |s: &str| Some(Identifier::SessionId(s.into()));
        assert_eq!(act(tab("9"), false, true, false, false), ActivatePlan { window: Some(2), tab: Some(9), select_tab: true, ..Default::default() });
        assert_eq!(act(tab("77"), false, false, false, false).error, Some(AStatus::BadIdentifier));
        assert_eq!(act(window("pty-Z"), false, false, false, false).error, Some(AStatus::BadIdentifier));
        // A bad identifier comes before app activation.
        assert_eq!(act(window("pty-Z"), false, false, false, true), ActivatePlan { error: Some(AStatus::BadIdentifier), ..Default::default() });
        assert_eq!(act(window("pty-A"), false, true, false, false), ActivatePlan { window: Some(1), error: Some(AStatus::InvalidOption), ..Default::default() });
        assert_eq!(
            act(session("2"), true, true, true, false),
            ActivatePlan { window: Some(1), tab: Some(5), pane: Some(2), raise: true, select_tab: true, select_session: true, ..Default::default() }
        );
        // A buried session is restored first; its tab is found after.
        let p = act(session("8"), false, false, false, false);
        assert_eq!((p.pane, p.unbury, p.error), (Some(8), true, None));
        assert_eq!(act(session("99"), false, false, false, false).error, Some(AStatus::BadIdentifier));
        assert_eq!(act(None, true, false, false, false).error, Some(AStatus::InvalidOption));
        assert_eq!(act(None, false, false, false, true), ActivatePlan { activate_app: true, ..Default::default() });
        // App activation precedes the option checks (`H:3011-3104`).
        let p = act(window("pty-A"), false, false, true, true);
        assert_eq!((p.activate_app, p.error), (true, Some(AStatus::InvalidOption)));
        // A step before the failing check still runs.
        let p = act(window("pty-A"), true, true, false, false);
        assert_eq!((p.raise, p.select_tab, p.error), (true, false, Some(AStatus::InvalidOption)));
    }

    fn close(target: Target) -> Vec<CloseStep> {
        close_steps(&pb::CloseRequest { force: None, target: Some(target) }, &fixture())
    }

    #[test]
    fn close_follows_iterm2() {
        let strings = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(close(Target::Windows(CloseWindows { window_ids: strings(&["pty-B", "pty-Z"]) })), [CloseStep::Window(2), CloseStep::NotFound]);
        assert_eq!(close(Target::Tabs(CloseTabs { tab_ids: strings(&["5", "77", "x", "+6"]) })), [CloseStep::Tab(5), CloseStep::NotFound, CloseStep::NotFound, CloseStep::NotFound]);
        assert_eq!(
            close(Target::Sessions(CloseSessions { session_ids: strings(&["8", "1", "99"]) })),
            [CloseStep::Session { pane: 8, unbury: true }, CloseStep::Session { pane: 1, unbury: false }, CloseStep::NotFound]
        );
        assert!(close_steps(&pb::CloseRequest { force: Some(true), target: None }, &fixture()).is_empty());
        assert_eq!(close_response(&[CStatus::Ok, CStatus::NotFound, CStatus::UserDeclined]).statuses, [0, 1, 2]);
    }

    #[test]
    fn lock_layout_refuses_like_iterm2() {
        use crate::winmodel::Locks;
        let locked = Locks { size: false, layout: true };
        let open = Locks::default();
        let sized = Locks { size: true, layout: false };
        let tab = CloseStep::Tab(5);
        let session = CloseStep::Session { pane: 1, unbury: false };
        // Tabs and sessions without force are refused in a locked window.
        assert!(lock_refuses(tab, false, &locked));
        assert!(lock_refuses(session, false, &locked));
        // Force ignores the lock; windows are never refused.
        assert!(!lock_refuses(tab, true, &locked));
        assert!(!lock_refuses(session, true, &locked));
        assert!(!lock_refuses(CloseStep::Window(1), false, &locked));
        assert!(!lock_refuses(CloseStep::NotFound, false, &locked));
        // Unlocked, or only the size locked.
        for l in [open, sized] {
            assert!(!lock_refuses(tab, false, &l));
            assert!(!lock_refuses(session, false, &l));
        }
    }
}
