//! The cursor's look and geometry, as iTerm2 draws it (`iTermCursorRenderer.m`). Pure: the
//! scene reads a `CursorLook` per pane and draws what it says.

use alacritty_terminal::vte::ansi::CursorShape;

use crate::globals::Globals;
use crate::layout::Rect;

/// The measures iTerm2 reads for the cursor's box, bar and underline. Lengths in points are
/// multiplied by `scale` where they are used.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Geometry {
    /// The window's scale factor: one point is `scale` pixels.
    pub scale: f32,
    /// The font's own cell, without the spacings. (0, 0) means "the same as the cell".
    pub base_cell: (f32, f32),
    /// Advanced "FullHeightCursor".
    pub full_height: bool,
    /// Advanced "VerticalBarCursorWidth", points.
    pub bar_width: f32,
    /// Advanced "UnderlineCursorHeight", points.
    pub underline_height: f32,
    /// Advanced "UnderlineCursorOffset", points.
    pub underline_offset: f32,
}

impl Default for Geometry {
    fn default() -> Self {
        Geometry { scale: 1.0, base_cell: (0.0, 0.0), full_height: false, bar_width: 1.0, underline_height: 2.0, underline_offset: 0.0 }
    }
}

impl Geometry {
    pub fn from_globals(g: &Globals, scale: f32, base_cell: (f32, f32)) -> Geometry {
        Geometry {
            scale,
            base_cell,
            full_height: g.bool("FullHeightCursor"),
            bar_width: g.number("VerticalBarCursorWidth").unwrap_or(1.0) as f32,
            underline_height: g.number("UnderlineCursorHeight").unwrap_or(2.0) as f32,
            underline_offset: g.number("UnderlineCursorOffset").unwrap_or(0.0) as f32,
        }
    }
}

/// The box a block cursor fills at `row`, `col` (`origin` is the pane's text origin).
/// iTerm2 centers a spacing-free cursor in the line (`iTermCursorRenderer.m:332-339`,
/// rounded to whole points in Metal); with Full Height Cursor it is as tall as the taller of
/// the line and the font's cell and starts on the line's top (`:337`).
pub fn box_rect(geo: &Geometry, origin: (f32, f32), cell: (f32, f32), row: usize, col: usize, wide: bool) -> Rect {
    let (cw, ch) = cell;
    let base_w = if geo.base_cell.0 > 0.0 { geo.base_cell.0 } else { cw };
    let base_h = if geo.base_cell.1 > 0.0 { geo.base_cell.1 } else { ch };
    let scale = geo.scale.max(f32::MIN_POSITIVE);
    let line = row as f32 * ch;
    let (top, h) = if geo.full_height {
        (line, ch.max(base_h))
    } else {
        (line + (((ch - base_h) / 2.0 / scale).round() * scale).max(0.0), ch.min(base_h))
    };
    let w = cw.min(base_w) * if wide { 2.0 } else { 1.0 };
    Rect { x: origin.0 + col as f32 * cw, y: origin.1 + top, w, h }
}

/// The solid part of the cursor: the box, the bar (`bar_width` points wide, the box's height),
/// or the underline (`underline_height` points high on the box's bottom, raised by
/// `underline_offset` points).
pub fn cursor_rect(geo: &Geometry, shape: CursorShape, origin: (f32, f32), cell: (f32, f32), row: usize, col: usize, wide: bool) -> Rect {
    let b = box_rect(geo, origin, cell, row, col, wide);
    match shape {
        CursorShape::Beam => Rect { w: geo.bar_width * geo.scale, ..b },
        CursorShape::Underline => {
            let h = geo.underline_height * geo.scale;
            Rect { y: b.y + b.h - h - geo.underline_offset * geo.scale, h, ..b }
        }
        _ => b,
    }
}

/// Everything the scene draws for one pane's cursor. Later tasks fill the later fields.
#[derive(Clone, Debug, PartialEq)]
pub struct CursorLook {
    /// The cursor is shown (not hidden by blink or an Auto Composer).
    pub visible: bool,
    pub geometry: Geometry,
    pub hide_unfocused: bool,
    pub alpha: f32,
    pub offset: (f32, f32),
    pub smart: Option<(f32, f32)>,
    pub boost: f32,
    pub shadow: bool,
    pub outline: bool,
    pub smear: Option<[(f32, f32); 6]>,
    pub ime: Option<crate::ime::Drawn>,
}

impl CursorLook {
    /// iTerm2's defaults: opaque, in place, no smart color, no boost, no shadow, no outline,
    /// no smear, no IME.
    pub fn shown(visible: bool) -> CursorLook {
        CursorLook {
            visible,
            geometry: Geometry::default(),
            hide_unfocused: false,
            alpha: 1.0,
            offset: (0.0, 0.0),
            smart: None,
            boost: 0.0,
            shadow: false,
            outline: false,
            smear: None,
            ime: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ORIGIN: (f32, f32) = (0.0, 0.0);

    fn geo(scale: f32, base: (f32, f32)) -> Geometry {
        Geometry { scale, base_cell: base, ..Geometry::default() }
    }

    fn r(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect { x, y, w, h }
    }

    #[test]
    fn cursor_rect_follows_iterm2() {
        // The line is 4 taller than the font's cell: the box is centered.
        let g = geo(1.0, (10.0, 20.0));
        assert_eq!(box_rect(&g, ORIGIN, (10.0, 24.0), 1, 2, false), r(20.0, 26.0, 10.0, 20.0));
        // Negative spacing: no offset, the cell's height.
        assert_eq!(box_rect(&g, ORIGIN, (10.0, 16.0), 1, 2, false), r(20.0, 16.0, 10.0, 16.0));
        // The spacing-free width caps the width.
        assert_eq!(box_rect(&g, ORIGIN, (12.0, 24.0), 1, 2, false).w, 10.0);
        assert_eq!(box_rect(&g, ORIGIN, (10.0, 24.0), 1, 2, true).w, 20.0);
        // Metal rounds the offset to whole points at a doubled scale.
        let g2 = geo(2.0, (20.0, 41.0));
        assert_eq!(box_rect(&g2, ORIGIN, (20.0, 48.0), 0, 0, false).y, 4.0);
        // The origin moves every rect.
        assert_eq!(box_rect(&g, (5.0, 7.0), (10.0, 24.0), 1, 2, false), r(25.0, 33.0, 10.0, 20.0));
        // No base cell: the cell itself.
        assert_eq!(box_rect(&Geometry::default(), ORIGIN, (10.0, 24.0), 1, 2, false), r(20.0, 24.0, 10.0, 24.0));
    }

    #[test]
    fn full_height_cursor_follows_iterm2() {
        let g = Geometry { full_height: true, ..geo(1.0, (10.0, 20.0)) };
        assert_eq!(box_rect(&g, ORIGIN, (10.0, 24.0), 1, 0, false), r(0.0, 24.0, 10.0, 24.0));
        // The line is shorter than the font's cell: the top stays on the line's top
        // (`iTermCursorRenderer.m:337`), so it overflows below.
        assert_eq!(box_rect(&g, ORIGIN, (10.0, 16.0), 1, 0, false), r(0.0, 16.0, 10.0, 20.0));
    }

    #[test]
    fn bar_and_underline_follow_points() {
        let cell = (10.0, 24.0);
        let g = geo(1.0, (10.0, 20.0));
        assert_eq!(cursor_rect(&g, CursorShape::Beam, ORIGIN, cell, 1, 2, false), r(20.0, 26.0, 1.0, 20.0));
        assert_eq!(cursor_rect(&g, CursorShape::Underline, ORIGIN, cell, 1, 2, false), r(20.0, 44.0, 10.0, 2.0));
        assert_eq!(cursor_rect(&g, CursorShape::Block, ORIGIN, cell, 1, 2, false), box_rect(&g, ORIGIN, cell, 1, 2, false));
        let g15 = geo(1.5, (10.0, 20.0));
        assert_eq!(cursor_rect(&g15, CursorShape::Beam, ORIGIN, cell, 1, 2, false).w, 1.5);
        let u = cursor_rect(&g15, CursorShape::Underline, ORIGIN, cell, 1, 2, false);
        assert_eq!(u.h, 3.0);
        let raised = Geometry { underline_offset: 2.0, ..g15 };
        let ur = cursor_rect(&raised, CursorShape::Underline, ORIGIN, cell, 1, 2, false);
        assert_eq!(ur.y, u.y - 3.0);
        let fat = Geometry { bar_width: 3.0, underline_height: 1.0, ..geo(2.0, (10.0, 20.0)) };
        assert_eq!(cursor_rect(&fat, CursorShape::Beam, ORIGIN, cell, 1, 2, false).w, 6.0);
        assert_eq!(cursor_rect(&fat, CursorShape::Underline, ORIGIN, cell, 1, 2, false).h, 2.0);
        // A wide cell doubles the underline's width, not the bar's.
        assert_eq!(cursor_rect(&g, CursorShape::Underline, ORIGIN, cell, 1, 2, true).w, 20.0);
        assert_eq!(cursor_rect(&g, CursorShape::Beam, ORIGIN, cell, 1, 2, true).w, 1.0);
    }

    #[test]
    fn geometry_reads_the_advanced_keys() {
        let mut g = Globals::empty(std::path::Path::new("none"));
        let d = Geometry::from_globals(&g, 2.0, (9.0, 18.0));
        assert_eq!((d.scale, d.base_cell, d.full_height, d.bar_width, d.underline_height, d.underline_offset), (2.0, (9.0, 18.0), false, 1.0, 2.0, 0.0));
        g.set("FullHeightCursor", toml::Value::Boolean(true));
        g.set("VerticalBarCursorWidth", toml::Value::Float(3.0));
        let d = Geometry::from_globals(&g, 1.0, (0.0, 0.0));
        assert!(d.full_height);
        assert_eq!(d.bar_width, 3.0);
    }
}
