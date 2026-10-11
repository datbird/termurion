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

2. **The first menu says "iTerm2", not "Tandem".** The application menu (first item on the
   menu bar) should be named "Tandem". Check the items inside it too (for example "About
   iTerm2", "Quit iTerm2", "Hide iTerm2") and any other place the menu bar or its menus show
   the iTerm2 product name to the person; keep iTerm2 names only where they are file formats or
   compatibility keys (plists, escape sequences, the Python API). Seen 2026-10-11.
   Name candidates (Robert, 2026-10-11; "Tandem" is crowded if published): pinned **Orrery**
   and **Halyard**; also Armillary, Tellurion, Binnacle, Mizzen, Fairlead, Parhelion, Taffrail,
   Sheave, Ecliptic, Perihelion. Quick web checks
   found no terminal or developer tool using any of them (not a trademark search). Decide the
   name before this rename, so the menus change only once.

## In progress

1. **"Jump List: Access is denied. (0x80070005)" red banner.** Cause: EXTREME has "Show
   recently opened items in Start, Jump Lists and File Explorer" turned off, so Windows refuses
   the custom Recents category and the whole Jump List commit fails. Fix (in 13y2): skip the
   custom categories, keep the taskbar tasks, show no banner. Seen 2026-10-11.

## Done
