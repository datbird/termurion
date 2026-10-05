//! Powerline symbols drawn by Tandem (iTerm2's `iTermBoxDrawingBezierCurveFactory`, Powerline
//! half): which code points are drawn, which are double width, and the shape of each as pieces
//! in cell units, written by hand from the geometry. Nothing is read from a file: every shape is
//! built here, rasterized with the box pipeline (`boxdraw::rasterize`).

use crate::boxdraw::{Piece, Weight};

/// The code points iTerm2 always draws itself when Draw Powerline Glyphs is on.
pub const CORE: &[(u32, u32)] = &[(0xE0A0, 0xE0A2), (0xE0B0, 0xE0B3), (0xE0B9, 0xE0BF)];

/// How an extended symbol sits in its box (iTerm2's `iTermPowerlineDrawingOptions`): mirrored
/// left to right, squeezed to half the box width, and a one point column of ink past the left or
/// right edge so a separator meets its neighbor's background without a seam.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Options {
    pub mirrored: bool,
    pub half_width: bool,
    pub bleed_left: bool,
    pub bleed_right: bool,
}

const NONE: Options = Options { mirrored: false, half_width: false, bleed_left: false, bleed_right: false };
const LEFT: Options = Options { bleed_left: true, ..NONE };
const RIGHT: Options = Options { bleed_right: true, ..NONE };
const MIRROR: Options = Options { mirrored: true, ..NONE };
const HALF: Options = Options { half_width: true, ..NONE };

/// iTerm2's `powerlineExtendedSymbols`, by code point: the symbol's name and its options.
const EXTENDED: &[(u32, &str, Options)] = &[
    (0xE0A3, "uniE0A3_column-number", NONE),
    (0xE0B0, "uniE0B0_Powerline_normal-left", LEFT),
    (0xE0B2, "uniE0B2_Powerline_normal-right", RIGHT),
    (0xE0B4, "uniE0B4_right-half-circle-thick", LEFT),
    (0xE0B5, "uniE0B5_right-half-circle-thin", NONE),
    (0xE0B6, "uniE0B6_left-half-circle-thick", RIGHT),
    (0xE0B7, "uniE0B7_left-half-circle-thin", NONE),
    (0xE0B8, "uniE0B8_lower-left-triangle", LEFT),
    (0xE0C0, "uniE0C0_flame-thick", LEFT),
    (0xE0C1, "uniE0C1_flame-thin", NONE),
    (0xE0C2, "uniE0C0_flame-thick", Options { mirrored: true, bleed_left: true, ..NONE }),
    (0xE0C3, "uniE0C1_flame-thin", MIRROR),
    (0xE0C4, "uniE0C4_PowerlineExtraSymbols", NONE),
    (0xE0C5, "uniE0C4_PowerlineExtraSymbols", MIRROR),
    (0xE0C6, "uniE0C6_PowerlineExtraSymbols", NONE),
    (0xE0C7, "uniE0C6_PowerlineExtraSymbols", MIRROR),
    (0xE0C8, "uniE0C8_PowerlineExtraSymbols", LEFT),
    (0xE0C9, "uniE0C9_PowerlineExtraSymbols", LEFT),
    (0xE0CA, "uniE0C8_PowerlineExtraSymbols", MIRROR),
    (0xE0CB, "uniE0C9_PowerlineExtraSymbols", Options { mirrored: true, bleed_right: true, ..NONE }),
    (0xE0CC, "uniE0CC_PowerlineExtraSymbols", NONE),
    (0xE0CD, "uniE0CD_PowerlineExtraSymbols", NONE),
    (0xE0CE, "uniE0CE_lego_separator", NONE),
    (0xE0CF, "uniE0CF_lego_separator_thin", NONE),
    (0xE0D0, "uniE0D0_PowerlineExtraSymbols", NONE),
    (0xE0D1, "uniE0D1_lego_block_sideways", LEFT),
    (0xE0D2, "uniE0D2_PowerlineExtraSymbols", HALF),
    (0xE0D4, "uniE0D2_PowerlineExtraSymbols", Options { half_width: true, mirrored: true, ..NONE }),
    (0xE0D6, "uniE0D6_Powerline_normal-right-inverse-cutout", LEFT),
    (0xE0D7, "uniE0D7_Powerline_normal-left-inverse-cutout", RIGHT),
];

/// iTerm2's `doubleWidthPowerlineSymbols`.
const WIDE: &[u32] = &[
    0xE0B8, 0xE0B9, 0xE0BA, 0xE0BB, 0xE0BC, 0xE0BD, 0xE0BE, 0xE0BF, 0xE0C0, 0xE0C1, 0xE0C2, 0xE0C3, 0xE0C4, 0xE0C5, 0xE0C6, 0xE0C7, 0xE0C8, 0xE0C9, 0xE0CA, 0xE0CB, 0xE0CC, 0xE0CD, 0xE0CE, 0xE0CF, 0xE0D0, 0xE0D1, 0xE0D2, 0xE0D4,
];

/// The extended table's entry for `c`, when `on` (`SupportPowerlineExtendedSymbols`).
pub fn extended(c: u32, on: bool) -> Option<(&'static str, Options)> {
    if !on {
        return None;
    }
    EXTENDED.iter().find(|e| e.0 == c).map(|e| (e.1, e.2))
}

/// Whether iTerm2 draws `c` over two cells, when `on` (`MakeSomePowerlineSymbolsWide`).
pub fn wide(c: u32, on: bool) -> bool {
    on && WIDE.contains(&c)
}

/// Whether `c` is in the core set.
pub fn is_core(c: u32) -> bool {
    CORE.iter().any(|(a, b)| (*a..=*b).contains(&c))
}

/// Whether Tandem draws `c` as Powerline: the core always, the extended table with `extended_on`.
pub fn drawn(c: u32, extended_on: bool) -> bool {
    is_core(c) || extended(c, extended_on).is_some()
}

/// What a symbol is drawn from.
enum Shape {
    /// One of iTerm2's component strings (the box drawing path language), solid or stroked.
    Components(&'static str, bool),
    /// A hand-built shape, by name.
    Named(&'static str),
}

struct Layout {
    shape: Shape,
    opts: Options,
    /// Width over height of a shape that keeps its proportions in the cell (the core symbols
    /// that are not stretched); `None` stretches to the box.
    aspect: Option<f32>,
}

/// The aspect of the three core symbols that keep their proportions.
const KEPT: f32 = 37.0 / 60.0;

/// How `c` is drawn. The core wins over the extended table, as in iTerm2 (`isPowerlineGlyph` is
/// asked first).
fn layout(c: u32) -> Option<Layout> {
    let named = |name, opts, aspect| Some(Layout { shape: Shape::Named(name), opts, aspect });
    let comps = |s, solid| Some(Layout { shape: Shape::Components(s, solid), opts: NONE, aspect: None });
    match c {
        0xE0A0 => named("PowerlineVersionControlBranch", NONE, Some(KEPT)),
        0xE0A1 => named("PowerlineLN", NONE, Some(KEPT)),
        0xE0A2 => named("PowerlinePadlock", NONE, Some(KEPT)),
        0xE0B0 => named("PowerlineSolidRightArrow", LEFT, None),
        0xE0B1 => named("PowerlineLineRightArrow", NONE, None),
        0xE0B2 => named("PowerlineSolidLeftArrow", RIGHT, None),
        0xE0B3 => named("PowerlineLineLeftArrow", NONE, None),
        0xE0B9 | 0xE0BF => comps("a1g7", false),
        0xE0BA => comps("a7g1 g1g7 g7a7", true),
        0xE0BB | 0xE0BD => comps("a7g1", false),
        0xE0BC => comps("a1g1 g1a7 a7a1", true),
        0xE0BE => comps("g1a1 a1g7 g7g1", true),
        _ => EXTENDED.iter().find(|e| e.0 == c).and_then(|e| named(e.1, e.2, None)),
    }
}

// Hand-written geometry, in the unit box (0 to 1 across and down, y down). Never read from a file.

fn solid(points: &[(f32, f32)]) -> Piece {
    Piece::Solid(points.to_vec())
}

fn rect(x0: f32, y0: f32, x1: f32, y1: f32) -> Piece {
    solid(&[(x0, y0), (x1, y0), (x1, y1), (x0, y1)])
}

fn line(from: (f32, f32), to: (f32, f32), weight: Weight) -> Piece {
    Piece::Line { from, to, weight }
}

fn curve(from: (f32, f32), c1: (f32, f32), c2: (f32, f32), to: (f32, f32), weight: Weight) -> Piece {
    Piece::Curve { from, c1, c2, to, weight }
}

/// Points on an ellipse from angle `a0` to `a1` (degrees, 0 to the right, 90 down).
fn arc(cx: f32, cy: f32, rx: f32, ry: f32, a0: f32, a1: f32, steps: u32) -> Vec<(f32, f32)> {
    (0..=steps)
        .map(|i| {
            let a = (a0 + (a1 - a0) * i as f32 / steps as f32).to_radians();
            ((cx + rx * a.cos()).clamp(0.0, 1.0), (cy + ry * a.sin()).clamp(0.0, 1.0))
        })
        .collect()
}

/// The four cubic curves of an ellipse outline (kappa 0.5523).
fn ellipse(cx: f32, cy: f32, rx: f32, ry: f32, weight: Weight) -> Vec<Piece> {
    const K: f32 = 0.5523;
    let (kx, ky) = (rx * K, ry * K);
    vec![
        curve((cx + rx, cy), (cx + rx, cy + ky), (cx + kx, cy + ry), (cx, cy + ry), weight),
        curve((cx, cy + ry), (cx - kx, cy + ry), (cx - rx, cy + ky), (cx - rx, cy), weight),
        curve((cx - rx, cy), (cx - rx, cy - ky), (cx - kx, cy - ry), (cx, cy - ry), weight),
        curve((cx, cy - ry), (cx + kx, cy - ry), (cx + rx, cy - ky), (cx + rx, cy), weight),
    ]
}

fn outline(points: &[(f32, f32)], weight: Weight) -> Vec<Piece> {
    (0..points.len()).map(|i| line(points[i], points[(i + 1) % points.len()], weight)).collect()
}

/// A grid of squares: `F` solid, `B` heavy outline, `O` light outline, anything else empty.
fn squares(rows: &[&str], fill: f32) -> Vec<Piece> {
    let (cols, n) = (rows.iter().map(|r| r.len()).max().unwrap_or(1) as f32, rows.len() as f32);
    let mut out = Vec::new();
    for (r, row) in rows.iter().enumerate() {
        for (c, ch) in row.bytes().enumerate() {
            let (x0, y0) = (c as f32 / cols, r as f32 / n);
            let (x1, y1) = (x0 + fill / cols, y0 + fill / n);
            match ch {
                b'F' => out.push(rect(x0, y0, x1, y1)),
                b'B' => out.extend(outline(&[(x0, y0), (x1, y0), (x1, y1), (x0, y1)], Weight::Heavy)),
                b'O' => out.extend(outline(&[(x0, y0), (x1, y0), (x1, y1), (x0, y1)], Weight::Light)),
                _ => {}
            }
        }
    }
    out
}

/// The upright "N" of the line and column symbols, in the box `x0..x1`, `y0..y1`.
fn letter_n(x0: f32, y0: f32, x1: f32, y1: f32) -> Piece {
    let (w, h) = (x1 - x0, y1 - y0);
    let at = |u: f32, v: f32| (x0 + u * w, y0 + v * h);
    solid(&[at(0.0, 0.0), at(0.3, 0.0), at(0.7, 0.58), at(0.7, 0.0), at(1.0, 0.0), at(1.0, 1.0), at(0.7, 1.0), at(0.3, 0.42), at(0.3, 1.0), at(0.0, 1.0)])
}

/// The shape called `name` in the unit box, before mirroring and bleed.
fn named(name: &str) -> Vec<Piece> {
    use Weight::{Heavy, Light};
    match name {
        // The version control branch: a post, and a path leaving it that bends up to an arrowhead.
        "PowerlineVersionControlBranch" => vec![
            rect(0.06, 0.06, 0.24, 0.94),
            curve((0.15, 0.86), (0.15, 0.62), (0.72, 0.80), (0.72, 0.36), Heavy),
            solid(&[(0.48, 0.38), (0.72, 0.06), (0.96, 0.38)]),
        ],
        // "LN": a stepped L over an N.
        "PowerlineLN" => vec![solid(&[(0.0, 0.02), (0.30, 0.02), (0.30, 0.30), (0.78, 0.30), (0.78, 0.46), (0.0, 0.46)]), letter_n(0.22, 0.56, 1.0, 0.98)],
        // The closed padlock: a shackle over a body.
        "PowerlinePadlock" => vec![
            rect(0.04, 0.44, 0.96, 0.98),
            line((0.26, 0.46), (0.26, 0.32), Heavy),
            curve((0.26, 0.32), (0.26, 0.02), (0.74, 0.02), (0.74, 0.32), Heavy),
            line((0.74, 0.32), (0.74, 0.46), Heavy),
        ],
        "PowerlineSolidRightArrow" | "uniE0B0_Powerline_normal-left" => vec![solid(&[(0.0, 0.0), (1.0, 0.5), (0.0, 1.0)])],
        "PowerlineSolidLeftArrow" | "uniE0B2_Powerline_normal-right" => vec![solid(&[(1.0, 0.0), (0.0, 0.5), (1.0, 1.0)])],
        "PowerlineLineRightArrow" => vec![line((0.0, 0.0), (1.0, 0.5), Light), line((0.0, 1.0), (1.0, 0.5), Light)],
        "PowerlineLineLeftArrow" => vec![line((1.0, 0.0), (0.0, 0.5), Light), line((1.0, 1.0), (0.0, 0.5), Light)],
        // Column number: a "C" bracket over an N.
        "uniE0A3_column-number" => vec![solid(&[(0.0, 0.0), (1.0, 0.0), (1.0, 0.12), (0.22, 0.12), (0.22, 0.36), (1.0, 0.36), (1.0, 0.48), (0.0, 0.48)]), letter_n(0.0, 0.56, 1.0, 1.0)],
        "uniE0B4_right-half-circle-thick" => vec![solid(&arc(0.0, 0.5, 1.0, 0.5, -90.0, 90.0, 24))],
        "uniE0B6_left-half-circle-thick" => vec![solid(&arc(1.0, 0.5, 1.0, 0.5, 270.0, 90.0, 24))],
        "uniE0B5_right-half-circle-thin" => vec![
            curve((0.0, 0.0), (0.5523, 0.0), (1.0, 0.2238), (1.0, 0.5), Light),
            curve((1.0, 0.5), (1.0, 0.7762), (0.5523, 1.0), (0.0, 1.0), Light),
        ],
        "uniE0B7_left-half-circle-thin" => vec![
            curve((1.0, 0.0), (0.4477, 0.0), (0.0, 0.2238), (0.0, 0.5), Light),
            curve((0.0, 0.5), (0.0, 0.7762), (0.4477, 1.0), (1.0, 1.0), Light),
        ],
        "uniE0B8_lower-left-triangle" => vec![solid(&[(0.0, 0.0), (0.0, 1.0), (1.0, 1.0)])],
        // Flames: a solid body on the left whose right edge breaks into tongues.
        "uniE0C0_flame-thick" => vec![solid(&[
            (0.0, 0.18), (0.16, 0.08), (0.38, 0.28), (0.52, 0.14), (0.80, 0.30), (1.0, 0.26), (0.76, 0.42), (0.92, 0.50), (0.60, 0.56), (0.96, 0.70), (0.50, 0.72), (0.72, 0.86), (0.30, 0.80), (0.22, 0.96), (0.0, 0.92),
        ])],
        "uniE0C1_flame-thin" => vec![
            curve((0.0, 0.22), (0.30, 0.22), (0.45, 0.02), (0.96, 0.10), Light),
            curve((0.0, 0.50), (0.35, 0.50), (0.55, 0.28), (0.92, 0.40), Light),
            curve((0.0, 0.76), (0.30, 0.76), (0.50, 0.58), (0.84, 0.66), Light),
            curve((0.0, 0.96), (0.30, 0.92), (0.52, 0.86), (0.70, 0.90), Light),
        ],
        // Pixelated squares dissolving to the right.
        "uniE0C4_PowerlineExtraSymbols" => squares(
            &["FFFBFF.O", "FFFF.F.O", "FFFFB...", "FFF.B...", "FFFBFFO.", "FFFFF...", "FF..B..O", "FFFO.O..", "FF..O..."],
            0.72,
        ),
        "uniE0C6_PowerlineExtraSymbols" => squares(&["FFO..", "FFB.O", "FF...", "FFBO.", "FFF..", "FFBOO", "FF..."], 0.74),
        // A waveform: a bar on the left with spikes of different lengths.
        "uniE0C8_PowerlineExtraSymbols" => {
            let lengths = [0.50, 0.90, 0.62, 1.0, 0.70, 0.96, 0.52, 0.84, 0.60];
            let n = lengths.len() as f32;
            let mut pts = vec![(0.0, 0.0)];
            for (i, l) in lengths.iter().enumerate() {
                let (top, mid, bottom) = (i as f32 / n, (i as f32 + 0.5) / n, (i as f32 + 1.0) / n);
                pts.push((0.10, top));
                pts.push((*l, mid));
                pts.push((0.10, bottom));
            }
            pts.push((0.0, 1.0));
            vec![solid(&pts)]
        }
        // A block with a half circle bitten out of its left side.
        "uniE0C9_PowerlineExtraSymbols" => {
            let mut pts = vec![(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)];
            pts.extend(arc(0.0, 0.5, 1.0, 0.5, 90.0, -90.0, 24));
            vec![solid(&pts)]
        }
        // Two pentagons and a hexagon, solid and outlined.
        "uniE0CC_PowerlineExtraSymbols" => vec![
            solid(&[(0.0, 0.0), (0.28, 0.0), (0.40, 0.20), (0.28, 0.40), (0.0, 0.40)]),
            solid(&[(0.0, 0.60), (0.28, 0.60), (0.40, 0.80), (0.28, 1.0), (0.0, 1.0)]),
            solid(&[(0.50, 0.50), (0.62, 0.30), (0.88, 0.30), (1.0, 0.50), (0.88, 0.70), (0.62, 0.70)]),
        ],
        "uniE0CD_PowerlineExtraSymbols" => {
            let mut out = outline(&[(0.02, 0.02), (0.28, 0.02), (0.40, 0.20), (0.28, 0.40), (0.02, 0.40)], Heavy);
            out.extend(outline(&[(0.02, 0.60), (0.28, 0.60), (0.40, 0.80), (0.28, 0.98), (0.02, 0.98)], Heavy));
            out.extend(outline(&[(0.50, 0.50), (0.62, 0.30), (0.88, 0.30), (0.98, 0.50), (0.88, 0.70), (0.62, 0.70)], Heavy));
            out
        }
        // A lego block seen from a corner: top and two sides.
        "uniE0CE_lego_separator" => vec![
            solid(&[(0.5, 0.04), (0.96, 0.28), (0.5, 0.50), (0.04, 0.28)]),
            solid(&[(0.04, 0.35), (0.47, 0.57), (0.47, 0.98), (0.04, 0.76)]),
            solid(&[(0.53, 0.57), (0.96, 0.35), (0.96, 0.76), (0.53, 0.98)]),
        ],
        "uniE0CF_lego_separator_thin" => {
            let mut out = outline(&[(0.5, 0.04), (0.96, 0.28), (0.5, 0.50), (0.04, 0.28)], Light);
            out.extend(outline(&[(0.04, 0.35), (0.47, 0.57), (0.47, 0.98), (0.04, 0.76)], Light));
            out.extend(outline(&[(0.53, 0.57), (0.96, 0.35), (0.96, 0.76), (0.53, 0.98)], Light));
            out
        }
        // Four rings, each a ring seen at a slant.
        "uniE0D0_PowerlineExtraSymbols" => [(0.26, 0.25), (0.72, 0.25), (0.26, 0.75), (0.72, 0.75)].iter().flat_map(|(x, y)| ellipse(*x, *y, 0.21, 0.21, Heavy)).collect(),
        // A lego block lying on its side: the block and two studs.
        "uniE0D1_lego_block_sideways" => vec![rect(0.0, 0.0, 0.80, 1.0), rect(0.80, 0.14, 1.0, 0.40), rect(0.80, 0.60, 1.0, 0.86)],
        // A "K" made of two slanted blocks.
        "uniE0D2_PowerlineExtraSymbols" => vec![solid(&[(0.0, 0.0), (1.0, 0.0), (0.34, 0.46), (0.0, 0.46)]), solid(&[(0.0, 0.54), (0.34, 0.54), (1.0, 1.0), (0.0, 1.0)])],
        // The corners of a diamond cut out of a block.
        "uniE0D6_Powerline_normal-right-inverse-cutout" => vec![solid(&[(0.0, 0.0), (1.0, 0.0), (0.0, 0.5)]), solid(&[(0.0, 0.5), (1.0, 1.0), (0.0, 1.0)])],
        "uniE0D7_Powerline_normal-left-inverse-cutout" => vec![solid(&[(1.0, 0.0), (0.0, 0.0), (1.0, 0.5)]), solid(&[(1.0, 0.5), (0.0, 1.0), (1.0, 1.0)])],
        _ => Vec::new(),
    }
}

/// What `c` is made of, in cell units across the symbol's own box (one cell, two for a wide
/// symbol, half of that for a half-width one), mirrored when its options ask. Empty when `c` is
/// not drawn (the extended table is empty with `extended_on` false).
pub fn pieces(c: u32, extended_on: bool) -> Vec<Piece> {
    if !drawn(c, extended_on) {
        return Vec::new();
    }
    let Some(layout) = layout(c) else { return Vec::new() };
    let mut list = match layout.shape {
        Shape::Components(s, solid) => crate::boxdraw::path_pieces(s, solid),
        Shape::Named(name) => named(name),
    };
    if layout.opts.mirrored {
        for p in &mut list {
            mirror(p);
        }
    }
    list
}

fn mirror(p: &mut Piece) {
    let flip = |q: &mut (f32, f32)| q.0 = 1.0 - q.0;
    match p {
        Piece::Line { from, to, .. } => {
            flip(from);
            flip(to);
        }
        Piece::Curve { from, c1, c2, to, .. } => {
            for q in [from, c1, c2, to] {
                flip(q);
            }
        }
        Piece::Solid(points) => points.iter_mut().for_each(flip),
    }
}

/// Draws `c` into `mask` (`w` by `h`, zeroed), when it is a Powerline symbol. The box is the
/// whole mask (two cells for a wide symbol; the caller sizes it). Shapes that keep their
/// proportions sit centered; the others fill the box. As in iTerm2 the image is mirrored about
/// the box, then squeezed to half width, then a one point column of ink is added on a bleed side.
pub(crate) fn draw(c: char, mask: &mut [u8], w: u16, h: u16, scale: f32) -> bool {
    let Some(layout) = layout(u32::from(c)) else { return false };
    if let Shape::Components(s, solid) = layout.shape {
        crate::boxdraw::draw_path(mask, w, h, s, solid, scale);
        return true;
    }
    let Shape::Named(name) = layout.shape else { return false };
    let (cw, ch) = (f32::from(w), f32::from(h));
    // The destination of the unit box inside the cell.
    let (dx, dy, dw, dh) = match layout.aspect {
        Some(a) if cw / ch > a => ((cw - ch * a) / 2.0, 0.0, ch * a, ch),
        Some(a) => (0.0, (ch - cw / a) / 2.0, cw, cw / a),
        None => (0.0, 0.0, cw, ch),
    };
    let o = layout.opts;
    let map = |(u, v): (f32, f32)| -> (f32, f32) {
        let mut x = dx + u * dw;
        if o.half_width {
            x *= 0.5;
        }
        if o.mirrored {
            x = cw - x;
        }
        (x, dy + v * dh)
    };
    let unit = (dh * 0.05).max(scale);
    let width = |weight: Weight| match weight {
        Weight::Light => unit,
        Weight::Heavy | Weight::Double => unit * 2.0,
    };
    let mut fills: Vec<Vec<(f32, f32)>> = Vec::new();
    let mut strokes: Vec<(f32, Piece)> = Vec::new();
    for p in named(name) {
        match p {
            Piece::Solid(points) => fills.push(points.into_iter().map(map).collect()),
            Piece::Line { from, to, weight } => strokes.push((width(weight), Piece::Line { from: map(from), to: map(to), weight })),
            Piece::Curve { from, c1, c2, to, weight } => strokes.push((width(weight), Piece::Curve { from: map(from), c1: map(c1), c2: map(c2), to: map(to), weight })),
            }
    }
    // One point of ink past the edge of the image on a bleed side.
    if o.bleed_left {
        fills.push([(0.0, 0.0), (scale / dw, 0.0), (scale / dw, 1.0), (0.0, 1.0)].into_iter().map(map).collect());
    }
    if o.bleed_right {
        fills.push([(1.0 - scale / dw, 0.0), (1.0, 0.0), (1.0, 1.0), (1.0 - scale / dw, 1.0)].into_iter().map(map).collect());
    }
    fill_all(mask, w, h, &fills);
    stroke_all(mask, w, h, &strokes);
    true
}

/// Fills every polygon (all turned the same way, so overlaps join instead of cancelling).
fn fill_all(mask: &mut [u8], w: u16, h: u16, polygons: &[Vec<(f32, f32)>]) {
    use zeno::{Command, Mask, Point};
    let mut commands: Vec<Command> = Vec::new();
    for poly in polygons.iter().filter(|p| p.len() >= 3) {
        let area: f32 = (0..poly.len()).map(|i| poly[i].0 * poly[(i + 1) % poly.len()].1 - poly[(i + 1) % poly.len()].0 * poly[i].1).sum();
        let ordered: Vec<(f32, f32)> = if area < 0.0 { poly.iter().rev().copied().collect() } else { poly.clone() };
        commands.push(Command::MoveTo(Point::new(ordered[0].0, ordered[0].1)));
        commands.extend(ordered[1..].iter().map(|p| Command::LineTo(Point::new(p.0, p.1))));
        commands.push(Command::Close);
    }
    if commands.is_empty() {
        return;
    }
    let mut out = vec![0u8; mask.len()];
    let mut render = Mask::new(&commands[..]);
    render.size(u32::from(w), u32::from(h));
    render.render_into(&mut out, None);
    merge(mask, &out);
}

/// Strokes lines and curves, each width in one pass, square caps and miter joins.
fn stroke_all(mask: &mut [u8], w: u16, h: u16, strokes: &[(f32, Piece)]) {
    use zeno::{Cap, Command, Join, Mask, Point, Stroke};
    let mut widths: Vec<f32> = strokes.iter().map(|s| s.0).collect();
    widths.sort_by(f32::total_cmp);
    widths.dedup();
    for width in widths {
        let mut commands: Vec<Command> = Vec::new();
        for (_, p) in strokes.iter().filter(|s| s.0 == width) {
            match p {
                Piece::Line { from, to, .. } => {
                    commands.push(Command::MoveTo(Point::new(from.0, from.1)));
                    commands.push(Command::LineTo(Point::new(to.0, to.1)));
                }
                Piece::Curve { from, c1, c2, to, .. } => {
                    commands.push(Command::MoveTo(Point::new(from.0, from.1)));
                    commands.push(Command::CurveTo(Point::new(c1.0, c1.1), Point::new(c2.0, c2.1), Point::new(to.0, to.1)));
                }
                _ => {}
            }
        }
        let mut out = vec![0u8; mask.len()];
        let mut stroke = Stroke::new(width);
        stroke.cap(Cap::Square).join(Join::Miter);
        let mut render = Mask::new(&commands[..]);
        render.size(u32::from(w), u32::from(h)).style(&stroke);
        render.render_into(&mut out, None);
        merge(mask, &out);
    }
}

fn merge(mask: &mut [u8], src: &[u8]) {
    for (d, s) in mask.iter_mut().zip(src) {
        *d = (*d).max(*s);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::boxdraw;

    const NO: Options = Options { mirrored: false, half_width: false, bleed_left: false, bleed_right: false };

    #[test]
    fn powerline_sets_follow_iterm2() {
        assert!(boxdraw::drawn('\u{E0B0}', true, false));
        assert!(!boxdraw::drawn('\u{E0B0}', false, false));
        assert_eq!(extended(0xE0C0, true), Some(("uniE0C0_flame-thick", Options { bleed_left: true, ..NO })));
        assert_eq!(extended(0xE0C2, true), Some(("uniE0C0_flame-thick", Options { mirrored: true, bleed_left: true, ..NO })));
        assert_eq!(extended(0xE0D4, true), Some(("uniE0D2_PowerlineExtraSymbols", Options { half_width: true, mirrored: true, ..NO })));
        assert_eq!(extended(0xE0C0, false), None);
        assert!(wide(0xE0B8, true) && !wide(0xE0B0, true) && !wide(0xE0B8, false));
        assert!(!wide(0xE0D3, true), "iTerm2's list skips it");
        assert!(wide(0xE0D4, true) && wide(0xE0D2, true) && !wide(0xE0D6, true));
        // The extended table has iTerm2's 30 entries.
        let n = (0xE0A0..=0xE0D8u32).filter(|c| extended(*c, true).is_some()).count();
        assert_eq!(n, 30);
        // Every drawn symbol has pieces, and they stay inside the symbol's own box.
        let core = CORE.iter().flat_map(|(a, b)| *a..=*b);
        let ext = (0xE0A0..=0xE0D8u32).filter(|c| extended(*c, true).is_some());
        for c in core.chain(ext) {
            let list = pieces(c, true);
            assert!(!list.is_empty(), "{c:X} has pieces");
            for p in &list {
                for (x, y) in points(p) {
                    assert!((0.0..=1.0).contains(&x) && (0.0..=1.0).contains(&y), "{c:X}: ({x}, {y}) is outside the box");
                }
            }
        }
        // Without the extended table only the core is drawn.
        assert!(pieces(0xE0C0, false).is_empty() && !pieces(0xE0B0, false).is_empty());
        // Nothing here is read from a file: the code of this module holds no asset path and no include.
        let source = include_str!("powerline.rs");
        let code = source.split("#[cfg(test)]").next().unwrap();
        for needle in [concat!("include", "_str!"), concat!("include", "_bytes!"), concat!("resources", "/powerline"), concat!(".", "pdf"), concat!(".", "svg"), concat!(".", "eps")] {
            assert!(!code.contains(needle), "the module reads or names an asset: {needle}");
        }
    }

    fn points(p: &Piece) -> Vec<(f32, f32)> {
        match p {
            Piece::Line { from, to, .. } => vec![*from, *to],
            Piece::Curve { from, c1, c2, to, .. } => vec![*from, *c1, *c2, *to],
            Piece::Solid(pts) => pts.clone(),
        }
    }

    fn col(mask: &[u8], w: usize, x: usize, h: usize) -> Vec<u8> {
        (0..h).map(|y| mask[y * w + x]).collect()
    }

    #[test]
    fn powerline_masks_follow_iterm2() {
        // The solid right arrowhead: the left column is full (the bleed), the tip is at the middle.
        let m = boxdraw::rasterize('\u{E0B0}', 10, 20, 1.0);
        assert!(col(&m, 10, 0, 20).iter().all(|a| *a == 255), "column 0 is full height");
        let tip = col(&m, 10, 9, 20);
        let set: Vec<usize> = (0..20).filter(|y| tip[*y] > 0).collect();
        assert!(!set.is_empty() && set.iter().all(|y| (8..=11).contains(y)), "the tip is near row 10: {set:?}");
        // A wide symbol covers both halves of its two cells.
        let w = boxdraw::rasterize('\u{E0B8}', 20, 20, 1.0);
        assert!(col(&w, 20, 0, 20).iter().all(|a| *a == 255) && w[19 * 20 + 19] > 100, "the triangle spans 20 px");
        assert!(w[19 * 20 + 10] > 200 && w[1 * 20 + 3] == 0, "lower left triangle");
        // A mirrored symbol is the unmirrored mask flipped left to right.
        let (a, b) = (boxdraw::rasterize('\u{E0C0}', 20, 20, 1.0), boxdraw::rasterize('\u{E0C2}', 20, 20, 1.0));
        assert!(a.iter().any(|v| *v > 0));
        for y in 0..20 {
            for x in 0..20 {
                let (p, q) = (i32::from(a[y * 20 + x]), i32::from(b[y * 20 + (19 - x)]));
                assert!((p - q).abs() <= 2, "({x}, {y}): {p} against {q}");
            }
        }
        // A half-width symbol keeps to its half of the box, the mirrored one to the other half.
        let (l, r) = (boxdraw::rasterize('\u{E0D2}', 20, 20, 1.0), boxdraw::rasterize('\u{E0D4}', 20, 20, 1.0));
        assert!((0..20).all(|y| (10..20).all(|x| l[y * 20 + x] == 0)) && l.iter().any(|v| *v > 0));
        assert!((0..20).all(|y| (0..10).all(|x| r[y * 20 + x] == 0)) && r.iter().any(|v| *v > 0));
        // Every drawn symbol leaves ink in a cell and in a double cell, and is total.
        for c in CORE.iter().flat_map(|(a, b)| *a..=*b).chain((0xE0A0..=0xE0D8u32).filter(|c| extended(*c, true).is_some())) {
            let ch = char::from_u32(c).unwrap();
            assert!(boxdraw::rasterize(ch, 10, 20, 1.0).iter().any(|v| *v > 0), "{c:X} draws");
            assert!(boxdraw::rasterize(ch, 20, 20, 2.0).iter().any(|v| *v > 0), "{c:X} draws at 2x");
            assert_eq!(boxdraw::rasterize(ch, 0, 20, 1.0).len(), 0);
            assert_eq!(boxdraw::rasterize(ch, 5000, 20, 1.0).len(), 5000 * 20);
        }
    }
}
