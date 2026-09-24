//! Tabs in most-recently-used order, for Cycle Tabs Forward and Backward. While the
//! modifiers of the cycling chord stay down, each press goes one tab deeper; letting
//! go makes the chosen tab the most recent (iTerm2's Ctrl+Tab).

#[derive(Default)]
pub struct Mru {
    /// Tab ids, most recent first.
    order: Vec<u64>,
    /// While cycling, the index of the tab shown now.
    cursor: Option<usize>,
}

impl Mru {
    /// A tab became active. Ignored while cycling, so walking the list does not reorder it.
    pub fn touch(&mut self, id: u64) {
        if self.cursor.is_some() {
            return;
        }
        self.order.retain(|t| *t != id);
        self.order.insert(0, id);
    }

    pub fn forget(&mut self, id: u64) {
        self.order.retain(|t| *t != id);
        self.cursor = None;
    }

    /// The next tab to show: one deeper (forward) or one back (backward), wrapping.
    pub fn step(&mut self, forward: bool) -> Option<u64> {
        let n = self.order.len();
        if n < 2 {
            return None;
        }
        let i = match (self.cursor, forward) {
            (None, true) => 1,
            (None, false) => n - 1,
            (Some(i), true) => (i + 1) % n,
            (Some(i), false) => (i + n - 1) % n,
        };
        self.cursor = Some(i);
        Some(self.order[i])
    }

    /// The cycling chord's modifiers came up: the tab shown is now the most recent.
    pub fn commit(&mut self) {
        if let Some(i) = self.cursor.take() {
            let id = self.order.remove(i);
            self.order.insert(0, id);
        }
    }

    pub fn cycling(&self) -> bool {
        self.cursor.is_some()
    }

    pub fn order(&self) -> &[u64] {
        &self.order
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cycling_walks_deeper_then_commits() {
        let mut m = Mru::default();
        for id in [1, 2, 3] {
            m.touch(id);
        } // 3 is most recent
        assert_eq!(m.step(true), Some(2));
        assert_eq!(m.step(true), Some(1));
        m.commit();
        assert_eq!(m.step(true), Some(3));
    }

    #[test]
    fn backward_starts_at_the_oldest_and_wraps() {
        let mut m = Mru::default();
        for id in [1, 2, 3] {
            m.touch(id);
        }
        assert_eq!(m.step(false), Some(1));
        assert_eq!(m.step(false), Some(2));
        assert_eq!(m.step(true), Some(1));
        m.commit();
        assert_eq!(m.order(), [1, 3, 2]);
    }

    #[test]
    fn a_closed_tab_is_forgotten() {
        let mut m = Mru::default();
        for id in [1, 2] {
            m.touch(id);
        }
        m.forget(1);
        assert_eq!(m.step(true), None);
    }
}
