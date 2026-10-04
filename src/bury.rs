//! Buried sessions (iTerm2's iTermBuriedSessions): a session that runs on outside every tab,
//! listed under Session > Buried Sessions, and put back where it was when it is restored.

/// A buried session and where it came from.
#[derive(Clone, Debug, PartialEq)]
pub struct Buried {
    pub pane: u64,
    /// The window's registry key and the tab's id when it was buried.
    pub window: u64,
    pub tab: u64,
    pub name: String,
}

#[derive(Clone, Debug, Default)]
pub struct Burial {
    list: Vec<Buried>,
}

/// Where a restored session goes (iTerm2's restoreSession).
#[derive(Clone, Debug, PartialEq)]
pub enum Restore {
    /// Back into its tab.
    InTab(u64, u64),
    /// A new tab of its window (the tab is gone).
    NewTab(u64),
    /// A new window (the window is gone).
    NewWindow,
}

impl Burial {
    pub fn bury(&mut self, b: Buried) {
        self.list.retain(|x| x.pane != b.pane);
        self.list.push(b);
    }

    /// Takes a session out of the list, for a restore.
    pub fn take(&mut self, pane: u64) -> Option<Buried> {
        let i = self.list.iter().position(|b| b.pane == pane)?;
        Some(self.list.remove(i))
    }

    /// Sessions buried from a window that was recreated now name its new key.
    pub fn rekey(&mut self, old: u64, new: u64) {
        for b in self.list.iter_mut().filter(|b| b.window == old) {
            b.window = new;
        }
    }

    /// The session buried last.
    pub fn newest(&self) -> Option<u64> {
        self.list.last().map(|b| b.pane)
    }

    pub fn contains(&self, pane: u64) -> bool {
        self.list.iter().any(|b| b.pane == pane)
    }

    /// The Buried Sessions menu's sessions: (pane, name), oldest first.
    pub fn items(&self) -> Vec<(u64, String)> {
        self.list.iter().map(|b| (b.pane, b.name.clone())).collect()
    }
}

pub fn restore_target(b: &Buried, window_open: bool, tab_open: bool) -> Restore {
    match (window_open, tab_open) {
        (true, true) => Restore::InTab(b.window, b.tab),
        (true, false) => Restore::NewTab(b.window),
        _ => Restore::NewWindow,
    }
}

/// A hidden tmux window in the Buried Sessions menu: iTerm2's "prefix client - name" (iTerm2
/// separates them with a long dash; Tandem's text uses a hyphen).
pub fn hidden_window_title(prefix: &str, client: &str, name: &str) -> String {
    let client = client.trim();
    let client = if client.is_empty() { "tmux" } else { client };
    format!("{prefix}{client} - {name}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(pane: u64) -> Buried {
        Buried { pane, window: 1, tab: 2, name: format!("s{pane}") }
    }

    #[test]
    fn buried_gateways_return_on_detach() {
        let mut list = Burial::default();
        list.bury(b(5));
        list.bury(b(6));
        list.bury(b(5));
        assert_eq!(list.items(), vec![(6, "s6".into()), (5, "s5".into())], "once each, newest last");
        assert!(list.contains(5));
        let gw = list.take(5).unwrap();
        assert!(!list.contains(5) && list.take(5).is_none());
        assert_eq!(restore_target(&gw, true, true), Restore::InTab(1, 2));
        assert_eq!(restore_target(&gw, true, false), Restore::NewTab(1));
        assert_eq!(restore_target(&gw, false, false), Restore::NewWindow);
        list.bury(b(7));
        assert_eq!(list.newest(), Some(7), "the last window closing brings back the newest");
        assert_eq!(hidden_window_title("\u{21a3} ", " ", "logs"), "\u{21a3} tmux - logs");
    }
}
