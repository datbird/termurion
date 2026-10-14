# Milestone 8c acceptance checklist

Close any open Termurion window. Copy `tandem.exe`, `tandem-host.exe`, `conpty.dll`, and
`OpenConsole.exe` from `%USERPROFILE%\src\termurion\target\release\` together, then run `tandem.exe`.
Use WSL tmux 3.4: `tmux new -d -s work`, then `tmux -CC attach -t work`. Mark each line pass or fail.

| # | Check | Result |
|---|---|---|
| 1 | Resize the tmux window with the mouse: from a WSL shell outside tmux, `tmux display -p -t work '#{window_width}x#{window_height}'` follows the Termurion window's size in cells. Change the font size (Win+= / Win+-): the size follows again. | |
| 2 | Split the tmux tab (Win+D): tmux splits (`tmux list-panes -t work` shows two panes) and Termurion shows the new pane with a shell. Win+Shift+D splits the other way. | |
| 3 | Drag the divider between the panes and let go: during the drag nothing jumps back; after release `tmux list-panes -t work -F '#{pane_width}'` shows the new widths. | |
| 4 | Maximize Active Pane (Win+Shift+Enter): tmux zooms (`#{window_zoomed_flag}` is 1) and Termurion shows only that pane; again: both panes return. | |
| 5 | Swap the two panes (a key bound to Swap With Next Pane): tmux swaps them (`tmux list-panes` order changes) and Termurion follows. | |
| 6 | Edit Tab Title on the tmux tab: type `renamed`: `tmux list-windows -t work` shows `renamed`, and the tab title reads the prefix plus `renamed`. | |
| 7 | Close Session (Win+W) on one tmux pane: tmux kills it (`tmux list-panes` shows one pane) and it closes in Termurion. No Undo Close is offered for it. | |
| 8 | Win+T in a tmux tab: "What kind of tab do you want to open?" asks (Yes: New tmux Tab, No: Use Default Profile). Yes: a new tmux window opens as a tab in this Termurion window (`tmux list-windows` shows it). No: an ordinary tab opens. Shell > tmux > New Tmux Tab and New Tmux Window work without asking; the window one opens a new Termurion window. | |
| 9 | With two tmux tabs in a window, Move Tab Left/Right: `tmux list-windows` order follows. | |
| 10 | Close one of the two tmux tabs (Win+Shift+W or the tab's close): a bar asks "Kill tmux window, terminating its jobs, or hide it?..." with Hide, Cancel, Kill. Hide: the tab closes and `tmux show -v -t work @hidden` is set; detach and attach again: that window stays closed and a notification says "Some tmux windows were hidden." Kill: `tmux list-windows` loses it. | |
| 11 | Close the Termurion window that holds the last tmux tab: the bar asks "Kill window and its jobs, hide window from view, or detach from tmux session?..." with Hide, Detach tmux Session, Kill, Cancel. Detach tmux Session: the gateway prints "Detaching..." and "Detached". | |
| 12 | Shell > tmux > Detach: the session detaches. Stop tmux from answering (from outside, `kill -STOP $(pgrep -f "tmux.*-CC")`), then Detach twice: "A previous detach request has not yet been honored. Force detach?" OK forces it. `kill -CONT` it afterwards. | |
| 13 | Shell > tmux > Force Detach: the gateway prints "Exiting tmux mode, but tmux client may still be running." and "Detached". | |
| 14 | Attach the same session twice (a second `tmux -CC attach -t work` in another Termurion tab): a bar says "Cannot Attach: This instance of Termurion is already attached to this session" with OK, Reveal, Force Detach Other. OK: the second connection detaches. Force Detach Other (repeat the attach): the first one detaches and the second stays. | |
| 15 | Settings > Advanced: "Suppress kill/hide dialog when closing a tmux tab." on: closing a tmux tab hides it without the bar. Turn it off. | |
