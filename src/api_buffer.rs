//! The Python API's GetBuffer (`PS:22381-22488`), GetPrompt (`PS:22500-22574`) and ListPrompts
//! (`PS:22490-22498`, `VS:1023-1051`): line ranges in absolute lines, each line's text with
//! its code points per cell and run-length styles (`PS:22176-22379`), and the prompt answers,
//! so the app only reads the screen.

use alacritty_terminal::grid::GridCell;
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::vte::ansi::{Color, NamedColor};

use crate::api_proto::pb;

/// The most lines `trailing_lines` returns (`PS:22405`).
pub const MAX_TRAILING: i64 = 1_000_000;

/// What absolute line numbers mean on a screen: the lines above the main screen (lost off the
/// top of its history, then the history kept), the history kept, and the screen's size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Facts {
    pub above: i64,
    pub history: i64,
    pub rows: i64,
    pub cols: i32,
}

/// A windowed range in absolute coordinates: start and end as (x, absolute y), and the
/// column window (location, length; length 0 is the full width).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Window {
    pub start: (i32, i64),
    pub end: (i32, i64),
    pub columns: (i64, i64),
}

/// The range a request's `line_range` names (`PS:22381-22414`): a windowed range as is;
/// otherwise exactly one of the screen (`screen_contents_only`, by presence) and the last
/// `trailing_lines` lines (at most `MAX_TRAILING`, at most every line kept). None for an
/// invalid range, which iTerm2 answers with no GetBuffer response at all (`PS:22419-22423`).
pub fn line_range(r: Option<&pb::LineRange>, f: Facts) -> Option<Window> {
    let r = r?;
    if let Some(w) = &r.windowed_coord_range {
        let cr = w.coord_range.unwrap_or_default();
        let (s, e) = (cr.start.unwrap_or_default(), cr.end.unwrap_or_default());
        let cols = w.columns.unwrap_or_default();
        let window = Window { start: (s.x(), s.y()), end: (e.x(), e.y()), columns: (cols.location(), cols.length()) };
        return (window.start.0 >= 0).then_some(window);
    }
    if r.screen_contents_only.is_some() == r.trailing_lines.is_some() {
        return None;
    }
    let (first, count) = match r.trailing_lines {
        Some(t) => {
            let n = MAX_TRAILING.min((t as i64).min(f.history + f.rows));
            (f.above + f.rows - n, n)
        }
        None => (f.above, f.rows),
    };
    Some(Window { start: (0, first), end: (0, first + count), columns: (0, 0) })
}

/// The grid line (negative is history) of absolute line `abs`; None once it was lost from
/// the history or when it is below the screen.
pub fn grid_line(abs: i64, f: Facts) -> Option<i32> {
    let line = abs - f.above;
    (line >= -f.history && line < f.rows).then_some(line as i32)
}

/// The absolute line of grid line `line`.
pub fn abs_line(line: i32, f: Facts) -> i64 {
    f.above + line as i64
}

/// The cells of one row that a windowed range reads (`enumerateCharsInRange:`,
/// iTermTextExtractor): from and to (exclusive) and whether the line is a soft end. The
/// uninitialized cells at the end are left out (default blanks; with a column window that is
/// not the full width, any blank); a line whose end is not reached (the last line, cut by
/// the range) is soft. None when the line gives no contents (the last line with end x 0).
pub fn span(row: &[Cell], first: bool, last: bool, wrapped: bool, w: Window) -> Option<(usize, usize, bool)> {
    let width = row.len() as i64;
    let (loc, len) = w.columns;
    let full = (loc == 0 && len == width) || len <= 0;
    let left = if len > 0 { loc } else { 0 };
    let startx = if first { (w.start.0 as i64).max(0) } else { left };
    let mut endx = if len > 0 { loc + len } else { width }.min(width);
    if last {
        endx = endx.min(w.end.0 as i64).max(0);
    }
    let null = |c: &Cell| c.is_empty() || (!full && c.c == ' ' && c.zerowidth().is_none_or(|z| z.is_empty()) && c.image().is_none());
    let mut nulls = 0;
    let mut x = endx - 1;
    while x >= loc.max(0) && null(&row[x as usize]) {
        nulls += 1;
        x -= 1;
    }
    let from = loc.max(startx).min(width - 1).max(0);
    let to = (endx - nulls).max(0);
    let reached = !last || nulls > 0;
    if reached {
        let soft = if full { wrapped } else { nulls == 0 };
        return Some((from as usize, (to.max(from)) as usize, soft));
    }
    // Enumeration stopped inside the line: soft, when any cell was read.
    (from < to).then_some((from as usize, to as usize, true))
}

/// An `AlternateColor` or standard index for an alacritty color, as iTerm2 keeps it: the
/// default colors are DEFAULT, the 16 named colors and their dim forms standard indices.
enum Kind {
    Default,
    Standard(u32),
    Rgb(pb::RgbColor),
}

fn kind(c: Color) -> Kind {
    match c {
        Color::Named(n) => match n {
            NamedColor::Foreground | NamedColor::Background | NamedColor::Cursor | NamedColor::BrightForeground | NamedColor::DimForeground => Kind::Default,
            n if (n as usize) < 16 => Kind::Standard(n as u32),
            n => Kind::Standard(n as u32 - NamedColor::DimBlack as u32),
        },
        Color::Indexed(i) => Kind::Standard(i as u32),
        Color::Spec(rgb) => Kind::Rgb(pb::RgbColor { red: Some(rgb.r as u32), green: Some(rgb.g as u32), blue: Some(rgb.b as u32) }),
    }
}

/// A link's identifier: None for one alacritty generated (`N_alacritty`) for a link sent
/// without an id, which iTerm2 sends with none.
fn link_id(id: &str) -> Option<String> {
    let generated = id.strip_suffix("_alacritty").is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
    (!generated).then(|| id.to_string())
}

/// A cell's style (`protoStyleForCharacter:externalAttributes:`, `PS:22176-22315`), `repeats`
/// 1. An image cell has no colors and is an iTerm2 image. Alacritty keeps no guarded
/// attribute: it is false.
pub fn style_of(cell: &Cell) -> pb::CellStyle {
    use pb::cell_style::{BgColor, FgColor};
    let alt = pb::AlternateColor::Default as i32;
    let image = cell.image().is_some();
    let (fg, bg) = if image {
        (None, None)
    } else {
        let fg = match kind(cell.fg) {
            Kind::Default => FgColor::FgAlternate(alt),
            Kind::Standard(i) => FgColor::FgStandard(i),
            Kind::Rgb(c) => FgColor::FgRgb(c),
        };
        let bg = match kind(cell.bg) {
            Kind::Default => BgColor::BgAlternate(alt),
            Kind::Standard(i) => BgColor::BgStandard(i),
            Kind::Rgb(c) => BgColor::BgRgb(c),
        };
        (Some(fg), Some(bg))
    };
    let f = cell.flags;
    // iTerm2 copies the color value's three fields; an indexed color keeps its index in red.
    let underline_color = cell.underline_color().map(|c| match c {
        Color::Spec(rgb) => pb::RgbColor { red: Some(rgb.r as u32), green: Some(rgb.g as u32), blue: Some(rgb.b as u32) },
        Color::Indexed(i) => pb::RgbColor { red: Some(i as u32), green: Some(0), blue: Some(0) },
        Color::Named(n) => pb::RgbColor { red: Some(n as u32), green: Some(0), blue: Some(0) },
    });
    let url = cell.hyperlink().map(|h| pb::Url { url: Some(h.uri().to_string()), identifier: link_id(h.id()) });
    pb::CellStyle {
        fg_color: fg,
        bg_color: bg,
        bold: Some(f.contains(Flags::BOLD)),
        faint: Some(f.contains(Flags::DIM)),
        italic: Some(f.contains(Flags::ITALIC)),
        blink: Some(f.contains(Flags::BLINK)),
        underline: Some(f.intersects(Flags::ALL_UNDERLINES)),
        strikethrough: Some(f.contains(Flags::STRIKEOUT)),
        invisible: Some(f.contains(Flags::HIDDEN)),
        inverse: Some(f.contains(Flags::INVERSE)),
        guarded: Some(false),
        image: Some(if image { pb::ImagePlaceholderType::Iterm2 } else { pb::ImagePlaceholderType::None } as i32),
        underline_color,
        block_id: None,
        url,
        repeats: Some(1),
    }
}

/// One line's contents (`stringForLine:`, `PS:22317-22379`) from cells `from..to` of `row`,
/// its default blank cells at the end left out: the cells' code points (a wide character's
/// spacer and an image cell have none), code points per cell and, when asked, styles, both
/// run-length encoded; soft or hard end. `block` is the line's block id.
pub fn line_contents(row: &[Cell], from: usize, to: usize, wrapped: bool, styles: bool, block: Option<&str>, form: crate::encoding::Form) -> pb::LineContents {
    let to = to.min(row.len());
    let mut end = to;
    while end > from && row[end - 1].is_empty() {
        end -= 1;
    }
    let mut text = String::new();
    let mut cpps: Vec<pb::CodePointsPerCell> = Vec::new();
    let mut runs: Vec<pb::CellStyle> = Vec::new();
    for cell in row.get(from..end).unwrap_or_default() {
        let n = if cell.image().is_some() || cell.flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
            0
        } else {
            let mut one = String::from(cell.c);
            one.extend(cell.zerowidth().unwrap_or_default());
            crate::encoding::normalize_in_place(&mut one, form);
            text.push_str(&one);
            one.chars().count() as i32
        };
        match cpps.last_mut() {
            Some(c) if c.num_code_points == Some(n) => c.repeats = Some(c.repeats.unwrap_or(0) + 1),
            _ => cpps.push(pb::CodePointsPerCell { num_code_points: Some(n), repeats: Some(1) }),
        }
        if styles {
            let mut s = style_of(cell);
            s.block_id = block.map(str::to_string);
            match runs.last_mut() {
                Some(r) if pb::CellStyle { repeats: s.repeats, ..r.clone() } == s => r.repeats = Some(r.repeats.unwrap_or(1) + 1),
                _ => runs.push(s),
            }
        }
    }
    let continuation = if wrapped { pb::line_contents::Continuation::SoftEol } else { pb::line_contents::Continuation::HardEol };
    pb::LineContents { text: Some(text), code_points_per_cell: cpps, continuation: Some(continuation as i32), style: runs }
}

impl Facts {
    /// The lines lost off the top of the history (iTerm2's scrollback overflow): the number
    /// of the oldest line kept.
    pub fn lost(&self) -> i64 {
        self.above - self.history
    }
}

/// The lines above a grid's screen as iTerm2 numbers them (`cumulativeScrollbackOverflow`
/// plus the history): lost lines plus the history kept, so a reflow that moves rows in or out
/// of the history never makes a number negative. The grid's running `scrolled_off`, which
/// triggers and marks use, is not it.
pub fn above(g: &alacritty_terminal::grid::Grid<Cell>) -> i64 {
    use alacritty_terminal::grid::Dimensions;
    g.dropped() as i64 + g.history_size() as i64
}

/// The facts of a screen: absolute lines count the main screen's lost and kept history, the
/// history is the main screen's, the size the screen's.
pub fn facts(screen: &crate::screen::Screen) -> Facts {
    use alacritty_terminal::grid::Dimensions;
    let (primary, shown) = (screen.term().primary_grid(), screen.term().grid());
    Facts { above: above(primary), history: primary.history_size() as i64, rows: shown.screen_lines() as i64, cols: shown.columns() as i32 }
}

/// GetBuffer's answer from `screen` (`PS:22416-22488`): each line of the range still kept,
/// history from the main screen and the rows of the screen shown below it (iTerm2 reads the
/// alternate screen under the main history), and the cursor of the screen shown. None for an
/// invalid range, which is answered with no GetBuffer response (`S:739-751`).
pub fn get_buffer(screen: &crate::screen::Screen, range: Option<&pb::LineRange>, styles: bool) -> Option<pb::GetBufferResponse> {
    use alacritty_terminal::index::{Column, Line};
    let f = facts(screen);
    let w = line_range(range, f)?;
    let (primary, shown) = (screen.term().primary_grid(), screen.term().grid());
    // Block ids by number (a number whose id was reused since has none).
    let blocks: std::collections::HashMap<u32, &str> = screen.blocks.by_id.iter().map(|(id, n)| (*n, id.as_str())).collect();
    let first = w.start.1.max(abs_line(-(f.history as i32), f));
    let last = w.end.1.min(abs_line(f.rows as i32 - 1, f));
    let mut lines = Vec::new();
    for y in first..=last {
        let Some(line) = grid_line(y, f) else { continue };
        let row = if line < 0 { &primary[Line(line)] } else { &shown[Line(line)] };
        let cells = &row[Column(0)..];
        let wrapped = cells.last().is_some_and(|c| c.flags.contains(Flags::WRAPLINE));
        let Some((from, to, soft)) = span(cells, y == first, y == w.end.1, wrapped, w) else { continue };
        lines.push(line_contents(cells, from, to, soft, styles, blocks.get(&row.block()).copied(), screen.normalization));
    }
    let cursor = shown.cursor.point;
    Some(buffer_response(lines, w, (cursor.column.0 as i32, abs_line(cursor.line.0, f))))
}

fn coord(p: (i32, i64)) -> pb::Coord {
    pb::Coord { x: Some(p.0), y: Some(p.1) }
}

fn coord_range(a: (i32, i64), b: (i32, i64)) -> pb::CoordRange {
    pb::CoordRange { start: Some(coord(a)), end: Some(coord(b)) }
}

/// GetBuffer's answer (`PS:22467-22487`): the lines, the cursor in absolute lines, OK, and
/// the range in absolute lines.
pub fn buffer_response(lines: Vec<pb::LineContents>, w: Window, cursor: (i32, i64)) -> pb::GetBufferResponse {
    pb::GetBufferResponse {
        status: Some(pb::get_buffer_response::Status::Ok as i32),
        contents: lines,
        cursor: Some(coord(cursor)),
        windowed_coord_range: Some(pb::WindowedCoordRange {
            coord_range: Some(coord_range(w.start, w.end)),
            columns: Some(pb::Range { location: Some(w.columns.0), length: Some(w.columns.1) }),
        }),
        ..Default::default()
    }
}

/// What a prompt mark says, in (x, absolute line) points: its id, the prompt (from the mark's
/// row), the command, where output began, the cursor, the command's text (None when there is
/// neither the mark's command nor one being typed), the directory and the exit code.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PromptFacts {
    pub id: u32,
    pub prompt: Option<((i32, i64), (i32, i64))>,
    pub command: Option<((i32, i64), (i32, i64))>,
    pub output_start: Option<(i32, i64)>,
    pub cursor: (i32, i64),
    pub text: Option<String>,
    pub directory: Option<String>,
    pub code: Option<i32>,
}

/// GetPrompt's answer (`PS:22532-22574`): PROMPT_UNAVAILABLE with no mark; else the ranges
/// that are set (output ending at the cursor), the command, the directory, FINISHED with the
/// exit status once there is a code, RUNNING once output began, else EDITING, and the id.
/// No excluded subranges: Termurion keeps none.
pub fn prompt_response(f: Option<&PromptFacts>) -> pb::GetPromptResponse {
    use pb::get_prompt_response::{State, Status};
    let Some(f) = f else { return pb::GetPromptResponse { status: Some(Status::PromptUnavailable as i32), ..Default::default() } };
    let state = match (f.code, f.output_start) {
        (Some(_), _) => State::Finished,
        (None, Some(_)) => State::Running,
        (None, None) => State::Editing,
    };
    pb::GetPromptResponse {
        status: Some(Status::Ok as i32),
        prompt_range: f.prompt.map(|(a, b)| coord_range(a, b)),
        command_range: f.command.map(|(a, b)| coord_range(a, b)),
        output_range: f.output_start.map(|a| coord_range(a, f.cursor)),
        working_directory: f.directory.clone(),
        command: f.text.clone(),
        prompt_state: Some(state as i32),
        exit_status: f.code.map(|c| c as u32),
        unique_prompt_id: Some(f.id.to_string()),
        excluded_subranges: Vec::new(),
    }
}

/// ListPrompts' ids (`VS:1023-1051`): the prompt marks oldest first, from `first` (nothing
/// when it never comes) through `last`.
pub fn prompt_ids(ids: &[u32], first: Option<&str>, last: Option<&str>) -> Vec<String> {
    let mut out = Vec::new();
    let mut found = first.is_none();
    for id in ids {
        let s = id.to_string();
        if !found {
            if Some(s.as_str()) != first {
                continue;
            }
            found = true;
        }
        let end = Some(s.as_str()) == last;
        out.push(s);
        if end {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use alacritty_terminal::term::cell::Hyperlink;
    use alacritty_terminal::vte::ansi::Rgb;

    const F: Facts = Facts { above: 120, history: 100, rows: 24, cols: 80 };

    fn range(r: pb::LineRange) -> Option<Window> {
        line_range(Some(&r), F)
    }

    fn windowed(s: (i32, i64), e: (i32, i64)) -> pb::LineRange {
        pb::LineRange {
            windowed_coord_range: Some(pb::WindowedCoordRange {
                coord_range: Some(coord_range(s, e)),
                columns: Some(pb::Range { location: Some(0), length: Some(0) }),
            }),
            ..Default::default()
        }
    }

    #[test]
    fn line_range_follows_iterm2() {
        let screen = pb::LineRange { screen_contents_only: Some(true), ..Default::default() };
        assert_eq!(range(screen), Some(Window { start: (0, 120), end: (0, 144), columns: (0, 0) }));
        let t = |n| range(pb::LineRange { trailing_lines: Some(n), ..Default::default() });
        assert_eq!(t(10), Some(Window { start: (0, 134), end: (0, 144), columns: (0, 0) }));
        let long = t(5000).unwrap();
        assert_eq!(long.end.1 - long.start.1, 124);
        assert_eq!((long.start, long.end), ((0, 20), (0, 144)));
        assert_eq!(range(pb::LineRange { screen_contents_only: Some(true), trailing_lines: Some(3), ..Default::default() }), None);
        assert_eq!(range(pb::LineRange::default()), None);
        assert_eq!(line_range(None, F), None);
        assert_eq!(range(windowed((2, 130), (5, 131))), Some(Window { start: (2, 130), end: (5, 131), columns: (0, 0) }));
        assert_eq!(range(windowed((-1, 130), (5, 131))), None);
        assert_eq!(grid_line(120, F), Some(0));
        assert_eq!(grid_line(20, F), Some(-100));
        assert_eq!(grid_line(19, F), None);
        assert_eq!(grid_line(144, F), None);
        assert_eq!(abs_line(-3, F), 117);
    }

    fn cell(c: char) -> Cell {
        Cell { c, ..Cell::default() }
    }

    /// "ab", a wide character and its spacer, "e" with a combining acute, then blanks.
    fn sample() -> Vec<Cell> {
        let mut row = vec![cell('a'), cell('b'), Cell { flags: Flags::WIDE_CHAR, ..cell('\u{4e2d}') }, Cell { flags: Flags::WIDE_CHAR_SPACER, ..cell(' ') }];
        let mut e = cell('e');
        e.push_zerowidth('\u{301}');
        row.push(e);
        row.extend((0..5).map(|_| Cell::default()));
        row
    }

    #[test]
    fn line_contents_follow_iterm2() {
        use pb::line_contents::Continuation;
        let row = sample();
        let l = line_contents(&row, 0, row.len(), false, true, None, crate::encoding::Form::None);
        assert_eq!(l.text.as_deref(), Some("ab\u{4e2d}e\u{301}"));
        let cpps: Vec<(i32, i32)> = l.code_points_per_cell.iter().map(|c| (c.num_code_points(), c.repeats())).collect();
        assert_eq!(cpps, vec![(1, 3), (0, 1), (2, 1)]);
        assert_eq!(l.style.iter().map(|s| s.repeats()).sum::<u32>(), 5);
        assert_eq!(l.continuation, Some(Continuation::HardEol as i32));
        let soft = line_contents(&row, 0, row.len(), true, true, None, crate::encoding::Form::None);
        assert_eq!(soft.continuation, Some(Continuation::SoftEol as i32));
        assert_eq!(line_contents(&row, 1, 2, false, true, None, crate::encoding::Form::None).text.as_deref(), Some("b"));
        assert!(line_contents(&row, 0, row.len(), false, false, None, crate::encoding::Form::None).style.is_empty());
    }

    /// Text leaves the grid normalized, and the code points per cell count what was sent.
    #[test]
    fn line_contents_follow_the_normalization_form() {
        use crate::encoding::Form;
        let row = sample();
        let nfc = line_contents(&row, 0, row.len(), false, false, None, Form::Nfc);
        assert_eq!(nfc.text.as_deref(), Some("ab\u{4e2d}\u{e9}"));
        let cpps: Vec<(i32, i32)> = nfc.code_points_per_cell.iter().map(|c| (c.num_code_points(), c.repeats())).collect();
        assert_eq!(cpps, vec![(1, 3), (0, 1), (1, 1)]);
        let precomposed = [cell('\u{e9}')];
        let nfd = line_contents(&precomposed, 0, 1, false, false, None, Form::Nfd);
        assert_eq!(nfd.text.as_deref(), Some("e\u{301}"));
        assert_eq!(nfd.code_points_per_cell.iter().map(|c| (c.num_code_points(), c.repeats())).collect::<Vec<_>>(), vec![(2, 1)]);
    }

    fn fg(c: Color) -> Option<pb::cell_style::FgColor> {
        style_of(&Cell { fg: c, ..cell('x') }).fg_color
    }

    #[test]
    fn styles_follow_iterm2() {
        use pb::cell_style::{BgColor, FgColor};
        let bold = Cell { fg: Color::Named(NamedColor::Red), flags: Flags::BOLD, ..cell('x') };
        let l = line_contents(&[bold.clone(), bold], 0, 2, false, true, None, crate::encoding::Form::None);
        assert_eq!(l.style.len(), 1);
        assert_eq!(l.style[0].fg_color, Some(FgColor::FgStandard(1)));
        assert_eq!((l.style[0].bold, l.style[0].repeats), (Some(true), Some(2)));
        let default = pb::AlternateColor::Default as i32;
        assert_eq!(fg(Color::Named(NamedColor::Foreground)), Some(FgColor::FgAlternate(default)));
        assert_eq!(fg(Color::Named(NamedColor::BrightBlue)), Some(FgColor::FgStandard(12)));
        assert_eq!(fg(Color::Named(NamedColor::DimGreen)), Some(FgColor::FgStandard(2)));
        assert_eq!(fg(Color::Indexed(200)), Some(FgColor::FgStandard(200)));
        assert_eq!(fg(Color::Spec(Rgb { r: 1, g: 2, b: 3 })), Some(FgColor::FgRgb(pb::RgbColor { red: Some(1), green: Some(2), blue: Some(3) })));
        assert_eq!(style_of(&cell('x')).bg_color, Some(BgColor::BgAlternate(default)));
        let flag = |f: Flags| style_of(&Cell { flags: f, ..cell('x') });
        assert_eq!(flag(Flags::DIM).faint, Some(true));
        assert_eq!(flag(Flags::HIDDEN).invisible, Some(true));
        assert_eq!(flag(Flags::STRIKEOUT).strikethrough, Some(true));
        for u in [Flags::UNDERLINE, Flags::DOUBLE_UNDERLINE, Flags::UNDERCURL, Flags::DOTTED_UNDERLINE, Flags::DASHED_UNDERLINE] {
            assert_eq!(flag(u).underline, Some(true));
        }
        assert_eq!(flag(Flags::INVERSE).inverse, Some(true));
        assert_eq!(flag(Flags::BLINK).blink, Some(true));
        let mut linked = cell('x');
        linked.set_hyperlink(Some(Hyperlink::new(Some("id1"), "https://example.com/".into())));
        assert_eq!(style_of(&linked).url, Some(pb::Url { url: Some("https://example.com/".into()), identifier: Some("id1".into()) }));
        let mut anonymous = cell('x');
        anonymous.set_hyperlink(Some(Hyperlink::new(None::<String>, "https://example.com/".into())));
        assert_eq!(style_of(&anonymous).url, Some(pb::Url { url: Some("https://example.com/".into()), identifier: None }));
        let mut under = cell('x');
        under.set_underline_color(Some(Color::Spec(Rgb { r: 4, g: 5, b: 6 })));
        assert_eq!(style_of(&under).underline_color, Some(pb::RgbColor { red: Some(4), green: Some(5), blue: Some(6) }));
        let plain = style_of(&cell('x'));
        assert_eq!((plain.blink, plain.guarded, plain.image), (Some(false), Some(false), Some(pb::ImagePlaceholderType::None as i32)));
        assert_eq!((plain.bold, plain.underline, plain.url.clone(), plain.underline_color), (Some(false), Some(false), None, None));
        let blocked = line_contents(&[cell('x')], 0, 1, false, true, Some("7"), crate::encoding::Form::None);
        assert_eq!(blocked.style[0].block_id.as_deref(), Some("7"));
        assert_eq!(line_contents(&[cell('x')], 0, 1, false, true, None, crate::encoding::Form::None).style[0].block_id, None);
    }

    #[test]
    fn spans_follow_the_text_extractor() {
        let w = Window { start: (0, 0), end: (0, 3), columns: (0, 0) };
        let row = sample();
        // A middle line: the blanks at the end are left out, the row's wrap decides.
        assert_eq!(span(&row, false, false, false, w), Some((0, 5, false)));
        assert_eq!(span(&row, false, false, true, w), Some((0, 5, true)));
        // The last line with end x 0 gives nothing; cut inside the text it is soft.
        assert_eq!(span(&row, false, true, false, w), None);
        let cut = Window { end: (2, 3), ..w };
        assert_eq!(span(&row, false, true, false, cut), Some((0, 2, true)));
        // Reaching the blanks, the line keeps its own end.
        let past = Window { end: (8, 3), ..w };
        assert_eq!(span(&row, false, true, false, past), Some((0, 5, false)));
        // The first line starts at start x.
        let from = Window { start: (1, 0), ..w };
        assert_eq!(span(&row, true, false, false, from), Some((1, 5, false)));
        // A column window narrower than the width: any blank at its end is left out, and the
        // end is hard only when blanks were left out.
        let mut spaced = vec![cell('a'), cell(' '), cell('b'), cell(' ')];
        spaced.extend((0..6).map(|_| Cell::default()));
        let narrow = Window { columns: (0, 4), ..w };
        assert_eq!(span(&spaced, false, false, true, narrow), Some((0, 3, false)));
        let tight = Window { columns: (1, 2), ..w };
        assert_eq!(span(&spaced, false, false, false, tight), Some((1, 3, true)));
        // An empty line still gives (empty) contents.
        let blank: Vec<Cell> = (0..10).map(|_| Cell::default()).collect();
        assert_eq!(span(&blank, false, false, false, w), Some((0, 0, false)));
    }

    fn facts() -> PromptFacts {
        PromptFacts { id: 7, cursor: (3, 12), text: Some("ls".into()), directory: Some("/tmp".into()), ..Default::default() }
    }

    #[test]
    fn prompt_follows_iterm2() {
        use pb::get_prompt_response::{State, Status};
        assert_eq!(prompt_response(None), pb::GetPromptResponse { status: Some(Status::PromptUnavailable as i32), ..Default::default() });
        let done = prompt_response(Some(&PromptFacts { code: Some(1), ..facts() }));
        assert_eq!((done.prompt_state, done.exit_status), (Some(State::Finished as i32), Some(1)));
        assert_eq!(prompt_response(Some(&PromptFacts { code: Some(-1), ..facts() })).exit_status, Some(4294967295));
        let running = prompt_response(Some(&PromptFacts { output_start: Some((0, 11)), ..facts() }));
        assert_eq!(running.prompt_state, Some(State::Running as i32));
        assert_eq!(running.output_range, Some(coord_range((0, 11), (3, 12))));
        assert_eq!(running.exit_status, None);
        let editing = prompt_response(Some(&facts()));
        assert_eq!(editing.prompt_state, Some(State::Editing as i32));
        assert_eq!((editing.output_range, editing.prompt_range, editing.command_range), (None, None, None));
        let ranges = prompt_response(Some(&PromptFacts { prompt: Some(((0, 10), (2, 10))), command: Some(((2, 10), (0, 11))), ..facts() }));
        assert_eq!(ranges.prompt_range, Some(coord_range((0, 10), (2, 10))));
        assert_eq!(ranges.command_range, Some(coord_range((2, 10), (0, 11))));
        assert_eq!(editing.command.as_deref(), Some("ls"));
        assert_eq!(editing.working_directory.as_deref(), Some("/tmp"));
        assert_eq!(editing.unique_prompt_id.as_deref(), Some("7"));
        assert!(editing.excluded_subranges.is_empty());
        // No command and none being typed: the field is unset.
        assert_eq!(prompt_response(Some(&PromptFacts { text: None, ..facts() })).command, None);
        assert_eq!(editing.status, Some(Status::Ok as i32));
    }

    #[test]
    fn prompt_ids_follow_iterm2() {
        let ids = [3, 5, 9];
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(prompt_ids(&ids, None, None), s(&["3", "5", "9"]));
        assert_eq!(prompt_ids(&ids, Some("5"), None), s(&["5", "9"]));
        assert_eq!(prompt_ids(&ids, None, Some("5")), s(&["3", "5"]));
        assert_eq!(prompt_ids(&ids, Some("4"), None), Vec::<String>::new());
        assert_eq!(prompt_ids(&ids, Some("9"), Some("3")), s(&["9"]));
    }

    fn texts(r: &pb::GetBufferResponse) -> Vec<String> {
        r.contents.iter().map(|l| l.text().to_string()).collect()
    }

    fn cursor_of(r: &pb::GetBufferResponse) -> (i32, i64) {
        r.cursor.map(|c| (c.x(), c.y())).unwrap()
    }

    #[test]
    fn get_buffer_reads_history_and_the_screen_shown() {
        let screen_only = pb::LineRange { screen_contents_only: Some(true), ..Default::default() };
        let trailing = pb::LineRange { trailing_lines: Some(5), ..Default::default() };
        let mut s = crate::screen::Screen::new(10, 3);
        s.feed(b"1\r\n2\r\n3\r\n4\r\n5");
        let r = get_buffer(&s, Some(&screen_only), false).unwrap();
        assert_eq!(texts(&r), vec!["3", "4", "5"]);
        assert_eq!(cursor_of(&r), (1, 4));
        assert_eq!(texts(&get_buffer(&s, Some(&trailing), false).unwrap()), vec!["1", "2", "3", "4", "5"]);
        // On the alternate screen: the main history, then the alternate rows.
        s.feed(b"\x1b[?1049h\x1b[Halt");
        let r = get_buffer(&s, Some(&screen_only), false).unwrap();
        assert_eq!(texts(&r), vec!["alt", "", ""]);
        assert_eq!(cursor_of(&r), (3, 2));
        assert_eq!(texts(&get_buffer(&s, Some(&trailing), false).unwrap()), vec!["1", "2", "alt", "", ""]);
        assert_eq!(get_buffer(&s, None, false), None);
    }

    #[test]
    fn a_reflow_never_numbers_a_line_below_zero() {
        use alacritty_terminal::grid::Dimensions;
        use alacritty_terminal::selection::SelectionType;
        let all = pb::LineRange { trailing_lines: Some(100), ..Default::default() };
        let mut s = crate::screen::Screen::new(10, 3);
        s.feed(b"aaaaaaaa\r\nbbbbbbbb\r\ncccccccc\r\ndddddddd\r\neeee");
        assert_eq!((super::facts(&s).lost(), super::facts(&s).history), (0, 2));
        let before = s.term().primary_grid().scrolled_off();
        // A split narrows the pane: each long line wraps onto two rows, which go into the
        // history without scrolling.
        s.resize(4, 3);
        let g = s.term().primary_grid();
        assert_eq!(g.scrolled_off(), before, "the running count triggers and marks use stays");
        assert!((g.scrolled_off() as i64) < g.history_size() as i64, "the old base would be negative");
        let f = super::facts(&s);
        assert_eq!((f.lost(), f.history), (0, 6), "overflow is never negative");
        let r = get_buffer(&s, Some(&all), false).unwrap();
        assert_eq!(r.windowed_coord_range.and_then(|w| w.coord_range).and_then(|c| c.start).map(|c| c.y()), Some(0));
        assert_eq!(texts(&r)[0], "aaaa");
        // A selection of the oldest line, read back as GetSelection gives it, is one
        // SetSelection accepts, on the same line.
        let sub = crate::api_misc::from_sel(SelectionType::Simple, (0, 0), (0, 3), f.lost(), f.cols);
        let sel = crate::api_misc::to_sel(&sub, f.cols, f.lost()).expect("SetSelection accepts it");
        assert_eq!((sel.start, sel.end), ((0, 0), (0, 4)));
        // Widening again merges the rows back; the numbers stay at or above zero.
        s.resize(10, 3);
        let f = super::facts(&s);
        assert_eq!((f.lost(), f.history), (0, 2));
        assert_eq!(s.term().primary_grid().scrolled_off(), before);
    }

    #[test]
    fn lines_lost_off_the_history_count_as_overflow() {
        // A full history drops its oldest line on each scroll.
        let mut s = crate::screen::Screen::with_scrollback(10, 3, 2);
        s.feed(b"1\r\n2\r\n3\r\n4\r\n5\r\n6");
        assert_eq!((super::facts(&s).lost(), super::facts(&s).history), (1, 2));
        // A narrowing reflow past the history limit loses the oldest rows.
        let mut s = crate::screen::Screen::with_scrollback(8, 2, 3);
        s.feed(b"aaaaaa\r\nbbbbbb\r\ncccccc\r\ndddddd\r\ne");
        assert_eq!((super::facts(&s).lost(), super::facts(&s).history), (0, 3));
        s.resize(3, 2);
        let f = super::facts(&s);
        assert_eq!(f.history, 3);
        assert_eq!(f.lost() + f.history + f.rows, 9, "every row ever made is numbered once");
        // Clearing the history loses nothing (iTerm2 keeps its overflow and drops the lines).
        s.set_rules(crate::emu::Rules { clear_history: Some(false), ..Default::default() });
        s.feed(b"\x1b[3J");
        assert_eq!((super::facts(&s).lost(), super::facts(&s).history), (f.lost(), 0));
        assert_eq!(s.term().primary_grid().dropped(), f.lost() as u64);
    }

    #[test]
    fn buffer_response_is_absolute() {
        let w = Window { start: (0, 120), end: (0, 144), columns: (0, 0) };
        let r = buffer_response(vec![pb::LineContents::default()], w, (4, 130));
        assert_eq!(r.status, Some(pb::get_buffer_response::Status::Ok as i32));
        assert_eq!(r.contents.len(), 1);
        assert_eq!(r.cursor, Some(coord((4, 130))));
        assert_eq!(r.windowed_coord_range.and_then(|w| w.coord_range), Some(coord_range((0, 120), (0, 144))));
    }
}
