//! OSC 9;4 progress bars (13p): the inline bar at a pane's top edge, the tab cell's bar, and the
//! frames that keep them moving (`SV:2120-2184`, `PT:9168-9192`, `TAB:7827-7846`,
//! `TBC:305-356`). The state, the colors and the clock are in `progress`; `scene::progress_bar`
//! draws one.

use super::*;
use crate::progress::{self, Progress};

impl App {
    /// Moves every pane's animation clock to what its program last reported. The clock counts
    /// in fractions of the bar, so a resize never restarts an ease. Run at the start of a frame,
    /// so the tab bar and the panes read the same clocks.
    pub(super) fn update_progress_anims(&mut self, now: Instant) {
        for p in self.panes.values_mut() {
            let shown = p.screen.progress;
            p.progress_anim.update(shown, 1.0, now);
        }
    }

    /// The scheme's name and Enable Progress Bars for the pane's profile.
    fn progress_profile(&self, id: PaneId) -> (bool, String) {
        match self.profile_of(id) {
            Some(p) => (p.bool("Enable Progress Bars"), p.string("Progress Bar Color Scheme")),
            None => (true, "default".to_string()),
        }
    }

    /// The tab's bar for its cell (`PTYTab progress`, `TBC cellShouldShowTabProgressBar`): the
    /// active pane's progress when visible, else the first other visible one, drawn in the
    /// active pane's scheme and only with more than one tab and the active pane's Enable.
    pub(super) fn tab_progress_look(&self, t: &Tab) -> Option<scene::ProgressLook> {
        let state = |id: &PaneId| self.panes.get(id).map_or(Progress::Stopped, |p| p.screen.progress);
        let ids = t.layout.panes();
        let others: Vec<PaneId> = ids.iter().copied().filter(|id| *id != t.focused).collect();
        let shown = progress::tab_progress(state(&t.focused), &others.iter().map(state).collect::<Vec<_>>());
        let (enabled, scheme) = self.progress_profile(t.focused);
        if !progress::tab_shown(self.tabs.len(), enabled, shown) {
            return None;
        }
        // The pane the state came from carries the clock.
        let from = if state(&t.focused).visible() { Some(t.focused) } else { others.iter().copied().find(|id| state(id).visible()) };
        let anim = from.and_then(|id| self.panes.get(&id)).map(|p| p.progress_anim.clone())?;
        Some((shown, anim, scheme, self.dark()))
    }

    /// Draws pane `id`'s own bar over the top edge of `rect` when the rules put it there: the
    /// pane's full width, `Progress Bar Height` points (cut to the top margin), over the
    /// profile's background (`SV:1307-1316`).
    pub(super) fn draw_inline_progress(&self, frame: &mut Frame, id: PaneId, rect: Rect) {
        let Some(p) = self.panes.get(&id) else { return };
        let Some(tab) = self.tabs.get(self.active) else { return };
        let (enabled, scheme) = self.progress_profile(id);
        // The tab's cell shows the bar unless the tab has no cell in the bar (collapsed away or
        // in the overflow menu), where the inline bar stands in (`isHiddenInBar`).
        let in_bar = self.tab_bar_rect().is_some() && self.tab_bar.cells.iter().any(|(i, _, _)| *i == self.active);
        if !progress::inline_shown(enabled, p.screen.progress, self.tabs.len(), tab.layout.panes().len(), in_bar) {
            return;
        }
        let points = self.profile_of(id).map_or(2.0, |pr| pr.float("Progress Bar Height"));
        let height = (progress::bar_height(points, self.globals.int("TerminalVMargin").max(0) as f64) as f32 * self.scale()).round().max(1.0);
        let bar = Rect { h: height, ..rect };
        let bg = p.screen.palette.bg;
        let dark = crate::tab_theme::hsp(bg) < 0.5;
        scene::progress_bar(frame, bar, &(p.screen.progress, p.progress_anim.clone(), scheme, dark), Some(bg), rect, Instant::now());
    }

    /// Asks for the frames bars need: a redraw of each animating pane's window each `FAST_GAP`,
    /// and nothing at all (the loop sleeps) when no bar moves. Run in `about_to_wait`.
    pub(super) fn tick_progress(&mut self) {
        let now = Instant::now();
        let running: Vec<PaneId> = self.panes.iter().filter(|(id, p)| p.screen.progress.visible() && p.progress_anim.animating(now) && self.progress_profile(**id).0).map(|(id, _)| *id).collect();
        if running.is_empty() {
            self.progress_frame = None;
            return;
        }
        if self.progress_frame.is_none_or(|due| now >= due) {
            for id in running {
                self.redraw_pane_window(id);
            }
            self.progress_frame = Some(now + crate::cadence::FAST_GAP);
        }
    }

    /// When the next bar frame is due, None when no bar animates (set by `tick_progress`).
    pub(super) fn progress_wake(&self) -> Option<Instant> {
        self.progress_frame
    }
}

#[cfg(test)]
mod tests {
    /// The bars' wiring: the clocks move before the tab bar's looks are made, the inline bar
    /// is drawn inside the pane's iteration, and the frame deadline joins the one `wake`
    /// minimum (no second timer) and stops when nothing animates.
    #[test]
    fn progress_is_wired_into_the_frame_and_the_wake() {
        let m = include_str!("mod.rs");
        let (anims, looks) = (m.find("self.update_progress_anims(Instant::now());").unwrap(), m.find("let mut looks: Vec<scene::TabLook>").unwrap());
        assert!(anims < looks, "clocks first");
        assert!(m.contains("self.draw_inline_progress(&mut frame, id, rect);"));
        assert!(m.contains("let progress_due = self.progress_wake();"));
        assert!(m.contains("cursor_due, frame_due, flicker_due, progress_due]"), "folded into the one wake array");
        assert!(m.contains("progress_frame: Option<Instant>,") && m.contains("progress_frame: None,"));
        let me = include_str!("progress_ui.rs").split("#[cfg(test)]").next().unwrap();
        assert!(me.contains("self.progress_frame = None;"), "a still bar lets the loop sleep");
        assert!(!me.contains("thread::spawn") && !me.contains("set_timer"), "no second timer");
        assert!(include_str!("requests_ui.rs").contains("Request::Progress(_) => self.redraw_pane_window(pane),"));
        assert!(include_str!("requests_ui.rs").contains("progress: self.tab_progress_look(t) }"));
        assert!(include_str!("frames_ui.rs").contains("|| self.progress_frame.is_some()"));
    }
}
