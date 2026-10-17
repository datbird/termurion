# Fixes and tweaks (beta)

Things found while using Termurion after 0.0.1-beta1. Add new items at the end of Open. Each
item says what is wrong, what it should do, and where it was seen. Move an item to Done with its
commit. The list from before the beta is in [`TWEAKS.md`](TWEAKS.md).

## Open

(none)

## In progress

(none)

## Done

14. **The renderer drew through Vulkan, on the Intel GPU, by accident.** Seen 2026-10-17: host.log
    said `renderer: Vulkan on Intel(R) UHD Graphics 770` on a computer with an RTX 4080 SUPER.
    Cause: the first window used wgpu's default instance, which lists Vulkan first, while the
    see-through windows already used DirectX 12; and with "Prefer integrated to discrete GPU" off
    the request was "no preference", which leaves the pick to the enumeration order. Done
    2026-10-17: every window uses DirectX 12 (`gpu_pick.rs`), falling back to Vulkan and then OpenGL
    only when DirectX 12 makes no adapter, surface or device; host.log names the API used, every
    adapter (once), why a fallback happened, and whether the GPU setting was honored. The GPU
    follows iTerm2's `preferIntegratedGPU` (default on, so the Intel GPU stays the default; the row
    is live only with both kinds of GPU): on asks for the low-power GPU, off asks for the
    high-performance one (it was "no preference"). A lost device is now logged
    (`renderer: device lost`); the surface-lost path still remakes the swapchain. Set
    `TERMURION_RENDERER` to `dx12`, `vulkan` or `gl` to try one first. Measured headless (test
    `each_api_honors_the_gpu_preference_and_draws`, 1200 x 800, 40 text rows, with readback):
    DirectX 12 3.38 ms on the Intel GPU and 3.79 ms on the RTX; Vulkan 3.64 ms and 3.93 ms. OpenGL
    cannot make the renderer's offscreen target (no view formats), so it is the last resort only.
    Help: `help/gpu-renderer.md`.

13. **No shortcut uses the Win key.** Ruled 2026-10-16: no shortcut can use the Win key, a Windows
    limitation (Windows keeps most Win chords, and they are lost over remote desktop, a KVM or a
    keyboard sharing tool). Item 12 left 34 chords (Cmd+Option+Shift, Cmd+Ctrl+Shift, four
    modifiers), the workgroup peers and a profile's new-window shortcut on the Win key. Done
    2026-10-16 on branch `nowin`:
    - Every default has a press without Win in both Shortcut Key modes. The chords with no free pair
      of Ctrl, Shift and Alt are moved by an explicit table (`EXTRAS` in `src/shortcut_key.rs`),
      the same keys in both modes. iTerm2's Cmd chord stays the stored identity (key mappings,
      import and export); the menus, Settings > Keys, tooltips and help show the pressed keys.
    - The Shortcut Key setting offers Ctrl+Shift and Ctrl+Alt. A saved Win (1) reads as Ctrl+Shift
      and is dropped when the settings load.
    - The keyboard hook no longer takes Win chords for Termurion (`Gate`, the Win-chord decision,
      the password manager, AI Chat and Cockpit registrations, the chord event to the app, Respect
      shortcuts from Windows keyboard shortcuts for Termurion's own chords). It stays for the
      modifier remap and the system-wide hotkeys. The password manager, AI Chat and Cockpit read
      their chords from their own key events (`WindowKeys`).
    - The Settings key recorder refuses a Win press and shows "Windows keeps the Win key for itself, so a shortcut cannot use it. Use Ctrl, Shift or Alt."
      (key bindings, Leader and hotkeys). The pointer binding editor's Win check box is named after
      the shortcut key.
    - A press resolves to the chords it can be: the table first, then the pair, then a profile's
      new-window shortcut; the first one something is bound to runs. Ctrl+Shift+Alt+key is also
      Cmd+Shift+key, so a key Cmd+Shift already uses keeps its command, and that profile opens in a
      tab. Free for a window: Q, R, X and 1 to 9.
    - Windows Terminal conventions used: Alt+Shift+- and Alt+Shift+= split a pane. Ctrl+Shift+PgUp and
      PgDn are not free (Cmd+PgUp and Cmd+PgDn scroll a page in the shipped key map), so Move Tab
      takes Alt+Shift+PgUp and PgDn. No letter or digit is used with Ctrl+Alt or Shift+Alt: a
      profile's shortcut (Cmd+Ctrl+key) uses all of them.
    - The hotkey window's double-tap key offers Control, Shift and Alt (iTerm2's stored 0 to 2 keep
      their meaning); a saved 3 (Command, the Win key) reads as Control. The Navigation Shortcuts
      choices read "Shortcut key+Number" and "Shortcut key+Alt+Number" instead of Win. The remap
      targets "Left Win" and "Right Win" stay: they remap the physical key, they are not shortcuts.
    - Test `every_default_has_a_chord_without_win_and_no_two_collide` walks every menu item, shipped
      key mapping, preset, number shortcut, workgroup peer, profile shortcut and the password manager,
      AI Chat and Cockpit chords in both modes: no chord uses Win, every default has one, no two
      collide in a scope. Test `a_saved_win_value_becomes_ctrl_shift` covers the migration.

    The chords that moved (the same in both modes; "Now" is the pressed keys):

    | Command | iTerm2 | Before (Win) | Now |
    |---|---|---|---|
    | Shell > New Window with Current Profile | Cmd+Option+Shift+N | Win+Alt+Shift+N | Ctrl+Shift+Alt+N |
    | Shell > New Tab with Current Profile | Cmd+Option+Shift+T | Win+Alt+Shift+T | Ctrl+Shift+F11 |
    | Shell > Split Horizontally... | Cmd+Option+Shift+H | Win+Alt+Shift+H | Alt+Shift+- |
    | Shell > Split Vertically... | Cmd+Option+Shift+V | Win+Alt+Shift+V | Alt+Shift+= |
    | Shell > Print > Buffer | Cmd+Option+Shift+P | Win+Alt+Shift+P | Ctrl+Shift+F10 |
    | Shell > Broadcast Input > Send Input to Current Session Only | Cmd+Option+Shift+I | Win+Alt+Shift+I | Alt+Shift+F1 |
    | Shell > Broadcast Input > Toggle Broadcast Input to Current Session | Cmd+Ctrl+Option+Shift+I | Win+Ctrl+Alt+Shift+I | Alt+Shift+F2 |
    | Termurion > Make Termurion Default Term | Cmd+Ctrl+Shift+\ | Win+Ctrl+Shift+\ | Ctrl+Shift+Alt+F10 |
    | Termurion > Make Terminal Default Term | Cmd+Ctrl+Option+Shift+\ | Win+Ctrl+Alt+Shift+\ | Ctrl+Shift+Alt+F11 |
    | Shell > tmux > Detach | Cmd+Ctrl+Shift+D | Win+Ctrl+Shift+D | Ctrl+Alt+F1 |
    | Shell > tmux > New Tmux Window | Cmd+Ctrl+Shift+N | Win+Ctrl+Shift+N | Ctrl+Alt+F2 |
    | Shell > tmux > New Tmux Tab | Cmd+Ctrl+Shift+T | Win+Ctrl+Shift+T | Ctrl+Alt+F3 |
    | Shell > tmux > Pause Pane | Cmd+Ctrl+Shift+P | Win+Ctrl+Shift+P | Ctrl+Alt+F4 |
    | Shell > tmux > Dashboard | Cmd+Ctrl+Shift+Enter | Win+Ctrl+Shift+Enter | Ctrl+Alt+F5 |
    | Edit > Selection Respects Soft Boundaries | Cmd+Option+Shift+A | Win+Alt+Shift+A | Ctrl+Shift+F5 |
    | Edit > Find > Clear Find | Cmd+Option+Shift+E | Win+Alt+Shift+E | Ctrl+Shift+F2 |
    | Edit > Find > Find All Smart Selection Matches | Cmd+Option+Shift+U | Win+Alt+Shift+U | Ctrl+Shift+F3 |
    | Edit > Find > Pick Result to Open | Cmd+Option+Shift+O | Win+Alt+Shift+O | Ctrl+Shift+F4 |
    | Edit > Find > Filter | Cmd+Option+Shift+F | Win+Alt+Shift+F | Ctrl+Shift+F1 |
    | Edit > Marks and Annotations > Next Annotation | Cmd+Option+Shift+Down | Win+Alt+Shift+Down | Ctrl+Shift+F7 |
    | Edit > Marks and Annotations > Previous Annotation | Cmd+Option+Shift+Up | Win+Alt+Shift+Up | Ctrl+Shift+F8 |
    | Edit > Set Named Mark | Cmd+Option+Shift+M | Win+Alt+Shift+M | Ctrl+Shift+F6 |
    | Edit > Fold All | Cmd+Option+Shift+L | Win+Alt+Shift+L | Ctrl+Shift+F9 |
    | View > Auto Command Completion | Cmd+Option+Shift+Y | Win+Alt+Shift+Y | Alt+Shift+F3 |
    | View > Auto Composer | Cmd+Option+Shift+. | Win+Alt+Shift+. | Alt+Shift+F4 |
    | Session > Stop Coprocess | Cmd+Option+Shift+R | Win+Alt+Shift+R | Alt+Shift+F5 |
    | Session > Bury Session | Cmd+Option+Shift+B | Win+Alt+Shift+B | Alt+Shift+F6 |
    | Session > Open AI Chat... | Cmd+Ctrl+Shift+Y | Win+Ctrl+Shift+Y | Alt+Shift+F7 |
    | Window > Cockpit | Cmd+Option+Shift+C | Win+Alt+Shift+C | Alt+Shift+F8 |
    | Window > Arrangements > Save Current Window as Arrangement | Cmd+Option+Shift+S | Win+Alt+Shift+S | Alt+Shift+F9 |
    | Window > Select Tab > Select Next Tab Group | Cmd+Ctrl+Shift+] | Win+Ctrl+Shift+] | Ctrl+Shift+Alt+PgDn |
    | Window > Select Tab > Select Previous Tab Group | Cmd+Ctrl+Shift+[ | Win+Ctrl+Shift+[ | Ctrl+Shift+Alt+PgUp |
    | Window > Move Tab > Move Tab Left | Cmd+Option+Shift+[ | Win+Alt+Shift+[ | Alt+Shift+PgUp |
    | Window > Move Tab > Move Tab Right | Cmd+Option+Shift+] | Win+Alt+Shift+] | Alt+Shift+PgDn |
    | Debug build only: Capture GPU Frame | Cmd+Ctrl+Option+Shift+G | Win+Ctrl+Alt+Shift+G | Ctrl+Shift+Alt+F12 |
    | Workgroup peer 1 to 9 | Cmd+Option+Shift+1 to 9 | Alt+Shift+Win+1 to 9 | Ctrl+Shift+Alt+F1 to F9 |
    | A profile's new-window shortcut | Cmd+Ctrl+Option+key | Ctrl+Win+Alt+key | Ctrl+Shift+Alt+key |

11. **After a program left the alternate screen, the prompt drew far from where the shell wrote.**
    Seen 2026-10-16 in PowerShell: ssh to a Linux box, tmux (alternate screen), the window
    resized many times (272x70, 80x23, 272x70 and drag steps), tmux detached, ssh exited, and the
    prompt drew on row 6 while PSReadLine echoed `exit` on row 19. Cause: while the alternate
    screen was up, every resize also resized the main screen behind it. A shrink pushed main rows
    into history, and the next growth pulled none back (a pty-backed grid keeps its rows, item 9),
    so the main screen, and the saved cursor the program restores on leaving, sat on other rows
    than ConPTY's. conhost leaves the main buffer alone while the alternate one is up and resizes
    it once, to the final size, when the program leaves (`_handleDeferredResize`). Done
    2026-10-16: alacritty_terminal patch 35 does the same. New live tests in
    `src/resize_live.rs`: a PowerShell stand-in for tmux (alternate screen, status line, scroll
    region, redraw on resize) resized in steps, in a fast drag and as it leaves, real tmux in WSL
    (skipped when absent), and seeded walks that enter and leave the alternate screen among
    resizes. With ConPTY 1.25 and no fix the first stand-in test failed (the grid's prompt on row 13,
    ConPTY's on row 69); with the fix the stand-in, drag, leave-resize and tmux tests pass and the
    idle and in-flight walks still show 0 problems over 6 seeds. Alternate-screen walks (6 seeds): 0 problems when resizes come once the program is up; 2 over 6 seeds when a resize also lands while the program is still starting (the shell's in-flight case, same bound as item 9).

9. **After a resize, typed text lands mid-screen, not at the prompt.** Seen 2026-10-15 in
   PowerShell: a long `dir`, then a window resize, then typing; the echo appeared about 15 rows
   above the prompt. Cause: after a resize the grid pulled scrollback down and moved the prompt
   to the bottom row, while ConPTY keeps the top of its screen in place and adds blank rows under
   the cursor, so the shell drew at ConPTY's row. Fixed for a resize at a quiet prompt in
   3f57288f (alacritty_terminal patch 34: on a pty-backed screen, growth adds rows below the
   cursor and pulls nothing from history), with live ConPTY tests in `src/resize_live.rs` and a
   `pty: resize` line in host.log. A resize while output is printing: 19444cd0 ships ConPTY
   1.25.260930003, which asks the terminal for its cursor after a resize and takes the answer
   (microsoft/terminal PR 19535), so the grid is the authority. Seeded live walks: 0 mismatches at
   an idle prompt over 6 seeds, 0 to 1 with output in flight (only a key typed during a size
   change; Windows Terminal has no sync for that case either).

10. **The menu bar flashed on a Minimal window during a resize.** Seen 2026-10-15. Cause: the
    app kept a note that the bar was hidden, and paths that attach a bar (a menu rebuild, a
    window made again for transparency) did not check it, so a bar stayed attached under the
    client area and showed each time a resize redrew the frame. A probe confirmed a menu on the
    live Minimal window. Done 2026-10-16 in 977bc21a: the app asks the window (`GetMenu`) each
    frame and on each resize, and a bar built for a hidden window is detached as it attaches.

12. **Full screen could not be left: its hotkey needed the Win key.** Seen 2026-10-16. View >
    Toggle Full Screen is iTerm2's Cmd+Ctrl+F, and with the default Shortcut Key (Ctrl+Shift) a
    Cmd chord that no pair of Ctrl, Shift and Alt covered stayed on the Win key. Windows keeps many
    Win chords, and a Win chord is also lost over remote desktop, a KVM or a keyboard sharing
    tool. The menu bar is hidden in full screen, so with the chord lost there was no way out.
    The hook's own decision for Win+Ctrl+F was checked and is right (test
    `the_full_screen_chord_is_taken_by_the_hook_in_win_mode_only`: the Win mode takes the press and
    its key-up), so the fault was the design, not one bug in the hook. Done 2026-10-16: no default
    shortcut needs the Win key now. Cmd+Ctrl is the free pair, Ctrl+Alt in Ctrl+Shift mode (so
    Toggle Full Screen is Ctrl+Alt+F and Move Divider is Ctrl+Alt+arrows) and Shift+Alt in Ctrl+Alt
    mode; an AltGr press that types a character is still left to typing. F11 toggles full screen
    in every mode unless a key mapping claims F11, and the menu shows both. In full screen,
    pressing and releasing Alt alone, or moving the pointer to the top edge of the window, brings
    the menu bar back (`menu_peek.rs`); a second Alt tap or any menu command sends it away.
    Chords that ask for three of iTerm2's modifiers (Cmd+Option+Shift, Cmd+Ctrl+Shift) have no pair
    left in either mode (34 menu items in a release build; nearly all would collide with a
    Cmd+Shift chord on the same key): they have no shortcut outside the Win mode, the menu does not show one, and the Win form
    still works where Windows lets the hook have it. Test
    `every_default_chord_without_win_is_reachable_and_no_two_collide` walks every menu item, shipped
    key mapping and preset. Item 13 supersedes the Win form: those chords have keys of their own now.

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
