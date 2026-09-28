# Milestone 14a acceptance checklist

Build a release binary (`scripts/xt build --release`). Close any open Tandem window, copy
`tandem.exe`, `tandem-host.exe`, `tandem-git.exe`, `tandem-utils.exe`, `conpty.dll`, and
`OpenConsole.exe` from `C:\Users\datbi\src\tandem\target\release\` (the worktree build is in
`C:\Users\datbi\src\tandem-14a\target\release\`) together, then run `tandem.exe` once. Check that
the `utilities` folder beside `tandem.exe` now holds 17 programs, `it2ssh.exe` among them. Quit
Tandem, end `tandem-host.exe` (Task Manager), and start Tandem again.

Setup: have a Linux host you can reach with key authentication, or WSL's own sshd (in WSL,
`sudo service ssh start`, with your Windows public key from `%USERPROFILE%\.ssh` in WSL's
`~/.ssh/authorized_keys`); `ssh user@host` from PowerShell must connect without a password
before you start. Below, `user@host` is that login. Have WSL with bash (or Git Bash) for the
escape-sequence checks, and copy iTerm2's own bash `it2ssh` (`OtherResources/Utilities/it2ssh` in
the iTerm2 source) into WSL as `~/it2ssh` with `chmod +x ~/it2ssh`. Make a profile named `SSH
test`. Run every check in a new session unless it says otherwise. Mark each line pass or fail.

Until milestone 14b adds the conductor, SSH integration runs plain ssh with a one-line notice;
the live handshake, `-N`, the no-argument usage, and the checks listed at the end are 14b's.

| # | Check | Result |
|---|---|---|
| 1 | Settings > Profiles > `SSH test` > General: set Command type to SSH. A "Configure…" button appears beside it. Switch to Login Shell, Command, Custom Shell and Browser in turn: the button is gone for each. | |
| 2 | With Command type SSH, click "Configure…": the sheet shows "SSH Integration" (checked), "Environment Variables to Copy to Remote Host:" with a text field, "Files to Copy to Remote Host:" with a two-part table ("File or Folder", "Destination") and + and minus buttons, "Path to SSH:" with the placeholder `C:\Windows\System32\OpenSSH\ssh.exe` and "Browse", a "?" button, OK and Cancel. | |
| 3 | In the sheet, click +: it offers "Files…" and "Folder…". Choose "Files…" and pick a file under your home folder: the sheet asks "Destination folder on remote host:" with `~` filled in; click Add and a row appears whose source starts `~\` and whose destination is `~`. Do the same with "Folder…"; Cancel at the destination adds nothing. Select a row: the minus button is enabled and removes it; with no row selected it is disabled. | |
| 4 | Click "Browse": the dialog is titled "Select SSH Binary". Type the full path of a `.txt` file in the dialog (its filter shows only programs): "Selected file must be an executable." shows and the path is unchanged. Pick `C:\Windows\System32\OpenSSH\ssh.exe`: the path field holds it. | |
| 5 | Click "?": the browser opens `https://gitlab.com/gnachman/iterm2/-/wikis/SSH-Integration`. | |
| 6 | Type `FOO BAR` in the variables field and click OK. Reopen the sheet: the field reads `FOO BAR`, the file rows and the path are kept. Change them all and click Cancel; reopen: the OK'd values are back. | |
| 7 | Uncheck "SSH Integration": the variables field and the files table (with its + and minus buttons) are disabled; check it again and they are enabled. | |
| 8 | Set the Command to `user@host`, SSH Integration on, and open a tab with `SSH test`: the session runs ssh and logs in to the host (no shell prompt from Windows first), and the window shows the banner "SSH integration arrives in a later Tandem build; this SSH profile runs plain ssh." | |
| 9 | Turn SSH Integration off (Configure…, uncheck, OK) and open another `SSH test` tab: it logs in the same way and shows no SSH banner. `exit` on the remote closes the session like any other command ending. | |
| 10 | Clear the Command of `SSH test` (Command type still SSH) and open a tab: it runs your default shell, with no SSH banner. | |
| 11 | In a PowerShell session, `it2ssh user@host` prints "it2ssh: SSH integration arrives in a later Tandem build; running plain ssh." and then logs in; `exit` returns to PowerShell. `$env:TANDEM_SSH_TRANSPORT` in that PowerShell session is empty. | |
| 12 | In a WSL (or Git Bash) session in Tandem, run `printf '\033]1337;it2ssh=a b c d\a\033]1337;SendConductor=v=2\a'`: a gray banner line "Out-of-date version of it2ssh detected. Please upgrade it2ssh." appears, and typing `echo ok` then Enter still works. | |
| 13 | Same session: `printf '\033]1337;it2ssh=a b c d\a\033]1337;SendConductor=v=4\a'` shows "Future version of it2ssh detected. Please upgrade Tandem."; `printf '\033]1337;SendConductor=v=4\a'` alone (no it2ssh first) shows nothing; typing still works. | |
| 14 | Same session: `printf '\033]1337;it2ssh=a b c d\a\033]1337;SendConductor=v=3\a'` shows no banner, and typing still works: nothing is typed into the shell for you and no keystroke is held back. | |
| 15 | In a WSL session in Tandem, run `~/it2ssh user@host` (iTerm2's own script): it connects and then waits, with no banner. Type a few letters: nothing is lost or typed into your local shell. Press Enter, then `~.` to end ssh; the script then waits for a line: press Enter and the WSL prompt returns, and typing works. | |
| 16 | Open `docs/superpowers/rulings/m14a-rulings.txt`: its first line names the transport the probe measured and Robert chose, the shipped ConPTY in passthrough mode, and its cost (invalid UTF-8 inside framed output arrives as U+FFFD). | |
| 17 | In the menu bar, Shell > ssh's three items (Disconnect, Download Files..., Remote host can control iTerm2) are shown disabled, and clicking them does nothing (they belong to milestone 14). | |

## Kept bugs and rulings a person can see

| # | Item | Result |
|---|---|---|
| 18 | The version banners and the end banner end with the default colors (SGR reset); iTerm2 restores the colors that were set before the banner (see checks 12 and 13). | |
| 19 | "Future version of it2ssh detected. Please upgrade Tandem." says Tandem where iTerm2 says iTerm2; the out-of-date text is iTerm2's word for word (checks 12 and 13). | |
| 20 | `printf '\033]1337;Env=FOO=YmFy\a'` (the per-name form the parity row describes) does nothing: iTerm2 reads only `Env=report=all:<base64 env>`. | |
| 21 | With Settings > Advanced > General > "Disable potentially insecure escape sequences" on, `EndSSH=` is ignored while `it2ssh=` and `SendConductor=` still act (the version banners of checks 12 and 13 still show), as in iTerm2. | |
| 22 | An SSH profile's Command is split by Windows' rules, not a shell's: a Command `user@host 'ls -l'` passes `'ls` and `-l'` to ssh as two words, where iTerm2 would pass `ls -l` as one. | |
| 23 | The variables field splits on spaces and commas and stores a list; clearing it stores an empty list (iTerm2's token field stores one empty string). The files table has a row-number column you click to select a row. | |
| 24 | "+" asks Files… or Folder… (Windows cannot choose both in one dialog), and the destination is asked in the sheet after the dialog, where iTerm2 puts it in the panel; Browse starts in the current path's folder or `C:\Windows\System32\OpenSSH` and accepts only `.exe` files. | |
| 25 | An SSH profile with a blank Command runs the shell and shows no SSH banner (check 10); iTerm2 has no such banner at all (the notice is Tandem's until 14b). | |
| 26 | A working directory in the profile only reaches the remote when it starts with `/` or `~`; iTerm2 sends the local folder's path. (Seen once 14b's conductor runs.) | |

## Carried to milestone 14b's checklist

These need the conductor (`CONDUCTOR_READY` on) and cannot be reached in 14a; 14b's checklist runs
them.

| # | Item | Result |
|---|---|---|
| 27 | EndSSH for a registered hop writes `\x03\n` into the pane, which releases Tandem's `it2ssh` waiting in its console read loop after ssh died while the conductor was being typed (only a registered hop makes EndSSH write it). | 14b |
| 28 | An SSH-Integration profile started through a session host older than protocol 4 shows "The session host is too old for this SSH session, so it runs inside Tandem." and the pane runs locally (only a passthrough launch can be refused). | 14b |
| 29 | `it2ssh` with no arguments prints ssh's usage with "usage: it2ssh" and exits 255; `it2ssh -N host` prints iTerm2's message (with its "it2sh" spelling) and exits 1; `-vvv host`, `-tt host` and `-- host` take the host as the option's value, as iTerm2's script does. | 14b |
| 30 | Ending a chain of two ssh hops shows "2 ssh sessions ended.." (two dots when no conductor remains), as iTerm2. | 14b |
