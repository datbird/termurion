#[cfg(not(feature = "std"))]
use alloc::{string::String, vec::Vec};
use core::ops::Range;

use crate::{AttrsOwned, HashMap, ShapeGlyph};

// Termurion patch 3: the caps. About 4.4 MB at most (entries about 300 bytes, glyphs 96).
/// Most runs the cache holds.
pub const MAX_ENTRIES: usize = 4096;
/// Most glyphs the cache holds across all its runs.
pub const MAX_GLYPHS: usize = 32768;
/// A longer run is shaped and never stored (long unique tokens, base64, URLs).
pub const MAX_RUN_BYTES: usize = 256;

/// Key for caching shape runs.
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub struct ShapeRunKey {
    pub text: String,
    pub default_attrs: AttrsOwned,
    pub attrs_spans: Vec<(Range<usize>, AttrsOwned)>,
    // Termurion patch 2: the same text shapes to other glyphs right to left (mirrored, reversed).
    pub rtl: bool,
}

/// A helper structure for caching shape runs.
#[derive(Clone)]
pub struct ShapeRunCache {
    age: u64,
    cache: HashMap<ShapeRunKey, (u64, Vec<ShapeGlyph>)>,
    // Termurion patch 3: glyphs held, the switch and the counters.
    glyphs: usize,
    enabled: bool,
    hits: u64,
    misses: u64,
}

impl Default for ShapeRunCache {
    fn default() -> Self {
        Self {
            age: 0,
            cache: HashMap::default(),
            glyphs: 0,
            enabled: true,
            hits: 0,
            misses: 0,
        }
    }
}

impl ShapeRunCache {
    /// Get cache item, updating age if found
    pub fn get(&mut self, key: &ShapeRunKey) -> Option<&Vec<ShapeGlyph>> {
        match self.cache.get_mut(key) {
            Some((age, glyphs)) => {
                self.hits += 1;
                *age = self.age;
                Some(&*glyphs)
            }
            None => {
                self.misses += 1;
                None
            }
        }
    }

    /// Insert cache item with current age
    pub fn insert(&mut self, key: ShapeRunKey, glyphs: Vec<ShapeGlyph>) {
        // Termurion patch 3: a full cache first drops what was not used since the last trim,
        // and stores nothing when that is not enough.
        if let Some((_, old)) = self.cache.remove(&key) {
            self.glyphs -= old.len();
        }
        let n = glyphs.len();
        if n > MAX_GLYPHS {
            return;
        }
        if self.cache.len() >= MAX_ENTRIES || self.glyphs + n > MAX_GLYPHS {
            let now = self.age;
            let mut held = 0;
            self.cache.retain(|_key, (age, glyphs)| {
                let keep = *age >= now;
                if keep {
                    held += glyphs.len();
                }
                keep
            });
            self.glyphs = held;
            if self.cache.len() >= MAX_ENTRIES || self.glyphs + n > MAX_GLYPHS {
                return;
            }
        }
        self.glyphs += n;
        self.cache.insert(key, (self.age, glyphs));
    }

    /// Remove anything in the cache with an age older than `keep_ages`
    pub fn trim(&mut self, keep_ages: u64) {
        let mut held = 0;
        self.cache.retain(|_key, (age, glyphs)| {
            let keep = *age + keep_ages >= self.age;
            if keep {
                held += glyphs.len();
            }
            keep
        });
        self.glyphs = held;
        // Increase age
        self.age += 1;
    }

    // Termurion patch 3: the rest of this impl.

    /// Drops every run and resets the counters.
    pub fn clear(&mut self) {
        self.cache.clear();
        self.glyphs = 0;
        self.hits = 0;
        self.misses = 0;
    }

    /// Runs held.
    pub fn len(&self) -> usize {
        self.cache.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cache.is_empty()
    }

    /// Glyphs held across all runs.
    pub fn glyph_count(&self) -> usize {
        self.glyphs
    }

    /// Hits and misses since creation or the last `clear`.
    pub fn stats(&self) -> (u64, u64) {
        (self.hits, self.misses)
    }

    /// Off: every run is shaped and nothing is stored (turning it off also clears).
    pub fn set_enabled(&mut self, on: bool) {
        self.enabled = on;
        if !on {
            self.clear();
        }
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub(crate) fn count_miss(&mut self) {
        self.misses += 1;
    }
}

impl core::fmt::Debug for ShapeRunCache {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_tuple("ShapeRunCache").finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Attrs, CacheKeyFlags};

    fn key(i: usize) -> ShapeRunKey {
        ShapeRunKey {
            text: format!("w{i}"),
            default_attrs: AttrsOwned::new(&Attrs::new()),
            attrs_spans: Vec::new(),
            rtl: false,
        }
    }

    fn glyphs(n: usize) -> Vec<ShapeGlyph> {
        (0..n)
            .map(|i| ShapeGlyph {
                start: i,
                end: i + 1,
                x_advance: 0.5,
                y_advance: 0.0,
                x_offset: 0.0,
                y_offset: 0.0,
                ascent: 0.8,
                descent: 0.2,
                font_monospace_em_width: None,
                font_id: fontdb::ID::dummy(),
                font_weight: fontdb::Weight::NORMAL,
                glyph_id: i as u16,
                color_opt: None,
                metadata: 0,
                cache_key_flags: CacheKeyFlags::empty(),
                metrics_opt: None,
            })
            .collect()
    }

    #[test]
    fn the_cache_never_passes_its_caps() {
        let mut c = ShapeRunCache::default();
        for i in 0..10000 {
            c.insert(key(i), glyphs(4));
            assert!(c.len() <= MAX_ENTRIES, "{} entries", c.len());
            assert!(c.glyph_count() <= MAX_GLYPHS, "{} glyphs", c.glyph_count());
            if i % 100 == 99 {
                c.trim(60);
            }
        }
        for i in 10000..10300 {
            c.insert(key(i), glyphs(200));
            assert!(c.len() <= MAX_ENTRIES);
            assert!(c.glyph_count() <= MAX_GLYPHS, "{} glyphs", c.glyph_count());
        }
        // The count matches what is held.
        let held: usize = c.cache.values().map(|(_, g)| g.len()).sum();
        assert_eq!(c.glyph_count(), held);
    }

    #[test]
    fn a_full_cache_drops_what_this_frame_did_not_use() {
        let mut c = ShapeRunCache::default();
        for i in 0..MAX_ENTRIES {
            c.insert(key(i), glyphs(4));
        }
        assert_eq!(c.len(), MAX_ENTRIES);
        c.trim(60);
        for i in 0..10 {
            assert!(c.get(&key(i)).is_some());
        }
        c.insert(key(MAX_ENTRIES), glyphs(4));
        assert_eq!(c.len(), 11);
        assert_eq!(c.glyph_count(), 44);
        // Everything now was used since the last trim: nothing goes, the new run is not stored.
        let mut i = MAX_ENTRIES + 1;
        while c.len() < MAX_ENTRIES {
            c.insert(key(i), glyphs(4));
            i += 1;
        }
        c.insert(key(i), glyphs(4));
        assert_eq!(c.len(), MAX_ENTRIES);
        assert!(c.get(&key(i)).is_none());
    }

    #[test]
    fn trim_drops_what_is_older_than_keep() {
        let mut c = ShapeRunCache::default();
        c.insert(key(0), glyphs(3));
        for _ in 0..3 {
            c.trim(2);
            assert_eq!(c.len(), 1);
            assert_eq!(c.glyph_count(), 3);
        }
        c.trim(2);
        assert_eq!(c.len(), 0);
        assert_eq!(c.glyph_count(), 0);
    }

    #[test]
    fn clear_empties_and_resets_the_counts() {
        let mut c = ShapeRunCache::default();
        c.insert(key(0), glyphs(3));
        assert!(c.get(&key(0)).is_some());
        assert!(c.get(&key(1)).is_none());
        c.count_miss();
        assert_eq!(c.stats(), (1, 2));
        c.clear();
        assert_eq!(c.len(), 0);
        assert!(c.is_empty());
        assert_eq!(c.glyph_count(), 0);
        assert_eq!(c.stats(), (0, 0));
    }
}
