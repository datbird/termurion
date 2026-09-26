# Tandem parity roadmap

The target is every row in the three inventory files. Each milestone below names the inventory
sections it closes. A milestone ends when those rows read `have`, or carry a written reason why
Windows cannot support them. Each milestone gets its own plan in `docs/superpowers/plans/`.

| File | Rows | have | partial | missing |
|---|---|---|---|---|
| `01-profile-settings.md` | 314 | 168 | 125 | 21 |
| `02-global-settings.md` | 820 | 237 | 71 | 512 |
| `03-features-and-menus.md` | 643 | 396 | 35 | 212 |
| Total | 1,777 | 801 | 231 | 745 |

Done: milestone 0 (ConPTY and mouse probe), milestone 1 (terminal basics: tabs, splits,
renderer, keys, selection, copy and paste), and milestone 2 (profiles: every profile setting is
saved and editable in the settings window; colors, fonts, spacing, cursor, scrollback, window size,
command, and directory take effect; iTerm2 import; the Profiles window), and milestone 3 (3a:
iTerm2's menu bar and right-click menu, about 60 commands, Win as Cmd; 3b: global and profile key
bindings with all 77 action codes, the Leader, presets, pointer bindings with all 32 actions,
modifier remapping, number shortcuts, and the profile Keys options). Actions and settings that
need a later feature are stored and name their milestone. Milestone 4a: shell integration (OSC 133, OSC 7,
the 1337 shell keys, a PowerShell script, iTerm2's installer for bash and zsh), prompt, manual,
and named marks on their rows (a small alacritty_terminal patch), the mark commands and alerts,
mark indicators, and Reuse previous session's directory. Milestone 4b: the find bar (five modes,
search as you type, match highlights, the count), Find Globally, Filter, Zoom In on Selection, Find
URLs and the other labeled results, Smart Selection (rules, precision, quadruple click, actions),
Semantic History (Win-click on files and URLs, all seven actions but coprocesses), OSC 8 links, Open
Selection, and the Smart Selection and Semantic History editors. Milestone 4c: Copy Mode (and key
actions 45 to 48 and 73), Replace Selection, Clear to Start of Selection, soft boundaries, folds,
annotations (and OSC 1337 AddAnnotation), the session note, per-line timestamps, the cursor
guide, Find Cursor, and the Named Marks menu (two more alacritty_terminal patches: row time,
fold, and note slots, and Grid::splice). Milestone 5a: inline images (OSC 1337 File and
multipart files with iTerm2's sizing, downloads to the Downloads folder after asking, sixel, the
kitty graphics protocol, animated GIFs), drawn by a textured-quad pipeline from image tiles kept
in the grid cells (alacritty_terminal patches 7 and 8). Milestone 5b: the keyboard protocols (modifyOtherKeys
1 and 2, CSI u, raw key reporting, the kitty keyboard protocol with all five flags and key
releases), the Session > Terminal State key modes, Literal Controls, and Emulation Level, the
profile's two key reporting settings, and the reports iTerm2 answers (DA1 to DA3 by level,
XTVERSION, `CSI 1337 n`, DECRQSS, XTGETTCAP, synchronized output, the dark-mode query and mode
2031; alacritty_terminal patch 9 and patch 8 revised). Milestone 5c: the clipboard sequences (OSC 52
write and read with iTerm2's policy, OSC 1337 Copy and CopyToClipboard), the escape-sequence
trust gate, iTerm2's announcement bar in the pane, the small OSC 1337 keys (CursorShape,
ClearScrollback, HighlightCursorLine, RequestAttention, the cell size and window and screen
frames, UnicodeVersion, SetColors, Capabilities, key labels, StealFocus), OSC 133 E to H, OSC
21337 tab status, Copy with Styles, Copy with Control Sequences, and Buffer Input. Milestone 5d:
session-local profiles (iTerm2's divorced sessions), profile background images (the four modes,
Blend, folder rotation, per-pane or shared), the OSC 1337 keys that ask first (SetProfile,
SetProfileProperty, SetBackgroundImageFile, OpenURL, ReportVariable, RequestUpload), and Print and
Page Setup. Milestone 5e: Make Screenshot (a frozen copy of the pane, the panel with minimap,
line range, Redact and Highlight, blur or solid redaction, Copy, Save, Save As, 30000 px parts),
and OSC 1337 Block, UpdateBlock, and Button (alacritty_terminal patches 10 and 11). Milestone 5f
closed milestone 5: portholes (a document shown rendered in place of grid rows: markdown with
pulldown-cmark, JSON with key order kept, code highlighted by syntect in iTerm2's theme classes;
made by Block `render=1`, File= text documents, and Render Selection Natively; close, the language
list, Wide, selection and copy inside, Find marks, link clicks that ask; alacritty_terminal patch
12). Milestone 6 is split in three. Milestone 6a: triggers (the profile's list in iTerm2's stored form,
checked on each finished logical line and every 0.5 s for Instant triggers, plain and interpolated
parameters, every action iTerm2 lists with the later-milestone ones marked, the Triggers editor,
and Session > Triggers; alacritty_terminal patch 13). Milestone 6b: the bell settings and the
offer to silence a busy bell, Windows notifications with iTerm2's texts and filters (a click
reveals the session), OSC 9 and OSC 1337 Notification, idle and new-output tracking with tab
indicators, Notify on Status Change, the idle code, event triggers, and badges. Milestone 6c
closed milestone 6: the foreground job (the deepest console program under the shell, from the
process tree), the Job title components, Job Started and Job Ended triggers and the trigger Job
field, Prompt Before Closing with Jobs to Ignore, coprocesses (Session > Run Coprocess and Stop
Coprocess, the triggers, key action, Smart Selection, Semantic History, and file-drop starts,
silent coprocesses, the indicator, the stderr banner), and Automatic Profile Switching (rules,
scores, the profile stack, the rule editor, and a debug log). Milestone 7 is split in six (7a
many windows, 7b closing and startup, 7c arrangements, 7d restoration, 7e hotkey windows, 7f
broadcast input). Milestone 7a: many windows (numbers, New Window and New Window with Current
Profile, the Profiles menu list, switching by number and Next/Previous Window, Move Tab and Move
Session to a new window, Merge All Windows, Duplicate Window), every window style with iTerm2's
frame rules, new-window placement, the Screen setting, Lock Size and Lock Layout, Resize Window,
Edit Window and Tab Title, and Dim background windows. Milestone 7b: the session, tab, and
window close questions and the quit question with iTerm2's rules and texts, Undo Close, the
Closing and Startup settings. Milestone 7c: window arrangements (save, restore as windows or
tabs, files in iTerm2's plist form with a trust question for downloaded ones, the Settings
Arrangements page, Change Profile in Arrangement, the default arrangement at startup, and
Duplicate Window rebuilt from a snapshot). Milestone 9g: the status bar framework (the profile's Status Bar
Layout in iTerm2's dictionary form, one bar per window or per pane at the top or bottom, the
Stable and Tightly Packed layout algorithms, component bands, separators, the edge separator,
auto-rainbow, the placeholder, the Spring and the Fixed-size Spacer, the bar's right-click menu)
and iTerm2's setup window (the component menu, Active Components with drag, drop, reorder, and
delete, Configure Component for every knob type, Auto-Rainbow, Advanced, live apply with Cancel
restoring the layout); the content components come in milestone 9h. Milestone 9h: the status bar's
content components (Clock with TR 35 and localized formats, Host Name, User Name, Job Name, Current
Directory with its click menu, Interpolated String, Custom Action, the Actions, Snippets and Triggers
menus, Search Tool and Filter Tool bound to find and Filter, the Progress Indicator for slow pastes,
Call Script Function and the Composer as stubs until milestone 11 and the Composer milestone), each
text component shorter by iTerm2's width variants as the bar narrows, the temporary search, filter
and progress components, the tmux status bar (status-left and status-right on the 8f option
monitors), and Save lines to scrollback with status bar present (alacritty_terminal patch 14).
Milestone 9i: the four graph components (Battery Level, CPU Utilization, Memory
Utilization, Network Throughput) with Windows samplers (GetSystemPowerStatus, GetSystemTimes,
GlobalMemoryStatusEx, GetIfTable2) behind iTerm2's math, a 60-sample sparkline drawn as pixel-column
quads with the animation knob, and iTerm2's kept quirks (Battery's charging-icon reserve, CPU's
since-boot first sample, Network's unclamped rates); and the git state component, polled off the
UI thread with the git CLI standing in for libgit2 (porcelain v2 status, rev-parse for the
repo-operation files), hardened against hostile repo config and UNC paths anywhere on the way, with
the timeout warning, iTerm2's click menu (Commit, Add & Commit, Stash, Log, Push, Pull, Check out,
Show Debug Info) run through single-use windows and a small tandem-git.exe wrapper, a Log popover,
and the user.git* session variables for a remote host.

## Order

The order follows dependencies first. Almost everything reads a profile setting, runs through
the command system, or shows up in a menu, so those come first.

| # | Milestone | Closes (inventory sections) | Depends on |
|---|---|---|---|
| 2 | Profiles and the settings window | 01: all tabs (the data model and the settings that change drawing, text, cursor, window, terminal, session). 02: Appearance, Profiles window, Settings sub-tab. iTerm2 plist and Dynamic Profiles import. | 1 |
| 3 | Commands, menus, key and pointer bindings | 03 section 1 (every menu command). 02: Keys (Key Bindings, Navigation Shortcuts, Remap Modifiers, all 75 action types), Pointer (all 32 actions). A menu bar and a right-click menu. | 2 |
| 4 | Find, marks, and shell integration | 03: find, captured output, annotations, marks and navigation, shell integration. 01: Smart Selection, Semantic History. OSC 133, 1337 (CurrentDir, SetMark, SetUserVar, RemoteHost), OSC 7, OSC 8 links. | 3 |
| 5 | Inline images and the remaining escape codes | 03 section 2: OSC 1337 File, sixel, the kitty graphics and keyboard protocols, OSC 52, synchronized updates, every other listed sequence. | 4 |
| 6 | Triggers, badges, notifications, automatic profile switching | 01: Triggers (every action), Session (bell, alerts), badge settings. 03: triggers subsystem. | 4 |
| 7 | Windows, hotkey windows, arrangements, restoration, broadcast input | 02: Hotkey, Arrangements, Window, Closing, Startup. 03: hotkey windows, arrangements, session restoration, broadcast input modes. | 3 |
| 8 | tmux integration | 03: tmux integration. 02: General > tmux, Advanced > Tmux Integration. | 3, 7 |
| 9 | Status bar, toolbelt, Composer, paste tools, open quickly, snippets | 03: status bar (every component), toolbelt (every tool), Composer, paste special and paste history, open quickly. | 3, 4 |
| 10 | Instant Replay and the password manager | 03: Instant Replay, password manager (Windows Credential Manager behind it). | 3 |
| 11 | Python API and utilities | 03: Python API (the same protobuf API over a local socket), utilities scripts (Windows builds of imgcat, it2dl, it2copy and the rest). 02: Magic, Advanced > Scripting. | most of the above |
| 12 | AI features | 02: AI, AI > Prompts, AI > Features. 03: AI and chat features. | 3, 9 |
| 13 | Advanced settings sweep and parity audit | 02: every Advanced row not closed earlier. A final pass over all three files. | all |
| 14 | SSH integration | 03: iTerm2's SSH integration (it2ssh, the conductor and its remote scripts, file download), Shell > ssh; OSC 1337 it2ssh, SendConductor, EndSSH, wrap, and Env's use. | 5 |

## Windows notes that apply everywhere

- Cmd is the Win key and Option is Alt (spec 3.4, Robert's choice in milestone 3a). A keyboard
  hook takes Win chords while Tandem is in front; chords Tandem does not use stay with Windows.
- macOS-only rows (Touch Bar, Services menu, Spaces, Secure Keyboard Entry, Sparkle updates) get a
  Windows equivalent where one exists, or a written reason in the inventory.
- Each milestone updates the status column of the rows it closes, so the table above stays true.
