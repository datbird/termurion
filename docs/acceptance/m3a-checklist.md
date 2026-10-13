# Milestone 3a acceptance checklist

Close any open Termurion window, then run `C:\Users\datbi\src\termurion\target\release\tandem.exe`.
Cmd is the Win key. Mark each line pass or fail.

| # | Check | Result |
|---|---|---|
| 1 | The window has iTerm2's menu bar: iTerm2, Shell, Edit, View, Session, Scripts, Profiles, Window, Help. Each command shows its chord (for example `New Tab  Win+T`). Commands for later milestones are grey. | |
| 2 | Win+T opens a tab. Win+W closes the pane. Win+D splits side by side; Win+Shift+D splits top and bottom. | |
| 3 | Win+Alt+arrows move between panes. Win+] and Win+[ cycle panes. Win+Shift+] and Win+Shift+[ switch tabs. | |
| 4 | Windows' own Win chords do not fire in Termurion (Win+D does not show the desktop, Win+T does not jump to the taskbar). The Start menu does not open after them. | |
| 5 | Win+L still locks. In another app, Win chords behave as normal. Win alone still opens Start. | |
| 6 | Win+, opens Settings. Win+O opens Profiles. Win+I (Edit Session) opens Settings on this pane's profile. | |
| 7 | Win+= and Win+- change the text size; Win+0 resets it. | |
| 8 | View > Maximize Active Pane fills the tab with the focused pane; again restores the splits. | |
| 9 | Win+K clears the buffer and keeps the prompt line at the top; the next command prints right under it, not at the bottom. Shell > Save Contents... writes the whole buffer to a file. | |
| 10 | Right-click (in a shell, not in vim) opens the context menu: Copy, Paste, Split, Maximize, Select All, Clear Buffer, Edit Session, Close. | |
| 11 | Run `vim` in WSL: Session > Terminal State shows Alternate Screen checked. Quit vim: unchecked. | |
| 12 | Win+Ctrl+arrows move the divider next to the focused pane. Window > Arrange Windows > Arrange Split Panes Evenly evens them out. | |
| 13 | Session > Reset (in a shell after `printf '\e[?25l'`) brings the cursor back and keeps the text. | |
| 14 | Tap Alt, then type: the letters reach the shell (the menu bar does not take them). In WSL run `htop`: F10 quits htop and does not open the menu bar. | |
| 15 | While bumbles drives the keyboard from the Mac, Win chords (Win+T, Win+D) still reach Termurion. | |
