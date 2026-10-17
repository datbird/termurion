//! The HTTP authentication coordinator of a browser pane (13x7 Task 8): a pure state machine, a
//! port of iTerm2's `BrowserBasicAuthCoordinator` (BAC:87-448) for WebView2's
//! `BasicAuthenticationRequested`, with Termurion's fixes (pre-flight B5, B6, P10, S3, S4, S8).
//! It is driven by `Event`s (the owner translates WebView2's navigation events and the async
//! results of its effects) and returns `Effect`s; it never touches WebView2, the store or a
//! window. One `Coordinator` belongs to one pane's view and goes with it.
//!
//! Guarantees (iTerm2's seven, kept):
//! 1. Exactly one terminal decision per challenge: `Supply` or `Cancel` (`DefaultHandling` is
//!    never emitted; WebView2's own dialog is never shown), so a deferral never hangs. A
//!    challenge the coordinator refuses (hidden pane, over a cap, too long) is answered `Cancel`
//!    at once, and a result for an id that is already answered gives nothing.
//! 2. No synthetic HTTP. Acceptance is read only from the person's own main-frame response.
//! 3. A remembered credential is persisted only after a 2xx main-frame response for its origin,
//!    and never a credential that came from the store.
//! 4. A store-derived credential (saved or picked) seen rejected this session is never offered
//!    again as the prefill-by-default; a rejection is a re-challenge of a space we supplied this
//!    navigation, or a main-frame 401.
//! 5. Correlation is origin-exact and single-space (the ambiguity guard, flaw A).
//! 6. `last_entered` is prompt prefill only.
//! 7. A superseded navigation is a no-op: the awaiting slot is cleared on every fresh start.
//!
//! Termurion's additions (rulings R13x78-1 to R13x78-6):
//! - NO silent credential supply, ever. A saved credential is only the PREFILL of the prompt the
//!   person answers; `Supply` follows a `PromptResult::Submitted` or a `PickerResult` and nothing
//!   else. A page that makes a cross-origin request therefore cannot learn which sites have a
//!   saved credential (B6), and a challenge over `http` is never looked up.
//! - A challenge is held by the pane that raised it, bound to its navigation epoch; a fresh
//!   navigation, a failed one, a reset (`Event::Reset`: pane close, `browser_reset_world`) or the
//!   pane being hidden cancels every challenge it still holds (`Cancel`, and `ClosePrompt` for
//!   the open one). Only a shown pane is asked; a challenge that arrives while hidden is cancelled.
//! - One open prompt per pane, at most `MAX_QUEUED` waiting, `MAX_PER_NAV` challenges per
//!   navigation, and the pending count and rate of `webhost::PermissionLimiter` (the one owner of
//!   WebView2 deferral caps, S4); the rest are cancelled.
//! - A proxy challenge (`is_proxy`) is prompt-only, never main, never looked up or saved, and its
//!   request names the proxy (the space host is the proxy's). A challenge over `http` is prompted
//!   with `cleartext` set (the password crosses the network unencrypted) and offers nothing to
//!   remember; remembering needs `https`, the only origin the browser store matches.
//! - Secrets: a `Credential` is a `Secret` (wiped on drop); the coordinator keeps at most one
//!   awaiting credential, one `last_entered` per space (at most `MAX_SPACES` spaces) and one saved
//!   prefill per held challenge, and drops them on every terminal path. Nothing is logged.
//!
//! Accepted flaws (iTerm2's, same on WebView2): A. the challenge carries no frame identity, so
//! "main document" means "main navigation still provisional and the origin equals it"; a
//! same-origin subframe is trusted like the page (the ambiguity guard stops a wrong persist).
//! B. Subframes and cross-origin requests are prompt-only. C. WebView2 owns its credential
//! cache. D. A redirect chain delivers only the final main-frame response.

use std::collections::{HashMap, HashSet, VecDeque};
use std::time::Instant;

use crate::pwm::Secret;

/// Challenges held at once beyond the open prompt (the rest are cancelled).
pub const MAX_QUEUED: usize = 4;
/// After the person cancels a prompt, the same origin's challenges are cancelled at once this long.
pub const COOLDOWN: std::time::Duration = std::time::Duration::from_millis(crate::cooldown::SPAN_MS);
/// Challenges accepted per navigation epoch (the rest are cancelled).
pub const MAX_PER_NAV: usize = 16;
/// Protection spaces remembered per pane (rejection flags and prompt prefill).
pub const MAX_SPACES: usize = 16;
/// A realm longer than this (characters) is refused, never shown cut.
pub const MAX_REALM: usize = 256;
/// A host longer than this (characters) is refused.
pub const MAX_HOST: usize = 253;
/// The realm as the person reads it, and the host.
pub const SHOWN_REALM: usize = 128;
pub const SHOWN_ORIGIN: usize = 60;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Origin {
    pub scheme: String,
    pub host: String,
    pub port: u16,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SpaceKey {
    pub scheme: String,
    pub host: String,
    pub port: u16,
    pub realm: String,
}

impl SpaceKey {
    pub fn origin(&self) -> Origin {
        Origin { scheme: self.scheme.clone(), host: self.host.clone(), port: self.port }
    }

    /// The server's realm as the person reads it: hidden and bidi characters dropped, at most
    /// `SHOWN_REALM` characters (B5a). Always shown quoted and AFTER the host.
    pub fn shown_realm(&self) -> String {
        crate::inert_files::display_text(&self.realm, SHOWN_REALM, false)
    }

    /// `scheme://host[:port]` as the person reads it: the scheme and the port are always shown, a
    /// long host is cut from the left so its END stays (the `browser_perms::origin_label` rule, S5).
    pub fn shown_origin(&self) -> String {
        let host = crate::inert_files::display_text(&self.host, MAX_HOST, false);
        let default = (self.scheme == "https" && self.port == 443) || (self.scheme == "http" && self.port == 80);
        let port = if default { String::new() } else { format!(":{}", self.port) };
        crate::browser_perms::origin_label(&format!("{}://{}{}", self.scheme, host, port), SHOWN_ORIGIN)
    }
}

/// A user name and password. The password is wiped on drop; `Clone` makes a counted copy.
pub struct Credential {
    pub user: String,
    pub password: Secret,
}

impl Clone for Credential {
    fn clone(&self) -> Credential {
        Credential { user: self.user.clone(), password: self.password.duplicate() }
    }
}

impl PartialEq for Credential {
    fn eq(&self, o: &Credential) -> bool {
        self.user == o.user && self.password.expose() == o.password.expose()
    }
}

impl std::fmt::Debug for Credential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Credential(<redacted>)")
    }
}

/// Where a supplied credential came from. A typed rejection marks nothing; a saved or picked one
/// marks the space.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Saved,
    Typed,
    Picked,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ChallengeId(pub u64);

#[derive(Debug, PartialEq)]
pub enum Event {
    MainProvisionalStarted(Option<Origin>),
    MainFrameRedirect(Option<Origin>),
    MainFrameResponse { origin: Origin, status: u16 },
    MainCommitted,
    MainEndedWithoutResponse,
    /// A challenge from any frame. `is_proxy`: a `Proxy-Authenticate` challenge (its `space` names
    /// the proxy). `saved_lookup_possible`: the store is readable without an interactive prompt.
    Challenge {
        id: ChallengeId,
        space: SpaceKey,
        is_basic: bool,
        is_proxy: bool,
        proposed_user: Option<String>,
        saved_lookup_possible: bool,
        store_can_remember: bool,
    },
    SavedLookupResult { id: ChallengeId, credential: Option<Credential> },
    PromptResult { id: ChallengeId, outcome: PromptOutcome },
    PickerResult { id: ChallengeId, chosen: Option<Credential>, typed_fallback: Option<Credential> },
    SaveResult { space: SpaceKey, host: String, success: bool },
    /// The pane is (not) the one the person is looking at. Hidden: every held challenge is
    /// cancelled, and one that arrives while hidden is cancelled.
    Visible(bool),
    /// The pane closed or its world was reset (`browser_reset_world`): everything is cancelled and
    /// forgotten, secrets dropped.
    Reset,
}

#[derive(Debug, PartialEq)]
pub enum PromptOutcome {
    Cancelled,
    Submitted { credential: Credential, remember: bool },
    OpenPasswordManager(Option<Credential>),
}

#[derive(Debug, PartialEq)]
pub enum Effect {
    Supply(ChallengeId, Credential),
    Cancel(ChallengeId),
    DefaultHandling(ChallengeId),
    LookupSaved { id: ChallengeId, space: SpaceKey, preferred_user: Option<String> },
    Prompt(ChallengeId, PromptRequest),
    ShowPicker { id: ChallengeId, space: SpaceKey, typed: Option<Credential> },
    /// Close the prompt or picker window of this challenge (always followed by its `Cancel`).
    ClosePrompt(ChallengeId),
    Persist(SpaceKey, Credential),
    Toast(String),
}

#[derive(Debug, PartialEq)]
pub struct PromptRequest {
    pub space: SpaceKey,
    pub prefill: Option<Credential>,
    pub offer_remember: bool,
    pub offer_password_manager: bool,
    pub remember_by_default: bool,
    /// The password would cross the network unencrypted (Basic over http): the prompt says so.
    pub cleartext: bool,
    /// A proxy asks: the prompt names the proxy (`space.host`), not the page.
    pub proxy: bool,
}

pub fn save_failed_text(host: &str) -> String {
    format!("Could not save the password for {}", crate::inert_files::display_text(host, MAX_HOST, false))
}

#[derive(Default)]
struct SpaceState {
    saved_rejected_this_session: bool,
    last_entered: Option<Credential>,
}

/// The single main-document credential supplied and awaiting its main-frame response.
struct Awaiting {
    space: SpaceKey,
    credential: Credential,
    source: Source,
    remember: bool,
}

/// One held challenge, bound to the navigation epoch it arrived in.
struct Ctx {
    space: SpaceKey,
    is_basic: bool,
    is_proxy: bool,
    is_main: bool,
    /// The prompt may offer Remember and the Password Manager (main, Basic, https, not a proxy,
    /// and a store that can remember).
    can_store: bool,
    epoch: u64,
    /// A saved lookup is outstanding.
    looking: bool,
    /// The picker window is open for this challenge.
    in_picker: bool,
    /// What the store returned: the prompt's prefill, never a supply.
    saved_prefill: Option<Credential>,
}

#[derive(Default)]
pub struct Coordinator {
    spaces: HashMap<SpaceKey, SpaceState>,
    awaiting: Option<Awaiting>,
    main_spaces: HashSet<SpaceKey>,
    main_provisional_origin: Option<Origin>,
    main_provisional: bool,
    contexts: HashMap<ChallengeId, Ctx>,
    /// The challenge whose prompt (or picker) is on screen.
    open: Option<ChallengeId>,
    waiting: VecDeque<ChallengeId>,
    /// The pane is shown only once it says so (fail closed: a new coordinator asks nothing).
    shown: bool,
    epoch: u64,
    /// Origins whose prompt the person cancelled, and when: a page that reloads itself into a 401
    /// cannot bring the prompt back for `COOLDOWN` (M1).
    cancelled: crate::cooldown::Cooldown<Origin>,
    /// Told what a coordinator dropped without `Reset` still held, answered Cancel (set by the owner).
    drop_sink: Option<Box<dyn FnMut(Vec<Effect>)>>,
    nav_count: usize,
    limiter: crate::webhost::PermissionLimiter,
    /// Tests that cancel and re-challenge at once switch the cooldown off.
    #[cfg(test)]
    no_cooldown: bool,
}

impl Coordinator {
    pub fn reduce(&mut self, e: Event) -> Vec<Effect> {
        self.reduce_at(e, Instant::now())
    }

    /// Tells `f` what a coordinator dropped without a `Reset` still held, answered Cancel (the owner
    /// delivers each to WebView2, so no deferral is left hanging).
    pub fn on_drop(&mut self, f: Box<dyn FnMut(Vec<Effect>)>) {
        self.drop_sink = Some(f);
    }

    /// Challenges held now (a lookup, the open prompt, the queue).
    pub fn pending(&self) -> usize {
        self.contexts.len()
    }

    /// Protection spaces with a remembered flag or prefill.
    pub fn remembered(&self) -> usize {
        self.spaces.len()
    }

    pub fn has_awaiting(&self) -> bool {
        self.awaiting.is_some()
    }

    /// `reduce` with the clock given (the rate limit reads it).
    pub fn reduce_at(&mut self, e: Event, now: Instant) -> Vec<Effect> {
        match e {
            Event::MainProvisionalStarted(origin) => {
                self.main_provisional = true;
                self.main_provisional_origin = origin;
                self.awaiting = None;
                self.main_spaces.clear();
                self.new_epoch()
            }
            Event::MainFrameRedirect(origin) => {
                // Same navigation, new target: a pending supply stays, the chain's spaces accumulate.
                self.main_provisional = true;
                self.main_provisional_origin = origin;
                Vec::new()
            }
            Event::MainCommitted => {
                self.main_provisional = false;
                self.main_provisional_origin = None;
                self.awaiting = None;
                self.main_spaces.clear();
                Vec::new()
            }
            Event::MainEndedWithoutResponse => {
                self.main_provisional = false;
                self.main_provisional_origin = None;
                self.awaiting = None;
                self.main_spaces.clear();
                self.new_epoch()
            }
            Event::Visible(v) => {
                self.shown = v;
                if v {
                    Vec::new()
                } else {
                    self.cancel_all()
                }
            }
            Event::Reset => {
                self.main_provisional = false;
                self.main_provisional_origin = None;
                self.awaiting = None;
                self.main_spaces.clear();
                let fx = self.new_epoch();
                self.spaces.clear();
                self.cancelled.clear();
                fx
            }
            Event::Challenge { id, space, is_basic, is_proxy, proposed_user, saved_lookup_possible, store_can_remember } => {
                self.challenge(id, space, is_basic, is_proxy, proposed_user, saved_lookup_possible, store_can_remember, now)
            }
            Event::SavedLookupResult { id, credential } => {
                let Some(ctx) = self.contexts.get_mut(&id) else { return Vec::new() };
                if !ctx.looking || ctx.epoch != self.epoch {
                    return Vec::new();
                }
                ctx.looking = false;
                if !self.is_live(id) {
                    // The page moved on while the lookup ran: answer, never prompt for it.
                    return self.cancel_one(id);
                }
                if let Some(ctx) = self.contexts.get_mut(&id) {
                    ctx.saved_prefill = credential;
                }
                self.ready(id)
            }
            Event::PromptResult { id, outcome } => {
                if !self.is_open(id) || self.contexts.get(&id).is_some_and(|c| c.in_picker) {
                    return Vec::new();
                }
                match outcome {
                    PromptOutcome::Cancelled => {
                        let origin = self.contexts[&id].space.origin();
                        self.cancelled.start(origin, crate::cooldown::ms_of(now));
                        self.cancel_one(id)
                    }
                    PromptOutcome::Submitted { credential, remember } => {
                        let (from_store, can_store) = {
                            let ctx = &self.contexts[&id];
                            (ctx.saved_prefill.as_ref() == Some(&credential), ctx.can_store)
                        };
                        let space = self.contexts[&id].space.clone();
                        // Only what the person TYPED is remembered as a prefill; a stored credential has its own
                        // slot (`saved_prefill`) and never becomes "last typed" (I1).
                        if !from_store {
                            self.space_mut(&space).last_entered = Some(credential.clone());
                        }
                        let source = if from_store { Source::Saved } else { Source::Typed };
                        self.supply(id, credential, source, remember && can_store && !from_store)
                    }
                    PromptOutcome::OpenPasswordManager(typed) => {
                        let ctx = self.contexts.get_mut(&id).expect("open has a context");
                        if !ctx.can_store {
                            // The prompt never offered it: answer, fail closed.
                            return self.cancel_one(id);
                        }
                        ctx.in_picker = true;
                        vec![Effect::ShowPicker { id, space: ctx.space.clone(), typed }]
                    }
                }
            }
            Event::PickerResult { id, chosen, typed_fallback } => {
                if !self.is_open(id) || !self.contexts.get(&id).is_some_and(|c| c.in_picker) {
                    return Vec::new();
                }
                if let Some(ctx) = self.contexts.get_mut(&id) {
                    ctx.in_picker = false;
                }
                if let Some(chosen) = chosen.as_ref().filter(|c| !c.user.is_empty()).cloned() {
                    return self.supply(id, chosen, Source::Picked, false);
                }
                // Backed out, or an entry with no user name (Basic needs one): prompt again with
                // what is known, so the person does not retype it.
                let space = self.contexts[&id].space.clone();
                let last = self.spaces.get(&space).and_then(|s| s.last_entered.as_ref());
                let prefill = merge_prefill(chosen.as_ref(), typed_fallback.as_ref(), last);
                let offer = self.contexts[&id].can_store;
                let (cleartext, proxy) = {
                    let c = &self.contexts[&id];
                    (c.is_basic && c.space.scheme == "http", c.is_proxy)
                };
                vec![Effect::Prompt(
                    id,
                    PromptRequest { space, prefill, offer_remember: offer, offer_password_manager: offer, remember_by_default: false, cleartext, proxy },
                )]
            }
            Event::MainFrameResponse { origin, status } => {
                let Some(a) = self.awaiting.take().filter(|a| a.space.origin() == origin) else { return Vec::new() };
                // Ambiguity guard (flaw A): two spaces at this origin challenged as main this
                // navigation cannot be told apart by an origin-only response.
                if self.main_spaces.iter().filter(|s| s.origin() == origin).count() > 1 {
                    return Vec::new();
                }
                self.resolve(a, status)
            }
            Event::SaveResult { space, host, success } => {
                if success {
                    self.space_mut(&space).saved_rejected_this_session = false;
                    Vec::new()
                } else {
                    vec![Effect::Toast(save_failed_text(&host))]
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn challenge(
        &mut self,
        id: ChallengeId,
        space: SpaceKey,
        is_basic: bool,
        is_proxy: bool,
        proposed_user: Option<String>,
        saved_lookup_possible: bool,
        store_can_remember: bool,
        now: Instant,
    ) -> Vec<Effect> {
        if self.contexts.contains_key(&id) {
            return Vec::new();
        }
        // Refused, and answered at once: a hidden pane, server text over the caps, a flood.
        if !self.shown
            || (self.in_cooldown(&space.origin(), now))
            || space.realm.chars().count() > MAX_REALM
            || space.host.chars().count() > MAX_HOST
            || self.nav_count >= MAX_PER_NAV
            || !self.limiter.allow(now, self.contexts.len())
        {
            return vec![Effect::Cancel(id)];
        }
        self.nav_count += 1;
        let is_main = !is_proxy && self.main_provisional && self.main_provisional_origin.as_ref() == Some(&space.origin());
        let can_store = is_main && is_basic && store_can_remember && space.scheme == "https";
        self.contexts.insert(
            id,
            Ctx { space: space.clone(), is_basic, is_proxy, is_main, can_store, epoch: self.epoch, looking: false, in_picker: false, saved_prefill: None },
        );
        if is_main {
            self.main_spaces.insert(space.clone());
            if self.awaiting.as_ref().is_some_and(|a| a.space == space) {
                // The credential supplied for this space this navigation was rejected (the server
                // asks again): never offered as the default again.
                if let Some(a) = self.awaiting.take() {
                    self.rejected(&a.space, a.source);
                }
                return self.ready(id);
            }
        }
        if !can_store {
            return self.ready(id);
        }
        let rejected = self.spaces.get(&space).is_some_and(|s| s.saved_rejected_this_session);
        if saved_lookup_possible && !rejected {
            if let Some(c) = self.contexts.get_mut(&id) {
                c.looking = true;
            }
            return vec![Effect::LookupSaved { id, space, preferred_user: proposed_user }];
        }
        self.ready(id)
    }

    /// The challenge is ready to be asked: it opens if nothing is open, waits if the queue has
    /// room, else it is cancelled.
    fn ready(&mut self, id: ChallengeId) -> Vec<Effect> {
        if self.open.is_none() {
            self.open = Some(id);
            return vec![self.prompt(id)];
        }
        if self.waiting.len() >= MAX_QUEUED {
            self.contexts.remove(&id);
            return vec![Effect::Cancel(id)];
        }
        self.waiting.push_back(id);
        Vec::new()
    }

    fn in_cooldown(&self, origin: &Origin, now: Instant) -> bool {
        #[cfg(test)]
        if self.no_cooldown {
            return false;
        }
        self.cancelled.active(origin, crate::cooldown::ms_of(now))
    }

    fn is_open(&self, id: ChallengeId) -> bool {
        self.open == Some(id) && self.contexts.get(&id).is_some_and(|c| c.epoch == self.epoch)
    }

    /// The challenge is still the live main provisional navigation's.
    fn is_live(&self, id: ChallengeId) -> bool {
        self.contexts.get(&id).is_some_and(|c| c.is_main && self.main_provisional && self.main_provisional_origin.as_ref() == Some(&c.space.origin()))
    }

    fn prompt(&self, id: ChallengeId) -> Effect {
        let ctx = &self.contexts[&id];
        let state = self.spaces.get(&ctx.space);
        let prefill = ctx.saved_prefill.clone().or_else(|| state.and_then(|s| s.last_entered.clone()));
        // Remember starts on only to replace a saved credential the server rejected (not because
        // the person typed one earlier with Remember off: that was a refusal).
        let remember_by_default = false;
        Effect::Prompt(
            id,
            PromptRequest {
                space: ctx.space.clone(),
                prefill,
                offer_remember: ctx.can_store,
                offer_password_manager: ctx.can_store,
                remember_by_default,
                cleartext: ctx.is_basic && ctx.space.scheme == "http",
                proxy: ctx.is_proxy,
            },
        )
    }

    /// The challenge is answered: forget it, free its slot and open the next waiting one.
    fn forget(&mut self, id: ChallengeId) -> Vec<Effect> {
        self.contexts.remove(&id);
        self.waiting.retain(|w| *w != id);
        if self.open == Some(id) {
            self.open = None;
        }
        while self.shown && self.open.is_none() {
            let Some(next) = self.waiting.pop_front() else { break };
            if self.contexts.contains_key(&next) {
                self.open = Some(next);
                return vec![self.prompt(next)];
            }
        }
        Vec::new()
    }

    fn cancel_one(&mut self, id: ChallengeId) -> Vec<Effect> {
        let mut fx = vec![Effect::Cancel(id)];
        fx.extend(self.forget(id));
        fx
    }

    /// Every held challenge is answered Cancel (the open one's window closed first).
    fn cancel_all(&mut self) -> Vec<Effect> {
        let mut ids: Vec<ChallengeId> = self.contexts.keys().copied().collect();
        ids.sort_by_key(|i| i.0);
        let mut fx = Vec::new();
        for id in ids {
            if self.open == Some(id) {
                fx.push(Effect::ClosePrompt(id));
            }
            fx.push(Effect::Cancel(id));
        }
        self.contexts.clear();
        self.waiting.clear();
        self.open = None;
        fx
    }

    /// A new navigation epoch: what the old one held is cancelled.
    fn new_epoch(&mut self) -> Vec<Effect> {
        let fx = self.cancel_all();
        self.epoch += 1;
        self.nav_count = 0;
        fx
    }

    fn space_mut(&mut self, key: &SpaceKey) -> &mut SpaceState {
        if !self.spaces.contains_key(key) && self.spaces.len() >= MAX_SPACES
            && let Some(old) = self.spaces.keys().next().cloned() {
                self.spaces.remove(&old);
            }
        self.spaces.entry(key.clone()).or_default()
    }

    /// Answer a challenge with the credential the person gave or picked.
    fn supply(&mut self, id: ChallengeId, credential: Credential, source: Source, remember: bool) -> Vec<Effect> {
        let live = self.is_live(id);
        let space = self.contexts[&id].space.clone();
        if live {
            self.awaiting = Some(Awaiting { space, credential: credential.clone(), source, remember });
        }
        let mut fx = vec![Effect::Supply(id, credential)];
        fx.extend(self.forget(id));
        fx
    }

    /// The server rejected a credential. A stored one (saved or picked) is never offered or resent: the
    /// space is flagged, and the prefill keeps at most the user name (I1).
    fn rejected(&mut self, space: &SpaceKey, source: Source) {
        if source == Source::Typed {
            return;
        }
        let st = self.space_mut(space);
        st.saved_rejected_this_session = true;
        if let Some(last) = st.last_entered.as_mut() {
            last.password = Secret::new(String::new());
        }
    }

    fn resolve(&mut self, a: Awaiting, status: u16) -> Vec<Effect> {
        if status == 401 {
            self.rejected(&a.space, a.source);
            return Vec::new();
        }
        // Any other status means the sign-in itself worked. Only a saved credential's success
        // clears the flag (a typed or picked one says nothing about the stored one).
        if a.source == Source::Saved {
            self.space_mut(&a.space).saved_rejected_this_session = false;
        }
        if a.remember && (200..300).contains(&status) {
            return vec![Effect::Persist(a.space, a.credential)];
        }
        Vec::new()
    }
}

impl Drop for Coordinator {
    fn drop(&mut self) {
        if let Some(mut f) = self.drop_sink.take() {
            let fx = self.cancel_all();
            if !fx.is_empty() {
                f(fx);
            }
        }
    }
}

fn non_empty(s: Option<&str>) -> Option<&str> {
    s.filter(|s| !s.is_empty())
}

/// The picker re-prompt's prefill: typed user first, picked password first.
fn merge_prefill(picked: Option<&Credential>, typed: Option<&Credential>, last: Option<&Credential>) -> Option<Credential> {
    let user = non_empty(typed.map(|c| c.user.as_str())).or(non_empty(picked.map(|c| c.user.as_str()))).or(non_empty(last.map(|c| c.user.as_str())));
    let password = non_empty(picked.map(|c| c.password.expose()))
        .or(non_empty(typed.map(|c| c.password.expose())))
        .or(non_empty(last.map(|c| c.password.expose())));
    if user.is_none() && password.is_none() {
        return None;
    }
    Some(Credential { user: user.unwrap_or("").to_string(), password: Secret::new(password.unwrap_or("").to_string()) })
}

// --- The store rules, texts and challenge helpers (Task 9) ---

pub const OK: &str = "OK";
pub const CANCEL: &str = "Cancel";
pub const PASSWORD_MANAGER: &str = "Password Manager";
pub const PASSWORD_PLACEHOLDER: &str = "Password";
pub const USER_PLACEHOLDER: &str = "User name";
pub const REMEMBER: &str = "Remember this password";

#[derive(Debug, PartialEq)]
pub enum SavePlan {
    SetPassword(crate::pwm::Account),
    Add,
}

/// The stored account name of a protection space (`BAS:71-78`): the scheme always, the port only
/// when it is not the scheme's default, then " (<realm>)" when there is a realm. The coordinator
/// refuses a realm over `MAX_REALM` and a host over `MAX_HOST`, so a name is at most about 530
/// characters; a store that cannot hold that refuses the Add and the person sees the failure toast.
pub fn account_name(scheme: &str, host: &str, port: u16, realm: &str) -> String {
    let default = (scheme == "https" && port == 443) || (scheme == "http" && port == 80);
    let mut name = format!("{scheme}://{host}");
    if !default {
        name.push_str(&format!(":{port}"));
    }
    if !realm.is_empty() {
        name.push_str(&format!(" ({realm})"));
    }
    name
}

/// The prompt's heading text (`BAM:228-233`). The inputs must already be inert (`display_text`).
pub fn prompt_text(host: &str, realm: &str) -> String {
    if realm.is_empty() {
        format!("The website \u{201c}{host}\u{201d} requires a user name and password.")
    } else {
        format!("The website \u{201c}{host}\u{201d} requires a user name and password for \u{201c}{realm}\u{201d}.")
    }
}

/// The server's realm is labelled as the site's own words, never Termurion's (B5a).
pub const REALM_LABEL: &str = "The site describes this sign-in as (text from the site, not from Termurion):";
pub const CLEARTEXT_WARNING: &str = "Warning: this connection is not encrypted. The password would be sent as plain text.";

/// The whole text of the sign-in prompt: the real origin (or the proxy) as the permission prompt
/// shows it, the realm only as clearly labelled untrusted text, and the cleartext warning (R13x78-5).
pub fn prompt_heading(req: &PromptRequest) -> String {
    let origin = req.space.shown_origin();
    let mut text = prompt_text(&origin, "");
    if req.proxy {
        text = text.replacen("The website", "The proxy", 1);
    }
    let realm = req.space.shown_realm();
    if !realm.is_empty() {
        text.push_str(&format!("\n\n{REALM_LABEL}\n\u{201c}{realm}\u{201d}"));
    }
    if req.cleartext {
        text.push_str(&format!("\n\n{CLEARTEXT_WARNING}"));
    }
    text
}

/// The scheme word and the `realm` parameter of a `WWW-Authenticate` or `Proxy-Authenticate`
/// challenge (RFC 7235): the parameter name is case-insensitive, the value a token or a quoted
/// string with backslash escapes; empty when absent. A quoted value that merely contains `realm=`
/// is not a parameter.
pub fn parse_challenge(header: &str) -> (String, String) {
    let h = header.trim_start();
    let end = h.find(|c: char| c.is_ascii_whitespace()).unwrap_or(h.len());
    let scheme = h[..end].to_string();
    let b = &h.as_bytes()[end..];
    let rest = &h[end..];
    let mut i = 0;
    while i < b.len() {
        while i < b.len() && (b[i].is_ascii_whitespace() || b[i] == b',') {
            i += 1;
        }
        let start = i;
        while i < b.len() && !matches!(b[i], b'=' | b',') && !b[i].is_ascii_whitespace() {
            i += 1;
        }
        let name = &rest[start..i];
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= b.len() || b[i] != b'=' {
            continue;
        }
        i += 1;
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        let mut value = String::new();
        if i < b.len() && b[i] == b'"' {
            i += 1;
            let mut chars = rest[i..].char_indices();
            let mut closed = i;
            while let Some((k, c)) = chars.next() {
                match c {
                    '\\' => {
                        if let Some((_, n)) = chars.next() {
                            value.push(n);
                        }
                    }
                    '"' => {
                        closed = i + k + 1;
                        break;
                    }
                    c => value.push(c),
                }
                closed = i + k + c.len_utf8();
            }
            i = closed;
        } else {
            let s0 = i;
            while i < b.len() && b[i] != b',' && !b[i].is_ascii_whitespace() {
                i += 1;
            }
            value = rest[s0..i].to_string();
        }
        if name.eq_ignore_ascii_case("realm") {
            return (scheme, value);
        }
    }
    (scheme, String::new())
}

/// `uri` without its fragment: the request for `/#frag` carries no fragment, so the two are the same
/// document (BAM:121-130).
pub fn strip_fragment(uri: &str) -> &str {
    uri.split_once('#').map_or(uri, |(head, _)| head)
}

/// WebView2's navigation events (13x7 Task 10) as the coordinator's main-frame events, by
/// `NavigationId`: only the live navigation counts, a superseded one answers nothing. WebView2 does
/// not order `WebResourceResponseReceived` before `ContentLoading` (review I8), so a commit is held
/// until the live navigation's final (non-3xx) response arrives or the navigation completes, and
/// the response is always reduced before the commit (a typed Remember persists on the response).
///
/// HTTP authentication in WebView2 (probe run 2026-10-11b) differs from WebKit's order: the 401 (or
/// 407) response and its commit come FIRST (Chromium commits the server's 401 page), then the
/// challenge, then the navigation ends `ok=false` while the deferral is still held, and a supplied
/// credential is retried as a NEW navigation to the same URL. So a 401/407 is not final: the sign-in
/// stays the live main navigation's (`auth_wait`) until it is answered. A supply arms the retry
/// (`supplied`): the next navigation to that URL continues the sign-in (`MainFrameRedirect`, which
/// keeps the coordinator's awaiting credential) and its response decides. Without a supply (Cancel, or
/// no challenge at all) the held 401 is the final response once nothing is held (`end_wait`).
#[derive(Default)]
pub struct NavTracker {
    live: Option<u64>,
    origin: Option<Origin>,
    /// The live navigation's URI without its fragment (the retry goes to the same URI).
    uri: String,
    /// The final response was reduced.
    responded: bool,
    /// The commit arrived and waits for the response.
    commit_held: bool,
    /// The commit was reduced.
    committed: bool,
    /// The navigation completed (a later response is nothing).
    done: bool,
    /// A 401/407 response held while its challenge may still be answered.
    auth_wait: Option<u16>,
    /// The navigation completed with `ok` while `auth_wait` (WebView2's "credentials required").
    ended_ok: Option<bool>,
    /// A credential was supplied for the held sign-in: the next navigation to `uri` is its retry.
    retry: bool,
}

impl NavTracker {
    pub fn starting(&mut self, nav: u64, uri: &str, redirect: bool) -> Vec<Event> {
        let origin = origin_of(uri);
        let bare = strip_fragment(uri).to_string();
        if redirect && self.live == Some(nav) && !self.done {
            self.origin = origin.clone();
            self.uri = bare;
            self.responded = false;
            self.auth_wait = None;
            return vec![Event::MainFrameRedirect(origin)];
        }
        if !redirect && self.retry && self.uri == bare && origin.is_some() {
            // The engine's retry of the sign-in: the same sign-in, a new navigation id.
            *self = NavTracker { live: Some(nav), origin: origin.clone(), uri: bare, ..NavTracker::default() };
            return vec![Event::MainFrameRedirect(origin)];
        }
        *self = NavTracker { live: Some(nav), origin: origin.clone(), uri: bare, ..NavTracker::default() };
        vec![Event::MainProvisionalStarted(origin)]
    }

    pub fn committed(&mut self, nav: u64) -> Vec<Event> {
        if self.live != Some(nav) || self.done || self.committed || self.commit_held {
            return Vec::new();
        }
        if self.responded || self.origin.is_none() {
            self.committed = true;
            return vec![Event::MainCommitted];
        }
        self.commit_held = true;
        Vec::new()
    }

    pub fn completed(&mut self, nav: u64, ok: bool) -> Vec<Event> {
        if self.live != Some(nav) || self.done {
            return Vec::new();
        }
        self.done = true;
        if self.auth_wait.is_some() {
            // Decided by `end_wait` (nothing held) or by the retry (a supply).
            self.ended_ok = Some(ok);
            return Vec::new();
        }
        if self.commit_held {
            self.commit_held = false;
            self.committed = true;
            return vec![Event::MainCommitted];
        }
        if !self.committed && !ok {
            return vec![Event::MainEndedWithoutResponse];
        }
        Vec::new()
    }

    pub fn response(&mut self, nav: u64, status: u16) -> Vec<Event> {
        if self.live != Some(nav) || self.done || self.responded || (300..400).contains(&status) {
            return Vec::new();
        }
        let Some(origin) = self.origin.clone() else { return Vec::new() };
        if status == 401 || status == 407 {
            // Not final yet: WebView2 asks after it (the commit stays held with it).
            self.auth_wait.get_or_insert(status);
            return Vec::new();
        }
        self.responded = true;
        let mut out = vec![Event::MainFrameResponse { origin, status }];
        if self.commit_held {
            self.commit_held = false;
            self.committed = true;
            out.push(Event::MainCommitted);
        }
        out
    }

    /// A credential was handed over while the live navigation's 401/407 is held: its retry follows.
    pub fn supplied(&mut self) {
        if self.auth_wait.is_some() {
            self.retry = true;
        }
    }

    /// The credential the retry would continue is gone (the coordinator holds no awaiting supply any
    /// more: the navigation was parked, the world reset): the next load of the same URL is a new one.
    pub fn retry_lost(&mut self) {
        self.retry = false;
    }

    /// The held 401/407 becomes the final response once the navigation completed, nothing is held
    /// any more (`held` false) and no credential was supplied: the server's own 401 page stays.
    pub fn end_wait(&mut self, held: bool) -> Vec<Event> {
        let (Some(status), Some(ok)) = (self.auth_wait, self.ended_ok) else { return Vec::new() };
        if held || self.retry {
            return Vec::new();
        }
        self.auth_wait = None;
        self.ended_ok = None;
        let mut out = Vec::new();
        if let Some(origin) = self.origin.clone() {
            self.responded = true;
            out.push(Event::MainFrameResponse { origin, status });
        }
        if self.commit_held {
            self.commit_held = false;
            self.committed = true;
            out.push(Event::MainCommitted);
        } else if !self.committed && !ok {
            out.push(Event::MainEndedWithoutResponse);
        }
        out
    }
}

/// `scheme://host[:port]` of an http or https URI, lower-cased, default ports 443 and 80; None for any
/// other scheme (iTerm2's default port there is -1, `BAM:325`), no host, a bad port or user information.
pub fn origin_of(uri: &str) -> Option<Origin> {
    let (scheme, rest) = uri.split_once("://")?;
    let scheme = scheme.to_ascii_lowercase();
    if scheme != "http" && scheme != "https" {
        return None;
    }
    let authority = &rest[..rest.find(['/', '?', '#']).unwrap_or(rest.len())];
    if authority.is_empty() || authority.contains('@') {
        return None;
    }
    let (host, port) = if authority.starts_with('[') {
        let end = authority.find(']')?;
        (&authority[..=end], authority[end + 1..].strip_prefix(':').unwrap_or(""))
    } else {
        authority.split_once(':').unwrap_or((authority, ""))
    };
    if host.is_empty() || !port.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let port = if port.is_empty() { if scheme == "https" { 443 } else { 80 } } else { port.parse::<u16>().ok()? };
    Some(Origin { scheme, host: host.to_ascii_lowercase(), port })
}

/// The protection space of a challenge at `uri` with `realm`.
pub fn space_of(uri: &str, realm: &str) -> Option<SpaceKey> {
    let o = origin_of(uri)?;
    Some(SpaceKey { scheme: o.scheme, host: o.host, port: o.port, realm: realm.to_string() })
}

/// Whether the challenge comes from the configured browser proxy (`BrowserProxyEnabled` with its host
/// and port): its host and port equal the proxy's, the host compared without case. `proxy` is None
/// when no proxy is set. The caller also treats every challenge as a proxy's while a proxy is on and
/// WebView2's reported URI is unproven (P14).
pub fn is_proxy_challenge(uri: &str, proxy: Option<(&str, u16)>) -> bool {
    let Some((host, port)) = proxy else { return false };
    origin_of(uri).is_some_and(|o| o.host.eq_ignore_ascii_case(host) && o.port == port)
}

/// Dev Null Mode never remembers (`BAS:41-48`).
pub fn can_remember(dev_null: bool) -> bool {
    !dev_null
}

/// The store may be read without any prompt (`BAS:87-89`).
pub fn unlocked_without_prompt(authenticated: bool, require_open: bool, require_every: bool) -> bool {
    !require_open || (authenticated && !require_every)
}

/// The first account with the wanted name, preferring the proposed user when it is non-empty
/// (`BAS:123-134`).
pub fn pick_saved<'a>(accounts: &'a [crate::pwm::Account], wanted: &str, preferred_user: Option<&str>) -> Option<&'a crate::pwm::Account> {
    let mut named = accounts.iter().filter(|a| a.account_name == wanted);
    let first = named.next()?;
    match preferred_user.filter(|u| !u.is_empty()) {
        Some(u) => accounts.iter().find(|a| a.account_name == wanted && a.user_name == u).or(Some(first)),
        None => Some(first),
    }
}

/// Saving (`BAS:172-187`): the password of an existing (name, user) account is replaced, else a new
/// account is added with no flags.
pub fn save_plan(accounts: &[crate::pwm::Account], name: &str, user: &str) -> SavePlan {
    match accounts.iter().find(|a| a.account_name == name && a.user_name == user) {
        Some(a) => SavePlan::SetPassword(a.clone()),
        None => SavePlan::Add,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn o() -> Origin {
        Origin { scheme: "https".into(), host: "a.b".into(), port: 443 }
    }
    fn quiet() -> Coordinator {
        let mut c = Coordinator::default();
        c.no_cooldown = true;
        c
    }
    fn sp(realm: &str) -> SpaceKey {
        SpaceKey { scheme: "https".into(), host: "a.b".into(), port: 443, realm: realm.into() }
    }
    fn cred(u: &str, p: &str) -> Credential {
        Credential { user: u.into(), password: Secret::new(p.into()) }
    }
    fn id(n: u64) -> ChallengeId {
        ChallengeId(n)
    }
    fn chal_with(n: u64, space: SpaceKey, basic: bool, proxy: bool, lookup: bool, store: bool) -> Event {
        Event::Challenge { id: id(n), space, is_basic: basic, is_proxy: proxy, proposed_user: None, saved_lookup_possible: lookup, store_can_remember: store }
    }
    fn chal(n: u64, realm: &str) -> Event {
        chal_with(n, sp(realm), true, false, true, true)
    }
    fn start(c: &mut Coordinator) {
        // A pane is not shown until it says so (I2): the tests' pane is on screen.
        c.reduce(Event::Visible(true));
        assert!(c.reduce(Event::MainProvisionalStarted(Some(o()))).is_empty());
    }
    fn submit(n: u64, u: &str, p: &str, remember: bool) -> Event {
        Event::PromptResult { id: id(n), outcome: PromptOutcome::Submitted { credential: cred(u, p), remember } }
    }
    fn only_prompt(fx: Vec<Effect>) -> (ChallengeId, PromptRequest) {
        assert_eq!(fx.len(), 1, "{fx:?}");
        match fx.into_iter().next() {
            Some(Effect::Prompt(i, r)) => (i, r),
            other => panic!("not a prompt: {other:?}"),
        }
    }
    /// A Basic main challenge whose saved lookup answers `saved`, then the prompt.
    fn to_prompt(c: &mut Coordinator, n: u64, realm: &str, saved: Option<Credential>) -> PromptRequest {
        let fx = c.reduce(chal(n, realm));
        assert!(matches!(fx.as_slice(), [Effect::LookupSaved { .. }]), "{fx:?}");
        only_prompt(c.reduce(Event::SavedLookupResult { id: id(n), credential: saved })).1
    }

    #[test]
    fn coordinator_follows_iterm2() {
        let mut c = quiet();
        start(&mut c);
        let fx = c.reduce(chal(1, "r"));
        assert_eq!(fx, vec![Effect::LookupSaved { id: id(1), space: sp("r"), preferred_user: None }]);
        // A saved credential only PREFILLS the prompt (no silent supply); the person submits it.
        let (_, req) = only_prompt(c.reduce(Event::SavedLookupResult { id: id(1), credential: Some(cred("u", "p")) }));
        assert_eq!(req.prefill, Some(cred("u", "p")));
        assert_eq!(c.reduce(submit(1, "u", "p", true)), vec![Effect::Supply(id(1), cred("u", "p"))]);
        // The credential came from the store: never re-persisted.
        assert!(c.reduce(Event::MainFrameResponse { origin: o(), status: 200 }).is_empty());
        // Typed, remember, 200: persisted.
        start(&mut c);
        to_prompt(&mut c, 2, "r2", None);
        assert_eq!(c.reduce(submit(2, "t", "w", true)), vec![Effect::Supply(id(2), cred("t", "w"))]);
        assert_eq!(c.reduce(Event::MainFrameResponse { origin: o(), status: 200 }), vec![Effect::Persist(sp("r2"), cred("t", "w"))]);
        // 302 persists nothing.
        start(&mut c);
        to_prompt(&mut c, 3, "r3", None);
        c.reduce(submit(3, "t", "w", true));
        assert!(c.reduce(Event::MainFrameResponse { origin: o(), status: 302 }).is_empty());
        // 401 persists nothing.
        start(&mut c);
        to_prompt(&mut c, 4, "r4", None);
        c.reduce(submit(4, "t", "w", true));
        assert!(c.reduce(Event::MainFrameResponse { origin: o(), status: 401 }).is_empty());
        // Remember off persists nothing (a new pane: the per-second limit is the pane's).
        let mut c = quiet();
        start(&mut c);
        to_prompt(&mut c, 5, "r5", None);
        c.reduce(submit(5, "t", "w", false));
        assert!(c.reduce(Event::MainFrameResponse { origin: o(), status: 200 }).is_empty());
    }

    #[test]
    fn rejected_saved_is_never_prefilled_by_default() {
        let mut c = quiet();
        start(&mut c);
        let req = to_prompt(&mut c, 1, "r", Some(cred("u", "bad")));
        assert!(!req.remember_by_default);
        c.reduce(submit(1, "u", "bad", false));
        // Re-challenged for the same space in the same navigation: the saved one was rejected.
        let (_, req) = only_prompt(c.reduce(chal(2, "r")));
        assert!(!req.remember_by_default && req.offer_remember, "Remember stays off after a rejection (I1)");
        assert!(req.prefill.is_none(), "a rejected saved credential is never prefilled again (I1)");
        c.reduce(Event::PromptResult { id: id(2), outcome: PromptOutcome::Cancelled });
        // A fresh navigation: no lookup, a prompt.
        start(&mut c);
        let (_, req) = only_prompt(c.reduce(chal(3, "r")));
        assert!(!req.remember_by_default && req.prefill.is_none());
        c.reduce(Event::PromptResult { id: id(3), outcome: PromptOutcome::Cancelled });
        // A saved save that landed re-enables lookups.
        assert!(c.reduce(Event::SaveResult { space: sp("r"), host: "a.b".into(), success: true }).is_empty());
        start(&mut c);
        assert!(matches!(c.reduce(chal(4, "r")).as_slice(), [Effect::LookupSaved { .. }]));
    }

    #[test]
    fn a_401_after_a_picked_supply_marks_the_space() {
        let mut c = quiet();
        start(&mut c);
        c.reduce(chal(1, "r"));
        c.reduce(Event::SavedLookupResult { id: id(1), credential: None });
        let fx = c.reduce(Event::PromptResult { id: id(1), outcome: PromptOutcome::OpenPasswordManager(None) });
        assert!(matches!(fx.as_slice(), [Effect::ShowPicker { .. }]));
        let fx = c.reduce(Event::PickerResult { id: id(1), chosen: Some(cred("u", "p")), typed_fallback: None });
        assert_eq!(fx, vec![Effect::Supply(id(1), cred("u", "p"))]);
        assert!(c.reduce(Event::MainFrameResponse { origin: o(), status: 401 }).is_empty());
        start(&mut c);
        let (_, req) = only_prompt(c.reduce(chal(2, "r")));
        assert!(!req.remember_by_default && req.prefill.is_none());
    }

    #[test]
    fn subframes_and_digest_prompt_only() {
        let off = |req: &PromptRequest| !req.offer_remember && !req.offer_password_manager;
        // Cross-origin during the main navigation, before commit.
        let mut c = quiet();
        start(&mut c);
        let other = SpaceKey { host: "x.y".into(), ..sp("r") };
        let (_, req) = only_prompt(c.reduce(chal_with(1, other.clone(), true, false, true, true)));
        assert!(off(&req) && !req.remember_by_default);
        // After commit, even for the page's own origin.
        c.reduce(Event::MainCommitted);
        c.reduce(Event::PromptResult { id: id(1), outcome: PromptOutcome::Cancelled });
        let (_, req) = only_prompt(c.reduce(chal(2, "r")));
        assert!(off(&req));
        c.reduce(Event::PromptResult { id: id(2), outcome: PromptOutcome::Cancelled });
        let (_, req) = only_prompt(c.reduce(chal_with(3, other, true, false, true, true)));
        assert!(off(&req));
        // Non-Basic main challenge.
        let mut c = quiet();
        start(&mut c);
        let (_, req) = only_prompt(c.reduce(chal_with(1, sp("r"), false, false, true, true)));
        assert!(off(&req));
        // Dev Null: the store cannot remember.
        let mut c = quiet();
        start(&mut c);
        let (_, req) = only_prompt(c.reduce(chal_with(1, sp("r"), true, false, true, false)));
        assert!(off(&req));
        // A cross-origin subframe never persists or marks anything: submit then a 200.
        let mut c = quiet();
        start(&mut c);
        let other = SpaceKey { host: "x.y".into(), ..sp("r") };
        c.reduce(chal_with(1, other, true, false, true, true));
        c.reduce(submit(1, "u", "p", true));
        assert!(c.reduce(Event::MainFrameResponse { origin: o(), status: 200 }).is_empty());
    }

    #[test]
    fn no_silent_supply_and_no_cleartext_lookup() {
        // The only effects that carry a credential to the server follow a person's answer.
        let mut c = quiet();
        start(&mut c);
        c.reduce(chal(1, "r"));
        let fx = c.reduce(Event::SavedLookupResult { id: id(1), credential: Some(cred("u", "p")) });
        assert!(fx.iter().all(|e| matches!(e, Effect::Prompt(..))), "{fx:?}");
        // Over http: no lookup, a prompt that says cleartext and offers nothing to remember.
        let mut c = quiet();
        let http_o = Origin { scheme: "http".into(), host: "a.b".into(), port: 80 };
        c.reduce(Event::Visible(true));
        c.reduce(Event::MainProvisionalStarted(Some(http_o)));
        let space = SpaceKey { scheme: "http".into(), port: 80, ..sp("r") };
        let (_, req) = only_prompt(c.reduce(chal_with(1, space, true, false, true, true)));
        assert!(req.cleartext && !req.offer_remember && !req.offer_password_manager);
        // Over https: not cleartext.
        let mut c = quiet();
        start(&mut c);
        c.reduce(chal(1, "r"));
        let req = only_prompt(c.reduce(Event::SavedLookupResult { id: id(1), credential: None })).1;
        assert!(!req.cleartext && !req.proxy);
    }

    #[test]
    fn a_proxy_challenge_names_the_proxy_and_is_prompt_only() {
        let mut c = quiet();
        start(&mut c);
        let proxy = SpaceKey { scheme: "http".into(), host: "a.b".into(), port: 8080, realm: "corp".into() };
        let (_, req) = only_prompt(c.reduce(chal_with(1, proxy.clone(), true, true, true, true)));
        assert!(req.proxy && req.space == proxy && !req.offer_remember && !req.offer_password_manager);
        // Never persisted, whatever follows.
        c.reduce(submit(1, "u", "p", true));
        assert!(c.reduce(Event::MainFrameResponse { origin: proxy.origin(), status: 200 }).is_empty());
        // Even when its origin equals the page's.
        let mut c = quiet();
        start(&mut c);
        let same = sp("corp");
        let (_, req) = only_prompt(c.reduce(chal_with(1, same, true, true, true, true)));
        assert!(req.proxy && !req.offer_remember);
    }

    #[test]
    fn answers_exactly_once() {
        let mut c = quiet();
        start(&mut c);
        to_prompt(&mut c, 1, "r", None);
        let cancelled = || Event::PromptResult { id: id(1), outcome: PromptOutcome::Cancelled };
        assert_eq!(c.reduce(cancelled()), vec![Effect::Cancel(id(1))]);
        assert!(c.reduce(cancelled()).is_empty());
        assert!(c.reduce(submit(1, "u", "p", false)).is_empty());
        assert!(c.reduce(Event::SavedLookupResult { id: id(1), credential: None }).is_empty());
        // A submitted answer is final too.
        to_prompt(&mut c, 2, "r", None);
        assert_eq!(c.reduce(submit(2, "u", "p", false)).len(), 1);
        assert!(c.reduce(submit(2, "u", "p", false)).is_empty());
        // A lookup answered twice prompts once.
        let mut c = quiet();
        start(&mut c);
        c.reduce(chal(1, "r"));
        assert_eq!(c.reduce(Event::SavedLookupResult { id: id(1), credential: None }).len(), 1);
        assert!(c.reduce(Event::SavedLookupResult { id: id(1), credential: Some(cred("u", "p")) }).is_empty());
        // The navigation moved on during the lookup: cancelled at the move, the late result is nothing.
        let mut c = quiet();
        start(&mut c);
        c.reduce(chal(1, "r"));
        assert_eq!(c.reduce(Event::MainProvisionalStarted(Some(o()))), vec![Effect::Cancel(id(1))]);
        assert!(c.reduce(Event::SavedLookupResult { id: id(1), credential: None }).is_empty());
        assert!(c.reduce(Event::SavedLookupResult { id: id(1), credential: Some(cred("u", "p")) }).is_empty());
    }

    #[test]
    fn the_picker_reprompts_with_merged_prefill() {
        let mut c = quiet();
        start(&mut c);
        to_prompt(&mut c, 1, "r", None);
        let fx = c.reduce(Event::PromptResult { id: id(1), outcome: PromptOutcome::OpenPasswordManager(Some(cred("typed", "tp"))) });
        assert!(matches!(fx.as_slice(), [Effect::ShowPicker { typed: Some(_), .. }]));
        // A picked entry with no user name: typed user first, picked password first.
        let (_, req) = only_prompt(c.reduce(Event::PickerResult { id: id(1), chosen: Some(cred("", "pw")), typed_fallback: Some(cred("typed", "tp")) }));
        assert_eq!(req.prefill, Some(cred("typed", "pw")));
        assert!(req.offer_remember && req.offer_password_manager && !req.remember_by_default, "the picker re-prompt starts with Remember off");
        // Backed out with nothing: no prefill.
        let fx = c.reduce(Event::PromptResult { id: id(1), outcome: PromptOutcome::OpenPasswordManager(None) });
        assert!(matches!(fx.as_slice(), [Effect::ShowPicker { .. }]));
        let (_, req) = only_prompt(c.reduce(Event::PickerResult { id: id(1), chosen: None, typed_fallback: None }));
        assert_eq!(req.prefill, None);
        // With the store unable to remember: every offer false.
        let mut c = quiet();
        start(&mut c);
        let (_, req) = only_prompt(c.reduce(chal_with(1, sp("r"), true, false, false, false)));
        assert!(!req.offer_remember && !req.offer_password_manager && !req.remember_by_default);
        // A stray picker answer supplies nothing.
        assert!(c.reduce(Event::PickerResult { id: id(1), chosen: Some(cred("u", "p")), typed_fallback: None }).is_empty());
        // The picker is only for a prompt that offered it.
        let fx = c.reduce(Event::PromptResult { id: id(1), outcome: PromptOutcome::OpenPasswordManager(None) });
        assert_eq!(fx, vec![Effect::Cancel(id(1))]);
    }

    #[test]
    fn ambiguity_guard_follows_iterm2() {
        let mut c = quiet();
        start(&mut c);
        to_prompt(&mut c, 1, "one", None);
        c.reduce(submit(1, "u", "p", true));
        // A second realm at the same origin challenged as main in the same navigation.
        to_prompt(&mut c, 2, "two", None);
        c.reduce(submit(2, "u2", "p2", true));
        assert!(c.reduce(Event::MainFrameResponse { origin: o(), status: 200 }).is_empty());
        // A redirect chain across two origins persists on the final origin.
        let mut c = quiet();
        c.reduce(Event::Visible(true));
        let o1 = Origin { host: "first.b".into(), ..o() };
        c.reduce(Event::MainProvisionalStarted(Some(o1.clone())));
        let s1 = SpaceKey { host: "first.b".into(), ..sp("one") };
        c.reduce(chal_with(1, s1, true, false, true, true));
        c.reduce(Event::SavedLookupResult { id: id(1), credential: None });
        c.reduce(submit(1, "u", "p", true));
        c.reduce(Event::MainFrameRedirect(Some(o())));
        to_prompt(&mut c, 2, "two", None);
        c.reduce(submit(2, "t", "w", true));
        assert_eq!(c.reduce(Event::MainFrameResponse { origin: o(), status: 200 }), vec![Effect::Persist(sp("two"), cred("t", "w"))]);
    }

    #[test]
    fn superseded_is_a_no_op() {
        let mut c = quiet();
        start(&mut c);
        to_prompt(&mut c, 1, "r", None);
        c.reduce(submit(1, "u", "p", true));
        let other = Origin { host: "c.d".into(), ..o() };
        c.reduce(Event::MainProvisionalStarted(Some(other)));
        assert!(c.reduce(Event::MainFrameResponse { origin: o(), status: 200 }).is_empty());
        assert_eq!(
            c.reduce(Event::SaveResult { space: sp("r"), host: "a.b".into(), success: false }),
            vec![Effect::Toast("Could not save the password for a.b".into())]
        );
        assert_eq!(save_failed_text("a\u{202e}b"), "Could not save the password for ab");
    }

    #[test]
    fn one_open_prompt_a_bounded_queue_and_the_rest_cancelled() {
        let mut c = quiet();
        start(&mut c);
        // Subframe-style challenges (other origin) so each is prompt-only and immediate.
        let mk = |n: u64| SpaceKey { host: format!("h{n}.x"), ..sp("r") };
        let mut prompts = 0;
        let mut cancels = 0;
        let t = Instant::now();
        // Spread over time so only the queue cap (not the rate) decides.
        for n in 1..=12u64 {
            let fx = c.reduce_at(chal_with(n, mk(n), true, false, false, true), t + std::time::Duration::from_secs(n * 2));
            prompts += fx.iter().filter(|e| matches!(e, Effect::Prompt(..))).count();
            cancels += fx.iter().filter(|e| matches!(e, Effect::Cancel(_))).count();
        }
        assert_eq!(prompts, 1, "one open prompt");
        assert_eq!(cancels, 12 - 1 - MAX_QUEUED);
        assert!(c.pending() <= 1 + MAX_QUEUED);
        // Answering the open one opens the next, one at a time.
        let fx = c.reduce(Event::PromptResult { id: id(1), outcome: PromptOutcome::Cancelled });
        assert_eq!(fx.len(), 2);
        assert_eq!(fx[0], Effect::Cancel(id(1)));
        assert!(matches!(&fx[1], Effect::Prompt(i, _) if *i == id(2)));
    }

    #[test]
    fn the_flood_is_rate_limited_by_the_shared_deferral_caps() {
        let mut c = quiet();
        start(&mut c);
        let t = Instant::now();
        let mut cancelled = 0;
        let total = crate::webhost::PERMISSIONS_PER_SECOND + 6;
        for n in 0..total as u64 {
            let sp = SpaceKey { host: format!("h{n}.x"), ..sp("r") };
            let fx = c.reduce_at(chal_with(n, sp, true, false, false, true), t);
            cancelled += fx.iter().filter(|e| matches!(e, Effect::Cancel(_))).count();
        }
        // At most PERMISSIONS_PER_SECOND were held in the second; every other was answered Cancel.
        assert!(c.pending() <= crate::webhost::PERMISSIONS_PER_SECOND);
        assert_eq!(cancelled + c.pending(), total);
    }

    #[test]
    fn at_most_max_per_nav_challenges_per_navigation() {
        let mut c = quiet();
        start(&mut c);
        let t = Instant::now();
        let mut held = 0;
        for n in 0..(MAX_PER_NAV as u64 + 8) {
            // A new second each time (the rate window is not what is tested); answer each at once.
            let at = t + std::time::Duration::from_secs(n * 2);
            let sp = SpaceKey { host: format!("h{n}.x"), ..sp("r") };
            let fx = c.reduce_at(chal_with(n, sp, true, false, false, true), at);
            if fx.iter().any(|e| matches!(e, Effect::Prompt(..))) {
                held += 1;
                c.reduce(Event::PromptResult { id: id(n), outcome: PromptOutcome::Cancelled });
            }
        }
        assert_eq!(held, MAX_PER_NAV);
        // A fresh navigation starts a fresh count.
        start(&mut c);
        let fx = c.reduce_at(chal_with(900, SpaceKey { host: "z.x".into(), ..sp("r") }, true, false, false, true), t + std::time::Duration::from_secs(999));
        assert!(fx.iter().any(|e| matches!(e, Effect::Prompt(..))));
    }

    #[test]
    fn a_navigation_a_failure_or_a_reset_cancels_what_is_held() {
        let mk = |n: u64| SpaceKey { host: format!("h{n}.x"), ..sp("r") };
        for end in [Event::MainProvisionalStarted(None), Event::MainEndedWithoutResponse, Event::Reset] {
            let mut c = quiet();
            start(&mut c);
            let t = Instant::now();
            c.reduce_at(chal_with(1, mk(1), true, false, false, true), t);
            c.reduce_at(chal_with(2, mk(2), true, false, false, true), t);
            let fx = c.reduce(end);
            assert_eq!(fx, vec![Effect::ClosePrompt(id(1)), Effect::Cancel(id(1)), Effect::Cancel(id(2))]);
            assert_eq!(c.pending(), 0);
            // Nothing a late result says answers again.
            assert!(c.reduce(Event::PromptResult { id: id(1), outcome: PromptOutcome::Cancelled }).is_empty());
            assert!(c.reduce(submit(2, "u", "p", true)).is_empty());
        }
        // A commit keeps a later subresource challenge asked, and does not cancel an open one.
        let mut c = quiet();
        start(&mut c);
        c.reduce(chal_with(1, mk(1), true, false, false, true));
        assert!(c.reduce(Event::MainCommitted).is_empty());
        assert_eq!(c.pending(), 1);
    }

    #[test]
    fn reset_forgets_every_secret() {
        let mut c = quiet();
        start(&mut c);
        to_prompt(&mut c, 1, "r", Some(cred("u", "bad")));
        c.reduce(submit(1, "u", "bad", false));
        let (_, req) = only_prompt(c.reduce(chal(2, "r")));
        assert!(req.prefill.is_none(), "rejected: nothing prefilled");
        c.reduce(Event::Reset);
        assert!(c.remembered() == 0 && !c.has_awaiting());
        // The rejection flag is gone: lookups resume on the next navigation.
        start(&mut c);
        assert!(matches!(c.reduce(chal(3, "r")).as_slice(), [Effect::LookupSaved { .. }]));
    }

    #[test]
    fn a_hidden_pane_is_never_asked() {
        let mut c = quiet();
        start(&mut c);
        assert_eq!(c.reduce(Event::Visible(false)), vec![]);
        assert_eq!(c.reduce(chal(1, "r")), vec![Effect::Cancel(id(1))]);
        assert_eq!(c.pending(), 0);
        c.reduce(Event::Visible(true));
        c.reduce(chal(2, "r"));
        let fx = c.reduce(Event::Visible(false));
        assert_eq!(fx, vec![Effect::Cancel(id(2))]);
        // With a prompt open, the window is closed first.
        c.reduce(Event::Visible(true));
        start(&mut c);
        c.reduce(chal(3, "r"));
        c.reduce(Event::SavedLookupResult { id: id(3), credential: None });
        assert_eq!(c.reduce(Event::Visible(false)), vec![Effect::ClosePrompt(id(3)), Effect::Cancel(id(3))]);
    }

    #[test]
    fn a_challenge_that_is_too_long_is_refused_not_cut() {
        let mut c = quiet();
        start(&mut c);
        let long = "x".repeat(MAX_REALM + 1);
        assert_eq!(c.reduce(chal(1, &long)), vec![Effect::Cancel(id(1))]);
        let host = SpaceKey { host: "h".repeat(MAX_HOST + 1), ..sp("r") };
        assert_eq!(c.reduce(chal_with(2, host, true, false, true, true)), vec![Effect::Cancel(id(2))]);
        assert_eq!(c.pending(), 0);
    }

    #[test]
    fn shown_text_is_inert_and_keeps_the_end_of_the_host() {
        let s = SpaceKey { realm: format!("Termurion\u{202e}: type your master password\n{}", "z".repeat(300)), ..sp("r") };
        let r = s.shown_realm();
        assert!(!r.contains('\u{202e}') && !r.contains('\n') && r.chars().count() <= SHOWN_REALM);
        let long = SpaceKey { host: format!("{}.bank.example", "a.".repeat(60)), port: 8443, ..sp("r") };
        let shown = long.shown_origin();
        assert!(shown.starts_with("https://") && shown.ends_with("bank.example:8443"), "{shown}");
        assert_eq!(sp("r").shown_origin(), "https://a.b");
    }

    #[test]
    fn remembered_spaces_are_bounded_and_debug_is_redacted() {
        let mut c = quiet();
        let t = Instant::now();
        for n in 0..(MAX_SPACES as u64 * 2) {
            start(&mut c);
            let s = SpaceKey { realm: format!("r{n}"), ..sp("r") };
            let at = t + std::time::Duration::from_secs(n * 2);
            c.reduce_at(chal_with(n, s, true, false, false, true), at);
            c.reduce(submit(n, "user", "hunter2", false));
        }
        assert!(c.remembered() <= MAX_SPACES);
        let dbg = format!("{:?}", cred("user", "hunter2"));
        assert!(!dbg.contains("hunter2") && !dbg.contains("user"));
        let ev = format!("{:?}", submit(1, "user", "hunter2", true));
        assert!(!ev.contains("hunter2") && !ev.contains("user"), "{ev}");
    }

    fn acct(name: &str, user: &str) -> crate::pwm::Account {
        crate::pwm::Account { id: format!("{name}|{user}"), account_name: name.into(), user_name: user.into(), display: name.into(), has_otp: false, send_otp: false, source_label: None }
    }

    #[test]
    fn a_typed_then_rejected_stored_credential_leaves_at_most_the_user_name() {
        let mut c = quiet();
        start(&mut c);
        // The person typed one, then picked a stored one that the server rejects: the stored password
        // is gone from every prefill; the typed user name may stay.
        c.reduce(chal(1, "r"));
        c.reduce(Event::SavedLookupResult { id: id(1), credential: None });
        c.reduce(submit(1, "typed", "typedpw", false));
        c.reduce(Event::MainFrameResponse { origin: o(), status: 401 });
        start(&mut c);
        c.reduce(chal(2, "r"));
        c.reduce(Event::SavedLookupResult { id: id(2), credential: None });
        c.reduce(Event::PromptResult { id: id(2), outcome: PromptOutcome::OpenPasswordManager(None) });
        c.reduce(Event::PickerResult { id: id(2), chosen: Some(cred("stored", "storedpw")), typed_fallback: None });
        c.reduce(Event::MainFrameResponse { origin: o(), status: 401 });
        start(&mut c);
        let (_, req) = only_prompt(c.reduce(chal(3, "r")));
        assert!(!req.remember_by_default);
        let p = req.prefill.expect("the typed user name stays");
        assert!(p.password.expose().is_empty() && p.user == "typed", "the stored credential never comes back");
    }

    #[test]
    fn a_background_pane_raises_no_prompt() {
        let mut c = Coordinator::default();
        assert!(c.reduce(Event::MainProvisionalStarted(Some(o()))).is_empty());
        assert_eq!(c.reduce(chal(1, "r")), vec![Effect::Cancel(id(1))], "a pane is not shown until it says so");
        c.reduce(Event::Visible(true));
        assert!(matches!(c.reduce(chal(2, "r")).as_slice(), [Effect::LookupSaved { .. }]));
    }

    #[test]
    fn a_cancelled_origin_is_quiet_for_the_cooldown() {
        let mut c = Coordinator::default();
        let t = Instant::now();
        c.reduce_at(Event::Visible(true), t);
        c.reduce_at(Event::MainProvisionalStarted(Some(o())), t);
        c.reduce_at(chal(1, "r"), t);
        c.reduce_at(Event::SavedLookupResult { id: id(1), credential: None }, t);
        c.reduce_at(Event::PromptResult { id: id(1), outcome: PromptOutcome::Cancelled }, t);
        // A page that reloads itself into the same 401 cannot bring the prompt back.
        c.reduce_at(Event::MainProvisionalStarted(Some(o())), t);
        let soon = t + std::time::Duration::from_secs(30);
        assert_eq!(c.reduce_at(chal(2, "r"), soon), vec![Effect::Cancel(id(2))]);
        let later = t + COOLDOWN + std::time::Duration::from_secs(1);
        assert!(matches!(c.reduce_at(chal(3, "r"), later).as_slice(), [Effect::LookupSaved { .. }]));
    }

    #[test]
    fn the_cancel_cooldown_is_the_shared_bounded_one() {
        let mut c = Coordinator::default();
        let t0 = Instant::now();
        c.reduce_at(Event::Visible(true), t0);
        for n in 0..(crate::cooldown::KEPT as u64 + 40) {
            let t = t0 + std::time::Duration::from_secs(n);
            let origin = Origin { scheme: "https".into(), host: format!("h{n}.example"), port: 443 };
            let space = SpaceKey { scheme: "https".into(), host: format!("h{n}.example"), port: 443, realm: "r".into() };
            c.reduce_at(Event::MainProvisionalStarted(Some(origin)), t);
            let fx = c.reduce_at(chal_with(n + 1, space, true, false, true, true), t);
            assert!(matches!(fx.as_slice(), [Effect::LookupSaved { .. }]), "{n}: {fx:?}");
            c.reduce_at(Event::SavedLookupResult { id: id(n + 1), credential: None }, t);
            c.reduce_at(Event::PromptResult { id: id(n + 1), outcome: PromptOutcome::Cancelled }, t);
            assert!(c.cancelled.len() <= crate::cooldown::KEPT);
        }
        assert!(!c.cancelled.is_empty());
        let src = include_str!("browser_basic_auth.rs");
        assert!(!src.contains(&format!("cancelled: {}", "HashMap")), "one cooldown owner");
    }

    fn origin_at(host: &str) -> Origin {
        Origin { scheme: "https".into(), host: host.into(), port: 443 }
    }

    #[test]
    fn navigation_mapping_follows_iterm2() {
        let mut t = NavTracker::default();
        assert_eq!(t.starting(1, "https://a.b/", false), vec![Event::MainProvisionalStarted(Some(o()))]);
        assert_eq!(t.starting(1, "https://c.d/", true), vec![Event::MainFrameRedirect(Some(origin_at("c.d")))]);
        assert_eq!(t.response(1, 200), vec![Event::MainFrameResponse { origin: origin_at("c.d"), status: 200 }]);
        assert_eq!(t.committed(1), vec![Event::MainCommitted]);
        let mut t = NavTracker::default();
        t.starting(1, "https://a.b/", false);
        assert!(t.response(9, 200).is_empty(), "another navigation");
        assert!(t.response(1, 302).is_empty(), "a redirect is never the final response");
        assert!(t.committed(9).is_empty(), "a superseded load");
        assert_eq!(t.completed(1, false), vec![Event::MainEndedWithoutResponse]);
        let mut t = NavTracker::default();
        t.starting(1, "https://a.b/", false);
        t.response(1, 200);
        t.committed(1);
        assert!(t.completed(1, false).is_empty(), "after a commit");
        let mut t = NavTracker::default();
        assert_eq!(t.starting(2, "about:blank", false), vec![Event::MainProvisionalStarted(None)]);
        assert_eq!(strip_fragment("https://a.b/x#f"), "https://a.b/x");
        assert_eq!(strip_fragment("https://a.b/x"), "https://a.b/x");
    }

    #[test]
    fn response_order_does_not_matter() {
        let both = || vec![Event::MainFrameResponse { origin: o(), status: 200 }, Event::MainCommitted];
        let mut t = NavTracker::default();
        t.starting(1, "https://a.b/", false);
        assert!(t.committed(1).is_empty(), "held until the response or completion");
        assert_eq!(t.response(1, 200), both());
        let mut t = NavTracker::default();
        t.starting(1, "https://a.b/", false);
        let mut got = t.response(1, 200);
        got.extend(t.committed(1));
        assert_eq!(got, both());
        let mut t = NavTracker::default();
        t.starting(1, "https://a.b/", false);
        t.committed(1);
        assert_eq!(t.completed(1, true), vec![Event::MainCommitted]);
        assert!(t.response(1, 200).is_empty(), "a response after completion is nothing");
        // A new navigation forgets the old one entirely.
        t.starting(2, "https://a.b/", false);
        assert!(t.committed(1).is_empty());
    }

    #[test]
    fn a_dropped_coordinator_answers_everything_it_holds() {
        let seen = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let mut c = Coordinator::default();
        let sink = seen.clone();
        c.on_drop(Box::new(move |fx| sink.borrow_mut().extend(fx)));
        start(&mut c);
        c.reduce(chal(1, "r"));
        c.reduce(Event::SavedLookupResult { id: id(1), credential: None });
        c.reduce(chal(2, "q"));
        drop(c);
        assert_eq!(*seen.borrow(), vec![Effect::ClosePrompt(id(1)), Effect::Cancel(id(1)), Effect::Cancel(id(2))]);
    }

    #[test]
    fn store_rules_follow_iterm2() {
        assert_eq!(account_name("https", "a.b", 443, ""), "https://a.b");
        assert_eq!(account_name("http", "a.b", 8080, "R"), "http://a.b:8080 (R)");
        assert_eq!(account_name("http", "a.b", 443, ""), "http://a.b:443");
        assert_eq!(prompt_text("a.b", "R"), "The website \u{201c}a.b\u{201d} requires a user name and password for \u{201c}R\u{201d}.");
        assert_eq!(prompt_text("a.b", ""), "The website \u{201c}a.b\u{201d} requires a user name and password.");
        assert!(!can_remember(true) && can_remember(false));
        assert!(!unlocked_without_prompt(false, true, false));
        assert!(unlocked_without_prompt(true, true, false));
        assert!(unlocked_without_prompt(false, false, false));
        assert!(!unlocked_without_prompt(true, true, true), "authenticate every time: never silent");
        assert!(unlocked_without_prompt(true, false, true));
        let two = [acct("https://a.b", "x"), acct("https://a.b", "y")];
        assert_eq!(pick_saved(&two, "https://a.b", Some("y")).map(|a| a.user_name.as_str()), Some("y"));
        assert_eq!(pick_saved(&two, "https://a.b", None).map(|a| a.user_name.as_str()), Some("x"));
        assert_eq!(pick_saved(&two, "https://a.b", Some("")).map(|a| a.user_name.as_str()), Some("x"), "an empty preferred user does not count");
        assert_eq!(pick_saved(&two, "https://a.b", Some("z")).map(|a| a.user_name.as_str()), Some("x"));
        assert!(pick_saved(&two, "https://c.d", None).is_none());
        assert_eq!(save_plan(&two, "https://a.b", "y"), SavePlan::SetPassword(two[1].clone()));
        assert_eq!(save_plan(&two, "https://a.b", "q"), SavePlan::Add);
        assert_eq!(save_plan(&two, "https://c.d", "x"), SavePlan::Add);
        assert_eq!(REMEMBER, "Remember this password");
        assert_eq!((OK, CANCEL, PASSWORD_MANAGER, PASSWORD_PLACEHOLDER, USER_PLACEHOLDER), ("OK", "Cancel", "Password Manager", "Password", "User name"));
    }

    #[test]
    fn challenge_parsing_follows_rfc7235() {
        let p = |h: &str| parse_challenge(h);
        assert_eq!(p(r#"Basic realm="Secure Area", charset="UTF-8""#), ("Basic".into(), "Secure Area".into()));
        assert_eq!(p("basic realm=x"), ("basic".into(), "x".into()));
        assert_eq!(p("NTLM"), ("NTLM".into(), String::new()));
        assert_eq!(p(r#"Basic realm="a\"b""#), ("Basic".into(), "a\"b".into()));
        assert_eq!(p(r#"Basic charset="realm=evil", REALM="real""#), ("Basic".into(), "real".into()), "a quoted value is not a parameter");
        assert_eq!(p(""), (String::new(), String::new()));
        let s = space_of("HTTPS://A.B/x", "R").unwrap();
        assert_eq!((s.scheme.as_str(), s.host.as_str(), s.port, s.realm.as_str()), ("https", "a.b", 443, "R"));
        assert_eq!(space_of("http://a.b:81/", "").unwrap().port, 81);
        assert_eq!(space_of("http://a.b/", "").unwrap().port, 80);
        assert!(space_of("file:///c:/x", "").is_none());
        assert!(space_of("ftp://a.b/", "R").is_none() && origin_of("ftp://a.b:21/").is_none());
        let o = origin_of("https://A.B/x#f").unwrap();
        assert_eq!((o.scheme.as_str(), o.host.as_str(), o.port), ("https", "a.b", 443));
        assert!(origin_of("https://u:p@a.b/").is_none(), "user information is refused");
        assert!(is_proxy_challenge("http://127.0.0.1:8888/", Some(("127.0.0.1", 8888))));
        assert!(!is_proxy_challenge("https://a.b/", Some(("127.0.0.1", 8888))));
        assert!(is_proxy_challenge("http://PROXY.lan:3128/", Some(("proxy.lan", 3128))));
        assert!(!is_proxy_challenge("http://127.0.0.1:8888/", None));
    }

    #[test]
    fn the_prompt_heading_names_the_origin_and_labels_the_realm() {
        let mut space = sp("r");
        space.realm = "Termurion\u{202e}: type your master password".into();
        let req = PromptRequest { space, prefill: None, offer_remember: false, offer_password_manager: false, remember_by_default: false, cleartext: true, proxy: false };
        let h = prompt_heading(&req);
        assert!(h.starts_with("The website \u{201c}https://a.b\u{201d} requires"), "{h}");
        assert!(h.contains(REALM_LABEL) && !h.contains('\u{202e}'), "{h}");
        assert!(h.find("a.b").unwrap() < h.find("Termurion").unwrap(), "the realm comes after the host");
        assert!(h.contains(CLEARTEXT_WARNING));
        let proxy = PromptRequest { proxy: true, cleartext: false, ..req };
        let h = prompt_heading(&proxy);
        assert!(h.starts_with("The proxy") && !h.contains(CLEARTEXT_WARNING));
    }
}
