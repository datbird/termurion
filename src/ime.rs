//! The IME composition drawn at the cursor (`iTermTextDrawingHelper.m:3621-3770`). Pure: the
//! app stores a `Preedit`, lays it over the grid with `layout`, and the scene draws the result.

use crate::layout::Rect;
use crate::palette::Rgb;

/// What the IME is composing: its text and the caret as a byte offset (None: at the end).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Preedit {
    pub text: String,
    pub cursor: Option<usize>,
}

/// A run of composition text on one row: from `col`, `cells` columns wide.
#[derive(Clone, Debug, PartialEq)]
pub struct Piece {
    pub row: usize,
    pub col: usize,
    pub text: String,
    pub cells: usize,
}

/// A composition ready to draw: its pieces, the caret cell and the caret's color.
#[derive(Clone, Debug, PartialEq)]
pub struct Drawn {
    pub pieces: Vec<Piece>,
    pub caret: (usize, usize),
    pub caret_color: Rgb,
}

/// What the app does with one `WindowEvent::Ime`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    Ignore,
    /// The composition ended (cancelled, emptied, or the IME went away): nothing is sent.
    Clear,
    /// A composition is in progress: draw it, send nothing.
    Store(Preedit),
    /// The person accepted this text: it is typed input now.
    Commit(String),
}

/// winit delivers `Preedit("")` to end a composition: that clears, it is not an empty
/// composition to draw. The caret is the start of the cursor range (a byte offset).
pub fn step(ime: &winit::event::Ime) -> Step {
    use winit::event::Ime;
    match ime {
        Ime::Enabled => Step::Ignore,
        Ime::Disabled => Step::Clear,
        Ime::Preedit(text, _) if text.is_empty() => Step::Clear,
        Ime::Preedit(text, cursor) => Step::Store(Preedit { text: text.clone(), cursor: cursor.map(|(start, _)| start) }),
        Ime::Commit(text) if text.is_empty() => Step::Clear,
        Ime::Commit(text) => Step::Commit(text.clone()),
    }
}

/// Keys belong to the IME, not the session, while a composition is non-empty.
pub fn holds_keys(p: Option<&Preedit>) -> bool {
    p.is_some_and(|p| !p.text.is_empty())
}

/// Lays `p` over the grid from `start` (row, col) in `cols` columns, as iTerm2 does
/// (`iTermTextDrawingHelper.m:3621-3770`): a character that would be cut at the right edge
/// moves to the next row whole. Returns the pieces and the caret cell: the cell the character
/// at the caret offset takes (after its wrap), or just past the end of the text; a string that
/// ends exactly at the right edge leaves the caret at `(row, cols)`.
pub fn layout(p: &Preedit, start: (usize, usize), cols: usize) -> (Vec<Piece>, (usize, usize)) {
    use unicode_width::UnicodeWidthChar;
    let cols = cols.max(1);
    let (mut row, mut col) = start;
    let mut pieces: Vec<Piece> = Vec::new();
    let mut caret = None;
    for (at, c) in p.text.char_indices() {
        let w = c.width().unwrap_or(0);
        if w > 0 && col + w > cols && col > 0 {
            row += 1;
            col = 0;
        }
        if caret.is_none() && p.cursor.is_some_and(|k| k <= at) {
            caret = Some((row, col));
        }
        match pieces.last_mut() {
            Some(last) if last.row == row => {
                last.text.push(c);
                last.cells += w;
            }
            _ => pieces.push(Piece { row, col, text: c.to_string(), cells: w }),
        }
        col += w;
    }
    (pieces, caret.unwrap_or((row, col)))
}

/// The caret's rectangle (`iTermTextDrawingHelper.m:3738-3765`): 2 points wide and the font's
/// own cell high (`base_h`), centered in the line, and kept inside the right margin.
pub fn caret_rect(caret: (usize, usize), origin: (f32, f32), cell: (f32, f32), base_h: f32, cols: usize, scale: f32) -> Rect {
    let (cw, ch) = cell;
    let w = 2.0 * scale;
    let h = if base_h > 0.0 { base_h } else { ch };
    let x = (origin.0 + caret.1 as f32 * cw).min(origin.0 + cols as f32 * cw - w);
    let y = origin.1 + caret.0 as f32 * ch + ((ch - h) / 2.0).round().max(0.0);
    Rect { x, y, w, h }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pre(text: &str, cursor: Option<usize>) -> Preedit {
        Preedit { text: text.to_string(), cursor }
    }

    fn piece(row: usize, col: usize, text: &str, cells: usize) -> Piece {
        Piece { row, col, text: text.to_string(), cells }
    }

    #[test]
    fn ime_layout_follows_iterm2() {
        let (p, c) = layout(&pre("abc", Some(3)), (3, 2), 10);
        assert_eq!((p, c), (vec![piece(3, 2, "abc", 3)], (3, 5)));
        // Two wide characters from column 8: the second wraps; the caret at the end follows it.
        let (p, c) = layout(&pre("\u{65e5}\u{672c}", Some(6)), (3, 8), 10);
        assert_eq!(p, vec![piece(3, 8, "\u{65e5}", 2), piece(4, 0, "\u{672c}", 2)]);
        assert_eq!(c, (4, 2));
        // The caret before the wrapped character moves to the next row with it.
        let (_, c) = layout(&pre("\u{65e5}\u{672c}", Some(3)), (3, 8), 10);
        assert_eq!(c, (4, 0));
        // A wide character that does not fit is not split: row 3 gets nothing.
        let (p, c) = layout(&pre("\u{65e5}", None), (3, 9), 10);
        assert_eq!(p, vec![piece(4, 0, "\u{65e5}", 2)]);
        assert_eq!(c, (4, 2));
        // A string ending exactly at the right edge leaves the caret at (row, cols).
        let (p, c) = layout(&pre("abc", Some(3)), (3, 7), 10);
        assert_eq!(p, vec![piece(3, 7, "abc", 3)]);
        assert_eq!(c, (3, 10));
    }

    #[test]
    fn ime_caret_follows_iterm2() {
        let r = caret_rect((2, 3), (0.0, 0.0), (10.0, 24.0), 20.0, 10, 1.0);
        assert_eq!(r, Rect { x: 30.0, y: 50.0, w: 2.0, h: 20.0 });
        // At the right margin the caret is shoved left to stay inside the text area.
        let r = caret_rect((2, 10), (0.0, 0.0), (10.0, 24.0), 20.0, 10, 1.0);
        assert_eq!(r.x, 98.0);
        let r = caret_rect((2, 3), (0.0, 0.0), (10.0, 24.0), 20.0, 10, 1.5);
        assert_eq!(r.w, 3.0);
    }

    #[test]
    fn ime_events_become_steps() {
        use winit::event::Ime;
        assert_eq!(step(&Ime::Enabled), Step::Ignore);
        assert_eq!(step(&Ime::Disabled), Step::Clear);
        // An empty preedit ends a composition; it is not drawn as an empty one.
        assert_eq!(step(&Ime::Preedit(String::new(), None)), Step::Clear);
        assert_eq!(step(&Ime::Preedit("\u{306b}".to_string(), Some((3, 3)))), Step::Store(pre("\u{306b}", Some(3))));
        assert_eq!(step(&Ime::Preedit("ab".to_string(), None)), Step::Store(pre("ab", None)));
        assert_eq!(step(&Ime::Commit("\u{65e5}\u{672c}".to_string())), Step::Commit("\u{65e5}\u{672c}".to_string()));
        assert_eq!(step(&Ime::Commit(String::new())), Step::Clear);
    }

    #[test]
    fn keys_are_held_only_while_composing() {
        assert!(!holds_keys(None));
        assert!(!holds_keys(Some(&pre("", None))));
        assert!(holds_keys(Some(&pre("a", None))));
    }
}
