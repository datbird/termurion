# Milestone 13u acceptance checklist (keys and hotkeys)

Build a release binary (`scripts/xt build --release`); close any open Tandem window, copy
`tandem.exe`, `tandem-host.exe`, `tandem-git.exe`, `conpty.dll`, and `OpenConsole.exe` from
`C:\Users\datbi\src\tandem\target\release\` together, then run `tandem.exe`. Run the whole list in
a desktop session on EXTREME (not over ssh: a session without a desktop cannot show a layout, a
fade or a focus change). Mark each line pass or fail. Rows marked "not run in tests" were never
exercised by the automated suite, which has no composed desktop.

Setup: Settings > Advanced > Experimental Features > "Support DECSET 1036?" on for rows 1 to 4.
Install a second keyboard layout (German) for rows 14 to 17. Have tmux 3.2 or newer on a host
reachable by `tmux -CC` for the tmux rows. Keep a Notepad window open.

## Keys

| # | Check | Result |
|---|---|---|
| 1 | With Support DECSET 1036 on and Left Alt key changeable on (profile Keys), Left Alt Key Sends = Normal: in bash run `printf '\e[?1036h'`, then Alt+b moves back a word. | |
| 2 | `printf '\e[?1036l'` and Alt+b no longer moves back a word (the mode is off again). `printf '\e[?1036$p'` prints `^[[?1036;1$y` while set and `;2$y` while reset. | |
| 3 | Right Alt key changeable off (the default): Right Alt ignores the mode. Turn Support DECSET 1036 off in Advanced: the `printf` changes nothing (iTerm2's release default). | |
| 4 | A soft reset (`printf '\e[!p'`) and a full reset (`reset`) clear the mode. | |
| 5 | In a program using the kitty protocol (`kitten show-key -m kitty`), with Left Control Key set to Hyper in the profile, Left Ctrl+a reports modifier 17 and Right Ctrl+a reports 5; Meta reports 33; Left Win Key as Super: Win+a (a chord Windows does not keep) reports 9. In a plain shell Ctrl+a still sends ^A whatever the mapping. Not run in tests: which physical side winit reports (left and right Ctrl and Win). | |
| 6 | Left Alt remapped to Control (Keys > Remap Modifiers), Remap modifiers globally off: Alt+C interrupts a running command in Tandem (^C) and, in a Tandem text field, copies; Alt+Tab still switches windows; Right Alt is unchanged. | |
| 7 | With Remap modifiers globally on, Left Alt as Control: Alt+C in Tandem interrupts as before. Hold the remapped Alt, flip the switch, release: no modifier stays down in Windows (try Ctrl+click in Notepad afterwards). | |
| 8 | Left Win remapped to Control with remapping not global: Win+C interrupts in Tandem and Win+T does not open a tab; Right Win chords still run menu commands. | |

## Respect shortcuts, hotkeys and the Windows shell

| # | Check | Result |
|---|---|---|
| 9 | Keys > Remap Modifiers: "Respect shortcuts from Windows keyboard shortcuts" is ticked by default. Add a global key mapping on Win+B (any action): with the option on, Win+B in Tandem does Windows' (notification area focus); off, it runs the mapping. Win+T still makes a tab either way. (Win+E is a Tandem menu command and never yields.) | |
| 10 | Stock shortcuts: Win+Left and Win+Right change tab, Win+Home, Win+Shift+Left and Win+Up and Down still work in Tandem (the stock mappings are Tandem's own) with the option on. Edit the Win+Left mapping to Send Text: it now yields to Windows (snaps the window) until the option is off. The Natural Text Editing preset's Win+Left and Win+Right still work unedited. | |
| 11 | From the work Mac through bumbles: Cmd+T (new tab), Cmd+Left (previous tab) and Cmd+D still reach Tandem; a mapping made on Cmd+B goes to Windows until the option is off. bumbles-injected Win+Arrow still switches tabs. | |
| 12 | Windows 11 shell chords still reach Windows with a person's own mapping on them and the option on: Win+Ctrl+Space (previous input language) and Win+Ctrl+1 (last window of the first pinned app). To confirm and add to the shell list if Windows consumes them: Win+Alt+Up and Down, Win+Ctrl+O, Win+Shift+R, Win+Shift+Enter. | |
| 13 | The remap hook never acts while another app is in front: with Left Alt remapped and Remap globally on, type in Notepad and Edge: Alt behaves as Alt (menus open, Alt+Tab works) and Win chords are Windows'. Hotkey chords registered for Tandem (hotkey windows) still work from the other app. | |
| 14 | Force keyboard on, Keyboard locale German: the popup lists installed layouts by name (US and US-International distinct) and is greyed until Force keyboard is on. Typing in Tandem gives German keys (z and y swap). Not run in tests: ActivateKeyboardLayout and the layout list on a desktop. | |
| 15 | Switch to Notepad: the previous layout is yours again; back to Tandem: German again. Opening Settings or another Tandem window does not flap the layout. | |
| 16 | Switch layouts inside Tandem with Win+Space: forced back once; a second switch sticks until you leave and return. | |
| 17 | "Only force once per session": a new tab forces German at its first focus only; you may switch afterward; leaving Tandem does not restore, so under Windows' default input mode (one input method for all apps) Notepad types German too until Tandem exits; closing Tandem restores it. A stored layout that is no longer installed shows empty and forces nothing. | |
| 18 | No modifier or layout stays changed after Tandem quits, crashes or Erase All: after each of File > Quit, `taskkill /f /im tandem.exe` (a crash, hard kill: Windows removes the hook; a layout change is per thread), and Erase All, press Alt, Ctrl and Win in Notepad (none stuck) and check the layout is yours. | |
| 19 | With a global key mapping on a Win chord and Respect on, Win+E opens Explorer (a Tandem menu command Use Selection for Find still wins in Tandem; from another app it is Windows'). | |
| 20 | The Undo key action in a Snippets or Actions tool field undoes the last change; in the terminal it beeps. | |

## Session hotkeys

| # | Check | Result |
|---|---|---|
| 21 | Edit Session (Win+I) on a pane shows a "Session hotkey" row; Settings opened with no session shows none, and neither does another profile picked in the list while Edit Session is open (select the session's profile again: the row is back). Record Ctrl+Alt+1 on two sessions: from another app the press brings a session's window, tab and pane forward; a second press cycles to the other session; it does not flip back to the previous window. | |
| 22 | Record a bare key, Shift+A or Win+E: the row warns and nothing registers. Win+Ctrl+Space and Win+Ctrl+1 are refused too (Windows keeps them). | |
| 23 | A program-supplied session hotkey asks and shows the true chord: from a shell set it (`it2` or the API SetProfileProperty "Session Hotkey"), or attach a tmux whose `@hotkeys` holds one: the question's first line is the chord that would register, on its own (Ctrl+Alt+1 for a keycode of 1, not the characters), then "A program in session 'name' asks to register this chord ..." (or "tmux session 'name'"). A tmux session named with a right-to-left override (`tmux new -s "$(printf 'a\u202eb')"`) or a 10 KB name leaves the chord line intact; the name is cut with an ellipsis. Ignore leaves it off and the row shows "Not registered: you chose Ignore"; Register turns it on. After Register, change the shared profile's Session Hotkey another way (the API's SetProfileProperty on the profile, not the session): the allowed chord still works and the row shows "Not registered: this is not the hotkey you allowed ...". | |
| 24 | A flood of requests gives at most one prompt: a loop that sets 40 different session hotkeys from one session produces one question at a time, and after Ignore none. | |
| 25 | A hotkey restored from an arrangement or an Undo Close asks once more (it is treated as a program's value). | |
| 26 | tmux -CC: a pane's session hotkey survives detach and attach (after Register); `tmux show -v @hotkeys` shows `h_` hex for the live panes only; no entry grows after panes close. | |
| 27 | Close the session: its hotkey stops; quit Tandem: all hotkeys stop. | |

## Hotkey windows and the pin

| # | Check | Result |
|---|---|---|
| 28 | ShowPinnedIndicator on, a hotkey window whose profile has Auto-hide off: each pane shows a pin glyph in its top-right indicators (after the broadcast and coprocess glyphs) with the hover text "This Hotkey Window is pinned."; auto-hiding or off: none. | |
| 29 | The fade on NVIDIA, then on Intel: the default (see-through, DirectComposition) hotkey window fades in and out over 0.25 s by hotkey. Normal and centered styles fade in place; a top-style window slides and fades together (ease-in); full screen does not fade. Not run in tests: the DirectComposition present itself. | |
| 30 | A hotkey profile with Transparency 0 (an opaque window) shows and hides at once with the shipped default. To try its fade, set `FADE_LAYERED` to `true` in `src/hotkey_motion.rs`, rebuild, and check frames are never blank, the window is fully opaque and clickable after the fade, and no half-transparent window is left; if any frame is blank set it back to `false` (the safe default; opaque windows then do not fade). Do this on Intel and NVIDIA; record the result in the roadmap before the two Animate rows become have. Before flipping it for good, `set_window_alpha` must follow the removal of `WS_EX_LAYERED` with `SetWindowPos(SWP_FRAMECHANGED)`, or a stale frame can show after the fade. | |
| 31 | "Animate hotkey window" off: every style shows and hides at once. Pressing the hotkey mid fade leaves no split or stuck window; switching a window between opaque and see-through while it fades ends the fade at once. | |

## tmux

| # | Check | Result |
|---|---|---|
| 32 | The forced layout on a real desktop in a tmux tab is untouched: attach with tmux -CC while Force keyboard is on and the layout is still the forced one. | |
| 33 | Attach a tmux -CC session with a client smaller than the tab (resize Tandem larger before tmux answers): the panes draw at tmux's size at the top left and the strips to the right and below are 10% lighter than a dark background (10% darker than a light one); then tmux resizes and the strips shrink to under one cell. Clicks, selection, the find bar and pane title bars stay inside the panes; clicks in the filler do nothing. | |
| 34 | Full screen with UseBlackFillerColorForTmuxInFullScreen on: the strips are black; off: the tint. A see-through profile: the filler takes the pane's transparency. | |
| 35 | TmuxWindowsOpenInBackground on: `tmux new-window` in a Tandem window with several tabs adds a tab but the selected tab and the keyboard focus stay; background tmux windows do not steal focus: a window that becomes its own Tandem window opens without taking it (Windows may still raise it; report it if so). Off: the new tab is selected. Attaching keeps focus on the attaching tab. | |
| 36 | A tmux pane whose profile has Title Components Custom with no Title Function shows the tmuxTitlePrefix then the tmux window name; with a Title Function it shows the prefix then the function's value; with the built-in title it is unchanged. | |
| 37 | General > tmux: "Open tmux windows as tabs in the attaching window" is a checkbox; "When attaching, open unrecognized windows in" (Windows, Tabs in a new window) is greyed while the box is checked, and unchecking keeps its choice. | |

## Settings rows

| # | Check | Result |
|---|---|---|
| 38 | Settings > Advanced shows the four new rows in their sections, all off: Hotkey (pinned indicator), Tmux Integration (black filler, background), Experimental Features (DECSET 1036). Keys > Remap Modifiers shows Remap modifiers globally, Force keyboard, Only force once per session and Keyboard locale (both greyed until Force keyboard is on) and Respect shortcuts (on). Profiles > Keys shows "Alt Key Sends (Left/Right)", "Left/Right Alt key changeable" and "Left/Right Win Key" enabled, with no "macOS only" hover. | |

## Kept iTerm2 quirks and rulings a person can see

| # | Item | Result |
|---|---|---|
| 39 | Windows' shell chords are the stand-in for macOS symbolic hotkeys (ruling R7): a person's own mapping on one goes to Windows while Respect is on; Tandem's menu commands, stock global mappings and unedited preset entries never yield. | |
| 40 | On AltGr layouts Windows reports Right Alt as Ctrl+Alt, so Alt Key Sends (Right) only matters where Right Alt is plain Alt. | |
| 41 | Bucky bits reach only the kitty protocol, as iTerm2's modern mapper; legacy encodings do not change. An unmapped Win key alone still reports 57444u (iTerm2 would hide it). | |
| 42 | DECSET 1036 follows iTerm2's release default (off): Support DECSET 1036 does nothing until its Advanced key is on. | |
| 43 | A session hotkey needs Ctrl, Alt or Win, is never a chord Windows keeps, and at most 16 are on; a program that clears a person's live hotkey does so without a question. | |
| 44 | The forced layout id is the HKL's two words, so it can differ from a registry layout id for some IMEs; a layout change by the person is noticed one pass late. | |
| 45 | Profile shortcuts on Ctrl+Win+D, F, B, C, Q, N, S, V and 1-9 are stored and shown and do nothing (13t, Windows keeps them). | |
| 46 | Traditional full screen is the only style that does not fade; a failed or refused fade shows the window at once and turns fades off for the run. | |
| 47 | The host-change title offer (`02:1116`) is not in this milestone (14e). | |
