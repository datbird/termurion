//! Make Screenshot's decisions: the line range, annotation labels and rectangles, how a
//! tall image splits into parts, and file names.

use std::ops::Range;

use crate::layout::Rect;

/// iTerm2's cap on one image's height in pixels (and `MAX_PX^2 / 5` pixels in all).
pub const MAX_PX: usize = 30000;

/// A range of buffer lines, kept inside `0..total` with start <= end.
pub fn clamp_range(start: usize, end: usize, total: usize) -> (usize, usize) {
    if total == 0 {
        return (0, 0);
    }
    let start = start.min(total - 1);
    (start, end.clamp(start, total - 1))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Redact,
    Highlight,
}

/// An annotation's label in the panel's list: `Redact: 3 lines: “...”`.
pub fn label(kind: Kind, lines: &[String]) -> String {
    let head = match kind {
        Kind::Redact => "Redact",
        Kind::Highlight => "Highlight",
    };
    let n = lines.len();
    let q = |t: &str| format!("\u{201c}{t}\u{201d}");
    let body = if lines.iter().all(|l| l.trim().is_empty()) {
        format!("{n} lines")
    } else if n == 1 {
        let t = lines[0].as_str();
        let shown = if t.chars().count() > 20 { format!("{}...", t.chars().take(17).collect::<String>()) } else { t.to_string() };
        format!("{n} lines: {}", q(&shown))
    } else {
        let first: String = lines[0].chars().take(10).collect();
        let last_line = &lines[n - 1];
        let skip = last_line.chars().count().saturating_sub(10);
        let last: String = last_line.chars().skip(skip).collect();
        format!("{n} lines: {}...{}", q(&first), q(&last))
    };
    format!("{head}: {body}")
}

/// The rectangles an annotation covers in the image: one per selected line (`(buffer
/// line, first column, last column)`), or one for a box selection; lines outside `range`
/// are left out. Positions are relative to the image's top-left.
pub fn rects(spans: &[(usize, usize, usize)], is_box: bool, range: (usize, usize), cell: (f32, f32), margin: f32) -> Vec<Rect> {
    let inside: Vec<&(usize, usize, usize)> = spans.iter().filter(|(l, _, _)| *l >= range.0 && *l <= range.1).collect();
    let rect = |line: usize, from: usize, to: usize, rows: usize| Rect {
        x: margin + from as f32 * cell.0,
        y: (line - range.0) as f32 * cell.1,
        w: (to + 1 - from) as f32 * cell.0,
        h: rows as f32 * cell.1,
    };
    match (is_box, inside.first(), inside.last()) {
        (true, Some(first), Some(last)) => vec![rect(first.0, first.1, first.2, last.0 - first.0 + 1)],
        _ => inside.iter().filter(|(_, f, t)| t >= f).map(|(l, f, t)| rect(*l, *f, *t, 1)).collect(),
    }
}

/// How `lines` lines split into images under `max_px` tall and `max_px^2 / 5` pixels.
pub fn parts(lines: usize, line_px: usize, width_px: usize, max_px: usize) -> Vec<Range<usize>> {
    let line_px = line_px.max(1);
    let by_height = max_px / line_px;
    let by_pixels = (max_px * max_px / 5) / (width_px.max(1) * line_px);
    let per = by_height.min(by_pixels).max(1);
    (0..lines).step_by(per).map(|s| s..(s + per).min(lines)).collect()
}

/// A screen holding `count` rows of `from` starting at grid line `first` (negative is
/// history), with its palette and images, and no cursor: one batch of a screenshot.
pub fn slice(from: &crate::screen::Screen, first: i32, count: usize) -> crate::screen::Screen {
    use alacritty_terminal::grid::Dimensions;
    use alacritty_terminal::index::Line;
    let g = from.term().grid();
    let cols = g.columns();
    let (top, bottom) = (-(g.history_size() as i32), g.screen_lines() as i32 - 1);
    let count = count.max(1);
    let look = crate::screen::Look { scrollback: 0, ..crate::screen::Look::default() };
    let mut s = crate::screen::Screen::with_look(cols, count, from.palette.clone(), look);
    s.feed(b"\x1b[?25l");
    let rows = (first..first + count as i32).map(|l| if (top..=bottom).contains(&l) { g[Line(l)].clone() } else { alacritty_terminal::grid::Row::new(cols) }).collect();
    s.splice_rows(0, count as i32 - 1, rows);
    s.images = from.images.clone();
    s
}

/// Batches of at most `max` lines covering `first..=last`.
pub fn batches(first: usize, last: usize, max: usize) -> Vec<(usize, usize)> {
    let max = max.max(1);
    (first..=last).step_by(max).map(|s| (s, (s + max - 1).min(last))).collect()
}

/// The default file name, from the local time.
pub fn file_name(y: u16, mo: u16, d: u16, h: u16, mi: u16, s: u16) -> String {
    format!("Tandem-Screenshot-{y:04}-{mo:02}-{d:02}-{h:02}-{mi:02}-{s:02}.png")
}

/// Part `i` (1-based) of `n` of a screenshot saved as `base` (a name without `.png`).
pub fn part_name(base: &str, i: usize, n: usize) -> String {
    format!("{base}-part-{i}-of-{n}.png")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_clamps() {
        assert_eq!(clamp_range(5, 3, 100), (5, 5));
        assert_eq!(clamp_range(0, 500, 100), (0, 99));
        assert_eq!(clamp_range(200, 300, 100), (99, 99));
        assert_eq!(clamp_range(3, 4, 0), (0, 0));
    }

    #[test]
    fn labels_follow_iterm2() {
        let l = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(label(Kind::Redact, &l(&["", "  "])), "Redact: 2 lines");
        assert_eq!(label(Kind::Highlight, &l(&["password123"])), "Highlight: 1 lines: \u{201c}password123\u{201d}");
        assert_eq!(label(Kind::Redact, &l(&["abcdefghijklmnopqrstuvwxyz"])), "Redact: 1 lines: \u{201c}abcdefghijklmnopq...\u{201d}");
        assert_eq!(label(Kind::Redact, &l(&["first line of text", "middle", "the very last line"])), "Redact: 3 lines: \u{201c}first line\u{201d}...\u{201c} last line\u{201d}");
    }

    #[test]
    fn redaction_rects_per_line() {
        let cell = (10.0, 20.0);
        let spans = [(10, 4, 9), (11, 0, 79), (12, 0, 2)];
        let r = rects(&spans, false, (10, 20), cell, 5.0);
        assert_eq!(r[0], Rect { x: 45.0, y: 0.0, w: 60.0, h: 20.0 });
        assert_eq!(r[1], Rect { x: 5.0, y: 20.0, w: 800.0, h: 20.0 });
        assert_eq!(r[2], Rect { x: 5.0, y: 40.0, w: 30.0, h: 20.0 });
        assert_eq!(rects(&spans, false, (11, 11), cell, 5.0).len(), 1, "clipped to the range");
        let b = rects(&[(10, 2, 4), (11, 2, 4), (12, 2, 4)], true, (10, 20), cell, 0.0);
        assert_eq!(b, vec![Rect { x: 20.0, y: 0.0, w: 30.0, h: 60.0 }], "a box is one rectangle");
    }

    #[test]
    fn parts_split_tall_images() {
        assert_eq!(parts(100, 20, 800, MAX_PX), vec![0..100]);
        // 30000 / 20 = 1500 lines a part by height.
        let p = parts(4000, 20, 800, MAX_PX);
        assert_eq!(p, vec![0..1500, 1500..3000, 3000..4000]);
        // A very wide image: the pixel cap (180 Mpx) wins over the height.
        let wide = parts(4000, 20, 20000, MAX_PX);
        assert_eq!(wide[0], 0..450);
        assert_eq!(parts(0, 20, 800, MAX_PX), Vec::<Range<usize>>::new());
        assert_eq!(parts(3, 50000, 800, MAX_PX), vec![0..1, 1..2, 2..3], "at least one line a part");
    }

    #[test]
    fn a_slice_holds_the_lines() {
        use alacritty_terminal::grid::Dimensions;
        use alacritty_terminal::index::{Column, Line};
        let mut s = crate::screen::Screen::with_scrollback(8, 3, 100);
        for i in 0..6 {
            s.feed(format!("\x1b[31mrow{i}\x1b[0m\r\n").as_bytes());
        }
        let first = -(s.term().grid().history_size() as i32);
        let b = slice(&s, first + 1, 3);
        assert_eq!(b.term().screen_lines(), 3);
        assert_eq!(b.term().columns(), 8);
        let text = |l: i32| (0..8).map(|c| b.term().grid()[Line(l)][Column(c)].c).collect::<String>().trim_end().to_string();
        assert_eq!((text(0), text(1), text(2)), ("row1".to_string(), "row2".to_string(), "row3".to_string()));
        assert_eq!(b.term().grid()[Line(0)][Column(0)].fg, s.term().grid()[Line(first + 1)][Column(0)].fg, "styles come along");
        assert_eq!(b.term().grid().history_size(), 0);
        assert!(!b.term().mode().contains(alacritty_terminal::term::TermMode::SHOW_CURSOR), "no cursor");
        assert_eq!(batches(0, 2499, 1000), vec![(0, 999), (1000, 1999), (2000, 2499)]);
        assert_eq!(batches(5, 5, 1000), vec![(5, 5)]);
    }

    #[test]
    fn names_have_the_time() {
        assert_eq!(file_name(2026, 9, 5, 1, 2, 3), "Tandem-Screenshot-2026-09-05-01-02-03.png");
        assert_eq!(part_name("Tandem-Screenshot-x", 2, 3), "Tandem-Screenshot-x-part-2-of-3.png");
    }
}
