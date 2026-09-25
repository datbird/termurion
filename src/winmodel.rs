//! Terminal windows: their numbers (iTerm2 gives each the lowest free number), the order
//! Next and Previous Window follow, titles, and the parked state of windows that are not
//! the current one.


use std::collections::BTreeMap;

/// Window numbers: each open window has the lowest number free when it opened.
#[derive(Clone, Debug, Default)]
pub struct Registry {
    numbers: BTreeMap<u64, usize>,
}

impl Registry {
    pub fn open(&mut self, id: u64) -> usize {
        let used: std::collections::BTreeSet<usize> = self.numbers.values().copied().collect();
        let n = (0..).find(|n| !used.contains(n)).unwrap_or(0);
        self.numbers.insert(id, n);
        n
    }

    pub fn close(&mut self, id: u64) {
        self.numbers.remove(&id);
    }

    pub fn number(&self, id: u64) -> Option<usize> {
        self.numbers.get(&id).copied()
    }

    pub fn by_number(&self, n: usize) -> Option<u64> {
        self.numbers.iter().find(|(_, v)| **v == n).map(|(k, _)| *k)
    }

    /// Windows by number.
    pub fn order(&self) -> Vec<u64> {
        let mut v: Vec<(usize, u64)> = self.numbers.iter().map(|(k, n)| (*n, *k)).collect();
        v.sort();
        v.into_iter().map(|(_, k)| k).collect()
    }

    fn step(&self, id: u64, by: isize) -> Option<u64> {
        let order = self.order();
        let i = order.iter().position(|k| *k == id)? as isize;
        let n = order.len() as isize;
        order.get(((i + by).rem_euclid(n)) as usize).copied()
    }

    /// Next Window: the next number, wrapping.
    pub fn next(&self, id: u64) -> Option<u64> {
        self.step(id, 1)
    }

    pub fn previous(&self, id: u64) -> Option<u64> {
        self.step(id, -1)
    }
}

/// What asked for a new session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpenRequest {
    NewTab,
    NewWindow,
    /// A profile picked from the Profiles menu, with Option (Alt) held or not.
    ProfileMenu { option: bool },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Tab,
    Window,
}

/// Where a new session opens: `prevent_tab` is the profile's "Always open in a new window,
/// never in a tab"; `profiles_in_new_window` is `openProfilesInNewWindow`.
pub fn open_target(request: OpenRequest, prevent_tab: bool, profiles_in_new_window: bool) -> Target {
    let wanted = match request {
        OpenRequest::NewWindow => Target::Window,
        OpenRequest::NewTab => Target::Tab,
        OpenRequest::ProfileMenu { option } => {
            if option != profiles_in_new_window { Target::Window } else { Target::Tab }
        }
    };
    if prevent_tab { Target::Window } else { wanted }
}

/// A window's title: "N. " first when Show window number is on (number shown from 1).
pub fn title(base: &str, number: usize, show_number: bool) -> String {
    if show_number { format!("{}. {base}", number + 1) } else { base.to_string() }
}

/// The Window menu's entry: the title and the switch shortcut (Alt+Win+1 to 9), or the
/// number from 10 on.
pub fn menu_title(base: &str, number: usize) -> String {
    if number < 9 { format!("{base} - Alt+Win+{}", number + 1) } else { format!("{base} - {}", number + 1) }
}

/// The state of every window but the current one. The current window's state lives in the
/// caller (`live`); `enter` swaps another window's state in.
#[derive(Debug)]
pub struct Parked<S> {
    current: Option<u64>,
    parked: BTreeMap<u64, S>,
}

impl<S> Default for Parked<S> {
    fn default() -> Self {
        Parked { current: None, parked: BTreeMap::new() }
    }
}

impl<S> Parked<S> {
    /// The first window: its state is already live.
    pub fn start(&mut self, id: u64) {
        self.current = Some(id);
    }

    pub fn current(&self) -> Option<u64> {
        self.current
    }

    /// A window that is not current.
    pub fn park(&mut self, id: u64, state: S) {
        self.parked.insert(id, state);
    }

    /// Makes `id` current, swapping its state into `live`. False for an unknown window.
    pub fn enter(&mut self, id: u64, live: &mut S) -> bool {
        if self.current == Some(id) {
            return true;
        }
        let Some(mut state) = self.parked.remove(&id) else { return false };
        std::mem::swap(live, &mut state);
        if let Some(old) = self.current {
            self.parked.insert(old, state);
        }
        self.current = Some(id);
        true
    }

    /// The current window closed: another becomes current (its state swapped into `live`),
    /// or None when it was the last.
    pub fn close_current(&mut self, live: &mut S) -> Option<u64> {
        let (id, mut state) = self.parked.pop_first()?;
        std::mem::swap(live, &mut state);
        self.current = Some(id);
        Some(id)
    }

    /// Takes a parked window out to make it current (after the current one closed).
    pub fn pop_parked(&mut self) -> Option<(u64, S)> {
        self.parked.pop_first()
    }

    /// A parked window closed.
    pub fn remove(&mut self, id: u64) -> Option<S> {
        self.parked.remove(&id)
    }

    /// Every window, sorted.
    pub fn ids(&self) -> Vec<u64> {
        let mut v: Vec<u64> = self.parked.keys().copied().chain(self.current).collect();
        v.sort();
        v
    }

    pub fn parked_mut(&mut self) -> impl Iterator<Item = (&u64, &mut S)> {
        self.parked.iter_mut()
    }

    pub fn parked(&self) -> impl Iterator<Item = (&u64, &S)> {
        self.parked.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_reuse_the_lowest_free() {
        let mut r = Registry::default();
        assert_eq!((r.open(10), r.open(11), r.open(12)), (0, 1, 2));
        r.close(10);
        assert_eq!(r.open(13), 0, "the lowest free number again");
        assert_eq!(r.number(12), Some(2));
        assert_eq!(r.by_number(0), Some(13));
        assert_eq!(r.order(), vec![13, 11, 12]);
    }

    #[test]
    fn next_and_previous_wrap() {
        let mut r = Registry::default();
        for id in [5, 6, 7] {
            r.open(id);
        }
        assert_eq!((r.next(5), r.next(7), r.previous(5)), (Some(6), Some(5), Some(7)));
        let mut one = Registry::default();
        one.open(1);
        assert_eq!(one.next(1), Some(1));
    }

    #[test]
    fn titles_carry_the_number() {
        assert_eq!(title("pwsh", 0, true), "1. pwsh");
        assert_eq!(title("pwsh", 2, false), "pwsh");
        assert_eq!(menu_title("pwsh", 0), "pwsh - Alt+Win+1");
        assert_eq!(menu_title("pwsh", 9), "pwsh - 10");
    }

    #[test]
    fn slots_swap_cleanly() {
        let mut p: Parked<Vec<u32>> = Parked::default();
        let mut live = vec![1];
        p.start(100);
        p.park(200, vec![2]);
        assert!(p.enter(200, &mut live));
        assert_eq!((live.clone(), p.current()), (vec![2], Some(200)));
        live.push(22);
        assert!(p.enter(100, &mut live));
        assert_eq!(live, vec![1], "window 100 got its own state back");
        assert!(p.enter(200, &mut live));
        assert_eq!(live, vec![2, 22], "and 200 kept its change");
        assert!(p.enter(200, &mut live), "entering the current window is a no-op");
        assert!(!p.enter(999, &mut live), "an unknown window is refused");
        assert_eq!(live, vec![2, 22]);
    }

    #[test]
    fn closing_one_window_keeps_the_others() {
        let mut p: Parked<Vec<u32>> = Parked::default();
        let mut live = vec![1];
        p.start(1);
        p.park(2, vec![2]);
        p.park(3, vec![3]);
        // Close the current window: another one becomes current.
        let next = p.close_current(&mut live);
        assert_eq!(next, Some(2));
        assert_eq!(live, vec![2]);
        assert_eq!(p.ids(), vec![2, 3]);
        assert_eq!(p.remove(3), Some(vec![3]), "closing a parked window takes its state out");
        assert_eq!(p.close_current(&mut live), None, "the last window closes");
    }

    #[test]
    fn open_target_follows_iterm2() {
        use OpenRequest::*;
        assert_eq!(open_target(NewWindow, false, false), Target::Window);
        assert_eq!(open_target(NewTab, false, false), Target::Tab);
        assert_eq!(open_target(NewTab, true, false), Target::Window, "Always open in a new window, never in a tab");
        assert_eq!(open_target(ProfileMenu { option: false }, false, false), Target::Tab);
        assert_eq!(open_target(ProfileMenu { option: true }, false, false), Target::Window, "Option for a new window");
        assert_eq!(open_target(ProfileMenu { option: false }, false, true), Target::Window, "openProfilesInNewWindow swaps them");
        assert_eq!(open_target(ProfileMenu { option: true }, false, true), Target::Tab);
        assert_eq!(open_target(ProfileMenu { option: true }, true, true), Target::Window, "the profile still refuses a tab");
    }
}
