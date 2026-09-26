//! The status bar's four graphs in the app (milestone 9i, Task 3): which meters are
//! wanted, sampling them on `statusbar_metrics::Meters`' schedule from the status bar's own
//! tick, the slide animation's frames, and what each graph shows (`graph_of`), which
//! `statusbar_ui::draw_widget` draws as quads.
//!
//! Citations use the abbreviations from the milestone 9i brief's Resume notes, paths under
//! `~/gitrepos/iterm2-ref/sources/StatusBar/Components/`: `SPK` =
//! `iTermStatusBarSparklinesComponent.m`, `BAT` = `iTermStatusBarBatteryComponent.m`,
//! `CPU`/`MEM`/`NET` = `iTermStatusBar{CPU,Memory,Network}UtilizationComponent.m`.

use super::statusbar_content::{Content, Graph, Widget};
use super::statusbar_ui::color_of;
use super::*;
use crate::palette::Rgb;
use crate::statusbar::Component;
use crate::statusbar_components::{self as sbc, Kind};
use crate::statusbar_metrics::{self as sm, Meters, Wanted};
use crate::statusbar_sparkline as sp;
use std::time::Duration;

/// `CPU:53-76`, `MEM:39-62`, `BAT:98-108`: the graphs' width, minimum and preferred.
const GRAPH_PT: f32 = 120.0;
/// `NET:80-103`: Network Throughput's.
const NETWORK_PT: f32 = 170.0;
/// The charging glyph's column (the PNG is 16 by 17), reserved at Battery's right.
pub(super) const CHARGING_PT: f32 = 16.0;
/// `NET`: the download and upload lines' colors when the knobs are unset.
const DOWNLOAD_DEFAULT: Rgb = crate::palette::rgb(0, 0, 255);
const UPLOAD_DEFAULT: Rgb = crate::palette::rgb(255, 0, 0);
const ANIMATE_KEY: &str = "animateGraphStatusBarComponents";
const EXCLUDE_TUNNELS_KEY: &str = "excludeUtunFromNetworkUtilization";

/// A graph class.
fn is_graph(class: &str) -> bool {
    matches!(Kind::of(class), Some(Kind::Battery | Kind::Cpu | Kind::Memory | Kind::Network))
}

/// A color knob's color.
fn knob_color(c: &Component, key: &str) -> Option<Rgb> {
    c.knobs.get(key).and_then(color_of).map(|(rgb, _)| rgb)
}

/// A checkbox knob, else its default.
fn knob_bool(c: &Component, key: &str, default: bool) -> bool {
    c.knobs.get(key).and_then(toml::Value::as_bool).unwrap_or(default)
}

/// What a graph component shows now, from the meters (`SPK:196-200`: each series drops the
/// oldest stored sample and keeps the last 60; the ceiling is over everything stored).
/// `has_battery` is this computer's (`metrics_win::has_battery`); `since_sample` is the
/// time since the last sample while the graphs animate, else None.
pub(super) fn graph_of(kind: Kind, c: &Component, m: &Meters, has_battery: bool, since_sample: Option<Duration>) -> Content {
    let base = Graph {
        left: String::new(),
        right: String::new(),
        left_reserve: (String::new(), 0.0),
        right_reserve: (String::new(), 0.0),
        left_color: None,
        right_color: None,
        series: Vec::new(),
        ceiling: 1.0,
        charging: false,
        since_sample,
    };
    let mut icon = sbc::icon(kind);
    let mut empty = false;
    let mut size = GRAPH_PT;
    let graph = match kind {
        // `CPU:78-152`: the samples left-padded with zeros to 60; the smoothed estimate at
        // the left, room for "100%".
        Kind::Cpu => {
            let samples = m.cpu.samples.values();
            let padded = if samples.len() >= sp::SHOWN { samples.clone() } else { sm::cpu_graph(&samples) };
            Graph { left: format!("{}%", sm::cpu_estimate(&samples)), left_reserve: ("100%".into(), 0.0), ceiling: sp::ceiling(&[&padded]), series: vec![(sp::graph_values(&padded), None)], ..base }
        }
        // `MEM:64-123`: the fractions; the bytes used at the left, its own width plus 4.
        Kind::Memory => {
            let samples = m.memory.samples.values();
            let left = sm::memory_text(m.memory.samples.last(), m.memory.total);
            Graph { left_reserve: (left.clone(), 4.0), left, ceiling: sp::ceiling(&[&samples]), series: vec![(sp::graph_values(&samples), None)], ..base }
        }
        // `NET:115-199`: download and upload rates, each in its color, over both series'
        // largest; the rates as texts at either end.
        Kind::Network => {
            size = NETWORK_PT;
            let down: Vec<f64> = m.network.samples.iter().map(|&(d, _)| d).collect();
            let up: Vec<f64> = m.network.samples.iter().map(|&(_, u)| u).collect();
            let (d, u) = m.network.samples.back().copied().unwrap_or((0.0, 0.0));
            Graph {
                left: format!("{}\u{2193}", sm::format_bytes_compact(d)),
                right: format!("{}\u{2191}", sm::format_bytes_compact(u)),
                left_reserve: ("123.0 TB\u{2193}".into(), 0.0),
                right_reserve: ("123.0 TB\u{2191}".into(), 0.0),
                left_color: knob_color(c, sbc::DOWNLOAD_TEXT_COLOR_KEY),
                right_color: knob_color(c, sbc::UPLOAD_TEXT_COLOR_KEY),
                ceiling: sp::ceiling(&[&down, &up]),
                series: vec![
                    (sp::graph_values(&down), Some(knob_color(c, sbc::DOWNLOAD_COLOR_KEY).unwrap_or(DOWNLOAD_DEFAULT))),
                    (sp::graph_values(&up), Some(knob_color(c, sbc::UPLOAD_COLOR_KEY).unwrap_or(UPLOAD_DEFAULT))),
                ],
                ..base
            }
        }
        // `BAT:110-234`: the percentages over 1.0; the texts and reserves `battery_view`
        // decides; empty without a battery.
        _ => {
            icon = Some(sbc::battery_icon(has_battery));
            empty = !has_battery;
            let view = sm::battery_view(m.battery.states.back(), m.on_ac, has_battery, knob_bool(c, sbc::SHOW_PERCENTAGE_KEY, true), knob_bool(c, sbc::SHOW_TIME_KEY, false));
            let values: Vec<f64> = m.battery.states.iter().map(|s| f64::from(s.percentage.unwrap_or(0)) / 100.0).collect();
            Graph {
                left: view.left,
                right: view.right,
                left_reserve: (view.left_reserve.to_string(), 0.0),
                right_reserve: (view.right_reserve.to_string(), if view.right_icon_reserved { CHARGING_PT } else { 0.0 }),
                charging: view.charging_icon,
                series: vec![(sp::graph_values(&values), None)],
                ..base
            }
        }
    };
    Content { empty, uncopyable: true, fixed: Some((size, size)), icon, widget: Some(Widget::Graph(graph)), ..Content::text(Vec::new()) }
}

impl App {
    /// A graph component's content for now.
    pub(super) fn graph_content(&self, kind: Kind, c: &Component) -> Content {
        let since = self.globals.bool(ANIMATE_KEY).then(|| self.meters.last_sample.map(|t| t.elapsed())).flatten();
        graph_of(kind, c, &self.meters, crate::metrics_win::has_battery(), since)
    }

    /// The meters some session's bar has a component for (on screen or not: iTerm2's
    /// components live per session).
    pub(super) fn wanted_meters(&self) -> Wanted {
        let mut w = Wanted::default();
        for &id in self.panes.keys() {
            if !self.shows_status_bar(id) {
                continue;
            }
            for c in self.bar_layout(id).0.components {
                match Kind::of(&c.class) {
                    Some(Kind::Cpu) => w.cpu = true,
                    Some(Kind::Memory) => w.memory = true,
                    Some(Kind::Network) => w.network = true,
                    Some(Kind::Battery) => w.battery = true,
                    _ => {}
                }
            }
        }
        w
    }

    /// The tab shows a bar with a graph.
    fn tab_shows_graph(&self, tab: &Tab) -> bool {
        self.status_bars(tab).iter().any(|b| self.bar_layout(b.pane).0.components.iter().any(|c| is_graph(&c.class)))
    }

    /// The current window, when it is on screen and its front tab shows a graph.
    fn current_shows_graph(&self) -> bool {
        let on_screen = self.gpu.as_ref().is_some_and(|g| g.window.is_minimized() != Some(true) && g.window.is_visible() != Some(false));
        on_screen && self.tabs.get(self.active).is_some_and(|t| self.tab_shows_graph(t))
    }

    /// Redraws every window on screen whose front tab shows a graph; true when any does.
    fn redraw_graph_windows(&self) -> bool {
        let mut any = false;
        if self.current_shows_graph() {
            self.redraw();
            any = true;
        }
        for (window, tab) in self.parked_fronts() {
            if self.tab_shows_graph(tab) {
                window.request_redraw();
                any = true;
            }
        }
        any
    }

    /// Samples the wanted meters when due; a new sample redraws the windows showing a
    /// graph. While the graphs animate (`animateGraphStatusBarComponents`) and the last
    /// sample is under a second old, those windows redraw every `FRAME` (5 a second,
    /// iTerm2's preferred rate), and only while a graph is on screen. When to come back.
    pub(super) fn tick_meters(&mut self, now: Instant) -> Option<Instant> {
        let wanted = self.wanted_meters();
        let exclude = self.globals.bool(EXCLUDE_TUNNELS_KEY);
        let changed = self.meters.tick(now, wanted, exclude, &mut self.samplers);
        // One frame past the slide's end, so its last frame draws in place.
        let sliding = self.globals.bool(ANIMATE_KEY) && self.meters.last_sample.is_some_and(|t| now.saturating_duration_since(t) < sp::SLIDE + sp::FRAME);
        let frame_due = self.graph_frame_due.is_none_or(|due| now >= due);
        let mut frame = None;
        if changed || (sliding && frame_due) {
            let shown = self.redraw_graph_windows();
            if shown && sliding {
                frame = Some(now + sp::FRAME);
            }
        } else if sliding {
            frame = self.graph_frame_due;
        }
        self.graph_frame_due = frame;
        [self.meters.next_due(), frame].into_iter().flatten().min()
    }
}
