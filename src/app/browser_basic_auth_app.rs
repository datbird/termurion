//! HTTP authentication in a browser pane (13x7 Task 10): WebView2's `BasicAuthenticationRequested`
//! (and the proxy's) goes only through the pane's `Coordinator` (Task 8). `AuthPane` is the pure
//! half: it takes WebView2's facts (a navigation starting, committed, responded, completed; a
//! challenge; the pane shown or hidden; the prompt's and the picker's answers), drives the
//! coordinator and answers with `Action`s the `App` carries out. Every challenge holds a WebView2
//! deferral under a token; each token is answered exactly once, with the person's credential only
//! after the person answered a prompt or a picker (`Supply`), else cancelled: on every failure, a
//! new navigation, a parked navigation, the pane hidden, a reset, the pane closing, the view going
//! away, the coordinator being dropped. Nothing here logs a user name or a password.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::WindowId;

use super::*;
use crate::basic_auth_ui::{AuthAnswer, AuthWindow};
use crate::browser_basic_auth::{self as ba, ChallengeId, Credential, Effect, NavTracker, PromptOutcome, PromptRequest, SpaceKey};
use crate::browser_password::PwPage;
use crate::pwm::Secret;
use crate::pwm_source::Scope;
use crate::webhost::{Event, ViewId};

use super::pwm_app::{AuthWork, PwmOpen, PwmTarget, StoreReply};

/// A lookup the store has not answered by now ends the challenges held (Cancel).
const LOOKUP_WAIT: Duration = Duration::from_secs(15);

/// Challenge ids are unique across panes (a picker names its challenge only by number).
static NEXT_CHALLENGE: AtomicU64 = AtomicU64::new(1);

/// Tokens of challenges whose coordinator was dropped without a reset: (pane, view, token). The
/// owner answers each Cancel on the next pass (the view's own drop cancels the rest).
static DROPPED: Mutex<Vec<(PaneId, ViewId, u64)>> = Mutex::new(Vec::new());

/// What the `App` does for the pane's pure state.
#[derive(Debug)]
pub(super) enum Action {
    /// Complete the deferral under `token`: Some hands over the person's credential, None cancels.
    Answer { token: u64, answer: Option<(String, Secret)> },
    Lookup { id: u64, space: SpaceKey, preferred_user: Option<String> },
    Picker { id: u64, space: SpaceKey },
    ClosePicker(u64),
    Persist { space: SpaceKey, credential: Credential },
    Toast(String),
}

/// What a challenge depends on besides its text.
pub(super) struct ChallengeCx {
    pub dev_null: bool,
    /// The configured browser proxy (host, port) when `BrowserProxyEnabled`.
    pub proxy: Option<(String, u16)>,
    /// The browser store can be read now without any prompt.
    pub unlocked: bool,
}

/// One pane's HTTP authentication state. App-wide, keyed by pane (no `swap_live` entry); it goes with
/// the view (`basic_auth_reset`) and the pane (`basic_auth_pane_gone`).
pub(super) struct AuthPane {
    view: ViewId,
    coordinator: ba::Coordinator,
    tracker: NavTracker,
    /// Challenge id to WebView2 token (shared with the coordinator's drop sink).
    slots: Arc<Mutex<HashMap<u64, u64>>>,
    /// The prompt the coordinator asked for, until its window opens.
    pub(super) want: Option<(u64, PromptRequest)>,
    /// The open prompt window.
    pub(super) prompt: Option<(u64, AuthWindow)>,
    /// The challenge whose picker is open and the user name and password typed before it.
    picker: Option<(u64, Option<Credential>)>,
    lookups: HashMap<u64, Instant>,
    shown: Option<bool>,
}

impl AuthPane {
    pub(super) fn new(pane: PaneId, view: ViewId, shown: bool) -> AuthPane {
        let slots: Arc<Mutex<HashMap<u64, u64>>> = Arc::default();
        let mut coordinator = ba::Coordinator::default();
        let sink = slots.clone();
        coordinator.on_drop(Box::new(move |fx| {
            for e in fx {
                if let Effect::Cancel(id) | Effect::DefaultHandling(id) = e {
                    if let Some(token) = sink.lock().unwrap().remove(&id.0) {
                        DROPPED.lock().unwrap().push((pane, view, token));
                    }
                }
            }
        }));
        let mut p = AuthPane { view, coordinator, tracker: NavTracker::default(), slots, want: None, prompt: None, picker: None, lookups: HashMap::new(), shown: None };
        p.set_visible(shown);
        p
    }

    pub(super) fn view(&self) -> ViewId {
        self.view
    }

    /// Challenges held now.
    #[cfg(test)]
    pub(super) fn pending(&self) -> usize {
        self.coordinator.pending()
    }

    pub(super) fn holds(&self, id: u64) -> bool {
        self.slots.lock().unwrap().contains_key(&id)
    }

    /// The pane is (not) shown; only a change reaches the coordinator.
    pub(super) fn set_visible(&mut self, shown: bool) -> Vec<Action> {
        if self.shown == Some(shown) {
            return Vec::new();
        }
        self.shown = Some(shown);
        self.feed(ba::Event::Visible(shown))
    }

    /// WebView2's NavigationStarting (the one verdict and the sync gate already passed).
    pub(super) fn starting(&mut self, nav: u64, uri: &str, redirect: bool) -> Vec<Action> {
        let events = self.tracker.starting(nav, uri, redirect);
        self.feed_all(events)
    }

    pub(super) fn committed(&mut self, nav: u64) -> Vec<Action> {
        let events = self.tracker.committed(nav);
        self.feed_all(events)
    }

    pub(super) fn completed(&mut self, nav: u64, ok: bool) -> Vec<Action> {
        let events = self.tracker.completed(nav, ok);
        let mut out = self.feed_all(events);
        out.extend(self.end_wait());
        out
    }

    /// A held 401/407 that nothing waits on any more is the final response (`NavTracker::end_wait`).
    fn end_wait(&mut self) -> Vec<Action> {
        let events = self.tracker.end_wait(self.coordinator.pending() > 0);
        self.feed_all(events)
    }

    pub(super) fn response(&mut self, nav: u64, status: i32) -> Vec<Action> {
        let status = u16::try_from(status).unwrap_or(0);
        let events = self.tracker.response(nav, status);
        self.feed_all(events)
    }

    /// A navigation the 13x6 sync gate parked: it never started, so there is nothing to bind a
    /// challenge to; whatever the pane still holds is cancelled (the page is about to be replaced).
    pub(super) fn parked(&mut self) -> Vec<Action> {
        self.feed(ba::Event::MainProvisionalStarted(None))
    }

    /// `BasicAuthenticationRequested`: the challenge becomes the coordinator's, or is cancelled at once.
    pub(super) fn challenge(&mut self, token: u64, uri: &str, challenge: &str, cx: &ChallengeCx) -> Vec<Action> {
        let (scheme, realm) = ba::parse_challenge(challenge);
        let Some(space) = ba::space_of(uri, &realm) else {
            return vec![Action::Answer { token, answer: None }];
        };
        let proxy = ba::is_proxy_challenge(uri, cx.proxy.as_ref().map(|(h, p)| (h.as_str(), *p)));
        // Fail safe (R13x710-2): whether WebView2 names the proxy or the site in a proxy's 407 is not
        // proven where the probe cannot run, so while a proxy is configured no challenge is looked up
        // or remembered (a proxy's credential is not the site's).
        let remember = !proxy && cx.proxy.is_none() && ba::can_remember(cx.dev_null);
        let id = NEXT_CHALLENGE.fetch_add(1, Ordering::Relaxed);
        self.slots.lock().unwrap().insert(id, token);
        let event = ba::Event::Challenge {
            id: ChallengeId(id),
            space,
            is_basic: scheme.eq_ignore_ascii_case("basic"),
            is_proxy: proxy,
            proposed_user: None,
            saved_lookup_possible: remember && cx.unlocked,
            store_can_remember: remember,
        };
        self.feed(event)
    }

    /// The store's answer to a lookup (a late one for a challenge that ended gives nothing).
    pub(super) fn looked(&mut self, id: u64, found: Option<(String, Secret)>) -> Vec<Action> {
        self.lookups.remove(&id);
        let credential = found.map(|(user, password)| Credential { user, password });
        self.feed(ba::Event::SavedLookupResult { id: ChallengeId(id), credential })
    }

    pub(super) fn saved(&mut self, space: SpaceKey, host: String, success: bool) -> Vec<Action> {
        self.feed(ba::Event::SaveResult { space, host, success })
    }

    /// The prompt's answer. The window is gone.
    pub(super) fn prompt_answer(&mut self, id: u64, answer: AuthAnswer) -> Vec<Action> {
        if self.prompt.as_ref().is_some_and(|(p, _)| *p == id) {
            self.prompt = None;
        }
        let outcome = match answer {
            AuthAnswer::Cancel => PromptOutcome::Cancelled,
            AuthAnswer::Ok { user, password, remember } => PromptOutcome::Submitted { credential: Credential { user, password }, remember },
            AuthAnswer::PasswordManager { user, password } => {
                PromptOutcome::OpenPasswordManager((!user.is_empty() || !password.is_empty()).then_some(Credential { user, password }))
            }
        };
        self.feed(ba::Event::PromptResult { id: ChallengeId(id), outcome })
    }

    /// The picker's outcome (or its end); the typed credential it was opened with comes back.
    pub(super) fn picked(&mut self, id: u64, chosen: Option<(String, Secret)>) -> Vec<Action> {
        let typed = match self.picker.take() {
            Some((p, typed)) if p == id => typed,
            other => {
                self.picker = other;
                None
            }
        };
        let chosen = chosen.map(|(user, password)| Credential { user, password });
        self.feed(ba::Event::PickerResult { id: ChallengeId(id), chosen, typed_fallback: typed })
    }

    /// The pane closed, its view went or its world was reset: everything held is cancelled, the
    /// windows go, secrets are dropped.
    pub(super) fn gone(&mut self) -> Vec<Action> {
        let mut out = self.feed(ba::Event::Reset);
        // Belt: a token the coordinator did not name is still cancelled.
        let rest: Vec<u64> = self.slots.lock().unwrap().drain().map(|(_, t)| t).collect();
        out.extend(rest.into_iter().map(|token| Action::Answer { token, answer: None }));
        if let Some((id, _)) = self.picker.take() {
            out.push(Action::ClosePicker(id));
        }
        self.want = None;
        self.prompt = None;
        self.lookups.clear();
        out
    }

    /// Lookups the store did not answer in time end the challenges held (Cancel).
    pub(super) fn expired(&mut self, now: Instant) -> Vec<Action> {
        if !self.lookups.values().any(|t| now.saturating_duration_since(*t) >= LOOKUP_WAIT) {
            return Vec::new();
        }
        self.lookups.clear();
        self.parked()
    }

    /// The soonest a lookup can expire.
    pub(super) fn due(&self) -> Option<Instant> {
        self.lookups.values().map(|t| *t + LOOKUP_WAIT).min()
    }

    fn feed_all(&mut self, events: Vec<ba::Event>) -> Vec<Action> {
        events.into_iter().flat_map(|e| self.feed(e)).collect()
    }

    fn feed(&mut self, e: ba::Event) -> Vec<Action> {
        let fx = self.coordinator.reduce(e);
        let answered = fx.iter().any(|e| matches!(e, Effect::Supply(..) | Effect::Cancel(..) | Effect::DefaultHandling(..)));
        if fx.iter().any(|e| matches!(e, Effect::Supply(..))) {
            // WebView2 retries a handed-over credential as a new navigation (`NavTracker`).
            self.tracker.supplied();
        }
        if !self.coordinator.has_awaiting() {
            // The retry only continues an awaiting supply (final review M3).
            self.tracker.retry_lost();
        }
        let mut out = self.actions(fx);
        if answered {
            out.extend(self.end_wait());
        }
        out
    }

    fn answer(&mut self, id: ChallengeId, answer: Option<(String, Secret)>) -> Option<Action> {
        self.lookups.remove(&id.0);
        let token = self.slots.lock().unwrap().remove(&id.0)?;
        Some(Action::Answer { token, answer })
    }

    fn actions(&mut self, fx: Vec<Effect>) -> Vec<Action> {
        let mut out = Vec::new();
        for e in fx {
            match e {
                Effect::Supply(id, c) => out.extend(self.answer(id, Some((c.user.clone(), c.password.duplicate())))),
                Effect::Cancel(id) | Effect::DefaultHandling(id) => {
                    if self.want.as_ref().is_some_and(|(w, _)| *w == id.0) {
                        self.want = None;
                    }
                    out.extend(self.answer(id, None));
                }
                Effect::LookupSaved { id, space, preferred_user } => {
                    self.lookups.insert(id.0, Instant::now());
                    out.push(Action::Lookup { id: id.0, space, preferred_user });
                }
                Effect::Prompt(id, req) => self.want = Some((id.0, req)),
                Effect::ShowPicker { id, space, typed } => {
                    self.picker = Some((id.0, typed));
                    out.push(Action::Picker { id: id.0, space });
                }
                Effect::ClosePrompt(id) => {
                    if self.want.as_ref().is_some_and(|(w, _)| *w == id.0) {
                        self.want = None;
                    }
                    if self.prompt.as_ref().is_some_and(|(p, _)| *p == id.0) {
                        self.prompt = None;
                    }
                    if self.picker.as_ref().is_some_and(|(p, _)| *p == id.0) {
                        self.picker = None;
                        out.push(Action::ClosePicker(id.0));
                    }
                }
                Effect::Persist(space, credential) => out.push(Action::Persist { space, credential }),
                Effect::Toast(t) => out.push(Action::Toast(t)),
            }
        }
        out
    }
}

/// The account name a challenge's space is stored under.
fn name_of(space: &SpaceKey) -> String {
    ba::account_name(&space.scheme, &space.host, space.port, &space.realm)
}

/// Saving asks the BROWSER store's authentication, never the terminal's, and only when a prompt is due.
pub(super) fn authorize_save(needs_prompt: bool, authenticate: &mut dyn FnMut(Scope) -> bool) -> bool {
    !needs_prompt || authenticate(Scope::Browser)
}

impl App {
    /// A browser view event that concerns HTTP authentication (all others pass).
    pub(super) fn basic_auth_event(&mut self, pane: PaneId, e: &Event) {
        let relevant = matches!(e, Event::Starting { .. } | Event::Committed { .. } | Event::Response { .. } | Event::Navigated { .. } | Event::BasicAuth { .. } | Event::Parked { .. });
        if !relevant {
            return;
        }
        let view = e.view();
        if self.basic_auth.get(&pane).is_some_and(|a| a.view() != view) {
            self.basic_auth_reset(pane);
        }
        if !self.basic_auth.contains_key(&pane) {
            // A pane is shown only once it says so; the first fact it gave is the starting point.
            let shown = self.browser_views.get(&pane).is_some_and(|b| b.replay.shown);
            self.basic_auth.insert(pane, AuthPane::new(pane, view, shown));
        }
        let acts = match e {
            Event::Starting { nav, url, redirect, .. } => self.basic_auth.get_mut(&pane).map(|a| a.starting(*nav, url, *redirect)),
            Event::Committed { nav, .. } => self.basic_auth.get_mut(&pane).map(|a| a.committed(*nav)),
            Event::Response { nav, status, .. } => self.basic_auth.get_mut(&pane).map(|a| a.response(*nav, *status)),
            Event::Navigated { nav, ok, .. } => self.basic_auth.get_mut(&pane).map(|a| a.completed(*nav, *ok)),
            Event::Parked { .. } => self.basic_auth.get_mut(&pane).map(|a| a.parked()),
            Event::BasicAuth { token, uri, challenge, .. } => {
                let cx = self.basic_auth_cx(pane);
                self.basic_auth.get_mut(&pane).map(|a| a.challenge(*token, uri, challenge, &cx))
            }
            _ => None,
        };
        self.basic_auth_run(pane, acts.unwrap_or_default());
    }

    fn basic_auth_cx(&mut self, pane: PaneId) -> ChallengeCx {
        let dev_null = self.pane_dev_null(pane);
        let proxy = self.globals.bool("BrowserProxyEnabled").then(|| (self.globals.string("BrowserProxyHost"), u16::try_from(self.globals.int("BrowserProxyPort")).unwrap_or(0)));
        ChallengeCx { dev_null, proxy, unlocked: self.basic_auth_unlocked() }
    }

    /// The pane is shown or hidden (every frame; only a change counts).
    pub(super) fn basic_auth_visible(&mut self, pane: PaneId, shown: bool) {
        let acts = self.basic_auth.get_mut(&pane).map(|a| a.set_visible(shown)).unwrap_or_default();
        self.basic_auth_run(pane, acts);
    }

    /// The pane's view or world went: everything held is cancelled and forgotten.
    pub(super) fn basic_auth_reset(&mut self, pane: PaneId) {
        if let Some(mut a) = self.basic_auth.remove(&pane) {
            let acts = a.gone();
            self.basic_auth.insert(pane, a);
            self.basic_auth_run(pane, acts);
            self.basic_auth.remove(&pane);
        }
    }

    /// The pane closed.
    pub(super) fn basic_auth_pane_gone(&mut self, pane: PaneId) {
        self.basic_auth_reset(pane);
    }

    /// Carries out the actions; a lookup or save that cannot be sent answers the coordinator at once.
    fn basic_auth_run(&mut self, pane: PaneId, acts: Vec<Action>) {
        let mut todo: std::collections::VecDeque<Action> = acts.into();
        while let Some(act) = todo.pop_front() {
            match act {
                Action::Answer { token, answer } => {
                    if let Some(v) = self.browser_views.get(&pane).and_then(|b| b.view.as_ref()) {
                        v.answer_basic_auth(token, answer);
                    }
                }
                Action::Lookup { id, space, preferred_user } => {
                    let work = AuthWork::Lookup { pane, id, name: name_of(&space), preferred_user };
                    if !self.basic_auth_store_job(work) {
                        todo.extend(self.basic_auth.get_mut(&pane).map(|a| a.looked(id, None)).unwrap_or_default());
                    }
                }
                Action::Picker { id, space } => {
                    let origin_url = format!("{}://{}:{}/", space.scheme, space.host, space.port);
                    self.open_password_manager_with(PwmOpen {
                        account: Some(name_of(&space)),
                        pane: Some(pane),
                        scope: Scope::Browser,
                        for_user: true,
                        send_both: true,
                        target: PwmTarget::BasicAuth(id),
                        pick_page: PwPage::new(&origin_url, 0),
                        ..PwmOpen::default()
                    });
                }
                Action::ClosePicker(id) => self.basic_auth_picker_gone(id),
                Action::Persist { space, credential } => {
                    let needs = self.basic_auth_save_needs_prompt();
                    let ok = authorize_save(needs, &mut |scope| self.authenticate(scope));
                    let host = space.host.clone();
                    let sent = ok && {
                        let work = AuthWork::Save { pane, space: space.clone(), name: name_of(&space), user: credential.user.clone(), password: credential.password.duplicate() };
                        self.basic_auth_store_job(work)
                    };
                    if !sent {
                        todo.extend(self.basic_auth.get_mut(&pane).map(|a| a.saved(space, host, false)).unwrap_or_default());
                    }
                }
                Action::Toast(t) => self.show_toast(&t),
            }
        }
    }

    /// Takes the store's answers, opens a prompt that is wanted, answers the coordinators dropped
    /// without a reset and ends the lookups that took too long. Returns when the next lookup expires.
    pub(super) fn basic_auth_tick(&mut self, event_loop: &ActiveEventLoop) -> Option<Instant> {
        for (pane, view, token) in std::mem::take(&mut *DROPPED.lock().unwrap()) {
            if let Some(v) = self.browser_views.get(&pane).filter(|b| b.id == view).and_then(|b| b.view.as_ref()) {
                v.answer_basic_auth(token, None);
            }
        }
        for r in self.pwm.take_store_replies() {
            let (pane, acts) = match r {
                StoreReply::Looked { pane, id, found } => (pane, self.basic_auth.get_mut(&pane).map(|a| a.looked(id, found))),
                StoreReply::Saved { pane, space, host, success } => (pane, self.basic_auth.get_mut(&pane).map(|a| a.saved(space, host, success))),
            };
            self.basic_auth_run(pane, acts.unwrap_or_default());
        }
        let now = Instant::now();
        let panes: Vec<PaneId> = self.basic_auth.keys().copied().collect();
        for pane in &panes {
            let acts = self.basic_auth.get_mut(pane).map(|a| a.expired(now)).unwrap_or_default();
            self.basic_auth_run(*pane, acts);
        }
        for pane in &panes {
            self.basic_auth_open_prompt(event_loop, *pane);
        }
        self.basic_auth.values().filter_map(AuthPane::due).min()
    }

    /// Opens the sign-in prompt the coordinator asked for; a window that cannot open cancels it.
    fn basic_auth_open_prompt(&mut self, event_loop: &ActiveEventLoop, pane: PaneId) {
        let Some(a) = self.basic_auth.get_mut(&pane) else { return };
        if a.prompt.is_some() {
            return;
        }
        let Some((id, req)) = a.want.take() else { return };
        let opened = match self.gpu.as_ref() {
            Some(g) => {
                let (user, password) = req.prefill.as_ref().map_or(("", None), |c| (c.user.as_str(), Some(&c.password)));
                AuthWindow::open(event_loop, &g.instance, &g.adapter, &g.device, &g.queue, &ba::prompt_heading(&req), user, password, req.offer_remember, req.remember_by_default, req.offer_password_manager)
            }
            None => Err("no graphics device".to_string()),
        };
        match opened {
            Ok(w) => {
                w.focus();
                if let Some(a) = self.basic_auth.get_mut(&pane) {
                    a.prompt = Some((id, w));
                }
            }
            Err(e) => {
                self.banners.set("window", format!("Could not open the window: {e}"));
                let acts = self.basic_auth.get_mut(&pane).map(|a| a.prompt_answer(id, AuthAnswer::Cancel)).unwrap_or_default();
                self.basic_auth_run(pane, acts);
            }
        }
    }

    /// Events for a sign-in prompt window. False for other windows. A close is Cancel.
    pub(super) fn basic_auth_window_event(&mut self, window: WindowId, e: &WindowEvent) -> bool {
        let Some(pane) = self.basic_auth.iter().find(|(_, a)| a.prompt.as_ref().is_some_and(|(_, w)| w.id() == window)).map(|(p, _)| *p) else { return false };
        let Some((id, w)) = self.basic_auth.get_mut(&pane).and_then(|a| a.prompt.as_mut()) else { return true };
        let id = *id;
        let answer = match e {
            WindowEvent::CloseRequested => Some(AuthAnswer::Cancel),
            WindowEvent::RedrawRequested => match w.redraw() {
                Some(a) => Some(a),
                None => return true,
            },
            other => {
                w.event(other);
                return true;
            }
        };
        if let Some(answer) = answer {
            let acts = self.basic_auth.get_mut(&pane).map(|a| a.prompt_answer(id, answer)).unwrap_or_default();
            self.basic_auth_run(pane, acts);
        }
        true
    }

    /// A picker's outcome (`BAP:60-73`); a challenge that already ended gives nothing.
    pub(super) fn basic_auth_picked(&mut self, challenge: u64, picked: Option<(String, Secret)>) {
        let Some(pane) = self.basic_auth.iter().find(|(_, a)| a.holds(challenge)).map(|(p, _)| *p) else { return };
        let acts = self.basic_auth.get_mut(&pane).map(|a| a.picked(challenge, picked)).unwrap_or_default();
        self.basic_auth_run(pane, acts);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cx(dev_null: bool, unlocked: bool) -> ChallengeCx {
        ChallengeCx { dev_null, proxy: None, unlocked }
    }

    fn pane() -> AuthPane {
        let mut p = AuthPane::new(1, 1, true);
        p.starting(1, "https://a.b/", false);
        p
    }

    fn answers(acts: &[Action]) -> Vec<(u64, bool)> {
        acts.iter().filter_map(|a| if let Action::Answer { token, answer } = a { Some((*token, answer.is_some())) } else { None }).collect()
    }

    fn lookup_id(acts: &[Action]) -> u64 {
        acts.iter().find_map(|a| if let Action::Lookup { id, .. } = a { Some(*id) } else { None }).expect("a lookup")
    }

    fn typed(user: &str, pw: &str, remember: bool) -> AuthAnswer {
        AuthAnswer::Ok { user: user.into(), password: Secret::new(pw.into()), remember }
    }

    const BASIC: &str = "Basic realm=\"R\"";

    /// Review Focus 9: a challenge ends exactly once, whichever way it ends.
    #[test]
    fn pending_challenges_end_once() {
        // A saved credential only prefills; the person's OK answers the token once with it.
        let mut p = pane();
        let acts = p.challenge(11, "https://a.b/", BASIC, &cx(false, true));
        let id = lookup_id(&acts);
        let acts = p.looked(id, Some(("saved".into(), Secret::new("pw".into()))));
        assert!(answers(&acts).is_empty(), "a saved credential is never supplied silently");
        let (_, req) = p.want.take().expect("the prompt is wanted");
        assert_eq!(req.prefill.as_ref().map(|c| c.user.as_str()), Some("saved"));
        let acts = p.prompt_answer(id, typed("saved", "pw", false));
        assert_eq!(answers(&acts), vec![(11, true)]);
        assert!(answers(&p.prompt_answer(id, typed("saved", "pw", false))).is_empty(), "a second answer gives nothing");
        // A prompt answered Cancel answers None once.
        let mut p = pane();
        let id = lookup_id(&p.challenge(12, "https://a.b/", BASIC, &cx(false, true)));
        p.looked(id, None);
        assert!(p.want.is_some());
        assert_eq!(answers(&p.prompt_answer(id, AuthAnswer::Cancel)), vec![(12, false)]);
        // The pane gone with a prompt open answers None once.
        let mut p = pane();
        let id = lookup_id(&p.challenge(13, "https://a.b/", BASIC, &cx(false, true)));
        p.looked(id, None);
        let acts = p.gone();
        assert_eq!(answers(&acts), vec![(13, false)]);
        assert!(p.want.is_none() && p.prompt.is_none() && p.pending() == 0);
        assert!(answers(&p.gone()).is_empty(), "a second reset answers nothing");
        // A result for an id the pane does not hold answers nothing.
        assert!(answers(&p.looked(9_999_999, None)).is_empty());
        assert!(answers(&p.prompt_answer(9_999_999, AuthAnswer::Cancel)).is_empty());
    }

    /// A parked navigation and a new one cancel what the pane holds; a hidden pane asks nothing.
    #[test]
    fn navigation_hidden_and_parked_cancel_every_challenge() {
        let hold = |p: &mut AuthPane, token| {
            let id = lookup_id(&p.challenge(token, "https://a.b/", BASIC, &cx(false, true)));
            p.looked(id, None);
            id
        };
        let mut p = pane();
        hold(&mut p, 21);
        assert_eq!(answers(&p.parked()), vec![(21, false)], "a navigation parked by the sync gate");
        assert!(p.want.is_none());
        let mut p = pane();
        hold(&mut p, 22);
        assert_eq!(answers(&p.starting(2, "https://a.b/", false)), vec![(22, false)], "a fresh navigation");
        let mut p = pane();
        hold(&mut p, 23);
        assert_eq!(answers(&p.set_visible(false)), vec![(23, false)], "hidden");
        assert_eq!(answers(&p.challenge(24, "https://a.b/", BASIC, &cx(false, true))), vec![(24, false)], "a hidden pane is never asked");
        let mut p = AuthPane::new(1, 1, false);
        p.starting(1, "https://a.b/", false);
        assert_eq!(answers(&p.challenge(25, "https://a.b/", BASIC, &cx(false, true))), vec![(25, false)], "not shown until it says so");
        // A challenge for a URI that is no space is cancelled at once.
        let mut p = pane();
        assert_eq!(answers(&p.challenge(26, "about:blank", BASIC, &cx(false, true))), vec![(26, false)]);
    }

    /// Review Focus 8: a Dev Null pane never remembers.
    #[test]
    fn dev_null_never_remembers() {
        let mut p = pane();
        let acts = p.challenge(31, "https://a.b/", BASIC, &cx(true, true));
        assert!(!acts.iter().any(|a| matches!(a, Action::Lookup { .. })), "no lookup");
        let (id, req) = p.want.take().expect("it prompts at once");
        assert!(!req.offer_remember && !req.offer_password_manager && !req.remember_by_default, "no checkbox, no Password Manager button");
        let acts = p.prompt_answer(id, typed("u", "p", true));
        assert_eq!(answers(&acts), vec![(31, true)]);
        let acts = p.response(1, 200);
        assert!(!acts.iter().any(|a| matches!(a, Action::Persist { .. })), "a typed value is never saved");
    }

    /// A remembered sign-in persists only after the server accepted it, whatever order the events come in.
    #[test]
    fn a_remembered_credential_persists_after_a_2xx_response() {
        let mut p = pane();
        let id = lookup_id(&p.challenge(41, "https://a.b/", BASIC, &cx(false, true)));
        p.looked(id, None);
        let acts = p.prompt_answer(id, typed("u", "p", true));
        assert_eq!(answers(&acts), vec![(41, true)]);
        assert!(p.committed(1).is_empty(), "the commit waits for the response");
        let acts = p.response(1, 200);
        assert!(matches!(acts.as_slice(), [Action::Persist { .. }]), "{acts:?}");
        // A proxy challenge is prompt-only and never remembered.
        let mut p = pane();
        let proxy = ChallengeCx { dev_null: false, proxy: Some(("127.0.0.1".into(), 8080)), unlocked: true };
        let acts = p.challenge(42, "http://127.0.0.1:8080/", "Basic realm=\"P\"", &proxy);
        assert!(!acts.iter().any(|a| matches!(a, Action::Lookup { .. })));
        let (_, req) = p.want.take().unwrap();
        assert!(req.proxy && !req.offer_remember);
        // While a proxy is on, a challenge that names another host is prompt-only too (fail safe).
        let mut p = pane();
        let acts = p.challenge(43, "https://a.b/", BASIC, &proxy);
        assert!(!acts.iter().any(|a| matches!(a, Action::Lookup { .. })));
        let (_, req) = p.want.take().unwrap();
        assert!(!req.proxy && !req.offer_remember && !req.offer_password_manager);
    }

    /// Probe run 2026-10-11b, WebView2's real order: the 401 response and its commit come BEFORE the
    /// challenge (Chromium commits the server's 401 page and then asks), the navigation then ends
    /// `ok=false` (status 17, credentials required) while the deferral is held, and the person's answer
    /// is retried as a NEW navigation to the same URL, whose response decides. The sign-in must still
    /// be the main document's (lookup, Remember, Password Manager), a remembered credential persists
    /// after the retry's 200, a rejected stored one is flagged when the retry is challenged again, and
    /// a Cancel ends the navigation (a later challenge is no longer the main document's).
    #[test]
    fn webview2_order_signs_in_and_remembers() {
        let url = "https://a.b/";
        let begin = |p: &mut AuthPane, nav: u64| {
            assert!(p.starting(nav, url, false).is_empty());
            assert!(p.response(nav, 401).is_empty());
            assert!(p.committed(nav).is_empty());
        };
        // Typed with Remember: persisted once the retry's 200 arrives, never before.
        let mut p = AuthPane::new(1, 1, true);
        begin(&mut p, 2);
        let acts = p.challenge(81, url, BASIC, &cx(false, true));
        let id = lookup_id(&acts);
        p.looked(id, None);
        let (_, req) = p.want.take().expect("the prompt");
        assert!(req.offer_remember && req.offer_password_manager, "the main document's sign-in offers Remember and the manager: {req:?}");
        assert!(p.completed(2, false).is_empty(), "the 'credentials required' end cancels nothing");
        assert!(p.holds(id));
        let acts = p.prompt_answer(id, typed("u", "p", true));
        assert_eq!(answers(&acts), vec![(81, true)]);
        assert!(p.starting(3, url, false).is_empty(), "the engine's retry is the same sign-in");
        let acts = p.response(3, 200);
        assert!(matches!(acts.as_slice(), [Action::Persist { .. }]), "{acts:?}");
        assert!(p.committed(3).is_empty() && p.completed(3, true).is_empty());
        // The person answers before the navigation's end arrives: the same.
        let mut p = AuthPane::new(1, 1, true);
        begin(&mut p, 2);
        let id = lookup_id(&p.challenge(82, url, BASIC, &cx(false, true)));
        p.looked(id, None);
        p.want.take();
        assert_eq!(answers(&p.prompt_answer(id, typed("u", "p", true))), vec![(82, true)]);
        assert!(p.completed(2, false).is_empty());
        p.starting(3, url, false);
        assert!(matches!(p.response(3, 200).as_slice(), [Action::Persist { .. }]));
        // A saved credential the server rejects: the retry is challenged again and the saved one is
        // never prefilled again (Task 8 I1), whatever the order.
        let mut p = AuthPane::new(1, 1, true);
        begin(&mut p, 2);
        let id = lookup_id(&p.challenge(83, url, BASIC, &cx(false, true)));
        p.looked(id, Some(("saved".into(), Secret::new("old".into()))));
        p.want.take();
        p.completed(2, false);
        assert_eq!(answers(&p.prompt_answer(id, typed("saved", "old", false))), vec![(83, true)]);
        begin(&mut p, 3);
        let acts = p.challenge(84, url, BASIC, &cx(false, true));
        assert!(!acts.iter().any(|a| matches!(a, Action::Lookup { .. })), "a rejected saved credential is not looked up again: {acts:?}");
        let (_, req) = p.want.take().expect("asked again");
        assert!(req.prefill.as_ref().is_none_or(|c| c.password.expose().is_empty()), "the rejected password is not prefilled");
        // Cancel: the 401 page stays; the navigation is over, a later challenge is not the main one.
        let mut p = AuthPane::new(1, 1, true);
        begin(&mut p, 2);
        let id = lookup_id(&p.challenge(85, url, BASIC, &cx(false, true)));
        p.looked(id, None);
        p.want.take();
        p.completed(2, false);
        assert_eq!(answers(&p.prompt_answer(id, AuthAnswer::Cancel)), vec![(85, false)]);
        assert!(p.starting(3, url, false).is_empty(), "a later load of the same URL is a new navigation");
        let acts = p.response(3, 200);
        assert!(acts.is_empty(), "nothing awaits: {acts:?}");
        // A navigation elsewhere after the supply is a new one: the credential is not persisted.
        let mut p = AuthPane::new(1, 1, true);
        begin(&mut p, 2);
        let id = lookup_id(&p.challenge(86, url, BASIC, &cx(false, true)));
        p.looked(id, None);
        p.want.take();
        p.completed(2, false);
        p.prompt_answer(id, typed("u", "p", true));
        p.starting(3, "https://a.b/other", false);
        assert!(p.response(3, 200).is_empty(), "another URL is not the retry");
    }

    /// Final review M3: the retry continues the coordinator's awaiting credential, so once that is gone
    /// (the navigation was parked, or the world reset) the next load of the same URL is a new
    /// navigation, never the retry.
    #[test]
    fn the_retry_is_dropped_with_the_credential_it_continues() {
        let url = "https://a.b/";
        for end in ["parked", "reset"] {
            let mut p = AuthPane::new(1, 1, true);
            p.starting(2, url, false);
            p.response(2, 401);
            p.committed(2);
            let id = lookup_id(&p.challenge(87, url, BASIC, &cx(false, true)));
            p.looked(id, None);
            p.want.take();
            p.completed(2, false);
            assert_eq!(answers(&p.prompt_answer(id, typed("u", "p", true))), vec![(87, true)]);
            if end == "parked" {
                p.parked();
            } else {
                p.feed(ba::Event::Reset);
            }
            assert_eq!(p.tracker.starting(3, url, false), vec![ba::Event::MainProvisionalStarted(ba::origin_of(url))], "{end}: the same URL is a new navigation");
        }
    }

    /// Digest, NTLM and Negotiate are prompted, never remembered or looked up.
    #[test]
    fn challenge_kinds_follow_the_rules() {
        for scheme in ["Digest", "NTLM", "Negotiate"] {
            let mut p = pane();
            let acts = p.challenge(51, "https://a.b/", &format!("{scheme} realm=\"R\""), &cx(false, true));
            let (id, req) = p.want.take().map(|w| (w.0, w.1)).unwrap_or_else(|| {
                let id = lookup_id(&acts);
                p.looked(id, None);
                p.want.take().unwrap()
            });
            let _ = id;
            assert!(!req.offer_remember, "{scheme} is never remembered");
        }
    }

    /// Persist unlocks the browser store only.
    #[test]
    fn persist_unlocks_only_the_browser_store() {
        let mut asked = Vec::new();
        assert!(authorize_save(true, &mut |s| {
            asked.push(s);
            true
        }));
        assert_eq!(asked, vec![Scope::Browser]);
        let mut asked = Vec::new();
        assert!(authorize_save(false, &mut |s| {
            asked.push(s);
            true
        }));
        assert!(asked.is_empty(), "an unlocked store is not asked");
        assert!(!authorize_save(true, &mut |_| false), "a refused authentication saves nothing");
    }

    /// A picker that ends with nothing prompts again with what was typed, Remember off.
    #[test]
    fn picker_follows_iterm2() {
        let mut p = pane();
        let id = lookup_id(&p.challenge(61, "https://a.b/", BASIC, &cx(false, true)));
        p.looked(id, None);
        p.want.take();
        let acts = p.prompt_answer(id, AuthAnswer::PasswordManager { user: "typed".into(), password: Secret::new("tp".into()) });
        assert!(acts.iter().any(|a| matches!(a, Action::Picker { id: i, .. } if *i == id)));
        let acts = p.picked(id, None);
        assert!(answers(&acts).is_empty());
        let (_, req) = p.want.take().expect("the prompt comes back");
        let pre = req.prefill.expect("with what was typed");
        assert_eq!((pre.user.as_str(), pre.password.expose()), ("typed", "tp"));
        assert!(!req.remember_by_default);
        // A pick answers the token once.
        p.prompt_answer(id, AuthAnswer::PasswordManager { user: String::new(), password: Secret::new(String::new()) });
        assert_eq!(answers(&p.picked(id, Some(("u".into(), Secret::new("p".into()))))), vec![(61, true)]);
        assert!(answers(&p.picked(id, None)).is_empty());
    }

    /// A lookup the store never answers ends the challenges (Cancel), and its late result is nothing.
    #[test]
    fn a_stuck_lookup_cancels() {
        let mut p = pane();
        let id = lookup_id(&p.challenge(71, "https://a.b/", BASIC, &cx(false, true)));
        assert!(p.expired(Instant::now()).is_empty());
        let acts = p.expired(Instant::now() + LOOKUP_WAIT + Duration::from_secs(1));
        assert_eq!(answers(&acts), vec![(71, false)]);
        assert!(answers(&p.looked(id, Some(("u".into(), Secret::new("p".into()))))).is_empty() && p.want.is_none());
    }

    /// A dropped coordinator hands its tokens to the owner to cancel.
    #[test]
    fn a_dropped_pane_leaves_no_deferral() {
        let tag = 9_000_001u64;
        let mut p = AuthPane::new(7_001, 7_001, true);
        p.starting(1, "https://a.b/", false);
        let id = lookup_id(&p.challenge(tag, "https://a.b/", BASIC, &cx(false, true)));
        assert!(p.holds(id));
        drop(p);
        let dropped = DROPPED.lock().unwrap().iter().any(|(pane, view, token)| (*pane, *view, *token) == (7_001, 7_001, tag));
        assert!(dropped);
        DROPPED.lock().unwrap().retain(|(pane, _, _)| *pane != 7_001);
    }

    /// The wiring: every exit of the pane reaches the cancel path, in the files that own them.
    #[test]
    fn the_wiring_reaches_every_exit() {
        let prod = |s: &'static str| &s[..s.find("#[cfg(test)]").unwrap_or(s.len())];
        let app = prod(include_str!("browser_app.rs"));
        assert!(app.contains("self.basic_auth_event(pane, e);"));
        assert!(app.contains("self.basic_auth_visible(pane, true);") && app.contains("self.basic_auth_visible(pane, false);"));
        assert!(app.contains("self.basic_auth_pane_gone(pane);"));
        assert!(prod(include_str!("browser_scripts_app.rs")).contains("self.basic_auth_reset(pane);"));
        assert!(include_str!("mod.rs").contains("self.basic_auth_window_event(id, &event)") && include_str!("mod.rs").contains("self.basic_auth_tick(event_loop)"));
        let setup = app;
        assert!(setup.contains("main_responses: true") && setup.contains("basic_auth: true"));
        let mine = prod(include_str!("browser_basic_auth_app.rs"));
        assert!(!mine.contains("log!"), "nothing is logged");
        assert!(mine.contains("DROPPED"), "a dropped coordinator is answered");
    }
}
