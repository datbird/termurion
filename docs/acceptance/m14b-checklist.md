# Milestone 14b acceptance checklist

Build a release binary (`scripts/xt build --release`). Close any open Tandem window, copy
`tandem.exe`, `tandem-host.exe`, `tandem-git.exe`, `tandem-utils.exe`, `conpty.dll`, and
`OpenConsole.exe` from `C:\Users\datbi\src\tandem\target\release\` (the worktree build is in
`C:\Users\datbi\src\tandem-14b\target\release\`) together, then run `tandem.exe` once. Check that
the `utilities` folder beside `tandem.exe` holds `it2ssh.exe` (SSH profiles run it). Quit Tandem,
end `tandem-host.exe` (Task Manager), and start Tandem again.

Setup:

- A Linux host reachable with OpenSSH for Windows and key authentication; `ssh user@host` from
  PowerShell must connect without a password. On it: a user `zuser` whose login shell is zsh,
  a user `buser` whose login shell is bash, and Python 3 removed from PATH for both (or Python
  3.6). Once, a host (or user) with Python 3.12: 14b gives level 1 there too.
- On the same host, a user `slowuser` whose login shell is a script that stalls setup, saved as
  `/usr/local/bin/slowsh` with `chmod +x`: `#!/bin/sh`, then
  `[ "$1" = --version ] && exec sleep 60`, then `exec /bin/zsh "$@"` (`sudo usermod -s
  /usr/local/bin/slowsh slowuser`). conductor.sh runs the login shell with `--version` during
  setup, so setup waits there.
- On the same host, a user `bareuser` (zsh) whose PATH at login has no `getent`, `id`, `python3`,
  `python` or `perl`, for example a folder of symlinks to `sh`, `stty`, `base64`, `awk`, `tar`,
  `gzip`, `mkdir`, `head`, `grep`, `dd`, `cat`, `rm`, `mktemp`, `tr`, `basename`, `tty`, `env`,
  `sed` and `zsh` (the list in `scripts/record-conductor.py`) set with a `Match User bareuser`
  block and `SetEnv PATH=<that folder>` in the host's `sshd_config`.
- Copy iTerm2's own bash `it2ssh` (`OtherResources/Utilities/it2ssh` in the iTerm2 source) to
  the host as `~/it2ssh` for `zuser` (`chmod +x ~/it2ssh`; `ssh zuser@host` on the host itself
  must log in without a password), and into WSL as `~/it2ssh`. WSL needs
  tmux for check 21.
- In Windows, add a user environment variable `LANG` = `C.UTF-8` (Settings > System > About >
  Advanced system settings > Environment Variables), and create `%USERPROFILE%\.vimrc` holding
  one line `" tandem 14b`. Then start Tandem.
- A profile `SSH test`: General > Command type SSH, Command `zuser@host`; Configure…: SSH
  Integration on, Environment Variables to Copy `LANG`, Files to Copy `~\.vimrc` to `~/` (+ >
  Files…, pick it, destination `~/`); General > "Load shell integration automatically" on.

Run every check in a new session unless it says otherwise. Mark each line pass or fail.

| # | Check | Result |
|---|---|---|
| 1 | In a cmd session in Tandem, `it2ssh` with no arguments prints ssh's usage with `usage: it2ssh` in place of `usage: ssh`, and `echo %ERRORLEVEL%` shows 255. | |
| 2 | `it2ssh -N host` prints "it2sh is meant for interactive use via SSH only and is not compatible with the -N argument." (iTerm2's "it2sh" spelling) and exits 1. `it2ssh -vvv host`, `it2ssh -tt host` and `it2ssh -- host` take the host as the option's value, as iTerm2's script does. | |
| 3 | Open a tab with `SSH test`: a banner line starting `ssh ` names `zuser@host`, then the zsh prompt appears. Run `ls` twice, then Edit > Marks and Annotations > Previous Mark: the view jumps to the earlier prompt (iTerm2's zsh integration loaded by injection). | |
| 4 | Same session: `echo $LANG` prints `C.UTF-8`; `cat ~/.vimrc` prints `" tandem 14b`. | |
| 5 | Set `SSH test` > General > Initial working directory to Custom with the path `/tmp`, and open a tab: `pwd` prints `/tmp`. Set the path to `C:\Users` and open a tab: `pwd` prints the remote home (a Windows path never reaches the remote; iTerm2 would send it). Set it back to Home. | |
| 6 | Open a tab with `SSH test` and, right after it opens, type `echo early` and Enter before the prompt shows: once the prompt appears, `echo early` runs once and prints `early` once; nothing typed is lost or doubled. | |
| 7 | In PowerShell in Tandem, `it2ssh buser@host`: the bash prompt appears, with no shell integration marks (Previous Mark finds none): iTerm2 drops bash's `--posix` at level 1, so bash integration does not load. | |
| 8 | In PowerShell in Tandem, `it2ssh zuser@host`, then `exit` on the remote: the banner "ssh exited." shows and the PowerShell prompt comes back without pressing Enter (Tandem's `\x03\n` line released it2ssh's wait for a line). | |
| 9 | In PowerShell, `it2ssh zuser@host`; on the remote run `~/it2ssh zuser@host` (iTerm2's script, a second hop to the same host) and at once type `echo two` and Enter: it runs once after the second prompt appears (the second setup chains onto the first and holds nothing beyond its connecting stage). Then `exit`: the banner reads "ssh exited", an em dash, then "now at host." (with `zuser@` in front of the host when your Windows user name is not `zuser`), and the first remote's prompt returns without pressing Enter; `exit` again shows "ssh exited." and the PowerShell prompt. | |
| 10 | Repeat check 9 up to the second prompt, then end `ssh.exe` in Task Manager: the banner reads "2 ssh sessions ended.." (two dots, iTerm2's text) and the PowerShell prompt returns. | |
| 11 | Against the Python 3.12 host (or user), `it2ssh` gives the same level 1 session as check 3: the prompt, the variable and the file (14c adds level 2). | |
| 12 | `it2ssh bareuser@host`: setup finishes and the zsh prompt shows marks (check 3's test). conductor.sh on that host prints "using_passwd: not found" inside getshell's reply; Tandem skips that line and takes the shell from the next one (iTerm2 bug B15, guarded), where iTerm2 would take the error text as the shell and load no integration. | |
| 13 | In PowerShell, `it2ssh slowuser@host`: after the `ssh ` banner nothing happens; type `echo early` and Enter. After 30 seconds "An error occurred while setting up the SSH environment:", then "getshell: No response after 30 seconds", then "ssh exited." appear; the `echo early` typed during the stall is dropped, as in iTerm2, and never reaches conductor.sh (nothing runs it on the remote); the pane shows output again and what you type now goes to the remote at once (conductor.sh is still waiting there, with echo off, so it may show nothing). Close the tab to end the session, as in iTerm2. | |
| 14 | In PowerShell, `it2ssh slowuser@host`; while it waits, end `ssh.exe` in Task Manager, then press Enter once. After 30 seconds the three lines of check 13 show and the PowerShell prompt does not return yet: the Enter typed while connecting is dropped, as in iTerm2. Press Enter again: it2ssh's wait for a blank line ends and the PowerShell prompt returns, as in iTerm2. | |
| 15 | In PowerShell, `it2ssh zuser@host` and, at once, type `echo one` and Enter while the banner is the only thing shown: `echo one` runs once on the remote after the prompt appears (check 6 from a plain it2ssh). | |
| 16 | Settings > Advanced > General: turn "Disable potentially insecure escape sequences" on, then run check 8: no "ssh exited." banner shows and it2ssh waits: press Enter and the PowerShell prompt returns (`EndSSH=` is ignored, as in iTerm2). Turn the setting off. | |
| 17 | In PowerShell, `it2ssh zuser@host`, then on the remote `printf '\e[31m'` and `exit`: the "ssh exited." banner is a gray line and ends with the default colors, so what PowerShell prints after it is not red; iTerm2 restores the colors set before the banner (a kept difference). The `ssh ` banner of check 3 is the same gray line; the error lines of check 13 are in the default colors, where iTerm2 prints them in the current colors. | |
| 18 | Start the previous Tandem build (before milestone 14a) and open a tab so its `tandem-host.exe` runs; quit that Tandem, leaving `tandem-host.exe` running. Start the new `tandem.exe` from its own folder and open a tab with `SSH test`: the window shows "The session host is too old for this SSH session, so it runs inside Tandem." and the session connects as in check 3. End the old `tandem-host.exe` afterwards. (Untested by the suite: it needs a window.) | |
| 19 | Rename `utilities\it2ssh.exe` beside `tandem.exe` and open a tab with `SSH test`: the window shows "The utilities folder has no it2ssh.exe, so this SSH profile runs plain ssh." and plain ssh logs in. Rename it back. | |
| 20 | In an `SSH test` session, run `tmux -CC` on the remote (tmux installed there): tmux integration windows open and typing in them works; detach (the tmux menu or `tmux detach` in a pane) returns to the zsh prompt, and `exit` shows "ssh exited.". | |
| 21 | In a WSL session, run `tmux -CC`; in a pane of the tmux integration window, run `~/it2ssh zuser@host` (iTerm2's script): each setup step goes on at once (no step waits for a keypress or other tmux traffic) and the zsh prompt appears within a few seconds. | |

## Kept bugs and rulings a person can see

| # | Item | Result |
|---|---|---|
| 22 | Add a Windows user environment variable `FOO` = `a b` (as `LANG` above, then restart Tandem) and set Environment Variables to Copy to `LANG FOO`. Open `SSH test`: `echo $FOO` prints nothing (iTerm2 bugs B1 and B2 kept: the value goes as `$'a b'`, conductor.sh splits it and does not export it, and reports success). With `FOO` = `x=1`, `echo $FOO` prints `$'x=1'` literally. | |
| 23 | Files to Copy with the destination `xy/dir` (not starting with `~` or `/`): the file lands in `~/dir` (iTerm2 bug B8 kept: the first two characters are dropped). | |
| 24 | A variable or file only reaches the remote from Tandem's own `it2ssh.exe` (the origin secret proves the session is local); iTerm2's bash `it2ssh` run in WSL copies none of them (check 21's session: `echo $LANG` shows the host's own value). | |
| 25 | A Files to Copy entry larger than 64 MiB, or one on a network path (`\\server\share\x`), is not copied: a gray banner line "A Files to Copy tarball for /$HOME was skipped: " names the path and the reason, the files sent with it in the same tarball are skipped too, and the rest of setup goes on. | |
| 26 | Against a host whose `/bin/sh` is dash (Debian, Ubuntu), make an incompressible 4 MiB file in PowerShell: `$b = New-Object byte[] 4MB; (New-Object Random).NextBytes($b); [IO.File]::WriteAllBytes("$env:USERPROFILE\big.bin", $b)`, and add `~\big.bin` to `SSH test`'s Files to Copy with destination `~/`. Open `SSH test`: only the `ssh ` banner shows for about 9 minutes (conductor.sh reads a command at a cost that grows with the square of its size), then the prompt appears with no "No response" error, and `ls -l ~/big.bin` shows 4194304. Tandem allows a write 30 seconds plus 300 seconds per square MiB of its encoded size (about 4 hours here), where a fixed 30 seconds failed it; iTerm2 waits with no limit. Where `/bin/sh` is bash the same copy takes about 2 hours, in iTerm2 as well. Take the file out of Files to Copy afterwards. | |

The `\x03\n` line (check 8), the session host that is too old (check 18), the chain of two hops
(check 10), the banner colors (check 17), `EndSSH=` under the insecure-sequences setting (check
16) and the remote working directory (check 5) are 14a's checks 26 to 32, carried here.
