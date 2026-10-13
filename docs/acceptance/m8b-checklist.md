# Milestone 8b acceptance checklist

Close any open Termurion window. Copy `tandem.exe`, `tandem-host.exe`, `conpty.dll`, and
`OpenConsole.exe` from `C:\Users\datbi\src\termurion\target\release\` together, then run `tandem.exe`.
The checks use tmux 3.4 in WSL. Edits from Termurion (splits, divider drags, closing, renaming into
tmux) are 8c. Mark each line pass or fail.

| # | Check | Result |
|---|---|---|
| 1 | In WSL: `tmux new -d -s work`, `tmux split-window -h -t work`, `tmux new-window -t work -n logs`. Then `tmux -CC attach -t work`: two new Termurion windows open (Settings > General > tmux "When attaching, open unrecognized windows in" is Windows). The first shows two panes side by side, the second one pane. Tab titles start with the arrow prefix and show the tmux window names. | |
| 2 | The panes show what the shells had before (history included: scroll up). Type `ls` Enter in a pane: it runs there. Type `echo "a;b" && printf '\t'` Enter: the text arrives exactly. | |
| 3 | Run `vim` in one pane, quit it: the screen comes back. Detach (Esc in the gateway), attach again while vim runs: the vim screen shows, and after `:q` the shell screen underneath is the one from before vim. | |
| 4 | With the window number shown (Settings > Appearance), the title of a tmux window ends with ` [<client name>]`. The gateway's own title reads `[<arrow> <title> <client>]`. | |
| 5 | From a WSL shell outside tmux: `tmux split-window -v -t work:0`: the first window's right side splits into two; the old panes keep their text. `tmux kill-pane -t work:0.1`: that pane closes in Termurion. | |
| 6 | `tmux new-window -t work -n extra`: a new tab opens in the focused Termurion window (anonymousTmuxWindowsOpenInCurrentWindow is "Focused window"). `tmux rename-window -t work:extra renamed`: the tab title changes. `tmux kill-window -t work:renamed`: the tab closes. | |
| 7 | `tmux select-pane -t work:0.0` from outside: Termurion focuses that pane. Click the other pane in Termurion: `tmux display -p -t work '#{pane_index}'` from outside reports it. | |
| 8 | Settings: "When attaching, open unrecognized windows in" = "Tabs in a new window". Detach, attach again: all tmux windows open as tabs of ONE new window. Set it to "Tabs in the attaching window", detach, attach: they open as tabs next to the gateway's tab. Set it back to Windows. | |
| 9 | Create 11 tmux windows (`for i in $(seq 11); do tmux new-window -d -t work; done`), then attach: no window opens and a notification says "Too many tmux windows!" / "Use the tmux dashboard to select which to open." Kill the extra windows. | |
| 10 | Settings > Profiles now lists a profile named "tmux" (made on the first attach, scrollback 1000). Change its background color: new tmux panes use it (detach and attach again). | |
| 11 | In a tmux pane run `cat -v`, then press Ctrl+Alt+x and Shift+Enter: the pane shows what tmux sends for those keys (tmux 3.4 encodes named keys itself). Ctrl+C ends it. | |
| 12 | Detach with Esc in the gateway: the gateway prints "Detached"; the tmux panes' sessions end (each follows the profile's "After a session ends"). | |
