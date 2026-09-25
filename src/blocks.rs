//! OSC 1337 Block, UpdateBlock, and Button: command-output blocks with a gutter bar, copy
//! and fold buttons, and buttons a program puts in its output.

/// `key=value;key=value` pairs as iTerm2 reads them: split at the first `=`, pairs
/// without `=` dropped, the last of a repeated key wins.
pub fn parse(value: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for (k, v) in value.split(';').filter_map(|p| p.split_once('=')) {
        out.retain(|(old, _)| old != k);
        out.push((k.to_string(), v.to_string()));
    }
    out
}

/// The value of `key` in parsed pairs.
pub fn get<'a>(pairs: &'a [(String, String)], key: &str) -> Option<&'a str> {
    pairs.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
}

/// Blocks a screen knows: each started block gets a number (stamped on its rows), and an
/// id names the newest block started with it.
#[derive(Clone, Debug, Default)]
pub struct Blocks {
    next: u32,
    /// Blocks open now, innermost last: (id, number).
    open: Vec<(String, u32)>,
    /// The newest number for each id, and each number's type.
    pub by_id: std::collections::HashMap<String, u32>,
    pub types: std::collections::HashMap<u32, String>,
    /// Folded blocks: block number to fold id.
    pub folds: std::collections::HashMap<u32, u32>,
}

impl Blocks {
    /// A block starts: its number.
    pub fn start(&mut self, id: &str, kind: Option<&str>) -> u32 {
        self.next += 1;
        let n = self.next;
        self.open.push((id.to_string(), n));
        self.by_id.insert(id.to_string(), n);
        if let Some(k) = kind {
            self.types.insert(n, k.to_string());
        }
        n
    }

    /// A block ends. The number rows get stamped with now (the innermost open block's, or 0).
    pub fn end(&mut self, id: &str) -> u32 {
        self.open.retain(|(i, _)| i != id);
        self.current()
    }

    /// The innermost open block's number (0 for none).
    pub fn current(&self) -> u32 {
        self.open.last().map_or(0, |(_, n)| *n)
    }
}

/// A button on a block's hover pill.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PillButton {
    Copy,
    Fold,
}

/// Where a hovered block's pill goes: the view row (the block's first row, or the top of
/// the view when that scrolled off) and each button's column (fold rightmost, copy to its
/// left; fold only for blocks of 3 or more rows, or folded ones). None when the block is
/// out of view.
pub fn pill(first_row: i32, last_row: i32, view_rows: usize, cols: usize, can_fold: bool) -> Option<(usize, Vec<(PillButton, usize)>)> {
    if last_row < 0 || first_row >= view_rows as i32 || cols < 6 {
        return None;
    }
    let row = first_row.max(0) as usize;
    let buttons = if can_fold { vec![(PillButton::Fold, cols - 3), (PillButton::Copy, cols - 5)] } else { vec![(PillButton::Copy, cols - 3)] };
    Some((row, buttons))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screen::Screen;

    fn kv(k: &str, v: &str) -> (String, String) {
        (k.into(), v.into())
    }

    #[test]
    fn block_args_parse() {
        assert_eq!(parse("id=a;attr=start;type=markdown"), vec![kv("id", "a"), kv("attr", "start"), kv("type", "markdown")]);
        assert_eq!(parse("id=a=b;junk;id=c"), vec![kv("id", "c")], "first =, no-= dropped, last wins");
        let p = parse("code=3;icon=doc.on.doc");
        assert_eq!(get(&p, "icon"), Some("doc.on.doc"));
        assert_eq!(get(&p, "nope"), None);
    }

    #[test]
    fn blocks_tag_rows_and_reuse_ids() {
        let mut s = Screen::with_scrollback(20, 10, 100);
        s.feed(b"before\r\n\x1b]1337;Block=id=b1;attr=start\x07one\r\n\r\ntwo\r\n\x1b]1337;Block=id=b1;attr=end\x07after\r\n");
        assert_eq!(s.block_rows("b1"), Some((1, 3)), "rows written while open, blank ones too");
        assert_eq!(s.block_rows("nope"), None);
        // The same id again: the newest block wins.
        s.feed(b"\x1b]1337;Block=id=b1;attr=start\x07three\r\n\x1b]1337;Block=id=b1;attr=end\x07");
        assert_eq!(s.block_rows("b1"), Some((5, 5)));
        // Never ended: the rows so far, no panic.
        s.feed(b"\x1b]1337;Block=id=open;attr=start\x07x\r\ny");
        assert_eq!(s.block_rows("open"), Some((6, 7)));
        // Nested: the inner block's rows carry the inner number.
        s.feed(b"\x1b]1337;Block=id=in;attr=start\x07z\r\n\x1b]1337;Block=id=in;attr=end\x07w");
        assert_eq!(s.block_rows("in"), Some((7, 7)));
        assert_eq!(s.block_rows("open"), Some((6, 8)), "the outer block goes on after the inner one ends");
    }

    #[test]
    fn blocks_ignore_the_alternate_screen() {
        let mut s = Screen::new(20, 5);
        s.feed(b"\x1b[?1049h\x1b]1337;Block=id=alt;attr=start\x07x\r\n\x1b]1337;Block=id=alt;attr=end\x07");
        assert_eq!(s.block_rows("alt"), None);
    }
    #[test]
    fn hover_pill_sticks_to_the_top() {
        assert_eq!(pill(5, 9, 24, 80, true), Some((5, vec![(PillButton::Fold, 77), (PillButton::Copy, 75)])));
        assert_eq!(pill(-10, 3, 24, 80, true).map(|p| p.0), Some(0), "sticks to the top of the view");
        assert_eq!(pill(5, 6, 24, 80, false), Some((5, vec![(PillButton::Copy, 77)])), "short blocks: copy only");
        assert_eq!(pill(-10, -2, 24, 80, true), None, "all above the view");
        assert_eq!(pill(30, 40, 24, 80, true), None, "below the view");
    }

    #[test]
    fn folded_block_that_scrolled_out() {
        let mut s = Screen::with_scrollback(20, 4, 3);
        s.feed(b"\x1b]1337;Block=id=f;attr=start\x07a\r\nb\r\nc\r\n\x1b]1337;Block=id=f;attr=end\x07");
        s.feed(b"\x1b]1337;UpdateBlock=id=f;action=fold\x07");
        assert_eq!(s.fold_count(), 1, "folded");
        let (f, l) = s.block_rows("f").unwrap();
        assert_eq!(f, l, "one summary row, still tagged");
        for i in 0..20 {
            s.feed(format!("line {i}\r\n").as_bytes());
        }
        s.prune_tags();
        s.feed(b"\x1b]1337;UpdateBlock=id=f;action=unfold\x07");
        assert_eq!(s.block_rows("f"), None, "gone from the buffer; unfold did nothing");
        s.feed(b"\x1b]1337;Block=id=g;attr=start\x07x\r\ny\r\nz\r\n\x1b]1337;Block=id=g;attr=end\x07\x1b]1337;UpdateBlock=id=g;action=fold\x07");
        s.feed(b"\x1b]1337;UpdateBlock=id=g;action=unfold\x07");
        let (f, l) = s.block_rows("g").unwrap();
        assert_eq!(l - f, 2, "unfolded back to three rows");
    }
}
