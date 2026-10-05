# Tandem parity roadmap

The target is every row in the three inventory files. Each milestone below names the inventory
sections it closes. A milestone ends when those rows read `have`, or carry a written reason why
Windows cannot support them. Each milestone gets its own plan in `docs/superpowers/plans/`.

| File | Rows | have | partial | missing |
|---|---|---|---|---|
| `01-profile-settings.md` | 316 | 269 | 36 | 11 |
| `02-global-settings.md` | 820 | 579 | 51 | 190 |
| `03-features-and-menus.md` | 647 | 566 | 43 | 38 |
| Total | 1,783 | 1,414 | 130 | 239 |

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
logging rows read partial until then (12c made them have). Every kept quirk is listed in
`docs/acceptance/m12b-checklist.md` and ruled on in `docs/superpowers/rulings/m12b-rulings.txt`.
Milestone 12c built the chat engine (03: Command safety checker, Terminal hard rules, Agentic
tool calling, Chat history persistence's storage half, and Streaming responses; 02: AISafetyCheck,
the eight AI permissions and the chat prompts as the agent uses them, and the two AI logging rows,
which now have a writer). It has no window of its own: the chat window is 12d, so its checklist is
run with 12d's. Chats and messages live in `%APPDATA%\Tandem\chatdb.sqlite` in iTerm2's schema (an
iTerm2 database copied there opens and migrates; a second Tandem shows the database error), and a
chat linked to a session reads it (state, screen, history, output), types into it, runs commands in
cmd, PowerShell or a POSIX shell with the output found by the prompt marks or a per-shell marker,
sets the clipboard, creates a file shown in Explorer and restarts the session, each only through
the permission a person chose (Ask parks, Always runs, Never refuses, the grant kept per chat and
session). The optional safety check flags a dangerous command, which then parks even at Always
Allow; iTerm2's hard rules are ported byte for byte with a Windows set beside them (a floor the
classifier backs up). Every request the engine sends passes 12a's AI gate for its own host, and
turning the gate off cancels what is running. Where Tandem closes a hole iTerm2 leaves (a line
typed in pieces is checked whole, the typing tools refuse line breaks and control keys, every
model-written field in a request is escaped) it is ruled and listed. The five browser tools wait
for milestone 13. Every kept quirk is listed in `docs/acceptance/m12c-checklist.md` and ruled on
in `docs/superpowers/rulings/m12c-rulings.txt`.
Milestone 12d built the chat window and the inline chat (03: View > Show Inline Chat, Session > Open
AI Chat..., Window > AI Chats, AI Chats window and In-chat find, 5 rows to have; Chat history
persistence's browsing half, already have). Window > AI Chats (Shift+Win+Y) opens one egui window with
the chat list (rename, delete, a search across every chat whose quote bug is fixed with bound
parameters, minted titles) and a conversation (markdown, a copy for each code block, Edit, Fork and
Delete on a message, attachments, drops and drafts), a toolbar of provider, model, effort, tier,
thinking and web search, and the Chat Info menu. The four request buttons, Select a Session (a
"Link this session to AI chat?" button on every pane), linking, per-category permissions and
Send Commands & Output to AI Automatically answer through 12c's one gate; a linked pane shows a
link glyph and a streaming glyph. Win+F finds in a conversation. View > Show Inline Chat
(Shift+Win+,) draws a per-pane panel from the same models at the pane's right, beside Clippings,
refused in tmux panes and kept by arrangements and restoration. A chat binds a terminal only through
the pane's own session id, never its number, so a relaunch never gives an old grant to a new pane,
and every file a model names goes through one helper that refuses Explorer-parsed types and marks
the file. Orchestration buttons show disabled until 12f; "Link Browser Session" until milestone 13.
Every kept quirk is listed in `docs/acceptance/m12d-checklist.md` and ruled on in
`docs/superpowers/rulings/m12d-rulings.txt`. 12c's checklist is run with this window.
Milestone 12e built the inline AI features (02: the three codecierge rows to have, AICompletion,
the Engage AI prompt and the chat icon prompt noted, logAITabTitleCorpus and aiGeneratedTabTitles
to n/a; 03: Edit > Engage Artificial Intelligence, Edit > Explain Output with AI, AI features >
Explain this output, the Codecierge tool and Composer autocomplete to have, AI-generated tab titles
to n/a; 01: Title components noted). Win+Y asks for a plain-English request (prefilled from the
selection or the typed command, with a skip checkbox) and puts the model's command in the large
Composer; Win+Alt+Y opens an AI chat that explains the selection or the last command and marks the
output with hidden notes, each reachable from a bullet link in the answer; with AI Completion on the
Composer offers model replacements in its popup (never as ghost text; a tmux pane never asks); and
Codecierge works through goal, suggestion and auto-run views, with iTerm2's Danger! and Privacy
warnings (auto-run only after Danger! is answered OK), a usage warning after 10 commands, and a
`write` tool that types `cat >` for a POSIX shell and refuses in cmd and PowerShell, local or remote
(final review ruling). An explanation request's "attached terminal content" link selects the
explained text, as iTerm2. Every request passes 12a's gate for its own host right before it is sent, and Codecierge
stops for real when the gate closes, on End Task, or when its pane closes. AI tab titles are not
available on Windows (ruling 7): the 4096 bit draws nothing. The chat icon waits for milestone 13's
SVG rasterizing (ruling 11). The tables count have, partial and missing rows; n/a rows (02: 3, 03: 1)
are left out of them. Every kept quirk is listed in `docs/acceptance/m12e-checklist.md` and ruled on
in `docs/superpowers/rulings/m12e-rulings.txt`; 12c's and 12d's checklists are run with this window.
Milestone 12f built the Cockpit and the Claude Code integration, and gave AI chats orchestration mode
(03: Install, Reinstall and Uninstall Claude Code Integration, Window > Cockpit and the Orchestration
mode row, 5 rows to have; OSC 21337, Toolbelt > Session Status and Window > Notify on Status Change
noted; 02: the orchestration prompt noted). Window > Cockpit (Alt+Win+Shift+C) is one always-on-top
egui panel, hidden while another app is in front, that lists every session of every window and the
buried ones by Status or by Window (Workgroup stays empty until milestone 13), with a filter bar,
search, a bell on a session or a window, and a command bar that types once into the selected rows or
`@` chips (Shift+Enter; `@all`, `@working`, `@waiting`, `@idle`; it types into buried sessions as
iTerm2 does). Sessions carry lasting stable ids (`ptys_...`, restored only at launch restoration).
`cc-status.exe`, a console program beside tandem.exe, turns Claude Code's ten hook events into the
session's tab status (working orange, waiting blue, idle green) through the Python API with the
session's cookie, and `set_status` keeps `background_tasks`. The setup writes the hooks into
`settings.json` without touching other settings (an unparsable or unreadable file is refused, a backup
is kept, the write is atomic); setCookie turns on only when the person answers its own question;
npm and WSL Claude Code sessions are not detected. In an AI chat the model can ask to switch
orchestration on (only the person's press does it), then drives sessions through 14 of iTerm2's 16
tools and 23 `session_*` mirrors: a session needs a claim (an `@` pick in the chat input grants one),
every keystroke goes through one typed-text gate that classifies a staged or split command as one
line, typing reaches only live, visible, non-secret panes, the safety check always runs, and watchers
and timers die with the chat's turn and with the launch. notify and request_notification_permission
are n/a (iPhone companion). Milestone 13's Workgroups sub-part
(`docs/superpowers/plans/2026-10-15-m13y1-workgroups.md`) owns start_code_review, the Workgroup
grouping, workgroup peers, role prefixes, the setup's workgroup and trigger steps, the browser
mirrors, `ai.default_code_review_prompt` and `ai.saved_prompt_names`. Nothing of 12f was checked on
screen during the build. Every kept quirk is listed in `docs/acceptance/m12f-checklist.md` and ruled on
in `docs/superpowers/rulings/m12f-rulings.txt`.
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
Milestone 13d built tab dragging, pinned tabs, tab groups and tab icons (01: 2 rows to have;
02: 16 rows to have; 03: 5 rows to have, 2 rows added as have, and the Duplicate Tab row
corrected). A tab drags to reorder in its bar (inside the pinned or unpinned zone), into another
window's bar, or out onto the desktop into a new window at the drop point, after
`minimumTabDragDistance`; Lock Layout refuses; the "+" button drags the window. The bar goes on
any edge (`TabViewType`: Top, Bottom, Left, Right); a side bar is rows `LeftTabBarWidth` wide,
resized by its inner edge, scrolling with the wheel or, with `ScrollableSideTabBar` off, sending
the rest to an overflow button. In full screen the bar hides unless Show tab bar in fullscreen
(View > Show Tabs in Fullscreen, Win+Shift+U), and with Flash tab bar on it shows for a second on
a tab switch, kept while Win is held. The window grows by the bar as it appears and shrinks back
as it hides, unless Preserve window size (R3). Pinned tabs sit first, 64 points wide with the pin
glyph, always ask before closing and are named in a window or quit question ("Tab #N is
pinned."). Groups (the Window > Tab items, the tab menu's Add Tab to Group, and a right-click
menu on the group's chip) carry a name and a palette color on a chip before their tabs, collapse
by a click on the chip, take a tab dropped onto a collapsed chip, move whole by a drag of the
chip, and are kept with pins in arrangements; Select Next and Previous Tab Group and Move Tab
Left and Right follow iTerm2's group rules. The tab's right-click menu is iTerm2's, with Tab
Color and Group Color opening 13c's picker. The profile icon shows iTerm2's MIT icon set for the
job or its ancestors (merged with the user's `graphic_icons.json`, `graphic_colors.json` and
`graphic_<name>.png` in `%APPDATA%\Tandem`, read at launch), a first-letter circle, or a custom
image. Duplicate Tab copies every pane next to the original. Nothing of 13d was checked on
screen during the build. Every kept quirk is listed in `docs/acceptance/m13d-checklist.md` and
ruled on in `docs/superpowers/rulings/m13d-rulings.txt`.
Milestone 13g built transparency and desktops (01: Space to have and one row added; 02: 7 rows to have; 03: the
dock click row to have; eight rows stay partial until the person confirms checklist line 1). A profile's Transparency
draws its default background and margins at 1 - Transparency over the desktop (per-pixel alpha through a
DirectComposition swapchain on a second Direct3D 12 renderer instance used only by see-through windows; opaque
windows keep the original path; the tab bar, status bars, toolbelt, dividers, popups and cursor stay opaque;
background images stay opaque). Keep background colors opaque keeps colored cells and the selection solid, Win+U
and View > Disable Transparency for Active Window work per window, and full screen drops it by iTerm2's state
machine (this also closes 13f's "transparent pane forces the border"). Blur is Windows 11 22H2 acrylic (none on
older Windows; the radius is stored and shown, with the warning above 26). A window that needs alpha later is
recreated once with its tabs, panes, position, full screen and topmost state. Space maps to Windows virtual
desktops (Desktop 1 to 9 in Task View order, All Desktops follows within a second, a missing desktop notifies once),
and Restore windows to same desktops moves restored windows back. Display changes (WM_DISPLAYCHANGE, work-area
changes) refit top-of-screen, maximized and full screen windows after UpdateScreenParamsDelay, and
ChaseAnchoredScreen moves a window back to its profile's screen. Exclude from taskbar and Alt+Tab (with its hotkey
only variant and HotkeyWindowsExcludedFromCycling) uses tool-window styles and a notification-area icon whose menu
lists every window, opens Settings, lists buried sessions and quits; the dock click is a left click on that icon.
The rows that need the person: 01:179 to 01:181, the new rendering row, 02:209, 02:262, 03:157 and 03:158 are partial
until checklist line 1 (a DirectComposition and an HWND swapchain in one process on the desktop GPU) passes, and
blur (01:182, checklist line 9) stays partial until acrylic is seen behind the visual swapchain, and Blur Radius (01:183) stays partial because it has no effect; if the line fails they stay partial
("stored and shown; no see-through rendering on this path"); if it passes they read have (1,199 have, 224 partial in total).
StatusBarIcon alone decides the icon (iTerm2's rule); with it off, starting Tandem again acts as the dock click. Nothing of 13g was
checked on screen during the build. Every kept quirk is listed in `docs/acceptance/m13g-checklist.md` and ruled on
in `docs/superpowers/rulings/m13g-rulings.txt`.
Milestone 13f built the window chrome, titles and sizing (01: 13 rows to have; 02: 15 rows to
have and the window number row's note corrected). Titles follow iTerm2's rules: "Allow title setting" off ignores programs' titles, "Each
tab may have its own window title" off copies a program's title to every session of its window,
and with the tab bar hidden and showWindowTitleWhenTabBarInvisible off the caption shows the
session's title. A profile's custom window and tab titles apply when its window or tab opens and,
like Edit Window Title and Edit Tab Title, are interpolated strings. A titled window shows its
switch shortcut after its title ("vim    Alt+Win+1", R11) and the Window menu lists every
window with its shortcut. "Heavy border around windows in light mode" (UseBorder) colors the
window's DWM border on Windows 11, with iTerm2's focused and unfocused colors; SquareWindowCorners
squares a new window's corners. Full screen hides the window's menu bar by default, and a
one-point line shows under the title bar when no tab bar is at the top (both on by default as in
iTerm2). Windows snap to whole cells while an edge is dragged (R3; Ctrl alone, or
DisableWindowSizeSnap, resizes smoothly), AllowLiveResize off stops edge resizing, and "Terminal
may resize window when not focused" off refuses SetWindowFrame from an unfocused pane. Make Text
Bigger, Smaller and Normal Size resize the window to keep its grid (R3), Window > Zoom maximizes
vertically only with MaxVertically, Window Type 4 opens full screen and 14 as a topmost window
without a taskbar button, and the profile's Window style popup has iTerm2's ten entries with
units popups sizing Normal, No Title Bar, Centered and edge windows in cells or percent of the
screen. SizeChangesAffectProfile stays missing for milestone 13n. The Compact and Minimal
caption label came with 13e, and a transparent pane forcing the border waits for 13g. Nothing of 13f
was checked on screen during the build. Every kept quirk is listed in
`docs/acceptance/m13f-checklist.md` and ruled on in `docs/superpowers/rulings/m13f-rulings.txt`.
Milestone 13e built the tab themes (01: 1 row to have and the compact Window Types 13, 16 and 18 rewritten; 02: 20 rows to have, 19 from
missing and 1 from partial). Settings > Appearance > General > Theme (the real key is
TabStyleWithAutomaticOption, default Regular) has iTerm2's seven entries: Regular follows Windows' light and dark
mode and its high contrast setting; Light, Dark, Light (High Contrast) and Dark (High Contrast) force the mode
on the title bar, the menu bar and Tandem's own windows (Profiles > Colors shows iTerm2's warning under its mode
line); Minimal and Compact leave the mode to Windows. Each theme draws the tab bar with iTerm2's pre-Tahoe
palette (PSMYosemite, Dark, the two High Contrast styles and PSMMinimal, with the fills, outlines, notch and
underline of colored tabs) and the split pane divider follows the theme. Under Minimal and Compact a window's
tabs move into the caption area with Tandem's own minimize, maximize and close buttons at the right (snap layouts
work on hover), a shortcut label ("Alt+Win+1") before them, and the window title when one tab hides the bar; the
menu bar cannot sit under that strip, so a menu button at its left, Alt or F10, and a right click on empty bar
space open the whole menu. A press in the top strip, on empty bar space, on a solitary tab, or Alt held on any bar
point moves the window; a double click on the empty strip maximizes. The window name shows beside the tabs (Never,
When a custom name is set, Always) and a double click on it edits the title. The nineteen Advanced rows for tab
darkness, minimal knobs, drag strips, bar height and caption room are edited in Settings > Advanced. Nothing of 13e was checked on screen
during the build. Context and popup menus still follow the system mode under a forced theme. Every kept quirk is
listed in `docs/acceptance/m13e-checklist.md` and ruled on in `docs/superpowers/rulings/m13e-rulings.txt`.
Milestone 13h built the pane title bars (02: 13 rows to have, 7 of them from partial; 03: 1 row to have, Move Session to
Split Pane). Every pane of a split tab shows iTerm2's title bar (Show per-pane title bars, on by default; "even if there is
only one pane" is enabled with it): a close button at the left, the pane's job or title, and a menu button at the right, in
the tab theme's colors, tinted by the pane's tab color and dimmed in an unfocused pane (paneTitleBarBackgroundColor and
paneTitleBarTextColor override them). With Shortcut to choose a split pane set, the title starts with its shortcut
("Alt+1   vim"). A pane's own status bar sits in its title bar when the bars are separate and at the top. The bar takes
its room from the pane, tmux sizes included. A click on the bar focuses the pane, a double click maximizes it, the close
button closes it, and the menu button or a right click opens the pane's context menu (a script's context menu items reach
only that pane, and no command of it acts on another pane). Session > Move Session > Move Session to Split Pane, Swap With
Session, the Move Pane pointer action and the Move to Split Pane key action start iTerm2's pick: a destination pane and a
side, in any window, with the six split pane colors. Dragging a title bar does the same with the pointer: onto another
pane's half, onto another window's pane or tab bar, or onto the desktop for a new window; a lone pane's bar drags the window,
and RequireOptionToDragSplitPaneTitleBar asks for Alt. Lock Layout refuses (a beep). tmux panes are moved and swapped by tmux.
The documented gap of 11c (a per-pane title bar menu adding context menu provider items) is closed. Nothing of 13h was
checked on screen during the build. Every kept quirk is listed in `docs/acceptance/m13h-checklist.md` and ruled on in
`docs/superpowers/rulings/m13h-rulings.txt`.
Milestone 13i built pane navigation (01: 3 rows to have, all from partial; 02: 8 rows to have, 3 from partial, 5 from
missing; 03: 1 row to have from missing, Lock Split Pane Width). Next Pane, Previous Pane, the pane number shortcut, the title bar
numbers and the Status tool follow one order: reading order by default (NavigatePanesInReadingOrder), the order last used
when it is off, with the ninth shortcut always the last pane (the Status tool reads the same numbers as the title bars). Select Pane by direction goes to the neighbor used last and
wraps at the edge (WrapFocus). A new split keeps focus off when it should (FocusNewSplitPaneWithFocusFollowsMouse, and a
split of a pane that is not active never takes focus) and uses the session's own profile (UseDivorcedProfileToSplit).
SplitPaneDividerWidth and SplitPaneColor set the dividers, scaled to the screen. Inactive panes dim toward gray by the
Dimming amount (0.01 to 0.6), with Dim inactive split panes, Dim only text and Dim background windows in a Dimming section
of Appearance, and the active pane gets its border. Lock Pane (context menu, title bar mark, Default Pane Locked, saved
arrangements) makes closing ask and moving refuse; Lock Split Pane Width caps a pane's width through window resizes.
Nothing of 13i was checked on screen during the build. Every kept quirk is listed in
`docs/acceptance/m13i-checklist.md` and ruled on in `docs/superpowers/rulings/m13i-rulings.txt`.
Milestone 13j built the mouse and the scroll wheel (01: 7 rows to have, all from partial; 02: 20 rows to have, 10 from missing
and 10 from partial, StealKeyFocus from missing to partial, one row added: NoSyncNeverAskAboutSettingAlternateMouseScroll; 03: one row added, OSC 22, missing).
iTerm2's defaults now hold, each a change users will see: in the alternate screen the wheel scrolls Tandem's history and
sends arrow keys only when the app set DECSET 1007 (which starts off), the profile's Automatically enable alternate mouse
scroll or the Advanced switch AlternateMouseScroll asks for them; one report and one arrow per wheel notch; horizontal
sensitivity 0.1; the click that activates a window is not reported (and never selects, copies or moves the cursor); new
profiles report the mouse (Mouse Reporting defaults on). Scrolling uses iTerm2's accumulators per pane, with a touchpad's
small deltas accumulated before rounding so a laptop does not over-scroll, acceleration, fast trackpad, proportional
reporting, double reports and natural scrolling. Report Mouse Wheel Events, Report Mouse Clicks and Drags and Restrict mouse
reporting to alternate screen work. A pane that echoes the mouse reports it was sent offers to turn reporting off for that
pane. Focus follows mouse gains steal and aggressive modes and makes another Tandem window key under the pointer; Ctrl+wheel
(a touchpad pinch) changes the font size. The pointer is the system I-beam over text (or one Tandem draws), a circled I-beam
under a mouse-reporting program, an arrow over an image, a pointing hand over a Win-hovered link, a crosshair with Ctrl+Alt
and resize arrows on dividers; Next Window, Previous Window and New Window With Profile pointer actions run. Every Tandem
overlay inside the terminal (info bars, pills, pickers, the find bar, the completion list, the composer, toasts, banners and
the pane move pick) holds clicks, hover and the wheel from a mouse-reporting program. Open URL in background and QuickLook
stay partial (final, ruling R7). StealKeyFocus stays partial: Windows may refuse the foreground. The program-set pointer
(OSC 22, a new 03 row marked missing) is owned by 13y2 (gaps sweep). Nothing of 13j was checked on screen during the build. Every kept quirk is listed in
`docs/acceptance/m13j-checklist.md` and ruled on in `docs/superpowers/rulings/m13j-rulings.txt`.
Milestone 13k built the cursor (01: 16 rows from partial to have and one from missing, Smooth blink presets, plus a new row, IME
composition; 02: 12 rows from missing to have; HDR Cursor stays partial, Tandem renders sRGB; 1,779 rows, 1,247 have, 203 partial,
329 missing on its branch; 1,781, 1,275, 187 and 319 once merged with 13j). The block cursor is spacing-free and
centered, the bar is VerticalBarCursorWidth and the underline UnderlineCursorHeight points, FullHeightCursor fills the line.
The blink is TimeBetweenBlinks (0.5 s), solid for half a second after the cursor moves, only in the focused pane, and programs
change it only with Allow Change Cursor Blink (alacritty_terminal patch 17); Hide cursor when unfocused, Smooth blink with its
six timings and five presets, Smart Cursor Color, Cursor Boost (it mutes text, backgrounds, selection and find matches),
Cursor shadow, DrawOutlineAroundCursor, Smooth slide and Animate large movements (a smear) follow iTerm2's source, and the
Windows IME composes at the cursor. Nothing of 13k was checked on screen during the build. Every kept quirk is listed in
`docs/acceptance/m13k-checklist.md` and ruled on in `docs/superpowers/rulings/m13k-rulings.txt`.
Milestone 13l built frames and underlines (01: 3 rows from partial to have: Use custom underline color, Underline Color, Reduce Flicker;
02: 17 rows to have (12 from missing, 5 from partial): Side and Top/bottom margins, disableAppNap, the adaptive-rate
and cadence keys, underlineHyperlinks, solidUnderlines, showMetalFPSmeter, metalRedrawPeriod, extendBackgroundColorIntoMargins; the
disable-GPU row (02:151) goes from missing to partial because Tandem has no CPU renderer; 03: Copy Performance Stats and GPU Renderer Availability to
have, plus two new rows, SGR underline styles with SGR 58 and Strikethrough; 1,783 rows, 1,299 have, 180 partial, 304 missing on its
branch). iTerm2's defaults now hold, each a change users will see: output draws on iTerm2's update cadence (30 fps over 10,000 bytes
a second, 60 below, the alternate screen at ActiveUpdateCadence, 1 s when idle, the Windows battery saver caps at 30) instead of every
vsync, with typed echoes and the cursor drawn at once; a visible window redraws every 0.5 s even idle (MetalRedrawPeriod); the integrated
GPU is preferred on a machine with two (after a restart); margins are 5 points at the sides and 2 at the top and bottom; OSC 8 links
use a 4-on, 3-off dash at the underline position. Underlines draw single, double, curly, dotted and dashed with SGR 58 colors, gaps
at descenders, strikethrough, and every Tandem surface over a pane (toast, banner, find bar, popups) cuts them. The alternate screen's
margins take its edge color, Reduce Flicker holds the picture while the cursor is hidden, and Copy Performance Stats and the FPS meter
show Tandem's own renderer timings. The two Disable GPU settings, UseAdaptiveFrameRate, DisableAdaptiveFrameRateInInteractiveApps,
UseLowPowerGPUWhenUnplugged and UseMultiPassUnderlineRenderer are stored and inert. Nothing of 13l was checked on screen during the
build. Every kept quirk is listed in `docs/acceptance/m13l-checklist.md` and ruled on in `docs/superpowers/rulings/m13l-rulings.txt`.
Milestone 13m built contrast and bidi (01: 7 rows to have, 5 of them from partial: Minimum Contrast, Harmonize 256 Colors, Use Theme Mark
Colors, Allow blinking text, Timestamps Style; 2 from missing: Excessive contrast warning, Use background image; 02: 8 rows from
missing to have: Bidi, rightJustifyRTLLines, detectParagraphDirection, disableBidiInAlternateScreen, HideScrollbar,
alwaysUseLineStyleMarks, showLocationsInScrollbar, showMarksInScrollbar; p3 and allowHDR from missing to partial, Tandem renders sRGB
and has no HDR swap chain; 03: Show Timestamps, Exit-code coloring and Mark rendering in scrollbar to have; 1,783 rows, 1,316 have,
175 partial, 292 missing). Minimum Contrast follows iTerm2's perceived-brightness rule on each cell's raw text color before Cursor
Boost and Dim Only Text, with a Black & White Text Only warning at 0.97; Harmonize 256 Colors rebuilds the cube in Lab; Use Theme
Mark Colors and line-style marks (Auto Composer or alwaysUseLineStyleMarks) follow iTerm2; SGR 5 text blinks with Blink Allowed
(alacritty_terminal patch 18). Right-to-left rows reorder through UAX 9 with mirrored characters and joined Arabic; the pointer,
a visual drag selection (patch 19), copy (logical), cursor, IME, links, notes and underlines follow the display order, mouse
reports carry the visual column, and explicit bidi controls are stripped so a program cannot flip text outside its line. Every
pane has iTerm2's scrollbar (legacy or thin overlay by Windows' "Always show scrollbars", HideScrollbar) with prompt, annotation
and Find ticks, and Adjacent timestamps get their own column through one per-pane text rectangle that tmux sizing shares. Deferred:
Highlights.selections (13s), the selected-command-region skip for line marks, OSC 12 and OSC 112 cursor colors. Nothing of 13m was
checked on screen during the build. Every kept quirk is listed in `docs/acceptance/m13m-checklist.md` and ruled on in
`docs/superpowers/rulings/m13m-rulings.txt`.
Milestone 13n built fonts (01: 9 rows from partial to have: Special Font Config, Use Bold Font, Use Italic Font, Thin Strokes, both
Anti Aliased switches, both Ligatures switches, Draw Powerline Glyphs; 02: 11 rows to have, 2 from partial and 9 from missing:
disableCustomBoxDrawing, minimumWeightDifferenceForBoldFont, preferSpeedToFullLigatureSupport, enableContextualAlternates,
forceAntialiasingOnRetina, makeSomePowerlineSymbolsWide, supportPowerlineExtendedSymbols, fontsForGenerousRounding, SizeChangesAffectProfile,
and from partial preserveFontSizeOnAutomaticProfileSwitch and fontChangeAffectsBroadcastingSessions; 03: Make Text Bigger, Make Text Smaller
from partial and Size Changes Update Profile from missing to have; 1,783 rows, 1,339 have, 162 partial, 282 missing). Every pane draws
with its own profile's fonts through renderer faces, and window chrome keeps the default profile's face. Line height comes from the font's
ascent, descent and leading (R3, was 1.2 times the size), with iTerm2's generous rounding for Consolas; bold and italic faces follow iTerm2's
weight rule with a one pixel double strike when no face is heavy enough; ligatures form only when the profile asks (R3), through OpenType
features and one buffer row per ligature level; ASCII and Non-ASCII Anti Aliased and Thin Strokes filter the glyph mask in a vendored
glyphon (crates/glyphon, patch 1); box drawing, block elements and every Powerline shape are drawn by Tandem as custom glyphs with zeno
(Powerline hand-drawn from geometry, nothing converted from iTerm2's images; the shades U+2591 to U+2593 stay with the font, as in iTerm2);
Make Text Bigger and Smaller change the focused pane, its broadcast group (FontChangeAffectsBroadcastingSessions) or, with Size Changes
Update Profile, the saved profile (R3, was every pane); an Automatic Profile Switch keeps the zoom; Special Font Config has iTerm2's
Special Exceptions window with Import and Export. Deferred: the Nerd Font bundle download (not offered), Restore Text and Session Size
restoring Columns and Rows (13f's window sizing), a "Font not installed" banner for a non-default profile. Nothing of 13n was checked on
screen during the build. Every kept quirk is listed in `docs/acceptance/m13n-checklist.md` and ruled on in
`docs/superpowers/rulings/m13n-rulings.txt`.
On the m13o branch, main (13n) merged into 13o's Tasks 1-7 (2026-10-05): 13o changes no parity row before its docs task, so
the union is 13n's (01: 316 rows, 238 have, 66 partial, 12 missing; 02: 820, 540, 53, 227; 03: 647, 561, 43, 43; 1,783 rows,
1,339 have, 162 partial, 282 missing). Seams wired at the merge: a cell 13o makes wide never joins a Non-ASCII ligature group, a U+200B
cell draws as a space, and the DEC 2048 report's pixel size uses each pane's own cell.
Milestone 13o built emulation (01: 10 rows from partial to have: Ambiguous Double Width, Unicode Normalization, Unicode Version,
Scrollback in Alternate Screen, Character Encoding, Terminal Type, Answerback String, Allow Title Reporting, Disable Printing, Disable
Smcup Rmcup; 02: 17 rows to have, 1 from partial and 16 from missing: preventEscapeSequenceFromClearingHistory (partial), focusReportingEnabled,
acceptOSC7, defaultTabStopWidth, xtermVersion, oscColorReport16Bits, zeroWidthSpaceAdvancesCursor, tabsWrapAround, maxURLLength,
disableDECRQCRA, saveScrollBufferWhenClearing, saveScrollbackWhenCursorMovesAbovePrompt, supportREPCode, resetSGROnPrompt, vs16Supported,
vs16SupportedInPrimaryScreen, fullWidthFlags, plus bufferDepth and aggressiveBaseCharacterDetection from missing to partial (stored, shown, inert, R2);
03: DEC mode 2048 from missing to have and the kitty drag-and-drop protocol from missing to partial; 1,783 rows, 1,367 have, 154 partial,
262 missing). R3 fixes of defaults: CSI 3 J asks before clearing history, the alternate screen keeps scrollback, U+200B advances the cursor,
emoji plus VS16 widen in the primary screen, lone regional indicators take two cells, and the stock profile uses Unicode 9 widths. The held-sequence
scanner runs the same (vendored) vte parser as the screen, so no padded or spelled-differently sequence slips past a gate; DECRQCRA, title reports,
ENQ answerbacks, the print guard, OSC 72 and 2048 reports are trusted-only where iTerm2 has no check, bounded and built rather than echoed.
Character Encoding uses Windows code pages (ISO 2022-JP is not streamed); typed text and the answerback are encoded, replies leave raw.
Deferred: DECRQCRA's origin mode and default-0 rectangles, DOCS (ESC % G), directory handles, offers, drags out and the SSH tier of OSC 72,
AggressiveBaseCharacterDetection off, BufferDepth. Nothing of 13o was checked on screen during the build. Every kept quirk is listed in
`docs/acceptance/m13o-checklist.md` and ruled on in `docs/superpowers/rulings/m13o-rulings.txt`.
On the m13p branch, main (13o) merged into 13p's Tasks 1-6 (2026-10-05): 13p changes no parity row before its docs task, so
the union is 13o's (01: 316 rows, 248 have, 56 partial, 12 missing; 02: 820, 557, 54, 209; 03: 647, 562, 44, 41; 1,783 rows,
1,367 have, 154 partial, 262 missing). Seams wired at the merge: 13o's TERM (the profile's Terminal Type) reaches a WSL session
through WSLENV (P17), every 13o Character Encoding choice has a locale name for 13p's locale guess (P18: GB18030, Big5, eucKR,
KOI8-R), Initial Text is typed through 13o's session encoder, and the offscreen command line reads only the displayed grid (no band
over 13o's alternate-screen history) and draws in the profile's Unicode Normalization form.
Milestone 13p built session start (01: 13 rows from partial to have: Send text at start, Initial working directory, the three Advanced
working directory rows, Set Locale Variables, Change Locale, Show Offscreen Command Line and its current-command key, Prevent Sleep, Enable
Progress Bars, Progress Bar Height and Progress Bar Color Scheme; Load shell integration automatically stays partial, PowerShell only; 02: 8 rows
to have, 6 from missing and 2 from partial: doNotSetCtype (stored, inert, R2), useColorfgbgFallback, shouldSetTerminfoDirs, preventSleepOnBatteryToo,
jiggleTTYSizeOnClearBuffer, fallbackLCCType, plus idleTimeSeconds and fastForegroundJobUpdates; 03: OSC 9;4 from partial to have; 1,783 rows, 1,389
have, 138 partial, 256 missing). WSL and MSYS2 programs get LANG or LC_CTYPE and COLORFGBG (WSL through WSLENV), WSL also Tandem's extended terminfo;
Initial Text, Advanced working directories, the offscreen command line, OSC 9;4 progress bars (inline and in tab cells, no taskbar button, as
iTerm2), Prevent Sleep, Clear Buffer's jiggle, the idle time and event-driven job updates are built. Security fixes found by the reviews: Initial Text
is never typed into an SSH password or host-key prompt and is blanked for git, orchestration, the script REPL and ssh: launches; a NUL in a profile value
cannot add an environment variable; an expanded working directory must be a local drive path; one network-path filter (UNC, device and file://host forms)
runs at every non-Settings ingress for the twelve path-valued profile keys (arrangements, imports, remote prefs, the API); the PowerShell injection
doubles every PowerShell quote character. Deferred: bash, zsh and fish injection in WSL or MSYS2 (milestone 14). Nothing of 13p was checked on
screen during the build. Every kept quirk is listed in `docs/acceptance/m13p-checklist.md` and ruled on in
`docs/superpowers/rulings/m13p-rulings.txt`.
On the m13q branch, main (13p) merged into 13q's Tasks 1-6 (2026-10-05): 13q changes no parity row before its docs task, so
the union is 13p's (01: 316 rows, 261 have, 43 partial, 12 missing; 02: 820, 565, 52, 203; 03: 647, 563, 43, 41; 1,783 rows,
1,389 have, 138 partial, 256 missing). Seams wired at the merge: a restart starts in the pane's own start folder (P13), drops the
old start's late Initial Text and types the new shell's own once (P14; an SSH profile still waits for its login), reads the
session start values again (idle time), and clears the dead program's progress bar; an exited shell leaves the job watcher when
its exit is first seen (no dead handle in the wait set); Prevent Sleep stops counting an exited shell.
Milestone 13q built session end, logging and archives (01: 8 rows to have, 7 from partial and 1 from missing: Close or restart session on end,
Warn about short-lived sessions, Automatically Log, Log Directory, Log Filename Format, Logging style, Archive On Closure, Archive Directory;
02: 15 rows, 14 to have and 1 to partial: suppressRestartAnnouncement, autoLogFormat, autologAppends, logTimestampFormat,
logTimestampsWithPlainText, sessionEndMessageText, sessionEndMessageDividerCharacter, sessionRestartedMessageText, sessionFinishedMessageText,
shortLivedSessionDuration and killSessionsOnLogout from missing, tmuxWindowsShouldCloseAfterDetach and smartLoggingWithAutoComposer from
partial, multiserver from missing to have and useNewContentFormat from missing to partial (both stored and shown, inert, R5); 03: Save Archive of
Current Session, Restore Archive and Log to File from missing to have; 1,783 rows, 1,414 have, 130 partial, 239 missing). A pane whose
shell ends now follows "After a session ends" (R3: it was always closed): No Action keeps the pane with a blue divider and a restart offer,
Close closes (with a warning under three seconds), Restart restarts in the same pane with its history kept, git windows stay with "Finished", and a
detached tmux window asks whether to close. Sessions log in four styles (raw, plain text, HTML, asciinema), automatically or from the Session
menu, and archive on closure or by Shell > Archives; Restore Archive opens a saved window read-only and Restart makes it live. Security fixes
found by the reviews: a pane never ends a program its shell left running without asking (close, every restart route including the API and AI tool),
log names and folders cannot be steered by a program's title, a downloaded archive asks before it opens and is bounded (512 MB, 64 windows,
10,000,000 lines a pane), and logs and archives are written out before Tandem exits (and at logoff), a Restart session answer is only for the shell it was asked about, an archived pane never revives as a live shell at launch, and every automatic log name ends in .log. Deferred: the minors listed in
`docs/superpowers/rulings/m13q-rulings.txt`. Nothing of 13q was checked on screen during the build. Every kept quirk is listed in
`docs/acceptance/m13q-checklist.md` and ruled on in that file.
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
Milestone 14c added level 2 of SSH integration, iTerm2's framer (03: Shell > ssh > Disconnect to
have; 02: unameCommand to have, alternateSSHIntegrationScript to n/a as a DEBUG-only setting, ruling
8; the Clock's two SSH knobs act; the it2ssh, SendConductor, Configure SSH and shell-integration
rows stay partial for 14d and 14e). framer.py ships byte for byte and runs through `runpython`;
Tandem frames every write to the remote, pipelines commands, keeps the remote job list (autopoll)
for the Job Name, the close question and the CPU graph, shows server time with a Segoe Fluent globe,
fills the `sshIntegrationLevel`, `homeDirectory`, `shell` and `uname` variables, and Shell > ssh >
Disconnect ends the hop. Nested ssh hops frame (it2ssh run on the host above); Duplicate Session,
Duplicate Tab and arrangements reconnect through the hops Tandem's origin token vouched for (the
first hop; a nested hop is text the host above printed, so it is never jumped through, ruling); a
split of a session whose innermost hop is vouched opens ssh to that host directly, else the saved
profile (iTerm2 splits to the innermost host; splits never use the jump chain); and a session survives quitting Tandem (tandem-host keeps it) and recovers after a crash
("Recovering ssh connection\u{2026}"). Beyond iTerm2, for safety: a local command line is built
only from a chain Tandem's own token vouched for, hops pass one validator (no option that runs a
command or loads a configuration, every argument `%q`-quoted), in-band recovery turns on only after
such a hook and ends with the last hop, a restored conductor is never vouched, hook nesting is capped at 8, and every remote size is
capped. Every kept quirk is listed in `docs/acceptance/m14c-checklist.md` and ruled on in
`docs/superpowers/rulings/m14c-rulings.txt`.
Milestone 14d added SSH file transfer over the framer (03: Shell > ssh > Download Files... to have; no
other row changes status: the it2ssh, SendConductor and shell-integration rows stay partial for 14e).
Shell > ssh > Download Files... opens Tandem's remote file panel on the focused pane's host
(Connected Hosts, history, Go to Folder with completions, hidden files, sortable columns, search that
streams results); the picked items download into a fresh `%TEMP%` batch folder and Explorer shows them
selected. A Win-click on a file name in an ssh pane downloads it into Downloads (a folder arrives as
`<name>.zip`), asking Keep Both or Replace on a name clash; dropping files or a folder on an ssh pane
with Alt held asks, then uploads them over the conductor (a folder as `<name>.tgz`); the Composer's Tab
completes remote file and command names. Beyond iTerm2, for safety: every name from the remote passes
the one inert-file rule before the first byte is fetched and every file is marked with the Mark of the
Web (a drive that cannot hold the mark fails the download); a remote folder zip is checked by Tandem's
own parser, extracted by tar.exe in a Job Object whose watch kills it at the claimed size or at 1 GiB
free, and moved into place by a walk that never follows a link; a remote that sends more than it
claimed fails the download; in a pane with a framing conductor, only web and mail links open (user-written
triggers are exempt, G2); other text is never opened, run or revealed on this PC (Win-click, the
context menu, Open Selection, porthole links, the status bar folder menu), it can only be downloaded, and the pane's remote folder is never saved into an arrangement; a closed
upload alert is Cancel. Deferred: the transfers lists, Stop, the "Download using SSH Integration"
right-click item, Favorites and an in-app browser. Nothing of 14d was checked on screen during the
build. Every kept quirk is listed in `docs/acceptance/m14d-checklist.md` and ruled on in
`docs/superpowers/rulings/m14d-rulings.txt`.
Milestone 14e added ssh: URLs and closed milestone 14 (14a to 14e all built; 14d landed first). A profile's
General tab has "URL schemes:" with "Select URL Schemes..." and `ssh`; ticking it asks, naming the
current handler and every `HKCU` entry OK replaces (or "There is currently no handler."), then writes
only `HKCU` keys (never `HKLM`, never a protected user choice, which is reported and left for Default
apps). An `ssh://user@host:port/path` link from a browser or Run starts `tandem.exe --url`, which
hands the URL to the running Tandem over a per-user pipe and exits (with none running it starts and
opens the tab); a Win-click on an `ssh:` link opens a tab only in a local pane with a bound profile.
The tab runs ssh with iTerm2's `-l`, `-p`, `-t` and `cd <path>; exec $SHELL -l`, through SSH
integration when "Use SSH integration for ssh: URLs" is on; a user name outside iTerm2's set (and
14c's grammar) shows "Illegal Username". Leaving an ssh session restores the keyboard mode and offers
to restore the title (a per-pane host stack, iTerm2's). Rows: 02 `SshIntegrationForURLs`, sshSchemePath,
sshURLsSupportPath and restoreKeyModeAutomaticallyOnHostChange to have, and
validCharactersInSSHUserNames to partial (iTerm2's default accepts characters Tandem refuses);
01 URL schemes handled to partial (`ssh` only); 03 Remote host can control iTerm2, OSC 1337 wrap and
02 channelsEnabled are n/a (rulings 3 and 4; they leave the counts, as in 12e and 14d), and the menu
item shows unavailable with no milestone (`Status::NotApplicable`). Beyond iTerm2, for safety: the
URL command line is accepted only as exactly `tandem.exe --url <one ssh URL>` and anything else is
refused; a URL's words pass iTerm2's check and 14c's grammar, a host-less URL opens nothing, a URL
path is a remote folder, sent only as iTerm2's cd word, and never touches this PC's file system (a `//` or `\\` path drops the cd part),
at most 4 external links open per 10 s, and each shows a banner naming `user@host:port`; the key
mode and title restored are the ones in effect at the host report. Deferred: other URL schemes
(ftp, telnet and the rest of iTerm2's 14), `$$` substitutions in a bound profile's command, a
window-style click and a
local mode set in the same output chunk as the restore. Nothing of 14e was checked on screen during the
build. Every kept quirk is listed in `docs/acceptance/m14e-checklist.md` and ruled on in
`docs/superpowers/rulings/m14e-rulings.txt`.

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
| 14 | SSH integration (closed with 14e) | 03: iTerm2's SSH integration (it2ssh, the conductor and its remote scripts, file download), Shell > ssh; OSC 1337 it2ssh, SendConductor, EndSSH, wrap, and Env's use. | 5 |

## Windows notes that apply everywhere

- Cmd is the Win key and Option is Alt (spec 3.4, Robert's choice in milestone 3a). A keyboard
  hook takes Win chords while Tandem is in front; chords Tandem does not use stay with Windows.
- macOS-only rows (Touch Bar, Services menu, Spaces, Secure Keyboard Entry, Sparkle updates) get a
  Windows equivalent where one exists, or a written reason in the inventory.
- Each milestone updates the status column of the rows it closes, so the table above stays true.
