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
    /// The cell's background is the profile's default (`Named(Background)`, not reversed): what
    /// Copy with Styles leaves unset with `CopyBackgroundColor` off.
    pub default_bg: bool,
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

/// The selection's rows, first to last: `(line, first column, last column)`. A visual drag
/// selection (`Selection::runs`) gives one span per run, in reading order; several runs on a
/// line follow each other and make one row (`same_line`). Every reader of the selected cells
/// goes through here, so the highlight, the plain copy and every other copy agree.
fn spans(screen: &Screen) -> Vec<(i32, usize, usize)> {
    let term = screen.term();
    // Extra selections (Win-drag, Select Matches): every selection's runs in reading order, as
    // the plain copy enumerates them (`seltext::ranges`: an overlap cancels).
    if !screen.sel.extras.is_empty() {
        let (width, history) = (term.columns(), term.grid().history_size() as i32);
        let mut out = Vec::new();
        for (a, b, _) in crate::seltext::ranges(&screen.subs(), width) {
            for row in a.row..=b.row {
                let from = if row == a.row { a.col } else { 0 };
                let to = if row == b.row { b.col } else { width - 1 };
                out.push((row as i32 - history, from, to));
            }
        }
        return out;
    }
    if let Some(sel) = term.selection.as_ref().filter(|s| !s.runs.is_empty()) {
        let top = term.topmost_line().0;
        return sel.runs.iter().filter(|(a, _)| a.line.0 >= top).map(|(a, b)| (a.line.0, a.column.0, b.column.0)).collect();
    }
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

/// Whether span `i` and the next are runs of one line (they make one row).
fn same_line(spans: &[(i32, usize, usize)], i: usize) -> bool {
    spans.get(i + 1).is_some_and(|n| n.0 == spans[i].0)
}

fn wraps(screen: &Screen, line: i32) -> bool {
    wraps_in(screen.term().grid(), line)
}

fn wraps_in(g: &alacritty_terminal::grid::Grid<Cell>, line: i32) -> bool {
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

/// Styled rows from `start` to `end` (grid lines and columns), the end column excluded and
/// its row not trimmed (iTerm2's contentInRange, for the Auto Composer's prompt): the blank
/// cells a prompt ends with stay. Rows before the last run to their line's end.
pub fn runs_of_range(screen: &Screen, start: (i32, usize), end: (i32, usize)) -> Vec<Row> {
    let cols = screen.term().columns();
    let spans: Vec<(i32, usize, usize)> = (start.0..=end.0).map(|l| (l, if l == start.0 { start.1 } else { 0 }, if l == end.0 { end.1 } else { cols })).collect();
    runs_spans(screen, &spans, true)
}

fn runs_over(screen: &Screen, spans: &[(i32, usize, usize)]) -> Vec<Row> {
    runs_spans(screen, spans, false)
}

/// `exact_last`: the last span's end column is excluded and its row kept as is.
fn runs_spans(screen: &Screen, spans: &[(i32, usize, usize)], exact_last: bool) -> Vec<Row> {
    let g = screen.term().grid();
    let palette = &screen.palette;
    let mut out = Vec::new();
    let mut row: Vec<Run> = Vec::new();
    for (i, &(line, from, to)) in spans.iter().enumerate() {
        let hard_end = !wraps(screen, line);
        let cells = if exact_last && i + 1 == spans.len() {
            let row = &g[Line(line)];
            (from..to.min(g.columns())).filter(|c| !row[Column(*c)].flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)).collect()
        } else {
            // A run followed by another on its line keeps its trailing blanks: they are selected.
            cells_of(screen, line, from, to.min(g.columns() - 1), hard_end && !same_line(spans, i))
        };
        for c in cells {
            let c = &g[Line(line)][Column(c)];
            let (fg, bg) = palette.cell(c.fg, c.bg, c.flags);
            let mut text = c.c.to_string();
            if let Some(extra) = c.zerowidth() {
                text.extend(extra);
            }
            crate::encoding::normalize_in_place(&mut text, screen.normalization);
            let run = Run {
                text,
                fg,
                bg,
                bold: c.flags.contains(Flags::BOLD),
                italic: c.flags.contains(Flags::ITALIC),
                underline: c.flags.intersects(Flags::ALL_UNDERLINES),
                link: c.hyperlink().map(|h| h.uri().to_string()),
                default_bg: c.bg == Color::Named(NamedColor::Background) && !c.flags.contains(Flags::INVERSE),
            };
            match row.last_mut() {
                Some(last) if Run { text: String::new(), ..last.clone() } == Run { text: String::new(), ..run.clone() } => last.text.push_str(&run.text),
                _ => row.push(run),
            }
        }
        if !same_line(spans, i) {
            out.push(Row { runs: std::mem::take(&mut row), hard_end });
        }
    }
    out
}

/// The columns of a selected row that hold text: wide-character spacers are skipped, and
/// at a hard line end the empty cells after the text are left out.
fn cells_of(screen: &Screen, line: i32, from: usize, to: usize, hard_end: bool) -> Vec<usize> {
    cells_in(screen.term().grid(), line, from, to, hard_end)
}

fn cells_in(g: &alacritty_terminal::grid::Grid<Cell>, line: i32, from: usize, to: usize, hard_end: bool) -> Vec<usize> {
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

/// How Copy with Styles treats backgrounds (iTerm2's `CopyBackgroundColor` and
/// `ExcludeBackgroundColorsFromCopiedStyle`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StyleOptions {
    pub copy_background: bool,
    pub exclude_backgrounds: bool,
}

impl StyleOptions {
    pub fn from_globals(g: &crate::globals::Globals) -> StyleOptions {
        StyleOptions { copy_background: g.bool("CopyBackgroundColor"), exclude_backgrounds: g.bool("ExcludeBackgroundColorsFromCopiedStyle") }
    }
}

/// The most HTML or RTF a copy writes; the rest of a huge selection is left out.
pub const MAX_STYLED_BYTES: usize = 8 << 20;

fn hex(c: Rgb) -> String {
    format!("#{:02x}{:02x}{:02x}", c.r, c.g, c.b)
}

/// Text for an HTML body or attribute: markup characters escaped (quotes of both kinds for
/// attribute values), control characters other than tab dropped.
fn esc(t: &str) -> String {
    let mut out = String::with_capacity(t.len());
    for c in t.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            '\t' => out.push(c),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

/// A font name that is safe inside a quoted CSS value and an RTF font table: quotes,
/// backslashes, semicolons, braces, angle brackets, ampersands and controls are dropped.
fn font_name(f: &str) -> String {
    f.chars().filter(|c| !c.is_control() && !"'\"\\;{}<>&".contains(*c)).collect()
}

fn point_size(pt: f32) -> f32 {
    if pt.is_finite() && pt > 0.0 { pt.min(1000.0) } else { 12.0 }
}

/// A link target a copied document may carry: http, https and mailto only (a program-authored
/// `javascript:` or `file:` target never reaches the clipboard), of sane length.
fn safe_link(url: &str) -> Option<&str> {
    (url.len() <= 4096 && crate::semantic::web_link(url)).then_some(url)
}

/// The background a run is written with, if any: none when every background is excluded, none
/// for a default background with `CopyBackgroundColor` off (`CharacterAttributes.swift:114-117`).
fn run_bg(r: &Run, opts: StyleOptions) -> Option<Rgb> {
    (!opts.exclude_backgrounds && (opts.copy_background || !r.default_bg)).then_some(r.bg)
}

/// The selection as an HTML fragment: a `pre` in the pane's font and background, with a
/// span per run and links as links. Output stops at `MAX_STYLED_BYTES`.
pub fn html(rows: &[Row], font: &str, size_pt: f32, bg: Rgb, opts: StyleOptions) -> String {
    let mut out = format!("<pre style=\"font-family:'{}';font-size:{}pt", font_name(font), point_size(size_pt));
    if opts.copy_background && !opts.exclude_backgrounds {
        out += &format!(";background-color:{}", hex(bg));
    }
    out += "\">";
    'rows: for (i, row) in rows.iter().enumerate() {
        for r in &row.runs {
            if out.len() > MAX_STYLED_BYTES {
                break 'rows;
            }
            let mut style = format!("color:{}", hex(r.fg));
            if let Some(b) = run_bg(r, opts) {
                style += &format!(";background-color:{}", hex(b));
            }
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
            match r.link.as_deref().and_then(safe_link) {
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
pub fn rtf(rows: &[Row], font: &str, size_pt: f32, opts: StyleOptions) -> String {
    let mut colors: Vec<Rgb> = Vec::new();
    let index = |c: Rgb, colors: &mut Vec<Rgb>| match colors.iter().position(|x| *x == c) {
        Some(i) => i + 1,
        None => {
            colors.push(c);
            colors.len()
        }
    };
    let mut body = String::new();
    'rows: for (i, row) in rows.iter().enumerate() {
        for r in &row.runs {
            if body.len() > MAX_STYLED_BYTES {
                break 'rows;
            }
            let f = index(r.fg, &mut colors);
            body += &format!("{{\\cf{f}");
            if let Some(bg) = run_bg(r, opts) {
                let b = index(bg, &mut colors);
                body += &format!("\\chcbpat{b}\\cb{b}");
            }
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
                    c if c.is_ascii_control() => {}
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
    format!("{{\\rtf1\\ansi\\deff0{{\\fonttbl{{\\f0\\fmodern {};}}}}{{\\colortbl;{table}}}\\f0\\fs{} {body}}}", font_name(font), (point_size(size_pt) * 2.0).round() as i32)
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

/// Session restoration's contents: the main screen's history and every row of its screen
/// (blank rows too, so the rows line up with the shell's own view), and its cursor as
/// (row, column) on the screen.
pub fn sgr_restorable(screen: &Screen) -> (String, (usize, usize)) {
    let g = screen.term().primary_grid();
    let top = -(g.history_size() as i32);
    let spans: Vec<_> = (top..g.screen_lines() as i32).map(|l| (l, 0, g.columns() - 1)).collect();
    let at = g.cursor.point;
    (sgr_grid(g, &spans, crate::encoding::Form::None), (at.line.0.max(0) as usize, at.column.0))
}

pub fn sgr_text(screen: &Screen) -> String {
    sgr_over(screen, &spans(screen))
}

fn sgr_over(screen: &Screen, spans: &[(i32, usize, usize)]) -> String {
    sgr_grid(screen.term().grid(), spans, screen.normalization)
}

/// The cells' text in `form`: Copy with Control Sequences normalizes; session restoration (None)
/// keeps what the program wrote.
fn sgr_grid(g: &alacritty_terminal::grid::Grid<Cell>, spans: &[(i32, usize, usize)], form: crate::encoding::Form) -> String {
    let mut out = String::new();
    let mut last: Option<Vec<String>> = None;
    for (i, (line, from, to)) in spans.iter().enumerate() {
        let hard_end = !wraps_in(g, *line);
        let more = same_line(spans, i);
        for col in cells_in(g, *line, *from, *to, hard_end && !more) {
            let c = &g[Line(*line)][Column(col)];
            let codes = sgr_codes(c);
            if last.as_ref() != Some(&codes) {
                out += &format!("\x1b[{}m", codes.join(";"));
                last = Some(codes);
            }
            let mut text = c.c.to_string();
            if let Some(extra) = c.zerowidth() {
                text.extend(extra);
            }
            crate::encoding::normalize_in_place(&mut text, form);
            out.push_str(&text);
        }
        if hard_end && !more && i + 1 < spans.len() {
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
        (Flags::BLINK, "5"),
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

    const DEFAULT_OPTS: StyleOptions = StyleOptions { copy_background: true, exclude_backgrounds: false };

    fn rgb(r: u8, g: u8, b: u8) -> Rgb {
        Rgb { r, g, b }
    }

    /// Copy, Copy with Styles and Copy with Control Sequences carry the profile's form, per
    /// cell: the selection runs and the cells they cover stay the same.
    #[test]
    fn copies_follow_the_normalization_form() {
        let look = crate::screen::Look { normalization: crate::encoding::Form::Nfc, ..crate::screen::Look::default() };
        let mut s = Screen::with_look(12, 2, crate::palette::Palette::default(), look);
        s.feed("e\u{301}x \u{5d0}\u{5d1}".as_bytes());
        select(&mut s, (0, 0), (0, 1));
        assert_eq!(s.selection_text().as_deref(), Some("\u{e9}x"));
        assert_eq!(runs(&s)[0].runs[0].text, "\u{e9}x");
        assert!(sgr_text(&s).contains("\u{e9}x"), "{:?}", sgr_text(&s));
        // The Hebrew run after it still copies logically, and its cells are where they were.
        select(&mut s, (0, 3), (0, 4));
        assert_eq!(s.selection_text().as_deref(), Some("\u{5d0}\u{5d1}"));
        assert_eq!(s.visual_col(0, 3), 4);
    }

    /// The display order never reaches the copy: the selection copies what the program wrote.
    #[test]
    fn copy_stays_logical() {
        let mut s = Screen::new(12, 2);
        s.feed("ab \u{5d0}\u{5d1}".as_bytes());
        assert_eq!(s.visual_col(0, 3), 4, "the row is drawn reordered");
        select(&mut s, (0, 3), (0, 4));
        assert_eq!(s.term().selection_to_string().as_deref(), Some("\u{5d0}\u{5d1}"));
        assert_eq!(runs(&s)[0].runs[0].text, "\u{5d0}\u{5d1}");
    }

    /// A visual drag selection (the aleph at logical 3 left out): every reader of the selection
    /// agrees with the highlight and the plain copy.
    #[test]
    fn every_selection_reader_uses_the_visual_runs() {
        let mut s = Screen::new(10, 3);
        s.feed("ab \u{5d0}\u{5d1}\r\nplain".as_bytes());
        select(&mut s, (0, 0), (0, 0));
        assert!(s.drag_visual_selection((0, 0, false), (0, 3, true)));
        assert_eq!(s.term().selection_to_string().as_deref(), Some("ab \u{5d1}"));
        let text = |rows: &[Row]| rows.iter().map(|r| r.runs.iter().map(|x| x.text.as_str()).collect::<String>()).collect::<Vec<_>>().join("\n");
        assert_eq!(text(&runs(&s)), "ab \u{5d1}", "Copy with Styles (HTML and RTF)");
        assert_eq!(text(&crate::print::rows(&s, crate::print::What::Selection)), "ab \u{5d1}", "Print Selection");
        let plain = |t: String| {
            let mut out = String::new();
            let mut esc = false;
            for c in t.chars() {
                match (esc, c) {
                    (_, '\x1b') => esc = true,
                    (true, 'm') => esc = false,
                    (false, c) => out.push(c),
                    _ => {}
                }
            }
            out
        };
        assert_eq!(plain(sgr_text(&s)), "ab \u{5d1}", "Copy with Control Sequences");
        let (spans, _) = selection_spans(&s);
        let cols: Vec<usize> = spans.iter().flat_map(|&(_, a, b)| a..=b).collect();
        assert_eq!(cols, vec![0, 1, 2, 4], "the screenshot's selection spans");
        // Two rows: the rest of row 0 in reading order, then row 1's start.
        select(&mut s, (0, 0), (0, 0));
        assert!(s.drag_visual_selection((0, 3, false), (1, 1, true)));
        assert_eq!(text(&runs(&s)), "\u{5d0}\u{5d1}\npl");
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
        let row = Row { runs: vec![Run { text: "<b>&\"".into(), fg: rgb(255, 0, 0), bg: rgb(0, 0, 0), bold: true, italic: true, underline: true, link: None, default_bg: false }, Run { text: "go".into(), fg: rgb(0, 0, 255), bg: rgb(0, 0, 0), bold: false, italic: false, underline: true, link: Some("https://a.b/?q=1&r=2".into()), default_bg: false }], hard_end: true };
        let h = html(&[row.clone(), row], "Cascadia Mono", 12.0, rgb(0, 0, 0), DEFAULT_OPTS);
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

    fn bg_rows() -> Vec<Row> {
        let run = |t: &str, bg: Rgb, default_bg: bool| Run { text: t.into(), fg: rgb(255, 255, 255), bg, bold: false, italic: false, underline: false, link: None, default_bg };
        vec![Row { runs: vec![run("dflt", rgb(0, 0, 0), true), run("red", rgb(200, 0, 0), false)], hard_end: true }]
    }

    /// `CharacterAttributes.swift:114-117`, `PTYTextView+ARC.m:1060-1065`: the default
    /// background is left unset with CopyBackgroundColor off; every background is dropped with
    /// ExcludeBackgroundColorsFromCopiedStyle.
    #[test]
    fn styled_backgrounds_follow_iterm2() {
        let rows = bg_rows();
        let span_bgs = |h: &str| -> Vec<bool> { h.split("<span").skip(1).map(|s| s.split('>').next().unwrap().contains("background-color")).collect() };
        let h = html(&rows, "Cascadia Mono", 12.0, rgb(0, 0, 0), DEFAULT_OPTS);
        assert_eq!(span_bgs(&h), [true, true], "{h}");
        assert!(h.starts_with("<pre style=\"") && h.split('>').next().unwrap().contains("background-color:#000000"), "{h}");
        let off = StyleOptions { copy_background: false, exclude_backgrounds: false };
        let h = html(&rows, "Cascadia Mono", 12.0, rgb(0, 0, 0), off);
        assert_eq!(span_bgs(&h), [false, true], "{h}");
        assert!(!h.split('>').next().unwrap().contains("background-color"), "the pre has none: {h}");
        assert!(h.contains("background-color:#c80000"), "{h}");
        let none = StyleOptions { copy_background: true, exclude_backgrounds: true };
        let h = html(&rows, "Cascadia Mono", 12.0, rgb(0, 0, 0), none);
        assert!(!h.contains("background-color"), "{h}");
        // RTF writes \cb only where HTML writes a background.
        let r = rtf(&rows, "Cascadia Mono", 12.0, DEFAULT_OPTS);
        assert_eq!(r.matches("\\cb").count(), 2, "{r}");
        let r = rtf(&rows, "Cascadia Mono", 12.0, off);
        assert_eq!(r.matches("\\cb").count(), 1, "{r}");
        assert!(r.contains("\\red200\\green0\\blue0;") && !r.contains("\\red0\\green0\\blue0;"), "{r}");
        let r = rtf(&rows, "Cascadia Mono", 12.0, none);
        assert!(!r.contains("\\cb") && !r.contains("chcbpat"), "{r}");
    }

    #[test]
    fn default_bg_follows_the_cell() {
        let mut s = Screen::new(12, 2);
        s.feed(b"a\x1b[41mb\x1b[0m\x1b[7mc\x1b[0m\x1b[48;2;0;0;0md");
        select(&mut s, (0, 0), (0, 3));
        let r = &runs(&s)[0].runs;
        let got: Vec<(&str, bool)> = r.iter().map(|x| (x.text.as_str(), x.default_bg)).collect();
        assert_eq!(got, [("a", true), ("b", false), ("c", false), ("d", false)]);
    }

    #[test]
    fn style_options_read_the_keys() {
        let g = crate::globals::Globals::empty(std::path::Path::new("none.toml"));
        assert_eq!(StyleOptions::from_globals(&g), StyleOptions { copy_background: true, exclude_backgrounds: false });
    }

    /// B9: a program-authored link target never reaches the clipboard unless it is http, https
    /// or mailto; every attribute value is escaped, quotes both kinds.
    #[test]
    fn html_links_are_web_only() {
        let link = |u: &str| Row { runs: vec![Run { text: "t".into(), fg: rgb(1, 1, 1), bg: rgb(0, 0, 0), bold: false, italic: false, underline: false, link: Some(u.into()), default_bg: true }], hard_end: true };
        let h = |u: &str| html(&[link(u)], "Cascadia Mono", 12.0, rgb(0, 0, 0), DEFAULT_OPTS);
        for bad in ["javascript:alert(1)", "file:///c:/x", "vbscript:x", "ms-msdt:/id", "JaVaScRiPt:1", "data:text/html,x", "//host/share", "C:\\x.exe", "ftp://h/x"] {
            let out = h(bad);
            assert!(!out.contains("<a ") && !out.contains("href"), "{bad}: {out}");
            assert!(out.contains("<span"), "the span stays: {out}");
        }
        for good in ["https://a.b/x", "http://a.b", "mailto:a@b.c", "HTTPS://A.B"] {
            assert!(h(good).contains("<a href=\""), "{good}");
        }
        let out = h("https://a.b/\"'<>&x\u{7}y");
        assert!(out.contains("href=\"https://a.b/&quot;&#39;&lt;&gt;&amp;xy\""), "{out}");
        assert!(!out.contains('\u{7}'));
    }

    #[test]
    fn copied_text_is_escaped_and_bounded() {
        let run = |t: &str| Run { text: t.into(), fg: rgb(1, 1, 1), bg: rgb(0, 0, 0), bold: false, italic: false, underline: false, link: None, default_bg: true };
        let rows = vec![Row { runs: vec![run("a'b\u{1b}[31m\u{0}\t<")], hard_end: true }];
        let h = html(&rows, "Bad'Font\";x<y>\u{1}", f32::NAN, rgb(0, 0, 0), DEFAULT_OPTS);
        assert!(!h.chars().any(|c| c.is_control() && c != '\t' && c != '\n'), "{h:?}");
        assert!(h.contains("a&#39;b[31m\t&lt;"), "{h}");
        assert!(h.contains("font-family:'BadFontxy'") && h.contains("font-size:12pt"), "{h}");
        let r = rtf(&rows, "A{B}\\;", 12.0, DEFAULT_OPTS);
        assert!(!r.chars().any(|c| c.is_control() && c != '\n'), "{r:?}");
        assert!(r.contains("\\f0\\fmodern AB;}"), "{r}");
        // Bounded: a huge copy stops at the cap and is still well formed.
        let big = vec![Row { runs: vec![run(&"x".repeat(1 << 20))], hard_end: true }; 40];
        let h = html(&big, "F", 12.0, rgb(0, 0, 0), DEFAULT_OPTS);
        assert!(h.len() <= MAX_STYLED_BYTES + 4096 && h.ends_with("</pre>"), "{}", h.len());
        let r = rtf(&big, "F", 12.0, DEFAULT_OPTS);
        assert!(r.len() <= MAX_STYLED_BYTES + 4096 && r.ends_with('}'), "{}", r.len());
    }

    #[test]
    fn rtf_has_a_color_table() {
        let run = |t: &str, fg: Rgb, bold: bool| Run { text: t.into(), fg, bg: rgb(0, 0, 0), bold, italic: false, underline: false, link: None, default_bg: false };
        let rows = vec![Row { runs: vec![run("a{b}\\", rgb(255, 0, 0), true), run("\u{e9}\u{4e16}", rgb(255, 0, 0), false)], hard_end: true }];
        let r = rtf(&rows, "Cascadia Mono", 11.0, DEFAULT_OPTS);
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
    fn sgr_copy_carries_blink() {
        let mut s = Screen::new(12, 3);
        s.feed(b"\x1b[5mab\x1b[25mc");
        select(&mut s, (0, 0), (0, 11));
        assert_eq!(sgr_text(&s), "\x1b[0;5mab\x1b[0mc\x1b[0m");
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
