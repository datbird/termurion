# Fixes and tweaks (beta)

Things found while using Termurion after 0.0.1-beta1. Add new items at the end of Open. Each
item says what is wrong, what it should do, and where it was seen. Move an item to Done with its
commit. The list from before the beta is in [`TWEAKS.md`](TWEAKS.md).

## Open

(none)

## In progress

9. **After a resize, typed text lands mid-screen, not at the prompt.** Seen 2026-10-15 in
   PowerShell: a long `dir`, then a window resize, then typing; the echo appeared about 15 rows
   above the prompt. Cause: after a resize the grid pulled scrollback down and moved the prompt
   to the bottom row, while ConPTY keeps the top of its screen in place and adds blank rows under
   the cursor, so the shell drew at ConPTY's row. Fixed for a resize at a quiet prompt in
   3f57288f (alacritty_terminal patch 34: on a pty-backed screen, growth adds rows below the
   cursor and pulls nothing from history), with live ConPTY tests in `src/resize_live.rs` and a
   `pty: resize` line in host.log. Still open: a resize while output is printing can still leave
   the cursor on the wrong row.

## Done

10. **The menu bar flashed on a Minimal window during a resize.** Seen 2026-10-15. Cause: the
    app kept a note that the bar was hidden, and paths that attach a bar (a menu rebuild, a
    window made again for transparency) did not check it, so a bar stayed attached under the
    client area and showed each time a resize redrew the frame. A probe confirmed a menu on the
    live Minimal window. Done 2026-10-16 in 977bc21a: the app asks the window (`GetMenu`) each
    frame and on each resize, and a bar built for a hidden window is detached as it attaches.

8. **Settings drop-downs flashed, closed, or opened slowly.** Seen 2026-10-15 in Settings >
   General. Cause: the egui windows ignored egui's request for another frame, so a popup's
   sizing pass and fade-in were drawn only at the next input. Done 2026-10-15 in 12608761 (draw
   the frames egui asks for now) and 722822b5 (delayed repaint requests wake the loop on time).

7. **Copy Base64-Decoded did not show for a selected word.** Seen 2026-10-15. Cause: the port
   required base64 padding, which iTerm2's decoder does not. Done 2026-10-15 in 96aba6aa: APR's
   lenient decoder, as iTerm2 links it (strip line breaks, decode the leading run of alphabet
   characters, try the URL-safe alphabet only when the standard one gives nothing).

6. **Selecting text crashed Termurion.** Seen 2026-10-15 (access violation, call to address 0
   from word selection). Cause: with edition 2024 precise capture, the ICU word-break session
   closure captured only copies of `guard.it` and `guard.api`, so the guard dropped when
   `session()` returned: the break iterator was closed and its text freed before every lookup
   (since 516b981a, 2026-10-05). Done 2026-10-15 in 5cff44a9: the closure calls a method on the
   whole guard; test `the_icu_session_keeps_its_iterator_open_until_dropped` fails on the old
   code. A sweep of the other Drop guards in move closures found no other case.

5. **The right-click menu, and other UI, was short-copied from iTerm2.** Seen 2026-10-14: the
   terminal right-click menu lacks about a dozen of iTerm2's items (Restart, Bury, Terminal
   State, Annotate Selection, Save Selection as Snippet, Add Trigger, Send Selection, Send Email,
   Download with scp, Save, the New Window, New Tab and Select submenus), in a different order
   and with different labels. Cause: it was hand-copied from a short summary, and the parity
   inventory checks settings keys, not what each menu and window shows. Audits found the same in
   10 code-built menus, 11 Settings panes (plus labels that paraphrase iTerm2) and 21 windows and
   alerts (`superpowers/audits/2026-10-14-ui-*.md`). Fix: every menu, window, Settings control
   and label as iTerm2 has it, in iTerm2's exact wording, with guard tests. Plan:
   `superpowers/plans/2026-10-14-ui-parity-sweep.md`.
   Done 2026-10-15. Four packages, each held to iTerm2's source by guard tests: the terminal
   right-click menu and the timestamp margin menu (`ctx_menu.rs`, merge e6206d6a); the Downloads
   and Uploads menus, transfer alerts, per-trigger items and Current Tab Color (b6bdfbf5); the
   windows, bars and alerts, with about 40 texts guarded in `win_texts.rs` (18c94170); and the
   Settings window, 344 controls checked against the xibs by `ui_labels.rs` (b50ec2c2). Review
   fixes in 6eaf134f. Rulings and the choices that differ from iTerm2:
   `superpowers/rulings/ui-sweep-{a,b,s,w}-rulings.txt`.


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
