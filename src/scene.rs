//! What to draw, as plain data: rectangles and rows of text. The app builds a
//! Frame from its panes, and the renderer draws it.

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::point_to_viewport;
use alacritty_terminal::vte::ansi::CursorShape;

use crate::cursor_look::{CursorLook, box_rect, cursor_rect};
use crate::layout::{Divider, Rect};
use crate::palette::{Rgb, rgb};
use crate::screen::Screen;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Quad {
    pub rect: Rect,
    pub color: [f32; 4],
}

#[derive(Clone, Debug, PartialEq)]
pub struct Span {
    pub text: String,
    pub color: Rgb,
    pub bold: bool,
    pub italic: bool,
    /// Draw with the profile's non-ASCII font, when it has one.
    pub alt_font: bool,
    /// Draw with the icon font (Segoe Fluent Icons).
    pub icon: bool,
    /// Draw with the prose font (a porthole's markdown).
    pub prose: bool,
    /// Draw with the badge font (bold).
    pub badge: bool,
    /// Draw with the toolbelt font (`toolbeltFont`, else Consolas).
    pub toolbelt: bool,
    /// A bold span of a terminal cell: when its face has no bold weight heavy enough, the
    /// renderer double-strikes it (a second copy one pixel right, iTerm2's `fakeBold`). Chrome
    /// text never sets it, so it is never struck twice.
    pub fake_bold: bool,
    /// The ligature level the renderer shapes the span at (`fonts::features`): 0 none, 1 standard,
    /// 2 standard plus contextual alternates. `pane_with` sets it from the profile's switches;
    /// spans of different levels never merge.
    pub ligatures: u8,
    /// The mask mode the renderer draws the span's glyphs in (`fonts::MaskMode::code`): 0 smooth,
    /// 1 aliased, 2 thin strokes. `pane_with` sets it from the profile; spans of different modes
    /// never merge, and each mode is cached apart in the glyph atlas.
    pub mask: u8,
    /// The Special Font Config entry that draws the span, as 1 plus its index in the face's table
    /// (`render::Face::table`); 0 draws in the face's own fonts. `pane_with` sets it; spans of
    /// different entries never merge, and a tabled span's text is the code points to draw.
    pub font: u16,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TextRow {
    pub x: f32,
    pub y: f32,
    pub spans: Vec<Span>,
    pub clip: Rect,
    /// Text size against the terminal font (a porthole's headings).
    pub scale: f32,
    /// The renderer's face the row is shaped and sized with (`render::Renderer::face`): a pane's
    /// rows carry their profile's face, every other row the default one.
    pub face: u16,
}

/// A piece of an inline image: where it goes, which part of the picture (u0, v0, u1, v1),
/// and the picture (`key` names it for the renderer's texture cache).
#[derive(Clone, Debug, PartialEq)]
pub struct ImageQuad {
    pub rect: Rect,
    pub uv: [f32; 4],
    pub key: (u64, usize),
    pub pixels: std::sync::Arc<Vec<u8>>,
    pub size: (u32, u32),
    /// The texture repeats past uv 0..1 (a tiled background image).
    pub repeat: bool,
}

/// A character Tandem draws itself to fill its cell (`boxdraw`): a box line, a block, a sextant.
/// It draws with the text, so it is cut, covered and held exactly like a row.
#[derive(Clone, Debug, PartialEq)]
pub struct CellGlyph {
    pub c: char,
    /// The cell (or cells) the character fills.
    pub rect: Rect,
    pub color: Rgb,
    /// The renderer face of the pane the cell is in.
    pub face: u16,
    /// Where it may draw: the pane's text rectangle, narrowed where a popup covers it. It also
    /// groups a pane's glyphs into one `TextArea`.
    pub clip: Rect,
}

/// Draw order: `quads`, then `images`, then `rows`, then `underlines` and `strikes`, then `overlay`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Frame {
    pub clear: [f32; 4],
    /// Background images, drawn before everything else.
    pub backgrounds: Vec<ImageQuad>,
    pub quads: Vec<Quad>,
    pub images: Vec<ImageQuad>,
    pub rows: Vec<TextRow>,
    /// Box and block characters Tandem draws itself, drawn with the rows.
    pub glyphs: Vec<CellGlyph>,
    pub overlay: Vec<Quad>,
    /// Underline and strikethrough strokes: drawn after the text, right before `overlay`, so
    /// the dimming and the cursor extras lie over them. Trimmed under popups by `cut_quads`.
    pub underlines: Vec<Quad>,
    /// Strikethrough strokes: drawn with `underlines`, trimmed and covered like them, but never
    /// broken around glyph ink (iTerm2's mask leaves strikethrough whole).
    pub strikes: Vec<Quad>,
    /// Break `underlines` around the ink of glyphs in their row (`SolidUnderlines` off).
    pub gap_underlines: bool,
    /// Popup boxes over the terminal: drawn after images, before text.
    pub popups: Vec<Quad>,
    /// Scrollbars and their ticks: no text, drawn after the strokes and before `overlay` (the
    /// dimming covers them). `cut_quads` trims them under a popup, so popups lie over a bar.
    pub scrollbars: Vec<Quad>,
    /// The line height of each non-default face a pane drew rows in (`pane_with` records it):
    /// what `cut_out` and `cut_rows_from` measure a pane row by, since a zoomed pane's rows are
    /// taller than the chrome's line.
    pub face_heights: Vec<(u16, f32)>,
}

/// A row's height for the cuts: its own face's line (times its scale) for a pane's row, else the
/// caller's `line_h`.
fn row_height_in(heights: &[(u16, f32)], row: &TextRow, line_h: f32) -> f32 {
    let base = heights.iter().find(|(f, _)| *f == row.face).map_or(line_h, |(_, h)| *h);
    base * row.scale.max(1.0)
}

/// Which frame of an animated image shows at `now_ms`.
pub fn frame_at(frames: &[crate::images::Frame], now_ms: u64) -> usize {
    let total: u64 = frames.iter().map(|f| f.delay_ms as u64).sum();
    if total == 0 {
        return 0;
    }
    let mut t = now_ms % total;
    for (i, f) in frames.iter().enumerate() {
        if t < f.delay_ms as u64 {
            return i;
        }
        t -= f.delay_ms as u64;
    }
    0
}

/// Milliseconds until an animated image shows its next frame (None for a still image).
pub fn ms_to_next_frame(frames: &[crate::images::Frame], now_ms: u64) -> Option<u64> {
    let total: u64 = frames.iter().map(|f| f.delay_ms as u64).sum();
    if frames.len() < 2 || total == 0 {
        return None;
    }
    let mut t = now_ms % total;
    for f in frames {
        let d = f.delay_ms as u64;
        if t < d {
            return Some(d - t);
        }
        t -= d;
    }
    None
}

/// The image pieces for one cell showing `tile` of `image` at `cell_rect`.
pub fn tile_quad(image: &crate::images::Image, tile: alacritty_terminal::term::cell::Tile, cell_rect: Rect, frame: usize) -> Option<ImageQuad> {
    let (cw, ch) = (cell_rect.w, cell_rect.h);
    let (bw, bh) = (image.cols as f32 * cw, image.rows as f32 * ch);
    let (iw, ih) = (image.width_px.max(1) as f32, image.height_px.max(1) as f32);
    // The picture's place in its box of cells: all of it, or fitted and centered.
    let (px, py, pw, ph) = if image.keep_aspect {
        let s = (bw / iw).min(bh / ih);
        ((bw - iw * s) / 2.0, (bh - ih * s) / 2.0, iw * s, ih * s)
    } else {
        (0.0, 0.0, bw, bh)
    };
    let (tx, ty) = (tile.x as f32 * cw, tile.y as f32 * ch);
    let (x0, y0) = (tx.max(px), ty.max(py));
    let (x1, y1) = ((tx + cw).min(px + pw), (ty + ch).min(py + ph));
    if x1 <= x0 || y1 <= y0 || pw <= 0.0 || ph <= 0.0 {
        return None;
    }
    let f = &image.frames[frame.min(image.frames.len().checked_sub(1)?)];
    Some(ImageQuad {
        rect: Rect { x: cell_rect.x + x0 - tx, y: cell_rect.y + y0 - ty, w: x1 - x0, h: y1 - y0 },
        uv: [(x0 - px) / pw, (y0 - py) / ph, (x1 - px) / pw, (y1 - py) / ph],
        key: (image.uid, frame.min(image.frames.len() - 1)),
        pixels: f.rgba.clone(),
        size: (image.width_px, image.height_px),
        repeat: false,
    })
}

pub const TAB_BAR: Rgb = rgb(0x1e, 0x1e, 0x1e);
const TAB_ACTIVE: Rgb = rgb(0x3a, 0x3a, 0x3a);
const TAB_TEXT: Rgb = rgb(0xc7, 0xc7, 0xc7);
const TAB_TEXT_DIM: Rgb = rgb(0x8a, 0x8a, 0x8a);
const DIVIDER: Rgb = rgb(0x4a, 0x4a, 0x4a);
const BANNER: Rgb = rgb(0x7a, 0x1e, 0x1e);
const FIND_BAR: Rgb = rgb(0x2b, 0x2b, 0x2b);
const LABEL_BG: Rgb = rgb(0xff, 0xd7, 0x00);
const NOTE: Rgb = rgb(0xff, 0xff, 0x00);
const STAMP_BG: Rgb = rgb(0x20, 0x20, 0x20);
const NOTE_BG: Rgb = rgb(0xff, 0xf8, 0xc4);
const BAR_BUTTON: Rgb = rgb(0xff, 0xff, 0xff);
const LABEL_TEXT: Rgb = rgb(0x00, 0x00, 0x00);
pub const FIND_ERROR: Rgb = rgb(0xff, 0x6b, 0x6b);
/// How much an unfocused pane fades toward the background, as iTerm2 dims inactive splits.
pub(crate) const DIM_ALPHA: f32 = crate::dim::Dim::LEGACY.amount;

fn srgb_to_linear(c: u8) -> f32 {
    let c = c as f32 / 255.0;
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

/// The strips of `rect` that lie outside `grid` (its cell grid): top, bottom, then left and right
/// within the grid's rows. Zero-size strips are skipped.
pub fn margins(rect: Rect, grid: Rect) -> Vec<Rect> {
    let bottom = grid.y + grid.h;
    let right = grid.x + grid.w;
    [
        Rect { x: rect.x, y: rect.y, w: rect.w, h: grid.y - rect.y },
        Rect { x: rect.x, y: bottom, w: rect.w, h: rect.y + rect.h - bottom },
        Rect { x: rect.x, y: grid.y, w: grid.x - rect.x, h: grid.h },
        Rect { x: right, y: grid.y, w: rect.x + rect.w - right, h: grid.h },
    ]
    .into_iter()
    .filter(|r| r.w > 0.0 && r.h > 0.0)
    .collect()
}

/// The strips of `outer` outside `inner` (the cell grid), each with its color: the top and
/// bottom across the whole width in `fill.default`, then the left and right of each row in that
/// row's colors (adjacent rows of one color join). They never overlap.
fn margin_strips(outer: Rect, inner: Rect, cell_h: f32, fill: &crate::margins::Fill) -> Vec<(Rect, Rgb)> {
    let mut out = Vec::new();
    let mut put = |rect: Rect, color: Rgb| {
        if rect.w > 0.0 && rect.h > 0.0 {
            out.push((rect, color));
        }
    };
    let (bottom, right) = (inner.y + inner.h, inner.x + inner.w);
    put(Rect { x: outer.x, y: outer.y, w: outer.w, h: inner.y - outer.y }, fill.default);
    put(Rect { x: outer.x, y: bottom, w: outer.w, h: outer.y + outer.h - bottom }, fill.default);
    let rows = if cell_h > 0.0 { (inner.h / cell_h).round() as usize } else { 0 };
    let mut row = 0;
    while row < rows {
        let (l, r) = fill.rows.get(row).copied().unwrap_or((fill.default, fill.default));
        let mut end = row + 1;
        while end < rows && fill.rows.get(end).copied().unwrap_or((fill.default, fill.default)) == (l, r) {
            end += 1;
        }
        let y = inner.y + row as f32 * cell_h;
        // The last row runs to the grid's bottom edge.
        let h = if end == rows { bottom - y } else { (end - row) as f32 * cell_h };
        put(Rect { x: outer.x, y, w: inner.x - outer.x, h }, l);
        put(Rect { x: right, y, w: outer.x + outer.w - right, h }, r);
        row = end;
    }
    out
}

/// Paints the margins of a pane in an opaque window, under its text: the one painter of every
/// pane's margins (the alternate screen's colors included). A strip the window's clear color
/// already shows is not drawn.
pub fn margin_fill(frame: &mut Frame, outer: Rect, inner: Rect, cell_h: f32, fill: &crate::margins::Fill, image_bg: Option<Rgb>) {
    for (rect, color) in margin_strips(outer, inner, cell_h, fill) {
        // A pane with a background image: its veil already shows the pane's color here.
        if image_bg == Some(color) {
            continue;
        }
        let color = rgba(color, 1.0);
        if color != frame.clear {
            frame.quads.push(Quad { rect, color });
        }
    }
}

/// `margin_fill` for a see-through window: each strip is one layer at the pane's `alpha`, or
/// opaque when it is a non-default color (not `pane_bg`) and `opaque_colors` is on (13g).
pub fn margin_fill_alpha(frame: &mut Frame, outer: Rect, inner: Rect, cell_h: f32, fill: &crate::margins::Fill, pane_bg: Rgb, alpha: f32, opaque_colors: bool, bg_image: bool) {
    for (rect, color) in margin_strips(outer, inner, cell_h, fill) {
        if bg_image && color == pane_bg {
            continue;
        }
        let a = if opaque_colors && color != pane_bg { 1.0 } else { alpha };
        frame.quads.push(Quad { rect, color: crate::transparency::premultiplied([color.r, color.g, color.b], a) });
    }
}

/// A color for the quad shader. The target is sRGB, so the shader works in linear light.
pub fn rgba(c: Rgb, a: f32) -> [f32; 4] {
    [srgb_to_linear(c.r), srgb_to_linear(c.g), srgb_to_linear(c.b), a]
}

/// iTerm2 draws no cursor at or below this alpha (`iTermTextDrawingHelper.m:3899-3913`).
const CURSOR_MIN_ALPHA: f32 = 0.001;

/// What a cell's underline asks for (`underline::style_for`, the SGR 58 color).
#[derive(Clone, Copy, PartialEq)]
struct Ul {
    style: crate::underline::Style,
    strike: bool,
    sgr: Option<Rgb>,
}

/// How the app draws underlines: the two Advanced switches, the face's metrics and the
/// window's scale. The default derives geometry from the cell (previews, tests).
#[derive(Clone, Debug, PartialEq)]
pub struct DrawPrefs {
    /// `UnderlineHyperlinks`: OSC 8 links get iTerm2's dashes.
    pub underline_hyperlinks: bool,
    /// `SolidUnderlines`: underlines do not break around glyph ink (Task 7).
    pub solid_underlines: bool,
    pub metrics: crate::underline::Metrics,
    pub scale: f32,
    /// What the pane's profile and font say about ligatures (`fonts::span_level`).
    pub ligatures: crate::fonts::LigatureRules,
    /// The ligature level of the Non-ASCII Font (`fonts::ligature_level` of that font).
    pub alt_level: u8,
    /// What the pane's profile and the window say about anti-aliasing and thin strokes.
    pub masks: crate::fonts::MaskRules,
    /// Tandem draws box and block characters itself (`DisableCustomBoxDrawing` off).
    pub custom_boxes: bool,
    /// The pane's profile draws Powerline symbols itself (Draw Powerline Glyphs).
    pub powerline: bool,
    /// `SupportPowerlineExtendedSymbols`.
    pub powerline_extended: bool,
    /// `MakeSomePowerlineSymbolsWide`.
    pub powerline_wide: bool,
    /// The pane's Special Font Config (`render::Renderer::face_table`): the code points it
    /// covers draw in its fonts, remapped when an entry has a destination.
    pub table: Option<std::sync::Arc<crate::font_table::Config>>,
}

impl Default for DrawPrefs {
    fn default() -> Self {
        DrawPrefs { underline_hyperlinks: true, solid_underlines: false, metrics: crate::underline::Metrics { underline_offset: 0.0, thickness: 0.0, x_height: 0.0, baseline: 0.0 }, scale: 1.0, ligatures: STOCK_LIGATURES, alt_level: 2, masks: crate::fonts::MaskRules::STOCK, custom_boxes: true, powerline: false, powerline_extended: true, powerline_wide: true, table: None }
    }
}

/// The stock profile's ligature rules: both switches off, speed preferred, a font at level 2.
pub const STOCK_LIGATURES: crate::fonts::LigatureRules = crate::fonts::LigatureRules { ascii: false, non_ascii: false, prefer_speed: true, level: 2 };

impl DrawPrefs {
    /// The prefs the live frame and a screenshot both draw with: the two Advanced switches from
    /// `globals`, the renderer's face metrics and the window's scale.
    pub fn from_globals(globals: &crate::globals::Globals, metrics: crate::underline::Metrics, scale: f32) -> DrawPrefs {
        DrawPrefs { underline_hyperlinks: globals.bool("UnderlineHyperlinks"), solid_underlines: globals.bool("SolidUnderlines"), metrics, scale, ligatures: crate::fonts::LigatureRules { prefer_speed: globals.bool("PreferSpeedToFullLigatureSupport"), ..STOCK_LIGATURES }, alt_level: STOCK_LIGATURES.level, masks: crate::fonts::MaskRules { retina: scale >= 2.0, force_aa_on_retina: globals.bool("ForceAntialiasingOnRetina"), ..crate::fonts::MaskRules::STOCK }, custom_boxes: !globals.bool("DisableCustomBoxDrawing"), powerline: false, powerline_extended: globals.bool("SupportPowerlineExtendedSymbols"), powerline_wide: globals.bool("MakeSomePowerlineSymbolsWide"), table: None }
    }

    /// The metrics to draw with in a cell of height `ch`: the face's, or (none measured) an
    /// underline on the cell's last pixel and a strikethrough at mid cell.
    fn metrics_for(&self, ch: f32) -> crate::underline::Metrics {
        if self.metrics.thickness > 0.0 {
            self.metrics
        } else {
            crate::underline::Metrics { underline_offset: 0.0, thickness: 1.0, x_height: ch / 2.0, baseline: ch }
        }
    }
}

/// A stretch of cells on one row with one underline style and color.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Run {
    first: usize,
    last: usize,
    style: crate::underline::Style,
    strike: bool,
    color: Rgb,
}

/// The row's underline runs: adjacent cells with the same style, strikethrough and final
/// color merge. A pure function of the row (13m splits runs into visual spans).
/// The color is the cell's SGR 58 color, else the profile's, else its final text color.
fn underline_runs(cells: &[CellLook], profile: Option<Rgb>) -> Vec<Run> {
    let mut runs: Vec<Run> = Vec::new();
    for (col, c) in cells.iter().enumerate() {
        let Some(ul) = c.ul.filter(|u| u.style != crate::underline::Style::None || u.strike) else { continue };
        let color = crate::underline::color_for(ul.sgr, profile, c.fg);
        match runs.last_mut() {
            Some(r) if r.last + 1 == col && r.style == ul.style && r.strike == ul.strike && r.color == color => r.last = col,
            _ => runs.push(Run { first: col, last: col, style: ul.style, strike: ul.strike, color }),
        }
    }
    runs
}

/// Pushes one run's strokes into `frame.underlines`. Dashes are phased by x from the pane's
/// left edge, so a split run keeps the pattern.
fn emit_run(frame: &mut Frame, rect: Rect, row: usize, cell: (f32, f32), run: &Run, draw: &DrawPrefs) {
    let (cw, ch) = cell;
    let scale = draw.scale.max(1.0);
    let m = draw.metrics_for(ch);
    let top = rect.y + row as f32 * ch;
    let t = crate::underline::thickness(&m, scale);
    let all = crate::underline::strokes(run.style, top + crate::underline::underline_y(&m, ch, scale), top + ch, t, scale);
    let (x0, x1) = (run.first as f32 * cw, (run.last + 1) as f32 * cw);
    for s in &all {
        for (a, b) in crate::underline::dash_pieces(x0, x1, s) {
            frame.underlines.push(Quad { rect: Rect { x: rect.x + a, y: s.y, w: b - a, h: s.h }, color: rgba(run.color, 1.0) });
        }
    }
    // Strikethrough never breaks around ink (iTerm2's mask leaves it whole), so it has its own list.
    if run.strike {
        let s = crate::underline::snap(crate::underline::Stroke { y: top + crate::underline::strike_y(&m, scale), h: t, on: 0.0, off: 0.0, shift: 0.0 });
        frame.strikes.push(Quad { rect: Rect { x: rect.x + x0, y: s.y, w: x1 - x0, h: s.h }, color: rgba(run.color, 1.0) });
    }
}

struct CellLook {
    /// The underline and strikethrough the cell asks for.
    ul: Option<Ul>,
    text: String,
    fg: Rgb,
    bg: Rgb,
    bold: bool,
    italic: bool,
    /// The background comes from the selection or the cursor.
    selected: bool,
    /// The ligature level the cell's text is shaped at (`fonts::span_level`).
    level: u8,
    /// The mask mode of the cell's text (`fonts::MaskMode::code`).
    mask: u8,
    /// The cursor is on this cell: a ligature never joins it to a neighbor.
    cursor: bool,
    /// The terminal made the cell two columns (`Flags::WIDE_CHAR`): 13o's width rules widen
    /// cells unicode-width calls narrow (ambiguous, VS16, lone flags), so this, not the text's
    /// width, keeps a wide cell out of a ligature group (S2).
    wide: bool,
    /// The cell joins its neighbors when drawn (an Arabic letter in a right-to-left run): its
    /// run is one text piece, so the font can pick the joining forms.
    joins: bool,
    /// The Special Font Config entry that draws the cell (1 plus its index) and the code point
    /// it draws in place of the first one.
    table: Option<(u16, char)>,
}

/// The ligature level of a cell's text under the pane's rules (`fonts::span_level`): the font
/// the text draws in is the Non-ASCII Font for anything outside ASCII.
fn cell_level(text: &str, draw: &DrawPrefs) -> u8 {
    let ascii = text.is_ascii();
    let c = text.chars().next().unwrap_or(' ');
    let rules = if ascii { draw.ligatures } else { crate::fonts::LigatureRules { level: draw.alt_level, ..draw.ligatures } };
    crate::fonts::span_level(ascii, crate::fonts::fast_path(c, draw.ligatures.ascii), rules)
}

/// The mask mode code of a cell's text under the pane's rules (`fonts::MaskRules::mode`): plain
/// ASCII follows the ASCII switch, anything else the Non-ASCII switch.
fn cell_mask(text: &str, draw: &DrawPrefs, dark: bool) -> u8 {
    draw.masks.mode(text.is_ascii(), dark).code()
}

/// Explicit bidi controls never take part in a ligature piece (13m strips them from the text).
fn bidi_control(c: char) -> bool {
    matches!(c, '\u{61c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
}

/// A one-column non-ASCII cell that Non-ASCII Ligatures may group with its neighbors (P7): wide
/// characters and emoji keep their own piece, and so do bidi controls. A cell the terminal made
/// wide never groups, whatever unicode-width says of its text (S2).
fn groupable(c: &CellLook) -> bool {
    use unicode_width::UnicodeWidthStr;
    !c.wide && !c.text.is_ascii() && c.text.width() == 1 && !c.text.chars().any(bidi_control)
}

/// The Special Font Config entry (1 plus its index) that draws a cell, and the code point it
/// draws in place of the cell's first one. ASCII never has an entry; a code point the table sends
/// to something that is not a character draws as it was sent.
fn table_cell(table: &crate::font_table::Config, text: &str) -> Option<(u16, char)> {
    let first = text.chars().next().filter(|c| !c.is_ascii())?;
    let (i, drawn) = table.find(u32::from(first))?;
    Some((u16::try_from(i + 1).ok()?, char::from_u32(drawn)?))
}

/// The text a cell draws: its own, with the first code point the table's when an entry remaps it.
fn shown_text(c: &CellLook) -> std::borrow::Cow<'_, str> {
    match c.table {
        Some((_, drawn)) => {
            let first = c.text.chars().next().map_or(0, char::len_utf8);
            std::borrow::Cow::Owned(format!("{drawn}{}", &c.text[first..]))
        }
        None => std::borrow::Cow::Borrowed(c.text.as_str()),
    }
}

/// The one rule for whether `next` continues the text piece `prev` is in, two neighboring
/// cells of a row in display order (S3). A joined Arabic run (13m, `CellLook.joins`) is one
/// piece and is never split; nothing else joins it (bidi controls are stripped from it, P9). Any
/// other cell continues only within its font (ASCII, or grouped one-column non-ASCII under
/// Non-ASCII Ligatures, P7) and when no ligature boundary falls between (`ligature_break`).
fn piece_continues(prev: &CellLook, next: &CellLook, group: bool) -> bool {
    // A Special Font Config range boundary ends a piece: a font cannot join across fonts (P7).
    if prev.table.map(|t| t.0) != next.table.map(|t| t.0) {
        return false;
    }
    if prev.joins || next.joins {
        return prev.joins && next.joins;
    }
    !ligature_break(prev, next) && if next.text.is_ascii() { prev.text.is_ascii() } else { group && groupable(next) && groupable(prev) }
}

/// Whether a ligature must not join `prev` and `next`, two neighboring cells: only when one of
/// them is shaped with ligatures, a color change, the cursor or the selection edge ends the run
/// (iTerm2 builds its attributed string per color and per cursor and selection state).
fn ligature_break(prev: &CellLook, next: &CellLook) -> bool {
    (prev.level > 0 || next.level > 0) && (prev.fg != next.fg || prev.cursor != next.cursor || prev.selected != next.selected || prev.bold != next.bold || prev.italic != next.italic)
}

/// Find matches to mark in a pane.
pub struct Highlights<'a> {
    pub matches: &'a [crate::find::Match],
    pub color: Rgb,
    /// The Win-click target under the pointer, underlined in `link`.
    pub hover: Option<crate::find::Match>,
    pub link: Rgb,
    /// Shortcut labels, each over the cells from its position.
    pub labels: &'a [(crate::find::Pos, String)],
    /// Annotated text (a yellow underline), with the note's text for a note on show.
    pub notes: &'a [(crate::find::Match, Option<String>)],
    /// How the pane dims (`App::pane_dim`). The app lays the gray quad over the whole pane;
    /// with Dim Only Text `pane_with` dims the text and non-default backgrounds itself.
    pub dim: crate::dim::Dim,
    /// The key window's focused pane at a password prompt: the key glyph replaces the cursor
    /// (`iTermTextDrawingHelper.m:4053-4068`).
    pub key_cursor: bool,
    /// The pane's background alpha (`1 - Transparency`); 1 draws as before.
    pub bg_alpha: f32,
    /// Keep background colors opaque: non-default and selected cells stay solid.
    pub opaque_colors: bool,
    /// The blink phase for text with the blink attribute: false hides it when the profile
    /// allows blinking. True in tests and screenshots.
    pub text_blink_on: bool,
}

/// What the find bar shows.
pub struct FindBar<'a> {
    pub query: &'a str,
    pub count: &'a str,
    pub mode: &'a str,
    /// The query is not a valid regular expression.
    pub error: bool,
}

/// Draws one pane's visible screen into `rect`.
/// `look` says how the cursor is drawn; `look.visible` false hides it (the blink phase).
pub fn pane(frame: &mut Frame, screen: &Screen, rect: Rect, cell: (f32, f32), focused: bool, look: &CursorLook) {
    pane_with(frame, screen, rect, cell, focused, look, &DrawPrefs::default(), None, crate::render::DEFAULT_FACE)
}

/// The cursor's screen row and column, and whether it sits on a wide character; None when
/// the program hid it or it is scrolled off the view.
pub fn cursor_cell(screen: &Screen) -> Option<(usize, usize, bool)> {
    let term = screen.term();
    let content = term.renderable_content();
    if content.cursor.shape == CursorShape::Hidden {
        return None;
    }
    let p = point_to_viewport(content.display_offset, content.cursor.point).filter(|p| p.line < term.screen_lines())?;
    let wide = term.grid()[content.cursor.point].flags.contains(Flags::WIDE_CHAR);
    Some((p.line, p.column.0, wide))
}

/// Whether the cursor blinks where someone can see it: a program that hid it (DECTCEM) or
/// scrolled it off the view leaves nothing to blink, so no blink clock runs for it.
pub fn cursor_blinks_shown(screen: &Screen) -> bool {
    screen.cursor_blinks() && cursor_cell(screen).is_some()
}

/// The leftmost visual column of the cells `col..col + cells` of a row in display order `order`.
fn visual_start(order: Option<&crate::bidi::Order>, col: usize, cells: usize) -> usize {
    order.and_then(|o| crate::bidi::visual_spans(o, col..col + cells).first().map(|r| r.start)).unwrap_or(col)
}

/// Where a composition starts: the cursor's cell, whether or not a program hid the cursor
/// (iTerm2 draws marked text regardless); the bottom-left of the visible grid when a scrolled
/// view has the cursor off screen.
pub fn ime_cell(screen: &Screen) -> (usize, usize) {
    let term = screen.term();
    match point_to_viewport(term.grid().display_offset(), term.grid().cursor.point).filter(|p| p.line < term.screen_lines()) {
        Some(p) => (p.line, p.column.0),
        None => (term.screen_lines().saturating_sub(1), 0),
    }
}

/// Draws the find bar at the top right of `pane`, over the text. Returns the mode label's
/// rectangle, which opens the mode menu when clicked.
pub fn find_bar(frame: &mut Frame, bar: &FindBar, pane: Rect, cell: (f32, f32)) -> Rect {
    const FIELD: usize = 24;
    let (cw, ch) = cell;
    let mode = format!("[{}]", bar.mode);
    let chars = 1 + FIELD + 2 + bar.count.chars().count() + 2 + mode.chars().count() + 1;
    let w = (chars as f32 * cw).min(pane.w);
    let r = Rect { x: pane.x + pane.w - w, y: pane.y, w, h: ch + 8.0 };
    frame.quads.push(Quad { rect: r, color: rgba(FIND_BAR, 1.0) });
    // The pane's text under the bar is cut off where the bar starts.
    cut_rows_from(frame, r, ch);
    // The end of a long query stays in view, with a caret after it.
    let shown: String = {
        let n = bar.query.chars().count();
        bar.query.chars().skip(n.saturating_sub(FIELD - 1)).collect()
    };
    let field = format!("{shown}_");
    let span = |text: String, color: Rgb| Span { text, color, bold: false, italic: false, alt_font: false, icon: false, prose: false, badge: false, toolbelt: false, fake_bold: false, ligatures: 0, mask: 0, font: 0 };
    let y = r.y + 4.0;
    let query_color = if bar.error { FIND_ERROR } else { TAB_TEXT };
    frame.rows.push(TextRow {
        x: r.x + cw,
        y,
        spans: vec![span(format!("{field:<FIELD$}  {}", bar.count), query_color)],
        clip: r,
        scale: 1.0,
        face: crate::render::DEFAULT_FACE,
    });
    let mode_x = r.x + (1 + FIELD + 2 + bar.count.chars().count() + 2) as f32 * cw;
    let mode_rect = Rect { x: mode_x, y: r.y, w: mode.chars().count() as f32 * cw, h: r.h };
    frame.rows.push(TextRow { x: mode_x, y, spans: vec![span(mode, TAB_TEXT_DIM)], clip: r, scale: 1.0, face: crate::render::DEFAULT_FACE });
    mode_rect
}

/// iTerm2's Copy Mode cursor: a flag in the lower third of the cell at `row`, `col` (on
/// screen), white, or light blue while selecting, with a black edge.
pub fn copy_cursor(frame: &mut Frame, rect: Rect, cell: (f32, f32), row: usize, col: usize, selecting: bool) {
    const SLICES: usize = 4;
    let (cw, ch) = cell;
    let (x, y) = (rect.x + col as f32 * cw, rect.y + row as f32 * ch);
    let (top, h) = (y + ch * 2.0 / 3.0, ch / 3.0 / SLICES as f32);
    let fill = if selecting { rgb(0xc1, 0xde, 0xff) } else { rgb(0xff, 0xff, 0xff) };
    // A triangle pointing up at the text, drawn as slices that widen toward the bottom.
    let slice = |i: usize, grow: f32| {
        let w = cw * (i + 1) as f32 / SLICES as f32;
        Rect { x: x + (cw - w) / 2.0 - grow, y: top + i as f32 * h - grow, w: w + 2.0 * grow, h: h + 2.0 * grow }
    };
    for i in 0..SLICES {
        frame.overlay.push(Quad { rect: slice(i, 1.0), color: rgba(rgb(0, 0, 0), 1.0) });
    }
    for i in 0..SLICES {
        frame.overlay.push(Quad { rect: slice(i, 0.0), color: rgba(fill, 1.0) });
    }
}

/// The cursor guide: a band of `color` (with its alpha) across the pane on screen row `row`.
pub fn cursor_guide(frame: &mut Frame, rect: Rect, cell: (f32, f32), row: usize, color: [f32; 4]) {
    let r = Rect { x: rect.x, y: rect.y + row as f32 * cell.1, w: rect.w, h: cell.1 };
    frame.quads.push(Quad { rect: r, color });
}

/// Per-line timestamps, in the profile's foreground on a 0.9-alpha block of its background
/// (`colors` is that pair), as iTerm2 draws every mode (`iTermTimestampDrawHelper.m`
/// `drawBackgroundInFrame:`). With no `column` each label is right-aligned on its screen row,
/// over the text, which stops where the label starts. With a `column` (Adjacent) the labels
/// form one block ending at the column's right edge: every label starts at the same x, and a
/// label equal to the one on the row above is two 1 px lines (`drawString:...`,
/// `rowIsRepeat:`). A block wider than the column extends left over the text, which then stops
/// where the block starts (`frameForStringGivenWidth:`, `x = MAX(0, maxX - w)`); it is never cut.
pub fn timestamps(frame: &mut Frame, labels: &[(usize, String)], rect: Rect, cell: (f32, f32), column: Option<Rect>, colors: (Rgb, Rgb)) {
    let (cw, ch) = cell;
    let (fg, bg) = colors;
    let span = |text: &str| Span { text: text.to_string(), color: fg, bold: false, italic: false, alt_font: false, icon: false, prose: false, badge: false, toolbelt: false, fake_bold: false, ligatures: 0, mask: 0, font: 0 };
    // The pane's text on a row, and its underlines and strikes, stop where its label's block starts.
    let clip_text = |frame: &mut Frame, y: f32, x: f32| {
        for r in frame.rows.iter_mut().filter(|r| r.y == y && r.clip.x + r.clip.w > x) {
            r.clip.w = (x - r.clip.x).max(0.0);
        }
        let band = Rect { x, y, w: 1.0e6, h: ch };
        cut_quads(frame, band);
        cut_glyphs(frame, band);
    };
    if let Some(col) = column {
        let right = col.x + col.w;
        let x = (right - crate::stamps::block_width(cw, labels)).max(rect.x);
        let over_text = x < rect.x + rect.w;
        let clip = Rect { x, w: right - x, ..col };
        let mut prev: Option<(usize, &str)> = None;
        for (row, text) in labels {
            let y = col.y + *row as f32 * ch;
            if over_text {
                clip_text(frame, y, x);
            }
            frame.quads.push(Quad { rect: Rect { x, y, w: right - x, h: ch }, color: rgba(bg, 0.9) });
            let repeat = prev.is_some_and(|(r, t)| r + 1 == *row && t == text);
            prev = Some((*row, text));
            if repeat {
                // The center of a repeat is 10 px into the block (`center = x + 10`).
                for dx in [-1.0, 1.0] {
                    frame.quads.push(Quad { rect: Rect { x: x + 10.0 + dx, y, w: 1.0, h: ch }, color: rgba(fg, 1.0) });
                }
                continue;
            }
            frame.rows.push(TextRow { x, y, spans: vec![span(text)], clip, scale: 1.0, face: crate::render::DEFAULT_FACE });
        }
        return;
    }
    let right = rect.x + rect.w;
    for (row, text) in labels {
        let n = text.chars().count() as f32;
        let y = rect.y + *row as f32 * ch;
        let bg_rect = Rect { x: right - (n + 2.0) * cw, y, w: (n + 2.0) * cw, h: ch };
        frame.quads.push(Quad { rect: bg_rect, color: rgba(bg, 0.9) });
        clip_text(frame, y, bg_rect.x);
        frame.rows.push(TextRow { x: right - (n + 1.0) * cw, y, spans: vec![span(text)], clip: rect, scale: 1.0, face: crate::render::DEFAULT_FACE });
    }
}

/// The tallest band `fill_polygon` fills (a smear is a cell or two; a profile's absurd values
/// must not make a frame loop for ever).
const POLYGON_MAX_BAND: f32 = 16384.0;

/// The first and last row of pixel bands a polygon covers, None for a polygon that cannot be
/// drawn: a point that is not finite, or a band taller than any pane.
pub fn polygon_band(points: &[(f32, f32)]) -> Option<(f32, f32)> {
    let top = points.iter().map(|p| p.1).fold(f32::INFINITY, f32::min).floor();
    let bottom = points.iter().map(|p| p.1).fold(f32::NEG_INFINITY, f32::max).ceil();
    if !top.is_finite() || !bottom.is_finite() || points.iter().any(|p| !p.0.is_finite() || !p.1.is_finite()) || bottom - top > POLYGON_MAX_BAND {
        return None;
    }
    Some((top, bottom))
}

/// Fills a convex polygon with one-pixel horizontal bands (a path renderer is not worth it
/// for the cursor's smear). Fewer than three points, or points that cannot be drawn, push
/// nothing.
pub fn fill_polygon(quads: &mut Vec<Quad>, points: &[(f32, f32)], color: [f32; 4]) {
    if points.len() < 3 {
        return;
    }
    let Some((top, bottom)) = polygon_band(points) else { return };
    let mut y = top;
    while y < bottom {
        let cy = y + 0.5;
        let (mut lo, mut hi) = (f32::INFINITY, f32::NEG_INFINITY);
        for i in 0..points.len() {
            let (p, q) = (points[i], points[(i + 1) % points.len()]);
            if p.1 != q.1 && cy >= p.1.min(q.1) && cy < p.1.max(q.1) {
                let x = p.0 + (q.0 - p.0) * (cy - p.1) / (q.1 - p.1);
                lo = lo.min(x);
                hi = hi.max(x);
            }
        }
        if hi > lo {
            quads.push(Quad { rect: Rect { x: lo, y, w: hi - lo, h: 1.0 }, color });
        }
        y += 1.0;
    }
}

/// Find Cursor: the window dims, except a clear circle of `radius` around `center`.
pub fn find_cursor(frame: &mut Frame, area: Rect, center: (f32, f32), radius: f32) {
    const BAND: f32 = 2.0;
    let color = rgba(rgb(0x40, 0x40, 0x40), 0.7);
    let (cx, cy) = center;
    let right = area.x + area.w;
    let mut y = area.y;
    while y < area.y + area.h {
        let h = BAND.min(area.y + area.h - y);
        let dy = y + h / 2.0 - cy;
        if dy.abs() < radius {
            // The circle's width on this band is left clear.
            let half = (radius * radius - dy * dy).sqrt();
            frame.overlay.push(Quad { rect: Rect { x: area.x, y, w: (cx - half - area.x).max(0.0), h }, color });
            let from = (cx + half).min(right);
            frame.overlay.push(Quad { rect: Rect { x: from, y, w: right - from, h }, color });
        } else {
            frame.overlay.push(Quad { rect: Rect { x: area.x, y, w: area.w, h }, color });
        }
        y += h;
    }
}

/// Black or white, whichever reads better on `bg`, as iTerm2 picks text on a match.
fn match_text(bg: Rgb) -> Rgb {
    let lum = 0.30 * bg.r as f32 + 0.59 * bg.g as f32 + 0.11 * bg.b as f32;
    if lum > 127.5 { rgb(0, 0, 0) } else { rgb(0xff, 0xff, 0xff) }
}

pub fn pane_with(
    frame: &mut Frame,
    screen: &Screen,
    rect: Rect,
    cell: (f32, f32),
    focused: bool,
    look: &CursorLook,
    draw: &DrawPrefs,
    highlights: Option<&Highlights>,
    face: u16,
) {
    let history = screen.term().grid().history_size() as i32;
    // Underlines and strikethrough come from the cells' flags (`CellLook.ul`); the Win-click
    // target under the pointer gets a solid line in the link color.
    let mut hovered: Vec<(usize, usize)> = Vec::new();
    let (cw, ch) = cell;
    if face != crate::render::DEFAULT_FACE && !frame.face_heights.iter().any(|(f, _)| *f == face) {
        frame.face_heights.push((face, ch));
    }
    let palette = &screen.palette;
    let term = screen.term();
    let (cols, lines) = (term.columns(), term.screen_lines());
    let content = term.renderable_content();
    // Cursor Hidden When Unfocused (`iTermTextDrawingHelper.m:4254-4256`).
    let key_cursor = highlights.is_some_and(|h| h.key_cursor);
    // A composition in progress replaces the terminal cursor (`iTermTextDrawingHelper.m:4267`),
    // except at a password prompt, where the key glyph is the cursor (P11).
    let ime = look.ime.as_ref().filter(|_| !key_cursor);
    // The cursor's logical cell; `cursor` below is where it draws once rows are in display order.
    let cursor_at = cursor_cell(screen).filter(|_| ime.is_none() && look.visible && look.alpha > CURSOR_MIN_ALPHA && !(look.hide_unfocused && !focused)).map(|(row, col, _)| (row, col));
    let wide_cursor = cursor_cell(screen).is_some_and(|(_, _, w)| w);
    let bg_brightness = crate::tab_theme::perceived(palette.bg);
    let bg_alpha = highlights.map_or(1.0, |h| h.bg_alpha);
    let opaque_colors = highlights.is_some_and(|h| h.opaque_colors);
    let block_cursor = !key_cursor && focused && content.cursor.shape == CursorShape::Block;
    // Smart Cursor Color: a focused box only (`iTermMetalPerFrameState.m:455-490`). The cell's
    // own colors and its neighbors' backgrounds come before selection and match highlights.
    let smart_box = look.smart.filter(|_| block_cursor).and_then(|(bg_t, fg_t)| {
        let (row, col) = cursor_at?;
        let at = |r: i64, c: i64| -> Option<alacritty_terminal::term::cell::Cell> {
            (r >= 0 && c >= 0 && (r as usize) < lines && (c as usize) < cols).then(|| term.grid()[alacritty_terminal::index::Point::new(alacritty_terminal::index::Line(r as i32 - content.display_offset as i32), alacritty_terminal::index::Column(c as usize))].clone())
        };
        let own = at(row as i64, col as i64)?;
        let (fg, bg) = palette.cell(own.fg, own.bg, own.flags);
        let (r, c) = (row as i64, col as i64);
        let neighbors: Vec<Rgb> = [(r - 1, c), (r, c - 1), (r, c + 1), (r + 1, c)]
            .into_iter()
            .filter_map(|(r, c)| at(r, c))
            .map(|n| palette.cell(n.fg, n.bg, n.flags - Flags::BOLD - Flags::DIM).1)
            .collect();
        Some(crate::cursor_look::smart_colors(fg, bg, &neighbors, bg_t as f64, fg_t as f64))
    });
    let cursor_box = smart_box.map_or(palette.cursor, |(b, _)| b);
    let cursor_text = smart_box.map_or(palette.cursor_text, |(_, t)| t);

    // Thin Strokes on Dark Backgrounds is decided per cell: the cell's background is darker than
    // its text (`iTermTextDrawingHelper.m:3045`, `iTermMetalPerFrameState.m:2186`).
    let dark = |fg: Rgb, bg: Rgb| crate::tab_theme::hsp(bg) < crate::tab_theme::hsp(fg);
    // Each row's display order (right-to-left text); None draws it as it is. A row is its own
    // line of program output: nothing here moves a cell to another row.
    let display_offset = content.display_offset as i32;
    let orders: Vec<Option<std::sync::Arc<crate::bidi::Order>>> = (0..lines).map(|row| screen.row_order(row as i32 - display_offset).filter(|o| o.lut.len() <= cols)).collect();
    let visual_col = |row: usize, col: usize| orders.get(row).and_then(|o| o.as_ref()).and_then(|o| o.lut.get(col)).map_or(col, |&v| v as usize);
    let cursor = cursor_at.map(|(row, col)| (row, visual_col(row, col)));
    let blank = || CellLook { ul: None, text: " ".into(), fg: palette.fg, bg: palette.bg, bold: false, italic: false, selected: false, level: cell_level(" ", draw), mask: cell_mask(" ", draw, dark(palette.fg, palette.bg)), cursor: false, wide: false, joins: false, table: None };
    let mut grid: Vec<Vec<CellLook>> = (0..lines).map(|_| (0..cols).map(|_| blank()).collect()).collect();
    for indexed in content.display_iter {
        let Some(vp) = point_to_viewport(content.display_offset, indexed.point) else { continue };
        let (row, col) = (vp.line, vp.column.0);
        if row >= lines || col >= cols {
            continue;
        }
        let c = indexed.cell;
        let (raw_fg, raw_bg) = palette.cell(c.fg, c.bg, c.flags);
        // Minimum contrast adjusts the raw text color against the raw background, before Cursor
        // Boost mutes and Dim Only Text dims (`iTermColorMap.m:318-360`). Block characters
        // (`contrastIneligible`, `iTermAttributedStringBuilder.m:755`) and hidden cells keep theirs.
        let exempt = c.flags.contains(Flags::HIDDEN) || (c.zerowidth().is_none() && crate::boxdraw::is_block(c.c));
        let adjust = |text: Rgb, against: Rgb| if exempt { text } else { crate::contrast::apply(text, against, palette.min_contrast) };
        // Cursor Boost mutes first, then Dim Only Text dims (`iTermColorMap.m:347-360`).
        let own_text = |against: Rgb| {
            let fg = crate::cursor_look::mute(adjust(raw_fg, against), palette.bg, look.boost);
            match highlights.map(|h| h.dim).filter(|d| d.only_text) {
                Some(d) => crate::dim::text_color(fg, d, bg_brightness),
                None => fg,
            }
        };
        let (mut fg, mut bg) = (own_text(raw_bg), crate::cursor_look::mute(raw_bg, palette.bg, look.boost));
        if let Some(d) = highlights.map(|h| h.dim).filter(|d| d.only_text) {
            bg = crate::dim::bg_color(bg, palette.bg, d, bg_brightness);
        }
        // The text and background the cell ended with, for the later choices; `own` while the
        // text is still the cell's own color.
        // The text a match or a hovered link chose, before any adjusting, so a selection over
        // the cell adjusts it against the selection color that draws (iTerm2 adjusts against the
        // background actually drawn).
        let (mut under, mut own) = (raw_bg, true);
        let mut raw_choice: Option<Rgb> = None;
        let mut hover = false;
        if let Some(h) = highlights {
            let at = crate::find::Pos { row: (indexed.point.line.0 + history) as usize, col };
            let i = h.matches.partition_point(|m| m.end < at);
            if h.matches.get(i).is_some_and(|m| m.start <= at) {
                // The match colors are muted by Cursor Boost like every other background and text.
                (fg, bg) = (crate::cursor_look::mute(adjust(match_text(h.color), h.color), palette.bg, look.boost), crate::cursor_look::mute(h.color, palette.bg, look.boost));
                (under, own) = (h.color, false);
                raw_choice = Some(match_text(h.color));
            }
            if h.hover.is_some_and(|m| m.start <= at && at <= m.end) {
                fg = adjust(h.link, under);
                own = false;
                raw_choice = Some(h.link);
                hover = true;
                hovered.push((row, visual_col(row, col)));
            }
        }
        let mut hidden = false;
        if let Some(h) = highlights {
            let at_row = (indexed.point.line.0 + history) as usize;
            if h.labels.iter().any(|(p, l)| p.row == at_row && col >= p.col && col < p.col + l.chars().count()) {
                (fg, bg) = (LABEL_TEXT, LABEL_BG);
                own = false;
                raw_choice = None;
                hidden = true;
            }
        }
        let mut selected = false;
        // A visual selection (bidi rows) is its runs inside the hull.
        if content.selection.as_ref().is_some_and(|s| s.contains(indexed.point)) && term.selection.as_ref().map_or(true, |s| s.covers(indexed.point)) {
            // The text is adjusted against the selection color; `fg` is already muted otherwise.
            fg = match palette.selected_text {
                Some(t) => crate::cursor_look::mute(adjust(t, palette.selection), palette.bg, look.boost),
                None if own => own_text(palette.selection),
                // Hover text is not muted (the hover ruling); a match's is.
                None => raw_choice.map_or(fg, |t| {
                    let a = adjust(t, palette.selection);
                    if hover { a } else { crate::cursor_look::mute(a, palette.bg, look.boost) }
                }),
            };
            bg = crate::cursor_look::mute(palette.selection, palette.bg, look.boost);
            selected = true;
        }
        if block_cursor && cursor_at == Some((row, col)) {
            // The cursor box is its own quad (after the background runs); the text takes its color.
            // A fading cursor (smooth blink) takes the text color part of the way.
            fg = if look.alpha < 1.0 { blend(fg, cursor_text, look.alpha) } else { cursor_text };
        }
        if let Some(t) = c.image() {
            if let Some(img) = screen.images.by_id.get(&t.image) {
                let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
                let r = Rect { x: rect.x + col as f32 * cw, y: rect.y + row as f32 * ch, w: cw, h: ch };
                if let Some(q) = tile_quad(img, t, r, frame_at(&img.frames, now)) {
                    frame.images.push(q);
                }
            }
        }
        // Blinking text hides in the off phase when the profile allows it
        // (`iTermMetalPerFrameState.m:1691`); the background keeps drawing, and so do the
        // neighbors' lines, but the cell's own underline and strikethrough go with its glyph.
        let blink_off = screen.blink_allowed && c.flags.contains(Flags::BLINK) && !highlights.is_none_or(|h| h.text_blink_on);
        let mut text = String::new();
        if hidden || blink_off {
            text.push(' ');
        } else if !c.flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
            // U+200B in a cell of its own (13o, Zero-Width Space Advances Cursor) draws as a
            // space: a zero-advance glyph in a shaped run would pull the text after it one cell
            // left (S3).
            text.push(if c.c == '\u{200B}' { ' ' } else { c.c });
            if let Some(extra) = c.zerowidth() {
                text.extend(extra);
            }
        }
        // No underline under a label, a picture or a spacer's missing text; a faint cell's SGR 58
        // color is faint too (`iTermMetalPerFrameState.m:1613-1620`), nothing else touches it.
        // The cell's text in the profile's Unicode Normalization form, before anything below
        // classifies it (S1: `cell_level`, `cell_mask`, the box-glyph and ligature tests).
        crate::encoding::normalize_in_place(&mut text, screen.normalization);
        let (style, strike) = crate::underline::style_for(c.flags, !hover && c.hyperlink().is_some(), draw.underline_hyperlinks);
        let sgr = c.underline_color().map(|u| {
            let u = palette.color(u);
            if c.flags.contains(Flags::DIM) { blend(palette.cell(c.fg, c.bg, c.flags - Flags::DIM).1, u, palette.faint_alpha) } else { u }
        });
        let ul = (!hidden && !blink_off && c.image().is_none()).then_some(Ul { style, strike, sgr });
        let level = cell_level(&text, draw);
        let mask = cell_mask(&text, draw, dark(fg, bg));
        grid[row][col] = CellLook {
            ul,
            text,
            fg,
            bg,
            bold: c.flags.contains(Flags::BOLD),
            italic: c.flags.contains(Flags::ITALIC),
            selected,
            level,
            mask,
            // The logical cursor cell (the row is not in display order yet); only a block cursor breaks a ligature.
            cursor: block_cursor && cursor_at == Some((row, col)),
            wide: c.flags.contains(Flags::WIDE_CHAR),
            joins: false,
            table: None,
        };
    }

    // Right-to-left rows: the whole row of looks moves to its display order once, so the
    // backgrounds, text pieces, colors and selection all follow (P5). Mirrored characters draw
    // as their counterparts; Arabic letters are marked so a run is shaped as one piece.
    for (row, order) in orders.iter().enumerate() {
        let Some(o) = order else { continue };
        let mut old: Vec<Option<CellLook>> = std::mem::take(&mut grid[row]).into_iter().map(Some).collect();
        let mut new: Vec<CellLook> = (0..cols)
            .map(|visual| {
                let logical = o.inverse.get(visual).map_or(visual, |&l| l as usize);
                old[logical].take().unwrap_or_else(blank)
            })
            .collect();
        for (logical, &visual) in o.lut.iter().enumerate() {
            let cell = &mut new[visual as usize];
            let first = cell.text.chars().next();
            if o.mirrored[logical] {
                if let Some(c) = first {
                    cell.text.replace_range(..c.len_utf8(), &crate::bidi::mirror(c).to_string());
                }
            }
            cell.joins = o.rtl[logical] && first.is_some_and(crate::bidi::joins);
        }
        grid[row] = new;
    }

    // Special Font Config: each cell a table entry covers takes its entry (after the display
    // order, so a mirrored character is the one looked up).
    if let Some(table) = draw.table.as_deref().filter(|t| !t.entries.is_empty()) {
        for cell in grid.iter_mut().flatten() {
            cell.table = table_cell(table, &cell.text);
        }
    }

    for (row, cells) in grid.iter().enumerate() {
        let y = rect.y + row as f32 * ch;
        // Background runs. In a see-through pane the default background is drawn here too (one
        // layer per pixel, `iTermTextDrawingHelper.m:772-790`); Keep background colors opaque
        // keeps non-default and selected cells solid. The cursor box lies over this, opaque.
        let alpha_of = |c: &CellLook| {
            if opaque_colors && (c.bg != palette.bg || c.selected) { 1.0 } else { bg_alpha }
        };
        let mut col = 0;
        while col < cols {
            let (bg, alpha) = (cells[col].bg, alpha_of(&cells[col]));
            let start = col;
            while col < cols && cells[col].bg == bg && alpha_of(&cells[col]) == alpha {
                col += 1;
            }
            if bg != palette.bg || alpha < 1.0 {
                let r = Rect { x: rect.x + start as f32 * cw, y, w: (col - start) as f32 * cw, h: ch };
                let color = if alpha < 1.0 { crate::transparency::premultiplied([bg.r, bg.g, bg.b], alpha) } else { rgba(bg, 1.0) };
                frame.quads.push(Quad { rect: r, color });
            }
        }
        if block_cursor && cursor.is_some_and(|(r, _)| r == row) {
            let (_, col) = cursor.unwrap();
            let r = box_rect(&look.geometry, (rect.x, rect.y), cell, row, col, wide_cursor);
            frame.quads.push(Quad { rect: r, color: rgba(cursor_box, look.alpha) });
        }
        // Text pieces. An ASCII run shares one piece. Any other character gets its own
        // piece at its cell's x, because fallback fonts (CJK, emoji) are not monospace
        // and would push the text after them off the grid.
        let flush = |piece: &mut Option<(usize, Vec<Span>)>, frame: &mut Frame| {
            if let Some((start, mut spans)) = piece.take() {
                while spans.last().is_some_and(|s| s.text.trim_end().is_empty()) {
                    spans.pop();
                }
                if !spans.is_empty() {
                    frame.rows.push(TextRow { x: rect.x + start as f32 * cw, y, spans, clip: rect, scale: 1.0, face });
                }
            }
        };
        // With Non-ASCII Ligatures a run of one-column non-ASCII cells shares a piece too (P7).
        let group = draw.ligatures.non_ascii;
        let mut piece: Option<(usize, Vec<Span>)> = None;
        let mut before: Option<usize> = None;
        let mut col = 0;
        while col < cells.len() {
            let c = &cells[col];
            if c.text.is_empty() {
                col += 1;
                continue;
            }
            let at = col;
            col += 1;
            // A box or block character is drawn by Tandem to fill its cell (iTerm2 does the
            // same): it leaves the text, ends the piece around it, and takes the cell's color.
            // The row is in display order here, so `at` is the VISUAL column and the character
            // is the mirrored one on a right-to-left row (S4); `c.fg` is the color after minimum
            // contrast, blocks exempt (S2); a blinked-out or hidden cell's text is a space (S7).
            let mut chars = c.text.chars();
            if let (true, Some(drawn), None) = (draw.custom_boxes, chars.next(), chars.next()) {
                if crate::boxdraw::drawn_with(drawn, draw.powerline, draw.powerline_extended, false) {
                    flush(&mut piece, frame);
                    // A wide Powerline symbol draws over the next cell too; the grid keeps one cell
                    // (the next cell's text still lays out at its own column), as in iTerm2.
                    let span = if draw.powerline && crate::powerline::wide(u32::from(drawn), draw.powerline_wide) { 2.0 } else { 1.0 };
                    frame.glyphs.push(CellGlyph { c: drawn, rect: Rect { x: rect.x + at as f32 * cw, y, w: cw * span, h: ch }, color: c.fg, face, clip: rect });
                    before = None;
                    continue;
                }
            }
            let ascii = c.text.is_ascii();
            let joinable = group && groupable(c);
            // What the cell draws: the table's code point in place of the sent one.
            let shown = shown_text(c);
            // The previous cell, when it is the one right before (a wide character's spacer is empty).
            let prev = before.filter(|b| *b + 1 == at).map(|b| &cells[b]);
            if !prev.is_some_and(|p| piece_continues(p, c, group)) {
                flush(&mut piece, frame);
            }
            let (_, spans) = piece.get_or_insert_with(|| (at, Vec::new()));
            let font = c.table.map_or(0, |t| t.0);
            let same = |s: &Span| s.color == c.fg && s.bold == c.bold && s.italic == c.italic && s.ligatures == c.level && s.mask == c.mask && s.font == font;
            let span = |text: String| Span { text, color: c.fg, bold: c.bold, italic: c.italic, alt_font: !ascii, icon: false, prose: false, badge: false, toolbelt: false, fake_bold: c.bold, ligatures: c.level, mask: c.mask, font: c.table.map_or(0, |t| t.0) };
            if c.joins {
                // An Arabic run is one piece at its visual start, in logical order (the cells
                // read right to left on screen), so the font joins the letters: each cell goes
                // in front. The shaper runs its own bidi over the piece: explicit controls
                // riding on a cell must not reach it (P9).
                let text: String = shown.chars().filter(|&ch| !crate::bidi::is_explicit_control(ch)).collect();
                match spans.first_mut() {
                    Some(s) if same(s) => s.text.insert_str(0, &text),
                    _ => spans.insert(0, span(text)),
                }
            } else {
                match spans.last_mut() {
                    Some(s) if same(s) => s.text.push_str(&shown),
                    _ => spans.push(span(shown.to_string())),
                }
                // Any other non-ASCII character gets its own piece at its cell's x, because
                // fallback fonts (CJK, emoji) are not monospace.
                if !ascii && !joinable {
                    flush(&mut piece, frame);
                }
            }
            before = Some(at);
        }
        flush(&mut piece, frame);
        for run in underline_runs(cells, palette.underline_color) {
            emit_run(frame, rect, row, cell, &run, draw);
        }
    }

    if let Some(h) = highlights {
        let top = history - content.display_offset as i32;
        for (p, label) in h.labels {
            let row = p.row as i32 - top;
            if row >= 0 && (row as usize) < lines {
                let span = Span { text: label.clone(), color: LABEL_TEXT, bold: true, italic: false, alt_font: false, icon: false, prose: false, badge: false, toolbelt: false, fake_bold: false, ligatures: 0, mask: 0, font: 0 };
                // The tag sits at the leftmost of the cells it covers, where the row draws them.
                let col = visual_start(orders.get(row as usize).and_then(|o| o.as_deref()), p.col, label.chars().count());
                frame.rows.push(TextRow { x: rect.x + col as f32 * cw, y: rect.y + row as f32 * ch, spans: vec![span], clip: rect, scale: 1.0, face });
            }
        }
    }
    let underline_y = |row: usize| rect.y + row as f32 * ch + ch - 1.0;
    if let Some(h) = highlights {
        let top = history - content.display_offset as i32;
        for (m, label) in h.notes {
            // iTerm2: a 1 px yellow line 2 px above the row's bottom under each annotated cell.
            // Only the rows on screen, and only columns inside the pane.
            let first = m.start.row.max(top.max(0) as usize);
            let last = m.end.row.min((top + lines as i32 - 1).max(0) as usize);
            for r in first..=last.max(first) {
                let row = r as i32 - top;
                if row < 0 || row as usize >= lines || r > m.end.row {
                    continue;
                }
                let a = (if r == m.start.row { m.start.col } else { 0 }).min(cols - 1);
                let b = (if r == m.end.row { m.end.col } else { cols - 1 }).min(cols - 1);
                let y = rect.y + row as f32 * ch + ch - 3.0;
                // One line per visual piece the annotated cells are drawn in.
                let pieces = match orders.get(row as usize).and_then(|o| o.as_deref()) {
                    Some(o) => crate::bidi::visual_spans(o, a..b + 1),
                    None => vec![a..b + 1],
                };
                for piece in pieces {
                    frame.overlay.push(Quad { rect: Rect { x: rect.x + piece.start as f32 * cw, y, w: (piece.end - piece.start) as f32 * cw, h: 1.0 }, color: rgba(NOTE, 1.0) });
                }
            }
            // A note on show: its text in a box after the annotated text.
            let row = m.end.row as i32 - top;
            if let (Some(text), true) = (label, row >= 0 && (row as usize) < lines) {
                let shown: String = text.lines().next().unwrap_or_default().chars().take(40).collect();
                // The box follows the annotated text's rightmost visual cell.
                let from = if m.start.row == m.end.row { m.start.col } else { 0 };
                let end_col = orders.get(row as usize).and_then(|o| o.as_deref()).and_then(|o| crate::bidi::visual_spans(o, from.min(m.end.col)..m.end.col + 1).last().map(|r| r.end - 1)).unwrap_or(m.end.col);
                let x = rect.x + ((end_col + 2) as f32 * cw).min((rect.w - (shown.chars().count() as f32 + 1.0) * cw).max(0.0));
                let y = rect.y + row as f32 * ch;
                let r = Rect { x, y, w: (shown.chars().count() as f32 + 1.0) * cw, h: ch };
                frame.quads.push(Quad { rect: r, color: rgba(NOTE_BG, 1.0) });
                let span = Span { text: shown, color: rgb(0, 0, 0), bold: false, italic: false, alt_font: false, icon: false, prose: false, badge: false, toolbelt: false, fake_bold: false, ligatures: 0, mask: 0, font: 0 };
                frame.rows.push(TextRow { x: x + cw * 0.5, y, spans: vec![span], clip: rect, scale: 1.0, face });
            }
        }
    }
    // Right-to-left rows reverse the order the hovered cells were found in.
    hovered.sort_unstable();
    if let Some(h) = highlights {
        let mut i = 0;
        while i < hovered.len() {
            let (row, start) = hovered[i];
            let mut end = start;
            while i + 1 < hovered.len() && hovered[i + 1] == (row, end + 1) {
                i += 1;
                end += 1;
            }
            let r = Rect { x: rect.x + start as f32 * cw, y: underline_y(row), w: (end - start + 1) as f32 * cw, h: 1.0 };
            frame.overlay.push(Quad { rect: r, color: rgba(h.link, 1.0) });
            i += 1;
        }
    }

    if let Some((row, col)) = cursor.filter(|_| key_cursor) {
        // Drawn over the cell, as iTerm2 draws its key image: light on a dark background, dark
        // on a light one (`key-light`, `key-dark`), sized to the cell (the icon font's em is
        // about the line height over 1.2).
        let r = Rect { x: rect.x + col as f32 * cw, y: rect.y + row as f32 * ch, w: cw, h: ch };
        let bg = grid.get(row).and_then(|l| l.get(col)).map_or(palette.bg, |c| c.bg);
        let scale = (cw * 1.2 / ch.max(1.0)).min(1.0);
        let span = Span { text: crate::pwm_prompt::KEY_GLYPH.to_string(), color: match_text(bg), bold: false, italic: false, alt_font: false, icon: true, prose: false, badge: false, toolbelt: false, fake_bold: false, ligatures: 0, mask: 0, font: 0 };
        frame.rows.push(TextRow { x: r.x, y: r.y + (ch - ch * scale) / 2.0, spans: vec![span], clip: r, scale, face });
    } else if let Some((row, col)) = cursor {
        let origin = (rect.x, rect.y);
        let r = box_rect(&look.geometry, origin, cell, row, col, wide_cursor);
        let mut solid = cursor_rect(&look.geometry, content.cursor.shape, origin, cell, row, col, wide_cursor);
        // A sliding bar or underline is drawn at its slid place, with its shadow and outline.
        solid.x += look.offset.0;
        solid.y += look.offset.1;
        let color = rgba(palette.cursor, look.alpha);
        match content.cursor.shape {
            CursorShape::Beam | CursorShape::Underline => {
                frame.overlay.push(Quad { rect: solid, color });
                // The shadow follows the cursor and fades with it; the outline is a second
                // pass over the text (`iTermTextDrawingHelper.m:556-565`).
                if let Some((r, mut c)) = look.shadow.then(|| crate::cursor_look::shadow(content.cursor.shape, solid, palette.cursor, look.geometry.scale)).flatten() {
                    c[3] *= look.alpha;
                    frame.overlay.push(Quad { rect: r, color: c });
                }
                if look.outline {
                    for (r, mut c) in crate::cursor_look::outline(content.cursor.shape, solid, palette.bg, look.geometry.scale) {
                        c[3] *= look.alpha;
                        frame.overlay.push(Quad { rect: r, color: c });
                    }
                }
            }
            _ if !block_cursor => {
                for edge in [
                    Rect { h: 1.0, ..r },
                    Rect { y: r.y + r.h - 1.0, h: 1.0, ..r },
                    Rect { w: 1.0, ..r },
                    Rect { x: r.x + r.w - 1.0, w: 1.0, ..r },
                ] {
                    frame.overlay.push(Quad { rect: edge, color: rgba(palette.cursor, 1.0) });
                }
            }
            _ => {}
        }
    }

    // A smear (`CursorSmearView`) is the cursor color, drawn over the cell, under the dim.
    if let Some(poly) = look.smear.filter(|_| !key_cursor) {
        fill_polygon(&mut frame.overlay, &poly, rgba(palette.cursor, 1.0));
    }

    // The IME composition: each run on the default background (cut out of the grid's text) in
    // the default text color with an underline, then the caret in its color.
    if let Some(d) = ime {
        for piece in d.pieces.iter().filter(|p| p.row < lines) {
            // On a right-to-left row the cells the composition covers are drawn where the row's
            // order puts them: the piece starts at the first of their visual pieces.
            let col = visual_start(orders.get(piece.row).and_then(|o| o.as_deref()), piece.col, piece.cells);
            let r = Rect { x: rect.x + col as f32 * cw, y: rect.y + piece.row as f32 * ch, w: piece.cells as f32 * cw, h: ch };
            cut_out(frame, r, ch);
            frame.popups.push(Quad { rect: r, color: rgba(palette.bg, 1.0) });
            let span = Span { text: piece.text.clone(), color: palette.fg, bold: false, italic: false, alt_font: false, icon: false, prose: false, badge: false, toolbelt: false, fake_bold: false, ligatures: 0, mask: 0, font: 0 };
            frame.rows.push(TextRow { x: r.x, y: r.y, spans: vec![span], clip: Rect { w: rect.x + rect.w - r.x, ..r }, scale: 1.0, face });
            let line = look.geometry.scale.max(1.0).round();
            frame.overlay.push(Quad { rect: Rect { y: r.y + ch - line, h: line, ..r }, color: rgba(palette.fg, 1.0) });
        }
        if d.caret.0 < lines {
            let caret = (d.caret.0, visual_col(d.caret.0, d.caret.1));
            frame.overlay.push(Quad { rect: crate::ime::caret_rect(caret, (rect.x, rect.y), cell, look.geometry.base_cell.1, cols, look.geometry.scale), color: rgba(d.caret_color, 1.0) });
        }
    }

    // Highlights carry their own dim (the app's gray quad); the preview paths pass none and keep
    // today's fade until 13y.
    if !focused && highlights.is_none() {
        frame.overlay.push(Quad { rect, color: rgba(palette.bg, DIM_ALPHA) });
    }
}

/// Draws the tab bar across `area`. Returns each tab's rectangle, for clicks.
/// iTerm2's mark colors (iTermTextDrawingHelper), in sRGB.
pub fn mark_rgba(c: crate::marks::MarkColor) -> [f32; 4] {
    use crate::marks::MarkColor::*;
    let (r, g, b) = match c {
        Success => (0.538, 0.757, 1.0),
        Other => (0.857, 0.847, 0.426),
        Error => (0.987, 0.448, 0.426),
    };
    [r, g, b, 1.0]
}

/// The color of a mark's indicator (`TDH:1305-1346`): with "Use Theme Mark Colors" the ANSI
/// blue, yellow and red; otherwise iTerm2's fixed colors (`mark_rgba`).
pub fn mark_color(c: crate::marks::MarkColor, palette: &crate::palette::Palette, theme: bool) -> Rgb {
    use crate::marks::MarkColor::*;
    if theme {
        return palette.ansi[match c {
            Success => 4,
            Other => 3,
            Error => 1,
        }];
    }
    let [r, g, b, _] = mark_rgba(c);
    let u = |v: f32| (v * 255.0).round() as u8;
    Rgb { r: u(r), g: u(g), b: u(b) }
}

/// A line-style mark's line color (`TDH:1448-1456`): the mark color blended 50% with the
/// default background.
pub fn line_mark_color(mark: Rgb, bg: Rgb) -> Rgb {
    let mix = |m: u8, b: u8| ((m as f32 + b as f32) / 2.0).round() as u8;
    rgb(mix(mark.r, bg.r), mix(mark.g, bg.g), mix(mark.b, bg.b))
}

/// A line-style mark (`TDH:1380-1414`): a 1 px line across the pane at the row's top minus
/// half a cell, stopping 15 cells short of the right when the mark has a command.
pub fn line_mark(frame: &mut Frame, rect: Rect, row: usize, cell: (f32, f32), color: Rgb, has_command: bool, cols: usize) {
    let y = (rect.y + (row as f32 - 0.5) * cell.1).round();
    if y < rect.y {
        return;
    }
    let cells = if has_command { cols.saturating_sub(LINE_MARK_RIGHT_INSET) } else { cols };
    frame.overlay.push(Quad { rect: Rect { x: rect.x, y, w: cells as f32 * cell.0, h: 1.0 }, color: rgba(color, 1.0) });
}

/// A pane's scrollbar, over the text and the pane's backdrop margins (it holds no text).
/// The legacy style is an opaque track with a rounded knob when there is history; the
/// overlay style is a translucent thumb, 7 points wide, widening to the track's width
/// under the pointer. Gray, light or dark with the window.
pub fn scrollbar(frame: &mut Frame, g: &crate::scrollbar::Geometry, style: crate::scrollbar::Style, dark: bool, hover: bool) {
    use crate::scrollbar::{LEGACY_WIDTH, OVERLAY_WIDTH, Style};
    let gray = |v: f32, a: f32| [v, v, v, a];
    match style {
        Style::Legacy => {
            let scale = g.track.w / LEGACY_WIDTH;
            frame.scrollbars.push(Quad { rect: g.track, color: gray(if dark { 0.13 } else { 0.96 }, 1.0) });
            let edge = Rect { w: scale.max(1.0), ..g.track };
            frame.scrollbars.push(Quad { rect: edge, color: gray(if dark { 0.24 } else { 0.86 }, 1.0) });
            if g.thumb.h < g.track.h {
                let inset = 3.0 * scale;
                let knob = Rect { x: g.thumb.x + inset, y: g.thumb.y + 2.0 * scale, w: g.thumb.w - 2.0 * inset, h: (g.thumb.h - 4.0 * scale).max(1.0) };
                let v = match (dark, hover) {
                    (false, false) => 0.62,
                    (false, true) => 0.48,
                    (true, false) => 0.42,
                    (true, true) => 0.58,
                };
                rounded_box(&mut frame.scrollbars, knob, knob.w / 2.0, gray(v, 1.0));
            }
        }
        Style::Overlay => {
            let scale = g.track.w / OVERLAY_WIDTH;
            let w = if hover { g.track.w } else { 7.0 * scale };
            let thumb = Rect { x: g.track.x + g.track.w - w, w, ..g.thumb };
            let (v, a) = (if dark { 1.0 } else { 0.0 }, if hover { 0.65 } else { 0.45 });
            rounded_box(&mut frame.scrollbars, thumb, w / 2.0, gray(v, a));
        }
    }
}

/// Marks and search results along the scrollbar (`MM:85-125`): per non-empty series, each
/// tick a 1 px outline around a fill inset by 1, at `MINIMAP_ALPHA`. `track` is the minimap
/// rectangle (`Geometry::minimap`). Series draw in the order given.
pub fn scrollbar_ticks(frame: &mut Frame, track: Rect, series: &[(crate::scrollbar::Series, Vec<usize>)], total: usize, scale: f32) {
    use crate::scrollbar::{MINIMAP_ALPHA, series_colors, ticks};
    for (kind, lines) in series {
        let (outline, fill) = series_colors(*kind);
        for t in ticks(track, lines, total, false, scale) {
            let c = rgba(outline, MINIMAP_ALPHA);
            // The outline is half a point stroked, so a point wide at each side: `scale` px. The
            // side edges stop at the top and bottom edges so no corner draws twice.
            let o = scale.max(1.0);
            for edge in [Rect { h: o, ..t }, Rect { y: t.y + t.h - o, h: o, ..t }, Rect { y: t.y + o, h: (t.h - 2.0 * o).max(0.0), w: o, ..t }, Rect { x: t.x + t.w - o, y: t.y + o, h: (t.h - 2.0 * o).max(0.0), w: o, ..t }] {
                frame.scrollbars.push(Quad { rect: edge, color: c });
            }
            let inner = Rect { x: t.x + o, y: t.y + o, w: (t.w - 2.0 * o).max(0.0), h: (t.h - 2.0 * o).max(0.0) };
            frame.scrollbars.push(Quad { rect: inner, color: rgba(fill, MINIMAP_ALPHA) });
        }
    }
}

/// `iTermTextDrawingHelperLineStyleMarkRightInsetCells`: the cells a line-style mark leaves
/// free at the right for a command's buttons.
const LINE_MARK_RIGHT_INSET: usize = 15;

/// True when a line-style mark on `row` is not drawn: inside the selected command's rows
/// (first, last) or two rows after `last`, as iTerm2's `NSMaxRange + 1` (`TDH:1387-1394`).
pub fn line_mark_hidden(row: usize, region: Option<(usize, usize)>) -> bool {
    region.is_some_and(|(first, last)| (first..=last).contains(&row) || row == last + 2)
}

/// Mark indicators: a bar in the left padding beside each marked row on screen, in the
/// mark's color (`mark_color`).
pub fn mark_bars(frame: &mut Frame, marks: &[(usize, crate::marks::MarkColor)], inner: Rect, cell_h: f32, pad: f32, palette: &crate::palette::Palette, theme: bool) {
    let w = (pad * 0.75).max(1.0);
    for (row, color) in marks {
        let rect = Rect { x: inner.x - pad, y: inner.y + *row as f32 * cell_h, w, h: cell_h };
        frame.overlay.push(Quad { rect, color: rgba(mark_color(*color, palette, theme), 1.0) });
    }
}

/// Block gutter bars: `(view row, folded, hovered)`, in the left padding; filled, an
/// outline when folded, red (ANSI 1) when hovered.
pub fn block_bars(frame: &mut Frame, bars: &[(usize, bool, bool)], inner: Rect, cell_h: f32, pad: f32, fg: Rgb, red: Rgb) {
    let w = (pad - 2.0).max(1.0);
    for &(row, folded, hover) in bars {
        let r = Rect { x: inner.x - pad + 1.0, y: inner.y + row as f32 * cell_h, w, h: cell_h };
        let color = rgba(if hover { red } else { fg }, 1.0);
        if folded {
            for edge in [
                Rect { h: 1.0, ..r },
                Rect { y: r.y + r.h - 1.0, h: 1.0, ..r },
                Rect { w: 1.0, ..r },
                Rect { x: r.x + r.w - 1.0, w: 1.0, ..r },
            ] {
                frame.overlay.push(Quad { rect: edge, color });
            }
        } else {
            frame.overlay.push(Quad { rect: r, color });
        }
    }
}

/// A hovered block's pill: one rounded bar behind its buttons, a copy icon (two squares),
/// and a fold icon (bars drawn closer together, or apart when folded).
pub fn block_pill(frame: &mut Frame, buttons: &[(Rect, crate::blocks::PillButton)], folded: bool, bg: Rgb, fg: Rgb) {
    let (Some(first), Some(last)) = (buttons.iter().map(|b| b.0.x).reduce(f32::min), buttons.iter().map(|b| b.0.x + b.0.w).reduce(f32::max)) else { return };
    let r = buttons[0].0;
    let pill = Rect { x: first - 6.0, y: r.y, w: last - first + 12.0, h: r.h };
    frame.overlay.push(Quad { rect: pill, color: rgba(bg, 0.95) });
    for edge in [Rect { h: 1.0, ..pill }, Rect { y: pill.y + pill.h - 1.0, h: 1.0, ..pill }, Rect { w: 1.0, ..pill }, Rect { x: pill.x + pill.w - 1.0, w: 1.0, ..pill }] {
        frame.overlay.push(Quad { rect: edge, color: rgba(fg, 0.3) });
    }
    let c = rgba(fg, 1.0);
    for (b, kind) in buttons {
        let s = (b.h * 0.7).min(b.w * 0.7);
        let (x, y) = (b.x + (b.w - s) / 2.0, b.y + (b.h - s) / 2.0);
        match kind {
            crate::blocks::PillButton::Copy => {
                let q = s * 0.7;
                for (dx, dy) in [(0.0, 0.0), (s - q, s - q)] {
                    let sq = Rect { x: x + dx, y: y + dy, w: q, h: q };
                    for e in [Rect { h: 1.0, ..sq }, Rect { y: sq.y + q - 1.0, h: 1.0, ..sq }, Rect { w: 1.0, ..sq }, Rect { x: sq.x + q - 1.0, w: 1.0, ..sq }] {
                        frame.overlay.push(Quad { rect: e, color: c });
                    }
                }
            }
            crate::blocks::PillButton::Fold => {
                let gap = if folded { s * 0.45 } else { s * 0.15 };
                let mid = y + s / 2.0;
                frame.overlay.push(Quad { rect: Rect { x, y: mid - gap - 1.0, w: s, h: 1.5 }, color: c });
                frame.overlay.push(Quad { rect: Rect { x, y: mid + gap, w: s, h: 1.5 }, color: c });
            }
        }
    }
}

/// A porthole's look: colors, the language button's text, Wide, and the sideways scroll.
pub struct PortholeChrome<'a> {
    pub fg: Rgb,
    pub bg: Rgb,
    pub language: &'a str,
    pub wide: bool,
    pub scroll_x: f32,
}

/// Where a porthole's controls are, for clicks.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PortholeControls {
    pub close: Rect,
    pub language: Rect,
    pub wide: Rect,
}

fn cut(r: Rect, clip: Rect) -> Option<Rect> {
    let x0 = r.x.max(clip.x);
    let y0 = r.y.max(clip.y);
    let x1 = (r.x + r.w).min(clip.x + clip.w);
    let y1 = (r.y + r.h).min(clip.y + clip.h);
    (x1 > x0 && y1 > y0).then_some(Rect { x: x0, y: y0, w: x1 - x0, h: y1 - y0 })
}

/// `a` moved `t` of the way toward `b`: `b` at alpha `t` over `a` (text has no alpha of its
/// own; the one blend for scene, the status bar and the composer).
pub fn blend(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    rgb(m(a.r, b.r), m(a.g, b.g), m(a.b, b.b))
}

/// Pixels between a porthole's frame and its text.
pub const PORTHOLE_MARGIN: f32 = 8.0;

/// The lines of a laid-out document, cut to `area`: the `marks` (selection and find rectangles
/// from the text's top left, with their color and alpha) under the text, then each line's code
/// band, rule, underlines and runs. `origin` is where the text's top left sits; `band` is the
/// left edge and width a code band and a rule are drawn in (a rule stops `PORTHOLE_MARGIN` short
/// of the band's right edge); `back` and `fg` are the document's background and text colors.
pub fn doc_lines(frame: &mut Frame, origin: (f32, f32), lines: &[crate::porthole::render::VisualLine], marks: &[(Rect, Rgb, f32)], area: Rect, band: (f32, f32), back: Rgb, fg: Rgb) {
    let (ox, oy) = origin;
    // Selection and find marks, from the text's top left, under the text.
    for (m, color, alpha) in marks {
        if let Some(r) = cut(Rect { x: ox + m.x, y: oy + m.y, ..*m }, area) {
            frame.quads.push(Quad { rect: r, color: rgba(*color, *alpha) });
        }
    }
    for l in lines {
        let y = oy + l.y;
        if l.code_bg {
            if let Some(r) = cut(Rect { x: band.0, y, w: band.1, h: l.h }, area) {
                frame.quads.push(Quad { rect: r, color: rgba(blend(back, fg, 0.08), 1.0) });
            }
        }
        if l.rule {
            if let Some(r) = cut(Rect { x: ox + l.x, y: y + l.h / 2.0, w: band.1 - PORTHOLE_MARGIN - l.x, h: 1.0 }, area) {
                frame.quads.push(Quad { rect: r, color: rgba(fg, 0.5) });
            }
        }
        for (run, (sx, sw)) in l.runs.iter().zip(&l.spans) {
            if run.underline {
                if let Some(r) = cut(Rect { x: ox + l.x + sx, y: y + l.h - 2.0, w: *sw, h: 1.0 }, area) {
                    frame.quads.push(Quad { rect: r, color: rgba(run.color, 1.0) });
                }
            }
        }
        let spans = l
            .runs
            .iter()
            .map(|r| Span { text: r.text.clone(), color: r.color, bold: r.bold, italic: r.italic, alt_font: false, icon: false, prose: r.face == crate::porthole::render::Face::Prose, badge: false, toolbelt: false, fake_bold: false, ligatures: 0, mask: 0, font: 0 })
            .collect();
        frame.rows.push(TextRow { x: ox + l.x, y, spans, clip: area, scale: l.scale, face: crate::render::DEFAULT_FACE });
    }
}

/// A porthole over its rows (iTerm2's chrome): the background moved 20% toward grey, a 1 px
/// border in the text color at 50%, the close button, the language button, and Wide at the
/// top right, then the laid-out lines (`lines` holds the ones in view), all cut to `clip`.
pub fn porthole(frame: &mut Frame, rect: Rect, clip: Rect, lines: &[crate::porthole::render::VisualLine], marks: &[(Rect, Rgb, f32)], chrome: &PortholeChrome, cell: (f32, f32)) -> PortholeControls {
    let (fg, bg) = (chrome.fg, chrome.bg);
    let back = blend(bg, rgb(128, 128, 128), 0.2);
    if let Some(r) = cut(rect, clip) {
        frame.quads.push(Quad { rect: r, color: rgba(back, 1.0) });
    }
    for edge in [Rect { h: 1.0, ..rect }, Rect { y: rect.y + rect.h - 1.0, h: 1.0, ..rect }, Rect { w: 1.0, ..rect }, Rect { x: rect.x + rect.w - 1.0, w: 1.0, ..rect }] {
        if let Some(r) = cut(edge, clip) {
            frame.quads.push(Quad { rect: r, color: rgba(fg, 0.5) });
        }
    }
    let s = cell.1;
    let close = Rect { x: rect.x + rect.w - 2.0 - s, y: rect.y + 2.0, w: s, h: s };
    let lw = (chrome.language.chars().count() as f32 + 2.0) * cell.0;
    let language = Rect { x: close.x - 4.0 - lw, y: close.y, w: lw, h: s };
    let ww = 8.0 * cell.0;
    let wide = Rect { x: language.x - 2.0 - ww, y: close.y, w: ww, h: s };
    let text = |t: String, color: Rgb, icon: bool| Span { text: t, color, bold: false, italic: false, alt_font: false, icon, prose: false, badge: false, toolbelt: false, fake_bold: false, ligatures: 0, mask: 0, font: 0 };
    if let Some(c) = cut(close, clip) {
        frame.rows.push(TextRow { x: close.x + (s - cell.0) / 2.0, y: close.y, spans: vec![text("\u{E711}".into(), blend(bg, fg, 0.5), true)], clip: c, scale: 1.0, face: crate::render::DEFAULT_FACE });
    }
    if let Some(c) = cut(language, clip) {
        frame.quads.push(Quad { rect: c, color: rgba(blend(back, fg, 0.12), 1.0) });
        frame.rows.push(TextRow { x: language.x + cell.0, y: language.y, spans: vec![text(chrome.language.to_string(), fg, false)], clip: c, scale: 1.0, face: crate::render::DEFAULT_FACE });
    }
    if let Some(c) = cut(wide, clip) {
        let label = if chrome.wide { "[x] Wide" } else { "[ ] Wide" };
        frame.rows.push(TextRow { x: wide.x, y: wide.y, spans: vec![text(label.into(), fg, false)], clip: c, scale: 1.0, face: crate::render::DEFAULT_FACE });
    }
    let inner = Rect { x: rect.x + 4.0, y: rect.y + 4.0 + s, w: rect.w - 8.0, h: rect.h - 8.0 - s };
    let Some(area) = cut(inner, clip) else { return PortholeControls { close, language, wide } };
    let ox = rect.x + PORTHOLE_MARGIN - chrome.scroll_x;
    let oy = rect.y + PORTHOLE_MARGIN + s;
    doc_lines(frame, (ox, oy), lines, marks, area, (inner.x, inner.w), back, fg);
    PortholeControls { close, language, wide }
}

/// A badge behind the text: four copies in the background color make its outline, then the
/// fill in the badge color at its alpha over the background.
pub fn badge(frame: &mut Frame, pane: Rect, l: &crate::badge::BadgeLayout, color: Rgb, alpha: f32, bg: Rgb, font_px: f32) {
    let fill = blend(bg, color, alpha);
    let scale = l.size / font_px.max(1.0);
    let line_h = l.size * crate::badge::LINE;
    for (dx, dy, c) in [(-1.0, 0.0, bg), (1.0, 0.0, bg), (0.0, -1.0, bg), (0.0, 1.0, bg), (0.0, 0.0, fill)] {
        for (i, (text, w)) in l.lines.iter().zip(&l.widths).enumerate() {
            let x = pane.x + l.rect.x + l.rect.w - w + dx;
            let y = pane.y + l.rect.y + i as f32 * line_h + dy;
            let span = Span { text: text.clone(), color: c, bold: true, italic: false, alt_font: false, icon: false, prose: false, badge: true, toolbelt: false, fake_bold: false, ligatures: 0, mask: 0, font: 0 };
            frame.rows.push(TextRow { x, y, spans: vec![span], clip: pane, scale, face: crate::render::DEFAULT_FACE });
        }
    }
}

/// How an inline button looks.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ButtonFace {
    Copy,
    Icon(char),
    /// A custom button whose icon has no glyph: nothing is drawn.
    Blank,
}

/// OSC 1337 inline buttons: a faint rounded box with the icon, at 40% when inert.
pub fn inline_buttons(frame: &mut Frame, buttons: &[(Rect, ButtonFace, bool)], bg: Rgb, fg: Rgb) {
    for (r, face, valid) in buttons {
        let a = if *valid { 1.0 } else { 0.4 };
        let mix = |t: f32| rgb((bg.r as f32 + (fg.r as f32 - bg.r as f32) * t).round() as u8, (bg.g as f32 + (fg.g as f32 - bg.g as f32) * t).round() as u8, (bg.b as f32 + (fg.b as f32 - bg.b as f32) * t).round() as u8);
        match face {
            ButtonFace::Blank => {}
            ButtonFace::Copy => {
                frame.quads.push(Quad { rect: *r, color: rgba(mix(0.15 * a), 1.0) });
                let s = (r.h * 0.6).min(r.w * 0.6);
                let q = s * 0.7;
                let (x, y) = (r.x + (r.w - s) / 2.0, r.y + (r.h - s) / 2.0);
                for (dx, dy) in [(0.0, 0.0), (s - q, s - q)] {
                    let sq = Rect { x: x + dx, y: y + dy, w: q, h: q };
                    for e in [Rect { h: 1.0, ..sq }, Rect { y: sq.y + q - 1.0, h: 1.0, ..sq }, Rect { w: 1.0, ..sq }, Rect { x: sq.x + q - 1.0, w: 1.0, ..sq }] {
                        frame.quads.push(Quad { rect: e, color: rgba(mix(a), 1.0) });
                    }
                }
            }
            ButtonFace::Icon(c) => {
                frame.quads.push(Quad { rect: *r, color: rgba(mix(0.15 * a), 1.0) });
                let span = Span { text: c.to_string(), color: mix(a), bold: false, italic: false, alt_font: false, icon: true, prose: false, badge: false, toolbelt: false, fake_bold: false, ligatures: 0, mask: 0, font: 0 };
                frame.rows.push(TextRow { x: r.x + r.w / 4.0, y: r.y, spans: vec![span], clip: *r, scale: 1.0, face: crate::render::DEFAULT_FACE });
            }
        }
    }
}

/// One tab's look: its title, its subtitle line (the OSC 21337 status joined in), the status
/// dot, the tab color a program chose, the indicator, the spinner and the tab's number.
#[derive(Clone, Debug, Default)]
pub struct TabLook {
    pub title: String,
    pub subtitle: String,
    pub dot: Option<Rgb>,
    pub color: Option<Rgb>,
    /// The one indicator a tab shows (when no status dot does).
    pub icon: Option<TabIcon>,
    /// The activity spinner shows (`activity_ui::tab_processing`).
    pub processing: bool,
    /// The tab's number, from 1.
    pub number: usize,
    /// The close button is held down (`closeButtonPressed`).
    pub close_pressed: bool,
    /// Dragged along the bar (`app::tab_drag_ui`): drawn on its own background, over the
    /// tabs it passes.
    pub dragged: bool,
    /// Pinned (`Tab.pinned`): the pin at the tab's left, and on a horizontal bar only the
    /// title's first character (`YS:915-1027`).
    pub pinned: bool,
    /// The tab's group: its name and color (`Tab.group`).
    pub group: Option<(String, Rgb)>,
    /// Hidden in a collapsed group: no cell (`tab_layout::TabInput::hidden`).
    pub hidden: bool,
    /// The session's icon (`PTYSession tabGraphic`), drawn 16 by 16 points at the tab's left.
    pub icon_image: Option<TabImage>,
}

/// A tab's icon picture (`app::tab_groups_ui::tab_icon_for`): a decoded picture (a bundled icon
/// already tinted, or a custom image; `key` names it for the texture cache), or the letter icon
/// (`GS imageForLetter:`), the letter knocked out of a circle of the color.
#[derive(Clone, Debug, PartialEq)]
pub enum TabImage {
    Picture { key: u64, pixels: std::sync::Arc<Vec<u8>>, size: (u32, u32) },
    Letter { letter: String, color: Rgb },
}

/// The letter icon's letter: bold at this size in points (`GS:442`).
pub const LETTER_PT: f32 = 12.0;
/// The letter icon's circle sits this far inside the 16-point box (`GS:455`).
const LETTER_INSET: f32 = 1.0;

/// A group run's name block: the padding at each side of the name (`kPSMSquaredGroupNamePad`,
/// `YS:1352`).
pub const GROUP_NAME_PAD: f32 = 10.0;
/// The colored area between the name and the first tab (`kPSMSquaredGroupBArea`).
pub const GROUP_B_AREA: f32 = 6.0;
/// A collapsed chip's room for its chevron (`kPSMSquaredCollapsedChevron`).
pub const GROUP_CHEVRON: f32 = 14.0;
/// The group name's size in points: bold at the system font size (`YS squaredGroupNameFont`,
/// `NSFont systemFontSize`).
pub const GROUP_NAME_PT: f32 = 13.0;
/// How far the run's outline sits outside the tabs (`kPSMSquaredGroupOutset`).
const GROUP_OUTSET: f32 = 1.0;
/// A collapsed group a dragged tab would join is framed this many points thick, this far
/// outside its block, in the accent color (`YS:1497-1504`, `controlAccentColor`).
const JOIN_FRAME: f32 = 2.0;
const JOIN_FRAME_OUTSET: f32 = 1.0;
/// Segoe Fluent Icons "ChevronDown": the collapsed chip's chevron, drawn at this size.
const GROUP_CHEVRON_GLYPH: char = '\u{E70D}';
const GROUP_CHEVRON_PT: f32 = 8.0;
/// A group with no color draws in `systemBlueColor` (`YS:1454`).
const GROUP_DEFAULT: Rgb = rgb(0, 122, 255);

/// The collapsed chip's label (`YS collapsedLabelForName:count:`): "Name  N", or "N" when the
/// group has no name.
pub fn collapsed_label(name: &str, count: usize) -> String {
    if name.is_empty() {
        count.to_string()
    } else {
        format!("{name}  {count}")
    }
}

/// A chip's width for its label's width `name_w` (`YS tabGroupChipCellWidthForName:`,
/// `tabGroupCollapsedChipCellWidthForName:memberCount:`): the name, padded each side, then the
/// colored area, or collapsed the chevron's room.
pub fn chip_width(name_w: f32, collapsed: bool) -> f32 {
    name_w.ceil() + GROUP_NAME_PAD * 2.0 + if collapsed { GROUP_CHEVRON } else { GROUP_B_AREA }
}

/// White or black text on `c` (`NSColor psmContrastingTextColor`, `NSColor+PSM.m:49-58`):
/// Rec.601 luma below 0.55 takes white.
pub fn contrasting(c: Rgb) -> Rgb {
    let luma = (0.299 * c.r as f32 + 0.587 * c.g as f32 + 0.114 * c.b as f32) / 255.0;
    if luma < 0.55 { rgb(0xff, 0xff, 0xff) } else { rgb(0, 0, 0) }
}

/// A pinned tab's mark (Segoe Fluent Icons "Pin"; iTerm2's SF Symbol "pin.fill").
pub const PIN_GLYPH: char = '\u{E718}';
/// `PinnedTabWidth`'s default in points (`AS:321`); the drawn width comes from the key.
pub const PINNED_TAB_WIDTH: f32 = 64.0;
/// The pin's alpha in a key window and otherwise (`YS:922`).
const PIN_ALPHA: (f32, f32) = (0.6, 0.3);

/// A tab's indicator, in iTerm2's order after the status dot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabIcon {
    Dead,
    Bell,
    NewOutput,
    Idle,
}

/// The indicator a tab shows: none when a status dot shows, else the first of dead, bell,
/// new output, and idle.
pub fn tab_icon(dot: bool, dead: bool, bell: bool, new_output: bool, idle: bool) -> Option<TabIcon> {
    if dot {
        return None;
    }
    [(dead, TabIcon::Dead), (bell, TabIcon::Bell), (new_output, TabIcon::NewOutput), (idle, TabIcon::Idle)].into_iter().find(|(on, _)| *on).map(|(_, i)| i)
}

/// The top-right indicator for a running coprocess (Segoe Fluent Icons "Switch").
pub const COPROCESS_GLYPH: char = '\u{E8AB}';
/// Broadcast input: this session sends (and receives), or only receives.
pub const BROADCAST_GLYPH: char = '\u{EC05}';
pub const RECEIVE_ONLY_GLYPH: char = '\u{E701}';

/// The top-right glyphs in iTerm2's order (right to left): broadcast or receive-only,
/// then the coprocess.
pub fn indicator_glyphs(sends: bool, receives: bool, coprocess: bool, linked: bool, streaming: bool) -> Vec<char> {
    indicators(sends, receives, coprocess, linked, streaming).into_iter().map(|(g, _)| g).collect()
}

/// What a press on an indicator glyph does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IndicatorKind {
    Other,
    AiLinked,
    AiStreaming,
}

/// The glyphs with their kinds, in the same order.
pub fn indicators(sends: bool, receives: bool, coprocess: bool, linked: bool, streaming: bool) -> Vec<(char, IndicatorKind)> {
    let mut out = Vec::new();
    if sends {
        out.push((BROADCAST_GLYPH, IndicatorKind::Other));
    } else if receives {
        out.push((RECEIVE_ONLY_GLYPH, IndicatorKind::Other));
    }
    if coprocess {
        out.push((COPROCESS_GLYPH, IndicatorKind::Other));
    }
    if linked {
        out.push((AI_LINKED_GLYPH, IndicatorKind::AiLinked));
    }
    if streaming {
        out.push((AI_STREAMING_GLYPH, IndicatorKind::AiStreaming));
    }
    out
}

/// The AI chat indicators: a session linked to a chat, and one whose finished commands stream to
/// a chat (`IH:96-117`; iTerm2 gives the streaming one the receive-only symbol).
pub const AI_LINKED_GLYPH: char = '\u{E8BD}';
pub const AI_STREAMING_GLYPH: char = RECEIVE_ONLY_GLYPH;

/// iTerm2's broadcast stripes as a tile to repeat: red at alpha 64 in "/" bands at 45
/// degrees, 40 px on and 40 px off along each row (80 px tile), scaled by `scale`.
pub fn stripe_tile(scale: f32) -> (u32, Vec<u8>) {
    let n = (80.0 * scale.max(0.5)).round() as u32;
    let half = n / 2;
    let mut px = vec![0u8; (n * n * 4) as usize];
    for y in 0..n {
        for x in 0..n {
            if (x + y) % n < half {
                let i = ((y * n + x) * 4) as usize;
                px[i..i + 4].copy_from_slice(&[255, 0, 0, 64]);
            }
        }
    }
    (n, px)
}

/// A pane's top-right indicators, right to left from the corner.
/// Answers each glyph's rectangle, for the ones a press acts on.
pub fn top_right_indicators(frame: &mut Frame, pane: Rect, glyphs: &[char], color: Rgb, cell: (f32, f32)) -> Vec<Rect> {
    let step = cell.0 * 2.5;
    let mut hits = Vec::new();
    for (i, g) in glyphs.iter().enumerate() {
        let x = pane.x + pane.w - (i as f32 + 1.0) * step;
        let span = Span { text: g.to_string(), color, bold: false, italic: false, alt_font: false, icon: true, prose: false, badge: false, toolbelt: false, fake_bold: false, ligatures: 0, mask: 0, font: 0 };
        frame.rows.push(TextRow { x, y: pane.y + 4.0, spans: vec![span], clip: pane, scale: 1.0, face: crate::render::DEFAULT_FACE });
        hits.push(Rect { x, y: pane.y + 4.0, w: cell.0 * 2.0, h: cell.1 });
    }
    hits
}

/// Segoe Fluent Icons glyphs for the indicators.
fn tab_icon_glyph(i: TabIcon) -> char {
    match i {
        TabIcon::Dead => '\u{E711}',
        TabIcon::Bell => '\u{EA8F}',
        TabIcon::NewOutput => '\u{E91F}',
        TabIcon::Idle => '\u{E708}',
    }
}

/// Segoe Fluent Icons "ChromeClose": a tab's close button.
pub const TAB_CLOSE_GLYPH: char = '\u{E8BB}';
/// How far a held close button's shade goes from the tab's color toward its text color.
const TAB_CLOSE_PRESSED: f32 = 0.3;
/// Segoe Fluent Icons "Add": the new tab button.
pub const TAB_ADD_GLYPH: char = '\u{E710}';
/// Segoe Fluent Icons "ChevronRight", drawn twice for the overflow button (iTerm2's
/// `overflowImage` is a right-pointing double chevron; the icon font has no double one).
pub const TAB_OVERFLOW_GLYPH: char = '\u{E76C}';
/// How far apart the overflow button's two chevrons are, in ems.
const TAB_OVERFLOW_GAP: f32 = 0.25;
/// Icon glyphs in a tab are drawn at this size in points (in the 16-point icon box).
const TAB_GLYPH_PT: f32 = 12.0;
/// The status dot's size in points.
const TAB_DOT_PT: f32 = 8.0;

/// Draws a laid-out tab bar (`tab_layout::build`) in `area`, `scale` pixels a point.
/// `font_scale` is a text row's scale for one point of type (the window scale over the terminal
/// font's pixel size) and `line` the terminal font's line height in pixels, so a label of `p`
/// points is a row of scale `p * font_scale`, `line * p * font_scale` pixels high.
/// `key` is the window being the key window of the active app (`windowIsMainAndAppIsActive`):
/// otherwise close buttons draw at half alpha. The tabs are cut to the room left of the "+"
/// pinned on a bar that scrolls (`tab_layout::tab_area_width`). `overflow_down`: the overflow
/// button's menu is open, so it draws pressed. The group runs are drawn over the tabs.
#[allow(clippy::too_many_arguments)]
pub fn tab_bar(frame: &mut Frame, tabs: &[TabLook], active: usize, bar: &crate::tab_layout::Bar, area: Rect, scale: f32, font_scale: f32, line: f32, key: bool, overflow_down: bool, pal: &crate::tab_theme::TabPalette) {
    draw_bar(frame, tabs, active, bar, area, scale, font_scale, line, key, overflow_down, pal);
}

/// Draws a side tab bar (milestone 13d) laid out by `tab_layout::build_vertical` in `area`, as
/// `tab_bar` draws a horizontal one (the same arguments): one row per shown tab and per group
/// chip, each drawn as a tab is, with the overflow button in the bottom row. Answers each tab's
/// row in pixels in `tabs` order (a zero-size rect for a tab with no row or a row wholly
/// outside `area`) and each chip shown with its group id.
#[allow(clippy::too_many_arguments)]
pub fn tab_bar_vertical(frame: &mut Frame, tabs: &[TabLook], active: usize, bar: &crate::tab_layout::Bar, area: Rect, scale: f32, font_scale: f32, line: f32, key: bool, overflow_down: bool, pal: &crate::tab_theme::TabPalette) -> (Vec<Rect>, Vec<(Rect, String)>) {
    draw_bar(frame, tabs, active, bar, area, scale, font_scale, line, key, overflow_down, pal);
    let px = |r: Rect| Rect { x: area.x + r.x * scale, y: area.y + r.y * scale, w: r.w * scale, h: r.h * scale };
    let mut rects = vec![Rect { x: 0.0, y: 0.0, w: 0.0, h: 0.0 }; tabs.len()];
    for (i, _, r) in &bar.cells {
        let row = px(*r);
        if let (Some(slot), Some(_)) = (rects.get_mut(*i), cut(row, area)) {
            *slot = row;
        }
    }
    let chips = bar.chips.iter().filter_map(|(id, r)| cut(px(*r), area).map(|_| (px(*r), id.clone()))).collect();
    (rects, chips)
}

#[allow(clippy::too_many_arguments)]
fn draw_bar(frame: &mut Frame, tabs: &[TabLook], active: usize, bar: &crate::tab_layout::Bar, area: Rect, scale: f32, font_scale: f32, line: f32, key: bool, overflow_down: bool, pal: &crate::tab_theme::TabPalette) {
    use crate::tab_layout::{self, SUBTITLE_OFFSET, TITLE_OFFSET_WITH_SUBTITLE};
    use crate::tab_theme::over;
    frame.quads.push(Quad { rect: area, color: rgba(pal.bar, 1.0) });
    let px = |r: Rect| Rect { x: area.x + r.x * scale, y: area.y + r.y * scale, w: r.w * scale, h: r.h * scale };
    let tabs_area = if bar.vertical { area } else { Rect { w: (tab_layout::tab_area_width(bar, area.w / scale) * scale).min(area.w), ..area } };
    // The bar's room beyond the last tab, in the margin fill.
    let far = bar.cells.iter().map(|(_, _, r)| if bar.vertical { area.y + (r.y + r.h) * scale } else { area.x + (r.x + r.w) * scale }).fold(if bar.vertical { area.y } else { area.x }, f32::max);
    let margin = if bar.vertical { Rect { y: far, h: area.y + area.h - far, ..area } } else { Rect { x: far, w: area.x + area.w - far, ..area } };
    // Minimal's end inset with the selected tab in the overflow menu is the bar's color, the
    // selected tab's (`drawEndInset`, `MIN:437-451`); a side bar's is always the unselected one.
    let inset = if pal.minimal.is_some() && !bar.vertical && bar.hidden.contains(&active) { pal.selected } else { pal.margin };
    if margin.w > 0.0 && margin.h > 0.0 {
        fill_quad(frame, margin, inset);
    }
    let first_quad = frame.quads.len();
    let title_scale = bar.font * font_scale;
    let sub_scale = tab_layout::subtitle_size(bar.font) * font_scale;
    let (title_h, sub_h) = (line * title_scale, line * sub_scale);
    let turn = spinner_turn();
    let any_colored = tabs.iter().any(|t| t.color.is_some());
    // The selected tab's cell, effective fill and brightness (`tabColorBrightness`, `YOS:776-797`),
    // for what is drawn over the lines.
    let mut selected_deco: Option<(usize, Rect, Rgb, f32)> = None;
    let span = |text: String, color: Rgb, bold: bool, italic: bool, icon: bool| Span { text, color, bold, italic, alt_font: false, icon, prose: !icon, badge: false, toolbelt: false, fake_bold: false, ligatures: 0, mask: 0, font: 0 };
    for (k, (i, c, r)) in bar.cells.iter().enumerate() {
        let Some(tab) = tabs.get(*i) else { continue };
        let cell = px(*r);
        // The parts inside a tab are placed from its top.
        let at = |q: Rect| px(Rect { y: q.y + r.y, ..q });
        let clip = clip_rect(cell, tabs_area);
        // Scrolled out of view.
        if clip.w <= 0.0 || clip.h <= 0.0 {
            continue;
        }
        let selected = *i == active;
        let (tab_fill, mut text_fill) = if selected { (pal.selected, pal.text_selected) } else { (pal.unselected, pal.text_unselected) };
        let mut back = over(pal.bar, tab_fill);
        // With any colored tab, a colored tab is filled with its color (`YOS:611-636`; Minimal:
        // the color itself at an alpha, `MIN:937-946`) and every tab's text follows the
        // brightness of its effective fill (`YOS:466-509`).
        let colored = tab.color.map(|col| match &pal.minimal {
            Some(m) => crate::tab_theme::minimal_colored_fill(col, selected, key, m.deselected_colored_alpha),
            None => crate::tab_theme::Fill { color: crate::tab_theme::colored_fill(col, selected, key), alpha: 1.0 },
        });
        if let Some(fill) = colored {
            back = over(back, fill);
        }
        if any_colored {
            text_fill = match colored {
                Some(_) => crate::tab_theme::colored_text(back, selected, pal.colored_text_prominence),
                None => crate::tab_theme::text_for_brightness(crate::tab_theme::fill_brightness(tab_fill, pal.dark_controls), selected, pal.colored_text_prominence),
            };
        }
        if selected {
            let brightness = if colored.is_some() { crate::tab_theme::hsp(back) } else { crate::tab_theme::fill_brightness(tab_fill, pal.dark_controls) };
            selected_deco = Some((k, cell, back, brightness));
        }
        // A dragged tab covers the tabs it passes, so it is opaque; so is an opaque color.
        if tab.dragged || colored.is_some_and(|f| f.alpha >= 1.0) {
            frame.quads.push(Quad { rect: cell, color: rgba(back, 1.0) });
        } else {
            fill_quad(frame, cell, tab_fill);
            if let Some(fill) = colored {
                fill_quad(frame, cell, fill);
            }
        }
        let color = over(back, text_fill);
        // A glyph from the icon font centered in `box_`.
        let glyph = |frame: &mut Frame, ch: char, box_: Rect, color: Rgb| {
            let s = TAB_GLYPH_PT * font_scale;
            let em = TAB_GLYPH_PT * scale;
            let x = box_.x + ((box_.w - em) / 2.0).floor();
            let y = box_.y + ((box_.h - line * s) / 2.0).floor();
            frame.rows.push(TextRow { x, y, spans: vec![span(ch.to_string(), color, false, false, true)], clip, scale: s, face: crate::render::DEFAULT_FACE });
        };
        // `YS drawInteriorWithTabCell:`: the close button at its alpha, halved in a window that
        // is not key.
        if let Some(close) = c.close.filter(|_| c.close_alpha > 0.0) {
            let alpha = if key { c.close_alpha } else { c.close_alpha / 2.0 };
            let close = at(close);
            // Held down: iTerm2's darker "down" image, here a shade behind the glyph.
            if tab.close_pressed {
                frame.quads.push(Quad { rect: close, color: rgba(blend(back, color, TAB_CLOSE_PRESSED * alpha), 1.0) });
            }
            glyph(frame, TAB_CLOSE_GLYPH, close, blend(back, color, alpha));
        }
        // The pin at the cell's left margin, not under an icon (`YS:915-930`).
        if tab.pinned && tab.icon_image.is_none() {
            let alpha = if key { PIN_ALPHA.0 } else { PIN_ALPHA.1 };
            let em = TAB_GLYPH_PT * scale;
            glyph(frame, PIN_GLYPH, Rect { x: cell.x + tab_layout::CELL_MARGIN * scale, w: em, ..cell }, blend(back, color, alpha));
        }
        // The status dot is the tab's icon (`tabStatusDotImage`), else the indicator glyph.
        if let Some(ir) = c.icon.map(at) {
            if let Some(dot) = tab.dot {
                let d = (TAB_DOT_PT * scale).round();
                let rect = Rect { x: ir.x + ((ir.w - d) / 2.0).floor(), y: ir.y + ((ir.h - d) / 2.0).floor(), w: d, h: d };
                frame.quads.push(Quad { rect, color: rgba(dot, 1.0) });
            } else if let Some(icon) = tab.icon {
                glyph(frame, tab_icon_glyph(icon), ir, color);
            }
        }
        // The icon, over the close button's place and fading as the close button shows
        // (`YS:932-941`: alpha 1 less the close button's).
        if let (Some(g), Some(image)) = (c.graphic.map(at), tab.icon_image.as_ref()) {
            let close = if key { c.close_alpha } else { c.close_alpha / 2.0 };
            let letter_w = bar.texts.get(k).map_or(0.0, |t| t.letter_width);
            tab_graphic(frame, image, g, 1.0 - close, back, clip, scale, font_scale, line, letter_w);
        }
        if let Some(ind) = c.indicator.filter(|_| tab.processing) {
            spinner(frame, at(ind), turn, color, scale);
        }
        let Some(text) = bar.texts.get(k) else { continue };
        // `YS:1000`: a pinned tab with an icon shows no title.
        if tab.pinned && tab.icon_image.is_some() {
            continue;
        }
        // The number, centered in its box (`YS drawInteriorWithTabCell:...`).
        if let Some(cr) = c.counter.map(at).filter(|_| !text.counter.is_empty()) {
            let x = cr.x + ((cr.w - text.counter_width * scale) / 2.0).floor();
            let y = cr.y + ((cr.h - title_h) / 2.0).floor();
            frame.rows.push(TextRow { x, y, spans: vec![span(text.counter.clone(), color, false, false, false)], clip, scale: title_scale, face: crate::render::DEFAULT_FACE });
        }
        let subtitle = bar.subtitles.get(k).filter(|s| !s.is_empty());
        let offset = if subtitle.is_some() { TITLE_OFFSET_WITH_SUBTITLE * scale } else { 0.0 };
        // The title: centered in its label, left-aligned when truncated.
        let label = at(c.label);
        let width = text.title_width() * scale;
        let x = if c.truncate { label.x } else { label.x + (label.w - width) / 2.0 };
        let y = cell.y + ((cell.h - title_h) / 2.0).floor() + offset;
        let mut run_x = x;
        let font_px = bar.font * scale;
        for (run, w) in &text.runs {
            if run.underline {
                let under = (y + (title_h + font_px * 0.8) / 2.0).round();
                frame.quads.push(Quad { rect: Rect { x: run_x, y: under, w: w * scale, h: 1.0 }, color: rgba(color, 1.0) });
            }
            run_x += w * scale;
        }
        let spans: Vec<Span> = text.runs.iter().map(|(r, _)| span(r.text.clone(), color, r.bold, r.italic, false)).collect();
        if !spans.is_empty() {
            frame.rows.push(TextRow { x, y, spans, clip, scale: title_scale, face: crate::render::DEFAULT_FACE });
        }
        if let Some(sub) = subtitle {
            let sl = at(text.subtitle_label);
            let sw = text.subtitle_width * scale;
            let x = if text.subtitle_truncate { sl.x } else { sl.x + (sl.w - sw) / 2.0 };
            let y = cell.y + ((cell.h - sub_h) / 2.0).floor() + offset + title_h + SUBTITLE_OFFSET * scale;
            frame.rows.push(TextRow { x, y, spans: vec![span(sub.clone(), color, false, false, false)], clip, scale: sub_scale, face: crate::render::DEFAULT_FACE });
        }
    }
    // Minimal: the bar's room before the first tab is drawn as an unselected tab, or as the selected
    // one when the first tab is selected and the setting says so (`drawStartInset`, `MIN:439-453`).
    if let Some(m) = &pal.minimal {
        let first = bar.cells.iter().min_by(|a, b| (a.2.x + a.2.y).total_cmp(&(b.2.x + b.2.y)));
        if let Some((i, _, r)) = first {
            let gap = if bar.vertical { Rect { h: (px(*r).y - area.y).max(0.0), ..area } } else { Rect { w: (px(*r).x - area.x).max(0.0), ..area } };
            if gap.w > 0.0 && gap.h > 0.0 {
                let color = match selected_deco {
                    Some((_, _, back, _)) if *i == active && m.left_inset_is_first_tab => back,
                    _ => pal.margin.color,
                };
                frame.quads.push(Quad { rect: gap, color: rgba(color, 1.0) });
            }
        }
    }
    group_runs(frame, tabs, bar, area, tabs_area, scale, font_scale, line);
    // The tabs' quads (backgrounds, colors, dots, spinners, underlines, group runs) are cut like
    // their text.
    let mut k = first_quad;
    while k < frame.quads.len() {
        match cut(frame.quads[k].rect, tabs_area) {
            Some(r) => {
                frame.quads[k].rect = r;
                k += 1;
            }
            None => {
                frame.quads.remove(k);
            }
        }
    }
    // The lines (`YOS:715-757`, `YOS:1313-1316`), clipped to the tabs' area grown by the pixel
    // the last tab's right line sits beyond its cell.
    {
        let grown = Rect { w: tabs_area.w + 1.0, h: tabs_area.h + 1.0, ..tabs_area };
        let line_quad = |frame: &mut Frame, r: Rect, f: crate::tab_theme::Fill| {
            if let Some(r) = cut(r, grown) {
                fill_quad(frame, r, f);
            }
        };
        let last = bar.cells.len().saturating_sub(1);
        let mut top_done = [false; 2];
        // The selected tab's lines first, then the unselected tabs' over them (`YOS:1290-1300`).
        let passes = [true, false].into_iter().flat_map(|pass| bar.cells.iter().enumerate().filter(move |(_, (i, _, _))| (*i == active) == pass));
        for (k, (i, _, r)) in passes {
            let cell = px(*r);
            let selected = *i == active;
            if bar.vertical {
                // A row's foot, across the row less the style's insets.
                let (l, rt) = pal.line_inset;
                line_quad(frame, Rect { x: cell.x + l * scale, y: cell.y + cell.h - 1.0, w: cell.w - (l + rt) * scale + 1.0, h: 1.0 }, pal.between);
            } else {
                // A left line before the first tab when it starts past the edge, the right line
                // inside every tab but the last and just past the last, from below the top pixel.
                if k == 0 && cell.x - area.x > 1.0 {
                    line_quad(frame, Rect { y: cell.y + 1.0, w: 1.0, h: cell.h - 1.0, ..cell }, pal.between);
                }
                let x = if k == last { cell.x + cell.w } else { cell.x + cell.w - 1.0 };
                line_quad(frame, Rect { x, y: cell.y + 1.0, w: 1.0, h: cell.h - 1.0, ..cell }, pal.between);
            }
            // The top line: a top bar (attached to the title bar) leaves it off the selected tab;
            // the other positions draw it on every tab (a side bar once per kind).
            let top = bar.position == crate::tab_position::TabPosition::Top;
            // (`shouldDrawTopLineSelected:attached:position:`, `YOS:1628-1646`): an unattached top
            // bar draws none.
            if (pal.attached || !top) && !(top && selected) && !(bar.vertical && std::mem::replace(&mut top_done[usize::from(selected)], true)) {
                line_quad(frame, Rect { y: area.y, h: 1.0, ..cell }, if selected { pal.top_selected } else { pal.top });
            }
        }
        // Past the "+" button the top line runs to the bar's end.
        if let (Some(add), crate::tab_position::TabPosition::Top, true) = (bar.add, bar.position, pal.attached) {
            let x = px(add).x;
            line_quad(frame, Rect { x, y: area.y, w: area.x + area.w - x, h: 1.0 }, pal.top);
        }
    }
    // The line between the bar and the content, along the content's side of the bar.
    {
        use crate::tab_position::TabPosition;
        let edge = match bar.position {
            TabPosition::Top => Rect { y: area.y + area.h - 1.0, h: 1.0, ..area },
            TabPosition::Bottom => Rect { h: 1.0, ..area },
            TabPosition::Left => Rect { x: area.x + area.w - 1.0, w: 1.0, ..area },
            TabPosition::Right => Rect { w: 1.0, ..area },
        };
        let solid = crate::tab_theme::Fill { color: rgb(0, 0, 0), alpha: 1.0 };
        // Attached or not, Dark's gradient (`DRK:113-125`) covers its 0.3 divider on the same row, so
        // the line stays solid.
        fill_quad(frame, edge, if pal.dark_edge_gradient { solid } else { pal.edge });
    }
    // What is drawn over the selected tab and the lines (`drawPostHocDecorationsOnSelectedCell:`),
    // only in a window with a colored tab: Minimal's underline (`MIN:278-325`) or the Regular
    // styles' outline (`YOS:805-839`).
    if let (true, Some((k, cell, _, brightness))) = (any_colored, selected_deco) {
        if let Some(m) = &pal.minimal {
            // Two 2-point strokes along the cell's bottom, from the bar's start when the first tab
            // takes the left inset. The tab's darkness is `brightness < 0` in iTerm2 (never true),
            // so the outer stroke is white over a dark bar.
            let from = if k == 0 && m.left_inset_is_first_tab && !bar.vertical { area.x } else { cell.x };
            let u = crate::tab_theme::minimal_underline(brightness < 0.0, pal.dark_controls, m.underline, key);
            let t = (2.0 * scale).round().max(1.0);
            for (rise, f) in [(t, u.outer), (2.0 * t, u.inner)] {
                fill_quad(frame, Rect { x: from, y: cell.y + cell.h - rise, w: cell.x + cell.w - from, h: t }, f);
            }
        } else {
            // The cell outset half a point sideways and inset half a point down, then each stroke
            // (centered on its rectangle's edge) `width` in from the last: the outer ring's outside
            // is `width / 2 - 1.5` in sideways and `width / 2 - 0.5` down, its inside and the inner
            // ring's outside `1.5 x width` less 1.5 and less 0.5, the inner ring's inside `2.5 x
            // width` less 1.5 and less 0.5. Each edge rounds to a pixel.
            let o = crate::tab_theme::selected_outline(brightness, pal.colored_outline, key);
            let w = o.width;
            let edge = |dx: f32, dy: f32| {
                let (x0, x1) = ((cell.x + dx * scale).round(), (cell.x + cell.w - dx * scale).round());
                let (y0, y1) = ((cell.y + dy * scale).round(), (cell.y + cell.h - dy * scale).round());
                Rect { x: x0, y: y0, w: x1 - x0, h: y1 - y0 }
            };
            let bounds = [edge(w / 2.0 - 1.5, w / 2.0 - 0.5), edge(1.5 * w - 1.5, 1.5 * w - 0.5), edge(2.5 * w - 1.5, 2.5 * w - 0.5)];
            for (ring, f) in [(0, o.outer), (1, o.inner)] {
                let (out, inn) = (bounds[ring], bounds[ring + 1]);
                let bands = if inn.w <= 0.0 || inn.h <= 0.0 {
                    vec![out]
                } else {
                    // Each pixel once: the top and bottom in full, the sides between them.
                    vec![
                        Rect { h: inn.y - out.y, ..out },
                        Rect { y: inn.y + inn.h, h: out.y + out.h - inn.y - inn.h, ..out },
                        Rect { y: inn.y, w: inn.x - out.x, h: inn.h, ..out },
                        Rect { x: inn.x + inn.w, y: inn.y, w: out.x + out.w - inn.x - inn.w, h: inn.h },
                    ]
                };
                for band in bands {
                    if let Some(band) = cut(band, tabs_area) {
                        fill_quad(frame, band, f);
                    }
                }
            }
        }
    }
    // Minimal: the outline notch around the selected tab replaces the edge line (`MIN:600-875`).
    if let Some(m) = &pal.minimal {
        let first = bar.cells.iter().map(|(i, _, r)| (*i, r.x + r.y)).min_by(|a, b| a.1.total_cmp(&b.1)).map(|c| c.0);
        use crate::tab_position::TabPosition;
        if let Some((_, _, r)) = bar.cells.iter().find(|(i, _, _)| *i == active) {
            // One tab in all (`bar.cells.count`, which also counts the overflow menu and collapsed
            // tabs, so the same count as `Inputs.tabs`): a bottom bar draws no outline, a top bar
            // none when the inset joins the tab (`MIN:594-598`, `:735-737`).
            let skip = tabs.len() == 1 && match bar.position {
                TabPosition::Top => m.left_inset_is_first_tab,
                TabPosition::Bottom => true,
                _ => false,
            };
            if !skip {
                for r in minimal_notch(area, px(*r), bar.position, m.left_inset_is_first_tab, first == Some(active)) {
                    fill_quad(frame, r, m.outline);
                }
            }
        } else if bar.hidden.contains(&active) && tabs.len() > 1 {
            // The selected tab is in the overflow menu: only the part before it is drawn, with the
            // right margin (24 points, `rightMarginForTabBarControlWithOverflow:`) standing in for
            // the tab on a horizontal bar and the overflow row on a side bar (`MIN:848-870`,
            // `:759-770`, `:890-911`, `:932-934`).
            let stand_in = if bar.vertical {
                bar.overflow.map(px)
            } else {
                let margin = crate::tab_layout::right_margin(true, bar.add.is_some()) * scale;
                Some(Rect { x: area.x + area.w - margin, w: margin, ..area })
            };
            if let Some(stand_in) = stand_in {
                for r in minimal_notch_parts(area, stand_in, bar.position, m.left_inset_is_first_tab, false).0 {
                    fill_quad(frame, r, m.outline);
                }
            }
        }
    }
    // The "+" and the overflow button (`PSM setupButtons`), over the tabs' right margin (a side
    // bar's overflow button is its bottom row). The overflow button is two chevrons a gap
    // apart, in the text color while its menu is open (`PSMOverflowPopUpButton`
    // `overflowImagePressed`).
    let s = TAB_GLYPH_PT * font_scale;
    let em = TAB_GLYPH_PT * scale;
    let (bright, dim) = (over(pal.bar, pal.text_selected), over(pal.bar, pal.text_unselected));
    let overflow_color = if overflow_down { bright } else { dim };
    let buttons = [(bar.add, TAB_ADD_GLYPH, 0.0, dim), (bar.overflow, TAB_OVERFLOW_GLYPH, TAB_OVERFLOW_GAP * em, overflow_color)];
    for (rect, ch, gap, color) in buttons.into_iter().filter_map(|(r, ch, gap, color)| r.map(|r| (px(r), ch, gap, color))) {
        let x = rect.x + ((rect.w - em - gap) / 2.0).floor();
        let y = rect.y + ((rect.h - line * s) / 2.0).floor();
        let xs: &[f32] = if gap > 0.0 { &[x, x + gap] } else { &[x] };
        for &x in xs {
            frame.rows.push(TextRow { x, y, spans: vec![span(ch.to_string(), color, false, false, true)], clip: clip_rect(rect, area), scale: s, face: crate::render::DEFAULT_FACE });
        }
    }
}

/// The 1-pixel lines of the Minimal outline notch (`MIN:600-875`) as disjoint rectangles: along
/// the bar's content side up to the selected tab (`selected`), up its left side, down its right
/// side and on to the bar's far end. A first tab drops the part before it when the left inset
/// counts as part of it (a bottom bar drops it whatever the setting says). In a side bar the
/// lines run down the inner edge and across the selected row's first and last pixel rows.
pub fn minimal_notch(bar: Rect, selected: Rect, position: crate::tab_position::TabPosition, left_inset_is_first_tab: bool, first_selected: bool) -> Vec<Rect> {
    let (mut before, after) = minimal_notch_parts(bar, selected, position, left_inset_is_first_tab, first_selected);
    before.extend(after);
    before
}

/// `minimal_notch` as its two parts: the lines before the selected tab (and its near side) and the
/// lines from its far side on. With the selected tab in the overflow menu only the first is drawn.
pub fn minimal_notch_parts(bar: Rect, selected: Rect, position: crate::tab_position::TabPosition, left_inset_is_first_tab: bool, first_selected: bool) -> (Vec<Rect>, Vec<Rect>) {
    use crate::tab_position::TabPosition::{Bottom, Left, Right, Top};
    let (mut before, mut after_parts) = (Vec::new(), Vec::new());
    let add = |out: &mut Vec<Rect>, r: Rect| {
        if r.w > 0.0 && r.h > 0.0 {
            out.push(r);
        }
    };
    match position {
        Top | Bottom => {
            let top = position == Top;
            let (line_y, side_y) = if top { (bar.y + bar.h - 1.0, bar.y) } else { (bar.y, bar.y + 1.0) };
            let side_h = bar.h - 1.0;
            if !(first_selected && (!top || left_inset_is_first_tab)) {
                add(&mut before, Rect { x: bar.x, y: line_y, w: selected.x - bar.x, h: 1.0 });
                if selected.x - 1.0 >= bar.x {
                    add(&mut before, Rect { x: selected.x - 1.0, y: side_y, w: 1.0, h: side_h });
                }
            }
            let after = selected.x + selected.w;
            if after < bar.x + bar.w {
                add(&mut after_parts, Rect { x: after, y: side_y, w: 1.0, h: side_h });
                add(&mut after_parts, Rect { x: after, y: line_y, w: bar.x + bar.w - after, h: 1.0 });
            }
        }
        Left | Right => {
            let edge = if position == Left { bar.x + bar.w - 1.0 } else { bar.x };
            if !(first_selected && left_inset_is_first_tab) {
                add(&mut before, Rect { x: edge, y: bar.y, w: 1.0, h: selected.y - bar.y });
                add(&mut before, Rect { x: bar.x, y: selected.y, w: bar.w, h: 1.0 });
            }
            let below = selected.y + selected.h;
            add(&mut after_parts, Rect { x: bar.x, y: below - 1.0, w: bar.w, h: 1.0 });
            add(&mut after_parts, Rect { x: edge, y: below, w: 1.0, h: bar.y + bar.h - below });
        }
    }
    (before, after_parts)
}

/// A quad of `f` over what is behind it; nothing when it is clear.
fn fill_quad(frame: &mut Frame, rect: Rect, f: crate::tab_theme::Fill) {
    if f.alpha > 0.0 {
        frame.quads.push(Quad { rect, color: rgba(f.color, f.alpha) });
    }
}

/// The icon font's size for the caption buttons' glyphs, in points (Windows 11's are 10).
const CAPTION_GLYPH_PT: f32 = 10.0;
/// Windows 11's caption button colors: the close button's red, and the hover and press tints of
/// the others (black on a light bar, white on a dark one).
const CAPTION_CLOSE: Rgb = rgb(0xc4, 0x2b, 0x1c);
const CAPTION_HOVER: (f32, f32) = (0.0373, 0.0605);
const CAPTION_PRESS: (f32, f32) = (0.0241, 0.0419);

/// The strip a Minimal or Compact window's tab bar and caption sit in, under everything drawn
/// there: the bar's material and its margin fill.
pub fn caption_bar(frame: &mut Frame, bar: Rect, pal: &crate::tab_theme::TabPalette) {
    frame.quads.push(Quad { rect: bar, color: rgba(pal.bar, 1.0) });
    fill_quad(frame, bar, pal.margin);
}

/// The three window buttons in their rects (`caption::layout`): each glyph from the icon font in
/// the selected tab's text color, a hover or press fill behind it (Windows 11's colors), the
/// close button red with a white glyph, Restore in place of Maximize in a maximized window.
#[allow(clippy::too_many_arguments)]
pub fn caption_buttons(frame: &mut Frame, buttons: &[(crate::caption::Button, Rect); 3], hover: Option<crate::caption::Button>, pressed: Option<crate::caption::Button>, maximized: bool, pal: &crate::tab_theme::TabPalette, scale: f32, font_scale: f32, line: f32) {
    use crate::caption::{Button, CLOSE_GLYPH, MAXIMIZE_GLYPH, MINIMIZE_GLYPH, RESTORE_GLYPH};
    let ink = crate::tab_theme::over(pal.bar, pal.text_selected);
    let s = CAPTION_GLYPH_PT * font_scale;
    let em = CAPTION_GLYPH_PT * scale;
    for (button, rect) in buttons {
        let down = pressed == Some(*button);
        let over = down || (pressed.is_none() && hover == Some(*button));
        let mut color = ink;
        if over {
            let fill = match (button, pal.dark_controls) {
                (Button::Close, _) => Some(rgba(CAPTION_CLOSE, if down { 0.9 } else { 1.0 })),
                (_, false) => Some(rgba(rgb(0, 0, 0), if down { CAPTION_PRESS.0 } else { CAPTION_HOVER.0 })),
                (_, true) => Some(rgba(rgb(255, 255, 255), if down { CAPTION_PRESS.1 } else { CAPTION_HOVER.1 })),
            };
            frame.quads.extend(fill.map(|color| Quad { rect: *rect, color }));
            if *button == Button::Close {
                color = rgb(255, 255, 255);
            }
        }
        let glyph = match button {
            Button::Minimize => MINIMIZE_GLYPH,
            Button::Maximize if maximized => RESTORE_GLYPH,
            Button::Maximize => MAXIMIZE_GLYPH,
            Button::Close => CLOSE_GLYPH,
        };
        let span = Span { text: glyph.to_string(), color, bold: false, italic: false, alt_font: false, icon: true, prose: false, badge: false, toolbelt: false, fake_bold: false, ligatures: 0, mask: 0, font: 0 };
        let x = rect.x + ((rect.w - em) / 2.0).floor();
        let y = rect.y + ((rect.h - line * s) / 2.0).floor();
        frame.rows.push(TextRow { x, y, spans: vec![span], clip: *rect, scale: s, face: crate::render::DEFAULT_FACE });
    }
}

/// The caption's menu button: the hamburger glyph in the selected text color, centered in `rect`.
pub fn caption_menu_button(frame: &mut Frame, rect: Rect, pal: &crate::tab_theme::TabPalette, scale: f32, font_scale: f32, line: f32) {
    let ink = crate::tab_theme::over(pal.bar, pal.text_selected);
    let s = CAPTION_GLYPH_PT * font_scale;
    let em = CAPTION_GLYPH_PT * scale;
    let span = Span { text: crate::caption::MENU_GLYPH.to_string(), color: ink, bold: false, italic: false, alt_font: false, icon: true, prose: false, badge: false, toolbelt: false, fake_bold: false, ligatures: 0, mask: 0, font: 0 };
    let x = rect.x + ((rect.w - em) / 2.0).floor();
    let y = rect.y + ((rect.h - line * s) / 2.0).floor();
    frame.rows.push(TextRow { x, y, spans: vec![span], clip: rect, scale: s, face: crate::render::DEFAULT_FACE });
}

/// A caption text row in the bar's prose font, `width` pixels wide, at `x`.
#[allow(clippy::too_many_arguments)]
fn caption_row(frame: &mut Frame, text: &str, x: f32, rect: Rect, color: Rgb, pal: &crate::tab_theme::TabPalette, font_scale: f32, line: f32) {
    let s = pal.font_pt * font_scale;
    let span = Span { text: text.to_string(), color, bold: false, italic: false, alt_font: false, icon: false, prose: true, badge: false, toolbelt: false, fake_bold: false, ligatures: 0, mask: 0, font: 0 };
    let y = rect.y + ((rect.h - line * s) / 2.0).floor();
    frame.rows.push(TextRow { x, y, spans: vec![span], clip: rect, scale: s, face: crate::render::DEFAULT_FACE });
}

/// The window's title in the caption where no tab bar shows: centered in `rect` (left-aligned and
/// clipped when `width`, its measured pixels, is more than the room), in the selected text color.
#[allow(clippy::too_many_arguments)]
pub fn caption_title(frame: &mut Frame, text: &str, rect: Rect, width: f32, pal: &crate::tab_theme::TabPalette, _scale: f32, font_scale: f32, line: f32) {
    if text.is_empty() {
        return;
    }
    let x = rect.x + ((rect.w - width) / 2.0).floor().max(0.0);
    caption_row(frame, text, x, rect, crate::tab_theme::over(pal.bar, pal.text_selected), pal, font_scale, line);
}

/// The window name beside the tabs: left-aligned in `rect` at `NAME_ALPHA` over the bar in the
/// selected text color, cut with an ellipsis to the rect's width (`measure` gives a text's pixels).
#[allow(clippy::too_many_arguments)]
pub fn caption_name(frame: &mut Frame, text: &str, rect: Rect, measure: &mut dyn FnMut(&str) -> f32, pal: &crate::tab_theme::TabPalette, _scale: f32, font_scale: f32, line: f32) {
    let text = crate::caption::fit_name(text, rect.w, measure);
    if text.is_empty() {
        return;
    }
    let full = crate::tab_theme::over(pal.bar, pal.text_selected);
    let color = crate::tab_theme::over(pal.bar, crate::tab_theme::Fill { color: full, alpha: crate::caption::NAME_ALPHA });
    caption_row(frame, &text, rect.x, rect, color, pal, font_scale, line);
}

/// The window switch label (R11) right-aligned in `rect` (`caption::layout`'s shortcut, the
/// measured `width` plus padding, half of it on each side), in the unselected text color.
#[allow(clippy::too_many_arguments)]
pub fn caption_shortcut(frame: &mut Frame, text: &str, rect: Rect, width: f32, pal: &crate::tab_theme::TabPalette, scale: f32, font_scale: f32, line: f32) {
    if text.is_empty() {
        return;
    }
    let x = rect.x + rect.w - width - 4.0 * scale;
    caption_row(frame, text, x, rect, crate::tab_theme::over(pal.bar, pal.text_unselected), pal, font_scale, line);
}

/// The squared group run decoration of iTerm2's non-Tahoe styles (`YS:1348-1600`), over the
/// tabs: for each chip whose run shows a tab, a solid block of the group color from the chip
/// to the first tab with the name in bold in the contrasting color, and a 1-point outline in
/// the group color around the chip and the run's tabs, outset 1 across the bar and cut to the
/// bar, the trailing row (a side bar's column) left to the divider. A collapsed group's chip
/// is the block alone with "Name  N" and a chevron. The quads are cut by the caller.
#[allow(clippy::too_many_arguments)]
fn group_runs(frame: &mut Frame, tabs: &[TabLook], bar: &crate::tab_layout::Bar, area: Rect, tabs_area: Rect, scale: f32, font_scale: f32, line: f32) {
    let px = |r: Rect| Rect { x: area.x + r.x * scale, y: area.y + r.y * scale, w: r.w * scale, h: r.h * scale };
    let bounds = Rect { x: 0.0, y: 0.0, w: area.w / scale, h: area.h / scale };
    let stroke = scale.round().max(1.0);
    let name_scale = GROUP_NAME_PT * font_scale;
    let name_h = line * name_scale;
    for ((id, chip), t) in bar.chips.iter().zip(&bar.chip_texts) {
        let color = tabs.get(t.run.start).and_then(|l| l.group.as_ref()).map_or(GROUP_DEFAULT, |g| g.1);
        let text = contrasting(color);
        // The run's tabs shown, in order.
        let shown: Vec<Rect> = bar.cells.iter().filter(|(i, _, _)| t.run.contains(i)).map(|c| c.2).collect();
        let union = shown.iter().copied().reduce(|a, b| {
            let (x, y) = (a.x.min(b.x), a.y.min(b.y));
            Rect { x, y, w: (a.x + a.w).max(b.x + b.w) - x, h: (a.y + a.h).max(b.y + b.h) - y }
        });
        let (bx, solid) = match union {
            Some(tabs_rect) => {
                let pill = if bar.vertical {
                    Rect { x: tabs_rect.x - GROUP_OUTSET, y: chip.y, w: tabs_rect.w + 2.0 * GROUP_OUTSET, h: tabs_rect.y + tabs_rect.h - chip.y }
                } else {
                    Rect { x: chip.x, y: tabs_rect.y - GROUP_OUTSET, w: tabs_rect.x + tabs_rect.w - chip.x, h: tabs_rect.h + 2.0 * GROUP_OUTSET }
                };
                let Some(bx) = trail(cut(pill, bounds), bar.vertical) else { continue };
                let first = shown.iter().copied().fold(shown[0], |a, b| if (bar.vertical && b.y < a.y) || (!bar.vertical && b.x < a.x) { b } else { a });
                let solid = if bar.vertical {
                    Rect { h: (first.y - bx.y).max(0.0), ..bx }
                } else {
                    Rect { w: (first.x - bx.x).max(0.0), ..bx }
                };
                (bx, solid)
            }
            None if t.collapsed => {
                let grown = if bar.vertical { *chip } else { Rect { y: chip.y - GROUP_OUTSET, h: chip.h + 2.0 * GROUP_OUTSET, ..*chip } };
                let Some(bx) = trail(cut(grown, bounds), bar.vertical) else { continue };
                (bx, bx)
            }
            // Every tab of an expanded run is in the overflow menu: nothing to enclose.
            None => continue,
        };
        let (b, sp) = (px(bx), px(solid));
        frame.quads.push(Quad { rect: sp, color: rgba(color, 1.0) });
        if !t.label.is_empty() {
            let x = sp.x + GROUP_NAME_PAD * scale;
            let y = sp.y + ((sp.h - name_h) / 2.0).floor();
            let span = Span { text: t.label.clone(), color: text, bold: true, italic: false, alt_font: false, icon: false, prose: true, badge: false, toolbelt: false, fake_bold: false, ligatures: 0, mask: 0, font: 0 };
            frame.rows.push(TextRow { x, y, spans: vec![span], clip: clip_rect(sp, tabs_area), scale: name_scale, face: crate::render::DEFAULT_FACE });
        }
        if t.collapsed && union.is_none() {
            // The chevron, centered in its room at the block's end (`YS:1482-1494`).
            let s = GROUP_CHEVRON_PT * font_scale;
            let cx = b.x + b.w - (GROUP_NAME_PAD + GROUP_CHEVRON / 2.0) * scale;
            let x = (cx - GROUP_CHEVRON_PT * scale / 2.0).floor();
            let y = b.y + ((b.h - line * s) / 2.0).floor();
            let span = Span { text: GROUP_CHEVRON_GLYPH.to_string(), color: text, bold: false, italic: false, alt_font: false, icon: true, prose: false, badge: false, toolbelt: false, fake_bold: false, ligatures: 0, mask: 0, font: 0 };
            frame.rows.push(TextRow { x, y, spans: vec![span], clip: clip_rect(b, tabs_area), scale: s, face: crate::render::DEFAULT_FACE });
        }
        // The squared outline.
        frame_edges(frame, b, stroke, color);
        // A collapsed group a dragged tab would join: the accent frame around its block.
        if t.collapsed && union.is_none() && bar.join.as_deref() == Some(id.as_str()) {
            let out = JOIN_FRAME_OUTSET * scale;
            let framed = Rect { x: b.x - out, y: b.y - out, w: b.w + 2.0 * out, h: b.h + 2.0 * out };
            frame_edges(frame, framed, (JOIN_FRAME * scale).round().max(1.0), LIST_SELECTED);
        }
    }
}

/// Draws a tab's icon in its 16-point box `g` (pixels) at `alpha`, cut to `clip`: a picture as
/// an image (faded pixels under a separate key when not opaque), or the letter icon as a circle
/// 1 point inside the box with the letter bold in the tab's background `back` centered on it
/// (`letter_w` its width in points at `LETTER_PT`).
#[allow(clippy::too_many_arguments)]
fn tab_graphic(frame: &mut Frame, image: &TabImage, g: Rect, alpha: f32, back: Rgb, clip: Rect, scale: f32, font_scale: f32, line: f32, letter_w: f32) {
    if alpha <= 0.0 {
        return;
    }
    let g = Rect { x: g.x.round(), y: g.y.round(), w: g.w.round(), h: g.h.round() };
    match image {
        TabImage::Picture { key, pixels, size } => {
            let Some(rect) = cut(g, clip) else { return };
            let uv = [(rect.x - g.x) / g.w, (rect.y - g.y) / g.h, (rect.x + rect.w - g.x) / g.w, (rect.y + rect.h - g.y) / g.h];
            let level = 255 - (alpha.clamp(0.0, 1.0) * 255.0).round() as usize;
            let pixels = if level == 0 {
                pixels.clone()
            } else {
                std::sync::Arc::new(pixels.chunks_exact(4).flat_map(|p| [p[0], p[1], p[2], (f32::from(p[3]) * alpha).round() as u8]).collect())
            };
            frame.images.push(ImageQuad { rect, uv, key: (*key, level), pixels, size: *size, repeat: false });
        }
        TabImage::Letter { letter, color } => {
            let inset = (LETTER_INSET * scale).round();
            let circle = Rect { x: g.x + inset, y: g.y + inset, w: g.w - 2.0 * inset, h: g.h - 2.0 * inset };
            rounded_box(&mut frame.quads, circle, circle.w / 2.0, rgba(blend(back, *color, alpha), 1.0));
            let s = LETTER_PT * font_scale;
            let x = circle.x + (circle.w - letter_w * scale) / 2.0;
            let y = circle.y + (circle.h - line * s) / 2.0;
            let span = Span { text: letter.clone(), color: back, bold: true, italic: false, alt_font: false, icon: false, prose: true, badge: false, toolbelt: false, fake_bold: false, ligatures: 0, mask: 0, font: 0 };
            frame.rows.push(TextRow { x, y, spans: vec![span], clip, scale: s, face: crate::render::DEFAULT_FACE });
        }
    }
}

/// A frame `stroke` pixels thick inside `r`.
fn frame_edges(frame: &mut Frame, r: Rect, stroke: f32, color: Rgb) {
    for edge in [
        Rect { h: stroke, ..r },
        Rect { y: r.y + r.h - stroke, h: stroke, ..r },
        Rect { w: stroke, ..r },
        Rect { x: r.x + r.w - stroke, w: stroke, ..r },
    ] {
        frame.quads.push(Quad { rect: edge, color: rgba(color, 1.0) });
    }
}

/// A run's box less its trailing row on a horizontal bar, or column on a side bar, which the
/// divider owns (`YS:1540-1551`); None when nothing is left.
fn trail(r: Option<Rect>, vertical: bool) -> Option<Rect> {
    let r = r?;
    let r = if vertical { Rect { w: r.w - 1.0, ..r } } else { Rect { h: r.h - 1.0, ..r } };
    (r.w > 0.0 && r.h > 0.0).then_some(r)
}

/// The spinner's turn, 0 to 1: an eighth more at each half-second activity tick.
fn spinner_turn() -> f32 {
    let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis());
    ((ms / 500) % 8) as f32 / 8.0
}

/// The activity spinner: a three-quarter arc of dots around `rect`'s center, fading toward its
/// tail, turned by `turn` (0 to 1).
pub fn spinner(frame: &mut Frame, rect: Rect, turn: f32, color: Rgb, scale: f32) {
    const DOTS: usize = 9;
    let d = (2.0 * scale).round().max(1.0);
    let radius = rect.w.min(rect.h) / 2.0 - d;
    let (cx, cy) = (rect.x + rect.w / 2.0, rect.y + rect.h / 2.0);
    for k in 0..DOTS {
        let a = (turn + 0.75 * k as f32 / (DOTS - 1) as f32) * std::f32::consts::TAU;
        let rect = Rect { x: cx + radius * a.cos() - d / 2.0, y: cy + radius * a.sin() - d / 2.0, w: d, h: d };
        frame.quads.push(Quad { rect, color: rgba(color, 0.25 + 0.75 * k as f32 / (DOTS - 1) as f32) });
    }
}

/// The split pane dividers, in the theme's `color` (`App::divider_color`).
pub fn dividers(frame: &mut Frame, dividers: &[Divider], color: Rgb) {
    for d in dividers {
        frame.overlay.push(Quad { rect: d.rect, color: rgba(color, 1.0) });
    }
}

/// The line under the title bar (`chrome::division_visible`): across the window's full
/// width at its top, `height` pixels tall (`chrome::division_height`), above a top status bar.
pub fn division_line(frame: &mut Frame, width: f32, height: f32, color: Rgb) {
    let rect = Rect { x: 0.0, y: 0.0, w: width, h: height };
    frame.quads.push(Quad { rect, color: rgba(color, 1.0) });
}

/// A pane's background image: its pieces first, then the pane's default background color
/// over them at `1 - blend` (iTerm2's blend with an opaque window). Cells with the default
/// background then draw nothing, so the veiled image shows; other cells cover it.
pub fn background(frame: &mut Frame, pane: Rect, pieces: &[crate::bgimage::Piece], image: &crate::bgimage::Loaded, bg: Rgb, blend: f32) {
    for (rect, uv, repeat) in pieces {
        frame.backgrounds.push(ImageQuad { rect: *rect, uv: *uv, key: (image.key, 0), pixels: image.rgba.clone(), size: image.size, repeat: *repeat });
    }
    frame.quads.push(Quad { rect: pane, color: rgba(bg, 1.0 - blend.clamp(0.0, 1.0)) });
}

/// iTerm2's announcement bar across the top of a pane: its text, buttons, and close box.
/// The pane's text under the bar is hidden.
/// Draws the bar and returns the box it drew (its hit rectangle).
pub fn info_bar(frame: &mut Frame, bar: &crate::infobar::Bar, l: &crate::infobar::Layout, cell: (f32, f32)) -> Rect {
    let r = l.bar;
    for row in frame.rows.iter_mut() {
        if row.y < r.y + r.h && row.y + cell.1 > r.y && row.clip.x < r.x + r.w && row.clip.x + row.clip.w > r.x {
            row.clip.w = 0.0;
        }
    }
    cut_quads(frame, r);
    cut_glyphs(frame, r);
    frame.quads.push(Quad { rect: r, color: rgba(NOTE_BG, 1.0) });
    let span = |text: String| Span { text, color: LABEL_TEXT, bold: false, italic: false, alt_font: false, icon: false, prose: false, badge: false, toolbelt: false, fake_bold: false, ligatures: 0, mask: 0, font: 0 };
    let text_end = l.buttons.first().map_or(l.close.x, |b| b.x);
    let clip = Rect { w: (text_end - r.x).max(0.0), ..r };
    frame.rows.push(TextRow { x: r.x + cell.0, y: r.y + 4.0, spans: vec![span(bar.shown_text())], clip, scale: 1.0, face: crate::render::DEFAULT_FACE });
    for (i, b) in l.buttons.iter().enumerate() {
        frame.quads.push(Quad { rect: *b, color: rgba(BAR_BUTTON, 1.0) });
        frame.rows.push(TextRow { x: b.x + cell.0, y: r.y + 4.0, spans: vec![span(bar.label(i))], clip: *b, scale: 1.0, face: crate::render::DEFAULT_FACE });
    }
    frame.rows.push(TextRow { x: l.close.x + cell.0 / 2.0, y: r.y + 4.0, spans: vec![span("x".into())], clip: l.close, scale: 1.0, face: crate::render::DEFAULT_FACE });
    r
}

/// The status bar's own colors when the layout sets none: the tab bar's background (no
/// macOS vibrancy on Windows), its text color (iTerm2's `labelColor`), and the edge
/// separator's line (the system separator color).
pub const STATUS_BAR_BG: Rgb = TAB_BAR;
pub const STATUS_BAR_TEXT: Rgb = TAB_TEXT;
pub const STATUS_BAR_EDGE: Rgb = DIVIDER;

/// One component's text on a status bar: where it starts, what it says, its color, its
/// size against the terminal font, and the component's own room (it is cut there).
pub struct BarText {
    pub x: f32,
    pub text: String,
    pub color: Rgb,
    pub scale: f32,
    pub clip: Rect,
    /// Drawn with the icon font (a component's Segoe Fluent Icons glyph).
    pub icon: bool,
    /// Pixels below the centered line (`statusBarComponentVerticalOffset`).
    pub dy: f32,
}

/// What a status bar shows, already placed in window pixels (`iTermStatusBarView`).
pub struct StatusBarDraw<'a> {
    pub rect: Rect,
    /// A bar in a pane's title bar: the title bar has painted the background and the edge.
    pub bare: bool,
    pub background: (Rgb, f32),
    /// Component background bands: from x, to x, color.
    pub bands: &'a [(f32, f32, Rgb, f32)],
    /// Separator lines' x; drawn only with a color.
    pub separators: &'a [f32],
    pub separator_color: Option<(Rgb, f32)>,
    /// Plain boxes over the bands (a field's border, the progress track): rect, color,
    /// alpha.
    pub boxes: &'a [(Rect, Rgb, f32)],
    /// Button faces (Call Script Function): rounded boxes, with the corner radius.
    pub buttons: &'a [(Rect, Rgb, f32, f32)],
    pub texts: &'a [BarText],
    /// The line between the bar and the terminal ("Draw separator between status bar and
    /// terminal"), already placed.
    pub edge: Option<Rect>,
    /// One point in pixels (the window's scale).
    pub scale: f32,
}

/// A rounded box as quads, one per pixel row inside each corner (a circle's inset there)
/// and one for the middle.
pub fn rounded_box(quads: &mut Vec<Quad>, r: Rect, radius: f32, color: [f32; 4]) {
    let rr = radius.round().clamp(0.0, (r.w / 2.0).min(r.h / 2.0).floor());
    let rows = rr as usize;
    for k in 0..rows {
        let d = rr - k as f32 - 0.5;
        let inset = (rr - (rr * rr - d * d).max(0.0).sqrt()).round();
        let w = r.w - 2.0 * inset;
        for y in [r.y + k as f32, r.y + r.h - 1.0 - k as f32] {
            quads.push(Quad { rect: Rect { x: r.x + inset, y, w, h: 1.0 }, color });
        }
    }
    let h = r.h - 2.0 * rr;
    if h > 0.0 {
        quads.push(Quad { rect: Rect { y: r.y + rr, h, ..r }, color });
    }
}

/// A pane's title bar to draw (`SessionTitleView`): where its parts sit, its colors, the
/// title, and whether the label shows (not while a status bar sits in the bar).
pub struct TitleBar<'a> {
    pub geometry: crate::pane_title::Geometry,
    pub look: crate::pane_title::Look,
    pub text: &'a str,
    pub show_label: bool,
}

/// Glyph size against a part's box (12 of the tab bar's 17 point close box).
const TITLE_GLYPH: f32 = 12.0 / 17.0;

/// A title bar: its background and (outside Minimal) the 1 point line, then the label, the
/// close, menu and lock glyphs. The background goes in `quads` so a status bar in the bar draws
/// over it; every row is clipped to its part. `Span` has no alpha, so each text color is
/// pre-blended into the background. `cell` is the terminal cell in pixels.
pub fn pane_title_bar(frame: &mut Frame, bar: &TitleBar, cell: (f32, f32)) {
    let (g, look) = (&bar.geometry, &bar.look);
    let back = look.background;
    // A see-through Minimal bar (P11) is premultiplied like the pane's margins; an opaque one is as before.
    let color = if look.background_alpha < 1.0 { crate::transparency::premultiplied([back.r, back.g, back.b], look.background_alpha) } else { rgba(back, 1.0) };
    frame.quads.push(Quad { rect: g.bar, color });
    if let Some((c, a)) = look.line {
        frame.quads.push(Quad { rect: g.line, color: rgba(c, a) });
    }
    let text = blend(back, look.text.0, look.text.1);
    let menu = blend(back, look.menu.0, look.menu.1);
    let span = |text: String, color: Rgb, icon: bool| Span { text, color, bold: false, italic: false, alt_font: false, icon, prose: false, badge: false, toolbelt: false, fake_bold: false, ligatures: 0, mask: 0, font: 0 };
    // The usable height centers the label above the line.
    let usable = if look.line.is_some() { g.line.y - g.bar.y } else { g.bar.h };
    if bar.show_label && g.label.w > 0.0 && cell.0 > 0.0 {
        let label = crate::pane_title::fit_label(bar.text, (g.label.w / cell.0) as usize);
        if !label.is_empty() {
            let y = g.bar.y + ((usable - cell.1) / 2.0).floor();
            frame.rows.push(TextRow { x: g.label.x, y, spans: vec![span(label, text, false)], clip: g.label, scale: 1.0, face: crate::render::DEFAULT_FACE });
        }
    }
    let mut glyph = |ch: char, r: Rect, color: Rgb| {
        let em = r.h * TITLE_GLYPH;
        let s = em / cell.1.max(1.0);
        let x = r.x + ((r.w - em) / 2.0).floor();
        let y = r.y + ((r.h - cell.1 * s) / 2.0).floor();
        frame.rows.push(TextRow { x, y, spans: vec![span(ch.to_string(), color, true)], clip: r, scale: s, face: crate::render::DEFAULT_FACE });
    };
    glyph('\u{E711}', g.close, text);
    glyph('\u{E712}', g.menu, menu);
    if let Some(l) = g.lock {
        glyph('\u{E72E}', l, text);
    }
}

/// A status bar (`SV:151-162`): the background, the component bands, the boxes, the
/// separators (one point wide, three points in from the top and bottom), button faces, the
/// text, and the edge line.
pub fn status_bar(frame: &mut Frame, d: &StatusBarDraw, cell: (f32, f32)) {
    let r = d.rect;
    if !d.bare {
        frame.quads.push(Quad { rect: r, color: rgba(d.background.0, d.background.1) });
    }
    for &(x0, x1, c, a) in d.bands {
        if x1 > x0 {
            frame.quads.push(Quad { rect: Rect { x: x0, y: r.y, w: x1 - x0, h: r.h }, color: rgba(c, a) });
        }
    }
    for &(b, c, a) in d.boxes {
        frame.quads.push(Quad { rect: b, color: rgba(c, a) });
    }
    if let Some((c, a)) = d.separator_color {
        let inset = (3.0 * d.scale).round();
        let w = d.scale.round().max(1.0);
        for &x in d.separators {
            frame.quads.push(Quad { rect: Rect { x, y: r.y + inset, w, h: (r.h - 2.0 * inset).max(0.0) }, color: rgba(c, a) });
        }
    }
    for &(b, c, a, radius) in d.buttons {
        rounded_box(&mut frame.quads, b, radius, rgba(c, a));
    }
    for t in d.texts {
        let line_h = cell.1 * t.scale;
        let y = r.y + ((r.h - line_h) / 2.0).round() + t.dy;
        let span = Span { text: t.text.clone(), color: t.color, bold: false, italic: false, alt_font: false, icon: t.icon, prose: !t.icon, badge: false, toolbelt: false, fake_bold: false, ligatures: 0, mask: 0, font: 0 };
        frame.rows.push(TextRow { x: t.x, y, spans: vec![span], clip: t.clip, scale: t.scale, face: crate::render::DEFAULT_FACE });
    }
    if let Some(e) = d.edge.filter(|_| !d.bare) {
        frame.quads.push(Quad { rect: e, color: rgba(STATUS_BAR_EDGE, 1.0) });
    }
}

/// Text already in the frame stops at `r` (a popup drawn over it): each line behind `r`
/// keeps its part left of `r` and gets a copy for its part right of it. `line_h` is a text
/// row's height (rows clip to their whole pane, so their own line decides). A popup's box
/// goes in `popups`, which draw after images and before text.
pub fn cut_out(frame: &mut Frame, r: Rect, line_h: f32) {
    let mut extra = Vec::new();
    let heights = &frame.face_heights;
    for row in frame.rows.iter_mut() {
        let c = row.clip;
        let line_h = row_height_in(heights, row, line_h);
        if row.y >= r.y + r.h || row.y + line_h <= r.y || c.y >= r.y + r.h || c.y + c.h <= r.y || c.x >= r.x + r.w || c.x + c.w <= r.x {
            continue;
        }
        let right = r.x + r.w;
        if c.x + c.w > right {
            let mut copy = row.clone();
            copy.clip = Rect { x: right, w: c.x + c.w - right, ..c };
            extra.push(copy);
        }
        row.clip = Rect { w: (r.x - c.x).max(0.0), ..c };
    }
    frame.rows.extend(extra);
    cut_quads(frame, r);
    cut_glyphs(frame, r);
}

/// A box drawn over the pane's rows: the text of the rows it touches stops where the box
/// starts, and so do those rows' underline and strikethrough strokes (the rule for every surface
/// that cuts text: its strokes go with it, or a program's SGR 4 line draws through the surface).
pub fn cut_rows_from(frame: &mut Frame, r: Rect, line_h: f32) {
    // The box itself cuts every stroke under it, rows with text or not (a row of underlined
    // spaces pushes no text row).
    cut_quads(frame, r);
    let mut bands: Vec<Rect> = Vec::new();
    let heights = &frame.face_heights;
    for row in frame.rows.iter_mut() {
        let h = row_height_in(heights, row, line_h);
        if row.y < r.y + r.h && row.y + h > r.y && row.clip.x + row.clip.w > r.x {
            row.clip.w = (r.x - row.clip.x).max(0.0);
            let band = Rect { x: r.x, y: row.y, w: 1.0e6, h };
            if !bands.iter().any(|b| b.y == band.y) {
                bands.push(band);
            }
        }
    }
    for b in bands {
        cut_quads(frame, b);
    }
    // The glyphs in the rows the box touches stop at its left edge, whole rows like the text.
    trim_glyphs(&mut frame.glyphs, |g| (g.rect.y < r.y + r.h && g.rect.y + g.rect.h > r.y).then_some(Rect { x: r.x, y: g.rect.y, w: 1.0e6, h: g.rect.h }));
}

/// The underline and strikethrough strokes already in the frame stop at `r` (a popup drawn over them): each
/// keeps the parts of it outside `r`. `cut_out` calls this, so every popup trims both.
pub fn cut_quads(frame: &mut Frame, r: Rect) {
    for quads in [&mut frame.underlines, &mut frame.strikes, &mut frame.scrollbars] {
        *quads = quads.drain(..).flat_map(|q| outside(q.rect, r).into_iter().map(move |rect| Quad { rect, ..q })).collect();
    }
}

/// Box and block glyphs already in the frame stop at `r` (a popup drawn over them): each keeps the
/// parts of its visible area outside `r`, as `cut_out` does for a row. A glyph wholly under `r`
/// goes; one straddling it keeps a clip narrowed to what is outside (two glyphs when `r` is
/// inside it, as `cut_out` copies a row). Each piece keeps its cell, so its mask is unchanged.
pub fn cut_glyphs(frame: &mut Frame, r: Rect) {
    trim_glyphs(&mut frame.glyphs, |_| Some(r));
}

/// Applies a hole to every glyph: `hole` names the part of the frame a glyph must not draw in.
fn trim_glyphs(glyphs: &mut Vec<CellGlyph>, hole: impl Fn(&CellGlyph) -> Option<Rect>) {
    let mut kept = Vec::with_capacity(glyphs.len());
    for g in glyphs.drain(..) {
        let Some(hole) = hole(&g) else {
            kept.push(g);
            continue;
        };
        // What shows of the glyph: its cell inside its clip.
        let (x0, x1) = (g.rect.x.max(g.clip.x), (g.rect.x + g.rect.w).min(g.clip.x + g.clip.w));
        let (y0, y1) = (g.rect.y.max(g.clip.y), (g.rect.y + g.rect.h).min(g.clip.y + g.clip.h));
        if x1 <= x0 || y1 <= y0 {
            continue;
        }
        let visible = Rect { x: x0, y: y0, w: x1 - x0, h: y1 - y0 };
        let parts = outside(visible, hole);
        if parts.len() == 1 && parts[0] == visible {
            kept.push(g);
            continue;
        }
        kept.extend(parts.into_iter().map(|clip| CellGlyph { clip, ..g.clone() }));
    }
    *glyphs = kept;
}

/// Lays `layer` (a flashing tab bar drawn apart) over what the frame has, at opacity
/// `alpha`: its boxes go in `popups` (over the sessions' backgrounds and images) with their
/// alpha scaled, its boxes' own popups and overlay likewise, and its text is blended from
/// `under` (the sessions' background) toward its color by `alpha`. The sessions' text behind
/// `area` is cut out while the bar is at least half opaque, and so is their overlay (which
/// draws after all text: dividers, dimming, cursors, underlines).
pub fn lay_over(frame: &mut Frame, layer: Frame, area: Rect, alpha: f32, under: Rgb, line_h: f32) {
    let alpha = alpha.clamp(0.0, 1.0);
    if alpha >= 0.5 {
        cut_out(frame, area, line_h);
        frame.overlay = frame.overlay.drain(..).flat_map(|q| outside(q.rect, area).into_iter().map(move |rect| Quad { rect, ..q })).collect();
        // `cut_out` already trimmed the underlines.
    }
    let faded = |q: Quad| Quad { color: [q.color[0], q.color[1], q.color[2], q.color[3] * alpha], ..q };
    frame.popups.extend(layer.quads.into_iter().chain(layer.popups).map(faded));
    frame.overlay.extend(layer.overlay.into_iter().map(faded));
    frame.scrollbars.extend(layer.scrollbars.into_iter().map(faded));
    frame.underlines.extend(layer.underlines.into_iter().map(faded));
    frame.strikes.extend(layer.strikes.into_iter().map(faded));
    // Pictures keep their full opacity and draw before popups (none in the bar yet).
    frame.images.extend(layer.images);
    frame.glyphs.extend(layer.glyphs.into_iter().map(|mut g| {
        g.color = blend(under, g.color, alpha);
        g
    }));
    frame.rows.extend(layer.rows.into_iter().map(|mut row| {
        for s in &mut row.spans {
            s.color = blend(under, s.color, alpha);
        }
        row
    }));
}

/// The parts of `r` outside `hole`: `r` itself when they do not meet, else up to four
/// pieces (above, below, then left and right of `hole` within its height); none when `hole`
/// covers `r`.
fn outside(r: Rect, hole: Rect) -> Vec<Rect> {
    let (top, bottom) = (r.y.max(hole.y), (r.y + r.h).min(hole.y + hole.h));
    let (left, right) = (r.x.max(hole.x), (r.x + r.w).min(hole.x + hole.w));
    if top >= bottom || left >= right {
        return vec![r];
    }
    [
        Rect { h: top - r.y, ..r },
        Rect { y: bottom, h: r.y + r.h - bottom, ..r },
        Rect { y: top, w: left - r.x, h: bottom - top, ..r },
        Rect { x: right, y: top, w: r.x + r.w - right, h: bottom - top },
    ]
    .into_iter()
    .filter(|p| p.w > 0.0 && p.h > 0.0)
    .collect()
}

/// The completion list's box, its first shown row and how many rows show (`None` without rows).
pub fn completion_rect(rows: &[String], selected: usize, footer: Option<&str>, at: (f32, f32), bounds: Rect, cell: (f32, f32)) -> Option<(Rect, usize, usize)> {
    const SHOWN: usize = 10;
    if rows.is_empty() {
        return None;
    }
    let first = selected.saturating_sub(SHOWN - 1);
    let shown = rows.len().min(first + SHOWN) - first;
    let widest = rows[first..first + shown].iter().map(|r| r.chars().count()).chain(footer.map(|f| f.chars().count())).max().unwrap_or(10).clamp(10, 60);
    let h = (shown + usize::from(footer.is_some())) as f32 * cell.1 + 4.0;
    let w = (widest + 2) as f32 * cell.0;
    // Below the cursor, or above it when it would not fit; kept inside the pane.
    let mut y = at.1;
    if y + h > bounds.y + bounds.h {
        y = (at.1 - cell.1 - h).max(bounds.y);
    }
    let x = at.0.min(bounds.x + bounds.w - w).max(bounds.x);
    Some((Rect { x, y, w, h }, first, shown))
}

/// The completion list at the cursor (Autocomplete, Auto Command Completion): rows with
/// the chosen one marked, and a footer. `at`: the cell below the cursor's top-left corner.
/// Draws the list and returns the box it drew (None when there is nothing to show).
pub fn completion_list(frame: &mut Frame, rows: &[String], selected: usize, footer: Option<&str>, at: (f32, f32), bounds: Rect, cell: (f32, f32)) -> Option<Rect> {
    let (r, first, shown) = completion_rect(rows, selected, footer, at, bounds, cell)?;
    let visible = &rows[first..first + shown];
    let (x, y) = (r.x, r.y);
    let w = r.w;
    cut_out(frame, r, cell.1);
    frame.popups.push(Quad { rect: r, color: rgba(NOTE_BG, 1.0) });
    let span = |text: String| Span { text, color: LABEL_TEXT, bold: false, italic: false, alt_font: false, icon: false, prose: false, badge: false, toolbelt: false, fake_bold: false, ligatures: 0, mask: 0, font: 0 };
    for (i, text) in visible.iter().enumerate() {
        let ry = y + 2.0 + i as f32 * cell.1;
        if first + i == selected {
            frame.popups.push(Quad { rect: Rect { x, y: ry, w, h: cell.1 }, color: rgba(BAR_BUTTON, 1.0) });
        }
        frame.rows.push(TextRow { x: x + cell.0, y: ry, spans: vec![span(text.clone())], clip: r, scale: 1.0, face: crate::render::DEFAULT_FACE });
    }
    if let Some(f) = footer {
        frame.rows.push(TextRow { x: x + cell.0, y: y + 2.0 + visible.len() as f32 * cell.1, spans: vec![span(f.to_string())], clip: r, scale: 1.0, face: crate::render::DEFAULT_FACE });
    }
    Some(r)
}

/// The session picker's button (`SessionView.m:1203-1235`): a labelled box centered in the
/// pane, as wide as the label plus padding and never wider than the pane. The box goes in
/// `popups` (text over it stays text), the label in `rows`. Answers the box, which is the
/// button's hit rectangle.
pub fn session_picker_button(frame: &mut Frame, pane: Rect, label: &str, fg: Rgb, bg: Rgb, cell: (f32, f32), scale: f32) -> Rect {
    let (pad, height) = (2.0 * cell.0, cell.1 + 16.0 * scale);
    let w = (label.chars().count() as f32 * cell.0 + 2.0 * pad).min(pane.w);
    let h = height.min(pane.h);
    let r = Rect { x: pane.x + (pane.w - w) / 2.0, y: pane.y + (pane.h - h) / 2.0, w, h };
    cut_out(frame, r, cell.1);
    frame.popups.push(Quad { rect: r, color: rgba(blend(bg, fg, 0.18), 1.0) });
    frame_lines(&mut frame.popups, r, rgba(blend(bg, fg, 0.6), 1.0));
    let span = Span { text: label.to_string(), color: fg, bold: false, italic: false, alt_font: false, icon: false, prose: false, badge: false, toolbelt: false, fake_bold: false, ligatures: 0, mask: 0, font: 0 };
    frame.rows.push(TextRow { x: r.x + pad.min(r.w / 2.0), y: r.y + (h - cell.1) / 2.0, spans: vec![span], clip: r, scale: 1.0, face: crate::render::DEFAULT_FACE });
    r
}

/// The offscreen command line (`TDH:3593-3617`): `band` is filled over the pane's top (the
/// rows, box glyphs and strokes under it are cut out first, so nothing of row 0 shows through),
/// outlined by a 1 pixel line at its top and bottom, and the command's cells on `line` are
/// drawn in it from `text_origin`. The box goes in `popups` and the text in `rows`, never in
/// `overlay`, which draws over text. `colors` is `offscreen::band_colors` (sRGB).
pub fn offscreen_band(frame: &mut Frame, screen: &Screen, band: Rect, text_origin: (f32, f32), cell: (f32, f32), line: i32, colors: ([f32; 4], [f32; 4]), face: u16) {
    let lin = |c: [f32; 4]| [srgb_to_linear((c[0] * 255.0).round() as u8), srgb_to_linear((c[1] * 255.0).round() as u8), srgb_to_linear((c[2] * 255.0).round() as u8), c[3]];
    cut_out(frame, band, cell.1);
    frame.popups.push(Quad { rect: band, color: lin(colors.0) });
    frame.popups.push(Quad { rect: Rect { h: 1.0, ..band }, color: lin(colors.1) });
    frame.popups.push(Quad { rect: Rect { y: band.y + band.h - 1.0, h: 1.0, ..band }, color: lin(colors.1) });
    if face != crate::render::DEFAULT_FACE && !frame.face_heights.iter().any(|(f, _)| *f == face) {
        frame.face_heights.push((face, cell.1));
    }
    let grid = screen.term().grid();
    if line < -(grid.history_size() as i32) || line >= grid.screen_lines() as i32 {
        return;
    }
    // The line's cells as the pane draws them: an ASCII run is one piece, any other character
    // its own piece at its cell (fallback fonts are not monospace). Box glyphs stay text here.
    let row = &grid[alacritty_terminal::index::Line(line)];
    let palette = &screen.palette;
    let mut piece: Option<(usize, Vec<Span>)> = None;
    let mut pieces: Vec<(usize, Vec<Span>)> = Vec::new();
    for col in 0..grid.columns() {
        let c = &row[alacritty_terminal::index::Column(col)];
        if c.flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
            continue;
        }
        let mut text = c.c.to_string();
        if let Some(extra) = c.zerowidth() {
            text.extend(extra);
        }
        // In the profile's Unicode Normalization form, as the pane draws it (S12, 13o).
        crate::encoding::normalize_in_place(&mut text, screen.normalization);
        let ascii = text.is_ascii();
        let fg = if c.flags.contains(Flags::HIDDEN) { palette.cell(c.fg, c.bg, c.flags).1 } else { palette.cell(c.fg, c.bg, c.flags).0 };
        let (bold, italic) = (c.flags.contains(Flags::BOLD), c.flags.contains(Flags::ITALIC));
        let span = |text: String| Span { text, color: fg, bold, italic, alt_font: !ascii, icon: false, prose: false, badge: false, toolbelt: false, fake_bold: bold, ligatures: 0, mask: 0, font: 0 };
        if !ascii {
            pieces.extend(piece.take());
            pieces.push((col, vec![span(text)]));
            continue;
        }
        let (_, spans) = piece.get_or_insert_with(|| (col, Vec::new()));
        match spans.last_mut() {
            Some(l) if l.color == fg && l.bold == bold && l.italic == italic => l.text.push_str(&text),
            _ => spans.push(span(text)),
        }
    }
    pieces.extend(piece);
    for (col, mut spans) in pieces {
        while spans.last().is_some_and(|s| s.text.trim_end().is_empty()) {
            spans.pop();
        }
        if let Some(l) = spans.last_mut() {
            l.text.truncate(l.text.trim_end().len());
        }
        if spans.is_empty() {
            continue;
        }
        frame.rows.push(TextRow { x: text_origin.0 + col as f32 * cell.0, y: text_origin.1, spans, clip: band, scale: 1.0, face });
    }
}

/// A tab's tooltip (`PT tabView:toolTipForTabViewItem:`): a framed box at `at` (under the
/// pointer) with one text row per line of `text`, moved left and up as far as it must to stay
/// inside `area` (the window).
pub fn tab_tooltip(frame: &mut Frame, text: &str, at: (f32, f32), area: Rect, cell: (f32, f32)) {
    let lines: Vec<&str> = text.lines().collect();
    let widest = lines.iter().map(|l| l.chars().count()).max().unwrap_or(0);
    let (w, h) = ((widest + 2) as f32 * cell.0, lines.len() as f32 * cell.1 + 4.0);
    let x = at.0.min(area.x + area.w - w).max(area.x);
    let y = at.1.min(area.y + area.h - h).max(area.y);
    let r = Rect { x, y, w, h };
    cut_out(frame, r, cell.1);
    frame.popups.push(Quad { rect: r, color: rgba(FIND_BAR, 1.0) });
    frame_lines(&mut frame.popups, r, rgba(DIVIDER, 1.0));
    for (i, line) in lines.iter().enumerate() {
        frame.rows.push(TextRow { x: x + cell.0, y: y + 2.0 + i as f32 * cell.1, spans: vec![plain_span(line.to_string(), TAB_TEXT, false)], clip: r, scale: 1.0, face: crate::render::DEFAULT_FACE });
    }
}

/// One clipping as the panel draws it: its title and its detail's lines.
pub struct ClipRow {
    pub title: String,
    pub detail: Vec<String>,
}

/// What a Clippings panel shows.
pub struct ClipDraw<'a> {
    pub layout: &'a crate::clippings::Layout,
    pub panel: Rect,
    pub rows: &'a [ClipRow],
    pub selected: &'a [usize],
    pub enabled: crate::clippings::Enabled,
    /// "N/M" while an archive shows, or a hovered button's tooltip.
    pub status: Option<&'a str>,
    /// Where a dragged row would land.
    pub drop_y: Option<f32>,
}

const CLIP_BG: Rgb = rgb(0x25, 0x25, 0x28);
const CLIP_SELECTED: Rgb = rgb(0x2f, 0x5f, 0xa8);
const CLIP_DISABLED: Rgb = rgb(0x5a, 0x5a, 0x5a);

fn clip_span(text: String, color: Rgb, bold: bool, icon: bool) -> Span {
    Span { text, color, bold, italic: false, alt_font: false, icon, prose: !icon, badge: false, toolbelt: false, fake_bold: false, ligatures: 0, mask: 0, font: 0 }
}

/// The Clippings panel at the right of a session: the rows, the button strip, the status.
pub fn clippings_panel(frame: &mut Frame, d: &ClipDraw, cell: (f32, f32), scale: f32) {
    use crate::clippings::Button;
    let l = d.layout;
    frame.quads.push(Quad { rect: d.panel, color: rgba(CLIP_BG, 1.0) });
    frame.quads.push(Quad { rect: Rect { w: 1.0, ..d.panel }, color: rgba(DIVIDER, 1.0) });
    let pad = 6.0 * scale;
    for (i, (row, r)) in d.rows.iter().zip(&l.rows).enumerate() {
        if r.y >= l.list.y + l.list.h || r.y + r.h <= l.list.y {
            continue;
        }
        let top = r.y.max(l.list.y);
        let clip = Rect { x: r.x, y: top, w: r.w, h: (r.y + r.h).min(l.list.y + l.list.h) - top };
        if d.selected.contains(&i) {
            frame.quads.push(Quad { rect: clip, color: rgba(CLIP_SELECTED, 1.0) });
        }
        let line_y = r.y + r.h - 1.0;
        if line_y >= l.list.y && line_y < l.list.y + l.list.h {
            frame.quads.push(Quad { rect: Rect { x: r.x, y: line_y, w: r.w, h: 1.0 }, color: rgba(DIVIDER, 0.6) });
        }
        let x = r.x + 10.0 * scale;
        frame.rows.push(TextRow { x, y: r.y + pad, spans: vec![clip_span(row.title.clone(), TAB_TEXT, true, false)], clip, scale: 1.0, face: crate::render::DEFAULT_FACE });
        for (k, line) in row.detail.iter().enumerate() {
            let y = r.y + pad + cell.1 + 2.0 * scale + k as f32 * cell.1;
            frame.rows.push(TextRow { x, y, spans: vec![clip_span(line.clone(), TAB_TEXT_DIM, false, false)], clip, scale: 1.0, face: crate::render::DEFAULT_FACE });
        }
    }
    if let Some(y) = d.drop_y {
        frame.quads.push(Quad { rect: Rect { x: l.list.x, y: y - 1.0, w: l.list.w, h: 2.0 }, color: rgba(CLIP_SELECTED, 1.0) });
    }
    let e = d.enabled;
    for (b, r) in &l.buttons {
        let (glyph, on) = match b {
            Button::Add => ('\u{E710}', e.add),
            Button::Remove => ('\u{E738}', e.remove),
            Button::Archive => ('\u{E7B8}', e.archive),
            Button::Send => ('\u{E724}', e.send),
            Button::Copy => ('\u{E8C8}', e.copy),
            Button::Back => ('\u{E76B}', e.back),
            Button::Forward => ('\u{E76C}', e.forward),
            Button::Close => ('\u{E711}', true),
        };
        frame.quads.push(Quad { rect: *r, color: rgba(TAB_ACTIVE, 1.0) });
        let color = if on { TAB_TEXT } else { CLIP_DISABLED };
        frame.rows.push(TextRow { x: r.x + (r.w - cell.0) / 2.0, y: r.y + (r.h - cell.1) / 2.0, spans: vec![clip_span(glyph.to_string(), color, false, true)], clip: *r, scale: 1.0, face: crate::render::DEFAULT_FACE });
    }
    if let Some(text) = d.status {
        let r = l.status;
        frame.rows.push(TextRow { x: r.x, y: r.y + (r.h - cell.1) / 2.0, spans: vec![clip_span(text.to_string(), TAB_TEXT_DIM, false, false)], clip: r, scale: 1.0, face: crate::render::DEFAULT_FACE });
    }
}

/// What the inline chat panel shows (Task 9: the toolbar and an empty band). The app fits the
/// title to its rectangle (`inline_chat::truncate_tail`); a hovered button's tip takes the
/// title's place.
pub struct InlineDraw<'a> {
    pub panel: Rect,
    pub layout: &'a crate::inline_chat::ToolbarLayout,
    pub title: &'a str,
    pub hover: Option<crate::inline_chat::Button>,
}

/// The inline chat panel at the right of a session: its toolbar band, the four glyph buttons,
/// the centered title, a separator under the toolbar and a leading hairline.
pub fn inline_chat_panel(frame: &mut Frame, d: &InlineDraw, cell: (f32, f32), scale: f32) {
    use crate::inline_chat::Button;
    let l = d.layout;
    frame.quads.push(Quad { rect: d.panel, color: rgba(CLIP_BG, 1.0) });
    frame.quads.push(Quad { rect: Rect { w: 1.0, ..d.panel }, color: rgba(DIVIDER, 1.0) });
    let band_h = crate::inline_chat::TOOLBAR_H * scale;
    frame.quads.push(Quad { rect: Rect { x: d.panel.x, y: d.panel.y + band_h - 1.0, w: d.panel.w, h: 1.0 }, color: rgba(DIVIDER, 1.0) });
    for (b, r) in [(Button::New, l.new), (Button::Switch, l.switch), (Button::Info, l.info), (Button::Hide, l.hide)] {
        if d.hover == Some(b) {
            frame.quads.push(Quad { rect: r, color: rgba(TAB_ACTIVE, 1.0) });
        }
        frame.rows.push(TextRow { x: r.x + (r.w - cell.0) / 2.0, y: r.y + (r.h - cell.1) / 2.0, spans: vec![clip_span(b.glyph().to_string(), TAB_TEXT, false, true)], clip: r, scale: 1.0, face: crate::render::DEFAULT_FACE });
    }
    let text_w = d.title.chars().count() as f32 * cell.0;
    let t = l.title;
    frame.rows.push(TextRow { x: t.x + ((t.w - text_w) / 2.0).max(0.0), y: t.y + (t.h - cell.1) / 2.0, spans: vec![clip_span(d.title.to_string(), TAB_TEXT, true, false)], clip: t, scale: 1.0, face: crate::render::DEFAULT_FACE });
}

/// One conversation row of the inline panel, placed in pixels already: its bubble (zero width for
/// a date or the typing row), its text rectangle (where `lines` start), its buttons as (rectangle,
/// title, the title's width, destructive, enabled) and the copy glyph of each code block.
pub struct InlineRowDraw<'a> {
    pub bubble: Rect,
    pub text: Rect,
    pub user: bool,
    pub lines: &'a [crate::porthole::render::VisualLine],
    /// A date row's label (centered in `text`).
    pub date: Option<&'a str>,
    pub typing: bool,
    pub buttons: Vec<(Rect, &'a str, f32, bool, bool)>,
    pub copies: Vec<Rect>,
    /// Where "Copied" shows, beside the copy glyph of the block just copied.
    pub copied: Option<Rect>,
}

/// The input band: its box, the attach and send glyph rectangles, the editor's lines already
/// laid out (x, y, text), the selection and caret, the placeholder while empty.
pub struct InlineInputDraw<'a> {
    pub rect: Rect,
    pub attach: Rect,
    pub send: Rect,
    pub send_glyph: char,
    pub send_enabled: bool,
    pub clip: Rect,
    pub lines: &'a [(f32, f32, String)],
    pub selection: &'a [Rect],
    pub caret: Option<Rect>,
    pub placeholder: Option<&'a str>,
    pub focused: bool,
    pub text_scale: f32,
}

/// What the inline panel's body shows under its toolbar.
pub struct InlineBodyDraw<'a> {
    /// The conversation's visible area: everything in the rows is cut to it.
    pub area: Rect,
    pub rows: Vec<InlineRowDraw<'a>>,
    /// The line above the input: "Copied" or the pending attachments' names.
    pub strip: Option<(Rect, &'a str)>,
    pub input: InlineInputDraw<'a>,
}

/// The user's bubble color (the accent) and the text on it.
const BUBBLE_USER: Rgb = LIST_SELECTED;

/// The inline panel's conversation and input: bubbles as quads (the person's in the accent color
/// with white text), each row's text by `doc_lines`, buttons as bordered rows with centered titles
/// (destructive in red, disabled at half alpha), the typing row as three dots, the date rows
/// centered, then the input band.
pub fn inline_chat_body(frame: &mut Frame, d: &InlineBodyDraw, cell: (f32, f32), scale: f32) {
    let agent_bubble = blend(CLIP_BG, rgb(128, 128, 128), 0.25);
    for row in &d.rows {
        if row.bubble.w > 0.0 {
            if let Some(r) = cut(row.bubble, d.area) {
                frame.quads.push(Quad { rect: r, color: rgba(if row.user { BUBBLE_USER } else { agent_bubble }, 1.0) });
            }
        }
        if let Some(label) = row.date {
            if let Some(c) = cut(row.text, d.area) {
                let w = label.chars().count() as f32 * cell.0 * 0.9;
                frame.rows.push(TextRow { x: row.text.x + ((row.text.w - w) / 2.0).max(0.0), y: row.text.y, spans: vec![clip_span(label.to_string(), TAB_TEXT_DIM, false, false)], clip: c, scale: 0.85, face: crate::render::DEFAULT_FACE });
            }
            continue;
        }
        if row.typing {
            let dot = (4.0 * scale).max(3.0);
            let (gap, total) = (dot, dot * 5.0);
            let x0 = row.text.x + (row.text.w - total) / 2.0;
            for i in 0..3 {
                let r = Rect { x: x0 + i as f32 * (dot + gap), y: row.text.y + (row.text.h - dot) / 2.0, w: dot, h: dot };
                if let Some(r) = cut(r, d.area) {
                    frame.quads.push(Quad { rect: r, color: rgba(TAB_TEXT_DIM, 1.0) });
                }
            }
            continue;
        }
        let (fg, back) = if row.user { (rgb(255, 255, 255), BUBBLE_USER) } else { (TAB_TEXT, agent_bubble) };
        let area = cut(row.text, d.area);
        if let Some(area) = area {
            // A rule stops short of the text's right edge by `PORTHOLE_MARGIN`.
            doc_lines(frame, (row.text.x, row.text.y), row.lines, &[], area, (row.text.x, row.text.w + PORTHOLE_MARGIN), back, fg);
        }
        for r in &row.copies {
            if let Some(c) = cut(*r, d.area) {
                frame.quads.push(Quad { rect: c, color: rgba(blend(back, fg, 0.15), 1.0) });
                frame.rows.push(TextRow { x: r.x + (r.w - cell.0) / 2.0, y: r.y + (r.h - cell.1) / 2.0, spans: vec![clip_span(crate::chat_view::COPY_GLYPH.to_string(), fg, false, true)], clip: c, scale: 1.0, face: crate::render::DEFAULT_FACE });
            }
        }
        if let Some(r) = row.copied {
            if let Some(c) = cut(r, d.area) {
                frame.rows.push(TextRow { x: r.x, y: r.y + (r.h - cell.1) / 2.0, spans: vec![clip_span(crate::chat_view::COPIED.to_string(), blend(back, fg, 0.6), false, false)], clip: c, scale: 0.85, face: crate::render::DEFAULT_FACE });
            }
        }
        for (r, title, title_w, destructive, enabled) in &row.buttons {
            let Some(c) = cut(*r, d.area) else { continue };
            let alpha = if *enabled { 1.0 } else { 0.5 };
            let color = if *destructive { rgb(0xff, 0x45, 0x3a) } else { fg };
            // The separator above the button, then its title centered.
            frame.quads.push(Quad { rect: Rect { h: 1.0, ..c }, color: rgba(color, 0.35 * alpha) });
            frame_lines(&mut frame.quads, c, rgba(color, 0.25 * alpha));
            frame.rows.push(TextRow { x: r.x + ((r.w - title_w) / 2.0).max(0.0), y: r.y + (r.h - cell.1) / 2.0, spans: vec![clip_span((*title).to_string(), blend(back, color, alpha), false, false)], clip: c, scale: 1.0, face: crate::render::DEFAULT_FACE });
        }
    }
    if let Some((r, text)) = d.strip {
        frame.rows.push(TextRow { x: r.x, y: r.y + (r.h - cell.1) / 2.0, spans: vec![clip_span(text.to_string(), TAB_TEXT_DIM, false, false)], clip: r, scale: 0.85, face: crate::render::DEFAULT_FACE });
    }
    let i = &d.input;
    frame.quads.push(Quad { rect: i.rect, color: rgba(FIELD_BG_DARK, 1.0) });
    frame_lines(&mut frame.quads, i.rect, if i.focused { rgba(LIST_SELECTED, 1.0) } else { rgba(DIVIDER, 1.0) });
    for r in i.selection {
        let r = clip_rect(*r, i.clip);
        if r.w > 0.0 && r.h > 0.0 {
            frame.quads.push(Quad { rect: r, color: rgba(LIST_SELECTED, NOTE_SELECTION_ALPHA) });
        }
    }
    for (x, y, line) in i.lines {
        if !line.is_empty() {
            frame.rows.push(TextRow { x: *x, y: *y, spans: vec![list_span(line.clone(), TAB_TEXT, false, 0)], clip: i.clip, scale: i.text_scale, face: crate::render::DEFAULT_FACE });
        }
    }
    if let (Some(text), true) = (i.placeholder, i.lines.iter().all(|l| l.2.is_empty())) {
        frame.rows.push(TextRow { x: i.clip.x, y: i.clip.y, spans: vec![list_span(text.to_string(), TAB_TEXT_DIM, false, 0)], clip: i.clip, scale: i.text_scale, face: crate::render::DEFAULT_FACE });
    }
    if let Some(c) = i.caret.map(|c| clip_rect(c, i.clip)).filter(|c| c.h > 0.0) {
        frame.quads.push(Quad { rect: Rect { w: scale.max(1.0), ..c }, color: rgba(TAB_TEXT, 1.0) });
    }
    for (r, glyph, on) in [(i.attach, '\u{E710}', true), (i.send, i.send_glyph, i.send_enabled)] {
        let color = if on { TAB_TEXT } else { TAB_TEXT_DIM };
        frame.rows.push(TextRow { x: r.x + (r.w - cell.0) / 2.0, y: r.y + (r.h - cell.1) / 2.0, spans: vec![clip_span(glyph.to_string(), color, false, true)], clip: r, scale: 1.0, face: crate::render::DEFAULT_FACE });
    }
}

/// A clipping's preview over the terminal: its title and all of its detail that fits.
pub fn clip_preview(frame: &mut Frame, area: Rect, title: &str, lines: &[String], cell: (f32, f32), scale: f32) {
    let w = (700.0 * scale).min(area.w - 2.0 * cell.0).max(cell.0 * 10.0);
    let h = ((lines.len() + 2) as f32 * cell.1 + 16.0 * scale).min(480.0 * scale).min(area.h - cell.1);
    let r = Rect { x: area.x + (area.w - w) / 2.0, y: area.y + (area.h - h) / 2.0, w, h };
    cut_out(frame, r, cell.1);
    frame.popups.push(Quad { rect: r, color: rgba(CLIP_BG, 1.0) });
    let x = r.x + 12.0 * scale;
    let mut y = r.y + 8.0 * scale;
    frame.rows.push(TextRow { x, y, spans: vec![clip_span(title.to_string(), TAB_TEXT, true, false)], clip: r, scale: 1.0, face: crate::render::DEFAULT_FACE });
    y += cell.1 * 1.5;
    for line in lines {
        if y + cell.1 > r.y + r.h {
            break;
        }
        frame.rows.push(TextRow { x, y, spans: vec![clip_span(line.clone(), TAB_TEXT, false, false)], clip: r, scale: 1.0, face: crate::render::DEFAULT_FACE });
        y += cell.1;
    }
}

/// What the toolbelt panel shows. The app measures the text (the scene cannot): `titles` are
/// already tail truncated to their title rectangles, with their widths in `title_widths`, and
/// `empty` is `NO_TOOLS` wrapped to the empty label's width, each line with its width.
pub struct BeltDraw<'a> {
    pub layout: &'a crate::toolbelt_layout::BeltLayout,
    pub titles: &'a [&'a str],
    pub dark: bool,
    pub title_widths: &'a [f32],
    pub empty: &'a [(String, f32)],
    /// The rows' text size against the terminal font, for 13 point text.
    pub text_scale: f32,
}

/// The panel's background: iTerm2's sidebar is clear in dark (Tandem has no vibrancy, so a
/// sidebar gray) and white in light.
const BELT_BG_DARK: Rgb = rgb(0x25, 0x25, 0x28);
const BELT_BG_LIGHT: Rgb = rgb(0xff, 0xff, 0xff);
/// Dividers: white 122/255 at alpha 0.25 (`TBV:338`).
const BELT_DIVIDER: Rgb = rgb(122, 122, 122);
const BELT_TEXT_LIGHT: Rgb = rgb(0x1e, 0x1e, 0x1e);
const BELT_ICON_LIGHT: Rgb = rgb(0x6e, 0x6e, 0x6e);
/// The close glyph (Segoe Fluent Icons Cancel) and the hamburger (More).
pub const BELT_CLOSE_GLYPH: char = '\u{E711}';
pub const BELT_MENU_GLYPH: char = '\u{E712}';

fn belt_span(text: String, color: Rgb, icon: bool) -> Span {
    Span { text, color, bold: false, italic: false, alt_font: false, icon, prose: !icon, badge: false, toolbelt: false, fake_bold: false, ligatures: 0, mask: 0, font: 0 }
}

/// The toolbelt at the right of a window: the background, the dividers between wrappers, each
/// wrapper's close glyph and centered title, the hamburger, and the empty label. Tools draw
/// their own content (`scene::tool_list`) into each wrapper's content rectangle.
pub fn toolbelt(frame: &mut Frame, d: &BeltDraw, cell: (f32, f32), scale: f32) {
    let l = d.layout;
    let _ = scale;
    let bg = if d.dark { BELT_BG_DARK } else { BELT_BG_LIGHT };
    let (text, icon) = if d.dark { (TAB_TEXT, TAB_TEXT_DIM) } else { (BELT_TEXT_LIGHT, BELT_ICON_LIGHT) };
    frame.quads.push(Quad { rect: l.panel, color: rgba(bg, 1.0) });
    for r in &l.dividers {
        frame.quads.push(Quad { rect: *r, color: rgba(BELT_DIVIDER, 0.25) });
    }
    let line_h = cell.1 * d.text_scale;
    // Glyph width: the font's size, a line being 1.2 of it.
    let glyph_w = line_h / 1.2;
    let glyph = |frame: &mut Frame, r: Rect, c: char| {
        frame.rows.push(TextRow { x: r.x + (r.w - glyph_w) / 2.0, y: r.y + (r.h - line_h) / 2.0, spans: vec![belt_span(c.to_string(), icon, true)], clip: r, scale: d.text_scale, face: crate::render::DEFAULT_FACE });
    };
    for (i, w) in l.wrappers.iter().enumerate() {
        glyph(frame, w.close, BELT_CLOSE_GLYPH);
        let (Some(title), Some(&width)) = (d.titles.get(i), d.title_widths.get(i)) else { continue };
        let r = w.title;
        if r.w <= 0.0 || title.is_empty() {
            continue;
        }
        let x = r.x + ((r.w - width) / 2.0).max(0.0);
        let clip = Rect { y: w.frame.y, h: w.frame.h.min(r.y - w.frame.y + r.h.max(line_h)), ..r };
        frame.rows.push(TextRow { x, y: r.y + (r.h - line_h) / 2.0, spans: vec![belt_span(title.to_string(), text, false)], clip, scale: d.text_scale, face: crate::render::DEFAULT_FACE });
    }
    glyph(frame, l.hamburger, BELT_MENU_GLYPH);
    if let Some(r) = l.empty_label {
        // Centered in the room below the 22 point band, or not at all when it does not fit
        // (`TBV:382-398`).
        let h = d.empty.len() as f32 * line_h;
        if h <= r.h {
            let mut y = r.y + (r.h - h) / 2.0;
            for (line, width) in d.empty {
                let x = r.x + ((r.w - width) / 2.0).max(0.0);
                frame.rows.push(TextRow { x, y, spans: vec![belt_span(line.clone(), text, false)], clip: r, scale: d.text_scale, face: crate::render::DEFAULT_FACE });
                y += line_h;
            }
        }
    }
}

/// One row of a toolbelt tool's list, as the app measured it: `text` and `second` (a smaller
/// second line, Profiles' tags) are already tail truncated to the row.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ListRow {
    pub text: String,
    pub second: Option<String>,
    pub bold: bool,
    /// A glyph (icon font) before the text, in its color.
    pub icon: Option<(char, Rgb)>,
    /// Outline depth (Snippets' folders).
    pub depth: usize,
    /// A folder's disclosure glyph, before the icon.
    pub disclosure: Option<char>,
    /// Tag chips on a second line (Snippets): each text and its width at `CHIP_SCALE`.
    pub chips: Vec<(String, f32)>,
    /// Text right-aligned on the first line in the tertiary color, and its width (Session
    /// Status' shortcut).
    pub right: Option<(String, f32)>,
    /// Lines under the first, each one line tall. With `status_color`, the first of them is a
    /// status line in that color at the text's x (no room when it is empty) and the rest are
    /// in the secondary color `lines_indent` further in; without, all are in the text color
    /// at `lines_indent` (Captured Output's wrapped label).
    pub lines: Vec<String>,
    /// Points.
    pub lines_indent: f32,
    /// The text drawn at half strength (a snoozed row); the lead glyphs keep theirs.
    pub dim: bool,
    /// A 1 pixel rule along the row's top (above the first snoozed row).
    pub rule_above: bool,
    pub status_color: Option<Rgb>,
    /// Glyphs (icon font) before the text in their colors, each in its own slot
    /// (`lead_width`): Session Status' snooze, bell and indicator dot.
    pub lead: Vec<(char, Rgb)>,
}

/// Session Status' indicator dot (Segoe Fluent Icons CircleFill), drawn 6 points across in a
/// 10 point slot; every other lead glyph gets a 12 point slot. Each slot has its spacing after
/// it: 4 points after the dot, 3 after the others (`ToolStatusCellView.swift:42-54`).
pub const STATUS_DOT: char = '\u{EA3B}';
const DOT_SLOT: f32 = 10.0;
const DOT_GAP: f32 = 4.0;
const DOT_PT: f32 = 6.0;
const LEAD_SLOT: f32 = 12.0;
const LEAD_GAP: f32 = 3.0;
/// The snooze and bell symbols' size (`SymbolConfiguration(pointSize: 10)`).
const LEAD_PT: f32 = 10.0;

/// The room a row's lead glyphs take before its text, pixels.
pub fn lead_width(lead: &[(char, Rgb)], scale: f32) -> f32 {
    lead.iter().map(|(c, _)| if *c == STATUS_DOT { DOT_SLOT + DOT_GAP } else { LEAD_SLOT + LEAD_GAP }).sum::<f32>() * scale
}

/// The lists' secondary text color (Session Status' detail, and a status without a color of
/// its own).
pub fn list_secondary(dark: bool) -> Rgb {
    if dark { TAB_TEXT_DIM } else { BELT_ICON_LIGHT }
}

/// What a toolbelt tool's list widget shows. The app measures the text (the scene cannot).
pub struct ListDraw<'a> {
    pub layout: &'a crate::tool_list::ListLayout,
    /// The rows that show, from row `first` on (the app builds only those).
    pub rows: &'a [ListRow],
    pub first: usize,
    pub selected: &'a [usize],
    pub dark: bool,
    /// The field's text as shown (its end, when it is wider than the field) and that text's
    /// width, for the caret.
    pub field: Option<(&'a str, f32)>,
    /// The part of the tool that has the keyboard.
    pub focus: Option<crate::tool_list::Focus>,
    /// Buttons drawn dimmed (they do nothing now).
    pub disabled: &'a [crate::tool_list::ToolButton],
    /// The popup's label and whether it is enabled.
    pub popup: Option<(&'a str, bool)>,
    /// The second line's text size against the rows'.
    pub second_scale: f32,
    /// A row drag's drop line, at this y.
    pub drop_y: Option<f32>,
    /// Toggle buttons that are on (Session Status' armed bell and snoozed row), drawn in the
    /// selection color.
    pub lit: &'a [crate::tool_list::ToolButton],
}

/// Snippets' tag chips: white on orange, at 0.8 of the toolbelt font (`TS:677-681`).
pub const CHIP_SCALE: f32 = 0.8;
pub const CHIP_BG: Rgb = rgb(247, 120, 26);
/// The room between two chips, points.
pub const CHIP_GAP: f32 = 4.0;

/// A selected row (17, 108, 214, the brief's selection color).
pub const LIST_SELECTED: Rgb = rgb(17, 108, 214);
const FIELD_BG_DARK: Rgb = rgb(0x1e, 0x1e, 0x1e);
const FIELD_BG_LIGHT: Rgb = rgb(0xf3, 0xf3, 0xf3);
const LIST_DISABLED_DARK: Rgb = rgb(0x5a, 0x5a, 0x5a);
const LIST_DISABLED_LIGHT: Rgb = rgb(0xb4, 0xb4, 0xb4);
/// A row's cell inset (`NSTableView+iTerm.m:65-69`) and an outline level's indent, in points.
pub const LIST_INSET: f32 = 2.0;
pub const LIST_INDENT: f32 = 16.0;
/// The popup's disclosure glyph (Segoe Fluent Icons ChevronDown).
const POPUP_GLYPH: char = '\u{E70D}';

fn list_span(text: String, color: Rgb, bold: bool, font: u8) -> Span {
    // font: 0 the toolbelt font, 1 the prose font, 2 the icon font.
    Span { text, color, bold, italic: false, alt_font: false, icon: font == 2, prose: font == 1, badge: false, toolbelt: font == 0, fake_bold: false, ligatures: 0, mask: 0, font: 0 }
}

/// A 1 pixel frame inside `r`.
fn frame_lines(quads: &mut Vec<Quad>, r: Rect, color: [f32; 4]) {
    quads.push(Quad { rect: Rect { h: 1.0, ..r }, color });
    quads.push(Quad { rect: Rect { y: r.y + r.h - 1.0, h: 1.0, ..r }, color });
    quads.push(Quad { rect: Rect { w: 1.0, ..r }, color });
    quads.push(Quad { rect: Rect { x: r.x + r.w - 1.0, w: 1.0, ..r }, color });
}

/// A toolbelt tool's list widget (`tool_list::layout`): the search field with its caret, the
/// visible rows with the selection, the top and bottom buttons, and the popup. `text_scale` is
/// the toolbelt font's size against the terminal font.
pub fn tool_list(frame: &mut Frame, d: &ListDraw, cell: (f32, f32), text_scale: f32, scale: f32) {
    use crate::tool_list::Focus;
    let l = d.layout;
    let (text, dim, disabled, field_bg) = if d.dark { (TAB_TEXT, TAB_TEXT_DIM, LIST_DISABLED_DARK, FIELD_BG_DARK) } else { (BELT_TEXT_LIGHT, BELT_ICON_LIGHT, LIST_DISABLED_LIGHT, FIELD_BG_LIGHT) };
    let line_h = cell.1 * text_scale;
    let glyph_w = line_h / 1.2;
    let inset = LIST_INSET * scale;

    if let (Some(r), Some((shown, width))) = (l.field, d.field) {
        frame.quads.push(Quad { rect: r, color: rgba(field_bg, 1.0) });
        let focused = d.focus == Some(Focus::Field);
        frame_lines(&mut frame.quads, r, if focused { rgba(LIST_SELECTED, 1.0) } else { rgba(DIVIDER, if d.dark { 1.0 } else { 0.4 }) });
        let x = r.x + 4.0 * scale;
        let y = r.y + (r.h - line_h) / 2.0;
        let clip = Rect { x: r.x + 1.0, w: (r.w - 2.0).max(0.0), ..r };
        if !shown.is_empty() {
            frame.rows.push(TextRow { x, y, spans: vec![list_span(shown.to_string(), text, false, 0)], clip, scale: text_scale, face: crate::render::DEFAULT_FACE });
        }
        if focused {
            let cx = (x + width).min(r.x + r.w - 2.0);
            frame.quads.push(Quad { rect: Rect { x: cx, y, w: scale.max(1.0), h: line_h }, color: rgba(text, 1.0) });
        }
    }

    let list = l.list;
    for (i, (row, r)) in d.rows.iter().zip(l.rows.iter().skip(d.first)).enumerate() {
        let i = d.first + i;
        if r.y >= list.y + list.h || r.y + r.h <= list.y {
            continue;
        }
        let top = r.y.max(list.y);
        let clip = Rect { x: r.x, y: top, w: r.w, h: (r.y + r.h).min(list.y + list.h) - top };
        let selected = d.selected.contains(&i);
        if selected {
            frame.quads.push(Quad { rect: clip, color: rgba(LIST_SELECTED, 1.0) });
        }
        let (fg, fg2) = if selected { (rgb(0xff, 0xff, 0xff), rgb(0xe6, 0xe6, 0xe6)) } else { (text, dim) };
        let bg = if selected { LIST_SELECTED } else if d.dark { BELT_BG_DARK } else { BELT_BG_LIGHT };
        // Half strength over the row's background (a snoozed row, `setTextAlpha(0.5)`).
        let soft = |c: Rgb| if row.dim { blend(c, bg, 0.5) } else { c };
        let (fg, fg2) = (soft(fg), soft(fg2));
        if row.rule_above {
            frame.quads.push(Quad { rect: Rect { x: r.x, y: r.y, w: r.w, h: 1.0 }, color: rgba(DIVIDER, if d.dark { 1.0 } else { 0.4 }) });
        }
        let mut x = r.x + inset + row.depth as f32 * LIST_INDENT * scale;
        let y = r.y + inset;
        for &(c, color) in &row.lead {
            let color = if selected { rgb(0xff, 0xff, 0xff) } else { color };
            // Each glyph drawn `size` across, centered in its slot on the first line (a line at
            // scale `s` is `cell.1 * s` tall, a glyph `1 / 1.2` of that).
            let (slot, gap, size) = if c == STATUS_DOT { (DOT_SLOT, DOT_GAP, DOT_PT) } else { (LEAD_SLOT, LEAD_GAP, LEAD_PT) };
            let s = size * scale * 1.2 / cell.1.max(1.0);
            let gh = cell.1 * s;
            frame.rows.push(TextRow { x: x + (slot * scale - gh / 1.2) / 2.0, y: y + (line_h - gh) / 2.0, spans: vec![list_span(c.to_string(), color, false, 2)], clip, scale: s, face: crate::render::DEFAULT_FACE });
            x += (slot + gap) * scale;
        }
        if let Some((right, w)) = &row.right {
            // The tertiary color; on the selection, white at 0.7 (`updateShortcutColor`).
            let color = if selected { blend(rgb(0xff, 0xff, 0xff), LIST_SELECTED, 0.3) } else { soft(blend(dim, bg, 0.35)) };
            frame.rows.push(TextRow { x: (r.x + r.w - inset - w).max(x), y, spans: vec![list_span(right.clone(), color, false, 0)], clip, scale: text_scale, face: crate::render::DEFAULT_FACE });
        }
        if let Some(c) = row.disclosure {
            frame.rows.push(TextRow { x, y, spans: vec![list_span(c.to_string(), fg2, false, 2)], clip, scale: text_scale, face: crate::render::DEFAULT_FACE });
            x += glyph_w + 2.0 * scale;
        }
        if let Some((c, color)) = row.icon {
            frame.rows.push(TextRow { x, y, spans: vec![list_span(c.to_string(), if selected { fg } else { color }, false, 2)], clip, scale: text_scale, face: crate::render::DEFAULT_FACE });
            x += glyph_w + 4.0 * scale;
        }
        frame.rows.push(TextRow { x, y, spans: vec![list_span(row.text.clone(), fg, row.bold, 0)], clip, scale: text_scale, face: crate::render::DEFAULT_FACE });
        if let Some(second) = &row.second {
            frame.rows.push(TextRow { x, y: y + line_h, spans: vec![list_span(second.clone(), fg2, false, 1)], clip, scale: text_scale * d.second_scale, face: crate::render::DEFAULT_FACE });
        }
        let mut ly = y + line_h;
        for (k, line) in row.lines.iter().enumerate() {
            let (lx, color) = match row.status_color {
                Some(c) if k == 0 => {
                    if line.is_empty() {
                        continue;
                    }
                    (x, if selected { fg } else { soft(c) })
                }
                Some(_) => (x + row.lines_indent * scale, fg2),
                None => (x + row.lines_indent * scale, fg),
            };
            frame.rows.push(TextRow { x: lx, y: ly, spans: vec![list_span(line.clone(), color, false, 0)], clip, scale: text_scale, face: crate::render::DEFAULT_FACE });
            ly += line_h;
        }
        let mut cx = x;
        for (tag, w) in &row.chips {
            let chip = Rect { x: cx, y: y + line_h, w: *w, h: line_h * CHIP_SCALE };
            let shown = Rect { w: chip.w.min((clip.x + clip.w - cx).max(0.0)), ..chip };
            frame.quads.push(Quad { rect: shown, color: rgba(CHIP_BG, 1.0) });
            frame.rows.push(TextRow { x: cx, y: chip.y, spans: vec![list_span(tag.clone(), rgb(0xff, 0xff, 0xff), false, 0)], clip, scale: text_scale * CHIP_SCALE, face: crate::render::DEFAULT_FACE });
            cx += w + CHIP_GAP * scale;
        }
    }

    if let Some(y) = d.drop_y.filter(|y| *y >= list.y - 1.0 && *y <= list.y + list.h + 1.0) {
        let h = (2.0 * scale).max(1.0);
        frame.quads.push(Quad { rect: Rect { x: list.x, y: (y - h / 2.0).clamp(list.y, (list.y + list.h - h).max(list.y)), w: list.w, h }, color: rgba(LIST_SELECTED, 1.0) });
    }

    for (b, r) in l.top.iter().chain(&l.buttons) {
        let color = if d.disabled.contains(b) {
            disabled
        } else if d.lit.contains(b) {
            LIST_SELECTED
        } else {
            dim
        };
        frame.rows.push(TextRow { x: r.x + (r.w - glyph_w) / 2.0, y: r.y + (r.h - line_h) / 2.0, spans: vec![list_span(b.glyph().to_string(), color, false, 2)], clip: *r, scale: text_scale, face: crate::render::DEFAULT_FACE });
    }

    if let (Some(r), Some((label, on))) = (l.popup, d.popup) {
        frame.quads.push(Quad { rect: r, color: rgba(field_bg, 1.0) });
        frame_lines(&mut frame.quads, r, rgba(DIVIDER, if d.dark { 1.0 } else { 0.4 }));
        let color = if on { text } else { disabled };
        let y = r.y + (r.h - line_h) / 2.0;
        let glyph_x = r.x + r.w - glyph_w - 6.0 * scale;
        let clip = Rect { w: (glyph_x - r.x).max(0.0), ..r };
        frame.rows.push(TextRow { x: r.x + 6.0 * scale, y, spans: vec![list_span(label.to_string(), color, false, 1)], clip, scale: text_scale, face: crate::render::DEFAULT_FACE });
        frame.rows.push(TextRow { x: glyph_x, y, spans: vec![list_span(POPUP_GLYPH.to_string(), color, false, 2)], clip: r, scale: text_scale, face: crate::render::DEFAULT_FACE });
    }
}

/// What the Notes tool shows (`TN`): the mode control's segments (each rectangle, label and
/// label width; `selected` is lit), the editor's frame with a 1 pixel border, its text lines
/// (x, y and text, already laid out and scrolled), the selection's rectangles, the caret while
/// it has the keyboard, and the scroller's thumb when the text is taller than the editor.
pub struct NoteDraw<'a> {
    pub segments: &'a [(Rect, &'a str, f32)],
    pub selected: usize,
    pub frame: Rect,
    /// The text's room inside the frame: lines, selection and caret are clipped to it.
    pub clip: Rect,
    pub lines: &'a [(f32, f32, String)],
    pub selection: &'a [Rect],
    pub caret: Option<Rect>,
    pub focused: bool,
    pub thumb: Option<Rect>,
    pub dark: bool,
    /// The segments' labels' size against the terminal font (the prose font).
    pub label_scale: f32,
}

/// The Notes selection: the brief's 17, 108, 214 at alpha 0.4.
pub const NOTE_SELECTION_ALPHA: f32 = 0.4;

/// A rectangle clipped to `clip` (zero-sized when outside it).
fn clip_rect(r: Rect, clip: Rect) -> Rect {
    let x = r.x.max(clip.x);
    let y = r.y.max(clip.y);
    let w = ((r.x + r.w).min(clip.x + clip.w) - x).max(0.0);
    let h = ((r.y + r.h).min(clip.y + clip.h) - y).max(0.0);
    Rect { x, y, w, h }
}

/// The Notes tool: the "Global" / "Session" segmented control, then the plain-text editor in
/// the toolbelt font (`NoteDraw`). `text_scale` is the toolbelt font against the terminal's.
pub fn note_editor(frame: &mut Frame, d: &NoteDraw, cell: (f32, f32), text_scale: f32, scale: f32) {
    let (text, field_bg) = if d.dark { (TAB_TEXT, FIELD_BG_DARK) } else { (BELT_TEXT_LIGHT, FIELD_BG_LIGHT) };
    let edge = rgba(DIVIDER, if d.dark { 1.0 } else { 0.4 });
    let label_h = cell.1 * d.label_scale;
    for (i, (r, label, w)) in d.segments.iter().enumerate() {
        let on = i == d.selected;
        frame.quads.push(Quad { rect: *r, color: rgba(if on { LIST_SELECTED } else { field_bg }, 1.0) });
        frame_lines(&mut frame.quads, *r, edge);
        let color = if on { rgb(0xff, 0xff, 0xff) } else { text };
        frame.rows.push(TextRow { x: r.x + ((r.w - w) / 2.0).max(0.0), y: r.y + (r.h - label_h) / 2.0, spans: vec![list_span(label.to_string(), color, false, 1)], clip: *r, scale: d.label_scale, face: crate::render::DEFAULT_FACE });
    }
    frame.quads.push(Quad { rect: d.frame, color: rgba(field_bg, 1.0) });
    frame_lines(&mut frame.quads, d.frame, if d.focused { rgba(LIST_SELECTED, 1.0) } else { edge });
    for r in d.selection {
        let r = clip_rect(*r, d.clip);
        if r.w > 0.0 && r.h > 0.0 {
            frame.quads.push(Quad { rect: r, color: rgba(LIST_SELECTED, NOTE_SELECTION_ALPHA) });
        }
    }
    for (x, y, line) in d.lines {
        if !line.is_empty() {
            frame.rows.push(TextRow { x: *x, y: *y, spans: vec![list_span(line.clone(), text, false, 0)], clip: d.clip, scale: text_scale, face: crate::render::DEFAULT_FACE });
        }
    }
    if let Some(c) = d.caret.map(|c| clip_rect(c, d.clip)).filter(|c| c.h > 0.0) {
        frame.quads.push(Quad { rect: Rect { w: scale.max(1.0), ..c }, color: rgba(text, 1.0) });
    }
    if let Some(t) = d.thumb {
        frame.quads.push(Quad { rect: t, color: rgba(TAB_TEXT_DIM, 0.6) });
    }
}

/// What the minimal composer shows (milestone 9l, `MCVC`/`MCXIB`): its panel (rounded, a 1
/// pixel gray border, the session's background), the editor's text lines already laid out,
/// wrapped, scrolled and colored in the pane's font (x, y, colored runs), the selection's
/// rectangles, the caret in the cursor color while it has the keyboard, the close glyph at the
/// top right, and the accessory row: the Help glyph, "AI" (when allowed) and the send tip
/// (x, y). `alpha` is the fade; `under` what the panel fades over (text has no alpha of its
/// own).
pub struct ComposerDraw<'a> {
    pub panel: Rect,
    pub radius: f32,
    pub alpha: f32,
    pub bg: Rgb,
    pub under: Rgb,
    pub fg: Rgb,
    pub clip: Rect,
    pub lines: &'a [(f32, f32, Vec<(String, Rgb, bool)>)],
    pub selection: &'a [Rect],
    pub caret: Option<(Rect, Rgb)>,
    pub close: Rect,
    pub help: Rect,
    pub ai: Option<Rect>,
    pub tip: Option<(f32, f32, &'a str)>,
    /// The AI completion warning left of the tip (`LCXIB:98`).
    pub warning: Option<(f32, f32, &'a str)>,
    /// The Help button's tooltip while the pointer is over it.
    pub tooltip: Option<&'a str>,
    /// The accessory row's text size against the pane's font.
    pub small: f32,
    pub line_h: f32,
    /// The Auto Composer: a plain box with this line (color, height) along its top, and none of
    /// the dropdown's border, buttons or tip.
    pub separator: Option<([f32; 4], f32)>,
    /// Backgrounds behind parts of the text (the Auto Composer's prompt).
    pub backs: &'a [(Rect, Rgb)],
}

/// The composer's gray border (`MCVC:154-156`, `NSColor.grayColor`).
pub const COMPOSER_BORDER: Rgb = rgb(0x80, 0x80, 0x80);
/// The close glyph (Segoe Fluent Icons Cancel) and the Help glyph (Help).
pub const COMPOSER_CLOSE_GLYPH: char = '\u{E711}';
pub const COMPOSER_HELP_GLYPH: char = '\u{E897}';


fn plain_span(text: String, color: Rgb, bold: bool) -> Span {
    Span { text, color, bold, italic: false, alt_font: false, icon: false, prose: false, badge: false, toolbelt: false, fake_bold: false, ligatures: 0, mask: 0, font: 0 }
}

/// The minimal composer over its pane (`ComposerDraw`). The panel is a popup box: the pane's
/// text under it is cut out and the box goes in `popups`, with the selection and the caret on
/// it; its own text rows come after.
/// Draws the composer and returns its panel (None when nothing was drawn).
pub fn composer(frame: &mut Frame, d: &ComposerDraw, cell: (f32, f32)) -> Option<Rect> {
    let a = d.alpha.clamp(0.0, 1.0);
    if a <= 0.0 || d.panel.w <= 2.0 || d.panel.h <= 2.0 {
        return None;
    }
    cut_out(frame, d.panel, d.line_h);
    let inner = Rect { x: d.panel.x + 1.0, y: d.panel.y + 1.0, w: d.panel.w - 2.0, h: d.panel.h - 2.0 };
    match d.separator {
        Some((color, h)) => {
            frame.popups.push(Quad { rect: d.panel, color: rgba(d.bg, a) });
            frame.popups.push(Quad { rect: Rect { h, ..d.panel }, color: [color[0], color[1], color[2], color[3] * a] });
        }
        None => {
            rounded_box(&mut frame.popups, d.panel, d.radius, rgba(COMPOSER_BORDER, a));
            rounded_box(&mut frame.popups, inner, (d.radius - 1.0).max(0.0), rgba(d.bg, a));
        }
    }
    for (r, color) in d.backs {
        let r = clip_rect(*r, d.clip);
        if r.w > 0.0 && r.h > 0.0 {
            frame.popups.push(Quad { rect: r, color: rgba(*color, a) });
        }
    }
    for r in d.selection {
        let r = clip_rect(*r, d.clip);
        if r.w > 0.0 && r.h > 0.0 {
            frame.popups.push(Quad { rect: r, color: rgba(LIST_SELECTED, NOTE_SELECTION_ALPHA * a) });
        }
    }
    if let Some((c, color)) = d.caret {
        let c = clip_rect(c, d.clip);
        if c.h > 0.0 && c.w > 0.0 {
            frame.popups.push(Quad { rect: c, color: rgba(color, a) });
        }
    }
    let under = |c: Rgb| blend(d.under, c, a);
    for (x, y, runs) in d.lines {
        let spans: Vec<Span> = runs.iter().filter(|(t, _, _)| !t.is_empty()).map(|(t, c, b)| plain_span(t.clone(), under(*c), *b)).collect();
        if !spans.is_empty() {
            frame.rows.push(TextRow { x: *x, y: *y, spans, clip: d.clip, scale: 1.0, face: crate::render::DEFAULT_FACE });
        }
    }
    if d.separator.is_some() {
        return Some(d.panel);
    }
    let icon = |frame: &mut Frame, r: Rect, glyph: char| {
        let (w, h) = (cell.0, cell.1);
        let span = Span { text: glyph.to_string(), color: under(d.fg), bold: false, italic: false, alt_font: false, icon: true, prose: false, badge: false, toolbelt: false, fake_bold: false, ligatures: 0, mask: 0, font: 0 };
        frame.rows.push(TextRow { x: r.x + ((r.w - w) / 2.0).max(0.0), y: r.y + ((r.h - h) / 2.0).max(0.0), spans: vec![span], clip: r, scale: 1.0, face: crate::render::DEFAULT_FACE });
    };
    icon(frame, d.close, COMPOSER_CLOSE_GLYPH);
    icon(frame, d.help, COMPOSER_HELP_GLYPH);
    let small_h = cell.1 * d.small;
    if let Some(r) = d.ai {
        let w = 2.0 * cell.0 * d.small;
        frame.rows.push(TextRow { x: r.x + ((r.w - w) / 2.0).max(0.0), y: r.y + ((r.h - small_h) / 2.0).max(0.0), spans: vec![plain_span("AI".into(), under(d.fg), false)], clip: r, scale: d.small, face: crate::render::DEFAULT_FACE });
    }
    if let Some((x, y, text)) = d.warning {
        frame.rows.push(TextRow { x, y, spans: vec![plain_span(text.to_string(), under(TAB_TEXT_DIM), false)], clip: inner, scale: d.small, face: crate::render::DEFAULT_FACE });
    }
    if let Some((x, y, tip)) = d.tip {
        frame.rows.push(TextRow { x, y, spans: vec![plain_span(tip.to_string(), under(TAB_TEXT_DIM), false)], clip: inner, scale: d.small, face: crate::render::DEFAULT_FACE });
    }
    // The tooltip above the Help button, a small box of its own over the text.
    if let Some(t) = d.tooltip {
        let (w, h) = ((t.chars().count() as f32 + 2.0) * cell.0 * d.small, cell.1 * d.small + 6.0);
        let r = Rect { x: d.help.x, y: (d.help.y - h - 2.0).max(d.panel.y), w, h };
        cut_out(frame, r, d.line_h);
        frame.popups.push(Quad { rect: r, color: rgba(FIND_BAR, a) });
        frame_lines(&mut frame.popups, r, rgba(DIVIDER, a));
        frame.rows.push(TextRow { x: r.x + cell.0 * d.small, y: r.y + 3.0, spans: vec![plain_span(t.to_string(), under(TAB_TEXT), false)], clip: r, scale: d.small, face: crate::render::DEFAULT_FACE });
    }
    Some(d.panel)
}

/// What the composer's completion popup shows (`CW:199-274`): rows of (text, the bold typed
/// part's byte range, the kind's glyph), the selected row, the selected item's detail strip,
/// or the "Thinking..." placeholder alone; placed at the caret (`caret`: its x, top and
/// bottom) inside `bounds`.
pub struct ComposerPopupDraw<'a> {
    pub caret: (f32, f32, f32),
    pub bounds: Rect,
    pub rows: &'a [(String, Option<std::ops::Range<usize>>, char)],
    pub selected: Option<usize>,
    pub detail: Option<&'a str>,
    pub placeholder: Option<&'a str>,
    pub dark: bool,
    pub scale: f32,
}

/// The popup's sizes in points (`CW`): a row and the space between rows, the width's range,
/// the most height it takes, the detail strip, the corner radius, and how far left of the
/// caret it starts.
pub const POPUP_ROW: f32 = 20.0;
pub const POPUP_SPACING: f32 = 2.0;
pub const POPUP_MIN_W: f32 = 300.0;
pub const POPUP_MAX_W: f32 = 500.0;
pub const POPUP_MAX_H: f32 = 200.0;
pub const POPUP_DETAIL: f32 = 30.0;
pub const POPUP_RADIUS: f32 = 8.0;
pub const POPUP_LEFT: f32 = 24.0;

/// The popup's frame and the first row shown, for drawing and hit tests: below the caret, or
/// above it (2 points up) when it would run past `bounds`' bottom; nudged into `bounds`
/// horizontally. `widest` is the widest row's width in pixels.
pub fn composer_popup_frame(caret: (f32, f32, f32), bounds: Rect, rows: usize, selected: Option<usize>, detail: bool, widest: f32, scale: f32) -> (Rect, usize) {
    let pitch = (POPUP_ROW + POPUP_SPACING) * scale;
    let strip = if detail { POPUP_DETAIL * scale } else { 0.0 };
    let max_rows = (((POPUP_MAX_H * scale - strip) / pitch).floor() as usize).max(1);
    let shown = rows.clamp(1, max_rows);
    let h = shown as f32 * pitch + POPUP_SPACING * scale + strip;
    let w = widest.clamp(POPUP_MIN_W * scale, POPUP_MAX_W * scale);
    let (x, top, bottom) = caret;
    let mut y = bottom;
    if y + h > bounds.y + bounds.h {
        y = top - 2.0 * scale - h;
    }
    let x = (x - POPUP_LEFT * scale).min(bounds.x + bounds.w - w).max(bounds.x);
    let first = selected.map_or(0, |s| s.saturating_sub(shown - 1)).min(rows.saturating_sub(shown));
    (Rect { x, y, w, h }, first)
}

/// Draws the popup (`ComposerPopupDraw`) as a popup box; returns its frame.
pub fn composer_popup(frame: &mut Frame, d: &ComposerPopupDraw, cell: (f32, f32)) -> Rect {
    let scale = d.scale;
    let n = if d.placeholder.is_some() { 1 } else { d.rows.len() };
    let widest = d.rows.iter().map(|(t, _, _)| (t.chars().count() as f32 + 3.0) * cell.0).fold(0.0, f32::max);
    let (r, first) = composer_popup_frame(d.caret, d.bounds, n, d.selected, d.detail.is_some(), widest, scale);
    let (bg, text) = if d.dark { (FIND_BAR, TAB_TEXT) } else { (rgb(0xff, 0xff, 0xff), BELT_TEXT_LIGHT) };
    cut_out(frame, r, cell.1);
    rounded_box(&mut frame.popups, r, POPUP_RADIUS * scale, rgba(DIVIDER, 1.0));
    let inner = Rect { x: r.x + 1.0, y: r.y + 1.0, w: r.w - 2.0, h: r.h - 2.0 };
    rounded_box(&mut frame.popups, inner, (POPUP_RADIUS * scale - 1.0).max(0.0), rgba(bg, 1.0));
    let pitch = (POPUP_ROW + POPUP_SPACING) * scale;
    let row_h = POPUP_ROW * scale;
    let text_y = |ry: f32| ry + ((row_h - cell.1) / 2.0).max(0.0);
    if let Some(p) = d.placeholder {
        let ry = r.y + POPUP_SPACING * scale;
        frame.rows.push(TextRow { x: r.x + 8.0 * scale, y: text_y(ry), spans: vec![plain_span(p.to_string(), TAB_TEXT_DIM, false)], clip: inner, scale: 1.0, face: crate::render::DEFAULT_FACE });
        return r;
    }
    let strip = if d.detail.is_some() { POPUP_DETAIL * scale } else { 0.0 };
    let list = Rect { h: (inner.h - strip).max(0.0), ..inner };
    for (k, (t, bold, glyph)) in d.rows.iter().enumerate().skip(first) {
        let ry = r.y + POPUP_SPACING * scale + (k - first) as f32 * pitch;
        if ry + row_h > list.y + list.h + 0.5 {
            break;
        }
        let on = d.selected == Some(k);
        let color = if on { rgb(0xff, 0xff, 0xff) } else { text };
        if on {
            let sel = Rect { x: r.x + 4.0 * scale, y: ry, w: r.w - 8.0 * scale, h: row_h };
            rounded_box(&mut frame.popups, sel, 4.0 * scale, rgba(LIST_SELECTED, 1.0));
        }
        let icon = Span { text: glyph.to_string(), color, bold: false, italic: false, alt_font: false, icon: true, prose: false, badge: false, toolbelt: false, fake_bold: false, ligatures: 0, mask: 0, font: 0 };
        frame.rows.push(TextRow { x: r.x + 8.0 * scale, y: text_y(ry), spans: vec![icon], clip: list, scale: 1.0, face: crate::render::DEFAULT_FACE });
        let mut spans = Vec::new();
        match bold.clone().filter(|b| b.end <= t.len() && t.is_char_boundary(b.end)) {
            Some(b) => {
                spans.push(plain_span(t[..b.end].to_string(), color, true));
                spans.push(plain_span(t[b.end..].to_string(), color, false));
            }
            None => spans.push(plain_span(t.clone(), color, false)),
        }
        spans.retain(|s| !s.text.is_empty());
        frame.rows.push(TextRow { x: r.x + 8.0 * scale + 2.0 * cell.0, y: text_y(ry), spans, clip: list, scale: 1.0, face: crate::render::DEFAULT_FACE });
    }
    if let Some(detail) = d.detail {
        let sy = r.y + r.h - strip;
        frame.popups.push(Quad { rect: Rect { x: inner.x, y: sy, w: inner.w, h: 1.0 }, color: rgba(DIVIDER, 1.0) });
        frame.rows.push(TextRow { x: r.x + 8.0 * scale, y: sy + ((strip - cell.1) / 2.0).max(0.0), spans: vec![plain_span(detail.to_string(), TAB_TEXT_DIM, false)], clip: inner, scale: 1.0, face: crate::render::DEFAULT_FACE });
    }
    r
}

/// What Codecierge shows (`CG:641-1115`): its label's lines, centered (x, y, text), a button
/// (its rectangle, label, label width) dimmed when disabled, and for the goal and suggestion
/// views the rest of the view. Onboarding has the lines and the button (Get Started or Reveal
/// Setting); the goal view the prompt as the lines, Start as the button and `goal`; the
/// suggestion view End Task as the button and `run`.
pub struct CodeciergeDraw<'a> {
    pub lines: &'a [(f32, f32, String)],
    pub clip: Rect,
    pub button: Rect,
    pub label: &'a str,
    pub label_w: f32,
    pub enabled: bool,
    pub dark: bool,
    pub goal: Option<GoalDraw<'a>>,
    pub run: Option<RunDraw<'a>>,
}

/// A text field of Codecierge: the editor's frame, text, selection and caret (the Notes
/// editor's drawing, no mode control), and the placeholder shown while it is empty (x, y, text).
pub struct FieldDraw<'a> {
    pub edit: NoteDraw<'a>,
    pub placeholder: Option<(f32, f32, &'a str)>,
}

/// The goal view's field and its checkbox: the box, whether it is ticked and enabled, and the
/// label's origin and text.
pub struct GoalDraw<'a> {
    pub field: FieldDraw<'a>,
    pub check: Rect,
    pub checked: bool,
    pub check_enabled: bool,
    pub check_label: (f32, f32, &'a str),
}

/// The suggestion view: the goal line (x, y, text) and the busy spinner's box before it, the
/// transcript (the laid-out markdown, its origin, the room it shows in and the shaded band's
/// left and width), the reply field, and the send button (its box and whether it is enabled).
pub struct RunDraw<'a> {
    pub goal: (f32, f32, &'a str),
    pub spinner: Option<(Rect, u32)>,
    pub lines: &'a [crate::porthole::render::VisualLine],
    pub origin: (f32, f32),
    pub room: Rect,
    pub band: (f32, f32),
    pub busy: bool,
    pub thumb: Option<Rect>,
    pub reply: FieldDraw<'a>,
    pub send: Rect,
    pub send_enabled: bool,
}

/// Codecierge's text color and field background for the theme (the markdown palette's).
pub fn codecierge_colors(dark: bool) -> (Rgb, Rgb) {
    if dark { (TAB_TEXT, FIELD_BG_DARK) } else { (BELT_TEXT_LIGHT, FIELD_BG_LIGHT) }
}

/// The Segoe Fluent Icons Send glyph (the reply's button).
pub const SEND_GLYPH: &str = "\u{E724}";
/// The Segoe Fluent Icons check mark (the ticked box).
const CHECK_GLYPH: &str = "\u{E73E}";

fn codecierge_field(frame: &mut Frame, f: &FieldDraw, cell: (f32, f32), text_scale: f32, scale: f32) {
    note_editor(frame, &f.edit, cell, text_scale, scale);
    if let Some((x, y, t)) = f.placeholder {
        let dim = if f.edit.dark { TAB_TEXT_DIM } else { LIST_DISABLED_LIGHT };
        frame.rows.push(TextRow { x, y, spans: vec![list_span(t.to_string(), dim, false, 0)], clip: f.edit.clip, scale: text_scale, face: crate::render::DEFAULT_FACE });
    }
}

/// Codecierge's views, in the prose font at `text_scale`.
pub fn codecierge(frame: &mut Frame, d: &CodeciergeDraw, cell: (f32, f32), text_scale: f32, scale: f32) {
    let (text, disabled, field_bg) = if d.dark { (TAB_TEXT, LIST_DISABLED_DARK, FIELD_BG_DARK) } else { (BELT_TEXT_LIGHT, LIST_DISABLED_LIGHT, FIELD_BG_LIGHT) };
    let line_h = cell.1 * text_scale;
    for (x, y, line) in d.lines {
        frame.rows.push(TextRow { x: *x, y: *y, spans: vec![list_span(line.clone(), text, false, 1)], clip: d.clip, scale: text_scale, face: crate::render::DEFAULT_FACE });
    }
    if let Some(g) = &d.goal {
        codecierge_field(frame, &g.field, cell, text_scale, scale);
        let edge = rgba(DIVIDER, if d.dark { 1.0 } else { 0.4 });
        frame.quads.push(Quad { rect: g.check, color: rgba(field_bg, 1.0) });
        frame_lines(&mut frame.quads, g.check, if g.check_enabled { edge } else { rgba(disabled, 1.0) });
        if g.checked {
            let span = list_span(CHECK_GLYPH.to_string(), if g.check_enabled { text } else { disabled }, false, 2);
            frame.rows.push(TextRow { x: g.check.x + 1.0, y: g.check.y + (g.check.h - line_h) / 2.0, spans: vec![span], clip: g.check, scale: text_scale, face: crate::render::DEFAULT_FACE });
        }
        let (x, y, t) = g.check_label;
        frame.rows.push(TextRow { x, y, spans: vec![list_span(t.to_string(), if g.check_enabled { text } else { disabled }, false, 1)], clip: d.clip, scale: text_scale, face: crate::render::DEFAULT_FACE });
    }
    if let Some(r) = &d.run {
        let (x, y, t) = r.goal;
        frame.rows.push(TextRow { x, y, spans: vec![list_span(t.to_string(), text, false, 1)], clip: d.clip, scale: text_scale, face: crate::render::DEFAULT_FACE });
        if let Some((b, phase)) = r.spinner {
            // Eight dots round a circle, one lit, turning with `phase`.
            let dot = (b.w / 6.0).max(1.5);
            let radius = (b.w - dot) / 2.0;
            for i in 0..8u32 {
                let a = i as f32 * std::f32::consts::TAU / 8.0;
                let age = (phase + 8 - i) % 8;
                let alpha = 1.0 - age as f32 / 8.0;
                let (cx, cy) = (b.x + b.w / 2.0 + radius * a.sin(), b.y + b.h / 2.0 - radius * a.cos());
                frame.quads.push(Quad { rect: Rect { x: cx - dot / 2.0, y: cy - dot / 2.0, w: dot, h: dot }, color: rgba(text, alpha.max(0.15)) });
            }
        }
        frame.quads.push(Quad { rect: r.room, color: rgba(field_bg, 1.0) });
        frame_lines(&mut frame.quads, r.room, rgba(DIVIDER, if d.dark { 1.0 } else { 0.4 }));
        let start = frame.rows.len();
        doc_lines(frame, r.origin, r.lines, &[], r.room, r.band, field_bg, text);
        // The document was laid out at `text_scale`: its rows are drawn at it.
        for row in &mut frame.rows[start..] {
            row.scale *= text_scale;
        }
        if r.busy {
            // Dimmed to `codecierge::BUSY_ALPHA` of its color over the background.
            for row in &mut frame.rows[start..] {
                for span in &mut row.spans {
                    span.color = blend(field_bg, span.color, crate::codecierge::BUSY_ALPHA);
                }
            }
        }
        if let Some(t) = r.thumb {
            frame.quads.push(Quad { rect: t, color: rgba(TAB_TEXT_DIM, 0.6) });
        }
        codecierge_field(frame, &r.reply, cell, text_scale, scale);
        let s = r.send;
        let color = if r.send_enabled { text } else { disabled };
        let span = list_span(SEND_GLYPH.to_string(), color, false, 2);
        frame.rows.push(TextRow { x: s.x + (s.w - line_h) / 2.0, y: s.y + (s.h - line_h) / 2.0, spans: vec![span], clip: s, scale: text_scale, face: crate::render::DEFAULT_FACE });
    }
    let b = d.button;
    rounded_box(&mut frame.quads, b, 4.0 * scale, rgba(DIVIDER, if d.dark { 1.0 } else { 0.4 }));
    let inner = Rect { x: b.x + 1.0, y: b.y + 1.0, w: (b.w - 2.0).max(0.0), h: (b.h - 2.0).max(0.0) };
    rounded_box(&mut frame.quads, inner, 3.0 * scale, rgba(field_bg, 1.0));
    let color = if d.enabled { text } else { disabled };
    frame.rows.push(TextRow { x: b.x + ((b.w - d.label_w) / 2.0).max(0.0), y: b.y + (b.h - line_h) / 2.0, spans: vec![list_span(d.label.to_string(), color, false, 1)], clip: clip_rect(b, d.clip), scale: text_scale, face: crate::render::DEFAULT_FACE });
}

/// Make Screenshot's sign at the pane's top right while the pane shows the frozen copy: a
/// small camera (a body, a lens, and a viewfinder).
pub fn camera_sign(frame: &mut Frame, pane: Rect, cell: (f32, f32)) {
    let (w, h) = (cell.0 * 1.6, cell.1 * 0.8);
    let x = pane.x + pane.w - 2.0 * cell.0 - w;
    let y = pane.y + cell.1 / 2.0;
    let c = rgba(TAB_TEXT, 0.9);
    frame.overlay.push(Quad { rect: Rect { x, y: y + h * 0.2, w, h: h * 0.8 }, color: c });
    frame.overlay.push(Quad { rect: Rect { x: x + w * 0.3, y, w: w * 0.3, h: h * 0.25 }, color: c });
    let l = h * 0.4;
    frame.overlay.push(Quad { rect: Rect { x: x + (w - l) / 2.0, y: y + h * 0.4, w: l, h: l }, color: rgba(TAB_BAR, 1.0) });
}

/// Buffer Input's sign at the pane's top right: two bars, as a pause symbol.
pub fn buffering_sign(frame: &mut Frame, pane: Rect, cell: (f32, f32)) {
    let h = cell.1;
    let w = (cell.0 / 3.0).max(2.0);
    let x = pane.x + pane.w - 2.0 * cell.0;
    for dx in [0.0, w * 2.0] {
        frame.overlay.push(Quad { rect: Rect { x: x + dx, y: pane.y + cell.1 / 2.0, w, h }, color: rgba(TAB_TEXT, 0.9) });
    }
}

/// RequestAttention=flash: the pane goes white and fades back (`strength` 1 to 0).
pub fn pane_flash(frame: &mut Frame, pane: Rect, strength: f32) {
    frame.overlay.push(Quad { rect: pane, color: rgba(rgb(0xff, 0xff, 0xff), 0.6 * strength.clamp(0.0, 1.0)) });
}

/// RequestAttention=fireworks: bursts of colored sparks around `center`, `t` from 0 to 1.
pub fn fireworks(frame: &mut Frame, center: (f32, f32), t: f32, cell: (f32, f32)) {
    const COLORS: [Rgb; 5] = [rgb(0xff, 0x55, 0x55), rgb(0xff, 0xd7, 0x00), rgb(0x55, 0xff, 0x55), rgb(0x55, 0xaa, 0xff), rgb(0xff, 0x77, 0xff)];
    let bursts = [(0.0f32, (0.0f32, 0.0f32)), (0.15, (-4.0, -2.0)), (0.3, (5.0, -3.0))];
    for (k, (start, (dx, dy))) in bursts.iter().enumerate() {
        let local = ((t - start) / (1.0 - start)).clamp(0.0, 1.0);
        if t < *start || local >= 1.0 {
            continue;
        }
        let (cx, cy) = (center.0 + dx * cell.0, center.1 + dy * cell.1);
        let radius = local * 6.0 * cell.0;
        let size = (cell.0 / 2.0).max(2.0);
        let alpha = 1.0 - local;
        for i in 0..12 {
            let a = i as f32 / 12.0 * std::f32::consts::TAU;
            let (x, y) = (cx + a.cos() * radius, cy + a.sin() * radius + local * local * cell.1 * 2.0);
            let color = COLORS[(i + k) % COLORS.len()];
            frame.overlay.push(Quad { rect: Rect { x: x - size / 2.0, y: y - size / 2.0, w: size, h: size }, color: rgba(color, alpha) });
        }
    }
}

/// The toast's box.
pub fn toast_rect(text: &str, area: Rect, cell: (f32, f32)) -> Rect {
    let w = (text.chars().count() as f32 + 4.0) * cell.0;
    Rect { x: area.x + ((area.w - w) / 2.0).max(0.0), y: area.y + area.h - 3.0 * cell.1, w: w.min(area.w), h: cell.1 + 12.0 }
}

/// A short message in a dark box near the bottom center of `area`.
pub fn toast(frame: &mut Frame, text: &str, area: Rect, cell: (f32, f32)) -> Rect {
    let r = toast_rect(text, area, cell);
    // Text under the box is cut where the box starts, as under the find bar.
    cut_rows_from(frame, r, cell.1);
    frame.quads.push(Quad { rect: r, color: rgba(STAMP_BG, 1.0) });
    let span = Span { text: text.to_string(), color: TAB_TEXT, bold: false, italic: false, alt_font: false, icon: false, prose: false, badge: false, toolbelt: false, fake_bold: false, ligatures: 0, mask: 0, font: 0 };
    frame.rows.push(TextRow { x: r.x + 2.0 * cell.0, y: r.y + 6.0, spans: vec![span], clip: r, scale: 1.0, face: crate::render::DEFAULT_FACE });
    r
}

/// The error bar's box.
pub fn banner_rect(area: Rect, cell: (f32, f32)) -> Rect {
    Rect { x: area.x, y: area.y + area.h - cell.1 - 8.0, w: area.w, h: cell.1 + 8.0 }
}

/// A one-line error bar along the bottom of `area`. Its box goes in `quads`, so its
/// text (in `rows`) draws on top.
pub fn banner(frame: &mut Frame, text: &str, area: Rect, cell: (f32, f32)) -> Rect {
    let r = banner_rect(area, cell);
    // The pane's text and strokes under the bar go, as under every surface.
    cut_out(frame, r, cell.1);
    frame.quads.push(Quad { rect: r, color: rgba(BANNER, 1.0) });
    let span = Span { text: text.to_string(), color: rgb(0xff, 0xff, 0xff), bold: false, italic: false, alt_font: false, icon: false, prose: false, badge: false, toolbelt: false, fake_bold: false, ligatures: 0, mask: 0, font: 0 };
    frame.rows.push(TextRow { x: r.x + cell.0, y: r.y + 4.0, spans: vec![span], clip: r, scale: 1.0, face: crate::render::DEFAULT_FACE });
    r
}

/// The FPS meter (`iTermMetalPerFrameState.m:2531-2552`): `text` right-aligned on row 0 of the
/// pane's text area, one cell per character, magenta on near black. Its box goes in `popups`
/// with the pane's text under it cut out, so the meter replaces that row's cells there.
pub fn fps_meter(frame: &mut Frame, text: &str, pane_inner: Rect, cell: (f32, f32)) {
    let chars = text.chars().count();
    let w = (chars as f32 * cell.0).min(pane_inner.w);
    let r = Rect { x: pane_inner.x + pane_inner.w - w, y: pane_inner.y, w, h: cell.1 };
    if r.w <= 0.0 {
        return;
    }
    let to_rgb = |c: [f32; 4]| rgb((c[0] * 255.0).round() as u8, (c[1] * 255.0).round() as u8, (c[2] * 255.0).round() as u8);
    cut_out(frame, r, cell.1);
    frame.popups.push(Quad { rect: r, color: rgba(to_rgb(crate::perf_stats::FPS_BG), 1.0) });
    let span = Span { text: text.to_string(), color: to_rgb(crate::perf_stats::FPS_FG), bold: false, italic: false, alt_font: false, icon: false, prose: false, badge: false, toolbelt: false, fake_bold: false, ligatures: 0, mask: 0, font: 0 };
    frame.rows.push(TextRow { x: r.x, y: r.y, spans: vec![span], clip: r, scale: 1.0, face: crate::render::DEFAULT_FACE });
}

#[cfg(test)]
mod broadcast_tests {
    use super::*;

    #[test]
    fn fps_meter_sits_right_aligned_on_row_zero() {
        let mut f = Frame::default();
        let pane = Rect { x: 10.0, y: 20.0, w: 400.0, h: 300.0 };
        let text = " [Frame 7: 63 fps] ";
        fps_meter(&mut f, text, pane, (8.0, 16.0));
        let w = text.chars().count() as f32 * 8.0;
        assert_eq!(f.popups.len(), 1);
        assert_eq!(f.popups[0].rect, Rect { x: 410.0 - w, y: 20.0, w, h: 16.0 });
        assert_eq!((f.rows.len(), f.rows[0].spans[0].color), (1, rgb(255, 0, 255)));
    }

    #[test]
    fn stripe_tile_matches_iterm2() {
        let (n, px) = stripe_tile(1.0);
        assert_eq!((n, px.len()), (80, 80 * 80 * 4));
        let at = |x: usize, y: usize| px[(y * 80 + x) * 4..(y * 80 + x) * 4 + 4].to_vec();
        assert_eq!(at(0, 0), vec![255, 0, 0, 64]);
        assert_eq!(at(39, 0), vec![255, 0, 0, 64]);
        assert_eq!(at(40, 0)[3], 0);
        assert_eq!(at(39, 1)[3], 0, "the band edge moves one pixel left per row: a slash");
        assert_eq!(at(79, 1)[3], 64);
        assert_eq!(stripe_tile(2.0).0, 160, "scaled for the display");
    }

    #[test]
    fn indicators_keep_iterm2_order() {
        assert_eq!(indicator_glyphs(true, true, true, false, false), vec![BROADCAST_GLYPH, COPROCESS_GLYPH]);
        assert_eq!(indicator_glyphs(false, true, false, false, false), vec![RECEIVE_ONLY_GLYPH]);
        assert_eq!(indicator_glyphs(false, false, true, false, false), vec![COPROCESS_GLYPH]);
        assert!(indicator_glyphs(false, false, false, false, false).is_empty());
    }

    #[test]
    fn indicator_glyphs_follow_iterm2() {
        assert_eq!(indicator_glyphs(true, true, true, true, true), vec![BROADCAST_GLYPH, COPROCESS_GLYPH, AI_LINKED_GLYPH, AI_STREAMING_GLYPH]);
        assert_eq!(indicator_glyphs(false, false, false, true, false), vec![AI_LINKED_GLYPH]);
        assert_eq!(AI_STREAMING_GLYPH, RECEIVE_ONLY_GLYPH, "iTerm2 uses one symbol for both");
        assert_eq!(AI_LINKED_GLYPH, '\u{E8BD}');
        // Each glyph knows what a press on it means; the streaming glyph is told apart from
        // receive-only by its kind, not its character.
        let kinds: Vec<IndicatorKind> = indicators(false, true, true, true, true).into_iter().map(|(_, k)| k).collect();
        assert_eq!(kinds, vec![IndicatorKind::Other, IndicatorKind::Other, IndicatorKind::AiLinked, IndicatorKind::AiStreaming]);
        let hits = top_right_indicators(&mut Frame::default(), Rect { x: 0.0, y: 0.0, w: 400.0, h: 100.0 }, &[BROADCAST_GLYPH, AI_LINKED_GLYPH], rgb(1, 2, 3), (8.0, 16.0));
        assert_eq!(hits.len(), 2);
        assert!(hits[0].x > hits[1].x, "right to left from the corner");
        assert!(hits[0].x + hits[0].w <= 400.0);
    }
}

#[cfg(test)]
mod tests {
    /// (quads, rows, digest) of the porthole frame in `doc_lines_draw_as_the_porthole_did`, taken from `porthole` before `doc_lines` was extracted from it (retaken when `TextRow` gained `face`: the digest reads the rows' Debug text, and the 10 quads and 15 rows are unchanged).
    const GOLDEN: (usize, usize, u64) = (10, 15, 10881690239227710009);

    /// The palette the tab bar had before themes: one flat dark bar, a lighter selected tab, the
    /// two greys for text and no lines. The layout tests below draw with it.
    fn legacy() -> crate::tab_theme::TabPalette {
        use crate::tab_theme::{Fill, TabPalette, TabStyle};
        let clear = Fill { color: TAB_BAR, alpha: 0.0 };
        TabPalette {
            style: TabStyle::Dark,
            resolved: TabStyle::Dark,
            bar: TAB_BAR,
            selected: Fill { color: TAB_ACTIVE, alpha: 1.0 },
            unselected: clear,
            margin: clear,
            text_selected: Fill { color: TAB_TEXT, alpha: 1.0 },
            text_unselected: Fill { color: TAB_TEXT_DIM, alpha: 1.0 },
            between: clear,
            edge: clear,
            top: clear,
            top_selected: clear,
            line_inset: (1.0, 2.0),
            dark_edge_gradient: false,
            attached: true,
            font_pt: 11.0,
            dark_controls: true,
            minimal: None,
            colored_outline: 0.5,
            colored_text_prominence: 0.5,
        }
    }

    /// A colored tab is filled with its color over its whole cell (no bottom strip), the selected
    /// one is outlined by two strokes, and every tab's text follows the brightness of its own fill.
    #[test]
    fn colored_tab_draws_fill_and_outline() {
        use crate::tab_layout::{self, TabInput};
        use crate::tab_theme::{Fill, Inputs, MinimalKnobs, TabStyle, colored_fill, colored_text, over, palette};
        let red = rgb(255, 0, 0);
        let pal = palette(
            TabStyle::Light,
            &Inputs { system_dark: false, system_high_contrast: false, focused: true, light_darkness: 0.07, dark_darkness: 0.5, high_visibility: true, session_bg: rgb(0, 0, 0), dimming: 0.0, minimal: MinimalKnobs::default(), colored_outline: 0.5, colored_text_prominence: 0.5, tabs: 2 },
        );
        let looks = vec![TabLook { title: "one".into(), number: 1, color: Some(red), ..TabLook::default() }, TabLook { title: "two".into(), number: 2, ..TabLook::default() }];
        let inputs: Vec<TabInput> = looks
            .iter()
            .map(|l| TabInput { title: l.title.clone(), subtitle: String::new(), icon: false, processing: false, number: l.number, pinned: false, group: None, hidden: false, graphic: false, letter: String::new() })
            .collect();
        let s = tab_layout::Settings::from_globals(&crate::globals::Globals::empty(std::path::Path::new("unused.toml")));
        let six = |t: &str, _pt: f32, _bold: bool| t.chars().count() as f32 * 6.0;
        let bar = tab_layout::build(&inputs, 0, 400.0, 0.0, None, &s, &six);
        let area = Rect { x: 0.0, y: 10.0, w: 400.0, h: 24.0 };
        let mut f = Frame::default();
        tab_bar(&mut f, &looks, 0, &bar, area, 1.0, 0.1, 20.0, true, false, &pal);
        let px = |r: Rect| Rect { x: area.x + r.x, y: area.y + r.y, w: r.w, h: r.h };
        let (first, second) = (px(bar.cells[0].2), px(bar.cells[1].2));
        let fill = colored_fill(red, true, true);
        assert!(f.quads.iter().any(|q| q.rect == first && q.color == rgba(fill, 1.0)), "the fill over the whole cell");
        assert!(!f.quads.iter().any(|q| q.rect.h == 3.0 && q.rect.y == first.y + first.h - 3.0), "no bottom strip");
        let text_color = |t: &str| f.rows.iter().find(|r| r.spans.iter().any(|s| s.text == t)).map(|r| r.spans[0].color);
        assert_eq!(text_color("one"), Some(over(fill, colored_text(fill, true, 0.5))), "the colored tab's text");
        let back = over(pal.bar, pal.unselected);
        let expect = over(back, Fill { color: rgb(0, 0, 0), alpha: 0.5 });
        assert_eq!(text_color("two"), Some(expect), "an uncolored tab's text at the unselected prominence");
        assert!(!f.quads.iter().any(|q| q.rect == second && q.color == rgba(fill, 1.0)));
    }

    /// The selected tab's outline (`YOS:805-839`): two strokes of the width `clamp(strength, 1, 3)`,
    /// the first outset half a point sideways, drawn after the lines and the edge so none covers
    /// it, each ring's corners drawn once; only with a colored tab, only on the selected tab.
    #[test]
    fn selected_outline_follows_iterm2() {
        use crate::tab_layout::{self, TabInput};
        use crate::tab_theme::{Inputs, MinimalKnobs, TabStyle, palette};
        let inputs = |outline: f32| Inputs { system_dark: false, system_high_contrast: false, focused: true, light_darkness: 0.07, dark_darkness: 0.5, high_visibility: true, session_bg: rgb(0, 0, 0), dimming: 0.0, minimal: MinimalKnobs::default(), colored_outline: outline, colored_text_prominence: 0.5, tabs: 2 };
        let mut looks = vec![TabLook { title: "one".into(), number: 1, color: Some(rgb(255, 0, 0)), ..TabLook::default() }, TabLook { title: "two".into(), number: 2, ..TabLook::default() }];
        let items: Vec<TabInput> = looks
            .iter()
            .map(|l| TabInput { title: l.title.clone(), subtitle: String::new(), icon: false, processing: false, number: l.number, pinned: false, group: None, hidden: false, graphic: false, letter: String::new() })
            .collect();
        let s = tab_layout::Settings::from_globals(&crate::globals::Globals::empty(std::path::Path::new("unused.toml")));
        let six = |t: &str, _pt: f32, _bold: bool| t.chars().count() as f32 * 6.0;
        let bar = tab_layout::build(&items, 1, 400.0, 0.0, None, &s, &six);
        let area = Rect { x: 0.0, y: 10.0, w: 400.0, h: 24.0 };
        let px = |r: Rect| Rect { x: area.x + r.x, y: area.y + r.y, w: r.w, h: r.h };
        let c = px(bar.cells[1].2);
        let draw = |looks: &[TabLook], outline: f32| {
            let mut f = Frame::default();
            tab_bar(&mut f, looks, 1, &bar, area, 1.0, 0.1, 20.0, true, false, &palette(TabStyle::Light, &inputs(outline)));
            f
        };
        // A ring's four bands, each pixel once: the top and bottom in full, the sides between.
        let ring = |o: Rect, i: Rect| {
            [
                Rect { x: o.x, y: o.y, w: o.w, h: i.y - o.y },
                Rect { x: o.x, y: i.y + i.h, w: o.w, h: o.y + o.h - i.y - i.h },
                Rect { x: o.x, y: i.y, w: i.x - o.x, h: i.h },
                Rect { x: i.x + i.w, y: i.y, w: o.x + o.w - i.x - i.w, h: i.h },
            ]
        };
        let inset = |r: Rect, dx: f32, dy: f32| Rect { x: r.x + dx, y: r.y + dy, w: r.w - 2.0 * dx, h: r.h - 2.0 * dy };
        let (white, black) = (|a: f32| rgba(rgb(255, 255, 255), a), |a: f32| rgba(rgb(0, 0, 0), a));
        // Width 1: the outer ring one pixel outside the cell sideways and level with its top and
        // bottom rows, the inner one just inside it. A bright selected tab: white outside, black inside.
        let f = draw(&looks, 0.5);
        for (band, color) in ring(inset(c, -1.0, 0.0), inset(c, 0.0, 1.0)).into_iter().map(|b| (b, white(0.5))).chain(ring(inset(c, 0.0, 1.0), inset(c, 1.0, 2.0)).into_iter().map(|b| (b, black(0.5)))) {
            assert!(f.quads.iter().any(|q| q.rect == band && q.color == color), "{band:?} {color:?}");
        }
        assert_eq!(f.quads.iter().filter(|q| q.color == white(0.5)).count(), 4, "the outer ring is four bands");
        assert_eq!(f.quads.iter().filter(|q| q.color == black(0.5)).count(), 4, "so is the inner one, and no tab but the selected has one");
        // Drawn after the edge line, whose row it shares.
        let at = |f: &Frame, rect: Rect| f.quads.iter().position(|q| q.rect == rect);
        let edge = f.quads.iter().position(|q| q.color == rgba(rgb(230, 230, 230), 1.0)).unwrap();
        assert!(edge < at(&f, Rect { x: c.x - 1.0, y: c.y + c.h - 1.0, w: c.w + 2.0, h: 1.0 }).unwrap(), "the outline covers the edge line");
        // Width 3: strokes three pixels thick (`YOS:829`).
        let f = draw(&looks, 3.0);
        for (band, color) in ring(inset(c, 0.0, 1.0), inset(c, 3.0, 4.0)).into_iter().map(|b| (b, white(1.0))).chain(ring(inset(c, 3.0, 4.0), inset(c, 6.0, 7.0)).into_iter().map(|b| (b, black(1.0)))) {
            assert!(f.quads.iter().any(|q| q.rect == band && q.color == color), "{band:?} {color:?}");
        }
        // Only with a colored tab: none without one.
        looks[0].color = None;
        let f = draw(&looks, 0.5);
        assert!(!f.quads.iter().any(|q| q.color == white(0.5) || q.color == black(0.5)), "no outline without a colored tab");
    }

    /// A side bar's lines: the selected tab's are drawn first, so the unselected tabs' (which
    /// share the top line's place) end on top (`YOS:1290-1300`).
    #[test]
    fn selected_lines_are_drawn_before_the_unselected() {
        use crate::tab_layout::{self, TabInput};
        use crate::tab_position::TabPosition;
        use crate::tab_theme::{Inputs, MinimalKnobs, TabStyle, palette};
        let inputs = Inputs { system_dark: true, system_high_contrast: false, focused: true, light_darkness: 0.07, dark_darkness: 0.5, high_visibility: true, session_bg: rgb(0, 0, 0), dimming: 0.0, minimal: MinimalKnobs::default(), colored_outline: 0.5, colored_text_prominence: 0.5, tabs: 3 };
        let looks: Vec<TabLook> = (1..=3).map(|n| TabLook { title: format!("t{n}"), number: n, ..TabLook::default() }).collect();
        let items: Vec<TabInput> = looks
            .iter()
            .map(|l| TabInput { title: l.title.clone(), subtitle: String::new(), icon: false, processing: false, number: l.number, pinned: false, group: None, hidden: false, graphic: false, letter: String::new() })
            .collect();
        let s = tab_layout::Settings::from_globals(&crate::globals::Globals::empty(std::path::Path::new("unused.toml")));
        let six = |t: &str, _pt: f32, _bold: bool| t.chars().count() as f32 * 6.0;
        let bar = tab_layout::Bar { position: TabPosition::Left, ..tab_layout::build_vertical(&items, 150.0, 200.0, 0.0, None, &s, &six) };
        let area = Rect { x: 0.0, y: 0.0, w: 150.0, h: 200.0 };
        let mut f = Frame::default();
        tab_bar_vertical(&mut f, &looks, 1, &bar, area, 1.0, 0.1, 20.0, true, false, &palette(TabStyle::Dark, &inputs));
        let top = |color: [f32; 4]| f.quads.iter().position(|q| q.rect.y == 0.0 && q.rect.h == 1.0 && q.color == color);
        let (selected, unselected) = (top(rgba(rgb(255, 255, 255), 0.20)), top(rgba(rgb(0, 0, 0), 1.0)));
        assert!(selected.is_some() && unselected.is_some(), "both top lines are drawn");
        assert!(selected < unselected, "the unselected line covers the selected one");
    }

    /// A top bar draws the Dark style's black top line on the unselected tabs only, and past the
    /// "+" button; the Light style draws none (`YOS:1313-1333`).
    #[test]
    fn top_lines_follow_iterm2() {
        use crate::tab_layout::{self, TabInput};
        use crate::tab_theme::{Inputs, MinimalKnobs, TabStyle, palette};
        let inputs = Inputs { system_dark: true, system_high_contrast: false, focused: true, light_darkness: 0.07, dark_darkness: 0.5, high_visibility: true, session_bg: rgb(0, 0, 0), dimming: 0.0, minimal: MinimalKnobs::default(), colored_outline: 0.5, colored_text_prominence: 0.5, tabs: 2 };
        let looks = vec![TabLook { title: "one".into(), number: 1, ..TabLook::default() }, TabLook { title: "two".into(), number: 2, ..TabLook::default() }];
        let tabs: Vec<TabInput> = looks
            .iter()
            .map(|l| TabInput { title: l.title.clone(), subtitle: String::new(), icon: false, processing: false, number: l.number, pinned: false, group: None, hidden: false, graphic: false, letter: String::new() })
            .collect();
        let s = tab_layout::Settings::from_globals(&crate::globals::Globals::empty(std::path::Path::new("unused.toml")));
        let six = |t: &str, _pt: f32, _bold: bool| t.chars().count() as f32 * 6.0;
        let bar = tab_layout::build(&tabs, 1, 400.0, 0.0, None, &s, &six);
        let area = Rect { x: 0.0, y: 10.0, w: 400.0, h: 24.0 };
        let draw = |style: TabStyle| {
            let mut f = Frame::default();
            tab_bar(&mut f, &looks, 1, &bar, area, 1.0, 0.1, 20.0, true, false, &palette(style, &inputs));
            f
        };
        let top = |r: Rect| Rect { x: area.x + r.x, y: area.y, w: r.w, h: 1.0 };
        let black = rgba(rgb(0, 0, 0), 1.0);
        let f = draw(TabStyle::Dark);
        assert!(f.quads.iter().any(|q| q.rect == top(bar.cells[0].2) && q.color == black), "the unselected tab's top line");
        assert!(!f.quads.iter().any(|q| q.rect == top(bar.cells[1].2)), "none on the selected tab of a top bar");
        assert!(bar.add.is_some_and(|a| f.quads.iter().any(|q| q.rect.x == area.x + a.x && q.rect.y == area.y && q.rect.h == 1.0 && q.color == black)), "past the add button");
        let f = draw(TabStyle::Light);
        assert!(!f.quads.iter().any(|q| q.rect.y == area.y && q.rect.h == 1.0 && q.rect.w < 400.0), "Light has no top line");
    }

    /// Compact with a Top bar is not attached to the title bar (`PT:10142`): no top line on any
    /// tab, none past the "+", and Dark's bottom line stays solid (its gradient covers the 0.3
    /// divider, `DRK:113-125`) (`YOS:1628-1646`, `YOS:1326`); Minimal and the others stay attached.
    #[test]
    fn unattached_top_bar_follows_iterm2() {
        use crate::tab_layout::{self, TabInput};
        use crate::tab_theme::{Inputs, MinimalKnobs, TabStyle, palette};
        let inputs = Inputs { system_dark: true, system_high_contrast: false, focused: true, light_darkness: 0.07, dark_darkness: 0.5, high_visibility: true, session_bg: rgb(0, 0, 0), dimming: 0.0, minimal: MinimalKnobs::default(), colored_outline: 0.5, colored_text_prominence: 0.5, tabs: 2 };
        let looks = vec![TabLook { title: "one".into(), number: 1, ..TabLook::default() }, TabLook { title: "two".into(), number: 2, ..TabLook::default() }];
        let tabs: Vec<TabInput> = looks
            .iter()
            .map(|l| TabInput { title: l.title.clone(), subtitle: String::new(), icon: false, processing: false, number: l.number, pinned: false, group: None, hidden: false, graphic: false, letter: String::new() })
            .collect();
        let s = tab_layout::Settings::from_globals(&crate::globals::Globals::empty(std::path::Path::new("unused.toml")));
        let six = |t: &str, _pt: f32, _bold: bool| t.chars().count() as f32 * 6.0;
        let bar = tab_layout::build(&tabs, 1, 400.0, 0.0, None, &s, &six);
        let area = Rect { x: 0.0, y: 10.0, w: 400.0, h: 24.0 };
        let draw = |attached: bool| {
            let mut f = Frame::default();
            let mut pal = palette(TabStyle::Dark, &inputs);
            pal.attached = attached;
            tab_bar(&mut f, &looks, 1, &bar, area, 1.0, 0.1, 20.0, true, false, &pal);
            f
        };
        let top_lines = |f: &Frame| f.quads.iter().filter(|q| q.rect.y == area.y && q.rect.h == 1.0 && q.rect.w < 400.0).count();
        assert!(top_lines(&draw(true)) >= 2, "attached: the unselected tab's line and the run past the add button");
        assert_eq!(top_lines(&draw(false)), 0, "unattached: no top lines at all");
        // The bottom line stays solid: Dark's gradient covers the 0.3 divider (`DRK:113-125`).
        let f = draw(false);
        let bottom = f.quads.iter().find(|q| q.rect.y == area.y + area.h - 1.0 && q.rect.w == 400.0).unwrap();
        assert_eq!(bottom.color, rgba(rgb(0, 0, 0), 1.0));
        // A bar on the bottom draws its lines whatever `attached` says.
        let mut low = bar.clone();
        low.position = crate::tab_position::TabPosition::Bottom;
        let mut f = Frame::default();
        let mut pal = palette(TabStyle::Dark, &inputs);
        pal.attached = false;
        tab_bar(&mut f, &looks, 1, &low, area, 1.0, 0.1, 20.0, true, false, &pal);
        assert!(top_lines(&f) >= 2, "a bottom bar always draws top lines (YOS:1628)");
    }

    fn caption_palette(style: crate::tab_theme::TabStyle, dark: bool) -> crate::tab_theme::TabPalette {
        use crate::tab_theme::{Inputs, MinimalKnobs, palette};
        palette(style, &Inputs { system_dark: dark, system_high_contrast: false, focused: true, light_darkness: 0.07, dark_darkness: 0.5, high_visibility: true, session_bg: rgb(0, 0, 0), dimming: 0.0, minimal: MinimalKnobs::default(), colored_outline: 0.5, colored_text_prominence: 0.5, tabs: 2 })
    }

    fn caption_button_rects() -> [(crate::caption::Button, Rect); 3] {
        use crate::caption::Button::*;
        [(Minimize, Rect { x: 862.0, y: 0.0, w: 46.0, h: 38.0 }), (Maximize, Rect { x: 908.0, y: 0.0, w: 46.0, h: 38.0 }), (Close, Rect { x: 954.0, y: 0.0, w: 46.0, h: 38.0 })]
    }

    /// The three caption buttons: glyphs in the selected text color, the Windows 11 hover and press
    /// colors, the restore glyph in a maximized window.
    #[test]
    fn caption_buttons_draw() {
        use crate::caption::{Button, CLOSE_GLYPH, MAXIMIZE_GLYPH, MINIMIZE_GLYPH, RESTORE_GLYPH};
        use crate::tab_theme::{TabStyle, over};
        let light = caption_palette(TabStyle::Light, false);
        let rects = caption_button_rects();
        let mut f = Frame::default();
        caption_buttons(&mut f, &rects, None, None, false, &light, 1.0, 0.1, 20.0);
        let glyphs: Vec<(String, Rgb)> = f.rows.iter().map(|r| (r.spans[0].text.clone(), r.spans[0].color)).collect();
        let ink = over(light.bar, light.text_selected);
        assert_eq!(glyphs, vec![(MINIMIZE_GLYPH.to_string(), ink), (MAXIMIZE_GLYPH.to_string(), ink), (CLOSE_GLYPH.to_string(), ink)]);
        assert!(f.rows.iter().all(|r| r.spans[0].icon), "drawn in the icon font");
        assert!(f.quads.is_empty(), "no hover, no fill");
        // A hovered Close: Windows 11's red with a white glyph.
        let mut f = Frame::default();
        caption_buttons(&mut f, &rects, Some(Button::Close), None, false, &light, 1.0, 0.1, 20.0);
        assert_eq!(f.quads.len(), 1);
        assert_eq!((f.quads[0].rect, f.quads[0].color), (rects[2].1, rgba(rgb(0xc4, 0x2b, 0x1c), 1.0)));
        assert_eq!(f.rows.iter().find(|r| r.spans[0].text == CLOSE_GLYPH.to_string()).unwrap().spans[0].color, rgb(255, 255, 255));
        // A hovered Minimize: black at 0.0373 on a light bar, white at 0.0605 on a dark one.
        let mut f = Frame::default();
        caption_buttons(&mut f, &rects, Some(Button::Minimize), None, false, &light, 1.0, 0.1, 20.0);
        assert_eq!((f.quads[0].rect, f.quads[0].color), (rects[0].1, rgba(rgb(0, 0, 0), 0.0373)));
        let dark = caption_palette(TabStyle::Dark, true);
        let mut f = Frame::default();
        caption_buttons(&mut f, &rects, Some(Button::Minimize), None, false, &dark, 1.0, 0.1, 20.0);
        assert_eq!((f.quads[0].rect, f.quads[0].color), (rects[0].1, rgba(rgb(255, 255, 255), 0.0605)));
        // Pressed is a little stronger; a pressed Close is the red at 0.9.
        let mut f = Frame::default();
        caption_buttons(&mut f, &rects, None, Some(Button::Close), false, &light, 1.0, 0.1, 20.0);
        assert_eq!(f.quads[0].color, rgba(rgb(0xc4, 0x2b, 0x1c), 0.9));
        // A maximized window offers Restore.
        let mut f = Frame::default();
        caption_buttons(&mut f, &rects, None, None, true, &light, 1.0, 0.1, 20.0);
        assert!(f.rows.iter().any(|r| r.spans[0].text == RESTORE_GLYPH.to_string()));
        assert!(!f.rows.iter().any(|r| r.spans[0].text == MAXIMIZE_GLYPH.to_string()));
    }

    /// R11: the window switch label draws right-aligned in its rect in the unselected text color;
    /// no text, no row.
    #[test]
    fn caption_shortcut_draws() {
        use crate::tab_theme::{TabStyle, over};
        let pal = caption_palette(TabStyle::Light, false);
        let rect = Rect { x: 792.0, y: 0.0, w: 70.0, h: 38.0 };
        let mut f = Frame::default();
        caption_shortcut(&mut f, "Alt+Win+1", rect, 62.0, &pal, 1.0, 0.1, 20.0);
        assert_eq!(f.rows.len(), 1);
        let row = &f.rows[0];
        assert_eq!(row.spans[0].text, "Alt+Win+1");
        assert_eq!(row.spans[0].color, over(pal.bar, pal.text_unselected));
        assert_eq!(row.x + 62.0, rect.x + rect.w - 4.0, "right-aligned with the padding's half on the right");
        assert!(row.y >= rect.y && row.y < rect.y + rect.h);
        let mut none = Frame::default();
        caption_shortcut(&mut none, "", rect, 0.0, &pal, 1.0, 0.1, 20.0);
        assert!(none.rows.is_empty() && none.quads.is_empty());
    }

    /// The caption text (a window with no tab bar showing) is centered in its rect, in the selected
    /// text color, clipped to it.
    #[test]
    fn caption_title_draws_centered() {
        use crate::tab_theme::{TabStyle, over};
        let pal = caption_palette(TabStyle::Dark, true);
        let rect = Rect { x: 0.0, y: 0.0, w: 862.0, h: 38.0 };
        let mut f = Frame::default();
        caption_title(&mut f, "vim", rect, 100.0, &pal, 1.0, 0.1, 20.0);
        assert_eq!(f.rows.len(), 1);
        assert_eq!((f.rows[0].x, f.rows[0].clip), (381.0, rect));
        assert_eq!(f.rows[0].spans[0].color, over(pal.bar, pal.text_selected));
        // Wider than the room: left-aligned and clipped.
        let mut wide = Frame::default();
        caption_title(&mut wide, "vim", rect, 900.0, &pal, 1.0, 0.1, 20.0);
        assert_eq!(wide.rows[0].x, 0.0);
    }

    /// The window name beside the tabs: left-aligned in its rect at alpha 0.55 over the bar, cut with
    /// an ellipsis to the rect's width.
    #[test]
    fn caption_name_draws_dim_and_truncated() {
        use crate::tab_theme::{Fill, TabStyle, over};
        let pal = caption_palette(TabStyle::Dark, true);
        let rect = Rect { x: 6.0, y: 0.0, w: 50.0, h: 38.0 };
        let mut m = |t: &str| t.chars().count() as f32 * 10.0;
        let mut f = Frame::default();
        caption_name(&mut f, "build-all", rect, &mut m, &pal, 1.0, 0.1, 20.0);
        assert_eq!(f.rows.len(), 1);
        assert_eq!((f.rows[0].x, f.rows[0].clip), (6.0, rect));
        assert_eq!(f.rows[0].spans[0].text, "buil\u{2026}");
        let full = over(pal.bar, pal.text_selected);
        assert_eq!(f.rows[0].spans[0].color, over(pal.bar, Fill { color: full, alpha: crate::caption::NAME_ALPHA }));
        let mut none = Frame::default();
        caption_name(&mut none, "", rect, &mut m, &pal, 1.0, 0.1, 20.0);
        assert!(none.rows.is_empty());
    }

    /// The bar draws with its palette: the material, the unselected fill, no fill for a clear
    /// selected tab, the line between tabs but not after the last, the edge on the content's
    /// side of each bar position, and the two text colors.
    #[test]
    fn tab_bar_uses_palette() {
        use crate::tab_layout::{self, TabInput};
        use crate::tab_position::TabPosition;
        use crate::tab_theme::{Inputs, MinimalKnobs, TabStyle, palette};
        let pal = palette(
            TabStyle::Light,
            &Inputs { system_dark: false, system_high_contrast: false, focused: true, light_darkness: 0.07, dark_darkness: 0.5, high_visibility: true, session_bg: rgb(0, 0, 0), dimming: 0.0, minimal: MinimalKnobs::default(), colored_outline: 0.5, colored_text_prominence: 0.5, tabs: 2 },
        );
        let looks = vec![TabLook { title: "one".into(), number: 1, ..TabLook::default() }, TabLook { title: "two".into(), number: 2, ..TabLook::default() }];
        let inputs: Vec<TabInput> = looks
            .iter()
            .map(|l| TabInput { title: l.title.clone(), subtitle: String::new(), icon: false, processing: false, number: l.number, pinned: false, group: None, hidden: false, graphic: false, letter: String::new() })
            .collect();
        let s = tab_layout::Settings::from_globals(&crate::globals::Globals::empty(std::path::Path::new("unused.toml")));
        let six = |t: &str, _pt: f32, _bold: bool| t.chars().count() as f32 * 6.0;
        let bar = tab_layout::build(&inputs, 1, 400.0, 0.0, None, &s, &six);
        let area = Rect { x: 0.0, y: 10.0, w: 400.0, h: 24.0 };
        let mut f = Frame::default();
        tab_bar(&mut f, &looks, 1, &bar, area, 1.0, 0.1, 20.0, true, false, &pal);
        let (first, second) = (bar.cells[0].2, bar.cells[1].2);
        let px = |r: Rect| Rect { x: area.x + r.x, y: area.y + r.y, w: r.w, h: r.h };
        assert_eq!((f.quads[0].rect, f.quads[0].color), (area, rgba(pal.bar, 1.0)), "the bar's material");
        assert!(f.quads.iter().any(|q| q.rect == px(first) && q.color == rgba(rgb(0, 0, 0), 0.07)), "the unselected tab's fill");
        assert!(!f.quads.iter().any(|q| q.rect == px(second)), "a clear selected tab has no fill quad");
        // `YOS:715-757`: the right line inside every tab but the last, just past the last, and
        // from below the top pixel.
        let line = |r: Rect, x: f32| Rect { x, y: r.y + 1.0, w: 1.0, h: r.h - 1.0 };
        let between = rgba(rgb(0, 0, 0), 0.07);
        assert!(f.quads.iter().any(|q| q.rect == line(px(first), px(first).x + first.w - 1.0) && q.color == between), "a line at the first tab's right");
        assert!(f.quads.iter().any(|q| q.rect == line(px(second), px(second).x + second.w) && q.color == between), "the last tab's line past its cell");
        assert!(!f.quads.iter().any(|q| q.rect == line(px(second), px(second).x + second.w - 1.0) && q.color == between), "none inside the last");
        assert!(!f.quads.iter().any(|q| q.rect.x == px(first).x && q.rect.w == 1.0 && q.color == between), "no left line for a first tab at the edge");
        let edge = rgba(rgb(230, 230, 230), 1.0);
        let edge_rect = |p: TabPosition| {
            let mut f = Frame::default();
            tab_bar(&mut f, &looks, 1, &tab_layout::Bar { position: p, ..bar.clone() }, area, 1.0, 0.1, 20.0, true, false, &pal);
            f.quads.iter().find(|q| q.color == edge).map(|q| q.rect)
        };
        assert_eq!(edge_rect(TabPosition::Top), Some(Rect { y: 33.0, h: 1.0, ..area }), "the bottom of a top bar");
        assert_eq!(edge_rect(TabPosition::Bottom), Some(Rect { h: 1.0, ..area }), "the top of a bottom bar");
        assert_eq!(edge_rect(TabPosition::Left), Some(Rect { x: 399.0, w: 1.0, ..area }), "the inner side of a left bar");
        assert_eq!(edge_rect(TabPosition::Right), Some(Rect { w: 1.0, ..area }), "the inner side of a right bar");
        // The margin past the last tab is drawn in the margin fill.
        let end = px(second).x + second.w;
        assert!(f.quads.iter().any(|q| q.rect.x == end && q.color == rgba(rgb(0, 0, 0), 0.07)), "the margin");
        let text = |t: &str| f.rows.iter().find(|r| r.spans.iter().any(|s| s.text == t)).unwrap().spans[0].color;
        assert_eq!((text("one"), text("two")), (rgb(0, 0, 0), rgb(0, 0, 0)), "black text for a focused window");
        let mut off = Frame::default();
        let unfocused = palette(TabStyle::Light, &Inputs { focused: false, system_dark: false, system_high_contrast: false, light_darkness: 0.07, dark_darkness: 0.5, high_visibility: true, session_bg: rgb(0, 0, 0), dimming: 0.0, minimal: MinimalKnobs::default(), colored_outline: 0.5, colored_text_prominence: 0.5, tabs: 2 });
        tab_bar(&mut off, &looks, 1, &bar, area, 1.0, 0.1, 20.0, false, false, &unfocused);
        let text = |t: &str| off.rows.iter().find(|r| r.spans.iter().any(|s| s.text == t)).unwrap().spans[0].color;
        assert_eq!((text("one"), text("two")), (rgb(164, 164, 164), rgb(189, 189, 189)), "unfocused: the two greys");
    }

    #[test]
    fn minimal_notch_follows_iterm2() {
        use super::*;
        use crate::tab_position::TabPosition::{Bottom, Left, Right, Top};
        let r = |x: f32, y: f32, w: f32, h: f32| Rect { x, y, w, h };
        let top = r(0.0, 0.0, 400.0, 38.0);
        let tab = r(100.0, 0.0, 100.0, 38.0);
        // A top bar (`MIN:804-875`): the bottom line to the tab's left edge, up it, down the
        // right edge and on along the bottom.
        assert_eq!(minimal_notch(top, tab, Top, false, false), [r(0.0, 37.0, 100.0, 1.0), r(99.0, 0.0, 1.0, 37.0), r(200.0, 0.0, 1.0, 37.0), r(200.0, 37.0, 200.0, 1.0)]);
        // The first tab with the left inset counted as part of it draws no left part.
        let first = r(20.0, 0.0, 100.0, 38.0);
        assert_eq!(minimal_notch(top, first, Top, true, true), [r(120.0, 0.0, 1.0, 37.0), r(120.0, 37.0, 280.0, 1.0)]);
        assert_eq!(minimal_notch(top, first, Top, false, true).len(), 4, "the inset apart from the tab: both parts");
        assert_eq!(minimal_notch(top, first, Top, true, false).len(), 4, "another tab selected: both parts");
        // A first tab at the very edge has no room for a left part.
        assert_eq!(minimal_notch(top, r(0.0, 0.0, 100.0, 38.0), Top, false, true), [r(100.0, 0.0, 1.0, 37.0), r(100.0, 37.0, 300.0, 1.0)]);
        // A bottom bar mirrors it along the top; only its first tab drops the left part, the
        // setting not consulted (`MIN:702-715`).
        let bottom = minimal_notch(top, tab, Bottom, false, false);
        assert_eq!(bottom, [r(0.0, 0.0, 100.0, 1.0), r(99.0, 1.0, 1.0, 37.0), r(200.0, 1.0, 1.0, 37.0), r(200.0, 0.0, 200.0, 1.0)]);
        assert_eq!(minimal_notch(top, first, Bottom, false, true), [r(120.0, 1.0, 1.0, 37.0), r(120.0, 0.0, 280.0, 1.0)]);
        // A side bar (`MIN:600-662`): on the inner edge around the selected row, the lines across
        // the row's first and last pixel rows.
        let side = r(0.0, 0.0, 120.0, 400.0);
        let row = r(0.0, 100.0, 120.0, 40.0);
        assert_eq!(minimal_notch(side, row, Left, false, false), [r(119.0, 0.0, 1.0, 100.0), r(0.0, 100.0, 120.0, 1.0), r(0.0, 139.0, 120.0, 1.0), r(119.0, 140.0, 1.0, 260.0)]);
        assert_eq!(minimal_notch(side, row, Right, false, false), [r(0.0, 0.0, 1.0, 100.0), r(0.0, 100.0, 120.0, 1.0), r(0.0, 139.0, 120.0, 1.0), r(0.0, 140.0, 1.0, 260.0)]);
        // The first row with the top inset joined draws no part above it.
        assert_eq!(minimal_notch(side, r(0.0, 0.0, 120.0, 40.0), Left, true, true), [r(0.0, 39.0, 120.0, 1.0), r(119.0, 40.0, 1.0, 360.0)]);
        assert_eq!(minimal_notch(side, r(0.0, 0.0, 120.0, 40.0), Left, false, true).len(), 3, "no vertical stub above a row at the edge");
    }

    #[test]
    fn minimal_bar_draws_notch_inset_and_underline() {
        use crate::tab_layout::{self, TabInput};
        use crate::tab_position::TabPosition;
        use crate::tab_theme::{Inputs, MinimalKnobs, TabStyle, palette};
        let inputs = |focused: bool, tabs: usize, knobs: MinimalKnobs| Inputs { system_dark: false, system_high_contrast: false, focused, light_darkness: 0.07, dark_darkness: 0.5, high_visibility: true, session_bg: rgb(0, 0, 0), dimming: 0.0, minimal: knobs, colored_outline: 0.5, colored_text_prominence: 0.5, tabs };
        let pal = palette(TabStyle::Minimal, &inputs(true, 2, MinimalKnobs::default()));
        let mut looks = vec![TabLook { title: "one".into(), number: 1, ..TabLook::default() }, TabLook { title: "two".into(), number: 2, ..TabLook::default() }];
        let items: Vec<TabInput> = looks
            .iter()
            .map(|l| TabInput { title: l.title.clone(), subtitle: String::new(), icon: false, processing: false, number: l.number, pinned: false, group: None, hidden: false, graphic: false, letter: String::new() })
            .collect();
        let s = tab_layout::Settings::from_globals(&crate::globals::Globals::empty(std::path::Path::new("unused.toml")));
        let six = |t: &str, _pt: f32, _bold: bool| t.chars().count() as f32 * 6.0;
        let bar = tab_layout::build(&items, 1, 400.0, 0.0, None, &s, &six);
        let area = Rect { x: 0.0, y: 10.0, w: 400.0, h: 24.0 };
        let px = |r: Rect| Rect { x: area.x + r.x, y: area.y + r.y, w: r.w, h: r.h };
        let (first, second) = (px(bar.cells[0].2), px(bar.cells[1].2));
        let draw = |looks: &[TabLook], pal: &crate::tab_theme::TabPalette, active: usize, bar: &tab_layout::Bar| {
            let mut f = Frame::default();
            tab_bar(&mut f, looks, active, bar, area, 1.0, 0.1, 20.0, true, false, pal);
            f
        };
        let f = draw(&looks, &pal, 1, &bar);
        let outline = rgba(rgb(255, 255, 255), 0.2);
        // The notch: the bottom line before the selected tab and after it, its sides; no line
        // between the tabs and none under the selected one.
        assert!(f.quads.iter().any(|q| q.color == outline && q.rect == Rect { x: area.x, y: area.y + area.h - 1.0, w: second.x - area.x, h: 1.0 }), "the bottom line before the tab");
        assert!(f.quads.iter().any(|q| q.color == outline && q.rect == Rect { x: second.x - 1.0, y: area.y, w: 1.0, h: area.h - 1.0 }), "the tab's left side");
        assert!(!f.quads.iter().any(|q| q.color == outline && q.rect.x >= second.x && q.rect.x < second.x + second.w && q.rect.w > 1.0), "open under the selected tab");
        assert!(f.quads.iter().all(|q| q.color != rgba(rgb(0, 0, 0), 1.0) || q.rect.h != 1.0), "no black edge line");
        // The room before the first tab is drawn as an unselected tab.
        let bar_at = |x: f32| tab_layout::Bar { cells: bar.cells.iter().map(|(i, c, r)| (*i, c.clone(), Rect { x: r.x + x, ..*r })).collect(), ..bar.clone() };
        let shifted = bar_at(30.0);
        let inset = |f: &Frame| f.quads.iter().any(|q| q.rect == Rect { x: area.x, y: area.y, w: 30.0, h: area.h } && q.color == rgba(rgb(13, 13, 13), 1.0));
        assert!(inset(&draw(&looks, &pal, 1, &shifted)), "the left inset in the unselected color");
        // ...or as the selected tab when the first tab is selected and the setting is on.
        let joined = palette(TabStyle::Minimal, &inputs(true, 2, MinimalKnobs { left_inset_is_first_tab: true, ..MinimalKnobs::default() }));
        assert!(!inset(&draw(&looks, &joined, 0, &shifted)), "joined to the selected first tab");
        assert!(inset(&draw(&looks, &joined, 1, &shifted)), "not joined when another tab is selected");
        // A colored tab: its color over the tab at the deselected alpha, and with any colored
        // tab the selected one's two-stroke underline (outer white, inner black on a dark bar).
        looks[0].color = Some(rgb(255, 0, 0));
        let f = draw(&looks, &pal, 1, &bar);
        assert!(f.quads.iter().any(|q| q.rect == first && q.color == rgba(rgb(255, 0, 0), 0.5)), "the deselected colored tab at half alpha");
        let under = |k: f32, w: f32| Rect { x: second.x, y: second.y + second.h - k, w, h: 2.0 };
        assert!(f.quads.iter().any(|q| q.rect == under(2.0, second.w) && q.color == rgba(rgb(255, 255, 255), 0.75)), "the outer underline");
        assert!(f.quads.iter().any(|q| q.rect == under(4.0, second.w) && q.color == rgba(rgb(0, 0, 0), 0.75f32.sqrt())), "the inner underline");
        assert!(!f.quads.iter().any(|q| q.color == rgba(rgb(255, 255, 255), 0.5) || q.color == rgba(rgb(0, 0, 0), 0.5)), "no Regular outline strokes");
        // A bottom bar's notch is on its top edge; one tab in a bottom bar draws none.
        let bottom = draw(&looks, &pal, 1, &tab_layout::Bar { position: TabPosition::Bottom, ..bar.clone() });
        assert!(bottom.quads.iter().any(|q| q.color == outline && q.rect == Rect { x: area.x, y: area.y, w: second.x - area.x, h: 1.0 }), "the top line of a bottom bar");
        let mut one = looks.clone();
        one.truncate(1);
        let single = tab_layout::build(&items[..1], 0, 400.0, 0.0, None, &s, &six);
        let solo = draw(&one, &pal, 0, &tab_layout::Bar { position: TabPosition::Bottom, ..single.clone() });
        assert!(!solo.quads.iter().any(|q| q.color == outline), "one tab in a bottom bar: no outline");
        let solo_top = draw(&one, &pal, 0, &single);
        assert!(solo_top.quads.iter().any(|q| q.color == outline), "one tab in a top bar: the outline");
    }

    /// The setup the overflow notch tests share: three tabs, the Minimal palette, the light-on-dark
    /// outline color.
    fn overflow_setup() -> (Vec<TabLook>, Vec<crate::tab_layout::TabInput>, crate::tab_layout::Settings, crate::tab_theme::TabPalette, crate::tab_theme::Inputs) {
        use crate::tab_theme::{Inputs, MinimalKnobs, TabStyle, palette};
        let inputs = Inputs { system_dark: false, system_high_contrast: false, focused: true, light_darkness: 0.07, dark_darkness: 0.5, high_visibility: true, session_bg: rgb(0, 0, 0), dimming: 0.0, minimal: MinimalKnobs::default(), colored_outline: 0.5, colored_text_prominence: 0.5, tabs: 3 };
        let looks: Vec<TabLook> = (1..=3).map(|n| TabLook { title: format!("t{n}"), number: n, ..TabLook::default() }).collect();
        let items = looks
            .iter()
            .map(|l| crate::tab_layout::TabInput { title: l.title.clone(), subtitle: String::new(), icon: false, processing: false, number: l.number, pinned: false, group: None, hidden: false, graphic: false, letter: String::new() })
            .collect();
        let s = crate::tab_layout::Settings::from_globals(&crate::globals::Globals::empty(std::path::Path::new("unused.toml")));
        (looks, items, s, palette(TabStyle::Minimal, &inputs), inputs)
    }

    #[test]
    fn minimal_notch_with_the_selected_tab_in_overflow() {
        use crate::tab_layout;
        use crate::tab_position::TabPosition;
        use crate::tab_theme::{Inputs, TabStyle, palette};
        let (looks, items, s, pal, inputs) = overflow_setup();
        let six = |t: &str, _pt: f32, _bold: bool| t.chars().count() as f32 * 6.0;
        // A real layout: 150 points hold one tab and the buttons' 24-point margin, and the selected
        // third tab is in the overflow menu.
        let fixed = tab_layout::Settings { prefs: tab_layout::Prefs { scrollable: false, ..s.prefs }, ..s.clone() };
        let mut bar = tab_layout::build(&items, 2, 150.0, 0.0, None, &fixed, &six);
        assert_eq!((bar.cells.len(), bar.hidden.clone()), (1, vec![1, 2]), "the layout the notch is drawn for");
        bar.position = TabPosition::Top;
        let area = Rect { x: 0.0, y: 10.0, w: 150.0, h: 24.0 };
        let mut f = Frame::default();
        tab_bar(&mut f, &looks, 2, &bar, area, 1.0, 0.1, 20.0, true, true, &pal);
        let outline = rgba(rgb(255, 255, 255), 0.2);
        // The part before the tab runs to the right margin, 24 points wide as `rightMarginFor...`
        // says (`MIN:848-870`), and up its side; nothing after.
        assert!(f.quads.iter().any(|q| q.color == outline && q.rect == Rect { x: 0.0, y: 33.0, w: 126.0, h: 1.0 }), "the bar-side line up to the margin");
        assert!(f.quads.iter().any(|q| q.color == outline && q.rect == Rect { x: 125.0, y: 10.0, w: 1.0, h: 23.0 }), "the side at the margin");
        assert_eq!(f.quads.iter().filter(|q| q.color == outline).count(), 2, "no part after");
        // The end inset is the bar color (the selected tab's), not the tinted unselected one.
        let far = bar.cells[0].2.x + bar.cells[0].2.w;
        assert!(f.quads.iter().any(|q| q.rect == Rect { x: far, y: 10.0, w: 150.0 - far, h: 24.0 } && q.color == rgba(rgb(0, 0, 0), 1.0)), "the end inset in the bar color");
        assert!(!f.quads.iter().any(|q| q.rect.x == far && q.color == rgba(rgb(13, 13, 13), 1.0)), "not the tinted unselected color");
        // A bottom bar mirrors it along its top edge.
        let mut f = Frame::default();
        tab_bar(&mut f, &looks, 2, &tab_layout::Bar { position: TabPosition::Bottom, ..bar.clone() }, area, 1.0, 0.1, 20.0, true, true, &pal);
        assert!(f.quads.iter().any(|q| q.color == outline && q.rect == Rect { x: 0.0, y: 10.0, w: 126.0, h: 1.0 }), "the bottom bar's top line up to the margin");
        assert!(f.quads.iter().any(|q| q.color == outline && q.rect == Rect { x: 125.0, y: 11.0, w: 1.0, h: 23.0 }), "the bottom bar's side at the margin");
        assert_eq!(f.quads.iter().filter(|q| q.color == outline).count(), 2, "no part after, in a bottom bar");
        // One tab in all (cells plus the overflow menu are all counted): never a notch when it is
        // the only tab, but a shown selected tab beside a collapsed one is not alone.
        let built = tab_layout::build(&items, 0, 400.0, 0.0, None, &s, &six);
        let mut alone = Frame::default();
        tab_bar(&mut alone, &looks[..1], 0, &tab_layout::Bar { cells: built.cells[..1].to_vec(), hidden: vec![], ..built.clone() }, Rect { w: 400.0, ..area }, 1.0, 0.1, 20.0, true, false, &palette(TabStyle::Minimal, &Inputs { tabs: 1, ..inputs }));
        assert!(alone.quads.iter().any(|q| q.color == outline), "one tab in a top bar: the outline");
        let mut beside = Frame::default();
        tab_bar(&mut beside, &looks, 0, &tab_layout::Bar { cells: built.cells[..1].to_vec(), hidden: vec![], ..built.clone() }, Rect { w: 400.0, ..area }, 1.0, 0.1, 20.0, true, false, &pal);
        assert!(beside.quads.iter().any(|q| q.color == outline), "one shown tab of three: a notch");
    }

    /// A side bar with the selected tab in the overflow menu draws the "above" outline as
    /// `drawOutlineAboveSelectedTabInVerticalTabBar` does with the overflow row as the tab
    /// (`MIN:890-911`, `:932-934`): the inner-edge line down to the overflow row and the line across
    /// it; nothing under it.
    #[test]
    fn minimal_notch_with_the_selected_row_in_overflow() {
        use crate::tab_layout;
        use crate::tab_position::TabPosition;
        let (looks, items, s, pal, _) = overflow_setup();
        let six = |t: &str, _pt: f32, _bold: bool| t.chars().count() as f32 * 6.0;
        let fixed = tab_layout::Settings { prefs: tab_layout::Prefs { scrollable: false, ..s.prefs }, ..s.clone() };
        let mut bar = tab_layout::build_vertical(&items, 120.0, 60.0, 0.0, None, &fixed, &six);
        assert_eq!((bar.cells.len(), bar.hidden.clone()), (1, vec![1, 2]), "one row shows, the rest are in the menu");
        let row = bar.overflow.expect("the overflow row");
        assert_eq!(row.y, 36.0);
        let area = Rect { x: 0.0, y: 10.0, w: 120.0, h: 60.0 };
        let outline = rgba(rgb(255, 255, 255), 0.2);
        for (position, edge_x) in [(TabPosition::Left, 119.0), (TabPosition::Right, 0.0)] {
            bar.position = position;
            let mut f = Frame::default();
            tab_bar_vertical(&mut f, &looks, 2, &bar, area, 1.0, 0.1, 20.0, true, true, &pal);
            let lines: Vec<Rect> = f.quads.iter().filter(|q| q.color == outline).map(|q| q.rect).collect();
            assert_eq!(lines, vec![Rect { x: edge_x, y: 10.0, w: 1.0, h: 36.0 }, Rect { x: 0.0, y: 46.0, w: 120.0, h: 1.0 }], "{position:?}: the edge down to the overflow row and the line across it");
        }
    }

    /// A flashing bar over the sessions: its boxes over theirs at the bar's alpha, its text
    /// blended from the sessions' background, their text behind it cut out only while the
    /// bar is at least half opaque.
    #[test]
    fn lay_over_fades_the_bar() {
        use super::*;
        let pane = Rect { x: 0.0, y: 0.0, w: 100.0, h: 100.0 };
        let bar = Rect { x: 0.0, y: 0.0, w: 100.0, h: 20.0 };
        let span = |color: Rgb| Span { text: "a".into(), color, bold: false, italic: false, alt_font: false, icon: false, prose: false, badge: false, toolbelt: false, fake_bold: false, ligatures: 0, mask: 0, font: 0 };
        let base = || {
            let mut f = Frame::default();
            f.quads.push(Quad { rect: pane, color: rgba(rgb(0, 0, 0), 1.0) });
            f.rows.push(TextRow { x: 0.0, y: 0.0, spans: vec![span(rgb(9, 9, 9))], clip: pane, scale: 1.0, face: crate::render::DEFAULT_FACE });
            // The sessions' overlay: a divider through the bar, an unfocused pane's dim, a
            // beam cursor on the top row, and a link underline below the bar.
            for rect in [
                Rect { x: 50.0, y: 0.0, w: 1.0, h: 100.0 },
                pane,
                Rect { x: 10.0, y: 2.0, w: 2.0, h: 16.0 },
                Rect { x: 0.0, y: 40.0, w: 30.0, h: 1.0 },
            ] {
                f.overlay.push(Quad { rect, color: rgba(rgb(1, 1, 1), 0.5) });
            }
            f
        };
        let layer = || {
            let mut l = Frame::default();
            l.quads.push(Quad { rect: bar, color: rgba(rgb(50, 50, 50), 1.0) });
            l.rows.push(TextRow { x: 4.0, y: 2.0, spans: vec![span(rgb(200, 100, 0))], clip: bar, scale: 1.0, face: crate::render::DEFAULT_FACE });
            l
        };
        let mut f = base();
        lay_over(&mut f, layer(), bar, 1.0, rgb(0, 0, 0), 16.0);
        assert_eq!(f.quads.len(), 1, "the sessions' boxes stay put");
        assert_eq!(f.popups, vec![Quad { rect: bar, color: rgba(rgb(50, 50, 50), 1.0) }]);
        assert_eq!(f.rows.len(), 2);
        assert_eq!(f.rows[0].clip.h, 100.0);
        assert_eq!(f.rows[0].clip.w, 0.0, "the session text behind the bar is cut out");
        assert_eq!(f.rows[1].spans[0].color, rgb(200, 100, 0));
        // Nothing the sessions put in the overlay shows over the bar: what crosses it keeps
        // its part outside, what lies inside it goes.
        let overlay: Vec<Rect> = f.overlay.iter().map(|q| q.rect).collect();
        assert_eq!(
            overlay,
            vec![Rect { x: 50.0, y: 20.0, w: 1.0, h: 80.0 }, Rect { x: 0.0, y: 20.0, w: 100.0, h: 80.0 }, Rect { x: 0.0, y: 40.0, w: 30.0, h: 1.0 }]
        );

        let mut f = base();
        lay_over(&mut f, layer(), bar, 0.25, rgb(0, 0, 0), 16.0);
        assert_eq!(f.popups[0].color, rgba(rgb(50, 50, 50), 0.25));
        assert_eq!(f.rows[0].clip, pane, "a mostly clear bar leaves the text behind it");
        assert_eq!(f.overlay.len(), 4, "and the overlay behind it");
        assert_eq!(f.rows[1].spans[0].color, rgb(50, 25, 0));
    }

    /// The toolbelt: its background by theme, the dividers at alpha 0.25, a close glyph per
    /// wrapper, the titles, the hamburger; the empty label only without tools.
    #[test]
    fn toolbelt_draws_wrappers_and_dividers() {
        use super::*;
        use crate::toolbelt::ToolId;
        use crate::toolbelt_layout::{Split, layout};
        let panel = Rect { x: 700.0, y: 0.0, w: 250.0, h: 401.0 };
        let split = Split { tools: vec![ToolId::Profiles, ToolId::Snippets], heights: vec![200.0, 200.0], collapsed: vec![false, false] };
        let l = layout(panel, &split, 1.0);
        let mut f = Frame::default();
        let d = BeltDraw { layout: &l, titles: &["Profiles", "Snippets"], dark: true, title_widths: &[50.0, 55.0], empty: &[], text_scale: 1.0 };
        toolbelt(&mut f, &d, (8.0, 16.0), 1.0);
        assert_eq!(f.quads[0], Quad { rect: panel, color: rgba(rgb(0x25, 0x25, 0x28), 1.0) });
        assert_eq!(f.quads[1].color, rgba(rgb(122, 122, 122), 0.25));
        assert_eq!(f.quads.len(), 2);
        let texts: Vec<&str> = f.rows.iter().map(|r| r.spans[0].text.as_str()).collect();
        assert_eq!(texts, vec!["\u{E711}", "Profiles", "\u{E711}", "Snippets", "\u{E712}"]);
        assert!(f.rows[1].x >= l.wrappers[0].title.x, "a title is centered in its rectangle");

        let empty = Split::default();
        let l = layout(panel, &empty, 1.0);
        let mut f = Frame::default();
        let lines = vec![("No Tools enabled.".to_string(), 100.0)];
        let d = BeltDraw { layout: &l, titles: &[], dark: false, title_widths: &[], empty: &lines, text_scale: 1.0 };
        toolbelt(&mut f, &d, (8.0, 16.0), 1.0);
        assert_eq!(f.quads[0].color, rgba(rgb(0xff, 0xff, 0xff), 1.0));
        assert_eq!(f.rows.last().unwrap().spans[0].text, "No Tools enabled.");
    }

    /// A tool's list: the field with its caret while it has the keyboard, the selection in
    /// 17, 108, 214 behind white text, a tag line smaller, dimmed buttons, the popup's label.
    #[test]
    fn tool_list_draws_field_rows_and_buttons() {
        use super::*;
        use crate::tool_list::{Focus, ListSpec, ToolButton, layout};
        let area = Rect { x: 0.0, y: 0.0, w: 200.0, h: 200.0 };
        let spec = ListSpec { field: true, right: vec![ToolButton::Open], popup_w: 10_000.0, ..ListSpec::default() };
        let l = layout(area, &spec, &[20.0, 34.0], 0.0, 1.0);
        let rows = vec![
            ListRow { text: "\u{2605} Default".into(), ..ListRow::default() },
            ListRow { text: "Work".into(), second: Some("ssh, prod".into()), ..ListRow::default() },
        ];
        let mut f = Frame::default();
        let d = ListDraw { layout: &l, rows: &rows, first: 0, selected: &[1], dark: true, field: Some(("ab", 16.0)), focus: Some(Focus::Field), disabled: &[ToolButton::Open], popup: Some(("New Tab", false)), second_scale: 0.8, drop_y: None, lit: &[] };
        tool_list(&mut f, &d, (8.0, 16.0), 1.0, 1.0);
        let texts: Vec<&str> = f.rows.iter().map(|r| r.spans[0].text.as_str()).collect();
        assert_eq!(texts, vec!["ab", "\u{2605} Default", "Work", "ssh, prod", "\u{E768}", "New Tab", "\u{E70D}"]);
        // The caret sits after the field's text.
        assert!(f.quads.iter().any(|q| q.rect.x == 4.0 + 16.0 && q.rect.w == 1.0));
        // The selected row's background and white text; the tag line is smaller.
        assert!(f.quads.iter().any(|q| q.color == rgba(LIST_SELECTED, 1.0) && q.rect.y == l.rows[1].y));
        assert_eq!(f.rows[2].spans[0].color, rgb(0xff, 0xff, 0xff));
        assert_eq!(f.rows[3].scale, 0.8);
        assert!(f.rows[3].spans[0].prose && f.rows[2].spans[0].toolbelt);
        // Open is dimmed, and so is the popup without a selection.
        assert_eq!(f.rows[4].spans[0].color, LIST_DISABLED_DARK);
        assert_eq!(f.rows[5].spans[0].color, LIST_DISABLED_DARK);

        // Only the rows from `first` on are given; each lands on its own row rectangle.
        let mut f = Frame::default();
        let d = ListDraw { layout: &l, rows: &rows[1..], first: 1, selected: &[1], dark: true, field: None, focus: None, disabled: &[], popup: None, second_scale: 0.8, drop_y: None, lit: &[] };
        tool_list(&mut f, &d, (8.0, 16.0), 1.0, 1.0);
        let work = f.rows.iter().find(|r| r.spans[0].text == "Work").unwrap();
        assert_eq!(work.y, l.rows[1].y + LIST_INSET);
        assert_eq!(work.spans[0].color, rgb(0xff, 0xff, 0xff));
    }

    /// Snippets' rows: a folder's disclosure before its icon, tag chips on the second line in
    /// orange at 0.8 size, and a drag's drop line.
    #[test]
    fn tool_list_draws_folders_chips_and_the_drop_line() {
        use super::*;
        use crate::tool_list::{ListSpec, layout};
        let area = Rect { x: 0.0, y: 0.0, w: 200.0, h: 200.0 };
        let l = layout(area, &ListSpec::default(), &[20.0, 36.0], 0.0, 1.0);
        let rows = vec![
            ListRow { text: "f".into(), icon: Some(('\u{E8B7}', CHIP_BG)), disclosure: Some('\u{E76C}'), ..ListRow::default() },
            ListRow { text: "s".into(), icon: Some(('\u{E8BD}', rgb(94, 159, 208))), depth: 1, chips: vec![(" t ".into(), 12.0), (" u ".into(), 10.0)], ..ListRow::default() },
        ];
        let mut f = Frame::default();
        let d = ListDraw { layout: &l, rows: &rows, first: 0, selected: &[], dark: true, field: None, focus: None, disabled: &[], popup: None, second_scale: 0.8, drop_y: Some(20.0), lit: &[] };
        tool_list(&mut f, &d, (8.0, 16.0), 1.0, 1.0);
        let texts: Vec<&str> = f.rows.iter().map(|r| r.spans[0].text.as_str()).collect();
        assert_eq!(texts, vec!["\u{E76C}", "\u{E8B7}", "f", "\u{E8BD}", "s", " t ", " u "]);
        // The folder's text comes after both glyphs; the child is indented.
        assert!(f.rows[2].x > f.rows[1].x && f.rows[1].x > f.rows[0].x);
        assert_eq!(f.rows[3].x, LIST_INSET + LIST_INDENT);
        // Chips: orange boxes under the text, white text at 0.8, a gap between them.
        let chips: Vec<Rect> = f.quads.iter().filter(|q| q.color == rgba(CHIP_BG, 1.0)).map(|q| q.rect).collect();
        assert_eq!(chips.len(), 2);
        assert_eq!(chips[0].y, l.rows[1].y + LIST_INSET + 16.0);
        assert_eq!(chips[1].x, chips[0].x + 12.0 + CHIP_GAP);
        assert_eq!((f.rows[5].scale, f.rows[5].spans[0].color), (CHIP_SCALE, rgb(0xff, 0xff, 0xff)));
        // The drop line at the second row's top.
        assert!(f.quads.iter().any(|q| q.color == rgba(LIST_SELECTED, 1.0) && q.rect.y == 19.0 && q.rect.w == 200.0));
    }

    /// Session Status' and Captured Output's rows: lead glyphs in their slots, the shortcut
    /// at the right, the status line in its color then the detail indented in the secondary
    /// color, a snoozed row dimmed with a rule above it, and wrapped lines in the text color.
    #[test]
    fn tool_list_draws_status_and_wrapped_rows() {
        use super::*;
        use crate::tool_list::{ListSpec, ToolButton, layout};
        let area = Rect { x: 0.0, y: 0.0, w: 200.0, h: 200.0 };
        let spec = ListSpec { left: vec![ToolButton::Bell], ..ListSpec::default() };
        let l = layout(area, &spec, &[52.0, 36.0], 0.0, 1.0);
        let orange = rgb(0xff, 0xa5, 0x00);
        let lead = vec![(ToolButton::Snooze.glyph(), TAB_TEXT_DIM), (STATUS_DOT, orange)];
        assert_eq!(lead_width(&lead, 2.0), (12.0 + 3.0 + 10.0 + 4.0) * 2.0);
        let rows = vec![
            ListRow { text: "zsh".into(), right: Some(("Win+2".into(), 30.0)), lines: vec!["Working".into(), "detail".into()], lines_indent: 14.0, dim: true, rule_above: true, status_color: Some(orange), lead: lead.clone(), ..ListRow::default() },
            ListRow { text: "-abc".into(), lines: vec!["def".into()], ..ListRow::default() },
        ];
        let mut f = Frame::default();
        let d = ListDraw { layout: &l, rows: &rows, first: 0, selected: &[], dark: true, field: None, focus: None, disabled: &[], popup: None, second_scale: 0.8, drop_y: None, lit: &[ToolButton::Bell] };
        tool_list(&mut f, &d, (8.0, 16.0), 1.0, 1.0);
        let texts: Vec<&str> = f.rows.iter().map(|r| r.spans[0].text.as_str()).collect();
        assert_eq!(texts, vec!["\u{E708}", "\u{EA3B}", "Win+2", "zsh", "Working", "detail", "-abc", "def", "\u{E7ED}"]);
        let name = &f.rows[3];
        assert_eq!(name.x, LIST_INSET + 29.0);
        // Dimmed: the text at half strength over the panel; the lead keeps its color.
        assert_eq!(name.spans[0].color, blend(TAB_TEXT, BELT_BG_DARK, 0.5));
        assert_eq!(f.rows[1].spans[0].color, orange);
        // The shortcut ends at the row's right inset.
        assert_eq!(f.rows[2].x, 200.0 - LIST_INSET - 30.0);
        // The status line under the name at its x, the detail 14 further in, one line apart.
        assert_eq!((f.rows[4].x, f.rows[4].y), (name.x, name.y + 16.0));
        assert_eq!(f.rows[4].spans[0].color, blend(orange, BELT_BG_DARK, 0.5));
        assert_eq!((f.rows[5].x, f.rows[5].y), (name.x + 14.0, name.y + 32.0));
        // The rule along the snoozed row's top.
        assert!(f.quads.iter().any(|q| q.rect.y == l.rows[0].y && q.rect.h == 1.0 && q.rect.w == 200.0));
        // A wrapped label's next line in the text color, right under it.
        assert_eq!((f.rows[7].x, f.rows[7].y, f.rows[7].spans[0].color), (f.rows[6].x, f.rows[6].y + 16.0, TAB_TEXT));
        // A lit button is drawn in the selection color.
        assert_eq!(f.rows[8].spans[0].color, LIST_SELECTED);
        // An empty status line takes no room: the detail moves up.
        let rows = vec![ListRow { text: "a".into(), lines: vec![String::new(), "d".into()], lines_indent: 14.0, status_color: Some(orange), ..ListRow::default() }];
        let mut f = Frame::default();
        let d = ListDraw { layout: &l, rows: &rows, first: 0, selected: &[], dark: true, field: None, focus: None, disabled: &[], popup: None, second_scale: 0.8, drop_y: None, lit: &[] };
        tool_list(&mut f, &d, (8.0, 16.0), 1.0, 1.0);
        let d_row = f.rows.iter().find(|r| r.spans[0].text == "d").unwrap();
        assert_eq!(d_row.y, LIST_INSET + 16.0);
    }

    /// A button face: the corner rows are inset, the middle is full width, and the box
    /// covers exactly its own rows.
    #[test]
    fn rounded_box_trims_corners() {
        use super::*;
        let mut q = Vec::new();
        let r = Rect { x: 10.0, y: 20.0, w: 40.0, h: 16.0 };
        rounded_box(&mut q, r, 4.0, [1.0; 4]);
        let top = q.iter().filter(|q| q.rect.y == 20.0).map(|q| q.rect).collect::<Vec<_>>();
        assert_eq!(top.len(), 1);
        assert!(top[0].x > 10.0 && top[0].x + top[0].w < 50.0, "{top:?}");
        let middle = q.last().unwrap().rect;
        assert_eq!(middle, Rect { x: 10.0, y: 24.0, w: 40.0, h: 8.0 });
        let rows: f32 = q.iter().map(|q| q.rect.h).sum();
        assert_eq!(rows, 16.0);
        let mut square = Vec::new();
        rounded_box(&mut square, r, 0.0, [1.0; 4]);
        assert_eq!(square.len(), 1);
        assert_eq!(square[0].rect, r);
    }

    #[test]
    fn popups_over_text_keep_their_own_text() {
        // Overlay quads draw after all text, so a popup's box must not be one: the terminal
        // text under the box is cut out instead, and the popup's own text shows.
        let mut f = Frame::default();
        let span = Span { text: "terminal".into(), color: LABEL_TEXT, bold: false, italic: false, alt_font: false, icon: false, prose: false, badge: false, toolbelt: false, fake_bold: false, ligatures: 0, mask: 0, font: 0 };
        let under = Rect { x: 0.0, y: 0.0, w: 400.0, h: 16.0 };
        f.rows.push(TextRow { x: 0.0, y: 0.0, spans: vec![span], clip: under, scale: 1.0, face: crate::render::DEFAULT_FACE });
        let bounds = Rect { x: 0.0, y: 0.0, w: 400.0, h: 300.0 };
        completion_list(&mut f, &["one".into(), "two".into()], 0, None, (100.0, 0.0), bounds, (8.0, 16.0));
        assert!(f.overlay.is_empty());
        let boxes: Vec<Rect> = f.popups.iter().map(|q| q.rect).collect();
        let b = boxes[0];
        // The terminal row is split around the box: left part and right part.
        assert_eq!(f.rows[0].clip, Rect { x: 0.0, y: 0.0, w: b.x, h: 16.0 });
        assert_eq!(f.rows[1].clip, Rect { x: b.x + b.w, y: 0.0, w: 400.0 - b.x - b.w, h: 16.0 });
        assert!(f.rows.iter().any(|r| r.spans[0].text == "one"));
    }

    #[test]
    fn tab_tooltip_stays_in_the_window() {
        let window = Rect { x: 0.0, y: 0.0, w: 400.0, h: 300.0 };
        let cell = (8.0, 16.0);
        // Terminal text under the tip is cut out; the tip is a popup box, not an overlay.
        let mut f = Frame::default();
        let span = plain_span("terminal".into(), LABEL_TEXT, false);
        f.rows.push(TextRow { x: 0.0, y: 40.0, spans: vec![span], clip: Rect { x: 0.0, y: 40.0, w: 400.0, h: 16.0 }, scale: 1.0, face: crate::render::DEFAULT_FACE });
        tab_tooltip(&mut f, "Name: zsh\nProfile: Default\nCommand: None", (100.0, 40.0), window, cell);
        assert!(f.overlay.is_empty() && f.quads.is_empty());
        let b = f.popups[0].rect;
        assert_eq!((b.x, b.y), (100.0, 40.0));
        assert_eq!(f.rows[0].clip.w, b.x);
        // Three lines, three text rows in the box, one per line.
        let rows: Vec<&TextRow> = f.rows.iter().filter(|r| r.clip == b).collect();
        assert_eq!(rows.iter().map(|r| r.spans[0].text.as_str()).collect::<Vec<_>>(), ["Name: zsh", "Profile: Default", "Command: None"]);
        assert!(rows.windows(2).all(|w| w[1].y == w[0].y + cell.1));
        // The widest line and a cell each side: "Profile: Default" is 16 characters.
        assert_eq!(b.w, 18.0 * cell.0);
        // A tip that would pass the right edge moves left to fit; past the bottom, up.
        let mut f = Frame::default();
        tab_tooltip(&mut f, "Name: zsh\nProfile: Default\nCommand: None", (390.0, 290.0), window, cell);
        let b = f.popups[0].rect;
        assert_eq!(b.x + b.w, window.w);
        assert!(b.y + b.h <= window.h && b.y >= 0.0);
        // A window too narrow for it keeps its left edge at the window's.
        let mut f = Frame::default();
        tab_tooltip(&mut f, "Name: a very long title indeed", (10.0, 0.0), Rect { w: 100.0, ..window }, cell);
        assert_eq!(f.popups[0].rect.x, 0.0);
    }

    #[test]
    fn popups_cut_only_the_lines_behind_them() {
        // Terminal rows clip to the whole pane: only the lines the box covers lose text.
        let mut f = Frame::default();
        let pane = Rect { x: 0.0, y: 0.0, w: 400.0, h: 160.0 };
        for i in 0..10 {
            let span = Span { text: format!("line {i}"), color: LABEL_TEXT, bold: false, italic: false, alt_font: false, icon: false, prose: false, badge: false, toolbelt: false, fake_bold: false, ligatures: 0, mask: 0, font: 0 };
            f.rows.push(TextRow { x: 0.0, y: i as f32 * 16.0, spans: vec![span], clip: pane, scale: 1.0, face: crate::render::DEFAULT_FACE });
        }
        completion_list(&mut f, &["one".into(), "two".into()], 0, None, (100.0, 64.0), pane, (8.0, 16.0));
        let b = f.popups[0].rect;
        assert!(f.quads.is_empty() && f.overlay.is_empty());
        for row in f.rows.iter().filter(|r| r.spans[0].text.starts_with("line")) {
            let behind = row.y + 16.0 > b.y && row.y < b.y + b.h;
            let keeps_all = row.clip == pane;
            assert_eq!(keeps_all, !behind, "row at {}", row.y);
        }
    }

    #[test]
    fn mark_colors_follow_iterm2() {
        use crate::marks::MarkColor::*;
        let pal = crate::palette::Palette::default();
        assert_eq!(mark_color(Success, &pal, false), rgb(137, 193, 255));
        assert_eq!(mark_color(Other, &pal, false), rgb(219, 216, 109));
        assert_eq!(mark_color(Error, &pal, false), rgb(252, 114, 109));
        assert_eq!(mark_color(Success, &pal, true), pal.ansi[4]);
        assert_eq!(mark_color(Other, &pal, true), pal.ansi[3]);
        assert_eq!(mark_color(Error, &pal, true), pal.ansi[1]);
        assert_eq!(line_mark_color(rgb(255, 0, 0), rgb(0, 0, 0)), rgb(128, 0, 0));
        assert_eq!(line_mark_color(rgb(0, 0, 0), rgb(255, 255, 255)), rgb(128, 128, 128));
    }

    #[test]
    fn line_marks_follow_iterm2() {
        let rect = Rect { x: 8.0, y: 100.0, w: 800.0, h: 400.0 };
        let mut f = Frame::default();
        line_mark(&mut f, rect, 3, (10.0, 20.0), rgb(255, 0, 0), true, 80);
        let q = f.overlay.last().unwrap();
        // round(100 + (3 - 0.5) * 20) = 150, 1 px tall, 65 cells wide from the pane's left.
        assert_eq!((q.rect.x, q.rect.y, q.rect.w, q.rect.h), (8.0, 150.0, 650.0, 1.0));
        assert!(q.color[0] > 0.9 && q.color[1] < 0.01, "{:?}", q.color);
        let mut f = Frame::default();
        line_mark(&mut f, rect, 3, (10.0, 20.0), rgb(255, 0, 0), false, 80);
        assert_eq!(f.overlay.last().unwrap().rect.w, 800.0);
        // Row 0 would sit half a cell above the pane: nothing is drawn.
        let mut f = Frame::default();
        line_mark(&mut f, rect, 0, (10.0, 20.0), rgb(255, 0, 0), false, 80);
        assert!(f.overlay.is_empty());
    }

    #[test]
    fn line_marks_skip_the_selected_command() {
        // Not drawn inside the region or on the row after it (`TDH:1387-1394`).
        let region = Some((4usize, 6usize));
        assert!(line_mark_hidden(4, region) && line_mark_hidden(6, region) && line_mark_hidden(8, region));
        assert!(!line_mark_hidden(3, region) && !line_mark_hidden(7, region) && !line_mark_hidden(9, region));
        assert!(!line_mark_hidden(5, None));
    }

    #[test]
    fn mark_bars_sit_in_the_left_padding() {
        let mut f = Frame::default();
        let inner = Rect { x: 10.0, y: 20.0, w: 100.0, h: 100.0 };
        mark_bars(&mut f, &[(2, crate::marks::MarkColor::Error)], inner, 16.0, 4.0, &crate::palette::Palette::default(), false);
        let q = f.overlay.last().unwrap();
        assert!(q.rect.x >= 6.0 && q.rect.x + q.rect.w <= 10.0, "{:?}", q.rect);
        assert_eq!((q.rect.y, q.rect.h), (52.0, 16.0));
        assert!(q.color[0] > 0.9 && q.color[1] < 0.5);
    }

    use super::*;
    use alacritty_terminal::index::{Column, Line, Point, Side};
    use alacritty_terminal::selection::{Selection, SelectionType};

    const CELL: (f32, f32) = (10.0, 20.0);
    const RECT: Rect = Rect { x: 0.0, y: 0.0, w: 800.0, h: 480.0 };

    fn screen(bytes: &[u8]) -> Screen {
        let mut s = Screen::new(80, 24);
        s.feed(bytes);
        s
    }

    fn row_text(f: &Frame, i: usize) -> String {
        f.rows[i].spans.iter().map(|s| s.text.as_str()).collect::<String>()
    }

    #[test]
    fn ligature_runs_split_the_row_as_iterm2() {
        use crate::fonts::LigatureRules;
        let s = screen(b"a->b");
        let on = DrawPrefs { ligatures: LigatureRules { ascii: true, non_ascii: false, prefer_speed: true, level: 2 }, ..DrawPrefs::default() };
        let mut f = Frame::default();
        pane_with(&mut f, &s, RECT, CELL, false, &CursorLook::shown(false), &on, None, crate::render::DEFAULT_FACE);
        let spans: Vec<(&str, u8)> = f.rows[0].spans.iter().map(|s| (s.text.trim_end(), s.ligatures)).collect();
        assert_eq!(spans, vec![("a", 0), ("->", 2), ("b", 0)], "letters take the fast path, punctuation runs ligate");
        // Without preferring speed the whole row is one span at the font's level.
        let full = DrawPrefs { ligatures: LigatureRules { prefer_speed: false, ..on.ligatures }, ..on };
        let mut f = Frame::default();
        pane_with(&mut f, &s, RECT, CELL, false, &CursorLook::shown(false), &full, None, crate::render::DEFAULT_FACE);
        assert_eq!(f.rows[0].spans.iter().map(|s| (s.text.trim_end(), s.ligatures)).collect::<Vec<_>>(), vec![("a->b", 2)]);
        // The stock profile draws no ligatures (R3).
        let mut f = Frame::default();
        pane_with(&mut f, &s, RECT, CELL, false, &CursorLook::shown(false), &DrawPrefs::default(), None, crate::render::DEFAULT_FACE);
        assert!(f.rows[0].spans.iter().all(|s| s.ligatures == 0), "{:?}", f.rows[0].spans);
    }

    #[test]
    fn ligature_runs_break_at_colors_the_cursor_and_the_selection() {
        use crate::fonts::LigatureRules;
        let on = DrawPrefs { ligatures: LigatureRules { ascii: true, non_ascii: false, prefer_speed: false, level: 2 }, ..DrawPrefs::default() };
        let rows = |bytes: &[u8], look: &CursorLook| {
            let s = screen(bytes);
            let mut f = Frame::default();
            pane_with(&mut f, &s, RECT, CELL, true, look, &on, None, crate::render::DEFAULT_FACE);
            f.rows.iter().map(|r| (r.x, r.spans.iter().map(|s| (s.text.trim_end().to_string(), s.ligatures)).collect::<Vec<_>>())).collect::<Vec<_>>()
        };
        // A color change inside a ligature splits its span.
        let r = rows(b"-\x1b[31m>\x1b[0m", &CursorLook::shown(false));
        assert_eq!(r.iter().map(|(_, s)| s.len()).sum::<usize>(), 2, "{r:?}");
        // The cursor cell is its own piece: the arrow before it cannot join it.
        let r = rows(b"->=\x1b[1D", &CursorLook::shown(true));
        assert!(r.len() >= 2 && r.iter().any(|(x, s)| *x == 20.0 && s[0].0 == "="), "{r:?}");
        // 13n final 5: only a block cursor breaks the run (iTerm2); a bar or an underline cursor
        // leaves the ligature whole.
        for shape in [&b"\x1b[6 q"[..], b"\x1b[4 q"] {
            let bytes = [&b"->="[..], shape, b"\x1b[1D"].concat();
            let r = rows(&bytes, &CursorLook::shown(true));
            assert_eq!(r.iter().map(|(_, s)| s.len()).sum::<usize>(), 1, "a non-block cursor does not split the ligature: {r:?}");
        }
    }

    #[test]
    fn non_ascii_ligatures_group_narrow_cells_as_iterm2() {
        use crate::fonts::LigatureRules;
        let rules = |non_ascii| DrawPrefs { ligatures: LigatureRules { ascii: false, non_ascii, prefer_speed: true, level: 2 }, ..DrawPrefs::default() };
        let pieces = |bytes: &[u8], prefs: &DrawPrefs| {
            let s = screen(bytes);
            let mut f = Frame::default();
            pane_with(&mut f, &s, RECT, CELL, false, &CursorLook::shown(false), prefs, None, crate::render::DEFAULT_FACE);
            f.rows.iter().map(|r| (r.x, r.spans.iter().map(|s| (s.text.trim_end().to_string(), s.ligatures)).collect::<Vec<_>>())).collect::<Vec<_>>()
        };
        let text = "\u{e0b0}\u{e0b1}\u{e0b2}".as_bytes();
        // Off (the stock profile): one piece per cell, level 0.
        assert_eq!(pieces(text, &rules(false)).len(), 3);
        // On: the three one-column cells share a piece at level 2 (a Nerd Font sequence can join).
        let on = pieces(text, &rules(true));
        assert_eq!(on, vec![(0.0, vec![("\u{e0b0}\u{e0b1}\u{e0b2}".to_string(), 2)])]);
        // A wide character keeps its own piece, and so does a bidi control.
        let wide = pieces("\u{e0b0}\u{4e2d}\u{e0b1}".as_bytes(), &rules(true));
        assert_eq!(wide.len(), 3, "{wide:?}");
        // The control rides in its base cell as a zero-width mark: that cell never groups.
        let bidi = pieces("\u{e0b0}\u{200f}\u{e0b1}".as_bytes(), &rules(true));
        assert_eq!(bidi, vec![(0.0, vec![("\u{e0b0}\u{200f}".to_string(), 2)]), (10.0, vec![("\u{e0b1}".to_string(), 2)])]);
    }

    /// S2 (13n x 13o): a cell the terminal made wide groups with nothing, even when
    /// unicode-width calls its text one column (an ambiguous character under Ambiguous Double
    /// Width, a lone regional indicator).
    #[test]
    fn ligature_groups_skip_wide_cells() {
        use crate::fonts::LigatureRules;
        let prefs = DrawPrefs { ligatures: LigatureRules { ascii: false, non_ascii: true, prefer_speed: true, level: 2 }, ..DrawPrefs::default() };
        let pieces = |s: &Screen| {
            let mut f = Frame::default();
            pane_with(&mut f, s, RECT, CELL, false, &CursorLook::shown(false), &prefs, None, crate::render::DEFAULT_FACE);
            f.rows.iter().map(|r| (r.x, r.spans.iter().map(|s| s.text.trim_end().to_string()).collect::<String>())).collect::<Vec<_>>()
        };
        // Two narrow Thai letters group (the control case).
        assert_eq!(pieces(&screen("\u{e01}\u{e02}".as_bytes())), vec![(0.0, "\u{e01}\u{e02}".to_string())]);
        let ambiguous = crate::screen::Look { ambiguous_wide: true, ..crate::screen::Look::default() };
        let mut s = Screen::with_look(80, 24, crate::palette::Palette::default(), ambiguous);
        s.feed("\u{e01}\u{a7}\u{e02}".as_bytes());
        assert_eq!(pieces(&s), vec![(0.0, "\u{e01}".to_string()), (10.0, "\u{a7}".to_string()), (30.0, "\u{e02}".to_string())]);
        let flag = screen("\u{e01}\u{1f1e6}\u{e02}".as_bytes());
        assert_eq!(pieces(&flag), vec![(0.0, "\u{e01}".to_string()), (10.0, "\u{1f1e6}".to_string()), (30.0, "\u{e02}".to_string())]);
    }

    /// S3 (13n x 13o): U+200B in a cell of its own draws as a space, so the ASCII run around it
    /// stays one piece and every later character keeps its column.
    #[test]
    fn zwsp_cell_keeps_columns() {
        let s = screen("a\u{200B}b".as_bytes());
        let mut f = Frame::default();
        pane_with(&mut f, &s, RECT, CELL, false, &CursorLook::shown(false), &DrawPrefs::default(), None, crate::render::DEFAULT_FACE);
        let rows: Vec<(f32, String)> = f.rows.iter().map(|r| (r.x, r.spans.iter().map(|s| s.text.trim_end().to_string()).collect())).collect();
        assert_eq!(rows, vec![(0.0, "a b".to_string())]);
    }

    fn normal_screen(text: &str, form: crate::encoding::Form) -> Screen {
        let mut s = Screen::with_look(80, 24, crate::palette::Palette::default(), crate::screen::Look { normalization: form, ..crate::screen::Look::default() });
        s.feed(text.as_bytes());
        s
    }

    /// Unicode Normalization (13o Task 8): the span text is the cell's text in the profile's
    /// form (S1: before the ligature, box and font decisions that read it).
    #[test]
    fn normalization_form_reaches_the_drawn_text() {
        use crate::encoding::Form;
        let draw = |s: &Screen| {
            let mut f = Frame::default();
            pane_with(&mut f, s, RECT, CELL, false, &CursorLook::shown(false), &DrawPrefs::default(), None, crate::render::DEFAULT_FACE);
            f.rows.iter().map(|r| (r.x, r.spans.iter().map(|s| s.text.trim_end().to_string()).collect::<String>())).collect::<Vec<_>>()
        };
        assert_eq!(draw(&normal_screen("e\u{301}x", Form::Nfc)), vec![(0.0, "\u{e9}".to_string()), (10.0, "x".to_string())]);
        assert_eq!(draw(&normal_screen("\u{e9}x", Form::Nfd)), vec![(0.0, "e\u{301}".to_string()), (10.0, "x".to_string())]);
        assert_eq!(draw(&normal_screen("e\u{301}x", Form::None)), vec![(0.0, "e\u{301}".to_string()), (10.0, "x".to_string())]);
    }

    /// S12 (13o x 13p): the offscreen band's text is in the profile's normalization form, like
    /// the pane's own rows.
    #[test]
    fn offscreen_band_normalizes() {
        use crate::encoding::Form;
        let band = |s: &Screen| {
            let mut f = Frame::default();
            offscreen_band(&mut f, s, RECT, (0.0, 0.0), CELL, 0, ([0.0; 4], [1.0; 4]), crate::render::DEFAULT_FACE);
            f.rows.iter().map(|r| r.spans.iter().map(|s| s.text.as_str()).collect::<String>()).collect::<Vec<_>>()
        };
        assert_eq!(band(&normal_screen("\u{e9}x", Form::Nfd)), vec!["e\u{301}".to_string(), "x".to_string()]);
        assert_eq!(band(&normal_screen("e\u{301}x", Form::Nfc)), vec!["\u{e9}".to_string(), "x".to_string()]);
        assert_eq!(band(&normal_screen("e\u{301}x", Form::None)), vec!["e\u{301}".to_string(), "x".to_string()]);
    }

    /// 13m bidi and 13j cells: the display order, the cells and the columns of a row are the
    /// same with or without normalization; only the cell text differs.
    #[test]
    fn normalization_keeps_bidi_order_and_columns() {
        use crate::encoding::Form;
        let text = "e\u{301}b \u{5d0}\u{5d1}";
        let xs = |form| {
            let s = normal_screen(text, form);
            let mut f = Frame::default();
            pane(&mut f, &s, RECT, CELL, false, &CursorLook::shown(false));
            (piece_x(&f, '\u{5d0}'), piece_x(&f, '\u{5d1}'), piece_x(&f, 'b'), s.visual_col(0, 3), s.visual_col(0, 4))
        };
        assert_eq!(xs(Form::Nfc), xs(Form::None));
        assert_eq!(xs(Form::Nfc).0, Some(40.0));
    }

    fn table_prefs(non_ascii: bool, extra: &[crate::font_table::Entry]) -> DrawPrefs {
        use crate::font_table::{Config, Entry};
        let mut entries = vec![
            Entry { start: 0x41, count: 26, destination: Some(0x10000), font_name: "Consolas".into(), point_size: None },
            Entry { start: 0xE000, count: 0x100, destination: None, font_name: "Symbols".into(), point_size: Some(9.0) },
        ];
        entries.extend_from_slice(extra);
        DrawPrefs {
            ligatures: crate::fonts::LigatureRules { ascii: false, non_ascii, prefer_speed: true, level: 2 },
            table: Some(std::sync::Arc::new(Config::with_entries(entries))),
            ..DrawPrefs::default()
        }
    }

    /// The pieces of a screen as (x, [(text, font)]).
    fn table_pieces(text: &str, prefs: &DrawPrefs) -> Vec<(f32, Vec<(String, u16)>)> {
        let s = screen(text.as_bytes());
        let mut f = Frame::default();
        pane_with(&mut f, &s, RECT, CELL, false, &CursorLook::shown(false), prefs, None, crate::render::DEFAULT_FACE);
        f.rows.iter().map(|r| (r.x, r.spans.iter().map(|s| (s.text.trim_end().to_string(), s.font)).collect())).collect()
    }

    /// Special Font Config (Task 9): a table range is its own piece in its own font, the code
    /// points an entry remaps are the ones drawn, and a range boundary ends a piece even when
    /// Non-ASCII Ligatures groups the neighbors (a font cannot join across fonts, P7).
    #[test]
    fn table_ranges_split_pieces_and_remap() {
        // Entries sort by the code points the program sends: 1 is the 0xE000 range, 2 sends 0x10000.. and draws 0x41.. of Consolas.
        let off = table_prefs(false, &[]);
        let p = table_pieces("a\u{10000}\u{10005}b\u{e0b0}\u{e0b1}c\u{3a9}", &off);
        assert_eq!(
            p,
            vec![
                (0.0, vec![("a".to_string(), 0)]),
                (10.0, vec![("A".to_string(), 2)]),
                (20.0, vec![("F".to_string(), 2)]),
                (30.0, vec![("b".to_string(), 0)]),
                (40.0, vec![("\u{e0b0}".to_string(), 1)]),
                (50.0, vec![("\u{e0b1}".to_string(), 1)]),
                (60.0, vec![("c".to_string(), 0)]),
                (70.0, vec![("\u{3a9}".to_string(), 0)]),
            ],
            "{p:?}"
        );
        // Grouping on: neighbors of one entry share a piece, neighbors of two entries never do.
        let on = table_prefs(true, &[]);
        let p = table_pieces("\u{10000}\u{10005}\u{e0b0}\u{e0b1}\u{3a9}", &on);
        assert_eq!(
            p,
            vec![(0.0, vec![("AF".to_string(), 2)]), (20.0, vec![("\u{e0b0}\u{e0b1}".to_string(), 1)]), (40.0, vec![("\u{3a9}".to_string(), 0)])],
            "{p:?}"
        );
        let edge = table_pieces("\u{10019}\u{e000}", &on);
        assert_eq!(edge, vec![(0.0, vec![("Z".to_string(), 2)]), (10.0, vec![("\u{e000}".to_string(), 1)])], "{edge:?}");
        // No table: every character is as it was.
        let none = table_pieces("\u{10000}\u{e0b0}", &DrawPrefs::default());
        assert!(none.iter().all(|(_, spans)| spans.iter().all(|(_, font)| *font == 0)), "{none:?}");
        assert_eq!(none[0].1[0].0, "\u{10000}");
        // A combining mark rides on the remapped base.
        let combined = table_pieces("\u{10000}\u{301}", &off);
        assert_eq!(combined, vec![(0.0, vec![("A\u{301}".to_string(), 2)])], "{combined:?}");
        // A cell the table sends to a code point that is not a character draws as it was sent.
        let bad = crate::font_table::Entry { start: 0xD7FF, count: 3, destination: Some(0x20000), font_name: "Consolas".into(), point_size: None };
        let odd = table_pieces("\u{20001}", &table_prefs(false, &[bad]));
        assert_eq!(odd.len(), 1, "{odd:?}");
    }

    /// Powerline and box glyphs Tandem draws never go through the table (the preflight's T7 to
    /// T9 seam): the glyph is chosen from the character, the table only picks text fonts.
    #[test]
    fn table_never_overrides_drawn_glyphs() {
        let on = DrawPrefs { powerline: true, ..table_prefs(false, &[]) };
        let draw = |text: &str, prefs: &DrawPrefs| {
            let s = screen(text.as_bytes());
            let mut f = Frame::default();
            pane_with(&mut f, &s, RECT, CELL, false, &CursorLook::shown(false), prefs, None, crate::render::DEFAULT_FACE);
            f
        };
        let f = draw("\u{e0b0}", &on);
        assert_eq!((f.glyphs.len(), f.rows.len()), (1, 0), "the Powerline arrow is Tandem's");
        // With Draw Powerline Glyphs off the character is text, in the table's font.
        let f = draw("\u{e0b0}", &table_prefs(false, &[]));
        assert_eq!((f.glyphs.len(), f.rows.len()), (0, 1));
        assert_eq!(f.rows[0].spans[0].font, 1);
        // A box character inside a range stays a glyph.
        let boxes = crate::font_table::Entry { start: 0x2500, count: 0x80, destination: None, font_name: "Consolas".into(), point_size: None };
        let f = draw("\u{2500}", &table_prefs(false, &[boxes.clone()]));
        assert_eq!((f.glyphs.len(), f.rows.len()), (1, 0));
        // With the font drawing boxes (DisableCustomBoxDrawing) it is text in the table's font.
        let f = draw("\u{2500}", &DrawPrefs { custom_boxes: false, ..table_prefs(false, &[boxes]) });
        assert_eq!((f.glyphs.len(), f.rows.len()), (0, 1));
        assert!(f.rows[0].spans[0].font >= 1);
    }

    /// What is drawn is the table's code point; what the screen holds, and so what is selected and
    /// copied, is what the program sent (S9).
    #[test]
    fn copy_ignores_the_table_remap() {
        let s = screen("\u{10000}\u{10005}".as_bytes());
        let mut f = Frame::default();
        pane_with(&mut f, &s, RECT, CELL, false, &CursorLook::shown(false), &table_prefs(false, &[]), None, crate::render::DEFAULT_FACE);
        assert_eq!(f.rows.iter().map(|r| r.spans[0].text.clone()).collect::<Vec<_>>(), vec!["A", "F"]);
        assert_eq!(s.line_text(0).trim_end(), "\u{10000}\u{10005}");
    }

    fn box_glyph(x: f32, y: f32, w: f32) -> CellGlyph {
        CellGlyph { c: '\u{2500}', rect: Rect { x, y, w, h: 20.0 }, color: rgb(1, 2, 3), face: 0, clip: RECT }
    }

    /// What of each glyph shows: its cell clipped to its clip, sorted left to right.
    fn shown(f: &Frame) -> Vec<(f32, f32, f32, f32)> {
        let mut v: Vec<(f32, f32, f32, f32)> = f
            .glyphs
            .iter()
            .map(|g| {
                let (x0, x1) = (g.rect.x.max(g.clip.x), (g.rect.x + g.rect.w).min(g.clip.x + g.clip.w));
                let (y0, y1) = (g.rect.y.max(g.clip.y), (g.rect.y + g.rect.h).min(g.clip.y + g.clip.h));
                (x0, y0, x1 - x0, y1 - y0)
            })
            .collect();
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        v
    }

    /// Review Focus 4: Tandem draws the box and block characters (a glyph each, out of the text),
    /// unless `DisableCustomBoxDrawing` leaves them to the font.
    #[test]
    fn box_drawing_switch_follows_iterm2() {
        let s = screen("\u{2500}\u{2500}a".as_bytes());
        let mut f = Frame::default();
        pane_with(&mut f, &s, RECT, CELL, false, &CursorLook::shown(false), &DrawPrefs::default(), None, crate::render::DEFAULT_FACE);
        assert_eq!(f.glyphs.len(), 2, "{:?}", f.glyphs);
        assert_eq!(f.glyphs[0].rect, Rect { x: 0.0, y: 0.0, w: 10.0, h: 20.0 });
        assert_eq!(f.glyphs[1].rect, Rect { x: 10.0, y: 0.0, w: 10.0, h: 20.0 });
        assert_eq!((f.glyphs[0].c, f.glyphs[0].color, f.glyphs[0].clip, f.glyphs[0].face), ('\u{2500}', s.palette.fg, RECT, crate::render::DEFAULT_FACE));
        assert_eq!(f.rows.len(), 1, "only the letter is text");
        assert_eq!((row_text(&f, 0).trim_end(), f.rows[0].x), ("a", 20.0));
        // With the switch on the font draws all three.
        let off = DrawPrefs { custom_boxes: false, ..DrawPrefs::default() };
        let mut f = Frame::default();
        pane_with(&mut f, &s, RECT, CELL, false, &CursorLook::shown(false), &off, None, crate::render::DEFAULT_FACE);
        assert!(f.glyphs.is_empty());
        assert_eq!((0..f.rows.len()).map(|i| row_text(&f, i).trim_end().to_string()).collect::<String>(), "\u{2500}\u{2500}a");
    }

    #[test]
    fn a_box_cell_ends_the_text_piece_around_it() {
        let s = screen("ab\u{2502}cd".as_bytes());
        let mut f = Frame::default();
        pane_with(&mut f, &s, RECT, CELL, false, &CursorLook::shown(false), &DrawPrefs::default(), None, crate::render::DEFAULT_FACE);
        assert_eq!((0..f.rows.len()).map(|i| (f.rows[i].x, row_text(&f, i).trim_end().to_string())).collect::<Vec<_>>(), vec![(0.0, "ab".to_string()), (30.0, "cd".to_string())]);
        assert_eq!(f.glyphs.len(), 1);
        assert_eq!(f.glyphs[0].rect.x, 20.0);
        // The glyph takes the cell's own color.
        let s = screen("\x1b[31m\u{2588}".as_bytes());
        let mut f = Frame::default();
        pane_with(&mut f, &s, RECT, CELL, false, &CursorLook::shown(false), &DrawPrefs::default(), None, crate::render::DEFAULT_FACE);
        assert_eq!(f.glyphs[0].color, s.palette.ansi[1]);
    }

    /// B1(a): a popup over box glyphs cuts them like text: whole, partly, or in two.
    #[test]
    fn glyphs_are_cut_under_popups() {
        let mut f = Frame::default();
        f.glyphs = vec![box_glyph(0.0, 0.0, 10.0), box_glyph(10.0, 0.0, 10.0), box_glyph(20.0, 0.0, 10.0), box_glyph(40.0, 0.0, 10.0), box_glyph(100.0, 0.0, 60.0), box_glyph(0.0, 60.0, 10.0)];
        // A popup from x 15 to 45 over the first row, and one from x 120 to 140 inside the wide glyph.
        cut_out(&mut f, Rect { x: 15.0, y: 0.0, w: 30.0, h: 20.0 }, 20.0);
        cut_out(&mut f, Rect { x: 120.0, y: 0.0, w: 20.0, h: 20.0 }, 20.0);
        assert_eq!(
            shown(&f),
            vec![(0.0, 0.0, 10.0, 20.0), (0.0, 60.0, 10.0, 20.0), (10.0, 0.0, 5.0, 20.0), (45.0, 0.0, 5.0, 20.0), (100.0, 0.0, 20.0, 20.0), (140.0, 0.0, 20.0, 20.0)],
            "the third cell is gone, the second and fourth lose their covered part, the wide one splits"
        );
        // Every piece keeps its cell, so its mask stays the same.
        assert!(f.glyphs.iter().filter(|g| g.rect.w == 60.0).count() == 2);
        // A box drawn over rows (find bar, toast) cuts everything from its left edge on, in the rows it touches.
        let mut f = Frame::default();
        f.glyphs = vec![box_glyph(0.0, 0.0, 10.0), box_glyph(30.0, 0.0, 10.0), box_glyph(30.0, 60.0, 10.0), box_glyph(5.0, 20.0, 10.0)];
        cut_rows_from(&mut f, Rect { x: 20.0, y: 0.0, w: 50.0, h: 30.0 }, 20.0);
        assert_eq!(shown(&f), vec![(0.0, 0.0, 10.0, 20.0), (5.0, 20.0, 10.0, 20.0), (30.0, 60.0, 10.0, 20.0)]);
    }

    /// B1(a), task 6 review: a timestamp label cuts the box glyphs under it as it cuts text, in both
    /// modes (right-aligned over the text, and the Adjacent block).
    #[test]
    fn timestamps_cut_box_glyphs() {
        // Row 3 is y 60; the label block is 100 px wide at the right edge of RECT (800 px).
        let glyphs = || vec![box_glyph(100.0, 60.0, 10.0), box_glyph(695.0, 60.0, 10.0), box_glyph(750.0, 60.0, 10.0), box_glyph(750.0, 0.0, 10.0)];
        let mut f = Frame::default();
        f.glyphs = glyphs();
        timestamps(&mut f, &[(3, "12:00:00".to_string())], RECT, CELL, None, STAMP_COLORS);
        assert_eq!(shown(&f), vec![(100.0, 60.0, 10.0, 20.0), (695.0, 60.0, 5.0, 20.0), (750.0, 0.0, 10.0, 20.0)], "the label's block covers the glyphs under it, nothing else");
        // Adjacent: the block starts at the same x on every labelled row and covers what is under it.
        let col = Rect { x: 700.0, y: 0.0, w: 100.0, h: 200.0 };
        let mut f = Frame::default();
        f.glyphs = glyphs();
        // A block wider than the column extends left over the text, which stops where it starts.
        timestamps(&mut f, &[(3, "2026-10-05 12:00:00 CDT".to_string())], Rect { x: 0.0, y: 0.0, w: 700.0, h: 200.0 }, CELL, Some(col), STAMP_COLORS);
        let x = f.quads.iter().find(|q| q.rect.y == 60.0).map(|q| q.rect.x).expect("the label block");
        assert!(x < 695.0, "the block is wider than the column: {x}");
        let seen = shown(&f);
        assert!(seen.iter().all(|g| g.1 != 60.0 || g.0 + g.2 <= x), "no glyph shows under the block at x {x}: {seen:?}");
        assert!(seen.contains(&(100.0, 60.0, 10.0, 20.0)) && seen.contains(&(750.0, 0.0, 10.0, 20.0)));
        assert!(!seen.iter().any(|g| g.0 == 695.0 && g.1 == 60.0 && g.2 == 10.0), "the glyph at 695 is cut: {seen:?}");
    }

    /// B1(a), task 6 review: the announcement bar hides the box glyphs under it as it hides text.
    #[test]
    fn info_bar_cuts_box_glyphs() {
        let bar = crate::infobar::Bar { id: "x".into(), text: "hi".into(), buttons: vec!["_Yes".into()], important: true };
        let l = crate::infobar::layout(&bar, RECT, CELL);
        let mut f = Frame::default();
        f.glyphs = vec![box_glyph(l.bar.x + 20.0, l.bar.y, 10.0), box_glyph(l.bar.x + 20.0, l.bar.y + l.bar.h + 40.0, 10.0)];
        info_bar(&mut f, &bar, &l, CELL);
        assert_eq!(shown(&f), vec![(l.bar.x + 20.0, l.bar.y + l.bar.h + 40.0, 10.0, 20.0)], "the glyph under the bar is gone, the one below stays");
    }

    #[test]
    fn lay_over_clips_glyphs() {
        let mut f = Frame::default();
        f.glyphs = vec![box_glyph(0.0, 0.0, 10.0), box_glyph(0.0, 40.0, 10.0)];
        let mut layer = Frame::default();
        layer.glyphs = vec![box_glyph(0.0, 0.0, 10.0)];
        let area = Rect { x: 0.0, y: 0.0, w: 100.0, h: 20.0 };
        let under = rgb(0, 0, 0);
        let mut over = f.clone();
        lay_over(&mut over, layer.clone(), area, 0.25, under, 20.0);
        assert_eq!(over.glyphs.len(), 3, "a faint bar leaves the sessions' glyphs");
        let mut over = f;
        lay_over(&mut over, layer, area, 1.0, under, 20.0);
        assert_eq!(over.glyphs.len(), 2, "an opaque bar hides the glyph behind it and brings its own");
        assert!(over.glyphs.iter().any(|g| g.rect.y == 40.0));
        assert!(over.glyphs.iter().any(|g| g.rect.y == 0.0 && g.color == rgb(1, 2, 3)), "full alpha keeps the layer's color");
    }

    /// Task 5 fix 1: Thin Strokes on dark backgrounds is decided per cell, as iTerm2 does (the
    /// cell's background against its text, `iTermTextDrawingHelper.m:3045`), not once per pane.
    #[test]
    fn thin_strokes_follow_each_cells_colors() {
        use crate::fonts::{MaskMode, MaskRules};
        let prefs = DrawPrefs { masks: MaskRules { thin: 2, retina: false, ..MaskRules::STOCK }, ..DrawPrefs::default() };
        let thin = MaskMode::Thin.code();
        let mask_of = |s: &Screen, text: &str| {
            let mut f = Frame::default();
            pane_with(&mut f, s, RECT, CELL, false, &CursorLook::shown(false), &prefs, None, crate::render::DEFAULT_FACE);
            f.rows.iter().flat_map(|r| r.spans.iter()).find(|sp| sp.text.trim_end() == text).map(|sp| sp.mask).unwrap_or(255)
        };
        // A dark pane: its own text is lighter than its background, so it thins; black text on a
        // white cell is darker than its cell, so it does not.
        let dark = screen(b"a\x1b[30;47mb");
        assert_eq!((mask_of(&dark, "a"), mask_of(&dark, "b")), (thin, MaskMode::Smooth.code()));
        // A light pane: dark text on light does not thin; white text on a blue cell does.
        let mut light = screen(b"a\x1b[97;44mb");
        light.palette.bg = rgb(255, 255, 255);
        light.palette.fg = rgb(0, 0, 0);
        assert_eq!((mask_of(&light, "a"), mask_of(&light, "b")), (MaskMode::Smooth.code(), thin));
    }

    #[test]
    fn a_red_background_makes_a_quad() {
        let s = screen(b"\x1b[41mX");
        let mut f = Frame::default();
        pane(&mut f, &s, RECT, CELL, true, &CursorLook::shown(true));
        let red = rgba(s.palette.ansi[1], 1.0);
        assert!(f.quads.iter().any(|q| q.color == red && q.rect == Rect { x: 0.0, y: 0.0, w: 10.0, h: 20.0 }), "{:?}", f.quads);
    }

    #[test]
    fn rows_carry_the_text() {
        let s = screen(b"hello\r\nworld");
        let mut f = Frame::default();
        pane(&mut f, &s, RECT, CELL, true, &CursorLook::shown(true));
        assert_eq!(row_text(&f, 0).trim_end(), "hello");
        assert_eq!(row_text(&f, 1).trim_end(), "world");
        assert_eq!((f.rows[1].x, f.rows[1].y), (0.0, 20.0));
    }

    #[test]
    fn focused_block_cursor_is_a_quad() {
        let s = screen(b"ab");
        let mut f = Frame::default();
        pane(&mut f, &s, RECT, CELL, true, &CursorLook::shown(true));
        let cursor = rgba(s.palette.cursor, 1.0);
        assert!(f.quads.iter().any(|q| q.color == cursor && q.rect.x == 20.0 && q.rect.y == 0.0));
    }

    #[test]
    fn unfocused_pane_is_dimmed_and_cursor_hollow() {
        let s = screen(b"ab");
        let mut f = Frame::default();
        pane(&mut f, &s, RECT, CELL, false, &CursorLook::shown(true));
        assert!(f.overlay.iter().any(|q| q.rect == RECT));
        assert!(!f.quads.iter().any(|q| q.color == rgba(s.palette.cursor, 1.0)));
    }

    #[test]
    fn hidden_when_unfocused_follows_iterm2() {
        let s = screen(b"ab");
        let cursor = rgba(s.palette.cursor, 1.0);
        let hide = CursorLook { hide_unfocused: true, ..CursorLook::shown(true) };
        // Unfocused with the key on: no box and no hollow edges.
        let mut f = Frame::default();
        pane(&mut f, &s, RECT, CELL, false, &hide);
        assert!(!f.quads.iter().any(|q| q.color == cursor));
        assert!(!f.overlay.iter().any(|q| q.color == cursor));
        // Focused: the block quad is there.
        let mut f = Frame::default();
        pane(&mut f, &s, RECT, CELL, true, &hide);
        assert!(f.quads.iter().any(|q| q.color == cursor && q.rect.x == 20.0));
        // Key off and unfocused: the four hollow edges, as before.
        let mut f = Frame::default();
        pane(&mut f, &s, RECT, CELL, false, &CursorLook::shown(true));
        assert_eq!(f.overlay.iter().filter(|q| q.color == cursor).count(), 4);
    }

    fn see_through(bg_alpha: f32, opaque_colors: bool) -> Highlights<'static> {
        Highlights { matches: &[], color: rgb(0, 0, 0), hover: None, link: rgb(0, 0, 0), labels: &[], notes: &[], dim: crate::dim::Dim::LEGACY, key_cursor: false, bg_alpha, opaque_colors, text_blink_on: true }
    }

    /// "X" on red, then a selected "a", then a plain "b".
    fn red_selected_default() -> Screen {
        let mut s = screen(b"\x1b[41mX\x1b[0mab");
        let mut sel = Selection::new(SelectionType::Simple, Point::new(Line(0), Column(1)), Side::Left);
        sel.update(Point::new(Line(0), Column(1)), Side::Right);
        s.term_mut().selection = Some(sel);
        s
    }

    fn quad_alpha_at(f: &Frame, x: f32, y: f32) -> Vec<f32> {
        f.quads.iter().filter(|q| q.rect.contains(x, y)).map(|q| q.color[3]).collect()
    }

    #[test]
    fn a_pane_draws_every_row_in_its_face() {
        // The pane's text rows, a find label and a note all take the face `pane_with` is given.
        let mut s = Screen::new(10, 3);
        s.feed(b"hello \xe4\xb8\xad world");
        let mut f = Frame::default();
        let hl = Highlights { matches: &[], color: rgb(255, 255, 0), hover: None, link: rgb(0, 0, 255), labels: &[(crate::find::Pos { row: 0, col: 1 }, "a".to_string())], notes: &[], dim: crate::dim::Dim::LEGACY, key_cursor: false, bg_alpha: 1.0, opaque_colors: false, text_blink_on: true };
        pane_with(&mut f, &s, RECT, CELL, true, &CursorLook::shown(true), &DrawPrefs::default(), Some(&hl), 7);
        assert!(f.rows.len() >= 3, "{:?}", f.rows);
        assert!(f.rows.iter().all(|r| r.face == 7), "{:?}", f.rows.iter().map(|r| r.face).collect::<Vec<_>>());
        let mut g = Frame::default();
        pane(&mut g, &s, RECT, CELL, true, &CursorLook::shown(true));
        assert!(!g.rows.is_empty() && g.rows.iter().all(|r| r.face == crate::render::DEFAULT_FACE), "pane() is the default face");
    }

    #[test]
    fn cell_backgrounds_take_the_alpha() {
        let s = red_selected_default();
        let mut f = Frame::default();
        pane_with(&mut f, &s, RECT, CELL, false, &CursorLook::shown(false), &DrawPrefs::default(), Some(&see_through(0.7, false)), crate::render::DEFAULT_FACE);
        assert_eq!(quad_alpha_at(&f, 5.0, 10.0), vec![0.7], "{:?}", f.quads);
        assert_eq!(quad_alpha_at(&f, 15.0, 10.0), vec![0.7], "{:?}", f.quads);
    }

    #[test]
    fn opaque_colors_keep_cells_solid() {
        let s = red_selected_default();
        let mut f = Frame::default();
        pane_with(&mut f, &s, RECT, CELL, false, &CursorLook::shown(false), &DrawPrefs::default(), Some(&see_through(0.7, true)), crate::render::DEFAULT_FACE);
        assert_eq!(quad_alpha_at(&f, 5.0, 10.0), vec![1.0], "{:?}", f.quads);
        assert_eq!(quad_alpha_at(&f, 15.0, 10.0), vec![1.0], "{:?}", f.quads);
        assert_eq!(quad_alpha_at(&f, 25.0, 10.0), vec![0.7], "{:?}", f.quads);
    }

    #[test]
    fn cursor_stays_opaque() {
        let s = screen(b"");
        let mut f = Frame::default();
        pane_with(&mut f, &s, RECT, CELL, true, &CursorLook::shown(true), &DrawPrefs::default(), Some(&see_through(0.5, false)), crate::render::DEFAULT_FACE);
        let top = f.quads.iter().rev().find(|q| q.rect.contains(5.0, 10.0)).unwrap();
        assert_eq!(top.color, rgba(s.palette.cursor, 1.0), "{:?}", f.quads);
    }

    #[test]
    fn block_cursor_in_a_see_through_pane_keeps_the_cell_background_alpha() {
        let s = screen(b"");
        let mut f = Frame::default();
        pane_with(&mut f, &s, RECT, CELL, true, &CursorLook::shown(true), &DrawPrefs::default(), Some(&see_through(0.5, false)), crate::render::DEFAULT_FACE);
        assert_eq!(quad_alpha_at(&f, 5.0, 10.0), vec![0.5, 1.0], "{:?}", f.quads);
    }

    #[test]
    fn block_cursor_is_spacing_free() {
        // A line 4 taller than the font's cell: the box is centered and the text takes cursor_text.
        let s = screen(b"ab\x1b[1;2H");
        let mut f = Frame::default();
        let mut look = CursorLook::shown(true);
        look.geometry.base_cell = (20.0, 16.0);
        pane(&mut f, &s, RECT, CELL, true, &look);
        let cursor = rgba(s.palette.cursor, 1.0);
        assert!(f.quads.iter().any(|q| q.color == cursor && q.rect == Rect { x: 10.0, y: 2.0, w: 10.0, h: 16.0 }), "{:?}", &f.quads[..3]);
        let b = f.rows[0].spans.iter().find(|sp| sp.text == "b").unwrap();
        assert_eq!(b.color, s.palette.cursor_text, "the text takes cursor_text");
    }

    #[test]
    fn ime_composition_draws_over_the_grid() {
        use crate::ime::{Drawn, Piece};
        let s = screen(b"hello");
        let cell = (20.0, 20.0);
        let yellow = rgb(255, 255, 0);
        let mut look = CursorLook::shown(true);
        look.geometry.base_cell = (20.0, 20.0);
        look.ime = Some(Drawn { pieces: vec![Piece { row: 0, col: 5, text: "ab".to_string(), cells: 2 }], caret: (0, 7), caret_color: yellow });
        let mut f = Frame::default();
        pane(&mut f, &s, RECT, cell, true, &look);
        let bg = rgba(s.palette.bg, 1.0);
        assert!(f.popups.iter().any(|q| q.color == bg && q.rect == Rect { x: 100.0, y: 0.0, w: 40.0, h: 20.0 }), "{:?}", f.popups);
        let row = f.rows.iter().find(|r| r.spans.iter().any(|sp| sp.text == "ab")).expect("composition text");
        assert_eq!((row.x, row.y), (100.0, 0.0));
        assert_eq!(row.spans[0].color, s.palette.fg);
        assert!(f.overlay.iter().any(|q| q.rect == Rect { x: 100.0, y: 19.0, w: 40.0, h: 1.0 } && q.color == rgba(s.palette.fg, 1.0)), "underline: {:?}", f.overlay);
        assert!(f.overlay.iter().any(|q| q.rect.x == 140.0 && q.rect.w == 2.0 && q.color == rgba(yellow, 1.0)), "caret: {:?}", f.overlay);
        // The grid's cell under the cursor is not a cursor quad: the terminal cursor is hidden.
        let cursor = rgba(s.palette.cursor, 1.0);
        assert!(!f.quads.iter().chain(&f.overlay).any(|q| q.color == cursor), "no terminal cursor");
    }

    #[test]
    fn ime_cell_survives_a_hidden_cursor_and_a_scrolled_view() {
        // A program that hid the cursor: the composition is still at its cell.
        assert_eq!(ime_cell(&screen(b"ab\x1b[?25l")), (0, 2));
        // Scrolled back so the cursor is off the view: the bottom-left of the visible grid.
        let mut s = screen(&b"x\r\n".repeat(60));
        s.term_mut().scroll_display(alacritty_terminal::grid::Scroll::Delta(10));
        assert_eq!(ime_cell(&s), (23, 0));
    }

    #[test]
    fn smooth_blink_fades_the_cursor() {
        // A block at alpha 0.5: the quad is half opaque and the text is halfway to cursor_text.
        let s = screen(b"ab\x1b[1;2H");
        let mut f = Frame::default();
        pane(&mut f, &s, RECT, CELL, true, &CursorLook { alpha: 0.5, ..CursorLook::shown(true) });
        assert!(f.quads.iter().any(|q| q.color == rgba(s.palette.cursor, 0.5) && q.rect.x == 10.0), "{:?}", f.quads);
        let b = f.rows[0].spans.iter().find(|sp| sp.text == "b").unwrap();
        assert_eq!(b.color, blend(s.palette.fg, s.palette.cursor_text, 0.5));
        // A beam at alpha 0.5 is a half opaque quad.
        let s = screen(b"ab\x1b[6 q\x1b[1;2H");
        let mut f = Frame::default();
        pane(&mut f, &s, RECT, CELL, true, &CursorLook { alpha: 0.5, ..CursorLook::shown(true) });
        assert!(f.overlay.iter().any(|q| q.color == rgba(s.palette.cursor, 0.5)), "{:?}", f.overlay);
        // At 0.001 and below iTerm2 draws no cursor: no quad, and the text keeps its own color.
        let s = screen(b"ab\x1b[1;2H");
        let mut f = Frame::default();
        pane(&mut f, &s, RECT, CELL, true, &CursorLook { alpha: 0.0005, ..CursorLook::shown(true) });
        let c = rgba(s.palette.cursor, 1.0)[..3].to_vec();
        assert!(!f.quads.iter().chain(f.overlay.iter()).any(|q| q.color[..3] == c[..]), "{:?}", f.quads);
        assert!(f.rows[0].spans.iter().all(|sp| sp.color == s.palette.fg), "{:?}", f.rows[0].spans);
    }

    #[test]
    fn smart_cursor_draws_its_colors() {
        // White on black, every neighbor black: the box is the text color, the text the background.
        let setup: &[u8] = b"\x1b[38;2;255;255;255m\x1b[48;2;0;0;0m\x1b[2J\x1b[Hab\x1b[1;2H";
        let mut s = screen(setup);
        s.palette.cursor = rgb(255, 0, 0);
        s.palette.cursor_text = rgb(0, 0, 255);
        let smart = CursorLook { smart: Some((0.5, 0.75)), ..CursorLook::shown(true) };
        let mut f = Frame::default();
        pane(&mut f, &s, RECT, CELL, true, &smart);
        assert!(f.quads.iter().any(|q| q.color == rgba(rgb(255, 255, 255), 1.0) && q.rect.x == 10.0), "{:?}", f.quads);
        let b = f.rows[0].spans.iter().find(|sp| sp.text == "b").unwrap();
        assert_eq!(b.color, rgb(0, 0, 0));
        // Unfocused: the hollow edges use the profile's cursor color.
        let mut f = Frame::default();
        pane(&mut f, &s, RECT, CELL, false, &smart);
        let cc = rgba(s.palette.cursor, 1.0);
        assert!(f.overlay.iter().any(|q| q.color == cc), "{:?}", f.overlay);
        assert!(!f.overlay.iter().any(|q| q.color == rgba(rgb(255, 255, 255), 1.0)), "{:?}", f.overlay);
        // A beam ignores smart colors.
        let mut s = screen(b"\x1b[38;2;255;255;255m\x1b[48;2;0;0;0m\x1b[2J\x1b[Hab\x1b[6 q\x1b[1;2H");
        s.palette.cursor = rgb(255, 0, 0);
        let mut f = Frame::default();
        pane(&mut f, &s, RECT, CELL, true, &smart);
        assert!(f.overlay.iter().any(|q| q.color == rgba(s.palette.cursor, 1.0)), "{:?}", f.overlay);
        // A neighbor as bright as the text pushes the box to black (the farthest gray); the text keeps the cell background (0.9 from black, past 0.75).
        let s = screen(b"\x1b[38;2;255;255;255m\x1b[48;2;230;230;230m\x1b[2J\x1b[Hab\x1b[1;2H");
        let mut f = Frame::default();
        pane(&mut f, &s, RECT, CELL, true, &smart);
        assert!(f.quads.iter().any(|q| q.color == rgba(rgb(0, 0, 0), 1.0) && q.rect.x == 10.0), "{:?}", f.quads);
        let b = f.rows[0].spans.iter().find(|sp| sp.text == "b").unwrap();
        assert_eq!(b.color, rgb(230, 230, 230));
    }

    fn with_min_contrast(s: &mut Screen, min: f32) {
        s.palette.min_contrast = min;
    }

    fn span_color(f: &Frame, text: &str) -> Option<Rgb> {
        f.rows.iter().flat_map(|r| &r.spans).find(|sp| sp.text.contains(text)).map(|sp| sp.color)
    }

    #[test]
    fn blink_follows_the_profile() {
        // A blinking "a" on red, then a plain "b".
        let mut s = screen(b"\x1b[41;5ma\x1b[0mb");
        let draw = |s: &Screen, on: bool| {
            let mut f = Frame::default();
            let hl = Highlights { text_blink_on: on, ..see_through(1.0, false) };
            pane_with(&mut f, s, RECT, CELL, false, &CursorLook::shown(false), &DrawPrefs::default(), Some(&hl), crate::render::DEFAULT_FACE);
            f
        };
        let red = rgba(s.palette.ansi[1], 1.0);
        // Not allowed: the off phase hides nothing.
        let f = draw(&s, false);
        assert_eq!(row_text(&f, 0).trim_end(), "ab");
        // Allowed and off: the glyph is gone, the background and the neighbor stay.
        s.blink_allowed = true;
        let f = draw(&s, false);
        assert_eq!(row_text(&f, 0).trim_end(), " b", "{:?}", f.rows);
        assert!(f.quads.iter().any(|q| q.color == red && q.rect.x == 0.0), "background still draws");
        // Allowed and on: drawn.
        let f = draw(&s, true);
        assert_eq!(row_text(&f, 0).trim_end(), "ab");
        // No highlights (previews, screenshots): drawn.
        let mut f = Frame::default();
        pane_with(&mut f, &s, RECT, CELL, false, &CursorLook::shown(false), &DrawPrefs::default(), None, crate::render::DEFAULT_FACE);
        assert_eq!(row_text(&f, 0).trim_end(), "ab");
    }

    /// Phase B (13l's ruling, from `iTermMetalPerFrameState.m`): a blinking cell in its off phase
    /// draws no underline and no strikethrough either; its neighbors keep theirs.
    #[test]
    fn blink_off_draws_no_underline_or_strike() {
        let mut s = screen(b"\x1b[4;9;5ma\x1b[0;4mb");
        s.blink_allowed = true;
        let draw = |s: &Screen, on: bool| {
            let mut f = Frame::default();
            let hl = Highlights { text_blink_on: on, ..see_through(1.0, false) };
            pane_with(&mut f, s, RECT, CELL, false, &CursorLook::shown(false), &DrawPrefs::default(), Some(&hl), crate::render::DEFAULT_FACE);
            f
        };
        let at = |qs: &[Quad], x: f32| qs.iter().any(|q| q.rect.x >= x && q.rect.x < x + 10.0);
        let f = draw(&s, false);
        assert!(!at(&f.underlines, 0.0), "no underline under the hidden glyph {:?}", f.underlines);
        assert!(!at(&f.strikes, 0.0), "no strikethrough through it {:?}", f.strikes);
        assert!(at(&f.underlines, 10.0), "the neighbor keeps its underline");
        let f = draw(&s, true);
        assert!(at(&f.underlines, 0.0) && at(&f.strikes, 0.0), "on phase draws both");
        // Not allowed: the off phase hides nothing, so the runs stay.
        s.blink_allowed = false;
        let f = draw(&s, false);
        assert!(at(&f.underlines, 0.0) && at(&f.strikes, 0.0));
    }

    /// Phase B (preflight T2 -> T7): a line-style mark takes the pane's text rect, so it stops
    /// at the text's last column and never runs under the scrollbar strip or the stamp column.
    #[test]
    fn line_marks_stay_in_the_text_rect() {
        let text = Rect { x: 5.0, y: 2.0, w: 800.0 - 10.0 - 15.0, h: 480.0 };
        let mut f = Frame::default();
        line_mark(&mut f, text, 3, CELL, rgb(1, 2, 3), false, 77);
        let q = f.overlay[0].rect;
        assert_eq!((q.x, q.y, q.h), (5.0, (2.0 + 2.5 * 20.0f32).round(), 1.0));
        assert!(q.x + q.w <= text.x + text.w, "{q:?} ends inside the text rect");
        assert_eq!(q.w, 770.0);
    }

    /// Phase B: underline, strikethrough and OSC 8 runs on a right-to-left row sit under the
    /// cells where they draw (the display order), and a logical run that is not contiguous on
    /// screen is several runs.
    #[test]
    fn underline_runs_follow_the_display_order() {
        let xs = |qs: &[Quad]| -> Vec<(f32, f32)> { qs.iter().map(|q| (q.rect.x, q.rect.x + q.rect.w)).collect() };
        let draw = |bytes: &str| {
            let s = screen(bytes.as_bytes());
            let mut f = Frame::default();
            pane(&mut f, &s, RECT, CELL, false, &CursorLook::shown(false));
            f
        };
        // Underline only the aleph of aleph bet gimel. A hebrew row draws gimel bet aleph from
        // the left, so the aleph is the third cell: x 20..30, never 0..10.
        let f = draw("\x1b[4m\u{5d0}\x1b[0m\u{5d1}\u{5d2}");
        assert_eq!(xs(&f.underlines), vec![(20.0, 30.0)], "the aleph draws third");
        // Strike only the bet: the middle cell either way, so a strike on the gimel pins order too.
        let f = draw("\u{5d0}\u{5d1}\x1b[9m\u{5d2}");
        assert_eq!(xs(&f.strikes), vec![(0.0, 10.0)], "the gimel draws first");
        // An OSC 8 link over the aleph only of "ab " + aleph bet: the aleph draws at column 4.
        let f = draw("ab \x1b]8;;http://x\x07\u{5d0}\x1b]8;;\x07\u{5d1}");
        let u = xs(&f.underlines);
        assert!(!u.is_empty() && u.iter().all(|&(a, b)| a >= 40.0 && b <= 50.0), "the link is a dash under column 4 only: {u:?}");
        // A pair that crosses a direction change: a, aleph, bet, b with the underline on logical
        // cells 0 and 1 (a and aleph). Display order is a, bet, aleph, b: columns 0 and 2, two runs.
        let f = draw("\x1b[4ma\u{5d0}\x1b[0m\u{5d1}b");
        assert_eq!(xs(&f.underlines), vec![(0.0, 10.0), (20.0, 30.0)], "two runs, not one");
    }

    /// The x of the first text piece holding `ch`.
    fn piece_x(f: &Frame, ch: char) -> Option<f32> {
        f.rows.iter().find(|r| r.spans.iter().any(|sp| sp.text.contains(ch))).map(|r| r.x)
    }

    #[test]
    fn bidi_rows_draw_in_display_order() {
        let mut s = screen("ab \u{5d0}\u{5d1}".as_bytes());
        let mut f = Frame::default();
        pane(&mut f, &s, RECT, CELL, false, &CursorLook::shown(false));
        assert_eq!(piece_x(&f, '\u{5d1}'), Some(30.0), "bet is drawn at column 3");
        assert_eq!(piece_x(&f, '\u{5d0}'), Some(40.0), "aleph at column 4");
        // Bidi off: logical order.
        s.bidi_rules.enabled = false;
        let mut f = Frame::default();
        pane(&mut f, &s, RECT, CELL, false, &CursorLook::shown(false));
        assert_eq!(piece_x(&f, '\u{5d0}'), Some(30.0));
        assert_eq!(piece_x(&f, '\u{5d1}'), Some(40.0));
    }

    #[test]
    fn bidi_mirrors_brackets_and_moves_the_row_together() {
        // aleph ( bet ) on a red background from column 1: the colors travel with their cells,
        // and the brackets are drawn as their counterparts.
        let s = screen("\u{5d0}\x1b[41m(\u{5d1})\x1b[0m".as_bytes());
        let mut f = Frame::default();
        pane(&mut f, &s, RECT, CELL, false, &CursorLook::shown(false));
        // Logical: aleph(0) ( (1) bet(2) ) (3). All right to left: ) bet ( aleph, so the
        // logical ")" draws mirrored as "(" at column 0 and "(" as ")" at column 2.
        let at = |col: f32| f.rows.iter().find(|r| r.x == col * 10.0).map(|r| r.spans.iter().map(|sp| sp.text.as_str()).collect::<String>());
        assert_eq!(at(0.0).as_deref(), Some("("), "{:?}", f.rows);
        assert_eq!(at(1.0).as_deref(), Some("\u{5d1}"));
        assert_eq!(at(2.0).as_deref(), Some(")"));
        assert_eq!(at(3.0).as_deref(), Some("\u{5d0}"));
        // The red background of logical cells 1..=3 now covers visual columns 0..=2.
        let red = rgba(s.palette.ansi[1], 1.0);
        assert!(f.quads.iter().any(|q| q.color == red && q.rect.x == 0.0 && q.rect.w == 30.0), "{:?}", f.quads);
    }

    #[test]
    fn bidi_cursor_and_hover_follow_the_display_order() {
        // The cursor sits on the aleph (logical column 3), which draws at column 4.
        let s = screen("ab \u{5d0}\u{5d1}\x1b[1;4H".as_bytes());
        let mut f = Frame::default();
        pane(&mut f, &s, RECT, CELL, true, &CursorLook::shown(true));
        let cursor = rgba(s.palette.cursor, 1.0);
        let box_quad = f.quads.iter().find(|q| q.color == cursor).expect("the block cursor");
        assert_eq!(box_quad.rect.x, 40.0);
        // Its text is the aleph, in the cursor text color, at the same cell.
        let at = f.rows.iter().find(|r| r.x == 40.0).unwrap();
        assert_eq!(at.spans[0].text, "\u{5d0}");
        assert_eq!(at.spans[0].color, s.palette.cursor_text);
    }

    /// P9: an explicit override riding on an Arabic cell never reaches the shaper, which would
    /// run its own bidi over the joined piece and honor it.
    #[test]
    fn arabic_runs_drop_explicit_controls() {
        let piece_text = |bytes: &str| {
            let s = screen(bytes.as_bytes());
            let mut f = Frame::default();
            pane(&mut f, &s, RECT, CELL, false, &CursorLook::shown(false));
            let piece = f.rows.iter().find(|r| r.spans.iter().any(|sp| sp.text.contains('\u{633}'))).unwrap();
            piece.spans.iter().map(|sp| sp.text.as_str()).collect::<String>()
        };
        let plain = piece_text("x \u{633}\u{644}\u{627}\u{645}");
        for ctl in ['\u{202a}', '\u{202b}', '\u{202c}', '\u{202d}', '\u{202e}', '\u{2066}', '\u{2067}', '\u{2068}', '\u{2069}'] {
            assert_eq!(piece_text(&format!("x \u{633}{ctl}\u{644}\u{627}\u{645}")), plain, "{ctl:?} must not reach the shaper");
        }
    }

    #[test]
    fn arabic_runs_are_one_piece() {
        // Each letter alone would lose its joining forms (P8): the run is one piece, in
        // logical order, at its visual start.
        let s = screen("x \u{633}\u{644}\u{627}\u{645}".as_bytes());
        let mut f = Frame::default();
        pane(&mut f, &s, RECT, CELL, false, &CursorLook::shown(false));
        let piece = f.rows.iter().find(|r| r.spans.iter().any(|sp| sp.text.contains('\u{633}'))).unwrap();
        let text: String = piece.spans.iter().map(|sp| sp.text.as_str()).collect();
        assert_eq!(text, "\u{633}\u{644}\u{627}\u{645}");
        assert_eq!(piece.x, 20.0);
    }

    /// Seam S2 (13m x 13n): a box line Tandem draws takes the cell's color AFTER minimum
    /// contrast, as text does; a block character is exempt (`contrastIneligible`,
    /// `iTermAttributedStringBuilder.m:755`) and keeps its own color.
    #[test]
    fn box_glyph_follows_minimum_contrast() {
        // Gray box line and gray full block on the same gray.
        let gray = rgb(100, 100, 100);
        let mut s = screen("\x1b[38;2;100;100;100m\x1b[48;2;100;100;100m\u{2500}\u{2588}".as_bytes());
        with_min_contrast(&mut s, 0.5);
        let mut f = Frame::default();
        pane_with(&mut f, &s, RECT, CELL, false, &CursorLook::shown(false), &DrawPrefs::default(), None, crate::render::DEFAULT_FACE);
        let color = |c: char| f.glyphs.iter().find(|g| g.c == c).map(|g| g.color);
        let adjusted = crate::contrast::apply(gray, gray, 0.5);
        assert_ne!(adjusted, gray, "the rule moves gray on gray");
        assert_eq!(color('\u{2500}'), Some(adjusted), "{:?}", f.glyphs);
        assert_eq!(color('\u{2588}'), Some(gray), "a block keeps its color");
    }

    /// Seam S4 (13m x 13n): a box glyph on a right-to-left row draws at its VISUAL column, with
    /// the row's display order, right justification included.
    #[test]
    fn box_glyphs_follow_the_display_order() {
        // ab, space, aleph, box line, bet, gimel: the right-to-left run 3..=6 reverses, so the
        // box line (logical 4) draws at column 5.
        let s = screen("ab \u{5d0}\u{2502}\u{5d1}\u{5d2}".as_bytes());
        let mut f = Frame::default();
        pane(&mut f, &s, RECT, CELL, false, &CursorLook::shown(false));
        assert_eq!(s.visual_col(0, 4), 5);
        assert_eq!(f.glyphs.iter().map(|g| (g.c, g.rect.x)).collect::<Vec<_>>(), vec![('\u{2502}', 50.0)]);
        // A right-to-left paragraph sits at the right edge; its box glyph moves with it.
        let mut s2 = screen("\u{5d0}\u{2502}\u{5d1}".as_bytes());
        s2.bidi_rules.detect_direction = true;
        s2.bidi_rules.right_justify = true;
        let visual = s2.visual_col(0, 1);
        assert!(visual > 2, "right-justified: {visual}");
        let mut f = Frame::default();
        pane(&mut f, &s2, RECT, CELL, false, &CursorLook::shown(false));
        assert_eq!(f.glyphs.iter().map(|g| g.rect.x).collect::<Vec<_>>(), vec![visual as f32 * CELL.0]);
    }

    /// Task 7: a wide Powerline symbol is one glyph two cells wide; the grid keeps one cell.
    #[test]
    fn powerline_glyphs_follow_iterm2() {
        let on = DrawPrefs { powerline: true, powerline_wide: true, powerline_extended: true, ..DrawPrefs::default() };
        let draw = |s: &Screen, prefs: &DrawPrefs| {
            let mut f = Frame::default();
            pane_with(&mut f, s, RECT, CELL, false, &CursorLook::shown(false), prefs, None, crate::render::DEFAULT_FACE);
            f
        };
        let s = screen("\u{E0B8}x".as_bytes());
        let f = draw(&s, &on);
        assert_eq!(f.glyphs.iter().map(|g| (g.c, g.rect.x, g.rect.w)).collect::<Vec<_>>(), vec![('\u{E0B8}', 0.0, 2.0 * CELL.0)], "two cells wide at column 0");
        assert_eq!(f.rows.iter().map(|r| (r.x, r.spans.iter().map(|s| s.text.as_str()).collect::<String>().trim_end().to_string())).collect::<Vec<_>>(), vec![(CELL.0, "x".to_string())], "x stays at column 1");
        // The wide key off: one cell. The profile switch off: no glyph, the font draws it.
        let narrow = draw(&s, &DrawPrefs { powerline_wide: false, ..on.clone() });
        assert_eq!(narrow.glyphs.iter().map(|g| g.rect.w).collect::<Vec<_>>(), vec![CELL.0]);
        assert!(draw(&s, &DrawPrefs { powerline: false, ..on.clone() }).glyphs.is_empty());
        // An extended symbol only with the extended key; a core one always.
        let s2 = screen("\u{E0C0}\u{E0B0}".as_bytes());
        assert_eq!(draw(&s2, &on).glyphs.len(), 2);
        let core_only = draw(&s2, &DrawPrefs { powerline_extended: false, ..on.clone() });
        assert_eq!(core_only.glyphs.iter().map(|g| g.c).collect::<Vec<_>>(), vec!['\u{E0B0}']);
        // The symbol takes the cell's color and ends the text piece around it.
        let s3 = screen("a\u{E0B0}b".as_bytes());
        let f = draw(&s3, &on);
        assert_eq!(f.glyphs.len(), 1);
        assert_eq!(f.rows.len(), 2, "a and b are separate pieces");
    }

    /// Seam S4 (13m x 13n), Task 7: a wide Powerline glyph on a right-to-left row sits at the
    /// visual column of its cell, two cells wide, and the grid width is unchanged.
    #[test]
    fn powerline_wide_in_rtl_row() {
        let on = DrawPrefs { powerline: true, ..DrawPrefs::default() };
        // A right-to-left paragraph of aleph, bet, the symbol and x: the symbol and x are one
        // left-to-right run to the left of the Hebrew, so the symbol (logical 2) is not at column 2.
        let mut s = screen("\u{5d0}\u{5d1}\u{E0B8}x".as_bytes());
        s.bidi_rules.detect_direction = true;
        let mut f = Frame::default();
        pane_with(&mut f, &s, RECT, CELL, false, &CursorLook::shown(false), &on, None, crate::render::DEFAULT_FACE);
        let visual = s.visual_col(0, 2);
        assert_ne!(visual, 2, "the display order moved the symbol");
        assert_eq!(f.glyphs.iter().map(|g| (g.c, g.rect.x, g.rect.w)).collect::<Vec<_>>(), vec![('\u{E0B8}', visual as f32 * CELL.0, 2.0 * CELL.0)]);
        // A right-to-left paragraph sits at the right edge; the glyph moves with it.
        let mut s2 = screen("\u{5d0}\u{E0B8}\u{5d1}".as_bytes());
        s2.bidi_rules.detect_direction = true;
        s2.bidi_rules.right_justify = true;
        let visual = s2.visual_col(0, 1);
        assert!(visual > 2, "right-justified: {visual}");
        let mut f = Frame::default();
        pane_with(&mut f, &s2, RECT, CELL, false, &CursorLook::shown(false), &on, None, crate::render::DEFAULT_FACE);
        assert_eq!(f.glyphs.iter().map(|g| (g.rect.x, g.rect.w)).collect::<Vec<_>>(), vec![(visual as f32 * CELL.0, 2.0 * CELL.0)]);
    }

    /// Seam S7 (13m x 13n): a blinking box line blinks with the text, and a cell under a label
    /// draws no box glyph.
    #[test]
    fn blinking_box_glyph_follows_the_phase() {
        let mut s = screen("\x1b[5m\u{2500}\x1b[0m\u{2502}".as_bytes());
        s.blink_allowed = true;
        let draw = |hl: &Highlights| {
            let mut f = Frame::default();
            pane_with(&mut f, &s, RECT, CELL, false, &CursorLook::shown(false), &DrawPrefs::default(), Some(hl), crate::render::DEFAULT_FACE);
            f.glyphs.iter().map(|g| (g.c, g.rect.x)).collect::<Vec<_>>()
        };
        assert_eq!(draw(&Highlights { text_blink_on: true, ..see_through(1.0, false) }), vec![('\u{2500}', 0.0), ('\u{2502}', 10.0)]);
        assert_eq!(draw(&Highlights { text_blink_on: false, ..see_through(1.0, false) }), vec![('\u{2502}', 10.0)], "the off phase hides the blinking one only");
        let labels = [(crate::find::Pos { row: 0, col: 1 }, "x".to_string())];
        assert_eq!(draw(&Highlights { labels: &labels, ..see_through(1.0, false) }), vec![('\u{2500}', 0.0)], "a label hides the glyph under it");
    }

    #[test]
    fn contrast_applies_per_cell() {
        // Gray on the same gray: unreadable until minimum contrast moves it.
        let mut s = screen(b"\x1b[38;2;100;100;100m\x1b[48;2;100;100;100mab");
        with_min_contrast(&mut s, 0.5);
        let mut f = Frame::default();
        pane(&mut f, &s, RECT, CELL, false, &CursorLook::shown(false));
        let c = span_color(&f, "ab").unwrap();
        assert!(c.r > 200, "dark gray on itself goes toward white: {c:?}");
        assert_eq!(c, crate::contrast::apply(rgb(100, 100, 100), rgb(100, 100, 100), 0.5));
        // On a light background the text goes dark.
        let mut s = screen(b"\x1b[38;2;250;250;250m\x1b[48;2;250;250;250mab");
        with_min_contrast(&mut s, 0.5);
        let mut f = Frame::default();
        pane(&mut f, &s, RECT, CELL, false, &CursorLook::shown(false));
        let c = span_color(&f, "ab").unwrap();
        assert!(c.r < 130 && c.r > 115, "{c:?}");
        // Zero keeps the palette color.
        let mut s = screen(b"\x1b[38;2;128;128;128m\x1b[48;2;128;128;128mab");
        with_min_contrast(&mut s, 0.0);
        let mut f = Frame::default();
        pane(&mut f, &s, RECT, CELL, false, &CursorLook::shown(false));
        assert_eq!(span_color(&f, "ab"), Some(rgb(128, 128, 128)));
        // 13l seam S1: a default-colored underline follows the adjusted text color; an SGR 58
        // color is left alone.
        let mut s = screen(b"\x1b[4;38;2;100;100;100m\x1b[48;2;100;100;100ma\x1b[58;2;0;255;0mb");
        with_min_contrast(&mut s, 0.5);
        let mut f = Frame::default();
        pane(&mut f, &s, RECT, CELL, false, &CursorLook::shown(false));
        let adjusted = crate::contrast::apply(rgb(100, 100, 100), rgb(100, 100, 100), 0.5);
        let ul = |x: f32| f.underlines.iter().find(|q| q.rect.x == x).map(|q| q.color);
        assert_eq!(ul(0.0), Some(rgba(adjusted, 1.0)), "follows the adjusted fg");
        assert_eq!(ul(10.0), Some(rgba(rgb(0, 255, 0), 1.0)), "SGR 58 does not");
    }

    #[test]
    fn selected_cells_contrast_against_the_selection_color() {
        let mut s = screen(b"\x1b[38;2;100;100;100mhello");
        s.palette.selection = rgb(100, 100, 100);
        s.palette.selected_text = None;
        with_min_contrast(&mut s, 0.5);
        let mut sel = Selection::new(SelectionType::Simple, Point::new(Line(0), Column(0)), Side::Left);
        sel.update(Point::new(Line(0), Column(1)), Side::Right);
        s.term_mut().selection = Some(sel);
        let mut f = Frame::default();
        pane(&mut f, &s, RECT, CELL, true, &CursorLook::shown(false));
        let sel_text = span_color(&f, "he").unwrap();
        assert_ne!(sel_text, rgb(100, 100, 100), "selected text is adjusted against the selection color");
        assert_eq!(sel_text, crate::contrast::apply(rgb(100, 100, 100), rgb(100, 100, 100), 0.5));
    }

    /// 13m final 3: a cell both find-matched (or hovered) and selected adjusts its text against
    /// the selection color that draws, not against the match color.
    #[test]
    fn a_matched_selected_cell_contrasts_against_the_selection() {
        let mut s = screen(b"hello");
        s.palette.selection = rgb(0, 0, 0);
        s.palette.selected_text = None;
        with_min_contrast(&mut s, 1.0);
        let mut sel = Selection::new(SelectionType::Simple, Point::new(Line(0), Column(0)), Side::Left);
        sel.update(Point::new(Line(0), Column(0)), Side::Right);
        s.term_mut().selection = Some(sel);
        // A white match color makes the match text black; against the black selection that is
        // unreadable until it is adjusted against the selection.
        let m = [crate::find::Match { start: crate::find::Pos { row: 0, col: 0 }, end: crate::find::Pos { row: 0, col: 0 } }];
        let hl = Highlights { matches: &m, color: rgb(255, 255, 255), ..see_through(1.0, false) };
        let mut f = Frame::default();
        pane_with(&mut f, &s, RECT, CELL, true, &CursorLook::shown(false), &DrawPrefs::default(), Some(&hl), crate::render::DEFAULT_FACE);
        let c = span_color(&f, "h").unwrap();
        assert_ne!(c, rgb(0, 0, 0), "black text on the black selection");
        assert_eq!(c, crate::contrast::apply(rgb(0, 0, 0), rgb(0, 0, 0), 1.0));
        // A hovered link over the selection does the same.
        let hl = Highlights { hover: Some(crate::find::Match { start: crate::find::Pos { row: 0, col: 0 }, end: crate::find::Pos { row: 0, col: 0 } }), link: rgb(0, 0, 0), ..see_through(1.0, false) };
        let mut f = Frame::default();
        pane_with(&mut f, &s, RECT, CELL, true, &CursorLook::shown(false), &DrawPrefs::default(), Some(&hl), crate::render::DEFAULT_FACE);
        assert_eq!(span_color(&f, "h"), Some(crate::contrast::apply(rgb(0, 0, 0), rgb(0, 0, 0), 1.0)));
    }

    /// An explicit Selected Text Color is adjusted against the selection color too.
    #[test]
    fn an_explicit_selected_text_color_is_adjusted() {
        let mut s = screen(b"hello");
        s.palette.selection = rgb(100, 100, 100);
        s.palette.selected_text = Some(rgb(100, 100, 100));
        with_min_contrast(&mut s, 0.5);
        let mut sel = Selection::new(SelectionType::Simple, Point::new(Line(0), Column(0)), Side::Left);
        sel.update(Point::new(Line(0), Column(1)), Side::Right);
        s.term_mut().selection = Some(sel);
        let mut f = Frame::default();
        pane(&mut f, &s, RECT, CELL, true, &CursorLook::shown(false));
        assert_eq!(span_color(&f, "he"), Some(crate::contrast::apply(rgb(100, 100, 100), rgb(100, 100, 100), 0.5)));
    }

    /// A hidden (SGR 8) cell keeps its color whatever the minimum contrast.
    #[test]
    fn hidden_cells_are_exempt_from_contrast() {
        let mut s = screen(b"\x1b[8;38;2;128;128;128m\x1b[48;2;128;128;128mab");
        with_min_contrast(&mut s, 1.0);
        let mut f = Frame::default();
        pane(&mut f, &s, RECT, CELL, false, &CursorLook::shown(false));
        assert!(f.rows.iter().flat_map(|r| &r.spans).all(|sp| sp.color == rgb(128, 128, 128) || sp.text.trim().is_empty()), "{:?}", f.rows);
    }

    #[test]
    fn contrast_runs_before_mute() {
        let mut s = screen(b"\x1b[38;2;128;128;128m\x1b[48;2;128;128;128mab");
        s.palette.bg = rgb(0, 0, 0);
        with_min_contrast(&mut s, 0.5);
        let mut f = Frame::default();
        pane(&mut f, &s, RECT, CELL, false, &CursorLook { boost: 0.5, ..CursorLook::shown(false) });
        let raw = crate::contrast::apply(rgb(128, 128, 128), rgb(128, 128, 128), 0.5);
        let want = crate::cursor_look::mute(raw, s.palette.bg, 0.5);
        assert_eq!(span_color(&f, "ab"), Some(want));
        let wrong = crate::contrast::apply(crate::cursor_look::mute(rgb(128, 128, 128), s.palette.bg, 0.5), crate::cursor_look::mute(rgb(128, 128, 128), s.palette.bg, 0.5), 0.5);
        assert_ne!(want, wrong);
    }

    #[test]
    fn cursor_text_is_not_adjusted() {
        let mut s = screen(b"ab\x1b[1;1H");
        s.palette.cursor = rgb(200, 200, 200);
        s.palette.cursor_text = rgb(200, 200, 200);
        with_min_contrast(&mut s, 1.0);
        let mut f = Frame::default();
        pane(&mut f, &s, RECT, CELL, true, &CursorLook::shown(true));
        assert_eq!(span_color(&f, "a"), Some(rgb(200, 200, 200)));
    }

    #[test]
    fn blocks_keep_their_color() {
        let mut s = screen(b"\x1b[38;2;128;128;128m\x1b[48;2;128;128;128m\xe2\x96\x88\xe2\x94\x80");
        with_min_contrast(&mut s, 1.0);
        let mut f = Frame::default();
        pane(&mut f, &s, RECT, CELL, false, &CursorLook::shown(false));
        // Both are drawn by Tandem (13n), so their colors are the glyphs'.
        let block = f.glyphs.iter().find(|g| g.c == '\u{2588}').unwrap();
        assert_eq!(block.color, rgb(128, 128, 128), "a block character keeps its foreground");
        let line = f.glyphs.iter().find(|g| g.c == '\u{2500}').unwrap();
        assert_ne!(line.color, rgb(128, 128, 128), "a box line is adjusted");
    }

    #[test]
    fn boost_mutes_all_but_the_cursor() {
        // White on black text, a red cell, Boost 0.5: everything but the cursor is halfway to black.
        let mut s = screen(b"\x1b[38;2;255;255;255m\x1b[48;2;0;0;0m\x1b[2J\x1b[H\x1b[48;2;200;0;0mc\x1b[48;2;0;0;0mab\x1b[1;3H");
        s.palette.bg = rgb(0, 0, 0);
        let mut f = Frame::default();
        pane(&mut f, &s, RECT, CELL, true, &CursorLook { boost: 0.5, ..CursorLook::shown(true) });
        let a = f.rows[0].spans.iter().find(|sp| sp.text.contains('a')).unwrap();
        assert_eq!(a.color, rgb(128, 128, 128));
        let b = f.rows[0].spans.iter().find(|sp| sp.text == "b").unwrap();
        assert_eq!(b.color, s.palette.cursor_text, "the cursor's text keeps its color");
        assert!(f.quads.iter().any(|q| q.color == rgba(rgb(100, 0, 0), 1.0) && q.rect.x == 0.0), "{:?}", f.quads);
        assert!(f.quads.iter().any(|q| q.color == rgba(s.palette.cursor, 1.0) && q.rect.x == 20.0), "the cursor is not muted");
        // Boost 0 leaves the text alone.
        let mut f = Frame::default();
        pane(&mut f, &s, RECT, CELL, true, &CursorLook::shown(true));
        let a = f.rows[0].spans.iter().find(|sp| sp.text.contains('a')).unwrap();
        assert_eq!(a.color, rgb(255, 255, 255));
    }

    #[test]
    fn boost_mutes_the_selection_and_find_matches() {
        // iTerm2 runs the selection and match backgrounds and the text through the muting too
        // (`iTermMetalPerFrameState.m:2191`, `iTermColorMap.m:327-350`, `:514-525`).
        let mut s = screen(b"hello ab");
        s.palette.bg = rgb(0, 0, 0);
        s.palette.selection = rgb(200, 100, 0);
        s.palette.selected_text = Some(rgb(100, 200, 100));
        let mut sel = Selection::new(SelectionType::Simple, Point::new(Line(0), Column(0)), Side::Left);
        sel.update(Point::new(Line(0), Column(1)), Side::Right);
        s.term_mut().selection = Some(sel);
        let m = crate::find::find_all(&s, "ab").unwrap();
        let mut f = Frame::default();
        let yellow = rgb(200, 200, 0);
        let hl = Highlights { matches: &m, color: yellow, hover: None, link: yellow, labels: &[], notes: &[], dim: crate::dim::Dim::LEGACY, key_cursor: false, bg_alpha: 1.0, opaque_colors: false, text_blink_on: true };
        pane_with(&mut f, &s, RECT, CELL, true, &CursorLook { boost: 0.5, ..CursorLook::shown(false) }, &DrawPrefs::default(), Some(&hl), crate::render::DEFAULT_FACE);
        let quad = |c: Rgb, x: f32| f.quads.iter().any(|q| q.color == rgba(c, 1.0) && q.rect.x == x);
        assert!(quad(rgb(100, 50, 0), 0.0), "selection background halfway to black: {:?}", f.quads);
        assert!(quad(rgb(100, 100, 0), 60.0), "match background halfway to black: {:?}", f.quads);
        let text = |t: &str| f.rows[0].spans.iter().find(|sp| sp.text.contains(t)).map(|sp| sp.color);
        assert_eq!(text("he"), Some(rgb(50, 100, 50)), "selected text is muted");
        // The match's own text color (black on this light yellow) is muted toward black too.
        assert_eq!(text("ab"), Some(crate::cursor_look::mute(match_text(yellow), s.palette.bg, 0.5)));
    }

    #[test]
    fn selected_text_without_a_color_is_muted_once() {
        let mut s = screen(b"hello");
        s.palette.bg = rgb(0, 0, 0);
        s.palette.fg = rgb(200, 100, 0);
        s.palette.selected_text = None;
        let mut sel = Selection::new(SelectionType::Simple, Point::new(Line(0), Column(0)), Side::Left);
        sel.update(Point::new(Line(0), Column(1)), Side::Right);
        s.term_mut().selection = Some(sel);
        let mut f = Frame::default();
        pane(&mut f, &s, RECT, CELL, true, &CursorLook { boost: 0.5, ..CursorLook::shown(false) });
        let text = f.rows[0].spans.iter().find(|sp| sp.text.contains("he")).map(|sp| sp.color);
        assert_eq!(text, Some(rgb(100, 50, 0)), "halfway to black once, not twice");
    }

    #[test]
    fn a_sliding_cursor_is_drawn_at_its_offset() {
        // The real cursor, its shadow and its outline all move with the slide (`CSA:130-155`).
        for (shape, code) in [(CursorShape::Underline, "4"), (CursorShape::Beam, "6")] {
            let s = screen(format!("ab\x1b[{code} q\x1b[1;2H").as_bytes());
            let rest = CursorLook { shadow: true, outline: true, ..CursorLook::shown(true) };
            let slid = CursorLook { offset: (-6.0, 1.5), ..rest.clone() };
            let solid = crate::cursor_look::cursor_rect(&rest.geometry, shape, (0.0, 0.0), CELL, 0, 1, false);
            let (mut a, mut b) = (Frame::default(), Frame::default());
            pane(&mut a, &s, RECT, CELL, true, &rest);
            pane(&mut b, &s, RECT, CELL, true, &slid);
            let color = rgba(s.palette.cursor, 1.0);
            let at = |f: &Frame| f.overlay.iter().find(|q| q.color == color).map(|q| q.rect);
            assert_eq!(at(&a), Some(solid));
            assert_eq!(at(&b), Some(Rect { x: solid.x - 6.0, y: solid.y + 1.5, ..solid }));
            assert_eq!(a.overlay.len(), b.overlay.len());
            for (qa, qb) in a.overlay.iter().zip(&b.overlay) {
                assert_eq!((qb.rect.x, qb.rect.y), (qa.rect.x - 6.0, qa.rect.y + 1.5), "{:?} vs {:?}", qa, qb);
            }
        }
    }

    #[test]
    fn a_smear_is_drawn_in_the_cursor_color() {
        let s = screen(b"ab");
        let hex = [(20.0, 20.0), (60.0, 20.0), (60.0, 20.0), (60.0, 40.0), (20.0, 40.0), (20.0, 40.0)];
        let look = CursorLook { smear: Some(hex), ..CursorLook::shown(true) };
        let mut f = Frame::default();
        pane(&mut f, &s, RECT, CELL, true, &look);
        let color = rgba(s.palette.cursor, 1.0);
        let area: f32 = f.overlay.iter().filter(|q| q.color == color && q.rect.w > 20.0).map(|q| q.rect.w * q.rect.h).sum();
        assert_eq!(area, 800.0, "the 40 by 20 hexagon is filled");
        let mut none = Frame::default();
        pane(&mut none, &s, RECT, CELL, true, &CursorLook::shown(true));
        assert_eq!(f.overlay.len() > none.overlay.len(), true);
    }

    #[test]
    fn polygon_fill_covers_the_shape() {
        let area = |q: &[Quad]| q.iter().map(|q| q.rect.w * q.rect.h).sum::<f32>();
        let mut q = Vec::new();
        fill_polygon(&mut q, &[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)], [1.0; 4]);
        assert_eq!(area(&q), 100.0);
        assert!(q.iter().all(|q| q.rect.x >= 0.0 && q.rect.y >= 0.0 && q.rect.x + q.rect.w <= 10.0 && q.rect.y + q.rect.h <= 10.0 && q.color == [1.0; 4]));
        let mut t = Vec::new();
        fill_polygon(&mut t, &[(0.0, 0.0), (10.0, 10.0), (0.0, 10.0)], [1.0; 4]);
        assert!((area(&t) - 50.0).abs() <= 5.0, "{}", area(&t));
        let mut e = Vec::new();
        fill_polygon(&mut e, &[], [1.0; 4]);
        fill_polygon(&mut e, &[(0.0, 0.0), (10.0, 10.0)], [1.0; 4]);
        assert!(e.is_empty());
    }

    #[test]
    fn shadow_and_outline_are_drawn_after_the_cursor() {
        let s = screen(b"ab\x1b[4 q\x1b[1;2H");
        let look = CursorLook { shadow: true, outline: true, ..CursorLook::shown(true) };
        let mut f = Frame::default();
        pane(&mut f, &s, RECT, CELL, true, &look);
        let solid = crate::cursor_look::cursor_rect(&look.geometry, CursorShape::Underline, (0.0, 0.0), CELL, 0, 1, false);
        let pos = |c: [f32; 4]| f.overlay.iter().position(|q| q.color == c);
        let cursor = pos(rgba(s.palette.cursor, 1.0)).expect("the underline");
        let shadow_color = if crate::cursor_look::brightness(s.palette.cursor) < 0.5 { [1.0, 1.0, 1.0, 0.5] } else { [0.0, 0.0, 0.0, 0.5] };
        let shadow = pos(shadow_color).expect("the shadow");
        assert!(shadow > cursor);
        assert_eq!(f.overlay[shadow].rect, Rect { x: solid.x, y: solid.y - 1.0, w: solid.w, h: 1.0 });
        let outline: Vec<_> = f.overlay.iter().skip(shadow + 1).filter(|q| q.color == rgba(s.palette.bg, 0.75)).collect();
        assert_eq!(outline.len(), 4, "{:?}", f.overlay);
        // The shadow fades with the smooth blink.
        let mut f = Frame::default();
        pane(&mut f, &s, RECT, CELL, true, &CursorLook { alpha: 0.5, ..look.clone() });
        assert!(f.overlay.iter().any(|q| q.color == [shadow_color[0], shadow_color[1], shadow_color[2], 0.25]), "{:?}", f.overlay);
        // A focused block has neither, and neither has an unfocused hollow box.
        for focus in [true, false] {
            let s = screen(b"ab\x1b[2 q\x1b[1;2H");
            let mut f = Frame::default();
            pane(&mut f, &s, RECT, CELL, focus, &look);
            assert!(!f.overlay.iter().any(|q| q.color == rgba(s.palette.bg, 0.75) || q.color[3] == 0.5), "{:?}", f.overlay);
        }
    }

    #[test]
    fn block_cursor_leaves_the_cell_background_alone() {
        let s = screen(b"\x1b[41ma\x1b[1D");
        let mut f = Frame::default();
        pane(&mut f, &s, RECT, CELL, true, &CursorLook::shown(true));
        let red = rgba(s.palette.ansi[1], 1.0);
        let at = |c| f.quads.iter().position(|q| q.color == c && q.rect.contains(5.0, 10.0));
        assert!(at(red) < at(rgba(s.palette.cursor, 1.0)), "the red run first, the cursor over it: {:?}", f.quads.iter().take(3).collect::<Vec<_>>());
    }

    #[test]
    fn underline_cursor_is_two_points_high() {
        let s = screen(b"ab\x1b[4 q");
        let mut f = Frame::default();
        pane(&mut f, &s, RECT, CELL, true, &CursorLook::shown(true));
        let u = f.overlay.iter().find(|q| q.color == rgba(s.palette.cursor, 1.0)).unwrap();
        assert_eq!(u.rect, Rect { x: 20.0, y: 18.0, w: 10.0, h: 2.0 });
    }

    #[test]
    fn a_hidden_cursor_has_nothing_to_blink() {
        let blinking = |bytes: &[u8]| {
            let mut s = Screen::with_look(80, 24, crate::palette::Palette::default(), crate::screen::Look { cursor_blink: true, ..Default::default() });
            s.feed(bytes);
            s
        };
        assert!(cursor_blinks_shown(&blinking(b"ab")), "a blinking cursor, shown");
        assert!(!cursor_blinks_shown(&blinking(b"ab\x1b[?25l")), "hidden by the program");
        let mut s = blinking(&b"x\r\n".repeat(60));
        assert!(cursor_blinks_shown(&s));
        s.term_mut().scroll_display(alacritty_terminal::grid::Scroll::Delta(10));
        assert!(!cursor_blinks_shown(&s), "scrolled off the view");
    }

    #[test]
    fn fill_polygon_ignores_what_cannot_be_drawn() {
        assert_eq!(polygon_band(&[(0.0, 0.0), (10.0, f32::NAN), (5.0, 10.0)]), None);
        assert_eq!(polygon_band(&[(0.0, f32::INFINITY), (10.0, 0.0), (5.0, 10.0)]), None);
        assert_eq!(polygon_band(&[(f32::NAN, 0.0), (10.0, 0.0), (5.0, 10.0)]), None);
        assert_eq!(polygon_band(&[(0.0, 0.0), (10.0, 1.0e9), (5.0, 10.0)]), None);
        assert_eq!(polygon_band(&[(0.0, 0.5), (10.0, 0.0), (5.0, 10.0)]), Some((0.0, 10.0)));
        // A sane triangle still fills.
        let mut q = Vec::new();
        fill_polygon(&mut q, &[(0.0, 0.0), (10.0, 0.0), (5.0, 10.0)], [1.0; 4]);
        assert!(!q.is_empty());
    }

    #[test]
    fn cursor_cell_reports_wide() {
        assert_eq!(cursor_cell(&screen(b"ab")), Some((0, 2, false)));
        assert_eq!(cursor_cell(&screen("\u{65e5}\x1b[1G".as_bytes())), Some((0, 0, true)));
        assert_eq!(cursor_cell(&screen(b"\x1b[?25l")), None);
    }

    #[test]
    fn beam_cursor_is_one_point_wide() {
        let s = screen(b"ab\x1b[6 q");
        let mut f = Frame::default();
        pane(&mut f, &s, RECT, CELL, true, &CursorLook::shown(true));
        let beam = f.overlay.iter().find(|q| q.color == rgba(s.palette.cursor, 1.0)).unwrap();
        assert_eq!(beam.rect, Rect { x: 20.0, y: 0.0, w: 1.0, h: 20.0 });
    }

    #[test]
    fn default_background_is_not_a_cell_quad() {
        let s = screen(b"abc");
        let mut f = Frame::default();
        pane_with(&mut f, &s, RECT, CELL, false, &CursorLook::shown(false), &DrawPrefs::default(), Some(&see_through(1.0, false)), crate::render::DEFAULT_FACE);
        assert!(f.quads.is_empty(), "{:?}", f.quads);
    }

    /// B1: `margin_fill` is the only margin painter, one layer per pixel: a 0.7 pane's margins
    /// compose to 0.7, never to 0.91.
    #[test]
    fn margin_fill_one_layer_in_a_see_through_pane() {
        let outer = Rect { x: 0.0, y: 0.0, w: 820.0, h: 500.0 };
        let inner = Rect { x: 10.0, y: 10.0, w: 800.0, h: 480.0 };
        let (bg, red) = (rgb(0, 0, 0), rgb(255, 0, 0));
        let mut fill = crate::margins::fill(&vec![(None, None); 24], bg, true, true);
        fill.rows[2] = (red, bg);
        let mut f = Frame::default();
        margin_fill_alpha(&mut f, outer, inner, 20.0, &fill, bg, 0.7, false, false);
        let a = |x: f32, y: f32| crate::transparency::composite(&f.quads, x, y)[3];
        for (x, y) in [(3.0, 3.0), (815.0, 495.0), (3.0, 100.0), (815.0, 100.0), (3.0, 55.0), (409.0, 3.0)] {
            assert!((a(x, y) - 0.7).abs() < 1e-4, "({x},{y}) {}", a(x, y));
        }
        assert!(a(400.0, 200.0) == 0.0, "the grid is not a margin");
        let area: f32 = f.quads.iter().map(|q| q.rect.w * q.rect.h).sum();
        assert_eq!(area, 820.0 * 500.0 - 800.0 * 480.0, "no pixel is covered twice");
        // A non-default side color is opaque only with Only The Default BG Color Uses Transparency.
        let mut g = Frame::default();
        margin_fill_alpha(&mut g, outer, inner, 20.0, &fill, bg, 0.7, true, false);
        assert!((crate::transparency::composite(&g.quads, 3.0, 55.0)[3] - 1.0).abs() < 1e-4);
        assert!((crate::transparency::composite(&g.quads, 815.0, 55.0)[3] - 0.7).abs() < 1e-4);
    }

    /// In an opaque window a strip the clear color already shows is not drawn; other colors are.
    #[test]
    fn margin_fill_opaque_window_skips_clear_colored_strips() {
        let outer = Rect { x: 0.0, y: 0.0, w: 820.0, h: 500.0 };
        let inner = Rect { x: 10.0, y: 10.0, w: 800.0, h: 480.0 };
        let (bg, red) = (rgb(10, 20, 30), rgb(255, 0, 0));
        let mut f = Frame { clear: rgba(bg, 1.0), ..Default::default() };
        margin_fill(&mut f, outer, inner, 20.0, &crate::margins::fill(&vec![(None, None); 24], bg, true, true), None);
        assert!(f.quads.is_empty(), "{:?}", f.quads);
        let mut fill = crate::margins::fill(&vec![(None, None); 24], bg, true, true);
        fill.rows[1] = (red, bg);
        fill.rows[2] = (red, bg);
        margin_fill(&mut f, outer, inner, 20.0, &fill, None);
        assert_eq!(f.quads, vec![Quad { rect: Rect { x: 0.0, y: 30.0, w: 10.0, h: 40.0 }, color: rgba(red, 1.0) }], "two red rows join into one strip");
        // Another pane's color over the window's clear color is drawn everywhere.
        let mut g = Frame { clear: rgba(red, 1.0), ..Default::default() };
        margin_fill(&mut g, outer, inner, 20.0, &crate::margins::fill(&vec![(None, None); 24], bg, true, true), None);
        let area: f32 = g.quads.iter().map(|q| q.rect.w * q.rect.h).sum();
        assert_eq!(area, 820.0 * 500.0 - 800.0 * 480.0);
    }

    /// A pane with a background image: its veil already shows the pane's color in the margins,
    /// so an opaque strip of that color would hide the image there. Other colors still draw.
    #[test]
    fn margin_fill_keeps_a_background_image_visible() {
        let outer = Rect { x: 0.0, y: 0.0, w: 820.0, h: 500.0 };
        let inner = Rect { x: 10.0, y: 10.0, w: 800.0, h: 480.0 };
        let (bg, red) = (rgb(10, 20, 30), rgb(255, 0, 0));
        let mut f = Frame { clear: rgba(red, 1.0), ..Default::default() };
        let mut fill = crate::margins::fill(&vec![(None, None); 24], bg, true, true);
        margin_fill(&mut f, outer, inner, 20.0, &fill, Some(bg));
        assert!(f.quads.is_empty(), "the veil shows the image: {:?}", f.quads);
        fill.rows[1] = (red, bg);
        margin_fill(&mut f, outer, inner, 20.0, &fill, Some(bg));
        assert_eq!(f.quads.len(), 0, "red equals the clear color here, so nothing either");
        let mut g = Frame { clear: rgba(rgb(1, 1, 1), 1.0), ..Default::default() };
        margin_fill(&mut g, outer, inner, 20.0, &fill, Some(bg));
        assert_eq!(g.quads, vec![Quad { rect: Rect { x: 0.0, y: 30.0, w: 10.0, h: 20.0 }, color: rgba(red, 1.0) }], "a row's own edge color still draws over the image");
        let mut h = Frame { clear: rgba(rgb(1, 1, 1), 1.0), ..Default::default() };
        margin_fill(&mut h, outer, inner, 20.0, &fill, None);
        assert!(h.quads.len() > 1, "without an image the pane color is drawn");
        let mut a = Frame::default();
        margin_fill_alpha(&mut a, outer, inner, 20.0, &fill, bg, 0.7, false, true);
        assert_eq!(a.quads.len(), 1, "see-through: the pane-colored strips are skipped too");
    }

    #[test]
    fn composited_pixel_alpha_follows_iterm2() {
        let s = red_selected_default();
        let pane = Rect { x: 0.0, y: 0.0, w: 820.0, h: 500.0 };
        let grid = Rect { x: 10.0, y: 10.0, w: 800.0, h: 480.0 };
        let bg = s.palette.bg;
        let build = |opaque: bool| {
            let mut f = Frame::default();
            margin_fill_alpha(&mut f, pane, grid, CELL.1, &crate::margins::fill(&[], bg, false, false), bg, 0.7, false, false);
            pane_with(&mut f, &s, grid, CELL, false, &CursorLook::shown(false), &DrawPrefs::default(), Some(&see_through(0.7, opaque)), crate::render::DEFAULT_FACE);
            f
        };
        let a = |f: &Frame, x: f32, y: f32| crate::transparency::composite(&f.quads, x, y)[3];
        let f = build(false);
        for (x, y) in [(15.0, 20.0), (25.0, 20.0), (35.0, 20.0), (3.0, 3.0), (815.0, 495.0)] {
            assert!((a(&f, x, y) - 0.7).abs() < 1e-4, "({x},{y}) {}", a(&f, x, y));
        }
        let f = build(true);
        assert!((a(&f, 15.0, 20.0) - 1.0).abs() < 1e-4);
        assert!((a(&f, 25.0, 20.0) - 1.0).abs() < 1e-4);
        assert!((a(&f, 35.0, 20.0) - 0.7).abs() < 1e-4);
        assert!((a(&f, 3.0, 3.0) - 0.7).abs() < 1e-4);
    }

    #[test]
    fn selection_uses_selection_colors() {
        let mut s = screen(b"hello");
        let mut sel = Selection::new(SelectionType::Simple, Point::new(Line(0), Column(0)), Side::Left);
        sel.update(Point::new(Line(0), Column(1)), Side::Right);
        s.term_mut().selection = Some(sel);
        let mut f = Frame::default();
        pane(&mut f, &s, RECT, CELL, true, &CursorLook::shown(true));
        let sel = rgba(s.palette.selection, 1.0);
        assert!(f.quads.iter().any(|q| q.color == sel && q.rect == Rect { x: 0.0, y: 0.0, w: 20.0, h: 20.0 }), "{:?}", f.quads);
    }

    #[test]
    fn wide_char_spacer_is_skipped() {
        let s = screen("\u{4e2d}x".as_bytes());
        let mut f = Frame::default();
        pane(&mut f, &s, RECT, CELL, true, &CursorLook::shown(true));
        let all: String = f.rows.iter().flat_map(|r| r.spans.iter().map(|s| s.text.as_str())).collect();
        assert!(all.starts_with("\u{4e2d}x"), "{all:?}");
    }

    #[test]
    fn non_ascii_text_is_placed_on_its_own_cell() {
        // Fallback fonts are not monospace, so a wide or non-ASCII glyph gets its own
        // row piece at its cell's x, and the text after it starts at its own cell too.
        let s = screen("a\u{4e2d}x".as_bytes());
        let mut f = Frame::default();
        pane(&mut f, &s, RECT, CELL, true, &CursorLook::shown(true));
        let at = |text: &str| f.rows.iter().find(|r| r.spans.iter().any(|s| s.text.trim_end() == text)).map(|r| r.x);
        assert_eq!(at("\u{4e2d}"), Some(10.0), "{:?}", f.rows);
        assert_eq!(at("x"), Some(30.0), "{:?}", f.rows);
    }

    #[test]
    fn the_cursor_blinks_off() {
        let s = screen(b"ab");
        let mut f = Frame::default();
        pane(&mut f, &s, RECT, CELL, true, &CursorLook::shown(false));
        assert!(!f.quads.iter().any(|q| q.color == rgba(s.palette.cursor, 1.0)));
    }

    #[test]
    fn non_ascii_pieces_use_the_alternate_font() {
        let s = screen("a\u{4e2d}".as_bytes());
        let mut f = Frame::default();
        pane(&mut f, &s, RECT, CELL, true, &CursorLook::shown(true));
        let wide = f.rows.iter().flat_map(|r| &r.spans).find(|sp| sp.text == "\u{4e2d}").unwrap();
        assert!(wide.alt_font);
        let a = f.rows.iter().flat_map(|r| &r.spans).find(|sp| sp.text == "a").unwrap();
        assert!(!a.alt_font);
    }

    #[test]
    fn block_rows_get_a_gutter_bar() {
        let inner = Rect { x: 10.0, y: 0.0, w: 100.0, h: 64.0 };
        let (fg, red) = (rgb(200, 200, 200), rgb(255, 0, 0));
        let mut f = Frame::default();
        block_bars(&mut f, &[(1, false, false)], inner, 16.0, 6.0, fg, red);
        assert_eq!(f.overlay, vec![Quad { rect: Rect { x: 5.0, y: 16.0, w: 4.0, h: 16.0 }, color: rgba(fg, 1.0) }], "filled, in the padding");
        let mut f = Frame::default();
        block_bars(&mut f, &[(2, true, false)], inner, 16.0, 6.0, fg, red);
        assert_eq!(f.overlay.len(), 4, "an outline when folded");
        let mut f = Frame::default();
        block_bars(&mut f, &[(0, false, true)], inner, 16.0, 6.0, fg, red);
        assert_eq!(f.overlay[0].color, rgba(red, 1.0), "red when hovered");
    }

    #[test]
    fn badge_behind_text() {
        let (fg, bg) = (rgb(255, 0, 0), rgb(0, 0, 0));
        let mut f = Frame::default();
        let l = crate::badge::BadgeLayout { size: 30.0, lines: vec!["hi".into()], widths: vec![40.0], rect: Rect { x: 50.0, y: 10.0, w: 40.0, h: 36.0 } };
        badge(&mut f, Rect { x: 0.0, y: 0.0, w: 200.0, h: 100.0 }, &l, fg, 0.5, bg, 15.0);
        assert_eq!(f.rows.len(), 5, "four outline copies and the fill");
        assert!(f.rows[..4].iter().all(|r| r.spans[0].color == bg), "the outline in the background color first");
        assert_eq!(f.rows[4].spans[0].color, rgb(128, 0, 0), "the fill at the badge color's alpha over the background");
        assert!(f.rows.iter().all(|r| r.spans[0].badge && r.scale == 2.0));
    }

    #[test]
    fn porthole_chrome_and_clip() {
        use crate::porthole::render::{Face, Run, VisualLine};
        let (fg, bg) = (rgb(200, 200, 200), rgb(0, 0, 0));
        let run = |t: &str, face| Run { text: t.into(), color: fg, bold: false, italic: false, underline: t == "link", face, link: None };
        let line = |y: f32, runs: Vec<Run>, scale: f32| VisualLine { spans: runs.iter().enumerate().map(|(i, _)| (i as f32 * 40.0, 40.0)).collect(), runs, x: 10.0, y, h: 20.0 * scale, scale, code_bg: false, rule: false, block: 0 };
        let lines = vec![line(0.0, vec![run("Title", Face::Prose)], 2.0), line(40.0, vec![run("code", Face::Code), run("link", Face::Prose)], 1.0)];
        let rect = Rect { x: 0.0, y: -40.0, w: 200.0, h: 200.0 };
        let clip = Rect { x: 0.0, y: 0.0, w: 200.0, h: 100.0 };
        let mut f = Frame::default();
        let chrome = PortholeChrome { fg, bg, language: "Markdown", wide: false, scroll_x: 0.0 };
        let c = porthole(&mut f, rect, clip, &lines, &[], &chrome, (10.0, 20.0));
        assert_eq!(f.quads[0].rect, clip, "the background, cut to the pane");
        assert_eq!(f.quads[0].color, rgba(rgb(26, 26, 26), 1.0), "20% toward grey");
        assert!(f.quads.iter().all(|q| q.rect.y >= clip.y && q.rect.y + q.rect.h <= clip.y + clip.h), "nothing outside the pane");
        let title = f.rows.iter().find(|r| r.spans.iter().any(|s| s.text == "Title")).expect("the title");
        assert!(title.spans[0].prose && title.scale == 2.0);
        assert_eq!(title.x, 8.0 + 10.0, "margin plus the line's indent");
        assert!(title.clip.y >= clip.y && title.clip.y + title.clip.h <= clip.y + clip.h);
        let code = f.rows.iter().find(|r| r.spans.iter().any(|s| s.text == "code")).unwrap();
        assert!(!code.spans[0].prose);
        assert!(f.quads.iter().any(|q| q.rect.h == 1.0 && q.rect.w == 40.0), "the link is underlined");
        assert!(c.close.x + c.close.w <= rect.x + rect.w - 2.0 && c.language.x + c.language.w <= c.close.x && c.wide.x + c.wide.w <= c.language.x, "close, then the language, then Wide, right to left");
    }

    #[test]
    fn inline_buttons_draw_faint_when_inert() {
        let (bg, fg) = (rgb(0, 0, 0), rgb(200, 100, 0));
        let r = Rect { x: 20.0, y: 16.0, w: 16.0, h: 16.0 };
        let mut f = Frame::default();
        inline_buttons(&mut f, &[(r, ButtonFace::Icon('\u{E768}'), true)], bg, fg);
        assert_eq!(f.rows.len(), 1, "the icon is text");
        assert!(f.rows[0].spans[0].icon, "in the icon font");
        assert_eq!(f.rows[0].spans[0].color, fg);
        let mut f = Frame::default();
        inline_buttons(&mut f, &[(r, ButtonFace::Icon('\u{E768}'), false)], bg, fg);
        assert_eq!(f.rows[0].spans[0].color, rgb(80, 40, 0), "40% toward the background");
        let mut f = Frame::default();
        inline_buttons(&mut f, &[(r, ButtonFace::Copy, true)], bg, fg);
        assert!(f.quads.len() > 1 && f.rows.is_empty(), "copy is drawn with quads");
        let mut f = Frame::default();
        inline_buttons(&mut f, &[(r, ButtonFace::Blank, true)], bg, fg);
        assert!(f.quads.is_empty() && f.rows.is_empty(), "an unknown icon draws nothing");
    }

    #[test]
    fn background_image_under_cells() {
        let img = crate::bgimage::Loaded { path: "x.png".into(), key: 7, size: (2, 2), rgba: std::sync::Arc::new(vec![255; 16]) };
        let pane = Rect { x: 0.0, y: 0.0, w: 80.0, h: 32.0 };
        let pieces = crate::bgimage::quads(crate::bgimage::Mode::Stretch, img.size, pane, pane);
        let s = screen(b"a\x1b[41mb\x1b[0m");
        let mut f = Frame::default();
        background(&mut f, pane, &pieces, &img, s.palette.bg, 0.25);
        pane_with(&mut f, &s, pane, CELL, false, &CursorLook::shown(false), &DrawPrefs::default(), None, crate::render::DEFAULT_FACE);
        assert_eq!(f.backgrounds.len(), 1);
        assert_eq!((f.backgrounds[0].key, f.backgrounds[0].repeat), ((7, 0), false));
        assert_eq!(f.quads[0], Quad { rect: pane, color: rgba(s.palette.bg, 0.75) }, "the default background veils the image");
        let red = s.palette.ansi[1];
        assert!(f.quads.iter().any(|q| q.color == rgba(red, 1.0)), "a colored cell covers the image");
        assert_eq!(f.quads.len(), 2, "the default cell draws no background");
    }

    #[test]
    fn vertical_tab_bar_draws_rows() {
        use crate::tab_layout::{self, TabInput};
        let s = tab_layout::Settings::from_globals(&crate::globals::Globals::empty(std::path::Path::new("unused.toml")));
        let looks = vec![
            TabLook { title: "one".into(), number: 1, ..TabLook::default() },
            TabLook { title: "two".into(), number: 2, ..TabLook::default() },
            TabLook { title: "a title far too long for the side bar".into(), number: 3, ..TabLook::default() },
        ];
        let inputs: Vec<TabInput> = looks.iter().map(|l| TabInput { title: l.title.clone(), subtitle: String::new(), icon: false, processing: false, number: l.number, pinned: false, group: None, hidden: false, graphic: false, letter: String::new() }).collect();
        let six = |t: &str, _pt: f32, _bold: bool| t.chars().count() as f32 * 6.0;
        let area = Rect { x: 0.0, y: 0.0, w: 150.0, h: 100.0 };
        let bar = tab_layout::build_vertical(&inputs, 150.0, 100.0, 0.0, None, &s, &six);
        let mut f = Frame::default();
        let (rects, chips) = tab_bar_vertical(&mut f, &looks, 1, &bar, area, 1.0, 0.1, 20.0, true, false, &legacy());
        let row = |y: f32| Rect { x: 0.0, y, w: 150.0, h: 24.0 };
        assert_eq!(rects, vec![row(0.0), row(24.0), row(48.0)]);
        assert!(chips.is_empty());
        assert!(f.quads.iter().any(|q| q.rect == area && q.color == rgba(TAB_BAR, 1.0)), "the bar's background");
        assert!(f.quads.iter().any(|q| q.rect == rects[1] && q.color == rgba(TAB_ACTIVE, 1.0)), "the active row's fill");
        assert!(!f.quads.iter().any(|q| q.rect == rects[0] && q.color == rgba(TAB_ACTIVE, 1.0)));
        // The long title stays on one line, cut to its label with an ellipsis.
        let long: Vec<&TextRow> = f.rows.iter().filter(|r| r.y >= 48.0 && r.spans.iter().any(|s| s.text.contains('\u{2026}'))).collect();
        assert_eq!(long.len(), 1, "one line");
        let text: String = long[0].spans.iter().map(|s| s.text.as_str()).collect();
        assert!(text.starts_with("a title"), "{text}");
        let short = f.rows.iter().find(|r| r.spans.iter().any(|s| s.text == "two")).expect("a title that fits");
        assert!(rects[1].contains(short.x, short.y));
        // A row is a tab: its number, in the prose font at the tab label size.
        let number = &bar.texts[1].counter;
        assert!(!number.is_empty());
        let n = f.rows.iter().find(|r| r.spans.iter().any(|s| &s.text == number)).expect("the number");
        assert!(rects[1].contains(n.x, n.y + 1.0) && n.scale == s.font * 0.1);

        // Scrolled 30: the first row is wholly above the bar (a zero-size rect); the second
        // shows its bottom 18 pixels, its text cut to the bar.
        let bar = tab_layout::build_vertical(&inputs, 150.0, 100.0, 30.0, None, &s, &six);
        let mut f = Frame::default();
        let (rects, _) = tab_bar_vertical(&mut f, &looks, 1, &bar, area, 1.0, 0.1, 20.0, true, false, &legacy());
        assert_eq!(rects[0], Rect { x: 0.0, y: 0.0, w: 0.0, h: 0.0 });
        assert_eq!(rects[1], row(-6.0));
        assert_eq!(rects[2], row(18.0));
        let two = f.rows.iter().find(|r| r.spans.iter().any(|s| s.text == "two")).expect("the second title");
        assert_eq!(two.clip, Rect { x: 0.0, y: 0.0, w: 150.0, h: 18.0 });
        assert!(f.quads.iter().all(|q| q.rect.y >= 0.0), "the quads are cut to the bar");

        // Not scrollable and too many rows: the overflow button in the bottom row.
        let fixed = tab_layout::Settings { prefs: tab_layout::Prefs { scrollable: false, ..s.prefs }, ..s.clone() };
        let bar = tab_layout::build_vertical(&inputs, 150.0, 60.0, 0.0, None, &fixed, &six);
        let mut f = Frame::default();
        let (rects, _) = tab_bar_vertical(&mut f, &looks, 1, &bar, Rect { h: 60.0, ..area }, 1.0, 0.1, 20.0, true, false, &legacy());
        assert_eq!(rects[0], row(0.0));
        assert_eq!((rects[1].h, rects[2].h), (0.0, 0.0), "in the overflow menu");
        assert!(f.rows.iter().any(|r| r.y >= 36.0 && r.spans.iter().any(|s| s.icon && s.text == TAB_OVERFLOW_GLYPH.to_string())), "the button");
    }

    #[test]
    fn group_run_follows_iterm2() {
        use crate::tab_layout::{self, ChipInput, TabInput};
        assert_eq!(collapsed_label("Build", 3), "Build  3");
        assert_eq!(collapsed_label("", 3), "3");
        assert_eq!(chip_width(40.0, false), 66.0);
        assert_eq!(chip_width(40.0, true), 74.0);
        assert_eq!(contrasting(rgb(0, 122, 255)), rgb(0xff, 0xff, 0xff), "luma 0.40");
        assert_eq!(contrasting(rgb(255, 149, 0)), rgb(0, 0, 0), "luma 0.64");

        let s = tab_layout::Settings::from_globals(&crate::globals::Globals::empty(std::path::Path::new("unused.toml")));
        let six = |t: &str, _pt: f32, _bold: bool| t.chars().count() as f32 * 6.0;
        let orange = rgb(255, 149, 0);
        let looks = |collapsed: bool| -> Vec<TabLook> {
            ["a", "b", "c"]
                .iter()
                .enumerate()
                .map(|(i, t)| TabLook { title: t.to_string(), number: i + 1, group: (i < 2).then(|| ("g".to_string(), orange)), hidden: collapsed && i < 2, ..TabLook::default() })
                .collect()
        };
        let inputs = |collapsed: bool| -> Vec<TabInput> {
            looks(collapsed)
                .iter()
                .enumerate()
                .map(|(i, l)| TabInput {
                    title: l.title.clone(),
                    subtitle: String::new(),
                    icon: false,
                    processing: false,
                    number: l.number,
                    pinned: false,
                    group: (i < 2).then(|| ChipInput { id: "g".into(), name: "g".into(), collapsed, count: 2, first: i == 0 }),
                    hidden: l.hidden,
                    graphic: false,
                    letter: String::new(),
                })
                .collect()
        };
        let area = Rect { x: 0.0, y: 0.0, w: 600.0, h: 24.0 };
        let bar = tab_layout::build(&inputs(false), 2, 600.0, 0.0, None, &s, &six);
        let mut f = Frame::default();
        tab_bar(&mut f, &looks(false), 2, &bar, area, 1.0, 0.1, 20.0, true, false, &legacy());
        let chip = bar.chips[0].1;
        let (t0, t1) = (bar.cells[0].2, bar.cells[1].2);
        let has = |f: &Frame, rect: Rect| f.quads.iter().any(|q| q.rect == rect && q.color == rgba(orange, 1.0));
        // The chip before tab 0 is solid group color, the bottom row left to the divider.
        assert_eq!(chip.x + chip.w, t0.x);
        assert!(has(&f, Rect { x: chip.x, y: 0.0, w: t0.x - chip.x, h: 23.0 }), "the name block");
        // A 1-pixel outline around the chip and both tabs: outset 1 cut to the bar at the top,
        // the bottom row left to the divider.
        let b = Rect { x: chip.x, y: 0.0, w: t1.x + t1.w - chip.x, h: 23.0 };
        for edge in [Rect { h: 1.0, ..b }, Rect { y: 22.0, h: 1.0, ..b }, Rect { w: 1.0, ..b }, Rect { x: b.x + b.w - 1.0, w: 1.0, ..b }] {
            assert!(has(&f, edge), "{edge:?}");
        }
        let name = f.rows.iter().find(|r| r.spans.iter().any(|s| s.text == "g")).expect("the name");
        assert!(name.spans[0].bold && name.spans[0].color == rgb(0, 0, 0), "bold, black on orange");
        assert_eq!(name.x, chip.x + GROUP_NAME_PAD);
        assert_eq!(name.scale, GROUP_NAME_PT * 0.1);
        // The side bar returns the chip with its group id.
        let side = Rect { x: 0.0, y: 0.0, w: 150.0, h: 200.0 };
        let vbar = tab_layout::build_vertical(&inputs(false), 150.0, 200.0, 0.0, None, &s, &six);
        let (rects, chips) = tab_bar_vertical(&mut Frame::default(), &looks(false), 2, &vbar, side, 1.0, 0.1, 20.0, true, false, &legacy());
        assert_eq!(chips, vec![(Rect { x: 0.0, y: 0.0, w: 150.0, h: 24.0 }, "g".to_string())]);
        assert_eq!(rects[0].y, 24.0, "the run's first tab is under its chip");

        // Collapsed: both tabs hidden, the chip drawn with "g  2" and the chevron.
        let bar = tab_layout::build(&inputs(true), 2, 600.0, 0.0, None, &s, &six);
        let mut f = Frame::default();
        tab_bar(&mut f, &looks(true), 2, &bar, area, 1.0, 0.1, 20.0, true, false, &legacy());
        let chip = bar.chips[0].1;
        assert!(has(&f, Rect { x: chip.x, y: 0.0, w: chip.w, h: 23.0 }), "the collapsed block");
        assert!(f.rows.iter().any(|r| r.spans.iter().any(|s| s.text == "g  2" && s.bold)), "the label");
        let chevron = f.rows.iter().find(|r| r.spans.iter().any(|s| s.icon && s.text == GROUP_CHEVRON_GLYPH.to_string())).expect("the chevron");
        assert!(chevron.x > chip.x + chip.w - GROUP_NAME_PAD - GROUP_CHEVRON && chevron.x < chip.x + chip.w - GROUP_NAME_PAD);
        assert!(!f.rows.iter().any(|r| r.spans.iter().any(|s| s.text == "a" || s.text == "b")), "the members are not drawn");
        // No drag over it: no accent frame. A dragged tab poised to join it: a 2-point frame 1
        // point outside the block, in the accent color, cut to the bar (`YS:1497-1504`).
        let accent = |f: &Frame| f.quads.iter().filter(|q| q.color == rgba(LIST_SELECTED, 1.0)).map(|q| q.rect).collect::<Vec<_>>();
        assert!(accent(&f).is_empty());
        let joining = tab_layout::Bar { join: Some("g".into()), ..bar.clone() };
        let mut f = Frame::default();
        tab_bar(&mut f, &looks(true), 2, &joining, area, 1.0, 0.1, 20.0, true, false, &legacy());
        let frame = accent(&f);
        assert_eq!(frame.len(), 4, "{frame:?}");
        assert!(frame.contains(&Rect { x: chip.x + chip.w - 1.0, y: 0.0, w: 2.0, h: 24.0 }), "the right edge: {frame:?}");
        assert!(frame.contains(&Rect { x: 0.0, y: 22.0, w: chip.w + 1.0, h: 2.0 }), "the bottom edge: {frame:?}");
        // Another group's id frames nothing; an expanded group is never a join target.
        let other = tab_layout::Bar { join: Some("h".into()), ..bar.clone() };
        let mut f = Frame::default();
        tab_bar(&mut f, &looks(true), 2, &other, area, 1.0, 0.1, 20.0, true, false, &legacy());
        assert!(accent(&f).is_empty());
        let expanded = tab_layout::Bar { join: Some("g".into()), ..tab_layout::build(&inputs(false), 2, 600.0, 0.0, None, &s, &six) };
        let mut f = Frame::default();
        tab_bar(&mut f, &looks(false), 2, &expanded, area, 1.0, 0.1, 20.0, true, false, &legacy());
        assert!(accent(&f).is_empty());
        let vbar = tab_layout::build_vertical(&inputs(true), 150.0, 200.0, 0.0, None, &s, &six);
        let (rects, chips) = tab_bar_vertical(&mut Frame::default(), &looks(true), 2, &vbar, side, 1.0, 0.1, 20.0, true, false, &legacy());
        let zero = Rect { x: 0.0, y: 0.0, w: 0.0, h: 0.0 };
        assert_eq!((rects[0], rects[1]), (zero, zero), "hidden tabs are zero-size");
        assert_eq!(rects[2].y, 24.0);
        assert_eq!(chips.len(), 1);
    }

    #[test]
    fn tab_bar_draws_the_layout() {
        use crate::tab_layout::{self, TabInput};
        assert_eq!(tab_icon(true, true, true, true, true), None, "a status dot wins, and draws itself");
        assert_eq!(tab_icon(false, true, true, true, true), Some(TabIcon::Dead));
        assert_eq!(tab_icon(false, false, true, true, true), Some(TabIcon::Bell));
        assert_eq!(tab_icon(false, false, false, true, true), Some(TabIcon::NewOutput));
        assert_eq!(tab_icon(false, false, false, false, true), Some(TabIcon::Idle));
        assert_eq!(tab_icon(false, false, false, false, false), None);

        let green = rgb(0, 0xff, 0);
        let looks = vec![
            TabLook { title: "shell".into(), dot: Some(green), processing: true, number: 1, ..TabLook::default() },
            TabLook { title: "two".into(), icon: Some(TabIcon::Bell), number: 2, ..TabLook::default() },
        ];
        let inputs: Vec<TabInput> = looks
            .iter()
            .map(|l| TabInput { title: l.title.clone(), subtitle: l.subtitle.clone(), icon: l.dot.is_some() || l.icon.is_some(), processing: l.processing, number: l.number, pinned: false, group: None, hidden: false, graphic: false, letter: String::new() })
            .collect();
        let s = tab_layout::Settings::from_globals(&crate::globals::Globals::empty(std::path::Path::new("unused.toml")));
        let six = |t: &str, _pt: f32, _bold: bool| t.chars().count() as f32 * 6.0;
        // Tab 0 hovered, tab 1 active; 2 pixels a point, the bar 10 pixels down.
        let bar = tab_layout::build(&inputs, 1, 400.0, 0.0, Some(0), &s, &six);
        let area = Rect { x: 0.0, y: 10.0, w: 800.0, h: 48.0 };
        let px = |r: Rect| Rect { x: area.x + r.x * 2.0, y: area.y + r.y * 2.0, w: r.w * 2.0, h: r.h * 2.0 };
        let mut f = Frame::default();
        tab_bar(&mut f, &looks, 1, &bar, area, 2.0, 0.1, 20.0, true, false, &legacy());

        assert!(f.quads.iter().any(|q| q.rect == px(bar.cells[1].2) && q.color == rgba(TAB_ACTIVE, 1.0)), "the active tab's background");
        let glyph = |c: char| f.rows.iter().filter(move |r| r.spans.iter().any(|s| s.icon && s.text == c.to_string()));
        let tab0 = px(bar.cells[0].2);
        let inside = |r: Rect, outer: Rect| r.x >= outer.x && r.x + r.w <= outer.x + outer.w && r.y >= outer.y && r.y + r.h <= outer.y + outer.h;
        let closes: Vec<&TextRow> = glyph(TAB_CLOSE_GLYPH).collect();
        assert_eq!(closes.len(), 1, "only the hovered tab's close button");
        assert!(tab0.contains(closes[0].x, closes[0].y + 1.0));

        // "two" (18 points, 36 pixels) centered in its label.
        let label = px(bar.cells[1].1.label);
        let two = f.rows.iter().find(|r| r.spans.iter().any(|s| s.text == "two")).expect("the title");
        assert!(two.spans[0].prose);
        assert_eq!(two.x, label.x + (label.w - 36.0) / 2.0);
        assert_eq!(two.scale, 11.0 * 0.1, "11 points");
        // Its line (20 x 1.1 pixels) centered in the bar.
        assert_eq!(two.y, area.y + ((48.0 - 22.0_f32) / 2.0).floor());

        // The number centered in its box, iTerm2's floor.
        let counter = px(bar.cells[0].1.counter.unwrap());
        let win1 = f.rows.iter().find(|r| r.spans.iter().any(|s| s.text == "Win+1")).expect("the number");
        assert_eq!(win1.x, counter.x + ((counter.w - 60.0) / 2.0).floor());

        // The status dot is the tab's icon, in the icon rect.
        let icon = px(bar.cells[0].1.icon.unwrap());
        assert!(f.quads.iter().any(|q| q.color == rgba(green, 1.0) && inside(q.rect, icon)), "the dot");
        // The bell of the other tab, in its icon rect.
        let bell = glyph('\u{EA8F}').next().expect("the bell");
        assert!(px(bar.cells[1].1.icon.unwrap()).contains(bell.x + 1.0, bell.y + 1.0));

        // The spinner, in the indicator rect of the processing tab only.
        let spin = px(bar.cells[0].1.indicator.unwrap());
        let arcs: Vec<&Quad> = f.quads.iter().filter(|q| inside(q.rect, spin)).collect();
        assert!(arcs.len() >= 6, "{arcs:?}");
        assert!(bar.cells[1].1.indicator.is_none());

        // HTML: an underlined run gets a one-pixel line under it.
        let html = tab_layout::Settings { html: true, ..s.clone() };
        let ul = vec![TabLook { title: "<u>ab</u>".into(), ..TabLook::default() }, TabLook::default()];
        let uin: Vec<TabInput> = ul.iter().map(|l| TabInput { title: l.title.clone(), subtitle: String::new(), icon: false, processing: false, number: 0, pinned: false, group: None, hidden: false, graphic: false, letter: String::new() }).collect();
        let bar = tab_layout::build(&uin, 0, 400.0, 0.0, None, &html, &six);
        let mut f = Frame::default();
        tab_bar(&mut f, &ul, 0, &bar, area, 2.0, 0.1, 20.0, true, false, &legacy());
        let ab = f.rows.iter().find(|r| r.spans.iter().any(|s| s.text == "ab")).expect("the run");
        assert!(f.quads.iter().any(|q| q.rect.h == 1.0 && q.rect.w == 24.0 && q.rect.x == ab.x), "{:?}", f.quads);

        // A subtitle line on a tall bar: the title 5 points up, the subtitle under it at 9 points.
        let tall = tab_layout::Settings { height: 28.0, ..s.clone() };
        let st = vec![TabLook { title: "t".into(), subtitle: "sub".into(), ..TabLook::default() }, TabLook::default()];
        let sin: Vec<TabInput> = st.iter().map(|l| TabInput { title: l.title.clone(), subtitle: l.subtitle.clone(), icon: false, processing: false, number: 0, pinned: false, group: None, hidden: false, graphic: false, letter: String::new() }).collect();
        let bar = tab_layout::build(&sin, 0, 400.0, 0.0, None, &tall, &six);
        let area = Rect { x: 0.0, y: 0.0, w: 800.0, h: 56.0 };
        let mut f = Frame::default();
        tab_bar(&mut f, &st, 0, &bar, area, 2.0, 0.1, 20.0, true, false, &legacy());
        let t = f.rows.iter().find(|r| r.spans.iter().any(|s| s.text == "t")).unwrap();
        let sub = f.rows.iter().find(|r| r.spans.iter().any(|s| s.text == "sub")).unwrap();
        assert_eq!(t.y, ((56.0 - 22.0_f32) / 2.0).floor() - 10.0);
        assert_eq!(sub.scale, 9.0 * 0.1);
        assert_eq!(sub.y, ((56.0 - 18.0_f32) / 2.0).floor() - 10.0 + 22.0 - 4.0);
    }

    #[test]
    fn tab_bar_cuts_tabs_at_the_pinned_add_button() {
        use crate::tab_layout::{self, TabInput};
        let base = tab_layout::Settings::from_globals(&crate::globals::Globals::empty(std::path::Path::new("unused.toml")));
        let s = tab_layout::Settings { prefs: tab_layout::Prefs { stretch: false, scrollable: true, ..base.prefs }, ..base };
        let looks: Vec<TabLook> = (1..=10).map(|i| TabLook { title: format!("tab{i}"), number: i, ..TabLook::default() }).collect();
        let inputs: Vec<TabInput> = looks.iter().map(|l| TabInput { title: l.title.clone(), subtitle: String::new(), icon: false, processing: false, number: l.number, pinned: false, group: None, hidden: false, graphic: false, letter: String::new() }).collect();
        let six = |t: &str, _pt: f32, _bold: bool| t.chars().count() as f32 * 6.0;
        // 120-point tabs scrolled by 100: tab 5 runs from 500 to 620, past the "+" at 576.
        let bar = tab_layout::build(&inputs, 5, 600.0, 100.0, None, &s, &six);
        assert_eq!(bar.cells[5].2.x, 500.0);
        let area = Rect { x: 0.0, y: 0.0, w: 600.0, h: 24.0 };
        let mut f = Frame::default();
        tab_bar(&mut f, &looks, 5, &bar, area, 1.0, 0.1, 20.0, true, false, &legacy());
        let add = bar.add.unwrap();
        assert_eq!(add.x, 576.0);
        let active = f.quads.iter().find(|q| q.color == rgba(TAB_ACTIVE, 1.0)).expect("the active tab");
        assert_eq!(active.rect, Rect { x: 500.0, y: 0.0, w: 76.0, h: 24.0 }, "cut at the pinned +");
        let plus = TAB_ADD_GLYPH.to_string();
        for r in f.rows.iter().filter(|r| r.spans.iter().all(|s| s.text != plus)) {
            assert!(r.clip.x + r.clip.w <= 576.0, "{r:?}");
            assert!(r.clip.x >= 0.0, "{r:?}");
        }
        let drawn = f.rows.iter().find(|r| r.spans.iter().any(|s| s.icon && s.text == plus)).expect("the + is drawn");
        assert!(add.contains(drawn.x + 1.0, drawn.y + 1.0));
    }

    #[test]
    fn tab_overflow_button_is_a_double_chevron() {
        use crate::tab_layout::{self, TabInput};
        let base = tab_layout::Settings::from_globals(&crate::globals::Globals::empty(std::path::Path::new("unused.toml")));
        let s = tab_layout::Settings { prefs: tab_layout::Prefs { scrollable: false, ..base.prefs }, ..base };
        let looks: Vec<TabLook> = (1..=10).map(|i| TabLook { title: format!("tab{i}"), number: i, ..TabLook::default() }).collect();
        let inputs: Vec<TabInput> = looks.iter().map(|l| TabInput { title: l.title.clone(), subtitle: String::new(), icon: false, processing: false, number: l.number, pinned: false, group: None, hidden: false, graphic: false, letter: String::new() }).collect();
        let six = |t: &str, _pt: f32, _bold: bool| t.chars().count() as f32 * 6.0;
        let bar = tab_layout::build(&inputs, 0, 400.0, 0.0, None, &s, &six);
        let overflow = bar.overflow.expect("tabs overflow");
        let area = Rect { x: 0.0, y: 0.0, w: 400.0, h: 24.0 };
        // iTerm2's `overflowImage` is a right-pointing double chevron, gray; `overflowImagePressed`
        // while its menu is open is the same in the text color (`PSMOverflowPopUpButton`).
        for (down, color) in [(false, TAB_TEXT_DIM), (true, TAB_TEXT)] {
            let mut f = Frame::default();
            tab_bar(&mut f, &looks, 0, &bar, area, 1.0, 0.1, 20.0, true, down, &legacy());
            let chevrons: Vec<&TextRow> = f.rows.iter().filter(|r| r.spans.iter().any(|s| s.icon && s.text == TAB_OVERFLOW_GLYPH.to_string())).collect();
            assert_eq!(chevrons.len(), 2, "two chevrons");
            assert!(chevrons[0].x < chevrons[1].x);
            assert!(chevrons.iter().all(|r| r.spans[0].color == color && overflow.contains(r.x + 1.0, r.y + 1.0)));
            // Centered in the button: the pair's middle is the button's middle.
            let mid = (chevrons[0].x + chevrons[1].x + TAB_GLYPH_PT) / 2.0;
            assert!((mid - (overflow.x + overflow.w / 2.0)).abs() <= 1.0, "{mid} in {overflow:?}");
        }
    }

    #[test]
    fn tab_close_button_dims_in_an_inactive_window() {
        use crate::tab_layout::{self, TabInput};
        let s = tab_layout::Settings::from_globals(&crate::globals::Globals::empty(std::path::Path::new("unused.toml")));
        let looks = vec![TabLook { title: "a".into(), number: 1, ..TabLook::default() }, TabLook { title: "b".into(), number: 2, ..TabLook::default() }];
        let inputs: Vec<TabInput> = looks.iter().map(|l| TabInput { title: l.title.clone(), subtitle: String::new(), icon: false, processing: false, number: l.number, pinned: false, group: None, hidden: false, graphic: false, letter: String::new() }).collect();
        let six = |t: &str, _pt: f32, _bold: bool| t.chars().count() as f32 * 6.0;
        // Tab 1 hovered and not active: its close button over the bar's color.
        let bar = tab_layout::build(&inputs, 0, 400.0, 0.0, Some(1), &s, &six);
        let area = Rect { x: 0.0, y: 0.0, w: 400.0, h: 24.0 };
        let close_color = |key: bool| {
            let mut f = Frame::default();
            tab_bar(&mut f, &looks, 0, &bar, area, 1.0, 0.1, 20.0, key, false, &legacy());
            let row = f.rows.iter().find(|r| r.spans.iter().any(|s| s.text == TAB_CLOSE_GLYPH.to_string())).expect("the close button");
            row.spans[0].color
        };
        assert_eq!(close_color(true), TAB_TEXT_DIM, "full alpha in the key window");
        assert_eq!(close_color(false), blend(TAB_BAR, TAB_TEXT_DIM, 0.5), "half alpha otherwise (YS drawInteriorWithTabCell:)");
        // Held down, a shade shows behind the glyph in its box.
        let close = bar.cells[1].1.close.unwrap();
        let shade = rgba(blend(TAB_BAR, TAB_TEXT_DIM, TAB_CLOSE_PRESSED), 1.0);
        let mut f = Frame::default();
        tab_bar(&mut f, &looks, 0, &bar, area, 1.0, 0.1, 20.0, true, false, &legacy());
        assert!(!f.quads.iter().any(|q| q.color == shade), "not held");
        let held: Vec<TabLook> = looks.iter().cloned().enumerate().map(|(i, l)| TabLook { close_pressed: i == 1, ..l }).collect();
        let mut f = Frame::default();
        tab_bar(&mut f, &held, 0, &bar, area, 1.0, 0.1, 20.0, true, false, &legacy());
        assert!(f.quads.iter().any(|q| q.color == shade && q.rect == close), "{:?}", f.quads);
    }

    #[test]
    fn pinned_look_follows_iterm2() {
        use crate::tab_layout::{self, TabInput};
        let s = tab_layout::Settings::from_globals(&crate::globals::Globals::empty(std::path::Path::new("unused.toml")));
        let looks = vec![TabLook { title: "vim".into(), number: 1, pinned: true, ..TabLook::default() }, TabLook { title: "two".into(), number: 2, ..TabLook::default() }];
        let inputs: Vec<TabInput> = looks.iter().map(|l| TabInput { title: l.title.clone(), subtitle: String::new(), icon: false, processing: false, number: l.number, pinned: l.pinned, group: None, hidden: false, graphic: false, letter: String::new() }).collect();
        let six = |t: &str, _pt: f32, _bold: bool| t.chars().count() as f32 * 6.0;
        // Hovered, the pinned tab still has no close button.
        let bar = tab_layout::build(&inputs, 1, 400.0, 0.0, Some(0), &s, &six);
        let cell = bar.cells[0].2;
        assert_eq!(cell.w, PINNED_TAB_WIDTH);
        let area = Rect { x: 0.0, y: 0.0, w: 400.0, h: 24.0 };
        let mut f = Frame::default();
        tab_bar(&mut f, &looks, 1, &bar, area, 1.0, 0.1, 20.0, true, false, &legacy());
        let in_cell = |r: &&TextRow| cell.contains(r.x, r.y + 1.0);
        let pins: Vec<&TextRow> = f.rows.iter().filter(|r| r.spans.iter().any(|s| s.icon && s.text == PIN_GLYPH.to_string())).collect();
        assert_eq!(pins.len(), 1, "one pin");
        assert_eq!(pins[0].x, cell.x + 6.0, "at the cell's left margin");
        let texts: Vec<String> = f.rows.iter().filter(in_cell).filter(|r| r.spans.iter().all(|s| !s.icon)).map(|r| r.spans.iter().map(|s| s.text.as_str()).collect()).collect();
        assert_eq!(texts, vec!["v".to_string()], "only the first character");
        assert!(!f.rows.iter().filter(in_cell).any(|r| r.spans.iter().any(|s| s.text == TAB_CLOSE_GLYPH.to_string())), "no close button");
        // The pin at iTerm2's alpha 0.6 in a key window, 0.3 otherwise.
        assert_eq!(pins[0].spans[0].color, blend(TAB_BAR, TAB_TEXT_DIM, 0.6));
        let mut f = Frame::default();
        tab_bar(&mut f, &looks, 1, &bar, area, 1.0, 0.1, 20.0, false, false, &legacy());
        let pin = f.rows.iter().find(|r| r.spans.iter().any(|s| s.text == PIN_GLYPH.to_string())).unwrap();
        assert_eq!(pin.spans[0].color, blend(TAB_BAR, TAB_TEXT_DIM, 0.3));

        // A side bar: the pin, then the whole title.
        let mut f = Frame::default();
        let side = Rect { x: 0.0, y: 0.0, w: 150.0, h: 100.0 };
        let vbar = tab_layout::build_vertical(&inputs, 150.0, 100.0, 0.0, None, &s, &six);
        let (rects, _) = tab_bar_vertical(&mut f, &looks, 1, &vbar, side, 1.0, 0.1, 20.0, true, false, &legacy());
        let pin = f.rows.iter().find(|r| r.spans.iter().any(|s| s.text == PIN_GLYPH.to_string())).expect("the pin");
        assert_eq!(pin.x, rects[0].x + 6.0);
        let vim = f.rows.iter().find(|r| r.spans.iter().any(|s| s.text == "vim")).expect("the whole title");
        assert!(rects[0].contains(vim.x, vim.y) && vim.x >= rects[0].x + 6.0 + 16.0 + 4.0);

        // With an icon: the icon, and neither the pin nor the title (`YS:916`, `YS:1000`).
        let letter = TabImage::Letter { letter: "V".into(), color: rgb(0xf7, 0x7b, 0x87) };
        let iconed: Vec<TabLook> = looks.iter().cloned().enumerate().map(|(i, l)| TabLook { icon_image: (i == 0).then(|| letter.clone()), ..l }).collect();
        let bar = tab_layout::build(&icon_inputs(&iconed), 1, 400.0, 0.0, None, &s, &six);
        let cell = bar.cells[0].2;
        let mut f = Frame::default();
        tab_bar(&mut f, &iconed, 1, &bar, area, 1.0, 0.1, 20.0, true, false, &legacy());
        assert!(!f.rows.iter().any(|r| r.spans.iter().any(|s| s.text == PIN_GLYPH.to_string())), "no pin");
        let texts: Vec<String> = f.rows.iter().filter(|r| cell.contains(r.x, r.y + 1.0)).map(|r| r.spans.iter().map(|s| s.text.as_str()).collect()).collect();
        assert_eq!(texts, vec!["V".to_string()], "the icon's letter only, no title");
    }

    /// The tab bar inputs for looks, as `App::tab_inputs` makes them (no groups).
    fn icon_inputs(looks: &[TabLook]) -> Vec<crate::tab_layout::TabInput> {
        looks
            .iter()
            .map(|l| crate::tab_layout::TabInput {
                title: l.title.clone(),
                subtitle: String::new(),
                icon: false,
                processing: false,
                number: l.number,
                pinned: l.pinned,
                group: None,
                hidden: false,
                graphic: l.icon_image.is_some(),
                letter: match &l.icon_image {
                    Some(TabImage::Letter { letter, .. }) => letter.clone(),
                    _ => String::new(),
                },
            })
            .collect()
    }

    #[test]
    fn tab_icon_draws() {
        use crate::tab_layout;
        let s = tab_layout::Settings::from_globals(&crate::globals::Globals::empty(std::path::Path::new("unused.toml")));
        let six = |t: &str, _pt: f32, _bold: bool| t.chars().count() as f32 * 6.0;
        let c = rgb(0xf7, 0x7b, 0x87);
        let pixels = std::sync::Arc::new(vec![9u8; 16]);
        let picture = TabImage::Picture { key: 42, pixels: pixels.clone(), size: (2, 2) };
        let looks = vec![
            TabLook { title: "vim".into(), number: 1, icon_image: Some(TabImage::Letter { letter: "V".into(), color: c }), ..TabLook::default() },
            TabLook { title: "git".into(), number: 2, icon_image: Some(picture.clone()), ..TabLook::default() },
        ];
        let bar = tab_layout::build(&icon_inputs(&looks), 1, 400.0, 0.0, None, &s, &six);
        let (c0, c1) = (bar.cells[0].2, bar.cells[1].2);
        let area = Rect { x: 0.0, y: 0.0, w: 400.0, h: 24.0 };
        let mut f = Frame::default();
        tab_bar(&mut f, &looks, 1, &bar, area, 1.0, 0.1, 20.0, true, false, &legacy());
        // The letter: a circle 1 point inside the 16-point box 6 points in (radius half its
        // side), the letter bold in the bar's color centered on it.
        let circle = Rect { x: c0.x + 7.0, y: 5.0, w: 14.0, h: 14.0 };
        let dots: Vec<Rect> = f.quads.iter().filter(|q| q.color == rgba(c, 1.0)).map(|q| q.rect).collect();
        let mut expect = Vec::new();
        rounded_box(&mut expect, circle, 7.0, rgba(c, 1.0));
        assert_eq!(dots, expect.iter().map(|q| q.rect).collect::<Vec<_>>());
        assert_eq!(dots[0], Rect { x: circle.x + 4.0, y: circle.y, w: 6.0, h: 1.0 }, "round at the top");
        let v = f.rows.iter().find(|r| r.spans.iter().any(|s| s.text == "V")).expect("the letter");
        assert!(v.spans[0].bold && v.spans[0].color == TAB_BAR);
        assert_eq!(v.x, circle.x + (14.0 - 6.0) / 2.0, "centered");
        assert_eq!(v.scale, LETTER_PT * 0.1);
        // The picture: one image, 16 points square, 6 points in and centered down.
        assert_eq!(f.images.len(), 1);
        assert_eq!(f.images[0], ImageQuad { rect: Rect { x: c1.x + 6.0, y: 4.0, w: 16.0, h: 16.0 }, uv: [0.0, 0.0, 1.0, 1.0], key: (42, 0), pixels: pixels.clone(), size: (2, 2), repeat: false });
        let title = f.rows.iter().find(|r| r.spans.iter().any(|s| s.text == "git")).expect("the title");
        assert!(title.x > c1.x + 6.0 + 16.0, "a short title stays centered, clear of the icon");
        // At 2 pixels a point the picture is 32 pixels square.
        let mut f = Frame::default();
        tab_bar(&mut f, &looks, 1, &bar, Rect { x: 0.0, y: 0.0, w: 800.0, h: 48.0 }, 2.0, 0.2, 40.0, true, false, &legacy());
        assert_eq!(f.images[0].rect, Rect { x: 2.0 * c1.x + 12.0, y: 8.0, w: 32.0, h: 32.0 });

        // A long title is truncated after the icon's room, 16 + 4 + 2 (`YS:1117`), not the
        // close button's 16 + 4.
        let long = "a title far too long for any tab in this bar at all, truly".to_string();
        let title_x = |image: Option<TabImage>| {
            let looks = vec![TabLook { title: long.clone(), number: 1, icon_image: image, ..TabLook::default() }, TabLook { title: "b".into(), number: 2, ..TabLook::default() }];
            let bar = tab_layout::build(&icon_inputs(&looks), 1, 400.0, 0.0, None, &s, &six);
            let mut f = Frame::default();
            tab_bar(&mut f, &looks, 1, &bar, area, 1.0, 0.1, 20.0, true, false, &legacy());
            let row = f.rows.iter().find(|r| r.spans.iter().any(|s| s.text.starts_with("a title"))).expect("the title");
            row.x - bar.cells[0].2.x
        };
        assert_eq!(title_x(None), 6.0 + 20.0);
        assert_eq!(title_x(Some(picture.clone())), 6.0 + 22.0);

        // Hovered, the close button shows and the icon fades out (alpha 1 less the close
        // button's); in a window that is not key the close button is half and so is the icon.
        let hover = tab_layout::build(&icon_inputs(&looks), 1, 400.0, 0.0, Some(1), &s, &six);
        let mut f = Frame::default();
        tab_bar(&mut f, &looks, 1, &hover, area, 1.0, 0.1, 20.0, true, false, &legacy());
        assert!(f.images.is_empty());
        let mut f = Frame::default();
        tab_bar(&mut f, &looks, 1, &hover, area, 1.0, 0.1, 20.0, false, false, &legacy());
        assert_eq!(f.images[0].key, (42, 127));
        assert_eq!(f.images[0].pixels[3], 5, "alpha 9 at half");

        // A side bar's label starts after the icon.
        let side = Rect { x: 0.0, y: 0.0, w: 150.0, h: 100.0 };
        let vbar = tab_layout::build_vertical(&icon_inputs(&looks), 150.0, 100.0, 0.0, None, &s, &six);
        let mut f = Frame::default();
        let (rects, _) = tab_bar_vertical(&mut f, &looks, 1, &vbar, side, 1.0, 0.1, 20.0, true, false, &legacy());
        assert_eq!(f.images[0].rect, Rect { x: rects[1].x + 6.0, y: rects[1].y + 4.0, w: 16.0, h: 16.0 });
        assert!(vbar.cells.iter().all(|(_, l, r)| l.label.x >= r.x + 26.0));
    }

    #[test]
    fn dragged_tab_has_its_own_background() {
        use crate::tab_layout::{self, TabInput};
        let s = tab_layout::Settings::from_globals(&crate::globals::Globals::empty(std::path::Path::new("unused.toml")));
        let looks = vec![TabLook { title: "a".into(), number: 1, ..TabLook::default() }, TabLook { title: "b".into(), number: 2, ..TabLook::default() }];
        let inputs: Vec<TabInput> = looks.iter().map(|l| TabInput { title: l.title.clone(), subtitle: String::new(), icon: false, processing: false, number: l.number, pinned: false, group: None, hidden: false, graphic: false, letter: String::new() }).collect();
        let six = |t: &str, _pt: f32, _bold: bool| t.chars().count() as f32 * 6.0;
        let bar = tab_layout::build(&inputs, 0, 400.0, 0.0, None, &s, &six);
        let area = Rect { x: 0.0, y: 0.0, w: 400.0, h: 24.0 };
        let cell = bar.cells[1].2;
        let back = |looks: &[TabLook]| {
            let mut f = Frame::default();
            tab_bar(&mut f, looks, 0, &bar, area, 1.0, 0.1, 20.0, true, false, &legacy());
            f.quads.iter().any(|q| q.rect == cell && q.color == rgba(TAB_BAR, 1.0))
        };
        assert!(!back(&looks), "an inactive tab draws on the bar");
        let dragged: Vec<TabLook> = looks.iter().cloned().enumerate().map(|(i, l)| TabLook { dragged: i == 1, ..l }).collect();
        assert!(back(&dragged), "a dragged tab covers the tabs it passes");
    }

    #[test]
    fn match_highlights_use_the_match_color() {
        let s = screen(b"xx ab ab");
        let m = crate::find::find_all(&s, "ab").unwrap();
        let mut f = Frame::default();
        let yellow = rgb(0xff, 0xff, 0x00);
        pane_with(&mut f, &s, RECT, CELL, true, &CursorLook::shown(false), &DrawPrefs::default(), Some(&Highlights { matches: &m, color: yellow, hover: None, link: yellow, labels: &[], notes: &[], dim: crate::dim::Dim::LEGACY, key_cursor: false, bg_alpha: 1.0, opaque_colors: false, text_blink_on: true }), crate::render::DEFAULT_FACE);
        let y = rgba(yellow, 1.0);
        assert!(f.quads.iter().any(|q| q.color == y && q.rect == Rect { x: 30.0, y: 0.0, w: 20.0, h: 20.0 }), "{:?}", f.quads);
        assert!(f.quads.iter().any(|q| q.color == y && q.rect == Rect { x: 60.0, y: 0.0, w: 20.0, h: 20.0 }));
        // Text on a yellow match is black.
        let ab = f.rows.iter().flat_map(|r| &r.spans).find(|sp| sp.text.starts_with("ab")).unwrap();
        assert_eq!(ab.color, rgb(0, 0, 0));
    }

    #[test]
    fn the_find_bar_covers_the_text_under_it() {
        let s = screen(b"a long line of text that runs under the find bar");
        let mut f = Frame::default();
        pane(&mut f, &s, RECT, CELL, true, &CursorLook::shown(false));
        let bar = FindBar { query: "abc", count: "2/5", mode: "Smart Case", error: false };
        let mode = find_bar(&mut f, &bar, RECT, CELL);
        let texts: Vec<String> = f.rows.iter().map(|r| r.spans.iter().map(|s| s.text.as_str()).collect()).collect();
        assert!(texts.iter().any(|t| t.contains("abc") && t.contains("2/5")), "{texts:?}");
        assert!(texts.iter().any(|t| t.contains("Smart Case")), "{texts:?}");
        let boxed = f.quads.last().unwrap().rect;
        assert_eq!(boxed.x + boxed.w, RECT.x + RECT.w);
        assert!(mode.x >= boxed.x && mode.x + mode.w <= boxed.x + boxed.w && mode.y == boxed.y);
        // The pane's first row stops where the bar starts.
        assert!(f.rows[0].clip.x + f.rows[0].clip.w <= boxed.x);
    }

    #[test]
    fn links_draw_a_dashed_underline() {
        // iTerm2's 4 on, 3 off dashes at the underline position (R3), only with UnderlineHyperlinks.
        let s = screen(b"\x1b]8;;https://x/\x1b\\ab\x1b]8;;\x1b\\ c");
        let mut f = Frame::default();
        pane(&mut f, &s, RECT, CELL, true, &CursorLook::shown(false));
        let under: Vec<(f32, f32, f32)> = f.underlines.iter().map(|q| (q.rect.x, q.rect.w, q.rect.y)).collect();
        assert_eq!(under, vec![(0.0, 4.0, 19.0), (7.0, 4.0, 19.0), (14.0, 4.0, 19.0)]);
        assert!(f.underlines.iter().all(|q| q.rect.h == 1.0));
        assert!(f.overlay.is_empty(), "the old overlay dashes are gone");
        let mut off = Frame::default();
        pane_with(&mut off, &s, RECT, CELL, true, &CursorLook::shown(false), &DrawPrefs { underline_hyperlinks: false, ..DrawPrefs::default() }, None, crate::render::DEFAULT_FACE);
        assert!(off.underlines.is_empty());
    }

    #[test]
    fn sgr_underlines_and_strikethrough_draw() {
        let s = screen(b"\x1b[4mab\x1b[0m \x1b[9mcd\x1b[0m\x1b[4:3mx");
        let mut f = Frame::default();
        pane(&mut f, &s, RECT, CELL, true, &CursorLook::shown(false));
        let fg = rgba(s.palette.fg, 1.0);
        // "ab": one merged solid line; "cd": a struck line at the baseline less half the x-height; "x": two curly lines.
        assert!(f.underlines.contains(&Quad { rect: Rect { x: 0.0, y: 19.0, w: 20.0, h: 1.0 }, color: fg }), "{:?}", f.underlines);
        assert!(f.strikes.contains(&Quad { rect: Rect { x: 30.0, y: 15.0, w: 20.0, h: 1.0 }, color: fg }), "{:?}", f.strikes);
        assert!(f.underlines.iter().all(|q| q.rect.y != 15.0 || q.rect.x < 30.0 || q.rect.x >= 50.0), "strikethrough is not an underline");
        let curly: Vec<&Quad> = f.underlines.iter().filter(|q| q.rect.x >= 50.0).collect();
        assert!(!curly.is_empty() && curly.iter().all(|q| q.rect.h == 1.0 && q.rect.w <= 3.0), "{curly:?}");
        assert!(curly.iter().any(|q| q.rect.y == 19.0) && curly.iter().any(|q| q.rect.y == 18.0));
    }

    #[test]
    fn underline_colors_follow_iterm2() {
        let mut s = screen(b"\x1b[4;58:2::255:0:0ma\x1b[0;4mb\x1b[0;4;58;5;2mc\x1b[59md");
        let red = rgba(rgb(255, 0, 0), 1.0);
        let find = |f: &Frame, x: f32| f.underlines.iter().find(|q| q.rect.x == x).map(|q| q.color);
        let mut f = Frame::default();
        pane(&mut f, &s, RECT, CELL, true, &CursorLook::shown(false));
        assert_eq!(find(&f, 0.0), Some(red), "SGR 58 wins");
        assert_eq!(find(&f, 10.0), Some(rgba(s.palette.fg, 1.0)), "else the text color");
        assert_eq!(find(&f, 20.0), Some(rgba(s.palette.ansi[2], 1.0)), "indexed SGR 58");
        // The profile's Underline Color replaces the text color, never an SGR 58 color.
        let green = rgb(0, 255, 0);
        s.palette.underline_color = Some(green);
        let mut f = Frame::default();
        pane(&mut f, &s, RECT, CELL, true, &CursorLook::shown(false));
        assert_eq!(find(&f, 0.0), Some(red));
        assert_eq!(find(&f, 10.0), Some(rgba(green, 1.0)));
        // The default color is the cell's final text color: a selected cell's.
        let runs = underline_runs(&[CellLook { ul: Some(Ul { style: crate::underline::Style::Single, strike: false, sgr: None }), text: "a".into(), fg: rgb(1, 2, 3), bg: rgb(0, 0, 0), bold: false, italic: false, selected: false, level: 0, mask: 0, cursor: false, wide: false, joins: false, table: None }], None);
        assert_eq!(runs[0].color, rgb(1, 2, 3));
    }

    /// 13n final review 1: a zoomed pane's rows are taller than the chrome's line, so a popup cuts
    /// a row by that row's own height (a row whose top is a default line above the popup still
    /// reaches into it), and a smaller-font pane's row that ends above the popup is left alone.
    #[test]
    fn popups_cut_pane_rows_by_their_own_height() {
        let mut s = Screen::new(10, 4);
        s.feed(b"line0\r\nline1\r\nline2");
        let rows_at = |f: &Frame, y: f32| f.rows.iter().filter(|r| r.y == y).map(|r| (r.clip.x, r.clip.w)).collect::<Vec<_>>();
        // Whether a piece of the row at `y` draws in the popup's columns.
        let reaches = |f: &Frame, y: f32, p: Rect| rows_at(f, y).iter().any(|&(x, w)| x < p.x + p.w && x + w > p.x);
        // A 2x pane: rows 40 tall. Row 1 spans y 40..80; the popup starts at y 70 (30 px below its top).
        let popup = Rect { x: 0.0, y: 70.0, w: 50.0, h: 20.0 };
        let mut f = Frame::default();
        pane_with(&mut f, &s, RECT, (10.0, 40.0), false, &CursorLook::shown(false), &DrawPrefs::default(), None, 7);
        assert!(!rows_at(&f, 40.0).is_empty());
        let mut g = f.clone();
        cut_out(&mut g, popup, 20.0);
        assert!(!reaches(&g, 40.0, popup), "row 1 (y 40..80) reaches the popup at y 70 and is cut: {:?}", rows_at(&g, 40.0));
        let mut g = f.clone();
        cut_rows_from(&mut g, popup, 20.0);
        assert!(!reaches(&g, 40.0, popup), "cut_rows_from by the row's height too: {:?}", rows_at(&g, 40.0));
        // A small-font pane: rows 10 tall. Row 1 is y 10..20, ending above the popup at y 70 (a 20 px chrome line would not reach either) and row 6 (y 60..70) just ends at the popup.
        let mut small = Screen::new(10, 8);
        small.feed(b"a\r\nb\r\nc\r\nd\r\ne\r\nf\r\ng");
        let mut f = Frame::default();
        pane_with(&mut f, &small, RECT, (5.0, 10.0), false, &CursorLook::shown(false), &DrawPrefs::default(), None, 7);
        let before = rows_at(&f, 60.0);
        let mut g = f.clone();
        cut_out(&mut g, Rect { x: 0.0, y: 75.0, w: 50.0, h: 20.0 }, 20.0);
        assert_eq!(rows_at(&g, 60.0), before, "the row at y 60..70 ends above the popup: left whole");
    }

    /// 13m Task 7: a popup, a toast or a find bar over a pane's right edge lies over the
    /// scrollbar, not under it: whatever cuts the text cuts the bar and its ticks.
    #[test]
    fn popups_lie_over_the_scrollbar() {
        use crate::scrollbar::{Series, Style, geometry};
        let pane = Rect { x: 0.0, y: 0.0, w: 800.0, h: 600.0 };
        let g = geometry(pane, Style::Legacy, 1.0, 100, 50, 0).unwrap();
        let hits = |f: &Frame, r: Rect| f.scrollbars.iter().any(|q| q.rect.x < r.x + r.w && q.rect.x + q.rect.w > r.x && q.rect.y < r.y + r.h && q.rect.y + q.rect.h > r.y);
        let fresh = || {
            let mut f = Frame::default();
            scrollbar(&mut f, &g, Style::Legacy, false, false);
            scrollbar_ticks(&mut f, g.minimap(false, 1.0), &[(Series::Search, vec![10, 40])], 150, 1.0);
            f
        };
        let over = Rect { x: 700.0, y: 100.0, w: 100.0, h: 300.0 };
        let mut f = fresh();
        assert!(hits(&f, over), "the bar is there before the popup");
        assert!(f.overlay.is_empty() && f.quads.is_empty(), "the bar is its own layer");
        cut_out(&mut f, over, CELL.1);
        assert!(!hits(&f, over), "cut_out trims the bar and its ticks");
        assert!(hits(&f, Rect { x: 780.0, y: 0.0, w: 20.0, h: 90.0 }), "the part above the popup stays");
        let mut f = fresh();
        cut_rows_from(&mut f, over, CELL.1);
        assert!(!hits(&f, over), "so does every surface that cuts rows");
        // The renderer draws the layer (after the strokes, before the overlay): wired, not dropped.
        assert!(include_str!("render.rs").contains("frame.scrollbars"));
    }

    #[test]
    fn underlines_are_cut_out_under_popups() {
        let mut f = Frame::default();
        let q = |x: f32, w: f32| Quad { rect: Rect { x, y: 19.0, w, h: 1.0 }, color: [1.0; 4] };
        f.underlines = vec![q(0.0, 100.0), q(200.0, 10.0)];
        cut_out(&mut f, Rect { x: 40.0, y: 0.0, w: 20.0, h: 40.0 }, 20.0);
        let xs: Vec<(f32, f32)> = f.underlines.iter().map(|q| (q.rect.x, q.rect.w)).collect();
        assert_eq!(xs, vec![(0.0, 40.0), (60.0, 40.0), (200.0, 10.0)]);
    }

    /// One rule for every Tandem surface drawn over pane rows: where it cuts the text it cuts the
    /// underline and strikethrough strokes too, or a program's SGR 4 or 9 line would draw through it.
    #[test]
    fn surfaces_over_rows_cut_the_strokes_too() {
        let line = format!("\x1b[4;9m{}\x1b[0m\r\n", "x".repeat(80)).repeat(23);
        let s = screen(line.as_bytes());
        let hits = |f: &Frame, r: Rect| f.underlines.iter().chain(&f.strikes).any(|q| q.rect.x < r.x + r.w && q.rect.x + q.rect.w > r.x && q.rect.y < r.y + r.h && q.rect.y + q.rect.h > r.y);
        let fresh = || {
            let mut f = Frame::default();
            pane(&mut f, &s, RECT, CELL, true, &CursorLook::shown(false));
            assert!(hits(&f, RECT), "the strokes are there before the surface");
            f
        };
        let mut f = fresh();
        let r = toast(&mut f, "saved", RECT, CELL);
        assert!(!hits(&f, r), "toast {r:?}");
        let mut f = fresh();
        let r = banner(&mut f, "oops", RECT, CELL);
        assert!(!hits(&f, r), "banner {r:?}");
        let mut f = fresh();
        let bar = FindBar { query: "abc", count: "2/5", mode: "Smart Case", error: false };
        let mode = find_bar(&mut f, &bar, RECT, CELL);
        let r = Rect { x: mode.x, y: 0.0, w: 10.0, h: CELL.1 };
        assert!(!hits(&f, r), "find bar {r:?}");
        let mut f = fresh();
        let ib = crate::infobar::Bar { id: "x".into(), text: "hi".into(), buttons: vec!["_Yes".into()], important: true };
        let l = crate::infobar::layout(&ib, RECT, CELL);
        let r = info_bar(&mut f, &ib, &l, CELL);
        assert!(!hits(&f, r), "info bar {r:?}");
        let mut f = fresh();
        timestamps(&mut f, &[(3, "12:00:00".to_string())], RECT, CELL, None, STAMP_COLORS);
        let r = Rect { x: 800.0 - 100.0, y: 60.0, w: 100.0, h: CELL.1 };
        assert!(!hits(&f, r), "timestamp {r:?}");
    }

    /// A row of underlined and struck SPACES pushes no text row, so a cut that follows the text
    /// rows alone leaves its strokes through the toast and the find bar: they cut by their rect.
    #[test]
    fn toast_and_find_bar_cut_strokes_on_blank_rows() {
        let line = format!("\x1b[4;9m{}\x1b[0m\r\n", " ".repeat(80)).repeat(23);
        let s = screen(line.as_bytes());
        let hits = |f: &Frame, r: Rect| f.underlines.iter().chain(&f.strikes).any(|q| q.rect.x < r.x + r.w && q.rect.x + q.rect.w > r.x && q.rect.y < r.y + r.h && q.rect.y + q.rect.h > r.y);
        let fresh = || {
            let mut f = Frame::default();
            pane(&mut f, &s, RECT, CELL, true, &CursorLook::shown(false));
            assert!(f.rows.iter().all(|r| r.spans.iter().all(|sp| sp.text.trim().is_empty())), "no row has visible text");
            assert!(hits(&f, RECT), "the strokes are there before the surface");
            f
        };
        let mut f = fresh();
        let r = toast(&mut f, "saved", RECT, CELL);
        assert!(!hits(&f, r), "toast {r:?}");
        let mut f = fresh();
        let bar = FindBar { query: "abc", count: "2/5", mode: "Smart Case", error: false };
        let mode = find_bar(&mut f, &bar, RECT, CELL);
        assert!(!hits(&f, Rect { x: mode.x, y: 0.0, w: 10.0, h: CELL.1 }), "find bar");
    }

    #[test]
    fn draw_prefs_come_from_the_globals() {
        let mut g = crate::globals::Globals::empty(std::path::Path::new("none.toml"));
        let m = crate::underline::Metrics { underline_offset: 2.0, thickness: 1.0, x_height: 8.0, baseline: 16.0 };
        let on = DrawPrefs::from_globals(&g, m, 2.0);
        assert!(on.underline_hyperlinks && !on.solid_underlines, "iTerm2's defaults");
        assert_eq!((on.metrics, on.scale), (m, 2.0));
        g.set("UnderlineHyperlinks", toml::Value::Boolean(false));
        g.set("SolidUnderlines", toml::Value::Boolean(true));
        let off = DrawPrefs::from_globals(&g, m, 1.0);
        assert!(!off.underline_hyperlinks && off.solid_underlines);
    }

    #[test]
    fn strikes_are_trimmed_like_underlines() {
        let q = |x: f32, w: f32| Quad { rect: Rect { x, y: 15.0, w, h: 1.0 }, color: [1.0; 4] };
        let mut f = Frame::default();
        f.strikes = vec![q(0.0, 100.0)];
        cut_out(&mut f, Rect { x: 40.0, y: 0.0, w: 20.0, h: 40.0 }, 20.0);
        let xs: Vec<(f32, f32)> = f.strikes.iter().map(|q| (q.rect.x, q.rect.w)).collect();
        assert_eq!(xs, vec![(0.0, 40.0), (60.0, 40.0)]);
        let mut layer = Frame::default();
        layer.strikes.push(q(0.0, 10.0));
        lay_over(&mut f, layer, Rect { x: 0.0, y: 100.0, w: 10.0, h: 10.0 }, 1.0, rgb(0, 0, 0), 16.0);
        assert_eq!(f.strikes.len(), 3, "the bar's own strike comes");
    }

    #[test]
    fn lay_over_clips_underlines() {
        let bar = Rect { x: 0.0, y: 0.0, w: 100.0, h: 20.0 };
        let mut f = Frame::default();
        f.underlines.push(Quad { rect: Rect { x: 0.0, y: 10.0, w: 50.0, h: 1.0 }, color: [1.0; 4] });
        f.underlines.push(Quad { rect: Rect { x: 0.0, y: 30.0, w: 50.0, h: 1.0 }, color: [1.0; 4] });
        let mut layer = Frame::default();
        layer.underlines.push(Quad { rect: Rect { x: 0.0, y: 15.0, w: 10.0, h: 1.0 }, color: [1.0, 1.0, 1.0, 1.0] });
        lay_over(&mut f, layer, bar, 1.0, rgb(0, 0, 0), 16.0);
        let ys: Vec<f32> = f.underlines.iter().map(|q| q.rect.y).collect();
        assert_eq!(ys, vec![30.0, 15.0], "the sessions' underline under the bar goes, the bar's own comes");
    }

    #[test]
    fn hover_underlines_in_the_link_color() {
        let s = screen(b"see x.rs");
        let mut f = Frame::default();
        let link = rgb(0x06, 0x45, 0xad);
        let hover = crate::find::Match { start: crate::find::Pos { row: 0, col: 4 }, end: crate::find::Pos { row: 0, col: 7 } };
        let hl = Highlights { matches: &[], color: rgb(0xff, 0xff, 0), hover: Some(hover), link, labels: &[], notes: &[], dim: crate::dim::Dim::LEGACY, key_cursor: false, bg_alpha: 1.0, opaque_colors: false, text_blink_on: true };
        pane_with(&mut f, &s, RECT, CELL, true, &CursorLook::shown(false), &DrawPrefs::default(), Some(&hl), crate::render::DEFAULT_FACE);
        let q = f.overlay.iter().find(|q| q.color == rgba(link, 1.0)).unwrap();
        assert_eq!(q.rect, Rect { x: 40.0, y: CELL.1 - 1.0, w: 40.0, h: 1.0 });
        let x = f.rows.iter().flat_map(|r| &r.spans).find(|sp| sp.text.starts_with("x.rs")).unwrap();
        assert_eq!(x.color, link);
    }

    /// The row "ab " + aleph + bet draws the aleph (logical column 3) at visual column 4 and the
    /// bet (logical 4) at visual column 3. Everything placed by a logical column follows.
    fn hl_with<'a>(matches: &'a [crate::find::Match], labels: &'a [(crate::find::Pos, String)], notes: &'a [(crate::find::Match, Option<String>)]) -> Highlights<'a> {
        Highlights { matches, color: rgb(0xff, 0xff, 0), hover: None, link: rgb(0, 0, 0xff), labels, notes, dim: crate::dim::Dim::LEGACY, key_cursor: false, bg_alpha: 1.0, opaque_colors: false, text_blink_on: true }
    }

    fn at(col: usize) -> crate::find::Pos {
        crate::find::Pos { row: 0, col }
    }

    #[test]
    fn bidi_find_and_selection_follow_the_order() {
        let mut s = screen("ab \u{5d0}\u{5d1}".as_bytes());
        // A find match over logical 3..4 is one run at visual 3..4.
        let m = [crate::find::Match { start: at(3), end: at(4) }];
        let mut f = Frame::default();
        pane_with(&mut f, &s, RECT, CELL, false, &CursorLook::shown(false), &DrawPrefs::default(), Some(&hl_with(&m, &[], &[])), crate::render::DEFAULT_FACE);
        let yellow: Vec<Rect> = f.quads.iter().filter(|q| q.color == rgba(rgb(0xff, 0xff, 0), 1.0)).map(|q| q.rect).collect();
        assert_eq!(yellow, vec![Rect { x: 30.0, y: 0.0, w: 20.0, h: 20.0 }]);
        // A selection of logical column 3 shades visual column 4.
        let mut sel = Selection::new(SelectionType::Simple, Point::new(Line(0), Column(3)), Side::Left);
        sel.update(Point::new(Line(0), Column(3)), Side::Right);
        s.term_mut().selection = Some(sel);
        let mut f = Frame::default();
        pane(&mut f, &s, RECT, CELL, false, &CursorLook::shown(false));
        let shade: Vec<Rect> = f.quads.iter().filter(|q| q.color == rgba(s.palette.selection, 1.0)).map(|q| q.rect).collect();
        assert_eq!(shade, vec![Rect { x: 40.0, y: 0.0, w: 10.0, h: 20.0 }]);
    }

    #[test]
    fn bidi_labels_notes_and_ime_follow_the_order() {
        let s = screen("ab \u{5d0}\u{5d1}".as_bytes());
        // A two-character label over logical 3..5 sits where its cells are, at visual 3.
        let labels = [(at(3), "12".to_string())];
        let mut f = Frame::default();
        pane_with(&mut f, &s, RECT, CELL, true, &CursorLook::shown(false), &DrawPrefs::default(), Some(&hl_with(&[], &labels, &[])), crate::render::DEFAULT_FACE);
        let tag = f.rows.iter().find(|r| r.spans.iter().any(|s| s.text == "12")).unwrap();
        assert_eq!(tag.x, 30.0);
        // A one-character label over the aleph sits at its visual column 4.
        let labels = [(at(3), "A".to_string())];
        let mut f = Frame::default();
        pane_with(&mut f, &s, RECT, CELL, true, &CursorLook::shown(false), &DrawPrefs::default(), Some(&hl_with(&[], &labels, &[])), crate::render::DEFAULT_FACE);
        assert_eq!(f.rows.iter().find(|r| r.spans.iter().any(|s| s.text == "A")).unwrap().x, 40.0);
        // A note over logical 3 underlines visual 4; over logical 0..3 it is two pieces.
        let notes = [(crate::find::Match { start: at(3), end: at(3) }, None), (crate::find::Match { start: at(0), end: at(3) }, None)];
        let mut f = Frame::default();
        pane_with(&mut f, &s, RECT, CELL, true, &CursorLook::shown(false), &DrawPrefs::default(), Some(&hl_with(&[], &[], &notes)), crate::render::DEFAULT_FACE);
        let y = CELL.1 - 3.0;
        let lines: Vec<(f32, f32)> = f.overlay.iter().filter(|q| q.color == rgba(NOTE, 1.0) && q.rect.y == y).map(|q| (q.rect.x, q.rect.w)).collect();
        assert_eq!(lines, vec![(40.0, 10.0), (0.0, 30.0), (40.0, 10.0)], "{lines:?}");
        // The IME composition over logical 3 draws at visual 4 and its caret at logical 4 is at visual 3.
        use crate::ime::{Drawn, Piece};
        let yellow = rgb(255, 255, 0);
        let mut look = CursorLook::shown(true);
        look.geometry.base_cell = CELL;
        look.ime = Some(Drawn { pieces: vec![Piece { row: 0, col: 3, text: "x".to_string(), cells: 1 }], caret: (0, 4), caret_color: yellow });
        let mut f = Frame::default();
        pane(&mut f, &s, RECT, CELL, true, &look);
        let bg = rgba(s.palette.bg, 1.0);
        assert!(f.popups.iter().any(|q| q.color == bg && q.rect.x == 40.0), "{:?}", f.popups);
        assert!(f.overlay.iter().any(|q| q.color == rgba(yellow, 1.0) && q.rect.x == 30.0), "caret: {:?}", f.overlay);
    }

    #[test]
    fn labels_hide_the_text_under_them() {
        let s = screen(b"see https://x.y now");
        let mut f = Frame::default();
        let labels = [(crate::find::Pos { row: 0, col: 4 }, "1".to_string())];
        let hl = Highlights { matches: &[], color: rgb(0xff, 0xff, 0), hover: None, link: rgb(0, 0, 0xff), labels: &labels, notes: &[], dim: crate::dim::Dim::LEGACY, key_cursor: false, bg_alpha: 1.0, opaque_colors: false, text_blink_on: true };
        pane_with(&mut f, &s, RECT, CELL, true, &CursorLook::shown(false), &DrawPrefs::default(), Some(&hl), crate::render::DEFAULT_FACE);
        let texts: Vec<String> = f.rows.iter().map(|r| r.spans.iter().map(|s| s.text.as_str()).collect()).collect();
        assert!(texts.iter().any(|t| t.starts_with("see  ttps")), "{texts:?}");
        let tag = f.rows.iter().find(|r| r.spans.iter().any(|s| s.text == "1")).unwrap();
        assert_eq!((tag.x, tag.y), (40.0, 0.0));
        assert!(f.quads.iter().any(|q| q.color == rgba(LABEL_BG, 1.0) && q.rect.x == 40.0));
    }

    #[test]
    fn copy_mode_cursor_is_a_flag() {
        let mut f = Frame::default();
        copy_cursor(&mut f, RECT, CELL, 2, 3, false);
        let white = rgba(rgb(0xff, 0xff, 0xff), 1.0);
        let fills: Vec<Rect> = f.overlay.iter().filter(|q| q.color == white).map(|q| q.rect).collect();
        assert!(fills.len() >= 3, "{fills:?}");
        let cell = Rect { x: 30.0, y: 40.0, w: 10.0, h: 20.0 };
        for r in &fills {
            assert!(r.x >= cell.x && r.x + r.w <= cell.x + cell.w + 0.01, "{r:?}");
            assert!(r.y >= cell.y + cell.h * 2.0 / 3.0 - 0.01 && r.y + r.h <= cell.y + cell.h + 0.01, "{r:?}");
        }
        let mut g = Frame::default();
        copy_cursor(&mut g, RECT, CELL, 2, 3, true);
        assert!(g.overlay.iter().any(|q| q.color == rgba(rgb(0xc1, 0xde, 0xff), 1.0)));
    }

    #[test]
    fn annotated_text_is_underlined_in_yellow() {
        let s = screen(b"see x.rs");
        let mut f = Frame::default();
        let m = crate::find::Match { start: crate::find::Pos { row: 0, col: 4 }, end: crate::find::Pos { row: 0, col: 7 } };
        let notes = [(m, Some("a file".to_string()))];
        let hl = Highlights { matches: &[], color: rgb(0xff, 0xff, 0), hover: None, link: rgb(0, 0, 0xff), labels: &[], notes: &notes, dim: crate::dim::Dim::LEGACY, key_cursor: false, bg_alpha: 1.0, opaque_colors: false, text_blink_on: true };
        pane_with(&mut f, &s, RECT, CELL, true, &CursorLook::shown(false), &DrawPrefs::default(), Some(&hl), crate::render::DEFAULT_FACE);
        assert!(f.overlay.iter().any(|q| q.color == rgba(NOTE, 1.0) && q.rect == Rect { x: 40.0, y: CELL.1 - 3.0, w: 40.0, h: 1.0 }), "{:?}", f.overlay);
        assert!(f.rows.iter().any(|r| r.spans.iter().any(|s| s.text == "a file")));
    }

    #[test]
    fn cursor_guide_fills_the_cursor_row() {
        let mut f = Frame::default();
        cursor_guide(&mut f, RECT, CELL, 3, [0.5, 0.9, 1.0, 0.25]);
        let q = f.quads.last().unwrap();
        assert_eq!(q.rect, Rect { x: 0.0, y: 60.0, w: 800.0, h: 20.0 });
        assert_eq!(q.color[3], 0.25);
    }

    #[test]
    fn timestamps_sit_at_the_right() {
        let mut f = Frame::default();
        timestamps(&mut f, &[(2, "14:02:09".into())], RECT, CELL, None, STAMP_COLORS);
        let r = f.rows.iter().find(|r| r.spans.iter().any(|s| s.text == "14:02:09")).unwrap();
        assert_eq!((r.x, r.y), (800.0 - 90.0, 40.0));
        assert!(f.quads.iter().any(|q| q.rect.x < 710.0 && q.rect.x + q.rect.w == 800.0 && q.rect.y == 40.0));
        // The profile's foreground on its background at 0.9 alpha, as in every iTerm2 mode.
        assert_eq!(r.spans[0].color, STAMP_COLORS.0);
        assert!(f.quads.iter().all(|q| q.color == rgba(STAMP_COLORS.1, 0.9)));
    }

    /// A light profile: dark text on a white background.
    const STAMP_COLORS: (Rgb, Rgb) = (rgb(0x10, 0x10, 0x10), rgb(0xff, 0xff, 0xff));

    #[test]
    fn adjacent_labels_follow_iterm2() {
        let col = Rect { x: 700.0, y: 0.0, w: 100.0, h: 200.0 };
        let labels: Vec<(usize, String)> = vec![(2, "14:02:09".into()), (3, "14:02:09".into()), (4, "14:02:10".into())];
        let text_row = |y: f32| TextRow { x: 0.0, y, spans: vec![], clip: Rect { x: 0.0, y, w: 800.0, h: 20.0 }, scale: 1.0, face: crate::render::DEFAULT_FACE };
        let mut f = Frame::default();
        f.rows.push(text_row(40.0));
        timestamps(&mut f, &labels, Rect { x: 0.0, y: 0.0, w: 700.0, h: 200.0 }, CELL, Some(col), STAMP_COLORS);
        // No text row is clipped, and nothing is cut out of the text.
        assert_eq!(f.rows[0].clip.w, 800.0);
        let stamp = |t: &str| f.rows.iter().find(|r| r.spans.iter().any(|s| s.text == t));
        // One block, left aligned at the column's right edge less the widest label and a cell.
        let a = stamp("14:02:09").unwrap();
        assert_eq!((a.x, a.y), (710.0, 40.0));
        assert_eq!(stamp("14:02:10").unwrap().x, 710.0);
        // The row right below with the same text is a repeat: two 1 px lines, no text.
        assert_eq!(f.rows.iter().filter(|r| r.spans.iter().any(|s| s.text == "14:02:09")).count(), 1);
        let lines: Vec<_> = f.quads.iter().filter(|q| q.rect.y == 60.0 && q.rect.w == 1.0 && q.rect.h == CELL.1).map(|q| q.rect.x).collect();
        assert_eq!(lines, vec![719.0, 721.0]);
        // Everything stays inside the column.
        assert!(f.rows.iter().skip(1).all(|r| r.clip.x >= col.x && r.clip.x + r.clip.w <= col.x + col.w));
        assert!(f.quads.iter().all(|q| q.rect.x >= col.x && q.rect.x + q.rect.w <= col.x + col.w));
        // Readable on a light profile: its foreground (the repeat lines too) on a 0.9-alpha block
        // of its background behind every row, as iTerm2 draws every mode.
        assert_eq!(a.spans[0].color, STAMP_COLORS.0);
        let fill = |y: f32| f.quads.iter().find(|q| q.rect.y == y && q.color == rgba(STAMP_COLORS.1, 0.9)).map(|q| (q.rect.x, q.rect.x + q.rect.w));
        assert_eq!([fill(40.0), fill(60.0), fill(80.0)], [Some((710.0, 800.0)); 3]);
        assert!(f.quads.iter().filter(|q| q.rect.w == 1.0).all(|q| q.color == rgba(STAMP_COLORS.0, 1.0)));
    }

    /// iTerm2 never cuts a label: a block wider than the column starts further left, over the
    /// text (`x = MAX(0, maxX - w)`), and the text under it stops where it starts.
    #[test]
    fn adjacent_labels_wider_than_the_column_extend_over_the_text() {
        let col = Rect { x: 700.0, y: 0.0, w: 40.0, h: 100.0 };
        let text = Rect { x: 0.0, y: 0.0, w: 700.0, h: 100.0 };
        let text_row = |y: f32| TextRow { x: 0.0, y, spans: vec![], clip: Rect { x: 0.0, y, w: 700.0, h: 20.0 }, scale: 1.0, face: crate::render::DEFAULT_FACE };
        let mut f = Frame::default();
        f.rows.push(text_row(0.0));
        f.rows.push(text_row(20.0));
        timestamps(&mut f, &[(0, "2026 Mar 7 14:02:09".into())], text, CELL, Some(col), STAMP_COLORS);
        // 19 characters and a cell of margin: 200 px, ending at the column's right edge.
        let label = &f.rows[2];
        assert_eq!(label.x, 540.0);
        assert!(label.clip.x <= 540.0 && label.clip.x + label.clip.w == 740.0, "{:?}", label.clip);
        assert_eq!(f.rows[0].clip.w, 540.0, "the text under the label stops where it starts");
        assert_eq!(f.rows[1].clip.w, 700.0, "a row without a label keeps its text");
        assert!(f.quads.iter().any(|q| q.rect == Rect { x: 540.0, y: 0.0, w: 200.0, h: 20.0 } && q.color == rgba(STAMP_COLORS.1, 0.9)));
        // A block wider than the whole pane starts at the text's left edge.
        let mut f = Frame::default();
        timestamps(&mut f, &[(0, "2026 Mar 7 14:02:09".into())], Rect { w: 100.0, ..text }, CELL, Some(Rect { x: 100.0, ..col }), STAMP_COLORS);
        assert_eq!(f.rows[0].x, 0.0);
    }

    #[test]
    fn find_cursor_leaves_a_hole() {
        let mut f = Frame::default();
        let area = Rect { x: 0.0, y: 0.0, w: 400.0, h: 300.0 };
        find_cursor(&mut f, area, (200.0, 150.0), 30.0);
        let covers = |x: f32, y: f32| f.overlay.iter().any(|q| x >= q.rect.x && x < q.rect.x + q.rect.w && y >= q.rect.y && y < q.rect.y + q.rect.h);
        assert!(!covers(200.0, 150.0));
        assert!(!covers(220.0, 150.0));
        assert!(covers(5.0, 5.0));
        assert!(covers(240.0, 150.0));
        assert!(covers(200.0, 190.0));
        assert!(f.overlay.iter().all(|q| q.color[3] > 0.5 && q.color[3] < 0.8));
    }

    #[test]
    fn notes_stay_inside_the_pane() {
        let s = Screen::new(10, 4);
        let mut f = Frame::default();
        let m = crate::find::Match { start: crate::find::Pos { row: 0, col: 16 }, end: crate::find::Pos { row: 1_000_000, col: 19 } };
        let notes = [(m, Some("n".to_string()))];
        let rect = Rect { x: 0.0, y: 0.0, w: 100.0, h: 80.0 };
        let hl = Highlights { matches: &[], color: rgb(0xff, 0xff, 0), hover: None, link: rgb(0, 0, 0xff), labels: &[], notes: &notes, dim: crate::dim::Dim::LEGACY, key_cursor: false, bg_alpha: 1.0, opaque_colors: false, text_blink_on: true };
        pane_with(&mut f, &s, rect, CELL, true, &CursorLook::shown(false), &DrawPrefs::default(), Some(&hl), crate::render::DEFAULT_FACE);
        assert!(f.overlay.iter().all(|q| q.rect.x + q.rect.w <= rect.w + 0.01 && q.rect.y + q.rect.h <= rect.h + 0.01), "{:?}", f.overlay);
        assert!(f.overlay.len() < 50);
    }

    #[test]
    fn tiles_become_textured_quads() {
        let mut s = screen(b"");
        let img = crate::images::Image {
            width_px: 20,
            height_px: 20,
            cols: 2,
            rows: 1,
            frames: vec![crate::images::Frame { rgba: std::sync::Arc::new(vec![9; 20 * 20 * 4]), delay_ms: 0 }],
            broken: false,
            keep_aspect: false,
            uid: 0,
        };
        s.place_image(img, true);
        let mut f = Frame::default();
        pane(&mut f, &s, RECT, CELL, true, &CursorLook::shown(false));
        assert_eq!(f.images.len(), 2);
        assert_eq!(f.images[0].rect, Rect { x: 0.0, y: 0.0, w: 10.0, h: 20.0 });
        assert_eq!(f.images[0].uv, [0.0, 0.0, 0.5, 1.0]);
        assert_eq!(f.images[1].uv, [0.5, 0.0, 1.0, 1.0]);
        assert_eq!(f.images[0].size, (20, 20));
    }

    #[test]
    fn kept_shape_letterboxes_the_picture() {
        // A 2:1 picture in a 2x1 cell box of 20x20 px sits in a 20x10 band in the middle.
        let img = crate::images::Image {
            width_px: 20,
            height_px: 10,
            cols: 2,
            rows: 1,
            frames: vec![crate::images::Frame { rgba: std::sync::Arc::new(vec![0; 800]), delay_ms: 0 }],
            broken: false,
            keep_aspect: true,
            uid: 7,
        };
        let t = alacritty_terminal::term::cell::Tile { image: 1, x: 1, y: 0 };
        let q = tile_quad(&img, t, Rect { x: 110.0, y: 40.0, w: 10.0, h: 20.0 }, 0).unwrap();
        assert_eq!(q.rect, Rect { x: 110.0, y: 45.0, w: 10.0, h: 10.0 });
        assert_eq!(q.uv, [0.5, 0.0, 1.0, 1.0]);
        assert_eq!(q.key, (7, 0));
    }

    #[test]
    fn animation_frames_follow_the_clock() {
        let f = |d| crate::images::Frame { rgba: std::sync::Arc::new(vec![]), delay_ms: d };
        let frames = [f(100), f(50)];
        assert_eq!(frame_at(&frames, 0), 0);
        assert_eq!(frame_at(&frames, 120), 1);
        assert_eq!(frame_at(&frames, 160), 0);
        assert_eq!(frame_at(&[f(0)], 999), 0);
    }

    #[test]
    fn animation_wakes_at_the_next_frame() {
        let f = |d| crate::images::Frame { rgba: std::sync::Arc::new(vec![]), delay_ms: d };
        assert_eq!(ms_to_next_frame(&[f(100), f(50)], 30), Some(70));
        assert_eq!(ms_to_next_frame(&[f(100), f(50)], 120), Some(30));
        assert_eq!(ms_to_next_frame(&[f(100)], 30), None);
        assert_eq!(ms_to_next_frame(&[f(0), f(0)], 30), None);
    }

    #[test]
    fn indicators_sit_top_right() {
        let mut f = Frame::default();
        let pane = Rect { x: 100.0, y: 50.0, w: 400.0, h: 300.0 };
        top_right_indicators(&mut f, pane, &[COPROCESS_GLYPH], rgb(1, 2, 3), (8.0, 16.0));
        assert_eq!(f.rows.len(), 1);
        let r = &f.rows[0];
        assert!(r.spans[0].icon && r.spans[0].text == COPROCESS_GLYPH.to_string());
        assert!(r.x > pane.x + pane.w - 40.0 && r.x < pane.x + pane.w && (r.y - pane.y).abs() < 8.0, "{} {}", r.x, r.y);
        let mut none = Frame::default();
        top_right_indicators(&mut none, pane, &[], rgb(1, 2, 3), (8.0, 16.0));
        assert!(none.rows.is_empty());
    }

    #[test]
    fn a_background_window_keeps_a_hollow_cursor() {
        let s = screen(b"ab");
        let hl = Highlights { matches: &[], color: rgb(0, 0, 0), hover: None, link: rgb(0, 0, 0), labels: &[], notes: &[], dim: crate::dim::Dim::NONE, key_cursor: false, bg_alpha: 1.0, opaque_colors: false, text_blink_on: true };
        let mut f = Frame::default();
        pane_with(&mut f, &s, RECT, CELL, false, &CursorLook::shown(true), &DrawPrefs::default(), Some(&hl), crate::render::DEFAULT_FACE);
        assert!(!f.overlay.iter().any(|q| q.rect == RECT), "the key pane of a background window keeps its brightness");
        assert!(!f.quads.iter().any(|q| q.color == rgba(s.palette.cursor, 1.0)), "its cursor is hollow");
    }

    #[test]
    fn dim_quads_follow_the_dim() {
        let s = screen(b"ab");
        let hl = |dim| Highlights { matches: &[], color: rgb(0, 0, 0), hover: None, link: rgb(0, 0, 0), labels: &[], notes: &[], dim, key_cursor: false, bg_alpha: 1.0, opaque_colors: false, text_blink_on: true };
        let whole = |f: &Frame| f.overlay.iter().any(|q| q.rect == RECT);
        // The render loop lays the gray quad over the whole pane; pane_with draws none with highlights.
        let mut f = Frame::default();
        pane_with(&mut f, &s, RECT, CELL, false, &CursorLook::shown(true), &DrawPrefs::default(), Some(&hl(crate::dim::Dim::NONE)), crate::render::DEFAULT_FACE);
        assert!(!whole(&f));
        let mut f = Frame::default();
        pane_with(&mut f, &s, RECT, CELL, false, &CursorLook::shown(true), &DrawPrefs::default(), Some(&hl(crate::dim::Dim { amount: 0.4, only_text: false })), crate::render::DEFAULT_FACE);
        assert!(!whole(&f), "the app draws the gray quad, not pane_with");
        // Dim Only Text: no quad, the text itself carries the dimmed color.
        let first = |f: &Frame| f.rows.iter().flat_map(|r| r.spans.iter()).find(|sp| sp.text.contains('a')).map(|sp| sp.color);
        let mut plain = Frame::default();
        pane_with(&mut plain, &s, RECT, CELL, false, &CursorLook::shown(true), &DrawPrefs::default(), Some(&hl(crate::dim::Dim::NONE)), crate::render::DEFAULT_FACE);
        let only_text = crate::dim::Dim { amount: 0.4, only_text: true };
        let mut f = Frame::default();
        pane_with(&mut f, &s, RECT, CELL, false, &CursorLook::shown(true), &DrawPrefs::default(), Some(&hl(only_text)), crate::render::DEFAULT_FACE);
        assert!(!whole(&f));
        let fg = s.palette.fg;
        assert_eq!(first(&plain), Some(fg));
        assert_eq!(first(&f), Some(crate::dim::text_color(fg, only_text, crate::tab_theme::perceived(s.palette.bg))));
        assert_ne!(first(&f), first(&plain));
        // No highlights and not focused: today's quad (the screenshot path).
        let mut g = Frame::default();
        pane_with(&mut g, &s, RECT, CELL, false, &CursorLook::shown(true), &DrawPrefs::default(), None, crate::render::DEFAULT_FACE);
        assert!(g.overlay.iter().any(|q| q.rect == RECT && q.color == rgba(s.palette.bg, 0.4)));
        let mut g = Frame::default();
        pane_with(&mut g, &s, RECT, CELL, true, &CursorLook::shown(true), &DrawPrefs::default(), None, crate::render::DEFAULT_FACE);
        assert!(!whole(&g));
    }

    #[test]
    fn a_password_prompt_draws_the_key_for_the_cursor() {
        let key = crate::pwm_prompt::KEY_GLYPH.to_string();
        let hl = |key_cursor| Highlights { matches: &[], color: rgb(0, 0, 0), hover: None, link: rgb(0, 0, 0), labels: &[], notes: &[], dim: crate::dim::Dim::LEGACY, key_cursor, bg_alpha: 1.0, opaque_colors: false, text_blink_on: true };
        let key_row = |f: &Frame| f.rows.iter().find(|r| r.spans.iter().any(|s| s.icon && s.text == key)).cloned();
        let mut s = screen(b"ab");
        s.palette.bg = rgb(0, 0, 0);
        let mut f = Frame::default();
        pane_with(&mut f, &s, RECT, CELL, true, &CursorLook::shown(true), &DrawPrefs::default(), Some(&hl(true)), crate::render::DEFAULT_FACE);
        let row = key_row(&f).expect("the key is drawn");
        assert_eq!((row.clip.x, row.clip.y), (20.0, 0.0), "in the cursor's cell");
        assert_eq!(row.spans[0].color, rgb(0xff, 0xff, 0xff), "light on a dark background");
        assert!(!f.quads.iter().chain(&f.overlay).any(|q| q.color == rgba(s.palette.cursor, 1.0)), "no cursor shape");
        s.palette.bg = rgb(0xff, 0xff, 0xff);
        let mut light = Frame::default();
        pane_with(&mut light, &s, RECT, CELL, true, &CursorLook::shown(true), &DrawPrefs::default(), Some(&hl(true)), crate::render::DEFAULT_FACE);
        assert_eq!(key_row(&light).map(|r| r.spans[0].color), Some(rgb(0, 0, 0)), "dark on a light background");
        let mut blink = Frame::default();
        pane_with(&mut blink, &s, RECT, CELL, true, &CursorLook::shown(false), &DrawPrefs::default(), Some(&hl(true)), crate::render::DEFAULT_FACE);
        assert!(key_row(&blink).is_none(), "it blinks with the cursor");
        // Over the cell: the character under the cursor is still drawn.
        let on_c = screen(b"abc\x1b[D");
        let mut over = Frame::default();
        pane_with(&mut over, &on_c, RECT, CELL, true, &CursorLook::shown(true), &DrawPrefs::default(), Some(&hl(true)), crate::render::DEFAULT_FACE);
        assert!(key_row(&over).is_some_and(|r| r.clip.x == 20.0));
        assert_eq!(row_text(&over, 0).trim_end(), "abc", "the key does not blank the cell");
        let s = screen(b"ab");
        let mut plain = Frame::default();
        pane_with(&mut plain, &s, RECT, CELL, true, &CursorLook::shown(true), &DrawPrefs::default(), Some(&hl(false)), crate::render::DEFAULT_FACE);
        assert!(key_row(&plain).is_none());
        assert!(plain.quads.iter().any(|q| q.color == rgba(s.palette.cursor, 1.0)), "the cursor otherwise");
    }
    #[test]
    fn offscreen_band_draws_the_command() {
        let mut s = Screen::new(20, 4);
        s.feed(b"ls -R\r\nnext");
        let mut f = Frame::default();
        let pane = Rect { x: 0.0, y: 0.0, w: 160.0, h: 64.0 };
        pane_with(&mut f, &s, pane, CELL, false, &CursorLook::shown(false), &DrawPrefs::default(), None, crate::render::DEFAULT_FACE);
        let (before, overlay) = (f.rows.len(), f.overlay.len());
        let band = Rect { x: 0.0, y: 0.0, w: 160.0, h: 32.0 };
        let colors = ([0.105, 0.105, 0.105, 0.5], [0.5, 0.5, 0.5, 1.0]);
        offscreen_band(&mut f, &s, band, (0.0, 8.0), CELL, 0, colors, 3);
        let lin = |c: [f32; 4]| [srgb_to_linear((c[0] * 255.0).round() as u8), srgb_to_linear((c[1] * 255.0).round() as u8), srgb_to_linear((c[2] * 255.0).round() as u8), c[3]];
        assert!(f.popups.iter().any(|q| q.rect == band && q.color == lin(colors.0)), "{:?}", f.popups);
        assert!(f.popups.iter().any(|q| q.rect == Rect { h: 1.0, ..band } && q.color == lin(colors.1)));
        assert!(f.popups.iter().any(|q| q.rect == Rect { y: 31.0, h: 1.0, ..band } && q.color == lin(colors.1)));
        assert!(f.overlay.len() == overlay && f.quads.iter().all(|q| q.rect != band), "the box is a popup");
        let row = f.rows.last().unwrap();
        assert!(f.rows.len() > before);
        assert_eq!(row.spans.iter().map(|s| s.text.as_str()).collect::<String>(), "ls -R");
        assert_eq!((row.clip, row.face, row.y), (band, 3, 8.0));
    }

    #[test]
    fn offscreen_band_covers_row_zero() {
        // Row 0 is underlined, struck and box drawn; nothing of it shows inside the band.
        let mut s = Screen::new(20, 4);
        s.feed("\x1b[4;9m\u{2500}\u{2502}abc\x1b[0m\r\n\x1b[4mxyz".as_bytes());
        let mut f = Frame::default();
        let pane = Rect { x: 0.0, y: 0.0, w: 160.0, h: 64.0 };
        pane_with(&mut f, &s, pane, CELL, false, &CursorLook::shown(false), &DrawPrefs::default(), None, crate::render::DEFAULT_FACE);
        assert!(!f.glyphs.is_empty() && !f.underlines.is_empty() && !f.strikes.is_empty(), "the fixture draws all three");
        let band = Rect { x: 0.0, y: 0.0, w: 160.0, h: 32.0 };
        let under = f.rows.len();
        offscreen_band(&mut f, &s, band, (0.0, 8.0), CELL, 0, ([0.1, 0.1, 0.1, 0.5], [0.5, 0.5, 0.5, 1.0]), crate::render::DEFAULT_FACE);
        let hit = |r: Rect| r.x < band.x + band.w && r.x + r.w > band.x && r.y < band.y + band.h && r.y + r.h > band.y;
        assert!(f.underlines.iter().chain(&f.strikes).all(|q| !hit(q.rect)), "strokes: {:?}", f.underlines);
        assert!(f.glyphs.iter().all(|g| {
            let c = g.clip;
            !(hit(g.rect) && c.w > 0.0 && c.h > 0.0 && hit(c))
        }), "glyphs: {:?}", f.glyphs);
        assert!(f.rows.iter().enumerate().all(|(i, r)| i >= under || r.clip.w <= 0.0 || !hit(Rect { x: r.clip.x, y: r.y, w: r.clip.w, h: 16.0 })), "rows: {:?}", f.rows.iter().map(|r| (r.y, r.clip, r.spans.iter().map(|s| s.text.clone()).collect::<String>())).collect::<Vec<_>>());
    }

    #[test]
    fn picker_hits_follow_iterm2() {
        let cell = (10.0, 20.0);
        let (fg, bg) = (rgb(0xee, 0xee, 0xee), rgb(0x10, 0x10, 0x10));
        let pane = Rect { x: 10.0, y: 20.0, w: 400.0, h: 300.0 };
        let mut f = Frame::default();
        let r = session_picker_button(&mut f, pane, "Link this session to AI chat?", fg, bg, cell, 1.0);
        assert!(((r.x + r.w / 2.0) - (pane.x + pane.w / 2.0)).abs() < 1.0, "centered across");
        assert!(((r.y + r.h / 2.0) - (pane.y + pane.h / 2.0)).abs() < 1.0, "centered down");
        let label_w = "Link this session to AI chat?".chars().count() as f32 * cell.0;
        assert!(r.w > label_w, "wider than the label: {} vs {label_w}", r.w);
        assert!(r.w < pane.w, "and narrower than this pane");
        assert!(r.h >= cell.1, "tall enough for a row");
        assert!(f.popups.iter().any(|q| q.rect == r), "the box is a popup, drawn under the text");
        assert!(f.overlay.is_empty(), "nothing goes in the overlay, which draws over text");
        assert_eq!(f.rows.len(), 1, "one label");
        let row = &f.rows[0];
        assert_eq!(row.spans.iter().map(|s| s.text.as_str()).collect::<String>(), "Link this session to AI chat?");
        assert!(row.x >= r.x && row.x + label_w <= r.x + r.w, "the label sits inside its box");
        // A pane narrower than the label: the box never spills over.
        let narrow = Rect { x: 0.0, y: 0.0, w: 120.0, h: 60.0 };
        let mut g = Frame::default();
        let r = session_picker_button(&mut g, narrow, "Link this session to AI chat?", fg, bg, cell, 1.0);
        assert!(r.w <= narrow.w && r.x >= narrow.x && r.x + r.w <= narrow.x + narrow.w, "kept inside the pane");
        assert!(r.h <= narrow.h);
    }

    /// A fixed markdown document laid out with a fixed measure, for the extraction test.
    fn sample_lines() -> Vec<crate::porthole::render::VisualLine> {
        use crate::porthole::{detect::Language, render};
        let md = "# Title\n\nSome *prose* with a [link](https://x.example/a) that wraps over more than one line of the narrow text.\n\n```rust\nfn main() {}\n```\n\n---\n\n- one\n- two\n";
        let doc = render::render(md, &Language::Markdown, &crate::palette::Palette::default(), true);
        let measure = |t: &str, _f: render::Face, s: f32| t.chars().count() as f32 * 8.0 * s;
        render::layout(&doc, 220.0, 16.0, &measure, false).lines
    }

    fn digest(f: &Frame) -> u64 {
        // FNV-1a over the frame's quads and rows.
        // The rows' `face` and the spans' `fake_bold` fields are 13n's; the digest is of the fields
        // the porthole drew before them.
        let text = format!("{:?}{:?}", f.quads, f.rows).replace(", face: 0", "").replace(", fake_bold: false", "").replace(", ligatures: 0", "").replace(", mask: 0", "").replace(", font: 0", "");
        text.bytes().fold(0xcbf29ce484222325u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x100000001b3))
    }

    #[test]
    fn doc_lines_draw_as_the_porthole_did() {
        let lines = sample_lines();
        let rect = Rect { x: 10.0, y: 20.0, w: 260.0, h: 300.0 };
        let chrome = PortholeChrome { fg: rgb(200, 200, 200), bg: rgb(20, 20, 20), language: "Markdown", wide: false, scroll_x: 0.0 };
        let marks = [(Rect { x: 8.0, y: 0.0, w: 40.0, h: 16.0 }, rgb(255, 230, 0), 0.5)];
        let mut whole = Frame::default();
        porthole(&mut whole, rect, rect, &lines, &marks, &chrome, (8.0, 16.0));
        // Before the extraction this exact frame came out of `porthole` alone; its digest was taken
        // from that code and the extraction must keep it.
        assert_eq!((whole.quads.len(), whole.rows.len(), digest(&whole)), GOLDEN, "the porthole's frame changed");
        // The same frame in two steps: the chrome with no lines, then the lines by `doc_lines`.
        let mut parts = Frame::default();
        porthole(&mut parts, rect, rect, &[], &[], &chrome, (8.0, 16.0));
        let inner = Rect { x: rect.x + 4.0, y: rect.y + 4.0 + 16.0, w: rect.w - 8.0, h: rect.h - 8.0 - 16.0 };
        let back = blend(chrome.bg, rgb(128, 128, 128), 0.2);
        doc_lines(&mut parts, (rect.x + PORTHOLE_MARGIN, rect.y + PORTHOLE_MARGIN + 16.0), &lines, &marks, inner, (inner.x, inner.w), back, chrome.fg);
        assert_eq!(parts.quads, whole.quads);
        assert_eq!(parts.rows, whole.rows);
    }
}

#[cfg(test)]
mod title_bar_tests {
    use super::*;
    use crate::hexcolor::white;
    use crate::pane_title::{self, Look};

    const CELL: (f32, f32) = (8.0, 16.0);

    fn dark_active() -> Look {
        Look { background: white(0.27), background_alpha: 1.0, text: (white(0.8), 1.0), menu: (white(0.8), 0.55), line: Some((white(1.0), 0.25)) }
    }

    /// P11: a see-through Minimal bar's background is the session color at the pane's alpha,
    /// premultiplied like the pane's own margins (`transparency::premultiplied`); the label stays
    /// pre-blended into the solid color.
    #[test]
    fn title_bar_background_carries_its_alpha() {
        let mut b = bar("bash", true);
        b.look.background = crate::palette::rgb(10, 20, 30);
        b.look.background_alpha = 0.6;
        b.look.line = None;
        let mut f = Frame::default();
        pane_title_bar(&mut f, &b, CELL);
        assert_eq!(f.quads[0].color, crate::transparency::premultiplied([10, 20, 30], 0.6));
        let label = f.rows.iter().find(|r| r.spans.iter().all(|s| !s.icon)).unwrap();
        assert_eq!(label.spans[0].color, blend(crate::palette::rgb(10, 20, 30), white(0.8), 1.0));
    }

    fn bar<'a>(text: &'a str, show_label: bool) -> TitleBar<'a> {
        let g = pane_title::geometry(Rect { x: 0.0, y: 0.0, w: 400.0, h: 22.0 }, 1.0, false);
        TitleBar { geometry: g, look: dark_active(), text, show_label }
    }

    fn texts(f: &Frame) -> Vec<String> {
        f.rows.iter().map(|r| r.spans.iter().map(|s| s.text.as_str()).collect()).collect()
    }

    #[test]
    fn title_bars_draw_as_iterm2() {
        let mut f = Frame::default();
        pane_title_bar(&mut f, &bar("bash", true), CELL);
        assert_eq!(f.quads[0], Quad { rect: Rect { x: 0.0, y: 0.0, w: 400.0, h: 22.0 }, color: rgba(white(0.27), 1.0) });
        assert_eq!(f.quads[1], Quad { rect: Rect { x: 0.0, y: 21.0, w: 400.0, h: 1.0 }, color: rgba(white(1.0), 0.25) });
        assert!(f.overlay.is_empty() && f.popups.is_empty(), "nothing of the title bar goes in the overlay");
        let label: Vec<&TextRow> = f.rows.iter().filter(|r| r.spans.iter().all(|s| !s.icon)).collect();
        assert_eq!(label.len(), 1);
        assert_eq!(label[0].x, 27.0);
        assert_eq!(label[0].spans[0].text, "bash");
        assert_eq!(label[0].spans[0].color, white(0.8));
        let icons: Vec<String> = f.rows.iter().filter(|r| r.spans[0].icon).map(|r| r.spans[0].text.clone()).collect();
        assert_eq!(icons, vec!["\u{E711}".to_string(), "\u{E712}".to_string()]);
        let menu = f.rows.iter().find(|r| r.spans[0].text == "\u{E712}").unwrap();
        assert_eq!(menu.spans[0].color, crate::scene::blend(white(0.27), white(0.8), 0.55), "the menu glyph is pre-blended");
        for r in &f.rows {
            assert!(r.clip.x >= 0.0 && r.clip.x + r.clip.w <= 400.0 && r.clip.y >= 0.0 && r.clip.y + r.clip.h <= 22.0, "rows are clipped to the bar");
        }
        // Vertically centered in the bar above its line.
        assert_eq!(label[0].y, ((21.0 - CELL.1) / 2.0).floor());

        let mut f = Frame::default();
        pane_title_bar(&mut f, &bar("bash", false), CELL);
        assert!(f.rows.iter().all(|r| r.spans[0].icon), "no title row without the label");
        assert_eq!(f.rows.len(), 2);
        // A bar in the title bar: no line in Minimal.
        let mut b = bar("x", true);
        b.look.line = None;
        let mut f = Frame::default();
        pane_title_bar(&mut f, &b, CELL);
        assert_eq!(f.quads.len(), 1);
        // The lock mark.
        let g = pane_title::geometry(Rect { x: 0.0, y: 0.0, w: 400.0, h: 22.0 }, 1.0, true);
        let mut f = Frame::default();
        pane_title_bar(&mut f, &TitleBar { geometry: g, ..bar("x", true) }, CELL);
        assert!(texts(&f).contains(&"\u{E72E}".to_string()));
    }

    #[test]
    fn title_labels_fit_by_display_width() {
        let cells = (346.0f32 / CELL.0) as usize;
        let long = "x".repeat(500);
        let mut f = Frame::default();
        pane_title_bar(&mut f, &bar(&long, true), CELL);
        let label = f.rows.iter().find(|r| !r.spans[0].icon).unwrap().spans[0].text.clone();
        assert_eq!(label.chars().count(), cells, "cut to the label width in cells");
        assert!(label.ends_with('\u{2026}'));
        // Wide CJK: two cells each, never past the width.
        let cjk = "\u{4e2d}".repeat(100);
        let fit = pane_title::fit_label(&cjk, cells);
        assert!(unicode_width::UnicodeWidthStr::width(fit.as_str()) <= cells);
        assert!(fit.ends_with('\u{2026}') && fit.chars().filter(|&c| c == '\u{4e2d}').count() == (cells - 1) / 2);
        // A grapheme cluster (e + combining acute, a flag) is never split.
        let combining = format!("{}e\u{301}", "a".repeat(cells - 2));
        assert_eq!(pane_title::fit_label(&combining, cells), combining, "fits exactly");
        let over = format!("{}e\u{301}zz", "a".repeat(cells - 2));
        assert_eq!(pane_title::fit_label(&over, cells), format!("{}e\u{301}\u{2026}", "a".repeat(cells - 2)), "the accent stays with its letter");
        assert_eq!(pane_title::fit_label(&over, cells - 1), format!("{}\u{2026}", "a".repeat(cells - 2)), "the whole cluster goes, never its accent alone");
        let flags = "\u{1f1fa}\u{1f1f8}".repeat(40);
        let fit = pane_title::fit_label(&flags, 9);
        assert_eq!(fit.chars().filter(|c| ('\u{1f1e6}'..='\u{1f1ff}').contains(c)).count() % 2, 0, "a flag is two indicators or none");
        // Short text is untouched, nothing fits in no room.
        assert_eq!(pane_title::fit_label("bash", cells), "bash");
        assert_eq!(pane_title::fit_label("bash", 0), "");
        assert_eq!(pane_title::fit_label("bash", 1), "\u{2026}");
    }

    #[test]
    fn hostile_titles_draw_as_one_inert_line() {
        let hostile = format!("a\u{202e}b\u{202d}c\u{2066}\u{7}\r\nd\u{200b}e{}", "\u{4e2d}".repeat(10_000));
        for text in [hostile.as_str(), "\u{202e}only rtl", "ok\u{202a}"] {
            let mut f = Frame::default();
            pane_title_bar(&mut f, &bar(text, true), CELL);
            for r in &f.rows {
                let t = &r.spans[0].text;
                assert!(t.chars().all(|c| !c.is_control() && !matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' | '\u{200b}'..='\u{200f}')), "{t:?}");
                assert!(unicode_width::UnicodeWidthStr::width(t.as_str()) <= 43 || r.spans[0].icon);
            }
        }
        let mut f = Frame::default();
        pane_title_bar(&mut f, &bar("a\u{202e}b\r\nc", true), CELL);
        assert_eq!(f.rows.iter().find(|r| !r.spans[0].icon).unwrap().spans[0].text, "abc");
    }
}

#[cfg(test)]
mod drawn_overlay_tests {
    use super::*;

    const CELL: (f32, f32) = (8.0, 16.0);
    const AREA: Rect = Rect { x: 0.0, y: 0.0, w: 800.0, h: 600.0 };

    /// The rect a draw returns is the one the hit test reads (13j round 3): one per kind.
    #[test]
    fn toast_hit_rect_is_the_drawn_rect() {
        let mut f = Frame::default();
        assert_eq!(toast(&mut f, "saved", AREA, CELL), toast_rect("saved", AREA, CELL));
        assert_eq!(f.quads.last().unwrap().rect, toast_rect("saved", AREA, CELL), "and it is the quad painted");
    }

    #[test]
    fn banner_hit_rect_is_the_drawn_rect() {
        let mut f = Frame::default();
        assert_eq!(banner(&mut f, "oops", AREA, CELL), banner_rect(AREA, CELL));
        assert_eq!(f.quads.last().unwrap().rect, banner_rect(AREA, CELL));
    }

    #[test]
    fn completion_hit_rect_is_the_drawn_rect() {
        let rows = vec!["ls".to_string(), "lsof".to_string()];
        let mut f = Frame::default();
        let drawn = completion_list(&mut f, &rows, 0, None, (10.0, 20.0), AREA, CELL);
        assert_eq!(drawn, completion_rect(&rows, 0, None, (10.0, 20.0), AREA, CELL).map(|c| c.0));
        assert_eq!(f.popups.first().map(|q| q.rect), drawn, "the box painted");
        assert_eq!(completion_list(&mut f, &[], 0, None, (0.0, 0.0), AREA, CELL), None, "nothing drawn, nothing held");
    }

    #[test]
    fn info_bar_hit_rect_is_the_drawn_rect() {
        let bar = crate::infobar::Bar { id: "x".into(), text: "hi".into(), buttons: vec!["_Yes".into()], important: true };
        let l = crate::infobar::layout(&bar, AREA, CELL);
        let mut f = Frame::default();
        assert_eq!(info_bar(&mut f, &bar, &l, CELL), l.bar);
        assert_eq!(f.quads[0].rect, l.bar);
    }

    #[test]
    fn composer_hit_rect_is_the_drawn_rect() {
        let panel = Rect { x: 10.0, y: 400.0, w: 500.0, h: 120.0 };
        let z = Rect { x: 0.0, y: 0.0, w: 0.0, h: 0.0 };
        let d = ComposerDraw { panel, radius: 6.0, alpha: 1.0, bg: rgb(0, 0, 0), under: rgb(0, 0, 0), fg: rgb(255, 255, 255), clip: panel, lines: &[], selection: &[], caret: None, close: z, help: z, ai: None, tip: None, warning: None, tooltip: None, small: 0.8, line_h: 16.0, separator: None, backs: &[] };
        let mut f = Frame::default();
        assert_eq!(composer(&mut f, &d, CELL), Some(panel));
        let faded = ComposerDraw { alpha: 0.0, ..d };
        assert_eq!(composer(&mut f, &faded, CELL), None, "a fully faded composer holds nothing");
    }

    #[test]
    fn composer_popup_hit_rect_is_the_drawn_rect() {
        let rows = vec![("ls".to_string(), None, 'c')];
        let d = ComposerPopupDraw { caret: (20.0, 300.0, 316.0), bounds: AREA, rows: &rows, selected: Some(0), detail: None, placeholder: None, dark: true, scale: 1.0 };
        let mut f = Frame::default();
        let drawn = composer_popup(&mut f, &d, CELL);
        let widest = (2.0 + 3.0) * CELL.0;
        assert_eq!(drawn, composer_popup_frame(d.caret, AREA, 1, Some(0), false, widest, 1.0).0);
    }

    fn bbox(quads: &[Quad]) -> Rect {
        let x0 = quads.iter().map(|q| q.rect.x).fold(f32::MAX, f32::min);
        let y0 = quads.iter().map(|q| q.rect.y).fold(f32::MAX, f32::min);
        let x1 = quads.iter().map(|q| q.rect.x + q.rect.w).fold(f32::MIN, f32::max);
        let y1 = quads.iter().map(|q| q.rect.y + q.rect.h).fold(f32::MIN, f32::max);
        Rect { x: x0, y: y0, w: x1 - x0, h: y1 - y0 }
    }

    #[test]
    fn scrollbar_ticks_draw_outlined_boxes_at_the_minimap_alpha() {
        use crate::scrollbar::{MINIMAP_ALPHA, Series, TICK_HEIGHT, series_colors};
        let track = Rect { x: 100.0, y: 10.0, w: 15.0, h: 203.0 };
        let mut f = Frame::default();
        scrollbar_ticks(&mut f, track, &[(Series::Search, vec![0, 99]), (Series::Error, vec![]), (Series::Manual, vec![50])], 100, 1.0);
        assert!(f.quads.is_empty() && f.rows.is_empty() && f.popups.is_empty(), "ticks hold no text and sit in the overlay");
        assert!(f.scrollbars.iter().all(|q| (q.color[3] - MINIMAP_ALPHA).abs() < 1e-6));
        // Two search ticks and one manual tick: each is four outline edges and one fill.
        assert_eq!(f.scrollbars.len(), 15, "an empty series draws nothing");
        let (outline, fill) = series_colors(Series::Search);
        let first = &f.scrollbars[..5];
        assert_eq!(first.iter().filter(|q| q.color == rgba(outline, MINIMAP_ALPHA)).count(), 4);
        let inner = first.iter().find(|q| q.color == rgba(fill, MINIMAP_ALPHA)).unwrap();
        assert_eq!(inner.rect, Rect { x: 101.0, y: 11.0, w: 13.0, h: TICK_HEIGHT - 2.0 });
        let top = first.iter().map(|q| q.rect.y).fold(f32::MAX, f32::min);
        assert_eq!(top, 10.0, "line 0 sits at the top of the track");
        let none: &[(Series, Vec<usize>)] = &[];
        let mut g = Frame::default();
        scrollbar_ticks(&mut g, track, none, 100, 1.0);
        assert!(g.scrollbars.is_empty());
    }

    #[test]
    fn scrollbar_pieces_scale_with_the_display() {
        use crate::scrollbar::{Series, Style, geometry};
        // 200%: the outline is 2 px and the fill sits 2 px in.
        let track = Rect { x: 100.0, y: 10.0, w: 30.0, h: 406.0 };
        let mut f = Frame::default();
        scrollbar_ticks(&mut f, track, &[(Series::Search, vec![0])], 100, 2.0);
        let top = f.scrollbars.iter().find(|q| q.rect.y == 10.0 && q.rect.h == 2.0).expect("a 2 px top edge");
        assert_eq!(top.rect.w, 30.0);
        let fill = f.scrollbars.last().unwrap();
        assert_eq!(fill.rect, Rect { x: 102.0, y: 12.0, w: 26.0, h: 2.0 });
        // The legacy track's edge line and the knob's top and bottom padding.
        let pane = Rect { x: 0.0, y: 0.0, w: 800.0, h: 600.0 };
        let g = geometry(pane, Style::Legacy, 2.0, 100, 50, 0).unwrap();
        let mut s = Frame::default();
        scrollbar(&mut s, &g, Style::Legacy, false, false);
        assert!(s.scrollbars.iter().any(|q| q.rect.w == 2.0 && q.rect.x == g.track.x && q.rect.h == g.track.h), "a 2 px edge");
        assert!(s.scrollbars.iter().any(|q| q.rect.y >= g.thumb.y + 3.9 && q.rect.y <= g.thumb.y + 4.1), "the knob starts 4 px in");
    }

    #[test]
    fn scrollbar_draws_the_track_and_thumb() {
        use crate::scrollbar::{Style, geometry};
        let pane = Rect { x: 0.0, y: 0.0, w: 800.0, h: 600.0 };
        for dark in [false, true] {
            let g = geometry(pane, Style::Legacy, 1.0, 100, 50, 0).unwrap();
            let mut f = Frame::default();
            scrollbar(&mut f, &g, Style::Legacy, dark, false);
            let track = f.scrollbars[0];
            assert_eq!((track.rect, track.color[3]), (g.track, 1.0), "the legacy track is opaque and fills its strip");
            let thumb = bbox(&f.scrollbars[2..]);
            assert!(thumb.x > g.track.x && thumb.x + thumb.w < g.track.x + g.track.w, "the knob sits inside the track");
            assert_eq!((thumb.y, thumb.h), (g.thumb.y + 2.0, g.thumb.h - 4.0), "the knob follows the thumb");
            assert!(f.scrollbars.iter().all(|q| q.rect.x >= g.track.x - 0.001), "nothing draws left of the strip");
            assert!(f.quads.is_empty() && f.rows.is_empty() && f.popups.is_empty());

            let flat = geometry(pane, Style::Legacy, 1.0, 0, 50, 0).unwrap();
            let mut f = Frame::default();
            scrollbar(&mut f, &flat, Style::Legacy, dark, false);
            assert_eq!(f.scrollbars.len(), 2, "no history: the track and its edge alone");
        }
        let (mut a, mut b) = (Frame::default(), Frame::default());
        let g = geometry(pane, Style::Legacy, 1.0, 100, 50, 0).unwrap();
        scrollbar(&mut a, &g, Style::Legacy, false, false);
        scrollbar(&mut b, &g, Style::Legacy, true, false);
        assert_ne!(a.scrollbars[0].color, b.scrollbars[0].color, "colors follow the theme");
    }

    #[test]
    fn overlay_scrollbar_is_a_thin_thumb_that_widens_on_hover() {
        use crate::scrollbar::{OVERLAY_WIDTH, Style, geometry};
        let pane = Rect { x: 0.0, y: 0.0, w: 800.0, h: 600.0 };
        let g = geometry(pane, Style::Overlay, 1.0, 100, 50, 0).unwrap();
        let (mut calm, mut over) = (Frame::default(), Frame::default());
        scrollbar(&mut calm, &g, Style::Overlay, false, false);
        scrollbar(&mut over, &g, Style::Overlay, false, true);
        let (c, o) = (bbox(&calm.scrollbars), bbox(&over.scrollbars));
        assert_eq!((c.w, o.w), (7.0, OVERLAY_WIDTH));
        assert_eq!((c.x + c.w, o.x + o.w), (800.0, 800.0), "both hug the pane's right edge");
        assert_eq!((c.y, c.h), (g.thumb.y, g.thumb.h));
        assert!(calm.scrollbars.iter().all(|q| q.color[3] < 1.0), "no track, the thumb is translucent");
        let mut dark = Frame::default();
        scrollbar(&mut dark, &g, Style::Overlay, true, false);
        assert_ne!(dark.scrollbars[0].color[..3], calm.scrollbars[0].color[..3]);
        assert!(over.scrollbars[0].color[3] > calm.scrollbars[0].color[3], "hover is stronger");
    }
}
