# Milestone 11c acceptance checklist

Build a release binary (`scripts/xt build --release`). Close any open Tandem window, copy
`tandem.exe`, `tandem-host.exe`, `tandem-git.exe`, `conpty.dll`, and `OpenConsole.exe` from
`C:\Users\datbi\src\tandem\target\release\` together, then run `tandem.exe`.

Setup: install Python 3 and run `py -m pip install iterm2` in a plain venv (pin `websockets<14`
if `extra_headers` raises a TypeError, as in the m11a and m11b checklists). Turn on Settings >
General > Python API > "Enable Python API" and accept the warning; turn `setCookie` on. Open a
window with a plain tab and a session bound to a key with action "Invoke Script Function..." and
a pointer binding with the same action; add a trigger whose parameter is an invocation such as
`myfunc(arg: matches[1])` against a regex with a capture group. Open a second window with a tmux
tab (`ssh` to a host with tmux, then `tmux -CC`). Add a status bar with an Interpolated String
component and a Call Script Function component to one session's bar. Each check below runs a
short script (an `@iterm2.RPC` function, a title provider, or a `StatusBarComponent`, as the check
needs) against `iterm2.run_until_complete` with the stock `iterm2` package, and states what
Tandem is expected to show.

| # | Check | Result |
|---|---|---|
| 1 | An `@iterm2.RPC` function called from the key binding (Invoke Script Function...) with a session focused runs in that session's scope, origin "Key Binding". | |
| 2 | The same function called from the pointer binding (kInvokeScriptFunction) runs in the clicked pane's scope and does not change focus. | |
| 3 | The trigger's invocation runs once per match on a line with more than one match; `matches[1]` is the first capture group; a group that did not match reads as a huge out-of-range location rather than an empty string. | |
| 4 | The status bar's Call Script Function component (Label, Function call, Timeout knobs) invokes on click, side effects on, origin the button's label. | |
| 5 | Typing the invocation directly into Open Quickly (View > Open Quickly) and picking it shows "Function Call Result" with "<invocation> returned:\n<value>"; a key-60 binding with an apply mode such as "All sessions" and no terminal window focused still runs once, in the app's scope. | |
| 6 | `iterm2.async_invoke_function` reaches the app, a session, a tab, and a window (four separate calls); a bad id in any of them answers "No such session" / "No such tab" / "No such window" as appropriate. | |
| 7 | A function that raises shows "<origin> Function Call Failed" with "Error running \u{201c}<invocation>\u{201d}:\n<message>", OK, and a "Suppress this message for ten minutes" checkbox; checking it and clicking OK silences repeats of that exact failure for ten minutes, and a burst of identical failures before the alert shows queues only one. | |
| 8 | A function that sleeps past its registration's timeout shows "Timeout (<n> sec) while <...>" with iTerm2's exact wording for whether the timeout hit while evaluating arguments or while waiting for the return. | |
| 9 | Stopping the script mid-call (Ctrl+C the script, or closing its console window) fails the outstanding call with "Script terminated during function call". | |
| 10 | `iterm2.alert(title, subtitle, buttons)` shows a dialog with those buttons and answers 1000 plus the index of the one clicked; closing the window answers Cancel if present, else the first button. | |
| 11 | `iterm2.get_string(title, subtitle, placeholder, defaultValue)` shows one dialog with a single OK button (no Cancel), titled in the window's heading, and returns the field's text. | |
| 12 | `Session.async_set_name(name)` renames the session; a later program title (`printf '\e]2;x\a'`) can still replace it. | |
| 13 | `Tab.async_set_title(title)` and `Window.async_set_title(title)` set their override titles. | |
| 14 | `Session.async_run_tmux_command("list-windows")` on the tmux tab's own gateway session returns tmux's real output; the same call on a non-tmux session fails "Not a tmux integration session". | |
| 15 | `Session.async_run_coprocess(commandLine)` starts a coprocess shown in the session, the same as the menu's Run Coprocess; `async_stop_coprocess()` stops it and answers whether one was running. | |
| 16 | A title provider chosen in Settings > Profiles > General > Title > Function: while the script runs, the session's title tracks the provider's returned text (trimmed, blank shown as a single space); stopping the script leaves the last title in place until a reference the invocation read changes, or another title provider registers, not an immediate "\u{2026}"; a session that never got a first answer shows "\u{2026}". | |
| 17 | A `StatusBarComponent` script with a Checkbox, a String, a Color, and a PositiveFloatingPoint knob: added to a bar through the setup window's Configure sheet, its knob values round-trip; a `cadence` knob re-polls on that interval; a click handler runs `__<id>__on_click(session_id: "<id>")`. | |
| 18 | `set_status_bar_component_unread_count(identifier, count)` draws a red badge with the count at the component's right edge, clipping the component's own text rather than covering it. | |
| 19 | A status bar component whose function raises shows the ladybug in place of its text; clicking it shows the persistent "Status Bar Script Error" alert. | |
| 20 | `async_open_popover(session_id, html, size)` from a component opens a popover showing the HTML's plain text (tags stripped, nothing rendered, no script runs). | |
| 21 | A registered context menu provider (role CONTEXT_MENU_ITEM) shows as an item in a session's right-click menu, after the custom actions and before Split Pane Vertically, with separators on both sides; picking it invokes with the session's scope, origin `Menu Item \u{201c}<title>\u{201d}`. | |
| 22 | `async_register_web_view_tool` lists the tool under the Toolbelt menu's hamburger picker and it toggles on and off like a built-in tool; shown, its panel is empty (no WebView2 host yet). | |
| 23 | The Interpolated String component's "String Value:" field with `\(1 + 2)` shows "3"; with `\(user.x?)` and no `user.x` set shows "" (the lax default), not an error. | |
| 24 | The `selection` variable read through a script (for example `Session.async_get_variable("selection")`) is capped at 100 characters (Settings > Advanced > "Maximum number of bytes ... to Python API"); `selectionLength` still reports the full count. | |

## Kept quirks and rulings a person can see

| # | Item | Result |
|---|---|---|
| 25 | `\(nope + 1)`, with `nope` undefined, fails with "Subexpression error: Nil operand in binary operation (+)", naming no variable. | |
| 26 | An unclosed `\(` in an interpolated string (for example `a \(session.hostname`) is left as literal text unchanged, not an error. | |
| 27 | `[1, 2] == [1, 2]` is a syntax error; comparing arrays needs two calls returning arrays (`f() == g()`). | |
| 28 | `matches[7]` past the capture count reads as "" with the lax setting on (default), and an index-out-of-range error with it off. | |
| 29 | `\(a ?? b)` still works as a Tandem extension (first non-empty text wins); it is not part of iTerm2's own language. | |
| 30 | Turning off Settings > Advanced > Scripting > "Should references to undefined variables in interpolated strings be converted to empty string?" makes a bare `\(bogus)` an error; a function call's own arguments are always strict regardless of the setting. | |
| 31 | A trigger's own parameter field, a badge, an Automatic Profile Switching rule, a coprocess command template, and Copy Interpolated String each refuse a function call in their text with "Function calls are not allowed here.", even though those fields now accept the rest of the expression language. | |
| 32 | The Title Function popup lists "Built-in components" (not a live preview of the built-in title) plus every registered provider; choosing "Built-in components" resets Title Components to 2 (Job). | |
| 33 | A dynamic web tool registered by `async_register_web_view_tool` shows in the Toolbelt menu and toggles, but its panel draws nothing; a WebView2 host is a documented gap for a later milestone. | |
| 34 | An HTML-format status bar component, and any popover's HTML, shows plain text with tags stripped, not rendered markup. | |
| 35 | The Open Quickly invocation row uses the Segoe Fluent Icons Code glyph. | |
| 36 | `iterm2.get_string`'s dialog has one OK button; iTerm2 shows OK and Cancel. | |
| 37 | `set_name` sets the session's title through the same path a program's OSC 2 title uses, so a later program title can still replace the name where iTerm2's own profile Name would persist under title components. | |
| 38 | `with_alpha` and the other hex-color built-ins are strict about every digit being hex; a partially valid iTerm2 string like `#ff00zz` is refused rather than half-read. | |
| 39 | `move_tab_to_window` called with no tab in scope (for example from a key binding) fails "Tab has no window"; only a missing `tab_id` argument gives "No such tab". | |
| 40 | `focus`, the clippings functions, `get_profile_property`, and `set_status` can target a buried session; `paste` cannot. | |
| 41 | Picking a context menu provider item on a session that closed in the meantime shows a false Function Call Failed alert instead of being refused quietly (known bug, parked; a tab or NUL character in a provider's display name also garbles its menu label). | |
| 42 | Per-pane title bar menus (not yet built), the Title Function popup's title preview, and a tmux Custom title's prefix are documented gaps for milestone 13, not bugs in this milestone. | |
