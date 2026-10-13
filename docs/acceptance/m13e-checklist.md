# Milestone 13e acceptance checklist

Build a release binary (`scripts/xt build --release`). Close any open Termurion window, copy
`tandem.exe`, `tandem-host.exe`, `tandem-git.exe`, `conpty.dll`, and `OpenConsole.exe` from
`C:\Users\datbi\src\termurion\target\release\` together, then run `tandem.exe`.

Nothing in this milestone was checked on screen while it was built, so every line here is a
first look. Most of the Win32 work (the caption frame, the hit tests, the key and menu paths, the
DPI handling, the forced title bar) was only unit tested through its pure parts, so the rows for
it are the ones to read twice. Setup: the Theme popup is Settings > Appearance > General; the
tab rows are Settings > Advanced ("Tabs") and the caption rows Settings > Advanced ("Windows");
a tab's color is the profile's Tab Color (Profiles > Colors) or the tab menu's Tab Color. Use two
tabs unless a row says otherwise, one of them colored. Have two windows open for the rows that
switch windows. Mark each line pass or fail.

## Theme popup and Regular

| # | Check | Result |
|---|---|---|
| 1 | Settings > Appearance > General > Theme lists, in order: Regular, Minimal, Compact, Light, Dark, Light (High Contrast), Dark (High Contrast). A fresh settings file shows Regular. | |
| 2 | Regular: switch Windows between light and dark mode (Settings > Personalization > Colors): the tab bar, the title bar and the menu bar follow at once. Turn on a Windows contrast theme: the tab bar changes to a High Contrast look at the next redraw (click in the window). Turn it off: back to the ordinary look. | |
| 3 | Regular in light mode: the selected tab is clear against a light grey bar, unselected tabs are slightly darker, thin lines between tabs. Dark mode: the unselected tabs are dark and the selected one is the bar's color. Tab text is 11 points and dims in a background window. | |

## Forced themes

| # | Check | Result |
|---|---|---|
| 4 | Light: with Windows in dark mode the title bar, the menu bar, the Settings window, the Password Manager and the replay panel all turn light. Dark: with Windows in light mode they all turn dark. | |
| 5 | With Theme Dark and Windows in light mode, change a Windows setting (switch the mode, then back, or change the accent color): the title bar stays dark each time and never flashes back to light. Repeat with Light in dark mode. Not run live before: this is the forced title bar across a Windows setting change. | |
| 6 | Set the Theme to Regular after a forced one: the title bar goes back to following Windows, also after the next Windows setting change. Open a new window while Dark is forced: it opens dark (a brief light title bar flash on open is a kept cost). | |
| 7 | A profile with "Use separate colors for light and dark mode" on, under Dark or Light: the terminal colors switch to that mode's set even when Windows is in the other mode. Under Regular they follow Windows. | |
| 8 | Profiles > Colors with separate light and dark colors on: under Light, Dark, Light (High Contrast) or Dark (High Contrast) the warning text shows under the "Editing the ... mode colors" line; under Regular, Minimal and Compact it does not. With separate colors off it never shows. | |
| 9 | Light (High Contrast): tab text is black, the selected tab matches the window background, unselected tabs are mid grey, text 12 points. Dark (High Contrast) with Advanced HighVisibility on (the default): the selected tab is light grey with black text and unselected tabs are near black with white text. Turn HighVisibility off: the selected tab is dark grey with white text. Light (High Contrast) does not change with it. | |
| 10 | Kept gap: right-click menus and other popup menus follow Windows' mode, not the forced theme (a light context menu under Dark is expected). | |

## Colored tabs

| # | Check | Result |
|---|---|---|
| 11 | Give one tab a Tab Color: it is filled with the color (brighter on the selected tab, softer when unselected) and the selected colored tab has a thin outline (white on a dark color, black on a light one). Tab text turns black or white by the fill. An uncolored tab beside it still follows the theme. | |
| 12 | Change Advanced ColoredSelectedTabOutlineStrength to 3 (thick) and 0 (none), and ColoredUnselectedTabTextProminence to 1 and 0.2: the outline and the unselected text change accordingly. | |
| 13 | Change Advanced LightModeInactiveTabDarkness and DarkModeInactiveTabDarkness: unselected tabs get darker in the Light and Dark themes respectively. | |

## Minimal

| # | Check | Result |
|---|---|---|
| 14 | Minimal: the tab bar takes the current session's background. Change the profile's background color (Profiles > Colors) or open a tab with another profile: the bar follows the selected tab's session. | |
| 15 | Minimal with two tabs: the selected tab is the bar color and the other is slightly different (Advanced MinimalTabStyleBackgroundColorDifference), and a thin line (the notch) runs along the bar's lower edge up to the selected tab, around its sides and on to the far end. With the first tab selected the left part is skipped. No lines between tabs and no line between the bar and the terminal. | |
| 16 | Minimal in a background window with Advanced Dim background windows on: the bar dims. Click the window: it returns to full color. With Dim background windows off nothing dims. | |
| 17 | Minimal with a colored tab: the tab color shows at full strength when selected and half when not; the selected colored tab has a two-line underline. Advanced MinimalSelectedTabUnderlineProminence and MinimalDeslectedColoredTabAlpha change them. | |
| 18 | Minimal with many tabs (so some go to the overflow menu) and the selected tab in the overflow menu: the bar still shows its edge line up to the overflow button. Repeat with a Left or Right tab bar (Appearance > Tabs). | |
| 19 | Advanced MinimalTabStyleOutlineStrength, MinimalTextLegibilityAdjustment and MinimalTabStyleTreatLeftInsetAsPartOfFirstTab change the outline, the text contrast and the first tab's left line. | |

## Minimal and Compact windows: tabs in the caption

| # | Check | Result |
|---|---|---|
| 20 | Minimal and Compact: the tabs move up into the caption area, with minimize, maximize and close buttons at the right (Windows 11 look: 46 points wide, Segoe Fluent Icons glyphs, the close button red on hover). The Minimal bar is 38 points tall (Advanced CompactMinimalTabBarHeight); Compact stays 24. The panes start right below it and the window's grid does not change. | |
| 21 | Hover the maximize button: Windows' snap layouts flyout appears, and choosing a layout snaps the window. Click maximize: the window maximizes and the glyph becomes restore. Minimize and close work, and the close button turns red under the pointer. Press a button and drag off it before releasing: nothing happens. | |
| 22 | With one tab (the bar hidden) the caption shows the window title where the bar would be, at the bar's height; open a second tab: the tabs replace it and the panes do not move. | |
| 23 | The window switch label ("Alt+Win+1") sits right-aligned before the caption buttons in Minimal and Compact windows. With Advanced "Show window number in titlebar accessory?" off the label goes and the caption title reads "1. <title>". | |
| 24 | The menu bar is not shown in a Minimal or Compact window. A menu button (three lines) at the left of the strip opens the whole menu under it. Alt pressed and released alone opens it, and so does F10. Alt with another key (Alt+Tab) or with Shift does not. AltGr on a non-US layout types its characters and opens nothing. | |
| 25 | Right-click on the empty space of the tab strip: the whole menu opens at the pointer. Right-click on a tab (also within its top 12 points): the tab menu. Right-click on a group's chip: the group menu. Right-click on a tab's close button: the tab menu. | |
| 26 | Not run live before: a Minimal window dragged from a 100% monitor to a 150% or 200% monitor and back keeps its client (Columns x Rows) and the tab strip does not grow or shrink. Repeat with a Compact window. Then maximize one and move it between the monitors with Win+Shift+Left/Right (dragging it would restore it first), or change its monitor's scale in Settings > Display while it is maximized: it stays maximized and fills the monitor's work area exactly (the bottom row is on screen and the tab strip starts at the work area's top, which is the monitor's top unless the taskbar is docked there). | |
| 27 | Switch the Theme between Regular and Minimal several times with a window open: the window keeps its client size each time and the menu bar hides and returns. Full screen (Ctrl+Win+F) in a Minimal window shows a plain full-screen window and leaving restores the strip. | |
| 28 | Advanced CompactTabBarStoplightButtonsWidth: 75 reserves exactly the three buttons; a larger value leaves more room at the right of the tabs, a smaller value lets the tabs run under the buttons. ExtraSpaceBeforeCompactTopTabBar adds a gap between the last tab (and the "+") and the buttons. Neither applies with a bottom or side bar. | |
| 29 | A Compact window with a top bar shows no top lines on the tabs (the bar is not attached to a title bar). | |

## Dragging the window

| # | Check | Result |
|---|---|---|
| 30 | In a Minimal or Compact window with two or more tabs, press and drag in the top 12 points (Minimal) or 10 points (Compact, Advanced MinimalEdgeDragSize and CompactEdgeDragSize) of a tab: the window moves (and snaps to screen edges). Press lower on a tab and drag sideways: the tabs reorder. Press lower on a tab and drag up into that strip: the window moves once the pointer is in the strip. | |
| 31 | Drag from empty bar space: the window moves. Double click empty bar space: the window maximizes (and again restores); it does not open a tab. | |
| 32 | With one tab in a Minimal or Compact window, drag the tab: the window moves. Turn off Advanced ConvertTabDragToWindowDragForSolitaryTabInCompactOrMinimalTheme: dragging the only tab behaves as in Regular (it tears off into a new window at the drop point). A No Title Bar window (profile Window style) with one tab behaves the same in Regular. | |
| 33 | With one tab in a Minimal window and the key on, drag the tab a long way: the window follows the pointer and no second window is made. | |
| 34 | Alt+drag from a tab and from empty bar space, in Regular, Minimal and Compact: the window moves each time. Alt+drag from a group chip drags the group, never the window. Dragging the "+" button moves the window with or without Alt while Advanced AllowDragOnAddTabButton is on; with it off, nothing moves. In a Minimal or Compact window the same holds with the press in the "+"'s top 12 (Compact 10) points, the edge strip: a click there opens a tab and, with the key off, a drag moves nothing. Alt+click on a tab selects it (or closes it on its close button); Alt held only after the drag has started also moves the window. Not run live before. | |
| 35 | A Compact Centered window (profile Window style Centered under Compact) has no top edge strip, and an Alt+drag on its bar (or a drag of the "+") moves nothing and drags no tab, as in iTerm2. The same in a Centered window under Regular and in a window docked to a screen edge. Dragging the empty strip of a Compact Centered window still moves it (Windows' caption). | |
| 36 | A Left or Right tab bar never has an edge strip: a drag on a side tab reorders, never moves the window. | |

## The window name

| # | Check | Result |
|---|---|---|
| 37 | Edit Window Title (Session menu) and type a name: in a Minimal or Compact window with two tabs the name shows beside the tabs, dimmed. With Advanced ShowWindowNameBesideTabs "Never" it goes; "When a custom name is set" (default) shows it only with a custom name; "Always" shows the window title when there is no custom name. | |
| 38 | A long name is cut with an ellipsis and shrinks as tabs are added; with too many tabs for it (under 44 points) it hides. With a scrolling tab bar it keeps its room. | |
| 39 | Double click the name, also within its top 12 points: Edit Window Title opens (the window does not maximize). Press and drag on the name: the window moves. A single click does nothing. Not run live before. | |
| 40 | A Regular window, a full-screen window and a window with a Left, Right or Bottom bar never show the name. | |

## Dividers

| # | Check | Result |
|---|---|---|
| 41 | Split a tab: the divider is light grey in Regular and Compact with Windows in light mode, Light and Light (High Contrast), and dark grey in Regular and Compact with Windows in dark mode, Dark and Dark (High Contrast). | |
| 42 | Minimal with a split: the divider is the session background nudged toward white (dark) or black (light) by MinimalSplitPaneDividerProminence (0.15). Set it to 0 with Dim background windows on: the divider is invisible when the other panes share one background. Set it to 1: a strong line. | |

## Kept bugs and rulings a person can see

| # | Kept behavior | Result |
|---|---|---|
| 43 | The tabs look like iTerm2 on macOS 15 (the pre-Tahoe styles); the Tahoe look is not drawn. The light and dark bar materials are fixed colors (rgb 225 and rgb 58), and Light (High Contrast)'s window color is rgb 236. | |
| 44 | Regular reads a Windows contrast theme as macOS's increased contrast; a contrast change applies at the next redraw, with no polling. | |
| 45 | Context and popup menus follow Windows' mode under a forced theme. | |
| 46 | The window may show a brief system-colored title bar when a forced-theme window opens. | |
| 47 | The caption buttons are Termurion's own, at the right: no menu bar in Minimal and Compact windows (the menu button, Alt and F10 replace it); Alt+Space still opens the Windows system menu. | |
| 48 | Shift+Alt alone does not open the menu; F10 opens it on the key press, not the release. | |
| 49 | A double click in the edge strip over a tab does nothing: it neither maximizes the window nor edits the tab (iTerm2 runs the title bar's double click only off a tab). Over the "+" or the overflow button in the strip it does not maximize either (the first click opens a tab or the overflow menu). Over a group chip in the strip it maximizes. | |
| 50 | An Alt drag starts after the pointer passes MinimumTabDragDistance, so a very small Alt jitter is still a click. | |
| 51 | The window name has no letter tracking, and a Minimal window's menu button has no hover highlight. | |
| 52 | With many tabs the window name hides even when the tabs would overflow anyway (all tabs count toward its room), and pinned tabs' and chips' real minimum widths are not counted. | |
| 53 | The Dark theme's one-point bottom edge is a solid black line, not iTerm2's gradient; DrawBottomLineForHorizontalTabBar does nothing. | |
| 54 | A compact window's taskbar title keeps the "N. " prefix while its caption shows the label at the right. | |

## Not run live: the caption frame's Win32 paths

| # | Check | Result |
|---|---|---|
| 55 | Size requests keep the asked grid in a Minimal window (note Columns x Rows before and after each): show and hide the toolbelt; move the tab bar to the Bottom, then Left, and show it with a second tab and hide it by closing that tab; Window > Zoom and Zoom again; a program's resize (`printf '\e[8;30;100t'`) and an API resize; restore a saved arrangement into a Minimal window. Each keeps exactly the asked Columns x Rows, never one caption short. | |
| 56 | tmux integration (`tmux -CC`) in a Minimal and in a Compact window: `tmux list-clients -F '#{client_width}x#{client_height}'` equals the visible Columns x Rows. Open a second tab (the bar shows) and close it (the bar hides): it still matches each time. | |
| 57 | Drag an edge of a Minimal window: the size snaps to whole cells (the grid under the strip), as in a Regular window. | |
| 58 | Lock Size on (Window menu) or Advanced AllowLiveResize off, in a Minimal window: a press on the window's very top edge and a drag moves the window; it never resizes it. | |
| 59 | Maximize a Minimal window: nothing is cut at the top (the tab strip's full height shows). With the taskbar docked at the top and set to auto-hide, pushing the pointer to the top edge still reveals it. On Windows 10 a thin top border line may show over the strip (kept). | |
| 60 | Minimize a Minimal window, switch the Theme to Regular (and another time to Minimal from Regular) while it is minimized, then restore it: note the rows. Kept: it may come back one caption and menu bar taller or shorter (the size is not kept for a minimized window). | |
| 61 | Setup: snap or move the window so its top-right corner is the screen's top-right corner, so the maximize button stays under the pointer when the window maximizes. Hover the maximize button, move onto a tab, then back onto the same button: it lights again each time. Press a caption button, drag off to a tab and release: nothing happens and no button stays lit. Double click the maximize button quickly: the window maximizes and restores (two clicks). | |
