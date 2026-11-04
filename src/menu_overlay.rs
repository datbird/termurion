//! The menu bar drawn inside the terminal window (Linux), and the pop-up menus beside it. It
//! reads the same muda menu the other systems show natively, live each frame, and sends the
//! chosen item's id back as a plain string. Nothing here touches the OS: the open state, the
//! layout, the pointer and the keys. The app draws the output (`take_output`) on its own surface.

use egui::{Align2, Color32, Context, FontId, Id, LayerId, Order, Pos2, Rect, Shape, Stroke, Vec2, pos2, vec2};
use muda::MenuItemKind;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::keyboard::{Key, NamedKey};

/// The strip's height in points.
const BAR_PT: f32 = 24.0;
const ROW_PT: f32 = 22.0;
const SEP_PT: f32 = 9.0;
const PAD: f32 = 12.0;
const CHECK_PT: f32 = 22.0;
const ARROW_PT: f32 = 16.0;
const GAP: f32 = 28.0;
const TITLE_PAD: f32 = 10.0;
const TEXT_PT: f32 = 14.0;
const EDGE: f32 = 4.0;

/// Whether the overlay took an event; a consumed one does not reach the terminal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Consumed {
    No,
    Yes,
}

/// One displayed row, read from a muda item.
#[derive(Clone, Debug)]
enum Row {
    Sep,
    Item { id: String, text: String, shortcut: String, enabled: bool, checked: Option<bool>, sub: bool },
}

impl Row {
    fn of(kind: &MenuItemKind) -> Row {
        let split = |t: String| -> (String, String) {
            let t = t.replace("&&", "&");
            match t.split_once('\t') {
                Some((a, b)) => (a.to_string(), b.to_string()),
                None => (t, String::new()),
            }
        };
        match kind {
            MenuItemKind::MenuItem(i) => {
                let (text, shortcut) = split(i.text());
                Row::Item { id: i.id().0.clone(), text, shortcut, enabled: i.is_enabled(), checked: None, sub: false }
            }
            MenuItemKind::Check(i) => {
                let (text, shortcut) = split(i.text());
                Row::Item { id: i.id().0.clone(), text, shortcut, enabled: i.is_enabled(), checked: Some(i.is_checked()), sub: false }
            }
            MenuItemKind::Submenu(s) => {
                let (text, shortcut) = split(s.text());
                Row::Item { id: s.id().0.clone(), text, shortcut, enabled: s.is_enabled(), checked: None, sub: true }
            }
            MenuItemKind::Icon(i) => {
                let (text, shortcut) = split(i.text());
                Row::Item { id: i.id().0.clone(), text, shortcut, enabled: i.is_enabled(), checked: None, sub: false }
            }
            // A separator has no text; any other predefined item is a plain row.
            MenuItemKind::Predefined(p) => {
                let text = p.text();
                if text.is_empty() {
                    Row::Sep
                } else {
                    Row::Item { id: p.id().0.clone(), text: text.replace("&&", "&"), shortcut: String::new(), enabled: true, checked: None, sub: false }
                }
            }
        }
    }

    /// Whether the keys and the pointer can land on it.
    fn live(&self) -> bool {
        matches!(self, Row::Item { enabled: true, .. })
    }

    fn is_sub(&self) -> bool {
        matches!(self, Row::Item { sub: true, .. })
    }
}

/// One open dropdown: the level's items and the row the pointer or the keys are on.
struct Level {
    items: Vec<MenuItemKind>,
    hot: Option<usize>,
    scroll: f32,
    reveal: bool,
}

impl Level {
    fn new(items: Vec<MenuItemKind>) -> Level {
        Level { items, hot: None, scroll: 0.0, reveal: false }
    }

    fn rows(&self) -> Vec<Row> {
        self.items.iter().map(Row::of).collect()
    }
}

/// Where the last paint put things, in points.
#[derive(Default)]
struct Frame {
    titles: Vec<(Rect, usize)>,
    levels: Vec<LevelFrame>,
}

struct LevelFrame {
    rect: Rect,
    /// The visible rows: (rect, index in the level's items).
    rows: Vec<(Rect, usize)>,
}

pub struct Overlay {
    levels: Vec<Level>,
    /// The bar title the first level belongs to; None for a pop-up.
    title: Option<usize>,
    /// Where a pop-up's first level goes.
    popup_at: Pos2,
    bar_items: Vec<MenuItemKind>,
    hover_title: Option<usize>,
    pointer: Option<Pos2>,
    /// The window in points, and the points to pixels scale.
    screen: Vec2,
    ppp: f32,
    swallow_release: bool,
    chosen: Option<String>,
    frame: Frame,
    out: Option<egui::FullOutput>,
    dirty: bool,
    started: std::time::Instant,
}

impl Drop for Overlay {
    fn drop(&mut self) {
        // egui's debug build checks that no texture change is dropped unhandled.
        if let Some(out) = self.out.as_mut() {
            out.textures_delta.clear();
        }
    }
}

impl Default for Overlay {
    fn default() -> Self {
        Overlay::new()
    }
}

impl Overlay {
    pub fn new() -> Overlay {
        Overlay {
            levels: Vec::new(),
            title: None,
            popup_at: Pos2::ZERO,
            bar_items: Vec::new(),
            hover_title: None,
            pointer: None,
            screen: vec2(800.0, 600.0),
            ppp: 1.0,
            swallow_release: false,
            chosen: None,
            frame: Frame::default(),
            out: None,
            dirty: false,
            started: std::time::Instant::now(),
        }
    }

    /// The strip's height in pixels at `scale`.
    pub fn bar_height(scale: f32) -> f32 {
        (BAR_PT * scale).round()
    }

    /// The window's size in pixels and its scale; a size change closes an open menu.
    pub fn set_screen(&mut self, width_px: f32, height_px: f32, scale: f32) {
        let scale = if scale > 0.0 { scale } else { 1.0 };
        let screen = vec2(width_px / scale, height_px / scale);
        if (screen - self.screen).length() > 0.5 || (scale - self.ppp).abs() > f32::EPSILON {
            self.close();
        }
        self.screen = screen;
        self.ppp = scale;
    }

    /// Follows the window's light or dark mode.
    pub fn set_dark(&self, ctx: &Context, dark: bool) {
        ctx.set_visuals(if dark { egui::Visuals::dark() } else { egui::Visuals::light() });
    }

    pub fn is_open(&self) -> bool {
        !self.levels.is_empty()
    }

    pub fn close(&mut self) {
        if self.is_open() {
            self.dirty = true;
        }
        self.levels.clear();
        self.title = None;
    }

    /// Shows `items` as a pop-up menu with its corner at `at` (points, window coordinates).
    pub fn open_popup(&mut self, items: Vec<MenuItemKind>, at: Pos2) {
        self.levels = vec![Level::new(items)];
        self.title = None;
        self.popup_at = at;
        self.dirty = true;
    }

    /// Like `open_popup`, at the last place the pointer was seen in the window.
    pub fn open_popup_at_pointer(&mut self, items: Vec<MenuItemKind>) {
        let at = self.pointer.unwrap_or(Pos2::ZERO);
        self.open_popup(items, at);
    }

    /// True once after something changed that the window should draw.
    pub fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }

    /// The frame the last `paint` made, for the app to tessellate and draw.
    pub fn take_output(&mut self) -> Option<egui::FullOutput> {
        self.out.take()
    }

    fn in_bar(&self, p: Pos2) -> bool {
        p.y >= 0.0 && p.y < BAR_PT && !self.bar_items.is_empty()
    }

    fn open_title(&mut self, bar_index: usize) {
        if let Some(MenuItemKind::Submenu(s)) = self.bar_items.get(bar_index) {
            self.levels = vec![Level::new(s.items())];
            self.title = Some(bar_index);
            self.dirty = true;
        }
    }

    /// The bar's submenu entries, as indexes into `bar_items`.
    fn title_indexes(&self) -> Vec<usize> {
        self.bar_items.iter().enumerate().filter(|(_, k)| matches!(k, MenuItemKind::Submenu(_))).map(|(i, _)| i).collect()
    }

    // ---- window events ----

    pub fn event(&mut self, _ctx: &Context, e: &WindowEvent) -> Consumed {
        match e {
            WindowEvent::CursorMoved { position, .. } => {
                let p = pos2(position.x as f32 / self.ppp, position.y as f32 / self.ppp);
                self.pointer = Some(p);
                self.hover(p);
                // Only a drag of the pointer through an open menu is ours; the strip alone does
                // not hold the pointer back from the terminal's drags.
                if self.is_open() { Consumed::Yes } else { Consumed::No }
            }
            WindowEvent::CursorLeft { .. } => {
                self.pointer = None;
                if self.hover_title.take().is_some() {
                    self.dirty = true;
                }
                Consumed::No
            }
            WindowEvent::MouseInput { state, button, .. } => self.mouse(*state, *button),
            WindowEvent::MouseWheel { delta, .. } => {
                if !self.is_open() {
                    return Consumed::No;
                }
                let dy = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y * ROW_PT * 3.0,
                    MouseScrollDelta::PixelDelta(p) => p.y as f32 / self.ppp,
                };
                if let Some(p) = self.pointer
                    && let Some(i) = self.frame.levels.iter().rposition(|l| l.rect.contains(p))
                    && let Some(level) = self.levels.get_mut(i)
                {
                    level.scroll = (level.scroll - dy).max(0.0);
                    self.dirty = true;
                }
                Consumed::Yes
            }
            // Leaving the window, or a new size under an open menu, ends it.
            WindowEvent::Focused(false) | WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => {
                self.close();
                Consumed::No
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if !self.is_open() || is_modifier(&event.logical_key) {
                    return Consumed::No;
                }
                let nav = nav_key(&event.logical_key);
                if event.state == ElementState::Released {
                    return if nav.is_some() { Consumed::Yes } else { Consumed::No };
                }
                match nav {
                    Some(k) => self.key(k),
                    // A key for the terminal ends the menu and goes on to it.
                    None => {
                        self.close();
                        Consumed::No
                    }
                }
            }
            _ => Consumed::No,
        }
    }

    fn hover(&mut self, p: Pos2) {
        let before = self.hover_title;
        self.hover_title = self.frame.titles.iter().find(|(r, _)| r.contains(p)).map(|(_, i)| *i);
        if before != self.hover_title {
            self.dirty = true;
        }
        if !self.is_open() {
            return;
        }
        // Another title while one is open swaps the dropdown, as a native bar does.
        if self.title.is_some()
            && let Some(t) = self.hover_title
            && self.title != Some(t)
        {
            self.open_title(t);
            return;
        }
        let Some(l) = self.frame.levels.iter().rposition(|l| l.rect.contains(p)) else { return };
        let Some(&(_, idx)) = self.frame.levels[l].rows.iter().find(|(r, _)| r.contains(p)) else { return };
        let rows = self.levels[l].rows();
        if !matches!(rows.get(idx), Some(Row::Item { .. })) {
            return;
        }
        if self.levels[l].hot == Some(idx) && self.levels.len() == l + 1 + usize::from(self.sub_open_at(l)) {
            return;
        }
        self.levels[l].hot = Some(idx);
        self.levels.truncate(l + 1);
        if rows[idx].live() && rows[idx].is_sub() {
            self.open_sub(l);
        }
        self.dirty = true;
    }

    fn sub_open_at(&self, l: usize) -> bool {
        self.levels[l].hot.is_some_and(|h| self.levels[l].rows().get(h).is_some_and(|r| r.live() && r.is_sub()))
    }

    /// Opens the submenu on the hot row of level `l`.
    fn open_sub(&mut self, l: usize) {
        let Some(h) = self.levels[l].hot else { return };
        if let Some(MenuItemKind::Submenu(s)) = self.levels[l].items.get(h) {
            let items = s.items();
            self.levels.truncate(l + 1);
            self.levels.push(Level::new(items));
            self.dirty = true;
        }
    }

    fn mouse(&mut self, state: ElementState, button: MouseButton) -> Consumed {
        let Some(p) = self.pointer else { return Consumed::No };
        match state {
            ElementState::Pressed => {
                if !self.is_open() {
                    if !self.in_bar(p) {
                        return Consumed::No;
                    }
                    self.swallow_release = true;
                    if button == MouseButton::Left
                        && let Some(&(_, t)) = self.frame.titles.iter().find(|(r, _)| r.contains(p))
                    {
                        self.open_title(t);
                    }
                    return Consumed::Yes;
                }
                // Open: every press is ours, and one outside ends the menu and stays with it.
                self.swallow_release = true;
                if button != MouseButton::Left {
                    return Consumed::Yes;
                }
                if let Some(&(_, t)) = self.frame.titles.iter().find(|(r, _)| r.contains(p)) {
                    if self.title == Some(t) {
                        self.close();
                    } else {
                        self.open_title(t);
                    }
                    return Consumed::Yes;
                }
                if let Some(l) = self.frame.levels.iter().rposition(|l| l.rect.contains(p)) {
                    if let Some(&(_, idx)) = self.frame.levels[l].rows.iter().find(|(r, _)| r.contains(p)) {
                        let rows = self.levels[l].rows();
                        if rows.get(idx).is_some_and(|r| r.live() && r.is_sub()) {
                            self.levels[l].hot = Some(idx);
                            self.open_sub(l);
                        }
                    }
                    return Consumed::Yes;
                }
                self.close();
                Consumed::Yes
            }
            ElementState::Released => {
                let swallowed = std::mem::take(&mut self.swallow_release);
                if !self.is_open() {
                    return if swallowed { Consumed::Yes } else { Consumed::No };
                }
                if button == MouseButton::Left
                    && let Some(l) = self.frame.levels.iter().rposition(|l| l.rect.contains(p))
                    && let Some(&(_, idx)) = self.frame.levels[l].rows.iter().find(|(r, _)| r.contains(p))
                {
                    self.choose(l, idx);
                }
                Consumed::Yes
            }
        }
    }

    /// Picks row `idx` of level `l` if it is an enabled item.
    fn choose(&mut self, l: usize, idx: usize) {
        if let Some(Row::Item { id, enabled: true, sub: false, .. }) = self.levels[l].rows().into_iter().nth(idx) {
            self.chosen = Some(id);
            self.close();
        }
    }

    // ---- keys ----

    /// A navigation key for an open menu (the real keys come through `event`).
    pub fn key(&mut self, k: egui::Key) -> Consumed {
        use egui::Key::*;
        if !self.is_open() {
            return Consumed::No;
        }
        let l = self.levels.len() - 1;
        self.dirty = true;
        match k {
            ArrowDown => self.step(l, true),
            ArrowUp => self.step(l, false),
            Home => self.edge(l, true),
            End => self.edge(l, false),
            Enter | Space => {
                if let Some(h) = self.levels[l].hot {
                    let rows = self.levels[l].rows();
                    match rows.get(h) {
                        Some(r) if r.live() && r.is_sub() => {
                            self.open_sub(l);
                            self.first_live(l + 1);
                        }
                        Some(r) if r.live() => self.choose(l, h),
                        _ => {}
                    }
                }
            }
            ArrowRight => {
                let on_sub = self.levels[l].hot.is_some_and(|h| self.levels[l].rows().get(h).is_some_and(|r| r.live() && r.is_sub()));
                if on_sub {
                    self.open_sub(l);
                    self.first_live(l + 1);
                } else {
                    self.next_title(true);
                }
            }
            ArrowLeft => {
                if self.levels.len() > 1 {
                    self.levels.pop();
                } else {
                    self.next_title(false);
                }
            }
            Escape => {
                if self.levels.len() > 1 {
                    self.levels.pop();
                } else {
                    self.close();
                }
            }
            _ => {}
        }
        Consumed::Yes
    }

    fn step(&mut self, l: usize, down: bool) {
        let rows = self.levels[l].rows();
        let n = rows.len();
        if n == 0 {
            return;
        }
        let mut i = self.levels[l].hot;
        for _ in 0..n {
            let next = match (i, down) {
                (None, true) => 0,
                (None, false) => n - 1,
                (Some(c), true) => (c + 1) % n,
                (Some(c), false) => (c + n - 1) % n,
            };
            i = Some(next);
            if rows[next].live() {
                self.levels[l].hot = Some(next);
                self.levels[l].reveal = true;
                self.levels.truncate(l + 1);
                return;
            }
        }
    }

    fn edge(&mut self, l: usize, first: bool) {
        self.levels[l].hot = None;
        self.step(l, first);
    }

    fn first_live(&mut self, l: usize) {
        if l < self.levels.len() {
            self.levels[l].hot = None;
            self.step(l, true);
        }
    }

    fn next_title(&mut self, forward: bool) {
        let Some(cur) = self.title else { return };
        let all = self.title_indexes();
        let Some(pos) = all.iter().position(|t| *t == cur) else { return };
        let n = all.len();
        let next = if forward { all[(pos + 1) % n] } else { all[(pos + n - 1) % n] };
        self.open_title(next);
    }

    // ---- painting ----

    /// Lays out and draws one frame: the bar when `bar` is given, and any open dropdowns. Returns
    /// the id of the item chosen since the last frame. The shapes wait in `take_output`.
    pub fn paint(&mut self, ctx: &Context, bar: Option<&muda::Menu>) -> Option<String> {
        self.bar_items = bar.map(|b| b.items()).unwrap_or_default();
        if bar.is_none() && self.title.is_some() {
            self.close();
        }
        ctx.set_pixels_per_point(self.ppp);
        let input = egui::RawInput {
            time: Some(self.started.elapsed().as_secs_f64()),
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, self.screen)),
            viewport_id: egui::ViewportId::ROOT,
            ..Default::default()
        };
        let mut frame = Frame::default();
        let mut out = ctx.run_ui(input, |ui| self.draw(ui.ctx(), &mut frame, bar.is_some()));
        // The fonts' texture changes ride in the output: a frame nobody drew first hands its
        // changes to this one, or the glyphs never reach the screen.
        if let Some(mut earlier) = self.out.take() {
            earlier.textures_delta.append(std::mem::take(&mut out.textures_delta));
            out.textures_delta = earlier.textures_delta;
        }
        self.frame = frame;
        self.out = Some(out);
        self.chosen.take()
    }

    fn draw(&mut self, ctx: &Context, frame: &mut Frame, with_bar: bool) {
        let visuals = ctx.global_style().visuals.clone();
        let text_color = visuals.text_color();
        let weak = visuals.weak_text_color();
        let font = FontId::proportional(TEXT_PT);
        let bar_layer = ctx.layer_painter(LayerId::new(Order::Foreground, Id::new("termurion-menu-bar")));
        let pop = ctx.layer_painter(LayerId::new(Order::Tooltip, Id::new("termurion-menu-drop")));
        let screen = self.screen;
        // The bar.
        if with_bar {
            let bar_rect = Rect::from_min_size(Pos2::ZERO, vec2(screen.x, BAR_PT));
            bar_layer.rect_filled(bar_rect, 0.0, visuals.panel_fill);
            bar_layer.line_segment([pos2(0.0, BAR_PT - 0.5), pos2(screen.x, BAR_PT - 0.5)], Stroke::new(1.0, visuals.window_stroke.color));
            let mut x = 2.0;
            for idx in self.title_indexes() {
                let Row::Item { text, enabled, .. } = Row::of(&self.bar_items[idx]) else { continue };
                let galley = bar_layer.layout_no_wrap(text, font.clone(), if enabled { text_color } else { weak });
                let w = galley.size().x + TITLE_PAD * 2.0;
                let rect = Rect::from_min_size(pos2(x, 0.0), vec2(w, BAR_PT - 1.0));
                if self.title == Some(idx) {
                    bar_layer.rect_filled(rect, 3.0, visuals.selection.bg_fill);
                } else if self.hover_title == Some(idx) && !self.is_open() {
                    bar_layer.rect_filled(rect, 3.0, visuals.widgets.hovered.weak_bg_fill);
                }
                bar_layer.galley(pos2(x + TITLE_PAD, (BAR_PT - galley.size().y) / 2.0 - 0.5), galley, text_color);
                frame.titles.push((rect, idx));
                x += w;
            }
        }
        // The dropdowns, each from the one before it.
        let mut parent: Option<Rect> = None;
        for l in 0..self.levels.len() {
            let rows = self.levels[l].rows();
            let any_sub = rows.iter().any(Row::is_sub);
            // Column widths from the text.
            let (mut label_w, mut short_w) = (0.0f32, 0.0f32);
            let mut cells: Vec<Option<(std::sync::Arc<egui::Galley>, Option<std::sync::Arc<egui::Galley>>)>> = Vec::new();
            for r in &rows {
                match r {
                    Row::Sep => cells.push(None),
                    Row::Item { text, shortcut, enabled, .. } => {
                        let color = if *enabled { text_color } else { weak };
                        let g = pop.layout_no_wrap(text.clone(), font.clone(), color);
                        let s = (!shortcut.is_empty()).then(|| pop.layout_no_wrap(shortcut.clone(), font.clone(), weak));
                        label_w = label_w.max(g.size().x);
                        short_w = short_w.max(s.as_ref().map_or(0.0, |s| s.size().x));
                        cells.push(Some((g, s)));
                    }
                }
            }
            let width = EDGE + PAD + CHECK_PT + label_w + if short_w > 0.0 { GAP + short_w } else { 0.0 } + if any_sub { ARROW_PT } else { 0.0 } + PAD;
            let total: f32 = rows.iter().map(|r| if matches!(r, Row::Sep) { SEP_PT } else { ROW_PT }).sum::<f32>() + EDGE * 2.0;
            // Where it goes.
            let (mut x, mut y) = match (l, parent) {
                (0, _) => match self.title.and_then(|t| frame.titles.iter().find(|(_, i)| *i == t)) {
                    Some((r, _)) => (r.min.x, BAR_PT - 1.0),
                    None => (self.popup_at.x, self.popup_at.y),
                },
                (_, Some(p)) => (p.max.x - 2.0, p.min.y - EDGE),
                _ => (0.0, 0.0),
            };
            let top_limit = if with_bar { BAR_PT } else { 0.0 };
            let height = total.min((screen.y - top_limit - 2.0).max(ROW_PT));
            if x + width > screen.x {
                x = if l == 0 { (screen.x - width).max(0.0) } else { (parent.map_or(0.0, |p| p.min.x) - width + 2.0).max(0.0) };
            }
            if y + height > screen.y {
                y = (screen.y - height).max(top_limit);
            }
            let rect = Rect::from_min_size(pos2(x, y), vec2(width, height));
            let max_scroll = (total - height).max(0.0);
            // Keyboard selection scrolls into view.
            if self.levels[l].reveal {
                self.levels[l].reveal = false;
                if let Some(h) = self.levels[l].hot {
                    let top: f32 = rows[..h].iter().map(|r| if matches!(r, Row::Sep) { SEP_PT } else { ROW_PT }).sum::<f32>() + EDGE;
                    let sc = &mut self.levels[l].scroll;
                    if top < *sc + EDGE {
                        *sc = (top - EDGE).max(0.0);
                    } else if top + ROW_PT > *sc + height - EDGE {
                        *sc = top + ROW_PT - height + EDGE;
                    }
                }
            }
            let scroll = self.levels[l].scroll.clamp(0.0, max_scroll);
            self.levels[l].scroll = scroll;
            let clip = pop.with_clip_rect(rect);
            // Shadow, body and edge.
            pop.rect_filled(rect.translate(vec2(2.0, 3.0)), 4.0, Color32::from_black_alpha(60));
            pop.rect_filled(rect, 4.0, visuals.window_fill);
            pop.rect_stroke(rect, 4.0, Stroke::new(1.0, visuals.window_stroke.color), egui::StrokeKind::Inside);
            let mut lf = LevelFrame { rect, rows: Vec::new() };
            let mut cy = y + EDGE - scroll;
            let hot = self.levels[l].hot;
            for (i, (row, cell)) in rows.iter().zip(cells).enumerate() {
                let h = if matches!(row, Row::Sep) { SEP_PT } else { ROW_PT };
                let rr = Rect::from_min_size(pos2(x + EDGE / 2.0, cy), vec2(width - EDGE, h));
                cy += h;
                if rr.max.y <= rect.min.y || rr.min.y >= rect.max.y {
                    continue;
                }
                match (row, cell) {
                    (Row::Sep, _) => {
                        let m = rr.center().y;
                        clip.line_segment([pos2(rr.min.x + PAD / 2.0, m), pos2(rr.max.x - PAD / 2.0, m)], Stroke::new(1.0, visuals.window_stroke.color));
                    }
                    (Row::Item { checked, enabled, sub, .. }, Some((label, short))) => {
                        let lit = hot == Some(i) && *enabled;
                        if lit {
                            clip.rect_filled(rr, 3.0, visuals.selection.bg_fill);
                        }
                        let ink = if lit { visuals.selection.stroke.color } else if *enabled { text_color } else { weak };
                        if *checked == Some(true) {
                            clip.text(pos2(rr.min.x + PAD + (CHECK_PT - 6.0) / 2.0, rr.center().y), Align2::CENTER_CENTER, "\u{2713}", font.clone(), ink);
                        }
                        let ly = rr.center().y - label.size().y / 2.0;
                        clip.galley_with_override_text_color(pos2(rr.min.x + PAD + CHECK_PT, ly), label, ink);
                        if let Some(s) = short {
                            let right = rr.max.x - PAD - if any_sub { ARROW_PT } else { 0.0 };
                            clip.galley_with_override_text_color(pos2(right - s.size().x, rr.center().y - s.size().y / 2.0), s, if lit { ink } else { weak });
                        }
                        if *sub {
                            let c = pos2(rr.max.x - PAD, rr.center().y);
                            clip.add(Shape::convex_polygon(vec![c + vec2(-3.0, -4.0), c + vec2(3.0, 0.0), c + vec2(-3.0, 4.0)], ink, Stroke::NONE));
                        }
                        let vis = rr.intersect(rect);
                        if vis.height() > 0.0 {
                            lf.rows.push((vis, i));
                        }
                    }
                    _ => {}
                }
            }
            parent = hot.and_then(|h| lf.rows.iter().find(|(_, i)| *i == h).map(|(r, _)| *r)).filter(|_| l + 1 < self.levels.len());
            frame.levels.push(lf);
        }
    }

    // ---- test windows into the last frame ----

    /// The title of the bar with this text, in points.
    #[cfg(test)]
    pub(crate) fn title_rect(&self, text: &str) -> Option<Rect> {
        self.frame.titles.iter().find(|(_, i)| matches!(Row::of(&self.bar_items[*i]), Row::Item { text: ref t, .. } if t == text)).map(|(r, _)| *r)
    }

    /// The visible row with this text at open level `level`, in points.
    #[cfg(test)]
    pub(crate) fn row_rect(&self, level: usize, text: &str) -> Option<Rect> {
        let lf = self.frame.levels.get(level)?;
        let rows = self.levels.get(level)?.rows();
        lf.rows.iter().find(|(_, i)| matches!(&rows[*i], Row::Item { text: t, .. } if t == text)).map(|(r, _)| *r)
    }

    #[cfg(test)]
    pub(crate) fn hot_text(&self, level: usize) -> Option<String> {
        let l = self.levels.get(level)?;
        match l.rows().get(l.hot?)? {
            Row::Item { text, .. } => Some(text.clone()),
            Row::Sep => None,
        }
    }
}

fn is_modifier(k: &Key) -> bool {
    matches!(k, Key::Named(NamedKey::Shift | NamedKey::Control | NamedKey::Alt | NamedKey::Super | NamedKey::Meta | NamedKey::AltGraph | NamedKey::CapsLock | NamedKey::NumLock))
}

/// The keys an open menu works with.
fn nav_key(k: &Key) -> Option<egui::Key> {
    Some(match k {
        Key::Named(NamedKey::ArrowDown) => egui::Key::ArrowDown,
        Key::Named(NamedKey::ArrowUp) => egui::Key::ArrowUp,
        Key::Named(NamedKey::ArrowLeft) => egui::Key::ArrowLeft,
        Key::Named(NamedKey::ArrowRight) => egui::Key::ArrowRight,
        Key::Named(NamedKey::Enter) => egui::Key::Enter,
        Key::Named(NamedKey::Space) => egui::Key::Space,
        Key::Named(NamedKey::Escape) => egui::Key::Escape,
        Key::Named(NamedKey::Home) => egui::Key::Home,
        Key::Named(NamedKey::End) => egui::Key::End,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    // The muda menus are the main thread's on macOS, where the overlay is not used and a test
    // thread cannot build one.
    #![cfg(not(target_os = "macos"))]
    use super::*;
    use muda::{CheckMenuItem, Menu, MenuItem, Submenu};
    use winit::dpi::PhysicalPosition;
    use winit::event::DeviceId;

    struct Rig {
        ctx: Context,
        o: Overlay,
        bar: Menu,
    }

    fn device() -> DeviceId {
        DeviceId::dummy()
    }

    impl Rig {
        fn new(bar: Menu) -> Rig {
            let mut o = Overlay::new();
            o.set_screen(800.0, 600.0, 1.0);
            let mut r = Rig { ctx: Context::default(), o, bar };
            r.frame();
            r
        }

        fn frame(&mut self) -> Option<String> {
            // Two passes, as the first of a context is egui's own warm-up.
            let first = self.o.paint(&self.ctx, Some(&self.bar));
            let second = self.o.paint(&self.ctx, Some(&self.bar));
            first.or(second)
        }

        fn mv(&mut self, p: Pos2) {
            self.o.event(&self.ctx, &WindowEvent::CursorMoved { device_id: device(), position: PhysicalPosition::new(p.x as f64, p.y as f64) });
        }

        fn button(&mut self, state: ElementState) -> Consumed {
            self.o.event(&self.ctx, &WindowEvent::MouseInput { device_id: device(), state, button: MouseButton::Left })
        }

        /// Press and release at `p`, painting after each so the layout is current.
        fn click(&mut self, p: Pos2) -> Option<String> {
            self.mv(p);
            self.button(ElementState::Pressed);
            let a = self.frame();
            self.button(ElementState::Released);
            let b = self.frame();
            a.or(b)
        }
    }

    fn shell_menu() -> (Menu, MenuItem) {
        let bar = Menu::new();
        let shell = Submenu::new("Shell", true);
        let new_tab = MenuItem::with_id("Shell > New Tab", "New Tab\tCtrl+Shift+T", true, None);
        shell.append(&new_tab).unwrap();
        shell.append(&MenuItem::with_id("Shell > Close", "Close", true, None)).unwrap();
        bar.append(&shell).unwrap();
        bar.append(&Submenu::new("View", true)).unwrap();
        (bar, new_tab)
    }

    fn center(r: Rect) -> Pos2 {
        r.center()
    }

    #[test]
    fn a_click_on_an_item_returns_its_id() {
        let (bar, _) = shell_menu();
        let mut rig = Rig::new(bar);
        let shell = rig.o.title_rect("Shell").expect("the Shell title is laid out");
        assert_eq!(rig.click(center(shell)), None, "opening a menu chooses nothing");
        assert!(rig.o.is_open());
        let row = rig.o.row_rect(0, "New Tab").expect("the dropdown shows New Tab");
        assert_eq!(rig.click(center(row)).as_deref(), Some("Shell > New Tab"));
        assert!(!rig.o.is_open(), "a choice closes the menu");
    }

    #[test]
    fn disabled_items_are_not_chosen() {
        let (bar, new_tab) = shell_menu();
        new_tab.set_enabled(false);
        let mut rig = Rig::new(bar);
        let shell = rig.o.title_rect("Shell").unwrap();
        rig.click(center(shell));
        let row = rig.o.row_rect(0, "New Tab").unwrap();
        assert_eq!(rig.click(center(row)), None);
        assert!(rig.o.is_open(), "the click on a dead row leaves the menu up");
    }

    #[test]
    fn a_checked_item_shows_checked() {
        let bar = Menu::new();
        let view = Submenu::new("View", true);
        let check = CheckMenuItem::with_id("View > Status Bar", "Status Bar", true, false, None);
        view.append(&check).unwrap();
        bar.append(&view).unwrap();
        let mut rig = Rig::new(bar);
        let title = rig.o.title_rect("View").unwrap();
        rig.click(center(title));
        let marks = |rig: &mut Rig| -> usize {
            rig.frame();
            let out = rig.o.out.as_ref().unwrap();
            out.shapes.iter().filter(|s| matches!(&s.shape, Shape::Text(t) if t.galley.job.text == "\u{2713}")).count()
        };
        assert_eq!(marks(&mut rig), 0, "unchecked: no mark");
        check.set_checked(true);
        assert_eq!(marks(&mut rig), 1, "a setter shows at the next frame");
    }

    #[test]
    fn keys_work_an_open_menu() {
        let bar = Menu::new();
        let shell = Submenu::new("Shell", true);
        shell.append(&MenuItem::with_id("a", "Alpha", true, None)).unwrap();
        shell.append(&MenuItem::with_id("dead", "Dead", false, None)).unwrap();
        shell.append(&MenuItem::with_id("b", "Beta", true, None)).unwrap();
        shell.append(&MenuItem::with_id("c", "Gamma", true, None)).unwrap();
        bar.append(&shell).unwrap();
        let edit = Submenu::new("Edit", true);
        edit.append(&MenuItem::with_id("e", "Copy", true, None)).unwrap();
        bar.append(&edit).unwrap();
        let mut rig = Rig::new(bar);
        let t = rig.o.title_rect("Shell").unwrap();
        rig.click(center(t));
        rig.o.key(egui::Key::ArrowDown);
        rig.o.key(egui::Key::ArrowDown);
        assert_eq!(rig.o.hot_text(0).as_deref(), Some("Beta"), "a disabled row is stepped over");
        assert_eq!(rig.o.key(egui::Key::Enter), Consumed::Yes);
        assert_eq!(rig.frame().as_deref(), Some("b"), "Enter chooses the second enabled item");
        // Right moves to the next menu, Esc closes.
        rig.click(center(t));
        rig.frame();
        rig.o.key(egui::Key::ArrowRight);
        rig.frame();
        assert!(rig.o.row_rect(0, "Copy").is_some(), "Right opened the next menu");
        rig.o.key(egui::Key::ArrowLeft);
        rig.frame();
        assert!(rig.o.row_rect(0, "Alpha").is_some(), "Left went back");
        rig.o.key(egui::Key::Escape);
        assert!(!rig.o.is_open());
    }

    #[test]
    fn a_click_outside_an_open_menu_closes_it_and_stays_with_the_menu() {
        let (bar, _) = shell_menu();
        let mut rig = Rig::new(bar);
        let t = rig.o.title_rect("Shell").unwrap();
        rig.click(center(t));
        assert!(rig.o.is_open());
        rig.mv(pos2(700.0, 500.0));
        assert_eq!(rig.button(ElementState::Pressed), Consumed::Yes, "the press stays with the menu");
        assert!(!rig.o.is_open());
        assert_eq!(rig.button(ElementState::Released), Consumed::Yes, "so does its release: no selection starts");
        assert_eq!(rig.button(ElementState::Pressed), Consumed::No, "the next click is the terminal's");
        // Focus loss and a resize close it too.
        rig.button(ElementState::Released);
        rig.click(center(t));
        assert!(rig.o.is_open());
        rig.o.event(&rig.ctx, &WindowEvent::Focused(false));
        assert!(!rig.o.is_open());
        rig.click(center(t));
        rig.o.set_screen(900.0, 600.0, 1.0);
        assert!(!rig.o.is_open(), "a new size closes it");
    }

    #[test]
    #[cfg(unix)] // test: Win32 menus hand an item's text back without the tab part
    fn the_shortcut_is_right_aligned_after_the_tab() {
        let bar = Menu::new();
        let shell = Submenu::new("Shell", true);
        shell.append(&MenuItem::with_id("t", "New Tab\tCtrl+Shift+T", true, None)).unwrap();
        bar.append(&shell).unwrap();
        let mut rig = Rig::new(bar);
        let t = rig.o.title_rect("Shell").unwrap();
        rig.click(center(t));
        rig.frame();
        let row = rig.o.row_rect(0, "New Tab").expect("the label is the part before the tab");
        let out = rig.o.out.as_ref().unwrap();
        let text = |s: &str| out.shapes.iter().find_map(|c| match &c.shape {
            Shape::Text(t) if t.galley.job.text == s => Some((t.pos, t.galley.size())),
            _ => None,
        });
        let (pos, size) = text("Ctrl+Shift+T").expect("the chord is its own galley");
        assert!(text("New Tab").is_some());
        let right = pos.x + size.x;
        assert!((right - (row.max.x - PAD)).abs() < 0.6, "right edge {right} vs row {row:?}");
    }
}
