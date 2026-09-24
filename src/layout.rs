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

fn overlaps(a0: f32, alen: f32, b0: f32, blen: f32) -> bool {
    a0 < b0 + blen && b0 < a0 + alen
}

impl Layout {
    pub fn new(pane: PaneId) -> Self {
        Layout { root: Node::Leaf(pane) }
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

    /// The nearest pane in direction `dir` from `from` that overlaps it on the other axis.
    pub fn neighbor(&self, from: PaneId, dir: Dir, area: Rect) -> Option<PaneId> {
        let rects = self.rects(area, 0.0);
        let (_, f) = *rects.iter().find(|(id, _)| *id == from)?;
        let (cx, cy) = (f.x + f.w / 2.0, f.y + f.h / 2.0);
        let dist = |r: &Rect| {
            let (dx, dy) = (r.x + r.w / 2.0 - cx, r.y + r.h / 2.0 - cy);
            dx * dx + dy * dy
        };
        rects
            .iter()
            .filter(|(id, r)| {
                *id != from
                    && match dir {
                        Dir::Left => r.x + r.w <= f.x + 0.5 && overlaps(r.y, r.h, f.y, f.h),
                        Dir::Right => r.x >= f.x + f.w - 0.5 && overlaps(r.y, r.h, f.y, f.h),
                        Dir::Up => r.y + r.h <= f.y + 0.5 && overlaps(r.x, r.w, f.x, f.w),
                        Dir::Down => r.y >= f.y + f.h - 0.5 && overlaps(r.x, r.w, f.x, f.w),
                    }
            })
            .min_by(|(_, a), (_, b)| dist(a).total_cmp(&dist(b)))
            .map(|(id, _)| *id)
    }
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
    fn neighbors_by_direction() {
        let mut l = Layout::new(1);
        l.split(1, Axis::Row, 2);
        l.split(2, Axis::Column, 3);
        assert_eq!(l.neighbor(3, Dir::Left, AREA), Some(1));
        assert_eq!(l.neighbor(2, Dir::Down, AREA), Some(3));
        assert_eq!(l.neighbor(3, Dir::Up, AREA), Some(2));
        assert_eq!(l.neighbor(1, Dir::Left, AREA), None);
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
}
