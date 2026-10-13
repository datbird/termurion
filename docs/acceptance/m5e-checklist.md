# Milestone 5e acceptance checklist

Close any open Termurion window, then run `C:\Users\datbi\src\termurion\target\release\tandem.exe`.
Cmd is the Win key and Option is Alt. The checks use WSL (`wsl`). Mark each line pass or fail.

| # | Check | Result |
|---|---|---|
| 1 | Run `seq 1 300`, then Session > Make Screenshot...: the pane freezes with a camera sign at its top right, and a "Make Screenshot" panel opens beside the window with a minimap and a line range. Run `seq 1 5` in the shell first in another pane to see the frozen one does not change. | |
| 2 | Drag the range in the minimap and type line numbers in the fields: the preview follows. | |
| 3 | Select some text in the frozen pane, press Redact Selection: the selection goes, the list shows "N lines: “text”", and the preview blurs those cells. Change Redaction method to Solid Color: they turn black. Move the blur radius: the blur follows. | |
| 4 | Select other text, Highlight Selection: a yellow outline with a glow shows around it. Remove one item and Clear All: the list empties. | |
| 5 | Copy to Clipboard: the button reads "Copied!" for about a second; paste into Paint: the image has the pane's full width, no cursor, no selection. | |
| 6 | Save: the panel closes, the pane goes live again, and Explorer shows `Termurion-Screenshot-<date>-<time>.png` on the Desktop. Save As... writes a PNG where you choose. | |
| 7 | Settings > Advanced > General > Screenshot save folder: set a folder; Save writes there. | |
| 8 | Run `seq 1 20000`, Make Screenshot, choose all lines: Copy is off; Save writes `...-part-1-of-N.png` files, each under 30000 px tall. | |
| 9 | `printf '\e]1337;Block=id=b1;attr=start\a'; seq 1 10; printf '\e]1337;Block=id=b1;attr=end\a'`: a bar shows in the left margin next to those rows. Hover them: the bar turns red and a pill with copy and fold buttons shows at the top right of the block. | |
| 10 | Press the pill's copy: a "Copied" toast shows; paste: 1 to 10, one per line. | |
| 11 | Press fold: the block becomes one faint italic line "1 …8 lines… 10" with an outlined bar. Click that line: it unfolds. Click the margin next to the block: it folds. | |
| 12 | `printf '\e]1337;UpdateBlock=id=b1;action=fold\a'` folds it, `action=unfold` unfolds it. | |
| 13 | `printf 'x\e]1337;Button=type=copy;block=b1\ay\n'`: a copy icon shows after x, then 4 spaces, then y. Click it: the block is copied. | |
| 14 | `printf '\e]1337;Button=type=custom;code=5;icon=play.fill\a\n'`: a play icon shows. Run `cat -v`, click the icon: `^[[?1337;5~` shows. Turn on Settings > Advanced > Disable potentially insecure escape sequences: clicking sends nothing. | |
| 15 | `printf '\e]1337;Button=type=custom;code=0\a'`: every custom button so far turns faint and clicks do nothing. | |
