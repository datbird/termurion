//! Clippings in the app: each session's list, the panel at its right (drawn, clicked,
//! scrolled, resized, reordered), View > Show Clippings, and the New Clipping sheet.

use super::*;
use crate::clip_add_ui::{AddClipWindow, AddEvent};
use crate::clippings::{self, Button as ClipButton, Clipping, Clippings, Hit};
use crate::layout::visible_rects;

/// iTerm2's key for the panel's width.
pub(super) const WIDTH_KEY: &str = "NoSyncGutterPanelWidth_com.iterm2.clippings";

/// One session's clippings and the panel's state.
#[derive(Default)]
pub(super) struct ClipState {
    pub(super) model: Clippings,
    pub(super) selected: Vec<usize>,
    pub(super) scroll: f32,
    pub(super) preview: bool,
    /// The panel has the keyboard (a click in it; a click in the terminal takes it back).
    pub(super) keys: bool,
    /// The last row clicked and when (a second click soon after is a double-click).
    last_click: Option<(usize, Instant)>,
}

/// A drag that began in a panel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum ClipDrag {
    /// The width: where the drag began and the width then (logical pixels).
    Resize { start_x: f32, start_w: f32 },
    Row { from: usize },
}

pub(super) fn double_click_ms() -> u128 {
    u128::from(unsafe { windows_sys::Win32::UI::Input::KeyboardAndMouse::GetDoubleClickTime() })
}

impl App {
    /// The panel's width in logical pixels.
    fn clip_width(&self) -> f32 {
        let v = self.globals.get(WIDTH_KEY).and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64)));
        clippings::clamp_width(v.map_or(clippings::DEFAULT_WIDTH, |f| f as f32))
    }

    /// A pane's terminal rectangle, then its Clippings panel's and its inline chat's when they
    /// show (Clippings on the left). The one place that takes room from a pane: `rect` is the
    /// pane less its own status bar, and every size, tmux's report included, follows from the
    /// terminal rectangle this answers.
    pub(super) fn gutter_split(&self, id: PaneId, rect: Rect) -> (Rect, Option<Rect>, Option<Rect>) {
        let clip = if self.clips.get(&id).is_some_and(|c| c.model.visible) { self.clip_width() * self.scale() } else { 0.0 };
        crate::inline_chat::split_gutter(rect, clip, self.inline_width(id) * self.scale())
    }

    /// The panels of the current tab: (pane, panel rectangle).
    fn clip_panels(&self) -> Vec<(PaneId, Rect)> {
        let Some(tab) = self.tabs.get(self.active) else { return Vec::new() };
        visible_rects(&tab.layout, tab.zoomed, self.pane_area_for(tab), self.gap()).into_iter().filter_map(|(id, r)| self.gutter_split(id, self.pane_bar_split(tab, id, r).0).1.map(|p| (id, p))).collect()
    }

    /// Characters per detail line in a panel.
    fn clip_cols(&self, panel: Rect) -> usize {
        let (cw, _) = self.cell();
        ((panel.w - 20.0 * self.scale()) / cw).floor().max(1.0) as usize
    }

    /// The rows as drawn: (title, detail lines), details capped at 100 pixels.
    fn clip_rows(&self, id: PaneId, panel: Rect) -> Vec<scene::ClipRow> {
        let (_, ch) = self.cell();
        let cap = ((100.0 * self.scale()) / ch).floor().max(1.0) as usize;
        let cols = self.clip_cols(panel);
        let Some(c) = self.clips.get(&id) else { return Vec::new() };
        c.model
            .shown()
            .iter()
            .map(|k| {
                let mut detail = clippings::wrap(&clippings::plain(&k.detail), cols);
                detail.truncate(cap);
                scene::ClipRow { title: k.title.clone(), detail }
            })
            .collect()
    }

    fn clip_layout(&self, id: PaneId, panel: Rect) -> (clippings::Layout, Vec<scene::ClipRow>) {
        let (_, ch) = self.cell();
        let rows = self.clip_rows(id, panel);
        let heights: Vec<(f32, f32)> = rows.iter().map(|r| (ch, r.detail.len() as f32 * ch)).collect();
        let scroll = self.clips.get(&id).map_or(0.0, |c| c.scroll);
        let l = clippings::layout(panel, &heights, scroll, self.scale());
        // Rows removed or rewrapped can leave the scroll past the end.
        let kept = clippings::clamp_scroll(scroll, &l);
        let l = if kept == scroll { l } else { clippings::layout(panel, &heights, kept, self.scale()) };
        (l, rows)
    }

    /// Draws the current tab's panels and any preview.
    pub(super) fn draw_clippings(&self, frame: &mut scene::Frame, cell: (f32, f32)) {
        for (id, panel) in self.clip_panels() {
            let Some(c) = self.clips.get(&id) else { continue };
            let (layout, rows) = self.clip_layout(id, panel);
            let hover = self.clip_hover.filter(|(p, _)| *p == id).map(|(_, b)| b.tooltip().to_string());
            let status = hover.or_else(|| c.model.status().map(|(s, _)| s));
            let drop_y = match self.drag {
                Drag::Clip { pane, kind: ClipDrag::Row { .. } } if pane == id => Some(self.clip_drop(&layout, self.cursor.1).1),
                _ => None,
            };
            let d = scene::ClipDraw { layout: &layout, panel, rows: &rows, selected: &c.selected, enabled: c.model.enabled(c.selected.len()), status: status.as_deref(), drop_y };
            scene::clippings_panel(frame, &d, cell, self.scale());
            if c.preview {
                if let (Some(&row), Some(tab)) = (c.selected.first(), self.tabs.get(self.active)) {
                    let term = self.visible(tab).into_iter().find(|(p, _)| *p == id).map(|(_, r)| r);
                    if let (Some(k), Some(term)) = (c.model.shown().get(row), term) {
                        let cols = ((700.0 * self.scale()).min(term.w) / cell.0).floor().max(10.0) as usize - 3;
                        let lines = clippings::wrap(&clippings::plain(&k.detail), cols);
                        scene::clip_preview(frame, term, &k.title, &lines, cell, self.scale());
                    }
                }
            }
        }
    }

    /// The rectangles `draw_clippings` covers: each panel and a shown preview (the preview takes
    /// the pane's whole text area).
    pub(super) fn clip_holes(&self) -> Vec<Rect> {
        let mut out = Vec::new();
        for (id, panel) in self.clip_panels() {
            out.push(panel);
            let Some(c) = self.clips.get(&id) else { continue };
            if let (true, Some(tab)) = (c.preview && !c.selected.is_empty(), self.tabs.get(self.active)) {
                out.extend(self.visible(tab).into_iter().find(|(p, _)| *p == id).map(|(_, r)| r));
            }
        }
        out
    }

    /// Where a dragged row lands: the row index it goes before and the line's y.
    fn clip_drop(&self, l: &clippings::Layout, y: f32) -> (usize, f32) {
        for (i, r) in l.rows.iter().enumerate() {
            if y < r.y + r.h / 2.0 {
                return (i, r.y);
            }
        }
        (l.rows.len(), l.rows.last().map_or(l.list.y, |r| r.y + r.h))
    }

    /// A left press. True when it was in a panel.
    pub(super) fn clip_press(&mut self, x: f32, y: f32, event_loop: &ActiveEventLoop) -> bool {
        let Some((id, panel)) = self.clip_panels().into_iter().find(|(_, p)| p.contains(x, y)) else {
            // A press anywhere else gives the keyboard back to the terminal.
            for c in self.clips.values_mut() {
                c.keys = false;
            }
            return false;
        };
        // One gutter panel holds the keyboard: the inline chat gives it up.
        super::inline_chat_ui::release_gutter_keys(&mut self.clips, &mut self.inline, super::inline_chat_ui::GutterOwner::Clippings(id));
        // The panel's session becomes the focused one.
        if let Some(t) = self.tabs.get_mut(self.active) {
            t.focused = id;
        }
        self.refresh_route();
        let (layout, _) = self.clip_layout(id, panel);
        let ctrl = self.mods.ctrl;
        let shift = self.mods.shift;
        match clippings::hit(&layout, x, y) {
            Hit::Resize => self.drag = Drag::Clip { pane: id, kind: ClipDrag::Resize { start_x: x, start_w: self.clip_width() } },
            Hit::Button(b) => self.clip_button(id, b, event_loop),
            Hit::Row(i) => {
                let now = Instant::now();
                let Some(c) = self.clips.get_mut(&id) else { return true };
                c.keys = true;
                let double = c.last_click.is_some_and(|(r, t)| r == i && now.duration_since(t).as_millis() <= double_click_ms());
                c.last_click = Some((i, now));
                if ctrl {
                    match c.selected.iter().position(|&r| r == i) {
                        Some(k) => {
                            c.selected.remove(k);
                        }
                        None => c.selected.push(i),
                    }
                } else if shift && !c.selected.is_empty() {
                    let a = c.selected[0];
                    c.selected = if a <= i { (a..=i).collect() } else { (i..=a).rev().collect() };
                } else {
                    c.selected = vec![i];
                }
                if double {
                    // A double-click pastes that clipping's detail only (iTerm2's).
                    if let Some(detail) = c.model.shown().get(i).map(|k| k.detail.clone()) {
                        c.last_click = None;
                        self.user_paste(id, &detail, crate::broadcast::Write::Paste);
                    }
                } else if c.model.viewing_live() && !ctrl && !shift {
                    self.drag = Drag::Clip { pane: id, kind: ClipDrag::Row { from: i } };
                }
            }
            Hit::Nothing => {
                if let Some(c) = self.clips.get_mut(&id) {
                    c.keys = true;
                    c.selected.clear();
                    c.preview = false;
                }
            }
        }
        self.redraw();
        true
    }

    fn clip_button(&mut self, id: PaneId, b: ClipButton, event_loop: &ActiveEventLoop) {
        let separator = self.globals.string("ClippingSeparator");
        let Some(c) = self.clips.get(&id) else { return };
        let e = c.model.enabled(c.selected.len());
        let mut sorted = c.selected.clone();
        sorted.sort_unstable();
        let picked: Vec<&Clipping> = sorted.iter().filter_map(|&i| c.model.shown().get(i)).collect();
        let text = clippings::format(&picked, &separator);
        let on = match b {
            ClipButton::Add => e.add,
            ClipButton::Remove => e.remove,
            ClipButton::Archive => e.archive,
            ClipButton::Send => e.send,
            ClipButton::Copy => e.copy,
            ClipButton::Back => e.back,
            ClipButton::Forward => e.forward,
            ClipButton::Close => true,
        };
        if !on {
            return;
        }
        match b {
            ClipButton::Add => return self.open_add_clipping(event_loop, id),
            ClipButton::Send => return self.user_paste(id, &text, crate::broadcast::Write::Paste),
            ClipButton::Copy => {
                if let Some(cb) = self.clipboard.as_mut() {
                    if let Err(err) = cb.set_text(text) {
                        self.banners.set("clipboard", format!("Could not copy: {err}"));
                    }
                }
                return;
            }
            _ => {}
        }
        let Some(c) = self.clips.get_mut(&id) else { return };
        c.preview = false;
        match b {
            ClipButton::Remove => {
                let rows = std::mem::take(&mut c.selected);
                c.model.remove(&rows);
            }
            ClipButton::Archive => {
                c.model.archive_all();
                c.selected.clear();
                c.scroll = 0.0;
            }
            ClipButton::Back | ClipButton::Forward => {
                c.model.step(b == ClipButton::Back);
                c.selected.clear();
                c.scroll = 0.0;
            }
            _ => {
                c.model.visible = false;
                c.keys = false;
                self.relayout();
                self.sync_checks();
            }
        }
        self.mark_state_dirty();
        self.redraw();
    }

    /// Pointer motion during a panel drag, and button tooltips.
    pub(super) fn clip_move(&mut self, x: f32, y: f32) {
        let hover = self.clip_panels().into_iter().find(|(_, p)| p.contains(x, y)).and_then(|(id, panel)| match clippings::hit(&self.clip_layout(id, panel).0, x, y) {
            Hit::Button(b) => Some((id, b)),
            _ => None,
        });
        if hover != self.clip_hover {
            self.clip_hover = hover;
            self.redraw();
        }
        match self.drag {
            Drag::Clip { kind: ClipDrag::Resize { start_x, start_w }, .. } => {
                let w = clippings::clamp_width(start_w + (start_x - x) / self.scale());
                self.globals.set(WIDTH_KEY, toml::Value::Float(f64::from(w.round())));
                self.relayout();
            }
            Drag::Clip { kind: ClipDrag::Row { .. }, .. } => self.redraw(),
            _ => {}
        }
    }

    pub(super) fn clip_release(&mut self, pane: PaneId, kind: ClipDrag) {
        match kind {
            ClipDrag::Resize { .. } => self.save_globals(),
            ClipDrag::Row { from } => {
                let Some(panel) = self.clip_panels().into_iter().find(|(p, _)| *p == pane).map(|(_, r)| r) else { return };
                let (layout, _) = self.clip_layout(pane, panel);
                if !layout.list.contains(self.cursor.0, self.cursor.1) {
                    return self.redraw();
                }
                let (to, _) = self.clip_drop(&layout, self.cursor.1);
                if let Some(c) = self.clips.get_mut(&pane) {
                    if let Some(at) = c.model.move_row(from, to) {
                        c.selected = vec![at];
                        self.mark_state_dirty();
                    }
                }
            }
        }
        self.redraw();
    }

    /// The wheel over a panel scrolls its list. True when it did.
    pub(super) fn clip_wheel(&mut self, dy: f32) -> bool {
        let (x, y) = self.cursor;
        let Some((id, panel)) = self.clip_panels().into_iter().find(|(_, p)| p.contains(x, y)) else { return false };
        let (layout, _) = self.clip_layout(id, panel);
        let (_, ch) = self.cell();
        let now = layout.rows.first().map_or(0.0, |r| layout.list.y - r.y);
        if let Some(c) = self.clips.get_mut(&id) {
            c.scroll = clippings::clamp_scroll(now - dy * ch, &layout);
        }
        self.redraw();
        true
    }

    /// Keys while the focused session's panel has the keyboard: Delete removes, Space
    /// previews, Up and Down move, Esc closes the preview or gives the keyboard back.
    /// Other keys go to the terminal (and take the keyboard back).
    pub(super) fn clip_key(&mut self, ev: &KeyEvent) -> bool {
        use winit::keyboard::{Key, NamedKey};
        let Some(id) = self.focused_id() else { return false };
        // Modifier keys alone change nothing; chords with Ctrl, Alt, or Win go on as usual.
        if matches!(ev.logical_key, Key::Named(NamedKey::Shift | NamedKey::Control | NamedKey::Alt | NamedKey::Super | NamedKey::Meta | NamedKey::AltGraph)) || self.mods.ctrl || self.mods.alt || self.mods.cmd {
            return false;
        }
        let Some(c) = self.clips.get_mut(&id).filter(|c| c.keys && c.model.visible) else { return false };
        if ev.state != winit::event::ElementState::Pressed {
            return matches!(ev.logical_key, Key::Named(NamedKey::Delete | NamedKey::Backspace | NamedKey::Space | NamedKey::ArrowUp | NamedKey::ArrowDown | NamedKey::Escape));
        }
        let n = c.model.shown().len();
        match &ev.logical_key {
            Key::Named(NamedKey::Delete | NamedKey::Backspace) => {
                if c.model.viewing_live() && !c.selected.is_empty() {
                    let rows = std::mem::take(&mut c.selected);
                    c.model.remove(&rows);
                    c.preview = false;
                    self.mark_state_dirty();
                }
            }
            Key::Named(NamedKey::Space) => c.preview = !c.preview && !c.selected.is_empty(),
            Key::Named(NamedKey::Escape) => {
                if c.preview {
                    c.preview = false;
                } else {
                    c.keys = false;
                }
            }
            Key::Named(NamedKey::ArrowUp) if n > 0 => {
                c.selected = vec![c.selected.first().map_or(n - 1, |&r| r.saturating_sub(1))];
                self.clip_reveal(id);
            }
            Key::Named(NamedKey::ArrowDown) if n > 0 => {
                c.selected = vec![c.selected.first().map_or(0, |&r| (r + 1).min(n - 1))];
                self.clip_reveal(id);
            }
            Key::Named(NamedKey::ArrowUp | NamedKey::ArrowDown) => {}
            _ => {
                c.keys = false;
                c.preview = false;
                self.redraw();
                return false;
            }
        }
        self.redraw();
        true
    }

    /// View > Show Clippings: the focused session's panel.
    pub(super) fn toggle_clippings(&mut self) {
        let Some(id) = self.focused_id() else { return };
        if self.tmux_of(id).is_some() && !self.clippings_shown() {
            return self.banners.set("clippings", "Clippings are not shown in tmux panes: tmux sizes the whole window and the panel would take columns it does not know about.");
        }
        let c = self.clips.entry(id).or_default();
        c.model.visible = !c.model.visible;
        if !c.model.visible {
            c.preview = false;
            c.keys = false;
        }
        self.mark_state_dirty();
        self.relayout();
        self.sync_checks();
    }

    pub(super) fn clippings_shown(&self) -> bool {
        self.focused_id().and_then(|id| self.clips.get(&id)).is_some_and(|c| c.model.visible)
    }

    fn open_add_clipping(&mut self, event_loop: &ActiveEventLoop, pane: PaneId) {
        // An open sheet now adds to this session.
        if let Some((w, p)) = &mut self.add_clip_win {
            *p = pane;
            w.focus();
            return;
        }
        let Some(g) = self.gpu.as_ref() else { return };
        match AddClipWindow::open(event_loop, &g.instance, &g.adapter, &g.device, &g.queue) {
            Ok(w) => self.add_clip_win = Some((w, pane)),
            Err(e) => self.banners.set("window", format!("Could not open the window: {e}")),
        }
    }

    pub(super) fn add_clip_event(&mut self, window: winit::window::WindowId, event: &WindowEvent) -> bool {
        let Some((w, pane)) = self.add_clip_win.as_mut() else { return false };
        if w.id() != window {
            return false;
        }
        let pane = *pane;
        match event {
            WindowEvent::CloseRequested => self.add_clip_win = None,
            WindowEvent::RedrawRequested => match w.redraw() {
                AddEvent::None => {}
                AddEvent::Cancel => self.add_clip_win = None,
                AddEvent::Add(title, detail) => {
                    self.add_clip_win = None;
                    // The panel's add always passes an empty type (iTerm2's).
                    let home = self.clip_home(pane);
                    self.clips.entry(home).or_default().model.add(Clipping { kind: String::new(), title, detail });
                    self.mark_state_dirty();
                    self.relayout();
                    self.sync_checks();
                }
            },
            other => {
                w.event(other);
            }
        }
        true
    }

    /// The session keys an arrangement keeps for a pane.
    pub(super) fn clippings_table(&self, id: PaneId) -> toml::Table {
        self.clips.get(&id).map(|c| c.model.to_table()).unwrap_or_default()
    }

    /// A restored session's clippings.
    pub(super) fn restore_clippings(&mut self, id: PaneId, session: &toml::Value) {
        let Some(t) = session.as_table() else { return };
        let model = Clippings::from_table(t);
        if model != Clippings::default() {
            self.clips.insert(id, ClipState { model, ..ClipState::default() });
        }
    }
}

impl App {
    /// Whether (x, y) is in one of the current tab's panels.
    pub(super) fn clip_split_hit(&self, x: f32, y: f32) -> bool {
        self.clip_panels().iter().any(|(_, p)| p.contains(x, y))
    }
}

impl App {
    /// Scrolls a panel so its first selected row shows whole.
    fn clip_reveal(&mut self, id: PaneId) {
        let Some(panel) = self.clip_panels().into_iter().find(|(p, _)| *p == id).map(|(_, r)| r) else { return };
        let (layout, _) = self.clip_layout(id, panel);
        let now = layout.rows.first().map_or(0.0, |r| layout.list.y - r.y);
        if let Some(c) = self.clips.get_mut(&id) {
            if let Some(&row) = c.selected.first() {
                c.scroll = clippings::reveal(now, &layout, row);
            }
        }
    }
}
