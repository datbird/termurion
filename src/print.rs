//! Shell > Print: the styled text of the screen, the selection, or the whole buffer, laid
//! out on pages the way iTerm2 prints it: at the terminal's width, scaled down to fit the
//! page width (never up), broken between lines, with no header or footer.

use std::ops::Range;

use crate::screen::Screen;
use crate::styled_copy::Row;

/// What Print prints (iTerm2's menu tags 0, 1, 2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum What {
    Screen,
    Selection,
    Buffer,
}

/// An escape-sequence print job's text as plain rows (CR, LF and CRLF end a row).
pub fn text_rows(text: &str, fg: alacritty_terminal::vte::ansi::Rgb, bg: alacritty_terminal::vte::ansi::Rgb) -> Vec<Row> {
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    text.split('\n')
        .map(|line| Row { runs: vec![crate::styled_copy::Run { text: line.to_string(), fg, bg, bold: false, italic: false, underline: false, link: None }], hard_end: true })
        .collect()
}

/// The styled rows to print.
pub fn rows(screen: &Screen, what: What) -> Vec<Row> {
    use alacritty_terminal::grid::Dimensions;
    let g = screen.term().grid();
    let lines = g.screen_lines() as i32;
    match what {
        What::Selection => crate::styled_copy::runs(screen),
        What::Screen => {
            let top = -(g.display_offset() as i32);
            crate::styled_copy::runs_of_lines(screen, top, top + lines - 1)
        }
        What::Buffer => crate::styled_copy::runs_of_lines(screen, -(g.history_size() as i32), lines - 1),
    }
}

/// Where each page's rows come from, and the scale the text is drawn at.
#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    pub scale: f32,
    pub pages: Vec<Range<usize>>,
}

/// Lays `rows` rows of `cols` columns (cells of `cell` size) on pages of `page` size (all
/// in the printer's units).
pub fn layout(rows: usize, cols: usize, cell: (f32, f32), page: (f32, f32)) -> Layout {
    let width = cols as f32 * cell.0;
    let scale = if width > page.0 && width > 0.0 { page.0 / width } else { 1.0 };
    let row_h = (cell.1 * scale).max(f32::MIN_POSITIVE);
    let per_page = ((page.1 / row_h).floor() as usize).max(1);
    let pages = (0..rows).step_by(per_page).map(|start| start..(start + per_page).min(rows)).collect();
    Layout { scale, pages }
}

/// Black and white: the text in black on the white page, with no colors.
pub fn black_and_white(rows: &mut [Row]) {
    use alacritty_terminal::vte::ansi::Rgb;
    for run in rows.iter_mut().flat_map(|r| r.runs.iter_mut()) {
        run.fg = Rgb { r: 0, g: 0, b: 0 };
        run.bg = Rgb { r: 255, g: 255, b: 255 };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alacritty_terminal::grid::Scroll;

    /// 13o Task 7: a print job's text is rows.
    #[test]
    fn text_rows_split_on_line_ends() {
        use alacritty_terminal::vte::ansi::Rgb;
        let c = Rgb { r: 1, g: 2, b: 3 };
        let rows = text_rows("bc\r\nd\re\nf", c, c);
        let got: Vec<&str> = rows.iter().map(|r| r.runs[0].text.as_str()).collect();
        assert_eq!(got, ["bc", "d", "e", "f"]);
    }
    use alacritty_terminal::index::{Column, Line, Point, Side};
    use alacritty_terminal::selection::{Selection, SelectionType};
    use alacritty_terminal::vte::ansi::Rgb;

    fn text(rows: &[Row]) -> Vec<String> {
        rows.iter().map(|r| r.runs.iter().map(|x| x.text.as_str()).collect()).collect()
    }

    #[test]
    fn print_rows_follow_iterm2() {
        let mut s = Screen::with_scrollback(10, 3, 100);
        for i in 0..6 {
            s.feed(format!("line{i}\r\n").as_bytes());
        }
        s.feed(b"\x1b[31mred\x1b[0m");
        assert_eq!(text(&rows(&s, What::Screen)), vec!["line4", "line5", "red"], "the rows in view");
        s.term_mut().scroll_display(Scroll::Delta(2));
        assert_eq!(text(&rows(&s, What::Screen)), vec!["line2", "line3", "line4"], "what is scrolled into view, not the bottom");
        assert_eq!(text(&rows(&s, What::Buffer)).len(), 7, "scrollback and screen");
        assert_eq!(text(&rows(&s, What::Buffer))[0], "line0");
        let mut sel = Selection::new(SelectionType::Simple, Point::new(Line(2), Column(0)), Side::Left);
        sel.update(Point::new(Line(2), Column(2)), Side::Right);
        s.term_mut().selection = Some(sel);
        let picked = rows(&s, What::Selection);
        assert_eq!(text(&picked), vec!["red"]);
        assert_eq!(picked[0].runs[0].fg, s.palette.ansi[1]);
    }

    #[test]
    fn pages_break_between_lines() {
        // 10 rows of 10 px on pages 35 px tall: 3 rows a page.
        let l = layout(10, 8, (5.0, 10.0), (100.0, 35.0));
        assert_eq!(l.pages, vec![0..3, 3..6, 6..9, 9..10]);
        assert_eq!(layout(0, 8, (5.0, 10.0), (100.0, 35.0)).pages, Vec::<Range<usize>>::new());
        // A page shorter than a row still takes one row.
        assert_eq!(layout(2, 8, (5.0, 10.0), (100.0, 4.0)).pages, vec![0..1, 1..2]);
    }

    #[test]
    fn scale_fits_never_grows() {
        assert_eq!(layout(4, 80, (10.0, 20.0), (400.0, 1000.0)).scale, 0.5, "800 wide on a 400 page");
        let l = layout(4, 10, (10.0, 20.0), (400.0, 1000.0));
        assert_eq!(l.scale, 1.0, "never scaled up");
        let small = layout(100, 80, (10.0, 20.0), (400.0, 100.0));
        assert_eq!(small.pages[0], 0..10, "scaled rows are shorter, so more fit");
    }

    #[test]
    fn black_and_white_drops_colors() {
        let mut s = Screen::new(10, 2);
        s.feed(b"\x1b[31;44mab");
        let mut r = rows(&s, What::Screen);
        black_and_white(&mut r);
        let run = &r[0].runs[0];
        assert_eq!((run.fg, run.bg), (Rgb { r: 0, g: 0, b: 0 }, Rgb { r: 255, g: 255, b: 255 }));
    }
}
