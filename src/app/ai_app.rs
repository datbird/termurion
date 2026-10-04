//! The AI features' alerts, one at a time (ruling 12c-7: the one AI alert queue of 12a to
//! 12f), the gate every AI entry point asks first, the consent it asks for, the one-vendor key
//! prompt (ruling 12e-1), and revealing Settings > General > AI. The decisions are `ai_gate`'s;
//! this is the showing and answering.

use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::WindowId;

use super::App;
use crate::ai_gate::{self, Alert, Then};
use crate::ai_key_ui::{KeyResult, KeyWindow};
use crate::ai_keys;
use crate::ai_settings::Vendor;
use crate::prompt_ui::{ConfirmResult, ConfirmWindow};
use crate::pwm::Secret;
use crate::settings_ai::{AiRequest, SubTab};

/// What a given or refused key continues (ruling 12e-1): the variants 12e added each have their
/// arm in `App::ai_key_given`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum KeyThen {
    #[allow(dead_code)] // Later callers that only store the key.
    Nothing,
    /// The settings window's model editor runs Test Connection.
    TestConnection,
    /// A chat turn (the chat id) continues with the key (minor m4).
    Turn(String),
    /// A chat's vector-store job (the chat id) starts again with the key (minor m4).
    Files(String),
    /// A 12e request (the run id) continues with the key; none ends it (`ai_runs`).
    Run(u64),
    /// A Codecierge session (its session guid) goes on to its goal.
    Codecierge(String),
    /// A waiting Explain Output request continues with the key (`ai_explain_app`).
    Explain,
}

impl App {
    /// Queues an AI alert; they show one at a time in order. One with a permanent-silence key
    /// whose answer was remembered is answered at once without showing (`scripts_alerts`).
    pub(super) fn ask_ai(&mut self, alert: Alert) {
        if let Some(id) = alert.silence.as_deref() {
            let selection = crate::warnings::selection_key(id);
            if self.globals.bool(id) {
                let index = usize::try_from(self.globals.int(&selection)).unwrap_or(0);
                // A silence that answers for the person is listed in Suppressed Alerts (ruling F3).
                if let Some(spec) = ai_gate::silence_spec(&alert) {
                    super::warnings_app::record_answer(&spec, index, &self.globals, &mut self.suppressed, super::warnings_app::unix_now());
                    self.catalog_due_after_record();
                }
                return self.ai_answered(alert.then, index);
            }
        }
        self.ai_asks.push_back(alert);
    }

    /// The gate for the default model: true when AI may be used now. Otherwise, unless
    /// `silent` or a gate alert is already queued or showing, it says why (`GK:23-99`), and
    /// the person repeats the action after answering (Decision).
    pub(super) fn ai_check(&mut self, silent: bool) -> bool {
        let found = self.ai_found();
        let check = ai_gate::check(&self.globals, crate::ai_catalog::bundled(), &found);
        self.ai_gate_answer(&check, silent)
    }

    /// The same rule for an endpoint the caller chose (ruling R12ab-2): a pinned model, the
    /// economy model, Test Connection, the key prompt.
    pub(super) fn ai_check_endpoint(&mut self, endpoint: Option<&crate::ai_models::Endpoint>, silent: bool) -> bool {
        let check = ai_gate::check_endpoint(&self.globals, endpoint);
        self.ai_gate_answer(&check, silent)
    }

    fn ai_gate_answer(&mut self, check: &ai_gate::Check, silent: bool) -> bool {
        let Some(alert) = ai_gate::alert(check) else { return true };
        if !silent {
            self.ask_gate(alert);
        }
        false
    }

    /// Queues a gate alert, unless one is queued or showing: then the showing alert comes to
    /// the front, so the action visibly waits on it (iTerm2's alert is modal, so the action
    /// cannot be repeated behind it).
    fn ask_gate(&mut self, alert: Alert) {
        if !self.ai_gate_pending() {
            return self.ask_ai(alert);
        }
        if let Some((w, _)) = &self.ai_alert {
            w.focus();
        }
    }

    /// Whether a gate alert is queued or showing (at most one at a time).
    fn ai_gate_pending(&self) -> bool {
        self.ai_asks.iter().chain(self.ai_alert.as_ref().map(|(_, a)| a)).any(ai_gate::is_gate)
    }

    /// Opens the next queued AI alert when none shows.
    pub(super) fn open_ai_alert(&mut self, event_loop: &ActiveEventLoop) {
        if self.ai_alert.is_some() || self.ai_asks.is_empty() {
            return;
        }
        let Some(g) = self.gpu.as_ref() else { return };
        let Some(alert) = self.ai_asks.pop_front() else { return };
        match ConfirmWindow::actions(event_loop, &g.instance, &g.adapter, &g.device, &g.queue, &alert.heading, &alert.text, &alert.buttons, alert.silence.is_some().then_some(ConfirmWindow::SUPPRESS), &[], false, false) {
            Ok(mut w) => {
                // An alert that names its refusing button makes a close and Escape answer it.
                w.close_with(alert.close);
                self.ai_alert = Some((w, alert));
            }
            Err(e) => self.banners.set("window", format!("Could not open the window: {e}")),
        }
    }

    /// Events for the showing AI alert. False for other windows.
    pub(super) fn ai_alert_event(&mut self, window: WindowId, event: &WindowEvent) -> bool {
        let Some((w, _)) = self.ai_alert.as_mut().filter(|(w, _)| w.id() == window) else { return false };
        let answer = match event {
            WindowEvent::CloseRequested => w.close_answer(),
            WindowEvent::RedrawRequested => w.redraw(),
            other => {
                w.event(other);
                ConfirmResult::Open
            }
        };
        let (index, suppress) = match answer {
            ConfirmResult::Action { index, suppress } => (index, suppress),
            // An `actions` window answers only with an action.
            ConfirmResult::Open | ConfirmResult::Ok { .. } | ConfirmResult::Cancel => return true,
        };
        let Some((_, alert)) = self.ai_alert.take() else { return true };
        if let (true, Some(id)) = (suppress, alert.silence.as_deref()) {
            self.globals.set(id, toml::Value::Boolean(true));
            self.globals.set(&crate::warnings::selection_key(id), toml::Value::Integer(index as i64));
            self.save_globals();
        }
        self.ai_answered(alert.then, index);
        true
    }

    /// Asks for one vendor's API key (iTerm2's registration sheet,
    /// `AITerm/AITermControllerRegistrationHelper.swift:40-51`), after the gate for the
    /// caller's own endpoint (ruling R12ab-2: never the default model's, so a caller going to
    /// another host is not shown the wrong consent). A closed gate or missing consent answers
    /// None at once, after the gate's alert; otherwise the prompt waits its turn.
    pub(crate) fn ask_ai_key(&mut self, vendor: Vendor, endpoint: Option<&crate::ai_models::Endpoint>, then: KeyThen) {
        if !self.ai_check_endpoint(endpoint, false) {
            return self.ai_key_given(then, None);
        }
        self.ai_key_asks.push_back((vendor, then));
    }

    /// Opens the next queued key prompt when none shows (one at a time).
    pub(super) fn open_ai_key_window(&mut self, event_loop: &ActiveEventLoop) {
        if self.ai_key_window.is_some() || self.ai_key_asks.is_empty() {
            return;
        }
        let Some(g) = self.gpu.as_ref() else { return };
        let Some((vendor, then)) = self.ai_key_asks.pop_front() else { return };
        match KeyWindow::open(event_loop, &g.instance, &g.adapter, &g.device, &g.queue, vendor) {
            Ok(w) => self.ai_key_window = Some((w, vendor, then)),
            Err(e) => {
                self.banners.set("window", format!("Could not open the window: {e}"));
                self.ai_key_given(then, None);
            }
        }
    }

    /// Events for the showing key prompt. False for other windows. OK with a key that is not
    /// blank stores it (a failed write still continues with it: `Keys::set` has cached it for
    /// the session, and no error text shows outside the settings window); Cancel, closing the
    /// window or a blank key continue with None.
    pub(super) fn ai_key_event(&mut self, window: WindowId, event: &WindowEvent) -> bool {
        let Some((w, _, _)) = self.ai_key_window.as_mut().filter(|(w, _, _)| w.id() == window) else { return false };
        let typed = match event {
            WindowEvent::CloseRequested => None,
            WindowEvent::RedrawRequested => match w.redraw() {
                KeyResult::Open => return true,
                KeyResult::Cancel => None,
                KeyResult::Ok(key) => Some(key),
            },
            other => {
                w.event(other);
                return true;
            }
        };
        let Some((_, vendor, then)) = self.ai_key_window.take() else { return true };
        let key = typed.and_then(|k| ai_keys::normalized(k.expose()).map(|t| Secret::new(t.to_string())));
        if let Some(k) = &key {
            let mut cache = ai_keys::shared().lock().unwrap_or_else(|e| e.into_inner());
            // A failed write is not shown: the key is cached for the session and used now.
            let _ = cache.set(vendor, k, &mut *ai_keys::system_store());
        }
        self.ai_key_given(then, key);
        true
    }

    /// Continues what asked for a key, with the key or None (refused, cancelled, blank).
    fn ai_key_given(&mut self, then: KeyThen, key: Option<Secret>) {
        match then {
            KeyThen::Nothing => drop(key),
            // The editor's next frame hands it to its tester (`settings_ai::drive_test`).
            KeyThen::TestConnection => {
                if let Some(s) = self.settings.as_mut() {
                    s.ai.test_key = Some(key);
                    s.request_redraw();
                }
            }
            KeyThen::Turn(chat) => self.ai_turn_key(&chat, key),
            KeyThen::Files(chat) => self.ai_files_key(&chat, key),
            KeyThen::Run(id) => self.ai_run_key(id, key),
            KeyThen::Explain => self.explain_key_given(key),
            // The key prompt stored the key; a given one goes on to the goal view, a refusal stays.
            KeyThen::Codecierge(guid) => {
                if key.is_some() {
                    self.codecierge_goal(&guid);
                }
            }
        }
    }

    /// Opens Settings > General > AI when an answer asked for it.
    pub(super) fn open_ai_reveal(&mut self, event_loop: &ActiveEventLoop) {
        let Some(sub) = self.ai_reveal.take() else { return };
        self.open_settings_page(event_loop, crate::settings_ui::Page::General);
        if self.settings.as_mut().is_some_and(|s| s.show_ai(sub)) {
            self.apply_edit(crate::settings_ui::Edit::GlobalsChanged);
        }
    }

    /// Advances Ollama discovery (every loop pass; cheap when nothing is due) while AI features
    /// are allowed: running refreshes are polled and the configured servers that are due are
    /// asked. New models redraw every window, so open pickers rebuild, and are saved; the
    /// settings window's refresh gets its answer (ruling R12ab-3).
    pub(crate) fn ai_discovery_tick(&mut self) {
        if !crate::ai_settings::allowed(&self.globals) {
            return;
        }
        let configs = crate::ai_discovery::configs(&self.globals);
        let changed = self.ai_discovery.tick(std::time::Instant::now(), &*self.ai_transport, &configs, self.notify.clone());
        if changed {
            self.ai_discovery.save();
            self.redraw_all();
            // The chat window's model list follows the newly found models (ruling F17).
            crate::chat_toolbar::mark_after_discovery(changed, &mut self.chat_toolbar_dirty);
            self.chat_window_refresh();
            if let Some(s) = self.settings.as_mut() {
                s.ai.found = self.ai_discovery.all();
                s.request_redraw();
            }
        }
        let Some(outcome) = self.ai_probe.and_then(|t| self.ai_discovery.finished(t)) else { return };
        self.ai_probe = None;
        if let Some(s) = self.settings.as_mut() {
            s.ai.probe = Some(outcome);
            s.request_redraw();
        }
    }

    /// When discovery next needs a tick by itself (running refreshes wake the loop).
    pub(super) fn ai_discovery_due(&self) -> Option<std::time::Instant> {
        if !crate::ai_settings::allowed(&self.globals) {
            return None;
        }
        self.ai_discovery.next_due(std::time::Instant::now())
    }

    /// The discovered Ollama models of the configured servers, for model resolution
    /// (`ai_model::configured`, `named`, `economy`). Reading them asks stale servers again, as
    /// iTerm2's reads do (`OC:158-170`).
    pub(crate) fn ai_found(&mut self) -> crate::ai_model::Discovered {
        self.ai_discovery_read();
        self.ai_discovery.discovered(&crate::ai_discovery::configs(&self.globals))
    }

    /// A read of the discovered models: stale configured servers are asked again, even after
    /// the automatic retries ran out, while AI features are allowed.
    fn ai_discovery_read(&mut self) {
        if !crate::ai_settings::allowed(&self.globals) {
            return;
        }
        let configs = crate::ai_discovery::configs(&self.globals);
        self.ai_discovery.wanted(std::time::Instant::now(), &*self.ai_transport, &configs, self.notify.clone());
    }

    /// Takes the settings window's AI request after its frame and acts on it (ruling R12ab-3): a
    /// discovery answer reaches the same window through `ai_discovery_tick`, a tester and a key
    /// prompt's answer through its `State`, and an alert goes on the AI alert queue (a gate
    /// alert only when none is queued or showing). A frame of Settings > General > AI > General
    /// read the discovered models (the model popup, the Regular and Budget pickers, Manage AI
    /// Models), which asks stale servers again; other pages read none.
    pub(crate) fn drain_ai_requests(&mut self, _event_loop: &ActiveEventLoop) {
        if self.settings.as_ref().is_some_and(|s| s.shows_ai_models()) {
            self.ai_discovery_read();
        }
        let Some(request) = self.settings.as_mut().and_then(|s| s.ai.pending.take()) else { return };
        match request {
            AiRequest::Discover { endpoint, headers, force } => {
                let t = self.ai_discovery.refresh(&endpoint, &headers, force, &*self.ai_transport, self.notify.clone());
                self.ai_probe = Some(t);
            }
            AiRequest::Alert(alert) if ai_gate::is_gate(&alert) => self.ask_gate(alert),
            AiRequest::Alert(alert) => self.ask_ai(alert),
            AiRequest::BuildTester(probe) => {
                let wire = crate::ai_turn::Wire::from_globals(&self.globals);
                let options = crate::ai_transport::options_from(&self.globals);
                let tester = crate::ai_test_conn::Tester::new(probe, wire, self.ai_transport.clone(), options, self.notify.clone());
                if let Some(s) = self.settings.as_mut() {
                    s.ai.tester = Some(tester);
                    s.request_redraw();
                }
            }
            // Ruling R12ab-2: the gate and the prompt are for the probe's own endpoint.
            AiRequest::AskKey(vendor, endpoint) => self.ask_ai_key(vendor, Some(&endpoint), KeyThen::TestConnection),
        }
    }

    /// Acts on an answer: the button's place in the alert's buttons. Later plans add the arms
    /// of their `Then` variants here.
    fn ai_answered(&mut self, then: Then, index: usize) {
        match then {
            Then::Nothing => {}
            Then::Reveal => {
                if index == 0 {
                    self.ai_reveal = Some(SubTab::General);
                }
            }
            // Turn On writes Granted; Not Now (also a close or Escape) writes Denied, as iTerm2.
            // A question overtaken by Confirm Change's grant writes nothing (ruling F31).
            Then::AutoProvide => {
                let current = crate::ai_context::Consent::from_global(self.globals.int(crate::ai_context::CONSENT_KEY));
                if let Some(consent) = crate::chat_menu::queued_consent_write(current, index) {
                    self.globals.set(crate::ai_context::CONSENT_KEY, toml::Value::Integer(consent.raw()));
                    self.save_globals();
                }
            }
            // OK turns the check on; either answer ends the question for good.
            Then::SafetyNag => self.ai_nag_answered(index),
            // Only the second button (Delete) deletes; Cancel, a close and Escape never do.
            Then::DeleteChats(ids) => {
                if index == 1 {
                    self.delete_chats(ids);
                }
            }
            // The alert names the chat, session and category; a stale answer does nothing.
            Then::ConfirmChange { chat, guid, category } => self.confirm_change_answered(&chat, &guid, category, index),
            // Only OK starts streaming; Cancel, a close and Escape never do.
            Then::PrivacyWarning { chat, guid } => {
                if index == 0 {
                    self.start_streaming(&chat, &guid);
                }
            }
            // Only Attach attaches; Insert Path, a close and a stale index insert the paths.
            Then::DropChoice { chat, supported, all } => self.chat_drop_answered(&chat, &supported, &all, index),
            // Only OK keeps the box ticked.
            Then::CodeciergeDanger(guid) => {
                if index != 0 {
                    if let Some(s) = self.codecierge.get_mut(&guid) {
                        s.auto_box = false;
                    }
                    self.tools_changed();
                    self.redraw_all();
                }
            }
            // Only OK remembers the warning and starts; Cancel, a close and Escape stop.
            Then::CodeciergePrivacy(guid) => {
                if index == 0 {
                    self.globals.set(crate::codecierge::PRIVACY_KEY, toml::Value::Boolean(true));
                    self.save_globals();
                    self.codecierge_start(&guid);
                }
            }
            Then::Grant(host) => {
                if index == 0 {
                    ai_gate::grant(&mut self.globals, &host);
                    self.save_globals();
                }
            }
        }
    }
}
