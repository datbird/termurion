//! Built rows kept between frames. A row is looked up by what it shows, so a row that did not
//! change is placed again instead of rebuilt.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::time::{Duration, Instant};

use alacritty_terminal::term::cell::Cell;

use crate::scene::{CellGlyph, Frame, ImageQuad, Quad, TextRow};

/// How long rows may be reused before the whole cache is rebuilt from scratch.
pub const ROW_CACHE_MAX_AGE: Duration = Duration::from_millis(500);
/// The cache keeps at least this many rows, however small the screen.
pub const ROW_CACHE_MIN_ENTRIES: usize = 256;

/// One built row, with every `y` measured from the row's top (`y = 0`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RowOut {
    pub quads: Vec<Quad>,
    pub rows: Vec<TextRow>,
    pub glyphs: Vec<CellGlyph>,
    pub underlines: Vec<Quad>,
    pub strikes: Vec<Quad>,
    pub images: Vec<ImageQuad>,
    /// Columns of the row's hovered cells.
    pub hovered: Vec<usize>,
}

impl RowOut {
    /// Adds the row to `frame` with its top at `top`, and records its hovered cells.
    pub fn place(&self, frame: &mut Frame, top: f32, row: usize, hovered: &mut Vec<(usize, usize)>) {
        let quad = |q: &Quad| {
            let mut q = *q;
            q.rect.y += top;
            q
        };
        frame.quads.extend(self.quads.iter().map(quad));
        frame.underlines.extend(self.underlines.iter().map(quad));
        frame.strikes.extend(self.strikes.iter().map(quad));
        frame.rows.extend(self.rows.iter().map(|r| {
            let mut r = r.clone();
            r.y += top;
            r.clip.y += top;
            r
        }));
        frame.glyphs.extend(self.glyphs.iter().map(|g| {
            let mut g = g.clone();
            g.rect.y += top;
            g.clip.y += top;
            g
        }));
        frame.images.extend(self.images.iter().map(|i| {
            let mut i = i.clone();
            i.rect.y += top;
            i
        }));
        hovered.extend(self.hovered.iter().map(|&col| (row, col)));
    }
}

/// What a built row depends on: the pane-wide inputs, the row's cells and its own highlights.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct RowKey {
    /// A hash of everything pane-wide that the row's look depends on.
    pub context: u64,
    pub cells: Vec<Cell>,
    /// A hash of the row's own highlight data (selection, matches, hover).
    pub extra: u64,
}

/// The index hash of a key's three parts. `RowKey` and the borrowed lookup both go through this,
/// so they always agree.
fn hash_parts(context: u64, cells: &[Cell], extra: u64) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    context.hash(&mut h);
    cells.hash(&mut h);
    extra.hash(&mut h);
    h.finish()
}

impl RowKey {
    fn hash64(&self) -> u64 {
        hash_parts(self.context, &self.cells, self.extra)
    }
}

pub struct RowCache {
    /// Entries by `hash64`; a bucket holds the rare rows whose hashes collide.
    rows: HashMap<u64, Vec<(RowKey, RowOut, u64)>>,
    count: usize,
    /// The frame being built; entries note the last frame that used them.
    frame: u64,
    /// Rows looked up or stored in the frame being built.
    used: usize,
    since: Instant,
}

impl Default for RowCache {
    fn default() -> Self {
        Self::new()
    }
}

impl RowCache {
    pub fn new() -> Self {
        RowCache { rows: HashMap::new(), count: 0, frame: 0, used: 0, since: Instant::now() }
    }

    pub fn get(&mut self, key: &RowKey) -> Option<&RowOut> {
        self.get_parts(key.context, &key.cells, key.extra)
    }

    /// Like `get`, but from the parts of a key, so a caller need not clone a row's cells.
    pub fn get_parts(&mut self, context: u64, cells: &[Cell], extra: u64) -> Option<&RowOut> {
        let frame = self.frame;
        let bucket = self.rows.get_mut(&hash_parts(context, cells, extra))?;
        let (_, out, last) = bucket
            .iter_mut()
            .find(|(k, _, _)| k.context == context && k.extra == extra && k.cells == cells)?;
        *last = frame;
        self.used += 1;
        Some(&*out)
    }

    pub fn put(&mut self, key: RowKey, out: RowOut) {
        let frame = self.frame;
        let bucket = self.rows.entry(key.hash64()).or_default();
        if let Some(e) = bucket.iter_mut().find(|(k, _, _)| *k == key) {
            // Same row stored again: replace it, but it is still one row on screen.
            if e.2 != frame {
                self.used += 1;
            }
            e.1 = out;
            e.2 = frame;
        } else {
            self.used += 1;
            self.count += 1;
            bucket.push((key, out, frame));
        }
    }

    pub fn clear(&mut self) {
        self.rows.clear();
        self.count = 0;
        self.used = 0;
        self.since = Instant::now();
    }

    /// True once the cache has gone `ROW_CACHE_MAX_AGE` without a clear.
    pub fn due(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.since) >= ROW_CACHE_MAX_AGE
    }

    pub fn len(&self) -> usize {
        self.count
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Ends a frame. Past four times the rows that were on screen (and the floor), the rows
    /// that frame did not use are dropped.
    pub fn end_frame(&mut self) {
        let cap = (4 * self.used).max(ROW_CACHE_MIN_ENTRIES);
        if self.count > cap {
            let frame = self.frame;
            for bucket in self.rows.values_mut() {
                bucket.retain(|e| e.2 == frame);
            }
            self.rows.retain(|_, b| !b.is_empty());
            self.count = self.rows.values().map(Vec::len).sum();
        }
        self.frame += 1;
        self.used = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::Rect;
    use crate::palette::rgb;
    use crate::scene::{CellGlyph, Frame, Quad, TextRow};
    use alacritty_terminal::term::cell::Cell;

    fn key() -> RowKey {
        let mut cells = vec![Cell::default(); 3];
        cells[1].c = 'a';
        RowKey { context: 7, cells, extra: 9 }
    }

    fn rect(y: f32) -> Rect {
        Rect { x: 1.0, y, w: 8.0, h: 16.0 }
    }

    fn out() -> RowOut {
        RowOut {
            quads: vec![Quad { rect: rect(0.0), color: [1.0; 4] }],
            rows: vec![TextRow { x: 1.0, y: 2.0, spans: vec![], clip: rect(0.0), scale: 1.0, face: 0 }],
            glyphs: vec![CellGlyph { c: '-', rect: rect(3.0), color: rgb(1, 2, 3), face: 0, clip: rect(0.0) }],
            hovered: vec![4],
            ..RowOut::default()
        }
    }

    #[test]
    fn a_row_is_found_by_its_key() {
        let mut cache = RowCache::new();
        cache.put(key(), out());
        assert_eq!(cache.get(&key()).unwrap().quads, out().quads);
        let mut k = key();
        k.context = 8;
        assert!(cache.get(&k).is_none());
        let mut k = key();
        k.cells[1].c = 'b';
        assert!(cache.get(&k).is_none());
        let mut k = key();
        k.extra = 10;
        assert!(cache.get(&k).is_none());
    }

    #[test]
    fn get_parts_matches_get() {
        let mut cache = RowCache::new();
        cache.put(key(), out());
        let k = key();
        assert_eq!(cache.get_parts(k.context, &k.cells, k.extra).unwrap().quads, out().quads);
        assert!(cache.get_parts(8, &k.cells, k.extra).is_none());
        let mut cells = k.cells.clone();
        cells[1].c = 'b';
        assert!(cache.get_parts(k.context, &cells, k.extra).is_none());
        assert!(cache.get_parts(k.context, &k.cells, 10).is_none());
        // A borrowed hit counts as a use, so it survives the trim.
        let mut cache = RowCache::new();
        for n in 0..300u64 {
            cache.put(RowKey { context: n, cells: vec![], extra: 0 }, RowOut::default());
        }
        cache.end_frame();
        assert!(cache.get_parts(5, &[], 0).is_some());
        cache.end_frame();
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn place_moves_the_row_to_its_top() {
        let mut frame = Frame::default();
        let mut hovered = Vec::new();
        out().place(&mut frame, 40.0, 5, &mut hovered);
        assert_eq!(frame.quads[0].rect.y, 40.0);
        assert_eq!(frame.rows[0].y, 42.0);
        assert_eq!(frame.rows[0].clip.y, 40.0);
        assert_eq!(frame.glyphs[0].rect.y, 43.0);
        assert_eq!(hovered, vec![(5, 4)]);
    }

    #[test]
    fn clear_and_age() {
        let mut cache = RowCache::new();
        let t0 = Instant::now();
        assert!(!cache.due(t0));
        assert!(cache.due(t0 + ROW_CACHE_MAX_AGE + Duration::from_millis(1)));
        cache.clear();
        assert!(!cache.due(Instant::now()));
    }

    #[test]
    fn entries_not_used_last_frame_are_dropped_past_the_cap() {
        let mut cache = RowCache::new();
        let key_n = |n: usize| RowKey { context: n as u64, cells: vec![], extra: 0 };
        for n in 0..1000 {
            cache.put(key_n(n), RowOut::default());
        }
        cache.end_frame();
        // 1000 rows were on screen, so all of them stay.
        assert_eq!(cache.len(), 1000);
        for n in 0..10 {
            assert!(cache.get(&key_n(n)).is_some());
        }
        cache.end_frame();
        // Ten rows on screen: the cap falls to the floor and the unused entries go.
        assert_eq!(cache.len(), 10);
    }
}
