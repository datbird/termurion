//! iTerm2's tab bar without drawing (milestone 13c): the metrics, the width rules, scrolling
//! and the overflow menu. Pure code in points; the app multiplies by the window scale once.
//!
//! Citations: `PSM` = `ThirdParty/PSMTabBarControl/source/PSMTabBarControl.m`, `YS` =
//! `.../PSMYosemiteTabStyle.m` under `~/gitrepos/iterm2-ref`; `RTV` =
//! `sources/TerminalView/iTermRootTerminalView.m`.

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
/// passes 2).
pub fn widths(desired: &[f32], bar_width: f32, p: &Prefs, scale: f32) -> Widths {
    let n = desired.len();
    // The left margin is the bar's inset, 0 here (`YS leftMarginForTabBarControl`).
    let available = |overflow: bool| bar_width - right_margin(overflow, p.add_button);
    if p.scrollable && n > 0 {
        // `PSM layoutScrollableHorizontalTabsWithCellCount:animate:`: when the natural widths
        // pass the room they scroll, else the bar looks as it does without scrolling.
        let natural: Vec<f32> = desired
            .iter()
            .map(|&d| if p.uneven { p.scrollable_width.max(MAX_WIDTH.min(d)) } else { p.scrollable_width })
            .collect();
        let total: f32 = natural.iter().sum();
        if total > available(false) {
            return Widths { widths: natural, overflow: false, content: total };
        }
    }
    // `PSM settledHorizontalCellWidths`: the overflow variant when the regular widths drop tabs
    // (or the add-tab button already takes the same margin).
    let regular = arrange(desired, available(false), p, scale);
    let widths = if p.add_button || regular.len() < n { arrange(desired, available(true), p, scale) } else { regular };
    let overflow = widths.len() < n;
    Widths { widths, overflow, content: 0.0 }
}

/// `PSM cellWidthsForHorizontalArrangementWithOverflow:` without pinned tabs or group chips
/// (13d) and with iTerm2's Yosemite intercell spacing of 0.
fn arrange(desired: &[f32], available: f32, p: &Prefs, scale: f32) -> Vec<f32> {
    let n = desired.len();
    if p.uneven {
        // `PSM variableCellWidthsWithOverflow:`: only when every tab fits at its clamped width.
        let clamped: Vec<f32> = desired.iter().map(|&d| p.min_width.max(d.min(MAX_WIDTH))).collect();
        if clamped.iter().sum::<f32>() <= available {
            return clamped;
        }
    }
    // `PSM shouldUseOptimalWidthWithOverflow:`.
    if !p.stretch && p.optimum * n as f32 <= available {
        return vec![p.optimum; n];
    }
    let visible = if p.min_width * n as f32 <= available {
        n
    } else if p.min_width > 0.0 {
        ((available / p.min_width).floor().max(0.0) as usize).min(n)
    } else {
        n
    };
    equal_shares(available, visible, scale)
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
        let w = widths(&three, 600.0, &p, 2.0);
        assert_eq!(w, Widths { widths: vec![192.0, 192.0, 192.0], overflow: false, content: 0.0 });
        // 553 points is 1106 device pixels: 368 each and 2 left over for the first tabs.
        assert_eq!(widths(&three, 577.0, &p, 2.0).widths, vec![184.5, 184.5, 184.0]);

        let fixed = Prefs { stretch: false, ..p };
        assert_eq!(widths(&three, 600.0, &fixed, 2.0).widths, vec![175.0, 175.0, 175.0]);
        // 5 x 175 does not fit in 576, 5 x 75 does: stretched instead.
        assert_eq!(
            widths(&[100.0; 5], 600.0, &fixed, 2.0).widths,
            vec![115.5, 115.5, 115.0, 115.0, 115.0]
        );

        // 9 tabs of minimum 75 in 476 points: floor(476 / 75) = 6 show, the rest overflow.
        let w = widths(&[100.0; 9], 500.0, &p, 2.0);
        assert_eq!(w.widths.len(), 6);
        assert!(w.overflow);
        assert_eq!(w.content, 0.0);
        // Without the add-tab button the regular layout drops tabs, so the overflow variant
        // (24 points of margin) is used, not the full 500.
        let bare = Prefs { add_button: false, ..p };
        let w = widths(&[100.0; 9], 500.0, &bare, 2.0);
        assert_eq!(w.widths.len(), 6);
        assert!(w.overflow);
        assert_eq!(w.widths.iter().sum::<f32>(), 476.0);
        // Without the add-tab button and nothing dropped, the whole bar is used.
        assert_eq!(widths(&three, 600.0, &bare, 2.0).widths, vec![200.0, 200.0, 200.0]);

        // Uneven: clamped to the minimum and 280 when every tab fits.
        let uneven = Prefs { uneven: true, ..p };
        let desired = [60.0, 150.0, 400.0];
        assert_eq!(widths(&desired, 800.0, &uneven, 2.0).widths, vec![75.0, 150.0, 280.0]);
        // The clamped sum 505 passes 476: the equal rule, 952 device pixels over 3.
        assert_eq!(widths(&desired, 500.0, &uneven, 2.0).widths, vec![159.0, 158.5, 158.5]);
    }

    #[test]
    fn scrolling_follows_iterm2() {
        let p = Prefs { scrollable: true, ..prefs() };
        let w = widths(&[100.0; 10], 600.0, &p, 2.0);
        assert_eq!(w, Widths { widths: vec![120.0; 10], overflow: false, content: 1200.0 });
        // Tabs that fit at 120 use the normal rule and do not scroll.
        let w = widths(&[100.0; 3], 600.0, &p, 2.0);
        assert_eq!(w, Widths { widths: vec![192.0, 192.0, 192.0], overflow: false, content: 0.0 });
        // Uneven: max(120, min(280, desired)).
        let uneven = Prefs { uneven: true, ..p };
        let w = widths(&[60.0, 150.0, 400.0, 200.0, 300.0], 600.0, &uneven, 2.0);
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
}
