# Milestone 8d acceptance checklist

Close any open Termurion window. Copy `tandem.exe`, `tandem-host.exe`, `conpty.dll`, and
`OpenConsole.exe` from `C:\Users\datbi\src\termurion\target\release\` together, then run `tandem.exe`.
Use WSL tmux 3.4 (pause mode needs 3.2 or later). Settings > General > tmux: set "Pause a pane if
catching up would take longer than (seconds):" to 10 for these checks. Mark each line pass or fail.

| # | Check | Result |
|---|---|---|
| 1 | `tmux -CC new`. In the tmux pane run `yes | pv -L 50m >/dev/null` is too fast to show; instead run `cat /dev/urandom | base64` and keep Termurion busy (drag the window around) until output lags. A bar says "This session will pause in about N seconds because it is buffering too much data." with Pause Settings; N counts down. | |
| 2 | When tmux pauses the pane, the bar changes to "tmux paused this session because too much output was buffered." with Unpause and Settings. The pane stops changing. Ctrl+C in it, then Unpause: the pane shows tmux's current screen (no doubled text), and typing works. | |
| 3 | Shell > tmux > Pause Pane on a quiet tmux pane: the bar says "Session paused." and the menu item is checked. Pause Pane again (or Unpause): the pane comes back and the check goes. | |
| 4 | Settings: "Unpause automatically" on, repeat item 1 to 2: when tmux pauses the pane no bar shows, and it runs again after about a second. Turn it off. | |
| 5 | Settings: "Warn before pausing" off: item 1 shows no warning bar (the paused bar still shows). Turn it on. Advanced > "Suppress announcement that tmux will pause a session." on: no warning either. Turn it off. | |
| 6 | Change the age limit to 30 while attached; from a WSL shell outside tmux, `tmux show -g pause-after` is not a tmux option, so instead check that item 1 now takes longer before the warning. Set it back to 120. | |
| 7 | Attach to a tmux older than 3.2 if one is at hand (else "n/a"): Pause Pane does nothing and no warnings show. | |
