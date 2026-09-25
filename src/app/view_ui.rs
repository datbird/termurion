//! View > Show Timestamps, Show Cursor Guide, and Find Cursor.

use std::time::{Duration, Instant};

use super::*;
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

    fn profile_of(&self, id: PaneId) -> Option<&Profile> {
        self.panes.get(&id).and_then(|p| self.store.get(&p.profile))
    }

    /// Timestamps show in a pane: its own choice, else its profile's Timestamps Visible.
    pub(super) fn stamps_on(&self, id: PaneId) -> bool {
        self.show_stamps.get(&id).copied().unwrap_or_else(|| self.profile_of(id).is_some_and(|p| p.bool("Timestamps Visible")))
    }

    /// The cursor guide shows in a pane: its own choice, else its profile's Use Cursor Guide.
    pub(super) fn guide_on(&self, id: PaneId) -> bool {
        self.show_guide.get(&id).copied().unwrap_or_else(|| self.profile_of(id).is_some_and(|p| p.bool("Use Cursor Guide")))
    }

    /// Draws a pane's cursor guide and timestamps.
    pub(super) fn draw_view_extras(&self, frame: &mut Frame, id: PaneId, inner: Rect, cell: (f32, f32)) {
        let Some(p) = self.panes.get(&id) else { return };
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
            let style = self.profile_of(id).map_or(1, |pr| pr.int("Timestamps Style"));
            let (x, y) = self.cursor;
            let near = x >= inner.x + inner.w - 12.0 * cell.0 && x <= inner.x + inner.w && y >= inner.y && y <= inner.y + inner.h;
            if style != 2 || near {
                let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
                let labels: Vec<(usize, String)> = (0..lines)
                    .filter_map(|row| {
                        let t = p.screen.row_time(row - offset) as u64;
                        (t > 0).then(|| (row as usize, stamps::label(now.saturating_sub(t), &stamps::local(t))))
                    })
                    .collect();
                scene::timestamps(frame, &labels, inner, cell);
            }
        }
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
        let inner = self.inner(rect);
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
