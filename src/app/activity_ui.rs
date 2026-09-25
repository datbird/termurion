//! Output and idle per session: the tab indicators, and the events notifications and event
//! triggers use.

use std::time::{Duration, Instant};

use super::*;
use crate::activity::ActivityEvent;

impl App {
    /// Whether any of these panes shows the new-output or the idle indicator.
    pub(super) fn activity_icons(&self, panes: &[PaneId]) -> (bool, bool) {
        let now = Instant::now();
        let show_new = self.globals.bool("ShowNewOutputIndicator");
        let mut new_output = false;
        let mut idle = false;
        for id in panes {
            if let Some(a) = self.activity.get(id).filter(|a| a.unseen()) {
                new_output |= show_new && a.is_processing(now);
                idle |= a.is_idle(now);
            }
        }
        (new_output, idle)
    }

    /// Output since the last tick, the user's view, and the events. When to come back.
    pub(super) fn tick_activity(&mut self) -> Option<Instant> {
        let now = Instant::now();
        let got: Vec<PaneId> = self.panes.iter_mut().filter_map(|(id, p)| std::mem::take(&mut p.got_output).then_some(*id)).collect();
        let mut changed = !got.is_empty();
        for id in got {
            self.activity.entry(id).or_default().output(now);
        }
        let looking: Vec<PaneId> = self.panes.keys().copied().filter(|id| self.looking_at(*id)).collect();
        for id in looking {
            if let Some(a) = self.activity.get_mut(&id).filter(|a| a.unseen()) {
                a.seen();
                changed = true;
            }
        }
        self.activity.retain(|id, _| self.panes.contains_key(id));
        let mut events: Vec<(PaneId, ActivityEvent)> = Vec::new();
        for (id, a) in self.activity.iter_mut() {
            events.extend(a.tick(now).into_iter().map(|e| (*id, e)));
        }
        let waiting = self.activity.values().any(|a| a.in_burst());
        changed |= !events.is_empty();
        for (pane, e) in events {
            self.activity_event(pane, e);
        }
        if changed {
            self.redraw();
        }
        waiting.then(|| now + Duration::from_millis(500))
    }

    /// An idle, new-output, or activity-after-idle event (notifications and event triggers).
    fn activity_event(&mut self, pane: PaneId, e: ActivityEvent) {
        match e {
            ActivityEvent::Idle => self.post_alert(pane, crate::alerts::AlertKind::Idle),
            ActivityEvent::NewOutput => self.post_alert(pane, crate::alerts::AlertKind::NewOutput),
            ActivityEvent::ActivityAfterIdle => {}
        }
    }
}
