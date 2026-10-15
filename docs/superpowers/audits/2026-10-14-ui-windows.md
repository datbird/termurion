# Termurion vs iTerm2: windows, sheets, panels and dialogs (excluding main menu and settings panes)

I covered 56 xibs plus 9 code-built areas, 65 windows in all. The findings come from sub-agents grepping and reading code, and I did not re-verify them. I did not verify the code-built groups as thoroughly as the xibs. Where an agent did not confirm something, I say so.

## Unruled gaps in xib windows

**iTermGlobalSearch.xib** (Termurion: `src/global_find_ui.rs`)
- Missing the match-mode popup, with all 5 items:
  - "Substring — Automatic"
  - "Substring — Case Sensitive"
  - "Substring — Case Insensitive"
  - "Regex — Case Sensitive"
  - "Regex — Case Insensitive"

**CommandInfoViewController.xib** (Termurion: `src/cmdinfo.rs`, `src/app/cmdinfo_app.rs`)
- Missing "Disable showing offscreen command".

**iTermPrintAccessoryViewController.xib** (Termurion: `src/app/seltools_ui.rs`, `src/print.rs`)
- Missing the "Black and white" checkbox in the print panel. Termurion asks in a separate Yes/No/Cancel box titled "Print".

**iTermShellIntegrationWindowController.xib** (Termurion: `src/app/menu.rs`, a single MessageBox)
- Wizard missing: "Download and Run Installer", "Internet-Free Install", "Go Back", "Continue", "Skip", "Preview Command", "Installation complete!", "OK".
- The two option descriptions are also missing: "Downloads the latest version. Runs curl | bash" and "Guided manual installation. No download required."

**UKCrashReporter.xib** (Termurion: NONE; `src/crashdump.rs` writes a dump only)
- Missing the whole dialog: "Crash Reporter", the "Crash Log" and "Description" tabs, "Send", "Discard", "Remind Me Later".

**iTerm2ImportStatus ImportingWindowController.xib** (Termurion: `src/settings_export.rs` has an alert only)
- Missing the "Importing…" window with "Importing settings and data…" and "Status".

**iTermOptionalComponentDownloadWindowController.xib** (Termurion: NONE)
- Missing the "Python Runtime Installer" window with "Downloading optional components…" and "Cancel".

**iTermBuildingScriptWindowController.xib** (Termurion: busy banner in `src/app/script_runtime_ui.rs`)
- Missing the window titled "Hang tight! Installing a new Python environment…".

**iTermOnboardingWindowController.xib** (Termurion: NONE)
- Missing the "What's New" carousel: "<", ">", five page dots, pages "Companion iPhone App", "Claude Code Integration", "Screenshots", "Orchestration", "Tab Groups". The iPhone page is ruled out in 01-profile-settings.md R12; the rest is unruled.

**AITerm.xib** (Termurion: `src/ai_key_ui.rs`)
- Missing the heading "Set Up Artificial Intelligence" (Termurion uses "Manage AI API Keys").
- Missing "You may be charged by your AI provider for each token sent or received."
- One agent cited a ruling, 12e-1 in a plan file, that may cover the heading change. Unverified.

**SplitPanel.xib** (Termurion: NONE found)
- Controls: profile popup, "Split current pane XXXally", "Split", "Cancel".
- Unverified whether Termurion's split-with-profile menu covers it.

**SearchableComboView.xib** (Termurion: NONE as a widget)
- Just a search field plus a table; egui inline filters cover it. Low impact.

**iTermScriptInspector.xib** (Termurion: `src/script_inspector_ui.rs`)
- The "⨁" button is unconfirmed.

**Other unverified items**
- iTermHelpMessageViewController: no clear equivalent.
- iTermSavePanelFileFormatAccessory: "File Format:" popup and "Include timestamps". Not an accessory, because rfd has no accessory view (m13q, m11d). Whether the second small window has both controls is unconfirmed.
- Advanced Paste: Speed slider and capture-group notes unchecked.
- Large composer "AI" button unchecked.
- "Tab size in spaces:" field for NumberOfSpaces not found.

## Complete (xib windows with no missing controls)

- **Composer and status bar:** Cockpit, Minimal composer, StatusBar composer, StatusBar large composer (text only), TextPopover.
- **Instant Replay and paste:** Instant Replay (Termurion adds "Set Start" and "Set End", ruled in m10a), Paste Special view and window, MiniPaste, MinimalPaste, PasteView.
- **Pickers, popups and tools:** Open Quickly, Password Manager, Autocomplete, CommandHistory, Directories, PasteboardHistory, JobTree.
- **Find and filter:** Find, MinimalFind, MiniSearchField, MiniFilter.
- **Status bar setup:** StatusBar Setup, Setup collection item, Configure Component, and the 5 knob views.
- **Tips, tmux and scripts:** TipCard, TipWindow, TmuxDashboard, Script Console, DependencyEditor, ScriptTemplatePicker.
- **Other:** AboutWindow (native message box), SessionParameterPanel, WindowShortcutAccessory, PseudoTerminal coprocess sheet, BetterFontPicker (ruled in m9g, m13n).
- **Minor wording and shortcut changes only:** Parameter panel; "Press ⇧↩ to send." became "Press Shift+Enter to send."; Password Manager "⌘B" became "Win+B".

## Code-built UI

**Toolbelt Session Status tool**
- Missing the Snooze and Notify tooltips: "Select a session to snooze it.", "Un-snooze the selected session.", "Snooze the selected session so it sinks to the bottom of the list and dims until its status changes.", plus four Notify tooltips.
- Dynamic web tools and RTFD Notes are ruled out (m11c, m11a, m9k).

**AI chat window**
- Missing the notice "Orchestration enabled. Agent can see content of all sessions."
- Missing the palette labels "Toggle Chat List", "Toggle Thinking", "AI Speed", "Toggle Web Search". These are macOS toolbar customization, so probably not applicable.

**Triggers editor**
- Missing the row menu item "Copy Trigger as URL to Clipboard".
- Missing the "CPU Time:" histogram.
- Missing "Show visualization" and "Hide visualization".
- Missing "No additional parameters required." and the per-event help lines.
- Reduced controls:
  - Exit Code popup ("Any", "Zero (Success)", "Non-Zero (Failure)", "Specific Value…") became a free-text field.
  - "Timeout:" and "Threshold:" became "Quiet for:" and "Running for:".
  - "Fire When:" became "When:".

**Session title bar**
- Missing the lock-button tooltip ("This pane is locked. It cannot be moved, swapped, or dragged, and closing it requires confirmation. Right-click to unlock.").
- Missing the accessibility label "Pane is locked".

**Timestamp margin menu**
- NONE. Missing "Set Baseline for Relative Timestamps" and "Disable Relative Timestamps".

**Status bar**
- Missing the composer item "Show History".
- Missing the prompt "Always launch this script when iTerm2 starts?" for the RPC component.

**Annotation and Copy Mode**
- Missing the "Delete annotation" button tooltip.
- Missing the first-use notification "Copy Mode" ("Copy Mode lets you make a selection with the keyboard. Click to view the manual.").

**Tab menu**
- Complete.

**Alerts and bars (no rulings found)**
- Close and quit alert: the "Why am I being prompted?" disclosure is missing (shown inline instead).
- Paste progress bar: missing the "Sending…" label and the keystroke-queue toggle with its two hints.
- Title prompts: "Set Tab Title" and "Set Window Title" are renamed "Edit Tab Title" and "Edit Window Title", and their help text is missing.
- Missing alerts:
  - Resize prompts:
    - "A program has tried to resize the window. Allow it?" (plus the unfocused variant)
  - Triggers:
    - "This session's triggers are pretty slow. Disable them in interactive apps?" plus "View Stats"
    - "Triggers disabled in interactive apps…"
    - "(Unnamed trigger)"
    - "Error in Trigger Parameter"
  - Downloads and uploads:
    - "A file is being downloaded. Abort the download?"
    - "Upload in Progress"
    - "Where would you like to save this download?" with "Save to Downloads"
    - "tar failed…"
    - "A file transfer was aborted for exceeding its declared size."
  - Profiles and URLs:
    - "That's a gnarly JSON blob… Try it Now" / "Don't Remind Me Again"
    - "Change This Profile" / "Change All Profiles"
    - "You are about to open N profiles"
    - `iterm2:` URL dialogs ("Run Command from URL", "Run in Current Session")
  - Paste dialogs:
    - "Paste Base64-Encoded Contents"
    - "Upload and Paste Path"
    - "Paste as Text"
    - "Save to Temp File and Paste Path"
    - "How would you like to paste…"
    - "Large Paste"
  - Restore and other:
    - "Still restoring your windows…" with "Discard Saved Windows"
    - "Some windows had trouble restoring…"
    - the horizontal-scroll info bar
    - "The folder “%@” contains no images…"
    - "%@ wants to control iTerm2 using the API over SSH integration…"
- Block and button marks ("Copy block to clipboard") are ruled out as not planned (m13y2 Task 10).

## Totals

| Item | Count |
|---|---|
| Windows and areas checked | 65 (56 xibs plus 9 code-built areas) |
| Complete | 38 |
| With unruled gaps | 21 |
| With only unverified items | 6 |
| Missing controls, unruled, confirmed | about 95 |

- Whole windows with no Termurion equivalent (unruled): UKCrashReporter, Onboarding "What's New", Importing status, Python Runtime Installer, SplitPanel (unverified).
- About 70 of the missing controls are the alert and bar texts in the last group above. The rest are in xib windows, the triggers editor and the Session Status tool.
- Of the xib windows, global search, command info, print, shell integration and UKCrashReporter are the most concrete gaps.