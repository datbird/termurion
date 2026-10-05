//! The offscreen command line (13p): while a long command's output has scrolled its line off
//! the top, a band at the pane's top shows the line, and a click on it scrolls back to it
//! (`PTYTextView.m:2150-2158`, `:7885-7896`). The rules and the geometry are in `offscreen`.

use super::*;
use crate::offscreen::{self, Facts};

/// The pane and line of the band under (`x`, `y`), if any. `covered` is whether something of
/// Tandem's (an info bar, the find bar, an indicator) is drawn over the point: it keeps its press.
fn hit(hits: &[(PaneId, Rect, i32)], covered: &dyn Fn(f32, f32) -> bool, x: f32, y: f32) -> Option<(PaneId, i32)> {
    if covered(x, y) {
        return None;
    }
    hits.iter().find(|(_, r, _)| r.contains(x, y)).map(|(id, _, line)| (*id, *line))
}

/// iTerm2 reveals the line on a single click, never on the click that activated the window
/// (`TV:7873`, `clickCount == 1`).
pub(super) fn click_allowed(first_mouse: bool, clicks: u8) -> bool {
    !first_mouse && clicks == 1
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

    /// Draws pane `id`'s band when the rules allow it, around `holes` (Tandem's own surfaces
    /// drawn over the pane's top). Answers the band's visible pieces and the line it shows,
    /// which the frame keeps as this window's hits.
    pub(super) fn draw_offscreen(&self, frame: &mut Frame, id: PaneId, rect: Rect, cell: (f32, f32), holes: &[Rect]) -> Vec<(Rect, i32)> {
        self.offscreen_band_for(frame, id, rect, cell, holes).unwrap_or_default()
    }

    fn offscreen_band_for(&self, frame: &mut Frame, id: PaneId, rect: Rect, cell: (f32, f32), holes: &[Rect]) -> Option<Vec<(Rect, i32)>> {
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
            // The displayed grid's lines (S12: 13o's alternate screen has its own history).
            top_line: -(offset as i32),
            lines_above: grid.history_size().saturating_sub(offset),
            history_top: -(grid.history_size() as i32),
            prompts: offscreen::displayed_prompts(p.screen.alt_screen(), prompts),
        };
        let line = offscreen::command_line(&facts)?;
        let scale = self.scale();
        let outer = self.text_outer(id, rect);
        let band = offscreen::band_rect(outer, cell, scale);
        let text = self.text_rect(id, rect);
        let face = self.pane_faces.face(id).unwrap_or(crate::render::DEFAULT_FACE);
        let colors = offscreen::band_colors(p.screen.palette.bg, p.screen.palette.fg);
        scene::offscreen_band(frame, &p.screen, band, (text.x, band.y + offscreen::PADDING * scale), cell, line, colors, face, holes);
        Some(offscreen::band_pieces(band, holes).into_iter().map(|r| (r, line)).collect())
    }

    /// A left press inside a band reveals its command at the top of the pane and is consumed.
    pub(super) fn offscreen_click(&mut self, x: f32, y: f32) -> bool {
        let covered = |x: f32, y: f32| self.tandem_ui_at(x, y);
        let Some((id, line)) = hit(&self.offscreen_hits, &covered, x, y) else { return false };
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
        let free = |_: f32, _: f32| false;
        assert_eq!(hit(&hits, &free, 50.0, 10.0), Some((1, -60)));
        assert_eq!(hit(&hits, &free, 250.0, 31.0), Some((2, -9)));
        assert_eq!(hit(&hits, &free, 150.0, 10.0), None);
        assert_eq!(hit(&hits, &free, 50.0, 40.0), None);
        assert_eq!(hit(&[], &free, 1.0, 1.0), None);
    }

    /// Task 6 review 1: a point under an info bar, the find bar or an indicator belongs to
    /// that surface, not to the band drawn beneath it.
    #[test]
    fn band_hit_area_excludes_tandem_surfaces() {
        let hits = [(1, Rect { x: 0.0, y: 0.0, w: 100.0, h: 32.0 }, -60)];
        let info_bar = |x: f32, y: f32| Rect { x: 0.0, y: 0.0, w: 100.0, h: 12.0 }.contains(x, y);
        assert_eq!(hit(&hits, &info_bar, 50.0, 5.0), None, "the info bar's strip");
        assert_eq!(hit(&hits, &info_bar, 50.0, 20.0), Some((1, -60)), "below it the band still answers");
    }

    /// Task 6 review 1 (minor): a single click that did not activate the window.
    #[test]
    fn band_click_is_a_single_non_first_click() {
        assert!(click_allowed(false, 1));
        assert!(!click_allowed(true, 1), "the click that activated the window");
        assert!(!click_allowed(false, 2), "the second press of a double click");
        assert!(!click_allowed(false, 3));
    }

    /// Task 6 review 1: the band's press runs after every Tandem surface that can sit over it
    /// (indicators, panels, inline buttons, info bar, porthole, block pills, the divider, title
    /// bars, find bar, the one overlay list) and before the scrollbar and the pane.
    #[test]
    fn band_press_runs_after_the_surfaces_over_it() {
        let m = include_str!("mod.rs");
        let press = m.split("fn mouse_press(").nth(1).unwrap();
        let at = |needle: &str| press.find(needle).unwrap_or_else(|| panic!("{needle} missing"));
        let band = at("self.offscreen_click(x, y)");
        for before in ["self.chat_indicator_press(x, y)", "self.clip_press(x, y, event_loop)", "self.inline_press(button, x, y, event_loop)", "self.info_bar_press(x, y, event_loop)", "self.porthole_press(x, y)", "self.block_press(x, y)", "self.divider_at(x, y)", "self.find_press(x, y)", "self.tandem_ui_at(x, y)"] {
            assert!(at(before) < band, "{before} must come first");
        }
        assert!(band < at("self.bar_press_only_activates(first, x, y)"), "the band is above the scrollbar");
        assert!(press.contains("click_allowed(first, self.next_os_clicks(0))"), "gated on a single, non-first click");
    }

    /// The band's wiring: per window in the swap and both slot literals, drawn after the pane
    /// and before the extras that cut it, in the pane's own cell and face.
    #[test]
    fn offscreen_band_is_wired_per_window_and_per_pane() {
        let slot = include_str!("windows_ui.rs");
        assert!(slot.contains("swap(&mut self.offscreen_hits, &mut s.offscreen_hits)"));
        assert_eq!(slot.matches("offscreen_hits: Vec::new()").count(), 2, "WinSlot::new and new_empty");
        let m = include_str!("mod.rs");
        // Task 6 review 2: drawn once every Tandem surface is, around their rectangles.
        let build = &m[m.find("fn build_frame(&mut self)").unwrap()..];
        let band = build.find("self.draw_offscreen(&mut frame, id, rect, cell, &holes)").unwrap();
        for before in ["self.draw_info_bar(&mut frame, id, rect, cell)", "self.draw_portholes(&mut frame, id, rect, cell)", "self.draw_coprocess_indicator(", "scene::find_bar(&mut frame", "self.draw_composer(&mut frame", "self.draw_completion(&mut frame", "mouse::Overlay::Toast", "mouse::Overlay::Banner"] {
            assert!(build.find(before).unwrap_or_else(|| panic!("{before} missing")) < band, "{before} is drawn before the band");
        }
        assert!(band < build.find("self.overlay_drawn = overlay_drawn;").unwrap());
        assert!(m.contains("let cell = self.pane_cell(id);"));
        let me = include_str!("offscreen_ui.rs").split("#[cfg(test)]").next().unwrap();
        assert!(me.contains("self.pane_faces.face(id)") && me.contains("self.text_outer(id, rect)") && !me.contains("marks_in_order"));
        assert!(!me.contains("frame.overlay"), "never text in the overlay");
        // S12: the geometry is the displayed grid's and the prompts are dropped off the main screen.
        assert!(me.contains("let grid = p.screen.term().grid();") && !me.contains("primary_grid"));
        assert!(me.contains("prompts: offscreen::displayed_prompts(p.screen.alt_screen(), prompts),"));
    }
}
