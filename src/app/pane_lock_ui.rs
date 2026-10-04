//! Lock Pane (`PTYSession locked`): the pane that refuses moves and asks before closing,
//! its context menu items and its title bar lock mark.

use super::*;
use crate::close_rules::LOCKED_REASON;

/// One of the context menu's lock items (`CMH:671-713`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LockItem {
    /// Lock Pane, with whether the pane is locked (a check item).
    Pane(bool),
    All,
    UnlockAll,
}

/// The lock items after Toggle Broadcasting Input: Lock Pane, then for a tab with several
/// panes Unlock All Panes in Tab when all are locked, else Lock All Panes in Tab and, when
/// any is locked, Unlock All Panes in Tab too (iTerm2 hides that one behind Option).
pub(super) fn lock_items(tab_panes: usize, locked: bool, all_locked: bool, any_locked: bool) -> Vec<(&'static str, LockItem)> {
    let mut out = vec![("Lock Pane", LockItem::Pane(locked))];
    if tab_panes > 1 {
        if !all_locked {
            out.push(("Lock All Panes in Tab", LockItem::All));
        }
        if any_locked {
            out.push(("Unlock All Panes in Tab", LockItem::UnlockAll));
        }
    }
    out
}

/// A pane move, swap or drag is refused under Lock Layout or a locked pane
/// (`MovePaneController.m:69-93`, `:128-135`, `:313-322`).
pub(super) fn move_refused(layout_locked: bool, pane_locked: bool) -> bool {
    layout_locked || pane_locked
}

/// Why closing a pane asks because of its lock: only a live locked pane has one
/// (`PS:3068-3078`).
pub(super) fn close_reason_for(exited: bool, locked: bool) -> Option<&'static str> {
    (!exited && locked).then_some(LOCKED_REASON)
}

impl App {
    /// A pane that was just created takes "Default Pane Locked" from its profile
    /// (`PS:6211-6213`, once, when its profile first loads).
    pub(super) fn apply_new_pane_defaults(&mut self, id: PaneId) {
        let locked = self.panes.get(&id).and_then(|p| self.store.get(&p.profile)).is_some_and(|pr| pr.bool("Default Pane Locked"));
        if let Some(p) = self.panes.get_mut(&id) {
            p.locked = locked;
        }
    }

    /// Whether `pane` is locked (a locked pane refuses every move to another tab or window).
    pub(super) fn pane_locked(&self, pane: PaneId) -> bool {
        self.panes.get(&pane).is_some_and(|p| p.locked)
    }

    /// The pane's lock mark and Lock Pane item flip (`toggleLocked`).
    pub(super) fn toggle_pane_lock(&mut self, pane: PaneId) {
        if let Some(p) = self.panes.get_mut(&pane) {
            p.locked = !p.locked;
            self.redraw();
        }
    }

    /// Lock All or Unlock All Panes in the tab of `pane` (`TAB:5589-5617`).
    pub(super) fn lock_all_in_tab(&mut self, pane: PaneId, lock: bool) {
        let Some(ids) = self.tabs.iter().find(|t| t.layout.panes().contains(&pane)).map(|t| t.layout.panes()) else { return };
        for id in ids {
            if let Some(p) = self.panes.get_mut(&id) {
                p.locked = lock;
            }
        }
        self.redraw();
    }

    /// The lock items for the menu of `pane`: its lock state and its tab's.
    pub(super) fn lock_items_for(&self, pane: PaneId) -> Vec<(&'static str, LockItem)> {
        let ids = self.tabs.iter().find(|t| t.layout.panes().contains(&pane)).map(|t| t.layout.panes()).unwrap_or_else(|| vec![pane]);
        let locks: Vec<bool> = ids.iter().map(|id| self.panes.get(id).is_some_and(|p| p.locked)).collect();
        lock_items(ids.len(), self.panes.get(&pane).is_some_and(|p| p.locked), locks.iter().all(|l| *l), locks.iter().any(|l| *l))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lock_items_follow_iterm2() {
        assert_eq!(lock_items(1, false, false, false), vec![("Lock Pane", LockItem::Pane(false))]);
        assert_eq!(lock_items(3, false, false, false), vec![("Lock Pane", LockItem::Pane(false)), ("Lock All Panes in Tab", LockItem::All)]);
        assert_eq!(
            lock_items(3, true, false, true),
            vec![("Lock Pane", LockItem::Pane(true)), ("Lock All Panes in Tab", LockItem::All), ("Unlock All Panes in Tab", LockItem::UnlockAll)]
        );
        assert_eq!(lock_items(3, true, true, true), vec![("Lock Pane", LockItem::Pane(true)), ("Unlock All Panes in Tab", LockItem::UnlockAll)]);
    }

    #[test]
    fn lock_refuses_moves() {
        assert!(!move_refused(false, false));
        assert!(move_refused(true, false));
        assert!(move_refused(false, true));
        assert!(move_refused(true, true));
        assert_eq!(close_reason_for(false, true), Some(crate::close_rules::LOCKED_REASON));
        assert_eq!(close_reason_for(true, true), None);
        assert_eq!(close_reason_for(false, false), None);
    }
}
