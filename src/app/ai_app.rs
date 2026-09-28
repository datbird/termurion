//! The AI features' alerts, one at a time (ruling 12c-7: the one AI alert queue of 12a to
//! 12f), the gate every AI entry point asks first, the consent it asks for, the one-vendor key
//! prompt (ruling 12e-1), and revealing Settings > General > AI. The decisions are `ai_gate`'s;
//! this is the showing and answering.

use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::WindowId;

use super::scripts_alerts::silence_keys;
use super::App;
use crate::ai_gate::{self, Alert, Then};
use crate::ai_key_ui::{KeyResult, KeyWindow};
use crate::ai_keys;
use crate::ai_settings::Vendor;
use crate::prompt_ui::{ConfirmResult, ConfirmWindow};
use crate::pwm::Secret;
use crate::settings_ai::SubTab;

/// What a given or refused key continues (ruling 12e-1). 12b adds `TestConnection`, 12c
/// `Turn(String)`, 12e `Run(u64)`, `Codecierge(String)` and `Explain`, each with its arm in
/// `App::ai_key_given`.
#[allow(dead_code)] // 12a asks for no key itself; 12b is the first caller.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum KeyThen {
    Nothing,
}

impl App {
    /// Queues an AI alert; they show one at a time in order. One with a permanent-silence key
    /// whose answer was remembered is answered at once without showing (`scripts_alerts`).
    pub(super) fn ask_ai(&mut self, alert: Alert) {
        if let Some(id) = alert.silence.as_deref() {
            let (flag, selection) = silence_keys(id);
            if self.globals.bool(&flag) {
                let index = usize::try_from(self.globals.int(&selection)).unwrap_or(0);
                return self.ai_answered(alert.then, index);
            }
        }
        self.ai_asks.push_back(alert);
    }

    /// The gate for the default model: true when AI may be used now. Otherwise, unless
    /// `silent` or a gate alert is already queued or showing, it says why (`GK:23-99`), and
    /// the person repeats the action after answering (Decision).
    pub(super) fn ai_check(&mut self, silent: bool) -> bool {
        let check = ai_gate::check(&self.globals, crate::ai_catalog::bundled());
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
        let pending = self.ai_asks.iter().chain(self.ai_alert.as_ref().map(|(_, a)| a)).any(ai_gate::is_gate);
        if !silent && !pending {
            self.ask_ai(alert);
        }
        false
    }

    /// Opens the next queued AI alert when none shows.
    pub(super) fn open_ai_alert(&mut self, event_loop: &ActiveEventLoop) {
        if self.ai_alert.is_some() || self.ai_asks.is_empty() {
            return;
        }
        let Some(g) = self.gpu.as_ref() else { return };
        let Some(alert) = self.ai_asks.pop_front() else { return };
        match ConfirmWindow::actions(event_loop, &g.instance, &g.adapter, &g.device, &g.queue, &alert.heading, &alert.text, &alert.buttons, alert.silence.is_some()) {
            Ok(w) => self.ai_alert = Some((w, alert)),
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
            let (flag, selection) = silence_keys(id);
            self.globals.set(&flag, toml::Value::Boolean(true));
            self.globals.set(&selection, toml::Value::Integer(index as i64));
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
    #[allow(dead_code)] // 12b's Test Connection, 12c's chat turn and 12e's features call it.
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
        }
    }

    /// Opens Settings > General > AI when an answer asked for it.
    pub(super) fn open_ai_reveal(&mut self, event_loop: &ActiveEventLoop) {
        let Some(sub) = self.ai_reveal.take() else { return };
        self.open_settings_page(event_loop, crate::settings_ui::Page::General);
        if let Some(s) = self.settings.as_mut() {
            s.show_ai(sub);
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
            Then::Grant(host) => {
                if index == 0 {
                    ai_gate::grant(&mut self.globals, &host);
                    self.save_globals();
                }
            }
        }
    }
}
