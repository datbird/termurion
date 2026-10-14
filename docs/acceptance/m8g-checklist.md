# Milestone 8g acceptance checklist

Close any open Termurion window. Copy `tandem.exe`, `tandem-host.exe`, `conpty.dll`, and
`OpenConsole.exe` from `%USERPROFILE%\src\termurion\target\release\` together, then run `tandem.exe`.
Use WSL tmux (`tmux -CC new -s work`); lines 3 to 5 need tmux 3.6 or later. Mark each line pass or fail.

| # | Check | Result |
|---|---|---|
| 1 | In a tmux pane: `tmux set-buffer hello`. A bar asks "The tmux paste buffer was updated. Would you like to mirror it to the local clipboard from now on?" [Always] [Never]. Choose Always; the clipboard is not changed yet. `tmux set-buffer world`: the clipboard holds `world`. | |
| 2 | Settings > General > tmux shows "Mirror tmux paste buffer to local clipboard" checked. Uncheck it; `tmux set-buffer again`: no bar (it was answered). | |
| 3 | With only Termurion attached: `printf '\e]4;1;?\a'; cat -v` then Enter: a `^[]4;1;rgb:...` answer comes back. | |
| 4 | Attach a second, plain client from another terminal (`tmux attach -t work` in WSL). Repeat line 3: the answer comes from that terminal, not Termurion (Termurion sends none). Detach the plain client. | |
| 5 | Settings > General > Selection: allow clipboard access; reading: Ask Each Time. With `tmux set -g set-clipboard off`: `printf '\e]52;c;?\a'` asks "Share clipboard contents with app in terminal?"; Just Once answers. With `set-clipboard on` (tmux 3.6): the question may show but no answer is sent. | |
| 6 | tmux 3.4 or later: `printf '\e]11;?\a'; cat -v` answers the profile's background color (tmux answers it). Change the profile's background color: the answer follows. | |
| 7 | Without set-titles, `printf '\e]2;build box\a'` in a tmux pane: the tab label becomes the session title (`build box` with the "tmux" profile's Session Name component). Before any title is set, tmux's pane title is the host name, so tmux tabs show the host name as iTerm2 does; with set-titles on (8f line 1) the set-titles string wins. | |
