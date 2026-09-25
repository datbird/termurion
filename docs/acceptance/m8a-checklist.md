# Milestone 8a acceptance checklist

Close any open Tandem window. Copy `tandem.exe`, `tandem-host.exe`, `conpty.dll`, and
`OpenConsole.exe` from `C:\Users\datbi\src\tandem\target\release\` together, then run `tandem.exe`.
The checks use tmux in WSL (Ubuntu has tmux 3.4). tmux windows do not open as tabs yet (8b).
Mark each line pass or fail.

| # | Check | Result |
|---|---|---|
| 1 | Run `wsl`, then `tmux -CC`. The pane shows "** tmux mode started **", an empty line, "Command Menu", a dashed line, and the four key lines (esc, X, L, C). No `^C` or command text shows up in the shell. | |
| 2 | Press L: "tmux logging on". Press C, type `list-sessions`, OK: "> list-sessions" and "Run command "list-sessions"" appear, then lines starting "< " and the session list (for example `0: 1 windows ...`). Press L again: "tmux logging off". | |
| 3 | Press C, type `display-message -p "%exit"`, OK: the pane prints `%exit` as a reply line and tmux mode stays on (press C again to check it still answers). | |
| 4 | Type other letters (a, b, lowercase l): nothing happens and nothing reaches tmux. | |
| 5 | Press Esc: "Detaching...", then "Detached". The WSL shell prompt comes back and takes typing normally. `tmux ls` shows the session still running. | |
| 6 | `tmux -CC attach`: the banner shows again. Press X: "Exiting tmux mode, but tmux client may still be running." then "Detached". tmux's own lines now show as plain text; press Ctrl+C or type `detach` Enter to get the shell back. | |
| 7 | `tmux kill-server`, then `tmux -CC attach`: the pane shows `tmux failed with error: "no sessions"` (curly quotes) and then "Detached". | |
| 8 | `tmux set -g aggressive-resize on` (outside -CC, in a normal `tmux` session, then detach), then `tmux -CC attach`: an alert titled "Unsupported tmux option" says the "aggressive-resize" option is turned on. OK: "Detached". Run `tmux set -g aggressive-resize off` afterwards. | |
| 9 | `LANG=C tmux -u -CC` works normally; `LANG=C tmux -CC new` (no -u) shows "UTF-8 Mode Not Detected" and ends tmux mode. (If tmux 3.4 always runs UTF-8, write "n/a".) | |
| 10 | `printf '\eP1000p'` in a normal shell: the banner appears; after about 5 seconds a bar says "Slow tmux Response: It's taking a long time for tmux to respond..." with OK, Reveal Setting, Don't Show This Again. Reveal Setting opens Settings on the profile. Press X to leave. | |
| 11 | Repeat item 10 and press Don't Show This Again: the next `printf '\eP1000p'` shows no bar. Settings > Advanced > "Suppress warning about a slow tmux response" is on; turn it off. | |
| 12 | Profiles > Terminal > "tmux uses newline (vs CRLF)" on, then `tmux -CC` in a new tab: it works the same as item 1 to 5. Turn it off. | |
| 13 | `tmux -CC`, then in another tab run `wsl -e tmux kill-server`: the gateway pane prints "Detached" (after "tmux exited unexpectedly." if tmux gave a reason). | |
| 14 | Settings > Advanced > "Tolerate unrecognized commands from server." on, then `tmux -CC` and from another tab `wsl -e tmux display-message -c <client> hi` is not needed: instead run `echo junk` is not possible in -CC, so write "n/a" unless you know a way to put text into the stream (an ssh banner does it). Turn it off. | |
