# Fixes and tweaks (beta)

Things found while using Termurion after 0.0.1-beta1. Add new items at the end of Open. Each
item says what is wrong, what it should do, and where it was seen. Move an item to Done with its
commit. The list from before the beta is in [`TWEAKS.md`](TWEAKS.md).

## Open

(none)

## In progress

(none)

## Done

2. **The session host outlives the app and blocks installs.** Seen 2026-10-14: no Termurion window
   or tray icon, but `termurion-host.exe` (started 22:18 the day before) still held one PowerShell
   session, so `scripts/install-windows.ps1` refused ("Still running: termurion-host"). host.log
   shows the app logged `tray: remove ok` with no `quit:` line, so the app went away without the
   quit path that ends host jobs. Find why the app exited that way (crash, or a close path that
   skips quit), and decide what a host with no app should do: keep the sessions for the next
   launch to reattach (crash recovery, as designed), but show it somewhere. The setup.exe
   installer is not blocked (Restart Manager closes the host).
   Done 2026-10-14. Cause: termurion.exe crashed (access violation in Windows' icu.dll,
   collation code, reached from a shell component; no dump existed), so the quit path never ran.
   Now: both exes write a minidump to %LOCALAPPDATA%\Termurion\crashes and log a `crash:` line
   (`crashdump.rs`); the app says goodbye on a clean quit (protocol 5), and a host whose app
   vanished reopens termurion.exe after 3 s (at most once per 5 minutes) and ends the sessions
   after 1 h with no app (`host_watch.rs`); ended sessions alone keep the host at most 1 h.


1. **Session > Move Session > Move Session to Window does nothing.** Seen 2026-10-14 with two
   windows open. Cause: the item moves the focused session into a NEW window, as iTerm2's does
   (`PseudoTerminal.m` `moveSessionToWindow:`). When the window holds only one session there is
   nothing to split off, so Termurion returns silently. iTerm2 greys the item out in that case
   (`validateMenuItem`: enabled only when the window has more than one session, and never for a
   locked session). Termurion leaves it enabled. Done 2026-10-14: both items now grey out by iTerm2's rule, per window
   (`menu::move_session_enabled`, test `move_session_items_follow_iterm2`).

3. **tmux on Windows: psmux for native tmux, and WSL tmux, tested live.** Done 2026-10-14: native
   Windows tmux support is now tested against psmux 3.3.8 and WSL support against tmux 3.4, with a
   live suite that drives real servers (`docs/probe/2026-10-14-psmux-wsl-tmux-results.md`).
   Termurion detects psmux and gates what it lacks (Move Session to Split Pane, tab reorder,
   Dashboard attach and new session, user variables), and ends the gateway when the session ends.
   Shell > tmux has New psmux Session and New tmux Session in WSL. Commits: f8731c6e (psmux
   transcripts), c5e566d8 (fixture tests), 4a57f672 (gating and gateway end), 4088828c (live suite
   and results), 4f978e69 (menu items). Docs: README, `help/tmux-integration.md`.

4. **The close question opened behind its window.** Seen 2026-10-14 (0.0.1-beta2): closing a
   window with running sessions showed "Close Window #1?" behind the Termurion window. The box
   disables its window, so every click on the window only played the Windows error sound, and
   Alt+Tab did not bring the box up. After about a minute of moving the window the box showed.
   Cause (proven): the box runs its own message loop inside a winit event handler, and winit
   0.30.13 answers each WM_PAINT in that state by asking for another (`RDW_INTERNALPAINT`,
   `event_loop.rs` WM_PAINT arm). Termurion always has a redraw pending, so the window paints
   in a loop for as long as the box is open, the thread spins, and the box can wait a long time
   for its first paint. Same loop as winit issue 4708 (window drags); Slint discussion 4102 saw
   the same hidden, dinging dialog after its winit 0.29 update. Measured on the desktop (test
   `winit_paint_loop_and_the_guard`): 860 ms of CPU per second with a box open, 0 ms with the
   guard. Done 2026-10-14: every message box goes through one door (`app/msgbox.rs`, test
   `no_message_box_bypasses_the_door`). While a box is open, a paint guard makes this thread's
   windows answer WM_PAINT with the default handler (no new request); closing the box removes
   it and repaints. Each box also asks to come to the front, and a watcher logs `dialog:` lines
   to host.log and shows and raises a box that is hidden or behind its window. host.log also
   names the renderer (`renderer:`).
