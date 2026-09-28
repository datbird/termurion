# Milestone 13d acceptance checklist

Build a release binary (`scripts/xt build --release`). Close any open Tandem window, copy
`tandem.exe`, `tandem-host.exe`, `tandem-git.exe`, `conpty.dll`, and `OpenConsole.exe` from
`C:\Users\datbi\src\tandem\target\release\` together, then run `tandem.exe`.

Nothing in this milestone was checked on screen while it was built, so every line here is a
first look. Setup: the tab bar settings are on Settings > Appearance > General ("Tab bar
location:"), Settings > Appearance > Tabs and Settings > Advanced > Tabs; the profile icon is on
Settings > Profiles > General ("Profile icon" and "Custom icon path"). Leave "Show tab bar even
when there is only one tab" off (the default) unless a check says otherwise. Run the shell checks
in a PowerShell session; the tmux checks need `tmux -CC` in WSL (milestone 8's setup). Put a PNG
of any size somewhere local (for example `C:\Users\datbi\Pictures\icon.png`). Mark each line pass
or fail.

## Dragging

| # | Check | Result |
|---|---|---|
| 1 | Open four tabs. Press on tab 1 and move less than 10 pixels: nothing moves. Keep dragging along the bar: the tab follows the pointer over the others and a 2-pixel accent-blue marker shows where it will land; release between tabs 3 and 4: tab 1 is now third. The press selected it, as any press does, and no close or title edit follows the release. | |
| 2 | Open a second window. Drag a tab of the first window onto the second window's bar and release: the tab moves there at the marked slot, the second window comes to the front with it selected, and the first window keeps its other tabs. | |
| 3 | Drag a tab out of the bar and release over the desktop: a new window opens with that tab, its top-left corner at the drop point. With "Show tab bar even when there is only one tab" on, drag a window's only tab along its own bar and release on the bar: nothing happens. Turn it off. | |
| 4 | Turn on Window > Lock Layout and try checks 1 and 3: the tab does not move and no window opens. Turn it off. | |
| 5 | In Advanced > Tabs turn off "Allow a tab to be dragged and dropped outside any existing tab bar to create a new window.": a tab dropped over the desktop stays where it was. Turn it back on. | |
| 6 | With two pinned tabs (Pin Tab, check 23) and two unpinned ones, drag a pinned tab far to the right: it stops at the last pinned slot. Drag an unpinned tab to the far left: it stops at the first unpinned slot. Drag a pinned tab over the desktop: no window opens; dragged onto another window's bar it moves there. | |
| 7 | Press on the "+" button and drag: the window moves with the pointer; a plain click on "+" still opens a tab. In Advanced > Tabs turn off "Allow dragging window by the Add Tab button in the tab bar?": a drag from "+" no longer moves the window. | |
| 8 | Set Advanced > Tabs "How far must the mouse move before a tab drag is initiated?" to 60 and restart Tandem: a tab starts to move only after about 60 pixels. Set it back to 10 and restart. | |

## Tab bar position and size

| # | Check | Result |
|---|---|---|
| 9 | With three tabs, set "Tab bar location:" to Bottom: the bar moves to the bottom edge and the sessions sit above it; hover, click, close button, right-click menu and "+" work there. | |
| 10 | Set it to Left: the tabs become rows 24 points tall down a side bar 150 points wide, each row with its title (left-aligned, starting after the close button's room even when the button is hidden), number and hover close button; a long title is cut at the row's end and never moves; there is no "+" on a side bar. The pointer over the bar's inner edge turns into the resize pointer; drag it to about 250 points: the bar follows. Quit and relaunch: the bar is still about 250 wide (Appearance > Tabs "Left (side) tab bar width" shows the new width). | |
| 11 | With the Left bar, open 40 tabs: the wheel over the bar scrolls the rows, and Win+Shift+] keeps the selected row in view. Turn off "Tab bar scrolls when tabs don't fit": the rows keep their height, the last row that fits becomes an overflow button across the bar's bottom, and it lists the rest. Turn it back on. | |
| 12 | Try to drag the side bar narrower than 50 points or wider than the window: it stops at 50, and at the window width less about 18 points. | |
| 13 | Set the location to Right: the bar is at the right edge; show the toolbelt (View > Toolbelt > Show Toolbelt): the toolbelt takes the far right and the bar sits just left of it. Right-click a row with rows below it: the menu reads "New Tab Below" and "Close Tabs Below". | |
| 14 | On a Bottom bar, rest the pointer on a tab: note where the tooltip shows (it is placed below the pointer and pulled back up on screen, so it may cover the bar); it must still be readable. Set the location back to Top. | |

## Full screen and the flash

| # | Check | Result |
|---|---|---|
| 15 | With three tabs, turn off Appearance > Tabs "Show tab bar in fullscreen" and enter full screen (View > Toggle Full Screen, Ctrl+Win+F): the bar is hidden and the sessions take its room. Press Win+Shift+U (View > Show Tabs in Fullscreen): the bar shows and the menu item is checked; press it again: it hides and the check clears. | |
| 16 | With the bar hidden in full screen, turn on "Flash tab bar when switching tabs in fullscreen" (enabled while "Show tab bar in fullscreen" is off or "Show tab bar even when there is only one tab" is off). Press Win+2 and let go of Win at once: the bar appears over the sessions at once, stays about one second, then fades out in about a quarter of a second, and the sessions do not move. | |
| 17 | Press Win+3 and keep holding Win for three seconds: the bar stays while Win is held and fades when Win is released. Press Win+1 while the bar is fading: it comes back fully and holds again. Click another application while the bar shows: it goes at once. | |
| 18 | Set Advanced > Tabs "How long in seconds to show tabs in fullscreen." to 3 and "Animation duration for fade in/out animation of tabs in full screen, in seconds." to 1: the flash holds three seconds and fades over one. Set them back to 1 and 0.25. Leave full screen. | |

## Window size when the bar shows or hides

| # | Check | Result |
|---|---|---|
| 19 | In a Normal window with one tab (no bar) in the middle of the screen, open a second tab: the bar appears and the window grows by the bar's height, its top-left corner fixed, so the session keeps its rows. Close the tab: the window shrinks back to its first size. Repeat three times: the size never drifts. | |
| 20 | Turn on Appearance > Tabs "Preserve window size when tab bar shows or hides" and repeat: the window keeps its size and the session loses rows instead. | |
| 21 | Also turn on "...but only if it would cause the window to go off-screen" (enabled only with the row above on). In the middle of the screen the window grows as in check 19. Move the window so its bottom edge is a few pixels above the taskbar, without touching it, and open a second tab: the window keeps its size; close the tab: it keeps its size again. Turn both off. | |
| 22 | With both off and the window a few pixels above the taskbar, open a second tab: the window grows only down to the taskbar (it stops at the work area). A maximized or snapped window never changes size when the bar shows or hides. | |

## Pinned tabs

| # | Check | Result |
|---|---|---|
| 23 | With four tabs, right-click tab 3 and choose Pin Tab: it moves to the front, 64 points wide (64 pixels at 100% display scale), with the pin glyph at its left and only the first character of its title; hovering it shows no close button and it shows no "Win+N" number. Pin tab 4 too: it lands right after the first pinned tab. | |
| 24 | Close a pinned tab with the tab menu's Close Tab: Tandem asks ("Close tab #1?" with "Tab #1 is pinned."), even with nothing running. Cancel. | |
| 25 | Quit Tandem with a pinned tab open: the quit question lists "Tab #1 is pinned." (and one line per other pinned tab, window by window). Cancel. | |
| 26 | Right-click a pinned tab: there is no Move to New Window, and the item reads Unpin Tab; Close Other Tabs and Close Tabs to the Right skip the pinned tabs. Unpin both. | |
| 27 | In Advanced > Tabs set "Width of pinned tabs in pixels." to 100: pinned tabs are 100 points wide. Set it back to 64. | |

## Groups

| # | Check | Result |
|---|---|---|
| 28 | With four tabs, select tab 2 and choose Window > Tab > Add Tab to New Group...: a "New Tab Group" window asks "Enter a name for this tab group." with the tab's title filled in and selected. Type `Build` and OK: a colored chip reading "Build" (bold) appears before tab 2, and a 1-point outline in the same color runs around the chip and the tab. Cancel on a second try does nothing. | |
| 29 | Right-click tab 3 > Add Tab to Group: the submenu has New Group... and "Build"; choose "Build": tab 3 joins, and "Build" shows a check in tab 3's submenu next time. Make a second group from tab 4: it gets the next color in iTerm2's order (blue, green, orange, purple, red, #50afbc, pink). | |
| 30 | Click the "Build" chip: the group collapses into a chip reading "Build  2" with a chevron, its tabs are hidden, and if a member was selected the selection moves to the nearest visible tab outside. Click it again: it expands. With every tab of the window in one group, the chip menu's Collapse Group is disabled. Collapse "Build" again, select the tab just before its chip and choose Session > Bury Session (a one-pane tab): the tab after it, inside "Build", is selected and the group expands; Session > Buried Sessions brings the buried session back as its own selected tab. | |
| 31 | Select tab 2 (inside "Build") and press Ctrl+Shift+Win+] (Window > Tab > Select Tab > Select Next Tab Group): the first tab after the group is selected. Ctrl+Shift+Win+[ goes back to the tab before the group. From an ungrouped tab they step one tab, like Select Next Tab, and a tab reached inside a collapsed group expands it. | |
| 32 | Select the group's first member and press Alt+Shift+Win+[ (Move Tab Left): the tab leaves the group and stays where it is, just before it; press it again: it moves one place left. Move an ungrouped tab right next to the group: the first press joins it to the group, and further presses move it within the group. | |
| 33 | Open a fifth tab, collapse "Build", select tab 1, and drag the fifth tab (not selected) onto the middle of the chip: the chip gets a 2-point accent-blue frame while the pointer is there; on release the tab joins the group, the group stays collapsed, and tab 1 is selected again. Drag the selected tab onto the chip instead: it joins and the group expands with it selected. Do both into another window's collapsed group too: a background tab leaves that window's selection as it was. With two collapsed chips side by side, a drop on the outer third of one next to the other lands between the two groups, also when the dragged tab sat between them. | |
| 34 | Drag the "Build" chip along the bar: the chip and its tabs move together and land at the marked slot; a drop inside another group's tabs snaps to that group's edge. Drag the chip to another window's bar: the whole group moves there. With another tab left in the window, drag it over the desktop: the group opens in a new window at the drop point with its last tab selected. | |
| 35 | Right-click the chip: New Tab in Group, Collapse Group, Rename Group..., Group Color, Duplicate Group, Move Group to New Window, Save Group as Window Arrangement..., Remove All Tabs from Group, Close Group, Close Other Tabs, Close Tabs to the Right. Try Rename Group... ("Rename Tab Group", seeded with the name), New Tab in Group (the new tab joins), Duplicate Group (a group "Build copy" appears after it), Close Group (one question "Close this tab group? (N tabs)" under "Close Tabs"). | |
| 36 | Remove Tab from Group (Window > Tab, or the tab menu) on a middle member: it leaves and moves just after the group. Remove All Tabs from Group dissolves the group. | |
| 37 | Pin a grouped tab: its whole group pins. Save the window with Window > Arrangements > Save Current Window as Arrangement, close the window, restore the arrangement: the pinned tabs, the groups, their names, colors and collapsed state come back. | |

## The tab menu and colors

| # | Check | Result |
|---|---|---|
| 38 | Right-click a tab (with three tabs): Select (a submenu "<title> #1" ... "#3"), Edit Session..., New Tab to the Right, Duplicate Tab, Close Tab, Close Other Tabs, Close Tabs to the Right, Move to New Window, Pin Tab, Add Tab to Group, Save Tab as Window Arrangement, Tab Color, with separators between sections and none doubled. A right press does not select the tab. With Lock Layout on, New Tab to the Right, Duplicate Tab, the Close items and Move to New Window are disabled. | |
| 39 | Tab Color: 13c's picker opens (Default, the swatches, Choose Color...); a swatch colors that tab. Group Color from the chip menu: the same picker, starting at the group's color; a swatch recolors the chip and the outline. | |
| 40 | New Tab to the Right opens a default-profile tab right of the clicked tab (in its group when it is grouped). Close Other Tabs and Close Tabs to the Right close without asking. | |

## Icons

| # | Check | Result |
|---|---|---|
| 41 | Set the profile's "Profile icon" to "Built-in Icon for Current App". At an idle PowerShell prompt each tab shows a "P" in a colored circle where the close button sits (pwsh has no built-in icon); hovering the tab shows the close button in its place instead. The same name always gets the same circle color. | |
| 42 | Run `git cat-file --batch` (git waits for input; Ctrl+C ends it): the tab shows the git icon in git's orange. Run `timeout /t 30`: a "T" circle while it waits. (`git log` at its pager may show the pager's book icon, since `less` is then the foreground job.) | |
| 43 | Turn off Advanced > Tabs "Use the running command's first letter as the tab's default icon if there isn't a built in one.": the letter circles go, the git icon stays. Turn it back on. | |
| 44 | Set "Profile icon" to Custom and "Custom icon path" to the PNG: the tab shows it in a 16-point box. A path that does not exist shows nothing, and so do `\\host\share\i.png`, `\/host/share/i.png` and a path under a folder link (`mklink /D`) to a share. "No Icon" shows none. | |
| 45 | With the git icon showing, create `%APPDATA%\Tandem\graphic_colors.json` containing `{"git": "#ff0000"}` and restart Tandem: the git icon is red. Delete the file and restart: orange again. | |
| 46 | Pin a tab that shows an icon: the pinned tab shows only the icon (no pin glyph, no title). The window's title bar keeps the Tandem app icon. | |

## Duplicate Tab

| # | Check | Result |
|---|---|---|
| 47 | Split a tab into two panes with different folders (`cd` in each), then Shell > Duplicate Tab: the copy opens right after the original with both panes, the same split and the same folders, and it is selected. In a group, the copy joins the group. | |
| 48 | Turn off Advanced > Tabs "When you duplicate a tab, should the copy open immediately to the right of the original tab?": the copy opens at the end instead. Turn it back on. | |

## tmux

| # | Check | Result |
|---|---|---|
| 49 | Attach `tmux -CC` with two windows as tabs, set the tab bar to Left: run `seq -s ' ' 1 400` in a tmux tab: the lines wrap at the visible edge of the pane and nothing is hidden under the bar. Drag the side bar wider: tmux's lines re-wrap to the narrower pane. Try Bottom and Right the same way. | |
| 50 | Right-click a tmux tab: there is no Duplicate Tab and no Pin Tab. Put a tmux tab in a group with a local tab and pin the local tab: nothing happens (a group holding a tmux tab never pins). | |
| 51 | Drag a tmux tab into another Tandem window: it moves whole. Detach and reattach: that tmux window opens in the window it was dragged to. | |
| 52 | New Tab to the Right on a tmux tab asks "What kind of tab do you want to open?"; New tmux Tab opens a tmux tab (at the end of the bar, kept item 70). Shell > Duplicate Tab on a tmux tab makes a local tab at the end. | |
| 53 | With "Built-in Icon for Current App", run `vim` in a tmux tab: the tab shows the vim icon. | |

## Kept bugs and rulings a person can see

| # | Kept behavior | Result |
|---|---|---|
| 54 | No floating image follows a tab dragged outside its window: the tab moves in its bar and a marker shows the slot. | |
| 55 | The side bar's width is saved when the drag ends, not while it moves; a click on the handle with no movement also saves the width shown. | |
| 56 | A torn-off window's outer top-left corner (title bar included) is at the drop point; iTerm2 places the window's tab bar there. | |
| 57 | A window's first layout does not fit the bar: a new window with an always-visible bar is one bar shorter in rows than its Columns x Rows. | |
| 58 | A window flush against a work-area edge counts as outside it (Windows' invisible resize borders), so "only if off-screen" keeps growing it; leave a few pixels (check 21). | |
| 59 | Turning "Show tab bar even when there is only one tab" on or off resizes one-tab windows by the bar. | |
| 60 | The flashing bar's text fades by blending toward the background; while it is more than half shown the session lines under it are hidden whole, so a line cut by the bar's lower edge blanks. Closing a background tab in full screen also flashes the bar. | |
| 61 | Unpinning the first of two pinned tabs puts it after the first unpinned tab, not at the start of the unpinned tabs (iTerm2's remove-then-insert). | |
| 62 | Under Lock Layout, Pin Tab beeps and does nothing (iTerm2 pins without moving). A pinned tab's title shows its first character, not its first composed character (a combining or emoji sequence shows only its first part). | |
| 63 | The group questions (Close Group, and the chip menu's Close Other Tabs and Close Tabs to the Right) read "(1 tabs)" for one tab, as iTerm2's, in an alert window with Close and Cancel (since the merge with 13b; closing the alert is Cancel). | |
| 64 | Duplicate Tab and Duplicate Group beep under Lock Layout (iTerm2 disables the menu items). | |
| 65 | The collapsed chip's chevron is the Segoe Fluent Icons chevron, not a stroked path; the group name is bold at 13 points. | |
| 66 | The Select submenu shows each tab's title on one line, with no subtitle. | |
| 67 | A chip drag under Lock Layout moves while dragging and snaps back on release (a tab drag is refused at the press). A locked window refuses a tab or group dropped into it. | |
| 68 | The icon sits in the close button's place: hidden while the hover close button shows, never shown in the key window with "Should tab close buttons always remain visible?" on (in a window that is not key it shows at half strength, YS:896-905). A centered title does not shift for the icon unless it truncates. | |
| 69 | Icon files and the two map files on a network path, however spelled, or reached through a link to one, are never read; a map with a trailing comma is read anyway. | |
| 70 | A tmux tab opened by New Tab to the Right lands at the end of the bar, not next to the clicked tab. | |
| 71 | An icon or map file over 8 MiB, or a picture over 4096 pixels a side, shows nothing (a Tandem limit; iTerm2 has none). | |
| 72 | A Custom icon path naming a Windows device (`COM1`, `NUL`, `CON.png`, `C:\Users\datbi\Pictures\LPT1.png`, `\\.\COM1`) or a folder shows no icon, and the window never stops responding (a Tandem limit: only regular files are read). | |
