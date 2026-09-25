//! Trigger actions the window runs, and Session > Triggers.

use super::*;
use crate::trigger_eval::TriggerRequest;

impl App {
    pub(super) fn trigger_request(&mut self, pane: PaneId, r: TriggerRequest) {
        match r {
            TriggerRequest::Alert { text, .. } => {
                self.ask(pane, "trigger-alert", &text, &["OK"], true);
            }
            // The rest arrive with the window actions (task 5 of milestone 6a).
            _ => {}
        }
    }
}
