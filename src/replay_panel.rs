//! Instant Replay's panel (pure): labels, slider, tick marks, export range steps, and
//! placement. `IRW` = `DVR/iTermInstantReplayWindowController.m`, `IRX` =
//! `DVR/Base.lproj/InstantReplay.xib`, `PT` = `TerminalView/PseudoTerminal.m`.

use crate::layout::Rect;

/// `IRX:23-29`.
pub const TITLE: &str = "Instant Replay Control";
/// Content size (`IRX:23-29`).
pub const SIZE: (f32, f32) = (602.0, 43.0);
/// `IRW:268-271`: width may not shrink below this; height is fixed.
pub const MIN_WIDTH: f32 = 270.0;
/// `IRW:13,106-113`: 0.9 normally, 1 while the pointer is over the panel.
pub const ALPHA: f32 = 0.9;
pub const LIVE: &str = "Live View";
pub const NOW: &str = "Now";
pub const EXPORT: &str = "Export\u{2026}";
pub const SET_START: &str = "Set Start";
pub const SET_END: &str = "Set End";
pub const CANCEL: &str = "Cancel";

/// `stringForTimestamp:` (`IRW:279-299`): the medium time alone for today's timestamps,
/// else the short date and the time. `today` and the closures let tests hand in fixed
/// strings and a fixed "now" instead of reading the clock.
pub fn time_label(
    ts_us: i64,
    today: (i32, u32),
    date: &dyn Fn(i64) -> String,
    time: &dyn Fn(i64) -> String,
    day_of: &dyn Fn(i64) -> (i32, u32),
) -> String {
    if day_of(ts_us) == today {
        time(ts_us)
    } else {
        format!("{} {}", date(ts_us), time(ts_us))
    }
}

/// `updateInstantReplayView` (`IRW:301-362`): the slider's fraction, `Some(1.0)` while
/// live (`current < 0`), `None` when the range is not positive (no division by zero).
pub fn slider(current: i64, first: i64, last: i64) -> Option<f32> {
    if current < 0 {
        return Some(1.0);
    }
    let span = last - first;
    if span <= 0 {
        return None;
    }
    Some((current - first) as f32 / span as f32)
}

/// `updateEvents` / `xFromFraction:width:` (`IRW:158-172,27-30`): one x per timestamp,
/// relative to the slice's own first and last, with repeats (adjacent equal x, as the
/// sorted iteration in `updateEvents` finds them) dropped.
pub fn ticks(timestamps: &[i64], width: f32) -> Vec<f32> {
    let mut out = Vec::new();
    let (Some(&first), Some(&last)) = (timestamps.first(), timestamps.last()) else {
        return out;
    };
    let span = (last - first) as f32;
    let mut prev: Option<f32> = None;
    for &t in timestamps {
        let f = if span > 0.0 { (t - first) as f32 / span } else { 0.0 };
        let x = (f * (width - 1.0)).round();
        if prev != Some(x) {
            out.push(x);
            prev = Some(x);
        }
    }
    out
}

/// The current-time label's x, centered on the knob and clamped inside the slider
/// (`IRW:337-346`).
pub fn label_x(frac: f32, slider_x: f32, slider_w: f32, label_w: f32) -> f32 {
    let mut x = frac * slider_w + slider_x - label_w / 2.0;
    if x + label_w > slider_x + slider_w {
        x = slider_x + slider_w - label_w;
    }
    if x < slider_x {
        x = slider_x;
    }
    x
}

/// `originForAccessoryOfSize:` (`PT:10491-10509`): centered under the window, above it
/// when there is no room below, else just above `tabs_bottom` inside the window. Y grows
/// downward on Windows, the opposite of AppKit, so "below" adds the panel's height past
/// an edge and "above" subtracts it.
pub fn origin(size: (f32, f32), window: Rect, work: Rect, tabs_bottom: f32) -> (f32, f32) {
    let (w, h) = size;
    let x = window.x + ((window.w - w) / 2.0).round();
    let room_below = (work.y + work.h) - (window.y + window.h);
    let room_above = window.y - work.y;
    let y = if room_below > h {
        window.y + window.h
    } else if room_above > h {
        window.y - h
    } else {
        tabs_bottom - h
    };
    (x, y)
}

/// The export button's state machine (`iTermInstantReplayState`, `IRW:201-260`).
/// `SetEnd` carries the start timestamp iTerm2 keeps in `_start`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Export {
    Normal,
    SetStart,
    SetEnd { start: i64 },
}

/// What pressing Export or Cancel asks the app to do.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PanelAct {
    Seek(f32),
    Export { from: i64, to: i64 },
    Beep,
}

/// The panel's own state: the export step, and the tick x's fixed at panel-open time
/// (the kept bug: they do not update while the panel stays open).
#[derive(Clone, Debug)]
pub struct Panel {
    pub export: Export,
    pub ticks_at_open: Vec<i64>,
}

impl Panel {
    /// `exportButton:` / `setState:byCancelling:NO` (`IRW:201-260`). `current` is the
    /// replay decoder's timestamp at the moment of the press.
    pub fn export_button(&mut self, current: i64) -> Vec<PanelAct> {
        match self.export {
            Export::Normal => {
                self.export = Export::SetStart;
                vec![PanelAct::Seek(0.0)]
            }
            Export::SetStart => {
                self.export = Export::SetEnd { start: current };
                vec![PanelAct::Seek(1.0)]
            }
            Export::SetEnd { start } => {
                if current < start {
                    vec![PanelAct::Beep]
                } else {
                    self.export = Export::Normal;
                    vec![PanelAct::Export { from: start, to: current }]
                }
            }
        }
    }

    /// `cancelButton:` / `setState:byCancelling:YES` (`IRW:209-211`): always back to
    /// Normal, never exports.
    pub fn cancel(&mut self) {
        self.export = Export::Normal;
    }

    /// The two button labels; the second is `None` while hidden (`IRW:233-259`).
    pub fn buttons(&self) -> (&'static str, Option<&'static str>) {
        match self.export {
            Export::Normal => (EXPORT, None),
            Export::SetStart => (SET_START, Some(CANCEL)),
            Export::SetEnd { .. } => (SET_END, Some(CANCEL)),
        }
    }

    /// The export range's red band, start to the live current position while `SetEnd`
    /// (`IRW:222-223,357-360`); `None` otherwise.
    pub fn band(&self, current: i64, first: i64, last: i64) -> Option<(f32, f32)> {
        match self.export {
            Export::SetEnd { start } => {
                let span = (last - first) as f32;
                if span <= 0.0 {
                    return None;
                }
                let sf = (start - first) as f32 / span;
                let cf = (current - first) as f32 / span;
                Some((sf, cf))
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panel_follows_iterm2() {
        let mut p = Panel { export: Export::Normal, ticks_at_open: vec![] };
        assert_eq!(p.buttons(), (EXPORT, None));

        assert_eq!(p.export_button(0), vec![PanelAct::Seek(0.0)]);
        assert_eq!(p.buttons(), (SET_START, Some(CANCEL)));

        assert_eq!(p.export_button(5000), vec![PanelAct::Seek(1.0)]);
        assert_eq!(p.buttons(), (SET_END, Some(CANCEL)));
        assert_eq!(p.band(5000, 0, 10000), Some((0.5, 0.5)), "band starts at 5000");

        assert_eq!(p.export_button(9000), vec![PanelAct::Export { from: 5000, to: 9000 }]);
        assert_eq!(p.buttons(), (EXPORT, None));

        let mut p2 = Panel { export: Export::SetEnd { start: 5000 }, ticks_at_open: vec![] };
        assert_eq!(p2.export_button(4000), vec![PanelAct::Beep], "end before start beeps");
        assert_eq!(p2.export, Export::SetEnd { start: 5000 }, "state unchanged");

        let mut p3 = Panel { export: Export::SetStart, ticks_at_open: vec![] };
        p3.cancel();
        assert_eq!(p3.export, Export::Normal);
        let mut p4 = Panel { export: Export::SetEnd { start: 100 }, ticks_at_open: vec![] };
        p4.cancel();
        assert_eq!(p4.export, Export::Normal);

        assert_eq!(slider(-1, 0, 0), Some(1.0), "live");
        assert_eq!(slider(1500, 1000, 2000), Some(0.5));
        assert_eq!(slider(1000, 1000, 1000), None, "zero range");

        let today = (2026, 100);
        let date = |_: i64| "9/25/26".to_string();
        let time = |_: i64| "3:04:05 PM".to_string();
        assert_eq!(
            time_label(1, today, &date, &time, &|_| (2026, 100)),
            "3:04:05 PM",
            "today: time only"
        );
        assert_eq!(
            time_label(1, today, &date, &time, &|_| (2026, 99)),
            "9/25/26 3:04:05 PM",
            "yesterday: date and time"
        );

        assert_eq!(ticks(&[0, 500, 501, 1000], 101.0), vec![0.0, 50.0, 100.0], "501 rounds to the same x as 500");

        assert_eq!(label_x(0.0, 86.0, 389.0, 63.0), 86.0, "clamped to the slider's left edge");
        assert_eq!(label_x(1.0, 86.0, 389.0, 63.0), 86.0 + 389.0 - 63.0, "clamped to the slider's right edge");
        assert_eq!(label_x(0.5, 86.0, 389.0, 63.0), 0.5 * 389.0 + 86.0 - 63.0 / 2.0, "centered on the knob");

        let window = Rect { x: 100.0, y: 100.0, w: 800.0, h: 600.0 };
        let work = Rect { x: 0.0, y: 0.0, w: 1920.0, h: 1080.0 };
        let expect_x: f32 = 100.0 + ((800.0f32 - 602.0) / 2.0).round();
        assert_eq!(origin(SIZE, window, work, 130.0), (expect_x, 700.0), "below: room under the window");

        let window_low = Rect { x: 100.0, y: 1050.0, w: 800.0, h: 20.0 };
        assert_eq!(origin(SIZE, window_low, work, 130.0).1, 1050.0 - 43.0, "no room below: above the window");

        let window_full = Rect { x: 0.0, y: 0.0, w: 1920.0, h: 1080.0 };
        assert_eq!(origin(SIZE, window_full, work, 130.0).1, 130.0 - 43.0, "no room either side: just above tabs_bottom");
    }
}
