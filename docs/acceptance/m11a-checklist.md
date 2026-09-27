# Milestone 11a acceptance checklist

Build a release binary (`scripts/xt build --release`). Close any open Tandem window, copy
`tandem.exe`, `tandem-host.exe`, `tandem-git.exe`, `conpty.dll`, and `OpenConsole.exe` from
`C:\Users\datbi\src\tandem\target\release\` together, then run `tandem.exe`. Restart
`tandem-host.exe` too (or let Tandem relaunch it) so it serves new sessions at protocol version 3;
an older host still runs but its new sessions lack the session environment until it restarts.

Setup: install Python 3 and run `py -m pip install iterm2` in a plain venv (if the stock-package
test fails with an `extra_headers` TypeError, pin `websockets<14` in that venv, a known clash with
recent `websockets` releases). Open a two-window layout: one window with a plain tab and a split,
a second window with a tmux tab (tmux integration mode) and one buried session. Have a second
Windows account (or `runas /user:`) available to test the peer check, and, if possible, one
elevated and one normal `cmd.exe` window.

| # | Check | Result |
|---|---|---|
| 1 | With "Enable Python API" off, `Test-NetConnection 127.0.0.1 -Port 1912` fails: nothing listens. | |
| 2 | Turn on Settings > General > Python API > "Enable Python API": the forced "Enable Python API?" warning appears; Cancel leaves the checkbox off. | |
| 3 | OK on that warning turns the checkbox on and starts the server: `netstat -ano | findstr 1912` shows only `127.0.0.1` and `[::1]` listening, never `0.0.0.0` or `::`. | |
| 4 | Import a Settings file (Settings > General > Import from iTerm2, or a Tandem settings file) with the Python API already enabled: the same "Enable Python API?" warning appears before the server starts. | |
| 5 | Bind the Toggle Setting key action to "Enable Python API" and press it: the same warning appears; the checkbox and the server only follow the choice made in the warning. | |
| 6 | After OK, `%APPDATA%\Tandem\python-api-consent` exists. Turn the API off: the file is deleted. Turn it on again: the warning appears again (consent is asked every time it is turned on, not remembered as an unattended setting). | |
| 7 | Settings > General > Python API > "Authentication method for Python API" shows "Require a cookie" or "Allow all apps to connect", mirroring the allow-all file's actual state (not a stored preference). | |
| 8 | Choose "Allow all apps to connect": a warning appears, then the file `%USERPROFILE%\Library\Application Support\iTerm2\disable-automation-auth` is created; its Properties > Security tab shows the current user as owner, and its size matches the expected magic length. | |
| 9 | With a symlink or junction placed at that same path by hand (`mklink` or `mklink /J`) instead of a plain file: the popup still reads "Require a cookie" and a script presenting no cookie is refused; the link is never treated as a valid allow-all file. | |
| 10 | From a shell outside any Tandem session (`ITERM2_COOKIE` unset) with "Require a cookie" selected, run a Python script that calls `iterm2.Connection.async_create()`: the "Allow Python API Usage?" prompt appears; close it with the window's X button: the connection is denied and the script fails. | |
| 11 | Re-run the script: OK on the prompt allows the connection; Cancel denies it; More Info opens the security page and also denies. | |
| 12 | Switch to "Allow all apps to connect": the same outside script now connects with no prompt at all. Switch back to "Require a cookie": the allow-all file is deleted from disk. | |
| 13 | With the API enabled and port 1912 already held (a second Tandem, or any other program listening on it), the alert names the port, the owning process, and its pid; the API stays off for whichever Tandem lost the race. | |
| 14 | In a Tandem session with `setCookie` on, run a Python script from that session's own shell: it connects with no prompt (`ITERM2_COOKIE` is set). Run it a second time from the same shell: it still connects (the cookie is reusable, not single-use). | |
| 15 | `echo %ITERM_SESSION_ID%` in that session shows `w<n>t<n>p<n>:<id>` matching the pane's actual window, tab and pane numbers. | |
| 16 | Check the session environment in a newly opened session, a split, a session restored from a saved arrangement, and a hosted (tandem-host) session: each carries `ITERM_SESSION_ID`, `TERM_SESSION_ID`, `ITERM_PROFILE`, and (with `setCookie` on) its own `ITERM2_COOKIE`. | |
| 17 | A connection attempt from another machine on the LAN (point a script at this machine's real IP instead of `localhost`) fails: the server accepts loopback connections only. | |
| 18 | A script run as a different Windows user (`runas /user:otheruser`) is refused by the peer check, whether or not it presents a cookie. | |
| 19 | With Tandem running elevated, a script from a normal (non-elevated) shell is refused; with Tandem running normally, a script from an elevated shell is refused. | |
| 20 | A script that begins a transaction (`StartTransactionRequest`) freezes the Tandem window (no redraw, no input) until it ends; killing the script (Ctrl+C, or closing its window) unfreezes Tandem at once. | |
| 21 | With the stock `iterm2` package (`python -m pip install iterm2`, no modification), a short script (`async_get_app`, then print each window/tab/session) connects over `ws://localhost:1912` and lists windows, tabs, splits, the tmux tab and the buried session matching Tandem's actual layout. | |
| 22 | The same script's five subscriptions (new session, terminate session, layout change, focus change, broadcast change) each fire once for the matching action: opening a tab or window, closing a pane, splitting a pane, switching the selected tab, and Tandem gaining or losing focus. | |
| 23 | Turn Broadcast Input on for two sessions, then off: the broadcast-change subscription fires both times, and `GetBroadcastDomains` reports the broadcasting sessions while it is on and nothing extra once it is off. | |
| 24 | Subscribing the same connection to the same notification type twice answers ALREADY_SUBSCRIBED on the second attempt; unsubscribing without a prior subscription answers NOT_SUBSCRIBED. | |
| 25 | With a script connected, turn "Enable Python API" off: the script's connection drops at once (its next call fails). | |

## Kept bugs and rulings a person can see

| # | Item | Result |
|---|---|---|
| 26 | Session cookies are reusable for the whole life of the pane, not single-use: a script can reconnect from the same shell as many times as it likes while the session lives (see check 14). | |
| 27 | A script started outside any Tandem session with no allow-all file crashes in the stock package with `FileNotFoundError` for `/usr/bin/osascript` (there is no Windows twin of AppleScript's cookie request); "Allow all apps to connect" or a session cookie is the only way in (see check 10). | |
| 28 | A missing `Upgrade`, `Connection`, or `Sec-WebSocket-Protocol` header passes rather than refuses, matching iTerm2's own nil comparison; a present but wrong value is still refused. | |
| 29 | Port 1912 already held by another process is fatal, with no fallback port (see check 13); that other process could also collect any cookie a script sends to whatever answers on 1912, a cost of iTerm2's TCP-based design carried onto Windows. | |
| 30 | A transaction that is never ended by its script (a runaway or crashed process that does not disconnect) hangs Tandem until the connection closes (see check 20). | |
| 31 | Some refusals (400, 401, 406) keep the socket open for up to 1 s after writing the answer, so the client reads the real answer instead of a connection reset. | |
| 32 | A request or frame over 16 MiB is refused and ends the connection (Tandem's own cap; iTerm2 has none). | |
| 33 | WSL2 in NAT mode cannot reach the loopback API (WSL2 in mirrored networking mode can). | |
