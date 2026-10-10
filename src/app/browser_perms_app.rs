//! Asking for a site's permission (milestone 13x6, Task 4). The prompt is a Tandem window, not
//! part of the page (`scripts_alerts`, guarded: no default button, input ignored for a moment).
//!
//! The decisions are pure (`Asks`): requests drained in one tick are grouped, planned
//! (`browser_perms::plan`) and become `Act`s; the `App` methods only gather facts (the saved row,
//! the system switches, the pane's navigation epoch) and carry the acts out. Default deny is
//! total: every request is answered, none is left holding its deferral, and a refusal never
//! reaches the database.

use std::collections::HashMap;

use super::scripts_alerts::{self, Then};
use super::App;
use crate::browser_perms::{self as bp, Decision, PermissionType, Plan, System};
use crate::layout::PaneId;
use crate::webhost::PermAnswer;

/// Requests that wait behind another open question of the same pane (one prompt at a time per
/// pane); the rest are denied at once.
const LATER_PER_PANE: usize = 4;
const LATER_TOTAL: usize = 32;
/// Requests one open question may hold; the rest are denied at once.
const WAITING_PER_ASK: usize = 16;

/// Per-pane state for the audio and mute work (Task 7 fills it); it lives on the pane, not the view.
#[derive(Default)]
#[allow(dead_code)] // Task 7 reads these
pub(super) struct PaneState {
    pub(super) muted_by_us: bool,
    pub(super) audio_disabled: bool,
}

/// One request as the page raised it, with the page's navigation epoch at that moment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Req {
    pub pane: PaneId,
    pub token: u64,
    pub kind: i32,
    pub uri: String,
    pub epoch: u64,
}

/// A request with the facts the plan needs, gathered by the `App`.
#[derive(Clone, Debug)]
pub(super) struct Prep {
    pub req: Req,
    /// A browser pane with a view (a tool, popover or Quick Look view is never asked).
    pub browser: bool,
    pub dev_null: bool,
    /// The frame's normalized origin; None when it cannot be keyed (opaque, malformed).
    pub origin: Option<String>,
    /// The top-level page's normalized origin.
    pub top: Option<String>,
    /// Saved decisions for the type asked and for CameraAndMicrophone.
    pub saved: Vec<(PermissionType, Decision)>,
}

/// The Windows switches for the two kinds that have one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Systems {
    pub notifications: System,
    pub location: System,
}

impl Systems {
    fn of(&self, t: PermissionType) -> System {
        match t {
            PermissionType::Notification => self.notifications,
            PermissionType::Geolocation => self.location,
            _ => System::Allowed,
        }
    }
}

pub(super) struct Waiter {
    pane: PaneId,
    token: u64,
    kind: i32,
    epoch: u64,
}

/// One open question.
pub(super) struct Ask {
    pub(super) pane: PaneId,
    pub(super) origin: String,
    /// The origin as the dialog names it ("<origin> on <top origin>" for an iframe).
    shown: String,
    pub(super) kind: PermissionType,
    pub(super) dev_null: bool,
    waiting: Vec<Waiter>,
}

/// What the `App` carries out.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Act {
    Answer { pane: PaneId, token: u64, answer: PermAnswer },
    /// Open question `id` through the alert path.
    Ask { id: u64, heading: String, text: String },
}

/// An answered question: what to save and who waits.
pub(super) struct Settled {
    pub(super) origin: String,
    pub(super) t: PermissionType,
    pub(super) d: Decision,
    pub(super) save: bool,
    pub(super) dev_null: bool,
    waiters: Vec<Waiter>,
}

#[derive(Default)]
pub(super) struct Asks {
    next: u64,
    open: HashMap<u64, Ask>,
    /// Requests the events of this tick raised (`browser_event` only pushes here).
    pub(super) tick: Vec<Req>,
    /// Requests behind another open question of their pane, in order.
    later: Vec<Req>,
}

fn deny(r: &Req) -> Act {
    Act::Answer { pane: r.pane, token: r.token, answer: PermAnswer::Deny }
}

impl Asks {
    /// Groups, plans and answers `preps` (one drain's requests).
    pub(super) fn plan_flush(&mut self, preps: Vec<Prep>, systems: Systems) -> Vec<Act> {
        let mut acts = Vec::new();
        // (pane, origin) -> the kinds 1 and 2 seen together in this drain.
        let mut media: HashMap<(PaneId, String), Vec<PermissionType>> = HashMap::new();
        for p in &preps {
            if let (true, Some(o), Some(t @ (PermissionType::Camera | PermissionType::Microphone))) = (p.browser, p.origin.as_ref(), bp::from_kind(p.req.kind)) {
                media.entry((p.req.pane, o.clone())).or_default().push(t);
            }
        }
        for p in preps {
            let (Some(origin), true) = (p.origin.clone(), p.browser) else {
                acts.push(deny(&p.req));
                continue;
            };
            let Some(asked) = bp::from_kind(p.req.kind) else {
                acts.push(deny(&p.req));
                continue;
            };
            let t = match media.get(&(p.req.pane, origin.clone())) {
                Some(kinds) if matches!(asked, PermissionType::Camera | PermissionType::Microphone) => bp::group(kinds),
                _ => asked,
            };
            let saved = p.saved.iter().find(|(st, _)| *st == t).map(|(_, d)| *d);
            match bp::plan(Some(t), saved, systems.of(t)) {
                // A saved decision answers; `saves` only for the kinds a page reads synchronously.
                Plan::Answer(d) => acts.push(Act::Answer { pane: p.req.pane, token: p.req.token, answer: bp::perm_answer(t, d, true) }),
                Plan::Deny | Plan::Default => acts.push(deny(&p.req)),
                Plan::Ask(t) => self.ask(p, origin, t, &mut acts),
            }
        }
        acts
    }

    fn ask(&mut self, p: Prep, origin: String, t: PermissionType, acts: &mut Vec<Act>) {
        let shown = match &p.top {
            Some(top) if *top != origin => format!("{origin} on {top}"),
            _ => origin.clone(),
        };
        let waiter = Waiter { pane: p.req.pane, token: p.req.token, kind: p.req.kind, epoch: p.req.epoch };
        if let Some(a) = self.open.values_mut().find(|a| a.origin == origin && a.kind == t && a.shown == shown && a.dev_null == p.dev_null) {
            if a.waiting.len() < WAITING_PER_ASK {
                a.waiting.push(waiter);
            } else {
                acts.push(deny(&p.req));
            }
            return;
        }
        // One prompt at a time per pane: a different question waits its turn, a bounded number.
        if self.open.values().any(|a| a.pane == p.req.pane) {
            if self.later.len() < LATER_TOTAL && self.later.iter().filter(|r| r.pane == p.req.pane).count() < LATER_PER_PANE {
                self.later.push(p.req);
            } else {
                acts.push(deny(&p.req));
            }
            return;
        }
        self.next += 1;
        let id = self.next;
        let clean = |s: &str| crate::inert_files::display_text(s, 300, false);
        let text = clean(&t.request_message(&shown));
        self.open.insert(id, Ask { pane: p.req.pane, origin, shown, kind: t, dev_null: p.dev_null, waiting: vec![waiter] });
        acts.push(Act::Ask { id, heading: t.dialog_title().to_string(), text });
    }

    /// Question `id` was answered by button `index` (None: closed, or never shown).
    pub(super) fn answered(&mut self, id: u64, index: Option<usize>) -> Option<Settled> {
        let a = self.open.remove(&id)?;
        let (d, save) = bp::answer_of(index);
        Some(Settled { origin: a.origin, t: a.kind, d, save, dev_null: a.dev_null, waiters: a.waiting })
    }

    /// The requests that were waiting behind `pane`'s question and can ask now.
    pub(super) fn take_later(&mut self, pane: PaneId) -> Vec<Req> {
        if self.open.values().any(|a| a.pane == pane) {
            return Vec::new();
        }
        let (mine, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut self.later).into_iter().partition(|r| r.pane == pane);
        self.later = rest;
        mine
    }

    /// `pane` is closing: its requests are forgotten (the host completes a dropped view's requests
    /// refused).
    pub(super) fn pane_gone(&mut self, pane: PaneId) {
        self.later.retain(|r| r.pane != pane);
        for a in self.open.values_mut() {
            a.waiting.retain(|w| w.pane != pane);
        }
        self.tick.retain(|r| r.pane != pane);
    }
}

/// The answers for a settled question. `stored` says the decision reached the database (a failed
/// save, a full table, blocks this request: never a grant). `epoch_of` is each pane's current
/// navigation epoch, None when it has no view (nothing to answer: the host refuses on drop).
pub(super) fn settle(s: &Settled, stored: bool, epoch_of: &dyn Fn(PaneId) -> Option<u64>) -> Vec<Act> {
    let failed = s.save && !stored;
    let (d, saved) = if failed { (Decision::Denied, false) } else { (s.d, s.save) };
    let mut acts = Vec::new();
    for w in &s.waiters {
        let Some(now) = epoch_of(w.pane) else { continue };
        // Another page now: the answer was for the one that asked.
        let answer = if now != w.epoch || failed { PermAnswer::Deny } else { bp::perm_answer(s.t, d, saved) };
        let _ = w.kind;
        acts.push(Act::Answer { pane: w.pane, token: w.token, answer });
    }
    acts
}

/// The Windows privacy switches (read only; a missing value is None).
#[derive(Default)]
struct Privacy {
    toast: Option<u32>,
    machine: Option<String>,
    user: Option<String>,
    non_packaged: Option<String>,
}

#[cfg(windows)]
fn read_privacy_values() -> Privacy {
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegGetValueW};
    let wide = |s: &str| s.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
    let dword = |root: HKEY, key: &str, name: &str| -> Option<u32> {
        let (key, name) = (wide(key), wide(name));
        let (mut data, mut bytes) = (0u32, 4u32);
        let rc = unsafe { RegGetValueW(root, key.as_ptr(), name.as_ptr(), RRF_RT_REG_DWORD, std::ptr::null_mut(), &mut data as *mut u32 as *mut _, &mut bytes) };
        (rc == ERROR_SUCCESS).then_some(data)
    };
    let string = |root: HKEY, key: &str, name: &str| -> Option<String> {
        let (key, name) = (wide(key), wide(name));
        let mut buf = vec![0u16; 64];
        let mut bytes = (buf.len() * 2) as u32;
        let rc = unsafe { RegGetValueW(root, key.as_ptr(), name.as_ptr(), RRF_RT_REG_SZ, std::ptr::null_mut(), buf.as_mut_ptr() as *mut _, &mut bytes) };
        if rc != ERROR_SUCCESS {
            return None;
        }
        let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        Some(String::from_utf16_lossy(&buf[..end]))
    };
    const STORE: &str = "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\CapabilityAccessManager\\ConsentStore\\location";
    Privacy {
        toast: dword(HKEY_CURRENT_USER, "Software\\Microsoft\\Windows\\CurrentVersion\\PushNotifications", "ToastEnabled"),
        machine: string(HKEY_LOCAL_MACHINE, STORE, "Value"),
        user: string(HKEY_CURRENT_USER, STORE, "Value"),
        non_packaged: string(HKEY_CURRENT_USER, &format!("{STORE}\\NonPackaged"), "Value"),
    }
}

#[cfg(not(windows))]
fn read_privacy_values() -> Privacy {
    Privacy::default()
}

fn systems_now() -> Systems {
    let p = read_privacy_values();
    Systems { notifications: bp::notifications_system(p.toast), location: bp::location_system(p.machine.as_deref(), p.user.as_deref(), p.non_packaged.as_deref()) }
}

impl App {
    /// A page asked for a permission: held until `flush_permission_requests` (WebView2 may raise one
    /// camera and microphone request as two events; grouping works across one drain).
    pub(super) fn browser_permission_request(&mut self, pane: PaneId, token: u64, kind: i32, uri: &str) {
        let epoch = self.browser_views.get(&pane).map_or(0, |b| b.reader.epoch);
        self.perm_asks.tick.push(Req { pane, token, kind, uri: uri.to_string(), epoch });
    }

    /// Whether Windows lets desktop programs use `t` (never prompted).
    #[allow(dead_code)] // the flush reads both switches once; Task 5 asks per kind
    pub(super) fn permission_system(&self, t: PermissionType) -> System {
        systems_now().of(t)
    }

    fn perm_prep(&mut self, req: Req) -> Prep {
        let browser = self.is_browser_pane(req.pane) && self.browser_views.get(&req.pane).is_some_and(|b| b.view.is_some());
        let dev_null = self.pane_dev_null(req.pane);
        let origin = bp::from_kind(req.kind).and_then(|_| bp::normalize_origin(&req.uri));
        let top = self.panes.get(&req.pane).and_then(|p| p.browser_io()).and_then(|io| bp::normalize_origin(&io.url));
        let mut saved = Vec::new();
        if let (true, Some(o), Some(asked)) = (browser, origin.as_deref(), bp::from_kind(req.kind)) {
            let mut types = vec![asked];
            if matches!(asked, PermissionType::Camera | PermissionType::Microphone) {
                types.push(PermissionType::CameraAndMicrophone);
            }
            if let Some(db) = self.browser_db(dev_null) {
                for t in types {
                    if let Some(row) = db.get_permission(o, t) {
                        saved.push((t, row.decision));
                    }
                }
            }
        }
        Prep { req, browser, dev_null, origin, top, saved }
    }

    /// Once per drain: groups, plans and answers the requests the events raised.
    pub(super) fn flush_permission_requests(&mut self) {
        if self.perm_asks.tick.is_empty() {
            return;
        }
        let reqs = std::mem::take(&mut self.perm_asks.tick);
        let preps: Vec<Prep> = reqs.into_iter().map(|r| self.perm_prep(r)).collect();
        // The registry is read only when a notification or location request is among them.
        let systems = if preps.iter().any(|p| matches!(p.req.kind, 3 | 4)) { systems_now() } else { Systems { notifications: System::Allowed, location: System::Allowed } };
        let acts = self.perm_asks.plan_flush(preps, systems);
        self.perm_acts(acts);
    }

    fn perm_acts(&mut self, acts: Vec<Act>) {
        for act in acts {
            match act {
                Act::Answer { pane, token, answer } => {
                    if let Some(v) = self.browser_views.get(&pane).and_then(|b| b.view.as_ref()) {
                        v.answer_permission(token, answer);
                    }
                }
                Act::Ask { id, heading, text } => {
                    let ask = scripts_alerts::ask(&heading, &text, &[bp::ALLOW, bp::BLOCK], None, crate::warnings::Kind::Persistent, Then::BrowserPermission(id));
                    self.ask_scripts(ask);
                }
            }
        }
    }

    /// Question `ask` was answered (None: closed or never shown, which blocks and saves nothing).
    pub(super) fn browser_permission_answered(&mut self, ask: u64, index: Option<usize>) {
        let Some(s) = self.perm_asks.answered(ask, index) else { return };
        let stored = !s.save || self.browser_db(s.dev_null).is_some_and(|db| db.save_permission(&s.origin, s.t, s.d, super::warnings_app::unix_now()));
        let epochs: HashMap<PaneId, u64> = self.browser_views.iter().filter(|(_, b)| b.view.is_some()).map(|(p, b)| (*p, b.reader.epoch)).collect();
        let acts = settle(&s, stored, &|p| epochs.get(&p).copied());
        let pane = s.waiters.first().map(|w| w.pane);
        self.perm_acts(acts);
        // The pane's next question, if its requests were waiting behind this one.
        let mut panes: Vec<PaneId> = s.waiters.iter().map(|w| w.pane).collect();
        panes.extend(pane);
        panes.sort();
        panes.dedup();
        for p in panes {
            let again = self.perm_asks.take_later(p);
            self.perm_asks.tick.extend(again);
        }
        self.flush_permission_requests();
    }

    /// A browser pane is closing: its requests are forgotten.
    pub(super) fn browser_permission_pane_gone(&mut self, pane: PaneId) {
        self.perm_asks.pane_gone(pane);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use PermissionType::*;

    const SYS: Systems = Systems { notifications: System::Allowed, location: System::Allowed };

    fn req(pane: PaneId, token: u64, kind: i32, uri: &str) -> Req {
        Req { pane, token, kind, uri: uri.into(), epoch: 7 }
    }

    fn prep(r: Req, saved: &[(PermissionType, Decision)]) -> Prep {
        Prep { origin: if bp::from_kind(r.kind).is_some() { bp::normalize_origin(&r.uri) } else { None }, top: Some("https://a.b".into()), req: r, browser: true, dev_null: false, saved: saved.to_vec() }
    }

    fn asked(acts: &[Act]) -> Vec<(u64, String, String)> {
        acts.iter().filter_map(|a| if let Act::Ask { id, heading, text } = a { Some((*id, heading.clone(), text.clone())) } else { None }).collect()
    }

    fn answers(acts: &[Act]) -> Vec<(u64, PermAnswer)> {
        acts.iter().filter_map(|a| if let Act::Answer { token, answer, .. } = a { Some((*token, *answer)) } else { None }).collect()
    }

    fn alive(_: PaneId) -> Option<u64> {
        Some(7)
    }

    #[test]
    fn browser_permission_flow_follows_iterm2() {
        let mut asks = Asks::default();
        let p: PaneId = 1;
        let acts = asks.plan_flush(vec![prep(req(p, 1, 4, "https://a.b/x"), &[])], SYS);
        let q = asked(&acts);
        assert_eq!(q.len(), 1);
        assert_eq!((q[0].1.as_str(), q[0].2.as_str()), ("Allow Notifications", "The website https://a.b wants to send you notifications."));
        // The same origin and kind while it is open waits for it: no second question.
        let acts = asks.plan_flush(vec![prep(req(p, 2, 4, "https://a.b/y"), &[])], SYS);
        assert!(acts.is_empty());
        let s = asks.answered(q[0].0, Some(0)).unwrap();
        assert_eq!((s.origin.as_str(), s.t, s.d, s.save), ("https://a.b", Notification, Decision::Granted, true));
        let acts = settle(&s, true, &alive);
        assert_eq!(answers(&acts), vec![(1, PermAnswer::State { state: 1, saves: true }), (2, PermAnswer::State { state: 1, saves: true })]);
        // A saved Denied answers at once, with no question.
        let acts = asks.plan_flush(vec![prep(req(p, 3, 4, "https://a.b/"), &[(Notification, Decision::Denied)])], SYS);
        assert_eq!(answers(&acts), vec![(3, PermAnswer::State { state: 2, saves: true })]);
        assert!(asked(&acts).is_empty());
        // Camera (2) and Microphone (1) drained together: one question for both.
        let acts = asks.plan_flush(vec![prep(req(2, 4, 2, "https://c.d/"), &[]), prep(req(2, 5, 1, "https://c.d/"), &[])], SYS);
        let q = asked(&acts);
        assert_eq!(q.len(), 1);
        assert_eq!(q[0].1, "Allow Camera and Microphone");
        let s = asks.answered(q[0].0, Some(0)).unwrap();
        assert_eq!(s.t, CameraAndMicrophone);
        let both = settle(&s, true, &alive);
        assert_eq!(answers(&both), vec![(4, PermAnswer::State { state: 1, saves: false }), (5, PermAnswer::State { state: 1, saves: false })]);
        // With a flush between them they ask twice.
        let mut asks = Asks::default();
        let a = asks.plan_flush(vec![prep(req(3, 6, 2, "https://c.d/"), &[])], SYS);
        let b = asks.plan_flush(vec![prep(req(4, 7, 1, "https://c.d/"), &[])], SYS);
        assert_eq!((asked(&a)[0].1.as_str(), asked(&b)[0].1.as_str()), ("Allow Camera", "Allow Microphone"));
        // A saved CameraAndMicrophone row does not answer a Camera request (exact type).
        let mut asks = Asks::default();
        let acts = asks.plan_flush(vec![prep(req(5, 8, 2, "https://c.d/"), &[(CameraAndMicrophone, Decision::Granted)])], SYS);
        assert_eq!(asked(&acts)[0].1, "Allow Camera");
        // An opaque origin is denied with no question.
        let acts = asks.plan_flush(vec![prep(req(6, 9, 4, "data:text/html,x"), &[])], SYS);
        assert_eq!(answers(&acts), vec![(9, PermAnswer::Deny)]);
        assert!(asked(&acts).is_empty());
        // Notifications off in Windows: denied, no question.
        let off = Systems { notifications: System::Denied, location: System::Allowed };
        let acts = asks.plan_flush(vec![prep(req(7, 10, 4, "https://a.b/"), &[])], off);
        assert_eq!(answers(&acts), vec![(10, PermAnswer::Deny)]);
    }

    #[test]
    fn closed_permission_alert_blocks_and_saves_nothing() {
        let mut asks = Asks::default();
        let p: PaneId = 1;
        let q = asked(&asks.plan_flush(vec![prep(req(p, 1, 4, "https://a.b/"), &[]), prep(req(p, 2, 4, "https://a.b/"), &[])], SYS));
        assert_eq!(q.len(), 1);
        let s = asks.answered(q[0].0, None).unwrap();
        assert!(!s.save, "a closed window saves nothing");
        let acts = settle(&s, true, &alive);
        assert_eq!(answers(&acts), vec![(1, PermAnswer::State { state: 2, saves: false }), (2, PermAnswer::State { state: 2, saves: false })], "both once, no third");
        assert!(asks.answered(q[0].0, None).is_none(), "an answered question answers once");
        // Asking again asks again.
        assert_eq!(asked(&asks.plan_flush(vec![prep(req(p, 3, 4, "https://a.b/"), &[])], SYS)).len(), 1);
        // The source routes a close and a dropped ask to the same blocking path.
        let src = include_str!("scripts_alerts.rs");
        assert!(src.contains("Then::BrowserPermission(id) => self.browser_permission_answered(id, None)"));
    }

    #[test]
    fn refused_requests_are_denied_never_untouched() {
        let mut asks = Asks::default();
        // Clipboard read (6) from a browser pane, and any kind from a tool view.
        let mut tool = prep(req(2, 2, 4, "https://a.b/"), &[]);
        tool.browser = false;
        let acts = asks.plan_flush(vec![prep(req(1, 1, 6, "https://a.b/"), &[]), tool], SYS);
        assert_eq!(answers(&acts), vec![(1, PermAnswer::Deny), (2, PermAnswer::Deny)]);
        assert!(asked(&acts).is_empty());
    }

    #[test]
    fn one_prompt_at_a_time_per_pane_and_the_queue_is_bounded() {
        let mut asks = Asks::default();
        let p: PaneId = 1;
        let first = asked(&asks.plan_flush(vec![prep(req(p, 1, 4, "https://a.b/"), &[])], SYS));
        // Different origins from the same pane wait behind it; past the cap they are denied.
        let reqs: Vec<Prep> = (0..8).map(|i| prep(req(p, 10 + i, 4, &format!("https://s{i}.example/")), &[])).collect();
        let acts = asks.plan_flush(reqs, SYS);
        assert!(asked(&acts).is_empty(), "no second window for the pane");
        assert_eq!(answers(&acts).len(), 8 - LATER_PER_PANE, "the rest are denied at once");
        assert!(answers(&acts).iter().all(|(_, a)| *a == PermAnswer::Deny));
        // Another pane is not held up.
        assert_eq!(asked(&asks.plan_flush(vec![prep(req(2, 30, 4, "https://z.example/"), &[])], SYS)).len(), 1);
        // After the first is answered the pane's waiting requests are released.
        assert!(asks.answered(first[0].0, Some(1)).is_some());
        let again = asks.take_later(p);
        assert_eq!(again.len(), LATER_PER_PANE);
        // A question with too many waiters denies the excess.
        let mut asks = Asks::default();
        let reqs: Vec<Prep> = (0..WAITING_PER_ASK as u64 + 3).map(|i| prep(req(p, i, 4, "https://a.b/"), &[])).collect();
        let acts = asks.plan_flush(reqs, SYS);
        assert_eq!(answers(&acts).len(), 3);
    }

    #[test]
    fn a_failed_save_blocks_and_a_stale_page_is_denied() {
        let mut asks = Asks::default();
        let p: PaneId = 1;
        let q = asked(&asks.plan_flush(vec![prep(req(p, 1, 4, "https://a.b/"), &[])], SYS));
        let s = asks.answered(q[0].0, Some(0)).unwrap();
        // The table is full: Allow is never granted.
        assert_eq!(answers(&settle(&s, false, &alive)), vec![(1, PermAnswer::Deny)]);
        // The page moved on (another epoch): the answer is for the one that asked.
        let q = asked(&asks.plan_flush(vec![prep(req(p, 2, 4, "https://a.b/"), &[])], SYS));
        let s = asks.answered(q[0].0, Some(0)).unwrap();
        assert_eq!(answers(&settle(&s, true, &|_| Some(8))), vec![(2, PermAnswer::Deny)]);
        // A pane with no view is not answered (its view's drop refuses).
        assert!(settle(&s, true, &|_| None).is_empty());
    }

    #[test]
    fn the_prompt_names_punycode_and_the_top_page_of_an_iframe() {
        let mut asks = Asks::default();
        let mut pr = prep(req(1, 1, 3, "https://xn--bcher-kva.example/"), &[]);
        pr.top = Some("https://top.example".into());
        let q = asked(&asks.plan_flush(vec![pr], SYS));
        assert_eq!(q[0].1, "Allow Location");
        assert_eq!(q[0].2, "The website https://xn--bcher-kva.example on https://top.example wants to know your location.");
        // The stored key is the frame's own origin.
        let s = asks.answered(q[0].0, Some(0)).unwrap();
        assert_eq!(s.origin, "https://xn--bcher-kva.example");
        // Unicode never reaches a prompt: such a host has no origin and is denied.
        assert_eq!(bp::normalize_origin("https://b\u{fc}cher.example/"), None);
    }

    #[test]
    fn the_flow_is_wired() {
        let ev = include_str!("browser_app.rs");
        assert!(ev.contains("self.browser_permission_request(pane, *token, *kind, uri)"));
        let tick = include_str!("webhost_app.rs");
        assert!(tick.contains("self.flush_permission_requests()"));
        let me = include_str!("browser_perms_app.rs");
        assert!(me.contains("RRF_RT_REG_DWORD") && !me.contains(concat!("RegSet", "Value")), "registry reads only");
    }
}
