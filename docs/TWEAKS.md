# Tweaks after the milestones

Things Robert finds while using Termurion. These are worked on after the main milestones land
(the AI browser tools and 13y2). Add new items at the end. Mark an item done with its commit.

Each item says what is wrong, what it should do, and where it was seen.

## Open

7. **Browser Save Page As has no SSH destination (13x4 ruling).** Item 7 (a) is done (see Done).
   (b) needs a design from Robert before code: a remote folder picker, an upload of a whole saved
   folder (up to 2000 files, 256 MiB) over the SSH file machinery, and what stands in for the Mark
   of the Web, create-new and cleanup guarantees the local saver has. Recorded by the 13y2 parity
   sweep. Plan written 2026-10-12, build after the rename: plan: docs/superpowers/plans/2026-10-20-tw7b-save-page-to-ssh.md
8. **Termurion stays running with no window.** On EXTREME, tandem.exe (started from Explorer at
   17:00 on 2026-10-11) and its tandem-host.exe were still running at 22:10 with no window
   (MainWindowHandle 0), so the installer refused to replace them ("Termurion is running"). Robert
   had no Termurion window open. Expected: when the last window closes, Termurion exits (unless a
   setting like a hotkey window or "stay running" asks it not to, and then it shows that it is
   still running, e.g. in the tray). tandem-host.exe exited a few seconds after tandem.exe was
   stopped. Seen 2026-10-11.
   Cause (tw8-windowless.md): this is the intended "resident" mode (iTerm2 parity:
   QuitWhenAllWindowsClosed defaults off, so Termurion stays running after the last window
   closes). Robert (2026-10-11): keep that behavior; the bug is that the tray icon never
   showed, so nothing told him Termurion was still running. Fix the tray: it must always appear
   while Termurion is resident (retry on failure and on TaskbarCreated after an Explorer restart),
   use the real Termurion icon, offer Quit, and log the result.
   2026-10-12 desktop try: the icon was there but Windows 11 hid it under the ^ overflow (new
   icons start hidden; Robert's EXTREME now shows it). Three bugs found and fixed the same day:
   (a) every tray menu pick (Quit, New Window) did nothing while resident: the find handler
   claimed every command when no session was focused (`browser_find::claims_without_session`);
   (b) a window woken from resident lost its menu bar: the old muda menu was dropped after the
   new one was attached, and muda's drop clears the window's menu (`replace_menu_bar`);
   (c) Settings > Advanced was blank: an unbounded right-to-left row took the whole page
   (same class fixed in shot_ui, pwm_ui, settings_ai, statusbar_setup_ui). host.log now logs
   each tray menu, pick and quit step. Needs Robert's try: tray Quit and New Window, the menu
   bar after a tray reopen, Settings > Advanced.

## In progress

(none)

## Done

10. **Help links pointed at iTerm2's site.** Done 2026-10-13 (main): Help and About open the repository; every other iTerm2 link (help pages, bug reports, installers, download.sh, manifest defaults) comes from `src/links.rs` and points at the repo. Help pages are in docs/help, installers and utilities in dist. Downloads 404 while the repository is private. Plan: docs/superpowers/plans/2026-10-13-tw10-repo-links.md.

1. **The first menu says "iTerm2", not the product name.** Done 2026-10-12 (branch m14r-text, c18fb0b4): the application menu and its items (About, Hide, Quit, Make Default Term, Help) read Termurion; menu identifiers and selectors keep iTerm2 text (compatibility keys).

2. **Rename the whole project to Termurion.** Done 2026-10-12 (branches m14r-text and m14r-data): user-visible text (b1fd3809), menu bar (c18fb0b4), docs and logo, identifiers, data and registry names.

9. **The app shortcut key (iTerm2's Cmd) must not default to the Windows key.** Done 2026-10-12 (branch tw9-keys, c9b20a0 and 0d58329): Keys > Shortcut Key is Ctrl+Shift (new default, also for anyone who never chose), Win, or Ctrl+Alt (warned: AltGr). Cmd+X is the pair, Cmd+Shift+X is Ctrl+Shift+Alt+X, Cmd+Option+X the third pair; chords no pair reaches (Cmd+Ctrl) stay on Win. Menus, tab/pane/window numbers, composer help and the bindings table show the real keys; stored bindings and import/export keep Cmd. A bound shortcut beats key reporting protocols (as iTerm2's menu does); unbound Ctrl+Shift keys reach the program. Rulings: docs/superpowers/rulings/tweaks-9-rulings.txt. Needs Robert's desktop try (Ctrl+Shift+D, Ctrl+Shift+T, Ctrl+Shift+C/V, and a program under CSI u).

1. **The menu bar ignores Windows dark mode.** Done 2026-10-11 (main, tw1-darkmenu merged): menus follow the Windows app mode and switch live; Automatic uses AllowDark so the title bar and terminal colors follow too. Needs Robert's desktop look (dark, light, live switch).

2. **"Jump List: Access is denied. (0x80070005)" red banner.** Cause: EXTREME has "Show
   recently opened items in Start, Jump Lists and File Explorer" turned off, so Windows refuses
   the custom Recents category and the whole Jump List commit fails. Done in 13y2 (13b7b50, retry
   in 634e998): the custom categories are skipped, the taskbar tasks stay, no banner. Seen
   2026-10-11. Checklist m13y2 row 22.

3. **OSC 22: a program sets the mouse pointer shape.** Done 2026-10-12 (tw-small, 396cd06): the
   23 X11 names iTerm2 knows map to pointer shapes (an exact table; an empty or unknown name puts
   the default back, as iTerm2's lookup miss does); the shape replaces the I-beam and the
   reporting ring over that pane; RIS, Reset and the program ending clear it.

4. **Three terminal sequences were partial.** Done 2026-10-12 (tw-small, 62f70f5): (a) SetColors
   `badge=` writes the session's Badge Color at alpha 1, as iTerm2 does (be18ca8); (b) ExecFailed makes the end bar say "Session failed
   to start." and closing raises no short-lived warning; (c) OSC 21337 already raised the pattern
   notification and the taskbar count (wired through `tick_status_change` earlier, the note was
   stale); a test pins it.

5. **Hotkey-window routing for a launch URL.** Done 2026-10-12 (tw-small, 552a17f): an
   ssh, telnet or ftp URL for a profile with a hotkey window opens there, as iTerm2's `getUrl`.
   Review fix (this branch): a window that fails to be made forgets its pending URL, and a URL
   that would be refused queues nothing and shows no banner.

6. **Other apps' link schemes (`vscode:`, `slack:`).** Done 2026-10-12 (tw-small, a35fa74 and
   a5633ab; Robert's ruling): asked once per scheme with Always, Once or Never, naming the app
   Windows registered (AssocQueryStringW, never link text); Always and Never are kept in the settings
   LinkSchemesAlways and LinkSchemesNever (Advanced > General, editable); a fixed blocked list
   (ms-msdt, search-ms, file, javascript, ms-settings, the whole Office family, jnlp,
   microsoft-edge and others, one table with reasons) never opens; no registered app, a handler
   that is not a .exe (resolved with ASSOCSTR_EXECUTABLE), or a cmd, powershell, wscript, mshta or
   rundll32 app, is refused; the two lists stay local (never synced or imported); ftp, ssh, telnet,
   x-man-page, iterm2 and termurion stay Termurion's own (security review fixes I1 to I4); launched by ShellExecuteW with the encoded URL as the one file argument. m13v1
   ruling updated.

7a. **Selection helpers in the terminal context menu.** Done 2026-10-12 (tw-small, 13cae8b).
