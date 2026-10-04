# Milestone 13h acceptance checklist

Build a release binary (`scripts/xt build --release`). Close any open Tandem window, copy
`tandem.exe`, `tandem-host.exe`, `tandem-git.exe`, `conpty.dll`, and `OpenConsole.exe` from
`C:\Users\datbi\src\tandem\target\release\` together, then run `tandem.exe`.

Nothing in this milestone was checked on screen while it was built, so every line here is a
first look. The pure parts (the title bar geometry and colors, the pick halves, the drop plans, the
tab slot rules, the menu pick rules, the tmux command strings) are unit tested. The drawing, the
glyphs, the mouse paths, the drags across windows and the whole cross-window move were not, so
read those rows twice. Setup: the two switches are Settings > Appearance > Panes; "Shortcut to
choose a split pane" is Settings > Keys > Navigation Shortcuts; the colors and the Alt rule are
Settings > Advanced ("Windows" and "Mouse"). Use a tab with three panes unless a row says
otherwise, and have two windows open (each with two tabs, one tab split) for the rows that cross
windows. Mark each line pass or fail.

## Title bars: showing, room and the title

| # | Check | Result |
|---|---|---|
| 1 | Split a tab: each pane gets a title bar at its top (on by default). A tab with one pane has none. Look at light and dark mode: the bar is a gray (darker for the focused pane, lighter for the others in Dark; the reverse in Light), with a thin line under it. | |
| 2 | Turn Show per-pane title bars off: every bar goes and the panes grow into the room at once, in every window including ones in the background. Turn it on: the bars and the old sizes come back. | |
| 3 | "...even if there is only one pane" is greyed out while the first switch is off. With the first on, turn it on: a lone pane gets a bar. Turn it off: it goes. | |
| 4 | The label shows the pane's job or title (run `vim`, then exit; set a title with `printf '\e]2;hello\a'`). A long title ends in an ellipsis and is never cut through an accented letter or a flag. | |
| 5 | Settings > Keys > Navigation Shortcuts > "Shortcut to choose a split pane" = Alt+Number: each pane's title reads "Alt+1   ..." in pane order, up to 8 panes (the ninth has none). With Win+Number or Ctrl+Number the prefix follows. No Shortcut: no prefix. | |
| 6 | Hostile titles. In a pane run `printf '\e]2;abc\xe2\x80\xaeXYZ\xe2\x80\x8b\r\nline2\a'` (a right-to-left override, a zero-width space, a line break) and a 10 000 character title (`printf '\e]2;%s\a' "$(head -c 10000 /dev/zero | tr '\0' x)"` in WSL or a PowerShell equivalent). The bar shows ONE clean line in left to right order, never a second line, the close and menu glyphs stay where they were, and the window does not slow down. | |
| 7 | With Show per-pane title bars on and "Show status bar" with "Separate status bars per pane" and the bar at the Top: each pane's status bar sits inside its title bar (no label, the title bar's color behind it, no edge line, the bar's own layout background wins). Bottom position: the bars stay under the panes, the title bars show the label. | |
| 8 | Colors. Set Advanced paneTitleBarBackgroundColor to `#336699`: the bars take it (the unfocused ones a little dimmer) and the text turns to the contrast. Set paneTitleBarTextColor to `#ffcc00`: the label turns that color. Try an invalid value (`336699`, `#12`): the default colors stay. Empty: default. A pane with a tab color shows a tint of it. | |
| 9 | Dimming. Turn on Dim Background Windows / Dim Inactive Split Panes (Advanced) and click another pane: the unfocused panes' title text dims. A pane that Broadcast Input reaches stays lit. | |
| 10 | Minimal and Compact themes (Settings > Appearance > General > Theme): the Minimal bar uses the pane's background and has no line under it. On a window with transparency the Minimal bar stays solid (kept, ruled). | |
| 11 | Glyph drawing: the close glyph at the left and the menu glyph at the right (dimmer) are Segoe Fluent Icons, sit centered in the bar's height, are not cut at the edge, and have the same optical size at 100 percent and at 150 percent scaling. Note any that look off center or too large or small (the size is a guess, the centering uses the em box). | |
| 12 | Zoom a pane (Maximize Active Pane): it keeps its title bar at the top. Un-zoom: all the bars return. A window opened with "even if only one pane" is about one title bar short of its asked rows (kept, ruled). | |

## Title bar clicks and the menu

| # | Check | Result |
|---|---|---|
| 13 | A click on a bar focuses that pane (the bar brightens). It never selects text or sends a click to the program. Middle click focuses it too and does nothing else. | |
| 14 | A double click on the bar maximizes the pane; another restores it. In a tmux tab the pane zooms through tmux (`tmux list-panes -F '#{pane_id} #{window_zoomed_flag}'` agrees). A drag after the first press never counts as a double click. | |
| 15 | The close glyph closes the pane on mouse release (with the confirmation when one applies: a running job). Press it, move off the glyph and release: nothing closes. With Lock Layout on: a beep, the pane stays. | |
| 16 | The menu glyph (and, separately, a right click anywhere on the bar) opens the pane's menu on release. It has Toggle Broadcasting Input (Stop Coprocess while one runs), a script's context menu items, Copy, Paste, Split, the Move Session group (Move Session to Tab only when the tab has several panes), Swap With Session..., Maximize, Select All, Clear Buffer, Edit Session, Close. No link, fold or Smart Selection items. | |
| 17 | Script context menu provider (the m11c provider sample): right click the bar of pane B while pane A is focused. The sample's item is there; pick it: the script's output names B (never A). Do it again after dragging B into another tab before picking: it still reaches B, or nothing, never A. | |
| 18 | Menu on an unfocused pane: right click pane B's bar while A is focused. Pick Paste, Split Vertically, Clear Buffer, Select All and Close one after another (re-focus A each time): each acts on B; afterwards the focus is back on A (except Split, which focuses the new pane, and a maximized B). Pick Move Session to Window on B: B moves, A keeps the focus. | |
| 19 | With a menu open on B, close B from the keyboard (or let its job end) and pick a command: nothing happens, nothing else closes or changes. | |
| 20 | The top 3 pixels of a lower pane's title bar still grab the divider above it; a press anywhere lower on the bar is the bar's. | |

## Pick mode (Move Session to Split Pane, Swap With Session)

| # | Check | Result |
|---|---|---|
| 21 | Session > Move Session > Move Session to Split Pane: the source pane turns green with "Select a destination pane"; the pointer over another pane shows a half in dark red (west, east, north or south by the nearest edge, with iTerm2's hysteresis) with "Click to move source pane to this split". The text sits over the fill and no status bar or title bar text shows through it. Both look right at 100 and 150 percent scaling. | |
| 22 | Do it with two windows open: the source shows in its window and the other window's panes show halves when the pointer is over them (the other window's last drawn half may linger until the pointer returns to it, kept). Click a half in the other window: the pane moves there and is focused; a source tab left empty closes (and an emptied window closes). | |
| 23 | Click the source pane: the pick is cancelled. Click a pane's title bar (the whole pane counts): treated as that pane. Click with no half showing: the pane moves BELOW the target (iTerm2's rule). | |
| 24 | The pane menu's Swap With Session...: the target shows its full pane with "Click to swap source pane with this one"; a click swaps the two (also across tabs and windows). | |
| 25 | The Move Pane pointer action (Settings > Pointer, set a button to Move Pane) and a key bound to Move to Split Pane start the same pick. | |
| 26 | Lock Layout (Window menu) on: starting the pick beeps and does not start. A destination window with Lock Layout on shows no half and a click there does nothing (a deliberate tightening, ruled). | |
| 27 | End of a pick: New Tab, Split, New Window or Duplicate (from the person) cancel it; a script that splits or opens a tab (the API, an orchestration launch) does not. Closing the source ends it. Starting it closes any Instant Replay panel in every window. | |
| 28 | tmux: `tmux -CC` with two tabs and split panes. Move a pane to a half of another pane in the same session: tmux moves it (`tmux list-panes -a` agrees, the layout follows and no size glitch shows). Swap two panes: tmux swaps. Two different tmux sessions refuse. The gateway pane is neither a source nor a target. | |
| 29 | Broadcast Input on in a tab: move a pane inside the same tab or window: the panes that stayed still receive the broadcast. A pane moved to another window or tab drops out of the old domain only. | |

## Dragging a title bar

| # | Check | Result |
|---|---|---|
| 30 | Press on a bar and drag 4 points or more: the pick look appears (source green, halves dark red); release over another pane's half: the pane moves there, focused. Release on its own pane or on a bar: nothing moves. A short drag (under 4 points) is only a click. | |
| 31 | Drag onto a pane of another window (a different window under the pointer): the half shows there; release moves the pane into that window, focused, and that window comes to the front. | |
| 32 | Release over a window's tab bar (its own or another window's): the pane becomes a new tab at that slot; never among pinned tabs, never inside a collapsed group (it lands after the group). The group pill itself does not join the group (kept). | |
| 33 | Release over the desktop (outside every Tandem window) or over another program's window: a new Tandem window opens with its top left at the pointer (it may reach past a monitor edge, kept). Another program's window is not treated as ours. | |
| 34 | A lone pane (with "even if only one pane"): dragging its bar moves the window (refused, with nothing happening, for a Centered, docked or full screen window). It cannot be dropped onto another pane by drag (use Move Session to Split Pane; kept, ruled). | |
| 35 | Advanced RequireOptionToDragSplitPaneTitleBar on: a plain drag does nothing (a click still focuses); an Alt drag moves the pane. The lone pane's window drag needs Alt too. Off: a plain drag moves it. | |
| 36 | Lock Layout on: a drag beeps and never starts. Dropping into a Lock Layout window is refused with a beep. | |
| 37 | tmux pane dragged onto another pane's half in its session: tmux moves it. Dropped onto a tab bar (its own window's or another window's): it breaks out into a tmux window of THAT window, next to the tab at the drop slot (a window with no tab of the same tmux session refuses with a beep). Dropped outside every window: the new window opens at the pointer. | |
| 38a | Ending a drag without a release: start a title bar drag and press Esc, then another and Alt-Tab away, then another and let go of the button outside any Tandem window (or take the mouse away with a screenshot tool): each time the overlays vanish in every window, nothing moves, and the next mouse move starts nothing. | |
| 38 | Instant Replay open on the dragged pane or a loaded recording: refused (a beep), nothing moves. | |

## Colors and sizes

| # | Check | Result |
|---|---|---|
| 39 | The six split pane colors (Advanced, "Windows"): set splitPaneSourceFillColor, splitPaneSourceBorderColor, splitPaneSourceInnerBorderColor, splitPaneTargetDropFillColor, splitPaneTargetDropBorderColor and splitPaneTargetDropInnerBorderColor to six distinct hex colors (`#ff0000`, `#00ff00`, `#0000ff`, `#ffff00`, `#00ffff`, `#ff00ff`), start a pick: the source and the half show them. An empty or invalid value shows iTerm2's green, red, white and black. | |
| 40 | tmux tabs with title bars keep tmux's sizes: open `tmux -CC` with a vertical stack of three panes; `tmux list-panes -F '#{pane_id} #{pane_width}x#{pane_height}'` equals each pane's visible grid (no scroll glitches, no row missing); turn the title bars off and on, and change Advanced statusBarHeight: the sizes follow at once, also in a parked window after you switch to it. | |

## Kept quirks and rulings a person can see

| # | Check | Result |
|---|---|---|
| 41 | The lock glyph on a bar does nothing yet (13i brings Lock Pane). | |
| 42 | A drag or pick drop is refused for a tmux gateway pane. | |
| 43 | Tandem's title bar menu has no per-pane link, fold or Smart Selection items (no click point). | |
| 44 | The title bar uses `shown_name` rules: control, bidi and zero-width characters are dropped and a title is cut at 200 characters before it is fitted to the bar. | |
| 45 | Soft hyphens, tag characters and variation selectors are not filtered from a title (they have no width). | |
| 46 | Label width counts each character's width; a text-style character followed by a variation selector may overflow the label by one cell, clipped at its edge. | |
| 47 | Context menu commands run on the menu's pane without moving the focus; a command whose pane is gone does nothing. | |
