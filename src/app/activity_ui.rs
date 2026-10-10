//! Output and idle per session: the tab indicators, and the events notifications and event
//! triggers use.

use std::time::{Duration, Instant};

use super::*;
use crate::activity::ActivityEvent;

/// What a decision asks of the display (13l: the one place that says when output, a pump or
/// the activity tick may draw). Nothing here draws at once: input, the mouse, resizes and the
/// cursor do that on their own paths.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Draw {
    None,
    Paced,
}

/// Output on `output_panes` panes, a real tab-bar state change (an indicator that was showing
/// went away, an idle or new-output event), or a spinner turning: all draw through the pacer;
/// nothing else draws. An empty pump (no pane got output, nothing changed) never draws.
pub(super) fn output_draw(output_panes: usize, tab_state_changed: bool, spinning: bool) -> Draw {
    if output_panes > 0 || tab_state_changed || spinning {
        Draw::Paced
    } else {
        Draw::None
    }
}

/// A pane's seen flip changes the tab bar only when its new-output indicator was already
/// showing before this tick's output: output into a pane being watched is seen in the same
/// tick and never showed.
pub(super) fn seen_changed_tab(was_unseen_before_output: bool, looking: bool) -> bool {
    was_unseen_before_output && looking
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
        let mut affected: Vec<PaneId> = Vec::new();
        let was_unseen: Vec<PaneId> = self.activity.iter().filter(|(_, a)| a.unseen()).map(|(id, _)| *id).collect();
        for id in got {
            self.activity.entry(id).or_default().output(now);
        }
        let looking: Vec<PaneId> = self.panes.keys().copied().filter(|id| self.looking_at(*id)).collect();
        for id in looking {
            if let Some(a) = self.activity.get_mut(&id).filter(|a| a.unseen()) {
                a.seen();
                if seen_changed_tab(was_unseen.contains(&id), true) {
                    changed = true;
                    affected.push(id);
                }
            }
        }
        self.activity.retain(|id, _| self.panes.contains_key(id));
        let mut events: Vec<(PaneId, ActivityEvent)> = Vec::new();
        for (id, a) in self.activity.iter_mut() {
            events.extend(a.tick(now).into_iter().map(|e| (*id, e)));
        }
        let waiting = self.activity.values().any(|a| a.in_burst());
        changed |= !events.is_empty();
        affected.extend(events.iter().map(|(id, _)| *id));
        for (pane, e) in events {
            self.activity_event(pane, e);
        }
        // A spinner turns at each tick while a session is processing.
        let spinning = !self.globals.bool("HideActivityIndicator") && self.activity.values().any(|a| a.is_processing(now));
        match output_draw(0, changed, spinning) {
            Draw::None => {}
            Draw::Paced => {
                let mut panes: Vec<PaneId> = self.activity.iter().filter(|(_, a)| a.is_processing(now)).map(|(id, _)| *id).collect();
                panes.extend(affected);
                self.paced_redraw_state(&panes, now);
            }
        }
        waiting.then(|| now + Duration::from_millis(500))
    }

    /// An idle, new-output, or activity-after-idle event (notifications and event triggers).
    fn activity_event(&mut self, pane: PaneId, e: ActivityEvent) {
        match e {
            ActivityEvent::Idle => {
                self.post_alert(pane, crate::alerts::AlertKind::Idle);
            }
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
    use super::{output_draw, seen_changed_tab, tab_processing, Draw};

    /// 13l review round 2, Critical 1: no source of output alone draws at once. Output (a pty,
    /// or a tmux or ssh pump that fed a pane), a tab state change and a spinner go through the
    /// pacer; an empty pump draws nothing.
    #[test]
    fn output_sources_draw_through_the_pacer() {
        assert_eq!(output_draw(1, false, false), Draw::Paced);
        assert_eq!(output_draw(3, true, true), Draw::Paced);
        assert_eq!(output_draw(0, true, false), Draw::Paced);
        assert_eq!(output_draw(0, false, true), Draw::Paced);
        assert_eq!(output_draw(0, false, false), Draw::None, "a pump that fed no pane draws nothing");
    }

    /// The focused pane's own output is seen in the tick it arrives and never showed an
    /// indicator, so it changes nothing in the tab bar; a pane whose indicator was up does.
    #[test]
    fn watched_output_does_not_change_the_tab_bar() {
        assert!(!seen_changed_tab(false, true));
        assert!(seen_changed_tab(true, true));
        assert!(!seen_changed_tab(true, false), "not looking: still unseen, nothing flips");
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
