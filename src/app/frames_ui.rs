//! The display pacer (13l): iTerm2 draws a session's output on an update timer whose period is
//! its update cadence (`iTermUpdateCadenceController`), at once for a typed echo, and every
//! other redraw request (keys, the mouse, resizes, bars, the cursor) draws immediately. Tandem
//! asks a window to draw after pane output only when its `Pacer` says a frame is due, and wakes
//! at the due time otherwise.

use super::*;
use crate::cadence::{self, Prefs, State};
use crate::power;

/// iTerm2's power polls are slow; Windows' battery saver changes rarely (the m9i rule).
const POWER_POLL: Duration = Duration::from_secs(1);

/// What a window's decision needs from its front tab's panes.
struct Reading {
    states: Vec<State>,
    typing: bool,
}

impl App {
    /// The cadence settings, read from the globals each time (a few lookups).
    pub(super) fn cadence_prefs(&self) -> Prefs {
        Prefs::from_globals(&self.globals)
    }

    /// Low Power Mode: Windows battery saver (`power_tick` keeps `power` current).
    fn power_saving(&self) -> bool {
        self.power.low_power_mode()
    }

    /// Samples the power line at most once a second (the m9i rule). A change of low power mode
    /// re-paces every window: a deferred frame is recomputed on its next output, and a still
    /// window gets a frame now so nothing waits on the old rate. Returns the next poll.
    pub(super) fn power_tick(&mut self, now: Instant) -> Option<Instant> {
        if self.power_due.is_some_and(|t| now < t) {
            return self.power_due;
        }
        self.power_due = Some(now + POWER_POLL);
        #[cfg(windows)]
        if let Some(state) = power::sample() {
            if self.set_power(state) {
                self.repace_all();
            }
        }
        self.power_due
    }

    /// Stores a sample; true when low power mode changed.
    fn set_power(&mut self, state: power::State) -> bool {
        let changed = self.power.low_power_mode() != state.low_power_mode();
        self.power = state;
        changed
    }

    /// Forgets every window's deferred frame time (computed from the old rate) and draws.
    fn repace_all(&mut self) {
        self.pacer = Default::default();
        for (_, s) in self.windows.parked_mut() {
            s.pacer = Default::default();
        }
        self.redraw();
    }

    /// Each pane's cadence state and whether a typed echo is in progress: `changed` are the
    /// panes that just got output. `Throughput::estimate` mutates, so it runs once per pane.
    fn read_panes(&mut self, panes: &[PaneId], changed: &[PaneId], active: bool, now: Instant) -> Reading {
        let low_power = self.power_saving();
        let last_key = self.last_keystroke;
        let mut reading = Reading { states: Vec::new(), typing: false };
        for id in panes {
            let last_output = self.activity.get(id).and_then(|a| a.last_output());
            let Some(p) = self.panes.get_mut(id) else { continue };
            let output = changed.contains(id);
            let idle = !output && !p.got_output && last_output.is_none_or(|t| now.saturating_duration_since(t) > cadence::IDLE_AFTER);
            let throughput = p.throughput.estimate(now);
            reading.typing |= output && cadence::typing_now(throughput, last_key, now);
            reading.states.push(State { active, idle, visible: true, alt_screen: p.own().alt_screen(), throughput, low_power });
        }
        reading
    }

    /// 13k's one decision for "a pane blinking a cursor or animating", for the key window.
    fn window_active(&self) -> bool {
        self.blink_clock_needed() || self.anim_frame.is_some()
    }

    /// The current window's frame interval: the smallest over the panes of its front tab.
    #[allow(dead_code)]
    pub(super) fn window_interval(&mut self, now: Instant) -> Duration {
        let panes = self.front_panes_here();
        let active = self.window_active();
        let reading = self.read_panes(&panes, &[], active, now);
        cadence::window_interval(&reading.states, &self.cadence_prefs())
    }

    /// The panes of the front tab of the current window when it is on screen.
    fn front_panes_here(&self) -> Vec<PaneId> {
        let on_screen = self.gpu.as_ref().is_some_and(|g| g.window.is_minimized() != Some(true) && g.window.is_visible() != Some(false));
        self.tabs.get(self.active).filter(|_| on_screen).map(|t| t.layout.panes()).unwrap_or_default()
    }

    /// Redraws after output in `outputs`: each window showing one of those panes in its front tab
    /// draws at once when its frame is due or a typed echo is in, else at the due time
    /// (`frame_wake`). A window that is hidden or minimized, or has none of them in front,
    /// is not asked at all.
    pub(super) fn paced_redraw(&mut self, outputs: &[PaneId], now: Instant) {
        // Not pane output (an exit, an API call with nothing fed): draws at once.
        if outputs.is_empty() {
            return self.redraw();
        }
        let prefs = self.cadence_prefs();
        let mut drew = false;
        let panes = self.front_panes_here();
        let changed: Vec<PaneId> = outputs.iter().copied().filter(|id| panes.contains(id)).collect();
        if !changed.is_empty() {
            let active = self.window_active();
            let reading = self.read_panes(&panes, &changed, active, now);
            if self.pacer.output(now, cadence::window_interval(&reading.states, &prefs), reading.typing) {
                self.redraw_here();
                drew = true;
            }
        }
        let on_screen = |w: &Window| w.is_visible() != Some(false) && w.is_minimized() != Some(true);
        let parked: Vec<(u64, Vec<PaneId>)> = self
            .windows
            .parked()
            .filter(|(_, s)| s.gpu.as_ref().is_some_and(|g| on_screen(&g.window)))
            .filter_map(|(k, s)| s.tabs.get(s.active).map(|t| (*k, t.layout.panes())))
            .filter(|(_, p)| outputs.iter().any(|id| p.contains(id)))
            .collect();
        for (key, panes) in parked {
            let changed: Vec<PaneId> = outputs.iter().copied().filter(|id| panes.contains(id)).collect();
            let reading = self.read_panes(&panes, &changed, false, now);
            let gap = cadence::window_interval(&reading.states, &prefs);
            if let Some((_, s)) = self.windows.parked_mut().find(|(k, _)| **k == key) {
                if s.pacer.output(now, gap, reading.typing) {
                    if let Some(g) = &s.gpu {
                        g.window.request_redraw();
                    }
                    drew = true;
                }
            }
        }
        // The screenshot panel's buttons follow the pane's selection.
        if let Some(s) = self.shot.as_ref().filter(|_| drew) {
            s.window.request_redraw();
        }
    }

    /// The earliest time a window's deferred frame is due. At or past a due time that window is
    /// asked to draw (and the time cleared), so a pending frame is never left until some other
    /// event. None when nothing is waiting: the loop then sleeps.
    pub(super) fn frame_wake(&mut self) -> Option<Instant> {
        let now = Instant::now();
        if self.pacer.take_due(now) {
            self.redraw_here();
        }
        let mut due = self.pacer.due();
        for (_, s) in self.windows.parked_mut() {
            if s.pacer.take_due(now) {
                if let Some(g) = &s.gpu {
                    g.window.request_redraw();
                }
            }
            due = [due, s.pacer.due()].into_iter().flatten().min();
        }
        due
    }
}
