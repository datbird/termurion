# Milestone 13f acceptance checklist

Build a release binary (`scripts/xt build --release`). Close any open Tandem window, copy
`tandem.exe`, `tandem-host.exe`, `tandem-git.exe`, `conpty.dll`, and `OpenConsole.exe` from
`C:\Users\datbi\src\tandem\target\release\` together, then run `tandem.exe`.

Nothing in this milestone was checked on screen while it was built, so every line here is a
first look. Setup: the window rows are on Settings > Appearance ("Windows") and Settings >
Advanced ("Windows"); "Each tab may have its own window title" is on Settings > General
("Window"); the profile rows are on Settings > Profiles > Window and Text. The border checks need
Windows 11 (Windows 10 shows no border). There is no Theme popup until milestone 13e, so a
Minimal window needs `TabStyleWithAutomaticOption = 5` in `%APPDATA%\Tandem\settings.toml` with
Tandem closed. Window Types 4 and 14 are set by hand in `%APPDATA%\Tandem\profiles.toml` (the
Window style popup shows them blank). Run the shell checks in a PowerShell session. Mark each
line pass or fail.

## Border and corners

| # | Check | Result |
|---|---|---|
| 1 | Turn on Appearance "Heavy border around windows in light mode" with the theme light: the window gets a thin border; switch to a dark theme: the border goes away. Turn Advanced `BordersOnlyInLightMode` off: the border shows in dark mode too. | |
| 2 | Set Advanced `WindowBorderColor` to `#ff000080` and `WindowBorderColorUnfocused` to another color (for example `#0000ff`): the focused window's border is red (opaque, the alpha is not used) and a window that loses focus shows the other color. Switch focus between two windows and watch both change. A value without the `#` is ignored. | |
| 3 | Enter full screen (Ctrl+Win+F): the border is gone; leave it: the border returns. With one pane set transparent nothing changes yet (13g). | |
| 4 | Turn on Advanced `SquareWindowCorners`, quit and relaunch: the FIRST window has square corners. Open a new window (Win+N): square too. Turn it off, relaunch: rounded again. | |

## Full screen and the division line

| # | Check | Result |
|---|---|---|
| 5 | Enter full screen: the window's own menu bar is hidden and the sessions use its room. Leave full screen: the menu bar returns. Turn off Appearance "Auto-hide menu bar in non-native fullscreen": in full screen the menu bar stays. Turn it back on. | |
| 6 | A Normal window with one tab and no tab bar shows a thin line right under the title bar. Open a second tab: the bar appears and the line goes. Close the tab: the line returns. | |
| 7 | With Appearance "Show line under title bar when the tab bar is not visible" off, the line never shows. A Minimal window (see the setup) and a full-screen window never show it. | |
| 8 | At 100% and 125% display scale the line is 1 pixel tall; at 150% and 200% it is 2 pixels. A new Normal window is still exactly its profile's Columns x Rows in cells. | |

## Titles

| # | Check | Result |
|---|---|---|
| 9 | With Advanced "Allow title setting" off in the profile, run `printf '\033]2;hi\a'`: the window title does not change. Turn it on and repeat: the title reads "hi". `printf '\033]1;icon\a'` never changes the title. | |
| 10 | Open two tabs with "Each tab may have its own window title" off (General > Window), and run `printf '\033]2;hi\a'` in tab 1: both tabs' sessions take the title, so tab 2's caption reads "hi". Turn it on: only the tab that set it changes. | |
| 11 | With one tab (no tab bar) turn Advanced "ShowWindowTitleWhenTabBarInvisible" off: the caption shows the session's own title (name, job) rather than the window title. To tell them apart, use a profile with Title components including Job, then `printf '\033]2;hi\a'`: with the key off the session title reads "hi (<job>)"; with it on the caption reads "hi". Add a second tab: the window title returns. | |
| 12 | In a profile turn on "Use custom window title" and set `\(currentTab.currentSession.path)`: a new window from that profile shows the current directory; `cd` elsewhere and the caption follows. Set a custom tab title in the same profile: a new tab shows it. Edit Window Title and Edit Tab Title also take `\(...)` text. A function call such as `\(uuid())` shows as written. | |
| 13 | Restore a saved arrangement: its window keeps the title saved with it, not the profile's custom title. | |

## Window number and the Window menu

| # | Check | Result |
|---|---|---|
| 14 | With Appearance "Show window number" on, a titled window's caption reads like "vim    Alt+Win+1" (four spaces, then its shortcut). Open a second window: "Alt+Win+2". The tenth window shows "10" with no shortcut. | |
| 15 | A window with no title bar (No Title Bar style or full screen) starts its title with "1. " instead. With "Show window number" off neither shows. | |
| 16 | The Window menu lists every window after Bring All To Front, each entry "<title> [em dash] Alt+Win+1" (a long dash between the title and the shortcut), the front one checked; choosing one focuses it. Turn Advanced "IncludeShortcutInWindowsMenu" off: the shortcut text leaves the entries. The list follows a retitle, a focus change, a new window and a close. | |

## Resizing

| # | Check | Result |
|---|---|---|
| 17 | Drag a Normal window's right edge: it snaps to whole columns, with about half a column of resistance, and the height re-snaps to whole rows as well. Drag a top or left edge: the opposite edge stays put. | |
| 18 | Hold Ctrl alone while dragging: the window resizes smoothly. Turn on Advanced `DisableWindowSizeSnap`: no snapping, but the window still cannot go below its minimum. Turn it off. | |
| 19 | The first window after a launch snaps too (it did not get the resize hook before this milestone). Maximized, full-screen and Lock Window Size windows never snap. | |
| 20 | Drag the window so it ends within one cell of the work area's edge: it fills the screen exactly, with no gap. | |
| 21 | Turn off Advanced `AllowLiveResize`: the window's edges no longer resize it (also a hotkey window and a restored arrangement's window). Turn it on. | |
| 22 | In a profile turn off "Terminal may resize window when not focused" (Session tab) and, from an unfocused pane, run `printf '\033]1337;SetWindowFrame=100;100;900;600\a'`: nothing changes (the numbers are AppKit points, y from the primary screen's bottom, not trust-gated). Focus the pane and repeat: the window moves and resizes. | |

## Font size and zoom

| # | Check | Result |
|---|---|---|
| 23 | With Appearance "Adjust window when changing font size" on (the default), use View > Make Text Bigger (Win+=): the Normal window grows so the columns and rows stay the same, its top-left corner fixed. Make Text Smaller and Make Text Normal Size shrink and restore it. Turn it off: the window keeps its size and the panes refit. | |
| 24 | A maximized or full-screen window keeps its frame when the font changes and the panes refit. In a split tab the whole tab's grid is kept, not one pane's. Other windows do not follow the font until milestone 13n. | |
| 25 | With General "Zoom maximizes vertically only" on, Window > Zoom (Shift+Win+0): the window stretches to the work area's height and keeps its x and width; Zoom again restores it. Off: Zoom maximizes and Zoom again restores. The maximize caption button always maximizes. | |
| 26 | With "Zoom maximizes vertically only" on, click Window > Zoom with Shift held: it maximizes fully (Shift inverts a click); Shift+Win+0 with the key on still zooms vertically (the chord does not invert). | |

## Window styles and sizes

| # | Check | Result |
|---|---|---|
| 27 | The profile's Window style popup has ten entries (Normal, Full Screen, Maximized, No Title Bar, Left/Right/Top/Bottom of Screen, Centered and so on). Beside Columns and Rows are units popups: "Columns" / "% of screen width" and "Rows" / "% of screen height". | |
| 28 | Pick "Top of Screen": the width popup shows "% of screen width" and the window fills the width; switch the height to "Rows" and open a window: it is that many rows tall. | |
| 29 | A Normal window with width 50 "% of screen width" and height 50 "% of screen height": a new window is about half the work area each way (whole cells). A No Title Bar and a Centered window take percents too. Set both back to cells: the window uses Columns x Rows. | |
| 30 | Edit `profiles.toml` (Tandem closed) to set Window Type 4: the window opens full screen. Set 14: the window is a topmost tool window with no taskbar button, and stays above others. | |
| 31 | For Maximized and full screen types the units labels read "columns by" and "rows" with the fields dimmed. | |

## Kept bugs and rulings a person can see

| # | Kept behavior | Result |
|---|---|---|
| 32 | On Windows 10 the border settings do nothing; on Windows 11 the border is opaque on all four edges and the alpha of the color is ignored. | |
| 33 | The Compact and Minimal caption label (right-aligned) waits for milestone 13e; a transparent pane forcing the border waits for 13g. | |
| 34 | Size Changes Update Profile does not exist yet (milestone 13n, 02:260 stays missing); a font change only fits the window. | |
| 35 | An unclosed `\(` in a custom title shows as literal text; a title with a function call shows the format as written. A tmux window keeps tmux's name and a profile's custom tab title is never sent to tmux. | |
| 36 | Dragging the edge of a Windows-Snap-arranged window snaps to the grid and can leave a gap beside its neighbor; the minimum size is the current tab's, not every tab's; after a DPI change mid-drag one step snaps to the old grid. | |
| 37 | The Window menu's "Zoom" from the keyboard menu path (Alt, arrows, Enter) counts as a click: Shift held inverts. Zoom after a manual move or resize restores the pre-zoom frame rather than zooming again. | |
| 38 | A tiny percent size (1 to 2 percent) makes a near one-cell window; there is no floor as the cells form has. | |
