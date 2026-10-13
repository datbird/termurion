# Milestone 13i acceptance checklist

Build a release binary (`scripts/xt build --release`). Close any open Termurion window, copy
`tandem.exe`, `tandem-host.exe`, `tandem-git.exe`, `conpty.dll`, and `OpenConsole.exe` from
`C:\Users\datbi\src\termurion\target\release\` together, then run `tandem.exe`.

Nothing in this milestone was checked on screen while it was built, so every line is a first look. The
pure parts (pane order, adjacency and wraparound, the dim amounts and colors, divider width, border
edges, lock texts and move refusals, width pin arithmetic) are unit tested. The wiring into the app, the
drawing and every mouse path were not, so read those rows twice. Settings: dimming rows are
Settings > Appearance > Dimming; the six other keys are Settings > Advanced (Tabs, General, Session,
Windows, Experimental Features). Use a tab with three panes unless a row says otherwise, and test the
divider rows at 100, 150 and 200 percent display scale. Mark each line pass or fail.

## Pane order: Next Pane, Previous Pane, numbers

| # | Check | Result |
|---|---|---|
| 1 | Make a tab with two panes in a top row and one wide pane under them. Next Pane (Window > Select Next Pane) visits top left, top right, bottom, then wraps to top left. Previous Pane goes the other way. | |
| 2 | Settings > Advanced > Tabs > NavigatePanesInReadingOrder off: click the bottom pane, then top right, then top left; Next Pane now follows the order last used (it goes to the pane used longest ago in the step order, and Previous Pane returns toward the last one), not the screen layout. Turn it on again and reading order returns at once. | |
| 3 | Make a tab with side-by-side columns (left pane, right column split in two): with the root split side by side, the order reads by column first (left, right top, right bottom). | |
| 4 | Next Pane then Previous Pane toggles between two panes without changing the order last used (quiet moves are not stamped). A click on a pane does stamp it. | |
| 5 | With Shortcut to choose a split pane = Alt+Number: Alt+1, Alt+2, Alt+3 pick the panes in the same order Next Pane visits, and each pane's title bar reads its number in that order (the title bar numbers and the Alt+digit agree, also with NavigatePanesInReadingOrder off). | |
| 6 | Status tool (Toolbelt) pane rows show the same numbers as the title bars. | |
| 7 | Ninth-pane rule: in a tab with exactly eight panes, Alt+9 and Alt+8 both pick the eighth pane and its title bar reads 9 (the Status tool says Alt+9 too). With nine or more panes, 1 to 8 pick by position and 9 is the last; with ten, the Status tool shows no pane shortcut for the ninth pane (its tab shortcut instead) and Alt+9 for the tenth. | |
| 8 | Next Pane in a maximized (zoomed) tab steps through every pane and leaves the zoom (a kept difference: iTerm2 swaps the maximized pane). | |

## Directional moves and WrapFocus

| # | Check | Result |
|---|---|---|
| 9 | Make a left pane and, to its right, two stacked panes (top right, bottom right). Click bottom right, click left, then Select Pane Right: it goes to bottom right (the neighbor used last), not top right. Click top right, click left, Select Pane Right: top right. | |
| 10 | At the right edge Select Pane Right wraps to the left pane (WrapFocus on). Select Pane Left at the left edge wraps to the right; Select Pane Above and Below wrap to the far end of the same column. | |
| 11 | Settings > Advanced > General > WrapFocus off: at an edge Select Pane Right does nothing (focus stays). | |
| 12 | The same moves work from the Select Pane menu items, the key actions, a pointer action and a script's select-pane-in-direction call. A swap-by-direction key action uses the same pick and keeps focus where it was. | |

## Splits: focus and profile

| # | Check | Result |
|---|---|---|
| 13 | Settings > Advanced > Session > FocusNewSplitPaneWithFocusFollowsMouse off and Settings > Pointer > General > Focus follows mouse on: split the focused pane. The new pane appears but focus stays on the old pane. With focus follows mouse off, or the key on, the new pane takes focus. | |
| 14 | With focus follows mouse on and the key off, a split made by a script (API) or an orchestration agent into a pane that is not active never takes focus; a script split of a background tab does not change that tab's focus. | |
| 15 | Change a pane's font from a script (or Edit Session), then split it: the new pane uses that font. With Settings > Advanced > Experimental Features > UseDivorcedProfileToSplit off, the new pane uses the saved profile's font. | |
| 16 | Split an ssh pane (a profile started by Configure SSH or an ssh:// link): the new pane connects to the same host as before. Change that ssh pane's tab color from a script (making it a session copy), split it: the new pane is a LOCAL shell with the copy's values (kept iTerm2 quirk: the divorced profile wins before the conductor). With UseDivorcedProfileToSplit off it is an ssh pane again. | |

## Dividers

| # | Check | Result |
|---|---|---|
| 17 | Settings > Advanced > Windows > SplitPaneDividerWidth 4: dividers become 4 points thick at once in every window, text reflows, tmux tabs resize. 9 draws 5 points; 0 or negative draws 1 point. Check at 100 percent, 150 percent and 200 percent display scale: 1 point is 1, 2 and 2 pixels (1 px at 100 percent, never 0), and the Clippings gutter, status bar and inline chat line up with the panes at width 5. | |
| 18 | SplitPaneColor #ff0000 turns every divider red; ff0000 (no #), a bad color or empty uses the theme color. In Minimal with prominence 0 and Dim inactive split panes on, the divider stays invisible unless SplitPaneColor is set. | |
| 19 | Drag a thick divider: the grab works from its edges (about 3 pixels beyond), and the divider tracks the pointer to within a few pixels at width 5 (a kept minor: the drag ignores the gap). | |

## Dimming

| # | Check | Result |
|---|---|---|
| 20 | Split a tab: the inactive panes are washed toward gray (0.4 by default), the active pane is lit; the margin around the text is washed too. Settings > Appearance > Dimming shows the slider first (Dimming amount), then Dim inactive split panes, Dim background windows, Dim only text. Moving the slider (0.01 to 0.6) changes the wash live. | |
| 21 | Dim inactive split panes off: no wash, inactive title bars still look inactive; the slider and Dim only text grey out unless Dim background windows is on. | |
| 22 | Dim only text on: the default background is left alone, text and colored cells dim, and no gray quad is drawn. Selection, search match and cursor colors do not dim (kept). | |
| 23 | Broadcast input to all panes: the receiving panes stay lit while the focused pane broadcasts (also in a window that is not in front); stop broadcasting and they dim. | |
| 24 | Dim background windows on, click another app: every pane of the window dims, and inactive panes dim 1.5 times as much (0.6 at the default, never above 0.9). Click back: the active pane is lit again. With it off, an unfocused window's active pane stays lit. | |
| 25 | Minimal theme: the outline boost of a dimmed pane touching the bar follows the pane's amount. Screenshot and the Open Quickly preview still use the fixed dim of before (a known gap). | |

## Active pane border

| # | Check | Result |
|---|---|---|
| 26 | Settings > Profiles > Colors > Other Colors > Border around active pane (off by default) turned on: the active pane has a 2 point border in the profile's Active Pane Border Color, framed around the terminal area (below its title bar, above a bottom status bar), and it moves when focus moves. A lone pane shows it too. Off hides it. Corners are square (kept). | |
| 27 | The window losing focus draws the border at half strength (an opaque color; a color with its own alpha is halved again). Light and dark variants follow the appearance. Dimmed inactive panes show no border. A see-through window keeps the border opaque. The Move Session preview covers it. | |

## Lock Pane

| # | Check | Result |
|---|---|---|
| 28 | Right-click a pane: Lock Pane (checked once locked), Lock All Panes in Tab; with some panes locked also Unlock All Panes in Tab; with all locked only Unlock All. | |
| 29 | A locked pane's title bar shows the lock mark left of the menu button; the tooltip reads "This pane is locked. It cannot be moved, swapped, or dragged, and closing it requires confirmation. Right-click to unlock."; a click on the mark unlocks. | |
| 30 | Close a locked pane running a shell: the question reads "This locked session is running ..." (jobs listed with "plus N others"); a locked pane lists only the lock reason, never also its profile reasons. An exited locked pane closed alone asks nothing; "This locked session will be closed." shows only in a tab or window close that asks for another reason. A tab with a locked pane reads "This tab (with a locked session) ...", a multi-pane tab "This multi-pane tab (with locked sessions) ...", a window "This window (with locked sessions) ...". Quit lists "This pane is locked.". | |
| 31 | Move Session to Split Pane, Swap With Session, a title bar drag, Move Session to Window and Move Session to Tab on a locked pane all refuse with a beep and nothing moves (the last two look enabled; kept). A swap key action (by direction, next or previous) still works. | |
| 32 | Lock All and Unlock All Panes in Tab lock and unlock every pane of the tab; the marks update. | |
| 33 | Settings > Profiles > Session > Default Pane Locked on: a new pane (a split, a new tab, a tmux pane) starts locked. A split of a locked pane starts locked only if its own profile says so. | |
| 34 | Restart Session on a locked pane keeps the lock. Save a window arrangement with a locked pane, close, restore: the pane is still locked (and an unlocked one is not). | |
| 35 | Undo Close of a locked pane brings it back still locked. | |

## Lock Split Pane Width

| # | Check | Result |
|---|---|---|
| 36 | Two panes side by side: Window > Split Pane > Lock Split Pane Width on the focused pane shows a check, and the check follows the focused pane (unchecked on the other). | |
| 37 | With it checked, widen the window: the locked pane never grows past its width and the other pane takes the change; narrow the window: the locked pane shrinks too. | |
| 38 | Unchecking: both panes scale again on the next resize. A single pane, a tmux pane or a pane only stacked in a column: the item does nothing and shows no check. | |
| 39 | Drag the divider to widen a locked pane: allowed (kept); the next window resize or the next split caps it again. Restart Session keeps the width lock. A pane pinned below 10 percent of its split holds at 10 percent. A side-by-side split whose two panes are both pinned is left alone. Pins are not saved in arrangements. | |

## Kept iTerm2 quirks and rulings a person can see

| # | Check | Result |
|---|---|---|
| 40 | A divorced ssh pane splits to a local shell with its own values; the next split of a Termurion-made ssh copy connects to the copy's host (m13i rulings, Task 5). | |
| 41 | Next Pane unzooms a zoomed tab instead of swapping the maximized pane. | |
| 42 | Dimming averages toward gray in linear light, so midtones differ slightly from iTerm2's. | |
| 43 | Dim only text leaves the tab bar undimmed and adds no Minimal outline boost, and leaves selection, match, cursor and label colors alone, and inline images and portholes are unverified. | |
| 44 | The active pane border has square corners where iTerm2 rounds them at the window's corners. | |
| 45 | The default divider is scaled: 2 pixels at 150 percent and 200 percent where Termurion drew 1. | |
| 46 | A pinned and locked tab's close question says "This tab", not "This pinned tab". | |
| 47 | Move Session to Window and to Tab look enabled for a locked pane and refuse with a beep. | |
| 48 | Unlock All Panes in Tab is a menu item of its own when some panes are locked (iTerm2 hides it behind Option). | |
| 49 | Width pins apply on a window resize and when a split is made (not on a toolbelt toggle or a DPI change); a divider drag can undo one until then. | |
| 50 | The Dimming section of Appearance holds Dim background windows, moved from Windows. | |
| 51 | Several kept tests only grep source text (wiring of the gap, the border, the split focus rule): only these rows prove the behavior. | |
