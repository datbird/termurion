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

/// A screen of the chosen buffer lines (0 is the oldest), in order: the minimap's rows.
pub fn pick(from: &crate::screen::Screen, lines: &[usize]) -> crate::screen::Screen {
    use alacritty_terminal::grid::Dimensions;
    use alacritty_terminal::index::Line;
    let g = from.term().grid();
    let (cols, history, rows) = (g.columns(), g.history_size() as i32, g.screen_lines() as i32);
    let count = lines.len().max(1);
    let look = crate::screen::Look { scrollback: 0, ..crate::screen::Look::default() };
    let mut s = crate::screen::Screen::with_look(cols, count, from.palette.clone(), look);
    s.feed(b"\x1b[?25l");
    let picked = lines
        .iter()
        .map(|&i| {
            let l = i as i32 - history;
            if l < rows { g[Line(l)].clone() } else { alacritty_terminal::grid::Row::new(cols) }
        })
        .chain(std::iter::repeat_with(|| alacritty_terminal::grid::Row::new(cols)))
        .take(count)
        .collect();
    s.splice_rows(0, count as i32 - 1, picked);
    s.images = from.images.clone();
    s
}

/// Batches of at most `max` lines covering `first..=last`.
pub fn batches(first: usize, last: usize, max: usize) -> Vec<(usize, usize)> {
    let max = max.max(1);
    (first..=last).step_by(max).map(|s| (s, (s + max - 1).min(last))).collect()
}

/// A frozen copy of a screen with all its scrollback, cursor hidden: what Make Screenshot
/// shows in the pane while its panel is open (output keeps going to the live screen).
pub fn snapshot(from: &crate::screen::Screen) -> crate::screen::Screen {
    use alacritty_terminal::grid::Dimensions;
    use alacritty_terminal::index::Line;
    let g = from.term().grid();
    let (cols, rows, history) = (g.columns(), g.screen_lines(), g.history_size());
    let look = crate::screen::Look { scrollback: history + rows + 1, ..crate::screen::Look::default() };
    let mut s = crate::screen::Screen::with_look(cols, rows, from.palette.clone(), look);
    s.feed(b"\x1b[?25l");
    let all: Vec<_> = (-(history as i32)..rows as i32).map(|l| g[Line(l)].clone()).collect();
    // Splicing in more rows than the screen holds pushes the rest into scrollback.
    s.splice_rows(0, rows as i32 - 1, all);
    s.images = from.images.clone();
    s
}

/// One Redact or Highlight annotation: its label and the selection it covers (buffer
/// line, first column, last column).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Annotation {
    pub kind: Kind,
    pub label: String,
    pub spans: Vec<(usize, usize, usize)>,
    pub is_box: bool,
}

/// How redactions are drawn: one choice for all of them, as in iTerm2.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Method {
    Blur(f32),
    Solid([u8; 3]),
}

/// The panel's state.
#[derive(Clone, Debug)]
pub struct Panel {
    pub total: usize,
    /// The chosen range, 0-based inclusive.
    pub range: (usize, usize),
    pub annotations: Vec<Annotation>,
    pub method: Method,
}

impl Panel {
    /// A panel over `total` lines, starting at the lines that were in view.
    pub fn new(total: usize, visible: (usize, usize)) -> Panel {
        Panel { total, range: clamp_range(visible.0, visible.1, total), annotations: Vec::new(), method: Method::Blur(8.0) }
    }

    /// Adds an annotation from the pane's selection. False (and nothing added) with no
    /// selection; the caller then clears the selection.
    pub fn annotate(&mut self, kind: Kind, spans: Vec<(usize, usize, usize)>, is_box: bool, lines: &[String]) -> bool {
        if spans.is_empty() {
            return false;
        }
        self.annotations.push(Annotation { kind, label: label(kind, lines), spans, is_box });
        true
    }

    /// Sets the range from the 1-based line fields.
    pub fn set_range_1based(&mut self, start: usize, end: usize) {
        self.range = clamp_range(start.saturating_sub(1), end.saturating_sub(1), self.total);
    }
}

/// The default file name, from the local time.
pub fn file_name(y: u16, mo: u16, d: u16, h: u16, mi: u16, s: u16) -> String {
    format!("Tandem-Screenshot-{y:04}-{mo:02}-{d:02}-{h:02}-{mi:02}-{s:02}.png")
}

/// Part `i` (1-based) of `n` of a screenshot saved as `base` (a name without `.png`).
/// The files a save writes to: the chosen path itself for one part (`.png` added when it
/// has another ending), else numbered parts named from it without its `.png`.
pub fn save_paths(chosen: &std::path::Path, n: usize) -> Vec<std::path::PathBuf> {
    let name = chosen.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
    let is_png = name.len() > 4 && name[name.len() - 4..].eq_ignore_ascii_case(".png");
    if n <= 1 {
        return vec![if is_png { chosen.to_path_buf() } else { chosen.with_file_name(format!("{name}.png")) }];
    }
    let stem = if is_png { &name[..name.len() - 4] } else { &name[..] };
    (1..=n).map(|i| chosen.with_file_name(part_name(stem, i, n))).collect()
}

pub fn part_name(base: &str, i: usize, n: usize) -> String {
    format!("{base}-part-{i}-of-{n}.png")
}

/// A `w` by `h` RGBA image as PNG bytes; Err when `rgba` is not that size.
pub fn png(w: u32, h: u32, rgba: Vec<u8>) -> Result<Vec<u8>, String> {
    let img = image::RgbaImage::from_raw(w, h, rgba).ok_or("the image had the wrong size")?;
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).map_err(|e| e.to_string())?;
    Ok(out.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png_encodes_rgba() {
        let px = vec![255, 0, 0, 255, 0, 255, 0, 255];
        let bytes = png(2, 1, px.clone()).unwrap();
        assert_eq!(bytes[..4], [0x89, b'P', b'N', b'G']);
        let back = image::load_from_memory(&bytes).unwrap().to_rgba8();
        assert_eq!(back.dimensions(), (2, 1));
        assert_eq!(back.into_raw(), px);
        assert!(png(2, 2, vec![0; 8]).is_err());
    }

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
        let history = s.term().grid().history_size();
        let m = pick(&s, &[0, 2, history + 1]);
        let mtext = |l: i32| (0..8).map(|c| m.term().grid()[Line(l)][Column(c)].c).collect::<String>().trim_end().to_string();
        assert_eq!((mtext(0), mtext(1), mtext(2)), ("row0".to_string(), "row2".to_string(), s.term().grid()[Line(1)].clone().into_iter().map(|c| c.c).collect::<String>().trim_end().to_string()));
        assert_eq!(batches(5, 5, 1000), vec![(5, 5)]);
    }

    #[test]
    fn panel_state_follows_selection() {
        let mut p = Panel::new(500, (480, 499));
        assert_eq!(p.range, (480, 499), "starts at the lines in view");
        assert_eq!(p.method, Method::Blur(8.0));
        assert!(!p.annotate(Kind::Redact, vec![], false, &[]), "no selection, nothing added");
        assert!(p.annotate(Kind::Redact, vec![(481, 0, 4)], false, &["hello".to_string()]));
        assert_eq!(p.annotations[0].label, "Redact: 1 lines: \u{201c}hello\u{201d}");
        p.set_range_1based(0, 900);
        assert_eq!(p.range, (0, 499), "1-based fields, clamped");
        p.set_range_1based(10, 3);
        assert_eq!(p.range, (9, 9));
    }

    #[test]
    fn snapshot_copies_scrollback_and_hides_the_cursor() {
        use alacritty_terminal::grid::Dimensions;
        use alacritty_terminal::index::{Column, Line};
        let mut s = crate::screen::Screen::with_scrollback(8, 3, 100);
        for i in 0..10 {
            s.feed(format!("\x1b[32mr{i}\x1b[0m\r\n").as_bytes());
        }
        s.feed(b"end");
        let snap = snapshot(&s);
        let g = snap.term().grid();
        assert_eq!(g.history_size(), s.term().grid().history_size());
        let text = |l: i32| (0..8).map(|c| g[Line(l)][Column(c)].c).collect::<String>().trim_end().to_string();
        assert_eq!(text(-(g.history_size() as i32)), "r0");
        assert_eq!(text(2), "end");
        assert_eq!(g[Line(0)][Column(0)].fg, s.term().grid()[Line(0)][Column(0)].fg);
        assert!(!snap.term().mode().contains(alacritty_terminal::term::TermMode::SHOW_CURSOR));
        s.feed(b"\r\nmore output");
        assert_eq!(text(2), "end", "the copy does not follow new output");
    }

    #[test]
    fn names_have_the_time() {
        assert_eq!(file_name(2026, 9, 5, 1, 2, 3), "Tandem-Screenshot-2026-09-05-01-02-03.png");
        assert_eq!(part_name("Tandem-Screenshot-x", 2, 3), "Tandem-Screenshot-x-part-2-of-3.png");
    }

    #[test]
    fn save_paths_keep_the_chosen_name() {
        use std::path::{Path, PathBuf};
        assert_eq!(save_paths(Path::new("C:/a/build.v2.png"), 1), vec![PathBuf::from("C:/a/build.v2.png")]);
        assert_eq!(save_paths(Path::new("C:/a/shot"), 1), vec![PathBuf::from("C:/a/shot.png")]);
        assert_eq!(save_paths(Path::new("C:/a/build.v2.PNG"), 2), vec![PathBuf::from("C:/a/build.v2-part-1-of-2.png"), PathBuf::from("C:/a/build.v2-part-2-of-2.png")]);
    }
}
