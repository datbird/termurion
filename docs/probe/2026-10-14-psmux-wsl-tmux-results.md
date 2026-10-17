# TW11 live suite: psmux 3.3.8, WSL tmux 3.4, and tmux over ssh, 2026-10-14

Machine: the Windows test machine, shipped ConPTY (`vendor/conpty`), psmux 3.3.8 from winget (`marlocarlo.psmux`), tmux 3.4 in WSL.

Every test drives a real server through a headless ConPTY with the same hook, gateway, handshake, and controller the app uses. No window opens. Each test makes its own server (`psmux -L termurion-test-...`, `tmux -L termurion-test-...`) and ends only that server; the default servers are never used.

Run: `scripts/xt test --release --lib tmux_live -- --ignored --nocapture --test-threads=1`. The tests print one `LIVE` line per fact; this file is made from them. The recorders for the fixtures are `tmux_probe::record_psmux_transcripts` and `record_psmux_layout`.

## Live suite

| Feature | psmux 3.3.8 | WSL tmux 3.4 | Notes |
|---|---|---|---|
| attach | pass | pass | Handshake, attach list, captures and monitors all answered; no abort on either. psmux reports `#{version}` as 3.3.8, which reads as tmux 3.3. |
| size to the tab | pass | pass | psmux opens at 120x30 whatever the ConPTY is (80x24); `resize-window -x 100 -y 28` and `refresh-client -C` bring it to the tab size. tmux takes it from `refresh-client -C @W:WxH` or `resize-window`. |
| new window | pass | pass | `new-window -PF "#{window_id}"` through the controller; the window opens with its panes. |
| split -h | pass | pass | `split-window -h -t %N`; the layout notice adds the pane. |
| split -v | pass | pass | `split-window -v -t %N`. |
| resize pane | pass | pass | `resize-pane -R -t %N 4` through the controller; the relist reads the new layout. |
| select-layout | pass | pass | `select-layout even-vertical`; Termurion reads the layout notice. |
| resize to a new tab size | pass | pass | `tabs_resized` after the tab changed size (90x26). |
| rename-window | pass | pass | `rename-window -t "$S:@W"`; `%window-renamed` brings it back. |
| send-keys round trip | pass | pass | Termurion's own `send` commands (literal, hex, raw) and the echo comes back as `%output`. psmux answers every `send -l...` and `send -t %N 0x..` with `%error command timed out` although the keys arrive; the errors are tolerated. |
| pause and unpause | pass | pass | `refresh-client -fpause-after=120`, `-A %N:pause`, `-A %N:continue`; both servers pause and unpause. |
| kill-pane | pass | pass | psmux says `%window-close` for the window when one of two panes dies; the controller checks `list-windows` and reads the layout instead of closing the tab. tmux sends a layout change. |
| detach | pass | pass | tmux: `detach`, `%exit`, the client ends. psmux: its client does not leave on `detach` until it reads more input, so Termurion drops the client (the server keeps the session). |
| session survives the detach | pass | pass | `list-sessions` on the test's own server after the detach. |
| kill-session of the attached session | pass | pass | tmux: `%exit`. psmux: a reply and silence; the gateway ends itself and the client is ended. |

Not run: the client list that tmux 3.6 and later answers, and the `refresh-client -r` color reports. tmux 3.4 has no `-r`: the controller sends it from 3.4 on and the error is tolerated. psmux reports 3.3, so it is not sent.

## tmux over ssh live suite, 2026-10-17

The ssh tests in `src/tmux_live.rs` (`suite_ssh`, `session_end_ssh`, `clipboard_ssh`, `locality_ssh`) drive tmux on another machine through `ssh.exe -tt` inside the same headless ConPTY, with the same hook, gateway, handshake, and controller. The host comes from `TERMURION_TMUX_SSH_HOST` (optional `TERMURION_TMUX_SSH_USER` and `TERMURION_TMUX_SSH_PORT`); with it unset each test prints `LIVE ssh SKIP` and passes. Each test starts `tmux -L termurion-test-<name><random> -f /dev/null -CC new-session` on the host and ends only that server (`kill-server`), also when the test panics. A failed fact now fails the test (`result` asserts).

Run: `TERMURION_TMUX_SSH_HOST=<host> XT_PASS_ENV=TERMURION_TMUX_SSH_HOST scripts/xt test --release --lib -- --ignored --nocapture --test-threads=1 tmux_live::tests::suite_ssh` (and the other three).

| Feature | tmux 3.4 over ssh (Linux) | tmux 3.6 over ssh (Linux) | Notes |
|---|---|---|---|
| attach | pass | pass | Version read as 3.4 and 3.6; no abort. |
| size to the tab | pass | pass | `100x28`. |
| new window, split -h, split -v | pass | pass | |
| resize pane, select-layout | pass | pass | |
| resize to a new tab size | pass | pass | `90x26`. |
| rename-window | pass | pass | |
| send-keys round trip | pass | pass | The remote shell echoes the marker back as `%output`. |
| pause and unpause | pass | pass | tmux 3.2 and later. |
| kill-pane | pass | pass | |
| detach | pass | pass | `detach`, `%exit`, the ssh client ends with exit 0 (80 to 680 ms). |
| session survives the detach | pass | pass | `list-sessions` through a second ssh call. |
| kill-server cleanup | pass | pass | `no server running` afterwards. |
| kill-session of the attached session | pass | pass | `%exit`, the client ends. |
| clipboard | pass | pass | `set-buffer` brings `%paste-buffer-changed buffer0`; the controller's `show-buffer` brings the text. Also passes on WSL tmux 3.4. |
| server is not local | pass | pass | `#{pid}` is a remote pid; no pane pid is asked for. |

Totals of the full run on the 3.4 host: 20 facts on psmux, 19 on WSL tmux, 19 on ssh, 58 pass and 0 fail. The 3.6 host ran the four ssh tests: 4 tests, 0 fail.

Notes on the harness: Windows' `ssh.exe` does not leave when its output goes to a pipe even though the remote command is done, so the helper that calls `tmux` on the host (`list-sessions`, `kill-server`) writes to a file and caps the call at 30 seconds.

## Local and remote servers (psmux, WSL, ssh)

`locality_psmux`, `locality_wsl` and `locality_ssh`. iTerm2 calls a server local when the process behind `#{pid}` is named tmux; a local server is asked for each pane's `#{pane_pid}` and the job comes from that process tree.

| Fact | psmux 3.3.8 | WSL tmux 3.4 | tmux over ssh |
|---|---|---|---|
| `#{pid}` names a Windows process here | `psmux.exe` | none, or a stranger's | none |
| server is local | yes | no | no |
| `#{pane_pid}` asked for | yes | never | never |
| pane pid is a Windows process | `powershell.exe` | not asked | not asked |
| a program started in the pane shows below that pid (job from the process tree) | pass (`ping`) | not applicable | not applicable |

Clipboard on psmux: not available. `set-buffer` is refused in control mode (an error with no text) and psmux sends no `%paste-buffer-changed`, so TmuxSyncClipboard has nothing to mirror. The test records this as a `NOTE` line and does not assert it.

## What psmux does differently

| Difference | How Termurion handles it |
|---|---|
| `phony-command` succeeds | Every tmux refuses it as an unknown command. The handshake takes success as psmux (`Handshake::psmux`, `Gateway::psmux`). |
| Lines end in a bare newline | tmux ends them in CR LF. The hook accepts both; `tests/fixtures/tmux/psmux-3.3.8-start.bin` and `-layout.bin` hold the recordings. |
| `%session-renamed name` has no id | tmux sends `$id name`. `tmux_proto::parse_psmux` reads the id-less form as the attached session; real tmux keeps the strict parser. |
| `kill-session` or `kill-server` of the attached session sends no `%exit` | The gateway ends itself (`Event::Gone`) when psmux answers such a command for the attached session, and the app ends the psmux client. |
| `detach` leaves its client running until more input arrives | The reply is a `%begin` line with `%exit` glued to its end and no newline, and only after the next write. Termurion sends nothing for a psmux detach: the gateway lets go and the client is ended. The hook also splits a `%begin ...%exit` line, whole or partial. |
| A client that ends without `%exit` left a stale gateway | `session_exited` now ends the gateway of a client that exited (psmux killed, or a crash). Real tmux ends it at `%exit` first, as before. |
| `%window-close` when one of two panes dies | The window stays. For psmux the controller asks `list-windows` and reads the layout when the window is still there. |
| `send` replies `%error command timed out` | The keys arrive. Key sends are already tolerated. |
| `show-buffer` of a missing buffer replies `command timed out` | tolerated for psmux (tmux: `no buffer`). |
| Pane options named `@...` are refused | `set -p -t %N @uservars` replies ok with the text `ERROR: pane-scoped option ... is not supported`; the controller sends no user variables to psmux. |
| No `move-pane`, `join-pane`, `swap-window`, `attach-session`, `new-session`, `switch-client` in control mode | Move Session to Split Pane shows a banner and sends nothing; tab reorder sends nothing; the Dashboard disables + and Attach. |
| `new-window -a -t "$S:+"` is refused | The Dashboard sends a plain `new-window` to psmux. |
| One server per session | In the probe, `list-sessions` in a control client listed the attached session only; the Dashboard shows one. |
| The first layout is 120x30 | Termurion's size commands (`refresh-client -C`, `resize-window`) take effect: the layout is the tab size (live suite, "size to the tab"). |
| `#{version}` is `3.3.8` | Two dots; Termurion reads it as 3.3, which turns on pause mode and per-window sizes, both of which psmux answers. |

## Command audit

Every command the controller sends (attach, size, options, monitors, pause, clipboard, edits, Dashboard, persistence), run against both servers by `audit_psmux` and `audit_wsl`, plus the raw forms from `tmux_edits`, `tmux_keys`, and `tmux_dashboard`. Ids are shown as `N`. "not sent" means that test did not send it (for psmux, because the command is gated).

10 of 96 commands differ.

| Command | psmux 3.3.8 | WSL tmux 3.4 | Handled |
|---|---|---|---|
| `phony-command` | ok | ERROR: parse error: unknown command: phony-command | psmux accepts it: this is how Termurion tells psmux from tmux |
| `resize-window -x 100 -y 28 -t @N` | ok | not sent |  |
| `move-pane -s "%N" -t "%N" -h` | ERROR: unknown command: move-pane | ok | left out for psmux (a banner says so) |
| `swap-window -s @N -t @N` | ERROR: unknown command: swap-window | ok | left out for psmux (tabs keep their order there) |
| `attach-session -t "$N"` | ERROR: unknown command: attach-session | ok | Dashboard Attach is disabled for psmux |
| `show-buffer -b buffer0` | ERROR: command timed out | ERROR: no buffer buffer0 | tolerated for psmux |
| `new-session -s "extra"` | ERROR: unknown command: new-session | ok | Dashboard + is disabled for psmux |
| `refresh-client -r "%N:\033]10;rgb:c8c8/c8c8/c8c8\033\\"` | not sent | ERROR: parse error: command refresh-client: unknown flag -r | tmux 3.4 has no -r; tolerated (psmux is below 3.4: not sent) |
| `refresh-client -r "%N:\033]11;rgb:0a0a/0a0a/0a0a\033\\"` | not sent | ERROR: parse error: command refresh-client: unknown flag -r | tmux 3.4 has no -r; tolerated (psmux is below 3.4: not sent) |
| `send ...` (literal, hex, `-H`, key names) | `-lt`, `0x..` and `-H` forms: ERROR: command timed out (the keys arrive); `send -t %N C-Left`: ok | ok | tolerated, as for every key command |

<details><summary>The other 86 commands answer ok on both</summary>

| Command | psmux 3.3.8 | WSL tmux 3.4 | Handled |
|---|---|---|---|
| `refresh-client -fpause-after=0,wait-exit` | ok | ok |  |
| `show-window-options -g aggressive-resize` | ok | ok |  |
| `show-option -g -v status` | ok | ok |  |
| `list-sessions -F " "` | ok | ok |  |
| `show-options -v -s default-terminal` | ok | ok |  |
| `list-keys` | ok | ok |  |
| `copy-mode -q` | ok | ok |  |
| `display-message -p "#{version}"` | ok | ok |  |
| `show-window-options pane-border-format` | ok | ok |  |
| `list-windows -F "#{socket_path}"` | ok | ok |  |
| `list-windows -F "#{pid}"` | ok | ok |  |
| `show-options -g message-style` | ok | ok |  |
| `display-message -p "#{pid}"` | ok | ok |  |
| `show-options -v -g set-titles` | ok | ok |  |
| `refresh-client -fpause-after=120` | ok | ok |  |
| `show -v -q -t $N @iterm2_size` | ok | ok |  |
| `show -v -q -t $N @iterm2_id` | ok | ok |  |
| `refresh-client -C 100,28` | ok | ok |  |
| `show -v -q -t $N @hidden` | ok | ok |  |
| `show -v -q -t $N @buried_indexes` | ok | ok |  |
| `show -v -q -t $N @affinities` | ok | ok |  |
| `show -v -q -t $N @per_window_settings` | ok | ok |  |
| `show -v -q -t $N @per_tab_settings` | ok | ok |  |
| `show -v -q -t $N @origins` | ok | ok |  |
| `show -v -q -t $N @hotkeys` | ok | ok |  |
| `show -v -q -t $N @tab_colors` | ok | ok |  |
| `list-sessions -F "#{session_id} #{session_name}"` | ok | ok |  |
| `list-windows -F "#{session_name} #{window_id} #{window_name} #{window_width} #{window_height} #{w...` | ok | ok |  |
| `set -t $N @iterm2_id "GUID"` | ok | ok |  |
| `capture-pane -peqJN -t "%N" -S -1000` | ok | ok |  |
| `capture-pane -peqJN -a -t "%N" -S -1000` | ok | ok |  |
| `list-panes -t "%N" -F "pane_id=#{pane_id} alternate_on=#{alternate_on} alternate_saved_x=#{altern...` | ok | ok |  |
| `capture-pane -p -P -C -t "%N"` | ok | ok |  |
| `refresh-client -A '%N:continue'` | ok | ok |  |
| `show-options -v -q -p -t %N @uservars` | ok | ok |  |
| `refresh-client -B 'it2_N:%N:#{pane_title}'` | ok | ok |  |
| `display-message -t '%N' -p '#{pane_title}'` | ok | ok |  |
| `refresh-client -B 'it2_N:%N:#{pane_current_command}'` | ok | ok |  |
| `display-message -t '%N' -p '#{pane_current_command}'` | ok | ok |  |
| `refresh-client -B 'it2_N:%N:#{pane_current_path}'` | ok | ok |  |
| `display-message -t '%N' -p '#{pane_current_path}'` | ok | ok |  |
| `display-message -p "#{status-interval}"` | ok | ok |  |
| `refresh-client -B 'it2_N:@N:#{T:status-left}'` | ok | ok |  |
| `display-message -t '@N' -p '#{T:status-left}'` | ok | ok |  |
| `refresh-client -B 'it2_N:@N:#{T:status-right}'` | ok | ok |  |
| `display-message -t '@N' -p '#{T:status-right}'` | ok | ok |  |
| `set -p -t %N @uservars "u_HEX"` | ok | ok | not sent to psmux |
| `split-window -h -t "%N"` | ok | ok |  |
| `split-window -v -t "%N" -c '#{pane_current_path}'` | ok | ok |  |
| `resize-pane -R -t "%N" 3` | ok | ok |  |
| `list-windows -F "#{window_id} #{window_layout} #{window_flags} #{window_visible_layout} #{pane-bo...` | ok | ok |  |
| `resize-pane -Z -t "%N"` | ok | ok |  |
| `swap-pane -s "%N" -t "%N"` | ok | ok |  |
| `new-window -PF '#{window_id}'` | ok | ok |  |
| `new-window -PF '#{window_id}' -c '#{pane_current_path}'` | ok | ok |  |
| `display -p -F "#{session_name} #{window_id} #{window_name} #{window_width} #{window_height} #{win...` | ok | ok |  |
| `rename-window -t "$N:@N" "renamed"` | ok | ok |  |
| `rename-window -t @N "renamed2"` | ok | ok |  |
| `select-pane -t %N -T "title"` | ok | ok |  |
| `select-pane -t "%N"` | ok | ok |  |
| `select-layout -t @N even-horizontal` | ok | ok |  |
| `list-panes -t %N -F '#{pane_id}'` | ok | ok |  |
| `set -t $N @iterm2_size "HEX"` | ok | ok |  |
| `set -t $N @hidden ""` | ok | ok |  |
| `set -t $N @buried_indexes "b_HEX"` | ok | ok |  |
| `set -t $N @affinities "31"` | ok | ok |  |
| `set -t $N @per_window_settings "w_HEX"` | ok | ok |  |
| `set -t $N @per_tab_settings "T_HEX"` | ok | ok |  |
| `set -t $N @origins "o_HEX"` | ok | ok |  |
| `set -t $N @hotkeys "h_HEX"` | ok | ok |  |
| `set -t $N @tab_colors "t_HEX"` | ok | ok |  |
| `set -p -t %N @uservars "abc"` | ok | ok | not sent to psmux |
| `set -t $N @hidden "i_HEX"` | ok | ok |  |
| `refresh-client -B 'it2_N'` | ok | ok |  |
| `list-windows -F "#{window_id}"` | ok | ok |  |
| `list-panes -s -t $N -F "#{pane_id}"` | ok | ok |  |
| `set -t $N @affinities "a_HEX"` | ok | ok |  |
| `rename-session -t "$N" "renamed-session"` | ok | ok |  |
| `list-clients -F "#{client_name}"` | ok | ok |  |
| `display-message -p '#{client_name}'` | ok | ok |  |
| `refresh-client -A '%N:pause'` | ok | ok |  |
| `refresh-client -C @N:100x28` | ok | ok |  |
| `resize-window -t @N -x 100 -y 28` | ok | ok |  |
| `refresh-client -B 'it2_x:%N:#{pane_title}'` | ok | ok |  |
| `refresh-client -B 'it2_x'` | ok | ok |  |
| `break-pane -P -F "#{window_id}" -s "%N"` | ok | ok |  |

</details>
