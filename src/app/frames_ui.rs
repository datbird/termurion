//! The display pacer (13l): iTerm2 draws a session's output on an update timer whose period is
//! its update cadence (`iTermUpdateCadenceController`), at once for a typed echo, and every
//! other redraw request (keys, the mouse, resizes, bars, the cursor) draws immediately. Termurion
//! asks a window to draw after pane output only when its `Pacer` says a frame is due, and wakes
//! at the due time otherwise.

use super::*;
use crate::cadence::{self, Prefs, State};
use crate::power;

/// What a window's decision needs from its front tab's panes.
struct Reading {
    states: Vec<State>,
    typing: bool,
}

/// Which of a burst of output a window shows: panes of its front tab, and whether any is in
/// its other tabs (the tab bar's title, new-output icon and exit state change there).
#[derive(Debug, Default)]
pub(super) struct Routed {
    pub front: Vec<PaneId>,
    pub back: Vec<PaneId>,
}

pub(super) fn route_output(outputs: &[PaneId], front: &[PaneId], back: &[PaneId]) -> Routed {
    let pick = |of: &[PaneId]| outputs.iter().copied().filter(|id| of.contains(id)).collect();
    Routed { front: pick(front), back: pick(back) }
}

/// A window's tabs' panes split into its front tab's and all the others'.
pub(super) fn split_tabs(tabs: Vec<Vec<PaneId>>, active: usize) -> (Vec<PaneId>, Vec<PaneId>) {
    let front = tabs.get(active).cloned().unwrap_or_default();
    let back = tabs.into_iter().enumerate().filter(|(i, _)| *i != active).flat_map(|(_, p)| p).collect();
    (front, back)
}

impl App {
    /// The cadence settings, read from the globals each time (a few lookups).
    pub(super) fn cadence_prefs(&self) -> Prefs {
        Prefs::from_globals(&self.globals)
    }

    /// Low Power Mode: Windows battery saver, sampled when asked (at most once a second; no
    /// timer wakes an idle app for it).
    fn power_saving(&mut self, now: Instant) -> bool {
        self.power.low_power_mode(now, power::sample)
    }

    /// Each pane's cadence state and whether a typed echo is in progress: `changed` are the
    /// panes that just got output. `Throughput::estimate` mutates, so it runs once per pane.
    fn read_panes(&mut self, panes: &[PaneId], changed: &[PaneId], active: bool, now: Instant, with_output: bool) -> Reading {
        let low_power = self.power_saving(now);
        let last_key = self.last_keystroke;
        let mut reading = Reading { states: Vec::new(), typing: false };
        for id in panes {
            let last_output = self.activity.get(id).and_then(|a| a.last_output());
            let Some(p) = self.panes.get_mut(id) else { continue };
            let output = with_output && changed.contains(id);
            let idle = !output && !p.got_output && last_output.is_none_or(|t| now.saturating_duration_since(t) > cadence::idle_after(p.idle_time));
            // A tick with no new output does not run the estimator again (it mutates).
            let throughput = if with_output { p.throughput.estimate(now) } else { 0 };
            reading.typing |= output && cadence::typing_now(throughput, last_key, now);
            reading.states.push(State { active, idle, visible: true, alt_screen: p.own().alt_screen(), throughput, low_power });
        }
        reading
    }

    /// 13k's one decision for "a pane blinking a cursor or animating", for the key window.
    fn window_active(&self) -> bool {
        self.blink_clock_needed() || self.anim_frame.is_some() || self.progress_frame.is_some()
    }

    /// The panes of the current window's front tab, and of its other tabs; both empty when the
    /// window is not on screen.
    pub(super) fn tab_panes_here(&self) -> (Vec<PaneId>, Vec<PaneId>) {
        let shown = self.hotkey.as_ref().map(|h| h.shown);
        let on_screen = self.gpu.as_ref().is_some_and(|g| window_on_screen(g.window.is_visible(), g.window.is_minimized(), shown));
        if !on_screen {
            return Default::default();
        }
        split_tabs(self.tabs.iter().map(|t| t.layout.panes()).collect(), self.active)
    }

    /// One window's decision after `outputs`: None when none of them is in the window, else the
    /// frame interval over its front panes plus the panes that got output in its other tabs (the
    /// tab bar shows their title, new-output icon and exit state) and whether a typed echo is in.
    fn output_decision(&mut self, front: &[PaneId], back: &[PaneId], outputs: &[PaneId], active: bool, now: Instant, with_output: bool) -> Option<(Duration, bool)> {
        let routed = route_output(outputs, front, back);
        if routed.front.is_empty() && routed.back.is_empty() {
            return None;
        }
        let changed: Vec<PaneId> = routed.front.iter().chain(&routed.back).copied().collect();
        let considered: Vec<PaneId> = front.iter().chain(&routed.back).copied().collect();
        let reading = self.read_panes(&considered, &changed, active, now, with_output);
        Some((cadence::window_interval(&reading.states, &self.cadence_prefs()), reading.typing))
    }

    /// Redraws after output in `outputs`: each window showing one of those panes (in any tab)
    /// draws at once when its frame is due or a typed echo is in, else at the due time
    /// (`frame_wake`). A window that is hidden or minimized is not asked at all.
    pub(super) fn paced_redraw(&mut self, outputs: &[PaneId], now: Instant) {
        self.paced_draw(outputs, now, true);
    }

    /// The same for a state change in `panes` that is not new output (a spinner turning, a tab
    /// indicator going away, an activity event): the pacer decides, the estimator is not run.
    pub(super) fn paced_redraw_state(&mut self, panes: &[PaneId], now: Instant) {
        self.paced_draw(panes, now, false);
    }

    fn paced_draw(&mut self, outputs: &[PaneId], now: Instant, with_output: bool) {
        // Nothing fed, nothing changed (an empty tmux or ssh pump): nothing to draw. Every
        // real state change draws through its own `redraw` call.
        if outputs.is_empty() {
            return;
        }
        let mut drew = false;
        let (front, back) = self.tab_panes_here();
        let active = self.window_active();
        if let Some((gap, typing)) = self.output_decision(&front, &back, outputs, active, now, with_output) {
            if self.pacer.output(now, gap, typing) {
                self.redraw_here();
                drew = true;
            }
        }
        let parked: Vec<(u64, Vec<PaneId>, Vec<PaneId>)> = self
            .windows
            .parked()
            .filter(|(_, s)| s.gpu.as_ref().is_some_and(|g| window_on_screen(g.window.is_visible(), g.window.is_minimized(), s.hotkey.as_ref().map(|h| h.shown))))
            .map(|(k, s)| {
                let (front, back) = split_tabs(s.tabs.iter().map(|t| t.layout.panes()).collect(), s.active);
                (*k, front, back)
            })
            .collect();
        for (key, front, back) in parked {
            let Some((gap, typing)) = self.output_decision(&front, &back, outputs, false, now, with_output) else { continue };
            if let Some((_, s)) = self.windows.parked_mut().find(|(k, _)| **k == key) {
                if s.pacer.output(now, gap, typing) {
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

    /// Copy Performance Stats: the saved renderer logs go on the clipboard (counters and
    /// timings only, `iTermPreciseTimerGetSavedLogs`).
    pub(super) fn copy_performance_stats(&mut self) {
        let text = self.perf.text();
        match self.clipboard.as_mut().map(|cb| cb.set_text(text).map_err(|e| e.to_string())) {
            Some(Err(e)) => self.banners.set("clipboard", format!("Copy failed: {e}")),
            None => self.banners.set("clipboard", "Copy failed: the clipboard is not available"),
            Some(Ok(())) => {}
        }
    }

    /// GPU Renderer Availability: Termurion always draws on the GPU, so it says enabled and names
    /// the adapter and backend this window draws on.
    pub(super) fn gpu_renderer_availability(&mut self) {
        use crate::perf_stats as ps;
        let info = self.gpu.as_ref().map(|g| g.render_adapter().get_info());
        let backend = info.as_ref().map(|i| format!("{:?}", i.backend));
        let text = ps::gpu_text(info.as_ref().zip(backend.as_deref()).map(|(i, b)| (i.name.as_str(), b)));
        let ask = super::scripts_alerts::ask(ps::GPU_TITLE, &text, &[crate::api_auth::OK], None, crate::warnings::Kind::Persistent, super::scripts_alerts::Then::Nothing);
        self.ask_scripts(ask);
    }

    /// `MetalRedrawPeriod`, in seconds (0.5 by default; 0 or less forces nothing).
    fn redraw_period(&self) -> Duration {
        let secs = self.globals.number("MetalRedrawPeriod").unwrap_or(0.5);
        if secs.is_finite() && secs > 0.0 { Duration::from_secs_f64(secs) } else { Duration::ZERO }
    }

    /// Whether the FPS meter is on (`ShowMetalFPSmeter`).
    fn fps_meter_on(&self) -> bool {
        self.globals.bool("ShowMetalFPSmeter")
    }

    /// The meter's text for this window, None while the meter is off.
    pub(super) fn fps_overlay(&self) -> Option<String> {
        self.fps_meter_on().then(|| crate::perf_stats::fps_text(self.frame_number, self.fps.value()))
    }

    /// Called by `render` for each frame: while the meter is on it adds the time since the
    /// window's last frame to the average and counts the frame. Off, it costs nothing.
    pub(super) fn note_frame(&mut self, before: Option<Instant>, now: Instant) {
        if !self.fps_meter_on() {
            return;
        }
        if let Some(b) = before {
            self.fps.add(now.saturating_duration_since(b).as_secs_f64());
        }
        self.frame_number += 1;
    }

    /// The earliest time a window needs a frame: its deferred output frame, or the minimum
    /// redraw period (`MetalRedrawPeriod`) after its last one. At or past a due time that
    /// window (and only it) is asked to draw. Hidden or minimized windows are never forced;
    /// with a period of 0 nothing is forced and an idle app sleeps. None when nothing waits.
    pub(super) fn frame_wake(&mut self) -> Option<Instant> {
        let now = Instant::now();
        let period = self.redraw_period();
        let on_screen = |w: &Window, hotkey: Option<&hotkey_ui::HotkeyWin>| window_on_screen(w.is_visible(), w.is_minimized(), hotkey.map(|h| h.shown));
        if self.pacer.take_due(now) {
            self.redraw_here();
        }
        let mut due = self.pacer.due();
        if self.gpu.as_ref().is_some_and(|g| on_screen(&g.window, self.hotkey.as_ref())) {
            due = [due, forced_frame(self.pacer.last(), period, now, || self.redraw_here())].into_iter().flatten().min();
        }
        for (_, s) in self.windows.parked_mut() {
            if s.pacer.take_due(now) {
                if let Some(g) = &s.gpu {
                    g.window.request_redraw();
                }
            }
            due = [due, s.pacer.due()].into_iter().flatten().min();
            if let Some(g) = s.gpu.as_ref().filter(|g| on_screen(&g.window, s.hotkey.as_ref())) {
                due = [due, forced_frame(s.pacer.last(), period, now, || g.window.request_redraw())].into_iter().flatten().min();
            }
        }
        due
    }

    /// When the earliest hold on a pane's last picture ends (Reduce Flicker). None while no
    /// pane is held, so an idle app sleeps.
    pub(super) fn flicker_wake(&self) -> Option<Instant> {
        self.panes.values().filter_map(|p| p.held_until).min()
    }

    /// Releases the holds that have ended: each pane draws live again, and its own window (it
    /// may be a parked one) is asked for a frame. Every hold is at most `flicker::HOLD` long,
    /// so none can outlast it.
    pub(super) fn flicker_tick(&mut self, now: Instant) -> Option<Instant> {
        let released = crate::flicker::release_holds(self.panes.iter_mut().map(|(id, p)| (*id, &mut p.held_until)), now);
        for id in released {
            self.redraw_pane_window(id);
        }
        self.flicker_wake()
    }
}

/// Whether a window is on screen: not hidden, not minimized and, for a hotkey window, shown
/// (`shown` false means DWM-cloaked, which winit still reports as visible).
pub(super) fn window_on_screen(visible: Option<bool>, minimized: Option<bool>, shown: Option<bool>) -> bool {
    visible != Some(false) && minimized != Some(true) && shown != Some(false)
}

/// One window's minimum-period frame: asks for it when due and returns the next wake (a frame
/// asked for now is next due a period on, so the loop does not spin until it is drawn).
fn forced_frame(last: Option<Instant>, period: Duration, now: Instant, draw: impl FnOnce()) -> Option<Instant> {
    let next = cadence::period_due(last, period, now)?;
    if next > now {
        return Some(next);
    }
    draw();
    Some(now + period)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 13l Task 4 fix (P18): a window is forced a frame only when it is on screen: not hidden,
    /// not minimized and, for a hotkey window, not cloaked (winit still calls a cloaked window
    /// visible).
    #[test]
    fn cloaked_hotkey_windows_are_not_on_screen() {
        assert!(window_on_screen(Some(true), Some(false), None));
        assert!(window_on_screen(None, None, Some(true)));
        assert!(!window_on_screen(Some(false), Some(false), None));
        assert!(!window_on_screen(Some(true), Some(true), None));
        assert!(!window_on_screen(Some(true), Some(false), Some(false)), "cloaked");
    }

    /// 13l review, Important 2: output in a window's other tabs still reaches the tab bar.
    #[test]
    fn output_in_a_back_tab_is_routed_to_the_window() {
        let (front, back) = ([1, 2], [3, 4]);
        let r = route_output(&[2, 3, 9], &front, &back);
        assert_eq!((r.front, r.back), (vec![2], vec![3]));
        let r = route_output(&[1], &front, &back);
        assert_eq!((r.front, r.back), (vec![1], Vec::new()));
        let r = route_output(&[4], &front, &back);
        assert_eq!((r.front, r.back), (Vec::new(), vec![4]));
        let r = route_output(&[9], &front, &back);
        assert_eq!((r.front, r.back), (Vec::new(), Vec::new()));
    }

    /// A still window is drawn once its period has passed, and not before; a zero period never
    /// forces a frame and never sets a wake (an idle app sleeps).
    #[test]
    fn forced_frame_waits_for_the_period() {
        let (t, p) = (Instant::now(), Duration::from_millis(500));
        let mut asked = 0;
        assert_eq!(forced_frame(Some(t), p, t + Duration::from_millis(100), || asked += 1), Some(t + p));
        assert_eq!(asked, 0);
        let now = t + p;
        assert_eq!(forced_frame(Some(t), p, now, || asked += 1), Some(now + p));
        assert_eq!(asked, 1);
        assert_eq!(forced_frame(Some(t), Duration::ZERO, now, || asked += 1), None);
        assert_eq!(asked, 1);
    }

    /// A burst of output wakes inside one frame draws once; the rest wait for the due time.
    #[test]
    fn a_burst_of_output_wakes_draws_once() {
        let mut pacer = cadence::Pacer::default();
        let gap = Duration::from_micros(15_667);
        let t0 = Instant::now();
        let mut draws = 0;
        for i in 0..10 {
            let now = t0 + Duration::from_millis(i);
            if pacer.output(now, gap, false) {
                pacer.drew(now);
                draws += 1;
            }
        }
        assert_eq!(draws, 1);
        assert!(pacer.due().is_some());
    }
}
