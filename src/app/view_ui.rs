//! View > Show Timestamps, Show Cursor Guide, and Find Cursor.

use std::time::{Duration, Instant};

use super::*;
use crate::scrollbar;
use crate::stamps;

/// Find Cursor stays at least this long, and while Win is held.
const FIND_CURSOR_HOLD: Duration = Duration::from_secs(1);
/// iTerm2's clear circle around the cursor, in pixels at 100%.
const FIND_CURSOR_RADIUS: f32 = 30.0;

impl App {
    /// The View commands here. Returns false for other selectors.
    pub(super) fn run_view_command(&mut self, selector: &str) -> bool {
        let Some(id) = self.focused_id() else { return false };
        match selector {
            "toggleShowTimestamps:" => {
                let on = !self.stamps_on(id);
                self.show_stamps.insert(id, on);
            }
            "toggleCursorGuide:" => {
                let on = !self.guide_on(id);
                self.show_guide.insert(id, on);
            }
            "findCursor:" => self.find_cursor_until = Some(Instant::now() + FIND_CURSOR_HOLD),
            _ => return false,
        }
        self.sync_checks();
        self.redraw();
        true
    }

    pub(super) fn profile_of(&self, id: PaneId) -> Option<&Profile> {
        self.panes.get(&id).and_then(|p| self.store.get(&p.profile))
    }

    /// Timestamps show in a pane: its own choice, else its profile's Timestamps Visible.
    pub(super) fn stamps_on(&self, id: PaneId) -> bool {
        self.show_stamps.get(&id).copied().unwrap_or_else(|| self.profile_of(id).is_some_and(|p| p.bool("Timestamps Visible")))
    }

    /// The pane's Adjacent-timestamp mode while its timestamps show (`profile` is its profile when
    /// the pane is not in `panes` yet).
    fn stamp_mode_with(&self, id: PaneId, profile: Option<&Profile>) -> Option<stamps::Mode> {
        let on = self.show_stamps.get(&id).copied().unwrap_or_else(|| profile.is_some_and(|p| p.bool("Timestamps Visible")));
        on.then(|| stamps::Mode::from_style(profile.map_or(1, |p| p.int("Timestamps Style"))))
    }

    /// The width a pane's grid gives up for Adjacent timestamps, in pixels (0 for Over, Hover,
    /// or timestamps off).
    pub(super) fn stamp_room(&self, id: PaneId) -> f32 {
        self.stamp_mode_with(id, self.profile_of(id)).map_or(0.0, |m| stamps::room(m, self.scale()))
    }

    /// The width every pane's grid gives up to a legacy scrollbar (`HideScrollbar` and the
    /// overlay style give none): the part of a pane's room the window itself must plan for.
    pub(super) fn scroll_room(&self) -> f32 {
        scrollbar::room(self.scroll_style, self.globals.bool("HideScrollbar"), self.scale())
    }

    /// What pane `id` gives up at its right edge: the scrollbar's strip and the timestamp column.
    pub(super) fn pane_room(&self, id: PaneId) -> f32 {
        self.pane_room_with(id, self.profile_of(id))
    }

    /// `pane_room` for a pane that is not in `panes` yet (it is being started with `profile`).
    pub(super) fn pane_room_for(&self, id: PaneId, profile: &Profile) -> f32 {
        self.pane_room_with(id, Some(profile))
    }

    fn pane_room_with(&self, id: PaneId, profile: Option<&Profile>) -> f32 {
        margins::pane_room(self.scroll_style, self.globals.bool("HideScrollbar"), self.stamp_mode_with(id, profile), self.scale())
    }

    /// The pane's rectangle less its scrollbar and timestamp column: the margin painter's outer.
    pub(super) fn text_outer(&self, id: PaneId, rect: Rect) -> Rect {
        margins::text_outer(rect, self.pane_room(id))
    }

    /// The pane's text area: THE rectangle the grid, the pointer and everything drawn on the
    /// text share (B5), so tmux, resizes, snaps and hit tests agree with what is drawn.
    pub(super) fn text_rect(&self, id: PaneId, rect: Rect) -> Rect {
        margins::text_rect(rect, self.margins(), self.pane_room(id))
    }

    /// The Adjacent column: between the text and the scrollbar, the text's height.
    pub(super) fn stamp_column(&self, id: PaneId, rect: Rect) -> Option<Rect> {
        let room = self.stamp_room(id);
        if room <= 0.0 {
            return None;
        }
        let text = self.text_rect(id, rect);
        Some(Rect { x: rect.x + rect.w - self.scroll_room() - room, y: text.y, w: room, h: text.h })
    }

    /// The pane's scrollbar under (`x`, `y`): its pane, the pane's rectangle and the bar's
    /// geometry. None when the bar is hidden (`HideScrollbar`), when an overlay bar has no
    /// history to show, or when the point is on no bar.
    pub(super) fn scrollbar_at(&self, x: f32, y: f32) -> Option<(PaneId, Rect, scrollbar::Geometry)> {
        let (id, rect) = self.pane_at(x, y)?;
        let g = self.scrollbar_geometry(id, rect)?;
        let t = g.track;
        (x >= t.x && x < t.x + t.w && y >= t.y && y < t.y + t.h).then_some((id, rect, g))
    }

    /// The pane's scrollbar geometry now: None while hidden, or an overlay bar with nothing to
    /// scroll.
    fn scrollbar_geometry(&self, id: PaneId, rect: Rect) -> Option<scrollbar::Geometry> {
        if self.globals.bool("HideScrollbar") {
            return None;
        }
        let g = self.panes.get(&id)?.screen.term().grid();
        scrollbar::geometry(rect, self.scroll_style, self.scale(), g.history_size(), g.screen_lines(), g.display_offset())
    }

    /// Whether the overlay style shows its thumb: under the pointer, while dragged, or for a
    /// second after the view moved. The legacy style always shows.
    fn scrollbar_shown(&self, id: PaneId) -> bool {
        self.scroll_style == scrollbar::Style::Legacy
            || self.scroll_hover == Some(id)
            || matches!(self.drag, Drag::Scrollbar { pane, .. } if pane == id)
            || self.scroll_seen.get(&id).is_some_and(|(_, until)| Instant::now() < *until)
    }

    /// Draws pane `id`'s scrollbar at the right edge of `rect`, in its own layer (popups drawn
    /// later cut it).
    pub(super) fn draw_scrollbar(&self, frame: &mut Frame, id: PaneId, rect: Rect, _cell: (f32, f32)) {
        let Some(g) = self.scrollbar_geometry(id, rect) else { return };
        if !self.scrollbar_shown(id) {
            return;
        }
        let hover = self.scroll_hover == Some(id) || matches!(self.drag, Drag::Scrollbar { pane, .. } if pane == id);
        scene::scrollbar(frame, &g, self.scroll_style, self.dark(), hover);
    }

    /// A press on a scrollbar: the thumb starts a drag, the track pages a screen; with nothing
    /// to scroll the press is swallowed (`scrollbar::press`). Any button is the bar's, never the
    /// program's or a selection's.
    pub(super) fn scroll_press(&mut self, button: MouseButton, x: f32, y: f32) -> bool {
        let Some((id, _, g)) = self.scrollbar_at(x, y) else { return false };
        let history = self.panes.get(&id).map_or(0, |p| p.screen.term().grid().history_size());
        if button != MouseButton::Left {
            return true;
        }
        self.focus_stamped(id);
        match scrollbar::press(&g, x, y, history) {
            scrollbar::Press::Pass | scrollbar::Press::Swallow => {}
            scrollbar::Press::Drag(grab) => self.drag = Drag::Scrollbar { pane: id, grab },
            scrollbar::Press::Page(up) => {
                if let Some(p) = self.panes.get_mut(&id) {
                    p.scroll(if up > 0 { Scroll::PageUp } else { Scroll::PageDown });
                }
            }
        }
        self.redraw();
        true
    }

    /// The thumb of pane `pane` follows the pointer to `y`.
    pub(super) fn scroll_drag_to(&mut self, pane: PaneId, grab: f32, y: f32) {
        let Some(rect) = self.pane_rect(pane) else { return };
        let Some(g) = self.scrollbar_geometry(pane, rect) else { return };
        let Some(p) = self.panes.get_mut(&pane) else { return };
        let (history, offset) = (p.screen.term().grid().history_size(), p.screen.term().grid().display_offset());
        let target = scrollbar::offset_for_drag(&g, grab, y, history);
        if target != offset {
            p.scroll(Scroll::Delta(target as i32 - offset as i32));
            self.redraw();
        }
    }

    /// The wheel over a scrollbar scrolls its pane by the usual rule (acceleration included).
    /// Returns whether it was over one.
    pub(super) fn scroll_wheel(&mut self, dy: f32, precise: bool) -> bool {
        let (x, y) = self.cursor;
        let Some((id, _, _)) = self.scrollbar_at(x, y) else { return false };
        let prefs = crate::wheel::Prefs::from_globals(&self.globals);
        let ch = self.cell().1 as f64 / self.scale() as f64;
        if let Some(p) = self.panes.get_mut(&id) {
            let lines = p.wheel.scroll.take(dy as f64, precise, ch, &prefs);
            if lines != 0.0 {
                p.scroll(Scroll::Delta(lines as i32));
            }
        }
        self.redraw();
        true
    }

    /// Notices a pane's view moving: an overlay bar then shows for a second. Redraws when the
    /// time is up, and says when to look next.
    pub(super) fn tick_scrollbars(&mut self) -> Option<Instant> {
        const SHOWN: Duration = Duration::from_secs(1);
        let now = Instant::now();
        let seen: Vec<(PaneId, usize)> = self.tabs.get(self.active).map(|t| t.layout.panes()).unwrap_or_default().into_iter().filter_map(|id| Some((id, self.panes.get(&id)?.screen.term().grid().display_offset()))).collect();
        self.scroll_seen.retain(|id, _| self.panes.contains_key(id));
        let mut moved = false;
        for (id, offset) in seen {
            match self.scroll_seen.get_mut(&id) {
                Some((last, until)) if *last != offset => {
                    (*last, *until) = (offset, now + SHOWN);
                    moved = true;
                }
                Some(_) => {}
                // First sight: the position, not a move.
                None => {
                    self.scroll_seen.insert(id, (offset, now));
                }
            }
        }
        let due = self.scroll_seen.values().map(|(_, until)| *until).filter(|until| *until > now).min();
        let expired = self.scroll_seen.values().any(|(_, until)| *until <= now && now.duration_since(*until) < Duration::from_millis(250));
        if moved || (expired && self.scroll_style == scrollbar::Style::Overlay) {
            self.redraw();
        }
        due
    }

    /// Windows' "Always show scrollbars" now: the style the panes size for. A change gives or
    /// takes the strip in every window (their ptys and tmux's report follow).
    pub(super) fn refresh_scroll_style(&mut self) {
        let style = scrollbar::current_style();
        if style != self.scroll_style {
            self.scroll_style = style;
            self.relayout_changed_windows();
        }
    }

    /// The cursor guide shows in a pane: its own choice, else its profile's Use Cursor Guide.
    pub(super) fn guide_on(&self, id: PaneId) -> bool {
        self.show_guide.get(&id).copied().unwrap_or_else(|| self.profile_of(id).is_some_and(|p| p.bool("Use Cursor Guide")))
    }

    /// Draws a pane's cursor guide and timestamps.
    pub(super) fn draw_view_extras(&self, frame: &mut Frame, id: PaneId, rect: Rect, cell: (f32, f32)) {
        let Some(p) = self.panes.get(&id) else { return };
        let inner = self.text_rect(id, rect);
        let grid = p.screen.term().grid();
        let (offset, lines) = (grid.display_offset() as i32, grid.screen_lines() as i32);
        if self.guide_on(id) {
            let row = grid.cursor.point.line.0 + offset;
            if (0..lines).contains(&row) {
                let dark = self.dark();
                let profile = self.profile_of(id);
                let color = profile.map_or(crate::palette::rgb(0xa6, 0xe8, 0xff), |pr| pr.color_for("Cursor Guide Color", dark));
                let alpha = profile
                    .and_then(|pr| pr.get("Cursor Guide Color"))
                    .and_then(|v| v.get("a").and_then(|a| a.as_float()))
                    .unwrap_or(0.25) as f32;
                scene::cursor_guide(frame, inner, cell, row as usize, rgba(color, alpha));
            }
        }
        if self.stamps_on(id) {
            // Hover (2) shows them only while the pointer is over the right edge of the pane.
            let mode = stamps::Mode::from_style(self.profile_of(id).map_or(1, |pr| pr.int("Timestamps Style")));
            let (x, y) = self.cursor;
            let near = x >= inner.x + inner.w - 12.0 * cell.0 && x <= inner.x + inner.w && y >= inner.y && y <= inner.y + inner.h;
            if mode != stamps::Mode::Hover || near {
                let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
                let labels: Vec<(usize, String)> = (0..lines)
                    .filter_map(|row| {
                        let t = p.screen.row_time(row - offset) as u64;
                        (t > 0).then(|| (row as usize, stamps::label(now.saturating_sub(t), &stamps::local(t))))
                    })
                    .collect();
                scene::timestamps(frame, &labels, inner, cell, self.stamp_column(id, rect).filter(|_| mode == stamps::Mode::Adjacent), (p.screen.palette.fg, p.screen.palette.bg));
            }
        }
    }

    /// The pointer is within 12 columns of the right edge of a pane that shows Hover
    /// timestamps.
    pub(super) fn near_stamp_edge(&self) -> bool {
        let (x, y) = self.cursor;
        let cell = self.cell();
        let Some((id, rect)) = self.pane_at(x, y) else { return false };
        if !self.stamps_on(id) || self.profile_of(id).map_or(1, |p| p.int("Timestamps Style")) != 2 {
            return false;
        }
        let inner = self.text_rect(id, rect);
        x >= inner.x + inner.w - 12.0 * cell.0
    }

    /// Shows a short message at the bottom of the window for 3 seconds.
    pub(super) fn show_toast(&mut self, text: &str) {
        self.toast = Some((text.to_string(), Instant::now() + Duration::from_secs(3)));
        self.redraw();
    }

    /// When the toast goes away; redraws once it has.
    pub(super) fn toast_due(&mut self) -> Option<Instant> {
        let until = self.toast.as_ref()?.1;
        if Instant::now() >= until {
            self.toast = None;
            self.redraw();
            return None;
        }
        Some(until)
    }

    /// Draws synchronized updates whose time ran out (a program that never ended one), and
    /// says when the next one is due.
    pub(super) fn flush_due_syncs(&mut self) -> Option<Instant> {
        let now = Instant::now();
        let mut flushed = false;
        for p in self.panes.values_mut() {
            flushed |= p.flush_due_sync(now);
        }
        if flushed {
            self.redraw();
        }
        self.panes.values().filter_map(|p| p.sync_deadline()).min()
    }

    /// Animated images on screen: redraws when a frame is due, and says when the next one is.
    pub(super) fn next_animation_frame(&mut self) -> Option<Instant> {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
        let tab = self.tabs.get(self.active)?;
        let next = tab
            .layout
            .panes()
            .iter()
            .filter_map(|id| self.panes.get(id))
            .flat_map(|p| p.screen.images.by_id.values())
            .filter_map(|img| scene::ms_to_next_frame(&img.frames, now))
            .min()?;
        if self.anim_due.is_some_and(|due| Instant::now() >= due) {
            self.redraw();
        }
        // Frames shorter than 20 ms (a GIF quirk) draw at 50 frames a second.
        let due = Instant::now() + Duration::from_millis(next.max(20));
        self.anim_due = Some(due);
        Some(due)
    }

    /// Draws Find Cursor over the whole window while it is up.
    pub(super) fn draw_find_cursor(&self, frame: &mut Frame, area: Rect, cell: (f32, f32)) {
        if self.find_cursor_until.is_none() {
            return;
        }
        let Some(tab) = self.tabs.get(self.active) else { return };
        let Some((_, rect)) = self.visible(tab).into_iter().find(|(id, _)| *id == tab.focused) else { return };
        let Some(p) = self.panes.get(&tab.focused) else { return };
        let grid = p.screen.term().grid();
        let row = grid.cursor.point.line.0 + grid.display_offset() as i32;
        let inner = self.text_rect(tab.focused, rect);
        let center = (inner.x + (grid.cursor.point.column.0 as f32 + 0.5) * cell.0, inner.y + (row as f32 + 0.5) * cell.1);
        scene::find_cursor(frame, area, center, FIND_CURSOR_RADIUS * self.scale());
    }

    /// Ends Find Cursor once its time is up and Win is no longer held. Returns when to
    /// look again.
    pub(super) fn tick_find_cursor(&mut self) -> Option<Instant> {
        let until = self.find_cursor_until?;
        if Instant::now() < until {
            return Some(until);
        }
        if self.mods.cmd {
            // Held: look again soon, and end when Win comes up.
            return Some(Instant::now() + Duration::from_millis(100));
        }
        self.find_cursor_until = None;
        self.redraw();
        None
    }
}
