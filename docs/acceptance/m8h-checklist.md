# Milestone 8h acceptance checklist

Close any open Termurion window. Copy `tandem.exe`, `tandem-host.exe`, `conpty.dll`, and
`OpenConsole.exe` from `C:\Users\datbi\src\termurion\target\release\` together, then run `tandem.exe`.
Use WSL tmux 3.4 (`tmux -CC new -s work`). Mark each line pass or fail.

| # | Check | Result |
|---|---|---|
| 1 | Split a tmux tab (two panes). Session > Move Session > Move Session to Tab: the focused pane becomes a new tmux window, shown as a tab of this Termurion window. | |
| 2 | Split again. Move Session to Window: the pane opens in a new Termurion window a little right of and below the first. | |
| 3 | In a local (non-tmux) tab with two panes, Move Session to Tab: the pane becomes its own tab. | |
| 4 | Move and resize the tmux Termurion window, Edit Tab Title on one tmux tab ("logs"), then detach (Shell > tmux > Detach) and attach again: the window opens at the same place and size, and the tab reads `<prefix>logs`. | |
| 5 | Put a window on a second monitor, detach, unplug or disable that monitor, attach: the window opens on a monitor you can see. | |
| 6 | Settings > Advanced: "Disable window position restoration in tmux integration." on: after a reattach the window opens where Windows puts it. Turn it off again. | |
| 7 | Close a tmux tab with Hide (it was the second tab). Reopen it from the Dashboard: it comes back as the second tab of the same window. | |
| 8 | Settings > Profiles > "tmux" > Colors: "Use custom tab color" on with a color: tmux tabs take it. Detach and attach: still colored. A local profile with a tab color colors its tabs too. | |
