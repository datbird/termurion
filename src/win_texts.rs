//! Guard for package W of the UI parity sweep: each window and alert text below is iTerm2's, word
//! for word, with the Mac words mapped (iTerm2 to Termurion, Option to Alt, Finder to Explorer).
//! Every row names where iTerm2 says it (file:line under its `sources` folder) and the constant
//! Termurion shows. A text changed here without changing the source it came from fails.

#[cfg(test)]
mod tests {
    use crate::{announce, bgimage, copymode, crash_report, nontext_paste, orch, pane_title, restore_state, settings_export, settings_triggers, shell_wizard, status_tool, trigger_file, whats_new, wintitle};

    /// (iTerm2 file:line, iTerm2's text with the Mac words mapped, Termurion's constant).
    fn rows() -> Vec<(&'static str, &'static str, &'static str)> {
        vec![
            // Windows.
            ("ThirdParty/UKCrashReporter/Base.lproj/UKCrashReporter.xib:379", "Remind Me Later", crash_report::REMIND),
            ("ThirdParty/UKCrashReporter/Base.lproj/UKCrashReporter.xib", "Crash Reporter", crash_report::TITLE),
            ("iTerm2ImportStatus/Base.lproj/ImportingWindowController.xib:18", "Importing\u{2026}", crate::panel_views::IMPORTING_TITLE),
            ("iTerm2ImportStatus/Base.lproj/ImportingWindowController.xib:43", "Importing settings and data\u{2026}", crate::panel_views::IMPORTING_LABEL),
            ("API/Base.lproj/iTermOptionalComponentDownloadWindowController.xib:21", "Python Runtime Installer", crate::panel_views::RUNTIME_TITLE),
            ("API/Base.lproj/iTermOptionalComponentDownloadWindowController.xib:33", "Downloading optional components\u{2026}", crate::panel_views::RUNTIME_LABEL),
            ("API/Base.lproj/iTermBuildingScriptWindowController.xib:27", "Hang tight! Installing a new Python environment\u{2026}", crate::panel_views::BUILDING_LABEL),
            ("SplitPanel/SplitPanel.m:30", "Split current pane vertically with profile:", crate::panel_views::SPLIT_VERTICAL_PROMPT),
            ("SplitPanel/SplitPanel.m:32", "Split current pane horizontally with profile:", crate::panel_views::SPLIT_HORIZONTAL_PROMPT),
            ("Settings/ImportExport.swift:482", "Extracting Archive", settings_export::STATUS_EXTRACTING),
            ("ShellIntegrationInstaller/Base.lproj/iTermShellIntegrationWindowController.xib:70", "Also install Termurion Utilities", shell_wizard::UTILITIES_BOX),
            ("ShellIntegrationInstaller/Base.lproj/iTermShellIntegrationWindowController.xib:147", "Download and Run Installer", shell_wizard::DOWNLOAD_BUTTON),
            ("ShellIntegrationInstaller/Base.lproj/iTermShellIntegrationWindowController.xib:161", "Internet-Free Install", shell_wizard::OFFLINE_BUTTON),
            ("ShellIntegrationInstaller/Base.lproj/iTermShellIntegrationWindowController.xib:398", "Installation complete!", shell_wizard::FINISHED_TITLE),
            ("TerminalView/Base.lproj/CommandInfoViewController.xib:404", "Disable showing offscreen command", crate::cmdinfo::DISABLE_OFFSCREEN),
            ("AppKit/iTermApplicationDelegate.m:1055", "Why am I being prompted?", "Why am I being prompted?"),
            ("Printing/Base.lproj/iTermPrintAccessoryViewController.xib:23", "Black and white", crate::platform::print::BLACK_AND_WHITE_LABEL),
            ("AITerm/Base.lproj/AITerm.xib:32", "Set Up Artificial Intelligence", crate::settings_ai::REGISTRATION_HEADING),
            ("Settings/iTermAddTriggerViewController.m:439", "CPU Time:", settings_triggers::CPU_TIME_LABEL),
            ("Settings/TriggerController.m:1123", "Copy Trigger as URL to Clipboard", trigger_file::COPY_URL_TITLE),
            ("Pasting/Base.lproj/NumberOfSpacesAccessoryView.xib:25", "Tab size in spaces:", crate::prompt_ui::TAB_SIZE_LABEL),
            // Alerts and announcements.
            ("PTYSession/PTYSession.m:15889", "A program has tried to resize the window. Allow it?", announce::RESIZE_TEXT),
            ("PTYSession/PTYSession.m:15927", "A program has tried to resize the window while this session was not active. Allow it?", announce::RESIZE_UNFOCUSED_TEXT),
            ("PTYSession/iTermNaggingController.m:896", "That's a gnarly JSON blob you've got there! Termurion can replace this hard-to-read selection with a pretty-printed value.", announce::JSON_TEXT),
            ("Pasting/iTermNonTextPasteHelper.swift:636", "Large Paste", nontext_paste::LARGE_PASTE_HEADING),
            ("Pasting/iTermNonTextPasteHelper.swift:74", "Paste Base64-Encoded Archive (tar.gz)", nontext_paste::FileAction::Base64Archive.label()),
            ("Pasting/iTermNonTextPasteHelper.swift:73", "Paste Base64-Encoded Contents", nontext_paste::FileAction::Base64.label()),
            ("Pasting/iTermNonTextPasteHelper.swift:342", "Save to Temp File and Paste Path", nontext_paste::ImageAction::SaveTemp.label()),
            ("StateRestoration/iTermRestorableStateDriver.m:91", "Some windows had trouble restoring last time Termurion launched. Try again?", restore_state::TROUBLE_TEXT),
            ("StateRestoration/iTermRestorableStateDriver.m:96", "Restore Windows?", restore_state::TROUBLE_HEADING),
            ("Settings/ProfilesWindowPreferencesViewController.m:875", "Empty Folder", bgimage::EMPTY_FOLDER_HEADING),
            ("TerminalView/PseudoTerminal.m:3835", "Set Window Title", wintitle::SET_WINDOW_TITLE),
            ("CopyMode/iTermCopyModeHandler.m:502", "Copy Mode lets you make a selection with the keyboard. Click to view the manual.", copymode::NOTIFICATION_DETAIL),
            ("TerminalView/SessionTitleView.m:99", "This pane is locked. It cannot be moved, swapped, or dragged, and closing it requires confirmation. Right-click to unlock.", pane_title::LOCK_TIP),
            ("AITerm/ChatViewController.swift:4086", "Orchestration disabled.", orch::DISABLED_NOTICE),
            ("StatusBar/Components/iTermStatusBarRPCProvidedTextComponent.m:407", "Always launch this script when Termurion starts?", crate::script_statusbar::AUTOLAUNCH_HEADING),
            ("StatusBar/Components/iTermStatusBarRPCProvidedTextComponent.m:402", "This will move the script into the AutoLaunch folder.", crate::script_statusbar::AUTOLAUNCH_TEXT),
            ("StatusBar/Components/iTermStatusBarRPCProvidedTextComponent.m:540", "Launch Script", crate::script_statusbar::LAUNCH_SCRIPT),
            ("Settings/iTermAddTriggerViewController.m:628", "Show visualization", settings_triggers::SHOW_VISUALIZATION),
            ("Settings/iTermAddTriggerViewController.m:1487", "Hide visualization", settings_triggers::HIDE_VISUALIZATION),
            ("Pasting/Base.lproj/PasteView.xib:25", "Pasting\u{2026}", crate::paste_view::PASTING),
            ("Pasting/PasteViewController.m:97", "Sending\u{2026}", crate::paste_view::SENDING),
            ("Pasting/PasteViewController.m:139", "Send keystrokes to terminal", crate::paste_view::KEYSTROKES_ACCESSIBILITY),
            ("Pasting/PasteViewController.m:143", "Type directly to the terminal (for example to answer a password prompt) instead of queueing your keystrokes until the paste finishes.", crate::paste_view::KEYSTROKES_TOOLTIP),
            ("Pasting/PasteViewController.m:218", "Typing is queued while pasting. Click the keyboard to toggle queueing.", crate::paste_view::QUEUED_HINT),
            ("Pasting/iTermNonTextPasteHelper.swift", "Paste Failed", nontext_paste::FAILED_HEADING),
            ("Pasting/iTermNonTextPasteHelper.swift", "The copied file no longer exists.", nontext_paste::COPIED_FILE_MISSING),
        ]
    }

    #[test]
    fn window_and_alert_texts_are_iterm2s() {
        for (at, want, got) in rows() {
            assert_eq!(got, want, "{at}");
        }
    }

    /// The row's text is never one of the Mac words iTerm2 uses for what is Windows here.
    /// The statistics table has iTerm2's column titles in iTerm2's order (`iTermHistogram.swift:10-30`).
    #[test]
    fn trigger_stats_table_has_iterm2_columns() {
        let mut t = crate::perf_stats::Tabular::time();
        let mut h = crate::perf_stats::Histogram::default();
        h.add(5.0);
        t.add_histogram(&h, 2, "\u{b5}s", "label");
        let head = t.text();
        let head = head.lines().next().unwrap().split_whitespace().collect::<Vec<_>>().join(" ");
        assert_eq!(head, "Min Time Distribution Max Time # Samples Mean Time P50 P95 Total Time");
    }

    #[test]
    fn no_mac_words_remain_in_these_texts() {
        for (at, _, got) in rows() {
            for bad in ["iTerm2", "Finder", "Option key", "\u{2014}"] {
                assert!(!got.contains(bad), "{at}: {got:?} still says {bad}");
            }
        }
        // Also the longer texts built with a function.
        assert_eq!(nontext_paste::large_paste_text(10001), "OK to paste 10001 bytes of base64-encoded data?");
        assert!(status_tool::SNOOZE_TIP_SNOOZE.starts_with("Snooze the selected session"));
        assert!(!crash_report::EXPLANATION.contains("Apple"));
        assert!(whats_new::PAGES.len() == 4, "the Companion page is ruled out (R12)");
    }
}
