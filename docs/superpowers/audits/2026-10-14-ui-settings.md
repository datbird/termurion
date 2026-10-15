# Termurion Settings UI vs iTerm2 audit (read-only)

**Method.** I dumped controls from `PreferencePanel.xib` and the sheet xibs, in visual order. I mapped the per-pane `defineControl` keys to `schema/*.toml`. Then I read `settings_ui.rs`, `settings_keys.rs`, `settings_advanced.rs` and the other `settings_*.rs` files to see what is actually drawn.

**Structure.** The Profiles and global pages are generic schema loops. `control()` and `global_control()` draw whatever the schema row's kind implies, plus a handful of special-cased rows.

- A `List` or `Dict` row with no special case draws only "`<label>: N item(s)`" with the tooltip "This editor arrives with its feature's milestone."
- A global `Int` row draws nothing unless it is one of the two margin keys.
- `Int` profile rows draw a bare drag number.

Key coverage is complete: every `KEY_*` / `kPreferenceKey*` read by iTerm2's Profiles and Preferences view controllers is in the schema or marked n/a. The gaps are all in what the UI draws.

**Labels.** Most schema labels are paraphrases, not iTerm2's wording. Examples: "Alt Key Sends (Left)" for "Left option (⌥) key:", "Cursor smooth slide" for "Smooth-typing animation", and "Place prompt at first column" for "Insert newline before start of command prompt if needed". I did not itemize this drift below.

## Per pane

**Global > General** (Startup, Closing, Magic, AI, Software Update, Selection, Window, Settings, tmux, Experimental)
- Missing, no control drawn:
  - "Memory usage per session (in MB)". `IRMemory` is bounded 0..1000 but draws nothing. A test (`settings_keys.rs:4360`) asserts that it draws nothing, and nothing rules that out.
  - "Pause a pane if catching up would take longer than (seconds):". `TmuxPauseModeAgeLimit` has no control.
  - "Allow sending of clipboard contents?" (Ask Each Time | Never | Always). `PhonyAllowSendingClipboardContents` has no schema row or control, yet 02:173 says "have".
  - "GPU rendering". 02:150 says "have", noting Termurion always uses GPU, but there is no control.
- Different control: "Window restoration policy:" is a 3-item popup in iTerm2. Termurion has two checkboxes ("Open default window arrangement", "Only restore hotkey window") and no "Use System Window Restoration Setting" item.
- On a different pane:
  - "New window placement:" (4 radios) is under Appearance > Windows. iTerm2 has it in General > Window.
  - "Double-click performs smart selection" and "Automatically enter copy mode on Shift+Arrow key with selection" are under Pointer > Selection. iTerm2 has them in General > Selection.
- Ruled out:
  - "Native full screen windows" (02:133 n/a).
  - AI "Plugin:" section, replaced by a consent dialog (`m12a-rulings.txt:1`, ruling 1).
- The AI sub-tabs (General, Prompts, Features) otherwise match.

**Global > Appearance**
- Missing: "New tabs open at end of tab bar". 02:227 says "have", but `AddNewTabAtEndOfTabs` is not in the schema and has no control.
- Ruled out (all n/a in the parity doc):
  - "Show proxy icon in window title bar" (02:211).
  - "Hide menu item icons" (02:196).
- The other General, Windows, Tabs, Panes and Dimming controls are present.

**Global > Keys** — complete. Key Bindings, Leader, Navigation Shortcuts, Hotkey, Remap Modifiers (with Reset to Defaults) and System are all present. "Repair Broken Bindings" is ruled out (02:289, nothing to repair).

**Global > Pointer**
- Missing: "Load Defaults" on the Bindings tab. It appears nowhere in src or docs.
- Ruled out: trackpad gestures (three-finger tap and swipes, Force Touch), noted macOS only in the pointer editor.

**Global > Shortcuts** (Actions, Snippets) — present. I checked only the Edit Snippet sheet strings, not the full controls.

**Global > Arrangements** — complete (list, "Set Default", delete, preview). Workgroups is a separate page.

**Global > Advanced** — complete. iTerm2 has about 505 advanced settings and Termurion has 426 rows. The ~80 absent ones are n/a in 02-global-settings.md (macOS, Metal, companion and similar). Two have no doc row at all: `enableSecureKeyboardEntryAutomatically` and `browserBundleID`.

**Settings window chrome**
- Missing: the Preferences-wide search field and its results window (`iTermPreferencesSearchEngineResultsWindowController`). Termurion has only the "Search Options" menu with "Show indicators for non-default values".

**Profiles list** (`ProfilesWindow.xib`)
- Missing: the "Other Actions…" popup items.
  - "Bulk Copy from Selected Profile…" and its sheet (checkboxes Colors, Text, Window, Terminal, Web, Keys, Session, Advanced; "To these profiles:"; Copy).
  - "Save Profile as JSON…".
  - "Save All Profiles as JSON…".
  - "Import JSON Profiles…".
- None of the four is in the parity docs or rulings. Termurion has "+", "Duplicate", "-", "Set as Default" and "Import from iTerm2...".

**Profiles > General**
- Missing:
  - "Tags:" editor. It is a placeholder "Tags: N item(s)", although 01 says "have".
  - Title "Components" chooser. "Title components" is a raw integer drag value, and the popup only offers "Built-in components" plus script providers.
  - "Enable Tall Tab Bar" with "Subtitle support requires a taller tab bar."
- "Badge:" Configure… and the Advanced working-directory sheet are flattened into inline rows (a layout difference, not missing).

**Profiles > Colors** — complete. Light/Dark, Color Presets, the hue wheel and all wells and sliders are present.

**Profiles > Text**
- Missing: the Font panel.
  - "Font:" and "Non-ASCII Font:" are plain text fields. There is no picker, size or weight, and the "(recommended for this font)" and "⚠️ This may reduce performance" hints are absent.
  - `FONT_KEYS` (`settings_ui.rs:1780`) disables these rows, plus "Use a different font for non-ASCII text" and both spacings, for every non-default profile: "Fonts follow the default profile for now; per-profile fonts come later". 01:114 says "have" with no note.
- The rest (cursor, rendering, Unicode, "Manage Special Exceptions…") is present.

**Profiles > Window**
- Missing: the "Screen:" popup (No Preference | Screen with Cursor | monitors). It is a raw integer drag value, although 01:174 describes the popup.

**Profiles > Terminal** — complete. "Report terminal type:" is a plain text field instead of iTerm2's preset combo box (drift only).

**Profiles > Session**
- Missing: the note "⚠️ You may want a trigger to stop buffering" under "Buffer input by default".
- "Password manager:" is a popup in the xib with no `defineControl` wiring in source, so I did not count it.
- "Filter Alerts" and "Configure Status Bar" are present.

**Profiles > Keys** — complete. "Delete sends ^H", Presets, the key table and the hotkey sheet are present. The Touch Bar rows are macOS only.

**Profiles > Advanced**
- Missing: the "Snippets Filter:" token field. It is a placeholder "N item(s)", although 01:316 says "have".
- Present: Triggers, Smart Selection, Semantic History, Automatic Profile Switching.

**Profiles > Web** (not a tab in Termurion)
- The tab does not exist. "Use /dev/null mode" and "Enable Instant Replay" appear on General for Browser profiles.
- Ruled out: "Page Zoom:" (01 "Browser Zoom": written by the zoom commands, not shown as a control).
- Ruled out: "Download Browser Plugin" and "Browser Plugin Installed" (WebView2 replaces it, m13x, 02:1003 and 02:1057).

**Sheet: Smart Selection** (inline editor, `settings_advanced.rs`)
- Missing:
  - "Load Defaults".
  - "Syntax Help".
  - "Open Regular Expression Visualizer".
  - The test playground ("Click on text in playground to test rules").
  - "Log debug info to Console.app".
- The "Name" column and field are labelled "Notes".

**Sheet: Semantic History** — present. It has the same 7 actions and a text field.

**Sheet: Triggers** — present, with "Instant" and "Enabled". "Copy to Profile" exists only as the Edit Session copy button.

**Sheet: Status Bar setup** — complete.

**Sheet: Hotkey window** — complete (Double-tap, Pin, Reopen, Animate, Floating, Dock click radios, Additional Hotkeys…).

**Sheet: Special Exceptions** — complete.

**Sheet: SSH configuration** — present. The strings exist in `ssh_config.rs`; I did not check the control layout.

**Sheet: Advanced GPU settings** — flattened into inline General rows.

**Sheet: Key mapping editor** — present. I only spot-checked it.

**Sheet: Edit Snippet** — present. The strings are in `settings_shortcuts.rs`.

## Totals

- **Panes checked:** 18 (7 global pages, the Profiles list, 9 profile tabs including Web, and the settings window chrome). Another 12 sheets and popovers were checked at lighter depth.
- **Complete:** 7 (Global Keys, Global Arrangements, Global Advanced, Global Shortcuts, Profile Colors, Profile Terminal, Profile Keys).
- **With unruled gaps:** 11 (Global General, Appearance, Pointer; window chrome; Profiles list; Profile General, Text, Window, Session, Advanced; Web, which is only a placement difference). The Smart Selection sheet also has gaps.
- **Controls missing:** 23.
  - Global General 3.
  - Appearance 1.
  - Pointer 1.
  - Preferences search 1.
  - Profiles list 4.
  - Profile General 3.
  - Text 2 (the Font and Non-ASCII Font panels).
  - Window 1.
  - Session 1.
  - Advanced 1.
  - Smart Selection 5.
- **Also off:**
  - 3 controls on the wrong pane.
  - 1 restoration popup replaced by checkboxes.
  - 5 font rows locked for non-default profiles.
- **"Have" in the parity docs but no UI control:** `PhonyAllowSendingClipboardContents`, `AddNewTabAtEndOfTabs`, `UseMetal`, Tags, Snippets Filter, Screen (a raw integer, not the described popup), the Title Components chooser (a raw integer), per-profile fonts, and the global `IRMemory` and `TmuxPauseModeAgeLimit`.

Everything is in `src/` (`settings_ui.rs`, `settings_keys.rs`, `settings_advanced.rs`) and `docs/parity/`. The iTerm2 xibs are under `iterm2-ref/sources/Settings/Base.lproj/`.