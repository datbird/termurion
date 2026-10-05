use std::ops::Range;

use crate::find::Logical;
use crate::globals::Globals;
use crate::seltools;

/// The most atoms a word reaches each way (`kLongMaximumWordLength`, `TE:32`).
pub const LONG_MAX: usize = 100_000;
/// The most bytes a regex atom is matched over (B6: program output never drives an unbounded regex).
pub const ATOM_MAX: usize = 256;
/// The most regex calls one click makes; the class run then stands (B6).
pub const ATOM_BUDGET: usize = 20_000;
/// With a regex, atoms are laid out from this many cells before the click, to bound the work.
const REGEX_REACH: usize = ATOM_BUDGET / 2;
/// Backtracking steps one regex call may take (the crate's default is a million).
const BACKTRACK_LIMIT: usize = 20_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    Null,
    Whitespace,
    Word,
    Other,
}

/// What counts as a word: the list of extra characters, or (regex mode) a pattern whose longest
/// match at a position is one word atom. In regex mode the list still classifies single
/// characters, iTerm2's kept quirk (`iTermWordExtractor.swift:136-141`).
#[derive(Clone, Debug)]
pub struct Rule {
    pub regex_mode: bool,
    pub chars: String,
    /// Anchored to the start of the text it is matched against.
    pub regex: Option<fancy_regex::Regex>,
}

impl Rule {
    pub fn from_globals(g: &Globals) -> Rule {
        let regex_mode = g.int("WordSelectionMode") == 1;
        let regex = if regex_mode { Rule::compile(&g.string("WordSelectionRegexPattern")) } else { None };
        Rule { regex_mode, chars: g.string("WordCharacters"), regex }
    }

    /// `pattern` anchored and bounded; None for an empty or invalid one (regex mode then has no
    /// regex atoms).
    pub fn compile(pattern: &str) -> Option<fancy_regex::Regex> {
        if pattern.is_empty() {
            return None;
        }
        fancy_regex::RegexBuilder::new(&format!("\\A(?:{})", crate::smartsel::to_rust_regex(pattern)))
            .backtrack_limit(BACKTRACK_LIMIT)
            .delegate_size_limit(1 << 20)
            .delegate_dfa_size_limit(1 << 20)
            .build()
            .ok()
    }
}

/// A scalar that adds no cell of its own: combining marks, variation selectors, joiners.
fn zero_width(c: char) -> bool {
    matches!(c as u32, 0x200B..=0x200D | 0x0300..=0x036F | 0x1AB0..=0x1AFF | 0x1DC0..=0x1DFF | 0x20D0..=0x20FF | 0xFE00..=0xFE0F | 0xFE20..=0xFE2F | 0xE0100..=0xE01EF)
}

/// A cell's class (`WE:883-918`). Letters, digits and zero-width marks (so a decomposed accent
/// stays with its letter) are Word, as are the rule's extra characters.
pub fn class_of(ch: &str, rule: &Rule) -> Class {
    if ch.is_empty() {
        Class::Null
    } else if ch.chars().all(char::is_whitespace) {
        Class::Whitespace
    } else if ch.chars().all(|c| c.is_alphanumeric() || zero_width(c)) || rule.chars.contains(ch) {
        Class::Word
    } else {
        Class::Other
    }
}

/// Where the platform says a word ends (`NSAttributedString.doubleClick(at:)`).
pub trait Segmenter {
    /// The byte range of the word around byte offset `at` of `text`.
    fn word(&self, text: &str, at: usize) -> Range<usize>;
}

/// No segmentation: the whole run stands.
pub struct NoSegmenter;

impl Segmenter for NoSegmenter {
    fn word(&self, text: &str, _at: usize) -> Range<usize> {
        0..text.len()
    }
}

/// `text` as UTF-16 units, and for each unit index (and the end) the byte offset it starts at.
pub fn utf16_map(text: &str) -> (Vec<u16>, Vec<usize>) {
    let mut units = Vec::with_capacity(text.len());
    let mut byte_of = Vec::with_capacity(text.len() + 1);
    for (o, c) in text.char_indices() {
        let mut buf = [0u16; 2];
        for u in c.encode_utf16(&mut buf) {
            units.push(*u);
            byte_of.push(o);
        }
    }
    byte_of.push(text.len());
    (units, byte_of)
}

#[cfg(windows)]
mod icu {
    use std::ffi::c_void;
    use std::sync::OnceLock;

    use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};

    /// `UBRK_WORD` of `UBreakIteratorType` (`ubrk.h`).
    pub const UBRK_WORD: i32 = 1;
    pub const UBRK_DONE: i32 = -1;

    type Open = unsafe extern "C" fn(kind: i32, locale: *const u8, text: *const u16, len: i32, status: *mut i32) -> *mut c_void;
    type Edge = unsafe extern "C" fn(iter: *mut c_void, offset: i32) -> i32;
    type Close = unsafe extern "C" fn(iter: *mut c_void);

    pub struct Api {
        pub open: Open,
        pub preceding: Edge,
        pub following: Edge,
        pub close: Close,
    }

    /// The word break iterator of Windows' `icu.dll` (1903 and later), resolved at run time so
    /// `tandem.exe` still starts where the DLL or a name is missing.
    pub fn api() -> Option<&'static Api> {
        static API: OnceLock<Option<Api>> = OnceLock::new();
        API.get_or_init(|| unsafe {
            let name: Vec<u16> = "icu.dll\0".encode_utf16().collect();
            let module = LoadLibraryW(name.as_ptr());
            if module.is_null() {
                return None;
            }
            let get = |n: &std::ffi::CStr| GetProcAddress(module, n.as_ptr().cast());
            Some(Api {
                open: std::mem::transmute::<unsafe extern "system" fn() -> isize, Open>(get(c"ubrk_open")?),
                preceding: std::mem::transmute::<unsafe extern "system" fn() -> isize, Edge>(get(c"ubrk_preceding")?),
                following: std::mem::transmute::<unsafe extern "system" fn() -> isize, Edge>(get(c"ubrk_following")?),
                close: std::mem::transmute::<unsafe extern "system" fn() -> isize, Close>(get(c"ubrk_close")?),
            })
        })
        .as_ref()
    }
}

/// ICU's word break iterator; the whole text where `icu.dll` or a function is missing.
#[cfg(windows)]
pub struct IcuSegmenter;

#[cfg(windows)]
impl Segmenter for IcuSegmenter {
    fn word(&self, text: &str, at: usize) -> Range<usize> {
        let whole = 0..text.len();
        let Some(api) = icu::api() else { return whole };
        let (units, byte_of) = utf16_map(text);
        let (Ok(len), Some(at16)) = (i32::try_from(units.len()), byte_of.iter().position(|b| *b >= at)) else { return whole };
        let mut status = 0i32;
        // SAFETY: `units` outlives the iterator, which is closed before returning.
        unsafe {
            let it = (api.open)(icu::UBRK_WORD, c"".as_ptr().cast(), units.as_ptr(), len, &mut status);
            if it.is_null() || status > 0 {
                return whole;
            }
            let start = (api.preceding)(it, at16 as i32 + 1);
            let end = (api.following)(it, at16 as i32);
            (api.close)(it);
            let back = |i: i32| if i == icu::UBRK_DONE { None } else { byte_of.get(i as usize).copied() };
            match (back(start).or(Some(0)), back(end).or(Some(text.len()))) {
                (Some(a), Some(b)) if a <= at && at < b => a..b,
                _ => whole,
            }
        }
    }
}

/// One atom: cells `start..end` of the line (`RegexAtomIterator`): a regex match, or one cell.
struct Atom {
    start: usize,
    end: usize,
    /// A regex match counts as a word and always continues one.
    forced: bool,
}

/// The line's atoms, laid out left to right from a starting cell as they are asked for.
struct Atoms<'a> {
    line: &'a Logical,
    rule: &'a Rule,
    list: Vec<Atom>,
    next: usize,
    budget: usize,
    calls: usize,
}

impl<'a> Atoms<'a> {
    fn byte(&self, cell: usize) -> usize {
        self.line.cells.get(cell).map_or(self.line.text.len(), |c| c.0)
    }

    fn text(&self, a: &Atom) -> &'a str {
        &self.line.text[self.byte(a.start)..self.byte(a.end)]
    }

    /// The longest regex match at cell `at`, as the cell it ends before. Bounded: the match is
    /// tried over at most ATOM_MAX bytes, an error is no match, and the calls are budgeted.
    fn regex_end(&mut self, at: usize) -> Option<usize> {
        let rule: &'a Rule = self.rule;
        let re = rule.regex.as_ref().filter(|_| self.budget > 0)?;
        self.budget -= 1;
        self.calls += 1;
        let from = self.byte(at);
        let mut to = (from + ATOM_MAX).min(self.line.text.len());
        while !self.line.text.is_char_boundary(to) {
            to -= 1;
        }
        let m = re.find(&self.line.text[from..to]).ok().flatten().filter(|m| m.start() == 0 && m.end() > 0)?;
        let stop = from + m.end();
        let mut end = at + 1;
        while end < self.line.cells.len() && self.byte(end) < stop {
            end += 1;
        }
        Some(end)
    }

    /// Lays out one more atom; false at the end of the line.
    fn grow(&mut self) -> bool {
        let start = self.next;
        if start >= self.line.cells.len() {
            return false;
        }
        let (end, forced) = match self.regex_end(start) {
            Some(end) => (end, true),
            None => (start + 1, false),
        };
        self.list.push(Atom { start, end, forced });
        self.next = end;
        true
    }

    fn class(&self, a: &Atom) -> Class {
        if a.forced { Class::Word } else { class_of(self.text(a), self.rule) }
    }

    /// Whether the atom always continues a word: a regex match, or in list mode a member of the list.
    fn extends(&self, a: &Atom) -> bool {
        a.forced || (!self.rule.regex_mode && !self.rule.chars.is_empty() && a.end - a.start == 1 && self.rule.chars.contains(self.text(a)))
    }
}

/// The byte range of the word at byte offset `click` of `line.text` (`iTermWordExtractor`): the
/// run of atoms of the click's class, refined by the segmenter for a word. A click past the
/// text's end selects nothing (an empty range); a click on an Other atom selects it alone.
pub fn word_at(line: &Logical, click: usize, rule: &Rule, seg: &dyn Segmenter) -> Range<usize> {
    word_at_counted(line, click, rule, seg).0
}

/// `word_at` and the number of regex calls it made.
pub fn word_at_counted(line: &Logical, click: usize, rule: &Rule, seg: &dyn Segmenter) -> (Range<usize>, usize) {
    let len = line.text.len();
    if click >= len || line.cells.is_empty() {
        return (len..len, 0);
    }
    let click_cell = line.cells.partition_point(|c| c.0 <= click).max(1) - 1;
    let reach = if rule.regex.is_some() { REGEX_REACH } else { LONG_MAX };
    let first = click_cell.saturating_sub(reach);
    let mut atoms = Atoms { line, rule, list: Vec::new(), next: first, budget: ATOM_BUDGET, calls: 0 };
    while atoms.list.last().map_or(true, |a| a.end <= click_cell) {
        if !atoms.grow() {
            break;
        }
    }
    let Some(ci) = atoms.list.iter().position(|a| a.end > click_cell) else { return (len..len, atoms.calls) };
    let range = run(&mut atoms, ci, seg);
    (range, atoms.calls)
}

fn run(atoms: &mut Atoms, ci: usize, seg: &dyn Segmenter) -> Range<usize> {
    let class = atoms.class(&atoms.list[ci]);
    let own = |atoms: &Atoms, i: usize| atoms.byte(atoms.list[i].start)..atoms.byte(atoms.list[i].end);
    if class == Class::Other {
        return own(atoms, ci);
    }
    let mut s = ci;
    while s > 0 && ci - s < LONG_MAX && atoms.class(&atoms.list[s - 1]) == class {
        s -= 1;
    }
    let mut e = ci + 1;
    while e - ci < LONG_MAX {
        if e >= atoms.list.len() && !atoms.grow() {
            break;
        }
        if atoms.class(&atoms.list[e]) != class {
            break;
        }
        e += 1;
    }
    let big = ci - s >= LONG_MAX || e - ci >= LONG_MAX;
    if class != Class::Word || big {
        return atoms.byte(atoms.list[s].start)..atoms.byte(atoms.list[e - 1].end);
    }
    segment(atoms, s, e, ci, seg)
}

/// `applyICUSegmentationToAtoms`: the run `s..e` refined by the segmenter from the click atom
/// outward, word-extending atoms always continuing.
fn segment(atoms: &Atoms, s: usize, e: usize, ci: usize, seg: &dyn Segmenter) -> Range<usize> {
    let base = atoms.byte(atoms.list[s].start);
    let combined = &atoms.line.text[base..atoms.byte(atoms.list[e - 1].end)];
    let at = |i: usize| atoms.byte(atoms.list[i].start) - base;
    // Forward.
    let mut new_end = ci;
    let mut i = ci;
    while i < e {
        if atoms.extends(&atoms.list[i]) {
            new_end = i + 1;
            i += 1;
            continue;
        }
        let end = seg.word(combined, at(i)).end;
        for j in i..e {
            if at(j) < end {
                new_end = j + 1;
            } else {
                break;
            }
        }
        if new_end < e && atoms.extends(&atoms.list[new_end]) && new_end > i {
            i = new_end;
        } else {
            break;
        }
    }
    // Backward.
    let mut new_start = ci;
    let mut i = ci;
    loop {
        if atoms.extends(&atoms.list[i]) {
            new_start = i;
            if i == s {
                break;
            }
            i -= 1;
            continue;
        }
        let start = seg.word(combined, at(i)).start;
        for j in (s..=i).rev() {
            if at(j) >= start {
                new_start = j;
            } else {
                break;
            }
        }
        if new_start > s && atoms.extends(&atoms.list[new_start - 1]) && new_start < i + 1 && new_start - 1 < i {
            i = new_start - 1;
        } else {
            break;
        }
    }
    if new_start >= new_end {
        return atoms.byte(atoms.list[ci].start)..atoms.byte(atoms.list[ci].end);
    }
    atoms.byte(atoms.list[new_start].start)..atoms.byte(atoms.list[new_end - 1].end)
}

/// The first and last column between the nearest soft boundaries (tmux dividers) either side of
/// `col` on `rows[row]`, None where the row has none.
pub fn window_at(rows: &[Vec<char>], row: usize, col: usize, width: usize) -> Option<(usize, usize)> {
    let dividers = seltools::divider_columns(rows, row);
    if dividers.is_empty() || width == 0 {
        return None;
    }
    Some(seltools::clip_to_dividers(0, width - 1, col, &dividers))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::find::Pos;

    fn rule(chars: &str) -> Rule {
        Rule { regex_mode: false, chars: chars.into(), regex: None }
    }

    fn default_rule() -> Rule {
        Rule::from_globals(&Globals::empty(std::path::Path::new("unused.toml")))
    }

    fn regex_rule(pattern: &str, chars: &str) -> Rule {
        Rule { regex_mode: true, chars: chars.into(), regex: Rule::compile(pattern) }
    }

    /// One row of text as a logical line: one cell per character, a wide one taking two columns
    /// of which the spacer is left out.
    fn line(text: &str) -> Logical {
        let mut cells = Vec::new();
        let mut col = 0;
        for (o, c) in text.char_indices() {
            cells.push((o, Pos { row: 0, col }));
            col += if (c as u32) >= 0x3000 { 2 } else { 1 };
        }
        Logical { text: text.to_string(), cells }
    }

    fn pick<'a>(text: &'a str, needle: &str, nth: usize, rule: &Rule, seg: &dyn Segmenter) -> &'a str {
        let at = text.match_indices(needle).nth(nth).unwrap().0;
        &text[word_at(&line(text), at, rule, seg)]
    }

    #[test]
    fn classes_follow_iterm2() {
        let r = default_rule();
        for w in ["a", "\u{e9}", "7", "\u{65e5}", "/", "-", "+", "\\", "~", "_", "."] {
            assert_eq!(class_of(w, &r), Class::Word, "{w:?}");
        }
        for o in ["=", "@", "#", ";", ",", ":"] {
            assert_eq!(class_of(o, &r), Class::Other, "{o:?}");
        }
        assert_eq!(class_of(" ", &r), Class::Whitespace);
        assert_eq!(class_of("\t", &r), Class::Whitespace);
        assert_eq!(class_of("", &r), Class::Null);
        assert_eq!(class_of(".", &rule("")), Class::Other);
        // A decomposed e with its accent is one Word cell (P5).
        assert_eq!(class_of("e\u{301}", &r), Class::Word);
    }

    #[test]
    fn word_runs_follow_iterm2() {
        let r = default_rule();
        let n = &NoSegmenter;
        assert_eq!(pick("cd /usr/local/bin; ls", "local", 0, &r, n), "/usr/local/bin");
        assert_eq!(pick("cd /usr/local/bin; ls", ";", 0, &r, n), ";");
        assert_eq!(pick("cd /usr/local/bin; ls", " ", 0, &r, n), " ");
        assert_eq!(pick("a==b", "=", 0, &r, n), "=");
        assert_eq!(pick("foo  bar", " ", 1, &r, n), "  ");
        assert_eq!(pick("x\u{65e5}\u{672c}y", "\u{65e5}", 0, &r, n), "x\u{65e5}\u{672c}y");
        // Past the text's end nothing wider than the click cell is selected.
        let l = line("abc");
        assert!(word_at(&l, 7, &r, n).is_empty());
    }

    #[test]
    fn regex_words_follow_iterm2() {
        let n = &NoSegmenter;
        let r = regex_rule("-|\\+|\\\\|~|_", "");
        assert_eq!(pick("a-b.c", "a", 0, &r, n), "a-b");
        let r = regex_rule("[0-9]+\\.[0-9]+", "");
        assert_eq!(pick("v1.25 ok", "2", 0, &r, n), "v1.25");
        // The word list still classifies single characters in regex mode (iTerm2 quirk).
        let r = regex_rule("_", ".");
        assert_eq!(pick("a.b", "a", 0, &r, n), "a.b");
    }

    /// Breaks at every change between ASCII and non-ASCII letters; anything else is its own segment.
    struct Fake;

    impl Segmenter for Fake {
        fn word(&self, text: &str, at: usize) -> Range<usize> {
            let kind = |c: char| match (c.is_alphabetic(), c.is_ascii()) {
                (true, true) => 1,
                (true, false) => 2,
                _ => 0,
            };
            let chars: Vec<(usize, char)> = text.char_indices().collect();
            let i = chars.iter().position(|(o, _)| *o == at).unwrap();
            let k = kind(chars[i].1);
            if k == 0 {
                return at..at + chars[i].1.len_utf8();
            }
            let (mut a, mut b) = (i, i);
            while a > 0 && kind(chars[a - 1].1) == k {
                a -= 1;
            }
            while b + 1 < chars.len() && kind(chars[b + 1].1) == k {
                b += 1;
            }
            chars[a].0..chars[b].0 + chars[b].1.len_utf8()
        }
    }

    #[test]
    fn segmentation_bridges_word_characters() {
        assert_eq!(pick("abc\u{65e5}\u{672c}", "b", 0, &rule(""), &Fake), "abc");
        // A word-extending character continues past the segment's end.
        assert_eq!(pick("abc.\u{65e5}", "b", 0, &rule("."), &Fake), "abc.\u{65e5}");
        // The regex's match bridges too.
        assert_eq!(pick("abc_\u{65e5}", "b", 0, &regex_rule("_", ""), &Fake), "abc_\u{65e5}");
    }

    #[test]
    fn soft_boundary_windows_follow_iterm2() {
        let rows: Vec<Vec<char>> = (0..DIVIDER_ROWS).map(|i| if i == 20 { "aaaaaaa" } else if i % 2 == 0 { "aaa\u{2502}bbb" } else { "ccc\u{2502}ddd" }.chars().collect()).collect();
        assert_eq!(window_at(&rows, 0, 1, 7), Some((0, 2)));
        assert_eq!(window_at(&rows, 3, 5, 7), Some((4, 6)));
        assert_eq!(window_at(&rows, 20, 5, 7), None);
    }

    const DIVIDER_ROWS: usize = 21;

    #[test]
    fn utf16_offsets_follow_surrogates() {
        let (units, byte_of) = utf16_map("a\u{1f600}b");
        assert_eq!(units.len(), 4);
        assert_eq!(byte_of, vec![0, 1, 1, 5, 6]);
    }

    #[test]
    fn regex_words_are_bounded() {
        // A pattern that never matches, and a backtracking one, over one huge line.
        let text: String = "0123456789".repeat(10_000);
        for pattern in ["(a*)*b", "[0-9]+\\.[0-9]+"] {
            let r = regex_rule(pattern, "");
            let l = line(&text);
            let (range, calls) = word_at_counted(&l, 50_000, &r, &NoSegmenter);
            assert!(calls <= ATOM_BUDGET, "{pattern}: {calls} regex calls");
            assert!(!range.is_empty());
        }
    }
}
