# Milestone 8e acceptance checklist

Close any open Termurion window. Copy `tandem.exe`, `tandem-host.exe`, `conpty.dll`, and
`OpenConsole.exe` from `%USERPROFILE%\src\termurion\target\release\` together, then run `tandem.exe`.
Use WSL tmux 3.4 with two sessions: `tmux new -d -s work; tmux new -d -s play`, then
`tmux -CC attach -t work`. Mark each line pass or fail.

| # | Check | Result |
|---|---|---|
| 1 | Shell > tmux > Dashboard (Shift+Ctrl+Win+Enter): a "tmux Dashboard" window shows the connection, the sessions `work` (checked) and `play`, and `work`'s windows. | |
| 2 | Select `play`: after about 1.5 s its windows show (dimmed, not open). Attach: Termurion switches to `play` (its windows open, `work`'s close). Select `work`, Attach again. | |
| 3 | Sessions +: a session "New Session" appears (`tmux ls`); + again makes "New Session 2". Select one, -: it is killed. Double-click a session name, type `renamed`, Enter: `tmux ls` shows it. | |
| 4 | Windows: close a tmux tab with Hide (8c). In the Dashboard the window shows dimmed; select it: Open in Window is on and Hide is off; Open in Window opens it. Select an open window: Hide hides it. Select two hidden ones: Open in Tabs opens both as tabs of one window. | |
| 5 | Windows +: a new window appears in the session. -: it is unlinked (`tmux list-windows`). Right-click a window name to rename it. "Search Windows" filters the list. | |
| 6 | Set the limit to 2 and attach to a session with 3 windows: none open, "Too many tmux windows!" shows, and the Dashboard opens. Set it back to 10. With "Open dashboard when attaching if there are hidden windows" on, attaching with a hidden window opens the Dashboard. | |
| 7 | Session > Bury Session (Opt+Win+Shift+B) on a pane of a window with two tabs: the pane leaves, and Session > Buried Sessions lists it. Pick it: it comes back into its tab. Bury the only tab of a window: the window closes; picking it opens it in a new window. | |
| 8 | In a buried session, `printf '\e]1337;Disinter\a'` (from a timer, for example `sleep 5; printf ...` before burying): it comes back by itself. | |
| 9 | Settings > General > tmux: "Automatically bury the tmux client session after connecting" on. Attach: the gateway pane leaves once the first tmux window opens; a "Session Buried" notification shows the first time only. Detach from a tmux tab (Shell > tmux > Detach): the gateway comes back and prints "Detached". | |
| 10 | Buried Sessions also lists hidden tmux windows as `<arrow> <client> - <name>`; picking one opens it. | |
| 11 | In a tmux pane press the Leader, then `c`: tmux makes a new window (tmux's prefix `c`). Leader then `:` does nothing (command-prompt is not bound). | |
| 12 | Settings > Keys: bind a key to "Send tmux Command" with `new-window`: in a tmux pane it opens a tmux window. | |
