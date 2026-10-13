# Milestone 13g acceptance checklist

Build a release binary (`scripts/xt build --release`). Close any open Termurion window, copy
`tandem.exe`, `tandem-host.exe`, `tandem-git.exe`, `conpty.dll`, and `OpenConsole.exe` from
`C:\Users\datbi\src\termurion\target\release\` together, then run `tandem.exe` in the desktop
session (not over SSH).

Setup: Windows 11 22H2 or later (a Windows 10 machine for the blur fallback line) with two
virtual desktops (Win+Ctrl+D to add one) and, for the display lines, a second monitor. Make a
profile with Transparency 0.3 (Settings > Profiles > Window) and a second with Transparency 0 and
Blur on; make a Dedicated Hotkey Window profile. Mark each line pass or fail. Nothing of 13g was
checked on screen during the build; every line below is unverified until you run it. Line 1 gates
the see-through rows in `docs/parity/`: if it passes the eight rows it names read have (blur and Blur Radius stay partial either way), if it fails they stay partial.

| # | Check | Result |
|---|---|---|
| 1 | P1, run this first: open a profile with Transparency 0.3 (it opens a see-through window on the second Direct3D 12 instance) and, at the same time, a normal opaque window and Settings. All three draw, none is black or blank, and the opaque window and Settings stay opaque. Start Termurion from a terminal-less launch and read stderr (or run a debug build): the `init_gpu` line names the opaque path's backend. If this line fails, every see-through row stays partial ("stored and shown; no see-through rendering on this path") and the banner "transparency unavailable" shows once. | |
| 2 | A profile with Transparency 0.3 shows the desktop through the default background and the margins at about 70 percent opacity, not through the tab bar, status bar, toolbelt or dividers, the cursor or a popup. | |
| 3 | A light theme at Transparency 0.5 looks even: no bright fringe at the edges of text or cells. | |
| 4 | Keep background colors opaque keeps a colored prompt segment (and the selection) solid while the default background stays see-through; turning it off makes them see-through. | |
| 5 | A red cell in a see-through pane is exactly as see-through as the default background (no darker patch, no 0.91 double layer). | |
| 6 | Win+U turns see-through off and on for the current window only, with the View > Use Transparency check mark following; other windows do not change. | |
| 7 | View > Disable Transparency for Active Window makes the focused window opaque and an unfocused see-through window stays see-through; the check follows. | |
| 8 | Full screen (Win+Enter) with "Disable transparency for non-native fullscreen windows by default" on drops see-through and leaving restores it; with it off, nothing changes. A window restored from a saved full screen state keeps its saved Use Transparency. | |
| 9 | Blur on Windows 11 22H2 or later frosts what shows through a see-through window (acrylic); Win+U off removes it and on restores it; it follows a tab switch, minimize and restore, and full screen. UNVERIFIED (P18d): acrylic may not show behind the DirectComposition swapchain; if so, blur rows stay partial. On Windows 10 or before 22H2 nothing is frosted and nothing breaks. | |
| 10 | Blur on a No Title Bar, Compact or Minimal window and beside the 13f heavy border: no black frame, no lost border (frame extension UNVERIFIED). | |
| 11 | Settings > Profiles > Window: Keep background colors opaque, Blur and Blur Radius are greyed at Transparency 0 (Blur Radius also until Blur is on); the override notice shows under Transparency above 0; the large radius warning (with its glyph) shows above 26. The radius slider changes nothing. | |
| 12 | A new Dedicated Hotkey Window is see-through (Transparency 0.3) and blurred. | |
| 13 | A window with Transparency 0 is unchanged in look and speed and still uses the original renderer path (the `init_gpu` line shows the same backend as before); only see-through windows use the second instance. | |
| 14 | Raising a profile's Transparency above 0 while its window is open recreates that window once, see-through, with its tabs, panes, shell sessions, position, size, menu bar, caption and border intact, no black frame, and focus only if it had focus. Recreating a full screen window keeps it full screen; a floating (topmost) hotkey window and an Accessory window stay topmost; a hidden window stays hidden; an open Password Manager is closed first and can reopen. A minimized window stays minimized and un-minimizes to its old frame (not at the screen corner, not zero size); a maximized window stays maximized and restores to its old normal frame. | |
| 15 | The first window of a transparent default profile opens opaque and is recreated once at launch (one flicker, no black window). With Initial Use Transparency off in the default profile the first window opens opaque. | |
| 16 | A background image under a see-through pane stays opaque and shows through the margins only at the pane alpha; dimmed inactive panes in a see-through window are a little more opaque (kept quirk). | |
| 17 | On a machine or session without a working second instance, windows stay opaque and the banner shows once; a failed window is not retried on every edit. | |
| 18 | The virtual desktop manager exists in the desktop session (the `desktops` unit test may skip over SSH). | |
| 19 | Space "Desktop 2" (two desktops) opens a new window on desktop 2 and switches there (UNVERIFIED, P18c: activating a window on another desktop switches to it; if it does not, the window stays on desktop 2 and one line is logged after 300 ms). A new tab, a Duplicate Window, an arrangement and a restored window never switch desktops. | |
| 20 | Space "Desktop 9" with two desktops notifies "Can't switch to desktop 9" once per run. | |
| 21 | Picking a numbered desktop in Settings shows the warning with its "Do not show" checkbox. | |
| 22 | MoveWindowToDesktop on a not-yet-shown window: a profile with Hide After Opening and Space "Desktop 2" opens its window on desktop 2 when it is first shown (UNVERIFIED). | |
| 23 | "All Desktops" follows a desktop switch within a second (the window is on the new current desktop; it is never pinned), and a hotkey window with it shows on the current desktop when it rolls in. A hotkey window with a numbered Space also shows where you are (iTerm2's behavior). | |
| 24 | Restore windows to same desktops brings windows back to their desktops at the next launch, and does nothing for Restore Window Arrangement; with the system restore policy off the row is greyed. | |
| 25 | Unplugging a monitor refits a top-of-screen window and a maximized one after one second (`UpdateScreenParamsDelay`), keeps a normal window where it is, and a hotkey window is re-framed; a burst of display messages (docking) refits once; a taskbar move refits edge windows. | |
| 26 | With `ChaseAnchoredScreen` on, a window whose profile Screen is 1 moves back when the monitor returns (a maximized window stays maximized); with it off nothing moves. | |
| 27 | A window recreated for alpha still refits after a display change. | |
| 28 | Exclude from taskbar and Alt+Tab switcher asks first (Cancel and closing the box leave it off), then removes every terminal window's taskbar button and Alt+Tab entry, and Settings and panel windows' too; windows keep their size, minimized, maximized and full screen state and focus across the flip, and the client area keeps its size (tool windows may draw a slimmer caption on some themes). | |
| 29 | In that mode the notification-area icon shows (with StatusBarIcon on); its right-click menu lists every window first (Settings, chat and panel windows included; a pick brings it forward and un-minimizes it), then Settings, Bring All Windows to Front, Buried Sessions, Check For Updates (disabled until 13v), Main Menu (no check marks) and Quit Termurion; each works. | |
| 30 | "...but only if all windows are hotkey windows" applies only while every window is a hotkey window. | |
| 31 | `StatusBarIcon` off removes the icon (iTerm2's rule); then starting Termurion again while it runs (a bare launch) is the dock click: it brings the last window forward or opens one, shows no second Termurion, and never shows a hidden hotkey window unless the hotkey window's dock preference says so. A launch with a URL or the relaunch after an import is not forwarded. | |
| 32 | After an Explorer restart (End task on explorer.exe, then run explorer.exe) the icon comes back; starting Termurion before the taskbar exists (logon) adds the icon once the taskbar appears. | |
| 33 | Closing the last window, Quit, Settings > Exit and the relaunch after Import All Settings and Data all remove the icon (no ghost); a killed process leaves a ghost the system clears on hover (kept quirk). | |
| 34 | A left click on the icon with "Always Show" shows the hotkey window and a second click hides it; with "Show if no other windows" it shows only with no regular window; a double click toggles twice (kept quirk). | |
| 35 | An open hotkey window is in Alt+Tab; with `HotkeyWindowsExcludedFromCycling` it leaves Alt+Tab and Next/Previous Window skip it. A hidden hotkey window is never in Alt+Tab. | |
| 36 | No window can become unreachable: with Exclude on, StatusBarIcon on, every window minimized, a window is reachable from the icon's menu; with StatusBarIcon off it is reachable by starting Termurion again; after a monitor is unplugged every window refits onto a screen; a window recreated for alpha is on screen; no hidden window outlives its hotkey. | |
| 37 | Kept quirk (ruling): a recreate flickers once and RenderDoc cannot capture the visual swapchain; two devices stay in memory for the rest of the run once the first see-through window has opened. | |
| 38 | Kept quirk (ruling): background images stay opaque under transparency; the cursor, tab bar, status bars, toolbelt, dividers and popups never take the alpha. | |
| 39 | Kept quirk (ruling): Blur Radius is stored and shown but acrylic has one fixed radius; the large radius warning shows although the slider does nothing. | |
| 40 | Kept quirk (ruling): a numbered Space applies only to new windows (a new tab never switches); Restore windows to same desktops is never applied to saved arrangements. | |
| 41 | Kept quirk (ruling): the tray menu's copy of the Main Menu shows no check marks; Check For Updates is disabled until 13v. | |
| 42 | A see-through window on desktop 2 recreated by a profile edit (raise its Transparency) stays on desktop 2 and does not jump to the desktop you are on. | |
| 43 | Restore windows to same desktops with a transparent first window: quit with it on desktop 2, start Termurion; it returns to desktop 2 (it is recreated once at launch and moved back before it shows). | |
| 44 | Toggling Exclude from Dock and Cmd-Tab (UI element mode) on a profile that is see-through shows no taskbar button flash on the recreated window, and the tray menu opens with no terminal window made yet (UI element mode, hotkey profiles only). | |
| 45 | Tray Bring All Windows to Front does not show a hidden hotkey window. | |
| 46 | Toggling Exclude with a window on desktop 2 leaves it on desktop 2 (the tool-window flip hides and reshows the window; unverified that it keeps its desktop). | |
| 47 | Windows 10 (or 11 before 22H2): a see-through window with Blur on shows no glass frame under its transparent pixels (the frame is extended only when DWM accepts the backdrop). | |
