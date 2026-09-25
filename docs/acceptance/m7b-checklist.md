# Milestone 7b acceptance checklist

Close any open Tandem window, then run `C:\Users\datbi\src\tandem\target\release\tandem.exe`.
Cmd is the Win key and Option is Alt. Mark each line pass or fail.

| # | Check | Result |
|---|---|---|
| 1 | Split a tab into two panes and run `ping -n 60 127.0.0.1` in one. Close the tab (Win+W on each pane closes one; use Window > Close Tab or the close box): "Close tab #1?" / "This multi-pane tab is running PING." (or ping). Cancel keeps it. | |
| 2 | Settings > General > Closing > "Confirm closing multiple sessions" off: closing that tab with no Prompt Before Closing asks nothing. On again, closing a window with two running sessions asks "Close Window #1?". | |
| 3 | A single pane with nothing running closes without a question. A pane in a split whose profile has Prompt Before Closing "Always" asks `Close session "..."?` with "This session will be closed." | |
| 4 | File > Quit (with one window, one session, Confirm "Quit Tandem" on): "Quit Tandem?" with "All sessions will be closed." and the reason. With Confirm Quit off and one session: no question. With two windows: one question covers both. | |
| 5 | Close a tab running `ping -n 60 127.0.0.1`, then within 5 seconds Shell > Undo Close (Win+Shift+T): the tab comes back with ping still counting. | |
| 6 | Close a tab, wait more than 5 seconds, Undo Close: nothing comes back, and Task Manager shows the closed shell is gone. | |
| 7 | Close one pane of a split and undo: it comes back beside the tab's focused pane. Close a whole window (with two open) and undo: the window comes back with its tabs. | |
| 8 | Profile > Session > "Undo can revive a closed session for up to" 0: closes cannot be undone. | |
| 9 | Settings > Advanced > "Select the tab to the left after closing a tab" on: closing the middle of three tabs selects the left one. | |
| 10 | Settings > General > Startup > "Open profiles window" on, restart Tandem: the Profiles window opens with the terminal window. | |
