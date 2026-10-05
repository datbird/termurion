//! The pane order: reading order from pane rectangles, last-use order from
//! activity stamps, and stepping and numbering through either (iTerm2's
//! `PTYTab orderedSessions`, `sessionInDirection:` and the pane number
//! shortcut).

use crate::layout::{PaneId, Rect};
use std::cmp::Ordering;

/// Panes in iTerm2's reading order. A stacked root (or one pane) reads top to
/// bottom then left to right; a side-by-side root reads left to right then top
/// to bottom. Origins within `margin` of each other are equal (1 point, in pixels); ties keep input order.
pub fn reading_order(rects: &[(PaneId, Rect)], side_by_side_root: bool, margin: f32) -> Vec<PaneId> {
    let cmp = |a: &Rect, b: &Rect| -> Ordering {
        let (a1, a2, b1, b2) = if side_by_side_root {
            (a.x, a.y, b.x, b.y)
        } else {
            (a.y, a.x, b.y, b.x)
        };
        if (a1 - b1).abs() < margin {
            a2.partial_cmp(&b2).unwrap_or(Ordering::Equal)
        } else {
            a1.partial_cmp(&b1).unwrap_or(Ordering::Equal)
        }
    };
    // iTerm2 sorts with Foundation's stable merge sort (`sortedArrayUsingComparator:`). The
    // comparator is not transitive (within 1 chains), so this is a hand-written top-down
    // merge sort (split at n/2, the left element wins ties): it never panics, as the std
    // sorts may, and matches that shape. Foundation's exact algorithm was not confirmed
    // locally.
    fn merge_sort(items: &[(PaneId, Rect)], cmp: &dyn Fn(&Rect, &Rect) -> Ordering) -> Vec<(PaneId, Rect)> {
        if items.len() < 2 {
            return items.to_vec();
        }
        let (l, r) = items.split_at(items.len() / 2);
        let (l, r) = (merge_sort(l, cmp), merge_sort(r, cmp));
        let mut out = Vec::with_capacity(items.len());
        let (mut i, mut j) = (0, 0);
        while i < l.len() && j < r.len() {
            if cmp(&l[i].1, &r[j].1) == Ordering::Greater {
                out.push(r[j]);
                j += 1;
            } else {
                out.push(l[i]);
                i += 1;
            }
        }
        out.extend_from_slice(&l[i..]);
        out.extend_from_slice(&r[j..]);
        out
    }
    merge_sort(rects, &cmp).into_iter().map(|(id, _)| id).collect()
}

/// Panes by ascending activity stamp (last use); ties keep input order.
pub fn activity_order(panes: &[PaneId], activity: &dyn Fn(PaneId) -> u64) -> Vec<PaneId> {
    let mut out = panes.to_vec();
    out.sort_by_key(|&id| activity(id));
    out
}

/// The pane `offset` places from `from` in `order`, wrapping. None when `from`
/// is not in the order.
pub fn step(order: &[PaneId], from: PaneId, offset: isize) -> Option<PaneId> {
    let at = order.iter().position(|&id| id == from)? as isize;
    let n = order.len() as isize;
    Some(order[(at + offset).rem_euclid(n) as usize])
}

/// The pane for a number key: 9 is the last pane, 1 to 8 are by position, any
/// other digit or a position past the end is None.
pub fn by_number(order: &[PaneId], digit: u8) -> Option<PaneId> {
    match digit {
        9 => order.last().copied(),
        1..=8 => order.get(digit as usize - 1).copied(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect { x, y, w, h }
    }

    #[test]
    fn reading_order_follows_iterm2() {
        // Column[Row[Column[1, 2], 3], 4]: tree order is 1 2 3 4.
        let nested = [
            (1, r(0.0, 0.0, 100.0, 25.0)),
            (2, r(0.0, 25.0, 100.0, 25.0)),
            (3, r(100.0, 0.0, 100.0, 50.0)),
            (4, r(0.0, 50.0, 200.0, 50.0)),
        ];
        assert_eq!(reading_order(&nested, false, 1.0), vec![1, 3, 2, 4]);
        // Row[Column[1, 2], 3]: a side-by-side root reads columns first.
        let row = [
            (1, r(0.0, 0.0, 100.0, 50.0)),
            (2, r(0.0, 50.0, 100.0, 50.0)),
            (3, r(100.0, 0.0, 100.0, 100.0)),
        ];
        assert_eq!(reading_order(&row, true, 1.0), vec![1, 2, 3]);
        assert_eq!(reading_order(&row, false, 1.0), vec![1, 3, 2]);
        // Origins within 1 of each other are equal: x decides.
        let near = [(1, r(50.0, 0.0, 50.0, 10.0)), (2, r(0.0, 0.5, 50.0, 10.0))];
        assert_eq!(reading_order(&near, false, 1.0), vec![2, 1]);
        // iTerm2's margin is 1 point, so on a 2x display 1.5 pixels apart are still equal.
        let apart = [(1, r(50.0, 0.0, 50.0, 10.0)), (2, r(0.0, 1.5, 50.0, 10.0))];
        assert_eq!(reading_order(&apart, false, 1.0), vec![1, 2]);
        assert_eq!(reading_order(&apart, false, 2.0), vec![2, 1]);
        // Equal origins keep input order; one pane and none are fine.
        let same = [(7, r(0.0, 0.0, 9.0, 9.0)), (5, r(0.0, 0.0, 9.0, 9.0))];
        assert_eq!(reading_order(&same, true, 1.0), vec![7, 5]);
        assert_eq!(reading_order(&same[..1], false, 1.0), vec![7]);
        assert!(reading_order(&[], false, 1.0).is_empty());
    }

    #[test]
    fn reading_order_survives_an_intransitive_comparator() {
        // y within 1 chains: 0, 0.9, 1.8 are not transitively equal. Must not
        // panic and must keep every pane once.
        let rects: Vec<(PaneId, Rect)> = (0..40)
            .map(|i| (i as PaneId, r((40 - i) as f32, i as f32 * 0.9, 5.0, 5.0)))
            .collect();
        let mut got = reading_order(&rects, false, 1.0);
        got.sort();
        assert_eq!(got, (0..40).collect::<Vec<PaneId>>());
    }

    #[test]
    fn activity_order_follows_iterm2() {
        let stamp = |id: PaneId| match id {
            1 => 4,
            2 => 9,
            3 => 2,
            _ => 0,
        };
        assert_eq!(activity_order(&[1, 2, 3], &stamp), vec![3, 1, 2]);
        // Ties keep input order.
        assert_eq!(activity_order(&[8, 6, 7], &|_| 1), vec![8, 6, 7]);
        assert_eq!(step(&[3, 1, 2], 2, 1), Some(3));
        assert_eq!(step(&[3, 1, 2], 2, -1), Some(1));
        assert_eq!(step(&[3, 1, 2], 3, -1), Some(2));
        assert_eq!(step(&[5], 5, 1), Some(5));
        assert_eq!(step(&[3, 1, 2], 9, 1), None);
        assert_eq!(step(&[], 9, 1), None);
        assert_eq!(by_number(&[1, 2, 3], 9), Some(3));
        assert_eq!(by_number(&[1, 2, 3], 2), Some(2));
        assert_eq!(by_number(&[1, 2, 3], 4), None);
        assert_eq!(by_number(&[1, 2, 3], 0), None);
        assert_eq!(by_number(&[], 9), None);
        // Eight panes: 8 is the eighth, 9 the last, the same pane.
        let eight: Vec<PaneId> = (1..=8).collect();
        assert_eq!(by_number(&eight, 8), Some(8));
        assert_eq!(by_number(&eight, 9), Some(8));
        // Twelve panes: 9 is still the last, not the ninth.
        let twelve: Vec<PaneId> = (1..=12).collect();
        assert_eq!(by_number(&twelve, 9), Some(12));
        assert_eq!(by_number(&twelve, 5), Some(5));
    }

    #[test]
    fn by_number_follows_iterm2() {
        let eight: Vec<PaneId> = (1..=8).collect();
        assert_eq!(by_number(&eight, 8), Some(8));
        assert_eq!(by_number(&eight, 9), Some(8));
        assert_eq!(by_number(&eight, 1), Some(1));
        let ten: Vec<PaneId> = (1..=10).collect();
        assert_eq!(by_number(&ten, 9), Some(10));
        assert_eq!(by_number(&ten, 8), Some(8));
        let three: Vec<PaneId> = (1..=3).collect();
        assert_eq!(by_number(&three, 5), None);
        assert_eq!(by_number(&three, 9), Some(3));
    }

    #[test]
    fn reading_order_is_a_top_down_merge_sort() {
        // y 0, 0.9, 1.8 chain within 1 but A and C differ: the comparator is not transitive.
        // A top-down merge sort (split at n/2, left wins ties) gives A, C, B; an insertion
        // sort would give B, A, C.
        let rects = [(1, r(5.0, 0.0, 10.0, 10.0)), (2, r(3.0, 0.9, 10.0, 10.0)), (3, r(1.0, 1.8, 10.0, 10.0))];
        assert_eq!(reading_order(&rects, false, 1.0), vec![1, 3, 2]);
    }
}
