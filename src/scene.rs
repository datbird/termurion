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
}

#[derive(Clone, Debug, PartialEq)]
pub struct TextRow {
    pub x: f32,
    pub y: f32,
    pub spans: Vec<Span>,
    pub clip: Rect,
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
}

/// Draw order: `quads`, then `images`, then `rows`, then `overlay`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Frame {
    pub clear: [f32; 4],
    pub quads: Vec<Quad>,
    pub images: Vec<ImageQuad>,
    pub rows: Vec<TextRow>,
    pub overlay: Vec<Quad>,
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
    })
}

const TAB_BAR: Rgb = rgb(0x1e, 0x1e, 0x1e);
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
const FIND_ERROR: Rgb = rgb(0xff, 0x6b, 0x6b);
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
    let span = |text: String, color: Rgb| Span { text, color, bold: false, italic: false, alt_font: false };
    let y = r.y + 4.0;
    let query_color = if bar.error { FIND_ERROR } else { TAB_TEXT };
    frame.rows.push(TextRow {
        x: r.x + cw,
        y,
        spans: vec![span(format!("{field:<FIELD$}  {}", bar.count), query_color)],
        clip: r,
    });
    let mode_x = r.x + (1 + FIELD + 2 + bar.count.chars().count() + 2) as f32 * cw;
    let mode_rect = Rect { x: mode_x, y: r.y, w: mode.chars().count() as f32 * cw, h: r.h };
    frame.rows.push(TextRow { x: mode_x, y, spans: vec![span(mode, TAB_TEXT_DIM)], clip: r });
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
        let span = Span { text: text.clone(), color: TAB_TEXT, bold: false, italic: false, alt_font: false };
        frame.rows.push(TextRow { x: right - (n + 1.0) * cw, y, spans: vec![span], clip: rect });
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
fn contrasting(bg: Rgb) -> Rgb {
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
    let block_cursor = focused && content.cursor.shape == CursorShape::Block;

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
                (fg, bg) = (contrasting(h.color), h.color);
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
                    frame.rows.push(TextRow { x: rect.x + start as f32 * cw, y, spans, clip: rect });
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
                _ => spans.push(Span { text: c.text.clone(), color: c.fg, bold: c.bold, italic: c.italic, alt_font: !ascii }),
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
                let span = Span { text: label.clone(), color: LABEL_TEXT, bold: true, italic: false, alt_font: false };
                frame.rows.push(TextRow { x: rect.x + p.col as f32 * cw, y: rect.y + row as f32 * ch, spans: vec![span], clip: rect });
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
                let span = Span { text: shown, color: rgb(0, 0, 0), bold: false, italic: false, alt_font: false };
                frame.rows.push(TextRow { x: x + cw * 0.5, y, spans: vec![span], clip: rect });
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

    if let Some((row, col)) = cursor {
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

    if !focused {
        frame.overlay.push(Quad { rect, color: rgba(palette.bg, DIM_ALPHA) });
    }
}

/// Draws the tab bar across `area`. Returns each tab's rectangle, for clicks.
/// iTerm2's mark colors (iTermTextDrawingHelper), in sRGB.
fn mark_rgba(c: crate::marks::MarkColor) -> [f32; 4] {
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

pub fn tab_bar(frame: &mut Frame, titles: &[String], active: usize, area: Rect, cell: (f32, f32)) -> Vec<Rect> {
    frame.quads.push(Quad { rect: area, color: rgba(TAB_BAR, 1.0) });
    let n = titles.len().max(1) as f32;
    let w = (area.w / n).floor();
    let mut rects = Vec::new();
    for (i, title) in titles.iter().enumerate() {
        let r = Rect { x: area.x + i as f32 * w, y: area.y, w, h: area.h };
        if i == active {
            frame.quads.push(Quad { rect: r, color: rgba(TAB_ACTIVE, 1.0) });
        }
        let fit = ((w / cell.0) as usize).saturating_sub(2);
        let text: String = title.chars().take(fit).collect();
        let color = if i == active { TAB_TEXT } else { TAB_TEXT_DIM };
        let y = r.y + ((r.h - cell.1) / 2.0).floor();
        frame.rows.push(TextRow { x: r.x + cell.0, y, spans: vec![Span { text, color, bold: false, italic: false, alt_font: false }], clip: r });
        rects.push(r);
    }
    rects
}

pub fn dividers(frame: &mut Frame, dividers: &[Divider]) {
    for d in dividers {
        frame.overlay.push(Quad { rect: d.rect, color: rgba(DIVIDER, 1.0) });
    }
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
    let span = |text: String| Span { text, color: LABEL_TEXT, bold: false, italic: false, alt_font: false };
    let text_end = l.buttons.first().map_or(l.close.x, |b| b.x);
    let clip = Rect { w: (text_end - r.x).max(0.0), ..r };
    frame.rows.push(TextRow { x: r.x + cell.0, y: r.y + 4.0, spans: vec![span(bar.text.clone())], clip });
    for (label, b) in bar.buttons.iter().zip(&l.buttons) {
        frame.quads.push(Quad { rect: *b, color: rgba(BAR_BUTTON, 1.0) });
        frame.rows.push(TextRow { x: b.x + cell.0, y: r.y + 4.0, spans: vec![span(label.clone())], clip: *b });
    }
    frame.rows.push(TextRow { x: l.close.x + cell.0 / 2.0, y: r.y + 4.0, spans: vec![span("x".into())], clip: l.close });
}

/// A one-line error bar along the bottom of `area`. Its box goes in `quads`, so its
/// text (in `rows`) draws on top.
pub fn banner(frame: &mut Frame, text: &str, area: Rect, cell: (f32, f32)) {
    let r = Rect { x: area.x, y: area.y + area.h - cell.1 - 8.0, w: area.w, h: cell.1 + 8.0 };
    frame.quads.push(Quad { rect: r, color: rgba(BANNER, 1.0) });
    let span = Span { text: text.to_string(), color: rgb(0xff, 0xff, 0xff), bold: false, italic: false, alt_font: false };
    frame.rows.push(TextRow { x: r.x + cell.0, y: r.y + 4.0, spans: vec![span], clip: r });
}

#[cfg(test)]
mod tests {

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
    fn tab_titles_are_cut_to_fit() {
        let mut f = Frame::default();
        let titles = vec!["a very long title that cannot fit".to_string(), "b".to_string()];
        let rects = tab_bar(&mut f, &titles, 0, Rect { x: 0.0, y: 0.0, w: 200.0, h: 30.0 }, CELL);
        assert_eq!(rects.len(), 2);
        assert_eq!(rects[1].x, 100.0);
        let first: String = f.rows[0].spans.iter().map(|s| s.text.as_str()).collect();
        assert!(first.chars().count() <= 8, "{first:?}");
    }

    #[test]
    fn match_highlights_use_the_match_color() {
        let s = screen(b"xx ab ab");
        let m = crate::find::find_all(&s, "ab").unwrap();
        let mut f = Frame::default();
        let yellow = rgb(0xff, 0xff, 0x00);
        pane_with(&mut f, &s, RECT, CELL, true, false, Some(&Highlights { matches: &m, color: yellow, hover: None, link: yellow, labels: &[], notes: &[] }));
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
        let hl = Highlights { matches: &[], color: rgb(0xff, 0xff, 0), hover: Some(hover), link, labels: &[], notes: &[] };
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
        let hl = Highlights { matches: &[], color: rgb(0xff, 0xff, 0), hover: None, link: rgb(0, 0, 0xff), labels: &labels, notes: &[] };
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
        let hl = Highlights { matches: &[], color: rgb(0xff, 0xff, 0), hover: None, link: rgb(0, 0, 0xff), labels: &[], notes: &notes };
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
        let hl = Highlights { matches: &[], color: rgb(0xff, 0xff, 0), hover: None, link: rgb(0, 0, 0xff), labels: &[], notes: &notes };
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
}
