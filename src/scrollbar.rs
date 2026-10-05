//! A pane's scrollbar: geometry, hit testing and the style Windows asks for
//! (iTerm2's legacy and overlay scrollers, `SV`, `PTYScroller`). Pure code.

use crate::layout::Rect;

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
    pub fn minimap(&self, dark: bool) -> Rect {
        let t = self.track;
        Rect { x: t.x, y: t.y + 2.0, w: if dark { t.w - 1.0 } else { t.w }, h: (t.h - 4.0).max(0.0) }
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
        assert_eq!(g.minimap(false), Rect { x: 785.0, y: 2.0, w: 15.0, h: 596.0 });
        assert_eq!(g.minimap(true), Rect { x: 785.0, y: 2.0, w: 14.0, h: 596.0 });
    }

    #[test]
    fn style_follows_the_registry_value() {
        assert_eq!(style_from_value(Some(0)), Style::Legacy);
        assert_eq!(style_from_value(Some(1)), Style::Overlay);
        assert_eq!(style_from_value(None), Style::Overlay);
    }
}
