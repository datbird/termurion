//! Termurion patch 10: how much a buffer kept between frames holds on to. Every kept buffer
//! (row writes, upload chunks, staging chunks, spent write buffers) follows one rule: keep what
//! recent frames needed, and give memory back only after the need stayed lower for a while.

/// Frames a lower need must last before the mark comes down to it (a few seconds at 60 to 120 Hz).
pub const DECAY_FRAMES: u32 = 300;

/// A decaying high-water mark of what a frame needed. It rises at once; after `DECAY_FRAMES`
/// frames that all needed less it comes down to the most of those. A steady frame of any size
/// keeps its storage, and a one-time burst is let go a few seconds later.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HighWater {
    mark: usize,
    /// The most a frame needed since the mark was last set.
    recent: usize,
    quiet: u32,
}

impl HighWater {
    /// Notes one frame's need and returns the mark.
    pub fn note(&mut self, need: usize) -> usize {
        if need >= self.mark {
            *self = HighWater { mark: need, recent: 0, quiet: 0 };
        } else {
            self.recent = self.recent.max(need);
            self.quiet += 1;
            if self.quiet >= DECAY_FRAMES {
                *self = HighWater { mark: self.recent, recent: 0, quiet: 0 };
            }
        }
        self.mark
    }

    pub fn mark(&self) -> usize {
        self.mark
    }

    /// Shrinks `v` to the mark once its room passes twice the mark. Growth by doubling stays
    /// under that, so a steady frame never reallocates here.
    pub fn trim<T>(&self, v: &mut Vec<T>) {
        if v.capacity() > self.mark.saturating_mul(2) {
            v.shrink_to(self.mark);
        }
    }

    /// Lets go of the spares in `items` the mark does not cover, largest first: one whose loss
    /// still leaves `owned` at or above the mark, and one bigger than twice the mark (and than
    /// `floor`, the smallest a pool makes), which no recent frame could have needed. `spare` is a
    /// spare's size, None for an item in use (always kept). Lowers `owned` by what goes and
    /// allocates nothing; with nothing to let go the order stays as it was.
    pub fn let_go<T>(&self, owned: &mut usize, items: &mut Vec<T>, spare: impl Fn(&T) -> Option<usize>, floor: usize) {
        let oversized = self.mark.saturating_mul(2).max(floor);
        let goes = |n: usize, owned: usize| n > oversized || owned.checked_sub(n).is_some_and(|rest| rest >= self.mark);
        if !items.iter().any(|t| spare(t).is_some_and(|n| goes(n, *owned))) {
            return;
        }
        items.sort_unstable_by_key(|t| std::cmp::Reverse(spare(t).unwrap_or(0)));
        items.retain(|t| match spare(t) {
            Some(n) if goes(n, *owned) => {
                *owned = owned.saturating_sub(n);
                false
            }
            _ => true,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mark_rises_at_once_and_decays_after_the_window() {
        let mut h = HighWater::default();
        assert_eq!(h.note(100), 100);
        assert_eq!(h.note(500), 500);
        for i in 1..DECAY_FRAMES {
            assert_eq!(h.note(if i == 7 { 80 } else { 40 }), 500, "frame {i}");
        }
        // The window ends: down to the most it saw, then to the steady need.
        assert_eq!(h.note(40), 80);
        for _ in 1..DECAY_FRAMES {
            h.note(40);
        }
        assert_eq!(h.note(40), 40);
        // A frame back at the mark starts the window again.
        for _ in 0..DECAY_FRAMES * 2 {
            h.note(40);
        }
        assert_eq!(h.mark(), 40);
    }

    #[test]
    fn trim_keeps_a_steady_vec_and_shrinks_after_decay() {
        let mut h = HighWater::default();
        let mut v: Vec<u8> = Vec::new();
        let mut caps = Vec::new();
        for _ in 0..10 {
            v.resize(12 << 20, 1);
            h.note(v.len());
            v.clear();
            h.trim(&mut v);
            caps.push((v.capacity(), v.as_ptr()));
        }
        assert!(caps.windows(2).all(|w| w[0] == w[1]), "a steady vec changed: {caps:?}");
        for _ in 0..DECAY_FRAMES {
            v.resize(1000, 1);
            h.note(v.len());
            v.clear();
            h.trim(&mut v);
        }
        assert!(v.capacity() <= 2000, "kept {} bytes", v.capacity());
    }

    #[test]
    fn let_go_drops_the_largest_spares_above_the_mark() {
        let mut h = HighWater::default();
        h.note(300);
        // In use: 100 (None); spares 200, 500, 200.
        let mut items = vec![Some(200), None, Some(500), Some(200)];
        let mut owned = 1000;
        h.let_go(&mut owned, &mut items, |t| *t, 0);
        items.sort();
        assert_eq!((items, owned), (vec![None, Some(200)], 300));
        let mut items = vec![Some(200)];
        let mut owned = 300;
        h.let_go(&mut owned, &mut items, |t| *t, 0);
        assert_eq!(items, [Some(200)]);
        // The only spare, too big for anything recent frames needed: it goes unless it is the
        // pool's smallest size.
        h = HighWater::default();
        h.note(10);
        let (mut items, mut owned) = (vec![Some(500)], 500);
        h.let_go(&mut owned, &mut items, |t| *t, 64);
        assert!(items.is_empty());
        let (mut items, mut owned) = (vec![Some(500)], 500);
        h.let_go(&mut owned, &mut items, |t| *t, 500);
        assert_eq!(items, [Some(500)]);
    }
}
