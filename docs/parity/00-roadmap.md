# Tandem parity roadmap

The target is every row in the three inventory files. Each milestone below names the inventory
sections it closes. A milestone ends when those rows read `have`, or carry a written reason why
Windows cannot support them. Each milestone gets its own plan in `docs/superpowers/plans/`.

| File | Rows | have | partial | missing |
|---|---|---|---|---|
| `01-profile-settings.md` | 314 | 76 | 172 | 66 |
| `02-global-settings.md` | 820 | 114 | 59 | 647 |
| `03-features-and-menus.md` | 636 | 197 | 25 | 414 |
| Total | 1,770 | 387 | 256 | 1,127 |

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
21337 tab status, Copy with Styles, Copy with Control Sequences, and Buffer Input. Milestone 5d
(next) closes the rest of milestone 5: Print and Page Setup, Make Screenshot, the OSC 1337 keys
that ask first (SetProfile, SetProfileProperty, SetBackgroundImageFile, OpenURL, ReportVariable,
RequestUpload), and Block, UpdateBlock, and Button.

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
| 14 | SSH integration | 03: iTerm2's SSH integration (it2ssh, the conductor and its remote scripts, file download), Shell > ssh; OSC 1337 it2ssh, SendConductor, EndSSH, wrap, and Env's use. | 5 |
| 13 | Advanced settings sweep and parity audit | 02: every Advanced row not closed earlier. A final pass over all three files. | all |

## Windows notes that apply everywhere

- Cmd is the Win key and Option is Alt (spec 3.4, Robert's choice in milestone 3a). A keyboard
  hook takes Win chords while Tandem is in front; chords Tandem does not use stay with Windows.
- macOS-only rows (Touch Bar, Services menu, Spaces, Secure Keyboard Entry, Sparkle updates) get a
  Windows equivalent where one exists, or a written reason in the inventory.
- Each milestone updates the status column of the rows it closes, so the table above stays true.
