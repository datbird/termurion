# Tandem parity roadmap

The target is every row in the three inventory files. Each milestone below names the inventory
sections it closes. A milestone ends when those rows read `have`, or carry a written reason why
Windows cannot support them. Each milestone gets its own plan in `docs/superpowers/plans/`.

| File | Rows | have | partial | missing |
|---|---|---|---|---|
| `01-profile-settings.md` | 314 | 173 | 122 | 19 |
| `02-global-settings.md` | 823 | 272 | 72 | 479 |
| `03-features-and-menus.md` | 643 | 486 | 43 | 114 |
| Total | 1,780 | 931 | 237 | 612 |

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
and the user.git* session variables for a remote host. Milestone 9j: the toolbelt, a native sidebar
panel (quads and text, no egui) drawn at the right of each window, taken from the window's own room
before the panes and the tmux size report, with iTerm2's geometry (width clamp, growth and shrink at
the screen edge, Default Toolbelt Width, Set Default Width), split view (collapse, expand, divider
drag, saved per-window proportions), the hamburger tool picker, View > Toolbelt (Show Toolbelt,
Set Default Width), window arrangements, toolbeltFont/toolbeltFontSize, and the Open Toolbelt profile
setting; and the seven tools iTerm2 ships (Actions, Command History, Named Marks, Paste History,
Profiles, Recent Directories, Snippets) with their rows, search fields, buttons, click and
double-click rules, modifiers, context menus, alerts, and undo, each keeping iTerm2's own small bugs
(the Snippets reorder index, Command History's unescaped Option-cd, the proportions prefix match, the
close glyph hiding the toolbelt everywhere). Milestone 9k: the last five built-in tools (Captured
Output, Session Status, Jobs, Notes, Codecierge) with iTerm2's rows, buttons, menus, alerts and
minimum heights; the Capture Output trigger feeding Captured Output; and View > Open Quickly
(Shift+Win+O and an optional global hotkey) as iTerm2's palette over sessions, windows, profiles,
arrangements, actions, snippets, named marks and menu items, with its minimum-subsequence scoring,
commands, preview and activation, keeping iTerm2's own bugs (Jobs' plural "processes" for one, Open
Quickly's x button closing instead of clearing, tips scoring below matches, the reversed
color-preset quotes, the double modifier symbol for a double-tap hotkey). Codecierge shows only its
onboarding panel (EnableAI stays off until milestone 12). Scripts and function invocations closed
in milestone 11c; a dynamic web tool's own drawing and color presets wait for milestone 13. Browser bookmarks stay
missing with no target milestone: Tandem has no browser profiles to read them from. Milestone 9l
closed milestone 9 (9a through 9l): the Composer, a native multi-line command editor drawn in the
pane like the find bar and the Notes editor (multiple carets, iTerm2's syntax highlighting and
colors, command history search, Tab completion of files and PATH commands, send, send each,
enqueue, Send to Advanced Paste, Copy with Styles and Copy with Control Sequences forwarding to
the pane), reachable from View > Show Composer (a floating dropdown), View > Auto Composer (which
replaces the shell prompt itself once shell integration reports one), the status bar Composer
component (a live field with a chevron history menu and an expand button, coexisting with the
dropdown), and key action 64 (Open Composer); autocomplete stays partial, since AI-sourced
suggestions wait for milestone 12. Milestone 9 is now complete. Milestone 10a closed Instant
Replay (03): a per-pane, memory-capped DVR recorder (key and diff frames, iTerm2's cadence and
byte-ring rules), Edit > Clear Instant Replay and View > Start Instant Replay (Win+Alt+K,
Win+Alt+B, key action 15), the floating scrub panel (slider, ticks, Left/Right/Shift stepping,
Export/Set Start/Set End), and Session > Log > Import Recording and Export Recording, each
keeping iTerm2's own kept bugs (the second-newest first frame, a stranded Clear during replay, a
restamped range export, tick marks fixed at panel-open time, and more, listed in
`docs/acceptance/m10a-checklist.md`). Milestone 10b closed milestone 10: the password manager (03),
opened by Win+Alt+F, a trigger, an automatic rising-edge detection of a password prompt, or a
click on the key-cursor glyph, one window per terminal window; Windows Credential Manager as the
built-in store, 1Password and LastPass through their CLIs, and an adapter protocol for KeePassXC,
Bitwarden and Keeper whose bundled adapter executables are not yet ported; the echo probe and
broadcast-safe entry, OTP/2FA autofill, and a Windows Hello (falling back to the Windows
credential prompt) re-authentication gate with its own Settings > Advanced > Password Manager
override rows; the profile's Open Password Manager Automatically and Open Password Manager
trigger action (01); and echoProbeDuration, disablePasswordManagerAnimations,
detectPasswordInput, detectPasswordInputDebounce, lastpassGroups, onePasswordAccount, and the
three Require Authentication globals (02), each keeping iTerm2's own quirks, listed in
`docs/acceptance/m10b-checklist.md` and ruled on in `docs/superpowers/rulings/m10b-rulings.txt`.
Milestone 10 is now complete. Milestone 11a closed the Python API server's core (03): a
loopback WebSocket server (127.0.0.1 and [::1], port 1912, iTerm2's protobuf-over-websocket
protocol 1.19) that starts only after a forced "Enable Python API?" warning (the Settings
checkbox, Import from iTerm2, and the Toggle Setting key action all reach it) and a consent
marker; the peer check (matching the connecting process's Windows user SID and elevation, in
place of macOS's euid check); authorization (a per-session or allow-all cookie, the allow-all
file at the same path and magic the `iterm2` pip package reads, and the "Allow Python API
Usage?" prompt); the session environment (ITERM2_COOKIE, ITERM_SESSION_ID, TERM_SESSION_ID,
ITERM_PROFILE) reaching new, split, restored and tandem-host sessions (host protocol version 3);
transactions that freeze the UI thread until they end or the owner disconnects; subscriptions
and notifications for new session, terminate session, layout change, focus change, and broadcast
change; the variable store (session, tab, window and app scopes with `user.*` values); and
ListSessions, Focus, and GetBroadcastDomains matching iTerm2's shapes, so the stock `iterm2` pip
package connects unmodified and lists Tandem's real windows, tabs and sessions; Enable Python
API, Require "Automation" permission for Python API, and setCookie (02) are done. Session/window/
tab control beyond ListSessions, setting the broadcast domain, and the variables API's own get/
set/monitor requests were built next, in milestone 11b. Every kept quirk of 11a is listed in
`docs/acceptance/m11a-checklist.md` and ruled on in `docs/superpowers/rulings/m11a-rulings.txt`.
Milestone 11b closed the rest of the Python API's request surface (03): SendText, Inject, Activate
and Close (each following iTerm2's checks, confirmations and Lock Layout rules); CreateTab and
SplitPane with custom profile properties applied to a session copy, never the saved profile;
GetBuffer, GetPrompt, ListPrompts and Screenshot (styles, code points, and the prompt's command
range while typing); the variables API's own get, set and VARIABLE_CHANGE monitoring; GetProperty
and SetProperty for window frame, fullscreen, session grid_size and buried, kept as iTerm2 reports
them (grid_size minus one, session "all" sets only the first); GetProfileProperty,
SetProfileProperty, ListProfiles and Preferences, with a fixed list of stored consents and program
paths refused for Windows safety; SavedArrangement, RestartSession, SetTabLayout and ReorderTabs;
MenuItem by XIB identifier; Selection, SetBroadcastDomains, tmux integration control (list, send,
hide/show, create), and ColorPreset (empty until milestone 13); and per-session notifications for
keystrokes (with a filter), screen updates, prompts and custom escape sequences. Every kept quirk
is listed in `docs/acceptance/m11b-checklist.md` and ruled on in
`docs/superpowers/rulings/m11b-rulings.txt`.
Milestone 11c closed the Python API's scripting layer (03): the full iTerm2 expression language,
ported to a hand-written tokenizer and precedence parser (arithmetic, comparisons, `x?` and `expr
?`, indexing, `&path` references, interpolated strings gated by `laxNilPolicyInInterpolatedStrings`,
with `??` kept as a Tandem extension); server-originated RPC registration and dispatch (namespaces,
signatures, timeouts, defaults resolved from scope, results and exceptions decoded with iTerm2's
texts); InvokeFunctionRequest for the app, a tab, a window, a session, or a method receiver; the
built-ins iTerm2 ships ported (`count`, `urlEncode`, `with_alpha`, `alert`, `get_string`, `focus`,
`paste`, the clippings functions, `get_profile_property`, `set_status`, the unread-count setter,
`move_tab_to_window`, and the session/tab/window methods, including `run_tmux_command` and
`run_coprocess`), the rest answering as unregistered functions until ported; every caller iTerm2
wires a function call into (a key or pointer binding's Invoke Script Function, a trigger's Invoke
Script Function with `matches` and the capture variables, the Call Script Function status bar
button, Open Quickly invocations, and `iterm2.async_invoke_function` itself) with iTerm2's scopes,
origins and timeouts, and a silenceable, mergeable Function Call Failed alert on error; a script
status bar component (knobs, cadence, click handler, an unread-count badge, an error ladybug and
alert, `async_open_popover` as a text popover); a Title Function provider chosen in Settings >
Profiles > General > Title; a context menu provider item; and RegisterTool, which answers iTerm2's
statuses and lists a dynamic tool in the Toolbelt menu, though it draws nothing (web views wait for
milestone 13, which hosts WebView2, flagged to Robert). A per-pane title bar menu adding context menu
provider items, the Title Function popup's title preview, and a tmux Custom title's prefix are
documented gaps for milestone 13. Every kept quirk is listed in
`docs/acceptance/m11c-checklist.md` and ruled on in `docs/superpowers/rulings/m11c-rulings.txt`.
Milestone 11d closed the Scripts menu (03) and the scripting settings (02): the Scripts folder
(`%APPDATA%\Tandem\Scripts`, or the Custom Python API Scripts Folder when it exists, has no spaces
and is not a network path) scanned into the menu, rescanned on change, with iTerm2's slow-scan
question; Python from Tandem's own uv (a pinned GitHub release checked by SHA-256) or an installed
Python 3, every download asked first with its source named (github.com/astral-sh/uv,
python-build-standalone, pypi.org) and never from AutoLaunch; Install Python Runtime and Check for
Updated Runtime; per-script environments built aside and swapped in; scripts launched hidden in a
Job Object with a single-use cookie and key, a check mark while a basic script runs, Alt to reveal,
and "Script Failed" on an abnormal end; the Script Console (logs, calls, filter, Terminate,
Restart, Clear Terminated) and the Scripting Inspector, reached from the notification, a function
error's "Reveal in Script Console" and the status bar's Launch Script recovery; AutoLaunch while the
API is on; New Python Script with iTerm2's templates; Open Python REPL; Import (unsigned zips
checked before extraction, Replace with a backup) and Export (iTerm2's unsigned zip); Manage
Dependencies; and Open Quickly's script items. The Automatic Profile Switching debug log moved into
the Script Console. pythonRuntimeUsesUV, the three runtime manifest URLs and setIT2AppPath are stored
and inert (no macOS runtime, no AppleScript target); signed `.its` archives are refused. Every kept
quirk is listed in `docs/acceptance/m11d-checklist.md` and ruled on in
`docs/superpowers/rulings/m11d-rulings.txt`.

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
