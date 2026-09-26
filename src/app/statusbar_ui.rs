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
use crate::statusbar_setup_ui::SetupEvent;
use crate::statusbar_layout::{self as sbl, Item, ItemKind, Placed, Position};
use crate::statusbar_components::{self as sbc, Kind};
use crate::statusbar_text as sbt;
use crate::statusbar_sparkline as sp;
use super::metrics_ui::CHARGING_PT;
use super::statusbar_content::{self as sbx, Content, FieldPart, MenuAt, Widget, BUTTON_DY_PT, BUTTON_PAD_PT, GLYPH_PT};

/// `iTermStatusBarPlaceholderComponent`'s text.
pub(super) const PLACEHOLDER_TEXT: &str = "Click here to configure status bar";
const LAYOUT_KEY: &str = "Status Bar Layout";
const SHOW_KEY: &str = "Show Status Bar";
/// The info bar a failed Interpolated String's click shows.
const STATUS_ERROR_BAR: &str = "statusbar:error";
/// The info bar a click that does nothing yet shows.
const STATUS_REFUSE_BAR: &str = "statusbar:refuse";
/// A button face's corner radius, and its inset from the bar's top and bottom, in points.
const BUTTON_RADIUS_PT: f32 = 4.0;
const BUTTON_INSET_PT: f32 = 2.0;
/// An icon's column, in points.
const ICON_PT: f32 = 17.0;
/// The status bar font's size when neither the layout nor the component sets one
/// (`[NSFont systemFontOfSize:12]`).
const DEFAULT_FONT_PT: f32 = 12.0;

/// What one on-screen component is and does. The framework's own pieces, and the content
/// components (`statusbar_content`), whose `Content` says what they show and do now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Piece {
    /// `iTermStatusBarSpringComponent`: empty space that draws only a background color.
    Spring,
    /// `iTermStatusBarFixedSpacerComponent`: a fixed amount of space.
    Spacer,
    /// `iTermStatusBarPlaceholderComponent`, shown alone when a layout has no component
    /// Tandem knows.
    Placeholder,
    /// A content component (milestone 9h).
    Content(Kind),
}

/// What a left click on a component does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Click {
    None,
    /// Opens the profile's status bar settings (the placeholder's click).
    Configure,
    /// Opens the component's menu at the component.
    Menu(Kind),
    /// Shows why the component failed (an Interpolated String's evaluation error).
    Error(String),
    /// Performs the component's action (Custom Action).
    Run,
    /// A Search or Filter Tool: the part under the click (the mode menu, a chevron, the
    /// close glyph) acts, else the field takes the keyboard.
    Field,
    /// The Progress Indicator: its cancel glyph cancels the paste.
    Cancel,
    /// Says why the click does nothing yet, in an info bar with OK.
    Refuse(&'static str),
}

/// The placeholder's content: its one text, and a click that opens the settings.
fn placeholder_content() -> Content {
    Content { click: Click::Configure, ..Content::text(vec![PLACEHOLDER_TEXT.to_string()]) }
}

impl Piece {
    /// The piece for a layout component's class; None for a class Tandem does not show (a
    /// content class it does not place, or one it does not know): kept in the profile,
    /// skipped on screen.
    pub(super) fn of(class: &str) -> Option<Piece> {
        match class {
            SPRING_CLASS => Some(Piece::Spring),
            SPACER_CLASS => Some(Piece::Spacer),
            _ => match Kind::of(class)? {
                k @ (Kind::Clock
                | Kind::HostName
                | Kind::UserName
                | Kind::JobName
                | Kind::CurrentDirectory
                | Kind::InterpolatedString
                | Kind::CustomAction
                | Kind::ActionsMenu
                | Kind::SnippetsMenu
                | Kind::TriggersMenu
                | Kind::CallScriptFunction
                | Kind::SearchTool
                | Kind::FilterTool
                | Kind::Composer
                | Kind::Progress
                | Kind::Battery
                | Kind::Cpu
                | Kind::Memory
                | Kind::Network
                | Kind::Git) => Some(Piece::Content(k)),
                _ => None,
            },
        }
    }

    /// The layout algorithms' sizing facts, in points. `widths` are the content's variants
    /// with their measured widths, `min_text_pt` the width of its `min_text`. Minimum and
    /// maximum follow `BLA:261-283`: the larger of the component's own minimum and its
    /// "minwidth" knob; the maximum is `statusBarComponentMaximumWidth` (`MAX(24,
    /// "maxwidth" ?: INFINITY)`). A text component's own minimum is its narrowest variant
    /// and its preferred width its widest (`TXT:178-229`); Job Name, Current Directory and
    /// git state clamp the preferred width by the knobs (`clampedWidth:`, `JOB:60-62`,
    /// `VAR:278-280`, `GIT:136-138`). git's own minimum is its text for branch `M`.
    /// A button (Call Script Function, `FN:127-142`) is its label plus `BUTTON_PAD_PT`,
    /// both minimum and preferred.
    pub(super) fn item(self, c: &Component, algorithm: Algorithm, content: &Content, widths: &[(String, f32)], min_text_pt: Option<f32>) -> Item {
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
                let w = c.spacer_width() as f32;
                Item { min: w.max(knob_min), preferred: w, kind: ItemKind::FixedSpacer, ..base }
            }
            Piece::Placeholder | Piece::Content(_) => {
                let stretch = statusbar::info_for(&c.class, algorithm).is_none_or(|i| i.stretch);
                let (own_min, preferred) = match content.fixed {
                    Some(fixed) => fixed,
                    None => {
                        let (narrow, wide) = if widths.is_empty() { (0.0, 0.0) } else { (sbt::narrowest(widths), sbt::widest(widths)) };
                        let pad = if content.button { BUTTON_PAD_PT } else { 0.0 };
                        // git's minimum is its text for branch `M` (`GIT:240-243`).
                        let own_min = if self == Piece::Content(Kind::Git) { min_text_pt.unwrap_or(0.0) } else { min_text_pt.map_or(narrow, |m| m.min(wide)) };
                        (own_min + pad, wide + pad)
                    }
                };
                let preferred = match self {
                    Piece::Content(Kind::JobName | Kind::CurrentDirectory | Kind::Git) => {
                        let raw_max = c.knobs.get(statusbar::MAX_WIDTH_KEY).and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64))).map_or(f32::INFINITY, |m| m as f32);
                        preferred.max(knob_min).min(raw_max)
                    }
                    _ => preferred,
                };
                Item { min: own_min.max(knob_min), preferred, stretch, icon: content.icon.is_some(), empty: content.empty, ..base }
            }
        }
    }
}

/// A `{r, g, b, a}` table as a color and its alpha.
pub(super) fn color_of(v: &toml::Value) -> Option<(Rgb, f32)> {
    let t = v.as_table()?;
    let c = |k: &str| t.get(k).and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64)));
    let byte = |f: f64| (f.clamp(0.0, 1.0) * 255.0).round() as u8;
    Some((crate::palette::rgb(byte(c("r")?), byte(c("g")?), byte(c("b")?)), c("a").unwrap_or(1.0).clamp(0.0, 1.0) as f32))
}

/// `NSColor colorWithHue:saturation:brightness:alpha:`, as `Rgb` bytes.
fn hsb(h: f32, s: f32, b: f32) -> Rgb {
    let (r, g, bl) = statusbar::hsb_to_rgb(h, s, b);
    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    crate::palette::rgb(byte(r), byte(g), byte(bl))
}

/// A bar on screen: the session whose layout it shows, and where it is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Bar {
    pub(super) pane: PaneId,
    pub(super) rect: Rect,
}

/// One component a bar shows: where it is in the layout (None for the placeholder), what
/// it is, what it shows now (None for a spring or spacer), its variants measured in
/// points, and its text size as a scale of the terminal font. A temporary component (and
/// the spring before a right one) has no place in the layout.
struct Shown {
    index: Option<usize>,
    temporary: bool,
    piece: Piece,
    component: Component,
    content: Option<Content>,
    widths: Vec<(String, f32)>,
    min_text_pt: Option<f32>,
    text_scale: f32,
    /// A `tail` content's first variant cut to its placed room, and its width in points.
    tail: Option<(String, f32)>,
}

impl Shown {
    /// `statusBarComponentCopyableString`: the content's own, else a text component's
    /// widest variant (`TXT`: `longestStringValue`); the framework's pieces have none.
    fn copyable(&self) -> Option<String> {
        let content = self.content.as_ref().filter(|c| matches!(self.piece, Piece::Content(_)) && !c.uncopyable)?;
        content.copy.clone().or_else(|| (!self.widths.is_empty()).then(|| sbt::pick(&self.widths, f32::INFINITY))).filter(|t| !t.is_empty())
    }
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
    /// The visible component under x (points from the bar's left), bounded by its own
    /// margins, not `bands`'s whole-bar tiling: empty space before the first component or
    /// past the last one hits nothing (the 9g review fix, `m9g-checklist.md` #17).
    fn hit(&self, x_pt: f32) -> Option<usize> {
        sbl::hit_index(&self.placed, &self.items, x_pt)
    }
}

/// The profile a status bar setup window edits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum SetupTarget {
    /// A saved profile, by Guid.
    Shared(String),
    /// The pane's own profile copy.
    Session(PaneId),
}

/// The open status bar setup window: its target, and the target's layout value when it
/// opened (what Cancel puts back; None when the profile had none).
pub(super) struct StatusSetup {
    win: crate::statusbar_setup_ui::StatusBarSetupWindow,
    target: SetupTarget,
    saved: Option<toml::Value>,
}

/// The right-click menu's target, until its pick arrives.
pub(super) struct StatusMenu {
    pane: PaneId,
    class: String,
    index: Option<usize>,
    copy: Option<String>,
    /// Checkbox knobs' keys, in menu order.
    checkboxes: Vec<String>,
    /// A component menu's numbered choices (`statusbar:<name>:<n>`), such as Current
    /// Directory's recent directories.
    choices: Vec<String>,
    /// The path a component menu acts on (Current Directory's).
    path: Option<String>,
}

impl StatusMenu {
    /// A component's own (left-click) menu.
    pub(super) fn for_component(pane: PaneId, class: &str, choices: Vec<String>, path: Option<String>) -> StatusMenu {
        StatusMenu { pane, class: class.to_string(), index: None, copy: None, checkboxes: Vec::new(), choices, path }
    }
}

impl App {
    /// The session's profile has "Status bar enabled".
    pub(super) fn shows_status_bar(&self, id: PaneId) -> bool {
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
        let bar = (self.per_pane_bars(tab) && self.shows_status_bar(id)).then(|| (self.status_bar_height(), self.status_bar_position()));
        sbl::pane_split(rect, bar)
    }

    /// What the current window's terminals are sized for now (see `Room`).
    pub(super) fn room(&self) -> sbl::Room {
        sbl::Room { area: self.content_area(), panes: self.tabs.iter().flat_map(|t| self.visible(t)).collect() }
    }

    /// Lays out again, now, every window whose room changed since its last layout: a bar,
    /// shared or per pane, came, went or changed height while another window was current.
    /// Its ptys and tmux's size report follow at once, not on its next frame, so a hidden
    /// window is right too. The current window stays current.
    pub(super) fn relayout_changed_windows(&mut self) {
        // No window is current (one is closing): each heals on its next frame instead.
        let Some(back) = self.current_window() else { return };
        let ids: Vec<WindowId> = self.registry.order().into_iter().filter_map(|k| self.window_id_of_key(k)).collect();
        for id in ids {
            if self.enter(id) && self.laid_out.as_ref() != Some(&self.room()) {
                self.relayout();
            }
        }
        self.enter(back);
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

    /// The session's profile layout (what edits change).
    fn status_layout(&self, pane: PaneId) -> Layout {
        let v = self.profile_of(pane).and_then(|p| p.get(LAYOUT_KEY));
        Layout::from_value(v.as_ref())
    }

    /// The layout the session's bar shows, and whether it is the tmux one: a tmux pane of a
    /// 2.9 or later server shows tmux's status lines with `UseTmuxStatusBar` on or an empty
    /// profile layout (`PTYSession.m:10077-10091`). Tandem computes it when drawing; it is
    /// never written to a profile.
    pub(super) fn bar_layout(&self, pane: PaneId) -> (Layout, bool) {
        let layout = self.status_layout(pane);
        let server = self.panes.get(&pane).and_then(|p| p.tmux_io()).and_then(|io| self.tmux.get(&io.gateway)).and_then(|t| t.hs.min);
        if sbc::uses_tmux_layout(self.globals.bool("UseTmuxStatusBar"), &layout, server) {
            return (sbc::tmux_layout(&layout), true);
        }
        (layout, false)
    }

    /// The text size for a component: its "Custom Font" knob, else the layout's font,
    /// else 12 points; the family stays the prose font. A scale of the terminal font. The
    /// graphs have no Custom Font knob, so a stray one is ignored (`SPK:436-437`).
    fn status_text_scale(&self, layout: &Layout, c: &Component) -> f32 {
        let graph = matches!(Kind::of(&c.class), Some(Kind::Battery | Kind::Cpu | Kind::Memory | Kind::Network));
        let own = if graph { None } else { c.font() };
        let font = own.filter(|f| !f.trim().is_empty()).or_else(|| layout.advanced.font.clone());
        let pt = font.map_or(DEFAULT_FONT_PT, |f| crate::profile::parse_font(&f).size_pt.clamp(4.0, 128.0));
        let term_px = self.gpu.as_ref().map_or(16.0, |g| g.renderer.font_px()).max(1.0);
        pt * self.scale() / term_px
    }

    /// Lays out one bar (`VC:156-198`): the layout's components Tandem knows, with the
    /// temporary ones (`VC:248-293`), or the placeholder when there are none, by the
    /// layout's algorithm. Find with no Search Tool in the layout puts a temporary one
    /// first; with one, it becomes mandatory (`SessionView.m:778-800`). A slow paste's
    /// progress is the temporary right component (`iTermPasteViewManager.m:77-89`), else
    /// Filter's when the layout's Filter Tool is not on the bar (`SessionView.m:762-776`).
    /// A tmux layout's components are not the profile's: nothing to configure or hide.
    fn place_bar(&mut self, bar: &Bar) -> Placement {
        let (layout, tmux) = self.bar_layout(bar.pane);
        let mut p = self.place_bar_with(bar, layout);
        if tmux {
            for s in &mut p.shown {
                s.index = None;
                s.temporary = true;
            }
        }
        p
    }

    fn place_bar_with(&mut self, bar: &Bar, layout: Layout) -> Placement {
        let find = self.finds.get(&bar.pane).filter(|f| f.open).map(|f| f.filter);
        let has = |kind: Kind| layout.components.iter().any(|c| c.class == kind.class());
        let search = find == Some(false);
        let left = (search && !has(Kind::SearchTool)).then(|| sbc::temporary(Kind::SearchTool));
        let progress = self.paste_shows_progress(bar.pane);
        let right = if progress {
            Some(sbc::temporary(Kind::Progress))
        } else if find == Some(true) && !has(Kind::FilterTool) {
            Some(sbc::temporary(Kind::FilterTool))
        } else {
            None
        };
        let p = self.place_components(bar, layout.clone(), left.clone(), right, search);
        let filter_shown = p.placed.iter().any(|pl| p.shown[pl.index].piece == Piece::Content(Kind::FilterTool));
        if find == Some(true) && !progress && !filter_shown && has(Kind::FilterTool) {
            return self.place_components(bar, layout, left, Some(sbc::temporary(Kind::FilterTool)), search);
        }
        p
    }

    /// Lays out the layout's components with the temporaries (`sbc::with_temporaries`).
    /// With `search`, the layout's first Search Tool is the mandatory view (`VC:84-91`).
    fn place_components(&mut self, bar: &Bar, layout: Layout, left: Option<Component>, right: Option<Component>, search: bool) -> Placement {
        let algorithm = layout.advanced.algorithm;
        let n_left = usize::from(left.is_some());
        let all = sbc::with_temporaries(&layout.components, left, right);
        let mut shown: Vec<Shown> = Vec::new();
        for (j, c) in all.iter().enumerate() {
            let index = j.checked_sub(n_left).filter(|&i| i < layout.components.len());
            if let Some(piece) = Piece::of(&c.class) {
                let content = match piece {
                    Piece::Content(kind) => Some(self.content(bar.pane, kind, c)),
                    _ => None,
                };
                shown.push(Shown { index, temporary: index.is_none(), piece, component: c.clone(), content, widths: Vec::new(), min_text_pt: None, text_scale: self.status_text_scale(&layout, c), tail: None });
            }
        }
        if shown.is_empty() {
            let c = Component { class: PLACEHOLDER_CLASS.into(), knobs: toml::Table::new(), config: toml::Table::new() };
            let text_scale = self.status_text_scale(&layout, &c);
            shown.push(Shown { index: None, temporary: false, piece: Piece::Placeholder, component: c, content: Some(placeholder_content()), widths: Vec::new(), min_text_pt: None, text_scale, tail: None });
        }
        let scale = self.scale();
        let cell_w = self.cell().0;
        // A text's width in points: `Renderer::text_width` in pixels over the scale, rounded
        // up (no NSTextField padding).
        let mut measure = |t: &str, text_scale: f32| -> f32 {
            let px = match self.gpu.as_mut() {
                Some(g) => g.renderer.text_width(t, true, text_scale),
                None => t.chars().count() as f32 * cell_w * text_scale,
            };
            (px / scale).ceil()
        };
        let mut items = Vec::with_capacity(shown.len());
        for s in &mut shown {
            match s.content.as_ref() {
                // A field's label and text, measured for drawing: the label first.
                Some(Content { widget: Some(Widget::Field { text, label, .. }), .. }) => {
                    s.widths = [label, text].into_iter().map(|t| (t.clone(), if t.is_empty() { 0.0 } else { measure(t, s.text_scale) })).collect();
                }
                // A graph's reserves (a text plus extra points), then its left and right texts.
                Some(Content { widget: Some(Widget::Graph(g)), .. }) => {
                    let mut w = |t: &str| if t.is_empty() { 0.0 } else { measure(t, s.text_scale) };
                    s.widths = vec![
                        (g.left_reserve.0.clone(), w(&g.left_reserve.0) + g.left_reserve.1),
                        (g.right_reserve.0.clone(), w(&g.right_reserve.0) + g.right_reserve.1),
                        (g.left.clone(), w(&g.left)),
                        (g.right.clone(), w(&g.right)),
                    ];
                }
                Some(content) if content.fixed.is_none() => {
                    s.widths = content.variants.iter().map(|v| (v.clone(), measure(v, s.text_scale))).collect();
                    s.min_text_pt = content.min_text.as_deref().map(|m| measure(m, s.text_scale));
                }
                _ => {}
            }
            let empty = Content::text(Vec::new());
            items.push(s.piece.item(&s.component, algorithm, s.content.as_ref().unwrap_or(&empty), &s.widths, s.min_text_pt));
        }
        if search {
            if let Some(k) = shown.iter().position(|s| !s.temporary && s.piece == Piece::Content(Kind::SearchTool)) {
                items[k].mandatory = true;
            }
        }
        let width_pt = bar.rect.w / scale;
        let placed = sbl::layout(&items, width_pt, algorithm, layout.advanced.remove_empty);
        // A `tail` content draws its first variant cut to the room beside its icon.
        for pl in &placed {
            let s = &mut shown[pl.index];
            let Some(content) = s.content.as_ref().filter(|c| c.tail) else { continue };
            let icon_pt = if content.icon.is_some() { ICON_PT } else { 0.0 };
            let first = content.variants.first().cloned().unwrap_or_default();
            let scale = s.text_scale;
            let text = sbt::tail_fit(&first, (pl.width - icon_pt).max(0.0), &mut |t| measure(t, scale));
            let w = if text.is_empty() { 0.0 } else { measure(&text, scale) };
            s.tail = Some((text, w));
        }
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
            // Whether the git component has a place on this bar (`tick_git`'s bump when it
            // comes onto the screen).
            if p.placed.iter().any(|pl| p.shown[pl.index].piece == Piece::Content(Kind::Git)) {
                self.git_placed.insert(bar.pane);
            } else {
                self.git_placed.remove(&bar.pane);
            }
            let adv = &p.layout.advanced;
            let background = adv.background_color.as_ref().and_then(color_of).unwrap_or((scene::STATUS_BAR_BG, 1.0));
            let default_text = adv.default_text_color.as_ref().and_then(color_of).map_or(scene::STATUS_BAR_TEXT, |(c, _)| c);
            // Auto-rainbow colors every visible component, springs and spacers included
            // (`VC:490-504`); Automatic picks by the window's appearance (review fix: the
            // same source the setup window uses for the same purpose, `open_status_setup`,
            // rather than the bar's own background).
            let rainbow = if adv.rainbow == Rainbow::Disabled { Vec::new() } else { statusbar::rainbow_colors(p.placed.len(), adv.rainbow, self.dark()) };
            let px = |x: f32| bar.rect.x + (x * scale).round();
            // A button's background color is its face, not a band (`FN:78-96`).
            let is_button = |i: usize| p.shown[i].content.as_ref().is_some_and(|c| c.button);
            let bands: Vec<(f32, f32, Rgb, f32)> = sbl::bands(&p.placed, &p.items, p.width_pt)
                .into_iter()
                .filter(|&(_, _, i)| !is_button(i))
                .filter_map(|(a, b, i)| p.shown[i].component.background_color().as_ref().and_then(color_of).map(|(c, al)| (px(a), px(b), c, al)))
                .collect();
            let mut buttons = Vec::new();
            let mut boxes = Vec::new();
            let separators: Vec<f32> = sbl::separators(&p.placed, &p.items).into_iter().map(px).collect();
            // `PS:22983-23010`: the layout's separator color, else the default text color at
            // a quarter alpha.
            let separator_color = Some(adv.separator_color.as_ref().and_then(color_of).unwrap_or((scene::STATUS_BAR_TEXT, 0.25)));
            let mut texts = Vec::new();
            for (k, pl) in p.placed.iter().enumerate() {
                let s = &p.shown[pl.index];
                let Some(content) = &s.content else { continue };
                let color = match rainbow.get(k) {
                    Some(&(h, sat, b)) => hsb(h, sat, b),
                    None => s.component.text_color().as_ref().and_then(color_of).map_or(default_text, |(c, _)| c),
                };
                let x = px(pl.x);
                let clip = Rect { x, y: bar.rect.y, w: (pl.width * scale).round(), h: bar.rect.h };
                // A button's face: its room, inset from the bar's edges, and its label 6
                // points in (`FN`: a borderless rounded button sized to fit).
                if content.button {
                    if let Some((c, a)) = s.component.background_color().as_ref().and_then(color_of) {
                        let inset = (BUTTON_INSET_PT * scale).round();
                        buttons.push((Rect { y: clip.y + inset, h: (clip.h - 2.0 * inset).max(0.0), ..clip }, c, a, BUTTON_RADIUS_PT * scale));
                    }
                }
                let pad_pt = if content.button { BUTTON_PAD_PT / 2.0 } else { 0.0 };
                let dy = if content.button { BUTTON_DY_PT * scale } else { 0.0 };
                // The icon's 17 point column at the left, tinted with the text color.
                let icon_pt = if content.icon.is_some() { ICON_PT } else { 0.0 };
                if let Some(glyph) = content.icon {
                    let icon_clip = Rect { w: (ICON_PT * scale).round().min(clip.w), ..clip };
                    texts.push(scene::BarText { x, text: glyph.to_string(), color, scale: s.text_scale, clip: icon_clip, icon: true, dy: 0.0 });
                }
                // A field or the progress track, in the room beside the icon.
                if let Some(widget) = &content.widget {
                    let room = Rect { x: px(pl.x + icon_pt), w: ((pl.width - icon_pt).max(0.0) * scale).round(), ..clip };
                    let look = WidgetLook { color, dim: mix(color, background.0), background: background.0, text_scale: s.text_scale, scale };
                    draw_widget(widget, room, &s.widths, &look, &mut boxes, &mut texts);
                    continue;
                }
                // The widest variant that fits beside the icon (`TXT:178-229`); User Name
                // always draws its full name (`VAR:249-251`).
                // git truncates its first variant's tail instead (`ATXT:271-278`).
                let room = (pl.width - icon_pt - 2.0 * pad_pt).max(0.0);
                let text = match &s.tail {
                    Some((t, _)) => t.clone(),
                    None if content.min_text.is_some() => content.variants.first().cloned().unwrap_or_default(),
                    None => sbt::pick(&s.widths, room),
                };
                if text.is_empty() {
                    continue;
                }
                let text_pt = match &s.tail {
                    Some((_, w)) => *w,
                    None => s.widths.iter().find(|(v, _)| *v == text).map_or(0.0, |(_, w)| *w),
                };
                let left = pl.x + icon_pt + pad_pt + if content.right_align { (room - text_pt).max(0.0) } else { 0.0 };
                let text_clip = Rect { x: px(pl.x + icon_pt + pad_pt), w: (room * scale).round(), ..clip };
                texts.push(scene::BarText { x: px(left), text, color, scale: s.text_scale, clip: text_clip, icon: false, dy });
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
            let d = scene::StatusBarDraw { rect: bar.rect, background, bands: &bands, separators: &separators, separator_color, boxes: &boxes, buttons: &buttons, texts: &texts, edge, scale };
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
        let x_pt = (x - bar.rect.x) / self.scale();
        let hit = p.hit(x_pt);
        match button {
            MouseButton::Left => {
                let click = hit.and_then(|i| p.shown[i].content.as_ref()).map_or(Click::None, |c| c.click.clone());
                // A click on the bar anywhere but a field gives the keyboard back to the
                // terminal, as a click in the terminal does.
                if click != Click::Field {
                    if let Some(f) = self.finds.get_mut(&bar.pane) {
                        f.typing = false;
                    }
                }
                match click {
                    Click::Configure => self.configure_status_bar(bar.pane, event_loop),
                    Click::Menu(kind) => {
                        let Some(i) = hit else { return true };
                        // At the component's left, below a bar at the top and above one at
                        // the bottom.
                        let left = p.placed.iter().find(|pl| pl.index == i).map_or(0.0, |pl| pl.x);
                        let at = self.menu_at(&bar, left);
                        match kind {
                            Kind::CurrentDirectory => {
                                let path = p.shown[i].content.as_ref().and_then(|c| c.copy.clone());
                                self.show_dir_menu(bar.pane, path, at);
                            }
                            Kind::Git => self.show_git_menu(bar.pane, at),
                            _ => self.show_component_menu(bar.pane, kind, at),
                        }
                    }
                    Click::Run => {
                        if let Some(i) = hit {
                            self.run_custom_action(&p.shown[i].component, event_loop);
                        }
                    }
                    // `SWS`: the evaluation error, with OK.
                    Click::Error(message) => {
                        self.ask(bar.pane, STATUS_ERROR_BAR, &message.replace('\n', " "), &["OK"], false);
                    }
                    Click::Refuse(why) => {
                        self.ask(bar.pane, STATUS_REFUSE_BAR, why, &["OK"], false);
                    }
                    Click::Field => {
                        let Some((i, pl)) = hit.and_then(|i| p.placed.iter().find(|pl| pl.index == i).map(|pl| (i, *pl))) else { return true };
                        let Piece::Content(kind) = p.shown[i].piece else { return true };
                        let Some((parts, offset)) = field_parts_of(&p.shown[i], pl.width) else { return true };
                        match sbx::part_at(&parts, x_pt - pl.x - offset) {
                            FieldPart::Mode => {
                                let at = self.menu_at(&bar, pl.x);
                                self.show_bar_mode_menu(kind == Kind::SearchTool, at);
                            }
                            // `searchNextPrev:`: the left chevron is Find Previous, the right
                            // one Find Next (which goes up).
                            FieldPart::Prev => self.find_step(bar.pane, false),
                            FieldPart::Next => self.find_step(bar.pane, true),
                            FieldPart::Close => self.bar_field_close(bar.pane, kind),
                            FieldPart::Text => self.bar_field_focus(bar.pane, kind),
                        }
                    }
                    // The cancel glyph at the Progress Indicator's right end.
                    Click::Cancel => {
                        let on_glyph = hit.and_then(|i| p.placed.iter().find(|pl| pl.index == i)).is_some_and(|pl| x_pt >= pl.x + pl.width - GLYPH_PT);
                        if on_glyph {
                            self.cancel_paste(bar.pane);
                        }
                    }
                    Click::None => {}
                }
            }
            MouseButton::Right => self.show_status_menu(bar.pane, &p, hit),
            _ => {}
        }
        self.redraw();
        true
    }

    /// Where a component's menu opens for a component `left_pt` points into `bar`: below a
    /// bar at the top, above one at the bottom.
    fn menu_at(&self, bar: &Bar, left_pt: f32) -> MenuAt {
        let x = bar.rect.x + left_pt * self.scale();
        match self.status_bar_position() {
            Position::Top => MenuAt { x, y: bar.rect.y + bar.rect.h, above: false },
            Position::Bottom => MenuAt { x, y: bar.rect.y, above: true },
        }
    }

    /// The field on the pane's bar that shows its find, while find is open: the Search Tool,
    /// or the Filter Tool while Filter is on (a layout's own or the temporary one). Then the
    /// in-pane find bar is not drawn.
    pub(super) fn bar_find_host(&self, pane: PaneId) -> Option<Kind> {
        let f = self.finds.get(&pane).filter(|f| f.open)?;
        let tab = self.tabs.iter().find(|t| t.layout.panes().contains(&pane))?;
        self.status_bars(tab).iter().any(|b| b.pane == pane).then_some(if f.filter { Kind::FilterTool } else { Kind::SearchTool })
    }

    /// A bar shows for the pane, in any window (the tab it is in shows its bar).
    pub(super) fn bar_shown_for(&self, pane: PaneId) -> bool {
        let has = |tab: &Tab| tab.layout.panes().contains(&pane) && self.status_bars(tab).iter().any(|b| b.pane == pane);
        self.tabs.iter().any(has) || self.windows.parked().any(|(_, s)| s.tabs.iter().any(has))
    }

    /// The component menu (`CV:310-388`).
    fn show_status_menu(&mut self, pane: PaneId, p: &Placement, hit: Option<usize>) {
        use muda::{CheckMenuItem, ContextMenu, Menu, MenuItem, PredefinedMenuItem};
        let Some(hwnd) = self.hwnd() else { return };
        let algorithm = p.layout.advanced.algorithm;
        let shown = hit.map(|i| &p.shown[i]);
        let class = shown.map_or(String::new(), |s| s.component.class.clone());
        let copy = shown.and_then(Shown::copyable);
        let menu = Menu::new();
        let mut results = Vec::new();
        if copy.is_some() {
            results.push(menu.append(&MenuItem::with_id("statusbar:copy", "Copy", true, None)));
            results.push(menu.append(&PredefinedMenuItem::separator()));
        }
        let mut checkboxes = Vec::new();
        if let Some(s) = shown {
            // A temporary component is not the layout's: nothing to configure or hide.
            if let Some(info) = statusbar::info_for(&s.component.class, algorithm).filter(|i| !i.internal && !s.temporary) {
                // The spring's Stable name is "Empty Space" (`SPR:42-49`).
                let short = info.short;
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
                let on = shown.and_then(|s| statusbar::knob_values(&s.component, algorithm).get(&k.key).and_then(toml::Value::as_bool)).unwrap_or(false);
                results.push(menu.append(&CheckMenuItem::with_id(format!("statusbar:knob:{i}"), k.label.replace('&', "&&"), true, on, None)));
            }
        }
        if let Some(Err(e)) = results.into_iter().find(Result::is_err) {
            self.banners.set("menu", format!("Status bar menu: {e}"));
        }
        self.status_menu = Some(StatusMenu { pane, class, index: shown.and_then(|s| s.index), copy, checkboxes: checkboxes.into_iter().map(|k| k.key).collect(), choices: Vec::new(), path: None });
        unsafe { menu.show_context_menu_for_hwnd(hwnd, None) };
        self.popup_menu = Some(menu);
    }

    /// A pick from the bar's menu. True when the id was one of its items.
    pub(super) fn status_bar_picked(&mut self, id: &str, event_loop: &ActiveEventLoop) -> bool {
        let Some(what) = id.strip_prefix("statusbar:") else { return false };
        let Some(m) = self.status_menu.take() else { return true };
        if self.dir_menu_picked(what, m.pane, &m.choices, m.path.as_deref(), event_loop) || self.git_menu_picked(what, m.pane, event_loop) || self.component_menu_picked(what, m.pane, event_loop) {
            return true;
        }
        match what {
            "copy" => {
                if let Some(text) = m.copy {
                    self.copy_status_text(text);
                }
            }
            "configure-bar" => self.configure_status_bar(m.pane, event_loop),
            // The setup window with that component selected and its sheet open (`DCV:190-205`).
            "configure" => {
                if let Some(target) = self.status_target(m.pane, &[LAYOUT_KEY, SHOW_KEY]) {
                    self.open_status_setup(target, m.index, true, event_loop);
                }
            }
            // Hide goes the way the knob toggle below goes (both are `statusBarSetLayout:`,
            // `VC:441-448`, `PS:23120-23128`): the layout the bar shows, written where the
            // session keeps it.
            "hide" => {
                let Some(target) = self.status_target(m.pane, &[LAYOUT_KEY]) else { return true };
                let mut layout = self.status_layout(m.pane);
                statusbar::hide_class(&mut layout, &m.class);
                self.write_status_layout(&target, Some(layout.to_value()));
            }
            "disable" => self.disable_status_bar(m.pane),
            k => {
                let Some(key) = k.strip_prefix("knob:").and_then(|i| i.parse::<usize>().ok()).and_then(|i| m.checkboxes.get(i).cloned()) else { return true };
                let Some(index) = m.index else { return true };
                // The layout the bar shows (the session's own copy when it has one), written
                // back where the session keeps it (`statusBarSetLayout:`, `PS:23120-23128`);
                // the knob's value is its saved one or else its default.
                let Some(target) = self.status_target(m.pane, &[LAYOUT_KEY]) else { return true };
                let mut layout = self.status_layout(m.pane);
                let algorithm = layout.advanced.algorithm;
                let Some(c) = layout.components.get_mut(index) else { return true };
                let on = statusbar::knob_values(c, algorithm).get(&key).and_then(toml::Value::as_bool).unwrap_or(false);
                c.knobs.insert(key, toml::Value::Boolean(!on));
                self.write_status_layout(&target, Some(layout.to_value()));
            }
        }
        true
    }

    /// Configure Status Bar and the placeholder's click: the setup window for the profile
    /// the session's bar comes from.
    pub(super) fn configure_status_bar(&mut self, pane: PaneId, event_loop: &ActiveEventLoop) {
        if let Some(target) = self.status_target(pane, &[LAYOUT_KEY, SHOW_KEY]) {
            self.open_status_setup(target, None, false, event_loop);
        }
    }

    /// Where a session's status bar layout is edited: its own profile copy when it has one
    /// that overrides any of `keys`, else the saved profile (`PS:23052-23066`,
    /// `PS:23113-23119`). None for a pane that is gone.
    pub(super) fn status_target(&self, pane: PaneId, keys: &[&str]) -> Option<SetupTarget> {
        let guid = &self.panes.get(&pane)?.profile;
        let own = self.store.sessions.iter().find(|s| s.profile.guid() == *guid).is_some_and(|s| keys.iter().any(|k| s.overridden.contains(*k)));
        Some(if own { SetupTarget::Session(pane) } else { SetupTarget::Shared(self.store.shared_guid(guid)) })
    }

    /// The target profile's saved `Status Bar Layout` value, as stored (None when unset).
    fn status_layout_value(&self, target: &SetupTarget) -> Option<toml::Value> {
        match target {
            SetupTarget::Shared(guid) => self.store.profiles.iter().find(|p| p.guid() == *guid)?.values.get(LAYOUT_KEY).cloned(),
            SetupTarget::Session(pane) => {
                let guid = &self.panes.get(pane)?.profile;
                self.store.sessions.iter().find(|s| s.profile.guid() == *guid)?.profile.values.get(LAYOUT_KEY).cloned()
            }
        }
    }

    /// Writes (Some) or removes (None) the target profile's layout, and redraws every bar.
    pub(super) fn write_status_layout(&mut self, target: &SetupTarget, value: Option<toml::Value>) {
        match target {
            SetupTarget::Shared(guid) => {
                let Some(profile) = self.store.get_mut(guid) else { return };
                match value {
                    Some(v) => profile.set(LAYOUT_KEY, v),
                    None => {
                        profile.values.remove(LAYOUT_KEY);
                    }
                }
                self.apply_edit(Edit::Changed(guid.clone()));
            }
            SetupTarget::Session(pane) => {
                self.set_session_values(*pane, &[(LAYOUT_KEY.to_string(), value)]);
                self.relayout();
            }
        }
        self.redraw_all();
    }

    /// Opens the setup window for `target` (`PSV:664-706`). With `select`, that component is
    /// selected, and with `configure` its Configure Component sheet opens. One window at a
    /// time (iTerm2's is a sheet): a window already open on `target` comes to the front; one
    /// open on another profile keeps its edits (they are already live, as after OK) and
    /// closes, and the window opens again on `target`. Its title names the profile.
    pub(super) fn open_status_setup(&mut self, target: SetupTarget, select: Option<usize>, configure: bool, event_loop: &ActiveEventLoop) {
        if let Some(s) = self.status_setup.as_mut() {
            if s.target == target {
                if let (Some(i), true) = (select, configure) {
                    s.win.configure(i);
                }
                s.win.focus();
                return;
            }
            self.status_setup = None;
        }
        let profile = match &target {
            SetupTarget::Shared(guid) => self.store.profiles.iter().find(|p| p.guid() == *guid),
            SetupTarget::Session(pane) => self.profile_of(*pane),
        };
        let Some(profile) = profile else { return };
        let title = setup_title(&profile.name(), matches!(target, SetupTarget::Session(_)));
        let layout = Layout::from_value(profile.get(LAYOUT_KEY).as_ref());
        // Automatic auto-rainbow follows the window's appearance (`PSV:654-662`; review
        // fix: the same source the bar itself uses, rather than the session background's).
        let dark_background = self.dark();
        let saved = self.status_layout_value(&target);
        let profiles: Vec<(String, String)> = self.store.profiles.iter().map(|p| (p.guid(), p.name())).collect();
        let Some(g) = self.gpu.as_ref() else { return };
        match crate::statusbar_setup_ui::StatusBarSetupWindow::open(event_loop, &g.instance, &g.adapter, &g.device, &g.queue, &title, layout, dark_background, profiles, select, configure, super::statusbar_content::clock_for_knobs) {
            Ok(win) => self.status_setup = Some(StatusSetup { win, target, saved }),
            Err(e) => self.banners.set("window", format!("Could not open the window: {e}")),
        }
    }

    /// The setup window's events: each change goes to the profile at once, OK keeps it,
    /// and Cancel or closing the window puts back the layout it opened with (`PSV:694-703`).
    pub(super) fn status_setup_event(&mut self, window: WindowId, event: &WindowEvent) -> bool {
        let Some(s) = self.status_setup.as_mut() else { return false };
        if s.win.id() != window {
            return false;
        }
        let what = match event {
            WindowEvent::CloseRequested => SetupEvent::Cancel,
            WindowEvent::RedrawRequested => s.win.redraw(),
            other => {
                s.win.event(other);
                SetupEvent::None
            }
        };
        match what {
            SetupEvent::None => {}
            SetupEvent::Changed => {
                let (target, value) = (s.target.clone(), s.win.layout.to_value());
                self.write_status_layout(&target, Some(value));
            }
            SetupEvent::Ok => self.status_setup = None,
            SetupEvent::Cancel => {
                if let Some(s) = self.status_setup.take() {
                    self.write_status_layout(&s.target, s.saved);
                }
            }
        }
        true
    }

    /// A bar on screen shows a Clock: a bar of the tab in front of the current window or of
    /// any other window on screen.
    pub(super) fn any_clock_shown(&self) -> bool {
        let clock = Kind::Clock.class();
        let has_clock = |tab: &Tab| self.status_bars(tab).iter().any(|b| self.bar_layout(b.pane).0.components.iter().any(|c| c.class == clock));
        let current = self.gpu.as_ref().is_some_and(|g| g.window.is_minimized() != Some(true) && g.window.is_visible() != Some(false));
        (current && self.tabs.get(self.active).is_some_and(has_clock)) || self.parked_front_tabs().into_iter().any(has_clock)
    }

    /// Disable Status Bar (`PS:23070-23080`): the session's own copy when it has one, else
    /// the shared profile (every session using it). A bar showing the tmux layout writes
    /// the session's copy: iTerm2 divorced that session to install the layout
    /// (`PS:10085-10090`), so its Disable never reaches the saved profile.
    fn disable_status_bar(&mut self, pane: PaneId) {
        let Some(guid) = self.panes.get(&pane).map(|p| p.profile.clone()) else { return };
        if self.bar_layout(pane).1 || self.store.sessions.iter().any(|s| s.profile.guid() == guid) {
            self.set_session_values(pane, &[(SHOW_KEY.to_string(), Some(toml::Value::Boolean(false)))]);
            return self.relayout();
        }
        let Some(profile) = self.store.get_mut(&guid) else { return };
        profile.set(SHOW_KEY, toml::Value::Boolean(false));
        self.apply_edit(Edit::Changed(guid));
    }
}

/// Half of `a` over `b`: text at half alpha (a placeholder, the results label).
fn mix(a: Rgb, b: Rgb) -> Rgb {
    let m = |x: u8, y: u8| ((u16::from(x) + u16::from(y)) / 2) as u8;
    crate::palette::rgb(m(a.r, b.r), m(a.g, b.g), m(a.b, b.b))
}

/// `a` at alpha `alpha` over `b` (text has no alpha of its own).
fn blend(a: Rgb, b: Rgb, alpha: f32) -> Rgb {
    let m = |x: u8, y: u8| (f32::from(x) * alpha + f32::from(y) * (1.0 - alpha)).round().clamp(0.0, 255.0) as u8;
    crate::palette::rgb(m(a.r, b.r), m(a.g, b.g), m(a.b, b.b))
}

/// How a widget draws: the text color, it at half alpha, the text size, and the scale.
struct WidgetLook {
    color: Rgb,
    dim: Rgb,
    /// The bar's background, which a translucent graph text color is blended over.
    background: Rgb,
    text_scale: f32,
    scale: f32,
}

/// A glyph sits this far into its column, in points.
const GLYPH_INSET_PT: f32 = 2.5;
/// The progress track's height, in points.
const TRACK_PT: f32 = 6.0;

/// A Search, Filter or Composer field's parts, from what `place_components` measured, for
/// a component `width_pt` wide; the parts are in points from the field's left, which is
/// the returned offset (its icon's column) into the component.
fn field_parts_of(s: &Shown, width_pt: f32) -> Option<(sbx::FieldParts, f32)> {
    let Some(Widget::Field { magnifier, chevrons, close, .. }) = s.content.as_ref()?.widget.as_ref() else { return None };
    let icon = if s.content.as_ref()?.icon.is_some() { ICON_PT } else { 0.0 };
    let label_w = s.widths.first().map_or(0.0, |(_, w)| *w);
    Some((sbx::field_parts((width_pt - icon).max(0.0), *magnifier, *chevrons, *close, label_w), icon))
}

/// Draws a field (a one point border in the text color at half alpha, full alpha while it
/// has the keyboard; its glyphs, text or placeholder, and label) or the progress track
/// into `room` (window pixels, the bar's height). `widths`: the field's measured label and
/// text.
fn draw_widget(widget: &Widget, room: Rect, widths: &[(String, f32)], look: &WidgetLook, boxes: &mut Vec<(Rect, Rgb, f32)>, texts: &mut Vec<scene::BarText>) {
    let scale = look.scale;
    let at = |a: f32, b: f32| Rect { x: room.x + (a * scale).round(), w: ((b - a).max(0.0) * scale).round(), ..room };
    let glyph = |texts: &mut Vec<scene::BarText>, part: (f32, f32), c: char| {
        let r = at(part.0, part.1);
        texts.push(scene::BarText { x: r.x + (GLYPH_INSET_PT * scale).round(), text: c.to_string(), color: look.color, scale: look.text_scale, clip: r, icon: true, dy: 0.0 });
    };
    let w_pt = room.w / scale;
    match widget {
        Widget::Field { text, placeholder, label, error, focused, magnifier, chevrons, close } => {
            let inset = (BUTTON_INSET_PT * scale).round();
            let b = Rect { y: room.y + inset, h: (room.h - 2.0 * inset).max(0.0), ..room };
            let t = scale.round().max(1.0);
            let alpha = if *focused { 1.0 } else { 0.5 };
            for edge in [Rect { h: t, ..b }, Rect { y: b.y + b.h - t, h: t, ..b }, Rect { w: t, ..b }, Rect { x: b.x + b.w - t, w: t, ..b }] {
                boxes.push((edge, look.color, alpha));
            }
            let label_w = widths.first().map_or(0.0, |(_, w)| *w);
            let text_w = widths.get(1).map_or(0.0, |(_, w)| *w);
            let parts = sbx::field_parts(w_pt, *magnifier, *chevrons, *close, label_w);
            if let Some(m) = parts.mode {
                glyph(texts, m, sbx::MAGNIFIER);
            }
            for (part, c) in [(parts.prev, sbx::CHEVRON_LEFT), (parts.next, sbx::CHEVRON_RIGHT), (parts.close, sbx::CLOSE_GLYPH)] {
                if let Some(part) = part {
                    glyph(texts, part, c);
                }
            }
            let clip = at(parts.text.0, parts.text.1);
            let (shown, color) = if text.is_empty() { (placeholder.to_string(), look.dim) } else { (text.clone(), if *error { scene::FIND_ERROR } else { look.color }) };
            // The end of a long query stays in view.
            let room_pt = parts.text.1 - parts.text.0;
            let left = if !text.is_empty() && text_w > room_pt { parts.text.1 - text_w } else { parts.text.0 };
            texts.push(scene::BarText { x: room.x + (left * scale).round(), text: shown, color, scale: look.text_scale, clip, icon: false, dy: 0.0 });
            if !label.is_empty() {
                let r = at(parts.label.0, parts.label.1);
                texts.push(scene::BarText { x: r.x, text: label.clone(), color: look.dim, scale: look.text_scale, clip: r, icon: false, dy: 0.0 });
            }
        }
        Widget::Progress(fraction) => {
            let track_w = (w_pt - GLYPH_PT).max(0.0);
            let h = (TRACK_PT * scale).round();
            let track = Rect { y: room.y + ((room.h - h) / 2.0).round(), h, ..at(0.0, track_w) };
            boxes.push((track, look.color, 0.25));
            boxes.push((Rect { w: (track.w * fraction).round(), ..track }, look.color, 1.0));
            glyph(texts, (track_w, w_pt), sbx::CLOSE_GLYPH);
        }
        // `SPK:447-543`: the sparklines in the content rect between the reserves, over the
        // baseline; the left text at the view's left and the right one at its reserve,
        // both 1 point above the centered line (`SPK:436-445`'s -1 fudge); the charging
        // glyph 2 points in from the right.
        Widget::Graph(g) => {
            let reserve = |i: usize| widths.get(i).map_or(0.0, |(_, w)| *w);
            let (left_reserve, right_reserve) = (reserve(0), reserve(1));
            let rect = sp::graph_rect(w_pt, room.h / scale, left_reserve, right_reserve);
            let to_px = |r: Rect| Rect { x: room.x + r.x * scale, y: room.y + r.y * scale, w: r.w * scale, h: r.h * scale };
            let clip = to_px(rect);
            if clip.w > 0.0 && clip.h > 0.0 {
                let base = to_px(sp::baseline(rect));
                boxes.push((Rect { y: base.y.round(), h: scale.round().max(1.0), ..base }, look.color, 0.5));
                let shift = sp::slide(g.since_sample, true, sp::bar_width(rect.w));
                for (values, color) in &g.series {
                    let points: Vec<(f32, f32)> = sp::path(values, rect, g.ceiling.max(1.0), shift).into_iter().map(|(x, y)| (room.x + x * scale, room.y + y * scale)).collect();
                    for r in sp::polyline_rects(&points, scale, clip) {
                        let (c, a) = color.unwrap_or((look.color, 1.0));
                        boxes.push((r, c, a));
                    }
                }
            }
            let dy = -scale;
            if !g.left.is_empty() {
                texts.push(scene::BarText { x: room.x, text: g.left.clone(), color: g.left_color.map_or(look.color, |(c, a)| blend(c, look.background, a)), scale: look.text_scale, clip: room, icon: false, dy });
            }
            if !g.right.is_empty() {
                let r = at((w_pt - right_reserve).max(0.0), w_pt);
                texts.push(scene::BarText { x: r.x, text: g.right.clone(), color: g.right_color.map_or(look.color, |(c, a)| blend(c, look.background, a)), scale: look.text_scale, clip: room, icon: false, dy });
            }
            if g.charging {
                let r = at((w_pt - CHARGING_PT - 2.0).max(0.0), (w_pt - 2.0).max(0.0));
                texts.push(scene::BarText { x: r.x, text: sbc::CHARGING_GLYPH.to_string(), color: look.color, scale: look.text_scale, clip: r, icon: true, dy: 0.0 });
            }
        }
    }
}

/// The setup window's title: the profile it edits, and whether that is a session's own
/// copy (whose edits reach only that session).
fn setup_title(profile: &str, session_copy: bool) -> String {
    if session_copy { format!("Configure Status Bar: {profile} (this session only)") } else { format!("Configure Status Bar: {profile}") }
}

#[cfg(test)]
mod tests {
    use super::*;
    use statusbar::SPACER_WIDTH_KEY;

    #[test]
    fn the_setup_window_names_its_profile() {
        assert_eq!(setup_title("Default", false), "Configure Status Bar: Default");
        assert_eq!(setup_title("Work", true), "Configure Status Bar: Work (this session only)");
    }

    fn component(class: &str, knobs: &[(&str, toml::Value)]) -> Component {
        Component { class: class.into(), knobs: knobs.iter().map(|(k, v)| (k.to_string(), v.clone())).collect(), config: toml::Table::new() }
    }

    #[test]
    fn pieces_size_like_iterm2() {
        assert_eq!(Piece::of(SPRING_CLASS), Some(Piece::Spring));
        assert_eq!(Piece::of(SPACER_CLASS), Some(Piece::Spacer));
        assert_eq!(Piece::of("iTermStatusBarClockComponent"), Some(Piece::Content(Kind::Clock)));
        assert_eq!(Piece::of("iTermStatusBarWorkingDirectoryComponent"), Some(Piece::Content(Kind::CurrentDirectory)));
        assert_eq!(placeholder_content().variants, vec!["Click here to configure status bar".to_string()]);
        assert_eq!(placeholder_content().click, Click::Configure);

        let none = Content::text(Vec::new());
        let spring = Piece::Spring.item(&component(SPRING_CLASS, &[]), Algorithm::TightlyPacked, &none, &[], None);
        assert_eq!((spring.kind, spring.min, spring.stretch, spring.margins), (ItemKind::Spring, 0.0, true, false));
        assert!(spring.preferred.is_infinite());

        // The spacer's width knob, 5 when absent or 0.
        let spacer = Piece::Spacer.item(&component(SPACER_CLASS, &[]), Algorithm::Stable, &none, &[], None);
        assert_eq!((spacer.kind, spacer.min, spacer.preferred, spacer.stretch), (ItemKind::FixedSpacer, 5.0, 5.0, false));
        let zero = Piece::Spacer.item(&component(SPACER_CLASS, &[(SPACER_WIDTH_KEY, toml::Value::Float(0.0))]), Algorithm::Stable, &none, &[], None);
        assert_eq!(zero.min, 5.0);
        let wide = Piece::Spacer.item(&component(SPACER_CLASS, &[(SPACER_WIDTH_KEY, toml::Value::Integer(30))]), Algorithm::Stable, &none, &[], None);
        assert_eq!(wide.min, 30.0);

        let placeholder = Piece::Placeholder.item(&component(PLACEHOLDER_CLASS, &[]), Algorithm::Stable, &placeholder_content(), &widths(&[(PLACEHOLDER_TEXT, 180.0)]), None);
        assert_eq!((placeholder.min, placeholder.preferred, placeholder.stretch, placeholder.margins), (180.0, 180.0, true, true));
    }

    fn widths(v: &[(&str, f32)]) -> Vec<(String, f32)> {
        v.iter().map(|(s, w)| (s.to_string(), *w)).collect()
    }

    /// `TXT:178-229`: a text component's minimum is its narrowest variant and its
    /// preferred its widest; Job and Current Directory clamp the preferred width by the
    /// min and max knobs (`JOB:60-62`, `VAR:278-280`), no other text component does.
    #[test]
    fn content_items_size_by_variants() {
        use crate::statusbar_components::{HOST_NAME_CLASS, JOB_NAME_CLASS, USER_NAME_CLASS};
        let two = widths(&[("a", 30.0), ("abc", 90.0)]);
        let content = Content::text(vec!["a".into(), "abc".into()]);
        let host = Piece::Content(Kind::HostName).item(&component(HOST_NAME_CLASS, &[]), Algorithm::Stable, &content, &two, None);
        assert_eq!((host.min, host.preferred, host.stretch, host.icon, host.empty), (30.0, 90.0, true, false, false));
        let capped = component(HOST_NAME_CLASS, &[("maxwidth", toml::Value::Float(50.0))]);
        assert_eq!(Piece::Content(Kind::HostName).item(&capped, Algorithm::Stable, &content, &two, None).preferred, 90.0, "Host Name does not clamp");

        let job = component(JOB_NAME_CLASS, &[("maxwidth", toml::Value::Float(50.0))]);
        let clamped = Piece::Content(Kind::JobName).item(&job, Algorithm::Stable, &content, &two, None);
        assert_eq!((clamped.min, clamped.preferred), (30.0, 50.0));

        let with_icon = Content { icon: Some('\u{E756}'), ..content.clone() };
        assert!(Piece::Content(Kind::JobName).item(&component(JOB_NAME_CLASS, &[]), Algorithm::Stable, &with_icon, &two, None).icon);

        let fixed = Content { fixed: Some((125.0, 200.0)), ..Content::text(Vec::new()) };
        let f = Piece::Content(Kind::HostName).item(&component(HOST_NAME_CLASS, &[]), Algorithm::Stable, &fixed, &[], None);
        assert_eq!((f.min, f.preferred), (125.0, 200.0));

        let empty = Content::text(vec![String::new()]);
        assert!(empty.empty);
        assert!(Piece::Content(Kind::HostName).item(&component(HOST_NAME_CLASS, &[]), Algorithm::Stable, &empty, &widths(&[("", 0.0)]), None).empty);

        // User Name: the minimum is the narrower of the full name and its first character
        // plus U+2026 (`VAR:234-247`).
        let user = Content::text(vec!["robert".into()]);
        let u = Piece::Content(Kind::UserName).item(&component(USER_NAME_CLASS, &[]), Algorithm::Stable, &user, &widths(&[("robert", 40.0)]), Some(12.0));
        assert_eq!((u.min, u.preferred), (12.0, 40.0));

        // The minwidth knob raises the minimum.
        let wide = component(HOST_NAME_CLASS, &[("minwidth", toml::Value::Float(60.0))]);
        assert_eq!(Piece::Content(Kind::HostName).item(&wide, Algorithm::Stable, &content, &two, None).min, 60.0);
    }

    /// The menu and button components are placed; a button is its label plus 12 points
    /// and does not stretch (`FN:127-150`); the menus and buttons offer no Copy.
    #[test]
    fn buttons_and_menus_size_like_iterm2() {
        use crate::statusbar_components::{ACTIONS_MENU_CLASS, CALL_SCRIPT_FUNCTION_CLASS, CUSTOM_ACTION_CLASS, SNIPPETS_MENU_CLASS, TRIGGERS_MENU_CLASS};
        for (class, kind) in [
            (CUSTOM_ACTION_CLASS, Kind::CustomAction),
            (ACTIONS_MENU_CLASS, Kind::ActionsMenu),
            (SNIPPETS_MENU_CLASS, Kind::SnippetsMenu),
            (TRIGGERS_MENU_CLASS, Kind::TriggersMenu),
            (CALL_SCRIPT_FUNCTION_CLASS, Kind::CallScriptFunction),
        ] {
            assert_eq!(Piece::of(class), Some(Piece::Content(kind)), "{class}");
        }
        let button = Content { button: true, uncopyable: true, click: Click::Refuse(super::super::statusbar_content::SCRIPT_REFUSAL), ..Content::text(vec!["Reload".into()]) };
        let item = Piece::Content(Kind::CallScriptFunction).item(&component(CALL_SCRIPT_FUNCTION_CLASS, &[]), Algorithm::Stable, &button, &widths(&[("Reload", 40.0)]), None);
        assert_eq!((item.min, item.preferred, item.stretch), (52.0, 52.0, false));
        let menu = Content { uncopyable: true, click: Click::Menu(Kind::ActionsMenu), ..Content::text(vec!["Perform Action\u{2026}".into()]) };
        let menu_item = Piece::Content(Kind::ActionsMenu).item(&component(ACTIONS_MENU_CLASS, &[]), Algorithm::Stable, &menu, &widths(&[("Perform Action\u{2026}", 90.0)]), None);
        assert_eq!((menu_item.min, menu_item.preferred, menu_item.stretch), (90.0, 90.0, true));
        let shown = Shown { index: Some(0), temporary: false, piece: Piece::Content(Kind::ActionsMenu), component: component(ACTIONS_MENU_CLASS, &[]), content: Some(menu), widths: widths(&[("Perform Action\u{2026}", 90.0)]), min_text_pt: None, text_scale: 1.0, tail: None };
        assert_eq!(shown.copyable(), None, "no Copy for a menu");
    }

    /// `BAT:98-108`, `CPU:53-76`, `MEM:39-62`, `NET:80-103`: a graph is 120 points (170
    /// for Network), minimum and preferred, beside its 17 point icon; none stretches; none
    /// has Copy (`GFX`/`SPK` never return a copyable string); Battery is empty on a
    /// computer with no battery (`BAT:59-79`).
    #[test]
    fn graph_items_size_like_iterm2() {
        use super::super::metrics_ui::graph_of;
        use crate::statusbar_components::{BATTERY_CLASS, CPU_CLASS, MEMORY_CLASS, NETWORK_CLASS};
        let meters = crate::statusbar_metrics::Meters::new();
        for (class, kind, size) in [(BATTERY_CLASS, Kind::Battery, 120.0), (CPU_CLASS, Kind::Cpu, 120.0), (MEMORY_CLASS, Kind::Memory, 120.0), (NETWORK_CLASS, Kind::Network, 170.0)] {
            assert_eq!(Piece::of(class), Some(Piece::Content(kind)), "{class}");
            let c = component(class, &[]);
            let content = graph_of(kind, &c, &meters, true, None);
            assert!(matches!(content.widget, Some(Widget::Graph(_))), "{class}");
            assert!(!content.empty, "{class}");
            for algorithm in [Algorithm::Stable, Algorithm::TightlyPacked] {
                let item = Piece::Content(kind).item(&c, algorithm, &content, &[], None);
                assert_eq!((item.min, item.preferred, item.stretch, item.icon), (size, size, false, true), "{class} {algorithm:?}");
                // Tightly packed, a lone graph takes its size and the icon's column (Stable
                // gives every component its share of the bar, stretching or not).
                if algorithm == Algorithm::TightlyPacked {
                    let placed = sbl::layout(&[item], 1000.0, algorithm, false);
                    assert_eq!(placed[0].width, size + ICON_PT, "{class}");
                }
            }
            let shown = Shown { index: Some(0), temporary: false, piece: Piece::Content(kind), component: c, content: Some(content), widths: Vec::new(), min_text_pt: None, text_scale: 1.0, tail: None };
            assert_eq!(shown.copyable(), None, "{class} has no Copy");
        }
        let no_battery = graph_of(Kind::Battery, &component(BATTERY_CLASS, &[]), &meters, false, None);
        assert!(no_battery.empty);
        assert_eq!(no_battery.icon, Some(sbc::NO_BATTERY_GLYPH));
        assert_eq!(graph_of(Kind::Battery, &component(BATTERY_CLASS, &[]), &meters, true, None).icon, Some(sbc::BATTERY_GLYPH));
    }

    /// `GIT:136-151,240-297`, `ATXT:271-278,307-313`: git's minimum is the width of its
    /// text for branch `M`, its preferred the rendered width clamped by the minwidth and
    /// maxwidth knobs (`clampedWidth:`); it stretches, shows its icon, truncates its tail,
    /// copies the branch, and is empty without a branch unless the timeout warning shows.
    #[test]
    fn git_item_sizes_like_iterm2() {
        use super::super::git_ui::git_content_of;
        use crate::git_state::{GitState, TIMED_OUT_ICON, TIMED_OUT_SHORT};
        use crate::statusbar_components::GIT_CLASS;
        assert_eq!(Piece::of(GIT_CLASS), Some(Piece::Content(Kind::Git)));
        let st = GitState { branch: "feature/login".into(), dirty: true, ..Default::default() };
        let content = git_content_of(Some(&st), None, true, false);
        assert_eq!(content.variants, vec!["feature/login\u{2009}\u{25CF}".to_string()]);
        assert_eq!(content.min_text.as_deref(), Some("M\u{2009}\u{25CF}"));
        assert!(content.tail && !content.empty && !content.uncopyable);
        assert_eq!(content.copy.as_deref(), Some("feature/login"));
        assert_eq!(content.click, Click::Menu(Kind::Git));
        assert_eq!(content.icon, Some('\u{E8AB}'));
        let w = widths(&[("feature/login\u{2009}\u{25CF}", 90.0)]);
        let sized = |c: &Component, content: &Content, w: &[(String, f32)], m: Option<f32>| Piece::Content(Kind::Git).item(c, Algorithm::Stable, content, w, m);
        let item = sized(&component(GIT_CLASS, &[]), &content, &w, Some(20.0));
        assert_eq!((item.min, item.preferred, item.stretch, item.icon, item.empty), (20.0, 90.0, true, true, false));
        let capped = sized(&component(GIT_CLASS, &[("maxwidth", toml::Value::Float(50.0))]), &content, &w, Some(20.0));
        assert_eq!((capped.min, capped.preferred), (20.0, 50.0));
        let floor = sized(&component(GIT_CLASS, &[("minwidth", toml::Value::Integer(120))]), &content, &w, Some(20.0));
        assert_eq!((floor.min, floor.preferred), (120.0, 120.0));
        let shown = |content: Content, w: Vec<(String, f32)>| Shown { index: Some(0), temporary: false, piece: Piece::Content(Kind::Git), component: component(GIT_CLASS, &[]), content: Some(content), widths: w, min_text_pt: None, text_scale: 1.0, tail: None };
        assert_eq!(shown(content.clone(), w.clone()).copyable().as_deref(), Some("feature/login"), "Copy is the branch, not the text");

        // No branch: empty, nothing to copy, no minimum.
        let none = git_content_of(None, None, true, false);
        assert!(none.empty && none.min_text.is_none());
        assert_eq!(none.variants, vec![String::new()]);
        assert_eq!(sized(&component(GIT_CLASS, &[]), &none, &widths(&[("", 0.0)]), None).min, 0.0);
        assert_eq!(shown(none, widths(&[("", 0.0)])).copyable(), None);

        // Timed out: the long warning first, never empty, still nothing to copy.
        let timed_out = git_content_of(None, None, true, true);
        assert_eq!(timed_out.variants, vec![TIMED_OUT_SHORT.to_string(), TIMED_OUT_ICON.to_string()]);
        assert!(!timed_out.empty && timed_out.tail);
        assert_eq!(shown(timed_out, widths(&[(TIMED_OUT_SHORT, 60.0), (TIMED_OUT_ICON, 12.0)])).copyable(), None);

        // A running command's status stands in for the branch, the minimum too.
        let busy = git_content_of(Some(&st), Some("Pushing\u{2026}"), true, false);
        assert_eq!((busy.variants[0].as_str(), busy.min_text.as_deref()), ("Pushing\u{2026}", Some("Pushing\u{2026}")));
    }

    #[test]
    fn rainbow_hues_are_rgb() {
        assert_eq!(hsb(0.0, 1.0, 1.0), crate::palette::rgb(255, 0, 0));
        assert_eq!(hsb(1.0 / 3.0, 1.0, 1.0), crate::palette::rgb(0, 255, 0));
        assert_eq!(hsb(0.5, 0.0, 0.5), crate::palette::rgb(128, 128, 128));
    }

    /// The hit test (review fix) is bounded per item, not tiled across the whole bar like
    /// `bands`: empty bar space past the last component hits nothing, so a right-click
    /// there offers only Configure Status Bar and Disable Status Bar (`m9g-checklist.md`
    /// #17). The pure geometry is `statusbar_layout::hit_index`'s own test; this just
    /// checks `Placement::hit` delegates to it rather than to `bands`.
    #[test]
    fn bar_hit_test_leaves_empty_space_unhit() {
        let c = component(SPACER_CLASS, &[(SPACER_WIDTH_KEY, toml::Value::Float(20.0))]);
        let item = Piece::Spacer.item(&c, Algorithm::Stable, &Content::text(Vec::new()), &[], None);
        let placed = vec![sbl::Placed { index: 0, x: 6.0, width: 20.0 }];
        let placement = Placement { layout: Layout::default(), shown: Vec::new(), items: vec![item], placed, width_pt: 200.0 };
        assert_eq!(placement.hit(10.0), Some(0));
        assert_eq!(placement.hit(150.0), None, "empty bar space, unlike bands, which would have hit the last item");
    }
}
