# Milestone 7f acceptance checklist

Close any open Tandem window. Copy `tandem.exe` and `tandem-host.exe` from
`C:\Users\datbi\src\tandem\target\release\` together, then run `tandem.exe`. Cmd is the Win key and
Option is Alt. Mark each line pass or fail.

| # | Check | Result |
|---|---|---|
| 1 | Split the window into two panes and open a second tab. Shell > Broadcast Input > Broadcast Input to All Panes in Current Tab (Win+Alt+I): a message box says "Keyboard input will be sent to 2 panes in the current tab." Cancel: nothing changes. Again, OK: the item is checked. | |
| 2 | Type `echo hi` and Enter: it appears and runs in both panes of this tab, not in the other tab. Both panes show the broadcast icon at their top right. | |
| 3 | Paste two lines (Win+V): both panes get both lines. Drop a file on a pane: both get the path. | |
| 4 | Choose Broadcast Input to All Panes in Current Tab again: broadcast turns off (Send Input to Current Session Only is checked). | |
| 5 | Broadcast Input to All Panes in All Tabs (Win+Shift+I): typing reaches the panes of both tabs. Choose it again: off. | |
| 6 | Toggle Broadcast Input to Current Session (Win+Ctrl+Alt+Shift+I) in pane A, then right-click pane B > Toggle Broadcasting Input: typing in A reaches B and back; no message box appeared. | |
| 7 | With A and B broadcasting, Current Session is Broadcast Source in A: typing in A reaches B; typing in B stays in B. B shows the receive-only icon. | |
| 8 | Show Background Pattern Indicator: the broadcasting panes get red diagonal stripes ("/") under the text. Turn it off. | |
| 9 | Settings > Keys: bind a key in the profile to "Send Text without Broadcasting" with `echo solo\n`: while broadcasting, it runs only in the pane you press it in. | |
| 10 | While broadcasting to 2 panes, Edit > Clear Buffer: a question offers Yes (Clear All), No (Current Session Only), Cancel, and each does what it says. Shell > Reset asks the same way. | |
| 11 | Run `vim` in one broadcasting pane: clicking inside vim (mouse reporting) moves only that vim's cursor; other panes get no mouse text. | |
| 12 | Settings > Advanced: turn on "Suppress warning about broadcasting input": item 1's message box no longer appears. Turn it off. | |
| 13 | Settings > Advanced > disableTopRightIndicators on: the broadcast icons disappear. Turn it off. | |
