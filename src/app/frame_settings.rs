//! The settings the frame loop reads, resolved once per change. A frame asks for layout,
//! cadence and the like many times; each ask reads this instead of looking keys up again.
//! It refreshes only when the globals' generation has moved, which every write bumps.

use std::cell::{Ref, RefCell};

use crate::cadence::Prefs;
use crate::globals::Globals;
use crate::render_fx::Pacing;
use crate::statusbar_layout::Position;
use crate::tab_layout;
use crate::tab_position::{self, TabPosition};
use crate::tab_theme::TabStyle;

pub(super) struct FrameSettings {
    /// The globals' generation these were read at; 0 is never one (they start at 1).
    generation: u64,
    pub cadence: Prefs,
    pub tab_position: TabPosition,
    pub tab_style: TabStyle,
    pub division_view: bool,
    /// `HideTab` and `ShowFullScreenTabBar`: when the tab bar shows.
    pub hide_tab: bool,
    pub show_full_screen_tab_bar: bool,
    /// The tab bar's settings as the globals give them; the theme's label size comes after.
    pub tab_layout: tab_layout::Settings,
    pub custom_tab_font: bool,
    pub status_bar_pt: f32,
    pub status_bar_position: Position,
    pub separate_status_bars: bool,
    pub effects_pacing: Pacing,
    pub auto_composer: bool,
    pub smart_logging: bool,
    pub always_line_marks: bool,
}

impl FrameSettings {
    pub fn new(g: &Globals) -> FrameSettings {
        let mut s = FrameSettings::read(g);
        s.generation = g.generation();
        s
    }

    /// Reads again only when the globals changed since the last read.
    pub fn refresh(&mut self, g: &Globals) {
        if self.generation != g.generation() {
            *self = FrameSettings::new(g);
        }
    }

    fn read(g: &Globals) -> FrameSettings {
        FrameSettings {
            generation: 0,
            cadence: Prefs::from_globals(g),
            tab_position: tab_position::from_value(g.int("TabViewType")),
            tab_style: g.get(crate::tab_theme::KEY).and_then(|v| v.as_integer()).map_or(TabStyle::Automatic, crate::tab_theme::from_value),
            division_view: g.bool("EnableDivisionView"),
            hide_tab: g.bool("HideTab"),
            show_full_screen_tab_bar: g.bool("ShowFullScreenTabBar"),
            tab_layout: tab_layout::Settings::from_globals(g),
            custom_tab_font: g.bool("UseCustomTabBarFontSize"),
            status_bar_pt: g.get("StatusBarHeight").and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64))).unwrap_or(21.0).max(1.0) as f32,
            status_bar_position: Position::from_code(g.int("StatusBarPosition")),
            separate_status_bars: g.bool("SeparateStatusBarsPerPane"),
            effects_pacing: Pacing::from_setting(g.int("EffectsAnimation")),
            auto_composer: g.bool("AutoComposer"),
            smart_logging: g.bool("SmartLoggingWithAutoComposer"),
            always_line_marks: g.bool("AlwaysUseLineStyleMarks"),
        }
    }
}

impl super::App {
    /// The resolved settings, current with the globals.
    pub(super) fn fs(&self) -> Ref<'_, FrameSettings> {
        current(&self.frame_settings, &self.globals)
    }
}

/// The cell's settings, refreshed first when the globals moved. Never panics on nesting: while a
/// `Ref` is held the refresh waits and the current values are served.
pub(super) fn current<'a>(cell: &'a RefCell<FrameSettings>, g: &Globals) -> Ref<'a, FrameSettings> {
    if cell.borrow().generation != g.generation()
        && let Ok(mut fs) = cell.try_borrow_mut()
    {
        fs.refresh(g);
    }
    cell.borrow()
}

pub(super) fn cell(g: &Globals) -> RefCell<FrameSettings> {
    RefCell::new(FrameSettings::new(g))
}

#[cfg(test)]
mod tests {
    use super::*;
    use toml::Value;

    #[test]
    fn fs_nests_inside_a_held_fs() {
        let mut g = Globals::empty(std::path::Path::new("never-written.toml"));
        let cell = cell(&g);
        let held = current(&cell, &g);
        // A nested read (`division_visible` reads `tab_style` inside a held Ref) must not panic.
        let inner = current(&cell, &g);
        assert_eq!(inner.cadence.maximum_frame_rate, held.cadence.maximum_frame_rate);
        drop((held, inner));
        // A change made while a Ref is held shows on the next read after the Refs drop.
        let held = current(&cell, &g);
        g.set("MaximumFrameRate", Value::Integer(30));
        let nested = current(&cell, &g);
        assert_eq!(nested.cadence.maximum_frame_rate, 60.0, "stale while a Ref is held, no panic");
        drop((held, nested));
        assert_eq!(current(&cell, &g).cadence.maximum_frame_rate, 30.0);
    }

    #[test]
    fn frame_settings_follow_a_change() {
        let mut g = Globals::empty(std::path::Path::new("never-written.toml"));
        let cell = cell(&g);
        assert_eq!(cell.borrow().cadence.maximum_frame_rate, 60.0);
        g.set("MaximumFrameRate", Value::Integer(30));
        cell.borrow_mut().refresh(&g);
        assert_eq!(cell.borrow().cadence.maximum_frame_rate, 30.0);
        g.set("StatusBarHeight", Value::Integer(40));
        cell.borrow_mut().refresh(&g);
        assert_eq!(cell.borrow().status_bar_pt, 40.0);
        g.unset("MaximumFrameRate");
        cell.borrow_mut().refresh(&g);
        assert_eq!(cell.borrow().cadence.maximum_frame_rate, 60.0);
    }
}
