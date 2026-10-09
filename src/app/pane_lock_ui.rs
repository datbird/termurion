//! Lock Pane (`PTYSession locked`): the pane that refuses moves and asks before closing,
//! its context menu items and its title bar lock mark.

use super::*;
use super::semantic_ui::CtxItem;
use crate::close_rules::LOCKED_REASON;

/// The lock items after Toggle Broadcasting Input: Lock Pane, then for a tab with several
/// panes Unlock All Panes in Tab when all are locked, else Lock All Panes in Tab and, when
/// any is locked, Unlock All Panes in Tab too (iTerm2 hides that one behind Option).
pub(super) fn lock_items(tab_panes: usize, locked: bool, all_locked: bool, any_locked: bool) -> Vec<(&'static str, CtxItem)> {
    let mut out = vec![("Lock Pane", CtxItem::LockPane(locked))];
    if tab_panes > 1 {
        if !all_locked {
            out.push(("Lock All Panes in Tab", CtxItem::LockAll));
        }
        if any_locked {
            out.push(("Unlock All Panes in Tab", CtxItem::UnlockAll));
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

/// Whether a pane's profile adds its own close reasons: not an exited pane, and not a live
/// locked one, whose only reason is the lock (`PS:3069-3078`).
pub(super) fn profile_reasons_apply(exited: bool, locked: bool) -> bool {
    !exited && !locked
}

/// Whether Lock Split Pane Width is offered: a non-tmux pane of a tab with several panes, with a
/// side by side split above it (`PS:21736-21754`).
pub(super) fn width_lock_allowed(tmux: bool, tab_panes: usize, row_ancestor: bool) -> bool {
    !tmux && tab_panes > 1 && row_ancestor
}

impl App {
    /// The menu item: the focused pane takes its current width in points as its cap, or loses
    /// the cap (`PS:21756-21764`). Not offered, nothing happens.
    pub(super) fn toggle_width_lock(&mut self) {
        let Some(id) = self.focused_id() else { return };
        let Some(tab) = self.tabs.get(self.active) else { return };
        let panes = tab.layout.panes();
        let allowed = width_lock_allowed(self.tmux_of(id).is_some(), panes.len(), crate::layout::has_row_ancestor(&tab.layout.tree(), id));
        if !allowed {
            return;
        }
        let area = self.content_area_for(tab);
        let width = tab.layout.rects(area, self.gap()).iter().find(|(p, _)| *p == id).map(|(_, r)| r.w / self.scale());
        let Some(p) = self.panes.get_mut(&id) else { return };
        p.pinned_width = if p.pinned_width.is_some() { None } else { width };
        self.redraw();
    }

    /// Whether the focused pane holds its width (the menu's check mark).
    pub(super) fn width_lock_checked(&self) -> bool {
        self.focused_id().and_then(|id| self.panes.get(&id)).is_some_and(|p| p.pinned_width.is_some())
    }

    /// A window resize caps every pinned pane of every tab at its width, in layout order
    /// (`TAB:6596-6640`, `resizeSubviewsWithOldSize:`). A pin on a pane whose parent is no longer
    /// a side by side split does nothing; tmux panes keep tmux's sizes.
    pub(super) fn apply_width_pins(&mut self) {
        let (scale, gap) = (self.scale(), self.gap());
        for ti in 0..self.tabs.len() {
            let area = self.content_area_for(&self.tabs[ti]);
            for id in self.tabs[ti].layout.panes() {
                let Some(width) = self.panes.get(&id).and_then(|p| p.pinned_width) else { continue };
                if self.tmux_of(id).is_some() {
                    continue;
                }
                let pinned = |q: PaneId| self.panes.get(&q).is_some_and(|p| p.pinned_width.is_some());
                self.tabs[ti].layout.pin_width(id, width * scale, area, gap, &pinned);
            }
        }
    }

    /// A pane that was just created takes "Default Pane Locked" from its profile
    /// (`PS:6211-6213`, once, when its profile first loads).
    pub(super) fn apply_new_pane_defaults(&mut self, id: PaneId) {
        let locked = self.panes.get(&id).and_then(|p| self.store.get(&p.profile)).is_some_and(|pr| pr.bool("Default Pane Locked"));
        if let Some(p) = self.panes.get_mut(&id) {
            p.locked = locked;
        }
    }

    /// What a session reads once, when it starts (13p, ruling P3): `IdleTimeSeconds` goes to the
    /// pane and its activity tracker (`PS:849`; a changed setting applies to new sessions only,
    /// and a restarted session keeps its tracker, so the tracker is told here).
    pub(super) fn apply_session_start(&mut self, id: PaneId) {
        let idle = crate::cadence::idle_time(self.globals.number("IdleTimeSeconds"));
        if let Some(p) = self.panes.get_mut(&id) {
            p.idle_time = idle;
        }
        self.activity.entry(id).or_default().set_idle(idle);
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
    pub(super) fn lock_items_for(&self, pane: PaneId) -> Vec<(&'static str, CtxItem)> {
        let ids = self.tabs.iter().find(|t| t.layout.panes().contains(&pane)).map(|t| t.layout.panes()).unwrap_or_else(|| vec![pane]);
        let locks: Vec<bool> = ids.iter().map(|id| self.panes.get(id).is_some_and(|p| p.locked)).collect();
        lock_items(ids.len(), self.panes.get(&pane).is_some_and(|p| p.locked), locks.iter().all(|l| *l), locks.iter().any(|l| *l))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_reapply_width_pins() {
        // PTYTab.m resizeSubviewsWithOldSize: runs when a split is made too: the menu path and the
        // API path both pin before they relayout.
        let menu = include_str!("mod.rs");
        let split = menu.split("fn split_as(").nth(1).and_then(|t| t.split("fn close_pane(").next()).unwrap();
        assert!(split.contains("self.apply_width_pins();"), "split_as re-pins");
        let api = include_str!("api_requests.rs");
        let body = api.split("tab.layout.split(target, axis, id);").nth(1).and_then(|t| t.split("SplitResult::Made(id)").next()).unwrap();
        assert!(body.contains("self.apply_width_pins();"), "api_split re-pins");
    }

    #[test]
    fn status_tool_uses_the_title_ordinals() {
        let tool = include_str!("toolbelt_more_ui.rs");
        assert!(tool.contains("pane_title::ordinal_of(&ordered, visible)"), "the Status tool shares the title bar helper");
        let titles = include_str!("pane_title_ui.rs");
        assert!(titles.contains("pane_title::ordinal_of(&panes, id)"), "so do the title bars");
        assert!(tool.contains("self.ordered_panes(t).hash(h)"), "the Status hash follows the pane order");
    }

    #[test]
    fn lock_items_follow_iterm2() {
        let ids = |a, b, c, d| lock_items(a, b, c, d).into_iter().map(|(l, i)| (l, i.id())).collect::<Vec<_>>();
        assert_eq!(ids(1, false, false, false), vec![("Lock Pane", CtxItem::LockPane(false).id())]);
        assert_eq!(ids(3, false, false, false), vec![("Lock Pane", CtxItem::LockPane(false).id()), ("Lock All Panes in Tab", CtxItem::LockAll.id())]);
        assert_eq!(
            ids(3, true, false, true),
            vec![("Lock Pane", CtxItem::LockPane(true).id()), ("Lock All Panes in Tab", CtxItem::LockAll.id()), ("Unlock All Panes in Tab", CtxItem::UnlockAll.id())]
        );
        assert!(matches!(lock_items(1, true, true, true)[0].1, CtxItem::LockPane(true)), "the check follows the lock");
        assert!(matches!(lock_items(1, false, false, false)[0].1, CtxItem::LockPane(false)));
        assert_eq!(ids(3, true, true, true), vec![("Lock Pane", CtxItem::LockPane(true).id()), ("Unlock All Panes in Tab", CtxItem::UnlockAll.id())]);
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
        assert!(profile_reasons_apply(false, false));
        assert!(!profile_reasons_apply(false, true), "a locked pane's only reason is the lock");
        assert!(!profile_reasons_apply(true, false));
    }

    #[test]
    fn width_lock_offers_follow_iterm2() {
        assert!(width_lock_allowed(false, 2, true));
        assert!(!width_lock_allowed(true, 2, true), "tmux");
        assert!(!width_lock_allowed(false, 1, true), "solo");
        assert!(!width_lock_allowed(false, 3, false), "no side-by-side split");
    }
}
