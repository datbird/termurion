//! Built rows kept between frames. A row is looked up by what it shows, so a row that did not
//! change is placed again instead of rebuilt.

use rustc_hash::FxHashMap as HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::{Duration, Instant};

use alacritty_terminal::term::cell::Cell;

use crate::layout::Rect;
use crate::scene::{CellGlyph, Frame, ImageQuad, Quad, TextRow};
use crate::screen::RowDamage;

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
    /// Adds the row to `frame` with its top at `top`, and records its hovered cells. Text and
    /// box glyphs are clipped to `clip`, the pane's rect, wherever the row sits.
    pub fn place(&self, frame: &mut Frame, top: f32, clip: Rect, row: usize, hovered: &mut Vec<(usize, usize)>) {
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
            r.clip = clip;
            r
        }));
        frame.glyphs.extend(self.glyphs.iter().map(|g| {
            let mut g = g.clone();
            g.rect.y += top;
            g.clip = clip;
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
    let mut h = rustc_hash::FxHasher::default();
    context.hash(&mut h);
    hash_cells(cells, &mut h);
    extra.hash(&mut h);
    h.finish()
}

/// A color folded into 32 bits. Collisions only cost a compare: a hit always compares the cells.
fn color_bits(c: alacritty_terminal::vte::ansi::Color) -> u32 {
    use alacritty_terminal::vte::ansi::Color;
    match c {
        Color::Named(n) => 0x0300_0000 | n as u32,
        Color::Spec(rgb) => 0x0100_0000 | (rgb.r as u32) << 16 | (rgb.g as u32) << 8 | rgb.b as u32,
        Color::Indexed(i) => 0x0200_0000 | i as u32,
    }
}

/// Hashes a row's cells in two writes each, not the derived field by field walk. This runs on
/// every row that alacritty touched, so it is the hot part of finding a row.
fn hash_cells(cells: &[Cell], h: &mut rustc_hash::FxHasher) {
    cells.len().hash(h);
    for c in cells {
        h.write_u64(c.c as u64 | (c.flags.bits() as u64) << 32);
        h.write_u64(color_bits(c.fg) as u64 | (color_bits(c.bg) as u64) << 32);
        if let Some(extra) = &c.extra {
            extra.hash(h);
        }
    }
}

impl RowKey {
    fn hash64(&self) -> u64 {
        hash_parts(self.context, &self.cells, self.extra)
    }
}

pub struct RowCache {
    /// Entries by `hash64`; a bucket holds the rare rows whose hashes collide.
    rows: HashMap<u64, Vec<(RowKey, Arc<RowOut>, u64)>>,
    count: usize,
    /// The frame being built; entries note the last frame that used them.
    frame: u64,
    /// Rows looked up or stored in the frame being built.
    used: usize,
    since: Instant,
    /// Lookups that found their row, since the cache was made.
    hits: u64,
    /// Rows that hashed and compared their cells, since the cache was made.
    compares: u64,
    /// Damage since the last frame drawn through the cache; None knows nothing (all rows).
    damage: Option<RowDamage>,
    /// Last frame's `(context, extra, entry)` at each viewport row.
    last: Vec<Option<(u64, u64, Arc<RowOut>)>>,
    /// The screen `last` was drawn from.
    screen: u64,
}

impl Default for RowCache {
    fn default() -> Self {
        Self::new()
    }
}

impl RowCache {
    pub fn new() -> Self {
        RowCache { rows: HashMap::default(), count: 0, frame: 0, used: 0, since: Instant::now(), hits: 0, compares: 0, damage: None, last: Vec::new(), screen: 0 }
    }

    pub fn get(&mut self, key: &RowKey) -> Option<&Arc<RowOut>> {
        self.get_parts(key.context, &key.cells, key.extra)
    }

    /// Like `get`, but from the parts of a key, so a caller need not clone a row's cells.
    pub fn get_parts(&mut self, context: u64, cells: &[Cell], extra: u64) -> Option<&Arc<RowOut>> {
        let frame = self.frame;
        self.compares += 1;
        let bucket = self.rows.get_mut(&hash_parts(context, cells, extra))?;
        let (_, out, last) = bucket
            .iter_mut()
            .find(|(k, _, _)| k.context == context && k.extra == extra && k.cells == cells)?;
        *last = frame;
        self.used += 1;
        self.hits += 1;
        Some(&*out)
    }

    /// Rows that hashed and compared their cells since the cache was made.
    pub fn compares(&self) -> u64 {
        self.compares
    }

    /// Notes what alacritty touched since the last set; the next frame drawn through the cache
    /// uses all of it.
    pub fn set_damage(&mut self, d: RowDamage) {
        self.damage = Some(match self.damage.take() {
            Some(old) => old.merge(d),
            None => d,
        });
    }

    /// The frame is drawn from screen `id`: another screen's rows are not kept.
    pub fn follow(&mut self, id: u64) {
        if self.screen != id {
            self.screen = id;
            self.last.clear();
        }
    }

    /// Takes the damage for a frame: Full when none was set. The frame's rows start over.
    pub(crate) fn begin_rows(&mut self, lines: usize) -> Vec<bool> {
        let clean = match self.damage.take() {
            Some(RowDamage::Rows(d)) if d.len() == lines && self.last.len() == lines => d.into_iter().map(|x| !x).collect(),
            _ => vec![false; lines],
        };
        self.last.resize(lines, None);
        clean
    }

    /// Last frame's entry at `row`, when the row is clean and was built with this `context` and `extra`.
    pub(crate) fn kept(&mut self, row: usize, context: u64, extra: u64) -> Option<Arc<RowOut>> {
        let (c, e, out) = self.last.get(row)?.as_ref()?;
        if (*c, *e) != (context, extra) {
            return None;
        }
        self.used += 1;
        self.hits += 1;
        Some(out.clone())
    }

    /// Notes what `row` shows this frame (None for a row that is never cached).
    pub(crate) fn note_row(&mut self, row: usize, shown: Option<(u64, u64, Arc<RowOut>)>) {
        if let Some(slot) = self.last.get_mut(row) {
            *slot = shown;
        }
    }

    /// How many lookups found their row since the cache was made.
    pub fn hits(&self) -> u64 {
        self.hits
    }

    pub fn put(&mut self, key: RowKey, out: RowOut) -> Arc<RowOut> {
        let out = Arc::new(out);
        let frame = self.frame;
        let bucket = self.rows.entry(key.hash64()).or_default();
        if let Some(e) = bucket.iter_mut().find(|(k, _, _)| *k == key) {
            // Same row stored again: replace it, but it is still one row on screen.
            if e.2 != frame {
                self.used += 1;
            }
            e.1 = out.clone();
            e.2 = frame;
        } else {
            self.used += 1;
            self.count += 1;
            bucket.push((key, out.clone(), frame));
        }
        out
    }

    pub fn clear(&mut self) {
        self.rows.clear();
        self.count = 0;
        self.used = 0;
        self.last.clear();
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
            rows: vec![TextRow { x: 1.0, y: 2.0, spans: Default::default(), clip: rect(0.0), scale: 1.0, face: 0 }],
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
    fn placing_a_row_shares_its_text() {
        let mut row = out();
        row.rows[0].spans = vec![crate::scene::Span { text: "hi".into(), color: rgb(1, 2, 3), bold: false, italic: false, alt_font: false, icon: false, prose: false, badge: false, toolbelt: false, fake_bold: false, ligatures: 0, mask: 0, font: 0 }].into();
        let mut frame = Frame::default();
        row.place(&mut frame, 0.0, rect(0.0), 0, &mut Vec::new());
        assert!(crate::scene::Spans::ptr_eq(&frame.rows[0].spans, &row.rows[0].spans));
    }

    #[test]
    fn place_moves_the_row_to_its_top() {
        let mut frame = Frame::default();
        let mut hovered = Vec::new();
        let pane = Rect { x: 0.0, y: 6.0, w: 100.0, h: 200.0 };
        out().place(&mut frame, 40.0, pane, 5, &mut hovered);
        assert_eq!(frame.quads[0].rect.y, 40.0);
        assert_eq!(frame.rows[0].y, 42.0);
        // The clip is the pane's rect, not the row's.
        assert_eq!(frame.rows[0].clip, pane);
        assert_eq!(frame.glyphs[0].rect.y, 43.0);
        assert_eq!(frame.glyphs[0].clip, pane);
        assert_eq!(hovered, vec![(5, 4)]);
    }

    #[test]
    fn hits_count_found_lookups_only() {
        let mut cache = RowCache::new();
        cache.put(key(), out());
        assert_eq!(cache.hits(), 0);
        assert!(cache.get(&key()).is_some());
        let k = key();
        assert!(cache.get_parts(k.context, &k.cells, k.extra).is_some());
        assert!(cache.get_parts(1, &k.cells, k.extra).is_none());
        assert_eq!(cache.hits(), 2);
        cache.clear();
        assert_eq!(cache.hits(), 2);
    }

    /// The setting turned off clears the cache every frame, so turning it on again starts empty
    /// and nothing from before is served.
    #[test]
    fn a_cleared_cache_serves_nothing_and_fills_again() {
        let mut cache = RowCache::new();
        cache.put(key(), out());
        assert!(cache.get(&key()).is_some());
        cache.clear();
        cache.end_frame();
        assert_eq!(cache.len(), 0);
        let hits = cache.hits();
        assert!(cache.get(&key()).is_none(), "nothing survives the clear");
        assert_eq!(cache.hits(), hits);
        cache.put(key(), out());
        assert!(cache.get(&key()).is_some(), "it fills again after");
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

    /// Each part of a cell moves the row's hash (not only its compare), so rows that differ in
    /// one attribute land in different buckets.
    #[test]
    fn every_part_of_a_cell_changes_the_row_hash() {
        use alacritty_terminal::term::cell::Flags;
        use alacritty_terminal::vte::ansi::{Color, NamedColor};
        let base = key().cells;
        let h = |cells: &[Cell]| hash_parts(7, cells, 9);
        let mut seen = vec![h(&base)];
        let mut vary = |f: &dyn Fn(&mut Cell)| {
            let mut cells = base.clone();
            f(&mut cells[1]);
            let v = h(&cells);
            assert!(!seen.contains(&v), "a changed cell hashed like another row");
            seen.push(v);
            let mut cache = RowCache::new();
            cache.put(RowKey { context: 7, cells: base.clone(), extra: 9 }, out());
            assert!(cache.get_parts(7, &cells, 9).is_none());
        };
        vary(&|c| c.c = 'b');
        vary(&|c| c.fg = Color::Spec(rgb(1, 2, 3)));
        vary(&|c| c.fg = Color::Indexed(3));
        vary(&|c| c.fg = Color::Named(NamedColor::Red));
        vary(&|c| c.bg = Color::Spec(rgb(1, 2, 3)));
        vary(&|c| c.bg = Color::Indexed(3));
        vary(&|c| c.flags = Flags::BOLD);
        vary(&|c| c.flags = Flags::UNDERLINE);
        vary(&|c| c.push_zerowidth('\u{301}'));
        vary(&|c| c.set_underline_color(Some(Color::Indexed(9))));
        // A cell more or fewer is a different row.
        let mut longer = base.clone();
        longer.push(Cell::default());
        assert_ne!(h(&base), h(&longer));
        assert_ne!(h(&base), h(&base[..2]));
    }
}
