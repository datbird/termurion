//! One per-key cooldown for every "the person closed this prompt, do not raise it again at
//! once" rule (the 13x6 permission prompt and the 13x7 sign-in prompt share it). Time is a
//! monotonic count of milliseconds (`mono_ms`, `ms_of`), never the wall clock, and the table is
//! bounded: when it is full the expired entries go, then the oldest.

use std::collections::HashMap;
use std::hash::Hash;
use std::time::Instant;

/// How long a closed prompt stays quiet.
pub const SPAN_MS: u64 = 60_000;
/// Most keys one table holds.
pub const KEPT: usize = 256;

fn start() -> Instant {
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    *START.get_or_init(Instant::now)
}

/// Milliseconds on the monotonic clock.
pub fn mono_ms() -> u64 {
    ms_of(Instant::now())
}

/// `t` as milliseconds on the same clock (an instant before the clock started reads 0).
pub fn ms_of(t: Instant) -> u64 {
    t.saturating_duration_since(start()).as_millis() as u64
}

pub struct Cooldown<K> {
    until: HashMap<K, u64>,
    span_ms: u64,
    kept: usize,
}

impl<K: Hash + Eq> Default for Cooldown<K> {
    fn default() -> Self {
        Self::new(SPAN_MS, KEPT)
    }
}

impl<K: Hash + Eq> Cooldown<K> {
    pub fn new(span_ms: u64, kept: usize) -> Self {
        Cooldown { until: HashMap::new(), span_ms, kept: kept.max(1) }
    }

    /// `key` is quiet from `now_ms` for the span.
    pub fn start(&mut self, key: K, now_ms: u64) {
        if !self.until.contains_key(&key) && self.until.len() >= self.kept {
            self.until.retain(|_, until| now_ms < *until);
            if self.until.len() >= self.kept {
                let drop_at = self.until.values().copied().min();
                if let Some(m) = drop_at {
                    let mut gone = false;
                    self.until.retain(|_, u| {
                        if !gone && *u == m {
                            gone = true;
                            false
                        } else {
                            true
                        }
                    });
                }
            }
        }
        self.until.insert(key, now_ms.saturating_add(self.span_ms));
    }

    /// Is `key` still quiet at `now_ms`?
    pub fn active(&self, key: &K, now_ms: u64) -> bool {
        self.until.get(key).is_some_and(|u| now_ms < *u)
    }

    pub fn clear(&mut self) {
        self.until.clear();
    }

    pub fn len(&self) -> usize {
        self.until.len()
    }

    pub fn is_empty(&self) -> bool {
        self.until.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_is_quiet_for_the_span_only() {
        let mut c = Cooldown::<&str>::default();
        assert!(!c.active(&"a", 0));
        c.start("a", 100);
        assert!(c.active(&"a", 100) && c.active(&"a", 100 + SPAN_MS - 1));
        assert!(!c.active(&"a", 100 + SPAN_MS));
        assert!(!c.active(&"b", 100));
    }

    #[test]
    fn the_table_stays_bounded_and_keeps_the_newest() {
        let mut c = Cooldown::<u64>::default();
        for n in 0..(KEPT as u64 * 3) {
            c.start(n, n);
            assert!(c.len() <= KEPT);
        }
        assert!(c.active(&(KEPT as u64 * 3 - 1), KEPT as u64 * 3));
        assert!(!c.active(&0, KEPT as u64 * 3), "the oldest was evicted");
        c.start(KEPT as u64 * 3 - 1, 5);
        assert!(c.len() <= KEPT, "restarting a key adds nothing");
    }

    #[test]
    fn expired_entries_go_before_live_ones() {
        let mut c = Cooldown::<u64>::new(10, 2);
        c.start(1, 0);
        c.start(2, 100);
        c.start(3, 105);
        assert!(c.active(&2, 105) && c.active(&3, 105) && !c.active(&1, 105));
        assert_eq!(c.len(), 2);
    }

    #[test]
    fn the_clock_is_monotonic_and_instants_map_onto_it() {
        let a = mono_ms();
        let t = Instant::now() + std::time::Duration::from_secs(5);
        assert!(ms_of(t) >= a + 4_900);
        assert!(mono_ms() >= a);
    }
}
