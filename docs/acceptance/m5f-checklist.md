# Milestone 5f acceptance checklist

Close any open Tandem window, then run `C:\Users\datbi\src\tandem\target\release\tandem.exe`.
Cmd is the Win key and Option is Alt. The checks use WSL (`wsl`). Mark each line pass or fail.

| # | Check | Result |
|---|---|---|
| 1 | `printf '\e]1337;Block=id=d;attr=start;type=markdown\a# Title\n\nSome *em* and **bold**, `code`, [a link](https://iterm2.com).\n\n- one\n- two\n\e]1337;Block=id=d;attr=end;render=1\a'`: the lines become a framed box with a large bold title, italic and bold words, code in the terminal font, and bullets. The prompt comes back below it. | |
| 2 | The box has a thin border, a slightly grey background, and at its top right a close button, a "Markdown" button, and "[ ] Wide". | |
| 3 | Click the link: a bar asks "Open URL https://iterm2.com?". Yes opens the browser. Click it again and choose Always: the next click opens with no bar. | |
| 4 | Drag across some words in the box: they highlight, and the terminal's own selection goes. Press Win+C and paste in Notepad: the rendered words, without `*` or `#`. | |
| 5 | Edit > Copy with Styles over the same selection, paste in WordPad: the bold and italic words keep their style. | |
| 6 | Hover the box's rows: the block's red bar and pill show; the pill's copy puts the raw markdown (with `#` and `*`) on the clipboard. | |
| 7 | Click the close button: the raw lines come back in place of the box. | |
| 8 | `cat > /tmp/a.py <<'X'` a few lines of Python `X`, then `printf '\e]1337;File=inline=1;name=%s:%s\a' "$(printf a.py | base64)" "$(base64 -w0 /tmp/a.py)"`: the file shows highlighted as Python (keywords, strings, comments in different colors). | |
| 9 | `printf '\e]1337;File=inline=1;type=application/json:%s\a' "$(printf '{"b":true,"a":[1,null]}' | base64)"`: pretty printed JSON, `b` before `a`, `true` (not 1), strings green, numbers red, null grey. | |
| 10 | Click the language button: a list opens with every language; pick "Plain text": the colors go and the box resizes. | |
| 11 | Print a long line of code in a porthole, click "[ ] Wide": the text stops wrapping; Shift+wheel scrolls it sideways. | |
| 12 | Select a few lines of `ls -l` output, Edit > Replace Selection > Render Selection Natively (Win+Alt+E): the whole rows become a porthole. With nothing selected the command beeps. | |
| 13 | Open the find bar (Win+F) and type a word that is in a porthole: it shows yellow in the box. | |
| 14 | Resize the window narrower: the box keeps its height and its text wraps inside it (cut at the bottom when it no longer fits). | |
| 15 | Settings > Advanced > General > Rendered code theme (dark background): point it at a CSS file with `.hljs-keyword { color: #ff0000; font-weight: bold }`; new portholes show keywords red and bold. Point it at a missing file: a banner names the file and the built-in colors draw. | |
| 16 | Settings > Advanced > General > Rendered documents start in Wide mode: turn it on; new portholes start with "[x] Wide". | |
| 17 | Run `vim`, then quit: portholes on the main screen are hidden while vim runs and come back after. | |
