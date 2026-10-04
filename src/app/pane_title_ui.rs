//! Pane title bars take their room: where a pane's title bar, per-pane status bar and
//! terminal sit (`SessionView.m:2255-2275`, `PTYTab.m:610-650`).

use super::*;
use crate::pane_title;
use crate::statusbar_layout::{self as sbl, Position};

/// A pane's rectangle cut into its parts.
pub(super) struct Chrome {
    /// What the terminal gets.
    pub(super) rest: Rect,
    /// The title bar, when the pane's tab shows them.
    #[allow(dead_code)] // the title bar draws from it (next task)
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
}
