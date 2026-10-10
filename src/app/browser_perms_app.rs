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
use crate::perm_queue::{Out, ProfileQueue};
use crate::webhost::{PermAnswer, ViewId};
use crate::webhost_win::NoteReport;

/// Requests that wait behind another open question of the same pane (one prompt at a time per
/// pane); the rest are denied at once.
const LATER_PER_PANE: usize = 4;
const LATER_TOTAL: usize = 32;
/// Requests one open question may hold; the rest are denied at once.
const WAITING_PER_ASK: usize = 16;
/// After a prompt is closed unanswered, the same origin and kind in the same pane is denied
/// without a window for this long (B2): a page cannot re-raise the window by looping.
const COOLDOWN_MS: u64 = 60_000;
const COOLDOWNS_KEPT: usize = 256;
/// A granted origin may show one notification this often; a flood from one site is dropped.
const WEB_NOTE_EVERY: std::time::Duration = std::time::Duration::from_secs(2);

/// Milliseconds on a monotonic clock (the cooldown's).
fn now_ms() -> u64 {
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    START.get_or_init(std::time::Instant::now).elapsed().as_millis() as u64
}

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
    /// The pane is shown (this window's active tab, not buried or hidden): only then may a window
    /// open for it (B1).
    pub visible: bool,
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
    /// A saved grant of a mirrored kind: written into the profile through the store's one write
    /// queue (`perm_queue`), and the request answered only once that write came back.
    Mirror { pane: PaneId, token: u64, epoch: u64, kind: i32, origin: String, answer: PermAnswer },
    /// Open question `id` through the alert path.
    Ask { id: u64, heading: String, text: String },
    /// Question `id` has no page left waiting (it navigated or closed): the window goes, queued or
    /// showing, and nothing is saved.
    Withdraw { id: u64 },
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
    /// Requests behind another open question of their pane, or for a pane that is not shown, in order.
    later: Vec<Req>,
    /// The clock the cooldown reads (milliseconds; the `App` sets it before each plan).
    pub(super) now_ms: u64,
    /// (pane, origin, type, dev null) -> when the cooldown ends.
    cooldown: HashMap<(PaneId, String, PermissionType, bool), u64>,
}

fn deny(r: &Req) -> Act {
    Act::Answer { pane: r.pane, token: r.token, answer: PermAnswer::Deny }
}

/// The act for an answer of type `t` on `origin`: an answer that `saves` (a saved Allow of a
/// mirrored kind) is mirrored first, anything else is answered at once.
fn answer_act(pane: PaneId, token: u64, epoch: u64, t: PermissionType, origin: &str, answer: PermAnswer) -> Act {
    match (answer, bp::kinds_of(t).first()) {
        (PermAnswer::State { saves: true, .. }, Some(&kind)) => Act::Mirror { pane, token, epoch, kind, origin: origin.to_string(), answer },
        _ => Act::Answer { pane, token, answer },
    }
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
                Plan::Answer(d) => acts.push(answer_act(p.req.pane, p.req.token, p.req.epoch, t, &origin, bp::perm_answer(t, d, true))),
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
        let waiter = Waiter { pane: p.req.pane, token: p.req.token, epoch: p.req.epoch };
        // Closed unanswered a moment ago: no window, no grant (B2).
        if self.cooldown.get(&(p.req.pane, origin.clone(), t, p.dev_null)).is_some_and(|until| self.now_ms < *until) {
            acts.push(deny(&p.req));
            return;
        }
        if let Some(a) = self.open.values_mut().find(|a| a.origin == origin && a.kind == t && a.shown == shown && a.dev_null == p.dev_null) {
            if a.waiting.len() < WAITING_PER_ASK {
                a.waiting.push(waiter);
            } else {
                acts.push(deny(&p.req));
            }
            return;
        }
        // One prompt at a time per pane, and none for a pane the person cannot see: the request
        // waits its turn, a bounded number, one per origin so a crowd cannot push another site out.
        if !p.visible || self.open.values().any(|a| a.pane == p.req.pane) {
            let same_origin = |r: &Req| bp::normalize_origin(&r.uri).as_deref() == Some(origin.as_str());
            let mine = self.later.iter().filter(|r| r.pane == p.req.pane);
            if self.later.len() < LATER_TOTAL && mine.clone().count() < LATER_PER_PANE && !mine.clone().any(same_origin) {
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
        if index.is_none() {
            if self.cooldown.len() >= COOLDOWNS_KEPT {
                let now = self.now_ms;
                self.cooldown.retain(|_, until| now < *until);
                if self.cooldown.len() >= COOLDOWNS_KEPT {
                    self.cooldown.clear();
                }
            }
            self.cooldown.insert((a.pane, a.origin.clone(), a.kind, a.dev_null), self.now_ms + COOLDOWN_MS);
        }
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

    /// Panes whose requests wait in the queue with nothing open for them and that are shown now.
    pub(super) fn ready_panes(&self, shown: &dyn Fn(PaneId) -> bool) -> Vec<PaneId> {
        let mut panes: Vec<PaneId> = self.later.iter().map(|r| r.pane).filter(|p| shown(*p) && !self.open.values().any(|a| a.pane == *p)).collect();
        panes.sort();
        panes.dedup();
        panes
    }

    /// `pane` is closing: its requests are forgotten (the host completes a dropped view's requests
    /// refused). A question nobody waits on any more is withdrawn.
    pub(super) fn pane_gone(&mut self, pane: PaneId) -> Vec<Act> {
        self.later.retain(|r| r.pane != pane);
        for a in self.open.values_mut() {
            a.waiting.retain(|w| w.pane != pane);
        }
        self.tick.retain(|r| r.pane != pane);
        self.withdraw_empty()
    }

    /// A page navigated or a pane went: every waiter whose pane is gone (None) is forgotten, every
    /// one whose page changed (another epoch) is denied, and a question with no waiter left is
    /// withdrawn without saving. Queued requests of a changed page are denied too.
    pub(super) fn dismiss(&mut self, epoch_of: &dyn Fn(PaneId) -> Option<u64>) -> Vec<Act> {
        let mut acts = Vec::new();
        for a in self.open.values_mut() {
            a.waiting.retain(|w| match epoch_of(w.pane) {
                None => false,
                Some(now) if now != w.epoch => {
                    acts.push(Act::Answer { pane: w.pane, token: w.token, answer: PermAnswer::Deny });
                    false
                }
                Some(_) => true,
            });
        }
        let (stale, kept): (Vec<Req>, Vec<Req>) = std::mem::take(&mut self.later).into_iter().partition(|r| epoch_of(r.pane).is_some_and(|now| now != r.epoch));
        self.later = kept;
        acts.extend(stale.iter().map(deny));
        acts.extend(self.withdraw_empty());
        acts
    }

    fn withdraw_empty(&mut self) -> Vec<Act> {
        let mut ids: Vec<u64> = self.open.iter().filter(|(_, a)| a.waiting.is_empty()).map(|(id, _)| *id).collect();
        ids.sort();
        for id in &ids {
            self.open.remove(id);
        }
        ids.into_iter().map(|id| Act::Withdraw { id }).collect()
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
        acts.push(answer_act(w.pane, w.token, w.epoch, s.t, &s.origin, answer));
    }
    acts
}

impl Settled {
    /// Whether any page that asked is still on the page it asked from.
    pub(super) fn any_live(&self, epoch_of: &dyn Fn(PaneId) -> Option<u64>) -> bool {
        self.waiters.iter().any(|w| epoch_of(w.pane) == Some(w.epoch))
    }
}

/// What a change to the saved permissions of one origin does to WebView2 (Task 5), by value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Op {
    /// The page's Revoke: the row goes, the profile state is reset, pages using the origin reload.
    Revoke(PermissionType),
    /// Revoke all of an origin: the types of the rows it held.
    RevokeAll(Vec<PermissionType>),
    /// The toolbar's Reset: the row goes, the profile state is reset, nothing reloads (`BVC:1075-1079`).
    Reset(PermissionType),
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct Effects {
    pub states: Vec<(i32, String, i32)>,
    pub reload: bool,
}

/// The profile resets and the reload an `Op` on `origin` needs. Only mirrored kinds (3 and 4) are
/// ever reset; media is never mirrored (Task 1).
pub(super) fn effects(op: &Op, origin: &str) -> Effects {
    let types: Vec<PermissionType> = match op {
        Op::Revoke(t) | Op::Reset(t) => vec![*t],
        Op::RevokeAll(ts) => ts.clone(),
    };
    let mut states: Vec<(i32, String, i32)> = Vec::new();
    for t in types {
        for &k in bp::kinds_of(t) {
            let e = (k, origin.to_string(), bp::STATE_DEFAULT);
            if !states.contains(&e) {
                states.push(e);
            }
        }
    }
    Effects { states, reload: !matches!(op, Op::Reset(_)) }
}

/// How a pane is brought up to date after a revoke: a main origin that matches reloads at once,
/// every other pane runs the sweep.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Reload {
    Now,
    Sweep,
}

pub(super) fn reload_plan(panes: &[(PaneId, String)], origin: &str) -> Vec<(PaneId, Reload)> {
    panes
        .iter()
        .map(|(p, url)| (*p, if bp::page_uses_origin(url, None, origin) { Reload::Now } else { Reload::Sweep }))
        .collect()
}

/// Whether a browser pane's own page may load now: never while the launch restores windows, and
/// never before its store's profile permissions were synced from the table (13x6 Task 5 fix: a
/// stale profile grant would otherwise work for the first page with no question and no row).
pub(super) fn page_may_load(restoring: bool, synced: bool) -> bool {
    !restoring && synced
}

/// The plan for a profile read: the table decides kinds 1 to 4. An unreadable table wants
/// nothing, so no grant of those kinds survives (fail closed).
pub(super) fn sync_for(states: &[(i32, String, i32)], rows_newest_first: Option<&[(String, PermissionType, Decision)]>) -> Vec<(i32, String, i32)> {
    let wanted = match rows_newest_first {
        Some(rows) => {
            let oldest_first: Vec<_> = rows.iter().rev().cloned().collect();
            bp::mirror_entries(&oldest_first)
        }
        None => Vec::new(),
    };
    bp::sync_plan(states, &wanted)
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
        let visible = self.browser_pane_shown(req.pane);
        Prep { req, browser, dev_null, origin, top, saved, visible }
    }

    /// Once per drain: groups, plans and answers the requests the events raised. Requests that
    /// waited for their pane to be shown are planned again once it is.
    pub(super) fn flush_permission_requests(&mut self) {
        let shown: HashMap<PaneId, bool> = self.browser_views.keys().map(|p| (*p, self.browser_pane_shown(*p))).collect();
        for pane in self.perm_asks.ready_panes(&|p| shown.get(&p).copied().unwrap_or(false)) {
            let again = self.perm_asks.take_later(pane);
            self.perm_asks.tick.extend(again);
        }
        if self.perm_asks.tick.is_empty() {
            return;
        }
        self.perm_asks.now_ms = now_ms();
        let reqs = std::mem::take(&mut self.perm_asks.tick);
        let preps: Vec<Prep> = reqs.into_iter().map(|r| self.perm_prep(r)).collect();
        // The registry is read only when a notification or location request is among them.
        let systems = if preps.iter().any(|p| matches!(p.req.kind, 3 | 4)) { systems_now() } else { Systems { notifications: System::Allowed, location: System::Allowed } };
        let acts = self.perm_asks.plan_flush(preps, systems);
        self.perm_acts(acts);
    }

    /// A page navigated or a pane closed: questions nobody waits on go, unsaved (B1, I3).
    pub(super) fn browser_permission_dismiss(&mut self) {
        let epochs: HashMap<PaneId, u64> = self.browser_views.iter().filter(|(_, b)| b.view.is_some()).map(|(p, b)| (*p, b.reader.epoch)).collect();
        let acts = self.perm_asks.dismiss(&|p| epochs.get(&p).copied());
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
                Act::Mirror { pane, token, epoch, kind, origin, answer } => {
                    let dev_null = self.pane_dev_null(pane);
                    self.perm_queue(dev_null).mirror_then_answer(kind, &origin, pane, token, epoch, answer);
                    self.perm_pump(dev_null);
                }
                Act::Withdraw { id } => self.browser_permission_withdraw(id),
                Act::Ask { id, heading, text } => {
                    let ask = scripts_alerts::ask(&heading, &text, &[bp::ALLOW, bp::BLOCK], None, crate::warnings::Kind::Persistent, Then::BrowserPermission(id));
                    self.ask_scripts(ask);
                }
            }
        }
    }

    /// Question `ask` was answered (None: closed or never shown, which blocks and saves nothing).
    pub(super) fn browser_permission_answered(&mut self, ask: u64, index: Option<usize>) {
        self.perm_asks.now_ms = now_ms();
        let Some(s) = self.perm_asks.answered(ask, index) else { return };
        let epochs: HashMap<PaneId, u64> = self.browser_views.iter().filter(|(_, b)| b.view.is_some()).map(|(p, b)| (*p, b.reader.epoch)).collect();
        // A late click for a page that has gone saves nothing (I3).
        let mut s = s;
        s.save = s.save && s.any_live(&|p| epochs.get(&p).copied());
        let stored = !s.save || self.browser_db(s.dev_null).is_some_and(|db| db.save_permission(&s.origin, s.t, s.d, super::warnings_app::unix_now()));
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

    /// The write queue of a store's profile (`dev_null` picks it).
    fn perm_queue(&mut self, dev_null: bool) -> &mut ProfileQueue {
        &mut self.perm_queues[usize::from(dev_null)]
    }

    /// Any live view of the store (`dev_null` picks it): where profile calls go. The lowest pane id
    /// wins, so the choice does not hop between views.
    fn store_view_id(&self, dev_null: bool) -> Option<ViewId> {
        let mut views: Vec<(PaneId, ViewId)> = self
            .browser_views
            .iter()
            .filter(|(p, b)| b.view.is_some() && self.pane_dev_null(**p) == dev_null)
            .map(|(p, b)| (*p, b.id))
            .collect();
        views.sort();
        views.first().map(|(_, v)| *v)
    }

    fn view_of_id(&self, view: ViewId) -> Option<&crate::webhost_win::WebView> {
        self.browser_views.values().find(|b| b.id == view).and_then(|b| b.view.as_ref())
    }

    /// Runs the store's write queue as far as it goes: answers and reloads at its head, then the
    /// next write or read on a live view of the store (none live: it waits for the next view).
    pub(super) fn perm_pump(&mut self, dev_null: bool) {
        loop {
            let view = self.store_view_id(dev_null);
            let outs = self.perm_queue(dev_null).step(view);
            if outs.is_empty() {
                return;
            }
            self.perm_outs(dev_null, outs);
        }
    }

    fn perm_outs(&mut self, dev_null: bool, outs: Vec<Out>) {
        for out in outs {
            match out {
                Out::Write { view, seq, kind, origin, state } => match self.view_of_id(view) {
                    Some(v) => v.set_permission_state(seq, kind, &origin, state),
                    None => {
                        let more = self.perm_queue(dev_null).view_gone(view);
                        self.perm_outs(dev_null, more);
                    }
                },
                Out::Read { view } => match self.view_of_id(view) {
                    Some(v) => v.read_permission_states(),
                    None => {
                        let more = self.perm_queue(dev_null).view_gone(view);
                        self.perm_outs(dev_null, more);
                    }
                },
                // The page that asked must still be the page it asked from.
                Out::Answer { pane, token, epoch, answer } => {
                    if let Some(b) = self.browser_views.get(&pane) {
                        let answer = if b.reader.epoch == epoch { answer } else { PermAnswer::Deny };
                        if let Some(v) = &b.view {
                            v.answer_permission(token, answer);
                        }
                    }
                }
                Out::Reload { origin } => self.reload_panes_using(dev_null, &origin),
                Out::Synced => self.browser_release_waiting_loads(Some(dev_null)),
                Out::GaveUp => {
                    crate::dlog!("webhost: the profile permission sync gave up after {} tries; pages wait for the next view", crate::perm_queue::SYNC_TRIES);
                    self.banners.set("window", "Browser pages are waiting: Tandem could not check this browser's site permissions. Open the page again to retry.");
                }
            }
        }
    }

    /// Whether `pane`'s store has synced its profile permissions from the table (its pages may load).
    pub(super) fn perm_synced(&self, pane: PaneId) -> bool {
        self.perm_queues[usize::from(self.pane_dev_null(pane))].synced()
    }

    /// A profile write came back (`Event::PermissionWritten`).
    pub(super) fn perm_written(&mut self, pane: PaneId, view: ViewId, seq: u64, ok: bool) {
        let dev_null = self.pane_dev_null(pane);
        let outs = self.perm_queue(dev_null).written(view, seq, ok);
        self.perm_outs(dev_null, outs);
        self.perm_pump(dev_null);
    }

    /// A web view went away: what its profile queue had in flight on it is sent again elsewhere.
    pub(super) fn perm_view_gone(&mut self, view: ViewId) {
        for dev_null in [false, true] {
            let outs = self.perm_queue(dev_null).view_gone(view);
            self.perm_outs(dev_null, outs);
            self.perm_pump(dev_null);
        }
    }

    /// The store's environment was lost: its queue goes (held answers are refused).
    pub(super) fn perm_store_lost(&mut self, dev_null: bool) {
        let outs = self.perm_queue(dev_null).lost();
        self.perm_outs(dev_null, outs);
    }

    /// At each view of a store: starts its sync unless it runs or is done (`perm_queue::Sync`).
    /// Nothing is stored by WebView2 itself, so what the profile holds from an earlier run is stale
    /// until a complete read's plan has rewritten it from the table.
    pub(super) fn sync_permission_states(&mut self, dev_null: bool) {
        self.perm_queue(dev_null).sync();
        self.perm_pump(dev_null);
    }

    /// The profile's non-default states arrived: the table decides kinds 1 to 4, and the plan goes
    /// through the queue ahead of anything asked after the read. The sync is done only when a
    /// COMPLETE read's plan has landed; an incomplete read fails closed: what was read is
    /// rewritten the same way and the profile is read again, a bounded number of times.
    pub(super) fn permission_states_read(&mut self, pane: PaneId, view: ViewId, states: &[(i32, String, i32)], complete: bool) {
        let dev_null = self.pane_dev_null(pane);
        let rows: Option<Vec<_>> = self.browser_db(dev_null).map(|db| db.all_permissions().into_iter().map(|r| (r.origin, r.kind, r.decision)).collect());
        let plan = sync_for(states, rows.as_deref());
        let Some(outs) = self.perm_queue(dev_null).read_answer(view, plan, complete) else { return };
        if !complete {
            crate::dlog!("webhost: the profile read was incomplete ({} states); read again", states.len());
        }
        self.perm_outs(dev_null, outs);
        self.perm_pump(dev_null);
    }

    fn apply_op(&mut self, dev_null: bool, origin: &str, op: Op) {
        let fx = effects(&op, origin);
        self.perm_queue(dev_null).revoke(fx.states, None);
        self.perm_pump(dev_null);
        if fx.reload {
            self.reload_panes_using(dev_null, origin);
        }
    }

    /// The page's Revoke: the row goes, the profile state is reset, and every pane using the origin reloads.
    #[allow(dead_code)] // Task 9's page calls it
    pub(super) fn revoke_permission(&mut self, dev_null: bool, origin: &str, t: PermissionType) {
        let Some(db) = self.browser_db(dev_null) else { return };
        db.revoke_permission(origin, t);
        self.apply_op(dev_null, origin, Op::Revoke(t));
    }

    /// Revoke all of an origin: every row goes, each saved mirrored kind is reset, panes reload.
    #[allow(dead_code)] // Task 9's page calls it
    pub(super) fn revoke_all_permissions(&mut self, dev_null: bool, origin: &str) {
        let Some(db) = self.browser_db(dev_null) else { return };
        let types: Vec<PermissionType> = db.permissions_for_origin(origin).into_iter().map(|r| r.kind).collect();
        db.revoke_all_permissions(origin);
        self.apply_op(dev_null, origin, Op::RevokeAll(types));
    }

    /// The toolbar's Reset: the row goes and the profile state is reset; nothing reloads.
    #[allow(dead_code)] // Task 8's menu calls it
    pub(super) fn reset_permission(&mut self, dev_null: bool, origin: &str, t: PermissionType) {
        let Some(db) = self.browser_db(dev_null) else { return };
        db.reset_permission(origin, t);
        self.apply_op(dev_null, origin, Op::Reset(t));
    }

    /// iTerm2's revocation sweep (`BPM:230-241`, `BM:518-586`): a pane whose main origin matches
    /// reloads at once, every other pane of the store asks the page for its frames' and resources'
    /// origins (isolated world) and reloads on a match; a failed script compares the main origin only.
    pub(super) fn reload_panes_using(&mut self, dev_null: bool, origin: &str) {
        let panes: Vec<(PaneId, String)> = self
            .browser_views
            .iter()
            .filter(|(p, b)| b.view.is_some() && self.pane_dev_null(**p) == dev_null)
            .filter_map(|(p, _)| self.panes.get(p).and_then(|pn| pn.browser_io()).map(|io| (*p, io.url.clone())))
            .collect();
        let body = format!("return ({});", bp::check_iframe_origins_js().trim_end().trim_end_matches(';'));
        for (pane, how) in reload_plan(&panes, origin) {
            match how {
                Reload::Now => self.reload_browser_view(pane),
                Reload::Sweep => {
                    let origin = origin.to_string();
                    self.browser_js_call(
                        pane,
                        &body,
                        serde_json::json!({}),
                        Box::new(move |app, pane, r| {
                            let frames = r.ok().and_then(|v| bp::frame_origins(&v));
                            let url = app.panes.get(&pane).and_then(|p| p.browser_io()).map(|io| io.url.clone()).unwrap_or_default();
                            if bp::page_uses_origin(&url, frames.as_deref(), &origin) {
                                app.reload_browser_view(pane);
                            }
                        }),
                    );
                }
            }
        }
    }

    fn reload_browser_view(&self, pane: PaneId) {
        if let Some(v) = self.browser_views.get(&pane).and_then(|b| b.view.as_ref()) {
            v.reload();
        }
    }

    /// A browser pane is closing: its requests are forgotten.
    pub(super) fn browser_permission_pane_gone(&mut self, pane: PaneId) {
        let acts = self.perm_asks.pane_gone(pane);
        self.perm_acts(acts);
    }

    /// A page's notification (13x6 Task 6). The database decides, not the page: only an origin with a
    /// saved Granted notifications row is shown, anything else is released without a sound. It
    /// goes through the one alert path (`post_alert`, `AlertKind::Web`), labelled with its origin.
    pub(super) fn browser_notification(&mut self, pane: PaneId, token: u64, origin: &str, title: &str, body: &str) {
        let origin = bp::normalize_origin(origin);
        let granted = origin.as_deref().filter(|_| self.is_browser_pane(pane)).and_then(|o| self.saved_notification(pane, o)).and_then(|d| bp::show_notification(Some(d)).then_some(d));
        let (Some(origin), Some(_)) = (origin, granted) else { return self.note_report(pane, token, NoteReport::Release) };
        // A granted site is still rate limited: one notification an origin every few seconds.
        if !self.web_note_limits.allow(origin.clone(), std::time::Instant::now(), WEB_NOTE_EVERY) {
            return self.note_report(pane, token, NoteReport::Release);
        }
        let kind = crate::alerts::AlertKind::Web { origin: origin.clone(), title: title.to_string(), body: body.to_string() };
        if self.post_alert(pane, kind) {
            // `post_alert` took the balloon from any older web notification; this one is on screen now.
            self.web_note.shown(pane, token, &origin);
            self.note_report(pane, token, NoteReport::Shown);
        } else {
            self.note_report(pane, token, NoteReport::Release);
        }
    }

    /// The page closed its notification: honored only for the one on screen, from an origin that is
    /// still granted (`BNH:58-62`).
    pub(super) fn browser_notification_closed(&mut self, pane: PaneId, token: u64) {
        let Some(origin) = self.web_note.origin_of(pane, token).map(str::to_string) else { return };
        if !bp::close_honored(self.saved_notification(pane, &origin)) {
            return;
        }
        self.web_note.take();
        crate::notify::clear_info();
        self.note_report(pane, token, NoteReport::Closed);
    }

    /// Another alert took the balloon: the web notification on it, if any, is reported closed.
    pub(super) fn web_note_replaced(&mut self) {
        if let Some((pane, token)) = self.web_note.take() {
            self.note_report(pane, token, NoteReport::Closed);
        }
    }

    /// A navigation or a closed pane: that pane's notification on the balloon goes.
    pub(super) fn browser_web_note_dropped(&mut self, pane: PaneId) {
        if let Some((pane, token)) = self.web_note.take_pane(pane) {
            crate::notify::clear_info();
            self.note_report(pane, token, NoteReport::Closed);
        }
    }

    fn saved_notification(&mut self, pane: PaneId, origin: &str) -> Option<Decision> {
        let dev_null = self.pane_dev_null(pane);
        self.browser_db(dev_null)?.get_permission(origin, PermissionType::Notification).map(|r| r.decision)
    }

    fn note_report(&self, pane: PaneId, token: u64, r: NoteReport) {
        if let Some(v) = self.browser_views.get(&pane).and_then(|b| b.view.as_ref()) {
            v.notification_report(token, r);
        }
    }

    /// Takes question `id` away: queued, or showing (its window closes with no answer).
    fn browser_permission_withdraw(&mut self, id: u64) {
        let is = |t: &Then| matches!(t, Then::BrowserPermission(i) if *i == id);
        self.alert_drop_queued(is);
        if self.scripts_alert.as_ref().is_some_and(|(_, a)| is(&a.then)) {
            self.scripts_alert = None;
        }
    }

    /// Whether pane `pane` is shown to the person: this window's active tab, in a layout (not buried
    /// or a hidden peer).
    fn browser_pane_shown(&self, pane: PaneId) -> bool {
        self.browser_pane_visible(pane)
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
        Prep { origin: if bp::from_kind(r.kind).is_some() { bp::normalize_origin(&r.uri) } else { None }, top: Some("https://a.b".into()), req: r, browser: true, dev_null: false, saved: saved.to_vec(), visible: true }
    }

    fn asked(acts: &[Act]) -> Vec<(u64, String, String)> {
        acts.iter().filter_map(|a| if let Act::Ask { id, heading, text } = a { Some((*id, heading.clone(), text.clone())) } else { None }).collect()
    }

    fn answers(acts: &[Act]) -> Vec<(u64, PermAnswer)> {
        acts.iter()
            .filter_map(|a| match a {
                Act::Answer { token, answer, .. } | Act::Mirror { token, answer, .. } => Some((*token, *answer)),
                _ => None,
            })
            .collect()
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
        assert_eq!(answers(&acts), vec![(3, PermAnswer::State { state: 2, saves: false })], "a Deny is never mirrored into the profile");
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
        // Within 60 s the same origin and kind in the same pane is denied with no window (B2); after
        // it, and for anything else, it asks again.
        let again = asks.plan_flush(vec![prep(req(p, 3, 4, "https://a.b/"), &[])], SYS);
        assert!(asked(&again).is_empty() && answers(&again) == vec![(3, PermAnswer::Deny)], "the cooldown denies quietly");
        let other = asked(&asks.plan_flush(vec![prep(req(p, 4, 3, "https://a.b/"), &[])], SYS));
        assert_eq!(other.len(), 1, "another kind asks");
        asks.answered(other[0].0, Some(1)).unwrap();
        let pane2 = asked(&asks.plan_flush(vec![prep(req(2, 5, 4, "https://a.b/"), &[])], SYS));
        assert_eq!(pane2.len(), 1, "another pane asks");
        asks.answered(pane2[0].0, Some(1)).unwrap();
        asks.now_ms = 60_001;
        assert_eq!(asked(&asks.plan_flush(vec![prep(req(p, 6, 4, "https://a.b/"), &[])], SYS)).len(), 1, "after the cooldown it asks again");
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
    fn browser_notification_follows_iterm2() {
        // An App cannot be built in a unit test (browser_test_seam); the pure rules are tested in
        // `browser_perms` and `alerts`, and this pins the wiring of the app side.
        let me = include_str!("browser_perms_app.rs");
        let body = me.split("pub(super) fn browser_notification(").nth(1).unwrap().split("pub(super) fn browser_notification_closed").next().unwrap();
        assert!(body.contains("bp::show_notification(") && body.contains("normalize_origin"), "only a granted, normalized origin");
        assert!(body.contains("AlertKind::Web") && body.contains("self.post_alert(") && !body.contains("show_with"), "the one alert path");
        assert!(body.contains("NoteReport::Release") && body.contains("NoteReport::Shown"));
        assert!(body.contains("web_note_limits.allow"), "a per-origin cap");
        let closed = me.split("pub(super) fn browser_notification_closed").nth(1).unwrap().split("pub(super) fn web_note_replaced").next().unwrap();
        assert!(closed.contains("bp::close_honored(") && closed.contains("clear_info()") && closed.contains("NoteReport::Closed"));
        let ev = include_str!("browser_app.rs");
        assert!(ev.contains("self.browser_notification(pane, *token, origin, title, body)") && ev.contains("self.browser_notification_closed(pane, *token)"));
        let alerts = include_str!("alerts_ui.rs");
        assert!(alerts.contains("self.web_note_replaced()"), "another alert reports the web notification closed");
        assert!(include_str!("browser_reader_app.rs").contains("self.browser_web_note_dropped(pane)"), "a navigation drops it");
        assert!(ev.contains("self.browser_web_note_dropped(pane)"), "a closed pane drops it");
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

    #[test]
    fn a_pane_that_is_not_shown_waits_and_asks_when_shown() {
        let mut asks = Asks::default();
        let mut hidden = prep(req(1, 1, 4, "https://a.b/"), &[]);
        hidden.visible = false;
        let acts = asks.plan_flush(vec![hidden], SYS);
        assert!(acts.is_empty(), "no window and no answer yet for a pane nobody sees");
        assert!(asks.ready_panes(&|_| false).is_empty(), "still hidden");
        assert_eq!(asks.ready_panes(&|p| p == 1), vec![1]);
        let again = asks.take_later(1);
        assert_eq!(again.len(), 1);
        assert_eq!(asked(&asks.plan_flush(again.into_iter().map(|r| prep(r, &[])).collect(), SYS)).len(), 1, "shown now: it asks");
        // The queue for a hidden pane is bounded, one request per origin.
        let mut asks = Asks::default();
        let mk = |t: u64, uri: &str| {
            let mut p = prep(req(2, t, 4, uri), &[]);
            p.visible = false;
            p
        };
        let acts = asks.plan_flush(vec![mk(1, "https://a.b/"), mk(2, "https://a.b/x"), mk(3, "https://c.d/")], SYS);
        assert_eq!(answers(&acts), vec![(2, PermAnswer::Deny)], "a second request of one origin is denied, not queued");
    }

    #[test]
    fn one_origin_cannot_crowd_out_another() {
        let mut asks = Asks::default();
        let q = asked(&asks.plan_flush(vec![prep(req(1, 1, 4, "https://a.b/"), &[])], SYS));
        assert_eq!(q.len(), 1);
        let uris = ["https://x1.test/", "https://x1.test/p", "https://x1.test/q", "https://x1.test/r", "https://top.test/"];
        let acts = asks.plan_flush(uris.iter().enumerate().map(|(i, u)| prep(req(1, 10 + i as u64, 4, u), &[])).collect(), SYS);
        assert_eq!(answers(&acts).iter().map(|a| a.0).collect::<Vec<_>>(), vec![11, 12, 13], "an origin's repeats are denied; the top page still waits");
        assert_eq!(asks.take_later(1).len(), 0, "the first question is still open");
    }

    #[test]
    fn navigation_or_closing_dismisses_the_prompt() {
        let mut asks = Asks::default();
        let q = asked(&asks.plan_flush(vec![prep(req(1, 1, 4, "https://a.b/"), &[])], SYS));
        // The page it asked from navigated (epoch 8): its waiter is denied and the question withdrawn.
        let acts = asks.dismiss(&|_| Some(8));
        assert_eq!(answers(&acts), vec![(1, PermAnswer::Deny)]);
        assert!(acts.contains(&Act::Withdraw { id: q[0].0 }));
        assert!(asks.answered(q[0].0, Some(0)).is_none(), "a late Allow click finds nothing to save");
        // Still the same page: nothing happens.
        let q = asked(&asks.plan_flush(vec![prep(req(1, 2, 4, "https://a.b/"), &[])], SYS));
        assert!(asks.dismiss(&|_| Some(7)).is_empty());
        // The pane closed.
        let acts = asks.pane_gone(1);
        assert_eq!(acts, vec![Act::Withdraw { id: q[0].0 }]);
        // Backstop: an answer for a page that has moved on saves nothing.
        let q = asked(&asks.plan_flush(vec![prep(req(1, 3, 4, "https://a.b/"), &[])], SYS));
        let s = asks.answered(q[0].0, Some(0)).unwrap();
        assert!(s.any_live(&|_| Some(7)) && !s.any_live(&|_| Some(9)) && !s.any_live(&|_| None));
    }

    #[test]
    fn profile_changes_follow_iterm2() {
        let origin = "https://a.b";
        let reset = |k: i32| (k, origin.to_string(), bp::STATE_DEFAULT);
        let fx = effects(&Op::Revoke(Notification), origin);
        assert_eq!((fx.states, fx.reload), (vec![reset(4)], true));
        let fx = effects(&Op::Revoke(Camera), origin);
        assert_eq!((fx.states, fx.reload), (vec![], true), "media is never mirrored but the pages still reload");
        let fx = effects(&Op::RevokeAll(vec![Notification, Geolocation, Camera, AudioPlayback]), origin);
        assert_eq!((fx.states, fx.reload), (vec![reset(4), reset(3)], true));
        let fx = effects(&Op::Reset(Geolocation), origin);
        assert_eq!((fx.states, fx.reload), (vec![reset(3)], false), "Reset reloads nothing (BVC:1075-1079)");
        // A pane on the origin reloads at once, any other runs the sweep.
        let panes = vec![(1, "https://a.b/x".to_string()), (2, "https://c.d/".to_string())];
        assert_eq!(reload_plan(&panes, origin), vec![(1, Reload::Now), (2, Reload::Sweep)]);
    }

    #[test]
    fn the_start_up_sync_fails_closed() {
        // A clipboard grant is kept; with an empty database nothing is sent for it.
        assert!(sync_for(&[(6, "https://a.b".into(), 1)], Some(&[])).is_empty());
        // A stale grant the table does not hold is reset; one it holds as Denied becomes Deny.
        let rows = [("https://c.d".to_string(), Notification, Decision::Denied)];
        let states = [(4, "https://a.b".to_string(), 1), (4, "https://c.d".to_string(), 1)];
        assert_eq!(sync_for(&states, Some(&rows)), vec![(4, "https://a.b".to_string(), 0), (4, "https://c.d".to_string(), 2)]);
        // An unreadable table wants nothing: no Allow of kinds 1 to 4 survives.
        assert_eq!(sync_for(&states, None), vec![(4, "https://a.b".to_string(), 0), (4, "https://c.d".to_string(), 0)]);
        // Newest-first rows: the newest wins.
        let rows = [("https://a.b".to_string(), Notification, Decision::Denied), ("https://a.b".to_string(), Notification, Decision::Granted)];
        assert_eq!(sync_for(&[], Some(&rows)), vec![(4, "https://a.b".to_string(), 2)]);
    }

    #[test]
    fn revocation_is_wired() {
        let me = include_str!("browser_perms_app.rs");
        let prod = me.split("\n#[cfg(test)]\nmod tests").next().unwrap();
        assert!(prod.contains("db.revoke_permission(origin, t)") && prod.contains("db.reset_permission(origin, t)") && prod.contains("db.revoke_all_permissions(origin)"));
        assert!(prod.contains("self.browser_js_call(") && prod.contains("bp::frame_origins(&v)") && prod.contains("bp::check_iframe_origins_js()"), "the one sweep script runs in the isolated world");
        let ev = include_str!("browser_app.rs");
        assert!(ev.contains("self.sync_permission_states(dev_null)") && ev.contains("self.permission_states_read(pane, *view, states, *complete)"));
        let rd = include_str!("browser_reader_app.rs");
        assert!(rd.contains("self.browser_permission_dismiss()"), "a navigation dismisses the question");
        assert!(include_str!("webhost_app.rs").contains("self.perm_store_lost("), "a lost environment is synced again");
        // The dev-null store never reaches the shared profile: calls pick the view by store.
        assert!(prod.contains("self.pane_dev_null(**p) == dev_null"));
    }

    #[test]
    fn a_saved_grant_is_mirrored_through_the_queue_before_the_answer() {
        // A saved Allow of a mirrored kind becomes a Mirror act (the queue writes, then answers);
        // a Deny, an unsaved answer and media are answered at once.
        let allow = PermAnswer::State { state: 1, saves: true };
        assert_eq!(answer_act(1, 2, 7, Notification, "https://a.b", allow), Act::Mirror { pane: 1, token: 2, epoch: 7, kind: 4, origin: "https://a.b".into(), answer: allow });
        assert_eq!(answer_act(1, 2, 7, Geolocation, "https://a.b", allow), Act::Mirror { pane: 1, token: 2, epoch: 7, kind: 3, origin: "https://a.b".into(), answer: allow });
        let media = PermAnswer::State { state: 1, saves: false };
        assert_eq!(answer_act(1, 2, 7, Camera, "https://a.b", media), Act::Answer { pane: 1, token: 2, answer: media });
        assert_eq!(answer_act(1, 2, 7, Notification, "https://a.b", PermAnswer::Deny), Act::Answer { pane: 1, token: 2, answer: PermAnswer::Deny });
        let mut asks = Asks::default();
        let q = asked(&asks.plan_flush(vec![prep(req(1, 1, 4, "https://a.b/"), &[])], SYS));
        let s = asks.answered(q[0].0, Some(0)).unwrap();
        assert!(matches!(settle(&s, true, &alive)[..], [Act::Mirror { kind: 4, .. }]), "the dialog's Allow is mirrored first");
        let saved = asks.plan_flush(vec![prep(req(1, 2, 3, "https://a.b/"), &[(Geolocation, Decision::Granted)])], SYS);
        assert!(matches!(saved[..], [Act::Mirror { kind: 3, .. }]), "a saved grant answering a new request too");
    }

    #[test]
    fn every_profile_write_goes_through_the_one_queue() {
        let me = include_str!("browser_perms_app.rs");
        let prod = me.split("\n#[cfg(test)]\nmod tests").next().unwrap();
        assert!(!prod.contains(concat!("set_permission_", "states(")), "no batch write");
        assert_eq!(prod.matches(concat!(".set_permission_", "state(")).count(), 1, "one place issues writes: the queue's output");
        assert_eq!(prod.matches(concat!(".read_permission_", "states()")).count(), 1, "one place issues reads: the queue's output");
        let op = &prod[prod.find("fn apply_op(").unwrap()..];
        assert!(op[..400].contains(".revoke(fx.states"), "revoke and reset write through the queue with a new generation");
        assert!(prod.contains(".mirror_then_answer(kind, &origin, pane, token, epoch, answer)"), "a grant is mirrored, then answered");
        let ev = include_str!("browser_app.rs");
        assert!(ev.contains("Event::PermissionWritten { view, seq, ok } => self.perm_written(pane, *view, *seq, *ok)"));
        let host = include_str!("webhost_app.rs");
        let dropped = &host[host.find("pub(super) fn web_view_dropped(").unwrap()..];
        assert!(dropped[..300].contains("self.perm_view_gone(view)"), "a view that goes hands its write on");
        assert!(include_str!("adblock_app.rs").contains("self.perm_store_lost(true)"));
    }

    #[test]
    fn the_sync_is_done_only_after_a_complete_read_landed() {
        let me = include_str!("browser_perms_app.rs");
        let prod = me.split("\n#[cfg(test)]\nmod tests").next().unwrap();
        let read = &prod[prod.find("pub(super) fn permission_states_read(").unwrap()..prod.find("fn apply_op(").unwrap()];
        assert!(read.contains(".read_answer(view, plan, complete)"), "the queue decides from the answer's completeness");
        assert!(read.contains("self.perm_outs(dev_null, outs)") && read.contains("self.perm_pump(dev_null)"));
        let sync = &prod[prod.find("pub(super) fn sync_permission_states(").unwrap()..prod.find("pub(super) fn permission_states_read(").unwrap()];
        assert!(sync.contains("self.perm_queue(dev_null).sync()"), "every view of a store starts its sync unless it runs or is done");
        assert!(!prod.contains("SyncState"), "no second record of the sync");
        let ev = include_str!("browser_app.rs");
        let created = &ev[ev.find("Event::Created(_) => {").unwrap()..];
        assert!(created[..600].contains("self.sync_permission_states(dev_null)"));
    }

    #[test]
    fn no_page_loads_before_its_store_synced() {
        assert!(page_may_load(false, true));
        assert!(!page_may_load(false, false), "not synced: the page waits");
        assert!(!page_may_load(true, true), "restoring: the page waits");
        let rs = include_str!("browser_restore_app.rs");
        let own = &rs[rs.find("pub(super) fn browser_load_own").unwrap()..rs.find("pub(super) fn browser_nav_starting").unwrap()];
        let gate = own.find("page_may_load(self.browser_restoring(), self.perm_synced(pane))").expect("the one road for a pane's own load is gated");
        assert!(gate < own.find("navigate_tandem").unwrap() && own[gate..].contains("restore_load = Some("), "it waits, never loads");
        let done = &rs[rs.find("fn browser_release_waiting_loads").unwrap()..rs.find("pub(super) fn browser_load_own").unwrap()];
        assert!(done.contains("restore_load.take()") && done.contains("self.browser_load_own(pane, &url)"));
        let app = include_str!("browser_app.rs");
        let nav = &app[app.find("pub(super) fn browser_navigate(").unwrap()..app.find("pub(super) fn browser_suggest").unwrap()];
        assert!(nav.contains("let synced = self.perm_synced(pane);") && nav.find("if !synced").unwrap() < nav.find("navigate_tandem").unwrap(), "a typed page waits too");
        let me = include_str!("browser_perms_app.rs");
        assert!(me.contains("Out::Synced => self.browser_release_waiting_loads(Some(dev_null))"), "the sync's end loads what waits");
    }
}
