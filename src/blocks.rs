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

/// Blocks still open at once, at most: a program that starts blocks and never ends them
/// drops the oldest.
pub const MAX_OPEN: usize = 64;

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
        if self.open.len() >= MAX_OPEN {
            self.open.remove(0);
        }
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

/// What an OSC 1337 Button is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ButtonKind {
    /// Copies the newest block with this id.
    Copy { block: String },
    /// Reports `ESC [ ? 1337 ; code ~` when clicked (trusted terminals only).
    Custom { code: u32, icon: String },
}

/// A button written into the text at a cell, 2 cells wide.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Button {
    pub id: u32,
    pub col: usize,
    pub kind: ButtonKind,
    /// False after an invalid custom button: drawn faint and inert.
    pub valid: bool,
}

/// What a Button sequence asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ButtonArgs {
    Add(ButtonKind),
    /// `type=custom` without a code above 0 and an icon: every custom button goes inert.
    Invalidate,
    Ignore,
}

/// iTerm2's reading of a Button sequence.
pub fn button_args(pairs: &[(String, String)]) -> ButtonArgs {
    match get(pairs, "type") {
        Some("copy") => match get(pairs, "block") {
            Some(b) => ButtonArgs::Add(ButtonKind::Copy { block: b.to_string() }),
            None => ButtonArgs::Ignore,
        },
        Some("custom") => match (int_value(get(pairs, "code").unwrap_or("")), get(pairs, "icon")) {
            (code, Some(icon)) if code > 0 => ButtonArgs::Add(ButtonKind::Custom { code: code as u32, icon: icon.to_string() }),
            _ => ButtonArgs::Invalidate,
        },
        _ => ButtonArgs::Ignore,
    }
}

/// Foundation's intValue: leading spaces, a sign, the digits that follow; 0 when none;
/// clamped to the 32-bit range.
fn int_value(s: &str) -> i64 {
    let t = s.trim_start();
    let (neg, rest) = match t.as_bytes().first() {
        Some(b'-') => (true, &t[1..]),
        Some(b'+') => (false, &t[1..]),
        _ => (false, t),
    };
    let mut v: i64 = 0;
    for d in rest.bytes().take_while(u8::is_ascii_digit) {
        v = (v * 10 + (d - b'0') as i64).min(i32::MAX as i64 + 1);
    }
    let v = if neg { -v } else { v };
    v.clamp(i32::MIN as i64, i32::MAX as i64)
}

/// The buttons of a screen: rows carry a key, and each key names the buttons on that row.
#[derive(Clone, Debug, Default)]
pub struct Buttons {
    next_id: u32,
    next_key: u32,
    /// The key count at which the next prune runs.
    prune_at: usize,
    pub rows: std::collections::HashMap<u32, Vec<Button>>,
}

impl Buttons {
    /// Time to drop the buttons of rows that are gone: at 256 keys, then at twice what the
    /// last prune kept, so a flood of buttons costs one scan per doubling.
    pub fn prune_due(&self) -> bool {
        self.rows.len() >= self.prune_at.max(256)
    }

    /// A prune ran.
    pub fn pruned(&mut self) {
        self.prune_at = (self.rows.len() * 2).max(256);
    }

    /// A new key for a row with no buttons yet.
    pub fn new_key(&mut self) -> u32 {
        self.next_key += 1;
        self.next_key
    }

    /// Adds a button under `key`: its id.
    pub fn add(&mut self, key: u32, col: usize, kind: ButtonKind) -> u32 {
        self.next_id += 1;
        let id = self.next_id;
        self.rows.entry(key).or_default().push(Button { id, col, kind, valid: true });
        id
    }

    /// An invalid custom button: every custom button so far goes inert.
    pub fn invalidate_custom(&mut self) {
        for b in self.rows.values_mut().flatten() {
            if matches!(b.kind, ButtonKind::Custom { .. }) {
                b.valid = false;
            }
        }
    }
}

/// What a click on a button does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Click {
    CopyBlock(String),
    /// Bytes to send to the program, as a report.
    Report(Vec<u8>),
    Nothing,
}

pub fn click(button: &Button, trusted: bool) -> Click {
    match &button.kind {
        ButtonKind::Copy { block } => Click::CopyBlock(block.clone()),
        ButtonKind::Custom { code, .. } if button.valid && trusted => Click::Report(format!("\x1b[?1337;{code}~").into_bytes()),
        ButtonKind::Custom { .. } => Click::Nothing,
    }
}

/// The button covering column `col` (a button is 2 cells wide).
pub fn button_hit(buttons: &[Button], col: usize) -> Option<&Button> {
    buttons.iter().rev().find(|b| col >= b.col && col < b.col + 2)
}

/// iTerm2's SF Symbol names drawn with Segoe Fluent Icons (Segoe MDL2 Assets on Windows 10,
/// which has the same code points). A `.fill` or `.circle` form draws its plain symbol.
pub fn icon_glyph(name: &str) -> Option<char> {
    const ICONS: &[(&str, u32)] = &[
        ("doc.on.doc", 0xE8C8), ("doc.on.clipboard", 0xE77F), ("scissors", 0xE8C6), ("doc", 0xE8A5), ("doc.text", 0xE8A5),
        ("folder", 0xE8B7), ("play", 0xE768), ("pause", 0xE769), ("stop", 0xE71A), ("arrow.clockwise", 0xE72C),
        ("arrow.counterclockwise", 0xE72C), ("arrow.triangle.2.circlepath", 0xE895), ("trash", 0xE74D), ("plus", 0xE710),
        ("minus", 0xE738), ("xmark", 0xE711), ("checkmark", 0xE73E), ("magnifyingglass", 0xE721), ("gear", 0xE713),
        ("gearshape", 0xE713), ("pencil", 0xE70F), ("square.and.pencil", 0xE70F), ("square.and.arrow.down", 0xE74E),
        ("square.and.arrow.up", 0xE72D), ("house", 0xE80F), ("info", 0xE946), ("exclamationmark.triangle", 0xE7BA),
        ("questionmark", 0xE897), ("link", 0xE71B), ("envelope", 0xE715), ("arrow.down", 0xE74B), ("arrow.up", 0xE74A),
        ("arrow.left", 0xE72B), ("arrow.right", 0xE72A), ("arrow.up.right.square", 0xE8A7), ("terminal", 0xE756),
        ("globe", 0xE774), ("lock", 0xE72E), ("lock.open", 0xE785), ("star", 0xE734), ("heart", 0xEB51), ("flag", 0xE7C1),
        ("pin", 0xE718), ("ladybug", 0xEBE8), ("ant", 0xEBE8), ("clock", 0xE823), ("calendar", 0xE787), ("person", 0xE77B),
        ("person.2", 0xE716), ("line.3.horizontal.decrease", 0xE71C), ("arrow.up.arrow.down", 0xE8CB), ("ellipsis", 0xE712),
        ("bookmark", 0xE8A4), ("cloud", 0xE753), ("icloud.and.arrow.up", 0xE898), ("icloud.and.arrow.down", 0xE896),
        ("arrow.uturn.backward", 0xE7A7), ("arrow.uturn.forward", 0xE7A6), ("camera", 0xE722), ("mic", 0xE720),
        ("speaker.wave.2", 0xE767), ("speaker.slash", 0xE74F), ("printer", 0xE749), ("bolt", 0xE945), ("eye", 0xE7B3),
        ("eye.slash", 0xED1A), ("chevron.left", 0xE76B), ("chevron.right", 0xE76C), ("chevron.up", 0xE70E),
        ("chevron.down", 0xE70D), ("tag", 0xE8EC), ("plus.magnifyingglass", 0xE8A3), ("minus.magnifyingglass", 0xE71F),
        ("wrench", 0xE90F), ("wrench.and.screwdriver", 0xE90F), ("hammer", 0xE90F), ("power", 0xE7E8), ("shuffle", 0xE8B1),
        ("repeat", 0xE8EE), ("list.bullet", 0xE8FD), ("chevron.left.forwardslash.chevron.right", 0xE943),
        ("rectangle.compress.vertical", 0xE73F), ("rectangle.expand.vertical", 0xE740),
    ];
    let find = |n: &str| ICONS.iter().find(|(k, _)| *k == n).and_then(|(_, c)| char::from_u32(*c));
    let plain = name.strip_suffix(".fill").unwrap_or(name);
    find(name).or_else(|| find(plain)).or_else(|| find(plain.strip_suffix(".circle").or(plain.strip_suffix(".square")).unwrap_or(plain)))
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

    #[test]
    fn buttons_parse_and_write_spaces() {
        let mut s = Screen::new(40, 5);
        s.feed(b"ab\x1b]1337;Button=type=copy;block=b1\x07c");
        let b = s.buttons_on_line(0);
        assert_eq!(b.len(), 1);
        assert_eq!((b[0].col, &b[0].kind, b[0].valid), (2, &ButtonKind::Copy { block: "b1".into() }, true));
        assert_eq!(s.line_text(0).trim_end(), "ab    c", "4 spaces after the button");
        s.feed(b"\x1b]1337;Button=type=custom;code=7;icon=play.fill\x07");
        let b = s.buttons_on_line(0);
        assert_eq!(b.len(), 2, "two buttons on one row");
        assert_eq!((b[1].col, &b[1].kind), (7, &ButtonKind::Custom { code: 7, icon: "play.fill".into() }));
        // Not a button: nothing added, nothing written.
        s.feed(b"\r\n\x1b]1337;Button=type=copy\x07\x1b]1337;Button=type=other;code=1;icon=x\x07");
        assert!(s.buttons_on_line(1).is_empty());
        assert_eq!(s.term().grid().cursor.point.column.0, 0);
        // An unknown icon still writes the spaces; it only draws nothing.
        s.feed(b"\x1b]1337;Button=type=custom;code=1;icon=no.such.symbol\x07");
        assert_eq!(s.term().grid().cursor.point.column.0, 4);
        assert_eq!(icon_glyph("no.such.symbol"), None);
        assert_eq!(icon_glyph("doc.on.doc"), Some('\u{E8C8}'));
        assert_eq!(icon_glyph("play.fill"), icon_glyph("play"), "fill variants map to the same icon");
    }

    #[test]
    fn invalid_custom_button_disables_all() {
        let mut s = Screen::new(40, 5);
        s.feed(b"\x1b]1337;Button=type=custom;code=3;icon=play\x07\x1b]1337;Button=type=copy;block=x\x07");
        s.feed(b"\x1b]1337;Button=type=custom;code=0;icon=play\x07");
        assert_eq!(s.term().grid().cursor.point.column.0, 8, "the invalid one writes nothing");
        let b = s.buttons_on_line(0);
        assert!(!b[0].valid, "custom buttons go inert");
        assert!(b[1].valid, "copy buttons stay");
        s.feed(b"\x1b]1337;Button=type=custom;code=4abc;icon=stop\x07");
        let b = s.buttons_on_line(0);
        assert_eq!(b.len(), 3);
        assert!(b[2].valid, "a new custom button is valid again (intValue reads 4)");
        assert_eq!(b[2].kind, ButtonKind::Custom { code: 4, icon: "stop".into() });
    }

    #[test]
    fn custom_button_clicks_report_when_trusted() {
        let custom = |valid| Button { id: 1, col: 0, kind: ButtonKind::Custom { code: 12, icon: "play".into() }, valid };
        assert_eq!(click(&custom(true), true), Click::Report(b"\x1b[?1337;12~".to_vec()));
        assert_eq!(click(&custom(true), false), Click::Nothing, "untrusted: nothing is sent");
        assert_eq!(click(&custom(false), true), Click::Nothing, "inert");
        let copy = Button { id: 2, col: 0, kind: ButtonKind::Copy { block: "b".into() }, valid: true };
        assert_eq!(click(&copy, false), Click::CopyBlock("b".into()), "copy is not trust-gated");
        assert_eq!(button_hit(&[custom(true), Button { col: 5, ..copy.clone() }], 6).map(|b| b.id), Some(2), "a button is 2 cells wide");
        assert_eq!(button_hit(&[custom(true)], 2), None);
    }

    #[test]
    fn block_text_is_plain_and_trimmed() {
        let mut s = Screen::new(10, 6);
        s.feed(b"\x1b]1337;Block=id=c;attr=start\x07ab  \r\n0123456789xy\r\ncd\x1b]1337;Block=id=c;attr=end\x07\r\n");
        assert_eq!(s.block_text("c").as_deref(), Some("ab\n0123456789xy\ncd\n"), "wrapped rows join, trailing spaces go");
        s.feed(b"\x1b[?1049hfull screen");
        assert_eq!(s.block_text("c").as_deref(), Some("ab\n0123456789xy\ncd\n"), "read from the main screen");
        assert_eq!(s.block_text("none"), None);
    }

    #[test]
    fn block_hover_walks_rows_and_skips_the_alt_screen() {
        let mut s = Screen::new(20, 8);
        s.feed(b"\x1b]1337;Block=id=b;attr=start\x07a\r\nb\r\nc\x1b]1337;Block=id=b;attr=end\x07\r\nx");
        let n = s.blocks.by_id["b"];
        assert_eq!(s.block_at_line(1), Some((n, 0, 2)));
        s.feed(b"\x1b]1337;Block=id=v;attr=start\x07");
        s.feed(b"\x1b[?1049hvim\r\nrows");
        assert_eq!(s.block_at_line(0), None, "no bars or pill over a full-screen app");
    }

    #[test]
    fn open_blocks_and_button_prunes_are_bounded() {
        let mut b = Blocks::default();
        for i in 0..1000 {
            b.start(&format!("never{i}"), None);
        }
        assert!(b.open.len() <= MAX_OPEN, "blocks that never end do not pile up");
        let mut bs = Buttons::default();
        for _ in 0..300 {
            let k = bs.new_key();
            bs.add(k, 0, ButtonKind::Copy { block: "x".into() });
        }
        assert!(bs.prune_due());
        bs.pruned();
        assert!(!bs.prune_due(), "after a prune that kept everything, the next waits for twice as many");
        for _ in 0..300 {
            let k = bs.new_key();
            bs.add(k, 0, ButtonKind::Copy { block: "x".into() });
        }
        assert!(bs.prune_due());
    }

    #[test]
    fn unfold_on_the_alt_screen_keeps_the_fold() {
        let mut s = Screen::new(20, 8);
        s.feed(b"\x1b]1337;Block=id=f;attr=start\x07a\r\nb\r\nc\r\n\x1b]1337;Block=id=f;attr=end\x07");
        assert!(s.fold_block("f"));
        s.feed(b"\x1b[?1049h");
        assert!(!s.unfold_block("f"));
        s.feed(b"\x1b[?1049l");
        assert!(s.unfold_block("f"), "the fold still belongs to the block");
    }

    #[test]
    fn buttons_past_the_edge_are_dropped() {
        let mut s = Screen::new(12, 4);
        s.feed(b"12345\x1b]1337;Button=type=copy;block=b\x07");
        assert_eq!(s.buttons_on_line(0).len(), 1);
        s.resize(6, 4);
        assert!(s.buttons_on_line(0).is_empty(), "a button at column 5 does not fit 6 columns");
    }
}
