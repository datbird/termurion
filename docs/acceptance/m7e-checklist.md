# Milestone 7e acceptance checklist

Close any open Termurion window. Copy `tandem.exe` and `tandem-host.exe` from
`%USERPROFILE%\src\termurion\target\release\` together, then run `tandem.exe`. Cmd is the Win key and
Option is Alt. Mark each line pass or fail.

| # | Check | Result |
|---|---|---|
| 1 | Settings > Keys: under "Hotkey", press "Create a Dedicated Hotkey Window...". The "HotKey Window Preferences" window opens with the text about configuring a new profile. Click "Click to Set" (it reads "Recording"), press Ctrl+` : it shows the chord. OK: "Hotkey Window Successfully Configured" names the new profile "Hotkey Window". | |
| 2 | Click into another app (Notepad). Press Ctrl+` : a window slides down from the top edge, full width, about 25 rows, with a shell, and has focus. There is no taskbar button for it. | |
| 3 | Press Ctrl+` again: it slides up and away, and focus goes back to Notepad. | |
| 4 | Show it, then click Notepad: it hides by itself (auto-hide). Show it, open Settings (Win+,): it stays. | |
| 5 | Window > Pin Hotkey Window (Win+Shift+-) is checked while the hotkey window is in front. With it pinned, clicking Notepad leaves the window up; Ctrl+` then focuses it; Ctrl+` again hides it. Unpin it. | |
| 6 | Profiles > Hotkey Window > Keys: "Configure Hotkey Window". Tick "Double-tap key:" with Control, clear the Hotkey field, OK. Tap Ctrl twice quickly: the window toggles. Hold Ctrl for a second: nothing. Type with AltGr twice (on a layout that has it): nothing. | |
| 7 | In the sheet, set the Hotkey to Alt+Space: "Confirm Shortcut" asks whether you are sure ("This looks like a commonly used keystroke."). Cancel keeps the old one. | |
| 8 | Set the Hotkey to Win+T (the shell owns it): the hotkey still works and Windows' taskbar-focus action does not run. Set a chord another app registered (for example one of your AutoHotkey chords): the sheet says "Warning: Another app uses this shortcut". | |
| 9 | Duplicate the Hotkey Window profile and give the copy the same hotkey: the sheet says "Warning: Another profile uses this shortcut". Press the hotkey: both windows show together; again: both hide. Delete the copy. | |
| 10 | Additional Hotkeys...: add F12 as a second shortcut, OK, OK. F12 toggles the window too. | |
| 11 | Profile Window > Style "Normal" on the hotkey profile: the window shows and hides at once (no slide). Style back to "Full-Width Top of Screen". | |
| 12 | "Floating window" on: the shown hotkey window stays above other windows. | |
| 13 | "Automatically reopen on app reactivation" on: show it, click Notepad (it hides), click a Termurion window: it comes back. | |
| 14 | Close every regular Termurion window: Termurion keeps running (the hotkey still shows its window). Close the hotkey window's last tab: Termurion ends. | |
| 15 | Settings > Keys: tick "Show/hide all windows with a system-wide hotkey": it becomes Alt+Space. With Termurion in front, Alt+Space minimizes all its windows; from another app, Alt+Space brings them back. With no windows open, it opens one. | |
| 16 | Settings > General > Startup "Only Restore Hotkey Window": quit and start Termurion: no window shows; the hotkey shows the hotkey window. Set it back to "Use System Window Restoration Setting". | |
| 17 | With the hotkey window shown and holding text, quit and relaunch: it does not show at launch; the hotkey shows it with its text restored. | |
| 18 | Keys: bind a key to "Toggle Pin Hotkey Window" in the hotkey profile: the key flips the Pin check in the Window menu. | |
