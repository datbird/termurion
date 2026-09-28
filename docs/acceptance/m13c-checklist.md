# Milestone 13c acceptance checklist

Build a release binary (`scripts/xt build --release`). Close any open Tandem window, copy
`tandem.exe`, `tandem-host.exe`, `tandem-git.exe`, `conpty.dll`, and `OpenConsole.exe` from
`C:\Users\datbi\src\tandem\target\release\` together, then run `tandem.exe`.

Setup: the tab bar settings are on Settings > Appearance > Tabs and Settings > Advanced > Tabs.
The number and text Advanced rows (Minimum tab width, Minimum tab width for compact tabs,
Preferred tab width, Tab width when the tab bar is scrollable, Default tab bar height, Minimum
width for tab labels, Custom tab label font size, Colors for tab color menu item) have no editor
until milestone 13a's controls land (ruled); set them with Tandem closed in
`%APPDATA%\Tandem\settings.toml` under `[settings]`, for example `DefaultTabBarHeight = 30.0`,
and remove the line afterwards to return to the default. Use a window at about 1000 points wide
unless a check says otherwise. Run the shell checks in a PowerShell session (`printf` checks in WSL
or Git Bash). Mark each line pass or fail.

| # | Check | Result |
|---|---|---|
| 1 | With one tab, no tab bar shows (Appearance > Tabs "Show tab bar even when there is only one tab" is off by default). Open two more tabs: the bar appears, 24 points tall (24 pixels at 100% display scale). | |
| 2 | With three tabs the tabs are equal and together fill the whole bar, with the "+" button right after the last tab. | |
| 3 | Each of the three tabs shows "Win+1", "Win+2", "Win+3" at its right end. Change Keys > "Shortcut to select a tab" to Alt+Number: the tabs read "Alt+1" and so on; to No Shortcut: no numbers. Turn off Appearance > Tabs "Show tab numbers": the numbers go. | |
| 4 | Hover a tab: a close button appears at its left and the title stays centered; move away: the button goes. In Advanced > Tabs turn on "Should tab close buttons always remain visible?": every tab shows its close button and the title starts after it. Turn off "Tabs have close buttons": no tab has one. | |
| 5 | In an unfocused Tandem window the close buttons draw at half strength. | |
| 6 | Turn off "Stretch tabs to fill bar": the three tabs are 175 points wide each, from the left. | |
| 7 | Open 20 tabs: the bar scrolls instead of shrinking the tabs (each 120 points wide). Shift+wheel over the bar (or a horizontal wheel or trackpad swipe) scrolls it; the "+" stays pinned at the right end and tabs are cut under it. Press Win+Shift+] (Window > Tab > Select Tab > Select Next Tab) repeatedly: whenever the selected tab would pass the edge, the bar scrolls to show it fully. | |
| 8 | With 20 tabs, turn off Appearance > Tabs "Tab bar scrolls when tabs don't fit": the tabs that fit share the bar and a double chevron button at the right lists the hidden tabs, each with its number as " (n)" and a check on the selected one; picking one selects it. | |
| 9 | Open three tabs and give them the titles `server-a: ~/src`, `server-a: ~/doc`, `server-a: ~/bin` (Edit Tab Title, or `printf '\033]1;server-a: ~/src\a'` in each). Narrow the window until the titles do not fit: each loses its start (an ellipsis, then "~/src"), not its end. Titles `alpha one long`, `beta two long`, `gamma three long` in narrow tabs lose their end instead. | |
| 10 | Turn on "Support basic HTML tags in tab titles" and run `printf '\033]1;<b>bold</b> x\a'`: the tab shows "bold" in bold and " x" plain. `<i>` gives italic, `<u>` an underline, and `&lt;` shows "<". Turn the setting off: the tags show as written. | |
| 11 | Set the profile's Subtitle (Profiles > General) to `\(session.hostname)` and set `DefaultTabBarHeight = 30.0` (Setup): the bar is taller and each tab shows the host name on a second, smaller line under its title. With `DefaultTabBarHeight = 24.0` (or the line removed) the subtitle does not show. | |
| 12 | Middle-click a tab: it closes (with the close confirmation when a program is running, as Close Tab). Turn off Advanced > Tabs "Should middle-click on a tab in the tab bar close the tab?": a middle click does nothing to the tab. | |
| 13 | Click a tab's close button: the tab closes through Close Tab (the same confirmation). Press on the close button, slide onto the tab body and release: nothing closes, and the tab is selected. | |
| 14 | Double-click a tab: it is selected and the Edit Tab Title window opens. Turn off "Should double-clicking a tab open a window to edit its title?": a double click only selects. | |
| 15 | Double-click the empty part of the bar (with stretching off, right of the last tab): a new tab opens. | |
| 16 | Click "+": a new tab opens with the default profile. Turn on "Should the "new tab" button in the tab bar use the current session's profile?" and, from a tab running the second profile, click "+": the new tab runs that profile. Turn on "Remove the "new tab" button from horizontal tab bars?": the "+" is gone. | |
| 17 | Press a tab and hold: it is selected on the press. Turn off "Select tabs on mouse-down?" and open a new window: in the new window a tab is selected only on the release, while the first window still selects on the press (the setting affects new windows only); a press with Alt held also waits for the release. | |
| 18 | In a background tab run `ping -t localhost`: the tab shows a spinner while it prints, apart from the new-output indicator; the selected tab never shows it. Turn off Appearance > Tabs "Show activity indicator": no spinner. | |
| 19 | Rest the pointer on a tab for half a second: a tooltip shows "Name: <the tab's title>", a second line with its subtitle (blank when none), "Profile: <profile name>" and "Command: <the command it started, or None>". Moving off the tab or pressing a button hides it. Turn on "Disable tab bar tooltips?": no tooltip. | |
| 20 | Turn on "Show tab bar even when there is only one tab" and close tabs down to one: the bar stays. | |
| 21 | Open a second window with 20 tabs next to the first; scroll one window's bar: the other's offset does not move, and hovering one window's tab shows no close button or tooltip in the other. | |
| 22 | Set `UseCustomTabBarFontSize` on (Advanced > Tabs "Use custom font size for tab labels?") and `CustomTabBarFontSize = 14.0` (Setup): the tab labels are larger. | |
| 23 | Turn on "Uneven tab widths allowed." with tabs titled `a`, `a much longer title here`, `b`: each tab takes its own width when they all fit; with many tabs they fall back to equal widths. | |
| 24 | (After milestone 13d) Right-click a tab and choose Tab Color: a popup shows "Default", seven swatches labeled "#fb6b62" to "#787878", and "Choose Color...". A swatch colors the tab; "Default" clears it; "Choose Color..." opens the Windows color dialog starting at the tab's color, and OK colors the tab. | |
| 25 | (After milestone 13d) Set `TabColorMenuOptions = "#123 #00ff00"` (Setup): the Tab Color popup shows "Default", "#112233", "#00ff00" and "Choose Color...". | |

## Kept bugs and rulings a person can see

| # | Kept behavior | Result |
|---|---|---|
| 26 | Tab titles are centered in Segoe UI at 11 points (iTerm2's system font size), not the terminal font, and are no longer left-aligned. | |
| 27 | The session status (OSC 21337) no longer prefixes the title in the status color: it joins the subtitle line after an en dash, in the subtitle's color, and shows only when the bar is 28 points or taller (check 11), as iTerm2. | |
| 28 | The tab number reads "Win+1" (Windows words for iTerm2's Cmd glyph), wider than iTerm2's Cmd glyph and "1". With Alt+Win as the modifier the tab reads "Alt+Win+1" while Keys shows "Win+Alt+Number"; both are right. | |
| 29 | Tabs past the ninth show no number (only the last one shows "Win+9"), but still keep the number's room. | |
| 30 | The close button appears at once on hover (iTerm2 fades it in). A held close button shows a shade behind its glyph. | |
| 31 | With close buttons shown only on hover, the title stays centered as if the button were not there, and moves right only when it would run into the button's room. | |
| 32 | A tab's icon (new output, bell, idle, dead, the status dot) is dropped when the label would get 35 points or less. | |
| 33 | The spinner is a rotating arc stepping every half second, not macOS's spinner. | |
| 34 | The overflow menu lists plain titles (no icons). The selected tab stays in the menu when it is hidden; it is not moved into view. | |
| 35 | The tooltip waits 500 ms after the pointer settles, follows the pointer on its tab, uses the terminal font, returns after a click once the pointer rests again, and is not hidden by typing. | |
| 36 | "Draw bottom line for horizontal tabbar in Regular, Dark and Light theme?" changes nothing (iTerm2's tab styles never read it; R2). | |
| 37 | Tab titles longer than 255 characters are cut to 255 plus an ellipsis. Upper-case tags (`<B>`) are not HTML: a title with only upper-case tags shows as written, and inside an HTML title they are removed without styling. | |
| 38 | Colors for tab color menu item: the help mentions `color(p3 1 0.5 0.25)`, which iTerm2's parser does not read either; `p3#rrggbb` works, and a word that is not a color is skipped. | |
| 39 | Tab Color is a popup of swatches plus Windows' color dialog, not iTerm2's in-menu color grid; there are no recent colors. Choosing a tab color drops a color a program set for that tab. | |
| 40 | In a tmux window, a double click on the empty bar and "+" open a tmux tab (the menu's New Tab does not ask tmux). | |
| 41 | "Tab bar scrolls when tabs don't fit" is on by default and makes the top bar scroll, as in iTerm2 (its label says side, its code does not check); the side tab bar is milestone 13d's. | |
| 42 | A press on tab 1 of one window then on tab 1 of another within the double-click time counts as a double click. | |
| 43 | A bold or italic title is measured in the regular face, so it may center a few pixels off. | |
| 44 | The eight number and text Advanced > Tabs rows (Setup) have no editor in this build; milestone 13a adds it. | |
