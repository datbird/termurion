//! The text cursor on screen: what `scene::pane_with` is told to draw for each pane, and the
//! clocks behind it.

use super::*;
use crate::cursor_anim;
use crate::cursor_look::{CursorLook, Geometry};
use alacritty_terminal::grid::Dimensions;

impl App {
    /// How the cursor of `pane` is drawn. `visible` is whether it is shown at all (an Auto
    /// Composer's cover); the blink phase is folded in here. `App.cursor` is the mouse pointer,
    /// never read here.
    pub(super) fn cursor_look_for(&self, pane: PaneId, focused: bool, visible: bool) -> CursorLook {
        let hide_unfocused = self.panes.get(&pane).and_then(|p| self.store.get(&p.profile)).is_some_and(|p| p.bool("Cursor Hidden When Unfocused"));
        // Smooth blink: the cursor is drawn at the fade's alpha whatever the on/off phase.
        let smooth = self.cursor_alpha.filter(|_| focused);
        CursorLook {
            hide_unfocused,
            alpha: smooth.unwrap_or(1.0),
            geometry: Geometry::from_globals(&self.globals, self.scale(), self.base_cell()),
            ..CursorLook::shown(visible && (smooth.is_some() || self.cursor_visible_now(pane, focused)))
        }
    }

    /// True when the key window's focused pane's cursor blinks, which asks for the blink clock.
    /// The terminal's own style is read, not the profile: it carries a program's change where
    /// "Allow Change Cursor Blink" lets it. 13m adds blinking text it may blink here.
    pub(super) fn blink_clock_needed(&self) -> bool {
        self.focused && self.tabs.get(self.active).and_then(|t| self.panes.get(&t.focused)).is_some_and(|p| p.screen.cursor_blinks())
    }

    /// Whether the blink phase leaves `pane`'s cursor drawn. Only the key window's focused pane
    /// (`focused`) can meet the blink condition; every other pane answers true.
    pub(super) fn cursor_visible_now(&self, pane: PaneId, focused: bool) -> bool {
        let Some(p) = self.panes.get(&pane) else { return true };
        let still = p.motion.since_moved(Instant::now());
        !cursor_anim::blink_condition(p.screen.cursor_blinks(), focused, still) || self.blink.visible
    }

    /// Advances the blink clock and asks for a redraw when the phase flips. Returns the next
    /// flip, or None when nothing blinks (the loop then sleeps).
    pub(super) fn tick_cursor(&mut self) -> Option<Instant> {
        let now = Instant::now();
        if let Some(due) = self.tick_smooth_blink(now) {
            self.cursor_due = due;
            return due;
        }
        self.cursor_due = if self.blink_clock_needed() {
            let half = cursor_anim::half_period(self.globals.number("TimeBetweenBlinks").unwrap_or(0.5));
            let (changed, next) = self.blink.tick(now, half);
            if changed {
                self.redraw();
            }
            Some(next)
        } else {
            // Nothing blinks: the phase rests visible, and a later start begins a fresh period.
            if !self.blink.visible {
                self.blink.visible = true;
                self.redraw();
            }
            self.blink.flipped = now;
            None
        };
        self.cursor_due
    }

    /// The smooth blink (`iTermCursorBlinkFadeAnimator`): None when the focused pane's profile
    /// has it off (the on/off blink runs instead), else Some(the next frame, if any). The alpha
    /// is stored for drawing; a redraw is asked only when it changed, so a dwell sleeps and a
    /// fade runs frame by frame (`FAST_GAP`).
    fn tick_smooth_blink(&mut self, now: Instant) -> Option<Option<Instant>> {
        let pane = self.tabs.get(self.active).map(|t| t.focused);
        let p = pane.and_then(|id| self.panes.get(&id));
        let smooth = p.and_then(|p| self.store.get(&p.profile)).filter(|prof| prof.bool("Cursor Smooth Blink")).map(crate::cursor_anim::FadeTimes::from_profile);
        let (Some(p), Some(times)) = (p, smooth) else {
            self.blink_fade = Default::default();
            self.cursor_alpha = None;
            return None;
        };
        let blinks = p.screen.cursor_blinks();
        let since = p.motion.since_moved(now);
        let condition = cursor_anim::blink_condition(blinks, self.focused, since);
        let (alpha, next) = self.blink_fade.alpha(condition, now, &times);
        let changed = self.cursor_alpha != Some(alpha);
        self.cursor_alpha = Some(alpha);
        if changed {
            self.redraw();
        }
        let wake = if condition {
            next.map(|d| now + d.max(crate::cadence::FAST_GAP))
        } else if self.focused && blinks && since != Duration::MAX && since <= cursor_anim::MOVE_HOLD {
            // Solid after a move: wake when the hold ends and the blink may start.
            Some(now + (cursor_anim::MOVE_HOLD - since) + Duration::from_millis(1))
        } else {
            None
        };
        Some(wake)
    }

    /// The deadline `tick_cursor` computed, for `about_to_wait`'s `wake` minimum.
    #[allow(dead_code)]
    pub(super) fn cursor_wake(&self) -> Option<Instant> {
        self.cursor_due
    }

    /// Feeds the cursor of every visible pane of the active tab to its `Motion`, before the
    /// frame's pane loop borrows the tab (the loop only reads `Motion`).
    pub(super) fn prepare_cursors(&mut self) {
        let Some(tab) = self.tabs.get(self.active) else { return };
        let panes: Vec<PaneId> = self.visible(tab).into_iter().map(|(id, _)| id).collect();
        let now = Instant::now();
        for id in panes {
            self.note_cursor_moves(id, now);
        }
    }

    /// Notes `pane`'s cursor for a frame. iTerm2 compares the screen-relative grid point
    /// (`PTYTextView.m:2098`, `iTermTextDrawingHelper.m:3896`), so scrolling output under a
    /// cursor on the last row is no move; a hidden cursor leaves the stored cell alone.
    fn note_cursor_moves(&mut self, pane: PaneId, now: Instant) {
        let Some(p) = self.panes.get_mut(&pane) else { return };
        let cell = scene::cursor_cell(&p.screen).map(|_| {
            let point = p.screen.term().grid().cursor.point;
            (i64::from(point.line.0), point.column.0)
        });
        p.motion.note(cell, now);
    }
}
