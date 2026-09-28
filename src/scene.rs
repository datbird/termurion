//! What to draw, as plain data: rectangles and rows of text. The app builds a
//! Frame from its panes, and the renderer draws it.

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::point_to_viewport;
use alacritty_terminal::vte::ansi::CursorShape;

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
}

#[derive(Clone, Debug, PartialEq)]
pub struct TextRow {
    pub x: f32,
    pub y: f32,
    pub spans: Vec<Span>,
    pub clip: Rect,
    /// Text size against the terminal font (a porthole's headings).
    pub scale: f32,
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

/// Draw order: `quads`, then `images`, then `rows`, then `overlay`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Frame {
    pub clear: [f32; 4],
    /// Background images, drawn before everything else.
    pub backgrounds: Vec<ImageQuad>,
    pub quads: Vec<Quad>,
    pub images: Vec<ImageQuad>,
    pub rows: Vec<TextRow>,
    pub overlay: Vec<Quad>,
    /// Popup boxes over the terminal: drawn after images, before text.
    pub popups: Vec<Quad>,
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
const DIM_ALPHA: f32 = 0.4;

fn srgb_to_linear(c: u8) -> f32 {
    let c = c as f32 / 255.0;
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

/// A color for the quad shader. The target is sRGB, so the shader works in linear light.
pub fn rgba(c: Rgb, a: f32) -> [f32; 4] {
    [srgb_to_linear(c.r), srgb_to_linear(c.g), srgb_to_linear(c.b), a]
}

struct CellLook {
    text: String,
    fg: Rgb,
    bg: Rgb,
    bold: bool,
    italic: bool,
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
    /// Not focused (a hollow cursor) but not dimmed: the key pane of a background window
    /// when Dim background windows is off.
    pub undimmed: bool,
    /// The key window's focused pane at a password prompt: the key glyph replaces the cursor
    /// (`iTermTextDrawingHelper.m:4053-4068`).
    pub key_cursor: bool,
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
/// `cursor_on` is the blink phase: false hides the cursor.
pub fn pane(frame: &mut Frame, screen: &Screen, rect: Rect, cell: (f32, f32), focused: bool, cursor_on: bool) {
    pane_with(frame, screen, rect, cell, focused, cursor_on, None)
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
    for row in frame.rows.iter_mut() {
        if row.y < r.y + r.h && row.y + ch > r.y && row.clip.x + row.clip.w > r.x {
            row.clip.w = (r.x - row.clip.x).max(0.0);
        }
    }
    // The end of a long query stays in view, with a caret after it.
    let shown: String = {
        let n = bar.query.chars().count();
        bar.query.chars().skip(n.saturating_sub(FIELD - 1)).collect()
    };
    let field = format!("{shown}_");
    let span = |text: String, color: Rgb| Span { text, color, bold: false, italic: false, alt_font: false, icon: false, prose: false, badge: false, toolbelt: false };
    let y = r.y + 4.0;
    let query_color = if bar.error { FIND_ERROR } else { TAB_TEXT };
    frame.rows.push(TextRow {
        x: r.x + cw,
        y,
        spans: vec![span(format!("{field:<FIELD$}  {}", bar.count), query_color)],
        clip: r,
        scale: 1.0,
    });
    let mode_x = r.x + (1 + FIELD + 2 + bar.count.chars().count() + 2) as f32 * cw;
    let mode_rect = Rect { x: mode_x, y: r.y, w: mode.chars().count() as f32 * cw, h: r.h };
    frame.rows.push(TextRow { x: mode_x, y, spans: vec![span(mode, TAB_TEXT_DIM)], clip: r, scale: 1.0 });
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

/// Per-line timestamps: each label right-aligned on its screen row, over the text.
pub fn timestamps(frame: &mut Frame, labels: &[(usize, String)], rect: Rect, cell: (f32, f32)) {
    let (cw, ch) = cell;
    let right = rect.x + rect.w;
    for (row, text) in labels {
        let n = text.chars().count() as f32;
        let y = rect.y + *row as f32 * ch;
        let bg = Rect { x: right - (n + 2.0) * cw, y, w: (n + 2.0) * cw, h: ch };
        frame.quads.push(Quad { rect: bg, color: rgba(STAMP_BG, 0.85) });
        // The pane's text on this row stops where the label starts.
        for r in frame.rows.iter_mut().filter(|r| r.y == y && r.clip.x + r.clip.w > bg.x) {
            r.clip.w = (bg.x - r.clip.x).max(0.0);
        }
        let span = Span { text: text.clone(), color: TAB_TEXT, bold: false, italic: false, alt_font: false, icon: false, prose: false, badge: false, toolbelt: false };
        frame.rows.push(TextRow { x: right - (n + 1.0) * cw, y, spans: vec![span], clip: rect, scale: 1.0 });
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
    cursor_on: bool,
    highlights: Option<&Highlights>,
) {
    let history = screen.term().grid().history_size() as i32;
    // Underlines: OSC 8 links get dashes in their text color; the Win-click target under
    // the pointer gets a solid line in the link color.
    let mut dashes: Vec<(usize, usize, Rgb)> = Vec::new();
    let mut hovered: Vec<(usize, usize)> = Vec::new();
    let (cw, ch) = cell;
    let palette = &screen.palette;
    let term = screen.term();
    let (cols, lines) = (term.columns(), term.screen_lines());
    let content = term.renderable_content();
    let cursor = point_to_viewport(content.display_offset, content.cursor.point)
        .filter(|_| cursor_on && content.cursor.shape != CursorShape::Hidden)
        .map(|p| (p.line, p.column.0));
    let key_cursor = highlights.is_some_and(|h| h.key_cursor);
    let block_cursor = !key_cursor && focused && content.cursor.shape == CursorShape::Block;

    let blank = || CellLook { text: " ".into(), fg: palette.fg, bg: palette.bg, bold: false, italic: false };
    let mut grid: Vec<Vec<CellLook>> = (0..lines).map(|_| (0..cols).map(|_| blank()).collect()).collect();
    for indexed in content.display_iter {
        let Some(vp) = point_to_viewport(content.display_offset, indexed.point) else { continue };
        let (row, col) = (vp.line, vp.column.0);
        if row >= lines || col >= cols {
            continue;
        }
        let c = indexed.cell;
        let (mut fg, mut bg) = palette.cell(c.fg, c.bg, c.flags);
        let mut hover = false;
        if let Some(h) = highlights {
            let at = crate::find::Pos { row: (indexed.point.line.0 + history) as usize, col };
            let i = h.matches.partition_point(|m| m.end < at);
            if h.matches.get(i).is_some_and(|m| m.start <= at) {
                (fg, bg) = (match_text(h.color), h.color);
            }
            if h.hover.is_some_and(|m| m.start <= at && at <= m.end) {
                fg = h.link;
                hover = true;
                hovered.push((row, col));
            }
        }
        if !hover && c.hyperlink().is_some() {
            dashes.push((row, col, fg));
        }
        let mut hidden = false;
        if let Some(h) = highlights {
            let at_row = (indexed.point.line.0 + history) as usize;
            if h.labels.iter().any(|(p, l)| p.row == at_row && col >= p.col && col < p.col + l.chars().count()) {
                (fg, bg) = (LABEL_TEXT, LABEL_BG);
                hidden = true;
            }
        }
        if content.selection.as_ref().is_some_and(|s| s.contains(indexed.point)) {
            (fg, bg) = (palette.selected_text.unwrap_or(fg), palette.selection);
        }
        if block_cursor && cursor == Some((row, col)) {
            (fg, bg) = (palette.cursor_text, palette.cursor);
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
        let mut text = String::new();
        if hidden {
            text.push(' ');
        } else if !c.flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
            text.push(c.c);
            if let Some(extra) = c.zerowidth() {
                text.extend(extra);
            }
        }
        grid[row][col] = CellLook {
            text,
            fg,
            bg,
            bold: c.flags.contains(Flags::BOLD),
            italic: c.flags.contains(Flags::ITALIC),
        };
    }

    for (row, cells) in grid.iter().enumerate() {
        let y = rect.y + row as f32 * ch;
        // Background runs.
        let mut col = 0;
        while col < cols {
            let bg = cells[col].bg;
            let start = col;
            while col < cols && cells[col].bg == bg {
                col += 1;
            }
            if bg != palette.bg {
                let r = Rect { x: rect.x + start as f32 * cw, y, w: (col - start) as f32 * cw, h: ch };
                frame.quads.push(Quad { rect: r, color: rgba(bg, 1.0) });
            }
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
                    frame.rows.push(TextRow { x: rect.x + start as f32 * cw, y, spans, clip: rect, scale: 1.0 });
                }
            }
        };
        let mut piece: Option<(usize, Vec<Span>)> = None;
        for (col, c) in cells.iter().enumerate() {
            if c.text.is_empty() {
                continue;
            }
            let ascii = c.text.is_ascii();
            if !ascii {
                flush(&mut piece, frame);
            }
            let (_, spans) = piece.get_or_insert_with(|| (col, Vec::new()));
            match spans.last_mut() {
                Some(s) if s.color == c.fg && s.bold == c.bold && s.italic == c.italic => s.text.push_str(&c.text),
                _ => spans.push(Span { text: c.text.clone(), color: c.fg, bold: c.bold, italic: c.italic, alt_font: !ascii, icon: false, prose: false, badge: false, toolbelt: false }),
            }
            if !ascii {
                flush(&mut piece, frame);
            }
        }
        flush(&mut piece, frame);
    }

    if let Some(h) = highlights {
        let top = history - content.display_offset as i32;
        for (p, label) in h.labels {
            let row = p.row as i32 - top;
            if row >= 0 && (row as usize) < lines {
                let span = Span { text: label.clone(), color: LABEL_TEXT, bold: true, italic: false, alt_font: false, icon: false, prose: false, badge: false, toolbelt: false };
                frame.rows.push(TextRow { x: rect.x + p.col as f32 * cw, y: rect.y + row as f32 * ch, spans: vec![span], clip: rect, scale: 1.0 });
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
                frame.overlay.push(Quad { rect: Rect { x: rect.x + a as f32 * cw, y, w: (b + 1 - a.min(b + 1)) as f32 * cw, h: 1.0 }, color: rgba(NOTE, 1.0) });
            }
            // A note on show: its text in a box after the annotated text.
            let row = m.end.row as i32 - top;
            if let (Some(text), true) = (label, row >= 0 && (row as usize) < lines) {
                let shown: String = text.lines().next().unwrap_or_default().chars().take(40).collect();
                let x = rect.x + ((m.end.col + 2) as f32 * cw).min((rect.w - (shown.chars().count() as f32 + 1.0) * cw).max(0.0));
                let y = rect.y + row as f32 * ch;
                let r = Rect { x, y, w: (shown.chars().count() as f32 + 1.0) * cw, h: ch };
                frame.quads.push(Quad { rect: r, color: rgba(NOTE_BG, 1.0) });
                let span = Span { text: shown, color: rgb(0, 0, 0), bold: false, italic: false, alt_font: false, icon: false, prose: false, badge: false, toolbelt: false };
                frame.rows.push(TextRow { x: x + cw * 0.5, y, spans: vec![span], clip: rect, scale: 1.0 });
            }
        }
    }
    for (row, col, color) in dashes {
        for part in [0.0, 0.5] {
            let r = Rect { x: rect.x + (col as f32 + part) * cw, y: underline_y(row), w: (cw / 4.0).max(1.0), h: 1.0 };
            frame.overlay.push(Quad { rect: r, color: rgba(color, 1.0) });
        }
    }
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
        let span = Span { text: crate::pwm_prompt::KEY_GLYPH.to_string(), color: match_text(bg), bold: false, italic: false, alt_font: false, icon: true, prose: false, badge: false, toolbelt: false };
        frame.rows.push(TextRow { x: r.x, y: r.y + (ch - ch * scale) / 2.0, spans: vec![span], clip: r, scale });
    } else if let Some((row, col)) = cursor {
        let r = Rect { x: rect.x + col as f32 * cw, y: rect.y + row as f32 * ch, w: cw, h: ch };
        let color = rgba(palette.cursor, 1.0);
        let thin = 2.0_f32.min(cw);
        match content.cursor.shape {
            CursorShape::Beam => frame.overlay.push(Quad { rect: Rect { w: thin, ..r }, color }),
            CursorShape::Underline => frame.overlay.push(Quad { rect: Rect { y: r.y + r.h - thin, h: thin, ..r }, color }),
            _ if !block_cursor => {
                for edge in [
                    Rect { h: 1.0, ..r },
                    Rect { y: r.y + r.h - 1.0, h: 1.0, ..r },
                    Rect { w: 1.0, ..r },
                    Rect { x: r.x + r.w - 1.0, w: 1.0, ..r },
                ] {
                    frame.overlay.push(Quad { rect: edge, color });
                }
            }
            _ => {}
        }
    }

    if !focused && !highlights.is_some_and(|h| h.undimmed) {
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

/// Mark indicators: a bar in the left padding beside each marked row on screen.
pub fn mark_bars(frame: &mut Frame, marks: &[(usize, crate::marks::MarkColor)], inner: Rect, cell_h: f32, pad: f32) {
    let w = (pad * 0.75).max(1.0);
    for (row, color) in marks {
        let rect = Rect { x: inner.x - pad, y: inner.y + *row as f32 * cell_h, w, h: cell_h };
        frame.overlay.push(Quad { rect, color: mark_rgba(*color) });
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
    let text = |t: String, color: Rgb, icon: bool| Span { text: t, color, bold: false, italic: false, alt_font: false, icon, prose: false, badge: false, toolbelt: false };
    if let Some(c) = cut(close, clip) {
        frame.rows.push(TextRow { x: close.x + (s - cell.0) / 2.0, y: close.y, spans: vec![text("\u{E711}".into(), blend(bg, fg, 0.5), true)], clip: c, scale: 1.0 });
    }
    if let Some(c) = cut(language, clip) {
        frame.quads.push(Quad { rect: c, color: rgba(blend(back, fg, 0.12), 1.0) });
        frame.rows.push(TextRow { x: language.x + cell.0, y: language.y, spans: vec![text(chrome.language.to_string(), fg, false)], clip: c, scale: 1.0 });
    }
    if let Some(c) = cut(wide, clip) {
        let label = if chrome.wide { "[x] Wide" } else { "[ ] Wide" };
        frame.rows.push(TextRow { x: wide.x, y: wide.y, spans: vec![text(label.into(), fg, false)], clip: c, scale: 1.0 });
    }
    let inner = Rect { x: rect.x + 4.0, y: rect.y + 4.0 + s, w: rect.w - 8.0, h: rect.h - 8.0 - s };
    let Some(area) = cut(inner, clip) else { return PortholeControls { close, language, wide } };
    let ox = rect.x + PORTHOLE_MARGIN - chrome.scroll_x;
    let oy = rect.y + PORTHOLE_MARGIN + s;
    // Selection and find marks, from the text's top left, under the text.
    for (m, color, alpha) in marks {
        if let Some(r) = cut(Rect { x: ox + m.x, y: oy + m.y, ..*m }, area) {
            frame.quads.push(Quad { rect: r, color: rgba(*color, *alpha) });
        }
    }
    for l in lines {
        let y = oy + l.y;
        if l.code_bg {
            if let Some(r) = cut(Rect { x: inner.x, y, w: inner.w, h: l.h }, area) {
                frame.quads.push(Quad { rect: r, color: rgba(blend(back, fg, 0.08), 1.0) });
            }
        }
        if l.rule {
            if let Some(r) = cut(Rect { x: ox + l.x, y: y + l.h / 2.0, w: inner.w - PORTHOLE_MARGIN - l.x, h: 1.0 }, area) {
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
            .map(|r| Span { text: r.text.clone(), color: r.color, bold: r.bold, italic: r.italic, alt_font: false, icon: false, prose: r.face == crate::porthole::render::Face::Prose, badge: false, toolbelt: false })
            .collect();
        frame.rows.push(TextRow { x: ox + l.x, y, spans, clip: area, scale: l.scale });
    }
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
            let span = Span { text: text.clone(), color: c, bold: true, italic: false, alt_font: false, icon: false, prose: false, badge: true, toolbelt: false };
            frame.rows.push(TextRow { x, y, spans: vec![span], clip: pane, scale });
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
                let span = Span { text: c.to_string(), color: mix(a), bold: false, italic: false, alt_font: false, icon: true, prose: false, badge: false, toolbelt: false };
                frame.rows.push(TextRow { x: r.x + r.w / 4.0, y: r.y, spans: vec![span], clip: *r, scale: 1.0 });
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
pub fn indicator_glyphs(sends: bool, receives: bool, coprocess: bool) -> Vec<char> {
    let mut out = Vec::new();
    if sends {
        out.push(BROADCAST_GLYPH);
    } else if receives {
        out.push(RECEIVE_ONLY_GLYPH);
    }
    if coprocess {
        out.push(COPROCESS_GLYPH);
    }
    out
}

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
pub fn top_right_indicators(frame: &mut Frame, pane: Rect, glyphs: &[char], color: Rgb, cell: (f32, f32)) {
    let step = cell.0 * 2.5;
    for (i, g) in glyphs.iter().enumerate() {
        let x = pane.x + pane.w - (i as f32 + 1.0) * step;
        let span = Span { text: g.to_string(), color, bold: false, italic: false, alt_font: false, icon: true, prose: false, badge: false, toolbelt: false };
        frame.rows.push(TextRow { x, y: pane.y + 4.0, spans: vec![span], clip: pane, scale: 1.0 });
    }
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
pub fn tab_bar(frame: &mut Frame, tabs: &[TabLook], active: usize, bar: &crate::tab_layout::Bar, area: Rect, scale: f32, font_scale: f32, line: f32, key: bool, overflow_down: bool) {
    draw_bar(frame, tabs, active, bar, area, scale, font_scale, line, key, overflow_down);
}

/// Draws a side tab bar (milestone 13d) laid out by `tab_layout::build_vertical` in `area`, as
/// `tab_bar` draws a horizontal one (the same arguments): one row per shown tab and per group
/// chip, each drawn as a tab is, with the overflow button in the bottom row. Answers each tab's
/// row in pixels in `tabs` order (a zero-size rect for a tab with no row or a row wholly
/// outside `area`) and each chip shown with its group id.
#[allow(clippy::too_many_arguments)]
pub fn tab_bar_vertical(frame: &mut Frame, tabs: &[TabLook], active: usize, bar: &crate::tab_layout::Bar, area: Rect, scale: f32, font_scale: f32, line: f32, key: bool, overflow_down: bool) -> (Vec<Rect>, Vec<(Rect, String)>) {
    draw_bar(frame, tabs, active, bar, area, scale, font_scale, line, key, overflow_down);
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
fn draw_bar(frame: &mut Frame, tabs: &[TabLook], active: usize, bar: &crate::tab_layout::Bar, area: Rect, scale: f32, font_scale: f32, line: f32, key: bool, overflow_down: bool) {
    use crate::tab_layout::{self, SUBTITLE_OFFSET, TITLE_OFFSET_WITH_SUBTITLE};
    frame.quads.push(Quad { rect: area, color: rgba(TAB_BAR, 1.0) });
    let px = |r: Rect| Rect { x: area.x + r.x * scale, y: area.y + r.y * scale, w: r.w * scale, h: r.h * scale };
    let tabs_area = if bar.vertical { area } else { Rect { w: (tab_layout::tab_area_width(bar, area.w / scale) * scale).min(area.w), ..area } };
    let first_quad = frame.quads.len();
    let title_scale = bar.font * font_scale;
    let sub_scale = tab_layout::subtitle_size(bar.font) * font_scale;
    let (title_h, sub_h) = (line * title_scale, line * sub_scale);
    let turn = spinner_turn();
    let span = |text: String, color: Rgb, bold: bool, italic: bool, icon: bool| Span { text, color, bold, italic, alt_font: false, icon, prose: !icon, badge: false, toolbelt: false };
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
        let back = if *i == active { TAB_ACTIVE } else { TAB_BAR };
        if *i == active || tab.dragged {
            frame.quads.push(Quad { rect: cell, color: rgba(back, 1.0) });
        }
        if let Some(col) = tab.color {
            let h = (3.0 * scale).round();
            frame.quads.push(Quad { rect: Rect { y: cell.y + cell.h - h, h, ..cell }, color: rgba(col, 1.0) });
        }
        let color = if *i == active { TAB_TEXT } else { TAB_TEXT_DIM };
        // A glyph from the icon font centered in `box_`.
        let glyph = |frame: &mut Frame, ch: char, box_: Rect, color: Rgb| {
            let s = TAB_GLYPH_PT * font_scale;
            let em = TAB_GLYPH_PT * scale;
            let x = box_.x + ((box_.w - em) / 2.0).floor();
            let y = box_.y + ((box_.h - line * s) / 2.0).floor();
            frame.rows.push(TextRow { x, y, spans: vec![span(ch.to_string(), color, false, false, true)], clip, scale: s });
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
            frame.rows.push(TextRow { x, y, spans: vec![span(text.counter.clone(), color, false, false, false)], clip, scale: title_scale });
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
            frame.rows.push(TextRow { x, y, spans, clip, scale: title_scale });
        }
        if let Some(sub) = subtitle {
            let sl = at(text.subtitle_label);
            let sw = text.subtitle_width * scale;
            let x = if text.subtitle_truncate { sl.x } else { sl.x + (sl.w - sw) / 2.0 };
            let y = cell.y + ((cell.h - sub_h) / 2.0).floor() + offset + title_h + SUBTITLE_OFFSET * scale;
            frame.rows.push(TextRow { x, y, spans: vec![span(sub.clone(), color, false, false, false)], clip, scale: sub_scale });
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
    // The "+" and the overflow button (`PSM setupButtons`), over the tabs' right margin (a side
    // bar's overflow button is its bottom row). The overflow button is two chevrons a gap
    // apart, in the text color while its menu is open (`PSMOverflowPopUpButton`
    // `overflowImagePressed`).
    let s = TAB_GLYPH_PT * font_scale;
    let em = TAB_GLYPH_PT * scale;
    let overflow_color = if overflow_down { TAB_TEXT } else { TAB_TEXT_DIM };
    let buttons = [(bar.add, TAB_ADD_GLYPH, 0.0, TAB_TEXT_DIM), (bar.overflow, TAB_OVERFLOW_GLYPH, TAB_OVERFLOW_GAP * em, overflow_color)];
    for (rect, ch, gap, color) in buttons.into_iter().filter_map(|(r, ch, gap, color)| r.map(|r| (px(r), ch, gap, color))) {
        let x = rect.x + ((rect.w - em - gap) / 2.0).floor();
        let y = rect.y + ((rect.h - line * s) / 2.0).floor();
        let xs: &[f32] = if gap > 0.0 { &[x, x + gap] } else { &[x] };
        for &x in xs {
            frame.rows.push(TextRow { x, y, spans: vec![span(ch.to_string(), color, false, false, true)], clip: clip_rect(rect, area), scale: s });
        }
    }
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
            let span = Span { text: t.label.clone(), color: text, bold: true, italic: false, alt_font: false, icon: false, prose: true, badge: false, toolbelt: false };
            frame.rows.push(TextRow { x, y, spans: vec![span], clip: clip_rect(sp, tabs_area), scale: name_scale });
        }
        if t.collapsed && union.is_none() {
            // The chevron, centered in its room at the block's end (`YS:1482-1494`).
            let s = GROUP_CHEVRON_PT * font_scale;
            let cx = b.x + b.w - (GROUP_NAME_PAD + GROUP_CHEVRON / 2.0) * scale;
            let x = (cx - GROUP_CHEVRON_PT * scale / 2.0).floor();
            let y = b.y + ((b.h - line * s) / 2.0).floor();
            let span = Span { text: GROUP_CHEVRON_GLYPH.to_string(), color: text, bold: false, italic: false, alt_font: false, icon: true, prose: false, badge: false, toolbelt: false };
            frame.rows.push(TextRow { x, y, spans: vec![span], clip: clip_rect(b, tabs_area), scale: s });
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
            let span = Span { text: letter.clone(), color: back, bold: true, italic: false, alt_font: false, icon: false, prose: true, badge: false, toolbelt: false };
            frame.rows.push(TextRow { x, y, spans: vec![span], clip, scale: s });
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

pub fn dividers(frame: &mut Frame, dividers: &[Divider]) {
    for d in dividers {
        frame.overlay.push(Quad { rect: d.rect, color: rgba(DIVIDER, 1.0) });
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
pub fn info_bar(frame: &mut Frame, bar: &crate::infobar::Bar, l: &crate::infobar::Layout, cell: (f32, f32)) {
    let r = l.bar;
    for row in frame.rows.iter_mut() {
        if row.y < r.y + r.h && row.y + cell.1 > r.y && row.clip.x < r.x + r.w && row.clip.x + row.clip.w > r.x {
            row.clip.w = 0.0;
        }
    }
    frame.quads.push(Quad { rect: r, color: rgba(NOTE_BG, 1.0) });
    let span = |text: String| Span { text, color: LABEL_TEXT, bold: false, italic: false, alt_font: false, icon: false, prose: false, badge: false, toolbelt: false };
    let text_end = l.buttons.first().map_or(l.close.x, |b| b.x);
    let clip = Rect { w: (text_end - r.x).max(0.0), ..r };
    frame.rows.push(TextRow { x: r.x + cell.0, y: r.y + 4.0, spans: vec![span(bar.shown_text())], clip, scale: 1.0 });
    for (i, b) in l.buttons.iter().enumerate() {
        frame.quads.push(Quad { rect: *b, color: rgba(BAR_BUTTON, 1.0) });
        frame.rows.push(TextRow { x: b.x + cell.0, y: r.y + 4.0, spans: vec![span(bar.label(i))], clip: *b, scale: 1.0 });
    }
    frame.rows.push(TextRow { x: l.close.x + cell.0 / 2.0, y: r.y + 4.0, spans: vec![span("x".into())], clip: l.close, scale: 1.0 });
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

/// A status bar (`SV:151-162`): the background, the component bands, the boxes, the
/// separators (one point wide, three points in from the top and bottom), button faces, the
/// text, and the edge line.
pub fn status_bar(frame: &mut Frame, d: &StatusBarDraw, cell: (f32, f32)) {
    let r = d.rect;
    frame.quads.push(Quad { rect: r, color: rgba(d.background.0, d.background.1) });
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
        let span = Span { text: t.text.clone(), color: t.color, bold: false, italic: false, alt_font: false, icon: t.icon, prose: !t.icon, badge: false, toolbelt: false };
        frame.rows.push(TextRow { x: t.x, y, spans: vec![span], clip: t.clip, scale: t.scale });
    }
    if let Some(e) = d.edge {
        frame.quads.push(Quad { rect: e, color: rgba(STATUS_BAR_EDGE, 1.0) });
    }
}

/// Text already in the frame stops at `r` (a popup drawn over it): each line behind `r`
/// keeps its part left of `r` and gets a copy for its part right of it. `line_h` is a text
/// row's height (rows clip to their whole pane, so their own line decides). A popup's box
/// goes in `popups`, which draw after images and before text.
pub fn cut_out(frame: &mut Frame, r: Rect, line_h: f32) {
    let mut extra = Vec::new();
    for row in frame.rows.iter_mut() {
        let c = row.clip;
        let line_h = line_h * row.scale.max(1.0);
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
    }
    let faded = |q: Quad| Quad { color: [q.color[0], q.color[1], q.color[2], q.color[3] * alpha], ..q };
    frame.popups.extend(layer.quads.into_iter().chain(layer.popups).map(faded));
    frame.overlay.extend(layer.overlay.into_iter().map(faded));
    // Pictures keep their full opacity and draw before popups (none in the bar yet).
    frame.images.extend(layer.images);
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

/// The completion list at the cursor (Autocomplete, Auto Command Completion): rows with
/// the chosen one marked, and a footer. `at`: the cell below the cursor's top-left corner.
pub fn completion_list(frame: &mut Frame, rows: &[String], selected: usize, footer: Option<&str>, at: (f32, f32), bounds: Rect, cell: (f32, f32)) {
    const SHOWN: usize = 10;
    if rows.is_empty() {
        return;
    }
    let first = selected.saturating_sub(SHOWN - 1);
    let visible = &rows[first..rows.len().min(first + SHOWN)];
    let widest = visible.iter().map(|r| r.chars().count()).chain(footer.map(|f| f.chars().count())).max().unwrap_or(10).clamp(10, 60);
    let h = (visible.len() + usize::from(footer.is_some())) as f32 * cell.1 + 4.0;
    let w = (widest + 2) as f32 * cell.0;
    // Below the cursor, or above it when it would not fit; kept inside the pane.
    let mut y = at.1;
    if y + h > bounds.y + bounds.h {
        y = (at.1 - cell.1 - h).max(bounds.y);
    }
    let x = at.0.min(bounds.x + bounds.w - w).max(bounds.x);
    let r = Rect { x, y, w, h };
    cut_out(frame, r, cell.1);
    frame.popups.push(Quad { rect: r, color: rgba(NOTE_BG, 1.0) });
    let span = |text: String| Span { text, color: LABEL_TEXT, bold: false, italic: false, alt_font: false, icon: false, prose: false, badge: false, toolbelt: false };
    for (i, text) in visible.iter().enumerate() {
        let ry = y + 2.0 + i as f32 * cell.1;
        if first + i == selected {
            frame.popups.push(Quad { rect: Rect { x, y: ry, w, h: cell.1 }, color: rgba(BAR_BUTTON, 1.0) });
        }
        frame.rows.push(TextRow { x: x + cell.0, y: ry, spans: vec![span(text.clone())], clip: r, scale: 1.0 });
    }
    if let Some(f) = footer {
        frame.rows.push(TextRow { x: x + cell.0, y: y + 2.0 + visible.len() as f32 * cell.1, spans: vec![span(f.to_string())], clip: r, scale: 1.0 });
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
        frame.rows.push(TextRow { x: x + cell.0, y: y + 2.0 + i as f32 * cell.1, spans: vec![plain_span(line.to_string(), TAB_TEXT, false)], clip: r, scale: 1.0 });
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
    Span { text, color, bold, italic: false, alt_font: false, icon, prose: !icon, badge: false, toolbelt: false }
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
        frame.rows.push(TextRow { x, y: r.y + pad, spans: vec![clip_span(row.title.clone(), TAB_TEXT, true, false)], clip, scale: 1.0 });
        for (k, line) in row.detail.iter().enumerate() {
            let y = r.y + pad + cell.1 + 2.0 * scale + k as f32 * cell.1;
            frame.rows.push(TextRow { x, y, spans: vec![clip_span(line.clone(), TAB_TEXT_DIM, false, false)], clip, scale: 1.0 });
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
        frame.rows.push(TextRow { x: r.x + (r.w - cell.0) / 2.0, y: r.y + (r.h - cell.1) / 2.0, spans: vec![clip_span(glyph.to_string(), color, false, true)], clip: *r, scale: 1.0 });
    }
    if let Some(text) = d.status {
        let r = l.status;
        frame.rows.push(TextRow { x: r.x, y: r.y + (r.h - cell.1) / 2.0, spans: vec![clip_span(text.to_string(), TAB_TEXT_DIM, false, false)], clip: r, scale: 1.0 });
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
    frame.rows.push(TextRow { x, y, spans: vec![clip_span(title.to_string(), TAB_TEXT, true, false)], clip: r, scale: 1.0 });
    y += cell.1 * 1.5;
    for line in lines {
        if y + cell.1 > r.y + r.h {
            break;
        }
        frame.rows.push(TextRow { x, y, spans: vec![clip_span(line.clone(), TAB_TEXT, false, false)], clip: r, scale: 1.0 });
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
    Span { text, color, bold: false, italic: false, alt_font: false, icon, prose: !icon, badge: false, toolbelt: false }
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
        frame.rows.push(TextRow { x: r.x + (r.w - glyph_w) / 2.0, y: r.y + (r.h - line_h) / 2.0, spans: vec![belt_span(c.to_string(), icon, true)], clip: r, scale: d.text_scale });
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
        frame.rows.push(TextRow { x, y: r.y + (r.h - line_h) / 2.0, spans: vec![belt_span(title.to_string(), text, false)], clip, scale: d.text_scale });
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
                frame.rows.push(TextRow { x, y, spans: vec![belt_span(line.clone(), text, false)], clip: r, scale: d.text_scale });
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
    Span { text, color, bold, italic: false, alt_font: false, icon: font == 2, prose: font == 1, badge: false, toolbelt: font == 0 }
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
            frame.rows.push(TextRow { x, y, spans: vec![list_span(shown.to_string(), text, false, 0)], clip, scale: text_scale });
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
            frame.rows.push(TextRow { x: x + (slot * scale - gh / 1.2) / 2.0, y: y + (line_h - gh) / 2.0, spans: vec![list_span(c.to_string(), color, false, 2)], clip, scale: s });
            x += (slot + gap) * scale;
        }
        if let Some((right, w)) = &row.right {
            // The tertiary color; on the selection, white at 0.7 (`updateShortcutColor`).
            let color = if selected { blend(rgb(0xff, 0xff, 0xff), LIST_SELECTED, 0.3) } else { soft(blend(dim, bg, 0.35)) };
            frame.rows.push(TextRow { x: (r.x + r.w - inset - w).max(x), y, spans: vec![list_span(right.clone(), color, false, 0)], clip, scale: text_scale });
        }
        if let Some(c) = row.disclosure {
            frame.rows.push(TextRow { x, y, spans: vec![list_span(c.to_string(), fg2, false, 2)], clip, scale: text_scale });
            x += glyph_w + 2.0 * scale;
        }
        if let Some((c, color)) = row.icon {
            frame.rows.push(TextRow { x, y, spans: vec![list_span(c.to_string(), if selected { fg } else { color }, false, 2)], clip, scale: text_scale });
            x += glyph_w + 4.0 * scale;
        }
        frame.rows.push(TextRow { x, y, spans: vec![list_span(row.text.clone(), fg, row.bold, 0)], clip, scale: text_scale });
        if let Some(second) = &row.second {
            frame.rows.push(TextRow { x, y: y + line_h, spans: vec![list_span(second.clone(), fg2, false, 1)], clip, scale: text_scale * d.second_scale });
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
            frame.rows.push(TextRow { x: lx, y: ly, spans: vec![list_span(line.clone(), color, false, 0)], clip, scale: text_scale });
            ly += line_h;
        }
        let mut cx = x;
        for (tag, w) in &row.chips {
            let chip = Rect { x: cx, y: y + line_h, w: *w, h: line_h * CHIP_SCALE };
            let shown = Rect { w: chip.w.min((clip.x + clip.w - cx).max(0.0)), ..chip };
            frame.quads.push(Quad { rect: shown, color: rgba(CHIP_BG, 1.0) });
            frame.rows.push(TextRow { x: cx, y: chip.y, spans: vec![list_span(tag.clone(), rgb(0xff, 0xff, 0xff), false, 0)], clip, scale: text_scale * CHIP_SCALE });
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
        frame.rows.push(TextRow { x: r.x + (r.w - glyph_w) / 2.0, y: r.y + (r.h - line_h) / 2.0, spans: vec![list_span(b.glyph().to_string(), color, false, 2)], clip: *r, scale: text_scale });
    }

    if let (Some(r), Some((label, on))) = (l.popup, d.popup) {
        frame.quads.push(Quad { rect: r, color: rgba(field_bg, 1.0) });
        frame_lines(&mut frame.quads, r, rgba(DIVIDER, if d.dark { 1.0 } else { 0.4 }));
        let color = if on { text } else { disabled };
        let y = r.y + (r.h - line_h) / 2.0;
        let glyph_x = r.x + r.w - glyph_w - 6.0 * scale;
        let clip = Rect { w: (glyph_x - r.x).max(0.0), ..r };
        frame.rows.push(TextRow { x: r.x + 6.0 * scale, y, spans: vec![list_span(label.to_string(), color, false, 1)], clip, scale: text_scale });
        frame.rows.push(TextRow { x: glyph_x, y, spans: vec![list_span(POPUP_GLYPH.to_string(), color, false, 2)], clip: r, scale: text_scale });
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
        frame.rows.push(TextRow { x: r.x + ((r.w - w) / 2.0).max(0.0), y: r.y + (r.h - label_h) / 2.0, spans: vec![list_span(label.to_string(), color, false, 1)], clip: *r, scale: d.label_scale });
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
            frame.rows.push(TextRow { x: *x, y: *y, spans: vec![list_span(line.clone(), text, false, 0)], clip: d.clip, scale: text_scale });
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
    Span { text, color, bold, italic: false, alt_font: false, icon: false, prose: false, badge: false, toolbelt: false }
}

/// The minimal composer over its pane (`ComposerDraw`). The panel is a popup box: the pane's
/// text under it is cut out and the box goes in `popups`, with the selection and the caret on
/// it; its own text rows come after.
pub fn composer(frame: &mut Frame, d: &ComposerDraw, cell: (f32, f32)) {
    let a = d.alpha.clamp(0.0, 1.0);
    if a <= 0.0 || d.panel.w <= 2.0 || d.panel.h <= 2.0 {
        return;
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
            frame.rows.push(TextRow { x: *x, y: *y, spans, clip: d.clip, scale: 1.0 });
        }
    }
    if d.separator.is_some() {
        return;
    }
    let icon = |frame: &mut Frame, r: Rect, glyph: char| {
        let (w, h) = (cell.0, cell.1);
        let span = Span { text: glyph.to_string(), color: under(d.fg), bold: false, italic: false, alt_font: false, icon: true, prose: false, badge: false, toolbelt: false };
        frame.rows.push(TextRow { x: r.x + ((r.w - w) / 2.0).max(0.0), y: r.y + ((r.h - h) / 2.0).max(0.0), spans: vec![span], clip: r, scale: 1.0 });
    };
    icon(frame, d.close, COMPOSER_CLOSE_GLYPH);
    icon(frame, d.help, COMPOSER_HELP_GLYPH);
    let small_h = cell.1 * d.small;
    if let Some(r) = d.ai {
        let w = 2.0 * cell.0 * d.small;
        frame.rows.push(TextRow { x: r.x + ((r.w - w) / 2.0).max(0.0), y: r.y + ((r.h - small_h) / 2.0).max(0.0), spans: vec![plain_span("AI".into(), under(d.fg), false)], clip: r, scale: d.small });
    }
    if let Some((x, y, tip)) = d.tip {
        frame.rows.push(TextRow { x, y, spans: vec![plain_span(tip.to_string(), under(TAB_TEXT_DIM), false)], clip: inner, scale: d.small });
    }
    // The tooltip above the Help button, a small box of its own over the text.
    if let Some(t) = d.tooltip {
        let (w, h) = ((t.chars().count() as f32 + 2.0) * cell.0 * d.small, cell.1 * d.small + 6.0);
        let r = Rect { x: d.help.x, y: (d.help.y - h - 2.0).max(d.panel.y), w, h };
        cut_out(frame, r, d.line_h);
        frame.popups.push(Quad { rect: r, color: rgba(FIND_BAR, a) });
        frame_lines(&mut frame.popups, r, rgba(DIVIDER, a));
        frame.rows.push(TextRow { x: r.x + cell.0 * d.small, y: r.y + 3.0, spans: vec![plain_span(t.to_string(), under(TAB_TEXT), false)], clip: r, scale: d.small });
    }
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
        frame.rows.push(TextRow { x: r.x + 8.0 * scale, y: text_y(ry), spans: vec![plain_span(p.to_string(), TAB_TEXT_DIM, false)], clip: inner, scale: 1.0 });
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
        let icon = Span { text: glyph.to_string(), color, bold: false, italic: false, alt_font: false, icon: true, prose: false, badge: false, toolbelt: false };
        frame.rows.push(TextRow { x: r.x + 8.0 * scale, y: text_y(ry), spans: vec![icon], clip: list, scale: 1.0 });
        let mut spans = Vec::new();
        match bold.clone().filter(|b| b.end <= t.len() && t.is_char_boundary(b.end)) {
            Some(b) => {
                spans.push(plain_span(t[..b.end].to_string(), color, true));
                spans.push(plain_span(t[b.end..].to_string(), color, false));
            }
            None => spans.push(plain_span(t.clone(), color, false)),
        }
        spans.retain(|s| !s.text.is_empty());
        frame.rows.push(TextRow { x: r.x + 8.0 * scale + 2.0 * cell.0, y: text_y(ry), spans, clip: list, scale: 1.0 });
    }
    if let Some(detail) = d.detail {
        let sy = r.y + r.h - strip;
        frame.popups.push(Quad { rect: Rect { x: inner.x, y: sy, w: inner.w, h: 1.0 }, color: rgba(DIVIDER, 1.0) });
        frame.rows.push(TextRow { x: r.x + 8.0 * scale, y: sy + ((strip - cell.1) / 2.0).max(0.0), spans: vec![plain_span(detail.to_string(), TAB_TEXT_DIM, false)], clip: inner, scale: 1.0 });
    }
    r
}

/// What Codecierge shows (`CC:646-709`): its label's lines, centered (x, y, text), a button
/// (its rectangle, label, label width) dimmed when disabled.
pub struct CodeciergeDraw<'a> {
    pub lines: &'a [(f32, f32, String)],
    pub clip: Rect,
    pub button: Rect,
    pub label: &'a str,
    pub label_w: f32,
    pub enabled: bool,
    pub dark: bool,
}

/// Codecierge's onboarding view: the label and its one button, in the prose font at
/// `text_scale`.
pub fn codecierge(frame: &mut Frame, d: &CodeciergeDraw, cell: (f32, f32), text_scale: f32, scale: f32) {
    let (text, disabled, field_bg) = if d.dark { (TAB_TEXT, LIST_DISABLED_DARK, FIELD_BG_DARK) } else { (BELT_TEXT_LIGHT, LIST_DISABLED_LIGHT, FIELD_BG_LIGHT) };
    let line_h = cell.1 * text_scale;
    for (x, y, line) in d.lines {
        frame.rows.push(TextRow { x: *x, y: *y, spans: vec![list_span(line.clone(), text, false, 1)], clip: d.clip, scale: text_scale });
    }
    let b = d.button;
    rounded_box(&mut frame.quads, b, 4.0 * scale, rgba(DIVIDER, if d.dark { 1.0 } else { 0.4 }));
    let inner = Rect { x: b.x + 1.0, y: b.y + 1.0, w: (b.w - 2.0).max(0.0), h: (b.h - 2.0).max(0.0) };
    rounded_box(&mut frame.quads, inner, 3.0 * scale, rgba(field_bg, 1.0));
    let color = if d.enabled { text } else { disabled };
    frame.rows.push(TextRow { x: b.x + ((b.w - d.label_w) / 2.0).max(0.0), y: b.y + (b.h - line_h) / 2.0, spans: vec![list_span(d.label.to_string(), color, false, 1)], clip: clip_rect(b, d.clip), scale: text_scale });
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

/// A short message in a dark box near the bottom center of `area`.
pub fn toast(frame: &mut Frame, text: &str, area: Rect, cell: (f32, f32)) {
    let w = (text.chars().count() as f32 + 4.0) * cell.0;
    let r = Rect { x: area.x + ((area.w - w) / 2.0).max(0.0), y: area.y + area.h - 3.0 * cell.1, w: w.min(area.w), h: cell.1 + 12.0 };
    // Text under the box is cut where the box starts, as under the find bar.
    for row in frame.rows.iter_mut() {
        if row.y < r.y + r.h && row.y + cell.1 > r.y && row.clip.x + row.clip.w > r.x {
            row.clip.w = (r.x - row.clip.x).max(0.0);
        }
    }
    frame.quads.push(Quad { rect: r, color: rgba(STAMP_BG, 1.0) });
    let span = Span { text: text.to_string(), color: TAB_TEXT, bold: false, italic: false, alt_font: false, icon: false, prose: false, badge: false, toolbelt: false };
    frame.rows.push(TextRow { x: r.x + 2.0 * cell.0, y: r.y + 6.0, spans: vec![span], clip: r, scale: 1.0 });
}

/// A one-line error bar along the bottom of `area`. Its box goes in `quads`, so its
/// text (in `rows`) draws on top.
pub fn banner(frame: &mut Frame, text: &str, area: Rect, cell: (f32, f32)) {
    let r = Rect { x: area.x, y: area.y + area.h - cell.1 - 8.0, w: area.w, h: cell.1 + 8.0 };
    frame.quads.push(Quad { rect: r, color: rgba(BANNER, 1.0) });
    let span = Span { text: text.to_string(), color: rgb(0xff, 0xff, 0xff), bold: false, italic: false, alt_font: false, icon: false, prose: false, badge: false, toolbelt: false };
    frame.rows.push(TextRow { x: r.x + cell.0, y: r.y + 4.0, spans: vec![span], clip: r, scale: 1.0 });
}

#[cfg(test)]
mod broadcast_tests {
    use super::*;

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
        assert_eq!(indicator_glyphs(true, true, true), vec![BROADCAST_GLYPH, COPROCESS_GLYPH]);
        assert_eq!(indicator_glyphs(false, true, false), vec![RECEIVE_ONLY_GLYPH]);
        assert_eq!(indicator_glyphs(false, false, true), vec![COPROCESS_GLYPH]);
        assert!(indicator_glyphs(false, false, false).is_empty());
    }
}

#[cfg(test)]
mod tests {

    /// A flashing bar over the sessions: its boxes over theirs at the bar's alpha, its text
    /// blended from the sessions' background, their text behind it cut out only while the
    /// bar is at least half opaque.
    #[test]
    fn lay_over_fades_the_bar() {
        use super::*;
        let pane = Rect { x: 0.0, y: 0.0, w: 100.0, h: 100.0 };
        let bar = Rect { x: 0.0, y: 0.0, w: 100.0, h: 20.0 };
        let span = |color: Rgb| Span { text: "a".into(), color, bold: false, italic: false, alt_font: false, icon: false, prose: false, badge: false, toolbelt: false };
        let base = || {
            let mut f = Frame::default();
            f.quads.push(Quad { rect: pane, color: rgba(rgb(0, 0, 0), 1.0) });
            f.rows.push(TextRow { x: 0.0, y: 0.0, spans: vec![span(rgb(9, 9, 9))], clip: pane, scale: 1.0 });
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
            l.rows.push(TextRow { x: 4.0, y: 2.0, spans: vec![span(rgb(200, 100, 0))], clip: bar, scale: 1.0 });
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
        let span = Span { text: "terminal".into(), color: LABEL_TEXT, bold: false, italic: false, alt_font: false, icon: false, prose: false, badge: false, toolbelt: false };
        let under = Rect { x: 0.0, y: 0.0, w: 400.0, h: 16.0 };
        f.rows.push(TextRow { x: 0.0, y: 0.0, spans: vec![span], clip: under, scale: 1.0 });
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
        f.rows.push(TextRow { x: 0.0, y: 40.0, spans: vec![span], clip: Rect { x: 0.0, y: 40.0, w: 400.0, h: 16.0 }, scale: 1.0 });
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
            let span = Span { text: format!("line {i}"), color: LABEL_TEXT, bold: false, italic: false, alt_font: false, icon: false, prose: false, badge: false, toolbelt: false };
            f.rows.push(TextRow { x: 0.0, y: i as f32 * 16.0, spans: vec![span], clip: pane, scale: 1.0 });
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
    fn mark_bars_sit_in_the_left_padding() {
        let mut f = Frame::default();
        let inner = Rect { x: 10.0, y: 20.0, w: 100.0, h: 100.0 };
        mark_bars(&mut f, &[(2, crate::marks::MarkColor::Error)], inner, 16.0, 4.0);
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
    fn a_red_background_makes_a_quad() {
        let s = screen(b"\x1b[41mX");
        let mut f = Frame::default();
        pane(&mut f, &s, RECT, CELL, true, true);
        let red = rgba(s.palette.ansi[1], 1.0);
        assert!(f.quads.iter().any(|q| q.color == red && q.rect == Rect { x: 0.0, y: 0.0, w: 10.0, h: 20.0 }), "{:?}", f.quads);
    }

    #[test]
    fn rows_carry_the_text() {
        let s = screen(b"hello\r\nworld");
        let mut f = Frame::default();
        pane(&mut f, &s, RECT, CELL, true, true);
        assert_eq!(row_text(&f, 0).trim_end(), "hello");
        assert_eq!(row_text(&f, 1).trim_end(), "world");
        assert_eq!((f.rows[1].x, f.rows[1].y), (0.0, 20.0));
    }

    #[test]
    fn focused_block_cursor_is_a_quad() {
        let s = screen(b"ab");
        let mut f = Frame::default();
        pane(&mut f, &s, RECT, CELL, true, true);
        let cursor = rgba(s.palette.cursor, 1.0);
        assert!(f.quads.iter().any(|q| q.color == cursor && q.rect.x == 20.0 && q.rect.y == 0.0));
    }

    #[test]
    fn unfocused_pane_is_dimmed_and_cursor_hollow() {
        let s = screen(b"ab");
        let mut f = Frame::default();
        pane(&mut f, &s, RECT, CELL, false, true);
        assert!(f.overlay.iter().any(|q| q.rect == RECT));
        assert!(!f.quads.iter().any(|q| q.color == rgba(s.palette.cursor, 1.0)));
    }

    #[test]
    fn selection_uses_selection_colors() {
        let mut s = screen(b"hello");
        let mut sel = Selection::new(SelectionType::Simple, Point::new(Line(0), Column(0)), Side::Left);
        sel.update(Point::new(Line(0), Column(1)), Side::Right);
        s.term_mut().selection = Some(sel);
        let mut f = Frame::default();
        pane(&mut f, &s, RECT, CELL, true, true);
        let sel = rgba(s.palette.selection, 1.0);
        assert!(f.quads.iter().any(|q| q.color == sel && q.rect == Rect { x: 0.0, y: 0.0, w: 20.0, h: 20.0 }), "{:?}", f.quads);
    }

    #[test]
    fn wide_char_spacer_is_skipped() {
        let s = screen("\u{4e2d}x".as_bytes());
        let mut f = Frame::default();
        pane(&mut f, &s, RECT, CELL, true, true);
        let all: String = f.rows.iter().flat_map(|r| r.spans.iter().map(|s| s.text.as_str())).collect();
        assert!(all.starts_with("\u{4e2d}x"), "{all:?}");
    }

    #[test]
    fn non_ascii_text_is_placed_on_its_own_cell() {
        // Fallback fonts are not monospace, so a wide or non-ASCII glyph gets its own
        // row piece at its cell's x, and the text after it starts at its own cell too.
        let s = screen("a\u{4e2d}x".as_bytes());
        let mut f = Frame::default();
        pane(&mut f, &s, RECT, CELL, true, true);
        let at = |text: &str| f.rows.iter().find(|r| r.spans.iter().any(|s| s.text.trim_end() == text)).map(|r| r.x);
        assert_eq!(at("\u{4e2d}"), Some(10.0), "{:?}", f.rows);
        assert_eq!(at("x"), Some(30.0), "{:?}", f.rows);
    }

    #[test]
    fn the_cursor_blinks_off() {
        let s = screen(b"ab");
        let mut f = Frame::default();
        pane(&mut f, &s, RECT, CELL, true, false);
        assert!(!f.quads.iter().any(|q| q.color == rgba(s.palette.cursor, 1.0)));
    }

    #[test]
    fn non_ascii_pieces_use_the_alternate_font() {
        let s = screen("a\u{4e2d}".as_bytes());
        let mut f = Frame::default();
        pane(&mut f, &s, RECT, CELL, true, true);
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
        pane_with(&mut f, &s, pane, CELL, false, false, None);
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
        let (rects, chips) = tab_bar_vertical(&mut f, &looks, 1, &bar, area, 1.0, 0.1, 20.0, true, false);
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
        let (rects, _) = tab_bar_vertical(&mut f, &looks, 1, &bar, area, 1.0, 0.1, 20.0, true, false);
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
        let (rects, _) = tab_bar_vertical(&mut f, &looks, 1, &bar, Rect { h: 60.0, ..area }, 1.0, 0.1, 20.0, true, false);
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
        tab_bar(&mut f, &looks(false), 2, &bar, area, 1.0, 0.1, 20.0, true, false);
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
        let (rects, chips) = tab_bar_vertical(&mut Frame::default(), &looks(false), 2, &vbar, side, 1.0, 0.1, 20.0, true, false);
        assert_eq!(chips, vec![(Rect { x: 0.0, y: 0.0, w: 150.0, h: 24.0 }, "g".to_string())]);
        assert_eq!(rects[0].y, 24.0, "the run's first tab is under its chip");

        // Collapsed: both tabs hidden, the chip drawn with "g  2" and the chevron.
        let bar = tab_layout::build(&inputs(true), 2, 600.0, 0.0, None, &s, &six);
        let mut f = Frame::default();
        tab_bar(&mut f, &looks(true), 2, &bar, area, 1.0, 0.1, 20.0, true, false);
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
        tab_bar(&mut f, &looks(true), 2, &joining, area, 1.0, 0.1, 20.0, true, false);
        let frame = accent(&f);
        assert_eq!(frame.len(), 4, "{frame:?}");
        assert!(frame.contains(&Rect { x: chip.x + chip.w - 1.0, y: 0.0, w: 2.0, h: 24.0 }), "the right edge: {frame:?}");
        assert!(frame.contains(&Rect { x: 0.0, y: 22.0, w: chip.w + 1.0, h: 2.0 }), "the bottom edge: {frame:?}");
        // Another group's id frames nothing; an expanded group is never a join target.
        let other = tab_layout::Bar { join: Some("h".into()), ..bar.clone() };
        let mut f = Frame::default();
        tab_bar(&mut f, &looks(true), 2, &other, area, 1.0, 0.1, 20.0, true, false);
        assert!(accent(&f).is_empty());
        let expanded = tab_layout::Bar { join: Some("g".into()), ..tab_layout::build(&inputs(false), 2, 600.0, 0.0, None, &s, &six) };
        let mut f = Frame::default();
        tab_bar(&mut f, &looks(false), 2, &expanded, area, 1.0, 0.1, 20.0, true, false);
        assert!(accent(&f).is_empty());
        let vbar = tab_layout::build_vertical(&inputs(true), 150.0, 200.0, 0.0, None, &s, &six);
        let (rects, chips) = tab_bar_vertical(&mut Frame::default(), &looks(true), 2, &vbar, side, 1.0, 0.1, 20.0, true, false);
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
        tab_bar(&mut f, &looks, 1, &bar, area, 2.0, 0.1, 20.0, true, false);

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
        tab_bar(&mut f, &ul, 0, &bar, area, 2.0, 0.1, 20.0, true, false);
        let ab = f.rows.iter().find(|r| r.spans.iter().any(|s| s.text == "ab")).expect("the run");
        assert!(f.quads.iter().any(|q| q.rect.h == 1.0 && q.rect.w == 24.0 && q.rect.x == ab.x), "{:?}", f.quads);

        // A subtitle line on a tall bar: the title 5 points up, the subtitle under it at 9 points.
        let tall = tab_layout::Settings { height: 28.0, ..s.clone() };
        let st = vec![TabLook { title: "t".into(), subtitle: "sub".into(), ..TabLook::default() }, TabLook::default()];
        let sin: Vec<TabInput> = st.iter().map(|l| TabInput { title: l.title.clone(), subtitle: l.subtitle.clone(), icon: false, processing: false, number: 0, pinned: false, group: None, hidden: false, graphic: false, letter: String::new() }).collect();
        let bar = tab_layout::build(&sin, 0, 400.0, 0.0, None, &tall, &six);
        let area = Rect { x: 0.0, y: 0.0, w: 800.0, h: 56.0 };
        let mut f = Frame::default();
        tab_bar(&mut f, &st, 0, &bar, area, 2.0, 0.1, 20.0, true, false);
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
        tab_bar(&mut f, &looks, 5, &bar, area, 1.0, 0.1, 20.0, true, false);
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
            tab_bar(&mut f, &looks, 0, &bar, area, 1.0, 0.1, 20.0, true, down);
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
            tab_bar(&mut f, &looks, 0, &bar, area, 1.0, 0.1, 20.0, key, false);
            let row = f.rows.iter().find(|r| r.spans.iter().any(|s| s.text == TAB_CLOSE_GLYPH.to_string())).expect("the close button");
            row.spans[0].color
        };
        assert_eq!(close_color(true), TAB_TEXT_DIM, "full alpha in the key window");
        assert_eq!(close_color(false), blend(TAB_BAR, TAB_TEXT_DIM, 0.5), "half alpha otherwise (YS drawInteriorWithTabCell:)");
        // Held down, a shade shows behind the glyph in its box.
        let close = bar.cells[1].1.close.unwrap();
        let shade = rgba(blend(TAB_BAR, TAB_TEXT_DIM, TAB_CLOSE_PRESSED), 1.0);
        let mut f = Frame::default();
        tab_bar(&mut f, &looks, 0, &bar, area, 1.0, 0.1, 20.0, true, false);
        assert!(!f.quads.iter().any(|q| q.color == shade), "not held");
        let held: Vec<TabLook> = looks.iter().cloned().enumerate().map(|(i, l)| TabLook { close_pressed: i == 1, ..l }).collect();
        let mut f = Frame::default();
        tab_bar(&mut f, &held, 0, &bar, area, 1.0, 0.1, 20.0, true, false);
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
        tab_bar(&mut f, &looks, 1, &bar, area, 1.0, 0.1, 20.0, true, false);
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
        tab_bar(&mut f, &looks, 1, &bar, area, 1.0, 0.1, 20.0, false, false);
        let pin = f.rows.iter().find(|r| r.spans.iter().any(|s| s.text == PIN_GLYPH.to_string())).unwrap();
        assert_eq!(pin.spans[0].color, blend(TAB_BAR, TAB_TEXT_DIM, 0.3));

        // A side bar: the pin, then the whole title.
        let mut f = Frame::default();
        let side = Rect { x: 0.0, y: 0.0, w: 150.0, h: 100.0 };
        let vbar = tab_layout::build_vertical(&inputs, 150.0, 100.0, 0.0, None, &s, &six);
        let (rects, _) = tab_bar_vertical(&mut f, &looks, 1, &vbar, side, 1.0, 0.1, 20.0, true, false);
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
        tab_bar(&mut f, &iconed, 1, &bar, area, 1.0, 0.1, 20.0, true, false);
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
        tab_bar(&mut f, &looks, 1, &bar, area, 1.0, 0.1, 20.0, true, false);
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
        tab_bar(&mut f, &looks, 1, &bar, Rect { x: 0.0, y: 0.0, w: 800.0, h: 48.0 }, 2.0, 0.2, 40.0, true, false);
        assert_eq!(f.images[0].rect, Rect { x: 2.0 * c1.x + 12.0, y: 8.0, w: 32.0, h: 32.0 });

        // A long title is truncated after the icon's room, 16 + 4 + 2 (`YS:1117`), not the
        // close button's 16 + 4.
        let long = "a title far too long for any tab in this bar at all, truly".to_string();
        let title_x = |image: Option<TabImage>| {
            let looks = vec![TabLook { title: long.clone(), number: 1, icon_image: image, ..TabLook::default() }, TabLook { title: "b".into(), number: 2, ..TabLook::default() }];
            let bar = tab_layout::build(&icon_inputs(&looks), 1, 400.0, 0.0, None, &s, &six);
            let mut f = Frame::default();
            tab_bar(&mut f, &looks, 1, &bar, area, 1.0, 0.1, 20.0, true, false);
            let row = f.rows.iter().find(|r| r.spans.iter().any(|s| s.text.starts_with("a title"))).expect("the title");
            row.x - bar.cells[0].2.x
        };
        assert_eq!(title_x(None), 6.0 + 20.0);
        assert_eq!(title_x(Some(picture.clone())), 6.0 + 22.0);

        // Hovered, the close button shows and the icon fades out (alpha 1 less the close
        // button's); in a window that is not key the close button is half and so is the icon.
        let hover = tab_layout::build(&icon_inputs(&looks), 1, 400.0, 0.0, Some(1), &s, &six);
        let mut f = Frame::default();
        tab_bar(&mut f, &looks, 1, &hover, area, 1.0, 0.1, 20.0, true, false);
        assert!(f.images.is_empty());
        let mut f = Frame::default();
        tab_bar(&mut f, &looks, 1, &hover, area, 1.0, 0.1, 20.0, false, false);
        assert_eq!(f.images[0].key, (42, 127));
        assert_eq!(f.images[0].pixels[3], 5, "alpha 9 at half");

        // A side bar's label starts after the icon.
        let side = Rect { x: 0.0, y: 0.0, w: 150.0, h: 100.0 };
        let vbar = tab_layout::build_vertical(&icon_inputs(&looks), 150.0, 100.0, 0.0, None, &s, &six);
        let mut f = Frame::default();
        let (rects, _) = tab_bar_vertical(&mut f, &looks, 1, &vbar, side, 1.0, 0.1, 20.0, true, false);
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
            tab_bar(&mut f, looks, 0, &bar, area, 1.0, 0.1, 20.0, true, false);
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
        pane_with(&mut f, &s, RECT, CELL, true, false, Some(&Highlights { matches: &m, color: yellow, hover: None, link: yellow, labels: &[], notes: &[], undimmed: false, key_cursor: false }));
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
        pane(&mut f, &s, RECT, CELL, true, false);
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
        let s = screen(b"\x1b]8;;https://x/\x1b\\ab\x1b]8;;\x1b\\ c");
        let mut f = Frame::default();
        pane(&mut f, &s, RECT, CELL, true, false);
        let under: Vec<Rect> = f.overlay.iter().map(|q| q.rect).filter(|r| r.y == CELL.1 - 1.0 && r.h == 1.0).collect();
        assert!(under.len() >= 4, "{under:?}");
        assert!(under.iter().all(|r| r.w < CELL.0 && r.x < 2.0 * CELL.0), "{under:?}");
    }

    #[test]
    fn hover_underlines_in_the_link_color() {
        let s = screen(b"see x.rs");
        let mut f = Frame::default();
        let link = rgb(0x06, 0x45, 0xad);
        let hover = crate::find::Match { start: crate::find::Pos { row: 0, col: 4 }, end: crate::find::Pos { row: 0, col: 7 } };
        let hl = Highlights { matches: &[], color: rgb(0xff, 0xff, 0), hover: Some(hover), link, labels: &[], notes: &[], undimmed: false, key_cursor: false };
        pane_with(&mut f, &s, RECT, CELL, true, false, Some(&hl));
        let q = f.overlay.iter().find(|q| q.color == rgba(link, 1.0)).unwrap();
        assert_eq!(q.rect, Rect { x: 40.0, y: CELL.1 - 1.0, w: 40.0, h: 1.0 });
        let x = f.rows.iter().flat_map(|r| &r.spans).find(|sp| sp.text.starts_with("x.rs")).unwrap();
        assert_eq!(x.color, link);
    }

    #[test]
    fn labels_hide_the_text_under_them() {
        let s = screen(b"see https://x.y now");
        let mut f = Frame::default();
        let labels = [(crate::find::Pos { row: 0, col: 4 }, "1".to_string())];
        let hl = Highlights { matches: &[], color: rgb(0xff, 0xff, 0), hover: None, link: rgb(0, 0, 0xff), labels: &labels, notes: &[], undimmed: false, key_cursor: false };
        pane_with(&mut f, &s, RECT, CELL, true, false, Some(&hl));
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
        let hl = Highlights { matches: &[], color: rgb(0xff, 0xff, 0), hover: None, link: rgb(0, 0, 0xff), labels: &[], notes: &notes, undimmed: false, key_cursor: false };
        pane_with(&mut f, &s, RECT, CELL, true, false, Some(&hl));
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
        timestamps(&mut f, &[(2, "14:02:09".into())], RECT, CELL);
        let r = f.rows.iter().find(|r| r.spans.iter().any(|s| s.text == "14:02:09")).unwrap();
        assert_eq!((r.x, r.y), (800.0 - 90.0, 40.0));
        assert!(f.quads.iter().any(|q| q.rect.x < 710.0 && q.rect.x + q.rect.w == 800.0 && q.rect.y == 40.0));
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
        let hl = Highlights { matches: &[], color: rgb(0xff, 0xff, 0), hover: None, link: rgb(0, 0, 0xff), labels: &[], notes: &notes, undimmed: false, key_cursor: false };
        pane_with(&mut f, &s, rect, CELL, true, false, Some(&hl));
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
        pane(&mut f, &s, RECT, CELL, true, false);
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
    fn a_background_window_is_not_dimmed_unless_asked() {
        let s = screen(b"ab");
        let hl = |undimmed| Highlights { matches: &[], color: rgb(0, 0, 0), hover: None, link: rgb(0, 0, 0), labels: &[], notes: &[], undimmed, key_cursor: false };
        let mut f = Frame::default();
        pane_with(&mut f, &s, RECT, CELL, false, true, Some(&hl(true)));
        assert!(!f.overlay.iter().any(|q| q.rect == RECT), "the key pane of a background window keeps its brightness");
        assert!(!f.quads.iter().any(|q| q.color == rgba(s.palette.cursor, 1.0)), "its cursor is hollow");
        let mut g = Frame::default();
        pane_with(&mut g, &s, RECT, CELL, false, true, Some(&hl(false)));
        assert!(g.overlay.iter().any(|q| q.rect == RECT));
    }

    #[test]
    fn a_password_prompt_draws_the_key_for_the_cursor() {
        let key = crate::pwm_prompt::KEY_GLYPH.to_string();
        let hl = |key_cursor| Highlights { matches: &[], color: rgb(0, 0, 0), hover: None, link: rgb(0, 0, 0), labels: &[], notes: &[], undimmed: false, key_cursor };
        let key_row = |f: &Frame| f.rows.iter().find(|r| r.spans.iter().any(|s| s.icon && s.text == key)).cloned();
        let mut s = screen(b"ab");
        s.palette.bg = rgb(0, 0, 0);
        let mut f = Frame::default();
        pane_with(&mut f, &s, RECT, CELL, true, true, Some(&hl(true)));
        let row = key_row(&f).expect("the key is drawn");
        assert_eq!((row.clip.x, row.clip.y), (20.0, 0.0), "in the cursor's cell");
        assert_eq!(row.spans[0].color, rgb(0xff, 0xff, 0xff), "light on a dark background");
        assert!(!f.quads.iter().chain(&f.overlay).any(|q| q.color == rgba(s.palette.cursor, 1.0)), "no cursor shape");
        s.palette.bg = rgb(0xff, 0xff, 0xff);
        let mut light = Frame::default();
        pane_with(&mut light, &s, RECT, CELL, true, true, Some(&hl(true)));
        assert_eq!(key_row(&light).map(|r| r.spans[0].color), Some(rgb(0, 0, 0)), "dark on a light background");
        let mut blink = Frame::default();
        pane_with(&mut blink, &s, RECT, CELL, true, false, Some(&hl(true)));
        assert!(key_row(&blink).is_none(), "it blinks with the cursor");
        // Over the cell: the character under the cursor is still drawn.
        let on_c = screen(b"abc\x1b[D");
        let mut over = Frame::default();
        pane_with(&mut over, &on_c, RECT, CELL, true, true, Some(&hl(true)));
        assert!(key_row(&over).is_some_and(|r| r.clip.x == 20.0));
        assert_eq!(row_text(&over, 0).trim_end(), "abc", "the key does not blank the cell");
        let s = screen(b"ab");
        let mut plain = Frame::default();
        pane_with(&mut plain, &s, RECT, CELL, true, true, Some(&hl(false)));
        assert!(key_row(&plain).is_none());
        assert!(plain.quads.iter().any(|q| q.color == rgba(s.palette.cursor, 1.0)), "the cursor otherwise");
    }
}
