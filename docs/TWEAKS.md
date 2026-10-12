# Tweaks after the milestones

Things Robert finds while using Tandem. These are worked on after the main milestones land
(the AI browser tools and 13y2). Add new items at the end. Mark an item done with its commit.

Each item says what is wrong, what it should do, and where it was seen.

## Open

1. **The first menu says "iTerm2", not the product name.** The application menu (first item
   on the menu bar) should be named "Termurion" (see item 2). Check the items inside it too
   (for example "About iTerm2", "Quit iTerm2", "Hide iTerm2") and any other place the menu bar
   or its menus show the iTerm2 product name to the person; keep iTerm2 names only where they
   are file formats or compatibility keys (plists, escape sequences, the Python API). Do this
   together with item 2, so the menus change only once. Seen 2026-10-11.

2. **Rename the whole project from Tandem to Termurion.** Robert chose the name on
   2026-10-11 ("Tandem" is crowded: several products and a trademark-heavy word). Checks that
   day: no web, crates.io, npm, PyPI or GitHub use; termurion.com/.dev/.app/.io/.org did not
   resolve; RDAP lookups the same day returned "not found" for termurion.com, .net, .org, .io,
   .dev and .app (all unregistered then; register the ones you want before announcing). Before
   starting: a USPTO search for TERMURION in classes 9 and 42. Pre-flight scan done 2026-10-11
   (8,752 "tandem" hits; keep the two DPAPI entropy strings byte-identical; migrate data
   folders, registry keys, Credential Manager targets; alias .tandemexport and TANDEM_* vars). The rename covers everything the person or the system sees:
   - window titles, the menu bar, About box, dialogs, banners, tooltips, notifications, the
     tray, the taskbar and Jump List, and every user-facing text;
   - the exe and helper names (tandem.exe, tandem-host.exe, adapters), the installer and
     install-extreme.ps1 (install folder, Start Menu and desktop shortcuts, uninstall entry),
     the AppUserModelID, registry keys, URL and file associations, named pipes, mutexes and
     window classes;
   - data folders (%APPDATA% and %LOCALAPPDATA% "Tandem"): migrate them once on first run of
     the renamed build, keep a copy of the old folder until the migration is confirmed, and
     keep DPAPI-sealed data readable (same user, so the seal survives; test it);
   - the crate and package name, module and doc names, the README, roadmap, parity files,
     checklists and rulings text, the logo and icons, and the GitHub repo
     (datbird/tandem to datbird/termurion; update every remote and scripts/xt paths, and the
     EXTREME build folders);
   - memory notes and handoff files that name Tandem.
   Keep iTerm2 names where they are compatibility keys (see item 1). Plan it as its own short
   milestone with a pre-flight scan for every "tandem" string (case-insensitive) and a
   migration test, then a desktop probe.

6. **Other apps' link schemes (`vscode:`, `slack:`).** iTerm2 opens any scheme
   (m13v1 ruling, item 6); Tandem refuses them by that deliberate security ruling. Changing it
   needs a ruling from Robert on the question and a deny list. Recorded by the
   13y2 carried pass. Not started 2026-10-12 for that reason (tweaks progress.md has the
   questions and the proposed design: confirm first, deny list, the one URL-open owner).

7. **Browser Save Page As has no SSH destination (13x4 ruling).** Item 7 (a) is done (see Done).
   (b) needs a design from Robert before code: a remote folder picker, an upload of a whole saved
   folder (up to 2000 files, 256 MiB) over the SSH file machinery, and what stands in for the Mark
   of the Web, create-new and cleanup guarantees the local saver has. Recorded by the 13y2 parity
   sweep. Looked at 2026-10-12, not started (tweaks progress.md).
8. **Tandem stays running with no window.** On EXTREME, tandem.exe (started from Explorer at
   17:00 on 2026-10-11) and its tandem-host.exe were still running at 22:10 with no window
   (MainWindowHandle 0), so the installer refused to replace them ("Tandem is running"). Robert
   had no Tandem window open. Expected: when the last window closes, Tandem exits (unless a
   setting like a hotkey window or "stay running" asks it not to, and then it shows that it is
   still running, e.g. in the tray). tandem-host.exe exited a few seconds after tandem.exe was
   stopped. Seen 2026-10-11.
   Cause (tw8-windowless.md): this is the intended "resident" mode (iTerm2 parity:
   QuitWhenAllWindowsClosed defaults off, so Tandem stays running after the last window
   closes). Robert (2026-10-11): keep that behavior; the bug is that the tray icon never
   showed, so nothing told him Tandem was still running. Fix the tray: it must always appear
   while Tandem is resident (retry on failure and on TaskbarCreated after an Explorer restart),
   use the real Tandem icon, offer Quit, and log the result.

9. **The app shortcut key (iTerm2's Cmd) must not default to the Windows key.** Win
   collides with many Windows shortcuts (Win+L, Win+D, Win+arrows, Win+Shift+S, Win+number),
   and some never reach an app. Make it a setting: Ctrl+Shift (new default, as Windows Terminal
   and VS Code), Win, or Ctrl+Alt. Not Ctrl alone (shell control keys) and not Alt (iTerm2's
   Option-as-Meta; shells use Alt+B/F). Ctrl+Alt is AltGr on many European layouts, so warn when
   it is chosen. Shortcuts that already use Shift (Cmd+Shift+D) map to Ctrl+Shift+Alt+D. Menus
   show the real keys; key binding import/export keeps iTerm2's Cmd notation; existing saved
   bindings keep working. Robert, 2026-10-11.

## In progress

(none)

## Done

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
   `badge=` writes the session's Badge Color; (b) ExecFailed makes the end bar say "Session failed
   to start." and closing raises no short-lived warning; (c) OSC 21337 already raised the pattern
   notification and the taskbar count (wired through `tick_status_change` earlier, the note was
   stale); a test pins it.

5. **Hotkey-window routing for a launch URL.** Done 2026-10-12 (tw-small, 552a17f): an
   ssh, telnet or ftp URL for a profile with a hotkey window opens there, as iTerm2's `getUrl`.

7a. **Selection helpers in the terminal context menu.** Done 2026-10-12 (tw-small, 13cae8b).
