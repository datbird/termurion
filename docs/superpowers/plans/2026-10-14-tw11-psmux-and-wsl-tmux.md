# TW11: psmux and WSL tmux, tested and supported

Robert (2026-10-14): recommend psmux for native (non-WSL) tmux on Windows; every tmux feature
tested against native Windows sessions is tested against psmux, and the docs say so; WSL tmux
gets its own support too.

## Facts (probe of 2026-10-14, psmux 3.3.8 from winget `marlocarlo.psmux`, WSL tmux 3.4)

- `psmux -L <name> -CC new-session -s <name>` through the shipped ConPTY delivers the control
  mode opener (`ESC P 1000 p`), `%begin`/`%end`, `%window-add`, `%sessions-changed`,
  `%session-changed`, `%output`, a `display-message -p` reply and a `list-windows -F
  "#{window_layout}"` reply (`a8ff,120x30,0,0,2`).
- Difference 1: `kill-session -t <attached session>` answers `%begin`/`%end` but no `%exit`
  follows within 20 s (tmux sends `%exit`). The gateway must not wait forever for it.
- Difference 2: the first layout is 120x30 while the ConPTY is 80x24 (the size before
  `refresh-client -C`); check that Termurion's size commands take effect.
- psmux is the user's daily multiplexer on the test machine: a probe must use its own `-L`
  server name and end with `kill-session` of its own session. Never `kill-server` without `-L`.

## Work

1. Recorder: turn `src/tmux_probe.rs` into recorders for both flavors (ignored tests):
   `psmux-3.3.8-start.bin`, `psmux-3.3.8-layout.bin` beside the WSL ones.
2. Fixture tests: every test that reads a WSL fixture also runs against the psmux one.
3. Command audit: run every command Termurion's controller sends (attach, size, options,
   monitors, pause, clipboard, edits, dashboard, persist) against psmux and WSL tmux 3.4 in a
   live ignored test; record which answer `%error`. Gate the unsupported ones by flavor
   (detect psmux from its version or a format it answers) so the session never shows errors.
4. Session end without `%exit` (difference 1): end the gateway when the attached session is
   gone (its own kill, `%sessions-changed` plus a `list-sessions` check, or the client
   process ending).
5. Live suite (ignored, headless, no window): through ConPTY plus the real gateway and
   controller code, against both psmux and WSL tmux: attach, new window, split -h/-v, resize,
   select-layout, kill-pane, rename-window, send-keys round trip, pause (if supported), detach.
   Results table: `docs/probe/2026-10-14-psmux-wsl-tmux-results.md`.
6. Starting a session: Shell > tmux > "New psmux Session" (runs `psmux -CC new-session -A -s
   termurion` in a new tab) and "New tmux Session in WSL" (`wsl.exe -e tmux -CC new-session -A
   -s termurion`). A missing program shows an info bar with the install command
   (`winget install psmux`, or `sudo apt install tmux` in WSL), never a dead tab.
7. Docs: README (tmux section recommends psmux for native tmux, WSL tmux and ssh as the other
   ways; "native Windows tmux is tested against psmux"), docs/help/tmux-integration.md, the
   parity notes, FIXES.md entry.
