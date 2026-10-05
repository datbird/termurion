//! The offscreen command line (13p): while a long command's output has scrolled its line off
//! the top, a band at the pane's top shows the line, and a click on it scrolls back to it
//! (`PTYTextView.m:2150-2158`, `:7885-7896`). The rules and the geometry are in `offscreen`.

use super::*;
use crate::offscreen::{self, Facts};

/// The pane and line of the band under (`x`, `y`), if any.
fn hit(hits: &[(PaneId, Rect, i32)], x: f32, y: f32) -> Option<(PaneId, i32)> {
    hits.iter().find(|(_, r, _)| r.contains(x, y)).map(|(id, _, line)| (*id, *line))
}

impl App {
    /// Every visible pane's prompt facts for this frame, from the screen's cache (never the
    /// per-frame scan of the whole buffer, P11). Only panes whose profile shows the
    /// band are asked.
    pub(super) fn prepare_offscreen(&mut self) {
        self.offscreen_prepared.clear();
        let Some(tab) = self.tabs.get(self.active) else { return };
        let ids: Vec<PaneId> = self.visible(tab).into_iter().map(|(id, _)| id).collect();
        for id in ids {
            let on = self.profile_of(id).is_some_and(|p| p.bool("Show Offscreen Command line"));
            let Some(p) = self.panes.get_mut(&id) else { continue };
            // No band over Instant Replay or a Filter or Zoom view: those marks are not the session's.
            if on && p.replay.is_none() && p.live.is_none() {
                let facts = p.screen.offscreen_facts();
                self.offscreen_prepared.insert(id, facts);
            }
        }
    }

    /// Draws pane `id`'s band when the rules allow it. Answers its rectangle and the line it
    /// shows, which the frame keeps as this window's hits.
    pub(super) fn draw_offscreen(&self, frame: &mut Frame, id: PaneId, rect: Rect, cell: (f32, f32)) -> Option<(Rect, i32)> {
        let prompts = self.offscreen_prepared.get(&id)?;
        let p = self.panes.get(&id)?;
        let profile = self.store.get(&p.profile)?;
        let grid = p.screen.term().grid();
        let offset = grid.display_offset();
        let facts = Facts {
            rows: grid.screen_lines(),
            copy_mode: self.copy_modes.contains_key(&id),
            user_scrolled: offset > 0,
            cursor_row: grid.cursor.point.line.0.max(0) as usize,
            enabled: profile.bool("Show Offscreen Command line"),
            for_current: profile.bool("Show Offscreen Command line for Current Command"),
            alt_screen: p.screen.alt_screen(),
            top_line: -(offset as i32),
            lines_above: grid.history_size().saturating_sub(offset),
            history_top: -(grid.history_size() as i32),
            prompts: prompts.clone(),
        };
        let line = offscreen::command_line(&facts)?;
        let scale = self.scale();
        let outer = self.text_outer(id, rect);
        let band = offscreen::band_rect(outer, cell, scale);
        let text = self.text_rect(id, rect);
        let face = self.pane_faces.face(id).unwrap_or(crate::render::DEFAULT_FACE);
        let colors = offscreen::band_colors(p.screen.palette.bg, p.screen.palette.fg);
        scene::offscreen_band(frame, &p.screen, band, (text.x, band.y + offscreen::PADDING * scale), cell, line, colors, face);
        Some((band, line))
    }

    /// A left press inside a band reveals its command at the top of the pane and is consumed.
    pub(super) fn offscreen_click(&mut self, x: f32, y: f32) -> bool {
        let Some((id, line)) = hit(&self.offscreen_hits, x, y) else { return false };
        self.show_line(id, line, false);
        self.redraw();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offscreen_click_follows_iterm2() {
        let r = |x: f32, y: f32| Rect { x, y, w: 100.0, h: 32.0 };
        let hits = [(1, r(0.0, 0.0), -60), (2, r(200.0, 0.0), -9)];
        assert_eq!(hit(&hits, 50.0, 10.0), Some((1, -60)));
        assert_eq!(hit(&hits, 250.0, 31.0), Some((2, -9)));
        assert_eq!(hit(&hits, 150.0, 10.0), None);
        assert_eq!(hit(&hits, 50.0, 40.0), None);
        assert_eq!(hit(&[], 1.0, 1.0), None);
    }

    /// The band's wiring: per window in the swap and both slot literals, drawn after the pane
    /// and before the extras that cut it, in the pane's own cell and face.
    #[test]
    fn offscreen_band_is_wired_per_window_and_per_pane() {
        let slot = include_str!("windows_ui.rs");
        assert!(slot.contains("swap(&mut self.offscreen_hits, &mut s.offscreen_hits)"));
        assert_eq!(slot.matches("offscreen_hits: Vec::new()").count(), 2, "WinSlot::new and new_empty");
        let m = include_str!("mod.rs");
        let (band, porthole) = (m.find("self.draw_offscreen(&mut frame, id, rect, cell)").unwrap(), m.find("self.draw_portholes(&mut frame, id, rect, cell)").unwrap());
        assert!(band < porthole, "later popups cut the band");
        assert!(m.contains("let cell = self.pane_cell(id);"));
        let me = include_str!("offscreen_ui.rs").split("#[cfg(test)]").next().unwrap();
        assert!(me.contains("self.pane_faces.face(id)") && me.contains("self.text_outer(id, rect)") && !me.contains("marks_in_order"));
        assert!(!me.contains("frame.overlay"), "never text in the overlay");
    }
}
