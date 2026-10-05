//! A pane's scrollbar: geometry, hit testing and the style Windows asks for
//! (iTerm2's legacy and overlay scrollers, `SV`, `PTYScroller`). Pure code.

use crate::layout::Rect;
use crate::palette::{Rgb, rgb};

/// Legacy scrollers take their width from the content; overlay scrollers float over it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Style {
    Legacy,
    Overlay,
}

pub const LEGACY_WIDTH: f32 = 15.0;
pub const OVERLAY_WIDTH: f32 = 11.0;
pub const MIN_THUMB: f32 = 18.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Geometry {
    pub track: Rect,
    pub thumb: Rect,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Hit {
    /// A press on the thumb, with the distance from the thumb's top.
    Thumb(f32),
    PageUp,
    PageDown,
    None,
}

impl Geometry {
    /// Where the marks and search-result ticks go: the track inset 2 vertically, and one
    /// narrower on the right in a dark window (`SV:2615-2648`, `updateMinimapFrameAnimated`).
    pub fn minimap(&self, dark: bool, scale: f32) -> Rect {
        let t = self.track;
        Rect { x: t.x, y: t.y + 2.0 * scale, w: if dark { t.w - scale } else { t.w }, h: (t.h - 4.0 * scale).max(0.0) }
    }
}

/// The width a pane's grid gives up: only the legacy style takes room, and `HideScrollbar`
/// gives it back. Overlay scrollers float over the content.
pub fn room(style: Style, hidden: bool, scale: f32) -> f32 {
    if hidden || style == Style::Overlay { 0.0 } else { LEGACY_WIDTH * scale }
}

/// The track at the right edge of `pane` and the thumb in it. The thumb is the visible
/// fraction of the whole buffer, at least `MIN_THUMB`; offset 0 (following the output) is
/// the bottom. The overlay style has nothing to show without history (iTerm2 hides its
/// overlay scroller); the legacy style then shows a thumb as tall as the track.
pub fn geometry(pane: Rect, style: Style, scale: f32, history: usize, lines: usize, offset: usize) -> Option<Geometry> {
    if style == Style::Overlay && history == 0 {
        return None;
    }
    let w = match style {
        Style::Legacy => LEGACY_WIDTH,
        Style::Overlay => OVERLAY_WIDTH,
    } * scale;
    let track = Rect { x: pane.x + pane.w - w, y: pane.y, w, h: pane.h };
    let total = (history + lines.max(1)) as f32;
    let h = if history == 0 { track.h } else { (track.h * lines as f32 / total).max(MIN_THUMB * scale).min(track.h) };
    let travel = track.h - h;
    let from_top = if history == 0 { 0.0 } else { 1.0 - offset.min(history) as f32 / history as f32 };
    Some(Geometry { track, thumb: Rect { x: track.x, y: track.y + travel * from_top, w, h } })
}

/// What a press at (`x`, `y`) lands on: the thumb (with its distance from the thumb's
/// top), the track above it (a page up), the track below it (a page down), or neither.
pub fn hit(g: &Geometry, x: f32, y: f32) -> Hit {
    let t = g.track;
    if x < t.x || x >= t.x + t.w || y < t.y || y >= t.y + t.h {
        return Hit::None;
    }
    if y < g.thumb.y {
        Hit::PageUp
    } else if y < g.thumb.y + g.thumb.h {
        Hit::Thumb(y - g.thumb.y)
    } else {
        Hit::PageDown
    }
}

/// What a left press at (`x`, `y`) does to a pane's scrollbar.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Press {
    /// Not on the bar: the pane takes the press.
    Pass,
    /// On the bar with nothing to scroll: the press is the bar's and does nothing.
    Swallow,
    /// A drag of the thumb, holding it this far below its top.
    Drag(f32),
    /// A page up (the track above the thumb) or down (below it).
    Page(i32),
}

/// A press on a bar with no history pages and drags nothing (the legacy thumb then fills the
/// track, so a hit there is a `Thumb`): it only keeps the press from the pane (iTerm2's scroller
/// is disabled when the content fits).
pub fn press(g: &Geometry, x: f32, y: f32, history: usize) -> Press {
    match hit(g, x, y) {
        Hit::None => Press::Pass,
        _ if history == 0 => Press::Swallow,
        Hit::Thumb(grab) => Press::Drag(grab),
        Hit::PageUp => Press::Page(1),
        Hit::PageDown => Press::Page(-1),
    }
}

/// The scrollback offset for a drag that holds the thumb `grab` below its top with the
/// pointer at `y`: the thumb follows the pointer within the track.
pub fn offset_for_drag(g: &Geometry, grab: f32, y: f32, history: usize) -> usize {
    let travel = g.track.h - g.thumb.h;
    if history == 0 || travel <= 0.0 {
        return 0;
    }
    let top = (y - grab - g.track.y).clamp(0.0, travel);
    ((1.0 - top / travel) * history as f32).round() as usize
}

/// Windows' "Always show scrollbars" (`DynamicScrollbars` 0) is the legacy style; dynamic
/// scrollbars (1, or no value) are the overlay style, as macOS follows its own setting.
pub fn style_from_value(v: Option<u32>) -> Style {
    if v == Some(0) { Style::Legacy } else { Style::Overlay }
}

/// The style Windows asks for now (`HKCU\Control Panel\Accessibility\DynamicScrollbars`).
#[cfg(windows)]
pub fn system_style() -> Style {
    use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
    let key: Vec<u16> = "Control Panel\\Accessibility\0".encode_utf16().collect();
    let name: Vec<u16> = "DynamicScrollbars\0".encode_utf16().collect();
    let mut data = 0u32;
    let mut bytes = std::mem::size_of::<u32>() as u32;
    let rc = unsafe { RegGetValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr(), RRF_RT_REG_DWORD, std::ptr::null_mut(), &mut data as *mut u32 as *mut _, &mut bytes) };
    style_from_value((rc == 0).then_some(data))
}

/// The style now: Windows' setting where there is one, else the overlay style.
pub fn current_style() -> Style {
    #[cfg(windows)]
    return system_style();
    #[cfg(not(windows))]
    Style::Overlay
}

/// The kinds of tick along the scrollbar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Series {
    Success,
    Other,
    Error,
    Manual,
    Annotation,
    Search,
}

pub const TICK_HEIGHT: f32 = 3.0;
pub const MINIMAP_ALPHA: f32 = 0.62;

/// iTerm2's tick series for a mark by its exit code (`iTermIntervalTreeObjectTypeForObject`): no
/// code is a manual mark, 0 success, 128 to 160 other, anything else an error.
pub fn series_of_mark(code: Option<i32>) -> Series {
    match code {
        None => Series::Manual,
        Some(0) => Series::Success,
        Some(128..=160) => Series::Other,
        Some(_) => Series::Error,
    }
}

/// The series to draw for a pane, in drawing order: search results first (iTerm2 adds the
/// search minimap before the marks), then the mark series, then annotations. `locations` is
/// ShowLocationsInScrollbar (it gates everything), `show_marks` ShowMarksInScrollbar (it gates
/// the marks and the annotations, and only counts with `locations`). Empty series are left out.
pub fn collect(locations: bool, show_marks: bool, search: &[usize], marks: &[(Series, usize)], notes: &[usize]) -> Vec<(Series, Vec<usize>)> {
    let mut out = Vec::new();
    if !locations {
        return out;
    }
    if !search.is_empty() {
        out.push((Series::Search, search.to_vec()));
    }
    if show_marks {
        for kind in [Series::Success, Series::Other, Series::Error, Series::Manual] {
            let lines: Vec<usize> = marks.iter().filter(|(k, _)| *k == kind).map(|(_, l)| *l).collect();
            if !lines.is_empty() {
                out.push((kind, lines));
            }
        }
        if !notes.is_empty() {
            out.push((Series::Annotation, notes.to_vec()));
        }
    }
    out
}

/// Ticks for `lines` (buffer lines, 0 the oldest) of a `total` line buffer along `track`
/// (`MM:85-125`): a line's place is its fraction of the whole buffer, `round(fraction *
/// (height - 3))` from the top, and a tick less than 2 points below the last drawn is
/// skipped. Lines are bucketed per pixel row first, so the work is the line count and the
/// memory the track height, whatever the history; the order of `lines` does not matter.
/// `style_narrow` is the overlay scroller's narrow state: 11 wide, 5 in (`SV:2626-2629`).
/// The height, the merge distance and the narrow values are points: times `scale`.
pub fn ticks(track: Rect, lines: &[usize], total: usize, style_narrow: bool, scale: f32) -> Vec<Rect> {
    if total == 0 || lines.is_empty() || track.h <= 0.0 {
        return Vec::new();
    }
    let height = TICK_HEIGHT * scale;
    let span = (track.h - height).max(0.0);
    let mut rows = vec![false; span.round() as usize + 1];
    for &line in lines.iter().filter(|&&l| l < total) {
        // iTerm2's `height - round((1 - fraction) * height)` in flipped coordinates, top-down.
        let from_bottom = ((1.0 - line as f64 / total as f64) * f64::from(span)).round();
        let y = (f64::from(span).round() - from_bottom).max(0.0) as usize;
        let last = rows.len() - 1;
        rows[y.min(last)] = true;
    }
    let (x, w) = if style_narrow { (track.x + 5.0 * scale, 11.0 * scale) } else { (track.x, track.w) };
    let mut out = Vec::new();
    let mut last: Option<usize> = None;
    for (y, _) in rows.iter().enumerate().filter(|(_, hit)| **hit) {
        if last.is_some_and(|l| (y as f32) < l as f32 + 2.0 * scale) {
            continue;
        }
        out.push(Rect { x, y: track.y + y as f32, w, h: height });
        last = Some(y);
    }
    out
}

/// A series' outline and fill (`SV:268-299`, `MM:183-186`): the mark colors saturated (hue
/// kept, saturation and brightness 1) with the fill dimmed 0.2 toward white; other marks keep
/// their color; manual marks gray; annotations yellow. Ticks ignore Use Theme Mark Colors.
pub fn series_colors(s: Series) -> (Rgb, Rgb) {
    use crate::marks::MarkColor;
    let mark = |c: MarkColor| {
        let [r, g, b, _] = crate::scene::mark_rgba(c);
        let u = |v: f32| (v * 255.0).round() as u8;
        rgb(u(r), u(g), u(b))
    };
    let saturated = |c: Rgb| {
        let (h, _, _) = crate::hexcolor::to_hsb(c);
        crate::hexcolor::from_hsb(h, 1.0, 1.0)
    };
    let dimmed = |c: Rgb| (c, crate::tab_theme::toward_gray(c, 0.2, 1.0));
    match s {
        Series::Success => dimmed(saturated(mark(MarkColor::Success))),
        Series::Other => dimmed(mark(MarkColor::Other)),
        Series::Error => dimmed(saturated(mark(MarkColor::Error))),
        Series::Manual => (crate::hexcolor::white(0.5), crate::hexcolor::white(0.7)),
        Series::Annotation => dimmed(rgb(255, 255, 0)),
        Series::Search => (rgb(128, 128, 0), rgb(255, 255, 0)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PANE: Rect = Rect { x: 0.0, y: 0.0, w: 800.0, h: 600.0 };

    #[test]
    fn scrollbar_geometry_follows_iterm2() {
        assert_eq!(room(Style::Legacy, false, 1.5), 22.5);
        assert_eq!(room(Style::Overlay, false, 1.5), 0.0);
        assert_eq!(room(Style::Legacy, true, 1.0), 0.0);

        let g = geometry(PANE, Style::Legacy, 1.0, 100, 50, 0).unwrap();
        assert_eq!(g.track, Rect { x: 785.0, y: 0.0, w: 15.0, h: 600.0 });
        assert_eq!(g.thumb.h, 200.0);
        assert_eq!(g.thumb.y, 400.0, "offset 0 is the bottom");
        assert_eq!((g.thumb.x, g.thumb.w), (785.0, 15.0));
        let top = geometry(PANE, Style::Legacy, 1.0, 100, 50, 100).unwrap();
        assert_eq!(top.thumb.y, 0.0, "the full history is the top");
        let mid = geometry(PANE, Style::Legacy, 1.0, 100, 50, 50).unwrap();
        assert_eq!(mid.thumb.y, 200.0);
        let past = geometry(PANE, Style::Legacy, 1.0, 100, 50, 500).unwrap();
        assert_eq!(past.thumb.y, 0.0, "an offset past the history clamps");

        let huge = geometry(PANE, Style::Legacy, 1.0, 100_000, 50, 0).unwrap();
        assert_eq!(huge.thumb.h, MIN_THUMB);
        assert_eq!(huge.thumb.y + huge.thumb.h, 600.0);
        let huge_top = geometry(PANE, Style::Legacy, 1.0, 100_000, 50, 100_000).unwrap();
        assert_eq!(huge_top.thumb.y, 0.0);

        assert_eq!(geometry(PANE, Style::Overlay, 1.0, 0, 50, 0), None);
        let flat = geometry(PANE, Style::Legacy, 1.0, 0, 50, 0).unwrap();
        assert_eq!(flat.thumb, flat.track, "no history: the thumb fills the track");
    }

    #[test]
    fn geometry_scales_and_sits_at_the_pane_edge() {
        let pane = Rect { x: 100.0, y: 40.0, w: 400.0, h: 300.0 };
        let g = geometry(pane, Style::Legacy, 2.0, 10, 10, 0).unwrap();
        assert_eq!(g.track, Rect { x: 470.0, y: 40.0, w: 30.0, h: 300.0 });
        assert_eq!((g.thumb.y, g.thumb.h), (190.0, 150.0));
        let o = geometry(pane, Style::Overlay, 1.0, 10, 10, 0).unwrap();
        assert_eq!((o.track.x, o.track.w), (489.0, OVERLAY_WIDTH));
        let tiny = geometry(pane, Style::Legacy, 2.0, 100_000, 10, 0).unwrap();
        assert_eq!(tiny.thumb.h, MIN_THUMB * 2.0, "the minimum thumb scales");
    }

    #[test]
    fn hits_follow_iterm2() {
        let g = geometry(PANE, Style::Legacy, 1.0, 100, 50, 50).unwrap();
        assert_eq!(g.thumb.y, 200.0);
        assert_eq!(hit(&g, 790.0, 230.0), Hit::Thumb(30.0));
        assert_eq!(hit(&g, 790.0, 100.0), Hit::PageUp);
        assert_eq!(hit(&g, 790.0, 450.0), Hit::PageDown);
        assert_eq!(hit(&g, 700.0, 230.0), Hit::None);
        assert_eq!(hit(&g, 790.0, 700.0), Hit::None);
        assert_eq!(hit(&g, 790.0, 200.0), Hit::Thumb(0.0), "the thumb's top edge is the thumb");
        assert_eq!(hit(&g, 790.0, 400.0), Hit::PageDown, "its bottom edge is below it");

        let top = Rect { y: 0.0, ..g.thumb };
        let g2 = Geometry { thumb: top, ..g };
        assert_eq!(offset_for_drag(&g, 30.0, 30.0, 100), 100, "thumb at the top: the full history");
        assert_eq!(offset_for_drag(&g, 30.0, -50.0, 100), 100, "past the top clamps");
        assert_eq!(offset_for_drag(&g, 30.0, 430.0, 100), 0, "thumb at the bottom: 0");
        assert_eq!(offset_for_drag(&g, 30.0, 900.0, 100), 0);
        assert_eq!(offset_for_drag(&g, 30.0, 230.0, 100), 50);
        assert_eq!(offset_for_drag(&g2, 0.0, 0.0, 0), 0, "no history: nothing to drag");
    }

    #[test]
    fn minimap_insets_like_the_session_view() {
        let g = geometry(PANE, Style::Legacy, 1.0, 100, 50, 0).unwrap();
        assert_eq!(g.minimap(false, 1.0), Rect { x: 785.0, y: 2.0, w: 15.0, h: 596.0 });
        assert_eq!(g.minimap(true, 1.0), Rect { x: 785.0, y: 2.0, w: 14.0, h: 596.0 });
    }

    #[test]
    fn ticks_and_minimap_scale_with_the_display() {
        let track = |h: f32| Rect { x: 10.0, y: 20.0, w: 30.0, h };
        // 200%: height 6, merge distance 4, the narrow overlay 22 wide, 10 in.
        let t = ticks(track(406.0), &[0, 3, 4, 99], 400, false, 2.0);
        assert_eq!(t.iter().map(|r| r.y - 20.0).collect::<Vec<_>>(), vec![0.0, 4.0, 99.0]);
        assert!(t.iter().all(|r| r.h == 2.0 * TICK_HEIGHT));
        let t = ticks(track(406.0), &[0], 400, true, 2.0);
        assert_eq!((t[0].x, t[0].w), (20.0, 22.0));
        // 125%: height 3.75, and the last line sits at the track's height less one tick.
        let t = ticks(track(203.75), &[0, 99], 100, false, 1.25);
        assert_eq!(t.iter().map(|r| r.y - 20.0).collect::<Vec<_>>(), vec![0.0, 198.0]);
        assert!(t.iter().all(|r| r.h == 3.75));
        // Lines 0 and 1 of 100 at 125% are 2 px apart: under 2.5, one tick.
        assert_eq!(ticks(track(203.75), &[0, 1], 200, false, 1.25).len(), 1);
        // The minimap's insets scale too.
        let g = geometry(PANE, Style::Legacy, 2.0, 100, 50, 0).unwrap();
        assert_eq!(g.minimap(false, 2.0), Rect { x: 770.0, y: 4.0, w: 30.0, h: 592.0 });
        assert_eq!(g.minimap(true, 2.0), Rect { x: 770.0, y: 4.0, w: 28.0, h: 592.0 });
    }

    #[test]
    fn style_follows_the_registry_value() {
        assert_eq!(style_from_value(Some(0)), Style::Legacy);
        assert_eq!(style_from_value(Some(1)), Style::Overlay);
        assert_eq!(style_from_value(None), Style::Overlay);
    }

    #[test]
    fn ticks_follow_iterm2() {
        let track = |h: f32| Rect { x: 10.0, y: 20.0, w: 15.0, h };
        // Height 203, 100 lines: line 0 at the top, line 99 at round(0.99 * 200).
        let t = ticks(track(203.0), &[0, 99], 100, false, 1.0);
        assert_eq!(t.iter().map(|r| r.y - 20.0).collect::<Vec<_>>(), vec![0.0, 198.0]);
        assert!(t.iter().all(|r| r.h == TICK_HEIGHT && r.x == 10.0 && r.w == 15.0));
        // Lines 500 and 501 of 1000 in a 103 high track both land on y 50: one tick.
        let t = ticks(track(103.0), &[500, 501], 1000, false, 1.0);
        assert_eq!(t.iter().map(|r| r.y - 20.0).collect::<Vec<_>>(), vec![50.0]);
        // Lines 0 and 1 of 100,000: within 2 points, one tick.
        assert_eq!(ticks(track(203.0), &[0, 1], 100_000, false, 1.0).len(), 1);
        // A tick 2 points below the last is kept; 1 point is not.
        let t = ticks(track(103.0), &[0, 1, 2, 3], 100, false, 1.0);
        assert_eq!(t.iter().map(|r| r.y - 20.0).collect::<Vec<_>>(), vec![0.0, 2.0]);
        assert!(ticks(track(203.0), &[], 100, false, 1.0).is_empty());
        assert!(ticks(track(203.0), &[5], 0, false, 1.0).is_empty());
        // The narrow overlay style: 11 wide, 5 in.
        let t = ticks(track(203.0), &[0], 100, true, 1.0);
        assert_eq!((t[0].x, t[0].w), (15.0, 11.0));
    }

    #[test]
    fn ticks_stay_bounded_and_order_free() {
        let track = Rect { x: 0.0, y: 0.0, w: 15.0, h: 403.0 };
        let lines: Vec<usize> = (0..2_000_000).rev().collect();
        let t = ticks(track, &lines, 2_000_000, false, 1.0);
        assert!(t.len() <= 201, "one tick per two pixel rows at most: {}", t.len());
        assert!(t.windows(2).all(|w| w[1].y - w[0].y >= 2.0), "sorted top-down, 2 apart");
        assert_eq!(t[0].y, 0.0);
        let up: Vec<usize> = (0..2_000_000).collect();
        assert_eq!(ticks(track, &up, 2_000_000, false, 1.0), t, "the input order does not matter");
        assert!(ticks(track, &[9, 2_000_000, 5_000_000], 2_000_000, false, 1.0).len() == 1, "lines past the buffer are ignored");
    }

    #[test]
    fn series_follow_iterm2() {
        use crate::palette::rgb;
        assert_eq!(series_colors(Series::Search), (rgb(128, 128, 0), rgb(255, 255, 0)));
        assert_eq!(series_colors(Series::Manual), (rgb(128, 128, 128), rgb(179, 179, 179)));
        // Success: the success color's hue at full saturation and brightness, its fill 0.2 toward white.
        let (out, fill) = series_colors(Series::Success);
        let (h, _, _) = crate::hexcolor::to_hsb(rgb(137, 193, 255));
        assert_eq!(out, crate::hexcolor::from_hsb(h, 1.0, 1.0));
        assert_eq!(fill, crate::tab_theme::toward_gray(out, 0.2, 1.0));
        let (out, fill) = series_colors(Series::Error);
        let (h, _, _) = crate::hexcolor::to_hsb(rgb(252, 114, 109));
        assert_eq!(out, crate::hexcolor::from_hsb(h, 1.0, 1.0));
        assert_eq!(fill, crate::tab_theme::toward_gray(out, 0.2, 1.0));
        // Other keeps its own color (iTerm2 only dims it); annotations are yellow.
        let (out, fill) = series_colors(Series::Other);
        assert_eq!((out, fill), (rgb(219, 216, 109), crate::tab_theme::toward_gray(rgb(219, 216, 109), 0.2, 1.0)));
        let (out, fill) = series_colors(Series::Annotation);
        assert_eq!((out, fill), (rgb(255, 255, 0), crate::tab_theme::toward_gray(rgb(255, 255, 0), 0.2, 1.0)));
        assert_eq!((TICK_HEIGHT, MINIMAP_ALPHA), (3.0, 0.62));
    }

    /// Phase B: with no history a press on the bar is swallowed (no drag, no page); with history
    /// it drags the thumb or pages by the side it is on; off the bar it goes to the pane.
    #[test]
    fn a_press_without_history_drags_and_pages_nothing() {
        let pane = Rect { x: 0.0, y: 0.0, w: 800.0, h: 600.0 };
        let flat = geometry(pane, Style::Legacy, 1.0, 0, 50, 0).unwrap();
        assert_eq!(press(&flat, 790.0, 300.0, 0), Press::Swallow, "the full-height thumb is a hit, and does nothing");
        assert_eq!(press(&flat, 100.0, 300.0, 0), Press::Pass);
        let g = geometry(pane, Style::Legacy, 1.0, 100, 50, 0).unwrap();
        assert_eq!(press(&g, 100.0, 300.0, 100), Press::Pass);
        assert_eq!(press(&g, 790.0, g.thumb.y + 5.0, 100), Press::Drag(5.0));
        assert_eq!(press(&g, 790.0, g.thumb.y - 5.0, 100), Press::Page(1), "above the thumb: toward older lines");
        let up = geometry(pane, Style::Legacy, 1.0, 100, 50, 50).unwrap();
        assert_eq!(press(&up, 790.0, up.thumb.y + up.thumb.h + 5.0, 100), Press::Page(-1));
    }

    /// Task 8 (Review Focus 4): search ticks come from the open find bar's matches only, the two
    /// keys gate as iTerm2's do, and the marks take their series from their exit codes.
    #[test]
    fn ticks_follow_the_find_bar() {
        let marks = [(Series::Success, 3), (Series::Error, 9), (Series::Manual, 4), (Series::Success, 12)];
        let kinds = |v: &[(Series, Vec<usize>)]| v.iter().map(|(k, _)| *k).collect::<Vec<_>>();
        // The find bar is open with 3 matches: 3 search ticks, drawn before the marks.
        let open = collect(true, true, &[5, 6, 20], &marks, &[7]);
        assert_eq!(kinds(&open), vec![Series::Search, Series::Success, Series::Error, Series::Manual, Series::Annotation]);
        assert_eq!(open[0].1, vec![5, 6, 20]);
        assert_eq!(open[1].1, vec![3, 12]);
        // Closed: no matches, no search series.
        let closed = collect(true, true, &[], &marks, &[]);
        assert_eq!(kinds(&closed), vec![Series::Success, Series::Error, Series::Manual]);
        // ShowLocationsInScrollbar off: nothing at all.
        assert!(collect(false, true, &[5], &marks, &[7]).is_empty());
        assert!(collect(false, false, &[5], &marks, &[7]).is_empty());
        // ShowMarksInScrollbar off: search ticks, no mark or annotation ticks.
        assert_eq!(kinds(&collect(true, false, &[5, 6, 20], &marks, &[7])), vec![Series::Search]);
    }

    #[test]
    fn mark_series_follow_the_exit_code() {
        assert_eq!(series_of_mark(None), Series::Manual, "no code: a manual mark");
        assert_eq!(series_of_mark(Some(0)), Series::Success);
        assert_eq!(series_of_mark(Some(1)), Series::Error);
        assert_eq!(series_of_mark(Some(127)), Series::Error);
        assert_eq!(series_of_mark(Some(128)), Series::Other);
        assert_eq!(series_of_mark(Some(160)), Series::Other);
        assert_eq!(series_of_mark(Some(161)), Series::Error);
        assert_eq!(series_of_mark(Some(-1)), Series::Error);
    }
}
