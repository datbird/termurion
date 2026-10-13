# Milestone 1 acceptance checklist

Run `C:\Users\datbi\src\termurion\target\release\tandem.exe` on EXTREME. Mark each line pass or fail,
with a note on anything that feels unlike iTerm2.

| # | Check | iTerm2 behavior it copies | Result |
|---|---|---|---|
| 1 | Start `tandem.exe`. A window opens with a PowerShell prompt. | New window opens a shell | |
| 2 | Type `dir` and press Enter. Output appears in iTerm2's default colors. | Default profile colors | |
| 3 | Ctrl+Shift+D splits right. Ctrl+Shift+Alt+D splits down. The inactive pane is dimmed. | Cmd+D, Cmd+Shift+D, dim inactive splits | |
| 4 | Ctrl+Shift+] and Ctrl+Shift+[ move between panes. Ctrl+Shift+Alt+arrows move by direction. | Cmd+], Cmd+[, Cmd+Option+arrows | |
| 5 | Drag a divider. The panes resize and the text reflows. | Drag split dividers | |
| 6 | Ctrl+Shift+T opens a tab and the tab bar appears. Click a tab to switch. Ctrl+Shift+1 selects the first tab. | Cmd+T, Cmd+1 | |
| 7 | Drag to select text; it is copied at once. Double-click selects a word, triple-click a line. Ctrl+Alt-drag selects a rectangle. | Copy on selection, smart clicks, Cmd+Option-drag | |
| 8 | Middle-click pastes. Ctrl+Shift+V pastes. | Middle-click paste, Cmd+V | |
| 9 | In `wsl`, run `vim` and `:set mouse=a`. Clicks move the cursor. Alt-drag still selects text. | Option-drag overrides mouse reporting | |
| 10 | In `wsl`, run `tmux` with `set -g mouse on`. The wheel scrolls tmux history; a click selects a pane. | Mouse reporting to tmux | |
| 11 | In `wsl`, run `htop`. Clicking a column header sorts by it. | Mouse reporting | |
| 12 | In a plain shell, the wheel scrolls the history. Shift+PageUp and Shift+PageDown page through it. | Scrollback | |
| 13 | Type `exit` in a pane; it closes. The last pane closes its tab; the last tab closes the window. | Session ends, pane closes | |

Not in milestone 1 (later milestones): your iTerm2 profile colors and font, the right-click menu,
quad-click smart selection, Ctrl-click to open links, the hotkey window, triggers, and tmux -CC.

## Deferred review items for milestone 2

The final review's Critical and Important items are fixed. These smaller items wait:

1. Banners ("Copy failed", "GPU validation error") never clear, no space is reserved for a pane error, and dividers and dimming draw over the banner.
2. `Pane::pump` drains all waiting output in one go. Heavy output could delay drawing. Add a budget.
3. The dimming of an unfocused pane leaves its 4 px padding undimmed.
4. The window title is set on every frame.
5. A divider drag keeps its path if the layout changes during the drag.
6. If the GPU offers no sRGB format, colors draw too dark.
7. The block cursor is one cell wide on a wide character. The divider gap does not scale with the display.
8. With very many tabs, a tab can get narrower than one cell.
9. Not in scope yet: IME (CJK typing), focus reporting (DECSET 1004), the right-click menu.
