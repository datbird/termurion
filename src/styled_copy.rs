//! Edit > Copy with Styles (HTML and RTF with colors, bold, italic, underline, and links)
//! and Edit > Copy with Control Sequences (the text with iTerm2's SGR codes), from the
//! selection's cells.

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::vte::ansi::{Color, NamedColor, Rgb};

use crate::screen::Screen;

/// A run of cells with one look.
#[derive(Clone, Debug, PartialEq)]
pub struct Run {
    pub text: String,
    pub fg: Rgb,
    pub bg: Rgb,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub link: Option<String>,
}

/// One row of the selection. `hard_end` is false when the row wraps into the next.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub runs: Vec<Run>,
    pub hard_end: bool,
}

/// The selection's rows as `(grid line, first column, last column)`, and whether it is a
/// box.
pub fn selection_spans(screen: &Screen) -> (Vec<(i32, usize, usize)>, bool) {
    let term = screen.term();
    let is_box = term.selection.as_ref().and_then(|s| s.to_range(term)).is_some_and(|r| r.is_block);
    (spans(screen), is_box)
}

/// The selection's rows, first to last: `(line, first column, last column)`.
fn spans(screen: &Screen) -> Vec<(i32, usize, usize)> {
    let term = screen.term();
    let Some(r) = term.selection.as_ref().and_then(|s| s.to_range(term)) else { return Vec::new() };
    let last_col = term.columns() - 1;
    (r.start.line.0..=r.end.line.0)
        .map(|l| {
            if r.is_block {
                (l, r.start.column.0.min(r.end.column.0), r.start.column.0.max(r.end.column.0))
            } else {
                let from = if l == r.start.line.0 { r.start.column.0 } else { 0 };
                let to = if l == r.end.line.0 { r.end.column.0 } else { last_col };
                (l, from, to)
            }
        })
        .collect()
}

fn wraps(screen: &Screen, line: i32) -> bool {
    let g = screen.term().grid();
    g[Line(line)][Column(g.columns() - 1)].flags.contains(Flags::WRAPLINE)
}

/// A cell iTerm2 would hold as empty: a default blank past the text.
fn empty(c: &Cell) -> bool {
    c.c == ' ' && c.fg == Color::Named(NamedColor::Foreground) && c.bg == Color::Named(NamedColor::Background) && c.flags.is_empty() && c.hyperlink().is_none()
}

/// The selection as styled runs, with the pane's colors (bold, dim, reverse, and hidden
/// applied as drawn). Blank cells after a row's text are left out.
pub fn runs(screen: &Screen) -> Vec<Row> {
    runs_over(screen, &spans(screen))
}

/// Styled rows for whole grid lines `first..=last` (grid lines: negative is history).
pub fn runs_of_lines(screen: &Screen, first: i32, last: i32) -> Vec<Row> {
    let cols = screen.term().columns();
    let spans: Vec<(i32, usize, usize)> = (first..=last).map(|l| (l, 0, cols - 1)).collect();
    runs_over(screen, &spans)
}

fn runs_over(screen: &Screen, spans: &[(i32, usize, usize)]) -> Vec<Row> {
    let g = screen.term().grid();
    let palette = &screen.palette;
    let mut out = Vec::new();
    for &(line, from, to) in spans {
        let hard_end = !wraps(screen, line);
        let cells = cells_of(screen, line, from, to, hard_end);
        let mut row: Vec<Run> = Vec::new();
        for c in cells {
            let c = &g[Line(line)][Column(c)];
            let (fg, bg) = palette.cell(c.fg, c.bg, c.flags);
            let mut text = c.c.to_string();
            if let Some(extra) = c.zerowidth() {
                text.extend(extra);
            }
            let run = Run {
                text,
                fg,
                bg,
                bold: c.flags.contains(Flags::BOLD),
                italic: c.flags.contains(Flags::ITALIC),
                underline: c.flags.intersects(Flags::ALL_UNDERLINES),
                link: c.hyperlink().map(|h| h.uri().to_string()),
            };
            match row.last_mut() {
                Some(last) if Run { text: String::new(), ..last.clone() } == Run { text: String::new(), ..run.clone() } => last.text.push_str(&run.text),
                _ => row.push(run),
            }
        }
        out.push(Row { runs: row, hard_end });
    }
    out
}

/// The columns of a selected row that hold text: wide-character spacers are skipped, and
/// at a hard line end the empty cells after the text are left out.
fn cells_of(screen: &Screen, line: i32, from: usize, to: usize, hard_end: bool) -> Vec<usize> {
    let g = screen.term().grid();
    let row = &g[Line(line)];
    let mut end = to;
    if hard_end {
        while end >= from && empty(&row[Column(end)]) {
            if end == 0 {
                return Vec::new();
            }
            end -= 1;
        }
        if end < from {
            return Vec::new();
        }
    }
    (from..=end).filter(|c| !row[Column(*c)].flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)).collect()
}

fn hex(c: Rgb) -> String {
    format!("#{:02x}{:02x}{:02x}", c.r, c.g, c.b)
}

/// The selection as an HTML fragment: a `pre` in the pane's font and background, with a
/// span per run and links as links.
pub fn html(rows: &[Row], font: &str, size_pt: f32, bg: Rgb) -> String {
    let esc = |t: &str| t.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;");
    let mut out = format!("<pre style=\"font-family:'{}';font-size:{}pt;background-color:{}\">", esc(font), size_pt, hex(bg));
    for (i, row) in rows.iter().enumerate() {
        for r in &row.runs {
            let mut style = format!("color:{};background-color:{}", hex(r.fg), hex(r.bg));
            if r.bold {
                style += ";font-weight:bold";
            }
            if r.italic {
                style += ";font-style:italic";
            }
            if r.underline {
                style += ";text-decoration:underline";
            }
            let span = format!("<span style=\"{style}\">{}</span>", esc(&r.text));
            match &r.link {
                Some(url) => out += &format!("<a href=\"{}\">{span}</a>", esc(url)),
                None => out += &span,
            }
        }
        if row.hard_end && i + 1 < rows.len() {
            out.push('\n');
        }
    }
    out + "</pre>"
}

/// Wraps an HTML fragment in the Windows clipboard's "HTML Format" header, whose byte
/// offsets point at the document and the fragment.
pub fn cf_html(fragment: &str) -> String {
    let header = |a: usize, b: usize, c: usize, d: usize| {
        format!("Version:0.9\r\nStartHTML:{a:010}\r\nEndHTML:{b:010}\r\nStartFragment:{c:010}\r\nEndFragment:{d:010}\r\n")
    };
    let head = header(0, 0, 0, 0).len();
    let before = "<html><body>\r\n<!--StartFragment-->";
    let after = "<!--EndFragment-->\r\n</body></html>";
    let start_frag = head + before.len();
    let end_frag = start_frag + fragment.len();
    let end = end_frag + after.len();
    format!("{}{before}{fragment}{after}", header(head, end, start_frag, end_frag))
}

/// The selection as RTF with a color table.
pub fn rtf(rows: &[Row], font: &str, size_pt: f32) -> String {
    let mut colors: Vec<Rgb> = Vec::new();
    let index = |c: Rgb, colors: &mut Vec<Rgb>| match colors.iter().position(|x| *x == c) {
        Some(i) => i + 1,
        None => {
            colors.push(c);
            colors.len()
        }
    };
    let mut body = String::new();
    for (i, row) in rows.iter().enumerate() {
        for r in &row.runs {
            let (f, b) = (index(r.fg, &mut colors), index(r.bg, &mut colors));
            body += &format!("{{\\cf{f}\\chcbpat{b}\\cb{b}");
            if r.bold {
                body += "\\b";
            }
            if r.italic {
                body += "\\i";
            }
            if r.underline {
                body += "\\ul";
            }
            body.push(' ');
            for ch in r.text.chars() {
                match ch {
                    '\\' | '{' | '}' => {
                        body.push('\\');
                        body.push(ch);
                    }
                    c if c.is_ascii() => body.push(c),
                    c => {
                        let mut units = [0u16; 2];
                        for u in c.encode_utf16(&mut units) {
                            body += &format!("\\u{}?", *u as i16);
                        }
                    }
                }
            }
            body.push('}');
        }
        if row.hard_end && i + 1 < rows.len() {
            body += "\\par\n";
        }
    }
    let table: String = colors.iter().map(|c| format!("\\red{}\\green{}\\blue{};", c.r, c.g, c.b)).collect();
    format!("{{\\rtf1\\ansi\\deff0{{\\fonttbl{{\\f0\\fmodern {font};}}}}{{\\colortbl;{table}}}\\f0\\fs{} {body}}}", (size_pt * 2.0).round() as i32)
}

/// Copy with Control Sequences: the text with iTerm2's SGR codes where the look changes,
/// `ESC[0m` and a newline at each hard line end, wrapped rows joined, `ESC[0m` at the end.
/// The whole buffer (history and screen, to the last line with text) as `sgr_text` writes
/// it, for session restoration.
pub fn sgr_all(screen: &Screen) -> String {
    let g = screen.term().grid();
    let top = -(g.history_size() as i32);
    let last_col = g.columns() - 1;
    let Some(last) = (top..g.screen_lines() as i32).rev().find(|l| (0..=last_col).any(|c| !empty(&g[Line(*l)][Column(c)]))) else {
        return String::new();
    };
    sgr_over(screen, &(top..=last).map(|l| (l, 0, last_col)).collect::<Vec<_>>())
}

pub fn sgr_text(screen: &Screen) -> String {
    sgr_over(screen, &spans(screen))
}

fn sgr_over(screen: &Screen, spans: &[(i32, usize, usize)]) -> String {
    let g = screen.term().grid();
    let mut out = String::new();
    let mut last: Option<Vec<String>> = None;
    for (i, (line, from, to)) in spans.iter().enumerate() {
        let hard_end = !wraps(screen, *line);
        for col in cells_of(screen, *line, *from, *to, hard_end) {
            let c = &g[Line(*line)][Column(col)];
            let codes = sgr_codes(c);
            if last.as_ref() != Some(&codes) {
                out += &format!("\x1b[{}m", codes.join(";"));
                last = Some(codes);
            }
            out.push(c.c);
            if let Some(extra) = c.zerowidth() {
                out.extend(extra);
            }
        }
        if hard_end && i + 1 < spans.len() {
            out += "\x1b[0m\n";
            // After the reset the look is the default, and the first cell may say so
            // again only if it differs.
            last = Some(vec!["0".to_string()]);
        }
    }
    out + "\x1b[0m"
}

/// iTerm2's SGR parameters for a cell: 0, foreground, background, 1 2 3, underline, 5 8 7
/// 9, underline color.
pub fn sgr_codes(c: &Cell) -> Vec<String> {
    let mut out = vec!["0".to_string()];
    out.extend(color_code(c.fg, 30, 38));
    out.extend(color_code(c.bg, 40, 48));
    for (flag, code) in [
        (Flags::BOLD, "1"),
        (Flags::DIM, "2"),
        (Flags::ITALIC, "3"),
        (Flags::UNDERLINE, "4"),
        (Flags::UNDERCURL, "4:3"),
        (Flags::DOUBLE_UNDERLINE, "21"),
        (Flags::DOTTED_UNDERLINE, "4:4"),
        (Flags::DASHED_UNDERLINE, "4:5"),
        (Flags::HIDDEN, "8"),
        (Flags::INVERSE, "7"),
        (Flags::STRIKEOUT, "9"),
    ] {
        if c.flags.contains(flag) {
            out.push(code.to_string());
        }
    }
    match c.underline_color() {
        Some(Color::Spec(rgb)) => out.push(format!("58:2:{}:{}:{}", rgb.r, rgb.g, rgb.b)),
        Some(Color::Indexed(i)) => out.push(format!("58:5:{i}")),
        Some(Color::Named(n)) if (n as usize) < 16 => out.push(format!("58:5:{}", n as usize)),
        _ => {}
    }
    out
}

/// A color's SGR parameter: `base` + 0 to 7, `base` + 60 + 0 to 7 for bright, else the
/// `ext` (38 or 48) forms. The default colors have none. Bright backgrounds are 100 to
/// 107 (iTerm2 writes 108 to 115 there, a bug not copied).
fn color_code(c: Color, base: usize, ext: usize) -> Option<String> {
    let index = |i: usize| match i {
        0..=7 => (base + i).to_string(),
        8..=15 => (base + 60 + i - 8).to_string(),
        _ => format!("{ext}:5:{i}"),
    };
    match c {
        Color::Named(n) if (n as usize) < 16 => Some(index(n as usize)),
        Color::Named(_) => None,
        Color::Indexed(i) => Some(index(i as usize)),
        Color::Spec(rgb) => Some(format!("{ext}:2:1:{}:{}:{}", rgb.r, rgb.g, rgb.b)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alacritty_terminal::index::{Point, Side};
    use alacritty_terminal::selection::{Selection, SelectionType};

    fn select(s: &mut Screen, from: (i32, usize), to: (i32, usize)) {
        let mut sel = Selection::new(SelectionType::Simple, Point::new(Line(from.0), Column(from.1)), Side::Left);
        sel.update(Point::new(Line(to.0), Column(to.1)), Side::Right);
        s.term_mut().selection = Some(sel);
    }

    fn rgb(r: u8, g: u8, b: u8) -> Rgb {
        Rgb { r, g, b }
    }

    #[test]
    fn styled_runs_follow_cells() {
        let mut s = Screen::new(12, 3);
        s.feed(b"\x1b[1;31mab\x1b[0m\x1b[7mc\x1b[0m\x1b[8md\x1b[0m\xe4\xb8\x96\x1b]8;;https://x.io\x1b\\L\x1b]8;;\x1b\\");
        select(&mut s, (0, 0), (0, 11));
        let rows = runs(&s);
        assert_eq!(rows.len(), 1);
        let r = &rows[0].runs;
        let p = s.palette.clone();
        assert_eq!(r[0].text, "ab");
        assert!(r[0].bold);
        let bold_red = p.cell(Color::Named(NamedColor::Red), Color::Named(NamedColor::Background), Flags::BOLD).0;
        assert_eq!(r[0].fg, bold_red, "bold as drawn (bright when the profile says so)");
        assert_eq!((r[1].text.as_str(), r[1].fg, r[1].bg), ("c", p.bg, p.fg), "reverse video swaps");
        assert_eq!((r[2].text.as_str(), r[2].fg), ("d", p.bg), "hidden text in the background color");
        assert_eq!(r[3].text, "\u{4e16}", "a wide character once, no spacer");
        assert_eq!((r[4].text.as_str(), r[4].link.as_deref()), ("L", Some("https://x.io")));
        assert_eq!(r.len(), 5, "blank cells after the text are left out");
        assert!(rows[0].hard_end);
    }

    #[test]
    fn html_escapes_and_colors() {
        let row = Row { runs: vec![Run { text: "<b>&\"".into(), fg: rgb(255, 0, 0), bg: rgb(0, 0, 0), bold: true, italic: true, underline: true, link: None }, Run { text: "go".into(), fg: rgb(0, 0, 255), bg: rgb(0, 0, 0), bold: false, italic: false, underline: true, link: Some("https://a.b/?q=1&r=2".into()) }], hard_end: true };
        let h = html(&[row.clone(), row], "Cascadia Mono", 12.0, rgb(0, 0, 0));
        assert!(h.contains("&lt;b&gt;&amp;&quot;"), "{h}");
        assert!(h.contains("color:#ff0000") && h.contains("background-color:#000000"));
        assert!(h.contains("font-weight:bold") && h.contains("font-style:italic") && h.contains("text-decoration:underline"));
        assert!(h.contains("<a href=\"https://a.b/?q=1&amp;r=2\""));
        assert!(h.contains("font-family:'Cascadia Mono'") && h.contains("font-size:12pt"));
        assert_eq!(h.matches('\n').count(), 1, "a newline between rows, none after the last");
        let doc = cf_html(&h);
        let offset = |key: &str| -> usize {
            let at = doc.find(key).unwrap() + key.len();
            doc[at..at + 10].trim().parse().unwrap()
        };
        assert!(doc.starts_with("Version:0.9\r\n"));
        assert!(doc[offset("StartHTML:")..].starts_with("<html>"));
        assert!(doc[offset("StartFragment:")..].starts_with(&h));
        assert_eq!(offset("EndFragment:"), offset("StartFragment:") + h.len());
        assert_eq!(offset("EndHTML:"), doc.len());
    }

    #[test]
    fn rtf_has_a_color_table() {
        let run = |t: &str, fg: Rgb, bold: bool| Run { text: t.into(), fg, bg: rgb(0, 0, 0), bold, italic: false, underline: false, link: None };
        let rows = vec![Row { runs: vec![run("a{b}\\", rgb(255, 0, 0), true), run("\u{e9}\u{4e16}", rgb(255, 0, 0), false)], hard_end: true }];
        let r = rtf(&rows, "Cascadia Mono", 11.0);
        assert!(r.starts_with("{\\rtf1\\ansi"));
        assert!(r.contains("{\\fonttbl{\\f0\\fmodern Cascadia Mono;}}"));
        assert!(r.contains("{\\colortbl;\\red255\\green0\\blue0;\\red0\\green0\\blue0;}"), "{r}");
        assert!(r.contains("\\fs22"));
        assert!(r.contains("\\cf1") && r.contains("\\b "), "{r}");
        assert!(r.contains("a\\{b\\}\\\\"));
        assert!(r.contains("\\u233?\\u19990?"));
        assert!(r.ends_with('}'));
    }

    #[test]
    fn sgr_copy_matches_iterm2() {
        let mut s = Screen::new(12, 3);
        s.feed(b"\x1b[1;31mab\x1b[0m c\x1b[48;5;200mz\x1b[0;101my\x1b[0;38;2;1;2;3;4:3;7mq\x1b[0m");
        select(&mut s, (0, 0), (0, 11));
        assert_eq!(sgr_text(&s), "\x1b[0;31;1mab\x1b[0m c\x1b[0;48:5:200mz\x1b[0;101my\x1b[0;38:2:1:1:2:3;4:3;7mq\x1b[0m");
    }

    #[test]
    fn whole_buffer_sgr_keeps_history() {
        let mut s = Screen::with_scrollback(10, 2, 100);
        s.feed(b"\x1b[31mone\x1b[0m\r\ntwo\r\nthree");
        assert_eq!(sgr_all(&s), "\x1b[0;31mone\x1b[0m\ntwo\x1b[0m\nthree\x1b[0m");
        assert_eq!(sgr_all(&Screen::new(5, 2)), "", "an empty screen saves nothing");
    }

    #[test]
    fn wrapped_rows_join_without_newline() {
        let mut s = Screen::new(4, 4);
        s.feed(b"abcdef\r\ngh");
        select(&mut s, (0, 0), (2, 3));
        assert_eq!(sgr_text(&s), "\x1b[0mabcdef\x1b[0m\ngh\x1b[0m");
        let rows = runs(&s);
        assert_eq!(rows.iter().map(|r| r.hard_end).collect::<Vec<_>>(), vec![false, true, true]);
    }
}
