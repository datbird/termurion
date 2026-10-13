# Milestone 7d acceptance checklist

Close any open Termurion window. Copy `tandem.exe` and `tandem-host.exe` from
`C:\Users\datbi\src\termurion\target\release\` together (they must stay in the same folder), then run
`tandem.exe`. Cmd is the Win key. Mark each line pass or fail.

| # | Check | Result |
|---|---|---|
| 1 | Task Manager > Details shows `tandem-host.exe` running under your user while Termurion is open. Your shells (pwsh or cmd) are children of `tandem-host.exe`, not of `tandem.exe`. | |
| 2 | Open two windows. Split one of them, run `dir` in each pane, and `cd` somewhere different in one pane. Quit with Cmd+Q, then start Termurion: both windows, the split, and each pane's text come back. Under each pane's text is a gray full-width line "Session Contents Restored on <date> at <time>", and a new prompt starts below it in the folder the pane was in. | |
| 3 | Colors come back: run `Write-Host red -ForegroundColor Red` (or `echo` with color in cmd), quit, relaunch: the restored word is red. | |
| 4 | Run `ping -t 127.0.0.1` in a pane. Wait 70 seconds, then end `tandem.exe` in Task Manager (End task). Start Termurion: the window comes back with the ping STILL running in the same pane (new lines keep arriving), no restored-contents banner on that pane, and no lines appear twice at the join. | |
| 5 | Item 4 again, but end Termurion within 5 seconds of starting the ping: the pane still takes the running ping back (lines from before the last save may be missing, none doubled). | |
| 6 | Settings > Advanced > Session: turn off "Quit ends all running jobs". Run `ping -t 127.0.0.1`, press Cmd+Q: the quit question (if Confirm Quit asks) says "Sessions will be restored automatically when Termurion is relaunched." Relaunch: the ping is still running. Turn the setting back on. | |
| 7 | With the setting on (the default), start `ping -t 127.0.0.1` and Cmd+Q: the question says "All sessions will be closed." After the quit, `tandem-host.exe` exits within about 5 seconds and no ping.exe is left in Task Manager. | |
| 8 | Close every window with its close box (the last one ends Termurion). Start Termurion: one fresh window opens, nothing is restored. | |
| 9 | Orphans: run `ping -t 127.0.0.1`, end `tandem.exe` in Task Manager, delete `%APPDATA%\Tandem\state`, start Termurion. A second window opens with the running ping as a tab and the notice "This already-running session was restored but its contents were not saved." | |
| 10 | Start a second copy of `tandem.exe` while the first runs: it opens one fresh window (it does not restore the first Termurion's windows) with a notice that its shells run inside Termurion this time. The first Termurion's panes keep working, and quitting the second copy does not change what the first restores. | |
| 11 | Settings > Advanced > Terminal: turn off "Restore window contents at startup". Quit and relaunch: windows and splits come back, panes start empty (no old text, no banner). Turn it back on. | |
| 12 | Settings > General > Startup, window restoration "Open Default Window Arrangement" (with a saved default arrangement): relaunch opens the arrangement, not the last windows. Set it back to "Use System Window Restoration Setting". | |
| 13 | Move a window partly off the right edge of the screen, quit, relaunch: it comes back fully on screen, the same size. | |
| 14 | Settings > Advanced > Session: turn off "Enable session restoration", restart Termurion: shells are children of `tandem.exe` again and no `tandem-host.exe` starts. Turn it back on and restart. | |
| 15 | Write garbage into `%APPDATA%\Tandem\state` (for example with Notepad) and start Termurion: a normal window opens, a notice says the saved windows could not be restored, and the file is now `state.bad`. | |
