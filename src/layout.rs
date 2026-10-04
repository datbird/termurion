//! Split panes: a binary tree of panes, and the rectangles they get.

pub type PaneId = u64;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }

    pub fn inset(&self, by: f32) -> Rect {
        Rect { x: self.x + by, y: self.y + by, w: (self.w - 2.0 * by).max(0.0), h: (self.h - 2.0 * by).max(0.0) }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    /// Side by side, split by a vertical line.
    Row,
    /// Stacked, split by a horizontal line.
    Column,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Divider {
    pub rect: Rect,
    /// Which child to take at each split from the root: false = first, true = second.
    pub path: Vec<bool>,
    pub axis: Axis,
    /// The area of the split this divider belongs to.
    pub parent: Rect,
}

/// The pane before the divider at `path` (left of it or above it): tmux resizes that one,
/// as iTerm2 resizes the view before the splitter.
pub fn pane_before(tree: &Tree<PaneId>, path: &[bool]) -> Option<PaneId> {
    let mut node = tree;
    for step in path {
        node = match node {
            Tree::Split { a, b, .. } => if *step { b } else { a },
            Tree::Leaf(_) => return None,
        };
    }
    match node {
        Tree::Split { a, .. } => a.leaves().first().map(|p| **p),
        Tree::Leaf(_) => None,
    }
}

/// A layout's shape with any leaf type (arrangements carry sessions in the leaves).
#[derive(Clone, Debug, PartialEq)]
pub enum Tree<T> {
    Leaf(T),
    Split { axis: Axis, ratio: f32, a: Box<Tree<T>>, b: Box<Tree<T>> },
}

impl<T> Tree<T> {
    /// The same shape with each leaf mapped (in order, left or top first).
    pub fn map<U>(self, f: &mut impl FnMut(T) -> U) -> Tree<U> {
        match self {
            Tree::Leaf(t) => Tree::Leaf(f(t)),
            Tree::Split { axis, ratio, a, b } => {
                let a = a.map(f);
                Tree::Split { axis, ratio, a: Box::new(a), b: Box::new(b.map(f)) }
            }
        }
    }

    /// The same shape without the leaves `keep` refuses; a split that loses one side
    /// becomes its other side. None when no leaf is kept.
    pub fn retain(self, keep: &mut impl FnMut(&T) -> bool) -> Option<Tree<T>> {
        match self {
            Tree::Leaf(t) => keep(&t).then_some(Tree::Leaf(t)),
            Tree::Split { axis, ratio, a, b } => match (a.retain(keep), b.retain(keep)) {
                (Some(a), Some(b)) => Some(Tree::Split { axis, ratio, a: Box::new(a), b: Box::new(b) }),
                (one, other) => one.or(other),
            },
        }
    }

    /// The leaves in order.
    pub fn leaves(&self) -> Vec<&T> {
        match self {
            Tree::Leaf(t) => vec![t],
            Tree::Split { a, b, .. } => [a.leaves(), b.leaves()].concat(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Node {
    Leaf(PaneId),
    Split { axis: Axis, ratio: f32, a: Box<Node>, b: Box<Node> },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    root: Node,
}

/// Splits `r` into (first, second, divider).
fn split_rect(r: Rect, axis: Axis, ratio: f32, gap: f32) -> (Rect, Rect, Rect) {
    match axis {
        Axis::Row => {
            let aw = ((r.w - gap) * ratio).floor();
            (
                Rect { w: aw, ..r },
                Rect { x: r.x + aw + gap, w: r.w - aw - gap, ..r },
                Rect { x: r.x + aw, w: gap, ..r },
            )
        }
        Axis::Column => {
            let ah = ((r.h - gap) * ratio).floor();
            (
                Rect { h: ah, ..r },
                Rect { y: r.y + ah + gap, h: r.h - ah - gap, ..r },
                Rect { y: r.y + ah, h: gap, ..r },
            )
        }
    }
}

impl Layout {
    pub fn new(pane: PaneId) -> Self {
        Layout { root: Node::Leaf(pane) }
    }

    /// The layout's shape.
    pub fn tree(&self) -> Tree<PaneId> {
        fn go(n: &Node) -> Tree<PaneId> {
            match n {
                Node::Leaf(p) => Tree::Leaf(*p),
                Node::Split { axis, ratio, a, b } => Tree::Split { axis: *axis, ratio: *ratio, a: Box::new(go(a)), b: Box::new(go(b)) },
            }
        }
        go(&self.root)
    }

    /// A layout of that shape.
    pub fn from_tree(t: Tree<PaneId>) -> Layout {
        fn go(t: Tree<PaneId>) -> Node {
            match t {
                Tree::Leaf(p) => Node::Leaf(p),
                Tree::Split { axis, ratio, a, b } => {
                    // A broken file can give NaN or infinite sizes.
                    let ratio = if ratio.is_finite() { ratio.clamp(0.02, 0.98) } else { 0.5 };
                    Node::Split { axis, ratio, a: Box::new(go(*a)), b: Box::new(go(*b)) }
                }
            }
        }
        Layout { root: go(t) }
    }

    /// Splits `target`. `new` goes right of it (Row) or below it (Column).
    /// False if `target` is not in this layout.
    pub fn split(&mut self, target: PaneId, axis: Axis, new: PaneId) -> bool {
        fn go(n: &mut Node, t: PaneId, axis: Axis, new: PaneId) -> bool {
            match n {
                Node::Leaf(id) if *id == t => {
                    *n = Node::Split { axis, ratio: 0.5, a: Box::new(Node::Leaf(t)), b: Box::new(Node::Leaf(new)) };
                    true
                }
                Node::Leaf(_) => false,
                Node::Split { a, b, .. } => go(a, t, axis, new) || go(b, t, axis, new),
            }
        }
        go(&mut self.root, target, axis, new)
    }

    /// Removes `target`, and its sibling takes its space. False if `target` is not
    /// here or is the last pane.
    pub fn remove(&mut self, target: PaneId) -> bool {
        fn go(n: &mut Node, t: PaneId) -> bool {
            let Node::Split { a, b, .. } = n else { return false };
            let keep = if **a == Node::Leaf(t) {
                std::mem::replace(&mut **b, Node::Leaf(0))
            } else if **b == Node::Leaf(t) {
                std::mem::replace(&mut **a, Node::Leaf(0))
            } else {
                return go(a, t) || go(b, t);
            };
            *n = keep;
            true
        }
        go(&mut self.root, target)
    }

    pub fn panes(&self) -> Vec<PaneId> {
        self.rects(Rect { x: 0.0, y: 0.0, w: 0.0, h: 0.0 }, 0.0).into_iter().map(|(id, _)| id).collect()
    }

    pub fn rects(&self, area: Rect, gap: f32) -> Vec<(PaneId, Rect)> {
        fn go(n: &Node, r: Rect, gap: f32, out: &mut Vec<(PaneId, Rect)>) {
            match n {
                Node::Leaf(id) => out.push((*id, r)),
                Node::Split { axis, ratio, a, b } => {
                    let (ra, rb, _) = split_rect(r, *axis, *ratio, gap);
                    go(a, ra, gap, out);
                    go(b, rb, gap, out);
                }
            }
        }
        let mut out = Vec::new();
        go(&self.root, area, gap, &mut out);
        out
    }

    pub fn rect_of(&self, id: PaneId, area: Rect, gap: f32) -> Option<Rect> {
        self.rects(area, gap).into_iter().find(|(p, _)| *p == id).map(|(_, r)| r)
    }

    pub fn dividers(&self, area: Rect, gap: f32) -> Vec<Divider> {
        fn go(n: &Node, r: Rect, gap: f32, path: &mut Vec<bool>, out: &mut Vec<Divider>) {
            if let Node::Split { axis, ratio, a, b } = n {
                let (ra, rb, rd) = split_rect(r, *axis, *ratio, gap);
                out.push(Divider { rect: rd, path: path.clone(), axis: *axis, parent: r });
                path.push(false);
                go(a, ra, gap, path, out);
                path.pop();
                path.push(true);
                go(b, rb, gap, path, out);
                path.pop();
            }
        }
        let mut out = Vec::new();
        go(&self.root, area, gap, &mut Vec::new(), &mut out);
        out
    }

    /// Puts every divider back in the middle of its split.
    pub fn even(&mut self) {
        fn go(n: &mut Node) {
            if let Node::Split { ratio, a, b, .. } = n {
                *ratio = 0.5;
                go(a);
                go(b);
            }
        }
        go(&mut self.root);
    }

    /// Moves the divider on `dir`'s side of `pane` by `fraction` of its split, toward `dir`.
    /// Returns false when `pane` has no divider on that side.
    pub fn nudge(&mut self, pane: PaneId, dir: Dir, fraction: f32) -> bool {
        let axis = match dir {
            Dir::Left | Dir::Right => Axis::Row,
            Dir::Up | Dir::Down => Axis::Column,
        };
        let toward_second = matches!(dir, Dir::Right | Dir::Down);
        // Walk down to the pane, remembering each split on the right axis where the pane
        // sits on the side that faces `dir`. The deepest one is the adjacent divider.
        fn go(n: &mut Node, pane: PaneId, axis: Axis, toward_second: bool, fraction: f32) -> Option<bool> {
            let Node::Split { axis: a, ratio, a: first, b: second } = n else {
                return if *n == Node::Leaf(pane) { Some(false) } else { None };
            };
            let in_first = go(first, pane, axis, toward_second, fraction);
            let (found, moved) = match in_first {
                Some(m) => (true, m),
                None => match go(second, pane, axis, toward_second, fraction) {
                    Some(m) => (false, m),
                    None => return None,
                },
            };
            if moved || *a != axis || found != toward_second {
                return Some(moved);
            }
            let step = if toward_second { fraction } else { -fraction };
            *ratio = (*ratio + step).clamp(0.1, 0.9);
            Some(true)
        }
        go(&mut self.root, pane, axis, toward_second, fraction).unwrap_or(false)
    }

    /// Exchanges two panes' places. False unless both are here.
    pub fn swap(&mut self, a: PaneId, b: PaneId) -> bool {
        fn go(n: &mut Node, a: PaneId, b: PaneId) {
            match n {
                Node::Leaf(id) if *id == a => *id = b,
                Node::Leaf(id) if *id == b => *id = a,
                Node::Leaf(_) => {}
                Node::Split { a: first, b: second, .. } => {
                    go(first, a, b);
                    go(second, a, b);
                }
            }
        }
        let panes = self.panes();
        if a == b || !panes.contains(&a) || !panes.contains(&b) {
            return false;
        }
        go(&mut self.root, a, b);
        true
    }

    /// Grows `pane` along `axis` by `by` of the nearest split on that axis (negative
    /// shrinks it). False when no split on that axis holds the pane.
    pub fn grow(&mut self, pane: PaneId, axis: Axis, by: f32) -> bool {
        fn go(n: &mut Node, pane: PaneId, axis: Axis, by: f32) -> Option<bool> {
            let Node::Split { axis: a, ratio, a: first, b: second } = n else {
                return if *n == Node::Leaf(pane) { Some(false) } else { None };
            };
            let (in_first, done) = match go(first, pane, axis, by) {
                Some(d) => (true, d),
                None => (false, go(second, pane, axis, by)?),
            };
            if done || *a != axis {
                return Some(done);
            }
            *ratio = (*ratio + if in_first { by } else { -by }).clamp(0.1, 0.9);
            Some(true)
        }
        go(&mut self.root, pane, axis, by).unwrap_or(false)
    }

    /// The pane after (or before) `from` in reading order, wrapping. None with one pane.
    pub fn next(&self, from: PaneId, forward: bool) -> Option<PaneId> {
        let panes = self.panes();
        let n = panes.len();
        let i = panes.iter().position(|p| *p == from)?;
        (n > 1).then(|| panes[if forward { (i + 1) % n } else { (i + n - 1) % n }])
    }

    /// Sets the split ratio at `path`, kept between 0.1 and 0.9.
    pub fn set_ratio(&mut self, path: &[bool], ratio: f32) {
        let mut n = &mut self.root;
        for &second in path {
            n = match n {
                Node::Split { a, b, .. } => {
                    if second {
                        &mut **b
                    } else {
                        &mut **a
                    }
                }
                Node::Leaf(_) => return,
            };
        }
        if let Node::Split { ratio: r, .. } = n {
            *r = ratio.clamp(0.1, 0.9);
        }
    }

    /// The panes B in `from`'s projection looking `dir` (`sessionsInProjectionOfSession:`,
    /// `TAB:1454-1487`): B overlaps `from` on the cross axis by more than `margin` and starts at
    /// or past where `from` ends. `rects` are the pane rectangles (`after` false mirrors them).
    fn projection(rects: &[(PaneId, Rect)], from: PaneId, vertical: bool, after: bool, margin: f32) -> Vec<PaneId> {
        let Some(a) = rects.iter().find(|(id, _)| *id == from).map(|(_, r)| Span::of(r, vertical, after)) else {
            return Vec::new();
        };
        rects
            .iter()
            .filter(|(id, r)| {
                let b = Span::of(r, vertical, after);
                *id != from && b.bottom >= a.top + margin && b.top + margin < a.bottom && b.left >= a.right
            })
            .map(|(id, _)| *id)
            .collect()
    }

    /// The panes adjacent to `from` looking `dir`: in its projection with no other pane of the
    /// projection between them (`sessionsAdjacentToSession:`, `TAB:1489-1531`). `margin` is
    /// iTerm2's 1-point safety margin in pixels.
    pub fn adjacent(&self, from: PaneId, dir: Dir, area: Rect, gap: f32, margin: f32) -> Vec<PaneId> {
        let rects = self.rects(area, gap);
        let (vertical, after) = axis_of(dir);
        let candidates = Self::projection(&rects, from, vertical, after, margin);
        let span = |id: PaneId| rects.iter().find(|(p, _)| *p == id).map(|(_, r)| Span::of(r, vertical, after));
        let Some(a) = span(from) else { return Vec::new() };
        candidates
            .iter()
            .copied()
            .filter(|&b| {
                let Some(b) = span(b) else { return false };
                !candidates.iter().any(|&c| {
                    let Some(c) = span(c) else { return false };
                    c != b
                        && c.bottom >= a.top
                        && c.top < a.bottom
                        && c.bottom >= b.top
                        && c.top < b.bottom
                        && c.left >= a.right
                        && c.right < b.left
                })
            })
            .collect()
    }

    /// The panes to wrap to when nothing is adjacent: those in the projection looking the other
    /// way with the smallest `left`, the far edge (`TAB:1543-1560`).
    pub fn wraparound(&self, from: PaneId, dir: Dir, area: Rect, gap: f32, margin: f32) -> Vec<PaneId> {
        let rects = self.rects(area, gap);
        let (vertical, after) = axis_of(dir);
        let far = Self::projection(&rects, from, vertical, !after, margin);
        let left = |id: PaneId| rects.iter().find(|(p, _)| *p == id).map_or(f32::MAX, |(_, r)| Span::of(r, vertical, after).left);
        let min = far.iter().map(|&id| left(id)).fold(f32::MAX, f32::min);
        far.into_iter().filter(|&id| left(id) - min < 0.01).collect()
    }
}

/// A pane's extent in the projection's frame: `top` and `bottom` across, `left` and `right`
/// along the direction (negated when looking before, so "after" always means larger).
#[derive(Clone, Copy, PartialEq)]
struct Span {
    top: f32,
    bottom: f32,
    left: f32,
    right: f32,
}

impl Span {
    fn of(r: &Rect, vertical: bool, after: bool) -> Span {
        let (x, w, y, h) = if vertical { (r.y, r.h, r.x, r.w) } else { (r.x, r.w, r.y, r.h) };
        let (left, right) = if after { (x, x + w) } else { (-(x + w), -x) };
        Span { top: y, bottom: y + h, left, right }
    }
}

/// Whether a direction is vertical (x and y swap) and looks after (right or down).
fn axis_of(dir: Dir) -> (bool, bool) {
    match dir {
        Dir::Left => (false, false),
        Dir::Right => (false, true),
        Dir::Up => (true, false),
        Dir::Down => (true, true),
    }
}

/// The candidate with the highest stamp; ties keep the first (`maxWithComparator:`).
pub fn most_recent(candidates: &[PaneId], activity: &dyn Fn(PaneId) -> u64) -> Option<PaneId> {
    candidates.iter().copied().fold(None, |best: Option<PaneId>, id| match best {
        Some(b) if activity(id) <= activity(b) => Some(b),
        _ => Some(id),
    })
}

/// The rectangles a tab shows: only the zoomed pane when one is zoomed and still here.
pub fn visible_rects(layout: &Layout, zoomed: Option<PaneId>, area: Rect, gap: f32) -> Vec<(PaneId, Rect)> {
    match zoomed.filter(|z| layout.panes().contains(z)) {
        Some(z) => vec![(z, area)],
        None => layout.rects(area, gap),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_pane_before_a_divider() {
        let t: Tree<PaneId> = Tree::Split {
            axis: Axis::Row,
            ratio: 0.3,
            a: Box::new(Tree::Leaf(1)),
            b: Box::new(Tree::Split { axis: Axis::Row, ratio: 0.5, a: Box::new(Tree::Leaf(2)), b: Box::new(Tree::Leaf(3)) }),
        };
        assert_eq!(pane_before(&t, &[]), Some(1), "A|B: A, not B");
        assert_eq!(pane_before(&t, &[true]), Some(2), "B|C: B");
        assert_eq!(pane_before(&Tree::Leaf(1), &[]), None);
    }

    #[test]
    fn swapping_panes_keeps_the_shape() {
        let mut l = Layout::new(1);
        l.split(1, Axis::Row, 2);
        l.split(2, Axis::Column, 3);
        let area = Rect { x: 0.0, y: 0.0, w: 100.0, h: 50.0 };
        let before: Vec<Rect> = l.rects(area, 0.0).into_iter().map(|(_, r)| r).collect();
        assert!(l.swap(1, 3));
        assert_eq!(l.panes(), [3, 2, 1]);
        assert_eq!(l.rects(area, 0.0).into_iter().map(|(_, r)| r).collect::<Vec<_>>(), before);
        assert!(!l.swap(1, 9));
    }

    #[test]
    fn grow_moves_the_divider_toward_the_neighbor() {
        let mut l = Layout::new(1);
        l.split(1, Axis::Row, 2);
        let area = Rect { x: 0.0, y: 0.0, w: 100.0, h: 50.0 };
        assert!(l.grow(2, Axis::Row, 0.1));
        let w2 = l.rect_of(2, area, 0.0).unwrap().w;
        assert!((w2 - 60.0).abs() < 0.01, "{w2}");
        assert!(l.grow(1, Axis::Row, -0.1));
        assert!((l.rect_of(1, area, 0.0).unwrap().w - 30.0).abs() < 0.01);
        assert!(!l.grow(2, Axis::Column, 0.1));
    }

    #[test]
    fn next_pane_wraps() {
        let mut l = Layout::new(1);
        l.split(1, Axis::Row, 2);
        assert_eq!(l.next(1, true), Some(2));
        assert_eq!(l.next(2, true), Some(1));
        assert_eq!(l.next(1, false), Some(2));
        assert_eq!(Layout::new(1).next(1, true), None);
    }

    use super::*;

    const AREA: Rect = Rect { x: 0.0, y: 0.0, w: 101.0, h: 51.0 };

    fn r(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect { x, y, w, h }
    }

    #[test]
    fn one_pane_fills_the_area() {
        assert_eq!(Layout::new(1).rects(AREA, 1.0), vec![(1, AREA)]);
    }

    #[test]
    fn split_right_and_down() {
        let mut l = Layout::new(1);
        assert!(l.split(1, Axis::Row, 2));
        assert_eq!(l.rects(AREA, 1.0), vec![(1, r(0.0, 0.0, 50.0, 51.0)), (2, r(51.0, 0.0, 50.0, 51.0))]);
        assert!(l.split(2, Axis::Column, 3));
        assert_eq!(l.rects(AREA, 1.0)[1..], [(2, r(51.0, 0.0, 50.0, 25.0)), (3, r(51.0, 26.0, 50.0, 25.0))]);
        assert!(!l.split(9, Axis::Row, 4));
    }

    #[test]
    fn remove_gives_the_space_back() {
        let mut l = Layout::new(1);
        l.split(1, Axis::Row, 2);
        l.split(2, Axis::Column, 3);
        assert!(l.remove(2));
        assert_eq!(l.panes(), vec![1, 3]);
        assert_eq!(l.rects(AREA, 1.0)[1], (3, r(51.0, 0.0, 50.0, 51.0)));
        assert!(l.remove(3));
        assert_eq!(l.rects(AREA, 1.0), vec![(1, AREA)]);
    }

    #[test]
    fn cannot_remove_the_last_pane() {
        let mut l = Layout::new(1);
        assert!(!l.remove(1));
        assert!(!l.remove(7));
    }

    #[test]
    fn divider_drag_sets_the_ratio() {
        let mut l = Layout::new(1);
        l.split(1, Axis::Row, 2);
        let d = &l.dividers(AREA, 1.0)[0];
        assert_eq!((d.path.clone(), d.axis, d.rect), (vec![], Axis::Row, r(50.0, 0.0, 1.0, 51.0)));
        l.set_ratio(&[], 0.25);
        assert_eq!(l.rects(AREA, 1.0)[0].1.w, 25.0);
        l.set_ratio(&[], 0.0);
        assert_eq!(l.rects(AREA, 1.0)[0].1.w, 10.0);
    }

    #[test]
    fn adjacency_follows_iterm2() {
        let big = Rect { x: 0.0, y: 0.0, w: 200.0, h: 100.0 };
        let mut l = Layout::new(1);
        l.split(1, Axis::Row, 2);
        l.split(2, Axis::Column, 3);
        assert_eq!(l.adjacent(1, Dir::Right, big, 1.0, 1.0), vec![2, 3]);
        assert_eq!(l.adjacent(2, Dir::Left, big, 1.0, 1.0), vec![1]);
        assert_eq!(l.adjacent(2, Dir::Down, big, 1.0, 1.0), vec![3]);
        assert_eq!(l.adjacent(3, Dir::Left, big, 1.0, 1.0), vec![1]);
        assert_eq!(l.adjacent(3, Dir::Up, big, 1.0, 1.0), vec![2]);
        assert_eq!(l.adjacent(1, Dir::Left, big, 1.0, 1.0), Vec::<PaneId>::new());
        // Three columns: 2 sits between 1 and 3.
        let mut c = Layout::new(1);
        c.split(1, Axis::Row, 2);
        c.split(2, Axis::Row, 3);
        assert_eq!(c.adjacent(1, Dir::Right, big, 1.0, 1.0), vec![2]);
        // The 1-point margin with fractional geometry (gap 1.5): the left column splits at 49, the
        // right one at 48.5 (a 0.5 overlap with 1, excluded) or 47.5 (a 1.5 overlap, included).
        let build = |ratio: f32| {
            let mut s = Layout::new(1);
            s.split(1, Axis::Row, 2);
            s.split(1, Axis::Column, 4);
            s.split(2, Axis::Column, 3);
            s.set_ratio(&[true], ratio);
            s
        };
        let (half, deep) = (build(0.48), build(0.47));
        assert_eq!(half.rect_of(3, big, 1.5).map(|r| r.y), Some(48.5));
        assert_eq!(deep.rect_of(3, big, 1.5).map(|r| r.y), Some(47.5));
        assert_eq!(half.adjacent(1, Dir::Right, big, 1.5, 1.0), vec![2], "a 0.5 overlap is under the margin");
        assert_eq!(deep.adjacent(1, Dir::Right, big, 1.5, 1.0), vec![2, 3], "a 1.5 overlap is in the projection");
        let mut t = Layout::new(1);
        t.split(1, Axis::Column, 2);
        t.split(1, Axis::Row, 3);
        // 3 is the top-left quarter, 1 its neighbor: 2 spans the bottom; 3 looking down sees 2 only.
        assert_eq!(t.adjacent(3, Dir::Down, big, 1.0, 1.0), vec![2]);
        // A margin wider than the overlap drops the neighbor (the 1-point rule).
        assert_eq!(l.adjacent(2, Dir::Left, big, 1.0, 60.0), Vec::<PaneId>::new());
    }

    #[test]
    fn wraparound_follows_iterm2() {
        let big = Rect { x: 0.0, y: 0.0, w: 200.0, h: 100.0 };
        let mut l = Layout::new(1);
        l.split(1, Axis::Row, 2);
        l.split(2, Axis::Column, 3);
        assert_eq!(l.wraparound(2, Dir::Right, big, 1.0, 1.0), vec![1]);
        assert_eq!(l.wraparound(3, Dir::Down, big, 1.0, 1.0), vec![2]);
        let mut c = Layout::new(1);
        c.split(1, Axis::Row, 2);
        c.split(2, Axis::Row, 3);
        assert_eq!(c.wraparound(1, Dir::Left, big, 1.0, 1.0), vec![3]);
        let one = Layout::new(1);
        for d in [Dir::Left, Dir::Right, Dir::Up, Dir::Down] {
            assert!(one.adjacent(1, d, big, 1.0, 1.0).is_empty());
            assert!(one.wraparound(1, d, big, 1.0, 1.0).is_empty());
        }
    }

    #[test]
    fn direction_pick_follows_iterm2() {
        let stamps = |id: PaneId| if id == 2 { 5 } else { 9 };
        assert_eq!(most_recent(&[2, 3], &stamps), Some(3));
        assert_eq!(most_recent(&[], &stamps), None);
        assert_eq!(most_recent(&[4, 5], &|_| 7), Some(4), "ties keep the first");
    }

    #[test]
    fn rect_of_finds_a_pane_by_id() {
        let mut l = Layout::new(1);
        l.split(1, Axis::Row, 2);
        assert_eq!(l.rect_of(2, AREA, 1.0), Some(r(51.0, 0.0, 50.0, 51.0)));
        assert_eq!(l.rect_of(9, AREA, 1.0), None);
    }

    #[test]
    fn even_resets_every_split() {
        let mut l = Layout::new(1);
        l.split(1, Axis::Row, 2);
        l.set_ratio(&[], 0.2);
        l.even();
        assert_eq!(l.rects(AREA, 1.0)[0].1.w, 50.0);
    }

    #[test]
    fn nudge_moves_the_divider_next_to_the_pane() {
        let mut l = Layout::new(1);
        l.split(1, Axis::Row, 2);
        assert!(l.nudge(1, Dir::Right, 0.1));
        assert_eq!(l.rects(AREA, 1.0)[0].1.w, 60.0);
        assert!(l.nudge(2, Dir::Left, 0.2));
        assert_eq!(l.rects(AREA, 1.0)[0].1.w, 40.0);
        assert!(!l.nudge(1, Dir::Up, 0.1), "no horizontal divider to move");
    }

    #[test]
    fn closing_the_zoomed_pane_unzooms() {
        let mut l = Layout::new(1);
        l.split(1, Axis::Row, 2);
        assert_eq!(visible_rects(&l, Some(2), AREA, 1.0), vec![(2, AREA)]);
        l.remove(2);
        // The zoomed pane is gone: the tab shows what is left.
        assert_eq!(visible_rects(&l, Some(2), AREA, 1.0), vec![(1, AREA)]);
    }

    #[test]
    fn rect_helpers() {
        assert!(r(0.0, 0.0, 10.0, 10.0).contains(9.9, 0.0));
        assert!(!r(0.0, 0.0, 10.0, 10.0).contains(10.0, 0.0));
        assert_eq!(r(0.0, 0.0, 10.0, 10.0).inset(2.0), r(2.0, 2.0, 6.0, 6.0));
    }

    #[test]
    fn retain_drops_leaves_and_their_splits() {
        let leaf = |p: PaneId| Box::new(Tree::Leaf(p));
        let t = Tree::Split { axis: Axis::Row, ratio: 0.3, a: leaf(1), b: Box::new(Tree::Split { axis: Axis::Column, ratio: 0.6, a: leaf(2), b: leaf(3) }) };
        assert_eq!(t.clone().retain(&mut |p| *p != 2), Some(Tree::Split { axis: Axis::Row, ratio: 0.3, a: leaf(1), b: leaf(3) }), "the sibling takes the space");
        assert_eq!(t.clone().retain(&mut |p| *p == 3), Some(Tree::Leaf(3)));
        assert_eq!(t.clone().retain(&mut |_| true), Some(t.clone()));
        assert_eq!(t.retain(&mut |_| false), None, "nothing kept");
    }

    #[test]
    fn bad_ratios_are_made_safe() {
        let t = Tree::Split { axis: Axis::Row, ratio: f32::NAN, a: Box::new(Tree::Leaf(1)), b: Box::new(Tree::Leaf(2)) };
        let Tree::Split { ratio, .. } = Layout::from_tree(t).tree() else { panic!() };
        assert_eq!(ratio, 0.5);
    }
}
