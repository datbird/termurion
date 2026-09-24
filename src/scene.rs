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

/// Draw order: `quads`, then `rows`, then `overlay`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Frame {
    pub clear: [f32; 4],
    pub quads: Vec<Quad>,
    pub rows: Vec<TextRow>,
    pub overlay: Vec<Quad>,
}

const TAB_BAR: Rgb = rgb(0x1e, 0x1e, 0x1e);
const TAB_ACTIVE: Rgb = rgb(0x3a, 0x3a, 0x3a);
const TAB_TEXT: Rgb = rgb(0xc7, 0xc7, 0xc7);
const TAB_TEXT_DIM: Rgb = rgb(0x8a, 0x8a, 0x8a);
const DIVIDER: Rgb = rgb(0x4a, 0x4a, 0x4a);
const BANNER: Rgb = rgb(0x7a, 0x1e, 0x1e);
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

/// Draws one pane's visible screen into `rect`.
/// `cursor_on` is the blink phase: false hides the cursor.
pub fn pane(frame: &mut Frame, screen: &Screen, rect: Rect, cell: (f32, f32), focused: bool, cursor_on: bool) {
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
        if content.selection.as_ref().is_some_and(|s| s.contains(indexed.point)) {
            (fg, bg) = (palette.selected_text.unwrap_or(fg), palette.selection);
        }
        if block_cursor && cursor == Some((row, col)) {
            (fg, bg) = (palette.cursor_text, palette.cursor);
        }
        let mut text = String::new();
        if !c.flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
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
}
