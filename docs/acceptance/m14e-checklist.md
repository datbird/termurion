# Milestone 14e acceptance checklist

Build a release binary (`scripts/xt build --release`). Close any open Termurion window, copy
`tandem.exe`, `tandem-host.exe`, `tandem-git.exe`, `tandem-utils.exe` (SSH profiles run its
`utilities\it2ssh.exe`), `conpty.dll`, and `OpenConsole.exe` from
`%USERPROFILE%\src\termurion\target\release\` (the worktree build is in
`%USERPROFILE%\src\termurion-14e\target\release\`) together, then run `tandem.exe` once from a folder
with a path you can name in the registry (the protocol handler stores that exact path; moving the
exe later makes Termurion ask to register again).

Nothing of 14e was seen on screen while it was built: no window was drawn, no App-level flow ran,
and the OS-level pieces were never exercised: the registry registration with OK, a browser
`ssh:` link, the second-instance hand-over over the pipe (only test pipe names were used), the
window coming forward, the flood banner, and the keyboard mode and title restore after ssh exits.
Every row below is therefore a first look. Do 14a to 14d's checklists first (SSH integration and
framing must work).

Setup:

- A Linux host reachable with OpenSSH for Windows (key login, no password prompt), and a WSL or Git
  Bash pane on this PC where `su` to a second local user works (for the two-local-hosts row).
- A second user profile or machine where another app owns `ssh:` links (PuTTY with its URL handler,
  or a browser-registered handler), if available; otherwise the "no handler" row is the only
  registration question to see.
- Termurion registered nowhere before the first row: check `HKCU\Software\Classes\ssh` does not exist
  (or note what it holds, since OK replaces it without keeping it).
- Process Monitor or Task Manager's Details tab open for the security rows; Wireshark or Process
  Monitor for the `\\attacker.example` row.
- Debug output visible (DebugView) for the refusal log lines.

Mark each line pass or fail.

| # | Check | Result |
|---|---|---|
| 1 | Profiles > General (Settings window, a profile's General tab) shows "URL schemes:" with a menu titled "Select URL Schemes..." listing `ssh` only (iTerm2's lists 14; other schemes are not milestone 14). `ssh` is unchecked and Termurion is not yet registered. Nothing is written to the registry just by opening the menu. | |
| 2 | Tick `ssh` while another app (or none) owns `ssh:` links: the question "Termurion is not the default handler for ssh. Would you like to set Termurion as the default handler?" appears with OK and Cancel and names the current handler ("The current handler is: <name>") or, only when nothing exists, "There is currently no handler.". It names EVERY entry OK replaces: an effective handler, a different program already in `HKCU\Software\Classes\ssh` ("Termurion will replace this program's registration: ..."), and a protected Windows choice that stays in force. With a program such as PuTTY registered, its name is in the question. Cancel changes nothing and the tick stays off. | |
| 3 | Answer OK: Settings > Apps > Default apps lists Termurion for SSH (or Windows says so in its own words), the tick shows, and `regedit` shows keys only under `HKCU` (`Software\Classes\ssh`, `Software\Classes\Tandem.ssh`, `Software\Tandem\Capabilities`, `Software\RegisteredApplications\Tandem`), nothing under `HKLM` and nothing written to a `UserChoice` key. If Windows keeps another app (a protected user choice), Termurion says so ("Windows keeps another app as the handler for ssh links. Choose Termurion for ssh in Settings > Apps > Default apps.") and the tick stays off until Default apps is changed. | |
| 4 | Win+R `ssh://user@host` with Termurion running opens a new tab running ssh to that host in the running Termurion (the process Run started exits; there is no second Termurion window or process left in Task Manager), the window comes forward, and a banner "Opened an ssh link to user@host." is shown. | |
| 5 | Win+R `ssh://host" --x "y` opens nothing (the extra arguments are refused, R14de-9); the log (debug output) has "url: refused command line with N arguments" and does not contain the URL. | |
| 6 | With Termurion closed, Win+R `ssh://user@host` starts Termurion and opens the tab after the first window (no extra empty window first). | |
| 7 | `ssh://user@host/tmp` lands in `/tmp` on the remote in both modes of "Use SSH integration for ssh: URLs": the path always travels as the command word `cd /tmp; exec $SHELL -l` (with integration the conductor runs it with the command words, as iTerm2 does; no conductor `cd` command is sent). No "Start directory ... does not exist" banner appears for the URL's path in either mode. `ssh://user@host/a%20b` (a space) lands in the remote home with no `cd` word sent, in both modes (ruling 14e-R10). | |
| 8 | `ssh://user@host/~/w` (with a `w` folder in the remote home): with integration ON the session lands in `~/w` (the conductor runs `cd ~/w`); with integration OFF ssh's remote shell runs iTerm2's escaped `cd \~/w`, which fails, and the session starts in the home directory. `ssh://user@host/no/such/dir` lands in the home directory in both modes. Whenever a `cd` fails the shell still starts (`;` then `exec $SHELL -l`), and with integration on the session stays integrated (no Abort banner, no "plain shell" fallback). Security (14e-R10): with a file `/tmp/pwned` absent on the remote, `ssh://user@host/x;touch%20/tmp/pwned`, `ssh://user@host/$(touch%20/tmp/pwned)` and `ssh://user@host/%60touch%20/tmp/pwned%60` each land in the remote home in both modes and `/tmp/pwned` still does not exist afterwards. | |
| 9 | With Settings > General > Experimental > "Use SSH integration for ssh: URLs" ticked, a new link shows SSH integration in the session (14a to 14c's banner and framing); with it unticked the tab runs plain ssh through a Custom command. | |
| 10 | `ssh://bad%20user@host` shows an "Illegal Username" alert with iTerm2's text ("The SSH user name ... contained a disallowed character. The set of allowed characters is limited for security reasons. You can modify it in Settings > Advanced > Valid characters in SSH user names.") and opens nothing. Adding a space to Settings > Advanced > Valid characters in SSH user names does NOT allow it (a deviation from iTerm2: Termurion's 14c grammar also applies, ruling P2, so the setting can only narrow); removing `+` from the setting makes `a+b@host` illegal. | |
| 11 | `ssh://a+b@host`, `ssh://jos%C3%A9@host` and `ssh://a%0Ab@host` show "Illegal Username" and open nothing (the iTerm2 check and 14c's grammar both apply, P2). | |
| 12 | `ssh://-oProxyCommand=calc@x`, `ssh://-x` and `ssh://%2Doproxy/` open nothing and start no program (no calculator, no ssh process). | |
| 13 | `ssh://[::1]/` opens nothing (IPv6 links are not supported, as iTerm2); `ssh://h%20x/` and `ssh://u@h:99999/` open nothing. | |
| 14 | Host-less links `ssh:h`, `ssh:///a`, `ssh://u@/a` and `ssh:/etc;x` open nothing in BOTH modes (integration on and off) and no bare `ssh` tab appears (ruling P13(d)). | |
| 15 | Hostile paths: `ssh://host/a%00b`, `ssh://host/%1b[31m`, `ssh://host/a%22b` and `ssh://host/a%5Cb` still open a tab to `host` but the remote command has no `cd` part (the cd part is dropped, `-t` stays) and no escape sequence is injected. | |
| 16 | `ssh://host//attacker.example/share` and `ssh://host/%2F%2Fattacker.example/share` and `ssh://host/%5C%5Cattacker.example/share`: a tab opens to `host` with no cd part; Termurion never touches `\\attacker.example` (watch it with Process Monitor or a Wireshark filter on SMB: no connection attempt, no NTLM). (Security row, the Task 3 Critical.) | |
| 17 | A remote path in a URL is never probed on this PC: `ssh://host/C:/Windows` and `ssh://host/srv/app` produce no "Start directory ... does not exist" banner and no file system access to `C:\Windows` or `\srv\app`. | |
| 18 | Win-click on `ssh://host` printed in a local pane (a link) opens a tab when a profile is bound to `ssh` (and the "URL schemes:" tick shows), and is refused with Termurion's message ("Termurion opens only http, https, mailto, and ftp links, not ssh: links.") when none is bound. | |
| 19 | In a pane that is itself an ssh session with SSH integration (a framing conductor), Win-click on a printed `ssh://host` link does NOT open a local tab; the click goes the 14d route (secure copy only). OSC 8 and OSC 1337 OpenURL with an `ssh:` URL sent by the remote (`printf '\e]8;;ssh://evil@h\e\\x\e]8;;\e\\'`) open nothing locally. | |
| 20 | Unticking `ssh` leaves Termurion registered (iTerm2 leaves the handler): a browser `ssh:` link still opens a tab with the default profile (iTerm2's getUrl), a Win-click on an `ssh:` link in a pane is refused again, and the tick shows off. | |
| 21 | Tick again while Termurion is already the handler: no question, the tick shows (iTerm2 asks only when it is not the handler). | |
| 22 | Deleting the bound profile in the Profiles window removes its binding (no stale tick on another profile); importing from iTerm2 prunes bindings that match no profile. | |
| 23 | A link in a browser (Chrome or Edge: a page with `<a href="ssh://user@host">`) asks the browser's own external-protocol question once, then opens a tab in the running Termurion. | |
| 24 | A second Termurion is never started when one runs: Win+R `ssh://user@host` five times in a row opens four tabs (at most 4 external links open per 10 s) and one banner "N ssh links ignored" for the rest; waiting 10 s and sending another opens it again with the count reset. | |
| 25 | With one Termurion elevated (run as administrator) and one normal, each opens links only in its own instance: a link started from a normal Run never reaches the elevated Termurion (separate `-admin` pipe) and starts a normal one. | |
| 26 | A client that connects to `\\.\pipe\tandem-url-<SID>` and sends nothing is dropped after about 2 s and does not block the next link (test with a one-line PowerShell that opens the pipe and sleeps, then Win+R a real link within 3 s: it opens). | |
| 27 | Keyboard mode restore: in an ssh session (any host) switch Session > Terminal State to "CSI u Mode", then exit ssh (the prompt's RemoteHost returns to this PC): the key mode is back to what it was before ssh (Standard) with "Automatically restore keyboard mode when an ssh session ends?" on (Settings > Advanced > Experimental Features, default on). Check with Session > Terminal State (the mode checked). | |
| 28 | The same with the setting off: the mode is left alone after exit. | |
| 29 | A remote that sets a key mode in the SAME output chunk as its host report (e.g. `printf '\e]1337;RemoteHost=u@box\a\e[>4;2m'`) cannot choose what is restored: after returning to this PC the mode is the one in effect before that report, not Mok2 (ruling R7, reversed same-chunk ruling). | |
| 30 | `su` on this PC then `exit` changes nothing about the key mode (two local users are both local hosts, iTerm2's `bothLocalhost` skip, ruling P14); no offer bar appears. | |
| 31 | When the title changed during the ssh session, the offer bar "Automatically restore the tab and window title when an ssh session ends?" appears on return with "_Only This Time", "Always" and "Never"; Only This Time restores the title once; Always restores now and on every later session (and resolves other panes' pending offers); Never leaves the title and likewise stops future offers. With Allow Title Setting off in the profile nothing is offered or restored. | |
| 32 | Nested hops: ssh from host A to host B then exit back to A restores A's saved mode, and exit back to this PC restores the local one (the stack pops to the entry for that host); a hostile remote printing many different RemoteHost reports does not slow Termurion or lose the local entry (stack capped at 32, the first local entry kept). | |
| 33 | Shell > ssh > Remote host can control iTerm2 is shown unavailable (disabled), with no "milestone" text, and its chord (none) does nothing; the menu bar and the right-click menu agree (rulings 3 and P7). | |
| 34 | Kept iTerm2 quirks a person can see: kitty key flags are not restored after ssh exits (set CSI u with flags then exit: the mode returns, the flags do not); `ssh://h/~/w` runs `cd \~/w` (above); a path with a quote or backslash drops the cd part but `-t` stays (the remote gets a tty either way); the ignored-link count resets only after a quiet 10 s. | |
| 35 | Kept Termurion limits a person can see: a malformed link opens nothing and says nothing; a hotkey-window profile bound to `ssh` opens a tab, not the hotkey window; `$$HOST$$` in a bound profile's command stays literal; a window-style click opens a tab. | |
| 36 | Registration reversibility: closing Termurion and deleting `HKCU\Software\Classes\ssh` by hand makes the tick show off again on the next launch (Termurion is not the handler); the question then names "There is currently no handler." and registering again works. | |
| 37 | Security: no ssh: URL from any source (link, Run, pipe, remote OSC, Smart Selection) ever starts `cmd.exe`, a program from the URL's text, or a second Termurion when one runs; look in Task Manager's details after sending each hostile link above. | |
