//! The Cockpit (`CW`, `CX`): one panel listing every session of every window, with a
//! command bar that writes to the chosen ones. This file is the pure part of Window >
//! Cockpit: the constants and texts, the group mode, the frame and split rules, and the tip
//! line under the command bar.

use std::time::{Duration, Instant};

/// The window's title (`CX:23`).
pub const TITLE: &str = "Cockpit";
/// The window's content size at first open (`CX:23`), in pixels.
pub const SIZE: (f32, f32) = (877.0, 416.0);
/// The saved frame, "x y w h".
pub const FRAME_KEY: &str = "NoSyncCockpitFrame";
/// The saved height of the list (iTerm2's split autosave, `CW:65`).
pub const SPLIT_KEY: &str = "NoSyncCockpitSplit";
/// The saved group mode, 0, 1 or 2 (`CW:1305-1315`).
pub const GROUP_KEY: &str = "NoSyncCockpitGroupMode";
/// The list keeps at least this height and the composer at least `COMPOSER_MIN` (`CW:2753-2772`).
pub const LIST_MIN: f32 = 100.0;
pub const COMPOSER_MIN: f32 = 110.0;
/// The filter bar's height (`CX`).
pub const FILTER_BAR_HEIGHT: f32 = 30.0;
/// What the tip line says when no message shows (`CW:63`).
pub const TIP: &str = "Type @ to choose sessions to write to";
/// The command field's hint.
pub const PLACEHOLDER: &str = "Type here to write to selected sessions\u{2026}";
/// How long a message replaces the tip (`CW:862-873`).
pub const MESSAGE_FOR: Duration = Duration::from_secs(4);

/// How the list groups sessions (`CW:1269-1315`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum GroupMode {
    #[default]
    Status,
    Window,
    Workgroup,
}

pub const MODES: [GroupMode; 3] = [GroupMode::Status, GroupMode::Window, GroupMode::Workgroup];

impl GroupMode {
    /// The saved value; anything unknown is Status.
    pub fn from_raw(v: i64) -> GroupMode {
        match v {
            1 => GroupMode::Window,
            2 => GroupMode::Workgroup,
            _ => GroupMode::Status,
        }
    }

    pub fn raw(self) -> i64 {
        match self {
            GroupMode::Status => 0,
            GroupMode::Window => 1,
            GroupMode::Workgroup => 2,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            GroupMode::Status => "Status",
            GroupMode::Window => "Window",
            GroupMode::Workgroup => "Workgroup",
        }
    }

    pub fn tooltip(self) -> &'static str {
        match self {
            GroupMode::Status => "Group sessions by status (Waiting / Working / Idle), within each window.",
            GroupMode::Window => "Group sessions by window, then by tab and split pane.",
            GroupMode::Workgroup => "Show only sessions in a workgroup, grouped by workgroup.",
        }
    }
}

/// The window's frame in pixels: top left and content size.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Frame {
    /// "x y w h": four finite numbers, the width and height positive.
    pub fn parse(text: &str) -> Option<Frame> {
        let mut it = text.split_whitespace().map(|p| p.parse::<f32>().ok().filter(|v| v.is_finite()));
        let (x, y, w, h) = (it.next()??, it.next()??, it.next()??, it.next()??);
        if it.next().is_some() || w <= 0.0 || h <= 0.0 {
            return None;
        }
        Some(Frame { x, y, w, h })
    }

    pub fn text(&self) -> String {
        format!("{} {} {} {}", self.x, self.y, self.w, self.h)
    }
}

/// A monitor's work area (without the taskbar), in pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Area {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// The size of the corner that must stay on a monitor for the frame to count as reachable.
const GRAB: f32 = 40.0;

/// Where the window opens: the saved frame when its top left corner is on a monitor, else
/// `SIZE` centered in the first work area. A frame larger than its area is shrunk to fit.
pub fn place(saved: Option<Frame>, work: &[Area]) -> Frame {
    let Some(first) = work.first() else {
        return saved.unwrap_or(Frame { x: 0.0, y: 0.0, w: SIZE.0, h: SIZE.1 });
    };
    let on = |f: &Frame| work.iter().find(|a| f.x >= a.x && f.y >= a.y && f.x + GRAB <= a.x + a.w && f.y + GRAB <= a.y + a.h);
    let (frame, area) = match saved.and_then(|f| on(&f).map(|a| (f, *a))) {
        Some(pair) => pair,
        None => (Frame { x: first.x + (first.w - SIZE.0) / 2.0, y: first.y + (first.h - SIZE.1) / 2.0, w: SIZE.0, h: SIZE.1 }, *first),
    };
    if frame.w <= area.w && frame.h <= area.h {
        return frame;
    }
    let (w, h) = (frame.w.min(area.w), frame.h.min(area.h));
    Frame { x: frame.x.clamp(area.x, area.x + area.w - w), y: frame.y.clamp(area.y, area.y + area.h - h), w, h }
}

/// The list's height in a split of `total`: the saved one, else what the composer leaves,
/// kept between `LIST_MIN` and what leaves the composer its `COMPOSER_MIN`.
pub fn split(saved: Option<f32>, total: f32, composer: f32) -> f32 {
    let hi = (total - COMPOSER_MIN).max(LIST_MIN);
    saved.unwrap_or(total - composer).clamp(LIST_MIN, hi)
}

/// The composer: 12 lines of the default profile's font plus 40 (`CW:419-445`).
pub fn composer_height(line_px: f32) -> f32 {
    12.0 * line_px + 40.0
}

/// The line under the command bar: a message for `MESSAGE_FOR`, then the tip.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Tip {
    text: String,
    error: bool,
    until: Option<Instant>,
}

impl Tip {
    pub fn show(&mut self, text: &str, error: bool, now: Instant) {
        self.text = text.to_string();
        self.error = error;
        self.until = Some(now + MESSAGE_FOR);
    }

    /// The text and whether it is an error.
    pub fn current(&self, now: Instant) -> (String, bool) {
        match self.until {
            Some(u) if now < u => (self.text.clone(), self.error),
            _ => (TIP.to_string(), false),
        }
    }

    /// Forgets an expired message; true when the line just went back to the tip.
    pub fn expire(&mut self, now: Instant) -> bool {
        if self.until.is_some_and(|u| now >= u) {
            self.until = None;
            return true;
        }
        false
    }

    /// When the line next changes by itself.
    pub fn next_change(&self) -> Option<Instant> {
        self.until
    }
}

/// What the panel keeps between redraws (later tasks add fields).
#[derive(Clone, Debug, Default)]
pub struct View {
    pub mode: GroupMode,
    /// The list's height in points, once the person has dragged the divider or it was saved.
    pub list_height: Option<f32>,
    pub tip: Tip,
    /// Set by Window > Cockpit: the next redraw puts the caret in the command bar.
    pub focus_command: bool,
    /// The filter bar's search text (case-insensitive substring of session titles).
    pub search: String,
    /// The status bucket the filter bar shows, by its exact key.
    pub status_filter: Option<String>,
    /// Collapsed rows; expansion lives here, keyed by row, so a rebuild keeps it.
    pub collapsed: std::collections::BTreeSet<crate::cockpit_rows::RowKey>,
    /// The chosen rows, by key (never by index).
    pub selected: Vec<crate::cockpit_rows::RowKey>,
}

/// What a redraw asks the app to do (later tasks add variants).
#[derive(Clone, Debug, PartialEq)]
pub enum Act {
    Closed,
    Moved(Frame),
    Split(f32),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cockpit_texts_follow_iterm2() {
        assert_eq!(TITLE, "Cockpit");
        assert_eq!(SIZE, (877.0, 416.0));
        assert_eq!(TIP, "Type @ to choose sessions to write to");
        assert_eq!(PLACEHOLDER, "Type here to write to selected sessions\u{2026}");
        assert_eq!(MESSAGE_FOR, Duration::from_secs(4));
        assert_eq!(LIST_MIN, 100.0);
        assert_eq!(COMPOSER_MIN, 110.0);
        assert_eq!(FILTER_BAR_HEIGHT, 30.0);
        assert_eq!(FRAME_KEY, "NoSyncCockpitFrame");
        assert_eq!(SPLIT_KEY, "NoSyncCockpitSplit");
        assert_eq!(GROUP_KEY, "NoSyncCockpitGroupMode");
    }

    #[test]
    fn group_mode_follows_iterm2() {
        assert_eq!(GroupMode::from_raw(0), GroupMode::Status);
        assert_eq!(GroupMode::from_raw(1), GroupMode::Window);
        assert_eq!(GroupMode::from_raw(2), GroupMode::Workgroup);
        assert_eq!(GroupMode::from_raw(7), GroupMode::Status);
        assert_eq!(GroupMode::from_raw(-1), GroupMode::Status);
        for m in MODES {
            assert_eq!(GroupMode::from_raw(m.raw()), m);
        }
        assert_eq!(MODES.map(GroupMode::label), ["Status", "Window", "Workgroup"]);
        assert_eq!(MODES[0].tooltip(), "Group sessions by status (Waiting / Working / Idle), within each window.");
        assert_eq!(MODES[1].tooltip(), "Group sessions by window, then by tab and split pane.");
        assert_eq!(MODES[2].tooltip(), "Show only sessions in a workgroup, grouped by workgroup.");
        assert_eq!(GroupMode::default(), GroupMode::Status);
    }

    #[test]
    fn frame_follows_windows() {
        assert_eq!(Frame::parse("10 20 877 416"), Some(Frame { x: 10.0, y: 20.0, w: 877.0, h: 416.0 }));
        assert_eq!(Frame::parse("10 20 0 416"), None);
        assert_eq!(Frame::parse("a b c d"), None);
        assert_eq!(Frame::parse("1 2 3"), None);
        assert_eq!(Frame::parse("1 2 3 4 5"), None);
        assert_eq!(Frame { x: 10.0, y: 20.0, w: 877.0, h: 416.0 }.text(), "10 20 877 416");
        let area = [Area { x: 0.0, y: 0.0, w: 1920.0, h: 1040.0 }];
        let f = Frame { x: 100.0, y: 80.0, w: 600.0, h: 300.0 };
        assert_eq!(place(Some(f), &area), f);
        // Only the top left 40 by 40 has to be on the monitor.
        let edge = Frame { x: 1900.0 - 40.0, y: 1000.0 - 40.0, w: 600.0, h: 300.0 };
        assert_eq!(place(Some(edge), &area), edge);
        let centered = Frame { x: 521.5, y: 312.0, w: 877.0, h: 416.0 };
        assert_eq!(place(Some(Frame { x: 5000.0, ..f }), &area), centered);
        assert_eq!(place(None, &area), centered);
        // Off every monitor in a second area's space only when no area holds the corner.
        let two = [area[0], Area { x: 1920.0, y: 0.0, w: 1280.0, h: 1000.0 }];
        assert_eq!(place(Some(Frame { x: 2000.0, ..f }), &two), Frame { x: 2000.0, ..f });
        let big = Frame { x: 0.0, y: 0.0, w: 3000.0, h: 2000.0 };
        assert_eq!(place(Some(big), &area), Frame { x: 0.0, y: 0.0, w: 1920.0, h: 1040.0 });
    }

    #[test]
    fn split_follows_iterm2() {
        assert_eq!(split(None, 386.0, 232.0), 154.0);
        assert_eq!(split(Some(50.0), 386.0, 232.0), 100.0);
        assert_eq!(split(Some(380.0), 386.0, 232.0), 276.0);
        assert_eq!(split(None, 386.0, 300.0), 100.0);
        assert_eq!(composer_height(16.0), 232.0);
    }

    #[test]
    fn tip_follows_iterm2() {
        let t0 = Instant::now();
        let mut tip = Tip::default();
        assert_eq!(tip.current(t0), (TIP.to_string(), false));
        assert_eq!(tip.next_change(), None);
        tip.show("Sent to 2 sessions", false, t0);
        assert_eq!(tip.current(t0), ("Sent to 2 sessions".to_string(), false));
        assert_eq!(tip.next_change(), Some(t0 + MESSAGE_FOR));
        assert_eq!(tip.current(t0 + Duration::from_millis(3999)), ("Sent to 2 sessions".to_string(), false));
        assert_eq!(tip.current(t0 + MESSAGE_FOR), (TIP.to_string(), false));
        tip.show("No matching sessions", true, t0);
        assert_eq!(tip.current(t0), ("No matching sessions".to_string(), true));
        let t1 = t0 + Duration::from_secs(3);
        tip.show("Sent to 1 session", false, t1);
        assert_eq!(tip.current(t0 + Duration::from_secs(5)), ("Sent to 1 session".to_string(), false));
        assert_eq!(tip.next_change(), Some(t1 + MESSAGE_FOR));
        assert!(!tip.expire(t1));
        assert!(tip.expire(t1 + MESSAGE_FOR));
        assert_eq!(tip.next_change(), None);
        assert!(!tip.expire(t1 + MESSAGE_FOR));
    }
}
