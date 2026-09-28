//! The AI features' alerts, one at a time (ruling 12c-7: the one AI alert queue of 12a to
//! 12f), the gate every AI entry point asks first, the consent it asks for, and revealing
//! Settings > General > AI. The decisions are `ai_gate`'s; this is the showing and answering.

use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::WindowId;

use super::scripts_alerts::silence_keys;
use super::App;
use crate::ai_gate::{self, Alert, Then};
use crate::prompt_ui::{ConfirmResult, ConfirmWindow};
use crate::settings_ai::SubTab;

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
    /// economy model, Test Connection, the key prompt. First called by Task 7's `ask_ai_key`.
    #[allow(dead_code)]
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
