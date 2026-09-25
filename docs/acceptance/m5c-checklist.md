# Milestone 5c acceptance checklist

Close any open Tandem window, then run `C:\Users\datbi\src\tandem\target\release\tandem.exe`.
Cmd is the Win key and Option is Alt. The checks use WSL (`wsl`). Mark each line pass or fail.

| # | Check | Result |
|---|---|---|
| 1 | `printf '\e]52;c;%s\a' "$(printf hello | base64)"` with clipboard access off: a yellow bar at the top of the pane says the terminal attempted to access the clipboard. Open Settings opens Settings on the General pane. | |
| 2 | Turn on General > Selection > "Applications in terminal may access clipboard" and repeat step 1: no bar, and Ctrl+V in Notepad pastes `hello`. | |
| 3 | Copy some text in Windows. `printf '\e]52;c;?\a'; read -rs -t10 -d '\' x; echo "$x" | cat -v`: a bar asks "Share clipboard contents with app in terminal?". Just Once: the reply appears with the text in base64, and "Clipboard contents reported" shows at the bottom for 3 seconds. Never: later requests get no bar and no reply. | |
| 4 | `printf '\e]1337;CopyToClipboard=\ahello there\e]1337;EndCopy\a'`: the text draws and is on the clipboard. `printf '\e]1337;Copy=:%s\a' "$(printf abc | base64)"`: `abc` is on the clipboard. | |
| 5 | Settings > Advanced > "Disable potentially insecure escape sequences" on: step 4 copies nothing; `printf '\e]1337;Capabilities\a'` gets no answer. Turn it off again. | |
| 6 | `printf '\e]1337;CursorShape=1\a'`: the cursor is a bar. `CursorShape=0` goes back to a block. | |
| 7 | `seq 500; printf '\e]1337;ClearScrollback\a'`: the screen and scrollback clear; the prompt line stays. | |
| 8 | `printf '\e]1337;HighlightCursorLine=yes\a'`: the cursor row gets the cursor guide; `=no` removes it. | |
| 9 | Switch to another window, then from Tandem run `sleep 2; printf '\e]1337;RequestAttention=once\a'`: the taskbar button flashes once. `=yes` flashes until Tandem comes to the front. `=flash`: the pane flashes white. `=fireworks`: sparks burst at the cursor. | |
| 10 | `printf '\e]1337;ReportCellSize\a'; read -rs -t1 -d '\' x; echo "$x" | cat -v` shows `ReportCellSize=<height>;<width>;<scale>`. `ReportWindowFrame` and `ReportScreenFrames` answer with numbers that match the window and monitors (points, y from the bottom). | |
| 11 | `printf '\e]1337;SetWindowFrame=100;100;800;500\a'`: the window moves and resizes (y counts from the bottom of the main monitor). | |
| 12 | `printf '\e]1337;SetColors=bg=202040,red=ff8000,tab=00a0ff\a'; printf '\e[31mred\e[0m\n'`: the background changes, red text is orange, and with two tabs open this tab has a blue stripe. `tab=default` removes the stripe. | |
| 13 | `printf '\e]1337;StealFocus\a'` from a background tab after `sleep 2`: a box asks "A control sequence attempted to activate a session. Allow it?"; Yes brings the tab forward. | |
| 14 | With two tabs, in one run `printf '\e]21337;status=waiting for input;indicator=#ff0000;status-color=#ffcc00\a'`: that tab shows a red dot and yellow "waiting for input" before its title. The next prompt clears it. | |
| 15 | Print colored text (`ls --color=always`), select it, Edit > Copy with Styles, paste into Word or WordPad: colors, bold, and the font come along. Edit > Copy with Control Sequences, paste into Notepad: the text has `ESC[...m` codes. | |
| 16 | Edit > Undo, Redo, and Cut are grayed out while a terminal has focus. | |
| 17 | Shell > Toggle Buffer Input: a pause sign shows at the pane's top right and the menu item is checked. Type `echo hi` and Enter: nothing happens. Toggle it off: `echo hi` runs. Profiles > Terminal > "Buffer Input by Default" on makes new sessions start that way. | |
