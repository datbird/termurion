# Milestone 8a acceptance checklist

Close any open Termurion window. Copy `tandem.exe`, `tandem-host.exe`, `conpty.dll`, and
`OpenConsole.exe` from `C:\Users\datbi\src\termurion\target\release\` together, then run `tandem.exe`.
The checks use tmux in WSL (Ubuntu has tmux 3.4). tmux windows do not open as tabs yet (8b).
Mark each line pass or fail.

| # | Check | Result |
|---|---|---|
| 1 | Run `wsl`, then `tmux -CC`. The pane shows "** tmux mode started **", an empty line, "Command Menu", a dashed line, and the four key lines (esc, X, L, C). No `^C` or command text shows up in the shell. | |
| 2 | Press L: "tmux logging on". Press C, type `list-sessions`, OK: "> list-sessions" and "Run command "list-sessions"" appear, then lines starting "< " and the session list (for example `0: 1 windows ...`). Press L again: "tmux logging off". | |
| 3 | Press C, type `display-message -p "%exit"`, OK: as in iTerm2, a reply line starting `%exit` ends tmux mode. "Detached" prints, the rest of the reply shows as plain text, and tmux is still running in control mode: type `detach` and Enter to get the shell back. | |
| 4 | `tmux -CC` again. Type other letters (a, b, lowercase l), arrow keys, F5, and Alt+x: nothing happens and nothing reaches tmux. With Broadcast Input to All Panes on in a tab with a second pane, typing in the other pane does not reach the gateway, and C in the gateway types nothing into the other pane. | |
| 5 | Press Esc: "Detaching...", then "Detached". The WSL shell prompt comes back and takes typing normally. `tmux ls` shows the session still running. | |
| 6 | `tmux -CC attach`: the banner shows again. Press X: "Exiting tmux mode, but tmux client may still be running." then "Detached". tmux's own lines now show as plain text; press Ctrl+C or type `detach` Enter to get the shell back. | |
| 7 | `tmux kill-server`, then `tmux -CC attach`: the pane shows `tmux failed with error: "no sessions"` (curly quotes) and then "Detached". | |
| 8 | `tmux set -g aggressive-resize on` (outside -CC, in a normal `tmux` session, then detach), then `tmux -CC attach`: an alert titled "Unsupported tmux option" says the "aggressive-resize" option is turned on. OK: "Detached". Run `tmux set -g aggressive-resize off` afterwards. | |
| 9 | `LANG=C tmux -u -CC` works normally; `LANG=C tmux -CC new` (no -u) shows "UTF-8 Mode Not Detected" and ends tmux mode. (If tmux 3.4 always runs UTF-8, write "n/a".) | |
| 10 | A fake tmux that never answers. In the WSL shell run: `stty -echo; trap '' INT; printf '\033P1000p%%begin 1 1 0\n%%end 1 1 0\n%%session-changed $0 x\n'; sleep 30; stty echo; trap - INT`. The banner appears; after about 5 seconds a bar says "Slow tmux Response: It's taking a long time for tmux to respond..." with OK, Reveal Setting, and Don't Show This Again. Reveal Setting opens Settings on the pane's profile. Press X to leave tmux mode. | |
| 11 | Repeat item 10 and press Don't Show This Again: the next run shows no bar. Settings > Advanced > "Suppress warning about a slow tmux response" is on; turn it off. | |
| 12 | Profiles > Terminal > "tmux uses newline (vs CRLF)" on, then `tmux -CC` in a new tab: items 1 and 5 work the same. Turn it off. | |
| 13 | `tmux -CC`, then in another tab run `wsl -e tmux kill-server`: the gateway pane prints "Detached" (after "tmux exited unexpectedly." and the `%exit` line if tmux gave a reason). | |
