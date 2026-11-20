//! What decides whether output needs the menu bar's check marks synced. `after_output` syncs
//! only when this key moves, so a read of plain text costs one comparison, not a dozen menu
//! refreshes.

use super::*;
use super::ai_explain_app::has_selection;

/// Everything outside a `Screen` that output (or the end of a shell) can change and a menu
/// shows. The screens' own facts arrive through their epochs (`Screen::menu_facts`).
#[derive(Clone, PartialEq, Debug)]
pub(super) struct OutputMenuKey {
    pub focused: Option<PaneId>,
    /// Wrapping sum over every pane of both screens' `menu_epoch`.
    pub epochs: u64,
    /// The focused pane's `ended || exited.is_some()`.
    pub exited: Option<bool>,
    /// The focused pane's `log.is_some()`.
    pub logging: Option<bool>,
    /// The focused pane's `has_selection`.
    pub selection: bool,
    pub transfers: u64,
    pub menus_dirty: bool,
    /// Auto Composer's check follows the shell history, which output (a command mark) fills.
    pub auto_composer: bool,
}

impl App {
    pub(super) fn output_menu_key(&self) -> OutputMenuKey {
        let focused = self.tabs.get(self.active).map(|t| t.focused);
        let pane = focused.and_then(|id| self.panes.get(&id));
        let epochs = self.panes.values().fold(0u64, |sum, p| sum.wrapping_add(p.own().menu_epoch()).wrapping_add(p.screen.menu_epoch()));
        OutputMenuKey {
            focused,
            epochs,
            exited: pane.map(|p| p.ended || p.exited.is_some()),
            logging: pane.map(|p| p.log.is_some()),
            selection: pane.is_some_and(|p| has_selection(p.own())),
            transfers: self.transfers.generation(),
            menus_dirty: self.menus_dirty,
            auto_composer: self.auto_composer_checked(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> OutputMenuKey {
        OutputMenuKey { focused: Some(1), epochs: 7, exited: Some(false), logging: Some(false), selection: false, transfers: 0, menus_dirty: false, auto_composer: false }
    }

    #[test]
    fn the_key_follows_focus_and_dirty() {
        let base = key();
        assert_eq!(base, key());
        assert_ne!(base, OutputMenuKey { focused: Some(2), ..key() });
        assert_ne!(base, OutputMenuKey { menus_dirty: true, ..key() });
        assert_ne!(base, OutputMenuKey { selection: true, ..key() });
        assert_ne!(base, OutputMenuKey { epochs: 8, ..key() });
        assert_ne!(base, OutputMenuKey { exited: Some(true), ..key() });
        assert_ne!(base, OutputMenuKey { logging: Some(true), ..key() });
        assert_ne!(base, OutputMenuKey { transfers: 1, ..key() });
    }

    /// The body of `after_output`: only a changed key runs the menu sync.
    #[test]
    fn after_output_syncs_only_on_a_new_key() {
        let src = include_str!("mod.rs");
        let from = src.find("fn after_output(").expect("after_output");
        let body = src[from..].split("\n    }\n").next().unwrap();
        assert!(body.contains("output_menu_key()"));
        let at = body.find("self.sync_checks()").expect("the sync");
        assert_eq!(body.matches("self.sync_checks()").count(), 1);
        let branch = body[..at].rfind("if ").map(|i| &body[i..at]).unwrap();
        assert!(branch.contains("!=") && branch.contains("self.menu_key"), "the sync sits in the key's != branch: {branch}");
    }
}
