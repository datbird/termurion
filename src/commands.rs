//! Which iTerm2 menu commands work in Tandem now, and which milestone brings the rest.
//! A command owned by a later milestone shows disabled; it never half-works.

use crate::menus::{self, Chord, ChordKey, Item};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// Works as in iTerm2.
    Done,
    /// Works, with the named difference until a later milestone.
    Partial(&'static str),
    /// Shown disabled until this milestone.
    Later(u8),
    /// Never built, with the reason (milestone 14e ruling P7): shown disabled like `Later`, but it
    /// promises no milestone.
    NotApplicable(&'static str),
}

impl Status {
    /// The command does nothing and its menu item is disabled.
    pub fn unavailable(self) -> bool {
        matches!(self, Status::Later(_) | Status::NotApplicable(_))
    }
}

/// Commands iTerm2 keeps disabled while a terminal has focus: Undo and Redo have nothing
/// to undo there, and Cut is never allowed.
pub const DISABLED_IN_SESSION: &[&str] = &["undo:", "redo:", "cut:"];

/// Commands iTerm2 disables while its Python runtime comes from uv, its default and Tandem's
/// only way (`AD:454-462`): installing a hand-downloaded legacy runtime zip means nothing then.
pub const DISABLED_UNDER_UV: &[&str] = &["installAlreadyDownloadedPythonRuntime:"];

/// Commands that work now. "selector|Label" names one item of a shared selector.
pub const DONE: &[&str] = &[
    "showAbout:", "showPrefWindow:", "checkForUpdatesFromMenu:", "showTipOfTheDay:", "copyModeShortcuts:", "openSourceLicenses:", "terminate:", "hide:", "newSession:", "newSessionAlternate:",
    "newSessionWithSameProfile:", "duplicateTab:", "splitHorizontally:", "splitVertically:",
    "openSplitHorizontallySheet:", "openSplitVerticallySheet:", "saveContents:", "saveDocumentAs:",
    "closeCurrentSession:", "closeTerminalWindow:", "closeCurrentTab:", "copy:", "paste:|Paste",
    "startStopLogging:", "debugLogging:", "saveRetrospectiveDebugLogs:", "saveArchive:", "restoreArchive:", "detachTmux:", "forceDetachTmux:", "newTmuxWindow:", "newTmuxTab:", "toggleTmuxPausePane:", "openDashboard:", "revealCockpit:", "bury:",
    "pasteSelection:", "selectAll:", "clearBuffer:", "clearScrollbackBuffer:", "toggleFullScreenMode:", "toggleFullScreenTabBar:", "toggleUseTransparency:", "toggleDisableTransparencyForActiveWindow:", "toggleLockSplitPaneWidth:",
    "nextTabGroup:", "previousTabGroup:", "addTabToNewGroupAction:", "removeTabFromGroupAction:",
    "copyPerformanceStats:", "gpuRendererAvailability:", "biggerFont:", "smallerFont:", "toggleSizeChangesAffectProfile:", "returnToDefaultSize:|Make Text Normal Size",
    "returnToDefaultSize:|Restore Text and Session Size", "maximizePane:",
    "editCurrentSession:", "reset:", "resetCharset:", "terminalStateReset:",
    "terminalStateToggleAlternateScreen:", "terminalStateToggleFocusReporting:",
    "terminalStateToggleMouseReporting:", "terminalStateTogglePasteBracketing:",
    "terminalStateToggleApplicationCursor:", "terminalStateToggleApplicationKeypad:|Application Keypad",
    "terminalStateToggleApplicationKeypad:|Standard Key Reporting Mode",
    "terminalStateToggleApplicationKeypad:|modifyOtherKeys Mode 1",
    "terminalStateToggleApplicationKeypad:|modifyOtherKeys Mode 2", "terminalStateToggleApplicationKeypad:|CSI u Mode",
    "terminalStateToggleApplicationKeypad:|Raw Key Reporting Mode",
    "terminalStateToggleApplicationKeypad:|Disambiguate Escape",
    "terminalStateToggleApplicationKeypad:|Report All Event Types",
    "terminalStateToggleApplicationKeypad:|Report Alternate Keys",
    "terminalStateToggleApplicationKeypad:|Report All Keys as Escape Codes",
    "terminalStateToggleApplicationKeypad:|Report Associated Text", "terminalStateToggleLiteralMode:",
    "terminalStateSetEmulationLevel:", "toggleBufferInput:", "copyWithStyles:", "copyWithControlSequences:",
    "undo:", "redo:", "cut:", "print:", "runPageLayout:", "makeScreenshot:", "renderSelection:", "addTrigger:", "editTriggers:",
    "toggleEnableTriggersInInteractiveApps:", "enableAllTriggers:", "disableAllTriggers:", "toggleNotifyOnStatusChange:", "runCoprocess:", "stopCoprocess:", "newWindow:", "newWindowWithSameProfile:", "makeDefaultTerminal:", "unmakeDefaultTerminal:", "removeRecentProfilesFromDockMenu:",
    "moveTabToNewWindow:", "moveSessionToWindow:", "moveSessionToTab:", "paste:|Paste Slowly", "pasteFaster:", "pasteSlower:",
    "pasteSlowlyFaster:", "pasteSlowlySlower:", "pasteBase64Encoded:", "pasteOptions:", "openPasteHistory:", "openCommandHistory:", "openDirectories:", "openAutocomplete:", "toggleAutoCommandHistory:", "compose:", "toggleAutoComposer:", "toggleShowClippings:", "toggleMultiLinePasteWarning:|Warn Before Multi-Line Paste",
    "toggleMultiLinePasteWarning:|Limit Multi-Line Paste Warning to Shell Prompt",
    "toggleMultiLinePasteWarning:|Warn Before Pasting One Line Ending in a Newline at Shell Prompt", "promptToConvertTabsToSpacesWhenPasting:", "mergeAllWindows:", "duplicateWindow:", "togglePinHotkeyWindow:",
    "disableBroadcasting:", "enableSendInputToAllTabs:", "enableSendInputToAllPanes:", "toggleBroadcastingToCurrentSession:",
    "toggleUseBackgroundPatternIndicator:", "toggleBroadcastSource:",
    "toggleSizeLocked:", "toggleLayoutLocked:", "decreaseHeight:", "increaseHeight:", "decreaseWidth:", "increaseWidth:",
    "editWindowTitle:", "editTabTitle:", "arrangeHorizontally:", "undoCloseSession:",
    "saveWindowArrangement:", "saveCurrentWindowAsArrangement:", "importWindowArrangement:", "changeProfileInArrangement:",
    "showBookmarkWindow:", "installShellIntegration:", "installPythonRuntime:", "revealScriptsInFinder:", "newPythonScript:", "openREPL:", "importScript:", "exportScript:", "openDependencyEditor:", "openScriptConsole:", "showSuppressedAlerts:", "saveScrollPosition:", "addNamedMark:", "jumpToSavedScrollPosition:", "nextMark:",
    "previousMark:", "clearToLastMark:", "selectOutputOfLastCommand:", "selectCurrentCommand:",
    "toggleAlertOnNextMark:", "toggleAlertOnMarksInOffscreenSessions:", "enableMarkAlertShowsModalAlert:",
    "enableMarkAlertPostsNotification:", "enableMarkAlertRingsBell:", "performMiniaturize:", "performZoom:", "nextTab:", "previousTab:", "moveTabLeft:",
    "moveTabRight:", "selectPaneUp:", "selectPaneDown:", "selectPaneLeft:", "selectPaneRight:", "nextPane:",
    "previousPane:", "movePane:", "movePaneDividerUp:", "movePaneDividerDown:", "movePaneDividerLeft:",
    "movePaneDividerRight:", "arrangeSplitPanesEvenly:", "restartSession:", "duplicateSession:", "showHelp:",
    "arrangeInFront:", "performFindPanelAction:|Find...", "performFindPanelAction:|Use Selection for Find",
    "findNext:", "findPrevious:", "clearFindString:", "jumpToSelection:", "zoomOut:", "findUrls:", "performFindPanelAction:|Select Matches",
    "smartSelectAllVisible:", "revealContentNavigationShortcuts:", "findGlobally:", "openQuickly:", "openSelection:",
    "replaceSelectionWithPrettyPrintedJSON:", "replaceSelectionWithBase64Encoded:", "replaceSelectionWithBase64Decoded:",
    "clearToStartOfSelection:", "toggleCopyMode:", "foldSelection:", "foldAll:", "addNoteAtCursor:", "annotateSelection:", "nextAnnotation:",
    "previousAnnotation:", "showHideNotes:", "toggleShowTimestamps:", "toggleCursorGuide:", "findCursor:", "editSessionNote:", "setWindowStyle:|Normal", "setWindowStyle:|Full Screen", "setWindowStyle:|Maximized",
    "setWindowStyle:|No Title Bar", "setWindowStyle:|Centered", "setWindowStyle:|Centered, No Title Bar",
    "setWindowStyle:|Full-Width Bottom of Screen", "setWindowStyle:|Full-Width Top of Screen",
    "setWindowStyle:|Full-Height Left of Screen", "setWindowStyle:|Full-Height Right of Screen",
    "setWindowStyle:|Bottom of Screen", "setWindowStyle:|Top of Screen", "setWindowStyle:|Left of Screen",
    "setWindowStyle:|Right of Screen", "toggleToolbeltVisibility:", "setDefaultToolbeltWidth:",
    "irPrev:", "clearInstantReplay:", "exportRecording:", "loadRecording:", "openAIChats:", "openAIChat:", "toggleShowInlineChat:", "performNaturalLanguageQuery:", "explainOutputWithAI:", "openPasswordManager:",
    "sshDisconnect:", "downloadFiles:",
    "installClaudeCodeIntegration:", "reinstallClaudeCodeIntegration:", "uninstallClaudeCodeIntegration:",
];

/// Commands that will not be built, with the reason (rulings 3 and 4 of milestone 14).
const NOT_APPLICABLE: &[(&str, &str)] = &[
    (
        "toggleRemoteHostCanControlIterm2:",
        "Milestone 11 did not port the it2 command tree, so there is nothing for a remote host to control.",
    ),
    (
        "debugCheckSettingsControlTruncation:",
        "iTerm2 builds this only for development, and Tandem's Settings wrap their text rather than truncating it.",
    ),
];

/// Capture GPU Frame exists only in debug builds (`AD:402-419`); a release build does not have it.
#[cfg(not(debug_assertions))]
const RELEASE_NOT_APPLICABLE: (&str, &str) = ("captureNextMetalFrame:", "Tandem builds this only for development; iTerm2's release builds remove it.");

/// Commands that work with a known difference for now.
const PARTIAL: &[(&str, &str)] = &[
    ("toggleSelectionRespectsSoftBoundaries:", "Double-click and Smart Selection stop at pane dividers; a drag still crosses them."),
];

/// A "selector" or "selector|Label" entry names this item.
fn names(entry: &str, item: &Item) -> bool {
    match entry.split_once('|') {
        Some((sel, label)) => item.selector == sel && item.label() == label,
        None => item.selector == entry,
    }
}

fn in_list(list: &[&str], item: &Item) -> bool {
    list.iter().any(|d| names(d, item))
}

/// The milestone that brings a command, from the roadmap (docs/parity/00-roadmap.md).
fn owner(item: &Item) -> u8 {
    let s = item.selector.as_str();
    let path = item.id();
    let has = |needle: &str| path.contains(needle);
    let sel = |needle: &str| s.contains(needle);
    if sel("renderSelection") {
        5
    } else if has("tmux") || sel("Tmux") {
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
        || sel("mergeAll") || sel("arrangeHorizontally") || sel("SizeChanges")
    {
        7
    } else if has("Toolbelt") || has("Paste Special") || sel("compose") || sel("Composer") || sel("openQuickly")
        || sel("History") || sel("Directories") || sel("Autocomplete") || sel("Clippings") || sel("Snippet")
    {
        9
    } else if sel("ssh") || sel("downloadFiles") || sel("RemoteHost") {
        14
    } else if has("Terminal State") || sel("copyWith") || sel("CopyMode") || sel("undo") || sel("redo") || sel("cut:")
        || sel("replaceSelection") || sel("renderSelection") || sel("print") || sel("PageLayout") || sel("Screenshot")
        || sel("BufferInput")
    {
        5
    } else {
        13
    }
}

/// iTerm2's right-click menu, by menu item id. An empty entry is a separator.
pub const CONTEXT: [&str; 12] = [
    "Edit > Copy",
    "Edit > Paste",
    "",
    "Shell > Split Vertically with Current Profile",
    "Shell > Split Horizontally with Current Profile",
    "",
    "View > Maximize Active Pane",
    "Edit > Select All",
    "Edit > Clear Buffer",
    "",
    "Session > Edit Session...",
    "Shell > Close",
];

/// Extra chords for commands whose iTerm2 chord is awkward on a Windows keyboard:
/// Cmd+= for Make Text Bigger (iTerm2's is Cmd++, which is Shift+= here).
const EXTRA_CHORDS: [(Chord, &str); 1] =
    [(Chord { key: ChordKey::Char('='), cmd: true, alt: false, shift: false, ctrl: false }, "View > Make Text Bigger")];

/// The command a chord runs: the first item in menu order with this chord that works now
/// (alternates last). None when every item with the chord belongs to a later milestone,
/// so the keyboard hook leaves the chord to Windows.
pub fn command_for_chord(chord: &Chord) -> Option<&'static Item> {
    if let Some((_, id)) = EXTRA_CHORDS.iter().find(|(c, _)| c == chord) {
        return menus::by_id(id);
    }
    menus::items_for(chord).into_iter().find(|i| !status(i).unavailable())
}

pub fn status(item: &Item) -> Status {
    // A partial note wins over DONE: the command runs, and the note says how it differs.
    if let Some((_, note)) = PARTIAL.iter().find(|(entry, _)| names(entry, item)) {
        return Status::Partial(note);
    }
    if let Some((_, why)) = NOT_APPLICABLE.iter().find(|(entry, _)| names(entry, item)) {
        return Status::NotApplicable(why);
    }
    #[cfg(not(debug_assertions))]
    if item.selector == RELEASE_NOT_APPLICABLE.0 {
        return Status::NotApplicable(RELEASE_NOT_APPLICABLE.1);
    }
    #[cfg(debug_assertions)]
    if item.selector == "captureNextMetalFrame:" {
        return Status::Done;
    }
    if in_list(DONE, item) {
        return Status::Done;
    }
    Status::Later(owner(item))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// B8: GPU capture is debug-build code; a release build reports it not applicable and leaves
    /// its chord to Windows. Check Settings Control Truncation is never built (R5).
    #[test]
    fn release_has_no_gpu_capture() {
        let capture = items().iter().find(|i| i.selector == "captureNextMetalFrame:").unwrap();
        let truncation = items().iter().find(|i| i.selector == "debugCheckSettingsControlTruncation:").unwrap();
        assert!(matches!(status(truncation), Status::NotApplicable(why) if why.contains("development")));
        let chord = crate::menus::Chord { key: crate::menus::ChordKey::Char('g'), cmd: true, alt: true, shift: true, ctrl: true };
        if cfg!(debug_assertions) {
            assert_eq!(status(capture), Status::Done);
        } else {
            assert!(matches!(status(capture), Status::NotApplicable(_)));
            assert!(command_for_chord(&chord).is_none(), "the chord is left to Windows in a release build");
        }
        // The wgpu call is made in one debug-only file, inside a debug-only item.
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = vec![root.clone()];
        let mut found = Vec::new();
        while let Some(d) = files.pop() {
            for e in std::fs::read_dir(&d).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    files.push(p);
                } else if p.extension().is_some_and(|x| x == "rs") {
                    let t = std::fs::read_to_string(&p).unwrap();
                    for (i, line) in t.lines().enumerate() {
                        if line.contains("start_graphics_debugger_capture") && !line.trim_start().starts_with("//") && !p.ends_with("commands.rs") {
                            found.push((p.clone(), t.lines().take(i).collect::<Vec<_>>().join("\n")));
                        }
                    }
                }
            }
        }
        assert_eq!(found.len(), 1, "one call site: {:?}", found.iter().map(|f| &f.0).collect::<Vec<_>>());
        let (path, before) = &found[0];
        assert!(path.ends_with("gpu_capture.rs"), "{path:?}");
        let item_start = before.rfind("\nfn ").or_else(|| before.rfind("\npub fn ")).or_else(|| before.rfind("\npub(super) fn ")).expect("inside a function");
        let head: Vec<&str> = before[..item_start].lines().rev().take(3).collect();
        assert!(head.iter().any(|l| l.trim() == "#[cfg(debug_assertions)]"), "the function is debug-only: {head:?}");
    }

    #[test]
    fn move_pane_is_done() {
        assert_eq!(status(menus::by_identifier("Move Session to Split Pane").unwrap()), Status::Done);
    }
    use crate::menus::items;

    fn find(sel: &str, label: &str) -> &'static Item {
        items().iter().find(|i| i.selector == sel && i.label() == label).unwrap()
    }

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
                assert!((4..=14).contains(&m), "{} -> milestone {m}", i.id());
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

    fn win(key: crate::menus::ChordKey, shift: bool) -> crate::menus::Chord {
        crate::menus::Chord { key, cmd: true, alt: false, shift, ctrl: false }
    }

    #[test]
    fn a_shared_chord_goes_to_the_command_that_works() {
        // Show Clippings (Shift+Cmd+=) comes first in iTerm2's menu and works since
        // milestone 9f, so it gets the chord; Make Text Bigger keeps Cmd+=.
        use crate::menus::ChordKey::Char;
        assert_eq!(command_for_chord(&win(Char('='), true)).unwrap().selector, "toggleShowClippings:");
        assert_eq!(command_for_chord(&win(Char('='), false)).unwrap().selector, "biggerFont:");
    }

    #[test]
    fn a_chord_for_a_later_milestone_stays_with_windows() {
        use crate::menus::ChordKey::Char;
        assert_eq!(command_for_chord(&win(Char('t'), false)).unwrap().selector, "newSession:");
    }

    #[test]
    fn cockpit_is_done() {
        use crate::menus::ChordKey::Char;
        assert_eq!(status(find("revealCockpit:", "Cockpit")), Status::Done);
        let chord = crate::menus::Chord { key: Char('c'), cmd: true, alt: true, shift: true, ctrl: false };
        assert_eq!(command_for_chord(&chord).unwrap().selector, "revealCockpit:");
    }

    #[test]
    fn window_commands_are_done() {
        use crate::menus::ChordKey::Char;
        let chord = |shift: bool, ctrl: bool| crate::menus::Chord { key: Char('y'), cmd: true, alt: false, shift, ctrl };
        assert_eq!(command_for_chord(&chord(true, false)).unwrap().selector, "openAIChats:");
        assert_eq!(command_for_chord(&chord(true, true)).unwrap().selector, "openAIChat:");
        assert_eq!(command_for_chord(&chord(false, false)).unwrap().selector, "performNaturalLanguageQuery:");
    }

    #[test]
    fn transparency_items_are_done() {
        use crate::menus::ChordKey::Char;
        assert_eq!(status(find("toggleUseTransparency:", "Use Transparency")), Status::Done);
        assert_eq!(status(find("toggleDisableTransparencyForActiveWindow:", "Disable Transparency for Active Window")), Status::Done);
        assert_eq!(command_for_chord(&win(Char('u'), false)).unwrap().selector, "toggleUseTransparency:");
    }

    #[test]
    fn engage_is_done_and_takes_win_y() {
        use crate::menus::ChordKey::Char;
        assert_eq!(status(find("performNaturalLanguageQuery:", "Engage Artificial Intelligence")), Status::Done);
        assert_eq!(command_for_chord(&win(Char('y'), false)).unwrap().selector, "performNaturalLanguageQuery:");
    }

    #[test]
    fn explain_is_done_and_takes_win_alt_y() {
        use crate::menus::ChordKey::Char;
        assert_eq!(status(find("explainOutputWithAI:", "Explain Output with AI")), Status::Done);
        let chord = crate::menus::Chord { key: Char('y'), cmd: true, alt: true, shift: false, ctrl: false };
        assert_eq!(command_for_chord(&chord).unwrap().selector, "explainOutputWithAI:");
    }

    #[test]
    fn shift_win_comma_shows_the_inline_chat() {
        use crate::menus::ChordKey::Char;
        let chord = crate::menus::Chord { key: Char(','), cmd: true, alt: false, shift: true, ctrl: false };
        let item = command_for_chord(&chord).unwrap();
        assert_eq!(item.selector, "toggleShowInlineChat:");
        assert_eq!(status(item), Status::Done);
    }

    #[test]
    fn every_context_menu_id_resolves() {
        for id in CONTEXT {
            assert!(id.is_empty() || crate::menus::by_id(id).is_some(), "{id}");
        }
    }

    #[test]
    fn known_statuses() {
        let find = |sel: &str, label: &str| items().iter().find(|i| i.selector == sel && i.label() == label).unwrap();
        assert_eq!(status(find("splitVertically:", "Split Vertically with Current Profile")), Status::Done);
        assert_eq!(status(find("toggleShowTimestamps:", "Show Timestamps")), Status::Done, "13m: Adjacent has its own column, so no PARTIAL note");
        assert_eq!(status(find("newWindow:", "New Window")), Status::Done, "7a opens real windows");
        assert_eq!(status(find("moveSessionToTab:", "Move Session to Tab")), Status::Done, "8h");
        assert_eq!(status(find("performFindPanelAction:", "Find...")), Status::Done);
        assert_eq!(status(find("performFindPanelAction:", "Select Matches")), Status::Done, "13s: one selection per match");
        assert_eq!(status(find("performFindPanelAction:", "Find...")), Status::Done);
        assert_eq!(status(find("openDashboard:", "Dashboard")), Status::Done, "8e");
        assert_eq!(status(find("setWindowStyle:", "Maximized")), Status::Done);
        assert_eq!(status(find("setWindowStyle:", "Full-Width Top of Screen")), Status::Done, "every style arrives in 7a");
        assert_eq!(status(find("toggleToolbeltVisibility:", "Show Toolbelt")), Status::Done, "9j");
        assert_eq!(status(find("setDefaultToolbeltWidth:", "Set Default Width")), Status::Done, "9j");
        assert_eq!(status(find("openQuickly:", "Open Quickly")), Status::Done, "9k");
        assert_eq!(status(find("compose:", "Show Composer")), Status::Done, "9l");
        assert_eq!(status(find("toggleAutoComposer:", "Auto Composer")), Status::Done, "9l");
        assert_eq!(status(find("irPrev:", "Start Instant Replay")), Status::Done, "10a");
        assert_eq!(status(find("clearInstantReplay:", "Clear Instant Replay")), Status::Done, "10a");
        assert_eq!(status(find("exportRecording:", "Export Recording")), Status::Done, "10a");
        assert_eq!(status(find("loadRecording:", "Import Recording")), Status::Done, "10a");
        assert_eq!(status(find("openPasswordManager:", "Password Manager")), Status::Done, "10b");
        assert_eq!(status(find("installPythonRuntime:", "Install Python Runtime")), Status::Done, "11d");
        assert_eq!(status(find("revealScriptsInFinder:", "Reveal Scripts in Finder")), Status::Done, "11d");
        assert_eq!(status(find("newPythonScript:", "New Python Script")), Status::Done, "11d");
        assert_eq!(status(find("openScriptConsole:", "Console")), Status::Done, "11d");
        assert_eq!(status(find("openREPL:", "Open Python REPL")), Status::Done, "11d");
        assert_eq!(status(find("importScript:", "Import...")), Status::Done, "11d");
        assert_eq!(status(find("exportScript:", "Export...")), Status::Done, "11d");
        assert_eq!(status(find("openDependencyEditor:", "Manage Dependencies...")), Status::Done, "11d");
        assert_eq!(status(find("showSuppressedAlerts:", "Suppressed Alerts...")), Status::Done, "13b");
        assert_eq!(status(find("openAIChats:", "AI Chats")), Status::Done, "12d");
        assert_eq!(status(find("openAIChat:", "Open AI Chat...")), Status::Done, "12d");
        assert_eq!(status(find("toggleShowInlineChat:", "Show Inline Chat")), Status::Done, "12d");
        assert_eq!(status(find("sshDisconnect:", "Disconnect")), Status::Done, "14c");
        for (sel, label) in [("installClaudeCodeIntegration:", "Install Claude Code Integration"), ("reinstallClaudeCodeIntegration:", "Reinstall Claude Code Integration"), ("uninstallClaudeCodeIntegration:", "Uninstall Claude Code Integration")] {
            assert_eq!(status(find(sel, label)), Status::Done, "12f");
        }
        assert_eq!(status(find("downloadFiles:", "Download Files...")), Status::Done, "14d");
        assert_eq!(
            status(find("toggleRemoteHostCanControlIterm2:", "Remote host can control iTerm2")),
            Status::NotApplicable("Milestone 11 did not port the it2 command tree, so there is nothing for a remote host to control."),
            "14e ruling 3: not applicable, no milestone"
        );
        assert_eq!(owner(find("toggleCopyMode:", "Copy Mode")), 5, "done in 5, and not moved with ssh");
        assert!(DISABLED_UNDER_UV.contains(&"installAlreadyDownloadedPythonRuntime:"));
        for sel in DISABLED_UNDER_UV {
            assert!(items().iter().any(|i| i.selector == *sel), "{sel} is not in the menu");
        }
    }

    #[test]
    fn a_not_applicable_command_is_unavailable_and_owns_no_chord() {
        let item = find("toggleRemoteHostCanControlIterm2:", "Remote host can control iTerm2");
        assert!(matches!(status(item), Status::NotApplicable(_)));
        assert!(status(item).unavailable());
        assert!(Status::Later(14).unavailable());
        assert!(!Status::Done.unavailable() && !Status::Partial("x").unavailable());
    }

    #[test]
    fn download_files_is_done() {
        let item = items().iter().find(|i| i.selector == "downloadFiles:").expect("Download Files is in the menu");
        assert_eq!(status(item), Status::Done);
    }

    #[test]
    fn tab_group_commands_work() {
        use crate::menus::{Chord, ChordKey};
        for sel in ["nextTabGroup:", "previousTabGroup:", "addTabToNewGroupAction:", "removeTabFromGroupAction:"] {
            let item = items().iter().find(|i| i.selector == sel).unwrap_or_else(|| panic!("{sel}"));
            assert_eq!(status(item), Status::Done, "{sel}");
        }
        let chord = |c: char| Chord { key: ChordKey::Char(c), cmd: true, alt: false, shift: true, ctrl: true };
        assert_eq!(command_for_chord(&chord(']')).unwrap().selector, "nextTabGroup:", "Ctrl+Shift+Win+]");
        assert_eq!(command_for_chord(&chord('[')).unwrap().selector, "previousTabGroup:", "Ctrl+Shift+Win+[");
        let dup = items().iter().find(|i| i.selector == "duplicateTab:").unwrap();
        assert_eq!(status(dup), Status::Done);
    }

    #[test]
    fn show_tabs_in_fullscreen_works() {
        let item = items().iter().find(|i| i.selector == "toggleFullScreenTabBar:" && i.label() == "Show Tabs in Fullscreen").unwrap();
        assert_eq!(status(item), Status::Done);
    }
}
