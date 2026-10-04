# Milestone 14c acceptance checklist

Build a release binary (`scripts/xt build --release`). Close any open Tandem window, copy
`tandem.exe`, `tandem-host.exe`, `tandem-git.exe`, `tandem-utils.exe` (SSH profiles run its
`utilities\it2ssh.exe`), `conpty.dll`, and `OpenConsole.exe` from
`C:\Users\datbi\src\tandem\target\release\` (the worktree build is in
`C:\Users\datbi\src\tandem-14c\target\release\`) together, then run `tandem.exe` once. Check that the
`utilities` folder beside `tandem.exe` holds `it2ssh.exe`. Quit Tandem, end `tandem-host.exe`
(Task Manager), and start Tandem again.

Setup:

- Host A: a Linux host with Python 3.7 or later, reachable with OpenSSH for Windows and key
  authentication (`ssh user@host` from PowerShell connects without a password), with `vim`, `htop`,
  `tmux` and `mpstat` (the sysstat package) installed. For the Clock checks it should be in a time
  zone other than this PC's (set `TZ` for the user or pick a host in another region).
- Host B: a Linux host with only Python 3.6 or none on PATH.
- Host C: a second host reachable from host A with a key (`ssh userc@hostc` from host A logs in
  without a password), for the nested checks.
- The `SSH test` profile of milestone 14b (General > Command type SSH, Command `user@hostA`,
  Configure...: SSH Integration on), and a copy `SSH old` with Command `user@hostB`. Either
  profile or `it2ssh user@host` typed in a Tandem pane from 14a reaches the same code.
- A badge on `SSH test` (Settings > Profiles > General > Badge): `\(session.sshIntegrationLevel)
  \(session.uname)`. Add the status bar components Clock (with "Show server time in SSH
  integration?" and "Use server time zone in SSH integration?" on), Job Name and CPU Utilization.

Run every check in a new session unless it says otherwise. Mark each line pass or fail.

| # | Check | Result |
|---|---|---|
| 1 | Open `SSH test` (host A). The prompt appears, and the badge reads `2 Linux`. Run `it2getvar session.sshIntegrationLevel`, `homeDirectory`, `shell` and `uname`, or add them to a badge: level 2, the host's home directory, its login shell and `Linux`. (The remote's values are cleaned of control characters and cut at 1024 characters.) | |
| 2 | In that session run `vim`, then `htop`, then `tmux -CC` (tmux integration attaches as in milestone 8). Each renders and takes keys; leave each with its own quit key. | |
| 3 | Paste 5000 characters (for example `python -c "print('x'*5000)"` copied on the PC) into `cat > /tmp/p.txt`, press Ctrl+D, then `wc -c /tmp/p.txt` on the host: 5000 (plus the newline if you pasted one). The paste arrives whole, in order. | |
| 4 | Run `printf '\033[c'` on the host: a terminal answer (a `^[[?...c` text) is printed and the session stays usable (a device-attributes reply does not corrupt the framed channel; typing still reaches the remote). | |
| 5 | Start `sleep 300` (or `vim`) on the host: the tab title and the Job Name component show it; leave it and the title returns to the shell. A login shell shows its leading "-" in the job (iTerm2 quirk). | |
| 6 | Close the tab while `vim` runs: the question names `vim` (the last remote process list; a job started less than a second before closing may be missed, ruling). With more than 32 remote programs the list ends "plus N others". | |
| 7 | Run `yes > /dev/null` on the host for 10 seconds: the CPU Utilization component shows the remote CPU rising (not this PC's). On a host without `mpstat` the graph stays empty (iTerm2 same). | |
| 8 | With Clock on a host in another zone: the Clock shows the host's time followed by a globe glyph (Segoe Fluent Icons, not an emoji). Turn off "Use server time zone in SSH integration?" for that bar component: the zone part goes. Turn off "Show server time in SSH integration?": the Clock shows this PC's time with no globe. The time is refreshed about every 15 minutes and on a host change. | |
| 9 | Run `sudo -k; sudo true` (or `su`) on the host: at the password prompt the password manager's key cursor shows (a framed channel carries the remote's echo-off flag, like a local prompt). Cancel with Ctrl+C. | |
| 10 | Resize the window while `htop` runs. Over the ConPTY transport (the default, ruling 1) `htop` reflows to the new size. If the transport fell back to pipes, a resize does not reach the remote and `htop` keeps its old size: that is documented, not a failure. | |
| 11 | Open the Shell menu > ssh: the Disconnect item reads "Disconnect from userA@hostA" (the host's description) and is enabled. In a local pane it is disabled. Choose it: a gray banner "Disconnected from userA@hostA" shows and the ssh session ends. The terminal then waits: press Enter once and the local prompt returns (it2ssh waits for a newline, iTerm2 quirk). | |
| 12 | Open `SSH old` (host B, Python older than 3.7): the badge reads `1 Linux` (or an empty uname), typing works, and Shell > ssh > Disconnect shows "Can\u{2019}t Disconnect"; choosing it raises an alert "Advanced SSH features are unavailable because Python 3.7 or later was not found on userB@hostB" with one OK button and no "Do not show again" box (iTerm2 passes no identifier). Closing the alert counts as OK. | |
| 13 | In `SSH test` run `ssh userc@hostc` (use `cd /tmp` first and `cd /var` there). The second hop frames too: the badge still reads level 2, the Disconnect item names the innermost host, the Job Name shows the hop's jobs. | |
| 14 | In the nested session, Session > Duplicate Session (or Shell > Duplicate Session): the new session reconnects through both hops (host A, then host C) and opens in the same remote directory (`/var`). Run `hostname` to confirm. | |
| 15 | In the nested session, Session > Split Vertically: the new pane opens ssh to the innermost host (host C) directly, not through host A. | |
| 16 | In `SSH test` (single hop, directory `/tmp`), quit Tandem with the session open (File > Quit; tandem-host keeps it) and start `tandem.exe` again: the session returns and keeps working; typing reaches the remote, the badge, Job Name and Disconnect item are right, and jobs update. | |
| 17 | After 16, Duplicate Session in the restored session: a plain new session of the same profile opens (a restored conductor is never vouched for by Tandem's origin token, so no local ssh command is built from it; ruling). A session opened fresh from the profile after the restart duplicates through its hops. | |
| 18 | Kill Tandem without saving state (Task Manager > End task on `tandem.exe`, leave `tandem-host.exe`) and start it again: "Recovering ssh connection\u{2026}" shows, then "ssh connection recovered!", and the session works. Recovery turns on only for a session that began with a hook that passed Tandem's origin token check (an SSH profile, or it2ssh run in a Tandem pane): an it2ssh from iTerm2's own bash script on a Mac or WSL does not enable it (ruling). | |
| 19 | In a local PowerShell pane (no ssh), print a recovery attempt: `[Console]::Out.Write([char]27 + ']1337;it2ssh=a b c d' + [char]7 + [char]27 + ']134;:begin-recovery' + [char]7)`. Nothing is typed into the shell and no host appears in the Disconnect title (an it2ssh OSC alone never enables recovery). | |
| 20 | Arrangements: save an arrangement (Window > Save Window Arrangement as...) holding the nested session, then edit its saved file so a "Pending Jumps" entry is not in exact `printf "%q "` form (for example add a space or a `;`), and restore it: the hop list is dropped and the pane starts as an ordinary session of its profile. With the unedited file the restore asks to trust the arrangement and the prompt lists each hop's text; accepting reconnects through the hops. | |
| 21 | Pending Jumps apply only to a session of an SSH profile; a plain profile's session ignores them. | |
| 22 | A coprocess started on a framed session receives only the remote's raw output (depth 0), and on `SSH old` (level 1) it receives nothing (iTerm2 quirk, ruling 14c-4). Its output typed into the session reaches the remote. | |
| 23 | Wire quirks kept for parity (invisible but pinned by tests): the framer deregistration command is spelled `dereigster`; Disconnect and the framed shell's exit send the pre-framer `quit` behind the line prefix; a background job whose stderr speaks completes twice. | |
| 24 | A laptop that sleeps for a minute with a framed session open: the session still works after waking (after framing only file commands keep the 30 second timeout, M14-I2). | |
| 25 | Windows-only values with no ssh: in a local pane the badge reads `0 Windows_NT` and `homeDirectory` is `%USERPROFILE%`; a script's `get_time_offset` answers the Windows zone name as `tz`. | |
| 26 | `\(session.shell)` in a local pane gives the shell integration's shell, else the profile command's program name. | |

Rulings that decide what you see (full list with costs in
`docs/superpowers/rulings/m14c-rulings.txt`): framer.py and it2ssh ship byte for byte; hook nesting
depth is capped at 8 (a ninth hop's integration shows as plain output); every remote size is capped;
input is never sent raw to the pty while any ancestor conductor frames; a local ssh command is
built only from a chain Tandem's origin token vouched for; recovery starts only after a vouched
hook; a restored conductor is never vouched; arrangement Pending Jumps must be in exact `printf "%q "`
form or the list is dropped, and are shown in the trust prompt.
