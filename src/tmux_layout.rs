//! tmux window layouts (iTerm2's TmuxLayoutParser and the checksum of
//! iTermTmuxLayoutBuilder): `csum,WxH,X,Y` followed by a pane id, `{...}` (side by side),
//! or `[...]` (stacked). Tandem's `Layout` is binary, so an n-ary split becomes nested
//! splits whose ratios come from tmux's cell counts.

use std::collections::HashMap;

use crate::layout::{Axis, Layout, PaneId, Tree};

#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    Leaf { w: u32, h: u32, x: u32, y: u32, pane: Option<u32> },
    Split { axis: Axis, w: u32, h: u32, x: u32, y: u32, children: Vec<Node> },
}

impl Node {
    fn w(&self) -> u32 {
        match self {
            Node::Leaf { w, .. } | Node::Split { w, .. } => *w,
        }
    }

    fn h(&self) -> u32 {
        match self {
            Node::Leaf { h, .. } | Node::Split { h, .. } => *h,
        }
    }
}

/// `WxH,X,Y` at the start of `s`: the numbers and the length read.
fn header(s: &str) -> Option<([u32; 4], usize)> {
    let mut nums = [0u32; 4];
    let mut i = 0;
    let b = s.as_bytes();
    for (k, sep) in [b'x', b',', b',', 0].into_iter().enumerate() {
        let start = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        nums[k] = s[start..i].parse().ok()?;
        if sep != 0 {
            if b.get(i) != Some(&sep) {
                return None;
            }
            i += 1;
        }
    }
    Some((nums, i))
}

/// The layout, or None for a short or malformed one (iTerm2's parsedLayoutFromString).
pub fn parse(layout: &str) -> Option<Node> {
    if layout.len() <= 5 || !layout.is_char_boundary(5) {
        return None;
    }
    let mut tree = parse_node(&layout[5..])?;
    if let Node::Leaf { w, h, x, y, .. } = tree {
        // The root is always a split: a do-nothing one around a lone pane.
        tree = Node::Split { axis: Axis::Row, w, h, x, y, children: vec![tree] };
    }
    Some(coalesce(tree))
}

fn parse_node(s: &str) -> Option<Node> {
    // The node's kind is whichever bracket comes first (iTerm2's nodeTypeInLayout).
    let open = s.find(['[', '{']);
    let Some(open) = open else {
        // A leaf: the first `WxH,X,Y[,pane]` anywhere in it.
        let start = s.find(|c: char| c.is_ascii_digit())?;
        let ([w, h, x, y], n) = header(&s[start..])?;
        let rest = &s[start + n..];
        let pane = rest.strip_prefix(',').and_then(|r| r.split(|c: char| !c.is_ascii_digit()).next()).and_then(|d| d.parse().ok());
        return Some(Node::Leaf { w, h, x, y, pane });
    };
    let axis = if s.as_bytes()[open] == b'{' { Axis::Row } else { Axis::Column };
    let ([w, h, x, y], _) = header(s).unwrap_or(([0; 4], 0));
    let inner = children_range(s, open)?;
    Some(Node::Split { axis, w, h, x, y, children: parse_array(inner)? })
}

/// The text between the bracket at `open` and its match, counting either kind.
fn children_range(s: &str, open: usize) -> Option<&str> {
    let mut depth = 0;
    for (i, c) in s.bytes().enumerate().skip(open) {
        match c {
            b'[' | b'{' => depth += 1,
            b']' | b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&s[open + 1..i]);
                }
            }
            _ => {}
        }
    }
    None
}

fn parse_array(mut s: &str) -> Option<Vec<Node>> {
    let mut out = Vec::new();
    // A malformed item ends the list, as iTerm2's parseLayoutArray loop does.
    while let Some((first, rest)) = split_first(s) {
        out.push(parse_node(first)?);
        s = rest;
    }
    Some(out)
}

/// iTerm2's splitOffFirstLayoutInLayoutArray.
fn split_first(s: &str) -> Option<(&str, &str)> {
    let s = s.strip_prefix(',').unwrap_or(s);
    if s.is_empty() {
        return None;
    }
    let (_, mut n) = header(s)?;
    // The optional pane id (greedy, as the regex `(?:,[0-9]+)?`).
    let b = s.as_bytes();
    if b.get(n) == Some(&b',') && b.get(n + 1).is_some_and(|c| c.is_ascii_digit()) {
        n += 1;
        while n < b.len() && b[n].is_ascii_digit() {
            n += 1;
        }
    }
    match b.get(n) {
        None => Some((s, "")),
        Some(b',') => Some((&s[..n], &s[n + 1..])),
        Some(b'[' | b'{') => {
            let inner = children_range(s, n)?;
            let end = n + inner.len() + 2;
            Some((&s[..end], &s[end..]))
        }
        Some(_) => None,
    }
}

/// A child split along its parent's axis gives its children to the parent.
fn coalesce(node: Node) -> Node {
    match node {
        Node::Split { axis, w, h, x, y, children } => {
            let mut out = Vec::new();
            for c in children {
                match coalesce(c) {
                    Node::Split { axis: a, children: grand, .. } if a == axis => out.extend(grand),
                    c => out.push(c),
                }
            }
            Node::Split { axis, w, h, x, y, children: out }
        }
        leaf => leaf,
    }
}

/// pane-border-status: the status row is not in the layout, so a pane at the top (top) or
/// at the bottom (bottom) loses a row; never below one.
pub fn border_status(node: Node, top: bool, bottom: bool, window_h: u32) -> Node {
    match node {
        Node::Leaf { w, h, x, y, pane } if h > 1 => {
            if top && y == 0 {
                Node::Leaf { w, h: h - 1, x, y: y + 1, pane }
            } else if bottom && y + h == window_h {
                Node::Leaf { w, h: h - 1, x, y, pane }
            } else {
                Node::Leaf { w, h, x, y, pane }
            }
        }
        Node::Split { axis, w, h, x, y, children } => Node::Split { axis, w, h, x, y, children: children.into_iter().map(|c| border_status(c, top, bottom, window_h)).collect() },
        leaf => leaf,
    }
}

/// What a window of this layout draws in pixels at `cell` (width, height): tmux's own cells, a
/// divider counting as one cell as tmux counts it (Tandem's gap sits inside that cell).
pub fn natural_size(node: &Node, cell: (f32, f32), _gap: f32) -> (f32, f32) {
    (node.w() as f32 * cell.0, node.h() as f32 * cell.1)
}

/// What the size report takes off a tmux tab's area, for `pane_area`.
pub struct Room<'a> {
    pub cell: (f32, f32),
    pub gap: f32,
    /// The side and top margins.
    pub margins: (f32, f32),
    /// The title bar height, when the tab shows title bars.
    pub titles: Option<f32>,
    /// A pane's room at its right edge (scrollbar, timestamp column).
    pub pane_room: &'a dyn Fn(PaneId) -> f32,
}

/// The area a tmux tab of window `node` and `layout` draws its panes in: tmux's cells plus what
/// the size report took off (the inverse of the report), at the top left of `area`. Runs every
/// frame and on every pointer move, so it walks the layout in place.
pub fn pane_area(area: crate::layout::Rect, node: &Node, layout: &Layout, room: &Room) -> crate::layout::Rect {
    let stack = room.titles.map_or(0.0, |h| crate::layout::max_stack(layout, Axis::Column) as f32 * h);
    let row_room = crate::layout::widest_row_room(layout, &|id| (room.pane_room)(id));
    let natural = natural_size(node, room.cell, room.gap);
    drawn_area(area, natural, (2.0 * room.margins.0 + row_room, 2.0 * room.margins.1 + stack)).0
}

/// Where a tmux tab's panes are laid out in `area`, and the strips left for the filler: the panes
/// take tmux's size (`natural`, plus the `extra` width and height the size report took off) at the
/// top left; a dimension that is not larger than the area keeps the whole area.
pub fn drawn_area(area: crate::layout::Rect, natural: (f32, f32), extra: (f32, f32)) -> (crate::layout::Rect, [Option<crate::layout::Rect>; 2]) {
    use crate::layout::Rect;
    let drawn = Rect { w: (natural.0 + extra.0).min(area.w), h: (natural.1 + extra.1).min(area.h), ..area };
    // A fixed pair, right and below: the filler is drawn every frame.
    let right = (drawn.w < area.w).then_some(Rect { x: drawn.x + drawn.w, w: area.w - drawn.w, ..area });
    let below = (drawn.h < area.h).then_some(Rect { y: drawn.y + drawn.h, w: drawn.w, h: area.h - drawn.h, ..area });
    (drawn, [right, below])
}

/// `csum,text` with tmux's checksum (for layouts sent back to tmux).
pub fn checksum(text: &str) -> String {
    let mut csum: u16 = 0;
    for b in text.bytes() {
        csum = (csum >> 1).wrapping_add((csum & 1) << 15);
        csum = csum.wrapping_add(b as u16);
    }
    format!("{csum:04x},{text}")
}

/// The panes, first to last (iTerm2's windowPanesInParseTree; a leaf with no id is 0).
pub fn panes(node: &Node) -> Vec<u32> {
    sizes(node).into_iter().map(|(p, _, _)| p).collect()
}

/// Each pane's size in cells.
pub fn sizes(node: &Node) -> Vec<(u32, u32, u32)> {
    match node {
        Node::Leaf { w, h, pane, .. } => vec![(pane.unwrap_or(0), *w, *h)],
        Node::Split { children, .. } => children.iter().flat_map(sizes).collect(),
    }
}

/// Tandem's layout: each n-ary split nests to the right, and each ratio is the first
/// child's cells over the cells both sides share (tmux's one-cell divider is the gap).
pub fn to_layout(node: &Node, ids: &HashMap<u32, PaneId>) -> Option<Layout> {
    fn go(n: &Node, ids: &HashMap<u32, PaneId>) -> Option<Tree<PaneId>> {
        match n {
            Node::Leaf { pane, .. } => ids.get(&pane.unwrap_or(0)).map(|id| Tree::Leaf(*id)),
            Node::Split { axis, children, .. } => {
                let size = |c: &Node| if *axis == Axis::Row { c.w() } else { c.h() } as f32;
                let mut kids: Vec<(Tree<PaneId>, f32)> = children.iter().map(|c| Some((go(c, ids)?, size(c)))).collect::<Option<_>>()?;
                let (mut tree, mut cells) = kids.pop()?;
                while let Some((t, s)) = kids.pop() {
                    let ratio = if s + cells > 0.0 { s / (s + cells) } else { 0.5 };
                    tree = Tree::Split { axis: *axis, ratio, a: Box::new(t), b: Box::new(tree) };
                    // The rest now spans both and the divider between them.
                    cells += s + 1.0;
                }
                Some(tree)
            }
        }
    }
    go(node, ids).map(Layout::from_tree)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TWO_SPLITS: &str = "d67e,80x24,0,0{40x24,0,0,0,39x24,41,0[39x12,41,0,1,39x11,41,13,2]}";

    fn leaf(w: u32, h: u32, x: u32, y: u32, pane: u32) -> Node {
        Node::Leaf { w, h, x, y, pane: Some(pane) }
    }

    #[test]
    fn layouts_parse_like_iterm2() {
        assert_eq!(
            parse(TWO_SPLITS),
            Some(Node::Split {
                axis: Axis::Row,
                w: 80,
                h: 24,
                x: 0,
                y: 0,
                children: vec![leaf(40, 24, 0, 0, 0), Node::Split { axis: Axis::Column, w: 39, h: 24, x: 41, y: 0, children: vec![leaf(39, 12, 41, 0, 1), leaf(39, 11, 41, 13, 2)] }],
            })
        );
        // A lone pane gets a root split; a same-way child is hoisted.
        assert_eq!(parse("b25d,80x24,0,0,5"), Some(Node::Split { axis: Axis::Row, w: 80, h: 24, x: 0, y: 0, children: vec![leaf(80, 24, 0, 0, 5)] }));
        let hoisted = parse("abcd,80x24,0,0{20x24,0,0,1,59x24,21,0{29x24,21,0,2,29x24,51,0,3}}").unwrap();
        assert_eq!(panes(&hoisted), vec![1, 2, 3]);
        assert!(matches!(&hoisted, Node::Split { children, .. } if children.len() == 3));
        // Three deep, both kinds.
        let deep = parse("0000,80x24,0,0[80x12,0,0{40x12,0,0,1,39x12,41,0[39x6,41,0,2,39x5,41,7,3]},80x11,0,13,4]").unwrap();
        assert_eq!(panes(&deep), vec![1, 2, 3, 4]);
        // Every layout of the recorded session.
        let bytes = std::fs::read("tests/fixtures/tmux/wsl-3.4-layout.bin").unwrap();
        let text = String::from_utf8_lossy(&bytes);
        let layouts: Vec<&str> = text.lines().filter_map(|l| l.trim_end_matches('\r').strip_prefix("%layout-change @0 ")).map(|l| l.split(' ').next().unwrap()).collect();
        assert_eq!(layouts.len(), 2);
        for l in layouts {
            assert!(parse(l).is_some(), "{l}");
        }
    }

    #[test]
    fn bad_layouts_are_none() {
        assert_eq!(parse(""), None);
        assert_eq!(parse("d67e,"), None, "tmux 3.8 sends an empty layout when every pane floats");
        assert_eq!(parse("d67e,80x24,0,0{40x24,0,0,0"), None, "unbalanced");
        assert!(parse("d67e,80x24,0,0{40x24,0,0,0]").is_some(), "iTerm2 counts either bracket kind");
    }

    #[test]
    fn checksum_matches_tmux() {
        assert_eq!(checksum(&TWO_SPLITS[5..]), TWO_SPLITS);
        let one = "80x24,0,0{40x24,0,0,0,39x24,41,0,1}";
        assert_eq!(checksum(one), format!("8205,{one}"), "from the WSL transcript");
    }

    #[test]
    fn layouts_convert_cell_exact() {
        let node = parse(TWO_SPLITS).unwrap();
        assert_eq!(sizes(&node), vec![(0, 40, 24), (1, 39, 12), (2, 39, 11)]);
        let ids: HashMap<u32, PaneId> = [(0, 10), (1, 11), (2, 12)].into();
        let tree = to_layout(&node, &ids).unwrap().tree();
        let Tree::Split { axis: Axis::Row, ratio, a, b } = tree else { panic!("{tree:?}") };
        assert_eq!(*a, Tree::Leaf(10));
        assert!((ratio - 40.0 / 79.0).abs() < 1e-6, "{ratio}");
        let Tree::Split { axis: Axis::Column, ratio, a, b } = *b else { panic!() };
        assert_eq!((*a, *b), (Tree::Leaf(11), Tree::Leaf(12)));
        assert!((ratio - 12.0 / 23.0).abs() < 1e-6);
        // Three across: the first takes its cells of all three and both dividers' share.
        let three = parse("abcd,80x24,0,0{20x24,0,0,1,29x24,21,0,2,29x24,51,0,3}").unwrap();
        let ids: HashMap<u32, PaneId> = [(1, 1), (2, 2), (3, 3)].into();
        let Tree::Split { ratio, b, .. } = to_layout(&three, &ids).unwrap().tree() else { panic!() };
        assert!((ratio - 20.0 / 79.0).abs() < 1e-6, "{ratio}");
        assert!(matches!(*b, Tree::Split { ratio, .. } if (ratio - 0.5).abs() < 1e-6));
        // A pane Tandem has no id for: no layout.
        assert!(to_layout(&node, &HashMap::new()).is_none());
        // pane-border-status top: the top panes lose a row.
        let top = border_status(node.clone(), true, false, 24);
        assert_eq!(sizes(&top), vec![(0, 40, 23), (1, 39, 11), (2, 39, 11)]);
        let bottom = border_status(node, false, true, 24);
        assert_eq!(sizes(&bottom), vec![(0, 40, 23), (1, 39, 12), (2, 39, 10)]);
    }

    #[test]
    fn natural_size_counts_tmux_cells() {
        let one = parse("0000,80x24,0,0,0").unwrap();
        assert_eq!(natural_size(&one, (10.0, 20.0), 1.0), (800.0, 480.0));
        // Two 40-column panes side by side: the divider is one more cell.
        let two = parse("0000,81x24,0,0{40x24,0,0,0,40x24,41,0,1}").unwrap();
        assert_eq!(natural_size(&two, (10.0, 20.0), 1.0), (810.0, 480.0));
        assert_eq!(natural_size(&two, (10.0, 20.0), 10.0), (810.0, 480.0), "the gap is inside the divider cell");
        let stacked = parse("0000,80x25,0,0[80x12,0,0,0,80x12,0,13,1]").unwrap();
        assert_eq!(natural_size(&stacked, (10.0, 20.0), 1.0), (800.0, 500.0));
    }

    #[test]
    fn the_pane_area_builds_no_layout_tree() {
        use crate::layout::Rect;
        let node = parse("0000,81x24,0,0{40x24,0,0,0,40x24,41,0,1}").unwrap();
        let mut layout = Layout::new(1);
        layout.split(1, Axis::Row, 2);
        layout.split(2, Axis::Column, 3);
        let area = Rect { x: 0.0, y: 0.0, w: 2000.0, h: 1000.0 };
        let room = Room { cell: (10.0, 20.0), gap: 1.0, margins: (5.0, 2.0), titles: Some(18.0), pane_room: &|id| if id == 1 { 15.0 } else { 7.0 } };
        let before = crate::layout::tree_builds();
        let drawn = pane_area(area, &node, &layout, &room);
        assert_eq!(crate::layout::tree_builds(), before, "no tree is built per frame or pointer move");
        // 81 cells + margins + the widest row's room (15 + 7); 24 rows + margins + two stacked title bars.
        assert_eq!(drawn, Rect { x: 0.0, y: 0.0, w: 810.0 + 10.0 + 22.0, h: 480.0 + 4.0 + 36.0 });
        let none = Room { titles: None, ..room };
        assert_eq!(pane_area(area, &node, &layout, &none).h, 484.0);
    }

    #[test]
    fn the_filler_allocates_nothing_per_frame() {
        let src = include_str!("app/tmux_tabs_ui.rs");
        let body = src.split("fn tmux_filler(").nth(1).and_then(|r| r.split("\n    }\n").next()).expect("tmux_filler");
        assert!(!body.contains("Vec"), "the filler's strips are a fixed array, not a Vec per frame");
        assert!(body.find("tmux_node").is_some_and(|i| i < body.find("content_area_for").unwrap_or(usize::MAX)), "a tab that is not tmux returns before any work");
        assert_eq!(body.matches("content_area_for").count(), 1, "the content area is computed once");
    }

    #[test]
    fn a_smaller_window_leaves_filler_strips() {
        use crate::layout::Rect;
        let area = Rect { x: 10.0, y: 30.0, w: 1000.0, h: 600.0 };
        // Larger area: the panes at tmux's size at the top left, a strip right and one below.
        let (drawn, strips) = drawn_area(area, (800.0, 480.0), (10.0, 4.0));
        assert_eq!(drawn, Rect { x: 10.0, y: 30.0, w: 810.0, h: 484.0 });
        assert_eq!(strips, [Some(Rect { x: 820.0, y: 30.0, w: 190.0, h: 600.0 }), Some(Rect { x: 10.0, y: 514.0, w: 810.0, h: 116.0 })]);
        // A smaller area keeps the whole area, and no filler.
        let (drawn, strips) = drawn_area(area, (1200.0, 700.0), (0.0, 0.0));
        assert_eq!((drawn, strips), (area, [None, None]));
        // One dimension larger: only that strip.
        let (drawn, strips) = drawn_area(area, (1200.0, 400.0), (0.0, 0.0));
        assert_eq!((drawn.w, drawn.h, strips.iter().flatten().count()), (1000.0, 400.0, 1));
    }
}
