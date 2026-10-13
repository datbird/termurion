//! Box and block drawing (iTerm2's `iTermBoxDrawingBezierCurveFactory`): which characters Termurion
//! draws itself, their path descriptions in cell units, and the alpha masks they rasterize to.

/// True for iTerm2's `blockDrawingCharacters`: block elements, shades, quadrants, the corner
/// triangles, the Powerline separators and the sextants. Minimum contrast leaves them alone
/// because they sit beside empty cells whose background should match.
pub fn is_block(c: char) -> bool {
    matches!(u32::from(c),
        0x2580..=0x259F
        | 0x25E2..=0x25E5
        | 0x25F8..=0x25FA
        | 0x25FF
        // Powerline (https://github.com/ryanoasis/powerline-extra-symbols).
        | 0xE0B0 | 0xE0B2 | 0xE0B4 | 0xE0B6 | 0xE0B8 | 0xE0BA | 0xE0BC | 0xE0BE
        | 0xE0C0 | 0xE0C2 | 0xE0C8 | 0xE0CA | 0xE0D1 | 0xE0D2 | 0xE0D4 | 0xE0D6 | 0xE0D7
        // The 60 block sextants.
        | 0x1FB00..=0x1FB3B)
}

/// The weight of a line piece.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Weight {
    Light,
    Heavy,
    Double,
}

/// One part of a drawn character, in cell units (0 to 1 across and down).
#[derive(Clone, Debug, PartialEq)]
pub enum Piece {
    Line { from: (f32, f32), to: (f32, f32), weight: Weight },
    Curve { from: (f32, f32), c1: (f32, f32), c2: (f32, f32), to: (f32, f32), weight: Weight },
    Solid(Vec<(f32, f32)>),
}

/// iTerm2's path descriptions, by code point (`shapeBuilderForBoxDrawingCode:`): the components
/// string (four letters per stroke, `x1 y1 x2 y2`, a curve adds `cx1 cy1 cx2 cy2`; the letters
/// index iTerm2's `xs` and `ys` tables) and whether the path is filled instead of stroked.
const COMPONENTS: &[(u32, &str, bool)] = &[
    (0x2500, "a4g4", false),
    (0x2501, "a3g3 a5g5", false),
    (0x2502, "d1d7", false),
    (0x2503, "c1c7 e1e7", false),
    (0x250C, "g4d4 d4d7", false),
    (0x250D, "g3d3 d3d7 g5d5", false),
    (0x250E, "g4c4 c4c7 e4e7", false),
    (0x250F, "g3c3 c3c7 g5e5 e5e7", false),
    (0x2510, "a4d4 d4d7", false),
    (0x2511, "a3d3 d3d7 a5d5", false),
    (0x2512, "a4e4 e4e7 c4c7", false),
    (0x2513, "a3e3 e3e7 a5c5 c5c7", false),
    (0x2514, "d1d4 d4g4", false),
    (0x2515, "d1d5 d5g5 d3g3", false),
    (0x2516, "c1c4 c4g4 e1e4", false),
    (0x2517, "c1c5 c5g5 e1e3 e3g3", false),
    (0x2518, "a4d4 d4d1", false),
    (0x2519, "a5d5 d5d1 a3d3", false),
    (0x251A, "a4e4 e4e1 c4c1", false),
    (0x251B, "a5e5 e5e1 a3c3 c3c1", false),
    (0x251C, "d1d7 d4g4", false),
    (0x251D, "d1d7 d3g3 d5g5", false),
    (0x251E, "c1c4 e1e4 e4g4 d4d7", false),
    (0x251F, "d1d4 d4g4 c4c7 e4e7", false),
    (0x2520, "c1c7 e1e7 e4g4", false),
    (0x2521, "c1c4 c4g4 e1e3 e3g3 d4d7", false),
    (0x2522, "d1d4 c7c3 c3g3 e7e5 e5g5", false),
    (0x2523, "c1c7 e1e3 e3g3 g5e5 e5e7", false),
    (0x2524, "d1d7 a4d4", false),
    (0x2525, "d1d7 a3d3 a5d5", false),
    (0x2526, "c1c4 e1e4 a4d4 d4d7", false),
    (0x2527, "d1d4 d4a4 c4c7 e4e7", false),
    (0x2528, "a4c4 c1c7 e1e7", false),
    (0x2529, "c1c3 c3a3 e1e5 e5a5 d4d7", false),
    (0x252A, "a3d3 d3d7 a5c5 c5c7 d1d4", false),
    (0x252B, "a3c3 c3c1 a5c5 c5c7 e1e7", false),
    (0x252C, "a4g4 d4d7", false),
    (0x252D, "a3d3 a5d5 d7d4 d4g4", false),
    (0x252E, "a4d4 d4d7 d3g3 d5g5", false),
    (0x252F, "a3g3 a5g5 d5d7", false),
    (0x2530, "a4g4 c4c7 e4e7", false),
    (0x2531, "a3e3 e3e7 a5c5 c5c7 d4g4", false),
    (0x2532, "a4d4 c7c3 c3g3 e7e5 e5g5", false),
    (0x2533, "a3g3 a5c5 c5c7 e7e5 e5g5", false),
    (0x2534, "a4g4 d1d4", false),
    (0x2535, "d1d4 d4g4 a3d3 a5d5", false),
    (0x2536, "a4d4 d4d1 d3g3 d5g5", false),
    (0x2537, "a3g3 a5g5 d1d4", false),
    (0x2538, "a4g4 c1c4 e1e4", false),
    (0x2539, "a3c3 c3c1 a5e5 e5e1 d4g4", false),
    (0x253A, "a4d4 c1c5 c5g5 d1d3 d3g3", false),
    (0x253B, "a5g5 a3c3 c3c1 e1e3 e3g3", false),
    (0x253C, "a4g4 d1d7", false),
    (0x253D, "d1d7 d4g4 a3d3 a5d5", false),
    (0x253E, "d1d7 a4d4 d3g3 d5g5", false),
    (0x253F, "d1d7 a3g3 a5g5", false),
    (0x2540, "a4g4 d4d7 c1c4 e1e4", false),
    (0x2541, "a4g4 d1d4 c4c7 e4e7", false),
    (0x2542, "a4g4 c1c7 e1e7", false),
    (0x2543, "a3c3 c3c1 a5e5 e5e1 d7d4 d4g4", false),
    (0x2544, "a4d4 d4d7 c1c5 c5g5 e1e3 e3g3", false),
    (0x2545, "d1d4 d4g4 a3e3 e3e7 a5c5 c5c7", false),
    (0x2546, "a4d4 d4d1 c7c3 c3g3 e7e5 e5g5", false),
    (0x2547, "a5g5 a3c3 c3c1 e1e3 e3g3 d4d7", false),
    (0x2548, "d1d4 a3g3 a5c5 c5c7 e7e5 e5g5", false),
    (0x2549, "a3c3 c3c1 a5c5 c5c7 e1e7 d4g4", false),
    (0x254A, "a4c4 c1c7 e1e3 e3g3 e7e5 e5g5", false),
    (0x254B, "a3g3 a5g5 c1c7 e1e7", false),
    (0x2550, "a2g2 a6g6", false),
    (0x2551, "b1b7 f1f7", false),
    (0x2552, "g2d2 d2d7 g6d6", false),
    (0x2553, "g4b4 b4b7 f4f7", false),
    (0x2554, "g2b2 b2b7 g6f6 f6f7", false),
    (0x2555, "a2d2 d2d7 a6d6", false),
    (0x2556, "a4f4 f4f7 b4b7", false),
    (0x2557, "a2f2 f2f7 a6b6 b6b7", false),
    (0x2558, "d1d6 d6g6 d2g2", false),
    (0x2559, "b1b4 b4g4 f1f4", false),
    (0x255A, "b1b6 b6g6 f1f2 f2g2", false),
    (0x255B, "a2d2 a6d6 d6d1", false),
    (0x255C, "a4f4 f4f1 b4b1", false),
    (0x255D, "a2b2 b2b1 a6f6 f6f1", false),
    (0x255E, "d1d7 d2g2 d6g6", false),
    (0x255F, "b1b7 f1f7 f4g4", false),
    (0x2560, "b1b7 f1f2 f2g2 f7f6 f6g6", false),
    (0x2561, "d1d7 a2d2 a6d6", false),
    (0x2562, "a4b4 b1b7 f1f7", false),
    (0x2563, "a2b2 b2b1 a6b6 b6b7 f1f7", false),
    (0x2564, "a2g2 a6g6 d6d7", false),
    (0x2565, "a4g4 b4b7 f4f7", false),
    (0x2566, "a2g2 a6b6 b6b7 f7f6 f6g6", false),
    (0x2567, "a6g6 a2g2 d1d2", false),
    (0x2568, "a4g4 b1b4 f1f4", false),
    (0x2569, "a2b2 b2b1 f1f2 f2g2 a6g6", false),
    (0x256A, "a2g2 a6g6 d1d7", false),
    (0x256B, "b1b7 f1f7 a4g4", false),
    (0x256C, "a2b2 b2b1 f1f2 f2g2 g6f6 f6f7 b7b6 b6a6", false),
    (0x256D, "g4d7d4d4", false),
    (0x256E, "a4d7d4d4", false),
    (0x256F, "a4d1d4d4", false),
    (0x2570, "d1g4d4d4", false),
    (0x2571, "a7g1", false),
    (0x2572, "a1g7", false),
    (0x2573, "a7g1 a1g7", false),
    (0x2574, "a4d4", false),
    (0x2575, "d1d4", false),
    (0x2576, "d4g4", false),
    (0x2577, "d4d7", false),
    (0x2578, "a3d3 a5d5", false),
    (0x2579, "c1c4 e1e4", false),
    (0x257A, "d3g3 d5g5", false),
    (0x257B, "c4c7 e4e7", false),
    (0x257C, "a4d4 d3g3 d5g5", false),
    (0x257D, "d1d4 c4c7 e4e7", false),
    (0x257E, "a3d3 a5d5 d4g4", false),
    (0x257F, "c1c4 e1e4 d4d7", false),
    (0x25E2, "a:k1 k1k: k:a:", true),
    (0x25E3, "a1a: a:k: k:a1", true),
    (0x25E4, "a1k1 k1a: a:a1", true),
    (0x25E5, "a1k1 k1k: k:a1", true),
    (0x25F8, "a1f1 f1a4 a4a1", false),
    (0x25F9, "a1g1 g1g4 g4a1", false),
    (0x25FA, "a7a4 a4f7 f7a7", false),
    (0x25FF, "a7g4 g4g7 g7a7", false),
    (0x1FB3C, "a9a7 a7d7 d7a9", true),
    (0x1FB3D, "a9a7 a7g7 g7a9", true),
    (0x1FB3E, "a8a7 a7d7 d7a8", true),
    (0x1FB3F, "a8a7 a7g7 g7a8", true),
    (0x1FB40, "a1a7 a7d7 d7a1", true),
    (0x1FB41, "g7l7 l7l8 l8j1 j1g1 g1g7", true),
    (0x1FB42, "g7l7 l7l8 l8g1 g1g7", true),
    (0x1FB43, "g7l7 l7l9 l9j1 j1g1 g1g7", true),
    (0x1FB44, "g7l7 l7l9 l9g1 g1g7", true),
    (0x1FB45, "g7l7 l7l9 l9j1 j1g1 g1g7", true),
    (0x1FB46, "g7l7 l7l9 l9g8 g8g7", true),
    (0x1FB47, "d7g7 g7g9 g9d7", true),
    (0x1FB48, "a7g7 g7g9 g9a7", true),
    (0x1FB49, "d7g7 g7g8 g8d7", true),
    (0x1FB4A, "a7g7 g7g8 g8a7", true),
    (0x1FB4B, "d7g7 g7g1 g1d7", true),
    (0x1FB4C, "l7l1 l1j1 j1g8 g8g7 g7l7", true),
    (0x1FB4D, "l7l1 l1g8 g8g7 g7l7", true),
    (0x1FB4E, "l7l1 l1j1 j1g9 g9g7 g7l7", true),
    (0x1FB4F, "l7l1 l1g9 g9g7 g7l7", true),
    (0x1FB50, "l7l1 l1j1 j1g7 g7l7", true),
    (0x1FB51, "l7l8 l8g9 g9g7 g7l7", true),
    (0x1FB52, "g1g7 g7j7 j7l9 l9l1 l1g1", true),
    (0x1FB53, "g1g7 g7l9 l9l1 l1g1", true),
    (0x1FB54, "g1g7 g7j7 j7l8 l8l1 l1g1", true),
    (0x1FB55, "g1g7 g7l8 l8l1 l1g1", true),
    (0x1FB56, "g1g7 g7j7 j7l1 l1g1", true),
    (0x1FB57, "a8a1 a1d1 d1a8", true),
    (0x1FB58, "a8a1 a1g1 g1a8", true),
    (0x1FB59, "a9a1 a1d1 d1a9", true),
    (0x1FB5A, "a9a1 a1g1 g1a9", true),
    (0x1FB5B, "a7a1 a1d1 d1a7", true),
    (0x1FB5C, "l1g1 g1g8 g8l9 l9l1", true),
    (0x1FB5D, "l1g1 g1g9 g9j7 j7l7 l7l1", true),
    (0x1FB5E, "l1g1 g1g9 g9l7 l7l1", true),
    (0x1FB5F, "l1g1 g1g8 g8j7 j7l7 l7l1", true),
    (0x1FB60, "l1g1 g1g8 g8l7 l7l1", true),
    (0x1FB61, "l1g1 g1j7 j7l7 l7l1", true),
    (0x1FB62, "d1g1 g1g8 g8d1", true),
    (0x1FB63, "a1g1 g1g8 g8a1", true),
    (0x1FB64, "d1g1 g1g9 g9d1", true),
    (0x1FB65, "a1g1 g1g9 g9a1", true),
    (0x1FB66, "d1g1 g1g7 g7d1", true),
    (0x1FB67, "g1g9 g9l8 l8l1 l1g1", true),
    (0x1FB68, "l1g1 g1g7 g7l7 l7j4 j4l1", true),
    (0x1FB69, "l1j4 j4g1 g1g7 g7l7 l7l1", true),
    (0x1FB6A, "l1g1 g1j4 j4g7 g7l7 l7l1", true),
    (0x1FB6B, "l1g1 g1g7 g7j4 j4l7 l7l1", true),
    (0x1FB6C, "l1l7 l7j4 j4l1", true),
    (0x1FB6D, "l1m1 m1j4 j4l1", true),
    (0x1FB6E, "g1g7 g7j4 j4g1", true),
    (0x1FB6F, "l7g7 g7j4 j4l7", true),
];

/// The solid boxes of the block elements (`shapeBuilderForSolidBoxesForCode:`): four characters
/// each, the origin letter and digit and the width and height, in eighths (thirds for a letter).
const BLOCKS: &[(u32, &[&str])] = &[
    (0x2580, &["a084"]),
    (0x2581, &["a781"]),
    (0x2582, &["a682"]),
    (0x2583, &["a583"]),
    (0x2584, &["a484"]),
    (0x2585, &["a385"]),
    (0x2586, &["a286"]),
    (0x2587, &["a187"]),
    (0x2588, &["a088"]),
    (0x2589, &["a078"]),
    (0x258A, &["a068"]),
    (0x258B, &["a058"]),
    (0x258C, &["a048"]),
    (0x258D, &["a038"]),
    (0x258E, &["a028"]),
    (0x258F, &["a018"]),
    (0x2590, &["e048"]),
    (0x2594, &["a081"]),
    (0x2595, &["h018"]),
    (0x2596, &["a444"]),
    (0x2597, &["e444"]),
    (0x2598, &["a044"]),
    (0x2599, &["a044", "a444", "e444"]),
    (0x259A, &["a044", "e444"]),
    (0x259B, &["a044", "e044", "a444"]),
    (0x259C, &["a044", "e044", "e444"]),
    (0x259D, &["e044"]),
    (0x259E, &["e044", "a444"]),
    (0x259F, &["e044", "a444", "e444"]),
];

/// What a drawn character is: solid boxes, or a path.
enum Shape {
    Boxes(Vec<String>),
    Path(&'static str, bool),
}

/// The sextant at `code` (U+1FB00 to U+1FB3B): the digits of its cells (1 2 / 3 4 / 5 6). Unicode
/// leaves out the left and right halves (patterns 21 and 42), which have their own characters.
fn sextant_cells(code: u32) -> String {
    let mut mask = code - 0x1FB00 + 1;
    if mask >= 21 {
        mask += 1;
    }
    if mask >= 42 {
        mask += 1;
    }
    (0..6).filter(|bit| mask & (1 << bit) != 0).map(|bit| char::from(b'1' + bit as u8)).collect()
}

/// iTerm2's `solidBoxesForSextant:`: each cell is a half-width, third-height box.
fn sextant_boxes(digits: &str) -> Vec<String> {
    digits
        .chars()
        .map(|d| {
            let (left, top) = match d {
                '1' => ('a', '0'),
                '2' => ('e', '0'),
                '3' => ('a', 'B'),
                '4' => ('e', 'B'),
                '5' => ('a', 'C'),
                _ => ('e', 'C'),
            };
            format!("{left}{top}4B")
        })
        .collect()
}

fn shape(c: char) -> Option<Shape> {
    let code = u32::from(c);
    match code {
        0x1FB00..=0x1FB3B => return Some(Shape::Boxes(sextant_boxes(&sextant_cells(code)))),
        _ => {}
    }
    if let Ok(i) = BLOCKS.binary_search_by_key(&code, |b| b.0) {
        return Some(Shape::Boxes(BLOCKS[i].1.iter().map(|s| s.to_string()).collect()));
    }
    let i = COMPONENTS.binary_search_by_key(&code, |b| b.0).ok()?;
    Some(Shape::Path(COMPONENTS[i].1, COMPONENTS[i].2))
}

/// Whether Termurion draws `c` itself: never with `disabled` (`DisableCustomBoxDrawing`). The
/// Powerline set joins it with `powerline` (the profile's Draw Powerline Glyphs), with its
/// extended symbols too (`SupportPowerlineExtendedSymbols`, on here).
pub fn drawn(c: char, powerline: bool, disabled: bool) -> bool {
    drawn_with(c, powerline, true, disabled)
}

/// `drawn` with the extended Powerline symbols chosen by `extended`.
pub fn drawn_with(c: char, powerline: bool, extended: bool, disabled: bool) -> bool {
    !disabled && (shape(c).is_some() || (powerline && crate::powerline::drawn(u32::from(c), extended)))
}

/// One stroke of a components string: the letters of its ends and, for a curve, of its controls.
struct Seg {
    from: (u8, u8),
    to: (u8, u8),
    controls: Option<((u8, u8), (u8, u8))>,
}

/// iTerm2's reading of a components string (`shapeBuilderForComponents:`): four bytes a stroke,
/// four more when they are letters (a curve's controls), one separator.
fn parse(components: &str) -> Vec<Seg> {
    let b = components.as_bytes();
    let mut segs = Vec::new();
    let mut i = 0;
    while i + 4 <= b.len() {
        let (from, to) = ((b[i], b[i + 1]), (b[i + 2], b[i + 3]));
        i += 4;
        let mut controls = None;
        if i + 4 <= b.len() && b[i].is_ascii_alphabetic() {
            controls = Some(((b[i], b[i + 1]), (b[i + 2], b[i + 3])));
            i += 4;
        }
        i += 1;
        segs.push(Seg { from, to, controls });
    }
    segs
}

/// A letter of the x table in cell units: the center letters (b to f, j) are the middle.
fn unit_x(letter: u8) -> f32 {
    match letter {
        b'a' | b'l' | b'n' => 0.0,
        b'b'..=b'f' | b'j' => 0.5,
        b'h' => 1.0 / 3.0,
        b'i' => 2.0 / 3.0,
        _ => 1.0,
    }
}

/// A character of the y table in cell units.
fn unit_y(letter: u8) -> f32 {
    match letter {
        b'1' | b'/' => 0.0,
        b'2'..=b'6' => 0.5,
        b'8' => 1.0 / 3.0,
        b'9' => 2.0 / 3.0,
        _ => 1.0,
    }
}

fn unit_point(p: (u8, u8)) -> (f32, f32) {
    (unit_x(p.0), unit_y(p.1))
}

/// A box of a block's description: its edges before rounding, for a cell of `w` by `h`.
fn box_edges(part: &str, w: f32, h: f32) -> (f32, f32, f32, f32) {
    let b = part.as_bytes();
    let xo = if b[0] <= b'Z' { w * f32::from(b[0] - b'A') / 3.0 } else { w * f32::from(b[0] - b'a') / 8.0 };
    let yo = if b[1] >= b'A' { h * f32::from(b[1] - b'A') / 3.0 } else { h * f32::from(b[1] - b'0') / 8.0 };
    let bw = if b[2] >= b'A' { w / 3.0 * f32::from(b[2] - b'A') } else { w / 8.0 * f32::from(b[2] - b'0') };
    let bh = if b[3] >= b'A' { h / 3.0 * f32::from(b[3] - b'A') } else { h / 8.0 * f32::from(b[3] - b'0') };
    (xo, yo, xo + bw, yo + bh)
}

/// The weight a straight stroke has by the line it lies on: iTerm2 draws a heavy line as two
/// strokes a pixel apart (`3` and `5`, `c` and `e`) and a double line as two with a gap (`2` and
/// `6`, `b` and `f`).
fn weight_of(seg: &Seg) -> Weight {
    let perpendicular = if seg.from.1 == seg.to.1 {
        seg.from.1
    } else if seg.from.0 == seg.to.0 {
        seg.from.0
    } else {
        return Weight::Light;
    };
    match perpendicular {
        b'3' | b'5' | b'c' | b'e' => Weight::Heavy,
        b'2' | b'6' | b'b' | b'f' => Weight::Double,
        _ => Weight::Light,
    }
}

/// What `c` is made of, in cell units (empty when Termurion does not draw it). This is a summary of
/// iTerm2's descriptions, not what is rasterized: the two strokes of a heavy or double line
/// merge into one piece, and a stroke drawn twice (iTerm2 repeats the shared arm of a corner)
/// appears once, at its heaviest.
pub fn pieces(c: char) -> Vec<Piece> {
    match shape(c) {
        None => Vec::new(),
        Some(Shape::Boxes(parts)) => parts
            .iter()
            .map(|p| {
                let (x0, y0, x1, y1) = box_edges(p, 1.0, 1.0);
                Piece::Solid(vec![(x0, y0), (x1, y0), (x1, y1), (x0, y1)])
            })
            .collect(),
        Some(Shape::Path(components, solid)) => path_pieces(components, solid),
    }
}

/// The pieces of one components string (`solid` fills it): the summary `pieces` gives a path.
pub(crate) fn path_pieces(components: &str, solid: bool) -> Vec<Piece> {
    if solid {
        let mut polygons: Vec<Vec<(f32, f32)>> = Vec::new();
        let mut current: Vec<(f32, f32)> = Vec::new();
        for seg in parse(components) {
            let (a, b) = (unit_point(seg.from), unit_point(seg.to));
            if current.last() != Some(&a) {
                if !current.is_empty() {
                    polygons.push(std::mem::take(&mut current));
                }
                current.push(a);
            }
            current.push(b);
        }
        polygons.push(current);
        polygons
            .into_iter()
            .map(|mut p| {
                if p.len() > 2 && p.first() == p.last() {
                    p.pop();
                }
                Piece::Solid(p)
            })
            .collect()
    } else {
        let mut out: Vec<Piece> = Vec::new();
        for seg in parse(components) {
            let (a, b) = (unit_point(seg.from), unit_point(seg.to));
            if let Some((c1, c2)) = seg.controls {
                // Written from the end the arc starts at, vertical first.
                out.push(Piece::Curve { from: b, c1: unit_point(c2), c2: unit_point(c1), to: a, weight: Weight::Light });
                continue;
            }
            let weight = weight_of(&seg);
            let same = |p: &Piece| matches!(p, Piece::Line { from, to, .. } if (*from, *to) == (a, b) || (*from, *to) == (b, a));
            match out.iter().position(same) {
                Some(i) => {
                    if let Piece::Line { weight: w, .. } = &mut out[i] {
                        *w = w.max_by_rank(weight);
                    }
                }
                None => out.push(Piece::Line { from: a, to: b, weight }),
            }
        }
        out
    }
}

impl Weight {
    fn rank(self) -> u8 {
        match self {
            Weight::Light => 0,
            Weight::Heavy => 1,
            Weight::Double => 2,
        }
    }

    fn max_by_rank(self, other: Weight) -> Weight {
        if other.rank() > self.rank() { other } else { self }
    }
}

/// The largest cell (in pixels, either way) Termurion rasterizes; a bigger request is an empty mask.
const MAX_CELL: u16 = 1024;

/// The alpha mask (`w * h`, row by row) of `c` in a cell of `w` by `h` physical pixels at device
/// `scale` (a stroke is `scale` rounded pixels thick, at least one). Total: a character Termurion
/// does not draw, an empty cell and an absurd size give an all-zero mask of the right length.
pub fn rasterize(c: char, w: u16, h: u16, scale: f32) -> Vec<u8> {
    let mut mask = vec![0u8; usize::from(w) * usize::from(h)];
    if w == 0 || h == 0 || w > MAX_CELL || h > MAX_CELL {
        return mask;
    }
    let scale = if scale.is_finite() && scale >= 1.0 { scale.round().min(8.0) } else { 1.0 };
    match shape(c) {
        None => {
            crate::powerline::draw(c, &mut mask, w, h, scale);
        }
        Some(Shape::Boxes(parts)) => {
            for part in &parts {
                // Rounded to pixel boundaries for sharp edges (`alignX` of a solid shape).
                let (x0, y0, x1, y1) = box_edges(part, f32::from(w), f32::from(h));
                fill_rect(&mut mask, w, h, x0.round(), y0.round(), x1.round(), y1.round());
            }
        }
        Some(Shape::Path(components, solid)) => draw_path(&mut mask, w, h, components, solid, scale),
    }
    mask
}

fn fill_rect(mask: &mut [u8], w: u16, h: u16, x0: f32, y0: f32, x1: f32, y1: f32) {
    let (x0, x1) = (x0.clamp(0.0, f32::from(w)) as usize, x1.clamp(0.0, f32::from(w)) as usize);
    let (y0, y1) = (y0.clamp(0.0, f32::from(h)) as usize, y1.clamp(0.0, f32::from(h)) as usize);
    for y in y0..y1 {
        mask[y * usize::from(w) + x0..y * usize::from(w) + x1.max(x0)].fill(255);
    }
}

/// iTerm2's `shapeBuilderForComponents:` and `drawShape:`: the letters resolve against the cell
/// (center letters a pixel or half a pixel either side of the middle, by the scale), points of a
/// stroked path move onto half pixels so a one pixel line is crisp, and a stroke is `scale`
/// wide with square caps, clipped to the cell.
pub(crate) fn draw_path(mask: &mut [u8], w: u16, h: u16, components: &str, solid: bool, scale: f32) {
    use zeno::{Cap, Command, Join, Mask, Point, Stroke};
    let (cw, ch) = (f32::from(w), f32::from(h));
    let (hc, vc) = (cw / 2.0, ch / 2.0);
    // Low DPI: a heavy center line is two pixels, off center; high DPI: centered.
    let (full, half, up) = if scale >= 2.0 { (scale, scale / 2.0, scale / 2.0) } else { (1.0, 0.0, 1.0) };
    let xs = [0.0, hc - full, hc - half, hc, hc + up, hc + full, cw, cw / 3.0 - full, 2.0 * cw / 3.0 - full, cw / 2.0 - full, cw, (1.0 / scale) / 2.0 - full, cw - full, 0.0];
    let ys = [-scale / 4.0, ch - full, 0.0, vc - full, vc - half, vc, vc + up, vc + full, ch, ch / 3.0, 2.0 * ch / 3.0, ch];
    let half_stroke = scale / 2.0;
    let align = |v: f32, edge: f32| -> f32 {
        if solid {
            return v.round();
        }
        if v <= 0.0 {
            return 0.0;
        }
        if v >= edge {
            return edge;
        }
        if scale < 2.0 && v - v.floor() < 0.01 {
            return v + half_stroke;
        }
        v
    };
    let point = |p: (u8, u8)| -> Option<Point> {
        let x = *xs.get(usize::from(p.0.checked_sub(b'a')?))?;
        let y = *ys.get(usize::from(p.1.checked_sub(b'/')?))?;
        Some(Point::new(align(x, cw), align(y, ch)))
    };
    let mut commands: Vec<Command> = Vec::new();
    let mut last: Option<(u8, u8)> = None;
    for seg in parse(components) {
        let (Some(a), Some(b)) = (point(seg.from), point(seg.to)) else { return };
        if last != Some(seg.from) {
            if solid && !commands.is_empty() {
                commands.push(Command::Close);
            }
            commands.push(Command::MoveTo(a));
        }
        match seg.controls {
            Some((c1, c2)) => {
                let (Some(c1), Some(c2)) = (point(c1), point(c2)) else { return };
                commands.push(Command::CurveTo(c1, c2, b));
            }
            None => commands.push(Command::LineTo(b)),
        }
        last = Some(seg.to);
    }
    if solid && !commands.is_empty() {
        commands.push(Command::Close);
    }
    let mut render = Mask::new(&commands[..]);
    render.size(u32::from(w), u32::from(h));
    if solid {
        render.render_into(mask, None);
    } else {
        let mut stroke = Stroke::new(scale);
        stroke.cap(Cap::Square).join(Join::Miter);
        render.style(&stroke).render_into(mask, None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_set_follows_iterm2() {
        for c in ['\u{2580}', '\u{2588}', '\u{258F}', '\u{2590}', '\u{2591}', '\u{2593}', '\u{2594}', '\u{2595}', '\u{2596}', '\u{259F}', '\u{25E2}', '\u{25E5}', '\u{25F8}', '\u{25FA}', '\u{25FF}', '\u{E0B0}', '\u{E0B2}', '\u{E0D7}', '\u{1FB00}', '\u{1FB3B}'] {
            assert!(is_block(c), "{c:?}");
        }
        // Box lines, the powerline glyphs outside the list, the gap in the triangles, and the
        // sextant neighbors are not blocks.
        for c in ['\u{2500}', '\u{2502}', '\u{257F}', '\u{25E1}', '\u{25E6}', '\u{25FB}', '\u{25FE}', '\u{E0B1}', '\u{E0B3}', '\u{E0D8}', '\u{1FAFF}', '\u{1FB3C}', 'a', ' '] {
            assert!(!is_block(c), "{c:?}");
        }
    }

    fn at(m: &[u8], w: usize, x: usize, y: usize) -> u8 {
        m[y * w + x]
    }

    #[test]
    fn box_paths_follow_iterm2() {
        assert!(drawn('\u{2500}', false, false));
        assert!(!drawn('\u{2500}', false, true), "DisableCustomBoxDrawing draws nothing");
        assert!(!drawn('a', false, false));
        assert!(drawn('\u{1FB00}', false, false), "sextants");
        assert!(drawn('\u{1FB6F}', false, false), "wedges");
        assert!(!drawn('\u{E0B0}', false, false), "Powerline only when the profile asks (Task 7 fills it)");
        // iTerm2 leaves the dashed lines to the font.
        for c in ['\u{2504}', '\u{2508}', '\u{254C}', '\u{254F}'] {
            assert!(!drawn(c, false, false), "{c:?}");
        }
        assert_eq!(pieces('\u{2500}'), vec![Piece::Line { from: (0.0, 0.5), to: (1.0, 0.5), weight: Weight::Light }]);
        assert_eq!(pieces('\u{2503}'), vec![Piece::Line { from: (0.5, 0.0), to: (0.5, 1.0), weight: Weight::Heavy }]);
        assert_eq!(pieces('\u{2550}'), vec![Piece::Line { from: (0.0, 0.5), to: (1.0, 0.5), weight: Weight::Double }]);
        assert_eq!(pieces('\u{2588}'), vec![Piece::Solid(vec![(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)])]);
        // iTerm2 leaves the shades to the font (they are in `blockDrawingCharacters`, the minimum
        // contrast exemption, but not in the drawn set), so Termurion draws nothing for them.
        for c in ['\u{2591}', '\u{2592}', '\u{2593}'] {
            assert!(!drawn(c, false, false) && !drawn(c, true, false) && pieces(c).is_empty(), "{c:?}");
            assert!(rasterize(c, 8, 16, 1.0).iter().all(|&a| a == 0), "{c:?}");
            assert!(is_block(c), "the contrast exemption stays: {c:?}");
        }
        assert_eq!(pieces('\u{256D}'), vec![Piece::Curve { from: (0.5, 1.0), c1: (0.5, 0.5), c2: (0.5, 0.5), to: (1.0, 0.5), weight: Weight::Light }]);
        assert_eq!(pieces('\u{2571}'), vec![Piece::Line { from: (0.0, 1.0), to: (1.0, 0.0), weight: Weight::Light }]);
        // A heavy corner is one heavy path (its two strokes merge); a double corner two double lines.
        assert_eq!(
            pieces('\u{250F}'),
            vec![Piece::Line { from: (1.0, 0.5), to: (0.5, 0.5), weight: Weight::Heavy }, Piece::Line { from: (0.5, 0.5), to: (0.5, 1.0), weight: Weight::Heavy }]
        );
        assert_eq!(pieces('\u{2554}').len(), 2);
        // The quadrants and sextants are solids of the cell's eighths and thirds.
        assert_eq!(pieces('\u{2598}'), vec![Piece::Solid(vec![(0.0, 0.0), (0.5, 0.0), (0.5, 0.5), (0.0, 0.5)])]);
        let third = 1.0 / 3.0;
        assert_eq!(pieces('\u{1FB00}'), vec![Piece::Solid(vec![(0.0, 0.0), (0.5, 0.0), (0.5, third), (0.0, third)])]);
        assert_eq!(pieces('\u{1FB0B}').len(), 2, "sextants 3 and 4");
        assert_eq!(pieces('\u{25E2}'), vec![Piece::Solid(vec![(0.0, 1.0), (1.0, 0.0), (1.0, 1.0)])]);
        assert!(pieces('a').is_empty());
        assert!(is_block('\u{2580}') && !is_block('\u{2500}'));
    }

    /// The seam with 13m: every block (but Powerline and the shades, which iTerm2 leaves to the
    /// font) is a character Termurion draws.
    #[test]
    fn drawn_set_contains_every_block_except_powerline_when_off() {
        for n in 0..=0x1FB6F_u32 {
            let Some(c) = char::from_u32(n) else { continue };
            if is_block(c) && !(0xE000..=0xF8FF).contains(&n) && !(0x2591..=0x2593).contains(&n) {
                assert!(drawn(c, false, false), "{c:?}");
            }
        }
    }

    #[test]
    fn box_masks_fill_the_cell() {
        let full = rasterize('\u{2588}', 9, 19, 1.0);
        assert_eq!(full.len(), 9 * 19);
        assert!(full.iter().all(|&a| a == 255), "a full block is the whole cell");
        // A horizontal line reaches both edges, so neighbors meet.
        let h = rasterize('\u{2500}', 9, 19, 1.0);
        assert!(at(&h, 9, 0, 9) > 0 && at(&h, 9, 8, 9) > 0);
        assert_eq!((0..9).filter(|&x| at(&h, 9, x, 9) == 255).count(), 9, "row 9, one pixel thick");
        assert!(h[..9 * 9].iter().all(|&a| a == 0) && h[9 * 10..].iter().all(|&a| a == 0));
        let v = rasterize('\u{2502}', 9, 19, 1.0);
        assert!(at(&v, 9, 4, 0) > 0 && at(&v, 9, 4, 18) > 0);
        assert_eq!((0..19).filter(|&y| at(&v, 9, 4, y) == 255).count(), 19);
        // The lower half rounds to whole pixels: rows 10 to 18 for 19 rows.
        let lower = rasterize('\u{2584}', 9, 19, 1.0);
        for y in 0..19 {
            let row = &lower[y * 9..y * 9 + 9];
            assert!(row.iter().all(|&a| a == if y >= 10 { 255 } else { 0 }), "row {y}");
        }
    }

    #[test]
    fn heavy_and_double_lines_follow_iterm2_pixels() {
        // Heavy: two rows from the center row down at 1x (iTerm2 draws strokes at y3 and y5).
        let heavy = rasterize('\u{2501}', 9, 19, 1.0);
        let rows: Vec<usize> = (0..19).filter(|&y| at(&heavy, 9, 0, y) == 255).collect();
        assert_eq!(rows, vec![9, 10]);
        let heavy16 = rasterize('\u{2501}', 9, 16, 1.0);
        assert_eq!((0..16).filter(|&y| at(&heavy16, 9, 4, y) == 255).collect::<Vec<_>>(), vec![8, 9]);
        // Double: two light lines with a pixel between (rows 8 and 10 for 19 rows).
        let double = rasterize('\u{2550}', 9, 19, 1.0);
        assert_eq!((0..19).filter(|&y| at(&double, 9, 4, y) == 255).collect::<Vec<_>>(), vec![8, 10]);
        // At 2x every stroke is two pixels.
        let light2 = rasterize('\u{2500}', 18, 40, 2.0);
        assert_eq!((0..40).filter(|&y| at(&light2, 18, 9, y) == 255).collect::<Vec<_>>(), vec![19, 20]);
        // A corner is joined: the row and the column share the center pixel.
        let corner = rasterize('\u{250C}', 9, 19, 1.0);
        assert_eq!(at(&corner, 9, 4, 9), 255);
        assert_eq!(at(&corner, 9, 8, 9), 255);
        assert_eq!(at(&corner, 9, 4, 18), 255);
        assert_eq!(at(&corner, 9, 0, 9), 0);
        assert_eq!(at(&corner, 9, 4, 0), 0);
    }

    #[test]
    fn lines_meet_their_neighbors_at_every_cell_size() {
        for (w, h) in [(7_u16, 15_u16), (8, 16), (9, 19), (10, 21), (11, 23), (13, 29)] {
            let hl = rasterize('\u{2500}', w, h, 1.0);
            let rows: Vec<usize> = (0..h as usize).filter(|&y| at(&hl, w as usize, 0, y) > 0).collect();
            assert_eq!(rows.len(), 1, "{w}x{h}");
            assert!((0..w as usize).all(|x| at(&hl, w as usize, x, rows[0]) == 255), "{w}x{h}: edge to edge");
            let vl = rasterize('\u{2502}', w, h, 1.0);
            let cols: Vec<usize> = (0..w as usize).filter(|&x| at(&vl, w as usize, x, 0) > 0).collect();
            assert_eq!(cols.len(), 1, "{w}x{h}");
            assert!((0..h as usize).all(|y| at(&vl, w as usize, cols[0], y) == 255), "{w}x{h}: edge to edge");
        }
    }

    #[test]
    fn every_drawn_character_leaves_ink_and_never_panics() {
        for n in (0x2500..=0x259F_u32).chain(0x25E2..=0x25FF).chain(0x1FB00..=0x1FB6F) {
            let c = char::from_u32(n).unwrap();
            if !drawn(c, false, false) {
                continue;
            }
            for (w, h, s) in [(8_u16, 16_u16, 1.0_f32), (9, 19, 1.0), (17, 36, 2.0), (1, 1, 1.0)] {
                let m = rasterize(c, w, h, s);
                assert_eq!(m.len(), w as usize * h as usize, "{c:?}");
                if w > 1 {
                    assert!(m.iter().any(|&a| a > 0), "{c:?} {w}x{h} has no ink");
                }
            }
            assert!(!pieces(c).is_empty(), "{c:?}");
        }
    }

    #[test]
    fn rasterize_is_total() {
        assert!(rasterize('\u{2500}', 0, 10, 1.0).is_empty());
        assert!(rasterize('\u{2500}', 10, 0, 1.0).is_empty());
        assert_eq!(rasterize('\u{2500}', 4000, 3, 1.0).len(), 12_000, "an absurd size is still a valid, empty mask");
        assert_eq!(rasterize('a', 5, 5, 1.0), vec![0; 25], "a character Termurion does not draw is empty");
        assert_eq!(rasterize('\u{2500}', 9, 19, f32::NAN).len(), 9 * 19);
        assert_eq!(rasterize('\u{2500}', 9, 19, -3.0).len(), 9 * 19);
    }

    #[test]
    fn rounded_corners_and_diagonals_draw() {
        let arc = rasterize('\u{256D}', 9, 19, 1.0);
        assert!(at(&arc, 9, 8, 9) > 0 && at(&arc, 9, 4, 18) > 0, "the arc reaches the right and bottom edges");
        assert_eq!(at(&arc, 9, 0, 0), 0);
        let slash = rasterize('\u{2571}', 9, 19, 1.0);
        assert!(at(&slash, 9, 0, 18) > 0 && at(&slash, 9, 8, 0) > 0 && at(&slash, 9, 0, 0) == 0);
        let tri = rasterize('\u{25E2}', 10, 10, 1.0);
        assert_eq!(at(&tri, 10, 9, 9), 255);
        assert_eq!(at(&tri, 10, 0, 0), 0);
        assert!(at(&tri, 10, 9, 0) > 100, "the black triangle reaches the top right corner (half of its pixel)");
    }
}
