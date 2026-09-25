# Milestone 4c acceptance checklist

Close any open Tandem window, then run `C:\Users\datbi\src\tandem\target\release\tandem.exe`.
Cmd is the Win key and Option is Alt. Mark each line pass or fail.

| # | Check | Result |
|---|---|---|
| 1 | Run `dir C:\Windows`. Edit > Copy Mode (Win+Shift+C): a small white flag shows under the cursor. `k`, `j`, `h`, `l`, `w`, `b`, `0`, `$`, `g`, `G` move it; `5k` moves 5 rows up. The view follows it. | |
| 2 | In Copy Mode press `v`, move, and the selection grows; `V` selects whole lines; Ctrl+V selects a box; `o` swaps the ends. The flag turns light blue while selecting. `y` copies and leaves Copy Mode; paste in Notepad to check. `q` or Esc leaves without copying. | |
| 3 | Select a word with the mouse, then press Shift+Left: Copy Mode starts at the start of the selection and the selection grows left. Turn off Settings > Pointer > Selection > Automatically enter copy mode: Shift+Left now goes to the shell. | |
| 4 | Keys pane: bind a key to "Copy Mode Commands" with `v$y`: pressing it copies from the cursor to the end of its line. Bind "Move End of Selection Right" with `3`: the selection grows by 3 characters. | |
| 5 | `echo {"a":1,"b":[2,3]}`, triple-click that output line, Edit > Replace Selection > Replace with Pretty-Printed JSON: the line becomes indented JSON. Nothing is typed into the shell. Base 64 encode then decode brings the text back. | |
| 6 | Select several output lines, Edit > Fold Selected Lines (Win+Shift+L): they become one grey italic line like `first …5 lines… last`. Click it: the lines come back. Fold again, right-click the fold: Unfold. | |
| 7 | Run a few commands with shell integration, then Edit > Fold All: everything above the prompt folds into one line with the time and the first and last commands. | |
| 8 | Select a line in the middle, Edit > Clear to Start of Selection (from the menu; Win+L stays with Windows as Lock): everything above it is gone, the rest stays. | |
| 9 | Edit > Marks and Annotations > Add Annotation at Cursor (Win+Alt+M): a small window opens; type a note. The text at the cursor gets a yellow underline and the note shows beside it. Click the underlined text: the note opens again. Delete asks "Really remove annotation?". | |
| 10 | Select text, Annotate Selection. Then Next Annotation and Previous Annotation jump between notes. View > Show Annotations (Win+\\) hides all notes (underlines stay), and again shows them. | |
| 11 | In WSL: `printf '\e]1337;AddAnnotation=5|hello\a'; echo world`: `world` gets an annotation reading hello. | |
| 12 | View > Show Timestamps (Win+Shift+E): each line shows the time it was written at the right. Lines older than a day show the weekday. Profile Session tab > Timestamps: "Over contents when mouse hovers" shows them only with the pointer at the right edge. | |
| 13 | View > Show Cursor Guide (Win+Alt+;): a light blue band marks the cursor row. Profile Colors > Cursor Guide Color changes it. The toggle is per session. | |
| 14 | View > Find Cursor (Win+/): the window dims except a clear circle at the cursor for about a second; holding Win keeps it up until Win comes up. | |
| 15 | View > Show Session Note (Win+Alt+G): a Session Note window opens; text typed there is still there after closing and reopening it (for this session). | |
| 16 | Set two named marks (Win+Alt+Shift+M). Edit > Named Marks lists them by name, top first; picking one scrolls to it. | |
| 17 | Tick Edit > Selection Respects Soft Boundaries. In tmux with two side-by-side panes, double-click a word next to the divider: the selection stops at the `│`. | |
