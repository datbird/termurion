//! Right-to-left display order of one row of cells (iTerm2's `BidiDisplayInfo`, which is built
//! per screen line, not per wrapped paragraph).
//!
//! Pure: a row's cells in, a permutation of exactly those cells out. Nothing here knows about
//! panes, Tandem's own text or any other row, so a program's text can only ever reorder within
//! its own line. Safety rules (preflight P9, P10):
//! - The explicit embedding, override and isolate controls (U+202A to U+202E, U+2066 to U+2069)
//!   are left out of the text the algorithm sees, so a program cannot reverse a line with RLO.
//!   They stay on their cell for copying. Only implicit reordering remains: letters, and the
//!   invisible marks RLM, ALM and LRM (ruled in m13m-rulings.txt).
//! - Anything unexpected (several paragraphs, a length mismatch, a table that is not a
//!   permutation) gives None: the row draws in logical order.
//! - Only rows holding a right-to-left letter are analysed, and a row is capped at `MAX_CELLS`.

use unicode_bidi::{BidiClass, BidiInfo, Level, bidi_class};

/// The most cells one row may hold for analysis; a wider row draws in logical order.
const MAX_CELLS: usize = 4096;

/// One cell of a row as text: the base character and its zero-width characters, whether it is
/// the first half of a wide character, and whether it is that character's second half (a
/// spacer, which has no text).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CellText {
    pub text: String,
    pub wide: bool,
    pub spacer: bool,
}

/// A row's display order. `lut[logical] = visual` and `inverse[visual] = logical`, per cell.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Order {
    pub lut: Vec<u32>,
    pub inverse: Vec<u32>,
    /// By logical cell: the cell lies in a right-to-left run (odd level).
    pub rtl: Vec<bool>,
    /// By logical cell: draw the mirrored counterpart of its character (rule L4: a mirrorable
    /// character at an odd level).
    pub mirrored: Vec<bool>,
    pub paragraph_rtl: bool,
}

/// The settings that decide whether and how a row is reordered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rules {
    /// `Bidi` (General > Experimental): right-to-left text support.
    pub enabled: bool,
    /// `DetectParagraphDirection`: a row whose first strong character is right to left reads
    /// right to left.
    pub detect_direction: bool,
    /// `RightJustifyRTLLines`: pad a row so a right-to-left paragraph sits at the right edge.
    pub right_justify: bool,
    /// `DisableBidiInAlternateScreen`.
    pub off_in_alternate: bool,
}

impl Default for Rules {
    /// iTerm2's defaults.
    fn default() -> Self {
        Rules { enabled: true, detect_direction: false, right_justify: true, off_in_alternate: false }
    }
}

impl Rules {
    pub fn from_globals(g: &crate::globals::Globals) -> Rules {
        Rules {
            enabled: g.bool("Bidi"),
            detect_direction: g.bool("DetectParagraphDirection"),
            right_justify: g.bool("RightJustifyRTLLines"),
            off_in_alternate: g.bool("DisableBidiInAlternateScreen"),
        }
    }
}

/// A strong right-to-left letter (class R or AL).
pub fn is_rtl_letter(c: char) -> bool {
    !c.is_ascii() && matches!(bidi_class(c), BidiClass::R | BidiClass::AL)
}

/// The explicit embedding, override and isolate controls.
pub fn is_explicit_control(c: char) -> bool {
    matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
}

/// Whether `text` holds a right-to-left letter (a strong R or AL character).
pub fn has_rtl(text: &str) -> bool {
    text.chars().any(is_rtl_letter)
}

/// Whether a row's first strong directional character is right to left (rules P2 and P3),
/// walking scalars.
pub fn paragraph_is_rtl(text: &str) -> bool {
    for c in text.chars() {
        if c.is_ascii() && !c.is_ascii_alphabetic() {
            continue;
        }
        match bidi_class(c) {
            BidiClass::L => return false,
            BidiClass::R | BidiClass::AL => return true,
            _ => {}
        }
    }
    false
}

/// Whether the character joins its neighbors when drawn (Arabic and the other Arabic-letter
/// scripts): its run must be shaped as one piece.
pub fn joins(c: char) -> bool {
    !c.is_ascii() && bidi_class(c) == BidiClass::AL
}

/// The mirrored counterpart of `c` (`bidi_mirror::PAIRS`), else `c`. Characters outside the BMP
/// have none in the table. `unicode-bidi` 0.3.18 exposes no mirroring data.
pub fn mirror(c: char) -> char {
    match u16::try_from(c as u32) {
        Ok(u) => char::from_u32(u32::from(crate::bidi_mirror::counterpart(u))).unwrap_or(c),
        Err(_) => c,
    }
}

/// The display order of a row, or None when it needs none: disabled, the alternate screen with
/// `off_in_alternate`, no right-to-left letter, or anything the algorithm cannot place.
pub fn order(cells: &[CellText], rules: Rules, alternate: bool) -> Option<Order> {
    if !rules.enabled || (alternate && rules.off_in_alternate) || cells.is_empty() || cells.len() > MAX_CELLS {
        return None;
    }
    // One unit per character: a wide cell and its spacer move together.
    let mut text = String::new();
    let mut unit_cell: Vec<usize> = Vec::new();
    let mut unit_width: Vec<usize> = Vec::new();
    let mut unit_offset: Vec<usize> = Vec::new();
    let mut i = 0;
    while i < cells.len() {
        let width = if cells[i].wide && cells.get(i + 1).is_some_and(|c| c.spacer) { 2 } else { 1 };
        let at = text.len();
        for c in cells[i].text.chars().filter(|&c| !is_explicit_control(c)) {
            // An empty cell holds NUL: a real space, as it draws (`replacingNulWithSpace`).
            text.push(if c == '\0' { ' ' } else { c });
        }
        if text.len() == at {
            // A lone spacer, or a cell holding only controls, is a neutral space.
            text.push(' ');
        }
        unit_cell.push(i);
        unit_width.push(width);
        unit_offset.push(at);
        i += width;
    }
    if !has_rtl(&text) {
        return None;
    }
    let paragraph_rtl = rules.detect_direction && paragraph_is_rtl(&text);
    let info = BidiInfo::new(&text, Some(if paragraph_rtl { Level::rtl() } else { Level::ltr() }));
    if info.paragraphs.len() != 1 {
        return None;
    }
    let levels = info.reordered_levels(&info.paragraphs[0], 0..text.len());
    if levels.len() != text.len() {
        return None;
    }
    let unit_levels: Vec<Level> = unit_offset.iter().map(|&o| levels[o]).collect();
    if !unit_levels.iter().any(|l| l.is_rtl()) {
        return None;
    }
    let visual = BidiInfo::reorder_visual(&unit_levels);
    let n = cells.len();
    let mut lut = vec![u32::MAX; n];
    let mut rtl = vec![false; n];
    let mut mirrored = vec![false; n];
    let mut pos = 0usize;
    let mut placed = vec![false; unit_cell.len()];
    if visual.len() != unit_cell.len() {
        return None;
    }
    for &u in &visual {
        if u >= unit_cell.len() || std::mem::replace(&mut placed[u], true) {
            return None;
        }
        let (cell, width) = (unit_cell[u], unit_width[u]);
        let odd = unit_levels[u].is_rtl();
        let first = cells[cell].text.chars().find(|&c| !is_explicit_control(c));
        for k in 0..width {
            lut[cell + k] = (pos + k) as u32;
            rtl[cell + k] = odd;
        }
        mirrored[cell] = odd && first.is_some_and(|c| mirror(c) != c);
        pos += width;
    }
    if pos != n || lut.contains(&u32::MAX) {
        return None;
    }
    let inverse = invert(&lut);
    Some(Order { lut, inverse, rtl, mirrored, paragraph_rtl })
}

fn invert(lut: &[u32]) -> Vec<u32> {
    let mut inverse = vec![0; lut.len()];
    for (logical, &visual) in lut.iter().enumerate() {
        inverse[visual as usize] = logical as u32;
    }
    inverse
}

/// The order padded to `width` cells (iTerm2's `pad`): a right-to-left paragraph moves its
/// cells right by the growth and the new cells fill the left in reverse; otherwise the new
/// cells follow in order.
pub fn pad(o: &Order, width: usize) -> Order {
    let base = o.lut.len();
    if width <= base {
        return o.clone();
    }
    let growth = (width - base) as u32;
    let lut: Vec<u32> = if o.paragraph_rtl {
        o.lut.iter().map(|v| v + growth).chain((0..growth).rev()).collect()
    } else {
        o.lut.iter().copied().chain(base as u32..width as u32).collect()
    };
    let mut rtl = o.rtl.clone();
    rtl.resize(width, false);
    let mut mirrored = o.mirrored.clone();
    mirrored.resize(width, false);
    let inverse = invert(&lut);
    Order { lut, inverse, rtl, mirrored, paragraph_rtl: o.paragraph_rtl }
}

/// The visual column of logical column `logical`; a row with no order (or a column past it)
/// keeps its column.
pub fn visual_for_logical(o: Option<&Order>, logical: usize) -> usize {
    o.and_then(|o| o.lut.get(logical)).map_or(logical, |&v| v as usize)
}

/// The logical column drawn at visual column `visual`.
pub fn logical_for_visual(o: Option<&Order>, visual: usize) -> usize {
    o.and_then(|o| o.inverse.get(visual)).map_or(visual, |&l| l as usize)
}

/// The visual pieces a logical range covers, sorted, adjacent cells merged (iTerm2's
/// `visualRange` and `enumerateLogicalRanges`): a selection or highlight over logical cells
/// that are not contiguous on screen is several rectangles. Columns past the order keep
/// their place.
pub fn visual_spans(o: &Order, logical: std::ops::Range<usize>) -> Vec<std::ops::Range<usize>> {
    let mut visual: Vec<usize> = logical.map(|l| visual_for_logical(Some(o), l)).collect();
    visual.sort_unstable();
    let mut spans: Vec<std::ops::Range<usize>> = Vec::new();
    for v in visual {
        match spans.last_mut() {
            Some(last) if last.end == v => last.end = v + 1,
            _ => spans.push(v..v + 1),
        }
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cells(s: &str) -> Vec<CellText> {
        s.chars().map(|c| CellText { text: c.to_string(), wide: false, spacer: false }).collect()
    }

    fn on() -> Rules {
        Rules { enabled: true, detect_direction: false, right_justify: true, off_in_alternate: false }
    }

    fn detect() -> Rules {
        Rules { detect_direction: true, ..on() }
    }

    fn inverse_of(lut: &[u32]) -> Vec<u32> {
        let mut inv = vec![0; lut.len()];
        for (logical, &visual) in lut.iter().enumerate() {
            inv[visual as usize] = logical as u32;
        }
        inv
    }

    #[test]
    fn order_follows_iterm2() {
        let o = order(&cells("ab \u{5d0}\u{5d1}"), on(), false).unwrap();
        assert_eq!(o.lut, vec![0, 1, 2, 4, 3]);
        assert_eq!(o.inverse, inverse_of(&o.lut));
        assert_eq!(o.rtl, vec![false, false, false, true, true]);
        assert!(!o.paragraph_rtl);
        // Detection with an English first strong character gives the same.
        let o = order(&cells("ab \u{5d0}\u{5d1}"), detect(), false).unwrap();
        assert_eq!(o.lut, vec![0, 1, 2, 4, 3]);
        // A Hebrew first strong character: the paragraph is right to left and the English run
        // keeps its own order at the left.
        let o = order(&cells("\u{5d0}\u{5d1} ab"), detect(), false).unwrap();
        assert!(o.paragraph_rtl);
        assert_eq!(o.lut, vec![4, 3, 2, 0, 1]);
        assert_eq!(o.inverse, inverse_of(&o.lut));
        // Without detection the base direction is left to right.
        let o = order(&cells("\u{5d0}\u{5d1} ab"), on(), false).unwrap();
        assert!(!o.paragraph_rtl);
        assert_eq!(o.lut, vec![1, 0, 2, 3, 4]);
        // No right-to-left character, disabled, and the alternate screen with the setting.
        assert_eq!(order(&cells("abc"), on(), false), None);
        assert_eq!(order(&cells("ab \u{5d0}"), Rules { enabled: false, ..on() }, false), None);
        assert_eq!(order(&cells("ab \u{5d0}"), Rules { off_in_alternate: true, ..on() }, true), None);
        assert!(order(&cells("ab \u{5d0}"), on(), true).is_some(), "the alternate screen reorders unless told not to");
        assert!(order(&cells("ab \u{5d0}"), Rules { off_in_alternate: true, ..on() }, false).is_some());
    }

    #[test]
    fn wide_cells_stay_together() {
        let mut row = cells("\u{5d0}");
        row.push(CellText { text: "\u{4e2d}".into(), wide: true, spacer: false });
        row.push(CellText { text: String::new(), wide: false, spacer: true });
        row.extend(cells("b"));
        let o = order(&row, detect(), false).unwrap();
        // The Hebrew paragraph reads right to left: b, then the wide character, then the aleph.
        assert!(o.paragraph_rtl);
        let mut seen = o.lut.clone();
        seen.sort_unstable();
        assert_eq!(seen, vec![0, 1, 2, 3], "every logical cell appears once");
        assert_eq!(o.lut[2], o.lut[1] + 1, "the spacer sits right after its base: {:?}", o.lut);
        assert_eq!(o.inverse, inverse_of(&o.lut));
        // Also in a left to right paragraph with the wide character inside the Hebrew run.
        let mut row = cells("a\u{5d0}");
        row.push(CellText { text: "\u{4e2d}".into(), wide: true, spacer: false });
        row.push(CellText { text: String::new(), wide: false, spacer: true });
        row.extend(cells("\u{5d1}"));
        let o = order(&row, on(), false).unwrap();
        assert_eq!(o.lut[3], o.lut[2] + 1, "{:?}", o.lut);
        let mut seen = o.lut.clone();
        seen.sort_unstable();
        assert_eq!(seen, vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn mirrors_follow_the_standard() {
        let o = order(&cells("\u{5d0}(\u{5d1})"), on(), false).unwrap();
        assert_eq!(o.mirrored, vec![false, true, false, true]);
        assert_eq!(mirror('('), ')');
        assert_eq!(mirror(')'), '(');
        assert_eq!(mirror('a'), 'a');
        assert_eq!(mirror('\u{4e2d}'), '\u{4e2d}');
        assert_eq!(mirror('\u{1d7ce}'), '\u{1d7ce}', "outside the BMP maps to itself");
        // Parentheses around English text after English text resolve to an even level (rule
        // N0) in a right-to-left paragraph: not mirrored (rule L4, odd levels only).
        let o = order(&cells("\u{5d0} ab (cd)"), detect(), false).unwrap();
        assert!(o.paragraph_rtl);
        assert_eq!(o.mirrored, vec![false, false, false, false, false, false, false, false, false]);
        assert_eq!(o.rtl[5..], [false, false, false, false]);
    }

    #[test]
    fn padding_follows_iterm2() {
        let o = Order { lut: vec![1, 0], inverse: vec![1, 0], rtl: vec![true, true], mirrored: vec![false, true], paragraph_rtl: true };
        let p = pad(&o, 4);
        assert_eq!(p.lut, vec![3, 2, 1, 0]);
        assert_eq!(p.inverse, vec![3, 2, 1, 0]);
        assert_eq!((p.rtl.len(), p.mirrored.len()), (4, 4));
        assert_eq!(p.mirrored, vec![false, true, false, false]);
        assert!(p.paragraph_rtl);
        let q = pad(&Order { paragraph_rtl: false, ..o.clone() }, 4);
        assert_eq!(q.lut, vec![1, 0, 2, 3]);
        assert_eq!(q.inverse, vec![1, 0, 2, 3]);
        assert_eq!(pad(&o, 2), o, "no growth, no change");
    }

    #[test]
    fn first_strong_and_has_rtl() {
        assert!(has_rtl("ab \u{5d0}"));
        assert!(has_rtl("\u{627}"));
        assert!(!has_rtl("abc 123 \u{4e2d}"));
        assert!(paragraph_is_rtl("  1 \u{5d0}a"));
        assert!(!paragraph_is_rtl("a\u{5d0}"));
        assert!(!paragraph_is_rtl("123"));
    }

    /// Security (P9): a program cannot reverse a line with an override or an embedding.
    #[test]
    fn explicit_overrides_do_not_reorder() {
        // RLO rides on a cell as a zero-width extra and would reverse "abc" in a real UBA.
        let row = vec![
            CellText { text: "x\u{202e}".into(), wide: false, spacer: false },
            CellText { text: "a".into(), wide: false, spacer: false },
            CellText { text: "b".into(), wide: false, spacer: false },
            CellText { text: "c".into(), wide: false, spacer: false },
        ];
        assert_eq!(order(&row, on(), false), None);
        // An RLO in a cell of its own, and every other explicit control, with Hebrew present.
        for ctl in ['\u{202a}', '\u{202b}', '\u{202c}', '\u{202d}', '\u{202e}', '\u{2066}', '\u{2067}', '\u{2068}', '\u{2069}'] {
            let mut row = cells("ab");
            row.push(CellText { text: ctl.to_string(), wide: false, spacer: false });
            row.extend(cells("cd\u{5d0}"));
            let o = order(&row, on(), false).unwrap();
            assert_eq!(&o.lut[..5], &[0, 1, 2, 3, 4], "{ctl:?} must not reverse the English text");
            assert_eq!(o.lut.len(), 6);
        }
    }

    /// Fail closed and bounded: whatever the row holds, the table is a permutation of its cells.
    #[test]
    fn lut_is_always_a_permutation() {
        let pool: Vec<&str> = vec![
            "a", "b", " ", "\u{0}", "\u{5d0}", "\u{5d1}", "\u{627}", "\u{628}", "1", "(", ")", "\u{202e}", "\u{2067}", "\u{2069}", "\u{200f}", "\u{300}", "\u{4e2d}", "\u{2029}", "\u{a}",
        ];
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for _ in 0..500 {
            let n = 1 + (next() % 40) as usize;
            let mut row: Vec<CellText> = Vec::new();
            while row.len() < n {
                let s = pool[(next() % pool.len() as u64) as usize];
                if s == "\u{4e2d}" {
                    row.push(CellText { text: s.into(), wide: true, spacer: false });
                    row.push(CellText { text: String::new(), wide: false, spacer: true });
                } else {
                    row.push(CellText { text: s.into(), wide: false, spacer: false });
                }
            }
            for rules in [on(), detect()] {
                if let Some(o) = order(&row, rules, false) {
                    let mut seen = o.lut.clone();
                    seen.sort_unstable();
                    assert_eq!(seen, (0..row.len() as u32).collect::<Vec<_>>(), "{row:?}");
                    assert_eq!(o.inverse, inverse_of(&o.lut));
                    assert_eq!((o.rtl.len(), o.mirrored.len()), (row.len(), row.len()));
                }
            }
        }
    }

    /// Ruling: the implicit marks RLM, ALM and LRM keep driving the order, as in UAX 9 and
    /// iTerm2 (real right-to-left text needs them). Only the explicit controls are neutralized.
    #[test]
    fn implicit_marks_still_reorder() {
        let o = order(&cells("a\u{200f}b"), on(), false).expect("an RLM makes the row right to left");
        assert!(o.rtl[1], "the RLM sits at an odd level");
        let o = order(&cells("1\u{61c}2 \u{5d0}"), on(), false).expect("an ALM is a strong Arabic letter");
        assert!(o.rtl[1]);
    }

    /// A pathological row of controls is quick: they are dropped before the algorithm runs.
    #[test]
    fn a_row_of_controls_is_bounded() {
        let row: Vec<CellText> = (0..2000).map(|i| CellText { text: if i % 2 == 0 { "\u{202e}".into() } else { "\u{2067}\u{5d0}".into() }, wide: false, spacer: false }).collect();
        let start = std::time::Instant::now();
        let o = order(&row, on(), false);
        assert!(start.elapsed() < std::time::Duration::from_millis(500), "{:?}", start.elapsed());
        if let Some(o) = o {
            assert_eq!(o.lut.len(), 2000);
        }
    }

    #[test]
    fn mapping_round_trips() {
        let rows = ["ab \u{5d0}\u{5d1}", "\u{5d0}\u{5d1} ab", "a\u{5d0}\u{5d1}b\u{5d2}c"];
        for text in rows {
            for rules in [on(), detect()] {
                let o = order(&cells(text), rules, false).unwrap();
                for x in 0..o.lut.len() {
                    assert_eq!(logical_for_visual(Some(&o), visual_for_logical(Some(&o), x)), x, "{text:?} {x}");
                    assert_eq!(visual_for_logical(Some(&o), logical_for_visual(Some(&o), x)), x);
                }
            }
        }
        // Without an order, and past the end of a short one, both are the identity.
        assert_eq!((visual_for_logical(None, 7), logical_for_visual(None, 7)), (7, 7));
        let o = order(&cells("ab \u{5d0}\u{5d1}"), on(), false).unwrap();
        assert_eq!((visual_for_logical(Some(&o), 9), logical_for_visual(Some(&o), 9)), (9, 9));
        assert_eq!((visual_for_logical(Some(&o), 3), logical_for_visual(Some(&o), 4)), (4, 3));
        assert_eq!(visual_spans(&o, 2..5), vec![2..5]);
        assert_eq!(visual_spans(&o, 3..4), vec![4..5]);
        assert_eq!(visual_spans(&o, 4..5), vec![3..4]);
        assert_eq!(visual_spans(&o, 3..3), Vec::<std::ops::Range<usize>>::new());
        // A range that is not contiguous on screen is several pieces: in "a\u{5d0}\u{5d1}b" the
        // Hebrew run swaps, so logical 0..2 is visual 0 and 2 (the cells 0 and 2 apart).
        let o = order(&cells("a\u{5d0}\u{5d1}b"), on(), false).unwrap();
        assert_eq!(o.lut, vec![0, 2, 1, 3]);
        assert_eq!(visual_spans(&o, 0..2), vec![0..1, 2..3]);
        assert_eq!(visual_spans(&o, 1..3), vec![1..3]);
        // Past the order's end the columns map to themselves.
        assert_eq!(visual_spans(&o, 3..6), vec![3..6]);
    }
}
