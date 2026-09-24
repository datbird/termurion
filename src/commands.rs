//! Which iTerm2 menu commands work in Tandem now, and which milestone brings the rest.
//! A command owned by a later milestone shows disabled; it never half-works.

use crate::menus::Item;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// Works as in iTerm2.
    Done,
    /// Works, with the named difference until a later milestone.
    Partial(&'static str),
    /// Shown disabled until this milestone.
    Later(u8),
}

/// Commands that work now. "selector|Label" names one item of a shared selector.
pub const DONE: &[&str] = &[
    "showAbout:", "showPrefWindow:", "terminate:", "hide:", "newSession:", "newSessionAlternate:",
    "newSessionWithSameProfile:", "duplicateTab:", "splitHorizontally:", "splitVertically:",
    "openSplitHorizontallySheet:", "openSplitVerticallySheet:", "saveContents:", "saveDocumentAs:",
    "closeCurrentSession:", "closeTerminalWindow:", "closeCurrentTab:", "copy:", "paste:|Paste",
    "pasteSelection:", "selectAll:", "clearBuffer:", "clearScrollbackBuffer:", "toggleFullScreenMode:",
    "biggerFont:", "smallerFont:", "returnToDefaultSize:|Make Text Normal Size",
    "returnToDefaultSize:|Restore Text and Session Size", "maximizePane:",
    "editCurrentSession:", "reset:", "resetCharset:", "terminalStateReset:",
    "terminalStateToggleAlternateScreen:", "terminalStateToggleFocusReporting:",
    "terminalStateToggleMouseReporting:", "terminalStateTogglePasteBracketing:",
    "terminalStateToggleApplicationCursor:", "terminalStateToggleApplicationKeypad:|Application Keypad",
    "showBookmarkWindow:", "performMiniaturize:", "performZoom:", "nextTab:", "previousTab:", "moveTabLeft:",
    "moveTabRight:", "selectPaneUp:", "selectPaneDown:", "selectPaneLeft:", "selectPaneRight:", "nextPane:",
    "previousPane:", "movePaneDividerUp:", "movePaneDividerDown:", "movePaneDividerLeft:",
    "movePaneDividerRight:", "arrangeSplitPanesEvenly:", "restartSession:", "duplicateSession:", "showHelp:",
    "arrangeInFront:", "setWindowStyle:|Normal", "setWindowStyle:|Full Screen", "setWindowStyle:|Maximized",
];

/// Commands that work with a known difference for now.
const PARTIAL: &[(&str, &str)] = &[
    ("newWindow:", "Opens a tab until separate windows arrive in milestone 7."),
    ("newWindowWithSameProfile:", "Opens a tab until separate windows arrive in milestone 7."),
    ("biggerFont:", "Changes every pane, since all panes share one font until per-pane fonts arrive."),
    ("smallerFont:", "Changes every pane, since all panes share one font until per-pane fonts arrive."),
];

fn in_list(list: &[&str], item: &Item) -> bool {
    list.iter().any(|d| match d.split_once('|') {
        Some((sel, label)) => item.selector == sel && item.label() == label,
        None => item.selector == *d,
    })
}

/// The milestone that brings a command, from the roadmap (docs/parity/00-roadmap.md).
fn owner(item: &Item) -> u8 {
    let s = item.selector.as_str();
    let path = item.id();
    let has = |needle: &str| path.contains(needle);
    let sel = |needle: &str| s.contains(needle);
    if has("tmux") || sel("Tmux") {
        8
    } else if has("Scripts") || sel("Script") || sel("REPL") || sel("PythonRuntime") || sel("Dependency") {
        11
    } else if sel("AI") || sel("NaturalLanguage") || sel("Cockpit") || sel("InlineChat") {
        12
    } else if sel("ir") && s.starts_with("ir") || sel("InstantReplay") || sel("PasswordManager") || sel("Recording") {
        10
    } else if has("Find") || has("Marks") || sel("Mark") || sel("Annotation") || sel("Note") || sel("fold") || sel("Filter")
        || sel("Selection") || sel("jumpTo") || sel("CurrentCommand") || sel("LastCommand") || sel("openSelection")
        || sel("clearTo") || sel("Timestamps") || sel("CursorGuide") || sel("findCursor") || sel("zoom")
        || sel("installShellIntegration")
    {
        4
    } else if sel("Trigger") || sel("Coprocess") || sel("Notify") || sel("Logging") && has("Log") {
        6
    } else if has("Broadcast") || has("Arrangements") || has("Window Style") || has("Arrange Windows") || has("Resize Window")
        || sel("Workgroup") || sel("TabGroup") || sel("Group") || sel("moveSession") || sel("movePane") || sel("bury")
        || sel("Hotkey") || sel("Archive") || sel("TabToNewWindow") || sel("Locked") || sel("duplicateWindow")
        || sel("undoClose") || sel("editWindowTitle") || sel("editTabTitle") || sel("Transparency") || sel("FullScreenTabBar")
        || sel("mergeAll") || sel("arrangeHorizontally") || sel("SizeChanges") || sel("LockSplitPaneWidth")
    {
        7
    } else if has("Toolbelt") || has("Paste Special") || sel("compose") || sel("Composer") || sel("openQuickly")
        || sel("History") || sel("Directories") || sel("Autocomplete") || sel("Clippings") || sel("Snippet")
    {
        9
    } else if has("Terminal State") || sel("copyWith") || sel("CopyMode") || sel("undo") || sel("redo") || sel("cut:")
        || sel("replaceSelection") || sel("renderSelection") || sel("print") || sel("PageLayout") || sel("ssh")
        || sel("downloadFiles") || sel("RemoteHost") || sel("Screenshot") || sel("BufferInput")
    {
        5
    } else {
        13
    }
}

pub fn status(item: &Item) -> Status {
    // A partial note wins over DONE: the command runs, and the note says how it differs.
    if let Some((_, note)) = PARTIAL.iter().find(|(sel, _)| item.selector == *sel) {
        return Status::Partial(note);
    }
    if in_list(DONE, item) {
        return Status::Done;
    }
    Status::Later(owner(item))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::menus::items;

    #[test]
    fn every_done_entry_names_a_real_menu_command() {
        for d in DONE {
            let (sel, label) = d.split_once('|').map_or((*d, None), |(s, l)| (s, Some(l)));
            assert!(
                items().iter().any(|i| i.selector == sel && label.is_none_or(|l| i.label() == l)),
                "{d} is not in the menu"
            );
        }
    }

    #[test]
    fn every_command_has_a_status() {
        for i in items().iter().filter(|i| !i.separator && !i.submenu && !i.selector.is_empty()) {
            let s = status(i);
            if let Status::Later(m) = s {
                assert!((4..=13).contains(&m), "{} -> milestone {m}", i.id());
            }
        }
    }

    /// Prints every command's status for the parity inventory: `cargo test --lib dump_statuses -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn dump_statuses() {
        for i in items().iter().filter(|i| !i.separator && !i.submenu && !i.selector.is_empty()) {
            println!("STATUS\t{}\t{:?}", i.id(), status(i));
        }
    }

    #[test]
    fn known_statuses() {
        let find = |sel: &str, label: &str| items().iter().find(|i| i.selector == sel && i.label() == label).unwrap();
        assert_eq!(status(find("splitVertically:", "Split Vertically with Current Profile")), Status::Done);
        assert!(matches!(status(find("newWindow:", "New Window")), Status::Partial(_)));
        assert_eq!(status(find("performFindPanelAction:", "Find...")), Status::Later(4));
        assert_eq!(status(find("detachTmux:", "Detach")), Status::Later(8));
        assert_eq!(status(find("setWindowStyle:", "Maximized")), Status::Done);
        assert_eq!(status(find("setWindowStyle:", "Full-Width Top of Screen")), Status::Later(7));
    }
}
