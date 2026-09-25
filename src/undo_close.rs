//! Undo Close: sessions, tabs, and windows closed by the person wait here, their shells
//! still running, until Session Close Undo Timeout passes.


use std::time::{Duration, Instant};

/// What closed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Session,
    Tab,
    Window,
}

/// Where an undone close goes back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Where {
    SameTab,
    NewTab,
    SameWindow,
    NewWindow,
}

/// A session goes back into its tab (or a new tab in the window used now); a tab into its
/// window (or a new window); a window is made again.
pub fn target(kind: Kind, window_alive: bool, tab_alive: bool) -> Where {
    match kind {
        Kind::Session if window_alive && tab_alive => Where::SameTab,
        Kind::Session => Where::NewTab,
        Kind::Tab if window_alive => Where::SameWindow,
        Kind::Tab | Kind::Window => Where::NewWindow,
    }
}

/// The profile's Session Close Undo Timeout; None (0, negative, not a number) means a close
/// cannot be undone.
pub fn timeout(secs: f64) -> Option<Duration> {
    (secs.is_finite() && secs > 0.0).then(|| Duration::from_secs_f64(secs.min(86400.0)))
}

/// Closed things with their deadlines, newest last.
#[derive(Debug)]
pub struct Stack<T> {
    entries: Vec<(Instant, T)>,
}

impl<T> Default for Stack<T> {
    fn default() -> Self {
        Stack { entries: Vec::new() }
    }
}

impl<T> Stack<T> {
    pub fn push(&mut self, t: T, deadline: Instant) {
        self.entries.push((deadline, t));
    }

    /// Undo Close: the newest entry still before its deadline.
    pub fn pop(&mut self, now: Instant) -> Option<T> {
        let i = self.entries.iter().rposition(|(d, _)| *d > now)?;
        Some(self.entries.remove(i).1)
    }

    /// Entries whose deadline passed (their shells end).
    pub fn expire(&mut self, now: Instant) -> Vec<T> {
        let (gone, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut self.entries).into_iter().partition(|(d, _)| *d <= now);
        self.entries = kept;
        gone.into_iter().map(|(_, t)| t).collect()
    }

    /// Quit: everything ends.
    pub fn clear(&mut self) -> Vec<T> {
        std::mem::take(&mut self.entries).into_iter().map(|(_, t)| t).collect()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn next_deadline(&self) -> Option<Instant> {
        self.entries.iter().map(|(d, _)| *d).min()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn entries_expire() {
        let t0 = Instant::now();
        let mut s = Stack::default();
        s.push("a", t0 + Duration::from_secs(5));
        s.push("b", t0 + Duration::from_secs(1));
        assert_eq!(s.expire(t0 + Duration::from_secs(2)), vec!["b"]);
        assert_eq!(s.pop(t0 + Duration::from_secs(6)), None, "after its deadline nothing comes back");
        assert_eq!(s.expire(t0 + Duration::from_secs(6)), vec!["a"], "and it ends");
        assert!(s.is_empty());
    }

    #[test]
    fn undo_pops_the_newest() {
        let t0 = Instant::now();
        let mut s = Stack::default();
        s.push(1, t0 + Duration::from_secs(5));
        s.push(2, t0 + Duration::from_secs(5));
        assert_eq!(s.pop(t0), Some(2));
        assert_eq!(s.pop(t0), Some(1));
        assert_eq!(s.pop(t0), None);
        assert_eq!(s.next_deadline(), None);
    }

    #[test]
    fn undo_targets() {
        assert_eq!(target(Kind::Session, true, true), Where::SameTab);
        assert_eq!(target(Kind::Session, true, false), Where::NewTab, "its tab is gone: a new tab");
        assert_eq!(target(Kind::Session, false, false), Where::NewTab, "in the window used now");
        assert_eq!(target(Kind::Tab, true, false), Where::SameWindow);
        assert_eq!(target(Kind::Tab, false, false), Where::NewWindow, "its window is gone");
        assert_eq!(target(Kind::Window, true, true), Where::NewWindow);
        assert_eq!(timeout(5.0), Some(Duration::from_secs(5)));
        assert_eq!(timeout(0.0), None, "0 turns Undo Close off");
        assert_eq!(timeout(f64::NAN), None);
    }

    #[test]
    fn quit_drops_the_stack() {
        let t0 = Instant::now();
        let mut s = Stack::default();
        s.push("x", t0 + Duration::from_secs(60));
        assert_eq!(s.clear(), vec!["x"], "quitting ends every waiting shell");
        assert!(s.is_empty());
    }
}
