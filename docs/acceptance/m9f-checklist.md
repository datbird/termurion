# Milestone 9f acceptance checklist

Close any open Termurion window. Copy `tandem.exe`, `tandem-host.exe`, `conpty.dll`, and
`OpenConsole.exe` from `%USERPROFILE%\src\termurion\target\release\` together, then run `tandem.exe`.
Mark each line pass or fail.

| # | Check | Result |
|---|---|---|
| 1 | View > Show Clippings (Shift+Win+=): a panel opens at the right of the session and the menu item is checked; the terminal gets narrower (its columns shrink). Choose it again: the panel closes. | |
| 2 | Open the panel. The + button opens "New Clipping" (Title, Detail, Cancel, Add). Add "First" with detail `Use **bold** and \`code\``: the row shows "First" and "Use bold and code". Add a second, "Second", with a long detail: it wraps and stops after about 100 pixels. | |
| 3 | Return in the Title field adds; Shift+Enter in the Detail adds; Esc cancels. | |
| 4 | Click "First", Ctrl+click "Second", then Send: the shell receives `**First**`, the detail, a line `--`, then `**Second**` and its detail (through the paste warnings as a paste). Copy puts the same text on the clipboard. | |
| 5 | Double-click a row: only its detail is pasted. | |
| 6 | Select a row and press Space: a preview shows over the terminal with the title and the whole detail; Space or Esc closes it. Delete removes the selected row. Up and Down move the selection. Typing a letter goes to the shell. | |
| 7 | Drag a row above another: the order changes (a line shows where it will land). | |
| 8 | Archive: the list empties. The left arrow shows the archived list with "1/1" in the strip; Add, Remove, and Archive are off there, Send and Copy work. The right arrow returns to the live list. | |
| 9 | Hover a strip button: its tooltip shows in the strip (for example "Archive all clippings"). | |
| 10 | Drag the panel's left edge: it widens and narrows (225 to 1200 pixels); the width is the same for every panel and survives a restart. | |
| 11 | Split the pane: each half has its own clippings; the panel stays with its session. The close button (x) hides the panel. | |
| 12 | Save a window arrangement with a session that has clippings and an archive, close it, restore the arrangement: the clippings, the archive, and the shown panel come back. | |
| 13 | Advanced > Session > clippingSeparator set to `\n==\n`: Send joins with `==`. | |
| 14 | Session > Open Autocomplete... on a screen with words: the list's words are readable (the list is no longer a blank box). | |
| 15 | In a tmux tab, View > Show Clippings shows a bar saying Clippings are not shown in tmux panes. | |
