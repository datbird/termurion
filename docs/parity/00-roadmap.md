# Tandem parity roadmap

The target is every row in the three inventory files. Each milestone below names the inventory
sections it closes. A milestone ends when those rows read `have`, or carry a written reason why
Windows cannot support them. Each milestone gets its own plan in `docs/superpowers/plans/`.

| File | Rows | have | partial | missing |
|---|---|---|---|---|
| `01-profile-settings.md` | 314 | 174 | 121 | 19 |
| `02-global-settings.md` | 823 | 382 | 80 | 361 |
| `03-features-and-menus.md` | 645 | 520 | 48 | 77 |
| Total | 1,782 | 1,076 | 249 | 457 |

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
Milestone 11e closed the utilities (03, 02): one console program, `tandem-utils.exe`, holds
Windows builds of imgcat, imgls, it2cat, it2dl, it2ul, it2copy, it2setcolor, it2check,
it2attention, it2universion, it2getvar, it2profile, it2setkeylabel, it2git, it2tip and it2api,
each writing the bytes, texts and exit codes of iTerm2's script (tmux and screen wrapping exactly
where each script wraps, macOS `base64`), with replies read from the console in raw or cbreak mode
and every console mode restored on exit, Ctrl+C and panic. Tandem makes `utilities\` beside
tandem.exe at every start (16 hard links plus iTerm2's own `it2api.py`, refreshed when stale,
failures shown in a banner) and appends it to each new session's PATH (`addUtilitiesToPATH`);
sessions get iTerm2's variables (`TERM_PROGRAM=iTerm.app`, `TERM_PROGRAM_VERSION`, `COLORTERM`,
`TERM_FEATURES`, and `LC_TERMINAL=iTerm2` under `shouldSetLCTerminal`); `CSI 1337 n` answers
`ITERM2 3.7.0`, so Tandem's it2check passes; RequestUpload takes files or folders (a Files/Folders bar
first) at iTerm2's pace (`accelerateUploads`, else the quick-paste settings); and Install Shell
Integration offers iTerm2 Utilities for bash-family shells. iTerm2's bash scripts still work in
WSL, Git Bash and over ssh, except it2check: ConPTY takes its reply's leading `ESC [ I` for a
focus report and drops it, so the script's check fails there (measured in WSL and Git Bash, inferred over ssh.exe from
the same ConPTY input, a kept ConPTY limitation; the native it2check restores the prefix). it2setcolor's presets wait for milestone 13 and it2ssh for milestone
14; the Swift `it2` CLI is not ported. Every kept quirk is listed in
`docs/acceptance/m11e-checklist.md` and ruled on in `docs/superpowers/rulings/m11e-rulings.txt`.
Milestone 12a opened milestone 12 (AI features) with its settings (02: AI, AI > Prompts, AI >
Features, and the Advanced AI rows): Settings > General > AI with iTerm2's General, Prompts and
Features sub-tabs; "Enable generative AI features" and "Offer AI command suggestion" as ordinary
settings that Import from iTerm2 never brings across; one API key per vendor in Windows Credential
Manager under `TandemAI/`, which the password manager never lists; the AI Model popup over iTerm2's
bundled model catalog (never updated online, a documented gap) and Manage AI Models… with iTerm2's
presets, per-model custom headers and the one-time migration of the legacy global headers; every
prompt with iTerm2's default text, Reset, the missing-variable warning and the help; the eight
default chat permissions; and the gate every AI entry point asks first (the administrator's switch,
Enable, then a one-time consent per host that replaces iTerm2's plugin), wired into the Composer
and Codecierge, whose AI answers still wait for 12b to 12f. Apple Intelligence and AI tab titles
are n/a on Windows. Every kept quirk is listed in `docs/acceptance/m12a-checklist.md` and ruled on in
`docs/superpowers/rulings/m12a-rulings.txt`.
Milestone 12b brought the AI providers (03: Multi-provider support, Streaming responses, the wire
half of Custom API headers and the request-time half of Withheld-key advice; 02: the model, URL, API,
token, timeout and zero data retention settings as request inputs, and the Advanced `aiProxy`,
`ollamaNumCtx`, `ollamaKeepAlive` and AI logging rows): OpenAI Responses and Chat Completions (with
Early O1 and legacy Completions), Anthropic, Gemini, DeepSeek and Ollama's native API, each built as
iTerm2 builds its request, sent over WinHTTP with the system proxy and TLS (no curl, no new crate),
and streamed as the bytes arrive, with iTerm2's error texts and token arithmetic. A localhost or
private host never gets the vendor key, the placeholder key "placeholder" contacts nothing, redirects
are followed only to the host consent was given for, every answer is capped at 64 MiB, and the raw
wire log replaces every key and every header value but known non-secret ones with a fingerprint.
Ollama discovery fills the Regular and Budget pickers and expands auto-discover entries in the AI
Model popup; Manage AI Models… gains Fetch Models, Refresh Models and Test Connection. OpenAI file
uploads and vector stores are built for 12c, only for models at api.openai.com. The chat itself,
which is the first to send a conversation, comes with 12c, so Streaming responses and the two AI
logging rows read partial until then. Every kept quirk is listed in
`docs/acceptance/m12b-checklist.md` and ruled on in `docs/superpowers/rulings/m12b-rulings.txt`.
Milestone 13a built Settings > Advanced as iTerm2's pane (02): the rows under iTerm2's 19
categories in Finder order with bold, pinned headers; a search that matches every typed word in a
row's description or category; "Show only non-default values", which keeps the row being edited
until editing ends; and each kind edited as iTerm2 does (No/Yes, Unspecified/No/Yes where
Unspecified removes the key, a list storing the index, integers and floats read as iTerm2's
`integerValue` and `doubleValue`, text as typed). Twenty keys that are not iTerm2 Advanced
settings moved to a "Stored" tab no pane draws, and the Advanced texts are iTerm2's descriptions
with Tandem's Windows wording where it behaves differently. "Show indicators for non-default
values" (a menu at the right of the Settings page bar) marks changed controls with iTerm2's bar,
including its per-control rules; General > Experimental holds the right-to-left and ssh: URL
checkboxes; a plist import consumes the legacy Control and single-hotkey-window keys. General >
Settings loads settings from a custom folder (one `tandem-settings.toml`; network folders
refused) or an `http(s)` URL (fetched with `curl.exe`), with Save Now, the Save changes popup and
iTerm2's load, overwrite and quit questions; snippets, notes and icons do not sync. Export All
writes `Tandem State.tandemexport` (a gzip tar of the Python runtimes list, the settings, the
`%APPDATA%\Tandem` files and the scripts as unsigned zips; the Python API consent never leaves),
Import All stages an export and applies it at one restart, and Erase All empties Tandem's folders
and quits, or logs a dry run to the debug output. PreferredBaseDir and
noSyncNeverRemindPrefsChangesLostForUrl are stored and inert. Every kept quirk is listed in
`docs/acceptance/m13a-checklist.md` and ruled on in `docs/superpowers/rulings/m13a-rulings.txt`.
Milestone 14a opened milestone 14 (SSH integration) with the transport, the SSH profile and the
handshake (01, 03; 01: no row to have, 03: 2 rows to have and 4 to partial). A probe against a
loopback sshd in WSL measured the transports; Robert chose the shipped ConPTY in passthrough mode
(ruling 1), which carries iTerm2's protocol byte for byte except that invalid UTF-8 arrives as
U+FFFD. The Command type SSH runs `ssh <command>`, or with SSH Integration on Tandem's own
`it2ssh` (a `tandem-utils` command following iTerm2's script, with the origin secret from a
per-user pipe); Configure… holds iTerm2's four settings (SSH Integration, variables and files to
copy, Path to SSH), partial until milestone 14b makes them act; OSC 1337 `Env` (report=all), `it2ssh`, `SendConductor` and `EndSSH` are read
as iTerm2 reads them, with its version and end banners, the hop chain, and `\x03\n` releasing
it2ssh's local read loop; the session host speaks protocol 4 so an SSH session can be hosted.
Until milestone 14b's conductor hook existed (`CONDUCTOR_READY` false), it2ssh and SSH profiles
ran plain ssh with a one-line notice and no typing was ever held. The Shell > ssh menu items and
seven SSH settings in 02 (sshSchemePath, alternateSSHIntegrationScript, unameCommand,
validCharactersInSSHUserNames, sshURLsSupportPath, restoreKeyModeAutomaticallyOnHostChange,
channelsEnabled) belong to milestone 14 (ruling 6). Every kept quirk is listed in
`docs/acceptance/m14a-checklist.md` and ruled on in `docs/superpowers/rulings/m14a-rulings.txt`.
Milestone 13c built iTerm2's tab bar (02: Appearance > Tabs and the Advanced tab rows; 01:
Subtitle): 24 points tall (`defaultTabBarHeight`; Tandem's old 1.6 cells is gone) and hidden with
one tab (`HideTab`); iTerm2's width rules (equal tabs stretched to fill in whole device pixels, 175
points with stretching off, uneven widths only when every tab fits, a minimum of 75 points or 60
with tab numbers hidden); a bar whose tabs do not fit scrolls at 120 points with Shift+wheel or a
horizontal wheel and keeps the selected tab in view (`ScrollableSideTabBar`, which the horizontal
bar reads too), else an overflow menu lists the hidden tabs. Inside a tab: a close button on the
left shown on hover (or always), the title centered in Segoe UI at 11 points (or the custom size)
and cut at its start or end by iTerm2's smart truncation, basic HTML (`<b>`, `<i>`, `<u>`), the
profile's Subtitle on a second line at 28 points or more with the session status joined to it,
the icon, the spinner of a background tab that is printing, and the tab's shortcut ("Win+1" to
"Win+8", "Win+9" on the last tab). A plain press selects (or the release, with `selectsTabsOnMouseDown` off, read as a window opens), the close
button and a middle click close through Close Tab, a double click on a tab opens Edit Tab Title and
on the empty bar opens a tab, "+" opens a tab (a tmux tab in a tmux window; the current profile
with `addTabButtonUsesCurrentProfile`), and a tooltip shows the tab's name, profile and command.
Each window keeps its own scroll offset, hover and press. The Tab Color picker (Default, swatches
from `tabColorMenuOptions`, Choose Color... with Windows' color dialog) is built; it opens from
milestone 13d's tab menu, so `tabColorMenuOptions` stays partial until then, and the side tab bar
is 13d's. `drawBottomLineForHorizontalTabBar` is stored and inert (no iTerm2 style reads it). The
eight number and text Advanced > Tabs rows are edited in Settings > Advanced > Tabs with milestone
13a's controls.
Every kept quirk is listed in `docs/acceptance/m13c-checklist.md` and ruled on in
`docs/superpowers/rulings/m13c-rulings.txt`.
Milestone 13b put every question Tandem remembers an answer to on iTerm2's warning model (02:
the Warnings rows, 03: Suppressed Alerts): one checkbox rule ("Suppress this message
permanently" for one button that may be remembered, "Remember my choice" for several, with the
ten-minute and 30-day forms), Cancel and the do-not-remember buttons never remembered, a
persistent warning never silenced, and each button answering to Alt plus its letter (underlined
with "Buttons in modal alerts indicate keyboard shortcuts"). Every answer given automatically is
listed in Tandem > Suppressed Alerts, whose menu title counts them with the newest one's age;
Un-suppress asks again, and "Always show alerts with remembered selections" shows them with the
saved answer and "Permanently Forget Saved Selection". The paste and tmux close questions moved
from pane bars to alert windows, the broadcast input warning got its checkbox, and Clear and
Reset with broadcast ask iTerm2's three-button warning (a stored No no longer answers "current
only"). New: iTerm2's Restart Session, download and inline display (with the 1 GiB follow-up),
annotation removal, Meta and key override confirmations; the offers to turn focus reporting,
mouse reporting and paste bracketing off after a host change (Always and Never write the
Advanced pane's Unspecified/No/Yes rows); the offer to disable mouse reporting after a drag then
Copy; the arrangement announcements (a missing profile restores from the arrangement's own copy
with no banner, a broken copy offers Assign Profile, a missing folder starts in home with iTerm2's
banner or, for an SSH profile, offers Repair); and the out-of-date shell integration notice, read
from iTerm2's version tables (Tandem's PowerShell script is in none).
noSyncSuppressMissingProfileInArrangementWarning and noSyncNeverAskAboutDEC2048Frustration are
stored and shown with nothing reading them; the Replace Profile warning waits for 13t's Copy to
Profile and DEC mode 2048 (a new 03 row) for 13o. Every kept quirk is listed in
`docs/acceptance/m13b-checklist.md` and ruled on in `docs/superpowers/rulings/m13b-rulings.txt`.
Milestone 14b added iTerm2's conductor, SSH integration level 1 (01, 03; no row changes status: the
Configure SSH, Load shell integration automatically, it2ssh and handshake rows stay partial until
milestone 14c's framer gives level 2). The `DCS 2000p` hook takes the remote's output while iTerm2's
unmodified conductor.sh sets up, and Tandem writes it the bytes iTerm2 writes, pinned against
recordings of the real script: getshell, the environment variables to copy, Files to Copy and
iTerm2's shell integration files as gzip tarballs, the injection for zsh, fish and xonsh (bash only
at level 2, as iTerm2), the profile's remote directory, the Python check and the login shell.
Typing, pastes, terminal reports and coprocess output made during setup reach the remote once, in
order, after it; the conductor's own writes are never held. Python 3.7 or later also gets level 1
until milestone 14c. Every setup step has a 30-second limit (a Files to Copy write gets more with
its size, since conductor.sh reads it in time that grows with the square of its size), and a failure
shows iTerm2's error text, lets output show again and never leaves typing held (typing made during a
failed or ended setup is dropped, as in iTerm2); EndSSH pops the conductor chain and restores each
hop's saved state. The iTerm2 bugs met (B1 to B17) are kept or fixed one by one. Every kept quirk is
listed in `docs/acceptance/m14b-checklist.md` and ruled on in
`docs/superpowers/rulings/m14b-rulings.txt`.

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
