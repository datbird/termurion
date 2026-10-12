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

3. **OSC 22: a program sets the mouse pointer shape.** iTerm2 supports it
   (VT100XtermParser.m:291, VT100Terminal.m:2723, PTYSession.m:18420). Tandem has no parser entry
   and no pointer-name map, so a program cannot change the pointer over its pane. It should parse
   OSC 22 (and the reset form), map X11 cursor names to `pointer_shape`, and reset on pane leave
   and on a terminal reset. Recorded by 13j and 13y2 Task 10 (parity 03, OSC 22 row).

4. **Three terminal sequences are partial.** (a) `OSC 1337 ; SetColors` does not set the badge
   color; (b) `OSC 1337 ; ExecFailed` keeps its flag but draws no session-ended message; (c) `OSC
   21337` (tab status) keeps the dot, text and priority but raises no notification and no taskbar
   badge. Each is a small change in the module that owns it. Recorded by 13y2 Task 10 (parity 03).

5. **Hotkey-window routing for a launch URL.** iTerm2 opens a URL launch in a hotkey window
   when one fits (AD:2311-2322); Tandem opens it in a normal window. Recorded by the 13y2
   carried pass.

6. **Other apps' link schemes (`vscode:`, `slack:`).** iTerm2 opens any scheme
   (m13v1 ruling, item 6); Tandem refuses them by that deliberate security ruling. Changing it
   needs a ruling from Robert on the question and a deny list. Recorded by the
   13y2 carried pass.

7. **Two small gaps found by the 13y2 sweep.** (a) The selection helpers (numbers, dates,
   colors, bytes, Base64, synonyms) and the Smart Selection actions that iTerm2 puts in a
   terminal's context menu (`iTermTextViewContextMenuHelper.m:440-460`) show only in a browser
   pane's menu; `src/ctx_help.rs` is pure and shared, so the terminal menu needs the call.
   (b) Browser Save Page As has no SSH destination (13x4 ruling). Recorded by the 13y2 parity
   sweep.

8. **Tandem stays running with no window.** On EXTREME, tandem.exe (started from Explorer at
   17:00 on 2026-10-11) and its tandem-host.exe were still running at 22:10 with no window
   (MainWindowHandle 0), so the installer refused to replace them ("Tandem is running"). Robert
   had no Tandem window open. Expected: when the last window closes, Tandem exits (unless a
   setting like a hotkey window or "stay running" asks it not to, and then it shows that it is
   still running, e.g. in the tray). tandem-host.exe exited a few seconds after tandem.exe was
   stopped. Seen 2026-10-11.

## In progress

(none)

## Done

1. **The menu bar ignores Windows dark mode.** Done 2026-10-11 (main, tw1-darkmenu merged): menus follow the Windows app mode and switch live; Automatic uses AllowDark so the title bar and terminal colors follow too. Needs Robert's desktop look (dark, light, live switch).

2. **"Jump List: Access is denied. (0x80070005)" red banner.** Cause: EXTREME has "Show
   recently opened items in Start, Jump Lists and File Explorer" turned off, so Windows refuses
   the custom Recents category and the whole Jump List commit fails. Done in 13y2 (13b7b50, retry
   in 634e998): the custom categories are skipped, the taskbar tasks stay, no banner. Seen
   2026-10-11. Checklist m13y2 row 22.
