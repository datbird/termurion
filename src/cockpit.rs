//! The Cockpit (`CW`, `CX`): one panel listing every session of every window, with a
//! command bar that writes to the chosen ones. This file is the pure part of Window >
//! Cockpit: the constants and texts, the group mode, the frame and split rules, and the tip
//! line under the command bar.

use std::time::{Duration, Instant};

use crate::cockpit_rows::RowKey;
use crate::inert_files::shown_name;
use crate::layout::PaneId;
use crate::tabstatus::{xterm_color, Status};

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

/// The toolbar buttons' widths (`CX:111-148`).
pub const SETTINGS_W: f32 = 33.0;
pub const GROUP_W: f32 = 100.0;
pub const NOTIFY_W: f32 = 33.0;
/// The gear's accessibility label (`CX`).
pub const SETTINGS_A11Y: &str = "Settings";
/// The bell's accessibility labels, unarmed and armed (`CW:604-657`).
pub const NOTIFY_A11Y: &str = "Notify on status change";
pub const NOTIFY_ARMED_A11Y: &str = "Notify on status change armed";

pub fn notify_tooltip(armed: bool) -> &'static str {
    if armed {
        "Watching for a status change on the selected item. An alert will appear on the next change, then turn this off."
    } else {
        "Notify with an alert when the selected window or session\u{2019}s status changes."
    }
}

/// The bell works on a window or a session row.
pub fn notify_enabled(selection: Option<&RowKey>) -> bool {
    matches!(selection, Some(RowKey::Window(_) | RowKey::Session(_)))
}

/// The gear works in Status mode only (`CW:566-597`).
pub fn settings_enabled(mode: GroupMode) -> bool {
    mode == GroupMode::Status
}

/// A row's height, and a session's with a detail line (`CW:1410-1419,1846-1850`).
pub const ROW_HEIGHT: f32 = 24.0;
pub const ROW_HEIGHT_DETAIL: f32 = 38.0;
/// The kind glyphs, from Segoe Fluent Icons (Segoe MDL2 Assets has the same code points):
/// a window, a tab, the command prompt, a box for buried sessions, people for a workgroup, a
/// bell and a ringing bell.
pub const WINDOW_GLYPH: char = '\u{E737}';
pub const TAB_GLYPH: char = '\u{E7C4}';
pub const SESSION_GLYPH: char = '\u{E756}';
pub const BURIED_GLYPH: char = '\u{E7B8}';
pub const WORKGROUP_GLYPH: char = '\u{E716}';
pub const BELL_GLYPH: char = '\u{E7ED}';
pub const BELL_ARMED_GLYPH: char = '\u{EA8F}';
/// The status filter's popup is at least this wide (`CW:362-415`).
pub const FILTER_POPUP_MIN: f32 = 160.0;
/// Room each segment of the filter bar takes beyond its label.
const SEGMENT_PAD: f32 = 16.0;

/// The color of a status word (`CW:2618-2633`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tint {
    Rgb(u8, u8, u8),
    /// The secondary label color.
    Secondary,
}

/// The status color when it parses, else the indicator when it does, else secondary.
pub fn status_tint(st: &Status) -> Tint {
    [st.status_color.as_deref(), st.indicator.as_deref()]
        .into_iter()
        .flatten()
        .find_map(|t| xterm_color(t.trim()))
        .map_or(Tint::Secondary, |c| Tint::Rgb(c.r, c.g, c.b))
}

/// A run of a session's detail line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub code: bool,
}

/// A session's detail (markdown a program sent) as one line of styled runs: line breaks and
/// blocks become spaces, other block markup is dropped, and each run is cleaned like any name
/// a remote sent.
pub fn detail_spans(markdown: &str) -> Vec<Span> {
    use pulldown_cmark::{Event, Parser, Tag, TagEnd};
    let (mut bold, mut italic) = (0u32, 0u32);
    let mut spans: Vec<Span> = Vec::new();
    let mut push = |text: &str, bold: bool, italic: bool, code: bool| {
        let text = shown_name(&text.replace(['\n', '\r', '\t'], " "));
        if text.is_empty() {
            return;
        }
        match spans.last_mut() {
            Some(l) if (l.bold, l.italic, l.code) == (bold, italic, code) => l.text.push_str(&text),
            _ => spans.push(Span { text, bold, italic, code }),
        }
    };
    for e in Parser::new(markdown) {
        match e {
            Event::Start(Tag::Strong) => bold += 1,
            Event::Start(Tag::Emphasis) => italic += 1,
            Event::End(TagEnd::Strong) => bold = bold.saturating_sub(1),
            Event::End(TagEnd::Emphasis) => italic = italic.saturating_sub(1),
            Event::Text(t) => push(&t, bold > 0, italic > 0, false),
            Event::Code(t) => push(&t, bold > 0, italic > 0, true),
            Event::SoftBreak | Event::HardBreak => push(" ", bold > 0, italic > 0, false),
            Event::End(TagEnd::Paragraph | TagEnd::Heading(_) | TagEnd::Item | TagEnd::CodeBlock | TagEnd::BlockQuote(_)) => push(" ", false, false, false),
            _ => {}
        }
    }
    if let Some(l) = spans.last_mut() {
        l.text.truncate(l.text.trim_end().len());
    }
    spans.retain(|s| !s.text.is_empty());
    spans
}

/// The filter bar's items (`CW:362-415`): All, then each present status in bucket order. The
/// key is the status as sent (None is All); the label shows it cleaned.
pub fn filter_items(total: usize, counts: &[(String, usize)]) -> Vec<(Option<String>, String)> {
    std::iter::once((None, format!("All ({total})"))).chain(counts.iter().map(|(k, n)| (Some(k.clone()), format!("{} ({n})", shown_name(k))))).collect()
}

/// Whether the labels fit side by side in `width`, as a segmented row; else the bar is a popup.
pub fn filter_fits(labels: &[String], char_px: f32, width: f32) -> bool {
    labels.iter().map(|l| l.chars().count() as f32 * char_px + SEGMENT_PAD).sum::<f32>() <= width
}

/// What a double-click on a row does (`CW:667-743`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Jump {
    Session(PaneId),
    /// The window's guid: its active tab's focused session.
    Window(String),
    /// The tab's id: its focused session.
    Tab(u64),
    Nothing,
}

/// Groups and the buried root do nothing; workgroups have no leader until milestone 13.
pub fn jump_for(key: &RowKey) -> Jump {
    match key {
        RowKey::Session(p) => Jump::Session(*p),
        RowKey::Window(g) => Jump::Window(g.clone()),
        RowKey::Tab(t) => Jump::Tab(*t),
        RowKey::Group { .. } | RowKey::BuriedRoot | RowKey::Workgroup(_) => Jump::Nothing,
    }
}

/// The chosen rows after a click on `clicked`. `visible` is the list top to bottom. A plain
/// click chooses that row, Ctrl adds or removes it, Shift chooses the run from the first chosen
/// row to it (only rows that can be chosen). A row that cannot be chosen changes nothing.
pub fn select(visible: &[RowKey], current: &[RowKey], clicked: &RowKey, ctrl: bool, shift: bool) -> Vec<RowKey> {
    if !clicked.selectable() {
        return current.to_vec();
    }
    if shift {
        let anchor = current.first().unwrap_or(clicked);
        let (Some(a), Some(b)) = (visible.iter().position(|k| k == anchor), visible.iter().position(|k| k == clicked)) else { return vec![clicked.clone()] };
        let run: Vec<RowKey> = if a <= b { visible[a..=b].to_vec() } else { visible[b..=a].iter().rev().cloned().collect() };
        return run.into_iter().filter(RowKey::selectable).collect();
    }
    if ctrl {
        let mut out = current.to_vec();
        match out.iter().position(|k| k == clicked) {
            Some(i) => {
                out.remove(i);
            }
            None => out.push(clicked.clone()),
        }
        return out;
    }
    vec![clicked.clone()]
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

/// Where the window opens: the saved frame (physical pixels) when its top left corner is on a
/// monitor, else `SIZE` (logical, so times `scale`) centered in the first work area. A frame
/// larger than its area is shrunk to fit. A scale that is not a positive number counts as 1.
pub fn place(saved: Option<Frame>, work: &[Area], scale: f32) -> Frame {
    let scale = if scale.is_finite() && scale > 0.0 { scale } else { 1.0 };
    let size = (SIZE.0 * scale, SIZE.1 * scale);
    let Some(first) = work.first() else {
        return saved.unwrap_or(Frame { x: 0.0, y: 0.0, w: size.0, h: size.1 });
    };
    let on = |f: &Frame| work.iter().find(|a| f.x >= a.x && f.y >= a.y && f.x + GRAB <= a.x + a.w && f.y + GRAB <= a.y + a.h);
    let (frame, area) = match saved.and_then(|f| on(&f).map(|a| (f, *a))) {
        Some(pair) => pair,
        None => (Frame { x: first.x + (first.w - size.0) / 2.0, y: first.y + (first.h - size.1) / 2.0, w: size.0, h: size.1 }, *first),
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
    /// What the first chosen row is (the app fills it before each redraw); the bell follows it.
    pub selection_kind: Option<RowKey>,
    /// The chosen row's watch is armed (the app fills it before each redraw).
    pub selection_armed: bool,
    /// The gear's rect as last drawn, in points: x, y, w, h (the settings popover opens below it).
    pub gear_rect: Option<(f32, f32, f32, f32)>,
    /// The command bar's targets, in the order they were chosen; kept after a send.
    pub chips: Vec<crate::mention::Chip>,
    /// The `@` picker while the command text has a completion target.
    pub picker: Option<crate::mention::Picker>,
}

/// What a redraw asks the app to do (later tasks add variants).
#[derive(Clone, Debug, PartialEq)]
pub enum Act {
    Closed,
    Moved(Frame),
    Split(f32),
    /// The gear: open or close Session Status' settings under it.
    Settings,
    /// A Group button: the new mode.
    Group(GroupMode),
    /// The bell: arm or disarm the selected window or session.
    Notify,
    /// The search field's new text.
    Search(String),
    /// A click on a row: the new chosen rows, in order.
    Select(Vec<RowKey>),
    /// A click on a row's disclosure triangle.
    Toggle(RowKey),
    /// A double-click on a row.
    Jump(RowKey),
    /// The filter bar's choice (None is All).
    StatusFilter(Option<String>),
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
        assert_eq!(place(Some(f), &area, 1.0), f);
        // Only the top left 40 by 40 has to be on the monitor.
        let edge = Frame { x: 1900.0 - 40.0, y: 1000.0 - 40.0, w: 600.0, h: 300.0 };
        assert_eq!(place(Some(edge), &area, 1.0), edge);
        let centered = Frame { x: 521.5, y: 312.0, w: 877.0, h: 416.0 };
        assert_eq!(place(Some(Frame { x: 5000.0, ..f }), &area, 1.0), centered);
        assert_eq!(place(None, &area, 1.0), centered);
        // Off every monitor in a second area's space only when no area holds the corner.
        let two = [area[0], Area { x: 1920.0, y: 0.0, w: 1280.0, h: 1000.0 }];
        assert_eq!(place(Some(Frame { x: 2000.0, ..f }), &two, 1.0), Frame { x: 2000.0, ..f });
        // A first open at 150% scale: SIZE is logical, so the frame is 1.5 times larger, centered,
        // and (at 416 * 1.5) above the 280 logical minimum height instead of clamped to it.
        let hi = [Area { x: 0.0, y: 0.0, w: 2880.0, h: 1560.0 }];
        let first = place(None, &hi, 1.5);
        assert_eq!(first, Frame { x: (2880.0 - 877.0 * 1.5) / 2.0, y: (1560.0 - 416.0 * 1.5) / 2.0, w: 877.0 * 1.5, h: 416.0 * 1.5 });
        assert_eq!(place(None, &[], 2.0), Frame { x: 0.0, y: 0.0, w: 877.0 * 2.0, h: 416.0 * 2.0 });
        // A saved frame is already physical and is not scaled again.
        assert_eq!(place(Some(f), &hi, 1.5), f);
        // A scale that is not a positive number counts as 1.
        assert_eq!(place(None, &area, 0.0), centered);
        assert_eq!(place(None, &area, f32::NAN), centered);
        let big = Frame { x: 0.0, y: 0.0, w: 3000.0, h: 2000.0 };
        assert_eq!(place(Some(big), &area, 1.0), Frame { x: 0.0, y: 0.0, w: 1920.0, h: 1040.0 });
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
    fn toolbar_follows_iterm2() {
        assert_eq!(notify_tooltip(true), "Watching for a status change on the selected item. An alert will appear on the next change, then turn this off.");
        assert_eq!(notify_tooltip(false), "Notify with an alert when the selected window or session\u{2019}s status changes.");
        assert!(notify_enabled(Some(&RowKey::Window("pty-1".into()))));
        assert!(notify_enabled(Some(&RowKey::Session(3))));
        assert!(!notify_enabled(Some(&RowKey::Tab(1))));
        assert!(!notify_enabled(Some(&RowKey::Group { scope: "pty-1".into(), status: "idle".into() })));
        assert!(!notify_enabled(Some(&RowKey::BuriedRoot)));
        assert!(!notify_enabled(Some(&RowKey::Workgroup("w".into()))));
        assert!(!notify_enabled(None));
        assert!(settings_enabled(GroupMode::Status));
        assert!(!settings_enabled(GroupMode::Window));
        assert!(!settings_enabled(GroupMode::Workgroup));
        assert_eq!(SETTINGS_A11Y, "Settings");
        assert_eq!(NOTIFY_A11Y, "Notify on status change");
        assert_eq!(NOTIFY_ARMED_A11Y, "Notify on status change armed");
        assert_eq!((SETTINGS_W, GROUP_W, NOTIFY_W), (33.0, 100.0, 33.0));
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

    #[test]
    fn status_color_follows_iterm2() {
        let st = |color: Option<&str>, ind: Option<&str>| Status { status: Some("x".into()), status_color: color.map(str::to_string), indicator: ind.map(str::to_string), detail: None };
        assert_eq!(status_tint(&st(Some("#ff9500"), None)), Tint::Rgb(255, 149, 0));
        assert_eq!(status_tint(&st(None, Some("#5f87ff"))), Tint::Rgb(95, 135, 255));
        assert_eq!(status_tint(&st(Some("#ff9500"), Some("#5f87ff"))), Tint::Rgb(255, 149, 0));
        assert_eq!(status_tint(&st(None, None)), Tint::Secondary);
        // A bad color text falls through to the next rule.
        assert_eq!(status_tint(&st(Some("orange"), Some("#5f87ff"))), Tint::Rgb(95, 135, 255));
        assert_eq!(status_tint(&st(Some("orange"), Some("nope"))), Tint::Secondary);
    }

    #[test]
    fn detail_markdown_is_one_line() {
        let sp = |text: &str, bold: bool, italic: bool, code: bool| Span { text: text.into(), bold, italic, code };
        assert_eq!(
            detail_spans("Allow **Bash**: `ls`\nnext"),
            vec![sp("Allow ", false, false, false), sp("Bash", true, false, false), sp(": ", false, false, false), sp("ls", false, false, true), sp(" next", false, false, false)]
        );
        assert_eq!(detail_spans("*x*"), vec![sp("x", false, true, false)]);
        assert_eq!(detail_spans("# H"), vec![sp("H", false, false, false)]);
        assert_eq!(detail_spans(""), Vec::<Span>::new());
        // Blocks join with a space; the text a program sent loses its control characters.
        assert_eq!(detail_spans("one\n\ntwo"), vec![sp("one two", false, false, false)]);
        assert_eq!(detail_spans("a\u{1b}[2Jb\u{202e}c"), vec![sp("a[2Jbc", false, false, false)]);
    }

    #[test]
    fn filter_bar_follows_iterm2() {
        let counts = [("Waiting".to_string(), 2), ("working".to_string(), 1), ("No status".to_string(), 1)];
        assert_eq!(
            filter_items(5, &counts),
            vec![(None, "All (5)".to_string()), (Some("Waiting".to_string()), "Waiting (2)".to_string()), (Some("working".to_string()), "working (1)".to_string()), (Some("No status".to_string()), "No status (1)".to_string())]
        );
        assert_eq!(filter_items(0, &[]), vec![(None, "All (0)".to_string())]);
        // A status a program sent is shown cleaned; the filter key stays as sent.
        assert_eq!(filter_items(1, &[("a\u{7}b".to_string(), 1)])[1], (Some("a\u{7}b".to_string()), "ab (1)".to_string()));
        let short: Vec<String> = ["All (5)", "Waiting (2)"].iter().map(|s| s.to_string()).collect();
        assert!(filter_fits(&short, 8.0, 800.0));
        // (7 * 10 + 16) + (11 * 10 + 16) = 212
        assert!(filter_fits(&short, 10.0, 212.0));
        assert!(!filter_fits(&short, 10.0, 211.0));
        assert_eq!(FILTER_POPUP_MIN, 160.0);
    }

    #[test]
    fn jumps_follow_iterm2() {
        assert_eq!(jump_for(&RowKey::Session(3)), Jump::Session(3));
        assert_eq!(jump_for(&RowKey::Window("pty-A".into())), Jump::Window("pty-A".into()));
        assert_eq!(jump_for(&RowKey::Tab(12)), Jump::Tab(12));
        assert_eq!(jump_for(&RowKey::Group { scope: "pty-A".into(), status: "idle".into() }), Jump::Nothing);
        assert_eq!(jump_for(&RowKey::BuriedRoot), Jump::Nothing);
        assert_eq!(jump_for(&RowKey::Workgroup("w".into())), Jump::Nothing);
    }

    #[test]
    fn rows_have_iterm2_heights_and_glyphs() {
        assert_eq!((ROW_HEIGHT, ROW_HEIGHT_DETAIL), (24.0, 38.0));
        let glyphs = [WINDOW_GLYPH, TAB_GLYPH, SESSION_GLYPH, BURIED_GLYPH, WORKGROUP_GLYPH, BELL_GLYPH, BELL_ARMED_GLYPH];
        for (i, g) in glyphs.iter().enumerate() {
            assert!(('\u{E000}'..='\u{F8FF}').contains(g), "glyph {i} is in the icon range");
            assert!(!glyphs[..i].contains(g), "glyph {i} is distinct");
        }
        assert_eq!(SESSION_GLYPH, '\u{E756}');
    }

    #[test]
    fn selection_follows_clicks() {
        use RowKey::*;
        let w = Window("A".into());
        let g = Group { scope: "A".into(), status: "idle".into() };
        let visible = vec![w.clone(), g.clone(), Session(1), Session(2), Session(3), Tab(7), Session(4)];
        // A plain click chooses one row.
        assert_eq!(select(&visible, &[Session(1)], &Session(3), false, false), vec![Session(3)]);
        // Ctrl adds, and removes a row already chosen.
        assert_eq!(select(&visible, &[Session(1)], &Session(3), true, false), vec![Session(1), Session(3)]);
        assert_eq!(select(&visible, &[Session(1), Session(3)], &Session(1), true, false), vec![Session(3)]);
        // Shift extends from the first chosen row over the rows the list shows, skipping the
        // ones that cannot be chosen.
        assert_eq!(select(&visible, &[Session(1)], &Session(4), false, true), vec![Session(1), Session(2), Session(3), Tab(7), Session(4)]);
        assert_eq!(select(&visible, &[Session(3)], &w, false, true), vec![Session(3), Session(2), Session(1), w.clone()]);
        assert_eq!(select(&visible, &[], &Session(2), false, true), vec![Session(2)]);
        // A row that cannot be chosen changes nothing.
        assert_eq!(select(&visible, &[Session(1)], &g, false, false), vec![Session(1)]);
        assert_eq!(select(&visible, &[Session(1)], &g, true, true), vec![Session(1)]);
    }
}
