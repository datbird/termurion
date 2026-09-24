# Milestone 3b acceptance checklist

Close any open Tandem window, then run `C:\Users\datbi\src\tandem\target\release\tandem.exe`.
Cmd is the Win key and Option is Alt. Mark each line pass or fail.

| # | Check | Result |
|---|---|---|
| 1 | Settings (Win+,) has Profiles, Keys, and Pointer panes; General, Appearance, Arrangements, and Advanced are grey. | |
| 2 | Profiles > Import from iTerm2... with your Mac's `com.googlecode.iterm2.plist`: the Keys pane then lists your iTerm2 global key bindings, and each profile's Keys tab lists its key mappings. | |
| 3 | Win+1, Win+2, Win+3 switch to tabs 1 to 3; Win+9 goes to the last tab. Set "Shortcut to choose a split pane" to Ctrl+Number: Ctrl+2 focuses the second pane. | |
| 4 | With three tabs, hold Ctrl and press Tab twice, then let go: you land two tabs back in recent order. Ctrl+Tab again returns to the tab you came from. | |
| 5 | Profile Keys tab > Presets > Natural Text Editing > Replace all mappings. In pwsh or zsh, Alt+Left and Alt+Right move by word, Win+Left and Win+Right go to the line start and end, Alt+Backspace deletes a word. | |
| 6 | Profile Keys tab: tick "Delete sends ^H". In `cat -v` (WSL), Backspace shows `^H`. Untick it: `^?`. | |
| 7 | Keys pane > +: record Ctrl+Shift+K, action Send Text, text `echo hi\n`. Pressing Ctrl+Shift+K runs `echo hi`. | |
| 8 | Keys pane > Leader: record Ctrl+B. Add a binding: record Ctrl+B then S (the recorder shows "Leader, S"), action Split Horizontally with Profile, pick a profile. Ctrl+B then S splits; the title shows `[Leader]` between the two keys. Press Ctrl+B, switch to another app and back: `[Leader]` is gone. | |
| 9 | Add a binding Win+J to Next Pane. Win+J cycles panes. Delete the binding: Win+J goes back to Windows. | |
| 10 | Remap Modifiers: Left Control to Left Win. Ctrl+T now opens a tab (as Win+T). On a German or French layout, AltGr still types `@`. Switch to another app while holding Left Control, let go there: no key sticks. Hold Left Control in another app, click into Tandem, keep holding past the key repeat, then let go: nothing stays held. Reset to Defaults. | |
| 11 | Profile Keys tab: Option Key Sends (Left) = Normal or Esc+: Alt+B in zsh moves back a word. Meta: Alt+B sends one byte with the high bit set (`cat -v` shows `M-b`). | |
| 12 | Right-click opens the context menu; in vim with `set mouse=a`, right-click still opens it. Tick "Right click reported to apps": vim now gets the right click. Middle-click pastes; in vim with `mouse=a` vim gets the middle click. With two splits, middle-click the unfocused one: the text lands in the split you clicked. | |
| 13 | Alt-click on the prompt line of pwsh moves the cursor to the click. | |
| 14 | Tick "Focus follows mouse": moving the pointer over a split pane focuses it without a click. | |
| 15 | Add a key binding with Run Sequence: Send Text `ls\n`, then Next Pane. It runs `ls` and moves to the next pane. Toggle Mouse Reporting (bind it to a key) stops vim from getting clicks until pressed again. | |
| 16 | A binding to a later action (for example Start Instant Replay) shows "Start Instant Replay comes in milestone 10." and does nothing else. | |
