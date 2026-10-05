//! Underline and strikethrough: iTerm2's styles, colors, geometry and dash patterns.
//!
//! Pure: a cell's flags to a style, the colors to draw it in, and the strokes (rectangles
//! with a dash pattern) the renderer fills. Lengths are pixels; `scale` is the window's
//! device scale, the grid iTerm2's `retinaRound` rounds to.

use crate::palette::Rgb;
use alacritty_terminal::term::cell::Flags;

/// `iTermShaderTypes.h:42-59`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Style {
    None,
    Single,
    Double,
    Curly,
    Dotted,
    Dashed,
    /// An OSC 8 link's dashes.
    DashedSingle,
}

/// A cell's underline style and whether it is struck through
/// (`iTermMetalPerFrameState.m:1569-1608`): an SGR style wins over the link's dashes, and
/// strikethrough is added to either.
pub fn style_for(flags: Flags, hyperlink: bool, underline_hyperlinks: bool) -> (Style, bool) {
    let style = if flags.contains(Flags::UNDERLINE) {
        Style::Single
    } else if flags.contains(Flags::DOUBLE_UNDERLINE) {
        Style::Double
    } else if flags.contains(Flags::UNDERCURL) {
        Style::Curly
    } else if flags.contains(Flags::DOTTED_UNDERLINE) {
        Style::Dotted
    } else if flags.contains(Flags::DASHED_UNDERLINE) {
        Style::Dashed
    } else if hyperlink && underline_hyperlinks {
        Style::DashedSingle
    } else {
        Style::None
    };
    (style, flags.contains(Flags::STRIKEOUT))
}

/// The cell's SGR 58 color, else the profile's Underline Color when it is on, else the text
/// color (`iTermTextRendererTransientState.mm:447-455`).
pub fn color_for(cell_color: Option<Rgb>, profile_color: Option<Rgb>, text: Rgb) -> Rgb {
    cell_color.or(profile_color).unwrap_or(text)
}

/// The primary face's numbers at the current size, in pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Metrics {
    /// The font's underline distance below the baseline.
    pub underline_offset: f32,
    pub thickness: f32,
    pub x_height: f32,
    /// The baseline's distance from the cell top.
    pub baseline: f32,
}

fn retina_round(v: f32, scale: f32) -> f32 {
    (v * scale).round() / scale
}

fn retina_floor(v: f32, scale: f32) -> f32 {
    (v * scale).floor() / scale
}

/// The underline's top from the cell top (`iTermTextDrawingHelper.m:3046-3065`): the font's
/// distance below the baseline kept between 0.4 and 0.5 of the x-height, one device half
/// pixel up, and never below the cell.
pub fn underline_y(m: &Metrics, cell_h: f32, scale: f32) -> f32 {
    let min = retina_round(m.x_height * 0.4, scale);
    let dist = if m.underline_offset < min {
        min
    } else if m.underline_offset > m.x_height / 2.0 {
        m.x_height / 2.0
    } else {
        m.underline_offset
    };
    let preferred = retina_round(m.baseline + dist, scale) - 1.0 / (2.0 * scale);
    retina_round(preferred, scale).min(retina_floor(cell_h - thickness(m, scale), scale))
}

/// The strikethrough's top from the cell top (`:3068-3073`).
pub fn strike_y(m: &Metrics, scale: f32) -> f32 {
    retina_round(m.baseline - m.x_height / 2.0, scale)
}

/// At least half a point, on the pixel grid (`:3075-3077`).
pub fn thickness(m: &Metrics, scale: f32) -> f32 {
    retina_round(m.thickness, scale).max(0.5)
}

/// One filled line: `y` its absolute top (y down), `on`/`off` its dashes (0 for solid),
/// `shift` added to x before the phase is taken.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stroke {
    pub y: f32,
    pub h: f32,
    pub on: f32,
    pub off: f32,
    pub shift: f32,
}

/// The strokes as the shader computes them (`iTermTextShaderCommon.metal:121-229`,
/// `:256-268`), unsnapped. Double and Curly count from the cell bottom: Double's lower line
/// sits `max(0, 1 pt - thickness)` up, its second two thicknesses above that; Curly's two
/// one-point lines start half a point up and are on in turns every 3 points.
pub fn raw_strokes(style: Style, underline_y: f32, cell_bottom: f32, thickness: f32, scale: f32) -> Vec<Stroke> {
    let line = |y: f32, h: f32, on: f32, off: f32, shift: f32| Stroke { y, h, on: on * scale, off: off * scale, shift: shift * scale };
    match style {
        Style::None => vec![],
        Style::Single => vec![Stroke { y: underline_y, h: thickness, on: 0.0, off: 0.0, shift: 0.0 }],
        Style::Dotted => vec![Stroke { y: underline_y, h: thickness, on: scale, off: scale, shift: 0.0 }],
        Style::Dashed | Style::DashedSingle => vec![Stroke { y: underline_y, h: thickness, on: 4.0 * scale, off: 3.0 * scale, shift: 0.0 }],
        Style::Double => {
            let lower = (scale - thickness).max(0.0);
            let upper = lower + 2.0 * thickness;
            vec![
                Stroke { y: cell_bottom - lower - thickness, h: thickness, on: 0.0, off: 0.0, shift: 0.0 },
                Stroke { y: cell_bottom - upper - thickness, h: thickness, on: 0.0, off: 0.0, shift: 0.0 },
            ]
        }
        Style::Curly => {
            let lower = scale / 2.0;
            let upper = lower + scale;
            vec![line(cell_bottom - lower - scale, scale, 3.0, 3.0, 0.0), line(cell_bottom - upper - scale, scale, 3.0, 3.0, 3.0)]
        }
    }
}

/// `raw_strokes` on whole device pixels (round half up, at least one pixel thick): iTerm2
/// anti-aliases a half pixel, Tandem's quad pipeline does not.
pub fn strokes(style: Style, underline_y: f32, cell_bottom: f32, thickness: f32, scale: f32) -> Vec<Stroke> {
    raw_strokes(style, underline_y, cell_bottom, thickness, scale).into_iter().map(snap).collect()
}

/// One stroke on the pixel grid (also the strikethrough's solid line).
pub fn snap(s: Stroke) -> Stroke {
    Stroke { y: (s.y + 0.5).floor(), h: s.h.round().max(1.0), ..s }
}

/// The on-intervals of `s` across `x0..x1`, phased by absolute x (`fmod(x + shift, on + off)`).
pub fn dash_pieces(x0: f32, x1: f32, s: &Stroke) -> Vec<(f32, f32)> {
    if x1 <= x0 {
        return vec![];
    }
    let period = s.on + s.off;
    if s.on <= 0.0 || s.off <= 0.0 || period <= 0.0 {
        return vec![(x0, x1)];
    }
    let mut out = Vec::new();
    let mut k = ((x0 + s.shift) / period).floor();
    loop {
        let start = k * period - s.shift;
        if start >= x1 {
            break;
        }
        let (a, b) = (start.max(x0), (start + s.on).min(x1));
        if b > a {
            out.push((a, b));
        }
        k += 1.0;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::palette::rgb;
    use alacritty_terminal::term::cell::Flags;

    #[test]
    fn styles_follow_iterm2() {
        let s = |f: Flags, link: bool, on: bool| style_for(f, link, on);
        assert_eq!(s(Flags::UNDERLINE, false, true), (Style::Single, false));
        assert_eq!(s(Flags::DOUBLE_UNDERLINE, false, true), (Style::Double, false));
        assert_eq!(s(Flags::UNDERCURL, false, true), (Style::Curly, false));
        assert_eq!(s(Flags::DOTTED_UNDERLINE, false, true), (Style::Dotted, false));
        assert_eq!(s(Flags::DASHED_UNDERLINE, false, true), (Style::Dashed, false));
        assert_eq!(s(Flags::UNDERLINE | Flags::STRIKEOUT, false, true), (Style::Single, true));
        assert_eq!(s(Flags::STRIKEOUT, false, true), (Style::None, true));
        assert_eq!(s(Flags::empty(), true, true), (Style::DashedSingle, false));
        assert_eq!(s(Flags::empty(), true, false), (Style::None, false));
        assert_eq!(s(Flags::UNDERLINE, true, true), (Style::Single, false));
    }

    #[test]
    fn colors_follow_iterm2() {
        let (red, green, white) = (rgb(255, 0, 0), rgb(0, 255, 0), rgb(255, 255, 255));
        assert_eq!(color_for(Some(red), Some(green), white), red);
        assert_eq!(color_for(None, Some(green), white), green);
        assert_eq!(color_for(None, None, white), white);
    }

    #[test]
    fn geometry_follows_iterm2() {
        let m = Metrics { underline_offset: 2.0, thickness: 1.0, x_height: 8.0, baseline: 14.0 };
        assert_eq!(underline_y(&m, 17.0, 1.0), 16.0);
        let m4 = Metrics { x_height: 4.0, ..m };
        assert_eq!(underline_y(&m4, 17.0, 1.0), 16.0);
        let t = |v: f32, s: f32| thickness(&Metrics { thickness: v, ..m }, s);
        assert_eq!(t(0.3, 1.0), 0.5);
        assert_eq!(t(1.4, 1.0), 1.0);
        assert_eq!(t(1.4, 2.0), 1.5);
        assert_eq!(strike_y(&m, 1.0), 10.0);
    }

    fn solid(y: f32, h: f32) -> Stroke {
        Stroke { y, h, on: 0.0, off: 0.0, shift: 0.0 }
    }

    #[test]
    fn patterns_follow_iterm2() {
        // The source values (STC:121-137, :256-268), before snapping to pixels.
        assert_eq!(raw_strokes(Style::Single, 16.0, 17.0, 1.0, 1.0), vec![solid(16.0, 1.0)]);
        assert_eq!(raw_strokes(Style::Double, 16.0, 17.0, 1.0, 1.0), vec![solid(16.0, 1.0), solid(14.0, 1.0)]);
        let curly = |a: f32, b: f32, s: f32| {
            vec![
                Stroke { y: a, h: s, on: 3.0 * s, off: 3.0 * s, shift: 0.0 },
                Stroke { y: b, h: s, on: 3.0 * s, off: 3.0 * s, shift: 3.0 * s },
            ]
        };
        assert_eq!(raw_strokes(Style::Curly, 16.0, 17.0, 1.0, 1.0), curly(15.5, 14.5, 1.0));
        assert_eq!(raw_strokes(Style::Double, 32.0, 34.0, 2.0, 2.0), vec![solid(32.0, 2.0), solid(28.0, 2.0)]);
        assert_eq!(raw_strokes(Style::Curly, 32.0, 34.0, 2.0, 2.0), curly(31.0, 29.0, 2.0));
        // Drawn strokes sit on whole pixels (the quad pipeline does not anti-alias).
        assert_eq!(strokes(Style::Curly, 16.0, 17.0, 1.0, 1.0), curly(16.0, 15.0, 1.0));
        assert_eq!(strokes(Style::Curly, 32.0, 34.0, 2.0, 2.0), curly(31.0, 29.0, 2.0));
        assert_eq!(strokes(Style::Single, 16.0, 17.0, 1.0, 1.0), vec![solid(16.0, 1.0)]);
        assert_eq!(strokes(Style::Single, 16.0, 17.0, 0.5, 1.0), vec![solid(16.0, 1.0)], "at least one pixel");
        let one = |s| strokes(s, 16.0, 17.0, 1.0, 1.0);
        assert_eq!(one(Style::Dotted), vec![Stroke { y: 16.0, h: 1.0, on: 1.0, off: 1.0, shift: 0.0 }]);
        let dashed = Stroke { y: 16.0, h: 1.0, on: 4.0, off: 3.0, shift: 0.0 };
        assert_eq!(one(Style::Dashed), vec![dashed]);
        assert_eq!(one(Style::DashedSingle), vec![dashed]);
        assert_eq!(dash_pieces(0.0, 14.0, &dashed), vec![(0.0, 4.0), (7.0, 11.0)]);
        assert_eq!(dash_pieces(5.0, 14.0, &dashed), vec![(7.0, 11.0)]);
        assert_eq!(dash_pieces(2.0, 3.0, &solid(0.0, 1.0)), vec![(2.0, 3.0)]);
        // Curly's upper line is on where the lower is off.
        let up = strokes(Style::Curly, 16.0, 17.0, 1.0, 1.0)[1];
        assert_eq!(dash_pieces(0.0, 6.0, &up), vec![(3.0, 6.0)]);
        let d2 = strokes(Style::Dashed, 32.0, 34.0, 2.0, 2.0)[0];
        assert_eq!((d2.on, d2.off), (8.0, 6.0));
    }
}
