//! Output and idle per session: the tab indicators, and the events notifications and event
//! triggers use.

use std::time::{Duration, Instant};

use super::*;
use crate::activity::ActivityEvent;

/// What the activity tick asks of the display.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TabDraw {
    Nothing,
    Now,
    Paced,
}

/// A state change (seen, an event) draws at once; a turning spinner draws at the cadence.
pub(super) fn tab_draw(state_changed: bool, spinning: bool) -> TabDraw {
    if state_changed {
        TabDraw::Now
    } else if spinning {
        TabDraw::Paced
    } else {
        TabDraw::Nothing
    }
}

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

    /// Whether any of these panes is processing (output within the last 2 s), apart from
    /// whether the output was seen.
    pub(super) fn any_processing(&self, panes: &[PaneId]) -> bool {
        let now = Instant::now();
        panes.iter().any(|id| self.activity.get(id).is_some_and(|a| a.is_processing(now)))
    }

    /// Output since the last tick, the user's view, and the events. When to come back.
    pub(super) fn tick_activity(&mut self) -> Option<Instant> {
        let now = Instant::now();
        let got: Vec<PaneId> = self.panes.iter_mut().filter_map(|(id, p)| std::mem::take(&mut p.got_output).then_some(*id)).collect();
        // Output alone draws through the pacer (`after_output`), never from here: only a change of
        // state (seen, an event) draws at once.
        let mut changed = false;
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
        // A spinner turns at each tick while a session is processing.
        let spinning = !self.globals.bool("HideActivityIndicator") && self.activity.values().any(|a| a.is_processing(now));
        match tab_draw(changed, spinning) {
            TabDraw::Nothing => {}
            TabDraw::Now => self.redraw(),
            TabDraw::Paced => {
                let turning: Vec<PaneId> = self.activity.iter().filter(|(_, a)| a.is_processing(now)).map(|(id, _)| *id).collect();
                self.paced_redraw(&turning, now);
            }
        }
        waiting.then(|| now + Duration::from_millis(500))
    }

    /// An idle, new-output, or activity-after-idle event (notifications and event triggers).
    fn activity_event(&mut self, pane: PaneId, e: ActivityEvent) {
        match e {
            ActivityEvent::Idle => self.post_alert(pane, crate::alerts::AlertKind::Idle),
            // No New Output alerts from a window that broadcasts (iTerm2): the echo of typing
            // would post one per keystroke.
            ActivityEvent::NewOutput => {
                let back = self.current_window();
                let broadcasting = self.enter_pane_window(pane) && self.broadcast.mode(&self.broadcast_view()) != crate::broadcast::Mode::Off;
                if let Some(b) = back {
                    self.enter(b);
                }
                if !broadcasting {
                    self.post_alert(pane, crate::alerts::AlertKind::NewOutput);
                }
            }
            ActivityEvent::ActivityAfterIdle => {}
        }
    }
}

/// A tab shows the activity spinner while it is processing and not the foreground tab, unless
/// `HideActivityIndicator` (`TAB:1328-1332`). New output is apart: unseen output while
/// processing (`activity_icons`).
pub(super) fn tab_processing(any_processing: bool, foreground: bool, hide: bool) -> bool {
    any_processing && !foreground && !hide
}

#[cfg(test)]
mod tests {
    use super::{tab_draw, tab_processing, TabDraw};

    /// 13l review, Critical 1: output alone never asks for a draw here (the pacer owns it);
    /// a state change draws at once, and a spinning indicator draws on the pacer's schedule.
    #[test]
    fn output_alone_does_not_draw_from_the_activity_tick() {
        assert_eq!(tab_draw(false, false), TabDraw::Nothing);
        assert_eq!(tab_draw(true, false), TabDraw::Now);
        assert_eq!(tab_draw(true, true), TabDraw::Now);
        assert_eq!(tab_draw(false, true), TabDraw::Paced);
    }

    #[test]
    fn processing_is_apart_from_new_output() {
        // The spinner: a processing background tab, unless `HideActivityIndicator`.
        assert!(tab_processing(true, false, false));
        for (any, foreground, hide) in [(false, false, false), (true, true, false), (true, false, true), (false, true, true), (true, true, true)] {
            assert!(!tab_processing(any, foreground, hide), "{any} {foreground} {hide}");
        }
    }
}
