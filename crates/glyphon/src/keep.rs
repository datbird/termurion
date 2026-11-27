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

    /// Shrinks `v` to the mark once its room passes one and a half times the mark. A steady
    /// frame reallocates here at most once (a doubled vector comes down to the mark, which then
    /// holds every frame of the window).
    pub fn trim<T>(&self, v: &mut Vec<T>) {
        if v.capacity() > self.mark.saturating_add(self.mark / 2) {
            v.shrink_to(self.mark);
        }
    }
}

/// Size classes of pooled chunks: a chunk of `n` bytes is in class `ceil(log2(n))`.
pub const CLASSES: usize = usize::BITS as usize + 1;

/// The size class of a chunk of `n` bytes.
pub fn class_of(n: usize) -> usize {
    (usize::BITS - n.saturating_sub(1).leading_zeros()) as usize
}

/// Chunks per size class.
pub type Counts = [u32; CLASSES];

/// What a pool of reused chunks keeps: per size class, as many chunks as recent frames had in
/// use at once (a `HighWater` each). Counting per class keeps the shape a load needs, so a frame
/// that holds one big chunk and the next that holds many small ones both find theirs.
#[derive(Clone, Debug)]
pub struct PoolMarks {
    marks: [HighWater; CLASSES],
    /// This frame's most chunks in use at once, per class.
    peak: Counts,
}

impl Default for PoolMarks {
    fn default() -> Self {
        PoolMarks { marks: [HighWater::default(); CLASSES], peak: [0; CLASSES] }
    }
}

impl PoolMarks {
    /// Notes the chunks in use now (taken when the pool hands one out) toward this frame's peak.
    pub fn in_use(&mut self, now: &Counts) {
        for (p, n) in self.peak.iter_mut().zip(now) {
            *p = (*p).max(*n);
        }
    }

    /// Ends a frame: notes its peak and lets go of the spares in `items` past each class's mark,
    /// smallest first. `owned` counts every chunk of the pool per class (spares and those in use,
    /// in `items` or not) and is lowered by what goes. `spare` is an item's size, None for one in
    /// use. Allocates nothing; with nothing to let go the order stays as it was.
    pub fn end_frame<T>(&mut self, owned: &mut Counts, items: &mut Vec<T>, spare: impl Fn(&T) -> Option<usize>) {
        let mut keep = [0u32; CLASSES];
        for c in 0..CLASSES {
            keep[c] = self.marks[c].note(std::mem::take(&mut self.peak[c]) as usize) as u32;
        }
        if !items.iter().any(|t| spare(t).is_some_and(|n| owned[class_of(n)] > keep[class_of(n)])) {
            return;
        }
        items.sort_unstable_by_key(|t| spare(t).unwrap_or(usize::MAX));
        items.retain(|t| match spare(t) {
            Some(n) if owned[class_of(n)] > keep[class_of(n)] => {
                owned[class_of(n)] -= 1;
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
    fn classes_are_powers_of_two() {
        assert_eq!([class_of(0), class_of(1), class_of(2), class_of(3), class_of(4), class_of(5)], [0, 0, 1, 2, 2, 3]);
        assert_eq!((class_of(256 << 10), class_of((256 << 10) + 1)), (18, 19));
    }

    fn end(p: &mut PoolMarks, owned: &mut Counts, items: &[Option<usize>]) -> Vec<Option<usize>> {
        let mut items = items.to_vec();
        p.end_frame(owned, &mut items, |t| *t);
        items.sort();
        items
    }

    #[test]
    fn a_pool_keeps_each_class_recent_frames_held() {
        let mut p = PoolMarks::default();
        let (big, small) = (class_of(4 << 20), class_of(256 << 10));
        // One frame held two big chunks at once, another twenty small ones.
        let mut now = [0; CLASSES];
        now[big] = 2;
        p.in_use(&now);
        let mut owned = [0; CLASSES];
        owned[big] = 2;
        assert_eq!(end(&mut p, &mut owned, &[Some(4 << 20); 2]).len(), 2);
        let mut now = [0; CLASSES];
        now[small] = 20;
        p.in_use(&now);
        owned[small] = 20;
        let all: Vec<Option<usize>> = [Some(4 << 20); 2].into_iter().chain([Some(256 << 10); 20]).collect();
        assert_eq!(end(&mut p, &mut owned, &all).len(), 22, "a chunk either frame needs was let go");
        // A third small spare nobody held, and a third big one: they go, the smallest first.
        let mut owned3 = owned;
        owned3[big] = 3;
        owned3[small] = 21;
        let mut more = all.clone();
        more.extend([Some(4 << 20), Some(256 << 10)]);
        assert_eq!(end(&mut p, &mut owned3, &more).len(), 22);
        assert_eq!((owned3[big], owned3[small]), (2, 20));
    }

    #[test]
    fn a_pool_lets_a_burst_go_after_the_window() {
        let mut p = PoolMarks::default();
        let big = class_of(16 << 20);
        let mut now = [0; CLASSES];
        now[big] = 1;
        p.in_use(&now);
        let mut owned = [0; CLASSES];
        owned[big] = 1;
        assert_eq!(end(&mut p, &mut owned, &[Some(16 << 20)]).len(), 1);
        for _ in 1..DECAY_FRAMES {
            assert_eq!(end(&mut p, &mut owned, &[Some(16 << 20)]).len(), 1);
        }
        assert!(end(&mut p, &mut owned, &[Some(16 << 20)]).is_empty(), "the burst's chunk stayed");
        assert_eq!(owned[big], 0);
    }
}
