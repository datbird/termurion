# Milestone 8f acceptance checklist

Close any open Termurion window. Copy `tandem.exe`, `tandem-host.exe`, `conpty.dll`, and
`OpenConsole.exe` from `%USERPROFILE%\src\termurion\target\release\` together, then run `tandem.exe`.
Use WSL tmux 3.4 (`tmux -CC new -s work`) unless a line says otherwise. Mark each line pass or fail.

| # | Check | Result |
|---|---|---|
| 1 | Before attaching, run `tmux set -g set-titles on; tmux set -g set-titles-string '#{pane_current_command} in #S'`. Attach: the tab reads `<prefix>bash in work`. Run `top`: within about a second it reads `<prefix>top in work`. | |
| 2 | In a tmux pane: `printf '\e]2;build box\a'`. Session > Copy Interpolated String with `\(session.tmuxPaneTitle)`: the clipboard holds `build box`. | |
| 3 | Settings > Profiles > "tmux" > General: set the icon to "Built-in Icon for Current App" and reattach. Run `vim`; Copy Interpolated String with `\(session.jobName)` gives `vim`. | |
| 4 | `cd /tmp`, press a key, wait a second: `\(session.path)` gives `/tmp`. A new local (non-tmux) tab does not start in `/tmp`. | |
| 5 | In a tmux pane: `printf '\e]1337;SetUserVar=%s=%s\a' color $(printf blue \| base64)`. `tmux show -p @uservars` shows a `u_` value. Detach and attach again: `\(user.color)` gives `blue`. | |
| 6 | In a tmux pane with tmux's `default-terminal` "screen-256color": `printf '\e[3mitalic\e[23m plain\n'`: "italic" shows in reverse video. With `tmux set -s default-terminal tmux-256color` and a reattach it shows in italics. | |
| 7 | Settings > Advanced: "Support TERM=screen", "Convert italics to reverse video in tmux integration?", "Convert italics to reverse video in tmux integration, even when TERM matches *tmux*?", and "Poll for foreground job name in tmux integration with tmux < 3.2?" are listed. | |
| 8 | With tmux 3.1 (if you have one) and "Poll for foreground job name..." on: `\(session.jobName)` follows the running program without typing. | |
