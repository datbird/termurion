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
    let span = |text: String, color: Rgb| Span { text, color, bold: false, italic: false, alt_font: false, icon: false, prose: false };
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
        let span = Span { text: text.clone(), color: TAB_TEXT, bold: false, italic: false, alt_font: false, icon: false, prose: false };
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
                _ => spans.push(Span { text: c.text.clone(), color: c.fg, bold: c.bold, italic: c.italic, alt_font: !ascii, icon: false, prose: false }),
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
                let span = Span { text: label.clone(), color: LABEL_TEXT, bold: true, italic: false, alt_font: false, icon: false, prose: false };
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
                let span = Span { text: shown, color: rgb(0, 0, 0), bold: false, italic: false, alt_font: false, icon: false, prose: false };
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

fn blend(a: Rgb, b: Rgb, t: f32) -> Rgb {
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
    let text = |t: String, color: Rgb, icon: bool| Span { text: t, color, bold: false, italic: false, alt_font: false, icon, prose: false };
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
            .map(|r| Span { text: r.text.clone(), color: r.color, bold: r.bold, italic: r.italic, alt_font: false, icon: false, prose: r.face == crate::porthole::render::Face::Prose })
            .collect();
        frame.rows.push(TextRow { x: ox + l.x, y, spans, clip: area, scale: l.scale });
    }
    PortholeControls { close, language, wide }
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
                let span = Span { text: c.to_string(), color: mix(a), bold: false, italic: false, alt_font: false, icon: true, prose: false };
                frame.rows.push(TextRow { x: r.x + r.w / 4.0, y: r.y, spans: vec![span], clip: *r, scale: 1.0 });
            }
        }
    }
}

/// One tab's look: its title, its OSC 21337 status text and color, the status dot, and
/// the tab color a program chose.
#[derive(Clone, Debug, Default)]
pub struct TabLook {
    pub title: String,
    pub status: Option<(String, Rgb)>,
    pub dot: Option<Rgb>,
    pub color: Option<Rgb>,
}

pub fn tab_bar(frame: &mut Frame, tabs: &[TabLook], active: usize, area: Rect, cell: (f32, f32)) -> Vec<Rect> {
    frame.quads.push(Quad { rect: area, color: rgba(TAB_BAR, 1.0) });
    let n = tabs.len().max(1) as f32;
    let w = (area.w / n).floor();
    let mut rects = Vec::new();
    for (i, tab) in tabs.iter().enumerate() {
        let title = &tab.title;
        let r = Rect { x: area.x + i as f32 * w, y: area.y, w, h: area.h };
        if i == active {
            frame.quads.push(Quad { rect: r, color: rgba(TAB_ACTIVE, 1.0) });
        }
        if let Some(c) = tab.color {
            frame.quads.push(Quad { rect: Rect { y: r.y + r.h - 3.0, h: 3.0, ..r }, color: rgba(c, 1.0) });
        }
        let mut x = r.x + cell.0;
        if let Some(c) = tab.dot {
            let d = (cell.1 / 2.0).floor();
            frame.quads.push(Quad { rect: Rect { x, y: r.y + ((r.h - d) / 2.0).floor(), w: d, h: d }, color: rgba(c, 1.0) });
            x += d + cell.0 / 2.0;
        }
        let fit = ((r.x + w - x) / cell.0).max(0.0) as usize;
        let fit = fit.saturating_sub(1);
        let color = if i == active { TAB_TEXT } else { TAB_TEXT_DIM };
        let span = |text: String, color: Rgb| Span { text, color, bold: false, italic: false, alt_font: false, icon: false, prose: false };
        let mut spans = Vec::new();
        let mut left = fit;
        if let Some((status, c)) = &tab.status {
            let s: String = format!("{status} ").chars().take(left).collect();
            left -= s.chars().count();
            spans.push(span(s, *c));
        }
        spans.push(span(title.chars().take(left).collect(), color));
        let y = r.y + ((r.h - cell.1) / 2.0).floor();
        frame.rows.push(TextRow { x, y, spans, clip: r, scale: 1.0 });
        rects.push(r);
    }
    rects
}

pub fn dividers(frame: &mut Frame, dividers: &[Divider]) {
    for d in dividers {
        frame.overlay.push(Quad { rect: d.rect, color: rgba(DIVIDER, 1.0) });
    }
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
    let span = |text: String| Span { text, color: LABEL_TEXT, bold: false, italic: false, alt_font: false, icon: false, prose: false };
    let text_end = l.buttons.first().map_or(l.close.x, |b| b.x);
    let clip = Rect { w: (text_end - r.x).max(0.0), ..r };
    frame.rows.push(TextRow { x: r.x + cell.0, y: r.y + 4.0, spans: vec![span(bar.shown_text())], clip, scale: 1.0 });
    for (i, b) in l.buttons.iter().enumerate() {
        frame.quads.push(Quad { rect: *b, color: rgba(BAR_BUTTON, 1.0) });
        frame.rows.push(TextRow { x: b.x + cell.0, y: r.y + 4.0, spans: vec![span(bar.label(i))], clip: *b, scale: 1.0 });
    }
    frame.rows.push(TextRow { x: l.close.x + cell.0 / 2.0, y: r.y + 4.0, spans: vec![span("x".into())], clip: l.close, scale: 1.0 });
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
    let span = Span { text: text.to_string(), color: TAB_TEXT, bold: false, italic: false, alt_font: false, icon: false, prose: false };
    frame.rows.push(TextRow { x: r.x + 2.0 * cell.0, y: r.y + 6.0, spans: vec![span], clip: r, scale: 1.0 });
}

/// A one-line error bar along the bottom of `area`. Its box goes in `quads`, so its
/// text (in `rows`) draws on top.
pub fn banner(frame: &mut Frame, text: &str, area: Rect, cell: (f32, f32)) {
    let r = Rect { x: area.x, y: area.y + area.h - cell.1 - 8.0, w: area.w, h: cell.1 + 8.0 };
    frame.quads.push(Quad { rect: r, color: rgba(BANNER, 1.0) });
    let span = Span { text: text.to_string(), color: rgb(0xff, 0xff, 0xff), bold: false, italic: false, alt_font: false, icon: false, prose: false };
    frame.rows.push(TextRow { x: r.x + cell.0, y: r.y + 4.0, spans: vec![span], clip: r, scale: 1.0 });
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
        let titles: Vec<TabLook> = ["a very long title that cannot fit", "b"].iter().map(|t| TabLook { title: t.to_string(), ..TabLook::default() }).collect();
        let rects = tab_bar(&mut f, &titles, 0, Rect { x: 0.0, y: 0.0, w: 200.0, h: 30.0 }, CELL);
        assert_eq!(rects.len(), 2);
        assert_eq!(rects[1].x, 100.0);
        let first: String = f.rows[0].spans.iter().map(|s| s.text.as_str()).collect();
        assert!(first.chars().count() <= 8, "{first:?}");
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
    fn tab_shows_status_dot() {
        let mut f = Frame::default();
        let green = rgb(0, 0xff, 0);
        let red = rgb(0xff, 0, 0);
        let tabs = vec![
            TabLook { title: "shell".into(), status: Some(("waiting".into(), red)), dot: Some(green), color: Some(rgb(0, 0, 0xff)) },
            TabLook { title: "other".into(), ..TabLook::default() },
        ];
        let rects = tab_bar(&mut f, &tabs, 1, Rect { x: 0.0, y: 0.0, w: 400.0, h: 30.0 }, CELL);
        let inside = |q: &Quad| q.rect.x >= rects[0].x && q.rect.x + q.rect.w <= rects[0].x + rects[0].w;
        assert!(f.quads.iter().any(|q| inside(q) && q.color == rgba(green, 1.0) && q.rect.w < 20.0), "a dot");
        assert!(f.quads.iter().any(|q| inside(q) && q.color == rgba(rgb(0, 0, 0xff), 1.0)), "the tab color");
        let spans: Vec<&Span> = f.rows.iter().filter(|r| r.clip.x < rects[1].x).flat_map(|r| &r.spans).collect();
        assert_eq!(spans.first().map(|s| (s.text.as_str(), s.color)), Some(("waiting ", red)), "status before the title");
        assert!(spans.iter().any(|s| s.text.contains("shell")));
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
