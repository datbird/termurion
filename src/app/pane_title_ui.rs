//! Pane title bars take their room: where a pane's title bar, per-pane status bar and
//! terminal sit (`SessionView.m:2255-2275`, `PTYTab.m:610-650`).

use super::*;
use crate::layout::visible_rects;
use crate::pane_title;
use crate::statusbar_layout::{self as sbl, Position};

/// `sessionTabHasMultipleDistinctTabColors` (`PTYTab.m:7333-7348`): more than one distinct
/// color among the panes that have one.
pub(super) fn distinct_tab_colors(colors: impl IntoIterator<Item = Option<crate::palette::Rgb>>) -> bool {
    let mut first = None;
    for c in colors.into_iter().flatten() {
        match first {
            None => first = Some(c),
            Some(f) if f != c => return true,
            Some(_) => {}
        }
    }
    false
}

/// What a press on a title bar does (`SessionTitleView.m`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TitleAction {
    Close,
    Menu,
    ToggleLock,
    Maximize,
    Focus,
}

/// The action for `button` (0 left, 1 right, 2 middle) pressed on `hit` as click number
/// `clicks`: the close button, the menu button and the lock mark by their own buttons; a right
/// click anywhere is the menu (`STV:164-166`, NSView's `menu`); a double click on the bar
/// maximizes (`PTYTab.m:7414-7416`); other presses just focus.
pub(super) fn title_action(hit: pane_title::Hit, button: u8, clicks: u8) -> Option<TitleAction> {
    use pane_title::Hit;
    match (button, hit) {
        (1, _) => Some(TitleAction::Menu),
        (0, Hit::Close) => Some(TitleAction::Close),
        (0, Hit::Menu) => Some(TitleAction::Menu),
        (0, Hit::Lock) => Some(TitleAction::ToggleLock),
        (0, Hit::Bar) if clicks == 2 => Some(TitleAction::Maximize),
        (0, Hit::Bar) => Some(TitleAction::Focus),
        _ => None,
    }
}

/// A left press on the bar itself (not a button, not the double click) arms the pane drag
/// (`SessionTitleView mouseDragged:`).
pub(super) fn arms_drag(hit: pane_title::Hit, button: u8, clicks: u8) -> bool {
    hit == pane_title::Hit::Bar && button == 0 && title_action(hit, button, clicks) == Some(TitleAction::Focus)
}

/// Whether a press makes its pane the tab's focused pane: only a left or middle press on the
/// bar itself (`SessionTitleView.m:386-395`); the buttons and the right click's menu leave the
/// focus where it is, so the menu's commands act on their own pane and give the focus back.
pub(super) fn press_focuses(hit: pane_title::Hit, button: u8) -> bool {
    hit == pane_title::Hit::Bar && matches!(button, 0 | 2)
}

/// The buttons act on the release over the part that was pressed (an NSButton tracks to the
/// release, P14); the bar's own actions and the right-click menu act on the press.
pub(super) fn acts_on_release(action: TitleAction) -> bool {
    matches!(action, TitleAction::Close | TitleAction::Menu | TitleAction::ToggleLock)
}

/// A left press on a title bar button waiting for its release: the pane, the part, and the
/// action it will take.
pub(super) type TitlePress = (PaneId, pane_title::Hit, TitleAction);

/// A pane's rectangle cut into its parts.
pub(super) struct Chrome {
    /// What the terminal gets.
    pub(super) rest: Rect,
    /// The title bar, when the pane's tab shows them.
    pub(super) title: Option<Rect>,
    /// The pane's own status bar: in the title bar's slot when it sits at the top of a
    /// pane with a title bar, else a strip at the top or bottom of the rest.
    pub(super) bar: Option<Rect>,
}

/// The pure split (`SessionView.m:2255-2275`): the title bar off the top, then a bottom bar
/// off the rest; a top bar goes into the title bar's slot (`STV:182-195`), or off the rest
/// when there is no title bar. `bar` is the bar's height and position; `scale` places the
/// slot's parts. A pane always keeps a pixel under its title bar.
pub(super) fn chrome(rect: Rect, title_h: f32, title: bool, bar: Option<(f32, Position)>, scale: f32) -> Chrome {
    if !title {
        let (rest, bar) = sbl::pane_split(rect, bar);
        return Chrome { rest, title: None, bar };
    }
    let (title_rect, below) = pane_title::split(rect, title_h.min((rect.h - 1.0).max(0.0)));
    match bar {
        Some((_, Position::Top)) => Chrome { rest: below, bar: Some(pane_title::geometry(title_rect, scale, false).status), title: Some(title_rect) },
        other => {
            let (rest, bar) = sbl::pane_split(below, other);
            Chrome { rest, title: Some(title_rect), bar }
        }
    }
}

impl App {
    /// Whether the tab's panes show title bars (`PTYTab updatePaneTitles`): every pane of
    /// the tab or none. A zoomed tab still counts every pane.
    pub(super) fn tab_shows_titles(&self, tab: &Tab) -> bool {
        let top_bar = self.per_pane_bars(tab) && self.status_bar_position() == Position::Top && tab.layout.panes().iter().any(|&p| self.shows_status_bar(p));
        pane_title::shows_titles(self.globals.bool("ShowPaneTitles"), self.globals.bool("ShowPaneTitlesEvenIfOnlyOnePane"), tab.layout.panes().len(), top_bar)
    }

    /// A title bar's height in pixels.
    pub(super) fn title_height(&self) -> f32 {
        pane_title::height(self.status_bar_pt(), self.scale())
    }

    /// A pane's tab color: a program's (or tmux's saved) color, else its profile's. The tab and
    /// the pane's title bar read the same one (P4).
    pub(super) fn pane_tab_color(&self, id: PaneId) -> Option<crate::palette::Rgb> {
        self.tab_colors.get(&id).copied().or_else(|| self.panes.get(&id).and_then(|p| self.store.get(&p.profile)).and_then(|pr| pr.tab_color(self.dark())))
    }

    /// Draws the active tab's title bars and remembers where they are (`title_bars`, for
    /// clicks). The background goes in `frame.quads` before the status bars draw, so a bar in
    /// its title bar paints over it; the text is rows, never overlay.
    pub(super) fn draw_title_bars(&mut self, frame: &mut scene::Frame, cell: (f32, f32)) {
        self.title_bars.clear();
        let Some(tab) = self.tabs.get(self.active) else { return };
        if !self.tab_shows_titles(tab) {
            return;
        }
        // S1: the shortcut ordinals follow the order Alt+digit picks by (`pane_by_number`).
        let panes = self.ordered_panes(tab);
        let ordinals = pane_title::ordinals(panes.len());
        let tag = self.globals.int("SwitchPaneModifier");
        let (style, resolved) = (self.tab_style(), self.resolved_style());
        let legibility = self.globals.number("MinimalTextLegibilityAdjustment").map_or(1.0, |v| v as f32);
        let (custom_bg, custom_text) = (self.globals.string("PaneTitleBarBackgroundColor"), self.globals.string("PaneTitleBarTextColor"));
        let distinct = distinct_tab_colors(panes.iter().map(|&p| self.pane_tab_color(p)));
        let bview = self.broadcast_view();
        let dim_inactive = self.globals.bool("DimInactiveSplitPanes");
        // A see-through frame (`build_frame` set `frame_alpha`) gives each pane its 13g alpha, as
        // the pane's own background takes it; an opaque window draws every bar opaque.
        let alphas = if self.frame_alpha { self.pane_alphas(tab) } else { Vec::new() };
        let mut drawn = Vec::new();
        for (id, rect) in visible_rects(&tab.layout, tab.zoomed, self.content_area_for(tab), self.gap()) {
            let chrome = self.pane_chrome(tab, id, rect);
            let (Some(title), Some(p)) = (chrome.title, self.panes.get(&id)) else { continue };
            // A status bar in the title bar replaces the label (`STV:190-194`) and may color it.
            let bar_in_title = chrome.bar.is_some_and(|b| title.contains(b.x, b.y));
            let bar_bg = bar_in_title.then(|| self.bar_layout(id).0.advanced.background_color.as_ref().and_then(statusbar_ui::color_of).map(|(c, _)| c)).flatten();
            let focused_pane = id == tab.focused;
            let dim = self.pane_dim(tab, id, &bview);
            let parts = pane_title::Parts {
                style,
                resolved,
                dimmed: !focused_pane,
                dim_amount: dim.amount,
                dim_only_text: dim.only_text,
                dim_inactive,
                window_focused: self.focused,
                session_bg: p.screen.palette.bg,
                tab_color: self.pane_tab_color(id),
                distinct_tab_colors: distinct,
                custom_bg: &custom_bg,
                custom_text: &custom_text,
                bar_bg,
                legibility,
                locked: p.locked,
                transparency_alpha: alphas.iter().find(|(a, _)| *a == id).map_or(1.0, |(_, a)| *a),
            };
            // P11: the Minimal theme's background alpha (`transparencyAlpha`, `iTermTheme.m:322-353`)
            // comes from `look` via `transparency_alpha` above; text, menu and line stay pre-blended.
            let look = pane_title::look(&parts);
            let ordinal = panes.iter().position(|&x| x == id).and_then(|i| ordinals.get(i)).copied().unwrap_or(0);
            let text = pane_title::title_text(&self.session_title(id), ordinal, tag);
            let geometry = pane_title::geometry(title, self.scale(), parts.locked);
            scene::pane_title_bar(frame, &scene::TitleBar { geometry, look, text: &text, show_label: !bar_in_title }, cell);
            drawn.push((id, geometry));
        }
        self.title_bars = drawn;
    }

    /// The title bar and part under a point, in the active tab.
    pub(super) fn title_hit_at(&self, x: f32, y: f32) -> Option<(PaneId, pane_title::Hit)> {
        self.title_bars.iter().find_map(|(id, g)| pane_title::hit(g, x, y).map(|h| (*id, h)))
    }

    /// A press on a pane's title bar. A left or middle press on the bar itself focuses the pane
    /// (`press_focuses`); the buttons and the right click's menu leave the focus alone. The close,
    /// menu and lock buttons wait for the release over the same part (`title_release`, P14);
    /// the right click's menu and the double click's maximize act now. Nothing reaches the
    /// terminal: no report to the program, no selection. Left true when the press was on a bar.
    pub(super) fn title_press(&mut self, button: MouseButton, clicks: u8, x: f32, y: f32, _event_loop: &ActiveEventLoop) -> bool {
        let Some((pane, hit)) = self.title_hit_at(x, y) else { return false };
        if press_focuses(hit, button_number(button)) {
            if let Some(t) = self.tabs.get_mut(self.active) {
                t.focused = pane;
            }
            self.refresh_route();
        }
        let action = title_action(hit, button_number(button), clicks);
        match action {
            Some(a) if acts_on_release(a) && button == MouseButton::Left => self.title_press = Some((pane, hit, a)),
            Some(TitleAction::Menu) => self.show_context_menu_at(Some((pane, None))),
            Some(TitleAction::Maximize) => self.toggle_maximize(),
            // Focus is done; moving the pointer far enough from here drags the pane.
            Some(_) | None => {}
        }
        if arms_drag(hit, button_number(button), clicks) {
            self.drag = Drag::Title { pane, from: (x, y) };
        }
        self.redraw();
        true
    }

    /// The release of a title bar button's press: the action runs when the pointer is still over
    /// the part that was pressed. True when a press was waiting for this release.
    pub(super) fn title_release(&mut self, button: MouseButton, event_loop: &ActiveEventLoop) -> bool {
        if button != MouseButton::Left {
            return false;
        }
        let Some((pane, hit, action)) = self.title_press.take() else { return false };
        let (x, y) = self.cursor;
        if self.title_hit_at(x, y) != Some((pane, hit)) || !self.panes.contains_key(&pane) {
            return true;
        }
        match action {
            // `closeCurrentSession:`'s path for that pane.
            TitleAction::Close => {
                if self.layout_allowed(crate::winmodel::LayoutOp::ClosePane) && self.confirm_close(close_ui::Closing::Session(pane)) {
                    self.close_session_undoable(pane, event_loop);
                }
            }
            TitleAction::Menu => self.show_context_menu_at(Some((pane, None))),
            TitleAction::ToggleLock => self.toggle_pane_lock(pane),
            TitleAction::Maximize | TitleAction::Focus => {}
        }
        self.redraw();
        true
    }

    /// `maximizePane:` for the focused pane: a tmux pane zooms through tmux, any other toggles.
    pub(super) fn toggle_maximize(&mut self) {
        let Some(focused) = self.tabs.get(self.active).map(|t| t.focused) else { return };
        if self.tmux_of(focused).is_some() {
            self.tmux_zoom(focused);
        } else if let Some(t) = self.tabs.get_mut(self.active) {
            t.zoomed = if t.zoomed.is_some() { None } else { Some(t.focused) };
        }
    }

    /// A pane's rectangle cut into title bar, status bar and terminal.
    pub(super) fn pane_chrome(&self, tab: &Tab, id: PaneId, rect: Rect) -> Chrome {
        let bar = (self.per_pane_bars(tab) && self.shows_status_bar(id)).then(|| (self.status_bar_height(), self.status_bar_position()));
        chrome(rect, self.title_height(), self.tab_shows_titles(tab), bar, self.scale())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::statusbar_layout::Position;

    fn r(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect { x, y, w, h }
    }

    #[test]
    fn only_a_left_or_middle_press_on_the_bar_focuses() {
        use crate::pane_title::Hit::{Bar, Close, Lock, Menu};
        assert!(press_focuses(Bar, 0));
        assert!(press_focuses(Bar, 2));
        assert!(!press_focuses(Bar, 1), "a right click opens the menu without moving focus");
        for hit in [Close, Menu, Lock] {
            for button in [0, 1, 2] {
                assert!(!press_focuses(hit, button), "{hit:?} button {button}");
            }
        }
    }

    #[test]
    fn title_clicks_follow_iterm2() {
        use crate::pane_title::Hit::{Bar, Close, Lock, Menu};
        // (hit, button, clicks): left 0, right 1, middle 2.
        assert_eq!(title_action(Close, 0, 1), Some(TitleAction::Close));
        assert_eq!(title_action(Menu, 0, 1), Some(TitleAction::Menu));
        assert_eq!(title_action(Lock, 0, 1), Some(TitleAction::ToggleLock));
        assert_eq!(title_action(Bar, 0, 2), Some(TitleAction::Maximize));
        assert_eq!(title_action(Bar, 0, 1), Some(TitleAction::Focus));
        assert_eq!(title_action(Bar, 0, 3), Some(TitleAction::Focus), "only the double click maximizes");
        for hit in [Bar, Close, Menu, Lock] {
            assert_eq!(title_action(hit, 1, 1), Some(TitleAction::Menu), "right anywhere on the bar is the menu");
            assert_eq!(title_action(hit, 2, 1), None, "middle does nothing");
        }
        // The buttons act on release over the same part (P14); the bar's own actions on the press.
        assert!(acts_on_release(TitleAction::Close) && acts_on_release(TitleAction::Menu) && acts_on_release(TitleAction::ToggleLock));
        assert!(!acts_on_release(TitleAction::Maximize) && !acts_on_release(TitleAction::Focus));
    }

    #[test]
    fn a_bar_press_arms_the_drag() {
        use crate::pane_title::Hit::{Bar, Close, Lock, Menu};
        assert!(arms_drag(Bar, 0, 1), "a left press on the bar");
        assert!(!arms_drag(Bar, 0, 2), "the double click maximizes");
        assert!(!arms_drag(Bar, 1, 1), "right is the menu");
        for hit in [Close, Menu, Lock] {
            assert!(!arms_drag(hit, 0, 1), "a button is pressed, not dragged");
        }
    }

    #[test]
    fn chrome_splits_the_pane() {
        let rect = r(0.0, 30.0, 400.0, 200.0);
        let c = chrome(rect, 22.0, true, None, 1.0);
        assert_eq!((c.title, c.bar, c.rest), (Some(r(0.0, 30.0, 400.0, 22.0)), None, r(0.0, 52.0, 400.0, 178.0)));
        let c = chrome(rect, 22.0, true, Some((21.0, Position::Bottom)), 1.0);
        assert_eq!((c.bar, c.rest), (Some(r(0.0, 209.0, 400.0, 21.0)), r(0.0, 52.0, 400.0, 157.0)));
        let c = chrome(rect, 22.0, false, Some((21.0, Position::Top)), 1.0);
        assert_eq!((c.title, c.bar, c.rest), (None, Some(r(0.0, 30.0, 400.0, 21.0)), r(0.0, 51.0, 400.0, 179.0)), "no title: as today");
        let c = chrome(rect, 22.0, true, Some((21.0, Position::Top)), 1.0);
        assert_eq!((c.bar, c.rest), (Some(r(27.0, 30.0, 346.0, 21.0)), r(0.0, 52.0, 400.0, 178.0)), "the bar sits in the title bar's slot");
        let c = chrome(rect, 22.0, false, None, 1.0);
        assert_eq!((c.title, c.bar, c.rest), (None, None, rect), "turned off, the pane has its height back");
        let c = chrome(r(0.0, 0.0, 400.0, 10.0), 22.0, true, None, 1.0);
        assert!(c.rest.h >= 0.0 && c.title.unwrap().h <= 9.0, "a short pane keeps a pixel");
    }

    #[test]
    fn status_bar_moves_into_the_title_bar() {
        use crate::pane_title::shows_titles;
        assert!(!shows_titles(false, false, 2, false));
        assert!(shows_titles(false, false, 2, true), "a top per-pane bar forces titles on");
        // The pane without a bar still gets its title bar.
        let c = chrome(r(0.0, 0.0, 400.0, 100.0), 22.0, true, None, 1.0);
        assert!(c.title.is_some() && c.bar.is_none());
    }

    #[test]
    fn title_parts_follow_the_pane() {
        use crate::palette::rgb;
        let (a, b) = (rgb(1, 2, 3), rgb(9, 9, 9));
        assert!(!distinct_tab_colors([None, Some(a), None, Some(a)]));
        assert!(distinct_tab_colors([Some(a), None, Some(b)]));
        assert!(!distinct_tab_colors([None, None]));
        assert!(!distinct_tab_colors([]));
    }
}
