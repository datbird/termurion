//! The status bar in the app: where the bars go (one per window, the current session's,
//! or one per pane with "Separate status bars per pane"; always one per window in a tmux
//! tab), the room they take from the terminal, laying out and drawing each bar, clicks,
//! and the right-click menu (Copy, Configure/Hide component, Configure Status Bar,
//! Disable Status Bar, checkbox knobs).
//!
//! Citations use `.superpowers/research/m9-facts.md` section 1's abbreviations (`VC` =
//! `iTermStatusBarViewController.m`, `CV` = `iTermStatusBarContainerView.m`, `SV` =
//! `iTermStatusBarView.m`, `PS` = `PTYSession.m`).

use super::*;
use crate::layout::visible_rects;
use crate::palette::Rgb;
use crate::statusbar::{self, Algorithm, Component, KnobKind, Layout, Rainbow, PLACEHOLDER_CLASS, SPACER_CLASS, SPRING_CLASS};
use crate::statusbar_layout::{self as sbl, Item, ItemKind, Placed, Position};

/// `iTermStatusBarPlaceholderComponent`'s text.
pub(super) const PLACEHOLDER_TEXT: &str = "Click here to configure status bar";
const LAYOUT_KEY: &str = "Status Bar Layout";
const SHOW_KEY: &str = "Show Status Bar";
/// `iTermStatusBarFixedSpacerComponentWidthKnob`.
const SPACER_WIDTH_KEY: &str = "iTermStatusBarFixedSpacerComponentWidthKnob";
/// The status bar font's size when neither the layout nor the component sets one
/// (`[NSFont systemFontOfSize:12]`).
const DEFAULT_FONT_PT: f32 = 12.0;

/// What one on-screen component is and does. Milestone 9h's content components add their
/// own variants here: `text` then returns what they show now, `copyable` their Copy
/// string, and `click` what a left click does; `item` sizes them from their text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Piece {
    /// `iTermStatusBarSpringComponent`: empty space that draws only a background color.
    Spring,
    /// `iTermStatusBarFixedSpacerComponent`: a fixed amount of space.
    Spacer,
    /// `iTermStatusBarPlaceholderComponent`, shown alone when a layout has no component
    /// Tandem knows.
    Placeholder,
}

/// What a left click on a component does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Click {
    None,
    /// Opens the profile's status bar settings (the placeholder's click).
    Configure,
}

impl Piece {
    /// The piece for a layout component's class; None for a class Tandem does not show
    /// (the 9h classes, or one it does not know): kept in the profile, skipped on screen.
    pub(super) fn of(class: &str) -> Option<Piece> {
        match class {
            SPRING_CLASS => Some(Piece::Spring),
            SPACER_CLASS => Some(Piece::Spacer),
            _ => None,
        }
    }

    pub(super) fn text(self) -> Option<String> {
        match self {
            Piece::Placeholder => Some(PLACEHOLDER_TEXT.to_string()),
            Piece::Spring | Piece::Spacer => None,
        }
    }

    /// `statusBarComponentCopyableString`: none of the framework's own components has one.
    pub(super) fn copyable(self) -> Option<String> {
        None
    }

    pub(super) fn click(self) -> Click {
        match self {
            Piece::Placeholder => Click::Configure,
            Piece::Spring | Piece::Spacer => Click::None,
        }
    }

    /// The layout algorithms' sizing facts, in points. `text_pt` is the width of the
    /// piece's text. Minimum and maximum follow `BLA:261-283`: the larger of the
    /// component's own minimum and its "minwidth" knob; the maximum is
    /// `statusBarComponentMaximumWidth` (`MAX(24, "maxwidth" ?: INFINITY)`).
    fn item(self, c: &Component, algorithm: Algorithm, text_pt: f32) -> Item {
        let knob_min = c.min_width_knob() as f32;
        let max = c.max_width_knob() as f32;
        let spring = c.spring_constant(algorithm) as f32;
        let priority = c.priority() as f32;
        let base = Item { min: 0.0, max, preferred: 0.0, stretch: false, spring, margins: true, priority, kind: ItemKind::Normal, icon: false, empty: false, mandatory: false };
        match self {
            // SPR: minimum 0, preferred infinity, stretches, no margins.
            Piece::Spring => Item { min: knob_min.max(0.0), preferred: f32::INFINITY, max: f32::INFINITY, stretch: true, margins: false, kind: ItemKind::Spring, ..base },
            // FSP: its width knob, 5 when absent or 0.
            Piece::Spacer => {
                let w = spacer_width(c);
                Item { min: w.max(knob_min), preferred: w, kind: ItemKind::FixedSpacer, ..base }
            }
            // A one-variant text component: its text's width, stretchable.
            Piece::Placeholder => Item { min: text_pt.max(knob_min), preferred: text_pt, stretch: true, ..base },
        }
    }
}

fn knob_number(c: &Component, key: &str) -> Option<f64> {
    c.knobs.get(key).and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64)))
}

/// `iTermStatusBarFixedSpacerComponent`: `knob ?: 5`, so 0 also means 5.
fn spacer_width(c: &Component) -> f32 {
    match knob_number(c, SPACER_WIDTH_KEY) {
        Some(w) if w != 0.0 => w as f32,
        _ => 5.0,
    }
}

/// A `{r, g, b, a}` table as a color and its alpha.
fn color_of(v: &toml::Value) -> Option<(Rgb, f32)> {
    let t = v.as_table()?;
    let c = |k: &str| t.get(k).and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64)));
    let byte = |f: f64| (f.clamp(0.0, 1.0) * 255.0).round() as u8;
    Some((crate::palette::rgb(byte(c("r")?), byte(c("g")?), byte(c("b")?)), c("a").unwrap_or(1.0).clamp(0.0, 1.0) as f32))
}

/// `NSColor colorWithHue:saturation:brightness:alpha:`.
fn hsb(h: f32, s: f32, b: f32) -> Rgb {
    let h = (h.rem_euclid(1.0)) * 6.0;
    let i = h.floor();
    let f = h - i;
    let (p, q, t) = (b * (1.0 - s), b * (1.0 - s * f), b * (1.0 - s * (1.0 - f)));
    let (r, g, bl) = match i as i32 {
        0 => (b, t, p),
        1 => (q, b, p),
        2 => (p, b, t),
        3 => (p, q, b),
        4 => (t, p, b),
        _ => (b, p, q),
    };
    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    crate::palette::rgb(byte(r), byte(g), byte(bl))
}

fn is_dark(c: Rgb) -> bool {
    (0.299 * c.r as f32 + 0.587 * c.g as f32 + 0.114 * c.b as f32) / 255.0 < 0.5
}

/// A bar on screen: the session whose layout it shows, and where it is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Bar {
    pub(super) pane: PaneId,
    pub(super) rect: Rect,
}

/// One component a bar shows: where it is in the layout (None for the placeholder), what
/// it is, and its text at its font size (pixels, and as a scale of the terminal font).
struct Shown {
    index: Option<usize>,
    piece: Piece,
    component: Component,
    text: Option<String>,
    text_scale: f32,
}

/// A bar laid out, in points from the bar's left edge.
struct Placement {
    layout: Layout,
    shown: Vec<Shown>,
    items: Vec<Item>,
    placed: Vec<Placed>,
    width_pt: f32,
}

impl Placement {
    /// The visible component under x (points from the bar's left): a component's band
    /// covers its margins too.
    fn hit(&self, x_pt: f32) -> Option<usize> {
        sbl::bands(&self.placed, &self.items, self.width_pt).into_iter().find(|&(a, b, _)| x_pt >= a && x_pt < b).map(|(_, _, i)| i)
    }
}

/// The right-click menu's target, until its pick arrives.
pub(super) struct StatusMenu {
    pane: PaneId,
    class: String,
    index: Option<usize>,
    copy: Option<String>,
    /// Checkbox knobs' keys, in menu order.
    checkboxes: Vec<String>,
}

impl App {
    /// The session's profile has "Status bar enabled".
    fn shows_status_bar(&self, id: PaneId) -> bool {
        self.panes.get(&id).and_then(|p| self.store.get(&p.profile)).is_some_and(|p| p.bool(SHOW_KEY))
    }

    /// A bar's height in pixels: `statusBarHeight` points (21) at the window's scale.
    fn status_bar_height(&self) -> f32 {
        let pt = self.globals.get("statusBarHeight").and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64))).unwrap_or(21.0).max(1.0);
        (pt as f32 * self.scale()).round()
    }

    fn status_bar_position(&self) -> Position {
        Position::from_code(self.globals.int("StatusBarPosition"))
    }

    /// Bars per pane: "Separate status bars per pane", except in a tmux tab, whose bar
    /// is always the window's (`PTYTab.m:600-608`).
    fn per_pane_bars(&self, tab: &Tab) -> bool {
        self.globals.bool("SeparateStatusBarsPerPane") && !self.panes.get(&tab.focused).is_some_and(|p| p.tmux_io().is_some())
    }

    /// The session whose bar the window shows across the tab: the current session, when
    /// its profile shows one and bars are not per pane.
    fn shared_bar_pane(&self, tab: &Tab) -> Option<PaneId> {
        (!self.per_pane_bars(tab) && self.shows_status_bar(tab.focused)).then_some(tab.focused)
    }

    /// The area a tab's panes share: the window's, less the shared bar. A tmux tab's size
    /// report reads this, so tmux is told the size without the bar.
    pub(super) fn content_area_for(&self, tab: &Tab) -> Rect {
        let area = self.window_area();
        if self.shared_bar_pane(tab).is_some() { sbl::bar_rects(area, self.status_bar_height(), self.status_bar_position()).1 } else { area }
    }

    /// A pane's rectangle less its own bar, and the bar, with bars per pane.
    pub(super) fn pane_bar_split(&self, tab: &Tab, id: PaneId, rect: Rect) -> (Rect, Option<Rect>) {
        if self.per_pane_bars(tab) && self.shows_status_bar(id) {
            let (bar, rest) = sbl::bar_rects(rect, self.status_bar_height(), self.status_bar_position());
            (rest, Some(bar))
        } else {
            (rect, None)
        }
    }

    /// The tab's bars.
    pub(super) fn status_bars(&self, tab: &Tab) -> Vec<Bar> {
        if let Some(pane) = self.shared_bar_pane(tab) {
            let rect = sbl::bar_rects(self.window_area(), self.status_bar_height(), self.status_bar_position()).0;
            return vec![Bar { pane, rect }];
        }
        if !self.per_pane_bars(tab) {
            return Vec::new();
        }
        visible_rects(&tab.layout, tab.zoomed, self.content_area_for(tab), GAP).into_iter().filter_map(|(id, r)| self.pane_bar_split(tab, id, r).1.map(|rect| Bar { pane: id, rect })).collect()
    }

    fn status_layout(&self, pane: PaneId) -> Layout {
        let v = self.profile_of(pane).and_then(|p| p.get(LAYOUT_KEY));
        Layout::from_value(v.as_ref())
    }

    /// The text size for a component: its "Custom Font" knob, else the layout's font,
    /// else 12 points; the family stays the prose font. A scale of the terminal font.
    fn status_text_scale(&self, layout: &Layout, c: &Component) -> f32 {
        let font = c.font().filter(|f| !f.trim().is_empty()).or_else(|| layout.advanced.font.clone());
        let pt = font.map_or(DEFAULT_FONT_PT, |f| crate::profile::parse_font(&f).size_pt.clamp(4.0, 128.0));
        let term_px = self.gpu.as_ref().map_or(16.0, |g| g.renderer.font_px()).max(1.0);
        pt * self.scale() / term_px
    }

    /// Lays out one bar (`VC:156-198`): the layout's components Tandem knows, or the
    /// placeholder when there are none (`VC:248-293`), by the layout's algorithm.
    fn place_bar(&mut self, bar: &Bar) -> Placement {
        let layout = self.status_layout(bar.pane);
        let algorithm = layout.advanced.algorithm;
        let mut shown: Vec<Shown> = Vec::new();
        for (i, c) in layout.components.iter().enumerate() {
            if let Some(piece) = Piece::of(&c.class) {
                shown.push(Shown { index: Some(i), piece, component: c.clone(), text: piece.text(), text_scale: self.status_text_scale(&layout, c) });
            }
        }
        if shown.is_empty() {
            let c = Component { class: PLACEHOLDER_CLASS.into(), knobs: toml::Table::new(), config: toml::Table::new() };
            let text_scale = self.status_text_scale(&layout, &c);
            shown.push(Shown { index: None, piece: Piece::Placeholder, component: c, text: Piece::Placeholder.text(), text_scale });
        }
        let scale = self.scale();
        let cell_w = self.cell().0;
        let mut items = Vec::with_capacity(shown.len());
        for s in &shown {
            let text_pt = match &s.text {
                Some(t) => {
                    let px = match self.gpu.as_mut() {
                        Some(g) => g.renderer.text_width(t, true, s.text_scale),
                        None => t.chars().count() as f32 * cell_w * s.text_scale,
                    };
                    (px / scale).ceil()
                }
                None => 0.0,
            };
            items.push(s.piece.item(&s.component, algorithm, text_pt));
        }
        let width_pt = bar.rect.w / scale;
        let placed = sbl::layout(&items, width_pt, algorithm, layout.advanced.remove_empty);
        Placement { layout, shown, items, placed, width_pt }
    }

    /// Draws the current tab's bars.
    pub(super) fn draw_status_bars(&mut self, frame: &mut scene::Frame, cell: (f32, f32)) {
        let Some(tab) = self.tabs.get(self.active) else { return };
        let bars = self.status_bars(tab);
        let position = self.status_bar_position();
        let scale = self.scale();
        for bar in bars {
            let p = self.place_bar(&bar);
            let adv = &p.layout.advanced;
            let background = adv.background_color.as_ref().and_then(color_of).unwrap_or((scene::STATUS_BAR_BG, 1.0));
            let default_text = adv.default_text_color.as_ref().and_then(color_of).map_or(scene::STATUS_BAR_TEXT, |(c, _)| c);
            // Auto-rainbow colors every visible component, springs and spacers included
            // (`VC:490-504`); Automatic picks by the bar's own background.
            let rainbow = if adv.rainbow == Rainbow::Disabled { Vec::new() } else { statusbar::rainbow_colors(p.placed.len(), adv.rainbow, is_dark(background.0)) };
            let px = |x: f32| bar.rect.x + (x * scale).round();
            let bands: Vec<(f32, f32, Rgb, f32)> = sbl::bands(&p.placed, &p.items, p.width_pt)
                .into_iter()
                .filter_map(|(a, b, i)| p.shown[i].component.background_color().as_ref().and_then(color_of).map(|(c, al)| (px(a), px(b), c, al)))
                .collect();
            let separators: Vec<f32> = sbl::separators(&p.placed, &p.items).into_iter().map(px).collect();
            // `PS:22983-23010`: the layout's separator color, else the default text color at
            // a quarter alpha.
            let separator_color = Some(adv.separator_color.as_ref().and_then(color_of).unwrap_or((scene::STATUS_BAR_TEXT, 0.25)));
            let mut texts = Vec::new();
            for (k, pl) in p.placed.iter().enumerate() {
                let s = &p.shown[pl.index];
                let Some(text) = s.text.clone() else { continue };
                let color = match rainbow.get(k) {
                    Some(&(h, sat, b)) => hsb(h, sat, b),
                    None => s.component.text_color().as_ref().and_then(color_of).map_or(default_text, |(c, _)| c),
                };
                let x = px(pl.x);
                let clip = Rect { x, y: bar.rect.y, w: (pl.width * scale).round(), h: bar.rect.h };
                texts.push(scene::BarText { x, text, color, scale: s.text_scale, clip });
            }
            // `SV:73-149`: one point on the edge that touches the terminal.
            let edge = adv.edge_separator.then(|| {
                let h = scale.round().max(1.0);
                let y = match position {
                    Position::Top => bar.rect.y + bar.rect.h - h,
                    Position::Bottom => bar.rect.y,
                };
                Rect { y, h, ..bar.rect }
            });
            let d = scene::StatusBarDraw { rect: bar.rect, background, bands: &bands, separators: &separators, separator_color, texts: &texts, edge, scale };
            scene::status_bar(frame, &d, cell);
        }
    }

    /// A press on a bar of the current tab. True when it was on one (the terminal is not
    /// under it). The bar's session becomes the focused one; a left click does the
    /// component's click, a right click opens the bar's menu.
    pub(super) fn status_press(&mut self, button: MouseButton, x: f32, y: f32, event_loop: &ActiveEventLoop) -> bool {
        let Some(tab) = self.tabs.get(self.active) else { return false };
        let Some(bar) = self.status_bars(tab).into_iter().find(|b| b.rect.contains(x, y)) else { return false };
        if let Some(t) = self.tabs.get_mut(self.active) {
            t.focused = bar.pane;
        }
        self.refresh_route();
        let p = self.place_bar(&bar);
        let hit = p.hit((x - bar.rect.x) / self.scale());
        match button {
            MouseButton::Left => {
                if hit.is_some_and(|i| p.shown[i].piece.click() == Click::Configure) {
                    self.configure_status_bar(bar.pane, event_loop);
                }
            }
            MouseButton::Right => self.show_status_menu(bar.pane, &p, hit),
            _ => {}
        }
        self.redraw();
        true
    }

    /// The component menu (`CV:310-388`).
    fn show_status_menu(&mut self, pane: PaneId, p: &Placement, hit: Option<usize>) {
        use muda::{CheckMenuItem, ContextMenu, Menu, MenuItem, PredefinedMenuItem};
        let Some(hwnd) = self.hwnd() else { return };
        let algorithm = p.layout.advanced.algorithm;
        let shown = hit.map(|i| &p.shown[i]);
        let class = shown.map_or(String::new(), |s| s.component.class.clone());
        let copy = shown.and_then(|s| s.piece.copyable()).filter(|t| !t.is_empty());
        let menu = Menu::new();
        let mut results = Vec::new();
        if copy.is_some() {
            results.push(menu.append(&MenuItem::with_id("statusbar:copy", "Copy", true, None)));
            results.push(menu.append(&PredefinedMenuItem::separator()));
        }
        let mut checkboxes = Vec::new();
        if let Some(s) = shown {
            if let Some(info) = statusbar::info(&s.component.class).filter(|i| !i.internal) {
                // The spring's Stable name (`SPR:42-49`).
                let short = if info.class == SPRING_CLASS && algorithm == Algorithm::Stable { "Empty Space" } else { info.short };
                let knobs = statusbar::knobs(&s.component.class, algorithm);
                if !knobs.is_empty() {
                    results.push(menu.append(&MenuItem::with_id("statusbar:configure", format!("Configure {short}").replace('&', "&&"), true, None)));
                }
                results.push(menu.append(&MenuItem::with_id("statusbar:hide", format!("Hide {short}").replace('&', "&&"), true, None)));
                checkboxes = knobs.into_iter().filter(|k| k.kind == KnobKind::Checkbox).collect();
            }
        }
        results.push(menu.append(&MenuItem::with_id("statusbar:configure-bar", "Configure Status Bar", true, None)));
        results.push(menu.append(&MenuItem::with_id("statusbar:disable", "Disable Status Bar", true, None)));
        if !checkboxes.is_empty() {
            results.push(menu.append(&PredefinedMenuItem::separator()));
            for (i, k) in checkboxes.iter().enumerate() {
                let on = shown.and_then(|s| s.component.knobs.get(&k.key)).or(k.default.as_ref()).and_then(toml::Value::as_bool).unwrap_or(false);
                results.push(menu.append(&CheckMenuItem::with_id(format!("statusbar:knob:{i}"), k.label.replace('&', "&&"), true, on, None)));
            }
        }
        if let Some(Err(e)) = results.into_iter().find(Result::is_err) {
            self.banners.set("menu", format!("Status bar menu: {e}"));
        }
        self.status_menu = Some(StatusMenu { pane, class, index: shown.and_then(|s| s.index), copy, checkboxes: checkboxes.into_iter().map(|k| k.key).collect() });
        unsafe { menu.show_context_menu_for_hwnd(hwnd, None) };
        self.popup_menu = Some(menu);
    }

    /// A pick from the bar's menu. True when the id was one of its items.
    pub(super) fn status_bar_picked(&mut self, id: &str, event_loop: &ActiveEventLoop) -> bool {
        let Some(what) = id.strip_prefix("statusbar:") else { return false };
        let Some(m) = self.status_menu.take() else { return true };
        match what {
            "copy" => {
                if let (Some(text), Some(cb)) = (m.copy, self.clipboard.as_mut()) {
                    if let Err(err) = cb.set_text(text) {
                        self.banners.set("clipboard", format!("Could not copy: {err}"));
                    }
                }
            }
            // Task 4's setup window opens from both; until then, the profile's Session page.
            "configure" | "configure-bar" => self.configure_status_bar(m.pane, event_loop),
            "hide" => self.edit_status_layout(m.pane, |l| statusbar::hide_class(l, &m.class)),
            "disable" => self.disable_status_bar(m.pane),
            k => {
                let Some(key) = k.strip_prefix("knob:").and_then(|i| i.parse::<usize>().ok()).and_then(|i| m.checkboxes.get(i).cloned()) else { return true };
                let Some(index) = m.index else { return true };
                self.edit_status_layout(m.pane, |l| {
                    if let Some(c) = l.components.get_mut(index) {
                        let on = c.knobs.get(&key).and_then(toml::Value::as_bool).unwrap_or(false);
                        c.knobs.insert(key, toml::Value::Boolean(!on));
                    }
                });
            }
        }
        true
    }

    /// Configure Status Bar and the placeholder's click: the Settings window at the
    /// session's profile, on its Session tab.
    pub(super) fn configure_status_bar(&mut self, pane: PaneId, event_loop: &ActiveEventLoop) {
        let Some(guid) = self.panes.get(&pane).map(|p| self.store.shared_guid(&p.profile)) else { return };
        self.open_settings_page(event_loop, crate::settings_ui::Page::Profiles);
        if let Some(s) = self.settings.as_mut() {
            s.show_profile_tab(&guid, "Session");
        }
    }

    /// Changes the session's saved profile's layout (Hide writes to the original profile
    /// even for a session with its own copy, `PS:23130-23135`).
    fn edit_status_layout(&mut self, pane: PaneId, f: impl FnOnce(&mut Layout)) {
        let Some(guid) = self.panes.get(&pane).map(|p| self.store.shared_guid(&p.profile)) else { return };
        let Some(profile) = self.store.get_mut(&guid) else { return };
        let mut layout = Layout::from_value(profile.get(LAYOUT_KEY).as_ref());
        f(&mut layout);
        profile.set(LAYOUT_KEY, layout.to_value());
        self.apply_edit(Edit::Changed(guid));
    }

    /// Disable Status Bar (`PS:23070-23080`): the session's own copy when it has one, else
    /// the shared profile (every session using it).
    fn disable_status_bar(&mut self, pane: PaneId) {
        let Some(guid) = self.panes.get(&pane).map(|p| p.profile.clone()) else { return };
        if self.store.sessions.iter().any(|s| s.profile.guid() == guid) {
            self.set_session_values(pane, &[(SHOW_KEY.to_string(), Some(toml::Value::Boolean(false)))]);
            return self.relayout();
        }
        let Some(profile) = self.store.get_mut(&guid) else { return };
        profile.set(SHOW_KEY, toml::Value::Boolean(false));
        self.apply_edit(Edit::Changed(guid));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn component(class: &str, knobs: &[(&str, toml::Value)]) -> Component {
        Component { class: class.into(), knobs: knobs.iter().map(|(k, v)| (k.to_string(), v.clone())).collect(), config: toml::Table::new() }
    }

    #[test]
    fn pieces_size_like_iterm2() {
        assert_eq!(Piece::of(SPRING_CLASS), Some(Piece::Spring));
        assert_eq!(Piece::of(SPACER_CLASS), Some(Piece::Spacer));
        assert_eq!(Piece::of("iTermStatusBarClockComponent"), None);
        assert_eq!(Piece::Placeholder.text().as_deref(), Some("Click here to configure status bar"));
        assert_eq!(Piece::Placeholder.click(), Click::Configure);

        let spring = Piece::Spring.item(&component(SPRING_CLASS, &[]), Algorithm::TightlyPacked, 0.0);
        assert_eq!((spring.kind, spring.min, spring.stretch, spring.margins), (ItemKind::Spring, 0.0, true, false));
        assert!(spring.preferred.is_infinite());

        // The spacer's width knob, 5 when absent or 0.
        let spacer = Piece::Spacer.item(&component(SPACER_CLASS, &[]), Algorithm::Stable, 0.0);
        assert_eq!((spacer.kind, spacer.min, spacer.preferred, spacer.stretch), (ItemKind::FixedSpacer, 5.0, 5.0, false));
        let zero = Piece::Spacer.item(&component(SPACER_CLASS, &[(SPACER_WIDTH_KEY, toml::Value::Float(0.0))]), Algorithm::Stable, 0.0);
        assert_eq!(zero.min, 5.0);
        let wide = Piece::Spacer.item(&component(SPACER_CLASS, &[(SPACER_WIDTH_KEY, toml::Value::Integer(30))]), Algorithm::Stable, 0.0);
        assert_eq!(wide.min, 30.0);

        let placeholder = Piece::Placeholder.item(&component(PLACEHOLDER_CLASS, &[]), Algorithm::Stable, 180.0);
        assert_eq!((placeholder.min, placeholder.preferred, placeholder.stretch, placeholder.margins), (180.0, 180.0, true, true));
    }

    #[test]
    fn rainbow_hues_are_rgb() {
        assert_eq!(hsb(0.0, 1.0, 1.0), crate::palette::rgb(255, 0, 0));
        assert_eq!(hsb(1.0 / 3.0, 1.0, 1.0), crate::palette::rgb(0, 255, 0));
        assert_eq!(hsb(0.5, 0.0, 0.5), crate::palette::rgb(128, 128, 128));
        assert!(is_dark(scene::STATUS_BAR_BG));
    }
}
