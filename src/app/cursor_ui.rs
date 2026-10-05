//! The text cursor on screen: what `scene::pane_with` is told to draw for each pane, and the
//! clocks behind it.

use super::*;
use crate::cursor_anim::{self, Metrics};
use crate::cursor_look::{CursorLook, Geometry};

impl App {
    /// How the cursor of `pane` is drawn. `visible` is whether it is shown at all (an Auto
    /// Composer's cover); the blink phase is folded in here. `App.cursor` is the mouse pointer,
    /// never read here.
    pub(super) fn cursor_look_for(&self, pane: PaneId, focused: bool, visible: bool) -> CursorLook {
        let hide_unfocused = self.panes.get(&pane).and_then(|p| self.store.get(&p.profile)).is_some_and(|p| p.bool("Cursor Hidden When Unfocused"));
        // Smooth blink: the cursor is drawn at the fade's alpha whatever the on/off phase.
        let smooth = self.cursor_alpha.filter(|_| focused);
        // Smart Cursor Color (light or dark variant); the scene applies it to a focused box only,
        // whatever shape a program asked for.
        let dark = self.dark();
        let smart = self.panes.get(&pane).and_then(|p| self.store.get(&p.profile)).filter(|p| p.bool_for("Smart Cursor Color", dark)).map(|_| {
            let t = |key: &str, default: f64| self.globals.number(key).unwrap_or(default).clamp(0.0, 1.0) as f32;
            (t("SmartCursorColorBgThreshold", 0.5), t("SmartCursorColorFgThreshold", 0.75))
        });
        // Cursor Boost (light or dark variant, 0 to 0.8), Cursor Shadow, and the outline.
        let profile = self.panes.get(&pane).and_then(|p| self.store.get(&p.profile));
        let boost = profile.map_or(0.0, |p| p.float_for("Cursor Boost", dark).clamp(0.0, 0.8) as f32);
        let shadow = profile.is_some_and(|p| p.bool("Cursor Shadow"));
        let outline = self.globals.bool("DrawOutlineAroundCursor");
        let offset = self.panes.get(&pane).and_then(|p| p.motion.slide.as_ref()).and_then(|s| s.offset(Instant::now(), self.slide_duration())).unwrap_or((0.0, 0.0));
        let smear = self.panes.get(&pane).and_then(|p| p.motion.smear.as_ref()).and_then(|s| s.polygon(Instant::now(), self.smear_duration()));
        CursorLook {
            offset,
            smear,
            boost,
            shadow,
            outline,
            hide_unfocused,
            alpha: smooth.unwrap_or(1.0),
            smart,
            geometry: Geometry::from_globals(&self.globals, self.scale(), self.base_cell()),
            ime: self.ime_drawn(pane).filter(|_| focused),
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
        let slide = self.tick_slides(now);
        let blink = self.tick_blink(now);
        self.cursor_due = [slide, blink].into_iter().flatten().min();
        self.cursor_due
    }

    /// `CursorSlideAnimationDuration` as a Duration (default 0.05 s).
    fn slide_duration(&self) -> Duration {
        Duration::try_from_secs_f64(self.globals.number("CursorSlideAnimationDuration").unwrap_or(0.05)).unwrap_or(Duration::ZERO)
    }

    /// `CursorSmearAnimationDuration` as a Duration (default 0.1 s).
    fn smear_duration(&self) -> Duration {
        Duration::try_from_secs_f64(self.globals.number("CursorSmearAnimationDuration").unwrap_or(0.1)).unwrap_or(Duration::ZERO)
    }

    /// Keeps frames coming while a slide or smear runs (one clock for both): a frame each
    /// `FAST_GAP`, and a last one at rest when it ends. Returns the next frame's time, None
    /// when nothing runs.
    fn tick_slides(&mut self, now: Instant) -> Option<Instant> {
        let duration = self.slide_duration();
        let smear_duration = self.smear_duration();
        let ids: Vec<PaneId> = self.tabs.get(self.active).map(|t| self.visible(t).into_iter().map(|(id, _)| id).collect()).unwrap_or_default();
        let mut running = false;
        let mut ended = false;
        for id in ids {
            let Some(m) = self.panes.get_mut(&id).map(|p| &mut p.motion) else { continue };
            if let Some(s) = m.slide.as_ref() {
                if s.offset(now, duration).is_some() {
                    running = true;
                } else {
                    m.slide = None;
                    ended = true;
                }
            }
            if let Some(s) = m.smear.as_ref() {
                if s.polygon(now, smear_duration).is_some() {
                    running = true;
                } else {
                    m.smear = None;
                    ended = true;
                }
            }
        }
        if ended {
            self.redraw();
        }
        if !running {
            self.anim_frame = None;
            return None;
        }
        let due = self.anim_frame.unwrap_or(now);
        if now >= due {
            self.redraw();
            self.anim_frame = Some(now + crate::cadence::FAST_GAP);
        }
        self.anim_frame
    }

    /// The blink clocks: the smooth blink's, else the on/off blink's.
    fn tick_blink(&mut self, now: Instant) -> Option<Instant> {
        if let Some(due) = self.tick_smooth_blink(now) {
            return due;
        }
        if self.blink_clock_needed() {
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
        }
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
        let panes: Vec<(PaneId, Rect)> = self.visible(tab).into_iter().map(|(id, rect)| (id, self.inner(rect))).collect();
        self.ime_sync();
        let now = Instant::now();
        let shown: Vec<PaneId> = panes.iter().map(|(id, _)| *id).collect();
        for (id, inner) in panes {
            self.note_cursor_moves(id, inner, now);
        }
        // A pane that is not on screen (another tab or window) forgets its place, so showing it
        // again never animates from a stale rect.
        for (id, p) in self.panes.iter_mut() {
            if !shown.contains(id) {
                p.motion.forget();
            }
        }
    }

    /// Notes `pane`'s cursor for a frame. iTerm2 compares the screen-relative grid point
    /// (`PTYTextView.m:2098`, `iTermTextDrawingHelper.m:3896`), so scrolling output under a
    /// cursor on the last row is no move; a hidden cursor leaves the stored cell alone. Then it
    /// starts a slide for a move of the pixel rect that qualifies (`PTYTextView.m:1590-1644`).
    fn note_cursor_moves(&mut self, pane: PaneId, inner: Rect, now: Instant) {
        let (cell, base, scale) = (self.cell(), self.base_cell(), self.scale());
        let focused = self.focused && self.tabs.get(self.active).is_some_and(|t| t.focused == pane);
        let geometry = Geometry::from_globals(&self.globals, scale, base);
        let duration = self.slide_duration();
        let max_cells = self.globals.int("CursorSlideAnimationMaxCells");
        let min_distance = self.globals.number("CursorAnimationMinDistance").unwrap_or(150.0) as f32 * scale;
        let slide_on = self.panes.get(&pane).and_then(|p| self.store.get(&p.profile)).is_some_and(|p| p.bool("Cursor Smooth Slide"));
        let (animate, interactive_only) = self.panes.get(&pane).and_then(|p| self.store.get(&p.profile)).map_or((false, false), |p| (p.bool("Animate Movement"), p.bool("Animate Movement Only in Interactive Apps")));
        let hide_unfocused = self.panes.get(&pane).and_then(|p| self.store.get(&p.profile)).is_some_and(|p| p.bool("Cursor Hidden When Unfocused"));
        // A cursor that just moved is solid (the blink restarts), so only a hidden one is not drawn.
        let drawn = !(hide_unfocused && !focused);
        let Some(p) = self.panes.get_mut(&pane) else { return };
        let key_cursor = focused && p.at_password_prompt();
        let alt = p.screen.alt_screen();
        let content = p.screen.term().renderable_content();
        let shape = content.cursor.shape;
        let scrolled = content.display_offset;
        let screen_cell = scene::cursor_cell(&p.screen);
        let point = p.screen.term().grid().cursor.point;
        let hidden_cell = screen_cell.map(|_| (i64::from(point.line.0), point.column.0));
        p.motion.note(hidden_cell, now);
        p.motion.metrics_changed(Metrics { cell, base, scale, pane: inner, scrolled, alt });
        let Some((row, col, wide)) = screen_cell else {
            p.motion.last_rect = None;
            return;
        };
        let rect = crate::cursor_look::cursor_rect(&geometry, shape, (inner.x, inner.y), cell, row, col, wide);
        let solid_line = matches!(shape, alacritty_terminal::vte::ansi::CursorShape::Underline | alacritty_terminal::vte::ansi::CursorShape::Beam);
        let slide_ok = slide_on && solid_line && drawn && !key_cursor;
        // iTerm2 animates only a solid rectangle: a bar, an underline or a focused box (the
        // blink phase does not matter); a hollow box forgets where it was
        // (`PTYTextView.m:1600`, `iTermTextDrawingHelper.m:3804`).
        let solid = drawn && !key_cursor && (solid_line || focused);
        let smear_ok = solid && animate && (!interactive_only || alt);
        let rule = cursor_anim::MoveRule { slide_ok, max_cells, cell_w: cell.0, smear_ok, min_distance };
        match p.motion.classify(rect, inner, rule) {
            Some(cursor_anim::Started::Slide(from, to)) => p.motion.begin_slide(from, to, now, duration),
            Some(cursor_anim::Started::Smear(from, to)) => p.motion.begin_smear(from, to, now),
            None => {}
        }
        if !solid {
            p.motion.last_rect = None;
        }
    }
}
