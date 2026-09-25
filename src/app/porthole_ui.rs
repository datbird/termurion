//! Portholes in the window: laying their documents out with the renderer's fonts (cached
//! per width, font, and look), drawing them over their rows, and making them.

use std::cell::RefCell;
use std::collections::HashSet;

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::Line;

use super::*;
use crate::porthole::detect::Language;
use crate::porthole::render::{self, Face, Layout};
use crate::porthole::store::{rows_needed, Porthole, Source};
use crate::render::Renderer;
use crate::scene::PORTHOLE_MARGIN;

/// A porthole's layout and what it was made for, and its sideways scroll (Wide).
pub(super) struct PView {
    pub layout: Layout,
    key: (u32, u32, bool, String, bool),
    pub scroll_x: f32,
}

/// A drawn porthole: its pane and id, its controls, and its frame.
pub(super) type PortholeHit = (PaneId, u32, scene::PortholeControls, Rect);

/// The document laid out with the renderer's fonts.
fn lay(renderer: &mut Renderer, p: &Porthole, palette: &crate::palette::Palette, dark: bool, width: f32, cell_h: f32) -> Layout {
    let lang = p.detected.language.clone().unwrap_or(Language::Plain);
    let doc = render::render(&p.text, &lang, palette, dark);
    let r = RefCell::new(renderer);
    let measure = |t: &str, f: Face, s: f32| r.borrow_mut().text_width(t, f == Face::Prose, s);
    render::layout(&doc, width, cell_h, &measure, p.wide)
}


/// The width a pane's portholes lay out at.
fn text_width(cols: usize, cell_w: f32) -> f32 {
    (cols as f32 * cell_w - 2.0 * PORTHOLE_MARGIN).max(cell_w)
}

impl App {
    /// Replaces the pane's lines `first..=last` (grid lines of its own screen) with a
    /// porthole. None when nothing was made (the alternate screen, lines gone).
    pub(super) fn make_porthole(&mut self, pane: PaneId, first: i32, last: i32, source: Source) -> Option<u32> {
        let cell = self.cell();
        let gpu = self.gpu.as_mut()?;
        let p = self.panes.get_mut(&pane)?;
        let screen = p.own_mut();
        let width = text_width(screen.term().columns(), cell.0);
        let palette = screen.palette.clone();
        let dark = screen.is_dark();
        let renderer = RefCell::new(&mut gpu.renderer);
        let rows_for = |ph: &Porthole| rows_needed(lay(&mut renderer.borrow_mut(), ph, &palette, dark, width, cell.1).height, cell.1);
        let id = screen.render_range(first, last, source, &rows_for);
        if let Some(ph) = id.and_then(|i| screen.portholes.get_mut(&i)) {
            ph.sized = true;
        }
        self.redraw();
        id
    }

    /// Edit > Replace Selection > Render Selection Natively: the selection's whole rows.
    pub(super) fn render_selection(&mut self) {
        let beep = || unsafe {
            windows_sys::Win32::System::Diagnostics::Debug::MessageBeep(windows_sys::Win32::UI::WindowsAndMessaging::MB_OK);
        };
        let dragging = matches!(self.drag, Drag::Select { .. });
        let wide = self.globals.bool("defaultWideMode");
        let Some(id) = self.focused_id() else { return beep() };
        let Some(p) = self.panes.get_mut(&id) else { return beep() };
        // A Filter or Zoom view is a copy: nothing there to replace.
        if p.live.is_some() {
            return beep();
        }
        let term = p.screen.term();
        let Some(r) = term.selection.as_ref().and_then(|s| s.to_range(term)) else { return beep() };
        let (first, last) = crate::porthole::selection_rows((r.start.line.0, r.start.column.0), (r.end.line.0, r.end.column.0));
        let g = term.grid();
        let overlaps = (first..=last).any(|l| g[Line(l)].porthole() != 0);
        if !crate::porthole::selection_can_render(1, dragging, overlaps) {
            return beep();
        }
        p.screen.term_mut().selection = None;
        if self.make_porthole(id, first, last, Source { hint: None, filename: None, wide }).is_none() {
            beep();
        }
    }

    fn porthole_key(&self, s: &crate::screen::Screen, p: &Porthole) -> (u32, u32, bool, String, bool) {
        let cell = self.cell();
        let lang = p.detected.language.as_ref().map_or("Plain text".to_string(), |l| l.name());
        (text_width(s.term().columns(), cell.0).to_bits(), cell.1.to_bits(), s.is_dark(), lang, p.wide)
    }

    /// Lays out the portholes in view whose width, font, or look changed.
    pub(super) fn prepare_portholes(&mut self) {
        let cell = self.cell();
        let Some(tab) = self.tabs.get(self.active) else { return };
        let ids: Vec<PaneId> = self.visible(tab).into_iter().map(|(id, _)| id).collect();
        let mut keep: HashSet<(PaneId, u32)> = HashSet::new();
        let mut resize: Vec<(PaneId, u32, u32)> = Vec::new();
        for pane in ids {
            let Some(p) = self.panes.get(&pane) else { continue };
            let s = &p.screen;
            for (pid, ph) in &s.portholes {
                keep.insert((pane, *pid));
                let key = self.porthole_key(s, ph);
                if self.porthole_views.get(&(pane, *pid)).is_some_and(|v| v.key == key) {
                    continue;
                }
                let Some(gpu) = self.gpu.as_mut() else { return };
                let layout = lay(&mut gpu.renderer, ph, &s.palette, s.is_dark(), text_width(s.term().columns(), cell.0), cell.1);
                // The terminal estimated this one's rows; the window's fonts decide once.
                if !ph.sized {
                    resize.push((pane, *pid, rows_needed(layout.height, cell.1)));
                }
                let scroll_x = self.porthole_views.get(&(pane, *pid)).map_or(0.0, |v| v.scroll_x);
                self.porthole_views.insert((pane, *pid), PView { layout, key, scroll_x });
            }
        }
        for (pane, pid, rows) in resize {
            if let Some(s) = self.panes.get_mut(&pane).map(|p| &mut p.screen) {
                s.set_porthole_rows(pid, rows);
                if let Some(ph) = s.portholes.get_mut(&pid) {
                    ph.sized = true;
                }
            }
        }
        self.porthole_views.retain(|k, _| keep.contains(k) || self.panes.get(&k.0).is_some_and(|p| p.screen.portholes.contains_key(&k.1)));
    }

    /// Draws the portholes on a pane's rows in view. The drawn ones, for clicks.
    pub(super) fn draw_portholes(&self, frame: &mut Frame, id: PaneId, rect: Rect, cell: (f32, f32)) -> Vec<PortholeHit> {
        let Some(p) = self.panes.get(&id) else { return Vec::new() };
        let s = &p.screen;
        if s.term().mode().contains(alacritty_terminal::term::TermMode::ALT_SCREEN) || s.portholes.is_empty() {
            return Vec::new();
        }
        let g = s.term().grid();
        let off = g.display_offset() as i32;
        let (top_line, lines) = (-(g.history_size() as i32), g.screen_lines() as i32);
        let inner = self.inner(rect);
        let mut seen = HashSet::new();
        let mut hits = Vec::new();
        for r in 0..lines {
            let line = r - off;
            let pid = g[Line(line)].porthole();
            if pid == 0 || !seen.insert(pid) {
                continue;
            }
            let (Some(ph), Some(view)) = (s.portholes.get(&pid), self.porthole_views.get(&(id, pid))) else { continue };
            let mut top = line;
            while top > top_line && g[Line(top - 1)].porthole() == pid {
                top -= 1;
            }
            let mut bottom = line;
            while bottom < lines - 1 && g[Line(bottom + 1)].porthole() == pid {
                bottom += 1;
            }
            let prect = Rect { x: inner.x, y: inner.y + (top + off) as f32 * cell.1 + 4.0, w: inner.w, h: (bottom - top + 1) as f32 * cell.1 - 8.0 };
            let content_y = prect.y + PORTHOLE_MARGIN + cell.1;
            let shown = view.layout.visible(inner.y - content_y, inner.y + inner.h - content_y);
            let name = ph.detected.language.as_ref().map_or("Plain text".to_string(), |l| l.name());
            let chrome = scene::PortholeChrome { fg: s.palette.fg, bg: s.palette.bg, language: &name, wide: ph.wide, scroll_x: view.scroll_x };
            let controls = scene::porthole(frame, prect, inner, shown, &chrome, cell);
            hits.push((id, pid, controls, prect));
        }
        hits
    }
}
