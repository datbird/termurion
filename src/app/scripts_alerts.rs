//! The Scripts features' alerts, one at a time: downloads to confirm, the slow-scan question
//! and the notices after them (iTerm2's `iTermWarning`s from the scripts menu controller and
//! the provisioner). An alert that is permanently silenceable remembers its answer under
//! iTerm2's keys and is answered at once while silenced.

use std::collections::VecDeque;

use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::WindowId;

use super::script_runtime_ui::After;
use super::App;
use crate::prompt_ui::{ConfirmResult, ConfirmWindow};
use crate::script_provision::Job;

/// One alert waiting to show.
pub(super) struct Ask {
    pub heading: String,
    pub text: String,
    /// Left to right; Enter presses the first and Escape the one labeled "Cancel".
    pub buttons: Vec<String>,
    /// iTerm2's identifier when the alert can be silenced for good.
    pub silence: Option<&'static str>,
    pub then: Then,
}

/// What an answer does.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Then {
    Nothing,
    /// A download consent: the first button (OK) submits the jobs in order, each with what
    /// follows it; any other answer drops them silently (iTerm2's canceled download).
    Consent(Vec<(Job, After)>),
    /// "Performance Issue": Stop (0) or Continue (1).
    SlowScan,
}

/// The remembered-choice keys of a silenceable warning: the silence flag and the chosen
/// button (`iTermWarning.m`'s `identifier` and `identifier_selection`).
pub(super) fn silence_keys(id: &str) -> (String, String) {
    (id.to_string(), format!("{id}_selection"))
}

/// The queue holds at most this many; later asks are dropped (a flood of the same question
/// must not open windows forever).
const LIMIT: usize = 16;

fn push_capped(queue: &mut VecDeque<Ask>, ask: Ask) {
    if queue.len() < LIMIT {
        queue.push_back(ask);
    }
}

impl App {
    /// Queues an alert; a silenced one answers its remembered choice at once.
    pub(super) fn ask_scripts(&mut self, ask: Ask) {
        if let Some(id) = ask.silence {
            let (flag, selection) = silence_keys(id);
            if self.globals.bool(&flag) {
                let index = usize::try_from(self.globals.int(&selection)).unwrap_or(0);
                return self.scripts_answered(ask.then, index);
            }
        }
        push_capped(&mut self.scripts_alerts, ask);
    }

    /// Opens the next queued alert when none shows.
    pub(super) fn open_scripts_alerts(&mut self, event_loop: &ActiveEventLoop) {
        if self.scripts_alert.is_some() || self.scripts_alerts.is_empty() {
            return;
        }
        let Some(g) = self.gpu.as_ref() else { return };
        let Some(ask) = self.scripts_alerts.pop_front() else { return };
        let labels: Vec<&str> = ask.buttons.iter().map(String::as_str).collect();
        match ConfirmWindow::actions(event_loop, &g.instance, &g.adapter, &g.device, &g.queue, &ask.heading, &ask.text, &labels, ask.silence.is_some()) {
            Ok(w) => self.scripts_alert = Some((w, ask)),
            Err(e) => self.banners.set("window", format!("Could not open the window: {e}")),
        }
    }

    /// Events for the showing alert. False for other windows.
    pub(super) fn scripts_alert_event(&mut self, window: WindowId, event: &WindowEvent) -> bool {
        let Some((w, ask)) = self.scripts_alert.as_mut().filter(|(w, _)| w.id() == window) else { return false };
        let answer = match event {
            // iTerm2's alerts cannot be closed; closing the slow-scan question goes on scanning
            // rather than pressing Stop (the first button, which closing would otherwise mean).
            WindowEvent::CloseRequested if ask.then == Then::SlowScan => ConfirmResult::Action { index: 1, suppress: false },
            WindowEvent::CloseRequested => w.close_answer(),
            WindowEvent::RedrawRequested => w.redraw(),
            other => {
                w.event(other);
                ConfirmResult::Open
            }
        };
        let (index, suppress) = match answer {
            ConfirmResult::Open => return true,
            ConfirmResult::Action { index, suppress } => (index, suppress),
            // An `actions` window answers only with an action.
            ConfirmResult::Ok { .. } | ConfirmResult::Cancel => return true,
        };
        let Some((_, ask)) = self.scripts_alert.take() else { return true };
        if let (true, Some(id)) = (suppress, ask.silence) {
            let (flag, selection) = silence_keys(id);
            self.globals.set(&flag, toml::Value::Boolean(true));
            self.globals.set(&selection, toml::Value::Integer(index as i64));
            self.save_globals();
        }
        self.scripts_answered(ask.then, index);
        true
    }

    /// Acts on an answer: the button's place in the alert's buttons.
    pub(super) fn scripts_answered(&mut self, then: Then, index: usize) {
        match then {
            Then::Nothing => {}
            Then::Consent(jobs) => {
                if index == 0 {
                    self.provision_jobs(jobs);
                }
            }
            Then::SlowScan => {
                let go_on = index == 1;
                if let Some(scanner) = &self.scripts_scan {
                    scanner.answer_slow(go_on);
                }
                if !go_on {
                    let ask = Ask {
                        heading: crate::scripts::DISABLED_HEADING.to_string(),
                        text: crate::scripts::DISABLED_TEXT.to_string(),
                        buttons: vec![crate::api_auth::OK.to_string()],
                        silence: None,
                        then: Then::Nothing,
                    };
                    self.ask_scripts(ask);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silence_keys_follow_iterm2() {
        assert_eq!(silence_keys("NoSyncX"), ("NoSyncX".to_string(), "NoSyncX_selection".to_string()));
    }
}
