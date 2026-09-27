# Milestone 11b acceptance checklist

Build a release binary (`scripts/xt build --release`). Close any open Tandem window, copy
`tandem.exe`, `tandem-host.exe`, `tandem-git.exe`, `conpty.dll`, and `OpenConsole.exe` from
`C:\Users\datbi\src\tandem\target\release\` together, then run `tandem.exe`.

Setup: install Python 3 and run `py -m pip install iterm2` in a plain venv (pin `websockets<14`
if `extra_headers` raises a TypeError, as in the m11a checklist). Turn on Settings > General >
Python API > "Enable Python API" and accept the warning; turn `setCookie` on so a script run from
a Tandem session's own shell connects with no prompt. Open a two-window layout: window 1 has a
plain tab with a vertical split; window 2 has a tmux tab (`ssh` to a host with tmux, then
`tmux -CC`) and one buried session. Have a shell with shell integration installed in the plain
tab, so prompt marks exist.

Each check below runs a short script against `iterm2.run_until_complete` with the stock package
(no local modification), unless noted otherwise, and states what Tandem is expected to show.

| # | Check | Result |
|---|---|---|
| 1 | `async_send_text` on the split's shell types the text there, as if it had been typed; on a session in a broadcast domain it also reaches every other pane of that domain unless `suppress_broadcast` is set. | |
| 2 | `async_inject` on a session prints the given bytes as if they were program output, without going to the shell (a following prompt does not see it as typed input). | |
| 3 | `async_activate` on a session in the other window selects that window's tab and pane; the window comes to the front. | |
| 4 | `async_activate`'s raise brings a window to the front without activating Tandem itself (no taskbar flash claims focus); repeat it on a hidden hotkey window and it rolls in, which may take focus. | |
| 5 | `async_close` on a session with `force` unset asks first (Tandem's own confirmation) and returns `RPCException` / `USER_DECLINED` on Cancel; with `force=True` it closes with no prompt. | |
| 6 | With Lock Layout on for a tab, `async_close` on a session or tab in it (force unset) is refused (USER_DECLINED); a window close and any `force=True` close still work. | |
| 7 | `Window.async_create_tab(profile_customizations={"Background Color": {...}, "Command": "..."})` starts the new tab's pane with that background color and command; afterward, the saved profile it copied from is unchanged in Settings. | |
| 8 | `Window.async_create_tab(index=0)` places the new tab first; an out-of-range index still creates the tab and answers `INVALID_TAB_INDEX`. | |
| 9 | `Session.async_split_pane(vertical=True, profile_customizations={...})` splits and customizes the new pane without touching its saved profile. | |
| 10 | `Session.async_split_pane(before=True)` splits, and the new pane appears on the near side of the divider (a split followed by a swap). | |
| 11 | `async_get_screen_contents` on the split matches what is on screen, including a wide character (CJK) and a combining accent. | |
| 12 | `ScreenStyle` of a colored `ls` output reports the right foreground/background per run of characters. | |
| 13 | `async_get_prompt` right after Enter (nothing typed yet) shows an empty command range at the prompt; typing `ls` (not yet run) grows the range to the typed text, ending at the cursor. | |
| 14 | `async_list_prompts` after three commands lists all three prompt marks, oldest first. | |
| 15 | A `Session.async_get_screen_contents()`-driven screenshot (`grab_screenshot`) saved to a file opens as a picture of the pane; when the view is scrolled to the bottom, the cursor appears in the picture. | |
| 16 | `async_set_variable("user.x", 5)` then `async_get_variable("user.x")` returns the integer `5`, not the string `"5"`. | |
| 17 | A `VariableMonitor` on `session.columns` reports a change when the window (and so the pane) is resized. | |
| 18 | `async_set_grid_size(cols, rows)` resizes the window so the pane's grid matches; `async_get_property("grid_size")` reads back one less in each dimension (iTerm2's own quirk, kept). | |
| 19 | `async_get_profile` and `async_list_profiles` round-trip a profile's properties; `async_set_property` on a session (frame, fullscreen) changes the window as expected. | |
| 20 | `PreferenceKey` get and set (for example a non-secret General preference) round-trips, and the change is visible in Settings after the set. | |
| 21 | `PreferenceKey` set of `setCookie` to `True` shows the Confirm alert in Tandem; the request answers OK immediately, before the alert is answered; only clicking OK actually turns it on. | |
| 22 | `PreferenceKey` set of a stored-consent key (for example the API's own "don't ask again") answers `INVALID_VALUE` and changes nothing. | |
| 23 | `async_save_window_arrangement` then closing the window then `async_restore_window_arrangement` brings the window back with its tabs and splits. | |
| 24 | `async_restart` on a session whose shell has exited restarts it; on a tmux pane, a gateway session, or a still-running session (with `only_if_exited`) it is refused. | |
| 25 | Changing split sizes in the UI, then `Tab.async_update_layout()` from a script with new grid sizes, resizes the panes' ratios inside the tab without resizing the window itself. | |
| 26 | `async_set_tabs` (or `Tab` reorder) moves a tab from window 1 to window 2; it lands at the requested index and becomes window 2's selected tab. | |
| 27 | `RestartSession` and `SetTabLayout` both work on a session that is buried, or in a background (non-frontmatter) tab, not only the frontmost one. | |
| 28 | `MainMenu.async_select_menu_item("New Tab")` (by title, resolved to its identifier) opens a new tab, the same as picking it from the menu. | |
| 29 | The three added menu chords work from the keyboard: Shell > tmux > Dashboard (Win+Ctrl+Shift+Enter), Edit > Find > Select Matches (Win+Alt+Enter), View > Maximize Active Pane (Win+Shift+Enter); View > Zoom Out has no Esc menu chord, but pressing Esc in a zoomed view still zooms out. | |
| 30 | `async_get_selection` / `async_set_selection` round-trips a highlighted range on the pane's screen; setting a second sub-selection in the same request answers `REQUEST_MALFORMED`. | |
| 31 | `async_set_broadcast_domains` turns on Broadcast Input to the given sessions of one window; giving overlapping domains does not refuse the request, it silently keeps a session in only the last domain naming it. | |
| 32 | In the tmux tab, `async_send_command("list-windows")` returns tmux's real output. | |
| 33 | Hiding then showing a tmux window through the API (`SetWindowVisible`) matches Session > tmux > (the window's name) in the menu. | |
| 34 | `async_create_window` on the tmux connection always gets an answer, even when tried right as the new tmux window is being closed from the tmux side. | |
| 35 | `ColorPreset` list is empty, and getting any named preset (even a real iTerm2 preset name) answers "not found" (kept until milestone 13). | |
| 36 | A `KeystrokeMonitor` on the split's session prints Alt, Win (Cmd) and the four arrow keys, each with iTerm2's macOS key code. | |
| 37 | A `KeystrokeFilter` for Ctrl+C on that session swallows it (the shell never sees it), while the `KeystrokeMonitor` on the same session still reports it. | |
| 38 | Ending the filter script (Ctrl+C the script itself, or closing its window) immediately lets Ctrl+C reach the shell again — a filter dies with its script. | |
| 39 | A key with no macOS key code (for example F21, if the keyboard or a remap can send it) is not caught by a `KeystrokeFilter` on key codes, though a filter on its character still catches it. | |
| 40 | A `ScreenStreamer` on the split's session wakes as soon as new output appears (running `echo hi`). | |
| 41 | A `PromptMonitor` on the split's session sees PROMPT when Enter is pressed, COMMAND_START once the command begins running, and COMMAND_END when it finishes. | |
| 42 | A `CustomControlSequenceMonitor` sees the event for `printf '\e]1337;Custom=id=me:hi\a'` (with the trailing colon) but sees nothing for `printf '\e]1337;Custom=id=me\a'` (no colon). | |
| 43 | A plain script using only the stock `iterm2` package (no local modification) can do all of the above over `ws://localhost:1912` with `setCookie` on, matching the checks above end to end. | |

## Kept quirks and rulings a person can see

| # | Item | Result |
|---|---|---|
| 44 | `GetProperty("grid_size")` reports width and height one less than the pane's real grid, as iTerm2 does. | |
| 45 | `SetProperty` on session "all" changes only the first session found, not every session. | |
| 46 | A `VariableRequest` for window scope "all" answers `WINDOW_NOT_FOUND` rather than acting on every window. | |
| 47 | Subscribing to VARIABLE_CHANGE on "all" twice from the same script, then unsubscribing once, removes both copies and stops all future coverage for that script; a second script's own "all" subscription is untouched. | |
| 48 | `SetBroadcastDomains` with overlapping session lists never answers `BROADCAST_DOMAINS_NOT_DISJOINT`; it silently keeps only the last domain naming a shared session. | |
| 49 | An image (for example `imgcat` of a PNG) reports `ITERM2` in `ScreenStyle`, with 0 code points and no foreground/background color; no cell ever reports `KITTY`. | |
| 50 | A colored `ls` run's blink and guarded style flags are always false, since alacritty tracks neither. | |
| 51 | GetPreference of a preference that was never set answers the JSON text `"null"`, not an unset/missing field. | |
| 52 | A boolean profile property or preference (for example a top-level Settings toggle) reads back as `"1"` or `"0"` in its JSON, not `"true"`/`"false"`. | |
| 53 | `async_list_profiles` / `GetProfileProperty` with no keys named returns every schema key plus Guid, Original Guid and the other keys iTerm2 has no default for. | |
| 54 | An unrecognized `SavedArrangementRequest` action value is treated as RESTORE, not refused. | |
| 55 | `SavedArrangement` save silently replaces an existing name with the same one; the menu's own Save always asks first. | |
| 56 | Setting a WORD, SMART, LINE or WHOLE_LINE selection, then reading it back, reports the mode as CHARACTER (the exact range is kept, the mode name is not). | |
| 57 | A hyperlink id that looks like Tandem's own generated form (digits followed by `_alacritty`) is never reported, whether Tandem generated it or a program happened to send that exact text. | |
| 58 | Sending a tmux command whose text contains a raw CR or LF desynchronizes later tmux replies, exactly as it would against a real iTerm2 with tmux integration. | |
| 59 | A prompt's command range, read right after Enter with no text typed, is empty rather than one character wide; typing text (not yet run) grows it to the cursor. | |
| 60 | A profile-change notification subscription is accepted by iTerm2's own routing rules but never actually fires, even from Tandem, since the stock `iterm2` package cannot send that subscription type at all. | |
