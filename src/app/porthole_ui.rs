//! Portholes in the window: laying their documents out with the renderer's fonts (cached
//! per width, font, and look), drawing them over their rows, and making them.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::Line;

use super::*;
use crate::porthole::detect::Language;
use crate::porthole::render::{self, Face, Layout, Pos};
use crate::porthole::store::{rows_needed, Porthole, Source};
use crate::render::Renderer;
use crate::scene::PORTHOLE_MARGIN;

/// A porthole's layout and what it was made for, and its sideways scroll (Wide).
pub(super) struct PView {
    pub layout: Layout,
    key: (u32, u32, bool, String, bool),
    pub scroll_x: f32,
    /// This frame's selection and find marks, from the text's top left.
    pub marks: Vec<(Rect, crate::palette::Rgb, f32)>,
}

/// Which way a porthole selection is copied.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CopyAs {
    Plain,
    Styles,
    ControlSequences,
}

/// Rectangles over the text between two positions, one per line.
fn span_rects(layout: &Layout, a: Pos, b: Pos, measure: &dyn Fn(&str, Face, f32) -> f32) -> Vec<Rect> {
    let (a, b) = if a <= b { (a, b) } else { (b, a) };
    let mut out = Vec::new();
    for i in a.line..=b.line.min(layout.lines.len().saturating_sub(1)) {
        let l = &layout.lines[i];
        let len = layout.line_text(i).chars().count();
        let from = if i == a.line { a.col } else { 0 };
        let to = if i == b.line { b.col } else { len };
        let x0 = layout.x_of(Pos { line: i, col: from }, measure);
        let x1 = layout.x_of(Pos { line: i, col: to }, measure);
        // An empty line in the middle still shows as selected.
        let w = if x1 > x0 { x1 - x0 } else if i != b.line { 4.0 } else { 0.0 };
        if w > 0.0 {
            out.push(Rect { x: x0, y: l.y, w, h: l.h });
        }
    }
    out
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
                self.porthole_views.insert((pane, *pid), PView { layout, key, scroll_x, marks: Vec::new() });
            }
        }
        self.porthole_marks();
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

    /// This frame's marks: the selection in its porthole, and find matches while the find
    /// bar is open.
    fn porthole_marks(&mut self) {
        let mode = self.find_mode();
        let queries: HashMap<PaneId, String> = self.finds.iter().filter(|(_, f)| f.open && !f.query.is_empty()).map(|(id, f)| (*id, f.query.clone())).collect();
        let Some(gpu) = self.gpu.as_mut() else { return };
        let r = RefCell::new(&mut gpu.renderer);
        let measure = |t: &str, f: Face, s: f32| r.borrow_mut().text_width(t, f == Face::Prose, s);
        for ((pane, pid), view) in self.porthole_views.iter_mut() {
            view.marks.clear();
            let Some(p) = self.panes.get(pane) else { continue };
            if let Some(re) = queries.get(pane).and_then(|q| regex::Regex::new(&crate::find::pattern(q, mode)).ok()) {
                for (a, b) in view.layout.find(&re) {
                    view.marks.extend(span_rects(&view.layout, a, b, &measure).into_iter().map(|m| (m, crate::palette::rgb(255, 255, 0), 0.8)));
                }
            }
            if let Some((_, a, b)) = p.screen.porthole_selection.filter(|s| s.0 == *pid) {
                view.marks.extend(span_rects(&view.layout, a, b, &measure).into_iter().map(|m| (m, p.screen.palette.selection, 1.0)));
            }
        }
    }

    /// Where a point is in a porthole's text, held to its first and last lines.
    fn porthole_pos(&mut self, pane: PaneId, pid: u32, frame: Rect, x: f32, y: f32) -> Option<Pos> {
        let cell = self.cell();
        let view = self.porthole_views.get(&(pane, pid))?;
        let gpu = self.gpu.as_mut()?;
        let r = RefCell::new(&mut gpu.renderer);
        let measure = |t: &str, f: Face, s: f32| r.borrow_mut().text_width(t, f == Face::Prose, s);
        let (ox, oy) = (frame.x + PORTHOLE_MARGIN - view.scroll_x, frame.y + PORTHOLE_MARGIN + cell.1);
        let last = view.layout.lines.len().checked_sub(1)?;
        if y < oy {
            return Some(Pos { line: 0, col: 0 });
        }
        Some(view.layout.position(x - ox, y - oy, &measure).unwrap_or(Pos { line: last, col: view.layout.line_text(last).chars().count() }))
    }

    /// A press on a porthole: its controls, or the start of a selection (or a link click).
    pub(super) fn porthole_press(&mut self, x: f32, y: f32) -> bool {
        let Some(&(pane, pid, c, frame)) = self.porthole_hits.iter().find(|h| h.3.contains(x, y)) else { return false };
        if c.close.contains(x, y) {
            if let Some(p) = self.panes.get_mut(&pane) {
                p.screen.close_porthole(pid);
            }
            self.porthole_views.remove(&(pane, pid));
            self.redraw();
            return true;
        }
        if c.language.contains(x, y) {
            self.show_language_list(pane, pid);
            return true;
        }
        if c.wide.contains(x, y) {
            if let Some(ph) = self.panes.get_mut(&pane).and_then(|p| p.screen.portholes.get_mut(&pid)) {
                ph.wide = !ph.wide;
                ph.sized = false;
            }
            if let Some(v) = self.porthole_views.get_mut(&(pane, pid)) {
                v.scroll_x = 0.0;
            }
            self.redraw();
            return true;
        }
        let Some(pos) = self.porthole_pos(pane, pid, frame, x, y) else { return true };
        self.clear_porthole_selections();
        if let Some(p) = self.panes.get_mut(&pane) {
            p.screen.select_in_porthole(pid, pos, pos);
        }
        self.porthole_link = self.porthole_views.get(&(pane, pid)).and_then(|v| v.layout.link_at(pos)).map(|l| (pane, pid, l.to_string()));
        self.drag = Drag::Porthole { pane, id: pid, frame };
        self.redraw();
        true
    }

    pub(super) fn porthole_drag(&mut self, pane: PaneId, pid: u32, frame: Rect, x: f32, y: f32) {
        let Some(pos) = self.porthole_pos(pane, pid, frame, x, y) else { return };
        if let Some(sel) = self.panes.get_mut(&pane).and_then(|p| p.screen.porthole_selection.as_mut()).filter(|s| s.0 == pid) {
            sel.2 = pos;
        }
        self.redraw();
    }

    /// The button came up: a click with no drag drops the empty selection, or opens a link.
    pub(super) fn porthole_release(&mut self, pane: PaneId, pid: u32) {
        let link = self.porthole_link.take();
        let Some(p) = self.panes.get_mut(&pane) else { return };
        let empty = p.screen.porthole_selection.is_none_or(|(_, a, b)| a == b);
        if !empty {
            return self.redraw();
        }
        p.screen.clear_porthole_selection();
        if let Some((_, _, l)) = link.filter(|(lp, lid, _)| *lp == pane && *lid == pid) {
            let base = p.screen.portholes.get(&pid).and_then(|ph| ph.base_dir.clone());
            self.open_porthole_link(pane, &l, base.as_deref());
        }
        self.redraw();
    }

    pub(super) fn clear_porthole_selections(&mut self) {
        for p in self.panes.values_mut() {
            p.screen.clear_porthole_selection();
        }
    }

    /// Sideways scrolling in a Wide porthole (a sideways wheel, or Shift with the wheel).
    pub(super) fn porthole_wheel(&mut self, dx: f32, dy: f32) -> bool {
        let (x, y) = self.cursor;
        let Some(&(pane, pid, _, frame)) = self.porthole_hits.iter().find(|h| h.3.contains(x, y)) else { return false };
        let wide = self.panes.get(&pane).and_then(|p| p.screen.portholes.get(&pid)).is_some_and(|ph| ph.wide);
        let amount = if dx != 0.0 { dx } else if self.mods.shift { dy } else { 0.0 };
        if !wide || amount == 0.0 {
            return false;
        }
        let cw = self.cell().0;
        if let Some(v) = self.porthole_views.get_mut(&(pane, pid)) {
            let max = (v.layout.width - (frame.w - 2.0 * PORTHOLE_MARGIN)).max(0.0);
            v.scroll_x = (v.scroll_x - amount * cw).clamp(0.0, max);
        }
        self.redraw();
        true
    }

    /// Copy, Copy with Styles, or Copy with Control Sequences of a porthole's selection.
    /// False when the pane has none.
    pub(super) fn copy_porthole_selection(&mut self, id: PaneId, how: CopyAs) -> bool {
        let Some(p) = self.panes.get(&id) else { return false };
        let Some((pid, a, b)) = p.screen.porthole_selection else { return false };
        let Some(view) = self.porthole_views.get(&(id, pid)) else { return false };
        let text = view.layout.text(a, b);
        let rows = view.layout.rows_between(a, b);
        let bg = p.screen.palette.bg;
        let result = match how {
            CopyAs::Plain => self.clipboard.as_mut().map(|cb| cb.set_text(text).map_err(|e| e.to_string())),
            CopyAs::ControlSequences => self.clipboard.as_mut().map(|cb| cb.set_text(crate::porthole::sgr(&rows)).map_err(|e| e.to_string())),
            CopyAs::Styles => {
                let styled: Vec<crate::styled_copy::Row> = rows
                    .iter()
                    .map(|(runs, hard)| crate::styled_copy::Row {
                        runs: runs.iter().map(|r| crate::styled_copy::Run { text: r.text.clone(), fg: r.color, bg, bold: r.bold, italic: r.italic, underline: r.underline, link: r.link.clone() }).collect(),
                        hard_end: *hard,
                    })
                    .collect();
                let font = self.profile_of(id).map(|pr| pr.font("Normal Font")).unwrap_or(crate::profile::FontSpec { family: "Cascadia Mono".into(), size_pt: 12.0 });
                let html = crate::styled_copy::cf_html(&crate::styled_copy::html(&styled, &font.family, font.size_pt, bg));
                let rtf = crate::styled_copy::rtf(&styled, &font.family, font.size_pt);
                Some(crate::winclip::set(&text, &html, &rtf))
            }
        };
        if let Some(Err(e)) = result {
            self.banners.set("clipboard", format!("Copy failed: {e}"));
        }
        true
    }

    /// The language button: the candidates first, then every other language.
    fn show_language_list(&mut self, pane: PaneId, pid: u32) {
        let Some(ph) = self.panes.get(&pane).and_then(|p| p.screen.portholes.get(&pid)) else { return };
        let mut first = ph.detected.candidates.clone();
        first.sort_by_key(|l| l.name().to_lowercase());
        let rest: Vec<Language> = crate::porthole::detect::languages().into_iter().filter(|l| !first.contains(l)).collect();
        let mut items: Vec<Option<String>> = first.iter().map(|l| Some(l.name())).collect();
        if !first.is_empty() {
            items.push(None);
        }
        items.extend(rest.iter().map(|l| Some(l.name())));
        let mut all = first;
        all.extend(rest);
        self.porthole_languages = Some((pane, pid, all));
        self.show_porthole_languages(&items);
    }

    /// A language picked from the list: the porthole renders again and takes new rows.
    pub(super) fn porthole_language_picked(&mut self, id: &str) -> bool {
        let Some(i) = id.strip_prefix("porthole-lang:").and_then(|i| i.parse::<usize>().ok()) else { return false };
        let Some((pane, pid, list)) = self.porthole_languages.take() else { return true };
        if let (Some(lang), Some(ph)) = (list.get(i), self.panes.get_mut(&pane).and_then(|p| p.screen.portholes.get_mut(&pid))) {
            ph.detected.language = Some(lang.clone());
            ph.sized = false;
        }
        self.redraw();
        true
    }

    /// A link in a rendered document: web and mail links and local files, after asking
    /// (unless asked not to).
    fn open_porthole_link(&mut self, pane: PaneId, link: &str, base: Option<&str>) {
        let Some(target) = crate::porthole::link_target(link, base) else {
            return self.banners.set("link", format!("Tandem opens web and mail links and local files only, not {link:?}."));
        };
        let (text, key) = crate::porthole::link_prompt(&target);
        if self.globals.bool(key) {
            return self.open_link_target(&target);
        }
        if self.ask(pane, "porthole-link", &text, &["_Yes", "Always", "_No"], true) {
            self.porthole_link_asks.insert(pane, target);
        }
    }

    fn open_link_target(&mut self, target: &crate::porthole::LinkTarget) {
        let t = match target {
            crate::porthole::LinkTarget::Url(u) | crate::porthole::LinkTarget::File(u) => u.clone(),
        };
        if let Err(e) = super::semantic_ui::shell_open(&t, None, false) {
            self.banners.set("link", format!("Could not open {t}: {e}"));
        }
    }

    /// The link bar's answer. False for other bars.
    pub(super) fn porthole_link_bar(&mut self, pane: PaneId, id: &str, button: Option<&str>) -> bool {
        if id != "porthole-link" {
            return false;
        }
        let Some(target) = self.porthole_link_asks.remove(&pane) else { return true };
        match button {
            Some("_Yes") => self.open_link_target(&target),
            Some("Always") => {
                let (_, key) = crate::porthole::link_prompt(&target);
                self.globals.set(key, toml::Value::Boolean(true));
                self.save_globals();
                self.open_link_target(&target);
            }
            _ => {}
        }
        true
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
            let controls = scene::porthole(frame, prect, inner, shown, &view.marks, &chrome, cell);
            hits.push((id, pid, controls, prect));
        }
        hits
    }
}
