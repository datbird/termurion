//! iTerm2's tab bar without drawing (milestone 13c): the metrics, the width rules, scrolling,
//! the overflow menu, one tab's inside and the title, number, tooltip and color texts. Pure code
//! in points; the app multiplies by the window scale once.
//!
//! Citations: `PSM` = `ThirdParty/PSMTabBarControl/source/PSMTabBarControl.m`, `YS` =
//! `.../PSMYosemiteTabStyle.m`, `NSA` = `.../NSAttributedString+PSM.m`, `CT` =
//! `.../PSMCachedTitle.m` under `~/gitrepos/iterm2-ref`; `RTV` =
//! `sources/TerminalView/iTermRootTerminalView.m`, `PT` = `sources/TerminalView/PseudoTerminal.m`,
//! `TAB` = `sources/TerminalView/PTYTab.m`, `CMV` =
//! `sources/Infrastructure/Views/ColorsMenuItemView.m`.

use crate::layout::Rect;
use crate::palette::{rgb, Rgb};

/// `kSPMTabBarCellInternalXMargin` (`PSM:45`): the room at each end of a tab.
pub const CELL_MARGIN: f32 = 6.0;
/// `kPSMTabBarCellPadding` (`PSM:48`).
pub const CELL_PADDING: f32 = 4.0;
/// `kPSMTabBarCellIconPadding` (`PSM:49`).
pub const ICON_PADDING: f32 = 4.0;
/// `kPSMTabBarIconWidth` (`PSM:53`).
pub const ICON_WIDTH: f32 = 16.0;
/// `kPSMTabBarIndicatorWidth` (`PSM:52`): the activity spinner.
pub const INDICATOR_WIDTH: f32 = 16.0;
/// The close button, `TabClose_Front.png` (16 by 16).
pub const CLOSE_SIZE: f32 = 16.0;
/// `kPSMMinimumTitleWidth` (`PSM:51`).
pub const MIN_TITLE_WIDTH: f32 = 30.0;
/// `cellMaxWidth` (`PSM:304`).
pub const MAX_WIDTH: f32 = 280.0;
/// The bar's right margin when it holds the overflow or the add-tab button
/// (`YS rightMarginForTabBarControlWithOverflow:addTabButton:`).
pub const RIGHT_MARGIN: f32 = 24.0;
/// The add-tab button is 24 by 24, right after the last tab.
pub const ADD_BUTTON: f32 = 24.0;
/// `kPSMMetalObjectCounterRadius`: the tab number's box is twice this high.
pub const COUNTER_RADIUS: f32 = 7.0;
/// `kPSMMetalCounterMinWidth`.
pub const COUNTER_MIN_WIDTH: f32 = 20.0;
/// Titles longer than this many characters are cut (`CT:24`).
pub const TITLE_LIMIT: usize = 255;

/// The width preferences the bar is laid out with (`RTV:1984-1990`), in points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Prefs {
    /// `UseUnevenTabs`: tabs take their desired widths when all of them fit.
    pub uneven: bool,
    /// `cellMinWidth`, from `min_width`.
    pub min_width: f32,
    /// `OptimumTabWidth`.
    pub optimum: f32,
    /// `ScrollableTabWidth`: each tab's width on a bar that scrolls.
    pub scrollable_width: f32,
    /// `StretchTabsToFillBar`.
    pub stretch: bool,
    /// `ScrollableSideTabBar`: tabs that do not fit scroll instead of going to the overflow menu.
    pub scrollable: bool,
    /// The add-tab button is shown (horizontal bars unless `removeAddTabButton`).
    pub add_button: bool,
}

/// The minimum tab width: `MinCompactTabWidth` when tab numbers are hidden, else `MinTabWidth`
/// (`RTV:572-577`).
pub fn min_width(hide_numbers: bool, min_tab: f32, min_compact: f32) -> f32 {
    if hide_numbers {
        min_compact
    } else {
        min_tab
    }
}

/// The bar's right margin: room for the overflow or the add-tab button, else none
/// (`YS rightMarginForTabBarControlWithOverflow:addTabButton:`).
pub fn right_margin(overflow: bool, add_button: bool) -> f32 {
    if overflow || add_button {
        RIGHT_MARGIN
    } else {
        0.0
    }
}

/// The laid-out tab widths. `widths` is a prefix of the tabs; the rest go to the overflow menu.
#[derive(Clone, Debug, PartialEq)]
pub struct Widths {
    pub widths: Vec<f32>,
    /// Some tabs did not fit and are in the overflow menu.
    pub overflow: bool,
    /// The scroll extent (the left margin plus every tab); 0 when nothing scrolls.
    pub content: f32,
}

/// Lays out tabs with the given desired widths (`YS desiredWidthOfCell`) on a bar `bar_width`
/// points wide; `scale` is the device pixels per point that equal shares are rounded to (iTerm2
/// passes 2). `pinned` (parallel to `desired`; empty for none) marks the pinned tabs, which are
/// `pinned_width` wide whatever the rule (`PinnedTabWidth`).
pub fn widths(desired: &[f32], pinned: &[bool], pinned_width: f32, bar_width: f32, p: &Prefs, scale: f32) -> Widths {
    let n = desired.len();
    let is_pinned = |i: usize| pinned.get(i).copied().unwrap_or(false);
    // The left margin is the bar's inset, 0 here (`YS leftMarginForTabBarControl`).
    let available = |overflow: bool| bar_width - right_margin(overflow, p.add_button);
    if p.scrollable && n > 0 {
        // `PSM layoutScrollableHorizontalTabsWithCellCount:animate:`: when the natural widths
        // pass the room they scroll, else the bar looks as it does without scrolling
        // (`PSM naturalHorizontalCellWidths`).
        let natural: Vec<f32> = desired
            .iter()
            .enumerate()
            .map(|(i, &d)| {
                if is_pinned(i) {
                    pinned_width
                } else if p.uneven {
                    p.scrollable_width.max(MAX_WIDTH.min(d))
                } else {
                    p.scrollable_width
                }
            })
            .collect();
        let total: f32 = natural.iter().sum();
        if total > available(false) {
            return Widths { widths: natural, overflow: false, content: total };
        }
    }
    // `PSM settledHorizontalCellWidths`: the overflow variant when the regular widths drop tabs
    // (or the add-tab button already takes the same margin).
    let pinned: Vec<bool> = (0..n).map(is_pinned).collect();
    let arrange = |room: f32| arrange(desired, &pinned, pinned_width, room, p, scale);
    let regular = arrange(available(false));
    let widths = if p.add_button || regular.len() < n { arrange(available(true)) } else { regular };
    let overflow = widths.len() < n;
    Widths { widths, overflow, content: 0.0 }
}

/// `PSM cellWidthsForHorizontalArrangementWithOverflow:` without group chips (`chip_widths`) and with
/// iTerm2's Yosemite intercell spacing of 0. Pinned tabs are `pinned_width` wide and come off
/// the room first; the unpinned tabs share the rest by the usual rule. The result is a prefix
/// of the tabs: at the first tab that does not fit, it and the rest overflow.
fn arrange(desired: &[f32], pinned: &[bool], pinned_width: f32, available: f32, p: &Prefs, scale: f32) -> Vec<f32> {
    let n = desired.len();
    let pinned_count = pinned.iter().filter(|p| **p).count();
    let unpinned = n - pinned_count;
    if p.uneven {
        // `PSM variableCellWidthsWithOverflow:`: only when every tab fits at its clamped width.
        let clamped: Vec<f32> =
            desired.iter().zip(pinned).map(|(&d, &pin)| if pin { pinned_width } else { p.min_width.max(d.min(MAX_WIDTH)) }).collect();
        if clamped.iter().sum::<f32>() <= available {
            return clamped;
        }
    }
    // `PSM totalPinnedSpaceForPinnedCount:unpinnedCount:` with no spacing.
    let room = (available - pinned_count as f32 * pinned_width).max(0.0);
    // `PSM shouldUseOptimalWidthWithOverflow:`.
    let shares = if unpinned == 0 || (pinned_count > 0 && room <= 0.0) {
        Vec::new()
    } else if !p.stretch && p.optimum * unpinned as f32 <= room {
        vec![p.optimum; unpinned]
    } else {
        let visible = if p.min_width * unpinned as f32 <= room {
            unpinned
        } else if p.min_width > 0.0 {
            ((room / p.min_width).floor().max(0.0) as usize).min(unpinned)
        } else {
            unpinned
        };
        equal_shares(room, visible, scale)
    };
    // `PSM:3180-3202`: the tabs in order until one does not fit.
    let mut out = Vec::with_capacity(n);
    let (mut used, mut next) = (0.0, 0);
    for &pin in pinned {
        if pin {
            if used + pinned_width > available {
                break;
            }
            out.push(pinned_width);
            used += pinned_width;
        } else {
            let Some(&w) = shares.get(next) else { break };
            out.push(w);
            next += 1;
        }
    }
    out
}

/// `PSM computeCellFramesInContainerOfWidth:...`: `room` shared by `n` tabs in whole device
/// pixels, the remainder one pixel each to the first tabs.
fn equal_shares(room: f32, n: usize, scale: f32) -> Vec<f32> {
    if n == 0 {
        return Vec::new();
    }
    let total = ((room * scale).round() as i64).max(0);
    let base = total / n as i64;
    let rem = (total % n as i64) as usize;
    (0..n).map(|i| (base + (i < rem) as i64) as f32 / scale).collect()
}

/// The scroll offset kept within `0..=content - viewport` (`PSM clampScrollOffset`).
pub fn clamp_scroll(offset: f32, content: f32, viewport: f32) -> f32 {
    offset.min(max_scroll(content, viewport)).max(0.0)
}

/// `PSM maximumScrollOffset`: 0 when everything fits.
fn max_scroll(content: f32, viewport: f32) -> f32 {
    (content - viewport).max(0.0)
}

/// The offset that brings the tab at content position `x`, `w` wide, fully into the viewport
/// (`PSM scrollSelectedTabIntoView`).
pub fn scroll_into_view(offset: f32, x: f32, w: f32, viewport: f32, content: f32) -> f32 {
    // The leading margin is the bar's left inset, 0 here.
    let wanted = if x - offset < 0.0 {
        x
    } else if x + w - offset > viewport {
        x + w - viewport
    } else {
        return offset;
    };
    clamp_scroll(wanted, content, viewport)
}

/// A notched wheel over the bar (`PSM scrollWheel:`), deltas in lines. None when the event is
/// not the bar's: the bar does not scroll, or a plain vertical wheel. Else the new offset.
pub fn wheel_scroll(
    offset: f32,
    dx_lines: f32,
    dy_lines: f32,
    shift: bool,
    optimum: f32,
    content: f32,
    viewport: f32,
) -> Option<f32> {
    if max_scroll(content, viewport) <= 0.0 {
        return None;
    }
    // A horizontal delta, or Shift with a vertical one; a plain vertical wheel goes on to
    // whatever is behind the bar.
    let lines = if dx_lines != 0.0 {
        dx_lines
    } else if shift && dy_lines != 0.0 {
        dy_lines
    } else {
        return None;
    };
    // A notched wheel moves one optimum tab width per line.
    Some(clamp_scroll(offset - lines * optimum, content, viewport))
}

/// A precise (pixel) wheel delta in lines of one optimum tab width, so it scrolls by its own
/// distance; a line is at least one pixel, so a zero `OptimumTabWidth` gives no NaN.
pub fn pixel_lines(px_x: f64, px_y: f64, optimum: f32, scale: f32) -> (f32, f32) {
    let per_line = (optimum * scale).max(1.0);
    (px_x as f32 / per_line, px_y as f32 / per_line)
}

/// The tab number's modifier for `SwitchTabModifier` in Windows words, in iTerm2's glyph order
/// (`PSM _modifierString`: Control, Option, Shift, then Cmd). Empty with no shortcut.
pub fn modifier_string(switch_tab_modifier: i64) -> &'static str {
    match switch_tab_modifier {
        1 => "Ctrl+",
        4 => "Win+",
        5 => "Alt+",
        6 => "Alt+Win+",
        _ => "",
    }
}

/// The tab number's text (`YS attributedObjectCountValueForTabCell:`): the modifier and the
/// number for tabs 1 to 8, the modifier and "9" on the last tab, else empty. The counter keeps
/// its room when the text is empty.
pub fn counter_text(number: usize, is_last: bool, modifier: &str) -> String {
    if modifier.is_empty() {
        String::new()
    } else if number < 9 {
        crate::shortcut_key::mode().show(&format!("{modifier}{number}"))
    } else if is_last {
        crate::shortcut_key::mode().show(&format!("{modifier}9"))
    } else {
        String::new()
    }
}

/// The tab number's box width for its text width (`YS objectCounterRectForTabCell:`).
pub fn counter_width(text_width: f32) -> f32 {
    COUNTER_MIN_WIDTH.max(text_width + 2.0 * COUNTER_RADIUS - 6.0)
}

/// A tab's desired width (`YS desiredWidthOfTabCell:`): the left matter (margin, the close
/// button's room whether or not it is shown, the icon), the title, the right matter (the counter,
/// or else the close button's room again, and the indicator) and the right margin. `counter` is
/// the counter's box width.
pub fn desired_width(title_width: f32, icon: bool, counter: Option<f32>, indicator: bool) -> f32 {
    let mut left = CELL_MARGIN + CLOSE_SIZE + CELL_PADDING;
    if icon {
        left += ICON_WIDTH + ICON_PADDING;
    }
    // `YS widthOfRightMatterInCell:`.
    let mut right = match counter {
        Some(w) => w + CELL_PADDING,
        None => CLOSE_SIZE + CELL_PADDING,
    };
    if indicator {
        right += CELL_PADDING + INDICATOR_WIDTH;
    }
    right += CELL_MARGIN;
    (left + title_width + right).ceil()
}

/// What one tab holds, in points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CellInput {
    pub x: f32,
    pub width: f32,
    pub height: f32,
    /// `TabsHaveCloseButton`.
    pub close_buttons: bool,
    /// `TabCloseButtonsAlwaysVisible`; off, the button shows only on hover.
    pub close_always: bool,
    pub hovered: bool,
    /// The counter's box width (`counter_width`); None when the tab has no number.
    pub counter: Option<f32>,
    /// The tab has an icon (new output, bell, dead, idle, the status dot).
    pub icon: bool,
    /// The activity spinner shows.
    pub indicator: bool,
    /// The title's measured width.
    pub title_width: f32,
    /// `MinimumTabLabelWidth`: the icon is dropped unless the label keeps more than this.
    pub min_label: f32,
    /// A pinned tab: no close button; its pin takes the close button's room (`YS:854-915`).
    pub pinned: bool,
    /// The session's icon picture shows (`cachedTitle.inputs.graphic`, `YS:932-941`).
    pub graphic: bool,
    /// A side bar's row: the close button and the icon move the label instead of reserving
    /// room, and the label is left-aligned.
    pub vertical: bool,
}

/// Where one tab's parts go (`YS drawInteriorWithTabCell:inView:highlightAmount:`), in points.
/// Rectangles have y 0 at the tab's top.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CellLayout {
    pub close: Option<Rect>,
    /// 0 or 1 (iTerm2 fades by the hover amount); the drawer halves it in an inactive window.
    pub close_alpha: f32,
    pub icon: Option<Rect>,
    pub indicator: Option<Rect>,
    pub counter: Option<Rect>,
    /// The label's box, the tab's full height: the drawer places the text line(s) in it.
    pub label: Rect,
    /// The label is too long to center: it is drawn left-aligned and truncated. `build` also
    /// sets it for a pinned tab's single character, which iTerm2 draws left-aligned, and
    /// `cell` for every side bar row, whose labels iTerm2 aligns left (`PSMCachedTitle.m:213-217`).
    pub truncate: bool,
    /// Room on the left kept for a close button that shows only on hover, or for the icon
    /// picture.
    pub reserved: f32,
    /// The icon picture's box, over the close button's place (`YS drawGraphicWithCellFrame:`).
    pub graphic: Option<Rect>,
}

/// Lays out one tab's inside.
pub fn cell(input: &CellInput) -> CellLayout {
    let (x, w, h) = (input.x, input.width, input.height);
    let centered = |size: f32| ((h - size) / 2.0).floor();
    let mut label_x = x + CELL_MARGIN;
    let mut reserved = 0.0;
    let mut close = None;
    let mut close_alpha = 0.0;
    if input.pinned {
        // `YS drawInteriorWithTabCell:`: room for the pin where the close button would be.
        label_x += CLOSE_SIZE + CELL_PADDING;
    } else if input.close_buttons {
        close = Some(Rect { x: x + CELL_MARGIN, y: centered(CLOSE_SIZE), w: CLOSE_SIZE, h: CLOSE_SIZE });
        close_alpha = if input.close_always || input.hovered { 1.0 } else { 0.0 };
        if input.close_always || input.vertical {
            // `YS:889-891`: the label starts after the button's room, on a side bar even when
            // the button shows only on hover.
            label_x += CLOSE_SIZE + CELL_PADDING;
        } else {
            // `YS:880-888`, suppressed on a horizontal bar: the label is centered as though
            // this room were free, unless it would run into it.
            reserved = CLOSE_SIZE + CELL_PADDING;
        }
    }
    // `YS:932-941`: the icon picture 6 points in, centered, 16 square; a horizontal label keeps
    // clear of it when truncated, a side bar's label starts after it.
    let graphic = input.graphic.then(|| Rect { x: x + CELL_MARGIN, y: (h - ICON_WIDTH) / 2.0, w: ICON_WIDTH, h: ICON_WIDTH });
    if graphic.is_some() {
        let width = ICON_WIDTH + CELL_PADDING + 2.0;
        if input.vertical {
            label_x = label_x.max(x + width + CELL_PADDING);
        } else {
            reserved = f32::max(reserved, width);
        }
    }
    let counter = input.counter.map(|cw| Rect {
        x: x + w - CELL_MARGIN - cw,
        y: centered(2.0 * COUNTER_RADIUS),
        w: cw,
        h: 2.0 * COUNTER_RADIUS,
    });
    // `YS indicatorRectForTabCell:`.
    let indicator = input.indicator.then(|| {
        let min_x = counter.map_or(x + w - CELL_MARGIN, |c| c.x);
        Rect { x: min_x - ICON_PADDING - INDICATOR_WIDTH, y: centered(INDICATOR_WIDTH), w: INDICATOR_WIDTH, h: INDICATOR_WIDTH }
    });
    // `YS widthForLabelInCell:...`: the label's width and whether it truncates, given the width
    // of the icon's rect when an icon is drawn.
    let label_width = |icon_w: Option<f32>| {
        let mut lw = w - (label_x - x) - CELL_PADDING;
        if let Some(iw) = icon_w {
            lw -= iw + ICON_PADDING;
        } else if input.indicator {
            lw -= INDICATOR_WIDTH + ICON_PADDING;
        }
        if let Some(cw) = input.counter {
            lw -= cw + CELL_PADDING;
        }
        // `YS:1146-1152`: only a horizontal label gives way to the reserved room. A side
        // bar's label is always drawn left-aligned (`PSMCachedTitle.m:213-217`).
        if !input.vertical && (lw - input.title_width) / 2.0 < reserved {
            (lw - reserved, true)
        } else {
            (lw, input.vertical)
        }
    };
    // `YS:951-979`: iTerm2 asks with the icon's rect still zero, so only its padding counts.
    let icon = (input.icon && label_width(Some(0.0)).0 > input.min_label).then(|| {
        // `YS iconRectForTabCell:`: one point above center.
        let min_x = match (counter, indicator) {
            (Some(c), _) => c.x,
            (None, Some(i)) => i.x - CELL_MARGIN,
            (None, None) => x + w - CELL_MARGIN,
        };
        Rect { x: min_x - ICON_PADDING - ICON_WIDTH, y: centered(ICON_WIDTH) - 1.0, w: ICON_WIDTH, h: ICON_WIDTH }
    });
    let (lw, truncate) = label_width(icon.map(|r| r.w));
    // A truncated label is drawn after the reserved room.
    let shift = if truncate { reserved } else { 0.0 };
    let label = Rect { x: label_x + shift, y: 0.0, w: lw.max(0.0), h };
    CellLayout { close, close_alpha, icon, indicator, counter, label, truncate, reserved, graphic }
}

/// Which end of a long title gives way.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Truncation {
    /// "\u{2026}end of the title"
    Head,
    /// "start of the title\u{2026}"
    Tail,
}

/// Leading whitespace, symbols and punctuation (a spinner such as "\u{2810} ") are not
/// part of a smart-truncation prefix. Private-use characters (Nerd Font icons) are text to
/// Apple's character sets, so they are kept.
fn is_decoration(c: char) -> bool {
    let private = matches!(c, '\u{e000}'..='\u{f8ff}' | '\u{f0000}'..='\u{10ffff}');
    c.is_whitespace() || !(c.is_alphanumeric() || c.is_control() || private)
}

/// The first `n` characters after the leading decoration, or the raw first `n` when the title is
/// all decoration (`PSMSmartTruncationPrefix`, `PSM:759-787`).
pub fn smart_prefix(title: &str, n: usize) -> String {
    let chars: Vec<char> = title.chars().collect();
    match chars.iter().position(|&c| !is_decoration(c)) {
        Some(start) => chars[start..].iter().take(n).collect(),
        None => chars.iter().take(n).collect(),
    }
}

/// `PSM truncationStyle`: head truncation when more titles have unique 5-character suffixes
/// than prefixes; suffixes are also counted before a trailing " (...)" (`TBV:47` turns that on)
/// and the larger count is used. Titles under 5 characters are skipped.
pub fn truncation(titles: &[&str], smart: bool) -> Truncation {
    use std::collections::HashSet;
    const LEN: usize = 5;
    if titles.len() <= 1 || !smart {
        return Truncation::Tail;
    }
    let mut prefixes = HashSet::new();
    let mut suffixes = HashSet::new();
    let mut before_parens = HashSet::new();
    for title in titles {
        let chars: Vec<char> = title.chars().collect();
        if chars.len() < LEN {
            continue;
        }
        prefixes.insert(smart_prefix(title, LEN));
        suffixes.insert(chars[chars.len() - LEN..].iter().collect::<String>());
        if title.ends_with(')') {
            let open = chars.windows(2).rposition(|p| p == [' ', '(']);
            if let Some(open) = open.filter(|&o| o > LEN) {
                before_parens.insert(chars[open - LEN..open].iter().collect::<String>());
            }
        }
    }
    if prefixes.len() >= suffixes.len().max(before_parens.len()) {
        Truncation::Tail
    } else {
        Truncation::Head
    }
}

/// `text` cut to fit `width` with an ellipsis on the cut side, keeping whole characters;
/// unchanged when it fits.
pub fn truncate(text: &str, width: f32, how: Truncation, measure: &dyn Fn(&str) -> f32) -> String {
    if width <= 0.0 {
        return String::new();
    }
    if measure(text) <= width {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    let with = |keep: usize| -> String {
        match how {
            Truncation::Tail => chars[..keep].iter().chain(['\u{2026}'].iter()).collect(),
            Truncation::Head => ['\u{2026}'].iter().chain(chars[chars.len() - keep..].iter()).collect(),
        }
    };
    // The most characters that fit, by bisection (widths grow with the characters kept).
    let (mut lo, mut hi) = (0, chars.len() - 1);
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        if measure(&with(mid)) <= width {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    let best = with(lo);
    if lo == 0 && measure(&best) > width {
        String::new()
    } else {
        best
    }
}

/// A stretch of title text in one style.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Run {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
}

/// An HTML tab title's styled runs (`NSA newAttributedStringWithHTML:attributes:`). A title is
/// HTML only when it holds "<b>", "<i>" or "<u>" (exactly, as iTerm2's `containsString:`);
/// otherwise it is one plain run as written. In HTML, the six tags are found in any case, but
/// only the lower-case ones toggle their style; "&lt;" and "&amp;" are decoded between tags.
pub fn html_runs(title: &str) -> Vec<Run> {
    const OPEN: [&str; 3] = ["<b>", "<i>", "<u>"];
    const CLOSE: [&str; 3] = ["</b>", "</i>", "</u>"];
    let plain = |text: &str| Run { text: text.to_string(), bold: false, italic: false, underline: false };
    if !OPEN.iter().any(|t| title.contains(t)) {
        return if title.is_empty() { Vec::new() } else { vec![plain(title)] };
    }
    let mut runs: Vec<Run> = Vec::new();
    let mut style = [false; 3];
    let mut push = |text: &str, style: [bool; 3]| {
        let text = text.replace("&lt;", "<").replace("&amp;", "&");
        if text.is_empty() {
            return;
        }
        match runs.last_mut() {
            Some(r) if [r.bold, r.italic, r.underline] == style => r.text.push_str(&text),
            _ => runs.push(Run { text, bold: style[0], italic: style[1], underline: style[2] }),
        }
    };
    let bytes = title.as_bytes();
    let (mut cursor, mut i) = (0, 0);
    while i < bytes.len() {
        let tag = OPEN.iter().chain(CLOSE.iter()).find(|t| {
            bytes.get(i..i + t.len()).is_some_and(|b| b.eq_ignore_ascii_case(t.as_bytes()))
        });
        let Some(tag) = tag else {
            i += 1;
            continue;
        };
        push(&title[cursor..i], style);
        let found = &title[i..i + tag.len()];
        if let Some(k) = OPEN.iter().position(|t| *t == found) {
            style[k] = true;
        } else if let Some(k) = CLOSE.iter().position(|t| *t == found) {
            style[k] = false;
        }
        i += tag.len();
        cursor = i;
    }
    push(&title[cursor..], style);
    runs
}

/// The title's text without its HTML when HTML titles are on (`removingHTMLFromTabTitleIfNeeded`).
pub fn plain_title(title: &str, html: bool) -> String {
    if html {
        html_runs(title).into_iter().map(|r| r.text).collect()
    } else {
        title.to_string()
    }
}

/// Titles past `TITLE_LIMIT` characters keep that many and an ellipsis (`CT:24`).
pub fn limit_title(title: &str) -> String {
    if title.chars().count() <= TITLE_LIMIT {
        title.to_string()
    } else {
        title.chars().take(TITLE_LIMIT).chain(['\u{2026}']).collect()
    }
}

/// The tab's subtitle line (`TAB stringByAppendingSubtitleForActiveSession:`): the status text
/// before the session's subtitle, when `ShowSessionStatusInTabSubtitle` is on.
pub fn subtitle_text(subtitle: &str, status: Option<&str>, show_status: bool) -> String {
    match status.filter(|s| !s.is_empty() && show_status) {
        Some(s) if subtitle.is_empty() => s.to_string(),
        Some(s) => format!("{s} \u{2013} {subtitle}"),
        None => subtitle.to_string(),
    }
}

/// The tab's tooltip (`PT tabView:toolTipForTabViewItem:`); `name` is already without HTML.
pub fn tooltip(name: &str, profile: &str, command: Option<&str>) -> String {
    format!("Name: {name}\nProfile: {profile}\nCommand: {}", command.unwrap_or("None"))
}

/// iTerm2's fallback tab colors (`CMV:488-498`), also `TabColorMenuOptions`' default.
pub const DEFAULT_COLORS: [Rgb; 7] = [
    rgb(0xfb, 0x6b, 0x62),
    rgb(0xf6, 0xac, 0x47),
    rgb(0xf0, 0xdc, 0x4f),
    rgb(0xb5, 0xd7, 0x49),
    rgb(0x5f, 0xa3, 0xf8),
    rgb(0xc1, 0x8e, 0xd9),
    rgb(0x78, 0x78, 0x78),
];

/// The tab color menu's colors from `TabColorMenuOptions` (`CMV colors`): the spec split on
/// spaces, each word read by `NSColor colorFromHexString:`, words it cannot read skipped, and
/// `DEFAULT_COLORS` when none is left.
pub fn tab_colors(spec: &str) -> Vec<Rgb> {
    let colors: Vec<Rgb> = spec.split(' ').filter_map(crate::hexcolor::parse).collect();
    if colors.is_empty() {
        DEFAULT_COLORS.to_vec()
    } else {
        colors
    }
}

/// The label font's size without `UseCustomTabBarFontSize` (`YS fontSize`).
pub const DEFAULT_FONT: f32 = 11.0;
/// The subtitle line shows only on a bar this tall (`YS supportsMultiLineLabels`).
pub const SUBTITLE_MIN_BAR: f32 = 28.0;
/// The title moves up this much when a subtitle is drawn under it
/// (`YS verticalOffsetForTitleWhenSubtitlePresent`).
pub const TITLE_OFFSET_WITH_SUBTITLE: f32 = -5.0;
/// The subtitle sits this much closer to the title (`YS verticalOffsetForSubtitle`).
pub const SUBTITLE_OFFSET: f32 = -2.0;

/// The bar's height in points: `DefaultTabBarHeight` (`YS tabBarHeight`).
pub fn bar_height(default_height: f32) -> f32 {
    default_height.max(0.0)
}

/// The label font's size: `CustomTabBarFontSize` with `UseCustomTabBarFontSize`, else 11.
pub fn font_size(custom: bool, size: f32) -> f32 {
    if custom {
        size
    } else {
        DEFAULT_FONT
    }
}

/// The subtitle's font size (`YS subtitleFontSize`).
pub fn subtitle_size(font: f32) -> f32 {
    (font * 0.8).round()
}

/// A subtitle line is drawn: a tall enough bar and something to draw (`YS willDrawSubtitle:`).
pub fn shows_subtitle(bar_height: f32, subtitle: &str) -> bool {
    bar_height >= SUBTITLE_MIN_BAR && !subtitle.is_empty()
}

/// One tab as the bar lays it out.
#[derive(Clone, Debug, PartialEq)]
pub struct TabInput {
    /// The title as the session names it (HTML when `HTMLTabTitles` is on).
    pub title: String,
    /// The subtitle line, the status joined in (`subtitle_text`).
    pub subtitle: String,
    /// The tab shows an icon (the status dot, dead, bell, new output, idle).
    pub icon: bool,
    /// The activity spinner shows (`activity_ui::tab_processing`).
    pub processing: bool,
    /// The tab's number, from 1; 0 for none.
    pub number: usize,
    /// Pinned: `Settings::pinned_width` wide, with its pin and only its first character.
    pub pinned: bool,
    /// The tab's group; the run's first tab has the chip before it.
    pub group: Option<ChipInput>,
    /// Hidden in a collapsed group: no cell, its index kept (`isCollapsedHidden`).
    pub hidden: bool,
    /// The session's icon picture shows (`TabLook::icon_image`).
    pub graphic: bool,
    /// The letter icon's letter, measured for centering; "" for a picture or none.
    pub letter: String,
}

/// A grouped tab's group, as its chip needs it (`PSM cellsByInsertingTabGroupChipsInto:`).
#[derive(Clone, Debug, PartialEq)]
pub struct ChipInput {
    pub id: String,
    pub name: String,
    pub collapsed: bool,
    /// The members in the run (the collapsed chip's "Name  N").
    pub count: usize,
    /// The run's first tab: the chip goes before it.
    pub first: bool,
}

/// A group chip's text as drawn: its run of tabs (hidden members included), its label (the
/// name, or "Name  N" collapsed), truncated to the chip, and whether the group is collapsed.
#[derive(Clone, Debug, PartialEq)]
pub struct ChipText {
    pub run: std::ops::Range<usize>,
    pub label: String,
    pub label_width: f32,
    pub collapsed: bool,
}

/// A tab's texts as drawn, measured in points: the title's styled runs with their widths, the
/// subtitle's width and its own label box, and the number.
#[derive(Clone, Debug, PartialEq)]
pub struct CellText {
    pub runs: Vec<(Run, f32)>,
    pub subtitle_width: f32,
    /// The subtitle's label box and whether it is left-aligned and truncated; it is measured
    /// apart from the title (`YS drawSubtitle:...`).
    pub subtitle_label: Rect,
    pub subtitle_truncate: bool,
    pub counter: String,
    pub counter_width: f32,
    /// The letter icon's letter width at `scene::LETTER_PT`.
    pub letter_width: f32,
}

impl CellText {
    /// The title's width, all runs together.
    pub fn title_width(&self) -> f32 {
        self.runs.iter().map(|(_, w)| w).sum()
    }
}

/// The laid-out bar, in points from the bar's left, the scroll offset applied.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Bar {
    /// The shown tabs: tab index, inside, and the tab's rectangle.
    pub cells: Vec<(usize, CellLayout, Rect)>,
    /// Per cell, the title as drawn (plain text, truncated).
    pub titles: Vec<String>,
    /// Per cell, the subtitle as drawn; empty when none shows.
    pub subtitles: Vec<String>,
    /// Per cell, the texts measured.
    pub texts: Vec<CellText>,
    /// Tabs in the overflow menu.
    pub hidden: Vec<usize>,
    pub add: Option<Rect>,
    pub overflow: Option<Rect>,
    /// The scroll extent; 0 when nothing scrolls.
    pub content: f32,
    /// A tab's height; a side bar's is its whole height (a row is `Settings::height`).
    pub height: f32,
    /// The label font's size.
    pub font: f32,
    /// The group chips: group id and rectangle, one before each run.
    pub chips: Vec<(String, Rect)>,
    /// Per chip, its text.
    pub chip_texts: Vec<ChipText>,
    /// A side bar: the tabs are rows and scroll down, not along.
    pub vertical: bool,
    /// Draw state, set by the app on the bar it draws: the collapsed group a dragged tab would
    /// join if dropped now (`PSM collapsedTabGroupDropTargetIdentifier`), framed in the
    /// accent color.
    pub join: Option<String>,
    /// Draw state, set by the app: the edge the bar sits on, so the line between the bar and
    /// the content is drawn on the content's side.
    pub position: crate::tab_position::TabPosition,
}

/// The tab bar's settings (`RTV:1984-1990`, `PT:10112-10124`).
#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub prefs: Prefs,
    /// `HideTabNumber`.
    pub hide_numbers: bool,
    /// `TabsHaveCloseButton`.
    pub close_buttons: bool,
    /// `TabCloseButtonsAlwaysVisible`.
    pub close_always: bool,
    /// `HideActivityIndicator`.
    pub hide_activity: bool,
    /// `HTMLTabTitles`.
    pub html: bool,
    /// `TabTitlesUseSmartTruncation`.
    pub smart: bool,
    /// The label font's size in points; 13e sets it from its theme unless the custom size is on.
    pub font: f32,
    /// `MinimumTabLabelWidth`.
    pub min_label: f32,
    /// The tab number's modifier (`modifier_string`).
    pub modifier: &'static str,
    /// The bar's height in points.
    pub height: f32,
    /// `PinnedTabWidth`: a pinned tab's width in points.
    pub pinned_width: f32,
}

impl Settings {
    pub fn from_globals(g: &crate::globals::Globals) -> Settings {
        let num = |key: &str, default: f32| g.number(key).map_or(default, |v| v as f32);
        let hide_numbers = g.bool("HideTabNumber");
        Settings {
            prefs: Prefs {
                uneven: g.bool("UseUnevenTabs"),
                min_width: min_width(hide_numbers, num("MinTabWidth", 75.0), num("MinCompactTabWidth", 60.0)),
                optimum: num("OptimumTabWidth", 175.0),
                scrollable_width: num("ScrollableTabWidth", 120.0),
                stretch: g.bool("StretchTabsToFillBar"),
                scrollable: g.bool("ScrollableSideTabBar"),
                add_button: !g.bool("RemoveAddTabButton"),
            },
            hide_numbers,
            close_buttons: g.bool("TabsHaveCloseButton"),
            close_always: g.bool("TabCloseButtonsAlwaysVisible"),
            hide_activity: g.bool("HideActivityIndicator"),
            html: g.bool("HTMLTabTitles"),
            smart: g.bool("TabTitlesUseSmartTruncation"),
            font: font_size(g.bool("UseCustomTabBarFontSize"), num("CustomTabBarFontSize", DEFAULT_FONT)),
            min_label: num("MinimumTabLabelWidth", 35.0),
            modifier: modifier_string(g.int("SwitchTabModifier")),
            height: bar_height(num("DefaultTabBarHeight", 24.0)),
            pinned_width: num("PinnedTabWidth", crate::scene::PINNED_TAB_WIDTH),
        }
    }
}

impl Settings {
    /// The label font taken from the theme (`font_pt`: 12 for the High Contrast styles), unless
    /// `UseCustomTabBarFontSize` is on, when `CustomTabBarFontSize` stays.
    pub fn with_theme_font(self, g: &crate::globals::Globals, font_pt: f32) -> Settings {
        if g.bool("UseCustomTabBarFontSize") {
            self
        } else {
            Settings { font: font_pt, ..self }
        }
    }
}

/// A rectangle in points as device pixels.
pub fn to_px(r: Rect, scale: f32) -> Rect {
    Rect { x: r.x * scale, y: r.y * scale, w: r.w * scale, h: r.h * scale }
}

/// The styled runs of `truncated`, a cut of the runs' text by `truncate`: each kept character
/// keeps its style, the ellipsis takes its neighbor's.
fn cut_runs(runs: &[Run], truncated: &str, how: Truncation) -> Vec<Run> {
    // Nothing fits, not even the ellipsis: nothing is drawn.
    if truncated.is_empty() {
        return Vec::new();
    }
    let styled: Vec<(char, [bool; 3])> =
        runs.iter().flat_map(|r| r.text.chars().map(move |c| (c, [r.bold, r.italic, r.underline]))).collect();
    let plain: String = styled.iter().map(|(c, _)| c).collect();
    if plain == truncated {
        return runs.to_vec();
    }
    let kept = truncated.chars().count().saturating_sub(1);
    let chars: Vec<(char, [bool; 3])> = match how {
        Truncation::Tail => {
            let mut v = styled[..kept].to_vec();
            v.push(('\u{2026}', v.last().or(styled.first()).map_or([false; 3], |s| s.1)));
            v
        }
        Truncation::Head => {
            let tail = &styled[styled.len() - kept..];
            let mut v = vec![('\u{2026}', tail.first().or(styled.last()).map_or([false; 3], |s| s.1))];
            v.extend_from_slice(tail);
            v
        }
    };
    let mut out: Vec<Run> = Vec::new();
    for (c, st) in chars {
        match out.last_mut() {
            Some(r) if [r.bold, r.italic, r.underline] == st => r.text.push(c),
            _ => out.push(Run { text: c.to_string(), bold: st[0], italic: st[1], underline: st[2] }),
        }
    }
    out
}

/// The measured texts every layout shares: each tab's styled title runs (a horizontal pinned
/// tab keeps only its first character, `YS:1016-1027`), their plain text and width, its number
/// and the bar-wide truncation.
struct Texts {
    runs: Vec<Vec<Run>>,
    plains: Vec<String>,
    widths: Vec<f32>,
    counters: Vec<Option<(String, f32)>>,
    how: Truncation,
}

fn texts(tabs: &[TabInput], s: &Settings, measure: &dyn Fn(&str, f32, bool) -> f32, first_char_pins: bool) -> Texts {
    let n = tabs.len();
    let runs: Vec<Vec<Run>> = tabs
        .iter()
        .map(|t| {
            let title = limit_title(&t.title);
            if s.html {
                html_runs(&title)
            } else if title.is_empty() {
                Vec::new()
            } else {
                vec![Run { text: title, bold: false, italic: false, underline: false }]
            }
        })
        .collect();
    let plain_of = |runs: &[Run]| -> String { runs.iter().map(|r| r.text.as_str()).collect() };
    // The bar-wide rule reads every tab's whole title.
    let whole: Vec<String> = runs.iter().map(|r| plain_of(r)).collect();
    let how = truncation(&whole.iter().map(String::as_str).collect::<Vec<_>>(), s.smart);
    let runs: Vec<Vec<Run>> = runs
        .into_iter()
        .zip(tabs)
        .map(|(r, t)| {
            if !(t.pinned && first_char_pins) {
                return r;
            }
            r.iter()
                .find_map(|run| run.text.chars().next().map(|c| Run { text: c.to_string(), ..run.clone() }))
                .into_iter()
                .collect()
        })
        .collect();
    let plains: Vec<String> = runs.iter().map(|r| plain_of(r)).collect();
    let widths: Vec<f32> = plains.iter().map(|t| measure(t, s.font, false)).collect();
    let counters: Vec<Option<(String, f32)>> = tabs
        .iter()
        .enumerate()
        .map(|(i, t)| {
            // `TAB:1303-1305`: no count with `HideTabNumber`; a pinned tab has no room for one.
            (!s.hide_numbers && t.number > 0 && !t.pinned).then(|| {
                let text = counter_text(t.number, i + 1 == n, s.modifier);
                let w = measure(&text, s.font, false);
                (text, w)
            })
        })
        .collect();
    Texts { runs, plains, widths, counters, how }
}

/// Lays out the tab `i` in the cell `rect` (points, the bar's coordinates; the inside's y is
/// from the cell's top) and adds it to `bar`.
#[allow(clippy::too_many_arguments)]
fn place(bar: &mut Bar, i: usize, rect: Rect, t: &TabInput, hovered: bool, s: &Settings, x: &Texts, measure: &dyn Fn(&str, f32, bool) -> f32, first_char_pins: bool) {
    let (font, sub_font, how) = (s.font, subtitle_size(s.font), x.how);
    let mut layout = cell(&CellInput {
        x: rect.x,
        width: rect.w,
        height: rect.h,
        close_buttons: s.close_buttons,
        close_always: s.close_always,
        hovered,
        counter: x.counters[i].as_ref().map(|c| counter_width(c.1)),
        icon: t.icon,
        indicator: t.processing,
        title_width: x.widths[i],
        min_label: s.min_label,
        pinned: t.pinned,
        graphic: t.graphic,
        vertical: bar.vertical,
    });
    // A horizontal pinned tab's character is drawn whole and left-aligned (`YS:1018-1027`).
    let first_char = t.pinned && first_char_pins;
    layout.truncate |= first_char;
    let title = if first_char { x.plains[i].clone() } else { truncate(&x.plains[i], layout.label.w, how, &|text| measure(text, font, false)) };
    let placed: Vec<(Run, f32)> =
        cut_runs(&x.runs[i], &title, how).into_iter().map(|r| { let w = measure(&r.text, font, false); (r, w) }).collect();
    // The subtitle is measured on its own in the title's label room (`YS drawSubtitle:`).
    let (full_x, full_w) = if layout.truncate {
        (layout.label.x - layout.reserved, layout.label.w + layout.reserved)
    } else {
        (layout.label.x, layout.label.w)
    };
    // `YS:1044`: no subtitle on a pinned tab.
    let (subtitle, sub_w, sub_label, sub_truncate) = if !t.pinned && shows_subtitle(rect.h, &t.subtitle) {
        let whole = measure(&t.subtitle, sub_font, false);
        let cut = bar.vertical || (full_w - whole) / 2.0 < layout.reserved;
        let label = if cut {
            Rect { x: full_x + layout.reserved, y: 0.0, w: (full_w - layout.reserved).max(0.0), h: rect.h }
        } else {
            Rect { x: full_x, y: 0.0, w: full_w, h: rect.h }
        };
        let text = truncate(&t.subtitle, label.w, how, &|text| measure(text, sub_font, false));
        let w = measure(&text, sub_font, false);
        (text, w, label, cut)
    } else {
        (String::new(), 0.0, layout.label, false)
    };
    let (counter, counter_w) = x.counters[i].clone().unwrap_or_default();
    bar.cells.push((i, layout, rect));
    bar.titles.push(title);
    bar.subtitles.push(subtitle);
    bar.texts.push(CellText {
        runs: placed,
        subtitle_width: sub_w,
        subtitle_label: sub_label,
        subtitle_truncate: sub_truncate,
        counter,
        counter_width: counter_w,
        letter_width: if t.letter.is_empty() { 0.0 } else { measure(&t.letter, crate::scene::LETTER_PT, true) },
    });
}

/// One place in the bar: a group chip (the run's first tab, the chip's width) or a tab.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Slot {
    Chip(usize, f32),
    Tab(usize),
}

/// The chip's label: the name, or "Name  N" collapsed (`YS collapsedLabelForName:count:`).
fn chip_label(g: &ChipInput) -> String {
    if g.collapsed {
        crate::scene::collapsed_label(&g.name, g.count)
    } else {
        g.name.clone()
    }
}

/// The places in order: each tab, a chip before the first tab of each run
/// (`PSM cellsByInsertingTabGroupChipsInto:`), the chip `scene::chip_width` wide.
fn slots(tabs: &[TabInput], measure: &dyn Fn(&str, f32, bool) -> f32) -> Vec<Slot> {
    let mut out = Vec::with_capacity(tabs.len());
    for (i, t) in tabs.iter().enumerate() {
        if let Some(g) = t.group.as_ref().filter(|g| g.first) {
            let w = crate::scene::chip_width(measure(&chip_label(g), crate::scene::GROUP_NAME_PT, true), g.collapsed);
            out.push(Slot::Chip(i, w));
        }
        out.push(Slot::Tab(i));
    }
    out
}

/// The tabs of the run whose chip is before tab `first`: it and the following tabs of the same
/// group up to the next run's chip.
fn run_of(tabs: &[TabInput], first: usize) -> std::ops::Range<usize> {
    let id = tabs[first].group.as_ref().map(|g| g.id.as_str());
    let end = (first + 1..tabs.len())
        .find(|&j| tabs[j].group.as_ref().is_none_or(|g| g.first || Some(g.id.as_str()) != id))
        .unwrap_or(tabs.len());
    first..end
}

/// Adds the chip before tab `first` at `rect`, its label cut to `room` points.
fn place_chip(bar: &mut Bar, tabs: &[TabInput], first: usize, rect: Rect, room: f32, measure: &dyn Fn(&str, f32, bool) -> f32) {
    let Some(g) = tabs[first].group.as_ref() else { return };
    let pt = crate::scene::GROUP_NAME_PT;
    let label = truncate(&chip_label(g), room.max(0.0), Truncation::Tail, &|t| measure(t, pt, true));
    let label_width = measure(&label, pt, true);
    bar.chips.push((g.id.clone(), rect));
    bar.chip_texts.push(ChipText { run: run_of(tabs, first), label, label_width, collapsed: g.collapsed });
}

/// `PSM cellWidthsForHorizontalArrangementWithChipsWithOverflow:` and, on a bar that scrolls,
/// `naturalHorizontalCellWidths`, over `slots`: a chip and a pinned tab keep their widths, a
/// hidden tab has none (and never ends the prefix), the other tabs share the rest, each at
/// least the minimum and, unstretched, at most the optimum. The result is a prefix of the
/// slots; the rest overflow.
fn chip_widths(slots: &[Slot], tabs: &[TabInput], desired: &[f32], bar_width: f32, p: &Prefs, pinned_width: f32) -> Widths {
    let fixed = |sl: &Slot| match *sl {
        Slot::Chip(_, w) => Some(w),
        Slot::Tab(i) if tabs[i].hidden => Some(0.0),
        Slot::Tab(i) if tabs[i].pinned => Some(pinned_width),
        Slot::Tab(_) => None,
    };
    let desired_of = |sl: &Slot| match *sl {
        Slot::Tab(i) => desired[i],
        Slot::Chip(_, w) => w,
    };
    let available = |overflow: bool| bar_width - right_margin(overflow, p.add_button);
    if p.scrollable {
        let natural: Vec<f32> = slots
            .iter()
            .map(|sl| fixed(sl).unwrap_or_else(|| if p.uneven { p.scrollable_width.max(MAX_WIDTH.min(desired_of(sl))) } else { p.scrollable_width }))
            .collect();
        let total: f32 = natural.iter().sum();
        if total > available(false) {
            return Widths { widths: natural, overflow: false, content: total };
        }
    }
    let arrange = |room: f32| -> Vec<f32> {
        if p.uneven {
            // `PSM variableCellWidthsWithOverflow:`: every place at its clamped width, when all fit.
            let clamped: Vec<f32> = slots.iter().map(|sl| fixed(sl).unwrap_or_else(|| p.min_width.max(desired_of(sl).min(MAX_WIDTH)))).collect();
            if clamped.iter().sum::<f32>() <= room {
                return clamped;
            }
        }
        let reserved: f32 = slots.iter().filter_map(fixed).sum();
        let count = slots.iter().filter(|sl| fixed(sl).is_none()).count();
        let per_tab = if count > 0 {
            let share = (room - reserved).max(0.0) / count as f32;
            let share = if p.stretch { share } else { share.min(p.optimum) };
            share.max(p.min_width)
        } else {
            0.0
        };
        let mut out = Vec::with_capacity(slots.len());
        let (mut used, mut emitted) = (0.0, false);
        for sl in slots {
            if matches!(sl, Slot::Tab(i) if tabs[*i].hidden) {
                out.push(0.0);
                continue;
            }
            let w = fixed(sl).unwrap_or(per_tab);
            // A half point of rounding never pushes the last tab out (`fitTolerance`).
            if emitted && used + w > room + 0.5 {
                break;
            }
            out.push(w);
            used += w;
            emitted = true;
        }
        out
    };
    let regular = arrange(available(false));
    let widths = if p.add_button || regular.len() < slots.len() { arrange(available(true)) } else { regular };
    let overflow = widths.len() < slots.len();
    Widths { widths, overflow, content: 0.0 }
}

/// Lays out the bar (`PSM update:` with the Yosemite style), `bar_width` points wide, scrolled by
/// `offset`, `hovered` the tab under the pointer. `measure(text, font_points, bold)` is a text's
/// width in points (bold for a group chip's name and the letter icon). The tabs keep their order; iTerm2 moves no tab out of the overflow menu for the
/// selection, so `_active` does not change the layout. A group chip goes before each run of a
/// group's tabs; a tab hidden in a collapsed group has no cell.
pub fn build(
    tabs: &[TabInput],
    _active: usize,
    bar_width: f32,
    offset: f32,
    hovered: Option<usize>,
    s: &Settings,
    measure: &dyn Fn(&str, f32, bool) -> f32,
) -> Bar {
    let n = tabs.len();
    let x = texts(tabs, s, measure, true);
    let desired: Vec<f32> = (0..n)
        .map(|i| desired_width(x.widths[i], tabs[i].icon, x.counters[i].as_ref().map(|c| counter_width(c.1)), tabs[i].processing))
        .collect();
    let slots = slots(tabs, measure);
    let laid = if slots.len() > n {
        chip_widths(&slots, tabs, &desired, bar_width, &s.prefs, s.pinned_width)
    } else {
        // iTerm2 rounds equal shares to half points (`PSM:3205-3232`).
        let pinned: Vec<bool> = tabs.iter().map(|t| t.pinned).collect();
        widths(&desired, &pinned, s.pinned_width, bar_width, &s.prefs, 2.0)
    };
    let mut bar = Bar { content: laid.content, height: s.height, font: s.font, ..Bar::default() };
    let mut left = -offset;
    for (k, &w) in laid.widths.iter().enumerate() {
        match slots[k] {
            // The name block: the chip less the name's padding (`YS:1586-1591`); a collapsed
            // chip also keeps the chevron's room.
            Slot::Chip(first, _) => {
                let chevron = if tabs[first].group.as_ref().is_some_and(|g| g.collapsed) { crate::scene::GROUP_CHEVRON } else { 0.0 };
                let room = w - 2.0 * crate::scene::GROUP_NAME_PAD - chevron;
                place_chip(&mut bar, tabs, first, Rect { x: left, y: 0.0, w, h: s.height }, room, measure);
            }
            Slot::Tab(i) if tabs[i].hidden => {}
            Slot::Tab(i) => place(&mut bar, i, Rect { x: left, y: 0.0, w, h: s.height }, &tabs[i], hovered == Some(i), s, &x, measure, true),
        }
        left += w;
    }
    // `PSM _setupCells:`: the tabs past the laid-out prefix go to the overflow menu, but a
    // hidden tab never does.
    bar.hidden = slots[laid.widths.len()..].iter().filter_map(|sl| match *sl {
        Slot::Tab(i) if !tabs[i].hidden => Some(i),
        _ => None,
    }).collect();
    if !bar.hidden.is_empty() {
        // `PSM setupButtons`: the overflow button fills the right margin but its first point.
        let m = right_margin(true, s.prefs.add_button);
        bar.overflow = Some(Rect { x: bar_width - m + 1.0, y: 0.0, w: m - 1.0, h: s.height });
    } else if s.prefs.add_button {
        // `PSM finishUpdateWithRegularWidths:...`: right after the last tab, or pinned in the
        // right margin when the tabs scroll past it.
        let mut ax: f32 = laid.widths.iter().sum();
        if laid.content > 0.0 && ax + ADD_BUTTON > bar_width {
            ax = bar_width - right_margin(false, true);
        }
        bar.add = Some(Rect { x: ax, y: 0.0, w: ADD_BUTTON, h: s.height });
    }
    drop_undrawn_chips(&mut bar);
    bar
}

/// `PSM cellIsDrawnInBar:` (`PSM:4916-4921`): a chip laid out just before the overflow cut
/// whose tabs all went to the overflow menu is not drawn, so it is taken out of the bar and
/// nothing hits it. A collapsed chip, which has no tabs to show, stays.
fn drop_undrawn_chips(bar: &mut Bar) {
    let keep: Vec<bool> = bar.chip_texts.iter().map(|t| t.collapsed || bar.cells.iter().any(|(i, _, _)| t.run.contains(i))).collect();
    let mut k = keep.iter();
    bar.chips.retain(|_| *k.next().unwrap_or(&true));
    let mut k = keep.iter();
    bar.chip_texts.retain(|_| *k.next().unwrap_or(&true));
}

/// Lays out a side bar (`PSM update:`, vertical orientation), `bar_width` by `bar_height`
/// points, scrolled down by `offset`: a row `Settings::height` high per tab and per group chip
/// (a chip is a one-row band, `PSM heightOfTabGroupChipCell:`), none for a hidden tab, each
/// laid out as a horizontal tab is at the bar's width, the whole title kept on a pinned tab.
/// With `ScrollableSideTabBar` every row is placed and the rows scroll; without it, the rows
/// that fit keep their height and the rest go to the overflow menu, whose button takes the
/// bottom row (`PSM:2105-2133`, `YS frameForOverflowButtonWithAddTabButton:`). A side bar has
/// no "+" (`iTermTabBarControlView.m:283`).
pub fn build_vertical(
    tabs: &[TabInput],
    bar_width: f32,
    bar_height: f32,
    offset: f32,
    hovered: Option<usize>,
    s: &Settings,
    measure: &dyn Fn(&str, f32, bool) -> f32,
) -> Bar {
    let x = texts(tabs, s, measure, false);
    let slots = slots(tabs, measure);
    let row = s.height;
    let height = |sl: &Slot| match *sl {
        Slot::Tab(i) if tabs[i].hidden => 0.0,
        _ => row,
    };
    let mut bar = Bar { height: bar_height, font: s.font, vertical: true, ..Bar::default() };
    // Each place's top, for a prefix of the places.
    let mut tops: Vec<f32> = Vec::with_capacity(slots.len());
    if s.prefs.scrollable {
        let total: f32 = slots.iter().map(height).sum();
        bar.content = if total > bar_height { total } else { 0.0 };
        let mut y = -offset;
        for sl in &slots {
            tops.push(y);
            y += height(sl);
        }
    } else {
        let mut y = 0.0;
        for sl in &slots {
            let h = height(sl);
            if h == 0.0 {
                tops.push(y);
                continue;
            }
            if y + h <= bar_height {
                tops.push(y);
                y += h;
                continue;
            }
            // Out of room: the last row that fits gives its place to the overflow button,
            // past any hidden tabs before it.
            if !tops.is_empty() && bar_height - y < h {
                while !tops.is_empty() && matches!(slots[tops.len() - 1], Slot::Tab(i) if tabs[i].hidden) {
                    tops.pop();
                }
                tops.pop();
            }
            break;
        }
    }
    let name_room = bar_width - 1.0 - 2.0 * crate::scene::GROUP_NAME_PAD;
    for (k, &top) in tops.iter().enumerate() {
        let rect = Rect { x: 0.0, y: top, w: bar_width, h: row };
        match slots[k] {
            Slot::Chip(first, _) => {
                let chevron = if tabs[first].group.as_ref().is_some_and(|g| g.collapsed) { crate::scene::GROUP_CHEVRON } else { 0.0 };
                place_chip(&mut bar, tabs, first, rect, name_room - chevron, measure);
            }
            Slot::Tab(i) if tabs[i].hidden => {}
            Slot::Tab(i) => place(&mut bar, i, rect, &tabs[i], hovered == Some(i), s, &x, measure, false),
        }
    }
    bar.hidden = slots[tops.len()..].iter().filter_map(|sl| match *sl {
        Slot::Tab(i) if !tabs[i].hidden => Some(i),
        _ => None,
    }).collect();
    if !bar.hidden.is_empty() {
        bar.overflow = Some(Rect { x: 0.0, y: bar_height - row, w: bar_width, h: row });
    }
    drop_undrawn_chips(&mut bar);
    bar
}

/// What a point on the bar is over (`PSM mouseDown:`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    Tab(usize),
    /// A tab's close button while it is drawn.
    Close(usize),
    Add,
    Overflow,
    /// A group chip: an index into `Bar::chips`.
    Chip(usize),
    /// The bar with nothing on it.
    Bar,
}

/// The "+" rect in points, `offset` applied: right after the last tab it moves with the tabs;
/// pinned in the right margin of a bar that scrolls it stays (`PSM:3337-3350`).
fn add_rect(bar: &Bar, offset: f32) -> Option<Rect> {
    bar.add.map(|r| if bar.content > 0.0 { r } else { Rect { x: r.x - offset, ..r } })
}

/// What the point (`x`, `y`) is over, in points from the bar's top left, on a bar built with
/// offset 0 and scrolled by `offset`. None off the bar.
pub fn hit(bar: &Bar, offset: f32, x: f32, y: f32) -> Option<Hit> {
    if x < 0.0 || y < 0.0 || y >= bar.height {
        return None;
    }
    // The buttons sit over the tabs scrolled under them.
    if bar.overflow.is_some_and(|r| r.contains(x, y)) {
        return Some(Hit::Overflow);
    }
    if add_rect(bar, offset).is_some_and(|r| r.contains(x, y)) {
        return Some(Hit::Add);
    }
    // A side bar scrolls down, a horizontal one along.
    let (cx, cy) = if bar.vertical { (x, y + offset) } else { (x + offset, y) };
    if let Some(k) = bar.chips.iter().position(|(_, r)| r.contains(cx, cy)) {
        return Some(Hit::Chip(k));
    }
    let Some((i, layout, cell)) = bar.cells.iter().find(|(_, _, r)| r.contains(cx, cy)) else {
        return Some(Hit::Bar);
    };
    // `closeButtonVisible`: the button is one only while drawn. Its y is from the tab's top.
    let on_close = layout.close_alpha > 0.0 && layout.close.is_some_and(|r| r.contains(cx, cy - cell.y));
    Some(if on_close { Hit::Close(*i) } else { Hit::Tab(*i) })
}

/// The "+" rect in pixels at the window position `origin` of the bar's top left, `offset`
/// applied as `hit` does. None without the button.
pub fn add_rect_px(bar: &Bar, offset: f32, origin: (f32, f32), scale: f32) -> Option<Rect> {
    add_rect(bar, offset).map(|r| {
        let px = to_px(r, scale);
        Rect { x: px.x + origin.0, y: px.y + origin.1, ..px }
    })
}

/// A left press selects its tab at once: a single press without Alt when `SelectsTabsOnMouseDown`
/// is on (`PSM mouseDown:`); otherwise the release decides.
pub fn press_selects(selects_on_down: bool, alt: bool, clicks: u8) -> bool {
    selects_on_down && !alt && clicks == 1
}

/// What a left release on the bar does (`PSM handleMouseUp:`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Release {
    Close(usize),
    Select(usize),
    EditTitle(usize),
    NewTab,
    Nothing,
}

/// A left release at `up` after a press at `down`, `clicks` the press's click count.
/// `double_click_edits` is `DoubleClickTabToEdit`; `close_always` is
/// `TabCloseButtonsAlwaysVisible` (off, iTerm2 sets `disableTabClose` and a press on the close
/// button that slides onto the tab body still counts as a click on the tab). The "+" and the
/// overflow button are buttons of their own and give Nothing here.
pub fn release(down: Option<Hit>, up: Option<Hit>, clicks: u8, double_click_edits: bool, close_always: bool) -> Release {
    let (Some(down), Some(up)) = (down, up) else { return Release::Nothing };
    // The close button that was pressed.
    if let (Hit::Close(a), Hit::Close(b)) = (down, up)
        && a == b {
            return Release::Close(a);
        }
    // Two clicks off any tab, a group chip included (a chip is no cell, `PSM:3912-3914`):
    // iTerm2's New Tab (`tabViewDoubleClickTabBar:`).
    if matches!(up, Hit::Bar | Hit::Chip(_)) {
        return if clicks == 2 { Release::NewTab } else { Release::Nothing };
    }
    let tab_of = |h: Hit| match h {
        Hit::Tab(i) | Hit::Close(i) => Some(i),
        _ => None,
    };
    let (Some(from), Some(to)) = (tab_of(down), tab_of(up)) else { return Release::Nothing };
    // `closeButtonDoesNotInterfere`.
    let interferes = matches!(down, Hit::Close(_)) && close_always;
    if from != to || interferes {
        return Release::Nothing;
    }
    match clicks {
        1 => Release::Select(to),
        // `PT tabView:doubleClickTabViewItem:` selects, then edits when the setting is on.
        2 if double_click_edits => Release::EditTitle(to),
        2 => Release::Select(to),
        _ => Release::Nothing,
    }
}

/// A middle release closes the tab it was pressed on, anywhere on that tab, when
/// `MiddleClickClosesTab` is on (`PSM otherMouseUp:`, `PT:7272-7276`).
pub fn middle_release(down: Option<Hit>, up: Option<Hit>, middle_closes: bool) -> Option<usize> {
    let tab_of = |h: Option<Hit>| match h {
        Some(Hit::Tab(i) | Hit::Close(i)) => Some(i),
        _ => None,
    };
    match (tab_of(down), tab_of(up)) {
        (Some(a), Some(b)) if a == b && middle_closes => Some(a),
        _ => None,
    }
}

/// The width in points the tabs draw in on a bar `bar_width` wide: left of the right margin on
/// a bar that scrolls, so no tab draws under the pinned "+" (the tabs are a scroll view left of
/// the margin, `PSM scrollViewportLength`); the whole bar otherwise.
pub fn tab_area_width(bar: &Bar, bar_width: f32) -> f32 {
    if bar.content > 0.0 {
        (bar_width - right_margin(false, bar.add.is_some())).max(0.0)
    } else {
        bar_width
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prefs() -> Prefs {
        Prefs {
            uneven: false,
            min_width: 75.0,
            optimum: 175.0,
            scrollable_width: 120.0,
            stretch: true,
            scrollable: false,
            add_button: true,
        }
    }

    #[test]
    fn widths_follow_iterm2() {
        assert_eq!(min_width(false, 75.0, 60.0), 75.0);
        assert_eq!(min_width(true, 75.0, 60.0), 60.0);
        assert_eq!(right_margin(false, false), 0.0);
        assert_eq!(right_margin(true, false), 24.0);
        assert_eq!(right_margin(false, true), 24.0);

        let p = prefs();
        let three = [100.0; 3];
        // Stretched: 576 points shared in half points.
        let w = widths(&three, &[], 0.0, 600.0, &p, 2.0);
        assert_eq!(w, Widths { widths: vec![192.0, 192.0, 192.0], overflow: false, content: 0.0 });
        // 553 points is 1106 device pixels: 368 each and 2 left over for the first tabs.
        assert_eq!(widths(&three, &[], 0.0, 577.0, &p, 2.0).widths, vec![184.5, 184.5, 184.0]);

        let fixed = Prefs { stretch: false, ..p };
        assert_eq!(widths(&three, &[], 0.0, 600.0, &fixed, 2.0).widths, vec![175.0, 175.0, 175.0]);
        // 5 x 175 does not fit in 576, 5 x 75 does: stretched instead.
        assert_eq!(
            widths(&[100.0; 5], &[], 0.0, 600.0, &fixed, 2.0).widths,
            vec![115.5, 115.5, 115.0, 115.0, 115.0]
        );

        // 9 tabs of minimum 75 in 476 points: floor(476 / 75) = 6 show, the rest overflow.
        let w = widths(&[100.0; 9], &[], 0.0, 500.0, &p, 2.0);
        assert_eq!(w.widths.len(), 6);
        assert!(w.overflow);
        assert_eq!(w.content, 0.0);
        // Without the add-tab button the regular layout drops tabs, so the overflow variant
        // (24 points of margin) is used, not the full 500.
        let bare = Prefs { add_button: false, ..p };
        let w = widths(&[100.0; 9], &[], 0.0, 500.0, &bare, 2.0);
        assert_eq!(w.widths.len(), 6);
        assert!(w.overflow);
        assert_eq!(w.widths.iter().sum::<f32>(), 476.0);
        // Without the add-tab button and nothing dropped, the whole bar is used.
        assert_eq!(widths(&three, &[], 0.0, 600.0, &bare, 2.0).widths, vec![200.0, 200.0, 200.0]);

        // Uneven: clamped to the minimum and 280 when every tab fits.
        let uneven = Prefs { uneven: true, ..p };
        let desired = [60.0, 150.0, 400.0];
        assert_eq!(widths(&desired, &[], 0.0, 800.0, &uneven, 2.0).widths, vec![75.0, 150.0, 280.0]);
        // The clamped sum 505 passes 476: the equal rule, 952 device pixels over 3.
        assert_eq!(widths(&desired, &[], 0.0, 500.0, &uneven, 2.0).widths, vec![159.0, 158.5, 158.5]);
    }

    #[test]
    fn scrolling_follows_iterm2() {
        let p = Prefs { scrollable: true, ..prefs() };
        let w = widths(&[100.0; 10], &[], 0.0, 600.0, &p, 2.0);
        assert_eq!(w, Widths { widths: vec![120.0; 10], overflow: false, content: 1200.0 });
        // Tabs that fit at 120 use the normal rule and do not scroll.
        let w = widths(&[100.0; 3], &[], 0.0, 600.0, &p, 2.0);
        assert_eq!(w, Widths { widths: vec![192.0, 192.0, 192.0], overflow: false, content: 0.0 });
        // Uneven: max(120, min(280, desired)).
        let uneven = Prefs { uneven: true, ..p };
        let w = widths(&[60.0, 150.0, 400.0, 200.0, 300.0], &[], 0.0, 600.0, &uneven, 2.0);
        assert_eq!(
            w,
            Widths { widths: vec![120.0, 150.0, 280.0, 200.0, 280.0], overflow: false, content: 1030.0 }
        );

        assert_eq!(clamp_scroll(900.0, 1200.0, 576.0), 624.0);
        assert_eq!(clamp_scroll(-5.0, 1200.0, 576.0), 0.0);
        assert_eq!(clamp_scroll(50.0, 0.0, 576.0), 0.0);

        // The tab's right edge meets the viewport's.
        assert_eq!(scroll_into_view(0.0, 700.0, 120.0, 576.0, 1200.0), 244.0);
        // Its left edge meets the viewport's.
        assert_eq!(scroll_into_view(300.0, 100.0, 120.0, 576.0, 1200.0), 100.0);
        // A visible tab keeps the offset.
        assert_eq!(scroll_into_view(100.0, 200.0, 120.0, 576.0, 1200.0), 100.0);

        // A plain vertical wheel is not the bar's.
        assert_eq!(wheel_scroll(0.0, 0.0, -1.0, false, 175.0, 1200.0, 576.0), None);
        // Shift turns it into one optimum width per line.
        assert_eq!(wheel_scroll(0.0, 0.0, -1.0, true, 175.0, 1200.0, 576.0), Some(175.0));
        assert_eq!(wheel_scroll(0.0, -2.0, 0.0, false, 175.0, 1200.0, 576.0), Some(350.0));
        // Clamped at the end.
        assert_eq!(wheel_scroll(600.0, -2.0, 0.0, false, 175.0, 1200.0, 576.0), Some(624.0));
        // Nothing scrolls.
        assert_eq!(wheel_scroll(0.0, -2.0, 0.0, false, 175.0, 0.0, 576.0), None);
    }

    #[test]
    fn precise_deltas_survive_a_zero_optimum_width() {
        // 350 pixels at scale 2 over a 175-point optimum width is one line.
        assert_eq!(pixel_lines(-350.0, 0.0, 175.0, 2.0), (-1.0, 0.0));
        // A zero OptimumTabWidth (an emptied number field) gives finite lines, not NaN or
        // infinity, and the bar does not jump to its end.
        let (dx, dy) = pixel_lines(-30.0, 10.0, 0.0, 1.0);
        assert!(dx.is_finite() && dy.is_finite());
        assert_eq!(wheel_scroll(100.0, dx, dy, false, 0.0, 1200.0, 576.0), Some(100.0));
        assert_eq!(pixel_lines(0.0, 0.0, 0.0, 1.0), (0.0, 0.0));
    }

    #[test]
    fn numbers_follow_iterm2() {
        assert_eq!(modifier_string(4), "Win+");
        assert_eq!(modifier_string(5), "Alt+");
        assert_eq!(modifier_string(6), "Alt+Win+");
        assert_eq!(modifier_string(1), "Ctrl+");
        assert_eq!(modifier_string(9), "");
        assert_eq!(counter_text(1, false, "Win+"), "Ctrl+Shift+1");
        assert_eq!(counter_text(8, false, "Win+"), "Ctrl+Shift+8");
        // Only the last tab past 8 shows "9".
        assert_eq!(counter_text(9, false, "Win+"), "");
        assert_eq!(counter_text(12, true, "Win+"), "Ctrl+Shift+9");
        assert_eq!(counter_text(3, false, ""), "");
        // max(20, text + 2 * 7 - 6)
        assert_eq!(counter_width(12.0), 20.0);
        assert_eq!(counter_width(30.0), 38.0);
    }

    #[test]
    fn build_gives_pinned_tabs_their_width() {
        let base = defaults();
        assert_eq!(base.pinned_width, 64.0, "PinnedTabWidth");
        let s = Settings { prefs: Prefs { add_button: false, stretch: true, ..base.prefs }, ..base };
        let tabs = [TabInput { pinned: true, ..input("vim", 1) }, input("two", 2), input("three", 3)];
        let bar = build(&tabs, 1, 600.0, 0.0, Some(0), &s, &six_per_char);
        let (i, c, r) = &bar.cells[0];
        assert_eq!((*i, *r), (0, Rect { x: 0.0, y: 0.0, w: 64.0, h: 24.0 }));
        assert_eq!((c.close, c.counter), (None, None), "no close button and no number");
        assert_eq!(bar.titles[0], "v", "only the first character");
        assert!(c.truncate, "drawn left-aligned");
        assert_eq!(c.label.x, 6.0 + 16.0 + 4.0, "after the pin's room");
        assert_eq!(bar.cells[1].2, Rect { x: 64.0, y: 0.0, w: 268.0, h: 24.0 });
        assert_eq!(bar.cells[2].2, Rect { x: 332.0, y: 0.0, w: 268.0, h: 24.0 });
        assert!(bar.cells[1].1.counter.is_some());
        // Every tab unpinned: 13c's layout (`build_follows_iterm2`'s stretched thirds).
        let plain: Vec<TabInput> = tabs.iter().map(|t| TabInput { pinned: false, ..t.clone() }).collect();
        let bar = build(&plain, 1, 600.0, 0.0, None, &defaults(), &six_per_char);
        assert_eq!(bar.cells[2].2, Rect { x: 384.0, y: 0.0, w: 192.0, h: 24.0 });
        assert_eq!(bar.add, Some(Rect { x: 576.0, y: 0.0, w: 24.0, h: 24.0 }));
        // Not stretched: the unpinned tabs take the optimum when it fits after the pinned room.
        let fixed = Settings { prefs: Prefs { stretch: false, ..s.prefs }, ..s.clone() };
        let bar = build(&tabs, 1, 600.0, 0.0, None, &fixed, &six_per_char);
        assert_eq!(bar.cells.iter().map(|c| c.2.w).collect::<Vec<_>>(), vec![64.0, 175.0, 175.0]);
        // A scrolling bar: the pinned cell keeps its width in the natural widths.
        let many: Vec<TabInput> = (0..10).map(|i| TabInput { pinned: i == 0, ..input("t", i + 1) }).collect();
        let bar = build(&many, 0, 600.0, 0.0, None, &s, &six_per_char);
        assert_eq!(bar.content, 64.0 + 9.0 * 120.0);
        assert_eq!(bar.cells[0].2.w, 64.0);
        // Too many tabs: the pinned room comes off first, the unpinned share the rest at the
        // minimum or more, the others overflow: 476 - 128 = 348 fits 4 of minimum 75.
        let mut pins = [false; 9];
        pins[..2].copy_from_slice(&[true, true]);
        let w = widths(&[100.0; 9], &pins, 64.0, 500.0, &Prefs { scrollable: false, ..s.prefs }, 2.0);
        assert_eq!(w.widths, vec![64.0, 64.0, 87.0, 87.0, 87.0, 87.0]);
        assert!(w.overflow);
        // Scrolled, the pinned cell moves with the others (`PSM:3396-3403` shifts the whole walk).
        let bar = build(&many, 0, 600.0, 100.0, None, &s, &six_per_char);
        assert_eq!(bar.cells[0].2.x, -100.0);
    }

    fn tab() -> CellInput {
        CellInput {
            x: 0.0,
            width: 175.0,
            height: 24.0,
            close_buttons: true,
            close_always: false,
            hovered: false,
            counter: Some(38.0),
            icon: false,
            indicator: false,
            title_width: 40.0,
            min_label: 35.0,
            pinned: false,
            graphic: false,
            vertical: false,
        }
    }

    #[test]
    fn cell_follows_iterm2() {
        let c = cell(&tab());
        assert_eq!(c.close, Some(Rect { x: 6.0, y: 4.0, w: 16.0, h: 16.0 }));
        assert_eq!(c.close_alpha, 0.0);
        assert_eq!(c.reserved, 20.0);
        // The room is reserved, not moved: 175 - 6 - 4 - (38 + 4).
        assert_eq!(c.label, Rect { x: 6.0, y: 0.0, w: 123.0, h: 24.0 });
        assert!(!c.truncate);
        assert_eq!(c.counter, Some(Rect { x: 131.0, y: 5.0, w: 38.0, h: 14.0 }));
        assert_eq!(c.icon, None);
        assert_eq!(c.indicator, None);

        assert_eq!(cell(&CellInput { hovered: true, ..tab() }).close_alpha, 1.0);

        let always = cell(&CellInput { close_always: true, ..tab() });
        assert_eq!(always.label.x, 26.0);
        assert_eq!(always.reserved, 0.0);
        assert_eq!(always.close_alpha, 1.0);

        // A 110 title in the 123 label is centered 6.5 from its left, under the reserved 20:
        // left-aligned after the reserved room, 20 narrower.
        let long = cell(&CellInput { title_width: 110.0, ..tab() });
        assert!(long.truncate);
        assert_eq!(long.label, Rect { x: 26.0, y: 0.0, w: 103.0, h: 24.0 });

        // A side bar row (`YS:889-891`, `:1146-1152`): the hover-only close button moves the
        // label past its room instead of reserving it, the label is left-aligned, and a long
        // title never shifts or narrows further: 175 - 26 - 4 - 42 = 103.
        for title_width in [40.0, 110.0, 300.0] {
            let side = cell(&CellInput { vertical: true, title_width, ..tab() });
            assert_eq!((side.reserved, side.truncate, side.close_alpha), (0.0, true, 0.0), "{title_width}");
            assert_eq!(side.label, Rect { x: 26.0, y: 0.0, w: 103.0, h: 24.0 }, "{title_width}");
        }
        // With the icon picture the label still starts at 26, not 46.
        let side = cell(&CellInput { vertical: true, graphic: true, title_width: 300.0, ..tab() });
        assert_eq!(side.label.x, 26.0);
        assert_eq!(cell(&CellInput { vertical: true, hovered: true, ..tab() }).close_alpha, 1.0);

        // The icon sits 4 left of the counter, one point above center; the label loses it.
        let icon = cell(&CellInput { icon: true, ..tab() });
        assert_eq!(icon.icon, Some(Rect { x: 111.0, y: 3.0, w: 16.0, h: 16.0 }));
        assert_eq!(icon.label.w, 103.0);
        // The icon needs more than `min_label` of label room. iTerm2 asks with a zero-size icon
        // rect, so only its 4 points of padding count: 49 - 6 - 4 - 4 = 35 is not enough.
        let bare = CellInput { close_buttons: false, counter: None, icon: true, title_width: 0.0, ..tab() };
        assert_eq!(cell(&CellInput { width: 49.0, ..bare }).icon, None);
        assert!(cell(&CellInput { width: 50.0, ..bare }).icon.is_some());
        // A narrow tab whose label would truncate: 100 - 10 - 4 - 42 = 44, less the reserved 20.
        let narrow = cell(&CellInput { width: 100.0, icon: true, ..tab() });
        assert_eq!(narrow.icon, None);
        assert_eq!(narrow.label.w, 28.0);
        // Without a counter the icon is left of the right margin, else of the indicator.
        assert_eq!(cell(&CellInput { counter: None, ..icon_only() }).icon.unwrap().x, 175.0 - 6.0 - 4.0 - 16.0);
        let both = cell(&CellInput { counter: None, indicator: true, ..icon_only() });
        assert_eq!(both.indicator.unwrap().x, 149.0);
        assert_eq!(both.icon.unwrap().x, 149.0 - 6.0 - 4.0 - 16.0);

        // The indicator without a counter: right edge at 175 - 6 - 4; the label loses 16 + 4.
        let spin = cell(&CellInput { indicator: true, counter: None, ..tab() });
        let r = spin.indicator.unwrap();
        assert_eq!(r.x + r.w, 175.0 - 6.0 - 4.0);
        assert_eq!(r.y, 4.0);
        assert_eq!(spin.label.w, 175.0 - 6.0 - 4.0 - 20.0);
        // With a counter it is 4 left of it.
        assert_eq!(cell(&CellInput { indicator: true, ..tab() }).indicator.unwrap().x, 131.0 - 4.0 - 16.0);

        let none = cell(&CellInput { close_buttons: false, ..tab() });
        assert_eq!(none.close, None);
        assert_eq!(none.reserved, 0.0);
        assert_eq!(none.label.x, 6.0);

        // The tab sits at its own x.
        let moved = cell(&CellInput { x: 100.0, ..tab() });
        assert_eq!(moved.close.unwrap().x, 106.0);
        assert_eq!(moved.counter.unwrap().x, 231.0);
        assert_eq!(moved.label.x, 106.0);

        assert_eq!(desired_width(40.0, false, Some(38.0), false), 114.0);
        // Without a counter the close button's room is on the right too.
        assert_eq!(desired_width(40.0, false, None, false), 92.0);
        assert_eq!(desired_width(40.5, true, None, true), 133.0);
    }

    fn icon_only() -> CellInput {
        CellInput { icon: true, ..tab() }
    }

    fn one_per_char(s: &str) -> f32 {
        s.chars().count() as f32
    }

    #[test]
    fn truncation_follows_iterm2() {
        // One prefix, three suffixes.
        assert_eq!(truncation(&["server-a: ~/src", "server-a: ~/doc", "server-a: ~/bin"], true), Truncation::Head);
        // Three unique prefixes, three unique suffixes: a tie is Tail.
        assert_eq!(truncation(&["alpha one", "bravo one", "charlie one"], true), Truncation::Tail);
        assert_eq!(truncation(&["server-a: ~/src", "server-a: ~/doc", "server-a: ~/bin"], false), Truncation::Tail);
        assert_eq!(truncation(&["server-a: ~/src"], true), Truncation::Tail);
        assert_eq!(truncation(&[], true), Truncation::Tail);
        // Titles under 5 characters are skipped: one prefix against one suffix.
        assert_eq!(truncation(&["abc", "abd", "server-a: ~/src", "xy"], true), Truncation::Tail);
        // Spinners and spaces before the text are not part of the prefix.
        assert_eq!(smart_prefix("\u{2810} build", 5), "build");
        assert_eq!(smart_prefix("\u{2733} build it", 5), "build");
        assert_eq!(smart_prefix("***", 5), "***");
        assert_eq!(smart_prefix("*****!", 5), "*****");
        assert_eq!(
            truncation(&["\u{2810} build: one", "\u{2802} build: two", "\u{2733} build: six"], true),
            Truncation::Head
        );
        // " (n)" at the start (index 5) is not a trailing parenthetical past the first 5.
        assert_eq!(truncation(&["x-app (1)", "x-app (2)", "y-app (3)"], true), Truncation::Head);
        // One plain suffix " (12)", three before the parenthetical: the larger count wins.
        assert_eq!(truncation(&["srv: aaaa (12)", "srv: bbbb (12)", "srv: cccc (12)"], true), Truncation::Head);

        let m = &one_per_char;
        assert_eq!(truncate("abcdefghij", 6.0, Truncation::Tail, m), "abcde\u{2026}");
        assert_eq!(truncate("abcdefghij", 6.0, Truncation::Head, m), "\u{2026}fghij");
        assert_eq!(truncate("abcdefghij", 10.0, Truncation::Tail, m), "abcdefghij");
        assert_eq!(truncate("abcdefghij", 0.0, Truncation::Tail, m), "");
        assert_eq!(truncate("abcdefghij", 1.0, Truncation::Head, m), "\u{2026}");
        assert_eq!(truncate("\u{00e9}t\u{00e9} long", 4.0, Truncation::Tail, m), "\u{00e9}t\u{00e9}\u{2026}");
    }

    fn run(text: &str, bold: bool, italic: bool, underline: bool) -> Run {
        Run { text: text.into(), bold, italic, underline }
    }

    #[test]
    fn html_follows_iterm2() {
        assert_eq!(
            html_runs("a<b>b</b>c"),
            vec![run("a", false, false, false), run("b", true, false, false), run("c", false, false, false)]
        );
        // iTerm2's `isHTML` check is case-sensitive: upper-case tags alone are not HTML.
        assert_eq!(html_runs("<B>x</B>"), vec![run("<B>x</B>", false, false, false)]);
        // In an HTML title an upper-case tag is dropped but toggles nothing (`indexOfObject:`
        // compares exactly).
        assert_eq!(html_runs("<b>a</b><I>c</I>"), vec![run("a", true, false, false), run("c", false, false, false)]);
        assert_eq!(html_runs("<i><u>x</u></i>"), vec![run("x", false, true, true)]);
        assert_eq!(html_runs("1 &lt; 2 &amp; 3"), vec![run("1 &lt; 2 &amp; 3", false, false, false)]);
        assert_eq!(html_runs("<b>1 &lt; 2</b>"), vec![run("1 < 2", true, false, false)]);
        assert_eq!(html_runs("<font>x</font>"), vec![run("<font>x</font>", false, false, false)]);
        assert_eq!(html_runs("<b>x</b> <font>y"), vec![run("x", true, false, false), run(" <font>y", false, false, false)]);
        assert_eq!(html_runs(""), vec![]);
        assert_eq!(plain_title("<b>x</b>", true), "x");
        assert_eq!(plain_title("<b>x</b>", false), "<b>x</b>");
        assert_eq!(limit_title(&"a".repeat(300)), format!("{}\u{2026}", "a".repeat(255)));
        assert_eq!(limit_title(&"a".repeat(255)), "a".repeat(255));
    }

    #[test]
    fn texts_follow_iterm2() {
        assert_eq!(subtitle_text("sub", Some("running"), true), "running \u{2013} sub");
        assert_eq!(subtitle_text("", Some("running"), true), "running");
        assert_eq!(subtitle_text("sub", Some("running"), false), "sub");
        assert_eq!(subtitle_text("sub", Some(""), true), "sub");
        assert_eq!(subtitle_text("sub", None, true), "sub");
        assert_eq!(tooltip("zsh", "Default", Some("zsh -l")), "Name: zsh\nProfile: Default\nCommand: zsh -l");
        assert_eq!(tooltip("zsh", "Default", None), "Name: zsh\nProfile: Default\nCommand: None");

        assert_eq!(tab_colors("#fb6b62 #f6ac47 #f0dc4f #b5d749 #5fa3f8 #c18ed9 #787878"), DEFAULT_COLORS.to_vec());
        assert_eq!(DEFAULT_COLORS[0], rgb(0xfb, 0x6b, 0x62));
        assert_eq!(DEFAULT_COLORS[6], rgb(0x78, 0x78, 0x78));
        assert_eq!(tab_colors("#abc"), vec![rgb(0xaa, 0xbb, 0xcc)]);
        assert_eq!(tab_colors("#ffff80000000"), vec![rgb(0xff, 0x80, 0x00)]);
        // `p3#` (the form iTerm2's parser reads) converts through `p3_to_srgb`.
        let (r, g, b) = crate::import::p3_to_srgb(1.0, 0.0, 0.0);
        let to8 = |c: f64| (c * 255.0).round() as u8;
        assert_eq!(tab_colors("p3#ffff00000000"), vec![rgb(to8(r), to8(g), to8(b))]);
        assert_eq!(tab_colors("p3#ff0000"), vec![rgb(to8(r), to8(g), to8(b))]);
        // The help's "color(p3 1 0.5 0.25)" splits on spaces into words iTerm2's parser rejects.
        assert_eq!(tab_colors("color(p3 1 0.5 0.25)"), DEFAULT_COLORS.to_vec());
        // A broken spec falls back; bad words are skipped.
        assert_eq!(tab_colors(""), DEFAULT_COLORS.to_vec());
        assert_eq!(tab_colors("nope #12"), DEFAULT_COLORS.to_vec());
        assert_eq!(tab_colors("nope  #123456 #zzzzzz #00ff00"), vec![rgb(0x12, 0x34, 0x56), rgb(0, 0xff, 0)]);
    }

    #[test]
    fn theme_font_follows_iterm2() {
        let empty = crate::globals::Globals::empty(std::path::Path::new("unused.toml"));
        assert_eq!(Settings::from_globals(&empty).with_theme_font(&empty, 12.0).font, 12.0);
        assert_eq!(Settings::from_globals(&empty).with_theme_font(&empty, 11.0).font, 11.0);
        let mut g = crate::globals::Globals::empty(std::path::Path::new("unused.toml"));
        g.set("UseCustomTabBarFontSize", toml::Value::Boolean(true));
        g.set("CustomTabBarFontSize", toml::Value::Integer(14));
        assert_eq!(Settings::from_globals(&g).with_theme_font(&g, 12.0).font, 14.0);
    }

    fn six_per_char(s: &str, _font: f32, _bold: bool) -> f32 {
        s.chars().count() as f32 * 6.0
    }

    fn defaults() -> Settings {
        Settings::from_globals(&crate::globals::Globals::empty(std::path::Path::new("unused.toml")))
    }

    fn input(title: &str, number: usize) -> TabInput {
        TabInput { title: title.into(), subtitle: String::new(), icon: false, processing: false, number, pinned: false, group: None, hidden: false, graphic: false, letter: String::new() }
    }

    #[test]
    fn build_places_group_chips() {
        let forty = |_: &str, _: f32, _: bool| 40.0;
        let chip = |first: bool, collapsed: bool| Some(ChipInput { id: "g".into(), name: "g".into(), collapsed, count: 2, first });
        let tabs = [TabInput { group: chip(true, false), ..input("a", 1) }, TabInput { group: chip(false, false), ..input("b", 2) }, input("c", 3)];
        let bar = build(&tabs, 0, 600.0, 0.0, None, &defaults(), &forty);
        assert_eq!(bar.chips.len(), 1);
        let (id, r) = &bar.chips[0];
        assert_eq!(id, "g");
        assert_eq!(r.w, 66.0, "40 + 10 x 2 + 6");
        assert_eq!(r.x + r.w, bar.cells[0].2.x, "ends where tab 0 starts");
        assert_eq!(hit(&bar, 0.0, r.x + 5.0, 10.0), Some(Hit::Chip(0)));
        assert_eq!(bar.chip_texts[0].run, 0..2);
        // The tabs share what the chip leaves: (576 - 66) / 3.
        assert_eq!(bar.cells.iter().map(|c| c.2.w).collect::<Vec<_>>(), vec![170.0; 3]);

        // Collapsed: the members have no cells, the chip is 40 + 10 x 2 + 14, tab 2 after it.
        let hidden = |t: TabInput| TabInput { hidden: true, ..t };
        let tabs = [hidden(TabInput { group: chip(true, true), ..input("a", 1) }), hidden(TabInput { group: chip(false, true), ..input("b", 2) }), input("c", 3)];
        let bar = build(&tabs, 2, 600.0, 0.0, None, &defaults(), &forty);
        assert_eq!(bar.chips.len(), 1);
        assert_eq!(bar.chips[0].1.w, 74.0);
        assert_eq!(bar.cells.iter().map(|c| c.0).collect::<Vec<_>>(), vec![2]);
        assert_eq!(bar.cells[0].2.x, 74.0);
        assert!(bar.hidden.is_empty(), "a collapsed tab is not in the overflow menu");
        assert_eq!(bar.chip_texts[0].label, "g  2");

        // No groups: 13c's layout.
        let plain = [input("a", 1), input("b", 2), input("c", 3)];
        let bar = build(&plain, 0, 600.0, 0.0, None, &defaults(), &forty);
        assert!(bar.chips.is_empty());
        assert_eq!(bar.cells.iter().map(|c| c.2).collect::<Vec<_>>(), [0.0, 192.0, 384.0].map(|x| Rect { x, y: 0.0, w: 192.0, h: 24.0 }));
    }

    #[test]
    fn undrawn_chips_are_not_in_the_bar() {
        let forty = |_: &str, _: f32, _: bool| 40.0;
        let mut s = defaults();
        s.prefs.scrollable = false;
        let chip = |first: bool| Some(ChipInput { id: "g".into(), name: "g".into(), collapsed: false, count: 2, first });
        let tabs = [input("a", 1), TabInput { group: chip(true), ..input("b", 2) }, TabInput { group: chip(false), ..input("c", 3) }];
        // Rows a, chip, b fit in 80 points; c does not, so b gives its row to the overflow
        // button and the chip's whole run is in the menu: the chip is not drawn or hit.
        let bar = build_vertical(&tabs, 150.0, 80.0, 0.0, None, &s, &forty);
        assert_eq!(bar.hidden, vec![1, 2]);
        assert!(bar.chips.is_empty() && bar.chip_texts.is_empty());
        assert_eq!(hit(&bar, 0.0, 10.0, 30.0), Some(Hit::Bar));
        // With room for b the chip stays.
        let bar = build_vertical(&tabs, 150.0, 100.0, 0.0, None, &s, &forty);
        assert_eq!(bar.chips.len(), 1);
        assert_eq!(hit(&bar, 0.0, 10.0, 30.0), Some(Hit::Chip(0)));
    }

    #[test]
    fn side_bar_overflows_like_iterm2() {
        let base = defaults();
        let fixed = Settings { prefs: Prefs { scrollable: false, ..base.prefs }, ..base.clone() };
        let tabs: Vec<TabInput> = (0..10).map(|i| input("t", i + 1)).collect();
        // 100 points: rows at 0, 24, 48, 72 fit, the fifth does not; the last that fit gives
        // its row to the overflow button pinned at the bottom. Rows keep their height.
        let bar = build_vertical(&tabs, 150.0, 100.0, 0.0, None, &fixed, &six_per_char);
        assert!(bar.vertical);
        assert_eq!(bar.cells.iter().map(|c| c.2).collect::<Vec<_>>(), [0.0, 24.0, 48.0].map(|y| Rect { x: 0.0, y, w: 150.0, h: 24.0 }));
        assert_eq!(bar.hidden, (3..10).collect::<Vec<_>>());
        assert_eq!(bar.overflow, Some(Rect { x: 0.0, y: 76.0, w: 150.0, h: 24.0 }));
        assert_eq!(bar.add, None, "a side bar has no +");
        assert_eq!(hit(&bar, 0.0, 10.0, 80.0), Some(Hit::Overflow));
        // Scrolling: every row, the content their height, scrolled down by the offset.
        let bar = build_vertical(&tabs, 150.0, 100.0, 30.0, None, &base, &six_per_char);
        assert_eq!(bar.cells.len(), 10);
        assert_eq!((bar.content, bar.cells[1].2.y, bar.overflow), (240.0, -6.0, None));
        // A bar laid out unscrolled is hit at the scroll: row 2 starts at 48 - 30 = 18.
        let flat = build_vertical(&tabs, 150.0, 100.0, 0.0, None, &base, &six_per_char);
        assert_eq!(hit(&flat, 30.0, 10.0, 20.0), Some(Hit::Tab(2)));
        // A side row is laid out as a tab: its number, a close button on hover, its title.
        let bar = build_vertical(&tabs[..2], 150.0, 100.0, 0.0, Some(1), &base, &six_per_char);
        let (_, c, r) = &bar.cells[1];
        assert!(c.counter.is_some() && c.close_alpha == 1.0);
        assert_eq!(hit(&bar, 0.0, 6.0 + 8.0, r.y + 12.0), Some(Hit::Close(1)));
        // A group's chip is a row before its run; a collapsed group's tabs have none.
        let g = |first: bool, collapsed: bool| Some(ChipInput { id: "g".into(), name: "g".into(), collapsed, count: 2, first });
        let grouped = [TabInput { group: g(true, false), ..input("a", 1) }, TabInput { group: g(false, false), ..input("b", 2) }, input("c", 3)];
        let bar = build_vertical(&grouped, 150.0, 200.0, 0.0, None, &base, &six_per_char);
        assert_eq!(bar.chips, vec![("g".to_string(), Rect { x: 0.0, y: 0.0, w: 150.0, h: 24.0 })]);
        assert_eq!(bar.cells.iter().map(|c| c.2.y).collect::<Vec<_>>(), vec![24.0, 48.0, 72.0]);
        let hidden = |t: TabInput| TabInput { hidden: true, ..t };
        let collapsed = [hidden(TabInput { group: g(true, true), ..input("a", 1) }), hidden(TabInput { group: g(false, true), ..input("b", 2) }), input("c", 3)];
        let bar = build_vertical(&collapsed, 150.0, 200.0, 0.0, None, &base, &six_per_char);
        assert_eq!(bar.cells.iter().map(|c| (c.0, c.2.y)).collect::<Vec<_>>(), vec![(2, 24.0)]);
        assert_eq!(hit(&bar, 0.0, 10.0, 10.0), Some(Hit::Chip(0)));
    }

    #[test]
    fn bar_metrics_follow_iterm2() {
        assert_eq!(bar_height(24.0), 24.0);
        assert_eq!(font_size(false, 14.0), 11.0);
        assert_eq!(font_size(true, 14.0), 14.0);
        assert_eq!(subtitle_size(11.0), 9.0);
        assert!(!shows_subtitle(24.0, "x"));
        assert!(shows_subtitle(28.0, "x"));
        assert!(!shows_subtitle(28.0, ""));

        let s = defaults();
        assert_eq!(s.height, 24.0);
        assert_eq!(s.font, 11.0);
        assert_eq!(s.prefs.optimum, 175.0);
        assert_eq!(s.prefs.min_width, 75.0);
        assert!(s.prefs.scrollable);
        assert!(s.prefs.stretch);
        assert!(s.prefs.add_button);
        assert!(s.close_buttons);
        assert!(!s.close_always);
        assert!(s.smart);
        assert_eq!(s.modifier, "Win+");
        assert_eq!(s.min_label, 35.0);
        let tabs = [input("one", 1), input("two", 2), input("three", 3)];
        let bar = build(&tabs, 0, 600.0, 0.0, None, &s, &six_per_char);
        assert!(bar.cells.iter().all(|(_, c, _)| c.counter.is_some()));
        assert_eq!(bar.texts[0].counter, "Ctrl+Shift+1");

        let mut g = crate::globals::Globals::empty(std::path::Path::new("unused.toml"));
        g.set("HideTabNumber", toml::Value::Boolean(true));
        let s = Settings::from_globals(&g);
        assert_eq!(s.prefs.min_width, 60.0);
        let bar = build(&tabs, 0, 600.0, 0.0, None, &s, &six_per_char);
        assert!(bar.cells.iter().all(|(_, c, _)| c.counter.is_none()));
        assert_eq!(bar.texts[0].counter, "");
    }

    #[test]
    fn build_follows_iterm2() {
        let s = defaults();
        // 40 characters, 240 points: one prefix and three suffixes, so the head gives way.
        let long = |end: &str| format!("server-a: {}{end}", "x".repeat(25));
        let tabs = [input(&long("one11"), 1), input(&long("two22"), 2), input(&long("six33"), 3)];
        let bar = build(&tabs, 1, 600.0, 0.0, None, &s, &six_per_char);
        assert_eq!(bar.cells.len(), 3);
        assert_eq!(bar.cells[1].0, 1, "tab order kept, the active tab at its own index");
        // 576 stretched over 3 tabs.
        assert_eq!(bar.cells[2].2, Rect { x: 384.0, y: 0.0, w: 192.0, h: 24.0 });
        assert_eq!(bar.add, Some(Rect { x: 576.0, y: 0.0, w: 24.0, h: 24.0 }));
        assert_eq!(bar.overflow, None);
        assert!(bar.hidden.is_empty());
        assert_eq!(bar.height, 24.0);
        // The label: 192 - 6 - 4 - (80 + 4) = 98 (the counter reads Ctrl+Shift+1, 12 characters),
        // truncated after the reserved 20: 78 points, 12 characters and the ellipsis.
        let c = &bar.cells[0].1;
        assert!(c.truncate);
        assert_eq!(c.label.w, 78.0);
        let t0 = long("one11");
        let tail: String = t0.chars().skip(40 - 12).collect();
        assert_eq!(bar.titles[0], format!("\u{2026}{tail}"));
        assert_eq!(bar.texts[0].title_width(), 78.0);
        // The bar-wide rule: tail truncation with smart truncation off.
        let plain = build(&tabs, 1, 600.0, 0.0, None, &Settings { smart: false, ..s.clone() }, &six_per_char);
        assert!(plain.titles[0].ends_with('\u{2026}'));
        // No subtitle on a 24-point bar.
        let sub = [TabInput { subtitle: "sub".into(), ..input("a", 1) }, input("b", 2)];
        assert_eq!(build(&sub, 0, 600.0, 0.0, None, &s, &six_per_char).subtitles[0], "");
        let tall = build(&sub, 0, 600.0, 0.0, None, &Settings { height: 28.0, ..s.clone() }, &six_per_char);
        assert_eq!(tall.subtitles[0], "sub");
        assert_eq!(tall.texts[0].subtitle_width, 18.0);
        assert!(!tall.texts[0].subtitle_truncate);
        // The icon and the spinner reach the inside.
        let busy = [TabInput { icon: true, processing: true, ..input("a", 1) }, input("b", 2)];
        let b = build(&busy, 0, 600.0, 0.0, Some(0), &s, &six_per_char);
        assert!(b.cells[0].1.icon.is_some() && b.cells[0].1.indicator.is_some());
        assert_eq!(b.cells[0].1.close_alpha, 1.0, "hovered");
        assert_eq!(b.cells[1].1.close_alpha, 0.0);

        // HTML runs survive truncation with their styles.
        let html = Settings { html: true, ..s.clone() };
        let b = build(&[input("<b>bold</b> x", 1), input("b", 2)], 0, 600.0, 0.0, None, &html, &six_per_char);
        assert_eq!(b.titles[0], "bold x");
        assert_eq!(
            b.texts[0].runs,
            vec![(run("bold", true, false, false), 24.0), (run(" x", false, false, false), 12.0)]
        );
        assert_eq!(
            cut_runs(&[run("ab", true, false, false), run("cd", false, false, true)], "abc\u{2026}", Truncation::Tail),
            vec![run("ab", true, false, false), run("c\u{2026}", false, false, true)]
        );
        assert_eq!(cut_runs(&[run("abcd", false, false, false)], "", Truncation::Tail), Vec::new());
        assert_eq!(cut_runs(&[run("abcd", false, false, false)], "", Truncation::Head), Vec::new());

        // `RemoveAddTabButton`: no button and no right margin.
        let bare = Settings { prefs: Prefs { add_button: false, ..s.prefs }, ..s.clone() };
        let b = build(&tabs, 0, 600.0, 0.0, None, &bare, &six_per_char);
        assert_eq!(b.add, None);
        assert_eq!(b.cells[2].2.x + b.cells[2].2.w, 600.0);

        // 20 tabs on a bar that does not scroll: floor(576 / 75) = 7 show, the rest overflow.
        let many: Vec<TabInput> = (1..=20).map(|i| input(&format!("tab {i}"), i)).collect();
        let fixed = Settings { prefs: Prefs { scrollable: false, ..s.prefs }, ..s.clone() };
        let b = build(&many, 0, 600.0, 0.0, None, &fixed, &six_per_char);
        assert_eq!(b.cells.len(), 7);
        assert_eq!(b.hidden, (7..20).collect::<Vec<_>>());
        assert_eq!(b.overflow, Some(Rect { x: 577.0, y: 0.0, w: 23.0, h: 24.0 }));
        assert_eq!(b.add, None);
        // The default scrolls: 20 x 120, the add button pinned in the right margin, the offset
        // moving the tabs.
        let b = build(&many, 0, 600.0, 130.0, None, &s, &six_per_char);
        assert_eq!(b.cells.len(), 20);
        assert_eq!(b.content, 2400.0);
        assert_eq!(b.add, Some(Rect { x: 576.0, y: 0.0, w: 24.0, h: 24.0 }));
        assert_eq!(b.cells[1].2.x, -10.0);
        assert_eq!(b.cells[1].1.close.unwrap().x, b.cells[1].2.x + 6.0);

        assert_eq!(to_px(Rect { x: 1.0, y: 2.0, w: 3.0, h: 4.0 }, 1.5), Rect { x: 1.5, y: 3.0, w: 4.5, h: 6.0 });
    }

    #[test]
    fn hits_follow_iterm2() {
        // Not stretched: three tabs of 175, the "+" at 525, room past it.
        let s = Settings { prefs: Prefs { stretch: false, scrollable: false, ..defaults().prefs }, ..defaults() };
        let tabs = [input("a", 1), input("b", 2), input("c", 3)];
        let hovered = build(&tabs, 0, 600.0, 0.0, Some(1), &s, &six_per_char);
        let close = hovered.cells[1].1.close.unwrap();
        let (cx, cy) = (close.x + 2.0, close.y + 2.0);
        assert_eq!(hit(&hovered, 0.0, cx, cy), Some(Hit::Close(1)));
        assert_eq!(hit(&hovered, 0.0, 175.0 + 100.0, 12.0), Some(Hit::Tab(1)));
        assert_eq!(hit(&hovered, 0.0, 530.0, 12.0), Some(Hit::Add));
        assert_eq!(hit(&hovered, 0.0, 560.0, 12.0), Some(Hit::Bar));
        assert_eq!(hit(&hovered, 0.0, 100.0, 24.0), None, "below the bar");
        assert_eq!(hit(&hovered, 0.0, 100.0, -1.0), None);
        // The close button is only a button while drawn (`closeButtonVisible`).
        let idle = build(&tabs, 0, 600.0, 0.0, None, &s, &six_per_char);
        assert_eq!(hit(&idle, 0.0, cx, cy), Some(Hit::Tab(1)));
        let always = build(&tabs, 0, 600.0, 0.0, None, &Settings { close_always: true, ..s.clone() }, &six_per_char);
        assert_eq!(hit(&always, 0.0, cx, cy), Some(Hit::Close(1)));
        let none = build(&tabs, 0, 600.0, 0.0, Some(1), &Settings { close_buttons: false, ..s.clone() }, &six_per_char);
        assert_eq!(hit(&none, 0.0, cx, cy), Some(Hit::Tab(1)));

        // A scrolled bar: tabs of 120 from content x 0; at offset 200 the point x 10 is content
        // x 210, in tab 1.
        let scrolls = Settings { prefs: Prefs { scrollable: true, ..s.prefs }, ..s.clone() };
        let ten: Vec<TabInput> = (1..=10).map(|i| input(&format!("t{i}"), i)).collect();
        let bar = build(&ten, 0, 600.0, 0.0, None, &scrolls, &six_per_char);
        assert_eq!(bar.content, 1200.0);
        assert_eq!(hit(&bar, 200.0, 10.0, 12.0), Some(Hit::Tab(1)));
        assert_eq!(hit(&bar, 0.0, 10.0, 12.0), Some(Hit::Tab(0)));
        // The "+" stays pinned in the right margin, over the tabs scrolled under it.
        assert_eq!(hit(&bar, 200.0, 580.0, 12.0), Some(Hit::Add));

        // A bar that does not scroll and has too many tabs: the overflow button.
        let many: Vec<TabInput> = (1..=20).map(|i| input(&format!("t{i}"), i)).collect();
        let over = build(&many, 0, 600.0, 0.0, None, &s, &six_per_char);
        let o = over.overflow.unwrap();
        assert_eq!(hit(&over, 0.0, o.x + 5.0, 12.0), Some(Hit::Overflow));
    }

    #[test]
    fn add_rect_follows_the_bar() {
        let s = Settings { prefs: Prefs { stretch: false, scrollable: false, ..defaults().prefs }, ..defaults() };
        let tabs = [input("a", 1), input("b", 2), input("c", 3)];
        let bar = build(&tabs, 0, 600.0, 0.0, None, &s, &six_per_char);
        let add = bar.add.unwrap();
        let px = add_rect_px(&bar, 0.0, (0.0, 30.0), 2.0).unwrap();
        assert_eq!(px, Rect { x: add.x * 2.0, y: add.y * 2.0 + 30.0, w: add.w * 2.0, h: add.h * 2.0 });
        assert_eq!(add_rect_px(&bar, 100.0, (0.0, 30.0), 2.0).unwrap().x, px.x - 200.0);
        assert_eq!(add_rect_px(&bar, 0.0, (10.0, 0.0), 2.0).unwrap().x, px.x + 10.0);
        // Pinned in the right margin of a bar that scrolls (`PSM:3337-3350`): the offset leaves it.
        let scrolls = Settings { prefs: Prefs { scrollable: true, ..s.prefs }, ..s.clone() };
        let ten: Vec<TabInput> = (1..=10).map(|i| input(&format!("t{i}"), i)).collect();
        let scrolled = build(&ten, 0, 600.0, 0.0, None, &scrolls, &six_per_char);
        assert_eq!(add_rect_px(&scrolled, 300.0, (0.0, 0.0), 1.0), Some(Rect { x: 576.0, y: 0.0, w: 24.0, h: 24.0 }));
        // `RemoveAddTabButton`.
        let bare = Settings { prefs: Prefs { add_button: false, ..s.prefs }, ..s.clone() };
        assert_eq!(add_rect_px(&build(&tabs, 0, 600.0, 0.0, None, &bare, &six_per_char), 0.0, (0.0, 30.0), 2.0), None);
    }

    #[test]
    fn clicks_follow_iterm2() {
        use Hit::{Bar, Chip, Close, Tab};
        assert!(press_selects(true, false, 1));
        assert!(!press_selects(true, true, 1), "Alt waits for the release");
        assert!(!press_selects(false, false, 1));
        assert!(!press_selects(true, false, 2));
        // Close buttons always visible: a press on the close button owns the release.
        assert_eq!(release(Some(Close(1)), Some(Close(1)), 1, true, true), Release::Close(1));
        assert_eq!(release(Some(Tab(1)), Some(Close(1)), 1, true, true), Release::Select(1), "the close button closes only when it was pressed");
        assert_eq!(release(Some(Close(1)), Some(Tab(1)), 1, true, true), Release::Nothing, "a press on the close button that slides off does not select");
        // Source over brief: with hover-only close buttons (the default) iTerm2's
        // `disableTabClose` is on and the slide-off counts as a click on the tab.
        assert_eq!(release(Some(Close(1)), Some(Tab(1)), 1, true, false), Release::Select(1));
        assert_eq!(release(Some(Close(1)), Some(Close(1)), 1, true, false), Release::Close(1));
        assert_eq!(release(Some(Close(1)), Some(Close(2)), 1, true, true), Release::Nothing);
        assert_eq!(release(Some(Tab(1)), Some(Tab(2)), 1, true, true), Release::Nothing);
        assert_eq!(release(Some(Tab(1)), Some(Tab(1)), 2, true, true), Release::EditTitle(1));
        assert_eq!(release(Some(Tab(1)), Some(Tab(1)), 2, false, true), Release::Select(1), "a double click selects, and edits only with the setting");
        assert_eq!(release(Some(Tab(1)), Some(Tab(1)), 3, true, true), Release::Nothing);
        assert_eq!(release(Some(Bar), Some(Bar), 2, true, true), Release::NewTab);
        assert_eq!(release(Some(Bar), Some(Bar), 2, false, false), Release::NewTab);
        assert_eq!(release(Some(Chip(0)), Some(Chip(0)), 2, false, false), Release::NewTab, "a double click on a chip");
        assert_eq!(release(Some(Chip(0)), Some(Chip(0)), 1, false, false), Release::Nothing, "a chip's click toggles it instead");
        assert_eq!(release(Some(Bar), Some(Bar), 1, true, true), Release::Nothing);
        assert_eq!(release(Some(Bar), Some(Bar), 1, false, false), Release::Nothing);
        assert_eq!(release(Some(Hit::Add), Some(Hit::Add), 1, true, true), Release::Nothing, "the + is its own button");
        assert_eq!(release(Some(Tab(1)), None, 1, true, true), Release::Nothing, "released off the bar");
        assert_eq!(middle_release(Some(Tab(2)), Some(Tab(2)), true), Some(2));
        assert_eq!(middle_release(Some(Tab(2)), Some(Tab(2)), false), None);
        assert_eq!(middle_release(Some(Tab(1)), Some(Tab(2)), true), None);
        assert_eq!(middle_release(Some(Close(2)), Some(Tab(2)), true), Some(2), "a middle click anywhere on the tab closes it");
        assert_eq!(middle_release(Some(Bar), Some(Bar), true), None);
    }

    #[test]
    fn tabs_draw_left_of_the_pinned_add_button() {
        let s = Settings { prefs: Prefs { stretch: false, scrollable: true, ..defaults().prefs }, ..defaults() };
        let ten: Vec<TabInput> = (1..=10).map(|i| input(&format!("t{i}"), i)).collect();
        // A bar that scrolls: the tabs stop at the right margin, where the "+" is pinned.
        let scrolled = build(&ten, 0, 600.0, 0.0, None, &s, &six_per_char);
        assert!(scrolled.content > 0.0);
        assert_eq!(tab_area_width(&scrolled, 600.0), 576.0);
        // Without the "+" nothing is pinned and the tabs have the whole bar.
        let bare = Settings { prefs: Prefs { add_button: false, ..s.prefs }, ..s.clone() };
        assert_eq!(tab_area_width(&build(&ten, 0, 600.0, 0.0, None, &bare, &six_per_char), 600.0), 600.0);
        // A bar that fits: the "+" follows the last tab and nothing is cut.
        let three = [input("a", 1), input("b", 2), input("c", 3)];
        assert_eq!(tab_area_width(&build(&three, 0, 600.0, 0.0, None, &s, &six_per_char), 600.0), 600.0);
    }
}
