# Tweaks after the milestones

Things Robert finds while using Tandem. These are worked on after the main milestones land
(the AI browser tools and 13y2). Add new items at the end. Mark an item done with its commit.

Each item says what is wrong, what it should do, and where it was seen.

## Open

1. **The menu bar ignores Windows dark mode.** The "iTerm2", "Shell", "Edit", "View",
   "Session", "Scripts", "Profiles", "Window" and "Help" bar and its drop-down menus are always
   white. They should follow the Windows app mode (Settings > Personalization > Colors >
   "Choose your default app mode"): dark menus in dark mode, light menus in light mode, and a
   live switch when the person changes the mode. Seen on EXTREME, 2026-10-11 (dark mode, white
   bar). Note: the native Win32 menu bar has no documented dark mode; the usual paths are the
   uxtheme `SetPreferredAppMode` / `AllowDarkModeForWindow` calls or an owner-drawn bar. Pick
   one in a short plan first.

2. **The first menu says "iTerm2", not the product name.** The application menu (first item
   on the menu bar) should be named "Termurion" (see item 3). Check the items inside it too
   (for example "About iTerm2", "Quit iTerm2", "Hide iTerm2") and any other place the menu bar
   or its menus show the iTerm2 product name to the person; keep iTerm2 names only where they
   are file formats or compatibility keys (plists, escape sequences, the Python API). Do this
   together with item 3, so the menus change only once. Seen 2026-10-11.

3. **Rename the whole project from Tandem to Termurion.** Robert chose the name on
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
   Keep iTerm2 names where they are compatibility keys (see item 2). Plan it as its own short
   milestone with a pre-flight scan for every "tandem" string (case-insensitive) and a
   migration test, then a desktop probe.

## In progress

1. **"Jump List: Access is denied. (0x80070005)" red banner.** Cause: EXTREME has "Show
   recently opened items in Start, Jump Lists and File Explorer" turned off, so Windows refuses
   the custom Recents category and the whole Jump List commit fails. Fix (in 13y2): skip the
   custom categories, keep the taskbar tasks, show no banner. Seen 2026-10-11.

## Done
