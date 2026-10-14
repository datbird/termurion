# Milestone 4b acceptance checklist

Close any open Termurion window, then run `%USERPROFILE%\src\termurion\target\release\tandem.exe`.
Cmd is the Win key and Option is Alt. Mark each line pass or fail.

| # | Check | Result |
|---|---|---|
| 1 | Run `dir C:\Windows` a few times. Win+F opens the find bar at the top right. Type `sys`: every match gets a yellow background, one is selected, and the bar shows its count as `i/N`. Type only `sy`: the search waits about a second. | |
| 2 | In the bar, Return goes to the next match up and Shift+Return down. Win+G and Win+Shift+G do the same with the bar closed. At the top, the next Return wraps to the bottom and says "Search wrapped." | |
| 3 | Click the mode label in the bar: a menu lists Smart Case, Case Sensitive, Case Insensitive, Case-Sensitive Regex, Case-Insensitive Regex. With Smart Case, `sys` also finds `System32`; `Sys` does not find `sysnative`. With a regex mode, `win\w+` works, and `(` turns the query red. | |
| 4 | Esc closes the bar and clears the highlights. Win+F again: the old query is back. Click in the pane while the bar is open: typing goes to the shell again; click the bar's text: typing goes to the bar. | |
| 5 | Select a word, Win+E (Use Selection for Find): the bar searches for it. Win+Alt+Shift+E (Clear Find) empties the query. Scroll away, Win+J (Jump to Selection): the view comes back to the selection. | |
| 6 | Edit > Find > Find Globally (Win+Shift+F) with two tabs: typing a word lists matches grouped by tab, the word marked in yellow. Clicking a result brings that tab forward with the match selected. | |
| 7 | Edit > Find > Filter (Win+Alt+Shift+F): the bar says Filter, and typing `exe` leaves only lines that contain it. Run a command that prints `exe`: its new lines show up. Esc: the full buffer is back at the same scroll position. | |
| 8 | Select three lines, View > Zoom In on Selection (Win+Alt+Z): only those lines show. View > Zoom Out (or Esc) brings the whole buffer back. | |
| 9 | Print some URLs (`echo https://example.com www.iterm2.com`), then Edit > Find > Find URLs (Win+Alt+U): each URL gets a yellow label `1`, `2`. Press `1`: the browser opens it. Repeat and press Alt+`2`: the URL is copied. Any other key puts the labels away. | |
| 10 | Find All Smart Selection Matches (Win+Alt+Shift+U) labels paths, URLs, and quoted strings on screen. Pick Result to Open (Win+Alt+Shift+O) with find matches up labels those. | |
| 11 | Quadruple-click a URL like `https://example.com/a_b?x=1`: the whole URL is selected and copied. Settings > Pointer > Selection > Double-click performs smart selection: now a double click does that, and a quadruple click selects a word. | |
| 12 | In a folder with source files, `echo src\main.rs:12:5` (with that file present). Hold Win: the path is underlined in the link color and the pointer is a hand. Win-click: the file opens (in VS Code at line 12 when `code` is on the PATH). The Start menu does not open. | |
| 13 | Win-click a URL: the browser opens it. Win-click a folder name: Explorer opens it. Win-click plain text: nothing opens, and the click selects as usual. | |
| 14 | In WSL, `printf '\e]8;;https://example.com\e\\link\e]8;;\e\\\n'`: `link` has a dashed underline. Win-click opens example.com. Right-click it: the menu starts with Copy Link Address, which copies the URL. | |
| 15 | Settings > Profiles > Advanced: the Smart Selection rules table lists iTerm2's rules. Add a rule `TICKET-\d+`, precision high, with an action Open URL `https://example.com/\0`. Right-click `TICKET-42` in the pane: the menu starts with that action, which opens the URL. Win-click `TICKET-42` runs the same action. | |
| 16 | Advanced > Semantic History: pick "Run command..." and enter `notepad \1`. Win-click an existing file name: Notepad opens it. Pick "Open with default app" again afterward. | |
| 17 | Select a file name or URL, Edit > Open Selection (Win+Alt+O): it opens. With nothing selected, it beeps. | |
